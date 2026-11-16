//! リモート（スマホ）のファイル閲覧で使うショートカット（お気に入り）の**正本**（#1451）。
//!
//! `<data_dir>/remote/shortcuts.json` に「よく行くフォルダ」を貯める。
//! PWA・CLI（`tako remote shortcuts`）・MCP（`tako_remote_shortcuts`）の 3 口が
//! **この 1 実装**を通るので、どこから足しても同じ結果になる（開発不変条件）。
//!
//! # 既定のショートカットを**ファイルに書かない**理由
//!
//! `~` / デスクトップ / ダウンロードは [`defaults_for`] が**毎回組む**（macOS / unix は
//! `~/Desktop` / `~/Downloads`、Windows は Known Folder の置き場 = OneDrive へ移したデスクトップも出る。#2939）。
//! 初回に書き込む形にすると、
//!
//! - ホームの位置が変わった環境（別マシンへ data dir を持ち込んだ・`HOME` を変えた）で
//!   腐ったパスが残る
//! - 「消したのに次の起動で復活する」「消したまま二度と戻らない」の**両方**が起きる
//!   （どちらへ倒しても利用者の期待と外れる）
//!
//! ので、**既定は計算・消せるのは登録分だけ**に倒した。
//! 既定は [`Shortcut::builtin`] が true なので、画面・CLI は削除ボタンを出さない。
//!
//! # 認可はここに無い
//!
//! ショートカットは**行き先の宣言**であって、読める範囲を広げない。
//! 実際に読めるかは `tako_control::remote_files` の認可（role + ルート配下判定）が
//! 毎リクエスト決める。ここに許可リストを持たせない = 認可の正が 2 つに割れない。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::load_issue::LoadIssue;
use crate::paths;
use crate::platform::known_folder::{self, KnownFolder};

/// 登録できるショートカットの上限。UI の一覧が実用の範囲に収まればよく、
/// 溢れさせない目的だけなので大きめに取る
pub const MAX_SHORTCUTS: usize = 64;

/// 表示名の上限（バイトではなく文字数。1 行に収める目的）
pub const MAX_NAME_CHARS: usize = 64;

/// ファイル名（`<data_dir>/remote/` 配下）
pub const FILENAME: &str = "shortcuts.json";

/// ショートカット 1 件
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Shortcut {
    /// パスから導く 12 桁の不透明値。**秘密ではない**（削除の宛先に使うだけで、
    /// 認可はパスそのもので行う）。パス由来なので同じパスの二重登録が構造的に起きない
    pub id: String,
    /// 絶対パス（正規化済み）
    pub path: String,
    /// 表示名
    pub name: String,
    /// 追加した時刻（UNIX 秒）。既定のショートカットは 0
    #[serde(default)]
    pub added_at: u64,
    /// 計算で出している既定のショートカットか（**ファイルには書かれない**）
    #[serde(default, skip_serializing)]
    pub builtin: bool,
}

impl Shortcut {
    /// 既定（計算で出す）のショートカットを組む
    fn builtin(path: &Path, name: &str) -> Self {
        Self {
            id: id_of(&path.to_string_lossy()),
            path: path.to_string_lossy().to_string(),
            name: name.to_string(),
            added_at: 0,
            builtin: true,
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "path": self.path,
            // `~` 短縮した見せ方（正本は paths::shorten_home の 1 実装）
            "display_path": display_path(&self.path),
            "name": self.name,
            "added_at": self.added_at,
            "builtin": self.builtin,
        })
    }
}

/// `~` 短縮した見せ方。**素のホームと解決済みのホームの両方**で試す。
///
/// 保存するパスは `canonicalize` 済み（#970）なので、ホームが symlink 越しにある環境
/// （`/tmp` → `/private/tmp` の macOS や、ホームを別ボリュームへ張っている環境）では
/// 素の `$HOME` と前方一致しない = `~` にならず絶対パスが画面に出てしまう。
/// 両方を試すことで、どちらの綴りでも短縮が効く
fn display_path(path: &str) -> String {
    let short = paths::shorten_home(path);
    if short != path {
        return short;
    }
    let resolved = paths::home_dir().map(|h| crate::platform::path::canonicalize_or_self(&h));
    paths::shorten_home_with(path, resolved.as_deref())
}

/// パスから 12 桁の id を作る（FNV-1a 64bit）。
///
/// 暗号学的強度は要らない: id は秘密ではなく、削除の宛先を指すだけ。
/// **同じパスは必ず同じ id** になるので、二重登録が構造的に起きない
pub fn id_of(path: &str) -> String {
    // 切り出し安全: `{:012x}` の 16 進は 12 桁以上の ASCII
    format!("{:012x}", crate::fnv::fnv1a64(path.as_bytes()))[..12].to_string()
}

/// 永続ファイルの中身
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutsFile {
    /// スキーマ版数（#916 の移行機構が読む）
    pub version: u32,
    #[serde(default)]
    pub shortcuts: Vec<Shortcut>,
}

/// 現行のスキーマ版数
pub const CURRENT_VERSION: u32 = 1;

impl Default for ShortcutsFile {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            shortcuts: Vec::new(),
        }
    }
}

/// 追加が通らなかった理由。**断る理由を型で持つ**ので、HTTP / CLI / MCP が
/// 同じ分類で同じ文言を出せる（片方だけ無言で失敗しない）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutError {
    /// 相対パス（絶対パスだけを受ける）
    NotAbsolute,
    /// 実体が無い
    NotFound,
    /// フォルダではない
    NotADirectory,
    /// 上限に達している
    TooMany,
    /// 削除しようとしたものが無い
    UnknownShortcut,
    /// 既定のショートカットは削除できない
    Builtin,
}

impl ShortcutError {
    pub fn kind(self) -> &'static str {
        match self {
            Self::NotAbsolute => "not_absolute",
            Self::NotFound => "not_found",
            Self::NotADirectory => "not_a_directory",
            Self::TooMany => "too_many",
            Self::UnknownShortcut => "unknown_shortcut",
            Self::Builtin => "builtin",
        }
    }

    pub fn message_ja(self) -> String {
        match self {
            Self::NotAbsolute => "絶対パスで指定してください".to_string(),
            Self::NotFound => "そのフォルダが見つかりません".to_string(),
            Self::NotADirectory => "フォルダではありません".to_string(),
            Self::TooMany => format!("ショートカットは {MAX_SHORTCUTS} 件までです"),
            Self::UnknownShortcut => "そのショートカットは登録されていません".to_string(),
            Self::Builtin => "既定のショートカットは削除できません".to_string(),
        }
    }

    pub fn message_en(self) -> String {
        match self {
            Self::NotAbsolute => "Use an absolute path".to_string(),
            Self::NotFound => "That folder does not exist".to_string(),
            Self::NotADirectory => "Not a directory".to_string(),
            Self::TooMany => format!("At most {MAX_SHORTCUTS} shortcuts"),
            Self::UnknownShortcut => "No such shortcut".to_string(),
            Self::Builtin => "Built-in shortcuts cannot be removed".to_string(),
        }
    }

    /// HTTP のステータス。**認可の失敗ではない**ので 403 は使わない
    pub fn status(self) -> u16 {
        match self {
            Self::NotFound | Self::UnknownShortcut => 404,
            Self::TooMany => 409,
            _ => 400,
        }
    }
}

impl ShortcutsFile {
    /// 指定パスから読む。**解釈できない内容は既定値へ落とす前に退避する**（#916）。
    /// 退避しておかないと、直後の [`save_to`](Self::save_to) が元の内容を上書きして消す。
    ///
    /// 既定の置き場から読むのは `tako_control::data_files::load_remote_shortcuts`
    /// （落とした理由を persist.log へ残す口。#1824）。ここを直接呼ぶと記録が残らない
    pub fn load_from(path: &Path) -> Self {
        Self::load_from_checked(path).0
    }

    /// [`load_from`](Self::load_from) の本体。既定値へ落としたなら理由も返す（#1824）
    pub fn load_from_checked(path: &Path) -> (Self, Option<LoadIssue>) {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) => return (Self::default(), LoadIssue::read_failed(path, &e)),
        };
        match serde_json::from_str::<Self>(&text) {
            Ok(mut file) => {
                // 読んだ内容は「登録分」なので、既定の印を必ず落とす
                // （ファイルへ builtin が混ざっても削除できない項目が生えない）
                for s in &mut file.shortcuts {
                    s.builtin = false;
                }
                (file, None)
            }
            Err(e) => (Self::default(), Some(LoadIssue::unparsable(path, &e))),
        }
    }

    /// 既定の置き場へ書く
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = shortcuts_path() else {
            return Err(std::io::Error::other("data dir を解決できない"));
        };
        self.save_to(&path)
    }

    /// 指定パスへ書く。親ディレクトリとファイルは本人だけにする（`remote/` と同じ基準。unix は
    /// 0700 / 0600、Windows は継承を切った本人 + SYSTEM の DACL = 境界 `platform::private_file`・#2581）
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            let _ = crate::platform::private_file::restrict_dir(parent);
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)?;
        let _ = crate::platform::private_file::restrict(path);
        Ok(())
    }

    /// 登録分だけを返す（既定は含まない）
    pub fn user_shortcuts(&self) -> &[Shortcut] {
        &self.shortcuts
    }

    /// 1 件足す。**冪等**: 同じパスが既に在れば件数を増やさず、表示名だけ更新する。
    ///
    /// `now` を引数で受けるのは、時刻を読む場所をここへ閉じ込めないため
    /// （テストが実時間に依存しない）
    pub fn add(
        &mut self,
        path: &Path,
        name: Option<&str>,
        now: u64,
    ) -> Result<Shortcut, ShortcutError> {
        let canon = normalize_target(path)?;
        let path_str = canon.to_string_lossy().to_string();
        let id = id_of(&path_str);
        let label = display_name(name, &canon);

        if let Some(existing) = self.shortcuts.iter_mut().find(|s| s.id == id) {
            // 冪等: 件数は増やさない。名前を明示されたときだけ差し替える
            if name.is_some() {
                existing.name = label;
            }
            return Ok(existing.clone());
        }
        if self.shortcuts.len() >= MAX_SHORTCUTS {
            return Err(ShortcutError::TooMany);
        }
        let entry = Shortcut {
            id,
            path: path_str,
            name: label,
            added_at: now,
            builtin: false,
        };
        self.shortcuts.push(entry.clone());
        Ok(entry)
    }

    /// 1 件消す。`key` は id でもパスでもよい（利用者が打ちやすい方で消せる）。
    ///
    /// **既定のショートカットを指したときは専用の理由で断る**
    /// （「無い」と言うと、消えないのが不具合に見える）
    pub fn remove(&mut self, key: &str, home: Option<&Path>) -> Result<Shortcut, ShortcutError> {
        self.remove_with(key, home, &FolderLookup::system())
    }

    /// [`remove`](Self::remove) の本体（既定かどうかを [`listing_with`](Self::listing_with) と
    /// 同じ引き方で判定する = 一覧に既定として出る行は必ず「既定は消せない」で断る。#2939）
    pub fn remove_with(
        &mut self,
        key: &str,
        home: Option<&Path>,
        lookup: &FolderLookup<'_>,
    ) -> Result<Shortcut, ShortcutError> {
        let key_id = resolve_key(key);
        if let Some(at) = self.shortcuts.iter().position(|s| s.id == key_id) {
            return Ok(self.shortcuts.remove(at));
        }
        if defaults_with(home, lookup).iter().any(|s| s.id == key_id) {
            return Err(ShortcutError::Builtin);
        }
        Err(ShortcutError::UnknownShortcut)
    }

    /// 既定 + 登録分を、画面へ出す並びで返す。
    ///
    /// **既定が先**（`~` → Desktop → Downloads → 登録分の追加順）。
    /// 登録分に既定と同じパスが在れば、既定の側を落として二重に出さない
    pub fn listing(&self, home: Option<&Path>) -> Vec<Shortcut> {
        self.listing_with(home, &FolderLookup::system())
    }

    /// [`listing`](Self::listing) の本体（既知フォルダの引き方を渡せるので、Windows の置き場を
    /// どの OS のテストからも差し替えられる。#2939）
    pub fn listing_with(&self, home: Option<&Path>, lookup: &FolderLookup<'_>) -> Vec<Shortcut> {
        let mut out = Vec::new();
        let user_ids: std::collections::HashSet<&str> =
            self.shortcuts.iter().map(|s| s.id.as_str()).collect();
        for d in defaults_with(home, lookup) {
            if !user_ids.contains(d.id.as_str()) {
                out.push(d);
            }
        }
        out.extend(self.shortcuts.iter().cloned());
        out
    }
}

/// `key` が id の形ならそのまま、そうでなければパスとして id へ変換する。
///
/// id は 12 桁の 16 進なので、**そう見える文字列だけ**を id 扱いにする
/// （`/tmp` のようなパスを id と読み違えない）
fn resolve_key(key: &str) -> String {
    if key.len() == 12 && key.chars().all(|c| c.is_ascii_hexdigit()) {
        return key.to_string();
    }
    // パスで指された場合は、実体があれば正規化してから引く（symlink 経由でも当たる）。
    // 解決は境界の 1 実装を通す（#970。Windows の verbatim prefix を応答へ出さない）
    let canon = crate::platform::path::canonicalize_or_self(Path::new(key));
    id_of(&canon.to_string_lossy())
}

/// 追加対象の検査 + 正規化。**絶対パスで・実在して・フォルダであること**だけを要求する
/// （どこを指してよいかは認可の仕事で、ここではない）
fn normalize_target(path: &Path) -> Result<PathBuf, ShortcutError> {
    if !path.is_absolute() {
        return Err(ShortcutError::NotAbsolute);
    }
    // 保存するパスなので境界の 1 実装を通す（#970）
    let canon = crate::platform::path::canonicalize(path).map_err(|_| ShortcutError::NotFound)?;
    if !canon.is_dir() {
        return Err(ShortcutError::NotADirectory);
    }
    Ok(canon)
}

/// 表示名を決める。明示が無ければ末尾のフォルダ名、それも取れなければパスそのもの
fn display_name(name: Option<&str>, canon: &Path) -> String {
    let raw = name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            canon
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| canon.to_string_lossy().to_string())
        });
    // 改行・制御文字は落とす（1 行に出すので）。長さも切る
    let cleaned: String = raw.chars().filter(|c| !c.is_control()).collect();
    cleaned.chars().take(MAX_NAME_CHARS).collect()
}

/// #2939 の A/B（同一バイナリで修正前 = Windows でも `<ホーム>\Desktop` / `<ホーム>\Downloads` の決め打ち）
pub const LEGACY_ENV: &str = "TAKO_2939_LEGACY";

fn legacy() -> bool {
    std::env::var_os(LEGACY_ENV).is_some_and(|v| !v.is_empty())
}

/// 既定のショートカットに並べる既知フォルダ（並び順 = 画面の並び）
const DEFAULT_FOLDERS: [(KnownFolder, &str); 2] = [
    (KnownFolder::Desktop, "デスクトップ"),
    (KnownFolder::Downloads, "ダウンロード"),
];

/// 既定のショートカットの置き場の引き方（#2939）。本番は [`FolderLookup::system`]、
/// テストは OS の答え（`os_answer`）を差し替えて、Windows の置き場をどの OS からも再現する
pub struct FolderLookup<'a> {
    /// OS が答える既知フォルダの置き場（本番は境界 B39 [`known_folder::os_path`]。
    /// macOS / unix は常に `None` = ホームの下の慣例の名前で組む）
    pub os_answer: &'a dyn Fn(KnownFolder) -> Option<PathBuf>,
    /// 検証プロセスか（[`paths::is_verification_process`]）。検証プロセスは `HOME` を砂場へ
    /// 差し替えて動くが、OS の答えは差し替えに追従しない（本物の利用者の置き場を返す）。
    /// なので渡されたホームが OS の利用者のプロファイルと同じとき（= 砂場へ差し替えていない）
    /// だけ OS の答えを使い、違えば砂場のホームの下を見る
    pub verification: bool,
    /// [`LEGACY_ENV`]（OS の答えを見ない = 修正前）
    pub legacy: bool,
}

impl FolderLookup<'static> {
    /// 本番の引き方（境界 B39 の口・このプロセスの検証判定・A/B の env）
    pub fn system() -> Self {
        Self {
            os_answer: &known_folder::os_path,
            verification: paths::is_verification_process(),
            legacy: legacy(),
        }
    }
}

impl FolderLookup<'_> {
    /// 正規化済みの `home` に対して OS の答えを使ってよいか
    fn os_answers_apply(&self, home: &Path) -> bool {
        if self.legacy {
            return false;
        }
        if !self.verification {
            return true;
        }
        (self.os_answer)(KnownFolder::Profile)
            .is_some_and(|profile| crate::platform::path::canonicalize_or_self(&profile) == home)
    }

    /// `home`（正規化済み）の既知フォルダの置き場。OS の答えを使えるならそれ、
    /// 使えない・答えが無いならホームの下の慣例の名前
    fn folder_under(&self, home: &Path, folder: KnownFolder, use_os: bool) -> Option<PathBuf> {
        let os = if use_os {
            (self.os_answer)(folder)
        } else {
            None
        };
        os.or_else(|| folder.home_relative().map(|rel| home.join(rel)))
    }
}

/// 既定のショートカット（**ホームを引数で受ける**）。
///
/// 実在するものだけを返すので、デスクトップ / ダウンロードを消している環境で
/// 「押しても開けない行」が並ばない。ホームを引数にしてあるのは、
/// テストが `HOME` を書き換えずに（= 他のテストと干渉せずに）検査できるようにするため。
/// デスクトップ / ダウンロードの置き場は Windows では Known Folder で引く（#2939。[`FolderLookup`]）
pub fn defaults_for(home: Option<&Path>) -> Vec<Shortcut> {
    defaults_with(home, &FolderLookup::system())
}

/// [`defaults_for`] の本体（既知フォルダの引き方を渡せる純粋関数。#2939）
pub fn defaults_with(home: Option<&Path>, lookup: &FolderLookup<'_>) -> Vec<Shortcut> {
    let Some(home) = home else {
        return Vec::new();
    };
    // **登録分と同じ基準で正規化する**（#970 の 1 実装）。
    // ここを素のパスのままにすると、`add` 側は canonicalize 済みなので
    // **同じフォルダが「既定」と「登録分」の 2 行に分かれて出る**（id がパス由来のため）。
    // 実測: ホームが symlink 越し（`/tmp` → `/private/tmp`）の環境で Downloads が二重に出た。
    // Known Folder の答えも同じ正規化を通す（#2939）
    let home = crate::platform::path::canonicalize_or_self(home);
    let mut out = Vec::new();
    if home.is_dir() {
        out.push(Shortcut::builtin(&home, "ホーム"));
    }
    let use_os = lookup.os_answers_apply(&home);
    for (folder, label) in DEFAULT_FOLDERS {
        let Some(dir) = lookup.folder_under(&home, folder, use_os) else {
            continue;
        };
        let p = crate::platform::path::canonicalize_or_self(&dir);
        if p.is_dir() {
            out.push(Shortcut::builtin(&p, label));
        }
    }
    out
}

/// 既定のショートカット（実環境のホームを使う）
pub fn defaults() -> Vec<Shortcut> {
    defaults_for(paths::home_dir().as_deref())
}

/// 永続ファイルの場所
pub fn shortcuts_path() -> Option<PathBuf> {
    paths::data_dir().map(|d| d.join("remote").join(FILENAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tako-shortcuts-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("作れる");
        // テストも境界の 1 実装を通す（#970。例外を作らない）
        crate::platform::path::canonicalize(&dir).expect("正規化できる")
    }

    #[test]
    fn 同じパスを2回足しても1件のまま() {
        let dir = tmp_dir("dup");
        let target = dir.join("projects");
        std::fs::create_dir_all(&target).expect("作れる");
        let mut file = ShortcutsFile::default();
        file.add(&target, None, 100).expect("足せる");
        file.add(&target, None, 200).expect("足せる");
        assert_eq!(file.shortcuts.len(), 1, "冪等でなければならない");
        assert_eq!(file.shortcuts[0].added_at, 100, "初回の時刻を保つ");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 名前を明示したときだけ表示名が変わる() {
        let dir = tmp_dir("rename");
        let target = dir.join("projects");
        std::fs::create_dir_all(&target).expect("作れる");
        let mut file = ShortcutsFile::default();
        file.add(&target, None, 1).expect("足せる");
        assert_eq!(file.shortcuts[0].name, "projects");
        file.add(&target, None, 2).expect("足せる");
        assert_eq!(file.shortcuts[0].name, "projects", "名前なしでは変えない");
        file.add(&target, Some("仕事"), 3).expect("足せる");
        assert_eq!(file.shortcuts[0].name, "仕事");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 相対パスと不在とファイルは断る() {
        let dir = tmp_dir("reject");
        let file_path = dir.join("memo.txt");
        std::fs::write(&file_path, "x").expect("書ける");
        let mut file = ShortcutsFile::default();
        assert_eq!(
            file.add(Path::new("relative/dir"), None, 1),
            Err(ShortcutError::NotAbsolute)
        );
        assert_eq!(
            file.add(&dir.join("いない"), None, 1),
            Err(ShortcutError::NotFound)
        );
        assert_eq!(
            file.add(&file_path, None, 1),
            Err(ShortcutError::NotADirectory)
        );
        assert!(file.shortcuts.is_empty(), "断ったものは足さない");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn idでもパスでも消せる() {
        let dir = tmp_dir("remove");
        let a = dir.join("a");
        let b = dir.join("b");
        std::fs::create_dir_all(&a).expect("作れる");
        std::fs::create_dir_all(&b).expect("作れる");
        let mut file = ShortcutsFile::default();
        let sa = file.add(&a, None, 1).expect("足せる");
        file.add(&b, None, 2).expect("足せる");
        file.remove(&sa.id, None).expect("id で消せる");
        assert_eq!(file.shortcuts.len(), 1);
        file.remove(&b.to_string_lossy(), None)
            .expect("パスで消せる");
        assert!(file.shortcuts.is_empty());
        assert_eq!(
            file.remove("いない", None),
            Err(ShortcutError::UnknownShortcut)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 既定のショートカットは消せないと名指しで断る() {
        let home = tmp_dir("builtin-home");
        std::fs::create_dir_all(home.join("Desktop")).expect("作れる");
        let mut file = ShortcutsFile::default();
        let err = file
            .remove(&home.to_string_lossy(), Some(&home))
            .expect_err("既定は消せない");
        assert_eq!(err, ShortcutError::Builtin, "「無い」ではなく理由を返す");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn 既定は実在するものだけ出る() {
        let home = tmp_dir("defaults");
        std::fs::create_dir_all(home.join("Downloads")).expect("作れる");
        let list = defaults_for(Some(&home));
        let names: Vec<&str> = list.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["ホーム", "ダウンロード"],
            "Desktop が無い環境では出さない"
        );
        assert!(list.iter().all(|s| s.builtin), "既定の印が付く");
        assert!(defaults_for(None).is_empty(), "ホームが解決できなければ空");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn 登録分が既定と同じパスなら二重に出ない() {
        let home = tmp_dir("overlap");
        std::fs::create_dir_all(home.join("Desktop")).expect("作れる");
        let mut file = ShortcutsFile::default();
        file.add(&home.join("Desktop"), Some("私の机"), 1)
            .expect("足せる");
        let listing = file.listing(Some(&home));
        let desktop: Vec<&Shortcut> = listing
            .iter()
            .filter(|s| s.path.ends_with("Desktop"))
            .collect();
        assert_eq!(desktop.len(), 1, "二重に出さない");
        assert_eq!(desktop[0].name, "私の机", "登録分の名前が勝つ");
        assert!(!desktop[0].builtin, "登録分なので消せる");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// 実測で見つけた回帰（#1451）: 既定を素のパスのまま組むと、`add` 側は
    /// canonicalize 済みなので **同じフォルダが「既定」と「登録分」の 2 行に出る**
    #[test]
    fn symlink越しのホームでも既定と登録分が二重に出ない() {
        let real = tmp_dir("symlink-home");
        std::fs::create_dir_all(real.join("Downloads")).expect("作れる");
        // ホームへ symlink 越しで辿り着く経路を作る（macOS の `/tmp` → `/private/tmp` 相当）
        let link = real.parent().expect("親").join(format!(
            "tako-shortcuts-link-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&link);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).expect("張れる");
        #[cfg(not(unix))]
        let link = real.clone();

        let mut file = ShortcutsFile::default();
        // 利用者は symlink 側の綴りで登録する
        file.add(&link.join("Downloads"), Some("落とし物"), 1)
            .expect("足せる");
        let listing = file.listing(Some(&link));
        let downloads: Vec<&Shortcut> = listing
            .iter()
            .filter(|s| s.path.ends_with("Downloads"))
            .collect();
        assert_eq!(
            downloads.len(),
            1,
            "既定と登録分でパスの正規化基準が違うと同じフォルダが 2 行に出る（#1451）"
        );
        assert!(!downloads[0].builtin, "登録分が勝つ（消せる側を残す）");
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_dir_all(&real);
    }

    #[test]
    fn 保存して読み直しても同じ並び() {
        let dir = tmp_dir("roundtrip");
        let a = dir.join("a");
        let b = dir.join("b");
        std::fs::create_dir_all(&a).expect("作れる");
        std::fs::create_dir_all(&b).expect("作れる");
        let path = dir.join("remote").join(FILENAME);
        let mut file = ShortcutsFile::default();
        file.add(&a, Some("A"), 10).expect("足せる");
        file.add(&b, Some("B"), 20).expect("足せる");
        file.save_to(&path).expect("書ける");
        let back = ShortcutsFile::load_from(&path);
        assert_eq!(back.version, CURRENT_VERSION);
        let names: Vec<&str> = back.shortcuts.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["A", "B"], "追加順が保たれる");
        assert!(back.shortcuts.iter().all(|s| !s.builtin));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 壊れたファイルは退避されてから空になる() {
        let dir = tmp_dir("broken");
        let path = dir.join(FILENAME);
        std::fs::write(&path, "{こわれた").expect("書ける");
        let file = ShortcutsFile::load_from(&path);
        assert!(file.shortcuts.is_empty());
        assert_eq!(
            std::fs::read_to_string(crate::migration::quarantine_path(&path)).expect("読める"),
            "{こわれた",
            "既定へ落とす前に退避する（#916）"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 表示名は制御文字を落として長さを切る() {
        let dir = tmp_dir("name");
        let target = dir.join("x");
        std::fs::create_dir_all(&target).expect("作れる");
        let mut file = ShortcutsFile::default();
        let long = "あ".repeat(MAX_NAME_CHARS + 10);
        let s = file
            .add(&target, Some(&format!("ab\ncd{long}")), 1)
            .expect("足せる");
        assert!(!s.name.contains('\n'), "改行は落とす");
        assert_eq!(s.name.chars().count(), MAX_NAME_CHARS, "長さを切る");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 上限を超えたら断る() {
        let dir = tmp_dir("limit");
        let mut file = ShortcutsFile::default();
        for i in 0..MAX_SHORTCUTS {
            let p = dir.join(format!("d{i}"));
            std::fs::create_dir_all(&p).expect("作れる");
            file.add(&p, None, i as u64).expect("足せる");
        }
        let extra = dir.join("over");
        std::fs::create_dir_all(&extra).expect("作れる");
        assert_eq!(file.add(&extra, None, 1), Err(ShortcutError::TooMany));
        assert_eq!(file.shortcuts.len(), MAX_SHORTCUTS);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #2939 の機械の形: デスクトップを OneDrive へ移してあり（Known Folder Move）、
    /// `<ホーム>/Desktop` は無い。ダウンロードはホームの下のまま
    struct OneDriveHome {
        _scratch: crate::test_residue::ScratchDir,
        /// 素の（正規化前の）ホーム。OS のプロファイルの答えもこの綴りで返す
        home: PathBuf,
        /// OS が答えるデスクトップ（`<ホーム>/OneDrive/デスクトップ`）
        desktop: PathBuf,
    }

    impl OneDriveHome {
        fn new() -> Self {
            let scratch = crate::test_residue::ScratchDir::new("issue2939");
            let home = scratch.path().join("winuser");
            let desktop = home.join("OneDrive").join("デスクトップ");
            std::fs::create_dir_all(&desktop).expect("作れる");
            std::fs::create_dir_all(home.join("Downloads")).expect("作れる");
            assert!(!home.join("Desktop").exists(), "前提: ~/Desktop は無い");
            Self {
                _scratch: scratch,
                home,
                desktop,
            }
        }

        /// Windows の Known Folder の答え（このホームの利用者として答える）
        fn answer(&self, folder: KnownFolder) -> Option<PathBuf> {
            match folder {
                KnownFolder::Profile => Some(self.home.clone()),
                KnownFolder::Desktop => Some(self.desktop.clone()),
                KnownFolder::Downloads => Some(self.home.join("Downloads")),
                KnownFolder::Documents => None,
            }
        }
    }

    fn names(list: &[Shortcut]) -> Vec<&str> {
        list.iter().map(|s| s.name.as_str()).collect()
    }

    fn canon(p: &Path) -> String {
        crate::platform::path::canonicalize_or_self(p)
            .to_string_lossy()
            .to_string()
    }

    #[test]
    fn issue2939_onedriveへ移したデスクトップも既定に出る() {
        let m = OneDriveHome::new();
        let answer = |f| m.answer(f);
        // cargo test は検証プロセス。渡したホームが OS のプロファイルと同じなので OS の答えを使う
        let lookup = FolderLookup {
            os_answer: &answer,
            verification: true,
            legacy: false,
        };
        let list = defaults_with(Some(&m.home), &lookup);
        assert_eq!(names(&list), vec!["ホーム", "デスクトップ", "ダウンロード"]);
        assert_eq!(
            list[1].path,
            canon(&m.desktop),
            "OneDrive の下のデスクトップ"
        );
        assert!(list.iter().all(|s| s.builtin));

        // A/B: 修正前（`TAKO_2939_LEGACY=1`）は `<ホーム>/Desktop` の実在だけを見るので出ない = #2939 の症状
        let legacy = FolderLookup {
            os_answer: &answer,
            verification: true,
            legacy: true,
        };
        assert_eq!(
            names(&defaults_with(Some(&m.home), &legacy)),
            vec!["ホーム", "ダウンロード"]
        );
    }

    #[test]
    fn issue2939_登録分と同じフォルダを足しても2行に分かれない() {
        let m = OneDriveHome::new();
        let answer = |f| m.answer(f);
        let lookup = FolderLookup {
            os_answer: &answer,
            verification: false,
            legacy: false,
        };
        let mut file = ShortcutsFile::default();
        file.add(&m.desktop, Some("私の机"), 1).expect("足せる");
        let listing = file.listing_with(Some(&m.home), &lookup);
        assert_eq!(names(&listing), vec!["ホーム", "ダウンロード", "私の机"]);
        let desk: Vec<&Shortcut> = listing
            .iter()
            .filter(|s| s.path == canon(&m.desktop))
            .collect();
        assert_eq!(
            desk.len(),
            1,
            "Known Folder の答えと登録分が同じ正規化を通る（#970）"
        );
        assert!(!desk[0].builtin, "登録分が勝つ（消せる側を残す）");

        // 登録分を消すと既定へ戻り、既定は「既定は消せない」で断る（一覧と同じ引き方で判定する）
        file.remove_with(&m.desktop.to_string_lossy(), Some(&m.home), &lookup)
            .expect("登録分は消せる");
        assert_eq!(
            file.remove_with(&m.desktop.to_string_lossy(), Some(&m.home), &lookup),
            Err(ShortcutError::Builtin)
        );
        assert_eq!(
            names(&file.listing_with(Some(&m.home), &lookup)),
            vec!["ホーム", "デスクトップ", "ダウンロード"]
        );
    }

    /// 検証プロセスは `HOME` を砂場へ差し替えて動くが、Known Folder は差し替えに追従せず本物の利用者の
    /// 置き場を返す。渡されたホームが OS のプロファイルと違えば、砂場のホームの下を見る
    #[test]
    fn issue2939_検証プロセスは砂場のホームなら本物のデスクトップを見ない() {
        let real = OneDriveHome::new();
        let sandbox = crate::test_residue::ScratchDir::new("issue2939-sandbox");
        let home = sandbox.path().join("home");
        std::fs::create_dir_all(home.join("Desktop")).expect("作れる");
        let answer = |f| real.answer(f);

        let verification = FolderLookup {
            os_answer: &answer,
            verification: true,
            legacy: false,
        };
        let list = defaults_with(Some(&home), &verification);
        assert_eq!(names(&list), vec!["ホーム", "デスクトップ"]);
        assert_eq!(list[1].path, canon(&home.join("Desktop")), "砂場の Desktop");

        // 本番（検証プロセスでない）は、HOME を利用者が変えていても OS のデスクトップを出す
        let production = FolderLookup {
            os_answer: &answer,
            verification: false,
            legacy: false,
        };
        let list = defaults_with(Some(&home), &production);
        assert_eq!(names(&list), vec!["ホーム", "デスクトップ", "ダウンロード"]);
        assert_eq!(list[1].path, canon(&real.desktop));
    }

    /// OS が答えない既知フォルダ（API の失敗・macOS / unix）はホームの下の慣例の名前へ戻る
    #[test]
    fn issue2939_osが答えなければホームの下の慣例の名前() {
        let scratch = crate::test_residue::ScratchDir::new("issue2939-none");
        let home = scratch.path().join("home");
        std::fs::create_dir_all(home.join("Desktop")).expect("作れる");
        std::fs::create_dir_all(home.join("Downloads")).expect("作れる");
        let profile_only = |f: KnownFolder| (f == KnownFolder::Profile).then(|| home.clone());
        for verification in [false, true] {
            let lookup = FolderLookup {
                os_answer: &profile_only,
                verification,
                legacy: false,
            };
            let list = defaults_with(Some(&home), &lookup);
            let paths: Vec<&str> = list.iter().map(|s| s.path.as_str()).collect();
            assert_eq!(
                paths,
                vec![
                    canon(&home).as_str(),
                    canon(&home.join("Desktop")).as_str(),
                    canon(&home.join("Downloads")).as_str(),
                ],
                "verification={verification}"
            );
        }
    }

    /// macOS / unix の既定は #2939 の前と同じ（`~/Desktop` / `~/Downloads`）。本番の引き方
    /// （[`FolderLookup::system`]）の答えが、修正前（A/B の旧アーム）と一字一句同じ。
    /// Windows の `cargo test` でも、一時 dir のホームは OS のプロファイルと違うので同じになる
    #[test]
    fn issue2939_一時dirのホームでは本番の引き方も修正前と同じ() {
        let scratch = crate::test_residue::ScratchDir::new("issue2939-same");
        let home = scratch.path().join("home");
        std::fs::create_dir_all(home.join("Desktop")).expect("作れる");
        std::fs::create_dir_all(home.join("Downloads")).expect("作れる");
        let before = FolderLookup {
            os_answer: &|_| None,
            verification: false,
            legacy: true,
        };
        let system = defaults_for(Some(&home));
        assert_eq!(system, defaults_with(Some(&home), &before));
        assert_eq!(
            names(&system),
            vec!["ホーム", "デスクトップ", "ダウンロード"]
        );
        assert_eq!(system[1].path, canon(&home.join("Desktop")));
        assert_eq!(system[2].path, canon(&home.join("Downloads")));
    }
}
