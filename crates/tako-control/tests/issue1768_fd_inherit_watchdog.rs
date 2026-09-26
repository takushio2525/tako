//! **#1768 の番犬**: 長生きする子へ GUI の fd を渡さない。
//!
//! ## なぜ止めるのか
//!
//! ペインの PTY の子と `remote serve` の daemon は fork + exec で起きるので、fork の瞬間に
//! GUI が CLOEXEC 無しで開いていた fd をすべて受け継ぐ。修正前は Metal のシェーダ
//! キャッシュ（Apple のフレームワークが開く）が全ペインの fd 4 / 5 に 100% 入っていた。
//! 直し方は `platform::fd_inherit::seal_inherited_fds` の 1 実装を fork 後・exec 前の
//! **子の中で**走らせること:
//!
//! - PTY: `tty::new` を `fd_inherit::spawn_sealed` で包む（alacritty の `Command` に
//!   `pre_exec` を足す口が無いので、包んだあいだだけ fork の子ハンドラが構える）
//! - daemon: 既存の `pre_exec` の**中で**呼ぶ
//!
//! 挙動は `tako-core/tests/pty_fd_inherit.rs`（実 PTY）と `remote.rs` の単体テスト
//! （daemon の子）が測る。ここが止めるのは**構造**で、1 行で戻せてしまう形が 3 通りある:
//!
//! 1. [`ptyを起こす箇所は掃除を通す`] — 新しい PTY の起動口が `spawn_sealed` に包まれない
//! 2. [`pre_execは掃除を通す`] — 新しい fork + exec の子が掃除を通らない
//! 3. [`掃除の本体は確保しない`] — fork 後・exec 前の子で走る本体（`pre_exec` と
//!    fork の子ハンドラから呼ぶ）が async-signal-safe でなくなる（確保・ロック・出力）

use std::path::{Path, PathBuf};

// 本番コードの範囲取りは 1 実装（#1420）。テスト領域だけを潰し、行番号を保つ
#[path = "common/production_range.rs"]
mod production_range;

/// 掃除の 1 実装（綴りはこの 1 か所に持つ）
const SEAL: &str = "seal_inherited_fds()";
/// PTY の起動を包む口
const WRAP: &str = "spawn_sealed(";
/// 包みの開きから `tty::new` までに許す行数（`spawn_sealed(|| {` の次の行に置く形）
const WRAP_REACH: usize = 3;
const FD_INHERIT: &str = "crates/tako-core/src/platform/fd_inherit.rs";

/// `pre_exec` の中で使ってはいけない字面（確保・ロック・出力）
const NOT_SIGNAL_SAFE: &[&str] = &[
    "Vec",
    "vec!",
    "String",
    "format!",
    "Box",
    "to_string",
    "to_owned",
    "collect",
    "println!",
    "eprintln!",
    "Mutex",
    ".lock(",
    "Arc",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 本番コードの「コードだけの眺め」（コメントと文字列を潰す。行番号は保たれる）。
/// 1 ファイルを名指しで見るときは下限つき（範囲取りが本番コードを飲み込んだら落ちる）
fn code_of(rel: &str, src: &str) -> String {
    production_range::code_view_of(&production_range::production(src, rel))
}

/// 全ファイルを舐める走査用の眺め（下限を課さない。`mod.rs` のような数百バイトの
/// ファイルも走査に入るので、1 ファイルごとの下限は当てはまらない）
fn code_of_any(src: &str) -> String {
    production_range::code_view_of(&production_range::scan(src).text)
}

/// `crates/*/src` 配下の `.rs` を `(repo 相対パス, 原文)` で全件返す
fn production_sources() -> Vec<(String, String)> {
    let root = repo_root();
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = std::fs::read_dir(root.join("crates"))
        .expect("crates/ を読める")
        .filter_map(|e| e.ok().map(|e| e.path().join("src")))
        .filter(|p| p.is_dir())
        .collect();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("src 配下を読める").flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let rel = path
                    .strip_prefix(&root)
                    .expect("リポジトリ配下")
                    .to_string_lossy()
                    .replace('\\', "/");
                let src = std::fs::read_to_string(&path).expect("読める");
                out.push((rel, src));
            }
        }
    }
    out.sort();
    out
}

/// 規則 1: PTY を起こす呼び出し（`tty::new(` / `tty::from_fd(`）ごとに、その行か
/// 直前 [`WRAP_REACH`] 行に `spawn_sealed(` があるか（= 包まれているか）。違反は `file:line`
fn pty_spawns_without_seal(rel: &str, code: &str) -> Vec<String> {
    let lines: Vec<&str> = code.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !(line.contains("tty::new(") || line.contains("tty::from_fd(")) {
            continue;
        }
        let start = i.saturating_sub(WRAP_REACH);
        if !lines[start..=i].iter().any(|l| l.contains(WRAP)) {
            out.push(format!(
                "{rel}:{}: PTY の起動を platform::fd_inherit::{WRAP}…) で包んでいない",
                i + 1
            ));
        }
    }
    out
}

/// 規則 2: `pre_exec(` の閉包ごとに、`Ok(())` で閉じるまでに掃除を呼んでいるか
fn pre_execs_without_seal(rel: &str, code: &str) -> Vec<String> {
    let lines: Vec<&str> = code.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !line.contains("pre_exec(") {
            continue;
        }
        let end = (i..lines.len())
            .find(|&j| lines[j].contains("Ok(())"))
            .unwrap_or(lines.len() - 1);
        if !lines[i..=end].iter().any(|l| l.contains(SEAL)) {
            out.push(format!(
                "{rel}:{}: pre_exec の中で platform::fd_inherit::{SEAL} を呼んでいない",
                i + 1
            ));
        }
    }
    out
}

/// 規則 3: 掃除の本体（公開の入口と unix の `mod sys`）に確保・ロック・出力の字面が無いか
fn unsafe_in_seal_body(rel: &str, code: &str) -> Vec<String> {
    let lines: Vec<&str> = code.lines().collect();
    let mut ranges = Vec::new();
    // 公開の入口: `pub fn seal_inherited_fds` から列 0 の `}` まで
    if let Some(s) = lines
        .iter()
        .position(|l| l.starts_with("pub fn seal_inherited_fds"))
    {
        let e = (s..lines.len()).find(|&j| lines[j] == "}").unwrap_or(s);
        ranges.push((s, e));
    }
    // unix の器: `#[cfg(unix)]` 直後の `mod sys {` から列 0 の `}` まで
    if let Some(s) = (1..lines.len())
        .find(|&j| lines[j].starts_with("mod sys {") && lines[j - 1].contains("cfg(unix)"))
    {
        let e = (s..lines.len()).find(|&j| lines[j] == "}").unwrap_or(s);
        ranges.push((s, e));
    }
    let mut out = Vec::new();
    if ranges.len() < 2 {
        out.push(format!(
            "{rel}: 掃除の本体（pub fn seal_inherited_fds / #[cfg(unix)] mod sys）が見つからない"
        ));
    }
    for (s, e) in ranges {
        for (j, line) in lines.iter().enumerate().take(e + 1).skip(s) {
            if let Some(word) = NOT_SIGNAL_SAFE.iter().find(|w| line.contains(**w)) {
                out.push(format!(
                    "{rel}:{}: fork 後の子で走る掃除の本体に `{word}` がある（async-signal-safe でない）",
                    j + 1
                ));
            }
        }
    }
    out
}

fn fail_if_any(what: &str, violations: Vec<String>) {
    assert!(
        violations.is_empty(),
        "{what}（#1768。`.agent/conventions.md`「長生きする子へ GUI の fd を渡さない」）:\n{}",
        violations.join("\n")
    );
}

#[test]
fn ptyを起こす箇所は掃除を通す() {
    let mut violations = Vec::new();
    let mut spawns = 0;
    for (rel, src) in production_sources() {
        let code = code_of_any(&src);
        spawns += code.matches("tty::new(").count() + code.matches("tty::from_fd(").count();
        violations.extend(pty_spawns_without_seal(&rel, &code));
    }
    assert!(
        spawns >= 1,
        "PTY を起こす呼び出しが 1 つも見つからない（走査が空振り）"
    );
    fail_if_any("PTY の子へ GUI の fd が漏れる", violations);
}

#[test]
fn pre_execは掃除を通す() {
    let mut violations = Vec::new();
    let mut closures = 0;
    for (rel, src) in production_sources() {
        let code = code_of_any(&src);
        closures += code.matches("pre_exec(").count();
        violations.extend(pre_execs_without_seal(&rel, &code));
    }
    assert!(
        closures >= 1,
        "pre_exec が 1 つも見つからない（走査が空振り）"
    );
    fail_if_any("fork + exec の子へ GUI の fd が漏れる", violations);
}

#[test]
fn 掃除の本体は確保しない() {
    let src =
        std::fs::read_to_string(repo_root().join(FD_INHERIT)).expect("fd_inherit.rs を読める");
    fail_if_any(
        "掃除の本体が fork 後の子の中で呼べない",
        unsafe_in_seal_body(FD_INHERIT, &code_of(FD_INHERIT, &src)),
    );
}

#[test]
fn 検査は実際の違反を検出する() {
    // 規則 1: 掃除を通さない PTY の起動を名指す / 通していれば黙る
    let bare = "fn spawn() {\n    let x = 1;\n    let pty = tty::new(&o, w, 0);\n}\n";
    assert_eq!(
        pty_spawns_without_seal("t.rs", bare),
        vec![format!(
            "t.rs:3: PTY の起動を platform::fd_inherit::{WRAP}…) で包んでいない"
        )]
    );
    let wrapped = "fn spawn() {\n    let pty = crate::platform::fd_inherit::spawn_sealed(|| {\n        tty::new(&o, w, 0)\n    });\n}\n";
    assert!(pty_spawns_without_seal("t.rs", wrapped).is_empty());
    // 親で掃いてから包まずに起こす形（#1768 で退けた案）は包んだことにならない
    let parent_seal = "fn spawn() {\n    seal_inherited_fds();\n    let x = 1;\n    let y = 2;\n    let z = 3;\n    tty::from_fd(&o, 0, m, s);\n}\n";
    assert_eq!(pty_spawns_without_seal("t.rs", parent_seal).len(), 1);

    // 規則 2: 掃除を通さない pre_exec を名指す / 通していれば黙る
    let bare_exec = "cmd.pre_exec(|| {\n    libc::setsid();\n    Ok(())\n});\n";
    assert_eq!(
        pre_execs_without_seal("t.rs", bare_exec),
        vec![format!(
            "t.rs:1: pre_exec の中で platform::fd_inherit::{SEAL} を呼んでいない"
        )]
    );
    let sealed_exec =
        "cmd.pre_exec(|| {\n    libc::setsid();\n    tako_core::platform::fd_inherit::seal_inherited_fds();\n    Ok(())\n});\n";
    assert!(pre_execs_without_seal("t.rs", sealed_exec).is_empty());

    // 規則 3: 本体に確保が生えたら名指す
    let body = "pub fn seal_inherited_fds() -> usize {\n    sys::seal_all()\n}\n#[cfg(unix)]\nmod sys {\n    pub fn seal_all() -> usize {\n        let v: Vec<i32> = Vec::new();\n        0\n    }\n}\n";
    let found = unsafe_in_seal_body("t.rs", body);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].starts_with("t.rs:7: "), "{found:?}");
    // 本体が見つからない形（名前が変わった）も黙らない
    assert_eq!(unsafe_in_seal_body("t.rs", "fn other() {}\n").len(), 1);
}
