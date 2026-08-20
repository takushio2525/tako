//! リリースアセットの命名規則（#594 / #595 の正）
//!
//! **何のためにあるか**: 「どのファイルがどの OS 向けの配布物か」の判定が
//! リリース側（`scripts/release.sh`）と更新チェック側（`tako-app::update_checker`）で
//! 食い違うと、Windows クライアントが macOS の zip を掴む・自 OS 用アセットが無いのに
//! 「更新あり」と通知する、といった事故になる（#595 の背景）。
//!
//! そこで**命名規則の判定ロジックはこのモジュール 1 箇所を正とする**。
//! シェル側（`scripts/lib/release-assets.sh`）は同じ規則の写しで、
//! 両者が一致していることは本モジュールの同期テストが機械検証する
//! （テストがあるので「片方だけ直して気付かない」が起きない）。
//!
//! ## 命名規則
//!
//! ```text
//! tako-<tag>-<platform>-<arch>.<ext>
//!
//! tako-v0.5.13-macos-arm64.zip        macOS / Apple Silicon
//! tako-v0.6.0-test.1-macos-arm64.zip  テスト版（タグに `-` と `.` を含む）
//! tako-v0.6.0-windows-x86_64.exe      Windows インストーラー（#587）
//! tako-v0.6.0-windows-x86_64.zip      Windows ポータブル版
//! ```
//!
//! タグ自身が `-` と `.` を含む（`v0.6.0-test.1`）ため、**解析は必ず右から行う**。

use super::support::Platform;

/// アセット名の接頭辞
pub const PREFIX: &str = "tako-";

/// 配布アーキテクチャ。**アセット名に現れるトークンが正**
/// （`std::env::consts::ARCH` の `aarch64` とは綴りが違うので変換して使う）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arch {
    Arm64,
    X86_64,
}

impl Arch {
    /// アセット名に現れるトークン
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Arm64 => "arm64",
            Self::X86_64 => "x86_64",
        }
    }

    /// アセット名のトークンから復元する。**別名は受け付けない**
    /// （`win` / `amd64` 等を通すと規則外のファイルを配布物と誤認するため）
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "arm64" => Some(Self::Arm64),
            "x86_64" => Some(Self::X86_64),
            _ => None,
        }
    }

    /// 実行中のアーキテクチャ。マトリクス外の CPU では `None`
    /// （= 自分向けの配布物は存在しないので更新候補も出さない）
    pub fn current() -> Option<Self> {
        match std::env::consts::ARCH {
            "aarch64" | "arm64" => Some(Self::Arm64),
            "x86_64" => Some(Self::X86_64),
            _ => None,
        }
    }
}

/// そのプラットフォームで配布物として認める拡張子。**先頭が主形式**。
///
/// Windows の主形式はインストーラー（#587）。ポータブル zip も配布物として認める。
/// 新しい形式（`.msi` 等）を配るときは**ここを増やす**（増やし忘れると
/// 更新チェックがそのアセットを見落とし、利用者に更新が届かない）。
pub fn extensions(platform: Platform) -> &'static [&'static str] {
    match platform {
        Platform::MacOs => &["zip"],
        Platform::Windows => &["exe", "zip"],
    }
}

/// そのプラットフォームの主形式の拡張子
pub fn primary_extension(platform: Platform) -> &'static str {
    extensions(platform)[0]
}

/// リリースノートのダウンロード表に出す表示名
pub fn display_label(platform: Platform) -> &'static str {
    match platform {
        Platform::MacOs => "macOS",
        Platform::Windows => "Windows",
    }
}

/// 主形式のアセット名を組み立てる
pub fn asset_name(platform: Platform, arch: Arch, tag: &str) -> String {
    asset_name_with_ext(platform, arch, tag, primary_extension(platform))
}

/// 拡張子を指定してアセット名を組み立てる
pub fn asset_name_with_ext(platform: Platform, arch: Arch, tag: &str, ext: &str) -> String {
    format!(
        "{PREFIX}{tag}-{}-{}.{ext}",
        platform.as_str(),
        arch.as_str()
    )
}

/// 命名規則に沿って解析できたアセット
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAsset {
    /// `v0.6.0` / `v0.6.0-test.1`
    pub tag: String,
    pub platform: Platform,
    pub arch: Arch,
    pub ext: String,
}

impl ReleaseAsset {
    /// ファイル名を解析する。規則外なら `None`
    /// （= 配布物ではない。チェックサムや添付資料を掴まないための門番）
    pub fn parse(file_name: &str) -> Option<Self> {
        let rest = file_name.strip_prefix(PREFIX)?;
        // タグが `-` と `.` を含むので右から削っていく
        let (stem, ext) = rest.rsplit_once('.')?;
        let (left, arch_token) = stem.rsplit_once('-')?;
        let (tag, platform_token) = left.rsplit_once('-')?;
        if tag.is_empty() {
            return None;
        }
        // 別名を受け付けない厳格一致（`Platform::parse` は `win` 等も通すので使わない）
        let platform = [Platform::MacOs, Platform::Windows]
            .into_iter()
            .find(|p| p.as_str() == platform_token)?;
        let arch = Arch::parse(arch_token)?;
        if !extensions(platform).contains(&ext) {
            return None;
        }
        Some(Self {
            tag: tag.to_string(),
            platform,
            arch,
            ext: ext.to_string(),
        })
    }
}

/// そのファイル名が指定の環境向けの配布物か
pub fn is_for(file_name: &str, platform: Platform, arch: Arch) -> bool {
    ReleaseAsset::parse(file_name).is_some_and(|a| a.platform == platform && a.arch == arch)
}

/// アセット名の一覧から、指定環境向けのものを 1 つ選ぶ。
///
/// 主形式（macOS = zip、Windows = インストーラー）を優先し、
/// 無ければ他の許容形式へ落とす。**どれも無ければ `None`** =
/// 「このリリースには自分向けの配布物が無い」の判定になる（#595 要件 1）。
pub fn select<'a, I>(names: I, platform: Platform, arch: Arch) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut best: Option<(usize, &'a str)> = None;
    for name in names {
        let Some(asset) = ReleaseAsset::parse(name) else {
            continue;
        };
        if asset.platform != platform || asset.arch != arch {
            continue;
        }
        // extensions() の並び順が優先順位
        let Some(rank) = extensions(platform).iter().position(|e| *e == asset.ext) else {
            continue;
        };
        if best.is_none_or(|(best_rank, _)| rank < best_rank) {
            best = Some((rank, name));
        }
    }
    best.map(|(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arch_tokens_are_canonical() {
        assert_eq!(Arch::Arm64.as_str(), "arm64");
        assert_eq!(Arch::X86_64.as_str(), "x86_64");
        assert_eq!(Arch::parse("arm64"), Some(Arch::Arm64));
        assert_eq!(Arch::parse("x86_64"), Some(Arch::X86_64));
        // 別名は通さない（規則外ファイルの誤認を防ぐ）
        assert_eq!(Arch::parse("aarch64"), None);
        assert_eq!(Arch::parse("amd64"), None);
        assert_eq!(Arch::parse("x64"), None);
    }

    #[test]
    fn arch_current_is_known_or_none() {
        // 実行環境依存だが、返るなら必ず正規トークンに解決できる
        if let Some(a) = Arch::current() {
            assert!(Arch::parse(a.as_str()).is_some());
        }
    }

    #[test]
    fn asset_name_matches_shipped_convention() {
        // 実際に配布済みのアセット名と一致すること（回帰の錨）
        assert_eq!(
            asset_name(Platform::MacOs, Arch::Arm64, "v0.5.13"),
            "tako-v0.5.13-macos-arm64.zip"
        );
        assert_eq!(
            asset_name(Platform::Windows, Arch::X86_64, "v0.6.0"),
            "tako-v0.6.0-windows-x86_64.exe"
        );
        assert_eq!(
            asset_name_with_ext(Platform::Windows, Arch::X86_64, "v0.6.0", "zip"),
            "tako-v0.6.0-windows-x86_64.zip"
        );
    }

    #[test]
    fn parse_roundtrips_every_combination() {
        for platform in [Platform::MacOs, Platform::Windows] {
            for arch in [Arch::Arm64, Arch::X86_64] {
                for ext in extensions(platform) {
                    for tag in ["v0.6.0", "v0.6.0-test.1", "v10.20.30-test.99"] {
                        let name = asset_name_with_ext(platform, arch, tag, ext);
                        let parsed = ReleaseAsset::parse(&name)
                            .unwrap_or_else(|| panic!("解析できない: {name}"));
                        assert_eq!(parsed.tag, tag, "{name}");
                        assert_eq!(parsed.platform, platform, "{name}");
                        assert_eq!(parsed.arch, arch, "{name}");
                        assert_eq!(parsed.ext, *ext, "{name}");
                    }
                }
            }
        }
    }

    #[test]
    fn parse_rejects_irregular_names() {
        // 配布物と紛らわしいが規則外のファイル
        for name in [
            "tako.zip",                       // タグも OS も無い
            "checksums.txt",                  // 添付資料
            "tako-v0.6.0-macos.zip",          // arch 欠落
            "tako-v0.6.0-arm64.zip",          // platform 欠落
            "tako-v0.6.0-linux-x86_64.zip",   // 対象外 OS
            "tako-v0.6.0-macos-aarch64.zip",  // arch トークンが別名
            "tako-v0.6.0-win-x86_64.exe",     // platform トークンが別名
            "tako-v0.6.0-macos-arm64.dmg",    // 許容外の拡張子
            "tako-v0.6.0-macos-arm64.exe",    // macOS に exe は無い
            "sources-v0.6.0-macos-arm64.zip", // 接頭辞違い
            "tako--macos-arm64.zip",          // タグが空
            "tako-v0.6.0-macos-arm64",        // 拡張子なし
        ] {
            assert!(
                ReleaseAsset::parse(name).is_none(),
                "規則外なのに解析できてしまった: {name}"
            );
        }
        // Windows の zip はポータブル版として許容する
        assert!(ReleaseAsset::parse("tako-v0.6.0-windows-x86_64.zip").is_some());
    }

    #[test]
    fn is_for_requires_both_platform_and_arch() {
        let name = "tako-v0.6.0-macos-arm64.zip";
        assert!(is_for(name, Platform::MacOs, Arch::Arm64));
        assert!(!is_for(name, Platform::MacOs, Arch::X86_64));
        assert!(!is_for(name, Platform::Windows, Arch::Arm64));
    }

    #[test]
    fn select_prefers_primary_extension() {
        let names = [
            "checksums.txt",
            "tako-v0.6.0-windows-x86_64.zip",
            "tako-v0.6.0-windows-x86_64.exe",
            "tako-v0.6.0-macos-arm64.zip",
        ];
        // Windows はインストーラー（exe）が主形式
        assert_eq!(
            select(names, Platform::Windows, Arch::X86_64),
            Some("tako-v0.6.0-windows-x86_64.exe")
        );
        assert_eq!(
            select(names, Platform::MacOs, Arch::Arm64),
            Some("tako-v0.6.0-macos-arm64.zip")
        );
        // 該当が無ければ None（= このリリースには自分向けの配布物が無い）
        assert_eq!(select(names, Platform::MacOs, Arch::X86_64), None);
        assert_eq!(select(names, Platform::Windows, Arch::Arm64), None);
    }

    #[test]
    fn select_falls_back_to_secondary_extension() {
        let names = ["tako-v0.6.0-windows-x86_64.zip"];
        assert_eq!(
            select(names, Platform::Windows, Arch::X86_64),
            Some("tako-v0.6.0-windows-x86_64.zip")
        );
    }

    // --- シェル側（scripts/lib/release-assets.sh）との同期検証 ---
    //
    // 命名規則を 2 言語に写すことになるので、**ズレたら落ちるテスト**で縛る。
    // これが無いと片方だけ直して気付かず、#595 の事故が再発する。

    fn shell_lib_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/lib/release-assets.sh")
    }

    #[test]
    fn shell_mirror_declares_same_constants() {
        let src = std::fs::read_to_string(shell_lib_path()).expect("scripts/lib/release-assets.sh");
        // 接頭辞
        assert!(
            src.contains(&format!("TAKO_ASSET_PREFIX=\"{PREFIX}\"")),
            "シェル側の TAKO_ASSET_PREFIX が Rust の PREFIX と一致しない"
        );
        // プラットフォームごとの拡張子（並び順 = 優先順位も一致させる）
        for platform in [Platform::MacOs, Platform::Windows] {
            let var = format!("TAKO_ASSET_EXTS_{}", platform.as_str().to_uppercase());
            let want = format!("{var}=\"{}\"", extensions(platform).join(" "));
            assert!(
                src.contains(&want),
                "シェル側の {var} が Rust の extensions() と一致しない（期待: {want}）"
            );
            let label_var = format!("TAKO_ASSET_LABEL_{}", platform.as_str().to_uppercase());
            let want_label = format!("{label_var}=\"{}\"", display_label(platform));
            assert!(
                src.contains(&want_label),
                "シェル側の {label_var} が Rust の display_label() と一致しない（期待: {want_label}）"
            );
        }
    }

    /// シェル関数を実際に実行して Rust の生成結果と突き合わせる（最も強い同期検証）。
    /// Windows では sh が無い前提なので unix 限定（#583）
    #[cfg(unix)]
    #[test]
    fn shell_mirror_generates_identical_names() {
        let lib = shell_lib_path();
        for platform in [Platform::MacOs, Platform::Windows] {
            for arch in [Arch::Arm64, Arch::X86_64] {
                for tag in ["v0.6.0", "v0.6.0-test.1"] {
                    let script = format!(
                        ". '{}'; tako_asset_name '{tag}' '{}' '{}'",
                        lib.display(),
                        platform.as_str(),
                        arch.as_str()
                    );
                    let out = std::process::Command::new("sh")
                        .args(["-c", &script])
                        .output()
                        .expect("sh の実行に失敗");
                    assert!(
                        out.status.success(),
                        "シェル関数が失敗: {}",
                        String::from_utf8_lossy(&out.stderr)
                    );
                    let got = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    assert_eq!(
                        got,
                        asset_name(platform, arch, tag),
                        "シェルと Rust でアセット名が食い違う（{platform:?} / {arch:?} / {tag}）"
                    );
                }
            }
        }
    }
}
