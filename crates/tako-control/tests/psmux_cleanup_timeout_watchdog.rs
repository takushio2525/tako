//! 番犬: 器（psmux）を叩く経路が**期限なしで待っていない**（#1271）
//!
//! ## なぜ止めるのか
//!
//! `Command::output()` / `Command::status()` / `Child::wait()` には**タイムアウトが
//! 無い**。psmux のクライアントは返ってこないことがあり（実測: `kill-server` と
//! `send-keys -X scroll-up` が CPU 6% のまま返らない）、そこで無限に待つと
//!
//! 1. その run のテストが終わらない（バイナリ全体が固まる）
//! 2. **`Fixture::drop` が 1 つも走らない**ので器（サーバー）と中の pwsh が残る
//!
//! の 2 段で効く。#1114 の作業中の実測では 1 回の実行あたり器が約 2 個残り、
//! 半日で 25 → 74 個まで積み上がって機が目に見えて遅くなった（= 負荷起因の
//! フレークを自分で増幅する）。しかもこのビルドの psmux は**サーバーの名前が
//! `tmux.exe`** なので `Get-Process psmux` では残骸が 0 に見える。
//!
//! 直し方は `crates/tako-core/tests/common/psmux_ctl.rs` の 1 実装へ寄せること
//! （期限つきで待ち、返らなければ子を殺し、器が生き残っていれば **pid 指定**で止める）。
//!
//! ## 何を違反とするか（**誤検知しない形**）
//!
//! 走査するのは器を叩く 3 ファイルだけ。コメントと文字列は
//! `code_view` で潰してから見る（この番犬の説明文や assert の期待値を拾わないため）。
//!
//! | 形 | なぜ落とすか |
//! |---|---|
//! | `.output()` | 子の終了と EOF を**期限なし**で待つ |
//! | `.status()` | 同上（出力を捨てるだけで待ちは同じ） |
//! | `.wait_with_output()` | 同上 |
//! | `.wait()` で**直前 3 行に `.kill()` が無い**もの | 生きている子を期限なしで待つ |
//!
//! 対象外:
//!
//! | 形 | 例 | なぜ対象外か |
//! |---|---|---|
//! | 殺した子の刈り取り | `let _ = child.kill();` の直後の `let _ = child.wait();` | 相手は既に死んでいる |
//! | 期限つきの覗き見 | `child.try_wait()` | 待たない |
//! | A/B の旧アーム | `fn legacy_kill_server` | 旧の無限待ちを**わざと**再現する。`legacy_` 接頭辞で除外 |

use std::path::{Path, PathBuf};

#[path = "common/code_view.rs"]
mod code_view_mod;
use code_view_mod::code_view;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

/// 器を叩く 3 ファイル（後始末の経路はこの中にしか無い）
fn targets() -> Vec<PathBuf> {
    [
        "crates/tako-core/tests/common/psmux_ctl.rs",
        "crates/tako-core/tests/psmux_backend.rs",
        "crates/tako-core/tests/shell_integration_powershell.rs",
    ]
    .iter()
    .map(|rel| repo_root().join(rel))
    .collect()
}

/// 違反 1 件（行番号は 1 始まり）
#[derive(Debug, PartialEq, Eq)]
struct Hit {
    line: usize,
    kind: &'static str,
    text: String,
}

/// 期限なしで待つ形を拾う。`src` は生のソース（潰しはこの中で行う）
fn hits(src: &str) -> Vec<Hit> {
    let code = code_view(src);
    let code_lines: Vec<&str> = code.lines().collect();
    let raw_lines: Vec<&str> = src.lines().collect();
    let skip = legacy_lines(&code_lines);

    let mut out = Vec::new();
    for (i, line) in code_lines.iter().enumerate() {
        if skip.contains(&i) {
            continue;
        }
        let kind = if line.contains(".output()") {
            Some("期限なしの `Command::output()`")
        } else if line.contains(".wait_with_output()") {
            Some("期限なしの `Child::wait_with_output()`")
        } else if line.contains(".status()") {
            Some("期限なしの `Command::status()`")
        } else if line.contains(".wait()") && !killed_just_before(&code_lines, i) {
            Some("殺していない子を `Child::wait()` で待っている")
        } else {
            None
        };
        if let Some(kind) = kind {
            out.push(Hit {
                line: i + 1,
                kind,
                text: raw_lines.get(i).unwrap_or(&"").trim().to_string(),
            });
        }
    }
    out
}

/// `.wait()` の直前 3 行以内に `.kill()` があるか（= 既に殺した子の刈り取り）
fn killed_just_before(lines: &[&str], at: usize) -> bool {
    let from = at.saturating_sub(3);
    lines[from..=at].iter().any(|l| l.contains(".kill()"))
}

/// `fn legacy_…(` から同じインデントの `}` までの行番号（A/B の旧アーム = 対象外）。
/// 採り方は `psmux_e2e_wait_watchdog` と同じ
fn legacy_lines(lines: &[&str]) -> Vec<usize> {
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !line.trim_start().starts_with("fn legacy_") {
            continue;
        }
        let indent = indent_of(line);
        let end = lines
            .iter()
            .enumerate()
            .skip(i + 1)
            .find(|(_, l)| l.trim() == "}" && indent_of(l) == indent)
            .map(|(j, _)| j)
            .unwrap_or(lines.len() - 1);
        out.extend(i..=end);
    }
    out
}

/// 行まるごとのコメントだけ落とした眺め（文字列リテラルは残す）。
/// 走査対象の 3 ファイルには行コメントしか無い（`code_view` の潰しが効く形は
/// そちらで見る）ので、これで説明文と実装を分けられる
fn without_comment_lines(src: &str) -> String {
    src.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

#[test]
fn 器を叩く経路が期限なしで待っていない() {
    let mut found: Vec<String> = Vec::new();
    for path in targets() {
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} を読める: {e}", path.display()));
        let rel = path
            .strip_prefix(repo_root())
            .unwrap_or(&path)
            .display()
            .to_string();
        for hit in hits(&src) {
            found.push(format!("  {rel}:{} {}: {}", hit.line, hit.kind, hit.text));
        }
    }
    assert!(
        found.is_empty(),
        "期限なしの待ちが残っている（#1271）。`psmux_ctl::wait_bounded` / `psmux_ctl::psmux` / \
         `psmux_ctl::kill_server` へ寄せること（返らない回にテストごと固まり、器と pwsh が残る）:\n{}",
        found.join("\n")
    );
}

/// 後始末が「期限つき + pid 指定の止め」の形を保っていること。
/// 走査だけだと**中身を骨抜きにされても気づけない**ので、1 実装の側の要件も見る
#[test]
fn 後始末が期限と_pid_指定の止めを持っている() {
    let path = repo_root().join("crates/tako-core/tests/common/psmux_ctl.rs");
    let src = std::fs::read_to_string(&path).expect("psmux_ctl.rs を読める");
    let code = code_view(&src);
    for (needle, why) in [
        ("try_wait()", "期限つきで覗く形（`wait()` で待ち切らない）"),
        ("child.kill()", "期限を過ぎた子を殺す"),
        (
            "state_wait_budget",
            "上限の政策は `tako_core::wait_budget` の 1 実装を通す",
        ),
        ("pid_alive(", "kill-server の後に器がまだ居るかを見る"),
    ] {
        assert!(
            code.contains(needle),
            "後始末から {needle} が消えている（{why}）: {}",
            path.display()
        );
    }
    // 止め方そのものは**文字列リテラル**なので潰さずに見る（コメント行だけ落とす =
    // 説明文の「`taskkill /IM` は使わない」を実装と読み違えないため）
    let literals = without_comment_lines(&src);
    for (needle, why) in [
        ("taskkill", "残っていたら **pid 指定**で止める（Windows）"),
        ("/PID", "名前一致（`/IM`）ではなく pid で指定する"),
    ] {
        assert!(
            literals.contains(needle),
            "後始末から {needle} が消えている（{why}）: {}",
            path.display()
        );
    }
    // **名前一致の一括 kill は絶対に持ち込まない**（他ワーカーの器を巻き込む）
    for banned in ["/IM", "Stop-Process", "-Name"] {
        assert!(
            !literals.contains(banned),
            "名前一致の一括 kill が入っている（他インスタンスの器を巻き込む）: {banned}"
        );
    }
}

/// 走査先を取り違えて**何も見ていない番犬**になっていないこと
#[test]
fn 番犬が走査対象と検出力を持っている() {
    for path in targets() {
        assert!(path.exists(), "走査対象が無い: {}", path.display());
    }
    let backend =
        std::fs::read_to_string(repo_root().join("crates/tako-core/tests/psmux_backend.rs"))
            .expect("psmux_backend.rs を読める");
    assert!(
        backend.contains("impl Drop for Fixture"),
        "#1271 の現場（`Fixture::drop`）が走査対象に入っていない"
    );
    let ctl =
        std::fs::read_to_string(repo_root().join("crates/tako-core/tests/common/psmux_ctl.rs"))
            .expect("psmux_ctl.rs を読める");
    assert!(
        ctl.contains("fn legacy_kill_server"),
        "A/B の旧アームが消えている（除外規則が検証できない）"
    );

    // 検出力: #1271 で直した 4 つの形（修正前ソースそのまま）
    let old_drop = "impl Drop for Fixture {\n    fn drop(&mut self) {\n        \
        let _ = Command::new(&self.bin)\n            \
        .args([\"-L\", &self.socket, \"kill-server\"])\n            .output();\n    }\n}\n";
    assert_eq!(
        hits(old_drop).len(),
        1,
        "旧 `Drop` の無限待ちを検出できていない"
    );
    let old_status = "    let ok = Command::new(bin).arg(\"-V\").status();\n";
    assert_eq!(hits(old_status).len(), 1, "`status()` を検出できていない");
    let old_wait_out = "    let out = child.wait_with_output();\n";
    assert_eq!(
        hits(old_wait_out).len(),
        1,
        "`wait_with_output()` を検出できていない"
    );
    let bare_wait = "    let mut child = cmd.spawn().unwrap();\n    let _ = child.wait();\n";
    assert_eq!(
        hits(bare_wait).len(),
        1,
        "殺していない子の `wait()` を検出できていない"
    );

    // 誤検知しないこと
    let reap = "    let _ = child.kill();\n    let _ = child.wait();\n";
    assert!(hits(reap).is_empty(), "殺した子の刈り取りを誤検知");
    let peek = "    match child.try_wait() {\n        Ok(Some(s)) => s,\n    }\n";
    assert!(hits(peek).is_empty(), "`try_wait()` を誤検知");
    let legacy = "fn legacy_kill_server(bin: &str) {\n    \
        let _ = Command::new(bin).output();\n}\n";
    assert!(
        hits(legacy).is_empty(),
        "A/B の旧アームを誤検知（`legacy_` の除外が効いていない）"
    );
    let prose = "/// 素の `Command::output()` は期限を持たない（説明文）\nfn f() {}\n";
    assert!(hits(prose).is_empty(), "コメントの中の記述を誤検知");
    let literal = "    let msg = \"expected .output() to be bounded\";\n";
    assert!(hits(literal).is_empty(), "文字列リテラルの中の記述を誤検知");
    let field = "    let ok = out.status.success();\n";
    assert!(hits(field).is_empty(), "`status` フィールドの参照を誤検知");
}
