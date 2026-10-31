//! **#2603 の番犬**: 名前が `dash` / `ksh` のシェルを、種別の分からないシェル（断りで手作業）へ戻さない。
//! 確かめていない POSIX 系の名前を寄せない。
//!
//! ## 何が起きていたのか
//!
//! #2592 で名前 `sh` は `ShellKind::Sh`（`~/.profile`）になったが、名前自体が `dash` / `ksh` のときは
//! 「種別の分からないシェル」のままだった（main `4caf67f` の砂場で実測: status は `shell=null`・`next_step=path`、
//! `tako setup bootstrap path` は「使っているシェル（/bin/dash）の設定ファイルが分かりません」の断り、
//! `--check` と handoff も手作業の依頼）。本物の `/bin/dash` と `/bin/ksh`（ksh93u+）は、`-l -c`・argv[0] の
//! `-<名>`・対話ログインのどれでも `~/.profile` を読む（一時 HOME の全 profile に印を置いて実測）。
//!
//! いまは `shell_profile::SH_PROFILE_SHELLS`（dash / ksh）を `ShellKind::Sh` へ寄せる。A/B は `TAKO_2603_LEGACY=1`。
//! 外れたら file:line で名指す。
//!
//! ## ここで止めるもの
//!
//! 1. [`寄せた名前は本物のログインでprofileを読み確かめていない名前は寄せない`] — 実測と表。
//!    寄せた名前の本物（在るもの）を、本番の問い合わせと同じ起こし方（`exe::login_invocation`）で起こすと
//!    書き先 `~/.profile` を読む。起こし方の表も `-l -c`（実測した形）のまま。確かめていない POSIX 系の名前
//!    （mksh ほか）は種別の分からないシェルのまま。寄せる名前の表は番犬の [`VERIFIED`] と同じ
//! 2. [`dashとkshはpathの後に段が進み本物のログインでpathが見える`] — 実経路。`SHELL=/bin/dash` / `/bin/ksh` で
//!    path の後に status と welcome の段が `path` から進み、本物のそのシェルのログインから `~/.local/bin` の
//!    claude を引ける。undo-path の後のファイルは書く前と同じ。同じテストの中で `TAKO_2603_LEGACY=1` は
//!    断りで止まりファイルを書かないことも見る（A/B）
//!
//! **本物の環境を書き換えない**: `HOME` / `TAKO_DATA_DIR` / `TAKO_ORCHESTRATOR_DIR` / `CLAUDE_CONFIG_DIR` は
//! 使い捨ての器（`ScratchDir`）の下、プロセスの `PATH` は空の dir（`TMPDIR` は器の後始末が一時の置き場の下かを
//! 確かめるので変えない）。偽の claude は認証の問い合わせに「未ログイン」と答えるだけ。
//! env はプロセス全体の状態なので、2 は `run_alone!` の子で走らせる（#2400）。1 は子プロセスへ env を渡すだけ。
//! unix だけ（Windows の PATH 通しはユーザー環境変数へ書くので `SHELL` を読まない）
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tako_control::setup_bootstrap;
use tako_core::platform::agent_install::AgentKind;
use tako_core::platform::exe::{login_form, login_invocation, LoginForm};
use tako_core::shell_profile::{ShellKind, SH_PROFILE_SHELLS};

#[path = "common/production_range.rs"]
mod production_range;

const SHELL_PROFILE: &str = "crates/tako-core/src/shell_profile.rs";
const EXE: &str = "crates/tako-core/src/platform/exe.rs";

/// 旧挙動へ戻す A/B の env
const LEGACY: &str = "TAKO_2603_LEGACY";

/// 本物をログインで起こして `.profile` を読むと確かめた名前（#2603 の段 1。macOS 同梱の dash と ksh93u+）。
/// `SH_PROFILE_SHELLS` へ名前を足すなら、本物で確かめてからここへも足す
const VERIFIED: [&str; 2] = ["dash", "ksh"];

/// 手元で確かめられなかった POSIX 系の名前（寄せない = 種別の分からないシェルのまま）
const NOT_VERIFIED: [&str; 8] = [
    "mksh", "pdksh", "oksh", "yash", "posh", "ash", "ksh93", "busybox",
];

/// 本物を探す置き場
const BIN_DIRS: [&str; 5] = [
    "/bin",
    "/usr/bin",
    "/usr/local/bin",
    "/opt/homebrew/bin",
    "/usr/pkg/bin",
];

/// 偽の claude（PATH の外 = `~/.local/bin`）: 認証の問い合わせに「未ログイン」と答えるだけ
const FAKE_CLAUDE: &str = "#!/bin/sh\ncase \"$1 $2\" in\n  \"auth status\") echo '{\"loggedIn\":false}'; exit 0;;\nesac\n[ \"$1\" = \"--version\" ] && echo \"2.1.258 (Claude Code)\"\nexit 0\n";

/// 利用者が先に書いていた `.profile`（書き換えの前後でこの行が残るかを見る）
const USER_LINE: &str = "# user's own .profile (#2603)\nexport TAKO_2603_USER_LINE=1\n";

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

/// 寄せ方の表の場所
fn table_at() -> String {
    format!(
        "寄せる名前は {}、判定は {}",
        locate(SHELL_PROFILE, "pub const SH_PROFILE_SHELLS"),
        locate(SHELL_PROFILE, "pub fn from_shell_path(")
    )
}

/// 手元に在る本物（`name` の実行ファイル）
fn real_shell(name: &str) -> Option<String> {
    BIN_DIRS
        .iter()
        .map(|dir| Path::new(dir).join(name))
        .find(|path| path.is_file())
        .map(|path| path.display().to_string())
}

fn executable(at: &Path, body: &str) {
    std::fs::write(at, body).expect("偽物を書く");
    std::fs::set_permissions(at, std::fs::Permissions::from_mode(0o755)).expect("実行ビット");
}

/// 本番の問い合わせと同じ起こし方（`exe::login_invocation`）で `shell` をログインで起こし、標準出力を返す
fn login_run(shell: &str, home: &Path, script: &str) -> String {
    let plan = login_invocation(shell, script);
    let mut command = std::process::Command::new(&plan.program);
    if let Some(arg0) = &plan.arg0 {
        std::os::unix::process::CommandExt::arg0(&mut command, arg0);
    }
    let out = command
        .args(&plan.args)
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env("SHELL", shell)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("{shell} を起こせない: {e}"));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// 1: 実測と表。寄せた名前の本物はログインで `.profile` を読み、確かめていない名前は寄せない
#[test]
fn 寄せた名前は本物のログインでprofileを読み確かめていない名前は寄せない() {
    let scratch = tako_core::test_residue::ScratchDir::new("2603-table");
    let root = scratch.path().canonicalize().expect("正規化");
    let mut failures: Vec<String> = Vec::new();
    if SH_PROFILE_SHELLS[..] != VERIFIED[..] {
        failures.push(format!(
            "寄せる名前 {SH_PROFILE_SHELLS:?} が本物で確かめた名前 {VERIFIED:?} と違う。足すなら本物をログインで起こして \
             `.profile` を読むことを確かめ、この番犬の VERIFIED にも足すこと（#2603。{}）",
            table_at()
        ));
    }
    let mut measured = 0;
    for name in VERIFIED {
        if ShellKind::from_shell_path(name) != Some(ShellKind::Sh) {
            failures.push(format!(
                "{name}: sh と同じ `.profile` へ寄っていない = 種別の分からないシェルの断りで手作業へ戻る（#2603。{}）",
                table_at()
            ));
        }
        let Some(real) = real_shell(name) else {
            println!("{name}: 手元に無いので本物の読み方は見ない（{BIN_DIRS:?}）");
            continue;
        };
        if login_form(&real) != LoginForm::Flag {
            failures.push(format!(
                "{real}: 問い合わせの起こし方が `-l -c` でない（実測したのは `-l -c` の形。{}）",
                locate(EXE, "pub fn login_form(")
            ));
        }
        // 本物と、別の置き場の同じ名前の symlink（`$SHELL` は symlink のパスのまま起こされる）
        let alias_dir = root.join("alias").join(name);
        std::fs::create_dir_all(&alias_dir).expect("別名の置き場");
        let alias = alias_dir.join(name);
        std::os::unix::fs::symlink(&real, &alias).expect("symlink");
        for (label, shell) in [
            ("本物", real.clone()),
            ("symlink", alias.display().to_string()),
        ] {
            let home = root.join("homes").join(format!("{name}-{label}"));
            for rel in [".profile", ".bash_profile", ".zprofile", ".kshrc", ".shrc"] {
                let at = home.join(rel);
                std::fs::create_dir_all(at.parent().expect("親")).expect("一時のホーム");
                std::fs::write(&at, format!("echo READ:{rel}\n")).expect("印の profile");
            }
            let out = login_run(&shell, &home, ":");
            let read: Vec<&str> = out
                .lines()
                .filter_map(|line| line.strip_prefix("READ:"))
                .collect();
            let want = ShellKind::Sh.login_profile_in(&home);
            let want_rel = want
                .strip_prefix(&home)
                .expect("ホームの下")
                .to_string_lossy()
                .into_owned();
            println!("{name}（{label}）: ログインで読んだ {read:?} / tako の書き先 ~/{want_rel}");
            measured += 1;
            if !read.contains(&want_rel.as_str()) {
                failures.push(format!(
                    "{name}（{label}）: tako は ~/{want_rel} へ書くが、`{shell}` のログインが読むのは {read:?} \
                     = 書いた PATH が新しいターミナルに見えない（#2603。{}）",
                    table_at()
                ));
            }
        }
    }
    for name in NOT_VERIFIED {
        if ShellKind::from_shell_path(&format!("/usr/bin/{name}")).is_some() {
            failures.push(format!(
                "{name}: 本物で確かめていないのに既知のシェルへ寄せた（読まない profile へ書いて成功を返しうる。{}）",
                table_at()
            ));
        }
    }
    println!("本物のログインで書き先を読むと確かめた: {measured} 本");
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// 使い捨ての器の下に砂場を作り、本番の設定・データを読み書きしないよう env を向ける
fn sandbox(tag: &str) -> (tako_core::test_residue::ScratchDir, PathBuf) {
    // env を書き換えるので `run_alone!` の子の中だけで呼ぶ（#2400）
    tako_core::test_child::assert_alone("sandbox");
    let scratch = tako_core::test_residue::ScratchDir::new(&format!("2603-{tag}"));
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

/// 偽の claude を `~/.local/bin` に置いた新しいホームへ切り替える（`with_profile` なら利用者の `.profile` も置く）
fn fresh_home(root: &Path, label: &str, with_profile: bool) -> PathBuf {
    // env（HOME）を書き換えるので `run_alone!` の子の中だけで呼ぶ（#2400）
    tako_core::test_child::assert_alone("fresh_home");
    let home = root.join("homes").join(label);
    std::fs::create_dir_all(home.join(".local/bin")).expect("一時のホーム");
    executable(&home.join(".local/bin/claude"), FAKE_CLAUDE);
    if with_profile {
        std::fs::write(home.join(".profile"), USER_LINE).expect("利用者の .profile");
    }
    std::env::set_var("HOME", &home);
    home
}

fn set_shell(shell: &str, legacy: bool) {
    tako_core::test_child::assert_alone("set_shell");
    std::env::set_var("SHELL", shell);
    if legacy {
        std::env::set_var(LEGACY, "1");
    } else {
        std::env::remove_var(LEGACY);
    }
}

/// ホームのファイル（`.local` = 偽の claude と tako CLI の symlink の置き場は除く）の名前とバイト列
fn snapshot(home: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(home)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_name() != ".local" && entry.path().is_file())
                .map(|entry| {
                    (
                        entry.file_name().to_string_lossy().into_owned(),
                        std::fs::read(entry.path()).unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

fn names(files: &[(String, Vec<u8>)]) -> Vec<String> {
    files
        .iter()
        .map(|(rel, bytes)| format!("{rel}({}B)", bytes.len()))
        .collect()
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

/// 2: 実経路。dash / ksh で path の後に段が進み、本物のそのシェルのログインから PATH が見える
#[test]
fn dashとkshはpathの後に段が進み本物のログインでpathが見える() {
    use tako_control::setup_bootstrap::Step;
    use tako_control::welcome;
    if tako_core::run_alone!(dashとkshはpathの後に段が進み本物のログインでpathが見える)
    {
        return;
    }
    let (_scratch, root) = sandbox("path");
    let mut failures: Vec<String> = Vec::new();
    let mut advanced = 0;
    let mut want_advanced = 0;
    for name in VERIFIED {
        let Some(shell) = real_shell(name) else {
            println!("{name}: 手元に無いので見ない");
            continue;
        };
        // (ラベル, 利用者の .profile が先に在るか, A/B の旧挙動か)
        for (suffix, with_profile, legacy) in [
            ("", false, false),
            ("+profile", true, false),
            ("-legacy", false, true),
        ] {
            let label = format!("{name}{suffix}");
            let home = fresh_home(&root, &label, with_profile);
            set_shell(&shell, legacy);
            assert_no_real_agents(&label);
            let before = snapshot(&home);
            let status_before = setup_bootstrap::status_for(AgentKind::Claude).expect("status");
            if status_before.step != Step::Path {
                failures.push(format!(
                    "{label}: 書く前の段が path でない（砂場の前提が崩れた）: {:?}",
                    status_before.step
                ));
                continue;
            }
            let path = setup_bootstrap::ensure_path_for(AgentKind::Claude);
            let status = setup_bootstrap::status_for(AgentKind::Claude).expect("status");
            let step = welcome::probe();
            let wrote = snapshot(&home);
            let found = login_run(&shell, &home, "command -v claude");
            let found = found.trim();
            let undo = setup_bootstrap::undo_path_for(AgentKind::Claude);
            let after = snapshot(&home);
            let summary = path.as_ref().map_or_else(
                |e| format!("断り {}", e.lines().next().unwrap_or("")),
                |v| {
                    format!(
                        "shell={} profile={} verified={}",
                        v["shell"], v["profile_display"], v["verified"]
                    )
                },
            );
            println!(
                "{label}: path {summary} → {:?} / status の段={:?} / welcome の段={} / {name} -l から claude: {found:?} \
                 → undo {} → {:?}",
                names(&wrote),
                status.step,
                step.kind(),
                undo.as_ref().map_or_else(|e| format!("断り {}", e.lines().next().unwrap_or("")), |v| format!("change={}", v["change"])),
                names(&after)
            );
            if legacy {
                // A/B: 旧挙動では断りで止まり、何も書かず、段は path のまま（= この番犬が修正前を見分ける）
                if path.is_ok()
                    || wrote != before
                    || status.step != Step::Path
                    || step.kind() != "path"
                {
                    failures.push(format!(
                        "{label}: {LEGACY}=1 で旧挙動（断り・書かない・段は path）にならない = A/B が効いていない（{}）",
                        table_at()
                    ));
                }
                continue;
            }
            want_advanced += 1;
            let ok_path = matches!(&path, Ok(v) if v["shell"] == "sh" && v["profile_display"] == "~/.profile" && v["verified"] == true);
            let profile = wrote
                .iter()
                .find(|(rel, _)| rel == ".profile")
                .map(|(_, bytes)| String::from_utf8_lossy(bytes).into_owned())
                .unwrap_or_default();
            let kept_user = !with_profile || profile.starts_with(USER_LINE);
            let visible = found == home.join(".local/bin/claude").display().to_string();
            if ok_path && kept_user && status.step != Step::Path && step.kind() != "path" && visible
            {
                advanced += 1;
            } else {
                failures.push(format!(
                    "{label}: path {summary} / 利用者の行を保った={kept_user} / status の段={:?} / welcome の段={} / \
                     本物の {name} -l から claude={found:?}（~/.local/bin/claude のはず）= `.profile` へ書いて段が進み、\
                     ログインで見えるはず（#2603。{}）",
                    status.step,
                    step.kind(),
                    table_at()
                ));
            }
            if after != before {
                failures.push(format!(
                    "{label}: undo-path の後のファイルが書く前と違う（前 {:?} → 後 {:?}。{}）",
                    names(&before),
                    names(&after),
                    locate(SHELL_PROFILE, "fn remove_block_at(")
                ));
            }
        }
    }
    std::env::remove_var(LEGACY);
    println!("dash / ksh で path の後に段が進んだ: {advanced} / {want_advanced}");
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
