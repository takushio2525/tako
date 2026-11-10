//! claude の MCP の書き先が `CLAUDE_CONFIG_DIR` に従う 1 実装から出ていることの番犬（#2906）
//!
//! `claude mcp add --scope user` は子の env の `CLAUDE_CONFIG_DIR` に従って
//! `$CLAUDE_CONFIG_DIR/.claude.json`（未設定・空なら `~/.claude.json`）へ書く。#2906 の前の tako は
//! MCP の登録側だけが `~/.claude.json` 決め打ちで、`CLAUDE_CONFIG_DIR` を向けた環境では
//!
//! 1. 表示（`tako setup` / `tako setup-mcp` の「追加しました（…）」）が実際の書き先と違う
//! 2. 登録済みの判定（`dispatch::read_mcp_registration`）が読めず、毎回 remove → add の「追加しました」
//! 3. 健全性の判定（`agent_mcp::claude_registered_command`）が読めず「楽観判定」= 修復へ進まない
//! 4. claude CLI が無いときの直書き（`dispatch::setup_mcp_direct`）が claude から見えない場所へ書く
//!
//! だった。アンインストールの前の片付け（`setup_undo::claude_json_paths`）は既に正しい規則を持っていた。
//! 規則の正本は `tako_core::claude_config`、このプロセスの env から書き先を引く口は
//! `agent_mcp::claude_user_config_json`（= `dispatch::mcp_target_path(McpScope::User)`）の 1 本。
//!
//! # 何を見るか
//!
//! - **振る舞い**: `HOME` と `CLAUDE_CONFIG_DIR` を砂場へ向けた子プロセス（`run_alone!`）で、
//!   書き先・登録済みの判定・健全性・直書き・片付けの各口がどこを見るかを**口の名前で**確かめる
//!   （ホーム直下には別の command を書いた囮を置き、そこを読んだら名指して落ちる）
//! - **規則の一致**: MCP の書き先と `setup_undo::claude_json_paths` が同じ答えを出す
//! - **決め打ち**: 本番コード（`crates/*/src` の `#[cfg(test)]` の外）で中身が `.claude.json` で終わる
//!   文字列リテラルを `file:line` で名指す。MCP の登録とは別の規則で `.claude.json` を扱う読み手は
//!   [`ALLOWED_CLAUDE_JSON`] に**理由つき**で載せる（載せた行が消えたらそれも落とす）

use std::path::{Path, PathBuf};

use tako_control::dispatch::{self, McpScope};
use tako_core::test_residue::ScratchDir;

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view;

/// 規則の正本
const RESOLVER: &str = "crates/tako-core/src/claude_config.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 砂場の HOME と設定 dir（作るだけ。env は呼び手が子の中で当てる）
struct Sandbox {
    _scratch: ScratchDir,
    home: PathBuf,
    alt: PathBuf,
}

fn sandbox(tag: &str) -> Sandbox {
    let scratch = ScratchDir::new(&format!("i2906-{tag}"));
    let home = scratch.join("home");
    let alt = home.join(".claude-alt");
    std::fs::create_dir_all(&alt).expect("砂場の設定 dir を作れる");
    Sandbox {
        _scratch: scratch,
        home,
        alt,
    }
}

/// 子の中でだけ呼ぶ（`run_alone!` の後）。HOME（Windows は USERPROFILE も）を砂場へ向ける
fn point_home(home: &Path) {
    tako_core::test_child::assert_alone("point_home");
    std::env::set_var("HOME", home);
    std::env::set_var("USERPROFILE", home);
}

fn write_registration(path: &Path, command: &str) {
    let body = serde_json::json!({
        "numStartups": 3,
        "mcpServers": {
            "tako": { "type": "stdio", "command": command, "args": ["mcp", "serve"], "env": {} }
        }
    });
    std::fs::write(path, serde_json::to_string_pretty(&body).unwrap()).expect("登録を書ける");
}

fn registered_command(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v["mcpServers"]["tako"]["command"]
        .as_str()
        .map(String::from)
}

/// 生きている command（在る実行ファイル）。テストの実行ファイル自身を使う
fn alive_command() -> String {
    std::env::current_exe()
        .expect("テストの実行ファイル")
        .display()
        .to_string()
}

const DECOY: &str = "/decoy-2906/home-claude-json/tako";
const DEAD: &str = "/nonexistent-2906/old/tako";

/// 各口が `expected` を見ることを、口の名前で名指して確かめる（HOME / env は呼び手が当て済み）
fn assert_every_door_reads(expected: &Path, label: &str) {
    // 書き先・表示
    let target = dispatch::mcp_target_path(&McpScope::User);
    assert_eq!(
        target, expected,
        "[{label}] dispatch::mcp_target_path(User)（書き先・表示）が claude CLI の書く場所と違う"
    );

    // 健全性の判定・起動前の警告（`claude_registered_command`）
    let alive = alive_command();
    write_registration(expected, &alive);
    assert_eq!(
        tako_control::agent_mcp::claude_registered_command().as_deref(),
        Some(alive.as_str()),
        "[{label}] agent_mcp::claude_registered_command（健全性の判定）が {} の登録を読んでいない",
        expected.display()
    );

    // 登録済みの判定（`read_mcp_registration`）: 生きた登録があれば claude を起こさず「既に設定」
    let result = dispatch::setup_mcp_typed(&alive, &McpScope::User).expect("登録済みの判定");
    assert!(
        result.already_existed && !result.configured && !result.repaired,
        "[{label}] dispatch::read_mcp_registration（登録済みの判定）が {} の生きた登録を見ていない \
         （configured={} already_existed={} repaired={}）",
        expected.display(),
        result.configured,
        result.already_existed,
        result.repaired
    );
    assert_eq!(
        result.target_path, expected,
        "[{label}] setup_mcp の応答の target_path（表示）が書き先と違う"
    );

    // 片付け（`setup_undo`）も同じ場所を見る
    let targets = tako_control::setup_undo::Targets::from_process().expect("片付けの置き場");
    assert!(
        targets.claude_json.iter().any(|p| p == expected),
        "[{label}] setup_undo::Targets::from_process の claude_json に {} が無い: {:#?}",
        expected.display(),
        targets.claude_json
    );

    // 死んだ登録: 健全性の判定が読めて「壊れている」と分かる
    write_registration(expected, DEAD);
    assert_eq!(
        tako_control::agent_mcp::claude_registered_command().as_deref(),
        Some(DEAD),
        "[{label}] agent_mcp::claude_registered_command が死んだ登録を読めない（楽観判定になる）"
    );
}

#[test]
fn claude_config_dirを向けると全部の口がその中のclaude_jsonを見る() {
    if tako_core::run_alone!(claude_config_dirを向けると全部の口がその中のclaude_jsonを見る)
    {
        return;
    }
    let sb = sandbox("alt");
    point_home(&sb.home);
    std::env::set_var("CLAUDE_CONFIG_DIR", &sb.alt);
    let expected = sb.alt.join(".claude.json");
    // ホーム直下に囮（ここを読んだら決め打ちに戻っている）
    let decoy = sb.home.join(".claude.json");
    write_registration(&decoy, DECOY);

    assert_every_door_reads(&expected, "CLAUDE_CONFIG_DIR=<砂場>/.claude-alt");
    // 健全性の判定の残り（`claude mcp list` に tako の行があるか）はテストプロセスから claude を
    // 起こせない（#1261 の門 `agent_probe::blocked`）ので、登録のパスを読む口までをここで見る。
    // `claude mcp list` まで通した判定は PR の受け入れ（ビルドした CLI の `tako setup --check`）で見る

    // claude CLI が無いときの直書き: 死んだ登録を付け替えて $CLAUDE_CONFIG_DIR 側へ書く。
    // unix の CLI の解決はログインシェル（SHELL）に聞くので、答えないシェルにして claude を引けなくする
    #[cfg(unix)]
    {
        std::env::set_var("SHELL", "/usr/bin/false");
        let result =
            dispatch::setup_mcp_typed("/new-2906/tako", &McpScope::User).expect("直書きできる");
        assert!(
            result.configured && result.repaired,
            "直書きの経路を通っていない（configured={} repaired={}）",
            result.configured,
            result.repaired
        );
        assert_eq!(
            registered_command(&expected).as_deref(),
            Some("/new-2906/tako"),
            "dispatch::setup_mcp_direct（直書き）が $CLAUDE_CONFIG_DIR/.claude.json へ書いていない"
        );
    }

    assert_eq!(
        registered_command(&decoy).as_deref(),
        Some(DECOY),
        "ホーム直下の .claude.json（CLAUDE_CONFIG_DIR を向けた claude は読まない）を書き換えた"
    );
}

#[test]
fn claude_config_dirが未設定か空ならホーム直下を見る() {
    if tako_core::run_alone!(claude_config_dirが未設定か空ならホーム直下を見る) {
        return;
    }
    let sb = sandbox("home");
    point_home(&sb.home);
    let expected = sb.home.join(".claude.json");
    std::env::remove_var("CLAUDE_CONFIG_DIR");
    assert_every_door_reads(&expected, "CLAUDE_CONFIG_DIR 未設定");
    std::env::set_var("CLAUDE_CONFIG_DIR", "");
    assert_every_door_reads(&expected, "CLAUDE_CONFIG_DIR=（空）");
    assert!(
        !sb.alt.join(".claude.json").exists(),
        "未設定・空なのに別の設定 dir へ書いた"
    );
}

/// MCP の書き先と、アンインストールの前の片付け（`setup_undo`）が同じ規則で答える（env を書き換えない）
#[test]
fn mcpの書き先はsetup_undoと同じ規則() {
    use tako_control::claude_mod_install::{collect_targets, TargetInputs};
    let home = Path::new("/home/testuser");
    let default_dir = home.join(".claude");
    for env in [
        None,
        Some(""),
        Some("/home/testuser/.claude-alt"),
        Some("/home/testuser/.claude"),
    ] {
        let mcp = tako_control::agent_mcp::claude_user_config_json_with(env, Some(home), false)
            .expect("書き先");
        let undo = tako_control::setup_undo::claude_json_paths(&collect_targets(&TargetInputs {
            default_dir: Some(default_dir.clone()),
            process_env: env.map(String::from),
            ..TargetInputs::default()
        }));
        assert!(
            undo.contains(&mcp),
            "CLAUDE_CONFIG_DIR={env:?}: MCP の書き先 {} を片付けが見ない（規則がずれた）: {undo:#?}",
            mcp.display()
        );
    }
}

// --- `.claude.json` の決め打ち ------------------------------------------------------

/// 本番コードで `.claude.json` を名前として書いてよいもの（ファイル, その行の断片, 理由）。
/// 断片はコメントを潰した眺めの 1 行に含まれる形で書く（rustfmt の折り返しをまたがない）
const ALLOWED_CLAUDE_JSON: &[(&str, &str, &str)] = &[
    (
        RESOLVER,
        "pub const GLOBAL_JSON: &str = \".claude.json\";",
        "規則の正本（ファイル名の定数）",
    ),
    (
        "crates/tako-control/src/claude_tui.rs",
        "h.join(\".claude.json\").is_file()",
        "worker の事前信頼の書き先（アカウントの設定 dir + 在ればホーム直下の旧置き場の両方を書く別の規則・#944。MCP の登録ではない）",
    ),
    (
        "crates/tako-control/src/claude_tui.rs",
        "paths.push(dir.join(\".claude.json\"));",
        "worker の事前信頼の書き先（同上）",
    ),
    (
        "crates/tako-control/src/claude_tui.rs",
        "let legacy = home.join(\".claude.json\");",
        "worker の事前信頼の書き先（同上・旧置き場）",
    ),
    (
        "crates/tako-control/src/worker_mcp.rs",
        "push(dir.join(\".claude.json\"));",
        "worker へ持ち込む MCP の実績の読み取り（アカウントの設定 dir を全部読む。1 つに決めない）",
    ),
    (
        "crates/tako-control/src/worker_mcp.rs",
        "config_dirs.iter().map(|d| d.join(\".claude.json\")).collect();",
        "worker へ持ち込む MCP の実績の読み取り（設定 dir 群を全部読む）",
    ),
    (
        "crates/tako-control/src/worker_mcp.rs",
        "let legacy = home.join(\".claude.json\");",
        "worker へ持ち込む MCP の実績の読み取り（ホーム直下も読む）",
    ),
    (
        "crates/tako-control/src/config_share/catalog.rs",
        "path: \".claude.json\",",
        "設定共有の目録の項目名（claude の設定 dir の中の相対名で、共有しない側の印。置き場は組まない）",
    ),
];

/// 本番コードの眺め: `#[cfg(test)]` の領域とコメントを空白へ潰す（文字列は残す・行番号は原文のまま）
fn production_text(src: &str) -> String {
    code_view::without_comments(&production_range::scan(src).text)
}

/// 中身が `.claude.json` で終わる文字列リテラル（`".claude.json"` / `".claude/.claude.json"`）を
/// (行番号, その行) で返す。`text` は [`production_text`] の眺め
fn claude_json_literals(text: &str) -> Vec<(usize, String)> {
    const NEEDLE: &str = ".claude.json\"";
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find(NEEDLE) {
        let start = from + at;
        from = start + NEEDLE.len();
        // 文字列の中のエスケープした引用符（`"…\".claude.json\""`）は閉じの引用符ではない
        if text[..from - 1].ends_with('\\') {
            continue;
        }
        // 名前の頭: 開きの引用符か区切り（`x.claude.json` のような別の名前は数えない）
        if !matches!(text[..start].chars().next_back(), Some('"' | '/' | '\\')) {
            continue;
        }
        let line = text[..start].matches('\n').count() + 1;
        out.push((line, lines.get(line - 1).unwrap_or(&"").to_string()));
    }
    out
}

/// 1 ファイルの名指し（例外に載っていない `.claude.json` のリテラル）と、使った例外の添字
fn claude_json_offenders(rel: &str, src: &str) -> (Vec<String>, Vec<usize>) {
    let mut offenders = Vec::new();
    let mut used = Vec::new();
    for (line, text) in claude_json_literals(&production_text(src)) {
        let allowed = ALLOWED_CLAUDE_JSON
            .iter()
            .position(|(file, fragment, _)| *file == rel && text.contains(fragment));
        match allowed {
            Some(i) => used.push(i),
            None => offenders.push(format!("{rel}:{line} {}", text.trim())),
        }
    }
    (offenders, used)
}

/// `crates/*/src` の本番のファイル（テスト専用のファイルを除く）を (相対パス, 本文) で
fn product_files() -> Vec<(String, String)> {
    let files = production_range::product_sources();
    let test_only = production_range::test_only_files(&files);
    let root = repo_root();
    files
        .into_iter()
        .filter(|rel| !test_only.contains(rel))
        .map(|rel| {
            let body = std::fs::read_to_string(root.join(&rel)).expect("本番のファイル");
            (rel, body)
        })
        .collect()
}

#[test]
fn claudeのmcpの書き先をclaude_jsonの決め打ちで組まない() {
    let files = product_files();
    assert!(
        files.len() > 300,
        "走査した本番のファイルが少なすぎる（走査に失敗している）: {} 本",
        files.len()
    );
    let mut offenders = Vec::new();
    let mut used = std::collections::BTreeSet::new();
    for (rel, body) in &files {
        let (named, hit) = claude_json_offenders(rel, body);
        offenders.extend(named);
        used.extend(hit);
    }
    assert!(
        offenders.is_empty(),
        "claude の `.claude.json` を決め打ちで組んでいる: {offenders:#?}\n\
         `CLAUDE_CONFIG_DIR` を向けた claude は `$CLAUDE_CONFIG_DIR/.claude.json` を読み書きするので、\
         ホーム直下を決め打ちにすると登録済みの判定・健全性・直書き・表示が実際の置き場とずれる（#2906）。\
         MCP の user スコープの置き場は tako_control::agent_mcp::claude_user_config_json()\
         （= dispatch::mcp_target_path(McpScope::User)）、規則そのものは tako_core::claude_config を通すこと。\
         MCP の登録とは別の規則で扱う読み手なら、理由を添えて ALLOWED_CLAUDE_JSON に載せる"
    );
    let unused: Vec<String> = ALLOWED_CLAUDE_JSON
        .iter()
        .enumerate()
        .filter(|(i, _)| !used.contains(i))
        .map(|(_, (file, fragment, _))| format!("{file}: {fragment}"))
        .collect();
    assert!(
        unused.is_empty(),
        "使われていない例外（行が消えた・書き方が変わった）: {unused:#?}\n\
         直したなら ALLOWED_CLAUDE_JSON から消し、書き方が変わったなら断片を直す"
    );
}

/// 名指す形と、名指さない形（番犬自身の検出力）
#[test]
fn 注入したclaude_jsonの決め打ちを名指す() {
    let named = |src: &str| claude_json_offenders("x.rs", src).0;
    for (src, line) in [
        (
            "fn a(h: &Path) -> PathBuf {\n    h.join(\".claude.json\")\n}\n",
            2,
        ),
        (
            "fn a(h: &Path) -> PathBuf { h.join(\".claude/.claude.json\") }\n",
            1,
        ),
        ("fn a() { Path::new(r\"C:\\x\\.claude.json\"); }\n", 1),
        ("fn a() { let p = r#\".claude.json\"#; }\n", 1),
    ] {
        let hits = named(src);
        assert_eq!(hits.len(), 1, "名指す形: {src}");
        assert!(hits[0].starts_with(&format!("x.rs:{line} ")), "{hits:?}");
    }
    let quiet = [
        // コメント・doc の中
        "fn a() {\n    // h.join(\".claude.json\")\n}\n",
        "/// `home.join(\".claude.json\")` を組まない\nfn a() {}\n",
        // 説明文の途中・別の名前・エスケープした引用符の中
        "fn a() { msg(\"~/.claude.json に書きました\"); }\n",
        "fn a() { h.join(\"x.claude.json\"); h.join(\".claude.jsonc\"); }\n",
        "fn a() { msg(\"say \\\".claude.json\\\"\"); }\n",
        // テストの中（`#[cfg(test)]` の領域は本番コードではない）
        "fn a() {}\n#[cfg(test)]\nmod tests {\n    fn b(h: &Path) { h.join(\".claude.json\"); }\n}\n",
    ];
    for src in quiet {
        assert_eq!(named(src), Vec::<String>::new(), "名指さない形: {src}");
    }
}

/// 実ファイルへ修正前の決め打ちを戻すと、そのファイルの行で名指す（走査が実ファイルの形で生きている証拠）。
/// 置き換える断片が見つからなければ、読み手の書き方が変わったので注入の表を直すこと
#[test]
fn 実ファイルへclaude_jsonの決め打ちを戻すと名指す() {
    // (ファイル, 今の断片, 修正前の形)
    let cases: &[(&str, &str, &str)] = &[
        // 健全性の判定（#2906 の 3）
        (
            "crates/tako-control/src/agent_mcp.rs",
            "crate::dispatch::read_mcp_registration(&crate::dispatch::McpScope::User)",
            "read_tako_command(&home.join(\".claude.json\"))",
        ),
        // 置き場の表（setup_undo の codex / agy と同じ口の claude の腕）
        (
            "crates/tako-control/src/agent_mcp.rs",
            "WorkerAgent::Claude => claude_user_config_json(),",
            "WorkerAgent::Claude => home().map(|h| h.join(\".claude.json\")),",
        ),
        // master / solo の起動前の警告
        (
            "crates/tako-cli/src/main.rs",
            "let Some(cmd) = tako_control::agent_mcp::claude_registered_command() else {",
            "let Some(cmd) = read_tako_command(&home.join(\".claude.json\")) else {",
        ),
        // 片付け（規則を正本から引かずに組む形）
        (
            "crates/tako-control/src/setup_undo.rs",
            "push(global_json(Some(&t.dir), None));",
            "push(Some(t.dir.join(\".claude.json\")));",
        ),
        // 資格情報の置き場
        (
            "crates/tako-core/src/usage.rs",
            "crate::claude_config::global_json(normalize_config_dir(config_dir, home).as_deref(), home)",
            "home.map(|h| h.join(\".claude.json\"))",
        ),
    ];
    let root = repo_root();
    for (rel, fragment, before) in cases {
        let body = std::fs::read_to_string(root.join(rel)).expect("読み手のファイル");
        assert_eq!(
            claude_json_offenders(rel, &body).0,
            Vec::<String>::new(),
            "{rel} は今は通る"
        );
        let at = body.find(fragment).unwrap_or_else(|| {
            panic!("{rel} に注入する断片が無い（読み手の書き方が変わった）: {fragment}")
        });
        let fragment_line = body[..at].matches('\n').count() + 1;
        let injected = body.replacen(fragment, before, 1);
        let hits = claude_json_offenders(rel, &injected).0;
        assert_eq!(hits.len(), 1, "{rel} の注入を 1 つ名指すはず: {hits:#?}");
        assert!(
            hits[0].starts_with(&format!("{rel}:{fragment_line} ")),
            "{rel} の注入した行（{fragment_line}）を名指すはず: {hits:#?}"
        );
    }
}
