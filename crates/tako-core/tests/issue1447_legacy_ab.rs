//! 同一バイナリの A/B: `TAKO_1447_LEGACY=1` で **#1447 の不検知が再現する**
//!
//! Issue の画面（preview パネルつき AskUserQuestion）で、修正前の検知器は
//! `detect_choice_list` が `None` を返していた（= `choice_dialog: null` /
//! watch が `WORKER_DIALOG` を出せない / respond が「画面に存在しない」）。
//!
//! この env は `OnceLock` で**プロセス起動時に 1 度だけ**読む（他の dialog 系の
//! legacy スイッチと同じ）ので、1 プロセスでは片腕しか観られない。そこで
//! **既定の腕は in-process・legacy の腕は自分自身を子プロセスで起動**して観る。
//! 子は同じテスト関数へ入り、env が立っているので legacy 側の検査だけを行う。

use std::process::Command;

const LEGACY_ENV: &str = "TAKO_1447_LEGACY";
const TEST_NAME: &str = "legacyでpreviewつきダイアログが非検知に戻る";

const PREVIEW_ASK_60: &str = include_str!("fixtures/issue1447-preview-ask-60.txt");

fn rows(s: &str) -> Vec<&str> {
    s.lines().collect()
}

#[test]
fn legacyでpreviewつきダイアログが非検知に戻る() {
    let screen = rows(PREVIEW_ASK_60);

    if std::env::var_os(LEGACY_ENV).is_some() {
        // --- legacy（#1447 前）: Issue の症状そのもの ---
        let got = tako_core::dialog::detect_choice_list(&screen);
        assert!(
            got.is_none(),
            "legacy で検知できてしまう（A/B の入口が効いていない）: {got:?}"
        );
        assert!(
            !tako_core::dialog::is_choice_dialog(&screen),
            "legacy で `is_choice_dialog` が真（respond の実在検査が通ってしまう）"
        );
        eprintln!("[1447-ab] legacy: detect_choice_list -> None（#1447 の症状を再現）");
        return;
    }

    // --- 既定（#1447 修正後）: 3 択・番号つき・ハイライト 1 番目 ---
    let list = tako_core::dialog::detect_choice_list(&screen).expect("既定では検知される");
    assert_eq!(list.options.len(), 3);
    assert!(list.numbered);
    assert_eq!(list.highlighted, Some(0));
    eprintln!(
        "[1447-ab] 既定: detect_choice_list -> 3 択 {:?}",
        list.options
            .iter()
            .map(|o| o.label.clone())
            .collect::<Vec<_>>()
    );

    // --- legacy の腕は子プロセスで（env は起動時にしか読めない） ---
    let exe = std::env::current_exe().expect("テストバイナリのパスが取れる");
    let out = Command::new(&exe)
        .args(["--exact", TEST_NAME, "--nocapture", "--test-threads", "1"])
        .env(LEGACY_ENV, "1")
        .output()
        .expect("自分自身を legacy で起動できる");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "legacy の腕が落ちた（修正前の症状を再現できていない）:\n{text}"
    );
    assert!(
        text.contains("[1447-ab] legacy: detect_choice_list -> None"),
        "legacy の腕が走っていない:\n{text}"
    );
    eprintln!("[1447-ab] 子プロセス（legacy）も検査済み");
}
