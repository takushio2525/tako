//! claude_tui の実機 E2E（Issue #32）。実 tmux + 実 `claude` CLI + Anthropic API を
//! 使うためすべて `#[ignore]`。CI では走らない。手動実行:
//!
//! ```sh
//! cargo test -p tako-control --test claude_tui_e2e -- --ignored --test-threads=1
//! ```
//!
//! 前提: `claude` CLI がログイン済み / `tmux` がある / ネットワーク接続。
//!
//! 注意:
//! - 実ユーザーの `~/.claude.json` に一時ディレクトリの projects エントリを追加する
//!   （テスト終了時に best-effort で除去する）
//! - Claude Code の信頼は**祖先ディレクトリの信頼済みエントリにも及ぶ**（実測）。
//!   `std::env::temp_dir()`（`$TMPDIR` = `/var/folders/...`）はルートが信頼済みに
//!   なりがちなので使わず、`/private/tmp` 直下に作る。未信頼テストが
//!   「ダイアログが出ない」で落ちる場合は祖先の信頼済みエントリを疑うこと

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use tako_control::claude_tui;

/// 専用ソケットで tako 本体のバックエンド（tako-backend）や実験用 tmux と隔離する
const SOCKET: &str = "tako-e2e-32";

/// 3 テスト共通の応答マーカー（40+2 / 50−8 / 6×7 の答えを英語綴りで返させる）。
/// 数字の "42" はステータスライン（`5h 45% (→4h42m)` 等）と誤マッチするため使わない
const ANSWER_MARKER: &str = "forty-two";
const SPELL_SUFFIX: &str = "Reply with only the answer spelled out in English words, lowercase.";

/// 信頼済みの祖先が無い、素の未信頼ディレクトリを作る（モジュールコメント参照）
fn untrusted_base_dir(name: &str) -> PathBuf {
    PathBuf::from(format!(
        "/private/tmp/tako-e2e-32-{name}-{}",
        std::process::id()
    ))
}

/// テスト終了時に tmux セッションと作業ディレクトリを片付けるガード
struct SessionGuard {
    session: String,
    dir: PathBuf,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", SOCKET, "kill-session", "-t", &self.session])
            .output();
        let _ = std::fs::remove_dir_all(&self.dir);
        remove_trust_entry(&self.dir);
    }
}

/// ~/.claude.json からテスト用ディレクトリの projects エントリを除去する（best-effort）
fn remove_trust_entry(dir: &Path) {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let path = PathBuf::from(home).join(".claude.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    let Some(projects) = root.get_mut("projects").and_then(|p| p.as_object_mut()) else {
        return;
    };
    if projects.remove(&dir.display().to_string()).is_some() {
        if let Ok(serialized) = serde_json::to_string_pretty(&root) {
            let _ = std::fs::write(&path, serialized);
        }
    }
}

/// 指定ディレクトリで claude を tmux セッションとして起動する
fn launch_claude(session: &str, dir: &Path) -> SessionGuard {
    std::fs::create_dir_all(dir).expect("作業ディレクトリを作れる");
    let status = Command::new("tmux")
        .args([
            "-L",
            SOCKET,
            "new-session",
            "-d",
            "-s",
            session,
            "-x",
            "100",
            "-y",
            "35",
            "-c",
            dir.to_str().expect("テストパスは UTF-8"),
            "claude --model haiku",
        ])
        .status()
        .expect("tmux を実行できる");
    assert!(status.success(), "tmux new-session が失敗した");
    SessionGuard {
        session: session.to_string(),
        dir: dir.to_path_buf(),
    }
}

/// 画面にマーカー文字列が現れるまで待つ（claude の応答確認用。大文字小文字を無視）
fn wait_for_marker(session: &str, marker: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    let marker = marker.to_lowercase();
    while Instant::now() < deadline {
        if let Ok(lines) = tako_core::tmux::capture_session(Some(SOCKET), session) {
            if lines.iter().any(|l| l.to_lowercase().contains(&marker)) {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    false
}

fn dump_screen(session: &str) -> String {
    tako_core::tmux::capture_session(Some(SOCKET), session)
        .map(|l| l.join("\n"))
        .unwrap_or_else(|e| format!("<capture 失敗: {e}>"))
}

/// Issue #32 問題 1（フォールバック経路）: 未信頼フォルダの初回起動で信頼ダイアログが
/// 出ても、検出 → 承諾 → プロンプト送達が通る
#[test]
#[ignore = "実 tmux + 実 claude + API を使う（手動実行専用）"]
fn 未信頼フォルダでダイアログ承諾からの送達が通る() {
    let dir = untrusted_base_dir("trust-fallback");
    // 事前信頼はしない → 信頼ダイアログが表示されるはず
    let guard = launch_claude("trust-fallback", &dir);
    let report = claude_tui::deliver_via_tmux(
        Some(SOCKET),
        &guard.session,
        &format!("What is 40 + 2? {SPELL_SUFFIX}"),
        true,
    )
    .expect("送達が完了する");
    assert!(
        report.trust_dialogs_accepted >= 1,
        "信頼ダイアログを承諾しているはず（出ない場合は祖先ディレクトリの信頼済みエントリを疑う）: \
         {report:?}\n画面:\n{}",
        dump_screen(&guard.session)
    );
    assert!(
        report.verified,
        "入力欄が空へ戻ったことを検証できるはず: {report:?}\n画面:\n{}",
        dump_screen(&guard.session)
    );
    assert!(
        wait_for_marker(&guard.session, ANSWER_MARKER, Duration::from_secs(90)),
        "claude が応答するはず（= プロンプトが送達された）。画面:\n{}",
        dump_screen(&guard.session)
    );
}

/// Issue #32 問題 1（事前信頼経路）: spawn 前の ensure_trusted で信頼ダイアログ自体が
/// 出ず、そのまま送達が通る
#[test]
#[ignore = "実 tmux + 実 claude + API を使う（手動実行専用）"]
fn 事前信頼でダイアログなしの送達が通る() {
    let dir = untrusted_base_dir("pretrust");
    std::fs::create_dir_all(&dir).expect("作業ディレクトリを作れる");
    // 未信頼の親しか持たないディレクトリを起動前に信頼済みへ（= spawn の事前信頼と同じ）
    assert_eq!(
        claude_tui::ensure_trusted(&dir.display().to_string()),
        Ok(true),
        "事前信頼を書き込める"
    );
    let guard = launch_claude("pretrust", &dir);

    let report = claude_tui::deliver_via_tmux(
        Some(SOCKET),
        &guard.session,
        &format!("What is 50 - 8? {SPELL_SUFFIX}"),
        true,
    )
    .expect("送達が完了する");
    assert_eq!(
        report.trust_dialogs_accepted,
        0,
        "事前信頼済みならダイアログは出ないはず: {report:?}\n画面:\n{}",
        dump_screen(&guard.session)
    );
    assert!(report.verified, "送達検証が通るはず: {report:?}");
    assert!(
        wait_for_marker(&guard.session, ANSWER_MARKER, Duration::from_secs(90)),
        "claude が応答するはず。画面:\n{}",
        dump_screen(&guard.session)
    );
}

/// Issue #32 問題 2: 長文マルチラインが bracketed paste + 分離 Enter で
/// 「入力欄に貼り付いたまま」にならず 1 メッセージとして送達される
#[test]
#[ignore = "実 tmux + 実 claude + API を使う（手動実行専用）"]
fn 長文マルチラインsendが送達される() {
    let dir = untrusted_base_dir("multiline");
    let guard = launch_claude("multiline", &dir);
    // 起動時の信頼ダイアログはここでは本題でないため deliver に処理させる。
    // 旧実装で確実に失敗した形: 複数行 + 長い行 + 日本語 + 末尾改行
    let long_line = "これは長い行のテストです。".repeat(8);
    let text = format!(
        "You are being tested for multiline prompt delivery.\n\
         The following lines are part of ONE message:\n\
         - 項目その 1: マルチライン送信の検証\n\
         - 項目その 2: {long_line}\n\
         - item 3: this line is filler to make the message long\n\
         \n\
         Final line: What is 6 * 7? {SPELL_SUFFIX}\n"
    );
    let report = claude_tui::deliver_via_tmux(Some(SOCKET), &guard.session, &text, true)
        .expect("送達が完了する");
    assert!(
        report.verified,
        "マルチラインでも入力欄が空へ戻るはず: {report:?}\n画面:\n{}",
        dump_screen(&guard.session)
    );
    assert!(
        wait_for_marker(&guard.session, ANSWER_MARKER, Duration::from_secs(90)),
        "最終行の質問に応答するはず（= 全行が 1 メッセージで送達された）。画面:\n{}",
        dump_screen(&guard.session)
    );
}
