//! Markdown の編集を抜けたら描画へ戻す経路を見張る番犬（#1661）
//!
//! 旧実装は `apply_editor_text` が表示モードを無条件に `Code` へ落とし目次を空にし、
//! `set_preview_editing_local` には描き直しが無かったので、Markdown を編集して抜けても
//! コード表示のまま・目次は空のまま戻らなかった（抜けた後の保存・読み直しでもコード表示へ落ちた）。
//!
//! 直し方は「呼ぶ場所を増やす」ではなく**判定を経路の要 1 か所へ載せる**形なので、
//! 1 行で戻せてしまう形をソース走査で止める:
//!   1. 表示をエディタの行へ落とす（`apply_editor_text`）のは `refresh_preview_from_editor` だけ
//!   2. その要は、エディタの行が要るか（`shows_editor_lines`）を先に見て、要らなければ描き直す
//!   3. 編集を抜ける口（`set_preview_editing_local`）が描き直しを呼ぶ
//!   4. 描き直しはディスクではなく**本文**から組む（未保存の見出しも目次に出る）
//!   5. レイアウトへは抜けた先のモードを書く（再起動で `code` のまま戻らない）
//!   6. 編集中の目次は描画の目次と同じパーサ設定で作る（k 番目どうしが同じ見出し）
//!   7. A/B の口（`TAKO_1661_LEGACY`）が判定の 1 か所から効く
//!
//! 実際に戻るか（表示モード・目次・見ていた節）は `preview.rs` の単体テストと
//! visual-test 節 `md-edit-resume`、`scripts/test-md-edit-resume-1661.sh` が見る。

use std::path::{Path, PathBuf};

// 本番コードの範囲取りは 1 実装（#1420）。コメントを落とす眺めも同じ部品の中にある（#1609）
#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view;

const MAIN: &str = "crates/tako-app/src/main.rs";
const PREVIEW: &str = "crates/tako-app/src/preview.rs";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

/// 本番コードだけを、コメントを落とした眺めで読む（行番号はそのまま）
fn read_code(rel: &str) -> String {
    let src = std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"));
    let production = production_range::production(&src, rel);
    code_view::without_comments_checked(&production, rel)
}

/// `fn <name>(` の本文を切り出す（トップレベル / impl 内のどちらでも拾える）。
/// 見つからなければ名指しで落とす（関数の改名で番犬が空振りしない）
fn fn_body<'a>(src: &'a str, name: &str, rel: &str) -> &'a str {
    let at = src
        .lines()
        .scan(0usize, |offset, line| {
            let start = *offset;
            *offset += line.len() + 1;
            Some((start, line))
        })
        .find(|(_, line)| tako_core::source_scan::fn_head_name(line) == Some(name))
        .map(|(start, _)| start)
        .unwrap_or_else(|| panic!("{rel} に `fn {name}` が見つからない（#1661）"));
    let rest = &src[at..];
    let after_head = rest.find('\n').map_or(rest.len(), |i| i + 1);
    let end = rest[after_head..]
        .lines()
        .scan(after_head, |offset, line| {
            let start = *offset;
            *offset += line.len() + 1;
            Some((start, line))
        })
        .find(|(_, line)| tako_core::source_scan::fn_head_name(line).is_some())
        .map_or(rest.len(), |(start, _)| start);
    &rest[..end]
}

/// 見つけた綴りを `file:line` で名指しするための行番号
fn line_of(src: &str, needle: &str) -> usize {
    src.find(needle)
        .map(|at| src[..at].matches('\n').count() + 1)
        .unwrap_or(0)
}

#[test]
fn 表示をエディタの行へ落とすのはrefresh_preview_from_editorの1か所だけ() {
    let code = read_code(MAIN);
    let mut current_fn: Option<&str> = None;
    let mut calls = 0;
    for (index, line) in code.lines().enumerate() {
        if let Some(name) = tako_core::source_scan::fn_head_name(line) {
            current_fn = Some(name);
        }
        if line.contains("apply_editor_text(") {
            calls += 1;
            assert_eq!(
                current_fn,
                Some("refresh_preview_from_editor"),
                "{MAIN}:{} の `{}` が `apply_editor_text` を直に呼んでいる（#1661。表示を\
                 エディタの行へ落とすかは `refresh_preview_from_editor` が\
                 `shows_editor_lines` で決める。直に呼ぶと編集を抜けた Markdown が\
                 コード表示へ落ちて目次が空になる）",
                index + 1,
                current_fn.unwrap_or("?")
            );
        }
    }
    assert!(
        calls >= 1,
        "{MAIN} に `apply_editor_text(` の呼び出しが 1 つも無い（走査が空振りしている）"
    );
}

#[test]
fn 要はエディタの行が要るかを先に見て要らなければ描き直す() {
    let code = read_code(MAIN);
    let body = fn_body(&code, "refresh_preview_from_editor", MAIN);
    let head = line_of(&code, "fn refresh_preview_from_editor(");
    let judge = body.find("shows_editor_lines()");
    let restore = body.find("restore_rendered_preview(");
    let apply = body.find("apply_editor_text(");
    assert!(
        judge.is_some() && restore.is_some(),
        "{MAIN}:{head} の `refresh_preview_from_editor` が `shows_editor_lines` を見て\
         `restore_rendered_preview` へ分けていない（#1661。抜けた後の `tako edit save` /\
         読み直しでも Markdown がコード表示へ落ちる）"
    );
    assert!(
        judge < apply && restore < apply,
        "{MAIN}:{head} の `refresh_preview_from_editor` が判定より先に `apply_editor_text` を\
         呼んでいる（#1661。判定はエディタの行を組む前に見る）"
    );
}

#[test]
fn 編集を抜ける口がmarkdownを描画へ戻す() {
    let code = read_code(MAIN);
    let body = fn_body(&code, "set_preview_editing_local", MAIN);
    assert!(
        body.contains("resumes_rendered()") && body.contains("restore_rendered_preview("),
        "{MAIN}:{} の `set_preview_editing_local` が編集を抜けたときに描き直していない\
         （#1661。Issue の本体: Markdown を編集して抜けてもコード表示のまま・目次は空のまま）",
        line_of(&code, "fn set_preview_editing_local(")
    );
}

#[test]
fn 描き直しはディスクではなく本文から組む() {
    let code = read_code(MAIN);
    let body = fn_body(&code, "restore_rendered_preview", MAIN);
    let head = line_of(&code, "fn restore_rendered_preview(");
    assert!(
        body.contains("buffer.text()")
            && body.contains("markdown_from_text(")
            && body.contains("pending_md_resumes"),
        "{MAIN}:{head} の `restore_rendered_preview` が編集セッションの本文から組んでいない\
         （#1661。小さい文書はその場で `markdown_from_text`、大きい文書は `pending_md_resumes` へ）"
    );
    for disk in [
        "load_for_reload(",
        "spawn_preview_load(",
        "pending_preview_loads",
    ] {
        assert!(
            !body.contains(disk),
            "{MAIN}:{head} の `restore_rendered_preview` がディスクから読み直している（`{disk}`）\
             （#1661。未保存のまま抜けると書いた見出しが描画と目次から消える）"
        );
    }
}

#[test]
fn レイアウトへは抜けた先のモードを書く() {
    let code = read_code(MAIN);
    let body = fn_body(&code, "save_layout", MAIN);
    assert!(
        body.contains("preview::layout_mode("),
        "{MAIN}:{} の `save_layout` がプレビューの表示モードを `preview::layout_mode` から\
         採っていない（#1661。編集中の `code` を書くと再起動で Markdown がコード表示のまま戻る）",
        line_of(&code, "fn save_layout(")
    );
}

#[test]
fn 編集中の目次は描画の目次と同じパーサ設定で作る() {
    let code = read_code(PREVIEW);
    for name in ["markdown_source_outline", "parse_markdown_blocks"] {
        let body = fn_body(&code, name, PREVIEW);
        assert!(
            body.contains("markdown_options()") && body.contains("strip_bom("),
            "{PREVIEW}:{} の `{name}` がパーサのオプション（`markdown_options`）か BOM の剥がし方を\
             共有していない（#1661。描画の目次と編集中の目次の k 番目が食い違い、\
             項目番号と「見ていた節」の対応がずれる）",
            line_of(&code, &format!("fn {name}("))
        );
    }
    // オプションを組むのは `markdown_options` の中だけ
    let options_fn = line_of(&code, "fn markdown_options(");
    let mut current_fn: Option<&str> = None;
    for (index, line) in code.lines().enumerate() {
        if let Some(name) = tako_core::source_scan::fn_head_name(line) {
            current_fn = Some(name);
        }
        if line.contains("Options::ENABLE_") {
            assert_eq!(
                current_fn,
                Some("markdown_options"),
                "{PREVIEW}:{} がパーサのオプションを `markdown_options`（{PREVIEW}:{options_fn}）の\
                 外で組んでいる（#1661。2 か所に分かれると描画と編集中の目次が割れる）",
                index + 1
            );
        }
    }
}

#[test]
fn ab_の口は判定の1か所から効く() {
    let code = read_code(PREVIEW);
    let legacy = fn_body(&code, "md_edit_resume_legacy", PREVIEW);
    assert!(
        legacy.contains("\"TAKO_1661_LEGACY\""),
        "{PREVIEW}:{} の `md_edit_resume_legacy` が `TAKO_1661_LEGACY` を読んでいない",
        line_of(&code, "fn md_edit_resume_legacy(")
    );
    for name in ["resumes_rendered", "offers_source_outline"] {
        let body = fn_body(&code, name, PREVIEW);
        assert!(
            body.contains("md_edit_resume_legacy()"),
            "{PREVIEW}:{} の `{name}` が A/B の口を見ていない（#1661。同じバイナリで\
             修正前の挙動を再現できず、検証の検出力を示せない）",
            line_of(&code, &format!("fn {name}("))
        );
    }
}
