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
//! #2365 の 2 本目で、dispatch が理由の文へそのまま載せる tako-core のエラー型（編集・
//! ターミナル・Code Runner・コマンドカード・器・LSP の答えの読み）にも広げた（後半）。
//! #2365 の 3 本目で、保存の断りに添える案内（旧 `preview_recovery_hint`）と、dispatch の個別の文の
//! うちプレビュー・編集・ファイル操作・ペイン / タブ操作の断りを `Reason` へ足した（`Reason` の表と、
//! 案内の 3 状態を見る節）。
//! #2365 の続きで、`tako mod ui` の断り（UiError 一式 = 理由・使える値・枠・形の問題・
//! 書き込みの断り）にも広げた（末尾。1 つの断りの中で日英が混ざらないよう 1 本で移した）。
//!
//! このファイルは**独立したテストバイナリ**で、言語グローバルを触るのは
//! `display_follows_the_display_language` の 1 本だけ（ほかは `text_in` で言語を引数に渡す）。

use tako_control::DispatchError as E;
use tako_control::{NoTargetPaneAt, Reason};
use tako_core::i18n::{self, Lang};
use tako_core::{
    DiskState, LayoutParamError, PaneId, PaneTreeError, SplitAxis, TabId, WindowId, WorkspaceError,
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
        // #2365 の 2 本目: dispatch が理由の文へ載せる tako-core のエラー型も同じグローバルを引く
        for c in core_cases() {
            assert_eq!(
                c.value.to_string(),
                c.value.text(lang),
                "{:?} / {lang:?}",
                c.value
            );
        }
        // #2365 の続き: tako mod ui の断り（理由・使える値・形の問題・書き込みの断り）と枠も
        // 同じグローバルを引く（CLI の `tako mod ui` と MCP の action=ui は `message()` = この表示）
        for c in ui_cases() {
            assert_eq!(
                c.value.to_string(),
                c.value.text(lang),
                "{:?} / {lang:?}",
                c.value
            );
        }
        let framed = UiError {
            what: UiReason::HotkeyShape,
            allowed: vec![Allowed::Letters, Allowed::Digits],
        };
        assert_eq!(framed.to_string(), framed.text_in(lang));
        assert_eq!(framed.message(), framed.text_in(lang));
        // setup の answers の断りは枠（ほかの項目の断り）が日本語なので、表示言語に依らず
        // 日本語で引く（1 行に日英を混ぜない）
        let setup = tako_control::setup::SetupAnswers::from_json(
            r#"{"mod_ui": {"buttons": [{"kind": "slash", "value": "bash"}]}}"#,
        )
        .unwrap_err();
        assert_eq!(
            setup,
            "mod_ui: slash で送れるのは許可リストのコマンドだけ。使える値: compact / clear / \
             context / cost / model / effort / tako（ui.json は書いていない）",
            "{lang:?}"
        );
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
        Reason::RecoveryHint(_) => 51,
        Reason::SaveRefused { .. } => 52,
        Reason::UnsavedBlocksOpen(_) => 53,
        Reason::NewTabWithDirection => 54,
        Reason::ColumnNeedsLine => 55,
        Reason::CannotOpenFile { .. } => 56,
        Reason::OpenedButNoLine { .. } => 57,
        Reason::JumpUnavailable => 58,
        Reason::InvalidValue { .. } => 59,
        Reason::NotOneOf { .. } => 60,
        Reason::TextOnlyForType(_) => 61,
        Reason::CannotCombine(..) => 62,
        Reason::CannotCombineAll(_) => 63,
        Reason::ExactlyOneOf(..) => 64,
        Reason::AtLeastOneOf(..) => 65,
        Reason::OneOfRequired(..) => 66,
        Reason::ScrollToOrDelta => 67,
        Reason::MirrorScrollUnsupported => 68,
        Reason::NoTerminalSession(_) => 69,
        Reason::DirectionOnlyWithTarget => 70,
        Reason::CannotOpenFolder { .. } => 71,
        Reason::NotAFolder(_) => 72,
        Reason::InvalidFileName => 73,
        Reason::NoParentDir => 74,
        Reason::RenameFailed(_) => 75,
        Reason::CreateFileFailed(_) => 76,
        Reason::CreateDirFailed(_) => 77,
        Reason::AppNameMissing => 78,
        Reason::MoveDestMissing => 79,
        Reason::PasteDestMissing => 80,
        Reason::NoPaths => 81,
        Reason::ManyOpUnsupported(_) => 82,
        Reason::BadCopyId(_) => 83,
        Reason::CopyNotRunning(_) => 84,
    }
}
const REASON_VARIANTS: usize = 85;

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
        // #2365 の 3 本目（案内の 3 状態は [`案内の3つの状態と添え方が日英を持ち日本語は旧文と一致`] が全部見る）
        Reason::RecoveryHint(Some(DiskState::Changed)),
        Reason::SaveRefused {
            message: "external change".into(),
            conflict: DiskState::Changed,
        },
        Reason::UnsavedBlocksOpen(None),
        Reason::NewTabWithDirection,
        Reason::ColumnNeedsLine,
        Reason::CannotOpenFile {
            path: "/tmp/x".into(),
            error: "gone".into(),
        },
        Reason::OpenedButNoLine {
            path: "/tmp/x".into(),
            error: "binary".into(),
        },
        Reason::JumpUnavailable,
        Reason::InvalidValue {
            key: "action",
            got: "sideways".into(),
            allowed: "list / back / forward".into(),
        },
        Reason::NotOneOf {
            key: "movement",
            got: "diagonal".into(),
            allowed: "left / right".into(),
        },
        Reason::TextOnlyForType("undo".into()),
        Reason::CannotCombine("tab", "target"),
        Reason::CannotCombineAll("zoom / zoom_in / zoom_out / reset"),
        Reason::ExactlyOneOf("delta", "share"),
        Reason::AtLeastOneOf("title", "role"),
        Reason::OneOfRequired("pane", "tab"),
        Reason::ScrollToOrDelta,
        Reason::MirrorScrollUnsupported,
        Reason::NoTerminalSession(3),
        Reason::DirectionOnlyWithTarget,
        Reason::CannotOpenFolder {
            path: "/tmp/x".into(),
            error: "gone".into(),
        },
        Reason::NotAFolder("/tmp/x".into()),
        Reason::InvalidFileName,
        Reason::NoParentDir,
        Reason::RenameFailed("busy".into()),
        Reason::CreateFileFailed("busy".into()),
        Reason::CreateDirFailed("busy".into()),
        Reason::AppNameMissing,
        Reason::MoveDestMissing,
        Reason::PasteDestMissing,
        Reason::NoPaths,
        Reason::ManyOpUnsupported("rename".into()),
        Reason::BadCopyId("abc".into()),
        Reason::CopyNotRunning(9),
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
    // #2365 の 3 本目（dispatch の `preview_recovery_hint` と個別の文・GUI の `set_preview` の断り）
    HINT_CHANGED_OLD_JA,
    "external change（上書き: tako edit save --force / 読み直し（自分の変更は undo で戻せる）: tako edit reload / 差分: tako edit diff（MCP は tako_preview_save の action=overwrite / reload / diff））",
    "未保存の変更があるため別ファイルを開けない（保存: tako edit save / 変更を捨てる: tako edit reload（MCP は tako_preview_save / action=reload））",
    "new_tab と direction は同時に指定できない（新しいタブには分割元が無い）",
    "column は line と一緒に指定する",
    "ファイルを開けない（/tmp/x: gone）",
    "開いたが行へ飛べない（/tmp/x: binary）",
    "ジャンプ履歴はこの tako では使えない（GUI の中でだけ持つ）",
    "action が不正: sideways（list / back / forward）",
    "movement は left / right のどれか（\"diagonal\" は無い）",
    "text は command=type のときだけ渡す（\"undo\" は打つ文字を取らない）",
    "tab と target は同時に指定できない",
    "zoom / zoom_in / zoom_out / reset は同時に指定できない",
    "delta か share のどちらか一方を指定する",
    "title か role の少なくとも一方を指定する",
    "pane か tab のどちらかが要る",
    "to（絶対位置。0 = 最下部）か delta（相対行数）のどちらか一方を指定する",
    "このホストはバックエンドペインのスクロール表示に対応していない",
    "ペイン 3 にターミナルセッションが無い",
    "direction は target 指定時のみ使える",
    "フォルダを開けない（/tmp/x: gone）",
    "フォルダではない: /tmp/x",
    "無効なファイル名",
    "親ディレクトリが取得できない",
    "リネームに失敗: busy",
    "ファイル作成に失敗: busy",
    "フォルダ作成に失敗: busy",
    "name（アプリ名）を指定する",
    "dest（移動先のフォルダ）を指定する",
    "dest（貼り付け先のフォルダ）を指定する",
    "paths を 1 つ以上指定する",
    "paths で扱えるのは clipboard_copy / clipboard_cut / trash / move / copy（rename）",
    "name は取り消すコピーの番号（copy_progress の id）: abc",
    "番号 9 のコピーは走っていない",
];

/// #2365 より前の `preview_recovery_hint` の 3 つの文（外で消された / 外で変わった / 未保存だけ）
const HINT_DELETED_OLD_JA: &str = "作り直す: tako edit save --force / 差分: tako edit diff（MCP は tako_preview_save の action=overwrite / diff）";
const HINT_CHANGED_OLD_JA: &str = "上書き: tako edit save --force / 読み直し（自分の変更は undo で戻せる）: tako edit reload / 差分: tako edit diff（MCP は tako_preview_save の action=overwrite / reload / diff）";
const HINT_UNSAVED_OLD_JA: &str =
    "保存: tako edit save / 変更を捨てる: tako edit reload（MCP は tako_preview_save / action=reload）";

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
/// 全言語を 1 本で受ける腕（`(Self::TabNotFound(id), lang)` / `(Self::Operation(m), _)`）は
/// どの言語でも当たる
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
            ", _)".to_string(),
        ];
        if marks.iter().any(|m| window.contains(m.as_str())) {
            return Some(i + 1);
        }
    }
    None
}

/// 落ちたときに添える「日英の対を書いた行」
fn where_written(src: &str, path: &str, from: &str, debug: &str, lang: Lang) -> String {
    where_arm(src, path, from, variant_name(debug), lang)
}

/// [`where_written`] の腕の頭（`Self::` の後ろ）を明示する形
fn where_arm(src: &str, path: &str, from: &str, arm: &str, lang: Lang) -> String {
    match arm_line(src, from, arm, lang) {
        Some(line) => format!("{path}:{line}"),
        None => format!("{path}（{arm} の腕が見つからない）"),
    }
}

/// `Reason` の腕の頭。1 つのバリアントを中身で腕に分けたもの（#2365 の 3 本目の案内の 3 状態）は
/// 中身まで書く（バリアント名だけだと最初の腕 = 外で消されたときの腕を名指ししてしまう）
fn reason_arm(r: &Reason) -> String {
    match r {
        Reason::RecoveryHint(Some(DiskState::Deleted)) => {
            "RecoveryHint(Some(tako_core::DiskState::Deleted)".into()
        }
        Reason::RecoveryHint(Some(_)) => "RecoveryHint(Some(_)".into(),
        Reason::RecoveryHint(None) => "RecoveryHint(None".into(),
        other => variant_name(&format!("{other:?}")).to_string(),
    }
}

/// `Reason` の日英の対を書いた行
fn reason_where(r: &Reason, lang: Lang) -> String {
    where_arm(
        DISPATCH_SRC,
        DISPATCH_PATH,
        "impl Reason {",
        &reason_arm(r),
        lang,
    )
}

/// [`bilingual`] と同じ検査に、腕の行を添える
fn bilingual_at(src: &str, path: &str, from: &str, debug: &str, ja: &str, en: &str) {
    let at = |lang| where_written(src, path, from, debug, lang);
    bilingual_named(at, debug, ja, en);
}

/// [`bilingual_at`] の本体（`at` = 言語ごとの腕の行）
fn bilingual_named(at: impl Fn(Lang) -> String, debug: &str, ja: &str, en: &str) {
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
        bilingual_named(
            |lang| reason_where(&r, lang),
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
        assert_eq!(
            r.text_in(Lang::Ja),
            REASON_OLD_JA[reason_index(&r)],
            "{r:?}（{}）",
            reason_where(&r, Lang::Ja)
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
        for r in reason_samples().iter().chain(&hint_samples()) {
            assert!(
                arm_line(DISPATCH_SRC, "impl Reason {", &reason_arm(r), lang).is_some(),
                "{r:?} / {lang:?}"
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

// ---- #2365 の 3 本目: 保存の断りに添える案内（旧 `preview_recovery_hint`）の 3 状態と添え方 ----
//
// 英語表示で「英語の断り + 日本語の案内」が混ざって出ていた（#2365 の 2 本目の worker の報告）。
// 案内は 1 つのバリアントを外部変更の状態で 3 つの腕に分けるので、見本の表（1 バリアント 1 つ）
// とは別にここで 3 状態を全部見る

/// 案内の 3 状態（外で消された / 外で変わった / 未保存だけ）
fn hint_samples() -> Vec<Reason> {
    vec![
        Reason::RecoveryHint(Some(DiskState::Deleted)),
        Reason::RecoveryHint(Some(DiskState::Changed)),
        Reason::RecoveryHint(None),
    ]
}

#[test]
fn 案内の3つの状態と添え方が日英を持ち日本語は旧文と一致() {
    let old = [
        HINT_DELETED_OLD_JA,
        HINT_CHANGED_OLD_JA,
        HINT_UNSAVED_OLD_JA,
    ];
    for (r, old) in hint_samples().iter().zip(old) {
        let (ja, en) = (r.text_in(Lang::Ja), r.text_in(Lang::En));
        bilingual_named(|lang| reason_where(r, lang), &format!("{r:?}"), &ja, &en);
        assert_eq!(ja, old, "{r:?}（{}）", reason_where(r, Lang::Ja));
        // 案内のコマンドは英語表示でも同じ綴り（そのまま打ち写せる）
        for command in ["tako edit ", "tako_preview_save", "action="] {
            assert!(
                en.contains(command),
                "{r:?} の英語に {command} が無い: {en}（{}）",
                reason_where(r, Lang::En)
            );
        }
    }
    // 外から変わっていない（Unchanged）は「外で変わった」と同じ腕（#2365 より前の `Some(_)`）
    for lang in [Lang::Ja, Lang::En] {
        assert_eq!(
            Reason::RecoveryHint(Some(DiskState::Unchanged)).text_in(lang),
            Reason::RecoveryHint(Some(DiskState::Changed)).text_in(lang)
        );
    }
    // 添え方: dispatch の保存の断り（`preview_save`）と GUI の差し替えの断り（`set_preview`）。
    // 日本語は #2365 より前の `format!("{message}（{hint}）")` /
    // `format!("未保存の変更があるため別ファイルを開けない（{hint}）")` とバイト一致
    for state in [DiskState::Deleted, DiskState::Changed] {
        let hint = Reason::RecoveryHint(Some(state));
        let refused = Reason::SaveRefused {
            message: "m".into(),
            conflict: state,
        };
        assert_eq!(
            refused.text_in(Lang::Ja),
            format!("m（{}）", hint.text_in(Lang::Ja))
        );
        assert_eq!(
            refused.text_in(Lang::En),
            format!("m ({})", hint.text_in(Lang::En))
        );
    }
    for conflict in [None, Some(DiskState::Deleted), Some(DiskState::Changed)] {
        let hint = Reason::RecoveryHint(conflict);
        let blocked = Reason::UnsavedBlocksOpen(conflict);
        assert_eq!(
            blocked.text_in(Lang::Ja),
            format!(
                "未保存の変更があるため別ファイルを開けない（{}）",
                hint.text_in(Lang::Ja)
            )
        );
        let en = blocked.text_in(Lang::En);
        assert!(
            en.ends_with(&format!("({})", hint.text_in(Lang::En))),
            "{en}"
        );
        assert!(has_japanese(&en).is_none(), "{en}");
    }
}

// ---- #2365 の 2 本目: dispatch が理由の文へそのまま載せる tako-core のエラー型 ----
//
// 編集（TextEdit / RangeEdit / Changes / LineOp）・ターミナル（Session）・Code Runner（Runner）・
// コマンドカード（CommandCard）・器（Backend と InvalidSession の理由）・LSP の答えの読み
// （format / rename の ParseError）。どれも `text_in` の (バリアント, 言語) の網羅 match で、
// 落ちたら日英の対を書いたソースの行（`file:line`）を名指しする

use tako_core::backend::{BackendError, InvalidSessionReason};
use tako_core::command_card::CommandCardError;
use tako_core::lsp::{
    format::ParseError as FormatParseError, rename::ParseError as RenameParseError,
};
use tako_core::preview_limit::Truncation;
use tako_core::runner::RunnerError;
use tako_core::terminal::SessionError;
use tako_core::text_edit::{ChangesError, LineOpError, NotEditable, RangeEditError, TextEditError};

/// 型ごとの `text_in` を 1 つの口で呼ぶ（`InvalidSessionReason::text_in` だけ `&str` を返す）
trait Bilingual: std::fmt::Debug + std::fmt::Display {
    fn text(&self, lang: Lang) -> String;
}

macro_rules! bilingual {
    ($($t:ty),* $(,)?) => {
        $(impl Bilingual for $t {
            fn text(&self, lang: Lang) -> String {
                self.text_in(lang).to_string()
            }
        })*
    };
}

bilingual!(
    TextEditError,
    RangeEditError,
    ChangesError,
    LineOpError,
    SessionError,
    RunnerError,
    CommandCardError,
    BackendError,
    InvalidSessionReason,
    FormatParseError,
    RenameParseError,
);

/// 1 つの断りの見本
struct CoreCase {
    value: Box<dyn Bilingual>,
    /// 日英の対を書いたソース（`include_str!`）とその repo 相対パス
    src: &'static str,
    path: &'static str,
    /// `text_in` を持つ `impl` の行（腕はここから下で探す）
    from: &'static str,
    /// 腕の頭の `Self::` の後ろ（`ZeroLine` / `NotEditable(NotEditable::Binary`）
    arm: &'static str,
    /// #2365 より前の固定文（見本の値で展開したもの）
    old_ja: &'static str,
    /// 作り手が組んだ文をそのまま出す腕（日英が同じ文になる）
    passthrough: bool,
}

const TEXT_EDIT_SRC: &str = include_str!("../../tako-core/src/text_edit.rs");
const TEXT_EDIT_PATH: &str = "crates/tako-core/src/text_edit.rs";
const LINE_OPS_SRC: &str = include_str!("../../tako-core/src/text_edit/line_ops.rs");
const LINE_OPS_PATH: &str = "crates/tako-core/src/text_edit/line_ops.rs";
const TERMINAL_SRC: &str = include_str!("../../tako-core/src/terminal.rs");
const TERMINAL_PATH: &str = "crates/tako-core/src/terminal.rs";
const RUNNER_SRC: &str = include_str!("../../tako-core/src/runner.rs");
const RUNNER_PATH: &str = "crates/tako-core/src/runner.rs";
const CARD_SRC: &str = include_str!("../../tako-core/src/command_card.rs");
const CARD_PATH: &str = "crates/tako-core/src/command_card.rs";
const BACKEND_SRC: &str = include_str!("../../tako-core/src/backend/mod.rs");
const BACKEND_PATH: &str = "crates/tako-core/src/backend/mod.rs";
const LSP_FORMAT_SRC: &str = include_str!("../../tako-core/src/lsp/format.rs");
const LSP_FORMAT_PATH: &str = "crates/tako-core/src/lsp/format.rs";
const LSP_RENAME_SRC: &str = include_str!("../../tako-core/src/lsp/rename.rs");
const LSP_RENAME_PATH: &str = "crates/tako-core/src/lsp/rename.rs";

// 型ごとの通し番号（網羅 match = バリアントを足すとここがコンパイルで落ち、
// [`core_cases`] へ見本と旧文を足すまで通らない）

fn text_edit_index(e: &TextEditError) -> usize {
    match e {
        TextEditError::Read(_) => 0,
        TextEditError::InvalidUtf8 => 1,
        TextEditError::ExternalChanged => 2,
        TextEditError::ExternalDeleted => 3,
        TextEditError::NotEditable(NotEditable::OverLimit(_)) => 4,
        TextEditError::NotEditable(NotEditable::Binary) => 5,
        TextEditError::ReadOnly => 6,
        TextEditError::Write(_) => 7,
    }
}

fn range_edit_index(e: &RangeEditError) -> usize {
    match e {
        RangeEditError::ZeroLine => 0,
        RangeEditError::LineOutOfRange { .. } => 1,
        RangeEditError::ColumnOutOfRange { .. } => 2,
        RangeEditError::NotCharBoundary { .. } => 3,
        RangeEditError::InvertedRange { .. } => 4,
        RangeEditError::VersionMismatch { .. } => 5,
        RangeEditError::Changes(_) => 6,
        RangeEditError::NoPrimaryRange { .. } => 7,
    }
}

fn changes_index(e: &ChangesError) -> usize {
    match e {
        ChangesError::OutOfBounds { .. } => 0,
        ChangesError::Inverted { .. } => 1,
        ChangesError::NotCharBoundary { .. } => 2,
        ChangesError::Overlap { .. } => 3,
        ChangesError::VersionMismatch { .. } => 4,
    }
}

fn line_op_index(e: &LineOpError) -> usize {
    match e {
        LineOpError::NoCommentSyntax { .. } => 0,
    }
}

fn session_index(e: &SessionError) -> usize {
    match e {
        SessionError::Pty(_) => 0,
        SessionError::EventLoop(_) => 1,
    }
}

fn runner_index(e: &RunnerError) -> usize {
    match e {
        RunnerError::NoCommand(_) => 0,
        RunnerError::ProfileNotFound(_) => 1,
        RunnerError::CwdNotFound(_) => 2,
    }
}

fn card_index(e: &CommandCardError) -> usize {
    match e {
        CommandCardError::NoCommands => 0,
        CommandCardError::EmptyCommand { .. } => 1,
        CommandCardError::CommandTooLong { .. } => 2,
        CommandCardError::ControlCharacter { .. } => 3,
        CommandCardError::TooManyCommands { .. } => 4,
        CommandCardError::LabelTooLong { .. } => 5,
        CommandCardError::CardNotFound { .. } => 6,
        CommandCardError::IndexOutOfRange { .. } => 7,
        CommandCardError::StillRunning { .. } => 8,
    }
}

fn backend_index(e: &BackendError) -> usize {
    match e {
        BackendError::Unsupported { .. } => 0,
        BackendError::InvalidSession { .. } => 1,
        BackendError::Operation(_) => 2,
    }
}

fn invalid_session_index(r: &InvalidSessionReason) -> usize {
    match r {
        InvalidSessionReason::Empty => 0,
        InvalidSessionReason::TargetExpression => 1,
        InvalidSessionReason::Whitespace => 2,
        InvalidSessionReason::NotOurs => 3,
    }
}

fn format_parse_index(e: &FormatParseError) -> usize {
    match e {
        FormatParseError::NotArray => 0,
        FormatParseError::Malformed { .. } => 1,
    }
}

fn rename_parse_index(e: &RenameParseError) -> usize {
    match e {
        RenameParseError::NotObject => 0,
        RenameParseError::MalformedChange { .. } => 1,
        RenameParseError::MalformedChanges => 2,
    }
}

fn io(kind: std::io::ErrorKind, message: &str) -> std::io::Error {
    std::io::Error::new(kind, message.to_string())
}

/// 型ごとの見本（値は ASCII。英語側の検査で値の日本語を拾わないため）
fn text_edit_samples() -> Vec<TextEditError> {
    vec![
        TextEditError::Read(io(std::io::ErrorKind::NotFound, "gone")),
        TextEditError::InvalidUtf8,
        TextEditError::ExternalChanged,
        TextEditError::ExternalDeleted,
        TextEditError::NotEditable(NotEditable::OverLimit(Truncation::Lines { lines: 300_000 })),
        TextEditError::NotEditable(NotEditable::Binary),
        TextEditError::ReadOnly,
        TextEditError::Write(io(std::io::ErrorKind::Other, "disk full")),
    ]
}

fn range_edit_samples() -> Vec<RangeEditError> {
    vec![
        RangeEditError::ZeroLine,
        RangeEditError::LineOutOfRange { line: 9, total: 3 },
        RangeEditError::ColumnOutOfRange {
            line: 2,
            column: 8,
            length: 5,
        },
        RangeEditError::NotCharBoundary { line: 1, column: 2 },
        RangeEditError::InvertedRange {
            start_line: 3,
            start_column: 4,
            end_line: 1,
            end_column: 2,
        },
        RangeEditError::VersionMismatch {
            expected: 5,
            actual: 6,
        },
        RangeEditError::Changes(ChangesError::Overlap {
            first: 1,
            second: 2,
        }),
        RangeEditError::NoPrimaryRange {
            primary: 3,
            count: 2,
        },
    ]
}

fn changes_samples() -> Vec<ChangesError> {
    vec![
        ChangesError::OutOfBounds {
            index: 2,
            start: 10,
            end: 20,
            len: 5,
        },
        ChangesError::Inverted {
            index: 1,
            start: 9,
            end: 3,
        },
        ChangesError::NotCharBoundary { index: 4 },
        ChangesError::Overlap {
            first: 1,
            second: 2,
        },
        ChangesError::VersionMismatch {
            expected: 5,
            actual: 6,
        },
    ]
}

fn card_samples() -> Vec<CommandCardError> {
    vec![
        CommandCardError::NoCommands,
        CommandCardError::EmptyCommand { index: 2 },
        CommandCardError::CommandTooLong {
            index: 1,
            len: 5000,
            max: 4096,
        },
        CommandCardError::ControlCharacter { index: 3 },
        CommandCardError::TooManyCommands { len: 11, max: 10 },
        CommandCardError::LabelTooLong { len: 201, max: 200 },
        CommandCardError::CardNotFound { id: 4 },
        CommandCardError::IndexOutOfRange { index: 5, len: 1 },
        CommandCardError::StillRunning { index: 1, pane: 31 },
    ]
}

const INVALID_SESSION_REASONS: [InvalidSessionReason; 4] = [
    InvalidSessionReason::Empty,
    InvalidSessionReason::TargetExpression,
    InvalidSessionReason::Whitespace,
    InvalidSessionReason::NotOurs,
];

/// 全型の見本と、#2365 より前の固定文（`#[error(...)]` / `write!` の書式を見本の値で展開したもの）
fn core_cases() -> Vec<CoreCase> {
    fn case(
        value: impl Bilingual + 'static,
        (src, path, from): (&'static str, &'static str, &'static str),
        arm: &'static str,
        old_ja: &'static str,
    ) -> CoreCase {
        CoreCase {
            value: Box::new(value),
            src,
            path,
            from,
            arm,
            old_ja,
            passthrough: false,
        }
    }
    let text_edit = (TEXT_EDIT_SRC, TEXT_EDIT_PATH, "impl TextEditError {");
    let range = (TEXT_EDIT_SRC, TEXT_EDIT_PATH, "impl RangeEditError {");
    let changes = (TEXT_EDIT_SRC, TEXT_EDIT_PATH, "impl ChangesError {");
    let line_op = (LINE_OPS_SRC, LINE_OPS_PATH, "impl LineOpError {");
    let session = (TERMINAL_SRC, TERMINAL_PATH, "impl SessionError {");
    let runner = (RUNNER_SRC, RUNNER_PATH, "impl RunnerError {");
    let card = (CARD_SRC, CARD_PATH, "impl CommandCardError {");
    let backend = (BACKEND_SRC, BACKEND_PATH, "impl BackendError {");
    let reason = (BACKEND_SRC, BACKEND_PATH, "impl InvalidSessionReason {");
    let format = (LSP_FORMAT_SRC, LSP_FORMAT_PATH, "impl ParseError {");
    let rename = (LSP_RENAME_SRC, LSP_RENAME_PATH, "impl ParseError {");

    let text_edit_arms: [(&str, &str); 8] = [
        ("Read", "ファイルを読み込めない: gone"),
        ("InvalidUtf8", "UTF-8 テキストではないため編集できない"),
        (
            "ExternalChanged",
            "ファイルが外部で変更されたため保存しなかった",
        ),
        ("ExternalDeleted", "ファイルが外部で削除されている"),
        (
            "NotEditable(NotEditable::OverLimit",
            "ディスクの中身が編集できる上限を超えるため読み直せない: 300,000 行（上限 100,000 行）",
        ),
        (
            "NotEditable(NotEditable::Binary",
            "バイナリファイルは編集できない",
        ),
        // #2365 より前は `Write(io::Error::new(PermissionDenied, "読み取り専用ファイル"))`
        ("ReadOnly", "ファイルへ保存できない: 読み取り専用ファイル"),
        ("Write", "ファイルへ保存できない: disk full"),
    ];
    let range_arms: [(&str, &str); 8] = [
        ("ZeroLine", "行は 1 始まり（0 行目は無い）"),
        ("LineOutOfRange", "行 9 は文書の範囲外（全 3 行）"),
        ("ColumnOutOfRange", "行 2 桁 8 は行の長さ 5 を超えている"),
        ("NotCharBoundary", "行 1 桁 2 は文字の途中を指している"),
        ("InvertedRange", "範囲の終わり 1:2 が始まり 3:4 より前"),
        ("VersionMismatch", "文書の版が違う（指定 5 / 現在 6）"),
        ("Changes", "1 番目と 2 番目の編集の範囲が重なっている"),
        (
            "NoPrimaryRange",
            "置き換える範囲が無いか、カーソルを置く範囲の番号 3 が外（2 件）",
        ),
    ];
    let changes_arms: [(&str, &str); 5] = [
        (
            "OutOfBounds",
            "2 番目の編集の範囲 10..20 が本文（5 バイト）の外",
        ),
        (
            "Inverted",
            "1 番目の編集の範囲 9..3 は始まりが終わりより後ろ",
        ),
        (
            "NotCharBoundary",
            "4 番目の編集の範囲が文字の途中を指している",
        ),
        ("Overlap", "1 番目と 2 番目の編集の範囲が重なっている"),
        ("VersionMismatch", "文書の版が違う（指定 5 / 現在 6）"),
    ];
    let card_arms: [(&str, &str); 9] = [
        ("NoCommands", "コマンドが 1 件も指定されていない"),
        ("EmptyCommand", "空のコマンドは提示できない（2 件目）"),
        ("CommandTooLong", "コマンドが長すぎる（1 件目: 5000 バイト > 上限 4096）"),
        (
            "ControlCharacter",
            "コマンドに制御文字が含まれている（3 件目。改行とタブ以外は不可）",
        ),
        (
            "TooManyCommands",
            "1 カードに載せられるコマンドは 10 件まで（指定: 11 件）",
        ),
        ("LabelTooLong", "ラベルが長すぎる（201 文字 > 上限 200）"),
        ("CardNotFound", "カードが見つからない（id=4）"),
        (
            "IndexOutOfRange",
            "コマンド番号が範囲外（指定: 5、このカードは 1〜1）",
        ),
        (
            "StillRunning",
            "このコマンドはまだ実行中（1 件目・ペイン 31）。終わるか、そのペインを閉じてからもう一度実行する",
        ),
    ];
    let reason_arms: [(&str, &str); 4] = [
        ("Empty", "セッション名が空"),
        (
            "TargetExpression",
            "tmux のターゲット式（session:window.pane）はセッション名として使えない",
        ),
        ("Whitespace", "セッション名に空白・制御文字は使えない"),
        ("NotOurs", "tako が作った器ではない（接頭辞が一致しない）"),
    ];

    let mut out = Vec::new();
    for e in text_edit_samples() {
        let (arm, old) = text_edit_arms[text_edit_index(&e)];
        out.push(case(e, text_edit, arm, old));
    }
    for e in range_edit_samples() {
        let (arm, old) = range_arms[range_edit_index(&e)];
        out.push(case(e, range, arm, old));
    }
    for e in changes_samples() {
        let (arm, old) = changes_arms[changes_index(&e)];
        out.push(case(e, changes, arm, old));
    }
    let e = LineOpError::NoCommentSyntax {
        name: "notes.txt".into(),
    };
    let _ = line_op_index(&e);
    out.push(case(
        e,
        line_op,
        "NoCommentSyntax",
        "notes.txt のコメント記号を知らない（コメントの記号を持たない形式）",
    ));
    for e in [
        SessionError::Pty(io(std::io::ErrorKind::Other, "openpty")),
        SessionError::EventLoop(io(std::io::ErrorKind::Other, "spawn")),
    ] {
        let (arm, old) = [
            ("Pty", "PTY の生成に失敗した"),
            ("EventLoop", "PTY IO スレッドの起動に失敗した"),
        ][session_index(&e)];
        out.push(case(e, session, arm, old));
    }
    for e in [
        RunnerError::NoCommand("hint".into()),
        RunnerError::ProfileNotFound("build".into()),
        RunnerError::CwdNotFound("/tmp/x".into()),
    ] {
        let (arm, old) = [
            ("NoCommand", "実行コマンドが見つからない: hint"),
            (
                "ProfileNotFound",
                "指定されたプロファイル 'build' はファイル内に宣言されていない",
            ),
            ("CwdNotFound", "cwd が存在しない: /tmp/x"),
        ][runner_index(&e)];
        out.push(case(e, runner, arm, old));
    }
    for e in card_samples() {
        let (arm, old) = card_arms[card_index(&e)];
        out.push(case(e, card, arm, old));
    }
    for e in [
        BackendError::Unsupported {
            note: "no detach".into(),
        },
        BackendError::InvalidSession {
            name: "tako-abc:0.0".into(),
            reason: InvalidSessionReason::TargetExpression,
        },
        BackendError::Operation("tmux: no server".into()),
    ] {
        let i = backend_index(&e);
        let (arm, old) = [
            ("Unsupported", "no detach"),
            (
                "InvalidSession",
                "セッション名として不正（tako-abc:0.0）: tmux のターゲット式（session:window.pane）はセッション名として使えない",
            ),
            ("Operation", "tmux: no server"),
        ][i];
        let mut c = case(e, backend, arm, old);
        c.passthrough = i != 1;
        out.push(c);
    }
    for r in INVALID_SESSION_REASONS {
        let (arm, old) = reason_arms[invalid_session_index(&r)];
        out.push(case(r, reason, arm, old));
    }
    for e in [
        FormatParseError::NotArray,
        FormatParseError::Malformed { index: 2 },
    ] {
        let (arm, old) = [
            ("NotArray", "TextEdit の配列ではない"),
            ("Malformed", "2 番目の TextEdit の形が違う"),
        ][format_parse_index(&e)];
        out.push(case(e, format, arm, old));
    }
    for e in [
        RenameParseError::NotObject,
        RenameParseError::MalformedChange { index: 3 },
        RenameParseError::MalformedChanges,
    ] {
        let (arm, old) = [
            ("NotObject", "WorkspaceEdit ではない"),
            ("MalformedChange", "documentChanges の 3 番目の形が違う"),
            ("MalformedChanges", "changes の値が TextEdit の配列ではない"),
        ][rename_parse_index(&e)];
        out.push(case(e, rename, arm, old));
    }
    out
}

/// 落ちたときに添える「日英の対を書いた行」
fn core_where(c: &CoreCase, lang: Lang) -> String {
    match arm_line(c.src, c.from, c.arm, lang) {
        Some(line) => format!("{}:{line}", c.path),
        None => format!("{}（{} の腕が見つからない）", c.path, c.arm),
    }
}

#[test]
fn core_のエラー型の見本は全バリアントを1つずつ持つ() {
    covers_all(&text_edit_samples(), text_edit_index, 8);
    covers_all(&range_edit_samples(), range_edit_index, 8);
    covers_all(&changes_samples(), changes_index, 5);
    covers_all(&card_samples(), card_index, 9);
    covers_all(&INVALID_SESSION_REASONS, invalid_session_index, 4);
    // 1 つずつ並べた型（LineOp / Session / Runner / Backend / ParseError 2 種）は
    // 見本の数を網羅 match の通し番号の数とそろえる
    let cases = core_cases();
    let count = |from: &str, path: &str| {
        cases
            .iter()
            .filter(|c| c.from == from && c.path == path)
            .count()
    };
    assert_eq!(count("impl LineOpError {", LINE_OPS_PATH), 1);
    assert_eq!(count("impl SessionError {", TERMINAL_PATH), 2);
    assert_eq!(count("impl RunnerError {", RUNNER_PATH), 3);
    assert_eq!(count("impl BackendError {", BACKEND_PATH), 3);
    assert_eq!(count("impl ParseError {", LSP_FORMAT_PATH), 2);
    assert_eq!(count("impl ParseError {", LSP_RENAME_PATH), 3);
}

#[test]
fn core_のエラー型が日英を持ち英語に日本語が混ざらない() {
    for c in core_cases() {
        let debug = format!("{:?}", c.value);
        let (ja, en) = (c.value.text(Lang::Ja), c.value.text(Lang::En));
        let at = |lang| core_where(&c, lang);
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
        if let Some(ch) = has_japanese(&en) {
            panic!(
                "{debug} の英語に日本語（{ch}）が混ざる: {en}（{}）",
                at(Lang::En)
            );
        }
        if c.passthrough {
            assert_eq!(
                ja,
                en,
                "{debug} は作り手の文をそのまま出す（{}）",
                at(Lang::En)
            );
        } else {
            assert_ne!(ja, en, "{debug} の英語が日本語と同じ文（{}）", at(Lang::En));
        }
    }
}

#[test]
fn core_のエラー型の日本語は2365より前の固定文とバイト一致() {
    for c in core_cases() {
        assert_eq!(
            c.value.text(Lang::Ja),
            c.old_ja,
            "{:?}（{}）",
            c.value,
            core_where(&c, Lang::Ja)
        );
    }
}

/// 番犬自身の検査: 全見本の腕の行を日英とも引ける（引けないと名指しが空振りする）
#[test]
fn core_のエラー型の腕の行を日英とも引ける() {
    for c in core_cases() {
        for lang in [Lang::Ja, Lang::En] {
            assert!(
                arm_line(c.src, c.from, c.arm, lang).is_some(),
                "{:?} / {lang:?}: {} の {} 以降に Self::{} の腕が無い",
                c.value,
                c.path,
                c.from,
                c.arm
            );
        }
    }
}

/// 「見つからない」の読み（リモートの 404 寄せ）と「まだ実行中」の読み（リモートの 409 寄せ）は
/// 日英で同じ当たり方をする
#[test]
fn core_のエラー型の見つからないと実行中の読みは日英で同じ() {
    for c in core_cases() {
        let (ja, en) = (c.value.text(Lang::Ja), c.value.text(Lang::En));
        assert_eq!(
            E::says_not_found(&ja),
            E::says_not_found(&en),
            "{:?}: {ja} / {en}（{}）",
            c.value,
            core_where(&c, Lang::En)
        );
        assert_eq!(
            CommandCardError::says_still_running(&ja),
            CommandCardError::says_still_running(&en),
            "{:?}: {ja} / {en}（{}）",
            c.value,
            core_where(&c, Lang::En)
        );
    }
    // 当たるのは見つからない系・実行中だけ（前置きつきで載せても変わらない）
    for lang in [Lang::Ja, Lang::En] {
        let gone = CommandCardError::CardNotFound { id: 4 }.text_in(lang);
        assert!(E::says_not_found(&E::InvalidParams(gone).text_in(lang)));
        let running = CommandCardError::StillRunning { index: 1, pane: 31 }.text_in(lang);
        assert!(CommandCardError::says_still_running(
            &E::Operation(running).text_in(lang)
        ));
        let range = CommandCardError::IndexOutOfRange { index: 5, len: 1 }.text_in(lang);
        assert!(!E::says_not_found(&range) && !CommandCardError::says_still_running(&range));
    }
}

/// 境界: 入れ子（RangeEdit の中の Changes）は中の文をそのまま出し、前置きつきの断りへ
/// 載せても前置きは 1 回だけ。io エラーの原因は `source` で辿れる（表示には足さない）
#[test]
fn core_のエラー型の境界() {
    use std::error::Error as _;
    for lang in [Lang::Ja, Lang::En] {
        let inner = ChangesError::Inverted {
            index: 1,
            start: 9,
            end: 3,
        };
        let outer = RangeEditError::from(inner.clone());
        assert_eq!(outer.text_in(lang), inner.text_in(lang));
        let prefix = E::InvalidParams(String::new()).text_in(lang);
        let shown = E::InvalidParams(outer.text_in(lang)).text_in(lang);
        assert_eq!(shown.matches(prefix.as_str()).count(), 1, "{shown}");
        // 空の名前（InvalidSession の Empty）でも括弧の中が空になるだけで文は崩れない
        let empty = BackendError::InvalidSession {
            name: String::new(),
            reason: InvalidSessionReason::Empty,
        };
        assert!(empty
            .text_in(lang)
            .ends_with(InvalidSessionReason::Empty.text_in(lang)));
    }
    let pty = SessionError::Pty(io(std::io::ErrorKind::Other, "openpty"));
    assert_eq!(
        pty.source().map(|s| s.to_string()).as_deref(),
        Some("openpty")
    );
    let read = TextEditError::Read(io(std::io::ErrorKind::NotFound, "gone"));
    assert!(read.source().is_some());
    assert!(TextEditError::ReadOnly.source().is_none());
}

// ---- #2365 の続き: `tako mod ui` の断り（UiError 一式） ----
//
// 理由（UiReason）・使える値（Allowed）・枠（UiError の「ui.json は書いていない」）・形の問題
// （UiProblem）・書き込みの断り（UpdateError / SaveRefusal）を 1 本で移した（半分だけ移すと
// 1 つの断りの中で日英が混ざる）。どれも `text_in` の (バリアント, 言語) の網羅 match で、
// 落ちたら日英の対を書いたソースの行（`file:line`）を名指しする

use tako_core::claude_mod_ui::{
    Allowed, ButtonField, SaveRefusal, UiError, UiProblem, UiReason, UpdateError,
};

bilingual!(UiReason, UiProblem, Allowed, UpdateError, SaveRefusal);

const MOD_UI_SRC: &str = include_str!("../../tako-core/src/claude_mod_ui.rs");
const MOD_UI_PATH: &str = "crates/tako-core/src/claude_mod_ui.rs";

/// 1 つの断りの見本（ソースは [`MOD_UI_SRC`] で固定）
struct UiCase {
    value: Box<dyn Bilingual>,
    /// `text_in` を持つ `impl` の行（腕はここから下で探す）
    from: &'static str,
    /// 腕の頭の `Self::` の後ろ
    arm: &'static str,
    /// #2365 より前の固定文（見本の値で展開したもの）。単独では出なかった形（「外した」を
    /// 添えてだけ出ていた問題）は `None`（添えた形は [`ui_dropped_old_ja`] で突き合わせる）
    old_ja: Option<&'static str>,
    /// 日英で同じ語をそのまま出す（語彙の語・ボタンの id）
    passthrough: bool,
}

fn ui_reason_index(r: &UiReason) -> usize {
    match r {
        UiReason::IdShape => 0,
        UiReason::LabelEmpty => 1,
        UiReason::LabelControl => 2,
        UiReason::LabelTooWide { .. } => 3,
        UiReason::ShellTooLong { .. } => 4,
        UiReason::ShellControl => 5,
        UiReason::ShellEmpty => 6,
        UiReason::PromptEmpty => 7,
        UiReason::PromptTooLong => 8,
        UiReason::PromptControl => 9,
        UiReason::HotkeyShape => 10,
        UiReason::NotString { .. } => 11,
        UiReason::BadValue { .. } => 12,
        UiReason::NotBool { .. } => 13,
        UiReason::BadListValue { .. } => 14,
        UiReason::ListDuplicate { .. } => 15,
        UiReason::EmptyList { .. } => 16,
        UiReason::MoveToUnreadable => 17,
        UiReason::MoveToRange { .. } => 18,
        UiReason::NeedValue { .. } => 19,
        UiReason::NeedContent { .. } => 20,
        UiReason::NeedId { .. } => 21,
        UiReason::NeedTo { .. } => 22,
        UiReason::SetNeedsKey => 23,
        UiReason::SetKeyUnknown => 24,
        UiReason::PresetNeedsName => 25,
        UiReason::PresetUnknown => 26,
        UiReason::OpUnknown => 27,
        UiReason::AnswersPresetUnknown => 28,
        UiReason::AnswersSetKeyUnknown => 29,
        UiReason::NoSuchButton => 30,
        UiReason::SameAction { .. } => 31,
        UiReason::TooManyButtons => 32,
        UiReason::KindUnknown => 33,
        UiReason::KindNeeded => 34,
        UiReason::SlashNotAllowed => 35,
        UiReason::TakoOpUnknown => 36,
        UiReason::IdTaken => 37,
        UiReason::HotkeyTaken => 38,
        UiReason::Unshaped { .. } => 39,
    }
}
const UI_REASON_VARIANTS: usize = 40;

fn ui_problem_index(p: &UiProblem) -> usize {
    match p {
        UiProblem::SchemaVersion => 0,
        UiProblem::SegmentsEmpty => 1,
        UiProblem::SegmentsDuplicate => 2,
        UiProblem::ItemsEmpty => 3,
        UiProblem::ItemsDuplicate => 4,
        UiProblem::TooManyButtons { .. } => 5,
        UiProblem::DuplicateId { .. } => 6,
        UiProblem::DuplicateHotkey { .. } => 7,
        UiProblem::Field { .. } => 8,
        UiProblem::NotNormalized { .. } => 9,
        UiProblem::NotJson { .. } => 10,
        UiProblem::NotObject => 11,
        UiProblem::UnknownKey { .. } => 12,
        UiProblem::Missing { .. } => 13,
        UiProblem::SectionUnshaped { .. } => 14,
        UiProblem::ButtonsNotArray => 15,
        UiProblem::ButtonUnshaped { .. } => 16,
        UiProblem::SameIdOrHotkey { .. } => 17,
        UiProblem::OverMax { .. } => 18,
        UiProblem::Dropped(_) => 19,
        UiProblem::NotUtf8 => 20,
        UiProblem::ReadFailed { .. } => 21,
    }
}
const UI_PROBLEM_VARIANTS: usize = 22;

fn allowed_index(a: &Allowed) -> usize {
    match a {
        Allowed::Word(_) => 0,
        Allowed::Letters => 1,
        Allowed::Digits => 2,
        Allowed::Position => 3,
    }
}

fn update_error_index(e: &UpdateError) -> usize {
    match e {
        UpdateError::Invalid(_) => 0,
        UpdateError::Future { .. } => 1,
        UpdateError::Io(_) => 2,
    }
}

fn save_refusal_index(e: &SaveRefusal) -> usize {
    match e {
        SaveRefusal::Unshaped(_) => 0,
        SaveRefusal::NoDir => 1,
    }
}

/// 理由の見本と #2365 より前の固定文（値は ASCII。英語側の検査で値の日本語を拾わないため）
fn ui_reason_table() -> Vec<(UiReason, &'static str, &'static str)> {
    use UiReason as R;
    vec![
        (R::IdShape, "IdShape", "id は英小文字・数字・`-` の 24 文字まで（先頭は英数字）"),
        (R::LabelEmpty, "LabelEmpty", "label は空にできない（前後の空白も不可）"),
        (R::LabelControl, "LabelControl", "label に改行・制御文字は使えない"),
        (
            R::LabelTooWide { columns: 18 },
            "LabelTooWide",
            "label は 16 桁まで（全角は 2 桁。指定は 18 桁）",
        ),
        (
            R::ShellTooLong { max: 4096 },
            "ShellTooLong",
            "shell のコマンドは 4096 バイトまで",
        ),
        (
            R::ShellControl,
            "ShellControl",
            "shell のコマンドに制御文字は使えない（改行とタブだけ可）",
        ),
        (R::ShellEmpty, "ShellEmpty", "shell のコマンドは空にできない"),
        (R::PromptEmpty, "PromptEmpty", "prompt の文は空にできない"),
        (R::PromptTooLong, "PromptTooLong", "prompt の文は 1000 文字まで"),
        (
            R::PromptControl,
            "PromptControl",
            "prompt の文に制御文字は使えない（改行とタブだけ可）",
        ),
        (R::HotkeyShape, "HotkeyShape", "hotkey は数字 1 字か英小文字 1 字"),
        (
            R::NotString {
                key: "band.segments",
            },
            "NotString",
            "band.segments の値は文字列で渡す",
        ),
        (
            R::BadValue { key: "colors.dim" },
            "BadValue",
            "colors.dim に使えない値",
        ),
        (R::NotBool { key: "band.hidden" }, "NotBool", "band.hidden は真偽"),
        (
            R::BadListValue {
                key: "usage_bar.items",
            },
            "BadListValue",
            "usage_bar.items に使えない値が入っている",
        ),
        (
            R::ListDuplicate {
                key: "band.segments",
            },
            "ListDuplicate",
            "band.segments に同じ値が 2 回ある（並べたい順に 1 回ずつ）",
        ),
        (
            R::EmptyList {
                key: "usage_bar.items",
            },
            "EmptyList",
            "usage_bar.items は 1 つ以上並べる",
        ),
        (R::MoveToUnreadable, "MoveToUnreadable", "移す先の指定が読めない"),
        (R::MoveToRange { len: 4 }, "MoveToRange", "移す先は 1〜4"),
        (
            R::NeedValue { op: "set".into() },
            "NeedValue",
            "set には 値（value） が要る",
        ),
        (
            R::NeedContent {
                op: "button_add".into(),
            },
            "NeedContent",
            "button_add には 中身（value） が要る",
        ),
        (
            R::NeedId {
                op: "button_remove".into(),
            },
            "NeedId",
            "button_remove には ボタンの id（id） が要る",
        ),
        (
            R::NeedTo {
                op: "button_move".into(),
            },
            "NeedTo",
            "button_move には 移す先（to） が要る",
        ),
        (R::SetNeedsKey, "SetNeedsKey", "set にはキー（key）が要る"),
        (R::SetKeyUnknown, "SetKeyUnknown", "set のキーが語彙に無い"),
        (
            R::PresetNeedsName,
            "PresetNeedsName",
            "preset には雛形の名前（value）が要る",
        ),
        (R::PresetUnknown, "PresetUnknown", "雛形の名前が語彙に無い"),
        (R::OpUnknown, "OpUnknown", "op が語彙に無い"),
        (
            R::AnswersPresetUnknown,
            "AnswersPresetUnknown",
            "mod_ui.preset が語彙に無い",
        ),
        (
            R::AnswersSetKeyUnknown,
            "AnswersSetKeyUnknown",
            "mod_ui.set のキーが語彙に無い",
        ),
        (R::NoSuchButton, "NoSuchButton", "その id のボタンは無い"),
        (
            R::SameAction {
                id: "compact".into(),
            },
            "SameAction",
            "同じ動作のボタン（id compact）が既にある。ラベル等を変えるなら button_remove してから足す",
        ),
        (
            R::TooManyButtons,
            "TooManyButtons",
            "ボタンは 8 個まで（どれかを button_remove で外す）",
        ),
        (R::KindUnknown, "KindUnknown", "ボタンの kind が語彙に無い"),
        (
            R::KindNeeded,
            "KindNeeded",
            "ボタンの kind が要る（値が slash / tako の語でないので推せない）",
        ),
        (
            R::SlashNotAllowed,
            "SlashNotAllowed",
            "slash で送れるのは許可リストのコマンドだけ",
        ),
        (R::TakoOpUnknown, "TakoOpUnknown", "tako の操作 id が語彙に無い"),
        (R::IdTaken, "IdTaken", "その id は別のボタンが使っている"),
        (R::HotkeyTaken, "HotkeyTaken", "その hotkey は別のボタンが使っている"),
        (
            R::Unshaped {
                problems: vec![UiProblem::SegmentsEmpty, UiProblem::DuplicateId { index: 2 }],
            },
            "Unshaped",
            "操作の結果が形を満たさない（band.segments が空（隠すなら band.hidden を true にする）; buttons[2].id が前のボタンと重複）",
        ),
    ]
}

/// 形の問題の見本と #2365 より前の固定文
fn ui_problem_table() -> Vec<(UiProblem, &'static str, Option<&'static str>)> {
    use UiProblem as P;
    vec![
        (
            P::SchemaVersion,
            "SchemaVersion",
            Some("schema_version は 1"),
        ),
        (
            P::SegmentsEmpty,
            "SegmentsEmpty",
            Some("band.segments が空（隠すなら band.hidden を true にする）"),
        ),
        (
            P::SegmentsDuplicate,
            "SegmentsDuplicate",
            Some("band.segments に同じ区切りが 2 回ある"),
        ),
        (
            P::ItemsEmpty,
            "ItemsEmpty",
            Some("usage_bar.items が空（消すなら usage_bar.place を off にする）"),
        ),
        (
            P::ItemsDuplicate,
            "ItemsDuplicate",
            Some("usage_bar.items に同じ項目が 2 回ある"),
        ),
        (
            P::TooManyButtons { len: 9 },
            "TooManyButtons",
            Some("buttons が 9 個（8 個まで）"),
        ),
        (
            P::DuplicateId { index: 2 },
            "DuplicateId",
            Some("buttons[2].id が前のボタンと重複"),
        ),
        (
            P::DuplicateHotkey { index: 3 },
            "DuplicateHotkey",
            Some("buttons[3].hotkey が前のボタンと重複"),
        ),
        (
            P::Field {
                index: 1,
                field: ButtonField::Label,
                why: UiReason::LabelControl,
            },
            "Field",
            Some("buttons[1].label: label に改行・制御文字は使えない"),
        ),
        (
            P::NotNormalized {
                index: 0,
                field: ButtonField::Command,
            },
            "NotNormalized",
            Some("buttons[0].action.command: 前後の空白・改行を落とした形で書く"),
        ),
        (
            P::NotJson { line: 3, column: 7 },
            "NotJson",
            Some("JSON として読めない（3 行 7 桁）"),
        ),
        (
            P::NotObject,
            "NotObject",
            Some("最上位が JSON のオブジェクトではない"),
        ),
        (
            P::UnknownKey {
                key: "extra".into(),
            },
            "UnknownKey",
            Some("知らないキー `extra`"),
        ),
        (
            P::Missing { key: "buttons" },
            "Missing",
            Some("buttons が無い"),
        ),
        (
            P::SectionUnshaped { key: "chat" },
            "SectionUnshaped",
            Some("chat が語彙・形に合わない"),
        ),
        (
            P::ButtonsNotArray,
            "ButtonsNotArray",
            Some("buttons が配列ではない"),
        ),
        (P::ButtonUnshaped { index: 4 }, "ButtonUnshaped", None),
        (P::SameIdOrHotkey { index: 5 }, "SameIdOrHotkey", None),
        (P::OverMax { index: 8 }, "OverMax", None),
        (
            P::Dropped(Box::new(P::ButtonUnshaped { index: 4 })),
            "Dropped",
            Some("buttons[4] が語彙・形に合わない（外した）"),
        ),
        (P::NotUtf8, "NotUtf8", Some("UTF-8 として読めない")),
        (
            P::ReadFailed {
                kind: "permission denied".into(),
            },
            "ReadFailed",
            Some("読み取りに失敗（permission denied）"),
        ),
    ]
}

/// 「外した」を添えてだけ出ていた問題の、#2365 より前の固定文（`lenient_buttons` が組んでいた形）
fn ui_dropped_old_ja() -> Vec<(UiProblem, &'static str)> {
    use UiProblem as P;
    vec![
        (
            P::ButtonUnshaped { index: 4 },
            "buttons[4] が語彙・形に合わない（外した）",
        ),
        (
            P::SameIdOrHotkey { index: 5 },
            "buttons[5] の id か hotkey が前のボタンと重複（外した）",
        ),
        (
            P::OverMax { index: 8 },
            "buttons[8] は 8 個を超えた（外した）",
        ),
        (
            P::Field {
                index: 0,
                field: ButtonField::Hotkey,
                why: UiReason::HotkeyShape,
            },
            "buttons[0].hotkey: hotkey は数字 1 字か英小文字 1 字（外した）",
        ),
        (
            P::NotNormalized {
                index: 2,
                field: ButtonField::Text,
            },
            "buttons[2].action.text: 前後の空白・改行を落とした形で書く（外した）",
        ),
    ]
}

fn ui_allowed_table() -> Vec<(Allowed, &'static str, &'static str)> {
    vec![
        (Allowed::Word("compact".into()), "Word", "compact"),
        (Allowed::Letters, "Letters", "a〜z"),
        (Allowed::Digits, "Digits", "0〜9"),
        (Allowed::Position, "Position", "1〜8 の番号"),
    ]
}

fn ui_update_table() -> Vec<(UpdateError, &'static str, &'static str)> {
    vec![
        (
            UpdateError::Invalid(UiError {
                what: UiReason::OpUnknown,
                allowed: Vec::new(),
            }),
            "Invalid",
            "op が語彙に無い（ui.json は書いていない）",
        ),
        (
            UpdateError::Future { version: 2 },
            "Future",
            "ui.json は新しい tako が書いた形（schema_version 2）なので、この tako では書き換えない（tako を更新する）",
        ),
        (
            UpdateError::Io("disk full".into()),
            "Io",
            "ui.json を書けない: disk full",
        ),
    ]
}

fn ui_save_table() -> Vec<(SaveRefusal, &'static str, &'static str)> {
    vec![
        (
            SaveRefusal::Unshaped(vec![UiProblem::ItemsEmpty, UiProblem::SchemaVersion]),
            "Unshaped",
            "形を満たさない値は書かない（usage_bar.items が空（消すなら usage_bar.place を off にする）; schema_version は 1）",
        ),
        (SaveRefusal::NoDir, "NoDir", "ui.json の置き場が決まらない"),
    ]
}

/// 全型の見本（理由・問題・使える値・書き込みの断り）
fn ui_cases() -> Vec<UiCase> {
    fn case(
        value: impl Bilingual + 'static,
        from: &'static str,
        arm: &'static str,
        old_ja: Option<&'static str>,
    ) -> UiCase {
        UiCase {
            value: Box::new(value),
            from,
            arm,
            old_ja,
            passthrough: false,
        }
    }
    let mut out = Vec::new();
    for (r, arm, old) in ui_reason_table() {
        out.push(case(r, "impl UiReason {", arm, Some(old)));
    }
    for (p, arm, old) in ui_problem_table() {
        out.push(case(p, "impl UiProblem {", arm, old));
    }
    for (a, arm, old) in ui_allowed_table() {
        let word = matches!(a, Allowed::Word(_));
        let mut c = case(a, "impl Allowed {", arm, Some(old));
        c.passthrough = word;
        out.push(c);
    }
    for (e, arm, old) in ui_update_table() {
        out.push(case(e, "impl UpdateError {", arm, Some(old)));
    }
    for (e, arm, old) in ui_save_table() {
        out.push(case(e, "impl SaveRefusal {", arm, Some(old)));
    }
    out
}

/// 落ちたときに添える「日英の対を書いた行」
fn ui_where(from: &str, arm: &str, lang: Lang) -> String {
    match arm_line(MOD_UI_SRC, from, arm, lang) {
        Some(line) => format!("{MOD_UI_PATH}:{line}"),
        None => format!("{MOD_UI_PATH}（{arm} の腕が見つからない）"),
    }
}

/// 枠（`UiError::text_in` の `(使える値が空か, 言語)` の腕）の行
fn ui_frame_where(with_allowed: bool, lang: Lang) -> String {
    let token = match lang {
        Lang::Ja => "Ja",
        Lang::En => "En",
    };
    let head = format!("({}, {token})", !with_allowed);
    let lines: Vec<&str> = MOD_UI_SRC.lines().collect();
    let found = lines
        .iter()
        .position(|l| l.contains("impl UiError {"))
        .and_then(|start| {
            (start..lines.len()).find(|&i| lines[i].trim_start().starts_with(head.as_str()))
        });
    match found {
        Some(i) => format!("{MOD_UI_PATH}:{}", i + 1),
        None => format!("{MOD_UI_PATH}（枠の {head} の腕が見つからない）"),
    }
}

#[test]
fn mod_ui_の断りの見本は全バリアントを1つずつ持つ() {
    let reasons: Vec<UiReason> = ui_reason_table().into_iter().map(|(r, ..)| r).collect();
    covers_all(&reasons, ui_reason_index, UI_REASON_VARIANTS);
    let problems: Vec<UiProblem> = ui_problem_table().into_iter().map(|(p, ..)| p).collect();
    covers_all(&problems, ui_problem_index, UI_PROBLEM_VARIANTS);
    let allowed: Vec<Allowed> = ui_allowed_table().into_iter().map(|(a, ..)| a).collect();
    covers_all(&allowed, allowed_index, 4);
    let update: Vec<UpdateError> = ui_update_table().into_iter().map(|(e, ..)| e).collect();
    covers_all(&update, update_error_index, 3);
    let save: Vec<SaveRefusal> = ui_save_table().into_iter().map(|(e, ..)| e).collect();
    covers_all(&save, save_refusal_index, 2);
}

#[test]
fn mod_ui_の断りが日英を持ち英語に日本語が混ざらない() {
    for c in ui_cases() {
        let debug = format!("{:?}", c.value);
        let (ja, en) = (c.value.text(Lang::Ja), c.value.text(Lang::En));
        let at = |lang| ui_where(c.from, c.arm, lang);
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
        if let Some(ch) = has_japanese(&en) {
            panic!(
                "{debug} の英語に日本語（{ch}）が混ざる: {en}（{}）",
                at(Lang::En)
            );
        }
        if c.passthrough {
            assert_eq!(ja, en, "{debug} は語をそのまま出す（{}）", at(Lang::En));
        } else {
            assert_ne!(ja, en, "{debug} の英語が日本語と同じ文（{}）", at(Lang::En));
        }
    }
}

#[test]
fn mod_ui_の断りの日本語は2365より前の固定文とバイト一致() {
    for c in ui_cases() {
        if let Some(old) = c.old_ja {
            assert_eq!(
                c.value.text(Lang::Ja),
                old,
                "{:?}（{}）",
                c.value,
                ui_where(c.from, c.arm, Lang::Ja)
            );
        }
    }
    for (p, old) in ui_dropped_old_ja() {
        let dropped = UiProblem::Dropped(Box::new(p.clone()));
        assert_eq!(
            dropped.text_in(Lang::Ja),
            old,
            "{p:?}（{}）",
            ui_where("impl UiProblem {", "Dropped", Lang::Ja)
        );
        assert!(has_japanese(&dropped.text_in(Lang::En)).is_none(), "{p:?}");
    }
}

/// 番犬自身の検査: 全見本の腕の行と枠の行を日英とも引ける（引けないと名指しが空振りする）
#[test]
fn mod_ui_の断りの腕の行を日英とも引ける() {
    for c in ui_cases() {
        for lang in [Lang::Ja, Lang::En] {
            assert!(
                arm_line(MOD_UI_SRC, c.from, c.arm, lang).is_some(),
                "{:?} / {lang:?}: {MOD_UI_PATH} の {} 以降に Self::{} の腕が無い",
                c.value,
                c.from,
                c.arm
            );
        }
    }
    for with_allowed in [false, true] {
        for lang in [Lang::Ja, Lang::En] {
            let at = ui_frame_where(with_allowed, lang);
            assert!(!at.contains("見つからない"), "{at}");
        }
    }
}

/// 枠（「ui.json は書いていない」「使える値:」）と使える値の範囲（`a〜z` / `1〜8 の番号`）も
/// 理由と同じ言語でそろう。日本語は #2365 より前の `message()` とバイト一致
#[test]
fn mod_ui_の断りの枠と使える値は理由と同じ言語でそろう() {
    let plain = UiError {
        what: UiReason::LabelTooWide { columns: 18 },
        allowed: Vec::new(),
    };
    let hotkey = UiError {
        what: UiReason::HotkeyShape,
        allowed: vec![Allowed::Letters, Allowed::Digits],
    };
    let move_to = UiError {
        what: UiReason::MoveToUnreadable,
        allowed: vec![
            Allowed::Position,
            Allowed::Word("first".into()),
            Allowed::Word("last".into()),
            Allowed::Word("left".into()),
            Allowed::Word("right".into()),
        ],
    };
    for (e, old) in [
        (
            &plain,
            "label は 16 桁まで（全角は 2 桁。指定は 18 桁）（ui.json は書いていない）",
        ),
        (
            &hotkey,
            "hotkey は数字 1 字か英小文字 1 字。使える値: a〜z / 0〜9（ui.json は書いていない）",
        ),
        (
            &move_to,
            "移す先の指定が読めない。使える値: 1〜8 の番号 / first / last / left / right（ui.json は書いていない）",
        ),
    ] {
        let with_allowed = !e.allowed.is_empty();
        assert_eq!(
            e.text_in(Lang::Ja),
            old,
            "{e:?}（{}）",
            ui_frame_where(with_allowed, Lang::Ja)
        );
        let en = e.text_in(Lang::En);
        if let Some(ch) = has_japanese(&en) {
            panic!(
                "{e:?} の英語に日本語（{ch}）が混ざる: {en}（枠 {} / 理由 {}）",
                ui_frame_where(with_allowed, Lang::En),
                ui_where(
                    "impl UiReason {",
                    variant_name(&format!("{:?}", e.what)),
                    Lang::En
                )
            );
        }
        assert!(en.ends_with("(ui.json was not written)"), "{en}");
        assert_eq!(en.contains("Allowed values: "), with_allowed, "{en}");
    }
    assert_eq!(
        hotkey.text_in(Lang::En),
        "hotkey is one digit or one lowercase letter. Allowed values: a-z / 0-9 (ui.json was not written)"
    );
}

/// 一時の ui.json（テストの data dir = #944 の隔離先の中）
fn ui_scratch(name: &str) -> std::path::PathBuf {
    let dir = tako_core::paths::data_dir()
        .expect("テストの data dir")
        .join(format!(
            "issue2365-ui-{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(tako_core::claude_mod_ui::FILENAME)
}

/// 実経路: CLI `tako mod ui` / MCP `tako_mod` の action=ui が通る `claude_mod_ui::run` の断りは、
/// 1 つの応答の中で日英が混ざらない（英語表示に日本語が 1 字も無い・日本語表示は #2365 より前と
/// バイト一致）。MCP は前置き（`DispatchError` の表示）つきで返るので、それも同じ言語でそろう
#[test]
fn mod_ui_の実経路の断りは1つの応答に日英が混ざらない() {
    use serde_json::json;
    use tako_core::claude_mod_ui::{SetKey, ThemeColor, UiRequest, Vocab};
    let path = ui_scratch("lang");
    let req = |v: serde_json::Value| -> UiRequest { serde_json::from_value(v).unwrap() };
    // 9 個目を断らせるために 8 個まで足しておく（既定の compact + 7 個）
    for v in [
        "clear",
        "context",
        "cost",
        "model",
        "effort",
        "tako",
        "split-right",
    ] {
        tako_control::claude_mod_ui::run(&path, &req(json!({"op": "button_add", "value": v})))
            .unwrap();
    }
    let before = std::fs::read(&path).unwrap();
    let ids = "compact / clear / context / cost / model / effort / tako / split-right";
    let colors = ThemeColor::words().join(" / ");
    let keys = SetKey::words().join(" / ");
    let cases: Vec<(serde_json::Value, String)> = vec![
        (
            json!({"op": "button_add", "kind": "javascript", "value": "alert(1)"}),
            "ボタンの kind が語彙に無い。使える値: slash / tako / shell / prompt（ui.json は書いていない）".into(),
        ),
        (
            json!({"op": "button_remove", "id": "nope"}),
            format!("その id のボタンは無い。使える値: {ids}（ui.json は書いていない）"),
        ),
        (
            json!({"op": "button_move", "id": "compact", "to": "middle"}),
            "移す先の指定が読めない。使える値: 1〜8 の番号 / first / last / left / right（ui.json は書いていない）".into(),
        ),
        (
            json!({"op": "button_add", "value": "split-down"}),
            format!("ボタンは 8 個まで（どれかを button_remove で外す）。使える値: {ids}（ui.json は書いていない）"),
        ),
        (
            json!({"op": "set", "key": "colors.dim", "value": "#ff0000"}),
            format!("colors.dim に使えない値。使える値: {colors}（ui.json は書いていない）"),
        ),
        (
            json!({"op": "set", "key": "band.color", "value": "red"}),
            format!("set のキーが語彙に無い。使える値: {keys}（ui.json は書いていない）"),
        ),
        (
            json!({"op": "button_remove"}),
            "button_remove には ボタンの id（id） が要る（ui.json は書いていない）".into(),
        ),
        (
            json!({"op": "set", "key": "band.segments", "value": "pane,pane"}),
            "band.segments に同じ値が 2 回ある（並べたい順に 1 回ずつ）（ui.json は書いていない）"
                .into(),
        ),
    ];
    for (args, old_ja) in &cases {
        let err = tako_control::claude_mod_ui::run(&path, &req(args.clone())).unwrap_err();
        let at = |lang| match &err {
            tako_control::claude_mod_ui::RunError::Invalid(e) => ui_where(
                "impl UiReason {",
                variant_name(&format!("{:?}", e.what)),
                lang,
            ),
            other => format!("{other:?}"),
        };
        assert_eq!(&err.text_in(Lang::Ja), old_ja, "{args}（{}）", at(Lang::Ja));
        for lang in [Lang::Ja, Lang::En] {
            // MCP の応答（前置き + 理由の文）
            let shown = E::InvalidParams(err.text_in(lang)).text_in(lang);
            if lang == Lang::En {
                if let Some(ch) = has_japanese(&shown) {
                    panic!(
                        "{args} の英語の応答に日本語（{ch}）が混ざる: {shown}（{}）",
                        at(Lang::En)
                    );
                }
            } else {
                assert!(shown.ends_with(old_ja.as_str()), "{shown}");
            }
        }
    }
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "断った操作は書かない"
    );
    // 新しい tako が書いた形（書き換えの断り）も同じ
    let mut future: serde_json::Value = serde_json::from_slice(&before).unwrap();
    future["schema_version"] = json!(2);
    std::fs::write(&path, future.to_string()).unwrap();
    let err = tako_control::claude_mod_ui::run(
        &path,
        &req(json!({"op": "set", "key": "usage_bar.place", "value": "off"})),
    )
    .unwrap_err();
    assert_eq!(
        err.text_in(Lang::Ja),
        "ui.json は新しい tako が書いた形（schema_version 2）なので、この tako では書き換えない（tako を更新する）"
    );
    assert!(
        has_japanese(&err.text_in(Lang::En)).is_none(),
        "{}（{}）",
        err.text_in(Lang::En),
        ui_where("impl UpdateError {", "Future", Lang::En)
    );
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
