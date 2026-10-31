//! **#2597 の番犬**: csh / tcsh の利用者の明示コマンドのペインが `$SHELL -l -i -c` へ戻らない。
//!
//! ## 何が起きていたのか
//!
//! 明示コマンドを持つペイン（orchestrator の worker の起動・Code Runner・コマンドカード・`tako split --`・
//! SSH ペイン）は `platform::shell::login_shell_command` = `$SHELL -l -i -c <片>` で起きる。tcsh / csh は
//! `-l` を「唯一の引数」としてしか受け付けないので、`SHELL` が csh 系の利用者では
//! `Unknown option: '-l'` で rc=1 になり、ペインは何も残さず消えていた（2026-11-01 の隔離 GUI で実測:
//! split のペインも Code Runner のペインも 3 本とも即座に閉じ、ペインのログにこの 1 行だけが残る）。
//! 器（tmux）を通すペインはその手前で崩れていた: tmux は内側コマンドの 1 行を default-shell（= `$SHELL`）の
//! `-c` で解釈するので、複数行の片が `Unmatched '''.` になり、ペインが「実行中」のまま止まる。
//!
//! いまはログインの層が起こし方の表（`exe::login_invocation` の csh 系の行 = argv[0] を `-<名>` にして
//! `-c 'exec /bin/sh -c $argv[1]:q' <片>`。#2577）を引き、PTY の子へ argv[0] を渡す口が無いぶんを
//! `exec -a` を持つシェル（在る zsh / bash。macOS は `/bin/zsh -fc 'exec -a "$0" "$@"'`）の段で補う
//! （どれも無ければ argv[0] なしの非ログインの `-c` = 起きはする）。器の層は `$SHELL` が csh 系なら csh の引用
//! （`shell::quote_for_csh` = `!` と改行の前に `\`）で 1 行を組む。csh 系でないシェルの argv と 1 行は修正前と同じ。
//!
//! ## ここで止めるもの
//!
//! 1. [`ログインの層と器の層の配線`] — 上の 2 層が表と csh の引用を通していること（外れたら file:line）
//! 2. [`修正前の形を名指しできる`] — 1 へ修正前の形を注入すると file:line で落ちる（同じテストの中の A/B）
//! 3. [`本物のtcshで明示コマンドのペインがログインのpathのコマンドを起こす`] — 実経路（unix・`/bin/tcsh` が
//!    在るとき）。砂場の HOME の `.login` でだけ足した PATH のコマンドを、PTY のペイン（`split --` と同じ
//!    `hold_on_failure_command` の包み）と、器の default-shell の `-c`（tmux と #2367 の段の形）から起こせる。
//!    引用符・空白・`$`・`!`・改行を含む引数がそのまま届き、終了コードも伝わる。`TAKO_2597_LEGACY=1`
//!    （修正前の形）では起こせないことも同じテストの中で見る。csh 系でないシェル（zsh / bash / sh / dash /
//!    ksh / fish の名前 / `SHELL` 未設定）の argv と器の 1 行は修正前の式とバイト一致
//!
//! 明示コマンドの無い対話のペイン（`default_shell` = `$SHELL -l`）はここで見ない（#1945 の理由で別の口。
//! tcsh も `-l` 単独なら受け付ける）。
//!
//! **本物の環境を書き換えない**: 実経路の `HOME` は使い捨ての器（`ScratchDir`）の下、`PATH` は launchd の既定。
//! env はプロセス全体の状態なので `run_alone!` の子で走らせる（#2400）。`TMPDIR` は変えない

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const SHELL_RS: &str = "crates/tako-core/src/platform/shell.rs";
const BACKEND: &str = "crates/tako-core/src/backend/mod.rs";
const TMUX: &str = "crates/tako-core/src/tmux_backend.rs";
const QUOTE: &str = "crates/tako-core/src/shell.rs";
const EXE: &str = "crates/tako-core/src/platform/exe.rs";

/// 旧挙動へ戻す A/B の env
#[cfg(unix)]
const LEGACY: &str = "TAKO_2597_LEGACY";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// テスト領域を空白へ潰した本文（行番号・バイト位置は原文と同じ。#1420）
fn production(rel: &str) -> String {
    let path = repo_root().join(rel);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
    production_range::scan(&src).text
}

fn line_of(text: &str, at: usize) -> usize {
    text[..at].bytes().filter(|&b| b == b'\n').count() + 1
}

/// `head` で始まる関数の本体（頭の行番号と、最初の `{` から対応する `}` まで）
fn fn_body<'a>(text: &'a str, head: &str) -> Option<(usize, usize, &'a str)> {
    let start = text.find(head)?;
    let open = start + text[start..].find('{')?;
    let mut depth = 0usize;
    for (i, b) in text[open..].bytes().enumerate() {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((line_of(text, start), open, &text[open..=open + i]));
                }
            }
            _ => {}
        }
    }
    None
}

/// `rel` の関数 `head` の本体が `needles` をすべて含むか。足りなければ file:line で名指す
fn require(out: &mut Vec<String>, rel: &str, text: &str, head: &str, needles: &[&str], why: &str) {
    match fn_body(text, head) {
        None => out.push(format!(
            "{rel}: `{head}` が無い（改名したなら番犬も追うこと）"
        )),
        Some((line, _, body)) => {
            for needle in needles {
                if !body.contains(needle) {
                    out.push(format!(
                        "{rel}:{line}（{head}…）に `{needle}` が無い。{why}"
                    ));
                }
            }
        }
    }
}

/// 1 の本体
fn wiring_failures(shell_rs: &str, backend: &str, tmux: &str, quote: &str) -> Vec<String> {
    let mut out = Vec::new();
    // ログインの層: csh 系は表の行を引いて argv[0] の段で起こし、その腕は `-l` を持たない
    require(
        &mut out,
        SHELL_RS,
        shell_rs,
        "fn posix_login_shell_command(",
        &[
            "login_form(shell) == LoginForm::Argv0",
            "posix_argv0_command(&login_invocation(shell, script)",
        ],
        "csh / tcsh の利用者の明示コマンドのペインが `-l` で必ず落ちる（#2597）",
    );
    if let Some((_, open, body)) = fn_body(shell_rs, "fn posix_login_shell_command(") {
        if let Some(at) = body.find("LoginForm::Argv0") {
            let arm = &body[at..];
            let arm = &arm[..arm.find("let mut args").unwrap_or(arm.len())];
            if arm.contains("\"-l\"") || arm.contains("\"-i\"") {
                out.push(format!(
                    "{SHELL_RS}:{}（fn posix_login_shell_command の csh 系の腕）が `-l` / `-i` を渡す。\
                     tcsh / csh は `Unknown option: '-l'` で必ず落ちる（#2597）",
                    line_of(shell_rs, open + at)
                ));
            }
        }
    }
    require(
        &mut out,
        SHELL_RS,
        shell_rs,
        "fn posix_argv0_command(",
        &["ARGV0_EXEC.to_string()", "arg0.clone()", "invocation.program.clone()"],
        "PTY の子は argv[0] を渡す口が無いので、段を挟まないと tcsh が非ログインで起き `~/.login` を読まない（#2597）",
    );
    match shell_rs.find("const ARGV0_EXEC: &str = ") {
        None => out.push(format!("{SHELL_RS}: `const ARGV0_EXEC` が無い（#2597）")),
        Some(at) => {
            let line = &shell_rs[at..at + shell_rs[at..].find('\n').unwrap_or(0)];
            if !line.contains("exec -a \\\"$0\\\" \\\"$@\\\"") {
                out.push(format!(
                    "{SHELL_RS}:{}（ARGV0_EXEC）が `exec -a \"$0\" \"$@\"` でない（argv[0] が当たらない。#2597）",
                    line_of(shell_rs, at)
                ));
            }
        }
    }
    // 器の層: tmux の腕は `$SHELL` が csh 系なら csh の引用で 1 行を組む
    require(
        &mut out,
        BACKEND,
        backend,
        "pub(crate) fn compose_inner_command(",
        &["crate::tmux_backend::container_command_line(command)"],
        "器の default-shell が csh 系だと複数行の片が `Unmatched '''.` で崩れる（#2597）",
    );
    require(
        &mut out,
        TMUX,
        tmux,
        "pub(crate) fn container_command_line_for(",
        &[
            "crate::platform::exe::is_csh_family",
            "shell_quoted_with(command, crate::shell::quote_for_csh)",
        ],
        "器の default-shell が csh 系だと複数行の片が `Unmatched '''.` で崩れる（#2597）",
    );
    require(
        &mut out,
        QUOTE,
        quote,
        "pub fn quote_for_csh(",
        &[".replace('!', \"\\\\!\")", ".replace('\\n', \"\\\\\\n\")"],
        "csh は単引用符の中でも `!`（履歴置換）と改行を解釈する（#2597）",
    );
    out
}

/// 1 の続き: csh 系の判定（ペインの csh 系の腕と器の csh の引用が同じ判定を引く）が tcsh / csh を拾う
fn table_failures(exe: &str) -> Vec<String> {
    let mut out = Vec::new();
    require(
        &mut out,
        EXE,
        exe,
        "pub fn is_csh_family(",
        &["matches!(name, \"csh\" | \"tcsh\")"],
        "tcsh / csh の利用者のペインが `-l -i -c` へ戻り、器の 1 行も POSIX の引用へ戻る（#2597）",
    );
    require(
        &mut out,
        EXE,
        exe,
        "pub fn login_form(",
        &["if is_csh_family(shell)"],
        "起こし方の表が csh 系の行を返さない = ペインが `-l` で落ちる（#2597）",
    );
    out
}

fn wiring_now() -> (String, String, String, String) {
    (
        production(SHELL_RS),
        production(BACKEND),
        production(TMUX),
        production(QUOTE),
    )
}

/// 1: 2 層の配線
#[test]
fn ログインの層と器の層の配線() {
    let (s, b, t, q) = wiring_now();
    let mut found = wiring_failures(&s, &b, &t, &q);
    found.extend(table_failures(&production(EXE)));
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// 2: 修正前の形（csh 系の腕を外す・腕に `-l` を戻す・段を外す・器の 1 行を POSIX の引用へ戻す・
/// `!` を逃がさない）を 1 が名指す
#[test]
fn 修正前の形を名指しできる() {
    let (s, b, t, q) = wiring_now();
    let named = |s: &str, b: &str, t: &str, q: &str, want: &str, what: &str| {
        let got = wiring_failures(s, b, t, q);
        assert!(
            got.iter().any(|m| m.starts_with(want)),
            "{what} を {want} で名指さない: {got:?}"
        );
    };
    let swap = |text: &str, rel: &str, from: &str, to: &str| {
        assert!(text.contains(from), "{rel}: 注入の起点 `{from}` が無い");
        text.replacen(from, to, 1)
    };

    // (a) ログインの層が表を引かない（#2597 前 = csh 系も `-l -i -c`）
    let injected = swap(
        &s,
        SHELL_RS,
        "login_form(shell) == LoginForm::Argv0",
        "login_form(shell) == LoginForm::Flag && false",
    );
    named(
        &injected,
        &b,
        &t,
        &q,
        &format!("{SHELL_RS}:"),
        "表を引かないログインの層",
    );

    // (b) csh 系の腕へ `-l` を戻す
    let from = "return posix_argv0_command(&login_invocation(shell, script)";
    let injected = swap(
        &s,
        SHELL_RS,
        from,
        "let _ = [\"-l\"];\n        return posix_argv0_command(&login_invocation(shell, script)",
    );
    named(
        &injected,
        &b,
        &t,
        &q,
        &format!("{SHELL_RS}:"),
        "csh 系の腕の `-l`",
    );
    assert!(wiring_failures(&injected, &b, &t, &q)
        .iter()
        .any(|m| m.contains("csh 系の腕）が `-l` / `-i` を渡す")));

    // (c) 段が argv[0] を当てない
    let injected = swap(&s, SHELL_RS, "arg0.clone(),", "String::new(),");
    named(
        &injected,
        &b,
        &t,
        &q,
        &format!("{SHELL_RS}:"),
        "argv[0] を当てない段",
    );

    // (d) 器の 1 行が POSIX の引用のまま
    let injected = swap(
        &b,
        BACKEND,
        "crate::tmux_backend::container_command_line(command)",
        "crate::tmux_backend::shell_quoted(command)",
    );
    named(
        &s,
        &injected,
        &t,
        &q,
        &format!("{BACKEND}:"),
        "POSIX の引用のままの器の 1 行",
    );

    // (e) csh の引用が `!` を逃がさない
    let injected = swap(&q, QUOTE, ".replace('!', \"\\\\!\")", "");
    named(
        &s,
        &b,
        &t,
        &injected,
        &format!("{QUOTE}:"),
        "`!` を逃がさない csh の引用",
    );

    // (f) csh 系の判定から tcsh を外す
    let exe = production(EXE);
    let injected = swap(
        &exe,
        EXE,
        "matches!(name, \"csh\" | \"tcsh\")",
        "matches!(name, \"csh\")",
    );
    let got = table_failures(&injected);
    assert!(
        got.iter().any(|m| m.starts_with(&format!("{EXE}:"))),
        "tcsh を外した判定を {EXE} で名指さない: {got:?}"
    );

    // 注入しなければ何も名指さない（同じテストの中の A/B）
    assert!(wiring_failures(&s, &b, &t, &q).is_empty());
    assert!(table_failures(&exe).is_empty());
}

/// 3: 実経路。本物の tcsh / csh で、明示コマンドのペインが `.login` でだけ通る PATH のコマンドを起こす
#[cfg(unix)]
#[test]
fn 本物のtcshで明示コマンドのペインがログインのpathのコマンドを起こす() {
    use std::os::unix::fs::PermissionsExt;
    use tako_core::platform::shell::{hold_on_failure_command, login_shell_command};
    use tako_core::terminal::{SpawnCommand, SpawnOptions, TerminalSession};

    const TCSH: &str = "/bin/tcsh";
    if !Path::new(TCSH).exists() {
        eprintln!("{TCSH} が無いので実経路は飛ばす（macOS は同梱）");
        return;
    }
    if tako_core::run_alone!(本物のtcshで明示コマンドのペインがログインのpathのコマンドを起こす)
    {
        return;
    }
    tako_core::test_child::assert_alone("2597 の砂場");
    let scratch = tako_core::test_residue::ScratchDir::new("2597-tcsh");
    let root = scratch.path().canonicalize().expect("正規化");
    let home = root.join("home");
    let only = home.join("onlylogin");
    let outs = root.join("outs");
    std::fs::create_dir_all(&only).expect("一時の dir");
    std::fs::create_dir_all(&outs).expect("一時の dir");
    // 起きたら自分の引数を 1 行ずつ（NUL 区切り）書き出す。置き場は第 1 引数
    let probe = only.join("tako2597probe");
    std::fs::write(
        &probe,
        "#!/bin/sh\nout=$1; shift\nprintf '%s\\0' \"$@\" > \"$out\"\n",
    )
    .expect("偽物を書く");
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755)).expect("実行ビット");
    std::fs::write(
        home.join(".login"),
        "setenv PATH \"$HOME/onlylogin:$PATH\"\n",
    )
    .expect(".login を書く");
    // 本物の利用者名・ホスト名を画面へ出さない（#927）
    std::fs::write(home.join(".tcshrc"), "set prompt='T2597> '\n").expect(".tcshrc を書く");
    let alias = root.join("mysh");
    std::os::unix::fs::symlink(TCSH, &alias).expect("symlink");
    std::env::set_var("HOME", &home);
    std::env::set_var("PATH", tako_core::platform::exe::DEFAULT_LOGIN_PATH);
    std::env::remove_var(LEGACY);
    std::env::remove_var("TAKO_2577_LEGACY");
    std::env::remove_var("TAKO_1031_LEGACY");
    std::env::remove_var("ZDOTDIR");

    // 引用符・空白・`$`・`!`・改行・日本語
    let words: Vec<String> = ["a b", "it's", "$HOME", "x!y", "l1\nl2", "日本語"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let want: Vec<u8> = words
        .iter()
        .flat_map(|w| [w.as_bytes(), b"\0"].concat())
        .collect();
    let probe_cmd = |out: &Path| SpawnCommand {
        program: "tako2597probe".into(),
        args: std::iter::once(out.display().to_string())
            .chain(words.iter().cloned())
            .collect(),
    };
    let budget = tako_core::wait_budget::state_wait_budget(
        std::time::Duration::from_secs(20),
        tako_core::wait_budget::machine_busy(),
    );
    // 修正前の形は起きない側を確かめるだけなので短く待つ（起きるなら数十 ms で書く）
    let wait_file = |path: &Path, budget: std::time::Duration| {
        let deadline = std::time::Instant::now() + budget;
        while std::time::Instant::now() < deadline {
            if std::fs::read(path).is_ok_and(|b| !b.is_empty()) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        false
    };

    let mut shells = vec![TCSH.to_string(), alias.display().to_string()];
    if Path::new("/bin/csh").exists() {
        shells.push("/bin/csh".to_string());
    }
    let mut failures = Vec::new();
    for (i, shell) in shells.iter().enumerate() {
        std::env::set_var("SHELL", shell);
        for legacy in [false, true] {
            if legacy {
                std::env::set_var(LEGACY, "1");
            } else {
                std::env::remove_var(LEGACY);
            }
            let tag = format!("{i}-{}", if legacy { "legacy" } else { "fixed" });

            // (a) PTY のペイン（`split --` と同じ包み = 失敗したときだけ止まる → ログインの層）
            let out = outs.join(format!("pty-{tag}"));
            let command = login_shell_command(hold_on_failure_command(
                probe_cmd(&out),
                "__TAKO_EXIT=",
                Some(&outs.join(format!("exit-{tag}"))),
            ));
            let (session, _rx) = TerminalSession::spawn(
                200,
                30,
                SpawnOptions {
                    command: Some(command.clone()),
                    cwd: Some(root.clone()),
                    env: vec![("HOME".into(), home.display().to_string())],
                    scrollback_lines: None,
                },
            )
            .expect("PTY を起こせる");
            let ran = wait_file(
                &out,
                if legacy {
                    std::time::Duration::from_secs(3)
                } else {
                    budget
                },
            );
            drop(session);
            let got = std::fs::read(&out).unwrap_or_default();
            match (legacy, ran && got == want) {
                (false, false) => failures.push(format!(
                    "SHELL={shell}: PTY のペインがログインの PATH のコマンドを起こせない / 引数が崩れた \
                     argv={command:?} got={:?}",
                    String::from_utf8_lossy(&got)
                )),
                (true, true) => failures.push(format!(
                    "SHELL={shell}: {LEGACY}=1（修正前の `-l -i -c`）でも起きた = 検査が csh の差を見ていない"
                )),
                _ => {}
            }

            // (b) 器の default-shell の `-c`（tmux と #2367 の段が組む形）で、器へ渡す 1 行を解釈させる
            let out = outs.join(format!("container-{tag}"));
            let wrapped = login_shell_command(hold_on_failure_command(
                probe_cmd(&out),
                "__TAKO_EXIT=",
                Some(&outs.join(format!("cexit-{tag}"))),
            ));
            let line = tako_core::backend::inner_command_line(&wrapped);
            let status = std::process::Command::new(shell)
                .args(["-c", &line])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .expect("csh を起こせる");
            let got = std::fs::read(&out).unwrap_or_default();
            match (legacy, status.success() && got == want) {
                (false, false) => failures.push(format!(
                    "SHELL={shell}: 器の 1 行を default-shell が解釈できない / 引数が崩れた \
                     status={status:?} line={line:?} got={:?}",
                    String::from_utf8_lossy(&got)
                )),
                (true, true) => failures.push(format!(
                    "SHELL={shell}: {LEGACY}=1 でも器の 1 行が通った = 検査が csh の差を見ていない"
                )),
                _ => {}
            }
        }
        std::env::remove_var(LEGACY);

        // (c) 終了コードが伝わる（失敗したコマンドの 3 がログインの層を抜けて返る）
        let exit3 = login_shell_command(SpawnCommand {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "tako2597probe /dev/null x; exit 3".into()],
        });
        let status = std::process::Command::new(&exit3.program)
            .args(&exit3.args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("起こせる");
        if status.code() != Some(3) {
            failures.push(format!(
                "SHELL={shell}: 終了コード 3 が伝わらない: {status:?} argv={exit3:?}"
            ));
        }
    }

    // (d) csh 系でないシェルの argv と器の 1 行は修正前の式とバイト一致
    let plain = SpawnCommand {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "echo 'a!b'\nexit 3".into()],
    };
    let posix_line = |c: &SpawnCommand| {
        std::iter::once(&c.program)
            .chain(c.args.iter())
            .map(|w| tako_core::shell::quote_for_shell(w))
            .collect::<Vec<_>>()
            .join(" ")
    };
    for shell in [
        Some("/bin/zsh"),
        Some("/bin/bash"),
        Some("/bin/sh"),
        Some("/bin/dash"),
        Some("/bin/ksh"),
        Some("/opt/homebrew/bin/fish"),
        None,
    ] {
        match shell {
            Some(s) => std::env::set_var("SHELL", s),
            None => std::env::remove_var("SHELL"),
        }
        let got = login_shell_command(plain.clone());
        let program = shell.unwrap_or("/bin/sh");
        let want_args = vec![
            "-l".to_string(),
            "-i".to_string(),
            "-c".to_string(),
            posix_line(&plain),
        ];
        if got.program != program || got.args != want_args {
            failures.push(format!(
                "SHELL={shell:?}: 修正前の `$SHELL -l -i -c <片>` と違う: {got:?}"
            ));
        }
        let line = tako_core::backend::inner_command_line(&got);
        if line != posix_line(&got) {
            failures.push(format!(
                "SHELL={shell:?}: 器の 1 行が修正前の POSIX の引用と違う: {line:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
