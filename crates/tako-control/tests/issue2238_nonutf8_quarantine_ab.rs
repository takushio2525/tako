//! 同一バイナリの A/B: `TAKO_2238_LEGACY=1` で **#2238 の症状**が再現する
//!
//! UTF-8 でない `settings.json` / `run-configs.json` は文字列の退避口を通れず、退避されずに既定値へ
//! 落ちたあと**保存すると元の中身が上書きされて消えていた**。`claude-mod/ui.json` は UTF-8 でない
//! 中身を 1 本目が空いているときだけ写していたので、**別の中身で 2 回目に壊れると変更の操作が
//! 消していた**（persist.log には 1 本目を退避先と偽った行が残った）。修正後は 3 つとも recent /
//! shortcuts（#2231）と同じく、バイトのまま番号付きの退避へ移してから既定値で書き直す
//! （`LoadIssue::read_failed` = `migration::quarantine_raw`）。
//!
//! 製品が呼ぶのと同じ口を叩く: settings は CLI `tako telemetry off` と同じ `settings::load` →
//! 書き換え → `settings::save`、run-configs は CLI `tako run-config` / MCP `tako_run_defaults` の
//! 本体 `run_configs::handle`、ui.json は CLI `tako mod ui set` / MCP `tako_mod` の本体
//! `claude_mod_ui::run`。最後に起動時の移行（`tako migrate run` と同じ `report_json("run")`）を先に
//! 通した呼び順でも、退避が 1 本だけ（二重に移さない）であることを見る。
//!
//! env はプロセス全体の状態なので、他のテストと混ざらないよう**専用の統合テストバイナリ**に
//! 置き、**1 本のテスト関数**の中で既定 → legacy の順に見る（`TAKO_DATA_DIR` も同じ理由）。
//! 各段の判定は `--nocapture` で `[2238-ab]` の行として読める。

use std::path::{Path, PathBuf};

/// UTF-8 でない中身（UTF-16 の BOM で始まる・途中に 0xff）。目印の ASCII を含める
const BYTES_1: &[u8] = b"\xff\xfe{\"theme\": \"light\"} MARK_2238_ONE \xff";
/// 1 回目と別の中身（2 回目の破損）
const BYTES_2: &[u8] = b"\xfe\xff MARK_2238_TWO \x80\x81";

fn data_dir(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("tako-2238-ab-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("claude-mod")).expect("一時 data dir を作れる");
    // run-config のプロジェクトのルートをここで止める（上の実 HOME の印を拾わない）
    std::fs::create_dir_all(base.join("proj").join(".git")).expect("作れる");
    let dir = tako_core::platform::path::canonicalize(&base).expect("正規化できる");
    assert!(
        dir.starts_with(tako_core::platform::path::canonicalize(&std::env::temp_dir()).unwrap()),
        "一時 dir の中だけ"
    );
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

/// persist.log のうち `head` を含み、退避先を書いた行
fn stash_lines(dir: &Path, head: &str) -> Vec<String> {
    persist_log(dir)
        .lines()
        .filter(|l| l.contains(head) && l.contains("・退避 "))
        .map(|l| {
            l.replace(&dir.display().to_string(), "<tmp>")
                .split_once("] ")
                .map_or(l.to_string(), |(_, rest)| rest.to_string())
        })
        .collect()
}

/// CLI `tako telemetry off` と同じ手順（読み → 書き換え → 保存）
fn telemetry_off() {
    let mut settings = tako_control::settings::load();
    settings.telemetry = false;
    tako_control::settings::save(&settings).expect("保存できる");
}

/// CLI `tako run-config <file> --args …` / MCP `tako_run_defaults` の本体で引数を保存する
fn run_config_args(file: &Path, args: &str) -> serde_json::Value {
    let mut a = tako_control::run_configs::PathArgs::empty(file);
    a.args = Some(args);
    tako_control::run_configs::handle(a).expect("壊れていても保存できる")
}

/// CLI `tako mod ui set colors.dim text` / MCP `tako_mod` の `action=ui` の本体
fn mod_ui_set(path: &Path) -> serde_json::Value {
    let request = tako_core::claude_mod_ui::UiRequest {
        op: Some("set".into()),
        key: Some("colors.dim".into()),
        value: Some(serde_json::json!("text")),
        ..Default::default()
    };
    tako_control::claude_mod_ui::run(path, &request).expect("壊れていても変えられる")
}

/// 1 つのファイルを 2 回（別の中身で）壊し、そのつど `act`（読み → 保存）を走らせて確かめる。
/// 満たせなかった約束を `failed` へ積む
fn two_breaks(
    failed: &mut Vec<String>,
    label: &str,
    dir: &Path,
    name: &str,
    head: &str,
    act: &dyn Fn(),
) {
    let file = dir.join(name);
    let parent = file.parent().expect("親がある").to_path_buf();
    let base = file.file_name().unwrap().to_string_lossy().into_owned();
    for bytes in [BYTES_1, BYTES_2] {
        std::fs::write(&file, bytes).expect("書ける");
        act();
    }
    for (k, bytes) in [(1, BYTES_1), (2, BYTES_2)] {
        let held = stashed(&parent, &base, bytes);
        eprintln!("[2238-ab] {label} の {k} 回目の中身: {held:?}");
        if held.is_empty() {
            failed.push(format!("{label} の {k} 回目の中身が失われた"));
        }
    }
    let lines = stash_lines(dir, head);
    eprintln!("[2238-ab] {label} の記録: {lines:?}");
    // 2 回目の行は 2 回目の中身の置き場を名指す（修正前の ui.json は 1 本目を偽って書いた）
    let second = stashed(&parent, &base, BYTES_2);
    if lines.len() != 2 || !second.iter().any(|s| lines[1].contains(s.as_str())) {
        failed.push(format!(
            "{label} の退避先が persist.log に 2 行（2 行目が 2 回目の置き場）残らない"
        ));
    }
    // 同じ中身でもう一度壊す = 退避は積まない（本数も記録も増えない）
    let before = (slots(&parent, &base), lines.len());
    std::fs::write(&file, BYTES_2).expect("書ける");
    act();
    let after = (slots(&parent, &base), stash_lines(dir, head).len());
    eprintln!("[2238-ab] {label} の同じ中身の 3 回目: 退避 / 記録 {before:?} → {after:?}");
    if after != before {
        failed.push(format!(
            "{label} の同じ中身の破損で退避か記録が増えた（{before:?} → {after:?}）"
        ));
    }
}

/// 1 つの data dir で症状を確かめ、**満たせなかった約束**を返す（空 = 修正後の挙動）
fn scenario(dir: &Path) -> Vec<String> {
    std::env::set_var("TAKO_DATA_DIR", dir);
    assert_eq!(tako_core::paths::data_dir().as_deref(), Some(dir));
    let mut failed = Vec::new();

    // --- settings.json: `tako telemetry off`（読み → 書き換え → 保存） ---
    two_breaks(
        &mut failed,
        "settings.json",
        dir,
        "settings.json",
        "settings.json を読めないので既定値で動く",
        &telemetry_off,
    );
    let saved = std::fs::read_to_string(dir.join("settings.json")).expect("保存後は読める");
    assert!(
        saved.contains("\"telemetry\": false"),
        "既定値 + 書き換えが保存される: {saved}"
    );

    // --- run-configs.json: `tako run-config <file> --args`（読み → 足す → 保存） ---
    let file = dir.join("proj").join("a.sh");
    std::fs::write(&file, "echo hi\n").expect("書ける");
    let key = file.display().to_string();
    two_breaks(
        &mut failed,
        "run-configs.json",
        dir,
        "run-configs.json",
        "移行できず: run_configs ",
        &|| {
            let out = run_config_args(&file, "--flag");
            assert_eq!(out["saved"]["file"]["args"], "--flag", "{out}");
        },
    );
    let saved = tako_control::run_configs::load();
    assert_eq!(saved.files[&key].args.as_deref(), Some("--flag"));

    // --- claude-mod/ui.json: `tako mod ui set`（読み → 変える → 保存） ---
    let ui = tako_control::claude_mod_ui::default_path().expect("置き場が決まる");
    assert_eq!(ui, dir.join("claude-mod").join("ui.json"));
    two_breaks(
        &mut failed,
        "ui.json",
        dir,
        "claude-mod/ui.json",
        "claude-mod/ui.json の読めない部分を既定で動かす",
        &|| {
            let out = mod_ui_set(&ui);
            assert_eq!(out["written"], true, "{out}");
        },
    );

    let log = persist_log(dir);
    assert!(
        !log.contains("MARK_2238"),
        "persist.log にファイルの中身を載せない:\n{log}"
    );
    failed
}

/// 起動時の移行（`tako migrate run` / GUI 起動の `ensure_migrated` と同じ本体）を先に通した
/// 呼び順でも、退避は 1 本だけ（移行は文字として読めない中身を退避しない = 二重に移さない）
fn migration_first(dir: &Path) {
    std::env::set_var("TAKO_DATA_DIR", dir);
    let ui = dir.join("claude-mod").join("ui.json");
    for name in ["settings.json", "run-configs.json"] {
        std::fs::write(dir.join(name), BYTES_1).expect("書ける");
    }
    std::fs::write(&ui, BYTES_1).expect("書ける");
    // 種別ごとに絞る（全種別だと data dir の外 = Claude Code の設定 dir の印まで読む）
    for schema in ["settings", "run_configs", "claude_mod_ui"] {
        let report =
            tako_control::migrations::report_json("run", Some(schema)).expect("移行が走る");
        for file in report["files"].as_array().expect("files") {
            eprintln!(
                "[2238-ab] 移行を先に: {schema} → {}（{}）",
                file["state"],
                file["reason"].as_str().unwrap_or_default()
            );
        }
    }
    telemetry_off();
    let file = dir.join("proj").join("a.sh");
    std::fs::write(&file, "echo hi\n").expect("書ける");
    run_config_args(&file, "--flag");
    mod_ui_set(&ui);
    for (parent, name) in [
        (dir.to_path_buf(), "settings.json"),
        (dir.to_path_buf(), "run-configs.json"),
        (dir.join("claude-mod"), "ui.json"),
    ] {
        let n = slots(&parent, name);
        eprintln!("[2238-ab] 移行を先に: {name} の退避 {n} 本");
        assert_eq!(n, 1, "{name}: 移行と読み手で二重に退避しない");
        assert_eq!(
            stashed(&parent, name, BYTES_1).len(),
            1,
            "{name}: 中身が残る"
        );
    }
    let log = persist_log(dir);
    for line in log.lines() {
        let line = line.replace(&dir.display().to_string(), "<tmp>");
        if line.contains("settings") || line.contains("run_configs") || line.contains("ui.json") {
            eprintln!("[2238-ab] 移行を先に: {line}");
        }
    }
}

#[test]
fn utf8でない設定とデータは保存しても退避に残る() {
    std::env::remove_var("TAKO_2238_LEGACY");
    std::env::remove_var("TAKO_2231_LEGACY");
    std::env::remove_var("TAKO_1824_LEGACY");
    let fixed_dir = data_dir("fixed");
    let fixed = scenario(&fixed_dir);
    eprintln!("[2238-ab] 修正後: 満たせなかった約束 {fixed:?}");
    assert!(fixed.is_empty(), "修正後はすべて残る: {fixed:?}");

    let order_dir = data_dir("migrate-first");
    migration_first(&order_dir);

    std::env::set_var("TAKO_2238_LEGACY", "1");
    let legacy_dir = data_dir("legacy");
    let legacy = scenario(&legacy_dir);
    std::env::remove_var("TAKO_2238_LEGACY");
    eprintln!("[2238-ab] 修正前（TAKO_2238_LEGACY=1）: 満たせなかった約束 {legacy:?}");
    for want in [
        "settings.json の 1 回目の中身が失われた",
        "settings.json の 2 回目の中身が失われた",
        "run-configs.json の 1 回目の中身が失われた",
        "run-configs.json の 2 回目の中身が失われた",
        "ui.json の 2 回目の中身が失われた",
    ] {
        assert!(
            legacy.iter().any(|f| f == want),
            "legacy で #2238 の症状（{want}）が再現しない: {legacy:?}"
        );
    }
    assert!(
        !legacy.iter().any(|f| f == "ui.json の 1 回目の中身が失われた"),
        "修正前の ui.json も 1 回目は 1 本目へ写していた（A/B が修正前を忠実に再現する）: {legacy:?}"
    );

    for dir in [fixed_dir, order_dir, legacy_dir] {
        assert!(dir
            .starts_with(tako_core::platform::path::canonicalize(&std::env::temp_dir()).unwrap()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
