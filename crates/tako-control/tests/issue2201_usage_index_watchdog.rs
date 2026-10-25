//! usage 分析 S2（#2201。エピック #2192）の番犬: 会話記録の使用量の索引
//!
//! # なぜ要るか
//!
//! 索引は**利用者の会話記録**（`<設定 dir>/projects/**/*.jsonl`。本文・ツールの入力・cwd を含む）を
//! 読み続け、`<data_dir>/usage/` へ書き続ける。壊れ方はどれも応答の形を変えないので、挙動の単体テストだけ
//! では気づけない:
//!
//! - 読み手の構造体に `content` を 1 つ足すだけで、本文を読み込む（索引の欄に写せば残り続ける）
//! - 鍵を写すときに cwd・gitBranch を欄に足すだけで、ホームパス・作業の名前が残る
//! - GUI の起動の段や UI スレッドで索引を読み進めると、4 GB を読む間 GUI が止まる（#1984 の系統）
//!
//! # 何を縛るか
//!
//! 1. 実経路（`usage_payload` の `Refresh::Full` = CLI `tako usage` と同じ）: 偽の transcript の本文・
//!    ツールの入力・`wireToolInputs`・gitBranch・cwd・設定 dir のパスに入れた印が `usage/` のどこにも
//!    出ない。数えた量は期待値どおり。A/B（`TAKO_2192_S2_LEGACY=1`）では索引を作らない
//! 2. 読み手の構造体（`RawLine` / `RawMessage` / `RawUsage`）と永続する行の型（`BucketRow` /
//!    `CursorRow` / `LastMessage` / `SessionKeys` / `Tokens`）に本文・パス系の欄が無い
//! 3. tako-app は索引を直に触らない（`drive_for_gui` の背景だけ）。最初の更新は起動から 60 秒以上後
//!
//! [`逆戻りを名指しできる`] で、現行ソースへの注入（読み手に `content` を足す・行に `cwd` を足す）が
//! file:line で名指しされることを確かめる。

use std::path::{Path, PathBuf};

#[path = "common/detached_env.rs"]
mod detached_env;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

const CORE_INDEX: &str = "crates/tako-core/src/usage/index.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";
const USAGE_PANEL: &str = "crates/tako-app/src/usage_panel.rs";

/// 偽の transcript に入れる印（どこかに出たら名指す）
const BODY: &str = "usage-index-watchdog-body";
const TOOL: &str = "usage-index-watchdog-tool";
const WIRE: &str = "usage-index-watchdog-wire";
const BRANCH: &str = "usage-index-watchdog-branch";
const CWD: &str = "/home/testuser/dev/usage-index-watchdog-cwd";

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
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

fn assistant(id: &str, output: u64) -> String {
    format!(
        r#"{{"type":"assistant","isSidechain":false,"message":{{"id":"{id}","model":"claude-opus-5-5","role":"assistant","content":[{{"type":"text","text":"{BODY}"}},{{"type":"tool_use","name":"Bash","input":{{"command":"{TOOL}"}}}}],"usage":{{"input_tokens":1,"output_tokens":{output},"cache_read_input_tokens":10,"cache_creation_input_tokens":0}}}},"requestId":"req_{id}","timestamp":"2026-10-16T12:00:00.000Z","sessionId":"watchdog-s2","cwd":"{CWD}","gitBranch":"feat/{BRANCH}","wireToolInputs":{{"cmd":"{WIRE}"}},"uuid":"u-{id}-{output}"}}"#
    )
}

/// 1: 実経路。**env を使うので、この 1 本の中で順に見る**
#[test]
fn 実経路で本文もパスも索引に出ず_ab_では作らない() {
    // 走らせた人の TAKO_DATA_DIR（worker の検証 env の `/tmp/…`）を持ち込まない子で走らせる（#2391）
    if detached_env::rerun_without(
        "実経路で本文もパスも索引に出ず_ab_では作らない",
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
    let scratch = tako_core::test_residue::ScratchDir::new("usage-index-watchdog");
    let cfg = scratch.path().join("cfg");
    let slug = cfg.join("projects/-home-testuser-dev-usage-index-watchdog-cwd");
    std::fs::create_dir_all(&slug).unwrap();
    std::fs::write(
        slug.join("watchdog-s2.jsonl"),
        format!(
            "{}\n{}\n{}\n",
            assistant("msg_w1", 5),
            assistant("msg_w1", 40),
            r#"{"type":"user","message":{"content":"usage-index-watchdog-body"}}"#
        ),
    )
    .unwrap();
    std::env::set_var(tako_control::usage::index::CONFIG_DIRS_ENV, &cfg);
    let query = tako_control::usage::UsageQuery {
        by: Some("session".into()),
        window: Some("all".into()),
        ..Default::default()
    };

    // A/B: 旧挙動は索引を作らない
    std::env::set_var(tako_core::usage::index::S2_LEGACY_ENV, "1");
    let p = tako_control::usage::usage_payload(&query, tako_control::usage::Refresh::Full).unwrap();
    std::env::remove_var(tako_core::usage::index::S2_LEGACY_ENV);
    assert_eq!(p["index"]["enabled"], false);
    assert!(
        !usage.join(tako_control::usage::index::CURSOR_FILE).exists(),
        "A/B なのに索引を作った"
    );

    // 新挙動: 読んで数える（分割行は最大へ寄せる）
    let p = tako_control::usage::usage_payload(&query, tako_control::usage::Refresh::Full).unwrap();
    std::env::remove_var(tako_control::usage::index::CONFIG_DIRS_ENV);
    assert_eq!(p["index"]["enabled"], true);
    assert_eq!(p["index"]["update"]["responses"], 1, "{p}");
    let total = &p["logins"][0]["usage"]["total"];
    assert_eq!(total["tokens"]["output"], 40, "{p}");
    assert_eq!(total["responses"], 1);

    // 本文・ツールの入力・wireToolInputs・gitBranch・cwd・設定 dir のパスがどのファイルにも出ない
    let scanned = files_under(&usage);
    assert!(
        scanned.len() >= 3,
        "usage/ の走査が空振りしている（{} 個）",
        scanned.len()
    );
    let cfg_text = cfg.display().to_string();
    let needles = [BODY, TOOL, WIRE, BRANCH, CWD, cfg_text.as_str()];
    for file in &scanned {
        let text = std::fs::read_to_string(file).unwrap_or_default();
        for needle in needles {
            assert!(
                !text.contains(needle),
                "{} に {needle:?} が出た（本文・パスを索引へ書かない = #2201 / 設計書 §6）",
                file.display()
            );
        }
    }
    // 応答にも出ない（MCP / CLI に返すもの）
    let text = p.to_string();
    for needle in needles {
        assert!(!text.contains(needle), "応答に {needle:?} が出た");
    }
}

/// `pub struct Name {` / `struct Name {` の本体と開始行
fn struct_body<'a>(text: &'a str, name: &str) -> Option<(&'a str, usize)> {
    let head = text
        .find(&format!("pub struct {name} {{"))
        .or_else(|| text.find(&format!("struct {name} {{")))?;
    let rest = text.get(head..)?;
    let close = rest.find("\n}")?;
    let line = text.get(..head)?.lines().count() + 1;
    Some((rest.get(..close)?, line))
}

/// 欄の名前（`pub x: T` / `x: T`）
fn fields(body: &str) -> Vec<(usize, String)> {
    body.lines()
        .enumerate()
        .skip(1)
        .filter_map(|(i, l)| {
            let l = l.trim();
            if l.starts_with("//") || l.starts_with('#') {
                return None;
            }
            let l = l.strip_prefix("pub ").unwrap_or(l);
            let (name, _) = l.split_once(':')?;
            let name = name.trim();
            (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
                .then(|| (i, name.to_string()))
        })
        .collect()
}

/// 本文・パス・作業の名前に当たる欄
const DENY: &[&str] = &[
    "content", "text", "prompt", "body", "wire", "tool", "cwd", "path", "dir", "home", "branch",
    "uuid", "email",
];

/// 読み手と永続する行の型の違反を集める
fn struct_violations(core_index: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = 0;
    for name in [
        "RawLine",
        "RawMessage",
        "RawUsage",
        "BucketRow",
        "CursorRow",
        "LastMessage",
        "SessionKeys",
        "Tokens",
    ] {
        let Some((body, line)) = struct_body(core_index, name) else {
            out.push(format!("{CORE_INDEX}: {name} が見つからない"));
            continue;
        };
        for (i, field) in fields(body) {
            seen += 1;
            // 読み手は cwd を鍵の材料として読む（索引へは書かない = 1 の実経路で縛る）。uuid は応答の
            // 鍵の最後の手段（id も requestId も無い行）
            if name == "RawLine" && (field == "cwd" || field == "uuid") {
                continue;
            }
            for deny in DENY {
                if field.contains(deny) {
                    out.push(format!(
                        "{CORE_INDEX}:{} の {name} に {field}（{deny}）がある: 本文・パス・作業の名前を\
                         読まない・索引へ書かない（#2201 / 設計書 §6）",
                        line + i
                    ));
                }
            }
        }
    }
    if seen < 30 {
        out.push(format!("欄の走査が空振りしている（{seen} 個）"));
    }
    out
}

/// 2: 読み手と永続する行の型に本文・パス系の欄が無い
#[test]
fn 読み手と索引の行に本文やパスの欄が無い() {
    let v = struct_violations(&read(CORE_INDEX));
    assert!(v.is_empty(), "{}", v.join("\n"));
}

/// tako-app の違反を集める
fn app_violations(main: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, l) in main.lines().enumerate() {
        for word in ["usage_payload", "usage::index", "Indexer"] {
            if l.contains(word) {
                out.push(format!(
                    "{MAIN}:{}: tako-app が使用量の索引を直に触っている（{word}）: 背景の \
                     drive_for_gui だけを通す（起動の段・UI スレッドで会話記録を読まない）",
                    i + 1
                ));
            }
        }
    }
    if !main.contains("tako_control::usage::drive_for_gui(batch)") {
        out.push(format!(
            "{MAIN}: 定期処理が drive_for_gui を背景で呼んでいない"
        ));
    }
    out
}

/// 右パネルの usage ビュー（#2203）の違反を集める: 索引・観測の読みの口を直に呼ばず、CLI / MCP と同じ
/// 要求（`Request::Sessions` の usage）を `dispatch_off_ui` で投げる（= `prepare_offload` の background）
fn panel_violations(panel: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, l) in panel.lines().enumerate() {
        let code = l.trim_start();
        if code.starts_with("//") {
            continue;
        }
        for word in [
            "usage_payload",
            "payload_in",
            "summary_payload",
            "usage::index",
            "Indexer",
            "drive_for_gui",
            "record_for_gui",
            "read_rows",
            "read_keys",
        ] {
            if code.contains(word) {
                out.push(format!(
                    "{USAGE_PANEL}:{}: usage ビューが索引・観測の読みの口を直に呼んでいる（{word}）:                      `Request::Sessions` の usage を dispatch_off_ui で投げる（描画・UI スレッドで集計しない。#2203）",
                    i + 1
                ));
            }
        }
    }
    let fetch = panel
        .split("pub(crate) fn spawn_usage_fetch(")
        .nth(1)
        .and_then(|rest| rest.split("\n    pub(crate) fn ").next())
        .unwrap_or("");
    if !(fetch.contains("tako_control::protocol::Request::Sessions {")
        && fetch.contains("self.dispatch_off_ui(request"))
    {
        out.push(format!(
            "{USAGE_PANEL}: spawn_usage_fetch が `Request::Sessions` を dispatch_off_ui で投げていない              （CLI と同じ関数を background で通す経路が消えた。#2203）"
        ));
    }
    out
}

/// 3: tako-app は索引を直に触らない・最初の更新は起動から 60 秒以上後
#[test]
fn guiは背景のdrive_for_guiだけを通し起動の段では読まない() {
    let v = app_violations(&read(MAIN));
    assert!(v.is_empty(), "{}", v.join("\n"));
    let v = panel_violations(&read(USAGE_PANEL));
    assert!(v.is_empty(), "{}", v.join("\n"));
    assert!(
        tako_control::usage::GUI_FIRST_DELAY >= std::time::Duration::from_secs(60),
        "起動直後に会話記録を読み始める（設計書 §5.3 の「起動時は何も読まない」）"
    );
    assert!(tako_control::usage::GUI_RUN_BUDGET <= std::time::Duration::from_secs(10));
}

/// 注入: 現行ソースへの逆戻りが名指しされる（番犬の検出力）
#[test]
fn 逆戻りを名指しできる() {
    let core_index = read(CORE_INDEX);
    // (1) 読み手に本文を足す
    let injected = core_index.replacen(
        "struct RawMessage {\n",
        "struct RawMessage {\n    #[serde(default)]\n    content: Option<serde_json::Value>,\n",
        1,
    );
    assert_ne!(injected, core_index, "注入 (1) の足場が無い");
    let v = struct_violations(&injected);
    assert!(
        v.iter()
            .any(|m| m.contains("RawMessage に content") && m.starts_with(CORE_INDEX)),
        "注入 (1) を名指せない: {v:?}"
    );
    // (2) 写す鍵に cwd を足す
    let injected = core_index.replacen(
        "pub struct SessionKeys {\n",
        "pub struct SessionKeys {\n    pub cwd: Option<String>,\n",
        1,
    );
    assert_ne!(injected, core_index, "注入 (2) の足場が無い");
    let v = struct_violations(&injected);
    assert!(
        v.iter().any(|m| m.contains("SessionKeys に cwd")),
        "注入 (2) を名指せない: {v:?}"
    );
    // (3) GUI が UI スレッドで直に読む
    let main = read(MAIN);
    let injected = main.replacen(
        "fn drive_usage_record(",
        "fn boot() { let _ = tako_control::usage::usage_payload(&Default::default(), tako_control::usage::Refresh::Full); }\n    fn drive_usage_record(",
        1,
    );
    assert_ne!(injected, main, "注入 (3) の足場が無い");
    let v = app_violations(&injected);
    assert!(
        v.iter().any(|m| m.contains("usage_payload")),
        "注入 (3) を名指せない: {v:?}"
    );
    // (4) usage ビュー（#2203）が描画の中で直に集計する
    let panel = read(USAGE_PANEL);
    let injected = panel.replacen(
        "        let views = login_views(&payload, &level, result.at_unix, offset);",
        "        let _ = tako_control::usage::usage_payload(&self.usage.query(), tako_control::usage::Refresh::Skip);\n        let views = login_views(&payload, &level, result.at_unix, offset);",
        1,
    );
    assert_ne!(injected, panel, "注入 (4) の足場が無い");
    let v = panel_violations(&injected);
    assert!(
        v.iter()
            .any(|m| m.starts_with(USAGE_PANEL) && m.contains("usage_payload")),
        "注入 (4) を名指せない: {v:?}"
    );
    // (5) 読みを dispatch の経路から外す（同期の dispatch で UI スレッドに集計させる）
    let injected = panel.replacen(
        "self.dispatch_off_ui(request",
        "self.dispatch_sync_for_test(request",
        1,
    );
    assert_ne!(injected, panel, "注入 (5) の足場が無い");
    let v = panel_violations(&injected);
    assert!(
        v.iter()
            .any(|m| m.contains("dispatch_off_ui で投げていない")),
        "注入 (5) を名指せない: {v:?}"
    );
}
