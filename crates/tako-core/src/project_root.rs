//! プロジェクトルートの探索 — 「どこまで上へ辿ってよいか」の 1 実装（Issue #1656）
//!
//! Code Runner のプロジェクト既定（[`crate::runner_project`]）と、LSP のルート検出
//! （#1678 の `lsp/root.rs`。`.agent/plans/2026-09-lsp-s1.md` §5）が**同じ境界**で辿るための
//! 共有部品。**何を印にするか**（`Cargo.toml` / `package.json` …）は呼び出し側の表が持ち、
//! ここが決めるのは辿る範囲だけ。
//!
//! ## 境界（無制限に辿らない）
//!
//! 1. **天井（既定は HOME）は見ない・越えない**。`~/package.json` や `~/Makefile` の
//!    置き忘れは珍しくなく（ホームで `npm i` を打つと出来る）、これをプロジェクトと読むと
//!    ホーム直下の雑多なファイルが全部 `npm run …` で走る
//! 2. **ファイルシステムのルートは見ない**
//! 3. **`.git` がある段は見て、そこで止める**（リポジトリの外へ出ない）。
//!    submodule / worktree の `.git` はファイルなので `exists` で見る
//! 4. **深さ上限** [`MAX_DEPTH`] 段
//!
//! ## git プロセスを起こさない
//!
//! [`crate::git::repo_root`] は `git rev-parse` を起こす。Code Runner の検出は
//! プレビューを開くたびに UI スレッドで走るので、段ごとの `exists` だけで決める。
//!
//! ## 「このファイルのプロジェクトのルートはどこか」は [`detect`] の 1 本
//!
//! Code Runner の実行設定（#1726。`.agent/plans/2026-09-runner-settings.md` §4.5）は
//! プロジェクトのルートを設定のキーにする。ルートが Code Runner の cwd とずれると
//! 「project に保存した設定が効かない」になるので、[`detect`] は**種別の表
//! （[`crate::runner_project::KINDS`]）へ委ねて、実行の cwd と同じ答えを返す**。
//! cargo のワークスペースのように「印のある段」と「ルート」が違う種別があるので、
//! ルートの決め方をここへ複製しない。

use std::path::{Path, PathBuf};

use crate::platform::support::Platform;

/// 上へ辿る段数の上限（開始ディレクトリを 1 段目と数える）
pub const MAX_DEPTH: usize = 32;

/// 辿る範囲の境界
#[derive(Debug, Clone)]
pub struct SearchBounds {
    /// 見ない・越えないディレクトリ（本番は HOME。テストは一時 dir の外側）
    ceilings: Vec<PathBuf>,
    max_depth: usize,
}

impl SearchBounds {
    /// 本番の境界: HOME を天井にする。
    ///
    /// HOME は env の値と実体パスの両方を天井に積む（呼び出し側は実体パスで辿るので、
    /// HOME がシンボリックリンク越しに設定されていても天井が効く）
    pub fn for_user() -> Self {
        let mut ceilings = Vec::new();
        if let Some(home) = crate::paths::home_dir() {
            if let Ok(canonical) = crate::platform::path::canonicalize(&home) {
                if canonical != home {
                    ceilings.push(canonical);
                }
            }
            ceilings.push(home);
        }
        Self {
            ceilings,
            max_depth: MAX_DEPTH,
        }
    }

    /// 天井を明示する（テストはここで一時 dir の外側を天井にし、実 HOME を読まない）
    pub fn with_ceilings(ceilings: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            ceilings: ceilings.into_iter().collect(),
            max_depth: MAX_DEPTH,
        }
    }

    /// 深さ上限を差し替える（上限そのものを検査するテスト用）
    pub fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = max_depth;
        self
    }

    fn is_ceiling(&self, dir: &Path) -> bool {
        self.ceilings.iter().any(|c| same_dir(c, dir))
    }
}

/// パスの同一判定。Windows のファイルシステムは大文字小文字を区別しないので、
/// `%USERPROFILE%` と実体パスで綴りの大小が揃わなくても天井を効かせる
fn same_dir(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
    } else {
        a == b
    }
}

/// `start_dir` から上へ、境界の内側で**見てよい**ディレクトリを近い順に返す
pub fn candidate_dirs(start_dir: &Path, bounds: &SearchBounds) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut cur = Some(start_dir);
    while let Some(dir) = cur {
        if out.len() >= bounds.max_depth {
            break;
        }
        // 親が無い = ファイルシステムのルート（`/` / `C:\`）か空パス。どちらも見ない
        let Some(parent) = dir.parent() else {
            break;
        };
        if bounds.is_ceiling(dir) {
            break;
        }
        out.push(dir.to_path_buf());
        if dir.join(".git").exists() {
            break;
        }
        cur = Some(parent);
    }
    out
}

/// 見つかった印
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// 印のあったディレクトリ
    pub dir: PathBuf,
    /// 実際に見つかったファイル名（`*.csproj` なら `app.csproj`）
    pub marker: String,
}

/// 境界の内側で、`markers` のどれかを持つ最も近いディレクトリを探す
pub fn find_upward(start_dir: &Path, markers: &[&str], bounds: &SearchBounds) -> Option<Found> {
    find_upward_map(start_dir, bounds, |dir| {
        marker_in(dir, markers).map(|marker| Found {
            dir: dir.to_path_buf(),
            marker,
        })
    })
}

/// 境界の内側で、近い段から `probe` を当てて最初に `Some` を返したものを返す。
///
/// 印がファイル名で表せない探し方（`.venv` の**ディレクトリ**がある段・中身を読んで
/// 決める段）はこちらを使う。辿る範囲は [`candidate_dirs`] と同じ
pub fn find_upward_map<T>(
    start_dir: &Path,
    bounds: &SearchBounds,
    mut probe: impl FnMut(&Path) -> Option<T>,
) -> Option<T> {
    candidate_dirs(start_dir, bounds)
        .into_iter()
        .find_map(|dir| probe(&dir))
}

/// `dir` 直下にある印のうち、`markers` の並びで最初のもの。
///
/// 印の書式: ファイル名そのもの（`Cargo.toml`）か、`*` で始まる接尾辞（`*.csproj`）。
/// 接尾辞は名前順で最初の 1 つを返す（`read_dir` の順は OS 依存なので並べ替える）
pub fn marker_in(dir: &Path, markers: &[&str]) -> Option<String> {
    for marker in markers {
        if let Some(suffix) = marker.strip_prefix('*') {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            let mut hits: Vec<String> = entries
                .filter_map(Result::ok)
                .filter(|e| e.path().is_file())
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|name| name.len() > suffix.len() && name.ends_with(suffix))
                .collect();
            hits.sort();
            if let Some(first) = hits.into_iter().next() {
                return Some(first);
            }
        } else if dir.join(marker).is_file() {
            return Some((*marker).to_string());
        }
    }
    None
}

/// 境界の内側にある git リポジトリのルート（`.git` を持つ段）。git プロセスは起こさない
pub fn repo_root_within(start_dir: &Path, bounds: &SearchBounds) -> Option<PathBuf> {
    candidate_dirs(start_dir, bounds)
        .into_iter()
        .last()
        .filter(|dir| dir.join(".git").exists())
}

/// ファイルの属するプロジェクト
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInfo {
    /// プロジェクトのルート（= Code Runner のプロジェクト既定の cwd = `${workspaceRoot}`）
    pub root: PathBuf,
    /// 種別（[`crate::runner_project::ProjectKind::id`]。`"cargo"` 等）
    pub kind: &'static str,
}

/// ファイルの属するプロジェクトのルートと種別（本番の境界 = HOME を天井にする）。
///
/// **Code Runner の実行の cwd と同じ答え**を返す（種別の表へ委ねる）。どの種別も
/// 受け持たない拡張子・印の見つからないファイルは `None`
pub fn detect(file: &Path) -> Option<ProjectInfo> {
    detect_in(file, &SearchBounds::for_user())
}

/// [`detect`] の境界を外から渡す版（テスト用）
pub fn detect_in(file: &Path, bounds: &SearchBounds) -> Option<ProjectInfo> {
    // ルートは OS にもファイルの中身にも依らない（中身を見るのはコマンドの選び方だけ =
    // Go の `package` 句）ので、実行中の OS と空の先頭で表を引く
    crate::runner_project::detect(Platform::current(), file, "", bounds).map(|m| ProjectInfo {
        root: m.root,
        kind: m.kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一時 dir を 1 つ作り、その**外側**を天井にした境界と組で返す（実 HOME を読まない）
    fn sandbox(tag: &str) -> (PathBuf, SearchBounds) {
        let base = std::env::temp_dir().join(format!(
            "tako-1656-root-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let base = crate::platform::path::canonicalize(&base).unwrap();
        let bounds = SearchBounds::with_ceilings([base.clone()]);
        (base, bounds)
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }

    #[test]
    fn 天井は見ないし越えない() {
        let (base, bounds) = sandbox("ceiling");
        // 天井（= HOME 相当）の直下に置き忘れの印がある
        touch(&base.join("package.json"));
        let deep = base.join("scratch/sub");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(find_upward(&deep, &["package.json"], &bounds), None);
        let dirs = candidate_dirs(&deep, &bounds);
        assert_eq!(dirs, vec![deep.clone(), base.join("scratch")]);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn gitのある段は見てそこで止まる() {
        let (base, bounds) = sandbox("git");
        // リポジトリの外（上）にある印は拾わない
        touch(&base.join("outer/Cargo.toml"));
        std::fs::create_dir_all(base.join("outer/repo/.git")).unwrap();
        let src = base.join("outer/repo/src");
        std::fs::create_dir_all(&src).unwrap();
        assert_eq!(find_upward(&src, &["Cargo.toml"], &bounds), None);
        // `.git` の段そのものは見る
        touch(&base.join("outer/repo/Cargo.toml"));
        let found = find_upward(&src, &["Cargo.toml"], &bounds).expect("repo の段で見つかる");
        assert_eq!(found.dir, base.join("outer/repo"));
        assert_eq!(
            repo_root_within(&src, &bounds),
            Some(base.join("outer/repo"))
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn worktreeの_gitファイルでも止まる() {
        let (base, bounds) = sandbox("gitfile");
        touch(&base.join("Cargo.toml"));
        std::fs::create_dir_all(base.join("wt")).unwrap();
        std::fs::write(base.join("wt/.git"), "gitdir: /elsewhere\n").unwrap();
        assert_eq!(
            find_upward(&base.join("wt"), &["Cargo.toml"], &bounds),
            None
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn 深さ上限で止まる() {
        let (base, bounds) = sandbox("depth");
        touch(&base.join("d0/go.mod"));
        let mut deep = base.clone();
        for i in 0..5 {
            deep = deep.join(format!("d{i}"));
        }
        std::fs::create_dir_all(&deep).unwrap();
        // d4 から d0 までは開始段を含めて 5 段。上限 4 なら届かない
        assert_eq!(
            find_upward(&deep, &["go.mod"], &bounds.clone().with_max_depth(4)),
            None
        );
        let found = find_upward(&deep, &["go.mod"], &bounds.with_max_depth(5));
        assert_eq!(found.map(|f| f.dir), Some(base.join("d0")));
        assert_eq!(MAX_DEPTH, 32, "本番の上限");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn 最も近い印が勝つ() {
        let (base, bounds) = sandbox("nearest");
        touch(&base.join("p/package.json"));
        touch(&base.join("p/packages/a/package.json"));
        let src = base.join("p/packages/a/src");
        std::fs::create_dir_all(&src).unwrap();
        let found = find_upward(&src, &["package.json"], &bounds).unwrap();
        assert_eq!(found.dir, base.join("p/packages/a"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn 接尾辞の印は名前順で最初のもの() {
        let (base, bounds) = sandbox("suffix");
        touch(&base.join("app/Zeta.csproj"));
        touch(&base.join("app/Alpha.csproj"));
        // 名前が接尾辞そのもの（`.csproj`）はプロジェクトファイルではない
        touch(&base.join("other/.csproj"));
        let found = find_upward(&base.join("app"), &["*.csproj"], &bounds).unwrap();
        assert_eq!(found.marker, "Alpha.csproj");
        assert_eq!(
            find_upward(&base.join("other"), &["*.csproj"], &bounds),
            None
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn 印はファイルだけでディレクトリは数えない() {
        let (base, bounds) = sandbox("dirmarker");
        std::fs::create_dir_all(base.join("x/Cargo.toml")).unwrap();
        assert_eq!(find_upward(&base.join("x"), &["Cargo.toml"], &bounds), None);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// シンボリックリンク: 辿るのは**渡されたパスの字面**。実体へ寄せるのは呼び出し側
    /// （dispatch は `platform::path::canonicalize` を通してから渡す）。リンク切れの印は数えない
    #[cfg(unix)]
    #[test]
    fn シンボリックリンク越しの印() {
        let (base, bounds) = sandbox("symlink");
        touch(&base.join("real/Cargo.toml"));
        std::fs::create_dir_all(base.join("real/src")).unwrap();
        std::os::unix::fs::symlink(base.join("real"), base.join("link")).unwrap();
        let via_link = find_upward(&base.join("link/src"), &["Cargo.toml"], &bounds).unwrap();
        assert_eq!(via_link.dir, base.join("link"));
        let canonical = crate::platform::path::canonicalize(&base.join("link/src")).unwrap();
        let via_real = find_upward(&canonical, &["Cargo.toml"], &bounds).unwrap();
        assert_eq!(via_real.dir, base.join("real"));
        // リンクの印は実体があれば数え、リンク切れは数えない
        std::fs::create_dir_all(base.join("other")).unwrap();
        std::os::unix::fs::symlink(base.join("real/Cargo.toml"), base.join("other/Cargo.toml"))
            .unwrap();
        assert!(find_upward(&base.join("other"), &["Cargo.toml"], &bounds).is_some());
        std::fs::create_dir_all(base.join("dangling")).unwrap();
        std::os::unix::fs::symlink(base.join("nowhere"), base.join("dangling/Cargo.toml")).unwrap();
        assert_eq!(
            find_upward(&base.join("dangling"), &["Cargo.toml"], &bounds),
            None
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 天井の判定は段そのものとの一致（配下は天井ではない）。Windows は大小を区別しない
    /// （`%USERPROFILE%` と実体パスで綴りの大小が揃わないことがある）
    #[test]
    fn 天井の判定() {
        let bounds = SearchBounds::with_ceilings([PathBuf::from("/srv/box")]);
        assert!(bounds.is_ceiling(Path::new("/srv/box")));
        assert!(!bounds.is_ceiling(Path::new("/srv/box/proj")));
        assert!(!bounds.is_ceiling(Path::new("/srv")));
        assert_eq!(
            bounds.is_ceiling(Path::new("/srv/BOX")),
            cfg!(windows),
            "大小の扱いが OS の流儀と違う"
        );
        assert_eq!(bounds.max_depth, MAX_DEPTH);
    }

    #[test]
    fn ファイルシステムのルートは見ない() {
        let root = if cfg!(windows) {
            PathBuf::from(r"C:\")
        } else {
            PathBuf::from("/")
        };
        let bounds = SearchBounds::with_ceilings([]);
        assert!(candidate_dirs(&root, &bounds).is_empty());
    }
}
