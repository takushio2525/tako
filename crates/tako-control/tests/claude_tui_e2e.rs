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
//! - 実ユーザーの `.claude.json`（`claude_tui::config_json_paths`。既定は
//!   `~/.claude/.claude.json`）に一時ディレクトリの projects エントリを追加する
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

/// claude の `.claude.json` からテスト用ディレクトリの projects エントリを除去する
/// （best-effort）。書き先は `claude_tui::config_json_paths` と同じ解決規則で引く
/// （#558: claude は config dir 配下を読む。ホーム直下だけ消しても残骸が溜まっていた）
fn remove_trust_entry(dir: &Path) {
    for path in claude_tui::config_json_paths(None) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let Some(projects) = root.get_mut("projects").and_then(|p| p.as_object_mut()) else {
            continue;
        };
        if projects.remove(&dir.display().to_string()).is_some() {
            if let Ok(serialized) = serde_json::to_string_pretty(&root) {
                let _ = std::fs::write(&path, serialized);
            }
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

fn capture(session: &str) -> Option<Vec<String>> {
    tako_core::tmux::capture_session(Some(SOCKET), session).ok()
}

/// 条件が成立するまで待つ（500ms 間隔）
fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    false
}

/// claude TUI の入力欄（❯）が現れるまで待つ
fn wait_for_input_line(session: &str) {
    assert!(
        wait_until(Duration::from_secs(60), || {
            capture(session).is_some_and(|l| claude_tui::input_line(&l).is_some())
        }),
        "claude TUI の入力欄が現れるはず。画面:\n{}",
        dump_screen(session)
    );
}

/// 人間のタイプ相当: 1 バイトずつ送る（GUI の handle_key は 1 キーずつ PTY へ書く）
fn type_like_human(session: &str, text: &str) {
    for byte in text.as_bytes() {
        let status = Command::new("tmux")
            .args([
                "-L",
                SOCKET,
                "send-keys",
                "-t",
                &format!("={session}:"),
                "-H",
                &format!("{byte:02x}"),
            ])
            .status()
            .expect("tmux を実行できる");
        assert!(status.success(), "send-keys -H が失敗した");
        std::thread::sleep(Duration::from_millis(30));
    }
    std::thread::sleep(Duration::from_millis(500));
}

fn send_key(session: &str, key: &str) {
    tako_core::tmux::send_key(Some(SOCKET), session, key).expect("キー送信できる");
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

/// Issue #95: 入力欄に残留したテキストを Enter 単独送達（text = ""）で送信できる。
/// 人間のタイプ相当（tmux send-keys -l）でテキストだけ入力欄に載せた状態から、
/// deliver_via_tmux("") が Enter を送り、入力欄が空へ戻ることを検証する
#[test]
#[ignore = "実 tmux + 実 claude + API を使う（手動実行専用）"]
fn 残留テキストをenter単独送達で送信できる() {
    let dir = untrusted_base_dir("enter-only");
    std::fs::create_dir_all(&dir).expect("作業ディレクトリを作れる");
    assert_eq!(
        claude_tui::ensure_trusted(&dir.display().to_string()),
        Ok(true),
        "事前信頼を書き込める"
    );
    let guard = launch_claude("enter-only", &dir);

    // 入力欄（❯）の表示を待つ
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let lines =
            tako_core::tmux::capture_session(Some(SOCKET), &guard.session).expect("画面を読める");
        if claude_tui::input_line(&lines).is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "claude TUI の入力欄が現れるはず。画面:\n{}",
            dump_screen(&guard.session)
        );
        std::thread::sleep(Duration::from_millis(500));
    }

    // 人間のタイプ相当: テキストだけ入力欄に載せる（Enter は送らない = 残留状態）
    let status = Command::new("tmux")
        .args([
            "-L",
            SOCKET,
            "send-keys",
            "-t",
            &format!("={}:", guard.session),
            "-l",
            &format!("What is 3 * 7? {SPELL_SUFFIX}"),
        ])
        .status()
        .expect("tmux を実行できる");
    assert!(status.success(), "send-keys が失敗した");
    std::thread::sleep(Duration::from_millis(1500));

    // Enter 単独送達（tako_send_input text:"" + newline:true の tmux 経路と同じ）
    let report = claude_tui::deliver_via_tmux(Some(SOCKET), &guard.session, "", false)
        .expect("送達が完了する");
    assert!(
        report.verified,
        "入力欄が空へ戻ったことを検証できるはず: {report:?}\n画面:\n{}",
        dump_screen(&guard.session)
    );
    assert!(
        wait_for_marker(&guard.session, "twenty-one", Duration::from_secs(90)),
        "残留テキストが送信され claude が応答するはず。画面:\n{}",
        dump_screen(&guard.session)
    );
}

/// Issue #572: busy（生成中）の claude へ人間が打った指示は **入力欄ではなく
/// claude のメッセージキュー** に入る。このとき入力欄自体は空で、代わりに dim の
/// ヒント `Press up to edit queued messages` が表示される。
///
/// 修正前はこのヒントを「残留テキスト」と誤認していたため、Enter 単独送達
/// （#95 / master の Enter 代行）が Enter を 5 回空撃ちして `verified=false` で終わり、
/// master は「送達に失敗した」と読み違えていた（実際はキューにあり、ターン終了時に届く）。
///
/// ここでは「busy 中にタイプ → キューに入る → 誤検知しない → 実際に届く」を通す。
/// キューに入ったまま止まった場合の救出（`Up` → `Enter`）は deliver_via_tmux 内の
/// 実装と tako-app 側の定期チェックで行う（滞留は claude 側の状態のため、
/// テストから決定的には作れない）
#[test]
#[ignore = "実 tmux + 実 claude + API を使う（手動実行専用）"]
fn busy中にタイプした指示がキューに入り誤検知されない() {
    let dir = untrusted_base_dir("busy-queue");
    std::fs::create_dir_all(&dir).expect("作業ディレクトリを作れる");
    assert_eq!(
        claude_tui::ensure_trusted(&dir.display().to_string()),
        Ok(true),
        "事前信頼を書き込める"
    );
    let guard = launch_claude("busy-queue", &dir);
    wait_for_input_line(&guard.session);

    // ① 長めのタスクを送って busy にする
    claude_tui::deliver_via_tmux(
        Some(SOCKET),
        &guard.session,
        "Write a numbered list from 1 to 120. Each line is one short English sentence. No tools.",
        true,
    )
    .expect("送達が完了する");
    assert!(
        wait_until(Duration::from_secs(60), || {
            capture(&guard.session).is_some_and(|l| claude_tui::is_busy(&l))
        }),
        "claude が生成中になるはず。画面:\n{}",
        dump_screen(&guard.session)
    );

    // ② busy 中に人間が 1 バイトずつタイプ → Enter（GUI の handle_key と同じ形）
    type_like_human(&guard.session, &format!("What is 6 * 7? {SPELL_SUFFIX}"));
    send_key(&guard.session, "Enter");

    // ③ キューに入ったことを検知でき、かつ「残留テキスト」とは誤認しない
    assert!(
        wait_until(Duration::from_secs(20), || {
            capture(&guard.session).is_some_and(|l| claude_tui::queued_messages_pending(&l))
        }),
        "キューに入ったことを検知できるはず。画面:\n{}",
        dump_screen(&guard.session)
    );
    let lines = capture(&guard.session).expect("画面を読める");
    assert_eq!(
        claude_tui::input_line(&lines).map(claude_tui::input_content_is_empty),
        Some(true),
        "キュー滞留ヒントは残留テキストではない（修正前はここが Some(false)）。画面:\n{}",
        dump_screen(&guard.session)
    );

    // ④ この状態への Enter 単独送達は「送信済み」と正しく判定される
    //    （修正前は 5 回空撃ちして enter_retries=4 / verified=false で終わっていた）
    let report = claude_tui::deliver_via_tmux(Some(SOCKET), &guard.session, "", false)
        .expect("送達が完了する");
    assert!(
        report.verified,
        "キュー滞留ヒントを残留と誤認しないはず: {report:?}\n画面:\n{}",
        dump_screen(&guard.session)
    );
    assert_eq!(
        report.enter_retries, 0,
        "空撃ちの Enter 再送は起きないはず: {report:?}"
    );

    // ⑤ 実際に claude へ届く（= 応答が返る）
    assert!(
        wait_for_marker(&guard.session, ANSWER_MARKER, Duration::from_secs(180)),
        "busy 中に打った指示がターン終了後に処理されるはず。画面:\n{}",
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

// --- Issue #748: 選択肢ダイアログの検知・構造化取得・応答 ---

/// 指定モードで claude を起動する（#748。permission ダイアログを出すには
/// `--permission-mode manual` が要る。既定は auto でツール実行が自動承認される）
fn launch_claude_mode(session: &str, dir: &Path, mode: &str) -> SessionGuard {
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
            "110",
            "-y",
            "40",
            "-c",
            dir.to_str().expect("テストパスは UTF-8"),
            &format!("claude --model haiku --permission-mode {mode}"),
        ])
        .status()
        .expect("tmux を実行できる");
    assert!(status.success(), "tmux new-session が失敗した");
    SessionGuard {
        session: session.to_string(),
        dir: dir.to_path_buf(),
    }
}

/// ダイアログが実在するまで待って構造を返す
fn wait_for_dialog(session: &str, timeout: Duration) -> Option<claude_tui::ChoiceDialog> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(lines) = capture(session) {
            if let Some(d) = claude_tui::detect_choice_dialog(&lines) {
                return Some(d);
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    None
}

/// #748 受け入れ 1〜3（permission ダイアログ = 番号つき選択肢の実物）:
/// 検知（種別つき）→ 構造化取得 → 応答 → 解決 が実 claude で通り、
/// ダイアログ中のテキスト送達は入力欄へ混入しない。
///
/// **番号キーだけで確定する**ことを実測で固定するのがこのテストの主眼
/// （旧 respond は番号 + Enter を送っており、余分な Enter が解消後の入力欄へ抜けていた）
#[test]
#[ignore = "実 tmux + 実 claude + API を使う（手動実行専用）"]
fn issue748_実permissionダイアログを構造化して番号キーで確定できる() {
    let dir = untrusted_base_dir("dialog-permission");
    assert_eq!(
        claude_tui::ensure_trusted(&dir.display().to_string()),
        Ok(true),
        "事前信頼を書き込める（信頼ダイアログを本題から外す）"
    );
    let guard = launch_claude_mode("dialog-permission", &dir, "manual");
    wait_for_input_line(&guard.session);

    // 許可リストに無いコマンド（perl）を頼む → permission ダイアログが出る
    let report = claude_tui::deliver_via_tmux(
        Some(SOCKET),
        &guard.session,
        "Run exactly this with the Bash tool and nothing else: perl -e 'print 7'",
        true,
    )
    .expect("送達が完了する");
    assert!(report.verified, "依頼が送達される: {report:?}");

    // ① 検知: 種別つきで拾える
    let dialog = wait_for_dialog(&guard.session, Duration::from_secs(120)).unwrap_or_else(|| {
        panic!(
            "permission ダイアログを検知するはず。画面:\n{}",
            dump_screen(&guard.session)
        )
    });
    assert_eq!(
        dialog.kind,
        claude_tui::DialogKind::Permission,
        "種別は permission: {dialog:?}"
    );
    assert!(dialog.numbered, "番号つき: {dialog:?}");
    assert!(dialog.options.len() >= 2, "選択肢が並ぶ: {dialog:?}");
    assert_eq!(dialog.highlighted, Some(0), "既定は先頭: {dialog:?}");
    assert!(
        dialog.options[0].label.to_lowercase().starts_with("yes"),
        "先頭は承認: {dialog:?}"
    );

    // ② 入力欄は存在しない（= テキストを貼ってはいけない状態）
    let lines = capture(&guard.session).expect("画面が採れる");
    assert_eq!(
        claude_tui::input_line(&lines),
        None,
        "ダイアログ中は入力欄を返さない。画面:\n{}",
        dump_screen(&guard.session)
    );

    // ③ ダイアログ中のテキスト送達は貼らずに失敗する（入力欄へ混入しない）
    let blocked = claude_tui::deliver_via_tmux(
        Some(SOCKET),
        &guard.session,
        "この指示はダイアログに食われてはいけない",
        false,
    );
    assert!(
        blocked.is_err(),
        "ダイアログ中の送達は失敗するはず: {blocked:?}\n画面:\n{}",
        dump_screen(&guard.session)
    );
    let after_block = capture(&guard.session).expect("画面が採れる");
    assert!(
        !after_block.iter().any(|l| l.contains("食われてはいけない")),
        "送ろうとしたテキストが画面に現れない（貼っていない）。画面:\n{}",
        dump_screen(&guard.session)
    );
    let still = claude_tui::detect_choice_dialog(&after_block).expect("ダイアログは残っている");
    assert_eq!(still.labels(), dialog.labels(), "選択肢が変わっていない");

    // ④ 応答: **番号キーだけ**で確定する（Enter を送らない）
    let number = dialog.options[0].number.expect("番号がある").to_string();
    send_key(&guard.session, &number);
    assert!(
        wait_until(Duration::from_secs(30), || {
            capture(&guard.session).is_some_and(|l| claude_tui::detect_choice_dialog(&l).is_none())
        }),
        "番号キーだけでダイアログが解消するはず（Enter 不要の実測）。画面:\n{}",
        dump_screen(&guard.session)
    );
    // ⑤ 解決: 承認したコマンドが実行される
    assert!(
        wait_for_marker(&guard.session, "perl", Duration::from_secs(60)),
        "承認したコマンドが実行されるはず。画面:\n{}",
        dump_screen(&guard.session)
    );
}

/// #748 受け入れ 1（permission 以外の実ダイアログ）: `/model` の選択ダイアログを
/// 「一般の選択肢ダイアログ」として検知し、入力欄と混同しない。
/// API を消費しない（スラッシュコマンドはローカル処理）
#[test]
#[ignore = "実 tmux + 実 claude を使う（手動実行専用。API 消費なし）"]
fn issue748_モデル選択ダイアログを一般の選択肢として検知する() {
    let dir = untrusted_base_dir("dialog-select");
    assert_eq!(
        claude_tui::ensure_trusted(&dir.display().to_string()),
        Ok(true),
        "事前信頼を書き込める"
    );
    let guard = launch_claude_mode("dialog-select", &dir, "manual");
    wait_for_input_line(&guard.session);

    type_like_human(&guard.session, "/model");
    send_key(&guard.session, "Enter");

    let dialog = wait_for_dialog(&guard.session, Duration::from_secs(30)).unwrap_or_else(|| {
        panic!(
            "/model の選択ダイアログを検知するはず。画面:\n{}",
            dump_screen(&guard.session)
        )
    });
    assert_eq!(
        dialog.kind,
        claude_tui::DialogKind::Select,
        "permission でも limit でもない一般の選択: {dialog:?}"
    );
    assert!(dialog.numbered, "番号つき: {dialog:?}");
    assert!(dialog.options.len() >= 3, "モデルが並ぶ: {dialog:?}");
    assert!(
        dialog.highlighted.is_some(),
        "現在の選択が取れる: {dialog:?}"
    );
    let lines = capture(&guard.session).expect("画面が採れる");
    assert_eq!(
        claude_tui::input_line(&lines),
        None,
        "選択肢を入力欄と誤認しない（旧実装はここでモデル名を入力テキストとして返した）"
    );
    // Esc で閉じれば通常の入力欄に戻る（ダイアログ判定が居座らない）
    send_key(&guard.session, "Escape");
    assert!(
        wait_until(Duration::from_secs(20), || {
            capture(&guard.session).is_some_and(|l| {
                claude_tui::detect_choice_dialog(&l).is_none()
                    && claude_tui::input_line(&l).is_some()
            })
        }),
        "Esc で閉じれば入力欄が戻るはず。画面:\n{}",
        dump_screen(&guard.session)
    );
}
