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

## 2026-10-12（#1992 S0: PC リソース管理の設計 = 観測・順番待ち `tako queue`・優先度・ETA・見せ方・CLI / MCP・AI の運用・Windows と子 Issue 8 本）
- `.agent/plans/2026-10-resource-manager.md`: 観測（全プロセス走査）+ 明示の入口 `tako queue <cmd>` + 透過の cargo jobserver（`CARGO_MAKEFLAGS` の fifo）の 3 層。調停役は `ipc::submit` の手前で答え UI スレッドを通らない・再起動をまたぐ・tako 不在は待たずに走る。メモリ圧 warn は 1 件ずつ・critical で止める。MCP は `tako_queue` 1 本（約 1.8 KB）。判断点 10 個（§11）
- 実測（読み取りのみ）: 機全体 0.008 ms・全 1,320 プロセス走査 CPU 5.2 ms（5 秒間隔で 1 コアの 0.10%）・`ri_user_time` は mach tick・warn は普段の 42%・claude 1 本は子孫込み約 0.6 GB・cargo 1.95 は外の fifo jobserver に従う・tmux 3.6b は OSC 9;4 を通さない。子 Issue #1993〜#2000

## 2026-10-12（#1963: tako mod S7-4 = コマンドカードを mod で描く。会話の行にカード・帯の 1 行に card N と Copy / Run・描いたカードだけ GPUI のカード帯から外す）
- 応答の `view.cards`（store の論理文字列 + tako が組んだ CLI の引数 + ホットキー）で mod が `tako_show_command` の行と帯に描き、`renders.cards` で返す。GPUI の帯は `claude_mod::card_owner` の 1 本（新鮮な報告 ∧ renders.cards ∧ `cards.place`。拾う前の新しいカードは 5 秒まで保留）で外し、2 秒 tick で戻す。`show-command --list` の `owner`・A/B `TAKO_1963_LEGACY=1`・MATRIX `claude_mod_cards`
- 実測: `scripts/test-mod-command-cards-1963.sh` 48 PASS（実 claude 2.1.294・偽 Messages API が MCP を呼ぶ・行数 44 → 44 を 0.1 秒おきに採って一度も縮まない・全画面 / main screen のキーで Run・報告停止 45 秒 / claude 終了 0.2 秒で GPUI の帯へ・Copy 157 字改行 0）・visual-test `mod-card`（A/B で ① FAILED）・番犬 `issue1963_*`。罠: GUI ペインの claude の `$.ui.copy` は OSC 52 で本物のクリップボードへ届く

## 2026-10-13（#1991: git の書き込み系・起動時の orphan の後片付けを UI スレッドから外す + 再描画上限の CPU を蓋を開けて実測）
- commit / pull / push / stage / unstage / checkout / branch / merge / merge-abort / conflicts は cwd だけ写した `OffloadJob::GitWrite`（本体は同期の腕と同じ `git_write_reply`・同じリポジトリへの書き込みは `git_write_turn` で 1 本ずつ）、起動時の一括回収は計画だけ UI スレッドで本体は background、自動復帰の tmux の 3 口は `run_tmux`（上限つき）。A/B `TAKO_1991_LEGACY=1`・番犬は #1984 のものへ注入 8 通り（計 25）
- 実測: `scripts/test-ui-thread-offload-1991.sh` 修正後 25 PASS（別の要求 0.03〜0.13 秒・E1〜E4）/ 修正前 11 FAIL（G1〜G10 は 8 秒で打ち切り・R は 600 秒起動しない）。起動時の一括回収は本番 GUI が居ると見送り = 実経路は未実測。self-test は 82b / #1417 / #1422 が 2 回とも通過（全体は別々の無関係な項目 111 / #694 で止まった）。蓋を開けた CPU は上限なし 15.4% → 30 fps 12.5%（既定は据え置き）

## 2026-10-13（#1948: 言語の同定の正本 `tako_core::file_type` へ色分け・プレビュー種別・HTTP の型・インデント・Enter・コードブロックの別名を寄せ、`tako open` / `tako edit` の応答に `file_type` / `syntax`）
- 表は 1 形式 = 1 行（58 行。全 213 構文は並べない）。preview / text_edit / open_plan の拡張子の分岐を表の列へ移し、`.ino` の補い（#1949）・画像の形式・MIME も表へ。番犬 `issue1948_file_type_watchdog`（寄せる前の 3 ファイルで分岐 70 件を file:line で名指す・実行の既定へ `erl` の注入で `runner_defaults.rs:135` を名指す）。アイコンは拡張子を小文字で引く。カタログ +153 B
- 実測: 寄せる前の判定の写しと 2,324 パス × 7 列・1,430 トークンを比べる単体が寄せる前のコードでも緑、寄せた後の差分は名指した 2 種（`CMakeLists.txt` の Enter・特殊名の大文字違いの色）だけ。構文は 213 のまま。`scripts/test-file-type-1948.sh`（隔離 GUI）17 PASS = CLI と MCP の応答が字面まで一致・`TAKO_1949_LEGACY=1` で `.ino` だけ Plain Text

## 2026-10-12（#1977: 状態を送らないサーバは最初の診断まで読み込み中・表の版を上げたら古い版の段を片付ける・引き直しの間隔を直前の 1 巡で伸ばす）
- 実の pyright 1.1.414 は索引ができるまで補完に `{"isIncomplete":true,"items":[]}`・最初の診断が握手から 0.6 秒。`goto::loading_of` を「最初の診断まで（上限 10 秒）」へ。`fetch::sweep_old_versions`（印あり・今の版より前・生きた `.tako-lease-<pid>` 無しだけ消す）を `status.fetch.cleanup` / `install` の `cleanup` へ。間隔は 1 巡 × 50 を 15 秒〜5 分（`status.recheck`）。A/B `TAKO_1977_LEGACY=1`・カタログ +126 B
- 実測: 実の pyright を猶予 0 の注入で旧 3/3 が 0 件・新 3/3 が 2 件（注入なしでも 2.0 → 0.54〜0.62 秒）。ログインシェル 1 本 = 普通の rc 0.28〜0.52 秒 / conda の hook 1.8〜2.5 秒。e2e 8 本（偽サーバ `quiet-loading`）・製品への注入 3 通りで名指し。Windows 実機（3）は Issue のコメントへ手順

## 2026-10-13（#2016: 右パネル tasks の返答欄 = IME が欄の中に出る・欄の幅で折り返し 8 行で止まる・押した次のフレームで欄が空・返答の履歴を古い順・時刻つきでフォームの上へ）
- 真因（visual-test `tasks-input` の A/B）: IME は宛先が TaskComment なのに描画が未確定文字列もキャレット矩形も出さず候補窓がターミナルの原点へ / 行を縮まない横並びで描き 183 字でも 1 行・31 行で 566px / 送信が UI スレッドで積む → 運ぶ → 記録 → 一覧を回してから欄を空に。返答は dispatch と共有の 3 段（`user_task_respond_store` / `_deliver` / `_finish`）へ、時刻は `user_task::response_stamp` を画面と `tako todo show` で共有。A/B `TAKO_2016_LEGACY=1`・番犬 `issue2016_tasks_input_watchdog`（注入 14 通り）・MCP カタログ無変更
- 実測: `scripts/test-tasks-input-2016.sh` 6 PASS（NSView へ NSTextInputClient を直接送る IME・折り返し 7 行・上限 136px・送信が押下に足す時間 1.1 ms ← 旧 67〜91 ms（debug・105 KB）・続けて 3 回押しても順に積む・履歴が show と同じ・積めなかったら欄へ戻る）/ 旧は同じ節で FAILED。実 IME（かなモードの打鍵）は未検証 = manual-checks

## 2026-10-13（#1961: マウス報告を要求する TUI へ左クリック / ドラッグを渡す = ⌥ / Shift + ドラッグで tako の選択・tako mouse / tako click）
- 判断と符号化は `tako_core::mouse_forward` の 1 実装（route → decide → click_report_bytes）。器のペインは器のペイン側のフラグを background で問い合わせ、答えまでのイベントを溜めて send-keys -H で渡すか選択として再生。設定 `mouse_click_forward`（既定 ON・serde default）= `tako mouse` / MCP `tako_scrollback` の `mouse_forward` / 設定画面。AI は `tako click` / `tako_send_input` の `click`。A/B `TAKO_1961_LEGACY=1`・カタログ +861 B
- 実測: `scripts/test-click-forward-1961.sh` 7 PASS（visual 直接 / 器 × 新旧 = 新は 9 場面 OK・旧は ① で FAILED、実物の claude 2.1.294 で Jump to bottom・帯の compact / split right・AI のカード（#1963）の Run / 閉じるをクリックで押せた）。器の e2e は 150 回で押す / 離すの欠け 0・断片 0。番犬は注入 11 通りを名指し。罠: claude は mount 後 300 ms のクリックを捨てる

## 2026-10-13（#2009: 見えていないプレビューを 30 秒で手放し、キャッシュの実効上限を見えている量に合わせた）
- 内訳（隔離 GUI・release・14 タブ / 器つき端末 28 / プレビュー 8）: 修正前 943 MB のうち IOAccelerator 255 MB はほぼ全部が裏のプレビュー画像の GPU テクスチャ。`sweep_preview_memory`（`pane_visibility` が Hidden で 30 秒）がデコード結果・PDF の表示中と前後以外の PNG（並びは空白で保つ）・動画のフレームと止まったプレイヤー（`VideoResume`）を手放し、表に戻すと作り直す。実効上限 = 見えている量 × 2（下限 128 MiB・天井は設定）。`tako preview-cache` に `panes` ほか。A/B `TAKO_2009_LEGACY=1`・カタログ +281 B
- 実測: `scripts/test-preview-memory-2009.sh` 8 PASS（IOAccelerator 255 → 4.1 MB・LRU 314 → 0・元データ 70 → 8 MB・総量 943 → 459〜664 MB・visual は戻した絵が画素一致・旧は ② で落ちる）。器のペイン 1 本 0.23〜0.26 MB（tmux と二重に持たない = (5) は入れない）。残る heap の大半は malloc が抱える解放済みの空き（`MallocLargeCache=0` で 325 → 65 MB = 提案）

## 2026-10-13（#1692: エディタの行の操作 = 複製・移動・行ごとのインデント・行コメントの付け外しを TextBuffer の 1 実装に入れ、打鍵・CLI・MCP を 1:1 にした）
- 本体 `text_edit/line_ops.rs`（1 操作 = 1 回の編集 = undo 1 回・移動は既存の改行を並べ替えるだけ = CRLF に LF を混ぜない・対象行は `line_block` の 1 実装で ⇧Tab と共有・記号は file_type（#1948）の行の `comment` 列・記号の無い形式は理由を返す = `EditorCommand::apply` が `Result`）。打鍵は macOS ⌥↑↓ / ⇧⌥↑↓ / ⌘/ / ⌘] ⌘[・Windows は Ctrl 段（Alt+↑↓ はペインのフォーカス移動）・⌘/ ⌘] ⌘[ は shift を受けない（⇧⌘] はタブ切替）。CLI `tako edit line <操作>` / MCP `tako_preview_edit` の `command`（`line-*`）・カタログ +304 B
- 実測: `scripts/test-line-ops-1692.sh` 51 PASS（実打鍵 6 相・CLI / MCP 18 手順で保存後のファイルがバイト一致・応答が字面一致・空 / 改行なし / CRLF / 記号なし）。注入 6 通りで単体・番犬 `issue1692_line_ops_watchdog`・衝突の番犬が file:line で名指し

## 2026-10-13（#1685: LSP の参照検索・呼び出し階層・型階層 = 右クリック → 右パネルの一覧 / ツリー・行の ⌘クリックで定義ジャンプと同じ着地・CLI / MCP は `tako_lsp` の 10 action）
- 正本 `tako_core::lsp::xref`（応答の読み取り・ツリーの展開状態・ペインごとの保持 = 閉じたら消す）を dispatch `LspXref` / `LspResults` → `tako lsp references` 等 / MCP が 1:1 で通り、着地は `land_on_place` で定義ジャンプと共有。lsp-types 0.97 が `typeHierarchyProvider` を落とす穴・項目が増えて小窓で見切れるメニュー（窓に収めてスクロール）も直した。A/B `TAKO_1685_LEGACY=1`・カタログ +1.2 KB
- 実測: `scripts/test-lsp-xref-1685.sh` 32 PASS（visual-test `xref-1685` = ⌘クリックでペイン 2 → 3・90 行目が先頭・閉じたら保持 0、旧腕は ① で名指し）・実の rust-analyzer で tako の `symbol_kind_name` の参照 8 件が grep と一致・e2e 9 本・番犬（注入 10 通り）

## 2026-10-13（#1694: ワークスペース横断検索 = ⇧⌘F の右パネル search ビュー・結果の行でその行を開く・tako search / MCP tako_search）
- `tako_core::workspace_search`（`ignore` の walker + `regex`。.gitignore はリポジトリ内だけ・`.git` / バイナリ / 8 MB 超は数だけ・ディレクトリのリンクは辿らない・保持 2000 行 / 30 秒）を `prepare_offload` → background → 出す検索だけ置き場へ。着地は #1685 の `land_on_place`（`plan_landing` + 行指定の open）。右パネルの最小幅を 220 → 240px（条件つきのタブ 3 本 = 7 本 + バッジが収まる幅）。暗黙の範囲からホーム / ルートを外す。Windows は Ctrl+Shift+G。カタログ +1,674 B・番犬 `issue1694_*`
- 実測 `scripts/test-workspace-search-1694.sh` 16 PASS: 10,001 ファイルを 0.17 秒・4.7 秒の走査中に別の要求 83 回で最大 0.040 秒（`TAKO_OFFLOAD=0` の腕は 4.57 秒で NG）・メインスレッドの sample に走査 0・visual-test で ⌘クリック → ペイン 1→2・90 行目が見える

## 2026-10-13（#2017: 監視の走査（`claude agents --json`）を mod の報告で置き換えた = 新しい報告のあるペインは走査せず、走査は mod の無い claude のアカウント + 既定だけ・ログインシェルなし）
- `claude_mod::monitor_plan`（報告 20 秒以内・会話 ID つき・名指しと一致）で worker_status / self / report が走査を省く（応答の `agents_scan`）。走査は台帳で範囲（会話 ID / ペインの子孫）に絞る `ScanScope`・走査先ごとのキャッシュ・実体の直接起動。self の走査を UI スレッドから先取りへ（#1984 の番犬の穴 = ㉔㉕）。GUI のカタログも報告の会話 ID で埋める。A/B `TAKO_2017_LEGACY=1`
- 実測: `scripts/test-agent-scan-mod-2017.sh`（隔離 GUI・4 アカウント × mod 入りの実 claude 4 本・監視相当）で走査 44 → 0 本/60 秒・回収済みの子の CPU 6.68 → 0.29 秒。15 回/分は 1 本のキャッシュの上限（60 / (5 + 所要)）を超えない = 行で数えた二重計上（本番の同じ窓で pid 40 / 行 80）。本番の修正後は着地後に計測

## 2026-10-13（#2020: sessions.yaml を走査ごとに全体書き直ししていたのを、last_seen_at だけの変化は溜めて検出集合の変化・他の項目の変化・600 秒・GUI の終了で書く形へ）
- `sessions::LastSeenThrottle`: 書く時は溜めた「最後に見えた時刻」ごと書くので、その時点の中身は修正前と同一（並び順・GC・#466 の逆引きは書かない間も不変）。`.bak` は last_seen_at 以外が変わったときだけ・同じ内容は書かない・GUI は会話 0 本の走査と `on_app_quit` でも書く。形式は不変・A/B `TAKO_2020_LEGACY=1`
- 実測: 本番（読むだけ）54 分で書き直し 121 回のうち、集合か last_seen_at 以外が変わったのは 10 回。`scripts/test-sessions-write-2020.sh`（隔離 GUI 2 本・偽 claude 24 本・500 件）の 30 分で新 7 回（`.bak` 3）/ 旧 58 回（`.bak` 58）・3.9 / 45.0 MB。注入 5 通りで新しい単体が file:line で落ちる

## 2026-10-13（#2046: 貼り付けに API キー・パスワードらしきものがあれば PTY へ流す前に「送る / やめる」を確かめる。既定 ON・旧い settings.json も ON）
- 判定は `tako_core::paste_guard::scan` の 1 実装（既知の形の表 14 行 + 秘密鍵・JWT・URL のパスワード → 代入の値 → 3 字種・短い語の割合・エントロピーの塊。除外表 13 行・先頭 64 KB・100 件で打ち切り）。関所は `paste_to_terminal` の 1 か所・Enter / Esc はやめる側・送るはボタンだけ。CLI `tako paste-guard` / MCP `tako_settings` action=paste_guard / 設定画面のトグル。A/B `TAKO_2046_LEGACY=1`・番犬 `issue2046_paste_guard_watchdog`
- 実測: `scripts/test-paste-guard-2046.sh` 20 PASS（visual-test `paste-guard` = やめると目印より前に 1 バイトも無い・送ると届く・off で素通し・ブラケットペーストも包んで届く・persist.log に本文なし。旧挙動は ② で FAILED）。実データ 9378 片で当たり 7（すべてテストの偽の鍵等）・ランダム 32 文字 97.6%・最適化ビルドで最悪寄り 0.5 ms

## 2026-10-13（#2031: 右パネル tasks の 2 秒ごとの一覧更新を UI スレッドから外し、置き場の版が同じなら読まない）
- dispatch の "list" と共有の 2 段: 配送の観測 `user_task_list_observe`（UI・I/O なし）→ 書き戻しと読み・解釈 `user_task_list_load` + `parse_list`（background。版 = 更新時刻・大きさ・inode を読む前に採り、同じなら前回の正本から組み直す・添付の実在は毎回）。2 秒ループ・タブ・「完了も表示」は `spawn_user_tasks_list`。A/B `TAKO_2031_LEGACY=1`・注入 `TAKO_2031_INJECT=slow:<ms>`・番犬 `issue1984_*` 9 節（注入 7 通り）
- 実測（読み 1500 ms の注入）: visual-test `tasks-tick` の打鍵の間隔の最大 3049 → 56 ms・`scripts/test-tasks-tick-2031.sh` 5 PASS（IPC 最大 2849 → 26 ms・測定区間の UI スレッドの読み 8 → 0 回）

## 2026-10-13（#1993: PC リソース管理 S1 = 機とプロセスの計測を背景で採り、`tako queue` / MCP `tako_queue` で読む）
- 境界 B29 `platform::resources`（sysctl / mach / libproc。mach tick は timebase で換算・他ユーザーの rusage は呼ばない）+ 純関数 `tako_core::resources`（Activity Monitor の式・分類・帰属・重い処理 = 重い祖先を持たない木）+ `tako_control::resources`（背景スレッド・`ipc::submit` が UI を通さず答える）。GUI 不在の CLI は機の値だけ。番犬 `issue1993_*`（+ #1968 / #1984）・カタログ +574 B・Windows は Pending（#2000）
- 実測: `scripts/test-resources-1993.sh` 10 項目緑（vm_stat の式と一致・器の子孫 / nohup の子の帰属）。Apple のプラットフォームバイナリは env を読めない（0 キー）。費用は走査 5 秒で +0.57% → 見ていない間 30 秒へ落として差がばらつきの中（0.85% 対 0.87%）

## 2026-10-13（#2011 S0: 描画と RAM の徹底的な軽量化の計測と計画 = 設計書 `.agent/plans/2026-10-lightweight.md` と計測スクリプト・子 Issue 7 本）
- 本番 835 MB はプレビュー（キャッシュ 173 MB + GPU のアトラス 約 150 MB）と描画（drawable 3 面 + 窓に依らない固定 約 150 MB）で過半。器のグリッドは 26 枚で 1.6 MB（履歴は tmux 側）。CPU の最大は監視の `claude agents --json` 60 本/分 = 子プロセス 24%（本体 18.7%）
- `scripts/measure-footprint-2011.sh`（同条件の footprint・アイドル CPU・描画回数・起動時間・電力）。#2009 と重なる分は実測を渡し、重ならない #2017〜#2023 を起票（S2 監視スキャン・S3 ペインログ・S4 malloc・S5 sessions.yaml・S6 MCP ブリッジ・S7 display link 提案・S8 番犬）
## 2026-10-13（#1691: エディタで対応する括弧を強調 = 文字列・コメントを飛ばす探索・VS Code の採り方・行描画の背景・応答の `document.bracket_match`）
- `tako_core::text_edit::brackets`（記号は `file_type` の `comment` 列 + 新しい `lexical` 列）・採り方は直後 → 直前・選択中と括弧に接しないときは採らない・種類ごとに数える。GUI は対の 2 文字に背景、CLI / MCP は編集系の応答の `document.bracket_match`（ツール増なし・カタログ +81 B）。A/B `TAKO_1691_LEGACY=1`
- 実測: release 5,000 行で 1 打鍵（insert + 探索）中央値 0.156 ms・最大 0.268 ms、10 MB の 1 行 0.369 ms（窓 1 MiB）。単体 20 本 + dispatch / MCP 1 本、注入 2 通り（探さない / 文字列を飛ばさない）で名指しで落ちる
## 2026-10-13（#1691: エディタで対応する括弧を強調 = 文字列・コメントを飛ばす探索・VS Code の採り方・応答の `document.bracket_match`）
- `text_edit::brackets`（記号は `file_type` の `comment` 列 + 新しい `lexical` 列）・直後 → 直前・選択中と括弧に接しないときは採らない・種類ごとに数える。GUI は対の 2 文字に背景、CLI / MCP は編集系の応答（ツール増なし・カタログ +81 B）。A/B `TAKO_1691_LEGACY=1`
- 実測: release 5,000 行で 1 打鍵中央値 0.156 ms・最大 0.268 ms。単体 22 本 + dispatch / MCP・注入 3 通り・visual `bracket-match` 新 OK / 旧 FAILED・実 CLI / MCP stdio の e2e
