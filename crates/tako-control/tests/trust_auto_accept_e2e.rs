//! 信頼ダイアログの自動承諾の実キー E2E（Issue #1236）
//!
//! 実 claude の代わりに**模擬 TUI**（claude 2.x の番号なし信頼ダイアログを描く bash
//! スクリプト）を実 tmux で走らせ、`claude_tui::deliver_via_tmux` に自動承諾させる。
//! 模擬 TUI は受け取ったキーと確定した選択肢をファイルへ記録するので、
//! **「実際に送られたキー列」と「実際に確定されたラベル」**を機械検証できる
//! （GUI も Anthropic API も要らない）。
//!
//! A/B は同一バイナリで取る:
//! - 新: `Down` → `Enter` で `Yes, I trust this folder` が確定する
//! - 旧（`TAKO_1236_LEGACY=1`）: 素の `Enter` で **`No, exit` が確定する** = #1236 の事故
//!
//! 本物の tmux が無い環境（psmux しか無い Windows 等）ではスキップする。
//! **env を触るので、このファイルには 1 テストだけ置く**（テストバイナリ内の並列実行で
//! A/B のフラグが混ざらないようにするため）。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use tako_control::claude_tui;

/// 本番のバックエンドや他の実験と混ざらない専用ソケット
const SOCKET: &str = "tako-e2e-1236";

/// 模擬 TUI が描く選択肢（claude 2.x の実採取と同じ並び。既定は拒否側）
const DECLINE: &str = "No, exit";
const ACCEPT: &str = "Yes, I trust this folder";

fn real_tmux() -> bool {
    Command::new("tmux")
        .arg("-V")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| tako_core::tmux::announces_only_tmux(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or(false)
}

/// tmux セッションと一時ディレクトリを片付けるガード
struct EmuGuard {
    session: String,
    dir: PathBuf,
}

impl Drop for EmuGuard {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", SOCKET, "kill-session", "-t", &self.session])
            .output();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// claude 2.x の番号なし信頼ダイアログを描く模擬 TUI。
///
/// - 既定のハイライトは**拒否側**（`No, exit`）= #1236 が起きる形
/// - `↑` / `↓` でハイライトを動かし、`Enter` で確定する
/// - 受け取ったキーを `KEY=…`、確定したラベルを `CHOSE=…` としてログへ追記する
/// - 承諾側を確定したときだけ claude 相当の空の入力欄（`❯`）を描く
///   （拒否側は claude が終了するので入力欄は出ない）
const EMULATOR: &str = r#"#!/bin/bash
# tako の自動承諾（#1236）の A/B 用。実 claude の信頼ダイアログの形だけを真似る
LOG="$1"
: > "$LOG"
sel=0
labels=("No, exit" "Yes, I trust this folder")
draw() {
  printf '\033[2J\033[H'
  echo " Quick safety check: Is this a project you created or one you trust?"
  echo
  for i in 0 1; do
    if [ "$i" = "$sel" ]; then echo " > ${labels[$i]}"; else echo "   ${labels[$i]}"; fi
  done
  echo
  echo " Enter to confirm · Esc to cancel"
}
# 出力処理（onlcr）は残したまま 1 文字ずつ読む（raw にすると改行が階段状になる）。
# icrnl を切るのが要点: 付いたままだと Enter（CR）が LF へ変換され、bash 3.2 の
# `read -n1` が「区切り文字」として捨てるので Enter を受け取れない（実測）
stty -echo -icanon -icrnl min 1 time 0 2>/dev/null
draw
while :; do
  IFS= read -rsn1 c || break
  if [ "$c" = $'\033' ]; then
    IFS= read -rsn2 -t 1 rest
    case "$rest" in
      '[A') echo "KEY=Up" >> "$LOG"; if [ "$sel" -gt 0 ]; then sel=$((sel - 1)); fi; draw ;;
      '[B') echo "KEY=Down" >> "$LOG"; if [ "$sel" -lt 1 ]; then sel=$((sel + 1)); fi; draw ;;
    esac
  elif [ -z "$c" ] || [ "$c" = $'\r' ] || [ "$c" = $'\n' ]; then
    echo "KEY=Enter" >> "$LOG"
    echo "CHOSE=${labels[$sel]}" >> "$LOG"
    break
  fi
done
stty sane 2>/dev/null
printf '\033[2J\033[H'
echo "emulator done"
if [ "$sel" = 1 ]; then
  echo "--------------------------------"
  echo "❯ "
  echo "--------------------------------"
fi
exec sleep 120
"#;

/// 模擬 TUI を tmux セッションで起動し、ログのパスを返す
fn launch_emulator(tag: &str) -> (EmuGuard, PathBuf) {
    let dir = PathBuf::from(format!(
        "/private/tmp/tako-e2e-1236-{tag}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("作業ディレクトリを作れる");
    let script = dir.join("trust-emu.sh");
    std::fs::write(&script, EMULATOR).expect("模擬 TUI を書ける");
    let mut perm = std::fs::metadata(&script)
        .expect("stat できる")
        .permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perm.set_mode(0o755);
    }
    std::fs::set_permissions(&script, perm).expect("実行権を付けられる");

    let log = dir.join("keys.log");
    let session = format!("tako1236{tag}");
    let status = Command::new("tmux")
        .args([
            "-L",
            SOCKET,
            "new-session",
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
                log.to_str().expect("UTF-8")
            ),
        ])
        .status()
        .expect("tmux を実行できる");
    assert!(status.success(), "tmux new-session が失敗した");
    let guard = EmuGuard {
        session: session.clone(),
        dir,
    };
    // ダイアログが描かれるまで待つ（描画前に送ると自動承諾の材料が無い）
    let drawn = wait_until(Duration::from_secs(10), || {
        tako_core::tmux::capture_session(Some(SOCKET), &session)
            .map(|l| claude_tui::is_trust_dialog(&l) && claude_tui::is_choice_dialog(&l))
            .unwrap_or(false)
    });
    assert!(
        drawn,
        "模擬 TUI の信頼ダイアログが描かれない:\n{}",
        dump(&session)
    );
    (guard, log)
}

fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

fn dump(session: &str) -> String {
    tako_core::tmux::capture_session(Some(SOCKET), session)
        .map(|l| l.join("\n"))
        .unwrap_or_else(|e| format!("<capture 失敗: {e}>"))
}

/// ログから「送られたキー列」と「確定されたラベル」を読む
fn read_log(log: &Path) -> (Vec<String>, Option<String>) {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let keys = text
        .lines()
        .filter_map(|l| l.strip_prefix("KEY=").map(str::to_string))
        .collect();
    let chose = text
        .lines()
        .find_map(|l| l.strip_prefix("CHOSE=").map(str::to_string));
    (keys, chose)
}

/// 自動承諾を 1 回走らせ、模擬 TUI が記録したキー列と確定ラベルを返す
fn run_auto_accept(tag: &str) -> (Vec<String>, Option<String>) {
    let (guard, log) = launch_emulator(tag);
    // wait_ready=false: 模擬 TUI は claude ではないので入力欄待ちに長く付き合わせない。
    // 自動承諾（信頼ダイアログの処理）は wait_ready に関係なく行われる
    let report = claude_tui::deliver_via_tmux(Some(SOCKET), &guard.session, "PROBE1236", false);
    let confirmed = wait_until(Duration::from_secs(5), || read_log(&log).1.is_some());
    let (keys, chose) = read_log(&log);
    eprintln!(
        "[{tag}] deliver={:?} keys={:?} chose={:?} confirmed={confirmed}\n--- 画面 ---\n{}",
        report.as_ref().map(|r| r.trust_dialogs_accepted),
        keys,
        chose,
        dump(&guard.session)
    );
    (keys, chose)
}

#[test]
fn issue1236_自動承諾は承諾側を確定する() {
    if !real_tmux() {
        eprintln!("skip: 本物の tmux が無い環境");
        return;
    }

    // --- 新（#1236 の修正後）: ハイライトを読み、承諾側へ動かしてから確定する ---
    let (keys, chose) = run_auto_accept("new");
    assert_eq!(
        keys,
        vec!["Down".to_string(), "Enter".to_string()],
        "承諾側へ ↓ で動かしてから Enter を送ること（素の Enter は拒否側を確定する）"
    );
    assert_eq!(
        chose.as_deref(),
        Some(ACCEPT),
        "確定されたのは承諾側の選択肢であること"
    );

    // --- 旧（`TAKO_1236_LEGACY=1`）: 素の Enter で拒否側が確定する = #1236 の事故 ---
    std::env::set_var("TAKO_1236_LEGACY", "1");
    let (legacy_keys, legacy_chose) = run_auto_accept("legacy");
    std::env::remove_var("TAKO_1236_LEGACY");
    assert_eq!(
        legacy_keys,
        vec!["Enter".to_string()],
        "旧挙動はハイライトを見ずに Enter を送る"
    );
    assert_eq!(
        legacy_chose.as_deref(),
        Some(DECLINE),
        "旧挙動では `No, exit` が確定する（claude が終了し worker が黙って死ぬ）"
    );
}
