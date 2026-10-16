//! 同一バイナリの A/B: `TAKO_1824_LEGACY=1` で **#1824 の 2 つの症状**が再現する
//!
//! 1. `recent.json` / `remote/shortcuts.json` の破損が persist.log に 1 行も残らない
//!    （中身は core の読み手が退避するが、「いつ壊れていたか」を追えない）
//! 2. 壊れた layout.json を 2 回続けて開くと、1 回目の中身が失われる
//!    （`layout.json.corrupt` への rename が前回分を上書きする）
//!
//! どちらも製品が呼ぶのと同じ口を叩く: 読み手は `data_files::load_recent` /
//! `dispatch::dispatch_remote_shortcuts`（CLI の `tako remote shortcuts` と MCP の本体）、
//! layout は GUI の main と同じ `layout::try_load` → `layout::stash_after_failed_restore`。
//! **起動時の移行（`migrations::ensure_migrated`）は呼ばない** = 退避が呼び出し順に
//! 頼っていないことを、移行を先に通さない呼び順で確かめる（受け入れ条件 2）。
//!
//! env はプロセス全体の状態なので、他のテストと混ざらないよう**専用の統合テストバイナリ**に
//! 置き、**1 本のテスト関数**の中で既定 → legacy の順に見る（`TAKO_DATA_DIR` も同じ理由）。
//! 各段の判定は `--nocapture` で `[1824-ab]` の行として読める。

use std::path::{Path, PathBuf};

fn data_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tako-1824-ab-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("remote")).expect("一時 data dir を作れる");
    assert!(dir.starts_with(std::env::temp_dir()), "一時 dir の中だけ");
    dir
}

fn persist_log(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("persist.log")).unwrap_or_default()
}

fn count(log: &str, needle: &str) -> usize {
    log.lines().filter(|l| l.contains(needle)).count()
}

/// `dir` 直下で `name` から始まるファイルのうち、`marker` を含むもの
fn holding(dir: &Path, name: &str, marker: &str) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .expect("読める")
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(name))
        .filter(|e| {
            std::fs::read_to_string(e.path())
                .map(|t| t.contains(marker))
                .unwrap_or(false)
        })
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

/// 1 つの data dir で症状を確かめ、**満たせなかった約束**を返す（空 = 修正後の挙動）
fn scenario(dir: &Path) -> Vec<String> {
    std::env::set_var("TAKO_DATA_DIR", dir);
    assert_eq!(tako_core::paths::data_dir().as_deref(), Some(dir));
    let mut failed = Vec::new();

    // --- ① recent.json: 壊れた中身ごとに 1 行 ---
    std::fs::write(dir.join("recent.json"), "[MARK_RECENT_1").expect("書ける");
    let list = tako_control::data_files::load_recent();
    assert!(list.entries.is_empty(), "壊れていれば空で動く");
    let _ = tako_control::data_files::load_recent(); // 同じ中身の読み直しは増やさない
    let log = persist_log(dir);
    let n = count(&log, "移行できず: recent ");
    eprintln!("[1824-ab] recent.json の破損の記録 {n} 行");
    if n != 1 {
        failed.push(format!(
            "recent.json の破損が persist.log に 1 行残らない（{n} 行）"
        ));
    }
    if holding(dir, "recent.json.unreadable", "MARK_RECENT_1").is_empty() {
        failed.push("recent.json の壊れた中身が退避に残らない".into());
    }

    // --- ① remote/shortcuts.json: CLI / MCP の本体（dispatch）から読む ---
    std::fs::write(
        dir.join("remote/shortcuts.json"),
        "{ \"shortcuts\": [ \"MARK_SHORTCUT_1\"",
    )
    .expect("書ける");
    let listed = tako_control::dispatch::dispatch_remote_shortcuts("list", None, None)
        .expect("壊れていても一覧は返る");
    assert!(listed["shortcuts"].is_array());
    let log = persist_log(dir);
    let n = count(&log, "移行できず: remote_shortcuts ");
    eprintln!("[1824-ab] remote/shortcuts.json の破損の記録 {n} 行");
    if n != 1 {
        failed.push(format!(
            "remote/shortcuts.json の破損が persist.log に 1 行残らない（{n} 行）"
        ));
    }
    assert!(
        !log.contains("MARK_RECENT") && !log.contains("MARK_SHORTCUT"),
        "persist.log にファイルの中身を載せない:\n{log}"
    );

    // --- ② layout.json を中身を変えて 2 回壊し、そのたびに開く（GUI の main と同じ手順） ---
    for k in 1..=2 {
        std::fs::write(
            dir.join("layout.json"),
            format!("{{ \"version\": 1, \"tabs\": [ \"MARK_LAYOUT_{k}\"\n"),
        )
        .expect("書ける");
        let e = tako_control::layout::try_load().expect_err("壊れているので復元できない");
        let msg = tako_control::layout::stash_after_failed_restore(&e);
        eprintln!("[1824-ab] {k} 回目: {msg}");
        assert!(
            !msg.contains(&dir.display().to_string()),
            "テレメトリへ出る行に data dir のパスを載せない: {msg}"
        );
    }
    for k in 1..=2 {
        let held = holding(dir, "layout.json", &format!("MARK_LAYOUT_{k}"));
        eprintln!("[1824-ab] layout.json の {k} 回目の中身: {held:?}");
        if held.is_empty() {
            failed.push(format!("layout.json の {k} 回目の中身が失われた"));
        }
    }
    // 移行を先に通していない（= 退避は呼び出し順に頼っていない）ことを記録の側からも確かめる
    let log = persist_log(dir);
    assert_eq!(
        count(&log, "発生源 runtime"),
        0,
        "この A/B は ensure_migrated を通さない呼び順で見る:\n{log}"
    );
    failed
}

#[test]
fn legacyで記録の欠落と前回分の上書きが再現する() {
    // --- 既定（修正後）: 約束をすべて満たす ---
    std::env::remove_var("TAKO_1824_LEGACY");
    let fixed = data_dir("fixed");
    let after_fix = scenario(&fixed);
    assert!(
        after_fix.is_empty(),
        "修正後なのに満たせない: {after_fix:?}"
    );
    eprintln!("[1824-ab] 既定: PASS");

    // --- legacy（修正前）: 症状 1 と 2 がそのまま出る ---
    std::env::set_var("TAKO_1824_LEGACY", "1");
    let legacy = data_dir("legacy");
    let before_fix = scenario(&legacy);
    std::env::remove_var("TAKO_1824_LEGACY");
    for f in &before_fix {
        eprintln!("[1824-ab] legacy: FAIL {f}");
    }
    assert_eq!(
        before_fix,
        vec![
            "recent.json の破損が persist.log に 1 行残らない（0 行）".to_string(),
            "remote/shortcuts.json の破損が persist.log に 1 行残らない（0 行）".to_string(),
            "layout.json の 1 回目の中身が失われた".to_string(),
        ],
        "TAKO_1824_LEGACY=1 で修正前の症状が再現しない（A/B が取れない）"
    );
    // 修正前の退避先は前回分を上書きする `.corrupt` だった
    assert_eq!(
        holding(&legacy, "layout.json", "MARK_LAYOUT_2"),
        vec!["layout.json.corrupt".to_string()]
    );

    std::env::remove_var("TAKO_DATA_DIR");
    let _ = std::fs::remove_dir_all(&fixed);
    let _ = std::fs::remove_dir_all(&legacy);
}
