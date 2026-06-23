//! ファイルアイコン — 拡張子・ファイル名からSVGアイコンパスと色を解決する
//!
//! Zed の file_icons クレートを参考に、GPUI の `svg()` で描画可能な
//! 単色 SVG アイコンを拡張子・特殊ファイル名で引き当てる。
//! SVG アセットは `assets/icons/file_icons/` に配置し、コンパイル時に
//! `include_bytes!` で埋め込む（AssetSource 経由で配信）。

use std::borrow::Cow;
use std::path::Path;

use gpui::{AssetSource, Result, SharedString};

/// コンパイル時に SVG を埋め込む AssetSource
pub struct TakoAssets;

macro_rules! icon_asset {
    ($name:literal) => {
        (
            concat!("icons/file_icons/", $name, ".svg"),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/icons/file_icons/",
                $name,
                ".svg"
            ))
            .as_slice(),
        )
    };
}

static EMBEDDED_ASSETS: &[(&str, &[u8])] = &[
    icon_asset!("archive"),
    icon_asset!("audio"),
    icon_asset!("book"),
    icon_asset!("c"),
    icon_asset!("chevron_down"),
    icon_asset!("chevron_right"),
    icon_asset!("code"),
    icon_asset!("cpp"),
    icon_asset!("css"),
    icon_asset!("dart"),
    icon_asset!("database"),
    icon_asset!("diff"),
    icon_asset!("docker"),
    icon_asset!("elixir"),
    icon_asset!("elm"),
    icon_asset!("erlang"),
    icon_asset!("file"),
    icon_asset!("folder"),
    icon_asset!("folder_open"),
    icon_asset!("font"),
    icon_asset!("fsharp"),
    icon_asset!("git"),
    icon_asset!("go"),
    icon_asset!("graphql"),
    icon_asset!("hash"),
    icon_asset!("haskell"),
    icon_asset!("hcl"),
    icon_asset!("html"),
    icon_asset!("image"),
    icon_asset!("java"),
    icon_asset!("javascript"),
    icon_asset!("julia"),
    icon_asset!("jupyter"),
    icon_asset!("kotlin"),
    icon_asset!("lock"),
    icon_asset!("lua"),
    icon_asset!("metal"),
    icon_asset!("nim"),
    icon_asset!("nix"),
    icon_asset!("ocaml"),
    icon_asset!("package"),
    icon_asset!("php"),
    icon_asset!("prettier"),
    icon_asset!("prisma"),
    icon_asset!("python"),
    icon_asset!("r"),
    icon_asset!("react"),
    icon_asset!("roc"),
    icon_asset!("ruby"),
    icon_asset!("rust"),
    icon_asset!("sass"),
    icon_asset!("scala"),
    icon_asset!("settings"),
    icon_asset!("swift"),
    icon_asset!("terminal"),
    icon_asset!("toml"),
    icon_asset!("typescript"),
    icon_asset!("video"),
    icon_asset!("vue"),
    icon_asset!("yaml"),
    icon_asset!("zig"),
];

impl AssetSource for TakoAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        for &(key, bytes) in EMBEDDED_ASSETS {
            if key == path {
                return Ok(Some(Cow::Borrowed(bytes)));
            }
        }
        Ok(None)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut result = Vec::new();
        let prefix = if path.ends_with('/') {
            path.to_string()
        } else {
            format!("{path}/")
        };
        for &(key, _) in EMBEDDED_ASSETS {
            if let Some(rest) = key.strip_prefix(&prefix) {
                if !rest.contains('/') {
                    result.push(SharedString::from(rest.to_string()));
                }
            }
        }
        Ok(result)
    }
}

/// ファイルアイコンの種別（SVG アセットパスに対応）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum FileIconKind {
    // 言語系
    Rust,
    TypeScript,
    JavaScript,
    Python,
    Go,
    C,
    Cpp,
    Java,
    Ruby,
    Swift,
    Kotlin,
    Dart,
    Elixir,
    Elm,
    Erlang,
    FSharp,
    Haskell,
    Julia,
    Lua,
    Metal,
    Nim,
    Nix,
    OCaml,
    Php,
    R,
    Roc,
    Scala,
    Zig,
    // Web 系
    Html,
    Css,
    Sass,
    React,
    Vue,
    GraphQL,
    // 設定・データ系
    Toml,
    Yaml,
    Settings,
    Lock,
    Database,
    Prisma,
    Hcl,
    // ドキュメント系
    Book,
    Diff,
    // メディア系
    Image,
    Audio,
    Video,
    Font,
    // ツール系
    Git,
    Docker,
    Terminal,
    Package,
    Archive,
    Code,
    Hash,
    Prettier,
    Jupyter,
    // 汎用
    File,
    // フォルダ
    Folder,
    FolderOpen,
    // シェブロン
    ChevronRight,
    ChevronDown,
}

impl FileIconKind {
    pub fn svg_path(self) -> &'static str {
        match self {
            Self::Rust => "icons/file_icons/rust.svg",
            Self::TypeScript => "icons/file_icons/typescript.svg",
            Self::JavaScript => "icons/file_icons/javascript.svg",
            Self::Python => "icons/file_icons/python.svg",
            Self::Go => "icons/file_icons/go.svg",
            Self::C => "icons/file_icons/c.svg",
            Self::Cpp => "icons/file_icons/cpp.svg",
            Self::Java => "icons/file_icons/java.svg",
            Self::Ruby => "icons/file_icons/ruby.svg",
            Self::Swift => "icons/file_icons/swift.svg",
            Self::Kotlin => "icons/file_icons/kotlin.svg",
            Self::Dart => "icons/file_icons/dart.svg",
            Self::Elixir => "icons/file_icons/elixir.svg",
            Self::Elm => "icons/file_icons/elm.svg",
            Self::Erlang => "icons/file_icons/erlang.svg",
            Self::FSharp => "icons/file_icons/fsharp.svg",
            Self::Haskell => "icons/file_icons/haskell.svg",
            Self::Julia => "icons/file_icons/julia.svg",
            Self::Lua => "icons/file_icons/lua.svg",
            Self::Metal => "icons/file_icons/metal.svg",
            Self::Nim => "icons/file_icons/nim.svg",
            Self::Nix => "icons/file_icons/nix.svg",
            Self::OCaml => "icons/file_icons/ocaml.svg",
            Self::Php => "icons/file_icons/php.svg",
            Self::R => "icons/file_icons/r.svg",
            Self::Roc => "icons/file_icons/roc.svg",
            Self::Scala => "icons/file_icons/scala.svg",
            Self::Zig => "icons/file_icons/zig.svg",
            Self::Html => "icons/file_icons/html.svg",
            Self::Css => "icons/file_icons/css.svg",
            Self::Sass => "icons/file_icons/sass.svg",
            Self::React => "icons/file_icons/react.svg",
            Self::Vue => "icons/file_icons/vue.svg",
            Self::GraphQL => "icons/file_icons/graphql.svg",
            Self::Toml => "icons/file_icons/toml.svg",
            Self::Yaml => "icons/file_icons/yaml.svg",
            Self::Settings => "icons/file_icons/settings.svg",
            Self::Lock => "icons/file_icons/lock.svg",
            Self::Database => "icons/file_icons/database.svg",
            Self::Prisma => "icons/file_icons/prisma.svg",
            Self::Hcl => "icons/file_icons/hcl.svg",
            Self::Book => "icons/file_icons/book.svg",
            Self::Diff => "icons/file_icons/diff.svg",
            Self::Image => "icons/file_icons/image.svg",
            Self::Audio => "icons/file_icons/audio.svg",
            Self::Video => "icons/file_icons/video.svg",
            Self::Font => "icons/file_icons/font.svg",
            Self::Git => "icons/file_icons/git.svg",
            Self::Docker => "icons/file_icons/docker.svg",
            Self::Terminal => "icons/file_icons/terminal.svg",
            Self::Package => "icons/file_icons/package.svg",
            Self::Archive => "icons/file_icons/archive.svg",
            Self::Code => "icons/file_icons/code.svg",
            Self::Hash => "icons/file_icons/hash.svg",
            Self::Prettier => "icons/file_icons/prettier.svg",
            Self::Jupyter => "icons/file_icons/jupyter.svg",
            Self::File => "icons/file_icons/file.svg",
            Self::Folder => "icons/file_icons/folder.svg",
            Self::FolderOpen => "icons/file_icons/folder_open.svg",
            Self::ChevronRight => "icons/file_icons/chevron_right.svg",
            Self::ChevronDown => "icons/file_icons/chevron_down.svg",
        }
    }

    /// テーマの HSLA 色配列インデックス（tako-core Theme の色フィールド名）
    pub fn color_category(self) -> IconColor {
        match self {
            // 言語系 → green
            Self::Rust
            | Self::Python
            | Self::Go
            | Self::C
            | Self::Cpp
            | Self::Java
            | Self::Ruby
            | Self::Swift
            | Self::Kotlin
            | Self::Dart
            | Self::Elixir
            | Self::Elm
            | Self::Erlang
            | Self::FSharp
            | Self::Haskell
            | Self::Julia
            | Self::Lua
            | Self::Metal
            | Self::Nim
            | Self::Nix
            | Self::OCaml
            | Self::Php
            | Self::R
            | Self::Roc
            | Self::Scala
            | Self::Zig
            | Self::Code => IconColor::Green,
            // Web 系 → accent
            Self::Html
            | Self::Css
            | Self::Sass
            | Self::React
            | Self::Vue
            | Self::GraphQL
            | Self::TypeScript
            | Self::JavaScript => IconColor::Accent,
            // 設定・データ系 → peach
            Self::Toml
            | Self::Yaml
            | Self::Settings
            | Self::Lock
            | Self::Database
            | Self::Prisma
            | Self::Hcl => IconColor::Peach,
            // ドキュメント系 → accent
            Self::Book | Self::Diff => IconColor::Accent,
            // メディア系 → mauve
            Self::Image | Self::Audio | Self::Video | Self::Font => IconColor::Mauve,
            // ツール系 → yellow
            Self::Git
            | Self::Docker
            | Self::Terminal
            | Self::Package
            | Self::Archive
            | Self::Hash
            | Self::Prettier
            | Self::Jupyter => IconColor::Yellow,
            // 汎用 → dim
            Self::File => IconColor::Dim,
            // フォルダ → accent
            Self::Folder | Self::FolderOpen => IconColor::Accent,
            // シェブロン → dim
            Self::ChevronRight | Self::ChevronDown => IconColor::Dim,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconColor {
    Green,
    Accent,
    Peach,
    Mauve,
    Yellow,
    Dim,
}

/// 特殊ファイル名で先にマッチする（拡張子より優先）
fn match_special_filename(name: &str) -> Option<FileIconKind> {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "cargo.toml" | "cargo.lock" => Some(FileIconKind::Rust),
        "package.json" | "package-lock.json" => Some(FileIconKind::Package),
        "tsconfig.json" | "tsconfig.base.json" => Some(FileIconKind::TypeScript),
        "readme.md" | "readme.txt" | "readme" | "changelog.md" | "license.md" | "license" => {
            Some(FileIconKind::Book)
        }
        ".gitignore" | ".gitmodules" | ".gitattributes" => Some(FileIconKind::Git),
        "dockerfile" | "docker-compose.yml" | "docker-compose.yaml" | ".dockerignore" => {
            Some(FileIconKind::Docker)
        }
        "makefile" | "justfile" | "rakefile" | "taskfile.yml" => Some(FileIconKind::Terminal),
        ".prettierrc" | ".prettierrc.json" | ".prettierrc.yml" | ".prettierignore" => {
            Some(FileIconKind::Prettier)
        }
        ".eslintrc" | ".eslintrc.json" | ".eslintrc.js" | "eslint.config.js"
        | "eslint.config.mjs" | "eslint.config.ts" => Some(FileIconKind::Settings),
        "gemfile" | "gemfile.lock" => Some(FileIconKind::Ruby),
        "go.mod" | "go.sum" => Some(FileIconKind::Go),
        "flake.nix" | "flake.lock" | "default.nix" | "shell.nix" => Some(FileIconKind::Nix),
        _ => None,
    }
}

/// 拡張子からアイコン種別を解決
fn match_extension(ext: &str) -> FileIconKind {
    match ext {
        // 言語系
        "rs" => FileIconKind::Rust,
        "ts" | "mts" | "cts" => FileIconKind::TypeScript,
        "tsx" => FileIconKind::React,
        "js" | "mjs" | "cjs" => FileIconKind::JavaScript,
        "jsx" => FileIconKind::React,
        "py" | "pyi" | "pyw" => FileIconKind::Python,
        "go" => FileIconKind::Go,
        "c" | "h" => FileIconKind::C,
        "cpp" | "cxx" | "cc" | "hpp" | "hxx" | "hh" => FileIconKind::Cpp,
        "java" | "class" | "jar" => FileIconKind::Java,
        "rb" | "erb" => FileIconKind::Ruby,
        "swift" => FileIconKind::Swift,
        "kt" | "kts" => FileIconKind::Kotlin,
        "dart" => FileIconKind::Dart,
        "ex" | "exs" | "heex" => FileIconKind::Elixir,
        "elm" => FileIconKind::Elm,
        "erl" | "hrl" => FileIconKind::Erlang,
        "fs" | "fsx" | "fsi" => FileIconKind::FSharp,
        "hs" | "lhs" => FileIconKind::Haskell,
        "jl" => FileIconKind::Julia,
        "lua" => FileIconKind::Lua,
        "metal" => FileIconKind::Metal,
        "nim" | "nimble" => FileIconKind::Nim,
        "nix" => FileIconKind::Nix,
        "ml" | "mli" => FileIconKind::OCaml,
        "php" => FileIconKind::Php,
        "r" | "rmd" => FileIconKind::R,
        "roc" => FileIconKind::Roc,
        "scala" | "sc" => FileIconKind::Scala,
        "zig" => FileIconKind::Zig,
        "cs" => FileIconKind::Code,
        // Web 系
        "html" | "htm" | "xhtml" => FileIconKind::Html,
        "css" => FileIconKind::Css,
        "scss" | "sass" | "less" => FileIconKind::Sass,
        "vue" => FileIconKind::Vue,
        "graphql" | "gql" => FileIconKind::GraphQL,
        "svelte" => FileIconKind::Code,
        // 設定・データ系
        "toml" => FileIconKind::Toml,
        "yaml" | "yml" => FileIconKind::Yaml,
        "json" | "jsonc" | "json5" => FileIconKind::Settings,
        "lock" => FileIconKind::Lock,
        "sql" | "sqlite" | "db" => FileIconKind::Database,
        "prisma" => FileIconKind::Prisma,
        "hcl" | "tf" | "tfvars" => FileIconKind::Hcl,
        "ini" | "cfg" | "conf" | "env" | "properties" => FileIconKind::Settings,
        "xml" | "xsl" | "xslt" => FileIconKind::Code,
        "csv" | "tsv" => FileIconKind::Database,
        // ドキュメント系
        "md" | "mdx" | "markdown" => FileIconKind::Book,
        "txt" | "text" | "rst" | "adoc" => FileIconKind::Book,
        "tex" | "latex" | "bib" => FileIconKind::Book,
        "pdf" => FileIconKind::Book,
        "patch" | "diff" => FileIconKind::Diff,
        "ipynb" => FileIconKind::Jupyter,
        // メディア系
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" | "bmp" | "tiff" | "tif"
        | "avif" => FileIconKind::Image,
        "mp3" | "wav" | "ogg" | "flac" | "aac" | "wma" | "m4a" => FileIconKind::Audio,
        "mp4" | "avi" | "mov" | "wmv" | "mkv" | "webm" | "flv" | "m4v" => FileIconKind::Video,
        "ttf" | "otf" | "woff" | "woff2" | "eot" => FileIconKind::Font,
        // アーカイブ系
        "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" | "zst" | "tgz" => FileIconKind::Archive,
        // シェル系
        "sh" | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd" => FileIconKind::Terminal,
        // その他
        _ => FileIconKind::File,
    }
}

/// パスからアイコン種別を解決（特殊ファイル名 → 拡張子の優先順）
pub fn resolve_file_icon(path: &Path) -> FileIconKind {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    if let Some(kind) = match_special_filename(name) {
        return kind;
    }

    // 隠しファイル（ドットファイル）
    if name.starts_with('.') && !name.contains('.') {
        return FileIconKind::Settings;
    }

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    match_extension(ext)
}

/// フォルダアイコン（展開状態で切り替え）
pub fn folder_icon(expanded: bool) -> FileIconKind {
    if expanded {
        FileIconKind::FolderOpen
    } else {
        FileIconKind::Folder
    }
}

/// シェブロンアイコン
pub fn chevron_icon(expanded: bool) -> FileIconKind {
    if expanded {
        FileIconKind::ChevronDown
    } else {
        FileIconKind::ChevronRight
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn special_filenames() {
        assert_eq!(
            resolve_file_icon(Path::new("Cargo.toml")),
            FileIconKind::Rust
        );
        assert_eq!(
            resolve_file_icon(Path::new("package.json")),
            FileIconKind::Package
        );
        assert_eq!(
            resolve_file_icon(Path::new("README.md")),
            FileIconKind::Book
        );
        assert_eq!(
            resolve_file_icon(Path::new(".gitignore")),
            FileIconKind::Git
        );
        assert_eq!(
            resolve_file_icon(Path::new("Dockerfile")),
            FileIconKind::Docker
        );
    }

    #[test]
    fn extensions() {
        assert_eq!(resolve_file_icon(Path::new("main.rs")), FileIconKind::Rust);
        assert_eq!(
            resolve_file_icon(Path::new("index.ts")),
            FileIconKind::TypeScript
        );
        assert_eq!(resolve_file_icon(Path::new("App.tsx")), FileIconKind::React);
        assert_eq!(
            resolve_file_icon(Path::new("data.json")),
            FileIconKind::Settings
        );
        assert_eq!(
            resolve_file_icon(Path::new("photo.png")),
            FileIconKind::Image
        );
    }

    #[test]
    fn folder_icons() {
        assert_eq!(folder_icon(true), FileIconKind::FolderOpen);
        assert_eq!(folder_icon(false), FileIconKind::Folder);
    }

    #[test]
    fn all_icons_have_valid_svg_paths() {
        let kinds = [
            FileIconKind::Rust,
            FileIconKind::TypeScript,
            FileIconKind::JavaScript,
            FileIconKind::Python,
            FileIconKind::File,
            FileIconKind::Folder,
            FileIconKind::FolderOpen,
            FileIconKind::ChevronRight,
            FileIconKind::ChevronDown,
        ];
        for kind in kinds {
            let path = kind.svg_path();
            assert!(path.starts_with("icons/file_icons/"));
            assert!(path.ends_with(".svg"));
        }
    }

    #[test]
    fn asset_source_loads_embedded() {
        let assets = TakoAssets;
        let result = assets.load("icons/file_icons/rust.svg").unwrap();
        assert!(result.is_some());
        let bytes = result.unwrap();
        assert!(!bytes.is_empty());
        assert!(std::str::from_utf8(&bytes).unwrap().contains("<svg"));
    }

    #[test]
    fn asset_source_returns_none_for_unknown() {
        let assets = TakoAssets;
        let result = assets.load("icons/file_icons/nonexistent.svg").unwrap();
        assert!(result.is_none());
    }
}
