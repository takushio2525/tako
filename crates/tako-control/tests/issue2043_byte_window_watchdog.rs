//! **#2043 の番犬**: 番犬・テスト（`crates/*/tests/`）が、文字列を**固定のバイト幅**で
//! 切る添字を直書きしない。
//!
//! ## なぜ止めるのか
//!
//! 番犬は「目印の直後 400 バイト」「呼び出しの手前 1200 バイト」のように、目印の近傍を
//! 固定の幅で見る。これを `&src[at..at + 400]` と添字で書くと、幅の端が多バイト文字の途中に
//! 来た瞬間に `byte index N is not a char boundary` で panic する。番犬が読む製品コードは
//! 日本語のコメントと文言が多いので、**検査と無関係な PR が日本語を 1 行足しただけ**で
//! その PR の CI が赤くなる。実物（#2020 / PR #2037）: `agent_resume_watchdog` が
//! `if !has_children {` から 1200 バイトで切っていて、足したコメントで終端が 0x89 の途中に来た。
//! #2037 は同じ形の 2 本だけ直したので、#2043 で `crates/*/tests/` を洗い出し、12 か所を
//! `common/byte_window.rs` へ寄せた。
//!
//! ## 何を固定するか
//!
//! `crates/*/tests/` の `.rs`（`common/` を含む）の範囲添字 `[a..b]` と `.get(a..b)` のうち、
//! **幅が固定**の形を落とす。固定値 = 2 以上の整数リテラルか定数（`LOOKBACK`）。
//! 境界が識別子 1 つなら、同じ関数の手前の `let` を 1 段だけ遡って右辺で読む:
//!
//! - 前方: 終端が「起点 + 固定値」（`at..at + 400` / `at..(at + 400).min(len)` / `let end = …`）
//! - 後方: 始端が「終点 − 固定値」（`at.saturating_sub(1200)..at` / `at - 200..at`）
//! - 先頭: 先頭（か固定の位置）から固定値まで（`..80` / `..s.len().min(80)` / `17..23`）
//!
//! 通すもの:
//!
//! - `common/byte_window.rs` の `after` / `before` / `head`（端を外側へ文字境界まで広げる）
//! - 添字を `floor_char_boundary(` / `ceil_char_boundary(` で丸めてある
//! - **行の窓**: 受け手の宣言の型が `Vec` / スライス（`Vec<` / `&[` 等）・切り出しの直後が
//!   文字列には無いメソッド（`.iter()` / `.join(` 等）・起点が `enumerate()` / `position(` の
//!   添字のどれか（文字列ではないので文字境界が無い）
//! - 同じ行か 1 行上に `// 切り出し安全: <理由>`（#1746 と同じ目印。既知の固定リテラルなど）
//!
//! 手で丸める `while !s.is_char_boundary(end) { end -= 1 }` も落とす（`let mut end = idx + 1200`
//! を遡って前方の形と読む）。panic はしないが、#2037 の形は窓を**内側へ**縮める = 端にあった
//! 目印を黙って見なくなる向きなので、外側へ広げる 1 実装へ寄せる。
//!
//! 幅が変数（`fn window(s, at, width)` の `width`）の窓は固定幅ではないので見ない（既知の穴）。
//!
//! 形と行の窓の判定は `common/range_index.rs` の [`sites`]（[`Rule::FixedWindow`]）にある。
//! 製品ファイルの `#[cfg(test)]` の窓は `issue1790_byte_index_repo_watchdog` が同じ判定で見る（#1790）。
//!
//! ## 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で列挙と形の判定が実ファイルで生きていること（行の窓を形として
//! 拾ったうえで除外している件数・補助の利用件数）を、[`修正前の形を実際に落とす`] で
//! #2043 で直した 12 か所の修正前の形が `file:line` で名指しされることを、
//! [`補助は端の多バイト文字で落ちず窓を外側へ広げる`] で修正前の式が panic し補助が通ることを
//! 確かめる（注入は `cargo test` の中で完結し、リポジトリのファイルは触らない）。

use std::path::{Path, PathBuf};

// コメントと文字列を潰した眺め（バイト長と行番号が保たれる。説明文や注入の文字列を拾わない）
#[path = "common/code_view.rs"]
mod code_view;

// 範囲添字の検出は #1728 / #1746 の番犬と共有する 1 実装（#1757）。`.get(a..b)` も同じ部品で数える。
// 固定幅の窓の形と行の窓の判定も、製品ファイルの番犬（#1790）と共有するのでこの部品にある
#[path = "common/range_index.rs"]
mod range_index;

// 直す先の補助（ここでは補助自体の振る舞いを確かめる）
#[path = "common/byte_window.rs"]
mod byte_window;

use range_index::{sites, Rule, Verdict, REASON_MARK};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// `crates/*/tests/` の `.rs` を再帰で列挙する（リポジトリルートからの相対パス。並びは固定）
fn test_sources() -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries =
            std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{} が読める: {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("ディレクトリ項目").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|x| x == "rs") {
                out.push(path);
            }
        }
    }
    let root = repo_root();
    let mut files = Vec::new();
    let crates = std::fs::read_dir(root.join("crates")).expect("crates/ が読める");
    for entry in crates {
        let tests = entry.expect("crate の項目").path().join("tests");
        if tests.is_dir() {
            walk(&tests, &mut files);
        }
    }
    let mut rels: Vec<String> = files
        .iter()
        .map(|p| {
            p.strip_prefix(&root)
                .expect("リポジトリ内")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    rels.sort();
    rels
}

/// 検査本体（注入テストからも呼ぶので、読むソースは引数で受ける）。違反は `file:line — 理由`
fn offenders(raw: &str, rel: &str) -> Vec<String> {
    let code = code_view::code_view(raw);
    sites(raw, &code, Rule::FixedWindow)
        .into_iter()
        .filter(|s| s.verdict == Verdict::Offender)
        .map(|s| {
            format!(
                "{rel}:{} — 文字列を固定のバイト幅で切っている（{}: `{}`）。幅の端が多バイト文字の\
                 途中に来ると panic し、日本語を 1 行足しただけの無関係な PR の CI が赤くなる\
                 （#2020 / #2043）。`tests/common/byte_window.rs` の `after` / `before` / `head` を\
                 `#[path]` で取り込んで通す（端を外側へ文字境界まで広げる）。行の Vec なら宣言に型\
                 （`Vec<…>` / `&[…]`）を書く。既知の固定リテラル等で安全なら同じ行か 1 行上に\
                 `// {REASON_MARK} <理由>` を書く",
                s.line, s.shape, s.text
            )
        })
        .collect()
}

// --------------------------------------------------------------- テスト

#[test]
fn 番犬とテストに固定バイト幅の文字列の切り出しが無い() {
    let mut found = Vec::new();
    for rel in test_sources() {
        found.extend(offenders(&read(&rel), &rel));
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// 走査が空振りしていないこと（列挙が壊れて 0 件・形の判定が死んで 0 件で緑になるのを防ぐ）
#[test]
fn 走査が空振りしていない() {
    let files = test_sources();
    for must in [
        "crates/tako-control/tests/common/byte_window.rs",
        "crates/tako-control/tests/agent_resume_watchdog.rs",
        "crates/tako-core/tests/pty_exit_drain.rs",
        "crates/tako-cli/tests/issue1758_broken_pipe.rs",
    ] {
        assert!(
            files.iter().any(|f| f == must),
            "{must} が列挙に無い（crates/*/tests/ の列挙が壊れている）"
        );
    }
    assert!(files.len() >= 300, "列挙が少なすぎる: {} 件", files.len());

    let mut row = 0usize;
    let mut reason = 0usize;
    let mut helper_users = 0usize;
    for rel in &files {
        let raw = read(rel);
        let code = code_view::code_view(&raw);
        if code.contains("byte_window::") {
            helper_users += 1;
        }
        for s in sites(&raw, &code, Rule::FixedWindow) {
            match s.verdict {
                Verdict::RowWindow => row += 1,
                Verdict::Reason => reason += 1,
                _ => {}
            }
        }
    }
    // 行の窓（`lines[i..(i + 30).min(lines.len())]` 等）は 2026-10-16 の実測で 33 件。
    // 形の判定が死ぬとここが 0 になる
    assert!(
        row >= 20,
        "行の窓を形として拾えていない（{row} 件）。形の判定が壊れている"
    );
    // 理由コメントで通した固定リテラル（issue1679 の 2 件・platform_parity の 1 件・この番犬の注入 4 件）
    assert!(reason >= 7, "理由コメントの窓が {reason} 件しか見えない");
    // #2043 で寄せた 10 ファイル（＋この番犬）
    assert!(
        helper_users >= 11,
        "byte_window の利用が {helper_users} ファイルしか見えない（寄せた先が戻された?）"
    );
}

#[test]
fn 修正前の形を実際に落とす() {
    // #2043 で直した 12 か所の修正前の形（行はほぼ原文のまま）。各 1 件を名指しすること
    let cases: &[(&str, &str, usize)] = &[
        (
            "① agent_resume（#2037 の手の丸め）",
            "fn a() {\n    let source = read(MAIN);\n    let idx = source.find(\"x\").unwrap();\n    let mut end = (idx + 1200).min(source.len());\n    while !source.is_char_boundary(end) {\n        end -= 1;\n    }\n    let block = &source[idx..end];\n}\n",
            8,
        ),
        (
            "② agy の後方 1200",
            "fn b() {\n    let src = production(\"src/dispatch.rs\");\n    let call = src.find(\"x\").unwrap();\n    let before = &src[call.saturating_sub(1200)..call];\n}\n",
            4,
        ),
        (
            "③ issue1857 の前方 400（フィールド）",
            "fn c() {\n    let mcp = Views::of(MCP_REQUEST, &read(MCP_REQUEST));\n    let persist = mcp.text.find(\"x\").unwrap();\n    let window = &mcp.text[persist..(persist + 400).min(mcp.text.len())];\n}\n",
            4,
        ),
        (
            "④ issue1554 の先頭 200",
            "fn d(main: &str) {\n    let call = main.split(\"x\").nth(1).unwrap_or_default();\n    assert!(call[..call.len().min(200)].contains(\"persist_diag(\"));\n}\n",
            3,
        ),
        (
            "⑤ issue763 の定数で遡る",
            "fn e(src: &str) {\n    const LOOKBACK: usize = 200;\n    let at = src.find(\"x\").unwrap();\n    let back = &src[at.saturating_sub(LOOKBACK)..at];\n}\n",
            4,
        ),
        (
            "⑥ remote_link の let を遡る前方 240",
            "fn f(production: String) {\n    let at = production.find(\"x\").unwrap();\n    let window_end = (at + 240).min(production.len());\n    let window = &production[at..window_end];\n}\n",
            4,
        ),
        (
            "⑦ remote_guide の見出し 80",
            "fn g(body: &str) {\n    for line in body.lines() {\n        println!(\"{}\", &line[..line.len().min(80)]);\n    }\n}\n",
            3,
        ),
        (
            "⑧ .get で取ると端が文字の途中の日に窓が空になる",
            "fn h(src: &str) {\n    let at = src.find(\"x\").unwrap();\n    let window = src.get(at..at + 400).unwrap_or(\"\");\n}\n",
            3,
        ),
        (
            "⑨ 後方の引き算",
            "fn i(src: &str) {\n    let at = src.find(\"x\").unwrap();\n    let back = &src[at - 200..at];\n}\n",
            3,
        ),
        (
            "⑩ 固定の位置（#1756 の形）",
            "fn j(hex: &str) {\n    let r = &hex[0..2];\n}\n",
            2,
        ),
    ];
    for (label, src, line) in cases {
        let found = offenders(src, "注入.rs");
        let want = format!("注入.rs:{line} —");
        assert!(
            found.iter().any(|f| f.starts_with(&want)),
            "{label} を名指しできなかった（`{want}` が無い）: {found:?}"
        );
        assert_eq!(found.len(), 1, "{label} 以外まで拾った: {found:?}");
    }

    // 安全な形・行の窓は通す（常に落ちる無意味な番犬になっていないこと）
    let ok = "
fn ok(src: &str, lines: &[&str], at: usize) {
    let idx = src.find(\"x\").unwrap();
    let a = byte_window::after(src, idx, 1200);
    let b = byte_window::before(src, idx, 700);
    let c = byte_window::head(src, 80);
    let d = &src[idx..src.ceil_char_boundary(idx + 400)];
    let rows: Vec<&str> = src.lines().collect();
    let e = rows[idx..(idx + 30).min(rows.len())].join(\"\\n\");
    let f = &lines[at.saturating_sub(5)..=at];
    for (i, l) in rows.iter().enumerate() {
        let g = &rows[i..(i + 3).min(rows.len())];
    }
    match fn_window(src) {
        Some((at, window)) => {
            let near = window[at..(at + 4).min(window.len())].iter().any(|l| l.is_empty());
        }
        None => {}
    }
    let rest = &src[idx + 3..];
    let mut j = idx + 2;
    let name = &src[idx + 2..j];
    let close = src.find(\"}\").unwrap();
    let h = &src[idx..close + 1];
    // 切り出し安全: 固定のリテラル行（位置そのものの検証）
    let k = &src[17..23];
    let s = \"&src[at..at + 400] は文字列の中なので数えない\";
    // &src[at..at + 400] は注釈の中なので数えない
}
";
    let found = offenders(ok, "注入.rs");
    assert!(found.is_empty(), "安全な形まで落とした: {found:?}");

    // 理由コメントは 2 行以上離すと効かない（そばに置く規約）
    let far = "fn far(src: &str) {\n    // 切り出し安全: 離れた理由\n    let x = 1;\n    let k = &src[17..23];\n}\n";
    let found = offenders(far, "注入.rs");
    assert!(
        found.iter().any(|f| f.starts_with("注入.rs:4 —")),
        "離れた理由コメントで通してしまった: {found:?}"
    );
}

/// 修正前の式は端の多バイト文字で panic し、補助は通って窓を**外側へ**広げる
#[test]
fn 補助は端の多バイト文字で落ちず窓を外側へ広げる() {
    // 前方: 「目」が 399..402 に跨がる（400 は文字の途中）
    let text = format!("{}目印", "a".repeat(399));
    let idx = 0usize;
    // 切り出し安全: 修正前の式をわざと panic させる注入（catch_unwind の中）
    let old = std::panic::catch_unwind(|| text[idx..(idx + 400).min(text.len())].len());
    assert!(
        old.is_err(),
        "修正前の前方の式が panic しない（注入が効いていない）"
    );
    // #2037 の手の丸め（内側へ縮める）は「目」を窓から落とす
    let mut end = (idx + 400).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    // 切り出し安全: #2037 の手の丸めをそのまま再現して、縮む向きを確かめる
    let shrunk = &text[idx..end];
    assert!(
        !shrunk.contains('目'),
        "内側へ丸めると端の文字が落ちる前提が崩れた"
    );
    let window = byte_window::after(&text, 0, 400);
    assert_eq!(window.len(), 402, "終端は文字境界まで後ろへ広がる");
    assert!(
        window.ends_with('目'),
        "端に跨がる文字を窓に含める: {window:?}"
    );

    // 後方: 「目」が 0..3 に跨がり、起点の 1200 バイト手前（2）は文字の途中
    let text = format!("目{}X", "a".repeat(1199));
    let at = text.find('X').unwrap();
    // 切り出し安全: 修正前の式をわざと panic させる注入（catch_unwind の中）
    let old = std::panic::catch_unwind(|| text[at.saturating_sub(1200)..at].len());
    assert!(
        old.is_err(),
        "修正前の後方の式が panic しない（注入が効いていない）"
    );
    let window = byte_window::before(&text, at, 1200);
    assert!(window.starts_with('目'), "始端は文字境界まで前へ広がる");
    assert_eq!(window.len(), at);

    // 先頭: 4 バイト目は「い」の途中
    let text = "あいう";
    // 切り出し安全: 修正前の式をわざと panic させる注入（catch_unwind の中）
    let old = std::panic::catch_unwind(|| text[..text.len().min(4)].len());
    assert!(
        old.is_err(),
        "修正前の先頭の式が panic しない（注入が効いていない）"
    );
    assert_eq!(byte_window::head(text, 4), "あい");

    // 境界: 幅が末尾 / 先頭を越えたら文字列の端で止まる・幅 0 は空
    assert_eq!(byte_window::after("abc", 1, 10_000), "bc");
    assert_eq!(byte_window::before("abc", 2, 10_000), "ab");
    assert_eq!(byte_window::after("abc", 3, 5), "");
    assert_eq!(byte_window::head("目", 0), "");
    assert_eq!(byte_window::after("a目b", 1, 1), "目");

    // エラー経路: 起点が文字の途中なら補助の名指しで落ちる（黙って丸めない）
    let err = std::panic::catch_unwind(|| byte_window::after("目", 1, 1).len())
        .expect_err("起点が文字の途中なら落ちる");
    let msg = err.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(msg.contains("#2043"), "補助の名指しで落ちていない: {msg}");
}
