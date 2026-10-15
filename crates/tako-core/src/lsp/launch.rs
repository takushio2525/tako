//! 言語サーバの起動引数の組み立てと、実行ファイルの置き場の探索（#1951）の純粋部分
//!
//! 検出表の行（[`super::servers::ServerSpec`]）が持つ取り決めを、見つけたパスの並びにする:
//!
//! - **サーバそのもの**: 差し替えの env → 利用者の PATH（呼び手が上限つきで引く）→ 行の `bundled`
//!   （アプリの同梱。置き場の知識は [`crate::app_bundle`] の表 1 枚 = Code Runner の #1950 と同じ 1 実装）。
//!   ここが持つのは最後の 1 段（[`bundled_server`]）だけ
//! - **伴走**（[`Arg::Companion`]）: 見つけたサーバと**同じディレクトリ**を先に → PATH → `bundled`。
//!   Arduino IDE は arduino-language-server・arduino-cli・clangd を同じ場所に並べていて、その版の
//!   組み合わせで動くことを実測で確かめた（PATH の別の clangd を先に採らない）
//! - **空のファイル**（[`Arg::EmptyFile`]）: `<config_dir>/<id>/<名前>`（作るのは呼び手 = I/O）
//!
//! ファイルシステムは [`FsProbe`]、PATH は関数で受けるので、macOS の単体から Windows の置き場
//! （`.exe`）まで偽のファイルシステムで固定できる（#1616 / #1655 の作法）。
//! **言語名・実行ファイル名を書かない**（表の値は表の中だけ = `issue1678_lsp_watchdog`）。

use std::path::{Path, PathBuf};

use super::servers::{Arg, ServerSpec};
use crate::platform::support::Platform;
use crate::runtime_env::{DetectEnv, FsProbe};

/// 置き場を探すための周り（OS・PATH の引き方・同梱の置き場を解く環境・ファイルシステム）
pub struct Locator<'a> {
    pub platform: Platform,
    /// 名前を利用者の PATH で引く（unix はログインシェル = 待ちうるので、呼び手が上限つきで渡す）
    pub on_path: &'a dyn Fn(&str) -> Option<PathBuf>,
    pub env: &'a DetectEnv,
    pub fs: &'a dyn FsProbe,
}

/// どこで見つけたか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    /// 見つけたサーバと同じディレクトリ
    SameDir,
    /// 利用者の PATH
    Path,
    /// 行の `bundled`（アプリの同梱）
    Bundled,
}

/// 見つけた伴走 1 つ
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Companion {
    pub name: &'static str,
    pub path: PathBuf,
    pub found: Found,
}

impl Companion {
    /// 人に案内するコマンドの先頭に置く形（PATH で見つけたなら名前のまま = #322 の最簡形。
    /// それ以外は PATH から打てないのでフルパス）
    pub fn command_name(&self) -> String {
        match self.found {
            Found::Path => self.name.to_string(),
            Found::SameDir | Found::Bundled => self.path.to_string_lossy().into_owned(),
        }
    }
}

/// 組み立てた起動
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Built {
    /// 起動引数（表の並びのまま）
    pub args: Vec<String>,
    pub companions: Vec<Companion>,
    /// 呼び手が 0 バイトで用意するファイル（無ければ作る）
    pub empty_files: Vec<PathBuf>,
}

/// その OS での実行ファイルの名前（Windows は `.exe` を足す。既に付いていればそのまま）
pub fn exe_name(program: &str, platform: Platform) -> String {
    match platform {
        Platform::Windows if !program.to_ascii_lowercase().ends_with(".exe") => {
            format!("{program}.exe")
        }
        _ => program.to_string(),
    }
}

/// 実プロセスの環境（[`DetectEnv::from_process`] に、行の `bundled` の置き場が名指す環境変数を足す =
/// Code Runner の表に載っていない同梱を言語サーバの表だけが持っても読み先が欠けない）
pub fn process_env(spec: &ServerSpec) -> DetectEnv {
    let mut env = DetectEnv::from_process();
    if let Some(bundle) = spec.bundled {
        for dir in bundle.dirs.macos.iter().chain(bundle.dirs.windows) {
            if let crate::runtime_env::KnownDir::Var(name, _) = dir {
                if let Ok(value) = std::env::var(name) {
                    env.vars.insert((*name).to_string(), value);
                }
            }
        }
    }
    env
}

/// サーバそのものを行の `bundled` から探す（差し替えの env と PATH で見つからなかったときの最後の段）
pub fn bundled_server(spec: &ServerSpec, loc: &Locator<'_>) -> Option<PathBuf> {
    bundled(spec, spec.program, loc)
}

fn bundled(spec: &ServerSpec, name: &str, loc: &Locator<'_>) -> Option<PathBuf> {
    let bundle = spec.bundled?;
    crate::app_bundle::find(
        bundle,
        loc.platform,
        &exe_name(name, loc.platform),
        loc.env,
        loc.fs,
    )
}

/// 伴走 `name` を探す: 見つけたサーバ `server` と同じディレクトリ → PATH → 行の `bundled`
pub fn find_companion(
    spec: &ServerSpec,
    name: &'static str,
    server: &Path,
    loc: &Locator<'_>,
) -> Option<Companion> {
    let file = exe_name(name, loc.platform);
    let found = |path: PathBuf, found| Companion { name, path, found };
    if let Some(dir) = server.parent().filter(|d| !d.as_os_str().is_empty()) {
        let sibling = dir.join(&file);
        if loc.fs.is_file(&sibling) {
            return Some(found(sibling, Found::SameDir));
        }
    }
    if let Some(path) = (loc.on_path)(name) {
        return Some(found(path, Found::Path));
    }
    bundled(spec, name, loc).map(|path| found(path, Found::Bundled))
}

/// 空のファイルの置き場（`<config_dir>/<id>/<名前>`）
pub fn empty_file_path(config_dir: &Path, spec: &ServerSpec, name: &str) -> PathBuf {
    config_dir.join(spec.id).join(name)
}

/// 起動引数を組み立てる。伴走が 1 つでも見つからなければ `Err(その名前)`（サーバを起こさない）
pub fn build(
    spec: &ServerSpec,
    server: &Path,
    loc: &Locator<'_>,
    config_dir: &Path,
) -> Result<Built, &'static str> {
    let mut built = Built {
        args: Vec::with_capacity(spec.args.len()),
        companions: Vec::new(),
        empty_files: Vec::new(),
    };
    for arg in spec.args {
        match *arg {
            Arg::Lit(value) => built.args.push(value.to_string()),
            Arg::Companion(name) => {
                let companion = find_companion(spec, name, server, loc).ok_or(name)?;
                built
                    .args
                    .push(companion.path.to_string_lossy().into_owned());
                built.companions.push(companion);
            }
            Arg::EmptyFile(name) => {
                let path = empty_file_path(config_dir, spec, name);
                built.args.push(path.to_string_lossy().into_owned());
                built.empty_files.push(path);
            }
        }
    }
    Ok(built)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_bundle::ARDUINO_IDE;
    use crate::lsp::servers::{InstallHint, RootScope};
    use std::collections::{BTreeMap, BTreeSet};

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

    /// 表の値に依存しないテスト用の行（本物の表の行を名指さない）
    const 伴走あり: ServerSpec = ServerSpec {
        id: "srv",
        documents: &[],
        program: "srv-ls",
        args: &[
            Arg::Lit("-a"),
            Arg::Companion("tool-a"),
            Arg::Lit("-b"),
            Arg::Companion("tool-b"),
            Arg::Lit("-c"),
            Arg::EmptyFile("conf.yaml"),
        ],
        root_markers: &[],
        root_scope: RootScope::FileDir,
        workspace_root: None,
        bundled: Some(&ARDUINO_IDE),
        requires: None,
        install: InstallHint {
            macos: "",
            windows: "",
        },
        fetch: None,
    };

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

    const IDE: &str = "/Applications/Arduino IDE.app/Contents/Resources/app/lib/backend/resources";

    fn locate<'a>(
        platform: Platform,
        on_path: &'a dyn Fn(&str) -> Option<PathBuf>,
        env: &'a DetectEnv,
        fs: &'a dyn FsProbe,
    ) -> Locator<'a> {
        Locator {
            platform,
            on_path,
            env,
            fs,
        }
    }

    #[test]
    fn 引数は表の並びのまま伴走と空のファイルのパスを差し込む() {
        let e = env("/Users/testuser", &[]);
        let fs = Files(
            [
                PathBuf::from("/opt/x/tool-a"),
                PathBuf::from("/opt/x/tool-b"),
            ]
            .into(),
        );
        let no_path = |_: &str| None;
        let loc = locate(Platform::MacOs, &no_path, &e, &fs);
        let built = build(
            &伴走あり,
            Path::new("/opt/x/srv-ls"),
            &loc,
            Path::new("/data/lsp-config"),
        )
        .unwrap();
        assert_eq!(
            built.args,
            [
                "-a",
                "/opt/x/tool-a",
                "-b",
                "/opt/x/tool-b",
                "-c",
                "/data/lsp-config/srv/conf.yaml"
            ]
        );
        assert_eq!(
            built.empty_files,
            [PathBuf::from("/data/lsp-config/srv/conf.yaml")]
        );
        assert!(built.companions.iter().all(|c| c.found == Found::SameDir));
    }

    /// 同じディレクトリ → PATH → 同梱の順。PATH の別の版より同じディレクトリを先に採る
    #[test]
    fn 伴走は同じディレクトリから_path_同梱の順に探す() {
        let e = env("/Users/testuser", &[]);
        let in_ide = PathBuf::from(format!("{IDE}/tool-a"));
        let fs = Files([PathBuf::from("/opt/x/tool-a"), in_ide.clone()].into());
        let on_path = |name: &str| (name == "tool-a").then(|| PathBuf::from("/usr/bin/tool-a"));
        let loc = locate(Platform::MacOs, &on_path, &e, &fs);
        let got = find_companion(&伴走あり, "tool-a", Path::new("/opt/x/srv-ls"), &loc).unwrap();
        assert_eq!(
            (got.path.as_path(), got.found),
            (Path::new("/opt/x/tool-a"), Found::SameDir)
        );
        // 同じディレクトリに無ければ PATH
        let got =
            find_companion(&伴走あり, "tool-a", Path::new("/elsewhere/srv-ls"), &loc).unwrap();
        assert_eq!(
            (got.path.as_path(), got.found),
            (Path::new("/usr/bin/tool-a"), Found::Path)
        );
        assert_eq!(got.command_name(), "tool-a", "PATH なら名前のまま（#322）");
        // PATH にも無ければ同梱
        let no_path = |_: &str| None;
        let loc = locate(Platform::MacOs, &no_path, &e, &fs);
        let got =
            find_companion(&伴走あり, "tool-a", Path::new("/elsewhere/srv-ls"), &loc).unwrap();
        assert_eq!(
            (got.path.clone(), got.found),
            (in_ide.clone(), Found::Bundled)
        );
        assert_eq!(got.command_name(), in_ide.to_string_lossy());
        // どこにも無ければ組み立てない（見つからない伴走の名前を返す）
        assert_eq!(
            build(
                &伴走あり,
                Path::new("/elsewhere/srv-ls"),
                &loc,
                Path::new("/d")
            ),
            Err("tool-b")
        );
    }

    #[test]
    fn サーバそのものは同梱を最後の段で探す() {
        let e = env("/Users/testuser", &[]);
        let no_path = |_: &str| None;
        let server = PathBuf::from(format!("{IDE}/srv-ls"));
        let fs = Files([server.clone()].into());
        let loc = locate(Platform::MacOs, &no_path, &e, &fs);
        assert_eq!(bundled_server(&伴走あり, &loc), Some(server));
        // 同梱を持たない行は探さない
        let plain = ServerSpec {
            bundled: None,
            ..伴走あり
        };
        assert_eq!(bundled_server(&plain, &loc), None);
        let empty = Files(BTreeSet::new());
        let loc = locate(Platform::MacOs, &no_path, &e, &empty);
        assert_eq!(bundled_server(&伴走あり, &loc), None);
    }

    /// Windows は `.exe` の名前で同じディレクトリと同梱（環境変数から組む置き場）を探す（実機は未確認）
    #[test]
    fn windowsは_exe_の名前で探す() {
        assert_eq!(exe_name("tool-a", Platform::Windows), "tool-a.exe");
        assert_eq!(exe_name("tool-a.EXE", Platform::Windows), "tool-a.EXE");
        assert_eq!(exe_name("tool-a", Platform::MacOs), "tool-a");
        let e = env(
            r"C:\Users\winuser",
            &[("LOCALAPPDATA", r"C:\Users\winuser\AppData\Local")],
        );
        let ide = crate::app_bundle::dirs(&ARDUINO_IDE, Platform::Windows, &e)[0].clone();
        let server = ide.join("srv-ls.exe");
        let fs = Files(
            [
                server.clone(),
                ide.join("tool-a.exe"),
                ide.join("tool-b.exe"),
            ]
            .into(),
        );
        let no_path = |_: &str| None;
        let loc = locate(Platform::Windows, &no_path, &e, &fs);
        assert_eq!(bundled_server(&伴走あり, &loc), Some(server.clone()));
        let built = build(&伴走あり, &server, &loc, Path::new(r"C:\data\lsp-config")).unwrap();
        assert_eq!(built.args[1], ide.join("tool-a.exe").to_string_lossy());
        assert_eq!(built.args[3], ide.join("tool-b.exe").to_string_lossy());
        assert!(built.companions.iter().all(|c| c.found == Found::SameDir));
    }
}
