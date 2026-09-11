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

## 2026-09-11（#1301: 2 秒 tick の全ペイン フルスナップショット 2 本を末尾窓へ）
- `refresh_agent_metrics` / `drive_queued_message_recovery` が毎 tick 全ペインの `Screen`（行ごとに `String` + `Vec<StyleRun>` + `Vec<usize>` の 3 確保）を組んで文字列だけ取って捨てていた（#1001 の H2 / H3）。`TerminalSession::tail_lines(n)`（`Screen` を通さずグリッドから末尾 n 行）+ alt screen ゲート + C3 の判定順入れ替えへ。窓の正本は `AGENT_TUI_TAIL_LINES`=48（Issue 記載の 8 では #1093 / #1123 の上限見出し窓 24 を割る）
- 実測（隔離 GUI・`tako-vd`・1/4/12/22 ペイン・各 20 秒 × 3 窓）: 傾き **0.271 → 0.068 M 命令/秒/ペイン**（全 12 窓の最小二乗）= 2 秒 tick 1 回あたり 0.543 → 0.135 M 命令/ペイン。22 ペインの footprint 46.4 → 39.9 MB
- A/B は `TAKO_1001_C2_LEGACY=1` / `TAKO_1001_C3_LEGACY=1`。番犬 4 本が修正前ソースで file:line 名指し FAILED、`tail_lines` は故障注入 3 種で単体テストが落ちる

## 2026-09-11（#775: GUI 経路の close が workers.yaml へ発生源つきで closed を記録する）
- #658 で 3 経路（ペイン × / タブ × / cmd+W）は配線済みで、残っていたのは**たまり場カードの kill**（退避中ペインはどのタブにも居ないので `remove_pane_with` を通らず、drawer の on_click が後始末を独自列挙）と **`close_reason` が固定文字列 `explicit_close`**（発生源なし）の 2 つ。前者は `kill_shelved_pane` へ集約、後者は `registry::close_reason_for` の 1 実装へ寄せてペインログのクローズマーカーと同語彙（`close:kbd` / `close:gui` / `close:gui-tab` / `close:dispatch(cli)`）にした。CLI / MCP 側の対の経路 `Request::BackgroundKill` も同じ穴だったので併せて配線（スキーマ変更・移行なし）
- 隔離 GUI セルフテスト（`tako-vd`）で 4 経路を実操作して全項目通過: `kbd=closed/close:kbd gui=closed/close:gui cli=closed/close:dispatch(cli, caller=…)` / `tab=closed/close:gui-tab shelf=closed/close:gui` / `active=[]`（`orchestrator workers` に出ない）/ `all_has_tab=true`（--all では残る）/ `plain_entries=15->15`（worker でないペインは増やさない）
- A/B: `TAKO_775_LEGACY=1` で 3 経路が `closed/explicit_close` に戻り項目 87 が FAILED、`kill_shelved_pane` の記録フックを外した注入では項目 148 が `shelf=active/` で FAILED。番犬 5 本は注入 5 種すべてを file:line 名指しで落とす。`drop_backend_session` の境界番犬は drawer.rs の一括免除を撤去して締めた

## 2026-09-11（#757: ログイン失効を接続断・上限とは別種として検知するようにした）
- `WorkerErrorKind::LoginExpired`（`login_expired` / `relogin`）を新設。文言の正本は `agent_cli::login_expired_line` の 1 か所（#983 の起動時未認証検知もそこへ委譲。種別は呼び出し側のゲートで決まる）。判定順序は「ライブのダイアログ > 失効 > 上限メッセージ」で、上限行のほうが新しければ見送る
- 対象アカウント（`error.config_dir` / `error.account`）は会話の transcript の所在から逆引きし、失効を検知したときだけ走らせる。watch / MCP は `wait::error_json` の 1 実装で同形。仕様は FR-2.39
- 隔離 GUI + fixture 実測: 失効 3 文言 → `login_expired` / `relogin` / `account=alt`・上限ダイアログ中は `usage_limit` → 解除後に失効へ遷移・`ENOTFOUND` だけは `api_error` / `resume`・窓の外の残骸は `idle`。legacy アーム（`TAKO_757_LEGACY=1`）は誤分類（`api_error`）と無検知（`idle`）を再現

## 2026-09-11（#1349: コピー行の「選択なしなら Ctrl+C 送信」を実態へ）
- 隔離 GUI（tako-vd・pid 指定の CGEventPostToPid）で実測: 選択なし cmd+C は `sleep` を殺さず `^C` も出ず クリップボードも不変。同じ経路で撃った本物の Ctrl+C は殺した（= 観測に検出力あり）。選択あり cmd+C はコピーになる（回帰なし）
- docs の行を「選択が無いときは何も起きない」へ直し、注記で「中断の Ctrl+C は tako が横取りしない」を明示。実装を docs へ寄せる案 (b) は挙動変更なので Issue へ比較を残して master 判断へ
- 番犬 2 本（コピー行の説明文と `copy_selection` の両方向 / 素の ctrl-c 未バインド）を `docs_keyboard_shortcuts.rs` へ。注入 5 通りで file:line 名指し FAILED

## 2026-09-11（#1347: merge 後のリモートブランチ削除を merge-pr.sh 自身で閉じた）
- 真因は gh の順序（ローカル切り替え → ローカル削除 → リモート削除）。専用 worktree から実行すると 1 手目が `fatal: 'main' is already used by worktree` で落ち**リモートまで到達しない**（`git switch main` 単体で逐語再現・PR #1337 で実発生・棚卸しで merge 済み PR の head が origin に 5 本残存）
- `delete_remote_head_branch` を MERGED 確認の後に置いた（`gh api` で存在確認 → DELETE・冪等・**その PR の head 1 本だけ**・head == base と fork は触らない）。A/B `TAKO_1347_LEGACY=1` が gh 任せの腕で、モック 4 ケース（消し切る / legacy では残る / 冪等 / 門）を追加して 72 assert 緑
- ドッグフーディングで PR #1348 を修正版の `merge-pr.sh` から merge: gh は同じ worktree 事故で 1 を返したが `リモートブランチ … を削除した` が出て `git ls-remote` は空。**GitHub は `.gitattributes` の `merge=union` を適用しない**（実測）ので progress 系を触る PR は CI 中の他 PR 追記だけで CONFLICTING になると分かった（#1246 の前提が GitHub 側では成立しない）

## 2026-09-11（#1357: PWA の Playwright e2e を CI へ載せ、実行手順を置いた）
- 6 spec 50 項目が CI で 1 度も走らず実行手順もどこにも無かった（#425 の契約変更で spec が取り残され #632 が 6 週間放置 → #1089 として再起票）。`package.json` に `e2e` / `e2e:install`・`web/tako-remote/README.md` 新設・AGENTS.md と commands.md に 1 行
- CI の macOS ジョブ末尾で `npm run e2e:install` → `npm run e2e` を **blocking** で実行。追加は実測 **46〜87 秒**（2 run。差は worker 数がランナーの CPU 数に従うため = 33 秒 / 71 秒。3 分のゲート内なので採用）
- 実測: ローカル `50 passed (12.4s)`。空キャッシュから `e2e:install` 14 秒で復旧（#632 の `Executable doesn't exist` を隔離した `PLAYWRIGHT_BROWSERS_PATH` で再現）。ポート衝突は `reuseExistingServer: true` が黙って再利用し `waitForSelector` タイムアウトの形で落ちる

## 2026-09-11（#632: 承認カード e2e 3 本は #1089 で修正済みと実測確定）
- 現状 main で `screenshots-5b.spec.js` は 9/9 PASS・PWA e2e 全 6 spec も 50/50 PASS。`cf85756`（#1089 / PR #1100）が #632 の「対応案」（モックへ `permission_dialog` / assert を `/respond` + `choice`）を既に実装していた
- A/B（`cf85756^` の spec を現行実装へ当てる）で `.approval-card` の 10 秒タイムアウト × 3 を再現 = 症状は実在。旧契約（`/input` へ `y`/`n`）の grep は 0 件、境界の選択肢 N=2 / N=1 も一時 spec で PASS
- コード変更なし（install 不要）。実出力を付けて #632 を close し、真因（PWA e2e が CI で 1 度も走らず、実行手順が package.json / README のどこにも無い）を #1357 として起票した

## 2026-09-11（#1365: 衝突で CI の run が作られない状態を待たずに名指しで案内するようにした）
- base が進んで衝突すると GitHub は merge コミットを作れず `pull_request` の run を作らないので、期待名が永久に未登録のまま `wait-pr-checks.sh` がタイムアウト（2400 秒）まで待っていた（#775 の PR #1359 で 3 回）。未登録が残るあいだだけ `gh pr view --json mergeable` を引き、**2 回連続**で衝突を観測したら終了コード 4 + 取り込み手順を出す形へ
- 判定・案内文・終了コードは `scripts/lib/pr-conflict.sh` の 1 実装で、`merge-pr.sh` の門（待つ前 / merge 直前）も同じ言い方になる（CONFLICTING の拒否は 1 → 4 へ変更）。`UNKNOWN` = 計算中・空文字・`gh pr view` が引けないときは待ちを続ける
- 実測（既定間隔 20 秒）: 21 秒・ポーリング 2 回で 4。A/B `TAKO_1365_LEGACY=1` は同じ入力で待ち続ける（上限 60 秒で打ち切り = 2）。モック 106 PASS / 0 FAIL（従来 72。Test 20〜23 を追加）

## 2026-09-11（#372: 器を持たないペインも sleep guard の busy に数えた）
- 走査対象が器のセッションだけで、tmux 未導入 / persist OFF（cask の既定）では常に空 = `busy_agents` が無条件に 0。全ペイン対象 + 器なしは PTY 直下の子から辿る二段構え（判定 `has_running_descendants` / 数え方 `busy_count()`）へ。CLI の `status` も IPC でアプリの値を採る（保持フラグと busy はプロセスローカル static）
- 隔離 GUI（persist OFF・tako-vd）: 修正前は `sleep 300` 稼働 75 秒で 0 のまま → 修正後 1 + pmset に assertion、停止で 2 秒で解放。器あり構成も回帰なし。A/B `TAKO_372_LEGACY=1`・番犬 4 本が修正前ソースで file:line 名指し FAILED

## 2026-09-11（#633: 承認カードの command を「承認を求めている操作」から始める）
- 実測で真因を絞った: 本文の境界は「当たったら捨てる」3 つ（罫線 / 0 桁の非空行 = #1293 / 番号つき行）しか無く、**罫線を引かず箱ごと 0 桁で描く**許可ダイアログ（agy / claude の箱なし 2 形）では捨てる材料が無い。Issue 実測値の `⏺` 行そのものは #1293 が既に切っていた
- `dialog::BODY_START_MARKERS` + `body_start_row` で**本体の開始マーカーを起点**にし、無ければ従来のブロック抽出へフォールバック（起点を下げるだけ = 結果は必ず従来の接尾辞）。FR-2.25.12 / conventions #1293 節に追記
- 番犬 `crates/tako-control/tests/issue633_permission_command_anchor.rs` 8 本 + 単体 3 本。A/B `TAKO_633_LEGACY=1` で 2 本 FAILED（agy に発話が混ざる / 罫線の無い画面が `⎿` 行から始まる）
