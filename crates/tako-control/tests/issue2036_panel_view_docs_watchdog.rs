//! **#2036 の番犬**: md に手書きした `tako panel --view` の候補値が、正本
//! [`PanelViewWire::VALUES`] とずれないようにする。
//!
//! ## 何が起きていたのか（修正前）
//!
//! ビューは #1450（tasks）・#1679（diagnostics）・#1685（references）・#1694（search）で
//! 増えたが、手書きの一覧はそのたびに取り残された:
//!
//! - `tako panel --help` の `--view` の説明文が fleet / orch / git の 3 つで止まっていた
//!   （tako-cli の `help_text_parity::panel_viewのhelpは候補値の全件を説明つきで載せる` が縛る）
//! - AGENTS.md のコマンド索引と `.agent/commands.md` の
//!   `--view <fleet\|orch\|git\|tasks\|diagnostics>` に references / search が無かった
//! - docs の CLI リファレンスの例の列挙も同じ 5 つで止まっていた
//!
//! MCP `tako_panel` は #1467 で説明も enum も正本から生成する形になっている
//! （`issue1467_mcp_enum_watchdog`）。ここは**生成できない md** を見る。
//!
//! ## ここで止める 3 つ
//!
//! 1. [`候補の一覧はvaluesと一致する`] — `--view <a\|b\|…>` の形の一覧が `VALUES` と
//!    同じ並びで全件を挙げる（`<view>` のような `|` の無い仮引数は対象外）
//! 2. [`例の値は受理値`] — `tako panel … --view git` のような例の値が受理値（正式値 + 旧称）に在る
//! 3. [`例で並べる一覧は全件を載せる`] — 値を例で 1 行ずつ並べるページ（[`ENUMERATING`]）が
//!    全件を載せる
//!
//! 見逃す側へ倒れないよう、[`走査が空振りしていない`] と [`修正前の形を名指しできる`] で
//! 拾い方と検出力を同じファイルの中で固定する。落ちるときは **file:line で名指し**する。

use std::path::{Path, PathBuf};

use tako_control::protocol::PanelViewWire;

/// 値を例で 1 行ずつ並べている（= 全件を載せる約束の）ページ
const ENUMERATING: &[&str] = &["docs/src/content/docs/guides/cli-reference.md"];

/// 候補の一覧（`--view <a\|b\|…>`）を必ず持つファイル。持たなくなったら走査が壊れている
const MUST_LIST: &[&str] = &["AGENTS.md", ".agent/commands.md"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

#[derive(Debug, PartialEq)]
enum Mention {
    /// `--view <fleet\|orch\|…>`（`|` で区切った候補の一覧）
    List(Vec<String>),
    /// `--view git`（例の 1 値）
    Value(String),
}

/// 1 行から `tako panel … --view …` の言及を拾う。
///
/// コマンドの終わりはインラインコードの閉じ（`` ` ``）か行末。`--view` の後ろが値の無い
/// 言及（「`tako panel --view` のヘルプ」）や `|` の無い仮引数（`<view>`）は拾わない。
fn view_mentions(line: &str) -> Vec<Mention> {
    let mut out = Vec::new();
    for (start, _) in line.match_indices("tako panel") {
        let rest = &line[start..];
        let cmd = rest.split('`').next().unwrap_or(rest);
        let Some(pos) = cmd.find("--view") else {
            continue;
        };
        let after = cmd[pos + "--view".len()..].trim_start_matches([' ', '=']);
        if let Some(inner) = after.strip_prefix('<') {
            let inner = inner.split('>').next().unwrap_or("");
            if inner.contains('|') {
                let values = inner
                    .replace("\\|", "|")
                    .split('|')
                    .map(|s| s.trim().to_string())
                    .collect();
                out.push(Mention::List(values));
            }
        } else {
            let word: String = after
                .chars()
                .take_while(|c| c.is_ascii_lowercase() || *c == '_')
                .collect();
            if !word.is_empty() {
                out.push(Mention::Value(word));
            }
        }
    }
    out
}

/// 見る md（リポジトリルートからの相対パス, 本文）。
///
/// AGENTS.md・`.agent/` 直下（作業ログ `progress*.md` は履歴なので除く）・`docs/src` 配下の
/// md / mdx / astro。`.agent/plans/` は当時の計画の写しなので見ない。
fn sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, root: &Path, recursive: bool, out: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if recursive {
                    walk(&p, root, recursive, out);
                }
                continue;
            }
            let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if !matches!(ext, "md" | "mdx" | "astro") || name.starts_with("progress") {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&p) {
                let rel = p
                    .strip_prefix(root)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, text));
            }
        }
    }
    let root = repo_root();
    let mut out = Vec::new();
    let agents = root.join("AGENTS.md");
    out.push((
        "AGENTS.md".to_string(),
        std::fs::read_to_string(&agents).expect("AGENTS.md が読める"),
    ));
    walk(&root.join(".agent"), &root, false, &mut out);
    walk(&root.join("docs/src"), &root, true, &mut out);
    out.sort();
    out
}

/// `(ファイル, 行番号, 言及)` を全件集める
fn mentions(sources: &[(String, String)]) -> Vec<(String, usize, Mention)> {
    let mut out = Vec::new();
    for (rel, text) in sources {
        for (i, line) in text.lines().enumerate() {
            for m in view_mentions(line) {
                out.push((rel.clone(), i + 1, m));
            }
        }
    }
    out
}

fn list_mismatches(found: &[(String, usize, Mention)]) -> Vec<String> {
    found
        .iter()
        .filter_map(|(rel, line, m)| match m {
            Mention::List(values)
                if values.iter().map(String::as_str).ne(PanelViewWire::VALUES) =>
            {
                Some(format!(
                    "  {rel}:{line} の候補 {values:?} が正本 {:?} と一致しない",
                    PanelViewWire::VALUES
                ))
            }
            _ => None,
        })
        .collect()
}

fn unknown_values(found: &[(String, usize, Mention)]) -> Vec<String> {
    let accepted = PanelViewWire::accepted_values();
    found
        .iter()
        .filter_map(|(rel, line, m)| match m {
            Mention::Value(v) if !accepted.contains(&v.as_str()) => Some(format!(
                "  {rel}:{line} の例 --view {v} は受理値 {accepted:?} に無い"
            )),
            _ => None,
        })
        .collect()
}

/// [`ENUMERATING`] のページで例に載っていない正式値
fn missing_examples(found: &[(String, usize, Mention)], page: &str) -> Vec<&'static str> {
    PanelViewWire::VALUES
        .iter()
        .copied()
        .filter(|v| {
            !found
                .iter()
                .any(|(rel, _, m)| rel == page && *m == Mention::Value(v.to_string()))
        })
        .collect()
}

#[test]
fn 候補の一覧はvaluesと一致する() {
    let bad = list_mismatches(&mentions(&sources()));
    assert!(
        bad.is_empty(),
        "md に手書きした tako panel --view の候補がビューの正本とずれている\n{}\n\
         直し方: `PanelViewWire::VALUES`（tako-control/src/protocol.rs）と同じ並びで全件を書く",
        bad.join("\n")
    );
}

#[test]
fn 例の値は受理値() {
    let bad = unknown_values(&mentions(&sources()));
    assert!(
        bad.is_empty(),
        "md の tako panel --view の例が受け付けない値を使っている\n{}",
        bad.join("\n")
    );
}

#[test]
fn 例で並べる一覧は全件を載せる() {
    let found = mentions(&sources());
    for page in ENUMERATING {
        let missing = missing_examples(&found, page);
        assert!(
            missing.is_empty(),
            "{page} の tako panel --view の例に {missing:?} が無い（ビューを足したら例も 1 行足す）"
        );
    }
}

#[test]
fn 走査が空振りしていない() {
    let srcs = sources();
    let found = mentions(&srcs);
    for file in MUST_LIST {
        assert!(
            found
                .iter()
                .any(|(rel, _, m)| rel == file && matches!(m, Mention::List(_))),
            "{file} に tako panel --view <…|…> の候補の一覧が見つからない（走査か書式が変わった）"
        );
    }
    for page in ENUMERATING {
        assert!(
            srcs.iter().any(|(rel, _)| rel == page),
            "{page} が走査に入っていない"
        );
    }
    let values = found
        .iter()
        .filter(|(_, _, m)| matches!(m, Mention::Value(_)))
        .count();
    assert!(
        values >= 20,
        "例の言及が {values} 件しか採れていない（実測 26 / 2026-10-16）。拾い方が壊れた"
    );
}

/// 修正前の 3 つの形を、それぞれの検査が file:line で名指しできる
#[test]
fn 修正前の形を名指しできる() {
    let before = vec![
        (
            "AGENTS.md".to_string(),
            "| 右パネルのビュー切替 | `tako panel --show --view <fleet\\|orch\\|git\\|tasks\\|diagnostics>` |"
                .to_string(),
        ),
        (
            ENUMERATING[0].to_string(),
            [
                "tako panel --show --view fleet   # fleet",
                "tako panel --view orch           # orch",
                "tako panel --view git            # git",
                "tako panel --view tasks          # tasks",
                "tako panel --view diagnostics    # diagnostics",
                "tako panel --view fleets         # 打ち間違い",
            ]
            .join("\n"),
        ),
    ];
    let found = mentions(&before);

    let lists = list_mismatches(&found);
    assert!(
        lists.len() == 1 && lists[0].contains("AGENTS.md:1"),
        "5 値の候補の一覧を名指しできていない: {lists:?}"
    );
    let unknown = unknown_values(&found);
    assert!(
        unknown.len() == 1 && unknown[0].contains(&format!("{}:6", ENUMERATING[0])),
        "受理値に無い例を名指しできていない: {unknown:?}"
    );
    assert_eq!(
        missing_examples(&found, ENUMERATING[0]),
        ["references", "search"],
        "例の列挙から抜けた値を名指しできていない"
    );

    // 拾わない形: 値の無い言及・`|` の無い仮引数・別コマンドの --view
    assert!(view_mentions("`tako panel --view` のヘルプ文").is_empty());
    assert!(view_mentions("`tako panel --view <view>`").is_empty());
    assert!(view_mentions("`tako preview --view code`").is_empty());
    // 旧称は受理値なので例に使ってよい
    assert!(unknown_values(&mentions(&[(
        "x.md".to_string(),
        "`tako panel --view tmux`".to_string()
    )]))
    .is_empty());
}
