//! docs サイトの「全リスト」ページが実装から取り残されないための番犬（Issue #1316）
//!
//! `guides/mcp-tools.md` / `guides/cli-reference.md` は**手書き**のページで、
//! 生成スクリプトが無い。実測（2026-09-11）では MCP ツールが 149 個あるのに
//! ページは 147 を名乗り 138 個しか載せておらず、CLI は 84 種あるのに 69 を名乗り
//! 6 コマンドがページ内に 1 度も出てこなかった。**一覧に無い機能はユーザーには
//! 存在しない**ので、次のリリースで同じ取り残しが起きないよう CI で落とす。
//!
//! 拘束するのは 2 つ:
//!
//! 1. **件数** — docs が名乗っている個数が実装の個数と一致する
//! 2. **網羅性** — docs に出てくる名前集合が実装の名前集合を包む
//!    （逆向き = 実装に無い名前が docs に残っていないか、も MCP 側で見る）
//!
//! 数字の拾い方（前後の文面に依存する）が壊れて黙って通るのを避けるため、
//! **期待した記述が見つからないことも FAILED**にする。
//!
//! Issue #1547 で 2 つ足した:
//!
//! - トップページの**ヒーロー統計**（同じページの本文が 152 を名乗るそばで 128 を
//!   配信していた。見ていたのは本文だけだった）
//! - **手書きのエージェント 4 ページ**が名乗る能力件数（正本は
//!   `tako_core::agent_support::MATRIX`。47 → 52 になったあと 4 ページとも 47 のままだった）

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use tako_core::agent_support::{self, Agent};

const MCP_TOOLS_PAGE: &str = "docs/src/content/docs/guides/mcp-tools.md";
const CLI_PAGE: &str = "docs/src/content/docs/guides/cli-reference.md";
const MCP_SERVER_PAGE: &str = "docs/src/content/docs/features/mcp-server.md";
const INDEX_PAGE: &str = "docs/src/content/docs/index.mdx";
const CLI_MAIN: &str = "crates/tako-cli/src/main.rs";
const AGENT_CLAUDE_PAGE: &str = "docs/src/content/docs/agents/claude.md";
const AGENT_CODEX_PAGE: &str = "docs/src/content/docs/agents/codex.md";
const AGENT_AGY_PAGE: &str = "docs/src/content/docs/agents/agy.md";
const AGENT_LOCAL_PAGE: &str = "docs/src/content/docs/agents/local-llm.md";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    let p = repo_root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{} が読めない: {e}", p.display()))
}

// ---------------------------------------------------------------------------
// 実装側の正本
// ---------------------------------------------------------------------------

/// 公開している MCP ツール名（`mcp::tools()` がこのリポジトリの正本。
/// スナップショットとの一致は `mcp_catalog_snapshot.rs` が別に固定している）
fn mcp_tool_names() -> BTreeSet<String> {
    let names: BTreeSet<String> = tako_control::mcp::tools()
        .iter()
        .map(|tool| {
            tool["name"]
                .as_str()
                .expect("全 MCP ツールに文字列の name がある")
                .to_string()
        })
        .collect();
    assert!(
        names.len() >= 100,
        "MCP ツールが {} 個しか取れていない。番犬の材料の取り方が壊れている",
        names.len()
    );
    names
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// `#[command(... name = "x" ...)]` の x
fn attr_name(attr: &str) -> Option<String> {
    let after = attr.split_once("name")?.1.trim_start();
    let after = after.strip_prefix('=')?.trim_start();
    let after = after.strip_prefix('"')?;
    let end = after.find('"')?;
    Some(after[..end].to_string())
}

/// `Split(SplitArgs),` / `List,` / `Master {` のような variant 行から variant 名
fn variant_name(line: &str) -> Option<String> {
    if !line.starts_with(|c: char| c.is_ascii_uppercase()) {
        return None;
    }
    let end = line
        .find(|c: char| !c.is_ascii_alphanumeric())
        .unwrap_or(line.len());
    let rest = line[end..].trim_start();
    if rest.is_empty() || rest.starts_with(['(', '{', ',']) {
        Some(line[..end].to_string())
    } else {
        None
    }
}

/// clap derive の既定変換（`PreviewOutline` → `preview-outline`）
fn kebab(variant: &str) -> String {
    let mut out = String::with_capacity(variant.len() + 4);
    for (i, c) in variant.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i != 0 {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// `tako` のトップレベルコマンド名（`tako --help` の Commands 節から `help` を除いたもの）。
///
/// テストからバイナリを起こさずに済ませるため **`main.rs` の `enum Command` を
/// ソース走査**する。実バイナリの `--help` と一致することは Issue #1316 で実測済み
/// （84 種・差分 0）。走査が壊れたら下の sanity で落ちる。
fn cli_top_level_commands() -> BTreeSet<String> {
    let src = read(CLI_MAIN);
    let mut lines = src.lines();
    let mut opened = false;
    for line in lines.by_ref() {
        if line.trim() == "enum Command {" {
            opened = true;
            break;
        }
    }
    assert!(
        opened,
        "{CLI_MAIN} に `enum Command {{` が無い。CLI の定義の形が変わったので\
         この番犬（tests/docs_tool_inventory.rs）の走査も直すこと"
    );

    let mut out = BTreeSet::new();
    // enum の `{` を 1 つ開いた状態。variant は depth 1 の行だけ
    // （`Master { .. }` の中のフィールド行を variant と誤認しないため）
    let mut depth = 1usize;
    let mut name_override: Option<String> = None;
    for line in lines {
        let trimmed = line.trim();
        if depth == 1 && !trimmed.starts_with("//") {
            if let Some(attr) = trimmed.strip_prefix("#[command(") {
                if let Some(name) = attr_name(attr) {
                    name_override = Some(name);
                }
            } else if let Some(variant) = variant_name(trimmed) {
                let name = name_override.take().unwrap_or_else(|| kebab(&variant));
                assert!(
                    out.insert(name.clone()),
                    "{CLI_MAIN} のトップレベルコマンド `{name}` が重複している"
                );
            }
        }
        // 波括弧の数え上げは行コメントを落としてから（コメント内の `}` で
        // 桁借りして分かりにくく落ちるのを避ける）。飽和演算にしてあるので
        // 数え落としても「enum の終わりが早く来る」= sanity で落ちる形に倒れる
        let code = line.split("//").next().unwrap_or(line);
        depth = (depth + code.matches('{').count()).saturating_sub(code.matches('}').count());
        if depth == 0 {
            break;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// docs 側の読み取り
// ---------------------------------------------------------------------------

/// docs が名乗っている件数の在り処。`before` と `after` に挟まれた数字を読む
struct Claim {
    file: &'static str,
    before: &'static str,
    after: &'static str,
}

/// `before<数字>after` を全部探して (行番号, 数字) で返す
fn claimed_numbers(text: &str, before: &str, after: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = text[from..].find(before) {
        let head = from + rel;
        let digits_at = head + before.len();
        let digits_len = text[digits_at..]
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(text.len() - digits_at);
        from = head + before.len();
        if digits_len == 0 {
            continue;
        }
        let tail = digits_at + digits_len;
        if !text[tail..].starts_with(after) {
            continue;
        }
        let line = text[..head].matches('\n').count() + 1;
        let value: usize = text[digits_at..tail].parse().expect("数字として読める");
        out.push((line, value));
    }
    out
}

fn check_claims(claims: &[Claim], expected: usize, what: &str) {
    let mut problems = Vec::new();
    for claim in claims {
        let text = read(claim.file);
        let hits = claimed_numbers(&text, claim.before, claim.after);
        if hits.is_empty() {
            problems.push(format!(
                "  {}: 「{}<数>{}」という記述が見つからない\
                 （docs の文面が変わったか、番犬の拾い方が古い）",
                claim.file, claim.before, claim.after
            ));
            continue;
        }
        for (line, value) in hits {
            if value != expected {
                problems.push(format!(
                    "  {}:{}: 「{}」と書いてあるが実装の{}は {} 個",
                    claim.file, line, value, what, expected
                ));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "docs が名乗っている{what}の数が実装と合っていない（Issue #1316）\n{}\n\
         再計測: cargo test -p tako-control --test docs_tool_inventory -- --nocapture",
        problems.join("\n")
    );
}

/// 期待値を claim ごとに持つ版（`check_claims` は 1 つの期待値を全 claim へ配るので、
/// 「同じページの中で数が違う 5 つ」を見られない）。Issue #1547
struct CountClaim {
    file: &'static str,
    before: &'static str,
    after: &'static str,
    expected: usize,
    what: &'static str,
}

fn check_count_claims(claims: &[CountClaim], issue: &str) {
    let mut problems = Vec::new();
    for claim in claims {
        let text = read(claim.file);
        let hits = claimed_numbers(&text, claim.before, claim.after);
        if hits.is_empty() {
            problems.push(format!(
                "  {}: 「{}<数>{}」という記述が見つからない\
                 （docs の文面が変わったか、番犬の拾い方が古い）",
                claim.file, claim.before, claim.after
            ));
            continue;
        }
        for (line, value) in hits {
            if value != claim.expected {
                problems.push(format!(
                    "  {}:{}: {}を「{}」と書いてあるが実装は {} 件",
                    claim.file, line, claim.what, value, claim.expected
                ));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "docs が名乗っている件数が実装と合っていない（{issue}）\n{}\n\
         再計測: cargo test -p tako-control --test docs_tool_inventory -- --nocapture",
        problems.join("\n")
    );
}

/// マトリクスでその系統がその状態になっている能力の数
fn matrix_count(agent: Agent, status: &str) -> usize {
    agent_support::features(agent, Some(status)).len()
}

/// `text` に `token` が「識別子として独立して」出てくるか
/// （`tako_setup` が `tako_setup_mcp` の一部として出ているだけなら false）
fn contains_token(text: &str, token: &str) -> bool {
    let bytes = text.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = text[from..].find(token) {
        let start = from + rel;
        let end = start + token.len();
        let head_ok = start == 0 || !is_ident_byte(bytes[start - 1]);
        let tail_ok = end >= bytes.len() || !is_ident_byte(bytes[end]);
        if head_ok && tail_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

/// `text` に出てくる `tako_...` 形の名前（MCP ツール名の形）
fn mcp_names_in(text: &str) -> BTreeSet<String> {
    let bytes = text.as_bytes();
    let mut out = BTreeSet::new();
    let mut from = 0usize;
    while let Some(rel) = text[from..].find("tako_") {
        let start = from + rel;
        if start > 0 && is_ident_byte(bytes[start - 1]) {
            from = start + 1;
            continue;
        }
        let mut end = start;
        while end < bytes.len()
            && (bytes[end].is_ascii_lowercase()
                || bytes[end].is_ascii_digit()
                || bytes[end] == b'_')
        {
            end += 1;
        }
        out.insert(text[start..end].to_string());
        from = end.max(start + 1);
    }
    out
}

/// `## コマンド早見表` 節の本文（次の `## ` 見出しの直前まで）
fn cheat_sheet(text: &str) -> String {
    const HEAD: &str = "\n## コマンド早見表\n";
    let start = text
        .find(HEAD)
        .unwrap_or_else(|| panic!("{CLI_PAGE} に `## コマンド早見表` 節が無い"))
        + HEAD.len();
    let rest = &text[start..];
    let end = rest.find("\n## ").map_or(rest.len(), |i| i + 1);
    rest[..end].to_string()
}

/// インラインコードの**先頭トークン**（`tako ` を剥がす）。
/// `` `git log` `` → `git`、`` `mcp serve` `` → `mcp` のように、
/// サブコマンド付きで書かれた行もトップレベル名で拾える
fn code_span_heads(section: &str) -> BTreeSet<String> {
    assert!(
        section.matches('`').count().is_multiple_of(2),
        "早見表のバッククォートが奇数個。コードスパンの切り出しが成立しない"
    );
    let mut out = BTreeSet::new();
    for (i, part) in section.split('`').enumerate() {
        if i % 2 == 0 {
            continue;
        }
        let body = part.trim();
        let body = body.strip_prefix("tako ").unwrap_or(body);
        if let Some(head) = body.split_whitespace().next() {
            out.insert(head.to_string());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 番犬本体
// ---------------------------------------------------------------------------

#[test]
fn docsが名乗るmcpツール数が実装と一致する() {
    let expected = mcp_tool_names().len();
    println!("実装の MCP ツール数 = {expected}");
    check_claims(
        &[
            Claim {
                file: MCP_TOOLS_PAGE,
                before: "公開する ",
                after: " 個の MCP ツールの全リスト",
            },
            Claim {
                file: MCP_TOOLS_PAGE,
                before: "tako は **",
                after: " 個の MCP ツール**を",
            },
            Claim {
                file: MCP_SERVER_PAGE,
                before: "確認してから ",
                after: " 個のツールを公開します",
            },
            Claim {
                file: MCP_SERVER_PAGE,
                before: "## ",
                after: " 個の MCP ツール\n",
            },
            Claim {
                file: MCP_SERVER_PAGE,
                before: "tako は **",
                after: " 個**の MCP ツールを公開しています",
            },
            Claim {
                file: INDEX_PAGE,
                before: "<strong>",
                after: " 個</strong>の MCP ツールを内蔵しています",
            },
            Claim {
                file: INDEX_PAGE,
                before: "公開している ",
                after: " ツールの全リスト",
            },
            // トップページのヒーロー統計（#1547）。**本文の「152 個」だけを見ていて
            // ここを見ていなかった**ので、同じページの中で 128 と 152 が同時に
            // 配信されていた（本番サイトも 128 を出していた）
            Claim {
                file: INDEX_PAGE,
                before: "<span class=\"tako-stat-num\">",
                after: "</span><span class=\"tako-stat-label\">MCP ツール</span>",
            },
        ],
        expected,
        "MCP ツール",
    );
}

#[test]
fn docsが名乗るcliコマンド数が実装と一致する() {
    let expected = cli_top_level_commands().len();
    println!("実装の CLI トップレベルコマンド数 = {expected}");
    check_claims(
        &[
            Claim {
                file: CLI_PAGE,
                before: "tako コマンド全 ",
                after: " 種の逆引き一覧",
            },
            Claim {
                file: CLI_PAGE,
                before: "トップレベルのコマンドは **",
                after: " 種**",
            },
            Claim {
                file: CLI_PAGE,
                before: "引くための全 ",
                after: " コマンドの一覧です",
            },
            Claim {
                file: INDEX_PAGE,
                before: "<span>",
                after: " 個の <code>tako</code> コマンドの逆引き一覧",
            },
            // トップページのヒーロー統計（#1547。上の MCP 側と同じ取り残し）
            Claim {
                file: INDEX_PAGE,
                before: "<span class=\"tako-stat-num\">",
                after: "</span><span class=\"tako-stat-label\">CLI コマンド</span>",
            },
        ],
        expected,
        "CLI トップレベルコマンド",
    );
}

#[test]
fn mcpツール一覧に実装の全ツールが載っている() {
    let page = read(MCP_TOOLS_PAGE);
    let missing: Vec<String> = mcp_tool_names()
        .into_iter()
        .filter(|name| !contains_token(&page, name))
        .collect();
    assert!(
        missing.is_empty(),
        "{} に載っていない MCP ツールが {} 個ある（Issue #1316。\
         一覧に無い機能はユーザーには存在しない）\n  {}\n\
         直し方: 該当カテゴリの表に 1 行ずつ足す（説明は mcp::tools() の description から）",
        MCP_TOOLS_PAGE,
        missing.len(),
        missing.join("\n  ")
    );
}

#[test]
fn mcpツール一覧に実装に無いツール名が残っていない() {
    let page = read(MCP_TOOLS_PAGE);
    let implemented = mcp_tool_names();
    let stale: Vec<String> = mcp_names_in(&page)
        .into_iter()
        .filter(|name| !implemented.contains(name))
        .collect();
    assert!(
        stale.is_empty(),
        "{} に実装に無いツール名が残っている（改名・削除の取り残し）\n  {}",
        MCP_TOOLS_PAGE,
        stale.join("\n  ")
    );
}

#[test]
fn cliリファレンスの早見表に全トップレベルコマンドが載っている() {
    let page = read(CLI_PAGE);
    let listed = code_span_heads(&cheat_sheet(&page));
    let missing: Vec<String> = cli_top_level_commands()
        .into_iter()
        .filter(|name| !listed.contains(name))
        .collect();
    assert!(
        missing.is_empty(),
        "{} の「コマンド早見表」に載っていないコマンドが {} 個ある（Issue #1316）\n  {}\n\
         直し方: 早見表の該当カテゴリに `| [`名前`](#リンク先) | 何をするか |` を足す\
         （説明は `tako <名前> --help` の 1 行目から）",
        CLI_PAGE,
        missing.len(),
        missing.join("\n  ")
    );
}

#[test]
fn 手書きのエージェントページが名乗る能力件数がマトリクスと一致する() {
    let total = agent_support::MATRIX.len();
    println!("マトリクスの能力数 = {total}");
    for agent in Agent::ALL {
        println!(
            "  {}: 対応 {} / 一部対応 {} / 未対応 {} / 対象外 {}",
            agent.as_str(),
            matrix_count(agent, "supported"),
            matrix_count(agent, "degraded"),
            matrix_count(agent, "pending"),
            matrix_count(agent, "unsupported"),
        );
    }

    // `agent-support.md` は生成物（同期は gen-agent-support-docs.mjs --check が見る）なので
    // ここで見るのは**手書きの 4 ページ**だけ
    let claims = [
        CountClaim {
            file: AGENT_CLAUDE_PAGE,
            before: "[対応状況](/agent-support/) の ",
            after: " 件すべてが「対応」",
            expected: total,
            what: "能力の総数",
        },
        CountClaim {
            file: AGENT_CODEX_PAGE,
            before: "現時点で ",
            after: " 件中 ",
            expected: total,
            what: "能力の総数",
        },
        CountClaim {
            file: AGENT_CODEX_PAGE,
            before: " 件中 ",
            after: " 件が「対応」です",
            expected: matrix_count(Agent::Codex, "supported"),
            what: "codex の「対応」",
        },
        CountClaim {
            file: AGENT_CODEX_PAGE,
            before: "（一部対応 ",
            after: " 件・未対応 ",
            expected: matrix_count(Agent::Codex, "degraded"),
            what: "codex の「一部対応」",
        },
        CountClaim {
            file: AGENT_CODEX_PAGE,
            before: " 件・未対応 ",
            after: " 件・対象外 ",
            expected: matrix_count(Agent::Codex, "pending"),
            what: "codex の「未対応」",
        },
        CountClaim {
            file: AGENT_CODEX_PAGE,
            before: " 件・対象外 ",
            after: " 件）",
            expected: matrix_count(Agent::Codex, "unsupported"),
            what: "codex の「対象外」",
        },
        CountClaim {
            file: AGENT_AGY_PAGE,
            before: "現時点で ",
            after: " 件中 ",
            expected: total,
            what: "能力の総数",
        },
        CountClaim {
            file: AGENT_AGY_PAGE,
            before: " 件中 ",
            after: " 件が「対応」です",
            expected: matrix_count(Agent::Agy, "supported"),
            what: "agy の「対応」",
        },
        CountClaim {
            file: AGENT_AGY_PAGE,
            before: "（一部対応 ",
            after: " 件・未対応 ",
            expected: matrix_count(Agent::Agy, "degraded"),
            what: "agy の「一部対応」",
        },
        CountClaim {
            file: AGENT_AGY_PAGE,
            before: " 件・未対応 ",
            after: " 件・対象外 ",
            expected: matrix_count(Agent::Agy, "pending"),
            what: "agy の「未対応」",
        },
        CountClaim {
            file: AGENT_AGY_PAGE,
            before: " 件・対象外 ",
            after: " 件）",
            expected: matrix_count(Agent::Agy, "unsupported"),
            what: "agy の「対象外」",
        },
        CountClaim {
            file: AGENT_LOCAL_PAGE,
            before: "[対応状況](/agent-support/) の ",
            after: " 件のうち、対応しているものは ",
            expected: total,
            what: "能力の総数",
        },
        CountClaim {
            file: AGENT_LOCAL_PAGE,
            before: " 件のうち、対応しているものは ",
            after: " 件です",
            expected: matrix_count(Agent::Local, "supported"),
            what: "ローカル LLM の「対応」",
        },
        CountClaim {
            file: AGENT_LOCAL_PAGE,
            before: "- **対象外（",
            after: " 件）**",
            expected: matrix_count(Agent::Local, "unsupported"),
            what: "ローカル LLM の「対象外」",
        },
        CountClaim {
            file: AGENT_LOCAL_PAGE,
            before: "- **未対応（",
            after: " 件）**",
            expected: matrix_count(Agent::Local, "pending"),
            what: "ローカル LLM の「未対応」",
        },
    ];
    check_count_claims(&claims, "Issue #1547");
}

/// 材料の取り方が壊れたまま「全部載っている」と誤判定しないための足場。
/// ここが落ちたら上の 4 本の結果は信用できない
#[test]
fn 番犬の材料の取り方が壊れていない() {
    let commands = cli_top_level_commands();
    assert!(
        commands.len() >= 60,
        "CLI トップレベルコマンドが {} 個しか取れていない。{CLI_MAIN} の走査が壊れている",
        commands.len()
    );
    for anchor in [
        "split",
        "send",
        "list",
        "master",
        "orchestrator",
        "preview-outline", // #[command(name = ...)] の上書き経路
        "remote-folder",   // 同上（ハイフン入り）
        "run-interactive-status",
    ] {
        assert!(
            commands.contains(anchor),
            "`tako {anchor}` が走査結果に無い。{CLI_MAIN} の走査が壊れている"
        );
    }
    for name in &commands {
        assert!(
            name.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "コマンド名 `{name}` の形がおかしい（走査が variant 以外を拾っている）"
        );
    }

    let page = read(CLI_PAGE);
    let heads = code_span_heads(&cheat_sheet(&page));
    assert!(
        heads.len() >= 50,
        "早見表から拾えたコマンド名が {} 個しかない。切り出しが壊れている",
        heads.len()
    );

    // 数字の拾い方そのものの検算
    assert_eq!(
        claimed_numbers(
            "tako は **149 個の MCP ツール**を",
            "tako は **",
            " 個の MCP ツール**を"
        ),
        vec![(1, 149)]
    );
    assert!(claimed_numbers(
        "tako は ** 個の MCP ツール**を",
        "tako は **",
        " 個の MCP ツール**を"
    )
    .is_empty());
    assert!(claimed_numbers(
        "tako は **149 個のツール**を",
        "tako は **",
        " 個の MCP ツール**を"
    )
    .is_empty());
    assert!(contains_token("| `tako_setup_mcp` |", "tako_setup_mcp"));
    assert!(!contains_token("| `tako_setup_mcp` |", "tako_setup"));

    // 能力マトリクス側の材料（#1547）。ここが 0 に化けると
    // 「docs の数字と一致しない」ではなく「見つからない」側へ倒れて分かりにくいので、
    // 形だけ先に固定する
    let total = agent_support::MATRIX.len();
    assert!(
        total >= 40,
        "マトリクスの能力が {total} 件しか無い。材料の取り方が壊れている"
    );
    for agent in Agent::ALL {
        let sum: usize = ["supported", "degraded", "pending", "unsupported"]
            .iter()
            .map(|st| matrix_count(agent, st))
            .sum();
        assert_eq!(
            sum,
            total,
            "{} の状態別の合計が総数と合わない（status の文字列が変わった？）",
            agent.as_str()
        );
    }
    assert_eq!(
        matrix_count(Agent::Claude, "supported"),
        total,
        "claude は基準系なので全件が supported のはず（マトリクスの前提が変わった）"
    );
}
