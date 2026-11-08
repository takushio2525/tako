//! **#2776 の番犬**: 依存表の Tailscale の行が、remote の対応表の判定（setup の末尾の行 = #2768 と
//! 同じ 1 実装）を通らずに出る形へ戻らないようにする。
//!
//! ## 何が起きていたのか
//!
//! remote（スマホからの接続）は Windows ではまだ使えない（対応表の `tako_remote_setup` が Pending =
//! #971）。setup の末尾の行は #2768 で対応表の gate を通るようになったが、依存表
//! （`tako setup deps` / MCP `tako_setup_deps` / `tako setup` の依存の段 / check-health）は
//! Windows でも winget での Tailscale の導入を勧め続けていた。入れても使い道が無い。
//!
//! ## この形
//!
//! 判定は `remote_setup::remote_unusable` の 1 実装（対応表のマス → `remote_unusable_in`）。
//! 末尾の行（`platform_unsupported`）と、この環境の依存表（`setup_deps::current_deps` →
//! `offered_deps`）が同じこれを通る。対応表が使えると言った日に、両方が一緒に戻る。
//! 判定そのもの（マスの 4 状態と未登録）と行の出る / 出ないは単体テスト
//! （`remote_setup::tests::issue2776_*` / `setup_deps::tests::issue2776_*`）が両 OS で見るので、
//! ここは**配線**だけを縛る。macOS では remote が使えるので、gate を外しても単体テストの答えは
//! 変わらない = この番犬が名指す。
//!
//! 1. [`この環境の依存表は判定の1実装を通る`] — `current_deps` が `remote_unusable` を渡す
//! 2. [`依存表は判定を自分で持たない`] — setup_deps が対応表を引き直さない・OS を `cfg` で見ない
//! 3. [`一覧は絞った表から作る`] — 一覧（`status`）が全表を直に読まない・全表は名指しの `status_of` だけ
//! 4. [`対応表のキーを引くのは判定の1実装だけ`] — remote_setup の中でも `SUMMARY_SUPPORT_KEY` を
//!    引くのは `remote_unusable` だけ
//!
//! 落ちるときは **file:line で名指し**する。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view;

const SETUP_DEPS: &str = "crates/tako-control/src/setup_deps.rs";
const REMOTE_SETUP: &str = "crates/tako-control/src/remote_setup.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 本番領域（テスト領域を潰した全文。長さと行番号は保たれる）
fn production(rel: &str) -> String {
    let path = repo_root().join(rel);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
    production_range::production(&src, rel)
}

/// コメントだけを潰した眺め（文字列リテラルは残る）
fn non_comment(rel: &str) -> String {
    code_view::without_comments_checked(&production(rel), rel)
}

fn line_of(src: &str, at: usize) -> usize {
    src[..at].matches('\n').count() + 1
}

/// 関数 `name` の本体（`{` から対応する `}` まで）のバイト範囲。波括弧はコメントと文字列を潰した
/// 眺めで数える（眺めはバイト長を保つので、同じ範囲をコメントだけ潰した眺めへそのまま当てられる）
fn fn_range(rel: &str, name: &str) -> std::ops::Range<usize> {
    let code = code_view::code_view(&production(rel));
    let needle = format!("fn {name}(");
    let head = code
        .match_indices(needle.as_str())
        .map(|(at, _)| at)
        .find(|at| *at == 0 || !code.as_bytes()[at - 1].is_ascii_alphanumeric())
        .unwrap_or_else(|| {
            panic!("{rel}: `fn {name}` が見つからない（改名したならこの番犬も直す）")
        });
    let open = head + code[head..].find('{').expect("fn の本体の `{`");
    let mut depth = 0usize;
    for (offset, byte) in code.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return open..open + offset + 1;
                }
            }
            _ => {}
        }
    }
    panic!("{rel}: `fn {name}` の本体が閉じていない");
}

/// 関数本体（コメントだけ潰した眺め）と、その先頭行番号
fn fn_body(rel: &str, name: &str) -> (String, usize) {
    let view = non_comment(rel);
    let range = fn_range(rel, name);
    let start_line = line_of(&view, range.start);
    (view[range].to_string(), start_line)
}

/// `needle` が出る行を `rel:line` で並べる（`base_line` は本体の先頭行）
fn hits(body: &str, base_line: usize, rel: &str, needle: &str) -> Vec<String> {
    body.match_indices(needle)
        .map(|(at, _)| {
            let line = base_line + body[..at].matches('\n').count();
            format!("{rel}:{line}: `{needle}`")
        })
        .collect()
}

/// **1**: この環境の依存表は remote の判定の 1 実装の答えを `offered_deps` へ渡す
#[test]
fn この環境の依存表は判定の1実装を通る() {
    let (body, base) = fn_body(SETUP_DEPS, "current_deps");
    for needle in ["offered_deps(", "remote_setup::remote_unusable("] {
        assert!(
            body.contains(needle),
            "{SETUP_DEPS}:{base}: `current_deps` が `{needle}` を通っていない。\n\
             remote が使えない OS（Windows は #971）でも Tailscale の導入を勧める形に戻る。\
             判定は `remote_setup::remote_unusable` の 1 実装（setup の末尾の行と同じ = #2768 / #2776）"
        );
    }
}

/// **2**: setup_deps は判定を自分で持たない（対応表を引き直す・OS を見る = 末尾の行と割れる）
#[test]
fn 依存表は判定を自分で持たない() {
    let view = non_comment(SETUP_DEPS);
    let mut offenders = Vec::new();
    for needle in [
        "support_for(",
        "SUMMARY_SUPPORT_KEY",
        "\"tako_remote_setup\"",
        "is_usable(",
    ] {
        offenders.extend(hits(&view, 1, SETUP_DEPS, needle));
    }
    for name in ["offered_deps", "current_deps"] {
        let (body, base) = fn_body(SETUP_DEPS, name);
        for needle in ["cfg!(", "#[cfg(", "Platform::Windows", "Platform::MacOs"] {
            offenders.extend(hits(&body, base, SETUP_DEPS, needle));
        }
    }
    assert!(
        offenders.is_empty(),
        "依存表が remote の判定を自分で持っている:\n  {}\n\
         → 判定は `remote_setup::remote_unusable`（対応表の `tako_remote_setup`）の 1 実装だけ。\
         2 か所で判定すると、対応表が使えると言った日に片方だけ残る（#2776）",
        offenders.join("\n  ")
    );
}

/// **3**: 一覧（`status` = `tako setup deps` / MCP / setup の依存の段 / check-health）は絞った表から作る。
/// 全表（`all_current_deps`）は名指しで 1 件を引く `status_of` だけ。本番コードの他の場所は
/// 依存表の全表（`setup_deps::deps(`）を直に読まない
#[test]
fn 一覧は絞った表から作る() {
    let (body, base) = fn_body(SETUP_DEPS, "status");
    assert!(
        body.contains("current_deps()") && !body.contains("all_current_deps("),
        "{SETUP_DEPS}:{base}: `status` が絞った表（`current_deps`）から一覧を作っていない（#2776）"
    );

    // 全表を読むのは `status_of` だけ（宣言の行は数えない）
    let view = non_comment(SETUP_DEPS);
    let allowed = fn_range(SETUP_DEPS, "status_of");
    let mut offenders: Vec<String> = view
        .match_indices("all_current_deps(")
        .map(|(at, _)| at)
        .filter(|at| !allowed.contains(at) && !view[..*at].ends_with("fn "))
        .map(|at| format!("{SETUP_DEPS}:{}: `all_current_deps(`", line_of(&view, at)))
        .collect();

    let mut scanned = 0usize;
    for rel in production_range::product_sources() {
        if rel == SETUP_DEPS {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(repo_root().join(&rel)) else {
            continue;
        };
        scanned += 1;
        // テスト領域は潰す（`remote_backend_gate` ほかのテストは全表の器の行を見てよい）
        let body = code_view::code_view(&production_range::scan(&src).text);
        offenders.extend(hits(&body, 1, &rel, "setup_deps::deps("));
    }
    assert!(
        scanned > 100,
        "走査が当たっていない（{scanned} ファイルしか読めていない）"
    );
    assert!(
        offenders.is_empty(),
        "依存表の全表を、勧める一覧の代わりに読んでいる:\n  {}\n\
         → 一覧は `setup_deps::status()` / `current_deps()`（remote が使えない OS では Tailscale の行が無い）。\
         全表は名指しで 1 件を引く `status_of` だけ（`tako remote setup` の導入口 = #2776）",
        offenders.join("\n  ")
    );
}

/// **4**: remote の対応表のキーを引くのは判定の 1 実装（`remote_unusable`）だけ。
/// 定数の宣言の 1 行と本体の 1 か所の他に引く場所が増えたら、判定が 2 つになっている
#[test]
fn 対応表のキーを引くのは判定の1実装だけ() {
    let view = non_comment(REMOTE_SETUP);
    let inside = fn_range(REMOTE_SETUP, "remote_unusable");
    let offenders: Vec<String> = view
        .match_indices("SUMMARY_SUPPORT_KEY")
        .map(|(at, _)| at)
        .filter(|at| !inside.contains(at))
        // 宣言の行（`pub const SUMMARY_SUPPORT_KEY: &str = …`）は数えない
        .filter(|at| !view[..*at].ends_with("const "))
        .map(|at| {
            format!(
                "{REMOTE_SETUP}:{}: `SUMMARY_SUPPORT_KEY`",
                line_of(&view, at)
            )
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "remote の対応表のキーを判定の 1 実装の外で引いている:\n  {}\n\
         → `remote_unusable(platform)` の答えを使う（setup の末尾の行と依存表の Tailscale の行が\
         別々に判定しない = #2768 / #2776）",
        offenders.join("\n  ")
    );
}
