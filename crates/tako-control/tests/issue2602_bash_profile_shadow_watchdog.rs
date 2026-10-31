//! **#2602 の番犬**: setup の PATH 通しが作った `~/.bash_profile` が、`bash -l` の `~/.profile` /
//! `~/.bash_login` を覆い隠す形へ戻らない。
//!
//! ## 何が起きていたのか
//!
//! bash のログインは `.bash_profile` → `.bash_login` → `.profile` の**最初に在る 1 つだけ**を読む。
//! #2602 より前は 2 つの経路でこの順を崩していた（main `afc400d` の砂場 + 本物の `/bin/bash -l` で実測）:
//!
//! 1. **path**: `SHELL=/bin/bash` で `.profile`（か `.bash_login`）だけを持つ人に、bash の書き先 `.bash_profile` を
//!    新しく作った。その時点から元のファイルの中身（PATH・環境変数）が `bash -l` から消えた（`verified=true` なので
//!    setup は成功と答えた）
//! 2. **undo-path**: ブロックの除去は「元のバイト列へ戻す」1 実装で、tako が作ったファイルは 0 バイトで残った。
//!    空の `.bash_profile` が以後の `bash -l` で `.bash_login` / `.profile` を覆い隠した
//!
//! いまは bash の書き先を `bash -l` が実際に読む 1 つ（`ShellKind::login_profile_in`。どれも無ければ
//! `.bash_profile`）にし、ブロックを外して 0 バイトになったファイルは消す（`remove_block_at`）。
//! A/B は `TAKO_2602_LEGACY=1`。外れたら file:line で名指す。
//!
//! ## ここで止めるもの
//!
//! 1. [`bashの書き先の表と本物のbash_lが読むファイルが一致する`] — 実測。3 つの候補の全 8 通りの有無で、
//!    `login_profile_in` の答えが本物の `bash -l` の読むファイルと同じ
//! 2. [`profileかbash_loginだけの人にpathとundoを当ててもbash_lがその中身を読み続ける`] — 実経路。
//!    path の後も undo の後も、本物の `bash -l` が利用者の変数を読む。何も無い人は undo の後に
//!    `.bash_profile` が残らず、後から作った `.profile` を読む。同じテストの中で `TAKO_2602_LEGACY=1` は
//!    読まなくなることも見る（A/B）
//! 3. [`undoの後のファイルはtakoが書く前と同じ`] — 実経路。既知のシェル（zsh / bash / sh / fish）×
//!    `.bash_profile` / `.profile` の有無（無い・片方・両方）で、path → undo-path の後のホームのファイル
//!    （名前とバイト列）が書く前と一致する（tako が作った親ディレクトリ = fish の `~/.config/fish` は数えない）
//!
//! **本物の環境を書き換えない**: `HOME` / `TAKO_DATA_DIR` / `TAKO_ORCHESTRATOR_DIR` / `CLAUDE_CONFIG_DIR` は
//! 使い捨ての器（`ScratchDir`）の下、プロセスの `PATH` は空の dir（`TMPDIR` は器の後始末が一時の置き場の下かを
//! 確かめるので変えない）。偽の claude は認証の問い合わせに「未ログイン」と答えるだけ。
//! env はプロセス全体の状態なので、2 / 3 は `run_alone!` の子で走らせる（#2400）。1 は子プロセスへ env を渡すだけ。
//! unix だけ（Windows の PATH 通しはユーザー環境変数へ書くので `SHELL` を読まない）
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tako_control::setup_bootstrap;
use tako_core::platform::agent_install::AgentKind;
use tako_core::shell_profile::{ShellKind, BASH_LOGIN_PROFILES};

#[path = "common/production_range.rs"]
mod production_range;

const SHELL_PROFILE: &str = "crates/tako-core/src/shell_profile.rs";
const BOOTSTRAP: &str = "crates/tako-control/src/setup_bootstrap.rs";

/// 旧挙動へ戻す A/B の env
const LEGACY: &str = "TAKO_2602_LEGACY";

const BASH: &str = "/bin/bash";

/// 偽の claude（PATH の外 = `~/.local/bin`）: 認証の問い合わせに「未ログイン」と答えるだけ
const FAKE_CLAUDE: &str = "#!/bin/sh\ncase \"$1 $2\" in\n  \"auth status\") echo '{\"loggedIn\":false}'; exit 0;;\nesac\n[ \"$1\" = \"--version\" ] && echo \"2.1.258 (Claude Code)\"\nexit 0\n";

/// 偽の fish: ログインシェルの問い合わせに何も答えない（名前で種別が決まる。fish が無い機械でも書き先を見られる）
const FAKE_FISH: &str = "#!/bin/sh\n# #2602 の偽の fish: 何も答えない\nexit 1\n";

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

/// 書き先の選び方の場所
fn chooser_at() -> String {
    format!(
        "書き先は {}（候補の順は {}）、status は {}",
        locate(SHELL_PROFILE, "pub fn login_profile_in("),
        locate(SHELL_PROFILE, "pub const BASH_LOGIN_PROFILES"),
        locate(BOOTSTRAP, "fn shell_target(")
    )
}

/// 外し方の場所
fn remover_at() -> String {
    format!("外すのは {}", locate(SHELL_PROFILE, "fn remove_block_at("))
}

fn executable(at: &Path, body: &str) {
    std::fs::write(at, body).expect("偽物を書く");
    std::fs::set_permissions(at, std::fs::Permissions::from_mode(0o755)).expect("実行ビット");
}

/// 本物の `bash -l` を `home` で起こし、`script` の標準出力を返す（env は空から組む）
fn bash_login(home: &Path, script: &str) -> String {
    let out = std::process::Command::new(BASH)
        .args(["-l", "-c", script])
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("{BASH} -l を起こせない: {e}"));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// 利用者の profile の印の変数名（`.bash_login` → `TAKO_2602_BASH_LOGIN`）
fn var_of(rel: &str) -> String {
    format!(
        "TAKO_2602_{}",
        rel.trim_start_matches('.').replace('.', "_").to_uppercase()
    )
}

/// 利用者の profile（読まれたら印の変数が立つ。PATH には `~/.local/bin` を足さない = tako が書く条件）
fn user_profile(rel: &str) -> String {
    format!("# user's own {rel} (#2602)\nexport {}=1\n", var_of(rel))
}

/// `bash -l` から見える印の変数と、`~/.local/bin` が PATH に在るか
fn bash_sees(home: &Path, rel: &str) -> (bool, bool) {
    let var = var_of(rel);
    let out = bash_login(
        home,
        &format!(
            "printf '%s ' \"${{{var}:-unset}}\"; echo \":$PATH:\" | grep -q \":$HOME/.local/bin:\" && echo yes || echo no"
        ),
    );
    let mut words = out.split_whitespace();
    (words.next() == Some("1"), words.next() == Some("yes"))
}

/// 1: 実測。bash の書き先の表と、本物の `bash -l` が読むファイル
#[test]
fn bashの書き先の表と本物のbash_lが読むファイルが一致する() {
    if !Path::new(BASH).exists() {
        println!("{BASH} が無いので見ない");
        return;
    }
    let scratch = tako_core::test_residue::ScratchDir::new("2602-table");
    let root = scratch.path().canonicalize().expect("正規化");
    let mut failures: Vec<String> = Vec::new();
    for mask in 0u8..8 {
        let present: Vec<&str> = BASH_LOGIN_PROFILES
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .map(|(_, rel)| *rel)
            .collect();
        let home = root.join(format!("h{mask}"));
        std::fs::create_dir_all(&home).expect("一時のホーム");
        for rel in &present {
            std::fs::write(home.join(rel), format!("echo READ:{rel}\n")).expect("印の profile");
        }
        let out = bash_login(&home, ":");
        let read: Vec<&str> = out
            .lines()
            .filter_map(|line| line.strip_prefix("READ:"))
            .collect();
        let chosen = ShellKind::Bash.login_profile_in(&home);
        let chosen_rel = chosen
            .strip_prefix(&home)
            .expect("ホームの下")
            .to_string_lossy()
            .into_owned();
        println!("在る {present:?}: bash -l が読んだ {read:?} / tako の書き先 ~/{chosen_rel}");
        let want: Vec<&str> = if present.is_empty() {
            Vec::new()
        } else {
            vec![chosen_rel.as_str()]
        };
        if read != want {
            failures.push(format!(
                "在る {present:?}: tako は ~/{chosen_rel} へ書くが、`bash -l` が読むのは {read:?} = 書いた PATH が \
                 見えないか、新しく作ったファイルが元の profile を覆い隠す（#2602。{}）",
                chooser_at()
            ));
        }
        if present.is_empty() && chosen_rel != ".bash_profile" {
            failures.push(format!(
                "何も無い: 書き先が ~/{chosen_rel}（従来どおり ~/.bash_profile のはず。{}）",
                chooser_at()
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// 使い捨ての器の下に砂場を作り、本番の設定・データを読み書きしないよう env を向ける
fn sandbox(tag: &str) -> (tako_core::test_residue::ScratchDir, PathBuf) {
    // env を書き換えるので `run_alone!` の子の中だけで呼ぶ（#2400）
    tako_core::test_child::assert_alone("sandbox");
    let scratch = tako_core::test_residue::ScratchDir::new(&format!("2602-{tag}"));
    let root = scratch.path().canonicalize().expect("正規化");
    for dir in ["homes", "data", "orch", "ccd", "empty-path", "fakesh"] {
        std::fs::create_dir_all(root.join(dir)).expect("一時の dir");
    }
    std::env::set_var("TAKO_DATA_DIR", root.join("data"));
    std::env::set_var("TAKO_ORCHESTRATOR_DIR", root.join("orch"));
    std::env::set_var("CLAUDE_CONFIG_DIR", root.join("ccd"));
    std::env::remove_var("CODEX_HOME");
    std::env::remove_var("TAKO_SELF_TEST");
    std::env::remove_var("ZDOTDIR");
    std::env::remove_var("ENV");
    std::env::remove_var("XDG_CONFIG_HOME");
    std::env::remove_var(LEGACY);
    std::env::set_var("PATH", root.join("empty-path"));
    (scratch, root)
}

/// 偽の claude を `~/.local/bin` に置いた新しいホームへ切り替え、`existing` の利用者の profile を置く
fn fresh_home(root: &Path, label: &str, existing: &[&str]) -> PathBuf {
    // env（HOME）を書き換えるので `run_alone!` の子の中だけで呼ぶ（#2400）
    tako_core::test_child::assert_alone("fresh_home");
    let home = root.join("homes").join(label);
    std::fs::create_dir_all(home.join(".local/bin")).expect("一時のホーム");
    executable(&home.join(".local/bin/claude"), FAKE_CLAUDE);
    for rel in existing {
        std::fs::write(home.join(rel), user_profile(rel)).expect("利用者の profile");
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
    fn walk(dir: &Path, home: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let rel = path
                .strip_prefix(home)
                .expect("ホームの下")
                .to_string_lossy()
                .into_owned();
            if rel == ".local" {
                continue;
            }
            if path.is_dir() {
                walk(&path, home, out);
            } else {
                out.push((rel, std::fs::read(&path).unwrap_or_default()));
            }
        }
    }
    let mut out = Vec::new();
    walk(home, home, &mut out);
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

/// 2: 実経路。`.profile` / `.bash_login` だけの bash 利用者に path → undo-path を当てても、
/// 本物の `bash -l` がその中身を読み続ける
#[test]
fn profileかbash_loginだけの人にpathとundoを当ててもbash_lがその中身を読み続ける() {
    if tako_core::run_alone!(profileかbash_loginだけの人にpathとundoを当ててもbash_lがその中身を読み続ける)
    {
        return;
    }
    if !Path::new(BASH).exists() {
        println!("{BASH} が無いので見ない");
        return;
    }
    let (_scratch, root) = sandbox("path");
    let mut failures: Vec<String> = Vec::new();
    let mut kept = 0;
    // (ラベル, 書く前に在る利用者の profile, A/B の旧挙動か)
    let cases: [(&str, &str, bool); 3] = [
        ("profile", ".profile", false),
        ("bash_login", ".bash_login", false),
        ("profile-legacy", ".profile", true),
    ];
    for (label, rel, legacy) in cases {
        let home = fresh_home(&root, label, &[rel]);
        set_shell(BASH, legacy);
        assert_no_real_agents(label);
        let before = snapshot(&home);
        let (read_before, _) = bash_sees(&home, rel);
        assert!(
            read_before,
            "{label}: 書く前から bash -l が ~/{rel} を読まない（前提が崩れた）"
        );
        let path = setup_bootstrap::ensure_path_for(AgentKind::Claude);
        let (read_after_path, local_bin) = bash_sees(&home, rel);
        let after_path = snapshot(&home);
        let undo = setup_bootstrap::undo_path_for(AgentKind::Claude);
        let (read_after_undo, local_bin_after_undo) = bash_sees(&home, rel);
        let after_undo = snapshot(&home);
        println!(
            "{label}: path {} → ファイル {:?} / bash -l が ~/{rel} を読む={read_after_path} ~/.local/bin={local_bin} \
             → undo {} → ファイル {:?} / 読む={read_after_undo}",
            path.as_ref().map_or_else(|e| format!("断り {e}"), |v| format!("profile={} verified={}", v["profile_display"], v["verified"])),
            names(&after_path),
            undo.as_ref().map_or_else(|e| format!("断り {e}"), |v| format!("change={}", v["change"])),
            names(&after_undo),
        );
        if legacy {
            // A/B: 旧挙動では .bash_profile を作り、その時点から ~/.profile を読まない（= この番犬が修正前を見分ける）
            if read_after_path || read_after_undo {
                failures.push(format!(
                    "{label}: {LEGACY}=1 でも bash -l が ~/{rel} を読む = A/B が効いていない（{}）",
                    chooser_at()
                ));
            }
            continue;
        }
        let mut ok = true;
        if !read_after_path || !local_bin {
            ok = false;
            failures.push(format!(
                "{label}: path の後、bash -l が ~/{rel} を読む={read_after_path}・~/.local/bin が PATH に在る={local_bin} \
                 （両方 true のはず。ファイル {:?}。{}）",
                names(&after_path),
                chooser_at()
            ));
        }
        if after_path.iter().any(|(name, _)| name == ".bash_profile") {
            ok = false;
            failures.push(format!(
                "{label}: path が ~/.bash_profile を新しく作った = 以後の bash -l が ~/{rel} を読まない（{}）",
                chooser_at()
            ));
        }
        if after_undo != before || !read_after_undo || local_bin_after_undo {
            ok = false;
            failures.push(format!(
                "{label}: undo-path の後が書く前と違う（前 {:?} → 後 {:?}・bash -l が ~/{rel} を読む={read_after_undo}・\
                 ~/.local/bin が残る={local_bin_after_undo}。{}）",
                names(&before),
                names(&after_undo),
                remover_at()
            ));
        }
        if ok {
            kept += 1;
        }
    }

    // ---- 何も無い人: undo の後に .bash_profile が残らず、後から作った .profile を bash -l が読む ----
    for legacy in [false, true] {
        let label = if legacy { "none-legacy" } else { "none" };
        let home = fresh_home(&root, label, &[]);
        set_shell(BASH, legacy);
        let path = setup_bootstrap::ensure_path_for(AgentKind::Claude);
        let wrote = snapshot(&home);
        let undo = setup_bootstrap::undo_path_for(AgentKind::Claude);
        let left = snapshot(&home);
        std::fs::write(home.join(".profile"), user_profile(".profile"))
            .expect("後から作る .profile");
        let (read_later, _) = bash_sees(&home, ".profile");
        println!(
            "{label}: path {} → {:?} → undo {} → {:?} / 後から作った ~/.profile を bash -l が読む={read_later}",
            path.as_ref().map_or_else(|e| format!("断り {e}"), |v| format!("profile={}", v["profile_display"])),
            names(&wrote),
            undo.as_ref().map_or_else(|e| format!("断り {e}"), |v| format!("change={}", v["change"])),
            names(&left),
        );
        let created = wrote.iter().any(|(name, _)| name == ".bash_profile");
        if legacy {
            if read_later {
                failures.push(format!(
                    "{label}: {LEGACY}=1 でも undo の後に作った ~/.profile を bash -l が読む = A/B が効いていない（{}）",
                    remover_at()
                ));
            }
        } else if !created || !left.is_empty() || !read_later {
            failures.push(format!(
                "{label}: path は ~/.bash_profile を作り（{created}）、undo で消えて何も残らず（残り {:?}）、\
                 後から作った ~/.profile を bash -l が読む（{read_later}）はず = 空の .bash_profile が残ると \
                 .profile を覆い隠す（#2602。{}）",
                names(&left),
                remover_at()
            ));
        } else {
            kept += 1;
        }
    }
    std::env::remove_var(LEGACY);
    println!("bash -l が利用者の profile を読み続けた: {kept} / 3");
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// 3: 実経路。既知のシェル × `.bash_profile` / `.profile` の有無で、undo-path の後のファイルが書く前と同じ
#[test]
fn undoの後のファイルはtakoが書く前と同じ() {
    if tako_core::run_alone!(undoの後のファイルはtakoが書く前と同じ) {
        return;
    }
    let (_scratch, root) = sandbox("undo");
    let fish = root.join("fakesh/fish");
    executable(&fish, FAKE_FISH);
    let mut shells: Vec<(&str, String)> = vec![("sh", "/bin/sh".to_string())];
    for (label, path) in [("zsh", "/bin/zsh"), ("bash", BASH)] {
        if Path::new(path).exists() {
            shells.push((label, path.to_string()));
        }
    }
    shells.push(("fish", fish.display().to_string()));
    let existing: [&[&str]; 4] = [
        &[],
        &[".profile"],
        &[".bash_profile"],
        &[".bash_profile", ".profile"],
    ];
    let mut failures: Vec<String> = Vec::new();
    let mut same = 0;
    let mut total = 0;
    for (shell_label, shell) in &shells {
        for files in existing {
            let label = format!("{shell_label}-{}", files.join("+").replace('.', ""));
            let home = fresh_home(&root, &label, files);
            set_shell(shell, false);
            let before = snapshot(&home);
            let path = setup_bootstrap::ensure_path_for(AgentKind::Claude);
            let wrote = snapshot(&home);
            let undo = setup_bootstrap::undo_path_for(AgentKind::Claude);
            let after = snapshot(&home);
            total += 1;
            println!(
                "{label}: 前 {:?} → path {} {:?} → undo {} {:?}",
                names(&before),
                path.as_ref()
                    .map_or_else(|e| format!("断り {e}"), |v| format!("{}", v["change"])),
                names(&wrote),
                undo.as_ref()
                    .map_or_else(|e| format!("断り {e}"), |v| format!("{}", v["change"])),
                names(&after)
            );
            if wrote == before {
                failures.push(format!(
                    "{label}: path が何も書かなかった（砂場の前提が崩れた）: {path:?}"
                ));
                continue;
            }
            if after == before {
                same += 1;
            } else {
                failures.push(format!(
                    "{label}: undo-path の後のファイルが書く前と違う（前 {:?} → 後 {:?}）。作ったファイルを 0 バイトで \
                     残すと、bash -l の .profile を覆い隠す（#2602。{}・{}）",
                    names(&before),
                    names(&after),
                    remover_at(),
                    chooser_at()
                ));
            }
        }
    }
    println!("undo-path の後が書く前と同じ: {same} / {total}");
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
