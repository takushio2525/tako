//! リリースアセットの命名規則（#594 / #595 の正）
//!
//! **何のためにあるか**: 「どのファイルがどの OS 向けの配布物か」の判定が
//! リリース側（`scripts/release.sh`）と更新チェック側（`tako-app::update_checker`）で
//! 食い違うと、Windows クライアントが macOS の zip を掴む・自 OS 用アセットが無いのに
//! 「更新あり」と通知する、といった事故になる（#595 の背景）。
//!
//! そこで**命名規則の判定ロジックはこのモジュール 1 箇所を正とする**。
//! シェル側（`scripts/lib/release-assets.sh`。macOS のリリース）と
//! PowerShell 側（`installer/windows/lib/release-assets.ps1`。Windows のリリース。#587）は
//! 同じ規則の写しで、3 者が一致していることは本モジュールの同期テストが機械検証する
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

use super::support::{Note, Platform};

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

/// リリースが配布対象とするプラットフォーム（リリースノートの表の行順）。
///
/// **リリースは全部が揃って初めて成立する**（#965）。片方だけ出ると、欠けた OS の
/// 利用者には「更新が無い」ように見えたまま（#595 の判定は自 OS 用アセットの有無で決まる）
/// バージョンだけが進む。`missing_platforms` がその状態を機械検出する。
pub const PLATFORMS: [Platform; 2] = [Platform::MacOs, Platform::Windows];

/// macOS 版が動く最低 OS バージョン。
/// 正は `scripts/build-app.sh` が Info.plist へ書く `LSMinimumSystemVersion`
/// （食い違いは `os_requirements_match_build_declarations` が落とす）
pub const MACOS_MIN_VERSION: &str = "11.0";

/// Windows 版が動く最低 OS ビルド。
/// 正は `installer/windows/tako.iss` の `MinVersion`（インストーラーが自分で弾く値）
pub const WINDOWS_MIN_BUILD: &str = "10.0.17763";

/// リリースノートに出す動作要件（#965）。
///
/// 配布物を落とす前に「自分の環境で動くのか」が分かる必要がある。文言は日英併記の
/// `Note` 1 箇所で定義し、シェル側の写し（`scripts/lib/release-assets.sh`）は
/// 同期テストで拘束する（#435 準拠 = 表示言語ごとに別実装を作らない）。
///
/// **arch は現在の配布実態を書いている**（macOS = Apple Silicon 1 本 /
/// Windows = x64 1 本）。universal 版や Intel 版を配り始めるときはここも直す
/// （ダウンロード表の方は実アセット名から組むので自動で追随する）。
pub fn os_requirement(platform: Platform) -> Note {
    match platform {
        Platform::MacOs => Note::new(
            "macOS 11.0 以降 / Apple Silicon（arm64）",
            "macOS 11.0 or later / Apple Silicon (arm64)",
        ),
        Platform::Windows => Note::new(
            "Windows 10 バージョン 1809（ビルド 10.0.17763）以降 / x64",
            "Windows 10 version 1809 (build 10.0.17763) or later / x64",
        ),
    }
}

/// アセット名の一覧から、**配布物が 1 つも無いプラットフォーム**を返す（#965）。
///
/// arch は問わない（リリース側が判りたいのは「どの OS の利用者が取り残されるか」で、
/// 現在の配布は macOS = arm64 / Windows = x86_64 の 1 本ずつ）。
/// 規則外のファイル名（チェックサム等）は `ReleaseAsset::parse` が無視する。
pub fn missing_platforms<'a, I>(names: I) -> Vec<Platform>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut present = [false; PLATFORMS.len()];
    for name in names {
        let Some(asset) = ReleaseAsset::parse(name) else {
            continue;
        };
        if let Some(i) = PLATFORMS.iter().position(|p| *p == asset.platform) {
            present[i] = true;
        }
    }
    PLATFORMS
        .iter()
        .zip(present)
        .filter(|(_, seen)| !*seen)
        .map(|(platform, _)| *platform)
        .collect()
}

/// 両 OS の配布物が揃っているか（片肺リリースの検出）。#965
pub fn is_complete<'a, I>(names: I) -> bool
where
    I: IntoIterator<Item = &'a str>,
{
    missing_platforms(names).is_empty()
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
        // Windows のプレビュー反復（#723）。`installer/windows/build-installer.ps1` が
        // ISCC へ渡す OutputBaseFilename はこの名前から拡張子を落としたもの
        assert_eq!(
            asset_name(Platform::Windows, Arch::X86_64, "v0.5.13-win.3"),
            "tako-v0.5.13-win.3-windows-x86_64.exe"
        );
    }

    #[test]
    fn parse_roundtrips_every_combination() {
        for platform in [Platform::MacOs, Platform::Windows] {
            for arch in [Arch::Arm64, Arch::X86_64] {
                for ext in extensions(platform) {
                    // `-win.N` は Windows のプレビュー反復が実際に使うタグ（#723）。
                    // タグ自身が `-` と `.` を含むので「右から解析」の回帰の錨になる
                    for tag in [
                        "v0.6.0",
                        "v0.6.0-test.1",
                        "v10.20.30-test.99",
                        "v0.5.13-win.3",
                    ] {
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

    // --- リリースの完全性 / 動作要件（#965）---

    #[test]
    fn missing_platforms_detects_one_sided_release() {
        // 両 OS 揃っている = 完全
        let both = [
            "tako-v0.7.9-macos-arm64.zip",
            "tako-v0.7.9-windows-x86_64.exe",
            "tako-v0.7.9-windows-x86_64.zip",
        ];
        assert!(missing_platforms(both).is_empty());
        assert!(is_complete(both));

        // macOS だけ = Windows の利用者が取り残される（v0.7.8 までの実状）
        let mac_only = ["tako-v0.7.8-macos-arm64.zip"];
        assert_eq!(missing_platforms(mac_only), vec![Platform::Windows]);
        assert!(!is_complete(mac_only));

        // Windows だけ
        let win_only = ["tako-v0.7.9-windows-x86_64.exe"];
        assert_eq!(missing_platforms(win_only), vec![Platform::MacOs]);
        assert!(!is_complete(win_only));

        // 規則外のファイルは配布物として数えない（添付資料を「揃っている」と誤判定しない）
        let decoys = ["checksums.txt", "tako-v0.7.9-linux-x86_64.zip", "tako.zip"];
        assert_eq!(
            missing_platforms(decoys),
            vec![Platform::MacOs, Platform::Windows]
        );
        // アセットが 1 つも無いリリースも当然「揃っていない」
        assert!(!is_complete(std::iter::empty::<&str>()));
    }

    #[test]
    fn platforms_covers_every_supported_target() {
        // extensions() / display_label() を持つ OS が PLATFORMS から漏れていないこと。
        // 漏れると「その OS のアセットが無くても完全」と判定してしまう
        for platform in [Platform::MacOs, Platform::Windows] {
            assert!(
                PLATFORMS.contains(&platform),
                "{platform:?} が PLATFORMS に無い（片肺検出の網から漏れる）"
            );
        }
        assert_eq!(PLATFORMS.len(), 2);
    }

    /// 動作要件の数値が**実際にビルドへ埋まっている宣言**と一致していること。
    ///
    /// ノートに書いた要件が配布物の実際の下限とズレると、動かない環境の利用者に
    /// ダウンロードさせる（甘い側）か、動く環境の利用者を諦めさせる（厳しい側）。
    #[test]
    fn os_requirements_match_build_declarations() {
        // Windows: インストーラー自身が弾く下限
        let iss = std::fs::read_to_string(repo_path("installer/windows/tako.iss"))
            .expect("installer/windows/tako.iss");
        assert!(
            iss.contains(&format!("MinVersion={WINDOWS_MIN_BUILD}")),
            "tako.iss の MinVersion が WINDOWS_MIN_BUILD ({WINDOWS_MIN_BUILD}) と一致しない"
        );
        // macOS: .app の Info.plist へ書かれる下限
        let build_app = std::fs::read_to_string(repo_path("scripts/build-app.sh"))
            .expect("scripts/build-app.sh");
        assert!(
            build_app.contains("LSMinimumSystemVersion"),
            "build-app.sh が LSMinimumSystemVersion を書いていない"
        );
        assert!(
            build_app.contains(&format!("<string>{MACOS_MIN_VERSION}</string>")),
            "build-app.sh の LSMinimumSystemVersion が MACOS_MIN_VERSION ({MACOS_MIN_VERSION}) と一致しない"
        );

        // 文言（日英とも）にその数値が入っていること
        let mac = os_requirement(Platform::MacOs);
        assert!(mac.ja().contains(MACOS_MIN_VERSION) && mac.en().contains(MACOS_MIN_VERSION));
        let win = os_requirement(Platform::Windows);
        assert!(win.ja().contains(WINDOWS_MIN_BUILD) && win.en().contains(WINDOWS_MIN_BUILD));
        // arch も明示する（macOS 版は arm64 のみ = Intel Mac では動かない）
        assert!(mac.ja().contains(Arch::Arm64.as_str()));
        assert!(win.ja().contains(Arch::X86_64.as_str()) || win.ja().contains("x64"));
    }

    // --- 写し（sh / PowerShell）との同期検証 ---
    //
    // 命名規則を 3 言語に写すことになるので、**ズレたら落ちるテスト**で縛る。
    // これが無いと片方だけ直して気付かず、#595 の事故が再発する。

    fn repo_path(rel: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(rel)
    }

    fn shell_lib_path() -> std::path::PathBuf {
        repo_path("scripts/lib/release-assets.sh")
    }

    fn powershell_lib_path() -> std::path::PathBuf {
        repo_path("installer/windows/lib/release-assets.ps1")
    }

    /// Inno Setup のスクリプトがアセット名を**自分で組み立てていない**ことの番犬（#587）。
    ///
    /// `.iss` の ISPP は Rust からも pwsh からも実行できないので、ここで名前を組み直されると
    /// 命名規則の 3 つ目の実装ができてしまい、同期テストの網にかからない。
    /// `OutputBaseFilename` は `build-installer.ps1` が `/DAssetBaseName=` で渡す値だけを使い、
    /// 未定義なら `#error` で落ちる、という形を維持させる
    #[test]
    fn inno_setup_does_not_build_asset_names_itself() {
        let src = std::fs::read_to_string(repo_path("installer/windows/tako.iss"))
            .expect("installer/windows/tako.iss");
        assert!(
            src.contains("OutputBaseFilename={#AssetBaseName}"),
            "OutputBaseFilename が /DAssetBaseName= の値をそのまま使っていない"
        );
        assert!(
            src.contains("#ifndef AssetBaseName") && src.contains("#error"),
            "AssetBaseName 未定義時に #error で落ちる形になっていない             （黙ってフォールバックすると #595 の食い違いが再発する）"
        );
        // 命名規則の断片を .iss へ書き戻していないか。`#error` / コメントの例示は
        // 完成形の 1 語（tako-vX-windows-x86_64）なので、それらを除いた上で
        // 「接頭辞と platform / arch トークンを + で連結している」行を禁じる
        for (i, line) in src.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with(';') || code.starts_with("#error") {
                continue;
            }
            let builds_name = code.contains('+')
                && code.contains(PREFIX)
                && code.contains(Platform::Windows.as_str());
            assert!(
                !builds_name,
                "tako.iss:{} がアセット名を組み立てている: {line}                 （命名規則は release_assets.rs が正。/DAssetBaseName= で受け取ること）",
                i + 1
            );
        }
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
    /// 片肺検出（#965）と動作要件の写しが Rust と一致していること。
    ///
    /// `scripts/release.sh` は**シェル側の関数で**リリースの完全性を判定するので、
    /// ここがズレると「揃っていないのに揃ったと報告する」= 検査が空振りする。
    #[cfg(unix)]
    #[test]
    fn shell_mirror_detects_same_missing_platforms() {
        let lib = shell_lib_path();
        let cases: [&[&str]; 6] = [
            &[
                "tako-v0.7.9-macos-arm64.zip",
                "tako-v0.7.9-windows-x86_64.exe",
            ],
            &["tako-v0.7.9-macos-arm64.zip"],
            &["tako-v0.7.9-windows-x86_64.exe"],
            &["tako-v0.7.9-windows-x86_64.zip"],
            &["checksums.txt"],
            &[],
        ];
        for names in cases {
            let quoted: Vec<String> = names.iter().map(|n| format!("'{n}'")).collect();
            let script = format!(
                ". '{}'; tako_asset_missing_platforms {}",
                lib.display(),
                quoted.join(" ")
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
            let got: Vec<String> = String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();
            let want: Vec<String> = missing_platforms(names.iter().copied())
                .into_iter()
                .map(|p| p.as_str().to_string())
                .collect();
            assert_eq!(got, want, "片肺判定がシェルと Rust で食い違う（{names:?}）");
        }
    }

    #[test]
    fn shell_mirror_declares_same_requirements() {
        let src = std::fs::read_to_string(shell_lib_path()).expect("scripts/lib/release-assets.sh");
        for platform in PLATFORMS {
            let suffix = platform.as_str().to_uppercase();
            let note = os_requirement(platform);
            for (lang, want_text) in [("JA", note.ja()), ("EN", note.en())] {
                let want = format!("TAKO_ASSET_REQ_{suffix}_{lang}=\"{want_text}\"");
                assert!(
                    src.contains(&want),
                    "シェル側の TAKO_ASSET_REQ_{suffix}_{lang} が Rust の os_requirement() と一致しない（期待: {want}）"
                );
            }
        }
    }

    // --- PowerShell 側（installer/windows/lib/release-assets.ps1）との同期検証 ---
    //
    // Windows のリリースは GitHub Actions ではなく実機の PowerShell で回す（#587）ので、
    // 配布物の名前を決めるのは PowerShell 側になる。ここがズレると
    // 「インストーラーは作られたのに更新チェックが自 OS 向けと認識しない」= #595 の再来。

    #[test]
    fn powershell_mirror_declares_same_constants() {
        let src = std::fs::read_to_string(powershell_lib_path())
            .expect("installer/windows/lib/release-assets.ps1");
        // 接頭辞
        assert!(
            src.contains(&format!("$TakoAssetPrefix = '{PREFIX}'")),
            "PowerShell 側の $TakoAssetPrefix が Rust の PREFIX と一致しない"
        );
        // Windows の配布 arch トークン（`x64` 等の別名を書くと Rust 側の parse が弾く）
        assert!(
            src.contains(&format!(
                "$TakoAssetArchWindows = '{}'",
                Arch::X86_64.as_str()
            )),
            "PowerShell 側の $TakoAssetArchWindows が Rust の Arch::X86_64 と一致しない"
        );
        // プラットフォームごとの拡張子（並び順 = 優先順位も一致させる）と表示名
        for platform in [Platform::MacOs, Platform::Windows] {
            // macos -> Macos / windows -> Windows（PowerShell の変数名は PascalCase）
            let suffix = {
                let name = platform.as_str();
                let mut c = name.chars();
                match c.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
                    None => String::new(),
                }
            };
            let want_exts = format!(
                "$TakoAssetExts{suffix} = '{}'",
                extensions(platform).join(" ")
            );
            assert!(
                src.contains(&want_exts),
                "PowerShell 側の $TakoAssetExts{suffix} が Rust の extensions() と一致しない（期待: {want_exts}）"
            );
            let want_label = format!("$TakoAssetLabel{suffix} = '{}'", display_label(platform));
            assert!(
                src.contains(&want_label),
                "PowerShell 側の $TakoAssetLabel{suffix} が Rust の display_label() と一致しない（期待: {want_label}）"
            );
        }
    }

    /// PowerShell 関数を実際に実行して Rust の生成結果と突き合わせる（最も強い同期検証）。
    ///
    /// pwsh が無い環境（素の Linux コンテナ等）では**検証をスキップする**。
    /// macOS の CI ランナーと Windows には pwsh があるので、CI では必ず走る
    /// （上の定数テストは pwsh 不要なので、どの環境でもドリフトは検出できる）
    #[test]
    fn powershell_mirror_generates_identical_names() {
        let Some(pwsh) = ["pwsh", "powershell"].into_iter().find(|bin| {
            std::process::Command::new(bin)
                .args(["-NoProfile", "-Command", "exit 0"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        }) else {
            eprintln!("pwsh が無いので PowerShell 写しの実行検証をスキップ（定数検証は実施済み）");
            return;
        };

        let lib = powershell_lib_path();
        // 1 プロセスで全組み合わせを出させる（pwsh の起動は 1 回 1 秒近くかかる）
        let mut script = format!(". '{}'\n", lib.display());
        let mut want: Vec<String> = Vec::new();
        for platform in [Platform::MacOs, Platform::Windows] {
            for arch in [Arch::Arm64, Arch::X86_64] {
                for tag in ["v0.6.0", "v0.6.0-test.1", "v0.5.13-win.3"] {
                    for ext in extensions(platform) {
                        script.push_str(&format!(
                            "Get-TakoAssetName -Tag '{tag}' -Platform '{}' -Arch '{}' -Ext '{ext}'\n",
                            platform.as_str(),
                            arch.as_str()
                        ));
                        want.push(asset_name_with_ext(platform, arch, tag, ext));
                    }
                    // Ext 省略時は主形式になること
                    script.push_str(&format!(
                        "Get-TakoAssetName -Tag '{tag}' -Platform '{}' -Arch '{}'\n",
                        platform.as_str(),
                        arch.as_str()
                    ));
                    want.push(asset_name(platform, arch, tag));
                    // Inno Setup の OutputBaseFilename（= 拡張子を除いたベース名）
                    script.push_str(&format!(
                        "Get-TakoAssetBaseName -Tag '{tag}' -Platform '{}' -Arch '{}'\n",
                        platform.as_str(),
                        arch.as_str()
                    ));
                    let primary = asset_name(platform, arch, tag);
                    let base = primary
                        .rsplit_once('.')
                        .expect("主形式に拡張子がある")
                        .0
                        .to_string();
                    want.push(base);
                }
            }
        }

        let out = std::process::Command::new(pwsh)
            .args(["-NoProfile", "-Command", &script])
            .output()
            .expect("pwsh の実行に失敗");
        assert!(
            out.status.success(),
            "PowerShell 写しの実行が失敗: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let got: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        assert_eq!(
            got,
            want,
            "PowerShell と Rust でアセット名が食い違う（stderr: {}）",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
