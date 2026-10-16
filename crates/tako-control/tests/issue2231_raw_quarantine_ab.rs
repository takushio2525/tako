//! 同一バイナリの A/B: `TAKO_2231_LEGACY=1` で **#2231 の症状**が再現する
//!
//! UTF-8 でない `recent.json` / `remote/shortcuts.json` は文字列の退避口
//! （`migration::quarantine_unreadable`）を通れず、既定値へ落ちたあと**項目を足して保存すると
//! 元の中身が上書きされて消えていた**。修正後はバイトのまま番号付きの退避へ移してから既定値で
//! 書き直す（layout.json と同じ扱い = `migration::quarantine_raw`）。
//!
//! 製品が呼ぶのと同じ口を叩く: recent は GUI / dispatch の `OpenDir` と同じ
//! `data_files::load_recent` → `push` → `save`、shortcuts は CLI `tako remote shortcuts add` と
//! MCP の本体 `dispatch::dispatch_remote_shortcuts("add", …)`。
//!
//! env はプロセス全体の状態なので、他のテストと混ざらないよう**専用の統合テストバイナリ**に
//! 置き、**1 本のテスト関数**の中で既定 → legacy の順に見る（`TAKO_DATA_DIR` も同じ理由）。
//! 各段の判定は `--nocapture` で `[2231-ab]` の行として読める。

use std::path::{Path, PathBuf};

use tako_core::recent::RecentEntry;

/// UTF-8 でない中身（UTF-16 の BOM で始まる・途中に 0xff）。目印の ASCII を含める
const BYTES_1: &[u8] = b"\xff\xfe{\"entries\": [] MARK_2231_ONE \xff}";
/// 1 回目と別の中身（2 回目の破損）
const BYTES_2: &[u8] = b"\xfe\xff MARK_2231_TWO \x80\x81";

fn data_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tako-2231-ab-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("remote")).expect("一時 data dir を作れる");
    assert!(dir.starts_with(std::env::temp_dir()), "一時 dir の中だけ");
    dir
}

/// `dir` 直下で `name` の退避（`<name>.unreadable[.<n>].bak`）のうち、中身が `bytes` と一致するもの
fn stashed(dir: &Path, name: &str, bytes: &[u8]) -> Vec<String> {
    let prefix = format!("{name}.unreadable");
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .expect("読める")
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
        .filter(|e| std::fs::read(e.path()).is_ok_and(|b| b == bytes))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

/// `dir` 直下の `name` の退避の本数
fn slots(dir: &Path, name: &str) -> usize {
    let prefix = format!("{name}.unreadable");
    std::fs::read_dir(dir)
        .expect("読める")
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
        .count()
}

fn persist_log(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("persist.log")).unwrap_or_default()
}

/// GUI / dispatch の `OpenDir` と同じ手順で 1 件足して保存する
fn recent_add(path: &str) {
    let mut recent = tako_control::data_files::load_recent();
    recent.push(RecentEntry::Directory {
        path: path.to_string(),
    });
    recent.save();
}

fn recent_paths(dir: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(dir.join("recent.json")).expect("保存後は文字として読める");
    let list: tako_core::recent::RecentList = serde_json::from_str(&text).expect("JSON");
    list.entries.iter().map(|e| e.label().to_string()).collect()
}

/// 1 つの data dir で症状を確かめ、**満たせなかった約束**を返す（空 = 修正後の挙動）
fn scenario(dir: &Path) -> Vec<String> {
    std::env::set_var("TAKO_DATA_DIR", dir);
    assert_eq!(tako_core::paths::data_dir().as_deref(), Some(dir));
    let mut failed = Vec::new();

    // --- recent.json: 読み → 追加 → 保存 を 2 回（中身を変えて壊す） ---
    std::fs::write(dir.join("recent.json"), BYTES_1).expect("書ける");
    recent_add("/a");
    std::fs::write(dir.join("recent.json"), BYTES_2).expect("書ける");
    recent_add("/b");
    // 保存した結果は正しい（既定値 + 足した 1 件）
    assert_eq!(
        recent_paths(dir),
        vec!["/b".to_string()],
        "壊れていれば既定値（空）へ落ちてから足す"
    );
    for (k, bytes) in [(1, BYTES_1), (2, BYTES_2)] {
        let held = stashed(dir, "recent.json", bytes);
        eprintln!("[2231-ab] recent.json の {k} 回目の中身: {held:?}");
        if held.is_empty() {
            failed.push(format!("recent.json の {k} 回目の中身が失われた"));
        }
    }
    let log = persist_log(dir);
    let lines: Vec<&str> = log
        .lines()
        .filter(|l| l.contains("recent ") && l.contains("発生源 load）"))
        .collect();
    eprintln!("[2231-ab] recent.json の記録: {lines:?}");
    if lines.iter().filter(|l| l.contains("・退避 ")).count() != 2 {
        failed.push("recent.json の退避先が persist.log に 2 行残らない".into());
    }
    assert!(
        !log.contains("MARK_2231"),
        "persist.log にファイルの中身を載せない:\n{log}"
    );

    // 同じ中身でもう一度壊す = 退避は積まない（本数も記録も増えない）
    let before = (slots(dir, "recent.json"), lines.len());
    std::fs::write(dir.join("recent.json"), BYTES_2).expect("書ける");
    recent_add("/c");
    let log = persist_log(dir);
    let after = (
        slots(dir, "recent.json"),
        log.lines()
            .filter(|l| l.contains("recent ") && l.contains("発生源 load）"))
            .count(),
    );
    eprintln!("[2231-ab] 同じ中身の 3 回目: 退避 / 記録 {before:?} → {after:?}");
    if after != before {
        failed.push(format!(
            "同じ中身の破損で退避か記録が増えた（{before:?} → {after:?}）"
        ));
    }

    // --- remote/shortcuts.json: CLI / MCP の本体（dispatch）で足して保存する ---
    let remote = dir.join("remote");
    let target = dir.join("target-dir");
    std::fs::create_dir_all(&target).expect("作れる");
    std::fs::write(remote.join("shortcuts.json"), BYTES_1).expect("書ける");
    let added = tako_control::dispatch::dispatch_remote_shortcuts(
        "add",
        Some(&target.to_string_lossy()),
        None,
    )
    .expect("壊れていても足せる");
    assert!(added["added"].is_object(), "{added}");
    let held = stashed(&remote, "shortcuts.json", BYTES_1);
    eprintln!("[2231-ab] remote/shortcuts.json の中身: {held:?}");
    if held.is_empty() {
        failed.push("remote/shortcuts.json の中身が失われた".into());
    }
    let saved = std::fs::read_to_string(remote.join("shortcuts.json")).expect("保存後は読める");
    assert!(
        saved.contains("target-dir"),
        "足した 1 件が保存される: {saved}"
    );

    std::env::remove_var("TAKO_DATA_DIR");
    failed
}

#[test]
fn legacyで足して保存すると元の中身が消える() {
    // --- 既定（修正後）: 約束をすべて満たす ---
    std::env::remove_var("TAKO_2231_LEGACY");
    let fixed = data_dir("fixed");
    let after_fix = scenario(&fixed);
    assert!(
        after_fix.is_empty(),
        "修正後なのに満たせない: {after_fix:?}"
    );
    eprintln!("[2231-ab] 既定: PASS");

    // --- legacy（修正前）: 足して保存すると元の中身がどこにも残らない ---
    std::env::set_var("TAKO_2231_LEGACY", "1");
    let legacy = data_dir("legacy");
    let before_fix = scenario(&legacy);
    std::env::remove_var("TAKO_2231_LEGACY");
    for f in &before_fix {
        eprintln!("[2231-ab] legacy: FAIL {f}");
    }
    assert_eq!(
        before_fix,
        vec![
            "recent.json の 1 回目の中身が失われた".to_string(),
            "recent.json の 2 回目の中身が失われた".to_string(),
            "recent.json の退避先が persist.log に 2 行残らない".to_string(),
            "remote/shortcuts.json の中身が失われた".to_string(),
        ],
        "TAKO_2231_LEGACY=1 で修正前の症状が再現しない（A/B が取れない）"
    );

    let _ = std::fs::remove_dir_all(&fixed);
    let _ = std::fs::remove_dir_all(&legacy);
}
