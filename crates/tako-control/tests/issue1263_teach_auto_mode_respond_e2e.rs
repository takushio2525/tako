//! auto mode の環境学習の確認へ respond する実キー E2E（Issue #1263）
//!
//! 実 claude の代わりに**模擬 TUI**（`Teach auto mode about your environment?` の
//! 番号つき 3 択 + **その下の空の入力欄**を描く bash スクリプト）を実 tmux で走らせ、
//! `dispatch::respond_via` に応答させる。模擬 TUI は受け取ったキーと確定した選択肢を
//! ファイルへ記録するので、**「実際に送られたキー列」と「実際に確定されたラベル」**を
//! 機械検証できる（GUI も Anthropic API も要らない）。
//!
//! 検証するのは #1263 の受け入れ条件 2:
//! - `--choice "Don't show again"`（ラベル）で `resolved: true`
//! - `--choice 3`（番号）で `resolved: true`
//! - 送られたキーは**番号キー 1 つだけ**（番号つきなので矢印も Enter も使わない）
//!
//! 本物の tmux が無い環境（psmux しか無い Windows 等）ではスキップする。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use tako_control::reach::DialogAccess;

/// 本番のバックエンドや他の実験と混ざらない専用ソケット
const SOCKET: &str = "tako-e2e-1263";

/// 模擬 TUI が描く選択肢（Issue #1263 本文の実採取と同じ並び）
const LABELS: [&str; 3] = ["Yes", "Not now", "Don't show again"];

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

/// テストから tmux セッションへ直に届く `DialogAccess`
/// （in-process / detached と同じ 2 手だけを実装する）
struct TmuxAccess {
    session: String,
}

impl DialogAccess for TmuxAccess {
    fn capture(&self) -> Result<Vec<String>, String> {
        tako_core::tmux::capture_session(Some(SOCKET), &self.session)
    }

    fn send_key(&self, key: &str) -> Result<(), String> {
        tako_core::tmux::send_key(Some(SOCKET), &self.session, key)
    }

    fn route(&self) -> &'static str {
        "test-tmux"
    }
}

/// claude の auto mode 環境学習の確認を描く模擬 TUI。
///
/// - 番号つき 3 択で、**選択肢の下に空の入力欄**を描く（= #1263 が起きる形）
/// - 番号キー（`1` / `2` / `3`）で確定する（実 claude の番号つきダイアログと同じ）
/// - 受け取ったキーを `KEY=…`、確定したラベルを `CHOSE=…` としてログへ追記する
/// - 確定後はダイアログを消して入力欄だけを描く（= respond の解消検証が効く形）
const EMULATOR: &str = r#"#!/bin/bash
# tako の respond（#1263）の実キー検証用。実 claude の画面の形だけを真似る
LOG="$1"
: > "$LOG"
sel=0
labels=("Yes" "Not now" "Don't show again")
rule="────────────────────────────────────────────────────────────────────────"
draw() {
  printf '\033[2J\033[H'
  echo "  Teach auto mode about your environment?"
  echo
  echo "  Auto mode works better when it knows your environment. Takes about a minute."
  echo
  for i in 0 1 2; do
    if [ "$i" = "$sel" ]; then
      echo "  ❯ $((i + 1)). ${labels[$i]}"
    else
      echo "    $((i + 1)). ${labels[$i]}"
    fi
  done
  echo
  echo "  Enter to confirm · Esc to cancel"
  echo "$rule"
  echo "❯"
  echo "$rule"
  echo "  claude-opus-5 · ctx 12%"
}
# 出力処理（onlcr）は残したまま 1 文字ずつ読む。icrnl を切るのが要点（#1236 の実測）
stty -echo -icanon -icrnl min 1 time 0 2>/dev/null
draw
chosen=""
while :; do
  IFS= read -rsn1 c || break
  case "$c" in
    1|2|3)
      echo "KEY=$c" >> "$LOG"
      chosen="${labels[$((c - 1))]}"
      echo "CHOSE=$chosen" >> "$LOG"
      break
      ;;
    $'\033')
      IFS= read -rsn2 -t 1 rest
      case "$rest" in
        '[A') echo "KEY=Up" >> "$LOG"; if [ "$sel" -gt 0 ]; then sel=$((sel - 1)); fi; draw ;;
        '[B') echo "KEY=Down" >> "$LOG"; if [ "$sel" -lt 2 ]; then sel=$((sel + 1)); fi; draw ;;
      esac
      ;;
    ''|$'\r'|$'\n')
      echo "KEY=Enter" >> "$LOG"
      chosen="${labels[$sel]}"
      echo "CHOSE=$chosen" >> "$LOG"
      break
      ;;
  esac
done
stty sane 2>/dev/null
# 確定後はダイアログを消す（入力欄だけが残る = respond の解消検証が真になる形）
printf '\033[2J\033[H'
echo "  auto mode setup finished"
echo "$rule"
echo "❯"
echo "$rule"
echo "  claude-opus-5 · ctx 12%"
exec sleep 120
"#;

/// 模擬 TUI を tmux セッションで起動し、ガードとログのパスを返す
fn launch_emulator(tag: &str) -> (EmuGuard, PathBuf) {
    let dir = PathBuf::from(format!(
        "/private/tmp/tako-e2e-1263-{tag}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("作業ディレクトリを作れる");
    let script = dir.join("teach-emu.sh");
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
    let session = format!("tako1263{tag}");
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
    // ダイアログが検知されるまで待つ（描画前に応答させると材料が無い）
    let drawn = wait_until(Duration::from_secs(10), || {
        tako_core::tmux::capture_session(Some(SOCKET), &session)
            .map(|l| tako_control::claude_tui::is_choice_dialog(&l))
            .unwrap_or(false)
    });
    assert!(
        drawn,
        "模擬 TUI のダイアログが検知されない（#1263 の検知が効いていない）:\n{}",
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

/// `choice` で 1 回応答し、respond の応答・送られたキー列・確定ラベルを返す
fn respond_once(tag: &str, choice: &str) -> (serde_json::Value, Vec<String>, Option<String>) {
    let (guard, log) = launch_emulator(tag);
    let access = TmuxAccess {
        session: guard.session.clone(),
    };
    let result = tako_control::dispatch::respond_via(&access, 1263, Some(choice), Some("test"))
        .unwrap_or_else(|e| {
            panic!(
                "[{tag}] respond が失敗した: {e:?}\n{}",
                dump(&guard.session)
            )
        });
    let confirmed = wait_until(Duration::from_secs(5), || read_log(&log).1.is_some());
    let (keys, chose) = read_log(&log);
    eprintln!(
        "[{tag}] choice={choice:?} result={result} keys={keys:?} chose={chose:?} \
         confirmed={confirmed}\n--- 画面 ---\n{}",
        dump(&guard.session)
    );
    (result, keys, chose)
}

#[test]
fn issue1263_auto_mode確認へラベルと番号で応答できる() {
    if !real_tmux() {
        eprintln!("skip: 本物の tmux が無い環境");
        return;
    }

    // --- 下見（choice 省略）: 送信せず構造だけ返す ---
    let (guard, log) = launch_emulator("peek");
    let access = TmuxAccess {
        session: guard.session.clone(),
    };
    let peek = tako_control::dispatch::respond_via(&access, 1263, None, Some("test"))
        .expect("下見が成功する");
    eprintln!("[peek] {peek}");
    assert_eq!(peek["kind"], "select", "種別は select（#1263）");
    assert_eq!(peek["numbered"], true, "番号キーで確定できる");
    assert_eq!(peek["responded"], false, "下見はキーを送らない");
    assert_eq!(peek["options"].as_array().expect("options").len(), 3);
    assert!(
        read_log(&log).0.is_empty(),
        "下見でキーが送られてはいけない: {:?}",
        read_log(&log).0
    );
    drop(guard);

    // --- ラベル指定（`Don't show again`）---
    let (by_label, keys, chose) = respond_once("label", "Don't show again");
    assert_eq!(by_label["resolved"], true, "ダイアログが解消される");
    assert_eq!(by_label["choice"], 3);
    assert_eq!(by_label["choice_text"], "Don't show again");
    assert_eq!(by_label["stray_input"], false, "入力欄へ漏れていない");
    assert_eq!(
        keys,
        vec!["3".to_string()],
        "番号つきなので番号キー 1 つだけを送る（矢印も Enter も使わない）"
    );
    assert_eq!(chose.as_deref(), Some(LABELS[2]));

    // --- 番号指定（`3`）---
    let (by_number, keys, chose) = respond_once("number", "3");
    assert_eq!(by_number["resolved"], true);
    assert_eq!(by_number["choice_text"], "Don't show again");
    assert_eq!(keys, vec!["3".to_string()]);
    assert_eq!(chose.as_deref(), Some(LABELS[2]));
}
