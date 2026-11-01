//! **#2576 の番犬**: 種別の分からないシェル（tcsh・nu 等）で、初回起動の導線（welcome）の「次にやること」と
//! check-health の `cli_in_path` が `tako setup bootstrap path` へ誘導する形へ戻らない。
//!
//! そのコマンドは同じ断り（`SetupRefusal::ShellProfileUnknown`）で止まる（堂々巡り。`tako setup` の側は #2564 で
//! 止めた）。修正前（実測。砂場の隔離 GUI へ CLI `tako welcome` / `tako check-health --json` と MCP の
//! `tako_welcome` / `tako_check_health`）: welcome の `step.command` が `tako setup bootstrap path`、
//! check-health の `cli_in_path` が「外部ターミナルでも使いたい場合は tako setup bootstrap path を実行すること」。
//!
//! 見るもの:
//!
//! 1. 実経路（`welcome::probe` = dispatch の offload が呼ぶのと同じ確かめ）: 砂場のホームの PATH の外に claude を
//!    置き、`SHELL` を偽の tcsh / nu にすると、段は `path` のまま打つコマンドを持たず（`command: null`）、
//!    足すディレクトリの一覧（`$HOME/.local/bin`）と断りの全文（表示言語）を持つ。ボタンは自動の段（確かめ直し）で、
//!    同じ断りで止まるコマンドを打たない。既知のシェル（zsh / bash / sh / fish）は従来どおり
//!    `tako setup bootstrap path`
//! 2. check-health の文（`setup_bootstrap::tako_cli_in_path_issue`）と、その材料の判定
//!    （`setup_bootstrap::tako_cli_dir_by_hand_for` = `tako setup --check` の行と同じ 1 実装）: 足すしかないなら
//!    足す置き場所で閉じ、そうでなければ修正前とバイト一致
//! 3. 配線: `welcome::next_step` と dispatch の `finish_check_health` が上の 1 実装を通っている
//!
//! 外れたら file:line で名指す（直す場所が分からない番犬は直されない）。
//!
//! **本物の環境を書き換えない**: `HOME` / `TAKO_DATA_DIR` / `TAKO_ORCHESTRATOR_DIR` / `CLAUDE_CONFIG_DIR` /
//! `TMPDIR` は一時の置き場の下、`PATH` は空の dir、`SHELL` は何も答えずに終わる偽のログインシェル（名前で種別が
//! 決まる）。偽の claude は認証の問い合わせに「未ログイン」と答えるだけ。env と表示言語はプロセス全体の状態なので、
//! 実経路は `run_alone!` の子で走らせる（#2400）。実経路（1）と判定（2'）は unix だけ: Windows の PATH 通しは
//! ユーザー環境変数へ書くので当たらない（Windows で当たらないことは `welcome` の単体テストが Windows の CI で見る）。
//! 文（2）と配線（3）は両 OS で見る

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use serde_json::json;
use tako_control::setup_bootstrap;
#[cfg(unix)]
use tako_control::welcome::{self, Launch, NextStep};
#[cfg(unix)]
use tako_core::i18n::{self, Lang};
#[cfg(unix)]
use tako_core::platform::agent_install::AgentKind;
#[cfg(unix)]
use tako_core::platform::shell_dialect::ShellDialect;
use tako_core::source_scan::{fn_head_name, is_top_level_fn_head};

#[path = "common/production_range.rs"]
mod production_range;

const WELCOME: &str = "crates/tako-control/src/welcome.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const BOOTSTRAP: &str = "crates/tako-control/src/setup_bootstrap.rs";

/// 偽のログインシェル: PATH の読み取りにも `command -v` にも何も答えない
#[cfg(unix)]
const FAKE_SHELL: &str = "#!/bin/sh\n# #2576 の偽のログインシェル: 何も答えない\nexit 1\n";

/// 偽の claude（PATH の外 = `~/.local/bin`）: 認証の問い合わせに「未ログイン」と答えるだけ
#[cfg(unix)]
const FAKE_CLAUDE: &str = "#!/bin/sh\ncase \"$1 $2\" in\n  \"auth status\") echo '{\"loggedIn\":false}'; exit 0;;\nesac\n[ \"$1\" = \"--version\" ] && echo \"2.1.258 (Claude Code)\"\nexit 0\n";

/// 同じ断りで止まるコマンド（ここへ誘導したら堂々巡り）
const LOOP_COMMAND: &str = "bootstrap path";

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

/// コメントでない行だけ
fn is_code(line: &str) -> bool {
    let trimmed = line.trim_start();
    !trimmed.is_empty() && !trimmed.starts_with("//")
}

#[cfg(unix)]
fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

/// 1: 実経路。未知のシェルの welcome の「次にやること」は同じ断りで止まるコマンドを示さない
#[cfg(unix)]
#[test]
fn 未知のシェルのwelcomeは同じ断りで止まるコマンドを示さない() {
    if tako_core::run_alone!(未知のシェルのwelcomeは同じ断りで止まるコマンドを示さない)
    {
        return;
    }
    // 消してよい範囲（macOS の一時の置き場は /var → /private/var の symlink なので正規化して比べる）
    let base = std::env::temp_dir()
        .canonicalize()
        .expect("一時の置き場の正規化");
    let root = base.join(format!("tako-2576-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("一時の置き場");
    let root = root.canonicalize().expect("正規化");
    for dir in [
        "home/.local/bin",
        "data",
        "orch",
        "ccd",
        "tmp",
        "empty-path",
        "fakesh",
    ] {
        std::fs::create_dir_all(root.join(dir)).expect("一時の dir");
    }
    // 本番の設定・データを読み書きしない（どの置き場も一時の場所の下）
    std::env::set_var("HOME", root.join("home"));
    std::env::set_var("TAKO_DATA_DIR", root.join("data"));
    std::env::set_var("TAKO_ORCHESTRATOR_DIR", root.join("orch"));
    std::env::set_var("CLAUDE_CONFIG_DIR", root.join("ccd"));
    std::env::set_var("TMPDIR", root.join("tmp"));
    std::env::remove_var("CODEX_HOME");
    std::env::remove_var("TAKO_SELF_TEST");
    std::env::set_var("PATH", root.join("empty-path"));
    let executable = |at: &Path, body: &str| {
        std::fs::write(at, body).expect("偽物を書く");
        std::fs::set_permissions(at, std::fs::Permissions::from_mode(0o755)).expect("実行ビット");
    };
    executable(&root.join("home/.local/bin/claude"), FAKE_CLAUDE);
    let fake = |name: &str| -> String {
        let at = root.join("fakesh").join(name);
        executable(&at, FAKE_SHELL);
        at.display().to_string()
    };
    // 確かめは claude を引けたら認証の問い合わせで起こすので、偽のシェルの下で本物を引けないことを先に確かめる
    // （引けたら何も叩かずに止まる。#2550 の実経路テストと同じ門）
    std::env::set_var("SHELL", fake("tcsh"));
    assert!(
        tako_core::platform::exe::find("claude").is_none(),
        "claude を引けてしまう（注入が効いていない）ので確かめが本物を起こしうる。先へ進まない"
    );
    let original = i18n::lang();
    let mut failures: Vec<String> = Vec::new();
    let decided_at = || {
        format!(
            "判定は {} → {}、写しは {}",
            locate(WELCOME, "fn next_step("),
            locate(BOOTSTRAP, "pub fn path_by_hand_refusal_for("),
            locate(WELCOME, "pub fn command("),
        )
    };

    // ---- 種別の分からないシェル: 段は path のまま、打つコマンドを持たず、足すディレクトリの一覧で閉じる ----
    let mut closed = 0;
    for name in ["tcsh", "nu"] {
        std::env::set_var("SHELL", fake(name));
        for lang in [Lang::Ja, Lang::En] {
            i18n::set_lang(lang);
            let step = welcome::probe();
            let value = step.to_json();
            let label = format!("{name} / {lang:?}");
            println!("{label}: {value}");
            let mut problems = Vec::new();
            if step.kind() != "path" || step.agent() != Some(AgentKind::Claude) {
                problems.push(format!("段が path（claude）でない: {step:?}"));
            }
            if step.command().is_some() || !value["command"].is_null() {
                problems.push(format!("打つコマンドを示している: {}", value["command"]));
            }
            if value.to_string().contains(LOOP_COMMAND) {
                problems.push(format!("`tako setup {LOOP_COMMAND}` へ誘導している"));
            }
            let dirs: Vec<&str> = value["dirs"]
                .as_array()
                .map(|a| a.iter().filter_map(|d| d.as_str()).collect())
                .unwrap_or_default();
            if !dirs.contains(&"$HOME/.local/bin") {
                problems.push(format!(
                    "足すディレクトリ（$HOME/.local/bin）が無い: {}",
                    value["dirs"]
                ));
            }
            let note = value["note"].as_str().unwrap_or_default();
            if !note.lines().any(|line| line == "  $HOME/.local/bin") {
                problems.push(format!(
                    "断りの全文（足すディレクトリの一覧）が無い: {note:?}"
                ));
            }
            if lang == Lang::En {
                if let Some(c) = has_japanese(note) {
                    problems.push(format!("英語表示の断りに日本語（{c}）: {note:?}"));
                }
            }
            match welcome::launch_for(&step, ShellDialect::Posix) {
                Launch::Auto => {}
                other => problems.push(format!("ボタンが確かめ直し（自動の段）でない: {other:?}")),
            }
            if problems.is_empty() {
                closed += 1;
            } else {
                failures.push(format!(
                    "{label}: 同じ断りで止まるコマンドへ誘導する形へ戻った（#2576。{}）: {}",
                    decided_at(),
                    problems.join(" / ")
                ));
            }
        }
    }

    // ---- 既知のシェル: 従来どおり `tako setup bootstrap path`（そのコマンドが書ける）----
    i18n::set_lang(Lang::Ja);
    let mut kept = 0;
    for name in ["zsh", "bash", "sh", "fish"] {
        std::env::set_var("SHELL", fake(name));
        let step = welcome::probe();
        if step
            == (NextStep::Path {
                agent: AgentKind::Claude,
            })
            && step.command().as_deref() == Some("tako setup bootstrap path")
            && step.to_json()
                == json!({ "kind": "path", "agent": "claude", "command": "tako setup bootstrap path" })
        {
            kept += 1;
        } else {
            failures.push(format!(
                "{name}: 既知のシェルの段が修正前と違う（{}）: {}",
                decided_at(),
                step.to_json()
            ));
        }
    }

    i18n::set_lang(original);
    assert!(
        root.starts_with(&base),
        "一時の置き場の外を消そうとした: {}",
        root.display()
    );
    let _ = std::fs::remove_dir_all(&root);
    println!("種別の分からないシェルで一覧で閉じた: {closed} / 4（tcsh / nu × 日英）");
    println!("既知のシェルで従来どおり: {kept} / 4（zsh / bash / sh / fish）");
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// 2: check-health の `cli_in_path` の文。足すしかないなら同じ断りで止まるコマンドを示さず足す置き場所で閉じ、
/// そうでなければ修正前とバイト一致（check-health の診断は日本語固定 = #1505）
#[test]
fn check_healthのcli_in_pathは足すしかないなら置き場所で閉じる() {
    let at = locate(BOOTSTRAP, "pub fn tako_cli_in_path_issue(");
    let injected = Path::new("/Applications/tako.app/Contents/MacOS");
    let mut failures = Vec::new();
    for (inj, want_level) in [(Some(injected), "info"), (None, "error")] {
        let (level, message) = setup_bootstrap::tako_cli_in_path_issue(
            inj,
            Some("$HOME/.local/bin".to_string()),
            tako_core::i18n::Lang::Ja,
        );
        if level != want_level
            || message.contains(LOOP_COMMAND)
            || !message.contains("$HOME/.local/bin をそのシェルの書き方で PATH へ足してください")
        {
            failures.push(format!(
                "{at}: 足すしかないときの {want_level} が同じ断りで止まるコマンドへ誘導する / 置き場所で閉じない（#2576）: \
                 ({level}) {message}"
            ));
        }
    }
    // 足すしかないのでなければ、修正前の文とバイト一致（既知のシェル・Windows・PATH に在ってリンクだけ無い）
    let before = [
        (
            Some(injected),
            "info",
            format!(
                "tako CLI は PATH に無いが、tako が開くシェルには {} を自動で追加するので \
                 tako の中では `tako` が使える（#601）。外部ターミナルでも使いたい場合は \
                 tako setup bootstrap path を実行すること（#1502）",
                injected.display()
            ),
        ),
        (
            None,
            "error",
            "tako CLI が PATH に見つからない。tako setup bootstrap path で設置するか、\
             scripts/build-app.sh --install でインストールすること"
                .to_string(),
        ),
    ];
    for (inj, want_level, want) in before {
        let got = setup_bootstrap::tako_cli_in_path_issue(inj, None, tako_core::i18n::Lang::Ja);
        if got != (want_level, want.clone()) {
            failures.push(format!(
                "{at}: 足すしかないのでないときの文が修正前と違う: {got:?}\n  修正前: ({want_level}) {want}"
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// 2'. check-health と `tako setup --check` が引く判定（tako CLI の置き場所を自分で足すしかないか）
#[cfg(unix)]
#[test]
fn tako_cliの置き場所を自分で足すしかないのは未知のシェルで置き場所がpathに無いときだけ() {
    let at = locate(BOOTSTRAP, "pub fn tako_cli_dir_by_hand_for(");
    let home = Path::new("/home/testuser");
    let state = |resolved: bool, on_path: bool| {
        json!({
            "resolved": resolved,
            "dir": "/home/testuser/.local/bin",
            "dir_display": "~/.local/bin",
            "dir_on_path": on_path,
        })
    };
    let cases = [
        // (状態, 未知のシェル, ホーム, 期待)
        (
            state(true, false),
            true,
            Some(home),
            Some("$HOME/.local/bin"),
        ),
        // ホームを解決できなければ表示用のパスで示す（--check の行の従来どおり）
        (state(true, false), true, None, Some("~/.local/bin")),
        // PATH に在る（リンクだけ無い / 切れている）なら bootstrap path は通る
        (state(true, true), true, Some(home), None),
        // 既知のシェル・Windows（ユーザー環境変数へ書く）
        (state(true, false), false, Some(home), None),
        // 置き場所を解決できない（実行中のバイナリの隣に tako が無い）
        (state(false, false), true, Some(home), None),
    ];
    let mut failures = Vec::new();
    for (value, by_hand, home, want) in cases {
        let got = setup_bootstrap::tako_cli_dir_by_hand_for(&value, by_hand, home);
        if got.as_deref() != want {
            failures.push(format!(
                "{at}: {value} / 未知のシェル={by_hand} / ホーム={home:?}: {got:?}（期待 {want:?}）"
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// トップレベル関数 1 本の本体（行番号つき）と頭の行番号
fn fn_body(text: &str, name: &str) -> Option<(usize, Vec<(usize, String)>)> {
    let mut head = None;
    let mut out = Vec::new();
    let mut inside = false;
    for (index, line) in text.lines().enumerate() {
        if is_top_level_fn_head(line) {
            inside = fn_head_name(line) == Some(name);
            if inside {
                head = Some(index + 1);
            }
            continue;
        }
        if inside {
            out.push((index + 1, line.to_string()));
        }
    }
    head.map(|head| (head, out))
}

/// 3: 配線。welcome と check-health が #2564 の判定・案内の 1 実装を通っている（外れた場所を file:line で）
#[test]
fn welcomeとcheck_healthは未知のシェルの判定と案内の1実装を通る() {
    // (ファイル, 関数, 在るべき口, 在ってはいけない口)
    let wiring: [(&str, &str, &[&str], &[&str]); 5] = [
        (WELCOME, "next_step", &["path_by_hand_refusal_for"], &[]),
        (WELCOME, "next_step_with", &["NextStep::PathByHand"], &[]),
        (
            DISPATCH,
            "finish_check_health",
            &["tako_cli_in_path_issue(", "tako_cli_dir_by_hand("],
            // 案内の文を直書きに戻すと、未知のシェルの判定を通らない
            &[
                "bootstrap path を実行すること",
                "bootstrap path で設置するか",
            ],
        ),
        (
            BOOTSTRAP,
            "tako_cli_in_path_issue",
            &["PathByHandNote::CliCheckHint"],
            &[],
        ),
        (
            BOOTSTRAP,
            "tako_cli_dir_by_hand",
            &["path_by_hand_for("],
            &[],
        ),
    ];
    let mut failures = Vec::new();
    for (rel, name, want, forbid) in wiring {
        let text = production(rel);
        let Some((head, body)) = fn_body(&text, name) else {
            failures.push(format!(
                "{rel}: `fn {name}` が無い（改名したなら番犬も追うこと）"
            ));
            continue;
        };
        for needle in want {
            if !body
                .iter()
                .any(|(_, line)| is_code(line) && line.contains(needle))
            {
                failures.push(format!(
                    "{rel}:{head}: `fn {name}` が `{needle}` を通っていない = 種別の分からないシェルで \
                     `tako setup bootstrap path` へ誘導する形へ戻った（#2576。そのコマンドも同じ断りで止まる）"
                ));
            }
        }
        for needle in forbid {
            for (line_no, line) in &body {
                if is_code(line) && line.contains(needle) {
                    failures.push(format!(
                        "{rel}:{line_no}: `fn {name}` が案内の文を直書きしている（#2576。未知のシェルの判定を\
                         通らない = `tako_cli_in_path_issue` へ寄せる）: {}",
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
