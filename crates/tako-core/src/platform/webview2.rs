//! Windows の Web ビュー（WebView2）の置き場・ランタイムの版・生成の失敗の案内（#2431）
//!
//! ## ユーザーデータの置き場
//!
//! wry は `WebContext` を渡さないと WebView2 の既定（**実行ファイルの隣**の
//! `<実行ファイル名>.WebView2`）を使う。tako の配布物では
//! `%LOCALAPPDATA%\Programs\tako\tako-app.exe.WebView2` になり、アンインストールしても残り、
//! アプリ内更新（#2434）が実行ファイルを入れ替える場所とも同居していた。
//! 新しい置き場は [`user_data_dir`]（`%LOCALAPPDATA%\tako\WebView2`。理由は
//! [`crate::paths::local_data_dir`]）で、旧い置き場からは `tako_control::migrations` の
//! 自動移行が [`relocate`] で移す（#916）。
//!
//! 移せない（Web ビューが開いていて msedgewebview2 がファイルを掴んでいる）あいだは、
//! 旧い置き場を**そのまま使い続ける**（[`DataDirChoice::PendingLegacy`]）。新しい空の置き場で
//! 始めると Cookie・ログインが片方へ割れるため。移行は次の発火（GUI の起動・`tako master`・
//! `tako setup`・`tako migrate run`）でまた試される。
//!
//! ## ランタイムの版
//!
//! [`runtime`] は Microsoft が文書化している検出の方法（Evergreen の client の
//! レジストリの `pv`。`0.0.0.0` と空は入っていない）で読む。GUI を要らないので CLI の
//! `check-health`・`tako setup --check` からも同じ答えが出る。
//!
//! ## 生成の失敗の分類
//!
//! WebView2 のローダーは「ランタイムが無い」も「互換の版が無い（[`REQUIRED_VERSION`] より古い）」も
//! 同じ `HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND)`（`0x80070002`）で返す。区別は入っている版
//! （[`runtime`]）で付ける（[`classify`]）。文言は日英の網羅 match（[`CreateFailure::message_in`]）

use std::path::{Path, PathBuf};

use crate::i18n::Lang;

/// wry が WebView2 の環境を作るときに求める版（`TargetCompatibleBrowserVersion`）。
///
/// wry 0.57.1 は `CoreWebView2EnvironmentOptions::default()` を渡し、webview2-com 0.39.1 の既定は
/// webview2-com-sys 0.39.1 の `CORE_WEBVIEW_TARGET_PRODUCT_VERSION`。これより古いランタイムしか
/// 無い PC では、ローダーが互換の版を見つけられず `0x80070002` を返す。
/// webview2-com-sys の版を変えたら読み直す（番犬 `webview.rs` の
/// `issue2431_求めるwebview2の版はwryの既定と同じ`）
pub const REQUIRED_VERSION: &str = "145.0.3800.47";

/// GUI の実行ファイル名（インストーラ `installer/windows/tako.iss` の `AppExeName`）
pub const APP_EXE: &str = "tako-app.exe";

/// 新しい置き場のフォルダ名（`<local_data_dir>/WebView2`）
pub const DIR_NAME: &str = "WebView2";

/// 入れ方の案内に出す配布ページ（Microsoft の WebView2 の入口。Evergreen Bootstrapper がある）
pub const DOWNLOAD_PAGE: &str = "https://developer.microsoft.com/microsoft-edge/webview2/";

/// 入れ方の案内に出すコマンド（#322 の最簡形。winget の ID は `Microsoft.EdgeWebView2Runtime`）
pub const INSTALL_COMMAND: &str = "winget install Microsoft.EdgeWebView2Runtime";

/// ローダーが「ランタイムが無い・互換の版が無い」で返す `HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND)`
pub const HRESULT_FILE_NOT_FOUND: u32 = 0x8007_0002;

// --- ユーザーデータの置き場 -----------------------------------------------------

/// 新しい置き場（`%LOCALAPPDATA%\tako\WebView2`。隔離・テストは data dir の下）
pub fn user_data_dir() -> Option<PathBuf> {
    crate::paths::local_data_dir().map(|d| d.join(DIR_NAME))
}

/// WebView2 が既定で作る置き場（`<実行ファイルの dir>\tako-app.exe.WebView2`）
pub fn legacy_dir_beside(exe_dir: &Path) -> PathBuf {
    exe_dir.join(format!("{APP_EXE}.WebView2"))
}

/// いま在る旧い置き場（移行の対象）。
///
/// 見るのは `tako-app.exe` / `tako.exe` が並ぶ dir（自分の実行ファイルの dir）と、
/// インストーラが記録した入れた先（開発ビルドの CLI から見ても製品の旧い置き場を拾う）。
/// **Windows 以外・隔離（`TAKO_DATA_DIR`）・検証のプロセス（テスト・`TAKO_ISOLATED` 等）は
/// 常に空** = 隔離した検証が本番の旧い置き場を動かさない
pub fn legacy_user_data_dirs() -> Vec<PathBuf> {
    if !cfg!(windows)
        || std::env::var_os("TAKO_DATA_DIR").is_some_and(|d| !d.is_empty())
        || crate::paths::is_verification_process()
    {
        return Vec::new();
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    legacy_candidates(
        exe_dir.as_deref(),
        crate::platform::install_info::install_location().as_deref(),
    )
    .into_iter()
    .filter(|dir| dir.is_dir())
    .collect()
}

/// 旧い置き場の候補（純関数。同じ dir は 1 つにまとめる）
pub fn legacy_candidates(exe_dir: Option<&Path>, install_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for dir in [exe_dir, install_dir].into_iter().flatten() {
        let candidate = legacy_dir_beside(dir);
        if !out.contains(&candidate) {
            out.push(candidate);
        }
    }
    out
}

/// Web ビューを作るときにどの置き場を渡すか
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataDirChoice {
    /// Windows 以外（WebContext を渡さない = 挙動を変えない）
    NotApplicable,
    /// 置き場が決まらない（`%LOCALAPPDATA%` も `%USERPROFILE%` も無い）。WebView2 の既定に任せる
    Unresolved,
    /// 新しい置き場
    New(PathBuf),
    /// 旧い置き場からの移行が済んでいない（使用中で移せなかった）ので旧い置き場を使う
    PendingLegacy(PathBuf),
}

impl DataDirChoice {
    /// wry の `WebContext` へ渡す置き場（None = 渡さない）
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::New(dir) | Self::PendingLegacy(dir) => Some(dir),
            Self::NotApplicable | Self::Unresolved => None,
        }
    }

    /// 機械可読の名前（check-health の `data_dir_choice`）
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotApplicable => "not_applicable",
            Self::Unresolved => "unresolved",
            Self::New(_) => "new",
            Self::PendingLegacy(_) => "pending_legacy",
        }
    }
}

/// いまの PC で Web ビューへ渡す置き場（Web ビューを作るたびに決め直す = 途中で移行が
/// 済めば次の Web ビューから新しい置き場になる）
pub fn data_dir_choice() -> DataDirChoice {
    if !cfg!(windows) {
        return DataDirChoice::NotApplicable;
    }
    let new = user_data_dir();
    let new_exists = new.as_deref().is_some_and(Path::is_dir);
    choose(new, new_exists, legacy_user_data_dirs().into_iter().next())
}

/// [`data_dir_choice`] の規則（純関数）。`legacy` は**いま在る**旧い置き場
pub fn choose(new: Option<PathBuf>, new_exists: bool, legacy: Option<PathBuf>) -> DataDirChoice {
    match (new, legacy) {
        (None, _) => DataDirChoice::Unresolved,
        (Some(new), _) if new_exists => DataDirChoice::New(new),
        (Some(_), Some(legacy)) => DataDirChoice::PendingLegacy(legacy),
        (Some(new), None) => DataDirChoice::New(new),
    }
}

// --- 旧い置き場からの移設 -------------------------------------------------------

/// 旧い置き場からの移設の結果（`apply = false` のときは「これから起きること」）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Relocation {
    /// 旧い置き場を丸ごと新しい置き場へ移した（同じボリュームの上書きしない rename =
    /// 中身は 1 バイトも変えず、途中で止まらない。旧い置き場の名前は消えるが何も消していない）
    Moved { to: PathBuf },
    /// 新しい置き場が既にあったので、旧い置き場は混ぜずに `.pre-v1.bak` へ退避した
    SetAside { backup: PathBuf },
    /// 別のボリュームだったので新しい置き場へ写し、旧い置き場を `.pre-v1.bak` へ退避した
    Copied { to: PathBuf, backup: PathBuf },
}

/// 移設できなかった理由。**どの場合も旧い置き場は無傷**（次の発火でまた試す）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelocateError {
    /// 使用中（Web ビューが開いていて msedgewebview2 がファイルを掴んでいる）
    InUse { from: PathBuf, detail: String },
    /// 新しい置き場の親を作れない
    CreateParent { dir: PathBuf, detail: String },
    /// 別のボリュームへ写せなかった（写しかけは消した）
    Copy { to: PathBuf, detail: String },
    /// 写し終えたが旧い置き場を退避できなかった（新しい置き場は使える。次の発火で退避する）
    SetAsideAfterCopy { from: PathBuf, detail: String },
    /// それ以外の失敗
    Other { from: PathBuf, detail: String },
}

impl RelocateError {
    /// 表示言語を明示しての文言（#2365 の型）
    pub fn text_in(&self, lang: Lang) -> String {
        match (self, lang) {
            (Self::InUse { from, detail }, Lang::Ja) => format!(
                "WebView2 のユーザーデータ（{}）が使用中なので移せない（Web ビューを閉じてから \
                 tako を起動し直すと移る。それまでは旧い置き場を使う）: {detail}",
                from.display()
            ),
            (Self::InUse { from, detail }, Lang::En) => format!(
                "the WebView2 user data ({}) is in use, so it was not moved (it moves once the web views \
                 are closed and tako is restarted; until then the old location stays in use): {detail}",
                from.display()
            ),
            (Self::CreateParent { dir, detail }, Lang::Ja) => format!(
                "WebView2 のユーザーデータの新しい置き場の親（{}）を作れない: {detail}",
                dir.display()
            ),
            (Self::CreateParent { dir, detail }, Lang::En) => format!(
                "cannot create the parent ({}) of the new WebView2 user data location: {detail}",
                dir.display()
            ),
            (Self::Copy { to, detail }, Lang::Ja) => format!(
                "WebView2 のユーザーデータを別のドライブの新しい置き場（{}）へ写せない\
                 （写しかけは消した・旧い置き場はそのまま）: {detail}",
                to.display()
            ),
            (Self::Copy { to, detail }, Lang::En) => format!(
                "cannot copy the WebView2 user data to the new location on another drive ({}) \
                 (the partial copy was removed; the old location is untouched): {detail}",
                to.display()
            ),
            (Self::SetAsideAfterCopy { from, detail }, Lang::Ja) => format!(
                "WebView2 のユーザーデータは新しい置き場へ写したが、旧い置き場（{}）を退避できない\
                 （次の起動で退避する）: {detail}",
                from.display()
            ),
            (Self::SetAsideAfterCopy { from, detail }, Lang::En) => format!(
                "the WebView2 user data was copied to the new location, but the old location ({}) \
                 could not be set aside (it will be on the next start): {detail}",
                from.display()
            ),
            (Self::Other { from, detail }, Lang::Ja) => format!(
                "WebView2 のユーザーデータ（{}）を移せない（旧い置き場はそのまま）: {detail}",
                from.display()
            ),
            (Self::Other { from, detail }, Lang::En) => format!(
                "cannot move the WebView2 user data ({}) (the old location is untouched): {detail}",
                from.display()
            ),
        }
    }
}

impl std::fmt::Display for RelocateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(crate::i18n::lang()))
    }
}

/// 旧い置き場 `from` を新しい置き場 `to` へ移す。`apply = false` は見るだけ（何も触らない）。
///
/// - `to` が無い: 上書きしない rename で丸ごと移す（[`Relocation::Moved`]）。別のボリュームなら
///   一時の名前へ写してから `to` へ据え、旧い置き場を `.pre-v1.bak` へ退避する
///   （[`Relocation::Copied`]。写しかけで失敗したら写しかけだけを消す）
/// - `to` が在る: 2 つの WebView2 のプロファイルは混ぜられないので新しい方を正とし、
///   旧い置き場を `.pre-v1.bak` へ退避する（[`Relocation::SetAside`]）
///
/// 旧い置き場の中身は**どの経路でも消さない**。冪等: 移した後は旧い置き場が無いので、
/// 呼び出し側（[`legacy_user_data_dirs`]）が対象に挙げない
pub fn relocate(from: &Path, to: &Path, apply: bool) -> Result<Relocation, RelocateError> {
    relocate_with(from, to, apply, &crate::platform::fs_move::rename_noreplace)
}

/// [`relocate`] の本体（rename を差し替えられる = 別のボリュームを macOS のテストで作る）
pub fn relocate_with(
    from: &Path,
    to: &Path,
    apply: bool,
    rename: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<Relocation, RelocateError> {
    if std::fs::symlink_metadata(to).is_ok() {
        return set_aside(from, apply, rename);
    }
    if !apply {
        return Ok(Relocation::Moved {
            to: to.to_path_buf(),
        });
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| RelocateError::CreateParent {
            dir: parent.to_path_buf(),
            detail: e.to_string(),
        })?;
    }
    match rename(from, to) {
        Ok(()) => Ok(Relocation::Moved {
            to: to.to_path_buf(),
        }),
        // 見てから移すまでのあいだに新しい置き場ができた（別の発火点が先に作った）
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => set_aside(from, apply, rename),
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => copy_across(from, to, rename),
        Err(e) => Err(move_error(from, e)),
    }
}

/// 旧い置き場を `.pre-v1.bak`（在れば `.pre-v1.bak.2` …）へ退避する
fn set_aside(
    from: &Path,
    apply: bool,
    rename: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<Relocation, RelocateError> {
    let backup = free_backup_path(from);
    if apply {
        rename(from, &backup).map_err(|e| move_error(from, e))?;
    }
    Ok(Relocation::SetAside { backup })
}

/// 別のボリュームへ写す（一時の名前へ写し終えてから `to` へ据える = `to` に写しかけを見せない）
fn copy_across(
    from: &Path,
    to: &Path,
    rename: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<Relocation, RelocateError> {
    let staging = sibling_with(to, &format!(".migrating-{}", std::process::id()));
    let copied = copy_tree(from, &staging).and_then(|()| rename(&staging, to));
    if let Err(e) = copied {
        // 消すのは自分が作った写しかけだけ（旧い置き場には触らない）
        let _ = std::fs::remove_dir_all(&staging);
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            return set_aside(from, true, rename);
        }
        return Err(if in_use(&e) {
            RelocateError::InUse {
                from: from.to_path_buf(),
                detail: e.to_string(),
            }
        } else {
            RelocateError::Copy {
                to: to.to_path_buf(),
                detail: e.to_string(),
            }
        });
    }
    let backup = free_backup_path(from);
    rename(from, &backup).map_err(|e| RelocateError::SetAsideAfterCopy {
        from: from.to_path_buf(),
        detail: e.to_string(),
    })?;
    Ok(Relocation::Copied {
        to: to.to_path_buf(),
        backup,
    })
}

/// フォルダを丸ごと写す（リンクは辿らず飛ばす = 旧い置き場の退避に残る）
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let dest = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &dest)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
}

fn move_error(from: &Path, e: std::io::Error) -> RelocateError {
    if in_use(&e) {
        RelocateError::InUse {
            from: from.to_path_buf(),
            detail: e.to_string(),
        }
    } else {
        RelocateError::Other {
            from: from.to_path_buf(),
            detail: e.to_string(),
        }
    }
}

/// 使用中で動かせない失敗か。Windows は開いているファイルを含むフォルダの rename を
/// `ERROR_ACCESS_DENIED`（5）/ `ERROR_SHARING_VIOLATION`（32）/ `ERROR_LOCK_VIOLATION`（33）で断る
fn in_use(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ResourceBusy
    ) || (cfg!(windows) && matches!(e.raw_os_error(), Some(32 | 33)))
}

/// 退避先（`<from>.pre-v1.bak`。在れば `.2` から空いている番号）
fn free_backup_path(from: &Path) -> PathBuf {
    let base = crate::migration::backup_path(from, 1);
    if std::fs::symlink_metadata(&base).is_err() {
        return base;
    }
    for n in 2..100u32 {
        let mut name = base.as_os_str().to_os_string();
        name.push(format!(".{n}"));
        let candidate = PathBuf::from(name);
        if std::fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
    }
    base
}

fn sibling_with(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

// --- ランタイムの版 -------------------------------------------------------------

/// WebView2 ランタイムの有無と版
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Runtime {
    /// Windows 以外（macOS の WKWebView は OS に含まれる）
    NotApplicable,
    /// 入っていない（レジストリに版が無い・空・`0.0.0.0`）
    Missing,
    /// 入っている版（[`REQUIRED_VERSION`] より古いこともある = [`Self::is_outdated`]）
    Installed { version: String },
}

impl Runtime {
    /// 入っている版が [`REQUIRED_VERSION`] より古いか（読めない版は古いと言わない）
    pub fn is_outdated(&self) -> bool {
        match self {
            Self::Installed { version } => older_than(version, REQUIRED_VERSION),
            Self::NotApplicable | Self::Missing => false,
        }
    }

    /// 機械可読の状態（check-health の `webview2.state`）
    pub fn state(&self) -> &'static str {
        match self {
            Self::NotApplicable => "not_applicable",
            Self::Missing => "missing",
            Self::Installed { .. } if self.is_outdated() => "outdated",
            Self::Installed { .. } => "installed",
        }
    }

    /// 入っている版
    pub fn version(&self) -> Option<&str> {
        match self {
            Self::Installed { version } => Some(version),
            Self::NotApplicable | Self::Missing => None,
        }
    }

    /// 診断の 1 行（check-health の診断項目・`tako setup --check`）。None = 対象外（Windows 以外）
    pub fn summary_in(&self, lang: Lang) -> Option<String> {
        let outdated = self.is_outdated();
        Some(match (self, lang) {
            (Self::NotApplicable, _) => return None,
            (Self::Missing, Lang::Ja) => {
                "WebView2 ランタイム: 無い（Web ビューを開けない）".to_string()
            }
            (Self::Missing, Lang::En) => {
                "WebView2 Runtime: not installed (web views cannot open)".to_string()
            }
            (Self::Installed { version }, Lang::Ja) if outdated => format!(
                "WebView2 ランタイム: {version}（古い。Web ビューには {REQUIRED_VERSION} 以上が要る）"
            ),
            (Self::Installed { version }, Lang::En) if outdated => format!(
                "WebView2 Runtime: {version} (too old; web views need {REQUIRED_VERSION} or later)"
            ),
            (Self::Installed { version }, Lang::Ja) => {
                format!("WebView2 ランタイム: {version}（Web ビューに要る版 {REQUIRED_VERSION} 以上）")
            }
            (Self::Installed { version }, Lang::En) => format!(
                "WebView2 Runtime: {version} (web views need {REQUIRED_VERSION} or later)"
            ),
        })
    }

    /// 入っていない・古いときの次の一手（check-health の issue・診断の続きの行）。
    /// 整っている・対象外なら None
    pub fn next_step_in(&self, lang: Lang) -> Option<String> {
        let outdated = self.is_outdated();
        Some(match (self, lang) {
            (Self::NotApplicable, _) => return None,
            (Self::Installed { .. }, _) if !outdated => return None,
            (Self::Missing, Lang::Ja) => format!(
                "WebView2 ランタイムが入っていないので Web ビューを開けない。{INSTALL_COMMAND} で入れる\
                 （または {DOWNLOAD_PAGE} の Evergreen Bootstrapper）。入れた後は tako の再起動は要らない（#2431）"
            ),
            (Self::Missing, Lang::En) => format!(
                "The WebView2 Runtime is not installed, so web views cannot open. Install it with \
                 {INSTALL_COMMAND} (or the Evergreen Bootstrapper at {DOWNLOAD_PAGE}); tako does not need \
                 a restart afterwards (#2431)"
            ),
            (Self::Installed { version }, Lang::Ja) => format!(
                "WebView2 ランタイムが古い（{version}・Web ビューには {REQUIRED_VERSION} 以上が要る）。\
                 {DOWNLOAD_PAGE} の Evergreen Bootstrapper を実行すると最新版へ上がる（#2431）"
            ),
            (Self::Installed { version }, Lang::En) => format!(
                "The WebView2 Runtime is too old ({version}; web views need {REQUIRED_VERSION} or later). \
                 Running the Evergreen Bootstrapper at {DOWNLOAD_PAGE} updates it to the latest version (#2431)"
            ),
        })
    }
}

/// いまの PC の WebView2 ランタイム（Windows はレジストリを読む。それ以外は対象外）
pub fn runtime() -> Runtime {
    if !cfg!(windows) {
        return Runtime::NotApplicable;
    }
    runtime_from_versions(&imp::registered_versions())
}

/// レジストリから読めた `pv` の値（機械単位・ユーザー単位）から決める（純関数）。
/// 空・`0.0.0.0`・数字でないものは「無い」として扱い、いちばん新しい版を採る
pub fn runtime_from_versions(found: &[Option<String>]) -> Runtime {
    found
        .iter()
        .flatten()
        .map(|v| v.trim())
        .filter_map(|v| parse_version(v).filter(|p| *p != [0; 4]).map(|p| (p, v)))
        .max_by_key(|(parsed, _)| *parsed)
        .map_or(Runtime::Missing, |(_, version)| Runtime::Installed {
            version: version.to_string(),
        })
}

/// `a.b.c.d` の版（足りない欄は 0）。数字でない欄があれば None
pub fn parse_version(text: &str) -> Option<[u32; 4]> {
    let mut out = [0u32; 4];
    let mut parts = text.split('.');
    for slot in out.iter_mut() {
        match parts.next() {
            Some(part) => *slot = part.parse().ok()?,
            None => break,
        }
    }
    if parts.next().is_some() || text.is_empty() {
        return None;
    }
    Some(out)
}

/// `installed` が `required` より古いか（どちらかが読めなければ偽 = 古いと言わない）
pub fn older_than(installed: &str, required: &str) -> bool {
    matches!(
        (parse_version(installed), parse_version(required)),
        (Some(a), Some(b)) if a < b
    )
}

// --- 生成の失敗の分類 -----------------------------------------------------------

/// Web ビューの生成の失敗の分類
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateFailure {
    /// ランタイムが無い（または登録はあるが使えない）
    RuntimeMissing,
    /// ランタイムが [`REQUIRED_VERSION`] より古い
    RuntimeOutdated { installed: String },
    /// それ以外
    Other,
}

impl CreateFailure {
    /// 機械可読の種別（`tako web list` / `read` の `create_failure`）
    pub fn kind(&self) -> &'static str {
        match self {
            Self::RuntimeMissing => "runtime_missing",
            Self::RuntimeOutdated { .. } => "runtime_outdated",
            Self::Other => "other",
        }
    }

    /// Web ビューのエラー表示・CLI / MCP の `error` の文言（`detail` は wry の原文）
    pub fn message_in(&self, lang: Lang, detail: &str) -> String {
        match (self, lang) {
            (Self::RuntimeMissing, Lang::Ja) => format!(
                "Web ビューを作れない: WebView2 ランタイムが見つからない。{INSTALL_COMMAND} で入れる\
                 （または {DOWNLOAD_PAGE} の Evergreen Bootstrapper）。入れたら再読み込みで開き直す\
                 （詳細: {detail}）"
            ),
            (Self::RuntimeMissing, Lang::En) => format!(
                "Cannot create the web view: the WebView2 Runtime was not found. Install it with \
                 {INSTALL_COMMAND} (or the Evergreen Bootstrapper at {DOWNLOAD_PAGE}), then reload to \
                 reopen it (details: {detail})"
            ),
            (Self::RuntimeOutdated { installed }, Lang::Ja) => format!(
                "Web ビューを作れない: WebView2 ランタイムが古い（入っている版 {installed}・\
                 {REQUIRED_VERSION} 以上が要る）。{DOWNLOAD_PAGE} の Evergreen Bootstrapper を実行して\
                 最新版へ上げたら、再読み込みで開き直す（詳細: {detail}）"
            ),
            (Self::RuntimeOutdated { installed }, Lang::En) => format!(
                "Cannot create the web view: the WebView2 Runtime is too old (installed {installed}; \
                 {REQUIRED_VERSION} or later is needed). Run the Evergreen Bootstrapper at {DOWNLOAD_PAGE} \
                 to update it, then reload to reopen it (details: {detail})"
            ),
            (Self::Other, Lang::Ja) => format!(
                "Web ビューを作れない（{detail}）。再読み込みで開き直す。続くなら tako check-health の \
                 webview2 で WebView2 ランタイムの版を確かめる"
            ),
            (Self::Other, Lang::En) => format!(
                "Cannot create the web view ({detail}). Reload to reopen it; if it keeps failing, check \
                 the WebView2 Runtime version under webview2 in tako check-health"
            ),
        }
    }

    /// いまの表示言語での [`Self::message_in`]
    pub fn message(&self, detail: &str) -> String {
        self.message_in(crate::i18n::lang(), detail)
    }
}

/// wry のエラーの原文から HRESULT を拾う。
///
/// wry の `Error::WebView2Error` の表示は webview2-com の `Error` の Debug
/// （`WindowsError(Error { code: HRESULT(0x80070002), message: "…" })`。HRESULT の Debug は
/// windows-result 0.4 の `HRESULT({:#010X})`）。拾えなければ None
pub fn hresult_in(text: &str) -> Option<u32> {
    let start = text.find("HRESULT(0x")? + "HRESULT(0x".len();
    let hex = text.get(start..start + 8)?;
    u32::from_str_radix(hex, 16).ok()
}

/// 生成の失敗を分ける（純関数）。
///
/// 入っている版がいちばん強い手がかり: 無ければ「無い」、[`REQUIRED_VERSION`] より古ければ「古い」。
/// 版が足りているのに `0x80070002`（ローダーが互換のランタイムを見つけられない）なら、登録は
/// あるが使えない状態なので「無い」と同じく入れ直しを案内する。それ以外は「その他」
pub fn classify(error_text: &str, runtime: &Runtime) -> CreateFailure {
    match runtime {
        Runtime::Missing => CreateFailure::RuntimeMissing,
        Runtime::Installed { version } if runtime.is_outdated() => CreateFailure::RuntimeOutdated {
            installed: version.clone(),
        },
        _ if hresult_in(error_text) == Some(HRESULT_FILE_NOT_FOUND) => {
            CreateFailure::RuntimeMissing
        }
        _ => CreateFailure::Other,
    }
}

/// いまの PC での分類（Windows 以外は None = 従来の文言のまま）
pub fn classify_here(error_text: &str) -> Option<CreateFailure> {
    if !cfg!(windows) {
        return None;
    }
    Some(classify(error_text, &runtime()))
}

// --- レジストリ（Windows） ------------------------------------------------------

#[cfg(windows)]
mod imp {
    //! Microsoft の文書（Distribute your app and the WebView2 Runtime の
    //! 「Detect if a WebView2 Runtime is already installed」）の検出の方法:
    //! Evergreen の client のキーの `pv`（REG_SZ）を機械単位（64 bit の Windows は
    //! WOW6432Node）とユーザー単位で読む。`windows-sys` を足さず必要な 3 関数だけ
    //! 宣言する方針は `platform::install_info` / `platform::user_path` と同じ

    const CLIENT: &str = r"Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    const HKEY_CURRENT_USER: isize = -2147483647; // 0x80000001
    const HKEY_LOCAL_MACHINE: isize = -2147483646; // 0x80000002
    const KEY_QUERY_VALUE: u32 = 0x0001;
    const ERROR_SUCCESS: i32 = 0;
    const REG_SZ: u32 = 1;

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegOpenKeyExW(
            hkey: isize,
            lpsubkey: *const u16,
            uloptions: u32,
            samdesired: u32,
            phkresult: *mut isize,
        ) -> i32;
        fn RegQueryValueExW(
            hkey: isize,
            lpvaluename: *const u16,
            lpreserved: *mut u32,
            lptype: *mut u32,
            lpdata: *mut u8,
            lpcbdata: *mut u32,
        ) -> i32;
        fn RegCloseKey(hkey: isize) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub(super) fn registered_versions() -> Vec<Option<String>> {
        vec![
            read_pv(
                HKEY_LOCAL_MACHINE,
                &format!(r"SOFTWARE\WOW6432Node\{CLIENT}"),
            ),
            read_pv(HKEY_LOCAL_MACHINE, &format!(r"SOFTWARE\{CLIENT}")),
            read_pv(HKEY_CURRENT_USER, &format!(r"Software\{CLIENT}")),
        ]
    }

    fn read_pv(root: isize, subkey: &str) -> Option<String> {
        let subkey = wide(subkey);
        let mut hkey: isize = 0;
        // SAFETY: subkey は NUL 終端の生存するバッファ、hkey はスタック上の変数
        let rc = unsafe { RegOpenKeyExW(root, subkey.as_ptr(), 0, KEY_QUERY_VALUE, &mut hkey) };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let value = read_string(hkey, "pv");
        // SAFETY: RegOpenKeyExW が成功したハンドルだけを閉じる
        unsafe { RegCloseKey(hkey) };
        value
    }

    /// 開いたキーから REG_SZ の値を読む。2 段階呼び出し（長さ問い合わせ → 実取得）
    fn read_string(hkey: isize, name: &str) -> Option<String> {
        let name_w = wide(name);
        let mut kind: u32 = 0;
        let mut len: u32 = 0;
        // SAFETY: 出力ポインタはスタック上の生存する変数。データバッファに null を
        // 渡すのは「必要なバイト数だけ問い合わせる」規定の呼び出し方
        let rc = unsafe {
            RegQueryValueExW(
                hkey,
                name_w.as_ptr(),
                std::ptr::null_mut(),
                &mut kind,
                std::ptr::null_mut(),
                &mut len,
            )
        };
        if rc != ERROR_SUCCESS || kind != REG_SZ || len == 0 {
            return None;
        }
        // len はバイト数。奇数長でも溢れないよう 1 要素ぶん余裕を持たせる
        let mut buf = vec![0u16; len as usize / 2 + 1];
        let mut got = (buf.len() * 2) as u32;
        // SAFETY: buf は got バイト分を確保済みで、そのバイト数を got に入れて渡している
        let rc = unsafe {
            RegQueryValueExW(
                hkey,
                name_w.as_ptr(),
                std::ptr::null_mut(),
                &mut kind,
                buf.as_mut_ptr() as *mut u8,
                &mut got,
            )
        };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let units = (got as usize / 2).min(buf.len());
        let end = buf[..units].iter().position(|&c| c == 0).unwrap_or(units);
        Some(String::from_utf16_lossy(&buf[..end]))
    }
}

#[cfg(not(windows))]
mod imp {
    pub(super) fn registered_versions() -> Vec<Option<String>> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 旧い置き場は実行ファイルの隣のtako_app_exe_webview2() {
        let dir = PathBuf::from(r"C:\Users\winuser\AppData\Local\Programs\tako");
        assert_eq!(legacy_dir_beside(&dir), dir.join("tako-app.exe.WebView2"));
        // 同じ dir（自分の実行ファイルの dir = 入れた先）は 1 つにまとめる
        assert_eq!(
            legacy_candidates(Some(&dir), Some(&dir)),
            vec![dir.join("tako-app.exe.WebView2")]
        );
        let dev = PathBuf::from(r"D:\src\tako\target\debug");
        assert_eq!(
            legacy_candidates(Some(&dev), Some(&dir)),
            vec![
                dev.join("tako-app.exe.WebView2"),
                dir.join("tako-app.exe.WebView2")
            ]
        );
        assert!(legacy_candidates(None, None).is_empty());
    }

    /// 検証のプロセス（このテスト自身）は旧い置き場を挙げない = 本番の旧い置き場を動かさない
    #[test]
    fn テストプロセスは旧い置き場を挙げない() {
        assert!(legacy_user_data_dirs().is_empty());
    }

    #[test]
    fn 新しい置き場はローカルの置き場の下のwebview2() {
        let dir = user_data_dir().expect("テストプロセスでは必ず解決する");
        assert_eq!(
            Some(dir),
            crate::paths::local_data_dir().map(|d| d.join("WebView2"))
        );
    }

    #[test]
    fn 置き場の選び方() {
        let new = PathBuf::from("/l/tako/WebView2");
        let legacy = PathBuf::from("/p/tako/tako-app.exe.WebView2");
        // 決まらない = WebView2 の既定に任せる
        assert_eq!(
            choose(None, false, Some(legacy.clone())),
            DataDirChoice::Unresolved
        );
        // 新しい置き場が在れば新しい方（旧い置き場が残っていても混ぜない）
        assert_eq!(
            choose(Some(new.clone()), true, Some(legacy.clone())),
            DataDirChoice::New(new.clone())
        );
        // 移行が済んでいない（使用中で移せなかった）あいだは旧い置き場を使い続ける
        assert_eq!(
            choose(Some(new.clone()), false, Some(legacy.clone())),
            DataDirChoice::PendingLegacy(legacy.clone())
        );
        // どちらも無ければ新しい置き場で始める
        assert_eq!(
            choose(Some(new.clone()), false, None),
            DataDirChoice::New(new.clone())
        );
        assert_eq!(DataDirChoice::New(new.clone()).path(), Some(new.as_path()));
        assert_eq!(DataDirChoice::Unresolved.path(), None);
        assert_eq!(DataDirChoice::NotApplicable.path(), None);
    }

    /// macOS では WebContext を渡さない（Web ビューの挙動を変えない）
    #[test]
    #[cfg(not(windows))]
    fn windows以外は置き場を渡さない() {
        assert_eq!(data_dir_choice(), DataDirChoice::NotApplicable);
        assert_eq!(runtime(), Runtime::NotApplicable);
        assert_eq!(classify_here("HRESULT(0x80070002)"), None);
    }

    /// Windows の Web ビューは新しい置き場（テストプロセスでは data dir の下の WebView2）を渡す
    #[test]
    #[cfg(windows)]
    fn windowsは新しい置き場を渡す() {
        assert_eq!(
            data_dir_choice(),
            DataDirChoice::New(user_data_dir().expect("解決する"))
        );
        // ランタイムの検出は対象外にならない（版の有無は実行環境しだい）
        let rt = runtime();
        println!("TAKO_2431: runtime = {rt:?}");
        assert_ne!(rt, Runtime::NotApplicable);
    }

    #[test]
    fn 版の読み方と比べ方() {
        assert_eq!(parse_version("145.0.3800.47"), Some([145, 0, 3800, 47]));
        assert_eq!(parse_version("146"), Some([146, 0, 0, 0]));
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("1.2.3.4.5"), None);
        assert_eq!(parse_version("abc"), None);
        assert!(older_than("144.0.3719.115", REQUIRED_VERSION));
        assert!(older_than("145.0.3800.46", REQUIRED_VERSION));
        assert!(!older_than("145.0.3800.47", REQUIRED_VERSION));
        assert!(!older_than("146.0.3856.84", REQUIRED_VERSION));
        // 読めない版は古いと言わない
        assert!(!older_than("unknown", REQUIRED_VERSION));
    }

    #[test]
    fn レジストリの値から有無と版を決める() {
        assert_eq!(runtime_from_versions(&[]), Runtime::Missing);
        assert_eq!(
            runtime_from_versions(&[None, Some(String::new()), Some("0.0.0.0".into())]),
            Runtime::Missing
        );
        // 機械単位とユーザー単位の両方にあれば新しい方
        assert_eq!(
            runtime_from_versions(&[
                Some("144.0.3719.115".into()),
                None,
                Some("146.0.3856.84".into())
            ]),
            Runtime::Installed {
                version: "146.0.3856.84".into()
            }
        );
        let old = Runtime::Installed {
            version: "120.0.2210.91".into(),
        };
        assert!(old.is_outdated());
        assert_eq!(old.state(), "outdated");
        assert_eq!(Runtime::Missing.state(), "missing");
        assert_eq!(Runtime::NotApplicable.state(), "not_applicable");
        assert_eq!(
            Runtime::Installed {
                version: "146.0.3856.84".into()
            }
            .state(),
            "installed"
        );
    }

    /// wry の原文（webview2-com の Error の Debug）から HRESULT を拾う
    #[test]
    fn hresultを拾う() {
        let raw = "WebView2 error: WindowsError(Error { code: HRESULT(0x80070002), \
                   message: \"指定されたファイルが見つかりません。\" })";
        assert_eq!(hresult_in(raw), Some(HRESULT_FILE_NOT_FOUND));
        assert_eq!(hresult_in("HRESULT(0x8007064E)"), Some(0x8007_064E));
        assert_eq!(hresult_in("WebView2 error: TaskCanceled"), None);
        assert_eq!(hresult_in("HRESULT(0x8007)"), None);
    }

    #[test]
    fn 生成の失敗を3つに分ける() {
        let not_found =
            "WebView2 error: WindowsError(Error { code: HRESULT(0x80070002), message: \"\" })";
        let other =
            "WebView2 error: WindowsError(Error { code: HRESULT(0x80004005), message: \"\" })";
        let fresh = Runtime::Installed {
            version: "146.0.3856.84".into(),
        };
        let old = Runtime::Installed {
            version: "120.0.2210.91".into(),
        };
        // ランタイムが無い: 原文によらず「無い」
        assert_eq!(
            classify(not_found, &Runtime::Missing),
            CreateFailure::RuntimeMissing
        );
        assert_eq!(
            classify(other, &Runtime::Missing),
            CreateFailure::RuntimeMissing
        );
        // 古い: 互換の版が無い（0x80070002）は入っている版で「古い」と分かる
        assert_eq!(
            classify(not_found, &old),
            CreateFailure::RuntimeOutdated {
                installed: "120.0.2210.91".into()
            }
        );
        // 版は足りているのに見つからない = 登録はあるが使えない → 入れ直し（「無い」と同じ案内）
        assert_eq!(classify(not_found, &fresh), CreateFailure::RuntimeMissing);
        // それ以外
        assert_eq!(classify(other, &fresh), CreateFailure::Other);
        assert_eq!(
            classify("WebView2 error: TaskCanceled", &fresh),
            CreateFailure::Other
        );
        assert_eq!(CreateFailure::RuntimeMissing.kind(), "runtime_missing");
        assert_eq!(
            CreateFailure::RuntimeOutdated {
                installed: String::new()
            }
            .kind(),
            "runtime_outdated"
        );
        assert_eq!(CreateFailure::Other.kind(), "other");
    }

    /// どの分類も次にやることを言い、原文を失わない（日英とも・絵文字なし）
    #[test]
    fn 失敗の文言は次の一手と原文を持つ() {
        let detail = "WebView2 error: HRESULT(0x80070002)";
        let cases = [
            (CreateFailure::RuntimeMissing, INSTALL_COMMAND),
            (
                CreateFailure::RuntimeOutdated {
                    installed: "120.0.2210.91".into(),
                },
                DOWNLOAD_PAGE,
            ),
            (CreateFailure::Other, "check-health"),
        ];
        for (failure, step) in cases {
            for lang in [Lang::Ja, Lang::En] {
                let text = failure.message_in(lang, detail);
                assert!(text.contains(step), "{failure:?} {lang:?}: {text}");
                assert!(
                    text.contains(detail),
                    "{failure:?} {lang:?} が原文を落とした"
                );
                assert!(!text.chars().any(crate::emoji::is_emoji), "{text}");
            }
            assert_ne!(
                failure.message_in(Lang::Ja, detail),
                failure.message_in(Lang::En, detail)
            );
        }
        let outdated = CreateFailure::RuntimeOutdated {
            installed: "120.0.2210.91".into(),
        };
        for lang in [Lang::Ja, Lang::En] {
            let text = outdated.message_in(lang, detail);
            assert!(text.contains("120.0.2210.91") && text.contains(REQUIRED_VERSION));
        }
    }

    #[test]
    fn 診断の行と次の一手() {
        assert_eq!(Runtime::NotApplicable.summary_in(Lang::Ja), None);
        assert_eq!(Runtime::NotApplicable.next_step_in(Lang::Ja), None);
        let fresh = Runtime::Installed {
            version: "146.0.3856.84".into(),
        };
        assert!(fresh
            .summary_in(Lang::Ja)
            .unwrap()
            .contains("146.0.3856.84"));
        assert_eq!(fresh.next_step_in(Lang::Ja), None);
        for lang in [Lang::Ja, Lang::En] {
            let missing = Runtime::Missing.next_step_in(lang).unwrap();
            assert!(missing.contains(INSTALL_COMMAND), "{missing}");
            let old = Runtime::Installed {
                version: "120.0.2210.91".into(),
            };
            let step = old.next_step_in(lang).unwrap();
            assert!(
                step.contains("120.0.2210.91") && step.contains(DOWNLOAD_PAGE),
                "{step}"
            );
            assert!(old.summary_in(lang).unwrap().contains(REQUIRED_VERSION));
        }
    }

    // --- 移設 -------------------------------------------------------------------

    /// 一時の置き場（片付けは `ScratchDir` が持つ = 失敗しても TMPDIR に残さない。#1312）
    fn scratch(name: &str) -> crate::test_residue::ScratchDir {
        crate::test_residue::ScratchDir::new(&format!("2431-{name}"))
    }

    /// 旧い置き場の形（WebView2 が作る `EBWebView/Default/…` を模す）
    fn legacy_profile(root: &Path) -> PathBuf {
        let from = root
            .join("Programs")
            .join("tako")
            .join("tako-app.exe.WebView2");
        let default = from.join("EBWebView").join("Default");
        std::fs::create_dir_all(&default).unwrap();
        std::fs::write(default.join("Cookies"), b"cookie-bytes").unwrap();
        std::fs::write(from.join("EBWebView").join("Local State"), b"{}").unwrap();
        from
    }

    fn read(path: &Path) -> Vec<u8> {
        std::fs::read(path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
    }

    #[test]
    fn 旧い置き場を丸ごと移し2回目は何もしない() {
        let scratch_move = scratch("move");
        let root = scratch_move.path();
        let from = legacy_profile(&root);
        let to = root.join("Local").join("tako").join("WebView2");
        // 見るだけ: 何も触らない
        assert_eq!(
            relocate(&from, &to, false),
            Ok(Relocation::Moved { to: to.clone() })
        );
        assert!(from.is_dir() && !to.exists(), "見るだけで書いた");
        // 当てる: 中身はそのまま新しい置き場へ（親も作る）
        assert_eq!(
            relocate(&from, &to, true),
            Ok(Relocation::Moved { to: to.clone() })
        );
        assert!(!from.exists());
        assert_eq!(read(&to.join("EBWebView/Default/Cookies")), b"cookie-bytes");
        assert_eq!(read(&to.join("EBWebView/Local State")), b"{}");
        // 2 回目: 旧い置き場が無いので対象に挙がらない（呼び手は在る dir だけを渡す）。
        // 渡されても新しい置き場を壊さない（旧い置き場が無い = rename が失敗するだけ）
        assert!(relocate(&from, &to, true).is_err());
        assert_eq!(read(&to.join("EBWebView/Default/Cookies")), b"cookie-bytes");
    }

    #[test]
    fn 新旧の両方があれば混ぜずに旧い方を退避する() {
        let scratch_both = scratch("both");
        let root = scratch_both.path();
        let from = legacy_profile(&root);
        let to = root.join("Local").join("tako").join("WebView2");
        std::fs::create_dir_all(to.join("EBWebView")).unwrap();
        std::fs::write(to.join("EBWebView").join("Local State"), b"new").unwrap();
        let backup = crate::migration::backup_path(&from, 1);
        assert_eq!(
            relocate(&from, &to, false),
            Ok(Relocation::SetAside {
                backup: backup.clone()
            })
        );
        assert!(from.is_dir(), "見るだけで書いた");
        assert_eq!(
            relocate(&from, &to, true),
            Ok(Relocation::SetAside {
                backup: backup.clone()
            })
        );
        assert!(!from.exists());
        assert_eq!(
            read(&backup.join("EBWebView/Default/Cookies")),
            b"cookie-bytes"
        );
        // 新しい置き場は触らない
        assert_eq!(read(&to.join("EBWebView/Local State")), b"new");
        // 退避先が埋まっていれば次の番号
        let again = legacy_profile(&root);
        assert_eq!(again, from);
        let second = PathBuf::from(format!("{}.2", backup.display()));
        assert_eq!(
            relocate(&from, &to, true),
            Ok(Relocation::SetAside {
                backup: second.clone()
            })
        );
        assert_eq!(
            read(&second.join("EBWebView/Default/Cookies")),
            b"cookie-bytes"
        );
    }

    /// 移せないとき（使用中・親を作れない）は旧い置き場が無傷で残る
    #[test]
    #[cfg(unix)]
    fn 移せないときは旧い置き場が無傷で残る() {
        use std::os::unix::fs::PermissionsExt;
        let scratch_fail = scratch("fail");
        let root = scratch_fail.path();
        let from = legacy_profile(&root);
        // 旧い置き場の親を読み取り専用にする = rename が断られる（Windows の使用中と同じ形の失敗）
        let parent = from.parent().unwrap().to_path_buf();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o555)).unwrap();
        let to = root.join("Local").join("tako").join("WebView2");
        let result = relocate(&from, &to, true);
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            matches!(result, Err(RelocateError::InUse { .. })),
            "{result:?}"
        );
        assert_eq!(
            read(&from.join("EBWebView/Default/Cookies")),
            b"cookie-bytes"
        );
        // 新しい置き場の親がファイル = 作れない
        let blocked = root.join("blocked");
        std::fs::write(&blocked, b"file").unwrap();
        let result = relocate(&from, &blocked.join("tako").join("WebView2"), true);
        assert!(
            matches!(result, Err(RelocateError::CreateParent { .. })),
            "{result:?}"
        );
        assert_eq!(
            read(&from.join("EBWebView/Default/Cookies")),
            b"cookie-bytes"
        );
        for lang in [Lang::Ja, Lang::En] {
            let text = result.as_ref().unwrap_err().text_in(lang);
            assert!(text.contains("WebView2"), "{text}");
        }
    }

    /// 別のボリューム: 写してから据え、旧い置き場を退避する。写しかけで失敗したら写しかけだけを消す
    #[test]
    fn 別のボリュームは写してから旧い置き場を退避する() {
        let scratch_cross = scratch("cross");
        let root = scratch_cross.path();
        let from = legacy_profile(&root);
        let to = root.join("Local").join("tako").join("WebView2");
        // 最初の rename（旧い置き場 → 新しい置き場）だけ別のボリュームとして断る
        let cross = |a: &Path, b: &Path| -> std::io::Result<()> {
            if a == from.as_path() && b == to.as_path() {
                return Err(std::io::Error::from(std::io::ErrorKind::CrossesDevices));
            }
            crate::platform::fs_move::rename_noreplace(a, b)
        };
        let backup = crate::migration::backup_path(&from, 1);
        assert_eq!(
            relocate_with(&from, &to, true, &cross),
            Ok(Relocation::Copied {
                to: to.clone(),
                backup: backup.clone()
            })
        );
        assert_eq!(read(&to.join("EBWebView/Default/Cookies")), b"cookie-bytes");
        assert_eq!(
            read(&backup.join("EBWebView/Default/Cookies")),
            b"cookie-bytes"
        );
        assert!(!from.exists());
        let leftovers: Vec<_> = std::fs::read_dir(to.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".migrating-"))
            .collect();
        assert!(leftovers.is_empty(), "写しかけが残った: {leftovers:?}");

        // 写しかけで失敗: 据える rename を断る → 写しかけは消え、旧い置き場は無傷
        let scratch_cross_fail = scratch("cross-fail");
        let root = scratch_cross_fail.path();
        let from = legacy_profile(&root);
        let to = root.join("Local").join("tako").join("WebView2");
        let refuse = |a: &Path, _b: &Path| -> std::io::Result<()> {
            if a == from.as_path() {
                return Err(std::io::Error::from(std::io::ErrorKind::CrossesDevices));
            }
            Err(std::io::Error::other("injected"))
        };
        let result = relocate_with(&from, &to, true, &refuse);
        assert!(
            matches!(result, Err(RelocateError::Copy { .. })),
            "{result:?}"
        );
        assert_eq!(
            read(&from.join("EBWebView/Default/Cookies")),
            b"cookie-bytes"
        );
        assert!(!to.exists());
        let staging: Vec<_> = std::fs::read_dir(to.parent().unwrap())
            .unwrap()
            .flatten()
            .collect();
        assert!(staging.is_empty(), "写しかけが残った");
    }
}
