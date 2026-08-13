//! ビュー単位の描画キャッシュ（Issue #786）
//!
//! tako は `TakoApp` 1 個をすべてのウィンドウのルートビューにしている（#339）。
//! そのままだと `cx.notify()` 1 回でアプリ全体（タブバー・サイドバー・右パネル・
//! ステータスバー・表示中タブの全ペイン）の element ツリーを作り直し、GPUI が
//! それを丸ごとレイアウト（taffy）してペイントする。#782 の実測では、端末の中身に
//! 関係なく**毎フレーム 5.1M instructions の固定費**がここで溶けていた。
//!
//! GPUI は `AnyView::cached(style)` で「その entity が notify されておらず、
//! bounds / content_mask / text_style が前フレームと同じなら prepaint と paint を
//! 丸ごと再利用する」仕組みを持つ（zed が汚れていないエディタ・パネルに使っている）。
//! ここではその単位になる子ビューを 2 種類だけ用意する。
//!
//! - [`PaneBody`]: ペイン 1 枚の本体
//! - [`Chrome`]: タブバー / サイドバー / 右パネル / ステータスバー
//!
//! どちらも状態は持たず、描画の実体は `TakoApp` 側の既存メソッドをそのまま呼ぶ
//! （見た目のコードは 1 本のまま）。汚れ方の規約は以下の 2 つだけ:
//!
//! 1. **PTY 出力**はそのペインの [`PaneBody`] だけを notify する
//!    （`TakoApp::request_term_redraw`）
//! 2. **それ以外のすべての状態変化**は従来どおり `TakoApp` を notify する。
//!    子ビューは `cx.observe(TakoApp)` で自分も汚すので、取りこぼしは構造的に起きない
//!
//! この順序なので「新しく足した状態変化を汚し忘れる」事故が起きない
//! （明示的に PTY 経路へ載せない限り、全部が従来どおり全体を汚す）。
//!
//! ## `cached` は入れ子にできない（#801 の実測）
//!
//! GPUI は `AnyView::cached` が**実際に描き直すあいだ** `window.refreshing = true` を
//! 立てる（gpui `view.rs` の prepaint）。再利用の条件に `!window.refreshing` が
//! 入っているので、**キャッシュビューの中のキャッシュビューは一度も当たらない**。
//! ペインヘッダを [`PaneBody`] の内側でさらにキャッシュしても効かないのはこれが理由
//! （効かせるにはヘッダをペイン枠ごとルート側の兄弟へ持ち上げる必要がある）。
//!
//! また `cached` は「汚れていても」得がある: 中身は `layout_as_root` で確定サイズの
//! 別パスとして解かれるので、ルートの flexbox がその部分木を測り直さない。
//! 実測では、汚れたペイン本体をキャッシュ無しで出しただけで **+0.86M instr/frame**
//! 掛かった（119x21・空画面）。「どうせ描き直すから素で出す」は逆効果になる。

use gpui::{
    div, prelude::*, AnyElement, AnyView, Context, Entity, Render, StyleRefinement, Subscription,
    WeakEntity, Window,
};
use tako_core::PaneId;

use crate::TakoApp;

/// キャッシュを切って同じバイナリで A/B を取るための逃げ道（`TAKO_786_NO_VIEW_CACHE=1`）。
///
/// 効果測定（#786 の受け入れ 1・2）と、描画異常が出たときに
/// 「ビューを分けたせい」と「キャッシュを効かせたせい」を切り分けるために使う。
/// 既定は有効（未設定 = キャッシュする）
fn view_cache_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("TAKO_786_NO_VIEW_CACHE").is_some())
}

/// ビューキャッシュが有効か（セルフテスト 108 の前提確認用）
pub(crate) fn enabled() -> bool {
    !view_cache_disabled()
}

/// 子ビューを `AnyView::cached` で包む（#786）。
///
/// `style` はキャッシュビューの layout にそのまま使われる（GPUI は中身を見ずに
/// スタイルだけで大きさを決める）ので、呼び出し側が大きさを確定させること。
/// `TAKO_786_NO_VIEW_CACHE=1` のときは同じスタイルの箱に入れて毎フレーム描き直す。
///
/// **`view.read(cx)` を毎フレーム必ず通す**のが要点（下記）。
pub(crate) fn cached_view<T: Render>(
    view: &Entity<T>,
    style: StyleRefinement,
    cx: &gpui::App,
) -> AnyElement {
    // #786 の踏み抜きどころ: `cx.notify()` がウィンドウの再描画に化けるのは、その
    // entity が「このウィンドウで**このフレームにアクセスされた**」と記録されている
    // あいだだけ（`App::notify` は `tracked_entities` で絞り込み、外れていると
    // observer を呼ぶだけで dirty を立てない）。記録は draw の最後に
    // `accessed_entities` の中身で作り直される。
    //
    // キャッシュが当たったフレームは GPUI が `element_state.accessed_entities` を
    // 積み直してくれるが、**その集合は初回 prepaint の差分**で作られる。ビューを
    // 親の render の中で `cx.new` すると、その id は差分を取る前に
    // `accessed_entities` へ入ってしまうため差分から漏れ、次のフレームで tracked から
    // 外れて**二度と描き直されなくなる**（実測: プレビューペインが開いた直後の
    // 1 フレームで固まり、目次ジャンプが効かなくなった）。
    // ここで毎フレーム明示的に読んで、記録を自前で確定させる。
    let _ = view.read(cx);
    let view = AnyView::from(view.clone());
    if view_cache_disabled() {
        let mut wrapper = div();
        *wrapper.style() = style;
        return wrapper.child(view).into_any_element();
    }
    view.cached(style).into_any_element()
}

/// ペイン 1 枚の本体を描く子ビュー（#786）。
///
/// 自前の状態は持たず、描画は `TakoApp::render_pane_body` に委譲する。
/// 位置と大きさは呼び出し側（`AnyView::cached` に渡すスタイル）が持つので、
/// このビューは与えられた矩形いっぱいに描く。
pub(crate) struct PaneBody {
    app: WeakEntity<TakoApp>,
    pane: PaneId,
    /// `TakoApp` が notify されたら自分も汚す購読（上記の規約 2）
    _app_dirty: Subscription,
}

impl PaneBody {
    pub(crate) fn new(app: &Entity<TakoApp>, pane: PaneId, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(app, |_, _, cx| cx.notify());
        Self {
            app: app.downgrade(),
            pane,
            _app_dirty: sub,
        }
    }
}

impl Render for PaneBody {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(app) = self.app.upgrade() else {
            return div().into_any_element();
        };
        let pane = self.pane;
        app.update(cx, |app, cx| app.render_pane_body(pane, cx))
    }
}

/// キャッシュ単位にするクローム（ペインの外側の UI）の種類。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum ChromePart {
    /// 上部のタブバー（#380 で全ウィンドウ共有）
    TabBar,
    /// 左サイドバー（ファイルツリー）
    Sidebar,
    /// 右パネル（fleet / orch / git）
    Panel,
    /// 下部ステータスバー
    StatusBar,
}

/// クローム 1 枚を描く子ビュー（#786）。実体は `TakoApp` の既存 render メソッド。
pub(crate) struct Chrome {
    app: WeakEntity<TakoApp>,
    part: ChromePart,
    _app_dirty: Subscription,
}

impl Chrome {
    pub(crate) fn new(app: &Entity<TakoApp>, part: ChromePart, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(app, |_, _, cx| cx.notify());
        Self {
            app: app.downgrade(),
            part,
            _app_dirty: sub,
        }
    }
}

impl Render for Chrome {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(app) = self.app.upgrade() else {
            return div().into_any_element();
        };
        let part = self.part;
        app.update(cx, |app, cx| {
            app.chrome_renders = app.chrome_renders.saturating_add(1);
            match part {
                ChromePart::TabBar => app.render_tab_bar(window, cx).into_any_element(),
                // 非表示のときは呼び出し側が要素を出さないので、ここは保険の空描画
                ChromePart::Sidebar => app
                    .render_sidebar(cx)
                    .map(IntoElement::into_any_element)
                    .unwrap_or_else(|| div().into_any_element()),
                ChromePart::Panel => app
                    .render_panel(cx)
                    .map(IntoElement::into_any_element)
                    .unwrap_or_else(|| div().into_any_element()),
                ChromePart::StatusBar => app.render_status_bar(cx).into_any_element(),
            }
        })
    }
}
