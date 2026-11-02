//! PowerShell シェル統合（OSC 7 / 133）の統合テスト（#525）。**実際にシェルを起動して測る**。
//!
//! 単体テスト（`shell_integration.rs` 内）は「tako がペインの PowerShell をどんな引数で
//! 起こすか」（#2432）と「以前の版の `$PROFILE` のブロックをどう取り除くか」を固定する。
//! こちらは **統合スクリプトを読んだ PowerShell が実際に OSC を出すか**、そしてそれが
//! `TerminalSession` の `command_state()` / `cwd()` へ届くかを見る。
//! ペインの状態ドット（`tako list` の `state`）はこの 2 つがそのまま出ているので、
//! ここが緑なら「ペインの状態が idle / running / failed で報告される」ことの実測になる。
//!
//! Windows 以外ではスキップする（統合スクリプトが PowerShell 用）。
//!
//! ## 実行上の注意
//!
//! `encoding_conpty.rs` と同じく、出荷構成（コンソールを持たない GUI プロセス）に合わせて
//! テスト冒頭で `FreeConsole` する。**ユーザーの `$PROFILE` は一切触らない** —
//! スクリプトそのものの検査は `-NoProfile` で起動したシェルへドットソースで読ませ、
//! 製品の起動形（`shell_integration::powershell_pane_command`。profile を読む形）の検査は
//! 実行ポリシーを Process スコープで Restricted にして走らせる（Restricted では
//! profile のスクリプトが読まれないので、実行環境の profile に揺らされない）。
//! Restricted を作るのは環境変数 `PSExecutionPolicyPreference`（Process スコープの置き場）だけで、
//! CurrentUser / LocalMachine のポリシー（レジストリ）は触らない。

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tako_core::backend::{PsmuxBackend, SessionBackend, SessionRef};
use tako_core::terminal::{
    CommandState, SessionEvent, SpawnCommand, SpawnOptions, TerminalSession,
};

/// psmux を叩く経路は**すべて期限つき**（#1271）。素の `Command` で待つと
/// 返らない回にテストプロセスごと固まり、器（サーバー）と中の pwsh が残る
#[path = "common/psmux_ctl.rs"]
mod psmux_ctl;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn FreeConsole() -> i32;
}

fn detach_own_console() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        FreeConsole();
    });
}

/// テストごとの統合スクリプト（#2659）。**置き場はテストごとに 1 つ**で、スコープを抜けると消える。
///
/// 以前は同じプロセスの全テストが `%TEMP%\tako-si-test-<pid>\tako.ps1` を共有して書き直していた。
/// あるテストのペインの PowerShell が読む瞬間に別のテストが同じファイルへ書くと、Windows では
/// 読み手（`ReadAllText`）か書き手（`std::fs::write`）の片方が共有違反（OS エラー 32）で落ちる
/// （CI の 11/01 は書き手が `write_script_in` で panic し、11/02 は読み手が黙って統合なしになり
/// `wait_ready` の 30 秒待ちが切れた）。製品は 1 プロセスで 1 回書いてから読ませるので、
/// テストも「自分のファイルを書いてから自分のペインに読ませる」形に揃える
struct Script {
    _dir: tako_core::test_residue::ScratchDir,
    path: PathBuf,
}

impl std::ops::Deref for Script {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.path
    }
}

/// 統合スクリプトをテストごとの置き場へ書き出す。**リポジトリの正本をそのまま使う**
/// （テスト用に書き直すと、本番と違うものを検証してしまう）
fn write_script() -> Script {
    let dir = tako_core::test_residue::ScratchDir::new("i2659-si");
    let path = write_script_in(dir.path());
    Script { _dir: dir, path }
}

/// 置き場を選んで書き出す（#2432 の「空白入りの置き場」の検査用。置き場は呼び手が
/// `ScratchDir` で作る = 他のテストと共有しない。#2659）
fn write_script_in(dir: &Path) -> PathBuf {
    const SCRIPT: &str = include_str!("../shell-integration/tako.ps1");
    std::fs::create_dir_all(dir).expect("一時ディレクトリを作れること");
    let path = dir.join("tako.ps1");
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(SCRIPT.as_bytes());
    std::fs::write(&path, bytes).expect("統合スクリプトを書けること");
    path
}

/// 実行ポリシーを Restricted にする環境変数（Process スコープ。Windows のクライアント OS の
/// 既定の CurrentUser / LocalMachine と同じ効き方をし、レジストリは触らない。#2432）
const RESTRICTED: (&str, &str) = ("PSExecutionPolicyPreference", "Restricted");

/// スクリプト**ファイル**をドットソースする形（スクリプトそのものの検査用。#2432 より前の
/// `$PROFILE` のブロックと同じく、実行ポリシーの対象になる読ませ方）
fn dot_source_command(program: &str, script: &Path) -> SpawnCommand {
    SpawnCommand {
        program: program.to_string(),
        args: vec![
            "-NoLogo".into(),
            "-NoProfile".into(),
            "-NoExit".into(),
            "-Command".into(),
            ".".into(),
            script.display().to_string(),
        ],
    }
}

fn pwsh7() -> Option<String> {
    let pf = std::env::var("ProgramFiles").ok()?;
    let path = format!("{pf}\\PowerShell\\7\\pwsh.exe");
    Path::new(&path).exists().then_some(path)
}

fn windows_powershell() -> Option<String> {
    let root = std::env::var("SystemRoot").ok()?;
    let path = format!("{root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    Path::new(&path).exists().then_some(path)
}

struct Pane {
    session: TerminalSession,
    rx: futures::channel::mpsc::UnboundedReceiver<SessionEvent>,
}

impl Pane {
    /// 統合スクリプトを読み込ませた PowerShell ペイン。
    ///
    /// `-NoProfile` はユーザーの `$PROFILE` を巻き込まないため（実行環境の
    /// oh-my-posh 等でテストが揺れない）。`TAKO_PANE_ID` は統合スクリプトの発動条件。
    ///
    /// `-Command` の後ろは **語ごとに分けて**渡す（PowerShell が空白で連結する）。
    /// `". 'path'"` の 1 語にすると Windows PowerShell 5.1 が引用符を取りこぼして
    /// ドットソースごと落ちる（実測。pwsh 7 は同じ渡し方でも通るので気づきにくい）
    fn new(program: &str, script: &Path) -> Self {
        Self::new_in(program, script, std::env::temp_dir())
    }

    /// 起動 cwd を明示するペイン（#970 の検証用）。**verbatim な cwd は
    /// `Set-Location` では作れない**（PowerShell の FileSystem プロバイダが
    /// `Cannot find path` で拒否する = 実測）ので、`CreateProcess` へ渡す形でしか
    /// 再現できない
    fn new_in(program: &str, script: &Path, cwd: PathBuf) -> Self {
        Self::spawn(dot_source_command(program, script), cwd, &[])
    }

    /// **製品の起動形**（#2432）のペイン。tako が素のペインで PowerShell を起こすときと
    /// 同じ引数（`shell_integration::powershell_pane_command`）で起こす。`extra_env` で
    /// 実行ポリシー等を足す
    fn product(program: &str, script: &Path, extra_env: &[(&str, &str)]) -> Self {
        Self::spawn(
            tako_core::shell_integration::powershell_pane_command(program, script),
            std::env::temp_dir(),
            extra_env,
        )
    }

    fn spawn(command: SpawnCommand, cwd: PathBuf, extra_env: &[(&str, &str)]) -> Self {
        let mut env = vec![("TAKO_PANE_ID".to_string(), "1".to_string())];
        env.extend(
            extra_env
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string())),
        );
        let (session, rx) = TerminalSession::spawn(
            120,
            40,
            SpawnOptions {
                command: Some(command),
                cwd: Some(cwd),
                env,
                scrollback_lines: None,
            },
        )
        .expect("PowerShell ペインを起動できること");
        Self { session, rx }
    }

    fn pump(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            let _ = self.session.process_event(ev);
        }
    }

    /// 端末の Enter と同じ **CR だけ**を送る。`\r\n` にすると PSReadLine が余りの LF を
    /// 継続行の開始と解釈し、`>>` で次の入力を待ってしまう（実測）。
    /// tako 本体も LF は CR へ正規化して送っている（#95）
    fn send_line(&self, line: &str) {
        self.session.write(format!("{line}\r").into_bytes());
    }

    fn screen(&self) -> String {
        self.session.visible_lines().join("\n")
    }

    /// 条件が満たされるまで待つ。満たされなければ false
    fn wait<F: Fn(&TerminalSession) -> bool>(&mut self, timeout: Duration, pred: F) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            self.pump();
            if pred(&self.session) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_state(&mut self, want: CommandState, timeout: Duration) -> bool {
        self.wait(timeout, |s| s.command_state() == want)
    }

    /// 側路（#766）を汲みながら待つ。
    ///
    /// 製品では tako の定期更新が `drain_osc_sinks` でこれをやる（2 秒 tick）。
    /// テストは待ちを速くするため自分で回すが、**通す経路は製品と同じ**
    /// （`SinkReader::take_new` → `TerminalSession::feed_osc_bytes`。#1199 で
    /// 待ち合わせ先が「書き手の pid つき」になったので読み口も製品と同じものを使う）
    fn drain_sink(&mut self, reader: &mut tako_core::osc_sink::SinkReader) {
        for bytes in reader.take_new() {
            self.session.feed_osc_bytes(&bytes);
        }
    }

    fn wait_state_via_sink(
        &mut self,
        reader: &mut tako_core::osc_sink::SinkReader,
        want: CommandState,
        timeout: Duration,
    ) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            self.pump();
            self.drain_sink(reader);
            if self.session.command_state() == want {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// プロンプトが出る = 統合スクリプトが読み終わって OSC 133;A/B が流れた状態
    fn wait_ready(&mut self) {
        assert!(
            self.wait(Duration::from_secs(30), |s| s.command_state()
                == CommandState::Idle
                && s.cwd().is_some()),
            "統合が起動しない（state={:?} cwd={:?}）\n{}",
            self.session.command_state(),
            self.session.cwd(),
            self.screen()
        );
    }
}

/// **受け入れ条件そのもの**: 統合を読んだ PowerShell ペインで
/// idle → running → idle / failed(exit code) が報告される。
///
/// `Running` の観測は「長く走るコマンドの最中」を狙うので、3 秒眠るコマンドを使う
fn state_transitions(program: &str) {
    detach_own_console();
    let script = write_script();
    run_state_transitions(Pane::new(program, &script));
}

/// [`state_transitions`] の本体。読ませ方（ドットソース / 製品の起動形）を問わず同じ遷移を見る
fn run_state_transitions(mut pane: Pane) {
    pane.wait_ready();

    // 1) 実行中は Running（OSC 133;C）
    pane.send_line("Start-Sleep -Seconds 3");
    assert!(
        pane.wait_state(CommandState::Running, Duration::from_secs(15)),
        "実行中に Running にならない\n{}",
        pane.screen()
    );

    // 2) 正常終了で Idle（OSC 133;D;0）
    assert!(
        pane.wait_state(CommandState::Idle, Duration::from_secs(20)),
        "終了後に Idle へ戻らない\n{}",
        pane.screen()
    );

    // 3) ネイティブ exe の終了コードがそのまま Failed(code) になる
    pane.send_line("cmd.exe /c exit 7");
    assert!(
        pane.wait_state(CommandState::Failed(7), Duration::from_secs(20)),
        "終了コードが Failed(7) にならない（state={:?}）\n{}",
        pane.session.command_state(),
        pane.screen()
    );

    // 4) cmdlet の失敗は $LASTEXITCODE を持たないので Failed(1)
    pane.send_line("Get-Item 'C:\\tako-does-not-exist-9db3'");
    assert!(
        pane.wait_state(CommandState::Failed(1), Duration::from_secs(20)),
        "cmdlet 失敗が Failed(1) にならない（state={:?}）\n{}",
        pane.session.command_state(),
        pane.screen()
    );

    // 5) 成功すれば Failed は解消する
    pane.send_line("Write-Output ok");
    assert!(
        pane.wait_state(CommandState::Idle, Duration::from_secs(20)),
        "成功後も Failed のまま（state={:?}）\n{}",
        pane.session.command_state(),
        pane.screen()
    );
}

/// cwd 追従（OSC 7）。**Windows のドライブレターが落ちないこと**まで見る
/// （`file:///C:/…` の先頭 `/` を残すと存在しないパスになる）
/// `dir_name` は移動先ディレクトリ名。**打鍵経路に日本語を流すかどうか**で分けている
/// （OSC 7 の往復に日本語を通すのは pwsh 7 側で見る。Windows PowerShell 5.1 は
/// 日本語を打ち込むと PSReadLine が行を確定できず、シェル統合とは無関係に止まる）
fn cwd_tracking(program: &str, dir_name: &str) {
    detach_own_console();
    let script = write_script();
    run_cwd_tracking(Pane::new(program, &script), dir_name);
}

/// [`cwd_tracking`] の本体（読ませ方を問わない）
fn run_cwd_tracking(mut pane: Pane, dir_name: &str) {
    pane.wait_ready();

    let start = pane.session.cwd().map(Path::to_path_buf).expect("初期 cwd");
    assert!(
        start.is_absolute() && start.exists(),
        "初期 cwd が実在するパスでない: {start:?}"
    );

    // 固定名安全: 呼び手が pid 入りの名前を渡す（日本語を打鍵経路へ流すので ScratchDir の整形を通せない）
    let target = std::env::temp_dir().join(dir_name);
    std::fs::create_dir_all(&target).expect("移動先を作れること");
    pane.send_line(&format!("Set-Location '{}'", target.display()));

    let ok = pane.wait(Duration::from_secs(20), |s| {
        s.cwd().is_some_and(|c| same_dir(c, &target))
    });
    assert!(
        ok,
        "cd に追従しない（cwd={:?} 期待={target:?}）\n{}",
        pane.session.cwd(),
        pane.screen()
    );
    let _ = std::fs::remove_dir_all(&target);
}

/// 区切り文字・ドライブレターの大小・**8.3 短縮名**の違いを許容して比較する。
///
/// CI ランナーの `%TEMP%` は `C:\Users\<8.3 短縮名>\…` の**短縮形**で、PowerShell は
/// 長い名前（`C:\Users\<winuser>\…`）を報告する（#1278 で 3 本が落ちた）。
/// `canonicalize` が両方を長い名前へ寄せるので、実在するあいだはこれで揃う
/// （返りは verbatim `\\?\C:\…` なので prefix を落としてから比べる）
fn same_dir(a: &Path, b: &Path) -> bool {
    fn norm(p: &Path) -> String {
        let resolved = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let s = resolved.to_string_lossy().replace('/', "\\").to_lowercase();
        s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
    }
    norm(a) == norm(b)
}

#[test]
fn pwsh7のペインで状態が_idle_running_failed_と報告される() {
    let Some(program) = pwsh7() else {
        eprintln!("skip: PowerShell 7 が無い");
        return;
    };
    state_transitions(&program);
}

#[test]
fn windowspowershell51のペインでも状態が報告される() {
    let Some(program) = windows_powershell() else {
        eprintln!("skip: Windows PowerShell 5.1 が無い");
        return;
    };
    state_transitions(&program);
}

#[test]
fn pwsh7のペインでcwdが追従する() {
    let Some(program) = pwsh7() else {
        eprintln!("skip: PowerShell 7 が無い");
        return;
    };
    // 空白 + 日本語で percent エンコードの往復まで見る
    cwd_tracking(&program, &format!("tako si 作業 {}", std::process::id()));
}

/// **#970 の回帰**: 起動 cwd が verbatim でも OSC 7 の cwd が壊れないこと。
///
/// tako 自身は `canonicalize` の戻りから prefix を落とす（境界 B26 =
/// `tako_core::platform::path`）。それでもシェルが verbatim な作業ディレクトリを
/// **継承する**経路は残る（tako 自体が verbatim な cwd で起動された / #970 より前の版が
/// 保存した layout の cwd で開き直した）。**`Set-Location` では作れない**（PowerShell の
/// FileSystem プロバイダが `Cannot find path` で拒否する = 実測）ので、再現は
/// `CreateProcess` へ渡す形に限られる。
///
/// 統合スクリプトが剥がさないと `\` → `/` の置換で `//?/C:/…` になり、OSC 7 を解いた
/// cwd は `///?/C:/…`（実在しないパス）へ壊れる
#[test]
fn 起動cwdがverbatimでもoscのcwdが壊れない() {
    let Some(program) = pwsh7() else {
        eprintln!("skip: PowerShell 7 が無い");
        return;
    };
    detach_own_console();
    let script = write_script();
    let plain = std::env::temp_dir().join(format!("tako 970 verbatim {}", std::process::id()));
    let _ = std::fs::remove_dir_all(&plain);
    std::fs::create_dir_all(&plain).expect("起動 cwd を作れること");
    let verbatim = PathBuf::from(format!(r"\\?\{}", plain.display()));

    let mut pane = Pane::new_in(&program, &script, verbatim);
    pane.wait_ready();

    let reported = pane
        .session
        .cwd()
        .map(Path::to_path_buf)
        .expect("初期 cwd が報告されること");
    let text = reported.display().to_string();
    assert!(
        !text.contains('?'),
        "OSC 7 の cwd に verbatim の残骸が入っている: {text}\n{}",
        pane.screen()
    );
    assert!(
        same_dir(&reported, &plain),
        "起動 cwd と報告された cwd が違う（報告={text} 期待={}）\n{}",
        plain.display(),
        pane.screen()
    );
    let _ = std::fs::remove_dir_all(&plain);
}

#[test]
fn windowspowershell51のペインでもcwdが追従する() {
    let Some(program) = windows_powershell() else {
        eprintln!("skip: Windows PowerShell 5.1 が無い");
        return;
    };
    cwd_tracking(&program, &format!("tako si dir {}", std::process::id()));
}

/// **器（psmux）は OSC を外へ通さない**ことを実測で固定する（#525）。
///
/// 統合スクリプト自体は器の中でも正しく読み込まれ、`$TMUX` を見て DCS パススルーで
/// 包む（macOS の tmux ではこれで届く）。ところが psmux 3.3.7 は
/// `allow-passthrough on` を**受理するのに素通ししない**: 別途の実測で
/// 素の OSC・DCS（ESC 二重化あり / なし）の 3 形すべてが外側へ出ず、
/// 同時に流した平文だけが届いた。
///
/// そこでこのテストは「届かないこと」を固定する。psmux が素通しに対応したら
/// **落ちて教えてくれる**ので、そのとき `PsmuxBackend` の `osc_passthrough` を
/// true へ倒し、このテストを「届くこと」へ書き換える
#[test]
fn 器の中では統合が読み込まれてもoscが外へ出ない() {
    detach_own_console();
    let Some(program) = pwsh7() else {
        eprintln!("skip: PowerShell 7 が無い");
        return;
    };
    let Some(bin) = psmux_bin() else {
        eprintln!("skip: psmux が無い（TAKO_PSMUX_BIN / PATH）");
        return;
    };
    // psmux は PSMUX_SESSION が見えていると入れ子とみなして起動を拒む（実測）。
    // tako のペインの中から `cargo test` した場合がこれに当たるので、その環境では飛ばす
    // （素の端末 / CI では変数が無いので普通に走る）。
    // 環境変数の除去はプロセス全体に効くので、並列テストの安全のためここでは行わない
    if std::env::var_os("PSMUX_SESSION").is_some() || std::env::var_os("TMUX").is_some() {
        eprintln!(
            "skip: 既に psmux / tmux の中で走っている（PSMUX_SESSION= を外して実行すること）"
        );
        return;
    }
    let script = write_script();
    // **ソケットは必ず隔離する**（psmux の kill-server は -L を落とすと全ソケットを殺す）
    let socket = format!("tako-si-test-{}", std::process::id());
    let owner_dir = std::env::temp_dir().join(format!("tako-si-owners-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&owner_dir);
    let backend = PsmuxBackend::with_parts(
        bin.clone(),
        "3.3.7".into(),
        socket.clone(),
        owner_dir.clone(),
    );
    let name = SessionRef::new(format!("tako-si{}", std::process::id() % 100_000)).unwrap();

    // `-Command` の後ろは複数引数のまま渡す（PowerShell が空白で連結する）。
    // 1 語に空白を入れると psmux → cmd.exe → ConPTY の 3 層で引用符の解釈を跨ぐことになる。
    //
    // プログラムも **PATH 上の `pwsh.exe`**（空白なし）で指定する。絶対パス
    // （`C:\Program Files\…`）にすると psmux が `cmd.exe /c '…'` へ包み直すので、
    // ここで見たい「器のパススルー」ではなく cmd の引用符解釈を試すことになってしまう
    let _ = program;
    let options = SpawnOptions {
        command: Some(SpawnCommand {
            program: "pwsh.exe".to_string(),
            args: vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NoExit".into(),
                "-Command".into(),
                ".".into(),
                script.display().to_string(),
            ],
        }),
        cwd: Some(std::env::temp_dir()),
        env: vec![("TAKO_PANE_ID".into(), "1".into())],
        scrollback_lines: None,
    };
    let (session, rx) = TerminalSession::spawn(120, 40, backend.wrap_spawn(options, &name))
        .expect("psmux クライアントを起動できること");
    let mut pane = Pane { session, rx };

    // 器の後始末は assert より先に必ず通す（**-L 必須**。省くと全ソケットのサーバーが死ぬ）
    let cleanup = || {
        psmux_ctl::kill_server(&bin, &socket);
        let _ = std::fs::remove_dir_all(&owner_dir);
    };

    // 器の中のシェルが起動してプロンプトを出すまで待つ（統合が読まれるのはこのとき）
    let started = pane.wait(Duration::from_secs(40), |s| {
        s.visible_lines().iter().any(|l| l.contains("PS "))
    });
    // 統合が動いていれば OSC 133 で Idle になるはず。**器を通らないので Unknown のまま**
    pane.send_line("cmd.exe /c exit 3");
    let leaked = pane.wait_state(CommandState::Failed(3), Duration::from_secs(15));
    let screen = pane.screen();
    let state = pane.session.command_state();
    let cwd = pane.session.cwd().map(Path::to_path_buf);
    drop(pane);
    cleanup();
    assert!(started, "器の中でシェルが起動しない\n{screen}");
    assert!(
        !leaked,
        "psmux が OSC を素通しするようになった（歓迎すべき変化）。\
         PsmuxBackend の osc_passthrough を true にして、このテストを\
         「届くこと」の検証へ書き換えること\n{screen}"
    );
    assert_eq!(
        state,
        CommandState::Unknown,
        "器の中では OSC 133 が届かない前提が崩れた\n{screen}"
    );
    // cwd も OSC 7 ではなく spawn 時の値のまま（区切りが `\` = tako が渡した値）
    assert!(
        cwd.is_some_and(|c| c.to_string_lossy().contains('\\')),
        "cwd が OSC 7 由来に見える（前提が崩れた）"
    );

    // 器の能力申告が実測と一致していること（呼び出し側はここだけ見れば済む）
    assert!(
        !backend.capabilities().osc_passthrough,
        "psmux の osc_passthrough 申告が実測と食い違っている"
    );
}

/// **#766 の本体**: 器（psmux）の中でも側路を張れば cwd 追従とコマンド実行状態が届く。
///
/// 直前のテストが「器は OSC を外へ出さない」ことを固定している。**その事実は変わらない**
/// （psmux はパースして画面モデルへ落とし描き直す多重化器で、`allow-passthrough` は
/// upstream で値を読む側が無く DCS の tmux 形式も未実装 = 器の側では直らない）。
/// 変わったのは tako が素通しに依存しなくなったことで、統合スクリプトが同じ OSC
/// バイト列を `TAKO_OSC_SINK` のファイルへ書き、tako がそれを PTY と同じ
/// `osc_tap` へ通す。
///
/// ここが緑なら「ペインの状態ドットが idle / running / failed になり、cwd が cd に
/// 追従する」ことの実測になる（`tako list` の `state` / `cwd` はこの 2 つがそのまま出る）
#[test]
fn 器の中でも側路を張れば状態とcwdが届く() {
    detach_own_console();
    if pwsh7().is_none() {
        eprintln!("skip: PowerShell 7 が無い");
        return;
    }
    let Some(bin) = psmux_bin() else {
        eprintln!("skip: psmux が無い（TAKO_PSMUX_BIN / PATH）");
        return;
    };
    if std::env::var_os("PSMUX_SESSION").is_some() || std::env::var_os("TMUX").is_some() {
        eprintln!(
            "skip: 既に psmux / tmux の中で走っている（PSMUX_SESSION= を外して実行すること）"
        );
        return;
    }
    let script = write_script();
    let socket = format!("tako-si-sink-{}", std::process::id());
    let owner_dir =
        std::env::temp_dir().join(format!("tako-si-sink-owners-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&owner_dir);
    let backend = PsmuxBackend::with_parts(
        bin.clone(),
        "3.3.7".into(),
        socket.clone(),
        owner_dir.clone(),
    );
    let name = SessionRef::new(format!("tako-sk{}", std::process::id() % 100_000)).unwrap();

    // 側路の書き先は製品と同じ組み立て（`osc_sink::prepare`）で作る。
    //
    // **ディレクトリ名にあえて空白を入れている**: 書き先は器へ
    // `new-session -e TAKO_OSC_SINK=<path>` の**引数**として渡るので、
    // 引数のエスケープが崩れていると 3 語へ割れて器へ届かない（#884 で
    // `-c <空白入り cwd>` が同じ機序で即死した）。既定の data_dir は
    // `%APPDATA%\tako` で空白を含まないが、**ユーザー名に空白がある Windows
    // （`C:\Users\John Smith\...`）では本番構成そのものが該当する**
    let data_dir = std::env::temp_dir().join(format!("tako si sink {}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data_dir);
    std::fs::create_dir_all(&data_dir).expect("データディレクトリを作れること");
    let sink = tako_core::osc_sink::prepare(&data_dir, 1).expect("側路を張れること");

    // cd の行き先。**器の中で作る**のではなく先に作っておく（cd が失敗すると OSC 7 が出ない）。
    //
    // 名前を極端に短くしてあるのは器の都合。psmux は流し込んだ入力を落とすことがあり
    // （#640。だから製品の送達はエコー確認つきの `shell_send` を通る）、絶対パスを
    // そのまま打つと **行が消えて `\r` だけ届く**（実測: 60 文字の `cd '<絶対パス>'` が
    // 空行になり、プロンプトだけが 1 つ増えた）。cwd は Temp 直下から相対で移る
    let leaf = format!("t766{}", std::process::id() % 1000);
    let target = std::env::temp_dir().join(&leaf);
    std::fs::create_dir_all(&target).expect("cd 先を作れること");

    let options = SpawnOptions {
        command: Some(SpawnCommand {
            program: "pwsh.exe".to_string(),
            args: vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NoExit".into(),
                "-Command".into(),
                ".".into(),
                script.display().to_string(),
            ],
        }),
        cwd: Some(std::env::temp_dir()),
        env: vec![
            ("TAKO_PANE_ID".into(), "1".into()),
            (
                tako_core::osc_sink::SINK_ENV.into(),
                sink.display().to_string(),
            ),
        ],
        scrollback_lines: None,
    };
    let (session, rx) = TerminalSession::spawn(120, 40, backend.wrap_spawn(options, &name))
        .expect("psmux クライアントを起動できること");
    let mut pane = Pane { session, rx };
    // #1199: 読み口は製品と同じ `SinkReader`。**そのペインのシェルの pid** を
    // 器へ聞いて張るまで解決済みのファイルは読まない（器の中の別のシェルが
    // 同じ待ち合わせ先へ書けてしまうため）
    let mut reader = tako_core::osc_sink::SinkReader::new(&data_dir, 1);
    let mut writer_pid = None;
    for _ in 0..60 {
        writer_pid = tako_core::tmux_backend::pane_facts(&socket, name.as_str()).pid;
        if let Some(pid) = writer_pid {
            reader.set_writer_pid(pid);
            break;
        }
        pane.pump();
        std::thread::sleep(Duration::from_millis(250));
    }

    let cleanup = || {
        psmux_ctl::kill_server(&bin, &socket);
        let _ = std::fs::remove_dir_all(&owner_dir);
        let _ = std::fs::remove_dir_all(&data_dir);
        let _ = std::fs::remove_dir_all(&target);
    };

    // ① 起動して最初のプロンプトが出たら Idle（= 133;A が側路で届いた）
    let idle = pane.wait_state_via_sink(&mut reader, CommandState::Idle, Duration::from_secs(45));
    // ② 非ゼロ終了のコマンドで Failed(3)（= 133;D;3 が側路で届いた。終了コードつき）
    pane.send_line("cmd.exe /c exit 3");
    let failed = pane.wait_state_via_sink(
        &mut reader,
        CommandState::Failed(3),
        Duration::from_secs(20),
    );
    // ③ cd で cwd が追従する（= OSC 7 が側路で届いた）。
    // 器が入力を落とすことがあるので**エコーを見て打ち直す**（落ちたときに
    // 「OSC 7 が来ない」ではなく「入力が届いていない」と分かる形にしておく）
    let line = format!("cd {leaf}");
    let mut echoed = false;
    let mut followed = false;
    for _ in 0..3 {
        pane.send_line(&line);
        echoed |= pane.wait(Duration::from_secs(6), |s| {
            s.visible_lines().iter().any(|l| l.contains(&line))
        });
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            pane.pump();
            pane.drain_sink(&mut reader);
            followed = pane
                .session
                .cwd()
                .is_some_and(|c| c.to_string_lossy().contains(&leaf));
            if followed || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if followed {
            break;
        }
    }
    // ④ #1199: 器の中の**別のシェル**（psmux のプリウォーム済みプール）が同じペインの
    // 側路へ書いても、そのペインの cwd は動いてはいけない。器はペイン固有の env を
    // サーバーのグローバル環境へ入れるので、旧実装（パスだけの待ち合わせ）では
    // ウォームプールのシェルが `OSC 7 <ホーム>` を書き、リサイズのたびに
    // ペインの cwd がホームへ巻き戻っていた。
    //
    // **比べる相手は「書く前の cwd」**にする。書いた後の値を 2 回読んで比べると、
    // 巻き戻っていても両方が巻き戻り先を指すので必ず等しくなり検証にならない
    let cwd_before_foreign = pane.session.cwd().map(Path::to_path_buf);
    // 固定名安全: 作らない・消さない（別のシェルの OSC 7 の本文に載せる文字列だけ）
    let foreign_home = std::env::temp_dir().join("tako-1199-foreign");
    let foreign = tako_core::osc_sink::writer_sink_path(&data_dir, 1, 999_999);
    let bundle = format!(
        "\x1b]7;file:///{}\x07",
        foreign_home.display().to_string().replace('\\', "/")
    );
    std::fs::write(&foreign, bundle.as_bytes()).expect("別のシェルのぶんを置ける");
    for _ in 0..10 {
        pane.pump();
        pane.drain_sink(&mut reader);
        std::thread::sleep(Duration::from_millis(20));
    }
    let cwd_after_foreign = pane.session.cwd().map(Path::to_path_buf);

    let screen = pane.screen();
    let state = pane.session.command_state();
    let cwd = pane.session.cwd().map(Path::to_path_buf);
    drop(pane);
    cleanup();

    // 器が `#{pane_pid}` を答えないと側路の待ち合わせ先を張れない（#1199）。
    // ここが偽なら以下の 3 本も必ず落ちるので、原因が分かる形で先に見る
    assert!(
        writer_pid.is_some(),
        "器が `#{{pane_pid}}` を答えない（#1199。側路の待ち合わせ先を張れない = \
         cwd 追従とコマンド状態が届かなくなる）\n{screen}"
    );
    assert!(idle, "側路で 133;A が届かない（state={state:?}）\n{screen}");
    assert!(
        failed,
        "側路で 133;D の終了コードが届かない（state={state:?}）\n{screen}"
    );
    assert!(
        echoed,
        "器が cd の入力を落として画面に出ない（#640。OSC 7 の検証まで到達していない）\n{screen}"
    );
    assert!(
        followed,
        "側路で OSC 7 が届かない（cwd={cwd:?} 期待={leaf}）\n{screen}"
    );
    assert_eq!(
        cwd_after_foreign, cwd_before_foreign,
        "器の中の別のシェルが書いたぶんでペインの cwd が動いた（#1199 の巻き戻り。\
         別のシェルの書き先={foreign:?} その cwd={foreign_home:?}）\n{screen}"
    );

    // 器の能力申告は変えていない: psmux は今も素通ししない（側路がそれを補っている）
    assert!(
        !backend.capabilities().osc_passthrough,
        "psmux の osc_passthrough 申告を変えてはいけない（#766 は素通しを直していない）"
    );
    assert_eq!(
        tako_core::shell_integration::osc_transport(),
        tako_core::shell_integration::OscTransport::SideChannel,
        "この構成の申告が side-channel になっていない"
    );
}

// --- #2432: 製品の起動形（`$PROFILE` に書かず、ペインの起動引数で読ませる）---------------

/// **#2432 の受け入れ条件 1**: 実行ポリシーが Restricted でも、製品の起動形のペインは
/// idle → running → idle / failed(exit code) を報告する（スクリプトファイルを実行しないので
/// ポリシーの対象外）。PowerShell 7
#[test]
fn 製品の起動形はrestrictedでもpwsh7のペインで状態が報告される() {
    let Some(program) = pwsh7() else {
        eprintln!("skip: PowerShell 7 が無い");
        return;
    };
    detach_own_console();
    let script = write_script();
    run_state_transitions(Pane::product(&program, &script, &[RESTRICTED]));
}

/// 同じく Windows PowerShell 5.1（Windows に必ず居る側。pwsh 7 の無い PC の既定シェル）
#[test]
fn 製品の起動形はrestrictedでもwindowspowershell51のペインで状態が報告される() {
    let Some(program) = windows_powershell() else {
        eprintln!("skip: Windows PowerShell 5.1 が無い");
        return;
    };
    detach_own_console();
    let script = write_script();
    run_state_transitions(Pane::product(&program, &script, &[RESTRICTED]));
}

/// 実行ポリシーが Restricted でも cwd が追従する（OSC 7）
#[test]
fn 製品の起動形はrestrictedでもcwdが追従する() {
    let Some(program) = pwsh7().or_else(windows_powershell) else {
        eprintln!("skip: PowerShell が無い");
        return;
    };
    detach_own_console();
    let script = write_script();
    run_cwd_tracking(
        Pane::product(&program, &script, &[RESTRICTED]),
        &format!("tako-2432-cwd-{}", std::process::id()),
    );
}

/// **このテスト群の検出力**: 同じ Restricted で、スクリプト**ファイル**をドットソースする
/// 読ませ方（#2432 より前の `$PROFILE` のブロックと同じ）は統合が読めない。ここが崩れたら
/// 上の 3 本は Restricted を作れていない（= 緑でも受け入れ条件の証拠にならない）
#[test]
fn ファイルをドットソースする読ませ方はrestrictedでは統合が読めない() {
    let Some(program) = pwsh7().or_else(windows_powershell) else {
        eprintln!("skip: PowerShell が無い");
        return;
    };
    detach_own_console();
    let script = write_script();
    let mut pane = Pane::spawn(
        dot_source_command(&program, &script),
        std::env::temp_dir(),
        &[RESTRICTED],
    );
    // プロンプトが出るまで待つ（統合が読めていれば Idle になるが、ここではならない）
    let prompted = pane.wait(Duration::from_secs(30), |s| {
        s.visible_lines().iter().any(|l| l.contains("PS "))
    });
    pane.send_line("cmd.exe /c exit 3");
    let leaked = pane.wait_state(CommandState::Failed(3), Duration::from_secs(10));
    assert!(prompted, "プロンプトが出ない\n{}", pane.screen());
    assert!(
        !leaked,
        "Restricted なのにスクリプトファイルが読めている（RESTRICTED の作り方が効いていない）\n{}",
        pane.screen()
    );
    assert_eq!(
        pane.session.command_state(),
        CommandState::Unknown,
        "{}",
        pane.screen()
    );
}

/// 製品の起動形は**ペインの実行ポリシーを緩めない**。`-ExecutionPolicy Bypass` で起こすと
/// Process スコープ（環境変数 `PSExecutionPolicyPreference`）が Bypass になり、ペインの中で
/// 起こす子の PowerShell（エージェントや npm が起こすもの）まで Bypass を受け継ぐ
#[test]
fn 製品の起動形はペインの実行ポリシーを緩めない() {
    let Some(program) = pwsh7().or_else(windows_powershell) else {
        eprintln!("skip: PowerShell が無い");
        return;
    };
    detach_own_console();
    let script = write_script();
    let mut pane = Pane::product(&program, &script, &[RESTRICTED]);
    pane.wait_ready();
    // 打った行（`"EP=" + …`）には `EP=Restricted` が現れないので、出力だけを拾える
    pane.send_line(
        "Write-Output ('EP=' + (Get-ExecutionPolicy) + ';ENV=' + $env:PSExecutionPolicyPreference)",
    );
    let ok = pane.wait(Duration::from_secs(20), |s| {
        s.visible_lines()
            .iter()
            .any(|l| l.contains("EP=Restricted;ENV=Restricted"))
    });
    assert!(ok, "ペインの実行ポリシーが変わっている\n{}", pane.screen());
}

/// 統合スクリプトの置き場に空白があっても読める（`-EncodedCommand` で渡すので、ConPTY の
/// argv・PowerShell の引数解釈の層で引用符が崩れない。ユーザー名に空白がある PC の
/// `%APPDATA%\tako` がこれに当たる）。5.1 は引用符の取りこぼしで落ちやすい側なのでこちらで見る
#[test]
fn 置き場に空白があっても製品の起動形で読める() {
    let Some(program) = windows_powershell().or_else(pwsh7) else {
        eprintln!("skip: PowerShell が無い");
        return;
    };
    detach_own_console();
    let dir = tako_core::test_residue::ScratchDir::new("i2432-space");
    let script = write_script_in(&dir.path().join("tako data").join("shell integration"));
    let mut pane = Pane::product(&program, &script, &[RESTRICTED]);
    pane.wait_ready();
    pane.send_line("cmd.exe /c exit 5");
    assert!(
        pane.wait_state(CommandState::Failed(5), Duration::from_secs(20)),
        "空白入りの置き場から統合が読めていない（state={:?}）\n{}",
        pane.session.command_state(),
        pane.screen()
    );
}

/// 置き場が消えていても**赤いエラーを出さず**素のシェルとして使える（data dir を消された・
/// 器の中の古いセッションが前の置き場を指している、等）
#[test]
fn 置き場が無ければ黙って素のシェルになる() {
    let Some(program) = pwsh7().or_else(windows_powershell) else {
        eprintln!("skip: PowerShell が無い");
        return;
    };
    detach_own_console();
    let dir = tako_core::test_residue::ScratchDir::new("i2432-missing");
    let missing = dir.path().join("gone").join("tako.ps1");
    let mut pane = Pane::product(&program, &missing, &[RESTRICTED]);
    let prompted = pane.wait(Duration::from_secs(30), |s| {
        s.visible_lines().iter().any(|l| l.contains("PS "))
    });
    pane.send_line("Write-Output ('ALIVE=' + (1 + 1))");
    let alive = pane.wait(Duration::from_secs(20), |s| {
        s.visible_lines().iter().any(|l| l.contains("ALIVE=2"))
    });
    let screen = pane.screen();
    assert!(prompted && alive, "シェルとして使えない\n{screen}");
    assert_eq!(
        pane.session.command_state(),
        CommandState::Unknown,
        "{screen}"
    );
    // 起動コマンドの中身（パスや Test-Path）が画面にエラーとして出ていない
    assert!(
        !screen.contains("tako.ps1") && !screen.contains("__takoShellIntegrationScript"),
        "起動コマンドがエラーとして画面に出ている\n{screen}"
    );
    // 置き場が無いのは「読めなかった」ではない（#2659 の警告は出さない）
    assert!(!warned(&screen), "置き場が無いだけで警告を出した\n{screen}");
}

// --- #2659: 統合スクリプトの共有違反（テストの置き場・製品の読み直し・PSReadLine 抜き）-----------

/// 統合が入らなかったときの警告（製品の [`tako_core::shell_integration::powershell_bootstrap`]）が
/// 画面に出ているか。表示言語はテストプロセスの既定（英語）だが、どちらでも拾う
fn warned(screen: &str) -> bool {
    screen.contains("could not load the shell integration")
        || screen.contains("シェル統合を読み込めませんでした")
}

/// ファイルを**他の誰にも開かせない**形で開いて握る（共有違反の注入）。
/// 書き手の `std::fs::write` も読み手の `ReadAllText` も、握っている間は OS エラー 32 で失敗する
fn hold_exclusive(path: &Path) -> std::fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .expect("排他で開けること")
}

/// 製品の起動形の本文の前に「これから読む」印を置いて起こす（#2659 の注入）。
///
/// 印のファイルは本文の 1 文目で書かれ、次の文が統合スクリプトの `Test-Path` → `ReadAllText`。
/// 印が現れたときにロックを握っていれば、1 回目の読み込みは必ず共有違反になる
/// （PowerShell の起動にかかる時間がばらついても、ロックを放す時刻を印から数えるので揺れない）
fn product_with_marker(program: &str, script: &Path, marker: &Path) -> Pane {
    let marker = marker.display().to_string().replace('\'', "''");
    let body = format!(
        "[System.IO.File]::WriteAllText('{marker}', 'x'); {}",
        tako_core::shell_integration::powershell_bootstrap(script)
    );
    Pane::spawn(
        SpawnCommand {
            program: program.to_string(),
            args: vec![
                "-NoExit".into(),
                "-EncodedCommand".into(),
                tako_core::platform::shell::encode_powershell_command(&body),
            ],
        },
        std::env::temp_dir(),
        &[RESTRICTED],
    )
}

/// 注入でロックを握る長さ。製品の読み直しの上限（回数 × 間隔 = 0.5 秒）より短く、
/// 間隔（50ms）の数倍 = 何度か読み直してから読める長さ
const HOLD: Duration = Duration::from_millis(250);

#[test]
fn 注入の長さは製品の読み直しの上限に収まる() {
    let budget = tako_core::shell_integration::POWERSHELL_SCRIPT_READ_RETRY_INTERVAL
        * tako_core::shell_integration::POWERSHELL_SCRIPT_READ_RETRIES;
    assert!(
        HOLD * 3 / 2 < budget,
        "ロックを握る長さ {HOLD:?} が読み直しの上限 {budget:?} に余裕を持って収まっていない"
    );
    assert!(
        HOLD > tako_core::shell_integration::POWERSHELL_SCRIPT_READ_RETRY_INTERVAL * 2,
        "ロックが短すぎて読み直しを通らない"
    );
}

/// **#2659 の A**: テストごとの置き場は、他のテストのスクリプトが排他で握られていても書ける。
/// 検出力として、握られた同じパスへ書けば共有違反（OS エラー 32）で落ちる = 修正前に全テストが
/// 同じパスへ書いていた形（11/01 の CI で `write_script_in` が panic したのと同じ失敗）
#[test]
fn issue2659_テストごとの置き場は他のテストの排他に触れない() {
    let mine = write_script();
    let held = hold_exclusive(&mine);
    let err = std::fs::write(&*mine, b"x")
        .expect_err("排他で握ったファイルへ書けてしまう（注入が効いていない）");
    assert_eq!(err.raw_os_error(), Some(32), "共有違反ではない: {err:?}");
    // 別のテストの書き出し（修正後の形）は自分の置き場へ書くので、握られていても通る
    let other = write_script();
    assert_ne!(&*mine, &*other, "置き場がテストごとに分かれていない");
    assert!(other.is_file(), "{:?}", &*other);
    drop(held);
}

/// [`issue2659_ロックが外れれば読み直して統合が入る`] の本体（PowerShell 7 / 5.1 で同じ形）
fn read_after_release(program: &str) {
    detach_own_console();
    let dir = tako_core::test_residue::ScratchDir::new("i2659-hold");
    let script = write_script_in(dir.path());
    let marker = dir.path().join("reading");
    let held = hold_exclusive(&script);
    let mut pane = product_with_marker(program, &script, &marker);
    assert!(
        pane.wait(Duration::from_secs(30), |_| marker.exists()),
        "本文の 1 文目が走らない\n{}",
        pane.screen()
    );
    // 1 回目の読み込みは握っている間に起きる。読み直しの途中で放す
    std::thread::sleep(HOLD);
    drop(held);
    pane.wait_ready();
    pane.send_line("cmd.exe /c exit 4");
    assert!(
        pane.wait_state(CommandState::Failed(4), Duration::from_secs(20)),
        "読み直した統合で終了コードが報告されない（state={:?}）\n{}",
        pane.session.command_state(),
        pane.screen()
    );
    assert!(
        !warned(&pane.screen()),
        "読めたのに警告を出した\n{}",
        pane.screen()
    );
}

/// **#2659 の B（修正後）**: 読み込みの瞬間に別の誰かがスクリプトを握っていても、放されれば
/// 読み直して統合が入る（ロックは 1 回目の読み込みの前に握り、[`HOLD`] の後に放す）
#[test]
fn issue2659_ロックが外れれば読み直して統合が入る() {
    let programs: Vec<String> = pwsh7().into_iter().chain(windows_powershell()).collect();
    if programs.is_empty() {
        eprintln!("skip: PowerShell が無い");
        return;
    }
    for program in programs {
        read_after_release(&program);
    }
}

/// **#2659 の B の A/B**: `TAKO_2659_LEGACY=1`（修正前の本文 = 1 回だけ読んで黙る）は、同じ注入で
/// 統合が入らず、理由も出ない（11/02 の CI の画面と同じ = state Unknown のままプロンプトだけが出る）
#[test]
fn issue2659_legacyでは一度の失敗で統合が入らない() {
    let Some(program) = pwsh7().or_else(windows_powershell) else {
        eprintln!("skip: PowerShell が無い");
        return;
    };
    if tako_core::run_alone!(issue2659_legacyでは一度の失敗で統合が入らない) {
        return;
    }
    // 子の中だけ（他のテストは居ない。#2400）
    unsafe { std::env::set_var("TAKO_2659_LEGACY", "1") };
    detach_own_console();
    let dir = tako_core::test_residue::ScratchDir::new("i2659-legacy");
    let script = write_script_in(dir.path());
    let marker = dir.path().join("reading");
    let held = hold_exclusive(&script);
    let mut pane = product_with_marker(&program, &script, &marker);
    assert!(
        pane.wait(Duration::from_secs(30), |_| marker.exists()),
        "本文の 1 文目が走らない\n{}",
        pane.screen()
    );
    std::thread::sleep(HOLD);
    drop(held);
    let prompted = pane.wait(Duration::from_secs(30), |s| {
        s.visible_lines().iter().any(|l| l.contains("PS "))
    });
    pane.send_line("cmd.exe /c exit 4");
    let reported = pane.wait_state(CommandState::Failed(4), Duration::from_secs(10));
    let screen = pane.screen();
    assert!(prompted, "プロンプトが出ない\n{screen}");
    assert!(
        !reported,
        "修正前の本文なのに統合が入った（注入が 1 回目の読み込みに当たっていない）\n{screen}"
    );
    assert_eq!(
        pane.session.command_state(),
        CommandState::Unknown,
        "{screen}"
    );
    assert!(!warned(&screen), "修正前の本文が警告を出した\n{screen}");
}

/// **#2659 の B（読めないまま）**: 読み直しの上限まで握られたままなら統合は入らず、そのことが
/// 1 行の警告で画面に出る（修正前は黙って入らなかった）。シェルとしてはそのまま使える
#[test]
fn issue2659_読めないままなら統合が入らなかったと画面に出る() {
    let Some(program) = pwsh7().or_else(windows_powershell) else {
        eprintln!("skip: PowerShell が無い");
        return;
    };
    detach_own_console();
    let dir = tako_core::test_residue::ScratchDir::new("i2659-busy");
    let script = write_script_in(dir.path());
    let marker = dir.path().join("reading");
    let held = hold_exclusive(&script);
    let mut pane = product_with_marker(&program, &script, &marker);
    let shown = pane.wait(Duration::from_secs(30), |s| {
        warned(&s.visible_lines().join("\n"))
    });
    pane.send_line("Write-Output ('ALIVE=' + (1 + 1))");
    let alive = pane.wait(Duration::from_secs(20), |s| {
        s.visible_lines().iter().any(|l| l.contains("ALIVE=2"))
    });
    let screen = pane.screen();
    drop(held);
    assert!(shown, "読めなかったのに警告が出ない\n{screen}");
    assert!(alive, "シェルとして使えない\n{screen}");
    assert_eq!(
        pane.session.command_state(),
        CommandState::Unknown,
        "{screen}"
    );
    assert!(
        !screen.contains("__takoShellIntegration"),
        "一時変数の名前が画面に漏れている\n{screen}"
    );
}

/// PSReadLine が読めなかったペイン（11/02 の CI の画面の `Cannot load PSReadline module.`）でも、
/// 製品の起動形の統合は状態を報告する。PSReadLine を本文の前で外して、統合の
/// PSReadLine 抜きの経路（`tako.ps1` の `PreCommandLookupAction`）を決定的に通す
fn without_psreadline(program: &str) {
    detach_own_console();
    let script = write_script();
    let body = format!(
        "Remove-Module PSReadLine -Force -ErrorAction SilentlyContinue; {}",
        tako_core::shell_integration::powershell_bootstrap(&script)
    );
    let pane = Pane::spawn(
        SpawnCommand {
            program: program.to_string(),
            args: vec![
                "-NoExit".into(),
                "-EncodedCommand".into(),
                tako_core::platform::shell::encode_powershell_command(&body),
            ],
        },
        std::env::temp_dir(),
        &[RESTRICTED],
    );
    run_state_transitions(pane);
}

#[test]
fn psreadlineが無くてもpwsh7のペインで状態が報告される() {
    let Some(program) = pwsh7() else {
        eprintln!("skip: PowerShell 7 が無い");
        return;
    };
    without_psreadline(&program);
}

#[test]
fn psreadlineが無くてもwindowspowershell51のペインで状態が報告される() {
    let Some(program) = windows_powershell() else {
        eprintln!("skip: Windows PowerShell 5.1 が無い");
        return;
    };
    without_psreadline(&program);
}

fn psmux_bin() -> Option<String> {
    let candidate = std::env::var("TAKO_PSMUX_BIN")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "psmux".to_string());
    psmux_ctl::probe(&candidate).then_some(candidate)
}

/// 統合を入れていないペインでは何も起きない（= 統合が状態の唯一の出どころだと確かめる）。
/// これが無いと、上のテストが「たまたま Idle だった」だけでも通ってしまう
#[test]
fn 統合なしのペインでは状態もcwdも報告されない() {
    detach_own_console();
    let Some(program) = pwsh7() else {
        eprintln!("skip: PowerShell 7 が無い");
        return;
    };
    let (session, mut rx) = TerminalSession::spawn(
        120,
        40,
        SpawnOptions {
            command: Some(SpawnCommand {
                program,
                args: vec!["-NoLogo".into(), "-NoProfile".into()],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
            scrollback_lines: None,
        },
    )
    .expect("ペインを起動できること");

    let mut session = session;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        while let Ok(ev) = rx.try_recv() {
            let _ = session.process_event(ev);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // 起動 cwd はセッションが保持するが、OSC 133 由来の状態遷移は起きない
    assert_eq!(
        session.command_state(),
        CommandState::Unknown,
        "統合なしで状態が付いている（このテストの検出力が無い）"
    );
}
