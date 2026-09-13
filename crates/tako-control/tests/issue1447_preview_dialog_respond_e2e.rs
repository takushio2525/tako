//! preview パネルつき AskUserQuestion へ respond する実キー E2E（Issue #1447）
//!
//! 実 claude の代わりに**模擬 TUI**（選択肢の右へ枠つきの副画面を並べる
//! side-by-side layout を描く bash スクリプト）を実 tmux で走らせ、
//! `dispatch::respond_via` に応答させる。模擬 TUI は受け取ったキーと確定した
//! 選択肢をファイルへ記録するので、**実際に送られたキー列**と**実際に確定された
//! ラベル**を機械検証できる（GUI も Anthropic API も要らない）。
//!
//! 画面は Issue #1447 本文の実採取と同じ並び（幅 60 桁・個人情報なし）。
//! 修正前はここで `respond` が「ダイアログが画面に存在しない」で落ちていた。
//!
//! 本物の tmux が無い環境（psmux しか無い Windows 等）ではスキップする。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use tako_control::reach::DialogAccess;

#[path = "common/tmux_e2e.rs"]
mod tmux_e2e;

/// 本番のバックエンドや**他プロセスのテスト**と混ざらない専用の器（#1300）
fn socket() -> &'static str {
    tmux_e2e::socket_for("1447")
}

/// 模擬 TUI が描く選択肢（Issue #1447 本文の実採取と同じ並び）
const LABELS: [&str; 3] = [
    "権限を許可して私が流す（推奨）",
    "自分で流す",
    "AWS は後回し、PR だけ進める",
];

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
        tmux_e2e::release_session(socket(), &self.session);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

struct TmuxAccess {
    session: String,
}

impl DialogAccess for TmuxAccess {
    fn capture(&self) -> Result<Vec<String>, String> {
        tako_core::tmux::capture_session(Some(socket()), &self.session)
    }

    fn send_key(&self, key: &str) -> Result<(), String> {
        tako_core::tmux::send_key(Some(socket()), &self.session, key)
    }

    fn route(&self) -> &'static str {
        "test-tmux"
    }
}

/// AskUserQuestion の `preview` つき（2 カラム配置）を描く模擬 TUI。
///
/// 選択肢の行末に副画面の罫線が混ざり、長いラベルは**中身の桁より 1 桁浅い**
/// 4 桁へ折り返す（= #1447 が起きる形。実採取のとおり）
const EMULATOR: &str = r#"#!/bin/bash
# tako の respond（#1447）の実キー検証用。実 claude の画面の形だけを真似る
LOG="$1"
: > "$LOG"
sel=0
labels=("権限を許可して私が流す（推奨）" "自分で流す" "AWS は後回し、PR だけ進める")
rule="────────────────────────────────────────────────────────────"
mark() { if [ "$1" = "$sel" ]; then printf '❯'; else printf ' '; fi; }
draw() {
  printf '\033[2J\033[H'
  echo "  Made 1 scratchpad edit +149, pushed to feat/62-link-share,"
  echo "  created PR #66, ran 33 shell commands"
  echo
  echo "$rule"
  echo " ☐ AWS 適用"
  echo
  echo "│ #62 のコード・テスト・ドキュメントは完了して PR #66"
  echo "│ を出しました。残るのは AWS への適用（DynamoDB"
  echo "│ テーブル・IAM・Lambda 更新・CloudFront"
  echo "│ のビヘイビア）ですが、このセッションでは AWS"
  echo "│ の書き込み系コマンドがすべて自動モードの分類器にブロックさ"
  echo "│ れています。どう進めますか？"
  echo
  echo "$(mark 0) 1. 権限を許可して私が流す（    ┌────────────────────────┐"
  echo "    推奨）                       │ 許可が要る aws         │"
  echo "$(mark 1) 2. 自分で流す                  ├─── ✂ ─── 17 lines hidden"
  echo "$(mark 2) 3. AWS は後回し、PR            ┤"
  echo "    だけ進める                   └────────────────────────┘"
  echo
  echo "                                 Notes: press n to add notes"
  echo
  echo "$rule"
  echo "  Chat about this"
  echo
  echo "Enter to select · ↑/↓ to navigate · n to add notes · Esc to"
  echo "cancel"
}
# 出力処理（onlcr）は残したまま 1 文字ずつ読む。icrnl を切るのが要点（#1236 の実測）
stty -echo -icanon -icrnl min 1 time 0 2>/dev/null
draw
while :; do
  IFS= read -rsn1 c || break
  case "$c" in
    1|2|3)
      echo "KEY=$c" >> "$LOG"
      echo "CHOSE=${labels[$((c - 1))]}" >> "$LOG"
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
      echo "CHOSE=${labels[$sel]}" >> "$LOG"
      break
      ;;
  esac
done
stty sane 2>/dev/null
# 確定後はダイアログを消す（入力欄だけが残る = respond の解消検証が真になる形）
printf '\033[2J\033[H'
echo "  わかりました。進めます。"
echo "$rule"
echo "❯"
echo "$rule"
echo "  claude-opus-5 · ctx 12%"
exec sleep 120
"#;

fn launch_emulator(tag: &str) -> (EmuGuard, PathBuf) {
    let dir = PathBuf::from(format!(
        "/private/tmp/tako-e2e-1447-{tag}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("作業ディレクトリを作れる");
    let script = dir.join("preview-emu.sh");
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

    let log = dir.join("keys.log");
    let session = format!("tako1447{tag}");
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
            "70",
            "-y",
            "30",
            "-c",
            dir.to_str().expect("テストパスは UTF-8"),
            &format!(
                "{} {}",
                script.to_str().expect("UTF-8"),
                log.to_str().expect("UTF-8")
            ),
        ],
    ) {
        panic!("{diag}");
    }
    let drawn = wait_until(Duration::from_secs(10), || {
        tako_core::tmux::capture_session(Some(socket()), &session)
            .map(|l| tako_control::claude_tui::is_choice_dialog(&l))
            .unwrap_or(false)
    });
    assert!(
        drawn,
        "模擬 TUI のダイアログが検知されない（#1447 の不検知）:\n{}",
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
    tako_core::tmux::capture_session(Some(socket()), session)
        .map(|l| l.join("\n"))
        .unwrap_or_else(|e| format!("<capture 失敗: {e}>"))
}

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

fn respond_once(tag: &str, choice: &str) -> (serde_json::Value, Vec<String>, Option<String>) {
    let (guard, log) = launch_emulator(tag);
    let access = TmuxAccess {
        session: guard.session.clone(),
    };
    let result = tako_control::dispatch::respond_via(&access, 1447, Some(choice), Some("test"))
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
fn issue1447_previewつき三択へ番号とラベルで応答できる() {
    if !real_tmux() {
        eprintln!("skip: 本物の tmux が無い環境");
        return;
    }

    // --- 下見（choice 省略）: 送信せず構造だけ返す ---
    let (guard, log) = launch_emulator("peek");
    let access = TmuxAccess {
        session: guard.session.clone(),
    };
    let peek = tako_control::dispatch::respond_via(&access, 1447, None, Some("test"))
        .expect("下見が成功する（修正前は「ダイアログが画面に存在しない」）");
    eprintln!("[peek] {peek}");
    assert_eq!(peek["kind"], "select");
    assert_eq!(peek["numbered"], true);
    assert_eq!(peek["responded"], false, "下見はキーを送らない");
    let options = peek["options"].as_array().expect("options");
    assert_eq!(options.len(), 3, "{options:?}");
    for (i, want) in LABELS.iter().enumerate() {
        assert_eq!(options[i]["label"], *want, "ラベルに副画面が混ざっている");
    }
    assert!(read_log(&log).0.is_empty(), "下見でキーが送られている");
    drop(guard);

    // --- 番号指定（`2`）= Issue の受け入れ条件 3 ---
    let (by_number, keys, chose) = respond_once("number", "2");
    assert_eq!(by_number["resolved"], true, "ダイアログが解消される");
    assert_eq!(by_number["choice"], 2);
    assert_eq!(by_number["choice_text"], LABELS[1]);
    assert_eq!(by_number["stray_input"], false, "入力欄へ漏れていない");
    assert_eq!(
        keys,
        vec!["2".to_string()],
        "番号つきなので番号キー 1 つだけを送る"
    );
    assert_eq!(chose.as_deref(), Some(LABELS[1]));

    // --- ラベル指定（折り返しを繋いだラベルで引ける）---
    let (by_label, keys, chose) = respond_once("label", LABELS[2]);
    assert_eq!(by_label["resolved"], true);
    assert_eq!(by_label["choice"], 3);
    assert_eq!(keys, vec!["3".to_string()]);
    assert_eq!(chose.as_deref(), Some(LABELS[2]));
}
