//! Windows のアプリ内更新の腕が macOS の手段へ戻っていないことの番犬（#2434）
//!
//! #2434 の前は、Windows で更新を押すと `.exe` のインストーラーを `tako.zip` として保存し、
//! macOS 専用の展開コマンドへ渡して `/Applications/tako.app` を探していた（必ず失敗する）。
//! Windows の腕は `crates/tako-app/src/platform/installer_update.rs`（検査して無言で起こす）で、
//! `update_checker.rs` の `update_via_installer` と `restart_app` の Windows の枝がそこを通る。
//!
//! ここで見るのは 3 つ:
//! 1. Windows の腕（モジュール全体・`update_via_installer`・`restart_app` の Windows の枝）の
//!    コードに macOS の手段（展開コマンド・`.app`・バンドルの差し替え・`open -n`）が無い
//! 2. Windows の腕が検査（取得元・sha256）と無言の指定を通っている
//! 3. tako が渡す専用の指定（`/TAKOWAITPID` / `/TAKORELAUNCH`）の名前が、
//!    それを読む `installer/windows/tako.iss` の `{param:…}` と一致している
//!
//! 説明のコメントには macOS の手段の名前が書いてある（なぜ使わないかの説明）ので、
//! 不在の検査はコメントを落とした眺め（コードと文字列リテラル）で行う。

use std::path::{Path, PathBuf};

#[path = "common/code_view.rs"]
mod code_view;

const ARM: &str = "crates/tako-app/src/platform/installer_update.rs";
const CHECKER: &str = "crates/tako-app/src/update_checker.rs";
const ISS: &str = "installer/windows/tako.iss";

/// Windows の腕に現れてはいけない macOS の手段
const MAC_ONLY: [&str; 8] = [
    "ditto",
    ".app",
    "APP_INSTALL_PATH",
    "find_app_bundle",
    "update_via_zip",
    "replace_bundle_in_place",
    "open_new_instance",
    "app_bundle_path",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// `head` で始まる行から、次の行頭の item（`fn ` / `pub fn ` / `#[cfg`）の手前まで
fn item_body<'a>(src: &'a str, head: &str) -> &'a str {
    let start = src
        .find(head)
        .unwrap_or_else(|| panic!("{head} が見つからない（関数名を変えたら番犬も直す）"));
    let rest = &src[start + head.len()..];
    let end = ["\nfn ", "\npub fn ", "\n#[cfg", "\n/// ", "\n#[derive"]
        .iter()
        .filter_map(|m| rest.find(m))
        .min()
        .unwrap_or(rest.len());
    &src[start..start + head.len() + end]
}

/// `restart_app` の Windows の枝（`Restart::RunInstaller` の腕から関数の終わりまで）
fn restart_windows_arm(src: &str) -> &str {
    let body = item_body(src, "pub fn restart_app(");
    let at = body
        .find("Restart::RunInstaller(")
        .expect("restart_app に Windows の枝（Restart::RunInstaller）が無い");
    &body[at..]
}

/// Windows の腕の眺め（コメント抜き）に現れた macOS の手段
fn mac_tools_in(view: &str) -> Vec<&'static str> {
    MAC_ONLY
        .iter()
        .copied()
        .filter(|word| view.contains(word))
        .collect()
}

#[test]
fn windowsの腕にmacosの手段が無い() {
    let arm = code_view::without_comments_checked(&read(ARM), ARM);
    let found = mac_tools_in(&arm);
    assert!(
        found.is_empty(),
        "{ARM} に macOS の手段がある: {found:?}（#2434。Windows は zip の展開も .app の差し替えも\
         できない。インストーラーへ任せる）"
    );

    let checker = code_view::without_comments_checked(&read(CHECKER), CHECKER);
    let via_installer = item_body(&checker, "fn update_via_installer(");
    let found = mac_tools_in(via_installer);
    assert!(
        found.is_empty(),
        "update_via_installer に macOS の手段がある: {found:?}（#2434）"
    );
    let restart = restart_windows_arm(&checker);
    let found = mac_tools_in(restart);
    assert!(
        found.is_empty(),
        "restart_app の Windows の枝に macOS の手段がある: {found:?}（#2434）"
    );
}

#[test]
fn windowsの腕は検査と無言の指定を通る() {
    let checker = code_view::without_comments_checked(&read(CHECKER), CHECKER);
    let via_installer = item_body(&checker, "fn update_via_installer(");
    for must in [
        "installer_update::check_origin(",
        "installer_update::checkable(",
        "installer_update::download(",
    ] {
        assert!(
            via_installer.contains(must),
            "update_via_installer が {must} を通っていない（#2434。検査せずに起動しない）"
        );
    }
    assert!(
        restart_windows_arm(&checker).contains("installer_update::launch("),
        "restart_app の Windows の枝がインストーラーを起こしていない（#2434）"
    );

    let arm = code_view::without_comments_checked(&read(ARM), ARM);
    let args = item_body(&arm, "pub fn installer_args(");
    for flag in [
        "\"/SILENT\"",
        "\"/SUPPRESSMSGBOXES\"",
        "\"/NORESTART\"",
        "\"/CLOSEAPPLICATIONS\"",
        "\"/LOG\"",
        "WAIT_PID_PARAM",
        "RELAUNCH_PARAM",
    ] {
        assert!(
            args.contains(flag),
            "installer_args に {flag} が無い（#2434。無言で上書きして起動し直す指定）"
        );
    }
}

/// `pub const NAME: &str = "/VALUE";` の値（先頭の `/` を落とした名前）
fn param_name(src: &str, constant: &str) -> String {
    let head = format!("pub const {constant}: &str = \"/");
    let at = src
        .find(&head)
        .unwrap_or_else(|| panic!("{ARM} に {constant} が無い"));
    let rest = &src[at + head.len()..];
    rest[..rest.find('"').expect("閉じの引用符")].to_string()
}

#[test]
fn 専用の指定の名前がインストーラーと一致する() {
    let arm = code_view::without_comments_checked(&read(ARM), ARM);
    let iss = read(ISS);
    let wait = param_name(&arm, "WAIT_PID_PARAM");
    let relaunch = param_name(&arm, "RELAUNCH_PARAM");
    for (name, needle) in [
        (&wait, format!("ExpandConstant('{{param:{wait}|0}}')")),
        (
            &relaunch,
            format!("ExpandConstant('{{param:{relaunch}|}}')"),
        ),
    ] {
        assert!(
            iss.contains(&needle),
            "{ISS} が {name} を読んでいない（期待: {needle}）。tako が渡す名前とインストーラーが読む名前が\
             ずれると、待たずに上書きする / 起動し直さない（#2434）"
        );
    }
    // 待ちと起動し直しはイベント関数の中にあること（どこからも呼ばれない関数にしない）
    for event in [
        "function InitializeSetup(): Boolean;",
        "procedure DeinitializeSetup();",
    ] {
        assert!(iss.contains(event), "{ISS} に {event} が無い（#2434）");
    }
}

/// 番犬の検出力: Windows の腕へ macOS の展開を書き戻した写しを名指せること
#[test]
fn 番犬はwindowsの腕へ戻った展開コマンドを名指す() {
    let checker = code_view::without_comments_checked(&read(CHECKER), CHECKER);
    let head = "fn update_via_installer(";
    let at = checker.find(head).expect("update_via_installer");
    let injected = format!(
        "{}{head}info: &UpdateInfo) {{ std::process::Command::new(\"ditto\"); }}\n{}",
        &checker[..at],
        &checker[at + head.len()..]
    );
    let found = mac_tools_in(item_body(&injected, head));
    assert_eq!(found, vec!["ditto"], "注入した ditto を拾えていない");
}
