//! usage 分析 S1（#2200。エピック #2192）の番犬: 利用制限の観測とログインの鍵
//!
//! # なぜ要るか
//!
//! 観測の記録は**利用者の資格情報のファイル**（`.claude.json`）を読み、その値から鍵を作って
//! `<data_dir>/usage/` へ書き続ける。壊れ方はどれも応答の形を変えないので、挙動の単体テストだけでは
//! 気づけない:
//!
//! - 鍵を作る途中で accountUuid（やメール）を 1 つ欄に足すだけで、生の識別子がファイルに残り続ける
//! - 診断ログ（persist.log）に設定 dir・鍵を 1 つ混ぜるだけで、ホームパス・ログインが漏れる
//! - 報告の受け口（UI スレッドの dispatch）で鍵を引くと、報告のたびに数 MB の `.claude.json` を
//!   UI スレッドで読む（#1984 の系統の止まり）
//! - GUI の起動の段で `limits.jsonl` を読むと、記録が育つほど起動が遅くなる（設計書 §5.3 の「起動時は読まない」）
//!
//! # 何を縛るか
//!
//! 1. 実経路（`record_for_gui`）: 生の accountUuid・その断片・メール・設定 dir のパスが `usage/` にも
//!    persist.log にも出ない。同じ値の heartbeat で行が増えない。A/B（`TAKO_2192_S1_LEGACY=1`）では
//!    何も書かず置き場も作らない
//! 2. 永続する行の型（`LimitObservation` / `SessionLogin`）に、生の識別子・本文・パスに当たる欄が無い
//! 3. UI スレッド: 報告の受け口（`tako_control::claude_mod::accept_report` と `ModHub::accept`）は
//!    ファイルに触らず `tako_core::usage::pending_from_report` で積むだけ。tako-app から
//!    `tako_control::usage` を呼ぶのは定期処理の `drive_usage_record` の背景のブロックの中だけ
//!    （= 起動の段では読まない）。IPC / MCP の要約は `prepare_offload` が background のジョブにする
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`逆戻りを名指しできる`] で、現行ソースから作った注入（受け口で鍵を引く・UI スレッドで記録する）が
//! file:line で名指しされることを確かめる。

use std::path::{Path, PathBuf};

use tako_core::claude_mod::ModRateLimit;
use tako_core::usage::PendingUsage;

#[path = "common/detached_env.rs"]
mod detached_env;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

const CORE_USAGE: &str = "crates/tako-core/src/usage.rs";
const CORE_MOD: &str = "crates/tako-core/src/claude_mod.rs";
const CONTROL_MOD: &str = "crates/tako-control/src/claude_mod.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";

/// 偽の資格情報に入れる印（生の値がどこかに出たら名指す）
const UUID: &str = "c0ffee00-2200-4abc-8def-0123456789ab";
const EMAIL: &str = "usage-watchdog@example.com";
const BODY_MARK: &str = "usage-watchdog-body-marker";

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// `name` の関数本体（宣言の行から中括弧が閉じるまで）と開始行
fn fn_body<'a>(text: &'a str, head: &str) -> Option<(&'a str, usize)> {
    let start = text.find(head)?;
    let open = start + text[start..].find('{')?;
    let mut depth = 0usize;
    for (i, c) in text[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let line = text[..start].lines().count() + 1;
                    return Some((&text[start..open + i + 1], line));
                }
            }
            _ => {}
        }
    }
    None
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(files_under(&path));
            } else {
                out.push(path);
            }
        }
    }
    out
}

fn batch(config_dir: &Path, pct: f64, observed_at: u64) -> Vec<PendingUsage> {
    vec![PendingUsage {
        session_id: Some("watchdog-session".into()),
        config_dir: Some(config_dir.display().to_string()),
        limits: vec![ModRateLimit {
            kind: "seven_day".into(),
            percent_used: pct,
            resets_at: Some("2026-10-20T22:00:00.000Z".into()),
            observed_at,
        }],
    }]
}

fn lines(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .map(|t| t.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0)
}

/// 1: 実経路。**env と GUI の記録器（プロセスに 1 つ）を使うので、この 1 本の中で順に見る**
#[test]
fn 実経路で生の識別子も設定dirも出さず_heartbeatで増えず_ab_では書かない() {
    // 走らせた人の TAKO_DATA_DIR（worker の検証 env の `/tmp/…`）を持ち込まない子で走らせる（#2391）
    if detached_env::rerun_without(
        "実経路で生の識別子も設定dirも出さず_heartbeatで増えず_ab_では書かない",
        &["TAKO_DATA_DIR"],
    ) {
        return;
    }
    let data = tako_core::paths::data_dir().expect("テストの data dir");
    assert!(
        data.starts_with(std::env::temp_dir()),
        "テストの data dir が一時ディレクトリの外: 本番の置き場へ書く恐れ"
    );
    let usage = tako_control::usage::dir().unwrap();
    let scratch = tako_core::test_residue::ScratchDir::new("usage-watchdog");
    let cfg = scratch.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join(".claude.json"),
        format!(
            r#"{{"projects": {{"/x": {{"lastPrompt": "{BODY_MARK}"}}}},
                "oauthAccount": {{"accountUuid": "{UUID}", "emailAddress": "{EMAIL}"}}}}"#
        ),
    )
    .unwrap();

    // A/B: 旧挙動は何も書かない（置き場も作らない）
    std::env::set_var(tako_core::usage::S1_LEGACY_ENV, "1");
    tako_control::usage::record_for_gui(batch(&cfg, 10.0, 100));
    std::env::remove_var(tako_core::usage::S1_LEGACY_ENV);
    assert!(!usage.exists(), "A/B なのに {} を作った", usage.display());

    // 新挙動: 1 行ずつ書き、同じ値の heartbeat では増えない
    tako_control::usage::record_for_gui(batch(&cfg, 10.0, 100));
    let limits = usage.join(tako_control::usage::LIMITS_FILE);
    let sessions = usage.join(tako_control::usage::SESSIONS_FILE);
    assert_eq!(lines(&limits), 1, "観測が 1 行にならない");
    assert_eq!(lines(&sessions), 1, "会話の対応が 1 行にならない");
    for _ in 0..3 {
        tako_control::usage::record_for_gui(batch(&cfg, 10.0, 100));
    }
    assert_eq!(lines(&limits), 1, "同じ値の heartbeat で観測が増えた");
    tako_control::usage::record_for_gui(batch(&cfg, 11.0, 200));
    assert_eq!(lines(&limits), 2, "% が変わったのに増えない");

    // 生の識別子・本文・設定 dir がどのファイルにも出ない（usage/ と persist.log）
    let mut scanned = files_under(&usage);
    assert!(scanned.len() >= 2, "usage/ の走査が空振りしている");
    let persist = tako_control::diag::persist_log_path().unwrap();
    assert!(
        std::fs::read_to_string(&persist)
            .unwrap_or_default()
            .contains("usage: 利用制限の観測の記録を始めた"),
        "記録の開始が persist.log に無い（走査が空振りする）"
    );
    scanned.push(persist);
    let cfg_text = cfg.display().to_string();
    let needles = [
        UUID,
        // uuid の先頭と末尾の塊（断片だけが出ても名指す）
        UUID.split('-').next().unwrap(),
        UUID.rsplit('-').next().unwrap(),
        EMAIL,
        BODY_MARK,
        cfg_text.as_str(),
    ];
    for file in &scanned {
        let text = std::fs::read_to_string(file).unwrap_or_default();
        for needle in needles {
            assert!(
                !text.contains(needle),
                "{} に {needle:?} が出た（生の識別子・本文・設定 dir を書かない = #2200 / 設計書 §6）",
                file.display()
            );
        }
    }
    // 鍵は 12 桁の 16 進で、診断ログには出さない
    let row: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(&limits)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let key = row["login"].as_str().unwrap().to_string();
    assert_eq!(key.len(), tako_core::usage::KEY_HEX_LEN);
    assert_eq!(row["key_from"], "account");
    let log = std::fs::read_to_string(tako_control::diag::persist_log_path().unwrap()).unwrap();
    assert!(!log.contains(&key), "鍵が persist.log に出た");

    // 要約も生の値を出さない
    let summary = tako_control::usage::summary_payload().unwrap().to_string();
    assert!(summary.contains(&key));
    for needle in needles {
        assert!(!summary.contains(needle), "要約に {needle:?} が出た");
    }
}

/// 2: 永続する行の型に、生の識別子・本文・パスに当たる欄が無い
#[test]
fn 永続する行の型に生の識別子や本文の欄が無い() {
    const DENY: &[&str] = &[
        "uuid", "email", "account", "content", "text", "prompt", "message", "cwd", "label", "path",
        "dir", "home",
    ];
    let text = read(CORE_USAGE);
    let mut seen = 0;
    for name in ["pub struct LimitObservation", "pub struct SessionLogin"] {
        let (body, line) = fn_body(&text, name).unwrap_or_else(|| panic!("{name} が無い"));
        for (i, l) in body.lines().enumerate() {
            let l = l.trim();
            let Some(field) = l.strip_prefix("pub ").and_then(|r| r.split(':').next()) else {
                continue;
            };
            seen += 1;
            for deny in DENY {
                assert!(
                    !field.contains(deny),
                    "{CORE_USAGE}:{} の {name} に {field}（{deny}）がある: 生の識別子・本文・パスを\
                     usage/ へ書かない（#2200 / 設計書 §6）",
                    line + i
                );
            }
        }
    }
    assert!(seen >= 10, "欄の走査が空振りしている（{seen} 個）");
}

/// UI スレッドの決まりの違反を集める（[`逆戻りを名指しできる`] が同じ関数で注入を見る）
fn ui_violations(core_mod: &str, control_mod: &str, main: &str, dispatch: &str) -> Vec<String> {
    let mut out = Vec::new();
    // 報告の受け口（dispatch = UI スレッド）はファイルに触らず、鍵も引かない
    match fn_body(control_mod, "fn accept_report(") {
        Some((body, line)) => {
            for (i, l) in body.lines().enumerate() {
                if l.contains("crate::usage::") || l.contains("std::fs::") {
                    out.push(format!(
                        "{CONTROL_MOD}:{}: 報告の受け口（UI スレッド）で usage のファイルに触っている: {}",
                        line + i,
                        l.trim()
                    ));
                }
            }
        }
        None => out.push(format!("{CONTROL_MOD}: accept_report が見つからない")),
    }
    // ModHub::accept は比べて積むだけ
    match fn_body(core_mod, "pub fn accept(&mut self") {
        Some((body, line)) => {
            if !body.contains("crate::usage::pending_from_report(") {
                out.push(format!(
                    "{CORE_MOD}:{line}: ModHub::accept が pending_from_report で積んでいない"
                ));
            }
            for (i, l) in body.lines().enumerate() {
                if l.contains("std::fs::") || l.contains("File::") {
                    out.push(format!(
                        "{CORE_MOD}:{}: ModHub::accept（UI スレッド）でファイルに触っている: {}",
                        line + i,
                        l.trim()
                    ));
                }
            }
        }
        None => out.push(format!("{CORE_MOD}: ModHub::accept が見つからない")),
    }
    // tako-app から tako_control::usage を呼ぶのは drive_usage_record の背景のブロックの中だけ
    let drive = fn_body(main, "fn drive_usage_record(");
    match drive {
        Some((body, _)) => {
            if !body.contains("background_executor()") {
                out.push(format!(
                    "{MAIN}: drive_usage_record が記録を背景へ出していない"
                ));
            }
        }
        None => out.push(format!("{MAIN}: drive_usage_record が見つからない")),
    }
    let drive_range = drive.map(|(body, line)| (line, line + body.lines().count()));
    for (i, l) in main.lines().enumerate() {
        if !l.contains("tako_control::usage::") {
            continue;
        }
        let n = i + 1;
        let inside = drive_range.is_some_and(|(a, b)| (a..=b).contains(&n));
        // 背景のブロック（`.spawn(async move {` の行か、その直後の行）だけ
        let prev = main.lines().nth(i.saturating_sub(1)).unwrap_or("");
        let in_spawn = l.contains("spawn(async move") || prev.contains("spawn(async move");
        if !(inside && in_spawn) {
            out.push(format!(
                "{MAIN}:{n}: tako_control::usage を drive_usage_record の背景の外で呼んでいる\
                 （起動の段・UI スレッドで usage のファイルを読み書きしない）: {}",
                l.trim()
            ));
        }
    }
    if !main.contains("app.drive_usage_record(wcx);") {
        out.push(format!(
            "{MAIN}: 定期処理から drive_usage_record を呼んでいない"
        ));
    }
    // IPC / MCP の要約は background のジョブ
    if !dispatch.contains(
        "Request::Sessions { action, .. } if action == \"usage\" => {\n            Some(Ok(OffloadJob::UsageSummary(usage_query(request))))",
    ) {
        out.push(format!(
            "{DISPATCH}: prepare_offload に tako_sessions の action=usage の腕が無い（UI スレッドで読む）"
        ));
    }
    out
}

/// 3: UI スレッドで usage のファイルに触らない・起動の段で読まない
#[test]
fn uiスレッドと起動の段でusageのファイルに触らない() {
    let violations = ui_violations(
        &read(CORE_MOD),
        &read(CONTROL_MOD),
        &read(MAIN),
        &read(DISPATCH),
    );
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}

/// 注入: 現行ソースから作った逆戻りが名指しされる（番犬の検出力）
#[test]
fn 逆戻りを名指しできる() {
    let core_mod = read(CORE_MOD);
    let control_mod = read(CONTROL_MOD);
    let main = read(MAIN);
    let dispatch = read(DISPATCH);
    // (1) 受け口で鍵を引く（資格情報を UI スレッドで読む）
    let injected = control_mod.replacen(
        "    let pane_id = resolve_reporting_pane(host, pane)?;\n",
        "    let pane_id = resolve_reporting_pane(host, pane)?;\n    \
         let _ = std::fs::read_to_string(\"/x/.claude.json\");\n",
        1,
    );
    assert_ne!(injected, control_mod, "注入 (1) の足場が無い");
    let v = ui_violations(&core_mod, &injected, &main, &dispatch);
    assert!(
        v.iter()
            .any(|m| m.starts_with(CONTROL_MOD) && m.contains(".claude.json")),
        "注入 (1) を名指せない: {v:?}"
    );
    // (2) GUI が UI スレッドで直に記録する（背景へ出さない）
    let injected = main.replacen(
        "        cx.background_executor()\n            .spawn(async move { tako_control::usage::drive_for_gui(batch) })\n            .detach();\n",
        "        tako_control::usage::drive_for_gui(batch);\n",
        1,
    );
    assert_ne!(injected, main, "注入 (2) の足場が無い");
    let v = ui_violations(&core_mod, &control_mod, &injected, &dispatch);
    assert!(
        v.iter().any(|m| m.contains("背景")),
        "注入 (2) を名指せない: {v:?}"
    );
    // (3) 起動の段で要約を読む
    let injected = main.replacen(
        "fn drive_usage_record(",
        "fn boot() { let _ = tako_control::usage::summary_payload(); }\n    fn drive_usage_record(",
        1,
    );
    let v = ui_violations(&core_mod, &control_mod, &injected, &dispatch);
    assert!(
        v.iter().any(|m| m.contains("summary_payload")),
        "注入 (3) を名指せない: {v:?}"
    );
    // (4) 要約を background のジョブから外す
    let injected = dispatch.replacen(
        "Request::Sessions { action, .. } if action == \"usage\" => {\n            Some(Ok(OffloadJob::UsageSummary(usage_query(request))))",
        "",
        1,
    );
    let v = ui_violations(&core_mod, &control_mod, &main, &injected);
    assert!(
        v.iter().any(|m| m.contains("prepare_offload")),
        "注入 (4) を名指せない: {v:?}"
    );
}
