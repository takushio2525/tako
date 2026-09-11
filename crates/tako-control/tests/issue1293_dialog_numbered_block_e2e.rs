//! 会話ログの番号つき箇条書きの上に出たダイアログへ respond する実キー E2E（Issue #1293）
//!
//! #1263 の `issue1263_teach_auto_mode_respond_e2e` と同じ作りの**模擬 TUI**（実 tmux・
//! 専用ソケット）だが、ダイアログの**上に claude の会話ログ（番号つき箇条書き）を残す**。
//! これが #1293 の再現形で、修正前は下見が 6 択・番号重複・`title` が会話本文になり、
//! `--choice 1` は会話ログ側の要素へ解決されて**監査ログが嘘になる**。
//!
//! 検証するのは受け入れ条件 4:
//! - 下見（`choice` 省略）が **3 択**を返し、`title` に会話本文を含まない
//! - `--choice 1` が `Yes` に解決され、送られたキーは**番号キー 1 つだけ**
//!
//! A/B は `TAKO_1293_LEGACY=1`（同一バイナリで修正前の収集へ戻す）。
//! 本物の tmux が無い環境（psmux しか無い Windows 等）ではスキップする。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use tako_control::reach::DialogAccess;

#[path = "common/tmux_e2e.rs"]
mod tmux_e2e;

/// 本番のバックエンドや**他プロセスのテスト**と混ざらない専用の器（#1300）。
/// 固定名だと `cargo test --workspace` が 2 本走った瞬間に同名セッションを
/// 取り合って `duplicate session` で落ちる（実測は `tmux_e2e` のモジュール doc）
fn socket() -> &'static str {
    tmux_e2e::socket_for("1293")
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

/// tmux セッションと一時ディレクトリを片付けるガード
struct EmuGuard {
    session: String,
    dir: PathBuf,
}

impl Drop for EmuGuard {
    fn drop(&mut self) {
        // 器はこのプロセス専用。セッションを畳み、**このプロセスの最後の 1 本**なら
        // サーバーごと退役させてソケットファイルまで消す（tmux は残す）
        tmux_e2e::release_session(socket(), &self.session);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// テストから tmux セッションへ直に届く `DialogAccess`（CLI / MCP と同じ 2 手だけ）
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

/// **会話ログの番号つき箇条書きを上に残したまま** auto mode の確認を描く模擬 TUI。
/// 会話ログの並びは Issue #1293 本文の再現画面と同じ（番号が 1 から振り直される 3 項目）
const EMULATOR: &str = r#"#!/bin/bash
# tako の respond（#1293）の実キー検証用。実 claude の画面の形だけを真似る
LOG="$1"
: > "$LOG"
labels=("Yes" "Not now" "Don't show again")
rule="────────────────────────────────────────────────────────────────────────"
draw() {
  printf '\033[2J\033[H'
  echo "⏺ 直し方の候補は 3 つあります。"
  echo
  echo "  1. 待ちを状態待ちへ寄せる"
  echo "  2. 番犬テストを足す"
  echo "  3. 何もしない"
  echo
  echo "⏺ 1 を採ります。"
  echo
  echo "  Teach auto mode about your environment?"
  echo
  echo "  Auto mode works better when it knows your environment. Takes about a minute."
  echo
  echo "  ❯ 1. ${labels[0]}"
  echo "    2. ${labels[1]}"
  echo "    3. ${labels[2]}"
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
while :; do
  IFS= read -rsn1 c || break
  case "$c" in
    1|2|3)
      echo "KEY=$c" >> "$LOG"
      echo "CHOSE=${labels[$((c - 1))]}" >> "$LOG"
      break
      ;;
    ''|$'\r'|$'\n')
      echo "KEY=Enter" >> "$LOG"
      echo "CHOSE=${labels[0]}" >> "$LOG"
      break
      ;;
  esac
done
stty sane 2>/dev/null
# 確定後はダイアログを消す（入力欄だけが残る = respond の解消検証が真になる形）
printf '\033[2J\033[H'
echo "⏺ 1 を採ります。"
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
        "/private/tmp/tako-e2e-1293-{tag}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("作業ディレクトリを作れる");
    let script = dir.join("teach-emu.sh");
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
    let session = format!("tako1293{tag}");
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
                log.to_str().expect("UTF-8")
            ),
        ],
    ) {
        panic!("{diag}");
    }
    // ダイアログが検知されるまで待つ（描画前に応答させると材料が無い）
    let drawn = wait_until(Duration::from_secs(10), || {
        tako_core::tmux::capture_session(Some(socket()), &session)
            .map(|l| tako_control::claude_tui::is_choice_dialog(&l))
            .unwrap_or(false)
    });
    assert!(
        drawn,
        "模擬 TUI のダイアログが検知されない:\n{}",
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

#[test]
fn issue1293_会話ログの上のダイアログの下見が三択を返す() {
    if !real_tmux() {
        eprintln!("skip: 本物の tmux が無い環境");
        return;
    }

    // --- 下見（choice 省略）: 送信せず構造だけ返す ---
    let (guard, log) = launch_emulator("peek");
    let access = TmuxAccess {
        session: guard.session.clone(),
    };
    let peek = tako_control::dispatch::respond_via(&access, 1293, None, Some("test"))
        .expect("下見が成功する");
    eprintln!("[peek] {peek}");
    let options = peek["options"].as_array().expect("options").clone();
    assert_eq!(
        options.len(),
        3,
        "会話ログの箇条書きが選択肢へ混ざっている: {options:?}"
    );
    assert_eq!(options[0]["label"], "Yes");
    assert_eq!(options[0]["number"], 1);
    assert_eq!(peek["numbered"], true, "番号キーで確定できる");
    assert_eq!(peek["responded"], false, "下見はキーを送らない");
    let title = peek["title"].as_str().unwrap_or_default().to_string();
    assert!(
        title.contains("Teach auto mode about your environment?"),
        "title がダイアログの説明文でない: {title:?}"
    );
    for log_text in ["直し方の候補", "待ちを状態待ちへ寄せる", "1 を採ります"]
    {
        assert!(
            !title.contains(log_text),
            "title に会話本文が混ざっている（{log_text}）: {title:?}"
        );
    }
    assert!(
        read_log(&log).0.is_empty(),
        "下見でキーが送られてはいけない: {:?}",
        read_log(&log).0
    );
    drop(guard);
}

/// `--choice 1` が**会話ログ側の要素**へ解決されないこと（#1293 の壊れ方 2・3）。
/// 修正前は `choice_text` が `待ちを状態待ちへ寄せる` になり、送ったキーは同じでも
/// 監査ログと応答 JSON だけが嘘になる
#[test]
fn issue1293_番号指定はダイアログ側の選択肢へ解決される() {
    if !real_tmux() {
        eprintln!("skip: 本物の tmux が無い環境");
        return;
    }
    let (guard, log) = launch_emulator("choice1");
    let access = TmuxAccess {
        session: guard.session.clone(),
    };
    let result = tako_control::dispatch::respond_via(&access, 1293, Some("1"), Some("test"))
        .unwrap_or_else(|e| panic!("respond が失敗した: {e:?}\n{}", dump(&guard.session)));
    let confirmed = wait_until(Duration::from_secs(5), || read_log(&log).1.is_some());
    let (keys, chose) = read_log(&log);
    eprintln!("[choice1] result={result} keys={keys:?} chose={chose:?} confirmed={confirmed}");
    assert_eq!(result["resolved"], true, "ダイアログが解消される");
    assert_eq!(result["choice"], 1);
    assert_eq!(
        result["choice_text"], "Yes",
        "監査ログのラベルが会話本文になっている"
    );
    assert_eq!(result["stray_input"], false, "入力欄へ漏れていない");
    assert_eq!(
        keys,
        vec!["1".to_string()],
        "番号つきなので番号キー 1 つだけを送る"
    );
    assert_eq!(chose.as_deref(), Some("Yes"));
}
