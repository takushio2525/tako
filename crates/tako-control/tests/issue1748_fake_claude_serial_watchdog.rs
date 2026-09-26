//! 偽 claude を起こすテストの直列化の番犬（#1748）
//!
//! # なぜ要るか
//!
//! macOS の std は `Stdio::piped()` のパイプを `pipe()` → `FD_CLOEXEC` の 2 手で作る
//! （原子的に作る `pipe2` が無い）。その隙間に**同じプロセスの別スレッド**が spawn すると、
//! 生まれた子はそのパイプを CLOEXEC 無しで受け継ぎ、exec 後も握り続ける。
//!
//! `crates/tako-app/src/autorename.rs` の上限のテストは、stdout を握ったまま **30 秒眠る孫**を
//! わざと残す。兄弟のテスト（偽 claude を起こして応答を読む 3 本）の stdout の書き込み側を
//! この孫が受け継ぐと、兄弟の読み出しは孫が死ぬまで EOF を見ず、`READ_GRACE`（10 秒）で
//! `ClaudeRun::Failed` へ落ちる = `run_claude_with(..).is_some()` が偽（#1748 の症状）。
//! 起こす瞬間が重ならなければ受け継ぎは起きないので、4 本は `fake_claude_lock()` で直列にしてある。
//!
//! 壊れ方が**まれで静か**なのが厄介で、ロックを取らないテストを 1 本足しても手元の実行は
//! まず緑のまま通り、CI を PR の diff と無関係に赤くする形でだけ現れる（#1748 の実測では
//! 修正前の autorename 28 件を 4 本同時 × 250 周 = 1000 プロセスで 3 件）。なので
//! 「取っているか」をソースで縛る。
//!
//! # 何を縛るか
//!
//! 1. 偽 claude を起こす `#[test]`（本文に [`SPAWNERS`] のどれかがある）は、
//!    **最初に起こすより前に** `fake_claude_lock()` を取る
//! 2. `fake_claude_lock` が本当に排他している（`Mutex` を `lock` している）
//! 3. 眠る偽 claude（孫を残す形）を書くテストは、孫の pid を残させて `StopGrandchild` で止める
//!    （止めないと孫はテストプロセスより長生きし、生まれた瞬間に受け継いだ fd を 30 秒握る）
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば 1〜3 はすべて無意味に緑になるので、[`走査が空振りしていない`] で
//! 起こすテストが 4 本以上見えていることを固定し、[`逆戻りを名指しできる`] で
//! **修正前を再現した注入**が file:line で名指しされることを確かめる。
//! テスト領域の切り出しは #1420 の 1 実装（`tests_only`）、コメント落としは #1609 の 1 実装を通す。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const AUTORENAME: &str = "crates/tako-app/src/autorename.rs";

/// 偽 claude を実際に起こす（または起こす材料を書く）呼び出し
const SPAWNERS: [&str; 3] = ["spawn_claude(", "run_claude_with(", "write_fake_claude("];

/// 直列化の入口
const LOCK: &str = "fake_claude_lock()";

/// 眠る偽 claude（孫を残す形）の目印・孫の pid を残させる綴り・孫を止める後始末
const SLEEPER: &str = "sleep ";
const RECORDS_PID: &str = "echo $! >";
const STOP_GRANDCHILD: &str = "StopGrandchild(";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let path = workspace_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// テスト領域だけを、コメントを落として見る（行番号は保たれる）
fn test_view(src: &str) -> String {
    production_range::code_view::without_comments(&production_range::tests_only(src))
}

/// `#[test]` の付いた関数（名前・宣言行の 1-based 行番号・本文）
struct TestFn {
    name: String,
    line: usize,
    body: String,
}

/// 関数の終わりは宣言行と同じ字下げの `}`
fn test_fns(view: &str) -> Vec<TestFn> {
    let lines: Vec<&str> = view.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() != "#[test]" {
            i += 1;
            continue;
        }
        // `#[test]` と `fn` のあいだの属性（`#[cfg(unix)]` / `#[ignore = …]`）を飛ばす
        let mut j = i + 1;
        while j < lines.len() && lines[j].trim_start().starts_with("#[") {
            j += 1;
        }
        let Some(head) = lines.get(j) else { break };
        // 関数の頭の判定は共有ヘルパを通す（`pub(crate) fn` / `async fn` も頭と読む = #1496）
        let Some(name) = tako_core::source_scan::fn_head_name(head) else {
            i = j;
            continue;
        };
        let indent = head.len() - head.trim_start().len();
        let close = format!("{}}}", " ".repeat(indent));
        let end = lines
            .iter()
            .enumerate()
            .skip(j + 1)
            .find(|(_, l)| **l == close)
            .map(|(k, _)| k)
            .unwrap_or(lines.len() - 1);
        out.push(TestFn {
            name: name.to_string(),
            line: j + 1,
            body: lines[j..=end].join("\n"),
        });
        i = end + 1;
    }
    out
}

/// 最初に起こす位置（本文内のバイト位置）。起こさないテストは `None`
fn first_spawn(body: &str) -> Option<usize> {
    SPAWNERS.iter().filter_map(|s| body.find(s)).min()
}

/// 名前付きの関数の本文（テスト以外のヘルパも含めて探す）
fn fn_body(view: &str, name: &str) -> Option<String> {
    let lines: Vec<&str> = view.lines().collect();
    let start = lines
        .iter()
        .position(|l| tako_core::source_scan::fn_head_name(l) == Some(name))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(k, _)| k)?;
    Some(lines[start..=end].join("\n"))
}

/// 違反の一覧（`file:line — 理由`）
fn offenders(src: &str) -> Vec<String> {
    let view = test_view(src);
    let mut out = Vec::new();
    for t in test_fns(&view) {
        let Some(spawn) = first_spawn(&t.body) else {
            continue;
        };
        match t.body.find(LOCK) {
            None => out.push(format!(
                "{AUTORENAME}:{} — `{}` は偽 claude を起こすのに {LOCK} を取っていない",
                t.line, t.name
            )),
            Some(lock) if lock > spawn => out.push(format!(
                "{AUTORENAME}:{} — `{}` は {LOCK} を起こしたあとで取っている（先に取る）",
                t.line, t.name
            )),
            Some(_) => {}
        }
        let stops = t.body.contains(RECORDS_PID) && t.body.contains(STOP_GRANDCHILD);
        if t.body.contains(SLEEPER) && !stops {
            out.push(format!(
                "{AUTORENAME}:{} — `{}` は眠る孫を残すのに pid（{RECORDS_PID}）を残させて \
                 {STOP_GRANDCHILD}..) で止めていない",
                t.line, t.name
            ));
        }
    }
    match fn_body(&view, "fake_claude_lock") {
        None => out.push(format!(
            "{AUTORENAME} — `fn fake_claude_lock` が見つからない"
        )),
        Some(body) if !(body.contains("Mutex") && body.contains(".lock()")) => out.push(format!(
            "{AUTORENAME} — `fake_claude_lock` が排他していない（Mutex を lock していない）"
        )),
        Some(_) => {}
    }
    out
}

#[test]
fn 偽claudeを起こすテストは直列に並んでいる() {
    let found = offenders(&read(AUTORENAME));
    assert!(
        found.is_empty(),
        "#1748: 偽 claude を起こすテストが直列になっていない。並行する spawn が\n\
         パイプを受け継ぐと兄弟テストが READ_GRACE で落ちる（`.agent/conventions.md`\n\
         「長生きする子を残すテストは、パイプを読むテストと spawn を重ねない」）:\n{}",
        found.join("\n")
    );
}

#[test]
fn 走査が空振りしていない() {
    let view = test_view(&read(AUTORENAME));
    let spawning: Vec<String> = test_fns(&view)
        .into_iter()
        .filter(|t| first_spawn(&t.body).is_some())
        .map(|t| t.name)
        .collect();
    // 修正時点で起こすテストは 4 本（reject / accept / 両方失敗 / 上限）
    assert!(
        spawning.len() >= 4,
        "{AUTORENAME}: 偽 claude を起こすテストが {} 本しか見えない（走査の空振り）: {spawning:?}",
        spawning.len()
    );
    assert!(
        view.contains(SLEEPER) && view.contains(RECORDS_PID),
        "{AUTORENAME}: 眠る偽 claude（{SLEEPER:?} / {RECORDS_PID:?}）が見えない（走査の空振り）"
    );
}

#[test]
fn 逆戻りを名指しできる() {
    let src = read(AUTORENAME);
    let lock_line = "        let _serial = fake_claude_lock();\n";
    assert!(src.contains(lock_line), "注入の足場（{lock_line:?}）が無い");

    // 1. 修正前: どのテストもロックを取らない → 起こす 4 本が全部名指しされる
    let none = src.replace(lock_line, "");
    let found = offenders(&none);
    assert!(
        found
            .iter()
            .filter(|f| f.contains("を取っていない"))
            .count()
            >= 4,
        "ロックを外した注入を 4 本とも名指しできない: {found:?}"
    );
    assert!(
        found
            .iter()
            .all(|f| f.starts_with(&format!("{AUTORENAME}:"))),
        "file:line で名指ししていない: {found:?}"
    );

    // 2. 1 本だけ外す → その 1 本だけが名指しされる
    let target = "fn フラグが通るclaudeでは再試行せず付けたままになる() {\n";
    let one = src.replacen(&format!("{target}{lock_line}"), target, 1);
    assert_ne!(one, src, "注入の足場（{target:?} 直後のロック）が無い");
    let found = offenders(&one);
    assert!(
        found.len() == 1 && found[0].contains("フラグが通るclaudeでは再試行せず付けたままになる"),
        "1 本だけ外した注入を名指しできない: {found:?}"
    );

    // 3. 起こしたあとで取る → 順序違いとして名指しされる
    let late = one.replacen(
        "        let (bin, log) = write_fake_claude(&dir, \"accept\");\n",
        "        let (bin, log) = write_fake_claude(&dir, \"accept\");\n        let _serial = fake_claude_lock();\n",
        1,
    );
    let found = offenders(&late);
    assert!(
        found.len() == 1 && found[0].contains("あとで取っている"),
        "起こしたあとで取る注入を名指しできない: {found:?}"
    );

    // 4. 孫を止めない → 名指しされる
    let orphan = src.replacen("StopGrandchild(grandchild_pid)", "(grandchild_pid)", 1);
    assert_ne!(
        orphan, src,
        "注入の足場（StopGrandchild(grandchild_pid)）が無い"
    );
    let found = offenders(&orphan);
    assert!(
        found.len() == 1 && found[0].contains("止めていない"),
        "孫を止めない注入を名指しできない: {found:?}"
    );
    // 4'. #1748 以前の形（前景の `sleep 30`・pid を残さない）へ戻す → 名指しされる
    let legacy = src.replacen("sleep 30 &\\necho $! > '{}'\\nwait\\n", "sleep 30\\n", 1);
    assert_ne!(legacy, src, "注入の足場（偽 claude の本文）が無い");
    let found = offenders(&legacy);
    assert!(
        found.len() == 1 && found[0].contains("止めていない"),
        "#1748 以前の眠り方を名指しできない: {found:?}"
    );

    // 5. ロックが排他しない（何も取らずに返す）→ 名指しされる
    let noop = src.replacen("LOCK.lock()", "todo!()", 1);
    assert_ne!(noop, src, "注入の足場（LOCK.lock()…）が無い");
    let found = offenders(&noop);
    assert!(
        found.iter().any(|f| f.contains("排他していない")),
        "排他しないロックを名指しできない: {found:?}"
    );
}
