//! peer 送達（claude の Cross-Session Messaging）の実機 E2E（Issue #790）。
//! 実 tmux + 実 `claude` CLI + Anthropic API を使うためすべて `#[ignore]`。
//!
//! ```sh
//! cargo test -p tako-control --test peer_messaging_e2e -- --ignored --test-threads=1
//! ```
//!
//! 前提: `claude` CLI がログイン済み（v2.1.224 以降）/ `tmux` がある / ネットワーク接続。
//! 受信箱はサーバー側 gate に依存するので、gate が off の環境では
//! 「使えない → 従来経路へ落ちる」側のテストだけが通る（それも仕様どおりの結果）。
//!
//! 隔離: 専用 tmux ソケット + `TAKO_DATA_DIR`（persist.log を本番へ書かない）。
//! claude の会話は実ユーザーの config dir に残る（transcript を読む検証のため必須）。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use tako_control::{claude_tui, delivery, peer_messaging};

/// 専用ソケットで本番バックエンド（tako-backend）や他の実験用 tmux と隔離する
const SOCKET: &str = "tako-e2e-790";

/// 応答マーカー（数字はステータスライン `5h 45% (→4h42m)` と誤マッチするため英単語）
const SPELL_SUFFIX: &str = "Reply with only the answer spelled out in English words, lowercase.";

/// 作業ディレクトリ。`/private/tmp` 直下（`$TMPDIR` は祖先が信頼済みになりがち）
fn work_dir(name: &str) -> PathBuf {
    PathBuf::from(format!(
        "/private/tmp/tako-e2e-790-{name}-{}",
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
        assert!(
            self.dir.starts_with("/private/tmp/"),
            "一時ディレクトリ以外を削除しようとしている: {}",
            self.dir.display()
        );
        let _ = std::fs::remove_dir_all(&self.dir);
        remove_trust_entry(&self.dir);
    }
}

/// claude の `.claude.json` からテスト用ディレクトリの projects エントリを除去する
/// （best-effort）。`launch_claude` が事前信頼を書き込むので、消さないと実行のたびに
/// ユーザーの設定へ残骸が溜まる（#612 と同じ後始末。書き先の解決規則も同じ）
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

/// 隔離 env（persist.log の書き先と tmux ソケット）をプロセスへ入れる。
/// `find_claude_pid_for_backend` / `socket_name` はこの env を読む
fn isolate(name: &str) -> PathBuf {
    let data = work_dir(&format!("{name}-data"));
    std::fs::create_dir_all(&data).expect("data dir を作れる");
    std::env::set_var("TAKO_DATA_DIR", &data);
    std::env::set_var("TAKO_TMUX_SOCKET", SOCKET);
    data
}

/// 事前信頼を書いてから claude を tmux セッションで起動する
/// （信頼ダイアログは #32 の担当。ここでは送達経路だけを見る）
fn launch_claude(session: &str, dir: &Path) -> SessionGuard {
    std::fs::create_dir_all(dir).expect("作業ディレクトリを作れる");
    claude_tui::ensure_trusted_in(None, &dir.display().to_string()).expect("事前信頼を書ける");
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

fn capture(session: &str) -> Option<Vec<String>> {
    tako_core::tmux::capture_session(Some(SOCKET), session).ok()
}

fn dump_screen(session: &str) -> String {
    capture(session)
        .map(|l| l.join("\n"))
        .unwrap_or_else(|| "<capture 失敗>".into())
}

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

/// 英単語の答えを待つ。区切り（`forty-two` / `forty two` / `Forty-Two.`）に依存しないよう
/// 英数字以外を落として比較する
fn wait_for_answer(session: &str, normalized_marker: &str, timeout: Duration) -> bool {
    wait_until(timeout, || {
        capture(session).is_some_and(|lines| {
            lines.iter().any(|line| {
                let squashed: String = line
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .flat_map(|c| c.to_lowercase())
                    .collect();
                squashed.contains(normalized_marker)
            })
        })
    })
}

/// 受信箱が使えるか（サーバー側 gate に依存する）。使えない環境ではテストをスキップする。
///
/// 受信箱は claude の起動途中に開く（実測 1.1 秒）ので、入力欄が見えた直後は
/// まだ無いことがある。一時的な理由なら待ってから判定する
/// （待たずに 1 回で諦めると、可用な環境でも黙ってスキップして検証が空振りする）
fn peer_target_or_skip(session: &str) -> Option<peer_messaging::PeerTarget> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match peer_messaging::resolve_for_backend(session) {
            Ok(target) => return Some(target),
            Err(reason) if reason.is_transient() && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(reason) => {
                eprintln!(
                    "SKIP: この環境では peer 送達が使えない（{}: {}）",
                    reason.code(),
                    reason.note()
                );
                return None;
            }
        }
    }
}

/// 解決できた宛先の事実を出す（失敗時の切り分けと、通ったときの証拠のため。
/// トークンは持たない = 出せない構造にしてある）
fn evidence(target: &peer_messaging::PeerTarget) {
    eprintln!(
        "[e2e-790] 宛先 pid={} kind={} status={} version={} peerProtocol={} socket={}",
        target.session.pid,
        target.session.kind,
        target.session.status.as_deref().unwrap_or("?"),
        target.session.version,
        target.session.peer_protocol,
        target.socket_path.display()
    );
}

/// transcript に「送信後の」peer 由来レコードがあるか（送達の機械検証）
fn transcript_has_peer_record(session_id: &str, from_len: u64) -> bool {
    let Some(path) = tako_control::transcript::find_transcript(session_id) else {
        return false;
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return false;
    };
    if (raw.len() as u64) < from_len {
        return false;
    }
    peer_messaging::verify_in_lines(raw[from_len as usize..].lines()).is_received()
}

fn transcript_len(session_id: &str) -> u64 {
    tako_control::transcript::find_transcript(session_id)
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .unwrap_or(0)
}

/// 受け入れ条件 2（idle）: idle の worker へ peer 経路で送達し transcript で受信を確認する
#[test]
#[ignore = "実 tmux + 実 claude + API を使う（手動実行専用）"]
fn peer_送達が_idle_の_worker_へ届く() {
    isolate("idle");
    let dir = work_dir("idle");
    let guard = launch_claude("peer-idle", &dir);
    wait_for_input_line(&guard.session);

    let Some(target) = peer_target_or_skip(&guard.session) else {
        return;
    };
    let session_id = target.session.session_id.clone();
    assert_eq!(
        target.session.kind, "interactive",
        "対話型セッションとして解決される"
    );
    evidence(&target);
    let before = transcript_len(&session_id);

    // 送達（agent_managed = true: worker 宛の送達を模す）
    let attempt = delivery::try_peer(
        &guard.session,
        &format!("What is 40 + 2? {SPELL_SUFFIX}"),
        true,
    );
    let outcome = match attempt {
        delivery::PeerAttempt::Sent(outcome) => outcome,
        delivery::PeerAttempt::Fallback { reason, .. } => {
            panic!("peer で送れるはず（fallback: {reason}）")
        }
        delivery::PeerAttempt::Refused { note } => panic!("peer で送れるはず（refused: {note}）"),
    };
    assert_eq!(outcome.transport, peer_messaging::Transport::Peer);
    eprintln!(
        "[e2e-790] idle 送達: transport={} 確認={}",
        outcome.transport.as_str(),
        outcome.verification.map(|v| v.as_str()).unwrap_or("なし")
    );
    assert!(
        outcome.verification.is_some_and(|v| v.is_received()),
        "transcript で受信を確認できる: {outcome:?}"
    );

    // ① transcript に peer 由来のレコードが増えている（機械検証）
    assert!(
        transcript_has_peer_record(&session_id, before),
        "送信後の transcript に peer 由来のレコードがある"
    );
    // ② claude が実際に応答している（画面）
    assert!(
        wait_for_answer(&guard.session, "fortytwo", Duration::from_secs(90)),
        "claude が応答するはず。画面:\n{}",
        dump_screen(&guard.session)
    );
}

/// 受け入れ条件 2（busy）: 生成中の worker へ送っても取りこぼさない
/// （従来経路が最も苦手にしてきた状況。#572）
#[test]
#[ignore = "実 tmux + 実 claude + API を使う（手動実行専用）"]
fn peer_送達が_busy_の_worker_へ届く() {
    isolate("busy");
    let dir = work_dir("busy");
    let guard = launch_claude("peer-busy", &dir);
    wait_for_input_line(&guard.session);

    let Some(target) = peer_target_or_skip(&guard.session) else {
        return;
    };
    let session_id = target.session.session_id.clone();
    evidence(&target);

    // ① 長めの生成を始めさせて busy にする
    match delivery::try_peer(
        &guard.session,
        "Count from 1 to 120, one number per line, with its English word. Do not use tools.",
        true,
    ) {
        delivery::PeerAttempt::Sent(_) => {}
        other => panic!("1 通目が peer で送れるはず: {}", attempt_label(&other)),
    }
    // レジストリの status で busy を待つ（画面の文言に依存しない）
    assert!(
        wait_until(Duration::from_secs(60), || {
            peer_messaging::resolve_for_backend(&guard.session)
                .is_ok_and(|t| t.session.status.as_deref() == Some("busy"))
        }),
        "生成中（busy）になるはず。画面:\n{}",
        dump_screen(&guard.session)
    );

    // ② 生成中に 2 通目を送る。
    //    「busy を一度でも観測した」だけでは送信時点も busy とは言えないので、
    //    送信の直前に取り直した状態を証拠として残し、busy でなければ落とす
    let at_send = peer_messaging::resolve_for_backend(&guard.session)
        .expect("宛先は解決できる")
        .session
        .status
        .unwrap_or_default();
    eprintln!("[e2e-790] 2 通目の送信時点の状態={at_send}");
    assert_eq!(at_send, "busy", "生成中に送っている");
    let before = transcript_len(&session_id);
    let outcome = match delivery::try_peer(
        &guard.session,
        &format!("Ignore the counting task. What is 900 + 5? {SPELL_SUFFIX}"),
        true,
    ) {
        delivery::PeerAttempt::Sent(outcome) => outcome,
        other => panic!("生成中でも peer で送れるはず: {}", attempt_label(&other)),
    };
    assert_eq!(outcome.transport, peer_messaging::Transport::Peer);
    eprintln!(
        "[e2e-790] busy 送達: transport={} 確認={}",
        outcome.transport.as_str(),
        outcome.verification.map(|v| v.as_str()).unwrap_or("なし")
    );
    assert!(
        outcome.verification.is_some_and(|v| v.is_received()),
        "生成中の送達はキュー投函として確認できる: {outcome:?}"
    );
    assert!(
        transcript_has_peer_record(&session_id, before),
        "生成中に送ったぶんも transcript に痕跡がある"
    );

    // ③ ターン終了後に 2 通目が処理される（取りこぼしていない）
    assert!(
        wait_for_answer(&guard.session, "hundredfive", Duration::from_secs(180)),
        "生成中に送った指示がターン後に処理されるはず。画面:\n{}",
        dump_screen(&guard.session)
    );
}

/// 受け入れ条件 3: peer が使えないときは従来のキー操作経路へ落ちて送達が成立する
#[test]
#[ignore = "実 tmux + 実 claude + API を使う（手動実行専用）"]
fn peer_が使えないときはキー経路で届く() {
    isolate("fallback");
    // peer 不成立を模擬する（実環境の「サーバー側 gate off」「古い claude」と同じ結果）
    std::env::set_var(peer_messaging::ENV_MODE, "off");
    let dir = work_dir("fallback");
    let guard = launch_claude("peer-fallback", &dir);
    wait_for_input_line(&guard.session);

    // ① 経路選択が従来経路を選ぶ
    match delivery::try_peer(&guard.session, "dummy", true) {
        delivery::PeerAttempt::Fallback { reason, .. } => assert_eq!(reason, "disabled"),
        other => panic!("従来経路へ落ちるはず: {}", attempt_label(&other)),
    }

    // ② 従来経路で実際に送達が成立する（画面で応答を確認）
    let report = claude_tui::deliver_via_tmux(
        Some(SOCKET),
        &guard.session,
        &format!("What is 6 * 7? {SPELL_SUFFIX}"),
        true,
    )
    .expect("送達が完了する");
    eprintln!("[e2e-790] keys 送達: {report:?}");
    assert!(report.verified, "キー経路で送達を検証できる: {report:?}");
    assert!(
        wait_for_answer(&guard.session, "fortytwo", Duration::from_secs(90)),
        "claude が応答するはず。画面:\n{}",
        dump_screen(&guard.session)
    );
    std::env::remove_var(peer_messaging::ENV_MODE);
}

/// panic メッセージ用のラベル（`PeerAttempt` は Debug を持たない = トークンを含みうるため）
fn attempt_label(attempt: &delivery::PeerAttempt) -> String {
    match attempt {
        delivery::PeerAttempt::Sent(outcome) => format!("Sent({:?})", outcome.transport),
        delivery::PeerAttempt::Fallback { reason, transient } => {
            format!("Fallback({reason}, transient={transient})")
        }
        delivery::PeerAttempt::Refused { note } => format!("Refused({note})"),
    }
}
