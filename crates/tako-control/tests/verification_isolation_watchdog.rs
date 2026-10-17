//! 番犬: **検証プロセスは本番の個人ファイルへ書かない**（Issue #1253 / #944 / #1030）
//!
//! #944 は「置き場を決める側を倒す」で `cargo test` を塞いだが、判定の一部が
//! `cfg(test)` のままだった。GUI セルフテストは `cargo test` ではなく**製品バイナリ**が
//! `TAKO_SELF_TEST=1` で立つので、そこには 1 ビットも効かない —— 実測では
//! `~/.claude.json` のテスト残骸 3,050 件のうち約 70% がこの経路だった。
//!
//! したがって検査するのは「書かないこと」だけでなく**判定の形**そのもの:
//! 書き先を決める関数が `cfg(test)` ではなく実行時判定
//! （`tako_core::paths::is_verification_process`）を引いていること。
//! 挙動側の番犬は `tako_control::test_write_isolation`（空 HOME で子を回す）と
//! `tako-app` の `update_checker`（偽 brew を PATH へ置く）にある。
//!
//! **#1970**: 検証は本番 tako のペインから起こされるので、本番の env（制御プレーンと mod）を
//! 受け継ぐ。隔離 GUI の入口（`scripts/lib/isolated-gui.sh`）がそれを GUI へ渡さず・自動リネームを
//! 止め・`CLAUDE_CONFIG_DIR` を一時の側へ向けること（本番がペインへ配る名前は本番側の正本から
//! 集めて突き合わせる）と、検証スクリプトが自分のプロセスで claude / tako を起こす前に
//! 落としていること（`rel:line` で名指す）を拘束する。

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルートを解決できない")
        .to_path_buf()
}

/// `fn <名前>(` から対応する `}` までの本体を返す（波括弧の対応で切る）
fn fn_body<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let at = text.find(&format!("fn {name}("))?;
    let open = text[at..].find('{')? + at;
    let mut depth = 0usize;
    for (i, c) in text[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[open..open + i + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

#[path = "common/code_view.rs"]
mod code_view;

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

/// **肯定の存在確認**（「この呼び出しが在る」）が見る眺め = コメントを落とした本文。
///
/// 全文へ `contains` すると、走査先の doc コメントに書いた同じ綴りで真になり、
/// 実体が消えても緑のままになる（#1609）。`.zshenv` などの非 Rust は全文のまま
fn read_code(rel: &str) -> String {
    code_view::without_comments_checked(&read(rel), rel)
}

/// 外部エージェントの設定の書き先は**実行時判定**で倒す。
///
/// `cfg(test)` だと GUI セルフテスト（製品バイナリ）が素通りする = #1253 の本体
#[test]
fn 設定の書き先は実行時判定で隔離される() {
    for (rel, func) in [
        (
            "crates/tako-control/src/orchestrator/mod.rs",
            "agent_config_home",
        ),
        ("crates/tako-control/src/claude_tui.rs", "env_config_dir"),
    ] {
        let text = read_code(rel);
        let body =
            fn_body(&text, func).unwrap_or_else(|| panic!("{rel}: fn {func} が見つからない"));
        assert!(
            body.contains("is_verification_process"),
            "{rel}: {func} が実行時判定（paths::is_verification_process）を引いていない（#1253）"
        );
        assert!(
            !body.contains("cfg(test)"),
            "{rel}: {func} が cfg(test) で隔離している。\
             GUI セルフテストは製品バイナリなので効かない（#1253）"
        );
    }
}

/// テストプロセスから brew を起こさない（#1253。#944 の `claude agents --json` と同型）。
///
/// `cargo test --workspace` がユーザーの `~/Library/Caches/Homebrew` へ
/// 698 ファイル作っていた（空 HOME で実測）
#[test]
fn 配布系統の判別はテストプロセスで外部コマンドを起こさない() {
    let rel = "crates/tako-app/src/update_checker.rs";
    let text = read_code(rel);
    let body = fn_body(&text, "detect_install_method_full").expect("判別関数が見つからない");
    let guard = body.find("is_test_process").unwrap_or_else(|| {
        panic!("{rel}: detect_install_method_full にテストプロセスの門番が無い（#1253）")
    });
    let probe = body.find("is_brew_available").unwrap_or_else(|| {
        panic!("{rel}: brew の実行が見当たらない（実装が変わったら番犬も直す）")
    });
    assert!(
        guard < probe,
        "{rel}: brew を起こした後で門番を通している（順番が逆だと外部コマンドが走る。#1253）"
    );
}

/// 対話 zsh の履歴は **rc の後に**当て直す（#1253）。
///
/// macOS の `/etc/zshrc` は `HISTFILE=${ZDOTDIR:-$HOME}/.zsh_history` を無条件で
/// 代入するので、環境変数を渡すだけでは対話シェルに効かない（実測で確認した）
#[test]
fn 検証の履歴はrcの後に当て直される() {
    let rel = "crates/tako-core/shell-integration/zshenv.zsh";
    // zsh は Rust ではないので眺めを通さない（`#` コメントは code_view の対象外）
    let text = read(rel);
    assert!(
        text.contains("TAKO_VERIFY_HISTFILE"),
        "{rel}: 検証用の履歴の書き先を当て直す仕掛けが無い（#1253）"
    );
    assert!(
        text.contains("precmd_functions+=(_tako_verify_histfile)"),
        "{rel}: 当て直しが precmd に載っていない。\
         .zshenv での代入は /etc/zshrc に潰される（#1253）"
    );
    // ペイン ID で絞ると、履歴を汚していた素の PTY（TAKO_PANE_ID を持たない）が漏れる
    let at = text
        .find("TAKO_VERIFY_HISTFILE-")
        .expect("ガード行が見つからない");
    let guard_line = text[..at].rsplit('\n').next().unwrap_or("");
    assert!(
        !guard_line.contains("TAKO_PANE_ID"),
        "{rel}: 当て直しを TAKO_PANE_ID で絞っている。\
         履歴を汚していたのはペイン ID を持たない素の PTY（#1253）"
    );
}

/// エージェント CLI の**問い合わせ起動**は正本の門番（`agent_probe::run`）を通る（#1261）。
///
/// tako 側が 1 バイトも書かなくても、実 CLI は起動しただけで自分のホームを作る
/// （実測: 空 HOME で `cargo test --workspace` → `~/.gemini` 32 ファイル /
/// `~/.codex/tmp/arg0/…/.lock` / `~/.claude.json` と `~/.claude/backups/`）。
/// 呼ぶ側それぞれに `if` を散らすと**新しい問い合わせを足したときに素通りする**ので、
/// 起動は 1 か所を通す形そのものを拘束する
#[test]
fn エージェントcliの問い合わせ起動は正本の門番を通る() {
    for (rel, func) in [
        // #1505 で認証とプランの問い合わせが 1 本になった（`is_authenticated_for` は
        // ここへ委譲するだけ）ので、門番を通る位置もこちら
        (
            "crates/tako-control/src/setup_bootstrap.rs",
            "auth_state_for",
        ),
        // 環境診断の経路の上限つき版（#2255。上限を引数に取る `agent_probe::output_within` を通る）
        (
            "crates/tako-control/src/setup_bootstrap.rs",
            "auth_state_within",
        ),
        // claude の MCP 登録の読み取り（#1505 で CLI から移った）。
        // こちらは上限つきの口（`agent_probe::output_with_timeout`）を通る
        // （`claude_health_with` / `claude_health_now` はここへ委譲するだけ）
        (
            "crates/tako-control/src/agent_mcp.rs",
            "claude_health_probe",
        ),
        ("crates/tako-control/src/agent_models.rs", "catalog"),
        (
            "crates/tako-control/src/stale_binary.rs",
            "extract_version_via_cli",
        ),
    ] {
        let text = read_code(rel);
        let body =
            fn_body(&text, func).unwrap_or_else(|| panic!("{rel}: fn {func} が見つからない"));
        assert!(
            body.contains("agent_probe::run")
                || body.contains("agent_probe::output_with_timeout")
                || body.contains("agent_probe::output_within"),
            "{rel}: {func} が正本の門番（agent_probe::run / output_with_timeout / output_within）を\
             通っていない。テストプロセスから実 claude / codex / agy が起きる（#1261）"
        );
        assert!(
            !body.contains(".output()"),
            "{rel}: {func} が子プロセスを直に起こしている。\
             起動は agent_probe::run へ寄せる（#1261）"
        );
    }
}

/// 門番自身は**実行時**にテストプロセスを見分け、判定してから起こす（#1261）。
///
/// `cfg(test)` だと統合テスト（`crates/*/tests/`）から呼ばれた lib は非テストビルドなので
/// 素通りする —— #1253 が GUI セルフテストで踏んだのと同じ穴
#[test]
fn 問い合わせの門番はテストプロセスを実行時に見分ける() {
    let rel = "crates/tako-control/src/agent_probe.rs";
    let text = read_code(rel);
    let gate = fn_body(&text, "blocked").expect("fn blocked が見つからない");
    assert!(
        gate.contains("is_test_process"),
        "{rel}: blocked が実行時判定（paths::is_test_process）を引いていない（#1261）"
    );
    assert!(
        !gate.contains("cfg(test)"),
        "{rel}: blocked が cfg(test) で判定している。\
         クレートを跨がないので統合テストが素通りする（#1261）"
    );
    let body = fn_body(&text, "run").expect("fn run が見つからない");
    // 呼ぶ側から `no_console_window` を取り上げた以上、内側で必ず当てる（#586 / #628）。
    // `platform_parity::コンソール窓を抑止していない子プロセス起動が増えていない` は
    // `agent_probe::run` を抑止済みとして数えるので、その前提をここで押さえる
    assert!(
        body.contains("no_console_window"),
        "{rel}: run が #586 のコンソール窓抑止を当てていない（#1261）"
    );
    let guard = body.find("blocked()").expect("run が門番を呼んでいない");
    let launch = body.find(".output()").expect("run が起動していない");
    assert!(
        guard < launch,
        "{rel}: 起こした後で門番を通している（順番が逆だと外部コマンドが走る。#1261）"
    );
}

// ------------------------------------------------------------------------------------------
// #1970: 隔離 GUI と検証スクリプトは本番 tako の env を持ち込まない
//
// 検証は本番 tako のペイン（= worker の claude の中）から起こされるので、何もしなければ
// 本番の制御プレーン（TAKO_SOCKET / TAKO_TOKEN / TAKO_PANE_ID …）と本番の mod
// （CLAUDE_CODE_PLUGIN_DIRS / TAKO_CLI）を丸ごと受け継ぐ。#1966 の worker は隔離 GUI の
// 自動リネームが本物の claude を起こし、本番の mod が一時の claude に載る形を踏んだ。
// 拘束するのは 2 か所: 隔離 GUI の入口（scripts/lib/isolated-gui.sh）が GUI へ渡さないこと、
// 検証スクリプトが自分のプロセスで claude / tako を起こす前に落としていること。
// ------------------------------------------------------------------------------------------

const ISOLATED_GUI_REL: &str = "scripts/lib/isolated-gui.sh";

/// Issue #1970 が列挙した「本番 tako が配る env」。ヘルパの一覧の下限で、検証スクリプトが
/// **claude** を起こす前に落としていなければならない組でもある
const ISSUE_1970_KEYS: &[&str] = &[
    "TAKO_SOCKET",
    "TAKO_PANE_ID",
    "TAKO_TAB_ID",
    "TAKO_TOKEN",
    "TAKO_CLI",
    "TAKO_MCP_URL",
    "CLAUDE_CODE_PLUGIN_DIRS",
    "TAKO_ORCHESTRATOR_ROLE",
];

/// 検証スクリプトが **tako CLI** を起こす前に落としていなければならない組。
/// AGENTS.md の `env -u TAKO_SOCKET -u TAKO_PANE_ID` と同じ（ソケットが無ければトークンは効かない）。
/// 残すと CLI は本番の GUI へ繋がり、本番のペインを自分として扱う（#1449 / #1450）
const TAKO_CLI_KEYS: &[&str] = &["TAKO_SOCKET", "TAKO_PANE_ID"];

/// ヘルパの `ISOLATED_GUI_DROP_ENV=( … )` に並んだ名前（コメント行と行末コメントは除く）
fn helper_drop_env(src: &str) -> Vec<String> {
    let start = src
        .find("\nISOLATED_GUI_DROP_ENV=(")
        .unwrap_or_else(|| panic!("{ISOLATED_GUI_REL}: ISOLATED_GUI_DROP_ENV=( が消えている"));
    let body = &src[start..];
    let open = body.find('(').expect("開き括弧") + 1;
    let close = body.find("\n)").unwrap_or_else(|| {
        panic!("{ISOLATED_GUI_REL}: ISOLATED_GUI_DROP_ENV の閉じ括弧（行頭の `)`）が見つからない")
    });
    let names: Vec<String> = body[open..close]
        .lines()
        .map(|l| l.split('#').next().unwrap_or(""))
        .flat_map(str::split_whitespace)
        .map(str::to_string)
        .collect();
    assert!(
        names.len() >= ISSUE_1970_KEYS.len(),
        "{ISOLATED_GUI_REL}: ISOLATED_GUI_DROP_ENV が {} 個しか読めない（範囲取りが壊れている）",
        names.len()
    );
    names
}

/// 本番の tako がペインへ配る env の名前（**本番側の正本から集める**。増やした名前を
/// ヘルパへ足し忘れたらここで落ちる）
fn production_pane_env() -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    // 制御プレーン: tako-app の spawn_session が `("TAKO_…".into(), …)` で積む名前
    let rel = "crates/tako-app/src/main.rs";
    let text = read_code(rel);
    let body = fn_body(&text, "spawn_session")
        .unwrap_or_else(|| panic!("{rel}: fn spawn_session が見つからない"));
    let mut rest = body;
    while let Some(at) = rest.find("(\"TAKO_") {
        let tail = &rest[at + 2..];
        let end = tail.find('"').expect("閉じ引用符");
        if tail[end..].starts_with("\".into()") {
            keys.push(tail[..end].to_string());
        }
        rest = &tail[end..];
    }
    for want in ["TAKO_SOCKET", "TAKO_TOKEN", "TAKO_MCP_URL", "TAKO_PANE_ID"] {
        assert!(
            keys.iter().any(|k| k == want),
            "{rel}: spawn_session から {want} を読めない（積み方が変わったら番犬も直す）。読めた名前: {keys:?}"
        );
    }
    // 器の中のシェルへ固定する名前・tako mod・シェル統合の控え（tako 固有のものだけ）
    keys.extend(
        tako_core::backend::PANE_SCOPED_ENV
            .iter()
            .chain(tako_core::claude_mod::INJECTED_KEYS)
            .map(|k| k.to_string()),
    );
    keys.extend(
        tako_core::shell_integration::INJECTED_KEYS
            .iter()
            .filter(|k| k.starts_with("TAKO_"))
            .map(|k| k.to_string()),
    );
    keys.extend(ISSUE_1970_KEYS.iter().map(|k| k.to_string()));
    keys.sort();
    keys.dedup();
    keys
}

#[test]
fn 隔離guiのヘルパは本番がペインへ配るenvをすべて落とす() {
    let src = read(ISOLATED_GUI_REL);
    let drop = helper_drop_env(&src);
    let missing: Vec<String> = production_pane_env()
        .into_iter()
        .filter(|k| !drop.contains(k))
        .collect();
    assert!(
        missing.is_empty(),
        "{ISOLATED_GUI_REL}: ISOLATED_GUI_DROP_ENV に本番 tako がペインへ配る {missing:?} が無い。\
         隔離 GUI とその子（ペイン・自動リネームの claude・mod）が本番の GUI / mod へ繋がる（#1970）"
    );
    // 起動の行が一覧から `env -u` を組み、既定（自動リネーム停止・一時の設定 dir）を
    // 呼び出し側の `VAR=VAL`（"$@"）より前に置いていること
    let code: String = src
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    for needle in [
        "for key in ${ISOLATED_GUI_DROP_ENV[@]+\"${ISOLATED_GUI_DROP_ENV[@]}\"}",
        "drop_args+=(-u \"$key\")",
        "\"TAKO_AUTO_RENAME=${TAKO_AUTO_RENAME:-0}\"",
        "\"CLAUDE_CONFIG_DIR=${claude_config}\"",
        "iso_default_zshrc \"$@\"",
    ] {
        assert!(
            code.contains(needle),
            "{ISOLATED_GUI_REL}: launch_isolated_gui に `{needle}` が無い（#1970 の既定が外れている）"
        );
    }
    let launch = code
        .lines()
        .find(|l| l.contains("\"$APP_BIN\"") && l.trim_start().starts_with("env "))
        .unwrap_or_else(|| panic!("{ISOLATED_GUI_REL}: GUI を起こす env の行が見つからない"));
    let pos = |s: &str| {
        launch
            .find(s)
            .unwrap_or_else(|| panic!("{ISOLATED_GUI_REL}: 起動の行に {s} が無い: {launch}"))
    };
    // 配列は bash 3.2 の慣用句（空でも set -u で落ちない）で展開する（shell_scripts の番犬と同じ）
    assert!(
        pos("${drop_args[@]+\"${drop_args[@]}\"}") < pos("${env_args[@]+\"${env_args[@]}\"}")
            && pos("${env_args[@]+\"${env_args[@]}\"}") < pos("\"$@\""),
        "{ISOLATED_GUI_REL}: 起動の行は `env -u …`（落とす）→ 既定 → 呼び出し側の `VAR=VAL` の順に並べる\
         （並べた値が既定に勝ち、落とした名前も並べれば届く。#1970）: {launch}"
    );
}

// ---- 検証スクリプトの走査 -------------------------------------------------------------

/// 走査で見つけた「claude / tako を起こすコマンド」1 つ
#[derive(Debug)]
struct Launch {
    /// 1 始まりの行番号（継続行はその先頭の行）
    line: usize,
    /// "claude" か "tako"
    what: &'static str,
    /// 起こすコマンドの単語（引用符つきのまま。`"$BIN/claude"` 等）
    cmd: String,
    /// 同じコマンドの `env -u K`（`env -i` なら全部）で落としている名前
    env_drops: Vec<String>,
    env_clear: bool,
}

/// 走査の結果: 起こすコマンドの一覧と、ファイル全体の `unset` で落としている名前
#[derive(Debug, Default)]
struct ScriptScan {
    launches: Vec<Launch>,
    unset: Vec<String>,
}

fn is_assignment(word: &str) -> bool {
    let name_end = word.find(['=', '+']).unwrap_or(0);
    name_end > 0
        && word[name_end..].trim_start_matches('+').starts_with('=')
        && word[..name_end]
            .chars()
            .enumerate()
            .all(|(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
}

/// 単語の囲みの引用符を外す（`"$TAKO_BIN"` → `$TAKO_BIN`）
fn unquote(word: &str) -> String {
    word.chars().filter(|c| *c != '"' && *c != '\'').collect()
}

/// 1 つのコマンド（単語の列）から、起こすもの・`env -u`・`unset` を読む
fn classify(words: &[String], line: usize, scan: &mut ScriptScan) {
    let mut i = 0;
    let mut env_drops = Vec::new();
    let mut env_clear = false;
    loop {
        let Some(w) = words.get(i) else { return };
        let bare = unquote(w);
        // 前置き（代入・exec 等）は読み飛ばす。`command -v` は在るかの確認なので読み飛ばさない
        let lookup = bare == "command" && words.get(i + 1).is_some_and(|n| n.starts_with('-'));
        // 後ろに単語が続く配列の展開（`${extra[@]+"${extra[@]}"} "$BIN/claude"`）は前置きの
        // `VAR=VAL` の束として読み飛ばす（コマンドは後ろの単語）
        let spread = bare.starts_with("${") && bare.contains("[@]") && words.len() > i + 1;
        if is_assignment(w)
            || matches!(bare.as_str(), "exec" | "nohup" | "time" | "!")
            || (bare == "command" && !lookup)
            || spread
        {
            i += 1;
        } else if bare == "env" {
            i += 1;
            while let Some(opt) = words.get(i) {
                let opt = unquote(opt);
                if opt == "-u" {
                    if let Some(k) = words.get(i + 1) {
                        env_drops.push(unquote(k));
                    }
                    i += 2;
                } else if let Some(k) = opt.strip_prefix("--unset=") {
                    env_drops.push(k.to_string());
                    i += 1;
                } else if opt == "-i" || opt == "-" {
                    env_clear = true;
                    i += 1;
                } else if opt.starts_with('-') {
                    i += 1;
                } else {
                    break;
                }
            }
        } else {
            break;
        }
    }
    let Some(raw) = words.get(i) else { return };
    let cmd = unquote(raw);
    if cmd == "unset" {
        scan.unset.extend(
            words[i + 1..]
                .iter()
                .map(|w| unquote(w))
                .filter(|w| !w.starts_with('-')),
        );
        return;
    }
    // パスで起こす claude（`"$BIN/claude"` = 実物を PATH の外から symlink で置く形）も数える
    let what = if cmd == "claude" || cmd.ends_with("/claude") {
        // 版の問い合わせは会話も mod も MCP も起こさない（mod の下限の判定で使う）
        if words
            .get(i + 1)
            .is_some_and(|a| matches!(unquote(a).as_str(), "--version" | "-v"))
        {
            return;
        }
        "claude"
    } else if matches!(
        cmd.as_str(),
        "tako" | "$TAKO_BIN" | "${TAKO_BIN}" | "$TAKO" | "${TAKO}"
    ) || cmd.ends_with("/tako")
    {
        "tako"
    } else {
        return;
    };
    scan.launches.push(Launch {
        line,
        what,
        cmd: raw.clone(),
        env_drops,
        env_clear,
    });
}

/// シェルの 1 行（継続行は連結済み）を、**このスクリプトのプロセスで走るコマンド**の単語の列へ分ける。
///
/// 引用符の中は単語の一部（`tk send "claude …"` はペインで走る = 隔離 GUI の env を継ぐ）。
/// `$( … )` は引用符の中でもこのプロセスで走るので、中身を別のコマンドとして読む。
/// `case` のパターン（`claude)`）はコマンドにしない
struct ShellLine<'a> {
    chars: Vec<char>,
    i: usize,
    out: &'a mut Vec<Vec<String>>,
}

impl ShellLine<'_> {
    /// コマンドの並びを読む。`nested` なら対応する `)` で戻る
    fn list(&mut self, nested: bool) {
        let mut words: Vec<String> = Vec::new();
        // 読みかけの単語（None = 単語の外。`#` が行コメントか単語の一部かの分かれ目）
        let mut word: Option<String> = None;
        macro_rules! end_word {
            () => {
                if let Some(w) = word.take() {
                    words.push(w);
                }
            };
        }
        macro_rules! end_cmd {
            () => {
                end_word!();
                if !words.is_empty() {
                    self.out.push(std::mem::take(&mut words));
                }
            };
        }
        while self.i < self.chars.len() {
            let c = self.chars[self.i];
            match c {
                ' ' | '\t' => {
                    end_word!();
                    // 予約語は区切り（`if claude …` / `then "$TAKO_BIN" …` / `{ … }`）
                    if words.len() == 1
                        && matches!(
                            words[0].as_str(),
                            "if" | "then" | "else" | "elif" | "do" | "while" | "until" | "{"
                        )
                    {
                        words.clear();
                    }
                    self.i += 1;
                }
                '#' if word.is_none() => break,
                ';' | '&' | '|' | '\n' => {
                    end_cmd!();
                    self.i += 1;
                }
                '(' => {
                    // `tk() {` の関数定義・`arr=(…)` の配列・`( … )` の副シェル
                    end_cmd!();
                    self.i += 1;
                    self.list(true);
                }
                ')' => {
                    self.i += 1;
                    if nested {
                        end_cmd!();
                        return;
                    }
                    // 対応の無い `)` は case のパターン = コマンドではない
                    words.clear();
                    word = None;
                }
                '\\' => {
                    let w = word.get_or_insert_with(String::new);
                    w.push(c);
                    if let Some(n) = self.chars.get(self.i + 1) {
                        w.push(*n);
                    }
                    self.i += 2;
                }
                '\'' => {
                    let w = word.get_or_insert_with(String::new);
                    w.push(c);
                    self.i += 1;
                    while self.i < self.chars.len() {
                        let q = self.chars[self.i];
                        w.push(q);
                        self.i += 1;
                        if q == '\'' {
                            break;
                        }
                    }
                }
                '"' => {
                    let mut w = word.take().unwrap_or_default();
                    w.push(c);
                    self.i += 1;
                    self.double_quoted(&mut w);
                    word = Some(w);
                }
                '$' if self.chars.get(self.i + 1) == Some(&'(') => {
                    word.get_or_insert_with(String::new).push_str("$(…)");
                    self.substitution();
                }
                '`' => {
                    // 逆引用符の置換は使っていない（読み飛ばす）
                    word.get_or_insert_with(String::new);
                    self.i += 1;
                    while self.i < self.chars.len() && self.chars[self.i] != '`' {
                        self.i += 1;
                    }
                    self.i += 1;
                }
                _ => {
                    let w = word.get_or_insert_with(String::new);
                    w.push(c);
                    self.i += 1;
                    // `}` だけの単語は区切り（関数の本体の括り）
                    if w == "}"
                        && self
                            .chars
                            .get(self.i)
                            .is_none_or(|n| n.is_whitespace() || *n == ';')
                    {
                        word = None;
                        end_cmd!();
                    }
                }
            }
        }
        end_cmd!();
    }

    /// `"…"` の中（開きの `"` は読み済み）。`$( … )` だけはコマンドとして読む
    fn double_quoted(&mut self, word: &mut String) {
        while self.i < self.chars.len() {
            let c = self.chars[self.i];
            match c {
                '\\' => {
                    word.push(c);
                    if let Some(n) = self.chars.get(self.i + 1) {
                        word.push(*n);
                    }
                    self.i += 2;
                }
                '"' => {
                    word.push(c);
                    self.i += 1;
                    return;
                }
                '$' if self.chars.get(self.i + 1) == Some(&'(') => {
                    word.push_str("$(…)");
                    self.substitution();
                }
                _ => {
                    word.push(c);
                    self.i += 1;
                }
            }
        }
    }

    /// `$(` の位置から。`$((` は算術なので読み飛ばし、`$(` はコマンドの並びとして読む
    fn substitution(&mut self) {
        if self.chars.get(self.i + 2) == Some(&'(') {
            let mut depth = 0usize;
            while self.i < self.chars.len() {
                match self.chars[self.i] {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            self.i += 1;
                            return;
                        }
                    }
                    _ => {}
                }
                self.i += 1;
            }
            return;
        }
        self.i += 2;
        self.list(true);
    }
}

/// ヒアドキュメントの終わりの印（`<<'EOF'` / `<<-EOF` / `<<"EOF"`。`<<<` は対象外）
fn heredoc_marker(line: &str) -> Option<(String, bool)> {
    let bytes = line.as_bytes();
    let mut at = 0;
    while let Some(off) = line[at..].find("<<") {
        let p = at + off;
        at = p + 2;
        if bytes.get(p + 2) == Some(&b'<') || (p > 0 && bytes[p - 1] == b'<') {
            at = p + 3;
            continue;
        }
        let mut rest = &line[p + 2..];
        let strip_tabs = rest.starts_with('-');
        if strip_tabs {
            rest = &rest[1..];
        }
        let rest = rest.trim_start().trim_start_matches(['\'', '"', '\\']);
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        // 算術のシフト（`$((1 << 2))`）を印と読まない = 名前は英字か `_` で始まる
        if name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
            return Some((name, strip_tabs));
        }
    }
    None
}

/// シェルスクリプト 1 本を走査する（ヒアドキュメントの本文は読まない = 書き出すスタブの中身）
fn scan_script(src: &str) -> ScriptScan {
    let mut scan = ScriptScan::default();
    let lines: Vec<&str> = src.lines().collect();
    let mut idx = 0;
    while idx < lines.len() {
        let start = idx;
        let mut logical = String::new();
        // 継続行（行末の `\`）を 1 行へ
        loop {
            let l = lines[idx];
            idx += 1;
            if let Some(cut) = l.strip_suffix('\\') {
                logical.push_str(cut);
                logical.push(' ');
                if idx < lines.len() {
                    continue;
                }
            } else {
                logical.push_str(l);
            }
            break;
        }
        let mut cmds = Vec::new();
        ShellLine {
            chars: logical.chars().collect(),
            i: 0,
            out: &mut cmds,
        }
        .list(false);
        for words in &cmds {
            classify(words, start + 1, &mut scan);
        }
        if let Some((end, strip_tabs)) = heredoc_marker(&logical) {
            while idx < lines.len() {
                let l = lines[idx];
                idx += 1;
                let l = if strip_tabs {
                    l.trim_start_matches('\t')
                } else {
                    l
                };
                if l == end {
                    break;
                }
            }
        }
    }
    // パスで起こす claude のうち、このスクリプトが `>` で書き出したもの（偽の claude のスタブ）は
    // 本物ではないので数えない。symlink で置いた実物（`ln -s`）は書き出さないので残る
    scan.launches.retain(|l| {
        l.what != "claude"
            || unquote(&l.cmd) == "claude"
            || ![">", ">>", "> ", ">> "]
                .iter()
                .any(|redirect| src.contains(&format!("{redirect}{}", l.cmd)))
    });
    scan
}

/// 落としていない名前を `rel:line` で並べる（起こすもの 1 種につき最初の 1 行と件数）
fn launch_violations(rel: &str, scan: &ScriptScan) -> Vec<String> {
    let mut out = Vec::new();
    for (what, required) in [("claude", ISSUE_1970_KEYS), ("tako", TAKO_CLI_KEYS)] {
        let bad: Vec<(&Launch, Vec<&str>)> = scan
            .launches
            .iter()
            .filter(|l| l.what == what && !l.env_clear)
            .map(|l| {
                let missing: Vec<&str> = required
                    .iter()
                    .copied()
                    .filter(|k| {
                        !scan.unset.iter().any(|u| u == k) && !l.env_drops.iter().any(|u| u == k)
                    })
                    .collect();
                (l, missing)
            })
            .filter(|(_, m)| !m.is_empty())
            .collect();
        if let Some((first, missing)) = bad.first() {
            out.push(format!(
                "{rel}:{}: {what} を起こす前に {missing:?} を落としていない（同じ形が {} 行）",
                first.line,
                bad.len()
            ));
        }
    }
    out
}

/// 検証スクリプト（`scripts/test-*.sh` / `measure-*.sh` / `verify-*.sh`）
fn verification_scripts() -> Vec<String> {
    let dir = repo_root().join("scripts");
    let mut out: Vec<String> = std::fs::read_dir(&dir)
        .expect("scripts/ を読む")
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| {
            n.ends_with(".sh")
                && (n.starts_with("test-") || n.starts_with("measure-") || n.starts_with("verify-"))
        })
        .map(|n| format!("scripts/{n}"))
        .collect();
    out.sort();
    assert!(
        out.len() >= 100,
        "検証スクリプトが {} 本しか見つからない（走査範囲の取り方が壊れている）",
        out.len()
    );
    out
}

#[test]
fn 検証スクリプトは本番のenvを落としてからclaudeとtakoを起こす() {
    let mut violations = Vec::new();
    let mut launching = 0usize;
    for rel in verification_scripts() {
        let scan = scan_script(&read(&rel));
        if !scan.launches.is_empty() {
            launching += 1;
        }
        violations.extend(launch_violations(&rel, &scan));
    }
    // 走査が何も拾わなくなったら、どんな漏れでも通る
    assert!(
        launching >= 50,
        "claude / tako を起こす検証スクリプトが {launching} 本しか見つからない（走査が壊れている）"
    );
    assert!(
        violations.is_empty(),
        "本番 tako の env を落とさずに claude / tako を起こす検証スクリプトがある（#1970）。\
         先頭で `unset {}` するか、その行を `env -u …` で起こす（tako CLI だけなら TAKO_SOCKET / TAKO_PANE_ID）:\n  {}",
        ISSUE_1970_KEYS.join(" "),
        violations.join("\n  ")
    );
}

#[test]
fn 走査は実際の持ち込みを検出する() {
    let all = ISSUE_1970_KEYS.join(" ");
    let cases: &[(&str, &str, usize)] = &[
        // (説明, スクリプト, 名指す数)
        ("落とさずに CLI", "\"$TAKO_BIN\" list\n", 1),
        ("CLI の 2 つを落とす", "unset TAKO_SOCKET TAKO_PANE_ID\n\"$TAKO_BIN\" list\n", 0),
        ("CLI の行で env -u", "env -u TAKO_SOCKET -u TAKO_PANE_ID TAKO_ISOLATED=1 \"$TAKO_BIN\" list\n", 0),
        ("関数の中の CLI", "tk() { \"$TAKO_BIN\" \"$@\"; }\n", 1),
        ("ソケットだけ落とした claude", "unset TAKO_SOCKET TAKO_PANE_ID\nclaude -p hi\n", 1),
        ("置換の中の claude", "out=\"$(CLAUDE_CONFIG_DIR=x claude plugin validate .)\"\n", 1),
        ("if の後の claude", "if claude -p hi >/dev/null; then :; fi\n", 1),
        ("env -i の claude", "env -i PATH=/bin claude -p hi\n", 0),
        ("ペインへ送る文字列", "unset TAKO_SOCKET TAKO_PANE_ID\n\"$TAKO_BIN\" send --pane 1 \"claude --model haiku\"\n", 0),
        ("ヒアドキュメントの本文", "cat > f <<'EOF'\nclaude -p hi\n\"$TAKO_BIN\" list\nEOF\n", 0),
        ("在るかの確認", "command -v claude >/dev/null || exit 0\n", 0),
        ("版の問い合わせ", "v=$(claude --version 2>/dev/null)\n", 0),
        ("版の後の会話", "v=$(claude --version)\nclaude -p hi\n", 1),
        ("symlink で置いた実物", "ln -sf \"$(command -v claude)\" \"$BIN/claude\"\n\"$BIN/claude\" plugin list\n", 1),
        ("書き出した偽の claude", "cat > \"$BIN/claude\" <<'EOF'\nexit 0\nEOF\n\"$BIN/claude\" --warm\n", 0),
        ("配列の展開の後ろの claude", "env -u TAKO_SOCKET HOME=\"$H\" ${extra[@]+\"${extra[@]}\"} \"$BIN/claude\" plugin list\n", 1),
        ("内側の印で分ける", "if [ -z \"${INNER:-}\" ]; then\n  unset TAKO_SOCKET TAKO_PANE_ID\n  exec cargo run -- bash \"$0\"\nfi\n\"$TAKO\" list\n", 0),
        ("case のパターン", "case \"$a\" in\n  claude) echo c ;;\nesac\n", 0),
        ("引用符の中の説明", "echo \"claude を起こす\"\n", 0),
        ("算術", "n=$((n + 1))\n", 0),
    ];
    for (what, src, want) in cases {
        let got = launch_violations("t.sh", &scan_script(src));
        assert_eq!(got.len(), *want, "{what}: {src:?} → {got:?}");
    }
    // 継続行の unset と行番号（継続行の先頭の行で名指す）
    let src = format!(
        "unset {} \\\n  TAKO_TOKEN\n\n\nclaude -p hi\n",
        ISSUE_1970_KEYS[..4].join(" ")
    );
    let got = launch_violations("t.sh", &scan_script(&src));
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].starts_with("t.sh:5: claude"), "{got:?}");
    assert!(
        got[0].contains("TAKO_CLI") && !got[0].contains("TAKO_SOCKET"),
        "{got:?}"
    );
    let src = format!("unset {all}\nclaude -p hi\n");
    assert!(launch_violations("t.sh", &scan_script(&src)).is_empty());
}

// ---- ヘルパの挙動（偽の GUI と偽の面の係で起こす。窓は出さない）------------------------

/// 面を用意する係の偽物（いつでも用意できる）
#[cfg(unix)]
const STUB_VD_1970: &str = "#!/bin/bash\ncase \"$1\" in ensure) exit 0 ;; bounds) echo '0 0 1 1'; exit 0 ;; esac\nexit 1\n";

/// GUI の偽物。受け取った env を丸ごと書き出す
#[cfg(unix)]
const STUB_APP_1970: &str = "#!/bin/bash\nenv > \"$STUB_ENV_OUT\"\n";

#[cfg(unix)]
struct Scratch1970(PathBuf);

#[cfg(unix)]
impl Scratch1970 {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("tako-1970-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("一時 dir を作る");
        // macOS の temp_dir は /var → /private/var のシンボリックリンク越し。ヘルパは物理パスで返す
        Scratch1970(dir.canonicalize().expect("一時 dir の物理パス"))
    }
    fn dir(&self, name: &str) -> PathBuf {
        let p = self.0.join(name);
        std::fs::create_dir_all(&p).expect("dir を作る");
        p
    }
}

#[cfg(unix)]
impl Drop for Scratch1970 {
    fn drop(&mut self) {
        let tmp = std::env::temp_dir().canonicalize().expect("temp_dir");
        assert!(
            self.0.starts_with(&tmp),
            "一時 dir の外を消そうとしている: {}",
            self.0.display()
        );
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// ヘルパを source して `launch_isolated_gui` を 1 回呼び、偽の GUI が受け取った env を返す
/// （macOS 同梱の bash 3.2 で走らせる）。`env` は呼び出し元の env（本番のペインに見立てる）、
/// `args` は `launch_isolated_gui` へ並べる `VAR=VAL`
#[cfg(unix)]
fn launch_with(
    scratch: &Scratch1970,
    env: &[(&str, &str)],
    args: &[&str],
) -> std::collections::BTreeMap<String, String> {
    use std::os::unix::fs::PermissionsExt;
    let run = scratch.dir("run");
    let vd = run.join("virtual-display.sh");
    let app = run.join("app");
    for (path, body) in [(&vd, STUB_VD_1970), (&app, STUB_APP_1970)] {
        std::fs::write(path, body).expect("偽物を書く");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let out = run.join("env.out");
    let _ = std::fs::remove_file(&out);
    let script = r#"
. "$HELPER"
APP_BIN="$STUB_APP_BIN"
launch_isolated_gui "$STUB_RUN/app.log" "$@" || exit $?
wait "$ISOLATED_GUI_PID"
"#;
    let mut cmd = std::process::Command::new("/bin/bash");
    cmd.arg("-c")
        .arg(script)
        .arg("launch_with")
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        // 偽の係の下でも実在しない面の名前（ヘルパが差し替え口を無視しても実機に触れない）
        .env("TAKO_VD_NAME", "tako-vd-watchdog-1970-absent")
        .env("HELPER", repo_root().join(ISOLATED_GUI_REL))
        .env("ISOLATED_GUI_VD", &vd)
        // 利用者のホームの見立て（ここには既定の .zshrc を書かない）
        .env("ISOLATED_GUI_REAL_HOME", scratch.dir("realhome"))
        .env("STUB_APP_BIN", &app)
        .env("STUB_RUN", &run)
        .env("STUB_ENV_OUT", &out);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let status = cmd.status().expect("/bin/bash を起こす");
    assert!(
        status.success(),
        "launch_isolated_gui が失敗した: {status:?}"
    );
    std::fs::read_to_string(&out)
        .expect("偽の GUI が起きていない")
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// 本番のペインに見立てた env（ヘルパが落とす名前すべてに本番の値を入れる）
#[cfg(unix)]
fn production_like_env(home: &Path) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = helper_drop_env(&read(ISOLATED_GUI_REL))
        .into_iter()
        .map(|k| (k, "prod-1970".to_string()))
        .collect();
    env.push((
        "CLAUDE_CONFIG_DIR".into(),
        "/nonexistent/real-claude-config".into(),
    ));
    env.push(("HOME".into(), home.display().to_string()));
    env
}

#[cfg(unix)]
#[test]
fn 隔離guiのヘルパは本番のenvを落とし既定を置いて起こす() {
    let scratch = Scratch1970::new("defaults");
    let home = scratch.dir("home");
    let env = production_like_env(&home);
    let env_ref: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();

    // 既定: 本番の名前は 1 つも届かず、自動リネームは止まる。HOME が一時の dir なので
    // export された本物の CLAUDE_CONFIG_DIR は外すだけ（claude はその HOME の .claude を使う）
    let got = launch_with(&scratch, &env_ref, &[]);
    let leaked: Vec<&String> = got.keys().filter(|k| got[*k] == "prod-1970").collect();
    assert!(
        leaked.is_empty(),
        "本番の env が隔離 GUI へ届いた: {leaked:?}"
    );
    assert_eq!(got.get("TAKO_AUTO_RENAME").map(String::as_str), Some("0"));
    assert_eq!(
        got.get("CLAUDE_CONFIG_DIR"),
        None,
        "一時の HOME なのに export された本物の CLAUDE_CONFIG_DIR が残った / 別の dir へ差し替えた"
    );
    let zshrc =
        std::fs::read_to_string(home.join(".zshrc")).expect("一時の HOME に既定の .zshrc が無い");
    assert!(zshrc.contains("PROMPT='tako %1~ %% '"), "{zshrc}");
    // 落とさない名前（検証スクリプトが渡す隔離の置き場）は届く
    assert_eq!(got.get("HOME").map(String::as_str), home.to_str());
    assert_eq!(got.get("TAKO_ISOLATED").map(String::as_str), Some("1"));

    // HOME が利用者のホームのまま（の見立て）なら、設定 dir はログの置き場の隣の一時 dir
    let real = scratch.dir("realhome");
    let real_arg = format!("HOME={}", real.display());
    let got = launch_with(&scratch, &env_ref, &[&real_arg]);
    let cfg = scratch.0.join("run/isolated-claude-config");
    assert_eq!(
        got.get("CLAUDE_CONFIG_DIR").map(String::as_str),
        cfg.to_str(),
        "利用者のホームのままなのに CLAUDE_CONFIG_DIR がログの置き場の隣の一時 dir になっていない"
    );
    assert!(cfg.is_dir(), "一時の設定 dir が作られていない");

    // 並べた値は既定にも落とす一覧にも勝つ（明示で on にできる口）
    let mine = scratch.dir("mine-config");
    let mine_arg = format!("CLAUDE_CONFIG_DIR={}", mine.display());
    let got = launch_with(
        &scratch,
        &env_ref,
        &["TAKO_AUTO_RENAME=1", &mine_arg, "TAKO_CLI=/iso/tako"],
    );
    assert_eq!(got.get("TAKO_AUTO_RENAME").map(String::as_str), Some("1"));
    assert_eq!(
        got.get("CLAUDE_CONFIG_DIR").map(String::as_str),
        mine.to_str()
    );
    assert_eq!(got.get("TAKO_CLI").map(String::as_str), Some("/iso/tako"));
    assert!(
        !got.contains_key("TAKO_SOCKET"),
        "並べていない名前は落ちたまま"
    );

    // export された自動リネームの指定は尊重する（本番 tako は配らない名前）
    let mut env_on = env_ref.clone();
    env_on.push(("TAKO_AUTO_RENAME", "1"));
    let got = launch_with(&scratch, &env_on, &[]);
    assert_eq!(got.get("TAKO_AUTO_RENAME").map(String::as_str), Some("1"));
}

#[cfg(unix)]
#[test]
fn 既定のzshrcは利用者のホームと既存のrcへ書かない() {
    let scratch = Scratch1970::new("zshrc");
    // 利用者のホーム（の見立て）には rc が無くても書かない
    let real = scratch.dir("realhome");
    let env = production_like_env(&real);
    let env_ref: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    launch_with(&scratch, &env_ref, &[]);
    assert!(
        !real.join(".zshrc").exists(),
        "利用者のホームへ .zshrc を書いた"
    );
    // 利用者のホームの中の dir（ZDOTDIR=~/.config/zsh の形）にも書かない
    let inner = scratch.dir("realhome/.config/zsh");
    let inner_arg = format!("ZDOTDIR={}", inner.display());
    launch_with(&scratch, &env_ref, &[&inner_arg]);
    assert!(
        !inner.join(".zshrc").exists(),
        "利用者のホームの中の ZDOTDIR へ .zshrc を書いた"
    );

    // ZDOTDIR を並べたら、HOME ではなくそちらへ置く
    let home = scratch.dir("home2");
    let zdot = scratch.dir("zdot");
    let home_arg = format!("HOME={}", home.display());
    let zdot_arg = format!("ZDOTDIR={}", zdot.display());
    launch_with(&scratch, &env_ref, &[&home_arg, &zdot_arg]);
    assert!(
        zdot.join(".zshrc").exists(),
        "ZDOTDIR へ既定の .zshrc を置いていない"
    );
    assert!(
        !home.join(".zshrc").exists(),
        "ZDOTDIR があるのに HOME へ書いた"
    );

    // 呼び出し側の rc は上書きしない（.zprofile だけでも初回設定メニューは出ないので足さない）
    let own = scratch.dir("own");
    std::fs::write(own.join(".zshrc"), "PROMPT='mine '\n").expect("rc を書く");
    let prof = scratch.dir("prof");
    std::fs::write(prof.join(".zprofile"), "export X=1\n").expect("rc を書く");
    for dir in [&own, &prof] {
        let arg = format!("HOME={}", dir.display());
        launch_with(&scratch, &env_ref, &[&arg]);
    }
    assert_eq!(
        std::fs::read_to_string(own.join(".zshrc")).unwrap(),
        "PROMPT='mine '\n"
    );
    assert!(
        !prof.join(".zshrc").exists(),
        ".zprofile があるのに .zshrc を足した"
    );
}
