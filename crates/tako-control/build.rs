//! PWA（`web/tako-remote/dist/`）が無いツリーでも `cargo build` が通るようにする（#1309 / #574）。
//!
//! `remote.rs` の rust_embed（`#[folder = "../../web/tako-remote/dist/"]`）はビルド済みの
//! PWA を**コンパイル時**に要求する。`dist/` は `.gitignore` 対象なので、
//! `git worktree add` 直後・クリーンチェックアウトのツリーには存在せず、
//! `#[derive(RustEmbed)] folder ... does not exist` → `PwaAssets::get` 未定義でコンパイルが落ちる。
//! CI は Rust の各ステップの前に npm build を挟んで回避しているが（#574）、
//! worker ごとに `../tako-wt-NNN` を切るローカルの worktree 運用には同じ手当てが無く、
//! 「共有ツリーから `dist/` をコピー」という手作業が毎回発生していた。
//!
//! ここが「`dist/index.html` が無ければ PWA をビルドする」の 1 か所。
//! **既にある `dist/` は一切触らない**ので、PWA がある環境ではビルド時間も生成物も変わらない
//! （CI は npm build 済みの状態でここへ来るため二重ビルドにならない）。
//! npm が無い / npm が失敗した場合は「何をすればよいか」が**1 行目に出る**エラーで止める。
//! 「dist が無ければ空の埋め込みで通す」は採らない（製品バイナリに PWA が入らないまま
//! 気付けない事故の余地を作るため）。
//!
//! シェルから単独で叩く経路は `scripts/build-pwa.sh`（`scripts/build-app.sh` もそれを呼ぶ）。
//! こちらが npm を直接起こすのは、Windows の開発機に bash があるとは限らないため。
//!
//! 判定（`pwa::plan`）と実行（`pwa::run`）はテストから `#[path = "../build.rs"]` で
//! **そのまま**取り込む（写しを作らない）ので、実 npm を起こさずに
//! dist 有 / 無 / npm 無の 3 ケースを固定できる（`tests/pwa_dist_build.rs`）。

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR はビルドスクリプトに必ず渡る");
    let pwa_dir = pwa::dir_from_manifest(std::path::Path::new(&manifest_dir));

    // rust_embed の proc-macro は cargo へ再ビルド条件を伝えられない（本家 README の注記）ので、
    // 埋め込み元をここで宣言する。これが無いと「PWA を作り直したのに埋め込みが古いまま」になる。
    // dist が無い初回はこの宣言自体が「変化あり」になり、生成できた 2 回目から静かになる
    println!(
        "cargo:rerun-if-changed={}",
        pwa::dist_dir(&pwa_dir).display()
    );

    match pwa::plan(&pwa_dir, pwa::find_npm(std::env::var_os("PATH").as_deref())) {
        // 既にある dist は触らない = この経路が「変わらないこと」を担保する
        pwa::Plan::UpToDate => {}
        pwa::Plan::Build { npm, install_deps } => {
            // npm が走るとビルドが数十秒延びる。理由が見えないと「なぜか遅い」になるので宣言する
            println!(
                "cargo::warning=PWA（web/tako-remote/dist）が無いので自動でビルドする（#1309）。\
                 単独で叩くなら scripts/build-pwa.sh"
            );
            if let Err(message) = pwa::run(&pwa_dir, &npm, install_deps) {
                fail(&message);
            }
            // npm が成功しても出力先が想定と違えば rust_embed はやはり落ちる。
            // 「ビルドは通ったのに同じエラー」で 1 往復させないため、ここで確かめる
            if let Err(message) = pwa::verify_built(&pwa_dir) {
                fail(&message);
            }
        }
        pwa::Plan::MissingNpm => fail(&pwa::missing_npm_message(&pwa_dir)),
    }
}

/// ビルドを止める。`panic!` ではなく stderr + 終了コードにするのは、
/// cargo が `--- stderr` として本文だけを見せてくれる（バックトレースで案内が埋もれない）ため
fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}

/// PWA の置き場を見て、何をするかを決める。`main` とテストが同じ 1 実装を見る
pub mod pwa {
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// このビルドで PWA に対して何をするか
    #[derive(Debug, PartialEq, Eq)]
    pub enum Plan {
        /// `dist/index.html` がある = 何もしない（既存の dist は触らない）
        UpToDate,
        /// PWA をビルドする。`npm` の実体パスと、`npm ci` が先に要るか
        Build { npm: PathBuf, install_deps: bool },
        /// npm が見つからない = 手順を案内して止める
        MissingNpm,
    }

    /// `crates/tako-control` から見た PWA のソース置き場。
    /// `canonicalize` はしない（#970。`..` を含んだままでも cargo / npm は正しく解決する）
    pub fn dir_from_manifest(manifest_dir: &Path) -> PathBuf {
        manifest_dir.join("..").join("..").join("web/tako-remote")
    }

    /// rust_embed が埋め込む先（`#[folder]` と同じ場所）
    pub fn dist_dir(pwa_dir: &Path) -> PathBuf {
        pwa_dir.join("dist")
    }

    /// 「PWA がビルド済みか」を決める 1 ファイル。
    /// `dist/` だけあって中身が無い（npm が途中で落ちた跡）を「済み」と誤判定しないため、
    /// ディレクトリの有無ではなくエントリポイントの有無で見る
    pub fn dist_index(pwa_dir: &Path) -> PathBuf {
        dist_dir(pwa_dir).join("index.html")
    }

    /// PATH から npm の実体を探す。
    ///
    /// `Command::new("npm")` に任せないのは Windows のため: PATH 上の実体は `npm.cmd` で、
    /// Rust の `Command` は PATH 検索で `.exe` しか補わない（`npm` は拡張子なしの
    /// shell script なので CreateProcess で起動できない）。
    pub fn find_npm(path_var: Option<&OsStr>) -> Option<PathBuf> {
        // 並びは PATH 検索と同じ「先に見つかったもの勝ち」
        let names: &[&str] = if cfg!(windows) {
            &["npm.cmd", "npm.exe", "npm.bat"]
        } else {
            &["npm"]
        };
        for dir in std::env::split_paths(path_var?) {
            if dir.as_os_str().is_empty() {
                continue;
            }
            for name in names {
                let candidate = dir.join(name);
                if is_executable(&candidate) {
                    return Some(candidate);
                }
            }
        }
        None
    }

    #[cfg(unix)]
    fn is_executable(path: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }

    #[cfg(not(unix))]
    fn is_executable(path: &Path) -> bool {
        path.is_file()
    }

    /// 何をするかを決める。**dist があれば npm を探しにも行かない**（npm 無しの環境で
    /// ビルド済みツリーが落ちない = 配布物のビルドと同じ前提）
    pub fn plan(pwa_dir: &Path, npm: Option<PathBuf>) -> Plan {
        if dist_index(pwa_dir).is_file() {
            return Plan::UpToDate;
        }
        match npm {
            // node_modules があるなら `npm run build` だけで済む（dist だけ消したツリー・
            // ネットワークの無い環境）。無ければ lockfile どおりに入れてからビルドする
            Some(npm) => Plan::Build {
                install_deps: !pwa_dir.join("node_modules").is_dir(),
                npm,
            },
            None => Plan::MissingNpm,
        }
    }

    /// PWA をビルドする。手順は `scripts/build-pwa.sh` / CI（#574）と同じ並び
    pub fn run(pwa_dir: &Path, npm: &Path, install_deps: bool) -> Result<(), String> {
        if install_deps {
            npm_step(pwa_dir, npm, &["ci", "--no-audit", "--no-fund"])?;
        }
        npm_step(pwa_dir, npm, &["run", "build"])
    }

    /// npm が成功したのに埋め込み元が揃っていない場合の案内（vite の出力先を変えた等）
    pub fn verify_built(pwa_dir: &Path) -> Result<(), String> {
        if dist_index(pwa_dir).is_file() {
            return Ok(());
        }
        Err(format!(
            "PWA をビルドしたのに {} が出来ていない: `scripts/build-pwa.sh` を実行して出力先を確かめること（#1309）\n  \
             rust_embed（crates/tako-control/src/remote.rs の PwaAssets）はこのファイルを含む dist/ を要求する。\n  \
             vite の出力先（web/tako-remote/vite.config.js の build.outDir）を変えたなら、rust_embed の #[folder] も対で直すこと。",
            dist_index(pwa_dir).display()
        ))
    }

    /// npm が無いときの案内。**1 行目だけで次の一手が分かる**ようにする
    pub fn missing_npm_message(pwa_dir: &Path) -> String {
        format!(
            "PWA が未ビルドで npm も見つからない: `scripts/build-pwa.sh` を実行してから cargo build をやり直すこと（#1309）\n  \
             要求元: rust_embed（crates/tako-control/src/remote.rs の PwaAssets）が {} をコンパイル時に読む。\n  \
             web/tako-remote/dist/ は .gitignore 対象なので、git worktree add 直後のツリーには存在しない。\n  \
             npm が入っていない環境では Node.js（CI と同じ v22 系）を入れるか、PWA をビルド済みの\n  \
             ツリーから web/tako-remote/dist/ をまるごとコピーすること。",
            dist_dir(pwa_dir).display()
        )
    }

    fn npm_step(pwa_dir: &Path, npm: &Path, args: &[&str]) -> Result<(), String> {
        // 出力は継承したまま流す（npm 自身のエラーが見えないと原因に辿り着けない）
        match Command::new(npm).args(args).current_dir(pwa_dir).status() {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(failed_message(
                pwa_dir,
                npm,
                args,
                &match status.code() {
                    Some(code) => format!("終了コード {code}"),
                    None => "シグナルで終了".to_string(),
                },
            )),
            Err(e) => Err(failed_message(pwa_dir, npm, args, &e.to_string())),
        }
    }

    fn failed_message(pwa_dir: &Path, npm: &Path, args: &[&str], reason: &str) -> String {
        format!(
            "PWA のビルドに失敗した: `scripts/build-pwa.sh` を実行して原因を確かめること（#1309）\n  \
             失敗したコマンド: {} {}（作業ディレクトリ {}）\n  \
             理由: {}\n  \
             npm の出力はこの上に出ている。ネットワークが無い環境では、PWA をビルド済みの\n  \
             ツリーから web/tako-remote/dist/ をコピーしても通る。",
            npm.display(),
            args.join(" "),
            pwa_dir.display(),
            reason
        )
    }
}
