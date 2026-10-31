//! **#2592 の番犬**: `SHELL` の名前が `sh` のとき、setup の PATH 通しが `sh -l` の読まない `~/.bash_profile` へ
//! 書く形へ戻らない。
//!
//! ## 何が起きていたのか
//!
//! setup の書き先は `ShellKind::from_shell_path`（`$SHELL` の名前）と `ShellKind::login_profile_rel` の表で決まる。
//! #2592 より前は名前 `sh` を bash と見なして `~/.bash_profile` へ書いていた。ところが bash は argv[0] の名前が
//! `sh` だと sh モード（POSIX モード）で起き、ログインでは `/etc/profile` と `~/.profile` **だけ**を読む
//! （macOS の `/bin/sh` は bash 3.2 の sh モード。dash・sh の名前で起こした zsh も `~/.profile`。2026-11-01 実測）。
//! ログインシェル経由の解決（`exe::user_shell` = `$SHELL -l -c …`）も利用者の端末のログインシェルもこの形で
//! 起きるので、書いた後も新しいターミナルの PATH に置き場所が見えず、`verified=false`・初回起動の導線
//! （welcome）の段・`tako setup bootstrap status` の `next_step` が `path` のまま残った（修正前の実測。
//! 砂場の HOME・`SHELL=/bin/sh` の CLI）。しかも `.bash_profile` が無かった人には、tako が作った
//! `.bash_profile` が以後の `bash -l` で `.profile` を覆い隠す。
//!
//! いまは名前 `sh` を `ShellKind::Sh`（`~/.profile`。行の形は bash と同じ POSIX の `case` / `export`）とし、
//! `undo-path` は書き先を移す前の `~/.bash_profile` に残った tako のブロックも外す。A/B は `TAKO_2592_LEGACY=1`。
//! 外れたら file:line で名指す（直す場所が分からない番犬は直されない）。
//!
//! ## ここで止めるもの
//!
//! 1. [`書き先の表と本物のログインシェルが読むファイルが一致する`] — 実測（unix）。在るシェル（`/bin/sh`・
//!    `/bin/bash`・`/bin/zsh`・fish と、sh の名前で起こす dash / bash / zsh・bash の名前で起こす `/bin/sh`）を
//!    砂場の HOME で、本番の問い合わせと同じ起こし方（`exe::login_invocation`）で起こし、読んだ profile の中に
//!    `login_profile_rel` の答えが入っているかを見る
//! 2. [`shで書いた後に段がpathから進む`] — 実経路（unix）。`SHELL=/bin/sh`（と sh の名前の dash / bash）で
//!    `ensure_path_for` の後に `verified=true`・status と welcome の段が `path` から進む。`.profile` /
//!    `.bash_profile` が先に在っても利用者の行を壊さず、`.bash_profile` は触らない。同じテストの中で
//!    `TAKO_2592_LEGACY=1` は `.bash_profile` へ書いて段が `path` のまま残ることも見る（A/B）
//! 3. [`shのundo_pathは書き先を移す前のブロックも外す`] — 実経路（unix）。修正前に置かれた `.bash_profile` の
//!    ブロックと `.profile` のブロックの両方を元のバイト列へ戻す。bash の undo は `.profile` に触らない
//!
//! **本物の環境を書き換えない**: `HOME` / `TAKO_DATA_DIR` / `TAKO_ORCHESTRATOR_DIR` / `CLAUDE_CONFIG_DIR` は
//! 使い捨ての器（`ScratchDir`）の下、プロセスの `PATH` は空の dir（`TMPDIR` は器の後始末が一時の置き場の下かを
//! 確かめるので変えない）。偽の claude は認証の問い合わせに「未ログイン」と答えるだけ。
//! env はプロセス全体の状態なので、2 / 3 は `run_alone!` の子で走らせる（#2400）。1 は子プロセスへ env を渡すだけ
//! unix だけ（Windows の PATH 通しはユーザー環境変数へ書くので `SHELL` を読まない）
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tako_control::setup_bootstrap;
use tako_core::platform::agent_install::AgentKind;
use tako_core::shell_profile::ShellKind;

#[path = "common/production_range.rs"]
mod production_range;

const SHELL_PROFILE: &str = "crates/tako-core/src/shell_profile.rs";
const BOOTSTRAP: &str = "crates/tako-control/src/setup_bootstrap.rs";

/// 旧挙動へ戻す A/B の env
const LEGACY: &str = "TAKO_2592_LEGACY";

/// tako の PATH ブロックの先頭行
const BLOCK_BEGIN: &str = "# >>> tako PATH >>>";

/// 偽の claude（PATH の外 = `~/.local/bin`）: 認証の問い合わせに「未ログイン」と答えるだけ
const FAKE_CLAUDE: &str = "#!/bin/sh\ncase \"$1 $2\" in\n  \"auth status\") echo '{\"loggedIn\":false}'; exit 0;;\nesac\n[ \"$1\" = \"--version\" ] && echo \"2.1.258 (Claude Code)\"\nexit 0\n";

/// 利用者が先に書いていた行（書き換えの前後でこの行が残るかを見る）
const USER_LINE: &str = "# user's own line (#2592)\nexport TAKO_2592_USER_LINE=1\n";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn production(rel: &str) -> String {
    let path = repo_root().join(rel);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
    production_range::production(&src, rel)
}

/// `needle` が最初に出る行を `path:line` で返す（失敗の文で直す場所を名指す）
fn locate(rel: &str, needle: &str) -> String {
    let src = production(rel);
    match src.lines().position(|line| line.contains(needle)) {
        Some(i) => format!("{rel}:{}", i + 1),
        None => format!("{rel}（{needle} が見つからない）"),
    }
}

/// 書き先の表の場所（名前 → 種別と、種別 → profile）
fn table_at() -> String {
    format!(
        "名前の判定は {}、書き先は {}",
        locate(SHELL_PROFILE, "pub fn from_shell_path("),
        locate(SHELL_PROFILE, "pub fn login_profile_rel(")
    )
}

fn executable(at: &Path, body: &str) {
    std::fs::write(at, body).expect("偽物を書く");
    std::fs::set_permissions(at, std::fs::Permissions::from_mode(0o755)).expect("実行ビット");
}

/// `target` を `name` の名前で指す symlink を `dir` に置く（argv[0] の名前で読むファイルが変わるシェルの検証用）
fn alias(dir: &Path, name: &str, target: &str) -> String {
    std::fs::create_dir_all(dir).expect("別名の置き場");
    let at = dir.join(name);
    std::os::unix::fs::symlink(target, &at).expect("symlink");
    at.display().to_string()
}

/// 1: 実測。書き先の表の答えを、本物のログインシェルが読む
#[test]
fn 書き先の表と本物のログインシェルが読むファイルが一致する() {
    let scratch = tako_core::test_residue::ScratchDir::new("2592-table");
    let root = scratch.path().canonicalize().expect("正規化");
    // 表に在る profile を全部置き、読まれたら名前を出す（fish の config.fish も `echo` で通る）
    let rels = [
        ShellKind::Sh,
        ShellKind::Bash,
        ShellKind::Zsh,
        ShellKind::Fish,
    ]
    .map(ShellKind::login_profile_rel);
    let mut shells: Vec<(String, String)> = Vec::new();
    for path in [
        "/bin/sh",
        "/bin/bash",
        "/bin/zsh",
        "/usr/bin/zsh",
        "/opt/homebrew/bin/fish",
        "/usr/local/bin/fish",
        "/usr/bin/fish",
    ] {
        if Path::new(path).exists() {
            shells.push((path.to_string(), path.to_string()));
        }
    }
    // argv[0] の名前で読むファイルが変わる形（`$SHELL` は symlink のパスのまま起こされる）
    for (label, name, target) in [
        ("sh→dash", "sh", "/bin/dash"),
        ("sh→bash", "sh", "/bin/bash"),
        ("sh→zsh", "sh", "/bin/zsh"),
        ("bash→/bin/sh", "bash", "/bin/sh"),
    ] {
        if Path::new(target).exists() {
            let dir = root.join("alias").join(label.replace(['→', '/'], "_"));
            shells.push((label.to_string(), alias(&dir, name, target)));
        }
    }
    assert!(
        shells.iter().any(|(label, _)| label == "/bin/sh"),
        "/bin/sh が無い（unix の前提が崩れた）"
    );
    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0;
    for (i, (label, shell)) in shells.iter().enumerate() {
        let Some(kind) = ShellKind::from_shell_path(shell) else {
            failures.push(format!("{label}: 種別を判定できない（{}）", table_at()));
            continue;
        };
        let home = root.join("homes").join(i.to_string());
        for rel in rels {
            let at = home.join(rel);
            std::fs::create_dir_all(at.parent().expect("親")).expect("一時のホーム");
            std::fs::write(&at, format!("echo READ:{rel}\n")).expect("印の profile");
        }
        let plan = tako_core::platform::exe::login_invocation(shell, ":");
        let mut command = std::process::Command::new(&plan.program);
        if let Some(arg0) = &plan.arg0 {
            std::os::unix::process::CommandExt::arg0(&mut command, arg0);
        }
        let out = command
            .args(&plan.args)
            .env_clear()
            .env("HOME", &home)
            .env("PATH", "/usr/bin:/bin")
            .env("SHELL", shell)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap_or_else(|e| panic!("{label} を起こせない: {e}"));
        let stdout = String::from_utf8_lossy(&out.stdout);
        let read: Vec<&str> = stdout
            .lines()
            .filter_map(|line| line.strip_prefix("READ:"))
            .collect();
        let want = kind.login_profile_rel();
        println!(
            "{label}: 種別 {} → 書き先 ~/{want} / ログインシェルが読んだ {read:?}",
            kind.as_str()
        );
        checked += 1;
        if !read.contains(&want) {
            failures.push(format!(
                "{label}: setup は ~/{want} へ書く（種別 {}）が、`{shell} -l` が読むのは {read:?} = 書いた PATH が \
                 新しいターミナルに見えず、段が path のまま残る（#2592。{}）",
                kind.as_str(),
                table_at()
            ));
        }
    }
    println!("書き先とログインシェルの読み先を突き合わせた: {checked} 本");
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// 使い捨ての器（スコープで消える = panic でも残さない。#1312 / #2561）の下に砂場を作り、
/// 本番の設定・データを読み書きしないよう env を向ける。2 つ目は器の正規化したパス
/// （macOS の一時の置き場は /var → /private/var の symlink なので、ログインシェルが返す PATH と比べられる形）
fn sandbox(tag: &str) -> (tako_core::test_residue::ScratchDir, PathBuf) {
    // env を書き換えるので `run_alone!` の子の中だけで呼ぶ（#2400）
    tako_core::test_child::assert_alone("sandbox");
    let scratch = tako_core::test_residue::ScratchDir::new(&format!("2592-{tag}"));
    let root = scratch.path().canonicalize().expect("正規化");
    for dir in ["homes", "data", "orch", "ccd", "empty-path"] {
        std::fs::create_dir_all(root.join(dir)).expect("一時の dir");
    }
    std::env::set_var("TAKO_DATA_DIR", root.join("data"));
    std::env::set_var("TAKO_ORCHESTRATOR_DIR", root.join("orch"));
    std::env::set_var("CLAUDE_CONFIG_DIR", root.join("ccd"));
    std::env::remove_var("CODEX_HOME");
    std::env::remove_var("TAKO_SELF_TEST");
    std::env::remove_var("ZDOTDIR");
    std::env::remove_var("ENV");
    std::env::remove_var(LEGACY);
    std::env::set_var("PATH", root.join("empty-path"));
    (scratch, root)
}

/// 偽の claude を `~/.local/bin` に置いた新しいホームへ切り替える
fn fresh_home(root: &Path, label: &str) -> PathBuf {
    // env（HOME）を書き換えるので `run_alone!` の子の中だけで呼ぶ（#2400）
    tako_core::test_child::assert_alone("fresh_home");
    let home = root.join("homes").join(label);
    std::fs::create_dir_all(home.join(".local/bin")).expect("一時のホーム");
    executable(&home.join(".local/bin/claude"), FAKE_CLAUDE);
    std::env::set_var("HOME", &home);
    home
}

/// `SHELL` と A/B の env を切り替える
fn set_shell(shell: &str, legacy: bool) {
    tako_core::test_child::assert_alone("set_shell");
    std::env::set_var("SHELL", shell);
    if legacy {
        std::env::set_var(LEGACY, "1");
    } else {
        std::env::remove_var(LEGACY);
    }
}

/// ホームのファイルの中身（無ければ `None`）
fn read(home: &Path, rel: &str) -> Option<String> {
    std::fs::read_to_string(home.join(rel)).ok()
}

/// 確かめは claude を引けたら認証の問い合わせで起こすので、砂場のログインシェルから本物の
/// エージェント CLI を引けないことを先に確かめる（引けたら何も叩かずに止まる）
fn assert_no_real_agents(label: &str) {
    let real: Vec<&str> = ["claude", "codex", "agy"]
        .into_iter()
        .filter(|name| tako_core::platform::exe::find(name).is_some())
        .collect();
    assert!(
        real.is_empty(),
        "{label}: 砂場のログインシェルから {real:?} を引けてしまう（確かめが本物を起こしうる）。先へ進まない"
    );
}

/// 2: 実経路。`SHELL` の名前が sh なら、setup が書いた profile をログインシェルの問い合わせが読む
#[test]
fn shで書いた後に段がpathから進む() {
    use tako_control::setup_bootstrap::Step;
    use tako_control::welcome;
    if tako_core::run_alone!(shで書いた後に段がpathから進む) {
        return;
    }
    let (_scratch, root) = sandbox("path");
    // (ラベル, SHELL の値, 先に在るファイル, A/B の旧挙動か)
    let mut cases: Vec<(String, String, &[&str], bool)> = vec![
        ("sh".into(), "/bin/sh".into(), &[], false),
        ("sh+profile".into(), "/bin/sh".into(), &[".profile"], false),
        (
            "sh+bash_profile".into(),
            "/bin/sh".into(),
            &[".bash_profile"],
            false,
        ),
        ("sh-legacy".into(), "/bin/sh".into(), &[], true),
    ];
    for (label, target) in [("sh→dash", "/bin/dash"), ("sh→bash", "/bin/bash")] {
        if Path::new(target).exists() {
            let dir = root.join("alias").join(label.replace('→', "_"));
            cases.push((label.into(), alias(&dir, "sh", target), &[], false));
        }
    }
    let mut failures: Vec<String> = Vec::new();
    let mut advanced = 0;
    let mut want_advanced = 0;
    for (label, shell, existing, legacy) in &cases {
        let home = fresh_home(&root, &label.replace('→', "_"));
        for rel in existing.iter() {
            std::fs::write(home.join(rel), USER_LINE).expect("利用者の profile");
        }
        set_shell(shell, *legacy);
        assert_no_real_agents(label);
        let before = setup_bootstrap::status_for(AgentKind::Claude).expect("status");
        if before.step != Step::Path {
            failures.push(format!(
                "{label}: 書く前の段が path でない（砂場の前提が崩れた）: {:?}",
                before.step
            ));
            continue;
        }
        let path = setup_bootstrap::ensure_path_for(AgentKind::Claude);
        let after = setup_bootstrap::status_for(AgentKind::Claude).expect("status");
        let step = welcome::probe();
        let summary = path.as_ref().map_or_else(
            |e| format!("断り {e}"),
            |v| {
                format!(
                    "shell={} wrote={} verified={}",
                    v["shell"], v["wrote"], v["verified"]
                )
            },
        );
        println!(
            "{label}: path {summary} / status の段={:?} / welcome の段={} / 先に在った {existing:?}",
            after.step,
            step.kind()
        );
        let seen = matches!(&path, Ok(v) if v["verified"] == true)
            && after.step != Step::Path
            && step.kind() != "path";
        if *legacy {
            // A/B: 旧挙動では .bash_profile へ書き、書いた後も段が path のまま（= この番犬が修正前を見分けられる）
            let wrote_bash_profile = matches!(&path, Ok(v) if v["shell"] == "bash")
                && read(&home, ".bash_profile").is_some_and(|t| t.contains(BLOCK_BEGIN));
            if !wrote_bash_profile || seen {
                failures.push(format!(
                    "{label}: {LEGACY}=1 で旧挙動（.bash_profile へ書き、段が path のまま）にならない \
                     = A/B が効いていない（path {summary} / status の段={:?}。{}）",
                    after.step,
                    table_at()
                ));
            }
            continue;
        }
        want_advanced += 1;
        let profile = read(&home, ".profile").unwrap_or_default();
        let wrote_profile = matches!(&path, Ok(v) if v["shell"] == "sh" && v["wrote"] == true)
            && profile.contains(BLOCK_BEGIN)
            && profile.contains("$HOME/.local/bin");
        if !wrote_profile {
            failures.push(format!(
                "{label}: setup が ~/.profile（sh -l が読む唯一の profile）へ書いていない: path {summary}（{}）",
                table_at()
            ));
            continue;
        }
        if existing.contains(&".profile") && !profile.starts_with(USER_LINE) {
            failures.push(format!(
                "{label}: 先に在った ~/.profile の利用者の行が先頭に残っていない: {profile:?}"
            ));
        }
        // .bash_profile は書かない（無ければ作らない = 以後の bash -l で .profile を覆い隠さない）
        let bash_profile = read(&home, ".bash_profile");
        let want_bash_profile = existing
            .contains(&".bash_profile")
            .then(|| USER_LINE.to_string());
        if bash_profile != want_bash_profile {
            failures.push(format!(
                "{label}: ~/.bash_profile を書き換えた（{want_bash_profile:?} → {bash_profile:?}）。\
                 sh のログインは読まず、無かった人には bash -l で ~/.profile を覆い隠す（{}）",
                table_at()
            ));
        }
        if seen {
            advanced += 1;
        } else {
            failures.push(format!(
                "{label}: setup が ~/.profile へ書いた後も段が進まない（path {summary} / status の段={:?} / \
                 welcome の段={}）= 書き先とログインシェルの問い合わせが食い違っている（#2592。{}・判定は {}）",
                after.step,
                step.kind(),
                table_at(),
                locate(BOOTSTRAP, "fn shell_kind_for(")
            ));
        }
    }
    std::env::remove_var(LEGACY);
    println!("SHELL の名前が sh で書いた後に段が進んだ: {advanced} / {want_advanced}");
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// 3: 実経路。sh の undo-path は書き先を移す前（`.bash_profile`）に残ったブロックも外す
#[test]
fn shのundo_pathは書き先を移す前のブロックも外す() {
    if tako_core::run_alone!(shのundo_pathは書き先を移す前のブロックも外す) {
        return;
    }
    let (_scratch, root) = sandbox("undo");
    let mut failures: Vec<String> = Vec::new();
    let undo_at = || {
        format!(
            "{} → {}",
            locate(BOOTSTRAP, "pub fn undo_path_for_typed("),
            locate(SHELL_PROFILE, "pub fn previous_profile_rel(")
        )
    };

    // ---- sh: 修正前に .bash_profile へ置かれたブロック + いまの .profile のブロック ----
    let home = fresh_home(&root, "sh");
    std::fs::write(home.join(".bash_profile"), USER_LINE).expect("利用者の .bash_profile");
    std::fs::write(home.join(".profile"), USER_LINE).expect("利用者の .profile");
    // 修正前の書き先と同じブロックを置く（修正前の setup が書いたものと同じ関数・同じ種別）
    let dirs = [home.join(".local/bin")];
    tako_core::shell_profile::ensure_dirs_on_path_in(&home, ShellKind::Bash, &dirs, None)
        .expect("修正前の形のブロック");
    set_shell("/bin/sh", false);
    assert_no_real_agents("sh");
    if let Err(e) = setup_bootstrap::ensure_path_for(AgentKind::Claude) {
        failures.push(format!("sh: path が断った: {e}"));
    }
    for rel in [".profile", ".bash_profile"] {
        if !read(&home, rel).is_some_and(|t| t.contains(BLOCK_BEGIN)) {
            failures.push(format!(
                "sh: undo の前に ~/{rel} にブロックが無い（前提が崩れた）"
            ));
        }
    }
    let undo = setup_bootstrap::undo_path_for(AgentKind::Claude);
    println!("sh: undo-path {undo:?}");
    for rel in [".profile", ".bash_profile"] {
        let now = read(&home, rel);
        if now.as_deref() != Some(USER_LINE) {
            failures.push(format!(
                "sh: undo-path の後の ~/{rel} が元のバイト列に戻らない: {now:?}（{}）",
                undo_at()
            ));
        }
    }
    match &undo {
        Ok(v) if v["shell"] == "sh" && v["change"] == "removed" => {
            if v["previous_profile"]["change"] != "removed" {
                failures.push(format!(
                    "sh: undo-path の応答に書き先を移す前のブロックを外した記録が無い: {v}（{}）",
                    undo_at()
                ));
            }
        }
        other => failures.push(format!(
            "sh: undo-path の応答が違う: {other:?}（{}）",
            undo_at()
        )),
    }

    // ---- bash（既知のシェル）: .bash_profile だけを外し、.profile のブロックには触らない ----
    let home = fresh_home(&root, "bash");
    let dirs = [home.join(".local/bin")];
    tako_core::shell_profile::ensure_dirs_on_path_in(&home, ShellKind::Sh, &dirs, None)
        .expect(".profile のブロック");
    let profile_before = read(&home, ".profile");
    set_shell("/bin/bash", false);
    if let Err(e) = setup_bootstrap::ensure_path_for(AgentKind::Claude) {
        failures.push(format!("bash: path が断った: {e}"));
    }
    let undo = setup_bootstrap::undo_path_for(AgentKind::Claude);
    println!("bash: undo-path {undo:?}");
    if read(&home, ".profile") != profile_before {
        failures.push(format!(
            "bash: undo-path が ~/.profile に触った（bash の書き先は ~/.bash_profile だけ。{}）",
            undo_at()
        ));
    }
    if read(&home, ".bash_profile").is_some_and(|t| t.contains(BLOCK_BEGIN)) {
        failures.push("bash: undo-path の後も ~/.bash_profile にブロックが残る".to_string());
    }
    if matches!(&undo, Ok(v) if v.get("previous_profile").is_some()) {
        failures.push(format!(
            "bash: undo-path の応答に previous_profile が増えた（既知のシェルの応答は修正前と同じ形。{}）",
            undo_at()
        ));
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
