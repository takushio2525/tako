//! **#2577 の番犬**: csh / tcsh のログインシェルへ `-l -c` を渡す形へ戻らない。
//!
//! ## 何が起きていたのか
//!
//! tcsh / csh は `-l` を「唯一の引数」としてしか受け付けない（`tcsh -l -c …` は `Unknown option: '-l'` で
//! rc=1。2026-10-31 実測・macOS 同梱の tcsh 6.21）。ログインシェルへ尋ねる口（`exe::ask_login_shell` =
//! `find` / `find_bounded` / `find_with_timeout` / `login_shell_path` / `bin_resolve`）と、ユーザーの環境で
//! CLI を走らせる子（`child_cmd::user_env_cli` = claude の agents 走査・LSP サーバの起動）が
//! `$SHELL -l -c …` だったので、`SHELL` が tcsh / csh の利用者では毎回失敗していた。修正前の砂場の
//! `tako check-health`: ログインシェルでだけ通る PATH の claude が「未導入（install の段）」、プロセスの
//! PATH にある `/usr/bin/git` まで「見つかりません」（`exe::find` は縮退先を持たない）。
//!
//! いまは起こし方をシェル種別の表（`exe::login_invocation`）の 1 実装で決め、csh / tcsh は argv[0] を
//! `-<名>`（login(1) と同じログインシェルの印）にして `-c 'exec /bin/sh -c $argv[1]:q' <片>` で起こす。
//! csh 系でないシェル（sh / bash / zsh / dash / ksh / fish ほか）の argv は #2577 前と同じ。
//!
//! ## ここで止めるもの
//!
//! 1. [`本物のtcshでログインシェルでだけ通るpathのコマンドを引ける`] — 実経路（unix・`/bin/tcsh` が在るとき）。
//!    砂場の HOME の `.login` でだけ足した PATH のコマンドを、`exe::find_with_timeout` / `exe::login_shell_path` /
//!    `child_cmd::user_env_cli` が引ける（tcsh・csh・symlink の別名）。同じテストの中で
//!    `TAKO_2577_LEGACY=1`（修正前の形）は引けないことも見る
//! 2. [`ログインシェルへlとcを並べるのは表と旧経路だけ`] — 本番コード（4 クレートの `src/`）で `"-l"` と `"-c"` を
//!    並べた argv が、起こし方の表（`exe::login_invocation`）と A/B の旧経路（名前が `_legacy` で終わる関数）の外に
//!    現れたら file:line で名指す（表を通らない口は csh 系の腕を持たない = tcsh で必ず落ちる）
//! 3. [`起こし方の表と配線`] — 表の csh 系の腕が `-l` を持たず `exe::CSH_LOGIN_SCRIPT` と argv[0] を渡すこと、
//!    尋ねる口（`ask_login_shell`）と `ChildCmd` が表を引いて argv[0] を当てること
//! 4. [`修正前の形を名指しできる`] — 2 / 3 へ修正前の形を注入すると file:line で落ちる（同じテストの中の A/B）
//!
//! 対話のペインのシェル（`platform::shell` の `$SHELL -l` / `$SHELL -l -i -c`）はここで見ない
//! （#1945 の理由で別の口。`-l` と `-c` を同じ文に並べていないので 2 にも掛からない）。
//!
//! **本物の環境を書き換えない**: 実経路の `HOME` は使い捨ての器（`ScratchDir`）の下、`PATH` は launchd の既定。
//! env はプロセス全体の状態なので `run_alone!` の子で走らせる（#2400）。`TMPDIR` は変えない

use std::path::{Path, PathBuf};

use tako_core::source_scan::fn_head_name;

#[path = "common/production_range.rs"]
mod production_range;

#[path = "common/byte_window.rs"]
mod byte_window;

const EXE: &str = "crates/tako-core/src/platform/exe.rs";
const CHILD_CMD: &str = "crates/tako-core/src/platform/child_cmd.rs";

/// 走査する範囲（本番コードを持つ 4 クレート全部。`poc/` は品質基準の対象外）
const ROOTS: &[&str] = &[
    "crates/tako-core/src",
    "crates/tako-control/src",
    "crates/tako-cli/src",
    "crates/tako-app/src",
];

/// 起こし方の表（ここだけが `-l -c` の並びを持ってよい）
const TABLE_FN: &str = "login_invocation";

/// 旧挙動へ戻す A/B の env
#[cfg(unix)]
const LEGACY: &str = "TAKO_2577_LEGACY";

/// 砂場の `.login` でだけ PATH に入るコマンド
#[cfg(unix)]
const PROBE: &str = "tako2577probe";

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

/// [`ROOTS`] の `.rs` を全部（相対パスと本番コードの本文）
fn product_files() -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{dir:?} が読める: {e}"));
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|x| x == "rs") {
                out.push(path);
            }
        }
    }
    let root = repo_root();
    let mut paths = Vec::new();
    for dir in ROOTS {
        walk(&root.join(dir), &mut paths);
    }
    paths.sort();
    paths
        .iter()
        .map(|path| {
            let rel = path
                .strip_prefix(&root)
                .expect("リポジトリの中")
                .to_string_lossy()
                .replace('\\', "/");
            let text = production(&rel);
            (rel, text)
        })
        .collect()
}

/// `files` のうち `rel` を `text` へ差し替えたもの（注入）
fn with_override(files: &[(String, String)], rel: &str, text: &str) -> Vec<(String, String)> {
    files
        .iter()
        .map(|(r, t)| {
            if r == rel {
                (r.clone(), text.to_string())
            } else {
                (r.clone(), t.clone())
            }
        })
        .collect()
}

fn line_of(text: &str, at: usize) -> usize {
    text[..at].bytes().filter(|&b| b == b'\n').count() + 1
}

/// 2 の本体: 表と旧経路の外で `"-l"` と `"-c"` を並べた argv の行
fn offenders(files: &[(String, String)]) -> Vec<String> {
    let mut out = Vec::new();
    for (rel, text) in files {
        // 行頭のバイト位置ごとに、いま居る関数の名前（入れ子の `fn` は内側の名前）
        let mut current: Option<String> = None;
        let mut offset = 0usize;
        for line in text.split('\n') {
            if let Some(name) = fn_head_name(line) {
                current = Some(name.to_string());
            }
            let mut hits = Vec::new();
            for (i, _) in line.match_indices("\"-l\"") {
                let window = byte_window::head(&text[offset + i..], 120);
                let window = window.split(';').next().unwrap_or(window);
                if window.contains("\"-c\"") {
                    hits.push(offset + i);
                }
            }
            for pat in ["\"-lc\"", "\"-ilc\"", "\"-lic\""] {
                hits.extend(line.match_indices(pat).map(|(i, _)| offset + i));
            }
            let name = current.as_deref().unwrap_or("?");
            let allowed = (rel == EXE && name == TABLE_FN) || name.ends_with("_legacy");
            if !allowed {
                for at in hits {
                    out.push(format!(
                        "{rel}:{}（fn {name}）がログインシェルへ `-l -c` を直に並べる。\
                         csh / tcsh は `-l` を唯一の引数としてしか受け付けず必ず落ちる（#2577）。\
                         起こし方の表 `tako_core::platform::exe::login_invocation` を通すこと",
                        line_of(text, at)
                    ));
                }
            }
            offset += line.len() + 1;
        }
    }
    out
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

/// 3 の本体: 表の csh 系の腕と、尋ねる口・`ChildCmd` の配線
fn wiring_failures(exe: &str, child_cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    // 表: csh 系の腕は `-l` を持たず、argv[0] と CSH_LOGIN_SCRIPT を渡す
    match fn_body(exe, "pub fn login_invocation(") {
        None => out.push(format!(
            "{EXE}: 起こし方の表 `pub fn login_invocation(` が無い（改名したなら番犬も追うこと）"
        )),
        Some((_, open, body)) => match body.find("LoginForm::Argv0 =>") {
            None => out.push(format!(
                "{EXE}:{}（fn login_invocation）に csh 系の腕 `LoginForm::Argv0 =>` が無い（#2577）",
                line_of(exe, open)
            )),
            Some(at) => {
                let arm = &body[at..];
                let arm_line = line_of(exe, open + at);
                if arm.contains("\"-l\"") {
                    out.push(format!(
                        "{EXE}:{arm_line}（fn login_invocation の csh 系の腕）が `-l` を渡す。\
                         tcsh / csh は `Unknown option: '-l'` で必ず落ちる（#2577）"
                    ));
                }
                for needle in ["arg0: Some(login_arg0_name(shell))", "CSH_LOGIN_SCRIPT.into()"] {
                    if !arm.contains(needle) {
                        out.push(format!(
                            "{EXE}:{arm_line}（fn login_invocation の csh 系の腕）に `{needle}` が無い \
                             （argv[0] でログインシェルにし、`-c` で POSIX の片を /bin/sh へ渡す形。#2577）"
                        ));
                    }
                }
            }
        },
    }
    // 尋ねる口: 表を引き、argv[0] を当てる
    match fn_body(exe, "fn ask_login_shell(") {
        None => out.push(format!(
            "{EXE}: 尋ねる口 `fn ask_login_shell(` が無い（改名したなら番犬も追うこと）"
        )),
        Some((line, _, body)) => {
            for needle in ["login_invocation(shell, script)", "CommandExt::arg0("] {
                if !body.contains(needle) {
                    out.push(format!(
                        "{EXE}:{line}（fn ask_login_shell）に `{needle}` が無い。\
                         起こし方の表を通さないと tcsh / csh の利用者で探索が毎回落ちる（#2577）"
                    ));
                }
            }
        }
    }
    // ユーザーの環境で CLI を走らせる子: 表を引き、計画から argv[0] を当てる
    match fn_body(child_cmd, "fn login_shell_snippet(") {
        None => out.push(format!(
            "{CHILD_CMD}: `fn login_shell_snippet(` が無い（改名したなら番犬も追うこと）"
        )),
        Some((line, _, body)) => {
            if !body.contains("login_invocation(shell, snippet)") {
                out.push(format!(
                    "{CHILD_CMD}:{line}（fn login_shell_snippet）が起こし方の表 `login_invocation` を通さない（#2577）"
                ));
            }
        }
    }
    match fn_body(child_cmd, "pub fn command(&self)") {
        None => out.push(format!(
            "{CHILD_CMD}: `ChildCmd::command` が無い（改名したなら番犬も追うこと）"
        )),
        Some((line, _, body)) => {
            for needle in ["login_arg0(&self.program, &self.args)", "CommandExt::arg0("] {
                if !body.contains(needle) {
                    out.push(format!(
                        "{CHILD_CMD}:{line}（fn command）に `{needle}` が無い。計画は argv[0] の欄を持たないので、\
                         ここで当てないと tcsh / csh が非ログインで起き `~/.login` の PATH を読まない（#2577）"
                    ));
                }
            }
        }
    }
    out
}

/// 2: 本番コードで `-l -c` を並べるのは表と A/B の旧経路だけ
#[test]
fn ログインシェルへlとcを並べるのは表と旧経路だけ() {
    let found = offenders(&product_files());
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// 3: 表の csh 系の腕と配線
#[test]
fn 起こし方の表と配線() {
    let found = wiring_failures(&production(EXE), &production(CHILD_CMD));
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// 4: 修正前の形（表を通さない `-l -c`・表の csh 系の腕の `-l`・argv[0] を当てない）を 2 / 3 が名指す
#[test]
fn 修正前の形を名指しできる() {
    let files = product_files();
    let exe = production(EXE);
    let child_cmd = production(CHILD_CMD);

    // (a) 尋ねる口が表を通さず `-l -c` を直に並べる（#2577 前の ask_login_shell）
    let from = "let invocation = login_invocation(shell, script);";
    assert!(exe.contains(from), "{EXE}: 注入の起点 `{from}` が無い");
    let injected = exe.replacen(
        from,
        "let args = [\"-l\", \"-c\", script];\n    let invocation = login_invocation(shell, script);",
        1,
    );
    let named = offenders(&with_override(&files, EXE, &injected));
    let want = format!(
        "{EXE}:{}（fn ask_login_shell）",
        line_of(
            &injected,
            injected.find("let args = [\"-l\"").expect("注入した行")
        )
    );
    assert!(
        named.iter().any(|m| m.starts_with(&want)),
        "注入した `-l -c` を {want} で名指さない: {named:?}"
    );

    // (b) CLI を走らせる子が表を通さず `-l -c` を直に並べる（#2577 前の login_shell_snippet）
    let from = "let invocation = crate::platform::exe::login_invocation(shell, snippet);";
    assert!(
        child_cmd.contains(from),
        "{CHILD_CMD}: 注入の起点 `{from}` が無い"
    );
    let injected = child_cmd.replacen(
        from,
        "let args = vec![\"-l\".to_string(), \"-c\".to_string(), snippet.to_string()];\n    \
         let invocation = crate::platform::exe::login_invocation(shell, snippet);",
        1,
    );
    let named = offenders(&with_override(&files, CHILD_CMD, &injected));
    assert!(
        named
            .iter()
            .any(|m| m.starts_with(&format!("{CHILD_CMD}:"))
                && m.contains("（fn login_shell_snippet）")),
        "注入した `-l -c` を {CHILD_CMD} の login_shell_snippet で名指さない: {named:?}"
    );
    let unwired = child_cmd.replacen(
        "login_invocation(shell, snippet)",
        "login_invocation_removed(shell, snippet)",
        1,
    );
    let named = wiring_failures(&exe, &unwired);
    assert!(
        named
            .iter()
            .any(|m| m.contains("（fn login_shell_snippet）")),
        "表を通さない login_shell_snippet を名指さない: {named:?}"
    );

    // (c) 表の csh 系の腕が `-l -c` へ戻る（表の中は 2 の対象外なので 3 が受け持つ）
    let from = "args: vec![\"-c\".into(), CSH_LOGIN_SCRIPT.into(), script.to_string()],";
    assert!(exe.contains(from), "{EXE}: 注入の起点 `{from}` が無い");
    let injected = exe.replacen(
        from,
        "args: vec![\"-l\".into(), \"-c\".into(), script.to_string()],",
        1,
    );
    let named = wiring_failures(&injected, &child_cmd);
    assert!(
        named
            .iter()
            .any(|m| m.starts_with(&format!("{EXE}:")) && m.contains("csh 系の腕）が `-l` を渡す")),
        "表の csh 系の腕の `-l` を名指さない: {named:?}"
    );

    // (d) ChildCmd が argv[0] を当てない（非ログインの tcsh = `~/.login` を読まない）
    let from = "login_arg0(&self.program, &self.args)";
    assert!(
        child_cmd.contains(from),
        "{CHILD_CMD}: 注入の起点 `{from}` が無い"
    );
    let injected = child_cmd.replacen(from, "None::<String>.or(None)", 1);
    let named = wiring_failures(&exe, &injected);
    assert!(
        named.iter().any(|m| m.contains("（fn command）")),
        "argv[0] を当てない ChildCmd::command を名指さない: {named:?}"
    );

    // 注入しなければ何も名指さない（同じテストの中の A/B）
    assert!(offenders(&files).is_empty());
    assert!(wiring_failures(&exe, &child_cmd).is_empty());
}

/// 1: 実経路。本物の tcsh で、`.login` でだけ通る PATH のコマンドを探索・PATH の読み取り・CLI の子が引ける
#[cfg(unix)]
#[test]
fn 本物のtcshでログインシェルでだけ通るpathのコマンドを引ける() {
    use std::os::unix::fs::PermissionsExt;
    use tako_core::platform::{child_cmd, exe};

    const TCSH: &str = "/bin/tcsh";
    if !Path::new(TCSH).exists() {
        eprintln!("{TCSH} が無いので実経路は飛ばす（macOS は同梱）");
        return;
    }
    if tako_core::run_alone!(本物のtcshでログインシェルでだけ通るpathのコマンドを引ける)
    {
        return;
    }
    tako_core::test_child::assert_alone("2577 の砂場");
    let scratch = tako_core::test_residue::ScratchDir::new("2577-tcsh");
    // macOS の一時の置き場は /var → /private/var の symlink なので、ログインシェルが返すパスと比べられる形にする
    let root = scratch.path().canonicalize().expect("正規化");
    let home = root.join("home");
    let only = home.join("onlylogin");
    std::fs::create_dir_all(&only).expect("一時の dir");
    let probe = only.join(PROBE);
    std::fs::write(&probe, "#!/bin/sh\nexit 0\n").expect("偽物を書く");
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755)).expect("実行ビット");
    std::fs::write(
        home.join(".login"),
        "setenv PATH \"$HOME/onlylogin:$PATH\"\n",
    )
    .expect(".login を書く");
    // 別名（名前が tcsh でない symlink）
    let alias = root.join("mysh");
    std::os::unix::fs::symlink(TCSH, &alias).expect("symlink");
    std::env::set_var("HOME", &home);
    std::env::set_var("PATH", exe::DEFAULT_LOGIN_PATH);
    std::env::remove_var(LEGACY);
    std::env::remove_var("ZDOTDIR");

    let want = probe.display().to_string();
    let only_dir = only.display().to_string();
    let budget = std::time::Duration::from_secs(20);
    let table = format!(
        "{EXE}:{}（fn login_invocation）",
        production(EXE)
            .lines()
            .position(|l| l.contains("pub fn login_invocation("))
            .map_or(0, |i| i + 1)
    );
    let mut shells = vec![TCSH.to_string(), alias.display().to_string()];
    if Path::new("/bin/csh").exists() {
        shells.push("/bin/csh".to_string());
    }
    let mut failures = Vec::new();
    for shell in &shells {
        std::env::set_var("SHELL", shell);
        let found = exe::find_with_timeout(PROBE, budget);
        if found.path.as_deref() != Some(want.as_str()) {
            failures.push(format!("SHELL={shell}: 探索が引けない: {found:?}"));
        }
        let read = exe::login_shell_path();
        if !read
            .value
            .as_deref()
            .is_some_and(|p| p.split(':').any(|d| d == only_dir))
        {
            failures.push(format!(
                "SHELL={shell}: ログインシェルの PATH に `.login` の分が無い: {read:?}"
            ));
        }
        let plan = child_cmd::user_env_cli(&format!("command -v {PROBE}"), PROBE, &[])
            .expect("unix は計画を組める");
        let out = plan
            .command()
            .stdin(std::process::Stdio::null())
            .output()
            .expect("ログインシェルを起こせる");
        let stdout = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() || stdout.trim() != want {
            failures.push(format!(
                "SHELL={shell}: CLI の子が引けない: {:?} stdout={stdout:?} stderr={:?}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "起こし方は {table}:\n{}",
        failures.join("\n")
    );

    // 同じテストの中の A/B: 修正前の形（`-l -c`）は tcsh で必ず落ちて引けない
    std::env::set_var(LEGACY, "1");
    std::env::set_var("SHELL", TCSH);
    let found = exe::find_with_timeout(PROBE, budget);
    assert_eq!(
        found.path, None,
        "A/B の旧挙動で引けてしまった（再現にならない）"
    );
    assert_eq!(
        exe::login_shell_path().value,
        None,
        "A/B の旧挙動で PATH を読めてしまった"
    );
    let plan = child_cmd::user_env_cli(&format!("command -v {PROBE}"), PROBE, &[]).expect("計画");
    let out = plan
        .command()
        .stdin(std::process::Stdio::null())
        .output()
        .expect("起こせる");
    assert!(
        !out.status.success() && String::from_utf8_lossy(&out.stderr).contains("-l"),
        "A/B の旧挙動の tcsh が `-l` を断らない: {:?} {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    std::env::remove_var(LEGACY);
}
