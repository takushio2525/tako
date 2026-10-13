//! 言語の同定の正本（Issue #1948 / エピック #1943）。純粋関数だけで I/O をしない
//!
//! 「このファイルは何の形式か」を**ファイル名 → 拡張子**の順で決める表をここ 1 か所に置く。
//! 次がすべてこの表を引く:
//!
//! - 色分け（tako-app の構文の選び方。`syntax` → two-face の順）と Markdown のコードブロックの別名
//! - プレビューの種別・HTTP の型（[`crate::open_plan`]。tako-app の画像の形式・Markdown 判定も同じ）
//! - インデントの既定と Enter の規則（[`crate::text_edit`]）
//! - 行コメントの記号（⌘/ と `tako edit line toggle-comment`。#1692）
//! - CLI / MCP の応答の `file_type`（`tako open` の結果・`tako edit` の応答）
//!
//! **全 213 構文を並べない**。行を持つのは「既定（[`CODE`] = code・two-face 任せ・4 スペース・
//! 括弧で深く）から外れる形式」と「挙動の表が名指す形式」だけで、表に無い形式は既定のまま。
//!
//! 挙動の表（`lsp::servers` / `platform::runner_defaults` / `runner_project`）は**分けたまま**
//! （#1726 設計 §9.2。1 枚にすると疎な表になる）。代わりに番犬
//! `crates/tako-control/tests/issue1948_file_type_watchdog.rs` が
//! 「挙動の表が名指す拡張子・ファイル名は必ずここに行がある」と
//! 「`preview.rs` / `text_edit.rs` / `open_plan.rs` に拡張子の文字列リテラルの `match` を書かない」
//! を強制する。設計は `.agent/plans/2026-10-language-coverage.md` §4

use std::path::Path;

use crate::open_plan::PreviewRoute;
use crate::text_edit::{CommentSyntax, IndentUnit};

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
    /// 行コメントの記号（#1692）。`None` = コメントの記号を持たない形式（プレーンテキスト・JSON・
    /// 表に無い形式）で、コメントの付け外しは本文を触らずに理由を返す
    pub comment: Option<CommentSyntax>,
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
    comment: None,
};

/// Arduino のスケッチ（#1949）。`TAKO_1949_LEGACY=1` の A/B が名指す
pub const ARDUINO: &str = "arduino";

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

/// Enter で深くしない文書
const fn prose(id: &'static str, extensions: &'static [&'static str]) -> FileType {
    FileType {
        id,
        extensions,
        enter: EnterRule::InheritOnly,
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
        comment: MARKUP,
        ..CODE
    },
    prose("mdx", &["mdx"]),
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
        ..CODE
    },
    FileType {
        id: "go",
        extensions: &["go"],
        indent: IndentUnit::Tab,
        comment: SLASHES,
        ..CODE
    },
    // ─── `:` でも深くするもの ───────────────────────────────────────
    FileType {
        id: "python",
        extensions: &["py", "pyi", "pyw"],
        enter: EnterRule::BracketsAndColon,
        comment: HASH,
        ..CODE
    },
    FileType {
        id: "yaml",
        extensions: &["yml", "yaml"],
        enter: EnterRule::BracketsAndColon,
        comment: HASH,
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
        ..CODE
    },
    FileType {
        id: "javascriptreact",
        extensions: &["jsx"],
        syntax: Some("JavaScript"),
        comment: SLASHES,
        ..CODE
    },
    lang("typescript", &["ts", "mts", "cts"], SLASHES),
    lang("typescriptreact", &["tsx"], SLASHES),
    // `.ino` は C++ として塗る（#1949）。two-face に Arduino の構文は無く、VS Code 同梱の
    // cpp 言語定義も `.ino` を C++ に含める。寄せるのは**色だけ**で、言語サーバは別
    // （clangd へ `.ino` を渡すと正しいスケッチに嘘のエラーが出る = #1951）
    FileType {
        id: ARDUINO,
        extensions: &["ino"],
        syntax: Some("C++"),
        fence_tokens: &["ino", "arduino"],
        comment: SLASHES,
        ..CODE
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
    named("just", &["Justfile"], "Makefile", HASH),
    // ─── 挙動の表（言語サーバ・実行の既定・実行のプロジェクト）が名指す言語 ──────
    lang("c", &["c"], SLASHES),
    // `.h` は VS Code と同じく C++ のヘッダとして数える（色は今は two-face 任せ = #1953）
    lang(
        "cpp",
        &["cpp", "cc", "cxx", "h", "hh", "hpp", "hxx"],
        SLASHES,
    ),
    lang("rust", &["rs"], SLASHES),
    lang("shellscript", &["sh", "bash", "zsh", "command"], HASH),
    lang("powershell", &["ps1"], HASH),
    lang("bat", &["bat", "cmd"], Some(CommentSyntax::Line("REM"))),
    lang("ruby", &["rb"], HASH),
    lang("perl", &["pl"], HASH),
    lang("php", &["php"], SLASHES),
    lang("lua", &["lua"], DASHES),
    lang("java", &["java"], SLASHES),
    lang("kotlin", &["kt"], SLASHES),
    lang("scala", &["scala"], SLASHES),
    lang("swift", &["swift"], SLASHES),
    lang("csharp", &["cs"], SLASHES),
    lang("fsharp", &["fs"], SLASHES),
    lang("vb", &["vb"], Some(CommentSyntax::Line("'"))),
    lang("dart", &["dart"], SLASHES),
    lang("r", &["r"], HASH),
    lang("julia", &["jl"], HASH),
    lang("haskell", &["hs"], DASHES),
    lang("clojure", &["clj"], SEMICOLON),
    lang("elixir", &["ex"], HASH),
    lang("zig", &["zig"], SLASHES),
    lang("nim", &["nim"], HASH),
    lang("sql", &["sql"], DASHES),
    lang("latex", &["tex"], Some(CommentSyntax::Line("%"))),
    // ノートブックは JSON（セルの中のコメントはノートブックの編集の外）
    lang("jupyter", &["ipynb"], None),
    // ─── コメントの記号を言うために行を持つ言語（#1692。色・インデント・Enter は既定のまま） ─
    lang("html", &["html", "htm"], MARKUP),
    lang("xml", &["xml"], MARKUP),
    lang("vue", &["vue"], MARKUP),
    lang("css", &["css"], C_BLOCK),
    lang("scss", &["scss"], SLASHES),
    lang("less", &["less"], SLASHES),
    lang("jsonc", &["jsonc"], SLASHES),
    lang("toml", &["toml"], HASH),
    FileType {
        id: "dockerfile",
        file_names: &["Dockerfile"],
        comment: HASH,
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
    fn enter_rule_の内訳() {
        assert!(!EnterRule::InheritOnly.brackets());
        assert!(!EnterRule::InheritOnly.colon());
        assert!(EnterRule::Brackets.brackets());
        assert!(!EnterRule::Brackets.colon());
        assert!(EnterRule::BracketsAndColon.brackets());
        assert!(EnterRule::BracketsAndColon.colon());
    }
}
