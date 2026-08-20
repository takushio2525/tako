# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-21、#822 = リミット自動復帰のプロファイル既定）

- ブランチ `feat/822-profile-limit-resume`（worktree `~/dev/tako-wt-822`）
- #813 のペイン単位オプトインを、master / solo プロファイルの既定値として
  spawn した worker ペインへ自動適用できるようにする（FR-2.27.11 を新設）

## 設計（実装済み）

解決順は **spawn 引数 → プロファイル → false**。正は
`tako_control::orchestrator::resolve_worker_limit_resume`（純関数）1 本で、
`dispatch_orchestrator_spawn` は解決結果を `set_limit_autoresume` でペイン属性へ
入れるだけ（判断ロジックを spawn 側に持たせない）。

- spawn 引数の `Some(false)` は「未指定」ではなく**明示 OFF**（プロファイル ON を
  その worker だけ打ち消す）。`or` ではなく `Option` の有無で判定している
- 見えるところ: spawn 応答の `limit_resume` / `orchestrator workers` の各行
  （ペインが居なければ `null` = 番号再利用の別ペインを誤って有効と報告しない）/
  `worker_status` / `read` / `list`（#813 のまま）/ ヘッダインジケータ（同じペイン属性）
- 3 経路 1:1: `profiles set --limit-resume` / MCP `tako_orchestrator_profiles` /
  GUI 設定画面 → プロファイル（すべて同じ dispatch）。MCP ツール数は不変
- **solo は worker を spawn しない**ので ON にしても効かない → `profile_to_json` が
  警告を返す（黙って死んだ設定にしない）
- `orchestrator run` / task checkpoint resume は spawn と同じ経路なのでプロファイル
  既定がそのまま効く。個別の `--オプション` は増やさない（#322）

## 次の一手

- 隔離セルフテスト（項目 117）の完走確認 → PR（Closes #822 / Refs #813）→ macOS CI 緑 →
  squash merge → install（他 worker と重ねない）

## 現フェーズで Read すべき設計書

- `.agent/requirements.md` FR-2.27（#813 の発動条件と #822 のプロファイル既定）
- `.agent/orchestrator.md`「リミット後の自動復帰」節（利用者向けの使い方）
