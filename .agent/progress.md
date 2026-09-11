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

## 2026-09-11（#1297: 入力欄の AI ゴースト提案を「人の下書き」と読まないようにした）
- #1273 の最後の関門（入力欄が空か）が**文字列だけ**だったので、claude が空欄へ dim で描く AI ゴースト提案が下書きに見えて覆せなかった（本番 pane 1636 / 1761 / 1775 / 1784 が永久 busy）。判定へ `read_pane` の `input_status.style` と同じ 1 実装から採った属性を渡し、ghost / none = 下書きなし・user / mixed = 下書きあり（#1273 の安全側は維持）。述語の正本は `InputStyle::is_user_draft`
- 属性の取れない素の tmux capture は従来の文字列判定へ落とし、`worker_status` の新フィールド `idle_override_blocked=input_draft_unreadable` に理由を残す（MCP / CLI 1:1）
- 実測: 実 PTY の dim 提案で `InputStyle::Ghost` → `status=idle` / 修正前ソースでは単体 5 本が `busy` で FAILED。A/B は `TAKO_1297_LEGACY=1`。番犬 4 本が 5 種の注入（文字列へ戻す / ghost を下書き / mixed を空 / 渡さない / 採らない）を file:line 名指しで落とす。実 claude 2.1.258 の入力欄プレースホルダが `ESC[2m` であることも隔離 tmux で確認

## 2026-09-11（#1314: psmux の conf を tmp → rename で差し替えるようにした）
- `backend::psmux::ensure_conf` が `new-session -f` の直前に最終パスへ直書きしていた（#625 の機序③が psmux 側に残存）。`write_conf_in`（pid + 連番の tmp → rename・rename 失敗時は tmp を掃除）へ寄せ、tmux 側（#625）と同じ作法に揃えた。**内容が同じなら書かない**ので、2 回目以降の spawn は最終パスに触らない（Windows の `FILE_SHARE_DELETE` 依存の置換そのものを避ける）
- A/B `TAKO_1314_LEGACY=1`（修正前の直書き）: 8 スレッド × 200 書き込みの再現テストが旧アーム **60/60 FAILED**（`len=0` / 短い本文が長い本文の先頭を潰した `len=55296`）→ 新アーム 0/60・負荷下（load 8 / 55〜60）0/50 × 2
- **Windows CI で tako-core のテストが 1 件も走っていなかった**（`cargo test --workspace` は非ブロッキング + #583 で打ち切り）ので、#1282 と同じ形で `backend::psmux` の実行検査を blocking ステップとして追加。実機確認は Windows 機が offline のため Issue に手順を残した

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

## 2026-09-11（#757: ログイン失効を接続断・上限とは別種として検知するようにした）
- `WorkerErrorKind::LoginExpired`（`login_expired` / `relogin`）を新設。文言の正本は `agent_cli::login_expired_line` の 1 か所（#983 の起動時未認証検知もそこへ委譲。種別は呼び出し側のゲートで決まる）。判定順序は「ライブのダイアログ > 失効 > 上限メッセージ」で、上限行のほうが新しければ見送る
- 対象アカウント（`error.config_dir` / `error.account`）は会話の transcript の所在から逆引きし、失効を検知したときだけ走らせる。watch / MCP は `wait::error_json` の 1 実装で同形。仕様は FR-2.39
- 隔離 GUI + fixture 実測: 失効 3 文言 → `login_expired` / `relogin` / `account=alt`・上限ダイアログ中は `usage_limit` → 解除後に失効へ遷移・`ENOTFOUND` だけは `api_error` / `resume`・窓の外の残骸は `idle`。legacy アーム（`TAKO_757_LEGACY=1`）は誤分類（`api_error`）と無検知（`idle`）を再現
