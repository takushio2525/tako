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

## 2026-09-11（#1336: 要件番号の一意性と参照の実在を番犬で固定した）
- `crates/tako-control/tests/fr_number_watchdog.rs` 1 本。定義の形は 3 つ（章 `## FR-5` / 節 `### FR-2.34` / 表 ID `| FR-2.34.1 |`）で **1 段も拾う**（`NFR-1`〜`8` が 1 段なので 2 段以上だけ見ると見逃す。1 段を定義に持つと章番号への言及も偽の参照切れにならない）。定義 482 件 / コード参照 613 件を実測
- A/B は fixture を置かず**現行テキストへの逆置換**（#1319 の変更は番号 14 行だけなので `84c16c7^` と同値。置換が空振りしたら落ちるガード付き）。実データ検証も実施: `84c16c7^` を置くと `要件番号は一意` が **FAILED で 3 系統 11 番号を行番号つきで名指し**（FR-2.34 + 配下 8 / FR-3.17 / FR-3.18）
- 自分自身も走査対象なので**架空の番号を書くと自分で拾う**（実装中に 2 度拾われた）。例外リストは作らず例を実在番号へ寄せ、参照切れの検出力は「定義側を 1 つ削る」形で証明。参照切れは現行 0 件

## 2026-09-11（#1333: PR の「CI 3 本緑を待って merge」を共通の待ちスクリプトへ寄せた）
- 「出ているチェックが全部非 pending = 完了」は push 直後に Cloudflare Pages しか登録されていない瞬間に通る（#1313 = PR #1328 が CI 完了前 merge・過去に #1253 / #1282）。判定を `scripts/wait-pr-checks.sh` の 1 実装へ寄せ、**期待名が全部そろって全部 completed** を **2 回連続**観測してから確定する形にした（期待名は `.github/workflows/*.yml` の pull_request job から導出・外部連携の Cloudflare Pages だけ定数）
- merge は `scripts/merge-pr.sh`（揃わなければ merge しない / CONFLICTING・BEHIND は待たずに拒否）。モックテスト 14 ケース（偽 gh + 偽リポジトリ）は CI の macOS ジョブで走り、A/B `TAKO_1333_LEGACY=1` の腕が「1 本だけで完了」「揺れの 1 回目で確定」を再現して Test 13 / 14 が固定する
- この PR 自身をこの経路で merge する（初回の実運用で CI 赤を 2 回検出して拒否 = #1313 の予算超過と #1343 のコンパイル破損）。**merge 直前に「緑を出した run の後に main が進んだか」を警告する**（PR の CI は merge 結果を検査するが merge base は run 開始時点で凍る = #1343 の事故クラス）

## 2026-09-11（#1318 #1321: README のリモート transport と `tako --help` の説明を実態へ）
- README の日英を #1038 後の実態（Tailscale `serve` → ループバック TCP `127.0.0.1` のエフェメラルポート・LAN / 外部から到達不可）へ。事実に反する「TCP ポートを一切開きません」を削除し、`tako remote --help` の `Tailscale Serve + UDS` も同じ形へ
- #982 で `AgentSupport` が `Platform` の doc の上へ挿し込まれ 3 行すべてが agent-support の説明になっていたのを分離。`platform` は `PlatformArgs` の「参照引数」露出をやめて自前の doc を持つ（`--help` 実出力で確認）
- 挙動は不変（doc コメント / README のみ）。`remote.rs:45` / `:1473` の同じ #1038 前の記述は #1332 へ切り出した

## 2026-09-11（#1332: remote.rs の doc が #1038 前の保証を語っていたのを実態へ）
- モジュール doc（`remote.rs:44`）と `run_daemon` doc が「UDS のみで listen・TCP ポートを一切開かない・別 OS ユーザーは接続自体が不能」のままで、同ファイルの実装（`:135-147`）と `threat-model-remote.md` と逆を向いていた。既定 = ループバック TCP / UDS は `TAKO_REMOTE_ENDPOINT=unix` の opt-in / 失われた保証と緩和は threat-model へ誘導、の形へ
- 横断 grep（`TCP ポートを一切` / `接続自体が不能` / `UDS + Tailscale` / `UDS 専用`）で同じ誤りが 3 か所残っていた: `protocol.rs:1137` の `RemoteStart` doc・MCP カタログ `tako_remote_start` の説明文（+ スナップショット）・`admin_request` の「UDS 専用」（実装は `local_endpoint` 経由で両対応）
- 挙動は不変。`remote.rs` / `protocol.rs` の diff はコメント行のみであることを `git diff -U0` で機械確認

## 2026-09-11（#1317 / #1322 / #1323: docs サイトの取り残し 3 ページを実挙動へ合わせた）
- settings（8 タブ・`--tab` は英語スラッグのみ）/ architecture（存在しない `shelve` → `background`・IPC に Windows の named pipe）/ keyboard-shortcuts（macOS / Windows の 3 列表へ作り直し）
- 表は `keybindings.rs` から全件起こした（macOS 45 本 / Windows 45 本・差分 0）。番犬 `crates/tako-control/tests/docs_keyboard_shortcuts.rs` が両方向を検査する（注入 3 通りでキーを名指し FAILED）

## 2026-09-11（#1316: docs の MCP / CLI の件数と掲載漏れを直し、番犬で拘束した）
- 実測（`mcp::tools()` = 149 / `tako --help` = 84）に対し docs は 147 / 128 / 69 / 68 を名乗り、MCP 11 ツール・CLI 10 コマンドが一覧に 1 度も出てこなかった。件数 11 か所を直し欠落を既存カテゴリへ追加（`mcp-tools.md` の「機械的に抽出」note も手書き + 番犬の事実へ）
- 番犬 `crates/tako-control/tests/docs_tool_inventory.rs` 6 本が件数と名前集合の両方を拘束。CLI 集合は `main.rs` の `enum Command` のソース走査（実バイナリの `--help` と差分 0 を実測）で、**記述が見つからないことも FAILED** にして黙って通らない形
- A/B（修正前 docs へ戻す）: 4/6 FAILED = 件数 11 か所を file:line で・欠落 21 件を名前で名指し → 修正後 6/6 緑。案 (a)（ページの生成物化）は構成変更なので未実施・ユーザー相談へ回す

## 2026-09-11（#1301: 2 秒 tick の全ペイン フルスナップショット 2 本を末尾窓へ）
- `refresh_agent_metrics` / `drive_queued_message_recovery` が毎 tick 全ペインの `Screen`（行ごとに `String` + `Vec<StyleRun>` + `Vec<usize>` の 3 確保）を組んで文字列だけ取って捨てていた（#1001 の H2 / H3）。`TerminalSession::tail_lines(n)`（`Screen` を通さずグリッドから末尾 n 行）+ alt screen ゲート + C3 の判定順入れ替えへ。窓の正本は `AGENT_TUI_TAIL_LINES`=48（Issue 記載の 8 では #1093 / #1123 の上限見出し窓 24 を割る）
- 実測（隔離 GUI・`tako-vd`・1/4/12/22 ペイン・各 20 秒 × 3 窓）: 傾き **0.271 → 0.068 M 命令/秒/ペイン**（全 12 窓の最小二乗）= 2 秒 tick 1 回あたり 0.543 → 0.135 M 命令/ペイン。22 ペインの footprint 46.4 → 39.9 MB
- A/B は `TAKO_1001_C2_LEGACY=1` / `TAKO_1001_C3_LEGACY=1`。番犬 4 本が修正前ソースで file:line 名指し FAILED、`tail_lines` は故障注入 3 種で単体テストが落ちる

## 2026-09-11（#775: GUI 経路の close が workers.yaml へ発生源つきで closed を記録する）
- #658 で 3 経路（ペイン × / タブ × / cmd+W）は配線済みで、残っていたのは**たまり場カードの kill**（退避中ペインはどのタブにも居ないので `remove_pane_with` を通らず、drawer の on_click が後始末を独自列挙）と **`close_reason` が固定文字列 `explicit_close`**（発生源なし）の 2 つ。前者は `kill_shelved_pane` へ集約、後者は `registry::close_reason_for` の 1 実装へ寄せてペインログのクローズマーカーと同語彙（`close:kbd` / `close:gui` / `close:gui-tab` / `close:dispatch(cli)`）にした。CLI / MCP 側の対の経路 `Request::BackgroundKill` も同じ穴だったので併せて配線（スキーマ変更・移行なし）
- 隔離 GUI セルフテスト（`tako-vd`）で 4 経路を実操作して全項目通過: `kbd=closed/close:kbd gui=closed/close:gui cli=closed/close:dispatch(cli, caller=…)` / `tab=closed/close:gui-tab shelf=closed/close:gui` / `active=[]`（`orchestrator workers` に出ない）/ `all_has_tab=true`（--all では残る）/ `plain_entries=15->15`（worker でないペインは増やさない）
- A/B: `TAKO_775_LEGACY=1` で 3 経路が `closed/explicit_close` に戻り項目 87 が FAILED、`kill_shelved_pane` の記録フックを外した注入では項目 148 が `shelf=active/` で FAILED。番犬 5 本は注入 5 種すべてを file:line 名指しで落とす。`drop_backend_session` の境界番犬は drawer.rs の一括免除を撤去して締めた

## 2026-09-11（#1347: merge 後のリモートブランチ削除を merge-pr.sh 自身で閉じた）
- 真因は gh の順序（ローカル切り替え → ローカル削除 → リモート削除）。専用 worktree から実行すると 1 手目が `fatal: 'main' is already used by worktree` で落ち**リモートまで到達しない**（`git switch main` 単体で逐語再現・PR #1337 で実発生・棚卸しで merge 済み PR の head が origin に 5 本残存）
- `delete_remote_head_branch` を MERGED 確認の後に置いた（`gh api` で存在確認 → DELETE・冪等・**その PR の head 1 本だけ**・head == base と fork は触らない）。A/B `TAKO_1347_LEGACY=1` が gh 任せの腕で、モック 4 ケース（消し切る / legacy では残る / 冪等 / 門）を追加して 72 assert 緑
- ドッグフーディングで PR #1348 を修正版の `merge-pr.sh` から merge: gh は同じ worktree 事故で 1 を返したが `リモートブランチ … を削除した` が出て `git ls-remote` は空。**GitHub は `.gitattributes` の `merge=union` を適用しない**（実測）ので progress 系を触る PR は CI 中の他 PR 追記だけで CONFLICTING になると分かった（#1246 の前提が GitHub 側では成立しない）

## 2026-09-11（#632: 承認カード e2e 3 本は #1089 で修正済みと実測確定）
- 現状 main で `screenshots-5b.spec.js` は 9/9 PASS・PWA e2e 全 6 spec も 50/50 PASS。`cf85756`（#1089 / PR #1100）が #632 の「対応案」（モックへ `permission_dialog` / assert を `/respond` + `choice`）を既に実装していた
- A/B（`cf85756^` の spec を現行実装へ当てる）で `.approval-card` の 10 秒タイムアウト × 3 を再現 = 症状は実在。旧契約（`/input` へ `y`/`n`）の grep は 0 件、境界の選択肢 N=2 / N=1 も一時 spec で PASS
- コード変更なし（install 不要）。実出力を付けて #632 を close し、真因（PWA e2e が CI で 1 度も走らず、実行手順が package.json / README のどこにも無い）を #1357 として起票した
