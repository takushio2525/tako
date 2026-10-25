//! usage 分析 S5（#2204。エピック #2192）の番犬: codex の rollout の使用量と利用制限の観測
//!
//! # なぜ要るか
//!
//! 索引は**利用者の codex の rollout**（`$CODEX_HOME/sessions/**/rollout-*.jsonl`。発話・推論・ツールの入出力・
//! `base_instructions`・cwd・git のブランチを含む）と `auth.json`（トークン）を読み、`<data_dir>/usage/` へ
//! 書き続ける。壊れ方はどれも応答の形を変えないので、挙動の単体テストだけでは気づけない:
//!
//! - 読み手の構造体に `content` / `base_instructions` を 1 つ足すだけで、本文を読み込む
//! - 読み位置（`cursor.jsonl` の `codex`）に cwd を足すだけで、ホームパスが残り続ける
//! - ログインの鍵に account_id をそのまま使うだけで、生の識別子が残る
//!
//! # 何を縛るか
//!
//! 1. 実経路（`usage_payload` の `Refresh::Full` = CLI `tako usage` と同じ）: 偽の rollout の発話・指示・
//!    ツールの入力・cwd・ブランチ・`auth.json` のトークンと account_id に入れた印が `usage/` のどこにも、
//!    応答にも出ない。codex のログインが `agent: codex` で出て、量は累積の差分どおり。A/B
//!    （`TAKO_2192_S5_LEGACY=1`）では codex を読まない
//! 2. 読み手の構造体（`RawLine` / `RawPayload` / `RawInfo` / `RawUsage`）と読み位置の型（`CodexCursor` /
//!    `CodexUsage`）に本文・パス系の欄が無い（読み手の `cwd` は鍵の材料なので除く）
//! 3. `crates/*/src`（lib のテスト = 1 プロセスの中で並列に走る）が、usage の読む env（S1 / S2 / S5 の A/B と
//!    置き場の差し替え）を `set_var` / `remove_var` で書き換えない（#2395。A/B の窓に並走する
//!    usage のテストの走査まで旧挙動になり、Windows CI で `agent: codex` が出ずに落ちた）
//!
//! [`逆戻りを名指しできる`] で、現行ソースへの注入が file:line で名指しされることを確かめる。

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

const CORE_CODEX: &str = "crates/tako-core/src/usage/codex.rs";

/// 偽の rollout と auth.json に入れる印（どこかに出たら名指す）
const BODY: &str = "usage-codex-watchdog-body";
const INSTR: &str = "usage-codex-watchdog-instructions";
const TOOL: &str = "usage-codex-watchdog-tool";
const BRANCH: &str = "usage-codex-watchdog-branch";
const TOKEN: &str = "usage-codex-watchdog-token";
const ACCOUNT: &str = "usage-codex-watchdog-account";
const CWD: &str = "/home/testuser/dev/usage-codex-watchdog-cwd";

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

fn rollout() -> String {
    let t = "2026-10-16T12:00:00.000Z";
    let tc = |ts: &str, input: u64, output: u64, pct: f64| {
        format!(
            r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{input},"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":{output},"reasoning_output_tokens":0,"total_tokens":{}}},"last_token_usage":null,"model_context_window":258400}},"rate_limits":{{"limit_id":"codex","primary":{{"used_percent":{pct},"window_minutes":10080,"resets_at":1792000000}},"secondary":null,"plan_type":"plus"}}}}}}"#,
            input + output
        )
    };
    [
        format!(
            r#"{{"timestamp":"{t}","type":"session_meta","payload":{{"id":"watchdog-s5","timestamp":"{t}","cwd":"{CWD}","source":"cli","base_instructions":{{"text":"{INSTR}"}},"git":{{"branch":"feat/{BRANCH}","repository_url":"https://example.com/{BRANCH}"}}}}}}"#
        ),
        format!(
            r#"{{"timestamp":"{t}","type":"turn_context","payload":{{"turn_id":"t","cwd":"{CWD}","model":"gpt-5.5","user_instructions":"{INSTR}","developer_instructions":"{INSTR}"}}}}"#
        ),
        format!(
            r#"{{"timestamp":"{t}","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"{BODY}"}}]}}}}"#
        ),
        format!(
            r#"{{"timestamp":"{t}","type":"response_item","payload":{{"type":"function_call","name":"shell","arguments":"{{\"command\":\"{TOOL}\"}}"}}}}"#
        ),
        format!(
            r#"{{"timestamp":"{t}","type":"event_msg","payload":{{"type":"agent_message","message":"{BODY}"}}}}"#
        ),
        tc("2026-10-16T12:01:00.000Z", 100, 10, 2.0),
        tc("2026-10-16T12:02:00.000Z", 300, 30, 5.0),
    ]
    .join("\n")
        + "\n"
}

/// 1: 実経路。**env を使うので、この 1 本の中で順に見る**
#[test]
fn 実経路で本文も指示もパスも資格情報も出ず_ab_では読まない() {
    // 走らせた人の TAKO_DATA_DIR（worker の検証 env の `/tmp/…`）を持ち込まない子で走らせる（#2391）
    if detached_env::rerun_without(
        "実経路で本文も指示もパスも資格情報も出ず_ab_では読まない",
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
    let scratch = tako_core::test_residue::ScratchDir::new("usage-codex-watchdog");
    let home = scratch.path().join("codexhome");
    let day = home.join("sessions/2026/10/16");
    std::fs::create_dir_all(&day).unwrap();
    std::fs::write(
        day.join("rollout-2026-10-16T12-00-00-watchdog-s5.jsonl"),
        rollout(),
    )
    .unwrap();
    std::fs::write(
        home.join("auth.json"),
        format!(
            r#"{{"auth_mode":"chatgpt","OPENAI_API_KEY":"{TOKEN}","tokens":{{"id_token":"{TOKEN}","access_token":"{TOKEN}","refresh_token":"{TOKEN}","account_id":"{ACCOUNT}"}},"last_refresh":"{TOKEN}"}}"#
        ),
    )
    .unwrap();
    // claude の設定 dir は空（codex だけを読ませる）
    let cfg = scratch.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::env::set_var(tako_control::usage::index::CONFIG_DIRS_ENV, &cfg);
    std::env::set_var(tako_control::usage::index::CODEX_HOMES_ENV, &home);
    let query = tako_control::usage::UsageQuery {
        by: Some("session".into()),
        window: Some("all".into()),
        ..Default::default()
    };

    // A/B: 旧挙動は codex を読まない
    std::env::set_var(tako_core::usage::codex::S5_LEGACY_ENV, "1");
    let p = tako_control::usage::usage_payload(&query, tako_control::usage::Refresh::Full).unwrap();
    std::env::remove_var(tako_core::usage::codex::S5_LEGACY_ENV);
    assert_eq!(p["index"]["update"]["codex_files"], 0, "{p}");
    assert_eq!(p["logins"].as_array().map(Vec::len), Some(0), "{p}");

    // 新挙動: 累積の差分（110 + 220）と観測 2 点
    let p = tako_control::usage::usage_payload(&query, tako_control::usage::Refresh::Full).unwrap();
    std::env::remove_var(tako_control::usage::index::CONFIG_DIRS_ENV);
    std::env::remove_var(tako_control::usage::index::CODEX_HOMES_ENV);
    assert_eq!(p["index"]["update"]["codex_files"], 1, "{p}");
    assert_eq!(p["index"]["update"]["codex_observations"], 2, "{p}");
    let login = &p["logins"][0];
    assert_eq!(login["agent"], "codex", "{p}");
    assert_eq!(login["key_from"], "account", "{p}");
    assert_eq!(login["usage"]["total"]["total_tokens"], 330, "{p}");
    assert_eq!(login["usage"]["total"]["responses"], 2, "{p}");

    // 本文・指示・ツール・cwd・ブランチ・トークン・account_id・CODEX_HOME のパスがどのファイルにも出ない
    let scanned = files_under(&usage);
    assert!(
        scanned.len() >= 4,
        "usage/ の走査が空振りしている（{} 個）",
        scanned.len()
    );
    let home_text = home.display().to_string();
    let needles = [
        BODY,
        INSTR,
        TOOL,
        BRANCH,
        TOKEN,
        ACCOUNT,
        CWD,
        home_text.as_str(),
    ];
    for file in &scanned {
        let text = std::fs::read_to_string(file).unwrap_or_default();
        for needle in needles {
            assert!(
                !text.contains(needle),
                "{} に {needle:?} が出た（本文・パス・資格情報を索引へ書かない = #2204 / 設計書 §6）",
                file.display()
            );
        }
    }
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

/// 本文・指示・パス・作業の名前・資格情報に当たる欄
const DENY: &[&str] = &[
    "content",
    "text",
    "prompt",
    "body",
    "message",
    "instruction",
    "argument",
    "tool",
    "cwd",
    "path",
    "dir",
    "home",
    "branch",
    "git",
    "repository",
    "token_",
    "account",
    "email",
];

/// 読み手と読み位置の型の違反を集める
fn struct_violations(core_codex: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = 0;
    for name in [
        "RawLine",
        "RawPayload",
        "RawInfo",
        "RawUsage",
        "CodexCursor",
        "CodexUsage",
    ] {
        let Some((body, line)) = struct_body(core_codex, name) else {
            out.push(format!("{CORE_CODEX}: {name} が見つからない"));
            continue;
        };
        for (i, field) in fields(body) {
            seen += 1;
            // 読み手は cwd を鍵の材料として読む（索引へは書かない = 1 の実経路で縛る）。RawInfo の
            // `total_token_usage` / `last_token_usage` と RawUsage の `*_tokens` は数の欄
            if (name == "RawPayload" && field == "cwd")
                || (matches!(name, "RawInfo" | "RawUsage") && field.ends_with("tokens"))
                || (name == "RawInfo" && field.ends_with("token_usage"))
            {
                continue;
            }
            for deny in DENY {
                if field.contains(deny) {
                    out.push(format!(
                        "{CORE_CODEX}:{} の {name} に {field}（{deny}）がある: 本文・パス・資格情報を\
                         読まない・索引へ書かない（#2204 / 設計書 §6）",
                        line + i
                    ));
                }
            }
        }
    }
    if seen < 25 {
        out.push(format!("欄の走査が空振りしている（{seen} 個）"));
    }
    out
}

/// 2: 読み手と読み位置の型に本文・パス系の欄が無い
#[test]
fn 読み手と読み位置の型に本文やパスの欄が無い() {
    let v = struct_violations(&read(CORE_CODEX));
    assert!(v.is_empty(), "{}", v.join("\n"));
}

const USAGE_INDEX: &str = "crates/tako-control/src/usage/index.rs";

/// `set_var(` / `remove_var(` の第 1 引数が usage の読む env を指していれば、その定数の名前（値）。
/// `S2_LEGACY_ENV` と `CONFIG_DIRS_ENV` は claude_mod / claude_mod_install にも同名があるので、
/// 修飾に usage の道があるか、usage のファイルの中の素の名前だけを数える
fn usage_env_of(rel: &str, arg: &str) -> Option<String> {
    let arg: String = arg.chars().filter(|c| !c.is_whitespace()).collect();
    let values = [
        tako_core::usage::S1_LEGACY_ENV,
        tako_core::usage::codex::S5_LEGACY_ENV,
        tako_core::usage::index::S2_LEGACY_ENV,
        tako_control::usage::index::CONFIG_DIRS_ENV,
        tako_control::usage::index::CODEX_HOMES_ENV,
    ];
    if let Some(lit) = arg.strip_prefix('"').and_then(|a| a.strip_suffix('"')) {
        return values.contains(&lit).then(|| lit.to_string());
    }
    let (qual, name) = arg.rsplit_once("::").unwrap_or(("", arg.as_str()));
    let in_usage = rel.contains("/usage/") || rel.ends_with("/usage.rs");
    let usage_path = qual.contains("usage")
        || qual.starts_with("core_index")
        || qual.starts_with("core_codex")
        || (qual.is_empty() && in_usage);
    match name {
        "S1_LEGACY_ENV" | "S5_LEGACY_ENV" | "CODEX_HOMES_ENV" => Some(name.to_string()),
        "S2_LEGACY_ENV" | "CONFIG_DIRS_ENV" if usage_path => Some(name.to_string()),
        _ => None,
    }
}

/// 1 ファイルの違反と、見た `set_var(` / `remove_var(` の数（コメントの行は見ない）
fn env_write_violations(rel: &str, text: &str) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut seen = 0;
    for needle in ["set_var(", "remove_var("] {
        for (at, _) in text.match_indices(needle) {
            let head = text[..at].rfind('\n').map_or(0, |i| i + 1);
            if text[head..at].trim_start().starts_with("//") {
                continue;
            }
            seen += 1;
            let rest = &text[at + needle.len()..];
            let arg = &rest[..rest.find([',', ')']).unwrap_or(rest.len())];
            if let Some(name) = usage_env_of(rel, arg) {
                let line = text[..at].matches('\n').count() + 1;
                out.push(format!(
                    "{rel}:{line}: lib のテストで usage の env（{name}）を書き換えている: lib のテストは \
                     1 プロセスの中で並列に走るので、並走する usage のテストの走査まで変わる（#2395）。\
                     A/B は値を引数で渡す（`walk_with`）か、結合テストの 1 本の中で書く"
                ));
            }
        }
    }
    (out, seen)
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                rs_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
}

/// 3: lib のテストが usage の読む env を書き換えない（#2395）
#[test]
fn libのテストはusageの読むenvを書き換えない() {
    let root = workspace_root();
    let mut files = Vec::new();
    for krate in std::fs::read_dir(root.join("crates")).unwrap().flatten() {
        rs_files(&krate.path().join("src"), &mut files);
    }
    let (mut violations, mut seen) = (Vec::new(), 0);
    for file in &files {
        let rel = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let text = std::fs::read_to_string(file).unwrap_or_default();
        let (v, n) = env_write_violations(&rel, &text);
        violations.extend(v);
        seen += n;
    }
    assert!(
        files.len() >= 100 && seen >= 50,
        "走査が空振りしている（{} 個のファイル・{seen} 個の書き換え）",
        files.len()
    );
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}

/// 注入: 現行ソースへの逆戻りが名指しされる（番犬の検出力）
#[test]
fn 逆戻りを名指しできる() {
    let core = read(CORE_CODEX);
    // (1) 読み手に指示の本文を足す
    let injected = core.replacen(
        "struct RawPayload {\n",
        "struct RawPayload {\n    #[serde(default)]\n    base_instructions: Option<serde_json::Value>,\n",
        1,
    );
    assert_ne!(injected, core, "注入 (1) の足場が無い");
    let v = struct_violations(&injected);
    assert!(
        v.iter()
            .any(|m| m.contains("RawPayload に base_instructions") && m.starts_with(CORE_CODEX)),
        "注入 (1) を名指せない: {v:?}"
    );
    // (2) 読み位置に cwd を足す
    let injected = core.replacen(
        "pub struct CodexCursor {\n",
        "pub struct CodexCursor {\n    pub cwd: Option<String>,\n",
        1,
    );
    assert_ne!(injected, core, "注入 (2) の足場が無い");
    let v = struct_violations(&injected);
    assert!(
        v.iter().any(|m| m.contains("CodexCursor に cwd")),
        "注入 (2) を名指せない: {v:?}"
    );
    // (3) 読み手に発話を足す
    let injected = core.replacen(
        "struct RawLine {\n",
        "struct RawLine {\n    #[serde(default)]\n    content: Option<String>,\n",
        1,
    );
    let v = struct_violations(&injected);
    assert!(
        v.iter().any(|m| m.contains("RawLine に content")),
        "注入 (3) を名指せない: {v:?}"
    );
    // (4) #2395 の前の形: lib のテストの A/B で S5 の env を立てる（rustfmt が折った形も）
    let index = read(USAGE_INDEX);
    let anchor = "let found = env.codex_indexer().walk_with(true).len();";
    assert!(index.contains(anchor), "注入 (4) の足場が無い");
    for set in [
        "std::env::set_var(core_codex::S5_LEGACY_ENV, \"1\");",
        "std::env::set_var(\n            tako_core::usage::codex::S5_LEGACY_ENV,\n            \"1\",\n        );",
    ] {
        let injected = index.replacen(
            anchor,
            &format!("{set}\n        let found = env.codex_indexer().walk().len();"),
            1,
        );
        let (v, _) = env_write_violations(USAGE_INDEX, &injected);
        assert!(
            v.iter()
                .any(|m| m.starts_with(&format!("{USAGE_INDEX}:")) && m.contains("S5_LEGACY_ENV")),
            "注入 (4) を名指せない: {set} → {v:?}"
        );
    }
    // (5) 別のモジュールのテストが値の文字列で S2 の A/B を立てる
    let rel = "crates/tako-control/src/claude_mod.rs";
    let (v, _) = env_write_violations(
        rel,
        "fn t() {\n    std::env::set_var(\"TAKO_2192_S2_LEGACY\", \"1\");\n}\n",
    );
    assert!(
        v.iter()
            .any(|m| m.starts_with(&format!("{rel}:2:")) && m.contains("TAKO_2192_S2_LEGACY")),
        "注入 (5) を名指せない: {v:?}"
    );
    // 同名の別の env（claude_mod の S2・claude_mod_install の置き場）とコメントは名指さない
    let (v, n) = env_write_violations(
        rel,
        "fn t() {\n    std::env::set_var(core::S2_LEGACY_ENV, \"1\");\n    \
         std::env::remove_var(CONFIG_DIRS_ENV);\n    // std::env::set_var(S5_LEGACY_ENV, \"1\");\n}\n",
    );
    assert_eq!((v.len(), n), (0, 2), "同名の別の env を名指した: {v:?}");
}
