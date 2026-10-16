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

use std::path::{Path, PathBuf};

use super::fetch::{Archive, Asset, Digest, Fetch, NpmPackage, PlatformAsset};
pub use super::init_options::InitOption;
use crate::app_bundle::AppBundle;

/// 表の 1 行 = 1 つの言語サーバ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerSpec {
    /// 安定 ID（CLI の `--name`・状態表示・env の差し替えのキー）
    pub id: &'static str,
    /// このサーバが受け持つ拡張子と、LSP へ申告する `languageId`
    pub documents: &'static [DocumentKind],
    /// 実行ファイル名。解決は必ず `platform::exe::find` を通す
    pub program: &'static str,
    /// 起動引数（並びのまま渡す）。固定の文字列のほかに、伴走の実行ファイルのパスと tako が
    /// 用意する空のファイルのパスを置ける（#1951。組み立ては [`super::launch`] の 1 実装）
    pub args: &'static [Arg],
    /// ルートの印。ファイルの場所から上へ辿って最初に見つかったものを rootUri にする
    pub root_markers: &'static [&'static str],
    /// ルートの取り方（#1951。ほとんどの行は [`RootScope::Project`]）
    pub root_scope: RootScope,
    /// フォルダの形で受け持つ文書（#2113。拡張子だけでは決まらない受け持ち）。無ければ `None`
    pub folder: Option<FolderClaim>,
    /// 印が入れ子になる言語の「最上位」判定（例: Cargo の workspace）。無ければ `None`
    pub workspace_root: Option<WorkspaceRoot>,
    /// PATH に無いときに探すアプリの同梱（#1951。サーバと伴走の両方を探す。置き場の知識は
    /// [`crate::app_bundle`] の表 1 枚 = Code Runner の #1950 と同じ 1 実装）。無ければ `None`
    pub bundled: Option<&'static AppBundle>,
    /// 起こす前に満たすべきこと（#1951）。満たさなければ起こさず、理由と満たすコマンドを出す
    pub requires: Option<Requirement>,
    /// 初期化オプション（`initialize` の `initializationOptions`）の既定（#2035）。無ければ空 =
    /// 送らない。利用者の上書きは settings.json の `lsp_init_options`（重ね方は
    /// [`super::init_options`] の 1 実装）
    pub init_options: &'static [InitOption],
    /// 未導入のときに案内する導入コマンド
    pub install: InstallHint,
    /// 見つからないときに tako が data dir へ取ってくる方法（#1944）。取れないサーバは `None`
    /// （導入コマンドの案内だけ = #1678 の振る舞い）
    pub fetch: Option<Fetch>,
}

/// 起動引数 1 つ（#1951）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arg {
    /// そのままの文字列（`--stdio` 等）
    Lit(&'static str),
    /// 伴走する実行ファイルのパス（名前を書く）。見つけたサーバと同じディレクトリ → PATH →
    /// 行の `bundled` の順に探し、無ければサーバを起こさない（未導入として理由を出す）
    Companion(&'static str),
    /// tako が `<data_dir>/lsp-config/<id>/<名前>` に用意する 0 バイトのファイルのパス
    /// （「設定ファイルのパスを必ず渡せ」と求めるが中身は要らないサーバ向け）
    EmptyFile(&'static str),
}

impl ServerSpec {
    /// 固定の文字列の引数だけ（tako が取ったサーバの起動 = 伴走も空のファイルも持たない行で使う）
    pub fn literal_args(&self) -> Vec<&'static str> {
        self.args
            .iter()
            .filter_map(|arg| match arg {
                Arg::Lit(value) => Some(*value),
                Arg::Companion(_) | Arg::EmptyFile(_) => None,
            })
            .collect()
    }

    /// 伴走の実行ファイルの名前（並びのまま）
    pub fn companions(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.args.iter().filter_map(|arg| match arg {
            Arg::Companion(name) => Some(*name),
            _ => None,
        })
    }
}

/// ルートの取り方（#1951）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootScope {
    /// 印を上へ辿る → `.git` → ファイルの場所（[`crate::lsp::root`] の冒頭の規則）
    Project,
    /// ファイルのあるディレクトリ（印も `.git` も見ない）。Arduino のスケッチはフォルダ単位で、
    /// 上の段のスケッチや repo の根をルートにするとサーバが別のスケッチを組む。行が
    /// [`FolderClaim`] を持てば、形に合うフォルダ（`src/` の下のファイルならその上の本体のフォルダ。#2113）
    FileDir,
}

/// フォルダの形で受け持つ文書（#2113）。Arduino のスケッチ内の `.h` / `.cpp` のように、拡張子は他の行
/// （C / C++ の行）と同じでも、**本体と同じフォルダにあるときだけ**この行が受け持つ（拡張子だけの行より
/// 先に見る）。
///
/// 形: ファイルのあるフォルダ、または上へ辿った `nested` の名前のフォルダの親に、フォルダと同じ名前の
/// `<フォルダ名>.<main>`（**大文字小文字まで一致**）があること。そのフォルダがルートで、仲間の文書を
/// 開くときは本体を先に開く（[`folder_main`]。サーバが本体から組んだ形でないと仲間を正しく解けない = 実測）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FolderClaim {
    /// 本体の拡張子（先頭の `.` を含まない）。行の `documents` のどれか
    pub main: &'static str,
    /// 本体のフォルダの直下で、その下を何段でも仲間に数えるフォルダの名前
    pub nested: &'static str,
    /// 形に合うときに受け持つ拡張子と `languageId`（他の行と重なってよい）
    pub documents: &'static [DocumentKind],
}

/// 起こす前の前提（#1951）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requirement {
    /// Arduino のスケッチの基板が選ばれている（ルート = スケッチのフォルダの設定を読む。読みと
    /// 案内の文は Code Runner の #1950 と同じ 1 実装 = [`crate::runner_project::sketch_board_selected`]）。
    /// `cli` は選び方のコマンドを打つ伴走（[`Arg::Companion`] の名前）
    SketchBoard { cli: &'static str },
}

impl Requirement {
    /// 満たすコマンドを打つ伴走の名前（[`Arg::Companion`] の 1 つ）
    pub fn companion(&self) -> &'static str {
        match self {
            Self::SketchBoard { cli } => cli,
        }
    }

    /// ルート `root` で満たしていなければ、理由と満たすコマンド。`cli` は案内のコマンドの先頭に
    /// 置く伴走の形（[`super::launch::Companion::command_name`]）。ファイルを 1 つ読むだけ
    pub fn unmet(
        &self,
        root: &Path,
        cli: &str,
        platform: crate::platform::support::Platform,
    ) -> Option<(String, String)> {
        match self {
            Self::SketchBoard { .. } => (!crate::runner_project::sketch_board_selected(root))
                .then(|| crate::runner_project::board_not_selected_note(root, cli, platform)),
        }
    }
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
///
/// 第 2 波の 6 行（#1956。候補の比較と採った順の理由は `.agent/plans/2026-10-language-coverage.md` §8）:
/// LaTeX（texlab）→ TOML（tombi）→ Swift（sourcekit-lsp）→ Dart（dart）→ CSS / SCSS / Less と HTML
/// （vscode-langservers-extracted）。前の 2 つは tako が取れる単体バイナリ、Swift と Dart は言語の道具に同梱
/// （入っていれば言語サーバも在る）、CSS と HTML は利用者の npm で入れる（依存 32 包みを取る口がまだ無い）
///
/// Arduino の 1 行（#1951）: `.ino` を arduino-language-server（Arduino IDE の同梱物）へ。伴走の
/// arduino-cli / clangd と空の設定ファイルを引数に並べ、基板が選ばれるまで起こさない。スケッチ内の
/// `.h` / `.cpp` もフォルダの形（[`FolderClaim`]）でこの行が受け持つ（#2113）
pub const SERVERS: &[ServerSpec] = &[
    ServerSpec {
        id: "rust-analyzer",
        documents: &[doc("rs", "rust")],
        program: "rust-analyzer",
        args: &[],
        root_markers: &["Cargo.toml"],
        root_scope: RootScope::Project,
        folder: None,
        workspace_root: Some(WorkspaceRoot {
            file: "Cargo.toml",
            contains: "[workspace]",
        }),
        bundled: None,
        requires: None,
        // #2035: 初回の build script と `cargo check` を `<target>/rust-analyzer` で走らせ、利用者の
        // `cargo build` と target のロックを取り合わない（cargo 1.95 の表示は `Blocking waiting for file
        // lock on artifact directory`）。費用は build script と proc-macro の成果物が 2 組になること
        // （比べた案と実測は `.agent/plans/2026-09-lsp-s1.md` §29）
        init_options: &[InitOption {
            key: "cargo.targetDir",
            value: "true",
        }],
        install: InstallHint {
            macos: "rustup component add rust-analyzer",
            windows: "rustup component add rust-analyzer",
        },
        // 2026-09-21 の週次リリース（公開から 2 週間以上置いた版）。ハッシュは GitHub Releases の
        // asset の digest。macOS は gzip 1 枚、Windows は zip の中の 1 つ
        fetch: Some(Fetch::Binary {
            version: "2026-09-21",
            assets: &[
                PlatformAsset {
                    os: "macos",
                    arch: "aarch64",
                    asset: Asset {
                        url: "https://github.com/rust-lang/rust-analyzer/releases/download/2026-09-21/rust-analyzer-aarch64-apple-darwin.gz",
                        digest: Digest::Sha256(
                            "eb474adfd12b6e66a6d0a7c25ed51210a64a0237f326f898e32e25164c9b11ad",
                        ),
                        archive: Archive::Gzip,
                        size: 13_874_039,
                    },
                },
                PlatformAsset {
                    os: "macos",
                    arch: "x86_64",
                    asset: Asset {
                        url: "https://github.com/rust-lang/rust-analyzer/releases/download/2026-09-21/rust-analyzer-x86_64-apple-darwin.gz",
                        digest: Digest::Sha256(
                            "1e2f706ee97d9f931ea36ed4e83f839efd0ac09870e18ec8350877d7b98ad8a0",
                        ),
                        archive: Archive::Gzip,
                        size: 14_625_353,
                    },
                },
                PlatformAsset {
                    os: "windows",
                    arch: "x86_64",
                    asset: Asset {
                        url: "https://github.com/rust-lang/rust-analyzer/releases/download/2026-09-21/rust-analyzer-x86_64-pc-windows-msvc.zip",
                        digest: Digest::Sha256(
                            "f62a18aad35568fd9205006a62f97613eac8c4f2ec5fe43c830dde09684e5e6f",
                        ),
                        archive: Archive::ZipMember("rust-analyzer.exe"),
                        size: 17_509_495,
                    },
                },
                PlatformAsset {
                    os: "windows",
                    arch: "aarch64",
                    asset: Asset {
                        url: "https://github.com/rust-lang/rust-analyzer/releases/download/2026-09-21/rust-analyzer-aarch64-pc-windows-msvc.zip",
                        digest: Digest::Sha256(
                            "2cfbd8fc8eb8f8232cc81eb338462abaf78081b4fda7a9fed91a6bf5b2f1ace0",
                        ),
                        archive: Archive::ZipMember("rust-analyzer.exe"),
                        size: 15_642_814,
                    },
                },
            ],
            probe_args: &["--version"],
        }),
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
        root_scope: RootScope::Project,
        folder: None,
        workspace_root: None,
        // macOS は Command Line Tools の clangd（#1956。#1944 の提案: 取らない代わりに Apple 公式の 1 行を案内する。
        // CLT を入れると `/usr/bin/clangd` とコンパイラのヘッダが一緒に入る = LLVM 一式は要らない）
        bundled: None,
        requires: None,
        init_options: &[],
        install: InstallHint {
            macos: "xcode-select --install",
            windows: "winget install LLVM.LLVM",
        },
        // 取らない（#1944）: 配布の zip が macOS で 100 MB（ヘッダ込み）あり、C / C++ の解析には
        // コンパイラのヘッダ（Xcode の Command Line Tools / MSVC / MinGW）が別に要る。
        // macOS はコンパイラを入れれば /usr/bin/clangd も入るので、取っても足りるのは稀
        fetch: None,
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
        args: &[Arg::Lit("--stdio")],
        root_markers: &["tsconfig.json", "jsconfig.json", "package.json"],
        root_scope: RootScope::Project,
        folder: None,
        workspace_root: None,
        bundled: None,
        requires: None,
        init_options: &[],
        install: InstallHint {
            macos: "npm install -g typescript-language-server typescript",
            windows: "npm install -g typescript-language-server typescript",
        },
        // 5.1.3（node >= 20・依存なし）+ typescript 5.9.3（JS 版の最後の系列・依存なし）。
        // 6.x は node >= 22.22 を求め、typescript 7.x は OS ごとのネイティブ包みに分かれるので採らない。
        // プロジェクトの node_modules に typescript があればサーバはそちらを使う
        fetch: Some(Fetch::Npm {
            packages: &[
                NpmPackage {
                    name: "typescript-language-server",
                    version: "5.1.3",
                    integrity: "r+pAcYtWdN8tKlYZPwiiHNA2QPjXnI02NrW5Sf2cVM3TRtuQ3V9EKKwOxqwaQ0krsaEXk/CbN90I5erBuf84Vg==",
                    size: 503_836,
                },
                NpmPackage {
                    name: "typescript",
                    version: "5.9.3",
                    integrity: "jl1vZzPDinLr9eUt3J/t7V6FgNEw9QjvBPdysz9KfQDD41fQrC2Y4vKQdiaUpFT4bXlb1RHhLpp8wtm6M5TgSw==",
                    size: 4_377_468,
                },
            ],
            entry: "node_modules/typescript-language-server/lib/cli.mjs",
            node_major: 20,
        }),
    },
    ServerSpec {
        id: "pyright",
        documents: &[doc("py", "python"), doc("pyi", "python")],
        program: "pyright-langserver",
        args: &[Arg::Lit("--stdio")],
        root_markers: &[
            "pyrightconfig.json",
            "pyproject.toml",
            "setup.py",
            "setup.cfg",
            "requirements.txt",
        ],
        root_scope: RootScope::Project,
        folder: None,
        workspace_root: None,
        bundled: None,
        requires: None,
        init_options: &[],
        install: InstallHint {
            macos: "npm install -g pyright",
            windows: "npm install -g pyright",
        },
        // 1.1.414（2026-09-09 公開。中身は 1 本に束ねてあり、依存の fsevents は省いても動く）
        fetch: Some(Fetch::Npm {
            packages: &[NpmPackage {
                name: "pyright",
                version: "1.1.414",
                integrity: "FPZZb51jepDX4eP7TEYDeNFtmE3WgwkkEcJpvH3/QmUSsj0EAy3LXu+xB4T/FWDejtsXlCfFr/rbWFjwuwuXww==",
                size: 4_226_827,
            }],
            entry: "node_modules/pyright/langserver.index.js",
            node_major: 14,
        }),
    },
    // ─── 第 2 波（#1956）────────────────────────────────────────────
    ServerSpec {
        id: "texlab",
        // `.bib` / `.sty` / `.cls` は `file_type` に行が無いので今は足さない（#1948 の番犬）
        documents: &[doc("tex", "latex")],
        program: "texlab",
        args: &[],
        // texlab 自身がルートの印に使う名前（無ければ `.git` → ファイルの場所）
        root_markers: &[
            ".latexmkrc",
            "latexmkrc",
            ".texlabroot",
            "texlabroot",
            "Tectonic.toml",
        ],
        root_scope: RootScope::Project,
        folder: None,
        workspace_root: None,
        bundled: None,
        requires: None,
        init_options: &[],
        install: InstallHint {
            macos: "brew install texlab",
            // winget に包みが無い（tako が取れないときだけ出る案内）
            windows: "scoop install texlab",
        },
        // v5.26.0（2026-06-25 公開）。ハッシュは GitHub Releases の asset の digest。
        // macOS は tar.gz の中の 1 つ、Windows は zip の中の 1 つ（どちらも 10 MB 前後）
        fetch: Some(Fetch::Binary {
            version: "5.26.0",
            assets: &[
                PlatformAsset {
                    os: "macos",
                    arch: "aarch64",
                    asset: Asset {
                        url: "https://github.com/latex-lsp/texlab/releases/download/v5.26.0/texlab-aarch64-macos.tar.gz",
                        digest: Digest::Sha256(
                            "af7972ffd230711ba04ada9b69cc32ce9111d9196ba69538062872faefdbee56",
                        ),
                        archive: Archive::TarGzMember("texlab"),
                        size: 10_154_621,
                    },
                },
                PlatformAsset {
                    os: "macos",
                    arch: "x86_64",
                    asset: Asset {
                        url: "https://github.com/latex-lsp/texlab/releases/download/v5.26.0/texlab-x86_64-macos.tar.gz",
                        digest: Digest::Sha256(
                            "6091611f756b28e1a57612b130c196df4b0bb6e22dde5cf5d890578513397daf",
                        ),
                        archive: Archive::TarGzMember("texlab"),
                        size: 10_253_264,
                    },
                },
                PlatformAsset {
                    os: "windows",
                    arch: "x86_64",
                    asset: Asset {
                        url: "https://github.com/latex-lsp/texlab/releases/download/v5.26.0/texlab-x86_64-windows.zip",
                        digest: Digest::Sha256(
                            "cb028d44c3d2b85d36a2ed52d41a0ff43a341b1f04c500c56c4524c4eb72b316",
                        ),
                        archive: Archive::ZipMember("texlab.exe"),
                        size: 9_997_967,
                    },
                },
                PlatformAsset {
                    os: "windows",
                    arch: "aarch64",
                    asset: Asset {
                        url: "https://github.com/latex-lsp/texlab/releases/download/v5.26.0/texlab-aarch64-windows.zip",
                        digest: Digest::Sha256(
                            "99b215e9a44169eb8d786c33484b963055e1c3dd40f68e14fcc47ae1c84e92c1",
                        ),
                        archive: Archive::ZipMember("texlab.exe"),
                        size: 9_853_223,
                    },
                },
            ],
            probe_args: &["--version"],
        }),
    },
    ServerSpec {
        id: "tombi",
        documents: &[doc("toml", "toml")],
        program: "tombi",
        args: &[Arg::Lit("lsp")],
        // 設定ファイルだけを印にする（Cargo.toml / pyproject.toml を印にすると workspace の crate ごとに
        // 器が分かれる）。無ければ `.git` → ファイルの場所
        root_markers: &["tombi.toml"],
        root_scope: RootScope::Project,
        folder: None,
        workspace_root: None,
        bundled: None,
        requires: None,
        init_options: &[],
        install: InstallHint {
            macos: "brew install tombi",
            windows: "winget install tombi-toml.tombi",
        },
        // v1.7.0（2026-10-01 公開）。taplo ではなく tombi を採った理由は §8（taplo 0.10.0 は配布元の
        // ハッシュが無く、今の SchemaStore の目録を読めずホバーが出ない）。Cargo.toml / pyproject.toml の
        // スキーマを内蔵していて、オフラインでもホバーが出る（実測）
        fetch: Some(Fetch::Binary {
            version: "1.7.0",
            assets: &[
                PlatformAsset {
                    os: "macos",
                    arch: "aarch64",
                    asset: Asset {
                        url: "https://github.com/tombi-toml/tombi/releases/download/v1.7.0/tombi-cli-1.7.0-aarch64-apple-darwin.tar.gz",
                        digest: Digest::Sha256(
                            "ff18e589f45780ebf77637d88f51d3e075b2500cc0bff99f477a79cf49d5ef95",
                        ),
                        archive: Archive::TarGzMember("tombi-cli-1.7.0-aarch64-apple-darwin/tombi"),
                        size: 7_594_215,
                    },
                },
                PlatformAsset {
                    os: "macos",
                    arch: "x86_64",
                    asset: Asset {
                        url: "https://github.com/tombi-toml/tombi/releases/download/v1.7.0/tombi-cli-1.7.0-x86_64-apple-darwin.tar.gz",
                        digest: Digest::Sha256(
                            "6d0f37f27282fa5f1551256f1d00c58cb0e61c443d89b5813c8be6223b73ce37",
                        ),
                        archive: Archive::TarGzMember("tombi-cli-1.7.0-x86_64-apple-darwin/tombi"),
                        size: 8_153_823,
                    },
                },
                PlatformAsset {
                    os: "windows",
                    arch: "x86_64",
                    asset: Asset {
                        url: "https://github.com/tombi-toml/tombi/releases/download/v1.7.0/tombi-cli-1.7.0-x86_64-pc-windows-msvc.zip",
                        digest: Digest::Sha256(
                            "13822150c15d37a76c989bb7b8f1e6433625b4e72a21dcab10b03d5d46386950",
                        ),
                        archive: Archive::ZipMember("tombi.exe"),
                        size: 10_395_001,
                    },
                },
                PlatformAsset {
                    os: "windows",
                    arch: "aarch64",
                    asset: Asset {
                        url: "https://github.com/tombi-toml/tombi/releases/download/v1.7.0/tombi-cli-1.7.0-aarch64-pc-windows-msvc.zip",
                        digest: Digest::Sha256(
                            "d1883c1d1167f823cc7c91bde2e41df3da24016f5e9c4ba37ca6e96798eba01f",
                        ),
                        archive: Archive::ZipMember("tombi.exe"),
                        size: 9_479_552,
                    },
                },
            ],
            probe_args: &["--version"],
        }),
    },
    ServerSpec {
        id: "sourcekit-lsp",
        documents: &[doc("swift", "swift")],
        program: "sourcekit-lsp",
        args: &[],
        root_markers: &["Package.swift", "buildServer.json"],
        root_scope: RootScope::Project,
        folder: None,
        workspace_root: None,
        bundled: None,
        requires: None,
        init_options: &[],
        install: InstallHint {
            macos: "xcode-select --install",
            windows: "winget install Swift.Toolchain",
        },
        // 取らない: Swift の道具（Xcode / Command Line Tools / Windows の Swift toolchain）に同梱で、
        // 単体では動かない（コンパイラと SDK が要る）。Swift を書ける環境ならもう入っている
        fetch: None,
    },
    ServerSpec {
        id: "dart",
        documents: &[doc("dart", "dart")],
        program: "dart",
        args: &[Arg::Lit("language-server"), Arg::Lit("--protocol=lsp")],
        root_markers: &["pubspec.yaml"],
        root_scope: RootScope::Project,
        folder: None,
        workspace_root: None,
        bundled: None,
        requires: None,
        init_options: &[],
        install: InstallHint {
            macos: "brew install dart-sdk",
            windows: "winget install Google.DartSDK",
        },
        // 取らない: Dart SDK（Flutter も同梱）の `dart language-server` そのもの。SDK ごとで 200 MB を超え、
        // Dart を書ける環境ならもう入っている
        fetch: None,
    },
    ServerSpec {
        id: "vscode-css-language-server",
        documents: &[doc("css", "css"), doc("scss", "scss"), doc("less", "less")],
        program: "vscode-css-language-server",
        args: &[Arg::Lit("--stdio")],
        root_markers: &["package.json"],
        root_scope: RootScope::Project,
        folder: None,
        workspace_root: None,
        bundled: None,
        requires: None,
        init_options: &[],
        install: InstallHint {
            macos: "npm install -g vscode-langservers-extracted",
            windows: "npm install -g vscode-langservers-extracted",
        },
        // 取らない（まだ）: vscode-langservers-extracted 4.10.0 は依存が 32 包み（展開 93 MB。TypeScript 4 系と
        // scoped の `@vscode/l10n` を含む）。[`Fetch::Npm`] は依存を持たない包みと scoped でない名前だけを取れる
        // 形（#1944）なので、取るには取得の口を広げる別の作業が要る。
        // **罠**（実測）: クライアントが `workspace.configuration` を申告して `null` で答えると、検証の途中で
        // 例外になり診断を 1 件も送らない。tako は申告しないので既定の設定で検証する
        fetch: None,
    },
    ServerSpec {
        id: "vscode-html-language-server",
        documents: &[doc("html", "html"), doc("htm", "html")],
        program: "vscode-html-language-server",
        args: &[Arg::Lit("--stdio")],
        root_markers: &["package.json"],
        root_scope: RootScope::Project,
        folder: None,
        workspace_root: None,
        bundled: None,
        requires: None,
        init_options: &[],
        install: InstallHint {
            macos: "npm install -g vscode-langservers-extracted",
            windows: "npm install -g vscode-langservers-extracted",
        },
        // 取らない（まだ）: CSS の行と同じ包み・同じ理由。診断は埋め込みの `<style>` / `<script>` の誤り
        // （HTML の構造そのものは検証しない）、ホバーは要素と属性の説明
        fetch: None,
    },
    // ─── Arduino（#1951。設計 `.agent/plans/2026-10-language-coverage.md` §5.3）──────────
    ServerSpec {
        id: "arduino-language-server",
        // languageId は Arduino IDE と同じ `cpp`。clangd の行には `.ino` を入れない（clangd 単体は `.ino` を
        // 解けず、arduino-cli の compile db を渡すと正しいスケッチに嘘のエラーが 10 件出る = S0 の実測）
        documents: &[doc("ino", "cpp")],
        program: "arduino-language-server",
        // `-cli-config` が無いと `Path to ArduinoCLI config file must be set.` で即終了し、0 バイトの
        // ファイルで足りる（実測）。基板は `-fqbn` で渡さない（正本は sketch.yaml = 2 つにしない）
        args: &[
            Arg::Lit("-cli"),
            Arg::Companion("arduino-cli"),
            Arg::Lit("-clangd"),
            Arg::Companion("clangd"),
            Arg::Lit("-cli-config"),
            Arg::EmptyFile("arduino-cli.yaml"),
        ],
        root_markers: &[],
        root_scope: RootScope::FileDir,
        // #2113: スケッチ内の C / C++（`src/` の下を含む = arduino-cli が一緒に組む範囲）もこの行。拡張子の
        // まま clangd へ渡すと Arduino の型を知らず嘘のエラーが出る（`'Arduino.h' file not found` 等 2 件）。
        // ALS は同じ器で先に `.ino` を開いた順でだけ正しく解く（`.h` だけ先だと疑わしい 1 件 = §5.3 の 6）ので、
        // 利用者が本体を開いていなければ tako が隠れて開く。languageId は C のファイルだけ `c`
        folder: Some(FolderClaim {
            main: "ino",
            nested: "src",
            documents: &[
                doc("h", "cpp"),
                doc("hh", "cpp"),
                doc("hpp", "cpp"),
                doc("hxx", "cpp"),
                doc("cpp", "cpp"),
                doc("cc", "cpp"),
                doc("cxx", "cpp"),
                doc("c", "c"),
            ],
        }),
        workspace_root: None,
        // brew に ALS の formula は無い。現実的な入手経路は Arduino IDE の同梱物（arduino-cli と
        // clangd 14 も同じ場所に並ぶ = 伴走は ALS と同じディレクトリを先に探す）
        bundled: Some(&crate::app_bundle::ARDUINO_IDE),
        // 基板が無いと 15 秒待っても診断が 1 回も来ずホバーも時間切れ = 黙って何も出ない（実測）
        requires: Some(Requirement::SketchBoard { cli: "arduino-cli" }),
        // Code Runner の「arduino-cli が無い」の案内（#1950）の 1 つ目と同じ（言語サーバまで揃う方）
        init_options: &[],
        install: InstallHint {
            macos: "brew install --cask arduino-ide",
            windows: "winget install --id ArduinoSA.IDE.stable",
        },
        // 取らない: 単体の配布が無く、伴走（arduino-cli・clangd）とコア（数百 MB）が別に要る
        fetch: None,
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

/// 表を指定して引く（**表の行を足すと答えが変わる**ことを表以外に触れずに示すための入口）。
///
/// フォルダの形の行（[`FolderClaim`]。#2113）があるので**ファイルシステムを見る**（拡張子がその行に当たる
/// ファイルだけ: 親のフォルダと、上の `nested` の名前のフォルダの親を stat 1 回ずつ。当たったフォルダだけ
/// 中身を並べて大文字小文字を確かめる）。**開いている文書は開いたときに決めた器で引く**こと
/// （`tako_control::lsp::LspManager::resolve`。開いた後で本体を足し引きしても器がぶれない）。
/// `TAKO_2113_LEGACY=1` は拡張子だけで引く（#2113 より前）
pub fn resolve_in<'a>(table: &'a [ServerSpec], path: &Path) -> Option<Resolved<'a>> {
    if folder_legacy() {
        return resolve_with(table, path, &|_, _| false);
    }
    resolve_with(table, path, &file_named)
}

/// 存在の確かめを注入して引く（純粋関数）。`named(dir, name)` は `dir` に名前が `name` と大文字小文字まで
/// 一致するファイルがあるか
pub fn resolve_with<'a>(
    table: &'a [ServerSpec],
    path: &Path,
    named: &dyn Fn(&Path, &str) -> bool,
) -> Option<Resolved<'a>> {
    let extension = path.extension()?.to_str()?;
    let by_folder = table.iter().find_map(|spec| {
        let claim = spec.folder.as_ref()?;
        let kind = kind_of(claim.documents, extension)?;
        folder_root_with(claim, path, named)?;
        Some(Resolved {
            spec,
            language_id: kind.language_id,
        })
    });
    by_folder.or_else(|| {
        table.iter().find_map(|spec| {
            kind_of(spec.documents, extension).map(|kind| Resolved {
                spec,
                language_id: kind.language_id,
            })
        })
    })
}

fn kind_of<'a>(documents: &'a [DocumentKind], extension: &str) -> Option<&'a DocumentKind> {
    documents
        .iter()
        .find(|kind| kind.extension.eq_ignore_ascii_case(extension))
}

/// フォルダの形の根（#2113。純粋関数）: ファイルのあるフォルダが本体を持てばそこ、無ければ上へ辿って
/// `nested` の名前のフォルダの親で本体を持つ最も近いもの。形に合わなければ `None`
pub fn folder_root_with(
    claim: &FolderClaim,
    file: &Path,
    named: &dyn Fn(&Path, &str) -> bool,
) -> Option<PathBuf> {
    let parent = file.parent()?;
    let holds_main = |dir: &Path| {
        dir.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| named(dir, &format!("{name}.{}", claim.main)))
    };
    if holds_main(parent) {
        return Some(parent.to_path_buf());
    }
    parent
        .ancestors()
        .filter(|dir| dir.file_name().is_some_and(|name| name == claim.nested))
        .filter_map(Path::parent)
        .find(|dir| holds_main(dir))
        .map(Path::to_path_buf)
}

/// フォルダの形で受け持った文書（本体でない仲間）なら、本体のパス（#2113。純粋関数）。サーバは本体を
/// 先に開いた順でないと仲間を正しく解けない = manager が利用者の開いていない本体を隠れて開く相手。
/// 本体そのもの・拡張子で受け持った文書・形に合わない文書は `None`
pub fn folder_main_with(
    spec: &ServerSpec,
    path: &Path,
    named: &dyn Fn(&Path, &str) -> bool,
) -> Option<PathBuf> {
    let claim = spec.folder.as_ref()?;
    let extension = path.extension()?.to_str()?;
    kind_of(claim.documents, extension)?;
    let root = folder_root_with(claim, path, named)?;
    let name = root.file_name()?.to_str()?;
    Some(root.join(format!("{name}.{}", claim.main)))
}

/// [`folder_main_with`] の実ファイルシステム版（`TAKO_2113_LEGACY=1` は常に `None`）
pub fn folder_main(spec: &ServerSpec, path: &Path) -> Option<PathBuf> {
    if folder_legacy() {
        return None;
    }
    folder_main_with(spec, path, &file_named)
}

/// `dir` に名前が `name` と大文字小文字まで一致するファイルがあるか。stat が当たったときだけ中身を並べる
/// （大文字小文字を区別しないディスクでは stat だけだと `blink.ino` が `Blink.ino` に当たる。arduino-cli は
/// 名前の違いを「missing」で落とす = Code Runner の #1950 と同じ判定）
pub fn file_named(dir: &Path, name: &str) -> bool {
    dir.join(name).is_file()
        && std::fs::read_dir(dir).is_ok_and(|entries| {
            entries
                .filter_map(Result::ok)
                .any(|entry| entry.file_name() == name)
        })
}

/// フォルダの形を見ない（#2113 より前 = 拡張子だけ）か
pub fn folder_legacy() -> bool {
    std::env::var_os("TAKO_2113_LEGACY").is_some_and(|v| v == "1")
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
    fn arduinoの行までで11行() {
        assert_eq!(
            SERVERS.len(),
            11,
            "表の行数が変わったら番犬とドキュメントも直す"
        );
    }

    /// #2035: 初期化オプションの既定は字句が JSON として読め、キーの形が正しい。
    /// 今あるのは rust-analyzer の `cargo.targetDir = true` だけ（増やしたら設計書 §29 も直す）
    #[test]
    fn 初期化オプションの既定は読める字句で_rust_analyzerだけが持つ() {
        use crate::lsp::init_options::{default_value, validate_key};
        for spec in SERVERS {
            for option in spec.init_options {
                assert!(
                    default_value(option).is_some(),
                    "{} の {} の字句 {:?} が JSON として読めない",
                    spec.id,
                    option.key,
                    option.value
                );
                validate_key(option.key).unwrap();
            }
        }
        let with: Vec<(&str, Vec<&str>)> = SERVERS
            .iter()
            .filter(|s| !s.init_options.is_empty())
            .map(|s| (s.id, s.init_options.iter().map(|o| o.key).collect()))
            .collect();
        assert_eq!(with, vec![("rust-analyzer", vec!["cargo.targetDir"])]);
    }

    /// #1951: `.ino` は arduino-language-server の行。伴走 2 つと空の設定ファイルを引数に並べ、
    /// ルートはファイルのディレクトリ・Arduino IDE の同梱を探す・基板が要る
    #[test]
    fn inoはarduino_language_serverの行が受け持つ() {
        let got = resolve(Path::new("Blink/Blink.ino")).expect(".ino の行");
        assert_eq!(
            (got.spec.id, got.language_id),
            ("arduino-language-server", "cpp")
        );
        let spec = got.spec;
        assert_eq!(
            spec.args,
            &[
                Arg::Lit("-cli"),
                Arg::Companion("arduino-cli"),
                Arg::Lit("-clangd"),
                Arg::Companion("clangd"),
                Arg::Lit("-cli-config"),
                Arg::EmptyFile("arduino-cli.yaml"),
            ]
        );
        assert_eq!(
            spec.companions().collect::<Vec<_>>(),
            ["arduino-cli", "clangd"]
        );
        assert_eq!(spec.literal_args(), ["-cli", "-clangd", "-cli-config"]);
        assert_eq!(spec.root_scope, RootScope::FileDir);
        assert_eq!(spec.bundled, Some(&crate::app_bundle::ARDUINO_IDE));
        assert_eq!(
            spec.requires,
            Some(Requirement::SketchBoard { cli: "arduino-cli" })
        );
        assert!(spec.fetch.is_none(), "単体の配布が無いので取らない");
        // clangd の行は `.ino` を受け持たない（嘘のエラー 10 件 = S0）
        let clangd = find_in(SERVERS, "clangd").unwrap();
        assert!(clangd.documents.iter().all(|d| d.extension != "ino"));
        // #2113: スケッチ内の C / C++ はフォルダの形でこの行（本体は `.ino`・`src/` の下も仲間）
        let claim = spec.folder.expect("フォルダの形の受け持ち");
        assert_eq!((claim.main, claim.nested), ("ino", "src"));
        let clangd_exts: Vec<_> = clangd.documents.iter().map(|d| d.extension).collect();
        let mut claimed: Vec<_> = claim.documents.iter().map(|d| d.extension).collect();
        claimed.sort_unstable();
        let mut expected = clangd_exts.clone();
        expected.sort_unstable();
        assert_eq!(claimed, expected, "C / C++ の行と同じ拡張子を受け持つ");
        assert_eq!(
            override_env_name(spec.id),
            "TAKO_LSP_BIN_ARDUINO_LANGUAGE_SERVER"
        );
    }

    /// 偽のファイルシステム（ある名前の集合。大文字小文字まで一致で答える）
    fn named_in(files: &'static [&'static str]) -> impl Fn(&Path, &str) -> bool {
        move |dir: &Path, name: &str| files.iter().any(|f| Path::new(f) == dir.join(name))
    }

    /// #2113: フォルダの形 = ファイルのあるフォルダか `src/` の上に `<フォルダ名>.ino` があれば ALS の行。
    /// 無ければ今どおり拡張子の行（clangd）。本体そのものは仲間でない（`folder_main` が `None`）
    #[test]
    fn スケッチ内の_c_と_cpp_はフォルダの形で_arduino_の行() {
        const FILES: &[&str] = &[
            "/w/Blink/Blink.ino",
            "/w/Blink/helper.h",
            "/w/Blink/src/util/deep.hpp",
            "/w/Blink/src/net.c",
            "/w/lib/lib.h",
            "/w/blinkc/Blinkc.ino",
            "/w/Blink/other/other.cpp",
        ];
        let named = named_in(FILES);
        let id = |path: &str| {
            resolve_with(SERVERS, Path::new(path), &named).map(|r| (r.spec.id, r.language_id))
        };
        let als = "arduino-language-server";
        assert_eq!(id("/w/Blink/helper.h"), Some((als, "cpp")));
        assert_eq!(
            id("/w/Blink/helper.CPP"),
            Some((als, "cpp")),
            "拡張子の大小"
        );
        assert_eq!(id("/w/Blink/src/util/deep.hpp"), Some((als, "cpp")));
        assert_eq!(id("/w/Blink/src/net.c"), Some((als, "c")));
        assert_eq!(id("/w/Blink/Blink.ino"), Some((als, "cpp")));
        // 形に合わない: スケッチでないフォルダ・大小だけ違う本体・`src/` でない入れ子
        assert_eq!(id("/w/lib/lib.h"), Some(("clangd", "c")));
        assert_eq!(id("/w/blinkc/x.cpp"), Some(("clangd", "cpp")));
        assert_eq!(id("/w/Blink/other/other.cpp"), Some(("clangd", "cpp")));
        // 他の言語は見ない（フォルダの形の行の拡張子でない）
        assert_eq!(
            resolve_with(SERVERS, Path::new("/w/Blink/notes.rs"), &named).map(|r| r.spec.id),
            Some("rust-analyzer")
        );
        let spec = find_in(SERVERS, als).unwrap();
        let main = |path: &str| folder_main_with(spec, Path::new(path), &named);
        assert_eq!(
            main("/w/Blink/helper.h"),
            Some(PathBuf::from("/w/Blink/Blink.ino"))
        );
        assert_eq!(
            main("/w/Blink/src/util/deep.hpp"),
            Some(PathBuf::from("/w/Blink/Blink.ino"))
        );
        assert_eq!(main("/w/Blink/Blink.ino"), None, "本体は仲間でない");
        assert_eq!(main("/w/lib/lib.h"), None);
        // 拡張子だけの行は本体を持たない
        let clangd = find_in(SERVERS, "clangd").unwrap();
        assert_eq!(
            folder_main_with(clangd, Path::new("/w/Blink/helper.h"), &named),
            None
        );
    }

    /// 入れ子のスケッチ: いちばん近い形のフォルダを採る（上の段のスケッチへ上がらない）
    #[test]
    fn フォルダの形は最も近いフォルダを採る() {
        const FILES: &[&str] = &["/w/Outer/Outer.ino", "/w/Outer/src/Inner/Inner.ino"];
        let named = named_in(FILES);
        let claim = find_in(SERVERS, "arduino-language-server")
            .and_then(|s| s.folder)
            .unwrap();
        let root = |path: &str| folder_root_with(&claim, Path::new(path), &named);
        assert_eq!(
            root("/w/Outer/src/Inner/a.h"),
            Some(PathBuf::from("/w/Outer/src/Inner"))
        );
        assert_eq!(
            root("/w/Outer/src/Inner/src/b.h"),
            Some(PathBuf::from("/w/Outer/src/Inner"))
        );
        assert_eq!(root("/w/Outer/src/c.h"), Some(PathBuf::from("/w/Outer")));
        assert_eq!(root("/w/Outer/lib/d.h"), None);
        assert_eq!(root("e.h"), None, "親の無い相対パス");
    }

    /// 実ファイルシステム: 大文字小文字を区別しないディスクでも名前の違う本体は形に合わない。
    /// `TAKO_2113_LEGACY` は env を変えるので子プロセスの e2e（tako-control）で見る
    #[test]
    fn 実ファイルシステムで本体の名前を大文字小文字まで確かめる() {
        let dir = crate::test_residue::ScratchDir::new("tako-2113-named");
        let sketch = dir.path().join("Blink");
        std::fs::create_dir_all(&sketch).unwrap();
        std::fs::write(sketch.join("Blink.ino"), "").unwrap();
        std::fs::write(sketch.join("helper.h"), "").unwrap();
        assert!(file_named(&sketch, "Blink.ino"));
        assert!(!file_named(&sketch, "blink.ino"));
        assert!(!file_named(&sketch, "Missing.ino"));
        assert_eq!(
            resolve(&sketch.join("helper.h")).map(|r| r.spec.id),
            Some("arduino-language-server")
        );
        let other = dir.path().join("blink2");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("Blink2.ino"), "").unwrap();
        assert_eq!(
            resolve(&other.join("x.h")).map(|r| r.spec.id),
            Some("clangd"),
            "フォルダ名と大小だけ違う本体は形に合わない"
        );
    }

    /// 前提・伴走・取得の組み合わせが整っている（前提の `cli` は伴走の名前・取れる行は固定の引数だけ =
    /// 取った版の起動は伴走を探さない・`FileDir` の行は印を持たない）
    #[test]
    fn 行の新しい欄の組み合わせが整っている() {
        for spec in SERVERS {
            if let Some(Requirement::SketchBoard { cli }) = spec.requires {
                assert!(
                    spec.companions().any(|c| c == cli),
                    "{}: 前提の cli {cli} が伴走に無い",
                    spec.id
                );
            }
            if spec.fetch.is_some() {
                assert!(
                    spec.args.iter().all(|a| matches!(a, Arg::Lit(_))),
                    "{}: 取れる行は固定の引数だけ",
                    spec.id
                );
            }
            if spec.root_scope == RootScope::FileDir {
                assert!(spec.root_markers.is_empty() && spec.workspace_root.is_none());
            }
            // #2113: フォルダの形の本体は行自身の拡張子で、ルートは形のフォルダ（FileDir）
            if let Some(claim) = spec.folder {
                assert!(
                    spec.documents.iter().any(|d| d.extension == claim.main),
                    "{}: 本体の拡張子 {} を行が受け持たない",
                    spec.id,
                    claim.main
                );
                assert_eq!(spec.root_scope, RootScope::FileDir, "{}", spec.id);
                assert!(!claim.documents.is_empty() && !claim.nested.is_empty());
            }
            for arg in spec.args {
                if let Arg::Companion(name) | Arg::EmptyFile(name) = arg {
                    assert!(
                        !name.is_empty() && !name.contains(['/', '\\']),
                        "{}: {name}",
                        spec.id
                    );
                }
            }
        }
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

    /// 第 2 波（#1956）: 拡張子 → 行・languageId と、tako が取れる行（この表の答えを e2e と docs が引く）
    #[test]
    fn 第2波の行と取得の有無() {
        let cases = [
            ("report.tex", "texlab", "latex", true),
            ("Cargo.toml", "tombi", "toml", true),
            ("main.swift", "sourcekit-lsp", "swift", false),
            ("lib/main.dart", "dart", "dart", false),
            ("site/style.css", "vscode-css-language-server", "css", false),
            ("app.scss", "vscode-css-language-server", "scss", false),
            ("theme.less", "vscode-css-language-server", "less", false),
            ("index.html", "vscode-html-language-server", "html", false),
            ("old/page.htm", "vscode-html-language-server", "html", false),
        ];
        for (path, id, language_id, fetchable) in cases {
            let got =
                resolve(Path::new(path)).unwrap_or_else(|| panic!("{path} を受け持つ行が無い"));
            assert_eq!((got.spec.id, got.language_id), (id, language_id), "{path}");
            assert_eq!(got.spec.fetch.is_some(), fetchable, "{id} の取得の有無");
            if let Some(fetch) = got.spec.fetch {
                for (os, arch) in [
                    ("macos", "aarch64"),
                    ("macos", "x86_64"),
                    ("windows", "x86_64"),
                    ("windows", "aarch64"),
                ] {
                    assert!(fetch.available_for(os, arch), "{id} {os} {arch}");
                }
            }
        }
        // 見送った形式（§8）は誰も受け持たない = 今の表示（言語サーバなし）のまま
        for path in [
            "Main.java",
            "main.go",
            "config.yaml",
            "ci.yml",
            "data.json",
            "App.vue",
        ] {
            assert!(resolve(Path::new(path)).is_none(), "{path}");
        }
        // clangd は取らない代わりに、macOS では Command Line Tools を案内する（#1944 の提案）
        assert_eq!(
            find_in(SERVERS, "clangd").map(|s| s.install.macos),
            Some("xcode-select --install")
        );
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
        // 足すのは第 2 波で見送った Java（#1956 の §8。JDK 21 が要る）
        const 追加: ServerSpec = ServerSpec {
            id: "jdtls",
            documents: &[doc("java", "java")],
            program: "jdtls",
            args: &[],
            root_markers: &["pom.xml", "build.gradle"],
            root_scope: RootScope::Project,
            folder: None,
            workspace_root: None,
            bundled: None,
            requires: None,
            init_options: &[],
            install: InstallHint {
                macos: "brew install jdtls",
                windows: "scoop install jdtls",
            },
            fetch: None,
        };
        let path = Path::new("src/Main.java");
        assert!(
            resolve_in(SERVERS, path).is_none(),
            "足す前は誰も受け持たない"
        );
        let mut extended = SERVERS.to_vec();
        extended.push(追加);
        let got = resolve_in(&extended, path).expect("足した行が受け持つ");
        assert_eq!(got.spec.id, "jdtls");
        assert_eq!(got.language_id, "java");
        assert_eq!(
            find_in(&extended, "jdtls").map(|s| s.program),
            Some("jdtls")
        );
        assert_eq!(override_env_name(got.spec.id), "TAKO_LSP_BIN_JDTLS");
        assert_eq!(
            override_env_name("vscode-css-language-server"),
            "TAKO_LSP_BIN_VSCODE_CSS_LANGUAGE_SERVER"
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
