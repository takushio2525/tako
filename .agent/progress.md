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

## 2026-09-13（#1441: 深い data dir でも IPC が立つようにし、立たないときは黙らせない）
- ソケットの実体を data dir 直下から外し、`tako_core::ipc_socket` の 1 実装（`<data_dir>/tako.sock` が `sun_path` に収まればそのまま / 収まらなければ `$TMPDIR/tako-<data dir の 16 桁 FNV-1a>.sock` → `/tmp`）へ寄せた。data dir 側には参照 `tako.sock.path` だけ。**symlink では解決しない**（上限は繋ぐ側の `connect()` に掛かる）
- 実測 A/B（tako-vd 上の隔離 GUI・data dir 150 バイト）: 新 = 受け口 75 バイト・`tako list` exit 0 / `TAKO_1441_LEGACY=1` = `path must be shorter than SUN_LEN` の 1 行のみ・discovery 空・`tako list` exit 1（#782 の症状の再現）。浅い data dir は固定パスのまま不変。深い `$TMPDIR`（110 バイト）は `/tmp/tako-<hash>.sock`（31 バイト）へ
- bind の成否を記録して `check_health` の `ipc` 節 + 通知欄（`notify_ui_failure` の 1 実装）へ。CLI は `tako check-health`（新設・MCP と 1:1）で、**届かないときはローカルの受け口診断**を返す。番犬 8 本 + 統合 1 本・注入 8 通りで file:line 名指し FAILED。**install 要**。**罠**: `scripts/check-windows.sh` は `tests/` を見ていなかったので統合テストの `cfg(unix)` 漏れを**手元 error 0・CI の Windows だけ赤**で通していた（#1264 の `--no-run` が E0433 を 2 件）→ `--all-targets` を既定にし、同じ注入で旧 = 0 件 / 新 = 名指し検出を実測

## 2026-09-13（#1442: 窓の位置・寸法を tako 自身の口で決められるようにした）
- `TAKO_DISPLAY` つきの隔離起動は保存フレームを無視して必ず中央 960x600 で開き、AX で動かすと本番 tako の窓に当たっていた。起動時 `TAKO_WINDOW_BOUNDS=x,y,w,h`（`w,h` だけなら中央）と `tako window move` / `resize`（MCP 1:1）を足し、解釈・検査は `tako_core::platform::window_bounds::resolve` の 1 実装へ。`window list` に `bounds` / `display` を載せて AX 無しで読めるようにした
- 実測（tako-vd = 1512,0 の 2560x1440・CGWindowList）: 修正前は保存フレーム 200,100,1400,900 があっても `2312,420,960,600` / 新 `100,50,1400,900` → `1612,50,1400,900` / `TAKO_1442_LEGACY=1` は `2312,420,960,600`。はみ出し・最小未満・読めない形は既定へ落ちて persist.log に理由（CLI / MCP はエラー）。**罠**: `move` 直後の `resize` が render 待ちの古い位置を土台にして移動を打ち消す（セルフテスト項目 77b が実際に落ちた）ので、依頼した矩形はその場で `window_frames` へ記録する
- 番犬 9 本（注入 10 通りで file:line 名指し）+ 単体 14 本・セルフテスト項目 77b（legacy 腕は FAILED）。workspace 4657 passed 0 failed・clippy 3 宇宙 0・check-windows error 0。**install 要**

## 2026-09-13（#1449: スマホの「+」を 3 択にして、ターミナル / SSH も新規に立てられるようにした）
- 「+」を `LaunchSheet`（master / ターミナル / SSH）へ。**操作は 1 つも新設していない**（ターミナル = `POST /api/tabs` だけ / SSH = #1080 の `POST /api/ssh {target:"tab"}`）。role は 3 種とも **Manage 据え置き**（「interact 以上」案は既存 Interact 端末の権限が黙って広がるので不採用 = 脅威モデルへ明記）
- 経路の宣言を `remote_launch::LAUNCH_ROUTES`（role / 監査 / PWA の呼び口）へ集約し `required_role` が引く形に。番犬 6 本 + e2e 11 本（61 passed）+ 実経路 `scripts/test-remote-launch-1449.sh` 38 PASS（**HOME ごと隔離**して実 `~/.ssh/config` を読まない = #927）。注入 7 通りすべて名指し FAILED
- **既存テストの罠 2 つを直した**: #841 以降 `test-remote-master-launch.sh` が全 403 で落ちていた（`TAKO_REMOTE_TRUSTED_PEER_NAMES="curl"`）→ 34 PASS へ復旧 / tako ペインからの実行は `TAKO_SOCKET` 継承で **CLI が本番 GUI を触る**（実測でタブ + QR が本番へ出た）→ `unset` + 起動ガード。**install 不要 / daemon 再起動要**
