//! **#2583 の番犬**: setup の PATH の案内の残り 3 つが修正前の形へ戻らない。
//!
//! 1. **`SHELL` が空・未設定**のとき、setup の PATH 通しは zsh の既定として `~/.zprofile` へ書く（#2550）のに、
//!    ログインシェルの問い合わせ（`exe::user_shell`）だけが `/bin/sh -l`（`~/.profile` を読む）へ落ちていた。
//!    そのため書いた後も「新しいターミナルの PATH に無い」と読まれ、初回起動の導線（welcome）の段・
//!    `tako setup bootstrap status` の `next_step` が `path` のまま残った（修正前の実測。砂場の HOME・
//!    `SHELL` 未設定の CLI と隔離 GUI）。既定は macOS の既定のログインシェル（zsh）へ揃えた
//! 2. **`tako setup bootstrap handoff` の指示文**が、種別の分からないシェルでも「`next_step` が `path` なら
//!    `tako setup bootstrap path` を実行する」と書いていた（そのコマンドは同じ断りで止まる = #2564 / #2576 と同じ
//!    堂々巡り）。いまはその段を、同じ断りの全文を利用者へ伝えて足してもらう依頼にする
//! 3. **check-health の `tako_cli_link`**（設置したリンクの指す先が消えている）が、種別の分からないシェルで
//!    置き場所が PATH に無いときも「tako setup bootstrap path で張り直すこと」と案内していた（張り直した後の
//!    PATH の段で同じ断りに止まる）。いまは起動時の張り直し（tako-app の `repair_tako_cli_link`）で閉じる
//!
//! 外れたら file:line で名指す（直す場所が分からない番犬は直されない）。
//!
//! **本物の環境を書き換えない**: `HOME` / `TAKO_DATA_DIR` / `TAKO_ORCHESTRATOR_DIR` / `CLAUDE_CONFIG_DIR` は
//! 使い捨ての器（`ScratchDir`）の下、`PATH` は空の dir（`TMPDIR` は器の後始末が一時の置き場の下かを確かめるので変えない。
//! 通る経路は一時の置き場へ書かない）。偽の claude は認証の問い合わせに「未ログイン」と答えるだけ。
//! env と表示言語はプロセス全体の状態なので、実経路は `run_alone!` の子で走らせる（#2400）。
//! 1 の実経路は macOS だけ（既定を揃えたのは macOS の既定のログインシェル。Windows の PATH 通しは
//! ユーザー環境変数へ書くので `SHELL` を読まない）。2 の実経路は unix だけ。文と配線は両 OS で見る

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tako_control::setup_bootstrap;
#[cfg(unix)]
use tako_core::platform::agent_install::AgentKind;
use tako_core::source_scan::{fn_head_name, is_top_level_fn_head};

#[path = "common/production_range.rs"]
mod production_range;

const EXE: &str = "crates/tako-core/src/platform/exe.rs";
const CHILD_CMD: &str = "crates/tako-core/src/platform/child_cmd.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const BOOTSTRAP: &str = "crates/tako-control/src/setup_bootstrap.rs";

/// 偽の claude（PATH の外 = `~/.local/bin`）: 認証の問い合わせに「未ログイン」と答えるだけ
#[cfg(unix)]
const FAKE_CLAUDE: &str = "#!/bin/sh\ncase \"$1 $2\" in\n  \"auth status\") echo '{\"loggedIn\":false}'; exit 0;;\nesac\n[ \"$1\" = \"--version\" ] && echo \"2.1.258 (Claude Code)\"\nexit 0\n";

/// 偽のログインシェル: PATH の読み取りにも `command -v` にも何も答えない（名前で種別が決まる）
#[cfg(unix)]
const FAKE_SHELL: &str = "#!/bin/sh\n# #2583 の偽のログインシェル: 何も答えない\nexit 1\n";

/// 同じ断りで止まるコマンド（種別の分からないシェルでここへ誘導したら堂々巡り）
const LOOP_COMMAND: &str = "bootstrap path";

/// 旧挙動へ戻す A/B の env
#[cfg(unix)]
const LEGACY: &str = "TAKO_2583_LEGACY";

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

/// 使い捨ての器（スコープで消える = panic でも残さない。#1312 / #2561）の下に砂場を作り、
/// 本番の設定・データを読み書きしないよう env を向ける。2 つ目は器の正規化したパス
/// （macOS の一時の置き場は /var → /private/var の symlink なので、ログインシェルが返す PATH と比べられる形）
#[cfg(unix)]
fn sandbox(tag: &str) -> (tako_core::test_residue::ScratchDir, PathBuf) {
    // env を書き換えるので `run_alone!` の子の中だけで呼ぶ（#2400）
    tako_core::test_child::assert_alone("sandbox");
    let scratch = tako_core::test_residue::ScratchDir::new(&format!("2583-{tag}"));
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
    std::env::remove_var(LEGACY);
    std::env::set_var("PATH", root.join("empty-path"));
    (scratch, root)
}

#[cfg(unix)]
fn executable(at: &Path, body: &str) {
    std::fs::write(at, body).expect("偽物を書く");
    std::fs::set_permissions(at, std::fs::Permissions::from_mode(0o755)).expect("実行ビット");
}

/// 偽の claude を `~/.local/bin` に置いた新しいホームへ切り替える
#[cfg(unix)]
fn fresh_home(root: &Path, label: &str) -> PathBuf {
    // env（HOME）を書き換えるので `run_alone!` の子の中だけで呼ぶ（#2400）
    tako_core::test_child::assert_alone("fresh_home");
    let home = root.join("homes").join(label);
    std::fs::create_dir_all(home.join(".local/bin")).expect("一時のホーム");
    executable(&home.join(".local/bin/claude"), FAKE_CLAUDE);
    std::env::set_var("HOME", &home);
    home
}

/// 1: 実経路。`SHELL` が空・未設定でも、setup の PATH 通しが書いた profile をログインシェルの問い合わせが読む
#[cfg(target_os = "macos")]
#[test]
fn shellが空か未設定でもsetupが書いたprofileをログインシェルの問い合わせが読む() {
    use tako_control::setup_bootstrap::Step;
    use tako_control::welcome;
    if tako_core::run_alone!(shellが空か未設定でもsetupが書いたprofileをログインシェルの問い合わせが読む)
    {
        return;
    }
    let (_scratch, root) = sandbox("unset");
    let decided_at = || {
        format!(
            "既定は {}（setup の既定 = {}）",
            locate(EXE, "pub fn fallback_login_shell("),
            locate(BOOTSTRAP, "fn shell_kind_for("),
        )
    };
    let mut failures: Vec<String> = Vec::new();
    let mut advanced = 0;
    // (ラベル, SHELL の値（None = 外す）, A/B の旧挙動か)
    let cases: [(&str, Option<&str>, bool); 3] = [
        ("unset", None, false),
        ("empty", Some(""), false),
        ("unset-legacy", None, true),
    ];
    for (label, shell, legacy) in cases {
        let home = fresh_home(&root, label);
        match shell {
            Some(value) => std::env::set_var("SHELL", value),
            None => std::env::remove_var("SHELL"),
        }
        if legacy {
            std::env::set_var(LEGACY, "1");
        } else {
            std::env::remove_var(LEGACY);
        }
        // 確かめは claude を引けたら認証の問い合わせで起こすので、砂場のログインシェルから本物の
        // エージェント CLI を引けないことを先に確かめる（引けたら何も叩かずに止まる）
        let real: Vec<&str> = ["claude", "codex", "agy"]
            .into_iter()
            .filter(|name| tako_core::platform::exe::find(name).is_some())
            .collect();
        assert!(
            real.is_empty(),
            "{label}: 砂場のログインシェルから {real:?} を引けてしまう（確かめが本物を起こしうる）。先へ進まない"
        );
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
        println!(
            "{label}: path={} / status の段={:?} / welcome の段={}",
            path.as_ref().map_or_else(
                |e| format!("断り {e}"),
                |v| format!(
                    "shell={} wrote={} verified={}",
                    v["shell"], v["wrote"], v["verified"]
                )
            ),
            after.step,
            step.kind()
        );
        let wrote_zprofile = matches!(&path, Ok(v) if v["shell"] == "zsh" && v["wrote"] == true)
            && std::fs::read_to_string(home.join(".zprofile"))
                .is_ok_and(|text| text.contains("$HOME/.local/bin"));
        if !wrote_zprofile {
            failures.push(format!(
                "{label}: setup が ~/.zprofile へ書いていない（{}）: {path:?}",
                locate(BOOTSTRAP, "fn shell_kind_for(")
            ));
            continue;
        }
        let seen = matches!(&path, Ok(v) if v["verified"] == true)
            && after.step != Step::Path
            && step.kind() != "path";
        if legacy {
            // A/B: 旧挙動では書いた後も段が path のまま（= この番犬が修正前を見分けられる）
            if seen {
                failures.push(format!(
                    "{label}: {LEGACY}=1 でも段が進んだ（旧挙動へ戻らない = A/B が効いていない。{}）",
                    decided_at()
                ));
            }
        } else if seen {
            advanced += 1;
        } else {
            failures.push(format!(
                "{label}: setup が ~/.zprofile へ書いた後も、ログインシェルの問い合わせがそれを読まない \
                 （verified={} / status の段={:?} / welcome の段={}）= `SHELL` が空・未設定の既定が setup と \
                 食い違っている（#2583。{}）",
                path.as_ref().map(|v| v["verified"].clone()).unwrap_or_default(),
                after.step,
                step.kind(),
                decided_at()
            ));
        }
    }
    std::env::remove_var(LEGACY);
    println!("SHELL が空・未設定で書いた後に段が進んだ: {advanced} / 2");
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// 2: 実経路。種別の分からないシェルの handoff の指示文は同じ断りで止まるコマンドを示さない
#[cfg(unix)]
#[test]
fn 未知のシェルのhandoffの指示文は同じ断りで止まるコマンドを示さない() {
    if tako_core::run_alone!(未知のシェルのhandoffの指示文は同じ断りで止まるコマンドを示さない)
    {
        return;
    }
    let (_scratch, root) = sandbox("handoff");
    let fake = |name: &str| -> String {
        let at = root.join("fakesh").join(name);
        executable(&at, FAKE_SHELL);
        at.display().to_string()
    };
    let decided_at = || {
        format!(
            "判定は {} → {}、文は {}",
            locate(BOOTSTRAP, "pub fn handoff_plan_for_typed("),
            locate(BOOTSTRAP, "pub fn path_by_hand_refusal("),
            locate(BOOTSTRAP, "pub fn handoff_prompt_text_for("),
        )
    };
    let prompt = || -> String {
        setup_bootstrap::handoff_plan_for(AgentKind::Claude, None).expect("引き継ぎの計画")
            ["prompt"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    let mut failures: Vec<String> = Vec::new();

    // ---- 種別の分からないシェル: PATH の段は利用者への依頼（同じ断りの全文つき） ----
    let mut closed = 0;
    for name in ["tcsh", "nu"] {
        fresh_home(&root, name);
        std::env::set_var("SHELL", fake(name));
        let text = prompt();
        let mut problems = Vec::new();
        if text.contains(LOOP_COMMAND) {
            problems.push(format!("`tako setup {LOOP_COMMAND}` へ誘導している"));
        }
        if !text
            .lines()
            .any(|line| line.trim_end() == "   >   $HOME/.local/bin")
        {
            problems.push("足すディレクトリ（$HOME/.local/bin）を伝えていない".to_string());
        }
        if !text.contains("代行しない") {
            problems.push("利用者へ依頼する（代行しない）と書いていない".to_string());
        }
        if problems.is_empty() {
            closed += 1;
        } else {
            failures.push(format!(
                "{name}: {}（#2583。{}）\n{text}",
                problems.join(" / "),
                decided_at()
            ));
        }
        // A/B: 旧挙動では誘導する（= この番犬が修正前を見分けられる）
        std::env::set_var(LEGACY, "1");
        if !prompt().contains("`tako setup bootstrap path` を実行する") {
            failures.push(format!(
                "{name}: {LEGACY}=1 でも誘導しない（旧挙動へ戻らない = A/B が効いていない。{}）",
                decided_at()
            ));
        }
        std::env::remove_var(LEGACY);
    }

    // ---- 既知のシェル・未設定: 修正前と同じ文（純粋関数の既定の口とバイト一致） ----
    let mut kept = 0;
    let known: [(&str, Option<String>); 5] = [
        ("unset", None),
        ("zsh", Some(fake("zsh"))),
        ("bash", Some(fake("bash"))),
        ("sh", Some(fake("sh"))),
        ("fish", Some(fake("fish"))),
    ];
    for (label, shell) in &known {
        fresh_home(&root, &format!("known-{label}"));
        match shell {
            Some(value) => std::env::set_var("SHELL", value),
            None => std::env::remove_var("SHELL"),
        }
        let plan = setup_bootstrap::install_plan_for(AgentKind::Claude).expect("導入計画");
        let want = setup_bootstrap::handoff_prompt_text(&plan, None);
        let text = prompt();
        if text == want && text.contains("`tako setup bootstrap path` を実行する") {
            kept += 1;
        } else {
            failures.push(format!(
                "{label}: 既知のシェルの指示文が修正前と違う（{}）\n--- 得た文\n{text}\n--- 修正前\n{want}",
                decided_at()
            ));
        }
    }
    println!(
        "未知のシェルで誘導しなかった: {closed} / 2・既知のシェルで修正前の文を保った: {kept} / 5"
    );
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// 3: check-health の `tako_cli_link` の文。足すしかないなら同じ断りで止まるコマンドを示さず、そうでなければ修正前とバイト一致
#[test]
fn check_healthのtako_cliリンクの案内は足すしかないとき同じ断りで止まるコマンドを示さない() {
    let at = locate(BOOTSTRAP, "pub fn tako_cli_link_issue(");
    let link = "~/.local/bin/tako";
    let mut failures = Vec::new();
    // 修正前（#1502 からの文）とバイト一致
    let before = "tako CLI のリンク（~/.local/bin/tako）の指す先が消えている。tako.app を移動・削除した場合は \
                  tako setup bootstrap path で張り直すこと（#1502）";
    let got = setup_bootstrap::tako_cli_link_issue(link, false);
    if got != before {
        failures.push(format!(
            "{at}: 足すしかないのでないときの文が修正前と違う: {got}\n  修正前: {before}"
        ));
    }
    let by_hand = setup_bootstrap::tako_cli_link_issue(link, true);
    if by_hand.contains(LOOP_COMMAND) {
        failures.push(format!(
            "{at}: 足すしかない（種別の分からないシェルで置き場所が PATH に無い）のに \
             `tako setup {LOOP_COMMAND}` へ誘導している（#2583。張り直した後の PATH の段で同じ断りに止まる）: {by_hand}"
        ));
    }
    if !by_hand.contains(link) || !by_hand.contains("起動し直す") {
        failures.push(format!(
            "{at}: リンクの場所と、起動し直すと張り直されることを示していない: {by_hand}"
        ));
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

/// `mod imp` の中の関数 1 本（字下げ 4 の頭から、同じ字下げの `}` まで）の本体と頭の行番号
fn nested_fn_body(text: &str, head: &str) -> Option<(usize, Vec<(usize, String)>)> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.starts_with("    ") && line.trim_start().starts_with(head))?;
    let body = lines[start + 1..]
        .iter()
        .enumerate()
        .take_while(|(_, line)| **line != "    }")
        .map(|(i, line)| (start + 2 + i, (*line).to_string()))
        .collect();
    Some((start + 1, body))
}

/// 4: 配線。3 つの口が #2583 の 1 実装（既定・判定・文）を通っている（外れた場所を file:line で）
#[test]
fn setupのpathの案内の残りは1実装を通る() {
    let mut failures = Vec::new();
    // (ファイル, 関数, 在るべき口, 在ってはいけない口)
    let wiring: [(&str, &str, &[&str], &[&str]); 3] = [
        (
            DISPATCH,
            "finish_check_health",
            &["tako_cli_link_issue(", "tako_cli_dir_by_hand("],
            // 案内の文を直書きに戻すと、未知のシェルの判定を通らない
            &["bootstrap path で張り直すこと"],
        ),
        (
            BOOTSTRAP,
            "handoff_plan_for_typed",
            &["handoff_prompt_text_for(", "path_by_hand_refusal("],
            // 既定の口（足すしかないときも bootstrap path を書く）へ戻すと判定を通らない
            &["handoff_prompt_text("],
        ),
        (
            BOOTSTRAP,
            "handoff_prompt_text",
            &["handoff_prompt_text_for("],
            // 文の 2 本目を持たない（既定の口は 1 実装へ委ねるだけ）
            &["next_step"],
        ),
    ];
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
                     同じ断りで止まる `tako setup bootstrap path` へ誘導する形へ戻った（#2583）"
                ));
            }
        }
        for needle in forbid {
            for (line_no, line) in &body {
                if is_code(line) && line.contains(needle) {
                    failures.push(format!(
                        "{rel}:{line_no}: `fn {name}` が 1 実装を通らない形へ戻った（#2583。`{needle}`）: {}",
                        line.trim()
                    ));
                }
            }
        }
    }

    // `SHELL` が空・未設定の既定は exe の 1 実装（setup の既定と揃えた値）。写しを持つと食い違う
    let exe = production(EXE);
    match nested_fn_body(&exe, "pub fn user_shell() -> String") {
        Some((head, body)) => {
            if !body
                .iter()
                .any(|(_, line)| is_code(line) && line.contains("fallback_login_shell()"))
            {
                failures.push(format!(
                    "{EXE}:{head}: unix の `user_shell` が `fallback_login_shell()` を通っていない = `SHELL` が空・未設定の \
                     既定が setup の既定（zsh → .zprofile）と食い違う形へ戻った（#2583）"
                ));
            }
        }
        None => failures.push(format!(
            "{EXE}: unix の `pub fn user_shell` が無い（改名したなら番犬も追うこと）"
        )),
    }
    let child = production(CHILD_CMD);
    for (index, line) in child.lines().enumerate() {
        if is_code(line) && (line.contains("var(\"SHELL\")") || line.contains("\"/bin/sh\".into()"))
        {
            failures.push(format!(
                "{CHILD_CMD}:{}: ログインシェルの解決の写しを持っている = `SHELL` が空・未設定の既定が探索（exe）と \
                 食い違う（#2583。`crate::platform::exe::user_shell()` を通す）: {}",
                index + 1,
                line.trim()
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
