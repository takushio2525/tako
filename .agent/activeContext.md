# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-18、#838 = Web ビューペインのちらつき）

- ブランチ `fix/838-webview-flicker`（worktree `~/dev/tako-wt-838`）
- ネイティブ Web ビュー（wry / WKWebView）が毎秒何度も消えては出るのを根治

## 根因（推測ではなく実測で確定）

可視性が**印（mark）方式**だった: ペイン本体の render が「自分は描かれた」と印を付け、
ルートの掃き出しが印の無い webview を隠す。これが #786（ペイン本体の
`AnyView::cached` 子ビュー化）で壊れていた。

- 子の render は**キャッシュが当たったフレームでは走らない** → 印が付かない → 隠される →
  次に `TakoApp` が notify されると再表示、の往復。#816 で PTY 出力が**そのペインだけ**を
  notify するようになったので、`TakoApp` を notify しないフレームが日常的に起きる
- 子の render はルートの掃き出しの**後**に走るので、`hide_all`（D&D / パレット /
  close 確認との重なり回避）も子に上書きされて効いていなかった

実測: 隔離セルフテスト項目 71 に足した検査で、notify 無しのフレームを 12 枚重ねると
旧経路は `visible=true → false`（切替 3 → 4）。新経路は不変。

## 直し方

フレーム同期をルート render（`sync_webview_frames`）へ移し、**どのウィンドウの render から
呼ばれても同じ答えになる材料だけ**（全ウィンドウ共有の `pane_text_areas` = 今どこかの
表示タブに載っているペインだけが残る。#339）から毎フレーム決め切る。印は撤去。
A/B は `TAKO_838_NO_ROOT_WEBVIEW_SYNC=1`（旧経路へ戻る）。

## 次の一手

- 品質ゲート + 隔離セルフテスト + visual-test 全節 + grid-bench 回帰なしの確認
- PR（`Closes #838`）→ macOS CI 緑 → squash merge → install（単独実行）

## 現フェーズで Read すべき設計書

- `.agent/architecture.md`「Web ビューペイン」節 =「フレーム同期を印でやってはいけない」
  規約と実測値の正
- `crates/tako-app/src/webview.rs` 冒頭 doc = wry 統合の全体像
