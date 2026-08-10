# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-10、Issue #793 setup への設定共有の導線 = 実装完了・PR 待ち）

- worktree `~/dev/tako-wt-793` / ブランチ `feat/793-setup-config-share`
- #513 の `tako config` は実装済みだったのに setup からの導線が無く、**この開発機でも
  未配線のまま放置されていた**（= 導線が無いと使われない実例）。それを是正した
- 新設 `tako-control::config_share::env`: ①配線済みか（`config-share.json`）
  ②共有対象が既に外部 git（dotfiles 等）で管理されていないか ③gh の認証状態、を
  **読み取りだけ**で検出する。案内の種類は純粋関数 `guidance`
  （`linked` / `broken` / `adopt_existing` / `fresh`）で決める
- 表示は setup サマリと `tako setup --check` が同じ判定から文言を作る（`config_share_lines`）。
  **質問は増やさない**（#262）。配線済みなら勧誘しない（冪等）
- 代行は setup 対話アシスタント側。検出結果を `setup-context.yaml` の `config_share`
  （guidance / next_command / gh_can_create_repo / external[]）で渡し、
  system-prompt.md の Step 3.5 に代行手順を書いた。既存ユーザーへは changes.yaml rev 14（guided）

## 設計判断（なぜ「相乗り」が第一案か）

- `~/.claude` を dotfiles の symlink にしている利用者に**別**の共有リポジトリを配線すると、
  同じ CLAUDE.md が 2 か所で管理され、`tako config pull` の書き込み
  （`config_io::atomic_write` の rename）が symlink を実ファイルへ置き換えて既存の配線を壊す
- 逆に既存リポジトリへ相乗りすれば、tako の書き出し先（`claude/…`）が既存の置き場と
  一致する限り**同じファイル**を指すので重複が生まれない。一致するかは
  `ExternalManaged::same_place`（`repo_rel == root`）で判定して表示・context に載せる

## 検証状況（隔離 e2e = PASS 55 / FAIL 0）

- 隔離 HOME + スタブ claude / gh + ローカル bare リポジトリ。**本番の HOME・`~/.claude`・
  dotfiles・GitHub には一切触れていない**（非干渉チェックも e2e に含む）
- 未配線 → 案内 / 配線済み → 状態のみ（3 回連続で同一）/ dotfiles 検出 → 相乗り提案 +
  二重管理の注意 / `--yes`・非 TTY → 副作用も代行案内も無し / pty 経由の対話端末 → 質問ゼロ
  のまま代行導線が出る / `gh repo create`（スタブ）→ `tako config init --remote` の連結
- fmt / clippy(-D warnings) / test --workspace 全緑（1921 件）+ docs build 成功 +
  Windows クロス check（`scripts/check-windows.sh`）エラー 0 / 警告 13 = baseline 不変
- **隔離セルフテストは完走せず**。同一手順 4 回で毎回別項目が落ち、**素の main（`b6c9e38`）
  でも落ちた**（本 PR: #601 / PDF #232 / #601、main: #666）。本番 tako.app が ~99% CPU で
  load 6〜16 の環境要因。#496 側も同日「#601 の固定待ちをリトライ化（main 由来の確定失敗）」
  「PDF / IME / tmux は main 由来失敗」と記録している
- 証拠: `/private/tmp/tako-793-e2e/evidence`、スクリプトは scratchpad の `e2e-793.sh`

## 不変条件

- 検出は**読み取りだけ**。`--yes` / 非 TTY で外部への副作用（リポジトリ作成・push）を作らない
- 標準 setup に質問を足さない（`decide_config_share_step` は Info のまま）
- 勝手にリポジトリを作らない・push しない（合意は対話アシスタント側で取る）
- 隔離検証では `CLAUDE_CONFIG_DIR` を必ず外す（外さないと `catalog::claude_home()` が
  本番のアカウント設定ディレクトリを指し、隔離が崩れる）

## 次の手順

1. PR（`Closes #793`）→ macOS CI 全ジョブ緑 → squash merge → `build-app.sh --install`
2. 実機での確認は本番 setup を走らせる形になるので、ユーザー判断（この機は
   `~/.claude` = `~/dotfiles/claude` の symlink なので `adopt_existing` が出るはず）

## 現フェーズで Read すべき設計書

- 設定共有まわり: `.agent/requirements.md` FR-5.14（.9〜.11 が #793）
- setup の流れ: `resources/setup/system-prompt.md`（Step 3.5）と `crates/tako-cli/src/setup.rs`
