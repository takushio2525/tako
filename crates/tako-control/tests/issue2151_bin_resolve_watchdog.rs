//! git / tmux の場所の解決が**上限なし**に戻らないかの番犬（#2151）
//!
//! # 何が起きていたか
//!
//! `git_bin()` / `tmux_bin()` の初回の解決（`resolve_bin`）が `git --version` とログインシェルの
//! `command -v` を上限なしに待っていた。rc が遅い・固まるログインシェルでは最初の git / tmux の
//! 操作が止まり（UI スレッドから呼ばれれば GUI ごと）、`OnceLock` の初期化の最中は同じ値を引く
//! 他のスレッドも全部その完了を待たされた。
//!
//! # 何を縛るか
//!
//! 1. 解決の本体（`crates/tako-core/src/bin_resolve.rs`）に上限なしの待ちが無い。
//!    例外は A/B の再現アーム `resolve_bin_legacy` だけで、入口がゲート（`issue2151_legacy()`）の後ろ
//! 2. 新しい形が上限つきの待ちを通る（`--version` は `probe::output_with_timeout`、ログインシェルは
//!    境界 B16 の `exe::login_shell_find`、どちらにも締め切りの残り `left()` を渡す）。
//!    上限は `parse_timeout_secs` で読む（0 / 不正 / 空は既定 = 外せない）
//! 3. git / tmux の解決がこの 1 実装を通る（自前で `Command` を組むと上限が 2 系統になる）
//! 4. 打ち切りの知らせの受け口を GUI と CLI の main が渡している（渡さないと persist.log に何も出ない）
//!
//! 落ちるときは **file:line で名指し**する。[`逆戻りを名指しできる`] で、修正前の形を注入した本文が
//! 実際に名指しされることを確かめる（見逃す側へ倒れた番犬を緑のまま残さない）。
//!
//! # 相方
//!
//! 待ちそのもの（戻らないログインシェルを締め切りで打ち切る・`OnceLock` を待つ別スレッドも一緒に
//! 戻る・通常の環境では修正前と同じ場所）は `tako_core::bin_resolve` の単体テスト、実経路
//! （本物の tako CLI に戻らない `$SHELL` を差し替える・A/B）は `scripts/test-bin-resolve-timeout-2151.sh`。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const BIN_RESOLVE: &str = "crates/tako-core/src/bin_resolve.rs";
const GIT: &str = "crates/tako-core/src/git.rs";
const TMUX: &str = "crates/tako-core/src/tmux.rs";
const APP_MAIN: &str = "crates/tako-app/src/main.rs";
const CLI_MAIN: &str = "crates/tako-cli/src/main.rs";

/// A/B の再現アーム（修正前の上限なしの待ちを**わざと残す**関数）
const LEGACY_ARM: &str = "resolve_bin_legacy";

/// 上限なしの待ち（右は名指しの理由）
const UNBOUNDED_WAITS: &[(&str, &str)] = &[
    (
        ".output()",
        "`Command::output()` は子が終わらないと返らない",
    ),
    (
        ".wait_with_output()",
        "`wait_with_output()` は子が終わらないと返らない",
    ),
    (".wait()", "`Child::wait()` は子が終わらないと返らない"),
    (
        ".status()",
        "`Command::status()` は子が終わらないと返らない",
    ),
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

/// テスト領域を空白へ潰した本文（行番号は保たれる。#1420）
fn read(rel: &str) -> String {
    let path = workspace_root().join(rel);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} が読める: {e}", path.display()));
    production_range::scan(&src).text
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// `fn 名前(` の名前（字下げ・`pub` / `pub(crate)` を問わない）
fn fn_name(line: &str) -> Option<&str> {
    if is_comment(line) {
        return None;
    }
    let at = line.find("fn ")?;
    let head = line[..at].trim();
    if !head
        .split_whitespace()
        .all(|w| matches!(w, "pub" | "pub(crate)" | "pub(super)" | "const"))
    {
        return None;
    }
    let rest = &line[at + 3..];
    let end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_'))?;
    (end > 0 && rest[end..].starts_with(['(', '<'])).then(|| &rest[..end])
}

/// 関数の窓（宣言行の 1-based 行番号と本文）。終わりは宣言行と同じ字下げの `}`
fn fn_window(src: &str, name: &str) -> Option<(usize, Vec<String>)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| fn_name(l) == Some(name))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)?;
    Some((
        start + 1,
        lines[start..=end].iter().map(|l| l.to_string()).collect(),
    ))
}

/// 窓の中で `needle` を含む**コードの**行（0-based の相対位置）
fn code_lines_with(window: &[String], needle: &str) -> Vec<usize> {
    window
        .iter()
        .enumerate()
        .filter(|(_, l)| !is_comment(l) && l.contains(needle))
        .map(|(i, _)| i)
        .collect()
}

/// 1 + 2 の検査（本文を受け取る形 = 注入した本文でも同じ検査を回せる）
fn scan_bin_resolve(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    // 1: 上限なしの待ちは再現アームの中だけ
    let mut current: Option<&str> = None;
    for (i, line) in src.lines().enumerate() {
        if let Some(name) = fn_name(line) {
            current = Some(name);
        }
        if is_comment(line) || current == Some(LEGACY_ARM) {
            continue;
        }
        for (needle, why) in UNBOUNDED_WAITS {
            if line.contains(needle) {
                out.push(format!(
                    "{BIN_RESOLVE}:{} — `fn {}` が上限なしの待ち `{needle}` を使っている（{why}。\
                     git / tmux の場所の解決は `OnceLock` の初期化なので、ここで止まると引いた \
                     スレッドが全部止まる = #2151）",
                    i + 1,
                    current.unwrap_or("(トップレベル)")
                ));
            }
        }
    }
    // 1 の続き: 入口は再現アームをゲートの後ろでだけ呼ぶ
    match fn_window(src, "resolve_bin") {
        None => out.push(format!(
            "{BIN_RESOLVE} — `fn resolve_bin` が見つからない（走査が空振り）"
        )),
        Some((at, window)) => {
            let gate = code_lines_with(&window, "issue2151_legacy()");
            let legacy = code_lines_with(&window, &format!("{LEGACY_ARM}("));
            let gated = !gate.is_empty()
                && !legacy.is_empty()
                && legacy.iter().all(|l| gate.iter().any(|g| g <= l));
            if !gated {
                out.push(format!(
                    "{BIN_RESOLVE}:{at} — `fn resolve_bin` が再現アーム `{LEGACY_ARM}` を \
                     A/B のゲート（`issue2151_legacy()`）の後ろで呼んでいない"
                ));
            }
            for needle in ["resolve_within(", "resolve_timeout()", "NOTICES.emit("] {
                if code_lines_with(&window, needle).is_empty() {
                    out.push(format!(
                        "{BIN_RESOLVE}:{at} — `fn resolve_bin` が `{needle}` を通っていない \
                         （上限つきの解決と打ち切りの知らせの配線が外れている）"
                    ));
                }
            }
        }
    }
    // 2: 新しい形の本体が上限つきの待ちに締め切りの残りを渡す
    match fn_window(src, "resolve_within") {
        None => out.push(format!(
            "{BIN_RESOLVE} — `fn resolve_within` が見つからない（走査が空振り）"
        )),
        Some((at, window)) => {
            for (needle, why) in [
                (
                    "probe::output_with_timeout(",
                    "`--version` を上限つきの待ちの 1 実装で待つ",
                ),
                (
                    "exe::login_shell_find(",
                    "ログインシェルを境界 B16 の上限つきの探索で尋ねる",
                ),
                (
                    "left()",
                    "締め切りまでの残りを渡す（段ごとに予算を払い直さない）",
                ),
            ] {
                if code_lines_with(&window, needle).is_empty() {
                    out.push(format!(
                        "{BIN_RESOLVE}:{at} — `fn resolve_within` に `{needle}` が無い（{why}）"
                    ));
                }
            }
            for line in code_lines_with(&window, "Command::new(") {
                out.push(format!(
                    "{BIN_RESOLVE}:{} — `fn resolve_within` が自前で `Command` を組んでいる \
                     （上限つきの待ちを通らない子プロセスになる）",
                    at + line
                ));
            }
        }
    }
    match fn_window(src, "resolve_timeout") {
        Some((at, window)) if code_lines_with(&window, "parse_timeout_secs(").is_empty() => out
            .push(format!(
                "{BIN_RESOLVE}:{at} — `fn resolve_timeout` が `probe::parse_timeout_secs` で読んでいない \
                 （0 / 不正 / 空を既定へ落とさないと、env 1 つで上限が外れる）"
            )),
        Some(_) => {}
        None => out.push(format!(
            "{BIN_RESOLVE} — `fn resolve_timeout` が見つからない（走査が空振り）"
        )),
    }
    out
}

/// 1 + 2: 解決の本体に上限なしの待ちが無く、新しい形が上限つきの待ちを通る
#[test]
fn 場所の解決は上限つきの待ちだけを通る() {
    let offenders = scan_bin_resolve(&read(BIN_RESOLVE));
    assert!(
        offenders.is_empty(),
        "git / tmux の場所の解決に上限なしの待ちが戻っている（#2151）:\n  {}",
        offenders.join("\n  ")
    );
}

/// 3: git / tmux の解決が 1 実装を通る
#[test]
fn gitとtmuxの解決は1実装を通る() {
    let mut offenders = Vec::new();
    for (rel, func) in [(GIT, "resolve_git_bin"), (TMUX, "resolve_tmux_bin")] {
        let src = read(rel);
        match fn_window(&src, func) {
            None => offenders.push(format!(
                "{rel} — `fn {func}` が見つからない（走査が空振り）"
            )),
            Some((at, window)) => {
                if code_lines_with(&window, "crate::resolve_bin(").is_empty() {
                    offenders.push(format!(
                        "{rel}:{at} — `fn {func}` が `crate::resolve_bin` を通っていない"
                    ));
                }
                for line in code_lines_with(&window, "Command::new(") {
                    offenders.push(format!(
                        "{rel}:{} — `fn {func}` が自前で `Command` を組んでいる（上限が 2 系統になる）",
                        at + line
                    ));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "git / tmux の場所の解決が上限つきの 1 実装（`bin_resolve::resolve_bin`）を通っていない（#2151）:\n  {}",
        offenders.join("\n  ")
    );
}

/// 4: 打ち切りの知らせの受け口を GUI と CLI の main が渡している
#[test]
fn 打ち切りの知らせの受け口をmainが渡している() {
    let mut offenders = Vec::new();
    for (rel, func) in [(APP_MAIN, "main"), (CLI_MAIN, "main")] {
        let src = read(rel);
        match fn_window(&src, func) {
            None => offenders.push(format!(
                "{rel} — `fn {func}` が見つからない（走査が空振り）"
            )),
            Some((at, window)) => {
                if code_lines_with(&window, "bin_resolve::set_notice_sink(").is_empty() {
                    offenders.push(format!(
                        "{rel}:{at} — `fn {func}` が `tako_core::bin_resolve::set_notice_sink` を \
                         呼んでいない（ログインシェルを打ち切った理由が persist.log に出ない）"
                    ));
                }
            }
        }
    }
    assert!(offenders.is_empty(), "{}", offenders.join("\n"));
}

/// 番犬そのものの検査: 修正前の形（上限なしの `.output()`・ゲートの無い入口）を注入した本文が
/// 名指しされる（見逃す側へ倒れていない）
#[test]
fn 逆戻りを名指しできる() {
    let src = read(BIN_RESOLVE);
    let bounded = "crate::probe::output_with_timeout(name, &[version_flag], left())";
    assert!(
        src.contains(bounded),
        "注入の目印が本文に無い（番犬の注入を追従させること）"
    );
    let unbounded = src.replace(
        bounded,
        "std::process::Command::new(name).arg(version_flag).output()",
    );
    let hits = scan_bin_resolve(&unbounded);
    assert!(
        hits.iter().any(|h| h.contains(".output()")) && hits.iter().any(|h| h.contains("Command")),
        "上限なしの `.output()` を注入しても名指しされない: {hits:?}"
    );
    let ungated = src.replace("crate::probe::issue2151_legacy()", "false");
    assert!(
        scan_bin_resolve(&ungated)
            .iter()
            .any(|h| h.contains("issue2151_legacy()")),
        "A/B のゲートを外しても名指しされない"
    );
}
