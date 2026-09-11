//! 番犬: 器を持たないペインでチャットビューが立たない形へ戻さない（#1397）
//!
//! ## なぜ止めるのか
//!
//! #1367 で「このペインで何か動いているか」（`agent_running`）は器なしペインでも
//! 真になったが、**チャットの列挙と live 解決は器つき前提のまま**だった。
//! 器を持たない構成（tmux 未導入 / persist OFF = Homebrew cask の既定）では
//! 次の 3 つが重なって、GUI モードのチャットビューが 1 度も立たない:
//!
//! 1. 列挙（`chat_view::collect_chat_targets`）が `backend_sessions` 起点 =
//!    器なしペインは 1 件も載らない
//! 2. live 解決（`agents::live_claude_sessions_by_backend`）が器のセッション名キーで、
//!    `backend_pane_pids()` が空なら即 `HashMap::new()`
//! 3. 判定表（`ui_mode::pane_display`）が alt screen をチャットより先に見る。
//!    **器なしでは claude の対話 TUI 自身が alt screen を使う**（2026-09-12 の
//!    隔離 GUI で実測: `alt_screen=true` / `claude_chat=false` / `display=terminal`）
//!
//! どれも 1 行で戻せる。単体テストは「いまの判定結果」を固定するが、**同じ形の
//! 再発**（列挙が器のセッション起点へ戻る / 解決が器のキーだけになる）は
//! ソースの形でしか止まらない。
//!
//! `main.rs` を走査対象にしないのは `issue1399_tree_notice_watchdog` と同じ理由
//! （production と隔離セルフテストが同居していてソース走査では区別できない）。

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

const CHAT: &str = "crates/tako-app/src/chat_view.rs";
const AGENTS: &str = "crates/tako-control/src/agents.rs";
const UI_MODE: &str = "crates/tako-core/src/ui_mode.rs";

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

/// 空白を全部落とした本文。rustfmt が `self\n    .terminals` のように折るので、
/// 式の形を見る検査は改行に依存させない
fn compact(body: &[(usize, String)]) -> String {
    joined(body)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// `body` の中で `needle` を含む行を `file:line: 本文` で並べる
fn hits_in(rel: &str, body: &[(usize, String)], needle: &str) -> Vec<String> {
    body.iter()
        .filter(|(_, l)| l.contains(needle))
        .map(|(n, l)| format!("{rel}:{n}: {}", l.trim()))
        .collect()
}

/// 1: チャット対象の列挙は器のセッション起点へ戻らない
#[test]
fn チャット対象の列挙は全ペイン起点() {
    let body = fn_body(CHAT, "    pub(crate) fn collect_chat_targets(");
    let code = compact(&body);
    assert!(
        code.contains("self.terminals.iter()"),
        "{CHAT}: collect_chat_targets が `self.terminals` 起点になっていない。\n\
         器を持たないペイン（tmux 未導入 / persist OFF = Homebrew cask の既定）は\n\
         `backend_sessions` に載らないので、チャットビューが 1 度も立たない（#1397）"
    );
    // 起点が器のセッションへ戻っていない（`backend_sessions` の参照は
    // 「そのペインに器があるか」の問い合わせだけが許される）
    assert!(
        !code.contains("self.backend_sessions.iter()"),
        "{CHAT}: collect_chat_targets が `backend_sessions` を起点に列挙している\n\
         （= #1397 の根因そのもの）。起点は全ペイン（`terminals`）で、\n\
         器の有無は `LiveSessionKey` の選び方にだけ効かせる:\n  {}",
        hits_in(CHAT, &body, "backend_sessions").join("\n  ")
    );
}

/// 2: 器なしペインのキーは (ペイン ID, PTY 直下の子 pid)
#[test]
fn 器なしペインのキーはペインidとpidの組() {
    let body = fn_body(CHAT, "    pub(crate) fn collect_chat_targets(");
    let code = compact(&body);
    assert!(
        code.contains("LiveSessionKey::pane(pane.as_u64(),session.child_pid()?,)")
            || code.contains("LiveSessionKey::pane(pane.as_u64(),session.child_pid()?)"),
        "{CHAT}: 器なしペインのキーが (ペイン ID, PTY 直下の子 pid) で組まれていない。\n\
         ペイン ID だけを鍵にすると、閉じて作り直したペイン（ID は再利用される #390）\n\
         へ前の会話が貼り付く（#1397 / #466 の sticky 規則）"
    );
}

/// 3: 列挙の alt screen 除外は「子プロセスが動いていないペイン」だけに効く
#[test]
fn alt_screenの除外は稼働中ペインに効かない() {
    let body = fn_body(CHAT, "    pub(crate) fn collect_chat_targets(");
    let code = compact(&body);
    assert!(
        code.contains("self.pane_inner_alt_screen(*pane)&&(legacy||!agent_running)"),
        "{CHAT}: alt screen のペインを無条件に列挙から外している。\n\
         **器なしでは claude の対話 TUI 自身が alt screen を使う**（実測）ので、\n\
         子プロセスが動いているペインは読みに行く必要がある（#1397）:\n  {}",
        hits_in(CHAT, &body, "pane_inner_alt_screen").join("\n  ")
    );
}

/// 4: live 解決は器のキーだけにならない
#[test]
fn live解決は器なしの起点も渡す() {
    let body = fn_body(CHAT, "pub(crate) fn load_chat_refresh(");
    let code = compact(&body);
    assert!(
        code.contains("agents::live_claude_sessions(&direct)"),
        "{CHAT}: load_chat_refresh が器なしの起点を渡していない。\n\
         `live_claude_sessions_by_backend` は器のセッション名しかキーに持たないので、\n\
         器を持たないペインは必ず解決から外れる（#1397 の根因）:\n  {}",
        hits_in(CHAT, &body, "live_claude_sessions").join("\n  ")
    );
    assert!(
        code.contains("LiveSessionKey::Pane{pane,pid}=>Some((*pane,*pid))"),
        "{CHAT}: load_chat_refresh が器なしキーから起点 (ペイン ID, pid) を\n\
         組み直していない（#1397）"
    );
}

/// 5: 解決の本体は器が空でも器なしペインを問われていれば続ける
#[test]
fn 解決は器が空でも器なしペインを見る() {
    let body = fn_body(AGENTS, "fn live_claude_sessions_in(");
    let code = compact(&body);
    assert!(
        code.contains("ifpanes.is_empty()&&direct.is_empty(){"),
        "{AGENTS}: live 解決が器の有無だけで早期に空を返している。\n\
         `backend_pane_pids()` が空 = 器が無いだけで、器なしペインの claude は\n\
         動いている（#1397 の根因の 1 つ）:\n  {}",
        hits_in(AGENTS, &body, "is_empty()").join("\n  ")
    );
    let inner = compact(&fn_body(AGENTS, "fn live_sessions_inner("));
    assert!(
        inner.contains("resolve_live_key(pid,parents,&pane_by_pid,&direct_by_pid)"),
        "{AGENTS}: live_sessions_inner が器あり / 器なしの二段構えを通っていない（#1397）"
    );
}

/// 6: sticky の器なしキーは pid 込みで生存判定する（ペイン ID 再利用の事故防止）
#[test]
fn stickyの器なしキーはpid込みで生死を見る() {
    let body = fn_body(AGENTS, "fn merge_live_sticky(");
    let code = compact(&body);
    assert!(
        code.contains("LiveSessionKey::Pane{pane,pid}=>"),
        "{AGENTS}: merge_live_sticky が器なしキーを別扱いしていない（#1397）"
    );
    assert!(
        code.contains("alive.contains(&(*pane,*pid))"),
        "{AGENTS}: 器なしの生存判定が (ペイン ID, pid) の完全一致になっていない。\n\
         ペイン ID だけで見ると、閉じて作り直した同じ番号のペインへ\n\
         前の会話が貼り付く（#390 / #466）:\n  {}",
        hits_in(AGENTS, &body, "alive").join("\n  ")
    );
    assert!(
        code.contains("None=>true"),
        "{AGENTS}: 器なしを問わない呼び出し（`direct_panes` = None）で\n\
         器なしの記憶を消している。器ありだけを問う経路（remote の v2 panes）が\n\
         チャット側の sticky を毎回飛ばす（#1397）"
    );
}

/// 7: 器なしのキーは pid をフィールドに持ち続ける（指紋）
#[test]
fn 器なしキーの形が変わっていない() {
    let source = read(AGENTS);
    let start = source
        .find("pub enum LiveSessionKey {")
        .expect("LiveSessionKey の定義（改名したら番犬も直す）");
    let body: String = source[start..]
        .lines()
        .take_while(|l| !l.starts_with('}'))
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    for field in ["Backend(String)", "Pane { pane: u64, pid: u32 }"] {
        assert!(
            body.contains(field),
            "{AGENTS}: LiveSessionKey から `{field}` が消えている。\n\
             器あり = セッション名 / 器なし = (ペイン ID, pid) の 2 本立てが #1397 の設計"
        );
    }
}

/// 8: 判定表はチャット確定を alt screen より先に見る
#[test]
fn 判定表はチャットをalt_screenより先に見る() {
    let body = fn_body(UI_MODE, "pub fn pane_display_in(");
    let code = compact(&body);
    let chat = code
        .find("ifinput.claude_chat{")
        .unwrap_or_else(|| panic!("{UI_MODE}: pane_display_in に claude_chat の枝が無い"));
    let alt = code
        .find("ifinput.alt_screen{")
        .unwrap_or_else(|| panic!("{UI_MODE}: pane_display_in に alt_screen の枝が無い"));
    assert!(
        chat < alt,
        "{UI_MODE}: alt screen をチャット確定より先に見ている（#1397 の根因の 1 つ）。\n\
         器なしでは claude の対話 TUI 自身が alt screen を使うので、\n\
         覆う対象そのものを理由にチャットを拒否することになる:\n  {}",
        hits_in(UI_MODE, &body, "alt_screen").join("\n  ")
    );
    assert!(
        code.contains("iflegacy_alt_screen_first&&input.alt_screen{"),
        "{UI_MODE}: A/B の腕（legacy_alt_screen_first）が消えている。\n\
         同一バイナリで旧挙動を再現できないと検出力の確認ができない（#1397）"
    );
    let public = compact(&fn_body(
        UI_MODE,
        "pub fn pane_display(input: PaneDisplayInput)",
    ));
    assert!(
        public.contains("pane_display_in(input,legacy_1397())"),
        "{UI_MODE}: 公開 API が legacy_1397() を通していない（env が唯一の差にならない）"
    );
}

/// 9: A/B の腕が 3 か所すべてに配線されている（列挙 / 解決 / 判定表）
#[test]
fn abの腕が三か所に配線されている() {
    let collect = compact(&fn_body(CHAT, "    pub(crate) fn collect_chat_targets("));
    assert!(
        collect.contains("tako_core::ui_mode::legacy_1397()"),
        "{CHAT}: collect_chat_targets に A/B の腕が無い（#1397）"
    );
    let load = compact(&fn_body(CHAT, "pub(crate) fn load_chat_refresh("));
    assert!(
        load.contains("tako_core::ui_mode::legacy_1397()"),
        "{CHAT}: load_chat_refresh に A/B の腕が無い（#1397）"
    );
    let arm = read(UI_MODE);
    assert!(
        arm.contains(r#"std::env::var("TAKO_1397_LEGACY")"#),
        "{UI_MODE}: TAKO_1397_LEGACY の読み口が消えている（#1397）"
    );
}
