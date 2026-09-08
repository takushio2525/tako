//! 復元用 claude セッション ID の扱いの番犬（#1076）
//!
//! #1076 は 2 つの形で「タブのタイトルだけ残って claude が居ない」を作っていた。
//! どちらも**壊れ方が同じ場所に戻ってきやすい**ので、ここで機械的に禁じる。
//!
//! 1. **保存済み ID をスキャン結果で丸ごと置き換えていた**
//!    （`claude agents --json` は「いま動いているか」しか答えないので、
//!    再起動直後は resume した claude が起動途中で 1 件も出ない → 全部消える →
//!    次の再起動で復元できない）。落とし方の正本は
//!    [`tako_core::claude_resume::ResumeIds`]（確認してから外す）で、
//!    `main.rs` はこの API 越しにしか触ってはいけない
//! 2. **resume コマンドを復元経路が自前で組んでいた**（`claude --resume <id>` の
//!    最小形。役割 env / `--model` / `--effort` が落ちるので、戻ってきた claude は
//!    master / worker として認識されない）。組み立ての正本は
//!    `tako_control::sessions`（`resume_command` / `restore_plan`）1 か所

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// 行コメント（規約の説明文・doc コメント）を落とした本文
fn code_lines(source: &str) -> Vec<(usize, &str)> {
    source
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l))
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .collect()
}

const MAIN: &str = "crates/tako-app/src/main.rs";
const SESSIONS: &str = "crates/tako-control/src/sessions.rs";

/// 1: GUI 側は保持規則の API 越しにしか ID マップを触らない
#[test]
fn 復元用idマップを丸ごと置き換えない() {
    let source = read(MAIN);
    // `= ...` での代入・`clear()` は「不在 = 終了」と決めつける形の入口。
    // 許すのは `seed` / `apply_scan` / `remove` / 読み出し（`get` 等）と、
    // A/B の `replace_all_legacy`（旧挙動の再現専用で `legacy_1076()` の中だけ）
    let mut bad = Vec::new();
    for (line_no, line) in code_lines(&source) {
        if !line.contains("claude_resume_sessions") {
            continue;
        }
        // フィールドそのものへの代入 / 全消しだけを見る
        // （`let x = &self.claude_resume_sessions;` のような読み出しの束縛は対象外）
        let assigns = [
            "self.claude_resume_sessions =",
            "app.claude_resume_sessions =",
        ]
        .iter()
        .any(|pat| line.contains(pat))
            || line.contains("claude_resume_sessions.clear()");
        if assigns {
            bad.push(format!("{MAIN}:{line_no}: {}", line.trim()));
        }
    }
    assert!(
        bad.is_empty(),
        "復元用 ID マップを丸ごと置き換え / 全消ししている（#1076 の根因）。\n\
         保持規則は tako_core::claude_resume::ResumeIds（確認してから外す）が正本で、\n\
         seed / apply_scan / remove 越しに触ること:\n  {}",
        bad.join("\n  ")
    );
}

/// 1': 「子プロセスが無い」だけで全消しする形が戻ってこないこと。
/// 前段ガード（`has_children`）が false の分岐は、**不在 1 回**として
/// 保持規則へ渡さなければならない
#[test]
fn 子プロセス不在の分岐は保持規則を通る() {
    let source = read(MAIN);
    let idx = source
        .find("if !has_children {")
        .expect("has_children の前段ガードが見つからない（#368 の構造が変わった）");
    let block = &source[idx..idx + 900];
    assert!(
        block.contains("apply_claude_resume_sessions(&[])"),
        "has_children が false の分岐が保持規則を通っていない（#1076）。\n\
         再起動直後は claude が起動途中でどのペインも子を持たないので、\n\
         ここで捨てると復元の種を自分で失う:\n{block}"
    );
}

/// 2: 復元経路は resume コマンドを自前で組まない（正本は sessions 側 1 か所）
#[test]
fn 復元経路はresumeコマンドを自前で組まない() {
    let source = read(MAIN);
    // テストモジュール（`mod persist_resume_tests`）は期待値としてコマンド文字列を書くので対象外
    let body = match source.find("mod persist_resume_tests {") {
        Some(i) => {
            let end = source[i..]
                .find("\n#[cfg(test)]\nmod ")
                .map(|e| i + e)
                .unwrap_or(source.len());
            format!("{}{}", &source[..i], &source[end..])
        }
        None => source.clone(),
    };
    let mut bad = Vec::new();
    for (line_no, line) in code_lines(&body) {
        if line.contains("--resume") {
            bad.push(format!("{line_no}: {}", line.trim()));
        }
    }
    assert!(
        bad.is_empty(),
        "復元経路が resume コマンドを自前で組んでいる（#1076）。\n\
         組み立ての正本は tako_control::sessions::restore_plan（= resume_command と同じ形）で、\n\
         2 つ目を置くと片方だけが起動条件（役割 env / --model / --effort）を落とす:\n  {}",
        bad.join("\n  ")
    );
    // 正本を実際に呼んでいること（判断ごと再実装したら消える呼び出し）
    assert!(
        source.contains("tako_control::sessions::restore_plan("),
        "復元経路が sessions::restore_plan を呼んでいない（#1076）"
    );
}

/// 2': `restore_plan` は `resume_command` と同じ組み立てを使う
/// （カタログの記録があるときに最小形へ落ちない）
#[test]
fn restore_planはresume_commandの組み立てを共有する() {
    let source = read(SESSIONS);
    let idx = source
        .find("pub fn restore_plan_in(")
        .expect("restore_plan_in が見つからない");
    let end = source[idx..]
        .find("\npub fn restore_plan(")
        .map(|e| idx + e)
        .expect("restore_plan が見つからない");
    let body = &source[idx..end];
    assert!(
        body.contains("resume_command_with_env("),
        "restore_plan が resume_command の組み立てを共有していない（#1076）:\n{body}"
    );
}
