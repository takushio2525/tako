//! `.app` バンドルを「置き場のパスを一度も空けずに」差し替える（抽象境界 B22。#1042）
//!
//! ## なぜ境界が要るか
//!
//! Dock のピン留めは `.app` への **file URL ブックマーク**（`com.apple.dock` の
//! `persistent-apps[].tile-data.book`）で持たれる。ブックマークは CNID（inode）を
//! 優先して解決し、パスは候補の 1 つでしかない。そのため
//!
//! 1. `mv /Applications/tako.app /Applications/tako.app.bak`
//! 2. 新版を `/Applications/tako.app` へ**新規に**コピー（別 inode）
//! 3. `rm -rf /Applications/tako.app.bak`
//!
//! という差し替えをすると、**手順 1 の瞬間に置き場が空になる**ので、追跡している側
//! （Dock）は「アプリが `.bak` へ移動した」としか読めず、自分の参照をそちらへ
//! 書き直す。手順 2 で新しい `tako.app` が現れても既に `.bak` へ張り付いており、
//! 手順 3 が**その実体を消す** → ピンが外れる（#1042。実測で決定的に再現）。
//!
//! ## 直し方は「置き場を一度も空けない」こと
//!
//! 新版を置き場の**隣**（同一ボリューム）へステージしてから
//! `renamex_np(2)` の `RENAME_SWAP` でアトミックに入れ替える。どの瞬間に観測しても
//! `/Applications/tako.app` は有効なバンドルで埋まっているので、追跡側が退避先へ
//! 逃げる余地が構造的に無い。
//!
//! さらに、標準的なバンドル（トップレベルが `Contents` だけ）では **`Contents/` だけを**
//! 入れ替える。こうすると `.app` 自体の inode が変わらないので、ブックマークは
//! **張り直しすら要らない**（実測: バンドルごとの入れ替えでは `isStale` が立つが、
//! `Contents` だけなら false のまま）。「Dock がどう再解決するか」に依存しない保証が
//! 得られるぶん、こちらを優先する。手段の順は
//! [`ReplaceStrategy::ContentsSwap`] → [`ReplaceStrategy::Swap`] →
//! [`ReplaceStrategy::MoveAside`]。
//!
//! 副次的に「入れ替えの手前で失敗しても旧アプリが壊れない」も得られる。旧実装は
//! 退避のあとで複製が失敗すると、**壊れたバンドルと `tako.app.bak` が残る**
//! （復旧の rename は置き場が空でないと通らないため。実測で確認）。
//!
//! 判断そのものは [`ReplaceStrategy`] として返すので、**どの手段を通ったかを
//! 呼び出し側・テスト・診断ログから確認できる**。

use std::path::{Path, PathBuf};

use crate::i18n::Lang;

/// 差し替えの断り（#2365）。日英の対は [`BundleError::text_in`] の (バリアント, 言語) の網羅 match の
/// 1 か所。日本語は #2365 より前の固定文とバイト一致（`tako-control/tests/issue1762_dispatch_error_lang.rs`
/// が突き合わせる）。呼び手は GUI の更新（`UpdateRefusal::BundleReplace`）だけなので、日本語の
/// `String` の口は持たない。OS の理由（`error`）は io のエラーの文をそのまま載せる。
/// 日本語の「見つかりません」は 404 へ寄せる読みに当たらないので、英語も ` not found` を使わない
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleError {
    /// 差し替え元（展開した新版）が無い
    SourceMissing(String),
    /// 置き場のパスにファイル名が無い
    DestInvalid(String),
    /// 置き場が空のときの rename 1 回が失敗した（置き場は変えていない）
    InstallFailed { dest: String, error: String },
    /// 入れ替え（`RENAME_SWAP`）が失敗した（置き場は変えていない）
    SwapFailed { dest: String, cause: SwapCause },
    /// #1042 前の手順（A/B）で置き場を `.bak` へ退避できない
    BackupFailed { dest: String, error: String },
    /// 退避 → 設置へ落ちたときに置き場を退避できない（置き場は変えていない）
    AsideFailed { dest: String, error: String },
    /// 退避 → 設置へ落ちたときに新版を置けない（退避は戻す）
    PlaceFailed { dest: String, error: String },
    /// 置き場の親ディレクトリが取れない
    ParentMissing(String),
    /// 置き場の隣に作業用ディレクトリを作れない
    StagingCreateFailed { parent: String, error: String },
    /// `ditto` を起動できない（macOS）
    DittoSpawn(String),
    /// バンドルの複製が失敗した（macOS は `ditto` の stderr・その他は io のエラー）
    CopyFailed(String),
}

/// 入れ替えが失敗した理由（[`BundleError::SwapFailed`] の中身）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwapCause {
    /// パスを C の文字列へ渡せない（NUL を含む）
    NulInPath,
    /// OS の理由（io のエラーの文）
    Os(String),
}

impl SwapCause {
    /// 表示言語を明示しての文言
    pub fn text_in(&self, lang: Lang) -> String {
        match (self, lang) {
            (Self::NulInPath, Lang::Ja) => "パスに NUL が含まれています".to_string(),
            (Self::NulInPath, Lang::En) => "the path contains a NUL byte".to_string(),
            (Self::Os(error), Lang::Ja | Lang::En) => error.clone(),
        }
    }
}

impl BundleError {
    /// 表示言語を明示しての文言（網羅 match = バリアントを足すと日英の両方を書くまで通らない）
    pub fn text_in(&self, lang: Lang) -> String {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::SourceMissing(path), Ja) => format!("差し替え元が見つかりません: {path}"),
            (Self::SourceMissing(path), En) => {
                format!("the replacement bundle does not exist: {path}")
            }
            (Self::DestInvalid(dest), Ja) => format!("置き場のパスが不正です: {dest}"),
            (Self::DestInvalid(dest), En) => format!("the install path is invalid: {dest}"),
            (Self::InstallFailed { dest, error }, Ja) => {
                format!("{dest} への設置に失敗: {error}（{dest} は変更していません）")
            }
            (Self::InstallFailed { dest, error }, En) => {
                format!("installing to {dest} failed: {error} ({dest} was not changed)")
            }
            (Self::SwapFailed { dest, cause }, Ja) => format!(
                "{dest} の入れ替えに失敗: {}（{dest} は変更していません）",
                cause.text_in(Ja)
            ),
            (Self::SwapFailed { dest, cause }, En) => format!(
                "swapping in the new {dest} failed: {} ({dest} was not changed)",
                cause.text_in(En)
            ),
            (Self::BackupFailed { dest, error }, Ja) => {
                format!("{dest} のバックアップに失敗: {error}")
            }
            (Self::BackupFailed { dest, error }, En) => {
                format!("backing up {dest} failed: {error}")
            }
            (Self::AsideFailed { dest, error }, Ja) => {
                format!("{dest} の退避に失敗: {error}（{dest} は変更していません）")
            }
            (Self::AsideFailed { dest, error }, En) => {
                format!("moving {dest} aside failed: {error} ({dest} was not changed)")
            }
            (Self::PlaceFailed { dest, error }, Ja) => format!("{dest} への設置に失敗: {error}"),
            (Self::PlaceFailed { dest, error }, En) => {
                format!("installing to {dest} failed: {error}")
            }
            (Self::ParentMissing(dest), Ja) => {
                format!("置き場の親ディレクトリが取れません: {dest}")
            }
            (Self::ParentMissing(dest), En) => {
                format!("cannot get the parent directory of the install path: {dest}")
            }
            (Self::StagingCreateFailed { parent, error }, Ja) => {
                format!("作業用ディレクトリの作成に失敗: {error}（{parent} に書き込めますか）")
            }
            (Self::StagingCreateFailed { parent, error }, En) => format!(
                "creating the work directory failed: {error} (check that {parent} is writable)"
            ),
            (Self::DittoSpawn(error), Ja) => format!("ditto の実行に失敗: {error}"),
            (Self::DittoSpawn(error), En) => format!("cannot run ditto: {error}"),
            (Self::CopyFailed(detail), Ja) => format!("バンドルの複製に失敗: {detail}"),
            (Self::CopyFailed(detail), En) => format!("copying the bundle failed: {detail}"),
        }
    }
}

impl std::fmt::Display for BundleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(crate::i18n::lang()))
    }
}

impl std::error::Error for BundleError {}

/// 差し替えに使った手段（診断・テスト用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplaceStrategy {
    /// `Contents/` だけを `RENAME_SWAP` で入れ替えた。**`.app` 自体の inode が変わらない**
    /// ので、ブックマークは張り直しすら要らない（実測で `isStale` が false のまま）
    ContentsSwap,
    /// バンドルごと `RENAME_SWAP` でアトミックに入れ替えた。**置き場が一度も空かない**
    Swap,
    /// 置き場が空だったので rename 1 回で置いた（入れ替える相手が居ない）
    FreshInstall,
    /// swap が使えない環境なので「退避 → 設置」へ落ちた。**置き場が一瞬空く**
    MoveAside,
}

impl ReplaceStrategy {
    /// 置き場のパスが途中で一度も空かない手段か（= ブックマークが逃げない）
    pub fn keeps_path_occupied(self) -> bool {
        !matches!(self, ReplaceStrategy::MoveAside)
    }

    /// `.app` 自体の identity（inode）まで保つ手段か
    pub fn keeps_bundle_identity(self) -> bool {
        matches!(self, ReplaceStrategy::ContentsSwap)
    }

    /// persist.log 等へ出す短いラベル
    pub fn label(self) -> &'static str {
        match self {
            ReplaceStrategy::ContentsSwap => "contents-swap",
            ReplaceStrategy::Swap => "swap",
            ReplaceStrategy::FreshInstall => "fresh",
            ReplaceStrategy::MoveAside => "move-aside",
        }
    }
}

/// `new_bundle` の中身で `dest` を差し替える。
///
/// - `dest` が既に在れば `RENAME_SWAP` で入れ替える（置き場を空けない）
/// - `dest` が無ければ rename 1 回で置く
/// - `RENAME_SWAP` が使えない環境では「退避 → 設置」へ落ちる（旧挙動）
///
/// 成功しても失敗しても、作業用に作ったステージ用ディレクトリは片付ける。
/// **入れ替えの手前で失敗したときは `dest` に一切触っていない**。
pub fn replace_bundle_in_place(
    dest: &Path,
    new_bundle: &Path,
) -> Result<ReplaceStrategy, BundleError> {
    if !new_bundle.exists() {
        return Err(BundleError::SourceMissing(new_bundle.display().to_string()));
    }
    if legacy_move_aside_forced() {
        legacy_replace(dest, new_bundle)?;
        return Ok(ReplaceStrategy::MoveAside);
    }
    let name = dest
        .file_name()
        .ok_or_else(|| BundleError::DestInvalid(dest.display().to_string()))?
        .to_os_string();

    // 隣にステージする（同一ボリュームであることが rename / swap の前提）
    let staging = StagingDir::create_next_to(dest)?;
    let staged = staging.dir().join(&name);
    stage_bundle(new_bundle, &staged)?;

    if !dest.exists() {
        // 置き場が空 → rename 1 回でアトミックに置ける（窓ゼロ）
        std::fs::rename(&staged, dest).map_err(|e| BundleError::InstallFailed {
            dest: dest.display().to_string(),
            error: e.to_string(),
        })?;
        return Ok(ReplaceStrategy::FreshInstall);
    }

    // まず `Contents/` だけの入れ替えを試す。成功すれば `.app` の inode が変わらないので、
    // Dock のブックマークは張り直しすら要らない（whole-bundle の入れ替えでは
    // `isStale` が立つ = いずれ張り直しが要る）
    if contents_swap_applies(dest, &staged) {
        match imp::swap_paths(&dest.join(BUNDLE_CONTENTS), &staged.join(BUNDLE_CONTENTS)) {
            Ok(()) => return Ok(ReplaceStrategy::ContentsSwap),
            Err(e) => {
                // バンドルごとの入れ替えへ落ちる。`Contents` は入れ替わっていないので
                // 置き場は壊れていない
                tracing::warn!(
                    "Contents の入れ替えに失敗したのでバンドルごと入れ替えます: {}",
                    e.cause().text_in(Lang::Ja)
                );
            }
        }
    }

    match imp::swap_paths(dest, &staged) {
        Ok(()) => return Ok(ReplaceStrategy::Swap),
        Err(e) if e.unsupported => {
            // 落ちる先は旧挙動。更新できないより「ピンが外れるかもしれない」を選ぶ
            tracing::warn!(
                "アトミックな入れ替えが使えないため退避 → 設置へ落ちます: {}",
                e.cause().text_in(Lang::Ja)
            );
        }
        Err(e) => {
            return Err(BundleError::SwapFailed {
                dest: dest.display().to_string(),
                cause: e.cause(),
            });
        }
    }

    move_aside_replace(dest, &staged, staging.dir())?;
    Ok(ReplaceStrategy::MoveAside)
}

/// #1042 の修正を入れる前へ戻す逃げ道（`TAKO_1042_LEGACY=1`）。A/B 計測専用
fn legacy_move_aside_forced() -> bool {
    std::env::var_os("TAKO_1042_LEGACY").is_some()
}

/// #1042 前の手順を**そのまま**再現する（A/B 計測用）。
///
/// 退避 → 置き場へ複製 → 退避先を削除。置き場が空いているのは**複製にかかる時間
/// まるごと**（41MB のバンドルで実測 1〜2ms。APFS は複製を clone で済ませるので
/// バイト数ほどには伸びない）。その間に追跡側が観測すると退避先へ張り付き、
/// 最後の削除でピンが外れる。
///
/// 複製が途中で失敗すると、**壊れたバンドルと退避先の両方が残る**（復旧の rename は
/// 置き場が空でないと通らないため）。これも当時の挙動そのまま
fn legacy_replace(dest: &Path, new_bundle: &Path) -> Result<(), BundleError> {
    // 当時のリテラルと同じ `<置き場>.bak`
    let backup = PathBuf::from(format!("{}.bak", dest.display()));
    if dest.exists() {
        let _ = std::fs::remove_dir_all(&backup);
        std::fs::rename(dest, &backup).map_err(|e| BundleError::BackupFailed {
            dest: dest.display().to_string(),
            error: e.to_string(),
        })?;
    }
    if let Err(e) = imp::copy_bundle(new_bundle, dest) {
        if backup.exists() {
            let _ = std::fs::rename(&backup, dest);
        }
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(&backup);
    Ok(())
}

/// 新版をステージ用ディレクトリへ運ぶ。
///
/// 同一ボリュームなら rename で一瞬（コピーが要らない）。またいでいるときだけ
/// 実コピーへ落ちる（macOS は署名・拡張属性を保つ `ditto`）。
fn stage_bundle(new_bundle: &Path, staged: &Path) -> Result<(), BundleError> {
    if std::fs::rename(new_bundle, staged).is_ok() {
        return Ok(());
    }
    imp::copy_bundle(new_bundle, staged)
}

/// 旧挙動: 置き場を退避してから新版を設置する。**置き場が一瞬空く**
fn move_aside_replace(dest: &Path, staged: &Path, staging_dir: &Path) -> Result<(), BundleError> {
    let aside = staging_dir.join("previous");
    std::fs::rename(dest, &aside).map_err(|e| BundleError::AsideFailed {
        dest: dest.display().to_string(),
        error: e.to_string(),
    })?;
    if let Err(e) = std::fs::rename(staged, dest) {
        // 設置に失敗したら退避を戻す（best-effort。ここで失敗すると置き場が空のまま残る）
        let _ = std::fs::rename(&aside, dest);
        return Err(BundleError::PlaceFailed {
            dest: dest.display().to_string(),
            error: e.to_string(),
        });
    }
    Ok(())
}

/// 置き場の隣に作る作業用ディレクトリ。drop で必ず消える
struct StagingDir {
    dir: PathBuf,
}

impl StagingDir {
    fn create_next_to(dest: &Path) -> Result<Self, BundleError> {
        let parent = dest
            .parent()
            .ok_or_else(|| BundleError::ParentMissing(dest.display().to_string()))?;
        // Finder に見えないようドット始まり。同時実行と残骸の衝突を避けるため pid を混ぜる。
        //
        // ここは `/Applications` の直下なので、#837 が実測した「Launch Services は
        // ディスク上の .app を自力で拾う（隠しディレクトリ配下でも約 133 秒後に登録された）」
        // に触れる。ただしこのディレクトリが `.app` を抱えているのは**入れ替えのあいだだけ**
        // （同一ボリュームなら rename なのでミリ秒級）で、成否によらず drop で消えるため、
        // 登録される窓へ届かない。#837 の「Finder の候補に tako が 2 つ並ぶ」は起こさない
        let dir = parent.join(format!(".tako-replace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| BundleError::StagingCreateFailed {
            parent: parent.display().to_string(),
            error: e.to_string(),
        })?;
        Ok(Self { dir })
    }

    fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// 入れ替えの失敗。`unsupported` は「この環境には手段が無い」= 落とし先へ進んでよい、
/// それ以外は「あるはずの手段が失敗した」= 置き場に触らず止める、を区別する。
///
/// enum ではなく構造体にしてあるのは、macOS 以外では片方の値しか作られず
/// 「never constructed」の警告になるため（プラットフォームで死ぬ変種を作らない）
struct SwapError {
    unsupported: bool,
    /// パスを C の文字列へ渡せなかった（NUL を含む）。真なら `source` は使わない
    nul_in_path: bool,
    source: std::io::Error,
}

impl SwapError {
    /// 断りへ載せる理由（#2365。文は [`SwapCause::text_in`] で引く）
    fn cause(&self) -> SwapCause {
        if self.nul_in_path {
            SwapCause::NulInPath
        } else {
            SwapCause::Os(self.source.to_string())
        }
    }
}

/// `.app` バンドルの中身が入っているディレクトリ
const BUNDLE_CONTENTS: &str = "Contents";

/// `Contents/` だけの入れ替えが使えるか。
///
/// 両方が**標準的なバンドルの形**（トップレベルが `Contents` ただ 1 つ）のときだけ。
/// 置き場に余分なトップレベル項目があると、`Contents` だけ替えてもそれが残ってしまう
/// ので、そのときはバンドルごと入れ替える側に倒す
fn contents_swap_applies(dest: &Path, staged: &Path) -> bool {
    has_only_contents(dest) && has_only_contents(staged)
}

fn has_only_contents(bundle: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(bundle) else {
        return false;
    };
    let mut names: Vec<_> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    names == [BUNDLE_CONTENTS] && bundle.join(BUNDLE_CONTENTS).is_dir()
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{BundleError, SwapError};
    use std::path::Path;

    /// 2 つのパスの中身をアトミックに入れ替える（`renamex_np` + `RENAME_SWAP`）
    pub(super) fn swap_paths(a: &Path, b: &Path) -> Result<(), SwapError> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let to_c = |p: &Path| {
            CString::new(p.as_os_str().as_bytes()).map_err(|_| SwapError {
                unsupported: false,
                nul_in_path: true,
                source: std::io::Error::from(std::io::ErrorKind::InvalidInput),
            })
        };
        let ca = to_c(a)?;
        let cb = to_c(b)?;
        // SAFETY: どちらも NUL 終端の有効なポインタで、呼び出し中だけ借りる
        let rc = unsafe { libc::renamex_np(ca.as_ptr(), cb.as_ptr(), libc::RENAME_SWAP) };
        if rc == 0 {
            return Ok(());
        }
        let err = std::io::Error::last_os_error();
        // ファイルシステムが swap を持たない / この組み合わせでは使えない
        let unsupported = matches!(
            err.raw_os_error(),
            Some(libc::ENOTSUP) | Some(libc::EINVAL) | Some(libc::ENOSYS)
        );
        Err(SwapError {
            unsupported,
            nul_in_path: false,
            source: err,
        })
    }

    /// 署名・拡張属性を保ったままバンドルを複製する（`ditto`）
    pub(super) fn copy_bundle(src: &Path, dest: &Path) -> Result<(), BundleError> {
        // #628 / #586: GUI プロセスから呼ばれうるのでコンソール窓を出させない
        // （unix では no-op だが、境界の規約を全ファイルで揃えておく）
        let output = crate::platform::process::no_console_window(&mut std::process::Command::new(
            "/usr/bin/ditto",
        ))
        .arg(src)
        .arg(dest)
        .output()
        .map_err(|e| BundleError::DittoSpawn(e.to_string()))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(BundleError::CopyFailed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::{BundleError, SwapError};
    use std::path::Path;

    /// 「使えない」の理由。落とし先へ進むときの診断（`tracing::warn!`）にだけ出る = 断りではない
    const NO_ATOMIC_SWAP: &str = "このプラットフォームにアトミックな入れ替えはありません";

    /// macOS 以外に `RENAME_SWAP` 相当は無いので、常に「使えない」を返して
    /// 呼び出し側を退避 → 設置へ落とす
    pub(super) fn swap_paths(_a: &Path, _b: &Path) -> Result<(), SwapError> {
        Err(SwapError {
            unsupported: true,
            nul_in_path: false,
            source: std::io::Error::new(std::io::ErrorKind::Unsupported, NO_ATOMIC_SWAP),
        })
    }

    /// 素の再帰コピー（拡張属性は扱わない）
    pub(super) fn copy_bundle(src: &Path, dest: &Path) -> Result<(), BundleError> {
        copy_dir_all(src, dest).map_err(|e| BundleError::CopyFailed(e.to_string()))
    }

    fn copy_dir_all(src: &Path, dest: &Path) -> std::io::Result<()> {
        if src.is_file() {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(src, dest)?;
            return Ok(());
        }
        std::fs::create_dir_all(dest)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_dir_all(&entry.path(), &dest.join(entry.file_name()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `TAKO_1042_LEGACY` を立てて、panic しても必ず戻す。
    ///
    /// env はプロセスに 1 つなので、呼ぶのは `run_alone!` を通したその 1 本だけの子の中だけ（#2400。
    /// 以前はこのモジュールの `Mutex` で囲んでいたが、それは同じロックを取るテストしか止めない）
    struct LegacyEnv;

    impl LegacyEnv {
        fn set() -> Self {
            crate::test_child::assert_alone("LegacyEnv::set");
            // SAFETY: 子のプロセスにはこのテストしか居ない（上の assert_alone）
            unsafe { std::env::set_var("TAKO_1042_LEGACY", "1") };
            Self
        }
    }

    impl Drop for LegacyEnv {
        fn drop(&mut self) {
            // tako:env-write-ok 戻すだけ（作る側の set が assert_alone で子の中を確かめている）
            // SAFETY: 同上
            unsafe { std::env::remove_var("TAKO_1042_LEGACY") };
        }
    }

    struct TempRoot {
        dir: PathBuf,
    }

    impl TempRoot {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "tako-1042-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("一時ディレクトリ");
            Self { dir }
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            // 一時ディレクトリ配下であることを確かめてから消す（実環境破壊の防止）
            if self.dir.starts_with(std::env::temp_dir()) {
                let _ = std::fs::remove_dir_all(&self.dir);
            }
        }
    }

    /// `.app` らしき最小のバンドルを作る
    fn make_bundle(path: &Path, version: &str) {
        std::fs::create_dir_all(path.join("Contents/MacOS")).expect("バンドル");
        std::fs::write(path.join("Contents/version.txt"), version).expect("版数");
    }

    fn version_of(path: &Path) -> String {
        std::fs::read_to_string(path.join("Contents/version.txt")).unwrap_or_default()
    }

    #[cfg(unix)]
    fn inode_of(path: &Path) -> u64 {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).expect("メタデータ").ino()
    }

    /// **macOS 限定**（#1278）。この検査は `ReplaceStrategy::ContentsSwap` =
    /// `renamex_np(2)` の `RENAME_SWAP` を選ぶことを見るが、
    /// 非 macOS の `imp::swap_paths` は**設計上つねに `unsupported`** を返す
    /// （「macOS 以外に `RENAME_SWAP` 相当は無いので退避 → 設置へ落とす」）。
    /// したがって非 macOS では `MoveAside` が正しい答えで、
    /// `keeps_path_occupied()` も false が正しい = 検査の前提そのものが macOS 形。
    /// `.app` バンドルと Dock のブックマークも macOS の概念
    #[test]
    #[cfg(target_os = "macos")]
    fn 既存を差し替えても置き場のパスは空にならない() {
        let root = TempRoot::new("replace");
        let dest = root.dir.join("Applications/tako.app");
        let new = root.dir.join("new/tako.app");
        make_bundle(&dest, "0.8.0");
        make_bundle(&new, "0.8.1");

        let strategy = replace_bundle_in_place(&dest, &new).expect("差し替え");
        assert!(
            strategy.keeps_path_occupied(),
            "置き場を空けない手段を選ぶこと: {strategy:?}"
        );
        assert_eq!(
            strategy,
            ReplaceStrategy::ContentsSwap,
            "標準的なバンドルなら Contents だけの入れ替えを選ぶこと"
        );
        assert_eq!(version_of(&dest), "0.8.1", "新版になっていること");
        assert!(
            !root.dir.join("Applications/tako.app.bak").exists(),
            "退避先を残さないこと"
        );
    }

    #[test]
    fn 差し替えのあと作業用ディレクトリが残らない() {
        let root = TempRoot::new("staging");
        let apps = root.dir.join("Applications");
        let dest = apps.join("tako.app");
        let new = root.dir.join("new/tako.app");
        make_bundle(&dest, "0.8.0");
        make_bundle(&new, "0.8.1");

        replace_bundle_in_place(&dest, &new).expect("差し替え");

        let leftovers: Vec<_> = std::fs::read_dir(&apps)
            .expect("読み取り")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != "tako.app")
            .collect();
        assert!(leftovers.is_empty(), "余計な残骸がある: {leftovers:?}");
    }

    #[test]
    fn 置き場が空なら新規に置ける() {
        let root = TempRoot::new("fresh");
        let dest = root.dir.join("Applications/tako.app");
        std::fs::create_dir_all(dest.parent().unwrap()).expect("親");
        let new = root.dir.join("new/tako.app");
        make_bundle(&new, "0.8.1");

        let strategy = replace_bundle_in_place(&dest, &new).expect("設置");
        assert_eq!(strategy, ReplaceStrategy::FreshInstall);
        assert_eq!(version_of(&dest), "0.8.1");
    }

    #[test]
    fn 差し替え元が無ければ置き場に触らない() {
        let root = TempRoot::new("missing-src");
        let dest = root.dir.join("Applications/tako.app");
        make_bundle(&dest, "0.8.0");
        let missing = root.dir.join("new/tako.app");

        let err = replace_bundle_in_place(&dest, &missing).expect_err("失敗すること");
        assert!(
            matches!(&err, BundleError::SourceMissing(p) if p == &missing.display().to_string()),
            "理由が分かる断り: {err:?}"
        );
        assert!(
            err.text_in(Lang::Ja).contains("差し替え元"),
            "理由が分かる文面: {err:?}"
        );
        assert_eq!(version_of(&dest), "0.8.0", "旧版が生きていること");
    }

    /// #1042 の核心: swap 経路では置き場の inode が「退避先へ移動して消える」形にならない。
    ///
    /// Dock のピン（ブックマーク）は CNID を追うので、旧 inode が
    /// **後で削除されるパス**へ移ると外れる。swap 後の旧 inode は作業用ディレクトリ側に
    /// 居るが、置き場は最初から最後まで有効なバンドルで埋まっている
    #[cfg(unix)]
    #[test]
    fn swap経路では置き場が常に有効なバンドルで埋まっている() {
        let root = TempRoot::new("occupied");
        let dest = root.dir.join("Applications/tako.app");
        let new = root.dir.join("new/tako.app");
        make_bundle(&dest, "0.8.0");
        make_bundle(&new, "0.8.1");
        let before = inode_of(&dest);

        let strategy = replace_bundle_in_place(&dest, &new).expect("差し替え");

        assert!(dest.exists(), "置き場が残っていること");
        assert_eq!(version_of(&dest), "0.8.1");
        if strategy == ReplaceStrategy::ContentsSwap {
            assert_eq!(
                inode_of(&dest),
                before,
                "Contents だけの入れ替えなら .app 自体の inode は変わらない\
                 （= Dock のブックマークが張り直しすら要らない）"
            );
        }
    }

    /// トップレベルに余分なものが在るバンドルは Contents だけ替えると残骸が残るので、
    /// バンドルごとの入れ替えへ倒す。
    ///
    /// **macOS 限定**（#1278。`既存を差し替えても置き場のパスは空にならない` と同じ理由）。
    /// `ReplaceStrategy::Swap` は `RENAME_SWAP` を前提にしており、
    /// 非 macOS では `imp::swap_paths` が設計どおり `unsupported` を返す
    #[test]
    #[cfg(target_os = "macos")]
    fn 標準的でないバンドルはバンドルごと入れ替える() {
        let root = TempRoot::new("nonstandard");
        let dest = root.dir.join("Applications/tako.app");
        let new = root.dir.join("new/tako.app");
        make_bundle(&dest, "0.8.0");
        make_bundle(&new, "0.8.1");
        // 旧版にだけ在る「トップレベルの余り物」。Contents だけ替えると残ってしまう
        std::fs::write(dest.join("LegacyIcon"), b"x").expect("余り物");

        let strategy = replace_bundle_in_place(&dest, &new).expect("差し替え");
        assert_eq!(strategy, ReplaceStrategy::Swap);
        assert!(strategy.keeps_path_occupied());
        assert_eq!(version_of(&dest), "0.8.1");
        assert!(
            !dest.join("LegacyIcon").exists(),
            "バンドルごと入れ替えたので余り物は残らない"
        );
    }

    /// 旧挙動（`TAKO_1042_LEGACY=1`）は「置き場を空ける」手段を選ぶ。
    /// A/B 計測の逃げ道が生きていることの固定
    #[test]
    fn 旧挙動へ戻す逃げ道が効く() {
        // env を書き換えるので、この 1 本だけの子で走らせる（#2400）
        if crate::run_alone!(旧挙動へ戻す逃げ道が効く) {
            return;
        }
        let root = TempRoot::new("legacy");
        let dest = root.dir.join("Applications/tako.app");
        let new = root.dir.join("new/tako.app");
        make_bundle(&dest, "0.8.0");
        make_bundle(&new, "0.8.1");

        let _legacy = LegacyEnv::set();
        let strategy = replace_bundle_in_place(&dest, &new).expect("差し替え");
        assert_eq!(strategy, ReplaceStrategy::MoveAside);
        assert!(
            !strategy.keeps_path_occupied(),
            "旧挙動は置き場を空ける手段である"
        );
        assert_eq!(version_of(&dest), "0.8.1", "更新自体は成立すること");
    }

    #[test]
    fn 手段のラベルが読める() {
        assert_eq!(ReplaceStrategy::ContentsSwap.label(), "contents-swap");
        assert_eq!(ReplaceStrategy::Swap.label(), "swap");
        assert_eq!(ReplaceStrategy::FreshInstall.label(), "fresh");
        assert_eq!(ReplaceStrategy::MoveAside.label(), "move-aside");
        for keeps in [
            ReplaceStrategy::ContentsSwap,
            ReplaceStrategy::Swap,
            ReplaceStrategy::FreshInstall,
        ] {
            assert!(keeps.keeps_path_occupied(), "{keeps:?}");
        }
        assert!(!ReplaceStrategy::MoveAside.keeps_path_occupied());
        // `.app` の identity まで保てるのは Contents だけの入れ替えのとき
        assert!(ReplaceStrategy::ContentsSwap.keeps_bundle_identity());
        for changes in [
            ReplaceStrategy::Swap,
            ReplaceStrategy::FreshInstall,
            ReplaceStrategy::MoveAside,
        ] {
            assert!(!changes.keeps_bundle_identity(), "{changes:?}");
        }
    }
}
