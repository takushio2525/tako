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

## 2026-09-12（#1367: 器なしペインの busy を close 確認 / GUI 判定 / agent_running へ届けた）
- #372 で走査は器なしペインも数えていたのに**引く側**が器のセッション名（`busy_sessions`）を見たままで、tmux 未導入 / persist OFF（= cask の既定）では close 確認（#566）が出ず・`busy_children`（#694）が false でスターターが被り・`agent_running` も false だった。問う口を `RunningChildrenScanState::is_pane_busy` の 1 実装へ寄せ、3 経路を `TakoApp::pane_has_busy_children` から通した（旧キャッシュ `busy_backend_sessions` はフィールドごと削除）
- 隔離 GUI（tako-vd・persist OFF・実 Cmd+W = `CGEventPostToPid`）の A/B: legacy（`TAKO_1367_LEGACY=1`）は `busy_agents=1` なのに `busy_children=false` / `display=starter` / **確認なしで即 close**、新は `busy_children=true`（1.0s）/ `terminal` / ダイアログが出てペインが残る。停止後は確認なし・承認経路の監査ログ（`close:kbd`）も器なしで残る
- 番犬 5 本 + A/B + 単体 5 本 + セルフテスト項目 73g（legacy アームで `waited=93.1s busy=false` の FAILED を実測）。注入 5 通りが file:line 名指し。**チャットの列挙（`collect_chat_targets`）は器つき前提のまま**なので器なしの会話表示は別 Issue

## 2026-09-12（#1004: リモート / SSH の手順を新 topic `remote` へ明文化した）
- 手順の全文を `guides/remote.md`（6,903 B）へ置き、prompt 側は master の topic 表 1 行 + solo の `### Remote / SSH` 節だけ（`tako context-budget` 実測 master +83 B / solo +242 B = 依頼上限 500 B 内）。移送でない本文は fixture へ宣言する道（#1154 の `guides_added_after_1154.md`）に乗せた
- 実測で Issue 案と食い違った点を本文へ反映: ①`ssh_config` は **`Include` を読まない**（ssh 自身は読む = 一覧に出ないホストへ名前指定で繋がる）②config の `Port` が 22 以外だと **tako 自身が開いたペインも自動検知に見送られる**（`-p` を渡すため）。失敗 5 種（unresolved / refused / auth / hostkey / conflict 経路）を実 sshd 相手に end-to-end で確認
- 番犬 `remote_guide.rs` 7 本（topic 消失 / master・solo のトリガー消失 / トリガーの肥大 / 手順の prompt インライン化）を注入 5 通りで file:line 名指し FAILED。隔離 solo の実会話は `tako_ssh_hosts` → `tako_orchestrator_guide{topic:"remote"}` → `remote_folder ls` → 接続情報の確認要求へ到達

## 2026-09-12（#1406: 脅威モデルの「別 OS ユーザーの到達は消滅した」を現行の既定へ揃えた）
- #1038 でループバック TCP が既定になったのに「残存リスク」節（`:249`）と「listen 範囲」節（`:87`）が UDS 前提のままだった。実測（`parse_endpoint_spec(None)` = Loopback / 本番 `endpoint_kind: loopback-tcp`）で確定させ、受容するリスクとして書き直した（Windows は `unix_supported()` = false で opt-in が無いことも明記）
- 番犬 `issue1406_threat_model_endpoint_watchdog` 5 本が**コードの既定 ↔ 文書**を双方向で縛る。注入 A/B: 旧記述を戻すと `:87` / `:249` を名指しで FAILED、コードの既定を UDS へ反転すると `local_endpoint.rs:8` と `:83` を名指しで FAILED
- #841 を Windows 限定から「ループバック TCP を使う全プラットフォーム」へ広げた（表題 + 本文追記）。docs + テストのみなので install 不要

## 2026-09-12（#1401: remote stop の stale 経路にも PID の正体確認を通した）
- #329 の fail-safe が stale 経路（PID ファイル無し → `/api/health` の pid を撃つ）に無かった。確認を `kill_stale_daemon` の**内側**へ移して結果型にし（呼び出し側で忘れられない形）、`ps` 出力が**空**のときも「確認できない = 撃たない」へ倒した（判定は `ps_args_is_tako_remote_serve` の 1 実装）。中止時は state を残し理由 + 手順を返す
- 修正前の実測（隔離 state・偽 health + 使い捨て `/bin/sleep`）: 実 CLI が sleep を殺して `{"stopped":true}` を返す / health が停止操作プロセス自身の pid を返す形ではテストバイナリが `signal: 15` で落ちた。修正後は sleep 生存・exit=1・state 残存
- 番犬 `issue1401_stale_stop_identity_watchdog` 7 本（注入 6 通りで `remote.rs:2185` / `2223` / `2364` / `2788` を file:line 名指し）。workspace 4430 passed 0 failed・check-windows error 0。**install 要**

## 2026-09-12（#1400: ssh config の Match の設定が直前の Host へ混入して宛先が化けるのを直した）
- 状態を `Section` の 2 値へ（`Match` / `Include` のあとは「どの Host にも属さない位置」）。キーワードは行頭の最初のトークンで切る（`Match exec "test -f a=b"` で検知が抜けていた）。複数パターンは全部エントリ・`Include` は `~/.ssh/` 起点 + glob + 深さ 16 + 循環検出つきで解決し、読めない理由は `warnings` へ返す
- 隔離 GUI（tako-vd・fixture HOME）で `tako ssh-hosts` の A/B: 修正前は `web1` と `prod(user=root port=2222)` の 2 件、修正後は `edge` / `inner`（Include 配下）/ `web1` / `web2` / `prod(user=null port=null)` の 5 件
- 番犬 `issue1400_ssh_config_watchdog` 7 本。注入 2 通りで file:line 名指し FAILED（`ssh_config.rs:175` ×2 / `:190`）。#1004 の `guides/remote.md`「Include は読まない」も同一 PR で実態へ寄せた

## 2026-09-12（#1399: ファイルツリーのローカル操作の失敗を共有の通知欄へ出した）
- ローカル行の 13 か所が dispatch の結果を `let _ =` / `if result.is_ok()` / `eprintln!` で捨てていて、**ごみ箱移動・リネームが無言で失敗**していた（同じサイドバーのリモート行は #919 から通知欄へ出していた = 1 画面に 2 方針）。出し口を `sidebar::notify_tree_failure` の 1 実装へ寄せ、リモート行と同じ `set_remote_notice` + persist.log（載せるのは操作名と `DispatchError::class()` の分類だけ）へ通した。`commit_inline_edit` の先頭 `take()` をやめ、**失敗時は入力欄と打った名前を残す**
- 隔離 GUI（tako-vd）セルフテスト項目 84b の A/B: 新 = `trash="削除 に失敗しました（<fixture>/gone.txt）: パスが存在しない…"` / `rename="名前変更 に失敗しました（taken.txt）: 既に存在する…"` / `kept="taken.txt"` / 成功時は無言・連続失敗は最後の 1 件が残る → 完走（`TAKO_APP_SELF_TEST_OK`・FAILED 0）。legacy（`TAKO_1399_LEGACY=1`）は `trash=None rename=None kept=None` で **FAILED**
- 番犬 `issue1399_tree_notice_watchdog` 8 本（UI モジュールの dispatch 走査 + `eprintln!` + 1 実装 + `take()` + 分類 + 検出力）。修正前ソースで 6/7 が file:line 名指し FAILED（`sidebar.rs:1540/1570/1613/1630/1646/1659/1705/1718/1732/1740/1971/2014/2019`）。別画面の同型 3 件は `KNOWN_DISCARDED` で段階導入

## 2026-09-12（#1411: tako 自身が開いた SSH ペインを自分の自動検知が見送るのを直した）
- 物差しを「ポートが 22 か」から「**宛先の名前だけでそのポートへ行けるか**」へ（`ConfiguredPorts` = `~/.ssh/config` の `Port`・判断は `port_reachable_by_name` の 1 箇所・材料は `scan` が走る tick だけ読む）。tako の `-p` は config の書き写しなので全部この側に入り、手打ちの `-p`（config に無い）と `-F <別 config>` は従来どおり見送る
- 同じ症状の 2 つ目の原因を同時に直した: tako の `-o ControlPath="…"` は macOS 既定 data_dir に空白があるので `ps` の 1 行が割れ、**続きの語が宛先に見えて** `RemoteCommand` で見送られていた（ポートが 22 でも起きる）。隔離 GUI + 使い捨て sshd の A/B（`TAKO_1411_LEGACY=1`）で legacy = `sessions:[]`（空白あり data dir は `RemoteCommand`・空白なしは Issue と同じ `PortOverride`）→ 新 = pane が `sessions` に `live` で載り、手打ちの `-p` だけが `PortOverride` で残る
- 番犬 `issue1411_self_opened_ssh_watchdog` 8 本（注入 6 通りで `ssh_detect.rs:378` / `:421` / `:423` / `remote.md:107` を file:line 名指し）。単体 16 本・workspace 4442 passed 0 failed・check-windows error 0。**install 要**

## 2026-09-12（#1403: remote daemon の HTTP 受信を少数ワーカーへ並列化した）
- 受信ループを `serve_http_requests` へ切り出し、`Arc<tiny_http::Server>` を既定 4 本（`TAKO_REMOTE_HTTP_WORKERS` で 1..=32）のワーカーが recv する形へ。合流は `thread::scope`（忘れられない）・1 本の panic は `catch_unwind` で監査ログへ・recv 破損は全員で降りる。daemon → app の IPC は `with_app_ipc` の 1 実装が往復の間ロックを握る（呼び出し 4 か所を集約）。波及で `append_audit` を 1 行 1 write へ（書く者が増えたので `writeln!` だと行が混ざる）
- 隔離 daemon（偽 tailscale の whois 3 秒・本番 pid 64092 は不可侵）の A/B: 修正前 B = **2.71 秒** → 新 **0.0004 秒**（A は 3.03 秒のまま）。`TAKO_1403_LEGACY=1` / `TAKO_REMOTE_HTTP_WORKERS=1` はどちらも 2.72 秒で旧挙動を再現。飽和の限界も実測（4 本同時で health 2.51 秒・6 本で溢れた 2 本が 6.04 秒）
- 番犬 `issue1403_http_workers_watchdog` 7 本 + 単体 6 本。注入 9 通りで file:line 名指し FAILED（IPC は `left: 4 / right: 1`）。**mid-file の `#[cfg(test)]` は禁止**（#1401 番犬の走査範囲が切れる）・`LEGACY_ARM` マーカーは実時間アサート用なので付けない。workspace 4487 passed 0 failed × 5・check-windows error 0

## 2026-09-12（#1398: ディレクトリへのシンボリックリンクがファイル扱いになるのを直した）
- 種別を「**辿った先**」で決める 1 実装（`filetree::entry_is_dir`。リンクのときだけ追加 `metadata`）へ寄せ、開く側（`OpenFile` の `is_file()` / `open_plan::route`）と向きを揃えた。辿って初めて起こる循環は `collect_rows` が canonical パスの照合で打ち切り、**打ち切りを行として見せる**（`RowNote::Error`・描画はリモート行と同じ `render_note_row` の 1 実装）
- 修正前ソースの実測: `link` 行が `("link", 1, is_dir=false)` で `toggle_dir` しても増えない → 修正後は展開でき `inner.txt` が depth 2 に出る。実注入の A/B は `file_type()` へ戻すと番犬が `filetree.rs:699` / `:690` を名指し FAILED、照合を落とすと祖先リンクの 2 周目（depth 3 に `README.md`）が出て loop テスト 2 本が FAILED
- 単体 10 本追加（切れたリンク / 相対 / `..` / 相互 ELOOP / 往復 / 500 件超 / git のしるし / 開く経路）+ 番犬 3 本（注入 5 通り）。workspace 4488 passed 0 failed・clippy 両宇宙 0・check-windows error 0。**install 要**
## 2026-09-12（#1375: セルフテストの「固定予算 + CLI の状態読み」16 件を状態待ちへ移し既知リストを空にした）
- 項目 18 / 19 / 21 / 23〜28 / 47 / 47b / 50 / 51b / 66b / 73c / 73f を `wait_for_cli_state`（`wait_for_app_state` + `cli_state_budget` の 1 実装。A/B の口・注入・診断行 `TAKO_SELF_TEST_1375` をここへ集約）へ寄せ、`KNOWN_FIXED_CLI_WAITS` を空にした。分割して新ペインを操作する 4 件は `split_focus_new_pane` で「着地 → アイドル」の 2 段に割り、以降は**返ったペイン ID** を見る（旧 73f は**打ったあとに**分割前のフォーカスを読んでいたので、着地が先だと窓を使い切るまで真にならない = 待ちを伸ばしても直らない形）
- 実測（隔離 GUI・tako-vd）: `INJECT=late` 全項目で 17 か所とも `ok=true`（`waited` = 旧予算 + 5 秒）で完走 / `LEGACY=all` は旧の固定予算（0.8〜15.0s）を再現して完走 / 項目ごとの `LEGACY+late` は **17/17 FAILED**（73f は Issue が観測した `73f: split で新ペインへフォーカスが移らない` そのまま）/ `never` は新経路でも **16/16 FAILED**。高負荷 3 回（load 6.5〜8.4）と load 10〜37 は完走、load 65〜80 の人工負荷では 73c が 4 倍上限（80 秒）を使い切って FAILED = 上限の政策どおり
- 予算の不等式は手書きの表をやめ**ソースから採った 21 か所**を検査（`cli_wait_budgets`）。番犬は空リストで緑
