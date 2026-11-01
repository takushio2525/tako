//! #2230 の番犬: ファイルの中身を解釈した誤りの理由に serde の素の文言を使わない
//!
//! serde_json / serde_yaml の文言は型が違うと `invalid type: string "<ファイルの値>"` のように
//! 読んだ値（YAML はそこまでのキーの道筋も）を引用する（実測）。読む口の理由は persist.log
//! （`migrations::record` の `移行できず:` / settings の申告 / `[projects add]` / `[adopt]` /
//! `[master]` の行 / 引き継ぎの移行の警告）・応答の `warnings`・CLI の `警告:` 行・master の
//! system prompt へ**同じ文字列のまま**流れる。流れ先は多く間接的なので、**作る所**で中身を落とす
//! （`tako_core::load_issue::{json_error_reason, yaml_error_reason}` = 種類と行・列だけ）。
//!
//! 1. **素の文言の禁止**: 本番コードで `serde_json::from_*` / `serde_yaml::from_*` の誤りを
//!    `{e}` / `{e:?}` / `e.to_string()` / 位置引数（`, e)`）で文字列にしている所を file:line で
//!    名指す。ファイルでない入力（IPC・HTTP・子プロセスの出力・CLI / MCP の引数）の誤りは
//!    値を引用しても渡した本人へ返るだけなので、[`ALLOWED`] に理由つきで載せたものだけ通す
//!    （使われなくなった許可も落とす = 表が黙って膨らまない）
//! 2. **口の存在**: persist.log へ流れる主な口（移行の validate 2 本・settings の申告・
//!    継承元の default.yaml）が中身を落とす関数を呼んでいる（別名の関数へ逃がした回帰も止める）
//!
//! 範囲取りは `common/production_range.rs`（テスト領域を見ない）。誤りの束縛と式の範囲は
//! コメントと文字列を潰した眺め（`code_view`）で数え、`{e}` は文字列を残した眺め
//! （`without_comments`）で見る（どちらもバイト位置が原文と同じ）。
//! 検出力は [`植えた回帰を名指せる`] と [`本物の口へ植えた回帰を名指せる`] で固定する。

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, without_comments};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// 解釈の呼び出し（`from_str` / `from_slice` / `from_reader` / `from_value`）
const SITES: &[&str] = &["serde_json::from_", "serde_yaml::from_"];

/// 誤りを束縛する書き方（`|X|` の閉包と `Err(X)` の腕）
const BINDERS: &[&str] = &[
    "map_err(|",
    "map_err(move |",
    "unwrap_or_else(|",
    "or_else(|",
    "inspect_err(|",
];

/// 素の文言を使ってよい所（**ファイルでない入力**だけ）。`needle` は違反の行に含まれる文字列
struct Allow {
    file: &'static str,
    needle: &'static str,
    why: &'static str,
}

const ALLOWED: &[Allow] = &[
    Allow {
        file: "crates/tako-core/src/claude_mod.rs",
        needle: "報告を読めない",
        why: "mod の報告（IPC の引数）。誤りは送った mod へ返すだけで診断ログへ書かない",
    },
    Allow {
        file: "crates/tako-cli/src/ipc.rs",
        needle: "Reason::ReplyUnreadable(",
        why: "GUI からの IPC 応答（ファイルではない）",
    },
    Allow {
        file: "crates/tako-cli/src/main.rs",
        needle: "RemoteRefusal::SetupAnswersJsonInvalid(",
        why: "利用者がいま渡した --answers（CLI の引数）。誤りは同じ画面へ返る（#2365 で文を RemoteRefusal へ移した）",
    },
    Allow {
        file: "crates/tako-cli/src/main.rs",
        needle: "Reason::ModReportNotJson(",
        why: "標準入力から受けた報告（呼び手がいま渡した入力）",
    },
    Allow {
        file: "crates/tako-cli/src/mcp_bridge.rs",
        needle: "Reason::NotJson(",
        why: "MCP クライアントが送った 1 行（stdio の入力）",
    },
    Allow {
        file: "crates/tako-cli/src/mcp_bridge.rs",
        needle: "JSON として解釈できない",
        why: "カタログ係が受けた 1 行（親のブリッジが stdin へ渡す MCP の入力）",
    },
    Allow {
        file: "crates/tako-control/src/agents.rs",
        // #2365: 断りは `AgentScanError::NotJson`（日英の文は型が組む）
        needle: "AgentScanError::NotJson(",
        why: "`claude agents --json` の出力（子プロセスの標準出力）",
    },
    Allow {
        file: "crates/tako-control/src/agent_models.rs",
        needle: "JSON として読めない",
        why: "エージェント CLI のモデル一覧の出力（子プロセスの標準出力）",
    },
    Allow {
        file: "crates/tako-control/src/dispatch.rs",
        needle: "SetupRefusal::AnswersMalformed(",
        why: "setup の answers（IPC / MCP の引数。#2365 で文を SetupRefusal へ移した）",
    },
    Allow {
        file: "crates/tako-control/src/remote.rs",
        needle: "応答を解釈できない",
        why: "リモート / daemon の HTTP・IPC の応答",
    },
    Allow {
        file: "crates/tako-control/src/remote.rs",
        needle: "RemoteRefusal::DaemonResponseParse(",
        why: "daemon の admin API の HTTP の応答（ファイルではない。#2365 で文を RemoteRefusal へ移した）",
    },
    Allow {
        file: "crates/tako-control/src/remote.rs",
        needle: "Reason::ReplyUnreadable(",
        why: "daemon が tako app から受けた IPC の応答の 1 行（ファイルではない。#2365 で Reason へ移した）",
    },
    Allow {
        file: "crates/tako-control/src/remote.rs",
        needle: "JSON パースエラー",
        why: "リモートの HTTP 要求の本文",
    },
    Allow {
        file: "crates/tako-control/src/tailscale.rs",
        needle: "tailscale whois の JSON を解釈できない",
        why: "`tailscale whois --json` の出力（子プロセスの標準出力）",
    },
    Allow {
        file: "crates/tako-control/src/tailscale.rs",
        needle: "tailscale serve status の JSON を解釈できない",
        why: "`tailscale serve status --json` の出力（子プロセスの標準出力）",
    },
    Allow {
        file: "crates/tako-control/src/ipc.rs",
        needle: "リクエストを解釈できない",
        why: "IPC の要求（クライアントが送った 1 行）。誤りは送り手へ返す",
    },
    Allow {
        file: "crates/tako-control/src/setup.rs",
        needle: "setup answers JSON が不正",
        why: "`tako setup --answers`（CLI の引数・標準入力）",
    },
    Allow {
        file: "crates/tako-control/src/acceptance_gates.rs",
        needle: "gh output のパースに失敗",
        why: "`gh pr view --json` の出力（子プロセスの標準出力）",
    },
    Allow {
        file: "crates/tako-control/src/remote_files.rs",
        needle: "JSON パースエラー",
        why: "リモートの HTTP 要求の本文",
    },
    Allow {
        file: "crates/tako-control/src/acceptance_gates.rs",
        needle: "Reason::JsonArgUnreadable(\"criteria\"",
        why: "受け入れ条件の --criteria（CLI / MCP の引数）",
    },
    Allow {
        file: "crates/tako-control/src/acceptance_gates.rs",
        needle: "Reason::CriterionKindUnreadable(",
        why: "受け入れ条件の --criteria の 1 件（CLI / MCP の引数）",
    },
    Allow {
        file: "crates/tako-control/src/acceptance_gates.rs",
        needle: "Reason::JsonArgUnreadable(\"results\"",
        why: "受け入れ条件の判定結果（CLI / MCP の引数）",
    },
    Allow {
        file: "crates/tako-control/src/mcp/request.rs",
        needle: "Reason::CannotParse(\"limits\"",
        why: "MCP の tools/call の引数",
    },
    Allow {
        file: "crates/tako-control/src/mcp/request.rs",
        needle: "Reason::ModUiArgs(",
        why: "MCP の tools/call の引数",
    },
    Allow {
        file: "crates/tako-app/src/settings_window.rs",
        needle: "advanced_parse_error",
        why: "保存前の編集バッファの検査。誤りは同じ窓の入力欄の横にだけ出す（診断ログ・応答へ流れない）",
    },
    Allow {
        file: "crates/tako-app/src/update_checker.rs",
        needle: "ParseCause::Json(",
        why: "GitHub API の応答（文は `ParseCause::Json` の日英の対が組む = #2365）",
    },
];

/// persist.log へ流れる主な口と、そこで呼ぶべき関数（関数ごと差し替える回帰を止める）
const FUNNELS: &[(&str, &str, &str)] = &[
    (
        "crates/tako-control/src/migrations.rs",
        "fn json_ok<",
        "load_issue::json_error_reason(",
    ),
    (
        "crates/tako-control/src/migrations.rs",
        "fn yaml_ok<",
        "load_issue::yaml_error_reason(",
    ),
    (
        "crates/tako-control/src/settings.rs",
        "fn load_and_report(",
        "load_issue::json_error_reason(",
    ),
    (
        "crates/tako-control/src/orchestrator/mod.rs",
        "fn read_profile_base_at(",
        // 理由は型（表示言語で組む。persist.log は日本語。#2365）。中身は種類と位置だけ = 同じ口
        "YamlReason::of(",
    ),
];

/// 素の文言を使った 1 か所
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Raw {
    file: String,
    line: usize,
    text: String,
}

impl Raw {
    fn show(&self) -> String {
        format!("{}:{}: {}", self.file, self.line, self.text)
    }
}

fn line_of(text: &str, byte: usize) -> usize {
    text[..byte].bytes().filter(|&b| b == b'\n').count() + 1
}

fn line_text(text: &str, byte: usize) -> String {
    let start = text[..byte].rfind('\n').map_or(0, |i| i + 1);
    let end = text[byte..].find('\n').map_or(text.len(), |i| byte + i);
    text[start..end].trim().to_string()
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// 解釈の呼び出しを含む文の頭（`at` の手前の `;` / `{` / `}` の直後から `at` まで）
fn stmt_head(code: &str, at: usize) -> &str {
    let start = code[..at].rfind([';', '{', '}']).map_or(0, |i| i + 1);
    &code[start..at]
}

/// 文の頭が `match <解釈>` / `if let … = <解釈>` / `let … = match` の形か（= 後ろのブロックが
/// 解釈の結果を受ける）。`if <解釈>.is_err() {` のブロックは別の処理なので範囲に入れない
fn takes_block(head: &str) -> bool {
    head.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .any(|w| w == "match" || w == "let")
}

/// `at` から始まる式の終わり（同じ深さの `;` / `,` か、外側の括弧が閉じる所。
/// `block` でなければ同じ深さの `{` でも終わる）。`match <解釈> { … }` の腕は範囲に入り、
/// 隣の腕（`Err(e) => 読み取りの誤り`）は入らない
fn expr_end(code: &str, at: usize, block: bool) -> usize {
    let mut depth = 0i32;
    for (i, b) in code.bytes().enumerate().skip(at) {
        match b {
            b'{' if depth == 0 && !block => return i,
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                if depth == 0 {
                    return i;
                }
                depth -= 1;
            }
            b';' | b',' if depth == 0 => return i,
            _ => {}
        }
    }
    code.len()
}

/// 範囲の中で誤りを束縛した名前（`|e|` / `Err(e)` / `Err(ref e)`。`_` は除く）
fn bindings(scope: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut take = |rest: &str, close: char| {
        let rest = rest.trim_start().trim_start_matches("ref ").trim_start();
        let name: String = rest
            .bytes()
            .take_while(|b| is_ident(*b))
            .map(char::from)
            .collect();
        let after = rest[name.len()..].trim_start();
        if !name.is_empty() && name != "_" && after.starts_with(close) {
            names.push(name);
        }
    };
    for binder in BINDERS {
        for (at, _) in scope.match_indices(binder) {
            take(&scope[at + binder.len()..], '|');
        }
    }
    for (at, _) in scope.match_indices("Err(") {
        let before = scope[..at].bytes().last();
        if before.is_some_and(is_ident) {
            continue;
        }
        take(&scope[at + "Err(".len()..], ')');
    }
    names.sort();
    names.dedup();
    names
}

/// 文の頭の `Err(e) =`（`if let` / `let … else` の束縛。`=>` の腕は含めない）
fn head_bindings(head: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (at, _) in head.match_indices("Err(") {
        if head[..at].bytes().last().is_some_and(is_ident) {
            continue;
        }
        let rest = head[at + "Err(".len()..]
            .trim_start()
            .trim_start_matches("ref ")
            .trim_start();
        let name: String = rest
            .bytes()
            .take_while(|b| is_ident(*b))
            .map(char::from)
            .collect();
        let after = rest[name.len()..].trim_start();
        let Some(after) = after.strip_prefix(')') else {
            continue;
        };
        let after = after.trim_start();
        if !name.is_empty() && name != "_" && after.starts_with('=') && !after.starts_with("=>") {
            out.push(name);
        }
    }
    out
}

/// `name` を素の文言にしている位置（`{e}` / `{e:?}` / `e.to_string()` / 位置引数の `, e)`）。
///
/// `{e}` は文字列の中なので `full`（文字列を残した眺め）で、ほかは `code`（文字列を潰した眺め）で
/// 探す（説明の文字列に `, e)` と書いても拾わない）。位置引数は rustfmt が折った形
/// （`"…{}",\n    e\n)`）も拾う。`&e` を関数へ渡すのは素の文言ではない
fn raw_positions(code: &str, full: &str, name: &str) -> Vec<usize> {
    let b = code.as_bytes();
    let mut out = Vec::new();
    for pat in [format!("{{{name}}}"), format!("{{{name}:")] {
        out.extend(full.match_indices(&pat).map(|(i, _)| i));
    }
    for (i, _) in code.match_indices(name) {
        let end = i + name.len();
        // 別の名前の一部（`ne` / `err_kind`）は見ない
        if (i > 0 && is_ident(b[i - 1])) || b.get(end).copied().is_some_and(is_ident) {
            continue;
        }
        let after = &code[end..];
        let prev = code[..i].trim_end().bytes().last();
        let next = after.trim_start().bytes().next();
        if after.starts_with(".to_string()")
            || (prev == Some(b',') && matches!(next, Some(b')') | Some(b',')))
        {
            out.push(i);
        }
    }
    out
}

/// 1 ファイルの素の文言（テスト領域・コメントは見ない）
fn raw_uses(rel: &str, src: &str) -> Vec<Raw> {
    let prod = production_range::scan(src).text;
    let code = code_view(&prod);
    let full = without_comments(&prod);
    let mut out = Vec::new();
    for site in SITES {
        for (at, _) in code.match_indices(site) {
            let head = stmt_head(&code, at);
            let end = expr_end(&code, at, takes_block(head));
            // `if let Err(e) = <解釈>` の束縛は呼び出しの手前にある
            let mut names = bindings(&code[at..end]);
            names.extend(head_bindings(head));
            for name in names {
                for off in raw_positions(&code[at..end], &full[at..end], &name) {
                    let pos = at + off;
                    out.push(Raw {
                        file: rel.to_string(),
                        line: line_of(&full, pos),
                        text: line_text(&full, pos),
                    });
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// 全クレートの `src` の `(相対パス, 中身)`（統合テストの `tests/` は見ない）
fn sources() -> Vec<(String, String)> {
    let root = repo_root();
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root.join("crates"))
        .expect("crates を読める")
        .flatten()
    {
        walk(&entry.path().join("src"), &mut files);
    }
    files.sort();
    let out: Vec<(String, String)> = files
        .iter()
        .map(|p| {
            let rel = p
                .strip_prefix(&root)
                .unwrap_or(p)
                .to_string_lossy()
                .replace('\\', "/");
            (rel, std::fs::read_to_string(p).unwrap_or_default())
        })
        .collect();
    assert!(
        out.len() > 100,
        "走査が壊れている（*.rs が {} 件しか見つからない）",
        out.len()
    );
    for (file, _, _) in FUNNELS {
        assert!(
            out.iter().any(|(rel, _)| rel == file),
            "{file} が見つからない（改名したならこの番犬も直す）"
        );
    }
    out
}

fn allowed(raw: &Raw) -> Option<usize> {
    ALLOWED
        .iter()
        .position(|a| a.file == raw.file && raw.text.contains(a.needle))
}

/// 全体の判定（違反・使われない許可）
fn verdict(srcs: &[(String, String)]) -> (Vec<Raw>, Vec<usize>, usize) {
    let mut bad = Vec::new();
    let mut used = vec![false; ALLOWED.len()];
    let mut seen = 0;
    for (rel, src) in srcs {
        for raw in raw_uses(rel, src) {
            seen += 1;
            match allowed(&raw) {
                Some(i) => used[i] = true,
                None => bad.push(raw),
            }
        }
    }
    let stale = (0..ALLOWED.len()).filter(|i| !used[*i]).collect();
    (bad, stale, seen)
}

#[test]
fn ファイルの中身の誤りに素の文言を使わない() {
    let (bad, stale, seen) = verdict(&sources());
    assert!(seen > 0, "許可表の入力すら見えない（走査が空振りしている）");
    assert!(
        bad.is_empty(),
        "serde の誤りを素の文言で文字列にしている（型が違うとファイルの値を引用し、persist.log・\
         応答・system prompt へ流れる = #2230）。ファイルの中身なら \
         tako_core::load_issue::json_error_reason(&e) / yaml_error_reason(&e, text) で組む。\
         ファイルでない入力なら ALLOWED に理由つきで載せる:\n{}",
        bad.iter().map(Raw::show).collect::<Vec<_>>().join("\n")
    );
    assert!(
        stale.is_empty(),
        "許可表の項目がもう使われていない（消す）: {:?}",
        stale
            .iter()
            .map(|i| format!(
                "{} {:?}（{}）",
                ALLOWED[*i].file, ALLOWED[*i].needle, ALLOWED[*i].why
            ))
            .collect::<Vec<_>>()
    );
}

/// `needle` で始まる関数の本体（コードの眺め）。見つからなければ None
fn fn_body<'a>(code: &'a str, needle: &str) -> Option<(usize, &'a str)> {
    let at = code.find(needle)?;
    let open = at + code[at..].find('{')?;
    let mut depth = 0i32;
    for (i, b) in code.bytes().enumerate().skip(open) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((at, &code[at..=i]));
                }
            }
            _ => {}
        }
    }
    None
}

/// 口が中身を落とす関数を呼んでいない所（`file:line`）
fn funnel_misses(srcs: &[(String, String)]) -> Vec<String> {
    let mut out = Vec::new();
    for (file, fn_needle, call) in FUNNELS {
        let Some((_, src)) = srcs.iter().find(|(rel, _)| rel == file) else {
            out.push(format!("{file}: 見つからない"));
            continue;
        };
        let code = code_view(&production_range::scan(src).text);
        match fn_body(&code, fn_needle) {
            None => out.push(format!(
                "{file}: `{fn_needle}` が見つからない（走査が空振り）"
            )),
            Some((at, body)) if !body.contains(call) => out.push(format!(
                "{file}:{} `{fn_needle}` が `{call}` を呼んでいない（persist.log へ流れる理由に \
                 中身が載りうる = #2230）",
                line_of(&code, at)
            )),
            Some(_) => {}
        }
    }
    out
}

#[test]
fn persist_logへ流れる口は中身を落とす() {
    let misses = funnel_misses(&sources());
    assert!(misses.is_empty(), "{misses:#?}");
}

/// 検出力: 素の文言の書き方を植えると file:line で名指す（テスト・コメント・隣の腕は見ない）
#[test]
fn 植えた回帰を名指せる() {
    let planted = "fn a(t: &str) -> Result<S, String> {\n    serde_json::from_str(t).map_err(|e| format!(\"x: {e}\"))\n}\n\
fn b(t: &str) -> Result<S, String> {\n    serde_yaml::from_str(t).map_err(|err| err.to_string())\n}\n\
fn c(t: &str) -> String {\n    match serde_json::from_str::<S>(t) {\n        Ok(_) => String::new(),\n        Err(e) => format!(\"{:?}\", e),\n    }\n}\n\
fn d(t: &str) -> Option<String> {\n    if let Err(ref why) = serde_yaml::from_str::<S>(t) {\n        return Some(format!(\"{why:?}\"));\n    }\n    None\n}\n\
fn e(t: &str) -> Result<S, String> {\n    serde_yaml::from_str(t).map_err(|e| {\n        format!(\n            \"x: {}\",\n            e\n        )\n    })\n}\n";
    let found: Vec<String> = raw_uses("x.rs", planted)
        .iter()
        .map(|r| format!("{}:{}", r.file, r.line))
        .collect();
    assert_eq!(
        found,
        vec!["x.rs:2", "x.rs:5", "x.rs:10", "x.rs:15", "x.rs:23"]
    );

    let clean = "// serde_json::from_str(t).map_err(|e| e.to_string())\n\
fn a(t: &str) -> Result<S, String> {\n    serde_json::from_str(t).map_err(|e| tako_core::load_issue::json_error_reason(&e))\n}\n\
fn b(t: &str) -> String {\n    match serde_json::from_str::<S>(t) {\n        Err(e) => format!(\"{} 行 {} 列\", e.line(), e.column()),\n        Ok(_) => String::new(),\n    }\n}\n\
fn c(p: &Path) -> Result<V, String> {\n    let v = match std::fs::read_to_string(p) {\n        Ok(s) => serde_json::from_str(&s).map_err(|e| json_error_reason(&e))?,\n        Err(e) => return Err(format!(\"読めない: {e}\")),\n    };\n    Ok(v)\n}\n\
#[cfg(test)]\nmod tests {\n    fn t(x: &str) { let _ = serde_json::from_str::<S>(x).map_err(|e| e.to_string()); }\n}\n";
    assert_eq!(raw_uses("x.rs", clean), Vec::<Raw>::new());
}

/// 本物の口へ素の文言を植えると、走査と口の確認の両方が file:line で名指す
#[test]
fn 本物の口へ植えた回帰を名指せる() {
    let base = sources();
    // (注入先, 注入元, 注入後, 注入後のうち違反として名指されるべき所)
    for (file, from, to, marker) in [
        (
            "crates/tako-control/src/migrations.rs",
            ".map_err(|e| tako_core::load_issue::json_error_reason(&e))",
            ".map_err(|e| e.to_string())",
            "e.to_string()",
        ),
        (
            "crates/tako-control/src/migrations.rs",
            ".map_err(|e| tako_core::load_issue::yaml_error_reason(&e, text))",
            ".map_err(|e| format!(\"{e:?}\"))",
            "{e:?}",
        ),
        (
            "crates/tako-control/src/settings.rs",
            "let reason = tako_core::load_issue::json_error_reason(&e);",
            "let reason = e.to_string();",
            "e.to_string()",
        ),
        (
            "crates/tako-control/src/orchestrator/mod.rs",
            "serde_yaml::from_str(&text).map_err(|e| UnreadableCause::Parse(YamlReason::of(&e, &text)))",
            "serde_yaml::from_str(&text).map_err(|e| UnreadableCause::Read(e.to_string()))",
            "e.to_string()",
        ),
        // rustfmt が折った位置引数（`"…{}",\n e\n)`）も拾う（#2365 で projects.yaml・ledger.yaml・workers.yaml の
        // 口が型の断りへ移ったので、同じ形の残る設定の移行（projects.yaml の読み直し）の口へ植える）
        (
            "crates/tako-control/src/migrations.rs",
            "\"projects.yaml として読めない: {}\",\n                tako_core::load_issue::yaml_error_reason(&e, text)",
            "\"projects.yaml として読めない: {}\",\n                e",
            "\n                e",
        ),
    ] {
        let mut injected = base.clone();
        let (_, src) = injected
            .iter_mut()
            .find(|(rel, _)| rel == file)
            .expect("注入先がある");
        assert_eq!(
            src.matches(from).count(),
            1,
            "{file} に注入元 `{from}` が 1 か所だけ無い（直したならこの表も直す）"
        );
        let at = src.find(from).expect("注入元がある");
        *src = src.replacen(from, to, 1);
        // 目印の先頭の空白・改行を飛ばした所が違反の行
        let lead = marker.len() - marker.trim_start().len();
        let line = line_of(src, at + to.find(marker).expect("目印が注入後にある") + lead);
        let (bad, _, _) = verdict(&injected);
        assert!(
            bad.iter().any(|r| r.file == file && r.line == line),
            "{file}:{line} への注入（`{to}`）を名指せない: {:#?}",
            bad.iter().map(Raw::show).collect::<Vec<_>>()
        );
    }
    // 関数ごと別名へ逃がした回帰は口の確認が名指す
    let mut injected = base.clone();
    let (_, src) = injected
        .iter_mut()
        .find(|(rel, _)| rel == "crates/tako-control/src/settings.rs")
        .expect("settings.rs がある");
    *src = src.replacen(
        "tako_core::load_issue::json_error_reason(&e)",
        "raw_reason(&e)",
        1,
    );
    let misses = funnel_misses(&injected);
    assert!(
        misses
            .iter()
            .any(|m| m.starts_with("crates/tako-control/src/settings.rs:")),
        "口の確認が settings.rs の差し替えを名指せない: {misses:#?}"
    );
}
