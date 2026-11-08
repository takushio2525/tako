//! **#2775 の番犬**: Windows の CI の通し検査（`scripts/windows/test-setup-windows-2767.ps1`）が、
//! setup の後を**案内どおりの 1 本の `tako setup bootstrap undo-path`**（`--agent` なし）で戻し、
//! ユーザー環境変数 Path をレジストリから読み戻して setup 前の並びと比べ続ける。
//!
//! 修正前の検査は 3 系統分の undo-path（`--agent codex` / `--agent agy` を足して）で戻していたので、
//! 「案内の 1 本では codex / agy の置き場が残る」不具合を緑のまま通していた。検査の本体は Windows の
//! CI でしか走らないので、その形へ戻ったらここ（macOS でも走る）で名指す。
//! 足す側と外す側の範囲そのものは `setup_bootstrap` の単体テスト（`user_path_dirs_for` の 8 通り）が縛る。

use std::path::Path;

const SCRIPT: &str = "scripts/windows/test-setup-windows-2767.ps1";

fn read(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート");
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

/// コメントを除いた行（行番号は 1 始まり）
fn code_lines(text: &str) -> Vec<(usize, &str)> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with('#'))
        .map(|(i, l)| (i + 1, l))
        .collect()
}

/// `function <name>` から次の行頭の `}` まで
fn ps_function<'a>(text: &'a str, name: &str) -> Option<(usize, Vec<&'a str>)> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.trim_start().starts_with(&format!("function {name}")))?;
    let end = lines[start..].iter().position(|l| *l == "}")? + start;
    Some((start + 1, lines[start..=end].to_vec()))
}

#[test]
fn 通し検査はundo_pathを系統を名指さずに叩く() {
    let script = read(SCRIPT);
    let named: Vec<String> = code_lines(&script)
        .into_iter()
        .filter(|(_, l)| l.contains("'undo-path'") && l.contains("'--agent'"))
        .map(|(n, l)| format!("{SCRIPT}:{n}: {}", l.trim()))
        .collect();
    assert!(
        named.is_empty(),
        "通し検査が undo-path を --agent つきで叩いている（#2775）。setup の要約が案内するのは \
         `tako setup bootstrap undo-path` の 1 本なので、系統ごとの undo-path で戻すと「案内の 1 本では \
         残る」不具合を緑のまま通す:\n{}",
        named.join("\n")
    );
    let (line, body) = ps_function(&script, "Invoke-UndoPath")
        .unwrap_or_else(|| panic!("{SCRIPT} に Invoke-UndoPath（案内どおりの 1 本）が無い"));
    let call = body
        .iter()
        .find(|l| l.contains("'undo-path'"))
        .unwrap_or_else(|| panic!("{SCRIPT}:{line}: Invoke-UndoPath が undo-path を叩いていない"));
    assert!(
        !call.contains("--agent"),
        "{SCRIPT}:{line}: Invoke-UndoPath が系統を名指している: {}",
        call.trim()
    );
}

#[test]
fn 通し検査はundo_pathの後にpathをレジストリから読み戻してsetup前と比べる() {
    let script = read(SCRIPT);
    let (line, body) = ps_function(&script, "Assert-UserPathRestored")
        .unwrap_or_else(|| panic!("{SCRIPT} に Assert-UserPathRestored が無い"));
    for (needle, why) in [
        ("Get-TakoUserPath", "レジストリから読み戻していない"),
        (
            "Compare-TakoPathEntries -Expected $Before.Entries",
            "setup 前（検査前）の並びと比べていない",
        ),
    ] {
        assert!(
            body.iter().any(|l| l.contains(needle)),
            "{SCRIPT}:{line}: Assert-UserPathRestored が {why}（{needle} が無い）"
        );
    }

    // undo-path を叩くたびに、直後（3 行以内）で読み戻して比べる
    let lines = code_lines(&script);
    let mut calls = 0;
    for (i, (n, l)) in lines.iter().enumerate() {
        if !l.contains("Invoke-UndoPath '") {
            continue;
        }
        calls += 1;
        let checked = lines[i + 1..]
            .iter()
            .take(3)
            .any(|(_, next)| next.contains("Assert-UserPathRestored"));
        assert!(
            checked,
            "{SCRIPT}:{n}: undo-path の後に Assert-UserPathRestored（setup 前の並びとの比べ）が無い: {}",
            l.trim()
        );
    }
    assert!(
        calls >= 2,
        "{SCRIPT} の undo-path が {calls} 本しかない（setup の後の 1 本と、外し終えた後の 2 回目を見る）"
    );

    // 最初の undo-path は setup の段（1〜3）の直後 = setup が足した分を 1 本で戻す
    let first_undo = lines
        .iter()
        .position(|(_, l)| l.contains("Invoke-UndoPath '"))
        .expect("undo-path の呼び出し");
    let last_setup_before = lines[..first_undo]
        .iter()
        .rposition(|(_, l)| l.contains("Invoke-Tako @('setup', '--yes'"))
        .expect("最初の undo-path より前に setup が無い");
    let between: Vec<&str> = lines[last_setup_before + 1..first_undo]
        .iter()
        .map(|(_, l)| *l)
        .filter(|l| l.contains("'bootstrap'"))
        .collect();
    assert!(
        between.is_empty(),
        "{SCRIPT}: setup と最初の undo-path の間で bootstrap を叩いている（setup が足した分そのものを \
         1 本で戻すのを見なくなる）: {between:?}"
    );
}
