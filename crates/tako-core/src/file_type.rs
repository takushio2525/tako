//! 言語の同定の正本（Issue #1948 / エピック #1943）。純粋関数だけで I/O をしない
//!
//! 「このファイルは何の形式か」を**ファイル名 → 拡張子**の順で決める表をここ 1 か所に置く。
//! 次がすべてこの表を引く:
//!
//! - 色分け（tako-app の構文の選び方。`syntax` → two-face の順）と Markdown のコードブロックの別名
//! - プレビューの種別・HTTP の型（[`crate::open_plan`]。tako-app の画像の形式・Markdown 判定も同じ）
//! - インデントの既定と Enter の規則（[`crate::text_edit`]。字下げを戻すキーワード・リストの印の
//!   引き継ぎ = #1759 の `dedent_after` 列と `list_continuation` 列も）
//! - 括弧の自動閉じをする形式か（`auto_close` 列。#1759）
//! - 行コメントの記号（⌘/ と `tako edit line toggle-comment`。#1692）
//! - 対応括弧の探索が中を飛ばす文字列とコメント（`lexical` 列 + `comment` 列。#1691）
//! - CLI / MCP の応答の `file_type`（`tako open` の結果・`tako edit` の応答）
//!
//! **全 214 構文（two-face の 213 + two-face に無い PowerShell = #1954）を並べない**。
//! 行を持つのは「既定（[`CODE`] = code・two-face 任せ・4 スペース・括弧で深く）から外れる形式」と
//! 「挙動の表が名指す形式」だけで、表に無い形式は既定のまま。
//! 拡張子だけでは塗る構文が決まらない形式（`.m` = MATLAB / Objective-C）は、本文の手掛かりで
//! 構文を決める純粋関数 [`syntax_by_content`] を色分けが行の `syntax` より先に引く（#1953）。
//!
//! 挙動の表（`lsp::servers` / `platform::runner_defaults` / `runner_project`）は**分けたまま**
//! （#1726 設計 §9.2。1 枚にすると疎な表になる）。代わりに番犬
//! `crates/tako-control/tests/issue1948_file_type_watchdog.rs` が
//! 「挙動の表が名指す拡張子・ファイル名は必ずここに行がある」と
//! 「`preview.rs` / `text_edit.rs` / `open_plan.rs` に拡張子の文字列リテラルの `match` を書かない」
//! を強制する。設計は `.agent/plans/2026-10-language-coverage.md` §4

use std::path::Path;

use crate::open_plan::PreviewRoute;
use crate::text_edit::{CommentSyntax, IndentUnit, Lexical, SingleQuote};

/// 表の 1 行 = 1 つの形式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileType {
    /// 安定 ID（CLI / MCP の応答の `file_type`）。言語は VS Code の言語 ID に揃え、
    /// 言語でない形式（画像・PDF・動画）は形式の名前
    pub id: &'static str,
    /// 拡張子（先頭の `.` を含まない小文字。照合は大文字小文字を問わない）
    pub extensions: &'static [&'static str],
    /// ファイル名で決まる形式（`Makefile` / `CMakeLists.txt` 等）。**拡張子より先に引く**。
    /// 照合は大文字小文字を問わない（two-face のファイル名の照合と同じ）
    pub file_names: &'static [&'static str],
    /// 塗る構文（two-face の構文名）。`None` = two-face の表（ファイル名 → 拡張子 → 1 行目）に任せる
    pub syntax: Option<&'static str>,
    /// Markdown のコードブロック（```` ```ino ````）の別名。two-face が知らない名前だけを置く
    /// （知っている名前は two-face が先に当たる。照合は大文字小文字を問わない）
    pub fence_tokens: &'static [&'static str],
    /// プレビューの種別
    pub preview: PreviewRoute,
    /// 画像の形式（`preview` が [`PreviewRoute::Image`] の行だけ `Some`）
    pub image: Option<ImageFormat>,
    /// HTTP の `Content-Type`（daemon が添付をその場で見せるとき。#1472）。
    /// 分からない形式は `None` = 呼び出し側が `application/octet-stream` へ倒す
    pub media_type: Option<&'static str>,
    /// 本文に手掛かりが無いときのインデント 1 段（#1654）
    pub indent: IndentUnit,
    /// Enter で 1 段深くする規則（#1654）
    pub enter: EnterRule,
    /// この語で始まる文の行で Enter を押すと 1 段浅くする（#1759。Python の `return` / `pass` …）。
    /// ブロックを抜ける文の後ろは同じ深さに続きを書かない言語だけが持つ。照合は語の単位で、
    /// ロジックは語の文字列を持たない（行を足すだけで言語が増える）
    pub dedent_after: &'static [&'static str],
    /// リストの行（`- ` / `* ` / `1. ` / `- [ ] `）で Enter を押すと次の行へ印を引き継ぐ（#1759。Markdown）
    pub list_continuation: bool,
    /// 開き括弧を打つと閉じ括弧も入れる（#1759。設定 `editor_auto_close` が ON のとき）。
    /// 文書（`[` はリンク・`(` は括弧書き）は入れない
    pub auto_close: bool,
    /// 行コメントの記号（#1692）。`None` = コメントの記号を持たない形式（プレーンテキスト・JSON・
    /// 表に無い形式）で、コメントの付け外しは本文を触らずに理由を返す
    pub comment: Option<CommentSyntax>,
    /// 文字列とブロックコメントの書き方（#1691）。対応括弧の探索が中の括弧を数えない
    pub lexical: Lexical,
}

/// 画像の形式（GPUI の `ImageFormat` と 1:1 だが GPUI 非依存にする）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    WebP,
    Svg,
}

/// Enter で 1 段深くする規則（#1654）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnterRule {
    /// 前の行のインデントを引き継ぐだけ（Markdown・プレーンテキスト。`[` はリンク、
    /// `(` は括弧書きで、入れ子の始まりではない）
    InheritOnly,
    /// 行末が `{` / `[` / `(` なら深くする（コード全般）
    Brackets,
    /// 括弧に加えて行末の `:` でも深くする（Python のブロック・YAML の入れ子）
    BracketsAndColon,
}

impl EnterRule {
    /// 行末の開き括弧で深くするか
    pub fn brackets(self) -> bool {
        !matches!(self, Self::InheritOnly)
    }

    /// 行末の `:` で深くするか
    pub fn colon(self) -> bool {
        matches!(self, Self::BracketsAndColon)
    }
}

/// 既定の形式（表に無い形式の扱い）。表の行は `..CODE` で差分だけを書く
pub const CODE: FileType = FileType {
    id: "",
    extensions: &[],
    file_names: &[],
    syntax: None,
    fence_tokens: &[],
    preview: PreviewRoute::Code,
    image: None,
    media_type: None,
    indent: IndentUnit::Spaces(4),
    enter: EnterRule::Brackets,
    dedent_after: &[],
    list_continuation: false,
    auto_close: true,
    comment: None,
    lexical: LEX_CODE,
};

/// ブロックを抜ける文（#1759）。この後ろへ同じ深さで書いても届かない（Python）
const BLOCK_EXITS: &[&str] = &["return", "pass", "break", "continue", "raise"];

// 文字列とブロックコメントの書き方（#1691）。表の行がこのどれかを `lexical` に持つ
/// 既定: `"…"` と 1 文字のリテラル `'x'`（表に無い形式もこれ）
const LEX_CODE: Lexical = Lexical {
    block_comment: None,
    double_quote: true,
    single_quote: SingleQuote::Char,
    backtick: false,
};
/// C の系譜: 既定 + `/* */`（`'` は 1 文字のリテラルだけ = Rust のライフタイム `'a` を文字列にしない）
const LEX_C: Lexical = Lexical {
    block_comment: Some(("/*", "*/")),
    ..LEX_CODE
};
/// C の系譜 + `` `…` ``（Go の生文字列）
const LEX_GO: Lexical = Lexical {
    backtick: true,
    ..LEX_C
};
/// `'…'` も文字列（Python / Ruby / YAML …。同じ行で閉じなければアポストロフィ）
const LEX_SCRIPT: Lexical = Lexical {
    single_quote: SingleQuote::String,
    ..LEX_CODE
};
/// スクリプト + `` `…` ``（シェルのコマンド置換）
const LEX_SHELL: Lexical = Lexical {
    backtick: true,
    ..LEX_SCRIPT
};
/// `/* */` と `'…'` の文字列（PHP / Dart / CSS / SQL）
const LEX_WEB: Lexical = Lexical {
    block_comment: Some(("/*", "*/")),
    ..LEX_SCRIPT
};
/// `/* */`・`'…'`・`` `…` ``（JS / TS のテンプレート）
const LEX_JS: Lexical = Lexical {
    backtick: true,
    ..LEX_WEB
};

/// Arduino のスケッチ（#1949）。`TAKO_1949_LEGACY=1` の A/B が名指す
pub const ARDUINO: &str = "arduino";

/// 既存の構文への別名で色の穴を埋めた行（#1953）。`TAKO_1953_LEGACY=1` の A/B がこの行の
/// `syntax` と本文の判定（[`syntax_by_content`]）を外し、#1953 以前（`.h` = Objective-C・
/// `.pde` / `.json5` / `.jsonc` = Plain Text）へ戻す
pub const ALIASES_1953: &[&str] = &["cpp", "processing", "jsonc", "json5"];

// 行コメントの記号（#1692）。表の行がこのどれかを `comment` に持つ
const SLASHES: Option<CommentSyntax> = Some(CommentSyntax::Line("//"));
const HASH: Option<CommentSyntax> = Some(CommentSyntax::Line("#"));
const DASHES: Option<CommentSyntax> = Some(CommentSyntax::Line("--"));
const SEMICOLON: Option<CommentSyntax> = Some(CommentSyntax::Line(";"));
/// 行コメントを持たない形式は行ごとに囲む（HTML 系・CSS）
const MARKUP: Option<CommentSyntax> = Some(CommentSyntax::Block {
    open: "<!--",
    close: "-->",
});
const C_BLOCK: Option<CommentSyntax> = Some(CommentSyntax::Block {
    open: "/*",
    close: "*/",
});

/// 色・インデント・Enter は既定のまま、行コメントの記号だけを持つ言語
/// （挙動の表が名指す言語と、コメントの記号を言うために行を持つ言語）
const fn lang(
    id: &'static str,
    extensions: &'static [&'static str],
    comment: Option<CommentSyntax>,
) -> FileType {
    FileType {
        id,
        extensions,
        comment,
        ..CODE
    }
}

/// [`lang`] に文字列とブロックコメントの書き方（#1691）を足した行
const fn lexed(
    id: &'static str,
    extensions: &'static [&'static str],
    comment: Option<CommentSyntax>,
    lexical: Lexical,
) -> FileType {
    FileType {
        lexical,
        ..lang(id, extensions, comment)
    }
}

/// Enter で深くしない文書（括弧も自動で閉じない）
const fn prose(id: &'static str, extensions: &'static [&'static str]) -> FileType {
    FileType {
        id,
        extensions,
        enter: EnterRule::InheritOnly,
        auto_close: false,
        lexical: Lexical::PROSE,
        ..CODE
    }
}

const fn image(
    id: &'static str,
    extensions: &'static [&'static str],
    format: ImageFormat,
    media_type: &'static str,
) -> FileType {
    FileType {
        id,
        extensions,
        preview: PreviewRoute::Image,
        image: Some(format),
        media_type: Some(media_type),
        ..CODE
    }
}

const fn video(
    id: &'static str,
    extensions: &'static [&'static str],
    media_type: &'static str,
) -> FileType {
    FileType {
        id,
        extensions,
        preview: PreviewRoute::Video,
        media_type: Some(media_type),
        ..CODE
    }
}

/// ファイル名だけで決まり、two-face が知らないので構文を名指す設定ファイル
const fn named(
    id: &'static str,
    file_names: &'static [&'static str],
    syntax: &'static str,
    comment: Option<CommentSyntax>,
) -> FileType {
    FileType {
        id,
        file_names,
        syntax: Some(syntax),
        comment,
        ..CODE
    }
}

/// 形式の表（**1 形式 = 1 行**。拡張子・ファイル名・コードブロックの別名は表全体で重複しない）
pub const TABLE: &[FileType] = &[
    // ─── 文書（Enter は前の行を引き継ぐだけ） ─────────────────────────
    FileType {
        id: "markdown",
        extensions: &["md", "markdown"],
        preview: PreviewRoute::Markdown,
        media_type: Some("text/markdown; charset=utf-8"),
        enter: EnterRule::InheritOnly,
        list_continuation: true,
        auto_close: false,
        comment: MARKUP,
        lexical: Lexical::PROSE,
        ..CODE
    },
    FileType {
        list_continuation: true,
        ..prose("mdx", &["mdx"])
    },
    prose("plaintext", &["txt"]),
    prose("restructuredtext", &["rst"]),
    FileType {
        comment: SLASHES,
        ..prose("asciidoc", &["adoc"])
    },
    // ─── 画像・PDF・動画（プレビューの種別と HTTP の型） ────────────────
    image("png", &["png"], ImageFormat::Png, "image/png"),
    image("jpeg", &["jpg", "jpeg"], ImageFormat::Jpeg, "image/jpeg"),
    image("gif", &["gif"], ImageFormat::Gif, "image/gif"),
    image("webp", &["webp"], ImageFormat::WebP, "image/webp"),
    // SVG は XML なので、本文を編集するときのコメントは XML の囲み
    FileType {
        comment: MARKUP,
        ..image("svg", &["svg"], ImageFormat::Svg, "image/svg+xml")
    },
    FileType {
        id: "pdf",
        extensions: &["pdf"],
        preview: PreviewRoute::Pdf,
        media_type: Some("application/pdf"),
        ..CODE
    },
    video("mp4", &["mp4"], "video/mp4"),
    video("webm", &["webm"], "video/webm"),
    // QuickTime は `video/quicktime`。iOS / macOS の Safari はこれで鳴る
    video("quicktime", &["mov"], "video/quicktime"),
    video("avi", &["avi"], "video/x-msvideo"),
    video("matroska", &["mkv"], "video/x-matroska"),
    // ─── インデントがタブで決まっているもの（Makefile はレシピ行がタブ必須・Go は gofmt） ─
    FileType {
        id: "makefile",
        extensions: &["mk"],
        file_names: &["Makefile", "GNUmakefile"],
        indent: IndentUnit::Tab,
        comment: HASH,
        lexical: LEX_SCRIPT,
        ..CODE
    },
    FileType {
        id: "go",
        extensions: &["go"],
        indent: IndentUnit::Tab,
        comment: SLASHES,
        lexical: LEX_GO,
        ..CODE
    },
    // ─── `:` でも深くするもの ───────────────────────────────────────
    FileType {
        id: "python",
        extensions: &["py", "pyi", "pyw"],
        enter: EnterRule::BracketsAndColon,
        dedent_after: BLOCK_EXITS,
        comment: HASH,
        lexical: LEX_SCRIPT,
        ..CODE
    },
    FileType {
        id: "yaml",
        extensions: &["yml", "yaml"],
        enter: EnterRule::BracketsAndColon,
        comment: HASH,
        lexical: LEX_SCRIPT,
        ..CODE
    },
    // `CMakeLists.txt` は拡張子だけで見ると `txt`（継承だけ）になる。ファイル名の行で
    // コードとして扱う（括弧で深く = #1948 の意図した差分）。色は two-face の CMake
    FileType {
        id: "cmake",
        file_names: &["CMakeLists.txt"],
        comment: HASH,
        ..CODE
    },
    // ─── two-face に無い拡張子・ファイル名を塗れる構文へ寄せるもの ────────────
    FileType {
        id: "javascript",
        extensions: &["js", "mjs", "cjs"],
        syntax: Some("JavaScript"),
        comment: SLASHES,
        lexical: LEX_JS,
        ..CODE
    },
    FileType {
        id: "javascriptreact",
        extensions: &["jsx"],
        syntax: Some("JavaScript"),
        comment: SLASHES,
        lexical: LEX_JS,
        ..CODE
    },
    lexed("typescript", &["ts", "mts", "cts"], SLASHES, LEX_JS),
    lexed("typescriptreact", &["tsx"], SLASHES, LEX_JS),
    // `.ino` は C++ として塗る（#1949）。two-face に Arduino の構文は無く、VS Code 同梱の
    // cpp 言語定義も `.ino` を C++ に含める。寄せるのは**色だけ**で、言語サーバは別
    // （clangd へ `.ino` を渡すと正しいスケッチに嘘のエラーが出る = #1951）
    FileType {
        id: ARDUINO,
        extensions: &["ino"],
        syntax: Some("C++"),
        fence_tokens: &["ino", "arduino"],
        comment: SLASHES,
        lexical: LEX_C,
        ..CODE
    },
    // `.pde`（Processing のスケッチ）は Java として塗る（#1953）。two-face に Processing の構文は
    // 無い。`.ino` と同じく寄せるのは**色だけ**で、`java` の行にはしない（クラスで包まないスケッチは
    // Java として正しくないので、Java の言語サーバ（#1956）へ渡すと嘘のエラーになる）
    FileType {
        id: "processing",
        extensions: &["pde"],
        syntax: Some("Java"),
        fence_tokens: &["pde", "processing"],
        comment: SLASHES,
        lexical: LEX_C,
        ..CODE
    },
    // JSON5 は JavaScript として塗る（#1953。GitHub の Linguist も `source.js`）。JSON の構文は
    // コメントと末尾カンマは通すが、JSON5 にしか無い書き方（クォートの無いキー・`'…'`・16 進・
    // `Infinity`・`.5`）を `invalid.illegal` にし、テーマがそれを地の色（#2d2d2d）で描くので
    // ダークでは字が消える（renovate.json5 の形で 144 字句中 113）。JavaScript なら 0
    FileType {
        id: "json5",
        extensions: &["json5"],
        syntax: Some("JavaScript"),
        fence_tokens: &["json5"],
        comment: SLASHES,
        lexical: LEX_WEB,
        ..CODE
    },
    // JSONC（コメントと末尾カンマを許す JSON）は JSON の構文で塗る（#1953）。JSON の構文は
    // `//` / `/* */` をコメントとして塗り、末尾カンマを `invalid` にしない（実測で 0 字句）
    FileType {
        syntax: Some("JSON"),
        fence_tokens: &["jsonc"],
        ..lexed("jsonc", &["jsonc"], SLASHES, LEX_C)
    },
    named("ignore", &[".dockerignore"], "Git Ignore", HASH),
    named("ini", &[".npmrc", ".yarnrc"], "INI", SEMICOLON),
    // JSON はコメントを持たない（表に無い `.json` も同じく記号なし）
    named(
        "json",
        &[".eslintrc", ".prettierrc", ".babelrc"],
        "JSON",
        None,
    ),
    FileType {
        lexical: LEX_SCRIPT,
        ..named("just", &["Justfile"], "Makefile", HASH)
    },
    // ─── 挙動の表（言語サーバ・実行の既定・実行のプロジェクト）が名指す言語 ──────
    lexed("c", &["c"], SLASHES, LEX_C),
    // `.h` は VS Code と同じく C++ のヘッダとして数え、C++ で塗る（#1953）。two-face は `.h` を
    // C / C++ / Objective-C / Objective-C++ が取り合い、最後の Objective-C が勝つ。C のヘッダも
    // C++ の構文で崩れない。Objective-C のヘッダだけは本文の手掛かりで今までどおり Objective-C
    // （[`syntax_by_content`]）。ほかの拡張子は two-face でも C++ なので変わらない
    FileType {
        syntax: Some("C++"),
        ..lexed(
            "cpp",
            &["cpp", "cc", "cxx", "h", "hh", "hpp", "hxx"],
            SLASHES,
            LEX_C,
        )
    },
    lexed("rust", &["rs"], SLASHES, LEX_C),
    lexed(
        "shellscript",
        &["sh", "bash", "zsh", "command"],
        HASH,
        LEX_SHELL,
    ),
    // `.psm1`（モジュール）/ `.psd1`（マニフェスト = ハッシュテーブルの式）も同じ言語
    // （VS Code の powershell 言語と同じ。色は #1954 で足した PowerShell の構文）
    lexed("powershell", &["ps1", "psm1", "psd1"], HASH, LEX_SCRIPT),
    lang("bat", &["bat", "cmd"], Some(CommentSyntax::Line("REM"))),
    lexed("ruby", &["rb"], HASH, LEX_SCRIPT),
    lexed("perl", &["pl"], HASH, LEX_SCRIPT),
    lexed("php", &["php"], SLASHES, LEX_WEB),
    lexed("lua", &["lua"], DASHES, LEX_SCRIPT),
    lexed("java", &["java"], SLASHES, LEX_C),
    lexed("kotlin", &["kt"], SLASHES, LEX_C),
    lexed("scala", &["scala"], SLASHES, LEX_C),
    lexed("swift", &["swift"], SLASHES, LEX_C),
    lexed("csharp", &["cs"], SLASHES, LEX_C),
    lexed("fsharp", &["fs"], SLASHES, LEX_C),
    lang("vb", &["vb"], Some(CommentSyntax::Line("'"))),
    lexed("dart", &["dart"], SLASHES, LEX_WEB),
    lexed("r", &["r"], HASH, LEX_SCRIPT),
    lang("julia", &["jl"], HASH),
    lang("haskell", &["hs"], DASHES),
    lang("clojure", &["clj"], SEMICOLON),
    lexed("elixir", &["ex"], HASH, LEX_SCRIPT),
    lang("zig", &["zig"], SLASHES),
    lang("nim", &["nim"], HASH),
    lexed("sql", &["sql"], DASHES, LEX_WEB),
    // LaTeX の `"` / `'` は引用符の組版で、文字列の区切りではない
    lexed(
        "latex",
        &["tex"],
        Some(CommentSyntax::Line("%")),
        Lexical::PROSE,
    ),
    // ノートブックは JSON（セルの中のコメントはノートブックの編集の外）
    lang("jupyter", &["ipynb"], None),
    // ─── コメントの記号を言うために行を持つ言語（#1692。色・インデント・Enter は既定のまま） ─
    lang("html", &["html", "htm"], MARKUP),
    lang("xml", &["xml"], MARKUP),
    lang("vue", &["vue"], MARKUP),
    lexed("css", &["css"], C_BLOCK, LEX_WEB),
    lexed("scss", &["scss"], SLASHES, LEX_WEB),
    lexed("less", &["less"], SLASHES, LEX_WEB),
    lexed("toml", &["toml"], HASH, LEX_SCRIPT),
    FileType {
        id: "dockerfile",
        file_names: &["Dockerfile"],
        comment: HASH,
        lexical: LEX_SCRIPT,
        ..CODE
    },
];

/// パスの形式（**ファイル名 → 拡張子**の順。どちらも大文字小文字を問わない）。
/// 表に無ければ `None`（既定 = [`CODE`] の扱い）
pub fn identify(path: &Path) -> Option<&'static FileType> {
    identify_in(TABLE, path)
}

/// 渡した表からパスの形式を引く（[`identify`] の中身）。表を引数で受けるのは、単体が
/// 「表に 1 行足すだけで結果が変わる」を表のコピーで確かめるため（#1692）
pub fn identify_in<'a>(table: &'a [FileType], path: &Path) -> Option<&'a FileType> {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| file_name_in(table, name))
        .or_else(|| extension_in(table, path.extension()?.to_str()?))
}

/// ファイル名で決まる形式
pub fn by_file_name(name: &str) -> Option<&'static FileType> {
    file_name_in(TABLE, name)
}

/// 拡張子（先頭の `.` を含まない）の形式
pub fn by_extension(ext: &str) -> Option<&'static FileType> {
    extension_in(TABLE, ext)
}

fn file_name_in<'a>(table: &'a [FileType], name: &str) -> Option<&'a FileType> {
    table.iter().find(|row| {
        row.file_names
            .iter()
            .any(|known| known.eq_ignore_ascii_case(name))
    })
}

fn extension_in<'a>(table: &'a [FileType], ext: &str) -> Option<&'a FileType> {
    table.iter().find(|row| {
        row.extensions
            .iter()
            .any(|known| known.eq_ignore_ascii_case(ext))
    })
}

/// Markdown のコードブロックの言語トークン（```` ```ino ````）の別名を持つ形式
pub fn by_fence_token(token: &str) -> Option<&'static FileType> {
    TABLE.iter().find(|row| {
        row.fence_tokens
            .iter()
            .any(|known| known.eq_ignore_ascii_case(token))
    })
}

/// パスの形式。表に無ければ既定（[`CODE`]）
pub fn identify_or_default(path: &Path) -> &'static FileType {
    identify(path).unwrap_or(&CODE)
}

/// 応答の `file_type`（表に無い形式は `None` = JSON の null）
pub fn id(path: &Path) -> Option<&'static str> {
    identify(path).map(|row| row.id)
}

// ─── 本文の手掛かりで塗る構文（#1953） ──────────────────────────────────────

/// 本文の手掛かりを探す行数（先頭から）。手掛かりは頭に出るので足り、編集中は 1 打鍵ごとの
/// 塗り直しでも走るので全文を舐めない
pub const SNIFF_LINES: usize = 200;

/// 本文の手掛かりが示す言語
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    /// `#import` / `@interface` / `@implementation` / `@protocol` / `@class` / `@property` / `@end`
    ObjectiveC,
    /// `%` のコメント（`%%` のセル・`%{` も）/ `function` / `classdef`
    Matlab,
}

/// 先頭 [`SNIFF_LINES`] 行の手掛かり。Objective-C の印が 1 つでもあれば Objective-C を先に取る
/// （GitHub の Linguist の `.m` の判定と同じ順。MATLAB の印は Objective-C の本文には出ない）
pub fn cue(text: &str) -> Option<Cue> {
    let mut matlab = false;
    for line in text.lines().take(SNIFF_LINES) {
        let line = line.trim_start_matches('\u{feff}').trim_start();
        if OBJECTIVE_C_WORDS
            .iter()
            .any(|word| starts_with_word(line, word))
            || line
                .strip_prefix("#import")
                .is_some_and(|rest| rest.starts_with([' ', '\t', '<', '"']))
        {
            return Some(Cue::ObjectiveC);
        }
        matlab |= line.starts_with('%') || MATLAB_WORDS.iter().any(|w| starts_with_word(line, w));
    }
    matlab.then_some(Cue::Matlab)
}

const OBJECTIVE_C_WORDS: &[&str] = &[
    "@interface",
    "@implementation",
    "@protocol",
    "@class",
    "@property",
    "@end",
];
const MATLAB_WORDS: &[&str] = &["function", "classdef"];

/// `line` が `word` で始まり、その後ろが識別子の続きではない（`functional` を `function` にしない）
fn starts_with_word(line: &str, word: &str) -> bool {
    line.strip_prefix(word).is_some_and(|rest| {
        !rest
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    })
}

/// 拡張子だけでは塗る構文が決まらない形式の、本文の手掛かりによる構文（two-face の構文名）。
/// 表の行の `syntax` より先に引く。手掛かりが無ければ `None` = 行の `syntax` → two-face の表の順のまま
///
/// - `.m`: MATLAB と Objective-C が取り合う（two-face は `.m` を Objective-C に、MATLAB の構文を
///   拡張子 `matlab` だけに当てる）。表の行は持たない（パスだけでは決まらないので `file_type` は null。
///   コメントの記号・括弧の書き方も今のまま）
/// - `.h`: 表では C++ の行。Objective-C のヘッダだけは #1953 以前と同じ Objective-C
pub fn syntax_by_content(path: &Path, text: &str) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?;
    if ext.eq_ignore_ascii_case("m") {
        Some(match cue(text)? {
            Cue::ObjectiveC => OBJECTIVE_C,
            Cue::Matlab => MATLAB,
        })
    } else if ext.eq_ignore_ascii_case("h") {
        (cue(text) == Some(Cue::ObjectiveC)).then_some(OBJECTIVE_C)
    } else {
        None
    }
}

/// [`syntax_by_content`] が返す構文名（tako-app の単体が構文セットに在ることを縛る）
pub const CONTENT_SYNTAXES: &[&str] = &[OBJECTIVE_C, MATLAB];
const OBJECTIVE_C: &str = "Objective-C";
const MATLAB: &str = "MATLAB";

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn 表の値は重複しない() {
        let mut ids = HashSet::new();
        let mut exts = HashSet::new();
        let mut names = HashSet::new();
        let mut fences = HashSet::new();
        for row in TABLE {
            assert!(!row.id.is_empty(), "ID の無い行: {row:?}");
            assert!(ids.insert(row.id), "ID が重複: {}", row.id);
            assert!(
                !row.extensions.is_empty() || !row.file_names.is_empty(),
                "{}: 拡張子もファイル名も無い行は引けない",
                row.id
            );
            for ext in row.extensions {
                assert_eq!(
                    *ext,
                    ext.to_ascii_lowercase(),
                    "{}: 拡張子は小文字で書く",
                    row.id
                );
                assert!(!ext.starts_with('.'), "{}: 拡張子に `.` を付けない", row.id);
                assert!(exts.insert(*ext), "拡張子 {ext} が 2 行にある");
            }
            for name in row.file_names {
                assert!(
                    names.insert(name.to_ascii_lowercase()),
                    "ファイル名 {name} が 2 行にある"
                );
            }
            for token in row.fence_tokens {
                assert!(
                    fences.insert(token.to_ascii_lowercase()),
                    "コードブロックの別名 {token} が 2 行にある"
                );
            }
        }
    }

    #[test]
    fn 画像の形式は画像の行だけが持つ() {
        for row in TABLE {
            assert_eq!(
                row.preview == PreviewRoute::Image,
                row.image.is_some(),
                "{}: 画像の行と画像の形式が対になっていない",
                row.id
            );
        }
    }

    #[test]
    fn ファイル名を拡張子より先に引く() {
        // `CMakeLists.txt` は拡張子だけなら plaintext（継承だけ）
        assert_eq!(id(Path::new("proj/CMakeLists.txt")), Some("cmake"));
        assert_eq!(id(Path::new("proj/notes.txt")), Some("plaintext"));
        assert_eq!(
            identify_or_default(Path::new("CMakeLists.txt")).enter,
            EnterRule::Brackets
        );
        assert_eq!(
            identify_or_default(Path::new("notes.txt")).enter,
            EnterRule::InheritOnly
        );
    }

    #[test]
    fn 大文字小文字を問わない() {
        assert_eq!(id(Path::new("Sketch.INO")), Some(ARDUINO));
        assert_eq!(id(Path::new("Photo.JPG")), Some("jpeg"));
        assert_eq!(id(Path::new("MAKEFILE")), Some("makefile"));
        assert_eq!(id(Path::new("gnumakefile")), Some("makefile"));
        assert_eq!(by_fence_token("Arduino").map(|row| row.id), Some(ARDUINO));
    }

    #[test]
    fn 表に無い形式と拡張子の無いパスは既定() {
        for path in [
            "README",
            "archive.tar.gz",
            ".bashrc",
            ".md",
            "",
            "trailing.",
            "a.unknownext",
        ] {
            let path = Path::new(path);
            assert_eq!(id(path), None, "{}", path.display());
            assert_eq!(identify_or_default(path), &CODE);
        }
        // 二重拡張子は最後の拡張子で引く
        assert_eq!(id(Path::new("types.d.ts")), Some("typescript"));
        assert_eq!(id(Path::new("a.b.c.py")), Some("python"));
    }

    #[test]
    fn 文字列とブロックコメントの書き方の内訳() {
        // 文書（Enter が継承だけの形式）と LaTeX は何も飛ばさない（`"` `'` は文の中の記号）
        for row in TABLE {
            if row.enter == EnterRule::InheritOnly || row.id == "latex" {
                assert_eq!(row.lexical, Lexical::PROSE, "{}", row.id);
            }
        }
        // `//` の行コメントを持つコードは C の系譜で `/* */` も持つ。新しい行で `lexical` を
        // 書き忘れると `/* ) */` の括弧を数える（#1691）。Zig にはブロックコメントが無い
        for row in TABLE {
            if row.comment == SLASHES && !matches!(row.id, "zig" | "asciidoc") {
                assert!(
                    row.lexical.block_comment.is_some(),
                    "{}: `//` の言語なのに `/* */` を持たない",
                    row.id
                );
            }
        }
        // Rust のライフタイム（`'a`）を文字列にしない
        assert_eq!(
            identify_or_default(Path::new("a.rs")).lexical.single_quote,
            SingleQuote::Char
        );
        assert_eq!(
            identify_or_default(Path::new("a.py")).lexical.single_quote,
            SingleQuote::String
        );
    }

    /// #1953: 既存の構文への別名（行の `syntax`）
    #[test]
    fn 別名で塗る形式の行() {
        let syntax = |path: &str| identify(Path::new(path)).and_then(|row| row.syntax);
        assert_eq!(syntax("lib.h"), Some("C++"));
        assert_eq!(syntax("LIB.H"), Some("C++"));
        assert_eq!(syntax("sketch.pde"), Some("Java"));
        assert_eq!(syntax("settings.jsonc"), Some("JSON"));
        assert_eq!(syntax("renovate.json5"), Some("JavaScript"));
        assert_eq!(id(Path::new("sketch.pde")), Some("processing"));
        assert_eq!(id(Path::new("config.json5")), Some("json5"));
        // `.pde` は `java` の行にしない（言語サーバの表が `java` を名指しても渡らない）
        assert_ne!(id(Path::new("sketch.pde")), id(Path::new("Main.java")));
        // `.m` と `.S` / `.s` は行を持たない（`.m` は本文で決める・`.S` は寄せない = #1953）
        for path in ["solve.m", "boot.S", "start.s"] {
            assert_eq!(id(Path::new(path)), None, "{path}");
        }
        for token in ["pde", "Processing", "json5", "JSONC"] {
            assert!(by_fence_token(token).is_some(), "```{token}");
        }
        // A/B が名指す行はすべて表に在り、構文を名指している
        for id in ALIASES_1953 {
            let row = TABLE.iter().find(|row| row.id == *id);
            assert!(row.and_then(|row| row.syntax).is_some(), "{id}");
        }
    }

    const MATLAB_SCRIPT: &str =
        "% 二次方程式を解く\nfunction x = solve(a, b, c)\n    x = -b / (2*a);\nend\n";
    const OBJC_SOURCE: &str =
        "#import \"Foo.h\"\n\n@implementation Foo\n- (int)one { return 1; }\n@end\n";

    /// #1953: `.m` は本文の手掛かりで MATLAB / Objective-C を決め、無ければ今のまま（two-face 任せ）
    #[test]
    fn mは本文の手掛かりで決める() {
        let m = Path::new("solve.m");
        assert_eq!(syntax_by_content(m, MATLAB_SCRIPT), Some("MATLAB"));
        assert_eq!(syntax_by_content(m, OBJC_SOURCE), Some("Objective-C"));
        assert_eq!(
            syntax_by_content(Path::new("Foo.M"), OBJC_SOURCE),
            Some("Objective-C")
        );
        // MATLAB の印は 1 つずつでも効く（コメントだけ・function だけ・classdef だけ）
        for text in [
            "%% セル\nx = 1;\n",
            "  %{\n  ブロックコメント\n  %}\n",
            "function [a, b] = f(x)\n",
            "function f\n",
            "classdef Point < handle\nend\n",
            "\u{feff}% BOM 付き\n",
            "x = 1;\r\n% CRLF\r\n",
        ] {
            assert_eq!(syntax_by_content(m, text), Some("MATLAB"), "{text:?}");
        }
        // Objective-C の印は MATLAB の印より強い（両方あれば Objective-C）
        for text in [
            "@interface Foo : NSObject\n@end\n",
            "#import <Foundation/Foundation.h>\n",
            "% 先に MATLAB 風の行\n@implementation Foo\n@end\n",
            "@protocol P\n@end\n",
        ] {
            assert_eq!(syntax_by_content(m, text), Some("Objective-C"), "{text:?}");
        }
        // 手掛かりが無い本文は今のまま（`None` = two-face の Objective-C）
        for text in [
            "",
            "x = linspace(0, 1, 10);\nplot(x, x.^2)\n",
            "#include <stdio.h>\nint main(void) { return 0; }\n",
            "functional = 1;\n",
            "#importx\n",
            "@endpoint = 1\n",
            "  x = 1; % 行末のコメントは数えない\n",
        ] {
            assert_eq!(syntax_by_content(m, text), None, "{text:?}");
        }
        // 先頭 SNIFF_LINES 行より後ろの手掛かりは見ない（全文を舐めない）
        let late = format!("{}% 遅い手掛かり\n", "x = 1;\n".repeat(SNIFF_LINES));
        assert_eq!(syntax_by_content(m, &late), None);
        let edge = format!("{}% 最後の行\n", "x = 1;\n".repeat(SNIFF_LINES - 1));
        assert_eq!(syntax_by_content(m, &edge), Some("MATLAB"));
    }

    /// #1953: `.h` は Objective-C のヘッダだけ本文で Objective-C、ほかは行の C++ のまま
    #[test]
    fn hはobjective_cのヘッダだけ本文で決める() {
        let h = Path::new("lib.h");
        assert_eq!(
            syntax_by_content(
                h,
                "#import <Foundation/Foundation.h>\n@interface Foo : NSObject\n@end\n"
            ),
            Some("Objective-C")
        );
        // C / C++ のヘッダと MATLAB 風の本文は `None` = 行の C++
        for text in [
            "#ifndef LIB_H\n#define LIB_H\nint add(int a, int b);\n#endif\n",
            "#pragma once\nnamespace geo { template <typename T> struct P { T x; }; }\n",
            "% 何かの断片\n",
            "",
        ] {
            assert_eq!(syntax_by_content(h, text), None, "{text:?}");
        }
        // ほかの拡張子・拡張子の無いパスは本文を見ない
        for path in ["a.hpp", "a.cpp", "a.matlab", "Makefile", "m", "h", ""] {
            assert_eq!(
                syntax_by_content(Path::new(path), OBJC_SOURCE),
                None,
                "{path}"
            );
            assert_eq!(
                syntax_by_content(Path::new(path), MATLAB_SCRIPT),
                None,
                "{path}"
            );
        }
    }

    /// #1759: 字下げを戻すキーワード・リストの印の引き継ぎ・括弧の自動閉じの内訳
    #[test]
    fn enterの続きと自動閉じの内訳() {
        for row in TABLE {
            // 文書（Enter が継承だけ）は括弧を閉じない（`[` はリンク・`(` は括弧書き）
            if row.enter == EnterRule::InheritOnly {
                assert!(!row.auto_close, "{}: 文書なのに括弧を自動で閉じる", row.id);
            }
            // リストの印を引き継ぐのは文書だけ（YAML の `- ` を継がない）
            if row.list_continuation {
                assert_eq!(row.enter, EnterRule::InheritOnly, "{}", row.id);
            }
            // 戻すキーワードは語（空白・記号を含まない）
            for word in row.dedent_after {
                assert!(
                    !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric()),
                    "{}: 戻すキーワード {word:?} は語ではない",
                    row.id
                );
            }
        }
        let py = identify_or_default(Path::new("a.py"));
        assert_eq!(py.dedent_after, BLOCK_EXITS);
        assert!(py.auto_close);
        assert!(identify_or_default(Path::new("README.md")).list_continuation);
        assert!(identify_or_default(Path::new("a.mdx")).list_continuation);
        assert!(!identify_or_default(Path::new("a.yaml")).list_continuation);
        assert!(!identify_or_default(Path::new("notes.txt")).list_continuation);
        assert!(!identify_or_default(Path::new("README.md")).auto_close);
        // 表に無い形式（コードの既定）は閉じ、戻すキーワードもリストの引き継ぎも無い
        assert!(CODE.auto_close && CODE.dedent_after.is_empty() && !CODE.list_continuation);
    }

    #[test]
    fn enter_rule_の内訳() {
        assert!(!EnterRule::InheritOnly.brackets());
        assert!(!EnterRule::InheritOnly.colon());
        assert!(EnterRule::Brackets.brackets());
        assert!(!EnterRule::Brackets.colon());
        assert!(EnterRule::BracketsAndColon.brackets());
        assert!(EnterRule::BracketsAndColon.colon());
    }
}
