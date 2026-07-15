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

追加要件 D/E により C の確認はさらに自動化側へ寄せた。標準実行は変更計画を
確認入力には使わず、適用後の最終サマリとしてだけ表示する。個別対話は
`--review` へ分離した。

## 修正後の実測

`scripts/verify-setup-multiagent.sh` を、標準入力 `/dev/null` とスクラッチ HOME /
PATH / `TAKO_ISOLATED=1` で実行した。入力回数は物理キー数ではなく、人間が
回答を送信する操作の回数で数えた。

| シナリオ | before 入力回数 | after 入力回数 | after 質問数 | 結果 |
|---|---:|---:|---:|---|
| 初回・認証済み Claude Pro 単独 | 5 回以上 | 0 | 0 | `detected: claude/pro` で完走 |
| 同じ HOME の 2 回目 | 5 回以上 | 0 | 0 | `previous` を引き継ぎ、`config.yaml` は byte-for-byte 不変 |
| `tako setup --yes` | オプション未実装 | 0 | 0 | stdin を読まず完走 |
| Claude 未認証 | 1 | 0 | 0 | 設定を書かず、ログイン手順つきエラー |

実ユーザーの Claude 認証だけを読み取り専用で実 CLI へ委譲し、書き込み先と
setup agent 起動をスクラッチへ隔離した追加実測でも、Claude Max を
`[detected]` で採用して入力 0・質問 0 で完走した。実ユーザーの tako /
Claude 設定は変更していない。

## エッジケースと機械駆動

- 認証済みだがプラン取得不能: `[default] unknown`、入力 0 で完走
- 前回 `pro` / 再検出 `free`: 両値を通知し `detected free` を優先
- 破損 `config.yaml`: 検出・書き込み前に停止し、破損内容の checksum 不変
- `--answers -`: agent / plans / instructions / profile / projects / orchestrator /
  sleep guard を stdin JSON で全適用
- MCP `tako_setup`: 全回答を dispatch `SetupRun` へ変換するテストと、dispatch が
  JSON を argv ではなく CLI stdin へ渡す子プロセス実行テストを通過
- 複数 CLI: Claude / Codex JWT Plus / agy を検出し、質問 0 で delegate profile を生成

## 品質ゲート

- `cargo build --workspace`
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`: app 91 passed / 2 ignored、CLI 25、control 430、core 276
- docs `npm run build`: 19 ページ生成
- `scripts/verify-setup-multiagent.sh`: 上記シナリオ、エッジケース、実 Claude 認証が全緑
