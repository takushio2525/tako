//! ファイルツリーのコピー / 切り取り / 貼り付けのクリップボード（FR-3.33 / Issue #1860）
//!
//! **何を・どのモード（複製 / 移動）で・どこへ貼るかを決める正本**。純粋関数だけで、
//! ファイルシステムも OS のクリップボードも触らない（読み書きは dispatch が host 経由で行う）。
//! ⌘V・右クリックの「貼り付け」・CLI `tako file paste`・MCP `tako_file_op` の `op=paste` が
//! 同じ規則で動くので、画面とコマンドで貼り付け先が割れない。
//!
//! ## 中身が 2 か所にある
//!
//! - **tako の中**（[`FileClipboard`]）: ⌘C / ⌘X で置いたパスとモード。**切り取り（移動）は
//!   ここにしか無い**（macOS のペーストボードには「切り取り」の印が無い）
//! - **OS のクリップボード**（[`OsFiles`]）: ⌘C / ⌘X は同時にファイルの URL を書く
//!   （Finder / エクスプローラーへ貼れる）。Finder でコピーしたファイルもここに来る
//!
//! どちらを使うかは [`resolve`]: tako が書いた後に OS のクリップボードが書き換わって
//! いなければ（変更番号が一致）tako の中身、書き換わっていれば OS の中身。

use std::path::{Path, PathBuf};

/// 貼るときに複製するか移すか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipMode {
    /// コピー（⌘C）→ 貼ると複製
    Copy,
    /// 切り取り（⌘X）→ 貼ると移動
    Cut,
}

impl ClipMode {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Cut => "cut",
        }
    }
}

/// tako が持つクリップボードの中身（⌘C / ⌘X で置いたもの）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileClipboard {
    pub paths: Vec<PathBuf>,
    pub mode: ClipMode,
    /// OS のクリップボードへ書けたときの変更番号（macOS の `changeCount` / Windows の
    /// `GetClipboardSequenceNumber`）。書けなかった（未対応の OS・失敗）ときは None で、
    /// tako の中だけのクリップボードとして扱う
    pub stamp: Option<u64>,
}

impl FileClipboard {
    /// `path` が切り取り中か（ツリーの行を薄く描くかどうか）
    pub fn is_cut(&self, path: &Path) -> bool {
        self.mode == ClipMode::Cut && self.paths.iter().any(|p| p == path)
    }
}

/// OS のクリップボードから読めたファイル
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OsFiles {
    /// 変更番号（読めない環境では None）
    pub stamp: Option<u64>,
    pub paths: Vec<PathBuf>,
    /// 置いた側が移動を求めている（エクスプローラーの切り取り = `Preferred DropEffect` が
    /// `DROPEFFECT_MOVE`。macOS には該当する印が無いので常に偽）
    pub cut: bool,
}

/// 貼るものがどちらから来たか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipSource {
    /// tako の ⌘C / ⌘X
    Tako,
    /// OS のクリップボード（Finder / エクスプローラーでコピーしたもの）
    Os,
}

impl ClipSource {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Tako => "tako",
            Self::Os => "os",
        }
    }
}

/// 貼るもの
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    pub paths: Vec<PathBuf>,
    pub mode: ClipMode,
    pub source: ClipSource,
}

/// tako の中身が古いか（tako が書いた後に OS のクリップボードが書き換わった）
pub fn is_stale(tako: &FileClipboard, os_stamp: Option<u64>) -> bool {
    match tako.stamp {
        Some(stamp) => os_stamp != Some(stamp),
        // OS へ書けなかった中身は比べる相手が無い（tako の中だけのクリップボード）
        None => false,
    }
}

/// 貼るものを決める（どちらにも無ければ None）
pub fn resolve(tako: Option<&FileClipboard>, os: &OsFiles) -> Option<Clip> {
    if let Some(tako) = tako.filter(|t| !t.paths.is_empty() && !is_stale(t, os.stamp)) {
        return Some(Clip {
            paths: tako.paths.clone(),
            mode: tako.mode,
            source: ClipSource::Tako,
        });
    }
    if os.paths.is_empty() {
        return None;
    }
    Some(Clip {
        paths: os.paths.clone(),
        mode: if os.cut {
            ClipMode::Cut
        } else {
            ClipMode::Copy
        },
        source: ClipSource::Os,
    })
}

/// 貼り付け先のフォルダ（VSCode のエクスプローラーと同じ規則）。
///
/// - **貼るもの自身の行**で貼る → その親（= 同じフォルダに複製を作る。フォルダを
///   自分の中へ写そうとして断られる、にしない）
/// - フォルダの行 → その中
/// - ファイルの行 → そのファイルのあるフォルダ
pub fn paste_target(src: &Path, row: &Path, row_is_dir: bool) -> PathBuf {
    if src == row || !row_is_dir {
        return row.parent().unwrap_or(row).to_path_buf();
    }
    row.to_path_buf()
}

/// 切り取りを貼った後に残す中身（移せたものを抜く。全部移せたら None = 空にする）。
///
/// 移せなかったもの（同名がある・別のボリューム）は切り取り中のまま残し、別の場所へ
/// 貼り直せるようにする（Finder の「移動できなかったものは元の場所に残る」と同じ）
pub fn after_cut_paste(clip: &FileClipboard, moved: &[PathBuf]) -> Option<FileClipboard> {
    let rest: Vec<PathBuf> = clip
        .paths
        .iter()
        .filter(|p| !moved.contains(p))
        .cloned()
        .collect();
    if rest.is_empty() {
        return None;
    }
    Some(FileClipboard {
        paths: rest,
        mode: clip.mode,
        stamp: clip.stamp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tako(mode: ClipMode, stamp: Option<u64>) -> FileClipboard {
        FileClipboard {
            paths: vec![PathBuf::from("/w/a.txt")],
            mode,
            stamp,
        }
    }

    #[test]
    fn takoの中身はosが書き換わっていなければ使い書き換わっていればosを使う() {
        let finder = OsFiles {
            stamp: Some(8),
            paths: vec![PathBuf::from("/f/x.png")],
            cut: false,
        };
        // tako が書いた直後（変更番号が一致）は tako の中身 = 切り取りが生きる
        let mine = OsFiles {
            stamp: Some(7),
            paths: vec![PathBuf::from("/w/a.txt")],
            cut: false,
        };
        let clip = resolve(Some(&tako(ClipMode::Cut, Some(7))), &mine).unwrap();
        assert_eq!((clip.mode, clip.source), (ClipMode::Cut, ClipSource::Tako));
        // Finder で何かをコピーした後は OS の中身（tako の切り取りは古い）
        let clip = resolve(Some(&tako(ClipMode::Cut, Some(7))), &finder).unwrap();
        assert_eq!(clip.paths, vec![PathBuf::from("/f/x.png")]);
        assert_eq!((clip.mode, clip.source), (ClipMode::Copy, ClipSource::Os));
        assert!(is_stale(&tako(ClipMode::Cut, Some(7)), Some(8)));
        // OS へ書けなかった中身は比べず使う
        let clip = resolve(Some(&tako(ClipMode::Copy, None)), &finder).unwrap();
        assert_eq!(clip.source, ClipSource::Tako);
        // エクスプローラーの切り取りは移動
        let cut = OsFiles {
            cut: true,
            ..finder.clone()
        };
        assert_eq!(resolve(None, &cut).unwrap().mode, ClipMode::Cut);
        // どちらにも無ければ何も貼らない
        assert_eq!(resolve(None, &OsFiles::default()), None);
        let gone = OsFiles {
            stamp: Some(9),
            ..OsFiles::default()
        };
        assert_eq!(resolve(Some(&tako(ClipMode::Copy, Some(7))), &gone), None);
    }

    #[test]
    fn 貼り付け先は行の種類と貼るもの自身かで決まる() {
        let src = Path::new("/w/d");
        assert_eq!(
            paste_target(src, Path::new("/w/x"), true),
            PathBuf::from("/w/x")
        );
        assert_eq!(
            paste_target(src, Path::new("/w/x/f.txt"), false),
            PathBuf::from("/w/x")
        );
        // 自分の行で貼る = 同じフォルダに複製
        assert_eq!(paste_target(src, src, true), PathBuf::from("/w"));
        let file = Path::new("/w/a.txt");
        assert_eq!(paste_target(file, file, false), PathBuf::from("/w"));
        // 自分の配下の行は「その中」のまま（コピーの段取りが配下として断る）
        assert_eq!(
            paste_target(src, Path::new("/w/d/sub"), true),
            PathBuf::from("/w/d/sub")
        );
    }

    #[test]
    fn 切り取りは移せたものだけ抜く() {
        let clip = FileClipboard {
            paths: vec![PathBuf::from("/w/a"), PathBuf::from("/w/b")],
            mode: ClipMode::Cut,
            stamp: Some(3),
        };
        assert!(clip.is_cut(Path::new("/w/a")));
        assert!(!clip.is_cut(Path::new("/w/c")));
        let rest = after_cut_paste(&clip, &[PathBuf::from("/w/a")]).unwrap();
        assert_eq!(rest.paths, vec![PathBuf::from("/w/b")]);
        assert_eq!(rest.stamp, Some(3));
        assert_eq!(
            after_cut_paste(&clip, &[PathBuf::from("/w/a"), PathBuf::from("/w/b")]),
            None
        );
        let copied = FileClipboard {
            mode: ClipMode::Copy,
            ..clip
        };
        assert!(!copied.is_cut(Path::new("/w/a")), "コピーは薄く描かない");
    }
}
