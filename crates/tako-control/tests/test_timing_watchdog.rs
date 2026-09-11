//! 番犬: 効果を**実時間で比べている**テストがワークスペースに無い（#1220 / #1167）
//!
//! ## なぜ止めるのか
//!
//! 「速くなっている」を `Instant::elapsed` の**比較**で固定したテストは、片方の
//! 計測窓にだけスケジューリングの待ちが入った回に落ちる。実測で 2 件踏んだ:
//!
//! - #1167: `claude_remote_link` の `追記ぶんだけ読むと定常コストが増えない`
//!   （高負荷で 4 回に 1 回）
//! - #1220: `remote_link_live` の `一覧付与のコストが桁で問題ないこと`
//!   （初回 23.8ms / 2 回目 1.7ms なので、2 回目に 22ms 止まれば反転する）
//!
//! 直し方は**測る軸を替える**こと。守りたい性質を負荷に依らない量
//! （読み出しバイト数 / 走査回数 / 呼び出し回数）で書けば、混み具合に依らず
//! 同じ性質を桁で固定できる。規約は `.agent/conventions.md`
//! 「効果を測る単体テストは実時間で比べない」。
//!
//! ## 何を違反とするか（**誤検知しない形**）
//!
//! `assert` 系マクロの**条件部**に、実時間の値が **2 つ以上**現れる形だけを落とす
//! （`assert!(warm <= cold, …)` / `assert!(t1.elapsed() < t0.elapsed())`）。
//! 実時間の値とは `.elapsed()` そのものと、`let x = ….elapsed();` で束縛された名前。
//!
//! 拾えないのは「`let` と `.elapsed()` が別の行に分かれた束縛」だけ（そのときは
//! `.elapsed()` が assert の条件部へ直接現れる形になりやすいので、そちらで捕まる）。
//!
//! 対象外（時間で書くのが正しい形。tests 配下の実際の使い方を分類して決めた）:
//!
//! | 形 | 例 | なぜ対象外か |
//! |---|---|---|
//! | タイムアウト / 待ち | `while t0.elapsed() < limit` / `Instant::now() < deadline` | 主題が「待つこと」。assert の条件部ではない |
//! | 報告のみ | `println!("所要={elapsed:?}")`（`issue1011_agents_scan_cost_e2e`。assert は回数） | 落ちる材料にしていない |
//!
//! ## 絶対予算は「宣言したものだけ」（#962 で広げた）
//!
//! 実時間の値が **1 つ**の形（`assert!(elapsed < Duration::from_secs(2), …)`）は
//! 2 窓の比較ではないので上の規則には当たらない。以前はこれを一律で対象外に
//! していたが、その根拠（**桁で開けてあれば負荷で反転しない**）が成り立って
//! いない実例が出た:
//!
//! - #962: `remote::tests::daemon_stop_implはゾンビpidを終了済みとして扱う` は
//!   予算 **2 秒**に対し、落ちる側（`daemon_stop_impl` のタイムアウト経路）が
//!   **5 秒**。桁が開いていないうえ、正常でも観測 1 回ぶんの `/bin/ps`
//!   （fork+exec）が詰まれば実測 2.27 / 5.10 / **10.07 秒**まで伸びた
//!
//! 桁が開いているかはソースからは判らないので、**置く側に根拠を宣言させる**
//! （[`ABSOLUTE_BUDGET_ALLOWLIST`]）。#962 の直し方は測る軸を替えること
//! （機構の観測値 = どの経路で抜けたか / 何回待ったか）。
//!
//! ## 走査範囲（#962 で広げた）
//!
//! `crates/*/tests/` だけでなく **`crates/*/src/` の `mod tests` も見る**
//! （#962 の現場は `remote.rs` の `mod tests` で、tests 配下しか見ていない
//! 番犬には**最初から見えていなかった**）。A/B の旧経路アーム
//! （`// … LEGACY_ARM 開始` 〜 `終了`）は両方の規則で対象外にする。
//!
//! ## 相棒
//!
//! `remote_link_watchdog.rs` の `追記ぶんだけ読む検査を実時間で測っていない` は
//! **1 ファイルに閉じた強い規則**（`claude_remote_link.rs` のテストは `Instant`
//! そのものを禁止）。こちらはワークスペース全体を、上の 2 形だけで見る

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

/// `crates/*/tests/` と `crates/*/src/` の `.rs` を集める（#962 で src も対象にした）
fn test_sources() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(crates) = std::fs::read_dir(repo_root().join("crates")) else {
        return out;
    };
    for entry in crates.flatten() {
        let mut stack = vec![entry.path().join("tests"), entry.path().join("src")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                let path = e.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|x| x == "rs") {
                    out.push(path);
                }
            }
        }
    }
    out.sort();
    out
}

#[path = "common/code_view.rs"]
mod code_view_mod;
use code_view_mod::code_view;

/// A/B の旧経路アームの除去は #1252 / #1265 の番犬と共有する（前処理を 2 か所に書かない）
#[path = "common/test_source.rs"]
mod test_source;
use test_source::blank_arms;

/// A/B の旧経路アームの囲み（`TAKO_<番号>_LEGACY_ARM`。番号は見ない = 全 Issue 共通）
const ARM_BEGIN: &str = "LEGACY_ARM 開始";
const ARM_END: &str = "LEGACY_ARM 終了";

/// 走査用の眺め（アームを空行へ潰してから、コメントと文字列を潰す）。
/// 行数はどちらの前処理でも保たれるので `file:line` はそのまま数えられる
fn scan_view(src: &str) -> (String, usize) {
    let (body, dropped) = blank_arms(src, ARM_BEGIN, ARM_END);
    (code_view(&body), dropped)
}

/// 実時間の**絶対予算**（実時間の値 1 つと `Duration::from_*` の比較）を許す場所。
///
/// 単一の絶対予算そのものは悪ではない。**落ちる側と桁で開いていれば**負荷では
/// 反転しない。桁が開いているかはソースからは判らないので、ここに根拠を書く形で
/// 宣言させる（#962 は 2 秒の予算に対して落ちる側が 5 秒 = 開いていなかった）。
///
/// 足すときは「正常時の実測」と「回帰したときの値」を書くこと。
/// 桁が開かないなら測る軸を替える（#962 の直し方 = 機構の観測値で見る）
const ABSOLUTE_BUDGET_ALLOWLIST: &[(&str, &str, &str)] = &[
    (
        "crates/tako-core/tests/psmux_backend.rs",
        "killはイコール無しで即座に効く",
        "予算 3 秒 / 正常は数 ms・回帰は `=` 付きターゲットの 5.1 秒ブロック",
    ),
    (
        "crates/tako-control/src/mcp/tests.rs",
        "遅いdispatch中も並行リクエストがブロックされない",
        "予算 200ms（と下限 400ms）/ 正常は数 ms・直列化したら遅い側の 500ms 待ち",
    ),
    (
        "crates/tako-control/src/sleep_guard.rs",
        "iokit_ffi_calls_are_fast",
        "予算 1 秒 / 正常は FFI ×400 で数百 µs・回帰はサブプロセス ×400 で数十秒",
    ),
];

/// その位置を囲んでいる `fn` の名前（入れ子の `fn` はテストに無いので直前のものが囲み）
fn enclosing_fn(code: &str, at: usize) -> String {
    let mut best = String::new();
    for (pos, _) in code[..at].match_indices("fn ") {
        if pos > 0 {
            let prev = code.as_bytes()[pos - 1] as char;
            if prev.is_alphanumeric() || prev == '_' {
                continue;
            }
        }
        let name: String = code[pos + 3..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            best = name;
        }
    }
    best
}

/// 実時間の**絶対予算**の形か（実時間の値と `Duration::from_*` の比較）
fn is_absolute_budget(cond: &str) -> bool {
    let time_value = cond.contains(".elapsed()") || word_like_elapsed(cond);
    time_value && cond.contains("Duration::from_")
}

/// `elapsed` を含む識別子が現れるか（`fast_elapsed` / `elapsed` / `slow_elapsed`）
fn word_like_elapsed(cond: &str) -> bool {
    cond.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|t| t.contains("elapsed"))
}

/// `let x = ….elapsed()…;` で束縛された名前（= 実時間の値）。
///
/// **同じ名前は 1 つに畳む**（#962 で src を走査して踏んだ: `main.rs` は
/// `let waited = started.elapsed();` を 3 か所で束縛していて、別スコープの
/// `let waited = format!(…)` を使う assert が「実時間の値 3 つ」に見えていた）
fn duration_names(code: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in code.lines() {
        let t = line.trim_start();
        let Some(rest) = t.strip_prefix("let ") else {
            continue;
        };
        if !line.contains(".elapsed()") {
            continue;
        }
        let name: String = rest
            .trim_start_matches("mut ")
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// 名前が**識別子として**現れる回数（`elapsed` が `elapsed_ms` に当たらないように）
fn word_count(haystack: &str, word: &str) -> usize {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let b = haystack.as_bytes();
    haystack
        .match_indices(word)
        .filter(|(at, _)| {
            let before_ok =
                *at == 0 || !(b[at - 1] as char).is_ascii() || !ident(b[at - 1] as char);
            let after = at + word.len();
            let after_ok = after >= b.len() || !ident(b[after] as char);
            before_ok && after_ok
        })
        .count()
}

/// assert 系マクロの**条件部**を返す（`(byte offset, 条件のテキスト)`）。
///
/// `assert!` は第 1 引数、`assert_eq!` / `assert_ne!` は第 1〜2 引数。
/// **メッセージ部は見ない**（`assert!(a.scans <= n, "…{cold:?}/{warm:?}…")` を
/// 違反にしないため。文字列は潰してあるが、フォーマット引数は素の名前で並ぶ）
fn assert_conditions(code: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (name, args) in [("assert!(", 1), ("assert_eq!(", 2), ("assert_ne!(", 2)] {
        for (at, _) in code.match_indices(name) {
            let open = at + name.len();
            let mut depth = 1i32;
            let mut cuts: Vec<usize> = Vec::new();
            let mut end = code.len();
            for (rel, c) in code[open..].char_indices() {
                match c {
                    '(' | '[' | '{' => depth += 1,
                    ')' | ']' | '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + rel;
                            break;
                        }
                    }
                    ',' if depth == 1 => cuts.push(open + rel),
                    _ => {}
                }
            }
            let cut = cuts.get(args - 1).copied().unwrap_or(end).min(end);
            out.push((
                at,
                code[open..cut]
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
            ));
        }
    }
    out
}

#[test]
fn 効果を実時間で比べているテストが無い() {
    let root = repo_root();
    let mut violations: Vec<String> = Vec::new();
    for path in test_sources() {
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        let (code, _) = scan_view(&src);
        let names = duration_names(&code);
        for (at, cond) in assert_conditions(&code) {
            let inline = cond.matches(".elapsed()").count();
            let named: usize = names.iter().map(|n| word_count(&cond, n)).sum();
            if inline + named < 2 {
                continue;
            }
            let line = code[..at].lines().count();
            let shown = path.strip_prefix(&root).unwrap_or(&path).display();
            violations.push(format!("  {shown}:{line}: assert(… {cond} …)"));
        }
    }
    assert!(
        violations.is_empty(),
        "実時間（`Instant::elapsed`）同士を assert の条件部で比べているテストがある\n\
         （片方の計測窓にだけ待ちが入った回に落ちる。#1167 は 4 回に 1 回・#1220 は\n\
         他 worker のビルドと同時で 1 回 FAILED）。守りたい性質を負荷に依らない量\n\
         （読み出しバイト数 / 走査回数 / 呼び出し回数）で書き直す:\n{}\n\n\
         直し方の実例: `remote_link_live` の `一覧付与のコストが桁で問題ないこと`\n\
         （`claude_remote_link::scan_counters` で回数とバイト数を数える）。\n\
         規約は `.agent/conventions.md`「効果を測る単体テストは実時間で比べない」",
        violations.join("\n")
    );
}

#[test]
fn 実時間の絶対予算は宣言したものだけ() {
    let root = repo_root();
    let mut violations: Vec<String> = Vec::new();
    for path in test_sources() {
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        let shown = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        let (code, _) = scan_view(&src);
        for (at, cond) in assert_conditions(&code) {
            if !is_absolute_budget(&cond) {
                continue;
            }
            let owner = enclosing_fn(&code, at);
            let declared = ABSOLUTE_BUDGET_ALLOWLIST
                .iter()
                .any(|(f, n, _)| shown.replace('\\', "/") == *f && owner == *n);
            if declared {
                continue;
            }
            let line = code[..at].lines().count();
            violations.push(format!("  {shown}:{line}（{owner}）: assert(… {cond} …)"));
        }
    }
    assert!(
        violations.is_empty(),
        "実時間の**絶対予算**を assert の条件部に置いているテストがある。\n\
         負荷で反転しないのは「落ちる側と桁で開いている」ときだけで、#962 は\n\
         予算 2 秒に対して落ちる側（タイムアウト経路）が 5 秒・正常でも `/bin/ps`\n\
         が詰まれば実測 10.07 秒だった:\n{}\n\n\
         直し方は測る軸を替えること（機構の観測値 = どの経路で抜けたか / 何回待ったか。\n\
         実例: `remote::TerminationWait` と `last_termination_wait()`）。\n\
         桁が開いている確信があるなら `ABSOLUTE_BUDGET_ALLOWLIST` へ根拠つきで足す。\n\
         規約は `.agent/conventions.md`「効果を測る単体テストは実時間で比べない」",
        violations.join("\n")
    );
}

/// 走査範囲とアーム除去を取り違えて**何も見ていない番犬**になっていないこと
#[test]
fn 番犬が走査対象とアームを見つけている() {
    let root = repo_root();
    let sources = test_sources();
    // src / tests の両方を拾えていること（#962 の現場は src 側）
    let rel = |p: &PathBuf| {
        p.strip_prefix(&root)
            .unwrap_or(p)
            .display()
            .to_string()
            .replace('\\', "/")
    };
    assert!(
        sources
            .iter()
            .any(|p| rel(p) == "crates/tako-control/src/remote.rs"),
        "src 配下（#962 の現場 remote.rs）を走査していない"
    );
    assert!(
        sources
            .iter()
            .any(|p| rel(p) == "crates/tako-core/tests/psmux_backend.rs"),
        "tests 配下を走査していない"
    );
    // 許可リストの宣言先が実在すること（テスト名が変わったら気づける）
    for (file, name, reason) in ABSOLUTE_BUDGET_ALLOWLIST {
        let src = std::fs::read_to_string(root.join(file))
            .unwrap_or_else(|e| panic!("{file} を読めない: {e}"));
        assert!(
            src.contains(&format!("fn {name}(")),
            "許可リストの宣言先が居ない: {file} の {name}"
        );
        assert!(!reason.is_empty(), "{file} の {name} に根拠が無い");
    }
    // #962 の A/B 旧経路アームが在り、除去が効いていること（マーカーの綴り違いで
    // **何も落とさない番犬**になっていたら、旧経路を本体扱いで誤検知する）
    let target = root.join("crates/tako-control/src/remote.rs");
    let src = std::fs::read_to_string(&target).expect("remote.rs を読める");
    let (blanked, dropped) = blank_arms(&src, ARM_BEGIN, ARM_END);
    assert_eq!(
        dropped, 1,
        "remote.rs の A/B 旧経路アーム（{ARM_BEGIN}）が 1 つでない。\
         マーカーを消したなら番犬の前提が崩れているので、番犬側も直すこと"
    );
    assert!(
        src.contains("TAKO_962_LEGACY"),
        "#962 の A/B アーム（TAKO_962_LEGACY）が消えている"
    );
    assert!(
        !blanked.contains("TAKO_962_LEGACY"),
        "アーム除去が効いていない（旧アサートが本体扱いで残る）"
    );
    assert_eq!(
        src.lines().count(),
        blanked.lines().count(),
        "アーム除去で行数が変わると file:line がずれる"
    );
}
