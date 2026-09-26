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

## 2026-09-26（#1709: 配布物へライセンス本文と第三者の著作権表示を同梱した）
- 法務監査（tako-legal）の即時修正分。`cargo about` で配布対象 547 クレートの本文・著作権表示を `THIRD-PARTY-LICENSES.md` へ生成（`about.toml` / `about.hbs`）、NOTICES に Zed のファイルアイコン（GPL + Lucide の ISC）と PWA の preact / marked / DOMPurify / Geist を追記、`build-app.sh` が 3 ファイルを `.app` の Resources へ署名前に置く
- `cargo deny check licenses` 0 errors・docs build / og:verify / verify-links 緑・telemetry.md の食い違い 3 点を訂正。`.app` の実ビルドはマシン負荷のため未実施（同梱は script を読んで判定）。Windows の同梱とアプリ内表示は提案止まり

## 2026-09-25（#1708: ドキュメントサイトの配信設定を点検した）
- `docs/public/_headers` を新設（HSTS・frame-ancestors / X-Frame-Options・CSP の基本指令・Permissions-Policy）。CSP はスクリプトを縛らない（Pagefind の wasm と GA・同意バナーの送信先を漏れなく許可しないと検索・計測が黙って止まるため）。旧ドメイン転送はスキーム・ポートを固定し FQDN の末尾ドットも拾う形に
- 再発防止に `docs/scripts/test-middleware.mjs`（25 ケース）と `verify-headers.mjs`（dist / 実 URL）を CI の docs 節へ。修正前の版へ当てると前者 5 件・後者 36 件で FAILED → 修正後は緑。wrangler pages dev + Playwright で検索・GA・同意の読み込みが壊れず、別オリジンの iframe だけ拒否されることを確認
- 依存は `npm audit fix` の範囲（12 → 6 件）。残りは astro 7 / starlight 0.42 / sharp 0.35 のメジャー更新が要る（静的出力で外部から届く経路は無い）

## 2026-09-23（#1579: UI の印をグリフから描画プリミティブへ）
- `×` U+00D7 を 8 件（drawer 2 / right_panel 4 / preview_render 1 / main 1）→ `svg().path(ui_icon::CLOSE)`、`⎇ tmux` → 語だけの `tmux`（U+2387 は多くのフォントに無く豆腐）、`● LIVE` 2 件 → `div().rounded_full()` の丸 + `LIVE`（`live_badge` の 1 実装）。新設 SVG はゼロ（CLOSE 流用 + 図形）
- 判定を `tako_core::emoji::is_icon_glyph`（表は明示。`▾`/`▸` は診断メッセージで実使用があるのでブロックで採らない）、番犬 `issue1579_ui_glyph_icon_watchdog` は**画面へ出る 2 経路だけ**（子要素シンク `CHILD_SINKS` = `.child(` / `.children(` の文字列 / `ui_text` カタログ）を見るので**許可リスト 0 件**。実ファイル注入 10 通りすべて file:line 名指しで FAILED → 戻して緑
- 実測: `bash scripts/test-glyph-icon-1579.sh` 5 PASS 0 FAIL（tako-vd 上・`live_dot` 82 色 / `close_icon` 9 色）・`ui_asset!("close")` を外す A/B で `close_icon` が **9 → 1 色**（`live_dot` は 82 色のまま）・workspace 5397 passed 0 failed・clippy 3 宇宙 0

## 2026-09-26（#1697: 名前が読めない瞬間に検証用 GUI がユーザーの画面へ落ちないようにした）
- `ensure` が面の uuid を記録し、名前が読めない面（`name=?`）はその uuid で当てる。`TAKO_DISPLAY` 明示を見失った検証用 GUI は開かずに終了 4（`isolated-gui.sh` は配線済みの機でだけ明示）
- 実測: A/B `TAKO_1697_LEGACY=1` と注入 5 通りが file:line 名指しで FAILED → 戻して緑・tako-vd 実経路 18 PASS・workspace 全 ok

## 2026-09-24（#1628 / #1707: 子の終了時に PTY を読み切るようにし、待ちを観測事象で段に切った）
- 待ちの段分け（`ARGC=` を起点に `PROBE-DONE` を待つ / 上限は `state_wait_budget` 由来 / 失敗メッセージに経過・段・子の生死・混み具合）を入れたら、**Issue の前提が誤りだと分かった**: 起動は 0.2 秒で終わっていて、遅いのではなく**子の終了後に末尾が読めていない**（画面は `ARGC=1` だけ）
- 真因は製品側。自前の `PtyLoop` は子の終了で `break 'event_loop` するが、upstream alacritty がそこに持つ `drain_on_exit` の段が **#817 の移植で落ちていた**（`..Options::default()` の既定 `false` を「未使用」と読んだ）。以後 PTY を読む者はいないので末尾は永久に失われる。**知らせる前に読み切る**段を入れた（読み手は両 OS とも非ブロッキング。1 回の `pty_read` は 64 KiB で切り上げるので繰り返しが要る）。契約「終了を知った時点で出力は全部画面にある」は `architecture.md` の `pty_loop` 節へ明記（真因は #1707 として独立起票）
- 網は 1 度作り直した: 大量出力は**背圧**で子が tako を追い越せず再現しない。真の機構は mio の 1 束での順序競合なので、実測の形（短い出力 → 即終了）を 30 回繰り返す形へ。**修正後は毎回緑**だが逆向き（外すと落ちる）は順序次第で保証できないため、旧経路 A/B は `#[ignore]` で CI から外し Windows 実機の手順として残した。CI の Windows は `pty_exit_drain` 1 passed / 1 ignored・`spawn_arg_quoting` 0.33 秒（main と同値）

## 2026-09-24（#1649: 編集カーソルを可視範囲へ追わせ、IME の下線も消えないようにした）
- `ListState` への `scroll_to(cursor)` が **0 件**で、打鍵 / 矢印 / ⌘F のヒット / undo / 貼り付けのどれでもカーソルが画面外のままだった。判断は `tako_core::editor_scroll`（`LineViewport` + `follow_cursor`・上下 3 行の余白・視野が狭いと詰める・端では可視化を優先）へ寄せ、UI は器の寸法を測って渡すだけにした。呼ぶのは `refresh_preview_from_editor`（編集の全経路の要）+ 検索ヒット + IME の変換開始の 3 か所、器は仮想リストと div スクロールの両方を**論理位置**で動かす
- 連鎖症状の IME 下線は `preview_pending_cursor_origin` が「可視化後にカーソル行が来る位置」を出して**本文の中にアンカーを残す**（行のレイアウトは paint でしか控えられないので、追従の直後 1 フレームは必ず無い）。効きは編集系の応答に乗る `viewport` で GUI の外から読める（位置自体は #1658 の `document.cursor`）
- 実測: **tako-vd 上の visual-test 節 `cursor-follow`（7 相）が新 = 全相 `ok=true` / `anchored=true`・旧（`TAKO_1649_LEGACY=1`）= 全相 `ok=false` / `anchored=false`**（視野 37 行 / 4,002 行 / PASS=10 FAIL=0）。番犬 6 本へ注入 9 通りすべて file:line 名指しで FAILED → 戻して緑・`editor_scroll` 単体 11 本 + `preview_render` 単体 2 本・workspace 0 failed・clippy 3 宇宙 0

## 2026-09-26（#1652: 修飾キー付きの打鍵が入口で全部捨てられていたのを直した）
- 入口の `if platform || control || alt { return false }` を外し、打鍵の意味を `platform::editor_keys` の 1 枚の表（両 OS の列）へ。`TextBuffer` に単語 / 行 / ページ / 文書端の移動と語・行単位の削除・smart Home・桁の記憶（desired column）を足し、`PreviewMove` / `PreviewDelete` + `tako edit move|delete` + MCP 2 本が打鍵と同じ口を通る
- 実測: `scripts/test-editor-keys-1652.sh` で tako-vd 上の実打鍵経路 9 相が緑・A/B `TAKO_1652_LEGACY=1` は ⌘↑ の相で FAILED・注入 7 通りがすべて file:line 名指しで FAILED → 戻して緑

## 2026-09-26（#1711: MCP ツールカタログの説明文を短くし、LSP 用に 16 KiB の余白を作った）
- `tools/list` が予算 204,800 B の残り 1.8 KB だったので、156 本中 47 本の description / 引数説明から根拠・仕組み・経緯・重複（`clear_*` 13 本・「省略で現状維持」・schema と同じ値）を外し **188,323 B**（main 116a63a の 204,579 B から -16,256 B）。ツール名・順序・型・必須は機械照合で不変、外した原文は `.agent/mcp-catalog-notes.md`
- 実測: 隔離 GUI + `tako mcp serve` の tools/list と `tako context-budget` が一致・violations 0・workspace 5463 passed 0 failed・clippy 3 宇宙 0。初回は面の名前が読めず「メイン画面へ開いた」警告が出たが、uuid 照合で窓は tako-vd 上だった（#1697 の症状）

## 2026-09-26（#1678: LSP 基盤（S1）を入れ、編集モードで言語サーバと握手して文書を同期するようにした）
- tako-core::lsp（UTF-8 ⇄ UTF-16 の入口 2 本・検出表 4 行・ルート検出・状態機械 8 状態・写しとの差分）+ tako-control::lsp（Content-Length の自作フレーミング・1 サーバ 3 スレッド・再起動 3 回まで・猶予 60 秒で停止・診断は開いている文書のぶんだけ）。`lsp-types` 0.97 を追加し tokio は 0 件のまま。操作は dispatch `LspServer` → CLI `tako lsp status/servers/restart/stop/logs` → MCP `tako_lsp_server`（157 ツール）
- 実測: e2e `issue1678_lsp_e2e` 15 本（偽サーバの実プロセス。`TAKO_1007_LEGACY=1` で 13 本 FAILED）・番犬 5 本（注入 2 通りが file:line で FAILED → 戻して緑）・隔離 GUI の `scripts/test-lsp-1678.sh`（アイドル 60 秒で受信 0 行・編集結果が 3 通りでバイト一致・kill -9 で孤児なし）・実 rust-analyzer 1.95.0 と握手（0.32 秒で稼働・能力 27 キー・診断 2 件。#1678 にコメント）

## 2026-09-26（#1726: Code Runner の実行設定と Python の実行環境の設計書を出した）
- `.agent/plans/2026-09-runner-settings.md`: 棚卸し（file:line）/ `run-configs.json` 新設（ローカル区分）/ 実行環境は ID で保存し実行時に解く / 自動選択 uv → `.venv` → poetry → pipenv → conda → pyenv → システム / env は境界 B1 でコマンドへ埋める / MCP は既存 2 本へ足す
- スライス S0〜S7（#1728〜#1735）と判断待ち J1〜J5（全部推奨どおりで確定）。着地順は #1656 → S2 → #1657 → #1662 → S3〜S6

## 2026-09-26（#1724: スマホからコマンドカードを実行できるようにした）
- PC のカードには「実行済み」の状態が無かったので、tako-core のカードへコマンドごとの実行記録（実行ペイン / running・exited・closed / 回数）と「実行中の同じコマンドは再実行しない」を足し、`list` の `runs` で返す。PC のカードは同じ記録を出し、確定は `dispatch::refresh_command_card_runs` の 1 本
- remote は宣言表 `remote_cards::CARD_ROUTES` の 2 本だけ（一覧 = Observe / 実行 = Interact・本文は受け取らない）で `ShowCommand` の list / run を素通し。PWA はペイン画面にカード + 全文の確認 1 回、observe には #1452 の権限リクエスト
- 実測: 実経路 `scripts/test-remote-command-card-1724.sh` 55 PASS 0 FAIL・e2e 新 spec 8 本（全体 109 passed）・番犬の注入 4 通りが file:line 名指しで FAILED → 戻して緑
## 2026-09-23（#1645: rebase で復活した作業ログのエントリを番犬で止めた）
- `context-budget fix` の移送（古いエントリを**消す**）と main 側の移送が rebase で噛み合っても git は衝突を報告せず auto-merge するので、archive 済みのエントリが `progress.md` へ黙って戻る（9/23 だけで 4 回・毎回 worker の目視で発見。予算を超えなければ既存の番犬では落ちない）。判定を `tako_core::context_budget::revivals` の 1 実装として足し、`tako context-budget` / MCP の violations と新設の番犬が同じものを通る
- 鍵は `LogEntry::archive_line()` = `fix` がアーカイブへ書くのと同じ 1 行。(日付, Issue 番号) の組は**実データで誤検出する**（アーカイブに `2026-09-14 #1450` が 3 件・`2026-07-05 #63` が 2 件）ので採らなかった
- 実測: 注入 3 通り（復活 / 重複 / 見出しの書式を壊す空振り検査）すべて file:line 名指しで FAILED → 戻して緑。**この PR 自身の rebase でも 2 件の復活を file:line で捕らえた**

## 2026-09-26（#1656: Code Runner がプロジェクトを見て cargo run / npm run / python -m 等で走るようにした）
- 解決器がファイル 1 枚しか見ず、cargo プロジェクトの `.rs` を `rustc` 単体で走らせて必ず失敗していた。上へ辿る範囲の 1 実装 `tako_core::project_root`（HOME の段は見ない / `.git` で止まる / 深さ 32。`detect` は #1726 の実行設定のキー・LSP #1678 と共有）と種別の表 `runner_project::KINDS`（cargo / go / npm / python / dotnet / make = 行追加で増える）を新設し、入口を `runner::resolve_file` の 1 本へ。優先は上書き → 宣言 → ユーザーの拡張子既定 → プロジェクト既定 → 組み込み。Issue の例（bin の無い lib crate）は `cargo run` が必ず落ちるので `cargo test -p <pkg> --lib <mod>::`
- 実測: `scripts/test-runner-project-1656.sh` **26 PASS 0 FAIL**（隔離 GUI で CLI `--dry-run` と MCP `tako_run_resolve` が字面一致・実際に `cargo run` が exit 0・HOME 直下の置き忘れを拾わない・`TAKO_1656_LEGACY=1` で `rustc` 単体へ戻る）・注入 6 通りすべて file:line 名指しで FAILED → 戻して緑・workspace 5465 passed 0 failed・clippy 3 宇宙 0

## 2026-09-26（#1725: ツリーのインライン入力の IME をターミナルから入力欄へ戻した）
- 実測で真因を確定: ASCII 打鍵は 9/9 で入力欄に入る一方、IME の変換は 9/9 で**隣のターミナルペインに束縛**（`app_text_input` がインライン編集を宛先から外していた）。GPUI はかなモードの印字キーを IME へ先に渡すので、下線・候補窓はターミナルのカーソルに出て、unmark の文字列は PTY へ流れていた。外側クリックでは閉じなかった
- `AppTextInput::TreeName` を足し、打鍵・⌘V・確定・unmark の 4 経路を `tree_name_insert` の 1 関数へ、状態を `TextField` へ、開く / 閉じるを 1 本ずつへ。未確定文字列とキャレットは共有部品で入力欄の中に描き、長い名前は `…` で詰める。Esc / 確定で元のペインへ戻り、`on_mouse_down_out` で閉じ、見えない入力欄は打鍵を奪わない
- 実測: セルフテスト項目 154（実マウスの入口 × 36 回・出力が流れている最中・外からの focus 移動）緑 / `TAKO_1725_LEGACY=1` で 1 回目から FAILED・番犬の注入 13 通り + 実ソースの逆戻りが `main.rs:13770` で名指し → 戻して緑

## 2026-09-26（#1728: 実行コマンドの切り詰めを文字境界へ寄せ、プレビュー描画の panic を直した）
- Code Runner のツールチップ（`&cmd[..60]`）と実行メニュー（`&plan.command[..40]`）がバイト位置で切っていて、日本語のファイル名で描画中に panic していた。既存の文字数ベース `crate::truncate` へ寄せ、同じファイルの検索欄（`&text[..cursor]`。`tako preview-search` がクエリだけ差し替えるとカーソルが文字の途中に残る）も `floor_char_boundary` で丸めてから分ける
- 実測: 修正前は単体 8 件 FAILED（7 件が `is not a char boundary`）→ 修正後 9 件 ok・番犬 `issue1728_byte_truncate_watchdog` は実ファイルへの注入 3 通りを file:line で名指し → 戻して緑・visual 節 `run-command-truncate` で修正前ビルドは例 1 を開いた時点で `preview_render.rs:2635` の panic（exit 134）、修正後は 3 か所を同じフレームで描いて完走・workspace 5424 passed（落ちた予算テスト 1 件は追記途中の本ファイルを読んだもので、移送後に単独で 13 ok）

## 2026-09-26（#1729: Code Runner の実行設定の型と Python の実行環境の検出を tako-core に置いた）
- `runner_config`（RunConfig / RuntimeRef / merge = file > project > 宣言 > 自動）と `runtime_env`（表 `KINDS` + 戦略 6 種 + `FsProbe`）を新設。Python は uv → venv → poetry → pipenv → conda → pyenv → システムの順で、Windows の列まで macOS の単体で固定。検出は stat と先頭読みだけで、辿る範囲は引数（#1656 の candidate_dirs に任せる）
- 番犬 `issue1729_runtime_env_table_watchdog`（表の語彙を表から集めて表の外の直書きを名指し・属性の OS 分岐・子プロセス）。注入 6 通りすべて FAILED → 戻して緑。Windows CI で区切り混在の除外漏れも露出 → 修正

## 2026-09-26（#1654: Tab / ⇧Tab でインデントし、Enter でインデントを引き継ぐようにした）
- 打鍵表に Tab の行（⇧ で浅く）を足し、`TextBuffer` に `indent` / `outdent` / `newline_and_indent` と既存行からのインデント推定（開いたとき 1 回）を追加。開き括弧の直後は 1 段深く（Python / YAML は `:` も・Markdown は継承だけ）、括弧の自動閉じは理由を書いて対象外。`PreviewEditCommand` + `tako edit indent|outdent|newline` + MCP は `tako_preview_edit` の `command`（ツールは増やさない）
- 実測: tako-vd の実打鍵経路 12 相が緑（Enter を素の改行へ戻す注入で E8 の症状が再現して FAILED）・注入 7 通りが名指しで FAILED → 戻して緑

## 2026-09-26（#1744: 隔離 GUI は面を用意できなければ起動しないようにした）
- `launch_isolated_gui` は `ensure` 失敗でも「起動は続ける」で素通しする作りで、uuid の記録が無い機では面の指定を持たない起動が tako の暗黙の既定でユーザーの画面へ落ちうる潜在経路があった（実例は未確認。9/26 の報告は #1697 の誤警告）。`ensure` 失敗・成功でも `bounds` で一覧に無いときは起動せず終了コード 4 + stderr へ「未実測: …」1 行。呼び手は `|| exit $?`、1505 は C/D だけ未実測で続行
- 番犬（#1490 の番犬を拡張）: ヘルパを `/bin/bash` で走らせ「用意できない面」5 通りを注入して偽 GUI が起きないこと + 呼び手の失敗の拾い方。注入 5 通りすべて file:line 名指しで FAILED → 戻して緑。実呼び手 1676 は面なしで exit 4・通常経路 26 PASS（persist.log `ディスプレイ指定 tako-vd: name で解決`）

## 2026-09-26（#1746: tako task gate の証拠をバイト位置で切らず文字境界で切るようにした）
- `print_gate_result` が証拠を `&ev[..120]` で切っていて、`exit 0; stdout: ` + 日本語だと 120 バイト目が「あ」の途中に当たり `gate check` / `show` が exit 101 で落ちた（修正前ビルドで実測。`check` は保存後に落ちる）。表示の切り詰めを `tako_core::text::truncate_chars`（文字数・`…` 込み 120 文字）へ寄せ、`sessions resume` の session id の先頭 8 バイトも文字単位へ
- 棚卸し: tako-cli の本番の範囲添字 5 件（危険 2 件を直し、安全 3 件へ `切り出し安全:` の理由コメント）/ tako-control 85 件は表示の切り詰めが丸め済みの 1 件だけ。番犬 `issue1746_cli_byte_slice_watchdog` は注入 2 通りを file:line で名指し → 戻して緑

## 2026-09-26（#1756: テーマの色の指定を文字単位で検査し、非 ASCII の値で GUI ごと落ちないようにした）
- `parse_hex_color` がバイト長 6 を確かめてから `&hex[0..2]` で切っており、`#赤色`（6 バイト）で GUI のメインスレッドが panic → abort（実測: CLI の `theme color` で Abort trap: 6・settings.json に残ると `did_finish_launching` で起動のたびに落ちる）。文字単位の検査 + `HexColorError`（空 / 16 進でない文字 / 桁違い）の `Result` へ替え、理由の文（日英）を dispatch（CLI / MCP / 設定画面）と起動時の persist.log が共有。`#+f+f+f`（`from_str_radix` の `+`）も弾く
- 実測: `scripts/test-theme-color-1756.sh` 修正前 8 PASS 17 FAIL → 修正後 31 PASS 0 FAIL・旧ロジック注入で新テスト 6 本が同じ panic で FAILED → 戻して緑・workspace 5469 passed 0 failed・clippy 3 宇宙 0

## 2026-09-26（#1748: autorename の偽 claude テストの間欠失敗を、パイプの受け継ぎと特定して直列化した）
- 疑いの ETXTBSY は macOS では起きない（実測）。真因は std が `Stdio::piped()` を `pipe()` → `FD_CLOEXEC` の 2 手で作る隙間に上限テストの spawn が重なり、30 秒眠る孫が兄弟の stdout の書き込み側を握ること → `READ_GRACE` 10 秒で `Failed`（lsof で相方 = 落ちたテストのプロセスを 3/3 件照合）
- 偽 claude を起こす 4 本を `fake_claude_lock()` で直列化・孫は pid を残させ `StopGrandchild` で止める（寿命 29.9 → 1.6 秒）・番犬 `issue1748_fake_claude_serial_watchdog`（注入 6 通りを名指し）・conventions に節
- 実測: autorename 28 件 × 4 本同時 × 250 周で修正前 3/1000 FAILED・漏れ 7 → 修正後 0/1000・漏れ 0（sleep 883 本）・workspace 5466 passed 0 failed・clippy 3 宇宙 0
## 2026-09-26（#1711: MCP ツールカタログの説明文を短くし、LSP 用に 16 KiB の余白を作った）
- `tools/list` が予算 204,800 B の残り 1.8 KB だったので、156 本中 47 本の description / 引数説明から根拠・仕組み・経緯・重複（`clear_*` 13 本・「省略で現状維持」・schema と同じ値）を外し **188,323 B**（main 116a63a の 204,579 B から -16,256 B）。ツール名・順序・型・必須は機械照合で不変、外した原文は `.agent/mcp-catalog-notes.md`
- 実測: 隔離 GUI + `tako mcp serve` の tools/list と `tako context-budget` が一致・violations 0・workspace 5463 passed 0 failed・clippy 3 宇宙 0。初回は面の名前が読めず「メイン画面へ開いた」警告が出たが、uuid 照合で窓は tako-vd 上だった（#1697 の症状）

## 2026-09-26（#1678: LSP 基盤（S1）を入れ、編集モードで言語サーバと握手して文書を同期するようにした）
- tako-core::lsp（UTF-8 ⇄ UTF-16 の入口 2 本・検出表 4 行・ルート検出・状態機械 8 状態・写しとの差分）+ tako-control::lsp（Content-Length の自作フレーミング・1 サーバ 3 スレッド・再起動 3 回まで・猶予 60 秒で停止・診断は開いている文書のぶんだけ）。`lsp-types` 0.97 を追加し tokio は 0 件のまま。操作は dispatch `LspServer` → CLI `tako lsp status/servers/restart/stop/logs` → MCP `tako_lsp_server`（157 ツール）
- 実測: e2e `issue1678_lsp_e2e` 15 本（偽サーバの実プロセス。`TAKO_1007_LEGACY=1` で 13 本 FAILED）・番犬 5 本（注入 2 通りが file:line で FAILED → 戻して緑）・隔離 GUI の `scripts/test-lsp-1678.sh`（アイドル 60 秒で受信 0 行・編集結果が 3 通りでバイト一致・kill -9 で孤児なし）・実 rust-analyzer 1.95.0 と握手（0.32 秒で稼働・能力 27 キー・診断 2 件。#1678 にコメント）
