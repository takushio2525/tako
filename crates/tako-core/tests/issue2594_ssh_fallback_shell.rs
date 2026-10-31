//! 実経路の番犬: SSH ペインが ssh の失敗・切断の後に戻るローカルのシェルにも、
//! 素のペインと同じシェル統合が載る（#2594）。
//!
//! ## 何が起きていたか
//!
//! SSH ペインは `$SHELL -l -i -c "/bin/sh -c '<ssh_pane_script>'"` で起き、ssh が
//! exit 255 で終わると `exec $SHELL -l` でローカルのシェルへ戻る（#1040）。ところが外側の zsh が
//! 統合の `.zshenv` を読んだ時点で `ZDOTDIR` は利用者の値へ戻される（孫の zsh へ統合を
//! 漏らさないための作り）ので、戻りの zsh にだけ統合が載らなかった。cwd の追従（OSC 7）・
//! コマンドの状態（OSC 133）・履歴の向け直し（#1253）がどれも効かず、macOS の `/etc/zshrc` が
//! `HISTFILE` を `$HOME/.zsh_history` へ代入し直すので、本番の HOME なら利用者の本物の履歴へ、
//! 検証なら一時 HOME へ書いていた（#2588 の後片付けと競った書き手）。
//!
//! ## 何を確かめるか
//!
//! 製品と**同じ組み立て**（dispatch の `ssh_pane_script` + `script_pane_command` →
//! tako-app の `login_shell_command` → `TerminalSession::spawn`）で PTY を起こし、ssh の代わりに
//! exit 255 で落ちるコマンドを走らせて、戻りのシェルで
//!
//! 1. コマンドの状態が OSC 133 で動く（`false` の後に `Failed(1)`）
//! 2. `cd` に cwd が OSC 7 で追従する
//! 3. 履歴の書き先が素のペインと同じ（検証プロセスなので `TAKO_VERIFY_HISTFILE` の置き場）
//!
//! を zsh と bash で見る。zsh は `Relaunch::default()`（= #2594 前の文字列）で 1 と 2 が
//! 起きないことも同じテストで確かめる（検出力。#2594 の症状は zsh だけで、bash は
//! `PROMPT_COMMAND` が外側の bash を素通りするので修正前から効く）。
//!
//! 開発者の器（tmux）の中で走らせても OSC がパススルーへ包まれないよう、器の env を外した
//! 子で走らせ直す。HOME はテストの置き場（利用者の rc・履歴に触らない）
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tako_core::i18n::Lang;
use tako_core::remote_fs::{ssh_pane_script, SshFailureRule};
use tako_core::shell_integration::Relaunch;
use tako_core::terminal::{CommandState, SessionEvent, SpawnOptions, TerminalSession};

/// 戻りのシェルのプロンプト（実ユーザー名・ホスト名を画面へ出さない = #927）
const PROMPT: &str = "T2594> ";

/// ssh の代わり: ssh の失敗行を出して exit 255（ssh 自身の失敗と同じ終了コード）
const FAKE_SSH: &str = "echo 'ssh: Could not resolve hostname no-such-host-2594: \
                        nodename nor servname provided, or not known' >&2; exit 255";

struct Pane {
    session: TerminalSession,
    rx: futures::channel::mpsc::UnboundedReceiver<SessionEvent>,
}

impl Pane {
    fn pump(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            let _ = self.session.process_event(ev);
        }
    }

    fn screen(&self) -> String {
        self.session.visible_lines().join("\n")
    }

    fn prompts(&self) -> usize {
        self.session
            .visible_lines()
            .iter()
            .filter(|l| l.starts_with(PROMPT.trim_end()))
            .count()
    }

    /// 条件が揃うまで待つ。上限は機の混み具合で伸ばす（#1162）。尽きたら画面つきで落ちる
    fn wait_for<F: Fn(&Self) -> bool>(&mut self, what: &str, pred: F) {
        let budget = tako_core::wait_budget::state_wait_budget(
            Duration::from_secs(30),
            tako_core::wait_budget::machine_busy(),
        );
        let deadline = Instant::now() + budget;
        loop {
            self.pump();
            if pred(self) {
                // シェルは OSC 133 / 7 をプロンプトの文字より先に出すので、プロンプトが画面に
                // 見えた時点でその通知は届いている。ここで汲み切ってから状態・cwd を読ませる
                self.pump();
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{what} が上限 {budget:?} までに揃わない: state={:?} cwd={:?}\n{}",
                self.session.command_state(),
                self.session.cwd(),
                self.screen()
            );
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    /// 1 行打ち、次のプロンプトが出るまで待つ
    fn run(&mut self, line: &str) {
        let before = self.prompts();
        self.session.write(format!("{line}\r").into_bytes());
        self.wait_for(&format!("`{line}` の後のプロンプト"), |p| {
            p.prompts() > before
        });
    }
}

/// 戻りのシェルで観測したこと
struct Observed {
    state: CommandState,
    cwd: Option<PathBuf>,
    histfile: String,
    screen: String,
}

/// 製品と同じ組み立てで SSH ペインを起こし、戻りのシェルで cd / 履歴の置き場 / `false` を打つ
fn observe(shell: &str, relaunch: &Relaunch, scratch: &Path, tag: &str) -> Observed {
    let home = scratch.join(format!("home-{tag}"));
    let work = scratch.join(format!("work-{tag}"));
    let moved = work.join("moved");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&moved).unwrap();
    // rc が 1 つも無いと zsh は初回設定メニューを出す。プロンプトは固定の文字列にする
    std::fs::write(
        home.join(".zshrc"),
        format!("PROMPT='{PROMPT}'\nRPROMPT=''\n"),
    )
    .unwrap();
    std::fs::write(home.join(".bash_profile"), format!("PS1='{PROMPT}'\n")).unwrap();

    // dispatch（`Request::OpenRemote` の split / tab）と同じ組み立て
    let argv = vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        FAKE_SSH.to_string(),
    ];
    let script = ssh_pane_script(
        tako_core::platform::shell::script_dialect(),
        &argv,
        "no-such-host-2594",
        None,
        Lang::Ja,
        SshFailureRule::ClientFailure,
        relaunch,
    );
    // tako-app の `spawn_session` と同じ包み（`$SHELL -l -i -c …`）。SHELL は子の env で決めてある
    let command =
        tako_core::login_shell_command(tako_core::platform::shell::script_pane_command(&script));
    assert_eq!(command.program, shell, "外側のシェルが {shell} でない");
    // 一時の置き場のパスは長いので、`cd` の行と `HF=` の行が折り返さない幅にする
    let (session, rx) = TerminalSession::spawn(
        240,
        40,
        SpawnOptions {
            command: Some(command),
            cwd: Some(work.clone()),
            env: vec![
                ("TAKO_PANE_ID".into(), "1".into()),
                ("HOME".into(), home.display().to_string()),
                ("SHELL".into(), shell.to_string()),
            ],
            scrollback_lines: None,
        },
    )
    .expect("PTY を起こせる");
    let mut pane = Pane { session, rx };
    pane.wait_for(
        "ssh の失敗の後に戻ったシェルのプロンプト",
        |p| {
            let screen = p.screen();
            screen.contains("ローカルのシェルに戻ります") && p.prompts() >= 1
        },
    );
    pane.run(&format!("cd '{}'", moved.display()));
    pane.run("echo \"HF=$HISTFILE\"");
    pane.run("false");
    let screen = pane.screen();
    let histfile = screen
        .lines()
        .find_map(|l| l.strip_prefix("HF="))
        .unwrap_or_default()
        .trim()
        .to_string();
    Observed {
        state: pane.session.command_state(),
        cwd: pane.session.cwd().map(Path::to_path_buf),
        histfile,
        screen,
    }
}

fn same_dir(a: Option<&Path>, b: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    a.is_some_and(|a| canon(a) == canon(b))
}

#[test]
fn ssh_の失敗後に戻るシェルにも統合が載る() {
    // 開発者の器（tmux）の env が残ると統合が OSC をパススルーへ包み、PTY 直下の
    // TerminalSession が読めない。器と統合の控えを外した子で走らせ直す（#2400 の入口）
    if tako_core::test_child::rerun_alone(
        tako_core::test_child::libtest_name(concat!(
            module_path!(),
            "::",
            "ssh_の失敗後に戻るシェルにも統合が載る"
        )),
        &[
            "TMUX",
            "TMUX_PANE",
            "TAKO_BACKEND_SOCKET",
            "TAKO_OSC_SINK",
            "ZDOTDIR",
            "TAKO_ORIG_ZDOTDIR",
            "PROMPT_COMMAND",
        ],
    ) {
        return;
    }
    let scratch = tako_core::test_residue::ScratchDir::new("issue2594-fallback");
    let verify_histfile = tako_core::paths::verification_histfile_env()
        .into_iter()
        .find(|(k, _)| k == "HISTFILE")
        .map(|(_, v)| v)
        .expect("テストは検証プロセスなので履歴の置き場がある（#1253）");

    for shell in ["/bin/zsh", "/bin/bash"] {
        if !Path::new(shell).exists() {
            println!("{shell} が無いので飛ばす");
            continue;
        }
        // `login_shell_command` はこのプロセスの SHELL を読む（子の中だけ。#2400）
        unsafe { std::env::set_var("SHELL", shell) };
        let name = Path::new(shell).file_name().unwrap().to_string_lossy();
        let moved_of = |tag: &str| scratch.path().join(format!("work-{tag}")).join("moved");

        // 修正後: 素のペインと同じ組（`shell_integration::relaunch` = dispatch が渡すもの）
        let tag = format!("{name}-fixed");
        let got = observe(
            shell,
            &tako_core::shell_integration::relaunch(),
            scratch.path(),
            &tag,
        );
        assert_eq!(
            got.state,
            CommandState::Failed(1),
            "{shell}: 戻りのシェルの `false` が OSC 133 で届かない（#2594）\n{}",
            got.screen
        );
        assert!(
            same_dir(got.cwd.as_deref(), &moved_of(&tag)),
            "{shell}: 戻りのシェルの cd に cwd が追従しない（OSC 7。#2594）: {:?}\n{}",
            got.cwd,
            got.screen
        );
        assert_eq!(
            got.histfile, verify_histfile,
            "{shell}: 戻りのシェルの履歴の書き先が素のペインと違う（#1253 / #2594）\n{}",
            got.screen
        );

        // 修正前（= `TAKO_2594_LEGACY=1` の文字列）: zsh は統合が載らない。検出力の確認
        if name == "zsh" {
            let tag = format!("{name}-legacy");
            let got = observe(shell, &Relaunch::default(), scratch.path(), &tag);
            assert_eq!(
                got.state,
                CommandState::Unknown,
                "修正前の文字列で OSC 133 が届いた = この番犬が #2594 を見分けられていない\n{}",
                got.screen
            );
            assert!(
                !same_dir(got.cwd.as_deref(), &moved_of(&tag)),
                "修正前の文字列で cwd が追従した = この番犬が #2594 を見分けられていない\n{}",
                got.screen
            );
            println!("修正前の zsh の HISTFILE（参考）: {}", got.histfile);
        }
    }
}

/// 番犬: 製品のコードで `ssh_pane_script` を呼ぶところは、どれも
/// `shell_integration::relaunch()` を渡している（空の組を渡すと戻りのシェルだけ統合が外れる =
/// #2594 へ戻る）。上の実経路はスクリプトの組み立てまでしか見ないので、呼び手はここで見る
#[test]
fn ssh_pane_script_の呼び手は戻りのシェルへ統合を渡している() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/");
    let mut files = Vec::new();
    collect_rs(crates, &mut files);
    let mut calls = 0;
    let mut bad = Vec::new();
    for file in &files {
        let rel = file.strip_prefix(crates).unwrap_or(file);
        // 製品のコード（`crates/<crate>/src/`）だけを見る
        if rel
            .components()
            .nth(1)
            .is_none_or(|c| c.as_os_str() != "src")
        {
            continue;
        }
        let text = std::fs::read_to_string(file).unwrap_or_default();
        // テストのモジュールは空の組で文字列の形を検査するので見ない
        let tests = test_mod_ranges(&text);
        for (idx, _) in text.match_indices("ssh_pane_script(") {
            if tests.iter().any(|&(s, e)| (s..e).contains(&idx)) {
                continue;
            }
            let line_start = text[..idx].rfind('\n').map_or(0, |i| i + 1);
            let head = text[line_start..].trim_start();
            // doc・コメント・定義そのものは呼び出しではない
            if head.starts_with("//") || head.starts_with("pub fn ssh_pane_script(") {
                continue;
            }
            calls += 1;
            let rest = &text[idx..];
            let args = &rest[..rest.find(");").map_or(rest.len(), |e| e + 2)];
            if !args.contains("shell_integration::relaunch()") {
                let line_no = text[..idx].matches('\n').count() + 1;
                bad.push(format!("crates/{}:{line_no}", rel.display()));
            }
        }
    }
    assert!(
        calls > 0,
        "製品のコードに ssh_pane_script の呼び出しが 1 つも見つからない（改名した？ この番犬も直す）"
    );
    assert!(
        bad.is_empty(),
        "ssh_pane_script へ `&tako_core::shell_integration::relaunch()` を渡していない呼び手がある\
         （ssh の失敗・切断で戻るシェルに統合が載らない = #2594）: {bad:?}"
    );
}

/// 行頭の `#[cfg(test)]` + `mod …` から、行頭の `}` までの範囲（テストのモジュール）
fn test_mod_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(pos) = text[from..].find("\n#[cfg(test)]\nmod ") {
        let start = from + pos + 1;
        let end = text[start..]
            .find("\n}\n")
            .map_or(text.len(), |e| start + e + 3);
        out.push((start, end));
        from = end;
    }
    out
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}
