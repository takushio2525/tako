//! 番犬: ssh の**見送り**を「判定のたびに」診断ログへ書いていない（#1258）
//!
//! ## なぜ止めるのか
//!
//! `SshScanState.skipped` は**走査を間引いた tick でも前回ぶんが載っている**
//! （`ssh_detect::scan` が「見ていない」を「消えた」と読み替えないための仕様）。
//! そこを 2 秒 tick のたびに素朴に回して `persist_log` すると、ペインの ssh が
//! 生きているあいだ同じ行が積もる。実測 = 非対話 ssh が約 3 時間ぶら下がって
//! **866 行**（#1258）。送達・復元・kill の行が埋もれて `grep` で追う運用
//! （#770 / #790）が壊れる。
//!
//! 正しい形は `ssh_detect::SshSkipLog::take_new`（鍵 = ペイン × ssh の pid ×
//! 理由）を通してから書くこと。いま見送られている姿は `remote-folder auto` の
//! `skipped` がいつでも返すので、ログに要るのは「いつ起きたか」の 1 行だけ。
//!
//! ## 何を違反とするか（**誤検知しない形**）
//!
//! `for … in <式> {` の**回す側の式**が `skipped` を含み（= 持ち越される一覧を
//! そのまま回している）、その本体が `persist_log` を呼ぶ形だけを落とす。
//! 回す側が `take_new(` を通っていれば対象外なので、
//! `for s in log.take_new(&state.skipped, legacy)` は違反にならない。
//! ループ変数の名前（`for skipped in &fresh`）は回す側ではないので当たらない。

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

/// `crates/*/src/` 配下の `.rs`
fn sources() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(crates) = std::fs::read_dir(repo_root().join("crates")) else {
        return out;
    };
    for entry in crates.flatten() {
        let mut stack = vec![entry.path().join("src")];
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

/// `//` から行末までを空白へ潰す（**行数を変えない** = 行番号がそのまま数えられる）
fn without_comments(src: &str) -> String {
    src.lines()
        .map(|line| match line.find("//") {
            Some(i) => format!("{}{}", &line[..i], " ".repeat(line.len() - i)),
            None => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 違反の行番号（1 始まり）と該当行を返す。`code` はコメントを潰した後のソース
fn offenders_in(code: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = code.lines().collect();
    let mut out = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("for ") {
            continue;
        }
        let Some((_, iterable)) = trimmed.split_once(" in ") else {
            continue;
        };
        // 回す側が持ち越される一覧そのものか（`take_new` を通っていれば健全）
        if !iterable.contains("skipped") || iterable.contains("take_new") {
            continue;
        }
        // 本体（`{` から対応する `}` まで）に診断ログの書き出しがあるか
        let mut depth = 0i32;
        let mut body = String::new();
        let mut started = false;
        for l in lines.iter().skip(idx) {
            for ch in l.chars() {
                match ch {
                    '{' => {
                        depth += 1;
                        started = true;
                    }
                    '}' => depth -= 1,
                    _ => {}
                }
            }
            body.push_str(l);
            body.push('\n');
            if started && depth <= 0 {
                break;
            }
        }
        if body.contains("persist_log") {
            out.push((idx + 1, trimmed.to_string()));
        }
    }
    out
}

#[test]
fn 見送りを判定のたびに診断ログへ書いていない() {
    let mut offenders: Vec<String> = Vec::new();
    for path in sources() {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (line, text) in offenders_in(&without_comments(&raw)) {
            let rel = path
                .strip_prefix(repo_root())
                .unwrap_or(&path)
                .display()
                .to_string();
            offenders.push(format!("{rel}:{line}: {text}"));
        }
    }
    assert!(
        offenders.is_empty(),
        "`skipped` は走査を間引いた tick でも前回ぶんが載っているので、そのまま回して\n\
         `persist_log` すると同じ行が ssh の生存中ずっと積もる（実測 3 時間で 866 行 = #1258）。\n\
         `ssh_detect::SshSkipLog::take_new`（ペイン × pid × 理由）を通してから書くこと:\n  {}",
        offenders.join("\n  ")
    );
}

/// `SkippedSsh` が `pid` を公開しているか（`code` はコメントを潰した後のソース）
fn struct_has_pid(code: &str) -> bool {
    let Some(at) = code.find("pub struct SkippedSsh") else {
        return false;
    };
    let end = code[at..].find('}').map(|i| at + i).unwrap_or(code.len());
    code[at..end].contains("pub pid: u32")
}

#[test]
fn 見送りは_pid_まで持ち帰る() {
    // pid が鍵から落ちると (ペイン, 理由) だけになり、ssh を打ち直しても
    // 二度と記録されない（= 抑止しすぎ）。構造体の側でそれを止める
    let path = repo_root().join("crates/tako-control/src/ssh_detect.rs");
    let src = std::fs::read_to_string(&path).expect("ssh_detect.rs を読めない");
    assert!(
        struct_has_pid(&without_comments(&src)),
        "SkippedSsh が pid を持っていない = 見送りログの鍵が (ペイン, 理由) に\n\
         縮み、ssh を打ち直しても二度と記録されない（#1258）: {}",
        path.display()
    );
    // 検出力（合成）: 別名へ替えた形・落とした形は通さない
    assert!(struct_has_pid(
        "pub struct SkippedSsh {\n    pub pane: u64,\n    pub pid: u32,\n}"
    ));
    assert!(!struct_has_pid(
        "pub struct SkippedSsh {\n    pub pane: u64,\n    pub proc_id: u32,\n}"
    ));
    assert!(!struct_has_pid(
        "pub struct SkippedSsh {\n    pub pane: u64,\n}"
    ));
}

#[test]
fn 番犬が走査対象と検出力を持っている() {
    let files = sources();
    assert!(
        files.len() > 50,
        "crates 配下の .rs をほとんど拾えていない: {} 件",
        files.len()
    );
    assert!(
        files
            .iter()
            .any(|p| p.ends_with("tako-app/src/ssh_folders.rs")),
        "#1258 の現場（ssh_folders.rs）が走査対象に入っていない"
    );
    // 修正前の形（合成）を確かに落とす
    let before = "        for skipped in &state.skipped {\n\
                  \x20           tako_control::diag::persist_log(&format!(\"見送り: {}\", skipped.pane));\n\
                  \x20       }\n";
    assert_eq!(
        offenders_in(before).len(),
        1,
        "修正前の形を検出できていない: {before}"
    );
    // 直した形（合成）は素通りする
    let after = "        let fresh = self.ssh_skip_log.take_new(&state.skipped, legacy);\n\
                 \x20       for skipped in &fresh {\n\
                 \x20           tako_control::diag::persist_log(&format!(\"見送り: {}\", skipped.pane));\n\
                 \x20       }\n";
    assert!(
        offenders_in(after).is_empty(),
        "直した形を誤検知している: {after}"
    );
    // `take_new` を回す側に直接書いた形も素通りする
    let inline = "        for skipped in self.ssh_skip_log.take_new(&state.skipped, legacy) {\n\
                  \x20           tako_control::diag::persist_log(\"見送り\");\n\
                  \x20       }\n";
    assert!(
        offenders_in(inline).is_empty(),
        "take_new 経由を誤検知している: {inline}"
    );
}
