//! アプリに同梱された道具の置き場（Issue #1950 / #1951。設計 `.agent/plans/2026-10-language-coverage.md` §5.3 / §5.4）
//!
//! **何のためにあるか**: Arduino IDE は `arduino-cli`・`arduino-language-server`・`clangd` を
//! アプリの中に同梱していて、PATH には出さない。授業で IDE を入れた人の PC はまさにこの形で
//! （PATH に arduino-cli が無く、IDE の中にだけある）、PATH だけを見ると「入っていない」と
//! 誤って案内することになる。
//!
//! ## 1 実装を 2 つの層が使う
//!
//! - Code Runner の実行環境の表（`runtime_env::kinds` の `arduino` 行の `Bundled` 検出器）が
//!   `${arduino_cli}` を解くとき
//! - 言語サーバの検出表（#1951）が ALS と伴走の `clangd` / `arduino-cli` を探すとき
//!
//! 置き場の知識（アプリの中のどのディレクトリか）はここの表 1 枚だけが持ち、探し方は
//! [`find`] / [`dirs`] の純粋関数 1 つずつにする。どちらも [`Platform`] を**引数で受け**、
//! ファイルシステムは [`FsProbe`] 越しに見るので、macOS の単体から Windows の置き場まで
//! 偽のファイルシステムで固定できる（#1616 / #1655 の作法）。
//!
//! ## Windows の列は未実機
//!
//! Arduino IDE 2.x の Windows 版は electron-builder の NSIS で入り、ユーザー単位の既定は
//! `%LOCALAPPDATA%\Programs\Arduino IDE`、全ユーザー（管理者）なら `%ProgramFiles%\Arduino IDE`。
//! 同梱物はその下の `resources\app\lib\backend\resources`（macOS の `Contents/Resources/app/…` と
//! 同じ並び）と見ているが、**Windows 実機では確かめていない**。`tako platform` の既知の制限に
//! 申告してある（`platform::support::notes::WIN_RUN_ARDUINO_UNVERIFIED`）

use std::path::PathBuf;

use crate::platform::support::Platform;
use crate::runtime_env::{DetectEnv, FsProbe, KnownDir, PerOs};

/// 道具を同梱するアプリ 1 つぶんの行
#[derive(Debug, PartialEq, Eq)]
pub struct AppBundle {
    /// 安定 ID（実行環境の検出器 ID・応答の `manager` に出る）
    pub id: &'static str,
    /// 人に見せる名前
    pub label: &'static str,
    /// 同梱の実行ファイルが並ぶディレクトリ（OS ごと。**並びが探す順**）
    pub dirs: PerOs<&'static [KnownDir]>,
}

/// Arduino IDE 2.x（2.3.5 の同梱物で実測: arduino-cli 1.2.0・arduino-language-server・
/// clangd 14.0.0・clang-format）
pub const ARDUINO_IDE: AppBundle = AppBundle {
    id: "arduino-ide",
    label: "Arduino IDE",
    dirs: PerOs {
        macos: &[
            // 既定（`brew install --cask arduino-ide` / dmg を /Applications へ置いた形）
            KnownDir::Abs(
                "/Applications/Arduino IDE.app/Contents/Resources/app/lib/backend/resources",
            ),
            // 利用者のアプリケーションフォルダ（管理者権限なしで置いた形）
            KnownDir::Home(&[
                "Applications",
                "Arduino IDE.app",
                "Contents",
                "Resources",
                "app",
                "lib",
                "backend",
                "resources",
            ]),
        ],
        windows: &[
            // ユーザー単位の既定（未実機）
            KnownDir::Var(
                "LOCALAPPDATA",
                &[
                    "Programs",
                    "Arduino IDE",
                    "resources",
                    "app",
                    "lib",
                    "backend",
                    "resources",
                ],
            ),
            // 全ユーザー（未実機）
            KnownDir::Var(
                "ProgramFiles",
                &[
                    "Arduino IDE",
                    "resources",
                    "app",
                    "lib",
                    "backend",
                    "resources",
                ],
            ),
        ],
    },
};

/// 表の全行（`id` と環境変数の名前を引く側が走査する）
pub const BUNDLES: &[&AppBundle] = &[&ARDUINO_IDE];

/// `bundle` の同梱ディレクトリの候補（解けたものだけ。並びは探す順）。
///
/// 言語サーバの伴走（#1951）は「見つけた ALS と同じディレクトリを先に」探すので、
/// ファイル名を決めずにディレクトリだけを返す口も持つ
pub fn dirs(bundle: &AppBundle, platform: Platform, env: &DetectEnv) -> Vec<PathBuf> {
    bundle
        .dirs
        .get(platform)
        .iter()
        .filter_map(|d| d.resolve(env))
        .collect()
}

/// `file_name`（`arduino-cli` / `arduino-cli.exe`）を `bundle` の同梱ディレクトリから探す。
/// 最初に在ったもののフルパス。無ければ `None`
pub fn find(
    bundle: &AppBundle,
    platform: Platform,
    file_name: &str,
    env: &DetectEnv,
    fs: &dyn FsProbe,
) -> Option<PathBuf> {
    if file_name.is_empty() {
        return None;
    }
    dirs(bundle, platform, env)
        .into_iter()
        .map(|d| d.join(file_name))
        .find(|p| fs.is_file(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;

    /// 在るファイルの集合だけを持つ偽のファイルシステム
    struct Files(BTreeSet<PathBuf>);

    impl FsProbe for Files {
        fn is_file(&self, path: &Path) -> bool {
            self.0.contains(path)
        }
        fn is_dir(&self, _: &Path) -> bool {
            false
        }
        fn read_head(&self, _: &Path, _: usize) -> Option<String> {
            None
        }
        fn list_dir(&self, _: &Path) -> Vec<String> {
            Vec::new()
        }
    }

    fn env(home: &str, vars: &[(&str, &str)]) -> DetectEnv {
        DetectEnv {
            home: Some(PathBuf::from(home)),
            vars: vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>(),
            path_dirs: Vec::new(),
        }
    }

    #[test]
    fn macosは既定のアプリケーションフォルダを先に探す() {
        let e = env("/Users/testuser", &[]);
        let sys = PathBuf::from(
            "/Applications/Arduino IDE.app/Contents/Resources/app/lib/backend/resources/arduino-cli",
        );
        let user = PathBuf::from(
            "/Users/testuser/Applications/Arduino IDE.app/Contents/Resources/app/lib/backend/resources/arduino-cli",
        );
        let both = Files([sys.clone(), user.clone()].into());
        assert_eq!(
            find(&ARDUINO_IDE, Platform::MacOs, "arduino-cli", &e, &both),
            Some(sys)
        );
        let only_user = Files([user.clone()].into());
        assert_eq!(
            find(&ARDUINO_IDE, Platform::MacOs, "arduino-cli", &e, &only_user),
            Some(user)
        );
        assert_eq!(
            find(
                &ARDUINO_IDE,
                Platform::MacOs,
                "arduino-cli",
                &e,
                &Files(BTreeSet::new())
            ),
            None
        );
    }

    /// Windows の列は環境変数から組む（macOS の単体から固定する。実機は未確認）
    #[test]
    fn windowsはユーザー単位の既定から全ユーザーの順() {
        let e = env(
            r"C:\Users\winuser",
            &[
                ("LOCALAPPDATA", r"C:\Users\winuser\AppData\Local"),
                ("ProgramFiles", r"C:\Program Files"),
            ],
        );
        let got = dirs(&ARDUINO_IDE, Platform::Windows, &e);
        assert_eq!(got.len(), 2, "{got:?}");
        let first = got[0].to_string_lossy().replace('\\', "/");
        let second = got[1].to_string_lossy().replace('\\', "/");
        assert!(
            first.starts_with("C:/Users/winuser/AppData/Local/Programs/Arduino IDE/resources/app"),
            "{first}"
        );
        assert!(
            second.starts_with("C:/Program Files/Arduino IDE/resources/app"),
            "{second}"
        );
        let exe = got[1].join("arduino-cli.exe");
        let fs = Files([exe.clone()].into());
        assert_eq!(
            find(&ARDUINO_IDE, Platform::Windows, "arduino-cli.exe", &e, &fs),
            Some(exe)
        );
        // 環境変数が無ければその置き場は解けない（推測で組まない）
        assert!(dirs(&ARDUINO_IDE, Platform::Windows, &env("/h", &[])).is_empty());
    }

    #[test]
    fn 空の名前は探さない() {
        let e = env("/Users/testuser", &[]);
        let dir = dirs(&ARDUINO_IDE, Platform::MacOs, &e)[0].clone();
        // ディレクトリそのものが `is_file` に見える壊れた偽物でも、空の名前では拾わない
        let fs = Files([dir.join("")].into());
        assert_eq!(find(&ARDUINO_IDE, Platform::MacOs, "", &e, &fs), None);
    }

    /// 表の行が整っている（id の重複なし・どちらの OS にも置き場がある・成分に区切りを書かない）
    #[test]
    fn 表の行が整っている() {
        let mut ids = BTreeSet::new();
        for b in BUNDLES {
            assert!(ids.insert(b.id), "id {} が重複", b.id);
            assert!(!b.dirs.macos.is_empty(), "{}: macOS の置き場が無い", b.id);
            assert!(
                !b.dirs.windows.is_empty(),
                "{}: Windows の置き場が無い",
                b.id
            );
            for d in b.dirs.macos.iter().chain(b.dirs.windows) {
                let comps: &[&str] = match d {
                    KnownDir::Home(c) | KnownDir::Var(_, c) => c,
                    KnownDir::Abs(_) => &[],
                };
                for c in comps {
                    assert!(
                        !c.contains('/') && !c.contains('\\'),
                        "{}: 成分 `{c}` に区切りがある",
                        b.id
                    );
                }
            }
        }
    }
}
