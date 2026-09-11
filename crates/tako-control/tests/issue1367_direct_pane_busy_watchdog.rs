//! 番犬: 1 ペインの busy を「器のセッション名」で判定する形へ戻さない（#1367）
//!
//! ## なぜ止めるのか
//!
//! #372 で走査（`RunningChildrenScanState`）は器あり / 器なしの両方を数えるように
//! なったが、**引く側**は器のセッション名（`busy_sessions`）を見たままだった。
//! 器を持たないペイン（tmux 未導入 / persist OFF = Homebrew cask の既定）は
//! そもそもセッション名を持たないので、走査が busy と知っていても消費側は必ず false:
//!
//! - close 確認（#566）が出ず、**稼働中のエージェントが cmd+W 一撃で消える**
//!   （2026-09-12 の隔離 GUI で実測。`busy_agents=1` なのにダイアログ無しで即 close）
//! - GUI モードの判定（#694）が「アイドルシェル」と読み、動いているペインへ
//!   スターターのカードを被せる（同実測で `display=starter`）
//! - チャットの `agent_running` が false になる
//!
//! 落ち方はどれも 1 行で戻せる。単体テストは「いまの判定結果」を固定するが、
//! **同じ形の再発**（消費側がまた `busy_sessions` を引く）はソースの形でしか止まらない。

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

const APP: &str = "crates/tako-app/src/main.rs";
const CHAT: &str = "crates/tako-app/src/chat_view.rs";
const AGENTS: &str = "crates/tako-control/src/agents.rs";

/// 指定した関数の本文（シグネチャ行から次の同インデント `}` まで）を
/// **行番号つき**で切り出す。コメント行は落とす（説明文の引用に当たらないため）
fn fn_body(rel: &str, signature: &str) -> Vec<(usize, String)> {
    let source = read(rel);
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{rel} に `{signature}` が見つからない（改名したら番犬も直す）"));
    let head_line = source[..start].lines().count();
    let indent = " ".repeat(signature.len() - signature.trim_start().len());
    let close = format!("\n{indent}}}");
    let end = source[start..]
        .find(&close)
        .map(|i| start + i)
        .unwrap_or(source.len());
    source[start..end]
        .lines()
        .enumerate()
        .map(|(i, l)| (head_line + i, l.to_string()))
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .collect()
}

fn joined(body: &[(usize, String)]) -> String {
    body.iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 空白を全部落とした本文。rustfmt が `self\n    .backend_sessions` のように折るので、
/// 式の形を見る検査は改行に依存させない
fn compact(body: &[(usize, String)]) -> String {
    joined(body)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// `rel` の中で `needle` を含む行を `file:line: 本文` で並べる
fn hits(rel: &str, needle: &str) -> Vec<String> {
    read(rel)
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with("//") && l.contains(needle))
        .map(|(i, l)| format!("{rel}:{}: {}", i + 1, l.trim()))
        .collect()
}

/// 1: close 確認（#566）は器の有無で分岐しない
#[test]
fn close確認は器のセッション名で判定しない() {
    let body = fn_body(APP, "    fn pane_close_needs_confirm(");
    let code = compact(&body);
    assert!(
        code.contains("self.pane_has_busy_children(pane_id)"),
        "{APP}: pane_close_needs_confirm が pane_has_busy_children（= 1 実装）を\n\
         通っていない。器なしペインでは確認なしで即 close に戻る（#1367 / #566）"
    );
    let bad: Vec<String> = body
        .iter()
        .filter(|(_, l)| l.contains("backend_sessions") || l.contains("busy_sessions"))
        .map(|(n, l)| format!("{APP}:{n}: {}", l.trim()))
        .collect();
    assert!(
        bad.is_empty(),
        "{APP}: close 確認の判定が器のセッション名を引いている（#1367 の根因）。\n\
         器を持たないペインはセッション名を持たないので必ず false になる:\n  {}",
        bad.join("\n  ")
    );
}

/// 2: GUI モードの判定材料（#694 の `busy_children`）も同じ 1 実装を通す
#[test]
fn guiモードの判定材料も1実装を通す() {
    let body = fn_body(APP, "    fn pane_display_input(");
    let code = compact(&body);
    assert!(
        code.contains("busy_children:self.pane_has_busy_children(pane_id)"),
        "{APP}: pane_display_input の busy_children が pane_has_busy_children を\n\
         通っていない。器なしペインでは動いているペインへスターターが被る（#1367 / #694）"
    );
    let bad: Vec<String> = body
        .iter()
        .filter(|(_, l)| l.contains("busy_sessions"))
        .map(|(n, l)| format!("{APP}:{n}: {}", l.trim()))
        .collect();
    assert!(
        bad.is_empty(),
        "{APP}: 器のセッション名を引いている:\n  {}",
        bad.join("\n  ")
    );
}

/// 3: チャットの `agent_running` も同じ 1 実装を通す
#[test]
fn チャットのagent_runningも1実装を通す() {
    let body = fn_body(CHAT, "    pub(crate) fn collect_chat_targets(");
    let code = compact(&body);
    assert!(
        code.contains("agent_running:self.pane_has_busy_children("),
        "{CHAT}: collect_chat_targets の agent_running が pane_has_busy_children を\n\
         通っていない。器なしペインではエージェントが動いていても\n\
         「稼働中」に見えない（#1367）"
    );
    let bad: Vec<String> = body
        .iter()
        .filter(|(_, l)| l.contains("busy_sessions"))
        .map(|(n, l)| format!("{CHAT}:{n}: {}", l.trim()))
        .collect();
    assert!(
        bad.is_empty(),
        "{CHAT}: 器のセッション名を引いている:\n  {}",
        bad.join("\n  ")
    );
}

/// 4: 「このペインは busy か」を問う口は tako-app 側に 1 つだけ
#[test]
fn ペイン単位のbusyを問う口は1実装() {
    let body = fn_body(APP, "    fn pane_has_busy_children(");
    let code = compact(&body);
    assert!(
        code.contains("self.running_children_scan.is_pane_busy(pane_id.as_u64())"),
        "{APP}: pane_has_busy_children が RunningChildrenScanState::is_pane_busy を\n\
         通っていない。判定を UI 層で組み直すと器なし / 器ありのどちらかが落ちる（#1367）"
    );
    // 消費側が走査結果の内訳（器のセッション名）を直接引いていない
    let bad: Vec<String> = [APP, CHAT]
        .iter()
        .flat_map(|rel| hits(rel, "busy_sessions"))
        .collect();
    assert!(
        bad.is_empty(),
        "tako-app が走査結果の `busy_sessions` を直に引いている（#1367 の根因）。\n\
         1 ペインの busy は is_pane_busy へ、数え上げは busy_count へ:\n  {}",
        bad.join("\n  ")
    );
    // 旧フィールド（器のセッション名のキャッシュ）が復活していない
    let revived: Vec<String> = [APP, CHAT]
        .iter()
        .flat_map(|rel| hits(rel, "busy_backend_sessions"))
        .collect();
    assert!(
        revived.is_empty(),
        "tako-app に `busy_backend_sessions`（器のセッション名のキャッシュ）が\n\
         復活している。#1367 はこのキャッシュを引く形そのものが根因:\n  {}",
        revived.join("\n  ")
    );
}

/// 5: 判定の本体は器あり / 器なしの両方を見る（片方だけだと片方の構成で永久に false）
#[test]
fn is_pane_busyは器なしと器ありの両方を見る() {
    let code = joined(&fn_body(AGENTS, "    pub fn is_pane_busy_in("));
    assert!(
        code.contains("busy_panes") && code.contains("busy_sessions"),
        "{AGENTS}: is_pane_busy_in が器なし（busy_panes）/ 器あり（busy_sessions）の\n\
         二段構えになっていない（#1367）"
    );
    assert!(
        code.contains("legacy_backend_only"),
        "{AGENTS}: A/B の腕（legacy_backend_only）が消えている。\n\
         同一バイナリで旧挙動を再現できないと、検出力の確認ができない（#1367）"
    );
    let public = joined(&fn_body(
        AGENTS,
        "    pub fn is_pane_busy(&self, pane: u64)",
    ));
    assert!(
        public.contains("legacy_1367()"),
        "{AGENTS}: 公開 API が legacy_1367() を通していない（env が唯一の差にならない）"
    );
}
