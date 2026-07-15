# Issue #262: setup UX の隔離実測と根本原因

## 実測条件

- 起点: v0.5.3（`6a4e06e`）
- 実ユーザーの tako 設定・指示ファイル・Codex auth は読み取り専用で参照し、
  書き込み対象だけをスクラッチ HOME へコピー
- 認証照会は実 CLI へ委譲し、setup agent 本体の起動は shim で終了
- tako の書き込みはスクラッチ HOME と `TAKO_ISOLATED=1` 配下だけ
- 実パス、token、メールアドレス、account ID は記録しない

## 検出結果

| 対象 | 結果 |
|---|---|
| claude | 認証済み、`subscriptionType=max` |
| codex | 認証済み、JWT plan claim は `free` |
| agy | 認証済み、プラン取得不能 |
| 外部依存 | tmux / cloudflared / git を検出 |
| FDA | 付与済み |
| スリープ防止 | 設定済み |

同じ隔離 HOME で現行 `tako setup` を 2 回連続実行した。どちらも CLI 側で
次の 5 問が出た。

1. スリープ防止レベル
2. setup を進めるエージェント
3. Claude Max の 5x / 20x 差分
4. Google プラン
5. 既存 default profile を推奨値で更新するか

GPT の `free` は `[自動検出]` と表示され、質問なしで採用された。2 回目は
1 回目に `selected_agent` と `provider_plans` が保存済みでも、質問数と内容が
変わらなかった。実測では安全のため agent 本体を起動していないが、現行実装は
この後も常に agent を起動し、初回質問または 2 回目メニューへ進む。

## 根本原因

1. `run_setup()` が `load_config()` より先に依存チェック、エージェント選択、
   プラン収集を実行し、前回値を使えない順序になっている
2. `select_setup_agent()` は複数 CLI なら必ず質問し、前回の
   `setup.selected_agent` を参照しない
3. `collect_provider_plans()` は前回値を受け取らず、インストール状況と無関係に
   Claude / GPT / Google を全巡回する
4. Claude `max` と agy のプラン取得不能は正しくフォールバック質問へ進むが、
   前回回答があっても毎回聞き直す
5. `run_sleep_guard_check(true)` は設定済みでも毎回質問する
6. `prepare_profile()` は実差分の有無に関係なく、既存 profile なら毎回確認する
7. CLI の後に setup agent の別対話があり、項目別確認をさらに重ねる
8. 変更計画を表す構造がなく、最終 diff 1 回確認と `--yes` が実現できない

## 修正方針

- A: 検出済み CLI だけを対象に、検出値を source 表示付きで即採用する。検出不能で
  前回値もない項目だけ質問する
- B: config / profile / 設定済み状態をフロー冒頭で読み、前回値を引き継ぐ。
  検出値と前回値が違う場合は検出を優先して通知する
- C: 適用内容を変更計画へ集約し、最後の 1 回だけ確認する。`--yes` は同じ計画を
  非対話で適用する
