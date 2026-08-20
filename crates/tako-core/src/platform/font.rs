//! 既定の等幅フォント解決（抽象境界 B16）
//!
//! ターミナル描画はセル幅（`'M'` の advance）と実グリフ幅が一致する前提で組まれている。
//! GPUI は解決できない family を**黙って** fallback stack（Windows では UI フォント =
//! プロポーショナルな Segoe UI）へ落とすため、その OS に存在しない family を既定にすると
//! 「全角の文字間が異常に空く」「同一行の文字が重なる」「列数が減って TUI が過剰折返しする」が
//! 一斉に起きる（Windows で macOS 専用の Menlo を既定にしていた #517 の実害）。
//!
//! したがって既定フォントは OS ごとに「その OS に必ず存在する等幅フォント」を返す。

use std::sync::OnceLock;

/// このプラットフォームの既定等幅フォントファミリ名。
/// 解決は一度きり（ファイル存在チェックを毎回のテーマ構築で走らせない）
pub fn default_monospace_family() -> String {
    static RESOLVED: OnceLock<String> = OnceLock::new();
    RESOLVED.get_or_init(imp::default_monospace_family).clone()
}

#[cfg(target_os = "macos")]
mod imp {
    /// macOS 同梱の等幅フォント。システムフォントなので必ず存在する
    pub(super) fn default_monospace_family() -> String {
        "Menlo".into()
    }
}

#[cfg(windows)]
mod imp {
    pub(super) fn default_monospace_family() -> String {
        super::resolve_windows_monospace(&font_dirs(), &|p| std::path::Path::new(p).exists())
    }

    /// システム全体 → ユーザー個別の順に探す（後者は管理者権限なしのインストール先）
    fn font_dirs() -> Vec<String> {
        let mut dirs = Vec::new();
        if let Some(root) = env_string("SystemRoot") {
            dirs.push(format!("{root}\\Fonts"));
        }
        if let Some(local) = env_string("LOCALAPPDATA") {
            dirs.push(format!("{local}\\Microsoft\\Windows\\Fonts"));
        }
        dirs
    }

    /// 非 UTF-8 の環境変数値は解決に使えないため落とす（後段のフォールバックへ回す）
    fn env_string(key: &str) -> Option<String> {
        std::env::var_os(key).and_then(|v| v.into_string().ok())
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    /// 非対応プラットフォーム（tako は macOS / Windows 対応）。
    /// 主要ディストリの標準等幅フォントを best-effort で指す
    pub(super) fn default_monospace_family() -> String {
        "DejaVu Sans Mono".into()
    }
}

/// Windows の既定等幅フォント解決（純粋関数。**macOS 上でもテストできる**ようにしてある）。
///
/// 優先順は「Cascadia Mono（Windows Terminal 同梱。Windows 11 の既定ターミナル体験と同じ）→
/// Consolas（Vista 以降の OS 同梱で必ずある）」。family 名の存在確認をフォントファイルで
/// 行うのは、GPUI 側は解決に失敗しても例外を出さず UI フォントへ落ちるだけで、
/// 「等幅でなくなった」ことを事後に検知できないため
#[cfg_attr(not(windows), allow(dead_code))]
fn resolve_windows_monospace(font_dirs: &[String], exists: &dyn Fn(&str) -> bool) -> String {
    // (DirectWrite の family 名, インストール時のファイル名)
    const CANDIDATES: &[(&str, &str)] = &[
        ("Cascadia Mono", "CascadiaMono.ttf"),
        ("Consolas", "consola.ttf"),
    ];
    for (family, file) in CANDIDATES {
        for dir in font_dirs.iter().filter(|d| !d.is_empty()) {
            if exists(&format!("{dir}\\{file}")) {
                return (*family).into();
            }
        }
    }
    // どれも見つからない（環境変数が痩せている等）。OS 同梱の Consolas に賭ける。
    // 少なくとも「macOS 専用フォント名」よりは解決できる見込みが高い
    "Consolas".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs() -> Vec<String> {
        vec![
            "C:\\Windows\\Fonts".into(),
            "C:\\Users\\u\\AppData\\Local\\Microsoft\\Windows\\Fonts".into(),
        ]
    }

    #[test]
    fn cascadia_monoがあればそれを最優先する() {
        let got = resolve_windows_monospace(&dirs(), &|_p| true);
        assert_eq!(got, "Cascadia Mono");
    }

    #[test]
    fn cascadia_monoが無ければconsolasへ落ちる() {
        let got = resolve_windows_monospace(&dirs(), &|p| !p.contains("CascadiaMono"));
        assert_eq!(got, "Consolas");
    }

    #[test]
    fn ユーザー個別のフォントディレクトリも探す() {
        // 管理者権限なしで入れた Cascadia Mono を拾えること
        let got = resolve_windows_monospace(&dirs(), &|p| {
            p == "C:\\Users\\u\\AppData\\Local\\Microsoft\\Windows\\Fonts\\CascadiaMono.ttf"
        });
        assert_eq!(got, "Cascadia Mono");
    }

    #[test]
    fn 探索先が無くても等幅フォント名を返す() {
        let got = resolve_windows_monospace(&[String::new()], &|_p| false);
        assert_eq!(got, "Consolas");
    }

    /// 既定フォントは「その OS に存在する等幅フォント」でなければならない。
    /// ここが崩れると GPUI がプロポーショナルフォントへ落ち、ターミナル描画が全面的に壊れる
    #[test]
    fn 既定フォントはこのosの等幅フォント() {
        let family = default_monospace_family();
        assert!(!family.is_empty());
        #[cfg(target_os = "macos")]
        assert_eq!(family, "Menlo");
        #[cfg(windows)]
        {
            assert_ne!(
                family, "Menlo",
                "macOS 専用フォントを Windows の既定にしない"
            );
            assert!(
                family == "Cascadia Mono" || family == "Consolas",
                "想定外の既定フォント: {family}"
            );
        }
    }
}
