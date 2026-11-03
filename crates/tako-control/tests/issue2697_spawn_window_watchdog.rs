//! **#2697 の番犬**: std の `Command` で起こす子を「起こしてから Job へ入れる」形に戻さない。
//!
//! ## なぜ止めるのか
//!
//! 動いている子を Job へ入れても、入れ終える前にその子が起こした孫は Job の外に生まれる（Windows は
//! 割り当ての前に生まれた子を Job へ入れない）。cmd.exe は起きてから 10〜15ms ほどで子を起こすので、
//! 起こした直後に呼び手のスレッドがそのくらい止まるだけで、取り消し・打ち切りの後に孫が残った
//! （Windows CI の `tako queue` の取り消しの間欠 = #2697）。修正は境界 B30
//! （`tako_core::platform::job`）の 1 実装 `Job::spawn_contained` / `KillScope::spawn`
//! = 一時停止で起こし、入れてから動かす。
//!
//! ## 何を固定するか
//!
//! 1. 製品のコード（`crates/*/src`）で起こし済みの子を入れる口（`Job::contain` / `KillScope::contain`）を
//!    呼ぶのは、境界そのもの（[`BOUNDARY`]）と [`ALLOWED`] の場所と件数だけ（probe の起こし済みの子の口・
//!    ペインの ConPTY）。ほかに書いたら `file:line` で落とす
//! 2. 起こして入れる子の呼び手（[`CALLERS`]）が一時停止の口を通っている
//!
//! 振る舞いの番犬（窓の注入を当てても孫まで止まる・`TAKO_2697_LEGACY=1` では残る）は Windows の CI の
//! `crates/tako-control/tests/issue2462_kill_paths_windows.rs` と
//! `crates/tako-core/tests/issue2462_probe_job_windows.rs`。コメントと文字列の中は数えない（`code_view`）。

use std::path::{Path, PathBuf};

#[path = "common/code_view.rs"]
mod code_view;

use code_view::code_view;

/// 起こし済みの子を入れる口（`Job::contains` は問い合わせなので数えない = 後ろが識別子の文字なら外す）
const NEEDLES: &[&str] = &["Job::contain", "KillScope::contain"];

/// 境界 B30 そのもの（一時停止の口の中の 1 段と、自分のテスト。走査しない）
const BOUNDARY: &str = "crates/tako-core/src/platform/job.rs";

/// 起こし済みの子を入れてよい場所（リポジトリルートからの相対パス, 件数, 理由）
const ALLOWED: &[(&str, usize, &str)] = &[
    (
        "crates/tako-core/src/probe.rs",
        1,
        "呼び手が自分で起こした子（器の CLI）を待つ `wait_with_timeout(Child)` の口 = From<Child>",
    ),
    (
        "crates/tako-core/src/terminal.rs",
        1,
        "ペインの ConPTY は alacritty_terminal が CreateProcessW を呼び、作成フラグが固定で一時停止で起こせない",
    ),
];

/// 一時停止の口を通るべき呼び手（相対パス, 中に在るべき呼び出し）
const CALLERS: &[(&str, &str)] = &[
    (
        "crates/tako-control/src/queue_wrapper.rs",
        "KillScope::spawn(",
    ),
    ("crates/tako-app/src/autorename.rs", "KillScope::spawn("),
    (
        "crates/tako-control/src/lsp/server.rs",
        "Job::spawn_contained(",
    ),
    ("crates/tako-core/src/probe.rs", "KillScope::spawn("),
    (
        "crates/tako-control/src/dispatch.rs",
        "probe::spawn_for_wait(",
    ),
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// `crates/*/src/` の `.rs`（リポジトリルートからの相対パス。並びは固定）
fn product_sources() -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
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
    let mut files = Vec::new();
    for krate in std::fs::read_dir(root.join("crates"))
        .expect("crates/ が読める")
        .flatten()
    {
        walk(&krate.path().join("src"), &mut files);
    }
    let mut rels: Vec<String> = files
        .iter()
        .map(|p| {
            p.strip_prefix(&root)
                .expect("ルートの下")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    rels.sort();
    rels
}

/// 起こし済みの子を入れる口の呼び出しの行番号（1 始まり。コメントと文字列は数えない）
fn contain_lines(src: &str) -> Vec<usize> {
    let view = code_view(src);
    let mut lines = Vec::new();
    for (i, line) in view.lines().enumerate() {
        let hit = NEEDLES.iter().any(|needle| {
            line.match_indices(needle).any(|(at, _)| {
                let rest = &line[at + needle.len()..];
                !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_')
            })
        });
        if hit {
            lines.push(i + 1);
        }
    }
    lines
}

/// 1 ファイルの違反（許された件数を超えた・許されていない場所の呼び出し）
fn findings_in(rel: &str, src: &str) -> Vec<String> {
    let lines = contain_lines(src);
    let allowed = ALLOWED
        .iter()
        .find(|(file, _, _)| *file == rel)
        .map_or(0, |(_, n, _)| *n);
    if lines.len() == allowed {
        return Vec::new();
    }
    if lines.len() < allowed {
        return vec![format!(
            "{rel}: 起こし済みの子を入れる口が {} 件（ALLOWED は {allowed} 件）。減ったなら ALLOWED を直す",
            lines.len()
        )];
    }
    lines
        .iter()
        .map(|line| {
            format!(
                "{rel}:{line}: 起こしてから Job へ入れている（#2697 の起動直後の窓 = 入れ終える前に子が起こした孫が \
                 Job の外に残る）。std の Command の子は `job::KillScope::spawn` / `Job::spawn_contained`（probe は \
                 `probe::spawn_for_wait`）で一時停止のまま入れてから動かす"
            )
        })
        .collect()
}

/// 呼び手が一時停止の口を通っていないもの
fn caller_findings(read: impl Fn(&str) -> String) -> Vec<String> {
    CALLERS
        .iter()
        .filter(|(rel, call)| !code_view(&read(rel)).contains(call))
        .map(|(rel, call)| {
            format!(
                "{rel}: `{call}` が無い（一時停止で起こして Job へ入れる口を通っていない。#2697）"
            )
        })
        .collect()
}

#[test]
fn std_のcommandの子を起こしてからjobへ入れない() {
    let mut findings = Vec::new();
    for rel in product_sources().iter().filter(|rel| *rel != BOUNDARY) {
        findings.extend(findings_in(rel, &read(rel)));
    }
    findings.extend(caller_findings(read));
    assert!(findings.is_empty(), "#2697:\n{}", findings.join("\n"));
}

/// 走査が実ファイルで生きている（許された場所の件数が一致する = 見つける側が空振りしていない）
#[test]
fn 走査が空振りしていない() {
    let files = product_sources();
    assert!(
        files.len() > 100,
        "製品のソースの列挙が少ない（{}）",
        files.len()
    );
    assert!(
        !contain_lines(&read(BOUNDARY)).is_empty(),
        "境界の中の入れる口を見つけられない（走査が空振り）"
    );
    for (rel, n, _) in ALLOWED {
        assert!(files.iter().any(|f| f == rel), "{rel} が列挙に無い");
        assert_eq!(contain_lines(&read(rel)).len(), *n, "{rel} の件数");
    }
    for (rel, _) in CALLERS {
        assert!(files.iter().any(|f| f == rel), "{rel} が列挙に無い");
    }
}

/// 修正前の形（起こしてから入れる）を実ファイルへ注入すると `file:line` で名指す
#[test]
fn 注入した修正前の形を名指しする() {
    // queue の取り消し: 一時停止の口を素の spawn + 入れる口へ戻す
    let rel = "crates/tako-control/src/queue_wrapper.rs";
    let src = read(rel);
    let anchor = "    let spawned = tako_core::platform::job::KillScope::spawn(";
    let at = src
        .find(anchor)
        .expect("注入の足場（KillScope::spawn の行）が無い");
    let line = src[..at].lines().count() + 1;
    let injected = format!(
        "{}    let mut child = cmd.spawn().expect(\"x\");\n    let _scope = tako_core::platform::job::KillScope::contain(child.id());\n{}",
        &src[..at],
        &src[at..].replacen("KillScope::spawn(", "KillScope::spawn_gone(", 1)
    );
    let findings = findings_in(rel, &injected);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(
        findings[0].starts_with(&format!("{rel}:{}:", line + 1)),
        "注入した行を名指していない: {findings:?}"
    );
    let callers = caller_findings(|r| if r == rel { injected.clone() } else { read(r) });
    assert!(
        callers.iter().any(|f| f.starts_with(rel)),
        "呼び手が口を通らなくなったのを名指していない: {callers:?}"
    );
    // 許された場所でも件数が増えれば落とす（probe の起こし済みの子の口をもう 1 つ）
    let probe = "crates/tako-core/src/probe.rs";
    let more = format!(
        "{}\nfn extra(pid: u32) {{ let _ = crate::platform::job::KillScope::contain(pid); }}\n",
        read(probe)
    );
    assert!(
        !findings_in(probe, &more).is_empty(),
        "件数の超過を名指していない"
    );
    // 問い合わせ（contains）とコメントの言及は数えない
    assert!(contain_lines("let _ = job.contains(1); // KillScope::contain(pid)\n").is_empty());
    assert!(contain_lines("let _ = Job::contains(&j, 1);\n").is_empty());
    assert_eq!(contain_lines("let j = Job::contain(1);\n").len(), 1);
    assert_eq!(contain_lines("x.and_then(job::Job::contain);\n").len(), 1);
}
