# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-05、Issue #761 handoff 後任 master の起動パラメータ）

- 2026-08-05 の初実地ハンドオフで後任 master が worker 用モデルで起動し、以後
  default プロファイル扱いになった不具合の修正（worktree `../tako-wt-761`）
- 直した 3 点（いずれも `dispatch_orchestrator_handoff`）:
  1. 起動コマンドを `build_worker_cmd` + `resolve_agent_launch`（worker 用）から
     `build_master_cmd`（CLI `tako master -<profile>` と同一経路）へ
     → profile の master model / effort が効き、master system prompt も付く
  2. `TAKO_ORCHESTRATOR_ROLE` に表示用 role（`orchestrator-master:<p>`）を入れていたのを
     env 用（`master:<p>`）へ。生成は `tako_core::handoff` の 2 関数に閉じた
  3. caller_role にペインの role ラベルが来る内部呼び出し（stale binary restart）でも
     プロファイルを解決できるよう `master_profile_of_any_role` を新設
- 起動コマンドの組み立てをペイン分割の**前**に移し、失敗時に空ペインを残さない

## 検証状況

- 実経路 e2e（隔離アプリ + プロファイル env の PATH で偽 claude を最優先）で before / after:
  - before = `--model …worker[1m] --effort high` / role env が `orchestrator-master:st761` /
    system prompt 無し / 後任の `orchestrator self` が `profile=default handoff=default.md`
  - after = `--model …master --effort xhigh` / role env `master:st761` /
    prompt マーカー検出 / `profile=st761 handoff=st761.md`
- 単体 3 本（dispatch）+ 2 本（tako-core）追加。各バグを戻すと FAILED になることを実測
- セルフテスト項目 102 を新設（起動コマンド + role env + 後任 self の通し検証）
- `cargo test --workspace` / `fmt --check` / clippy（全 target・deny warnings）全緑
- 隔離セルフテストは `TAKO_APP_SELF_TEST_OK`・exit 0（SKIPPED は 76d のみ = 環境要因）

## 不変条件

- 本番 GUI・本番 tmux・本番 data dir に触れない（隔離は TAKO_ISOLATED + 明示 data/discovery、
  CLI 側は呼び出し元の `TAKO_PANE_ID` / `TAKO_ORCHESTRATOR_ROLE` を必ず unset する）
- master の role は「表示用」と「env 用」で語彙が違う。生成は tako-core の関数経由に限る
- master_account の反映規則（#547）は変更しない

## 未着手・持ち越し

- #691 GUI モードのクローズはユーザーの実使用確認待ち
- #658、#601 案 2、#632、#633、#638、#651 ほか既存キューは #761 の対象外
