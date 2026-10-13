//! #1948: 言語の同定を `tako_core::file_type` へ寄せる前後で、挙動が全数一致することの検査
//!
//! 「寄せる前」は `d4b83de`（main）の判定を**そのまま写した** [`pre_1948`]。
//! 入力は two-face の全構文の `file_extensions` 全部（拡張子としても・ファイル名としても・
//! 大文字にしても）・設計書 §2 の 45 形式・挙動の表 3 本の拡張子とファイル名・
//! 端（拡張子なし・二重拡張子・dotfile・末尾の `.`・大文字）。
//!
//! 比べる列: 構文（同じ構文セットの中の**同一の参照**）・プレビュー種別・HTTP の型・画像の形式・
//! Markdown か・インデントの既定・Enter の規則（`TextBuffer` で実際に Enter を押して観測）・
//! Markdown のコードブロックの構文。
//!
//! この単体は**寄せる前のコードでも緑**だった（写しが寄せる前の判定と一致することの確認）。
//! 寄せた後の差分は [`intended`] の 2 種だけで、それ以外の 1 件でも食い違えば落ちる。

use super::*;
use std::path::PathBuf;
use syntect::parsing::{SyntaxReference, SyntaxSet};
use tako_core::open_plan::PreviewRoute;
use tako_core::text_edit::{IndentUnit, TextBuffer};

/// `d4b83de` の判定の写し（**この Issue の前の挙動の正本**。書き換えない）
mod pre_1948 {
    use super::*;

    /// 写し: `SyntectHighlighter::syntax_for_path`（`TAKO_1949_LEGACY` は未設定 = 既定）
    pub(super) fn syntax<'a>(ss: &'a SyntaxSet, path: &Path, text: &str) -> &'a SyntaxReference {
        let file_name = path.file_name().and_then(|v| v.to_str());
        if let Some(syn) = file_name.and_then(|name| {
            ss.find_syntax_by_extension(name)
                .or_else(|| filename_to_syntax(ss, name))
        }) {
            return syn;
        }
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase);
        extension
            .as_deref()
            .and_then(|ext| {
                ss.find_syntax_by_extension(ext)
                    .or_else(|| extension_fallback(ss, ext))
            })
            .or_else(|| {
                text.lines()
                    .next()
                    .and_then(|line| ss.find_syntax_by_first_line(line))
            })
            .unwrap_or_else(|| ss.find_syntax_plain_text())
    }

    /// 写し: `SyntectHighlighter::filename_to_syntax`
    fn filename_to_syntax<'a>(ss: &'a SyntaxSet, name: &str) -> Option<&'a SyntaxReference> {
        let mapped = match name {
            "Cargo.lock" | "Pipfile" | "Pipfile.lock" | "poetry.lock" => "toml",
            ".dockerignore" => "gitignore",
            ".editorconfig" | ".npmrc" | ".yarnrc" => "ini",
            ".eslintrc" | ".prettierrc" | ".babelrc" => "json",
            "Justfile" | "justfile" => "makefile",
            _ => return None,
        };
        ss.find_syntax_by_extension(mapped)
            .or_else(|| ss.find_syntax_by_name(mapped))
    }

    /// 写し: `SyntectHighlighter::extension_fallback` + `extension_alias(ext, false)`
    fn extension_fallback<'a>(ss: &'a SyntaxSet, ext: &str) -> Option<&'a SyntaxReference> {
        let mapped = match ext {
            "jsx" => "js",
            "mjs" | "cjs" => "js",
            "mts" | "cts" => "ts",
            "ino" => "cpp",
            _ => return None,
        };
        ss.find_syntax_by_extension(mapped)
    }

    /// 写し: `highlight_lang` の構文の選び方 + `fence_alias(token, false)`
    pub(super) fn token_syntax<'a>(ss: &'a SyntaxSet, lang: &str) -> &'a SyntaxReference {
        ss.find_syntax_by_token(lang)
            .or_else(|| {
                let arduino = ["ino", "arduino"]
                    .iter()
                    .any(|alias| lang.eq_ignore_ascii_case(alias));
                arduino
                    .then_some("cpp")
                    .and_then(|token| ss.find_syntax_by_token(token))
            })
            .unwrap_or_else(|| ss.find_syntax_plain_text())
    }

    /// 写し: `open_plan::preview_route`（+ `preview_route_for_extension`）
    pub(super) fn preview_route(path: &Path) -> PreviewRoute {
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            return PreviewRoute::Code;
        };
        match ext.to_ascii_lowercase().as_str() {
            "md" | "markdown" => PreviewRoute::Markdown,
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" => PreviewRoute::Image,
            "pdf" => PreviewRoute::Pdf,
            "mp4" | "webm" | "mov" | "avi" | "mkv" => PreviewRoute::Video,
            _ => PreviewRoute::Code,
        }
    }

    /// 写し: `open_plan::media_type`（+ `media_type_for_extension`）
    pub(super) fn media_type(path: &Path) -> Option<&'static str> {
        let ext = path.extension().and_then(|e| e.to_str())?;
        Some(match ext.to_ascii_lowercase().as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "svg" => "image/svg+xml",
            "mp4" => "video/mp4",
            "webm" => "video/webm",
            "mov" => "video/quicktime",
            "avi" => "video/x-msvideo",
            "mkv" => "video/x-matroska",
            "pdf" => "application/pdf",
            "md" | "markdown" => "text/markdown; charset=utf-8",
            _ => return None,
        })
    }

    /// 写し: `preview::image_format_from_path`
    pub(super) fn image_format(path: &Path) -> Option<ImageFileFormat> {
        let ext = path.extension()?.to_str()?;
        match ext.to_ascii_lowercase().as_str() {
            "png" => Some(ImageFileFormat::Png),
            "jpg" | "jpeg" => Some(ImageFileFormat::Jpeg),
            "gif" => Some(ImageFileFormat::Gif),
            "webp" => Some(ImageFileFormat::WebP),
            "svg" => Some(ImageFileFormat::Svg),
            _ => None,
        }
    }

    /// 写し: `preview::is_markdown_path`
    pub(super) fn is_markdown(path: &Path) -> bool {
        matches!(
            path.extension().and_then(|e| e.to_str()),
            Some(ext) if ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown")
        )
    }

    /// 写し: `text_edit::IndentUnit::for_path`
    pub(super) fn indent(path: &Path) -> IndentUnit {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if matches!(name.as_str(), "makefile" | "gnumakefile")
            || matches!(ext.as_str(), "mk" | "go")
        {
            IndentUnit::Tab
        } else {
            IndentUnit::Spaces(4)
        }
    }

    /// 写し: `text_edit::IndentRules::for_path`（`(括弧で深く, ':' で深く)`）
    pub(super) fn enter(path: &Path) -> (bool, bool) {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        match ext.as_str() {
            "md" | "markdown" | "mdx" | "txt" | "rst" | "adoc" => (false, false),
            "py" | "pyi" | "pyw" | "yml" | "yaml" => (true, true),
            _ => (true, false),
        }
    }
}

/// Enter の規則を `TextBuffer` で**実際に Enter を押して**観測する（`(括弧で深く, ':' で深く)`）
fn observed_enter(path: &Path) -> (bool, bool) {
    let deeper = |line: &str| {
        let mut buffer = TextBuffer::from_text(path.to_path_buf(), line.to_string());
        buffer.set_cursor(line.len(), false);
        buffer.newline_and_indent();
        // 改行の後ろ（新しい行）にインデントが入ったか。改行コードには依らない
        buffer.text().rsplit('\n').next() != Some("")
    };
    (deeper("a{"), deeper("a:"))
}

/// 設計書 §2 の 45 形式（`.agent/plans/2026-10-language-coverage.md`）
const DESIGN_FORMS: &[&str] = &[
    "sketch.ino",
    "sketch.pde",
    "lib.h",
    "lib.hpp",
    "main.c",
    "main.cpp",
    "boot.S",
    "Main.java",
    "main.py",
    "solve.m",
    "stats.r",
    "stats.R",
    "model.jl",
    "report.tex",
    "lab.ipynb",
    "App.vue",
    "App.svelte",
    "App.jsx",
    "App.tsx",
    "index.astro",
    "style.scss",
    "config.toml",
    "config.yaml",
    "config.json5",
    "config.ini",
    ".env",
    "Dockerfile",
    "Makefile",
    "CMakeLists.txt",
    "run.sh",
    "run.zsh",
    "run.ps1",
    "run.bat",
    "init.lua",
    "app.rb",
    "tool.pl",
    "main.go",
    "main.rs",
    "Main.kt",
    "main.swift",
    "Program.cs",
    "main.dart",
    "main.zig",
    "query.sql",
    "api.proto",
    "schema.graphql",
];

/// 寄せる前の表が名指していた特殊なファイル名（`filename_to_syntax`）と、その大文字小文字違い
const SPECIAL_NAMES: &[&str] = &[
    "Cargo.lock",
    "Pipfile",
    "Pipfile.lock",
    "poetry.lock",
    ".dockerignore",
    ".editorconfig",
    ".npmrc",
    ".yarnrc",
    ".eslintrc",
    ".prettierrc",
    ".babelrc",
    "Justfile",
    "justfile",
    "makefile",
    "GNUmakefile",
    "gnumakefile",
    "MAKEFILE",
    "cmakelists.txt",
];

/// 端（拡張子なし・二重拡張子・dotfile・末尾の `.`・大文字の拡張子）
const EDGES: &[&str] = &[
    "README",
    "noext",
    ".bashrc",
    ".md",
    ".ino",
    "archive.tar.gz",
    "types.d.ts",
    "notes.MD",
    "Photo.JPG",
    "Sketch.INO",
    "App.JSX",
    "trailing.",
    "a.b.c.py",
    "dir.with.dot/file",
];

/// 1 行目の候補（拡張子・ファイル名で決まらないときに 1 行目で引く経路も比べる）
const FIRST_LINES: &[&str] = &["", "#!/usr/bin/env python3\nprint(1)\n"];

/// 比べる入力の全数
fn inputs(ss: &SyntaxSet) -> Vec<PathBuf> {
    let mut names: Vec<String> = Vec::new();
    for syntax in ss.syntaxes() {
        for ext in &syntax.file_extensions {
            for ext in [ext.clone(), ext.to_ascii_uppercase()] {
                names.push(format!("sample.{ext}"));
                names.push(ext);
            }
        }
    }
    let behavior_exts = tako_core::lsp::servers::SERVERS
        .iter()
        .flat_map(|s| s.documents.iter().map(|d| d.extension))
        .chain(
            tako_core::platform::runner_defaults::TABLE
                .iter()
                .map(|e| e.ext),
        )
        .chain(
            tako_core::runner_project::KINDS
                .iter()
                .flat_map(|k| k.exts.iter().copied()),
        );
    for ext in behavior_exts {
        names.push(format!("sample.{ext}"));
        names.push(format!("sample.{}", ext.to_ascii_uppercase()));
    }
    for kind in tako_core::runner_project::KINDS {
        names.extend(kind.file_names.iter().map(|n| n.to_string()));
    }
    // 言語の同定の正本の行（拡張子・ファイル名。大文字の版も）
    for row in tako_core::file_type::TABLE {
        for ext in row.extensions {
            names.push(format!("sample.{ext}"));
            names.push(format!("sample.{}", ext.to_ascii_uppercase()));
        }
        for name in row.file_names {
            names.push(name.to_string());
            names.push(name.to_ascii_uppercase());
            names.push(name.to_ascii_lowercase());
        }
    }
    for name in SPECIAL_NAMES {
        names.push(name.to_string());
        names.push(name.to_ascii_uppercase());
    }
    names.extend(DESIGN_FORMS.iter().map(|n| n.to_string()));
    names.extend(EDGES.iter().map(|n| n.to_string()));
    names.sort();
    names.dedup();
    let mut paths: Vec<PathBuf> = names
        .into_iter()
        .map(|n| Path::new("proj").join(n))
        .collect();
    // ファイル名が取れないパス
    paths.push(PathBuf::new());
    paths.push(PathBuf::from("proj/"));
    paths
}

/// 1 つのパスで食い違った列（無ければ空）
fn differences(hl: &SyntectHighlighter, path: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for text in FIRST_LINES {
        let before = pre_1948::syntax(&hl.syntaxes, path, text);
        let after = hl.syntax_for_path(path, text);
        if !std::ptr::eq(before, after) {
            out.push(format!(
                "構文（1 行目 {:?}）: {} → {}",
                text.lines().next().unwrap_or(""),
                before.name,
                after.name
            ));
        }
    }
    let route = tako_core::open_plan::preview_route(path);
    if pre_1948::preview_route(path) != route {
        out.push(format!(
            "プレビュー種別: {:?} → {route:?}",
            pre_1948::preview_route(path)
        ));
    }
    let media = tako_core::open_plan::media_type(path);
    if pre_1948::media_type(path) != media {
        out.push(format!(
            "HTTP の型: {:?} → {media:?}",
            pre_1948::media_type(path)
        ));
    }
    let image = image_format_from_path(path);
    if pre_1948::image_format(path) != image {
        out.push(format!(
            "画像の形式: {:?} → {image:?}",
            pre_1948::image_format(path)
        ));
    }
    if pre_1948::is_markdown(path) != is_markdown_path(path) {
        out.push(format!(
            "Markdown か: {} → {}",
            pre_1948::is_markdown(path),
            is_markdown_path(path)
        ));
    }
    let indent = IndentUnit::for_path(path);
    if pre_1948::indent(path) != indent {
        out.push(format!(
            "インデントの既定: {:?} → {indent:?}",
            pre_1948::indent(path)
        ));
    }
    let enter = observed_enter(path);
    if pre_1948::enter(path) != enter {
        out.push(format!(
            "Enter の規則: {:?} → {enter:?}",
            pre_1948::enter(path)
        ));
    }
    out
}

/// 意図した差分の種類（#1948 で名指して変えたもの）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Intended {
    /// `CMakeLists.txt` は拡張子だけなら `txt`（継承だけ）。ファイル名の行でコードとして扱う
    CmakeEnter,
    /// 特殊なファイル名の色の補いを、大文字小文字違いの綴りにも当てる（two-face のファイル名の
    /// 照合と同じ）。寄せる前は綴りが一致したときだけ当たっていた
    NamedSyntaxAnyCase,
}

/// 寄せる前の補い（`filename_to_syntax`）が当たっていた綴りと、その構文
const NAMED_SYNTAX: &[(&str, &str)] = &[
    (".dockerignore", "Git Ignore"),
    (".npmrc", "INI"),
    (".yarnrc", "INI"),
    (".eslintrc", "JSON"),
    (".prettierrc", "JSON"),
    (".babelrc", "JSON"),
    ("Justfile", "Makefile"),
    ("justfile", "Makefile"),
];

/// 食い違いが意図した差分ならその種類
fn intended(path: &Path, diff: &str) -> Option<Intended> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if name.eq_ignore_ascii_case("CMakeLists.txt")
        && diff == "Enter の規則: (false, false) → (true, false)"
    {
        return Some(Intended::CmakeEnter);
    }
    let any_case = NAMED_SYNTAX.iter().any(|(spelled, syntax)| {
        name.eq_ignore_ascii_case(spelled)
            && !NAMED_SYNTAX.iter().any(|(exact, _)| *exact == name)
            && diff.starts_with("構文")
            && diff.ends_with(&format!(" → {syntax}"))
    });
    any_case.then_some(Intended::NamedSyntaxAnyCase)
}

/// 受け入れ条件: 寄せる前後で全数一致する（意図した差分 2 種を除く）
#[test]
fn 言語の同定を寄せる前後で挙動が全数一致する() {
    let hl = SyntectHighlighter::new();
    let paths = inputs(&hl.syntaxes);
    // two-face の全拡張子（大文字・ファイル名の版を含む）が入っている = 入力が縮退していない
    assert!(paths.len() >= 2000, "入力が縮退している: {}", paths.len());
    let mut found: Vec<String> = Vec::new();
    let mut named: Vec<(Intended, String)> = Vec::new();
    for path in &paths {
        for diff in differences(&hl, path) {
            match intended(path, &diff) {
                Some(kind) => named.push((
                    kind,
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                )),
                None => found.push(format!("{}: {diff}", path.display())),
            }
        }
    }
    assert!(
        found.is_empty(),
        "寄せる前後で食い違った（{} 件）:\n{}",
        found.len(),
        found.join("\n")
    );
    // 意図した差分は実際に起きている（名指した差分が黙って消えたら、表の行が効いていない）
    named.sort();
    named.dedup();
    let names = |kind: Intended| -> Vec<&str> {
        named
            .iter()
            .filter(|(k, _)| *k == kind)
            .map(|(_, n)| n.as_str())
            .collect()
    };
    assert_eq!(
        names(Intended::CmakeEnter),
        vec!["CMAKELISTS.TXT", "CMakeLists.txt", "cmakelists.txt"]
    );
    assert_eq!(
        names(Intended::NamedSyntaxAnyCase),
        vec![
            ".BABELRC",
            ".DOCKERIGNORE",
            ".ESLINTRC",
            ".NPMRC",
            ".PRETTIERRC",
            ".YARNRC",
            "JUSTFILE",
        ]
    );
}

/// 受け入れ条件: Markdown のコードブロックの言語トークンも全数一致する
#[test]
fn コードブロックの言語トークンが寄せる前後で全数一致する() {
    let hl = SyntectHighlighter::new();
    let mut tokens: Vec<String> = Vec::new();
    for syntax in hl.syntaxes.syntaxes() {
        tokens.push(syntax.name.clone());
        tokens.push(syntax.name.to_ascii_lowercase());
        for ext in &syntax.file_extensions {
            tokens.push(ext.clone());
            tokens.push(ext.to_ascii_uppercase());
        }
    }
    for token in [
        "ino", "arduino", "INO", "Arduino", "jsx", "mjs", "pde", "", "unknown",
    ] {
        tokens.push(token.to_string());
    }
    tokens.sort();
    tokens.dedup();
    let mut found: Vec<String> = Vec::new();
    for token in &tokens {
        let before = pre_1948::token_syntax(&hl.syntaxes, token);
        let after = hl.syntax_for_token(token);
        if !std::ptr::eq(before, after) {
            found.push(format!("```{token}: {} → {}", before.name, after.name));
        }
    }
    assert!(
        found.is_empty(),
        "コードブロックの構文が食い違った:\n{}",
        found.join("\n")
    );
}
