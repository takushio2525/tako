//! PowerShell シェル統合（OSC 7 / 133）の統合テスト（#525）。**実際にシェルを起動して測る**。
//!
//! 単体テスト（`shell_integration.rs` 内）は「tako が `$PROFILE` へ何を書くか」を固定する。
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
//! 統合スクリプトは `-NoProfile` で起動したシェルへドットソースで読ませる
//! （`$PROFILE` への配置そのものは単体テストと実機 e2e の担当）。

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tako_core::backend::{PsmuxBackend, SessionBackend, SessionRef};
use tako_core::terminal::{
    CommandState, SessionEvent, SpawnCommand, SpawnOptions, TerminalSession,
};

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

/// 統合スクリプトを一時ディレクトリへ書き出す。**リポジトリの正本をそのまま使う**
/// （テスト用に書き直すと、本番と違うものを検証してしまう）
fn write_script() -> PathBuf {
    const SCRIPT: &str = include_str!("../shell-integration/tako.ps1");
    let dir = std::env::temp_dir().join(format!("tako-si-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("一時ディレクトリを作れること");
    let path = dir.join("tako.ps1");
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(SCRIPT.as_bytes());
    std::fs::write(&path, bytes).expect("統合スクリプトを書けること");
    path
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
        let command = SpawnCommand {
            program: program.to_string(),
            args: vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NoExit".into(),
                "-Command".into(),
                ".".into(),
                script.display().to_string(),
            ],
        };
        let (session, rx) = TerminalSession::spawn(
            120,
            40,
            SpawnOptions {
                command: Some(command),
                cwd: Some(std::env::temp_dir()),
                env: vec![("TAKO_PANE_ID".into(), "1".into())],
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
    let mut pane = Pane::new(program, &script);
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
    let mut pane = Pane::new(program, &script);
    pane.wait_ready();

    let start = pane.session.cwd().map(Path::to_path_buf).expect("初期 cwd");
    assert!(
        start.is_absolute() && start.exists(),
        "初期 cwd が実在するパスでない: {start:?}"
    );

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

/// 区切り文字とドライブレターの大小だけが違う場合を許容して比較する
fn same_dir(a: &Path, b: &Path) -> bool {
    fn norm(p: &Path) -> String {
        p.to_string_lossy().replace('/', "\\").to_lowercase()
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
    };
    let (session, rx) = TerminalSession::spawn(120, 40, backend.wrap_spawn(options, &name))
        .expect("psmux クライアントを起動できること");
    let mut pane = Pane { session, rx };

    // 器の後始末は assert より先に必ず通す（**-L 必須**。省くと全ソケットのサーバーが死ぬ）
    let cleanup = || {
        let _ = std::process::Command::new(&bin)
            .args(["-L", &socket, "kill-server"])
            .output();
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

fn psmux_bin() -> Option<String> {
    let candidate = std::env::var("TAKO_PSMUX_BIN")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "psmux".to_string());
    std::process::Command::new(&candidate)
        .arg("-V")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| candidate)
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
