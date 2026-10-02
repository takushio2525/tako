//! ファイル・フォルダを別のフォルダへ複製する（FR-3.34 / Issue #1860）
//!
//! **判定・名前の決め方・実行の正本**。ファイルツリーの ⌘C → ⌘V（右クリックの
//! 「コピー」→「貼り付け」・Finder からの貼り付け）と CLI `tako file copy` / MCP
//! `tako_file_op` の `op=copy` は、どれも dispatch を通ってここを呼ぶ（開発不変条件）。
//! 切り取り → 貼り付けは移動なので [`crate::file_move`] の側（2 つ目の移動を作らない）。
//!
//! ## 上書きしない
//!
//! 貼り付け先に同じ名前があれば、Finder と同じく「名前 のコピー」「名前 のコピー 2」…
//! （Windows はエクスプローラーと同じ「名前 - コピー」「名前 - コピー (2)」）の空いている
//! 名前で置く（[`copy_name`]）。同じフォルダへのコピー = 複製もこれで成立する。
//! 置き場は**排他的に作る**（フォルダは `create_dir`、ファイルは `create_new` で名前を
//! 押さえてから中身を写す、リンクは `symlink`）ので、名前を決めてから作るまでの間に
//! 誰かが同じ名前を作っても上書きしない（その回は [`CopyRefusal::NameTaken`] で断る）。
//!
//! ## 半端に残さない
//!
//! 途中で失敗したら（読めない項目・空き容量・特殊ファイル）、**この呼び出しで作った
//! 置き場だけ**を消して元へ戻す。消すのは排他的に作れた置き場（= 呼ぶ前には無かったもの）
//! だけ。フォルダの権限は全部写し終えてから付ける（読み取り専用のフォルダを先に作ると、
//! 中身を写せず・戻すときも消せない）。
//!
//! ## 写すもの
//!
//! 中身と権限（`std::fs::copy`。macOS は `fcopyfile` で拡張属性も）。シンボリックリンクは
//! **リンクそのもの**（指す先は辿らない = 自分の祖先を指すリンクで無限に潜らない。
//! Finder / `cp -R` と同じ）。FIFO・ソケット・デバイスは読むと止まる / 写す意味が無いので
//! 理由つきで断る。

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::file_move::{lexical_verdict, DropVerdict, EntryKind, MoveRefusal};
use crate::i18n::Lang;
use crate::platform::fs_copy;
use crate::platform::path::canonicalize;
use crate::platform::support::Platform;

/// コピーできない理由
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyRefusal {
    /// コピー元が無い
    SourceMissing,
    /// 貼り付け先がフォルダではない（無い・ファイル）
    DestNotDir,
    /// コピー元に名前が無い（`/` やドライブの根）
    NoName,
    /// フォルダを自分自身の中へ
    IntoSelf,
    /// フォルダを自分の配下へ（写した先をまた写して終わらない）
    IntoDescendant,
    /// 名前を決めてから作るまでの間に同じ名前ができた（上書きしない）
    NameTaken,
    /// 別名を [`MAX_COPY_NUMBER`] まで試しても空かない
    NamesExhausted,
    /// 読めない項目がある（権限）
    Unreadable(PathBuf),
    /// 貼り付け先に書き込めない（権限）
    Unwritable(PathBuf),
    /// FIFO・ソケット・デバイス
    Special(PathBuf),
    /// シンボリックリンクを作る権限が無い（Windows は開発者モードか管理者が要る）
    SymlinkPrivilege(PathBuf),
    /// 空き容量が足りない
    NoSpace,
    /// OS が断った（中身は OS のエラー文）
    Io(String),
}

impl CopyRefusal {
    /// 機械可読の分類名（応答・診断に載せる）
    pub fn slug(&self) -> &'static str {
        match self {
            Self::SourceMissing => "source_missing",
            Self::DestNotDir => "dest_not_dir",
            Self::NoName => "no_name",
            Self::IntoSelf => "into_self",
            Self::IntoDescendant => "into_descendant",
            Self::NameTaken => "name_taken",
            Self::NamesExhausted => "names_exhausted",
            Self::Unreadable(_) => "unreadable",
            Self::Unwritable(_) => "unwritable",
            Self::Special(_) => "special",
            Self::SymlinkPrivilege(_) => "symlink_privilege",
            Self::NoSpace => "no_space",
            Self::Io(_) => "io",
        }
    }

    /// 理由（日本語。dispatch のエラー文と同じ流儀。画面の文言は `ui_text` が持つ）
    pub fn reason(&self) -> String {
        match self {
            Self::SourceMissing => "コピー元が見つからない".into(),
            Self::DestNotDir => "貼り付け先がフォルダではない".into(),
            Self::NoName => "コピー元に名前が無い".into(),
            Self::IntoSelf => "フォルダを自分自身の中へはコピーできない".into(),
            Self::IntoDescendant => "フォルダを自分の配下へはコピーできない".into(),
            Self::NameTaken => "貼り付け先に同じ名前ができた（上書きしない）".into(),
            Self::NamesExhausted => "空いている名前が見つからない（上書きしない）".into(),
            Self::Unreadable(p) => format!("読めない項目がある: {}", p.display()),
            Self::Unwritable(p) => format!("書き込めない: {}", p.display()),
            Self::Special(p) => format!(
                "特殊なファイル（FIFO・ソケット・デバイス）はコピーできない: {}",
                p.display()
            ),
            Self::SymlinkPrivilege(p) => format!(
                "シンボリックリンクを作る権限が無い（Windows は開発者モードか管理者が要る）: {}",
                p.display()
            ),
            Self::NoSpace => "空き容量が足りない".into(),
            Self::Io(e) => format!("OS が断った: {e}"),
        }
    }
}

/// 別名の付け方（OS のファイルマネージャに倣う）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamingStyle {
    /// macOS の Finder: 「a のコピー.txt」「a のコピー 2.txt」（英語は「a copy.txt」「a copy 2.txt」）
    Finder,
    /// Windows のエクスプローラー: 「a - コピー.txt」「a - コピー (2).txt」
    /// （英語は「a - Copy.txt」「a - Copy (2).txt」）
    Explorer,
}

/// 別名の付け方と言語
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyNaming {
    pub style: NamingStyle,
    pub lang: Lang,
}

impl CopyNaming {
    /// 実行中の OS と tako の表示言語（`tako lang`）
    pub fn current() -> Self {
        Self::for_platform(Platform::current(), crate::i18n::lang())
    }

    /// 値で決める（macOS の上から Windows の付け方も検査できる）
    pub fn for_platform(platform: Platform, lang: Lang) -> Self {
        let style = match platform {
            Platform::MacOs => NamingStyle::Finder,
            Platform::Windows => NamingStyle::Explorer,
        };
        Self { style, lang }
    }

    fn word(self) -> &'static str {
        match (self.style, self.lang) {
            (NamingStyle::Finder, Lang::Ja) => "のコピー",
            (NamingStyle::Finder, Lang::En) => "copy",
            (NamingStyle::Explorer, Lang::Ja) => "コピー",
            (NamingStyle::Explorer, Lang::En) => "Copy",
        }
    }
}

/// 別名の番号の上限（それ以上は [`CopyRefusal::NamesExhausted`]）
pub const MAX_COPY_NUMBER: u32 = 9999;

/// `n` 番目（1 始まり）の別名。ファイルは拡張子の前に、フォルダは名前の後ろに付ける。
///
/// Finder は「a のコピー.txt」をもう一度複製すると「a のコピー 2.txt」にする
/// （「a のコピー のコピー.txt」にしない）ので、付いている印を外してから数える。
/// エクスプローラーは外さない（「a - コピー - コピー.txt」になる）のでそのまま
pub fn copy_name(name: &str, is_dir: bool, n: u32, naming: CopyNaming) -> String {
    let (stem, ext) = split_ext(name, is_dir);
    let word = naming.word();
    let core = match naming.style {
        NamingStyle::Finder => {
            let base = strip_finder_mark(stem, word);
            if n <= 1 {
                format!("{base} {word}")
            } else {
                format!("{base} {word} {n}")
            }
        }
        NamingStyle::Explorer => {
            if n <= 1 {
                format!("{stem} - {word}")
            } else {
                format!("{stem} - {word} ({n})")
            }
        }
    };
    match ext {
        Some(ext) => format!("{core}.{ext}"),
        None => core,
    }
}

/// 名前を本体と拡張子に分ける（ドット始まり = 隠しファイルの先頭のドットは拡張子にしない）
fn split_ext(name: &str, is_dir: bool) -> (&str, Option<&str>) {
    if is_dir {
        return (name, None);
    }
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => (&name[..i], Some(&name[i + 1..])),
        _ => (name, None),
    }
}

/// 「 のコピー」「 のコピー 12」を外す（外すと空になるものは外さない）
fn strip_finder_mark<'a>(stem: &'a str, word: &str) -> &'a str {
    let mark = format!(" {word}");
    if let Some(base) = stem.strip_suffix(mark.as_str()).filter(|b| !b.is_empty()) {
        return base;
    }
    if let Some((head, num)) = stem.rsplit_once(' ') {
        if !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit()) {
            if let Some(base) = head.strip_suffix(mark.as_str()).filter(|b| !b.is_empty()) {
                return base;
            }
        }
    }
    stem
}

/// 貼り付け先で使う名前と、別名にしたか。空いていれば元の名前のまま
pub fn free_name(
    dest_dir: &Path,
    name: &OsStr,
    is_dir: bool,
    naming: CopyNaming,
) -> Result<(std::ffi::OsString, bool), CopyRefusal> {
    if !exists_no_follow(&dest_dir.join(name)) {
        return Ok((name.to_os_string(), false));
    }
    // UTF-8 でない名前は印を安全に挟めないので、同名があれば断る（上書きはしない）
    let Some(text) = name.to_str() else {
        return Err(CopyRefusal::NamesExhausted);
    };
    for n in 1..=MAX_COPY_NUMBER {
        let candidate = copy_name(text, is_dir, n, naming);
        if candidate != text && !exists_no_follow(&dest_dir.join(&candidate)) {
            return Ok((candidate.into(), true));
        }
    }
    Err(CopyRefusal::NamesExhausted)
}

/// コピーの段取り（[`plan`] が作り、[`execute`] が使う）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyPlan {
    /// コピー元（呼び手が渡した形のまま）
    pub from: PathBuf,
    /// 置く場所（貼り付け先のフォルダ + 決めた名前）
    pub to: PathBuf,
    pub kind: EntryKind,
    /// 同名があったので別名にした
    pub renamed: bool,
}

/// コピーできるかを確かめて段取りを作る（ファイルシステムを読むが書かない）
pub fn plan(src: &Path, dest_dir: &Path, naming: CopyNaming) -> Result<CopyPlan, CopyRefusal> {
    let meta = std::fs::symlink_metadata(src).map_err(|_| CopyRefusal::SourceMissing)?;
    let file_type = meta.file_type();
    let kind = if file_type.is_symlink() {
        EntryKind::Symlink
    } else if file_type.is_dir() {
        EntryKind::Dir
    } else if file_type.is_file() {
        EntryKind::File
    } else {
        return Err(CopyRefusal::Special(src.to_path_buf()));
    };
    let name = src.file_name().ok_or(CopyRefusal::NoName)?;
    if !std::fs::metadata(dest_dir).is_ok_and(|m| m.is_dir()) {
        return Err(CopyRefusal::DestNotDir);
    }
    // 実体の形で比べる（`file_move::plan` と同じ作り）。リンクは最後の成分を辿らない
    // （リンクを写すときに指す先のフォルダと取り違えない）ので親だけを引く。それ以外は
    // コピー元そのものを引く（大文字小文字を区別しないファイルシステムで綴りが違っても当たる）
    let from_real = if kind == EntryKind::Symlink {
        let parent = src
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        canonicalize(parent)
            .map_err(|_| CopyRefusal::SourceMissing)?
            .join(name)
    } else {
        canonicalize(src).map_err(|_| CopyRefusal::SourceMissing)?
    };
    let dest_real = canonicalize(dest_dir).map_err(|_| CopyRefusal::DestNotDir)?;
    match lexical_verdict(&from_real, &dest_real) {
        DropVerdict::Refused(MoveRefusal::IntoSelf) => return Err(CopyRefusal::IntoSelf),
        DropVerdict::Refused(MoveRefusal::IntoDescendant) => {
            return Err(CopyRefusal::IntoDescendant)
        }
        DropVerdict::Refused(other) => return Err(CopyRefusal::Io(other.reason())),
        // 同じフォルダ（= 複製）もここを通る。名前は下で別名にする
        DropVerdict::Unchanged | DropVerdict::Move => {}
    }
    let (chosen, renamed) = free_name(dest_dir, name, kind == EntryKind::Dir, naming)?;
    Ok(CopyPlan {
        from: src.to_path_buf(),
        to: dest_dir.join(chosen),
        kind,
        renamed,
    })
}

/// 写した量（応答の `entries` に載せる）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CopyStats {
    pub files: u64,
    pub dirs: u64,
    pub links: u64,
    pub bytes: u64,
}

impl CopyStats {
    /// 写した項目の数（ファイル + フォルダ + リンク）
    pub fn entries(&self) -> u64 {
        self.files + self.dirs + self.links
    }
}

/// 写す。失敗したらこの呼び出しで作った置き場だけを消して戻す
pub fn execute(plan: &CopyPlan) -> Result<CopyStats, CopyRefusal> {
    let mut job = Job::default();
    match job.copy_root(plan) {
        Ok(()) => {
            job.apply_dir_permissions();
            Ok(job.stats)
        }
        Err(refusal) => {
            if job.created_root {
                rollback(plan);
            }
            Err(refusal)
        }
    }
}

/// 戻す（[`execute`] が置き場を排他的に作れたときだけ呼ぶ = 呼ぶ前には無かったもの）
fn rollback(plan: &CopyPlan) {
    let _ = match plan.kind {
        EntryKind::Dir => std::fs::remove_dir_all(&plan.to),
        EntryKind::File | EntryKind::Symlink => std::fs::remove_file(&plan.to),
    };
}

#[derive(Default)]
struct Job {
    stats: CopyStats,
    /// 置き場を作れた（ここから先の失敗は戻す）
    created_root: bool,
    /// 写し終えてから付けるフォルダの権限（深い方から付ける = 逆順）
    dir_permissions: Vec<(PathBuf, std::fs::Permissions)>,
}

impl Job {
    fn copy_root(&mut self, plan: &CopyPlan) -> Result<(), CopyRefusal> {
        match plan.kind {
            EntryKind::Dir => {
                std::fs::create_dir(&plan.to).map_err(|e| write_error(&e, &plan.to))?;
                self.created_root = true;
                self.copy_dir(&plan.from, &plan.to)
            }
            EntryKind::File => {
                probe_readable(&plan.from)?;
                // 名前を押さえる（決めてから作るまでに誰かが作っていたら上書きせずに断る）
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&plan.to)
                    .map_err(|e| write_error(&e, &plan.to))?;
                self.created_root = true;
                self.copy_file(&plan.from, &plan.to)
            }
            EntryKind::Symlink => {
                self.copy_link(&plan.from, &plan.to)?;
                self.created_root = true;
                Ok(())
            }
        }
    }

    fn copy_dir(&mut self, src: &Path, dst: &Path) -> Result<(), CopyRefusal> {
        let meta = std::fs::symlink_metadata(src).map_err(|e| read_error(&e, src))?;
        let entries = std::fs::read_dir(src).map_err(|e| read_error(&e, src))?;
        for entry in entries {
            let entry = entry.map_err(|e| read_error(&e, src))?;
            let from = entry.path();
            let to = dst.join(entry.file_name());
            let file_type = entry.file_type().map_err(|e| read_error(&e, &from))?;
            if file_type.is_symlink() {
                self.copy_link(&from, &to)?;
            } else if file_type.is_dir() {
                std::fs::create_dir(&to).map_err(|e| write_error(&e, &to))?;
                self.copy_dir(&from, &to)?;
            } else if file_type.is_file() {
                // 作ったばかりのフォルダの中なので普通は空いている。大文字小文字を
                // 区別するボリュームから区別しないボリュームへ写すと名前がぶつかるので見る
                if exists_no_follow(&to) {
                    return Err(CopyRefusal::NameTaken);
                }
                probe_readable(&from)?;
                self.copy_file(&from, &to)?;
            } else {
                return Err(CopyRefusal::Special(from));
            }
        }
        self.stats.dirs += 1;
        self.dir_permissions
            .push((dst.to_path_buf(), meta.permissions()));
        Ok(())
    }

    fn copy_file(&mut self, src: &Path, dst: &Path) -> Result<(), CopyRefusal> {
        let bytes = std::fs::copy(src, dst).map_err(|e| write_error(&e, dst))?;
        self.stats.files += 1;
        self.stats.bytes += bytes;
        Ok(())
    }

    fn copy_link(&mut self, src: &Path, dst: &Path) -> Result<(), CopyRefusal> {
        fs_copy::copy_symlink(src, dst).map_err(|e| {
            if fs_copy::symlink_privilege_missing(&e) {
                CopyRefusal::SymlinkPrivilege(src.to_path_buf())
            } else {
                write_error(&e, dst)
            }
        })?;
        self.stats.links += 1;
        Ok(())
    }

    /// 写し終えたフォルダへ元の権限を付ける（深い方から = 読み取り専用の親が先に
    /// 付いて子へ付けられなくなる、を避ける）。付けられなくても中身は写せているので
    /// 失敗にはしない
    fn apply_dir_permissions(&mut self) {
        for (dir, permissions) in self.dir_permissions.drain(..) {
            let _ = std::fs::set_permissions(&dir, permissions);
        }
    }
}

/// 読めるかを先に確かめる（`std::fs::copy` の失敗は読み側か書き側かを区別しない）
fn probe_readable(path: &Path) -> Result<(), CopyRefusal> {
    std::fs::File::open(path)
        .map(drop)
        .map_err(|e| read_error(&e, path))
}

fn read_error(error: &std::io::Error, path: &Path) -> CopyRefusal {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => CopyRefusal::Unreadable(path.to_path_buf()),
        std::io::ErrorKind::NotFound => CopyRefusal::Io(format!(
            "コピーの途中で見つからなくなった: {}",
            path.display()
        )),
        _ => CopyRefusal::Io(format!("{}: {error}", path.display())),
    }
}

fn write_error(error: &std::io::Error, path: &Path) -> CopyRefusal {
    match error.kind() {
        std::io::ErrorKind::AlreadyExists => CopyRefusal::NameTaken,
        std::io::ErrorKind::PermissionDenied => CopyRefusal::Unwritable(path.to_path_buf()),
        std::io::ErrorKind::StorageFull => CopyRefusal::NoSpace,
        _ => CopyRefusal::Io(format!("{}: {error}", path.display())),
    }
}

/// リンクを辿らずに「その名前の何かがあるか」（壊れたリンクも「ある」）
fn exists_no_follow(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// `TAKO_1860_LEGACY=1` で**ファイルツリーがコピー / 切り取り / 貼り付けのキーを受けない**
/// （#1860 の A/B）。⌘C / ⌘X / ⌘V は #1860 の前と同じくペインへ流れ、右クリックの
/// 3 項目も出ない。CLI / MCP の `op=copy` は同じ経路のまま（画面の入口だけを外す）
pub fn tree_keys_legacy() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var("TAKO_1860_LEGACY").map(|v| v == "1") == Ok(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テストの置き場。**必ず一時 dir の中**（本物のファイルを書き換えない = #1811 の教訓）
    fn scratch(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "tako-file-copy-{}-{name}-{}",
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
            real.starts_with(&tmp) && real != tmp,
            "テストの置き場が一時 dir の外: {}",
            real.display()
        );
        real
    }

    fn touch(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn finder_ja() -> CopyNaming {
        CopyNaming::for_platform(Platform::MacOs, Lang::Ja)
    }

    fn copy(src: &Path, dest: &Path) -> Result<(CopyPlan, CopyStats), CopyRefusal> {
        let p = plan(src, dest, finder_ja())?;
        let stats = execute(&p)?;
        Ok((p, stats))
    }

    #[test]
    fn 別名はfinderとエクスプローラーの付け方に倣う() {
        let mac_ja = finder_ja();
        let mac_en = CopyNaming::for_platform(Platform::MacOs, Lang::En);
        let win_ja = CopyNaming::for_platform(Platform::Windows, Lang::Ja);
        let win_en = CopyNaming::for_platform(Platform::Windows, Lang::En);
        assert_eq!(copy_name("a.txt", false, 1, mac_ja), "a のコピー.txt");
        assert_eq!(copy_name("a.txt", false, 2, mac_ja), "a のコピー 2.txt");
        assert_eq!(copy_name("a.txt", false, 1, mac_en), "a copy.txt");
        assert_eq!(copy_name("a.txt", false, 3, mac_en), "a copy 3.txt");
        assert_eq!(copy_name("a.txt", false, 1, win_ja), "a - コピー.txt");
        assert_eq!(copy_name("a.txt", false, 2, win_ja), "a - コピー (2).txt");
        assert_eq!(copy_name("a.txt", false, 2, win_en), "a - Copy (2).txt");
        // フォルダは名前の後ろ（ドットを含んでも拡張子として割らない）
        assert_eq!(copy_name("v1.2", true, 1, mac_ja), "v1.2 のコピー");
        assert_eq!(copy_name("d", true, 2, win_en), "d - Copy (2)");
        // 隠しファイルの先頭のドット・末尾のドットは拡張子にしない
        assert_eq!(copy_name(".env", false, 1, mac_ja), ".env のコピー");
        assert_eq!(copy_name("a.", false, 1, mac_ja), "a. のコピー");
        // 拡張子は最後の 1 つ
        assert_eq!(copy_name("x.tar.gz", false, 1, mac_en), "x.tar copy.gz");
        // Finder は付いている印を外して数え直す。エクスプローラーは外さない
        assert_eq!(
            copy_name("a のコピー.txt", false, 2, mac_ja),
            "a のコピー 2.txt"
        );
        assert_eq!(
            copy_name("a のコピー 7.txt", false, 2, mac_ja),
            "a のコピー 2.txt"
        );
        assert_eq!(copy_name("のコピー", true, 1, mac_ja), "のコピー のコピー");
        assert_eq!(
            copy_name("a - コピー.txt", false, 1, win_ja),
            "a - コピー - コピー.txt"
        );
    }

    #[test]
    fn 同名が3つ以上あっても上書きせず次の空き番号に置く() {
        let base = scratch("many");
        touch(&base.join("a.txt"), "元");
        touch(&base.join("a のコピー.txt"), "1");
        touch(&base.join("a のコピー 2.txt"), "2");
        touch(&base.join("a のコピー 3.txt"), "3");
        let (p, stats) = copy(&base.join("a.txt"), &base).unwrap();
        assert!(p.renamed);
        assert_eq!(p.to, base.join("a のコピー 4.txt"));
        assert_eq!(stats.files, 1);
        assert_eq!(
            std::fs::read_to_string(base.join("a のコピー 4.txt")).unwrap(),
            "元"
        );
        for (name, body) in [
            ("a のコピー.txt", "1"),
            ("a のコピー 2.txt", "2"),
            ("a のコピー 3.txt", "3"),
        ] {
            assert_eq!(
                std::fs::read_to_string(base.join(name)).unwrap(),
                body,
                "{name} が上書きされた"
            );
        }
        // 写したものをもう一度同じ場所へ写すと「のコピー」を重ねずに数える
        let (p, _) = copy(&base.join("a のコピー 4.txt"), &base).unwrap();
        assert_eq!(p.to, base.join("a のコピー 5.txt"));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn 別のフォルダに同名が無ければ元の名前のまま() {
        let base = scratch("plain");
        touch(&base.join("src/a.txt"), "A");
        std::fs::create_dir_all(base.join("dst")).unwrap();
        let (p, _) = copy(&base.join("src/a.txt"), &base.join("dst")).unwrap();
        assert!(!p.renamed);
        assert_eq!(p.to, base.join("dst/a.txt"));
        assert_eq!(
            std::fs::read_to_string(base.join("src/a.txt")).unwrap(),
            "A"
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn フォルダは入れ子とリンクと権限ごと写る() {
        use std::os::unix::fs::PermissionsExt;
        let base = scratch("tree");
        touch(&base.join("d/a.txt"), "A");
        touch(&base.join("d/sub/deep/b.sh"), "#!/bin/sh\n");
        std::fs::create_dir_all(base.join("d/empty")).unwrap();
        std::fs::set_permissions(
            base.join("d/sub/deep/b.sh"),
            std::fs::Permissions::from_mode(0o751),
        )
        .unwrap();
        std::fs::set_permissions(base.join("d/a.txt"), std::fs::Permissions::from_mode(0o444))
            .unwrap();
        std::fs::set_permissions(base.join("d/sub"), std::fs::Permissions::from_mode(0o550))
            .unwrap();
        // 相対リンク（自分の祖先を指す = 辿ると無限に潜る）と壊れたリンク
        std::os::unix::fs::symlink("..", base.join("d/empty/up")).unwrap();
        std::os::unix::fs::symlink("nowhere", base.join("d/broken")).unwrap();
        std::fs::create_dir_all(base.join("dst")).unwrap();

        let (p, stats) = copy(&base.join("d"), &base.join("dst")).unwrap();
        assert_eq!(p.kind, EntryKind::Dir);
        let out = base.join("dst/d");
        assert_eq!(std::fs::read_to_string(out.join("a.txt")).unwrap(), "A");
        let mode = |p: &Path| std::fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&out.join("a.txt")), 0o444);
        assert_eq!(mode(&out.join("sub/deep/b.sh")), 0o751);
        assert_eq!(
            mode(&out.join("sub")),
            0o550,
            "読み取り専用のフォルダも写し終えてから"
        );
        assert!(out.join("empty").is_dir(), "空のフォルダも写る");
        assert_eq!(
            std::fs::read_link(out.join("empty/up")).unwrap(),
            PathBuf::from("..")
        );
        assert_eq!(
            std::fs::read_link(out.join("broken")).unwrap(),
            PathBuf::from("nowhere")
        );
        assert_eq!(
            stats,
            CopyStats {
                files: 2,
                dirs: 4,
                links: 2,
                bytes: 11
            }
        );
        // 後片付け（読み取り専用のフォルダは書けるように戻してから消す）
        for d in [base.join("d/sub"), out.join("sub")] {
            std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn 自分自身と配下へは実体の形で断る() {
        let base = scratch("self");
        std::fs::create_dir_all(base.join("d/sub/deep")).unwrap();
        assert_eq!(
            plan(&base.join("d"), &base.join("d"), finder_ja()),
            Err(CopyRefusal::IntoSelf)
        );
        assert_eq!(
            plan(&base.join("d"), &base.join("d/sub/deep"), finder_ja()),
            Err(CopyRefusal::IntoDescendant)
        );
        assert_eq!(
            plan(&base.join("d"), &base.join("d/sub/../sub"), finder_ja()),
            Err(CopyRefusal::IntoDescendant)
        );
        let names: Vec<_> = std::fs::read_dir(base.join("d/sub"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            names,
            vec![std::ffi::OsString::from("deep")],
            "何も作られていない"
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn コピー元が無い貼り付け先がフォルダでないを断る() {
        let base = scratch("missing");
        touch(&base.join("f.txt"), "f");
        assert_eq!(
            plan(&base.join("nope"), &base, finder_ja()),
            Err(CopyRefusal::SourceMissing)
        );
        assert_eq!(
            plan(&base.join("f.txt"), &base.join("nope"), finder_ja()),
            Err(CopyRefusal::DestNotDir)
        );
        assert_eq!(
            plan(&base.join("f.txt"), &base.join("f.txt"), finder_ja()),
            Err(CopyRefusal::DestNotDir)
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn 名前を決めた後に同名ができても上書きせずに断る() {
        let base = scratch("race");
        touch(&base.join("src/a.txt"), "新");
        std::fs::create_dir_all(base.join("src/d")).unwrap();
        std::fs::create_dir_all(base.join("dst")).unwrap();
        let p = plan(&base.join("src/a.txt"), &base.join("dst"), finder_ja()).unwrap();
        touch(&base.join("dst/a.txt"), "割り込み");
        assert_eq!(execute(&p), Err(CopyRefusal::NameTaken));
        assert_eq!(
            std::fs::read_to_string(base.join("dst/a.txt")).unwrap(),
            "割り込み",
            "割り込んだファイルを上書きしない・戻すときも消さない"
        );
        let p = plan(&base.join("src/d"), &base.join("dst"), finder_ja()).unwrap();
        touch(&base.join("dst/d/keep.txt"), "残す");
        assert_eq!(execute(&p), Err(CopyRefusal::NameTaken));
        assert!(base.join("dst/d/keep.txt").is_file());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn 読めない項目を含むフォルダは理由つきで断り何も残さない() {
        use std::os::unix::fs::PermissionsExt;
        let base = scratch("unreadable");
        touch(&base.join("d/ok.txt"), "ok");
        touch(&base.join("d/z/secret.txt"), "s");
        std::fs::create_dir_all(base.join("dst")).unwrap();
        let secret = base.join("d/z/secret.txt");
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o000)).unwrap();
        // root で走る CI では 0o000 でも読める（その回は検査の対象外）
        if std::fs::File::open(&secret).is_ok() {
            std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o644)).unwrap();
            std::fs::remove_dir_all(&base).unwrap();
            return;
        }
        let result = copy(&base.join("d"), &base.join("dst"));
        assert_eq!(
            result.map(|_| ()),
            Err(CopyRefusal::Unreadable(secret.clone()))
        );
        assert!(
            !base.join("dst/d").exists(),
            "途中まで写したものを戻していない"
        );
        assert!(CopyRefusal::Unreadable(secret.clone())
            .reason()
            .contains("secret.txt"));
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn fifoは理由つきで断り何も残さない() {
        let base = scratch("fifo");
        touch(&base.join("d/a.txt"), "a");
        std::fs::create_dir_all(base.join("dst")).unwrap();
        let fifo = base.join("d/pipe");
        let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: NUL 終端のパスを渡すだけ
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
        assert_eq!(
            copy(&base.join("d"), &base.join("dst")).map(|_| ()),
            Err(CopyRefusal::Special(fifo.clone()))
        );
        assert!(!base.join("dst/d").exists());
        // FIFO そのものも断る（開くと書き手を待って止まる）
        assert_eq!(
            plan(&fifo, &base.join("dst"), finder_ja()),
            Err(CopyRefusal::Special(fifo))
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn シンボリックリンクはリンクそのものを写す() {
        let base = scratch("link");
        std::fs::create_dir_all(base.join("real/inner")).unwrap();
        std::fs::create_dir_all(base.join("dst")).unwrap();
        std::os::unix::fs::symlink(base.join("real"), base.join("lnk")).unwrap();
        let (p, stats) = copy(&base.join("lnk"), &base.join("dst")).unwrap();
        assert_eq!(p.kind, EntryKind::Symlink);
        assert_eq!(stats.links, 1);
        assert_eq!(
            std::fs::read_link(base.join("dst/lnk")).unwrap(),
            base.join("real")
        );
        // リンクの指す先のフォルダへリンクを写すのは「配下」ではない
        let (p, _) = copy(&base.join("lnk"), &base.join("real")).unwrap();
        assert_eq!(p.to, base.join("real/lnk"));
        // 同じ場所へはリンクのまま別名で
        let (p, _) = copy(&base.join("lnk"), &base).unwrap();
        assert_eq!(p.to, base.join("lnk のコピー"));
        assert!(std::fs::symlink_metadata(&p.to)
            .unwrap()
            .file_type()
            .is_symlink());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn 日本語と空白を含むパスを写せる() {
        let base = scratch("ja");
        touch(&base.join("元 の場所/メモ 1.md"), "本文");
        std::fs::create_dir_all(base.join("貼り 先")).unwrap();
        let (p, _) = copy(&base.join("元 の場所/メモ 1.md"), &base.join("貼り 先")).unwrap();
        assert_eq!(std::fs::read_to_string(&p.to).unwrap(), "本文");
        let (p, _) = copy(&base.join("元 の場所"), &base.join("貼り 先")).unwrap();
        assert_eq!(p.to, base.join("貼り 先/元 の場所"));
        let (p, _) = copy(&base.join("元 の場所"), &base).unwrap();
        assert_eq!(p.to, base.join("元 の場所 のコピー"));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn 書き込み先の失敗を理由へ読み替える() {
        let full = std::io::Error::from(std::io::ErrorKind::StorageFull);
        assert_eq!(write_error(&full, Path::new("/x")), CopyRefusal::NoSpace);
        let taken = std::io::Error::from(std::io::ErrorKind::AlreadyExists);
        assert_eq!(write_error(&taken, Path::new("/x")), CopyRefusal::NameTaken);
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert_eq!(
            write_error(&denied, Path::new("/x")),
            CopyRefusal::Unwritable(PathBuf::from("/x"))
        );
        assert_eq!(
            read_error(&denied, Path::new("/y")),
            CopyRefusal::Unreadable(PathBuf::from("/y"))
        );
        assert!(CopyRefusal::NoSpace.reason().contains("空き容量"));
    }
}
