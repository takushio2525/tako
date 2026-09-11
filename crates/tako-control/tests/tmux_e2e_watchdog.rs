//! 番犬: 実 tmux の e2e が**器を取り合わない**・**失敗の理由を捨てない**（#1300）
//!
//! ## なぜ止めるのか
//!
//! 器（tmux サーバー）の名前を固定にすると、同じ機で `cargo test --workspace` が
//! 2 本走った瞬間に**同じサーバーの同じセッション名**を取り合う。この機は worktree を
//! 分けた worker が並ぶので、これは例外ではなく日常的に起こる。
//!
//! 実測（#1300・2026-09-11）: 同一バイナリを 2 本同時に走らせると片方が 0.03 秒で
//! `duplicate session: tako1236new` に当たって落ちた。そのとき PTY は 103/511・
//! ソケット 206 個で**枯渇はしていない**（#1265 の真因とは別物）。取り合いは
//! 逆向きにも効く: 後から来た側の後始末（`kill-session -t <固定名>`）が、
//! 先に走っている側のセッションを横から消せてしまう。
//!
//! しかも落ちた側の assert は **`assert!(status.success(), "tmux new-session が失敗した")`**
//! だったので、理由（stderr）も器の状態も残らず、真因の確定に測り直しが 1 往復要った。
//!
//! 直し方は `crates/tako-control/tests/common/tmux_e2e.rs` の 1 実装へ寄せること
//! （`socket_for` でプロセスごとの器を取り、`new_session` / `send_keys` で叩く）。
//!
//! ## 何を違反とするか（**誤検知しない形**）
//!
//! 走査するのは `crates/tako-control/tests/` 直下の `.rs` だけ。
//!
//! | 形 | なぜ落とすか |
//! |---|---|
//! | `Command::new("tmux")` で器を直に叩く | 器の名前も失敗の理由も 1 実装を通らない |
//! | `const SOCKET: &str = "…"`（固定名の器） | 別プロセスと同じサーバーを取り合う |
//! | `"tako-e2e-<数字>"` の完全固定リテラル | 同上（`"tako-e2e-1186-{}"` のような接頭辞は対象外） |
//! | `assert!(<x>.success(), "<補間なしの固定文字列>")` | 落ちた理由が 1 ビットも残らない |
//!
//! 対象外:
//!
//! | 形 | 例 | なぜ対象外か |
//! |---|---|---|
//! | 1 実装そのもの | `common/tmux_e2e.rs` | A/B の旧アームで固定名と素の文言を**わざと**作る |
//! | この番犬自身 | `tmux_e2e_watchdog.rs` | 説明文に違反の形を書く |
//! | 器の在否だけを見る呼び出し | `Command::new("tmux").arg("-V")` | 器を作らない（`-V` は名前を要らない） |
//! | 理由を持つ assert | `assert!(ok, "送れない: {stderr}")` | 補間がある = 理由が載っている |

use std::path::{Path, PathBuf};

#[path = "common/code_view.rs"]
mod code_view_mod;
use code_view_mod::{code_view, without_comment_lines};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

fn tests_dir() -> PathBuf {
    repo_root().join("crates/tako-control/tests")
}

/// 走査対象（`tests/` 直下の `.rs`。1 実装とこの番犬自身は除く）
fn targets() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(tests_dir())
        .expect("tests ディレクトリを読める")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
        .filter(|p| p.file_name().and_then(|n| n.to_str()) != Some("tmux_e2e_watchdog.rs"))
        .collect();
    out.sort();
    out
}

/// 違反 1 件（行番号は 1 始まり）
#[derive(Debug, PartialEq, Eq)]
struct Hit {
    line: usize,
    kind: &'static str,
    text: String,
}

/// 器を直に叩く形・理由を捨てる assert を拾う（コメントと文字列を潰した眺めで見る）
fn container_hits(src: &str) -> Vec<Hit> {
    let code = code_view(src);
    let raw: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    for (i, line) in code.lines().enumerate() {
        let text = raw.get(i).unwrap_or(&"").trim().to_string();
        // `Command::new("tmux")` は文字列が潰れて `Command::new("    ")` になるので
        // 生の行で見る（`-V` の在否確認だけは器を作らないので対象外）
        let raw_line = raw.get(i).copied().unwrap_or("");
        if !raw_line.trim_start().starts_with("//")
            && raw_line.contains(r#"Command::new("tmux")"#)
            && !raw_line.contains("-V")
            && !next_arg_is_version(&raw, i)
        {
            out.push(Hit {
                line: i + 1,
                kind: "器を直に叩いている（`Command::new(\"tmux\")`）",
                text: text.clone(),
            });
            continue;
        }
        if line.contains(".success()") && line.contains("assert!(") && !reason_carried(&raw, i) {
            out.push(Hit {
                line: i + 1,
                kind: "落ちた理由を持たない assert",
                text,
            });
        }
    }
    out
}

/// `Command::new("tmux")` の次の行が `.arg("-V")`（器の在否確認）か
fn next_arg_is_version(raw: &[&str], at: usize) -> bool {
    raw.get(at + 1).is_some_and(|l| l.contains("\"-V\""))
}

/// この assert が理由を載せているか（メッセージに補間 `{` がある）。
/// assert は複数行に割れるので、その行から `);` までを 1 つの式として見る
fn reason_carried(raw: &[&str], at: usize) -> bool {
    let mut text = String::new();
    for line in raw.iter().skip(at).take(8) {
        text.push_str(line);
        text.push('\n');
        if line.trim_end().ends_with(");") {
            break;
        }
    }
    text.contains('{')
}

/// 固定名の器を拾う（**文字列リテラルを見る**ので `code_view` は使わない）
fn fixed_socket_hits(src: &str) -> Vec<Hit> {
    let mut out = Vec::new();
    for (i, line) in without_comment_lines(src).lines().enumerate() {
        if line.contains("const SOCKET: &str =") {
            out.push(Hit {
                line: i + 1,
                kind: "固定名の器（`const SOCKET`）",
                text: line.trim().to_string(),
            });
            continue;
        }
        if let Some(name) = fixed_e2e_literal(line) {
            out.push(Hit {
                line: i + 1,
                kind: "固定名の器のリテラル",
                text: name,
            });
        }
    }
    out
}

/// `"tako-e2e-<数字>"`（完全固定）を返す。
/// `"tako-e2e-1186-{}"` のような**接頭辞**は pid を足す形なので拾わない
fn fixed_e2e_literal(line: &str) -> Option<String> {
    let mut rest = line;
    while let Some(at) = rest.find("\"tako-e2e-") {
        let after = &rest[at + 1..];
        let end = after.find('"')?;
        let lit = &after[..end];
        let tail = &lit["tako-e2e-".len()..];
        if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
            return Some(format!("\"{lit}\""));
        }
        rest = &after[end..];
    }
    None
}

#[test]
fn 実tmuxのe2eが器を直に叩いていない() {
    let mut found: Vec<String> = Vec::new();
    for path in targets() {
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} を読める: {e}", path.display()));
        let rel = path
            .strip_prefix(repo_root())
            .unwrap_or(&path)
            .display()
            .to_string();
        for hit in container_hits(&src) {
            found.push(format!("  {rel}:{} {}: {}", hit.line, hit.kind, hit.text));
        }
    }
    assert!(
        found.is_empty(),
        "実 tmux の e2e が 1 実装を通っていない（#1300）。\
         `crates/tako-control/tests/common/tmux_e2e.rs` の `new_session` / `send_keys` へ \
         寄せること（固定名の取り合いと、理由を捨てる assert の両方がここから生まれる）:\n{}",
        found.join("\n")
    );
}

#[test]
fn 実tmuxのe2eが固定名の器を使っていない() {
    let mut found: Vec<String> = Vec::new();
    for path in targets() {
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} を読める: {e}", path.display()));
        let rel = path
            .strip_prefix(repo_root())
            .unwrap_or(&path)
            .display()
            .to_string();
        for hit in fixed_socket_hits(&src) {
            found.push(format!("  {rel}:{} {}: {}", hit.line, hit.kind, hit.text));
        }
    }
    assert!(
        found.is_empty(),
        "固定名の器が残っている（#1300）。`tmux_e2e::socket_for(\"<Issue 番号>\")` を使うこと\
         （固定名だと同じ機の別プロセスと同じサーバー・同じセッション名を取り合い、\
         `duplicate session` で落ちる / 相手のセッションを横から消す）:\n{}",
        found.join("\n")
    );
}

/// 走査だけだと**1 実装の中身を骨抜きにされても気づけない**ので、そちらの要件も見る
#[test]
fn 器の1実装が_pid_と診断と期限を持っている() {
    let path = tests_dir().join("common/tmux_e2e.rs");
    let src = std::fs::read_to_string(&path).expect("tmux_e2e.rs を読める");
    let code = code_view(&src);
    for (needle, why) in [
        ("std::process::id()", "器の名前にプロセス id を入れる"),
        ("try_wait()", "期限つきで覗く形（`wait()` で待ち切らない）"),
        ("child.kill()", "期限を過ぎた子を殺す"),
        (
            "state_wait_budget",
            "上限の政策は `tako_core::wait_budget` の 1 実装を通す",
        ),
        ("fn sessions_on", "失敗時にそのソケットのセッションを見せる"),
        (
            "fn exhaustion",
            "失敗時に PTY / ソケット / サーバー数を見せる",
        ),
        ("remove_socket_file", "退役した器のソケットファイルまで消す"),
    ] {
        assert!(
            code.contains(needle),
            "器の 1 実装から {needle} が消えている（{why}）: {}",
            path.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 固定名だけを拾い接頭辞は拾わない() {
        assert_eq!(
            fixed_e2e_literal(r#"const SOCKET: &str = "tako-e2e-1236";"#).as_deref(),
            Some("\"tako-e2e-1236\"")
        );
        assert_eq!(
            fixed_e2e_literal(r#"let s = format!("tako-e2e-1186-{}", std::process::id());"#),
            None
        );
        assert_eq!(fixed_e2e_literal(r#"let s = "tako-e2e-persist";"#), None);
    }

    #[test]
    fn 補間のある_assert_は理由を持つ扱い() {
        let with = [r#"    assert!(ok, "送れない: {stderr}");"#];
        let without = [r#"    assert!(status.success(), "tmux new-session が失敗した");"#];
        assert!(reason_carried(&with, 0));
        assert!(!reason_carried(&without, 0));
    }

    #[test]
    fn 複数行に割れた_assert_も末尾まで見る() {
        let lines = [
            "    assert!(",
            "        drawn,",
            r#"        "模擬 TUI が描かれない:\n{}","#,
            "        dump(&session)",
            "    );",
        ];
        assert!(reason_carried(&lines, 0));
    }
}
