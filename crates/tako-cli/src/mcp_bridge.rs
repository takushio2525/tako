//! MCP の stdio ブリッジ（FR-2.3.2 のゼロコンフィグ接続を成立させる実体）。
//!
//! 1 行 1 JSON の MCP メッセージを stdin から読み、プロトコル処理は
//! `tako_control::mcp`（HTTP トランスポートと共有）に任せ、操作の実行だけ
//! IPC へ origin="mcp" で中継する。呼び出し元ペインは環境変数から特定する。
//!
//! ## 常駐の軽さ（#2021）
//!
//! claude（codex / agy）1 本につき 1 本、claude が終わるまで常駐する。#2021 の前は 1 本の
//! footprint が約 5.7〜7 MB で、中身のほとんどは「解放したのに OS へ返らない malloc の山」と
//! 「使わないフレームワークの像」だった（実測と比べた案は `.agent/plans/2026-10-lightweight.md` §11）。
//!
//! - **clap を通らない**: `tako` は引数がちょうど `mcp serve` のときコマンド定義の木を組まない
//!   （組むだけで約 2 MB の山が残る）
//! - **カタログを常駐プロセスで組まない**: tools/list の応答（約 200 KB）と許可パラメータの表は、
//!   同じ実行ファイルを使い捨ての子プロセス（カタログ係 = [`CATALOG_HELPER_ENV`]）として起こして
//!   組ませ、出力をそのまま流す（組むと約 2.7 MB の山が残る）。カタログ係も同じ
//!   `handle_message` で応答を作るので、**応答のバイト列は変わらない**
//! - **macOS の .app では専用の小さな実行ファイルへ引き渡す**: 登録のコマンドは `tako mcp serve`
//!   のまま、`tako` が同じ `Contents/MacOS` の `tako-mcp`（libSystem だけをリンク）へ exec する。
//!   `tako` がリンクする CoreFoundation / IOKit / CoreServices / libobjc の約 300 個の像を持たない。
//!   登録を変えないので、既存の登録（user スコープ・プロジェクトの `.mcp.json`・codex / agy）は
//!   移行なしでそのまま軽くなる。引き渡すときは malloc を解放したページを返す側に倒して起こす
//!   （[`MALLOC_TUNING_ENV`]）
//!
//! `TAKO_2021_LEGACY=1` で 3 つとも止まる（同じバイナリの A/B）。前後の計測は
//! `scripts/measure-mcp-bridge-2021.sh`。

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::OnceLock;

use serde_json::Value;
use tako_control::mcp::ParamsTable;
use tako_control::protocol::Request;

use crate::ipc::{caller_pane, outside_tako, resolve_caller_pane_via_app, send_request_via};

/// カタログ係として起こされた印（env。値は問わない）
pub const CATALOG_HELPER_ENV: &str = "TAKO_MCP_CATALOG_HELPER";

/// 引き渡し先の実行ファイル名（.app の `Contents/MacOS` で `tako` の隣に置く）
pub const SMALL_BRIDGE_NAME: &str = "tako-mcp";

/// 引き渡すときだけ `tako-mcp` へ渡す malloc の設定（macOS の libmalloc が起動時に読む env）。
///
/// 応答（数 KB〜数十 KB）を組んでは捨てる小さな割り付けが、CPU ごとの malloc の区画へ散って
/// 解放後も dirty のまま残り、tools/call を数百本流すと footprint が約 0.75 MB 伸びる
/// （実測: 2000 本で 1.84 MB）。これを立てると解放したページを OS へ返す側に倒れて 1.12 MB で
/// 止まる。代わりの CPU は 2000 本で +0.04 秒（1 本あたり約 20 µs）。
/// `tako-mcp` は起動直後にこれを env から消す（[`forget_malloc_tuning`]）ので、ブリッジが
/// 起こす子（カタログ係・受け入れゲートのコマンド）には渡らない
pub const MALLOC_TUNING_ENV: &str = "MallocSpaceEfficient";

/// [`MALLOC_TUNING_ENV`] を自分の env から消す（libmalloc は起動時に読み終えているので、
/// 自分の malloc の設定はそのまま。子へ渡さないため）。`tako-mcp` の main の先頭で、
/// スレッドを起こす前に呼ぶ
pub fn forget_malloc_tuning() {
    std::env::remove_var(MALLOC_TUNING_ENV);
}

/// `TAKO_2021_LEGACY=1` で #2021 の前の形（clap を通り、カタログを常駐プロセスで組み、
/// 引き渡さない）へ戻す
pub fn legacy() -> bool {
    matches!(std::env::var("TAKO_2021_LEGACY").as_deref(), Ok("1"))
}

/// 引数がちょうど `mcp serve` か（登録のコマンドは常にこの形 = `dispatch::mcp_target_path`）。
/// 余分な引数（`--help` 等）が付いたときは今までどおり clap へ回す
pub fn is_mcp_serve_invocation(mut args: impl Iterator<Item = OsString>) -> bool {
    matches!(
        (args.next(), args.next(), args.next()),
        (Some(a), Some(b), None) if a == "mcp" && b == "serve"
    )
}

/// カタログ（tools/list の応答と許可パラメータの表）をどこで組むか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Catalog {
    /// 使い捨ての子プロセス（カタログ係）に組ませる（#2021 の既定）
    Helper,
    /// 常駐プロセスの中で組む（#2021 の前の形。`TAKO_2021_LEGACY=1` の A/B）。
    /// カタログ係が使えないとき（起こせない・出力が壊れている）も 1 件ずつここへ倒す
    InProcess,
}

/// ブリッジの入口（`tako mcp serve` と `tako-mcp` で共有）。カタログ係として
/// 起こされたときはその仕事だけをして終わる
pub fn run() -> ExitCode {
    let result = if std::env::var_os(CATALOG_HELPER_ENV).is_some() {
        catalog_helper()
    } else {
        serve(Catalog::Helper)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            // std の `eprintln!` は書けないと panic する（読み手が閉じた stderr = #1758）。
            // 終わり方を「エラー終了 1」のまま保つため、書けなくても捨てる
            let _ = writeln!(std::io::stderr(), "error: {message}");
            ExitCode::FAILURE
        }
    }
}

/// macOS の .app に入っている `tako` なら、同じ dir の `tako-mcp` へ exec で引き渡す。
/// **戻ってきたら引き渡さなかった**（.app の外・`tako-mcp` が無い・exec の失敗・Windows）ので、
/// 呼び手はそのまま同じプロセスでブリッジを続ける。pid は変わらない（呼び出し元ペインの
/// pid 祖先辿り = #986 はそのまま効く）。
///
/// スレッドを起こす前に呼ぶこと（exec は他のスレッドを黙って消す）
pub fn handoff_to_small_bridge() {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let Some(small) = small_bridge_path() else {
            return;
        };
        let err = Command::new(&small)
            .args(["mcp", "serve"])
            .env(MALLOC_TUNING_ENV, "1")
            .exec();
        tako_control::diag::persist_log(&format!(
            "mcp serve: {} へ引き渡せない（{err}）。同じプロセスでブリッジを続ける（#2021）",
            small.display()
        ));
    }
}

/// 引き渡し先（.app の外・置かれていなければ `None`）
fn small_bridge_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // Homebrew の `bin/tako` は .app の中への symlink なので、実体の置き場で判定する
    let exe = tako_core::platform::path::canonicalize_or_self(&exe);
    small_bridge_beside(&exe).filter(|path| path.is_file())
}

/// `<名前>.app/Contents/MacOS/tako` の隣の `tako-mcp`（.app の外なら `None`）。
///
/// **.app の中に限る**: .app は 2 つを同じビルドから組んで置く（`scripts/build-app.sh`）ので
/// 版が揃っている。dev の `target/` は `cargo build --bin tako` だけでも古い `tako-mcp` が
/// 残り得て、引き渡すと古い版のブリッジが黙って動く（その場合は同じプロセスで続ける）
pub fn small_bridge_beside(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let in_bundle = macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension().is_some_and(|ext| ext == "app");
    in_bundle.then(|| macos.join(SMALL_BRIDGE_NAME))
}

/// ツール公開の判定は**環境変数のみ**で行う（発見ファイルは見ない）。
/// tako の外で起動された Claude セッションへツールを公開しない方針（FR-2.3.2 の
/// 「tako 外で 0 ツール」）を保つため。tako 内で起動された長寿命ブリッジが
/// アプリ再起動で stale になった場合のみ、exec 時にファイルへフォールバックする
fn connected_from_env() -> bool {
    matches!(
        (std::env::var("TAKO_SOCKET"), std::env::var("TAKO_TOKEN")),
        (Ok(s), Ok(t)) if !s.is_empty() && !t.is_empty()
    )
}

/// ブリッジが途中で終わる理由（#1805）
enum Stop {
    /// クライアントが stdout の読み手を閉じた（= 切断）。tako の失敗ではないので、stdin の
    /// EOF と同じく静かに 0 で終える。#1805 の前は `error: stdout への書き込みに失敗:
    /// Broken pipe` を出して 1 で終わっていた
    ClientGone,
    /// それ以外の失敗（`error: …` を出して 1）
    Failed(String),
}

impl Stop {
    /// stdout への書き込み（flush を含む）の失敗を分ける。読み手が閉じたのは macOS / Linux の
    /// EPIPE も Windows の `ERROR_NO_DATA` も `ErrorKind::BrokenPipe` で来る
    fn stdout(e: std::io::Error, what: &str) -> Self {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            Stop::ClientGone
        } else {
            Stop::Failed(format!("{what}: {e}"))
        }
    }
}

impl From<String> for Stop {
    fn from(message: String) -> Self {
        Stop::Failed(message)
    }
}

/// ブリッジ本体。stdin が閉じるまで 1 行ずつ応答する。
///
/// 終わり方: stdin の EOF → 0 / クライアントが stdout の読み手を先に閉じた → 0（#1805。
/// 痕跡は persist.log へ 1 行。stderr も閉じていることが多く、出しても読み手が居ない）/
/// それ以外の失敗 → `Err`（呼び手が `error: …` を出して 1）
pub fn serve(catalog: Catalog) -> Result<(), String> {
    match serve_until_end(catalog) {
        Ok(()) => Ok(()),
        Err(Stop::ClientGone) => {
            tako_control::diag::persist_log(&format!(
                "mcp serve: クライアントが stdout を閉じたので終わる（切断。pid={}）",
                std::process::id()
            ));
            Ok(())
        }
        Err(Stop::Failed(message)) => Err(message),
    }
}

fn serve_until_end(catalog: Catalog) -> Result<(), Stop> {
    let connected = connected_from_env();
    // 呼び出し元ペインの決め方は純粋関数で決めてから実行する（#986）
    let plan = caller_pane_plan(caller_pane(), connected, legacy_no_pid_fallback());
    let caller = match plan {
        CallerPanePlan::FromEnv(pane) => Some(pane),
        CallerPanePlan::ResolveByPid => {
            // env が届かない系統（agy / 親 env を渡さない MCP クライアント）向けの
            // pid 祖先辿り。アプリ側の `Request::ResolvePane` = #288 / #567 と同じ 1 実装
            let resolved = resolve_caller_pane_via_app(None)
                .ok()
                .flatten()
                .map(|c| (c.pane, c.method.unwrap_or_else(|| "unknown".into())));
            match resolved {
                Some((pane, method)) => {
                    tako_control::diag::persist_log(&format!(
                        "mcp serve: caller_pane を pid 祖先辿りで解決（pane={pane} method={method} pid={}）",
                        std::process::id()
                    ));
                    Some(pane)
                }
                None => {
                    tako_control::diag::persist_log(&format!(
                        "mcp serve: caller_pane を解決できない（TAKO_PANE_ID 無し + pid 祖先辿りも不一致。pid={}）",
                        std::process::id()
                    ));
                    None
                }
            }
        }
        CallerPanePlan::Unresolved => None,
    };
    // 前段の断り（ペイン未指定 = #2358・引数の型や不足・未知のパラメータ = #2365）は
    // このプロセスが組むので、その文の表示言語を設定から決める。#2358 は呼び出し元が
    // 決まっていれば読まなかったが、#2365 で引数の断りも日英になり、呼び出し元の有無に
    // 依らず起きるようになった。読むのは起動時の 1 回だけ（settings.json の言語の欄）
    tako_core::i18n::set_lang(tako_control::settings::peek_lang_setting().resolve());
    let caller_role = std::env::var("TAKO_ORCHESTRATOR_ROLE").ok();
    if catalog == Catalog::Helper {
        // 未知パラメータの検出（#227）の表も常駐プロセスで組まない
        tako_control::mcp::set_params_table_source(params_table_from_helper);
    }

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| format!("stdin の読み取りに失敗: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(message) => {
                if catalog == Catalog::Helper
                    && connected
                    && is_tools_list_request(&message)
                    && relay_tools_list(&line, &mut stdout)?
                {
                    continue;
                }
                let mut exec = |request: Request| -> Result<Value, String> {
                    if connected {
                        send_request_via(request, Some("mcp"))
                    } else {
                        Err(outside_tako())
                    }
                };
                let mut session = tako_control::mcp::McpSession {
                    caller_pane: caller,
                    caller_role: caller_role.clone(),
                    connected,
                    exec: &mut exec,
                };
                tako_control::mcp::handle_message(&message, &mut session)
            }
            Err(e) => Some(serde_json::json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {
                    "code": -32700,
                    "message": tako_control::Reason::NotJson(e.to_string()).to_string(),
                },
            })),
        };
        if let Some(response) = response {
            writeln!(stdout, "{response}")
                .map_err(|e| Stop::stdout(e, "stdout への書き込みに失敗"))?;
            stdout
                .flush()
                .map_err(|e| Stop::stdout(e, "stdout の flush に失敗"))?;
        }
    }
    Ok(())
}

/// 応答を返す tools/list か（id の無い notification は応答しないので、カタログも要らない）
fn is_tools_list_request(message: &Value) -> bool {
    message.get("method").and_then(Value::as_str) == Some("tools/list")
        && message.get("id").is_some_and(|id| !id.is_null())
}

/// tools/list の応答をカタログ係に作らせ、その行をバイト列のまま流す。
/// `Ok(false)` = 何も書いていない（呼び手が同じプロセスで作る）
fn relay_tools_list(line: &str, out: &mut impl Write) -> Result<bool, Stop> {
    let Some(mut helper) = spawn_catalog_helper(Some(line)) else {
        return Ok(false);
    };
    let Some(pipe) = helper.stdout.take() else {
        finish_helper(helper);
        return Ok(false);
    };
    let mut reader = BufReader::new(pipe);
    // 1 行目 = 許可パラメータの表。ここで受け取っておけば最初の tools/call で起こし直さずに済む
    let mut table = String::new();
    if !matches!(reader.read_line(&mut table), Ok(n) if n > 0) {
        finish_helper(helper);
        return Ok(false);
    }
    if let Ok(table) = serde_json::from_str::<ParamsTable>(&table) {
        let _ = PARAMS_TABLE.set(Some(table));
    }
    // 2 行目 = 応答の行。自分のヒープへ載せずに固定長の窓で流す（約 200 KB を抱えない）
    let mut window = [0u8; 16 * 1024];
    let mut written = 0usize;
    let mut last = b'\n';
    loop {
        let n = match reader.read(&mut window) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        // 切り出し安全: `[u8]` の窓を read が埋めた長さで切るだけ（文字列ではない）
        out.write_all(&window[..n])
            .map_err(|e| Stop::stdout(e, "stdout への書き込みに失敗"))?;
        written += n;
        last = window[n - 1];
    }
    let status = helper.wait();
    if written == 0 {
        log_helper_trouble("tools/list の応答が空だった");
        return Ok(false);
    }
    if last != b'\n' {
        // 途中で切れた（ここへ来るのはカタログ係が書きかけで落ちたときだけ）。行の区切りは保つ
        out.write_all(b"\n")
            .map_err(|e| Stop::stdout(e, "stdout への書き込みに失敗"))?;
    }
    if !matches!(status, Ok(status) if status.success()) {
        log_helper_trouble("tools/list を流した後にカタログ係が失敗で終わった");
    }
    out.flush()
        .map_err(|e| Stop::stdout(e, "stdout の flush に失敗"))?;
    Ok(true)
}

/// カタログ係から受け取った許可パラメータの表（受け取れなかったら `None` = 同じプロセスで組む）
static PARAMS_TABLE: OnceLock<Option<ParamsTable>> = OnceLock::new();

/// [`tako_control::mcp::set_params_table_source`] へ渡す出どころ
fn params_table_from_helper() -> Option<&'static ParamsTable> {
    PARAMS_TABLE.get_or_init(fetch_params_table).as_ref()
}

/// カタログ係を起こして表だけを受け取る（tools/list より先に tools/call が来たとき）
fn fetch_params_table() -> Option<ParamsTable> {
    let mut helper = spawn_catalog_helper(None)?;
    let mut line = String::new();
    let read = match helper.stdout.take() {
        Some(pipe) => BufReader::new(pipe).read_line(&mut line).ok(),
        None => None,
    };
    finish_helper(helper);
    let table = read
        .filter(|n| *n > 0)
        .and_then(|_| serde_json::from_str(&line).ok());
    if table.is_none() {
        log_helper_trouble("許可パラメータの表を受け取れない");
    }
    table
}

/// カタログ係（自分と同じ実行ファイル）を起こし、`message_line` を 1 行だけ渡して stdin を閉じる
fn spawn_catalog_helper(message_line: Option<&str>) -> Option<Child> {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            log_helper_trouble(&format!("自分の実行ファイルの場所が分からない（{e}）"));
            return None;
        }
    };
    let mut command = Command::new(exe);
    command
        .args(["mcp", "serve"])
        .env(CATALOG_HELPER_ENV, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // Windows でコンソールの窓を出させない（#586）
    tako_core::platform::process::no_console_window(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            log_helper_trouble(&format!("起こせない（{e}）"));
            return None;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        if let Some(line) = message_line {
            // 書けなくても続ける（カタログ係は stdin の終わりで仕事を終えるので、応答が
            // 空になって呼び手が同じプロセスで作る側へ倒れる）
            let _ = writeln!(stdin, "{line}");
        }
    }
    Some(child)
}

/// 出力を読み終えた（読むのをやめた）カタログ係を片付ける
fn finish_helper(mut helper: Child) {
    let _ = helper.kill();
    let _ = helper.wait();
}

fn log_helper_trouble(what: &str) {
    tako_control::diag::persist_log(&format!(
        "mcp serve: カタログ係（#2021）: {what}。同じプロセスで組む側へ倒す"
    ));
}

/// カタログ係（#2021）: 許可パラメータの表を 1 行目に、stdin で渡された MCP メッセージの
/// 応答を 2 行目以降に書いて終わる。常駐のブリッジと同じ `handle_message` で作るので、
/// 応答のバイト列は同じプロセスで作ったときと変わらない
fn catalog_helper() -> Result<(), String> {
    let connected = connected_from_env();
    let mut out = serde_json::to_string(&ParamsTable::from_catalog(&tako_control::mcp::tools()))
        .map_err(|e| format!("許可パラメータの表を書けない: {e}"))?;
    out.push('\n');
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| format!("stdin の読み取りに失敗: {e}"))?;
    for line in input.lines().filter(|line| !line.trim().is_empty()) {
        let message: Value =
            serde_json::from_str(line).map_err(|e| format!("JSON として解釈できない: {e}"))?;
        // 渡されるのは tools/list だけで、操作の中継は常駐のブリッジが持つ
        let mut exec = |_: Request| -> Result<Value, String> {
            Err("カタログ係は操作を中継しない（#2021）".into())
        };
        let mut session = tako_control::mcp::McpSession {
            caller_pane: None,
            caller_role: None,
            connected,
            exec: &mut exec,
        };
        if let Some(response) = tako_control::mcp::handle_message(&message, &mut session) {
            out.push_str(&response.to_string());
            out.push('\n');
        }
    }
    // 組み終えてから 1 度に書く（書きかけで落ちて、半端な行を流させない）
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(out.as_bytes())
        .and_then(|()| stdout.flush())
        .map_err(|e| format!("stdout への書き込みに失敗: {e}"))
}

/// MCP stdio ブリッジが呼び出し元ペインをどう決めるか（Issue #986）。
///
/// **判断を純粋関数に置く**ので、env とアプリを触らずに新旧どちらの形も検査できる
/// （env グローバルを触るテストは並列で競合する。#608 / #807）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CallerPanePlan {
    /// `TAKO_PANE_ID` が届いている（claude / codex の `env_vars` 経路）
    FromEnv(u64),
    /// env が届かないので pid 祖先辿りでアプリに問う（agy / 親 env を渡さない
    /// クライアント。#987 の agy master もここで解ける）
    ResolveByPid,
    /// tako の外（接続情報が無い = tools/list が 0 件になるので問う相手も居ない）
    Unresolved,
}

/// 解決順を決める。**env が在れば 1 バイトも変えない**（claude 経路への回帰ゼロ）。
/// `legacy` は #986 の A/B（`TAKO_986_LEGACY=1` で pid 祖先辿りを止める）
fn caller_pane_plan(env_pane: Option<u64>, connected: bool, legacy: bool) -> CallerPanePlan {
    match env_pane {
        Some(pane) => CallerPanePlan::FromEnv(pane),
        // 接続情報が無ければ tako の外なので、IPC を張らない（FR-2.3.2）
        None if !connected || legacy => CallerPanePlan::Unresolved,
        None => CallerPanePlan::ResolveByPid,
    }
}

/// #986 の A/B 用の env。`TAKO_986_LEGACY=1` で**同一バイナリのまま**
/// 「caller_pane は `TAKO_PANE_ID` だけで決める」旧挙動へ戻す
fn legacy_no_pid_fallback() -> bool {
    std::env::var("TAKO_986_LEGACY")
        .map(|v| v == "1")
        .unwrap_or(false)
}

#[cfg(test)]
mod issue986_caller_pane {
    use super::*;

    /// 受け入れ条件 3: `TAKO_PANE_ID` が在るときは 1 バイトも変わらない
    /// （claude / codex の `env_vars` 経路は IPC を 1 往復も増やさない）
    #[test]
    fn env_が在れば_env_をそのまま使う() {
        assert_eq!(
            caller_pane_plan(Some(42), true, false),
            CallerPanePlan::FromEnv(42)
        );
        // 接続情報の有無・A/B にも影響されない（env が最優先）
        assert_eq!(
            caller_pane_plan(Some(42), false, true),
            CallerPanePlan::FromEnv(42)
        );
    }

    /// 受け入れ条件 2: env が無いときは pid 祖先辿りへ落ちる（agy / #987 の経路）
    #[test]
    fn env_が無ければ_pid_祖先辿りへ落ちる() {
        assert_eq!(
            caller_pane_plan(None, true, false),
            CallerPanePlan::ResolveByPid
        );
    }

    /// 受け入れ条件 4（FR-2.3.2 の回帰）: tako の外では IPC を張らない。
    /// `connected` が false = `TAKO_SOCKET` / `TAKO_TOKEN` が無い状態で、
    /// ここで問い合わせに行くと tako 外の Claude セッションから見える挙動が変わる
    #[test]
    fn tako_の外では問い合わせに行かない() {
        assert_eq!(
            caller_pane_plan(None, false, false),
            CallerPanePlan::Unresolved
        );
    }

    /// A/B: `TAKO_986_LEGACY=1` 相当で pid 祖先辿りが止まる（旧挙動 = env だけ）
    #[test]
    fn legacy_なら_pid_祖先辿りをしない() {
        assert_eq!(
            caller_pane_plan(None, true, true),
            CallerPanePlan::Unresolved
        );
    }
}

#[cfg(test)]
mod issue2021_bridge {
    use super::*;

    fn args(list: &[&str]) -> impl Iterator<Item = OsString> {
        list.iter()
            .map(OsString::from)
            .collect::<Vec<_>>()
            .into_iter()
    }

    /// 早い分岐はちょうど `mcp serve` のときだけ（`--help` 等は clap の案内を保つ）
    #[test]
    fn 早い分岐はちょうど_mcp_serve_のときだけ() {
        assert!(is_mcp_serve_invocation(args(&["mcp", "serve"])));
        assert!(!is_mcp_serve_invocation(args(&["mcp", "serve", "--help"])));
        assert!(!is_mcp_serve_invocation(args(&["mcp"])));
        assert!(!is_mcp_serve_invocation(args(&["serve", "mcp"])));
        assert!(!is_mcp_serve_invocation(args(&[])));
        assert!(!is_mcp_serve_invocation(args(&["list"])));
    }

    /// 引き渡すのは .app の `Contents/MacOS` の中だけ（dev の target/ では古い tako-mcp が
    /// 残り得るので引き渡さない）
    #[test]
    fn 引き渡し先は_app_の中だけ() {
        assert_eq!(
            small_bridge_beside(Path::new("/Applications/tako.app/Contents/MacOS/tako")),
            Some(PathBuf::from(
                "/Applications/tako.app/Contents/MacOS/tako-mcp"
            ))
        );
        // 置き場の名前が違えば .app とみなさない
        for exe in [
            "/repo/target/release/tako",
            "/repo/target/debug/tako",
            "/opt/homebrew/bin/tako",
            "/x/tako.app/Contents/Resources/tako",
            "/x/tako/Contents/MacOS/tako",
            "/tako",
        ] {
            assert_eq!(small_bridge_beside(Path::new(exe)), None, "{exe}");
        }
    }
}
