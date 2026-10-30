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
//! #2365 の続きで、orchestrator の設定ファイル（projects / accounts / profiles）の読み書きとエージェント名の
//! 検証の断り（`orchestrator::ConfigError`）にも広げた（末尾）。型の口の本体と、それを呼ぶ dispatch の
//! 関数のソースも読んで名指す。
//!
//! このファイルは**独立したテストバイナリ**で、言語グローバルを触るのは
//! `display_follows_the_display_language` の 1 本だけ（ほかは `text_in` で言語を引数に渡す）。

use tako_control::DispatchError as E;
use tako_control::{NoTargetPaneAt, Reason};
use tako_core::dialog::ConfirmBlock;
use tako_core::i18n::{self, Lang};
use tako_core::load_issue::{YamlProblem, YamlReason};
use tako_core::remote_shortcuts::ShortcutError;
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
        // #2365 の続き: dispatch の setup 系の断り（MCP の `tako_setup` の answers ほか）は型の口が
        // 表示言語で返す（上の CLI の `tako setup` の従来の口は日本語のまま）
        for (r, _) in setup_refusal_table() {
            assert_eq!(r.to_string(), r.text_in(lang), "{r:?} / {lang:?}");
        }
        for (e, _) in agent_mcp_table() {
            assert_eq!(e.to_string(), e.text_in(lang), "{e:?} / {lang:?}");
        }
        let answers: tako_control::setup::SetupAnswers = serde_json::from_str(
            r#"{"mod_ui": {"buttons": [{"kind": "slash", "value": "bash"}]}}"#,
        )
        .expect("answers の形");
        let typed = answers.validate_typed().expect_err("許可リストの外");
        assert_eq!(typed.to_string(), typed.text_in(lang), "{lang:?}");
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
        // #2365 の続き: remote daemon が tako app へ繋ぐ口（remote.rs の AppIpcClient）
        Reason::AppCannotConnect { .. } => 179,
        Reason::AppRequestEncodeFailed(_) => 180,
        Reason::AppNoReply => 181,
        // #2365 の続き: 選択肢ダイアログへの応答・ダイアログ表示中の送信
        Reason::RespondHint { .. } => 182,
        Reason::ScreenCaptureFailed(_) => 183,
        Reason::KeySendFailed { .. } => 184,
        Reason::UnnumberedDialogBlocked(_) => 185,
        Reason::DialogVanishedWhileMoving => 186,
        Reason::CursorNotLanded(_) => 187,
        Reason::ChoiceLabelTruncated { .. } => 188,
        Reason::ChoiceEmpty(_) => 189,
        Reason::ChoiceOutOfRange { .. } => 190,
        Reason::ChoiceNoMatch { .. } => 191,
        Reason::ChoiceAmbiguous { .. } => 192,
        Reason::DialogBlocksSend { .. } => 193,
        // #2365 の続き: orchestrator
        Reason::NotMasterPane { .. } => 194,
        Reason::NotProfileOrProject { .. } => 195,
        Reason::CopyNameMissing => 196,
        Reason::CopyFromMissing => 197,
        Reason::AgentSettingsNeedAgent => 198,
        Reason::EnvEntryShape(_) => 199,
        Reason::CwdNotAbsolute(_) => 200,
        Reason::AccountNeedsConfigOrInherit => 201,
        Reason::AccountNotRegistered(_) => 202,
        Reason::RoleAmbiguous { .. } => 203,
        Reason::MasterPaneAmbiguous(_) => 204,
        Reason::ReportUnavailable(_) => 205,
        Reason::ProjectNotAllowed { .. } => 206,
        Reason::ProjectKeyNotFileName(_) => 207,
        Reason::HomeDirUnknown => 208,
        Reason::ActionExactlyOneOf { .. } => 209,
        Reason::NoHandoffMaterial { .. } => 210,
        Reason::SystemPromptDirFailed(_) => 211,
        Reason::SystemPromptWriteFailed(_) => 212,
        Reason::WorkerRegistryUnreadable(_) => 213,
        // #2365 の続き: 枠が日本語だった設定の保存・リモート閲覧のショートカット
        Reason::UpdateCardSaveFailed(_) => 214,
        Reason::WelcomeSaveFailed(_) => 215,
        Reason::SaveFailed(_) => 216,
        Reason::Shortcut(_) => 217,
        // #2365 の続き: Web ビュー・メニュー
        Reason::UrlEmpty => 218,
        Reason::WebShowNeedsId => 219,
        Reason::WebAlreadyDocked(_) => 220,
        Reason::WebNavigateNeedsTo => 221,
        Reason::MenuOpenCloseUnavailable => 222,
        Reason::MenuNameEmpty => 223,
        Reason::MenuNameAmbiguous { .. } => 224,
        Reason::MenuNotFound { .. } => 225,
        Reason::MenuUndefined => 226,
        Reason::MenuItemPathEmpty => 227,
        Reason::MenuItemAmbiguous { .. } => 228,
        Reason::MenuItemNotFound(_) => 229,
        // #2365 の続き: 個別の値の検証
        Reason::NetworkWithEnabled => 230,
        Reason::AllWithEnabled => 231,
        Reason::PositivePx(_) => 232,
        Reason::OneOfGiven { .. } => 233,
        Reason::PaneNeedsCheckText => 234,
        Reason::AtLeastOne(_) => 235,
        Reason::AutoCloseValue(_) => 236,
        Reason::RunConfigPathWithDefaults => 237,
        Reason::ReadNeedsPaneOrSession => 238,
        Reason::CwdUnavailable => 239,
        Reason::CwdMissing(_) => 240,
        Reason::TmuxResizeShape => 241,
        Reason::NoTmuxSessionOnPane(_) => 242,
        Reason::TmuxSessionNotFound { .. } => 243,
        // #2365 の続き: ジャンプ履歴・LSP・ワークスペース横断検索
        Reason::JumpNoEarlier => 244,
        Reason::JumpNoLater => 245,
        Reason::ChoiceOneBased => 246,
        Reason::ColumnNotCharBoundary { .. } => 247,
        Reason::ChoiceOutsideTargets { .. } => 248,
        Reason::ChoiceOutsideItems { .. } => 249,
        Reason::LimitAtLeastOneCandidate => 250,
        Reason::ItemNeedsResponseId => 251,
        Reason::LspSetOnlyWithOptions => 252,
        Reason::UnknownLspServer { .. } => 253,
        Reason::WorkspaceSearchUnavailable => 254,
        Reason::WorkspaceSearchNeedsQuery => 255,
        Reason::RelativePathNoBase(_) => 256,
        Reason::SearchRootsHomeOnly => 257,
        Reason::OpenNeedsItem => 258,
        Reason::NoShownSearchResults => 259,
        Reason::ItemOutsideResults { .. } => 260,
        // #2365 の続き: 変更履歴
        Reason::ChangelogFileChanged => 261,
        Reason::ChangelogNoRepo => 262,
        Reason::ChangelogNoPath => 263,
        Reason::DiffFileChanged => 264,
        Reason::DiffChangelogChanged(_) => 265,
        Reason::DiffChangelogReplaced => 266,
        // #2365 の続き: リモートフォルダ・config share・scrollback
        Reason::KeyNeeded(_) => 267,
        Reason::RemoteFolderNotOpen(_) => 268,
        Reason::NoPendingPush => 269,
        Reason::ConfigShareLinkNeedsTarget => 270,
        Reason::NoVesselForHistory(_) => 271,
        // #2365 の続き: コマンドカード・Code Runner
        Reason::CardsNeedGui => 272,
        Reason::NoCardOnPane => 273,
        Reason::ClipboardNeedsGui => 274,
        Reason::RunConfigNeedsPath => 275,
        Reason::SaveBeforeRunFailed { .. } => 276,
        Reason::FileUnreadable { .. } => 277,
        Reason::FileReadError { .. } => 278,
        // #2365 の続き: 先取りの答えが無かった断り・git・tmux・動画・ウィンドウ・引き継ぎ
        Reason::AgentCliUnchecked(_) => 279,
        Reason::GitConflictUnchecked => 280,
        Reason::TmuxSessionUnchecked => 281,
        Reason::NoConflict => 282,
        Reason::ServersWithSocket(_) => 283,
        Reason::NotVideoPane => 284,
        Reason::WindowGeometryInvalid(_) => 285,
        Reason::ProfileCwdBroken { .. } => 286,
        // #2365 の続き: ファイルツリー
        Reason::NotAbsolutePath => 287,
        Reason::FolderNotPinned(_) => 288,
        Reason::OneOfReceived { .. } => 289,
        Reason::TreeOnlyActiveTab { .. } => 290,
        Reason::FileTreeClosed => 291,
        Reason::TreePathWithKey => 292,
        Reason::TreeRowNotVisible(_) => 293,
        Reason::TreeNoSelection => 294,
        // #2365 の続き: supervisor・ledger・セッション再起動
        Reason::SupervisorModeValue => 295,
        Reason::SupervisorAction(_) => 296,
        Reason::LedgerPruneNeedsProject => 297,
        Reason::LedgerAction(_) => 298,
        Reason::RestartModeInvalid { .. } => 299,
        Reason::ResumeCommandUnbuildable { .. } => 300,
        Reason::ResumeFilesUnwritable(_) => 301,
        Reason::AgentTerminateFailed { .. } => 302,
    }
}
const REASON_VARIANTS: usize = 303;

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
        Reason::SaveSettingsFailed(IoCause::Other("Permission denied (os error 13)".into())),
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
        // #2365 の続き: remote daemon → app
        Reason::AppCannotConnect {
            socket: "/tmp/tako/control.sock".into(),
            error: "Connection refused (os error 61)".into(),
        },
        Reason::AppRequestEncodeFailed("key must be a string".into()),
        Reason::AppNoReply,
        // #2365 の続き: 選択肢ダイアログへの応答・ダイアログ表示中の送信
        Reason::RespondHint {
            count: 3,
            truncated: false,
        },
        Reason::ScreenCaptureFailed("tmux capture-pane failed".into()),
        Reason::KeySendFailed {
            key: "Enter".into(),
            error: "Broken pipe (os error 32)".into(),
        },
        Reason::UnnumberedDialogBlocked(ConfirmBlock::NoHighlight),
        Reason::DialogVanishedWhileMoving,
        Reason::CursorNotLanded("Yes".into()),
        Reason::ChoiceLabelTruncated {
            position: 2,
            label: "Opus (1M".into(),
            number: 2,
            labels: "1. Default / 2. Opus (1M".into(),
        },
        Reason::ChoiceEmpty("1. Yes / 2. No".into()),
        Reason::ChoiceOutOfRange {
            n: 9,
            count: 2,
            labels: "1. Yes / 2. No".into(),
        },
        Reason::ChoiceNoMatch {
            choice: "maybe".into(),
            truncated: false,
            labels: "1. Yes / 2. No".into(),
        },
        Reason::ChoiceAmbiguous {
            choice: "o".into(),
            hits: 2,
            labels: "1. Yes / 2. No".into(),
        },
        Reason::DialogBlocksSend {
            kind: "usage_limit",
            options: "1. Stop / 2. Switch".into(),
            pane: Some(5),
        },
        // #2365 の続き: orchestrator
        Reason::NotMasterPane {
            pane: 7,
            role: Some("worker:web".into()),
        },
        Reason::NotProfileOrProject {
            name: "nope".into(),
            profiles: vec!["default".into(), "fable".into()],
            projects: vec!["web".into()],
        },
        Reason::CopyNameMissing,
        Reason::CopyFromMissing,
        Reason::AgentSettingsNeedAgent,
        Reason::EnvEntryShape("NOEQ".into()),
        Reason::CwdNotAbsolute("rel/dir".into()),
        Reason::AccountNeedsConfigOrInherit,
        Reason::AccountNotRegistered("work".into()),
        Reason::RoleAmbiguous {
            role: "master:default".into(),
            panes: vec![3, 5],
        },
        Reason::MasterPaneAmbiguous(vec![3, 5]),
        Reason::ReportUnavailable(42),
        Reason::ProjectNotAllowed {
            project: "web".into(),
            allowed: vec!["api".into(), "docs".into()],
        },
        Reason::ProjectKeyNotFileName("a/b".into()),
        Reason::HomeDirUnknown,
        Reason::ActionExactlyOneOf {
            action: "show",
            a: "project",
            b: "profile",
        },
        Reason::NoHandoffMaterial {
            projects_dir: "/tmp/h/projects".into(),
            memo_path: "/tmp/h/default.md".into(),
        },
        Reason::SystemPromptDirFailed("Permission denied (os error 13)".into()),
        Reason::SystemPromptWriteFailed("No space left on device (os error 28)".into()),
        Reason::WorkerRegistryUnreadable("expected value at line 1 column 1".into()),
        // #2365 の続き: 枠が日本語だった設定の保存・リモート閲覧のショートカット
        Reason::UpdateCardSaveFailed(IoCause::Other("Read-only file system (os error 30)".into())),
        Reason::WelcomeSaveFailed(IoCause::DataDirUnresolved),
        Reason::SaveFailed("Permission denied (os error 13)".into()),
        Reason::Shortcut(ShortcutError::NotAbsolute),
        // #2365 の続き: Web ビュー・メニュー
        Reason::UrlEmpty,
        Reason::WebShowNeedsId,
        Reason::WebAlreadyDocked(2),
        Reason::WebNavigateNeedsTo,
        Reason::MenuOpenCloseUnavailable,
        Reason::MenuNameEmpty,
        Reason::MenuNameAmbiguous {
            query: "e".into(),
            candidates: vec!["Edit".into(), "View".into()],
        },
        Reason::MenuNotFound {
            query: "Nope".into(),
            candidates: vec!["File".into(), "Edit".into()],
        },
        Reason::MenuUndefined,
        Reason::MenuItemPathEmpty,
        Reason::MenuItemAmbiguous {
            path: "tab".into(),
            candidates: vec!["File/New Tab".into(), "File/Close Tab".into()],
        },
        Reason::MenuItemNotFound("File/Nope".into()),
        // #2365 の続き: 個別の値の検証
        Reason::NetworkWithEnabled,
        Reason::AllWithEnabled,
        Reason::PositivePx("width"),
        Reason::OneOfGiven {
            key: "mode",
            allowed: "confirm / warn".into(),
            got: "loud".into(),
        },
        Reason::PaneNeedsCheckText,
        Reason::AtLeastOne("max_mb"),
        Reason::AutoCloseValue("sometimes".into()),
        Reason::RunConfigPathWithDefaults,
        Reason::ReadNeedsPaneOrSession,
        Reason::CwdUnavailable,
        Reason::CwdMissing("/tmp/gone".into()),
        Reason::TmuxResizeShape,
        Reason::NoTmuxSessionOnPane(4),
        Reason::TmuxSessionNotFound {
            session: "work".into(),
            socket: None,
        },
        // #2365 の続き: ジャンプ履歴・LSP・ワークスペース横断検索
        Reason::JumpNoEarlier,
        Reason::JumpNoLater,
        Reason::ChoiceOneBased,
        Reason::ColumnNotCharBoundary {
            column: 3,
            line: 2,
            len: 7,
        },
        Reason::ChoiceOutsideTargets {
            choice: 4,
            count: 2,
        },
        Reason::ChoiceOutsideItems {
            choice: 9,
            total: 5,
        },
        Reason::LimitAtLeastOneCandidate,
        Reason::ItemNeedsResponseId,
        Reason::LspSetOnlyWithOptions,
        Reason::UnknownLspServer {
            name: "nope".into(),
            known: "rust-analyzer / pyright".into(),
        },
        Reason::WorkspaceSearchUnavailable,
        Reason::WorkspaceSearchNeedsQuery,
        Reason::RelativePathNoBase("src".into()),
        Reason::SearchRootsHomeOnly,
        Reason::OpenNeedsItem,
        Reason::NoShownSearchResults,
        Reason::ItemOutsideResults { item: 8, total: 3 },
        // #2365 の続き: 変更履歴
        Reason::ChangelogFileChanged,
        Reason::ChangelogNoRepo,
        Reason::ChangelogNoPath,
        Reason::DiffFileChanged,
        Reason::DiffChangelogChanged("unknown commit abc123".into()),
        Reason::DiffChangelogReplaced,
        // #2365 の続き: リモートフォルダ・config share・scrollback
        Reason::KeyNeeded("host"),
        Reason::RemoteFolderNotOpen(Some("devbox:/srv/app".into())),
        Reason::NoPendingPush,
        Reason::ConfigShareLinkNeedsTarget,
        Reason::NoVesselForHistory(9),
        // #2365 の続き: コマンドカード・Code Runner
        Reason::CardsNeedGui,
        Reason::NoCardOnPane,
        Reason::ClipboardNeedsGui,
        Reason::RunConfigNeedsPath,
        Reason::SaveBeforeRunFailed {
            pane: 6,
            error: "Read-only file system (os error 30)".into(),
        },
        Reason::FileUnreadable {
            path: "/tmp/x.py".into(),
            error: "No such file or directory (os error 2)".into(),
        },
        Reason::FileReadError {
            path: "/tmp/x.py".into(),
            error: "Is a directory (os error 21)".into(),
        },
        // #2365 の続き: 先取りの答えが無かった断り・git・tmux・動画・ウィンドウ・引き継ぎ
        Reason::AgentCliUnchecked("codex"),
        Reason::GitConflictUnchecked,
        Reason::TmuxSessionUnchecked,
        Reason::NoConflict,
        Reason::ServersWithSocket("tako-x".into()),
        Reason::NotVideoPane,
        Reason::WindowGeometryInvalid("x is not a number".into()),
        Reason::ProfileCwdBroken {
            error: "cwd does not exist: /tmp/gone".into(),
            profile: "default".into(),
        },
        // #2365 の続き: ファイルツリー
        Reason::NotAbsolutePath,
        Reason::FolderNotPinned("/tmp/a".into()),
        Reason::OneOfReceived {
            key: "action",
            allowed: "add / remove".into(),
            got: "bogus".into(),
        },
        Reason::TreeOnlyActiveTab { active: 1, tab: 2 },
        Reason::FileTreeClosed,
        Reason::TreePathWithKey,
        Reason::TreeRowNotVisible("/tmp/a/b.rs".into()),
        Reason::TreeNoSelection,
        // #2365 の続き: supervisor・ledger・セッション再起動
        Reason::SupervisorModeValue,
        Reason::SupervisorAction("bogus".into()),
        Reason::LedgerPruneNeedsProject,
        Reason::LedgerAction("bogus".into()),
        Reason::RestartModeInvalid {
            raw: "bogus".into(),
            values: "harness | handoff".into(),
        },
        Reason::ResumeCommandUnbuildable {
            pane: 5,
            error: Some("no conversation".into()),
        },
        Reason::ResumeFilesUnwritable("Permission denied (os error 13)".into()),
        Reason::AgentTerminateFailed {
            pid: 4321,
            error: "Operation not permitted (os error 1)".into(),
        },
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
    // #2365 の続き: remote.rs の AppIpcClient（日本語の文言が ipc.rs と違っていた 3 つ）
    "tako app へ接続できない (/tmp/tako/control.sock): Connection refused (os error 61)",
    "リクエストの構築に失敗: key must be a string",
    "tako app から応答が返らなかった",
    // #2365 の続き: dispatch の `respond_via` / `resolve_choice_index` / `dialog_send_refusal`
    "応答するには choice に番号（1-3）かラベルの一部を渡す",
    "画面の取得に失敗: tmux capture-pane failed",
    "キー Enter の送信に失敗: Broken pipe (os error 32)",
    "番号なしダイアログで選択カーソルが画面に無くハイライト位置を特定できない（Enter は送らない）\
     （手動で応答してください）",
    "カーソル移動中にダイアログが消えた（応答は送っていません）",
    "選択肢「Yes」へカーソルを移動できなかったため応答を中止した（Enter は送っていません）",
    "選択肢 2 のラベルは画面上で切り詰められているか説明との境目が読めない（`Opus (1M`）ため、\
     ラベル指定では確定できない。番号（--choice 2）で指定する。選択肢: 1. Default / 2. Opus (1M",
    "choice が空。選択肢: 1. Yes / 2. No",
    "choice 9 は範囲外（1-2）。選択肢: 1. Yes / 2. No",
    "「maybe」に一致する選択肢が無い。選択肢: 1. Yes / 2. No",
    "「o」が複数の選択肢に一致する（2件）。番号で指定する。選択肢: 1. Yes / 2. No",
    "選択肢ダイアログ（usage_limit）が表示中のため送信を中止した。\
     入力欄が奪われているので、テキストや Enter はダイアログのキー操作として食われる\
     （数字なら選択が確定してしまう）。選択肢: 1. Stop / 2. Switch。\
     応答は `tako orchestrator respond --pane 5 --choice <番号|ラベル>`\
     （choice を省略すると構造だけ確認できる）",
    // #2365 の続き: dispatch の orchestrator（adopt・profiles・accounts・role の解決・report・spawn・引き継ぎ）
    "ペイン 7 は master ではない（role=worker:web）。adopt は master のペインにだけ効く",
    "'nope' はプロファイルでもプロジェクトでもない。\n\
     プロファイル: default / fable\n\
     プロジェクト: web\n\
     プロジェクトを先に登録するには: tako orchestrator projects add --key nope --cwd <path>",
    "name（複製先）を指定する",
    "from（複製元）を指定する",
    "agent_* 系の設定には agent（対象エージェント名）を指定する",
    "env の形式が不正（KEY=VALUE が必要）: NOEQ",
    "cwd は絶対パスか `~/` から始まる形で指定する（相対パスは起動時に解決できない）: rel/dir",
    "config_dir か inherit のどちらかを指定する\
     （別 config dir なら config_dir、既定の資格情報をそのまま使うなら inherit=true）",
    "アカウント 'work' は登録されていない",
    "role 'master:default' が複数ペインに存在（pane: 3, 5）。--pane で明示指定してください",
    "master/solo ペインが複数（pane: 3, 5）。pid / env / stale map では解決できず、role も曖昧。\
     --pane で明示指定してください",
    "pane 42 の報告を取得できない（backend session 不在または scrollback 空）",
    "プロファイルの projects 制限により、プロジェクト 'web' への spawn は許可されていない（許可: api, docs）",
    "プロジェクトキーがファイル名として使えない: a/b",
    "ホームディレクトリが取得できない",
    "show は project か profile のどちらか一方を指定する",
    // 後ろに tako-core の雛形（`project_handoff_template`）が付く（[`reason_old_ja`]）
    "引き継ぎの材料が無い（プロジェクト単位のファイルも運用メモも空）。\n\
     プロジェクト固有の引き継ぎ: /tmp/h/projects/<project-key>.md\n\
     プロジェクトに紐付かない運用メモ: /tmp/h/default.md\n\
     master は引き継ぎ前に前者へ状態を書き込む必要がある。書式:\n",
    "system prompt の保存先を作れない: Permission denied (os error 13)",
    "system prompt の書き出しに失敗: No space left on device (os error 28)",
    "worker レジストリを読めない: expected value at line 1 column 1",
    // #2365 の続き: 設定の保存（更新通知カード・ウェルカムバナー）・リモート閲覧のショートカット
    "更新通知カードの状態を保存できない: Read-only file system (os error 30)",
    "ウェルカムバナーの状態を保存できない: データディレクトリを解決できない",
    "保存に失敗: Permission denied (os error 13)",
    "絶対パスで指定してください",
    // #2365 の続き: Web ビュー・メニュー
    "URL が空です",
    "id は必須（web list で確認）",
    "Web ビュー 2 は既に dock 退避中",
    "to は必須（back / forward / reload / URL）",
    "この環境の menu open / close は使えません（メニューは OS のメニューバーに載るため \
     tako が開閉できない）。項目の実行は menu invoke が使えます",
    "メニュー名が空",
    "メニュー名 'e' が曖昧です: Edit / View",
    "メニュー 'Nope' が見つかりません（候補: File / Edit）",
    "メニュー定義がありません（GUI が起動していない可能性があります）",
    "項目のパスが空",
    "項目 'tab' が曖昧です: File/New Tab / File/Close Tab",
    "メニュー項目 'File/Nope' が見つかりません（`tako menu list` で一覧できます）",
    // #2365 の続き: 個別の値の検証（limit-resume・panel・paste-guard・logs・run・sessions・tmux）
    "network と enabled は併用できない（全体の設定かペインの設定のどちらか）",
    "all と enabled は併用できない（一覧か設定のどちらか）",
    "width は正の数（px）を指定する",
    "mode は confirm / warn のどれか（指定: loud）",
    "pane は判定する文字（check / text）と一緒に渡す",
    "max_mb は 1 以上を指定する",
    "auto_close は success / always / never のいずれか（指定: \"sometimes\"）",
    "path（実行設定）と ext / command / remove（拡張子既定）は同時に使えない",
    "read には pane または session_id が必要",
    "cwd が取得できない",
    "cwd が存在しない: /tmp/gone",
    "cols と rows の両方を指定するか、reset を使うこと",
    "ペイン 4 に tmux セッションがない（tmux 永続化が無効 / 直接 spawn / 取り込みビューでもない）",
    "tmux セッション work が見つからない（既定サーバー）。対象は `tako tmux list` で確認すること",
    // #2365 の続き: dispatch のジャンプ履歴・LSP・ワークスペース横断検索
    "これより前の履歴は無い",
    "これより先の履歴は無い",
    "choice は 1 始まり（候補の一覧の番号）",
    "column 3 は 2 行目（7 バイト）の文字の境界ではない",
    "choice 4 は候補（2 か所）の外",
    "choice 9 は候補（5 件）の外",
    "limit は 1 以上（返す候補の件数）",
    "item を指定する（応答の id）",
    "set は action=options のときだけ使える",
    "サーバ名が不正: nope（rust-analyzer / pyright）",
    "この tako には右パネルの検索結果が無い（GUI のプライマリで使う。出さない検索は使える）",
    "search には query（検索語）が要る",
    "相対パス src の基準（ペインの cwd）が分からない。絶対パスで渡す",
    "探す範囲がホーム（またはルート）だけになる。フォルダへ cd するか、\
     ファイルツリーにフォルダを足す（tako tree add <フォルダ>）か、path で範囲を渡す",
    "open には item（結果の id。1 始まり）が要る",
    "右パネルに検索結果が無い（tako search <検索語> --show で出してから開く）",
    "item 8 は結果（3 件）の外",
    // #2365 の続き: dispatch の変更履歴
    "変更履歴を取っている間にプレビューのファイルが変わった（履歴を置かない）",
    "リポジトリ情報がない",
    "ファイルパス情報がない",
    "差分を取っている間にプレビューのファイルが変わった（差分を置かない）",
    "差分を取っている間に変更履歴が変わった（unknown commit abc123）",
    "差分を取っている間に変更履歴が入れ替わった（差分を置かない）",
    // #2365 の続き: dispatch のリモートフォルダ・config share・scrollback
    "host が必要",
    "開いていない: devbox:/srv/app（`list` で開いているものを確認できる）",
    "押し出せていない保存はありません（`pending` で確認できます）",
    "link には対象（リポジトリのパスまたは URL）が必要です",
    // 後ろに `reach::no_detached_capture_note` が付く（[`reason_old_ja`]）
    "ペイン 9 は器（永続化バックエンド）を持たないので履歴を読めない: ",
    // #2365 の続き: dispatch のコマンドカード・Code Runner
    "コマンド提案カードは GUI（tako-app）が必要（この接続先には保管庫が無い）",
    "このペインに表示中のコマンドカードが無い",
    "クリップボードへ書き込めない（GUI が必要）",
    "scope / profile / runtime / reset / args / env / cwd / before / board / port / \
     target は path（対象ファイル）と一緒に渡す",
    "実行前の保存に失敗したので実行しない（pane 6: Read-only file system (os error 30)）",
    "ファイルを読めない（/tmp/x.py: No such file or directory (os error 2)）",
    "ファイルの読み取りに失敗（/tmp/x.py: Is a directory (os error 21)）",
    // #2365 の続き: dispatch の先取り・git・tmux・動画・ウィンドウ・引き継ぎ
    "codex の CLI の所在を確かめられなかった（UI スレッドでは確かめない。\
     もう一度呼ぶと確かめ直す）",
    "コンフリクトの状態を確かめられなかった（UI スレッドでは git を待たない。\
     もう一度呼ぶと確かめ直す）",
    "tmux セッションを確かめられなかった（UI スレッドでは tmux を待たない。\
     もう一度呼ぶと確かめ直す）",
    "コンフリクトが発生していません（解消エージェントを起動する状況ではありません）",
    "--servers はソケット置き場全体を対象にするので --socket（tako-x）とは併用できない。\
     1 つのサーバーの中を掃除するなら --servers を外す",
    "対象ペインは動画プレビューではない",
    "ウィンドウの位置・寸法を指定できない: x is not a number",
    "cwd does not exist: /tmp/gone\n  直すには: tako orchestrator profiles set default --cwd <path>（解除は --clear-cwd）",
    // #2365 の続き: dispatch のファイルツリー
    "絶対パスを指定する",
    "指定フォルダはピン留めされていない: /tmp/a",
    "action は add / remove のいずれか（受け取った値: bogus）",
    "ファイルツリーはアクティブタブ（1）のものだけを出す（受け取った tab: 2）",
    "ファイルツリーが閉じている（`tako panel --filetree on` で開く）",
    "path（その行を選ぶ）と key（選んでいる行の上で押す）は同時に指定しない",
    "ファイルツリーに見えていない行: /tmp/a/b.rs（畳んだフォルダの中・リモートの行は選べない）",
    "ファイルツリーの行を選んでいない（先に path で選ぶ）",
    // #2365 の続き: dispatch の supervisor・ledger・セッション再起動
    "mode は auto / notify_only / off のいずれかを指定",
    "supervisor の action は status / set_mode / history のいずれか（不明: 'bogus'）",
    "project（前方一致プレフィックス）は必須",
    "不正な action 'bogus'。使用可能: list, stats, record, amend, prune",
    "mode が不正: \"bogus\"（harness | handoff。省略すると下見だけを返す）",
    "pane 5 の再開コマンドを組めないのでハーネス更新しない: no conversation",
    "再開に要るファイルを書けないので建て直さない: Permission denied (os error 13)",
    "エージェントのプロセス（pid 4321）を終了できなかったので建て直さない: \
     Operation not permitted (os error 1)",
];

/// [`REASON_OLD_JA`] の旧文。引き継ぎの材料が無い断りは、dispatch の固定文の後ろに tako-core の雛形
/// （#2365 より前も `project_handoff_template` = 表示言語の雛形を付けていた）が続く。
/// 器を持たないペインの履歴の断りは、後ろに `reach::no_detached_capture_note`（#2365 より前も同じ注記）が続く
fn reason_old_ja(r: &Reason) -> String {
    let fixed = REASON_OLD_JA[reason_index(r)];
    match r {
        Reason::NoHandoffMaterial { .. } => format!(
            "{fixed}{}",
            tako_core::handoff::project_handoff_template_in(Lang::Ja, "<project-key>")
        ),
        Reason::NoVesselForHistory(_) => {
            format!("{fixed}{}", tako_control::reach::no_detached_capture_note())
        }
        _ => fixed.to_string(),
    }
}

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
            | Reason::TmuxSessionNotFound { .. }
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
            reason_old_ja(&r),
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
/// `impl Reason {` の本体（日英の対の表）とコメントの行は外す（そこの日本語は正しい）。
///
/// 単体テストの始まりは `#[cfg(test)]` の次の行が `mod ` の所（[`front_scope_lines`] と同じ読み）。
/// 本体の途中の `#[cfg(test)] fn …`（テスト用の 1 関数）で切ると、その後ろ（ファイルツリー・
/// supervisor・セッション再起動ほか）を見ずに通っていた（#2365 の続きで直した）
fn dispatch_code_lines() -> Vec<(usize, &'static str)> {
    let lines: Vec<&'static str> = DISPATCH_SRC.lines().collect();
    let tests = (0..lines.len())
        .find(|&i| {
            lines[i].starts_with("#[cfg(test)]")
                && lines
                    .get(i + 1)
                    .is_some_and(|next| next.starts_with("mod "))
        })
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
        let r = Reason::SaveSettingsFailed(IoCause::Other(e.into()));
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
    japanese_outside(lines, &["unreachable!(", ".expect("])
}

/// [`front_japanese`] の本体。`internal` の呼び出し（内部の印・診断の出力）の中の日本語は見ない
/// （呼び出しが行をまたぐときは閉じ括弧まで外す）
fn japanese_outside(lines: &[(usize, &str)], internal: &[&str]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut internal_depth = 0i32;
    for (line, code) in lines {
        let trimmed = code.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }
        let first_internal = internal.iter().filter_map(|m| code.find(m)).min();
        if internal_depth == 0 && first_internal.is_some() {
            internal_depth = 1;
            let from = first_internal.unwrap_or(0);
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

// ---- #2365 の続き: 全部の置き場が共有する書き出し（config_io）・remote daemon → app の接続・
// 置き場の口（settings / discovery / layout）の「データディレクトリを解決できない」 ----
//
// 書き出しの断り（`config_io::WriteError`）は (バリアント, 言語) の網羅 match。`String` を返す口
// （`atomic_write` / `atomic_write_with_backup` / `rotate_backups` / `lock_exclusive`）は日本語のまま
// = persist.log・日本語の枠の呼び手はそのまま使い、表示言語に従う枠を持つ置き場（ゲート・
// チェックポイント・人がやること・実行設定）だけが型の口（`*_typed`）を通す。
// データディレクトリの断りは 5 か所が `config_io::DataDirUnresolved` を `io::Error` に包み、表示は
// 日本語のまま（診断ログ）・利用者へ返す枠は `IoCause` で枠と同じ言語に引く。
// remote daemon → app の 9 行は `Reason`（上の表）へ移した。ここではソースを読んで、日本語の
// 固定文の書き足し・日本語の口の呼び足し・`io::Error` の素の文での包みを `file:line` で名指す

use tako_control::config_io::{DataDirUnresolved, IoCause, WriteError};

const REMOTE_SRC: &str = include_str!("../src/remote.rs");
const REMOTE_PATH: &str = "crates/tako-control/src/remote.rs";
const SETTINGS_SRC: &str = include_str!("../src/settings.rs");
const SETTINGS_PATH: &str = "crates/tako-control/src/settings.rs";
const DISCOVERY_SRC: &str = include_str!("../src/discovery.rs");
const DISCOVERY_PATH: &str = "crates/tako-control/src/discovery.rs";
const LAYOUT_FILE_SRC: &str = include_str!("../src/layout.rs");
const LAYOUT_FILE_PATH: &str = "crates/tako-control/src/layout.rs";
const RUN_CONFIGS_SRC: &str = include_str!("../src/run_configs.rs");
const RUN_CONFIGS_PATH: &str = "crates/tako-control/src/run_configs.rs";
const LSP_MANAGER_SRC: &str = include_str!("../src/lsp/manager.rs");
const LSP_MANAGER_PATH: &str = "crates/tako-control/src/lsp/manager.rs";

/// `WriteError` の通し番号（網羅 match = バリアントを足すとここがコンパイルで落ちる）
fn write_index(e: &WriteError) -> usize {
    match e {
        WriteError::NoParent(_) => 0,
        WriteError::CreateDir(_) => 1,
        WriteError::TmpWrite { .. } => 2,
        WriteError::Rename { .. } => 3,
        WriteError::BackupShift { .. } => 4,
        WriteError::BackupCreate { .. } => 5,
    }
}
const WRITE_VARIANTS: usize = 6;

/// 見本と #2365 より前の固定文（`atomic_write` / `rotate_backups` が書いていた文を見本の値で展開）
fn write_table() -> Vec<(WriteError, &'static str)> {
    vec![
        (
            WriteError::NoParent("/".into()),
            "親ディレクトリがない: /",
        ),
        (
            WriteError::CreateDir("Not a directory (os error 20)".into()),
            "ディレクトリの作成に失敗: Not a directory (os error 20)",
        ),
        (
            WriteError::TmpWrite {
                path: "/tmp/user-tasks.yaml.tmp.42.0".into(),
                error: "Permission denied (os error 13)".into(),
            },
            "一時ファイルの書き込みに失敗 (/tmp/user-tasks.yaml.tmp.42.0): Permission denied (os error 13)",
        ),
        (
            WriteError::Rename {
                from: "/tmp/a.yaml.tmp.42.0".into(),
                to: "/tmp/a.yaml".into(),
                error: "Is a directory (os error 21)".into(),
            },
            "rename に失敗 (/tmp/a.yaml.tmp.42.0 → /tmp/a.yaml): Is a directory (os error 21)",
        ),
        (
            WriteError::BackupShift {
                path: "/tmp/a.yaml.bak.1".into(),
                error: "Is a directory (os error 21)".into(),
            },
            "バックアップの繰り下げに失敗 (/tmp/a.yaml.bak.1): Is a directory (os error 21)",
        ),
        (
            WriteError::BackupCreate {
                path: "/tmp/a.yaml".into(),
                error: "Is a directory (os error 21)".into(),
            },
            "バックアップの作成に失敗 (/tmp/a.yaml): Is a directory (os error 21)",
        ),
    ]
}

#[test]
fn 書き出しの断りは全バリアントが日英を持ち日本語は2365より前の固定文とバイト一致() {
    let errors: Vec<WriteError> = write_table().into_iter().map(|(e, _)| e).collect();
    covers_all(&errors, write_index, WRITE_VARIANTS);
    for (e, old) in write_table() {
        let debug = format!("{e:?}");
        let at = |lang| {
            where_written(
                CONFIG_IO_SRC,
                CONFIG_IO_PATH,
                "impl WriteError {",
                &debug,
                lang,
            )
        };
        assert_eq!(e.text_in(Lang::Ja), old, "{debug}（{}）", at(Lang::Ja));
        bilingual_named(at, &debug, &e.text_in(Lang::Ja), &e.text_in(Lang::En));
    }
}

/// 実ファイル: 日本語で返す口（persist.log・日本語の枠の呼び手が使う）は、型の口の日本語と同じ文を
/// 返す（= 修正前と同じ文）。境界: 親が無い（空のパス）・親の途中がファイル・`.bak.1` が
/// フォルダ（作れない）・`.bak.2` がフォルダ（繰り下げられない）
#[test]
fn 書き出しの日本語の口は型の口の日本語と同じ文を返す() {
    use tako_control::config_io as io;
    let dir = std::env::temp_dir().join(format!("tako-2365-write-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("一時ディレクトリ");

    let mut seen = Vec::new();
    // 親が無い
    let typed = io::atomic_write_typed(std::path::Path::new(""), "x").expect_err("親が無い");
    assert_eq!(typed, WriteError::NoParent(String::new()));
    assert_eq!(
        io::atomic_write(std::path::Path::new(""), "x").expect_err("親が無い"),
        typed.text_in(Lang::Ja)
    );
    seen.push(typed);
    // 親の途中がファイル（フォルダを作れない）
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, "x").expect("ファイル");
    let target = blocker.join("sub").join("a.yaml");
    let typed = io::atomic_write_typed(&target, "x").expect_err("作れない");
    assert!(matches!(typed, WriteError::CreateDir(_)), "{typed:?}");
    assert_eq!(
        io::atomic_write(&target, "x").expect_err("作れない"),
        typed.text_in(Lang::Ja)
    );
    seen.push(typed);
    // `.bak.1` がフォルダ（本体を複製できない）
    let path = dir.join("b.yaml");
    std::fs::write(&path, "v1").expect("本体");
    std::fs::create_dir_all(io::backup_path(&path, 1)).expect("bak.1 をフォルダに");
    let typed = io::atomic_write_with_backup_typed(&path, "v2").expect_err("複製できない");
    let WriteError::BackupCreate { path: named, .. } = &typed else {
        panic!("BackupCreate になる: {typed:?}");
    };
    assert_eq!(named, &path.display().to_string());
    assert_eq!(
        io::atomic_write_with_backup(&path, "v2").expect_err("複製できない"),
        typed.text_in(Lang::Ja)
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "v1",
        "本体は書き換えない"
    );
    seen.push(typed);
    // `.bak.1` がファイルで `.bak.2` がフォルダ（繰り下げられない）
    let path = dir.join("c.yaml");
    std::fs::write(&path, "v1").expect("本体");
    std::fs::write(io::backup_path(&path, 1), "v0").expect("bak.1");
    std::fs::create_dir_all(io::backup_path(&path, 2)).expect("bak.2 をフォルダに");
    let typed = io::rotate_backups_typed(&path).expect_err("繰り下げられない");
    let WriteError::BackupShift { path: named, .. } = &typed else {
        panic!("BackupShift になる: {typed:?}");
    };
    assert_eq!(named, &io::backup_path(&path, 1).display().to_string());
    assert_eq!(
        io::rotate_backups(&path).expect_err("繰り下げられない"),
        typed.text_in(Lang::Ja)
    );
    seen.push(typed);

    for e in &seen {
        let en = e.text_in(Lang::En);
        assert!(has_japanese(&en).is_none(), "{e:?} の英語に日本語: {en}");
    }
    assert!(dir.starts_with(std::env::temp_dir()));
    let _ = std::fs::remove_dir_all(&dir);
}

/// データディレクトリの印: `io::Error` の表示（= persist.log・起動時の警告・日本語の枠）は #2365 より前と
/// バイト一致のまま、利用者へ返す枠は `IoCause` で同じ言語に引ける。素の文字列で包んだ旧形は印として
/// 読まない（5 か所が印で包んでいることは下のソースの番犬が見る）
#[test]
fn データディレクトリの印は表示が日本語のまま中身を表示言語で引ける() {
    let e = DataDirUnresolved.io_error();
    assert_eq!(e.kind(), std::io::ErrorKind::Unsupported);
    assert_eq!(e.to_string(), "データディレクトリを解決できない");
    let cause = IoCause::of(&e);
    assert_eq!(cause, IoCause::DataDirUnresolved);
    assert_eq!(cause.text_in(Lang::Ja), "データディレクトリを解決できない");
    assert_eq!(
        cause.text_in(Lang::En),
        Reason::DataDirUnresolved.text_in(Lang::En)
    );
    assert!(has_japanese(&cause.text_in(Lang::En)).is_none());
    // OS の文は言語に依らずそのまま
    let os = std::io::Error::from_raw_os_error(13);
    assert_eq!(IoCause::of(&os), IoCause::Other(os.to_string()));
    for lang in [Lang::Ja, Lang::En] {
        assert_eq!(IoCause::of(&os).text_in(lang), os.to_string());
    }
    let legacy = std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "データディレクトリを解決できない",
    );
    assert!(matches!(IoCause::of(&legacy), IoCause::Other(_)));
    // settings.json の書き出しの断りに包んでも枠と中身の言語がそろう
    assert_eq!(
        Reason::settings_save_failed(&e).text_in(Lang::Ja),
        "設定の保存に失敗: データディレクトリを解決できない"
    );
    for cause in [
        IoCause::DataDirUnresolved,
        IoCause::Other("Read-only file system (os error 30)".into()),
        IoCause::Other(String::new()),
    ] {
        let r = Reason::SaveSettingsFailed(cause.clone());
        let en = r.text_in(Lang::En);
        assert!(has_japanese(&en).is_none(), "{cause:?}: {en}");
        assert_eq!(
            en,
            format!("failed to save the settings: {}", cause.text_in(Lang::En))
        );
        assert_eq!(
            r.text_in(Lang::Ja),
            format!("設定の保存に失敗: {}", cause.text_in(Lang::Ja))
        );
    }
}

/// 表示言語に従う枠を持つ置き場の書き出し（型の口を通すべき範囲）
const TYPED_WRITE_SCOPES: &[(&str, &str, Span)] = &[
    (
        GATES_PATH,
        GATES_SRC,
        Span::Item("impl AcceptanceGateStore {"),
    ),
    (CHECKPOINTS_PATH, CHECKPOINTS_SRC, Span::Whole),
    (
        USER_TASKS_PATH,
        USER_TASKS_SRC,
        Span::Item("pub fn mutate_at<"),
    ),
    (
        RUN_CONFIGS_PATH,
        RUN_CONFIGS_SRC,
        Span::Item("pub fn update_at<"),
    ),
];

/// 日本語で返す口（型の口 `*_typed` の外側）
const JA_WRITE_CALLS: [&str; 4] = [
    "config_io::lock_exclusive(",
    "config_io::atomic_write(",
    "config_io::atomic_write_with_backup(",
    "config_io::rotate_backups(",
];

#[test]
fn 表示言語に従う置き場は書き出しと排他の日本語の口を呼ばない() {
    let mut failures = Vec::new();
    for (path, src, span) in TYPED_WRITE_SCOPES {
        let Some(lines) = client_store_lines(src, span) else {
            failures.push(format!(
                "{path}: 走査の範囲 {} が見つからない（改名で番犬が空振りする。\
                 TYPED_WRITE_SCOPES を直す）",
                span_name(span)
            ));
            continue;
        };
        for (line, code) in lines {
            if code.trim_start().starts_with("//") {
                continue;
            }
            if let Some(call) = JA_WRITE_CALLS.iter().find(|c| code.contains(*c)) {
                failures.push(format!(
                    "{path}:{line}: 表示言語に従う置き場が日本語の口 `{call}…)` を呼んでいる = 英語表示でも \
                     書き出し・排他の断りが日本語で混ざる（`*_typed` を呼んで `.to_string()` で返す）: {}",
                    code.trim()
                ));
            }
        }
    }
    report_all(failures);
}

/// 書き出し・排他の口・remote daemon → app の接続・データディレクトリの印（日本語の固定文を書かない範囲。
/// 日英の対は `text_in` の網羅 match にだけ置く）
const IO_REFUSAL_SCOPES: &[(&str, &str, Span)] = &[
    (
        CONFIG_IO_PATH,
        CONFIG_IO_SRC,
        Span::Item("pub fn lock_exclusive("),
    ),
    (
        CONFIG_IO_PATH,
        CONFIG_IO_SRC,
        Span::Item("pub fn lock_exclusive_typed("),
    ),
    (
        CONFIG_IO_PATH,
        CONFIG_IO_SRC,
        Span::Item("pub fn atomic_write("),
    ),
    (
        CONFIG_IO_PATH,
        CONFIG_IO_SRC,
        Span::Item("pub fn atomic_write_typed("),
    ),
    (
        CONFIG_IO_PATH,
        CONFIG_IO_SRC,
        Span::Item("pub fn atomic_write_with_backup("),
    ),
    (
        CONFIG_IO_PATH,
        CONFIG_IO_SRC,
        Span::Item("pub fn atomic_write_with_backup_typed("),
    ),
    (
        CONFIG_IO_PATH,
        CONFIG_IO_SRC,
        Span::Item("pub fn rotate_backups("),
    ),
    (
        CONFIG_IO_PATH,
        CONFIG_IO_SRC,
        Span::Item("pub fn rotate_backups_typed("),
    ),
    (
        CONFIG_IO_PATH,
        CONFIG_IO_SRC,
        Span::Item("impl DataDirUnresolved {"),
    ),
    (CONFIG_IO_PATH, CONFIG_IO_SRC, Span::Item("impl IoCause {")),
    (REMOTE_PATH, REMOTE_SRC, Span::Item("impl AppIpcClient {")),
    (SETTINGS_PATH, SETTINGS_SRC, Span::Item("pub fn save(")),
    (DISCOVERY_PATH, DISCOVERY_SRC, Span::Item("pub fn write(")),
    (
        DISCOVERY_PATH,
        DISCOVERY_SRC,
        Span::Item("pub fn write_instance_only("),
    ),
];

#[test]
fn 書き出しと接続と置き場の口の断りに日本語の固定文が無い() {
    let mut failures = Vec::new();
    for (path, src, span) in IO_REFUSAL_SCOPES {
        let Some(lines) = client_store_lines(src, span) else {
            failures.push(format!(
                "{path}: 走査の範囲 {} が見つからない（改名で番犬が空振りする。\
                 IO_REFUSAL_SCOPES を直す）",
                span_name(span)
            ));
            continue;
        };
        for (line, text) in front_japanese(&lines) {
            failures.push(format!(
                "{path}:{line}: 書き出し・接続・置き場の口の断りに日本語の固定文がある = 英語表示にもそのまま出る \
                 （config_io::WriteError / tako_control::Reason へバリアントを足し、日英の対を text_in に書く）: {text}"
            ));
        }
    }
    report_all(failures);
}

/// 置き場の口（settings / discovery / layout）は「データディレクトリを解決できない」を素の文字列で
/// 包まない（印 `config_io::DataDirUnresolved` で包む = 利用者へ返す枠が `IoCause` で表示言語に引ける）
#[test]
fn 置き場の口はデータディレクトリの断りを印で包む() {
    let mut failures = Vec::new();
    for (path, src) in [
        (SETTINGS_PATH, SETTINGS_SRC),
        (DISCOVERY_PATH, DISCOVERY_SRC),
        (LAYOUT_FILE_PATH, LAYOUT_FILE_SRC),
    ] {
        let lines = front_scope_lines(src, None).expect("ファイルの本体");
        for (line, code) in lines {
            if code.trim_start().starts_with("//") {
                continue;
            }
            if line_literals(code)
                .iter()
                .any(|lit| lit.contains("データディレクトリを解決できない"))
            {
                failures.push(format!(
                    "{path}:{line}: データディレクトリの断りを素の文字列で書いている = 利用者へ返す枠が \
                     表示言語で引けない（`config_io::DataDirUnresolved.io_error()` で包む）: {}",
                    code.trim()
                ));
            }
        }
    }
    report_all(failures);
}

/// `settings::save(` の結果を同じ文の中で `e.to_string()`（= `io::Error` の日本語の表示）のまま
/// 断りへ載せている行（1 始まり）。文の終わり（`;` で閉じる行）まで、最大 5 行を見る
fn settings_save_raw_wraps(src: &str) -> Vec<usize> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if !l.contains("settings::save(") || l.trim_start().starts_with("//") {
            continue;
        }
        for (j, next) in lines.iter().enumerate().skip(i).take(5) {
            if next.contains("e.to_string()") && !next.trim_start().starts_with("//") {
                out.push(j + 1);
            }
            if next.trim_end().ends_with(';') {
                break;
            }
        }
    }
    out
}

/// 表示言語に従う枠（`Reason::SaveSettingsFailed` / 素の `Operation` / LSP の `Note`）へ settings.json の
/// 書き出しの断りを載せるときは `IoCause::of(&e)` を通す（素の `e.to_string()` は印を日本語で出す）
#[test]
fn settings_の書き出しの断りを素の文で包まない() {
    let mut failures = Vec::new();
    for (path, src) in [
        (DISPATCH_PATH, DISPATCH_SRC),
        (LSP_MANAGER_PATH, LSP_MANAGER_SRC),
    ] {
        for line in settings_save_raw_wraps(src) {
            failures.push(format!(
                "{path}:{line}: settings.json の書き出しの断りを `e.to_string()` のまま載せている = \
                 データディレクトリの断りが英語表示でも日本語で混ざる（`Reason::settings_save_failed(&e)` か \
                 `config_io::IoCause::of(&e)` を通す）"
            ));
        }
    }
    report_all(failures);
}

/// 番犬自身の検査: 文の終わりで止まり、同じ文の中の素の包みだけを名指す
#[test]
fn settings_の書き出しの走査は文の終わりで止まる() {
    let src = "fn a() {\n    let _ = crate::settings::save(&s);\n    x.map_err(|e| e.to_string());\n    crate::settings::save(&s)\n        .map_err(|e| DispatchError::Operation(e.to_string()))?;\n    // crate::settings::save(&s).map_err(|e| e.to_string())\n}\n";
    assert_eq!(settings_save_raw_wraps(src), vec![5]);
}

// ---- #2365 の続き: dispatch の個別の断り（選択肢ダイアログ・orchestrator・設定の保存・Web・メニュー・
// 値の検証）のソースを読む番犬 ----
//
// 日英の対は上の `Reason` の表で見る。表では見えないのは、移した断りの近くへ日本語の固定文を
// **書き足す**こと（英語表示でもその 1 か所だけ日本語で返る）。2 つの形で `file:line` を名指す:
// (1) 断りを全部移した関数（[`DISPATCH_REASON_SCOPES`]）は、関数の中に日本語の固定文があれば落とす
//     （`unreachable!` / `expect` の文とコメントは見ない = 前段の番犬と同じ読み方）
// (2) 日本語の残る大きな関数（`dispatch_inner` ほか）の中で移した文（[`DISPATCH_MOVED_OLD_JA`]）は、
//     旧文の断片が dispatch の本体（`impl Reason` の表と単体テストの外）へ戻れば落とす

/// 断りを全部 `Reason` へ移した dispatch の関数（頭）。persist.log・応答の注記など日本語のまま残す文を
/// 持つ関数はここへ入れず、[`DISPATCH_MOVED_OLD_JA`] の断片で見る
const DISPATCH_REASON_SCOPES: &[&str] = &[
    // 選択肢ダイアログへの応答（`tako orchestrator respond`）とダイアログ表示中の送信
    "pub fn respond_via(",
    "fn resolve_choice_index(",
    "fn dialog_blocks_send(",
    "fn dialog_send_refusal(",
    // orchestrator
    "pub fn dispatch_orchestrator_profiles(",
    "fn find_master_pane_strict(",
    "fn finish_orchestrator_report(",
    "pub fn dispatch_orchestrator_handoff_files(",
    "fn resolve_worker_query(",
    // リモート閲覧のショートカット・メニュー
    "pub fn dispatch_remote_shortcuts(",
    "fn require_in_window_menu(",
    "fn resolve_menu_index(",
    "pub fn resolve_menu_item(",
    // 値の検証・cwd・tmux
    "fn send_paste_gate(",
    "fn dispatch_logs_read(",
    "fn git_pane_cwd(",
    "fn tmux_select_window_target(",
    "fn compute_tmux_open_lookup(",
    // #2365 の続き: ジャンプ履歴・LSP・ワークスペース横断検索
    "fn jump(",
    "pub fn lsp_goto_prepare(",
    "fn check_line_column(",
    "fn workspace_search_unavailable(",
    "fn workspace_search_prepare(",
    "fn workspace_search_roots(",
    "fn workspace_search_open(",
    "pub fn lsp_completion_prepare(",
    "pub fn lsp_hover_prepare(",
    "pub fn lsp_server_action(",
    // 変更履歴の差分・リモートフォルダ・config share・scrollback
    "fn preview_changelog_diff_prepare(",
    "fn preview_changelog_diff_land(",
    "    fn need_host(",
    "    fn need_path(",
    "pub fn dispatch_config_share(",
    "fn scrollback_target_session(",
    // コマンドカード・Code Runner
    "fn cards_unsupported(",
    "fn command_card_err(",
    "fn save_previews_before_run(",
    "fn read_file_head(",
    // 先取りの答えが無かった断り・動画・ウィンドウ
    "fn agent_preflight(",
    "fn git_conflict_for(",
    "fn tmux_open_lookup(",
    "fn require_video_preview(",
    "fn window_geometry_op(",
    // ファイルツリー・supervisor・ledger
    "fn dispatch_tree_folder(",
    "fn dispatch_orchestrator_supervisor(",
    "fn dispatch_orchestrator_ledger(",
];

/// 日本語の残る関数の中で移した断りの旧文の断片と、寄せた先のバリアント
const DISPATCH_MOVED_OLD_JA: &[(&str, &str)] = &[
    ("更新通知カードの状態を保存できない", "UpdateCardSaveFailed"),
    ("ウェルカムバナーの状態を保存できない", "WelcomeSaveFailed"),
    ("URL が空です", "UrlEmpty"),
    ("id は必須（web list", "WebShowNeedsId"),
    ("は既に dock 退避中", "WebAlreadyDocked"),
    ("to は必須（back", "WebNavigateNeedsTo"),
    ("は併用できない（全体の設定か", "NetworkWithEnabled"),
    ("は併用できない（一覧か", "AllWithEnabled"),
    ("は正の数（px）", "PositivePx"),
    ("のどれか（指定: ", "OneOfGiven"),
    ("判定する文字（check / text）", "PaneNeedsCheckText"),
    ("は 1 以上を指定する", "AtLeastOne"),
    ("auto_close は success", "AutoCloseValue"),
    (
        "（拡張子既定）は同時に使えない",
        "RunConfigPathWithDefaults",
    ),
    ("cwd が取得できない", "CwdUnavailable"),
    ("cwd が存在しない", "CwdMissing"),
    ("cols と rows の両方", "TmuxResizeShape"),
    ("は master ではない（role=", "NotMasterPane"),
    (
        "はプロファイルでもプロジェクトでもない",
        "NotProfileOrProject",
    ),
    (
        "config_dir か inherit のどちらか",
        "AccountNeedsConfigOrInherit",
    ),
    ("は登録されていない", "AccountNotRegistered"),
    ("への spawn は許可されていない", "ProjectNotAllowed"),
    ("pane または tab を指定してください", "EitherOf"),
    ("引き継ぎの材料が無い", "NoHandoffMaterial"),
    ("system prompt の保存先を作れない", "SystemPromptDirFailed"),
    ("system prompt の書き出しに失敗", "SystemPromptWriteFailed"),
    ("ホームディレクトリが取得できない", "HomeDirUnknown"),
    ("worker レジストリを読めない", "WorkerRegistryUnreadable"),
    // #2365 の続き: LSP の着地・結果（「答えがまだ無い」の内部の取り違えは日本語のまま残す）
    ("か所）の外", "ChoiceOutsideTargets"),
    ("件）の外", "ChoiceOutsideItems / ItemOutsideResults"),
    ("（応答の id）", "ItemNeedsResponseId"),
    // 変更履歴（応答の注記「git 管理外のファイル」は日本語のまま残す）
    ("変更履歴を取っている間に", "ChangelogFileChanged"),
    ("リポジトリ情報がない", "ChangelogNoRepo"),
    ("ファイルパス情報がない", "ChangelogNoPath"),
    // dispatch_inner・リモートフォルダ・コマンドの実行
    ("ソケット置き場全体を対象にする", "ServersWithSocket"),
    ("path（対象ファイル）と一緒に渡す", "RunConfigNeedsPath"),
    ("で開いているものを確認できる", "RemoteFolderNotOpen"),
    ("押し出せていない保存はありません", "NoPendingPush"),
    ("クリップボードへ書き込めない", "ClipboardNeedsGui"),
    // orchestrator（引き継ぎの起動フォルダ・コンフリクト解消）
    ("（解除は --clear-cwd）", "ProfileCwdBroken"),
    ("コンフリクトが発生していません", "NoConflict"),
    // ファイルツリーの選択（応答の注記「リモート（SSH）の行は…」は日本語のまま残す）
    ("ファイルツリーはアクティブタブ", "TreeOnlyActiveTab"),
    ("ファイルツリーが閉じている", "FileTreeClosed"),
    ("選んでいる行の上で押す", "TreePathWithKey"),
    ("ファイルツリーに見えていない行", "TreeRowNotVisible"),
    ("のいずれか（受け取った値: ", "OneOfReceived"),
    ("ファイルツリーの行を選んでいない", "TreeNoSelection"),
    // セッション再起動（persist.log の行は日本語のまま残す）
    ("省略すると下見だけを返す", "RestartModeInvalid"),
    (
        "再開コマンドを組めないのでハーネス更新しない",
        "ResumeCommandUnbuildable",
    ),
    ("再開に要るファイルを書けない", "ResumeFilesUnwritable"),
    ("を終了できなかったので建て直さない", "AgentTerminateFailed"),
];

/// [`DISPATCH_MOVED_OLD_JA`] の断片が dispatch の本体の文字列リテラルにある行（1 始まり）
fn moved_old_ja_hits(lines: &[(usize, &str)]) -> Vec<(usize, &'static str, &'static str)> {
    let mut out = Vec::new();
    for (line, code) in lines {
        for lit in line_literals(code) {
            for (fragment, variant) in DISPATCH_MOVED_OLD_JA {
                if lit.contains(fragment) {
                    out.push((*line, *fragment, *variant));
                }
            }
        }
    }
    out
}

#[test]
fn dispatch_の個別の断りを移した関数に日本語の固定文が無い() {
    let mut failures = Vec::new();
    for head in DISPATCH_REASON_SCOPES {
        let Some(lines) = front_scope_lines(DISPATCH_SRC, Some(head)) else {
            failures.push(format!(
                "{DISPATCH_PATH}: 走査の範囲 `{head}` が見つからない（関数の改名で番犬が空振りする。\
                 DISPATCH_REASON_SCOPES を直す）"
            ));
            continue;
        };
        for (line, text) in front_japanese(&lines) {
            failures.push(format!(
                "{DISPATCH_PATH}:{line}: 断りを Reason へ移した関数（`{head}`）に日本語の固定文がある = \
                 英語表示にもそのまま出る（tako_control::Reason へバリアントを足し、日英の対を text_in に書く）: {text}"
            ));
        }
    }
    report_all(failures);
}

#[test]
fn dispatch_の個別の断りの旧文が本体に戻っていない() {
    let mut failures = Vec::new();
    for (line, fragment, variant) in moved_old_ja_hits(&dispatch_code_lines()) {
        failures.push(format!(
            "{DISPATCH_PATH}:{line}: 移した断りの旧文（「{fragment}」）が日本語の固定文で戻っている = \
             英語表示にもそのまま出る（Reason::{variant} を使う）"
        ));
    }
    report_all(failures);
}

/// 番犬自身の検査: 断片は旧文に当たり、`impl Reason` の表・コメントは見ない。移した関数の範囲は
/// 頭と同じ字下げの `}` で切れる（隣の関数へはみ出さない）
#[test]
fn dispatch_の個別の断りの番犬の境界() {
    let src = r#"fn a() -> Result<(), DispatchError> {
    // 更新通知カードの状態を保存できない（コメントは見ない）
    return Err(DispatchError::Operation(format!("更新通知カードの状態を保存できない: {e}")));
}
fn b() {
    let _ = "cwd が取得できない";
}
"#;
    let lines: Vec<(usize, &str)> = src.lines().enumerate().map(|(i, l)| (i + 1, l)).collect();
    let code: Vec<(usize, &str)> = lines
        .iter()
        .copied()
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .collect();
    let hits: Vec<(usize, &str)> = moved_old_ja_hits(&code)
        .into_iter()
        .map(|(line, _, variant)| (line, variant))
        .collect();
    assert_eq!(
        hits,
        vec![(3, "UpdateCardSaveFailed"), (6, "CwdUnavailable")]
    );
    let scoped = front_scope_lines(src, Some("fn a(")).expect("頭を引ける");
    assert_eq!(
        front_japanese(&scoped)
            .into_iter()
            .map(|(l, _)| l)
            .collect::<Vec<_>>(),
        vec![3]
    );
    // 移した関数の表と断片の表は、今の dispatch で全部当たる（改名で空振りしない）
    for head in DISPATCH_REASON_SCOPES {
        assert!(
            front_scope_lines(DISPATCH_SRC, Some(head)).is_some(),
            "{head}"
        );
    }
    // 断片は寄せた先の日本語の腕の文に含まれる（断片の書き間違いで番犬が何も見なくなるのを止める）
    let ja: Vec<String> = reason_samples().iter().map(reason_old_ja).collect();
    for (fragment, variant) in DISPATCH_MOVED_OLD_JA {
        assert!(
            ja.iter().any(|t| t.contains(fragment)),
            "断片「{fragment}」（{variant}）が Reason の日本語の文に無い"
        );
    }
    // 本体の範囲は単体テストの mod まで届く（途中の `#[cfg(test)] fn` で切れて、後ろの関数を
    // 見ずに通らない。#2365 の続き）
    let body = dispatch_code_lines();
    for head in [
        "fn dispatch_tree_selection(",
        "fn dispatch_session_restart(",
    ] {
        assert!(
            body.iter().any(|(_, l)| l.contains(head)),
            "dispatch の本体の範囲に `{head}` が入っていない"
        );
    }
}

/// 境界（#2365 の続き）: 値で腕を分けるもの（リモートフォルダの名前の有無・再開の理由の有無）を、
/// 表の見本に載せていない側も日英で見る。器の注記と「表示中のカードが無い」の読みは枠と同じ言語
#[test]
fn dispatch_の残りの断りの値で分かれる腕の境界() {
    let cases: Vec<(Reason, &str)> = vec![
        (
            Reason::RemoteFolderNotOpen(None),
            "開いていない: リモートフォルダ（`list` で開いているものを確認できる）",
        ),
        (
            Reason::ResumeCommandUnbuildable {
                pane: 5,
                error: None,
            },
            "pane 5 の再開コマンドを組めないのでハーネス更新しない: 理由不明",
        ),
    ];
    let mut failures = Vec::new();
    for (r, old) in &cases {
        let (ja, en) = (r.text_in(Lang::Ja), r.text_in(Lang::En));
        if ja != *old {
            failures.push(format!(
                "{r:?}: 日本語が #2365 より前と違う\n  want: {old}\n  got:  {ja}"
            ));
        }
        if let Some(c) = has_japanese(&en) {
            failures.push(format!("{r:?}: 英語に日本語（{c}）: {en}"));
        }
    }
    // 器を持たないペインの断りに添える注記は枠と同じ言語（日本語は reach の従来の注記そのもの）
    let vessel = Reason::NoVesselForHistory(3);
    assert!(vessel
        .text_in(Lang::Ja)
        .ends_with(&tako_control::reach::no_detached_capture_note()));
    assert!(vessel
        .text_in(Lang::En)
        .ends_with(&tako_control::reach::no_detached_capture_note_in(Lang::En)));
    assert_eq!(
        tako_control::reach::no_detached_capture_note(),
        tako_control::reach::no_detached_capture_note_in(Lang::Ja)
    );
    // 「表示中のコマンドカードが無い」の読み（リモートの 404 寄せ）は日英の両方に当たり、他には当たらない
    for lang in [Lang::Ja, Lang::En] {
        let text = Reason::NoCardOnPane.text_in(lang);
        assert!(Reason::says_no_command_card(&text), "{lang:?}: {text}");
        assert!(Reason::says_no_command_card(
            &E::Operation(text.clone()).text_in(lang)
        ));
        assert!(!Reason::says_no_command_card(
            &Reason::CardsNeedGui.text_in(lang)
        ));
    }
    report_all(failures);
}

/// 境界: 1 つのバリアントが値で腕を分けるもの（下見の案内・切り詰め・応答の案内のペイン・role の有無・
/// 一覧が空・確定を見送る理由・ショートカットの断り・書き出しの口の印・tmux のサーバー）を、表の見本に
/// 載せていない側も日英で見る。日本語は #2365 より前の文とバイト一致
#[test]
fn dispatch_の個別の断りの値で分かれる腕の境界() {
    let cases: Vec<(Reason, String)> = vec![
        (
            Reason::RespondHint {
                count: 2,
                truncated: true,
            },
            "応答するには choice に番号（1-2）を渡す（この画面はラベルが切り詰められているか説明との\
             境目が読めないのでラベル指定は受け付けない）"
                .into(),
        ),
        (
            Reason::UnnumberedDialogBlocked(ConfirmBlock::OutOfRange),
            "番号なしダイアログで選択肢の並びが採取のあいだに変わった（添字が範囲外。Enter は送らない）\
             （手動で応答してください）"
                .into(),
        ),
        (
            Reason::ChoiceNoMatch {
                choice: "x".into(),
                truncated: true,
                labels: "1. Defaul…".into(),
            },
            "「x」に一致する選択肢が無い（この画面はラベルが切り詰められているか説明との境目が読めないので、\
             ラベルでは確定できない。番号で指定する）。選択肢: 1. Defaul…"
                .into(),
        ),
        (
            Reason::DialogBlocksSend {
                kind: "permission",
                options: "1. Yes / 2. No".into(),
                pane: None,
            },
            "選択肢ダイアログ（permission）が表示中のため送信を中止した。入力欄が奪われているので、\
             テキストや Enter はダイアログのキー操作として食われる（数字なら選択が確定してしまう）。\
             選択肢: 1. Yes / 2. No。応答は `tako orchestrator respond --pane <N> --choice <番号|ラベル>`\
             （choice を省略すると構造だけ確認できる）"
                .into(),
        ),
        (
            Reason::NotMasterPane {
                pane: 7,
                role: None,
            },
            "ペイン 7 は master ではない（role=なし）。adopt は master のペインにだけ効く".into(),
        ),
        (
            Reason::NotProfileOrProject {
                name: "x".into(),
                profiles: Vec::new(),
                projects: Vec::new(),
            },
            "'x' はプロファイルでもプロジェクトでもない。\nプロファイル: （なし）\nプロジェクト: （なし）\n\
             プロジェクトを先に登録するには: tako orchestrator projects add --key x --cwd <path>"
                .into(),
        ),
        (
            Reason::UpdateCardSaveFailed(IoCause::DataDirUnresolved),
            "更新通知カードの状態を保存できない: データディレクトリを解決できない".into(),
        ),
        (
            Reason::WelcomeSaveFailed(IoCause::Other(String::new())),
            "ウェルカムバナーの状態を保存できない: ".into(),
        ),
        (
            Reason::TmuxSessionNotFound {
                session: "work".into(),
                socket: Some("tako-x".into()),
            },
            "tmux セッション work が見つからない（socket: tako-x）。対象は `tako tmux list` で確認すること"
                .into(),
        ),
    ];
    let mut failures = Vec::new();
    for (r, old) in &cases {
        let (ja, en) = (r.text_in(Lang::Ja), r.text_in(Lang::En));
        if &ja != old {
            failures.push(format!(
                "{r:?}: 日本語が #2365 より前と違う\n  want: {old}\n  got:  {ja}"
            ));
        }
        if let Some(c) = has_japanese(&en) {
            failures.push(format!("{r:?}: 英語に日本語（{c}）: {en}"));
        }
    }
    // ショートカットの断りは全種を tako-core の日英の対で引く
    for e in [
        ShortcutError::NotAbsolute,
        ShortcutError::NotFound,
        ShortcutError::NotADirectory,
        ShortcutError::TooMany,
        ShortcutError::UnknownShortcut,
        ShortcutError::Builtin,
    ] {
        let r = Reason::Shortcut(e);
        assert_eq!(r.text_in(Lang::Ja), e.message_ja());
        assert_eq!(r.text_in(Lang::En), e.message_en());
        // 「見つかりません」系も英語に ` not found` を含めない（404 寄せの読みを日英でそろえる）
        assert!(!E::says_not_found(&r.text_in(Lang::Ja)));
        assert!(!E::says_not_found(&r.text_in(Lang::En)));
    }
    // 引き継ぎの材料が無い断りの書式（雛形）は枠と同じ言語
    let handoff = Reason::NoHandoffMaterial {
        projects_dir: "/tmp/h".into(),
        memo_path: "/tmp/m.md".into(),
    };
    assert!(handoff
        .text_in(Lang::En)
        .ends_with(&tako_core::handoff::project_handoff_template_in(
            Lang::En,
            "<project-key>"
        )));
    // tmux の既定サーバーの日本語は tako-core の表示（`socket_label`）と同じ文
    let default_server = Reason::TmuxSessionNotFound {
        session: "w".into(),
        socket: None,
    };
    assert!(default_server
        .text_in(Lang::Ja)
        .contains(&tako_core::tmux::socket_label(None)));
    report_all(failures);
}

// ---- #2365 の続き: orchestrator の設定ファイル（projects / accounts / profiles）の読み書きと
// エージェント名の検証の断り（`orchestrator::ConfigError`） ----
//
// 断りの文は `ConfigError::text_in` の (バリアント, 言語) の網羅 match の 1 か所。従来の `String` の口
// （`ProjectsConfig::load` ほか）は日本語のまま = `tako master` の起動の流れ・persist.log・日本語の枠の
// 呼び手はそのまま使い、断りを表示言語で返す dispatch（projects / profiles / accounts / adopt /
// 引き継ぎ / spawn / コンフリクト解消）だけが型の口（`*_typed`）を通す。ここでは
// (1) 全バリアントの日英と旧文のバイト一致、(2) 口の本体に日本語の固定文を書き足していないこと、
// (3) dispatch の対象の関数が日本語の口を呼んで断りに載せていないことを `file:line` で名指す

use tako_control::orchestrator::{ConfigError, ConfigFile, ProfileKind, WorkerAgent};

const ORCH_MOD_SRC: &str = include_str!("../src/orchestrator/mod.rs");
const ORCH_MOD_PATH: &str = "crates/tako-control/src/orchestrator/mod.rs";
const ORCH_AGENT_SRC: &str = include_str!("../src/orchestrator/agent.rs");
const ORCH_AGENT_PATH: &str = "crates/tako-control/src/orchestrator/agent.rs";
const CONFIG_ERROR_SRC: &str = include_str!("../src/orchestrator/config_error.rs");
const CONFIG_ERROR_PATH: &str = "crates/tako-control/src/orchestrator/config_error.rs";

/// `ConfigError` の通し番号（網羅 match = バリアントを足すとここがコンパイルで落ちる）
fn config_error_index(e: &ConfigError) -> usize {
    match e {
        ConfigError::HomeUnresolved => 0,
        ConfigError::Read { .. } => 1,
        ConfigError::Parse { .. } => 2,
        ConfigError::Serialize(_) => 3,
        ConfigError::Lock(_) => 4,
        ConfigError::Write(_) => 5,
        ConfigError::CreateDir(_) => 6,
        ConfigError::CreateProfilesDir(_) => 7,
        ConfigError::WriteDefaultProfile(_) => 8,
        ConfigError::ListProfiles(_) => 9,
        ConfigError::DeleteProfile(_) => 10,
        ConfigError::ProfileNotFound { .. } => 11,
        ConfigError::ProfileExists { .. } => 12,
        ConfigError::DefaultProfileUndeletable(_) => 13,
        ConfigError::InvalidKind(_) => 14,
        ConfigError::ProfileNameEmpty => 15,
        ConfigError::ProfileNameLeadingDot(_) => 16,
        ConfigError::ProfileNameLeadingDash(_) => 17,
        ConfigError::ProfileNameBadChar(_) => 18,
        ConfigError::AccountNotFound(_) => 19,
        ConfigError::AccountInheritWithDir(_) => 20,
        ConfigError::AccountNoConfigDir(_) => 21,
        ConfigError::ProjectNotFound(_) => 22,
        ConfigError::ProjectCwdMissing(_) => 23,
        ConfigError::UnknownAgent(_) => 24,
        ConfigError::MasterAgentUnsupported(_) => 25,
        ConfigError::ProfileMasterAgent(_) => 26,
        ConfigError::ProfileWorkerAgent(_) => 27,
        ConfigError::ProfileCwdMissing { .. } => 28,
        ConfigError::ProfileProjectsUnregistered(_) => 29,
        ConfigError::ProfileEnvReserved(_) => 30,
        ConfigError::CtxThresholdOutOfRange(_) => 31,
    }
}
const CONFIG_ERROR_VARIANTS: usize = 32;

/// 見本と #2365 より前の固定文（`orchestrator` の口が書いていた文を見本の値で展開）
fn config_error_table() -> Vec<(ConfigError, &'static str)> {
    let yaml = YamlReason::Located {
        kind: YamlProblem::Syntax,
        at: Some((1, 11)),
    };
    let unknown = || ConfigError::UnknownAgent("gpt".into());
    vec![
        (ConfigError::HomeUnresolved, "ホームディレクトリが取得できない"),
        (
            ConfigError::Read {
                file: ConfigFile::Projects,
                error: "Permission denied (os error 13)".into(),
            },
            "projects.yaml の読み取りに失敗: Permission denied (os error 13)",
        ),
        (
            ConfigError::Read {
                file: ConfigFile::Accounts,
                error: "Permission denied (os error 13)".into(),
            },
            "accounts.yaml の読み取りに失敗: Permission denied (os error 13)",
        ),
        (
            ConfigError::Read {
                file: ConfigFile::Profile,
                error: "Is a directory (os error 21)".into(),
            },
            "プロファイルの読み取りに失敗: Is a directory (os error 21)",
        ),
        (
            ConfigError::Parse {
                file: ConfigFile::Projects,
                reason: yaml.clone(),
            },
            "projects.yaml のパースに失敗: YAML の構文が壊れている（1 行 11 列）",
        ),
        (
            ConfigError::Parse {
                file: ConfigFile::Accounts,
                reason: yaml.clone(),
            },
            "accounts.yaml のパースに失敗: YAML の構文が壊れている（1 行 11 列）",
        ),
        (
            ConfigError::Parse {
                file: ConfigFile::Profile,
                reason: YamlReason::Located {
                    kind: YamlProblem::Shape,
                    at: None,
                },
            },
            "プロファイルのパースに失敗: YAML の形が期待と違う",
        ),
        (
            ConfigError::Serialize("x".into()),
            "YAML のシリアライズに失敗: x",
        ),
        (
            ConfigError::Lock(LockError::Open {
                path: "/tmp/p.yaml.lock".into(),
                error: "Is a directory (os error 21)".into(),
            }),
            "ロックファイルを開けない (/tmp/p.yaml.lock): Is a directory (os error 21)",
        ),
        (
            ConfigError::Write(WriteError::NoParent("/".into())),
            "親ディレクトリがない: /",
        ),
        (
            ConfigError::CreateDir("Not a directory (os error 20)".into()),
            "ディレクトリの作成に失敗: Not a directory (os error 20)",
        ),
        (
            ConfigError::CreateProfilesDir("Not a directory (os error 20)".into()),
            "profiles ディレクトリの作成に失敗: Not a directory (os error 20)",
        ),
        (
            ConfigError::WriteDefaultProfile("Permission denied (os error 13)".into()),
            "default.yaml の書き込みに失敗: Permission denied (os error 13)",
        ),
        (
            ConfigError::ListProfiles("Permission denied (os error 13)".into()),
            "プロファイルディレクトリの読み取りに失敗: Permission denied (os error 13)",
        ),
        (
            ConfigError::DeleteProfile("Permission denied (os error 13)".into()),
            "プロファイルの削除に失敗: Permission denied (os error 13)",
        ),
        (
            ConfigError::ProfileNotFound {
                kind: None,
                name: "p".into(),
                path: "/tmp/profiles/p.yaml".into(),
            },
            "プロファイル 'p' が見つからない: /tmp/profiles/p.yaml",
        ),
        (
            ConfigError::ProfileNotFound {
                kind: Some(ProfileKind::Master),
                name: "p".into(),
                path: "/tmp/profiles/p.yaml".into(),
            },
            "master プロファイル 'p' が見つからない: /tmp/profiles/p.yaml",
        ),
        (
            ConfigError::ProfileNotFound {
                kind: Some(ProfileKind::Solo),
                name: "p".into(),
                path: "/tmp/solo-profiles/p.yaml".into(),
            },
            "solo プロファイル 'p' が見つからない: /tmp/solo-profiles/p.yaml",
        ),
        (
            ConfigError::ProfileExists {
                kind: ProfileKind::Master,
                name: "p".into(),
            },
            "master プロファイル 'p' は既に存在する（上書きするなら set を使う）",
        ),
        (
            ConfigError::DefaultProfileUndeletable(ProfileKind::Master),
            "既定プロファイル 'default' は削除できない（tako master が参照する）",
        ),
        (
            ConfigError::DefaultProfileUndeletable(ProfileKind::Solo),
            "既定プロファイル 'default' は削除できない（tako solo が参照する）",
        ),
        (
            ConfigError::InvalidKind("bogus".into()),
            "kind が不正: 'bogus'（master / solo）",
        ),
        (ConfigError::ProfileNameEmpty, "プロファイル名が空"),
        (
            ConfigError::ProfileNameLeadingDot("..".into()),
            "プロファイル名は '.' で始められない: '..'",
        ),
        (
            ConfigError::ProfileNameLeadingDash("-x".into()),
            "プロファイル名は '-' で始められない（起動コマンドのオプションと紛れる）: '-x'",
        ),
        (
            ConfigError::ProfileNameBadChar('/'),
            "プロファイル名に使えない文字が含まれる: '/'（英数字と - _ . のみ）",
        ),
        (
            ConfigError::AccountNotFound("a".into()),
            "アカウント 'a' が accounts.yaml に見つからない",
        ),
        (
            ConfigError::AccountInheritWithDir("a".into()),
            "アカウント 'a' は inherit: true と config_dir を同時に指定している\
             （既定の資格情報を使うなら config_dir を消す）",
        ),
        (
            ConfigError::AccountNoConfigDir("a".into()),
            "アカウント 'a' に config_dir が無い\
             （別 config dir なら config_dir を、既定の資格情報なら inherit: true を指定する）",
        ),
        (
            ConfigError::ProjectNotFound("k".into()),
            "プロジェクト 'k' が projects.yaml に見つからない",
        ),
        (
            ConfigError::ProjectCwdMissing("/x/k".into()),
            "cwd が存在しない: /x/k",
        ),
        (
            unknown(),
            "未対応のエージェント種別 'gpt'（対応: claude / codex / agy）",
        ),
        (
            ConfigError::MasterAgentUnsupported(WorkerAgent::Agy),
            "agy は master 非対応（master は claude / codex のみ。worker としては利用可能）",
        ),
        (
            ConfigError::ProfileMasterAgent(Box::new(unknown())),
            "プロファイルの master_agent が不正: 未対応のエージェント種別 'gpt'（対応: claude / codex / agy）",
        ),
        (
            ConfigError::ProfileMasterAgent(Box::new(ConfigError::MasterAgentUnsupported(
                WorkerAgent::Agy,
            ))),
            "プロファイルの master_agent が不正: agy は master 非対応（master は claude / codex のみ。worker としては利用可能）",
        ),
        (
            ConfigError::ProfileWorkerAgent(Box::new(unknown())),
            "プロファイルの worker_agent が不正: 未対応のエージェント種別 'gpt'（対応: claude / codex / agy）",
        ),
        (
            ConfigError::ProfileCwdMissing {
                expanded: "/h/x".into(),
                raw: "~/x".into(),
            },
            "プロファイルの cwd が存在しない: /h/x（元の指定: ~/x）",
        ),
        (
            ConfigError::ProfileProjectsUnregistered(vec!["a".into(), "b".into()]),
            "プロファイルの projects に projects.yaml に未登録のキーがある: a, b",
        ),
        (
            ConfigError::ProfileEnvReserved(vec!["TAKO_SOCKET".into(), "TAKO_TOKEN".into()]),
            "プロファイルの env に tako 内部変数が含まれている（上書き禁止）: TAKO_SOCKET, TAKO_TOKEN",
        ),
        (
            ConfigError::CtxThresholdOutOfRange(10),
            "ctx_threshold は 50〜60 の範囲で指定する: 10",
        ),
    ]
}

#[test]
fn orchestrator_の設定の断りは全バリアントが日英を持ち日本語は2365より前の固定文とバイト一致() {
    let table = config_error_table();
    let samples: Vec<ConfigError> = table.iter().map(|(e, _)| e.clone()).collect();
    covers_every(
        &samples,
        config_error_index,
        CONFIG_ERROR_VARIANTS,
        "ConfigError",
    );
    let mut failures = Vec::new();
    for (e, old) in &table {
        let (ja, en) = (e.text_in(Lang::Ja), e.text_in(Lang::En));
        if ja != *old {
            failures.push(format!(
                "{e:?}: 日本語が #2365 より前と違う\n  want: {old}\n  got:  {ja}"
            ));
        }
        if let Some(c) = has_japanese(&en) {
            failures.push(format!("{e:?}: 英語に日本語（{c}）が混ざる: {en}"));
        }
        if ja == en {
            failures.push(format!("{e:?}: 日英が同じ文（英語の書き忘れ）: {en}"));
        }
        // 日本語の口が使う文は日本語の文そのもの
        if e.clone().into_ja() != ja {
            failures.push(format!("{e:?}: into_ja が text_in(Ja) と違う"));
        }
    }
    report_all(failures);
}

/// 「見つからない」系は英語にも ` not found` を含む（リモートの HTTP 層が 404 へ寄せる判定を日英でそろえる）。
/// 包み（`ProfileMasterAgent` ほか）の枠は表示言語で、中身も同じ言語になる
#[test]
fn orchestrator_の設定の断りの見つからないの読みと包みの言語() {
    for e in [
        ConfigError::ProfileNotFound {
            kind: Some(ProfileKind::Solo),
            name: "p".into(),
            path: "/tmp/p.yaml".into(),
        },
        ConfigError::ProfileNotFound {
            kind: None,
            name: "p".into(),
            path: "/tmp/p.yaml".into(),
        },
        ConfigError::AccountNotFound("a".into()),
        ConfigError::ProjectNotFound("k".into()),
    ] {
        for lang in [Lang::Ja, Lang::En] {
            let text = e.text_in(lang);
            assert!(E::says_not_found(&text), "{lang:?}: {text}");
        }
    }
    let wrapped = ConfigError::ProfileWorkerAgent(Box::new(ConfigError::UnknownAgent("x".into())));
    assert_eq!(
        wrapped.text_in(Lang::En),
        "invalid worker_agent in the profile: unsupported agent kind 'x' (supported: claude / codex / agy)"
    );
    // dispatch へ載せる口は枠（invalid parameter:）と中身を同じ言語で組む
    assert_eq!(
        E::InvalidParams(wrapped.text_in(Lang::En)).text_in(Lang::En),
        "invalid parameter: invalid worker_agent in the profile: unsupported agent kind 'x' \
         (supported: claude / codex / agy)"
    );
}

/// 型の口と、それを日本語で包む従来の口の本体（日本語の固定文を書かない範囲。文は `ConfigError` にだけ置く）
const CONFIG_REFUSAL_SCOPES: &[(&str, &str, Span)] = &[
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("impl AccountsConfig {"),
    ),
    (ORCH_MOD_PATH, ORCH_MOD_SRC, Span::Item("fn resolve_entry(")),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("impl ProjectsConfig {"),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("    pub fn resolve_master_agent_typed("),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("    pub fn resolve_worker_agent_typed("),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("    pub fn resolve_cwd_typed(&self) ->"),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("    pub fn validate_projects_with("),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("    pub fn validate_env_typed("),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("    pub fn resolve_master_account_typed("),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("    pub fn resolved_env_plan_for_master_typed("),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("    pub fn load_typed(name: &str)"),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("    pub fn save(&self, name: &str)"),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("    pub fn mutate_named<"),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("    pub fn load_from_or_default_typed("),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("pub fn ensure_solo_defaults_typed("),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("fn solo_profile_path("),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("pub fn load_solo_profile("),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("fn read_profile_file("),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("fn write_profile_file("),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("fn mutate_profile_at<"),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("pub fn validate_master_agent_typed("),
    ),
    (ORCH_MOD_PATH, ORCH_MOD_SRC, Span::Item("fn profile_path(")),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("impl ProfileKind {"),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("pub fn validate_profile_name_typed("),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("pub fn list_profiles_of_typed("),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("pub fn load_profile_of_typed("),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("pub fn mutate_profile_of_typed<"),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("pub fn create_profile_of_typed("),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("pub fn create_profile_at("),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("pub fn delete_profile_of_typed("),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("pub fn delete_profile_at("),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("pub fn ensure_defaults_typed() ->"),
    ),
    (
        ORCH_AGENT_PATH,
        ORCH_AGENT_SRC,
        Span::Item("    pub fn parse_typed("),
    ),
    (
        CONFIG_ERROR_PATH,
        CONFIG_ERROR_SRC,
        Span::Item("    pub fn into_ja("),
    ),
    (
        CONFIG_ERROR_PATH,
        CONFIG_ERROR_SRC,
        Span::Item("    pub fn invalid("),
    ),
    (
        CONFIG_ERROR_PATH,
        CONFIG_ERROR_SRC,
        Span::Item("    pub fn operation("),
    ),
];

#[test]
fn orchestrator_の設定の口の本体に日本語の固定文が無い() {
    let mut failures = Vec::new();
    for (path, src, span) in CONFIG_REFUSAL_SCOPES {
        let Some(lines) = client_store_lines(src, span) else {
            failures.push(format!(
                "{path}: 走査の範囲 {} が見つからない（改名で番犬が空振りする。\
                 CONFIG_REFUSAL_SCOPES を直す）",
                span_name(span)
            ));
            continue;
        };
        for (line, text) in front_japanese(&lines) {
            failures.push(format!(
                "{path}:{line}: orchestrator の設定の口に日本語の固定文がある = 英語表示にもそのまま出る \
                 （orchestrator::ConfigError へバリアントを足し、日英の対を text_in に書く）: {text}"
            ));
        }
    }
    report_all(failures);
}

/// 断りを表示言語で返す dispatch の関数（orchestrator の設定を読み書きする所）
const CONFIG_DISPATCH_SCOPES: &[&str] = &[
    "pub fn dispatch_orchestrator_projects(",
    "pub fn dispatch_orchestrator_projects_mcp(",
    "fn dispatch_orchestrator_adopt(",
    "pub fn dispatch_orchestrator_profiles(",
    "pub fn dispatch_orchestrator_accounts(",
    "fn dispatch_orchestrator_handoff(",
    "fn dispatch_git_resolve_agent(",
    "fn dispatch_orchestrator_spawn(",
];

/// 日本語で返す口（型の口 `*_typed` の外側）。`.path(` は名前を取る `ProfileKind::path`
const JA_CONFIG_CALLS: [&str; 22] = [
    "ProjectsConfig::load()",
    "ProjectsConfig::mutate(",
    "AccountsConfig::load()",
    "AccountsConfig::mutate(",
    "accounts.resolve(",
    "config.resolve(",
    ".resolve_cwd(",
    ".resolve_worker_agent(",
    ".resolve_master_agent()",
    ".validate_env()",
    ".resolved_env_plan_for_master()",
    "Profile::load(",
    "load_profile_of(",
    "mutate_profile_of(",
    "create_profile_of(",
    "delete_profile_of(",
    "list_profiles_of(",
    "validate_master_agent(",
    "ProfileKind::parse(",
    "WorkerAgent::parse(",
    "ensure_defaults()",
    ".path(name)",
];

/// 文（呼び出しの行から `;` で終わる行まで）が断りを捨てている（`.ok()` / `unwrap_or` / `is_ok`）か
fn statement_discards_error(lines: &[(usize, &str)], at: usize) -> bool {
    let mut stmt = String::new();
    for (_, code) in &lines[at..] {
        stmt.push_str(code);
        if code.trim_end().ends_with(';') || code.trim_end().ends_with('{') {
            break;
        }
    }
    [".ok()", "unwrap_or", ".is_ok()", ".is_err()", "if let Ok("]
        .iter()
        .any(|d| stmt.contains(d))
}

/// dispatch の対象の関数の中で、日本語の口を呼んで断りに載せている行
fn ja_config_calls(lines: &[(usize, &str)]) -> Vec<(usize, &'static str, String)> {
    let mut out = Vec::new();
    for (i, (line, code)) in lines.iter().enumerate() {
        if code.trim_start().starts_with("//") {
            continue;
        }
        if let Some(call) = JA_CONFIG_CALLS.iter().find(|c| code.contains(*c)) {
            if !statement_discards_error(lines, i) {
                out.push((*line, *call, code.trim().to_string()));
            }
        }
    }
    out
}

#[test]
fn dispatch_は設定の断りを日本語の口から載せない() {
    let mut failures = Vec::new();
    for head in CONFIG_DISPATCH_SCOPES {
        let Some(lines) = front_scope_lines(DISPATCH_SRC, Some(head)) else {
            failures.push(format!(
                "{DISPATCH_PATH}: 走査の範囲 `{head}` が見つからない（関数の改名で番犬が空振りする。\
                 CONFIG_DISPATCH_SCOPES を直す）"
            ));
            continue;
        };
        for (line, call, code) in ja_config_calls(&lines) {
            failures.push(format!(
                "{DISPATCH_PATH}:{line}: 断りを表示言語で返す関数（`{head}`）が日本語の口 `{call}…)` の断りを \
                 載せている = 英語表示でも中身だけ日本語で混ざる（`*_typed` を呼んで ConfigError::operation / \
                 invalid で載せる）: {code}"
            ));
        }
    }
    report_all(failures);
}

/// 番犬自身の検査: 日本語の口を呼んで断りに載せた行だけを名指し、断りを捨てる文（`.ok()` /
/// `unwrap_or`）・型の口・コメントは見逃す
#[test]
fn dispatch_の設定の断りの番犬の境界() {
    let src = r#"fn a() -> Result<(), DispatchError> {
    // ProjectsConfig::load() はコメント
    let c = orchestrator::ProjectsConfig::load().map_err(DispatchError::Operation)?;
    let ok = orchestrator::ProjectsConfig::load()
        .map(|c| c.projects.len())
        .unwrap_or(0);
    let t = orchestrator::ProjectsConfig::load_typed().map_err(ConfigError::operation)?;
    let a = profile
        .resolve_worker_agent(agent)
        .map_err(DispatchError::InvalidParams)?;
    let b = profile.resolve_worker_agent_typed(agent).map_err(ConfigError::invalid)?;
}
"#;
    let lines = front_scope_lines(src, Some("fn a(")).expect("頭を引ける");
    let found: Vec<usize> = ja_config_calls(&lines)
        .into_iter()
        .map(|(l, _, _)| l)
        .collect();
    assert_eq!(found, vec![3, 9], "日本語の口の 2 か所だけを名指す");
}

// ---- #2365 の続き: GUI 側の口（ControlHost の実装）が組む断り（`HostRefusal`） ----
//
// dispatch は `host.*` の `Err(String)` を `Operation` / `InvalidParams` の中身としてそのまま載せる。
// GUI（tako-app の `impl *Host for TakoApp` と、そこから呼ぶ補助）が文を日本語で固定すると、英語表示でも
// 理由だけ日本語で返る。日英の対は `tako_control::HostRefusal` に置き、ここで
// (1) 表（全バリアントが日英・日本語は #2365 より前の固定文とバイト一致・見つからないの読み）と
// (2) GUI のソースを読んで、日本語の断りの書き足しを `file:line` で名指す。
// tako-app はバイナリ crate なので型は引けない = ソースをファイルとして読む

use tako_control::HostRefusal;

const HOST_REFUSAL_SRC: &str = include_str!("../src/host_refusal.rs");
const HOST_REFUSAL_PATH: &str = "crates/tako-control/src/host_refusal.rs";

/// バリアントの通し番号。**網羅 match**なので、バリアントを足すとここがコンパイルで落ちる
/// = [`host_refusal_table`] に旧文（か新しい日本語）を足すまで通らない
fn host_refusal_index(r: &HostRefusal) -> usize {
    match r {
        HostRefusal::NotPreviewPane => 0,
        HostRefusal::ZoomNeedsPdfOrImage => 1,
        HostRefusal::PageOutOfRange { .. } => 2,
        HostRefusal::ImagePageOnlyOne => 3,
        HostRefusal::LinesUncountable => 4,
        HostRefusal::LineMissing { .. } => 5,
        HostRefusal::NotCodePreview => 6,
        HostRefusal::NotMarkdownOutline => 7,
        HostRefusal::NotPdfOutline => 8,
        HostRefusal::NotMarkdownPreview => 9,
        HostRefusal::NotPdfPreview => 10,
        HostRefusal::LinkIndexOutOfRange { .. } => 11,
        HostRefusal::LinkNotBrowsable(_) => 12,
        HostRefusal::NoCodeBlock => 13,
        HostRefusal::CodeBlockOutOfRange { .. } => 14,
        HostRefusal::EditNotStarted => 15,
        HostRefusal::EditingOff => 16,
        HostRefusal::NotTextForEdit => 17,
        HostRefusal::BinaryNotEditable => 18,
        HostRefusal::SearchBarNeedsText => 19,
        HostRefusal::ChangelogOff => 20,
        HostRefusal::CommitNotFound(_) => 21,
        HostRefusal::NoPreviewInPane => 22,
        HostRefusal::VideoPlayerNotRunning => 23,
        HostRefusal::BadRate(_) => 24,
        HostRefusal::RateOutOfRange(_) => 25,
        HostRefusal::UnknownVideoAction(_) => 26,
        HostRefusal::WebViewNotFound { .. } => 27,
        HostRefusal::WebViewShown(_) => 28,
        HostRefusal::NoWebViewInPane(_) => 29,
        HostRefusal::NoShownWebView => 30,
        HostRefusal::ManyShownWebViews(_) => 31,
        HostRefusal::WindowNotReady => 32,
        HostRefusal::SettingsNotSaved(_) => 33,
        HostRefusal::NoBackendSession(_) => 34,
        HostRefusal::AttachClientAlive(_) => 35,
        HostRefusal::BackendSessionEnded { .. } => 36,
        HostRefusal::NoPendingPaste => 37,
        HostRefusal::PasteTargetClosed(_) => 38,
        HostRefusal::NotTerminalPane(_) => 39,
        HostRefusal::NotChatPane => 40,
        HostRefusal::ChatNotLoaded => 41,
        HostRefusal::NoAssistantMessage => 42,
        HostRefusal::MessageOutOfRange { .. } => 43,
        HostRefusal::MessageGone => 44,
        HostRefusal::NoCodeBlockInMessage => 45,
        HostRefusal::NothingToCopy => 46,
        HostRefusal::FileTreeClosed => 47,
    }
}
const HOST_REFUSAL_VARIANTS: usize = 48;

/// (断り, #2365 より前に GUI が組んでいた日本語の固定文)。中身で腕を分けるもの
/// （`LinkIndexOutOfRange` の total・`WebViewNotFound` の案内）は両方を載せる
fn host_refusal_table() -> Vec<(HostRefusal, String)> {
    use HostRefusal as H;
    vec![
        (H::NotPreviewPane, "プレビューペインではない".into()),
        (
            H::ZoomNeedsPdfOrImage,
            "ズーム操作は PDF・画像プレビューだけに対応する".into(),
        ),
        (
            H::PageOutOfRange { page: 9, total: 3 },
            "ページ範囲外: 9（全 3 ページ）".into(),
        ),
        (
            H::ImagePageOnlyOne,
            "画像プレビューの page は 1 だけ指定できる".into(),
        ),
        (H::LinesUncountable, "テキストとして行を数えられない".into()),
        (
            H::LineMissing { line: 12, count: 4 },
            "12 行目は無い（全 4 行）".into(),
        ),
        (
            H::NotCodePreview,
            "コード表示のプレビューではない（Markdown のレンダリング表示・画像・PDF には原文の行が無い）"
                .into(),
        ),
        (H::NotMarkdownOutline, "Markdown アウトラインの対象ではない".into()),
        (H::NotPdfOutline, "PDF アウトラインの対象ではない".into()),
        (H::NotMarkdownPreview, "Markdown プレビューではない".into()),
        (H::NotPdfPreview, "PDF プレビューではない".into()),
        (
            H::LinkIndexOutOfRange {
                index: 7,
                total: Some(2),
            },
            "リンクインデックス範囲外: 7（全 2 件）".into(),
        ),
        (
            H::LinkIndexOutOfRange {
                index: 7,
                total: None,
            },
            "リンクインデックス範囲外: 7".into(),
        ),
        (
            H::LinkNotBrowsable("file:///etc/hosts".into()),
            "ブラウザで開けないリンク（http / https のみ対応）: file:///etc/hosts".into(),
        ),
        (H::NoCodeBlock, "コードブロックが無い".into()),
        (
            H::CodeBlockOutOfRange { index: 5, total: 2 },
            "コードブロック範囲外: 5（全 2 個。0 始まり）".into(),
        ),
        (H::EditNotStarted, "編集モードを開始していない".into()),
        (H::EditingOff, "編集モードが無効".into()),
        (
            H::NotTextForEdit,
            "テキスト以外のプレビューは編集できない".into(),
        ),
        (H::BinaryNotEditable, "バイナリファイルは編集できない".into()),
        (
            H::SearchBarNeedsText,
            "テキスト以外のプレビュー（PDF・画像・動画）は検索欄を開けない".into(),
        ),
        (H::ChangelogOff, "チェンジログビューが有効ではない".into()),
        (
            H::CommitNotFound("nope".into()),
            "コミット nope が見つからない".into(),
        ),
        (H::NoPreviewInPane, "対象ペインにプレビューが無い".into()),
        (H::VideoPlayerNotRunning, "動画プレイヤーが起動していない".into()),
        (H::BadRate("fast".into()), "不正な速度値: fast".into()),
        (H::RateOutOfRange(9.5), "速度は 0.1〜4.0 の範囲: 9.5".into()),
        (
            H::UnknownVideoAction("rewind".into()),
            "不明なアクション: rewind".into(),
        ),
        (
            H::WebViewNotFound {
                id: 4,
                list_hint: true,
            },
            "Web ビュー 4 が見つからない（web list で確認）".into(),
        ),
        (
            H::WebViewNotFound {
                id: 4,
                list_hint: false,
            },
            "Web ビュー 4 が見つからない".into(),
        ),
        (H::WebViewShown(4), "Web ビュー 4 は表示中".into()),
        (
            H::NoWebViewInPane(8),
            "ペイン 8 に Web ビューは表示されていない".into(),
        ),
        (
            H::NoShownWebView,
            "表示中の Web ビューが無い（id か pane で指定）".into(),
        ),
        (
            H::ManyShownWebViews(3),
            "表示中の Web ビューが 3 個ある（id か pane で指定）".into(),
        ),
        (
            H::WindowNotReady,
            "ウィンドウ初期化前のため Web ビューを作れない（直後に再試行）".into(),
        ),
        (
            H::SettingsNotSaved(IoCause::Other("Permission denied (os error 13)".into())),
            "設定を保存できない: Permission denied (os error 13)".into(),
        ),
        (
            H::SettingsNotSaved(IoCause::DataDirUnresolved),
            format!(
                "設定を保存できない: {}",
                Reason::DataDirUnresolved.text_in(Lang::Ja)
            ),
        ),
        (
            H::NoBackendSession(6),
            "ペイン 6 は tmux のセッションを持たない（再 attach できるのは tmux 永続化のペインだけ）"
                .into(),
        ),
        (
            H::AttachClientAlive(6),
            "ペイン 6 の attach クライアントは動いている（再 attach は不要）".into(),
        ),
        (
            H::BackendSessionEnded {
                pane: 6,
                name: "tako-6".into(),
            },
            "ペイン 6 のセッション tako-6 は終わっている（再 attach できない。ペインは閉じてよい）"
                .into(),
        ),
        (H::NoPendingPaste, "確認待ちの貼り付けが無い".into()),
        (
            H::PasteTargetClosed(2),
            "送り先のペイン 2 が閉じている（貼り付けは捨てた）".into(),
        ),
        (H::NotTerminalPane(2), "ペイン 2 は端末ではない".into()),
        (H::NotChatPane, "チャット表示のペインではない".into()),
        (H::ChatNotLoaded, "会話がまだ読めていない".into()),
        (
            H::NoAssistantMessage,
            "assistant の発話がまだ無い（--message で指定できる）".into(),
        ),
        (
            H::MessageOutOfRange { index: 9, total: 3 },
            "メッセージ範囲外: 9（全 3 件。0 始まり）".into(),
        ),
        (H::MessageGone, "その発話はもう表示されていない".into()),
        (H::NoCodeBlockInMessage, "この発話にコードブロックが無い".into()),
        (H::NothingToCopy, "コピーする本文が無い".into()),
        (H::FileTreeClosed, "ファイルツリーが閉じている".into()),
    ]
}

/// `HostRefusal` の日英の対を書いた行
fn host_refusal_where(r: &HostRefusal, lang: Lang) -> String {
    where_written(
        HOST_REFUSAL_SRC,
        HOST_REFUSAL_PATH,
        "impl HostRefusal {",
        &format!("{r:?}"),
        lang,
    )
}

#[test]
fn gui_側の口の断りは全バリアントが日英を持ち日本語は2365より前の固定文とバイト一致() {
    let table = host_refusal_table();
    let samples: Vec<HostRefusal> = table.iter().map(|(r, _)| r.clone()).collect();
    covers_every(
        &samples,
        host_refusal_index,
        HOST_REFUSAL_VARIANTS,
        "HostRefusal",
    );
    let mut failures = Vec::new();
    for (r, old) in &table {
        let (ja, en) = (r.text_in(Lang::Ja), r.text_in(Lang::En));
        if ja != *old {
            failures.push(format!(
                "{}: {r:?} の日本語が #2365 より前と違う\n  want: {old}\n  got:  {ja}",
                host_refusal_where(r, Lang::Ja)
            ));
        }
        if let Some(c) = has_japanese(&en) {
            failures.push(format!(
                "{}: {r:?} の英語に日本語（{c}）が混ざる: {en}",
                host_refusal_where(r, Lang::En)
            ));
        }
        if en.trim().is_empty() || ja == en {
            failures.push(format!(
                "{}: {r:?} の英語が空か日本語と同じ文（英語の書き忘れ）: {en}",
                host_refusal_where(r, Lang::En)
            ));
        }
        // GUI は `String::from` / `?` で載せる = 組んだその場の表示言語の文（言語のグローバルは
        // `display_follows_the_display_language` が動かすので、日英のどちらかであることだけを見る）
        let shown = String::from(r.clone());
        if shown != ja && shown != en {
            failures.push(format!(
                "{r:?}: String への変換が日英どちらの文とも違う: {shown}"
            ));
        }
    }
    report_all(failures);
}

/// 「見つからない」系は英語にも ` not found` を含む（リモートの HTTP 層が 404 へ寄せる判定を日英でそろえる）。
/// 設定の保存の断りは中身（データディレクトリを解決できない）も枠と同じ言語になる
#[test]
fn gui_側の口の断りの見つからないの読みと中身の言語() {
    let not_found = [
        HostRefusal::CommitNotFound("abc".into()),
        HostRefusal::WebViewNotFound {
            id: 1,
            list_hint: true,
        },
        HostRefusal::WebViewNotFound {
            id: 1,
            list_hint: false,
        },
    ];
    for r in &not_found {
        for lang in [Lang::Ja, Lang::En] {
            let text = r.text_in(lang);
            assert!(E::says_not_found(&text), "{lang:?}: {text}");
        }
    }
    // 見つからない系でないものは英語でも当たらない（404 へ寄せすぎない）
    for (r, _) in host_refusal_table() {
        if matches!(
            r,
            HostRefusal::CommitNotFound(_) | HostRefusal::WebViewNotFound { .. }
        ) {
            continue;
        }
        for lang in [Lang::Ja, Lang::En] {
            let text = r.text_in(lang);
            assert!(!E::says_not_found(&text), "{r:?} {lang:?}: {text}");
        }
    }
    let unresolved = HostRefusal::SettingsNotSaved(IoCause::DataDirUnresolved).text_in(Lang::En);
    assert!(has_japanese(&unresolved).is_none(), "{unresolved}");
    assert!(
        unresolved.starts_with("cannot save the settings: "),
        "{unresolved}"
    );
    // dispatch が載せる枠（operation failed: など）と中身が同じ言語でそろう
    let wrapped = E::Operation(HostRefusal::EditNotStarted.text_in(Lang::En)).text_in(Lang::En);
    assert!(has_japanese(&wrapped).is_none(), "{wrapped}");
}

/// GUI 側の口の本体（tako-app）: (path, 関数か impl の頭)。頭から、頭と同じ字下げの `}` まで
const HOST_IMPL_SCOPES: &[(&str, &str)] = &[
    (
        "crates/tako-app/src/main.rs",
        "impl WorkspaceHost for TakoApp {",
    ),
    (
        "crates/tako-app/src/main.rs",
        "impl SessionHost for TakoApp {",
    ),
    ("crates/tako-app/src/main.rs", "impl TmuxHost for TakoApp {"),
    (
        "crates/tako-app/src/main.rs",
        "impl UiStateHost for TakoApp {",
    ),
    (
        "crates/tako-app/src/main.rs",
        "impl PreviewHost for TakoApp {",
    ),
    (
        "crates/tako-app/src/main.rs",
        "impl RemoteHost for TakoApp {",
    ),
    (
        "crates/tako-app/src/main.rs",
        "impl WebViewHost for TakoApp {",
    ),
    (
        "crates/tako-app/src/main.rs",
        "impl SystemHost for TakoApp {",
    ),
    // 口がそのまま委ねる補助（断りの文は口の断りとして CLI / MCP へ届く）
    (
        "crates/tako-app/src/main.rs",
        "    fn answer_pending_paste(",
    ),
    (
        "crates/tako-app/src/main.rs",
        "    fn set_preview_editing_local(",
    ),
    (
        "crates/tako-app/src/main.rs",
        "    fn apply_preview_text_local(",
    ),
    (
        "crates/tako-app/src/main.rs",
        "    fn edit_preview_range_local(",
    ),
    (
        "crates/tako-app/src/main.rs",
        "    fn edit_preview_ranges_local(",
    ),
    (
        "crates/tako-app/src/main.rs",
        "    fn set_preview_cursor_local(",
    ),
    ("crates/tako-app/src/main.rs", "    fn save_preview_local("),
    (
        "crates/tako-app/src/main.rs",
        "    fn revert_preview_local(",
    ),
    (
        "crates/tako-app/src/main.rs",
        "    fn push_preview_remote_sync(",
    ),
    ("crates/tako-app/src/main.rs", "    fn preview_undo_local("),
    ("crates/tako-app/src/main.rs", "    fn preview_redo_local("),
    (
        "crates/tako-app/src/main.rs",
        "    fn open_preview_search_bar(",
    ),
    (
        "crates/tako-app/src/main.rs",
        "    fn preview_search_local(",
    ),
    (
        "crates/tako-app/src/main.rs",
        "    fn preview_replace_local(",
    ),
    (
        "crates/tako-app/src/main.rs",
        "    fn run_preview_command_local(",
    ),
    ("crates/tako-app/src/main.rs", "    fn ensure_video_player("),
    ("crates/tako-app/src/main.rs", "    fn create_webview("),
    (
        "crates/tako-app/src/chat_view.rs",
        "    pub(crate) fn chat_copy_dispatch(",
    ),
    (
        "crates/tako-app/src/chat_view.rs",
        "    pub(crate) fn copy_chat_message(",
    ),
    (
        "crates/tako-app/src/click_forward.rs",
        "    pub(crate) fn click_direct_pane(",
    ),
    (
        "crates/tako-app/src/sidebar.rs",
        "    pub(crate) fn host_set_tree_selection(",
    ),
    (
        "crates/tako-app/src/preview.rs",
        "    pub fn open(preview: &PreviewState)",
    ),
    (
        "crates/tako-app/src/preview.rs",
        "pub(crate) fn search_jump<K>(",
    ),
];

/// 断りではない日本語の呼び出し（診断の出力 = persist.log / stderr は日本語のまま。内部の印も）
const HOST_INTERNAL_CALLS: [&str; 6] = [
    "unreachable!(",
    ".expect(",
    "eprintln!(",
    "persist_diag(",
    "persist_log(",
    "webview_parent_diag(",
];

/// まだ移していない断りと、断りではない応答の注記（文の断片, 理由）。**断片がソースから消えたら
/// ここから外す**（下の番犬が「古い」と落とす = 表が縮む方向にしか動かない）
const HOST_DEFERRED: [(&str, &str); 1] = [(
    "結果は action=eval_result で回収",
    "web_eval の成功の応答の注記（断りではない）",
)];

/// `start..` の範囲（頭と同じ字下げの `}` まで）。tako-app のソースは途中に `#[cfg(test)] mod` の
/// 宣言を持つので、[`front_scope_lines`] の「単体テストの前まで」の切り方は使わない
fn item_lines<'a>(src: &'a str, head: &str) -> Option<Vec<(usize, &'a str)>> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(head))?;
    let indent = &lines[start][..lines[start].len() - lines[start].trim_start().len()];
    let close = format!("{indent}}}");
    let len = lines[start..].iter().position(|l| *l == close)?;
    Some(
        lines[start..=start + len]
            .iter()
            .enumerate()
            .map(|(i, l)| (start + i + 1, *l))
            .collect(),
    )
}

/// GUI 側の口の走査範囲の中で、断りに書かれた日本語の固定文の行（診断の出力・内部の印・
/// [`HOST_DEFERRED`] は外す）
fn host_japanese(lines: &[(usize, &str)]) -> Vec<(usize, String)> {
    japanese_outside(lines, &HOST_INTERNAL_CALLS)
        .into_iter()
        .filter(|(_, text)| !HOST_DEFERRED.iter().any(|(frag, _)| text.contains(frag)))
        .collect()
}

fn read_repo_file(path: &str) -> String {
    let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path);
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("{path} が読めない: {e}"))
}

#[test]
fn gui_側の口の本体に日本語の断りの固定文が無い() {
    let mut failures = Vec::new();
    let mut deferred_seen = [false; HOST_DEFERRED.len()];
    for (path, head) in HOST_IMPL_SCOPES {
        let src = read_repo_file(path);
        let Some(lines) = item_lines(&src, head) else {
            failures.push(format!(
                "{path}: 走査の範囲 `{head}` が見つからない（改名で番犬が空振りする。HOST_IMPL_SCOPES を直す）"
            ));
            continue;
        };
        for (line, text) in host_japanese(&lines) {
            failures.push(format!(
                "{path}:{line}: GUI 側の口の断りに日本語の固定文がある = dispatch がそのまま載せ、英語表示にも出る \
                 （tako_control::HostRefusal へバリアントを足し、日英の対を text_in に書く）: {text}"
            ));
        }
        for (i, (frag, _)) in HOST_DEFERRED.iter().enumerate() {
            if lines.iter().any(|(_, code)| code.contains(frag)) {
                deferred_seen[i] = true;
            }
        }
    }
    for (i, (frag, why)) in HOST_DEFERRED.iter().enumerate() {
        if !deferred_seen[i] {
            failures.push(format!(
                "HOST_DEFERRED の `{frag}`（{why}）がもうソースに無い = 移し終えたので表から外す"
            ));
        }
    }
    report_all(failures);
}

/// 番犬自身の検査: 植えた日本語の断りを名指し、診断の出力（行をまたぐ `eprintln!` も）・`expect`・
/// コメント・[`HOST_DEFERRED`] は見逃す。範囲は頭と同じ字下げの `}` までで、隣へはみ出さない
#[test]
fn gui_側の口の番犬は植えた日本語を名指しし診断と注記は見逃す() {
    let src = r#"impl PreviewHost for TakoApp {
    fn a(&mut self) -> Result<(), String> {
        // コメントの日本語は見ない
        eprintln!(
            "warning: 設定を保存できない: {e}"
        );
        let p = self.previews.get(&pane).expect("確認済み");
        if p.is_none() {
            return Err("プレビューペインではない".into());
        }
        let t = codes.get(i).ok_or(HostRefusal::NoCodeBlock)?;
        Err(format!("{n} 件が範囲外"))
    }
    fn b(&self) -> Value {
        json!({ "hint": "結果は action=eval_result で回収（未完なら pending: true）" })
    }
}
fn outside() {
    let _ = "範囲の外";
}
"#;
    let lines = item_lines(src, "impl PreviewHost for TakoApp {").expect("頭を引ける");
    let found: Vec<usize> = host_japanese(&lines).into_iter().map(|(l, _)| l).collect();
    assert_eq!(found, vec![9, 12], "植えた 2 か所だけを名指す");
    assert!(item_lines(src, "impl Absent for TakoApp {").is_none());
}

// ---- #2365 の続き: 会話のカタログ・会話の再開・端末ログの断り（`SessionRefusal`）と、OS の設定・統合の
// 操作（フルディスクアクセス・スリープ防止 / 蓋閉じ継続・シェル統合）の断り（`OsRefusal` + tako-core の
// `shell_integration::SiError`） ----
//
// dispatch の `sessions` / `logs` / 会話を保った再起動の下見と `tako_fda` / `tako_sleep_guard` /
// `tako_shell_integration` は、これらの口の `Err(String)`（日本語の固定文）を素通ししていた。日英の対は
// 3 つの型の `text_in` の網羅 match に置き、従来の `String` の口は日本語のまま（persist.log・復元の内訳・
// CLI の手元の表示）、dispatch だけが型の口（`*_typed`）を通す。ここでは
// (1) 全バリアントの日英と旧文のバイト一致、(2) 型の口の本体の断りの組み立て（`Err(` / `.map_err(` /
// `.ok_or(`）に日本語の固定文を書き足していないこと、(3) dispatch が日本語の口を呼んで断りに載せて
// いないことを `file:line` で名指す。成功の応答の注記（`Ok` の文・warnings・osascript のスクリプト）は
// 断りではないので見ない

use tako_control::platform::lid::{LidError, Rail};
use tako_control::platform::os_integration::{Launch, OsFeature, OsIntegrationError};
use tako_control::{OsRefusal, SessionRefusal};
use tako_core::shell_integration::{Delivery, SiError};

const SESSIONS_SRC: &str = include_str!("../src/sessions.rs");
const SESSIONS_PATH: &str = "crates/tako-control/src/sessions.rs";
const RESUME_LAUNCH_SRC: &str = include_str!("../src/resume_launch.rs");
const RESUME_LAUNCH_PATH: &str = "crates/tako-control/src/resume_launch.rs";
const WORKER_MCP_SRC: &str = include_str!("../src/worker_mcp.rs");
const WORKER_MCP_PATH: &str = "crates/tako-control/src/worker_mcp.rs";
const FDA_SRC: &str = include_str!("../src/fda.rs");
const FDA_PATH: &str = "crates/tako-control/src/fda.rs";
const SLEEP_GUARD_SRC: &str = include_str!("../src/sleep_guard.rs");
const SLEEP_GUARD_PATH: &str = "crates/tako-control/src/sleep_guard.rs";
const SHELL_INTEGRATION_SRC: &str = include_str!("../src/shell_integration.rs");
const SHELL_INTEGRATION_PATH: &str = "crates/tako-control/src/shell_integration.rs";
const CORE_SHELL_INTEGRATION_SRC: &str = include_str!("../../tako-core/src/shell_integration.rs");
const CORE_SHELL_INTEGRATION_PATH: &str = "crates/tako-core/src/shell_integration.rs";
const SESSION_REFUSAL_SRC: &str = include_str!("../src/session_refusal.rs");
const SESSION_REFUSAL_PATH: &str = "crates/tako-control/src/session_refusal.rs";
const OS_REFUSAL_SRC: &str = include_str!("../src/os_refusal.rs");
const OS_REFUSAL_PATH: &str = "crates/tako-control/src/os_refusal.rs";

/// `SessionRefusal` の通し番号（網羅 match = バリアントを足すとここがコンパイルで落ちる）
fn session_refusal_index(r: &SessionRefusal) -> usize {
    match r {
        SessionRefusal::CatalogHomeUnresolved => 0,
        SessionRefusal::CatalogRead(_) => 1,
        SessionRefusal::CatalogParse(_) => 2,
        SessionRefusal::SessionNotFound(_) => 3,
        SessionRefusal::SessionAmbiguous { .. } => 4,
        SessionRefusal::InvalidSessionId => 5,
        SessionRefusal::ResumeUnsupported(_) => 6,
        SessionRefusal::TranscriptMissing(_) => 7,
        SessionRefusal::Config(_) => 8,
        SessionRefusal::MasterAgentNotClaude { .. } => 9,
        SessionRefusal::PromptDir(_) => 10,
        SessionRefusal::PromptWrite(_) => 11,
        SessionRefusal::McpSettingsDir { .. } => 12,
        SessionRefusal::McpSettingsWrite(_) => 13,
        SessionRefusal::LogUnreadable { .. } => 14,
    }
}
const SESSION_REFUSAL_VARIANTS: usize = 15;

/// 見本と #2365 より前の固定文（sessions / resume_launch / worker_mcp / pane_log が書いていた文を見本の値で展開）
fn session_refusal_table() -> Vec<(SessionRefusal, String)> {
    let yaml = YamlReason::Located {
        kind: YamlProblem::Syntax,
        at: Some((1, 11)),
    };
    vec![
        (
            SessionRefusal::CatalogHomeUnresolved,
            "ホームディレクトリが取得できない".into(),
        ),
        (
            SessionRefusal::CatalogRead("Permission denied (os error 13)".into()),
            "sessions.yaml の読み取りに失敗: Permission denied (os error 13)".into(),
        ),
        (
            SessionRefusal::CatalogParse(yaml.clone()),
            format!("sessions.yaml のパースに失敗: {}", yaml.text_in(Lang::Ja)),
        ),
        (
            SessionRefusal::SessionNotFound("nope".into()),
            "セッション 'nope' がカタログに見つからない（tako sessions list で確認）".into(),
        ),
        (
            SessionRefusal::SessionAmbiguous {
                prefix: "aaaa".into(),
                count: 2,
                candidates: vec!["aaaa1111".into(), "aaaa2222".into()],
            },
            "セッション 'aaaa' の候補が 2 件ある（もう少し長い ID を指定）: aaaa1111, aaaa2222"
                .into(),
        ),
        (
            SessionRefusal::InvalidSessionId,
            "session_id の形式が不正".into(),
        ),
        (
            SessionRefusal::ResumeUnsupported("local".into()),
            "agent 'local' のセッションは resume 非対応（対応: claude / codex / agy）".into(),
        ),
        (
            SessionRefusal::TranscriptMissing("aaaa1111".into()),
            "会話 aaaa1111 の記録（claude の transcript）が見つからない。\
             削除されたか、登録されていない設定 dir にある（`tako orchestrator accounts list`）"
                .into(),
        ),
        (
            SessionRefusal::Config(ConfigError::ProfileEnvReserved(vec!["TAKO_SOCKET".into()])),
            "プロファイルの env に tako 内部変数が含まれている（上書き禁止）: TAKO_SOCKET".into(),
        ),
        (
            SessionRefusal::MasterAgentNotClaude {
                profile: "codexm".into(),
                agent: "codex".into(),
            },
            "プロファイル 'codexm' のエージェントは codex なので claude の会話を\
             その組み立てで再開できない（`tako sessions resume <id>` で開く）"
                .into(),
        ),
        (
            SessionRefusal::PromptDir("Not a directory (os error 20)".into()),
            "system prompt の保存先を作れない: Not a directory (os error 20)".into(),
        ),
        (
            SessionRefusal::PromptWrite("Is a directory (os error 21)".into()),
            "system prompt の書き出しに失敗: Is a directory (os error 21)".into(),
        ),
        (
            SessionRefusal::McpSettingsDir {
                dir: "/tmp/x2365".into(),
                error: "Permission denied (os error 13)".into(),
            },
            "/tmp/x2365 を作れない: Permission denied (os error 13)".into(),
        ),
        (
            SessionRefusal::McpSettingsWrite(WriteError::NoParent("/x2365".into())),
            "親ディレクトリがない: /x2365".into(),
        ),
        (
            SessionRefusal::LogUnreadable {
                path: "/tmp/c1.log".into(),
                error: "Permission denied (os error 13)".into(),
            },
            "ログを読めない (/tmp/c1.log): Permission denied (os error 13)".into(),
        ),
    ]
}

/// `OsRefusal` の通し番号
fn os_refusal_index(r: &OsRefusal) -> usize {
    match r {
        OsRefusal::FdaPanelUnopenable => 0,
        OsRefusal::FdaMacOnly => 1,
        OsRefusal::BatteryFloorOutOfRange(_) => 2,
        OsRefusal::LidUnsupported => 3,
        OsRefusal::OsascriptFailed(_) => 4,
        OsRefusal::UserCanceled => 5,
        OsRefusal::SudoersInstallFailed(_) => 6,
        OsRefusal::UnknownShellIntegrationAction(_) => 7,
        OsRefusal::ShellIntegration(_) => 8,
        OsRefusal::SudoersRemoveFailed(_) => 9,
        OsRefusal::LidTeardownFailed(_) => 10,
        OsRefusal::BatterySettingsUnopenable(_) => 11,
        OsRefusal::BatterySettingsMacOnly => 12,
        OsRefusal::Integration(_) => 13,
    }
}
const OS_REFUSAL_VARIANTS: usize = 14;

/// `SiError`（tako-core）の通し番号
fn si_error_index(e: &SiError) -> usize {
    match e {
        SiError::WriteScripts(_) => 0,
        SiError::NothingToUninstall(Delivery::Automatic) => 1,
        SiError::NothingToUninstall(Delivery::Launch) => 2,
        SiError::ProfileUnreadable { .. } => 3,
        SiError::BackupFailed { .. } => 4,
        SiError::ProfileDeleteFailed { .. } => 5,
        SiError::ProfileRewriteFailed { .. } => 6,
    }
}
const SI_ERROR_VARIANTS: usize = 7;

fn si_error_table() -> Vec<(SiError, &'static str)> {
    let at = |path: &str| {
        (
            path.to_string(),
            "Permission denied (os error 13)".to_string(),
        )
    };
    let (p, e) = at("C:/Users/winuser/Documents/PowerShell/profile.ps1");
    vec![
        (
            SiError::WriteScripts("Not a directory (os error 20)".into()),
            "統合スクリプトを書き出せません: Not a directory (os error 20)",
        ),
        (
            SiError::NothingToUninstall(Delivery::Automatic),
            "この環境のシェル統合は環境変数の注入だけで完結するため、解除する配置がありません",
        ),
        (
            SiError::NothingToUninstall(Delivery::Launch),
            "この環境のシェル統合は tako がペインの PowerShell を起こすときの起動引数で読ませるため、\
             解除する配置がありません（以前の版が $PROFILE に置いた管理ブロックも残っていません）",
        ),
        (
            SiError::ProfileUnreadable {
                path: p.clone(),
                error: e.clone(),
            },
            "C:/Users/winuser/Documents/PowerShell/profile.ps1 を読めません: Permission denied (os error 13)",
        ),
        (
            SiError::BackupFailed {
                path: format!("{p}.pre-v1.bak"),
                error: e.clone(),
            },
            "退避に失敗したので取り除かない（C:/Users/winuser/Documents/PowerShell/profile.ps1.pre-v1.bak）: \
             Permission denied (os error 13)",
        ),
        (
            SiError::ProfileDeleteFailed {
                path: p.clone(),
                error: e.clone(),
            },
            "C:/Users/winuser/Documents/PowerShell/profile.ps1 を削除できません: Permission denied (os error 13)",
        ),
        (
            SiError::ProfileRewriteFailed { path: p, error: e },
            "C:/Users/winuser/Documents/PowerShell/profile.ps1 を書き換えられません: Permission denied (os error 13)",
        ),
    ]
}

fn os_refusal_table() -> Vec<(OsRefusal, &'static str)> {
    vec![
        (
            OsRefusal::FdaPanelUnopenable,
            "システム設定のフルディスクアクセスパネルを開けませんでした",
        ),
        (
            OsRefusal::FdaMacOnly,
            "フルディスクアクセスは macOS 専用の機能です",
        ),
        (
            OsRefusal::BatteryFloorOutOfRange(1),
            "残量下限は 5〜90 の範囲で指定してください（指定値: 1）",
        ),
        (
            OsRefusal::LidUnsupported,
            "この OS では蓋閉じ継続に対応していません",
        ),
        (
            OsRefusal::OsascriptFailed("No such file or directory (os error 2)".into()),
            "osascript の実行に失敗: No such file or directory (os error 2)",
        ),
        (OsRefusal::UserCanceled, "ユーザーがキャンセルしました"),
        (
            OsRefusal::SudoersInstallFailed("ERROR: visudo".into()),
            "sudoers 登録に失敗: ERROR: visudo",
        ),
        (
            OsRefusal::UnknownShellIntegrationAction("bogus".into()),
            "不明な action: \"bogus\"（status / install / uninstall のいずれか）",
        ),
        (
            OsRefusal::ShellIntegration(SiError::NothingToUninstall(Delivery::Automatic)),
            "この環境のシェル統合は環境変数の注入だけで完結するため、解除する配置がありません",
        ),
        // ---- #2365 の続き: 枠と中身（`platform::lid` / `os_integration`）を一緒に移したもの ----
        (
            OsRefusal::SudoersRemoveFailed("ERROR: rm".into()),
            "sudoers 削除に失敗: ERROR: rm",
        ),
        (
            OsRefusal::LidTeardownFailed(LidError::WriteSetting {
                rail: Rail::Ac,
                rc: 5,
            }),
            "蓋閉じ継続の解除に失敗: 蓋の設定を書けません（Ac, rc=5）",
        ),
        (
            OsRefusal::BatterySettingsUnopenable(OsIntegrationError::Spawn {
                what: Launch::Url,
                error: "No such file or directory (os error 2)".into(),
            }),
            "System Settings を開けません: URL を開けない: No such file or directory (os error 2)",
        ),
        (OsRefusal::BatterySettingsMacOnly, "macOS 以外では非対応"),
        (
            OsRefusal::Integration(OsIntegrationError::TrashFailed(
                "No such file or directory (os error 2)".into(),
            )),
            "ゴミ箱への移動に失敗: No such file or directory (os error 2)",
        ),
    ]
}

/// 表の 1 行の検査（日本語は旧文とバイト一致・英語に日本語が無い・日英が違う・`into_ja` は日本語の文）
fn check_refusal_row(
    failures: &mut Vec<String>,
    what: String,
    old: &str,
    ja: String,
    en: String,
    into_ja: String,
) {
    if ja != old {
        failures.push(format!(
            "{what}: 日本語が #2365 より前と違う\n  want: {old}\n  got:  {ja}"
        ));
    }
    if let Some(c) = has_japanese(&en) {
        failures.push(format!("{what}: 英語に日本語（{c}）が混ざる: {en}"));
    }
    if ja == en {
        failures.push(format!("{what}: 日英が同じ文（英語の書き忘れ）: {en}"));
    }
    if into_ja != ja {
        failures.push(format!("{what}: into_ja が text_in(Ja) と違う"));
    }
}

#[test]
fn 会話とosの設定の断りは全バリアントが日英を持ち日本語は2365より前の固定文とバイト一致() {
    let sessions = session_refusal_table();
    let samples: Vec<SessionRefusal> = sessions.iter().map(|(r, _)| r.clone()).collect();
    covers_every(
        &samples,
        session_refusal_index,
        SESSION_REFUSAL_VARIANTS,
        "SessionRefusal",
    );
    let os = os_refusal_table();
    let samples: Vec<OsRefusal> = os.iter().map(|(r, _)| r.clone()).collect();
    covers_every(&samples, os_refusal_index, OS_REFUSAL_VARIANTS, "OsRefusal");
    let si = si_error_table();
    let samples: Vec<SiError> = si.iter().map(|(e, _)| e.clone()).collect();
    covers_every(&samples, si_error_index, SI_ERROR_VARIANTS, "SiError");

    let mut failures = Vec::new();
    for (r, old) in &sessions {
        check_refusal_row(
            &mut failures,
            format!("{r:?}"),
            old,
            r.text_in(Lang::Ja),
            r.text_in(Lang::En),
            r.clone().into_ja(),
        );
    }
    for (r, old) in &os {
        check_refusal_row(
            &mut failures,
            format!("{r:?}"),
            old,
            r.text_in(Lang::Ja),
            r.text_in(Lang::En),
            r.clone().into_ja(),
        );
    }
    for (e, old) in &si {
        check_refusal_row(
            &mut failures,
            format!("{e:?}"),
            old,
            e.text_in(Lang::Ja),
            e.text_in(Lang::En),
            e.clone().into_ja(),
        );
    }
    report_all(failures);
}

/// 「見つからない」系は英語にも ` not found` を含む（リモートの HTTP 層が 404 へ寄せる判定を日英でそろえる）。
/// 候補が複数・形式の不正は「見つからない」ではない。包み（`Config` / `PromptDir` ほか）の中身も同じ言語
#[test]
fn 会話とosの設定の断りの見つからないの読みと包みの言語() {
    for r in [
        SessionRefusal::SessionNotFound("nope".into()),
        SessionRefusal::TranscriptMissing("aaaa1111".into()),
    ] {
        for lang in [Lang::Ja, Lang::En] {
            let text = r.text_in(lang);
            assert!(E::says_not_found(&text), "{lang:?}: {text}");
        }
    }
    for r in [
        SessionRefusal::SessionAmbiguous {
            prefix: "aaaa".into(),
            count: 2,
            candidates: vec!["aaaa1111".into(), "aaaa2222".into()],
        },
        SessionRefusal::InvalidSessionId,
    ] {
        for lang in [Lang::Ja, Lang::En] {
            let text = r.text_in(lang);
            assert!(!E::says_not_found(&text), "{lang:?}: {text}");
        }
    }
    // 包みの文は中身の型の表と同じ文（2 か所に書いて食い違わない）
    let config = ConfigError::ProfileNotFound {
        kind: Some(ProfileKind::Master),
        name: "p".into(),
        path: "/tmp/p.yaml".into(),
    };
    let wrapped = SessionRefusal::Config(config.clone());
    let write = WriteError::NoParent("/x".into());
    for lang in [Lang::Ja, Lang::En] {
        assert_eq!(wrapped.text_in(lang), config.text_in(lang));
        assert_eq!(
            SessionRefusal::McpSettingsWrite(write.clone()).text_in(lang),
            write.text_in(lang)
        );
        assert_eq!(
            SessionRefusal::PromptDir("e".into()).text_in(lang),
            Reason::SystemPromptDirFailed("e".into()).text_in(lang)
        );
        assert_eq!(
            OsRefusal::UnknownShellIntegrationAction("x".into()).text_in(lang),
            Reason::UnknownValue {
                key: "action",
                got: "x".into(),
                allowed: "status / install / uninstall".into(),
            }
            .text_in(lang)
        );
    }
    // dispatch へ載せる口は枠（invalid parameter:）と中身を同じ言語で組む
    assert_eq!(
        E::InvalidParams(OsRefusal::BatteryFloorOutOfRange(91).text_in(Lang::En)).text_in(Lang::En),
        "invalid parameter: the battery floor must be between 5 and 90 (got: 91)"
    );
}

/// 断りの組み立ての中の日本語の固定文（`Err(` / `.map_err(` / `.ok_or(` / `.ok_or_else(` の呼び出しの
/// 閉じ括弧まで）。成功の応答の注記（`Ok` の文・warnings・スクリプトの中身）とコメントは見ない
fn refusal_japanese(lines: &[(usize, &str)]) -> Vec<(usize, String)> {
    const MARKS: [&str; 4] = ["Err(", ".map_err(", ".ok_or(", ".ok_or_else("];
    let mut out = Vec::new();
    let mut depth = 0i32;
    for (line, code) in lines {
        if code.trim_start().starts_with("//") {
            continue;
        }
        // 断りの組み立ての中か（行をまたぐときは閉じ括弧まで）
        let from = if depth > 0 {
            Some(0)
        } else {
            MARKS.iter().filter_map(|m| code.find(m)).min()
        };
        let Some(from) = from else {
            continue;
        };
        let tail = &code[from..];
        depth =
            (depth + tail.matches('(').count() as i32 - tail.matches(')').count() as i32).max(0);
        if let Some(c) = line_literals(tail).iter().find_map(|lit| has_japanese(lit)) {
            out.push((*line, format!("（{c}）{}", code.trim())));
        }
    }
    out
}

/// 型の口の本体（断りの文は `SessionRefusal` / `OsRefusal` / `SiError` にだけ置く）
const SESSION_OS_REFUSAL_SCOPES: &[(&str, &str, Span)] = &[
    (
        SESSIONS_PATH,
        SESSIONS_SRC,
        Span::Item("    pub fn load_from_typed("),
    ),
    (
        SESSIONS_PATH,
        SESSIONS_SRC,
        Span::Item("    pub fn load_typed("),
    ),
    (
        SESSIONS_PATH,
        SESSIONS_SRC,
        Span::Item("    pub fn resolve_id_typed("),
    ),
    (
        SESSIONS_PATH,
        SESSIONS_SRC,
        Span::Item("pub fn list_payload_typed("),
    ),
    (
        SESSIONS_PATH,
        SESSIONS_SRC,
        Span::Item("pub fn show_payload_typed("),
    ),
    (
        SESSIONS_PATH,
        SESSIONS_SRC,
        Span::Item("pub fn resume_command_typed("),
    ),
    (
        SESSIONS_PATH,
        SESSIONS_SRC,
        Span::Item("fn resume_command_with_env("),
    ),
    (
        RESUME_LAUNCH_PATH,
        RESUME_LAUNCH_SRC,
        Span::Item("    pub fn materialize_typed("),
    ),
    (
        RESUME_LAUNCH_PATH,
        RESUME_LAUNCH_SRC,
        Span::Item("pub fn resume_launch_typed("),
    ),
    (
        RESUME_LAUNCH_PATH,
        RESUME_LAUNCH_SRC,
        Span::Item("pub fn resume_launch_in_typed("),
    ),
    (
        RESUME_LAUNCH_PATH,
        RESUME_LAUNCH_SRC,
        Span::Item("fn no_transcript("),
    ),
    (
        RESUME_LAUNCH_PATH,
        RESUME_LAUNCH_SRC,
        Span::Item("pub fn worker_resume_launch_typed("),
    ),
    (
        RESUME_LAUNCH_PATH,
        RESUME_LAUNCH_SRC,
        Span::Item("fn resume_profile_launch("),
    ),
    (
        RESUME_LAUNCH_PATH,
        RESUME_LAUNCH_SRC,
        Span::Item("fn resume_worker_launch("),
    ),
    (
        RESUME_LAUNCH_PATH,
        RESUME_LAUNCH_SRC,
        Span::Item("fn load_profile_for_resume("),
    ),
    (
        WORKER_MCP_PATH,
        WORKER_MCP_SRC,
        Span::Item("    pub fn materialize_typed("),
    ),
    (
        WORKER_MCP_PATH,
        WORKER_MCP_SRC,
        Span::Item("fn write_settings("),
    ),
    (
        ORCH_MOD_PATH,
        ORCH_MOD_SRC,
        Span::Item("pub fn build_master_cmd_with_plan_in_typed("),
    ),
    (FDA_PATH, FDA_SRC, Span::Item("pub fn open_settings_typed(")),
    (
        SLEEP_GUARD_PATH,
        SLEEP_GUARD_SRC,
        Span::Item("pub fn parse_battery_floor_typed("),
    ),
    (
        SLEEP_GUARD_PATH,
        SLEEP_GUARD_SRC,
        Span::Item("pub fn prepare_lid_control_typed("),
    ),
    (
        SLEEP_GUARD_PATH,
        SLEEP_GUARD_SRC,
        Span::Item("pub fn install_sudoers_typed("),
    ),
    (
        SLEEP_GUARD_PATH,
        SLEEP_GUARD_SRC,
        Span::Item("pub fn teardown_lid_control_typed("),
    ),
    (
        SLEEP_GUARD_PATH,
        SLEEP_GUARD_SRC,
        Span::Item("pub fn remove_sudoers_typed("),
    ),
    (
        SLEEP_GUARD_PATH,
        SLEEP_GUARD_SRC,
        Span::Item("pub fn open_battery_settings_typed("),
    ),
    (
        SHELL_INTEGRATION_PATH,
        SHELL_INTEGRATION_SRC,
        Span::Item("pub fn run_typed("),
    ),
    (
        CORE_SHELL_INTEGRATION_PATH,
        CORE_SHELL_INTEGRATION_SRC,
        Span::Item("pub fn install_typed("),
    ),
    (
        CORE_SHELL_INTEGRATION_PATH,
        CORE_SHELL_INTEGRATION_SRC,
        Span::Item("pub fn uninstall_typed("),
    ),
    (
        CORE_SHELL_INTEGRATION_PATH,
        CORE_SHELL_INTEGRATION_SRC,
        Span::Item("pub fn strip_legacy_block_typed("),
    ),
    (
        SESSION_REFUSAL_PATH,
        SESSION_REFUSAL_SRC,
        Span::Item("    pub fn operation("),
    ),
    (
        OS_REFUSAL_PATH,
        OS_REFUSAL_SRC,
        Span::Item("    pub fn invalid("),
    ),
    (
        OS_REFUSAL_PATH,
        OS_REFUSAL_SRC,
        Span::Item("    pub fn operation("),
    ),
];

#[test]
fn 会話とosの設定の型の口の断りに日本語の固定文が無い() {
    let mut failures = Vec::new();
    for (path, src, span) in SESSION_OS_REFUSAL_SCOPES {
        let Some(lines) = client_store_lines(src, span) else {
            failures.push(format!(
                "{path}: 走査の範囲 {} が見つからない（改名で番犬が空振りする。\
                 SESSION_OS_REFUSAL_SCOPES を直す）",
                span_name(span)
            ));
            continue;
        };
        for (line, text) in refusal_japanese(&lines) {
            failures.push(format!(
                "{path}:{line}: 型の口の断りに日本語の固定文がある = 英語表示にもそのまま出る \
                 （SessionRefusal / OsRefusal / SiError へバリアントを足し、日英の対を text_in に書く）: {text}"
            ));
        }
    }
    report_all(failures);
}

/// dispatch が呼んではいけない日本語の口（型の口 `*_typed` の外側）。蓋閉じ継続の解除・Battery の
/// 設定を開く口とファイル操作（`os_integration`）は、中身の `platform::lid` / `os_integration` の文と
/// 一緒に移した（#2365 の続き）
const JA_SESSION_OS_CALLS: [&str; 17] = [
    "sessions::list_payload(",
    "sessions::show_payload(",
    "SessionCatalog::load()",
    ".resolve_id(",
    "resume_launch::resume_launch(",
    "resume_launch::resume_launch_in(",
    ".materialize()",
    "fda::open_settings()",
    "sleep_guard::parse_battery_floor(",
    "sleep_guard::prepare_lid_control()",
    "shell_integration::run(",
    "sleep_guard::teardown_lid_control()",
    "sleep_guard::open_battery_settings()",
    "os_integration::reveal(",
    "os_integration::open_default(",
    "os_integration::open_with(",
    "os_integration::move_to_trash(",
];

/// dispatch の本体で、日本語の口を呼んで断りに載せている行（断りを捨てる文は見ない）
fn ja_session_os_calls(lines: &[(usize, &str)]) -> Vec<(usize, &'static str, String)> {
    let mut out = Vec::new();
    for (i, (line, code)) in lines.iter().enumerate() {
        if code.trim_start().starts_with("//") {
            continue;
        }
        if let Some(call) = JA_SESSION_OS_CALLS.iter().find(|c| code.contains(*c)) {
            if !statement_discards_error(lines, i) {
                out.push((*line, *call, code.trim().to_string()));
            }
        }
    }
    out
}

#[test]
fn dispatch_は会話とosの設定の断りを日本語の口から載せない() {
    let lines = dispatch_code_lines();
    let mut failures = Vec::new();
    for (line, call, code) in ja_session_os_calls(&lines) {
        failures.push(format!(
            "{DISPATCH_PATH}:{line}: dispatch が日本語の口 `{call}…)` の断りを載せている = 英語表示でも \
             中身だけ日本語で混ざる（`*_typed` を呼んで SessionRefusal / OsRefusal の operation / invalid で載せる）: {code}"
        ));
    }
    report_all(failures);
}

/// 番犬自身の検査: 断りの組み立ての中の日本語（行をまたぐ `Err(format!(` も）だけを名指し、
/// `Ok` の文・warnings・コメント・断りを捨てる文は見逃す
#[test]
fn 会話とosの設定の番犬の境界() {
    let src = r#"pub fn a_typed() -> Result<String, OsRefusal> {
    // Err("コメントの日本語は見ない")
    warnings.push(format!("既定プロファイルで再開した"));
    if x {
        return Ok("この OS では追加の権限も登録も不要です".to_string());
    }
    let p = path().ok_or("ホームディレクトリが取得できない")?;
    let q = run().map_err(|e| OsRefusal::OsascriptFailed(e.to_string()))?;
    Err(format!(
        "sudoers 登録に失敗: {}",
        detail
    ))
}
"#;
    let lines = front_scope_lines(src, Some("pub fn a_typed(")).expect("頭を引ける");
    let found: Vec<usize> = refusal_japanese(&lines)
        .into_iter()
        .map(|(l, _)| l)
        .collect();
    assert_eq!(found, vec![7, 10], "断りの日本語の 2 か所だけを名指す");

    let dispatch = r#"fn d() {
    // crate::sessions::show_payload(&id) はコメント
    let a = crate::sessions::show_payload(&id).map_err(DispatchError::Operation)?;
    let catalog = crate::sessions::SessionCatalog::load().ok();
    let b = crate::sessions::show_payload_typed(&id).map_err(SessionRefusal::operation)?;
    crate::shell_integration::run(action.as_deref()).map_err(DispatchError::InvalidParams)
}
"#;
    let lines = front_scope_lines(dispatch, Some("fn d(")).expect("頭を引ける");
    let found: Vec<usize> = ja_session_os_calls(&lines)
        .into_iter()
        .map(|(l, _, _)| l)
        .collect();
    assert_eq!(found, vec![3, 6], "日本語の口の 2 か所だけを名指す");
}

// ---- #2365 の続き: 更新の口（tako-app の `update_checker`）と、tako-core の文を GUI がそのまま返す 5 系統
// （`open_plan` の行・桁・行指定 = `PlanRefusal`・`PreviewViewState::apply` = `ViewRefusal`・
// `PreviewOutline::target` = `OutlineRefusal`・`preview_limit::Truncation::edit_refusal`・`mouse_forward` の
// クリック = `ClickRefusal`）と、変更履歴の取得の打ち切り（`git::HistoryTimedOut`） ----
//
// 以前は GUI（と dispatch）がこれらの日本語の固定文をそのまま返していた。日英の対は各型の `text_in` の
// 網羅 match に置き、`Display` / `From<_> for String` は組んだその場の表示言語。更新の口の型
// （`UpdateRefusal` / `UpdateNote` / `CheckError` / `InstallerError`）は tako-app（バイナリ）にあるので、
// 表の検査は tako-app の単体テスト（`update_checker::tests::更新の口の断りは全バリアントが日英を持ち…`）が
// 持ち、ここでは (1) tako-core の型の全バリアントの日英と旧文のバイト一致、(2) 断りの組み立て（`Err(` /
// `.map_err(` / `.ok_or(`）に日本語の固定文を書き足していないことを `file:line` で名指す

use tako_core::git::HistoryTimedOut;
use tako_core::mouse_forward::{ClickRefusal, Encoding, TakoReason};
use tako_core::open_plan::{PlanRefusal, PreviewRoute};
use tako_core::{OutlineRefusal, ViewRefusal};

const OPEN_PLAN_SRC: &str = include_str!("../../tako-core/src/open_plan.rs");
const OPEN_PLAN_PATH: &str = "crates/tako-core/src/open_plan.rs";
const PREVIEW_VIEW_SRC: &str = include_str!("../../tako-core/src/preview_view.rs");
const PREVIEW_VIEW_PATH: &str = "crates/tako-core/src/preview_view.rs";
const PREVIEW_OUTLINE_SRC: &str = include_str!("../../tako-core/src/preview_outline.rs");
const PREVIEW_OUTLINE_PATH: &str = "crates/tako-core/src/preview_outline.rs";
const PREVIEW_LIMIT_SRC: &str = include_str!("../../tako-core/src/preview_limit.rs");
const PREVIEW_LIMIT_PATH: &str = "crates/tako-core/src/preview_limit.rs";
const MOUSE_FORWARD_SRC: &str = include_str!("../../tako-core/src/mouse_forward.rs");
const MOUSE_FORWARD_PATH: &str = "crates/tako-core/src/mouse_forward.rs";
const GIT_SRC: &str = include_str!("../../tako-core/src/git.rs");
const GIT_PATH: &str = "crates/tako-core/src/git.rs";
const UPDATE_CHECKER_SRC: &str = include_str!("../../tako-app/src/update_checker.rs");
const UPDATE_CHECKER_PATH: &str = "crates/tako-app/src/update_checker.rs";
const INSTALLER_UPDATE_SRC: &str = include_str!("../../tako-app/src/platform/installer_update.rs");
const INSTALLER_UPDATE_PATH: &str = "crates/tako-app/src/platform/installer_update.rs";

/// 通し番号（網羅 match = バリアントを足すと見本の表を足すまでここが E0004 で落ちる）
fn plan_refusal_index(r: &PlanRefusal) -> usize {
    match r {
        PlanRefusal::LineNotOneBased => 0,
        PlanRefusal::ColumnNotOneBased => 1,
        PlanRefusal::NoLinesIn(_) => 2,
    }
}
const PLAN_REFUSAL_VARIANTS: usize = 3;

fn view_refusal_index(r: &ViewRefusal) -> usize {
    match r {
        ViewRefusal::ZoomOutOfRange => 0,
        ViewRefusal::PageZero => 1,
        ViewRefusal::PanNotFinite => 2,
    }
}
const VIEW_REFUSAL_VARIANTS: usize = 3;

fn outline_refusal_index(r: &OutlineRefusal) -> usize {
    match r {
        OutlineRefusal::ItemZero => 0,
        OutlineRefusal::OutOfRange { .. } => 1,
    }
}
const OUTLINE_REFUSAL_VARIANTS: usize = 2;

fn click_refusal_index(r: &ClickRefusal) -> usize {
    match r {
        ClickRefusal::NotForwarded(_) => 0,
        ClickRefusal::Unrepresentable { .. } => 1,
        ClickRefusal::ContainerUnresolved => 2,
    }
}
const CLICK_REFUSAL_VARIANTS: usize = 3;

fn truncation_index(t: &Truncation) -> usize {
    match t {
        Truncation::Bytes { .. } => 0,
        Truncation::Lines { .. } => 1,
    }
}
const TRUNCATION_VARIANTS: usize = 2;

/// 見本と #2365 より前の日本語（旧実装の固定文を同じ値で組んだ文）
fn plan_refusal_table() -> Vec<(PlanRefusal, &'static str)> {
    vec![
        (
            PlanRefusal::LineNotOneBased,
            "行番号は 1 始まり（0 以下は指定できない）",
        ),
        (
            PlanRefusal::ColumnNotOneBased,
            "桁番号は 1 始まり（0 以下は指定できない）",
        ),
        (
            PlanRefusal::NoLinesIn(PreviewRoute::Image),
            "行を指定して開けるのはテキスト（code / markdown）だけ: image",
        ),
        (
            PlanRefusal::NoLinesIn(PreviewRoute::Pdf),
            "行を指定して開けるのはテキスト（code / markdown）だけ: pdf",
        ),
    ]
}

fn view_refusal_table() -> Vec<(ViewRefusal, &'static str)> {
    vec![
        (
            ViewRefusal::ZoomOutOfRange,
            "ズーム倍率は 25%〜400% の範囲で指定する",
        ),
        (ViewRefusal::PageZero, "ページは 1 以上で指定する"),
        (ViewRefusal::PanNotFinite, "パン量は有限の数で指定する"),
    ]
}

fn outline_refusal_table() -> Vec<(OutlineRefusal, &'static str)> {
    vec![
        (
            OutlineRefusal::ItemZero,
            "アウトライン項目は 1 以上で指定する",
        ),
        (
            OutlineRefusal::OutOfRange { item: 9, total: 3 },
            "アウトライン項目の範囲外: 9（全 3 件）",
        ),
    ]
}

fn click_refusal_table() -> Vec<(ClickRefusal, &'static str)> {
    vec![
        (
            ClickRefusal::NotForwarded(TakoReason::NotRequested),
            "このペインのアプリはマウス報告を要求していない（not_requested）。シェル・less 等へのクリックは意味を持たないので渡さない",
        ),
        (
            ClickRefusal::NotForwarded(TakoReason::CopyMode),
            "器のペインが copy mode 中（copy_mode）。渡すと copy mode のキーとして解釈される",
        ),
        (
            ClickRefusal::NotForwarded(TakoReason::Unresolved),
            "クリックを渡せない（unresolved）",
        ),
        (
            ClickRefusal::NotForwarded(TakoReason::Disabled),
            "クリックを渡せない（disabled）",
        ),
        (
            ClickRefusal::Unrepresentable {
                row: 0,
                col: 300,
                encoding: Encoding::X10,
            },
            "座標 (row 0, col 300) は内側アプリの報告形式（X10）で表せない",
        ),
        (
            ClickRefusal::ContainerUnresolved,
            "器へ問い合わせられない（unresolved）",
        ),
    ]
}

fn truncation_refusal_table() -> Vec<(Truncation, &'static str)> {
    vec![
        (
            Truncation::Bytes { size: 12_345_678 },
            "大きさが上限を超えるため末尾を省略して表示しており、編集できない: 12.3 MB（上限 10 MB）",
        ),
        (
            Truncation::Lines { lines: 100_001 },
            "行数が上限を超えるため末尾を省略して表示しており、編集できない: 100,001 行（上限 100,000 行）",
        ),
    ]
}

const TIMED_OUT_OLD: &str = "変更履歴の取得を 3 秒で打ち切った（`git diff` が返らない。巨大なリポジトリ・ネットワーク上のフォルダ・ロック中の git を疑う。上限は env TAKO_CHANGELOG_GIT_TIMEOUT_SECS）";

fn timed_out_sample() -> HistoryTimedOut {
    HistoryTimedOut {
        step: "diff",
        budget: std::time::Duration::from_secs(3),
    }
}

/// 1 行の検査（日本語は旧文とバイト一致・英語に日本語が無い・英語の書き忘れが無い・
/// 「見つからない」の読みが日英でそろう）
fn check_core_row(
    failures: &mut Vec<String>,
    at_ja: String,
    at_en: String,
    what: String,
    old: &str,
    ja: &str,
    en: &str,
) {
    if ja != old {
        failures.push(format!(
            "{at_ja}: {what} の日本語が #2365 より前と違う\n  want: {old}\n  got:  {ja}"
        ));
    }
    if let Some(c) = has_japanese(en) {
        failures.push(format!(
            "{at_en}: {what} の英語に日本語（{c}）が混ざる: {en}"
        ));
    }
    if en.trim().is_empty() || ja == en {
        failures.push(format!(
            "{at_en}: {what} の英語が空か日本語と同じ文（英語の書き忘れ）: {en}"
        ));
    }
    if E::says_not_found(ja) != E::says_not_found(en) {
        failures.push(format!(
            "{at_en}: {what} の「見つからない」の読みが日英で違う\n  ja: {ja}\n  en: {en}"
        ));
    }
}

#[test]
fn 更新の口とcoreの文の断りは全バリアントが日英を持ち日本語は2365より前の固定文とバイト一致() {
    let plan = plan_refusal_table();
    covers_every(
        &plan.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
        plan_refusal_index,
        PLAN_REFUSAL_VARIANTS,
        "PlanRefusal",
    );
    let view = view_refusal_table();
    covers_every(
        &view.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
        view_refusal_index,
        VIEW_REFUSAL_VARIANTS,
        "ViewRefusal",
    );
    let outline = outline_refusal_table();
    covers_every(
        &outline.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
        outline_refusal_index,
        OUTLINE_REFUSAL_VARIANTS,
        "OutlineRefusal",
    );
    let click = click_refusal_table();
    covers_every(
        &click.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
        click_refusal_index,
        CLICK_REFUSAL_VARIANTS,
        "ClickRefusal",
    );
    let truncation = truncation_refusal_table();
    covers_every(
        &truncation.iter().map(|(t, _)| *t).collect::<Vec<_>>(),
        truncation_index,
        TRUNCATION_VARIANTS,
        "Truncation",
    );

    let mut failures = Vec::new();
    let at = |src: &str, path: &str, from: &str, debug: String, lang: Lang| {
        where_written(src, path, from, &debug, lang)
    };
    for (r, old) in plan {
        let d = format!("{r:?}");
        check_core_row(
            &mut failures,
            at(
                OPEN_PLAN_SRC,
                OPEN_PLAN_PATH,
                "impl PlanRefusal {",
                d.clone(),
                Lang::Ja,
            ),
            at(
                OPEN_PLAN_SRC,
                OPEN_PLAN_PATH,
                "impl PlanRefusal {",
                d.clone(),
                Lang::En,
            ),
            d,
            old,
            &r.text_in(Lang::Ja),
            &r.text_in(Lang::En),
        );
    }
    for (r, old) in view {
        let d = format!("{r:?}");
        check_core_row(
            &mut failures,
            at(
                PREVIEW_VIEW_SRC,
                PREVIEW_VIEW_PATH,
                "impl ViewRefusal {",
                d.clone(),
                Lang::Ja,
            ),
            at(
                PREVIEW_VIEW_SRC,
                PREVIEW_VIEW_PATH,
                "impl ViewRefusal {",
                d.clone(),
                Lang::En,
            ),
            d,
            old,
            &r.text_in(Lang::Ja),
            &r.text_in(Lang::En),
        );
    }
    for (r, old) in outline {
        let d = format!("{r:?}");
        check_core_row(
            &mut failures,
            at(
                PREVIEW_OUTLINE_SRC,
                PREVIEW_OUTLINE_PATH,
                "impl OutlineRefusal {",
                d.clone(),
                Lang::Ja,
            ),
            at(
                PREVIEW_OUTLINE_SRC,
                PREVIEW_OUTLINE_PATH,
                "impl OutlineRefusal {",
                d.clone(),
                Lang::En,
            ),
            d,
            old,
            &r.text_in(Lang::Ja),
            &r.text_in(Lang::En),
        );
    }
    for (r, old) in click {
        let d = format!("{r:?}");
        check_core_row(
            &mut failures,
            at(
                MOUSE_FORWARD_SRC,
                MOUSE_FORWARD_PATH,
                "impl ClickRefusal {",
                d.clone(),
                Lang::Ja,
            ),
            at(
                MOUSE_FORWARD_SRC,
                MOUSE_FORWARD_PATH,
                "impl ClickRefusal {",
                d.clone(),
                Lang::En,
            ),
            d,
            old,
            &r.text_in(Lang::Ja),
            &r.text_in(Lang::En),
        );
        // 理由の語（`not_requested` ほか）は日英どちらにも入る（呼び手・検証が語で読む）
        if let ClickRefusal::NotForwarded(reason) = r {
            for lang in [Lang::Ja, Lang::En] {
                let text = r.text_in(lang);
                if !text.contains(reason.as_str()) {
                    failures.push(format!(
                        "{r:?} {lang:?}: 理由の語 {} が無い: {text}",
                        reason.as_str()
                    ));
                }
            }
        }
    }
    for (t, old) in truncation {
        let d = format!("{t:?}");
        check_core_row(
            &mut failures,
            at(
                PREVIEW_LIMIT_SRC,
                PREVIEW_LIMIT_PATH,
                "pub fn edit_refusal_in(",
                d.clone(),
                Lang::Ja,
            ),
            at(
                PREVIEW_LIMIT_SRC,
                PREVIEW_LIMIT_PATH,
                "pub fn edit_refusal_in(",
                d.clone(),
                Lang::En,
            ),
            d,
            old,
            &t.edit_refusal_in(Lang::Ja),
            &t.edit_refusal_in(Lang::En),
        );
        // `tako edit` の応答の `limit.message` も同じ言語の同じ文
        if t.to_json_in(Lang::En)["message"] != t.edit_refusal_in(Lang::En).as_str() {
            failures.push(format!(
                "{t:?}: limit.message が edit_refusal_in(En) と違う"
            ));
        }
    }
    let timed = timed_out_sample();
    let timed_at = format!(
        "{GIT_PATH}:{}",
        GIT_SRC
            .lines()
            .position(|l| l.contains("impl HistoryTimedOut {"))
            .map_or(0, |i| i + 1)
    );
    check_core_row(
        &mut failures,
        timed_at.clone(),
        timed_at,
        "HistoryTimedOut".into(),
        TIMED_OUT_OLD,
        &timed.text_in(Lang::Ja),
        &timed.text_in(Lang::En),
    );
    // 段の名前と上限の env は日英どちらにも入る（スクリプト・単体テストが読む）
    for lang in [Lang::Ja, Lang::En] {
        let text = timed.text_in(lang);
        for needle in ["`git diff`", tako_core::git::CHANGELOG_TIMEOUT_ENV] {
            if !text.contains(needle) {
                failures.push(format!("HistoryTimedOut {lang:?}: {needle} が無い: {text}"));
            }
        }
    }
    report_all(failures);
}

/// 断りの組み立てに日本語の固定文を書き足してはいけない本体（日英の対は各型の `text_in` にだけ置く）。
/// 更新の口は GUI の口（`impl SystemHost for TakoApp`）が委ねる先 = HOST_IMPL_SCOPES の外なのでここで見る
const UPDATE_CORE_SCOPES: &[(&str, &str, Span)] = &[
    (
        OPEN_PLAN_PATH,
        OPEN_PLAN_SRC,
        Span::Item("pub fn preview_route_with_line("),
    ),
    (
        OPEN_PLAN_PATH,
        OPEN_PLAN_SRC,
        Span::Item("pub fn clamp_line("),
    ),
    (
        OPEN_PLAN_PATH,
        OPEN_PLAN_SRC,
        Span::Item("pub fn clamp_column("),
    ),
    (
        PREVIEW_VIEW_PATH,
        PREVIEW_VIEW_SRC,
        Span::Item("    pub fn apply("),
    ),
    (
        PREVIEW_OUTLINE_PATH,
        PREVIEW_OUTLINE_SRC,
        Span::Item("    pub fn target("),
    ),
    (
        MOUSE_FORWARD_PATH,
        MOUSE_FORWARD_SRC,
        Span::Item("pub fn click_tmux("),
    ),
    (
        MOUSE_FORWARD_PATH,
        MOUSE_FORWARD_SRC,
        Span::Item("pub fn click_pair("),
    ),
    (GIT_PATH, GIT_SRC, Span::Item("pub fn file_history(")),
    (GIT_PATH, GIT_SRC, Span::Item("pub fn commit_file_diff(")),
    (
        UPDATE_CHECKER_PATH,
        UPDATE_CHECKER_SRC,
        Span::Item("    pub fn parse_typed("),
    ),
    (
        UPDATE_CHECKER_PATH,
        UPDATE_CHECKER_SRC,
        Span::Item("pub fn repair_brew_typed("),
    ),
    (
        UPDATE_CHECKER_PATH,
        UPDATE_CHECKER_SRC,
        Span::Item("pub fn check_all_channels("),
    ),
    (
        UPDATE_CHECKER_PATH,
        UPDATE_CHECKER_SRC,
        Span::Item("pub fn update_target("),
    ),
    (
        UPDATE_CHECKER_PATH,
        UPDATE_CHECKER_SRC,
        Span::Item("fn run_route("),
    ),
    (
        UPDATE_CHECKER_PATH,
        UPDATE_CHECKER_SRC,
        Span::Item("fn refusal("),
    ),
    (
        UPDATE_CHECKER_PATH,
        UPDATE_CHECKER_SRC,
        Span::Item("fn update_via_installer("),
    ),
    (
        UPDATE_CHECKER_PATH,
        UPDATE_CHECKER_SRC,
        Span::Item("fn update_via_homebrew("),
    ),
    (
        UPDATE_CHECKER_PATH,
        UPDATE_CHECKER_SRC,
        Span::Item("fn update_via_zip("),
    ),
    (
        UPDATE_CHECKER_PATH,
        UPDATE_CHECKER_SRC,
        Span::Item("pub fn restart_app("),
    ),
    (
        INSTALLER_UPDATE_PATH,
        INSTALLER_UPDATE_SRC,
        Span::Item("pub fn download("),
    ),
    (
        INSTALLER_UPDATE_PATH,
        INSTALLER_UPDATE_SRC,
        Span::Item("pub fn launch("),
    ),
];

#[test]
fn 更新の口とcoreの文の断りの組み立てに日本語の固定文が無い() {
    let mut failures = Vec::new();
    for (path, src, span) in UPDATE_CORE_SCOPES {
        let Some(lines) = client_store_lines(src, span) else {
            failures.push(format!(
                "{path}: 走査の範囲 {} が見つからない（改名で番犬が空振りする。UPDATE_CORE_SCOPES を直す）",
                span_name(span)
            ));
            continue;
        };
        for (line, text) in refusal_japanese(&lines) {
            failures.push(format!(
                "{path}:{line}: 断りの組み立てに日本語の固定文がある = 英語表示にもそのまま出る \
                 （UpdateRefusal / PlanRefusal / ViewRefusal / OutlineRefusal / ClickRefusal へバリアントを足し、\
                 日英の対を text_in に書く）: {text}"
            ));
        }
    }
    report_all(failures);
}

// ---- #2365 の続き: OS のシェルに委ねる操作（`platform::os_integration` / `platform::finder_reveal`）と
// 蓋閉じ継続の上書き・復元（`platform::lid`）の断り ----
//
// dispatch の Finder / エクスプローラーで表示・既定アプリ / 指定のアプリで開く・ゴミ箱へ移す・
// 蓋閉じ継続の解除・System Settings の Battery を開くは、以前は中身の `Err(String)`（日本語の固定文）を
// 素通ししていた。枠（`OsRefusal`）だけ移すと 1 つの断りに日英が混ざるので、中身の型
// （`OsIntegrationError` / `LidError` と、その中で引く tako-core の `UrlBlocked` / `JsonReason`）と
// 一緒に移した。Windows の `ShellExecuteW` / `SHFileOperationW` の説明と電源プランの文も `cfg` の外の
// 表に置いたので、macOS のこのテストで日英を検査できる。

use tako_control::platform::os_integration::{shell_execute_reason, shfileop_reason};
use tako_core::load_issue::{JsonProblem, JsonReason};
use tako_core::url_guard::UrlBlocked;

const OS_INTEGRATION_SRC: &str = include_str!("../src/platform/os_integration.rs");
const OS_INTEGRATION_PATH: &str = "crates/tako-control/src/platform/os_integration.rs";
const FINDER_REVEAL_SRC: &str = include_str!("../src/platform/finder_reveal.rs");
const FINDER_REVEAL_PATH: &str = "crates/tako-control/src/platform/finder_reveal.rs";
const LID_SRC: &str = include_str!("../src/platform/lid.rs");
const LID_PATH: &str = "crates/tako-control/src/platform/lid.rs";
const URL_GUARD_SRC: &str = include_str!("../../tako-core/src/url_guard.rs");
const URL_GUARD_PATH: &str = "crates/tako-core/src/url_guard.rs";

/// `OsIntegrationError` の通し番号（網羅 match = バリアントを足すとここがコンパイルで落ちる）
fn os_integration_index(e: &OsIntegrationError) -> usize {
    match e {
        OsIntegrationError::UrlBlocked(_) => 0,
        OsIntegrationError::Spawn { .. } => 1,
        OsIntegrationError::Unsupported(_) => 2,
        OsIntegrationError::OpenCommandFailed(_) => 3,
        OsIntegrationError::UrlHandlerFailed(_) => 4,
        OsIntegrationError::UrlExitStatus { .. } => 5,
        OsIntegrationError::OsascriptSpawn(_) => 6,
        OsIntegrationError::AppPickCanceled => 7,
        OsIntegrationError::AppPathEmpty => 8,
        OsIntegrationError::TrashFailed(_) => 9,
        OsIntegrationError::TrashNoName(_) => 10,
        OsIntegrationError::AppKitUnavailable => 11,
        OsIntegrationError::WorkspaceUnavailable => 12,
        OsIntegrationError::NulInString => 13,
        OsIntegrationError::NsStringFailed => 14,
        OsIntegrationError::NulInPath(_) => 15,
        OsIntegrationError::FileUrlFailed(_) => 16,
        OsIntegrationError::RecordPathUnreadable => 17,
        OsIntegrationError::RecordWrite { .. } => 18,
        OsIntegrationError::PathHasNul => 19,
        OsIntegrationError::PathHasQuote => 20,
        OsIntegrationError::Absolutize(_) => 21,
        OsIntegrationError::PathMissing(_) => 22,
        OsIntegrationError::ShellExecute { .. } => 23,
        OsIntegrationError::TrashShellOp(_) => 24,
        OsIntegrationError::TrashAborted => 25,
    }
}

const OS_INTEGRATION_VARIANTS: usize = 26;

/// `LidError` の通し番号
fn lid_error_index(e: &LidError) -> usize {
    match e {
        LidError::RecordRead { .. } => 0,
        LidError::RecordMalformed { .. } => 1,
        LidError::DataDirUnresolved => 2,
        LidError::NothingToRestore => 3,
        LidError::BadSchemeGuid(_) => 4,
        LidError::Lock(_) => 5,
        LidError::Write(_) => 6,
        LidError::Serialize(_) => 7,
        LidError::ActiveScheme(_) => 8,
        LidError::ReadSetting { .. } => 9,
        LidError::WriteSetting { .. } => 10,
        LidError::ApplyScheme(_) => 11,
        LidError::Unsupported => 12,
    }
}

const LID_ERROR_VARIANTS: usize = 13;

/// `Launch` の通し番号
fn launch_index(l: &Launch) -> usize {
    match l {
        Launch::Finder => 0,
        Launch::Explorer => 1,
        Launch::DefaultApp => 2,
        Launch::App(_) => 3,
        Launch::TextEditor => 4,
        Launch::Url => 5,
        Launch::NewInstance => 6,
        Launch::AppPicker => 7,
    }
}

/// `OsFeature` の通し番号
fn os_feature_index(f: &OsFeature) -> usize {
    match f {
        OsFeature::Reveal => 0,
        OsFeature::OpenDefault => 1,
        OsFeature::OpenWith => 2,
        OsFeature::AppPicker => 3,
        OsFeature::TextEditor => 4,
        OsFeature::NewInstance => 5,
        OsFeature::Trash => 6,
    }
}

/// `UrlBlocked` の通し番号
fn url_blocked_index(b: &UrlBlocked) -> usize {
    match b {
        UrlBlocked::NoScheme => 0,
        UrlBlocked::DriveLetter => 1,
        UrlBlocked::Scheme => 2,
        UrlBlocked::ControlChar => 3,
        UrlBlocked::NoHost => 4,
    }
}

/// `JsonProblem` の通し番号
fn json_problem_index(p: &JsonProblem) -> usize {
    match p {
        JsonProblem::Syntax => 0,
        JsonProblem::Eof => 1,
        JsonProblem::Data => 2,
        JsonProblem::Io => 3,
    }
}

/// 見本と、#2365 より前の固定文（`os_integration` / `finder_reveal` の macOS・Windows・その他の実装の文）
fn os_integration_table() -> Vec<(OsIntegrationError, &'static str)> {
    use OsIntegrationError as O;
    let e = || "No such file or directory (os error 2)".to_string();
    vec![
        (
            O::UrlBlocked(UrlBlocked::Scheme),
            "この URL は開けない（許可していないスキーム）",
        ),
        (
            O::Spawn {
                what: Launch::DefaultApp,
                error: e(),
            },
            "デフォルトアプリで開けない: No such file or directory (os error 2)",
        ),
        (
            O::Unsupported(OsFeature::Trash),
            "ゴミ箱への移動はこのプラットフォームでは未対応です",
        ),
        (
            O::OpenCommandFailed(e()),
            "open コマンドの実行に失敗: No such file or directory (os error 2)",
        ),
        (
            O::UrlHandlerFailed(e()),
            "URL ハンドラの実行に失敗: No such file or directory (os error 2)",
        ),
        (
            O::UrlExitStatus {
                status: "exit status: 1".into(),
                url: "https://example.com/".into(),
            },
            "URL を開けない（終了コード exit status: 1）: https://example.com/",
        ),
        (
            O::OsascriptSpawn(e()),
            "osascript 起動に失敗: No such file or directory (os error 2)",
        ),
        (O::AppPickCanceled, "アプリ選択がキャンセルされた"),
        (O::AppPathEmpty, "アプリパスが空"),
        (
            O::TrashFailed(e()),
            "ゴミ箱への移動に失敗: No such file or directory (os error 2)",
        ),
        (
            O::TrashNoName("/".into()),
            "ゴミ箱への移動に失敗: 名前が無い: /",
        ),
        (
            O::AppKitUnavailable,
            "Finder を開けない: AppKit を読み込めない",
        ),
        (
            O::WorkspaceUnavailable,
            "Finder を開けない: NSWorkspace を使えない",
        ),
        (O::NulInString, "NUL を含む文字列"),
        (O::NsStringFailed, "NSString を作れない"),
        (O::NulInPath("/tmp/a".into()), "NUL を含むパス: /tmp/a"),
        (
            O::FileUrlFailed("/tmp/a".into()),
            "ファイルの URL を作れない: /tmp/a",
        ),
        (
            O::RecordPathUnreadable,
            "ファイルの URL からパスを読み戻せない",
        ),
        (
            O::RecordWrite {
                path: "/tmp/x/reveal.log".into(),
                error: e(),
            },
            "記録を書けない（/tmp/x/reveal.log）: No such file or directory (os error 2)",
        ),
        (O::PathHasNul, "パスに NUL 文字が含まれている"),
        (O::PathHasQuote, "パスに引用符が含まれている"),
        (
            O::Absolutize("The system cannot find the path specified. (os error 3)".into()),
            "パスを絶対化できない: The system cannot find the path specified. (os error 3)",
        ),
        (
            O::PathMissing(r"C:\tmp\a.txt".into()),
            r"パスが存在しない: C:\tmp\a.txt",
        ),
        (
            O::ShellExecute {
                what: Launch::Url,
                code: 31,
            },
            "URL を開けない: この種類のファイルに関連付けられたアプリが無い",
        ),
        (
            O::TrashShellOp(0x78),
            "ごみ箱への移動に失敗: アクセスが拒否された",
        ),
        (O::TrashAborted, "ごみ箱への移動が取り消された"),
    ]
}

/// 枠（何をしようとしたか）の全種と、#2365 より前の `spawn_open` / `shell_execute` の文
fn launch_table() -> Vec<(Launch, &'static str)> {
    vec![
        (Launch::Finder, "Finder を開けない: e"),
        (Launch::Explorer, "エクスプローラーを開けない: e"),
        (Launch::DefaultApp, "デフォルトアプリで開けない: e"),
        (
            Launch::App("Preview".into()),
            "アプリ 'Preview' で開けない: e",
        ),
        (Launch::TextEditor, "テキストエディタで開けない: e"),
        (Launch::Url, "URL を開けない: e"),
        (Launch::NewInstance, "アプリを起動できない: e"),
        (Launch::AppPicker, "アプリ選択ダイアログを開けない: e"),
    ]
}

/// 未対応の操作の全種と、#2365 より前の `unsupported(…)` の文（Windows の新規プロセス起動も同じ文）
fn os_feature_table() -> Vec<(OsFeature, &'static str)> {
    vec![
        (
            OsFeature::Reveal,
            "ファイルマネージャでの表示はこのプラットフォームでは未対応です",
        ),
        (
            OsFeature::OpenDefault,
            "既定アプリで開く操作はこのプラットフォームでは未対応です",
        ),
        (
            OsFeature::OpenWith,
            "アプリを指定して開く操作はこのプラットフォームでは未対応です",
        ),
        (
            OsFeature::AppPicker,
            "アプリ選択ダイアログはこのプラットフォームでは未対応です",
        ),
        (
            OsFeature::TextEditor,
            "テキストエディタで開く操作はこのプラットフォームでは未対応です",
        ),
        (
            OsFeature::NewInstance,
            "アプリの新規プロセス起動はこのプラットフォームでは未対応です",
        ),
        (
            OsFeature::Trash,
            "ゴミ箱への移動はこのプラットフォームでは未対応です",
        ),
    ]
}

/// URL の検査の理由の全種と、#2365 より前の `guarded_url` の文
fn url_blocked_table() -> Vec<(UrlBlocked, &'static str)> {
    vec![
        (
            UrlBlocked::NoScheme,
            "この URL は開けない（スキームが無い）",
        ),
        (
            UrlBlocked::DriveLetter,
            "この URL は開けない（スキームではなくドライブレター）",
        ),
        (
            UrlBlocked::Scheme,
            "この URL は開けない（許可していないスキーム）",
        ),
        (
            UrlBlocked::ControlChar,
            "この URL は開けない（制御文字入り）",
        ),
        (UrlBlocked::NoHost, "この URL は開けない（ホスト部が無い）"),
    ]
}

/// `ShellExecuteW` の戻り値の全種（と表に無い値）と、#2365 より前の `describe_shell_execute_error` の文
const SHELL_EXECUTE_OLD: [(isize, &str); 12] = [
    (0, "メモリまたはリソース不足"),
    (2, "ファイルが見つからない"),
    (3, "パスが見つからない"),
    (5, "アクセスが拒否された"),
    (8, "メモリ不足"),
    (26, "共有違反"),
    (27, "ファイルの関連付けが不完全または無効"),
    (28, "タイムアウト"),
    (29, "DDE トランザクションに失敗"),
    (31, "この種類のファイルに関連付けられたアプリが無い"),
    (32, "関連付けられた DLL が見つからない"),
    (1155, "ShellExecuteW が 1155 を返した"),
];

/// `SHFileOperationW` の戻り値の全種（と表に無い値）と、#2365 より前の `describe_shfileop_error` の文
const SHFILEOP_OLD: [(i32, &str); 8] = [
    (0x71, "同じファイルを指している"),
    (0x74, "ルートディレクトリは操作できない"),
    (0x75, "操作が取り消された"),
    (0x78, "アクセスが拒否された"),
    (0x79, "パスが MAX_PATH（260 文字）を超えている"),
    (0x7C, "パスが不正"),
    (0x402, "パスが見つからない（または不正）"),
    (0x10, "SHFileOperationW が 0x10 を返した"),
];

/// 見本と、#2365 より前の `platform::lid` の文（Windows の電源プランの文を含む）
fn lid_error_table() -> Vec<(LidError, String)> {
    let lock = LockError::Acquire {
        path: "/d/lid-guard.json.lock".into(),
        error: "Resource temporarily unavailable (os error 35)".into(),
    };
    let write = WriteError::NoParent("/x".into());
    vec![
        (
            LidError::RecordRead {
                path: "/d/lid-guard.json".into(),
                error: "Permission denied (os error 13)".into(),
            },
            "記録を読めません（/d/lid-guard.json）: Permission denied (os error 13)".into(),
        ),
        (
            LidError::RecordMalformed {
                path: "/d/lid-guard.json".into(),
                reason: JsonReason::Located {
                    kind: JsonProblem::Syntax,
                    line: 1,
                    column: 2,
                },
                quarantined: Some("/d/lid-guard.json.unreadable.bak".into()),
            },
            "蓋閉じ継続の記録を解釈できません（/d/lid-guard.json）: JSON の構文が壊れている（1 行 2 列）。\
             内容は /d/lid-guard.json.unreadable.bak へ退避しました"
                .into(),
        ),
        (
            LidError::DataDirUnresolved,
            "データディレクトリが解決できません".into(),
        ),
        (LidError::NothingToRestore, "戻す記録がありません".into()),
        (
            LidError::BadSchemeGuid("zz".into()),
            "記録の GUID を解釈できません: zz".into(),
        ),
        // ロック・書き出しの文は `config_io` の表（そちらの節が #2365 より前の文と突き合わせる）
        (LidError::Lock(lock.clone()), lock.text_in(Lang::Ja)),
        (LidError::Write(write.clone()), write.text_in(Lang::Ja)),
        // 日本語は serde の文言のまま（#2365 より前の `e.to_string()`）
        (
            LidError::Serialize("key must be a string".into()),
            "key must be a string".into(),
        ),
        (
            LidError::ActiveScheme(2),
            "PowerGetActiveScheme に失敗（rc=2）".into(),
        ),
        (
            LidError::ReadSetting {
                rail: Rail::Ac,
                rc: 2,
            },
            "蓋の設定を読めません（Ac, rc=2）".into(),
        ),
        (
            LidError::WriteSetting {
                rail: Rail::Dc,
                rc: 5,
            },
            "蓋の設定を書けません（Dc, rc=5）".into(),
        ),
        (
            LidError::ApplyScheme(5),
            "電源プランを適用できません（rc=5）".into(),
        ),
        (
            LidError::Unsupported,
            "この OS では蓋の設定を扱えません".into(),
        ),
    ]
}

/// 表の 1 行の検査（日本語は旧文とバイト一致・英語に日本語が無い・日英が違う）。落ちたら日英の対を
/// 書いた行（`file:line`）を添える
fn check_os_row(
    failures: &mut Vec<String>,
    what: &str,
    at: &dyn Fn(Lang) -> String,
    old: &str,
    ja: &str,
    en: &str,
) {
    if ja != old {
        failures.push(format!(
            "{}: {what}: 日本語が #2365 より前と違う\n  want: {old}\n  got:  {ja}",
            at(Lang::Ja)
        ));
    }
    if let Some(c) = has_japanese(en) {
        failures.push(format!(
            "{}: {what}: 英語に日本語（{c}）が混ざる: {en}",
            at(Lang::En)
        ));
    }
    if ja == en {
        failures.push(format!(
            "{}: {what}: 日英が同じ文（英語の書き忘れ）: {en}",
            at(Lang::En)
        ));
    }
    // 「見つからない」系は英語にも ` not found` を含める（リモートの HTTP 層が 404 へ寄せる判定）
    if E::says_not_found(ja) && !E::says_not_found(en) {
        failures.push(format!(
            "{}: {what}: 日本語は「見つからない」なのに英語に ` not found` が無い: {en}",
            at(Lang::En)
        ));
    }
}

#[test]
fn osのシェル連携と蓋閉じ継続の断りは全バリアントが日英を持ち日本語は2365より前の固定文とバイト一致(
) {
    let mut failures = Vec::new();

    let table = os_integration_table();
    let samples: Vec<OsIntegrationError> = table.iter().map(|(e, _)| e.clone()).collect();
    covers_every(
        &samples,
        os_integration_index,
        OS_INTEGRATION_VARIANTS,
        "OsIntegrationError",
    );
    for (e, old) in &table {
        let debug = format!("{e:?}");
        let at = |lang| {
            where_written(
                OS_INTEGRATION_SRC,
                OS_INTEGRATION_PATH,
                "impl OsIntegrationError {",
                &debug,
                lang,
            )
        };
        let (ja, en) = (e.text_in(Lang::Ja), e.text_in(Lang::En));
        check_os_row(&mut failures, &debug, &at, old, &ja, &en);
        if e.clone().into_ja() != ja {
            failures.push(format!("{debug}: into_ja が text_in(Ja) と違う"));
        }
    }

    // 枠の全種（`Spawn` で包んで見る）・未対応の全種・URL の検査の全種
    let launches = launch_table();
    covers_every(
        &launches.iter().map(|(l, _)| l.clone()).collect::<Vec<_>>(),
        launch_index,
        8,
        "Launch",
    );
    for (what, old) in &launches {
        let e = OsIntegrationError::Spawn {
            what: what.clone(),
            error: "e".into(),
        };
        let debug = format!("{what:?}");
        let at = |lang| {
            where_written(
                OS_INTEGRATION_SRC,
                OS_INTEGRATION_PATH,
                "impl Launch {",
                &debug,
                lang,
            )
        };
        check_os_row(
            &mut failures,
            &debug,
            &at,
            old,
            &e.text_in(Lang::Ja),
            &e.text_in(Lang::En),
        );
    }
    let features = os_feature_table();
    covers_every(
        &features.iter().map(|(f, _)| *f).collect::<Vec<_>>(),
        os_feature_index,
        7,
        "OsFeature",
    );
    for (feature, old) in &features {
        let e = OsIntegrationError::Unsupported(*feature);
        let debug = format!("{feature:?}");
        let at = |lang| {
            where_written(
                OS_INTEGRATION_SRC,
                OS_INTEGRATION_PATH,
                "impl OsFeature {",
                &debug,
                lang,
            )
        };
        check_os_row(
            &mut failures,
            &debug,
            &at,
            old,
            &e.text_in(Lang::Ja),
            &e.text_in(Lang::En),
        );
    }
    let blocked = url_blocked_table();
    covers_every(
        &blocked.iter().map(|(b, _)| *b).collect::<Vec<_>>(),
        url_blocked_index,
        5,
        "UrlBlocked",
    );
    for (reason, old) in &blocked {
        let e = OsIntegrationError::UrlBlocked(*reason);
        let debug = format!("{reason:?}");
        let at = |lang| {
            where_written(
                URL_GUARD_SRC,
                URL_GUARD_PATH,
                "impl UrlBlocked {",
                &debug,
                lang,
            )
        };
        check_os_row(
            &mut failures,
            &debug,
            &at,
            old,
            &e.text_in(Lang::Ja),
            &e.text_in(Lang::En),
        );
        // 診断ログへ流す従来の口（`as_str` / Display）は日本語のまま
        if format!("この URL は開けない（{reason}）") != *old {
            failures.push(format!(
                "{debug}: UrlBlocked の Display が #2365 より前と違う"
            ));
        }
    }

    // Windows の戻り値の説明（cfg の外の表 = macOS でも検査できる）
    for (code, old) in SHELL_EXECUTE_OLD {
        let at = |_| format!("{OS_INTEGRATION_PATH}（shell_execute_reason の {code} の腕）");
        check_os_row(
            &mut failures,
            &format!("ShellExecuteW {code}"),
            &at,
            old,
            &shell_execute_reason(code, Lang::Ja),
            &shell_execute_reason(code, Lang::En),
        );
    }
    for (code, old) in SHFILEOP_OLD {
        let at = |_| format!("{OS_INTEGRATION_PATH}（shfileop_reason の 0x{code:X} の腕）");
        check_os_row(
            &mut failures,
            &format!("SHFileOperationW 0x{code:X}"),
            &at,
            old,
            &shfileop_reason(code, Lang::Ja),
            &shfileop_reason(code, Lang::En),
        );
    }

    let lid = lid_error_table();
    let samples: Vec<LidError> = lid.iter().map(|(e, _)| e.clone()).collect();
    covers_every(&samples, lid_error_index, LID_ERROR_VARIANTS, "LidError");
    for (e, old) in &lid {
        let debug = format!("{e:?}");
        let at = |lang| where_written(LID_SRC, LID_PATH, "impl LidError {", &debug, lang);
        let (ja, en) = (e.text_in(Lang::Ja), e.text_in(Lang::En));
        check_os_row(&mut failures, &debug, &at, old, &ja, &en);
        if e.clone().into_ja() != ja {
            failures.push(format!("{debug}: into_ja が text_in(Ja) と違う"));
        }
    }

    // JSON の理由（tako-core。lid の記録の解釈が包みと同じ言語で引く）
    let problems = [
        (JsonProblem::Syntax, "JSON の構文が壊れている（3 行 5 列）"),
        (JsonProblem::Eof, "JSON が途中で切れている（3 行 5 列）"),
        (JsonProblem::Data, "JSON の形が期待と違う（3 行 5 列）"),
        (JsonProblem::Io, "読み取りに失敗（3 行 5 列）"),
    ];
    covers_every(
        &problems.iter().map(|(p, _)| *p).collect::<Vec<_>>(),
        json_problem_index,
        4,
        "JsonProblem",
    );
    for (kind, old) in problems {
        let r = JsonReason::Located {
            kind,
            line: 3,
            column: 5,
        };
        let debug = format!("{kind:?}");
        let at = |lang| {
            where_written(
                LOAD_ISSUE_SRC,
                LOAD_ISSUE_PATH,
                "impl JsonProblem {",
                &debug,
                lang,
            )
        };
        check_os_row(
            &mut failures,
            &debug,
            &at,
            old,
            &r.text_in(Lang::Ja),
            &r.text_in(Lang::En),
        );
    }
    report_all(failures);
}

/// 包み（`OsRefusal` の解除・Battery・ファイル操作 / `LidError` のロック・書き出し）の文は中身の型の表と
/// 同じ言語で組む・「見つからない」系は英語にも ` not found` を含む・JSON の理由は serde の誤りから
/// 採ると #2365 より前の `json_reason` と同じ・素の文言の A/B（`TAKO_2230_LEGACY`）の形は日英で同じ
#[test]
fn osのシェル連携と蓋閉じ継続の断りの包みと見つからないの読み() {
    let inner = OsIntegrationError::Spawn {
        what: Launch::Url,
        error: "e".into(),
    };
    let lid = LidError::WriteSetting {
        rail: Rail::Ac,
        rc: 5,
    };
    let lock = LockError::CreateDir("e".into());
    let write = WriteError::NoParent("/x".into());
    for lang in [Lang::Ja, Lang::En] {
        assert_eq!(
            OsRefusal::Integration(inner.clone()).text_in(lang),
            inner.text_in(lang)
        );
        assert!(OsRefusal::BatterySettingsUnopenable(inner.clone())
            .text_in(lang)
            .ends_with(&inner.text_in(lang)));
        assert!(OsRefusal::LidTeardownFailed(lid.clone())
            .text_in(lang)
            .ends_with(&lid.text_in(lang)));
        assert_eq!(
            LidError::Lock(lock.clone()).text_in(lang),
            lock.text_in(lang)
        );
        assert_eq!(
            LidError::Write(write.clone()).text_in(lang),
            write.text_in(lang)
        );
    }
    // 英語表示の枠と中身が 1 つの断りで混ざらない（dispatch の載せ方そのまま）
    let en = E::Operation(OsRefusal::LidTeardownFailed(lid).text_in(Lang::En)).text_in(Lang::En);
    assert_eq!(
        en,
        "failed to stop staying awake with the lid closed: cannot write the lid setting (Ac, rc=5)"
    );

    // 「見つからない」系（ファイル / パス / DLL が見つからない）は英語にも ` not found`
    for e in [
        OsIntegrationError::ShellExecute {
            what: Launch::DefaultApp,
            code: 2,
        },
        OsIntegrationError::ShellExecute {
            what: Launch::DefaultApp,
            code: 3,
        },
        OsIntegrationError::ShellExecute {
            what: Launch::DefaultApp,
            code: 32,
        },
        OsIntegrationError::TrashShellOp(0x402),
    ] {
        let en = e.text_in(Lang::En);
        assert!(E::says_not_found(&en), "{e:?}: {en}");
    }
    // 「パスが存在しない」は dispatch の `Reason::PathNotFound` と同じ文（日英とも）
    for lang in [Lang::Ja, Lang::En] {
        assert_eq!(
            OsIntegrationError::PathMissing("/x".into()).text_in(lang),
            Reason::PathNotFound("/x".into()).text_in(lang)
        );
    }

    // JSON の理由: serde の誤りから採る形は #2365 より前の `json_reason` と同じ（日本語）
    for body in ["", "{", "not json", "{\"a\":}"] {
        let err = serde_json::from_str::<serde_json::Value>(body).expect_err("壊れている");
        let r = JsonReason::located(&err);
        assert_eq!(
            r.text_in(Lang::Ja),
            tako_core::load_issue::json_reason(&err),
            "{body:?}"
        );
        assert!(has_japanese(&r.text_in(Lang::En)).is_none(), "{body:?}");
    }
    let raw = JsonReason::Raw("expected value at line 1 column 1".into());
    assert_eq!(raw.text_in(Lang::Ja), raw.text_in(Lang::En));
}

/// lid.rs の走査の範囲（ファイルの本体から、残留復元を飛ばす理由 = 診断ログだけへ流す
/// `should_clear_residual` を除く）
fn lid_scope_lines() -> ScopeLines {
    let whole = front_scope_lines(LID_SRC, None)?;
    let skip = front_scope_lines(LID_SRC, Some("pub fn should_clear_residual("))?;
    let (from, to) = (skip.first()?.0, skip.last()?.0);
    Some(
        whole
            .into_iter()
            .filter(|(line, _)| *line < from || *line > to)
            .collect(),
    )
}

/// 走査の範囲の行（行番号, 中身）。範囲が見つからなければ `None`
type ScopeLines = Option<Vec<(usize, &'static str)>>;

/// 断りの組み立ての中の日本語（`OsIntegrationError` / `LidError` の外に書いた文）を名指す範囲
fn os_lid_scopes() -> Vec<(&'static str, ScopeLines)> {
    vec![
        (
            OS_INTEGRATION_PATH,
            front_scope_lines(OS_INTEGRATION_SRC, None),
        ),
        (
            FINDER_REVEAL_PATH,
            front_scope_lines(FINDER_REVEAL_SRC, None),
        ),
        (LID_PATH, lid_scope_lines()),
    ]
}

#[test]
fn osのシェル連携と蓋閉じ継続の断りの組み立てに日本語の固定文が無い() {
    let mut failures = Vec::new();
    for (path, lines) in os_lid_scopes() {
        let Some(lines) = lines else {
            failures.push(format!(
                "{path}: 走査の範囲が見つからない（改名で番犬が空振りする。os_lid_scopes を直す）"
            ));
            continue;
        };
        for (line, text) in refusal_japanese(&lines) {
            failures.push(format!(
                "{path}:{line}: 断りの組み立てに日本語の固定文がある = 英語表示にもそのまま出る \
                 （OsIntegrationError / LidError へバリアントを足し、日英の対を text_in に書く）: {text}"
            ));
        }
    }
    report_all(failures);
}

/// 番犬自身の検査: lid.rs の範囲は残留復元の skip 理由（診断ログ）を外し、それ以外は外さない
#[test]
fn osのシェル連携と蓋閉じ継続の番犬の境界() {
    let lines = lid_scope_lines().expect("lid.rs の範囲を引ける");
    let has = |needle: &str| lines.iter().any(|(_, code)| code.contains(needle));
    assert!(!has("のためスキップ"), "skip 理由（診断ログ）は範囲の外");
    assert!(has("fn read_from("), "記録の読み取りは範囲の中");
    assert!(has("fn restore("), "復元は範囲の中");
    assert!(has("pub(super) fn apply("), "OS ごとの実装は範囲の中");
    // 範囲の中に植えた日本語の断りは名指される（`map_err` の閉包の中・行をまたぐ `Err(format!(`）
    let planted = "fn a() -> Result<(), LidError> {\n    let x = f().map_err(|e| LidError::Serialize(format!(\"書けない: {e}\")))?;\n    Err(LidError::BadSchemeGuid(format!(\n        \"解釈できない: {}\",\n        s\n    )))\n}\n";
    let lines = front_scope_lines(planted, Some("fn a(")).expect("頭を引ける");
    let found: Vec<usize> = refusal_japanese(&lines)
        .into_iter()
        .map(|(l, _)| l)
        .collect();
    assert_eq!(found, vec![2, 4], "断りの日本語の 2 か所だけを名指す");
}

// ---- #2365 の続き: setup 系の断り（`SetupRefusal`）と codex / agy への MCP 登録の断り（`AgentMcpError`） ----
//
// dispatch の `tako_setup_mcp` / `tako_setup` / `tako_setup_changes` / `tako_agents_sync_rules` /
// `tako_setup_deps` / `tako_setup_models` / `tako_setup_bootstrap`（系統名・status・handoff）は、以前は
// dispatch 自身の日本語の固定文（claude への MCP 登録・子の `tako setup`・answers）と、中身の口
// （`setup` / `agents_sync` / `setup_deps` / `setup_bootstrap` / `agent_mcp`）の `Err(String)` を素通し
// していた。日英の対は 2 つの型の `text_in` の網羅 match に置き、従来の `String` の口は日本語のまま
// （CLI の `tako setup` は枠が日本語 = FR-2.42.21）、dispatch だけが型の口（`*_typed`）を通す。ここでは
// (1) 全バリアントの日英と旧文のバイト一致・「見つからない」の読みが日英で同じ、(2) 型の口の本体の断りの
// 組み立て（`Err(` / `.map_err(` / `.ok_or(`）に日本語の固定文を書き足していないこと、(3) dispatch が
// 日本語の口を呼んで断りに載せていないことを `file:line` で名指す。成功の応答の注記（`Ok` の文 =
// 依存の `skipped` の detail・子の打ち切りの note・同期の未設定の message）は断りではないので見ない。
//
// bootstrap の install（インストーラの取得と実行）/ path / undo-path は、中身の tako-core の断り（profile の
// PATH ブロック `PathBlockError`・tako CLI の symlink `LinkError`・Windows のユーザー PATH `UserPathError`）と
// 一緒に移した。Windows だけが組む文も `cfg` の外の表に置いたので、macOS のこのテストで日英を見る。型の口の
// 本体が tako-core の日本語の口（`String` を返す従来の口）や `into_ja` を呼んで中身を日本語へ戻していないことも
// `file:line` で名指す

use tako_control::agent_mcp::AgentMcpError;
use tako_control::setup_refusal::FetchDetail;
use tako_control::SetupRefusal;
use tako_core::platform::user_path::UserPathError;
use tako_core::shell_profile::PathBlockError;
use tako_core::tako_cli_path::LinkError;

const SETUP_REFUSAL_SRC: &str = include_str!("../src/setup_refusal.rs");
const SETUP_REFUSAL_PATH: &str = "crates/tako-control/src/setup_refusal.rs";
const AGENT_MCP_SRC: &str = include_str!("../src/agent_mcp.rs");
const AGENT_MCP_PATH: &str = "crates/tako-control/src/agent_mcp.rs";
const SETUP_SRC: &str = include_str!("../src/setup.rs");
const SETUP_PATH: &str = "crates/tako-control/src/setup.rs";
const AGENTS_SYNC_SRC: &str = include_str!("../src/agents_sync.rs");
const AGENTS_SYNC_PATH: &str = "crates/tako-control/src/agents_sync.rs";
const SETUP_DEPS_SRC: &str = include_str!("../src/setup_deps.rs");
const SETUP_DEPS_PATH: &str = "crates/tako-control/src/setup_deps.rs";
const SETUP_BOOTSTRAP_SRC: &str = include_str!("../src/setup_bootstrap.rs");
const SETUP_BOOTSTRAP_PATH: &str = "crates/tako-control/src/setup_bootstrap.rs";
const SHELL_PROFILE_SRC: &str = include_str!("../../tako-core/src/shell_profile.rs");
const SHELL_PROFILE_PATH: &str = "crates/tako-core/src/shell_profile.rs";
const TAKO_CLI_PATH_SRC: &str = include_str!("../../tako-core/src/tako_cli_path.rs");
const TAKO_CLI_PATH_PATH: &str = "crates/tako-core/src/tako_cli_path.rs";
const USER_PATH_SRC: &str = include_str!("../../tako-core/src/platform/user_path.rs");
const USER_PATH_PATH: &str = "crates/tako-core/src/platform/user_path.rs";

/// `SetupRefusal` の通し番号（網羅 match = バリアントを足すとここがコンパイルで落ちる）
fn setup_refusal_index(r: &SetupRefusal) -> usize {
    use SetupRefusal as S;
    match r {
        S::ClaudeMcpAddSpawn(_) => 0,
        S::ClaudeMcpAddFailed { .. } => 1,
        S::McpConfigRead { .. } => 2,
        S::McpConfigNotJson { .. } => 3,
        S::McpServersNotObject => 4,
        S::McpConfigDirCreate { .. } => 5,
        S::McpConfigSerialize(_) => 6,
        S::McpConfigWrite { .. } => 7,
        S::AnswersMalformed(_) => 8,
        S::AnswersEncode(_) => 9,
        S::SetupSpawn(_) => 10,
        S::SetupStdinUnavailable => 11,
        S::AnswersSend(_) => 12,
        S::SetupWaitFailed(_) => 13,
        S::SetupFailed { .. } => 14,
        S::SelectedAgent(_) => 15,
        S::ProviderPlanKey(_) => 16,
        S::InstructionEmpty => 17,
        S::ProjectKeyEmpty => 18,
        S::ProjectCwdEmpty(_) => 19,
        S::LaunchAgent(_) => 20,
        S::ModUi(_) => 21,
        S::SleepMode(_) => 22,
        S::SleepPower(_) => 23,
        S::Profile(_) => 24,
        S::ConfigHomeUnresolved => 25,
        S::ConfigRead(_) => 26,
        S::ConfigParse(_) => 27,
        S::ChangelogParse(_) => 28,
        S::ChangelogPlatform { .. } => 29,
        S::SyncSourceUnset => 30,
        S::SyncSourceMissing(_) => 31,
        S::SyncSourceRead(_) => 32,
        S::SyncSourceEmpty => 33,
        S::UnknownDep { .. } => 34,
        S::DepInstallerSpawn { .. } => 35,
        S::DepInstallerFailed { .. } => 36,
        S::DepMissingAfterInstall { .. } => 37,
        S::UnknownBootstrapAgent { .. } => 38,
        S::BootstrapHomeUnresolved => 39,
        S::InstallDuringSelfTest => 40,
        S::InstallNotDelegable { .. } => 41,
        S::NoDownloader => 42,
        S::InstallerToolSpawn { .. } => 43,
        S::InstallerFetchFailed { .. } => 44,
        S::InstallerUnreadable(_) => 45,
        S::NotAnInstaller { .. } => 46,
        S::InstallerOutOfMemory { .. } => 47,
        S::InstallerKilled { .. } => 48,
        S::InstallerFailed { .. } => 49,
        S::InstalledButMissing { .. } => 50,
        S::ShellProfileUnknown { .. } => 51,
        S::ShellProfileUnknownForUndo => 52,
        S::UserPathNotReflected { .. } => 53,
        S::PathBlock(_) => 54,
        S::CliLink(_) => 55,
        S::UserPath(_) => 56,
    }
}

const SETUP_REFUSAL_VARIANTS: usize = 57;

/// tako-core の 3 型（bootstrap の path / undo-path の中身）の通し番号
fn path_block_index(e: &PathBlockError) -> usize {
    match e {
        PathBlockError::NoDirs => 0,
        PathBlockError::Read { .. } => 1,
        PathBlockError::CreateDir { .. } => 2,
        PathBlockError::Write { .. } => 3,
    }
}

const PATH_BLOCK_VARIANTS: usize = 4;

fn link_error_index(e: &LinkError) -> usize {
    match e {
        LinkError::Create { .. } => 0,
        LinkError::Replace { .. } => 1,
        LinkError::Remove { .. } => 2,
        LinkError::SymlinkUnsupported { .. } => 3,
    }
}

const LINK_ERROR_VARIANTS: usize = 4;

fn user_path_index(e: &UserPathError) -> usize {
    match e {
        UserPathError::Unsupported => 0,
        UserPathError::Spawn { .. } => 1,
        UserPathError::Failed { .. } => 2,
        UserPathError::BadOutput { .. } => 3,
    }
}

const USER_PATH_VARIANTS: usize = 4;

/// `AgentMcpError` の通し番号
fn agent_mcp_index(e: &AgentMcpError) -> usize {
    match e {
        AgentMcpError::CliNotFound { .. } => 0,
        AgentMcpError::ScopeUnsupported { .. } => 1,
        AgentMcpError::Spawn { .. } => 2,
        AgentMcpError::CommandFailed { .. } => 3,
        AgentMcpError::NotReflected { .. } => 4,
        AgentMcpError::NotSupportedHere { .. } => 5,
    }
}

const AGENT_MCP_VARIANTS: usize = 6;

/// `tako mod ui` に断られる answers（slash に許可リストの外のコマンド）の実物の `UiError`
fn mod_ui_refusal() -> tako_core::claude_mod_ui::UiError {
    let answers: tako_core::claude_mod_ui::UiAnswers =
        serde_json::from_str(r#"{"buttons": [{"kind": "slash", "value": "bash"}]}"#)
            .expect("answers の形");
    tako_core::claude_mod_ui::check_answers(&answers).expect_err("許可リストの外は断られる")
}

/// 見本と、#2365 より前の固定文（dispatch・`setup` / `agents_sync` / `setup_deps` / `setup_bootstrap` の
/// `format!` を見本の値で展開したもの）
fn setup_refusal_table() -> Vec<(SetupRefusal, String)> {
    use SetupRefusal as S;
    let e = || "No such file or directory (os error 2)".to_string();
    let enoent = e();
    let broken_json =
        serde_json::from_str::<serde_json::Value>("{ \"mcpServers\": ").expect_err("壊れている");
    let broken_yaml_text = "setup: [broken\n";
    let broken_yaml =
        serde_yaml::from_str::<serde_yaml::Value>(broken_yaml_text).expect_err("壊れている");
    let json_reason = tako_core::load_issue::json_error_reason(&broken_json);
    let yaml_reason = tako_core::load_issue::yaml_error_reason(&broken_yaml, broken_yaml_text);
    vec![
        (
            S::ClaudeMcpAddSpawn(e()),
            format!("claude mcp add の実行に失敗: {enoent}"),
        ),
        (
            S::ClaudeMcpAddFailed {
                status: "exit status: 1".into(),
                stderr: "boom\n".into(),
            },
            "claude mcp add が失敗 (exit exit status: 1): boom\n".into(),
        ),
        (
            S::McpConfigRead {
                path: "/h/.claude.json".into(),
                error: e(),
            },
            format!("/h/.claude.json の読み取りに失敗: {enoent}"),
        ),
        (
            S::McpConfigNotJson {
                path: "/h/.mcp.json".into(),
                reason: tako_core::load_issue::JsonReason::of(&broken_json),
            },
            format!(
                "/h/.mcp.json を JSON として解釈できないので書き換えを中止した（{json_reason}）。\
                 内容を直すか退避してからやり直してください"
            ),
        ),
        (S::McpServersNotObject, "mcpServers がオブジェクトでない".into()),
        (
            S::McpConfigDirCreate {
                path: "/h/p".into(),
                error: e(),
            },
            format!("/h/p の作成に失敗: {enoent}"),
        ),
        (
            S::McpConfigSerialize("e".into()),
            "JSON のシリアライズに失敗: e".into(),
        ),
        (
            S::McpConfigWrite {
                path: "/h/.mcp.json".into(),
                error: e(),
            },
            format!("/h/.mcp.json への書き込みに失敗: {enoent}"),
        ),
        (
            S::AnswersMalformed("invalid type: integer `1`, expected a string".into()),
            "setup answers が不正: invalid type: integer `1`, expected a string".into(),
        ),
        (
            S::AnswersEncode("e".into()),
            "setup answers の JSON 化に失敗: e".into(),
        ),
        (
            S::SetupSpawn(e()),
            format!("tako setup の起動に失敗: {enoent}"),
        ),
        (
            S::SetupStdinUnavailable,
            "tako setup の標準入力を開けない".into(),
        ),
        (
            S::AnswersSend("Broken pipe (os error 32)".into()),
            "setup answers の送信に失敗: Broken pipe (os error 32)".into(),
        ),
        (
            S::SetupWaitFailed("e".into()),
            "tako setup の完了待ちに失敗: e".into(),
        ),
        (
            S::SetupFailed {
                code: 3,
                output: "out".into(),
            },
            "tako setup が失敗しました (exit=3): out".into(),
        ),
        (
            S::SelectedAgent("x".into()),
            "selected_agent は claude / codex / agy のいずれかです: x".into(),
        ),
        (
            S::ProviderPlanKey("x".into()),
            "provider_plans のキーは claude / gpt / google のいずれかです: x".into(),
        ),
        (
            S::InstructionEmpty,
            "instruction_content は空にできません".into(),
        ),
        (S::ProjectKeyEmpty, "projects のキーは空にできません".into()),
        (
            S::ProjectCwdEmpty("k".into()),
            "projects.k.cwd は空にできません".into(),
        ),
        (
            S::LaunchAgent("x".into()),
            "launch_agent は claude / codex / agy / none のいずれかです: x".into(),
        ),
        (
            S::ModUi(mod_ui_refusal()),
            "mod_ui: slash で送れるのは許可リストのコマンドだけ。使える値: compact / clear / \
             context / cost / model / effort / tako（ui.json は書いていない）"
                .into(),
        ),
        (
            S::SleepMode("x".into()),
            "sleep_guard.mode は off / on / while-agents-running のいずれかです: x".into(),
        ),
        (
            S::SleepPower("x".into()),
            "sleep_guard.power は ac-only / always のいずれかです: x".into(),
        ),
        (
            S::Profile(ConfigError::ProfileMasterAgent(Box::new(
                ConfigError::UnknownAgent("x".into()),
            ))),
            "プロファイルの master_agent が不正: 未対応のエージェント種別 'x'（対応: claude / codex / agy）"
                .into(),
        ),
        (
            S::ConfigHomeUnresolved,
            "ホームディレクトリが取得できない（$HOME 未設定）".into(),
        ),
        (
            S::ConfigRead(e()),
            format!("config.yaml の読み取りに失敗: {enoent}"),
        ),
        (
            S::ConfigParse(YamlReason::of(&broken_yaml, broken_yaml_text)),
            format!("config.yaml のパースに失敗: {yaml_reason}"),
        ),
        (
            S::ChangelogParse(YamlReason::of(&broken_yaml, broken_yaml_text)),
            format!("changes.yaml のパースに失敗: {yaml_reason}"),
        ),
        (
            S::ChangelogPlatform {
                revision: 7,
                value: "linux".into(),
            },
            "changes.yaml revision 7 の platforms に未知の値: linux（macos / windows）".into(),
        ),
        (
            S::SyncSourceUnset,
            "共通ルールの正本パスが設定されていません。\n\
             tako setup で設定するか、--source オプションで指定してください"
                .into(),
        ),
        (
            S::SyncSourceMissing("/h/rules.md".into()),
            "正本ファイルが見つかりません: /h/rules.md\n\
             ファイルを作成するか、tako setup で正本のパスを設定してください"
                .into(),
        ),
        (
            S::SyncSourceRead(e()),
            format!("正本ファイルの読み取りに失敗: {enoent}"),
        ),
        (S::SyncSourceEmpty, "正本ファイルが空です".into()),
        (
            S::UnknownDep {
                got: "x".into(),
                allowed: "tmux / gh".into(),
            },
            "不明な依存: \"x\"（この環境の対象は tmux / gh）".into(),
        ),
        (
            S::DepInstallerSpawn {
                program: "/opt/homebrew/bin/brew".into(),
                error: e(),
            },
            format!("/opt/homebrew/bin/brew を起動できません: {enoent}"),
        ),
        (
            S::DepInstallerFailed {
                command: "brew install tmux".into(),
                code: 1,
                log: "  Error: x \n".into(),
            },
            "brew install tmux が失敗しました（exit 1）。上に出ているエラーを確認してください\nError: x"
                .into(),
        ),
        (
            S::DepMissingAfterInstall {
                command: "brew install tmux".into(),
                bin: "tmux".into(),
            },
            "brew install tmux は正常終了しましたが tmux が見つかりません。\n\
             ターミナルを開き直してから `tako setup deps` で確認してください"
                .into(),
        ),
        (
            S::UnknownBootstrapAgent {
                got: "x".into(),
                allowed: "claude / codex / agy".into(),
            },
            "不明なエージェント: \"x\"（claude / codex / agy のいずれか）".into(),
        ),
        (
            S::BootstrapHomeUnresolved,
            "ホームディレクトリを特定できません（HOME が未設定）".into(),
        ),
        (
            S::InstallDuringSelfTest,
            "セルフテスト中は実インストールを行いません（dry_run でのみ呼べます）".into(),
        ),
        (
            S::InstallNotDelegable {
                os: "linux".into(),
                command: INSTALL_CMD.into(),
            },
            format!(
                "この環境では tako が自動インストールを代行できません（{}）。\n\
                 次のコマンドを自分で実行してから `tako setup` をやり直してください:\n  {}",
                "linux", INSTALL_CMD,
            ),
        ),
        (
            S::NoDownloader,
            "curl も wget も見つかりません。どちらかを導入してから再実行してください\n\
             （macOS なら通常 curl が標準で入っています）"
                .into(),
        ),
        (
            S::InstallerToolSpawn {
                program: "/usr/bin/curl".into(),
                error: e(),
            },
            format!("{} を起動できません: {enoent}", "/usr/bin/curl"),
        ),
        (
            S::InstallerFetchFailed {
                url: INSTALL_URL.into(),
                detail: FetchDetail::Stderr("curl: (6) Could not resolve host: claude.ai".into()),
            },
            old_fetch_failed("（curl: (6) Could not resolve host: claude.ai）"),
        ),
        (
            S::InstallerFetchFailed {
                url: INSTALL_URL.into(),
                detail: FetchDetail::Exit {
                    program: "/usr/bin/curl".into(),
                    code: 22,
                },
            },
            old_fetch_failed(&format!("（{} が exit {}）", "/usr/bin/curl", 22)),
        ),
        (
            S::InstallerUnreadable(e()),
            format!("取得したインストーラを読めません: {enoent}"),
        ),
        (
            S::NotAnInstaller {
                url: INSTALL_URL.into(),
            },
            format!(
                "取得した内容がインストーラではありません（{} が HTML かエラーページを返しています）。\n\
                 社内プロキシやネットワーク制限が疑われます。ブラウザで {} を開いて中身を確認するか、\n\
                 ネットワークを変えて再実行してください",
                INSTALL_URL, INSTALL_URL
            ),
        ),
        (
            S::InstallerOutOfMemory {
                log: "  Killed \n".into(),
            },
            format!(
                "インストールがメモリ不足で中断されました（exit 137）。\
                 空きメモリを増やしてから再実行してください{}",
                "\nKilled"
            ),
        ),
        (
            S::InstallerKilled {
                code: 143,
                log: String::new(),
            },
            format!(
                "インストールが途中で強制終了しました（exit {}）。\
                 もう一度実行するか、上に出ているメッセージを確認してください{}",
                143, ""
            ),
        ),
        (
            S::InstallerFailed {
                product: "Codex CLI".into(),
                code: 1,
                troubleshoot_url: "https://learn.chatgpt.com/docs/codex/cli".into(),
                log: "error: x\n".into(),
            },
            format!(
                "{} のインストーラが失敗しました（exit {}）。上に出ているエラーを確認してください。\
                 解決しない場合は {} を参照してください{}",
                "Codex CLI", 1, "https://learn.chatgpt.com/docs/codex/cli", "\nerror: x"
            ),
        ),
        (
            S::InstalledButMissing {
                launcher: "~/.local/bin/claude".into(),
                command: INSTALL_CMD.into(),
            },
            format!(
                "インストーラは正常終了しましたが {} が見つかりません。\n\
                 `{}` を手で実行して、出力に出るエラーを確認してください",
                "~/.local/bin/claude", INSTALL_CMD
            ),
        ),
        (
            S::ShellProfileUnknown {
                shell: "/usr/bin/false".into(),
                dirs: vec!["~/.local/bin".into()],
            },
            format!(
                "使っているシェル（{}）の設定ファイルが分かりません。\n\
                 次の {} 行をご自身のシェルの設定ファイルへ追加してください:\n{}",
                "/usr/bin/false", 1, "  export PATH=\"~/.local/bin:$PATH\""
            ),
        ),
        (
            S::ShellProfileUnknown {
                shell: String::new(),
                dirs: vec!["~/.local/bin".into(), "/opt/tako/bin".into()],
            },
            format!(
                "使っているシェル（{}）の設定ファイルが分かりません。\n\
                 次の {} 行をご自身のシェルの設定ファイルへ追加してください:\n{}",
                "不明",
                2,
                "  export PATH=\"~/.local/bin:$PATH\"\n  export PATH=\"/opt/tako/bin:$PATH\""
            ),
        ),
        (
            S::ShellProfileUnknownForUndo,
            "使っているシェルの設定ファイルが分かりません".into(),
        ),
        (
            S::UserPathNotReflected {
                dir: "~\\.local\\bin".into(),
            },
            format!(
                "ユーザー PATH へ {} を追加できませんでした（書き込み後も反映されていません）。\n\
                 設定 → システム → バージョン情報 → 環境変数 から手で追加してください",
                "~\\.local\\bin"
            ),
        ),
        (
            S::PathBlock(PathBlockError::Write {
                path: "/h/.zprofile".into(),
                error: e(),
            }),
            format!("{} へ書けません: {enoent}", "/h/.zprofile"),
        ),
        (
            S::CliLink(LinkError::Remove {
                path: "/h/.local/bin/tako".into(),
                error: e(),
            }),
            format!("{} を削除できません: {enoent}", "/h/.local/bin/tako"),
        ),
        (
            S::UserPath(UserPathError::Unsupported),
            "このプラットフォームにはプロセス外に永続する PATH がありません（profile 経路を使ってください）"
                .into(),
        ),
    ]
}

/// 見本の公式インストーラ（取得元と 1 行）
const INSTALL_URL: &str = "https://claude.ai/install.sh";
const INSTALL_CMD: &str = "curl -fsSL https://claude.ai/install.sh | bash";

/// #2365 より前の `fetch_installer` の取得失敗の文（`detail` は括弧つきの添え書き）
fn old_fetch_failed(detail: &str) -> String {
    format!(
        "インストーラを取得できませんでした: {}\n\
         {}\n\
         ネットワーク接続とプロキシ設定を確認してから再実行してください。\n\
         社内プロキシ環境では {} へのアクセス許可が要ることがあります",
        INSTALL_URL, detail, INSTALL_URL,
    )
}

/// tako-core の profile の PATH ブロックの断りの見本と、#2365 より前の `shell_profile` の固定文
fn path_block_table() -> Vec<(PathBlockError, String)> {
    let enoent = "No such file or directory (os error 2)";
    let p = "/h/.zprofile";
    vec![
        (
            PathBlockError::NoDirs,
            "PATH へ通すディレクトリが 1 つもありません".into(),
        ),
        (
            PathBlockError::Read {
                path: p.into(),
                error: enoent.into(),
            },
            format!("{} を読めません: {enoent}", p),
        ),
        (
            PathBlockError::CreateDir {
                path: "/h".into(),
                error: enoent.into(),
            },
            format!("{} を作成できません: {enoent}", "/h"),
        ),
        (
            PathBlockError::Write {
                path: p.into(),
                error: enoent.into(),
            },
            format!("{} へ書けません: {enoent}", p),
        ),
    ]
}

/// tako CLI の symlink の断りの見本と、#2365 より前の `tako_cli_path` の固定文（Windows だけの
/// `SymlinkUnsupported` は、io::Error に包んだ文を「作成できません」が包んでいた形のまま）
fn link_error_table() -> Vec<(LinkError, String)> {
    let enoent = "No such file or directory (os error 2)";
    let l = "/h/.local/bin/tako";
    vec![
        (
            LinkError::Create {
                path: l.into(),
                error: enoent.into(),
            },
            format!("{} を作成できません: {enoent}", l),
        ),
        (
            LinkError::Replace {
                path: l.into(),
                error: enoent.into(),
            },
            format!("{} を置き換えられません: {enoent}", l),
        ),
        (
            LinkError::Remove {
                path: l.into(),
                error: enoent.into(),
            },
            format!("{} を削除できません: {enoent}", l),
        ),
        (
            LinkError::SymlinkUnsupported {
                path: "C:\\Users\\winuser\\.local\\bin\\tako".into(),
            },
            // 外側の `{} を作成できません: {e}` の `{e}` に、io::Error に包んだ内側の文が入っていた
            format!(
                "{} を作成できません: {} への symlink はこの OS では張れません（ユーザー環境変数 Path を使います）",
                "C:\\Users\\winuser\\.local\\bin\\tako", "C:\\Users\\winuser\\.local\\bin\\tako"
            ),
        ),
    ]
}

/// Windows のユーザー PATH の断りの見本と、#2365 より前の `user_path` の固定文
fn user_path_table() -> Vec<(UserPathError, String)> {
    vec![
        (
            UserPathError::Unsupported,
            "このプラットフォームにはプロセス外に永続する PATH がありません（profile 経路を使ってください）"
                .into(),
        ),
        (
            UserPathError::Spawn {
                program: "powershell.exe".into(),
                error: "Access is denied. (os error 5)".into(),
            },
            format!(
                "{} を起動できません: {}",
                "powershell.exe", "Access is denied. (os error 5)"
            ),
        ),
        (
            UserPathError::Failed {
                code: 1,
                stderr: "Access is denied.".into(),
            },
            format!(
                "PowerShell がユーザー PATH の操作に失敗しました（exit {}）: {}",
                1, "Access is denied."
            ),
        ),
        (
            UserPathError::BadOutput {
                output: "TAKO_KIND=ExpandString".into(),
            },
            format!(
                "ユーザー PATH の読み出し結果を解釈できません: {:?}",
                "TAKO_KIND=ExpandString"
            ),
        ),
    ]
}

/// 見本と、#2365 より前の `Display` の文
fn agent_mcp_table() -> Vec<(AgentMcpError, String)> {
    use AgentMcpError as A;
    let codex = WorkerAgent::Codex;
    vec![
        (
            A::CliNotFound { agent: codex },
            "codex CLI が見つからないので MCP 登録できません。次の一手: codex を導入して PATH を通してから \
             tako setup-mcp --agent codex をやり直してください（導入の案内: https://developers.openai.com/codex/cli）"
                .into(),
        ),
        (
            A::ScopeUnsupported {
                agent: WorkerAgent::Agy,
                scope: "project",
            },
            "agy は scope=project の MCP 登録に対応していません（ユーザーグローバルのみ）。\
             次の一手: --project を外して tako setup-mcp --agent agy を実行してください"
                .into(),
        ),
        (
            A::Spawn {
                agent: codex,
                detail: "Permission denied (os error 13)".into(),
            },
            "codex mcp add を起動できませんでした（Permission denied (os error 13)）。\
             次の一手: which codex でパスを確認し、実行権限があるか見てください"
                .into(),
        ),
        (
            A::CommandFailed {
                agent: codex,
                code: None,
                stderr: "  bad flag \n".into(),
            },
            "codex mcp add が失敗しました (exit signal): bad flag。\
             次の一手: 同じコマンドを手で実行して出力を確認してください（codex mcp add --help）"
                .into(),
        ),
        (
            A::NotReflected { agent: codex },
            "codex mcp add は成功したのに codex mcp list に tako が現れません（書式非対応・バージョン差の疑い）。\
             次の一手: codex --version と codex mcp list の出力を添えて tako の Issue へ報告してください"
                .into(),
        ),
        (
            A::NotSupportedHere {
                agent: WorkerAgent::Claude,
            },
            "claude の MCP 登録はこの経路では扱いません（claude は dispatch::setup_mcp が担当）".into(),
        ),
    ]
}

/// [`check_os_row`] に、「見つからない」の読みが日英で**同じ**こと（英語だけ ` not found` を含めて
/// 404 へ寄るのも許さない）を足した 1 行の検査
fn check_setup_row(
    failures: &mut Vec<String>,
    what: &str,
    at: &dyn Fn(Lang) -> String,
    old: &str,
    ja: &str,
    en: &str,
) {
    check_os_row(failures, what, at, old, ja, en);
    if E::says_not_found(en) && !E::says_not_found(ja) {
        failures.push(format!(
            "{}: {what}: 日本語は「見つからない」と言わないのに英語に ` not found` がある \
             （リモートの HTTP 層が英語表示だけ 404 へ寄せる）: {en}",
            at(Lang::En)
        ));
    }
}

#[test]
fn setup系の断りは全バリアントが日英を持ち日本語は2365より前の固定文とバイト一致() {
    let mut failures = Vec::new();

    let table = setup_refusal_table();
    covers_every(
        &table.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>(),
        setup_refusal_index,
        SETUP_REFUSAL_VARIANTS,
        "SetupRefusal",
    );
    for (r, old) in &table {
        let debug = format!("{r:?}");
        let at = |lang| {
            where_written(
                SETUP_REFUSAL_SRC,
                SETUP_REFUSAL_PATH,
                "impl SetupRefusal {",
                &debug,
                lang,
            )
        };
        let (ja, en) = (r.text_in(Lang::Ja), r.text_in(Lang::En));
        check_setup_row(&mut failures, &debug, &at, old, &ja, &en);
        if r.clone().into_ja() != ja {
            failures.push(format!("{debug}: into_ja が text_in(Ja) と違う"));
        }
    }

    let table = agent_mcp_table();
    covers_every(
        &table.iter().map(|(e, _)| e.clone()).collect::<Vec<_>>(),
        agent_mcp_index,
        AGENT_MCP_VARIANTS,
        "AgentMcpError",
    );
    for (e, old) in &table {
        let debug = format!("{e:?}");
        let at = |lang| {
            where_written(
                AGENT_MCP_SRC,
                AGENT_MCP_PATH,
                "    pub fn text_in(&self, lang: tako_core::i18n::Lang) -> String {",
                &debug,
                lang,
            )
        };
        let (ja, en) = (e.text_in(Lang::Ja), e.text_in(Lang::En));
        check_setup_row(&mut failures, &debug, &at, old, &ja, &en);
    }

    // bootstrap の path / undo-path の中身（tako-core の 3 型）
    let table = path_block_table();
    covers_every(
        &table.iter().map(|(e, _)| e.clone()).collect::<Vec<_>>(),
        path_block_index,
        PATH_BLOCK_VARIANTS,
        "PathBlockError",
    );
    for (e, old) in &table {
        let debug = format!("{e:?}");
        let at = |lang| {
            where_written(
                SHELL_PROFILE_SRC,
                SHELL_PROFILE_PATH,
                "impl PathBlockError {",
                &debug,
                lang,
            )
        };
        let (ja, en) = (e.text_in(Lang::Ja), e.text_in(Lang::En));
        check_setup_row(&mut failures, &debug, &at, old, &ja, &en);
        if e.clone().into_ja() != ja {
            failures.push(format!("{debug}: into_ja が text_in(Ja) と違う"));
        }
    }
    let table = link_error_table();
    covers_every(
        &table.iter().map(|(e, _)| e.clone()).collect::<Vec<_>>(),
        link_error_index,
        LINK_ERROR_VARIANTS,
        "LinkError",
    );
    for (e, old) in &table {
        let debug = format!("{e:?}");
        let at = |lang| {
            where_written(
                TAKO_CLI_PATH_SRC,
                TAKO_CLI_PATH_PATH,
                "impl LinkError {",
                &debug,
                lang,
            )
        };
        let (ja, en) = (e.text_in(Lang::Ja), e.text_in(Lang::En));
        check_setup_row(&mut failures, &debug, &at, old, &ja, &en);
        if e.clone().into_ja() != ja {
            failures.push(format!("{debug}: into_ja が text_in(Ja) と違う"));
        }
    }
    let table = user_path_table();
    covers_every(
        &table.iter().map(|(e, _)| e.clone()).collect::<Vec<_>>(),
        user_path_index,
        USER_PATH_VARIANTS,
        "UserPathError",
    );
    for (e, old) in &table {
        let debug = format!("{e:?}");
        let at = |lang| {
            where_written(
                USER_PATH_SRC,
                USER_PATH_PATH,
                "impl UserPathError {",
                &debug,
                lang,
            )
        };
        let (ja, en) = (e.text_in(Lang::Ja), e.text_in(Lang::En));
        check_setup_row(&mut failures, &debug, &at, old, &ja, &en);
        if e.clone().into_ja() != ja {
            failures.push(format!("{debug}: into_ja が text_in(Ja) と違う"));
        }
    }
    report_all(failures);
}

/// 包み（answers の `mod_ui: ` / profile の断り / config.yaml・changes.yaml のパース / MCP 設定の JSON）は
/// 中身の型と同じ言語で組む・導入器の出力の添え方（空なら添えない）・dispatch の載せ方で 1 つの断りに
/// 日英が混ざらない・answers の検証は型の口が表示言語、従来の口が日本語
#[test]
fn setup系の断りの包みと載せ方() {
    let ui = mod_ui_refusal();
    let profile = ConfigError::ProfileWorkerAgent(Box::new(ConfigError::UnknownAgent("x".into())));
    let broken = "a: [\n";
    let yaml = serde_yaml::from_str::<serde_yaml::Value>(broken).expect_err("壊れている");
    let reason = YamlReason::of(&yaml, broken);
    let json = serde_json::from_str::<serde_json::Value>("{").expect_err("壊れている");
    let json_reason = tako_core::load_issue::JsonReason::of(&json);
    for lang in [Lang::Ja, Lang::En] {
        assert!(SetupRefusal::ModUi(ui.clone())
            .text_in(lang)
            .ends_with(&ui.text_in(lang)));
        assert_eq!(
            SetupRefusal::Profile(profile.clone()).text_in(lang),
            profile.text_in(lang)
        );
        assert!(SetupRefusal::ConfigParse(reason.clone())
            .text_in(lang)
            .ends_with(&reason.text_in(lang)));
        assert!(SetupRefusal::McpConfigNotJson {
            path: "/x".into(),
            reason: json_reason.clone(),
        }
        .text_in(lang)
        .contains(&json_reason.text_in(lang)));
        // 導入器の出力が空なら添えない（改行で終わらない）
        let quiet = SetupRefusal::DepInstallerFailed {
            command: "brew install tmux".into(),
            code: 1,
            log: " \n ".into(),
        }
        .text_in(lang);
        assert!(!quiet.contains('\n'), "{quiet:?}");
    }
    // dispatch の載せ方そのまま（`invalid` = 前置き + 理由。前置きと理由の言語がそろう）
    assert_eq!(
        E::InvalidParams(SetupRefusal::SelectedAgent("x".into()).text_in(Lang::En))
            .text_in(Lang::En),
        "invalid parameter: selected_agent must be one of claude / codex / agy: x"
    );
    assert_eq!(
        E::InvalidParams(SetupRefusal::SelectedAgent("x".into()).text_in(Lang::Ja))
            .text_in(Lang::Ja),
        "無効なパラメータ: selected_agent は claude / codex / agy のいずれかです: x"
    );
    // answers の検証: 型の口は中身の型を返し、従来の口（CLI の `tako setup`）は日本語の文
    let answers: tako_control::setup::SetupAnswers =
        serde_json::from_str(r#"{"sleep_guard": {"power": "battery"}}"#).expect("answers の形");
    let typed = answers.validate_typed().expect_err("power の値が不正");
    assert_eq!(typed, SetupRefusal::SleepPower("battery".into()));
    assert_eq!(answers.validate().expect_err("同じ断り"), typed.into_ja());

    // bootstrap: 中身の型（tako-core）を包む断りは中身と同じ言語で組む・インストーラの出力の添え方
    // （空なら添えない）・従来の口（tako-core の `String` の口）は日本語
    let block = PathBlockError::NoDirs;
    let link = LinkError::SymlinkUnsupported {
        path: "C:\\x".into(),
    };
    let user = UserPathError::Failed {
        code: 1,
        stderr: "e".into(),
    };
    for lang in [Lang::Ja, Lang::En] {
        assert_eq!(
            SetupRefusal::from(block.clone()).text_in(lang),
            block.text_in(lang)
        );
        assert_eq!(
            SetupRefusal::from(link.clone()).text_in(lang),
            link.text_in(lang)
        );
        assert_eq!(
            SetupRefusal::from(user.clone()).text_in(lang),
            user.text_in(lang)
        );
        for quiet in [
            SetupRefusal::InstallerOutOfMemory { log: " \n".into() },
            SetupRefusal::InstallerKilled {
                code: 130,
                log: String::new(),
            },
        ] {
            let text = quiet.text_in(lang);
            assert!(!text.contains('\n'), "{text:?}");
        }
    }
    // 英語の「1 行 / N 行」
    let one = SetupRefusal::ShellProfileUnknown {
        shell: String::new(),
        dirs: vec!["~/.local/bin".into()],
    };
    assert!(one
        .text_in(Lang::En)
        .contains("(unknown) uses.\nAdd the following line to"));
    // 従来の口（tako-core）: 日本語の文
    let home = std::env::temp_dir().join(format!("tako-2365b-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).expect("一時の HOME");
    assert_eq!(
        tako_core::shell_profile::ensure_dirs_on_path_in(
            &home,
            tako_core::shell_profile::ShellKind::Zsh,
            &[],
            None
        )
        .expect_err("空は断る"),
        block.text_in(Lang::Ja)
    );
    assert_eq!(
        tako_core::shell_profile::ensure_dirs_on_path_in_typed(
            &home,
            tako_core::shell_profile::ShellKind::Zsh,
            &[],
            None
        )
        .expect_err("空は断る"),
        PathBlockError::NoDirs
    );
    let _ = std::fs::remove_dir_all(&home);
    assert_eq!(
        tako_core::platform::user_path::parse_read_output("x").expect_err("目印が無い"),
        UserPathError::BadOutput { output: "x".into() }.text_in(Lang::Ja)
    );
}

/// 型の口の本体（断りの文は `SetupRefusal` / `AgentMcpError` / `ConfigError` にだけ置く）
const SETUP_REFUSAL_SCOPES: &[(&str, &str, Span)] = &[
    (
        DISPATCH_PATH,
        DISPATCH_SRC,
        Span::Item("pub fn setup_mcp_typed("),
    ),
    (
        DISPATCH_PATH,
        DISPATCH_SRC,
        Span::Item("pub fn setup_mcp_agents("),
    ),
    (
        DISPATCH_PATH,
        DISPATCH_SRC,
        Span::Item("fn setup_mcp_via_cli("),
    ),
    (
        DISPATCH_PATH,
        DISPATCH_SRC,
        Span::Item("fn setup_mcp_direct("),
    ),
    (
        DISPATCH_PATH,
        DISPATCH_SRC,
        Span::Item("fn run_setup_cli_within("),
    ),
    (
        SETUP_PATH,
        SETUP_SRC,
        Span::Item("    pub fn validate_typed("),
    ),
    (
        SETUP_PATH,
        SETUP_SRC,
        Span::Item("pub fn config_yaml_path_typed("),
    ),
    (
        SETUP_PATH,
        SETUP_SRC,
        Span::Item("pub fn load_config_typed("),
    ),
    (
        SETUP_PATH,
        SETUP_SRC,
        Span::Item("pub fn load_config_from_typed("),
    ),
    (SETUP_PATH, SETUP_SRC, Span::Item("fn parse_changes(")),
    (SETUP_PATH, SETUP_SRC, Span::Item("fn changes_for_typed(")),
    (
        SETUP_PATH,
        SETUP_SRC,
        Span::Item("pub fn changes_status_typed("),
    ),
    (
        AGENTS_SYNC_PATH,
        AGENTS_SYNC_SRC,
        Span::Item("pub fn sync_rules("),
    ),
    (
        AGENTS_SYNC_PATH,
        AGENTS_SYNC_SRC,
        Span::Item("pub fn check_sync_status("),
    ),
    (
        AGENTS_SYNC_PATH,
        AGENTS_SYNC_SRC,
        Span::Item("pub fn run_sync_typed("),
    ),
    (
        AGENTS_SYNC_PATH,
        AGENTS_SYNC_SRC,
        Span::Item("pub fn status_typed("),
    ),
    (
        SETUP_DEPS_PATH,
        SETUP_DEPS_SRC,
        Span::Item("pub fn install_typed("),
    ),
    (
        SETUP_DEPS_PATH,
        SETUP_DEPS_SRC,
        Span::Item("fn run_installer("),
    ),
    (
        SETUP_BOOTSTRAP_PATH,
        SETUP_BOOTSTRAP_SRC,
        Span::Item("pub fn parse_agent_typed("),
    ),
    (
        SETUP_BOOTSTRAP_PATH,
        SETUP_BOOTSTRAP_SRC,
        Span::Item("fn home_dir_typed("),
    ),
    (
        SETUP_BOOTSTRAP_PATH,
        SETUP_BOOTSTRAP_SRC,
        Span::Item("fn install_plan_for_typed("),
    ),
    (
        SETUP_BOOTSTRAP_PATH,
        SETUP_BOOTSTRAP_SRC,
        Span::Item("pub fn status_for_typed("),
    ),
    (
        SETUP_BOOTSTRAP_PATH,
        SETUP_BOOTSTRAP_SRC,
        Span::Item("fn status_with("),
    ),
    (
        SETUP_BOOTSTRAP_PATH,
        SETUP_BOOTSTRAP_SRC,
        Span::Item("pub fn handoff_plan_for_typed("),
    ),
    // bootstrap の install / path / undo-path（BOOTSTRAP_TYPED_HEADS）と中身の tako-core の 3 型
    (
        SETUP_BOOTSTRAP_PATH,
        SETUP_BOOTSTRAP_SRC,
        Span::Item("pub fn install_for_typed("),
    ),
    (
        SETUP_BOOTSTRAP_PATH,
        SETUP_BOOTSTRAP_SRC,
        Span::Item("fn fetch_installer("),
    ),
    (
        SETUP_BOOTSTRAP_PATH,
        SETUP_BOOTSTRAP_SRC,
        Span::Item("fn run_installer("),
    ),
    (
        SETUP_BOOTSTRAP_PATH,
        SETUP_BOOTSTRAP_SRC,
        Span::Item("fn ensure_tako_cli_link("),
    ),
    (
        SETUP_BOOTSTRAP_PATH,
        SETUP_BOOTSTRAP_SRC,
        Span::Item("fn ensure_dirs_on_path("),
    ),
    (
        SETUP_BOOTSTRAP_PATH,
        SETUP_BOOTSTRAP_SRC,
        Span::Item("pub fn ensure_path_for_typed("),
    ),
    (
        SETUP_BOOTSTRAP_PATH,
        SETUP_BOOTSTRAP_SRC,
        Span::Item("fn ensure_user_path("),
    ),
    (
        SETUP_BOOTSTRAP_PATH,
        SETUP_BOOTSTRAP_SRC,
        Span::Item("fn ensure_user_path_dirs("),
    ),
    (
        SETUP_BOOTSTRAP_PATH,
        SETUP_BOOTSTRAP_SRC,
        Span::Item("pub fn undo_path_for_typed("),
    ),
    (
        SETUP_BOOTSTRAP_PATH,
        SETUP_BOOTSTRAP_SRC,
        Span::Item("fn undo_user_path("),
    ),
    // tako-core はファイルの本体ごと（Windows の `imp` も字面で見る）
    (SHELL_PROFILE_PATH, SHELL_PROFILE_SRC, Span::Whole),
    (TAKO_CLI_PATH_PATH, TAKO_CLI_PATH_SRC, Span::Whole),
    (USER_PATH_PATH, USER_PATH_SRC, Span::Whole),
];

/// bootstrap の install / path / undo-path の型の口の本体（dispatch から下の段）
const BOOTSTRAP_TYPED_HEADS: [&str; 10] = [
    "pub fn install_for_typed(",
    "fn fetch_installer(",
    "fn run_installer(",
    "fn ensure_tako_cli_link(",
    "fn ensure_dirs_on_path(",
    "pub fn ensure_path_for_typed(",
    "fn ensure_user_path(",
    "fn ensure_user_path_dirs(",
    "pub fn undo_path_for_typed(",
    "fn undo_user_path(",
];

/// 型の口の本体が呼んではいけない口（tako-core と setup_bootstrap の `String` を返す日本語の口・
/// 型を日本語の文へ戻す `into_ja` / `Lang::Ja`）。呼ぶと中身だけ日本語のまま型の口を通り抜ける
const JA_BOOTSTRAP_INNER_CALLS: [&str; 12] = [
    "shell_profile::ensure_dirs_on_path_in(",
    "shell_profile::ensure_dir_on_path_in(",
    "shell_profile::remove_from_profile_in(",
    "tako_cli_path::ensure_link(",
    "tako_cli_path::remove_link(",
    "user_path::read()",
    "user_path::write(",
    "install_plan_for(",
    "home_dir()",
    "into_ja",
    "Lang::Ja",
    "parse_read_output(",
];

/// 型の口の本体で、日本語の口を呼んで断りに載せている行（断りを捨てる文は見ない）
fn ja_bootstrap_inner_calls(lines: &[(usize, &str)]) -> Vec<(usize, &'static str, String)> {
    let mut out = Vec::new();
    for (i, (line, code)) in lines.iter().enumerate() {
        if code.trim_start().starts_with("//") {
            continue;
        }
        if let Some(call) = JA_BOOTSTRAP_INNER_CALLS.iter().find(|c| code.contains(*c)) {
            if !statement_discards_error(lines, i) {
                out.push((*line, *call, code.trim().to_string()));
            }
        }
    }
    out
}

#[test]
fn bootstrap_の型の口は中身の日本語の口を呼ばない() {
    let mut failures = Vec::new();
    for head in BOOTSTRAP_TYPED_HEADS {
        let Some(lines) = front_scope_lines(SETUP_BOOTSTRAP_SRC, Some(head)) else {
            failures.push(format!(
                "{SETUP_BOOTSTRAP_PATH}: 走査の範囲 `{head}` が見つからない\
                 （改名で番犬が空振りする。BOOTSTRAP_TYPED_HEADS を直す）"
            ));
            continue;
        };
        for (line, call, code) in ja_bootstrap_inner_calls(&lines) {
            failures.push(format!(
                "{SETUP_BOOTSTRAP_PATH}:{line}: 型の口が日本語の口 `{call}` を呼んでいる = 英語表示でも \
                 中身だけ日本語で混ざる（tako-core の `*_typed` を呼び、SetupRefusal の包みで載せる）: {code}"
            ));
        }
    }
    report_all(failures);
}

#[test]
fn setup系の型の口の断りに日本語の固定文が無い() {
    let mut failures = Vec::new();
    for (path, src, span) in SETUP_REFUSAL_SCOPES {
        let Some(lines) = client_store_lines(src, span) else {
            failures.push(format!(
                "{path}: 走査の範囲 {} が見つからない（改名で番犬が空振りする。SETUP_REFUSAL_SCOPES を直す）",
                span_name(span)
            ));
            continue;
        };
        for (line, text) in refusal_japanese(&lines) {
            failures.push(format!(
                "{path}:{line}: 型の口の断りに日本語の固定文がある = 英語表示にもそのまま出る \
                 （SetupRefusal へバリアントを足し、日英の対を text_in に書く）: {text}"
            ));
        }
    }
    report_all(failures);
}

/// dispatch が呼んではいけない日本語の口（型の口 `*_typed` の外側）。`setup_mcp(&` は claude への
/// MCP 登録の日本語の口（定義の行 `pub fn setup_mcp(tako_binary` には当たらない）
const JA_SETUP_CALLS: [&str; 14] = [
    "setup_mcp(&",
    "setup::changes_status()",
    "setup::load_config()",
    "setup_bootstrap::parse_agent(",
    "setup_bootstrap::status_for(",
    "setup_bootstrap::handoff_plan_for(",
    "setup_bootstrap::install_for(",
    "setup_bootstrap::ensure_path_for(",
    "setup_bootstrap::undo_path_for(",
    "setup_deps::install(",
    "agents_sync::run_sync(",
    "agents_sync::status()",
    "WorkerAgent::parse(",
    ".validate().map_err(",
];

/// dispatch の本体で、日本語の口を呼んで断りに載せている行（断りを捨てる文は見ない）
fn ja_setup_calls(lines: &[(usize, &str)]) -> Vec<(usize, &'static str, String)> {
    let mut out = Vec::new();
    for (i, (line, code)) in lines.iter().enumerate() {
        if code.trim_start().starts_with("//") {
            continue;
        }
        if let Some(call) = JA_SETUP_CALLS.iter().find(|c| code.contains(*c)) {
            if !statement_discards_error(lines, i) {
                out.push((*line, *call, code.trim().to_string()));
            }
        }
    }
    out
}

/// dispatch の setup 系の分岐（`Request::SetupMcp` から `Request::AgentsSyncRules` の終わりまで）と、
/// その先の関数（claude への MCP 登録・子の `tako setup`）。`orchestrator layout` の config.yaml の
/// 読み書き（`setup::load_config()` / `mutate_config`）は別の行（#2365 の表の「config.yaml 1」）
fn setup_dispatch_lines() -> Vec<(usize, &'static str)> {
    let all = dispatch_code_lines();
    let mut out = Vec::new();
    let mut inside = false;
    for (line, code) in all.iter().copied() {
        if code.contains("Request::SetupMcp {") || code.contains("Request::SetupChanges =>") {
            inside = true;
        }
        // 次の分岐の頭で止める（頭の行は範囲に入れない）
        if code.contains("Request::VideoPlayback {") || code.contains("Request::SleepGuard {") {
            inside = false;
        }
        if inside {
            out.push((line, code));
        }
    }
    for head in [
        "pub fn setup_mcp_typed(",
        "pub fn setup_mcp_agents(",
        "fn run_setup_cli_within(",
    ] {
        out.extend(front_scope_lines(DISPATCH_SRC, Some(head)).unwrap_or_default());
    }
    out
}

#[test]
fn dispatch_はsetup系の断りを日本語の口から載せない() {
    let lines = setup_dispatch_lines();
    assert!(
        lines.iter().any(|(_, c)| c.contains("Request::AgentsSyncRules {")),
        "dispatch の setup 系の分岐を引けない（範囲の印の改名で番犬が空振りする。setup_dispatch_lines を直す）"
    );
    let mut failures = Vec::new();
    for (line, call, code) in ja_setup_calls(&lines) {
        failures.push(format!(
            "{DISPATCH_PATH}:{line}: dispatch が日本語の口 `{call}…)` の断りを載せている = 英語表示でも \
             中身だけ日本語で混ざる（`*_typed` を呼んで SetupRefusal / ConfigError の operation / invalid で載せる）: {code}"
        ));
    }
    report_all(failures);
}

/// 番犬自身の検査: 型の口の範囲と dispatch の範囲が引け、植えた日本語・日本語の口の呼び足しを
/// 名指し、断りを捨てる文と定義の行は見逃す
#[test]
fn setup系の番犬の境界() {
    let lines = setup_dispatch_lines();
    let has = |needle: &str| lines.iter().any(|(_, code)| code.contains(needle));
    assert!(has("Request::SetupMcp {"), "setup-mcp の分岐は範囲の中");
    assert!(has("Request::SetupRun {"), "tako setup の分岐は範囲の中");
    assert!(
        has("fn run_setup_cli_within("),
        "子の tako setup は範囲の中"
    );
    assert!(!has("Request::VideoPlayback {"), "動画の分岐は範囲の外");
    assert!(
        !has("Request::SleepGuard {"),
        "スリープ防止の分岐は範囲の外"
    );

    let src = r#"fn d() {
    // crate::setup_deps::install(dep) はコメント
    let a = crate::setup_deps::install(dep, opts).map_err(DispatchError::Operation)?;
    let kind = WorkerAgent::parse(name).ok();
    let b = crate::setup_deps::install_typed(dep, opts).map_err(SetupRefusal::operation)?;
    let r = setup_mcp(&tako_bin, scope)?;
    parsed.validate().map_err(DispatchError::InvalidParams)?;
}
pub fn setup_mcp(tako_binary: &str) -> Result<(), String> {
    Err(SetupRefusal::SyncSourceRead(format!("正本ファイルの読み取りに失敗: {e}")))
}
"#;
    let d = front_scope_lines(src, Some("fn d(")).expect("頭を引ける");
    let found: Vec<usize> = ja_setup_calls(&d).into_iter().map(|(l, _, _)| l).collect();
    assert_eq!(found, vec![3, 6, 7], "日本語の口の 3 か所だけを名指す");
    let f = front_scope_lines(src, Some("pub fn setup_mcp(")).expect("頭を引ける");
    assert!(ja_setup_calls(&f).is_empty(), "定義の行は呼び出しではない");
    let found: Vec<usize> = refusal_japanese(&f).into_iter().map(|(l, _)| l).collect();
    assert_eq!(found, vec![10], "型の中へ書き戻した日本語を名指す");

    // bootstrap の型の口: 範囲が引け、中身の日本語の口の呼び足しを名指し、捨てる文とコメントは見逃す
    for head in BOOTSTRAP_TYPED_HEADS {
        assert!(
            front_scope_lines(SETUP_BOOTSTRAP_SRC, Some(head)).is_some(),
            "{head} の範囲を引けない"
        );
    }
    assert!(
        has("crate::setup_bootstrap::install_for_typed("),
        "bootstrap の install は dispatch の範囲の中"
    );
    let src = r#"fn ensure_dirs_on_path(home: &Path) -> Result<Value, SetupRefusal> {
    // shell_profile::ensure_dirs_on_path_in(home) はコメント
    let a = shell_profile::ensure_dirs_on_path_in(home, shell, dirs, None).map_err(|e| SetupRefusal::InstallerUnreadable(e))?;
    let on = user_path::read().map(|v| v.raw).unwrap_or_default();
    let b = shell_profile::ensure_dirs_on_path_in_typed(home, shell, dirs, None)?;
    let c = tako_cli_path::remove_link_typed(link).map_err(LinkError::into_ja);
    let d = install_plan_for(agent)?;
}
"#;
    let d = front_scope_lines(src, Some("fn ensure_dirs_on_path(")).expect("頭を引ける");
    let found: Vec<usize> = ja_bootstrap_inner_calls(&d)
        .into_iter()
        .map(|(l, _, _)| l)
        .collect();
    assert_eq!(found, vec![3, 6, 7], "日本語の口の 3 か所だけを名指す");
}
