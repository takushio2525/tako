//! build.rs（PWA の自動ビルド）の判定と実行を固定する（#1309）。
//!
//! **実 npm は起こさない**。PATH は一時ディレクトリ 1 つに差し替え、npm は
//! 「呼ばれた引数を書き出すだけ」の shim に置き換える。判定は build.rs 側の 1 実装を
//! `#[path]` でそのまま取り込むので、テストと本番で写しがずれない。
//! 書き先も一時ディレクトリだけ（本番の data dir / ホームには 1 バイトも書かない。#944）。

#[allow(dead_code)]
#[path = "../build.rs"]
mod build_script;

use build_script::pwa::{self, Plan};
use std::path::{Path, PathBuf};

/// 使い捨ての一時ツリー。名前は pid つき（同じ機で 2 本走っても取り合わない）で、
/// **落ちても Drop で消える**（テスト本体が作る作業 dir は自分で片付ける。#1296）
struct TempTree(PathBuf);

impl TempTree {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("tako-1309-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("一時ディレクトリを作れる");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// PWA の置き場を模した空のツリー（`<root>/web/tako-remote`）
    fn pwa_dir(&self) -> PathBuf {
        let dir = self.0.join("web/tako-remote");
        std::fs::create_dir_all(&dir).expect("PWA ディレクトリを作れる");
        dir
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_dist_index(pwa_dir: &Path) {
    let dist = pwa_dir.join("dist");
    std::fs::create_dir_all(&dist).expect("dist を作れる");
    std::fs::write(dist.join("index.html"), "<!doctype html>").expect("index.html を書ける");
}

/// 呼ばれた引数を 1 行ずつ `<shim_dir>/calls.log` へ書き出すだけの偽 npm を置く。
/// 戻り値はその shim が入った「PATH に載せるディレクトリ」
fn fake_npm(dir: &Path, exit_code: i32) -> PathBuf {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("shim ディレクトリを作れる");
    let log = dir.join("calls.log");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = bin.join("npm");
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit {exit_code}\n",
                log.display()
            ),
        )
        .expect("shim を書ける");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("shim に実行ビットを立てられる");
    }
    #[cfg(windows)]
    {
        // Windows の PATH 上の npm は npm.cmd（build.rs もそれを探す）
        let path = bin.join("npm.cmd");
        std::fs::write(
            &path,
            format!(
                "@echo off\r\n>>\"{}\" echo %*\r\nexit /b {exit_code}\r\n",
                log.display()
            ),
        )
        .expect("shim を書ける");
    }
    bin
}

fn shim_calls(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("calls.log"))
        .unwrap_or_default()
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

/// PATH に見立てた 1 ディレクトリから npm を解決する
fn npm_in(bin: &Path) -> Option<PathBuf> {
    pwa::find_npm(Some(bin.as_os_str()))
}

#[test]
fn dist_があれば何もしない() {
    let tree = TempTree::new("uptodate");
    let pwa_dir = tree.pwa_dir();
    write_dist_index(&pwa_dir);
    let bin = fake_npm(tree.path(), 0);

    assert_eq!(pwa::plan(&pwa_dir, npm_in(&bin)), Plan::UpToDate);
    // dist があるなら npm が無い環境（配布物のビルド機）でも落ちない
    assert_eq!(pwa::plan(&pwa_dir, None), Plan::UpToDate);
    // 判定だけで npm は 1 度も起きない
    assert!(shim_calls(tree.path()).is_empty());
}

#[test]
fn dist_が無ければ_npm_でビルドする() {
    let tree = TempTree::new("build");
    let pwa_dir = tree.pwa_dir();
    let bin = fake_npm(tree.path(), 0);

    // node_modules が無い = lockfile どおりに入れてからビルドする
    match pwa::plan(&pwa_dir, npm_in(&bin)) {
        Plan::Build { install_deps, npm } => {
            assert!(install_deps, "node_modules が無ければ npm ci が要る");
            assert_eq!(npm.parent(), Some(bin.as_path()));
        }
        other => panic!("dist が無ければビルドするはず: {other:?}"),
    }

    // node_modules があるならネットワークに触らず npm run build だけで済む
    std::fs::create_dir_all(pwa_dir.join("node_modules")).expect("node_modules を作れる");
    match pwa::plan(&pwa_dir, npm_in(&bin)) {
        Plan::Build { install_deps, .. } => {
            assert!(!install_deps, "node_modules があれば npm ci は要らない")
        }
        other => panic!("dist が無ければビルドするはず: {other:?}"),
    }
}

#[test]
fn dist_はあるが_index_htmlが無ければビルドする() {
    let tree = TempTree::new("empty-dist");
    let pwa_dir = tree.pwa_dir();
    // npm が途中で落ちた跡（ディレクトリだけある）を「ビルド済み」と誤判定しない
    std::fs::create_dir_all(pwa_dir.join("dist/assets")).expect("dist/assets を作れる");
    let bin = fake_npm(tree.path(), 0);

    assert!(matches!(
        pwa::plan(&pwa_dir, npm_in(&bin)),
        Plan::Build { .. }
    ));
}

#[test]
fn npm_が無ければ手順を案内して止める() {
    let tree = TempTree::new("no-npm");
    let pwa_dir = tree.pwa_dir();
    let empty_path = TempTree::new("no-npm-path");

    assert_eq!(
        pwa::plan(&pwa_dir, npm_in(empty_path.path())),
        Plan::MissingNpm
    );
    assert_eq!(pwa::find_npm(None), None, "PATH 自体が無くても落ちない");

    let message = pwa::missing_npm_message(&pwa_dir);
    let first_line = message.lines().next().unwrap_or_default();
    assert!(
        first_line.contains("scripts/build-pwa.sh"),
        "1 行目だけで次の一手が分かること: {first_line}"
    );
    assert!(first_line.contains("#1309"), "追える番号を残すこと");
}

#[test]
fn find_npm_は_path_の並び順で最初の実行可能ファイルを採る() {
    let tree = TempTree::new("find-npm");
    let first = tree.path().join("first");
    let second = tree.path().join("second");
    std::fs::create_dir_all(&first).expect("dir を作れる");
    let bin = fake_npm(&second, 0);
    // 1 つ目には「名前は同じだが実行できないもの」を置く（unix のみ実行ビットで判別できる）
    std::fs::write(
        first.join(if cfg!(windows) { "npm.txt" } else { "npm" }),
        "",
    )
    .expect("偽物を書ける");

    let path = std::env::join_paths([first.as_path(), bin.as_path()]).expect("PATH を組める");
    assert_eq!(
        pwa::find_npm(Some(path.as_os_str())),
        Some(bin.join(if cfg!(windows) { "npm.cmd" } else { "npm" })),
    );
}

#[test]
fn run_はnode_modulesの有無で_npm_ciを呼び分ける() {
    let tree = TempTree::new("run");
    let pwa_dir = tree.pwa_dir();
    let base = tree.path().to_path_buf();
    let bin = fake_npm(&base, 0);
    let npm = npm_in(&bin).expect("shim が見つかる");

    pwa::run(&pwa_dir, &npm, true).expect("shim は成功する");
    assert_eq!(
        shim_calls(&base),
        vec![
            "ci --no-audit --no-fund".to_string(),
            "run build".to_string()
        ],
        "node_modules が無いときは npm ci → npm run build の順"
    );

    let _ = std::fs::remove_file(base.join("calls.log"));
    pwa::run(&pwa_dir, &npm, false).expect("shim は成功する");
    assert_eq!(
        shim_calls(&base),
        vec!["run build".to_string()],
        "node_modules があるときは npm run build だけ"
    );
}

#[test]
fn npm_が非ゼロ終了なら手順つきのエラーで止まる() {
    let tree = TempTree::new("npm-fails");
    let pwa_dir = tree.pwa_dir();
    let base = tree.path().to_path_buf();
    let bin = fake_npm(&base, 3);
    let npm = npm_in(&bin).expect("shim が見つかる");

    let err = pwa::run(&pwa_dir, &npm, false).expect_err("非ゼロ終了は失敗として返る");
    let first_line = err.lines().next().unwrap_or_default();
    assert!(
        first_line.contains("scripts/build-pwa.sh") && first_line.contains("#1309"),
        "1 行目だけで次の一手が分かること: {first_line}"
    );
    assert!(
        err.contains("run build"),
        "失敗したコマンドが残ること: {err}"
    );
    assert!(err.contains("終了コード 3"), "理由が残ること: {err}");
}

#[test]
fn ビルド後に埋め込み元が揃っていなければ止める() {
    let tree = TempTree::new("verify");
    let pwa_dir = tree.pwa_dir();

    let err = pwa::verify_built(&pwa_dir).expect_err("index.html が無ければ失敗");
    let first_line = err.lines().next().unwrap_or_default();
    assert!(
        first_line.contains("scripts/build-pwa.sh") && first_line.contains("#1309"),
        "1 行目だけで次の一手が分かること: {first_line}"
    );

    write_dist_index(&pwa_dir);
    assert!(pwa::verify_built(&pwa_dir).is_ok());
}

#[test]
fn 埋め込み元のパスは_rust_embed_の_folder_と一致する() {
    // build.rs が張る rerun-if-changed / 判定の起点が remote.rs の #[folder] とずれると、
    // 「自動ビルドは走ったのに rust_embed は別の場所を見ている」が起こる
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let pwa_dir = pwa::dir_from_manifest(manifest);

    // dist が在るかに依らず比べたいので、実体ではなくパスの構成要素で見る
    assert_eq!(
        pwa::dist_dir(&pwa_dir),
        manifest.join("../../web/tako-remote/dist"),
        "remote.rs の #[folder] を変えたら build.rs も対で直すこと",
    );
    let source =
        std::fs::read_to_string(manifest.join("src/remote.rs")).expect("remote.rs を読める");
    assert!(
        source.contains("#[folder = \"../../web/tako-remote/dist/\"]"),
        "rust_embed の #[folder] が変わっている。build.rs の dir_from_manifest も対で直すこと",
    );
}

/// コメント行を除いた「実際に叩いている行」の行番号。
/// 手順の正本を呼ばなくなったのに、説明コメントだけが残っている形を見逃さないため
fn invocation_line(script: &str, needle: &str) -> Option<usize> {
    script
        .lines()
        .position(|line| line.contains(needle) && !line.trim_start().starts_with('#'))
}

#[test]
fn 配布物のビルドは_pwa_を_cargo_より先に作る() {
    // rust_embed はコンパイル時に dist/ を読むので、PWA ビルドは必ず cargo より前に来る。
    // build-app.sh がここを落とすと、リリース zip の PWA が stale になる（#60 / #1309）
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let build_app =
        std::fs::read_to_string(repo.join("scripts/build-app.sh")).expect("build-app.sh を読める");
    let pwa_at = invocation_line(&build_app, "scripts/build-pwa.sh")
        .expect("build-app.sh は PWA ビルドの正本を呼ぶこと");
    let cargo_at = invocation_line(&build_app, "cargo build --release")
        .expect("build-app.sh はリリースビルドを行う");
    assert!(
        pwa_at < cargo_at,
        "PWA のビルドは cargo build --release より前に置くこと（rust_embed が dist を要求する）"
    );
    // 手順の写しを持ち直していないこと（正本を呼ぶ形だけを許す）
    assert_eq!(
        invocation_line(&build_app, "npm "),
        None,
        "build-app.sh は npm を直接叩かず scripts/build-pwa.sh を呼ぶこと"
    );

    // Windows クロスチェックも同じ 1 実装を通す
    let cross = std::fs::read_to_string(repo.join("scripts/check-windows.sh"))
        .expect("check-windows.sh を読める");
    assert!(
        invocation_line(&cross, "scripts/build-pwa.sh --if-missing").is_some(),
        "check-windows.sh も PWA ビルドの正本を呼ぶこと"
    );
    assert_eq!(
        invocation_line(&cross, "npm "),
        None,
        "check-windows.sh は npm を直接叩かないこと"
    );

    // 正本のスクリプトが在って、そのまま叩ける
    let script = repo.join("scripts/build-pwa.sh");
    assert!(script.is_file(), "scripts/build-pwa.sh が無い");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = script
            .metadata()
            .expect("メタデータを読める")
            .permissions()
            .mode();
        assert!(mode & 0o111 != 0, "scripts/build-pwa.sh に実行ビットが無い");
    }
}
