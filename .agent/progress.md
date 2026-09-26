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

## 2026-09-26（#1726: Code Runner の実行設定と Python の実行環境の設計書を出した）
- `.agent/plans/2026-09-runner-settings.md`: 棚卸し（file:line）/ `run-configs.json` 新設（ローカル区分）/ 実行環境は ID で保存し実行時に解く / 自動選択 uv → `.venv` → poetry → pipenv → conda → pyenv → システム / env は境界 B1 でコマンドへ埋める / MCP は既存 2 本へ足す
- スライス S0〜S7（#1728〜#1735）と判断待ち J1〜J5（全部推奨どおりで確定）。着地順は #1656 → S2 → #1657 → #1662 → S3〜S6

## 2026-09-26（#1724: スマホからコマンドカードを実行できるようにした）
- PC のカードには「実行済み」の状態が無かったので、tako-core のカードへコマンドごとの実行記録（実行ペイン / running・exited・closed / 回数）と「実行中の同じコマンドは再実行しない」を足し、`list` の `runs` で返す。PC のカードは同じ記録を出し、確定は `dispatch::refresh_command_card_runs` の 1 本
- remote は宣言表 `remote_cards::CARD_ROUTES` の 2 本だけ（一覧 = Observe / 実行 = Interact・本文は受け取らない）で `ShowCommand` の list / run を素通し。PWA はペイン画面にカード + 全文の確認 1 回、observe には #1452 の権限リクエスト
- 実測: 実経路 `scripts/test-remote-command-card-1724.sh` 55 PASS 0 FAIL・e2e 新 spec 8 本（全体 109 passed）・番犬の注入 4 通りが file:line 名指しで FAILED → 戻して緑

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
