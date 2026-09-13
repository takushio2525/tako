# Progress Log

> AI が作業完了時に**末尾へ追記**する時系列ログ。新しいものほど下。
> **1 エントリは 1〜3 行**（何を / どこを / 結果）。詳細は git log・Issue・PR・`.agent/plans/` に委ねる。

このファイルは `AGENTS.md` から `@import` されるので**毎ターン全文が読み込まれる**。
予算（直近 5 作業日 / 20 エントリ / 12 KB）を超えたぶんは `progress-archive.md` へ
1 行で移る。移送は `tako context-budget fix` が行う（冪等・本文は改変しない・
全文は git 履歴に残る）。規約の全文は `AGENTS.md`「起動時ロードの予算」節。

## 追記フォーマット

```markdown

## YYYY-MM-DD（#Issue 一言）
- {何を / どこを / 結果}
- 関連コミット: `{shortsha}` `[種別] 概要`
- 次: {次にやることがあれば 1 行}
```

---

## 2026-09-12（#1422: 右パネルの tmux 復元と UI の eprintln! 5 か所を通知欄 / 診断へ寄せた）
- #1417 の番犬は窓の中に `Err(` が**在るだけ**で「扱った」と数えるので、`right_panel.rs:2022` は結果を `if opened.is_ok()` でしか見ていないのに**無関係な** `if let Err(e) = attach_pending_sessions(..)` で緑だった（実測で確認）。判定を**結果の束縛名の追跡**へ寄せ、`eprintln!` の検査を `sidebar.rs` 限定から全 UI モジュール（テストモジュールは除外・スコープ外は `KNOWN_EPRINTLN` の件数で段階導入）へ広げた
- 6 か所を振り分け: ユーザー操作 5 件（tmux 復元 / 復元ペインの PTY 起動 / バックグラウンド復帰 / コードのコピー / Code Runner）は共有の通知欄 + persist.log、背景処理 1 件（PDF 再ラスタライズ）は `log_ui_failure` で診断だけ。A/B の軸を画面（`NoticeArea`）から Issue（`NoticeArm`）へ分離（同じ画面に #1417 と #1422 の通知が同居し、画面で env を選ぶと互いの回帰を隠すため）
- 隔離 GUI（tako-vd）項目 84d の A/B: 新 = `restore=Some("セッションの復元 に失敗しました…")` / `unshelve=Some(…)` / `copy=Some(…)` / `bg_silent=true bg_lines=3` / `ok_silent=true` で完走（FAILED 0）、legacy（`TAKO_1422_LEGACY=1`）は全部 `None` で FAILED かつ #1417 の診断行は `legacy=false` のまま（A/B が互いを隠さない）。注入 10 通りが file:line 名指し FAILED・workspace 4538 passed 0 failed

## 2026-09-12（#1430: merge-pr.sh の終了コードを実測で言い切り、ローカル head も自分で消した）
- 実測で Issue の推測を否定: 現行スクリプトは worktree 事故でも**既に exit 0**（#1347 で解消済み）。実在した誤読の元は「`failed to run git: fatal:` + `警告: merge は済んだが gh が 1 で終わった`」の 2 行と、**すでに MERGED の PR への再実行が 1** を返すこと（使い捨て private リポ + 実 gh 2.88.1 で PR 7 本を実 merge して確定）
- gh の非ゼロを「merge の失敗ではない」注記へ、最終行を `merge 成立: PR #N は MERGED（url）/ 終了コード 0` へ、MERGED の再実行を冪等 0 へ（CLOSED は 1）。ローカル head は `git branch -D`、作業ツリーが握るときだけ外し方を名指しして残す（`worktree_holding_branch` / `delete_local_head_branch` の 1 実装）
- モック Test 24〜29（ローカルブランチと作業ツリーだけ実 git。25b = 本体の作業ツリーには「畳め」と言わない）で 137 PASS 0 FAIL。注入 4 通り（後始末を外す / 再実行を refuse へ戻す / 旧警告文へ戻す / 作業ツリー検出を殺す）すべて FAILED。A/B = `TAKO_1430_LEGACY=1`。**install 不要**（scripts + docs のみ）

## 2026-09-12（#1404: ツリーのスキャン対象の重複と、消えたルート配下の読み続けを直した）
- 組み立てを「`roots` の順 → `expanded` の未出（名前順）」の 1 実装へ（`Vec::dedup` は隣接しか落とさず `expanded` は `HashSet` = 順序が任意なので、全ルートが 2 回ずつ `read_dir` されていた）。外れたルートは `forget_under` で**配下ごと**忘れる（生きているルートの下は巻き込まない = 入れ子のルート）。事実と違う注釈も実態へ
- 隔離 GUI（tako-vd）の項目 135 を「ユニーク化 + 展開ディレクトリの存在」へ書き換え: 新 = 完走 / `TAKO_1404_LEGACY=1` = `targets=5 uniq=3` で FAILED / 展開 0 件の注入 = `targets=2 uniq=2 extra=[]` で FAILED（旧 assert `targets.len() > git_roots.len()` は重複だけで常に真 = 検出力ゼロだった）
- 単体 5 本（legacy で 4 本 FAILED・`rows` 不変の 1 本は両腕で緑）+ 番犬 3 本（注入 6 通りで file:line 名指し）。workspace 4579 passed 0 failed・clippy 両宇宙 0・check-windows error 0。**install 要**

## 2026-09-12（#1426: 裏タブの寸法合わせを毎フレームから key + 間引きへ寄せた）
- `sync_offscreen_pane_sizes` は render から毎フレーム通るのに、当て直す中身（`offscreen_areas`）が既に「key + 2 秒」で回っていたので**材料が同じあいだは同じ答えを出し直していた**。当て直す側も同じ単位へ寄せ、キーは `OffscreenAreaKey`（1 実装 `offscreen_area_key` に集約）+ 既定セル寸法（#647 の再発防止）+ 表示中ペイン数 + ペイン単位ズームの指紋。間隔は `OFFSCREEN_REFRESH_INTERVAL` の 1 定数を両者が見る
- 実測（隔離 GUI の grid-bench・22 ペイン / 表示 4・3000 フレーム）: 596〜776 ns/frame（`render` の 5.88〜6.28%・走査 82 比較 + 18 ペイン当て直し）→ **36〜38 ns/frame**（0.38〜0.41%・走査 0 / 当て直し 0）。同一バイナリの `TAKO_1426_LEGACY=1` は 645〜690 ns で旧挙動を再現。#932 の flicker ラウンドは `late_resize=false` で緑、既存 A/B（`TAKO_932_NO_OFFSCREEN_GEOMETRY=1`）では `late_resize=true` で落ちる = 検出力あり
- 単体 7 本 + 番犬 `issue1426_offscreen_sync_watchdog` 8 本（注入 8 通りで `main.rs:16126` / `:16158` / `:16238` / `:16245` / `:1330` / `:1344` を file:line 名指し）。範囲取りは #1420 の `production_range` の 1 実装を通す。workspace 4551 passed 0 failed・clippy 両宇宙 0・check-windows error 0・隔離セルフテスト完走。**install 要**

## 2026-09-12（#1432: UI に残る `eprintln!` 10 件を通知欄 / 診断へ振り分けた）
- `KNOWN_EPRINTLN` の 10 件を 1 実装（`notify_ui_op_failed` / `log_ui_failure`）へ。ユーザー操作 7 件は通知欄 + persist.log（ドロワーの D&D / 復元・カードの操作と実行ペイン起動・チャットのコピー・Finder の実在しないパス・ノートのリンク）、画面へ出せない 2 件は診断だけ、`autorename` の env つき診断は persist.log へ。画面は `NoticeArea` を 5 つ足して区別（`drawer` / `command_card` / `chat` / `open_file` / `update_window`）
- 隔離 GUI（tako-vd）項目 84e + 90 の A/B: 新 = 6 経路とも通知が出て `bg_lines=3`・成功時は無言で完走（`TAKO_APP_SELF_TEST_OK`）/ legacy（`TAKO_1432_LEGACY=1`）は 5 つとも `None`・`bg_lines=0` で FAILED。`TAKO_1432_LEGACY=1` でも #1399 / #1417 / #1422 は `legacy=false` のまま出る（軸が独立）
- `KNOWN_EPRINTLN` は空・番犬 12 本緑（A/B の env が Issue ごとに 1 対 1 であることの検査を 1 本追加）。workspace 4573 passed 0 failed・clippy 両宇宙 0・check-windows error 0。**install 要**

## 2026-09-12（#841: 偽 XFF でローカルから serve を名乗れるのを接続元プロセスの検証で塞いだ）
- XFF を読む口を `remote::forwarded_identity` の 1 実装へ寄せ、読む前に `local_endpoint::verify_peer` を通す。所有者ゲート（ソケットの uid が自分か root）+ 実行ファイル名ゲート（設置場所では判定しない）を両方通ったときだけ信じ、材料が欠けたら 403。分岐はエンドポイントの形（UDS は検証しない）
- 接続元 pid の解決は `procinfo::loopback_tcp_peer`（macOS = `net.inet.tcp.pcblist_n` の sysctl。**libproc の fd 走査では非 root から root の tailscaled が見えない**ことを実測して方式を変えた / Windows = `GetExtendedTcpTable`）。Windows は所有者を引けないので `owner_check=unavailable` と名乗る（残存リスクは脅威モデルへ）
- 実測 A/B（隔離 daemon + 別プロセス curl）: 新 = 403 `not_tailscale_daemon` + persist.log + `remote status` の `peer_verification` / `TAKO_841_LEGACY=1` = 検証を通らず whois 層まで到達。番犬 9 本・実ソース注入 5 通りで file:line 名指し FAILED。**install 要 / 本番 daemon 再起動要**

## 2026-09-13（#812: ペイン枠線をルート側オーバーレイ 1 枚へ集約した）
- 枠線のインクを `render` の `pane_borders`（`pane_headers` の直後に出すルート側 1 枚）へ集約。本体とヘッダ外枠は枠**幅**と角丸のクリップだけ持ち色を持たない（`Style::is_border_visible()` が false = quad が出ない / 会計は不変）。副作用でヘッダより低いペインの下端枠線が旧は 0 / 576 画素だったのが全部出るようになった（#803 の症状の残り）
- 実測 A/B（`pane-border` 節・同じ場面のまま腕を倒す）: 1 ペインで丸め角の差 **32 画素**（#803 の報告値と一致）・角の外 0 / 2 分割・ズーム・light は 60 画素・外 0。注入 4 通り（描かない / 丸めを落とす / 色規則を外す / 二重塗りへ戻す）で症状を名指し FAILED
- 番犬 `issue812_pane_border_watchdog` 5 本（注入 5 通りで file:line 名指し）。workspace 4623 passed 0 failed・check-windows error 0。**罠**: 手元の lint 2 本では `#[cfg(feature = "visual-test")]` の中が一度も lint されず CI だけ赤（`type_complexity`）→ AGENTS.md / commands.md の lint 行へ 3 本目（`-p tako-app --features visual-test`）を足した。**install 要**

## 2026-09-13（#1439: worker を別タブへ逃がさず、同じタブでフォントを縮めて桁数を確保する）
- 配置を常に `same_tab` へ（#1132 の `new_tab` / `overflow_tab` は対照 `TAKO_1439_LEGACY=1` へ退避）。足りない桁数は worker 領域**まるごと**の自動縮小で確保する 1 実装 `tako_control::worker_font::refit_worker_area`（段の選び方は純関数 `spawn_layout::fit_worker_font`・「その倍率で何桁入るか」だけ GUI の実測。spawn / close / 復元・リサイズが同じ実装を通る）。CLI / MCP は `tako orchestrator layout --auto-shrink-font / --min-worker-font-scale` で 1:1
- 実測 3 腕（tako-vd 上の隔離 GUI・worker 8 体）: 新 = **タブ 1 枚**・全部 `same_tab`・最狭 28 桁（床 0.6 + `cols_short=true`）/ `TAKO_1439_LEGACY=1` = タブ 8 枚（#1132 再現）/ `TAKO_1132_LEGACY=1` = タブ 1 枚・17 桁（#1132 前 再現）。エッジ 3 件（close で 42 → 58 桁へ復帰・`min_worker_cols=0`・`auto_shrink_font=false`）も実測
- 番犬 3 本（注入 8 通りで file:line 名指し）+ 単体 11 本・workspace 4629 passed 0 failed・clippy 3 宇宙 0・check-windows error 0。検証の器から AX の窓移動を外した（#1442）。**install 要**
