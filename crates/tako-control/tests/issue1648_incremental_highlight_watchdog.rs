//! 編集中の再ハイライトが全文へ戻らないことを見張る番犬（#1648）
//!
//! 旧実装の `preview::apply_editor_text` は 1 打鍵ごとに `highlighter().highlight(path, 全文)`
//! を UI スレッドで呼んでいた（release 実測: 1,087 行 75.5ms / 3,460 行 218.2ms /
//! 4,666 行 344.6ms）。ASCII 入力・IME 確定・貼り付け・BS / Del / Enter の**全経路**が
//! `refresh_preview_from_editor` → `apply_editor_text` を通るので、ここが全文へ戻ると
//! 打鍵のたびに画面が固まる状態へ丸ごと退行する。
//!
//! 直し方は「呼ぶ回数を減らす」ではなく**塗る範囲を型で絞る**こと:
//!   1. 行の切れ目の状態（`SavedState`）を `HighlightCache` が持ち回る
//!   2. `apply_editor_text` は全文 API ではなく `refresh_editor_lines` を呼ぶ
//!   3. 全文経路（`run`）と差分経路（`refresh_full` / `refresh_incremental`）が
//!      **同じ `step`** を通る = 塗り分けの食い違いが構造的に起こらない
//!   4. キャッシュの寿命は編集セッション（`EditState`）と同じ
//!   5. 塗り足すのは **自分が最後に書いた表示行**だけ（`highlight_stamp` の照合）
//!
//! ソース走査で見張るのはこの 5 つ。実際に絞れているか（再ハイライト行数）は
//! `tako-app` 側の単体テスト `一打鍵の再ハイライトは変わった行の周辺だけを走る` が見る。

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

#[path = "common/code_view.rs"]
mod code_view;

const PREVIEW: &str = "crates/tako-app/src/preview.rs";

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// **肯定の存在確認**が見る眺め（doc コメントに書いた同じ綴りで緑にならないように潰す。#1609）
fn read_code(root: &Path, rel: &str) -> String {
    code_view::without_comments_checked(&read(root, rel), rel)
}

/// `fn <name>(` の本文を切り出す（トップレベル / impl 内のどちらでも拾える）
fn fn_body(src: &str, name: &str) -> String {
    let heads = [
        format!("\nfn {name}("),
        format!("\npub fn {name}("),
        format!("\n    fn {name}("),
        format!("\n    pub fn {name}("),
    ];
    let at = heads
        .iter()
        .filter_map(|head| src.find(head.as_str()).map(|at| at + head.len()))
        .min()
        .unwrap_or_else(|| panic!("`fn {name}` が見つからない（#1648）"));
    let rest = &src[at..];
    let end = [
        rest.find("\nfn "),
        rest.find("\npub fn "),
        rest.find("\n    fn "),
        rest.find("\n    pub fn "),
    ]
    .into_iter()
    .flatten()
    .min()
    .unwrap_or(rest.len());
    rest[..end].to_string()
}

/// 見つけた綴りを `file:line` で名指しするための行番号
fn line_of(src: &str, needle: &str) -> usize {
    src.find(needle)
        .map(|at| src[..at].matches('\n').count() + 1)
        .unwrap_or(0)
}

#[test]
fn 編集中の再ハイライトは全文apiを呼ばない() {
    let root = workspace_root();
    let code = read_code(&root, PREVIEW);
    let body = fn_body(&code, "apply_editor_text");

    assert!(
        body.contains("refresh_editor_lines("),
        "{PREVIEW}:{} の `apply_editor_text` が `refresh_editor_lines` を通っていない（#1648。\
         差分経路を外すと 1 打鍵ごとに全文を塗り直す = 4,666 行で 344ms の退行）",
        line_of(&code, "pub fn apply_editor_text(")
    );
    assert!(
        body.contains("preview.highlight_stamp == Some(edit.highlight.stamp)"),
        "{PREVIEW}:{} の `apply_editor_text` が表示行の印を照合していない（#1648。\
         Markdown ⇄ Code の切替は `PreviewState` を入れ替えて `EditState` は残すので、\
         行数が偶然揃うと「前回の編集を取りこぼした表示行」へ差分を重ねて表示が壊れる）",
        line_of(&code, "pub fn apply_editor_text(")
    );
    assert!(
        !body.contains(".highlight(&preview.path") && !body.contains("highlight_text("),
        "{PREVIEW}:{} の `apply_editor_text` が全文ハイライト API を呼んでいる（#1648。\
         全文は初回とキャッシュが使えない時だけで、打鍵のたびに呼ぶ経路には置かない）",
        line_of(&code, "pub fn apply_editor_text(")
    );
}

#[test]
fn 差分経路は世代と構文と行数を照合してから使う() {
    let root = workspace_root();
    let code = read_code(&root, PREVIEW);
    let body = fn_body(&code, "refresh");

    for (needle, 理由) in [
        (
            "cache.generation == self.generation",
            "構文セットを載せ直す（#815 の 30 秒解放）と行状態が指すコンテキストの意味が変わりうる",
        ),
        (
            "cache.syntax == syntax.name",
            "1 行目の shebang を消すだけで構文が変わる（行状態は別言語のもの）",
        ),
        (
            "cache.starts.len() == lines.len()",
            "外からの差し替え（ライブリロード・undo）でキャッシュと表示行がずれる",
        ),
    ] {
        assert!(
            body.contains(needle),
            "{PREVIEW}:{} の `refresh` に `{needle}` の照合が無い（#1648。{理由}）",
            line_of(&code, "    fn refresh(")
        );
    }
}

#[test]
fn 全文経路と差分経路は同じ1行実装を通る() {
    let root = workspace_root();
    let code = read_code(&root, PREVIEW);

    for name in ["run", "refresh_full", "refresh_incremental"] {
        let body = fn_body(&code, name);
        assert!(
            body.contains("self.step("),
            "{PREVIEW}:{} の `{name}` が `step` を通っていない（#1648。1 行の塗り方を\
             2 つ持つと「全文で塗った色」と「差分で塗った色」が食い違い、\
             見た目の退行として表に出る）",
            line_of(&code, &format!("    fn {name}("))
        );
    }
}

#[test]
fn 行状態キャッシュは編集セッションと寿命を同じにする() {
    let root = workspace_root();
    let code = read_code(&root, PREVIEW);

    assert!(
        code.contains("pub highlight: HighlightCache,"),
        "{PREVIEW} の `EditState` が `HighlightCache` を持っていない（#1648。\
         ペインをまたいで生きる置き場（グローバル・`TakoApp` の別マップ）に移すと、\
         編集セッションが消えてもキャッシュが残り、消し忘れがそのままリークになる）"
    );
    let open = fn_body(&code, "open");
    assert!(
        open.contains("highlight: HighlightCache::default()"),
        "{PREVIEW}:{} の `EditState::open` が行状態キャッシュを初期化していない（#1648）",
        line_of(&code, "    pub fn open(")
    );
}
