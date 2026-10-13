//! 言語サーバの取得の表（#1944）。未導入のサーバを **tako の data dir へ** 取ってくるための
//! 「どこから・どの版を・どのハッシュで・どう展開して・どこへ置くか」を持つ純粋部分
//!
//! ダウンロード・ハッシュの検証・展開・起動の組み立ては `tako_control::lsp::fetch` が持ち、
//! ここは I/O を持たない（検出表 [`super::servers::SERVERS`] の各行が [`Fetch`] を 1 つ持つ）。
//!
//! ## 決めたこと（`.agent/plans/2026-09-lsp-s1.md` §23）
//!
//! - **取得元は配布元そのもの**（npm レジストリの tarball・nodejs.org・GitHub Releases）。
//!   利用者の npm / brew を起こさない（グローバルを汚さない・npm の install script を走らせない）
//! - **版を固定し、配布元が公開しているハッシュをそのまま固定する**（npm は `dist.integrity` の
//!   SHA-512、nodejs.org は `SHASUMS256.txt`、GitHub Releases は asset の `digest`）。
//!   自分で計算した値を置かない（どこから来た値かを説明できる形にする）
//! - **置き場は `<data_dir>/lsp-servers/`**（`TAKO_DATA_DIR` の隔離に従う。PATH へは足さない）
//! - **利用者のグローバルに入っている版があればそちらを使う**（取得は「見つからない」ときだけ）
//! - **表の版を上げたら古い版の段を片付ける**（#1977。判定は [`sweep_plan`]。消すのは tako の印がある・
//!   今の版より前に入った・生きている tako が使っていない段だけ）

use std::path::{Path, PathBuf};

/// 取得物の検証に使うハッシュ（配布元が公開している値）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Digest {
    /// SHA-256 の 16 進（nodejs.org の `SHASUMS256.txt` / GitHub Releases の asset の `digest`）
    Sha256(&'static str),
    /// SHA-512 の base64（npm レジストリの `dist.integrity` の `sha512-` より後ろ）
    Sha512Base64(&'static str),
}

/// 取得物の形と、そこから取り出すもの
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Archive {
    /// gzip 1 枚 = 実行ファイル 1 つ（rust-analyzer の macOS）
    Gzip,
    /// tar.gz の中の 1 ファイルだけ（Node.js の macOS = `node-v…/bin/node`）
    TarGzMember(&'static str),
    /// npm の tarball を丸ごと（先頭の 1 段 = `package/` を外して置く）
    NpmTarball,
    /// zip の中の 1 ファイルだけ（Windows の rust-analyzer / Node.js）
    ZipMember(&'static str),
}

/// 取得物 1 つ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asset {
    pub url: &'static str,
    pub digest: Digest,
    pub archive: Archive,
    /// 配布元が示した大きさ（バイト）。進捗の分母と、これを大きく超えたら打ち切る上限に使う
    pub size: u64,
}

/// OS と CPU の組 1 つぶんの取得物
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlatformAsset {
    /// `std::env::consts::OS` の綴り（`macos` / `windows`）
    pub os: &'static str,
    /// `std::env::consts::ARCH` の綴り（`aarch64` / `x86_64`）
    pub arch: &'static str,
    pub asset: Asset,
}

/// npm の tarball 1 つ（どの OS でも同じ中身）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NpmPackage {
    pub name: &'static str,
    pub version: &'static str,
    /// `dist.integrity` の `sha512-` より後ろ
    pub integrity: &'static str,
    /// `dist.unpackedSize` ではなく tarball の大きさの目安（進捗と上限に使う）
    pub size: u64,
}

impl NpmPackage {
    /// レジストリの tarball の URL（`https://registry.npmjs.org/<name>/-/<name>-<version>.tgz`）
    pub fn url(&self) -> String {
        format!(
            "https://registry.npmjs.org/{}/-/{}-{}.tgz",
            self.name, self.name, self.version
        )
    }

    pub fn asset(&self) -> OwnedAsset {
        OwnedAsset {
            url: self.url(),
            digest: Digest::Sha512Base64(self.integrity),
            archive: Archive::NpmTarball,
            size: self.size,
        }
    }
}

/// URL を組み立てた取得物（npm の URL は表から組むので `&'static str` で持てない）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedAsset {
    pub url: String,
    pub digest: Digest,
    pub archive: Archive,
    pub size: u64,
}

impl From<&Asset> for OwnedAsset {
    fn from(asset: &Asset) -> Self {
        Self {
            url: asset.url.to_string(),
            digest: asset.digest,
            archive: asset.archive,
            size: asset.size,
        }
    }
}

/// サーバの取得のしかた（検出表の 1 行が 1 つ持つ。取れないサーバは `None`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fetch {
    /// 単体の実行ファイル（OS と CPU ごとに 1 つ）
    Binary {
        version: &'static str,
        assets: &'static [PlatformAsset],
        /// PATH で見つかったものを信じる前に打つ引数（成功で終わらなければ「入っていない」扱い）。
        /// rustup の代理（`~/.cargo/bin/rust-analyzer`）は component が無くても PATH に居て、
        /// 起こすと即座に落ちる = 黙って出ない状態の最多の型（#1944 の実測）
        probe_args: &'static [&'static str],
    },
    /// npm の tarball（どの OS でも同じ）を Node.js で起こす
    Npm {
        packages: &'static [NpmPackage],
        /// 置き場からの相対パス（`node_modules/<name>/…`）
        entry: &'static str,
        /// これより古い Node.js では起こさない（`engines.node` の下限の major）
        node_major: u32,
    },
}

impl Fetch {
    /// 置き場の中の版の段の名前（版を変えたら別の段 = 古い版を上書きしない）
    pub fn version_key(&self) -> String {
        match self {
            Self::Binary { version, .. } => (*version).to_string(),
            Self::Npm { packages, .. } => packages
                .iter()
                .map(|p| format!("{}-{}", p.name, p.version))
                .collect::<Vec<_>>()
                .join("+"),
        }
    }

    /// この OS / CPU で取れるか
    pub fn available_for(&self, os: &str, arch: &str) -> bool {
        match self {
            Self::Binary { assets, .. } => select(assets, os, arch).is_some(),
            // npm の中身は OS に依らない。Node.js が要る（入っていなければ [`NODE`] を取る）
            Self::Npm { .. } => true,
        }
    }

    /// 取ってくるもの（この OS / CPU 向け。Node.js は含めない = 別に [`NODE`] で決める）
    pub fn assets_for(&self, os: &str, arch: &str) -> Vec<OwnedAsset> {
        match self {
            Self::Binary { assets, .. } => select(assets, os, arch)
                .map(|a| vec![OwnedAsset::from(a)])
                .unwrap_or_default(),
            Self::Npm { packages, .. } => packages.iter().map(NpmPackage::asset).collect(),
        }
    }

    /// 呼び名（`pyright 1.1.414` / `rust-analyzer 2026-09-21`。状態表示と persist.log に出す）
    pub fn label(&self, id: &str) -> String {
        match self {
            Self::Binary { version, .. } => format!("{id} {version}"),
            Self::Npm { packages, .. } => packages
                .first()
                .map(|p| format!("{} {}", p.name, p.version))
                .unwrap_or_else(|| id.to_string()),
        }
    }
}

/// OS と CPU に合う取得物を選ぶ
pub fn select<'a>(assets: &'a [PlatformAsset], os: &str, arch: &str) -> Option<&'a Asset> {
    assets
        .iter()
        .find(|a| a.os == os && a.arch == arch)
        .map(|a| &a.asset)
}

/// npm のサーバを起こす Node.js（利用者の PATH に足りる版が無いときだけ取る）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeRuntime {
    pub version: &'static str,
    pub assets: &'static [PlatformAsset],
}

impl NodeRuntime {
    pub fn label(&self) -> String {
        format!("Node.js {}", self.version)
    }
}

/// 取ってくる Node.js（LTS。2026-09-07 公開の v24.21.0。ハッシュは nodejs.org の SHASUMS256.txt）。
/// 実行ファイル 1 つだけを取り出す（npm・ヘッダは置かない）
pub const NODE: NodeRuntime = NodeRuntime {
    version: "24.21.0",
    assets: &[
        PlatformAsset {
            os: "macos",
            arch: "aarch64",
            asset: Asset {
                url: "https://nodejs.org/dist/v24.21.0/node-v24.21.0-darwin-arm64.tar.gz",
                digest: Digest::Sha256(
                    "bed7eea5325e1108f32ce5228ddd6a5f0f08a499ee42aa7442aea583702f6057",
                ),
                archive: Archive::TarGzMember("node-v24.21.0-darwin-arm64/bin/node"),
                size: 52_909_993,
            },
        },
        PlatformAsset {
            os: "macos",
            arch: "x86_64",
            asset: Asset {
                url: "https://nodejs.org/dist/v24.21.0/node-v24.21.0-darwin-x64.tar.gz",
                digest: Digest::Sha256(
                    "1462cb3b3046b815cf8ea436d3da450ec1a9f11dac7e5a46b0ada5305d7e8097",
                ),
                archive: Archive::TarGzMember("node-v24.21.0-darwin-x64/bin/node"),
                size: 54_203_979,
            },
        },
        PlatformAsset {
            os: "windows",
            arch: "x86_64",
            asset: Asset {
                url: "https://nodejs.org/dist/v24.21.0/node-v24.21.0-win-x64.zip",
                digest: Digest::Sha256(
                    "158f7685b44de51f6c0df1d153526cbcd3e1bc739a8dfc607721cef75de9e541",
                ),
                archive: Archive::ZipMember("node-v24.21.0-win-x64/node.exe"),
                size: 37_618_919,
            },
        },
        PlatformAsset {
            os: "windows",
            arch: "aarch64",
            asset: Asset {
                url: "https://nodejs.org/dist/v24.21.0/node-v24.21.0-win-arm64.zip",
                digest: Digest::Sha256(
                    "8779b1bde1d39f8d420e3b57aa657b39891af434d3de44a919044cec06785921",
                ),
                archive: Archive::ZipMember("node-v24.21.0-win-arm64/node.exe"),
                // 配布元の Content-Length を測っていない（x64 の zip と同じ桁。上限は大きさの 2 倍 + 余白）
                size: 37_000_000,
            },
        },
    ],
};

/// 取得物の置き場の根（`<data_dir>/lsp-servers`）
pub const ROOT_DIR_NAME: &str = "lsp-servers";

/// 済んだ置き場に最後に書く印（中身は何を入れたかの JSON。これが無い段は使わない）
pub const MARKER: &str = ".tako-installed";

/// 使っている印の接頭辞（#1977）。サーバを版の段から起こした tako のプロセスが
/// `<版の段>/.tako-lease-<pid>` を置く。片付けはその pid が生きている段を消さない
/// （古い tako の GUI が古い版で動いているあいだに、新しい tako が消さない）
pub const LEASE_PREFIX: &str = ".tako-lease-";

/// 使っている印のファイル名
pub fn lease_name(pid: u32) -> String {
    format!("{LEASE_PREFIX}{pid}")
}

/// 使っている印のファイル名から pid を読む（印でなければ `None`）
pub fn lease_pid(file_name: &str) -> Option<u32> {
    file_name.strip_prefix(LEASE_PREFIX)?.parse().ok()
}

/// 片付けの対象の候補 1 つ（`<root>/<id>/` の直下の段 1 つ。#1977）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionDir {
    /// 段の名前（版。`pyright-1.1.414` / `2026-09-21` / `24.21.0`）
    pub name: String,
    /// 済んだ印（[`MARKER`]）があるか = tako が置いたと言える
    pub marked: bool,
    /// 入れた時刻（印の `installed_at`。読めなければ印の mtime。秒）
    pub installed_at: Option<u64>,
    /// 生きている tako のプロセスが使っている印を置いている
    pub leased: bool,
    /// symlink（利用者が張ったもの。辿らず触らない）
    pub symlink: bool,
}

/// 片付けで残す理由（`tako lsp status` の `fetch.cleanup.kept[].reason`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepReason {
    /// 生きている tako が使っている（古い GUI が古い版で動いている）
    InUse,
    /// 今の版より後に入った（新しい tako が入れた = 古い tako が新しい版を消さない）
    Newer,
    /// 済んだ印が無い（tako が置いたと言えない = 利用者の物かもしれない）
    Unmarked,
    /// symlink（利用者が張った）
    Symlink,
}

impl KeepReason {
    pub fn slug(self) -> &'static str {
        match self {
            Self::InUse => "in_use",
            Self::Newer => "newer",
            Self::Unmarked => "unmarked",
            Self::Symlink => "symlink",
        }
    }
}

/// 古い版の段を消すかの判定（#1977。I/O を持たない純粋部分）。
///
/// 消すのは **今の版の段が済んでいて、それより前に入った、tako が置いた（印のある）段**だけ。
/// 今の版より後に入った段は残す（同じ data dir を古い tako と新しい tako が共有していても、
/// 古い側が新しい版を消さない = 互いに消し合って取り直し続けない）。使っている印が生きている段・
/// 印の無い段・symlink も残す。今の版の段が済んでいなければ何も消さない（比べる基準が無い）。
///
/// 返すのは今の版以外の各段の `Ok(())`（消す）/ `Err(理由)`（残す）
pub fn sweep_plan(
    current: &str,
    current_installed_at: Option<u64>,
    dirs: &[VersionDir],
) -> Vec<(String, Result<(), KeepReason>)> {
    let Some(current_at) = current_installed_at else {
        return Vec::new();
    };
    dirs.iter()
        .filter(|d| d.name != current)
        .map(|d| {
            let verdict = if d.symlink {
                Err(KeepReason::Symlink)
            } else if !d.marked {
                Err(KeepReason::Unmarked)
            } else if d.leased {
                Err(KeepReason::InUse)
            } else if d.installed_at.is_none_or(|at| at > current_at) {
                Err(KeepReason::Newer)
            } else {
                Ok(())
            };
            (d.name.clone(), verdict)
        })
        .collect()
}

/// 済んだ印の中身から入れた時刻（秒）を読む
pub fn installed_at_of(marker: &str) -> Option<u64> {
    serde_json::from_str::<serde_json::Value>(marker)
        .ok()?
        .get("installed_at")?
        .as_u64()
}

/// サーバの置き場（`<root>/<id>/<版>`）
pub fn install_dir(root: &Path, id: &str, fetch: &Fetch) -> PathBuf {
    root.join(id).join(fetch.version_key())
}

/// Node.js の置き場（`<root>/node/<版>`）
pub fn node_dir(root: &Path, node: &NodeRuntime) -> PathBuf {
    root.join("node").join(node.version)
}

/// 置き場の中の Node.js の実行ファイル
pub fn node_program(dir: &Path, windows: bool) -> PathBuf {
    dir.join(if windows { "node.exe" } else { "node" })
}

/// 置き場の中のサーバの実行ファイル（`Binary` のとき）
pub fn binary_program(dir: &Path, program: &str, windows: bool) -> PathBuf {
    if windows {
        dir.join(format!("{program}.exe"))
    } else {
        dir.join(program)
    }
}

/// ミラーの口（`TAKO_LSP_FETCH_BASE`）: `https://<host>/<path>` を `<base>/<host>/<path>` へ替える。
/// ハッシュは固定値で検証するので、ミラーが中身を差し替えても使われない（社内ミラー・検証用）
pub fn rewrite_url(url: &str, base: Option<&str>) -> String {
    let Some(base) = base.map(str::trim).filter(|b| !b.is_empty()) else {
        return url.to_string();
    };
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    format!("{}/{}", base.trim_end_matches('/'), rest)
}

/// tar / zip の中のパスを置き場の中の相対パスへ直す（`..`・絶対パス・ドライブ文字は拒む）。
/// `strip` は先頭から外す段の数（npm の tarball は `package/`）
pub fn safe_relative(name: &str, strip: usize) -> Option<PathBuf> {
    let normalized = name.replace('\\', "/");
    if normalized.starts_with('/') {
        return None;
    }
    let mut parts = Vec::new();
    for part in normalized.split('/') {
        match part {
            "" | "." => continue,
            ".." => return None,
            p if p.contains(':') => return None,
            p => parts.push(p),
        }
    }
    if parts.len() <= strip {
        return None;
    }
    Some(parts[strip..].iter().collect())
}

/// `node --version` の出力（`v24.8.0`）から major を読む
pub fn node_major_of(version_output: &str) -> Option<u32> {
    version_output
        .trim()
        .trim_start_matches('v')
        .split('.')
        .next()?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::servers::SERVERS;

    #[test]
    fn npm_の_url_はレジストリの形() {
        let p = NpmPackage {
            name: "pyright",
            version: "1.1.414",
            integrity: "x",
            size: 1,
        };
        assert_eq!(
            p.url(),
            "https://registry.npmjs.org/pyright/-/pyright-1.1.414.tgz"
        );
    }

    #[test]
    fn ミラーは_host_から後ろを足す() {
        let url = "https://nodejs.org/dist/v24.21.0/node.tar.gz";
        assert_eq!(rewrite_url(url, None), url);
        assert_eq!(rewrite_url(url, Some("  ")), url);
        assert_eq!(
            rewrite_url(url, Some("http://127.0.0.1:9/m/")),
            "http://127.0.0.1:9/m/nodejs.org/dist/v24.21.0/node.tar.gz"
        );
    }

    #[test]
    fn 展開のパスは外へ出られない() {
        assert_eq!(
            safe_relative("package/lib/a.js", 1),
            Some(PathBuf::from("lib/a.js"))
        );
        assert_eq!(
            safe_relative("./package/./a.js", 1),
            Some(PathBuf::from("a.js"))
        );
        assert_eq!(safe_relative("package/../../etc/passwd", 1), None);
        assert_eq!(safe_relative("/etc/passwd", 0), None);
        assert_eq!(safe_relative("C:/Windows/x", 0), None);
        assert_eq!(safe_relative("package\\..\\x", 1), None);
        assert_eq!(safe_relative("package/", 1), None, "外した後に何も残らない");
    }

    #[test]
    fn node_の版を読む() {
        assert_eq!(node_major_of("v24.8.0\n"), Some(24));
        assert_eq!(node_major_of("v14.0.0"), Some(14));
        assert_eq!(node_major_of("garbage"), None);
    }

    #[test]
    fn 置き場は版ごとに分かれる() {
        let root = Path::new("/data/lsp-servers");
        for spec in SERVERS {
            let Some(fetch) = &spec.fetch else { continue };
            let dir = install_dir(root, spec.id, fetch);
            assert!(dir.starts_with(root.join(spec.id)));
            assert!(!fetch.version_key().is_empty());
            assert!(!fetch.version_key().contains('/'));
        }
        assert_eq!(node_dir(root, &NODE), root.join("node").join(NODE.version));
        assert_eq!(
            node_program(Path::new("/n"), true),
            PathBuf::from("/n/node.exe")
        );
    }

    /// 表の値の形（ハッシュの桁・URL の配布元・版の固定）を縛る。値そのものは配布元の公開値で、
    /// どこから来たかは `.agent/plans/2026-09-lsp-s1.md` §23 に書いてある
    #[test]
    fn 取得物はどれも版を固定し配布元のハッシュを持つ() {
        let mut checked = 0;
        let check = |asset: &OwnedAsset| {
            assert!(asset.url.starts_with("https://"), "{}", asset.url);
            assert!(
                [
                    "https://registry.npmjs.org/",
                    "https://nodejs.org/dist/",
                    "https://github.com/"
                ]
                .iter()
                .any(|origin| asset.url.starts_with(origin)),
                "配布元以外から取らない: {}",
                asset.url
            );
            match asset.digest {
                Digest::Sha256(hex) => {
                    assert_eq!(hex.len(), 64, "{}", asset.url);
                    assert!(hex.bytes().all(|b| b.is_ascii_hexdigit()));
                }
                Digest::Sha512Base64(b64) => {
                    // SHA-512 = 64 バイト = base64 で 88 字（末尾 `==`）
                    assert_eq!(b64.len(), 88, "{}", asset.url);
                    assert!(b64.ends_with("=="));
                }
            }
            assert!(asset.size > 0);
        };
        for spec in SERVERS {
            let Some(fetch) = &spec.fetch else { continue };
            for (os, arch) in [
                ("macos", "aarch64"),
                ("macos", "x86_64"),
                ("windows", "x86_64"),
                ("windows", "aarch64"),
            ] {
                assert!(fetch.available_for(os, arch), "{} {os} {arch}", spec.id);
                for asset in fetch.assets_for(os, arch) {
                    check(&asset);
                    checked += 1;
                }
            }
        }
        for platform in NODE.assets {
            check(&OwnedAsset::from(&platform.asset));
        }
        assert!(checked > 0);
        // 版は日付やタグで固定する（latest / ^ / ~ を使わない）
        for spec in SERVERS {
            if let Some(Fetch::Npm { packages, .. }) = &spec.fetch {
                for p in *packages {
                    assert!(p.version.chars().all(|c| c.is_ascii_digit() || c == '.'));
                }
            }
        }
    }

    fn dir(name: &str, at: Option<u64>) -> VersionDir {
        VersionDir {
            name: name.to_string(),
            marked: true,
            installed_at: at,
            leased: false,
            symlink: false,
        }
    }

    #[test]
    fn 片付けは今の版より前に入った印のある段だけを消す() {
        let dirs = vec![
            dir("v3", Some(300)),
            dir("v1", Some(100)),
            dir("v2-same-time", Some(300)),
            dir("v9-newer", Some(900)),
            VersionDir {
                leased: true,
                ..dir("v0-in-use", Some(50))
            },
            VersionDir {
                marked: false,
                ..dir("user-dir", None)
            },
            VersionDir {
                symlink: true,
                ..dir("link", Some(10))
            },
            dir("no-time", None),
        ];
        let plan = sweep_plan("v3", Some(300), &dirs);
        let verdict = |name: &str| {
            plan.iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| *v)
                .unwrap_or_else(|| panic!("{name} が判定に無い"))
        };
        assert!(
            plan.iter().all(|(n, _)| n != "v3"),
            "今の版は判定に載らない"
        );
        assert_eq!(verdict("v1"), Ok(()));
        assert_eq!(verdict("v2-same-time"), Ok(()), "同じ時刻は前とみなす");
        assert_eq!(verdict("v9-newer"), Err(KeepReason::Newer));
        assert_eq!(verdict("v0-in-use"), Err(KeepReason::InUse));
        assert_eq!(verdict("user-dir"), Err(KeepReason::Unmarked));
        assert_eq!(verdict("link"), Err(KeepReason::Symlink));
        assert_eq!(
            verdict("no-time"),
            Err(KeepReason::Newer),
            "入れた時刻が分からない段は消さない側へ倒す"
        );
        assert!(
            sweep_plan("v3", None, &dirs).is_empty(),
            "今の版が済んでいなければ何も消さない"
        );
    }

    #[test]
    fn 使っている印と入れた時刻を読む() {
        assert_eq!(lease_name(4242), ".tako-lease-4242");
        assert_eq!(lease_pid(".tako-lease-4242"), Some(4242));
        assert_eq!(lease_pid(".tako-lease-x"), None);
        assert_eq!(lease_pid(MARKER), None);
        assert_eq!(
            installed_at_of(r#"{"label":"x","installed_at":1760000000}"#),
            Some(1_760_000_000)
        );
        assert_eq!(installed_at_of("{壊れた"), None);
        assert_eq!(installed_at_of(r#"{"label":"x"}"#), None);
    }

    #[test]
    fn 取れない組は選ばれない() {
        let fetch = SERVERS
            .iter()
            .find_map(|s| match s.fetch {
                Some(f @ Fetch::Binary { .. }) => Some(f),
                _ => None,
            })
            .expect("単体バイナリの行がある");
        assert!(!fetch.available_for("linux", "riscv64"));
        assert!(fetch.assets_for("linux", "riscv64").is_empty());
    }
}
