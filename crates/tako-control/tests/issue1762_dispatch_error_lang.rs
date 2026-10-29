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
//! #2365 の続きで、ファイルの移動・コピーの断り（tako-core の `MoveRefusal` / `CopyRefusal` と、
//! それを包む dispatch の「移動できない」「コピーできない」「N 件すべてできなかった」「貼り付けできない
//! （N 件すべて）」）を理由ごと広げた（理由は tako-core のエラー型の節・包みは `Reason` の表と
//! 「包みと理由の全組み合わせ」の節）。
//! #2365 の続きで、action で操作を選ぶツールの断り（不明な action・不明な値・`<操作> には <鍵> が必要`）と、
//! それらのツールが共有する設定の保存・置き場の断りを `Reason` の表へ足した。呼び手が候補を値で渡すので、
//! 旧形の日本語の固定文の書き足しと、渡す候補・案内の日本語は dispatch のソースを読んで名指す（末尾）。
//! #2365 の続きで、MCP の前段（`mcp/request.rs` の引数の組み立てと JSON-RPC の口）と CLI の前段
//! （`tako-cli` の `build_request` とそれが呼ぶ補助）の断りを `Reason` の表へ足した。前段に日本語の
//! 固定文を書き足すと英語表示でもその 1 か所だけ日本語で返るので、前段のソースを読んで名指す（最後）。
//!
//! このファイルは**独立したテストバイナリ**で、言語グローバルを触るのは
//! `display_follows_the_display_language` の 1 本だけ（ほかは `text_in` で言語を引数に渡す）。

use tako_control::DispatchError as E;
use tako_control::{NoTargetPaneAt, Reason};
use tako_core::i18n::{self, Lang};
use tako_core::load_issue::{YamlProblem, YamlReason};
use tako_core::user_task::TaskRefusal;
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
        // #2365 の続き: tako mod ui の枠の文・引数の読み方の断り・排他の断り・語彙の説明・
        // 選べる値の一覧も同じグローバルを引く（CLI の表示と MCP の応答はこの表示）
        for (t, _, _) in ui_text_table() {
            assert_eq!(t.to_string(), t.text_in(lang), "{t:?} / {lang:?}");
        }
        for e in CLI_REQUEST_ERRORS {
            assert_eq!(e.to_string(), e.text_in(lang), "{e:?} / {lang:?}");
        }
        for (e, _) in lock_table() {
            assert_eq!(e.to_string(), e.text_in(lang), "{e:?} / {lang:?}");
        }
        assert_eq!(
            tako_core::claude_mod_ui::choices(),
            tako_core::claude_mod_ui::choices_in(lang)
        );
        assert_eq!(
            Preset::Minimal.describe(),
            Preset::Minimal.describe_in(lang)
        );
        // #2365 の続き: tako mod（status / on / off / band / install）の枠・理由の文・置き場の断りも
        // 同じグローバルを引く（MCP の応答と CLI の表示はこの表示）
        for (t, _) in mod_text_table() {
            assert_eq!(t.to_string(), t.text_in(lang), "{t:?} / {lang:?}");
        }
        for (r, _) in off_reason_table() {
            assert_eq!(r.describe(), r.describe_in(lang), "{r:?} / {lang:?}");
        }
        for (c, _) in no_report_table() {
            assert_eq!(c.describe(), c.describe_in(lang), "{c:?} / {lang:?}");
        }
        for (e, _) in install_error_table() {
            assert_eq!(e.to_string(), e.text_in(lang), "{e:?} / {lang:?}");
        }
        for (c, _, _) in conflict_table() {
            assert_eq!(c.describe(), c.describe_in(lang), "{c:?} / {lang:?}");
        }
        for (b, _) in blocked_table() {
            assert_eq!(b.describe(), b.describe_in(lang), "{b:?} / {lang:?}");
        }
        assert_eq!(
            tako_core::paths::NoDataDir.to_string(),
            tako_core::paths::NoDataDir.text_in(lang)
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
        Reason::MoveRefused { .. } => 85,
        Reason::CopyRefused { .. } => 86,
        Reason::AllFailed(_) => 87,
        Reason::PasteAllFailed(_) => 88,
        Reason::NothingToPaste => 89,
        Reason::AlreadyInFolder => 90,
        Reason::UnknownValue { .. } => 91,
        Reason::UnknownActionPlain { .. } => 92,
        Reason::UnknownActionListed { .. } => 93,
        Reason::UnknownLidPowerCondition(_) => 94,
        Reason::ActionNeeds { .. } => 95,
        Reason::ActionNeedsHint { .. } => 96,
        Reason::ThemeSetNeedsMode => 97,
        Reason::ShortcutAddNeedsPath => 98,
        Reason::ShortcutRemoveNeedsPath => 99,
        Reason::SaveSettingsFailed(_) => 100,
        Reason::DataDirUnresolved => 101,
        Reason::UnknownColorKey(_) => 102,
        Reason::ReservedPresetName(_) => 103,
        Reason::PresetNameShape => 104,
        Reason::UnknownTheme { .. } => 105,
        Reason::UnfamiliarAction { .. } => 106,
        // #2365 の続き: MCP の前段
        Reason::ClickShape => 107,
        Reason::ClickCell(_) => 108,
        Reason::AxisXOrY => 109,
        Reason::ReattachWithEnabled => 110,
        Reason::TextNeedsType => 111,
        Reason::InvalidValueBare { .. } => 112,
        Reason::PathOrPaths => 113,
        Reason::InvalidPanelView(_) => 114,
        Reason::CannotParse(..) => 115,
        Reason::AdoptNameMissing => 116,
        Reason::MissingHint { .. } => 117,
        Reason::SetNotObject => 118,
        Reason::ActionMustGive { .. } => 119,
        Reason::ModMcpAction => 120,
        Reason::ModUiArgs(_) => 121,
        Reason::RuleNotBool(_) => 122,
        Reason::RulesShape => 123,
        Reason::PasteGuardOnly(_) => 124,
        Reason::OneOfListed { .. } => 125,
        Reason::JsonEncodeFailed { .. } => 126,
        Reason::FormatRangeIncomplete => 127,
        Reason::UnknownMethod(_) => 128,
        Reason::RequestNotJson => 129,
        Reason::NotJson(_) => 130,
        // #2365 の続き: CLI の前段
        Reason::ResizeOneOf => 131,
        Reason::PasteGuardTextUsage => 132,
        Reason::PasteGuardUnallowId => 133,
        Reason::StdinReadFailed(_) => 134,
        Reason::BadMovement(_) => 135,
        Reason::BadMotion(_) => 136,
        Reason::BadLineOp(_) => 137,
        Reason::ModDryRunOnly => 138,
        Reason::ModBandOnOff => 139,
        Reason::ModOnOffAfterBand => 140,
        Reason::ReattachNeedsPane => 141,
        Reason::ForegroundTarget => 142,
        Reason::FileMoveArgs => 143,
        Reason::FileCopyArgs => 144,
        Reason::CopyOneAtATime => 145,
        Reason::CopyPartlyFailed { .. } => 146,
        Reason::CwdUnreadable(_) => 147,
        Reason::EnvShape(_) => 148,
        Reason::ModReportUnreadable(_) => 149,
        Reason::ModReportNotJson(_) => 150,
        Reason::PositionLine(_) => 151,
        Reason::PositionColumn(_) => 152,
        Reason::RangeShape(_) => 153,
        Reason::ManyPartlyFailed { .. } => 154,
        // #2365 の続き: CLI / MCP のブリッジが GUI へ繋ぐ口（`tako-cli` の ipc.rs）と起動失敗の包み
        Reason::OutsideTako => 155,
        Reason::CannotConnect { .. } => 156,
        Reason::ConnectionCloneFailed(_) => 157,
        Reason::ReplyThreadEnded => 158,
        Reason::RequestEncodeFailed(_) => 159,
        Reason::SendFailed(_) => 160,
        Reason::ReceiveFailed(_) => 161,
        Reason::NoReply => 162,
        Reason::ReplyUnreadable(_) => 163,
        Reason::ReplyTimeout(_) => 164,
        Reason::LaunchPaneUnavailable { .. } => 165,
        // #2365 の続き: dispatch が呼ぶ置き場（acceptance_gates / task_checkpoints / user_tasks）
        Reason::StoreUnreadable { .. } => 166,
        Reason::StoreParseFailed { .. } => 167,
        Reason::StoreDirFailed { .. } => 168,
        Reason::YamlEncodeFailed(_) => 169,
        Reason::JsonSerializeFailed(_) => 170,
        Reason::JsonArgUnreadable(..) => 171,
        Reason::ItemNeedsId(_) => 172,
        Reason::CriterionKindUnreadable(..) => 173,
        Reason::UnknownBare { .. } => 174,
        Reason::UnknownPlain { .. } => 175,
        Reason::CriteriaEmpty => 176,
        Reason::GateNotFound(_) => 177,
        Reason::TaskGoneAfterRespond => 178,
    }
}
const REASON_VARIANTS: usize = 179;

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
        Reason::MoveRefused {
            refusal: MoveRefusal::NameTaken,
            src: "/tmp/a.txt".into(),
            dest: "/tmp/dst".into(),
        },
        Reason::CopyRefused {
            refusal: CopyRefusal::IntoDescendant,
            src: "/tmp/d".into(),
            dest: "/tmp/d/sub".into(),
        },
        Reason::AllFailed(vec!["reason one".into(), "reason two".into()]),
        Reason::PasteAllFailed(vec!["reason one".into(), "reason two".into()]),
        Reason::NothingToPaste,
        Reason::AlreadyInFolder,
        Reason::UnknownValue {
            key: "action",
            got: "bogus".into(),
            allowed: "status / set".into(),
        },
        Reason::UnknownActionPlain {
            got: "bogus".into(),
            allowed: "list / revoke / role",
        },
        Reason::UnknownActionListed {
            got: "bogus".into(),
            allowed: "status | init | link | unlink | push | pull | list",
        },
        Reason::UnknownLidPowerCondition("bogus".into()),
        Reason::ActionNeeds {
            action: "show".into(),
            key: "task_id",
        },
        Reason::ActionNeedsHint {
            action: "set",
            key: "value",
            hint: "system / ja / en",
        },
        Reason::ThemeSetNeedsMode,
        Reason::ShortcutAddNeedsPath,
        Reason::ShortcutRemoveNeedsPath,
        Reason::SaveSettingsFailed("Permission denied (os error 13)".into()),
        Reason::DataDirUnresolved,
        Reason::UnknownColorKey("nosuch".into()),
        Reason::ReservedPresetName("dark".into()),
        Reason::PresetNameShape,
        Reason::UnknownTheme {
            got: "nope".into(),
            presets: vec!["mine".into(), "work".into()],
        },
        Reason::UnfamiliarAction {
            got: "bogus".into(),
            allowed: "check / fix",
        },
        // #2365 の続き: MCP の前段
        Reason::ClickShape,
        Reason::ClickCell("row".into()),
        Reason::AxisXOrY,
        Reason::ReattachWithEnabled,
        Reason::TextNeedsType,
        Reason::InvalidValueBare {
            key: "op",
            got: "bogus".into(),
        },
        Reason::PathOrPaths,
        Reason::InvalidPanelView("bogus".into()),
        Reason::CannotParse(
            "limits",
            "invalid type: string \"x\", expected struct QueueLimits".into(),
        ),
        Reason::AdoptNameMissing,
        Reason::MissingHint {
            key: "action",
            hint: "check / run",
        },
        Reason::SetNotObject,
        Reason::ActionMustGive {
            action: "rename",
            key: "new_name",
        },
        Reason::ModMcpAction,
        Reason::ModUiArgs("unknown field `x`".into()),
        Reason::RuleNotBool("high_entropy".into()),
        Reason::RulesShape,
        Reason::PasteGuardOnly("enabled".into()),
        Reason::OneOfListed {
            key: "target",
            allowed: "new-pane | split | tab".into(),
        },
        Reason::JsonEncodeFailed {
            key: Some("criteria"),
            error: "boom".into(),
        },
        Reason::FormatRangeIncomplete,
        Reason::UnknownMethod("resources/list".into()),
        Reason::RequestNotJson,
        Reason::NotJson("expected value at line 1 column 1".into()),
        // #2365 の続き: CLI の前段
        Reason::ResizeOneOf,
        Reason::PasteGuardTextUsage,
        Reason::PasteGuardUnallowId,
        Reason::StdinReadFailed("stream did not contain valid UTF-8".into()),
        Reason::BadMovement("diagonal".into()),
        Reason::BadMotion("sideways".into()),
        Reason::BadLineOp("shuffle".into()),
        Reason::ModDryRunOnly,
        Reason::ModBandOnOff,
        Reason::ModOnOffAfterBand,
        Reason::ReattachNeedsPane,
        Reason::ForegroundTarget,
        Reason::FileMoveArgs,
        Reason::FileCopyArgs,
        Reason::CopyOneAtATime,
        Reason::CopyPartlyFailed {
            total: 3,
            failed: 2,
        },
        Reason::CwdUnreadable("No such file or directory (os error 2)".into()),
        Reason::EnvShape("NOEQ".into()),
        Reason::ModReportUnreadable("stream did not contain valid UTF-8".into()),
        Reason::ModReportNotJson("expected value at line 1 column 1".into()),
        Reason::PositionLine("x:0".into()),
        Reason::PositionColumn("1:y".into()),
        Reason::RangeShape("1:0".into()),
        Reason::ManyPartlyFailed {
            total: 4,
            failed: 1,
        },
        // #2365 の続き: GUI へ繋ぐ口と起動失敗の包み
        Reason::OutsideTako,
        Reason::CannotConnect {
            socket: "/tmp/tk/none.sock".into(),
            error: "No such file or directory (os error 2)".into(),
        },
        Reason::ConnectionCloneFailed("Bad file descriptor (os error 9)".into()),
        Reason::ReplyThreadEnded,
        Reason::RequestEncodeFailed("key must be a string".into()),
        Reason::SendFailed("Broken pipe (os error 32)".into()),
        Reason::ReceiveFailed("Connection reset by peer (os error 54)".into()),
        Reason::NoReply,
        Reason::ReplyUnreadable("expected value at line 1 column 1".into()),
        Reason::ReplyTimeout(30),
        Reason::LaunchPaneUnavailable {
            cause: "cannot connect".into(),
            cmd_hint: "tako master -fable".into(),
            stale_pane: Some(305),
        },
        // #2365 の続き: 置き場
        Reason::StoreUnreadable {
            file: "user-tasks.yaml",
            error: "Permission denied (os error 13)".into(),
        },
        Reason::StoreParseFailed {
            file: "task_checkpoints.yaml",
            reason: YamlReason::Located {
                kind: YamlProblem::Syntax,
                at: Some((3, 5)),
            },
        },
        Reason::StoreDirFailed {
            file: "user-tasks.yaml",
            error: "Permission denied (os error 13)".into(),
        },
        Reason::YamlEncodeFailed("boom".into()),
        Reason::JsonSerializeFailed("boom".into()),
        Reason::JsonArgUnreadable("criteria", "expected value at line 1 column 1".into()),
        Reason::ItemNeedsId("criterion"),
        Reason::CriterionKindUnreadable("tests".into(), "missing field `type`".into()),
        Reason::UnknownBare {
            key: "phase",
            got: "flying".into(),
        },
        Reason::UnknownPlain {
            key: "kind",
            got: "chore".into(),
            allowed: "approval / review / post / other".into(),
        },
        Reason::CriteriaEmpty,
        Reason::GateNotFound("task-9".into()),
        Reason::TaskGoneAfterRespond,
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
    "移動できない: 移動先に同じ名前がある（上書きしない）（/tmp/a.txt → /tmp/dst）",
    "コピーできない: フォルダを自分の配下へはコピーできない（/tmp/d → /tmp/d/sub）",
    "2 件すべてできなかった: reason one / reason two",
    "貼り付けできない（2 件すべて）: reason one / reason two",
    "貼り付けるファイルがクリップボードに無い",
    "既にそのフォルダにある（何もしていない）",
    "不明な action: \"bogus\"（status / set のいずれか）",
    "不明な action: bogus（list / revoke / role）",
    "不明な action: bogus。status | init | link | unlink | push | pull | list",
    "不明な lid_power_condition: \"bogus\"（ac-only = AC 電源に接続しているときだけ / \
     always = バッテリー駆動でも）",
    "show には task_id が必要",
    "set には value が必要（system / ja / en）",
    "set には mode が必要（dark / light / プリセット名）",
    "add には path が必要（絶対パス）",
    "remove には path が必要（id でも可）",
    "設定の保存に失敗: Permission denied (os error 13)",
    "データディレクトリを解決できない",
    "未知の色キー: nosuch",
    "dark は予約名",
    "名前は [a-z0-9-]{1,32}",
    "未知のテーマ: \"nope\"（target は dark / light / プリセット名。プリセット: mine / work）",
    "未知の action: bogus（check / fix）",
    // #2365 の続き: MCP の前段（`mcp/request.rs` / `mcp/mod.rs` / `mcp/http.rs` と stdio ブリッジ）
    "click は {\"row\": 行, \"col\": 桁}（0 始まりの表示セル）で指定する",
    "click.row を 0 以上の整数で指定する",
    "axis は \"x\" か \"y\" を指定する",
    "enabled と reattach は併用できない（切替か再 attach のどちらか）",
    "text は command=type と組で渡す",
    "op が不正: bogus",
    "path（または paths）を指定する",
    "view が不正: bogus（fleet | orch | git | tasks | diagnostics | references | search | usage。\
     tmux は fleet の旧称）",
    "limits を解釈できない: invalid type: string \"x\", expected struct QueueLimits",
    "name を指定する（プロファイル名 / プロジェクトキー）",
    "action を指定する（check / run）",
    "set はオブジェクト（キー → 値）",
    "rename は new_name を指定する",
    "action は status / on / off / band-on / band-off / ui / install / uninstall のどれか\
     （report は MCP に無い）",
    "action=ui の引数が読めない: unknown field `x`",
    "rules.high_entropy は true / false で指定する",
    "rules は {\"規則名\": true / false} で指定する",
    "enabled は action=paste_guard のときだけ使える",
    "target は new-pane | split | tab のいずれか",
    "criteria の JSON 変換に失敗: boom",
    "範囲の整形は line / column / end_line / end_column の 4 つをそろえて指定する（省略で文書全体）",
    "メソッド resources/list は未対応",
    "リクエストを JSON として解釈できない",
    "JSON として解釈できない: expected value at line 1 column 1",
    // #2365 の続き: CLI の前段（`tako-cli` の `build_request` とそれが呼ぶ補助）
    "--dx / --dy / --share-x / --share-y のどれか 1 つを指定する",
    "文字を渡せるのは `tako paste-guard check|allow|unallow|rule` だけ",
    "外す許可の ID（`tako paste-guard` の allow[].id）か all を渡す",
    "標準入力の読み取りに失敗: stream did not contain valid UTF-8",
    "移動の単位は left / right / up / down / word-left / word-right / line-start / smart-home / \
     line-end / doc-start / doc-end / page-up / page-down のどれか（\"diagonal\" は無い）",
    "削除の単位は char-backward / char-forward / word-backward / word-forward / to-line-start / \
     to-line-end のどれか（\"sideways\" は無い）",
    "行の操作は duplicate-up / duplicate-down / move-up / move-down / indent / outdent / comment / \
     uncomment / toggle-comment のどれか（\"shuffle\" は無い）",
    "--dry-run は install / uninstall にだけ付ける（tako mod install --dry-run）",
    "band には on か off を付ける（tako mod band off で帯を隠す）",
    "on / off は band の後ろにだけ付ける（帯の切替は tako mod band on|off。画面の UI 設定は tako mod ui）",
    "reattach には --pane N（再接続するペインの ID）が要る",
    "ペイン ID か --tab のどちらかを指定する（tako backgrounded で確認）",
    "移すものと移動先のフォルダを指定する（例: tako file move a.txt dst）",
    "コピー元と貼り付け先のフォルダを指定する（例: tako file copy a.txt dst）",
    "コピー元が複数のときは 1 件ずつ送る（file_copy_cli）",
    "3 件中 2 件をコピーできなかった",
    "作業ディレクトリを読めない: No such file or directory (os error 2)",
    "--env は K=V の形で指定する（指定: \"NOEQ\"）",
    "報告を stdin から読めない: stream did not contain valid UTF-8",
    "報告の JSON を読めない: expected value at line 1 column 1",
    "位置は 行:桁 で指定する（行が数値でない: x:0）",
    "位置は 行:桁 で指定する（桁が数値でない: 1:y）",
    "範囲は 行:桁-行:桁 で指定する（`-` が無い: 1:0）",
    "4 件中 1 件をできなかった",
    // #2365 の続き: `tako-cli` の ipc.rs（接続まわり）・`protocol::reply_timeout_message`・
    // main.rs の `launch_failure_message`
    "tako アプリへの接続情報が無い（TAKO_SOCKET / TAKO_TOKEN 未設定・接続情報ファイルも無し）。\
     tako アプリを起動するか、tako 内のターミナルで実行してください",
    "tako アプリへ接続できない（/tmp/tk/none.sock: No such file or directory (os error 2)）",
    "接続の複製に失敗: Bad file descriptor (os error 9)",
    "応答を待つスレッドが途中で終わった",
    "送信の構築に失敗: key must be a string",
    "送信に失敗: Broken pipe (os error 32)",
    "応答の受信に失敗: Connection reset by peer (os error 54)",
    "tako アプリから応答が返らなかった",
    "応答を解釈できない: expected value at line 1 column 1",
    "tako アプリが 30 秒応答しない（接続は受けたが返事が無い = GUI が固まっている可能性。\
     要求は届いているので、GUI が戻ると後から処理されることがある。\
     待つ上限は env TAKO_IPC_TIMEOUT_SECS（秒・0 で上限なし））",
    "起動先ペインを確保できない: cannot connect\n\
     復旧: tako アプリを起動し、その中のターミナルで `tako master -fable` を実行してください\n      \
     このシェルの TAKO_PANE_ID=305 は古い可能性があります: `unset TAKO_PANE_ID` してから再実行してください",
    // #2365 の続き: acceptance_gates.rs / task_checkpoints.rs / user_tasks.rs
    "user-tasks.yaml の読み取りに失敗: Permission denied (os error 13)",
    "task_checkpoints.yaml のパースに失敗: YAML の構文が壊れている（3 行 5 列）",
    "user-tasks.yaml の置き場を作れない: Permission denied (os error 13)",
    "YAML のシリアライズに失敗: boom",
    "JSON シリアライズに失敗: boom",
    "criteria の JSON パースに失敗: expected value at line 1 column 1",
    "criterion に id が必要",
    "criterion kind のパースに失敗 (id=tests): missing field `type`",
    "不明な phase: flying",
    "不明な kind: chore（approval / review / post / other のいずれか）",
    "criteria を 1 つ以上指定する",
    "ゲートが見つからない: task-9",
    "直前に更新したタスクが無い",
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
            | Reason::GateNotFound(_)
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
use tako_core::file_copy::CopyRefusal;
use tako_core::file_move::MoveRefusal;
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
    MoveRefusal,
    CopyRefusal,
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
const FILE_MOVE_SRC: &str = include_str!("../../tako-core/src/file_move.rs");
const FILE_MOVE_PATH: &str = "crates/tako-core/src/file_move.rs";
const FILE_COPY_SRC: &str = include_str!("../../tako-core/src/file_copy.rs");
const FILE_COPY_PATH: &str = "crates/tako-core/src/file_copy.rs";

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

fn move_refusal_index(r: &MoveRefusal) -> usize {
    match r {
        MoveRefusal::SourceMissing => 0,
        MoveRefusal::DestNotDir => 1,
        MoveRefusal::NoName => 2,
        MoveRefusal::IntoSelf => 3,
        MoveRefusal::IntoDescendant => 4,
        MoveRefusal::NameTaken => 5,
        MoveRefusal::CrossDevice => 6,
        MoveRefusal::Remote => 7,
        MoveRefusal::WorkspaceRoot => 8,
        MoveRefusal::Io(_) => 9,
    }
}
const MOVE_REFUSALS: usize = 10;

fn copy_refusal_index(r: &CopyRefusal) -> usize {
    match r {
        CopyRefusal::SourceMissing => 0,
        CopyRefusal::DestNotDir => 1,
        CopyRefusal::NoName => 2,
        CopyRefusal::IntoSelf => 3,
        CopyRefusal::IntoDescendant => 4,
        CopyRefusal::NameTaken => 5,
        CopyRefusal::NamesExhausted => 6,
        CopyRefusal::Unreadable(_) => 7,
        CopyRefusal::Unwritable(_) => 8,
        CopyRefusal::Special(_) => 9,
        CopyRefusal::SymlinkPrivilege(_) => 10,
        CopyRefusal::NoSpace => 11,
        CopyRefusal::Io(_) => 12,
        CopyRefusal::Vanished(_) => 13,
        CopyRefusal::NotRun => 14,
        CopyRefusal::Cancelled => 15,
    }
}
const COPY_REFUSALS: usize = 16;

/// 移動の断りの見本と #2365 より前の固定文（`MoveRefusal::reason` の日本語。値は ASCII）
fn move_refusal_table() -> Vec<(MoveRefusal, &'static str)> {
    vec![
        (MoveRefusal::SourceMissing, "移す元が見つからない"),
        (MoveRefusal::DestNotDir, "移動先がフォルダではない"),
        (MoveRefusal::NoName, "移す元に名前が無い"),
        (MoveRefusal::IntoSelf, "フォルダを自分自身へは移せない"),
        (
            MoveRefusal::IntoDescendant,
            "フォルダを自分の配下へは移せない",
        ),
        (
            MoveRefusal::NameTaken,
            "移動先に同じ名前がある（上書きしない）",
        ),
        (
            MoveRefusal::CrossDevice,
            "別のボリュームへは移せない（コピーしてから元を消す移動は未対応）",
        ),
        (MoveRefusal::Remote, "リモート（SSH）の項目は移せない"),
        (
            MoveRefusal::WorkspaceRoot,
            "ワークスペースのフォルダ（見出し）は移せない",
        ),
        (
            MoveRefusal::Io("Permission denied (os error 13)".into()),
            "OS が断った: Permission denied (os error 13)",
        ),
    ]
}

/// コピーの断りの見本と #2365 より前の固定文（`CopyRefusal::reason` の日本語。値は ASCII）。
/// `Vanished` / `NotRun` は #2365 より前は `Io` の中身に日本語の文を入れていたもの
/// （`read_error` の NotFound・dispatch の `finish_offload`）で、表示は `Io` の形のまま
fn copy_refusal_table() -> Vec<(CopyRefusal, &'static str)> {
    use std::path::PathBuf;
    vec![
        (CopyRefusal::SourceMissing, "コピー元が見つからない"),
        (CopyRefusal::DestNotDir, "貼り付け先がフォルダではない"),
        (CopyRefusal::NoName, "コピー元に名前が無い"),
        (
            CopyRefusal::IntoSelf,
            "フォルダを自分自身の中へはコピーできない",
        ),
        (
            CopyRefusal::IntoDescendant,
            "フォルダを自分の配下へはコピーできない",
        ),
        (
            CopyRefusal::NameTaken,
            "貼り付け先に同じ名前ができた（上書きしない）",
        ),
        (
            CopyRefusal::NamesExhausted,
            "空いている名前が見つからない（上書きしない）",
        ),
        (
            CopyRefusal::Unreadable(PathBuf::from("/tmp/secret")),
            "読めない項目がある: /tmp/secret",
        ),
        (
            CopyRefusal::Unwritable(PathBuf::from("/tmp/locked")),
            "書き込めない: /tmp/locked",
        ),
        (
            CopyRefusal::Special(PathBuf::from("/tmp/fifo")),
            "特殊なファイル（FIFO・ソケット・デバイス）はコピーできない: /tmp/fifo",
        ),
        (
            CopyRefusal::SymlinkPrivilege(PathBuf::from("/tmp/link")),
            "シンボリックリンクを作る権限が無い（Windows は開発者モードか管理者が要る）: /tmp/link",
        ),
        (CopyRefusal::NoSpace, "空き容量が足りない"),
        (
            CopyRefusal::Io("Permission denied (os error 13)".into()),
            "OS が断った: Permission denied (os error 13)",
        ),
        (
            CopyRefusal::Vanished(PathBuf::from("/tmp/gone")),
            "OS が断った: コピーの途中で見つからなくなった: /tmp/gone",
        ),
        (CopyRefusal::NotRun, "OS が断った: コピーが走らなかった"),
        (CopyRefusal::Cancelled, "取り消した（作りかけは消した）"),
    ]
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
    // #2365 の続き: 移動・コピーの断りの理由（dispatch の「移動できない」「コピーできない」が包む）
    let moves = (FILE_MOVE_SRC, FILE_MOVE_PATH, "impl MoveRefusal {");
    let copies = (FILE_COPY_SRC, FILE_COPY_PATH, "impl CopyRefusal {");
    for (r, old) in move_refusal_table() {
        let arm = variant_name(&format!("{r:?}")).to_string().leak();
        out.push(case(r, moves, arm, old));
    }
    for (r, old) in copy_refusal_table() {
        let arm = variant_name(&format!("{r:?}")).to_string().leak();
        out.push(case(r, copies, arm, old));
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
    let moves: Vec<MoveRefusal> = move_refusal_table().into_iter().map(|(r, _)| r).collect();
    covers_all(&moves, move_refusal_index, MOVE_REFUSALS);
    let copies: Vec<CopyRefusal> = copy_refusal_table().into_iter().map(|(r, _)| r).collect();
    covers_all(&copies, copy_refusal_index, COPY_REFUSALS);
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

// ---- #2365 の続き: 移動・コピーの断りの包み（dispatch）と理由（tako-core）の全組み合わせ ----
//
// 包みだけ移すと「英語の包み + 日本語の理由」に混ざる（#2365 の 3 本目の worker の判断）ので、
// 理由ごと 1 本で移した。理由そのものは上の tako-core のエラー型の節が見る。ここでは包み
// （`Reason::MoveRefused` / `CopyRefused` と、それを並べる `AllFailed` / `PasteAllFailed`）へ理由の
// 全バリアントを入れ、1 つの断りの中で言語がそろうこと・日本語が #2365 より前の組み立てと
// バイト一致することを見る。落ちたら包みの腕と理由の腕の両方の行（`file:line`）を名指しする

/// 包みと理由の 1 組（表示用の名前・日英の文・#2365 より前の日本語・名指し）
struct Wrapped {
    debug: String,
    ja: String,
    en: String,
    old_ja: String,
    at: Box<dyn Fn(Lang) -> String>,
}

/// 理由の全バリアントを包んだもの（移動 = `run_file_move` / コピー = `copy_refused_text` の形）
fn wrapped_refusals() -> Vec<Wrapped> {
    let (src, dest) = ("/tmp/src", "/tmp/dest");
    let mut out = Vec::new();
    for (refusal, old) in move_refusal_table() {
        let wrap = Reason::MoveRefused {
            refusal: refusal.clone(),
            src: src.into(),
            dest: dest.into(),
        };
        let arm = variant_name(&format!("{refusal:?}")).to_string();
        out.push(Wrapped {
            debug: format!("{wrap:?}"),
            ja: wrap.text_in(Lang::Ja),
            en: wrap.text_in(Lang::En),
            // #2365 より前の `run_file_move` の `format!("移動できない: {}（{} → {}）", …)`
            old_ja: format!("移動できない: {old}（{src} → {dest}）"),
            at: Box::new(move |lang| {
                format!(
                    "包み {} / 理由 {}",
                    reason_where(&wrap, lang),
                    where_arm(
                        FILE_MOVE_SRC,
                        FILE_MOVE_PATH,
                        "impl MoveRefusal {",
                        &arm,
                        lang
                    )
                )
            }),
        });
    }
    for (refusal, old) in copy_refusal_table() {
        let wrap = Reason::CopyRefused {
            refusal: refusal.clone(),
            src: src.into(),
            dest: dest.into(),
        };
        let arm = variant_name(&format!("{refusal:?}")).to_string();
        out.push(Wrapped {
            debug: format!("{wrap:?}"),
            ja: wrap.text_in(Lang::Ja),
            en: wrap.text_in(Lang::En),
            // #2365 より前の `copy_refused_text` の `format!("コピーできない: {}（{} → {}）", …)`
            old_ja: format!("コピーできない: {old}（{src} → {dest}）"),
            at: Box::new(move |lang| {
                format!(
                    "包み {} / 理由 {}",
                    reason_where(&wrap, lang),
                    where_arm(
                        FILE_COPY_SRC,
                        FILE_COPY_PATH,
                        "impl CopyRefusal {",
                        &arm,
                        lang
                    )
                )
            }),
        });
    }
    out
}

#[test]
fn 移動とコピーの包みと理由の全組み合わせで1つの断りに日英が混ざらない() {
    let wrapped = wrapped_refusals();
    assert_eq!(wrapped.len(), MOVE_REFUSALS + COPY_REFUSALS);
    for w in &wrapped {
        assert_eq!(w.ja, w.old_ja, "{}（{}）", w.debug, (w.at)(Lang::Ja));
        if let Some(c) = has_japanese(&w.en) {
            panic!(
                "{} の英語の断りに日本語（{c}）が混ざる: {}（{}）",
                w.debug,
                w.en,
                (w.at)(Lang::En)
            );
        }
        assert_ne!(w.ja, w.en, "{} の英語が日本語と同じ文", w.debug);
        // 「見つからない」の読み（リモートの 404 寄せ）は包んでも日英で同じ
        assert_eq!(
            E::says_not_found(&w.ja),
            E::says_not_found(&w.en),
            "{}: {} / {}（{}）",
            w.debug,
            w.ja,
            w.en,
            (w.at)(Lang::En)
        );
        // dispatch はこの文を前置きの無い断り（`Operation`）で返す = CLI / MCP の応答そのもの
        for (lang, text) in [(Lang::Ja, &w.ja), (Lang::En, &w.en)] {
            assert_eq!(&E::Operation(text.clone()).text_in(lang), text);
        }
    }
    // 全部断られたときの並べ方（`many_reply` = N 件すべてできなかった / `paste_reply` =
    // 貼り付けできない（N 件すべて））も、並べた 1 件ずつの断りと同じ言語でそろう
    // 1 件ずつの断りの文を並べる包み（`AllFailed` / `PasteAllFailed`）
    type Gather = fn(Vec<String>) -> Reason;
    let alls: [(Gather, &str); 2] = [
        (Reason::AllFailed, "2 件すべてできなかった: "),
        (Reason::PasteAllFailed, "貼り付けできない（2 件すべて）: "),
    ];
    for pair in wrapped.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        for (all, old_head) in alls {
            let ja = all(vec![a.ja.clone(), b.ja.clone()]).text_in(Lang::Ja);
            assert_eq!(ja, format!("{old_head}{} / {}", a.old_ja, b.old_ja));
            let en_reason = all(vec![a.en.clone(), b.en.clone()]);
            let en = en_reason.text_in(Lang::En);
            if let Some(c) = has_japanese(&en) {
                panic!(
                    "{} の英語の断りに日本語（{c}）が混ざる: {en}（{}）",
                    variant_name(&format!("{en_reason:?}")),
                    reason_where(&en_reason, Lang::En)
                );
            }
        }
    }
}

/// 境界: 包みのパスが空・日本語のパス（パスは書き換えず、包みと理由の言語だけが変わる）。
/// 1 件だけ断られたときは並べずにその文をそのまま返す（`many_reply` / `paste_reply`）ので、
/// 件数の包みは 2 件以上でだけ使われる
#[test]
fn 移動とコピーの包みの境界() {
    let empty = Reason::MoveRefused {
        refusal: MoveRefusal::SourceMissing,
        src: String::new(),
        dest: String::new(),
    };
    assert_eq!(
        empty.text_in(Lang::Ja),
        "移動できない: 移す元が見つからない（ → ）"
    );
    assert_eq!(
        empty.text_in(Lang::En),
        "cannot move: the item to move was not found ( → )"
    );
    let named = Reason::CopyRefused {
        refusal: CopyRefusal::NameTaken,
        src: "/tmp/メモ.txt".into(),
        dest: "/tmp/移動先".into(),
    };
    let en = named.text_in(Lang::En);
    let at = reason_where(&named, Lang::En);
    assert!(en.starts_with("cannot copy: "), "{en}（{at}）");
    assert!(
        en.ends_with("(/tmp/メモ.txt → /tmp/移動先)"),
        "{en}（{at}）"
    );
    // パスを除けば日本語は残らない
    let without_paths = en.replace("/tmp/メモ.txt", "").replace("/tmp/移動先", "");
    assert!(has_japanese(&without_paths).is_none(), "{en}（{at}）");
    // 3 件以上でも件数と並びは保つ
    let three = Reason::AllFailed(vec!["a".into(), "b".into(), "c".into()]);
    assert_eq!(three.text_in(Lang::Ja), "3 件すべてできなかった: a / b / c");
    assert_eq!(three.text_in(Lang::En), "all 3 items failed: a / b / c");
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
        UiProblem::ReadIo { .. } => 21,
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
            P::ReadIo {
                kind: "permission denied".into(),
            },
            "ReadIo",
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

/// 使える値のうち言語ごとに書く範囲（`a〜z` / `1〜8 の番号` 等）の腕の行（語は日英で同じなので除く）
fn ui_allowed_where(allowed: &[Allowed], lang: Lang) -> String {
    let lines: Vec<String> = allowed
        .iter()
        .filter(|a| !matches!(a, Allowed::Word(_)))
        .map(|a| ui_where("impl Allowed {", variant_name(&format!("{a:?}")), lang))
        .collect();
    if lines.is_empty() {
        "-".to_string()
    } else {
        lines.join(", ")
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
                "{e:?} の英語に日本語（{ch}）が混ざる: {en}（枠 {} / 理由 {} / 使える値 {}）",
                ui_frame_where(with_allowed, Lang::En),
                ui_where(
                    "impl UiReason {",
                    variant_name(&format!("{:?}", e.what)),
                    Lang::En
                ),
                ui_allowed_where(&e.allowed, Lang::En)
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
        // 名指しは理由・枠・使える値の範囲の腕のすべて（どこに混ざっても行が出る）
        let at = |lang| match &err {
            tako_control::claude_mod_ui::RunError::Invalid(e) => format!(
                "理由 {} / 枠 {} / 使える値 {}",
                ui_where(
                    "impl UiReason {",
                    variant_name(&format!("{:?}", e.what)),
                    lang,
                ),
                ui_frame_where(!e.allowed.is_empty(), lang),
                ui_allowed_where(&e.allowed, lang)
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

// ---- #2365 の続き: `tako mod ui` の成功の応答の枠・CLI の表示・引数の読み方・置き場・排他の断り ----
//
// 断り（UiError 一式）を移した後に残っていた枠（`applies_to` / `note` / `choices` の説明と `hotkey`）・
// CLI の人向けの表示（`print_mod_ui`）・置き場の断り（`UiText`）・語彙の説明（各型の `describe_in`）・
// 引数の読み方の断り（`CliRequestError`）・排他の断り（`config_io::LockError`）。どれも (バリアント, 言語)
// の網羅 match で、落ちたら日英の対を書いたソースの行（`file:line`）を名指しする

use tako_control::claude_mod_ui::CliRequestError;
use tako_control::config_io::LockError;
use tako_core::claude_mod_ui::{ButtonKind, Preset, SlashCommand, TakoOp, UiText};

const MOD_UI_CONTROL_SRC: &str = include_str!("../src/claude_mod_ui.rs");
const MOD_UI_CONTROL_PATH: &str = "crates/tako-control/src/claude_mod_ui.rs";
const CONFIG_IO_SRC: &str = include_str!("../src/config_io.rs");
const CONFIG_IO_PATH: &str = "crates/tako-control/src/config_io.rs";

/// 1 つの文の見本（日英の文・#2365 より前の日本語・日英の対を書いた行）
struct FrameCase {
    debug: String,
    ja: String,
    en: String,
    old_ja: String,
    at: Box<dyn Fn(Lang) -> String>,
}

fn frame_case(
    debug: String,
    text: impl Fn(Lang) -> String,
    (src, path, from): (&'static str, &'static str, &'static str),
    arm: String,
    old_ja: String,
) -> FrameCase {
    FrameCase {
        debug,
        ja: text(Lang::Ja),
        en: text(Lang::En),
        old_ja,
        at: Box::new(move |lang| match arm_line(src, from, &arm, lang) {
            Some(line) => format!("{path}:{line}"),
            None => format!("{path}（{arm} の腕が見つからない）"),
        }),
    }
}

fn ui_text_index(t: &UiText) -> usize {
    match t {
        UiText::AppliesTo => 0,
        UiText::FutureNote { .. } => 1,
        UiText::HotkeyRule => 2,
        UiText::NoDataDir => 3,
        UiText::Heading { .. } => 4,
        UiText::StateAbsent => 5,
        UiText::StateRepaired { .. } => 6,
        UiText::StateFuture => 7,
        UiText::StateReadError => 8,
        UiText::Changed { .. } => 9,
        UiText::Unchanged { .. } => 10,
        UiText::Allowed { .. } => 11,
        UiText::ButtonsHeading { .. } => 12,
        UiText::ButtonActionsHeading => 13,
        UiText::ShellPromptLine => 14,
        UiText::ExamplesHeading => 15,
    }
}
const UI_TEXT_VARIANTS: usize = 16;

fn cli_request_index(e: CliRequestError) -> usize {
    match e {
        CliRequestError::Usage => 0,
        CliRequestError::ButtonNeedsVerb => 1,
        CliRequestError::FlagsOnlyForAdd => 2,
        CliRequestError::SetNeedsKeyValue => 3,
        CliRequestError::AddNeedsValue => 4,
        CliRequestError::AddTooManyWords => 5,
        CliRequestError::RemoveNeedsId => 6,
        CliRequestError::MoveNeedsIdAndTo => 7,
        CliRequestError::PresetNeedsName => 8,
    }
}
const CLI_REQUEST_VARIANTS: usize = 9;

fn lock_index(e: &LockError) -> usize {
    match e {
        LockError::CreateDir(_) => 0,
        LockError::Open { .. } => 1,
        LockError::Acquire { .. } => 2,
    }
}
const LOCK_VARIANTS: usize = 3;

/// 枠の文の見本と #2365 より前の固定文（`Allowed` は並べる種類かどうかで 2 つ見る）
fn ui_text_table() -> Vec<(UiText, &'static str, &'static str)> {
    let path = || "/tmp/ui.json".to_string();
    vec![
        (
            UiText::AppliesTo,
            "AppliesTo",
            "tako のペインの claude へは次の報告（最大 15 秒）で届く（ui.json は tako の data dir にあり、Claude Code の設定 dir へは書かない）",
        ),
        (
            UiText::FutureNote { version: 99 },
            "FutureNote",
            "新しい tako が書いた形（schema_version 99）。読める範囲で使い、書き換えない",
        ),
        (
            UiText::HotkeyRule,
            "HotkeyRule",
            "数字 1 字か英小文字 1 字（重複不可。省くと空いている英字を選ぶ）",
        ),
        (
            UiText::NoDataDir,
            "NoDataDir",
            "データディレクトリが決まらない（HOME が無い）",
        ),
        (
            UiText::Heading { path: path() },
            "Heading",
            "Claude Code の画面の UI（tako mod）: /tmp/ui.json",
        ),
        (
            UiText::StateAbsent,
            "StateAbsent",
            "（ファイルは無い = 既定で動いている）",
        ),
        (
            UiText::StateRepaired {
                quarantine: "/tmp/ui.json.unreadable.bak".into(),
            },
            "StateRepaired",
            "（読めない部分を既定で動かした。元は /tmp/ui.json.unreadable.bak に残した）",
        ),
        (
            UiText::StateFuture,
            "StateFuture",
            "（新しい tako が書いた形。読める範囲で使い、書き換えない）",
        ),
        (
            UiText::StateReadError,
            "StateReadError",
            "（読めないので既定で動いている）",
        ),
        (
            UiText::Changed { path: path() },
            "Changed",
            "変えた: /tmp/ui.json",
        ),
        (
            UiText::Unchanged { path: path() },
            "Unchanged",
            "変わらない（もうその値）: /tmp/ui.json",
        ),
        (
            UiText::Allowed {
                values: "a / b".into(),
                list: true,
            },
            "Allowed",
            "（a / b から並べる）",
        ),
        (
            UiText::Allowed {
                values: "a / b".into(),
                list: false,
            },
            "Allowed",
            "（a / b）",
        ),
        (
            UiText::ButtonsHeading { count: 2, max: 8 },
            "ButtonsHeading",
            "ボタン（2 / 8 個）:",
        ),
        (
            UiText::ButtonActionsHeading,
            "ButtonActionsHeading",
            "ボタンに使える動作:",
        ),
        (
            UiText::ShellPromptLine,
            "ShellPromptLine",
            "shell  \"<コマンド>\"（新しいペインで実行）・prompt \"<文>\"（入力欄へ入れるだけ）",
        ),
        (UiText::ExamplesHeading, "ExamplesHeading", "例:"),
    ]
}

/// 語彙の説明の #2365 より前の固定文（`describe()` の日本語。並びは各型の `ALL`）
const SLASH_OLD_JA: [&str; 7] = [
    "会話を要約して ctx を空ける",
    "会話を消して新しく始める",
    "ctx の使い方を見る",
    "このセッションの費用を見る",
    "モデルを選び直す",
    "effort を選び直す",
    "tako のサイドバーを開く",
];
const TAKO_OP_OLD_JA: [&str; 9] = [
    "右に新しいペインを開く",
    "下に新しいペインを開く",
    "会話を保ったまま claude を起動し直す",
    "引き継ぎを書かせてセッションを交代する",
    "使用制限が解けたら自動で続ける（on）",
    "使用制限の自動復帰を止める（off）",
    "このペインを裏へ回す",
    "このペインを閉じる",
    "このペインのフォルダを開く",
];
const BUTTON_KIND_OLD_JA: [&str; 4] = [
    "Claude Code の組み込みコマンドを送る（/compact 等。許可リストだけ）",
    "tako の操作を呼ぶ（ペインの右クリックと同じ操作）",
    "書いたコマンドを新しい tako のペインで実行する（4096 バイトまで）",
    "入力欄へ文を入れる（送らない。1000 文字まで）",
];
const PRESET_OLD_JA: [&str; 3] = [
    "既定（ボタンは compact だけ）",
    "おすすめ（ボタン: compact / context / split right / handoff）",
    "最小（帯はペイン名・要注意・ボタンだけ、バーは ctx だけ、チャットの装飾なし）",
];
const SET_KEY_OLD_JA: [&str; 12] = [
    "帯（プロンプトの上の 1 行）を隠す",
    "帯に並べる区切りと順番",
    "使用制限・ctx のバーの置き場（prompt_hint = 入力欄の下の行の末尾）",
    "バーに出す項目と順番",
    "コマンドカードを誰が描くか",
    "チャット風の描画を誰が描くか",
    "発言の吹き出し",
    "コードブロックの下のコピーボタン",
    "ツール呼び出しの要約",
    "強調の色（テーマのキー）",
    "警告の色（テーマのキー）",
    "控えめな文字の色（テーマのキー）",
];

/// 引数の読み方の断りの #2365 より前の固定文（語彙の語の並びは今の語彙から組む）
fn cli_request_old_ja(e: CliRequestError) -> String {
    use tako_core::claude_mod_ui::{SetKey, Vocab};
    let usage = "使い方: tako mod ui [set <キー> <値> | button add [<kind>] <中身> | button remove <id> | button move <id> <先> | preset <名前> | reset]";
    match e {
        CliRequestError::Usage => usage.into(),
        CliRequestError::ButtonNeedsVerb => {
            format!("button の後ろに add / remove / move を付ける。{usage}")
        }
        CliRequestError::FlagsOnlyForAdd => {
            "--label / --hotkey / --id は button add のときだけ使う".into()
        }
        CliRequestError::SetNeedsKeyValue => format!(
            "set にはキーと値が要る（tako mod ui set <キー> <値>）。キー: {}",
            SetKey::words().join(" / ")
        ),
        CliRequestError::AddNeedsValue => format!(
            "button add には中身が要る（例: tako mod ui button add compact）。kind: {}",
            ButtonKind::words().join(" / ")
        ),
        CliRequestError::AddTooManyWords => {
            "コマンド・文は 1 つの引数で渡す（例: tako mod ui button add shell \"npm test\"）"
                .into()
        }
        CliRequestError::RemoveNeedsId => {
            "button remove には id を 1 つ付ける（id は tako mod ui の一覧）".into()
        }
        CliRequestError::MoveNeedsIdAndTo => {
            "button move には id と先を付ける（先: 1〜8 / first / last / left / right）".into()
        }
        CliRequestError::PresetNeedsName => format!(
            "preset には名前を 1 つ付ける: {}",
            Preset::words().join(" / ")
        ),
    }
}

const CLI_REQUEST_ERRORS: [CliRequestError; CLI_REQUEST_VARIANTS] = [
    CliRequestError::Usage,
    CliRequestError::ButtonNeedsVerb,
    CliRequestError::FlagsOnlyForAdd,
    CliRequestError::SetNeedsKeyValue,
    CliRequestError::AddNeedsValue,
    CliRequestError::AddTooManyWords,
    CliRequestError::RemoveNeedsId,
    CliRequestError::MoveNeedsIdAndTo,
    CliRequestError::PresetNeedsName,
];

fn lock_table() -> Vec<(LockError, &'static str)> {
    vec![
        (
            LockError::CreateDir("Permission denied (os error 13)".into()),
            "ディレクトリの作成に失敗: Permission denied (os error 13)",
        ),
        (
            LockError::Open {
                path: "/tmp/ui.json.lock".into(),
                error: "Is a directory (os error 21)".into(),
            },
            "ロックファイルを開けない (/tmp/ui.json.lock): Is a directory (os error 21)",
        ),
        (
            LockError::Acquire {
                path: "/tmp/ui.json.lock".into(),
                error: "Resource busy".into(),
            },
            "ロックの取得に失敗 (/tmp/ui.json.lock): Resource busy",
        ),
    ]
}

/// 語彙の説明の見本（型ごとに `ALL` を回す）
fn describe_cases<T, const N: usize>(
    all: &[T],
    old: [&'static str; N],
    from: &'static str,
    text: fn(T, Lang) -> &'static str,
) -> Vec<FrameCase>
where
    T: Copy + std::fmt::Debug + 'static,
{
    assert_eq!(all.len(), N, "{from} の語彙の数と旧文の表の数が違う");
    all.iter()
        .zip(old)
        .map(|(v, old)| {
            let v = *v;
            frame_case(
                format!("{v:?}.describe_in"),
                move |lang| text(v, lang).to_string(),
                (MOD_UI_SRC, MOD_UI_PATH, from),
                variant_name(&format!("{v:?}")).to_string(),
                old.to_string(),
            )
        })
        .collect()
}

/// 全部の見本（枠の文・語彙の説明 5 型・引数の読み方の断り・排他の断り）
fn frame_cases() -> Vec<FrameCase> {
    use tako_core::claude_mod_ui::{SetKey, Vocab};
    let mut out = Vec::new();
    for (t, arm, old) in ui_text_table() {
        let debug = format!("{t:?}");
        out.push(frame_case(
            debug,
            move |lang| t.text_in(lang),
            (MOD_UI_SRC, MOD_UI_PATH, "impl UiText {"),
            arm.to_string(),
            old.to_string(),
        ));
    }
    out.extend(describe_cases(
        SlashCommand::ALL,
        SLASH_OLD_JA,
        "impl SlashCommand {",
        SlashCommand::describe_in,
    ));
    out.extend(describe_cases(
        TakoOp::ALL,
        TAKO_OP_OLD_JA,
        "impl TakoOp {",
        TakoOp::describe_in,
    ));
    out.extend(describe_cases(
        ButtonKind::ALL,
        BUTTON_KIND_OLD_JA,
        "impl ButtonKind {",
        ButtonKind::describe_in,
    ));
    out.extend(describe_cases(
        Preset::ALL,
        PRESET_OLD_JA,
        "impl Preset {",
        Preset::describe_in,
    ));
    out.extend(describe_cases(
        SetKey::ALL,
        SET_KEY_OLD_JA,
        "impl SetKey {",
        SetKey::describe_in,
    ));
    for e in CLI_REQUEST_ERRORS {
        out.push(frame_case(
            format!("{e:?}"),
            move |lang| e.text_in(lang),
            (
                MOD_UI_CONTROL_SRC,
                MOD_UI_CONTROL_PATH,
                "impl CliRequestError {",
            ),
            variant_name(&format!("{e:?}")).to_string(),
            cli_request_old_ja(e),
        ));
    }
    for (e, old) in lock_table() {
        let arm = variant_name(&format!("{e:?}")).to_string();
        out.push(frame_case(
            format!("{e:?}"),
            move |lang| e.text_in(lang),
            (CONFIG_IO_SRC, CONFIG_IO_PATH, "impl LockError {"),
            arm,
            old.to_string(),
        ));
    }
    out
}

#[test]
fn mod_ui_の枠と表示と引数と排他の見本は全バリアントを持つ() {
    let texts: Vec<usize> = ui_text_table()
        .iter()
        .map(|(t, _, _)| ui_text_index(t))
        .collect();
    let missing: Vec<usize> = (0..UI_TEXT_VARIANTS)
        .filter(|i| !texts.contains(i))
        .collect();
    assert!(
        missing.is_empty(),
        "UiText の見本に無い通し番号: {missing:?}"
    );
    covers_all(
        &CLI_REQUEST_ERRORS,
        |e| cli_request_index(*e),
        CLI_REQUEST_VARIANTS,
    );
    let locks: Vec<LockError> = lock_table().into_iter().map(|(e, _)| e).collect();
    covers_all(&locks, lock_index, LOCK_VARIANTS);
}

/// 落ちたものを全部並べる（1 件目で止めると、ほかの書き忘れが次の実行まで見えない）
fn report_all(failures: Vec<String>) {
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn mod_ui_の枠と表示と引数と排他が日英を持ち英語に日本語が混ざらない() {
    let mut failures = Vec::new();
    for c in frame_cases() {
        if c.ja.trim().is_empty() || c.en.trim().is_empty() {
            failures.push(format!("{} の文が空（{}）", c.debug, (c.at)(Lang::En)));
        }
        if let Some(ch) = has_japanese(&c.en) {
            failures.push(format!(
                "{} の英語に日本語（{ch}）が混ざる: {}（{}）",
                c.debug,
                c.en,
                (c.at)(Lang::En)
            ));
        } else if c.ja == c.en {
            failures.push(format!(
                "{} の英語が日本語と同じ文（{}）",
                c.debug,
                (c.at)(Lang::En)
            ));
        }
    }
    report_all(failures);
}

#[test]
fn mod_ui_の枠と表示と引数と排他の日本語は2365より前の固定文とバイト一致() {
    let mut failures = Vec::new();
    for c in frame_cases() {
        if c.ja != c.old_ja {
            failures.push(format!(
                "{} の日本語が #2365 より前と違う（{}）\n  want: {}\n  got:  {}",
                c.debug,
                (c.at)(Lang::Ja),
                c.old_ja,
                c.ja
            ));
        }
    }
    report_all(failures);
}

/// 番犬自身の検査: 全見本の腕の行を日英とも引ける（引けないと名指しが空振りする）
#[test]
fn mod_ui_の枠と表示と引数と排他の腕の行を日英とも引ける() {
    for c in frame_cases() {
        for lang in [Lang::Ja, Lang::En] {
            let at = (c.at)(lang);
            assert!(
                !at.contains("腕が見つからない"),
                "{} / {lang:?}: {at}",
                c.debug
            );
        }
    }
}

/// 1 つの応答（`choices` 全体・排他の断りを包んだ `RunError`）の中で言語がそろう。
/// 日本語の `choices` は説明の文が #2365 より前と同じ（説明の文以外 = 語彙の語・数は言語で変わらない）
#[test]
fn mod_ui_の選べる値と排他の断りは1つの応答の中で同じ言語にそろう() {
    use tako_core::claude_mod_ui::choices_in;
    fn strings(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::String(s) => out.push(s.clone()),
            serde_json::Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
            serde_json::Value::Object(m) => m.values().for_each(|x| strings(x, out)),
            _ => {}
        }
    }
    let mut en = Vec::new();
    strings(&choices_in(Lang::En), &mut en);
    let mixed: Vec<&String> = en.iter().filter(|s| has_japanese(s).is_some()).collect();
    assert!(
        mixed.is_empty(),
        "英語の choices に日本語が混ざる: {mixed:?}"
    );
    let ja = choices_in(Lang::Ja);
    assert_eq!(
        ja["button"]["hotkey"],
        "数字 1 字か英小文字 1 字（重複不可。省くと空いている英字を選ぶ）"
    );
    assert_eq!(ja["presets"][2]["about"], PRESET_OLD_JA[2]);
    // 説明の文を外せば日英で同じ形（語彙の語・数・並び）
    fn without_text(v: &serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(m) => serde_json::Value::Object(
                m.iter()
                    .filter(|(k, _)| k.as_str() != "about" && k.as_str() != "hotkey")
                    .map(|(k, x)| (k.clone(), without_text(x)))
                    .collect(),
            ),
            serde_json::Value::Array(a) => {
                serde_json::Value::Array(a.iter().map(without_text).collect())
            }
            other => other.clone(),
        }
    }
    assert_eq!(without_text(&ja), without_text(&choices_in(Lang::En)));
    // 排他の断りは `RunError::Lock` に包んでも同じ言語のまま（MCP は前置きつきで返る）
    for (e, old) in lock_table() {
        let run = tako_control::claude_mod_ui::RunError::Lock(e.clone());
        assert_eq!(run.text_in(Lang::Ja), old);
        let shown = E::Operation(run.text_in(Lang::En)).text_in(Lang::En);
        assert!(has_japanese(&shown).is_none(), "{shown}");
    }
}

// --- #2365 の続き: tako mod（status / on / off / band / install / uninstall）の枠と理由 -----------
//
// `tako mod band on|off` の応答は `tako mod`（status）の応答そのものに `applies_to` を足した形なので、
// 同じ応答に載る文（注入しない理由 `OffReason`・報告の無い理由 `NoReportCause`・展開の失敗
// `InstallError`・skills の衝突 `Conflict`・置かない理由 `Blocked` / `WriteRefused`・置き場が決まらない
// `paths::NoDataDir`・枠と CLI の表示 `claude_mod::text::ModText`）をまとめて見る。どれも (バリアント, 言語)
// の網羅 match で、落ちたら日英の対を書いたソースの行（`file:line`）を名指しする

use tako_control::claude_mod::text::ModText;
use tako_core::claude_mod::{InstallError, NoReportCause, OffReason};
use tako_core::claude_mod_install::{Blocked, Conflict, WriteRefused};

const CORE_MOD_SRC: &str = include_str!("../../tako-core/src/claude_mod.rs");
const CORE_MOD_PATH: &str = "crates/tako-core/src/claude_mod.rs";
const CORE_MOD_INSTALL_SRC: &str = include_str!("../../tako-core/src/claude_mod_install.rs");
const CORE_MOD_INSTALL_PATH: &str = "crates/tako-core/src/claude_mod_install.rs";
const PATHS_SRC: &str = include_str!("../../tako-core/src/paths.rs");
const PATHS_PATH: &str = "crates/tako-core/src/paths.rs";
const CONTROL_MOD_SRC: &str = include_str!("../src/claude_mod.rs");
const CONTROL_MOD_PATH: &str = "crates/tako-control/src/claude_mod.rs";

/// バリアントの無い型（`NoDataDir` / `WriteRefused`）の `text_in` で、言語の腕（`Ja =>` / `(Self, Ja) =>`）が
/// 始まる行（1 始まり）
fn lang_arm_line(src: &str, from: &str, lang: Lang) -> Option<usize> {
    let token = match lang {
        Lang::Ja => "Ja",
        Lang::En => "En",
    };
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(from))?;
    let marks = [format!(" {token} =>"), format!(", {token}) =>")];
    (start..lines.len())
        .find(|&i| marks.iter().any(|m| lines[i].contains(m.as_str())))
        .map(|i| i + 1)
}

fn plain_case(
    debug: String,
    text: impl Fn(Lang) -> String,
    (src, path, from): (&'static str, &'static str, &'static str),
    old_ja: String,
) -> FrameCase {
    FrameCase {
        debug,
        ja: text(Lang::Ja),
        en: text(Lang::En),
        old_ja,
        at: Box::new(move |lang| match lang_arm_line(src, from, lang) {
            Some(line) => format!("{path}:{line}"),
            None => format!("{path}（{from} の {lang:?} の腕が見つからない）"),
        }),
    }
}

fn off_reason_index(r: &OffReason) -> usize {
    match r {
        OffReason::AbOff => 0,
        OffReason::Disabled => 1,
        OffReason::NotInstalled => 2,
        OffReason::NoCli => 3,
        OffReason::ClaudeUnknown => 4,
        OffReason::ClaudeTooOld(_) => 5,
        OffReason::NameConflict(_) => 6,
        OffReason::UserDisabled => 7,
    }
}
const OFF_REASON_VARIANTS: usize = 8;

fn no_report_index(c: &NoReportCause) -> usize {
    match c {
        NoReportCause::SafeMode => 0,
        NoReportCause::Bare => 1,
        NoReportCause::BeforeInjection => 2,
        NoReportCause::NotLoaded => 3,
        NoReportCause::Undetermined => 4,
    }
}
const NO_REPORT_VARIANTS: usize = 5;

fn install_error_index(e: &InstallError) -> usize {
    match e {
        InstallError::NoDataDir => 0,
        InstallError::CreateRoot { .. } => 1,
        InstallError::Write(_) => 2,
        InstallError::Replace(_) => 3,
    }
}
const INSTALL_ERROR_VARIANTS: usize = 4;

fn conflict_index(c: &Conflict) -> usize {
    match c {
        Conflict::UserSkillsDir { .. } => 0,
        Conflict::ModifiedCopy => 1,
        Conflict::SkillsSameName { .. } => 2,
        Conflict::InstalledPlugin { .. } => 3,
        Conflict::EnabledPlugin { .. } => 4,
    }
}
const CONFLICT_VARIANTS: usize = 5;

fn blocked_index(b: &Blocked) -> usize {
    match b {
        Blocked::Refused(_) => 0,
        Blocked::Legacy => 1,
        Blocked::Disabled => 2,
        Blocked::ClaudeUnknown => 3,
        Blocked::ClaudeTooOld(_) => 4,
    }
}
const BLOCKED_VARIANTS: usize = 5;

fn mod_text_index(t: &ModText) -> usize {
    match t {
        ModText::AppliesToOnOff => 0,
        ModText::AppliesToBand => 1,
        ModText::AppliesToInstall => 2,
        ModText::AppliesToUninstall => 3,
        ModText::NoHub => 4,
        ModText::UnknownAction { .. } => 5,
        ModText::UnknownInstallAction { .. } => 6,
        ModText::Stale { .. } => 7,
        ModText::BandHiddenByOtherMod => 8,
        ModText::SkillsOutdated => 9,
        ModText::SkillsUserDisabled => 10,
        ModText::SkillsNotInstalled => 11,
        ModText::SkillsNoConfigDir => 12,
        ModText::InstallTargetsFailed => 13,
        ModText::InjectionUnknown { .. } => 14,
        ModText::Injecting => 15,
        ModText::NotInjecting { .. } => 16,
        ModText::NotInjectingBare => 17,
        ModText::VersionLine { .. } => 18,
        ModText::InstallHeading { .. } => 19,
        ModText::SkillsHeading => 20,
        ModText::SkillsLegacy => 21,
        ModText::SkillsWritten => 22,
        ModText::SkillsRemoved => 23,
        ModText::SkillsWritePlanned => 24,
        ModText::SkillsRemovePlanned => 25,
        ModText::BandLegacy => 26,
        ModText::BandLine { .. } => 27,
        ModText::CardsLegacy => 28,
        ModText::CardsPlace { .. } => 29,
        ModText::CardsClickable => 30,
        ModText::CardsNoButtonLegacy => 31,
        ModText::CardsNoClick => 32,
        ModText::PaneBandHidden => 33,
        ModText::PaneBandShown { .. } => 34,
        ModText::PaneBandNotDrawn => 35,
        ModText::PaneCards { .. } => 36,
        ModText::SecondsAgo { .. } => 37,
    }
}
const MOD_TEXT_VARIANTS: usize = 38;

/// 注入しない理由の見本と #2365 より前の固定文
fn off_reason_table() -> Vec<(OffReason, String)> {
    use tako_core::claude_mod::{AB_OFF_ENV, MIN_CLAUDE_VERSION};
    vec![
        (
            OffReason::AbOff,
            format!("{AB_OFF_ENV} が立っている（A/B で注入を止めている）"),
        ),
        (
            OffReason::Disabled,
            "tako mod off で止めている（`tako mod on` で戻る）".into(),
        ),
        (
            OffReason::NotInstalled,
            "mod を data dir へ展開できていない".into(),
        ),
        (
            OffReason::NoCli,
            "実行中の tako の隣に tako CLI が無い".into(),
        ),
        (
            OffReason::ClaudeUnknown,
            "claude の版がまだ分からない（claude が入っていないか判定前）".into(),
        ),
        (
            OffReason::ClaudeTooOld("2.1.280".into()),
            format!("claude 2.1.280 は下限 {MIN_CLAUDE_VERSION} 未満（`claude update` で上がる）"),
        ),
        (
            OffReason::NameConflict(Box::new(Conflict::InstalledPlugin {
                id: "tako@other".into(),
            })),
            "同じ名前の plugin tako@other が入っている（plugins/installed_plugins.json）\
             （利用者の tako を優先し、このペインには注入しない）"
                .into(),
        ),
        (
            OffReason::UserDisabled,
            "Claude Code の /plugin で tako@skills-dir が止められている（利用者の選択を優先）"
                .into(),
        ),
    ]
}

fn no_report_table() -> Vec<(NoReportCause, &'static str)> {
    vec![
        (
            NoReportCause::SafeMode,
            "claude が --safe-mode で起動されていて mod を読まない（--safe-mode を外して起動し直せば載る）",
        ),
        (
            NoReportCause::Bare,
            "claude が --bare で起動されていて mod を読まない（--bare を外して起動し直せば載る）",
        ),
        (
            NoReportCause::BeforeInjection,
            "注入より前から動いていた claude（起動時の環境に tako の注入の印が無い）。\
             新しいペインで claude を起動すれば載る",
        ),
        (
            NoReportCause::NotLoaded,
            "claude には注入が届いているが mod から報告が無い。disableAllHooks / 組織の \
             allowManagedModsOnly で mod が読まれていないか、mod の読み込みに失敗した",
        ),
        (
            NoReportCause::Undetermined,
            "claude は動いているが mod から報告が無い。--safe-mode / --bare / \
             disableAllHooks / 組織の allowManagedModsOnly で mod が読まれていないか、\
             注入より前から動いていた claude",
        ),
    ]
}

fn install_error_table() -> Vec<(InstallError, &'static str)> {
    vec![
        (
            InstallError::NoDataDir,
            "データディレクトリが決まらない（HOME が無い）",
        ),
        (
            InstallError::CreateRoot {
                path: "/tmp/claude-mod".into(),
                error: "Permission denied (os error 13)".into(),
            },
            "/tmp/claude-mod を作れない: Permission denied (os error 13)",
        ),
        (
            InstallError::Write("No space left on device (os error 28)".into()),
            "mod を書き出せない: No space left on device (os error 28)",
        ),
        (
            InstallError::Replace("Directory not empty (os error 66)".into()),
            "mod を差し替えられない: Directory not empty (os error 66)",
        ),
    ]
}

/// 衝突の見本（腕の頭・#2365 より前の固定文）。`UserSkillsDir` は印の有無で腕が 2 本
fn conflict_table() -> Vec<(Conflict, &'static str, &'static str)> {
    vec![
        (
            Conflict::UserSkillsDir { named_tako: true },
            "UserSkillsDir { named_tako: true }",
            "skills/tako は利用者の plugin（tako の管理印が無い）なので触らない",
        ),
        (
            Conflict::UserSkillsDir { named_tako: false },
            "UserSkillsDir { named_tako: false }",
            "skills/tako は利用者のもの（tako の管理印が無い）なので触らない",
        ),
        (
            Conflict::ModifiedCopy,
            "ModifiedCopy",
            "skills/tako の写しに手が入っている（管理印のハッシュと違う）ので上書きしない",
        ),
        (
            Conflict::SkillsSameName {
                dir: "other".into(),
            },
            "SkillsSameName",
            "skills/other が同じ名前の plugin tako を名乗っている",
        ),
        (
            Conflict::InstalledPlugin {
                id: "tako@other".into(),
            },
            "InstalledPlugin",
            "同じ名前の plugin tako@other が入っている（plugins/installed_plugins.json）",
        ),
        (
            Conflict::EnabledPlugin {
                id: "tako@other".into(),
            },
            "EnabledPlugin",
            "同じ名前の plugin tako@other が有効になっている（settings.json の enabledPlugins）",
        ),
    ]
}

fn blocked_table() -> Vec<(Blocked, String)> {
    use tako_core::claude_mod::{AB_OFF_ENV, MIN_CLAUDE_VERSION};
    use tako_core::claude_mod_install::LEGACY_ENV;
    vec![
        (
            Blocked::Refused(WriteRefused {
                dir: "~/.claude".into(),
            }),
            "検証プロセス（テスト・TAKO_ISOLATED・セルフテスト）は一時 dir の外の設定 dir へ書かない: ~/.claude"
                .into(),
        ),
        (
            Blocked::Legacy,
            format!("{LEGACY_ENV} / {AB_OFF_ENV} が立っている（A/B で skills-dir へ入れない）"),
        ),
        (
            Blocked::Disabled,
            "tako mod off で止めている（`tako mod on` で戻る）".into(),
        ),
        (
            Blocked::ClaudeUnknown,
            "claude の版が分からない（claude が入っていないか判定前）ので入れない".into(),
        ),
        (
            Blocked::ClaudeTooOld("2.1.280".into()),
            format!(
                "claude 2.1.280 は下限 {MIN_CLAUDE_VERSION} 未満なので入れない（`claude update` で上がる）"
            ),
        ),
    ]
}

/// 枠と CLI の表示の文の見本と #2365 より前の固定文（引数で文の変わるものは代表を複数）
fn mod_text_table() -> Vec<(ModText, &'static str)> {
    let version = |claude: Option<&str>, min: &str, m: &str, install: &str| ModText::VersionLine {
        claude: claude.map(str::to_string),
        min: min.into(),
        mod_version: m.into(),
        install: install.into(),
    };
    let heading = |install: bool, dry_run: bool, claude: Option<&str>| ModText::InstallHeading {
        install,
        dry_run,
        claude: claude.map(str::to_string),
        min: "2.1.294".into(),
    };
    let band = |request: Option<bool>| ModText::BandLine {
        ctx_percent: 80,
        limit_percent: 90.0,
        request,
    };
    vec![
        (
            ModText::AppliesToOnOff,
            "次に作るペインから（動いている claude には効かない。止めるなら claude を起動し直す）",
        ),
        (
            ModText::AppliesToBand,
            "動いている claude の帯へ次の報告（最大 15 秒）で届く。Claude Code の中で /tako band on|off \
             を後から打てば、そちらが勝つ",
        ),
        (
            ModText::AppliesToInstall,
            "走っている claude にもホットリロードで効く（tako の外の claude では休眠）",
        ),
        (
            ModText::AppliesToUninstall,
            "走っている claude からも外れる。tako のペインには env の注入（S1）で今までどおり読まれる",
        ),
        (
            ModText::NoHub,
            "この tako は mod を扱わない（2 つ目のインスタンス等）",
        ),
        (
            ModText::UnknownAction {
                action: "bogus".into(),
            },
            "未知の action: bogus（status / on / off / band-on / band-off / ui / install / \
             uninstall / report）",
        ),
        (
            ModText::UnknownInstallAction {
                action: "bogus".into(),
            },
            "未知の action: bogus（install / uninstall）",
        ),
        (
            ModText::Stale {
                age_secs: 60,
                fresh_for_secs: 30,
            },
            "最後の報告から 60 秒（30 秒で失効）。claude が終わったか mod が止まった",
        ),
        (
            ModText::BandHiddenByOtherMod,
            "帯は他の mod に隠された（Claude Code の帯のフックが呼ばれていない = 外側の mod が next を\
             呼ばずに帯を描いている）。入力欄の下の行のバーとサイドバー（/tako）はそのまま使える",
        ),
        (
            ModText::SkillsOutdated,
            "tako の写しが古い（tako mod install か次の GUI 起動で差し替わる）",
        ),
        (
            ModText::SkillsUserDisabled,
            "Claude Code の /plugin で tako@skills-dir が止められている（tako のペインでも休眠する）",
        ),
        (
            ModText::SkillsNotInstalled,
            "まだ入れていない（tako mod install / tako setup で入る）",
        ),
        (
            ModText::SkillsNoConfigDir,
            "設定 dir が無い（claude がまだ使っていない。tako は設定 dir を作らない）",
        ),
        (
            ModText::InstallTargetsFailed,
            "置けない（外せない）設定 dir がある（targets の error）",
        ),
        (
            ModText::InjectionUnknown { reason: "x".into() },
            "tako mod: 不明（x）",
        ),
        (ModText::Injecting, "tako mod: 新しく作るペインへ注入する"),
        (
            ModText::NotInjecting { reason: "x".into() },
            "tako mod: 注入しない（x）",
        ),
        (ModText::NotInjectingBare, "tako mod: 注入しない"),
        (
            version(Some("2.1.294"), "2.1.294", "0.8.39", "written"),
            "claude 2.1.294（下限 2.1.294）/ mod 0.8.39 / 展開 written",
        ),
        (
            version(None, "-", "-", "-"),
            "claude 未判定（下限 -）/ mod - / 展開 -",
        ),
        (
            heading(true, true, Some("2.1.294")),
            "tako mod install（dry-run。何も書いていない）: claude 2.1.294（下限 2.1.294）",
        ),
        (
            heading(true, false, None),
            "tako mod install: claude 未判定（下限 2.1.294）",
        ),
        (
            heading(false, true, Some("2.1.294")),
            "tako mod uninstall（dry-run。何も書いていない）: claude 2.1.294（下限 2.1.294）",
        ),
        (
            heading(false, false, Some("2.1.294")),
            "tako mod uninstall: claude 2.1.294（下限 2.1.294）",
        ),
        (ModText::SkillsHeading, "Claude Code の設定 dir（skills/tako）:"),
        (
            ModText::SkillsLegacy,
            "skills: 入れない（TAKO_1959_LEGACY / TAKO_1877_NO_MOD）",
        ),
        (ModText::SkillsWritten, "  → 置いた"),
        (ModText::SkillsRemoved, "  → 外した"),
        (ModText::SkillsWritePlanned, "  → 置く予定"),
        (ModText::SkillsRemovePlanned, "  → 外す予定"),
        (ModText::BandLegacy, "帯: 描かない（TAKO_1877_S3_LEGACY）"),
        (
            band(Some(true)),
            "帯: ctx 80% / 使用制限 90% 以上で ctx・使用制限も出す・隠す（tako mod band on で戻す）",
        ),
        (
            band(Some(false)),
            "帯: ctx 80% / 使用制限 90% 以上で ctx・使用制限も出す・出す",
        ),
        (
            band(None),
            "帯: ctx 80% / 使用制限 90% 以上で ctx・使用制限も出す",
        ),
        (
            ModText::CardsLegacy,
            "カード: ペイン下のカード帯（TAKO_1963_LEGACY）",
        ),
        (
            ModText::CardsPlace {
                place: "auto".into(),
            },
            "カード: 置き場 auto（mod が Claude Code の画面に描いたカードはペイン下のカード帯に出さない）",
        ),
        (
            ModText::CardsClickable,
            "  会話の行のカードの「実行」をクリック 1 回で押せる（全画面の claude）",
        ),
        (
            ModText::CardsNoButtonLegacy,
            "  会話の行のカードにボタンを置かない（TAKO_2159_LEGACY）。帯の ctrl+x → Tab → キーで押す",
        ),
        (
            ModText::CardsNoClick,
            "  クリックを渡していないので、帯の ctrl+x → Tab → キーで押す（クリックで押すなら tako mouse on）",
        ),
        (ModText::PaneBandHidden, "  帯 非表示"),
        (
            ModText::PaneBandShown {
                columns: 120,
                segments: "pane tab".into(),
            },
            "  帯 120 桁: pane tab",
        ),
        (ModText::PaneBandNotDrawn, "  帯 描いていない"),
        (
            ModText::PaneCards {
                ids: "1, 2".into(),
            },
            "  カード [1, 2]",
        ),
        (ModText::SecondsAgo { secs: 3 }, "(3 秒前)"),
    ]
}

/// 全部の見本（tako-core の理由 5 型 + 番人の断り + 置き場の断り + 枠と CLI の表示）
fn mod_band_cases() -> Vec<FrameCase> {
    let mut out = Vec::new();
    for (r, old) in off_reason_table() {
        let arm = variant_name(&format!("{r:?}")).to_string();
        out.push(frame_case(
            format!("OffReason::{r:?}"),
            move |lang| r.describe_in(lang),
            (CORE_MOD_SRC, CORE_MOD_PATH, "impl OffReason {"),
            arm,
            old,
        ));
    }
    for (c, old) in no_report_table() {
        out.push(frame_case(
            format!("NoReportCause::{c:?}"),
            move |lang| c.describe_in(lang).to_string(),
            (CORE_MOD_SRC, CORE_MOD_PATH, "impl NoReportCause {"),
            variant_name(&format!("{c:?}")).to_string(),
            old.to_string(),
        ));
    }
    for (e, old) in install_error_table() {
        let arm = variant_name(&format!("{e:?}")).to_string();
        out.push(frame_case(
            format!("InstallError::{e:?}"),
            move |lang| e.text_in(lang),
            (CORE_MOD_SRC, CORE_MOD_PATH, "impl InstallError {"),
            arm,
            old.to_string(),
        ));
    }
    for (c, arm, old) in conflict_table() {
        out.push(frame_case(
            format!("Conflict::{c:?}"),
            move |lang| c.describe_in(lang),
            (
                CORE_MOD_INSTALL_SRC,
                CORE_MOD_INSTALL_PATH,
                "impl Conflict {",
            ),
            arm.to_string(),
            old.to_string(),
        ));
    }
    for (b, old) in blocked_table() {
        let arm = variant_name(&format!("{b:?}")).to_string();
        out.push(frame_case(
            format!("Blocked::{b:?}"),
            move |lang| b.describe_in(lang),
            (
                CORE_MOD_INSTALL_SRC,
                CORE_MOD_INSTALL_PATH,
                "impl Blocked {",
            ),
            arm,
            old,
        ));
    }
    let refused = WriteRefused {
        dir: "~/.claude".into(),
    };
    out.push(plain_case(
        "WriteRefused".into(),
        move |lang| refused.text_in(lang),
        (
            CORE_MOD_INSTALL_SRC,
            CORE_MOD_INSTALL_PATH,
            "impl WriteRefused {",
        ),
        "検証プロセス（テスト・TAKO_ISOLATED・セルフテスト）は一時 dir の外の設定 dir へ書かない: ~/.claude"
            .into(),
    ));
    out.push(plain_case(
        "paths::NoDataDir".into(),
        |lang| tako_core::paths::NoDataDir.text_in(lang).to_string(),
        (PATHS_SRC, PATHS_PATH, "impl NoDataDir {"),
        "データディレクトリが決まらない（HOME が無い）".into(),
    ));
    for (t, old) in mod_text_table() {
        let arm = variant_name(&format!("{t:?}")).to_string();
        out.push(frame_case(
            format!("ModText::{t:?}"),
            move |lang| t.text_in(lang),
            (CONTROL_MOD_SRC, CONTROL_MOD_PATH, "impl ModText {"),
            arm,
            old.to_string(),
        ));
    }
    out
}

/// 見本の通し番号が型の全バリアントを含むか（同じバリアントの見本が複数あってよい）
fn covers_every<T>(samples: &[T], index: impl Fn(&T) -> usize, n: usize, name: &str) {
    let seen: Vec<usize> = samples.iter().map(index).collect();
    let missing: Vec<usize> = (0..n).filter(|i| !seen.contains(i)).collect();
    assert!(
        missing.is_empty(),
        "{name} の見本に無いバリアント（通し番号）: {missing:?}"
    );
}

#[test]
fn mod_の状態と帯の応答の見本は全バリアントを持つ() {
    let offs: Vec<OffReason> = off_reason_table().into_iter().map(|(r, _)| r).collect();
    covers_every(&offs, off_reason_index, OFF_REASON_VARIANTS, "OffReason");
    let causes: Vec<NoReportCause> = no_report_table().into_iter().map(|(c, _)| c).collect();
    covers_every(
        &causes,
        no_report_index,
        NO_REPORT_VARIANTS,
        "NoReportCause",
    );
    let errors: Vec<InstallError> = install_error_table().into_iter().map(|(e, _)| e).collect();
    covers_every(
        &errors,
        install_error_index,
        INSTALL_ERROR_VARIANTS,
        "InstallError",
    );
    let conflicts: Vec<Conflict> = conflict_table().into_iter().map(|(c, _, _)| c).collect();
    covers_every(&conflicts, conflict_index, CONFLICT_VARIANTS, "Conflict");
    let blocked: Vec<Blocked> = blocked_table().into_iter().map(|(b, _)| b).collect();
    covers_every(&blocked, blocked_index, BLOCKED_VARIANTS, "Blocked");
    let texts: Vec<ModText> = mod_text_table().into_iter().map(|(t, _)| t).collect();
    covers_every(&texts, mod_text_index, MOD_TEXT_VARIANTS, "ModText");
}

#[test]
fn mod_の状態と帯の応答の文が日英を持ち英語に日本語が混ざらない() {
    let mut failures = Vec::new();
    for c in mod_band_cases() {
        if c.ja.trim().is_empty() || c.en.trim().is_empty() {
            failures.push(format!("{} の文が空（{}）", c.debug, (c.at)(Lang::En)));
        }
        if let Some(ch) = has_japanese(&c.en) {
            failures.push(format!(
                "{} の英語に日本語（{ch}）が混ざる: {}（{}）",
                c.debug,
                c.en,
                (c.at)(Lang::En)
            ));
        } else if c.ja == c.en {
            failures.push(format!(
                "{} の英語が日本語と同じ文（{}）",
                c.debug,
                (c.at)(Lang::En)
            ));
        }
    }
    report_all(failures);
}

#[test]
fn mod_の状態と帯の応答の文の日本語は2365より前の固定文とバイト一致() {
    let mut failures = Vec::new();
    for c in mod_band_cases() {
        if c.ja != c.old_ja {
            failures.push(format!(
                "{} の日本語が #2365 より前と違う（{}）\n  want: {}\n  got:  {}",
                c.debug,
                (c.at)(Lang::Ja),
                c.old_ja,
                c.ja
            ));
        }
    }
    report_all(failures);
}

/// 番犬自身の検査: 全見本の腕の行を日英とも引ける（引けないと名指しが空振りする）
#[test]
fn mod_の状態と帯の応答の文の腕の行を日英とも引ける() {
    for c in mod_band_cases() {
        for lang in [Lang::Ja, Lang::En] {
            let at = (c.at)(lang);
            assert!(
                !at.contains("腕が見つからない"),
                "{} / {lang:?}: {at}",
                c.debug
            );
        }
    }
}

// ---- #2365 の続き: action で操作を選ぶツールの断り（呼び手のソースを読む番犬） ----
//
// 不明な action・不明な値・操作に要る引数（`<操作> には <鍵> が必要`）と、それらのツールが共有する
// 設定の保存・置き場の断りの 5 つの族は、dispatch の 80 か所の呼び手が鍵の名前と候補を値で `Reason`
// （`UnknownValue` / `ActionNeeds` ほか）へ渡す。日英の対は上の `Reason` の表で見るが、次の 2 つは
// 表では見えない:
// (1) 呼び手が旧形の日本語の固定文（`DispatchError::InvalidParams(format!("不明な action: …"))`）を
//     書き足す = 英語表示でもその 1 か所だけ日本語で返る
// (2) 呼び手が渡す候補・形の案内（`allowed` / `hint`）に日本語を入れる = 英語の腕がそのまま埋める
// どちらも dispatch のソースを読んで `file:line` で名指す

/// dispatch の本体（単体テストの前まで）の行（1 始まりの行番号つき）。
/// `impl Reason {` の本体（日英の対の表）とコメントの行は外す（そこの日本語は正しい）
fn dispatch_code_lines() -> Vec<(usize, &'static str)> {
    let lines: Vec<&'static str> = DISPATCH_SRC.lines().collect();
    let tests = lines
        .iter()
        .position(|l| l.starts_with("#[cfg(test)]"))
        .unwrap_or(lines.len());
    let table = lines
        .iter()
        .position(|l| *l == "impl Reason {")
        .expect("dispatch.rs に impl Reason { が無い");
    let table_end = table
        + lines[table..]
            .iter()
            .position(|l| *l == "}")
            .expect("impl Reason の終わりが無い");
    lines[..tests]
        .iter()
        .enumerate()
        .filter(|(i, _)| !(table..=table_end).contains(i))
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .map(|(i, l)| (i + 1, *l))
        .collect()
}

/// 1 行の中の文字列リテラルの中身（`"…"`。行をまたぐものは行末まで）
fn line_literals(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = None;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        match start {
            None if c == '"' => start = Some(i + 1),
            None => {}
            Some(_) if escaped => escaped = false,
            Some(_) if c == '\\' => escaped = true,
            Some(s) if c == '"' => {
                out.push(&line[s..i]);
                start = None;
            }
            Some(_) => {}
        }
    }
    if let Some(s) = start {
        out.push(&line[s..]);
    }
    out
}

/// 鍵・操作の名前（`task_id` / `set-color` / `{action}`）
fn is_word(s: &str) -> bool {
    let s = s
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(s);
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// 旧形の日本語の固定文なら、その族の名前
fn old_form(lit: &str) -> Option<&'static str> {
    if lit.starts_with("不明な ") {
        return Some("不明な <鍵>: …（Reason::UnknownValue ほか）");
    }
    if lit.starts_with("設定の保存に失敗") {
        return Some("設定の保存に失敗（Reason::SaveSettingsFailed）");
    }
    if lit.starts_with("データディレクトリを解決できない") {
        return Some("データディレクトリを解決できない（Reason::DataDirUnresolved）");
    }
    if lit.starts_with("未知の") {
        return Some("未知の <何か>: …（Reason::UnknownColorKey / UnknownTheme ほか）");
    }
    let (action, rest) = lit.split_once(" には ")?;
    let (key, _) = rest.split_once(" が必要")?;
    (is_word(action) && is_word(key))
        .then_some("<操作> には <鍵> が必要（Reason::ActionNeeds / ActionNeedsHint）")
}

/// 候補・案内を値で受けるバリアント（呼び手の渡す文字列リテラルを見る）
const VALUE_TAKING: [&str; 5] = [
    "Reason::UnknownValue",
    "Reason::UnknownActionPlain",
    "Reason::UnknownActionListed",
    "Reason::UnfamiliarAction",
    "Reason::ActionNeeds",
];

#[test]
fn action_で選ぶツールの断りの旧形の日本語が_dispatch_に残っていない() {
    let mut failures = Vec::new();
    for (line, code) in dispatch_code_lines() {
        for lit in line_literals(code) {
            if let Some(family) = old_form(lit) {
                failures.push(format!(
                    "{DISPATCH_PATH}:{line}: 旧形の日本語の固定文（{family} へ寄せる）: {}",
                    code.trim()
                ));
            }
        }
    }
    report_all(failures);
}

#[test]
fn action_で選ぶツールの断りへ渡す候補と案内に日本語が無い() {
    let lines = dispatch_code_lines();
    let mut failures = Vec::new();
    let mut sites = 0;
    for (k, (line, code)) in lines.iter().enumerate() {
        if !VALUE_TAKING.iter().any(|v| code.contains(v)) {
            continue;
        }
        sites += 1;
        // 組み立ての終わり（`}` を含む行）までの文字列リテラルを見る
        for (at, part) in lines[k..lines.len().min(k + 8)].iter() {
            for lit in line_literals(part) {
                if let Some(c) = has_japanese(lit) {
                    failures.push(format!(
                        "{DISPATCH_PATH}:{at}: 断りへ渡す値に日本語（{c}）がある = 英語表示にも \
                         そのまま出る（{line} 行目からの組み立て）: {}",
                        part.trim()
                    ));
                }
            }
            if part.contains('}') {
                break;
            }
        }
    }
    // 番犬自身の検査: 呼び手を拾えている（#2365 の続きの時点で 57 か所。
    // 大きく減ったら走査の前提 = 1 行に `Reason::<名前>` が書かれる形が崩れている）
    assert!(
        sites >= 50,
        "値で受けるバリアントの呼び手が {sites} 件しか見えない"
    );
    report_all(failures);
}

/// 番犬自身の検査: 旧形の判定は旧文の 5 つの形に当たり、似た別の文には当たらない
#[test]
fn action_で選ぶツールの断りの旧形の判定の境界() {
    for lit in [
        "不明な action: {other:?}（status / set のいずれか）",
        "不明な lid_power_condition: {pc:?}（{}）",
        "設定の保存に失敗: {e}",
        "データディレクトリを解決できない",
        "未知の色キー: {k}",
        "show には task_id が必要",
        "{action} には id が必要",
        "set には mode が必要（dark / light / プリセット名）",
    ] {
        assert!(old_form(lit).is_some(), "{lit}");
    }
    for lit in [
        "不明なツール: {name}",
        "read には pane または session_id が必要",
        "データディレクトリが決まらない（HOME が無い）",
        "unknown action: {got:?} (one of {allowed})",
    ] {
        assert!(old_form(lit).is_none(), "{lit}");
    }
    assert_eq!(
        line_literals(r#"x("a \"b\" c", "d") "tail"#),
        vec![r#"a \"b\" c"#, "d", "tail"]
    );
}

/// 境界: 呼び手が埋める値（打たれた綴り・プリセットの一覧・OS の文）は日英どちらでも同じ形で埋まる
#[test]
fn action_で選ぶツールの断りの値の埋め込みの境界() {
    // 打たれた綴りは Debug の引用（中の引用符はエスケープ）で、日英とも同じ表記
    let quoted = Reason::UnknownValue {
        key: "action",
        got: "a\"b".into(),
        allowed: "x / y".into(),
    };
    assert_eq!(
        quoted.text_in(Lang::Ja),
        "不明な action: \"a\\\"b\"（x / y のいずれか）"
    );
    assert_eq!(
        quoted.text_in(Lang::En),
        "unknown action: \"a\\\"b\" (one of x / y)"
    );
    // プリセットが無いときは「なし」/ none、複数は ` / ` で並べる
    let none = Reason::UnknownTheme {
        got: "x".into(),
        presets: Vec::new(),
    };
    assert!(none.text_in(Lang::Ja).ends_with("プリセット: なし）"));
    assert!(none.text_in(Lang::En).ends_with("presets: none)"));
    let many = Reason::UnknownTheme {
        got: "x".into(),
        presets: vec!["a".into(), "b".into(), "c".into()],
    };
    assert!(many.text_in(Lang::Ja).ends_with("プリセット: a / b / c）"));
    assert!(many.text_in(Lang::En).ends_with("presets: a / b / c)"));
    // 書き出しの口の文は日英とも書き換えずに後ろへ付ける（空でも前置きだけ）
    for e in ["", "Read-only file system (os error 30)"] {
        let r = Reason::SaveSettingsFailed(e.into());
        assert_eq!(r.text_in(Lang::Ja), format!("設定の保存に失敗: {e}"));
        assert_eq!(
            r.text_in(Lang::En),
            format!("failed to save the settings: {e}")
        );
    }
    // 候補の意味（lid_power_condition）は言語ごとに引き、日本語はカタログの説明と同じ文
    assert_eq!(
        tako_control::sleep_guard::PowerCondition::values_hint(),
        tako_control::sleep_guard::PowerCondition::values_hint_in(Lang::Ja)
    );
    assert!(
        has_japanese(&tako_control::sleep_guard::PowerCondition::values_hint_in(
            Lang::En
        ))
        .is_none()
    );
}

// ---- #2365 の続き: MCP の前段・CLI の前段の断り（前段のソースを読む番犬） ----
//
// 前段の断りは `Reason` の表（上）で日英を見る。表では見えないのは、前段に日本語の固定文
// （`return Err("… を指定する".into())` / `format!("… が不正: {x}")`）を**書き足す**こと = 英語表示でも
// その 1 か所だけ日本語で返る。前段のソースを読んで `file:line` で名指す。
// `unreachable!` / `expect` の文（到達しない内部の印）とコメントは見ない

const MCP_REQUEST_SRC: &str = include_str!("../src/mcp/request.rs");
const MCP_REQUEST_PATH: &str = "crates/tako-control/src/mcp/request.rs";
const MCP_MOD_SRC: &str = include_str!("../src/mcp/mod.rs");
const MCP_MOD_PATH: &str = "crates/tako-control/src/mcp/mod.rs";
const MCP_HTTP_SRC: &str = include_str!("../src/mcp/http.rs");
const MCP_HTTP_PATH: &str = "crates/tako-control/src/mcp/http.rs";
const CLI_MAIN_SRC: &str = include_str!("../../tako-cli/src/main.rs");
const CLI_MAIN_PATH: &str = "crates/tako-cli/src/main.rs";

/// 前段の走査の範囲: (path, ソース, 関数の頭)。関数の頭が `None` ならファイルの本体（単体テストの前まで）
const FRONT_SCOPES: &[(&str, &str, Option<&str>)] = &[
    // MCP: 引数の組み立て（ファイル全体）と JSON-RPC の口
    (MCP_REQUEST_PATH, MCP_REQUEST_SRC, None),
    (
        MCP_MOD_PATH,
        MCP_MOD_SRC,
        Some("pub fn handle_message(message: &Value"),
    ),
    (
        MCP_MOD_PATH,
        MCP_MOD_SRC,
        Some("fn call_tool(params: &Value"),
    ),
    (MCP_HTTP_PATH, MCP_HTTP_SRC, Some("fn handle_http(")),
    // CLI: build_request とそれが（関数として）呼ぶ補助
    (
        CLI_MAIN_PATH,
        CLI_MAIN_SRC,
        Some("fn build_request(command: &Command)"),
    ),
    (
        CLI_MAIN_PATH,
        CLI_MAIN_SRC,
        Some("    fn request(&self) -> Result<Request, String> {"),
    ),
    (CLI_MAIN_PATH, CLI_MAIN_SRC, Some("fn parse_position(")),
    (
        CLI_MAIN_PATH,
        CLI_MAIN_SRC,
        Some("fn parse_line_col_range("),
    ),
    (CLI_MAIN_PATH, CLI_MAIN_SRC, Some("fn parse_direction(")),
    (
        CLI_MAIN_PATH,
        CLI_MAIN_SRC,
        Some("fn paste_guard_check_text("),
    ),
    (CLI_MAIN_PATH, CLI_MAIN_SRC, Some("fn read_mod_report(")),
    (CLI_MAIN_PATH, CLI_MAIN_SRC, Some("fn run_config_env(")),
    (CLI_MAIN_PATH, CLI_MAIN_SRC, Some("fn build_criteria_json(")),
    (CLI_MAIN_PATH, CLI_MAIN_SRC, Some("fn file_copy_requests(")),
    (CLI_MAIN_PATH, CLI_MAIN_SRC, Some("fn file_copy_cli(")),
    (CLI_MAIN_PATH, CLI_MAIN_SRC, Some("fn file_many_cli(")),
    (CLI_MAIN_PATH, CLI_MAIN_SRC, Some("fn target_pane(")),
];

/// `src` の走査範囲の行（1 始まりの行番号つき）。関数の頭が見つからなければ `None`
/// （走査の空振り = 改名で番犬が黙って何も見なくなるのを止める）
fn front_scope_lines<'a>(src: &'a str, head: Option<&str>) -> Option<Vec<(usize, &'a str)>> {
    let lines: Vec<&str> = src.lines().collect();
    // 単体テストの mod の始まり（`#[cfg(test)] use …` の 1 行だけの印は始まりではない）
    let tests = (0..lines.len())
        .find(|&i| {
            lines[i].starts_with("#[cfg(test)]")
                && lines
                    .get(i + 1)
                    .is_some_and(|next| next.starts_with("mod "))
        })
        .unwrap_or(lines.len());
    let (start, end) = match head {
        None => (0, tests),
        Some(head) => {
            let start = lines[..tests].iter().position(|l| l.contains(head))?;
            // 関数の終わり = 頭と同じ字下げの `}` の行
            let indent = &lines[start][..lines[start].len() - lines[start].trim_start().len()];
            let close = format!("{indent}}}");
            let len = lines[start..tests].iter().position(|l| *l == close)?;
            (start, start + len + 1)
        }
    };
    Some(
        lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, l)| (start + i + 1, *l))
            .collect(),
    )
}

/// 走査範囲の中で、断りに書き足された日本語の固定文の行（`unreachable!` / `expect` の文と
/// コメントは外す。`unreachable!(` が行をまたぐときは閉じ括弧まで外す）
fn front_japanese(lines: &[(usize, &str)]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut internal_depth = 0i32;
    for (line, code) in lines {
        let trimmed = code.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }
        if internal_depth == 0 && (code.contains("unreachable!(") || code.contains(".expect(")) {
            internal_depth = 1;
            let from = code
                .find("unreachable!(")
                .or_else(|| code.find(".expect("))
                .unwrap_or(0);
            // 閉じ括弧がその行に揃っていれば 0 へ戻る（呼び手の括弧も閉じる行は負にしない）
            let tail = &code[from..];
            internal_depth = (internal_depth + tail.matches('(').count() as i32
                - tail.matches(')').count() as i32
                - 1)
            .max(0);
            continue;
        }
        if internal_depth > 0 {
            internal_depth += code.matches('(').count() as i32 - code.matches(')').count() as i32;
            internal_depth = internal_depth.max(0);
            continue;
        }
        for lit in line_literals(code) {
            if let Some(c) = has_japanese(lit) {
                out.push((*line, format!("（{c}）{}", code.trim())));
                break;
            }
        }
    }
    out
}

#[test]
fn mcp_と_cli_の前段の断りに日本語の固定文が無い() {
    let mut failures = Vec::new();
    for (path, src, head) in FRONT_SCOPES {
        let Some(lines) = front_scope_lines(src, *head) else {
            failures.push(format!(
                "{path}: 走査の範囲 `{}` が見つからない（関数の改名で番犬が空振りする。\
                 FRONT_SCOPES を直す）",
                head.unwrap_or("<ファイル全体>")
            ));
            continue;
        };
        for (line, text) in front_japanese(&lines) {
            failures.push(format!(
                "{path}:{line}: 前段の断りに日本語の固定文がある = 英語表示にもそのまま出る \
                 （tako_control::Reason へバリアントを足し、日英の対を text_in に書く）: {text}"
            ));
        }
    }
    report_all(failures);
}

/// 番犬自身の検査: 植えた日本語の断りを名指し、コメント・`unreachable!`（行をまたぐ形も）・
/// `expect` は見逃す。範囲の終わりは頭と同じ字下げの `}` で、隣の関数へはみ出さない
#[test]
fn 前段の番犬は植えた日本語を名指しし内部の印とコメントは見逃す() {
    let src = r#"fn front(x: u8) -> Result<u8, String> {
    // 断りは日英（コメントの日本語は見ない）
    if x == 0 {
        return Err("x を指定する".into());
    }
    let y = Some(x).expect("1 件");
    match x {
        1 => unreachable!(
            "到達しない（内部の印）"
        ),
        2 => return Err(format!("{x} が不正: 範囲の外")),
        _ => {}
    }
    Ok(y)
}
fn next() {
    let _ = "隣の関数（範囲の外）";
}
"#;
    let lines = front_scope_lines(src, Some("fn front(")).expect("頭を引ける");
    let found: Vec<usize> = front_japanese(&lines).into_iter().map(|(l, _)| l).collect();
    assert_eq!(found, vec![4, 11], "植えた 2 か所だけを名指す");
    assert!(front_scope_lines(src, Some("fn absent(")).is_none());
    // ファイル全体（頭なし）は隣の関数も見る
    let whole = front_scope_lines(src, None).expect("全体");
    let found: Vec<usize> = front_japanese(&whole).into_iter().map(|(l, _)| l).collect();
    assert_eq!(found, vec![4, 11, 17]);
}

// ---- #2365 の続き: GUI へ繋ぐ口（接続まわり）と置き場の断り（ソースを読む番犬と tako-core の対） ----
//
// 接続まわり（`tako-cli` の ipc.rs・起動失敗の包み）と置き場（acceptance_gates / task_checkpoints /
// user_tasks と tako-core の `TaskStore`）の断りは `Reason` の表（上）と下の tako-core の対で日英を見る。
// 前段と同じく、表では見えない「日本語の固定文の書き足し」をソースを読んで `file:line` で名指す。
// 置き場のファイルの中身になる文（受け入れ条件の evidence・配送の理由）と master へ運ぶ返答の本文は
// 断りではないので走査の範囲に入れない（関数の頭で範囲を取る）

const IPC_SRC: &str = include_str!("../../tako-cli/src/ipc.rs");
const IPC_PATH: &str = "crates/tako-cli/src/ipc.rs";
const GATES_SRC: &str = include_str!("../src/acceptance_gates.rs");
const GATES_PATH: &str = "crates/tako-control/src/acceptance_gates.rs";
const CHECKPOINTS_SRC: &str = include_str!("../src/task_checkpoints.rs");
const CHECKPOINTS_PATH: &str = "crates/tako-control/src/task_checkpoints.rs";
const USER_TASKS_SRC: &str = include_str!("../src/user_tasks.rs");
const USER_TASKS_PATH: &str = "crates/tako-control/src/user_tasks.rs";
const CORE_USER_TASK_SRC: &str = include_str!("../../tako-core/src/user_task.rs");
const CORE_USER_TASK_PATH: &str = "crates/tako-core/src/user_task.rs";
const LOAD_ISSUE_SRC: &str = include_str!("../../tako-core/src/load_issue.rs");
const LOAD_ISSUE_PATH: &str = "crates/tako-core/src/load_issue.rs";

/// 接続まわり・置き場の走査の範囲
enum Span {
    /// 関数（または impl）の頭から、頭と同じ字下げの `}` まで（[`front_scope_lines`]）
    Item(&'static str),
    /// ファイルの本体（単体テストの前まで）
    Whole,
    /// ファイルの頭から、印を含む行の手前まで（ipc.rs は入れ子の単体テストを持つ `mod deadline` の前で切る）
    Until(&'static str),
}

const CLIENT_STORE_SCOPES: &[(&str, &str, Span)] = &[
    // 接続まわり: 接続情報の解決・接続・往復（unix / windows の両方）
    (IPC_PATH, IPC_SRC, Span::Until("    mod deadline {")),
    (
        CLI_MAIN_PATH,
        CLI_MAIN_SRC,
        Span::Item("fn launch_failure_message("),
    ),
    // 受け入れゲート（evidence を組む execute_command / check_pr_merged は置き場の中身なので外す）
    (
        GATES_PATH,
        GATES_SRC,
        Span::Item("impl AcceptanceGateStore {"),
    ),
    (
        GATES_PATH,
        GATES_SRC,
        Span::Item("fn store_path_or_refuse("),
    ),
    (GATES_PATH, GATES_SRC, Span::Item("fn gate_not_found(")),
    (GATES_PATH, GATES_SRC, Span::Item("pub fn parse_criteria(")),
    (
        GATES_PATH,
        GATES_SRC,
        Span::Item("pub fn set_gate_payload_at("),
    ),
    (
        GATES_PATH,
        GATES_SRC,
        Span::Item("pub fn set_gate_payload("),
    ),
    (
        GATES_PATH,
        GATES_SRC,
        Span::Item("pub fn show_gate_payload_at("),
    ),
    (
        GATES_PATH,
        GATES_SRC,
        Span::Item("pub fn show_gate_payload("),
    ),
    (GATES_PATH, GATES_SRC, Span::Item("pub fn parse_results(")),
    (
        GATES_PATH,
        GATES_SRC,
        Span::Item("pub fn record_results_payload_at("),
    ),
    (
        GATES_PATH,
        GATES_SRC,
        Span::Item("pub fn record_results_payload("),
    ),
    (
        GATES_PATH,
        GATES_SRC,
        Span::Item("pub fn execute_gate_check("),
    ),
    (
        GATES_PATH,
        GATES_SRC,
        Span::Item("pub fn execute_gate_check_at("),
    ),
    // チェックポイント（ファイル全体）
    (CHECKPOINTS_PATH, CHECKPOINTS_SRC, Span::Whole),
    // ユーザータスク（配送の理由 `UNRESOLVED_ORIGIN_REASON` と返答の本文は外す）
    (
        USER_TASKS_PATH,
        USER_TASKS_SRC,
        Span::Item("pub fn load_from("),
    ),
    (USER_TASKS_PATH, USER_TASKS_SRC, Span::Item("pub fn load(")),
    (
        USER_TASKS_PATH,
        USER_TASKS_SRC,
        Span::Item("fn store_path_or_refuse("),
    ),
    (
        USER_TASKS_PATH,
        USER_TASKS_SRC,
        Span::Item("pub fn mutate_at<"),
    ),
    (
        USER_TASKS_PATH,
        USER_TASKS_SRC,
        Span::Item("pub fn mutate<"),
    ),
    (
        USER_TASKS_PATH,
        USER_TASKS_SRC,
        Span::Item("pub fn parse_kind("),
    ),
    (
        USER_TASKS_PATH,
        USER_TASKS_SRC,
        Span::Item("pub fn parse_status("),
    ),
    (
        USER_TASKS_PATH,
        USER_TASKS_SRC,
        Span::Item("pub fn parse_decision("),
    ),
    (
        USER_TASKS_PATH,
        USER_TASKS_SRC,
        Span::Item("pub fn parse_via("),
    ),
    (USER_TASKS_PATH, USER_TASKS_SRC, Span::Item("fn unknown<")),
    (
        USER_TASKS_PATH,
        USER_TASKS_SRC,
        Span::Item("pub fn show_at("),
    ),
    (
        USER_TASKS_PATH,
        USER_TASKS_SRC,
        Span::Item("pub fn respond_at("),
    ),
    // tako-core の置き場の操作（`expect` の内部の印は見ない）
    (
        CORE_USER_TASK_PATH,
        CORE_USER_TASK_SRC,
        Span::Item("impl TaskStore {"),
    ),
    (
        CORE_USER_TASK_PATH,
        CORE_USER_TASK_SRC,
        Span::Item("fn not_found("),
    ),
];

/// 走査の範囲の行。範囲が見つからなければ `None`（改名で番犬が空振りするのを止める）
fn client_store_lines<'a>(src: &'a str, span: &Span) -> Option<Vec<(usize, &'a str)>> {
    match span {
        Span::Item(head) => front_scope_lines(src, Some(head)),
        Span::Whole => front_scope_lines(src, None),
        Span::Until(mark) => {
            let end = src.lines().position(|l| l.contains(mark))?;
            Some(
                src.lines()
                    .take(end)
                    .enumerate()
                    .map(|(i, l)| (i + 1, l))
                    .collect(),
            )
        }
    }
}

fn span_name(span: &Span) -> String {
    match span {
        Span::Item(head) => format!("`{head}`"),
        Span::Whole => "<ファイル全体>".into(),
        Span::Until(mark) => format!("<頭から `{}` の手前まで>", mark.trim()),
    }
}

#[test]
fn 接続まわりと置き場の断りに日本語の固定文が無い() {
    let mut failures = Vec::new();
    for (path, src, span) in CLIENT_STORE_SCOPES {
        let Some(lines) = client_store_lines(src, span) else {
            failures.push(format!(
                "{path}: 走査の範囲 {} が見つからない（改名で番犬が空振りする。\
                 CLIENT_STORE_SCOPES を直す）",
                span_name(span)
            ));
            continue;
        };
        for (line, text) in front_japanese(&lines) {
            failures.push(format!(
                "{path}:{line}: 接続まわり・置き場の断りに日本語の固定文がある = 英語表示にもそのまま出る \
                 （tako_control::Reason へバリアントを足し、日英の対を text_in に書く）: {text}"
            ));
        }
    }
    report_all(failures);
}

/// 番犬自身の検査: 印の手前で切る範囲は印の行と後ろを見ず、印が無ければ空振りとして `None`
#[test]
fn 接続まわりの範囲は印の手前で切る() {
    let src = "fn a() -> String {\n    \"接続できない\".into()\n}\n    mod deadline {\n        let _ = \"テストの文\";\n";
    let lines = client_store_lines(src, &Span::Until("    mod deadline {")).expect("印がある");
    let found: Vec<usize> = front_japanese(&lines).into_iter().map(|(l, _)| l).collect();
    assert_eq!(found, vec![2], "印より前の植えた 1 か所だけを名指す");
    assert!(client_store_lines(src, &Span::Until("mod absent {")).is_none());
}

/// tako-core の置き場の断り（`TaskRefusal`）の通し番号（網羅 match）
fn task_refusal_index(r: &TaskRefusal) -> usize {
    match r {
        TaskRefusal::EmptyTitle => 0,
        TaskRefusal::NothingToUpdate => 1,
        TaskRefusal::ClearTitle => 2,
        TaskRefusal::NotFound(_) => 3,
    }
}
const TASK_REFUSALS: usize = 4;

/// 見本と #2365 より前の固定文（`TaskStore` の操作が書いていた文）
fn task_refusal_table() -> Vec<(TaskRefusal, &'static str)> {
    vec![
        (
            TaskRefusal::EmptyTitle,
            "タイトルが空（一覧で選べないタスクは作れない）",
        ),
        (
            TaskRefusal::NothingToUpdate,
            "更新する項目が 1 つも指定されていない",
        ),
        (TaskRefusal::ClearTitle, "タイトルを空にはできない"),
        (
            TaskRefusal::NotFound("u-9".into()),
            "タスクが見つからない: u-9",
        ),
    ]
}

/// YAML の誤りの種類の通し番号（網羅 match）
fn yaml_problem_index(p: &YamlProblem) -> usize {
    match p {
        YamlProblem::Syntax => 0,
        YamlProblem::Empty => 1,
        YamlProblem::Shape => 2,
    }
}
const YAML_PROBLEMS: usize = 3;

/// 見本と #2365 より前の固定文（`load_issue::yaml_reason` の種類の文）
const YAML_PROBLEM_TABLE: [(YamlProblem, &str); YAML_PROBLEMS] = [
    (YamlProblem::Syntax, "YAML の構文が壊れている"),
    (YamlProblem::Empty, "YAML が空"),
    (YamlProblem::Shape, "YAML の形が期待と違う"),
];

#[test]
fn 置き場の_core_の断りの見本は全バリアントを持ち日英を持ち日本語は2365より前とバイト一致() {
    let refusals: Vec<TaskRefusal> = task_refusal_table().into_iter().map(|(r, _)| r).collect();
    covers_all(&refusals, task_refusal_index, TASK_REFUSALS);
    let problems: Vec<YamlProblem> = YAML_PROBLEM_TABLE.iter().map(|(p, _)| *p).collect();
    covers_all(&problems, yaml_problem_index, YAML_PROBLEMS);
    for (r, old) in task_refusal_table() {
        let debug = format!("{r:?}");
        let at = |lang| {
            where_written(
                CORE_USER_TASK_SRC,
                CORE_USER_TASK_PATH,
                "impl TaskRefusal {",
                &debug,
                lang,
            )
        };
        assert_eq!(r.text_in(Lang::Ja), old, "{debug}（{}）", at(Lang::Ja));
        bilingual_named(at, &debug, &r.text_in(Lang::Ja), &r.text_in(Lang::En));
        // 「見つからない」の読み（リモートの 404 寄せ）は見つからない系だけに日英で当たる
        for lang in [Lang::Ja, Lang::En] {
            assert_eq!(
                E::says_not_found(&r.text_in(lang)),
                matches!(r, TaskRefusal::NotFound(_)),
                "{debug} / {lang:?}"
            );
        }
    }
    for (p, old) in YAML_PROBLEM_TABLE {
        let debug = format!("{p:?}");
        let at = |lang| {
            where_written(
                LOAD_ISSUE_SRC,
                LOAD_ISSUE_PATH,
                "impl YamlProblem {",
                &debug,
                lang,
            )
        };
        assert_eq!(p.text_in(Lang::Ja), old, "{debug}（{}）", at(Lang::Ja));
        bilingual_named(at, &debug, p.text_in(Lang::Ja), p.text_in(Lang::En));
    }
}

/// 境界: パースの理由は位置の有無と A/B の素の文言でも包みと同じ言語にそろい、日本語は
/// persist.log へ流れる `yaml_error_reason`（日本語固定）と同じ文になる
#[test]
fn 置き場のパースの理由は位置の有無と素の文言でも包みと同じ言語() {
    let text = "gates: [\n  - task_id: a\n";
    let e = serde_yaml::from_str::<serde_yaml::Value>(text).expect_err("壊れている");
    let reason = YamlReason::located(&e, text);
    assert_eq!(
        reason.text_in(Lang::Ja),
        tako_core::load_issue::yaml_error_reason(&e, text),
        "日本語は persist.log へ流れる文と同じ"
    );
    let forms = [
        reason,
        YamlReason::Located {
            kind: YamlProblem::Empty,
            at: None,
        },
        YamlReason::Raw("invalid type: integer `5`, expected a sequence".into()),
    ];
    for form in forms {
        let wrapped = |lang| {
            Reason::StoreParseFailed {
                file: "user-tasks.yaml",
                reason: form.clone(),
            }
            .text_in(lang)
        };
        let en = wrapped(Lang::En);
        assert!(has_japanese(&en).is_none(), "{form:?}: {en}");
        assert!(en.starts_with("failed to parse user-tasks.yaml: "), "{en}");
        assert!(wrapped(Lang::Ja).starts_with("user-tasks.yaml のパースに失敗: "));
    }
    assert_eq!(
        YamlReason::Located {
            kind: YamlProblem::Shape,
            at: Some((2, 7)),
        }
        .text_in(Lang::En),
        "the YAML does not have the expected shape (line 2, column 7)"
    );
}

/// 境界: 起動失敗の包みは古い TAKO_PANE_ID が無ければ unset を案内せず、原因の文を言語ごとに
/// そのまま埋める（原因が改行を含んでも包みは 1 回だけ）
#[test]
fn 起動失敗の包みの境界() {
    for lang in [Lang::Ja, Lang::En] {
        let cause = Reason::OutsideTako.text_in(lang);
        let without = Reason::LaunchPaneUnavailable {
            cause: cause.clone(),
            cmd_hint: "tako master".into(),
            stale_pane: None,
        }
        .text_in(lang);
        assert!(!without.contains("unset TAKO_PANE_ID"), "{without}");
        assert!(without.contains(&cause), "{without}");
        assert!(without.contains("`tako master`"), "{without}");
        assert_eq!(without.lines().count(), 2, "{without}");
        let nested = Reason::LaunchPaneUnavailable {
            cause: "a\nb".into(),
            cmd_hint: "tako solo".into(),
            stale_pane: Some(7),
        }
        .text_in(lang);
        assert_eq!(nested.lines().count(), 4, "{nested}");
        assert!(nested.contains("TAKO_PANE_ID=7"), "{nested}");
    }
}
