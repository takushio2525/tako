//! preview — プレビューペイン（FR-3.2 コード / FR-3.3 Markdown）の読み込みと整形
//!
//! GPUI 非依存（描画は main.rs 側）。シンタックスハイライトは syntect だが、
//! 将来 tree-sitter へ差し替えられるよう [`Highlighter`] trait で抽象化する
//! （`architecture.md`「コンセプト②の実現」。ユーザー指示）。
//! Markdown は pulldown-cmark でイベントストリームをブロック列へ写す。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tako_control::protocol::PreviewModeWire;

/// 読み込みの上限（巨大ファイルで UI を固めない。超過分は切り詰めて明示する）
const MAX_BYTES: usize = 1_000_000;
const MAX_LINES: usize = 5_000;

/// プレビューの表示モード（ワイヤ表現 `PreviewModeWire` と 1:1）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewMode {
    Code,
    Markdown,
}

impl PreviewMode {
    pub fn to_wire(self) -> PreviewModeWire {
        match self {
            PreviewMode::Code => PreviewModeWire::Code,
            PreviewMode::Markdown => PreviewModeWire::Markdown,
        }
    }

    pub fn from_wire(wire: PreviewModeWire) -> Self {
        match wire {
            PreviewModeWire::Code => PreviewMode::Code,
            PreviewModeWire::Markdown => PreviewMode::Markdown,
        }
    }
}

/// ハイライト済みテキストの 1 区間。色はハイライタのテーマ由来（theme 非依存の生 RGB）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub color: Option<tako_core::Rgb>,
    pub bold: bool,
    pub italic: bool,
}

/// ハイライト済みの 1 行
pub type Line = Vec<Span>;

/// Markdown のインライン 1 区間（強調・インラインコード等のスタイルフラグ付き）
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MdSpan {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    /// リンクテキスト（アクセント色で描く。URL 自体は開かない = Web ペインは FR-3.8）
    pub link: bool,
}

/// Markdown のブロック（描画単位。FR-3.3）
#[derive(Debug, Clone, PartialEq)]
pub enum MdBlock {
    Heading {
        level: u8,
        spans: Vec<MdSpan>,
    },
    Paragraph {
        spans: Vec<MdSpan>,
    },
    /// リスト項目。`marker` は "•" / "1." 等、`depth` はネスト段
    ListItem {
        depth: usize,
        marker: String,
        spans: Vec<MdSpan>,
    },
    /// コードブロック（```lang はハイライトして保持する）
    CodeBlock {
        lines: Vec<Line>,
    },
    Quote {
        spans: Vec<MdSpan>,
    },
    Rule,
}

/// 読み込み済みのプレビュー内容
#[derive(Debug, Clone, PartialEq)]
pub enum PreviewContent {
    Code(Vec<Line>),
    Markdown(Vec<MdBlock>),
    /// 読めない・バイナリ等（正常系の劣化。ペインは開いたまま理由を表示する）
    Error(String),
}

/// プレビューペイン 1 枚分の状態（`TakoApp::previews` の値）
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewState {
    pub path: PathBuf,
    pub mode: PreviewMode,
    pub content: PreviewContent,
    /// 上限超過で切り詰めたか（フッタで明示する）
    pub truncated: bool,
}

impl PreviewState {
    /// Markdown レンダリングへ切り替え可能なファイルか（目アイコントグルの表示判定）
    pub fn markdown_capable(&self) -> bool {
        is_markdown_path(&self.path)
    }

    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

pub fn is_markdown_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some(ext) if ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown")
    )
}

/// ファイルを読み込んでプレビュー状態を作る（テスト用。本番は load_fast + background highlight）
#[cfg(test)]
pub fn load(path: &Path, mode: PreviewMode) -> PreviewState {
    let (text, truncated) = match read_text(path) {
        Ok(pair) => pair,
        Err(message) => {
            return PreviewState {
                path: path.to_path_buf(),
                mode,
                content: PreviewContent::Error(message),
                truncated: false,
            }
        }
    };
    let content = match mode {
        PreviewMode::Markdown => PreviewContent::Markdown(markdown_blocks(&text)),
        PreviewMode::Code => PreviewContent::Code(highlighter().highlight(path, &text)),
    };
    PreviewState {
        path: path.to_path_buf(),
        mode,
        content,
        truncated,
    }
}

/// 高速ロード（UI スレッド用）: ファイルを読むが syntect ハイライトはスキップする。
/// Code モードは平文（色なし）を返し、呼び出し側が background で [`highlight_text`] を
/// 走らせて差し替える。Markdown は pulldown-cmark が十分速いのでそのまま完成版を返す。
/// 戻り値の `Option<String>` は Code モードの生テキスト（background ハイライト用）
pub fn load_fast(path: &Path, mode: PreviewMode) -> (PreviewState, Option<String>) {
    let (text, truncated) = match read_text(path) {
        Ok(pair) => pair,
        Err(message) => {
            return (
                PreviewState {
                    path: path.to_path_buf(),
                    mode,
                    content: PreviewContent::Error(message),
                    truncated: false,
                },
                None,
            );
        }
    };
    let (content, raw) = match mode {
        PreviewMode::Markdown => (PreviewContent::Markdown(markdown_blocks(&text)), None),
        PreviewMode::Code => {
            let lines = text.lines().map(|l| vec![plain_span(l)]).collect();
            (PreviewContent::Code(lines), Some(text))
        }
    };
    (
        PreviewState {
            path: path.to_path_buf(),
            mode,
            content,
            truncated,
        },
        raw,
    )
}

/// background executor 上で呼ぶ: syntect ハイライトだけを実行して行列を返す
pub fn highlight_text(path: &Path, text: &str) -> Vec<Line> {
    highlighter().highlight(path, text)
}

/// テキストとして読む。バイナリ（NUL 含有）は明示エラー、上限超過は切り詰める
fn read_text(path: &Path) -> Result<(String, bool), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("読み込めない: {e}"))?;
    let truncated_bytes = bytes.len() > MAX_BYTES;
    let bytes = &bytes[..bytes.len().min(MAX_BYTES)];
    if bytes.contains(&0) {
        return Err("バイナリファイル（テキストとして表示できない）".into());
    }
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    let mut truncated = truncated_bytes;
    if text.lines().count() > MAX_LINES {
        text = text.lines().take(MAX_LINES).collect::<Vec<_>>().join("\n");
        truncated = true;
    }
    Ok((text, truncated))
}

/// シンタックスハイライタの抽象（差し替え点。現実装は syntect、将来 tree-sitter）
pub trait Highlighter: Send + Sync {
    /// パス（拡張子・1 行目）から構文を推定して全行をハイライトする
    fn highlight(&self, path: &Path, text: &str) -> Vec<Line>;
    /// 言語トークン（``` の info 文字列）からのハイライト（Markdown のコードブロック用）
    fn highlight_lang(&self, lang: &str, text: &str) -> Vec<Line>;
}

/// 既定ハイライタ（プロセス内で 1 度だけ構文セットを読む）
pub fn highlighter() -> &'static dyn Highlighter {
    static INSTANCE: OnceLock<SyntectHighlighter> = OnceLock::new();
    INSTANCE.get_or_init(SyntectHighlighter::new)
}

/// syntect 実装（bat / delta と同系の定番。純 Rust 構成 = regex-fancy）
pub struct SyntectHighlighter {
    syntaxes: syntect::parsing::SyntaxSet,
    theme: syntect::highlighting::Theme,
}

impl SyntectHighlighter {
    fn new() -> Self {
        let syntaxes = syntect::parsing::SyntaxSet::load_defaults_newlines();
        // ダーク背景（tako 既定テーマ）に合う同梱テーマ。見つからなければ任意の 1 つ
        let mut themes = syntect::highlighting::ThemeSet::load_defaults().themes;
        let theme = themes
            .remove("base16-eighties.dark")
            .or_else(|| themes.into_values().next())
            .unwrap_or_default();
        Self { syntaxes, theme }
    }

    fn run(&self, syntax: &syntect::parsing::SyntaxReference, text: &str) -> Vec<Line> {
        use syntect::easy::HighlightLines;
        let mut hl = HighlightLines::new(syntax, &self.theme);
        text.lines()
            .map(|line| {
                match hl.highlight_line(line, &self.syntaxes) {
                    Ok(regions) => regions
                        .into_iter()
                        .map(|(style, fragment)| Span {
                            text: fragment.to_string(),
                            color: Some(tako_core::Rgb {
                                r: style.foreground.r,
                                g: style.foreground.g,
                                b: style.foreground.b,
                            }),
                            bold: style
                                .font_style
                                .contains(syntect::highlighting::FontStyle::BOLD),
                            italic: style
                                .font_style
                                .contains(syntect::highlighting::FontStyle::ITALIC),
                        })
                        .collect(),
                    // ハイライト失敗行は素のテキストへ劣化（表示を欠けさせない）
                    Err(_) => vec![plain_span(line)],
                }
            })
            .collect()
    }
}

fn plain_span(text: &str) -> Span {
    Span {
        text: text.to_string(),
        color: None,
        bold: false,
        italic: false,
    }
}

impl Highlighter for SyntectHighlighter {
    fn highlight(&self, path: &Path, text: &str) -> Vec<Line> {
        let syntax = path
            .extension()
            .and_then(|e| e.to_str())
            .and_then(|ext| self.syntaxes.find_syntax_by_extension(ext))
            .or_else(|| {
                text.lines()
                    .next()
                    .and_then(|line| self.syntaxes.find_syntax_by_first_line(line))
            })
            .unwrap_or_else(|| self.syntaxes.find_syntax_plain_text());
        self.run(syntax, text)
    }

    fn highlight_lang(&self, lang: &str, text: &str) -> Vec<Line> {
        let syntax = self
            .syntaxes
            .find_syntax_by_token(lang)
            .unwrap_or_else(|| self.syntaxes.find_syntax_plain_text());
        self.run(syntax, text)
    }
}

/// Markdown をブロック列へパースする（FR-3.3）。表など未対応の構造は
/// テキストとして段落へ劣化させ、内容を落とさない
pub fn markdown_blocks(text: &str) -> Vec<MdBlock> {
    use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(text, options);

    let mut blocks = Vec::new();
    let mut spans: Vec<MdSpan> = Vec::new();
    let (mut bold, mut italic, mut strike, mut link) = (0u32, 0u32, 0u32, 0u32);
    // リストのネスト（None = 箇条書き、Some(n) = 番号付きの次番号）
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut quote_depth = 0u32;
    let mut heading: Option<u8> = None;
    // コードブロック蓄積（lang, 本文）
    let mut code: Option<(String, String)> = None;

    let push_span = |spans: &mut Vec<MdSpan>,
                     text: &str,
                     code_span: bool,
                     bold: u32,
                     italic: u32,
                     strike: u32,
                     link: u32| {
        if text.is_empty() {
            return;
        }
        spans.push(MdSpan {
            text: text.to_string(),
            bold: bold > 0,
            italic: italic > 0,
            code: code_span,
            strike: strike > 0,
            link: link > 0,
        });
    };
    // 段落・見出し等の区切りで溜まったスパンをブロック化する
    fn flush(
        blocks: &mut Vec<MdBlock>,
        spans: &mut Vec<MdSpan>,
        heading: Option<u8>,
        lists: &[Option<u64>],
        quote_depth: u32,
    ) {
        if spans.is_empty() {
            return;
        }
        let spans = std::mem::take(spans);
        if let Some(level) = heading {
            blocks.push(MdBlock::Heading { level, spans });
        } else if let Some(counter) = lists.last() {
            blocks.push(MdBlock::ListItem {
                depth: lists.len().saturating_sub(1),
                marker: match counter {
                    Some(n) => format!("{}.", n.saturating_sub(1)),
                    None => "•".to_string(),
                },
                spans,
            });
        } else if quote_depth > 0 {
            blocks.push(MdBlock::Quote { spans });
        } else {
            blocks.push(MdBlock::Paragraph { spans });
        }
    }

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
                heading = Some(level as u8);
            }
            Event::End(TagEnd::Heading(_)) => {
                flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
                heading = None;
            }
            Event::Start(Tag::List(start)) => {
                flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
                lists.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
                if let Some(Some(counter)) = lists.last_mut() {
                    *counter += 1;
                }
            }
            Event::End(TagEnd::Item) => {
                flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
            }
            Event::Start(Tag::BlockQuote(_)) => {
                flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
                quote_depth += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
                quote_depth = quote_depth.saturating_sub(1);
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((lang, String::new()));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((lang, body)) = code.take() {
                    let body = body.strip_suffix('\n').unwrap_or(&body);
                    blocks.push(MdBlock::CodeBlock {
                        lines: highlighter().highlight_lang(&lang, body),
                    });
                }
            }
            Event::Start(Tag::Paragraph) => {
                flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
            }
            Event::End(TagEnd::Paragraph) => {
                flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
            }
            Event::Start(Tag::Strong) => bold += 1,
            Event::End(TagEnd::Strong) => bold = bold.saturating_sub(1),
            Event::Start(Tag::Emphasis) => italic += 1,
            Event::End(TagEnd::Emphasis) => italic = italic.saturating_sub(1),
            Event::Start(Tag::Strikethrough) => strike += 1,
            Event::End(TagEnd::Strikethrough) => strike = strike.saturating_sub(1),
            Event::Start(Tag::Link { .. }) => link += 1,
            Event::End(TagEnd::Link) => link = link.saturating_sub(1),
            Event::Rule => {
                flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
                blocks.push(MdBlock::Rule);
            }
            Event::Text(t) => {
                if let Some((_, body)) = code.as_mut() {
                    body.push_str(&t);
                } else {
                    push_span(&mut spans, &t, false, bold, italic, strike, link);
                }
            }
            Event::Code(t) => push_span(&mut spans, &t, true, bold, italic, strike, link),
            Event::SoftBreak | Event::HardBreak => {
                push_span(&mut spans, " ", false, bold, italic, strike, link)
            }
            Event::TaskListMarker(done) => push_span(
                &mut spans,
                if done { "☑ " } else { "☐ " },
                false,
                bold,
                italic,
                strike,
                link,
            ),
            // 表・HTML 等はインラインテキストとして劣化（内容を落とさない）
            Event::Html(t) | Event::InlineHtml(t) => {
                push_span(&mut spans, &t, false, bold, italic, strike, link)
            }
            _ => {}
        }
    }
    flush(&mut blocks, &mut spans, heading, &lists, quote_depth);
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rustコードがハイライトされる() {
        let dir = std::env::temp_dir().join(format!("tako-preview-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("main.rs");
        std::fs::write(&path, "fn main() {\n    let x = 1;\n}\n").unwrap();
        let state = load(&path, PreviewMode::Code);
        let PreviewContent::Code(lines) = &state.content else {
            panic!("Code になる: {:?}", state.content);
        };
        assert_eq!(lines.len(), 3);
        // キーワード `fn` が複数スパンに分かれ、色が付く
        assert!(lines[0].len() > 1, "1 行目が複数スパンに分かれる");
        assert!(lines[0].iter().any(|s| s.color.is_some()));
        assert_eq!(
            lines[0].iter().map(|s| s.text.as_str()).collect::<String>(),
            "fn main() {"
        );
        assert!(!state.truncated);
        assert!(!state.markdown_capable());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn markdownがブロックへパースされる() {
        let text = "# 見出し\n\n本文 **強調** と `code`。\n\n- 項目1\n- 項目2\n\n```rust\nfn f() {}\n```\n\n---\n";
        let blocks = markdown_blocks(text);
        assert!(matches!(
            &blocks[0],
            MdBlock::Heading { level: 1, spans } if spans[0].text == "見出し"
        ));
        let MdBlock::Paragraph { spans } = &blocks[1] else {
            panic!("段落になる: {:?}", blocks[1]);
        };
        assert!(spans.iter().any(|s| s.bold && s.text == "強調"));
        assert!(spans.iter().any(|s| s.code && s.text == "code"));
        let items: Vec<_> = blocks
            .iter()
            .filter_map(|b| match b {
                MdBlock::ListItem { marker, spans, .. } => {
                    Some((marker.clone(), spans[0].text.clone()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            items,
            vec![
                ("•".to_string(), "項目1".to_string()),
                ("•".to_string(), "項目2".to_string())
            ]
        );
        assert!(blocks
            .iter()
            .any(|b| matches!(b, MdBlock::CodeBlock { lines } if !lines.is_empty())));
        assert!(blocks.iter().any(|b| matches!(b, MdBlock::Rule)));
    }

    #[test]
    fn 番号付きリストとネスト() {
        let blocks = markdown_blocks("1. one\n2. two\n   - sub\n");
        let items: Vec<_> = blocks
            .iter()
            .filter_map(|b| match b {
                MdBlock::ListItem {
                    depth,
                    marker,
                    spans,
                } => Some((*depth, marker.clone(), spans[0].text.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            items,
            vec![
                (0, "1.".to_string(), "one".to_string()),
                (0, "2.".to_string(), "two".to_string()),
                (1, "•".to_string(), "sub".to_string()),
            ]
        );
    }

    #[test]
    fn バイナリと不在は明示エラーになる() {
        let dir = std::env::temp_dir().join(format!("tako-preview-bin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bin.dat");
        std::fs::write(&path, [0u8, 159, 146, 150]).unwrap();
        let state = load(&path, PreviewMode::Code);
        assert!(matches!(&state.content, PreviewContent::Error(m) if m.contains("バイナリ")));
        let state = load(&dir.join("no-such.txt"), PreviewMode::Code);
        assert!(matches!(&state.content, PreviewContent::Error(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 性能計測（通常テストでは走らせない）: `cargo test -p tako-app --release -- --ignored --nocapture perf_`
    #[test]
    #[ignore]
    fn perf_ハイライト計測() {
        use std::time::Instant;
        let src_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs");

        let t0 = Instant::now();
        let hl = highlighter();
        let init = t0.elapsed();
        eprintln!("[perf] SyntaxSet+Theme ロード: {:?}", init);

        let text = std::fs::read_to_string(&src_path).unwrap();
        let lines = text.lines().count().min(MAX_LINES);
        let capped: String = text.lines().take(MAX_LINES).collect::<Vec<_>>().join("\n");

        let t1 = Instant::now();
        let out = hl.highlight(&src_path, &capped);
        eprintln!(
            "[perf] highlight main.rs（{} 行）: {:?}（{} 行出力）",
            lines,
            t1.elapsed(),
            out.len()
        );

        // 2 回目（SyntaxSet ロード済み）の load() 全体 = 旧同期経路
        let t2 = Instant::now();
        let state = load(&src_path, PreviewMode::Code);
        eprintln!(
            "[perf] load() 同期全体: {:?} truncated={}",
            t2.elapsed(),
            state.truncated
        );

        // load_fast = UI スレッドが払うコスト（ファイル読み + 平文化のみ）
        let t2b = Instant::now();
        let (fast_state, raw) = load_fast(&src_path, PreviewMode::Code);
        eprintln!(
            "[perf] load_fast() UI コスト: {:?} truncated={} raw={}bytes",
            t2b.elapsed(),
            fast_state.truncated,
            raw.as_ref().map(|s| s.len()).unwrap_or(0)
        );

        // Markdown: このリポジトリの requirements.md（大きめの実物）
        let md_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.agent/requirements.md");
        if md_path.is_file() {
            let md = std::fs::read_to_string(&md_path).unwrap();
            let t3 = Instant::now();
            let blocks = markdown_blocks(&md);
            eprintln!(
                "[perf] markdown_blocks requirements.md（{} bytes）: {:?}（{} ブロック）",
                md.len(),
                t3.elapsed(),
                blocks.len()
            );
        }
    }

    #[test]
    fn markdown判定はパス拡張子から() {
        assert!(is_markdown_path(Path::new("/a/README.md")));
        assert!(is_markdown_path(Path::new("/a/B.Markdown")));
        assert!(!is_markdown_path(Path::new("/a/main.rs")));
    }
}
