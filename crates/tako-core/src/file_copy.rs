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
//! 中身と権限・拡張属性（[`fs_copy::copy_file_exclusive`]。同じ APFS ボリュームなら OS の複製 =
//! clone、それ以外は macOS = `fcopyfile` / Windows = `CopyFileExW`）。シンボリックリンクは
//! **リンクそのもの**（指す先は辿らない = 自分の祖先を指すリンクで無限に潜らない。
//! Finder / `cp -R` と同じ）。FIFO・ソケット・デバイスは読むと止まる / 写す意味が無いので
//! 理由つきで断る。
//!
//! ## 進み具合と取り消し（FR-3.38 / #1867）
//!
//! 写す前に件数とバイトを数え（[`measure`]）、1 項目ごとに [`Progress`] を進める。
//! 取り消し（[`Progress::cancel`]）は**次の項目へ進む前**に見て、[`CopyRefusal::Cancelled`] で
//! 抜ける = 失敗と同じく**この呼び出しで作った置き場だけを消して戻す**（作りかけを残さない）。
//! 走っているコピーは [`jobs`] の一覧に載り、GUI のツリーの帯・
//! CLI `tako file progress` / `cancel`・MCP `tako_file_op` の `copy_progress` / `copy_cancel` が
//! 同じ一覧を読む。
//!
//! ## 1 つのファイルの途中でも止める・バイトが進む（FR-3.39 / #1895）
//!
//! ファイルの中身は OS の写す単位ごと（macOS は 1 MiB）にバイトを進め、**そのファイルの途中でも**
//! 取り消しを見て止まる（作りかけのファイルは [`fs_copy::copy_file_exclusive`] が消す）。
//! 同じ APFS ボリュームの中は先に OS の複製（clone = 中身を写さず一瞬で終わる）を試すので
//! 途中が無い。実測（256 MiB・M 系の Mac）: clone 0.1 ms / 中身を写す道は進み具合の
//! コールバックがあっても無くても同じ速さ（同じボリューム 37〜40 ms・別のボリューム 173〜195 ms）。
//! 残り時間の目安は [`eta`]（写し始めて [`ETA_MIN_ELAPSED`] 経ち、[`ETA_MIN_PERMILLE`] ‰ 以上
//! 写してから、始めからの平均の速さで出す = 始めのぶれを見せない）。

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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
    /// 取り消した（#1867。作りかけは消してある）
    Cancelled,
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
            Self::Cancelled => "cancelled",
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
            Self::Cancelled => "取り消した（作りかけは消した）".into(),
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
    execute_with(plan, &Progress::default())
}

/// 進み具合を `progress` へ載せながら写す（#1867）。取り消されたら次の項目へ進む前に
/// [`CopyRefusal::Cancelled`] で抜け、失敗と同じくこの呼び出しで作った置き場だけを消す
pub fn execute_with(plan: &CopyPlan, progress: &Progress) -> Result<CopyStats, CopyRefusal> {
    let mut job = Job {
        progress,
        // 遅延を注入した回は clone を試さない（一瞬で終わる clone には途中が無い = 進み具合と
        // 途中の取り消しを見るための注入なので、中身を写す道を通す。#1895）
        allow_clone: injected_chunk_delay().is_none() && !progress.no_clone.load(Ordering::Relaxed),
        stats: CopyStats::default(),
        created_root: false,
        dir_permissions: Vec::new(),
    };
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

struct Job<'a> {
    /// 進み具合と取り消し（#1867）
    progress: &'a Progress,
    /// 1 つのファイルを OS の複製（clone）で済ませてよいか（#1895）
    allow_clone: bool,
    stats: CopyStats,
    /// 置き場を作れた（ここから先の失敗は戻す）
    created_root: bool,
    /// 写し終えてから付けるフォルダの権限（深い方から付ける = 逆順）
    dir_permissions: Vec<(PathBuf, std::fs::Permissions)>,
}

impl Job<'_> {
    /// 次の項目へ進む前の関所（取り消しを見る。#1867）
    fn step(&self) -> Result<(), CopyRefusal> {
        injected_delay();
        if self.progress.is_cancelled() {
            return Err(CopyRefusal::Cancelled);
        }
        Ok(())
    }

    fn copy_root(&mut self, plan: &CopyPlan) -> Result<(), CopyRefusal> {
        self.step()?;
        match plan.kind {
            EntryKind::Dir => {
                std::fs::create_dir(&plan.to).map_err(|e| write_error(&e, &plan.to))?;
                self.created_root = true;
                self.progress.add_done(1, 0);
                self.copy_dir(&plan.from, &plan.to)
            }
            EntryKind::File => {
                probe_readable(&plan.from)?;
                // 置き場は `copy_file` が排他的に作る（決めてから作るまでに誰かが作っていたら
                // 上書きせずに断る）。失敗・取り消しの作りかけも `copy_file` の側で消える
                self.copy_file(&plan.from, &plan.to)?;
                self.created_root = true;
                Ok(())
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
            self.step()?;
            let entry = entry.map_err(|e| read_error(&e, src))?;
            let from = entry.path();
            let to = dst.join(entry.file_name());
            let file_type = entry.file_type().map_err(|e| read_error(&e, &from))?;
            if file_type.is_symlink() {
                self.copy_link(&from, &to)?;
            } else if file_type.is_dir() {
                std::fs::create_dir(&to).map_err(|e| write_error(&e, &to))?;
                self.progress.add_done(1, 0);
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

    /// 1 つのファイルを写す。置き場は排他的に作り（同名は上書きせず断る）、中身を写す間は
    /// バイトを進めて**そのファイルの途中でも**取り消しを見る（#1895）。失敗・取り消しの
    /// 作りかけは `fs_copy::copy_file_exclusive` が消す
    fn copy_file(&mut self, src: &Path, dst: &Path) -> Result<(), CopyRefusal> {
        if legacy_1895() {
            return self.copy_file_legacy(src, dst);
        }
        let progress = self.progress;
        let mut reported = 0u64;
        let outcome = fs_copy::copy_file_exclusive(src, dst, self.allow_clone, &mut |copied| {
            if let Some(delay) = injected_chunk_delay() {
                std::thread::sleep(delay);
            }
            if copied > reported {
                progress.add_done(0, copied - reported);
                reported = copied;
            }
            !progress.is_cancelled()
        });
        let bytes = match outcome {
            Ok((bytes, _route)) => bytes,
            Err(fs_copy::FileCopyError::Cancelled) => return Err(CopyRefusal::Cancelled),
            Err(fs_copy::FileCopyError::Io(e)) => return Err(write_error(&e, dst)),
        };
        // clone は途中が無いので、ここで一度に進める（中身を写した道は届いた残りだけ）
        if bytes > reported {
            progress.add_done(0, bytes - reported);
        }
        self.stats.files += 1;
        self.stats.bytes += bytes;
        progress.add_done(1, 0);
        Ok(())
    }

    /// `TAKO_1895_LEGACY=1` の A/B: #1895 の前の写し方（名前を `create_new` で押さえて
    /// `std::fs::copy` = 1 つのファイルの途中では止まらず、バイトは写し終えてから一度に進む。
    /// 押さえた名前の上へ写すので APFS の clone も効かない）
    fn copy_file_legacy(&mut self, src: &Path, dst: &Path) -> Result<(), CopyRefusal> {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dst)
            .map_err(|e| write_error(&e, dst))?;
        let bytes = match std::fs::copy(src, dst) {
            Ok(bytes) => bytes,
            Err(e) => {
                // 押さえた名前（この呼び出しが作ったもの）だけを消す
                let _ = std::fs::remove_file(dst);
                return Err(write_error(&e, dst));
            }
        };
        self.stats.files += 1;
        self.stats.bytes += bytes;
        self.progress.add_done(1, bytes);
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
        self.progress.add_done(1, 0);
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

// --- 進み具合と取り消し（FR-3.38 / #1867） ---------------------------------------

/// コピーの進み具合と取り消しの印。background で写す側が進め、UI スレッド（GUI の帯・
/// CLI / MCP の `copy_progress`）が読み、`copy_cancel` が印を立てる（どれも atomics だけ）
#[derive(Debug, Default)]
pub struct Progress {
    entries_total: AtomicU64,
    bytes_total: AtomicU64,
    entries_done: AtomicU64,
    bytes_done: AtomicU64,
    /// 数え終えた（[`measure`] が済んだ）
    counted: AtomicBool,
    cancel: AtomicBool,
    /// 写した件数がこれに達したら取り消したことにする（0 = 使わない）。単体テストが途中の
    /// 取り消しを決定的に起こすための印で、本番の経路は立てない
    cancel_at: AtomicU64,
    /// 写したバイトがこれに達したら取り消したことにする（0 = 使わない。#1895 の単体テストが
    /// 1 つのファイルの途中の取り消しを決定的に起こすための印。本番の経路は立てない）
    cancel_at_bytes: AtomicU64,
    /// OS の複製（clone）を試さず中身を写す（#1895 の単体テストが同じ APFS の一時 dir の中で
    /// 「1 つのファイルの途中」を作るための印。本番の経路は立てない）
    no_clone: AtomicBool,
    /// 数え終えた時刻（残り時間の目安の速さは、ここからの平均で測る。#1895）
    counted_at: std::sync::OnceLock<std::time::Instant>,
    /// 最後にバイトが進んだ時刻（数え終えてからのナノ秒。0 = まだ進んでいない。#1908）
    bytes_at_nanos: AtomicU64,
    /// バイトが進んだ回数（写す単位ごと = 止まったと見なすまでの目安。#1908）
    advances: AtomicU64,
}

/// ある時点の進み具合（応答・画面に載せる値）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProgressSnapshot {
    pub entries_done: u64,
    pub entries_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// まだ数えている（`*_total` は数えたところまで）
    pub counting: bool,
    /// 取り消しを受けた（写している 1 件の途中でも止まる。#1895）
    pub cancelled: bool,
    /// 数え終えてから（= 写し始めてから）の時間。数えている間は 0（#1895 の残り時間の目安）
    pub copying_for: std::time::Duration,
    /// 最後にバイトが進んだ時点（数え終えてからの時間。まだ進んでいなければ 0。#1908）
    pub bytes_at: std::time::Duration,
    /// バイトが進んだ回数（#1908）
    pub advances: u64,
}

impl Progress {
    /// 取り消す（次の項目へ進む前に止まり、作りかけを消す）
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        let at = self.cancel_at.load(Ordering::Relaxed);
        if at > 0 && self.entries_done.load(Ordering::Relaxed) >= at {
            return true;
        }
        let at_bytes = self.cancel_at_bytes.load(Ordering::Relaxed);
        if at_bytes > 0 && self.bytes_done.load(Ordering::Relaxed) >= at_bytes {
            return true;
        }
        self.cancel.load(Ordering::Relaxed)
    }

    /// 数え終えた印（ここから先の `*_total` は確定）
    pub fn finish_counting(&self) {
        let _ = self.counted_at.set(std::time::Instant::now());
        self.counted.store(true, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> ProgressSnapshot {
        // 進んだ時刻は「いま」（`copying_for`）より先に読む（後に読むと、間に届いた進みで
        // 進んだ時刻がいまを越え、#1908 の見積もりがその 1 回だけ旧式へ落ちる）
        let bytes_at = std::time::Duration::from_nanos(self.bytes_at_nanos.load(Ordering::Relaxed));
        ProgressSnapshot {
            entries_done: self.entries_done.load(Ordering::Relaxed),
            entries_total: self.entries_total.load(Ordering::Relaxed),
            bytes_done: self.bytes_done.load(Ordering::Relaxed),
            bytes_total: self.bytes_total.load(Ordering::Relaxed),
            counting: !self.counted.load(Ordering::Relaxed),
            cancelled: self.is_cancelled(),
            copying_for: self
                .counted_at
                .get()
                .map(std::time::Instant::elapsed)
                .unwrap_or_default(),
            bytes_at,
            advances: self.advances.load(Ordering::Relaxed),
        }
    }

    fn add_total(&self, entries: u64, bytes: u64) {
        self.entries_total.fetch_add(entries, Ordering::Relaxed);
        self.bytes_total.fetch_add(bytes, Ordering::Relaxed);
    }

    fn add_done(&self, entries: u64, bytes: u64) {
        self.entries_done.fetch_add(entries, Ordering::Relaxed);
        self.bytes_done.fetch_add(bytes, Ordering::Relaxed);
        if bytes > 0 {
            // 進んだ時刻を残す（#1908 の残り時間は、この時刻までの平均の速さで見積もる）
            if let Some(start) = self.counted_at.get() {
                let nanos = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
                self.bytes_at_nanos.store(nanos, Ordering::Relaxed);
            }
            self.advances.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// 写す前に `src` の件数とバイトを数えて `progress` の合計へ足す（[`execute_with`] が数えるのと
/// 同じ単位 = ファイル・フォルダ・リンクを 1 件、バイトはファイルの中身だけ）。
///
/// リンクは辿らない（祖先を指すリンクで潜らない）。読めないところは数えずに進む
/// （断るのは写すときの [`execute_with`] = 理由の正本は 1 つ）。取り消されたら Cancelled
pub fn measure(src: &Path, progress: &Progress) -> Result<(), CopyRefusal> {
    if progress.is_cancelled() {
        return Err(CopyRefusal::Cancelled);
    }
    let Ok(meta) = std::fs::symlink_metadata(src) else {
        return Ok(());
    };
    let file_type = meta.file_type();
    if file_type.is_symlink() {
        progress.add_total(1, 0);
    } else if file_type.is_file() {
        progress.add_total(1, meta.len());
    } else if file_type.is_dir() {
        progress.add_total(1, 0);
        if let Ok(entries) = std::fs::read_dir(src) {
            for entry in entries.flatten() {
                measure(&entry.path(), progress)?;
            }
        }
    }
    Ok(())
}

// --- 残り時間の目安（FR-3.39 / #1895） ----------------------------------------------

/// 残り時間の目安を出し始めるまでの写した時間。始めの数秒は速さがぶれる（読み先が
/// キャッシュに乗っている・小さなファイルが続く・clone が一瞬で終わる）ので見せない
pub const ETA_MIN_ELAPSED: std::time::Duration = std::time::Duration::from_secs(2);
/// 残り時間の目安を出し始めるまでに写したバイトの割合（千分率 = 1%）
pub const ETA_MIN_PERMILLE: u64 = 10;

/// 止まったと見なすまでの時間の下限（#1908）。これより短い途切れは数え下ろしを続ける
pub const ETA_STALL_MIN: std::time::Duration = std::time::Duration::from_secs(2);
/// 止まったと見なすまでの時間 = 進みの平均の間隔のこの倍（#1908。遅い媒体で 1 回ごとの
/// 間隔が長くても、ふだんの間隔のうちは止まったと見なさない）
pub const ETA_STALL_FACTOR: f64 = 3.0;

/// 残り時間の目安（GUI の帯・CLI / MCP の `copy_progress` の `eta_secs` が同じものを読む）。
///
/// 写し始めてから（数え終えてから）の**平均の速さ**で残りのバイトを割る（直近の速さで割ると
/// 1 チャンクごとに揺れる）。出さないとき: 数えている・取り消した・バイトが無い（空のフォルダと
/// リンクだけ）・写し終えた・写し始めて [`ETA_MIN_ELAPSED`] 経っていない・
/// [`ETA_MIN_PERMILLE`] ‰ 写していない。
///
/// **#1908: 平均の速さは最後にバイトが進んだ時点までで測り、そこから見積もった「写し終える
/// 時刻」までを数え下ろす**。#1895 は「いま」までの平均で割っていたので、次の進み（macOS は
/// 1 MiB ごと）が届くまでの間は分母の時間だけが伸びて見積もりが増え、届いた瞬間に減る =
/// 一定の速さでも 1 チャンクごとにのこぎり状に揺れ、帯の表記が前後の粒度を行き来した。
/// 数え下ろしは一定の速さなら単調に減り、GUI・CLI・MCP が状態を持たずに同じ値を読める。
/// 進みが途切れて [`stall_after`] を越えたら（止まった）、越えた分だけ #1895 と同じ伸び方
/// （残りのバイト ÷ 写したバイトの割合）で見積もりを後ろへずらす（つなぎ目で値が飛ばない）
pub fn eta(snap: &ProgressSnapshot) -> Option<std::time::Duration> {
    if snap.counting
        || snap.cancelled
        || snap.bytes_total == 0
        || snap.bytes_done == 0
        || snap.bytes_done >= snap.bytes_total
        || snap.copying_for < ETA_MIN_ELAPSED
    {
        return None;
    }
    if u128::from(snap.bytes_done) * 1000
        < u128::from(snap.bytes_total) * u128::from(ETA_MIN_PERMILLE)
    {
        return None;
    }
    let left = estimate(snap, legacy_1908());
    // 何日もかかる見積もりは見せる意味が無い（帯は時間までしか書かない）
    (left.is_finite() && left < 100.0 * 3600.0).then(|| std::time::Duration::from_secs_f64(left))
}

/// [`eta`] の見積もりの式（秒・0 以上。出すかどうかは [`eta`] が決める）。`legacy` = #1908 の
/// 前（「いま」までの平均の速さで割る）。単体テストが同じ入力で新旧の振れ幅を比べるために
/// 分けてある（外からは [`eta`] だけを呼ぶ）
fn estimate(snap: &ProgressSnapshot, legacy: bool) -> f64 {
    let now = snap.copying_for.as_secs_f64();
    let at = snap.bytes_at.as_secs_f64();
    let done = snap.bytes_done as f64;
    // 残りのバイト ÷ 写したバイト（平均の速さで割ると、残り時間 = この比 × 測った時間）
    let ratio = (snap.bytes_total - snap.bytes_done) as f64 / done;
    let left = if legacy || at <= 0.0 || at > now {
        // #1895: いままでの平均の速さで割る（進みの時刻が無いときもこれ）
        ratio * now
    } else {
        // 最後の進みから写し終えるまで（その時点までの平均の速さ）から、経った分を引く
        let base = ratio * at;
        let idle = now - at;
        let stall = stall_after(snap).as_secs_f64();
        if idle <= stall {
            base - idle
        } else {
            // 止まっている: 越えた分だけ #1895 と同じ伸び方で後ろへずらす
            base - stall + (idle - stall) * ratio
        }
    };
    left.max(0.0)
}

/// 進みが途切れて止まったと見なすまでの時間（#1908）: 進みの平均の間隔の
/// [`ETA_STALL_FACTOR`] 倍と [`ETA_STALL_MIN`] の大きい方
pub fn stall_after(snap: &ProgressSnapshot) -> std::time::Duration {
    let interval = if snap.advances == 0 {
        std::time::Duration::ZERO
    } else {
        snap.bytes_at / u32::try_from(snap.advances).unwrap_or(u32::MAX)
    };
    interval.mul_f64(ETA_STALL_FACTOR).max(ETA_STALL_MIN)
}

/// 帯に書く残り時間（[`eta`] を粗く丸めたもの。150 ms ごとに描き直してもちらつかない粒度）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EtaLabel {
    /// 10 秒未満
    Soon,
    /// 1 分未満（5 秒単位へ切り上げ）
    Seconds(u64),
    /// 1 時間未満（分へ四捨五入・最低 1 分）
    Minutes(u64),
    /// 1 時間以上（10 分単位へ切り上げ）
    Hours { hours: u64, minutes: u64 },
}

/// [`eta`] の値を帯に書く粒度へ丸める
pub fn eta_label(left: std::time::Duration) -> EtaLabel {
    let secs = left.as_secs_f64().ceil() as u64;
    if secs < 10 {
        return EtaLabel::Soon;
    }
    if secs < 60 {
        let up = secs.div_ceil(5) * 5;
        return if up < 60 {
            EtaLabel::Seconds(up)
        } else {
            EtaLabel::Minutes(1)
        };
    }
    if secs < 3600 {
        let minutes = ((secs + 30) / 60).max(1);
        return if minutes < 60 {
            EtaLabel::Minutes(minutes)
        } else {
            EtaLabel::Hours {
                hours: 1,
                minutes: 0,
            }
        };
    }
    let tens = secs.div_ceil(600);
    EtaLabel::Hours {
        hours: tens / 6,
        minutes: (tens % 6) * 10,
    }
}

/// 走っているコピー 1 つ（[`jobs`] の一覧に載る）
#[derive(Debug)]
pub struct CopyTicket {
    /// 一覧の中で一意の番号（`copy_cancel` の宛先）
    pub id: u64,
    pub progress: Progress,
    /// 写すもの
    pub sources: Vec<PathBuf>,
    /// 貼り付け先のフォルダ（1 つ目のもの）
    pub dest: PathBuf,
    started: std::time::Instant,
}

impl CopyTicket {
    /// 始めてからの時間（GUI は短いコピーで帯をちらつかせないよう、これで出し始めを遅らせる）
    pub fn elapsed(&self) -> std::time::Duration {
        self.started.elapsed()
    }
}

/// 走っているコピーの一覧（GUI・CLI・MCP が同じものを読む）
#[derive(Debug)]
pub struct CopyJobs {
    next: AtomicU64,
    live: Mutex<Vec<Arc<CopyTicket>>>,
}

impl Default for CopyJobs {
    fn default() -> Self {
        Self::new()
    }
}

impl CopyJobs {
    pub const fn new() -> Self {
        Self {
            next: AtomicU64::new(1),
            live: Mutex::new(Vec::new()),
        }
    }

    /// 一覧へ載せる。返した札を落とすと一覧から外れる（写し終えた・断った・取り消した、の
    /// どれでも外し忘れない）
    pub fn register(&self, sources: Vec<PathBuf>, dest: PathBuf) -> TicketGuard<'_> {
        let ticket = Arc::new(CopyTicket {
            id: self.next.fetch_add(1, Ordering::Relaxed),
            progress: Progress::default(),
            sources,
            dest,
            started: std::time::Instant::now(),
        });
        self.lock().push(Arc::clone(&ticket));
        TicketGuard { jobs: self, ticket }
    }

    /// 走っているコピー（始めた順）
    pub fn list(&self) -> Vec<Arc<CopyTicket>> {
        self.lock().clone()
    }

    /// 取り消す（`id` 省略 = 走っているものすべて）。印を立てた番号を返す
    pub fn cancel(&self, id: Option<u64>) -> Vec<u64> {
        let mut out = Vec::new();
        for ticket in self.lock().iter() {
            if id.is_none_or(|id| id == ticket.id) {
                ticket.progress.cancel();
                out.push(ticket.id);
            }
        }
        out
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Arc<CopyTicket>>> {
        // 一覧の読み書きで panic しない（持ち主が panic しても一覧そのものは壊れていない）
        self.live.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// 一覧に載せた札。落とすと一覧から外れる
#[derive(Debug)]
pub struct TicketGuard<'a> {
    jobs: &'a CopyJobs,
    ticket: Arc<CopyTicket>,
}

impl TicketGuard<'_> {
    pub fn ticket(&self) -> &CopyTicket {
        &self.ticket
    }

    /// 写す側が進める進み具合
    pub fn progress(&self) -> &Progress {
        &self.ticket.progress
    }
}

impl Drop for TicketGuard<'_> {
    fn drop(&mut self) {
        let id = self.ticket.id;
        self.jobs.lock().retain(|t| t.id != id);
    }
}

/// プロセスに 1 つの一覧（GUI のツリーの帯・dispatch の `copy_progress` / `copy_cancel`）
pub fn jobs() -> &'static CopyJobs {
    static JOBS: CopyJobs = CopyJobs::new();
    &JOBS
}

/// 検証用の遅延（`TAKO_1867_COPY_DELAY_MS=<ミリ秒>` で 1 項目ごとに待つ）。進み具合の帯と
/// 取り消しを**小さな fixture で**見るためのもの（大きなファイルを作る・CPU を焼く負荷で
/// 遅くしない）。未設定なら何もしない
fn injected_delay() {
    static DELAY: std::sync::OnceLock<Option<std::time::Duration>> = std::sync::OnceLock::new();
    let delay = DELAY.get_or_init(|| {
        std::env::var("TAKO_1867_COPY_DELAY_MS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|ms| *ms > 0)
            .map(std::time::Duration::from_millis)
    });
    if let Some(delay) = delay {
        std::thread::sleep(*delay);
    }
}

/// 検証用の遅延（`TAKO_1895_COPY_CHUNK_DELAY_MS=<ミリ秒>` で、1 つのファイルの中身を写す単位
/// （macOS は 1 MiB）ごとに待つ）。**1 つのファイルの途中**の進み具合と取り消しを、数十 MiB の
/// fixture で見るためのもの（数 GB のファイル・CPU を焼く負荷で遅くしない）。立てた回は
/// clone を試さない（一瞬で終わる clone には途中が無い）。未設定なら None
fn injected_chunk_delay() -> Option<std::time::Duration> {
    static DELAY: std::sync::OnceLock<Option<std::time::Duration>> = std::sync::OnceLock::new();
    *DELAY.get_or_init(|| {
        std::env::var("TAKO_1895_COPY_CHUNK_DELAY_MS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|ms| *ms > 0)
            .map(std::time::Duration::from_millis)
    })
}

/// `TAKO_1895_LEGACY=1` で **#1895 の前**へ戻す（同一バイナリの A/B）: 1 つのファイルは
/// `create_new` + `std::fs::copy` で写す（途中で止まらない・バイトは写し終えてから進む・clone も
/// 効かない）・ツリーの ⇧↑ / ⇧↓ / ⌘⌫（Windows は Delete）を受けない・帯に残り時間を出さない・
/// CLI `tako file copy a b dst` は 1 件ずつ別の要求（別のジョブ）で送る
pub fn legacy_1895() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var("TAKO_1895_LEGACY").map(|v| v == "1") == Ok(true))
}

/// `TAKO_1908_LEGACY=1` で **#1908 の前**へ戻す（同一バイナリの A/B）: ツリーの ↑ / ↓ /
/// ← / → / Enter / ⇧⌘↑ / ⇧⌘↓（Windows は Shift+Ctrl+Home / End）を受けない（ペインへ流れる）・
/// 残り時間は「いま」までの平均の速さで割る（1 チャンクごとにのこぎり状に揺れる）。
/// CLI / MCP の `tako tree selection` / `tako_tree_folder` の `selection` は同じ経路のまま
/// （画面の入口だけを外す）
pub fn legacy_1908() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var("TAKO_1908_LEGACY").map(|v| v == "1") == Ok(true))
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

    /// `allow_clone` が偽なら clone を試さず中身を写す道を通す（#1895）
    fn routed(
        plan: &CopyPlan,
        progress: &Progress,
        allow_clone: bool,
    ) -> Result<CopyStats, CopyRefusal> {
        progress.no_clone.store(!allow_clone, Ordering::Relaxed);
        execute_with(plan, progress)
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
    fn 数えた件数とバイトは写した量と一致し進み具合が満ちる() {
        let base = scratch("progress");
        touch(&base.join("d/a.txt"), "aaaa");
        touch(&base.join("d/sub/b.txt"), "bb");
        std::fs::create_dir_all(base.join("d/empty")).unwrap();
        std::fs::create_dir_all(base.join("dst")).unwrap();
        let progress = Progress::default();
        measure(&base.join("d"), &progress).unwrap();
        let counted = progress.snapshot();
        assert!(counted.counting, "数え終えた印を付けるまでは数えている");
        progress.finish_counting();
        // d / a.txt / sub / sub/b.txt / empty = 5 件、中身は 6 バイト
        assert_eq!(
            (counted.entries_total, counted.bytes_total),
            (5, 6),
            "{counted:?}"
        );
        assert_eq!((counted.entries_done, counted.bytes_done), (0, 0));
        let p = plan(&base.join("d"), &base.join("dst"), finder_ja()).unwrap();
        let stats = execute_with(&p, &progress).unwrap();
        let done = progress.snapshot();
        assert_eq!(done.entries_done, stats.entries());
        assert_eq!(done.bytes_done, stats.bytes);
        assert_eq!(
            (done.entries_done, done.bytes_done),
            (done.entries_total, done.bytes_total),
            "写し終えたら満ちる: {done:?}"
        );
        assert!(!done.counting && !done.cancelled);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn 取り消すと作りかけを消してコピー元は残る() {
        let base = scratch("cancel");
        for i in 0..5 {
            touch(&base.join(format!("d/f{i}.txt")), "x");
        }
        std::fs::create_dir_all(base.join("dst")).unwrap();
        let p = plan(&base.join("d"), &base.join("dst"), finder_ja()).unwrap();
        // 始める前に取り消してある = 置き場も作らない
        let progress = Progress::default();
        progress.cancel();
        assert_eq!(execute_with(&p, &progress), Err(CopyRefusal::Cancelled));
        assert!(!base.join("dst/d").exists());
        assert_eq!(
            measure(&base.join("d"), &progress),
            Err(CopyRefusal::Cancelled)
        );
        // 写している途中で取り消す（フォルダ + 2 件写したところで印が立つ）
        let progress = Progress::default();
        progress.cancel_at.store(3, Ordering::Relaxed);
        assert_eq!(execute_with(&p, &progress), Err(CopyRefusal::Cancelled));
        assert_eq!(
            progress.snapshot().entries_done,
            3,
            "途中まで写してから止まった"
        );
        assert!(
            !base.join("dst/d").exists(),
            "取り消したのに作りかけが残っている"
        );
        assert!(progress.snapshot().cancelled);
        for i in 0..5 {
            assert!(
                base.join(format!("d/f{i}.txt")).is_file(),
                "コピー元が消えた"
            );
        }
        assert_eq!(CopyRefusal::Cancelled.slug(), "cancelled");
        assert!(CopyRefusal::Cancelled.reason().contains("取り消した"));
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// 中身のあるファイル（`mib` MiB。同じ値が並ばないよう位置を混ぜる = 疎なファイルにしない）
    fn big_file(path: &Path, mib: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut body = vec![0u8; mib << 20];
        for (i, b) in body.iter_mut().enumerate() {
            *b = (i.wrapping_mul(2_654_435_761) >> 13) as u8;
        }
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn 一つのファイルの途中で取り消すと作りかけを残さずバイトは途中まで進んでいる() {
        let base = scratch("cancel-midfile");
        big_file(&base.join("big.bin"), 8);
        std::fs::create_dir_all(base.join("dst")).unwrap();
        let total = 8u64 << 20;
        let p = plan(&base.join("big.bin"), &base.join("dst"), finder_ja()).unwrap();
        let progress = Progress::default();
        measure(&base.join("big.bin"), &progress).unwrap();
        progress.finish_counting();
        // 2 MiB 写したところで取り消しの印が立つ（clone を試さず中身を写す道）
        progress.cancel_at_bytes.store(2 << 20, Ordering::Relaxed);
        assert_eq!(routed(&p, &progress, false), Err(CopyRefusal::Cancelled));
        let snap = progress.snapshot();
        assert!(
            !base.join("dst/big.bin").exists(),
            "1 つのファイルの途中で取り消したのに作りかけが残っている"
        );
        assert_eq!(snap.entries_done, 0, "写し終えたことになっていない");
        assert!(snap.cancelled);
        // macOS は 1 MiB ごとに届く = 途中で止まる。Windows は CopyFileExW の単位しだい
        // （止まり・作りかけを消すことは両 OS で見る）
        if cfg!(target_os = "macos") {
            assert!(
                snap.bytes_done >= 2 << 20 && snap.bytes_done < total,
                "ファイルの途中で止まっていない: {snap:?}"
            );
        }
        assert_eq!(
            std::fs::metadata(base.join("big.bin")).unwrap().len(),
            total,
            "コピー元が欠けた"
        );
        // 取り消した直後にもう一度写すと最後まで写る（同じ名前が空いている = 作りかけが無い）
        let again = Progress::default();
        let stats = routed(&p, &again, false).unwrap();
        assert_eq!(stats.bytes, total);
        assert_eq!(
            std::fs::read(base.join("dst/big.bin")).unwrap(),
            std::fs::read(base.join("big.bin")).unwrap()
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn 中身を写す道はバイトが単調に増えて写した量で満ちる() {
        let base = scratch("bytes-monotonic");
        big_file(&base.join("d/big.bin"), 4);
        touch(&base.join("d/small.txt"), "small");
        std::fs::create_dir_all(base.join("dst")).unwrap();
        let p = plan(&base.join("d"), &base.join("dst"), finder_ja()).unwrap();
        let progress = std::sync::Arc::new(Progress::default());
        measure(&base.join("d"), &progress).unwrap();
        progress.finish_counting();
        // 写している間の bytes_done を別のスレッドから読み続ける（減ったら単調でない）
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let watcher = {
            let progress = std::sync::Arc::clone(&progress);
            let stop = std::sync::Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut seen = Vec::new();
                while !stop.load(Ordering::Relaxed) {
                    seen.push(progress.snapshot().bytes_done);
                    std::thread::yield_now();
                }
                seen
            })
        };
        let stats = routed(&p, &progress, false).unwrap();
        stop.store(true, Ordering::Relaxed);
        let seen = watcher.join().unwrap();
        assert!(
            seen.windows(2).all(|w| w[0] <= w[1]),
            "バイトの進み具合が減った: {:?}",
            seen.windows(2).find(|w| w[0] > w[1])
        );
        let done = progress.snapshot();
        assert_eq!(done.bytes_done, stats.bytes);
        assert_eq!(
            (done.bytes_done, done.entries_done),
            (done.bytes_total, done.entries_total)
        );
        assert_eq!(
            std::fs::read(base.join("dst/d/big.bin")).unwrap(),
            std::fs::read(base.join("d/big.bin")).unwrap()
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn 写す先に同名ができていたらどちらの道でも上書きしない() {
        let base = scratch("exclusive");
        big_file(&base.join("src/a.bin"), 1);
        std::fs::create_dir_all(base.join("dst")).unwrap();
        for allow_clone in [true, false] {
            let p = plan(&base.join("src/a.bin"), &base.join("dst"), finder_ja()).unwrap();
            touch(&base.join("dst/a.bin"), "割り込み");
            assert_eq!(
                routed(&p, &Progress::default(), allow_clone),
                Err(CopyRefusal::NameTaken),
                "allow_clone={allow_clone}"
            );
            assert_eq!(
                std::fs::read_to_string(base.join("dst/a.bin")).unwrap(),
                "割り込み",
                "割り込んだファイルを上書き・削除した（allow_clone={allow_clone}）"
            );
            std::fs::remove_file(base.join("dst/a.bin")).unwrap();
        }
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn 空のファイルはどちらの道でも写り進み具合が満ちる() {
        let base = scratch("empty-file");
        touch(&base.join("src/empty.txt"), "");
        std::fs::create_dir_all(base.join("dst")).unwrap();
        for (allow_clone, dir) in [(true, "dst"), (false, "dst2")] {
            std::fs::create_dir_all(base.join(dir)).unwrap();
            let p = plan(&base.join("src/empty.txt"), &base.join(dir), finder_ja()).unwrap();
            let progress = Progress::default();
            measure(&base.join("src/empty.txt"), &progress).unwrap();
            progress.finish_counting();
            let stats = routed(&p, &progress, allow_clone).unwrap();
            assert_eq!((stats.files, stats.bytes), (1, 0));
            let snap = progress.snapshot();
            assert_eq!((snap.entries_done, snap.bytes_done), (1, 0));
            assert_eq!(
                std::fs::read(base.join(dir).join("empty.txt")).unwrap(),
                b""
            );
        }
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn 同じapfsボリュームの中はcloneで一瞬で写り属性も写る() {
        use std::os::unix::fs::PermissionsExt;
        let base = scratch("clone");
        big_file(&base.join("src/a.bin"), 2);
        std::fs::set_permissions(
            base.join("src/a.bin"),
            std::fs::Permissions::from_mode(0o640),
        )
        .unwrap();
        std::fs::create_dir_all(base.join("dst")).unwrap();
        let mut calls = 0;
        let (bytes, route) = fs_copy::copy_file_exclusive(
            &base.join("src/a.bin"),
            &base.join("dst/a.bin"),
            true,
            &mut |_| {
                calls += 1;
                true
            },
        )
        .unwrap();
        // 一時 dir が APFS でない環境（CI の一部）では中身を写す道へ落ちる = どちらでも中身は同じ
        assert_eq!(bytes, 2 << 20);
        if route == fs_copy::FileRoute::Clone {
            assert_eq!(calls, 0, "clone は途中が無い");
        }
        assert_eq!(
            std::fs::read(base.join("dst/a.bin")).unwrap(),
            std::fs::read(base.join("src/a.bin")).unwrap()
        );
        let mode = std::fs::metadata(base.join("dst/a.bin"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o640, "権限が写っていない（route={route:?}）");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn 写すバイトの口は累計を渡し偽で止めると作りかけを消す() {
        let base = scratch("fs-copy-data");
        big_file(&base.join("a.bin"), 6);
        let mut seen = Vec::new();
        let (bytes, route) = fs_copy::copy_file_exclusive(
            &base.join("a.bin"),
            &base.join("b.bin"),
            false,
            &mut |copied| {
                seen.push(copied);
                true
            },
        )
        .unwrap();
        assert_eq!((bytes, route), (6 << 20, fs_copy::FileRoute::Data));
        assert!(!seen.is_empty(), "進み具合が 1 度も届かない");
        assert!(
            seen.windows(2).all(|w| w[0] <= w[1]),
            "累計が減った: {seen:?}"
        );
        assert_eq!(
            seen.last().copied(),
            Some(6 << 20),
            "最後の累計が写した量と違う: {seen:?}"
        );
        // 1 回目の報告で止める
        let mut calls = 0;
        let stopped = fs_copy::copy_file_exclusive(
            &base.join("a.bin"),
            &base.join("c.bin"),
            false,
            &mut |_| {
                calls += 1;
                false
            },
        );
        assert!(
            matches!(stopped, Err(fs_copy::FileCopyError::Cancelled)),
            "{stopped:?}"
        );
        assert_eq!(calls, 1, "止めた後も進み具合が届いた");
        assert!(
            !base.join("c.bin").exists(),
            "止めたのに作りかけが残っている"
        );
        // 写す先が既にある = 断って触らない
        touch(&base.join("d.bin"), "keep");
        let taken = fs_copy::copy_file_exclusive(
            &base.join("a.bin"),
            &base.join("d.bin"),
            false,
            &mut |_| true,
        );
        match taken {
            Err(fs_copy::FileCopyError::Io(e)) => {
                assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists)
            }
            other => panic!("同名を断っていない: {other:?}"),
        }
        assert_eq!(std::fs::read_to_string(base.join("d.bin")).unwrap(), "keep");
        // コピー元が無い = 何も作らない
        let missing = fs_copy::copy_file_exclusive(
            &base.join("nope"),
            &base.join("e.bin"),
            true,
            &mut |_| true,
        );
        assert!(matches!(missing, Err(fs_copy::FileCopyError::Io(_))));
        assert!(!base.join("e.bin").exists());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn 書けないフォルダへは途中を作らず理由つきで断る() {
        use std::os::unix::fs::PermissionsExt;
        let base = scratch("unwritable-file");
        big_file(&base.join("a.bin"), 1);
        std::fs::create_dir_all(base.join("locked")).unwrap();
        std::fs::set_permissions(base.join("locked"), std::fs::Permissions::from_mode(0o555))
            .unwrap();
        // root で走る CI では書けてしまう（その回は検査の対象外）
        if std::fs::write(base.join("locked/probe"), "x").is_ok() {
            std::fs::set_permissions(base.join("locked"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
            std::fs::remove_dir_all(&base).unwrap();
            return;
        }
        for allow_clone in [true, false] {
            let p = plan(&base.join("a.bin"), &base.join("locked"), finder_ja()).unwrap();
            assert_eq!(
                routed(&p, &Progress::default(), allow_clone),
                Err(CopyRefusal::Unwritable(base.join("locked/a.bin"))),
                "allow_clone={allow_clone}"
            );
        }
        std::fs::set_permissions(base.join("locked"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        assert_eq!(std::fs::read_dir(base.join("locked")).unwrap().count(), 0);
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// いま進んだばかりの時点（進んだ時刻 = いま = #1895 の式と同じ値になる）
    fn snap(done: u64, total: u64, secs: f64) -> ProgressSnapshot {
        ProgressSnapshot {
            entries_done: 1,
            entries_total: 2,
            bytes_done: done,
            bytes_total: total,
            counting: false,
            cancelled: false,
            copying_for: std::time::Duration::from_secs_f64(secs),
            bytes_at: std::time::Duration::from_secs_f64(secs),
            advances: 1,
        }
    }

    /// 帯の表記を秒の大小で並べる（Soon < 10 秒 < … < 1 分 < …）
    fn label_rank(label: EtaLabel) -> u64 {
        match label {
            EtaLabel::Soon => 0,
            EtaLabel::Seconds(s) => s,
            EtaLabel::Minutes(m) => m * 60,
            EtaLabel::Hours { hours, minutes } => hours * 3600 + minutes * 60,
        }
    }

    /// 合成したコピー（`arrivals` = 1 MiB ずつ進んだ時刻）を 150 ms ごとに読んだときの、
    /// 見積もりの振れ（逆戻りの最大・逆戻りの合計・帯の表記が戻った回数・真の残りとの差の最大）
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Swing {
        max_rise: f64,
        total_rise: f64,
        label_back: usize,
        max_err: f64,
        samples: usize,
    }

    fn swing(arrivals: &[f64], finish: f64, legacy: bool) -> Swing {
        const MIB: u64 = 1 << 20;
        let total = arrivals.len() as u64 * MIB;
        let mut prev: Option<(f64, u64)> = None;
        let mut out = Swing {
            max_rise: 0.0,
            total_rise: 0.0,
            label_back: 0,
            max_err: 0.0,
            samples: 0,
        };
        // 読む時刻は進みの時刻とずらす（帯の刻みと写す単位は揃っていない）
        let mut now = 0.07;
        while now < finish {
            let done = arrivals.iter().filter(|t| **t <= now).count() as u64;
            let at = arrivals
                .iter()
                .copied()
                .filter(|t| *t <= now)
                .fold(0.0, f64::max);
            let snap = ProgressSnapshot {
                entries_done: 0,
                entries_total: 1,
                bytes_done: done * MIB,
                bytes_total: total,
                counting: false,
                cancelled: false,
                copying_for: std::time::Duration::from_secs_f64(now),
                bytes_at: std::time::Duration::from_secs_f64(at),
                advances: done,
            };
            // 出すかどうかは本物の `eta` で決め（新旧で同じ門）、値は新旧の式で比べる
            if eta(&snap).is_some() {
                let secs = estimate(&snap, legacy);
                let rank = label_rank(eta_label(std::time::Duration::from_secs_f64(secs)));
                if let Some((p, prank)) = prev {
                    let rise = secs - p;
                    if rise > 1e-9 {
                        out.max_rise = out.max_rise.max(rise);
                        out.total_rise += rise;
                    }
                    if rank > prank {
                        out.label_back += 1;
                    }
                }
                out.max_err = out.max_err.max((secs - (finish - now)).abs());
                out.samples += 1;
                prev = Some((secs, rank));
            }
            now += 0.15;
        }
        out
    }

    #[test]
    fn 一定の速さのコピーでは残り時間が単調に減り旧式より振れない() {
        // 40 MiB を 1 MiB ずつ 300 ms ごと（`TAKO_1895_COPY_CHUNK_DELAY_MS=300` と同じ形）= 12 秒
        let arrivals: Vec<f64> = (1..=40).map(|k| k as f64 * 0.3).collect();
        let new = swing(&arrivals, 12.0, false);
        let old = swing(&arrivals, 12.0, true);
        println!("ETA_SWING constant new={new:?} old={old:?}");
        assert!(
            new.samples > 50 && old.samples == new.samples,
            "{new:?} {old:?}"
        );
        assert_eq!(new.max_rise, 0.0, "一定の速さなら一度も増えない: {new:?}");
        assert_eq!(new.label_back, 0, "帯の表記が前の粒度へ戻らない: {new:?}");
        assert!(
            new.max_err < 1e-6,
            "一定の速さなら真の残りと一致する: {new:?}"
        );
        // #1895 の式は 1 チャンクごとに増えて減り、帯の表記も行き来する（比べる相手が揺れている）
        assert!(old.max_rise > 0.3 && old.label_back > 0, "{old:?}");
    }

    #[test]
    fn 速さが揺れても旧式より振れ幅が小さい() {
        // 300 ms ± 30% の決まった並びで揺らす（乱数を使わない = 毎回同じ）
        let mut t = 0.0;
        let arrivals: Vec<f64> = (0..60)
            .map(|k| {
                t += 0.3 * (1.0 + 0.3 * ((k * 7 % 11) as f64 / 5.0 - 1.0));
                t
            })
            .collect();
        let finish = *arrivals.last().unwrap();
        let new = swing(&arrivals, finish, false);
        let old = swing(&arrivals, finish, true);
        println!("ETA_SWING jitter new={new:?} old={old:?}");
        assert!(new.total_rise < old.total_rise, "{new:?} {old:?}");
        assert!(new.max_rise < old.max_rise, "{new:?} {old:?}");
        assert!(new.label_back <= old.label_back, "{new:?} {old:?}");
    }

    #[test]
    fn 止まったら見積もりは数え下ろしをやめてつなぎ目で飛ばずに伸びる() {
        // 40 MiB のうち 10 MiB を 300 ms ごとに写し（最後の進みは 3 秒）、そこで止まった
        let at = 3.0;
        let snap_at = |now: f64| ProgressSnapshot {
            entries_done: 0,
            entries_total: 1,
            bytes_done: 10 << 20,
            bytes_total: 40 << 20,
            counting: false,
            cancelled: false,
            copying_for: std::time::Duration::from_secs_f64(now),
            bytes_at: std::time::Duration::from_secs_f64(at),
            advances: 10,
        };
        let left = |now: f64| eta(&snap_at(now)).unwrap().as_secs_f64();
        // 平均の間隔 300 ms × 3 < 2 秒 = 2 秒までは数え下ろす（9 秒 → 7 秒）
        assert_eq!(stall_after(&snap_at(4.0)), ETA_STALL_MIN);
        assert!((left(3.0) - 9.0).abs() < 1e-6, "{}", left(3.0));
        assert!((left(5.0) - 7.0).abs() < 1e-6, "{}", left(5.0));
        // 止まったと見なした後は #1895 と同じ伸び方（残り ÷ 写した = 3 倍）で伸びる
        assert!((left(6.0) - 10.0).abs() < 1e-6, "{}", left(6.0));
        // つなぎ目で飛ばない（2 秒の前後 1 ms で差が 0.01 秒未満）
        assert!((left(5.001) - left(4.999)).abs() < 0.01);
        // 止まっている間に 0 へ落ちて「まもなく完了」と言い続けない
        assert!(left(12.0) > 9.0, "{}", left(12.0));
        // 遅い媒体（1 回の間隔が 1 秒）は 3 秒までふだんの途切れとして数え下ろす
        let mut slow = snap_at(13.0);
        slow.bytes_at = std::time::Duration::from_secs(10);
        assert_eq!(stall_after(&slow), std::time::Duration::from_secs(3));
        // 写し終える前の最後の 1 チャンク待ち = 0 で止まる（負にしない）
        let mut last = snap_at(3.2);
        last.bytes_done = (40 << 20) - 1;
        last.bytes_at = std::time::Duration::from_secs_f64(3.0);
        assert_eq!(eta(&last), Some(std::time::Duration::ZERO));
        // 進みの時刻が無い（まだ記録が無い）・いまを越えている = #1895 の式へ落ちる
        let mut none = snap(40_000_000, 100_000_000, 4.0);
        none.bytes_at = std::time::Duration::ZERO;
        assert!((eta(&none).unwrap().as_secs_f64() - 6.0).abs() < 0.01);
        none.bytes_at = std::time::Duration::from_secs(5);
        assert!((eta(&none).unwrap().as_secs_f64() - 6.0).abs() < 0.01);
    }

    #[test]
    fn 進んだ時刻と回数がバイトの進みにだけ載る() {
        let progress = Progress::default();
        progress.finish_counting();
        progress.add_done(1, 0);
        let s = progress.snapshot();
        assert_eq!((s.bytes_at, s.advances), (std::time::Duration::ZERO, 0));
        std::thread::sleep(std::time::Duration::from_millis(5));
        progress.add_done(0, 1 << 20);
        let s = progress.snapshot();
        assert_eq!(s.advances, 1);
        assert!(s.bytes_at > std::time::Duration::ZERO && s.bytes_at <= s.copying_for);
    }

    #[test]
    fn 残り時間は始めの数秒と少ししか写していないうちは出さず平均の速さで割る() {
        // 10 MB/s で 40% = 残り 60 MB = 6 秒
        let left = eta(&snap(40_000_000, 100_000_000, 4.0)).unwrap();
        assert!((left.as_secs_f64() - 6.0).abs() < 0.01, "{left:?}");
        // 写し始めて 2 秒経っていない
        assert_eq!(eta(&snap(40_000_000, 100_000_000, 1.9)), None);
        // 1% 写していない（10 GB の 0.5%）
        assert_eq!(eta(&snap(50_000_000, 10_000_000_000, 10.0)), None);
        assert!(
            eta(&snap(100_000_000, 10_000_000_000, 10.0)).is_some(),
            "1% ちょうどで出す"
        );
        // 数えている・取り消した・バイトが無い・写し終えた・0 バイト
        let mut s = snap(40, 100, 4.0);
        s.counting = true;
        assert_eq!(eta(&s), None);
        let mut s = snap(40, 100, 4.0);
        s.cancelled = true;
        assert_eq!(eta(&s), None);
        assert_eq!(eta(&snap(0, 0, 4.0)), None);
        assert_eq!(eta(&snap(100, 100, 4.0)), None);
        assert_eq!(eta(&snap(0, 100, 4.0)), None);
        // 何日もかかる見積もりは出さない
        assert_eq!(eta(&snap(1_000_000, 100_000_000_000_000, 10.0)), None);
        // 巨大な母数でも桁あふれしない
        assert!(eta(&snap(u64::MAX / 2, u64::MAX, 3.0)).is_some());
    }

    #[test]
    fn 残り時間は帯に書く粒度へ丸める() {
        use std::time::Duration;
        let s = |secs: f64| eta_label(Duration::from_secs_f64(secs));
        assert_eq!(s(0.2), EtaLabel::Soon);
        assert_eq!(s(9.0), EtaLabel::Soon);
        assert_eq!(s(9.5), EtaLabel::Seconds(10));
        assert_eq!(s(11.0), EtaLabel::Seconds(15));
        assert_eq!(s(25.0), EtaLabel::Seconds(25));
        assert_eq!(s(56.0), EtaLabel::Minutes(1), "60 秒へ切り上がったら 1 分");
        assert_eq!(s(60.0), EtaLabel::Minutes(1));
        assert_eq!(s(89.0), EtaLabel::Minutes(1));
        assert_eq!(s(90.0), EtaLabel::Minutes(2));
        assert_eq!(s(3569.0), EtaLabel::Minutes(59));
        assert_eq!(
            s(3590.0),
            EtaLabel::Hours {
                hours: 1,
                minutes: 0
            },
            "60 分へ丸まったら 1 時間"
        );
        assert_eq!(
            s(3601.0),
            EtaLabel::Hours {
                hours: 1,
                minutes: 10
            }
        );
        assert_eq!(
            s(4800.0),
            EtaLabel::Hours {
                hours: 1,
                minutes: 20
            }
        );
        assert_eq!(
            s(7200.0),
            EtaLabel::Hours {
                hours: 2,
                minutes: 0
            }
        );
    }

    #[test]
    fn 数え終えてからの時間が進み具合に載る() {
        let progress = Progress::default();
        assert_eq!(progress.snapshot().copying_for, std::time::Duration::ZERO);
        progress.finish_counting();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let first = progress.snapshot().copying_for;
        assert!(first >= std::time::Duration::from_millis(20), "{first:?}");
        // 2 度目の印は時刻を上書きしない（始めた時刻は 1 つ）
        progress.finish_counting();
        assert!(progress.snapshot().copying_for >= first);
    }

    #[test]
    fn 走っているコピーの一覧は札を落とすと外れ番号で取り消せる() {
        let jobs = CopyJobs::new();
        let a = jobs.register(vec![PathBuf::from("/w/a")], PathBuf::from("/w/dst"));
        let b = jobs.register(vec![PathBuf::from("/w/b")], PathBuf::from("/w/dst"));
        assert_ne!(a.ticket().id, b.ticket().id);
        let ids: Vec<u64> = jobs.list().iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![a.ticket().id, b.ticket().id]);
        // 番号で 1 つだけ
        assert_eq!(jobs.cancel(Some(b.ticket().id)), vec![b.ticket().id]);
        assert!(b.ticket().progress.is_cancelled());
        assert!(!a.ticket().progress.is_cancelled());
        // 無い番号は何も取り消さない
        assert!(jobs.cancel(Some(9999)).is_empty());
        drop(b);
        assert_eq!(jobs.list().len(), 1, "札を落としたら一覧から外れる");
        // 省略 = 全部
        assert_eq!(jobs.cancel(None), vec![a.ticket().id]);
        drop(a);
        assert!(jobs.list().is_empty());
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
