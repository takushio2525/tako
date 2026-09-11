//! 2 秒 tick の定期判定が**全画面スナップショットへ戻っていない**ことの番犬（#1301 / #1001 C2 / C3）
//!
//! `TerminalSession::visible_lines()` は [`crate::screen::snapshot_opts`] を通るので、
//! 1 回ごとに `cols * rows` のセル配列と行ごとの `ScreenLine`（`String` +
//! `Vec<StyleRun>` + `Vec<usize>` の 3 確保）を組んでから、文字列だけを取り出して
//! 残りを捨てる。これを **2 秒ごとに全ペイン分 2 回**やっていたのが #1001 の H2 / H3 で、
//! `perf.log` に `periodic_prep:agent_metrics` のメインスレッド専有 2,761 ms が実記録されている。
//!
//! 直したあとも、この経路は「画面を読む」という自然な書き方に戻りやすい
//! （`session.visible_lines()` は最短で目的を果たすので、次に触る人が気づかず戻す）。
//! 見た目には壊れず、ペイン数に比例して重くなるだけなので、機械検査で止める。
//!
//! A/B（`TAKO_1001_C2_LEGACY=1` / `TAKO_1001_C3_LEGACY=1`）の旧経路アームは対象外
//! （旧経路を**再現できること**が検出力の担保なので、そこに `visible_lines()` は要る）。

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

/// `fn <name>(` から中括弧の釣り合いで関数本体を切り出し、(本体, 開始行番号) を返す
fn fn_body(src: &str, name: &str) -> Option<(String, usize)> {
    let needle = format!("fn {name}(");
    let at = src.find(&needle)?;
    let line = src[..at].lines().count() + 1;
    let open = at + src[at..].find('{')?;
    let mut depth = 0usize;
    for (i, c) in src[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((src[open..open + i + 1].to_string(), line));
                }
            }
            _ => {}
        }
    }
    None
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// C2: `refresh_agent_metrics` は「素のシェルを素通りする」ゲートを持ち、
/// 自分では全画面スナップショットを撮らない
#[test]
fn 定期更新のメトリクス収集は全画面を撮らない() {
    let root = workspace_root();
    let main_rs = root.join("crates/tako-app/src/main.rs");
    let src = read(&main_rs);
    let (body, line) =
        fn_body(&src, "refresh_agent_metrics").expect("refresh_agent_metrics の定義");

    assert!(
        body.contains("is_alt_screen()"),
        "{}:{line} refresh_agent_metrics に alt screen ゲートが無い（素のシェルまで毎 tick 走査する。#1301 C2）",
        main_rs.display()
    );
    assert!(
        !body.contains("visible_lines()"),
        "{}:{line} refresh_agent_metrics が全画面スナップショットを撮っている（末尾窓 = `agent_metrics()` 経由にする。#1301 C2）",
        main_rs.display()
    );
}

/// C2: `TerminalSession::agent_metrics` は末尾窓の**共有定数**で採る
/// （`visible_lines()` は A/B の旧経路アームだけ）
#[test]
fn メトリクス抽出は末尾窓の共有定数で採る() {
    let root = workspace_root();
    let terminal_rs = root.join("crates/tako-core/src/terminal.rs");
    let src = read(&terminal_rs);
    let (body, line) = fn_body(&src, "agent_metrics").expect("agent_metrics の定義");

    assert!(
        body.contains("tail_lines(AGENT_TUI_TAIL_LINES)"),
        "{}:{line} agent_metrics が末尾窓の共有定数で採っていない（窓の値を直書きするとドリフトする。#1301 C2）",
        terminal_rs.display()
    );
    assert!(
        !body.contains("visible_lines()") || body.contains("c2_legacy()"),
        "{}:{line} agent_metrics が A/B の外で全画面を撮っている（#1301 C2）",
        terminal_rs.display()
    );
}

/// C3: `drive_queued_message_recovery` は**先に末尾窓でヒントの有無を見て**、
/// ヒントが出ているペインに限って全画面を撮る（判定の前に全ペインを撮らない）
#[test]
fn キュー救出は判定の前に全画面を撮らない() {
    let root = workspace_root();
    let main_rs = root.join("crates/tako-app/src/main.rs");
    let src = read(&main_rs);
    let (body, line) = fn_body(&src, "drive_queued_message_recovery")
        .expect("drive_queued_message_recovery の定義");

    let tail = body.find("tail_lines(");
    let hint = body
        .find("queued_messages_pending")
        .expect("ヒント判定の呼び出しが無い（関数の中身が変わった）");
    assert!(
        tail.is_some_and(|t| t < hint),
        "{}:{line} ヒント判定の材料を末尾窓で採っていない（判定の前に全ペインを撮っている。#1301 C3）",
        main_rs.display()
    );
    assert!(
        body[hint..].contains("visible_lines()"),
        "{}:{line} `state.observe` へ渡す全画面をヒント判定の後で採っていない（#1301 C3）",
        main_rs.display()
    );
    assert!(
        body.contains("AGENT_TUI_TAIL_LINES"),
        "{}:{line} 末尾窓の共有定数を使っていない（窓の値を直書きするとドリフトする。#1301）",
        main_rs.display()
    );
}

/// A/B の入口が両方とも残っていること（旧経路を再現できないと検出力を実測できない）
#[test]
fn 旧経路へ戻すab入口が両方とも在る() {
    let root = workspace_root();
    let main_rs = read(&root.join("crates/tako-app/src/main.rs"));
    let terminal_rs = read(&root.join("crates/tako-core/src/terminal.rs"));
    assert!(
        terminal_rs.contains("TAKO_1001_C2_LEGACY"),
        "C2 の A/B 入口（TAKO_1001_C2_LEGACY）が無い"
    );
    assert!(
        main_rs.contains("TAKO_1001_C3_LEGACY"),
        "C3 の A/B 入口（TAKO_1001_C3_LEGACY）が無い"
    );
}

// --- 振る舞いの同値性（形だけでなく「答えが変わらない」ことを固定する） ---

/// claude TUI の実測の形（#1093 の fixture 由来）を組む。
/// `above` = 入力欄より上にある会話行の数、`hint` = キュー滞留ヒントが出ているか
fn claude_screen(above: usize, hint: bool) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for i in 0..above {
        // 会話ログにも `>` で始まる行は普通に出る（引用・diff・シェルの PS2）。
        // 窓を狭めたときに**こちらを拾ってしまわない**ことが要点
        lines.push(if i % 5 == 0 {
            format!("> 引用のような行 {i}")
        } else {
            format!("  ⏺ 何かの出力 {i}")
        });
    }
    lines.push(String::new());
    lines.push("─".repeat(72));
    lines.push(if hint {
        format!("❯ {}", tako_control::claude_tui::QUEUED_MESSAGES_HINT)
    } else {
        "❯".to_string()
    });
    lines.push("─".repeat(72));
    for l in [
        "  ⏵⏵ accept edits on",
        "  ctx  33% ███░░░░░░░",
        "  5h   12%",
        "  7d    4%",
        "  ⏸ 待機中 · ? for shortcuts",
        "",
    ] {
        lines.push(l.to_string());
    }
    lines
}

/// C3 の窓で `queued_messages_pending` の答えが全画面と変わらないこと。
///
/// **窓の外に入力欄がある画面では両方 false になる**（全画面は「最下部の
/// プロンプト行」を拾うが、それはヒント行ではないので false / 窓側は
/// プロンプト行が無いので None → false）ので、答えは一致する。
/// claude の TUI はフッターが最下部に固定なので、ヒント行が窓の外へ出ることは無い
#[test]
fn キュー滞留の判定は末尾窓でも全画面と一致する() {
    use tako_control::claude_tui::queued_messages_pending;
    let n = tako_core::terminal::AGENT_TUI_TAIL_LINES;

    for above in [0usize, 3, 10, 40, 200] {
        for hint in [false, true] {
            let full = claude_screen(above, hint);
            let tail: Vec<String> = full.iter().rev().take(n).rev().cloned().collect();
            assert_eq!(
                queued_messages_pending(&full),
                queued_messages_pending(&tail),
                "above={above} hint={hint}: 末尾 {n} 行だと答えが変わる"
            );
            assert_eq!(
                queued_messages_pending(&tail),
                hint,
                "above={above}: ヒントの有無をそのまま返すはず"
            );
        }
    }
}

/// 入力欄そのものが無い画面（エージェント TUI ではない）でも一致すること。
/// 会話ログの `>` を入力欄と読み違えていないかの対照
#[test]
fn 入力欄の無い画面でも末尾窓の判定は全画面と一致する() {
    use tako_control::claude_tui::queued_messages_pending;
    let n = tako_core::terminal::AGENT_TUI_TAIL_LINES;

    let mut full: Vec<String> = (0..200).map(|i| format!("> ログ {i}")).collect();
    full.extend((0..60).map(|i| format!("  ふつうの出力 {i}")));
    let tail: Vec<String> = full.iter().rev().take(n).rev().cloned().collect();
    assert_eq!(
        queued_messages_pending(&full),
        queued_messages_pending(&tail),
        "入力欄の無い画面で答えが変わる"
    );
    assert!(
        !queued_messages_pending(&full),
        "ヒントが無いのに true（前提が崩れている）"
    );
}
