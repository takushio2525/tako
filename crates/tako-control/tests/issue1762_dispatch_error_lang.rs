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
//! ペイン未指定の断り（`NoTargetPaneAt`）にも広げた（中ほど）。
//!
//! #2365 で、dispatch と前段（CLI / MCP の引数の組み立て）が自分で組む理由の文（`Reason`）と
//! `orchestrator layout` の値の断り（`tako_core::LayoutParamError`）にも広げた（下半分）。
//! こちらは落ちたときに、日英の対を書いたソースの行（`file:line`）も名指しする。
//!
//! このファイルは**独立したテストバイナリ**で、言語グローバルを触るのは
//! `display_follows_the_display_language` の 1 本だけ（ほかは `text_in` で言語を引数に渡す）。

use tako_control::DispatchError as E;
use tako_control::{NoTargetPaneAt, Reason};
use tako_core::i18n::{self, Lang};
use tako_core::{
    LayoutParamError, PaneId, PaneTreeError, SplitAxis, TabId, WindowId, WorkspaceError,
};

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
        // #2365: dispatch と前段の理由の文・layout の値の断りも同じグローバルを引き、
        // `invalid` / `operation` で載せた断りは前置き + その表示言語の理由の文になる
        for r in reason_samples() {
            assert_eq!(r.to_string(), r.text_in(lang), "{r:?} / {lang:?}");
            let prefix = E::InvalidParams(String::new()).text_in(lang);
            assert_eq!(
                r.clone().invalid().to_string(),
                format!("{prefix}{}", r.text_in(lang)),
                "{r:?} / {lang:?}"
            );
            assert_eq!(r.clone().operation().to_string(), r.text_in(lang));
        }
        for e in layout_samples() {
            assert_eq!(e.to_string(), e.text_in(lang), "{e:?} / {lang:?}");
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

// ---- #2365: dispatch と前段の理由の文（Reason）と layout の値の断り（LayoutParamError） ----

/// `Reason` の通し番号（網羅 match = バリアントを足すとここがコンパイルで落ちる）
fn reason_index(r: &Reason) -> usize {
    match r {
        Reason::Missing(_) => 0,
        Reason::Required(_) => 1,
        Reason::NotNonNegInt(_) => 2,
        Reason::NotInteger(_) => 3,
        Reason::NotNumber(_) => 4,
        Reason::NotString(_) => 5,
        Reason::NotBool(_) => 6,
        Reason::ElementNotString(_) => 7,
        Reason::NotStringArray(_) => 8,
        Reason::NotObject(_) => 9,
        Reason::EnvValueNotString { .. } => 10,
        Reason::InvalidDirection(_) => 11,
        Reason::NotTogether(..) => 12,
        Reason::EitherOf(..) => 13,
        Reason::UnknownTool(_) => 14,
        Reason::UnknownParams { .. } => 15,
        Reason::NoToolName => 16,
        Reason::SessionLogNotFound(_) => 17,
        Reason::PaneLogNotFound(_) => 18,
        Reason::TranscriptNotFound(_) => 19,
        Reason::ProjectNotFound(_) => 20,
        Reason::PaneGone => 21,
        Reason::TabGone => 22,
        Reason::TabNotFound(_) => 23,
        Reason::MasterPaneNotFound => 24,
        Reason::BackendSessionNotFound(_) => 25,
        Reason::NoChoiceDialog => 26,
        Reason::ChoiceNotFound { .. } => 27,
        Reason::PasteTargetNotFound(_) => 28,
        Reason::CheckpointNotFound(_) => 29,
        Reason::NotPreviewPane(_) => 30,
        Reason::NotMarkdownPdfPane(_) => 31,
        Reason::NotPdfImagePane(_) => 32,
        Reason::PathNotFound(_) => 33,
        Reason::NotAFile(_) => 34,
        Reason::DirNotFound(_) => 35,
        Reason::AlreadyExists(_) => 36,
        Reason::NotGitRepo => 37,
        Reason::NoGitRepo => 38,
        Reason::EmptyProfileName(_) => 39,
        Reason::ModTakesNoArgs(_) => 40,
        Reason::FocusTarget => 41,
        Reason::ScrollTarget => 42,
        Reason::PasteGuardRuleUsage => 43,
        Reason::PasteGuardPaneOnlyCheck => 44,
        Reason::SearchQuery => 45,
        Reason::DirectionNeedsTarget => 46,
        Reason::MoveDestination => 47,
        Reason::EnvAndUnsetEnv(_) => 48,
        Reason::GatePredicate => 49,
        Reason::CliInvalidDirection(_) => 50,
    }
}
const REASON_VARIANTS: usize = 51;

/// 全バリアントの見本（値は ASCII。英語側の検査で値の日本語を拾わないため）
fn reason_samples() -> Vec<Reason> {
    vec![
        Reason::Missing("name".into()),
        Reason::Required("id".into()),
        Reason::NotNonNegInt("pane".into()),
        Reason::NotInteger("delta".into()),
        Reason::NotNumber("ratio".into()),
        Reason::NotString("cwd".into()),
        Reason::NotBool("focus".into()),
        Reason::ElementNotString("command".into()),
        Reason::NotStringArray("paths".into()),
        Reason::NotObject("env".into()),
        Reason::EnvValueNotString {
            key: "env".into(),
            name: "PATH".into(),
        },
        Reason::InvalidDirection("north".into()),
        Reason::NotTogether("click", "text"),
        Reason::EitherOf("pane", "tab"),
        Reason::UnknownTool("tako_nope".into()),
        Reason::UnknownParams {
            tool: "tako_theme".into(),
            unknown: vec!["colour".into(), "hue".into()],
            allowed: vec!["action".into(), "key".into()],
        },
        Reason::NoToolName,
        Reason::SessionLogNotFound("abc12345".into()),
        Reason::PaneLogNotFound(42),
        Reason::TranscriptNotFound("abc12345".into()),
        Reason::ProjectNotFound("web".into()),
        Reason::PaneGone,
        Reason::TabGone,
        Reason::TabNotFound(7),
        Reason::MasterPaneNotFound,
        Reason::BackendSessionNotFound(3),
        Reason::NoChoiceDialog,
        Reason::ChoiceNotFound {
            choice: "9".into(),
            labels: "1. Yes / 2. No".into(),
        },
        Reason::PasteTargetNotFound("/tmp/x".into()),
        Reason::CheckpointNotFound("t-1".into()),
        Reason::NotPreviewPane(5),
        Reason::NotMarkdownPdfPane(5),
        Reason::NotPdfImagePane(5),
        Reason::PathNotFound("/tmp/x".into()),
        Reason::NotAFile("/tmp/x".into()),
        Reason::DirNotFound("/tmp/x".into()),
        Reason::AlreadyExists("/tmp/x".into()),
        Reason::NotGitRepo,
        Reason::NoGitRepo,
        Reason::EmptyProfileName("tako master -2"),
        Reason::ModTakesNoArgs("install".into()),
        Reason::FocusTarget,
        Reason::ScrollTarget,
        Reason::PasteGuardRuleUsage,
        Reason::PasteGuardPaneOnlyCheck,
        Reason::SearchQuery,
        Reason::DirectionNeedsTarget,
        Reason::MoveDestination,
        Reason::EnvAndUnsetEnv("PATH".into()),
        Reason::GatePredicate,
        Reason::CliInvalidDirection("north".into()),
    ]
}

/// #2365 より前に dispatch・MCP の前段（`mcp/request.rs` / `mcp/mod.rs`）・CLI の前段
/// （`tako-cli` の main.rs）が書いていた固定文を、[`reason_samples`] の値で展開したもの
const REASON_OLD_JA: [&str; REASON_VARIANTS] = [
    "name を指定する",
    "id は必須",
    "pane は非負整数で指定する",
    "delta は整数で指定する",
    "ratio は数値で指定する",
    "cwd は文字列で指定する",
    "focus は真偽値で指定する",
    "command の要素は文字列で指定する",
    "paths は文字列の配列で指定する",
    "env は object（名前 → 値）で指定する",
    "env の値は文字列か null で指定する（PATH）",
    "direction が不正: north（right / down / left / up のいずれか）",
    "click と text は同時に指定しない",
    "pane または tab を指定してください",
    "不明なツール: tako_nope",
    "未知のパラメータ: colour, hue（tako_theme が受け付けるのは action, key のみ）",
    "ツール名（name）が無い",
    "セッション 'abc12345' の端末ログが見つからない（ログ保存が OFF だったか、上限で削除済み）",
    "ペイン 42 のログが見つからない（ログ保存が OFF だったか、上限で削除済み）",
    "セッション abc12345 の会話ログ（~/.claude/projects/ の transcript）が見つからない。claude 側で削除された可能性がある",
    "プロジェクト 'web' が見つからない",
    "ペインが見つからない",
    "タブが見つからない",
    "タブ 7 が見つからない",
    "master/solo ペインが見つからない（pane を明示指定するか、TAKO_ORCHESTRATOR_ROLE を確認してください）",
    "ペイン 3 のバックエンドセッションが見つからない",
    "ペイン画面に選択肢ダイアログが見つからない（既に解消済みか、別の画面状態です）",
    "9 に対応する選択肢が見つからない。選択肢: 1. Yes / 2. No",
    "貼り付け先が見つからない: /tmp/x",
    "チェックポイントが見つからない: t-1",
    "プレビューペインではない: 5",
    "Markdown・PDF プレビューペインではない: 5",
    "PDF・画像プレビューペインではない: 5",
    "パスが存在しない: /tmp/x",
    "ファイルではない: /tmp/x",
    "ディレクトリが存在しない: /tmp/x",
    "既に存在する: /tmp/x",
    "git リポジトリではない",
    "git リポジトリが見つかりません",
    "プロファイル名が空です（例: tako master -2）",
    "install は on / off や ui の引数を取らない",
    "フォーカス先のペイン ID か方向（--left 等）を指定する",
    "--to（絶対位置。0 = 最下部）か --delta（相対行数）を指定する",
    "規則の切替は `tako paste-guard rule <規則名> on|off`",
    "--pane は `tako paste-guard check` だけ",
    "検索語を指定する（tako search <検索語>）",
    "--right/--down/--up/--left は --target と併用する",
    "tab か --target か --new のいずれかを指定する",
    "PATH を --env と --unset-env の両方に渡している",
    "少なくとも 1 つの述語を指定する（--command / --pr-merged / --custom）",
    "不正な方向: north（right / down / left / up）",
];

/// 「見つからない」系（英語にも ` not found` を含める約束のもの）
fn reason_is_not_found(r: &Reason) -> bool {
    matches!(
        r,
        Reason::SessionLogNotFound(_)
            | Reason::PaneLogNotFound(_)
            | Reason::TranscriptNotFound(_)
            | Reason::ProjectNotFound(_)
            | Reason::PaneGone
            | Reason::TabGone
            | Reason::TabNotFound(_)
            | Reason::MasterPaneNotFound
            | Reason::BackendSessionNotFound(_)
            | Reason::NoChoiceDialog
            | Reason::ChoiceNotFound { .. }
            | Reason::PasteTargetNotFound(_)
            | Reason::CheckpointNotFound(_)
    )
}

/// `LayoutParamError` の通し番号（網羅 match）
fn layout_index(e: &LayoutParamError) -> usize {
    match e {
        LayoutParamError::UnknownPolicy(_) => 0,
        LayoutParamError::UnknownAlgorithm(_) => 1,
        LayoutParamError::MasterRatio(_) => 2,
        LayoutParamError::MinWorkerCols(_) => 3,
        LayoutParamError::MinWorkerFontScale(_) => 4,
    }
}
const LAYOUT_VARIANTS: usize = 5;

fn layout_samples() -> Vec<LayoutParamError> {
    vec![
        LayoutParamError::UnknownPolicy("golden".into()),
        LayoutParamError::UnknownAlgorithm("mosaic".into()),
        LayoutParamError::MasterRatio(7.0),
        LayoutParamError::MinWorkerCols(5),
        LayoutParamError::MinWorkerFontScale(2.0),
    ]
}

/// #2365 より前の固定文（`spawn_layout.rs` の parse と `dispatch_orchestrator_layout` の検証）
const LAYOUT_OLD_JA: [&str; LAYOUT_VARIANTS] = [
    "不明なレイアウトポリシー: golden（master-reserved / legacy）",
    "不明な配置アルゴリズム: mosaic（grid / spiral）",
    "master_ratio は 0.1〜0.9 で指定してください（指定値: 7）",
    "min_worker_cols は 0（保証しない）か 20〜400 で指定してください（指定値: 5）",
    "min_worker_font_scale は 0.4〜1 で指定してください（指定値: 2）",
];

const DISPATCH_SRC: &str = include_str!("../src/dispatch.rs");
const DISPATCH_PATH: &str = "crates/tako-control/src/dispatch.rs";
const LAYOUT_SRC: &str = include_str!("../../tako-core/src/spawn_layout.rs");
const LAYOUT_PATH: &str = "crates/tako-core/src/spawn_layout.rs";

/// `Debug` の表示からバリアント名を取る（`Missing("name")` → `Missing`）
fn variant_name(debug: &str) -> &str {
    let end = debug
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(debug.len());
    &debug[..end]
}

/// `text_in` の網羅 match で `(Self::<名前>…, <言語>)` の腕が始まる行（1 始まり）。
///
/// 腕は 1 行（`(Self::Missing(key), En) =>`）でも複数行（`Self::UnknownParams { … },` の
/// 次の行に `En,`）でもよい。`=>` までの間に言語の印があるものを採る。
/// 全言語を 1 本で受ける腕（`(Self::TabNotFound(id), lang)`）はどの言語でも当たる
fn arm_line(src: &str, from: &str, variant: &str, lang: Lang) -> Option<usize> {
    let token = match lang {
        Lang::Ja => "Ja",
        Lang::En => "En",
    };
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(from))?;
    let head = format!("Self::{variant}");
    for i in start..lines.len() {
        let line = lines[i];
        let Some(at) = line.find(&head) else {
            continue;
        };
        // `Self::NotString` が `Self::NotStringArray` に当たらないよう、名前の直後を見る
        let next = line[at + head.len()..].chars().next();
        if matches!(next, Some(c) if c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let mut window = String::new();
        for l in &lines[i..lines.len().min(i + 10)] {
            window.push_str(l);
            window.push('\n');
            if l.contains("=>") {
                break;
            }
        }
        let marks = [
            format!(", {token})"),
            format!("{token},\n"),
            format!(", {token} |"),
            ", lang)".to_string(),
        ];
        if marks.iter().any(|m| window.contains(m.as_str())) {
            return Some(i + 1);
        }
    }
    None
}

/// 落ちたときに添える「日英の対を書いた行」
fn where_written(src: &str, path: &str, from: &str, debug: &str, lang: Lang) -> String {
    match arm_line(src, from, variant_name(debug), lang) {
        Some(line) => format!("{path}:{line}"),
        None => format!("{path}（{} の腕が見つからない）", variant_name(debug)),
    }
}

/// [`bilingual`] と同じ検査に、腕の行を添える
fn bilingual_at(src: &str, path: &str, from: &str, debug: &str, ja: &str, en: &str) {
    let at = |lang| where_written(src, path, from, debug, lang);
    assert!(
        !ja.trim().is_empty(),
        "{debug} の日本語が空（{}）",
        at(Lang::Ja)
    );
    assert!(
        !en.trim().is_empty(),
        "{debug} の英語が空（{}）",
        at(Lang::En)
    );
    if let Some(c) = has_japanese(en) {
        panic!(
            "{debug} の英語に日本語（{c}）が混ざる: {en}（{}）",
            at(Lang::En)
        );
    }
    assert_ne!(ja, en, "{debug} の英語が日本語と同じ文（{}）", at(Lang::En));
}

#[test]
fn 理由の文の見本は全バリアントを1つずつ持つ() {
    covers_all(&reason_samples(), reason_index, REASON_VARIANTS);
    covers_all(&layout_samples(), layout_index, LAYOUT_VARIANTS);
}

#[test]
fn 理由の文が日英を持ち英語に日本語が混ざらない() {
    for r in reason_samples() {
        bilingual_at(
            DISPATCH_SRC,
            DISPATCH_PATH,
            "impl Reason {",
            &format!("{r:?}"),
            &r.text_in(Lang::Ja),
            &r.text_in(Lang::En),
        );
    }
    for e in layout_samples() {
        bilingual_at(
            LAYOUT_SRC,
            LAYOUT_PATH,
            "impl LayoutParamError {",
            &format!("{e:?}"),
            &e.text_in(Lang::Ja),
            &e.text_in(Lang::En),
        );
    }
}

#[test]
fn 理由の文の日本語は2365より前の固定文とバイト一致() {
    for r in reason_samples() {
        let debug = format!("{r:?}");
        assert_eq!(
            r.text_in(Lang::Ja),
            REASON_OLD_JA[reason_index(&r)],
            "{debug}（{}）",
            where_written(
                DISPATCH_SRC,
                DISPATCH_PATH,
                "impl Reason {",
                &debug,
                Lang::Ja
            )
        );
    }
    for e in layout_samples() {
        let debug = format!("{e:?}");
        assert_eq!(
            e.text_in(Lang::Ja),
            LAYOUT_OLD_JA[layout_index(&e)],
            "{debug}（{}）",
            where_written(
                LAYOUT_SRC,
                LAYOUT_PATH,
                "impl LayoutParamError {",
                &debug,
                Lang::Ja
            )
        );
    }
}

/// 番犬自身の検査: 全バリアントの腕の行を日英とも引ける（引けないと名指しが空振りする）
#[test]
fn 理由の文の腕の行を日英とも引ける() {
    for lang in [Lang::Ja, Lang::En] {
        for r in reason_samples() {
            let debug = format!("{r:?}");
            assert!(
                arm_line(DISPATCH_SRC, "impl Reason {", variant_name(&debug), lang).is_some(),
                "{debug} / {lang:?}"
            );
        }
        for e in layout_samples() {
            let debug = format!("{e:?}");
            assert!(
                arm_line(
                    LAYOUT_SRC,
                    "impl LayoutParamError {",
                    variant_name(&debug),
                    lang
                )
                .is_some(),
                "{debug} / {lang:?}"
            );
        }
    }
}

/// 「見つからない」の読み（リモートの 404 寄せ）は、理由の文でも日英で同じ当たり方をする。
/// ダイアログ不在の読み（リモートの承認応答の 409 寄せ）も日英の両方に当たる
#[test]
fn 理由の文の見つからないとダイアログ不在の読みは日英で同じ() {
    for r in reason_samples() {
        for lang in [Lang::Ja, Lang::En] {
            let text = r.text_in(lang);
            assert_eq!(
                E::says_not_found(&text),
                reason_is_not_found(&r),
                "{r:?} / {lang:?}: {text}"
            );
            assert_eq!(
                Reason::says_no_choice_dialog(&text),
                matches!(r, Reason::NoChoiceDialog),
                "{r:?} / {lang:?}: {text}"
            );
            // 前置きつき・前置きなしで載せても読みは変わらない
            let invalid = E::InvalidParams(text.clone()).text_in(lang);
            assert_eq!(E::says_not_found(&invalid), reason_is_not_found(&r));
        }
    }
    for e in layout_samples() {
        for lang in [Lang::Ja, Lang::En] {
            assert!(!E::says_not_found(&e.text_in(lang)), "{e:?} / {lang:?}");
        }
    }
}

/// 境界: 引数を取らないツールの未知のパラメータ（受け付けるものが空）・非有限値の layout
#[test]
fn 理由の文の境界() {
    let none = Reason::UnknownParams {
        tool: "tako_list_panes".into(),
        unknown: vec!["x".into()],
        allowed: Vec::new(),
    };
    assert_eq!(
        none.text_in(Lang::Ja),
        "未知のパラメータ: x（tako_list_panes が受け付けるのは 引数なし のみ）"
    );
    assert_eq!(
        none.text_in(Lang::En),
        "unknown parameter: x (tako_list_panes takes no arguments)"
    );
    let nan = LayoutParamError::MasterRatio(f32::NAN);
    assert_eq!(
        nan.text_in(Lang::Ja),
        "master_ratio は 0.1〜0.9 で指定してください（指定値: NaN）"
    );
    assert_eq!(
        nan.text_in(Lang::En),
        "master_ratio must be between 0.1 and 0.9 (given: NaN)"
    );
    // 理由の文の `TabNotFound` は dispatch の `TabNotFound` と同じ文（写しを持たない）
    for lang in [Lang::Ja, Lang::En] {
        assert_eq!(
            // 固定 ID 安全: 文の表を比べるだけで、タブを採番も探索もしない
            Reason::TabNotFound(7).text_in(lang),
            // 固定 ID 安全: 同上
            E::TabNotFound(7).text_in(lang)
        );
    }
}
