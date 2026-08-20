//! ペインの文字コードの回帰テスト（#655）。**実際に ConPTY を起動して測る**。
//!
//! tako の描画経路は入口から出口まで UTF-8 専用なのに、Windows の疑似コンソールは
//! OEM コードページ（日本語版 Windows なら CP932）で始まる。放っておくと子が吐いた
//! UTF-8 バイトを conhost が CP932 として解釈し、**tako が受け取る前に**文字が壊れる。
//! `platform::console` がペイン起動時に UTF-8 へ固定しているのを、ここで実測で固定する。
//!
//! Windows 以外ではスキップする（`platform::console` は unix で恒等）。
//!
//! ## 実行上の注意
//!
//! 固定は「tako 自身がコンソールを持っていないとき」だけ働く（出荷構成 = GUI
//! サブシステム）。テストプロセスはコンソールを持つので、各テストの冒頭で
//! `FreeConsole` して出荷構成に合わせる。`cargo test` は標準出力をパイプで
//! 受け取るため、コンソールを手放しても panic メッセージは失われない
//! （テストバイナリを端末で直に叩くと出力は消える）。

#![cfg(windows)]

use std::time::{Duration, Instant};

use tako_core::platform::console::{force_pin_pane_to_utf8, PinOutcome};
use tako_core::terminal::{SessionEvent, SpawnCommand, SpawnOptions, TerminalSession};

/// 化けの判定に使う文字列。CP932 にも UTF-8 にも存在する常用漢字を選ぶ
/// （どちらの符号でも表現できる = 化けたら符号の取り違えが原因だと確定できる）
const JP: &str = "日本語テスト";

/// UTF-8 が CP932 として解釈されたときに出る既知の並び。
/// 「化けていないこと」だけでなく「**この化け方をしていないこと**」も見る
const MOJIBAKE: &str = "譌･譛ｬ隱";

#[link(name = "kernel32")]
unsafe extern "system" {
    fn FreeConsole() -> i32;
}

/// 出荷構成（コンソールを持たない GUI プロセス）に合わせる。プロセス単位の状態なので一度だけ
fn detach_own_console() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        FreeConsole();
    });
}

struct Pane {
    session: TerminalSession,
    rx: futures::channel::mpsc::UnboundedReceiver<SessionEvent>,
}

impl Pane {
    /// `cmd.exe` を 1 ペイン分の ConPTY 上で起動する。
    /// `/d` で AutoRun を無効化し、`prompt` を固定してユーザー環境の影響を排除する
    fn new_cmd() -> Self {
        Self::spawn(SpawnCommand {
            program: "cmd.exe".into(),
            args: vec!["/d".into(), "/q".into(), "/k".into(), "prompt $G".into()],
        })
    }

    fn spawn(command: SpawnCommand) -> Self {
        let (session, rx) = TerminalSession::spawn(
            120,
            40,
            SpawnOptions {
                command: Some(command),
                cwd: Some(std::env::temp_dir()),
                env: Vec::new(),
            },
        )
        .expect("ConPTY を起動できること");
        Self { session, rx }
    }

    fn send_line(&self, line: &str) {
        self.session.write(format!("{line}\r\n").into_bytes());
    }

    /// `needle` を含む行が現れるまで待つ。出なければ false
    fn wait_for(&mut self, needle: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            while let Ok(ev) = self.rx.try_recv() {
                let _ = self.session.process_event(ev);
            }
            if self.screen_text().contains(needle) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn screen_text(&self) -> String {
        self.session.visible_lines().join("\n")
    }

    fn wait_prompt(&mut self) {
        assert!(
            self.wait_for(">", Duration::from_secs(15)),
            "プロンプトが出ない: {}",
            self.screen_text()
        );
    }
}

/// UTF-8 バイトを「バイトのまま」コンソールへ吐かせるためのファイル。
/// `type` は読んだバイトを `WriteFile` で流すので、解釈は出力コードページ任せになる
fn write_utf8_fixture(tag: &str, body: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("tako-enc-{}-{tag}.txt", std::process::id()));
    std::fs::write(&path, format!("{body}\r\n").as_bytes()).expect("fixture を書けること");
    path
}

/// ペイン起動時にコードページが UTF-8 へ固定されていること。
///
/// **修正前はここが 932 になる**（日本語版 Windows の OEM コードページ）
#[test]
fn ペイン起動時にコードページがutf8へ固定される() {
    detach_own_console();
    let mut pane = Pane::new_cmd();
    pane.wait_prompt();

    pane.send_line("chcp");
    assert!(
        pane.wait_for("65001", Duration::from_secs(15)),
        "コードページが UTF-8 に固定されていない: {}",
        pane.screen_text()
    );
}

/// ユーザーが報告した症状そのもの: UTF-8 のテキストが CP932 として解釈されて化ける
#[test]
fn utf8バイトを吐く子の出力が化けずに描画される() {
    detach_own_console();
    let fixture = write_utf8_fixture("render", JP);
    let mut pane = Pane::new_cmd();
    pane.wait_prompt();

    pane.send_line(&format!("type {}", fixture.display()));
    assert!(
        pane.wait_for(JP, Duration::from_secs(15)),
        "UTF-8 の日本語が正しく描画されない: {}",
        pane.screen_text()
    );
    assert!(
        !pane.screen_text().contains(MOJIBAKE),
        "CP932 誤解釈の化け方が残っている: {}",
        pane.screen_text()
    );
    let _ = std::fs::remove_file(&fixture);
}

/// 日本語ファイル名・絵文字（サロゲートペア）・長文（複数回の読み取りに跨がる分割復号）
#[test]
fn 日本語ファイル名と絵文字と長文でも化けない() {
    detach_own_console();
    // 長文は 1 回の PTY 読み取りに収まらない量にして、マルチバイト境界での
    // 分割復号（vte 側の責務）が壊れていないことも同時に見る
    let long = JP.repeat(600);
    let body = format!("{JP}\u{1F419}\n{long}");
    let path = std::env::temp_dir().join(format!(
        "tako-enc-{}-日本語ファイル.txt",
        std::process::id()
    ));
    std::fs::write(&path, format!("{body}\r\n").as_bytes()).expect("fixture を書けること");

    let mut pane = Pane::new_cmd();
    pane.wait_prompt();
    pane.send_line(&format!("type {}", path.display()));

    // 末尾（= 分割を何度も跨いだ後）まで化けずに届いていること
    assert!(
        pane.wait_for(&JP.repeat(3), Duration::from_secs(20)),
        "長文の日本語が化けている: {}",
        pane.screen_text()
    );
    assert!(
        !pane.screen_text().contains(MOJIBAKE),
        "CP932 誤解釈の化け方が残っている: {}",
        pane.screen_text()
    );
    let _ = std::fs::remove_file(&path);
}

/// 途中でユーザーが `chcp 932` に切り替えたら化ける（= 直せない領域）が、
/// 固定をやり直せば戻る。**固定の仕組みが冪等で、後からでも効く**ことの確認
#[test]
fn 途中でcp932へ切り替えても固定をやり直せば戻る() {
    detach_own_console();
    let fixture = write_utf8_fixture("switch", JP);
    let mut pane = Pane::new_cmd();
    pane.wait_prompt();

    // ユーザー（やシェルのプロファイル）がコードページを変えた状態を作る
    pane.send_line("chcp 932");
    assert!(
        pane.wait_for("932", Duration::from_secs(15)),
        "chcp 932 が効いていない: {}",
        pane.screen_text()
    );
    pane.send_line(&format!("type {}", fixture.display()));
    assert!(
        pane.wait_for(MOJIBAKE, Duration::from_secs(15)),
        "CP932 に落とした後は化けるはず（前提が崩れている）: {}",
        pane.screen_text()
    );

    // 固定をやり直せば以後は直る
    let pid = pane.session.child_pid().expect("child_pid が取れること");
    assert_eq!(force_pin_pane_to_utf8(pid), PinOutcome::Pinned);
    pane.send_line("cls");
    pane.send_line(&format!("type {}", fixture.display()));
    assert!(
        pane.wait_for(JP, Duration::from_secs(15)),
        "固定し直しても直らない: {}",
        pane.screen_text()
    );
    let _ = std::fs::remove_file(&fixture);
}

/// 既定シェル（tako が実際に起動するもの）でも固定が効いていること。
/// pwsh 7 は自分で UTF-8 にするが、Windows 同梱の PowerShell 5.1 はしない
/// （実測。#655 の根本原因）ので、そちらで確かめる
#[test]
fn powershell51のペインでもコードページがutf8になる() {
    detach_own_console();
    let program = "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe";
    if !std::path::Path::new(program).exists() {
        eprintln!("PowerShell 5.1 が無いのでスキップ");
        return;
    }
    let mut pane = Pane::spawn(SpawnCommand {
        program: program.into(),
        args: vec!["-NoLogo".into(), "-NoProfile".into()],
    });
    assert!(
        pane.wait_for(">", Duration::from_secs(20)),
        "プロンプトが出ない: {}",
        pane.screen_text()
    );

    pane.send_line("chcp.com");
    assert!(
        pane.wait_for("65001", Duration::from_secs(20)),
        "PowerShell 5.1 ペインが UTF-8 に固定されていない: {}",
        pane.screen_text()
    );
}
