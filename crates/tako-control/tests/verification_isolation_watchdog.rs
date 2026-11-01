//! 番犬: **検証プロセスは本番の個人ファイルへ書かない**（Issue #1253 / #944 / #1030）
//!
//! #944 は「置き場を決める側を倒す」で `cargo test` を塞いだが、判定の一部が
//! `cfg(test)` のままだった。GUI セルフテストは `cargo test` ではなく**製品バイナリ**が
//! `TAKO_SELF_TEST=1` で立つので、そこには 1 ビットも効かない —— 実測では
//! `~/.claude.json` のテスト残骸 3,050 件のうち約 70% がこの経路だった。
//!
//! したがって検査するのは「書かないこと」だけでなく**判定の形**そのもの:
//! 書き先を決める関数が `cfg(test)` ではなく実行時判定
//! （`tako_core::paths::is_verification_process`）を引いていること。
//! 挙動側の番犬は `tako_control::test_write_isolation`（空 HOME で子を回す）と
//! `tako-app` の `update_checker`（偽 brew を PATH へ置く）にある。
//!
//! **#1970**: 検証は本番 tako のペインから起こされるので、本番の env（制御プレーンと mod）を
//! 受け継ぐ。隔離 GUI の入口（`scripts/lib/isolated-gui.sh`）がそれを GUI へ渡さず・自動リネームを
//! 止め・`CLAUDE_CONFIG_DIR` を一時の側へ向けること（本番がペインへ配る名前は本番側の正本から
//! 集めて突き合わせる）と、検証スクリプトが自分のプロセスで claude / tako を起こす前に
//! 落としていること（`rel:line` で名指す）を拘束する。
//!
//! **#2527**: env を落としても tako CLI は発見ファイル（既定の data dir の `control.json`）から
//! 本番 GUI へ繋ぐ。`test-serve-watch.sh` の `tako remote start` が QR の画像を本番のアクティブな
//! タブへ開いた。tako CLI を起こす行は、発見の置き場（`TAKO_DATA_DIR` / `TAKO_DISCOVERY_DIR` /
//! `HOME`）を一時の側へ向けていることも拘束する（`# tako:prod-ok <理由>` の行だけ外す）。
//!
//! **#2598**: 上の走査は `scripts/test-*.sh` などの検証スクリプトだけを読み、検証スクリプトが source する
//! ヘルパ（`scripts/lib/*.sh`）を読んでいなかった。呼び手から見ればヘルパは関数呼び出しなので、
//! `wait_isolated_gui` の中の素の `"$TAKO_BIN" list` は呼び手の env（本番の `TAKO_SOCKET` / 既定の置き場）で
//! 本番 GUI へ届いた（PR #2596 の検証で 1 回）。ヘルパの関数は呼び手の env で走るので、ヘルパの tako を
//! 起こす行は**その行で**本番の env を落とし発見の置き場を向けていることを拘束し（ファイル先頭の
//! `unset` / `export` は数えない）、`wait_isolated_gui` が `iso_cli` を通して起こした GUI と同じ置き場へ
//! 繋ぐことをヘルパを実際に走らせて見る。
//!
//! **#2627**: 隔離 GUI のペインはログイン zsh で、HOME を本物のままにした検証では利用者の rc（pyenv の
//! 初期化等）を読んでいた。GUI の終了でペインへ HUP が重ねて届き、rc の `pyenv rehash` の EXIT の trap が
//! 途中で断たれて shim のロックが利用者の環境に残った（以後のログイン zsh が 60 秒待つ）。ヘルパが既定で
//! ペインの zsh を砂場の ZDOTDIR へ向けること（並べた値より後ろに置く・判定を呼ぶ）を file:line で、
//! 向け方の規則をヘルパを走らせて拘束し、利用者の rc を読ませる opt-in（`TAKO_ISO_USER_RC=1`）には
//! 行ごとに理由（`# tako:user-rc-ok <理由>`）を求める。

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルートを解決できない")
        .to_path_buf()
}

/// `fn <名前>(` から対応する `}` までの本体を返す（波括弧の対応で切る）
fn fn_body<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let at = text.find(&format!("fn {name}("))?;
    let open = text[at..].find('{')? + at;
    let mut depth = 0usize;
    for (i, c) in text[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[open..open + i + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

#[path = "common/code_view.rs"]
mod code_view;

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

/// **肯定の存在確認**（「この呼び出しが在る」）が見る眺め = コメントを落とした本文。
///
/// 全文へ `contains` すると、走査先の doc コメントに書いた同じ綴りで真になり、
/// 実体が消えても緑のままになる（#1609）。`.zshenv` などの非 Rust は全文のまま
fn read_code(rel: &str) -> String {
    code_view::without_comments_checked(&read(rel), rel)
}

/// 外部エージェントの設定の書き先は**実行時判定**で倒す。
///
/// `cfg(test)` だと GUI セルフテスト（製品バイナリ）が素通りする = #1253 の本体
#[test]
fn 設定の書き先は実行時判定で隔離される() {
    for (rel, func) in [
        (
            "crates/tako-control/src/orchestrator/mod.rs",
            "agent_config_home",
        ),
        ("crates/tako-control/src/claude_tui.rs", "env_config_dir"),
    ] {
        let text = read_code(rel);
        let body =
            fn_body(&text, func).unwrap_or_else(|| panic!("{rel}: fn {func} が見つからない"));
        assert!(
            body.contains("is_verification_process"),
            "{rel}: {func} が実行時判定（paths::is_verification_process）を引いていない（#1253）"
        );
        assert!(
            !body.contains("cfg(test)"),
            "{rel}: {func} が cfg(test) で隔離している。\
             GUI セルフテストは製品バイナリなので効かない（#1253）"
        );
    }
}

/// テストプロセスから brew を起こさない（#1253。#944 の `claude agents --json` と同型）。
///
/// `cargo test --workspace` がユーザーの `~/Library/Caches/Homebrew` へ
/// 698 ファイル作っていた（空 HOME で実測）
#[test]
fn 配布系統の判別はテストプロセスで外部コマンドを起こさない() {
    let rel = "crates/tako-app/src/update_checker.rs";
    let text = read_code(rel);
    let body = fn_body(&text, "detect_install_method_full").expect("判別関数が見つからない");
    let guard = body.find("is_test_process").unwrap_or_else(|| {
        panic!("{rel}: detect_install_method_full にテストプロセスの門番が無い（#1253）")
    });
    let probe = body.find("is_brew_available").unwrap_or_else(|| {
        panic!("{rel}: brew の実行が見当たらない（実装が変わったら番犬も直す）")
    });
    assert!(
        guard < probe,
        "{rel}: brew を起こした後で門番を通している（順番が逆だと外部コマンドが走る。#1253）"
    );
}

/// 対話 zsh の履歴は **rc の後に**当て直す（#1253）。
///
/// macOS の `/etc/zshrc` は `HISTFILE=${ZDOTDIR:-$HOME}/.zsh_history` を無条件で
/// 代入するので、環境変数を渡すだけでは対話シェルに効かない（実測で確認した）
#[test]
fn 検証の履歴はrcの後に当て直される() {
    let rel = "crates/tako-core/shell-integration/zshenv.zsh";
    // zsh は Rust ではないので眺めを通さない（`#` コメントは code_view の対象外）
    let text = read(rel);
    assert!(
        text.contains("TAKO_VERIFY_HISTFILE"),
        "{rel}: 検証用の履歴の書き先を当て直す仕掛けが無い（#1253）"
    );
    assert!(
        text.contains("precmd_functions+=(_tako_verify_histfile)"),
        "{rel}: 当て直しが precmd に載っていない。\
         .zshenv での代入は /etc/zshrc に潰される（#1253）"
    );
    // ペイン ID で絞ると、履歴を汚していた素の PTY（TAKO_PANE_ID を持たない）が漏れる
    let at = text
        .find("TAKO_VERIFY_HISTFILE-")
        .expect("ガード行が見つからない");
    let guard_line = text[..at].rsplit('\n').next().unwrap_or("");
    assert!(
        !guard_line.contains("TAKO_PANE_ID"),
        "{rel}: 当て直しを TAKO_PANE_ID で絞っている。\
         履歴を汚していたのはペイン ID を持たない素の PTY（#1253）"
    );
}

/// エージェント CLI の**問い合わせ起動**は正本の門番（`agent_probe::run`）を通る（#1261）。
///
/// tako 側が 1 バイトも書かなくても、実 CLI は起動しただけで自分のホームを作る
/// （実測: 空 HOME で `cargo test --workspace` → `~/.gemini` 32 ファイル /
/// `~/.codex/tmp/arg0/…/.lock` / `~/.claude.json` と `~/.claude/backups/`）。
/// 呼ぶ側それぞれに `if` を散らすと**新しい問い合わせを足したときに素通りする**ので、
/// 起動は 1 か所を通す形そのものを拘束する
#[test]
fn エージェントcliの問い合わせ起動は正本の門番を通る() {
    for (rel, func) in [
        // #1505 で認証とプランの問い合わせが 1 本になった（`is_authenticated_for` は
        // ここへ委譲するだけ）ので、門番を通る位置もこちら
        (
            "crates/tako-control/src/setup_bootstrap.rs",
            "auth_state_for",
        ),
        // 環境診断の経路の上限つき版（#2255。上限を引数に取る `agent_probe::output_within` を通る）
        (
            "crates/tako-control/src/setup_bootstrap.rs",
            "auth_state_within",
        ),
        // claude の MCP 登録の読み取り（#1505 で CLI から移った）。
        // こちらは上限つきの口（`agent_probe::output_with_timeout`）を通る
        // （`claude_health_with` / `claude_health_now` はここへ委譲するだけ）
        (
            "crates/tako-control/src/agent_mcp.rs",
            "claude_health_probe",
        ),
        ("crates/tako-control/src/agent_models.rs", "catalog"),
        (
            "crates/tako-control/src/stale_binary.rs",
            "extract_version_via_cli",
        ),
    ] {
        let text = read_code(rel);
        let body =
            fn_body(&text, func).unwrap_or_else(|| panic!("{rel}: fn {func} が見つからない"));
        assert!(
            body.contains("agent_probe::run")
                || body.contains("agent_probe::output_with_timeout")
                || body.contains("agent_probe::output_within"),
            "{rel}: {func} が正本の門番（agent_probe::run / output_with_timeout / output_within）を\
             通っていない。テストプロセスから実 claude / codex / agy が起きる（#1261）"
        );
        assert!(
            !body.contains(".output()"),
            "{rel}: {func} が子プロセスを直に起こしている。\
             起動は agent_probe::run へ寄せる（#1261）"
        );
    }
}

/// 門番自身は**実行時**にテストプロセスを見分け、判定してから起こす（#1261）。
///
/// `cfg(test)` だと統合テスト（`crates/*/tests/`）から呼ばれた lib は非テストビルドなので
/// 素通りする —— #1253 が GUI セルフテストで踏んだのと同じ穴
#[test]
fn 問い合わせの門番はテストプロセスを実行時に見分ける() {
    let rel = "crates/tako-control/src/agent_probe.rs";
    let text = read_code(rel);
    let gate = fn_body(&text, "blocked").expect("fn blocked が見つからない");
    assert!(
        gate.contains("is_test_process"),
        "{rel}: blocked が実行時判定（paths::is_test_process）を引いていない（#1261）"
    );
    assert!(
        !gate.contains("cfg(test)"),
        "{rel}: blocked が cfg(test) で判定している。\
         クレートを跨がないので統合テストが素通りする（#1261）"
    );
    let body = fn_body(&text, "run").expect("fn run が見つからない");
    // 呼ぶ側から `no_console_window` を取り上げた以上、内側で必ず当てる（#586 / #628）。
    // `platform_parity::コンソール窓を抑止していない子プロセス起動が増えていない` は
    // `agent_probe::run` を抑止済みとして数えるので、その前提をここで押さえる
    assert!(
        body.contains("no_console_window"),
        "{rel}: run が #586 のコンソール窓抑止を当てていない（#1261）"
    );
    let guard = body.find("blocked()").expect("run が門番を呼んでいない");
    let launch = body.find(".output()").expect("run が起動していない");
    assert!(
        guard < launch,
        "{rel}: 起こした後で門番を通している（順番が逆だと外部コマンドが走る。#1261）"
    );
}

// ------------------------------------------------------------------------------------------
// #1970: 隔離 GUI と検証スクリプトは本番 tako の env を持ち込まない
//
// 検証は本番 tako のペイン（= worker の claude の中）から起こされるので、何もしなければ
// 本番の制御プレーン（TAKO_SOCKET / TAKO_TOKEN / TAKO_PANE_ID …）と本番の mod
// （CLAUDE_CODE_PLUGIN_DIRS / TAKO_CLI）を丸ごと受け継ぐ。#1966 の worker は隔離 GUI の
// 自動リネームが本物の claude を起こし、本番の mod が一時の claude に載る形を踏んだ。
// 拘束するのは 2 か所: 隔離 GUI の入口（scripts/lib/isolated-gui.sh）が GUI へ渡さないこと、
// 検証スクリプトが自分のプロセスで claude / tako を起こす前に落としていること。
// ------------------------------------------------------------------------------------------

const ISOLATED_GUI_REL: &str = "scripts/lib/isolated-gui.sh";

/// Issue #1970 が列挙した「本番 tako が配る env」。ヘルパの一覧の下限で、検証スクリプトが
/// **claude** を起こす前に落としていなければならない組でもある
const ISSUE_1970_KEYS: &[&str] = &[
    "TAKO_SOCKET",
    "TAKO_PANE_ID",
    "TAKO_TAB_ID",
    "TAKO_TOKEN",
    "TAKO_CLI",
    "TAKO_MCP_URL",
    "CLAUDE_CODE_PLUGIN_DIRS",
    "TAKO_ORCHESTRATOR_ROLE",
];

/// 検証スクリプトが **tako CLI** を起こす前に落としていなければならない組。
/// AGENTS.md の `env -u TAKO_SOCKET -u TAKO_PANE_ID` と同じ（ソケットが無ければトークンは効かない）。
/// 残すと CLI は本番の GUI へ繋がり、本番のペインを自分として扱う（#1449 / #1450）
const TAKO_CLI_KEYS: &[&str] = &["TAKO_SOCKET", "TAKO_PANE_ID"];

/// 検証スクリプトが **tako CLI** を起こす前に、どれか 1 つを一時の側へ向けていなければならない名前（#2527）。
///
/// `TAKO_SOCKET` を落としても CLI は止まらない: 環境変数が無ければ発見ファイル
/// （`<TAKO_DISCOVERY_DIR か data dir>/control.json` と `instances/`）を読み、既定の置き場
/// （`$HOME/Library/Application Support/tako`）に居る本番 GUI へ繋ぐ（FR-2.2.9）。
/// `TAKO_ISOLATED=1` は CLI の data dir を変えない（#2531 から印のある CLI は既定の置き場の発見ファイルを
/// 読まずに断るが、印を付けない行・順番待ち・data dir の設定ファイルは既定の置き場のまま）。#2527 では
/// `test-serve-watch.sh` の `tako remote start` が QR の画像を本番のアクティブなタブへ開いた
const DISCOVERY_KEYS: &[&str] = &["TAKO_DATA_DIR", "TAKO_DISCOVERY_DIR", "HOME"];

/// ヘルパの `ISOLATED_GUI_DROP_ENV=( … )` に並んだ名前（コメント行と行末コメントは除く）
fn helper_drop_env(src: &str) -> Vec<String> {
    let start = src
        .find("\nISOLATED_GUI_DROP_ENV=(")
        .unwrap_or_else(|| panic!("{ISOLATED_GUI_REL}: ISOLATED_GUI_DROP_ENV=( が消えている"));
    let body = &src[start..];
    let open = body.find('(').expect("開き括弧") + 1;
    let close = body.find("\n)").unwrap_or_else(|| {
        panic!("{ISOLATED_GUI_REL}: ISOLATED_GUI_DROP_ENV の閉じ括弧（行頭の `)`）が見つからない")
    });
    let names: Vec<String> = body[open..close]
        .lines()
        .map(|l| l.split('#').next().unwrap_or(""))
        .flat_map(str::split_whitespace)
        .map(str::to_string)
        .collect();
    assert!(
        names.len() >= ISSUE_1970_KEYS.len(),
        "{ISOLATED_GUI_REL}: ISOLATED_GUI_DROP_ENV が {} 個しか読めない（範囲取りが壊れている）",
        names.len()
    );
    names
}

/// 本番の tako がペインへ配る env の名前（**本番側の正本から集める**。増やした名前を
/// ヘルパへ足し忘れたらここで落ちる）
fn production_pane_env() -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    // 制御プレーン: tako-app の spawn_session が `("TAKO_…".into(), …)` で積む名前
    let rel = "crates/tako-app/src/main.rs";
    let text = read_code(rel);
    let body = fn_body(&text, "spawn_session")
        .unwrap_or_else(|| panic!("{rel}: fn spawn_session が見つからない"));
    let mut rest = body;
    while let Some(at) = rest.find("(\"TAKO_") {
        let tail = &rest[at + 2..];
        let end = tail.find('"').expect("閉じ引用符");
        if tail[end..].starts_with("\".into()") {
            keys.push(tail[..end].to_string());
        }
        rest = &tail[end..];
    }
    for want in ["TAKO_SOCKET", "TAKO_TOKEN", "TAKO_MCP_URL", "TAKO_PANE_ID"] {
        assert!(
            keys.iter().any(|k| k == want),
            "{rel}: spawn_session から {want} を読めない（積み方が変わったら番犬も直す）。読めた名前: {keys:?}"
        );
    }
    // 器の中のシェルへ固定する名前・tako mod・シェル統合の控え（tako 固有のものだけ）
    keys.extend(
        tako_core::backend::PANE_SCOPED_ENV
            .iter()
            .chain(tako_core::claude_mod::INJECTED_KEYS)
            .map(|k| k.to_string()),
    );
    keys.extend(
        tako_core::shell_integration::INJECTED_KEYS
            .iter()
            .filter(|k| k.starts_with("TAKO_"))
            .map(|k| k.to_string()),
    );
    // #1998: cargo の共有 jobserver（spawn_session が `jobserver::pane_env()` で積む名前）。受け継ぐと隔離 GUI の
    // ペインの cargo が本番の札で絞られる（A/B の測定が本番の札の空きに左右される）
    keys.push(tako_core::jobserver::ENV.to_string());
    keys.extend(ISSUE_1970_KEYS.iter().map(|k| k.to_string()));
    keys.sort();
    keys.dedup();
    keys
}

#[test]
fn 隔離guiのヘルパは本番がペインへ配るenvをすべて落とす() {
    let src = read(ISOLATED_GUI_REL);
    let drop = helper_drop_env(&src);
    let missing: Vec<String> = production_pane_env()
        .into_iter()
        .filter(|k| !drop.contains(k))
        .collect();
    assert!(
        missing.is_empty(),
        "{ISOLATED_GUI_REL}: ISOLATED_GUI_DROP_ENV に本番 tako がペインへ配る {missing:?} が無い。\
         隔離 GUI とその子（ペイン・自動リネームの claude・mod）が本番の GUI / mod へ繋がる（#1970）"
    );
    // 起動の行が一覧から `env -u` を組み、既定（自動リネーム停止・一時の設定 dir）を
    // 呼び出し側の `VAR=VAL`（"$@"）より前に置いていること
    let code: String = src
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    for needle in [
        "for key in ${ISOLATED_GUI_DROP_ENV[@]+\"${ISOLATED_GUI_DROP_ENV[@]}\"}",
        "drop_args+=(-u \"$key\")",
        "\"TAKO_AUTO_RENAME=${TAKO_AUTO_RENAME:-0}\"",
        "\"CLAUDE_CONFIG_DIR=${claude_config}\"",
        "iso_default_zshrc \"$@\"",
    ] {
        assert!(
            code.contains(needle),
            "{ISOLATED_GUI_REL}: launch_isolated_gui に `{needle}` が無い（#1970 の既定が外れている）"
        );
    }
    let launch = code
        .lines()
        .find(|l| l.contains("\"$APP_BIN\"") && l.trim_start().starts_with("env "))
        .unwrap_or_else(|| panic!("{ISOLATED_GUI_REL}: GUI を起こす env の行が見つからない"));
    let pos = |s: &str| {
        launch
            .find(s)
            .unwrap_or_else(|| panic!("{ISOLATED_GUI_REL}: 起動の行に {s} が無い: {launch}"))
    };
    // 配列は bash 3.2 の慣用句（空でも set -u で落ちない）で展開する（shell_scripts の番犬と同じ）
    assert!(
        pos("${drop_args[@]+\"${drop_args[@]}\"}") < pos("${env_args[@]+\"${env_args[@]}\"}")
            && pos("${env_args[@]+\"${env_args[@]}\"}") < pos("\"$@\""),
        "{ISOLATED_GUI_REL}: 起動の行は `env -u …`（落とす）→ 既定 → 呼び出し側の `VAR=VAL` の順に並べる\
         （並べた値が既定に勝ち、落とした名前も並べれば届く。#1970）: {launch}"
    );
}

// ---- 検証スクリプトの走査 -------------------------------------------------------------

/// 走査で見つけた「claude / tako を起こすコマンド」1 つ
#[derive(Debug)]
struct Launch {
    /// 1 始まりの行番号（継続行はその先頭の行）
    line: usize,
    /// "claude" か "tako"
    what: &'static str,
    /// 起こすコマンドの単語（引用符つきのまま。`"$BIN/claude"` 等）
    cmd: String,
    /// 同じコマンドの `env -u K`（`env -i` なら全部）で落としている名前
    env_drops: Vec<String>,
    env_clear: bool,
    /// 同じコマンドの前置き（`K=V cmd` / `env K=V cmd`）で差し替えている名前（#2527）
    env_sets: Vec<String>,
}

/// 走査の結果: 起こすコマンドの一覧と、ファイル全体の `unset` で落としている名前・
/// `export` で子へ渡している名前
#[derive(Debug, Default)]
struct ScriptScan {
    launches: Vec<Launch>,
    unset: Vec<String>,
    /// `export K=V` / `export K` / `declare -x K` と、常に export 済みの `HOME` への代入（#2527）
    exported: Vec<String>,
    /// このスクリプトが `…/tako` のパスを代入した変数（`OLD=/Applications/…/MacOS/tako` 等）。
    /// `"$OLD" file copy …` も tako CLI を起こす行として数える（#2527）
    tako_vars: Vec<String>,
    /// 監査済みの例外宣言 `# tako:prod-ok <理由>` が効く行（宣言の行とその次の行。理由の無い宣言は無効）。
    /// 本番の GUI へ届くのが仕様の行だけに付ける（本番の順番待ちへ並ぶ `tako queue` = #1994）。
    /// 外れるのは発見の置き場の規則だけ（env を落とす規則は外れない）
    prod_ok: Vec<usize>,
}

/// `# tako:prod-ok <理由>` の宣言が効く行番号（1 始まり。宣言の行とその次の行）
fn prod_ok_lines(src: &str) -> Vec<usize> {
    let mut out = Vec::new();
    for (i, l) in src.lines().enumerate() {
        let declared = l
            .split("# tako:prod-ok")
            .nth(1)
            .is_some_and(|reason| !reason.trim().is_empty());
        if declared {
            out.extend([i + 1, i + 2]);
        }
    }
    out
}

/// `/tako` で終わるパスを代入した変数の名前（`NAME=…/tako` / `export NAME="${X:-…/tako}"` / `local NAME=…`）
fn tako_path_vars(src: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in src.lines() {
        let mut rest = line.trim_start();
        for kw in ["export ", "local ", "readonly "] {
            if let Some(r) = rest.strip_prefix(kw) {
                rest = r.trim_start();
            }
        }
        let Some(eq) = rest.find('=') else { continue };
        let name = &rest[..eq];
        if name.is_empty()
            || !name
                .chars()
                .enumerate()
                .all(|(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
        {
            continue;
        }
        let value = rest[eq + 1..].split([' ', '\t', ';']).next().unwrap_or("");
        let value = unquote(value);
        if value.trim_end_matches('}').ends_with("/tako") && !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    }
    out
}

fn is_assignment(word: &str) -> bool {
    let name_end = word.find(['=', '+']).unwrap_or(0);
    name_end > 0
        && word[name_end..].trim_start_matches('+').starts_with('=')
        && word[..name_end]
            .chars()
            .enumerate()
            .all(|(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
}

/// 単語の囲みの引用符を外す（`"$TAKO_BIN"` → `$TAKO_BIN`）
fn unquote(word: &str) -> String {
    word.chars().filter(|c| *c != '"' && *c != '\'').collect()
}

/// 代入の単語（`K=V` / `K+=V` / `export` の引数の `K`）の名前
fn assigned_name(word: &str) -> String {
    let bare = unquote(word);
    let end = bare.find(['=', '+']).unwrap_or(bare.len());
    bare[..end].to_string()
}

/// 1 つのコマンド（単語の列）から、起こすもの・`env -u`・`unset`・`export` を読む
fn classify(words: &[String], line: usize, scan: &mut ScriptScan) {
    let mut i = 0;
    let mut env_drops = Vec::new();
    let mut env_clear = false;
    let mut env_sets = Vec::new();
    loop {
        let Some(w) = words.get(i) else {
            // 代入だけの文。`HOME` は常に export 済みなので、代入がそのまま子へ渡る（#2527）
            if env_sets.iter().any(|k| k == "HOME") {
                scan.exported.push("HOME".to_string());
            }
            return;
        };
        let bare = unquote(w);
        // 前置き（代入・exec 等）は読み飛ばす。`command -v` は在るかの確認なので読み飛ばさない
        let lookup = bare == "command" && words.get(i + 1).is_some_and(|n| n.starts_with('-'));
        // 後ろに単語が続く配列の展開（`${extra[@]+"${extra[@]}"} "$BIN/claude"`）は前置きの
        // `VAR=VAL` の束として読み飛ばす（コマンドは後ろの単語）
        let spread = bare.starts_with("${") && bare.contains("[@]") && words.len() > i + 1;
        if is_assignment(w) {
            env_sets.push(assigned_name(w));
            i += 1;
        } else if matches!(bare.as_str(), "exec" | "nohup" | "time" | "!")
            || (bare == "command" && !lookup)
            || spread
        {
            i += 1;
        } else if bare == "env" {
            i += 1;
            while let Some(opt) = words.get(i) {
                let opt = unquote(opt);
                if opt == "-u" {
                    if let Some(k) = words.get(i + 1) {
                        env_drops.push(unquote(k));
                    }
                    i += 2;
                } else if let Some(k) = opt.strip_prefix("--unset=") {
                    env_drops.push(k.to_string());
                    i += 1;
                } else if opt == "-i" || opt == "-" {
                    env_clear = true;
                    i += 1;
                } else if opt.starts_with('-') {
                    i += 1;
                } else {
                    break;
                }
            }
        } else {
            break;
        }
    }
    let Some(raw) = words.get(i) else { return };
    let cmd = unquote(raw);
    if cmd == "unset" {
        scan.unset.extend(
            words[i + 1..]
                .iter()
                .map(|w| unquote(w))
                .filter(|w| !w.starts_with('-')),
        );
        return;
    }
    let args = &words[i + 1..];
    let declares_export = matches!(cmd.as_str(), "declare" | "typeset")
        && args.iter().any(|a| {
            let a = unquote(a);
            a.starts_with('-') && a.contains('x')
        });
    if cmd == "export" || declares_export {
        scan.exported.extend(
            args.iter()
                .filter(|w| !unquote(w).starts_with('-'))
                .map(|w| assigned_name(w)),
        );
        return;
    }
    // パスで起こす claude（`"$BIN/claude"` = 実物を PATH の外から symlink で置く形）も数える
    let what = if cmd == "claude" || cmd.ends_with("/claude") {
        // 版の問い合わせは会話も mod も MCP も起こさない（mod の下限の判定で使う）
        if words
            .get(i + 1)
            .is_some_and(|a| matches!(unquote(a).as_str(), "--version" | "-v"))
        {
            return;
        }
        "claude"
    } else if matches!(
        cmd.as_str(),
        "tako" | "$TAKO_BIN" | "${TAKO_BIN}" | "$TAKO" | "${TAKO}"
    ) || cmd.ends_with("/tako")
        || scan
            .tako_vars
            .iter()
            .any(|v| cmd == format!("${v}") || cmd == format!("${{{v}}}"))
    {
        // 版・使い方の表示は clap がその場で返す（GUI へも data dir へも行かない）。
        // サブコマンドの使い方（`tako queue --help` = queue.sh が順番待ちを知っているかを見る形）も同じ
        if args.first().is_some_and(|a| {
            matches!(
                unquote(a).as_str(),
                "--version" | "-V" | "--help" | "-h" | "help"
            )
        }) || args.iter().any(|a| unquote(a) == "--help")
        {
            return;
        }
        "tako"
    } else {
        return;
    };
    scan.launches.push(Launch {
        line,
        what,
        cmd: raw.clone(),
        env_drops,
        env_clear,
        env_sets,
    });
}

/// シェルの 1 行（継続行は連結済み）を、**このスクリプトのプロセスで走るコマンド**の単語の列へ分ける。
///
/// 引用符の中は単語の一部（`tk send "claude …"` はペインで走る = 隔離 GUI の env を継ぐ）。
/// `$( … )` は引用符の中でもこのプロセスで走るので、中身を別のコマンドとして読む。
/// `case` のパターン（`claude)`）はコマンドにしない
struct ShellLine<'a> {
    chars: Vec<char>,
    i: usize,
    out: &'a mut Vec<Vec<String>>,
}

impl ShellLine<'_> {
    /// コマンドの並びを読む。`nested` なら対応する `)` で戻る
    fn list(&mut self, nested: bool) {
        let mut words: Vec<String> = Vec::new();
        // 読みかけの単語（None = 単語の外。`#` が行コメントか単語の一部かの分かれ目）
        let mut word: Option<String> = None;
        macro_rules! end_word {
            () => {
                if let Some(w) = word.take() {
                    words.push(w);
                }
            };
        }
        macro_rules! end_cmd {
            () => {
                end_word!();
                if !words.is_empty() {
                    self.out.push(std::mem::take(&mut words));
                }
            };
        }
        // `|` 以外の区切りの直後で数えたコマンドの数。対応の無い `)`（case のパターン）を見たら
        // ここまで戻す = `tako | default)` の `|` で並べた選択肢をコマンドとして数えない（#2598）
        let mut pattern_start = self.out.len();
        while self.i < self.chars.len() {
            let c = self.chars[self.i];
            match c {
                ' ' | '\t' => {
                    end_word!();
                    // 予約語は区切り（`if claude …` / `then "$TAKO_BIN" …` / `{ … }`）
                    if words.len() == 1
                        && matches!(
                            words[0].as_str(),
                            "if" | "then" | "else" | "elif" | "do" | "while" | "until" | "{"
                        )
                    {
                        words.clear();
                    }
                    self.i += 1;
                }
                '#' if word.is_none() => break,
                ';' | '&' | '|' | '\n' => {
                    end_cmd!();
                    if c != '|' {
                        pattern_start = self.out.len();
                    }
                    self.i += 1;
                }
                '(' => {
                    // `tk() {` の関数定義・`arr=(…)` の配列・`( … )` の副シェル
                    end_cmd!();
                    self.i += 1;
                    self.list(true);
                    pattern_start = self.out.len();
                }
                ')' => {
                    self.i += 1;
                    if nested {
                        end_cmd!();
                        return;
                    }
                    // 対応の無い `)` は case のパターン = コマンドではない（`|` で並べた選択肢も）
                    self.out.truncate(pattern_start);
                    words.clear();
                    word = None;
                }
                '\\' => {
                    let w = word.get_or_insert_with(String::new);
                    w.push(c);
                    if let Some(n) = self.chars.get(self.i + 1) {
                        w.push(*n);
                    }
                    self.i += 2;
                }
                '\'' => {
                    let w = word.get_or_insert_with(String::new);
                    w.push(c);
                    self.i += 1;
                    while self.i < self.chars.len() {
                        let q = self.chars[self.i];
                        w.push(q);
                        self.i += 1;
                        if q == '\'' {
                            break;
                        }
                    }
                }
                '"' => {
                    let mut w = word.take().unwrap_or_default();
                    w.push(c);
                    self.i += 1;
                    self.double_quoted(&mut w);
                    word = Some(w);
                }
                '$' if self.chars.get(self.i + 1) == Some(&'(') => {
                    word.get_or_insert_with(String::new).push_str("$(…)");
                    self.substitution();
                }
                '`' => {
                    // 逆引用符の置換は使っていない（読み飛ばす）
                    word.get_or_insert_with(String::new);
                    self.i += 1;
                    while self.i < self.chars.len() && self.chars[self.i] != '`' {
                        self.i += 1;
                    }
                    self.i += 1;
                }
                _ => {
                    let w = word.get_or_insert_with(String::new);
                    w.push(c);
                    self.i += 1;
                    // `}` だけの単語は区切り（関数の本体の括り）
                    if w == "}"
                        && self
                            .chars
                            .get(self.i)
                            .is_none_or(|n| n.is_whitespace() || *n == ';')
                    {
                        word = None;
                        end_cmd!();
                    }
                }
            }
        }
        end_cmd!();
    }

    /// `"…"` の中（開きの `"` は読み済み）。`$( … )` だけはコマンドとして読む
    fn double_quoted(&mut self, word: &mut String) {
        while self.i < self.chars.len() {
            let c = self.chars[self.i];
            match c {
                '\\' => {
                    word.push(c);
                    if let Some(n) = self.chars.get(self.i + 1) {
                        word.push(*n);
                    }
                    self.i += 2;
                }
                '"' => {
                    word.push(c);
                    self.i += 1;
                    return;
                }
                '$' if self.chars.get(self.i + 1) == Some(&'(') => {
                    word.push_str("$(…)");
                    self.substitution();
                }
                _ => {
                    word.push(c);
                    self.i += 1;
                }
            }
        }
    }

    /// `$(` の位置から。`$((` は算術なので読み飛ばし、`$(` はコマンドの並びとして読む
    fn substitution(&mut self) {
        if self.chars.get(self.i + 2) == Some(&'(') {
            let mut depth = 0usize;
            while self.i < self.chars.len() {
                match self.chars[self.i] {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            self.i += 1;
                            return;
                        }
                    }
                    _ => {}
                }
                self.i += 1;
            }
            return;
        }
        self.i += 2;
        self.list(true);
    }
}

/// ヒアドキュメントの終わりの印（`<<'EOF'` / `<<-EOF` / `<<"EOF"`。`<<<` は対象外）
fn heredoc_marker(line: &str) -> Option<(String, bool)> {
    let bytes = line.as_bytes();
    let mut at = 0;
    while let Some(off) = line[at..].find("<<") {
        let p = at + off;
        at = p + 2;
        if bytes.get(p + 2) == Some(&b'<') || (p > 0 && bytes[p - 1] == b'<') {
            at = p + 3;
            continue;
        }
        let mut rest = &line[p + 2..];
        let strip_tabs = rest.starts_with('-');
        if strip_tabs {
            rest = &rest[1..];
        }
        let rest = rest.trim_start().trim_start_matches(['\'', '"', '\\']);
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        // 算術のシフト（`$((1 << 2))`）を印と読まない = 名前は英字か `_` で始まる
        if name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
            return Some((name, strip_tabs));
        }
    }
    None
}

/// シェルスクリプト 1 本を走査する（ヒアドキュメントの本文は読まない = 書き出すスタブの中身）
fn scan_script(src: &str) -> ScriptScan {
    let mut scan = ScriptScan {
        tako_vars: tako_path_vars(src),
        prod_ok: prod_ok_lines(src),
        ..ScriptScan::default()
    };
    let lines: Vec<&str> = src.lines().collect();
    let mut idx = 0;
    while idx < lines.len() {
        let start = idx;
        let mut logical = String::new();
        // 継続行（行末の `\`）を 1 行へ
        loop {
            let l = lines[idx];
            idx += 1;
            if let Some(cut) = l.strip_suffix('\\') {
                logical.push_str(cut);
                logical.push(' ');
                if idx < lines.len() {
                    continue;
                }
            } else {
                logical.push_str(l);
            }
            break;
        }
        let mut cmds = Vec::new();
        ShellLine {
            chars: logical.chars().collect(),
            i: 0,
            out: &mut cmds,
        }
        .list(false);
        for words in &cmds {
            classify(words, start + 1, &mut scan);
        }
        if let Some((end, strip_tabs)) = heredoc_marker(&logical) {
            while idx < lines.len() {
                let l = lines[idx];
                idx += 1;
                let l = if strip_tabs {
                    l.trim_start_matches('\t')
                } else {
                    l
                };
                if l == end {
                    break;
                }
            }
        }
    }
    // パスで起こす claude のうち、このスクリプトが `>` で書き出したもの（偽の claude のスタブ）は
    // 本物ではないので数えない。symlink で置いた実物（`ln -s`）は書き出さないので残る
    scan.launches.retain(|l| {
        l.what != "claude"
            || unquote(&l.cmd) == "claude"
            || ![">", ">>", "> ", ">> "]
                .iter()
                .any(|redirect| src.contains(&format!("{redirect}{}", l.cmd)))
    });
    scan
}

/// 落としていない名前を `rel:line` で並べる（起こすもの 1 種につき最初の 1 行と件数）
fn launch_violations(rel: &str, scan: &ScriptScan) -> Vec<String> {
    let mut out = Vec::new();
    for (what, required) in [("claude", ISSUE_1970_KEYS), ("tako", TAKO_CLI_KEYS)] {
        let bad: Vec<(&Launch, Vec<&str>)> = scan
            .launches
            .iter()
            .filter(|l| l.what == what && !l.env_clear)
            .map(|l| {
                let missing: Vec<&str> = required
                    .iter()
                    .copied()
                    .filter(|k| {
                        !scan.unset.iter().any(|u| u == k) && !l.env_drops.iter().any(|u| u == k)
                    })
                    .collect();
                (l, missing)
            })
            .filter(|(_, m)| !m.is_empty())
            .collect();
        if let Some((first, missing)) = bad.first() {
            out.push(format!(
                "{rel}:{}: {what} を起こす前に {missing:?} を落としていない（同じ形が {} 行）",
                first.line,
                bad.len()
            ));
        }
    }
    // tako CLI は env を落としても既定の置き場の発見ファイルから本番 GUI を見つける（#2527）
    let blind: Vec<&Launch> = scan
        .launches
        .iter()
        .filter(|l| l.what == "tako" && !l.env_clear && !scan.prod_ok.contains(&l.line))
        .filter(|l| {
            !DISCOVERY_KEYS
                .iter()
                .any(|k| scan.exported.iter().any(|e| e == k) || l.env_sets.iter().any(|e| e == k))
        })
        .collect();
    if let Some(first) = blind.first() {
        out.push(format!(
            "{rel}:{}: tako を起こす前に発見の置き場（{}）を一時の側へ向けていない\
             （同じ形が {} 行。env を落としても CLI は既定の置き場の control.json から本番 GUI へ繋ぐ）",
            first.line,
            DISCOVERY_KEYS.join(" / "),
            blind.len()
        ));
    }
    out
}

/// 検証スクリプト（`scripts/test-*.sh` / `measure-*.sh` / `verify-*.sh`）
fn verification_scripts() -> Vec<String> {
    let dir = repo_root().join("scripts");
    let mut out: Vec<String> = std::fs::read_dir(&dir)
        .expect("scripts/ を読む")
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| {
            n.ends_with(".sh")
                && (n.starts_with("test-") || n.starts_with("measure-") || n.starts_with("verify-"))
        })
        .map(|n| format!("scripts/{n}"))
        .collect();
    out.sort();
    assert!(
        out.len() >= 100,
        "検証スクリプトが {} 本しか見つからない（走査範囲の取り方が壊れている）",
        out.len()
    );
    out
}

#[test]
fn 検証スクリプトは本番のenvを落としてからclaudeとtakoを起こす() {
    let mut violations = Vec::new();
    let mut launching = 0usize;
    for rel in verification_scripts() {
        let scan = scan_script(&read(&rel));
        if !scan.launches.is_empty() {
            launching += 1;
        }
        violations.extend(launch_violations(&rel, &scan));
    }
    // 走査が何も拾わなくなったら、どんな漏れでも通る
    assert!(
        launching >= 50,
        "claude / tako を起こす検証スクリプトが {launching} 本しか見つからない（走査が壊れている）"
    );
    assert!(
        violations.is_empty(),
        "本番 tako の env を落とさずに claude / tako を起こす検証スクリプトがある（#1970 / #2527）。\
         先頭で `unset {}` するか、その行を `env -u …` で起こす（tako CLI だけなら TAKO_SOCKET / TAKO_PANE_ID）。\
         tako CLI は加えて発見の置き場を一時の側へ向ける（先頭で `export TAKO_DATA_DIR=\"$TMP/data\"` など。\
         {} のどれか）:\n  {}",
        ISSUE_1970_KEYS.join(" "),
        DISCOVERY_KEYS.join(" / "),
        violations.join("\n  ")
    );
}

// ---- ヘルパ（scripts/lib/*.sh）の走査（#2598）-------------------------------------------

/// 検証スクリプトが source するヘルパ（`scripts/lib/*.sh`）
fn helper_scripts() -> Vec<String> {
    let dir = repo_root().join("scripts/lib");
    let mut out: Vec<String> = std::fs::read_dir(&dir)
        .expect("scripts/lib/ を読む")
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| n.ends_with(".sh"))
        .map(|n| format!("scripts/lib/{n}"))
        .collect();
    out.sort();
    assert!(
        out.iter().any(|r| r == ISOLATED_GUI_REL),
        "scripts/lib/ の走査に {ISOLATED_GUI_REL} が無い（走査範囲の取り方が壊れている）: {out:?}"
    );
    out
}

/// ヘルパの中で tako CLI を素の env で起こす行（#2598）。
///
/// ヘルパの関数は**呼び手の env で走る**ので、ファイル先頭の `unset` / `export` では隔離できない
/// （source した呼び手の env を書き換えるのはヘルパの仕事ではなく、検証スクリプトの走査とは数え方が違う）。
/// tako を起こすコマンドごとに、**その行の** `env -u TAKO_SOCKET -u TAKO_PANE_ID`（か `env -i`）と
/// 発見の置き場の前置き（`TAKO_DISCOVERY_DIR=…` など）を求める。本番へ届くのが仕様の行（本番の順番待ち・
/// A/B の旧挙動の腕）だけ `# tako:prod-ok <理由>` で外す。名指すのは 1 行ずつ（ヘルパは数が少ない）
fn helper_violations(rel: &str, scan: &ScriptScan) -> Vec<String> {
    scan.launches
        .iter()
        .filter(|l| l.what == "tako" && !l.env_clear && !scan.prod_ok.contains(&l.line))
        .filter_map(|l| {
            let mut missing: Vec<&str> = TAKO_CLI_KEYS
                .iter()
                .copied()
                .filter(|k| !l.env_drops.iter().any(|d| d == k))
                .collect();
            if !DISCOVERY_KEYS
                .iter()
                .any(|k| l.env_sets.iter().any(|e| e == k))
            {
                missing.push("発見の置き場");
            }
            (!missing.is_empty()).then(|| {
                format!(
                    "{rel}:{}: ヘルパの中で tako を呼び手の env のまま起こしている（その行に無いもの: {missing:?}）。\
                     呼び手が本番 tako のペインなら本番の GUI へ届く",
                    l.line
                )
            })
        })
        .collect()
}

/// シェルの関数 `name() {` の本体（次の行頭の `}` まで）。無ければ None
fn shell_fn_body<'a>(src: &'a str, name: &str) -> Option<&'a str> {
    let start = src.find(&format!("\n{name}() {{"))? + 1;
    let end = src[start..].find("\n}\n")? + start;
    Some(&src[start..end])
}

#[test]
fn ヘルパの中のtakoは起こした隔離guiへだけ繋ぐ() {
    let mut violations = Vec::new();
    for rel in helper_scripts() {
        violations.extend(helper_violations(&rel, &scan_script(&read(&rel))));
    }
    // 空振りしていない: isolated-gui.sh の隔離の env 付きの行（iso_cli）を tako の起動として数えている
    let src = read(ISOLATED_GUI_REL);
    let scan = scan_script(&src);
    let isolated = scan
        .launches
        .iter()
        .filter(|l| l.what == "tako" && !scan.prod_ok.contains(&l.line))
        .count();
    assert!(
        isolated >= 1,
        "{ISOLATED_GUI_REL}: 隔離の env を付けて tako を起こす行（iso_cli）を走査が拾っていない（走査が壊れているか、\
         iso_cli が消えた）: {:?}",
        scan.launches
    );
    assert!(
        violations.is_empty(),
        "検証スクリプトが source するヘルパの中で tako CLI を呼び手の env のまま起こしている（#2598）。\
         ヘルパの関数は呼び手の env で走るので、呼び手が本番 tako のペインなら本番の TAKO_SOCKET へ、\
         env を落としていても既定の置き場の本番 GUI へ届く。isolated-gui.sh の中は iso_cli を通し、\
         他のヘルパはその行で `env -u TAKO_SOCKET -u TAKO_PANE_ID` と発見の置き場（{}）を付ける。\
         本番へ届くのが仕様の行だけ `# tako:prod-ok <理由>`:\n  {}",
        DISCOVERY_KEYS.join(" / "),
        violations.join("\n  ")
    );
    // 待ちは iso_cli を通す（素の tako に戻した形は上の走査が名指す。ここは呼び出しが消えていないことだけ）
    let wait = shell_fn_body(&src, "wait_isolated_gui")
        .unwrap_or_else(|| panic!("{ISOLATED_GUI_REL}: wait_isolated_gui() が見つからない"));
    assert!(
        wait.contains("iso_cli list"),
        "{ISOLATED_GUI_REL}: wait_isolated_gui が iso_cli で待っていない（#2598）"
    );
}

#[test]
fn ヘルパの走査は素のenvのtakoを名指す() {
    let cases: &[(&str, &str, usize)] = &[
        // (説明, ヘルパ, 名指す数)
        ("関数の中の素の CLI", "w() {\n  \"$TAKO_BIN\" list\n}\n", 1),
        // 検証スクリプトなら通る形でも、ヘルパでは数えない（関数は呼び手の env で走る）
        ("ファイル先頭の unset と export", "unset TAKO_SOCKET TAKO_PANE_ID\nexport TAKO_DATA_DIR=\"$T\"\nw() { \"$TAKO_BIN\" list; }\n", 1),
        ("その行で落として置き場を向ける", "env -u TAKO_SOCKET -u TAKO_PANE_ID ${d[@]+\"${d[@]}\"} \\\n  TAKO_ISOLATED=1 TAKO_DISCOVERY_DIR=\"$D\" \\\n  \"$TAKO_BIN\" \"$@\"\n", 0),
        ("落とすが置き場が既定のまま", "env -u TAKO_SOCKET -u TAKO_PANE_ID TAKO_ISOLATED=1 \"$TAKO_BIN\" list\n", 1),
        ("置き場だけ向けて落とさない", "TAKO_DISCOVERY_DIR=\"$D\" \"$TAKO_BIN\" list\n", 1),
        ("ソケットだけ落とす", "env -u TAKO_SOCKET TAKO_DISCOVERY_DIR=\"$D\" \"$TAKO_BIN\" list\n", 1),
        ("env -i", "env -i PATH=/bin \"$TAKO_BIN\" list\n", 0),
        ("本番へ届くのが仕様（宣言あり）", "# tako:prod-ok 本番の順番待ちへ並ぶ\n\"$TAKO_BIN\" queue cargo build\n", 0),
        ("理由の無い宣言は効かない", "\"$TAKO_BIN\" list # tako:prod-ok\n", 1),
        ("サブコマンドの使い方", "case \"$(tako queue --help 2>/dev/null)\" in *x*) : ;; esac\n", 0),
        // case のパターンの `|` で並べた選択肢（tmux-socket.sh の `tako | default | */*)`）はコマンドではない
        ("case のパターンの選択肢", "case \"$n\" in\n  tako | default | */*) echo x ;;\nesac\n", 0),
        ("case の本体の素の CLI", "case \"$n\" in\n  a | b) \"$TAKO_BIN\" list ;;\nesac\n", 1),
    ];
    for (what, src, want) in cases {
        let got = helper_violations("lib.sh", &scan_script(src));
        assert_eq!(got.len(), *want, "{what}: {src:?} → {got:?}");
    }
    let got = helper_violations("lib.sh", &scan_script("w() {\n\n  \"$TAKO_BIN\" list\n}\n"));
    assert!(
        got.len() == 1 && got[0].starts_with("lib.sh:3: ヘルパの中で tako"),
        "{got:?}"
    );

    // 逆戻りの注入（現行のヘルパから作る。ファイルは触らない）: 待ちの iso_cli を素の CLI へ戻すと、
    // その行を file:line で名指す
    let src = read(ISOLATED_GUI_REL);
    let wait =
        shell_fn_body(&src, "wait_isolated_gui").expect("wait_isolated_gui() が見つからない");
    let wait_at = src.find(wait).expect("本体の位置");
    let injected = format!(
        "{}{}{}",
        &src[..wait_at],
        wait.replacen("iso_cli list", "\"$TAKO_BIN\" list", 1),
        &src[wait_at + wait.len()..]
    );
    assert_ne!(injected, src, "注入が当たっていない");
    let line = injected
        .lines()
        .position(|l| l.contains("\"$TAKO_BIN\" list >/dev/null"))
        .expect("注入した行")
        + 1;
    let got = helper_violations(ISOLATED_GUI_REL, &scan_script(&injected));
    assert_eq!(
        got.len(),
        1,
        "待ちを素の CLI へ戻した形を名指していない: {got:?}"
    );
    assert!(
        got[0].starts_with(&format!("{ISOLATED_GUI_REL}:{line}: ")),
        "名指した行が違う（注入した行は {line}）: {got:?}"
    );
}

#[test]
fn 走査は実際の持ち込みを検出する() {
    let all = ISSUE_1970_KEYS.join(" ");
    let cases: &[(&str, &str, usize)] = &[
        // (説明, スクリプト, 名指す数)。tako は「env を落としていない」と「発見の置き場が既定のまま」を別々に数える
        ("落とさずに CLI", "\"$TAKO_BIN\" list\n", 2),
        ("CLI の 2 つを落とす", "unset TAKO_SOCKET TAKO_PANE_ID\nexport TAKO_DATA_DIR=\"$T/d\"\n\"$TAKO_BIN\" list\n", 0),
        ("CLI の行で env -u", "env -u TAKO_SOCKET -u TAKO_PANE_ID TAKO_ISOLATED=1 TAKO_DATA_DIR=\"$T\" \"$TAKO_BIN\" list\n", 0),
        ("関数の中の CLI", "tk() { \"$TAKO_BIN\" \"$@\"; }\n", 2),
        // #2527: env を落としても、発見の置き場が既定のままなら CLI は本番 GUI へ繋ぐ
        ("#2527 の形（unset と TAKO_ISOLATED だけ）", "unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID\nTAKO_ISOLATED=1 \"$TAKO_BIN\" remote start\n", 1),
        ("env -u と TAKO_ISOLATED だけ", "env -u TAKO_SOCKET -u TAKO_PANE_ID TAKO_ISOLATED=1 \"$TAKO_BIN\" list\n", 1),
        ("ソケットを明示しても発見へ落ちる", "unset TAKO_SOCKET TAKO_PANE_ID\nTAKO_SOCKET=\"$S\" TAKO_TOKEN=\"$K\" \"$TAKO_BIN\" list\n", 1),
        ("export しない代入は子へ渡らない", "unset TAKO_SOCKET TAKO_PANE_ID\nTAKO_DATA_DIR=\"$T\"\n\"$TAKO_BIN\" list\n", 1),
        ("代入の後で名前だけ export", "unset TAKO_SOCKET TAKO_PANE_ID\nTAKO_DISCOVERY_DIR=\"$T\"\nexport TAKO_DISCOVERY_DIR\n\"$TAKO_BIN\" list\n", 0),
        ("declare -x", "unset TAKO_SOCKET TAKO_PANE_ID\ndeclare -x TAKO_DATA_DIR=\"$T\"\n\"$TAKO_BIN\" list\n", 0),
        ("HOME の代入（常に export 済み）", "unset TAKO_SOCKET TAKO_PANE_ID\nHOME=\"$T/h\"\n\"$TAKO_BIN\" list\n", 0),
        ("行の前置きで HOME", "unset TAKO_SOCKET TAKO_PANE_ID\nHOME=\"$T/h\" \"$TAKO_BIN\" list\n", 0),
        ("関数の中の前置き", "unset TAKO_SOCKET TAKO_PANE_ID\ntk() { TAKO_DATA_DIR=\"$D\" \"$TAKO_BIN\" \"$@\"; }\n", 0),
        ("env -i の tako", "env -i PATH=/bin \"$TAKO_BIN\" list\n", 0),
        ("tako の版の問い合わせ", "v=$(\"$TAKO_BIN\" --version)\n", 0),
        ("パスを代入した変数の旧版", "unset TAKO_SOCKET TAKO_PANE_ID\nOLD=\"/Applications/tako.app/Contents/MacOS/tako\"\nh=\"$(\"$OLD\" file copy a b 2>&1)\"\n", 1),
        ("既定値つきで代入した変数", "OLD_TAKO_BIN=\"${OLD_TAKO_BIN:-/Applications/tako.app/Contents/MacOS/tako}\"\nunset TAKO_SOCKET TAKO_PANE_ID\nexport TAKO_DATA_DIR=\"$T\"\n\"$OLD_TAKO_BIN\" list\n", 0),
        ("tako 以外を代入した変数", "BIN=\"$T/fake-tool\"\n\"$BIN\" list\n", 0),
        ("本番の順番待ち（宣言あり・同じ行）", "unset TAKO_SOCKET TAKO_PANE_ID\nQ=\"/Applications/tako.app/Contents/MacOS/tako\"\n\"$Q\" queue cargo test # tako:prod-ok 本番の順番待ちへ並ぶ\n", 0),
        ("本番の順番待ち（宣言あり・直前の行）", "unset TAKO_SOCKET TAKO_PANE_ID\n# tako:prod-ok 本番の順番待ちへ並ぶ\n\"$TAKO_BIN\" queue cargo test\n", 0),
        ("理由の無い宣言は効かない", "unset TAKO_SOCKET TAKO_PANE_ID\n\"$TAKO_BIN\" queue cargo test # tako:prod-ok\n", 1),
        ("宣言は 2 行先まで効かない", "unset TAKO_SOCKET TAKO_PANE_ID\n# tako:prod-ok 本番の順番待ちへ並ぶ\n\n\"$TAKO_BIN\" queue cargo test\n", 1),
        ("宣言でも env を落とす規則は外れない", "\"$TAKO_BIN\" queue cargo test # tako:prod-ok 本番の順番待ちへ並ぶ\n", 1),
        ("ソケットだけ落とした claude", "unset TAKO_SOCKET TAKO_PANE_ID\nclaude -p hi\n", 1),
        ("置換の中の claude", "out=\"$(CLAUDE_CONFIG_DIR=x claude plugin validate .)\"\n", 1),
        ("if の後の claude", "if claude -p hi >/dev/null; then :; fi\n", 1),
        ("env -i の claude", "env -i PATH=/bin claude -p hi\n", 0),
        ("ペインへ送る文字列", "unset TAKO_SOCKET TAKO_PANE_ID\nexport TAKO_DATA_DIR=\"$T\"\n\"$TAKO_BIN\" send --pane 1 \"claude --model haiku\"\n", 0),
        ("ヒアドキュメントの本文", "cat > f <<'EOF'\nclaude -p hi\n\"$TAKO_BIN\" list\nEOF\n", 0),
        ("在るかの確認", "command -v claude >/dev/null || exit 0\n", 0),
        ("版の問い合わせ", "v=$(claude --version 2>/dev/null)\n", 0),
        ("版の後の会話", "v=$(claude --version)\nclaude -p hi\n", 1),
        ("symlink で置いた実物", "ln -sf \"$(command -v claude)\" \"$BIN/claude\"\n\"$BIN/claude\" plugin list\n", 1),
        ("書き出した偽の claude", "cat > \"$BIN/claude\" <<'EOF'\nexit 0\nEOF\n\"$BIN/claude\" --warm\n", 0),
        ("配列の展開の後ろの claude", "env -u TAKO_SOCKET HOME=\"$H\" ${extra[@]+\"${extra[@]}\"} \"$BIN/claude\" plugin list\n", 1),
        ("内側の印で分ける", "if [ -z \"${INNER:-}\" ]; then\n  unset TAKO_SOCKET TAKO_PANE_ID\n  export TAKO_DATA_DIR=\"$T\"\n  exec cargo run -- bash \"$0\"\nfi\n\"$TAKO\" list\n", 0),
        ("case のパターン", "case \"$a\" in\n  claude) echo c ;;\nesac\n", 0),
        ("case のパターンの | で並べた選択肢", "case \"$a\" in\n  tako | claude) echo c ;;\nesac\n", 0),
        ("引用符の中の説明", "echo \"claude を起こす\"\n", 0),
        ("算術", "n=$((n + 1))\n", 0),
    ];
    for (what, src, want) in cases {
        let got = launch_violations("t.sh", &scan_script(src));
        assert_eq!(got.len(), *want, "{what}: {src:?} → {got:?}");
    }
    // 継続行の unset と行番号（継続行の先頭の行で名指す）
    let src = format!(
        "unset {} \\\n  TAKO_TOKEN\n\n\nclaude -p hi\n",
        ISSUE_1970_KEYS[..4].join(" ")
    );
    let got = launch_violations("t.sh", &scan_script(&src));
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].starts_with("t.sh:5: claude"), "{got:?}");
    assert!(
        got[0].contains("TAKO_CLI") && !got[0].contains("TAKO_SOCKET"),
        "{got:?}"
    );
    let src = format!("unset {all}\nclaude -p hi\n");
    assert!(launch_violations("t.sh", &scan_script(&src)).is_empty());
    // 発見の置き場の名指しは最初の行と件数（#2527）
    let src =
        "unset TAKO_SOCKET TAKO_PANE_ID\n\n\"$TAKO_BIN\" remote start\n\"$TAKO_BIN\" remote stop\n";
    let got = launch_violations("t.sh", &scan_script(src));
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(
        got[0].starts_with("t.sh:3: tako を起こす前に発見の置き場")
            && got[0].contains("同じ形が 2 行"),
        "{got:?}"
    );
}

// ---- ヘルパの挙動（偽の GUI と偽の面の係で起こす。窓は出さない）------------------------

/// 面を用意する係の偽物（いつでも用意できる）
#[cfg(unix)]
const STUB_VD_1970: &str = "#!/bin/bash\ncase \"$1\" in ensure) exit 0 ;; bounds) echo '0 0 1 1'; exit 0 ;; esac\nexit 1\n";

/// GUI の偽物。受け取った env を丸ごと書き出す
#[cfg(unix)]
const STUB_APP_1970: &str = "#!/bin/bash\nenv > \"$STUB_ENV_OUT\"\n";

#[cfg(unix)]
struct Scratch1970(PathBuf);

#[cfg(unix)]
impl Scratch1970 {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("tako-1970-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("一時 dir を作る");
        // macOS の temp_dir は /var → /private/var のシンボリックリンク越し。ヘルパは物理パスで返す
        Scratch1970(dir.canonicalize().expect("一時 dir の物理パス"))
    }
    fn dir(&self, name: &str) -> PathBuf {
        let p = self.0.join(name);
        std::fs::create_dir_all(&p).expect("dir を作る");
        p
    }
}

#[cfg(unix)]
impl Drop for Scratch1970 {
    fn drop(&mut self) {
        let tmp = std::env::temp_dir().canonicalize().expect("temp_dir");
        assert!(
            self.0.starts_with(&tmp),
            "一時 dir の外を消そうとしている: {}",
            self.0.display()
        );
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// ヘルパを source して `launch_isolated_gui` を 1 回呼び、偽の GUI が受け取った env を返す
/// （macOS 同梱の bash 3.2 で走らせる）。`env` は呼び出し元の env（本番のペインに見立てる）、
/// `args` は `launch_isolated_gui` へ並べる `VAR=VAL`
#[cfg(unix)]
fn launch_with(
    scratch: &Scratch1970,
    env: &[(&str, &str)],
    args: &[&str],
) -> std::collections::BTreeMap<String, String> {
    launch_capture(scratch, env, args).0
}

/// [`launch_with`] と同じ起こし方で、偽の GUI が受け取った env と**ヘルパの stderr** を返す
/// （#2627 の案内の行を見る）
#[cfg(unix)]
fn launch_capture(
    scratch: &Scratch1970,
    env: &[(&str, &str)],
    args: &[&str],
) -> (std::collections::BTreeMap<String, String>, String) {
    use std::os::unix::fs::PermissionsExt;
    let run = scratch.dir("run");
    let vd = run.join("virtual-display.sh");
    let app = run.join("app");
    for (path, body) in [(&vd, STUB_VD_1970), (&app, STUB_APP_1970)] {
        std::fs::write(path, body).expect("偽物を書く");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let out = run.join("env.out");
    let _ = std::fs::remove_file(&out);
    let script = r#"
. "$HELPER"
APP_BIN="$STUB_APP_BIN"
launch_isolated_gui "$STUB_RUN/app.log" "$@" || exit $?
wait "$ISOLATED_GUI_PID"
"#;
    let mut cmd = std::process::Command::new("/bin/bash");
    cmd.arg("-c")
        .arg(script)
        .arg("launch_with")
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        // 偽の係の下でも実在しない面の名前（ヘルパが差し替え口を無視しても実機に触れない）
        .env("TAKO_VD_NAME", "tako-vd-watchdog-1970-absent")
        .env("HELPER", repo_root().join(ISOLATED_GUI_REL))
        .env("ISOLATED_GUI_VD", &vd)
        // 利用者のホームの見立て（ここには既定の .zshrc を書かない）
        .env("ISOLATED_GUI_REAL_HOME", scratch.dir("realhome"))
        .env("STUB_APP_BIN", &app)
        .env("STUB_RUN", &run)
        .env("STUB_ENV_OUT", &out);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd.output().expect("/bin/bash を起こす");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "launch_isolated_gui が失敗した: {:?}\n{stderr}",
        output.status
    );
    let got = std::fs::read_to_string(&out)
        .expect("偽の GUI が起きていない")
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    (got, stderr)
}

/// 本番のペインに見立てた env（ヘルパが落とす名前すべてに本番の値を入れる）
#[cfg(unix)]
fn production_like_env(home: &Path) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = helper_drop_env(&read(ISOLATED_GUI_REL))
        .into_iter()
        .map(|k| (k, "prod-1970".to_string()))
        .collect();
    env.push((
        "CLAUDE_CONFIG_DIR".into(),
        "/nonexistent/real-claude-config".into(),
    ));
    env.push(("HOME".into(), home.display().to_string()));
    env
}

#[cfg(unix)]
#[test]
fn 隔離guiのヘルパは本番のenvを落とし既定を置いて起こす() {
    let scratch = Scratch1970::new("defaults");
    let home = scratch.dir("home");
    let env = production_like_env(&home);
    let env_ref: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();

    // 既定: 本番の名前は 1 つも届かず、自動リネームは止まる。HOME が一時の dir なので
    // export された本物の CLAUDE_CONFIG_DIR は外すだけ（claude はその HOME の .claude を使う）
    let got = launch_with(&scratch, &env_ref, &[]);
    let leaked: Vec<&String> = got.keys().filter(|k| got[*k] == "prod-1970").collect();
    assert!(
        leaked.is_empty(),
        "本番の env が隔離 GUI へ届いた: {leaked:?}"
    );
    assert_eq!(got.get("TAKO_AUTO_RENAME").map(String::as_str), Some("0"));
    assert_eq!(
        got.get("CLAUDE_CONFIG_DIR"),
        None,
        "一時の HOME なのに export された本物の CLAUDE_CONFIG_DIR が残った / 別の dir へ差し替えた"
    );
    let zshrc =
        std::fs::read_to_string(home.join(".zshrc")).expect("一時の HOME に既定の .zshrc が無い");
    assert!(zshrc.contains("PROMPT='tako %1~ %% '"), "{zshrc}");
    // 落とさない名前（検証スクリプトが渡す隔離の置き場）は届く
    assert_eq!(got.get("HOME").map(String::as_str), home.to_str());
    assert_eq!(got.get("TAKO_ISOLATED").map(String::as_str), Some("1"));

    // HOME が利用者のホームのまま（の見立て）なら、設定 dir はログの置き場の隣の一時 dir
    let real = scratch.dir("realhome");
    let real_arg = format!("HOME={}", real.display());
    let got = launch_with(&scratch, &env_ref, &[&real_arg]);
    let cfg = scratch.0.join("run/isolated-claude-config");
    assert_eq!(
        got.get("CLAUDE_CONFIG_DIR").map(String::as_str),
        cfg.to_str(),
        "利用者のホームのままなのに CLAUDE_CONFIG_DIR がログの置き場の隣の一時 dir になっていない"
    );
    assert!(cfg.is_dir(), "一時の設定 dir が作られていない");

    // 並べた値は既定にも落とす一覧にも勝つ（明示で on にできる口）
    let mine = scratch.dir("mine-config");
    let mine_arg = format!("CLAUDE_CONFIG_DIR={}", mine.display());
    let got = launch_with(
        &scratch,
        &env_ref,
        &["TAKO_AUTO_RENAME=1", &mine_arg, "TAKO_CLI=/iso/tako"],
    );
    assert_eq!(got.get("TAKO_AUTO_RENAME").map(String::as_str), Some("1"));
    assert_eq!(
        got.get("CLAUDE_CONFIG_DIR").map(String::as_str),
        mine.to_str()
    );
    assert_eq!(got.get("TAKO_CLI").map(String::as_str), Some("/iso/tako"));
    assert!(
        !got.contains_key("TAKO_SOCKET"),
        "並べていない名前は落ちたまま"
    );

    // export された自動リネームの指定は尊重する（本番 tako は配らない名前）
    let mut env_on = env_ref.clone();
    env_on.push(("TAKO_AUTO_RENAME", "1"));
    let got = launch_with(&scratch, &env_on, &[]);
    assert_eq!(got.get("TAKO_AUTO_RENAME").map(String::as_str), Some("1"));
}

#[cfg(unix)]
#[test]
fn 既定のzshrcは利用者のホームと既存のrcへ書かない() {
    let scratch = Scratch1970::new("zshrc");
    // 利用者のホーム（の見立て）には rc が無くても書かない
    let real = scratch.dir("realhome");
    let env = production_like_env(&real);
    let env_ref: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    launch_with(&scratch, &env_ref, &[]);
    assert!(
        !real.join(".zshrc").exists(),
        "利用者のホームへ .zshrc を書いた"
    );
    // 利用者のホームの中の dir（ZDOTDIR=~/.config/zsh の形）にも書かない
    let inner = scratch.dir("realhome/.config/zsh");
    let inner_arg = format!("ZDOTDIR={}", inner.display());
    launch_with(&scratch, &env_ref, &[&inner_arg]);
    assert!(
        !inner.join(".zshrc").exists(),
        "利用者のホームの中の ZDOTDIR へ .zshrc を書いた"
    );

    // ZDOTDIR を並べたら、HOME ではなくそちらへ置く
    let home = scratch.dir("home2");
    let zdot = scratch.dir("zdot");
    let home_arg = format!("HOME={}", home.display());
    let zdot_arg = format!("ZDOTDIR={}", zdot.display());
    launch_with(&scratch, &env_ref, &[&home_arg, &zdot_arg]);
    assert!(
        zdot.join(".zshrc").exists(),
        "ZDOTDIR へ既定の .zshrc を置いていない"
    );
    assert!(
        !home.join(".zshrc").exists(),
        "ZDOTDIR があるのに HOME へ書いた"
    );

    // 呼び出し側の rc は上書きしない（.zprofile だけでも初回設定メニューは出ないので足さない）
    let own = scratch.dir("own");
    std::fs::write(own.join(".zshrc"), "PROMPT='mine '\n").expect("rc を書く");
    let prof = scratch.dir("prof");
    std::fs::write(prof.join(".zprofile"), "export X=1\n").expect("rc を書く");
    for dir in [&own, &prof] {
        let arg = format!("HOME={}", dir.display());
        launch_with(&scratch, &env_ref, &[&arg]);
    }
    assert_eq!(
        std::fs::read_to_string(own.join(".zshrc")).unwrap(),
        "PROMPT='mine '\n"
    );
    assert!(
        !prof.join(".zshrc").exists(),
        ".zprofile があるのに .zshrc を足した"
    );
}

// ---- 隔離 GUI のペインの zsh に利用者の rc を読ませない（#2627）---------------------------

/// 利用者の rc を読ませる opt-in の名前（#2627）
const USER_RC_OPTIN: &str = "TAKO_ISO_USER_RC";

/// opt-in に添える理由の宣言（`# tako:user-rc-ok <理由>`。宣言の行とその次の行に効く）
const USER_RC_OK: &str = "# tako:user-rc-ok";

/// A/B の旧挙動（rc を読む dir に手を付けない）の名前
const USER_RC_LEGACY: &str = "TAKO_2627_LEGACY";

/// 旧挙動の A/B を使ってよい検証スクリプト（#2627 の実経路テストだけ）
const USER_RC_LEGACY_OK: &[&str] = &["scripts/test-isolated-gui-user-rc-2627.sh"];

/// 行のうちコメントより前（空白の後の `#` からを落とす。引用符の中の `#` は数えない）
fn shell_code(line: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut prev = ' ';
    for (i, c) in line.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '#' && prev.is_whitespace() => return &line[..i],
            None => {}
        }
        prev = c;
    }
    line
}

/// 行の中の `NAME=値` が「立てる」値か（0 と空以外 = 立てる。変数は確かめられないので立てる側）。
/// 引用符の中は、引用の先頭に書いた代入（`"NAME=1"` = 引数 1 つ）だけを数える
/// （`echo "… NAME=1 …"` のような文の中の言及は数えない）
fn sets_flag(code: &str, name: &str) -> bool {
    let pat = format!("{name}=");
    // 開いている引用符と、その位置
    let mut quote: Option<(char, usize)> = None;
    let mut prev: Option<char> = None;
    for (i, c) in code.char_indices() {
        match quote {
            Some((q, _)) if c == q => {
                quote = None;
                prev = Some(c);
                continue;
            }
            None if c == '"' || c == '\'' => {
                quote = Some((c, i));
                prev = Some(c);
                continue;
            }
            _ => {}
        }
        if code[i..].starts_with(&pat) {
            // 名前の一部（`XTAKO_ISO_USER_RC=`）は数えない
            let word_start = !prev.is_some_and(|p| p.is_ascii_alphanumeric() || p == '_');
            let assignment = quote.is_none_or(|(_, open)| open + 1 == i);
            if word_start && assignment {
                let value: String = code[i + pat.len()..]
                    .chars()
                    .take_while(|c| !c.is_whitespace() && *c != ';')
                    .collect();
                let value = value.trim_matches(['"', '\'']);
                if !value.is_empty() && value != "0" {
                    return true;
                }
            }
        }
        prev = Some(c);
    }
    false
}

/// 走査の結果（#2627）: 名指す行と、opt-in を見つけた数（走査の空振りを見分ける）
#[derive(Debug, Default)]
struct UserRcScan {
    violations: Vec<String>,
    optins: usize,
}

/// 利用者の rc を読ませる opt-in に理由の無い行と、旧挙動の A/B を実経路テストの外で立てる行（#2627）。
///
/// opt-in（`TAKO_ISO_USER_RC=<0 / 空 以外>`）はペインの zsh に利用者の rc を読ませる。後片付けや GUI の
/// 終了が rc の途中を断つと利用者の環境（pyenv の shim のロック等）に残骸が残る（#1556 の worker が踏んだ）
/// ので、使う行ごとに「なぜ利用者の rc が要るか」を `# tako:user-rc-ok <理由>` で残させる（宣言は同じ行の
/// 行末か直前の行。継続行の途中には書けないので、継続した 1 つのコマンドは先頭の行の直前でもよい）。
/// 旧挙動（`TAKO_2627_LEGACY`）は理由を書いても外さない（A/B の実経路テストだけが使う）。
/// コメント行とヒアドキュメントの本文（書き出すスタブ）は読まない。名指すのは値を書いた物理行
fn user_rc_scan(rel: &str, src: &str) -> UserRcScan {
    let lines: Vec<&str> = src.lines().collect();
    let reason_on = |i: usize| {
        lines
            .get(i)
            .and_then(|l| l.split(USER_RC_OK).nth(1))
            .is_some_and(|r| !r.trim().is_empty())
    };
    let legacy_ok = USER_RC_LEGACY_OK.contains(&rel);
    let mut scan = UserRcScan::default();
    let mut idx = 0;
    while idx < lines.len() {
        // 継続行（行末の `\`）を 1 つのコマンドとして読む
        let start = idx;
        while idx + 1 < lines.len() && lines[idx].ends_with('\\') {
            idx += 1;
        }
        let end = idx;
        idx += 1;
        for (i, line) in lines.iter().enumerate().take(end + 1).skip(start) {
            let code = shell_code(line);
            if code.trim_start().starts_with('#') || code.trim().is_empty() {
                continue;
            }
            if sets_flag(code, USER_RC_OPTIN) {
                scan.optins += 1;
                let declared = reason_on(i)
                    || (i > 0 && reason_on(i - 1))
                    || (start > 0 && reason_on(start - 1))
                    || reason_on(end);
                if !declared {
                    scan.violations.push(format!(
                        "{rel}:{}: {USER_RC_OPTIN} でペインの zsh に利用者の rc を読ませているのに、理由の宣言（`{USER_RC_OK} <理由>`）が無い",
                        i + 1
                    ));
                }
            }
            if !legacy_ok && sets_flag(code, USER_RC_LEGACY) {
                scan.violations.push(format!(
                    "{rel}:{}: {USER_RC_LEGACY}（ペインの zsh に利用者の rc を読ませる旧挙動）を A/B の実経路テストの外で立てている",
                    i + 1
                ));
            }
        }
        // ヒアドキュメントの本文は読まない（書き出すスタブ）
        let logical: String = lines[start..=end].join(" ");
        if let Some((marker, strip_tabs)) = heredoc_marker(&logical) {
            while idx < lines.len() {
                let l = if strip_tabs {
                    lines[idx].trim_start_matches('\t')
                } else {
                    lines[idx]
                };
                idx += 1;
                if l == marker {
                    break;
                }
            }
        }
    }
    scan
}

#[test]
fn 利用者のrcを読ませるoptinには理由を書く() {
    let mut violations = Vec::new();
    let mut optins = 0usize;
    let mut scripts = verification_scripts();
    // ヘルパの定義（isolated-gui.sh 自身）は opt-in の使い方の説明を持つので除く
    scripts.extend(
        helper_scripts()
            .into_iter()
            .filter(|r| r != ISOLATED_GUI_REL),
    );
    for rel in scripts {
        let scan = user_rc_scan(&rel, &read(&rel));
        optins += scan.optins;
        violations.extend(scan.violations);
    }
    // 空振りしていない: #2627 の実経路テストの opt-in の腕（理由つき）を拾っている
    assert!(
        optins >= 1,
        "{USER_RC_OPTIN} の opt-in を 1 つも拾っていない（走査が壊れているか、#2627 の実経路テストの opt-in の腕が消えた）"
    );
    assert!(
        violations.is_empty(),
        "隔離 GUI のペインの zsh に利用者の rc を読ませる行に理由が無い（#2627）。rc の途中を後片付けや GUI の終了が\
         断つと、利用者の環境（pyenv の shim のロック等）に残骸が残る。本当に利用者の rc が要る検証だけ、その行か\
         直前の行に `{USER_RC_OK} <理由>` を書く（要らないなら opt-in を外す = 既定の砂場の ZDOTDIR）:\n  {}",
        violations.join("\n  ")
    );
}

#[test]
fn 利用者のrcのoptinの走査は理由の無い行を名指す() {
    let cases: &[(&str, &str, usize)] = &[
        // (説明, スクリプト, 名指す数)
        ("理由の無い opt-in", "launch_isolated_gui \"$L\" TAKO_ISO_USER_RC=1\n", 1),
        ("export の opt-in", "export TAKO_ISO_USER_RC=1\nlaunch_isolated_gui \"$L\"\n", 1),
        ("引用符で囲んだ opt-in", "launch_isolated_gui \"$L\" \"TAKO_ISO_USER_RC=1\"\n", 1),
        ("変数の値も opt-in と読む", "launch_isolated_gui \"$L\" TAKO_ISO_USER_RC=\"$X\"\n", 1),
        ("直前の行の宣言", "# tako:user-rc-ok 利用者の rc の PATH を測る\nlaunch_isolated_gui \"$L\" TAKO_ISO_USER_RC=1\n", 0),
        ("行末の宣言", "export TAKO_ISO_USER_RC=1 # tako:user-rc-ok 利用者の rc の PATH を測る\n", 0),
        ("理由の無い宣言は効かない", "# tako:user-rc-ok\nlaunch_isolated_gui \"$L\" TAKO_ISO_USER_RC=1\n", 1),
        ("宣言は 2 行先まで効かない", "# tako:user-rc-ok 利用者の rc の PATH を測る\n\nlaunch_isolated_gui \"$L\" TAKO_ISO_USER_RC=1\n", 1),
        ("継続した 1 つのコマンドの先頭の直前の宣言", "# tako:user-rc-ok 利用者の rc の PATH を測る\nlaunch_isolated_gui \"$L\" \\\n  TAKO_DATA_DIR=\"$D\" \\\n  TAKO_ISO_USER_RC=1\n", 0),
        ("継続した 1 つのコマンドに宣言が無い", "launch_isolated_gui \"$L\" \\\n  TAKO_ISO_USER_RC=1\n", 1),
        ("0 は opt-in ではない", "launch_isolated_gui \"$L\" TAKO_ISO_USER_RC=0\n", 0),
        ("unset は opt-in ではない", "unset TAKO_ISO_USER_RC TAKO_2627_LEGACY\n", 0),
        ("コメントの中", "# launch_isolated_gui \"$L\" TAKO_ISO_USER_RC=1\n", 0),
        ("ヒアドキュメントの本文", "cat > f <<'EOF'\nlaunch_isolated_gui \"$L\" TAKO_ISO_USER_RC=1\nEOF\n", 0),
        ("名前の一部", "XTAKO_ISO_USER_RC=1 true\n", 0),
        ("文の中の言及", "echo \"=== opt-in（TAKO_ISO_USER_RC=1）の腕 ===\"\n", 0),
        ("引用の先頭の代入は数える", "args=(\"TAKO_ISO_USER_RC=1\" \"TAKO_DATA_DIR=$D\")\n", 1),
        ("旧挙動を A/B の外で立てる", "launch_isolated_gui \"$L\" TAKO_2627_LEGACY=1\n", 1),
        ("旧挙動は理由を書いても外さない", "# tako:user-rc-ok 理由\nexport TAKO_2627_LEGACY=1\n", 1),
    ];
    for (what, src, want) in cases {
        let got = user_rc_scan("t.sh", src).violations;
        assert_eq!(got.len(), *want, "{what}: {src:?} → {got:?}");
    }
    // 名指すのは値を書いた物理行
    let got = user_rc_scan(
        "t.sh",
        "set -u\nlaunch_isolated_gui \"$L\" \\\n  TAKO_DATA_DIR=\"$D\" \\\n  TAKO_ISO_USER_RC=1\n",
    )
    .violations;
    assert!(
        got.len() == 1 && got[0].starts_with("t.sh:4: TAKO_ISO_USER_RC"),
        "{got:?}"
    );
    // 旧挙動の A/B は #2627 の実経路テストだけが使える
    assert!(user_rc_scan(
        USER_RC_LEGACY_OK[0],
        "launch_isolated_gui \"$L\" TAKO_2627_LEGACY=1\n"
    )
    .violations
    .is_empty());
}

/// 隔離 GUI のヘルパの起動の行が、砂場の ZDOTDIR を並べた `VAR=VAL`（`"$@"`）より後ろに置いているか（#2627）。
///
/// 前に置くと、並べた `ZDOTDIR=`（利用者のホームの中）が勝ってペインの zsh が利用者の rc を読む。
/// 判定（`iso_user_rc_sandbox_needed`）を呼ばなくなった形も名指す（行は `launch_isolated_gui() {`）
fn helper_rc_sandbox_violations(rel: &str, src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let def_line = src
        .lines()
        .position(|l| l.starts_with("launch_isolated_gui() {"))
        .map_or(0, |i| i + 1);
    let Some(body) = shell_fn_body(src, "launch_isolated_gui") else {
        return vec![format!("{rel}: launch_isolated_gui() が見つからない")];
    };
    let calls = body
        .lines()
        .map(shell_code)
        .any(|l| l.contains("iso_user_rc_sandbox_needed \"$@\""));
    if !calls {
        out.push(format!(
            "{rel}:{def_line}: launch_isolated_gui が iso_user_rc_sandbox_needed を呼んでいない = ペインの zsh へ砂場の ZDOTDIR を向けない"
        ));
    }
    match src
        .lines()
        .enumerate()
        .find(|(_, l)| l.contains("\"$APP_BIN\" > \"$log\""))
    {
        None => out.push(format!(
            "{rel}: 隔離 GUI を起こす行（`\"$APP_BIN\" > \"$log\"`）が見つからない"
        )),
        Some((i, line)) => {
            let args = line.find("\"$@\"");
            let rc = line.find("${rc_args[@]");
            if !matches!((args, rc), (Some(a), Some(r)) if r > a) {
                out.push(format!(
                    "{rel}:{}: 隔離 GUI を起こす行が砂場の ZDOTDIR（rc_args）を並べた `\"$@\"` より後ろに置いていない = 並べた ZDOTDIR が勝つ",
                    i + 1
                ));
            }
        }
    }
    out
}

#[test]
fn 隔離guiの起動は砂場のzdotdirを最後に置く() {
    let src = read(ISOLATED_GUI_REL);
    let got = helper_rc_sandbox_violations(ISOLATED_GUI_REL, &src);
    assert!(
        got.is_empty(),
        "隔離 GUI のペインの zsh が利用者の rc を読む形へ戻っている（#2627）:\n  {}",
        got.join("\n  ")
    );

    // 逆戻りの注入（現行のヘルパから作る。ファイルは触らない）: 起動の行から砂場を外す / 前へ動かす /
    // 判定を呼ばない。どれも file:line で名指す
    let lines: Vec<&str> = src.lines().collect();
    let launch_idx = lines
        .iter()
        .position(|l| l.contains("\"$APP_BIN\" > \"$log\""))
        .expect("起動の行");
    let launch_line = launch_idx + 1;
    // 起動の行だけを書き換えた全文（同じ綴りは iso_default_zshrc の行にもあるので全文の replacen は使わない）
    let with_launch = |line: String| {
        let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        out[launch_idx] = line;
        out.join("\n") + "\n"
    };
    let rc = " ${rc_args[@]+\"${rc_args[@]}\"}";
    let original = lines[launch_idx];
    assert!(
        original.contains(rc),
        "起動の行に砂場の ZDOTDIR（rc_args）が無い: {original}"
    );
    let removed = with_launch(original.replacen(rc, "", 1));
    let moved = with_launch(original.replacen(rc, "", 1).replacen(
        " \"$@\" ",
        &format!("{rc} \"$@\" "),
        1,
    ));
    let not_called = src.replacen(
        "if iso_user_rc_sandbox_needed \"$@\"; then",
        "if false; then",
        1,
    );
    let def_line = src
        .lines()
        .position(|l| l.starts_with("launch_isolated_gui() {"))
        .expect("launch_isolated_gui() の行")
        + 1;
    for (what, injected, line) in [
        ("起動の行から砂場を外す", &removed, launch_line),
        ("砂場を並べた値より前へ動かす", &moved, launch_line),
        ("判定を呼ばない", &not_called, def_line),
    ] {
        assert_ne!(injected, &src, "{what}: 注入が当たっていない");
        let got = helper_rc_sandbox_violations(ISOLATED_GUI_REL, injected);
        assert!(
            got.len() == 1 && got[0].starts_with(&format!("{ISOLATED_GUI_REL}:{line}: ")),
            "{what}: 逆戻りを {ISOLATED_GUI_REL}:{line} で名指していない: {got:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn 隔離guiのペインのzshは利用者のrcを読まない() {
    let scratch = Scratch1970::new("userrc");
    // 利用者のホームの見立て（launch_with が ISOLATED_GUI_REAL_HOME へ渡すのと同じ dir）
    let real = scratch.dir("realhome");
    let sandbox = scratch.0.join("run/isolated-zdotdir");
    let real_env = production_like_env(&real);
    let real_ref: Vec<(&str, &str)> = real_env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let zdotdir = |got: &std::collections::BTreeMap<String, String>| got.get("ZDOTDIR").cloned();

    // ① HOME が利用者のホーム → 砂場の ZDOTDIR（既定の .zshrc だけ）。利用者のホームには書かない
    let got = launch_with(&scratch, &real_ref, &[]);
    assert_eq!(
        zdotdir(&got).as_deref(),
        sandbox.to_str(),
        "HOME が利用者のホームなのにペインの zsh を砂場へ向けていない"
    );
    let zshrc =
        std::fs::read_to_string(sandbox.join(".zshrc")).expect("砂場に既定の .zshrc が無い");
    assert!(zshrc.contains("PROMPT='tako %1~ %% '"), "{zshrc}");
    assert!(
        !real.join(".zshrc").exists(),
        "利用者のホームへ .zshrc を書いた"
    );

    // ② 並べた ZDOTDIR が利用者のホームの中（~/.config/zsh の形）でも砂場が勝ち、その旨を 1 行出す
    let inner = scratch.dir("realhome/.config/zsh");
    let inner_arg = format!("ZDOTDIR={}", inner.display());
    let (got, stderr) = launch_capture(&scratch, &real_ref, &[&inner_arg]);
    assert_eq!(
        zdotdir(&got).as_deref(),
        sandbox.to_str(),
        "並べた利用者の ZDOTDIR が勝った"
    );
    assert!(
        stderr.contains("#2627"),
        "並べた値を砂場へ替えた旨が出ていない: {stderr}"
    );

    // ③ export された ZDOTDIR が利用者のホームの中なら、HOME が一時の dir でも砂場
    let tmp_home = scratch.dir("tmphome");
    let inner_s = inner.display().to_string();
    let mut tmp_env = production_like_env(&tmp_home);
    tmp_env.push(("ZDOTDIR".into(), inner_s.clone()));
    let tmp_ref: Vec<(&str, &str)> = tmp_env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let got = launch_with(&scratch, &tmp_ref, &[]);
    assert_eq!(
        zdotdir(&got).as_deref(),
        sandbox.to_str(),
        "export された利用者の ZDOTDIR を砂場へ向けていない"
    );

    // ④ 呼び手の一時の HOME / ZDOTDIR（利用者のホームの外）の rc は従来どおり読む = 足さない・替えない
    let home_env = production_like_env(&tmp_home);
    let home_ref: Vec<(&str, &str)> = home_env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let got = launch_with(&scratch, &home_ref, &[]);
    assert_eq!(
        zdotdir(&got),
        None,
        "呼び手の一時の HOME なのに ZDOTDIR を足した"
    );
    let zdot = scratch.dir("zdot");
    let zdot_arg = format!("ZDOTDIR={}", zdot.display());
    let got = launch_with(&scratch, &real_ref, &[&zdot_arg]);
    assert_eq!(
        zdotdir(&got).as_deref(),
        zdot.to_str(),
        "呼び手の一時の ZDOTDIR を替えた"
    );

    // ⑤ opt-in（並べる / export）と A/B の旧挙動は手を付けない。0 は opt-in ではない
    for args in [&["TAKO_ISO_USER_RC=1"][..], &["TAKO_2627_LEGACY=1"][..]] {
        let got = launch_with(&scratch, &real_ref, args);
        assert_eq!(zdotdir(&got), None, "{args:?} なのに ZDOTDIR を足した");
    }
    let mut optin_env = real_ref.clone();
    optin_env.push(("TAKO_ISO_USER_RC", "1"));
    optin_env.push(("ZDOTDIR", &inner_s));
    let got = launch_with(&scratch, &optin_env, &[]);
    assert_eq!(
        zdotdir(&got).as_deref(),
        Some(inner_s.as_str()),
        "export の opt-in なのに利用者の ZDOTDIR を替えた"
    );
    let got = launch_with(&scratch, &real_ref, &["TAKO_ISO_USER_RC=0"]);
    assert_eq!(
        zdotdir(&got).as_deref(),
        sandbox.to_str(),
        "TAKO_ISO_USER_RC=0 を opt-in と読んだ"
    );

    // ⑥ zsh 以外は ZDOTDIR を読まないので、利用者のホームのままなら「未対策」と 1 行出す（zsh は出さない）
    let (_, stderr) = launch_capture(&scratch, &real_ref, &["SHELL=/bin/bash"]);
    assert!(stderr.contains("未対策: SHELL=/bin/bash"), "{stderr}");
    let (_, stderr) = launch_capture(&scratch, &real_ref, &["SHELL=/bin/zsh"]);
    assert!(!stderr.contains("未対策"), "{stderr}");
}

// ---- 待ちの CLI の挙動（偽の GUI と偽の CLI で走らせる。#2598）----------------------------

/// CLI の偽物。受け取った env を丸ごと書き出して成功する（= 待ちは 1 回目で返る）
#[cfg(unix)]
const STUB_CLI_2598: &str = "#!/bin/bash\nenv > \"$STUB_CLI_OUT\"\n";

/// ヘルパを source して（`launch` なら `launch_isolated_gui` を 1 回呼んでから）`wait_isolated_gui` を呼び、
/// 偽の GUI と偽の CLI が受け取った env を返す（GUI を起こさなければ None）。
/// `env` は呼び出し元の env（本番のペインに見立てる）、`args` は `launch_isolated_gui` へ並べる `VAR=VAL`
#[cfg(unix)]
fn wait_with(
    scratch: &Scratch1970,
    tag: &str,
    env: &[(&str, &str)],
    args: &[&str],
    launch: bool,
) -> (
    Option<std::collections::BTreeMap<String, String>>,
    std::collections::BTreeMap<String, String>,
) {
    use std::os::unix::fs::PermissionsExt;
    let run = scratch.dir(tag);
    let vd = run.join("virtual-display.sh");
    let app = run.join("app");
    let cli = run.join("tako");
    for (path, body) in [
        (&vd, STUB_VD_1970),
        (&app, STUB_APP_1970),
        (&cli, STUB_CLI_2598),
    ] {
        std::fs::write(path, body).expect("偽物を書く");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let gui_out = run.join("gui.env");
    let cli_out = run.join("cli.env");
    let script = r#"
. "$HELPER"
APP_BIN="$STUB_APP_BIN"
TAKO_BIN="$STUB_CLI_BIN"
if [ -n "$STUB_LAUNCH" ]; then
    launch_isolated_gui "$STUB_RUN/app.log" "$@" || exit $?
    wait "$ISOLATED_GUI_PID"
fi
wait_isolated_gui "$STUB_RUN/app.log" 3
"#;
    let mut cmd = std::process::Command::new("/bin/bash");
    cmd.arg("-c")
        .arg(script)
        .arg("wait_with")
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("TAKO_VD_NAME", "tako-vd-watchdog-1970-absent")
        .env("HELPER", repo_root().join(ISOLATED_GUI_REL))
        .env("ISOLATED_GUI_VD", &vd)
        .env("ISOLATED_GUI_REAL_HOME", scratch.dir("realhome"))
        .env("STUB_APP_BIN", &app)
        .env("STUB_CLI_BIN", &cli)
        .env("STUB_RUN", &run)
        .env("STUB_ENV_OUT", &gui_out)
        .env("STUB_CLI_OUT", &cli_out)
        .env("STUB_LAUNCH", if launch { "1" } else { "" });
    for (k, v) in env {
        cmd.env(k, v);
    }
    let status = cmd.status().expect("/bin/bash を起こす");
    assert!(
        status.success(),
        "{tag}: wait_isolated_gui が失敗した: {status:?}"
    );
    let parse = |p: &Path| -> std::collections::BTreeMap<String, String> {
        std::fs::read_to_string(p)
            .unwrap_or_else(|e| panic!("{tag}: {} を読めない: {e}", p.display()))
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    };
    (launch.then(|| parse(&gui_out)), parse(&cli_out))
}

#[cfg(unix)]
#[test]
fn 待ちのcliは本番のenvを落とし起こしたguiと同じ置き場へ繋ぐ() {
    let scratch = Scratch1970::new("wait2598");
    let home = scratch.dir("home");
    let env = production_like_env(&home);
    let env_ref: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let leaked = |m: &std::collections::BTreeMap<String, String>| -> Vec<String> {
        m.iter()
            .filter(|(_, v)| v.as_str() == "prod-1970")
            .map(|(k, _)| k.clone())
            .collect()
    };
    let disc =
        |m: &std::collections::BTreeMap<String, String>| m.get("TAKO_DISCOVERY_DIR").cloned();

    // 置き場を渡さない起動（ヘルパの使い方の例の形）: ログの置き場の隣の一時 dir を GUI と CLI の両方へ
    let (gui, cli) = wait_with(&scratch, "none", &env_ref, &[], true);
    let gui = gui.expect("偽の GUI が起きていない");
    assert!(
        leaked(&cli).is_empty(),
        "本番の env が待ちの CLI へ届いた: {:?}",
        leaked(&cli)
    );
    assert_eq!(cli.get("TAKO_ISOLATED").map(String::as_str), Some("1"));
    let want = scratch.0.join("none/isolated-discovery");
    assert_eq!(disc(&gui).as_deref(), want.to_str(), "GUI へ渡した置き場");
    assert_eq!(
        disc(&cli),
        disc(&gui),
        "待ちの CLI が GUI と違う置き場を読む"
    );
    assert!(want.is_dir(), "一時の発見の置き場が作られていない");

    // 並べた置き場 → GUI と CLI に同じ値
    let mine = scratch.dir("mine-disc");
    let mine_arg = format!("TAKO_DISCOVERY_DIR={}", mine.display());
    let (gui, cli) = wait_with(&scratch, "arg", &env_ref, &[&mine_arg], true);
    assert_eq!(disc(&gui.expect("GUI")).as_deref(), mine.to_str());
    assert_eq!(disc(&cli).as_deref(), mine.to_str());
    assert!(leaked(&cli).is_empty(), "{:?}", leaked(&cli));

    // export された置き場 → 同じ値。空の指定は未指定と同じ（CLI が読めない置き場を作らない）
    let exported = scratch.dir("exported-disc");
    let mut env_exp = env_ref.clone();
    env_exp.push(("TAKO_DISCOVERY_DIR", exported.to_str().expect("utf8")));
    let (gui, cli) = wait_with(&scratch, "export", &env_exp, &[], true);
    assert_eq!(disc(&gui.expect("GUI")).as_deref(), exported.to_str());
    assert_eq!(disc(&cli).as_deref(), exported.to_str());
    let (gui, cli) = wait_with(&scratch, "empty", &env_exp, &["TAKO_DISCOVERY_DIR="], true);
    let want = scratch.0.join("empty/isolated-discovery");
    assert_eq!(disc(&gui.expect("GUI")).as_deref(), want.to_str());
    assert_eq!(disc(&cli).as_deref(), want.to_str());

    // launch を通さずに待つ: 置き場は呼び手の export、無ければ空 + 隔離の印（本番の既定の置き場は読まない）
    let (_, cli) = wait_with(&scratch, "nolaunch-exp", &env_exp, &[], false);
    assert_eq!(disc(&cli).as_deref(), exported.to_str());
    assert!(leaked(&cli).is_empty(), "{:?}", leaked(&cli));
    let (_, cli) = wait_with(&scratch, "nolaunch", &env_ref, &[], false);
    assert_eq!(disc(&cli).as_deref(), Some(""));
    assert_eq!(cli.get("TAKO_ISOLATED").map(String::as_str), Some("1"));
    assert!(leaked(&cli).is_empty(), "{:?}", leaked(&cli));

    // A/B の旧挙動の腕（TAKO_2598_LEGACY=1）は呼び手の env のまま = 本番の TAKO_SOCKET が届く（数え方が生きている）
    let mut env_legacy = env_ref.clone();
    env_legacy.push(("TAKO_2598_LEGACY", "1"));
    let (gui, cli) = wait_with(&scratch, "legacy", &env_legacy, &[], true);
    assert_eq!(
        cli.get("TAKO_SOCKET").map(String::as_str),
        Some("prod-1970")
    );
    assert_eq!(
        disc(&gui.expect("GUI")),
        None,
        "旧挙動の腕は GUI へ置き場を足さない"
    );
}
