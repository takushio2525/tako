//! ファイル・フォルダを別のフォルダへ移す（Issue #1834）
//!
//! **判定・実行・開いているファイルの付け替え先の計算の正本**。ファイルツリーの
//! ドラッグ＆ドロップと CLI `tako file move` / MCP `tako_file_op` の `op=move` は、
//! どれも dispatch の `FileOpKind::Move` を通ってここを呼ぶ（開発不変条件）。
//! ドラッグ中の表示（移せる / 移せない）も [`drop_verdict`] = 同じ規則で決めるので、
//! 画面が「移せる」と言った場所で dispatch が断る、という食い違いが構造的に起きない。
//!
//! ## 断るもの（黙って上書きしない・壊さない）
//!
//! - 移動先に**同じ名前**がある（[`MoveRefusal::NameTaken`]）。`rename(2)` は既存の
//!   ファイルや空のフォルダを黙って置き換えるので、呼ぶ前に必ず見る
//! - フォルダを**自分自身・自分の配下**へ（[`MoveRefusal::IntoSelf`] /
//!   [`MoveRefusal::IntoDescendant`]）。比較は実体の形（`canonicalize`）で行い、
//!   `..`・シンボリックリンク・Windows の大文字小文字の違いで素通りさせない
//! - **別のボリューム**（`rename` の EXDEV = [`MoveRefusal::CrossDevice`]）。
//!   コピーしてから元を消す形は途中で失敗すると両側が半端に残り、画面にも
//!   コマンドにも確認の段が無いので採らない（理由を返して断る）
//!
//! ## 開いているファイルの付け替え
//!
//! 移したファイル（フォルダなら配下すべて）をプレビュー・編集中のペインは、
//! [`follows`] が返す新しいパスへ付け替える。**移す前に**呼ぶこと（実体の形を
//! 引けるのは元の場所にあるうちだけ）。付け替えないと、ディスクの監視がそのファイルを
//! 「外で削除された」（#1659）と読み、未保存の変更が競合の帯の向こうへ閉じ込められる。

use std::path::{Path, PathBuf};

use crate::platform::path::canonicalize;

/// 移せない理由
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveRefusal {
    /// 移す元が無い
    SourceMissing,
    /// 移動先がフォルダではない（無い・ファイル）
    DestNotDir,
    /// 移す元に名前が無い（`/` やドライブの根）
    NoName,
    /// フォルダを自分自身へ
    IntoSelf,
    /// フォルダを自分の配下へ
    IntoDescendant,
    /// 移動先に同じ名前がある（上書きしない）
    NameTaken,
    /// 別のボリュームへ（`rename` の EXDEV）
    CrossDevice,
    /// リモート（SSH）の行。ローカルのファイルシステムの操作を通さない（#919）ので
    /// **画面だけが出す**（dispatch へはリモートのパスが届かない）
    Remote,
    /// ワークスペースのフォルダの見出し行。ペインの cwd やピン留めの実体なので、
    /// ドラッグの弾みで丸ごと動かさない（**画面だけが出す**。CLI / MCP はパスを
    /// 名指しした時点で意図が明らかなので断らない）
    WorkspaceRoot,
    /// OS が断った（権限など）。中身は OS のエラー文
    Io(String),
}

impl MoveRefusal {
    /// 機械可読の分類名（応答・診断に載せる）
    pub fn slug(&self) -> &'static str {
        match self {
            Self::SourceMissing => "source_missing",
            Self::DestNotDir => "dest_not_dir",
            Self::NoName => "no_name",
            Self::IntoSelf => "into_self",
            Self::IntoDescendant => "into_descendant",
            Self::NameTaken => "name_taken",
            Self::CrossDevice => "cross_device",
            Self::Remote => "remote",
            Self::WorkspaceRoot => "workspace_root",
            Self::Io(_) => "io",
        }
    }

    /// 理由（日本語。dispatch のエラー文と同じ流儀。画面の文言は `ui_text` が持つ）
    pub fn reason(&self) -> String {
        match self {
            Self::SourceMissing => "移す元が見つからない".into(),
            Self::DestNotDir => "移動先がフォルダではない".into(),
            Self::NoName => "移す元に名前が無い".into(),
            Self::IntoSelf => "フォルダを自分自身へは移せない".into(),
            Self::IntoDescendant => "フォルダを自分の配下へは移せない".into(),
            Self::NameTaken => "移動先に同じ名前がある（上書きしない）".into(),
            Self::CrossDevice => {
                "別のボリュームへは移せない（コピーしてから元を消す移動は未対応）".into()
            }
            Self::Remote => "リモート（SSH）の項目は移せない".into(),
            Self::WorkspaceRoot => "ワークスペースのフォルダ（見出し）は移せない".into(),
            Self::Io(e) => format!("OS が断った: {e}"),
        }
    }
}

/// 移すかどうかの判定
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropVerdict {
    /// 移せる
    Move,
    /// 既にそのフォルダにある（何も起きない）
    Unchanged,
    /// 移せない
    Refused(MoveRefusal),
}

/// **字面だけ**で決まる判定（ファイルシステムを読まない）。
///
/// [`plan`] は実体の形（`canonicalize` 済み）をこれに通し、画面のドラッグ中の判定
/// （[`drop_verdict`]）はツリーの行のパスをそのまま通す。規則はここ 1 か所
pub fn lexical_verdict(src: &Path, dest_dir: &Path) -> DropVerdict {
    if src == dest_dir {
        return DropVerdict::Refused(MoveRefusal::IntoSelf);
    }
    if src.parent() == Some(dest_dir) {
        return DropVerdict::Unchanged;
    }
    // `Path::starts_with` は成分単位なので `/a/b` と `/a/bc` を取り違えない
    if dest_dir.starts_with(src) {
        return DropVerdict::Refused(MoveRefusal::IntoDescendant);
    }
    DropVerdict::Move
}

/// ドラッグしている項目（画面の行）
#[derive(Debug, Clone, Copy)]
pub struct DragItem<'a> {
    pub path: &'a Path,
    /// ワークスペースのフォルダの見出し行
    pub workspace_root: bool,
    /// リモート（SSH）の行
    pub remote: bool,
}

/// 画面のドラッグ中の判定（ツリーの行の上に来るたびに呼ぶ）。
///
/// `name_taken` は「移動先にこの名前があるか」を答える。**判定が `Move` に
/// なるときだけ**呼ぶので、行をまたぐたびに stat が 1 回走るだけで済む
pub fn drop_verdict(
    item: DragItem<'_>,
    dest_dir: &Path,
    dest_remote: bool,
    name_taken: impl FnOnce(&Path) -> bool,
) -> DropVerdict {
    if item.remote || dest_remote {
        return DropVerdict::Refused(MoveRefusal::Remote);
    }
    if item.workspace_root {
        return DropVerdict::Refused(MoveRefusal::WorkspaceRoot);
    }
    match lexical_verdict(item.path, dest_dir) {
        DropVerdict::Move => {
            let Some(name) = item.path.file_name() else {
                return DropVerdict::Refused(MoveRefusal::NoName);
            };
            if name_taken(&dest_dir.join(name)) {
                DropVerdict::Refused(MoveRefusal::NameTaken)
            } else {
                DropVerdict::Move
            }
        }
        other => other,
    }
}

/// 移す段取り（[`plan`] が作り、[`follows`] と [`execute`] が使う）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovePlan {
    /// 移す元（呼び手が渡した形のまま）
    pub from: PathBuf,
    /// 移した後のパス（移動先のフォルダ + 元の名前）
    pub to: PathBuf,
    pub kind: EntryKind,
    /// 移す元の実体の形（親だけ `canonicalize` し、最後の成分は辿らない = リンクそのもの）。
    /// 開いているファイルの照合で、字面が食い違うとき（`/tmp` と `/private/tmp`、
    /// Windows の大文字小文字）に使う
    from_real: PathBuf,
}

/// 移す項目の種別（応答の `kind`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    /// シンボリックリンク（**リンクそのもの**を移す。指す先は動かさない）
    Symlink,
}

impl EntryKind {
    pub fn slug(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Dir => "dir",
            Self::Symlink => "symlink",
        }
    }
}

/// [`plan`] の結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Planned {
    /// 移す
    Move(MovePlan),
    /// 既にそのフォルダにある（何もしない）
    Unchanged { at: PathBuf, kind: EntryKind },
}

/// 移せるかを確かめて段取りを作る（ファイルシステムを読むが書かない）
pub fn plan(src: &Path, dest_dir: &Path) -> Result<Planned, MoveRefusal> {
    let meta = std::fs::symlink_metadata(src).map_err(|_| MoveRefusal::SourceMissing)?;
    let kind = if meta.file_type().is_symlink() {
        EntryKind::Symlink
    } else if meta.is_dir() {
        EntryKind::Dir
    } else {
        EntryKind::File
    };
    let name = src.file_name().ok_or(MoveRefusal::NoName)?;
    if !std::fs::metadata(dest_dir).is_ok_and(|m| m.is_dir()) {
        return Err(MoveRefusal::DestNotDir);
    }
    // 実体の形で比べる。移す元は**最後の成分を辿らない**（リンクを移すときに
    // 指す先のフォルダと取り違えない）
    let parent = src
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let from_real = canonicalize(parent)
        .map_err(|_| MoveRefusal::SourceMissing)?
        .join(name);
    let dest_real = canonicalize(dest_dir).map_err(|_| MoveRefusal::DestNotDir)?;
    match lexical_verdict(&from_real, &dest_real) {
        DropVerdict::Refused(refusal) => return Err(refusal),
        DropVerdict::Unchanged => {
            return Ok(Planned::Unchanged {
                at: src.to_path_buf(),
                kind,
            })
        }
        DropVerdict::Move => {}
    }
    let to = dest_dir.join(name);
    if exists_no_follow(&to) {
        return Err(MoveRefusal::NameTaken);
    }
    Ok(Planned::Move(MovePlan {
        from: src.to_path_buf(),
        to,
        kind,
        from_real,
    }))
}

/// 移す（`rename` 1 回）。直前にもう一度同名を見る（[`plan`] から実行までの間に
/// 作られたものを上書きしない。同じ UI スレッドの続きなので隙間は小さい）
pub fn execute(plan: &MovePlan) -> Result<(), MoveRefusal> {
    if exists_no_follow(&plan.to) {
        return Err(MoveRefusal::NameTaken);
    }
    std::fs::rename(&plan.from, &plan.to).map_err(|e| classify_io(&e))
}

/// `rename` の失敗を理由へ読み替える（EXDEV / `ERROR_NOT_SAME_DEVICE` は std が
/// どちらも `CrossesDevices` へ寄せる）
pub fn classify_io(error: &std::io::Error) -> MoveRefusal {
    if error.kind() == std::io::ErrorKind::CrossesDevices {
        MoveRefusal::CrossDevice
    } else {
        MoveRefusal::Io(error.to_string())
    }
}

/// リンクを辿らずに「その名前の何かがあるか」（壊れたリンクも「ある」）
fn exists_no_follow(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// 開いているファイル 1 つの付け替え
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Follow {
    pub pane: u64,
    pub from: PathBuf,
    pub to: PathBuf,
}

/// `path` が `from`（またはその配下）なら、移した後のパスを返す（字面だけで見る）。
///
/// ツリーの展開状態・ジャンプ履歴のように**元の場所がもう無い**ものの付け替えにも使う
pub fn remap(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    let rest = path.strip_prefix(from).ok()?;
    // `Path::join("")` は末尾に区切りを足すので、同じもの自身は `to` をそのまま返す
    Some(if rest.as_os_str().is_empty() {
        to.to_path_buf()
    } else {
        to.join(rest)
    })
}

/// 移した後に付け替えるペインと新しいパス（**移す前に**呼ぶ）。
///
/// まず字面で照合し、外れたら実体の形で照合する（`/tmp` 経由で開いたファイルを
/// `/private/tmp` の形で移す、Windows で大文字小文字が違う、など）
pub fn follows(plan: &MovePlan, open: &[(u64, PathBuf)]) -> Vec<Follow> {
    open.iter()
        .filter_map(|(pane, path)| {
            let to = remap(path, &plan.from, &plan.to).or_else(|| {
                let real = canonicalize(path).ok()?;
                remap(&real, &plan.from_real, &plan.to)
            })?;
            Some(Follow {
                pane: *pane,
                from: path.clone(),
                to,
            })
        })
        .collect()
}

/// `TAKO_1834_LEGACY=1` で**開いているペインを付け替えない**（#1834 の A/B）。
///
/// 移動そのものは同じ経路で行い、付け替えだけを外す。移したファイルを開いている
/// 編集ペインが「外で削除された」（#1659）の帯を出すのが、付け替えが効いていることの
/// 裏付けになる
pub fn follow_legacy() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var("TAKO_1834_LEGACY").map(|v| v == "1") == Ok(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テストの置き場。**必ず一時 dir の中**（本物のファイルを動かさない = #1811 の教訓）
    fn scratch(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "tako-file-move-{}-{name}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&base).unwrap();
        let real = canonicalize(&base).unwrap();
        let tmp = canonicalize(&std::env::temp_dir()).unwrap();
        assert!(
            real.starts_with(&tmp),
            "テストの置き場が一時 dir の外: {}",
            real.display()
        );
        real
    }

    fn touch(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn 字面の判定は自分自身と配下と同じ場所を見分ける() {
        let d = Path::new("/w/d");
        assert_eq!(
            lexical_verdict(d, d),
            DropVerdict::Refused(MoveRefusal::IntoSelf)
        );
        assert_eq!(
            lexical_verdict(d, Path::new("/w/d/sub/x")),
            DropVerdict::Refused(MoveRefusal::IntoDescendant)
        );
        assert_eq!(lexical_verdict(d, Path::new("/w")), DropVerdict::Unchanged);
        assert_eq!(lexical_verdict(d, Path::new("/w/other")), DropVerdict::Move);
        // 成分単位: `/w/d` の配下ではない `/w/dd`
        assert_eq!(lexical_verdict(d, Path::new("/w/dd")), DropVerdict::Move);
    }

    #[test]
    fn 画面の判定はリモートと見出しと同名を断る() {
        let item = DragItem {
            path: Path::new("/w/a.txt"),
            workspace_root: false,
            remote: false,
        };
        let never = |_: &Path| false;
        assert_eq!(
            drop_verdict(item, Path::new("/w/x"), false, never),
            DropVerdict::Move
        );
        assert_eq!(
            drop_verdict(item, Path::new("/w/x"), true, never),
            DropVerdict::Refused(MoveRefusal::Remote)
        );
        assert_eq!(
            drop_verdict(
                DragItem {
                    remote: true,
                    ..item
                },
                Path::new("/w/x"),
                false,
                never
            ),
            DropVerdict::Refused(MoveRefusal::Remote)
        );
        assert_eq!(
            drop_verdict(
                DragItem {
                    workspace_root: true,
                    ..item
                },
                Path::new("/w/x"),
                false,
                never
            ),
            DropVerdict::Refused(MoveRefusal::WorkspaceRoot)
        );
        // 同名は移動先のフォルダ + 元の名前で問い合わせる
        let mut asked = None;
        let verdict = drop_verdict(item, Path::new("/w/x"), false, |p: &Path| {
            asked = Some(p.to_path_buf());
            true
        });
        assert_eq!(verdict, DropVerdict::Refused(MoveRefusal::NameTaken));
        assert_eq!(asked, Some(PathBuf::from("/w/x/a.txt")));
        // 同じ場所では問い合わせない（stat を走らせない）
        let verdict = drop_verdict(item, Path::new("/w"), false, |_: &Path| {
            panic!("同じ場所で同名を問い合わせた")
        });
        assert_eq!(verdict, DropVerdict::Unchanged);
    }

    #[test]
    fn ファイルとフォルダを移し開いているファイルの付け替え先を返す() {
        let base = scratch("move");
        touch(&base.join("src/a.txt"), "A");
        touch(&base.join("folder/inner/x.txt"), "X");
        std::fs::create_dir_all(base.join("dst")).unwrap();

        let Ok(Planned::Move(p)) = plan(&base.join("src/a.txt"), &base.join("dst")) else {
            panic!("ファイルを移す段取りが立たない");
        };
        assert_eq!(p.kind, EntryKind::File);
        let open = vec![
            (1, base.join("src/a.txt")),
            (2, base.join("folder/inner/x.txt")),
        ];
        assert_eq!(
            follows(&p, &open),
            vec![Follow {
                pane: 1,
                from: base.join("src/a.txt"),
                to: base.join("dst/a.txt"),
            }]
        );
        execute(&p).unwrap();
        assert_eq!(
            std::fs::read_to_string(base.join("dst/a.txt")).unwrap(),
            "A"
        );
        assert!(!base.join("src/a.txt").exists());

        let Ok(Planned::Move(p)) = plan(&base.join("folder"), &base.join("dst")) else {
            panic!("フォルダを移す段取りが立たない");
        };
        assert_eq!(p.kind, EntryKind::Dir);
        assert_eq!(
            follows(&p, &open)
                .into_iter()
                .map(|f| (f.pane, f.to))
                .collect::<Vec<_>>(),
            vec![(2, base.join("dst/folder/inner/x.txt"))],
            "フォルダを移すと配下の開いているファイルが付け替わる"
        );
        execute(&p).unwrap();
        assert!(base.join("dst/folder/inner/x.txt").is_file());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn 同名は上書きせずに断る() {
        let base = scratch("taken");
        touch(&base.join("src/a.txt"), "新");
        touch(&base.join("dst/a.txt"), "既存");
        // 空のフォルダも `rename(2)` は黙って置き換えるので断る
        std::fs::create_dir_all(base.join("src/d")).unwrap();
        std::fs::create_dir_all(base.join("dst/d")).unwrap();
        assert_eq!(
            plan(&base.join("src/a.txt"), &base.join("dst")),
            Err(MoveRefusal::NameTaken)
        );
        assert_eq!(
            plan(&base.join("src/d"), &base.join("dst")),
            Err(MoveRefusal::NameTaken)
        );
        assert_eq!(
            std::fs::read_to_string(base.join("dst/a.txt")).unwrap(),
            "既存"
        );
        // 段取りの後に同名ができても、実行の直前にもう一度見て断る
        touch(&base.join("src/b.txt"), "b");
        let Ok(Planned::Move(p)) = plan(&base.join("src/b.txt"), &base.join("dst")) else {
            panic!("段取りが立たない");
        };
        touch(&base.join("dst/b.txt"), "割り込み");
        assert_eq!(execute(&p), Err(MoveRefusal::NameTaken));
        assert_eq!(
            std::fs::read_to_string(base.join("dst/b.txt")).unwrap(),
            "割り込み"
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn 自分自身と配下へは実体の形で断る() {
        let base = scratch("self");
        std::fs::create_dir_all(base.join("d/sub/deep")).unwrap();
        assert_eq!(
            plan(&base.join("d"), &base.join("d")),
            Err(MoveRefusal::IntoSelf)
        );
        assert_eq!(
            plan(&base.join("d"), &base.join("d/sub/deep")),
            Err(MoveRefusal::IntoDescendant)
        );
        // 字面を `..` でずらしても素通りしない
        assert_eq!(
            plan(&base.join("d"), &base.join("d/sub/../sub")),
            Err(MoveRefusal::IntoDescendant)
        );
        assert!(base.join("d/sub/deep").is_dir(), "何も動いていない");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn 同じ場所は何もしない() {
        let base = scratch("same");
        touch(&base.join("d/a.txt"), "a");
        assert_eq!(
            plan(&base.join("d/a.txt"), &base.join("d")),
            Ok(Planned::Unchanged {
                at: base.join("d/a.txt"),
                kind: EntryKind::File
            })
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn 移す元が無い移動先がフォルダでないを断る() {
        let base = scratch("missing");
        touch(&base.join("f.txt"), "f");
        assert_eq!(
            plan(&base.join("nope"), &base),
            Err(MoveRefusal::SourceMissing)
        );
        assert_eq!(
            plan(&base.join("f.txt"), &base.join("nope")),
            Err(MoveRefusal::DestNotDir)
        );
        std::fs::create_dir_all(base.join("d")).unwrap();
        assert_eq!(
            plan(&base.join("d"), &base.join("f.txt")),
            Err(MoveRefusal::DestNotDir)
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn 日本語と空白を含むパスを移せる() {
        let base = scratch("ja");
        touch(&base.join("元 の場所/メモ 1.md"), "本文");
        std::fs::create_dir_all(base.join("移動 先")).unwrap();
        let Ok(Planned::Move(p)) = plan(&base.join("元 の場所/メモ 1.md"), &base.join("移動 先"))
        else {
            panic!("段取りが立たない");
        };
        execute(&p).unwrap();
        assert_eq!(
            std::fs::read_to_string(base.join("移動 先/メモ 1.md")).unwrap(),
            "本文"
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn 別のボリュームは理由つきで断る() {
        // 実ボリュームを跨ぐ検査は `scripts/test-tree-move-1834.sh`（hdiutil の一時イメージ）。
        // ここは std が EXDEV を寄せる先の読み替えだけを固定する
        let exdev = std::io::Error::from(std::io::ErrorKind::CrossesDevices);
        assert_eq!(classify_io(&exdev), MoveRefusal::CrossDevice);
        assert!(MoveRefusal::CrossDevice.reason().contains("別のボリューム"));
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert_eq!(classify_io(&denied).slug(), "io");
    }

    #[test]
    fn 付け替えは成分単位で同じもの自身は末尾に区切りを足さない() {
        let from = Path::new("/w/d");
        let to = Path::new("/w/x/d");
        assert_eq!(remap(Path::new("/w/d"), from, to), Some(to.to_path_buf()));
        assert_eq!(
            remap(Path::new("/w/d/a/b.txt"), from, to),
            Some(PathBuf::from("/w/x/d/a/b.txt"))
        );
        assert_eq!(remap(Path::new("/w/dd/a.txt"), from, to), None);
        assert_eq!(remap(Path::new("/w/a.txt"), from, to), None);
    }

    #[test]
    fn 付け替えは字面が違っても実体が同じなら当たる() {
        let base = scratch("real");
        touch(&base.join("d/a.txt"), "a");
        std::fs::create_dir_all(base.join("dst")).unwrap();
        let Ok(Planned::Move(p)) = plan(&base.join("d"), &base.join("dst")) else {
            panic!("段取りが立たない");
        };
        // `..` を挟んだ字面で開いていても付け替わる
        let open = vec![(7, base.join("dst/../d/a.txt"))];
        assert_eq!(
            follows(&p, &open)
                .into_iter()
                .map(|f| f.to)
                .collect::<Vec<_>>(),
            vec![base.join("dst/d/a.txt")]
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn シンボリックリンクはリンクそのものを移す() {
        let base = scratch("link");
        std::fs::create_dir_all(base.join("real/inner")).unwrap();
        std::fs::create_dir_all(base.join("dst")).unwrap();
        std::os::unix::fs::symlink(base.join("real"), base.join("lnk")).unwrap();
        let Ok(Planned::Move(p)) = plan(&base.join("lnk"), &base.join("dst")) else {
            panic!("リンクを移す段取りが立たない");
        };
        assert_eq!(p.kind, EntryKind::Symlink);
        execute(&p).unwrap();
        assert!(std::fs::symlink_metadata(base.join("dst/lnk"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(base.join("real/inner").is_dir(), "指す先は動かない");
        // リンクを指す先のフォルダの中へ移すのは「配下」ではない（リンクが動くだけ）
        let Ok(Planned::Move(p)) = plan(&base.join("dst/lnk"), &base.join("real")) else {
            panic!("リンクを指す先へ移す段取りが立たない");
        };
        execute(&p).unwrap();
        assert!(std::fs::symlink_metadata(base.join("real/lnk")).is_ok());
        // 実体のフォルダを、配下を指すリンクの先へ移すのは断る
        std::os::unix::fs::symlink(base.join("real/inner"), base.join("to-inner")).unwrap();
        assert_eq!(
            plan(&base.join("real"), &base.join("to-inner")),
            Err(MoveRefusal::IntoDescendant)
        );
        std::fs::remove_dir_all(&base).unwrap();
    }
}
