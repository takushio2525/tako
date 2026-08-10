# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-10、Issue #496 残バグ = git パネルのクリックが一括 dismiss に食われる）

- ブランチ `fix/496-agent-menu-click`（worktree `~/dev/tako-wt-496`）。
  コンフリクトカードの「解消エージェントを起動」3 択が **merge 時から GUI で一度も
  動いていなかった**のを根治した（CLI / MCP の同じ dispatch は動くので見つからなかった）
- 根因: ルート div の `on_mouse_down` が `clear_text_input_focus()`（#503）を呼び
  `git_agent_menu_open` を落とす。GPUI の配送は mouse_down → mouse_up → click なので、
  **押下の瞬間に 3 択が消えて `on_click` が発火しない**
- 同型を 4 件見つけて一緒に直した（点検結果）:
  ①3 択 ②トグル（開いた状態から閉じられなかった）③ブランチ名入力欄の本体
  （クリックすると欄ごと消える）④作成ボタン（GUI から新規ブランチが作れない）
  ⑤キャンセルボタン。**コミット欄 / Web dock URL / Web アドレスバーは元から
  `stop_propagation` していて健全**（点検済み）

## 実測証拠

- visual-test 新設節 `conflict-card`（`TAKO_VISUAL_ONLY=conflict-card` で単独実行可）:
  使い捨てリポの実コンフリクトでカードを出し、**実 OS マウスと同じ `PlatformInput`** で押す
  → `claude` panes 1→2 / `codex` 2→3 / `agy` 3→4 でペインが立つ（feedback も成功文言）
- 検出力: 3 択の `stop_propagation` を外すと `panes 1->1 / feedback=None` で FAILED
  （= Issue 報告の症状「メニューは閉じるがペインは立たず無言」と一致）
- 番犬テスト 3 本（`right_panel::dismiss_guard_watchdog`）は **CI で毎回走る**。
  guard を外すと該当 id を名指しで FAILED になることを実測

## 詰まりどころ（次に同種の検証をする人向け）

- **合成マウスは通常セルフテストでは効かない**。`gpui_platform/test-support`（= `--features
  visual-test`）が要る。通常ビルドでは `dispatch_event` の hit test が当たらない
- `click_at` は MouseMove の**後に 1 フレーム描く**必要がある（`hitbox.is_hovered` は
  フレーム構築時の hit test 結果を見る）。この 1 行を入れるまで一切当たらなかった
- 矩形プローブの canvas は `.absolute()` だけだと **CSS 同様「本来置かれる位置」= 直前の子の
  下**へ落ちて 18px ずれる。`.top_0().left_0()` を必ず付ける

## 検証状況

- fmt / clippy(-D warnings、通常 + `--features visual-test` の両方) / test --workspace
  1906 passed 0 failed
- **visual-test 全節完走**（`TAKO_VISUAL_TEST_OK`。新設 conflict-card を含む）
- 隔離セルフテストは**完走できていない**。落ちるのは全部 main 由来 / 環境要因:
  ①#601 の PATH 注入（固定待ち → 本 PR でリトライ化して解消）②PDF アウトライン
  （`pdf_jump_delta_px=290.0`）③IME 確定（`IMKCFRunLoopWakeUpReliable` の OS エラー）
  ④tmux open の attach。②〜④は私の変更と因果なし（別 Issue 化を提案する）
- 本番 GUI（pid 17056）は全計測の前後で生存。本番 tmux socket `tako` には触っていない

## 事故と対策（2026-08-10）

- 隔離検証のつもりで叩いた CLI が **worker ペインの `TAKO_SOCKET` を継承して本番 GUI に
  接続**し、本番 master ペインへ `cd ...` を発話として送り、本番パネルを git ビューに切り替えた。
  `TAKO_DATA_DIR` / `TAKO_DISCOVERY_DIR` を渡すだけでは足りない
- 以後の隔離 CLI は **`env -u TAKO_SOCKET -u TAKO_TOKEN -u TAKO_PANE_ID -u TAKO_TAB_ID
  -u TAKO_MCP_URL` を必ず付ける**（ラッパを使い、接続直後にペイン数で隔離先を確認する）
- 座標ベースの GUI クリック（cliclick）は**同名プロセスが 3 つある環境で本番ウィンドウを
  掴む**（AppleScript の position も誤解決した）。合成 `PlatformInput` を使う

## 不変条件

- `clear_text_input_focus()` が落とす状態に依存して描かれるクリック要素は、必ず
  `on_mouse_down` で `cx.stop_propagation()` する（規約は `.agent/conventions.md`）
- #503 の意図は不変: `clear_text_input_focus()` 本体と `handle_key` の防御的クリアは
  **一切変更していない**（キー入力が奪われたまま残らない性質は維持）
- 検証は `TAKO_ISOLATED=1` + 専用 tmux socket + 専用 data / discovery dir。
  System Events のキーストローク送出は禁止

## 次の手順

1. push → PR（`Refs #496`。クローズ判定は master）→ macOS CI 全ジョブ緑 → squash merge
2. `scripts/build-app.sh --install`（他 worker のビルドと同時に走らせない）
3. PDF / IME / tmux のセルフテスト項目の main 由来失敗を別 Issue で起票する

## 現フェーズで Read すべき設計書

- クリック要素を足すとき: `.agent/conventions.md`「一括 dismiss に食われないクリック要素の作り方」
- git パネルのレイアウト方針: `.agent/requirements.md` の FR-3.6 / #494 節
