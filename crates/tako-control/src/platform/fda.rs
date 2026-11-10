//! プライバシー許可の案内（抽象境界 B15 `PermissionGuide`。macOS の TCC / フルディスクアクセス）。#2859
//!
//! OS の差はこのモジュールの内側（各関数の `cfg`）に閉じている。呼び手（`crate::fda` の状態の組み立て・
//! 診断の `fda` 項目・`tako setup` の FDA の段）は [`supported`] の答えを見て 1 本の経路で組み、
//! 対象外の OS では項目を出さない（#2859 で `fda.rs` からここへ移した。中身は移設の前後でトークン列が一致）。
//!
//! | OS | 対象か（[`supported`]） | 付与の判定（[`is_granted`]） | 設定画面（[`open_settings_typed`]） |
//! |---|---|---|---|
//! | macOS | true | FDA 保護下のパスへの `read_dir`（直接の API は無い） | システム設定の URL スキーム |
//! | それ以外 | false（TCC 相当が無い） | 常に true（判定不要） | `Err(OsRefusal::FdaMacOnly)` |

// FDA は macOS の TCC 固有の概念（抽象境界 B15）。プローブパス解決も macOS でのみ使う
#[cfg(target_os = "macos")]
use std::path::PathBuf;

/// この OS にフルディスクアクセス（TCC）の案内の対象があるか（#2859）。
/// false = 対象外（Windows ほか。TCC 相当が無い）。診断と `tako setup` はこれを見て項目を出さない。
/// CLI / MCP の `tako fda status` の `platform_supported` も同じ答え
pub fn supported() -> bool {
    cfg!(target_os = "macos")
}

/// FDA が付与されているかをベストエフォートで検出する。
/// macOS に直接の API は無いため、FDA 保護下のパスへの read_dir 試行で判定する。
/// - true: FDA 付与済み（保護パスを読めた）
/// - false: 未付与（Permission denied）
/// - macOS 以外では常に true を返す（TCC が無いため判定不要）
pub fn is_granted() -> bool {
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
    #[cfg(target_os = "macos")]
    {
        // ~/Library/Mail は kTCCServiceSystemPolicyAllFiles で保護されている。
        // FDA が無いと read_dir で Permission denied になる（ディレクトリが存在する場合）。
        // 存在しない場合は別のプローブパスを試す
        let probes = fda_probe_paths();
        for path in &probes {
            if !path.exists() {
                continue;
            }
            match std::fs::read_dir(path) {
                Ok(_) => return true,
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return false,
                Err(_) => continue, // 他のエラー（破損等）はスキップ
            }
        }
        // プローブパスがすべて存在しない場合は判定不能 → 付与済みと仮定
        // （ダイアログが出ていないなら問題ない）
        true
    }
}

/// FDA のプローブに使うパス群。存在する順に試行する
#[cfg(target_os = "macos")]
fn fda_probe_paths() -> Vec<PathBuf> {
    // FDA は macOS 専用だが、ホーム解決は他と同じ正本を通す（#893）
    let home = tako_core::paths::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    vec![
        home.join("Library/Mail"),
        home.join("Library/Safari"),
        home.join("Library/Messages"),
        home.join("Library/Cookies"),
    ]
}

/// macOS のシステム設定を開いてフルディスクアクセスのパネルを表示する（型の口。断りを表示言語で返す
/// dispatch が使う。#2365）。日本語の口 `crate::fda::open_settings` はこれを包む
pub fn open_settings_typed() -> Result<(), crate::OsRefusal> {
    #[cfg(target_os = "macos")]
    {
        // macOS 13+ のシステム設定 URL スキーム
        let urls = [
            "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles",
            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_AllFiles",
        ];
        for url in &urls {
            if crate::platform::os_integration::open_url_wait(url).is_ok() {
                return Ok(());
            }
        }
        Err(crate::OsRefusal::FdaPanelUnopenable)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(crate::OsRefusal::FdaMacOnly)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 対象か（`supported`）と、対象外の OS の答え（常に付与済み・設定画面は断る）が対で揃っている
    /// （#2859。呼び手は `supported` が false なら項目を出さないので、対象外で「未付与」と出る形を止める）
    #[test]
    fn 対象外のosでは付与済みと答え設定画面は断る() {
        if supported() {
            return;
        }
        assert!(is_granted(), "対象外の OS で未付与と答えた");
        assert!(
            matches!(open_settings_typed(), Err(crate::OsRefusal::FdaMacOnly)),
            "対象外の OS で設定画面を開こうとした"
        );
    }
}
