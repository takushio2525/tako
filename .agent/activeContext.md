# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md`、フェーズ計画は `roadmap.md` を見ること。
> 予算は **80 行以内**（`tako context-budget`）。過去ターンの実装詳細はここに書かない。

## 現在の対象（2026-09-08）

- main = `4d84695`。9/7〜9/8 に #1143（狭いペインの `/model` セレクタ）/ #1136（夜間リリースの
  使い捨て worktree 化）/ #1154（system prompt を予算対象へ）/ #1160 / #1162 / #1153 / #1165
  （検証環境とセルフテストの待ち）/ #1081 の v3 動画が着地。1 件ずつは progress.md
- **/Applications は 9/7 19:52 ビルド・GUI は 9/7 19:54 起動**（版数 v0.8.7）。それ以降の main
  = #1162 / #1153 / #1165 = **セルフテストの待ち条件のみ**なので、install + GUI 再起動は
  次に製品挙動を変える PR とまとめてよい
- **ユーザー目視待ち**: ツリー git 色（#1009）/ Finder D&D の実マウス（#1043）/ Dock ピン留めは
  次回の更新で確認（#1042）/ #1059 のスマホ 4 項目 / #1081 v3 の試聴（10:01・音声は数値のみ検査）
- **本番 remote は稼働中**（standalone tailscaled・`serve_ok=true`）。GUI 版 Tailscale と
  2 系統同時なので `warnings` が 1 行出るのは既知（#1038）
- **検証用 GUI は常設の仮想ディスプレイ `tako-vd` へ出す**（#1141 / #1150 / #1160。ユーザーの
  メイン画面に窓を出さない）。起動前に `scripts/lib/virtual-display.sh ensure`、
  レシピは `.agent/conventions.md`（`-u TERM -u COLORTERM` は不要になった = #946）
- **検収の status 読みは `/Applications/tako.app/Contents/MacOS/tako` で叩く**（PATH 先頭の
  `~/dev/tako/target` が stale だと新フィールドがキーごと無い = #432 と同じ罠）
- 作業は**専用 worktree**・main 直 push はしない（docs も PR 経由）

## 直近の観点

- **セルフテストの待ちを「状態待ち + `state_wait_budget`」へ寄せ切る途中**。#1162（実寸が届く前に
  fixture を描いていた）→ #1153（4 系統）→ #1165（画面エコー 9 か所）で移送済み。番犬 3 本
  （固定窓で `dispatch` / 増えた数 / 画面の文字列を待つ形）が再発を落とす。
  高負荷で落ちる open = #1167 / #995 / #771 / #1122 / #1114。**#1124 は #1153 で系統を
  根治済み・close 判断だけが残っている**
- **起動時ロードの予算（#1139）は `tako context-budget` が正**で CI の番犬が落とす。残る超過 4 件は
  リポ内が `progress.md` の 1 エントリ長（28 行 > 3。**書くときに守る** = 自動修正の対象外）、
  あとはリポ外の個人環境ファイル 3 つ（master system prompt の `append` = `local-rules.md`
  10.9 KB → **分離はユーザー相談が必要** / `handoff/default.md` 374 行 / `~/.claude/CLAUDE.md`）
- **Windows 実機**: 隔離 GUI セルフテストは #1127 で**初完走**（ただし `editbin /STACK` の迂回つき）。
  素のビルドで完走するための残ブロッカーは **#1133 のみ**（項目 80 でスタックオーバーフロー）。
  実機で残る open = #1137 / #1114（間欠失敗）/ #971（#1038 で実装済み・実機実測待ち）。
  Windows 全体は #467 配下。実測・A/B・引き継ぎ表は plan（下記）
- **リモート刷新（エピック #1059）は分割 A〜H が全部 main に着地**。残りはユーザーの実機スマホ
  確認だけ（①「Claude で開く」→ アプリ ②「+ master」でタブ + 起動 ③ファイルの閲覧・編集・保存
  ④SSH の切り替え / 新規接続）。Windows のファイル API（daemon → app の IPC が unix 実装のみ）は
  Windows 系の残バグとして別扱い

## 次の一手

- **#1167**（tako-control の「追記ぶんだけ読む」が高負荷で落ちる）着手中
- **#757 は完了**（ログイン失効を `api_error` から分離。判定順序は「ダイアログ > 失効 > 上限」）
- **#1273 は完了**（背景シェルの入力待ちを idle と判定。**install 済み**）。横展開は #1277
- 残るフレーク #995 / #771 / #1122 を #1153 と同じ形（状態待ち + 番犬）へ寄せる。
  **#1229 / #1252 / #1265 / #962 / #1274 は完了**（真因はいずれも「待ち不足」ではなく
  偽アンカー・入力の偶然・言語グローバルの二重読み。詳細は progress.md と各 Issue）
- **#633 は完了**（承認カードの `command` を本体の開始マーカー起点へ）。#757 と併せ **install 要**
- **#1370 は完了**（IPC のレイアウト変更後に 1 フレーム強制描画。隠れた窓・最小化・仮想面では
  notify だけでは cols/rows が古いまま。判定 = `protocol::changes_layout`）。**install 要**
- **#1400 は完了**（`~/.ssh/config` の `Match` の設定が直前の Host へ混入し、接続 argv が
  `ssh -p 2222 root@prod` へ化けていた。状態を `Section` の 2 値へ + `Include` を解決。
  #1004 の `guides/remote.md`「Include は読まない」も同一 PR で実態へ寄せた）。**install 要**
- **#1399 は完了**（ツリーのローカル操作 13 か所の失敗を共有の通知欄 + persist.log へ・リネーム
  失敗で名前を残す。1 実装 = `notify_tree_failure` / A/B = `TAKO_1399_LEGACY=1`）。**install 要**
- #1059 / #1081 はユーザー確認が済んだら close する
- #975 残: #987〜#991（#986 / #992 は着地）。open バグ #1013 / #1015 / #1022 / #1030 /
  #1033 / #1034 / #1035
- #1001 残: C6 / C7（`main.rs` 直列群・1 本ずつ）・C5 / C8〜C10（調査系）。**C2 / C3 は #1301 で完了**。
  C4 は #1012（PR #1020）で着地済みで、残るのは `starter.rs:368` の 1 件のみ（上限つきなので任意）
- #1007 IDE 化: S0 の一部（#1016）済み → S1（LSP 基盤）

## 現フェーズで Read すべき設計書

- セルフテストの待ち条件・番犬を触る: `.agent/conventions.md`「セルフテストの待ち条件の書き方」節
- Windows 実機: `.agent/plans/2026-08-windows-main-merge-wip.md`（ベースラインは 2026-09-02 に
  取り直した表・#1063 の DPI・#1091 / #935 / #936 / #1102 / #1127 の記録・`editbin` の迂回）
- SSH / リモート: `.agent/plans/2026-08-remote-folder.md`（#1040 / #1041 と A/B の env）+
  `.agent/plans/tako-remote-plan.md`（§10 = `remote serve` の不変条件）+
  `research/2026-09-01-remote-renewal-claude-official.md`（#1059）
- 動画・仮想ディスプレイ収録: `.agent/plans/2026-09-youtube-explainer.md`
- #1007 着手時 = `<data_dir>/orchestrator/research/2026-08-28-ide-editor-report.md` /
  #1001 着手時 = 同 `2026-08-28-perf-profile-report.md`（どちらもリポ外）
- 対応マトリクスの判定を触るなら `crates/tako-core/src/platform/support.rs`
