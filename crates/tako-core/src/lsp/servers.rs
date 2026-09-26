//! 言語サーバの検出表（#1678）。**言語の追加はこの表への行追加だけ**で済む形にする
//!
//! ユーザー要望（#1007、2026-08-28）は「ゆくゆくは HTML とかネット系の言語や java とか、
//! とにかく色々対応させたい」で、第 2 波以降の追加コストはこの表の形で決まる。
//!
//! **表以外に言語名・サーバ名を書かない**。`if language == "…"` のような分岐を散らすと
//! 追加が「行追加だけ」で済まなくなる。番犬（`crates/tako-control/tests/issue1678_lsp_watchdog.rs`）が
//! LSP のモジュールに表の値が文字列として現れたら file:line で名指す。
//!
//! 未導入のときの導入コマンドは**コマンドそのもの**を持つ（言語に依存しない文字列なので
//! 日英の訳が要らない）。「見つからない」「入れるには」という日英の文は tako-control 側の
//! 1 か所（`tako_control::lsp::text`）が持ち、ここは行ごとの差だけを持つ。

use std::path::Path;

/// 表の 1 行 = 1 つの言語サーバ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerSpec {
    /// 安定 ID（CLI の `--name`・状態表示・env の差し替えのキー）
    pub id: &'static str,
    /// このサーバが受け持つ拡張子と、LSP へ申告する `languageId`
    pub documents: &'static [DocumentKind],
    /// 実行ファイル名。解決は必ず `platform::exe::find` を通す
    pub program: &'static str,
    /// 起動引数
    pub args: &'static [&'static str],
    /// ルートの印。ファイルの場所から上へ辿って最初に見つかったものを rootUri にする
    pub root_markers: &'static [&'static str],
    /// 印が入れ子になる言語の「最上位」判定（例: Cargo の workspace）。無ければ `None`
    pub workspace_root: Option<WorkspaceRoot>,
    /// 未導入のときに案内する導入コマンド
    pub install: InstallHint,
}

/// 拡張子 1 つぶん
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentKind {
    /// 先頭の `.` を含まない拡張子（大文字小文字は区別しない）
    pub extension: &'static str,
    /// LSP の `languageId`
    pub language_id: &'static str,
}

/// 入れ子の印のうち「最上位」を採る判定（[`crate::lsp::root`] が使う）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceRoot {
    /// 見るファイル名
    pub file: &'static str,
    /// このファイルがこれを含んでいれば workspace の根とみなす
    pub contains: &'static str,
}

/// OS ごとの導入コマンド（#983 の「理由 + 次の一手」の次の一手）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstallHint {
    pub macos: &'static str,
    pub windows: &'static str,
}

impl InstallHint {
    /// 実行中の OS の導入コマンド
    pub fn command(&self) -> &'static str {
        self.command_for(cfg!(windows))
    }

    /// OS を指定しての導入コマンド（**macOS 上から Windows 側を検証できる**よう純粋関数にする）
    pub fn command_for(&self, windows: bool) -> &'static str {
        if windows {
            self.windows
        } else {
            self.macos
        }
    }
}

const fn doc(extension: &'static str, language_id: &'static str) -> DocumentKind {
    DocumentKind {
        extension,
        language_id,
    }
}

/// 段階 1 の 4 行（ユーザー確定の順。#1007）:
/// Rust → C/C++（「ヘッダ」= `#include` のヘッダ）→ TypeScript / JavaScript → Python
pub const SERVERS: &[ServerSpec] = &[
    ServerSpec {
        id: "rust-analyzer",
        documents: &[doc("rs", "rust")],
        program: "rust-analyzer",
        args: &[],
        root_markers: &["Cargo.toml"],
        workspace_root: Some(WorkspaceRoot {
            file: "Cargo.toml",
            contains: "[workspace]",
        }),
        install: InstallHint {
            macos: "rustup component add rust-analyzer",
            windows: "rustup component add rust-analyzer",
        },
    },
    ServerSpec {
        id: "clangd",
        documents: &[
            doc("c", "c"),
            doc("h", "c"),
            doc("cc", "cpp"),
            doc("cpp", "cpp"),
            doc("cxx", "cpp"),
            doc("hh", "cpp"),
            doc("hpp", "cpp"),
            doc("hxx", "cpp"),
        ],
        program: "clangd",
        args: &[],
        root_markers: &["compile_commands.json", "compile_flags.txt", ".clangd"],
        workspace_root: None,
        install: InstallHint {
            macos: "brew install llvm",
            windows: "winget install LLVM.LLVM",
        },
    },
    ServerSpec {
        id: "typescript-language-server",
        documents: &[
            doc("ts", "typescript"),
            doc("mts", "typescript"),
            doc("cts", "typescript"),
            doc("tsx", "typescriptreact"),
            doc("js", "javascript"),
            doc("mjs", "javascript"),
            doc("cjs", "javascript"),
            doc("jsx", "javascriptreact"),
        ],
        program: "typescript-language-server",
        args: &["--stdio"],
        root_markers: &["tsconfig.json", "jsconfig.json", "package.json"],
        workspace_root: None,
        install: InstallHint {
            macos: "npm install -g typescript-language-server typescript",
            windows: "npm install -g typescript-language-server typescript",
        },
    },
    ServerSpec {
        id: "pyright",
        documents: &[doc("py", "python"), doc("pyi", "python")],
        program: "pyright-langserver",
        args: &["--stdio"],
        root_markers: &[
            "pyrightconfig.json",
            "pyproject.toml",
            "setup.py",
            "setup.cfg",
            "requirements.txt",
        ],
        workspace_root: None,
        install: InstallHint {
            macos: "npm install -g pyright",
            windows: "npm install -g pyright",
        },
    },
];

/// 解決結果（どのサーバが・どの `languageId` で受け持つか）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved<'a> {
    pub spec: &'a ServerSpec,
    pub language_id: &'a str,
}

/// ファイルを受け持つサーバを表から引く
pub fn resolve(path: &Path) -> Option<Resolved<'static>> {
    resolve_in(SERVERS, path)
}

/// 表を指定して引く（**表の行を足すと答えが変わる**ことを表以外に触れずに示すための入口）
pub fn resolve_in<'a>(table: &'a [ServerSpec], path: &Path) -> Option<Resolved<'a>> {
    let extension = path.extension()?.to_str()?;
    table.iter().find_map(|spec| {
        spec.documents
            .iter()
            .find(|kind| kind.extension.eq_ignore_ascii_case(extension))
            .map(|kind| Resolved {
                spec,
                language_id: kind.language_id,
            })
    })
}

/// ID で表を引く
pub fn find_in<'a>(table: &'a [ServerSpec], id: &str) -> Option<&'a ServerSpec> {
    table.iter().find(|spec| spec.id == id)
}

/// 実行ファイルの差し替えに使う環境変数名（`TAKO_LSP_BIN_<ID を大文字 + `_`>`）。
///
/// ID から機械的に作るので、行を足せば差し替えの口も同時に生える（表以外を触らない）
pub fn override_env_name(id: &str) -> String {
    let mut name = String::from("TAKO_LSP_BIN_");
    name.extend(id.chars().map(|c| {
        if c.is_ascii_alphanumeric() {
            c.to_ascii_uppercase()
        } else {
            '_'
        }
    }));
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 段階1は4行() {
        assert_eq!(
            SERVERS.len(),
            4,
            "表の行数が変わったら番犬とドキュメントも直す"
        );
    }

    #[test]
    fn id_と拡張子は表の中で重複しない() {
        let mut ids: Vec<_> = SERVERS.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), SERVERS.len());
        let mut extensions: Vec<_> = SERVERS
            .iter()
            .flat_map(|s| s.documents.iter().map(|d| d.extension))
            .collect();
        let total = extensions.len();
        extensions.sort_unstable();
        extensions.dedup();
        assert_eq!(
            extensions.len(),
            total,
            "同じ拡張子を 2 つのサーバが取り合っている"
        );
    }

    #[test]
    fn 拡張子から表の行を引ける() {
        for spec in SERVERS {
            for kind in spec.documents {
                let path = format!("dir/sample.{}", kind.extension);
                let got = resolve(Path::new(&path)).expect("表にある拡張子は引ける");
                assert_eq!(got.spec.id, spec.id);
                assert_eq!(got.language_id, kind.language_id);
                // 大文字の拡張子でも引ける
                let upper = format!("dir/SAMPLE.{}", kind.extension.to_ascii_uppercase());
                assert_eq!(resolve(Path::new(&upper)).map(|r| r.spec.id), Some(spec.id));
            }
        }
    }

    #[test]
    fn 表に無い拡張子と拡張子の無いファイルは引けない() {
        assert!(resolve(Path::new("notes.md")).is_none());
        assert!(resolve(Path::new("Makefile")).is_none());
        assert!(resolve(Path::new("")).is_none());
    }

    /// 受け入れ条件: 表に 1 行足すだけで解決結果が変わる（表以外のコード変更は 0 行）。
    /// ここで足すのは**テストの中の表**で、[`resolve_in`] も [`find_in`] も
    /// [`override_env_name`] も 1 文字も触っていない
    #[test]
    fn 表に1行足すだけで解決結果が変わる() {
        const 追加: ServerSpec = ServerSpec {
            id: "vscode-html-language-server",
            documents: &[doc("html", "html"), doc("htm", "html")],
            program: "vscode-html-language-server",
            args: &["--stdio"],
            root_markers: &["package.json"],
            workspace_root: None,
            install: InstallHint {
                macos: "npm install -g vscode-langservers-extracted",
                windows: "npm install -g vscode-langservers-extracted",
            },
        };
        let path = Path::new("site/index.html");
        assert!(
            resolve_in(SERVERS, path).is_none(),
            "足す前は誰も受け持たない"
        );
        let mut extended = SERVERS.to_vec();
        extended.push(追加);
        let got = resolve_in(&extended, path).expect("足した行が受け持つ");
        assert_eq!(got.spec.id, "vscode-html-language-server");
        assert_eq!(got.language_id, "html");
        assert_eq!(
            find_in(&extended, "vscode-html-language-server").map(|s| s.program),
            Some("vscode-html-language-server")
        );
        assert_eq!(
            override_env_name(got.spec.id),
            "TAKO_LSP_BIN_VSCODE_HTML_LANGUAGE_SERVER"
        );
        // 既存の行の答えは変わらない
        assert_eq!(
            resolve_in(&extended, Path::new("a.rs")).map(|r| r.spec.id),
            resolve(Path::new("a.rs")).map(|r| r.spec.id)
        );
    }

    #[test]
    fn 導入コマンドは_os_ごとに引ける() {
        let spec = find_in(SERVERS, "clangd").unwrap();
        assert_eq!(spec.install.command_for(false), spec.install.macos);
        assert_eq!(spec.install.command_for(true), spec.install.windows);
        assert_eq!(
            spec.install.command(),
            spec.install.command_for(cfg!(windows))
        );
        for spec in SERVERS {
            assert!(!spec.install.macos.is_empty() && !spec.install.windows.is_empty());
        }
    }
}
