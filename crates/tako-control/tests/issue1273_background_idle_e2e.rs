//! 背景作業が残る入力待ちの判定 E2E（Issue #1273 / #289 の再発）
//!
//! #1273 の本番は「worker が最終報告を出して入力欄 `❯` が空・けれど Bash の背景シェルや
//! Monitor が生き残っていて `claude agents --json` が busy を返し続ける」状態だった。
//! 単体テストは文字列の fixture を固定するが、**実際の端末を通した画面**
//! （tmux の capture・空白詰め・幅で切られる行）でも同じ判定になることは
//! 実物でしか確かめられない。
//!
//! ここでは #1259 と同じ作法の模擬 TUI（実 tmux・GUI 不要）で、
//! **本物の背景の子（`sleep 3600 &`）を抱えたまま**画面を 2 つの状態に切り替えて測る:
//!
//! - `i` = ターン終了（`✻ Crunched for … · done … · 1 shell still running` + 空の `❯`）
//!   → 入力待ちと判定できる
//! - `g` = 生成中（スピナー + 同じ背景作業の申告 + 空の `❯`）
//!   → 判定しない（誤発火なし）
//!
//! 本物の tmux が無い環境（psmux しか無い Windows 等）ではスキップする。

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use tako_control::orchestrator::wait;
use tako_core::agent_support::Agent;

#[path = "common/tmux_e2e.rs"]
mod tmux_e2e;

/// 本番のバックエンドや**他プロセスのテスト**と混ざらない専用の器（#1300）。
/// 固定名だと `cargo test --workspace` が 2 本走った瞬間に同名セッションを
/// 取り合って `duplicate session` で落ちる（実測は `tmux_e2e` のモジュール doc）
fn socket() -> &'static str {
    tmux_e2e::socket_for("1273")
}

fn real_tmux() -> bool {
    Command::new("tmux")
        .arg("-V")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| tako_core::tmux::announces_only_tmux(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or(false)
}

struct EmuGuard {
    session: String,
    dir: PathBuf,
}

impl Drop for EmuGuard {
    fn drop(&mut self) {
        // 器はこのプロセス専用。セッションを畳み、**このプロセスの最後の 1 本**なら
        // サーバーごと退役させてソケットファイルまで消す（tmux は残す）
        tmux_e2e::release_session(socket(), &self.session);
        // 背景の子（sleep 3600）はセッションごと落ちるが、取りこぼしても
        // **自分が記録した pid だけ**を確実に片付ける
        if let Ok(pid) = std::fs::read_to_string(self.dir.join("child.pid")) {
            let _ = Command::new("kill").arg(pid.trim()).output();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// #1273 の実採取画面と同じ形を描く模擬 claude TUI。
///
/// - **背景の子**（`sleep 3600 &`）を抱えたまま入力欄で待つ
/// - `i` / `g` で「ターン終了」「生成中」を切り替える（同じ背景作業の申告を出したまま）
/// - `d` で人間の下書きを入力欄へ置く
const EMULATOR: &str = r#"#!/bin/bash
# tako の #1273 判定の実検証用。実 claude の画面の形だけを真似る
PIDFILE="$1"
sleep 3600 &
echo $! > "$PIDFILE"
rule="────────────────────────────────────────────────────────────────────────"
mode="idle"
buf=""
draw() {
  printf '\033[2J\033[H'
  echo "  2. Evidence per acceptance criterion"
  echo
  echo "  #0001 - the fixture returned two options and highlighted=0."
  echo
  echo "  4. commit / PR"
  echo
  echo "  - abc1234 [fix] something (#0001)"
  echo
  case "$mode" in
    idle) echo "✻ Crunched for 1h 25m 10s · done 10:47 AM · 1 shell still running" ;;
    busy) echo "✻ Misting… (3m 27s · ↓ 8.4k tokens)" ;;
  esac
  echo
  echo "$rule"
  echo "❯ $buf"
  echo "$rule"
  echo "  [model placeholder]  worker: placeholder task"
  echo "  ctx  49% ████░░░░░░"
  echo "  5h   26% ██░░░░░░░░"
  echo "  7d   86% ████████░░"
  echo "  ⏵⏵ auto mode on · 1 shell · ⏎ for agents"
}
stty -echo -icanon -icrnl min 1 time 0 2>/dev/null
draw
while :; do
  IFS= read -rsn1 c || break
  case "$c" in
    i) mode="idle"; buf=""; draw ;;
    g) mode="busy"; buf=""; draw ;;
    d) mode="idle"; buf="draft by human"; draw ;;
  esac
done
stty sane 2>/dev/null
exec sleep 120
"#;

fn launch_emulator(tag: &str) -> EmuGuard {
    let dir = PathBuf::from(format!(
        "/private/tmp/tako-e2e-1273-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("作業ディレクトリを作れる");
    let script = dir.join("bg-idle-emu.sh");
    std::fs::write(&script, EMULATOR).expect("模擬 TUI を書ける");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(&script)
            .expect("stat できる")
            .permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&script, perm).expect("実行権を付けられる");
    }
    let session = format!("tako1273{tag}");
    // 起動より**先に**ガードを作る。起動が落ちたときも作業ディレクトリと器が残らない
    let guard = EmuGuard {
        session: session.clone(),
        dir: dir.clone(),
    };
    if let Err(diag) = tmux_e2e::new_session(
        socket(),
        &[
            "-d",
            "-s",
            &session,
            "-x",
            "100",
            "-y",
            "30",
            "-c",
            dir.to_str().expect("テストパスは UTF-8"),
            &format!(
                "{} {}",
                script.to_str().expect("UTF-8"),
                dir.join("child.pid").to_str().expect("UTF-8"),
            ),
        ],
    ) {
        panic!("{diag}");
    }
    let drawn = wait_until(Duration::from_secs(15), || {
        capture(&session).contains("still running")
    });
    assert!(drawn, "模擬 TUI が描かれない:\n{}", capture(&session));
    guard
}

fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

fn capture(session: &str) -> String {
    tako_core::tmux::capture_session(Some(socket()), session)
        .map(|l| l.join("\n"))
        .unwrap_or_else(|e| format!("<capture 失敗: {e}>"))
}

/// 背景の子が本当に生きているか（`sleep 3600` の pid を kill -0 で確かめる）
fn child_alive(dir: &std::path::Path) -> bool {
    let Ok(pid) = std::fs::read_to_string(dir.join("child.pid")) else {
        return false;
    };
    Command::new("kill")
        .args(["-0", pid.trim()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn send(session: &str, key: &str) {
    if let Err(diag) = tmux_e2e::send_keys(socket(), &["-t", session, key]) {
        panic!("{diag}");
    }
}

/// 受け入れ条件 1 + 3: 実端末の画面で「ターン終了 = 入力待ち」「生成中 = 判定しない」
#[test]
fn 実端末の画面で背景作業つき入力待ちを見分ける() {
    if !real_tmux() {
        eprintln!("[e2e-1273] 本物の tmux が無いのでスキップ");
        return;
    }
    let guard = launch_emulator("detect");
    let session = guard.session.clone();
    let dir = guard.dir.clone();

    // ① 前提: 背景の子が生きている（= #1273 の再現条件）
    assert!(
        child_alive(&dir),
        "背景の子（sleep 3600）が生きていない = 再現条件を満たしていない"
    );

    // ② ターン終了の画面 → 入力待ちと判定できる
    let screen = capture(&session);
    assert_eq!(
        wait::background_work_summary(&screen).as_deref(),
        Some("1 shell"),
        "実端末の画面から背景作業の内訳を読めない:\n{screen}"
    );
    assert_eq!(
        wait::input_waiting_with_background_work(&screen, false, Some(Agent::Claude)).as_deref(),
        Some("1 shell"),
        "実端末の入力待ちを判定できない:\n{screen}"
    );
    // watch の再検査（`screen_looks_busy`）とも一致すること
    assert!(!wait::screen_looks_busy(&screen), "実端末の画面:\n{screen}");

    // ③ 生成中へ切り替える → 同じ背景作業の申告があっても判定しない
    send(&session, "g");
    assert!(
        wait_until(Duration::from_secs(10), || capture(&session)
            .contains("Misting…")),
        "生成中の画面へ切り替わらない:\n{}",
        capture(&session)
    );
    let busy_screen = capture(&session);
    assert!(
        wait::screen_looks_busy(&busy_screen),
        "生成中の画面を busy と読めていない:\n{busy_screen}"
    );
    assert_eq!(
        wait::input_waiting_with_background_work(&busy_screen, false, Some(Agent::Claude)),
        None,
        "生成中の画面を入力待ちと誤読している:\n{busy_screen}"
    );

    // ④ 人間の下書きがある入力欄も判定しない（踏み潰さない）
    send(&session, "d");
    assert!(
        wait_until(Duration::from_secs(10), || capture(&session)
            .contains("draft by human")),
        "下書きが画面に出ない:\n{}",
        capture(&session)
    );
    let draft_screen = capture(&session);
    assert_eq!(
        wait::input_waiting_with_background_work(&draft_screen, false, Some(Agent::Claude)),
        None,
        "下書きのある入力欄を「空の入力待ち」と読んでいる:\n{draft_screen}"
    );

    // ⑤ 戻せばまた判定できる（状態を持ち越していない）
    send(&session, "i");
    assert!(
        wait_until(Duration::from_secs(10), || {
            let s = capture(&session);
            wait::input_waiting_with_background_work(&s, false, Some(Agent::Claude)).is_some()
        }),
        "ターン終了へ戻しても判定できない:\n{}",
        capture(&session)
    );

    // ⑥ 判定のあいだ背景の子はずっと生きたまま（判定が子に触っていない）
    assert!(child_alive(&dir), "背景の子が判定の途中で消えている");
}

/// エッジ: 折りたたみ画面（`collapsed`）では実端末の画面でも判定しない
#[test]
fn 折りたたみ画面では実端末でも判定しない() {
    if !real_tmux() {
        eprintln!("[e2e-1273] 本物の tmux が無いのでスキップ");
        return;
    }
    let guard = launch_emulator("collapsed");
    let screen = capture(&guard.session);
    assert!(
        wait::input_waiting_with_background_work(&screen, false, Some(Agent::Claude)).is_some(),
        "前提（折りたたみでなければ判定できる）が崩れている:\n{screen}"
    );
    assert_eq!(
        wait::input_waiting_with_background_work(&screen, true, Some(Agent::Claude)),
        None,
        "折りたたみ中に画面を根拠にしている"
    );
}
