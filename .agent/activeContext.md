# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-14、Issue #789 サイドバー幅のクランプ統一 = PR 提出）

> 直前に #792（handoff の知識 / 実行状態の分離）が main へ入り install 済み（`40c4b2a`）。
> その書式と不変条件は `.agent/orchestrator.md` と FR-2.24.8 が正本。
> GUI 再起動が要るのは #792 と本件の両方（バイナリを差し替えたら 1 回で足りる）

- #307（サイドバーのドラッグリサイズ）の上限だけが経路で食い違っていた:
  ドラッグ = ウィンドウ幅の 50% / dispatch（CLI・MCP）= 固定 600px。下限 120px は一致していた
- 規則を `tako_core::sidebar`（`MIN_WIDTH` = 120 / `MAX_RATIO` = 0.5 / `clamp_width` /
  `max_width`）へ一本化。**ドラッグ側へ寄せた**理由は ①固定 px では広い窓で CLI から
  ドラッグ相当の幅に届かない（設計原則 5 の破れ）②固定 px は狭い窓で過大（600px は
  800px 窓の 75%）。仕様は `.agent/requirements.md` FR-3.23 に起こした

## 入った実装

- `tako-core/src/sidebar.rs` 新設（GPUI 非依存の純関数 + unit 7 本）。tako-app の
  `SIDEBAR_MIN_WIDTH` 定数は廃止（規則の二重定義を構造的に防ぐ）
- 状態は**要求値**（`TakoApp::sidebar_width`）、描画・座標計算は**実効幅**
  （`effective_sidebar_width()` = 要求値をビューポート幅でクランプ）に分離。
  render の会計（`estimated` / `pane_text_areas`）・ルートが渡す `.w()`・
  `sidebar.rs` の内側の `.w()` はすべて同じ実効幅を通る（#684 / #781 と同じ理由）
- dispatch 経路はウィンドウを持たないので、上限は `last_viewport_width`
  （render が毎フレーム控える「最後に描いた窓の幅」）から取る。応答に
  `sidebar_width_max` / `sidebar_width_min` を追加し、永続化は要求値 → **適用値**へ
- セルフテスト項目 109 = 実ハンドラ（`on_mouse_move`）と実 dispatch（`Request::Panel`）へ
  同じ数値を入れて一致を見る。窓を 1600 に固定するので旧固定 600px は必ず落ちる

## 不変条件

- クランプ規則の正は `tako_core::sidebar` だけ。tako-app / tako-control に px の直値を置かない
- **窓が狭くなっても要求値は書き換えない**（狭い窓では収まる幅で描き、広げ直す・
  再起動すると元の幅へ戻る）。窓の縮小で settings.json を上書きしないこと
- ビューポート幅が不明な文脈（起動直後・GUI 非依存）では上限を課さない。
  上限は必ず幅が分かる場所（描画時）で掛かる

## 環境メモ（他 worker も踏む）

- **Metal Toolchain が消えていた**（macOS の purgeable 資産。`xcrun metal` が
  `missing Metal Toolchain`）ため、新しい worktree では gpui のシェーダをビルドできない。
  復旧は `xcodebuild -downloadComponent MetalToolchain`（704MB・管理者権限不要・実施済み）
- 新しい worktree は `web/tako-remote/dist/` が無く rust_embed で tako-control が
  コンパイルできない。`cp -a` で既存 worktree から持ってくるか `npm run build`

## 次の一手（master 判断）

- PR（`Closes #789`）→ CI 緑 → squash merge → install（他 worker と重ねない）
- 上限を「ウィンドウ幅の 50%」にしたので、サイドバー 50% + 右パネル 70% を同時に
  指定するとペイン領域が負になり得る（ドラッグでも従来から可能）。気になるなら別 Issue

## 現フェーズで Read すべき設計書

- 幅・クランプ: `.agent/requirements.md` FR-3.23 / `crates/tako-core/src/sidebar.rs`
- 描画の会計: `.agent/architecture.md`「ビュー単位の描画キャッシュ」/
  `crates/tako-app/src/main.rs` の `effective_sidebar_width` と `pane_text_area_rect`
