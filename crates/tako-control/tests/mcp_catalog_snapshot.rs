//! MCP 公開カタログの完全スナップショット。
//!
//! ツール名だけでなく説明文・inputSchema・公開順も公開契約として固定し、
//! 構造整理で意図せず変わらないことを `cargo test` だけで検出する。

use std::collections::HashSet;
use std::path::Path;

use tako_control::mcp;

const SNAPSHOT: &str = include_str!("../testdata/mcp_tools_full_snapshot.json");

fn canonicalize(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(canonicalize).collect())
        }
        serde_json::Value::Object(values) => {
            let mut entries: Vec<_> = values.into_iter().collect();
            entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
            serde_json::Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonicalize(value)))
                    .collect(),
            )
        }
        value => value,
    }
}

/// 実行プラットフォームで変わる語のプレースホルダ。
///
/// リンクを開く打鍵は macOS = `⌘+クリック` / Windows = `Ctrl+クリック`（#763）。
/// カタログは**実行 OS の打鍵**を申告するのが正しいので、スナップショットは
/// そこだけ伏せて比べる（伏せないと Windows ランナーの `cargo test` が必ず割れる）
const LINK_CLICK_PLACEHOLDER: &str = "<link-click>";

/// 実行 OS に追従する語を伏せる。**1 件も見つからなければ失敗**
/// （追従をやめて直書きへ戻したら、それはそれで検出したい）
fn mask_platform_words(rendered: String) -> String {
    let link_click =
        tako_core::platform::keys::link_click(tako_core::platform::support::Platform::current());
    assert!(
        rendered.contains(&link_click),
        "カタログに「{link_click}」が 1 件も無い。\
         リンクの打鍵の申告が実行プラットフォームに追従していない（#763）"
    );
    rendered.replace(&link_click, LINK_CLICK_PLACEHOLDER)
}

fn rendered_catalog() -> String {
    let catalog = canonicalize(serde_json::Value::Array(mcp::tools()));
    mask_platform_words(
        serde_json::to_string_pretty(&catalog).expect("MCP カタログを JSON 化できる") + "\n",
    )
}

#[test]
fn mcp公開カタログが完全スナップショットと一致する() {
    let actual = rendered_catalog();
    if std::env::var_os("TAKO_UPDATE_MCP_SNAPSHOT").is_some() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/mcp_tools_full_snapshot.json");
        std::fs::write(&path, &actual)
            .unwrap_or_else(|e| panic!("スナップショットを更新できない {}: {e}", path.display()));
        return;
    }

    assert_eq!(
        actual, SNAPSHOT,
        "MCP 公開カタログが変化した。挙動変更でないことを確認し、意図した変更の場合だけ\
         `TAKO_UPDATE_MCP_SNAPSHOT=1 cargo test -p tako-control \
         --test mcp_catalog_snapshot` で更新する"
    );
}

#[test]
fn mcpツール名は空でなく重複しない() {
    let tools = mcp::tools();
    let mut names = HashSet::with_capacity(tools.len());
    for tool in &tools {
        let name = tool["name"]
            .as_str()
            .expect("全 MCP ツールに文字列の name が必要");
        assert!(!name.is_empty(), "MCP ツール名は空にできない");
        assert!(names.insert(name), "MCP ツール名が重複している: {name}");
    }
}

#[test]
fn snapshotのobjectキー順はserde_jsonのfeatureに依存しない() {
    let mut object = serde_json::Map::new();
    object.insert("z".into(), serde_json::json!({ "y": 1, "a": 2 }));
    object.insert("a".into(), serde_json::json!(0));

    let rendered = serde_json::to_string(&canonicalize(object.into())).unwrap();
    assert_eq!(rendered, r#"{"a":0,"z":{"a":2,"y":1}}"#);
}

/// 文字列中の `#` + 2〜4 桁の数字（Issue 番号の形）をすべて拾う
fn issue_refs(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'#' {
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        // 5 桁以上は Issue 番号ではない（誤検出を避ける）
        if (2..=4).contains(&(end - start)) && !bytes.get(end).is_some_and(u8::is_ascii_digit) {
            out.push(text[i..end].to_string());
        }
        i = end.max(i + 1);
    }
    out
}

/// JSON 値の中の**文字列**をすべて辿る（キーも値も、AI が受け取る全文）
fn collect_strings(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Array(items) => items.iter().for_each(|v| collect_strings(v, out)),
        serde_json::Value::Object(map) => map.values().for_each(|v| collect_strings(v, out)),
        _ => {}
    }
}

/// AI 向けの説明文（`tools/list` の応答）に Issue 番号を書かない。
///
/// MCP を繋いだエージェントは 1 本残らず起動した瞬間にこの全文を受け取るが、
/// **AI は番号から Issue を引けない**ので `#1234` はトークンを食うだけで情報を運ばない。
/// 根拠（どの Issue で入った説明か）は `catalog.rs` のソースコメント
/// `// 出自: #…` に残してあるので、情報は失われない。
#[test]
fn mcp説明文にissue番号を書かない() {
    let mut problems = Vec::new();
    for tool in mcp::tools() {
        let name = tool["name"].as_str().unwrap_or("?").to_string();
        let mut strings = Vec::new();
        collect_strings(&tool, &mut strings);
        for s in strings {
            let refs = issue_refs(&s);
            if !refs.is_empty() {
                problems.push(format!("  {name}: {} — {}", refs.join(" / "), s.trim()));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "MCP カタログの説明文に Issue 番号が {} 箇所ある。AI は番号を引けないので\
         説明文からは落とし、根拠は catalog.rs のソースコメント（`// 出自: #…`）へ書くこと\
         （Issue #1540）:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn issue番号の拾い方が桁でぶれない() {
    assert_eq!(issue_refs("#1540 と #99 を見る"), vec!["#1540", "#99"]);
    // 1 桁は拾わない（見出しの `#` や色指定と紛れる）／5 桁以上も拾わない
    assert!(issue_refs("#5 / #12345 / # 123 / abc#x").is_empty());
}

// ---------------------------------------------------------------------------
// 行の折り返しで語が空白なしに繋がる（Issue #1905）
// ---------------------------------------------------------------------------
//
// catalog.rs の説明文は Rust の文字列の行末 `\` で折り返す。`\` の継続は**次の行の先頭の
// 空白を捨てる**ので、`…を返す\` + 改行 + `    pane_id …` は「…を返すpane_id」になる。
// 説明文は和文と英字の間に半角空白を置いて書いている（「を JSON で返す」）ので、
// 継ぎ目だけ詰まった形はソースを読んでも気づけない（#1904 で 8 か所直した後にも残っていた）。
// 英字どうしの継ぎ目は語そのものが繋がる（`foo\` + `bar` = 「foobar」）のでなお悪い。

const CATALOG_SOURCE: &str = "src/mcp/catalog.rs";

/// 和文の文字（ひらがな・カタカナ・漢字）。中黒「・」と全角の約物は含めない
/// （「（tab と排他）」「・pane」のように約物と英字は空白なしで並べる）
fn is_wabun(c: char) -> bool {
    matches!(c,
        '\u{3041}'..='\u{309F}'
        | '\u{30A0}'..='\u{30FA}'
        | '\u{30FC}'..='\u{30FF}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{4E00}'..='\u{9FFF}'
        | '々')
}

/// 英字の語の文字（キー名・引数名・値に使う字）
fn is_eiji(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// 継ぎ目の左右が空白なしに並んではいけない組か
fn glued(left: char, right: char) -> bool {
    (is_wabun(left) && is_eiji(right))
        || (is_eiji(left) && is_wabun(right))
        || (is_eiji(left) && is_eiji(right))
}

/// ソースの行末 `\`（文字列の継続）の継ぎ目で、前の行の最後の文字と次の行の最初の文字が
/// 空白なしに繋がる箇所を `(行番号, 継ぎ目の前後)` で返す。1 つ目は見た継ぎ目の数
/// （走査が空振りしていないことを確かめる）
fn glued_continuations(source: &str) -> (usize, Vec<(usize, String)>) {
    let lines: Vec<&str> = source.lines().collect();
    let mut checked = 0usize;
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        let Some(body) = line.strip_suffix('\\') else {
            continue;
        };
        // `\\` で終わる行は継続ではなく、エスケープした `\` そのもの
        if body.chars().rev().take_while(|c| *c == '\\').count() % 2 == 1 {
            continue;
        }
        let mut rev = body.chars().rev();
        let Some(left) = rev.next() else {
            continue;
        };
        checked += 1;
        // 直前の文字がエスケープの一部（`\n` / `\"` …）なら、描かれるのはその英字ではない
        if rev.take_while(|c| *c == '\\').count() % 2 == 1 {
            continue;
        }
        // Rust は継続の後ろの空白（改行も）をすべて捨てる = 次に空白でない文字が継ぎ目の右
        let Some((next, right)) = lines[i + 1..]
            .iter()
            .enumerate()
            .find_map(|(k, l)| l.trim_start().chars().next().map(|c| (i + 1 + k, c)))
        else {
            continue;
        };
        if !glued(left, right) {
            continue;
        }
        let head: String = body
            .chars()
            .rev()
            .take(10)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let tail: String = lines[next].trim_start().chars().take(12).collect();
        out.push((i + 1, format!("「…{head}」+「{tail}…」")));
    }
    (checked, out)
}

fn catalog_source() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(CATALOG_SOURCE);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("catalog.rs を読めない {}: {e}", path.display()))
}

#[test]
fn catalogの折り返しで和文と英字が空白なしに繋がらない() {
    let (checked, glued) = glued_continuations(&catalog_source());
    // 走査が空振りすれば無意味に緑になる（説明文の継ぎ目は 800 を超える）
    assert!(checked > 500, "継ぎ目を {checked} 個しか見つけられていない");
    let lines: Vec<String> = glued
        .iter()
        .map(|(line, at)| format!("  crates/tako-control/{CATALOG_SOURCE}:{line}: {at}"))
        .collect();
    assert!(
        lines.is_empty(),
        "行末 `\\` の継ぎ目で語が空白なしに繋がる箇所が {} 個ある。継続は次の行の先頭の空白を\
         捨てるので、前の行の `\\` の直前に半角空白を置く（Issue #1905）:\n{}",
        lines.len(),
        lines.join("\n")
    );
}

#[test]
fn 折り返しの繋がりを行番号で名指せる() {
    let src = concat!(
        "        \"description\": \"返り値を返す\\\n", // 2: 和文 + 英字
        "            pane_id と tab を返す。tab\\\n",  // 3: 英字 + 和文
        "            から読む。foo\\\n",               // 4: 英字 + 英字
        "            bar を見る。改行は\\n\\\n",       // 5: `\n` の後は改行（拾わない）
        "            next を見る。空白を置く \\\n",    // 6: 空白あり（拾わない）
        "            pane（括弧）\\\n",                // 7: 約物 + 英字（拾わない）
        "            tab・\\\n",                       // 8: 中黒 + 英字（拾わない）
        "            pane と\\\n",                     // 9: 空行を挟んでも次の文字が右
        "\n",
        "            tab を返す\\\\\n", // 11: `\\` で終わる = 継続ではない
        "            x\",\n",
        "        // コメントの行末は見ない\\\n",
        "        y\n",
    );
    let src = format!("\n{src}");
    let (checked, glued) = glued_continuations(&src);
    let lines: Vec<usize> = glued.iter().map(|(line, _)| *line).collect();
    assert_eq!(lines, vec![2, 3, 4, 9], "{glued:?}");
    assert_eq!(checked, 8, "継ぎ目の数え方が変わった: {glued:?}");
    assert!(
        glued[0].1.contains("返り値を返す") && glued[0].1.contains("pane_id"),
        "{glued:?}"
    );

    // 実物への注入: catalog.rs の空白つきの継ぎ目 1 つから空白を抜くと、その行で名指す
    let real = catalog_source();
    let mut lines: Vec<String> = real.lines().map(str::to_string).collect();
    let at = lines
        .iter()
        .position(|l| {
            !l.trim_start().starts_with("//")
                && l.strip_suffix(" \\")
                    .and_then(|body| body.chars().next_back())
                    .is_some_and(is_wabun)
        })
        .expect("和文 + 空白 + `\\` で終わる継ぎ目が catalog.rs に 1 つも無い");
    let next_is_eiji = lines[at + 1..]
        .iter()
        .find_map(|l| l.trim_start().chars().next())
        .is_some_and(is_eiji);
    if !next_is_eiji {
        // 次の行を英字で始める形へ寄せる（注入の形を固定するため）
        lines.insert(at + 1, "                pane_id を返す\\".into());
    }
    let stripped = lines[at]
        .strip_suffix(" \\")
        .expect("空白つきの継ぎ目")
        .to_string();
    lines[at] = format!("{stripped}\\");
    let (_, glued) = glued_continuations(&(lines.join("\n") + "\n"));
    assert_eq!(
        glued.iter().map(|(line, _)| *line).collect::<Vec<_>>(),
        vec![at + 1],
        "注入した継ぎ目（{}:{}）を名指せない: {glued:?}",
        CATALOG_SOURCE,
        at + 1
    );
}

/// 実行 OS で変わる語を両方の OS の表記へ置き換えた全文（リンクの打鍵は macOS = `⌘+クリック` /
/// Windows = `Ctrl+クリック`）。「パスリンクの{打鍵}」は macOS では「の⌘」で通り、Windows でだけ
/// 「のCtrl」と詰まるので、どちらの OS で走らせても両方の形を見る
fn platform_variants(s: &str) -> Vec<String> {
    use tako_core::platform::keys::link_click;
    use tako_core::platform::support::Platform;
    let current = link_click(Platform::current());
    [Platform::MacOs, Platform::Windows]
        .into_iter()
        .map(|p| s.replace(&current, &link_click(p)))
        .collect()
}

/// 和文と英字が空白なしに隣り合う 2 文字を返す
fn adjacent_wabun_eiji(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    chars
        .windows(2)
        .filter(|w| (is_wabun(w[0]) && is_eiji(w[1])) || (is_eiji(w[0]) && is_wabun(w[1])))
        .map(|w| format!("{}{}", w[0], w[1]))
        .collect()
}

/// 描かれた全文でも和文と英字が空白なしに隣り合わない（継ぎ目以外 = 1 行の中に直書きした形や、
/// 他のファイルの正本から生成した案内文も見る）。バッククオートは英字に数えない
/// （「`ラベル=本文`」のようなコードの中の和文は詰めて書く）
#[test]
fn mcp説明文で和文と英字が空白なしに隣り合わない() {
    let mut problems = Vec::new();
    for tool in mcp::tools() {
        let name = tool["name"].as_str().unwrap_or("?").to_string();
        let mut strings = Vec::new();
        collect_strings(&tool, &mut strings);
        for s in strings {
            let mut found: Vec<String> = platform_variants(&s)
                .iter()
                .flat_map(|v| adjacent_wabun_eiji(v))
                .collect();
            found.sort();
            found.dedup();
            problems.extend(
                found
                    .into_iter()
                    .map(|pair| format!("  {name}: 「{pair}」")),
            );
        }
    }
    assert!(
        problems.is_empty(),
        "MCP カタログの説明文で和文と英字が空白なしに隣り合う箇所が {} 個ある\
         （和文と英字の間は半角空白。Issue #1905）:\n{}",
        problems.len(),
        problems.join("\n")
    );
}
