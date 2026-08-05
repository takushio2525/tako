//! #729 の前提の裏取り: **実 claude が `ESC CR`（meta-Enter）を「送信せず改行」として
//! 解釈する**ことを psmux ペインで確かめる。
//!
//! psmux は CSI u を握り潰すので、tako は運べない器では修飾付き Enter を `ESC CR` へ
//! 落とす（`keys::legacy_modified`）。**その落とし先が本当に改行になるか**は claude 側の
//! 解釈に依存していて、バイト列の到達（`psmux_backend.rs` の統合テスト）だけでは足りない。
//! ここが崩れると Shift+Enter は「無反応」から「入力欄がクリアされる / 誤送信される」へ
//! 悪化するので、前提が変わったら気づけるようにしておく。
//!
//! CI では回さない（claude CLI + 認証 + psmux が要る）。手元で
//! `cargo test -p tako-core --test claude_meta_enter -- --ignored --nocapture` で実行する。

use std::process::Command;
use std::time::Duration;

use futures::channel::mpsc::UnboundedReceiver;
use tako_core::backend::{PsmuxBackend, SessionBackend, SessionRef};
use tako_core::terminal::{SpawnCommand, SpawnOptions};
use tako_core::{SessionEvent, TerminalSession};

fn pump(s: &mut TerminalSession, rx: &mut UnboundedReceiver<SessionEvent>) {
    while let Ok(event) = rx.try_recv() {
        s.process_event(event);
    }
}

fn wait_for(
    s: &mut TerminalSession,
    rx: &mut UnboundedReceiver<SessionEvent>,
    needle: &str,
    attempts: usize,
) -> bool {
    for _ in 0..attempts {
        pump(s, rx);
        if s.visible_lines().iter().any(|l| l.contains(needle)) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

fn show(label: &str, s: &mut TerminalSession) {
    eprintln!("--- {label} ---");
    for line in s.visible_lines() {
        if !line.trim().is_empty() {
            eprintln!("  |{line}");
        }
    }
}

#[test]
#[ignore = "実 claude と認証が要る"]
fn claudeはesc_crを改行として解釈する() {
    // **本番 tako への誤接続を防ぐ**（activeContext の罠）。
    // これを剥がさないと子の claude が本番インスタンスへつながる
    for k in [
        "TAKO_SOCKET",
        "TAKO_TOKEN",
        "TAKO_PANE_ID",
        "TAKO_TAB_ID",
        "TAKO_MCP_URL",
        "PSMUX_SESSION",
    ] {
        std::env::remove_var(k);
    }

    let bin = std::env::var("TAKO_PSMUX_BIN")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "psmux".to_string());
    let socket = format!("tako-729claude-{}", std::process::id());
    let owner_dir = std::env::temp_dir().join(format!("tako-729claude-own-{}", std::process::id()));
    let backend = PsmuxBackend::with_parts(bin.clone(), "3.3.7".into(), socket.clone(), owner_dir);
    let name = SessionRef::new("tako-729claude01").unwrap();

    let opts = SpawnOptions {
        command: Some(SpawnCommand {
            program: "claude".into(),
            args: vec![],
        }),
        cwd: Some(std::env::temp_dir()),
        env: vec![],
    };
    let (mut s, mut rx) = TerminalSession::spawn(120, 34, backend.wrap_spawn(opts, &name))
        .expect("器の中で claude を spawn できる");

    // 未信頼フォルダのダイアログが出たら承諾する（既定で「1. Yes」が選択済み）
    if wait_for(&mut s, &mut rx, "I trust this folder", 150) {
        show("信頼ダイアログ", &mut s);
        s.write(b"\r".to_vec());
        std::thread::sleep(Duration::from_millis(2000));
        pump(&mut s, &mut rx);
    }

    // 入力欄が出るまで待つ（claude のプロンプト記号は `>` ではなく `❯`）
    let ready = wait_for(&mut s, &mut rx, "for shortcuts", 300);
    show("claude 起動後", &mut s);
    assert!(ready, "claude の入力欄が出ない");

    // 1 行目を打つ。**器が読み始める前のバイトは落ちる**（#640）ので、
    // 出るまで打ち直す。psmux は描画も遅れるので固定 sleep ではなく内容で待つ
    let mut typed = false;
    for _ in 0..6 {
        s.write(b"AAA".to_vec());
        if wait_for(&mut s, &mut rx, "AAA", 40) {
            typed = true;
            break;
        }
    }
    show("AAA 入力後", &mut s);
    assert!(typed, "AAA が入力欄に出ない");

    // **本命**: tako が psmux ペインで送る形（ESC CR）で改行を要求する
    let enc = tako_core::keys::KeyEncoding {
        extended_keys: false,
        ..Default::default()
    };
    let bytes = tako_core::keys::encode_key("shift-enter", enc).unwrap();
    assert_eq!(bytes, b"\x1b\r", "送る形が meta-Enter でない");
    s.write(bytes);
    std::thread::sleep(Duration::from_secs(4));
    pump(&mut s, &mut rx);
    show("ESC CR 送出後", &mut s);

    // 2 行目を打つ。改行できていれば AAA と BBB が**両方入力欄に残る**
    s.write(b"BBB".to_vec());
    let got_bbb = wait_for(&mut s, &mut rx, "BBB", 150);
    std::thread::sleep(Duration::from_secs(3));
    pump(&mut s, &mut rx);
    show(
        "BBB 入力後（ここで AAA と BBB が別行で残っていれば成功）",
        &mut s,
    );
    assert!(got_bbb, "BBB が入力欄に出ない（打鍵が届いていない）");

    // 成功条件: AAA と BBB が**別々の行**で入力欄に残っている
    // （同じ行 = 改行できていない / どちらか欠け = ESC でクリア or 送信された）
    let lines = s.visible_lines();
    let aaa = lines.iter().position(|l| l.contains("AAA"));
    let bbb = lines.iter().position(|l| l.contains("BBB"));
    eprintln!("AAA の行={aaa:?} / BBB の行={bbb:?}");
    match (aaa, bbb) {
        (Some(a), Some(b)) if a != b => {
            eprintln!("結果: ESC CR は「送信せず改行」として解釈された（別行に残った）")
        }
        (Some(a), Some(b)) => panic!("AAA と BBB が同じ行（{a}）＝ 改行できていない"),
        _ => panic!("AAA / BBB が揃っていない（ESC が入力欄をクリアした / 送信された疑い）"),
    }

    drop(s);
    let _ = backend.kill(&name);
    let _ = Command::new(&bin)
        .args(["-L", &socket, "kill-server"])
        .output();
}
