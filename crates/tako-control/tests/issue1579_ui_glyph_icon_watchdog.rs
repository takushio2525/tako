//! 番犬: **UI の印をグリフで描かない**（#1579 / #1536 / #217）
//!
//! ## なぜ止めるのか
//!
//! ユーザーの確定方針は「UI に絵文字・記号アイコンを使わない。アイコンは GPUI の
//! 描画プリミティブ（パス・図形）で描く」。#1536 の番犬
//! （`issue1536_no_emoji_ui_watchdog`）は **Unicode の Emoji プロパティ**で止めるので、
//! そこに当たらないグリフを印代わりに使う形は素通りしていた。実際 #1579 の時点で
//! `×`（U+00D7）が閉じる / kill ボタンに **8 件**（`drawer.rs` 2・`right_panel.rs` 4・
//! `preview_render.rs` 1・`main.rs` 1）、`⎇`（U+2387）が tmux バッジに 1 件、
//! `●`（U+25CF）がライブ印に 2 件残っていた。
//!
//! 字形へ意味を預けると環境で揺れる。`⎇` は多くのフォントに無く豆腐（□）になり、
//! `×` / `●` は太さ・大きさ・ベースライン位置がフォントごとに違うので、
//! 隣に並べた `svg()` のアイコンと揃わない。
//!
//! ## #1536 と何が違うか（**見る場所**が違う）
//!
//! `×` は**文中の記号としては正しい文字**（「20 × 300ms」「612×792 のページ」）。
//! #1536 のように「本番コードの文字列リテラルならどこでも落とす」形にすると、
//! セルフテストの check ラベル（`"ペインの × ボタンで kill（dispatch 経由）"`）まで
//! 巻き込んで**許可リストが肥大する**。そこでこの番犬は
//! **画面へ出る 2 経路だけ**を見る:
//!
//! 1. GPUI の**子要素シンク**（[`CHILD_SINKS`] = `.child(..)` / `.children(..)`）に
//!    渡る文字列リテラル（render コードが描く文字そのもの）
//! 2. `crates/tako-app/src/ui_text/` 配下の本番リテラル（カタログは全文が画面へ出る）
//!
//! この絞り込みのおかげで**許可リストが 1 件も要らない**（例外ゼロで緑）。
//! 例外が要る形を作りたくなったら、それは UI へ印を出そうとしている合図。
//!
//! ## 判定の正本
//!
//! 「印代わりのグリフか」は [`tako_core::emoji::is_icon_glyph`] の **1 実装**。
//! `is_emoji` と同じファイルに置いてあるので、片方だけ範囲が動くことがない。
//!
//! **ファイルの走査**（どの `.rs` を読むか）は `common/emoji_scan.rs` の
//! [`emoji_scan::sources_under`] を借りる（#1578 が #1536 / CLI 番犬のために置いた 1 実装）。
//! 走査対象が増減したとき 3 本の番犬がばらばらに動かないようにするため。
//! **リテラルの見方だけ**がこの番犬固有（`is_emoji` 側は全リテラル / こちらは
//! 子要素シンクとカタログだけ）なので、[`hits_in`] はここに持つ。

#[path = "common/emoji_scan.rs"]
mod emoji_scan;
use emoji_scan::{production_range, repo_root};
use production_range::code_view;

/// 走査対象（GUI が描く文字列があるのはここだけ）。#1536 の番犬と同じ根
const ROOTS: &[&str] = &["crates/tako-app/src"];

/// 走査対象の `.rs` を `(リポジトリ相対パス, 本文)` で返す（走査は共有実装）
fn app_sources() -> Vec<(String, String)> {
    emoji_scan::sources_under(ROOTS)
}

/// 見つけた 1 件（`file:line` で名指しするための最小の情報）
#[derive(Debug, Clone, PartialEq, Eq)]
struct Hit {
    rel: String,
    line: usize,
    ch: char,
    /// どちらの経路で見つけたか（直し方が変わる）
    via: Via,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
    /// 子要素シンク（[`CHILD_SINKS`]）に渡る文字列リテラル
    Child,
    /// `ui_text` のカタログ
    Catalog,
}

impl Hit {
    fn describe(&self) -> String {
        let how = match self.via {
            Via::Child => "子要素シンクに渡る文字列",
            Via::Catalog => "ui_text のカタログ",
        };
        format!(
            "{}:{}: {:?} (U+{:04X}) — {}",
            self.rel, self.line, self.ch, self.ch as u32, how
        )
    }
}

/// GPUI が「この文字を描く」と受け取る呼び出し（**綴り違いの抜け道を残さない**ため、
/// 1 つではなく表で持つ）。今は `.children(` に生のリテラルを渡している箇所は無いが、
/// `.child(` だけを見ていると `.children(["×"])` で素通りしてしまう
const CHILD_SINKS: &[&str] = &[".child(", ".children("];

/// バイト位置から 1 始まりの行番号（`production_range::scan` は行を保つ）
fn line_at(text: &str, byte: usize) -> usize {
    text.as_bytes()[..byte]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

/// 子要素シンクの開き括弧に対応する閉じ括弧のバイト位置（見つからなければ末尾）。
///
/// 数えるのは **`code_view` 側の括弧だけ**。文字列・文字リテラル・コメントの中の
/// 括弧は空白へ潰れているので、`.child(")")` のような書き方に釣られない
fn close_paren(code: &str, open: usize) -> usize {
    let b = code.as_bytes();
    let mut depth = 1usize;
    let mut i = open + 1;
    while i < b.len() {
        match b[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
        i += 1;
    }
    b.len()
}

/// 1 ファイルぶんの違反
fn hits_in(rel: &str, src: &str) -> Vec<Hit> {
    let production = production_range::scan(src).text;
    // 2 つの眺めは同じ走査の出し分けで、**バイト位置が原文と一致する**
    let code = code_view::code_view(&production);
    let lits = code_view::literals_only(&production);
    debug_assert_eq!(code.len(), lits.len());

    // 重複は**バイト位置**で潰す（行では潰さない = 同じ行に 2 個あれば 2 件出す）。
    // 入れ子の `.child(` は範囲が重なるので、これが無いと 1 文字を何度も数える
    let mut marked: std::collections::BTreeMap<usize, (char, Via)> =
        std::collections::BTreeMap::new();

    // 経路 1: 子要素シンクに渡る文字列
    for sink in CHILD_SINKS {
        let mut from = 0usize;
        while let Some(rel_at) = code[from..].find(sink) {
            // `(` の位置（ASCII なので `lits` 側も必ず文字境界）
            let open = from + rel_at + sink.len() - 1;
            let end = close_paren(&code, open);
            for (off, ch) in lits[open..end].char_indices() {
                if tako_core::emoji::is_icon_glyph(ch) {
                    marked.insert(open + off, (ch, Via::Child));
                }
            }
            from = open + 1;
        }
    }

    // 経路 2: `ui_text` のカタログは `.child()` を通らずに画面へ出るので、
    // リテラル全体を見る。同じ位置を経路 1 が既に押さえていれば上書きしない
    if rel.contains("/ui_text/") || rel.ends_with("/ui_text.rs") {
        for (byte, ch) in lits.char_indices() {
            if tako_core::emoji::is_icon_glyph(ch) {
                marked.entry(byte).or_insert((ch, Via::Catalog));
            }
        }
    }

    marked
        .into_iter()
        .map(|(byte, (ch, via))| Hit {
            rel: rel.to_string(),
            line: line_at(&production, byte),
            ch,
            via,
        })
        .collect()
}

fn all_hits() -> Vec<Hit> {
    app_sources()
        .iter()
        .flat_map(|(rel, src)| hits_in(rel, src))
        .collect()
}

#[test]
fn uiの印をグリフで描いていない() {
    let hits = all_hits();
    assert!(
        hits.is_empty(),
        "UI の印をグリフで描いている箇所が {} 件（#1579）。\
         `×` は `svg().path(ui_icon::CLOSE)`、`●` は `div().rounded_full()` の\
         図形、意味が語で足りるものは短い文字ラベルへ:\n{}",
        hits.len(),
        hits.iter()
            .map(|h| h.describe())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn 番犬は修正前の形を名指しで落とす() {
    // #1579 で実際に消した 3 つの形を合成ソースへ戻す（多行の `.child(` も含む）
    let injected = "\
fn render() {
    div().child(\"×\")                                  // drawer.rs の kill ボタン
        .child(\"⎇ tmux\")                              // right_panel.rs の tmux バッジ
        .child(\"● LIVE\")                              // preview_render.rs のライブ印
        .child(
            SharedString::from(\"×\"),
        )
        .children([\"●\"]);                              // 綴り違いの抜け道
}
";
    let hits = hits_in("crates/tako-app/src/injected.rs", injected);
    let found: Vec<(usize, char)> = hits.iter().map(|h| (h.line, h.ch)).collect();
    assert_eq!(
        found,
        vec![(2, '×'), (3, '⎇'), (4, '●'), (6, '×'), (8, '●')],
        "名指しがずれている: {found:?}"
    );
    assert!(
        hits[0]
            .describe()
            .starts_with("crates/tako-app/src/injected.rs:2: '×' (U+00D7)"),
        "file:line で名指ししていない: {}",
        hits[0].describe()
    );
    // 経路の表示が「直し方」を指していること
    assert!(hits[0].describe().contains("子要素シンク"));
}

#[test]
fn 画面へ出ない文字列は落とさない() {
    // セルフテストの check ラベル・診断メッセージ・コメントは UI ではない。
    // ここを落とすと許可リストが肥大して、番犬が「無視するもの」になる
    let not_ui = "\
fn selftest() {
    check(close_button_ok, \"ペインの × ボタンで kill（dispatch 経由）\");
    eprintln!(\"旧実装は 20 × 300ms の固定窓\");
    // タブの × はタブごと kill（コメントなので対象外）
    let label = format!(\"612×792 のページ\");
}
";
    let hits = hits_in("crates/tako-app/src/s.rs", not_ui);
    assert!(hits.is_empty(), "UI でない文字列を拾っている: {hits:?}");
}

#[test]
fn カタログは全文を見る() {
    // `ui_text` は `.child()` を通らずに画面へ出るので、リテラル全体が対象
    let catalog = "\
pub fn live() -> &'static str {
    tr!(\"● 配信中\", \"● LIVE\")
}
";
    let in_catalog = hits_in("crates/tako-app/src/ui_text/preview.rs", catalog);
    assert_eq!(in_catalog.len(), 2, "カタログの 2 件を拾えていない");
    assert!(in_catalog.iter().all(|h| h.via == Via::Catalog));
    // 同じ中身でもカタログ外なら `.child()` を通らない限り落とさない
    assert!(
        hits_in("crates/tako-app/src/other.rs", catalog).is_empty(),
        "カタログ外の非 UI 文字列まで拾っている"
    );
}

#[test]
fn 文字列の中の括弧に釣られない() {
    // `.child(")")` の `)` は `code_view` 側で潰れているので、範囲取りが早じまいしない
    let tricky = "\
fn render() {
    div().child(format!(\"{}(\", name)).child(\"×\");
}
";
    let hits = hits_in("crates/tako-app/src/t.rs", tricky);
    assert_eq!(
        hits.len(),
        1,
        "括弧入りリテラルで範囲取りが壊れている: {hits:?}"
    );
    assert_eq!(hits[0].ch, '×');
}

#[test]
fn 走査範囲が黙って消えていない() {
    let sources = app_sources();
    assert!(
        sources.len() > 30,
        "走査できたファイルが {} 件しかない（置き場が変わった?）",
        sources.len()
    );
    // シンクを 1 つも見つけられなくなったら、検出力を失ったまま緑になる。
    // **表の 1 本ずつ**を数えるので、綴りを消した変更もここで落ちる
    for sink in CHILD_SINKS {
        let sites: usize = sources
            .iter()
            .map(|(_, src)| {
                code_view::code_view(&production_range::scan(src).text)
                    .matches(sink)
                    .count()
            })
            .sum();
        assert!(
            sites > 20,
            "`{sink}` が {sites} 箇所しか見えない（code_view が壊れている? 綴りが変わった?）"
        );
    }
    // カタログ側の眺めも空でないこと
    let catalog_bytes: usize = sources
        .iter()
        .filter(|(rel, _)| rel.contains("/ui_text/"))
        .map(|(_, src)| {
            code_view::literals_only(&production_range::scan(src).text)
                .bytes()
                .filter(|b| !b.is_ascii_whitespace())
                .count()
        })
        .sum();
    assert!(
        catalog_bytes > 10_000,
        "ui_text のリテラルが {catalog_bytes} バイトしかない"
    );
}

#[test]
fn 判定の写しを持たない() {
    let me = std::fs::read_to_string(
        repo_root().join("crates/tako-control/tests/issue1579_ui_glyph_icon_watchdog.rs"),
    )
    .expect("この番犬自身");
    assert!(
        me.contains("tako_core::emoji::is_icon_glyph"),
        "番犬が共有実装を呼んでいない（判定が 2 つになると片方だけ動く）"
    );
    // #1536 側と同じファイルに同居していること（範囲の議論を 1 箇所へ集める）
    let core = std::fs::read_to_string(repo_root().join("crates/tako-core/src/emoji.rs"))
        .expect("tako-core の emoji.rs");
    assert!(
        core.contains("pub fn is_icon_glyph") && core.contains("pub fn is_emoji"),
        "2 つの判定が同じ 1 ファイルに無い"
    );
    // 走査（どの `.rs` を読むか）も #1578 の共有実装へ**丸ごと**委譲していること。
    // 自前のディレクトリ走査へ戻すと、対象が増えたとき 3 本の番犬で視界がずれる。
    // 「呼んでいる」ではなく「`app_sources` の本体がこの 1 行」を見る
    assert!(
        me.contains(
            "fn app_sources() -> Vec<(String, String)> {\n    emoji_scan::sources_under(ROOTS)\n}"
        ),
        "ファイル走査が共有実装（common/emoji_scan.rs）への 1 行委譲になっていない"
    );
}
