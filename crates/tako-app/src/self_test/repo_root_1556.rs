//! #1556: セルフテストが読み書きするソースツリーの根と、項目 14「tako CLI のビルド」の
//! `cargo build -p tako-cli` を回す dir を決める 1 実装。
//!
//! 以前の項目 14 は cargo を**カレントディレクトリのまま**起こしていたので、リポジトリの外
//! （scratchpad 等）から `TAKO_SELF_TEST=1` で起動すると `error: could not find Cargo.toml in <cwd>`
//! → `TAKO_APP_SELF_TEST_FAILED: tako CLI のビルド` で終わり、「リポジトリの根から起動する」という
//! 前提がどこにも出なかった。いまは根を実行ファイルの位置から辿り（`scripts/lib/isolated-gui.sh` の
//! `iso_repo_root` と同じ考え方）、そこで cargo を回す。辿れなければビルドせずに、表示言語の案内
//! （cwd と実行ファイルの場所つき）で止まる。
//!
//! 根を探す順（どれも `crates/tako-app/Cargo.toml` のある祖先を探す）:
//! 1. 実行中のバイナリ（`target/…/tako-app`）
//! 2. 実行時の環境変数 `CARGO_MANIFEST_DIR`（`cargo run` が子へ渡す。`CARGO_TARGET_DIR` を
//!    リポジトリの外へ向けたビルドを `cargo run --manifest-path …` で起こした形でも根に届く）。
//!    **ビルド時の `env!("CARGO_MANIFEST_DIR")` は使わない**（ビルド機の絶対パスが release
//!    バイナリへ残る = #1848）
//! 3. カレントディレクトリ（修正前と同じ）
//!
//! A/B は `TAKO_1556_LEGACY=1`（項目 14 は修正前と同じくカレントディレクトリで cargo を回し、
//! 根の候補からも 2 を外す）、根が見つからない形の注入は `TAKO_1556_INJECT=noroot`
//! （候補を 1 つも当てない = 配布された .app から起動した形）。

use std::path::{Path, PathBuf};
use tako_core::i18n::Lang;

/// 根の目印（この祖先がリポジトリの根）
const ROOT_MARKER: &str = "crates/tako-app/Cargo.toml";

/// `TAKO_1556_LEGACY=1`: 修正前の挙動（同一バイナリの A/B）
fn legacy_1556() -> bool {
    std::env::var("TAKO_1556_LEGACY").as_deref() == Ok("1")
}

/// `TAKO_1556_INJECT=noroot`: 根の候補を 1 つも当てない（見つからない経路の検証専用）
fn inject_noroot_1556() -> bool {
    std::env::var("TAKO_1556_INJECT").as_deref() == Ok("noroot")
}

/// 根をどの候補から辿ったか（診断の 1 行に出す）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RootFrom {
    /// 実行中のバイナリの位置
    Exe,
    /// 実行時の `CARGO_MANIFEST_DIR`（`cargo run` が渡す）
    ManifestEnv,
    /// カレントディレクトリ
    Cwd,
}

impl RootFrom {
    /// 診断の行に出す名前（ログの検索語なので言語に依らない）
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Exe => "exe",
            Self::ManifestEnv => "CARGO_MANIFEST_DIR",
            Self::Cwd => "cwd",
        }
    }
}

/// 根を探す候補（探す順）。`legacy` は修正前の 2 つ（実行ファイル → cwd）に戻す
#[derive(Debug, Default, Clone)]
pub(super) struct RootCandidates {
    pub exe: Option<PathBuf>,
    pub manifest_env: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
}

impl RootCandidates {
    /// 今のプロセスから候補を集める
    pub(super) fn current() -> Self {
        let manifest_env = if legacy_1556() {
            None
        } else {
            std::env::var_os("CARGO_MANIFEST_DIR")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Self {
            exe: std::env::current_exe().ok(),
            manifest_env,
            cwd: std::env::current_dir().ok(),
        }
    }

    /// 候補から根を探す（純粋関数。先に当たった候補が勝つ）
    pub(super) fn find_root(&self) -> Option<(PathBuf, RootFrom)> {
        [
            (self.exe.as_deref(), RootFrom::Exe),
            (self.manifest_env.as_deref(), RootFrom::ManifestEnv),
            (self.cwd.as_deref(), RootFrom::Cwd),
        ]
        .into_iter()
        .find_map(|(start, from)| root_above(start?).map(|root| (root, from)))
    }
}

/// `start` から上へ辿り、目印のある dir を返す
fn root_above(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|d| d.join(ROOT_MARKER).is_file())
        .map(Path::to_path_buf)
}

/// セルフテストが読み書きするソースツリーの根（#1848 / #1556）。リポジトリの外（配布物を
/// そのまま走らせた・注入）なら `None`
pub(super) fn source_tree_root() -> Option<PathBuf> {
    if inject_noroot_1556() {
        return None;
    }
    RootCandidates::current().find_root().map(|(root, _)| root)
}

/// 項目 14 が `cargo build -p tako-cli` を回す dir（#1556）
#[derive(Debug, PartialEq)]
pub(super) enum CliBuildDir {
    /// リポジトリの根で回す
    Root { root: PathBuf, from: RootFrom },
    /// `TAKO_1556_LEGACY=1`: 修正前と同じくカレントディレクトリのまま回す
    LegacyCwd,
    /// 根が見つからない = ビルドせずに案内で止まる
    Missing(RepoRootMissing),
}

/// 今のプロセスで項目 14 の dir を決める
pub(super) fn cli_build_dir() -> CliBuildDir {
    if legacy_1556() {
        return CliBuildDir::LegacyCwd;
    }
    let candidates = RootCandidates::current();
    let found = if inject_noroot_1556() {
        None
    } else {
        candidates.find_root()
    };
    decide(candidates, found)
}

/// 見つかった根（無ければ `None`）から項目 14 の dir を決める（純粋関数）
pub(super) fn decide(
    candidates: RootCandidates,
    found: Option<(PathBuf, RootFrom)>,
) -> CliBuildDir {
    match found {
        Some((root, from)) => CliBuildDir::Root { root, from },
        None => CliBuildDir::Missing(RepoRootMissing {
            cwd: shown(candidates.cwd.as_deref()),
            exe: shown(candidates.exe.as_deref()),
        }),
    }
}

/// 案内に出すパス（取れなければ `?`）
fn shown(path: Option<&Path>) -> String {
    path.map(|p| p.display().to_string())
        .unwrap_or_else(|| "?".into())
}

/// 根が見つからないときの案内（#1556）。日英の対はここ 1 か所で、`Display` は組んだその場の
/// 表示言語（`tako_core::i18n::lang()`）で引く
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RepoRootMissing {
    /// 起動したときのカレントディレクトリ
    pub cwd: String,
    /// 実行中のバイナリ
    pub exe: String,
}

impl RepoRootMissing {
    /// 表示言語を明示しての文言
    pub(super) fn text_in(&self, lang: Lang) -> String {
        let Self { cwd, exe } = self;
        match lang {
            Lang::Ja => format!(
                "14: tako CLI のビルド: リポジトリのルート（crates/tako-app/Cargo.toml のある dir）が\
                 見つからないのでビルドしない。リポジトリの中でビルドした tako-app（target/ の下）を起こすか、\
                 リポジトリの中から起動してください（cwd={cwd}・実行ファイル={exe}。#1556）"
            ),
            Lang::En => format!(
                "14: building the tako CLI: the repository root (the dir that holds \
                 crates/tako-app/Cargo.toml) was not found, so nothing is built. Run the tako-app built \
                 inside the repository (under target/) or start it from inside the repository \
                 (cwd={cwd}, executable={exe}. #1556)"
            ),
        }
    }
}

impl std::fmt::Display for RepoRootMissing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(tako_core::i18n::lang()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tako_core::test_residue::ScratchDir;

    /// 目印（`crates/tako-app/Cargo.toml`）を置いた偽のリポジトリと、その外の dir を作る
    fn fake_tree() -> (ScratchDir, PathBuf, PathBuf) {
        let base = ScratchDir::new("st1556");
        let repo = base.join("repo");
        std::fs::create_dir_all(repo.join("crates/tako-app")).unwrap();
        std::fs::write(repo.join(ROOT_MARKER), "[package]\n").unwrap();
        std::fs::create_dir_all(repo.join("target/debug")).unwrap();
        let outside = base.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        (base, repo, outside)
    }

    #[test]
    fn リポジトリの外のcwdでも実行ファイルの位置から根に届く() {
        let (_base, repo, outside) = fake_tree();
        let c = RootCandidates {
            exe: Some(repo.join("target/debug/tako-app")),
            manifest_env: None,
            cwd: Some(outside),
        };
        assert_eq!(c.find_root(), Some((repo, RootFrom::Exe)));
    }

    #[test]
    fn 実行ファイルが外ならcargo_runのcargo_manifest_dirから根に届く() {
        let (_base, repo, outside) = fake_tree();
        let c = RootCandidates {
            exe: Some(outside.join("elsewhere-target/debug/tako-app")),
            manifest_env: Some(repo.join("crates/tako-app")),
            cwd: Some(outside),
        };
        assert_eq!(c.find_root(), Some((repo, RootFrom::ManifestEnv)));
    }

    #[test]
    fn リポジトリの根から起動した形は修正前と同じ根へ届く() {
        let (_base, repo, _outside) = fake_tree();
        let c = RootCandidates {
            exe: Some(repo.join("target/debug/tako-app")),
            manifest_env: None,
            cwd: Some(repo.clone()),
        };
        let root = c.find_root().map(|(r, _)| r);
        assert_eq!(root.as_deref(), Some(repo.as_path()));
        // 実行ファイルが外でも cwd が根なら cwd で当たる（修正前の唯一の経路）
        let c = RootCandidates {
            exe: None,
            manifest_env: None,
            cwd: Some(repo.join("crates")),
        };
        assert_eq!(c.find_root(), Some((repo, RootFrom::Cwd)));
    }

    #[test]
    fn どの候補も外なら案内でcwdと実行ファイルを名指す() {
        let (_base, _repo, outside) = fake_tree();
        let exe = outside.join("tako.app/Contents/MacOS/tako-app");
        let c = RootCandidates {
            exe: Some(exe.clone()),
            manifest_env: Some(outside.join("not-a-crate")),
            cwd: Some(outside.clone()),
        };
        let found = c.find_root();
        assert_eq!(found, None);
        let CliBuildDir::Missing(why) = decide(c, found) else {
            panic!("根が無いのに Missing にならない");
        };
        assert_eq!(why.cwd, outside.display().to_string());
        assert_eq!(why.exe, exe.display().to_string());
        for lang in [Lang::Ja, Lang::En] {
            let text = why.text_in(lang);
            assert!(text.starts_with("14: "), "{lang:?}: {text}");
            assert!(
                text.contains(&format!("cwd={}", outside.display())),
                "{text}"
            );
            assert!(text.contains(&exe.display().to_string()), "{text}");
            assert!(text.contains("#1556"), "{text}");
        }
    }

    #[test]
    fn 英語の案内に日本語が混ざらず日本語の案内は日本語で書く() {
        let why = RepoRootMissing {
            cwd: "/tmp/x".into(),
            exe: "?".into(),
        };
        let en = why.text_in(Lang::En);
        assert!(
            !en.chars().any(|c| matches!(c, '\u{3000}'..='\u{30ff}' | '\u{4e00}'..='\u{9fff}' | '\u{ff00}'..='\u{ffef}')),
            "英語の案内に日本語が混ざっている: {en}"
        );
        assert!(why
            .text_in(Lang::Ja)
            .contains("リポジトリの中から起動してください"));
    }

    #[test]
    fn 候補が取れないときも案内は組める() {
        let c = RootCandidates::default();
        let found = c.find_root();
        let CliBuildDir::Missing(why) = decide(c, found) else {
            panic!("候補が空なのに Missing にならない");
        };
        assert_eq!((why.cwd.as_str(), why.exe.as_str()), ("?", "?"));
    }
}
