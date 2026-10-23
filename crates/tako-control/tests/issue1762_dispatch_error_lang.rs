//! dispatch の断り（`DispatchError` の表示）が表示言語に従うことの番犬（Issue #1762）。
//!
//! #1762 以前は表示の前置き（「無効なパラメータ: 」など）を日本語で固定していたので、
//! 英語表示では表示言語で書かれた理由の文と混ざった（「無効なパラメータ: Invalid color value: …」）。
//! ここで固定するのは次の 4 つ:
//! - **全バリアントが日英を持つ**: 英語側にかな・漢字が残らない。バリアントを足すと
//!   [`variant_index`] の網羅 match がコンパイルで落ち、[`samples`] へ足すまで通らない
//! - **日本語は #1762 より前とバイト一致**（[`OLD_JA`] の旧文の表）
//! - `Display` は表示言語のグローバル（`tako_core::i18n::lang()`）で `text_in` を引く
//! - 「見つからない」の読み（`says_not_found`）が日英どちらの表示にも当たる
//!   （リモートの HTTP 層が 404 へ寄せる判定に使う）
//!
//! #2358 で同じ 4 つを、dispatch が理由の文へそのまま載せる tako-core の断り
//! （`PaneTreeError` / `WorkspaceError`）と、dispatch へ届く前に CLI / MCP が手元で出す
//! ペイン未指定の断り（`NoTargetPaneAt`）にも広げた（下半分）。
//!
//! このファイルは**独立したテストバイナリ**で、言語グローバルを触るのは
//! `display_follows_the_display_language` の 1 本だけ（ほかは `text_in` で言語を引数に渡す）。

use tako_control::DispatchError as E;
use tako_control::NoTargetPaneAt;
use tako_core::i18n::{self, Lang};
use tako_core::{PaneId, PaneTreeError, SplitAxis, TabId, WindowId, WorkspaceError};

/// バリアントの通し番号。**網羅 match**なので、バリアントを足すとここがコンパイルで落ちる
/// = [`samples`] に足し、[`OLD_JA`] か英語の検査に載せるまで通らない
fn variant_index(e: &E) -> usize {
    match e {
        E::PaneNotFound(_) => 0,
        E::TabNotFound(_) => 1,
        E::NoTargetPane => 2,
        E::NoSession(_) => 3,
        E::InvalidParams(_) => 4,
        E::Operation(_) => 5,
    }
}
const VARIANTS: usize = 6;

/// 全バリアントの見本。理由の文は ASCII にしてある（英語側の検査で理由の文の日本語を拾わないため）
fn samples() -> Vec<E> {
    vec![
        E::PaneNotFound(42),
        E::TabNotFound(7),
        E::NoTargetPane,
        E::NoSession(3),
        E::InvalidParams("Invalid color value: #zz".into()),
        E::Operation("operation failed".into()),
    ]
}

/// #1762 より前の固定文（`#[error(...)]` の書式を [`samples`] の値で展開したもの）
const OLD_JA: [&str; VARIANTS] = [
    "ペイン 42 が見つからない",
    "タブ 7 が見つからない",
    "対象ペインが未指定（--pane 指定か TAKO_PANE_ID が必要）",
    "ペイン 3 にはターミナルセッションがない",
    "無効なパラメータ: Invalid color value: #zz",
    "operation failed",
];

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

#[test]
fn 見本は全バリアントを1つずつ持つ() {
    let mut seen = [false; VARIANTS];
    for e in samples() {
        let i = variant_index(&e);
        assert!(!seen[i], "見本に同じバリアントが 2 つある: {e:?}");
        seen[i] = true;
    }
    let missing: Vec<usize> = (0..VARIANTS).filter(|i| !seen[*i]).collect();
    assert!(
        missing.is_empty(),
        "見本に無いバリアント（通し番号）: {missing:?}"
    );
}

#[test]
fn 全バリアントが日英を持ち英語に日本語が混ざらない() {
    for e in samples() {
        let ja = e.text_in(Lang::Ja);
        let en = e.text_in(Lang::En);
        assert!(!ja.trim().is_empty(), "{e:?} の日本語が空");
        assert!(!en.trim().is_empty(), "{e:?} の英語が空");
        if let Some(c) = has_japanese(&en) {
            panic!("{e:?} の英語に日本語（{c}）が混ざる: {en}");
        }
        // 前置きを持つバリアントは日英で違う文になる（英語の書き忘れで日本語を写していない）
        if !matches!(e, E::Operation(_)) {
            assert_ne!(ja, en, "{e:?} の英語が日本語と同じ文");
        }
    }
}

#[test]
fn 日本語は1762より前の固定文とバイト一致() {
    for e in samples() {
        let want = OLD_JA[variant_index(&e)];
        assert_eq!(e.text_in(Lang::Ja), want, "{e:?}");
    }
}

/// 境界: 理由の文が空・入れ子（`: ` が 2 段）でも前置きを 1 回だけ付けて中身は変えない
#[test]
fn 理由の文が空でも入れ子でも前置きは1回だけ() {
    let empty = E::InvalidParams(String::new());
    assert_eq!(empty.text_in(Lang::Ja), "無効なパラメータ: ");
    assert_eq!(empty.text_in(Lang::En), "invalid parameter: ");
    assert_eq!(E::Operation(String::new()).text_in(Lang::En), "");

    let nested = "Invalid color value: #zz: contains 'z'";
    let e = E::InvalidParams(nested.into());
    assert_eq!(e.text_in(Lang::Ja), format!("無効なパラメータ: {nested}"));
    assert_eq!(e.text_in(Lang::En), format!("invalid parameter: {nested}"));
    // 日本語の理由の文は英語表示でも書き換えない（理由の文の日英は作る側の責務 = #1762 の外）
    let ja_reason = E::InvalidParams("master_ratio は 0.1〜0.9".into());
    assert_eq!(
        ja_reason.text_in(Lang::En),
        "invalid parameter: master_ratio は 0.1〜0.9"
    );
}

/// `Display`（= IPC の受け口が CLI / MCP へ返す文）は表示言語のグローバルで `text_in` を引く。
/// 言語グローバルを触るのはこのバイナリでこの 1 本だけ
#[test]
fn display_follows_the_display_language() {
    let original = i18n::lang();
    for lang in [Lang::Ja, Lang::En] {
        i18n::set_lang(lang);
        for e in samples() {
            assert_eq!(e.to_string(), e.text_in(lang), "{e:?} / {lang:?}");
        }
        // #2358: tako-core の断りと手元のペイン未指定の断りも同じグローバルを引く
        for e in tree_samples() {
            assert_eq!(e.to_string(), e.text_in(lang), "{e:?} / {lang:?}");
        }
        for e in workspace_samples() {
            assert_eq!(e.to_string(), e.text_in(lang), "{e:?} / {lang:?}");
        }
        for at in FRONTS {
            assert_eq!(at.to_string(), at.text_in(lang), "{at:?} / {lang:?}");
        }
        // dispatch は tako-core の断りを `op_err`（= `Operation(e.to_string())`）で包む。
        // 包んだ表示は理由の文そのもの（前置きは足さない）で、表示言語は 1 つにそろう
        for e in workspace_samples() {
            let wrapped = E::Operation(e.to_string());
            assert_eq!(wrapped.to_string(), e.text_in(lang), "{e:?} / {lang:?}");
        }
    }
    i18n::set_lang(original);
}

/// 「見つからない」の読みは、見つからない系のバリアントの日英どちらの表示にも当たり、
/// ほかのバリアントの前置きには当たらない
#[test]
fn 見つからないの読みは日英どちらの表示にも当たる() {
    for lang in [Lang::Ja, Lang::En] {
        for e in samples() {
            let text = e.text_in(lang);
            let not_found = matches!(e, E::PaneNotFound(_) | E::TabNotFound(_));
            assert_eq!(
                E::says_not_found(&text),
                not_found,
                "{e:?} / {lang:?}: {text}"
            );
        }
    }
    // 理由の文に書かれた「見つからない」（#1762 より前から 404 へ寄せていた形）も当たる
    assert!(E::says_not_found("タスクが見つからない: u-1"));
    assert!(!E::says_not_found("接続が切断された"));
}

// ---- #2358: tako-core の断り（PaneTreeError / WorkspaceError）と手元のペイン未指定の断り ----

/// `PaneTreeError` の通し番号（網羅 match = バリアントを足すとここがコンパイルで落ちる）
fn tree_index(e: &PaneTreeError) -> usize {
    match e {
        PaneTreeError::PaneNotFound(_) => 0,
        PaneTreeError::LastPane => 1,
        PaneTreeError::NoResizableSplit(..) => 2,
    }
}
const TREE_VARIANTS: usize = 3;

fn tree_samples() -> Vec<PaneTreeError> {
    vec![
        PaneTreeError::PaneNotFound(PaneId::from_raw(42)),
        PaneTreeError::LastPane,
        PaneTreeError::NoResizableSplit(PaneId::from_raw(3), SplitAxis::Horizontal),
    ]
}

/// #2358 より前の固定文（`#[error(...)]` の書式を [`tree_samples`] の値で展開したもの）
const TREE_OLD_JA: [&str; TREE_VARIANTS] = [
    "ペイン 42 が見つからない",
    "最後の 1 ペインは閉じられない（タブごと閉じる操作は Workspace 側で行う）",
    "ペイン 3 には Horizontal 軸でリサイズできる分割がない",
];

/// `WorkspaceError` の通し番号（網羅 match）
fn workspace_index(e: &WorkspaceError) -> usize {
    match e {
        WorkspaceError::TabNotFound(_) => 0,
        WorkspaceError::PaneNotFound(_) => 1,
        WorkspaceError::LastTab => 2,
        WorkspaceError::AlreadyInTab(..) => 3,
        WorkspaceError::MoveOntoSelf(_) => 4,
        WorkspaceError::WindowNotFound(_) => 5,
        WorkspaceError::LastWindow => 6,
    }
}
const WORKSPACE_VARIANTS: usize = 7;

fn workspace_samples() -> Vec<WorkspaceError> {
    vec![
        WorkspaceError::TabNotFound(TabId::from_raw(7)),
        WorkspaceError::PaneNotFound(PaneId::from_raw(42)),
        WorkspaceError::LastTab,
        WorkspaceError::AlreadyInTab(PaneId::from_raw(5), TabId::from_raw(2)),
        WorkspaceError::MoveOntoSelf(PaneId::from_raw(5)),
        WorkspaceError::WindowNotFound(WindowId::from_raw(9)),
        WorkspaceError::LastWindow,
    ]
}

/// #2358 より前の固定文（[`workspace_samples`] の値で展開したもの）
const WORKSPACE_OLD_JA: [&str; WORKSPACE_VARIANTS] = [
    "タブ 7 が見つからない",
    "ペイン 42 が見つからない",
    "最後の 1 タブは閉じられない（アプリ終了は UI 層の責務）",
    "ペイン 5 は既にタブ 2 にある",
    "ペイン 5 を自分自身の隣へは移動できない",
    "ウィンドウ 9 が見つからない",
    "最後の 1 ウィンドウは閉じられない",
];

/// 手元で断る口の通し番号（網羅 match）
fn front_index(at: NoTargetPaneAt) -> usize {
    match at {
        NoTargetPaneAt::Cli => 0,
        NoTargetPaneAt::Mcp => 1,
    }
}
const FRONTS: [NoTargetPaneAt; 2] = [NoTargetPaneAt::Cli, NoTargetPaneAt::Mcp];

/// #2358 より前に CLI（`tako-cli` の `target_pane`）と MCP（`mcp::request` の `target_pane`）が
/// 書いていた固定文
const FRONT_OLD_JA: [&str; 2] = [
    "対象ペインを特定できない（--pane を指定するか、tako アプリ内のターミナルで実行する）",
    "対象ペインを特定できない（pane を指定する。呼び出し元ペインの自動特定には TAKO_PANE_ID / X-Tako-Pane が必要）",
];

/// 見本が全バリアントを 1 つずつ持つか（`index` は網羅 match の通し番号）
fn covers_all<T: std::fmt::Debug>(samples: &[T], index: impl Fn(&T) -> usize, n: usize) {
    let mut seen = vec![false; n];
    for e in samples {
        let i = index(e);
        assert!(!seen[i], "見本に同じバリアントが 2 つある: {e:?}");
        seen[i] = true;
    }
    let missing: Vec<usize> = (0..n).filter(|i| !seen[*i]).collect();
    assert!(
        missing.is_empty(),
        "見本に無いバリアント（通し番号）: {missing:?}"
    );
}

/// 日英の両方を持ち、英語に日本語が混ざらず、英語が日本語の写しでないこと
fn bilingual(label: &str, ja: &str, en: &str) {
    assert!(!ja.trim().is_empty(), "{label} の日本語が空");
    assert!(!en.trim().is_empty(), "{label} の英語が空");
    if let Some(c) = has_japanese(en) {
        panic!("{label} の英語に日本語（{c}）が混ざる: {en}");
    }
    assert_ne!(ja, en, "{label} の英語が日本語と同じ文");
}

#[test]
fn core_の断りと手元の断りの見本は全バリアントを1つずつ持つ() {
    covers_all(&tree_samples(), tree_index, TREE_VARIANTS);
    covers_all(&workspace_samples(), workspace_index, WORKSPACE_VARIANTS);
    covers_all(&FRONTS, |at| front_index(*at), FRONTS.len());
}

#[test]
fn core_の断りと手元の断りが日英を持ち英語に日本語が混ざらない() {
    for e in tree_samples() {
        bilingual(
            &format!("{e:?}"),
            &e.text_in(Lang::Ja),
            &e.text_in(Lang::En),
        );
    }
    for e in workspace_samples() {
        bilingual(
            &format!("{e:?}"),
            &e.text_in(Lang::Ja),
            &e.text_in(Lang::En),
        );
    }
    for at in FRONTS {
        bilingual(
            &format!("{at:?}"),
            at.text_in(Lang::Ja),
            at.text_in(Lang::En),
        );
    }
}

#[test]
fn core_の断りと手元の断りの日本語は2358より前の固定文とバイト一致() {
    for e in tree_samples() {
        assert_eq!(e.text_in(Lang::Ja), TREE_OLD_JA[tree_index(&e)], "{e:?}");
    }
    for e in workspace_samples() {
        assert_eq!(
            e.text_in(Lang::Ja),
            WORKSPACE_OLD_JA[workspace_index(&e)],
            "{e:?}"
        );
    }
    for at in FRONTS {
        assert_eq!(
            at.text_in(Lang::Ja),
            FRONT_OLD_JA[front_index(at)],
            "{at:?}"
        );
    }
}

/// 境界: tako-core の断りを dispatch の前置きつきの断りへ入れ子にしても、前置きは 1 回だけで
/// 中身は変えない（言語ごとに `text_in` でそろえて包む）
#[test]
fn core_の断りを入れ子にしても前置きは1回だけ() {
    for lang in [Lang::Ja, Lang::En] {
        let prefix = E::InvalidParams(String::new()).text_in(lang);
        for e in workspace_samples() {
            let inner = e.text_in(lang);
            let outer = E::InvalidParams(inner.clone()).text_in(lang);
            assert_eq!(outer, format!("{prefix}{inner}"), "{e:?} / {lang:?}");
            assert_eq!(outer.matches(prefix.as_str()).count(), 1, "{outer}");
            assert_eq!(E::Operation(inner.clone()).text_in(lang), inner);
        }
    }
}

/// 「見つからない」の読みは tako-core の見つからない系だけに日英どちらでも当たる
/// （リモートの HTTP 層が 404 へ寄せる判定。#2358 より前の日本語の当たり方と同じ）
#[test]
fn core_の断りの見つからないの読みは日英どちらにも当たる() {
    for lang in [Lang::Ja, Lang::En] {
        for e in tree_samples() {
            let text = e.text_in(lang);
            let not_found = matches!(e, PaneTreeError::PaneNotFound(_));
            assert_eq!(
                E::says_not_found(&text),
                not_found,
                "{e:?} / {lang:?}: {text}"
            );
        }
        for e in workspace_samples() {
            let text = e.text_in(lang);
            let not_found = matches!(
                e,
                WorkspaceError::TabNotFound(_)
                    | WorkspaceError::PaneNotFound(_)
                    | WorkspaceError::WindowNotFound(_)
            );
            assert_eq!(
                E::says_not_found(&text),
                not_found,
                "{e:?} / {lang:?}: {text}"
            );
        }
        for at in FRONTS {
            let text = at.text_in(lang);
            assert!(!E::says_not_found(text), "{at:?} / {lang:?}: {text}");
        }
    }
}
