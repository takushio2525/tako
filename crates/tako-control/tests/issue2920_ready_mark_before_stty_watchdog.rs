//! **#2920 の番犬**: テストのシェルは、待ち手が「受け取れる」と読む合図を端末のモードを
//! 変える `stty` の**後**に出す。
//!
//! ## 何が起きたのか
//!
//! `dispatch::tests::issue2826_プロンプトを出したシェルへは修正前とバイト一致で即時に書く` は
//! 子のシェルに `printf '<OSC 133;A>'; stty raw -echo; exec cat > <file>` を走らせ、
//! `shell_marks().prompts >= 1`（= 印を受けた）を待ってから `echo hi` + Enter を書き、
//! ファイルに落ちたバイトを修正前（legacy）と突き合わせていた。印は `stty` の**前**に出るので、
//! 待ちを抜けた送信が raw になる前の端末へ届く回は、端末の ICRNL が `\r` を `\n` に変えて
//! `"echo hi\n"` が落ちる。製品の差ではなくテストの窓で比較が崩れ、CI の macOS で間欠的に
//! 落ちていた（PR #2919 で 2 回。印と `stty` の間に `sleep 0.3` を入れると毎回落ちる）。
//!
//! 直し方は合図を `stty` の後へ動かすだけ（`stty raw -echo; printf '<OSC 133;A>'; exec cat …`）。
//! raw にした後に届いた入力は `cat` が起きるまで端末の入力キューに raw のまま残るので、
//! 待ちの意味（プロンプトを出したシェル）は変わらない。
//!
//! ## ここで止めるもの
//!
//! **同じ行の中で、合図（`133;A` = プロンプトの印 / `READY` = 受け手の準備の印）が、端末の
//! 入力の扱いを変える `stty`（`raw` / `-echo` / `-icanon` / `-icrnl` / `cbreak`）より前に
//! ある**行を `file:line` で名指す。対象は `crates/` の `.rs` と `scripts/` の `.sh`。
//! `stty size` / `stty -g` / `stty sane`（読むだけ・戻すだけ）は数えない。コメントの行は見ない。
//!
//! 行をまたぐ形（ヒアドキュメントの偽エージェントが起動の 1 行を出してから次の行で `stty` を
//! 打つ）はここでは見ない。#2920 の調査時点で該当するのは `scripts/` の claude のスタブ 3 本で、
//! 送り手は起動の 1 行ではなく送達フロー（入力欄への映りを確かめて貼り直す・TUI でなければ
//! 2 秒待つ）を通るので、この窓を持たない。
//!
//! 意図して残す形は、その行に `tako:stty-order-ok <理由>` を書く。

use std::path::{Path, PathBuf};

/// この番犬自身（禁止形の文字列そのものを本文に持つので走査から外す）
const SELF_FILE: &str = "issue2920_ready_mark_before_stty_watchdog.rs";

/// 待ち手が「受け取れる」と読む合図
const READY_MARKS: &[&str] = &["133;A", "READY"];

/// 端末の入力の扱いを変える `stty` の引数
const STTY_MODES: &[&str] = &["raw", "-echo", "-icanon", "-icrnl", "cbreak"];

/// 意図して残す形の宣言
const OK_MARK: &str = "tako:stty-order-ok";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリのルートを解決できる")
}

fn walk(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path
                .file_name()
                .is_some_and(|n| n == "target" || n == "node_modules")
            {
                continue;
            }
            walk(&path, ext, out);
        } else if path.extension().is_some_and(|e| e == ext) {
            out.push(path);
        }
    }
}

fn rel(path: &Path) -> String {
    path.strip_prefix(repo_root())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// 行の中で、端末のモードを変える最初の `stty` の位置（`;` までの引数にモードの語がある）。
/// 語の途中（`Ghostty` ほか）は数えない
fn mode_stty_at(line: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(off) = line[from..].find("stty") {
        let at = from + off;
        let word_start = line[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        if !word_start {
            from = at + "stty".len();
            continue;
        }
        let args = &line[at + "stty".len()..];
        let args = args.split(';').next().unwrap_or(args);
        let changes_mode = args
            .split(|c: char| c.is_whitespace() || matches!(c, '\'' | '"' | ',' | '(' | ')'))
            .any(|word| STTY_MODES.contains(&word));
        if changes_mode {
            return Some(at);
        }
        from = at + "stty".len();
    }
    None
}

/// 合図が `stty` より前にある行なら、その合図を返す
fn mark_before_stty(line: &str) -> Option<&'static str> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with('#') || line.contains(OK_MARK) {
        return None;
    }
    let stty = mode_stty_at(line)?;
    READY_MARKS
        .iter()
        .copied()
        .find(|mark| line[..stty].contains(mark))
}

fn violations() -> (usize, Vec<String>) {
    let root = repo_root();
    let mut files = Vec::new();
    walk(&root.join("crates"), "rs", &mut files);
    walk(&root.join("scripts"), "sh", &mut files);
    files.sort();
    let mut found = Vec::new();
    for path in &files {
        if path.file_name().is_some_and(|n| n == SELF_FILE) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            if let Some(mark) = mark_before_stty(line) {
                found.push(format!(
                    "{}:{}: 合図 `{mark}` が stty より前: {}",
                    rel(path),
                    i + 1,
                    line.trim()
                ));
            }
        }
    }
    (files.len(), found)
}

#[test]
fn issue2920_テストのシェルは合図をsttyの後に出す() {
    let (scanned, found) = violations();
    assert!(
        scanned > 300,
        "走査が壊れている（.rs / .sh が {scanned} 件しか見つからない）"
    );
    assert!(
        found.is_empty(),
        "合図を端末のモードを変える stty より前に出している（#2920）。待ちを抜けた送信が raw に\n\
         なる前の端末へ届くと ICRNL が \\r を \\n に変え、エコーも混ざる。合図を stty の後へ\n\
         動かす（例: `stty raw -echo; printf '<印>'; exec cat > …`）。意図して残すなら行に\n\
         `{OK_MARK} <理由>`:\n{}",
        found.join("\n")
    );
}

/// 検出器そのもの: #2920 の修正前の行と、合図の別名・perl の形を名指す
#[test]
fn issue2920_検出器は合図が先の行を名指す() {
    let cases = [
        // #2920 の修正前（dispatch のテストのシェル）
        (
            r#""printf '\\033]133;A\\007'; stty raw -echo; exec cat > '{}'","#,
            "133;A",
        ),
        // 受け手の準備の印を先に出す
        (
            r#""stty -g; printf SINK-READY; stty raw -echo; exec cat > '{}'","#,
            "READY",
        ),
        // perl の system の形
        (
            r#"print "VIS-READY\r\n"; system('/bin/stty', 'raw', '-echo');"#,
            "READY",
        ),
        // -icanon だけでも入力の扱いが変わる
        (
            "printf 'READY'; stty -echo -icanon -icrnl min 1 time 0",
            "READY",
        ),
    ];
    for (line, mark) in cases {
        assert_eq!(mark_before_stty(line), Some(mark), "名指さない: {line}");
    }
}

/// 検出器そのもの: 修正後の形・読むだけ / 戻すだけの stty・コメント・宣言は名指さない
#[test]
fn issue2920_検出器は修正後の形と無関係な行を名指さない() {
    let cases = [
        // #2920 の修正後
        r#""stty raw -echo; printf '\\033]133;A\\007'; exec cat > '{}'","#,
        r#""{request}stty raw -echo; printf SINK-READY; exec cat > '{}'","#,
        // 読むだけ・戻すだけの stty
        "printf READY; stty size",
        "printf READY; rows=$(stty size)",
        "printf READY; stty sane 2>/dev/null",
        "printf READY; saved=$(stty -g)",
        // コメントの行
        "// 印（133;A）を stty raw の前に出すと窓が開く",
        "# READY を出してから stty raw する形は窓が開く",
        // 宣言つき
        "printf READY; stty raw -echo # tako:stty-order-ok 送り手は 2 秒待つ",
        // stty が無い
        "printf '\\033]133;A\\007\\033[?1049h'; exec sleep 600",
        // 語の途中の stty（Ghostty の後ろに raw の語があっても数えない）
        "let s = \"READY Ghostty raw\";",
    ];
    for line in cases {
        assert_eq!(mark_before_stty(line), None, "誤って名指した: {line}");
    }
}
