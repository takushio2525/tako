//! 同一バイナリの A/B: `TAKO_1283_LEGACY=1` で **#1283 の「飛べない」が再現する**
//!
//! #1283 の真因は「トークンの区切りが ASCII の空白と `()[]{}<>,;` だけなので、
//! **日本語の地の文にパスが埋まると文ごと 1 トークン**になり、実在チェックで
//! 落ちて 1 つもリンクにならない」。この env で修正前（トークン全体だけを試す）へ戻る。
//!
//! env はプロセス全体の状態でしかも一度しか読まない（`OnceLock`）ので、
//! 他のテストと混ざらないよう**専用の統合テストバイナリ**に置き、
//! さらに **1 本のテスト関数**の中で見る。

use tako_core::links::{self, LinkKind};

/// master のペインに出ていた形（#927 に沿ってホームは temp の仮ホーム）
const FORM_BACKQUOTED: &str = "動画は`~/Desktop/tako-promo/tako-explainer-v4.mp4`、確認して。";
const FORM_CJK_AFTER: &str =
    "> ~/Desktop/tako-promo/tako-explainer-v4.mp4このリンクが CMD＋クリックで飛べない";
const FORM_BARE: &str = "~/Desktop/tako-promo/tako-explainer-v4.mp4";

fn targets_in(line: &str, home: &std::path::Path, legacy: bool) -> Vec<String> {
    links::detect_in_lines_in(&[line.to_string()], 120, None, Some(home), legacy)
        .into_iter()
        .filter(|l| l.kind == LinkKind::Path)
        .map(|l| l.target)
        .collect()
}

/// legacy 側は**公開関数**（env を読む形）で確かめる
fn targets(line: &str, home: &std::path::Path) -> Vec<String> {
    links::detect_in_lines(&[line.to_string()], 120, None, Some(home))
        .into_iter()
        .filter(|l| l.kind == LinkKind::Path)
        .map(|l| l.target)
        .collect()
}

#[test]
fn legacyで地の文に埋まったパスが飛べないことが再現する() {
    let home = std::env::temp_dir().join(format!("tako_links_1283_ab_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join("Desktop/tako-promo")).unwrap();
    std::fs::write(home.join("Desktop/tako-promo/tako-explainer-v4.mp4"), "").unwrap();

    // --- 既定（#1283 の修正あり）: 3 形すべて検出できる ---
    for (label, line) in [
        ("バッククォート囲み", FORM_BACKQUOTED),
        ("CJK 直後", FORM_CJK_AFTER),
        ("素", FORM_BARE),
    ] {
        let got = targets_in(line, &home, false);
        assert_eq!(got.len(), 1, "既定で {label} が検出されない: {got:?}");
        eprintln!("[1283-ab] 既定 {label}: {got:?}");
    }

    // --- legacy: トークン全体だけを試す = 地の文が食い込んだ 2 形が落ちる ---
    std::env::set_var("TAKO_1283_LEGACY", "1");
    assert!(links::legacy_1283(), "env が読まれていない");
    for (label, line) in [
        ("バッククォート囲み", FORM_BACKQUOTED),
        ("CJK 直後", FORM_CJK_AFTER),
    ] {
        let got = targets(line, &home);
        assert!(
            got.is_empty(),
            "legacy なのに {label} が検出できている（A/B が成立していない）: {got:?}"
        );
        eprintln!("[1283-ab] legacy {label}: {got:?}（= cmd+クリックで飛べない）");
    }
    // 素の形は修正前でも飛べた（症状の切り分けの根拠）
    let bare = targets(FORM_BARE, &home);
    assert_eq!(bare.len(), 1, "legacy でも素の形は飛べる: {bare:?}");
    eprintln!("[1283-ab] legacy 素: {bare:?}（修正前から飛べた形）");

    let _ = std::fs::remove_dir_all(&home);
}
