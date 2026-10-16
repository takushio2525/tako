//! プロジェクトルートの検出（#1678）
//!
//! 1. ファイルのある場所から上へ辿り、表の `root_markers` のどれかが最初に見つかった所
//! 2. 表に `workspace_root` があれば、そこからさらに上へ辿って**最上位**の workspace を採る
//!    （Rust の crate は `Cargo.toml` を持つが、上に `[workspace]` を持つ `Cargo.toml` が
//!    あればそちらが rust-analyzer の見るルート）
//! 3. 印が無ければ `.git` を持つ所（リポジトリの根）
//! 4. それも無ければファイルの親ディレクトリ
//!
//! 表の `root_scope` が [`RootScope::FileDir`] の行（Arduino のスケッチ = フォルダ単位。#1951）は
//! 1〜3 を見ずにファイルの親ディレクトリ（上の段のスケッチや repo の根をルートにしない）。行が
//! フォルダの形（`servers::FolderClaim`。#2113）を持ち、ファイルが形に合えばその形のフォルダ
//! （`src/` の下の `.h` もスケッチのフォルダ = `.ino` と同じ器）。
//!
//! **`git` は起こさない**。`git::repo_root` は `git rev-parse` を起こすので、
//! 編集モードに入った瞬間の UI スレッドから呼ぶと窓が止まりうる（#1503 の型）。
//! `.git`（ディレクトリでも worktree のファイルでもよい）の有無を見る走査で足りる。
//!
//! `canonicalize` はしない（`.agent/conventions.md`「Issue #970」）。存在確認と読み込みは
//! 引数で受けるので、ファイルシステムに触れずに判定を固定できる。

use std::path::{Path, PathBuf};

use super::servers::{self, RootScope, ServerSpec};

/// 実ファイルシステムでルートを決める
pub fn find_root(spec: &ServerSpec, file: &Path) -> PathBuf {
    let named: &dyn Fn(&Path, &str) -> bool = if servers::folder_legacy() {
        &|_, _| false
    } else {
        &servers::file_named
    };
    find_root_with(
        spec,
        file,
        &|p| p.exists(),
        &|p| std::fs::read_to_string(p).ok(),
        named,
    )
}

/// 存在確認と読み込みを注入してルートを決める（純粋関数）。`named(dir, name)` はフォルダの形の
/// 本体の確かめ（[`servers::file_named`] = 大文字小文字まで一致）
pub fn find_root_with(
    spec: &ServerSpec,
    file: &Path,
    exists: &dyn Fn(&Path) -> bool,
    read: &dyn Fn(&Path) -> Option<String>,
    named: &dyn Fn(&Path, &str) -> bool,
) -> PathBuf {
    let start = file.parent().unwrap_or(file);
    if spec.root_scope == RootScope::FileDir {
        return spec
            .folder
            .as_ref()
            .and_then(|claim| servers::folder_root_with(claim, file, named))
            .unwrap_or_else(|| start.to_path_buf());
    }
    let marked = start.ancestors().find(|dir| {
        spec.root_markers
            .iter()
            .any(|marker| exists(&dir.join(marker)))
    });
    if let Some(first) = marked {
        if let Some(workspace) = spec.workspace_root {
            // 最上位を採る（見つけても止まらずに上まで辿る）
            let outermost = first.ancestors().filter(|dir| {
                read(&dir.join(workspace.file))
                    .is_some_and(|text| text.contains(workspace.contains))
            });
            if let Some(dir) = outermost.last() {
                return dir.to_path_buf();
            }
        }
        return first.to_path_buf();
    }
    if let Some(repo) = start.ancestors().find(|dir| exists(&dir.join(".git"))) {
        return repo.to_path_buf();
    }
    start.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::servers::{InstallHint, WorkspaceRoot};
    use std::collections::HashMap;

    /// 表の値に依存しないテスト用の行（本物の表の行を名指さない）
    const 印あり: ServerSpec = ServerSpec {
        id: "fake",
        documents: &[],
        program: "fake",
        args: &[],
        root_markers: &["proj.toml"],
        root_scope: RootScope::Project,
        folder: None,
        workspace_root: Some(WorkspaceRoot {
            file: "proj.toml",
            contains: "[workspace]",
        }),
        bundled: None,
        requires: None,
        init_options: &[],
        install: InstallHint {
            macos: "",
            windows: "",
        },
        fetch: None,
    };

    const 入れ子なし: ServerSpec = ServerSpec {
        workspace_root: None,
        ..印あり
    };

    fn fs(files: &[(&str, &str)]) -> HashMap<PathBuf, String> {
        files
            .iter()
            .map(|(p, t)| (PathBuf::from(p), (*t).to_string()))
            .collect()
    }

    fn root(spec: &ServerSpec, files: &HashMap<PathBuf, String>, file: &str) -> PathBuf {
        find_root_with(
            spec,
            Path::new(file),
            &|p| files.contains_key(p),
            &|p| files.get(p).cloned(),
            &|dir, name| files.contains_key(&dir.join(name)),
        )
    }

    #[test]
    fn 最初に見つかった印の場所がルート() {
        let files = fs(&[("/w/app/proj.toml", "")]);
        assert_eq!(
            root(&入れ子なし, &files, "/w/app/src/main.x"),
            PathBuf::from("/w/app")
        );
    }

    #[test]
    fn workspace_を持つ最上位を採る() {
        let files = fs(&[
            ("/w/proj.toml", "[workspace]\nmembers = []\n"),
            ("/w/crates/a/proj.toml", "[package]\n"),
        ]);
        assert_eq!(
            root(&印あり, &files, "/w/crates/a/src/lib.x"),
            PathBuf::from("/w")
        );
        // 入れ子の判定を持たない行は最初の印で止まる
        assert_eq!(
            root(&入れ子なし, &files, "/w/crates/a/src/lib.x"),
            PathBuf::from("/w/crates/a")
        );
    }

    #[test]
    fn workspace_が無ければ最初の印() {
        let files = fs(&[("/w/crates/a/proj.toml", "[package]\n")]);
        assert_eq!(
            root(&印あり, &files, "/w/crates/a/src/lib.x"),
            PathBuf::from("/w/crates/a")
        );
    }

    #[test]
    fn 印が無ければ_git_の根() {
        let files = fs(&[("/repo/.git", "gitdir: …")]);
        assert_eq!(
            root(&入れ子なし, &files, "/repo/docs/x/a.x"),
            PathBuf::from("/repo")
        );
    }

    /// #1951: スケッチはフォルダ単位。印・`.git`・上の段の印があってもファイルのディレクトリ
    #[test]
    fn ファイルのディレクトリの行は上へ辿らない() {
        const フォルダ単位: ServerSpec = ServerSpec {
            root_markers: &[],
            root_scope: RootScope::FileDir,
            folder: None,
            workspace_root: None,
            ..印あり
        };
        let files = fs(&[
            ("/repo/.git", "gitdir: …"),
            ("/repo/sketches/proj.toml", "[workspace]\n"),
        ]);
        assert_eq!(
            root(&フォルダ単位, &files, "/repo/sketches/Blink/Blink.x"),
            PathBuf::from("/repo/sketches/Blink")
        );
        // 同じファイルシステムで、今までの行は上の段の印を採る（違いはこの欄だけ）
        assert_eq!(
            root(&入れ子なし, &files, "/repo/sketches/Blink/Blink.x"),
            PathBuf::from("/repo/sketches")
        );
    }

    /// #2113: フォルダの形を持つ行は、`src/` の下のファイルも本体のフォルダ（`.ino` と同じ器）。
    /// 形に合わなければ今どおりファイルのディレクトリ
    #[test]
    fn フォルダの形の行は_src_の下も本体のフォルダ() {
        use crate::lsp::servers::{DocumentKind, FolderClaim};
        const 形あり: ServerSpec = ServerSpec {
            root_markers: &[],
            root_scope: RootScope::FileDir,
            folder: Some(FolderClaim {
                main: "m",
                nested: "src",
                documents: &[DocumentKind {
                    extension: "h",
                    language_id: "h",
                }],
            }),
            workspace_root: None,
            ..印あり
        };
        let files = fs(&[("/repo/.git", ""), ("/repo/Blink/Blink.m", "")]);
        for (file, want) in [
            ("/repo/Blink/a.h", "/repo/Blink"),
            ("/repo/Blink/src/a.h", "/repo/Blink"),
            ("/repo/Blink/src/x/y/a.h", "/repo/Blink"),
            ("/repo/Blink/Blink.m", "/repo/Blink"),
            ("/repo/Blink/lib/a.h", "/repo/Blink/lib"),
            ("/repo/other/a.h", "/repo/other"),
        ] {
            assert_eq!(root(&形あり, &files, file), PathBuf::from(want), "{file}");
        }
    }

    #[test]
    fn 何も無ければ親ディレクトリ() {
        let files = fs(&[]);
        assert_eq!(
            root(&入れ子なし, &files, "/tmp/scratch/a.x"),
            PathBuf::from("/tmp/scratch")
        );
    }
}
