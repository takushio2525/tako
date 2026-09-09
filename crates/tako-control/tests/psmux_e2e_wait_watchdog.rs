//! 番犬: 器（psmux）の e2e が**固定回数の窓**で画面や器の状態を待っていない（#1114）
//!
//! ## なぜ止めるのか
//!
//! `crates/tako-core/tests/psmux_backend.rs` は実バイナリの psmux を相手にする
//! 統合テストで、**自分自身が 18 本並列で走る**（= 器と pwsh を同時に 18 個立てる）。
//! 旧実装は「100ms × N 回」を 1 単位に最大 6 周する**固定予算**で待っていたが、
//! Windows 実機で測ると器の中の pwsh がプロンプトを出すまで **20〜34 秒**かかる
//! 負荷帯があり、6 周ぶんすべてをプロンプト待ちで使い切って偽 FAILED になっていた
//! （#1114 の実測: このバイナリを 4 多重 + プロセス生成負荷 12 で **9/16 FAILED**。
//! 落ちた画面は `PS …> Write-O` = 打鍵のエコーが途中で切れた形）。
//!
//! 固定窓は「混み具合に追従しない」ので、同じソース・同じ機でも回によって
//! 合否が入れ替わる。直し方は `.agent/conventions.md`
//! 「セルフテストの待ち条件の書き方」と同じで、**状態で待つ + 上限を
//! `tako_core::wait_budget::state_wait_budget` で伸ばす**（伸ばすだけ・4 倍で打ち切り）。
//!
//! ## 何を違反とするか（**誤検知しない形**）
//!
//! 走査するのは `psmux_backend.rs` の 1 ファイルだけ。落とすのは 2 形:
//!
//! 1. `for _ in 0..N {` の本体に `std::thread::sleep(` が**字句として**現れる
//!    （= 回数で待ち時間を決めている窓）
//! 2. `std::thread::sleep(` の直後 3 行以内に `assert!(` / `assert_eq!(` が来る
//!    （= 固定待ちの後に状態を見る形。旧 `sleep(800ms)` → `exists` がこれ）
//!
//! 対象外（時間で書くのが正しい形）:
//!
//! | 形 | 例 | なぜ対象外か |
//! |---|---|---|
//! | 期限式の待ち | `while Instant::now() < deadline { … sleep … }` | 主題が期限。回数で決めていない |
//! | ポーリング間隔 | `loop { … sleep(POLL) … }` | 上限は `budget_for` が持つ |
//! | 打鍵のペース | `for ch in keys.chars() { … sleep(60ms) }` | 回数 = 文字数。待ちではない |
//! | 回数だけが意味を持つ操作 | `for _ in 0..7 { f.raw(&["…", "scroll-up"]) }` | sleep が無い |
//! | A/B の旧アーム | `legacy_deliver_marker` | 旧の固定窓を**わざと**再現する。`legacy_` 接頭辞で除外 |

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

fn target() -> PathBuf {
    repo_root().join("crates/tako-core/tests/psmux_backend.rs")
}

/// 違反 1 件（行番号は 1 始まり）
#[derive(Debug, PartialEq, Eq)]
struct Hit {
    line: usize,
    kind: &'static str,
    text: String,
}

/// `for _ in 0..N {` の本体（インデントで閉じ括弧まで）に sleep があるか。
///
/// 本体の切り出しは**入口の行のインデント**を基準にする（`for` と同じ深さの
/// 行が来たらそこで閉じる）。`.agent/conventions.md` の他の番犬と同じ採り方
fn fixed_window_hits(src: &str) -> Vec<Hit> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut legacy_until: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        // `fn legacy_…(` から同じインデントの `}` までは A/B の旧アーム = 対象外
        if line.trim_start().starts_with("fn legacy_") {
            let indent = indent_of(line);
            legacy_until = lines
                .iter()
                .enumerate()
                .skip(i + 1)
                .find(|(_, l)| l.trim() == "}" && indent_of(l) == indent)
                .map(|(j, _)| j);
        }
        if legacy_until.is_some_and(|end| i <= end) {
            continue;
        }
        let trimmed = line.trim_start();
        if !(trimmed.starts_with("for _ in 0..") && trimmed.ends_with('{')) {
            continue;
        }
        let indent = indent_of(line);
        for body in lines.iter().skip(i + 1) {
            // 同じ深さの閉じ括弧まで
            if body.trim().starts_with('}') && indent_of(body) <= indent {
                break;
            }
            if body.contains("std::thread::sleep(") {
                out.push(Hit {
                    line: i + 1,
                    kind: "固定回数の窓で待っている",
                    text: format!("{} … {}", trimmed, body.trim()),
                });
                break;
            }
        }
    }
    out
}

/// `sleep` の直後 3 行以内に assert が来る形
fn sleep_then_assert_hits(src: &str) -> Vec<Hit> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !line.contains("std::thread::sleep(") {
            continue;
        }
        for body in lines.iter().skip(i + 1).take(3) {
            let t = body.trim_start();
            if t.starts_with("assert!(") || t.starts_with("assert_eq!(") {
                out.push(Hit {
                    line: i + 1,
                    kind: "固定待ちの直後に状態を見ている",
                    text: format!("{} → {}", line.trim(), t),
                });
                break;
            }
        }
    }
    out
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn all_hits(src: &str) -> Vec<Hit> {
    let mut out = fixed_window_hits(src);
    out.extend(sleep_then_assert_hits(src));
    out.sort_by_key(|h| h.line);
    out
}

#[test]
fn 器のe2eは固定回数の窓で待っていない() {
    let path = target();
    let src = std::fs::read_to_string(&path).expect("psmux_backend.rs を読める");
    let hits = all_hits(&src);
    assert!(
        hits.is_empty(),
        "固定予算の待ちが残っている（#1114）。状態待ち + `budget_for` / `wait_state` / \
         `pump_until*` へ寄せること:\n{}",
        hits.iter()
            .map(|h| format!(
                "  crates/tako-core/tests/psmux_backend.rs:{} {}: {}",
                h.line, h.kind, h.text
            ))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// 走査先を取り違えて**何も見ていない番犬**になっていないこと
#[test]
fn 番犬が走査対象と検出力を持っている() {
    let path = target();
    assert!(path.exists(), "走査対象が無い: {}", path.display());
    let src = std::fs::read_to_string(&path).expect("psmux_backend.rs を読める");
    assert!(
        src.contains("fn 器はクライアント切断後もattachで内容ごと戻る"),
        "#1114 の現場（対象テスト）が走査対象に入っていない"
    );
    assert!(
        src.contains("fn legacy_deliver_marker"),
        "A/B の旧アームが消えている（除外規則が検証できない）"
    );

    // 検出力: #1114 で直した 3 つの形を合成して確かめる
    let old_window = "    for _ in 0..6 {\n        \
        if !wait_for(&mut first, &mut rx1, prompt, 60) {\n            continue;\n        }\n        \
        std::thread::sleep(Duration::from_millis(100));\n    }\n";
    assert_eq!(
        fixed_window_hits(old_window).len(),
        1,
        "旧の固定窓を検出できていない"
    );
    let old_sleep = "    drop(first);\n    std::thread::sleep(Duration::from_millis(800));\n    \
        assert!(\n        f.backend.exists(&name),\n    );\n";
    assert_eq!(
        sleep_then_assert_hits(old_sleep).len(),
        1,
        "固定待ちの直後の assert を検出できていない"
    );

    // 誤検知しないこと
    let pacing = "    for ch in keys.chars() {\n        \
        std::thread::sleep(Duration::from_millis(60));\n    }\n";
    assert!(fixed_window_hits(pacing).is_empty(), "打鍵のペースを誤検知");
    let actions =
        "    for _ in 0..7 {\n        f.raw(&[\"send-keys\", \"-X\", \"scroll-up\"]);\n    }\n";
    assert!(
        fixed_window_hits(actions).is_empty(),
        "sleep の無い操作ループを誤検知"
    );
    let legacy = "fn legacy_deliver_marker() {\n    for _ in 0..6 {\n        \
        std::thread::sleep(Duration::from_millis(100));\n    }\n}\n";
    assert!(
        fixed_window_hits(legacy).is_empty(),
        "A/B の旧アームを誤検知（`legacy_` の除外が効いていない）"
    );
}
