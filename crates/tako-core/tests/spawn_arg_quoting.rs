//! PTY へ渡す argv の回帰テスト（#884）。**実際に子プロセスを起こして測る**。
//!
//! Windows には argv という概念が無く、`CreateProcessW` へ渡すのは 1 本の
//! コマンドライン文字列なので、`alacritty_terminal` が `program` と `args` を
//! 空白で連結する。既定（`escape_args = false`）は**各語を素のまま**つなぐため、
//! 空白を含む語が子側の CRT パーサで複数語へ割れていた。
//! `platform::shell::apply_arg_escaping` がそれを閉じているのを、ここで実測で固定する。
//!
//! Windows 以外ではスキップする（unix は `execvp` へ argv がそのまま渡る）。
//! `escape_args` は `#[cfg(target_os = "windows")]` でフィールドごと消えるため、
//! **macOS のユニットテストからは分岐の中身を踏めない**。この網が唯一の実挙動の担保。

#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use tako_core::terminal::{SessionEvent, SpawnCommand, SpawnOptions, TerminalSession};

/// 1 ペイン分の PTY。画面に出た文字列で判定する
struct Pane {
    session: TerminalSession,
    rx: futures::channel::mpsc::UnboundedReceiver<SessionEvent>,
}

impl Pane {
    fn spawn(options: SpawnOptions) -> Self {
        let (session, rx) = TerminalSession::spawn(140, 40, options).expect("PTY を起動できること");
        Self { session, rx }
    }

    /// `needle` を含む画面になるまで待つ。出なければ false
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

    /// 画面イベントを `dur` のあいだ取り込むだけ（判定はしない）。
    /// PTY が死ぬと以降は無音になるので、器へ問い合わせる合間に回す
    fn pump(&mut self, dur: Duration) {
        let deadline = Instant::now() + dur;
        while Instant::now() < deadline {
            while let Ok(ev) = self.rx.try_recv() {
                let _ = self.session.process_event(ev);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn screen_text(&self) -> String {
        self.session.visible_lines().join("\n")
    }
}

/// 受け取った argv をそのまま報告する PowerShell スクリプトを、
/// **名前に空白を含むディレクトリ**へ置く（`-File <パス>` 側も同時に試すため）
fn write_argv_probe(tag: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("tako-884 probe {}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("空白入りディレクトリを作れること");
    let script = dir.join("argv probe.ps1");
    std::fs::write(
        &script,
        "Write-Output (\"ARGC=\" + $args.Count)\r\n\
         for ($i = 0; $i -lt $args.Count; $i++) { Write-Output (\"ARG$i=[\" + $args[$i] + \"]\") }\r\n\
         Write-Output \"PROBE-DONE\"\r\n",
    )
    .expect("プローブを書けること");
    (dir, script)
}

/// 空白を含む引数が **1 語のまま**子へ届くこと（#884 の核心）。
///
/// 修正前はコマンドラインが素の空白連結になるため、
/// `-File <空白入りパス>` と `a b c` の両方が割れて PowerShell が起動に失敗する
/// （`ARGC=1` にならない）
#[test]
fn 空白を含む引数が1語のまま子へ届く() {
    let (dir, script) = write_argv_probe("argc");
    let mut pane = Pane::spawn(SpawnOptions {
        command: Some(SpawnCommand {
            program: "powershell.exe".into(),
            args: vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-File".into(),
                script.display().to_string(),
                "a b c".into(),
            ],
        }),
        cwd: Some(std::env::temp_dir()),
        env: Vec::new(),
    });
    let done = pane.wait_for("PROBE-DONE", Duration::from_secs(30));
    let screen = pane.screen_text();
    // 子が握っているとディレクトリを消せないので、先に PTY を落とす
    drop(pane);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(done, "プローブが完走しない（画面）:\n{screen}");
    assert!(
        screen.contains("ARGC=1"),
        "空白入りの引数が割れている（ARGC=1 にならない）:\n{screen}"
    );
    assert!(
        screen.contains("ARG0=[a b c]"),
        "引数の中身が変わっている:\n{screen}"
    );
}

/// テスト用の psmux（無ければスキップ）。`tmux_backend::wrap_options` が使う
/// `tmux_bin()` と同じ解決を通したいので、ここでも `tmux -V` で確かめる
fn container_bin() -> Option<&'static str> {
    let bin = tako_core::tmux::tmux_bin();
    Command::new(bin)
        .arg("-V")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| bin)
}

fn container(bin: &str, socket: &str) -> Command {
    let mut cmd = Command::new(bin);
    cmd.args(["-L", socket]);
    cmd
}

/// 器あり（persist ON）でも、空白を含む cwd のペインが生き残ること（#884 の症状そのもの）。
///
/// 修正前は `-c C:\...\dir with space` が `-c C:\...\dir` + `with` + `space` へ割れ、
/// 余った語が `new-session` の **shell-command** として実行されるため
/// `with: The term 'with' is not recognized` でペインが即死していた
/// （tako の器設定は `remain-on-exit` が off なので画面には何も出ない）
#[test]
fn 器ありでも空白入りcwdのペインが生き残る() {
    let Some(bin) = container_bin() else {
        eprintln!("skip: 器（tmux / psmux）が無い");
        return;
    };
    let space_dir = std::env::temp_dir().join(format!("tako-884 cwd {}", std::process::id()));
    std::fs::create_dir_all(&space_dir).expect("空白入り cwd を作れること");
    let socket = format!("tako-884test-{}", std::process::id());
    let session = format!("tako-884-{}", std::process::id());

    let wrapped = tako_core::tmux_backend::wrap_options(
        SpawnOptions {
            command: None,
            cwd: Some(space_dir.clone()),
            env: vec![("TAKO_PANE_ID".into(), "1".into())],
        },
        &socket,
        &session,
    );
    let mut pane = Pane::spawn(wrapped);

    // 器が「そのセッションのペインがこの cwd で生きている」と答えるまで待ち、
    // **そのあと生き続ける**ことまで見る。
    //
    // 最初に見えた 1 回で合格にしてはいけない（この網を作る過程で実測）:
    // `-c` が割れて存在しないディレクトリになると psmux は**クライアントの cwd**へ
    // 落ちるが、`TerminalSession::spawn` は `working_directory`（`CreateProcessW` の
    // `lpCurrentDirectory`）にも同じ cwd を渡していて、そちらは引用の影響を受けない。
    // 結果、壊れていても +600ms までは「正しい cwd のペインが居る」ように見え、
    // 余った語が shell-command として失敗した +1200ms 頃に消える
    let want = space_dir.display().to_string();
    let alive_now = |seen: &mut String| {
        let Ok(out) = container(bin, &socket)
            .args([
                "list-panes",
                "-a",
                "-F",
                "#{session_name} #{pane_dead} #{pane_current_path}",
            ])
            .output()
        else {
            return false;
        };
        *seen = String::from_utf8_lossy(&out.stdout).to_string();
        // cwd に空白が入るので、書式は「セッション名 / 生死 / 残り全部が cwd」で切る
        seen.lines().any(|line| {
            let mut it = line.trim_end().splitn(3, ' ');
            matches!(
                (it.next(), it.next(), it.next()),
                (Some(name), Some("0"), Some(path)) if name == session && path == want
            )
        })
    };

    let mut seen = String::new();
    let appeared_by = Instant::now() + Duration::from_secs(30);
    let mut appeared = false;
    while Instant::now() < appeared_by {
        pane.pump(Duration::from_millis(300));
        if alive_now(&mut seen) {
            appeared = true;
            break;
        }
    }

    // 生き残り確認。壊れているときは 1 秒強で消えるので、その 4 倍を見張る
    let mut alive = appeared;
    if appeared {
        let watch_until = Instant::now() + Duration::from_secs(4);
        while Instant::now() < watch_until {
            pane.pump(Duration::from_millis(400));
            if !alive_now(&mut seen) {
                alive = false;
                break;
            }
        }
    }

    let screen = pane.screen_text();
    drop(pane);
    // 後始末。**`-L` を落とすと全ソケットのサーバーが死ぬ**（psmux 実測）
    let _ = container(bin, &socket).arg("kill-server").output();
    let _ = std::fs::remove_dir_all(&space_dir);

    assert!(appeared, "空白入り cwd のペインが器の中に現れない\n期待 cwd: {want}\n器の応答: {seen}\n画面:\n{screen}");
    assert!(
        alive,
        "空白入り cwd のペインが現れたあと消えた（#884 の症状そのもの）\n\
         期待 cwd: {want}\n器の応答: {seen}\n画面:\n{screen}"
    );
}
