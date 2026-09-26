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

## 2026-09-26（#1756: テーマの色の指定を文字単位で検査し、非 ASCII の値で GUI ごと落ちないようにした）
- `parse_hex_color` がバイト長 6 を確かめてから `&hex[0..2]` で切っており、`#赤色`（6 バイト）で GUI のメインスレッドが panic → abort（実測: CLI の `theme color` で Abort trap: 6・settings.json に残ると `did_finish_launching` で起動のたびに落ちる）。文字単位の検査 + `HexColorError`（空 / 16 進でない文字 / 桁違い）の `Result` へ替え、理由の文（日英）を dispatch（CLI / MCP / 設定画面）と起動時の persist.log が共有。`#+f+f+f`（`from_str_radix` の `+`）も弾く
- 実測: `scripts/test-theme-color-1756.sh` 修正前 8 PASS 17 FAIL → 修正後 31 PASS 0 FAIL・旧ロジック注入で新テスト 6 本が同じ panic で FAILED → 戻して緑・workspace 5469 passed 0 failed・clippy 3 宇宙 0

## 2026-09-26（#1748: autorename の偽 claude テストの間欠失敗を、パイプの受け継ぎと特定して直列化した）
- 疑いの ETXTBSY は macOS では起きない（実測）。真因は std が `Stdio::piped()` を `pipe()` → `FD_CLOEXEC` の 2 手で作る隙間に上限テストの spawn が重なり、30 秒眠る孫が兄弟の stdout の書き込み側を握ること → `READ_GRACE` 10 秒で `Failed`（lsof で相方 = 落ちたテストのプロセスを 3/3 件照合）
- 偽 claude を起こす 4 本を `fake_claude_lock()` で直列化・孫は pid を残させ `StopGrandchild` で止める（寿命 29.9 → 1.6 秒）・番犬 `issue1748_fake_claude_serial_watchdog`（注入 6 通りを名指し）・conventions に節
- 実測: autorename 28 件 × 4 本同時 × 250 周で修正前 3/1000 FAILED・漏れ 7 → 修正後 0/1000・漏れ 0（sleep 883 本）・workspace 5466 passed 0 failed・clippy 3 宇宙 0

## 2026-09-26（#1678: LSP 基盤（S1）を入れ、編集モードで言語サーバと握手して文書を同期するようにした）
- tako-core::lsp（UTF-8 ⇄ UTF-16 の入口 2 本・検出表 4 行・ルート検出・状態機械 8 状態・写しとの差分）+ tako-control::lsp（Content-Length の自作フレーミング・1 サーバ 3 スレッド・再起動 3 回まで・猶予 60 秒で停止・診断は開いている文書のぶんだけ）。`lsp-types` 0.97 を追加し tokio は 0 件のまま。操作は dispatch `LspServer` → CLI `tako lsp status/servers/restart/stop/logs` → MCP `tako_lsp_server`
- 実測: e2e `issue1678_lsp_e2e` 17 本（偽サーバの実プロセス。`TAKO_1007_LEGACY=1` で FAILED）・番犬 5 本（注入 2 通りが file:line で FAILED → 戻して緑）・隔離 GUI の `scripts/test-lsp-1678.sh`（アイドル 60 秒で受信 0 行・編集結果が 3 通りでバイト一致・kill -9 で孤児なし）・実 rust-analyzer 1.95.0 と握手（0.32 秒で稼働・能力 27 キー・診断 2 件。#1678 にコメント）

## 2026-09-26（#1745: stdio ブリッジからも非同期 run が使えるようにした）
- 実測で確定: 非同期 run は HTTP MCP のハンドラが受け口のチャネルを握って立てており、`tako mcp serve`（`McpSession.ipc_tx: None`）は spawn 前に JSON-RPC エラー（`sync=true` を指定）で返っていた。受け口へ渡す処理を `ipc::submit` の 1 実装へ寄せ、新しい `Request::OrchestratorRunStart` をそこで受ける（IPC 接続スレッド・HTTP・完了待ちスレッドが同じ道）。`ipc_tx` は廃止し、guide `spawning` と prompt を非同期の既定（run_id → run_status → run_result）へ直した
- 実測: `scripts/test-orchestrator-run-stdio-1745.sh` 修正前 10 PASS 9 FAIL（落ちたのは stdio の非同期 9 項目）→ 修正後全 PASS。注入 A/B 2 通り（受け口で受けない / エンジンを旧エラーへ戻す）が名指しで FAILED → 戻して緑

## 2026-09-26（#1741: 追従スクロールとページ移動の可視行数を描いた行の実寸から数える 1 実装へ寄せた）
- 追従は「器 − 上下余白 28px」÷ `theme.line_height`(17px)、Page は「器 − 14px」÷ 描いた行(21px) と別々に数えていた（修正前実測: 661px の器で実矩形 30 行を追従は 37 行・↓ で行 30〜36 のカーソルが画面外・Page Down は最下段に貼り付き）。`editor_scroll::visible_rows`（実寸を積む純関数）+ `preview_row_geometry` の 1 実装へ寄せ、追従・Page・IME の見積もりが使う。`viewport.visible_lines` を応答へ
- 実測: 新節 `scripts/test-viewport-lines-1741.sh`（正解は実矩形）が修正前 FAIL=5 → 修正後 PASS=5（8 / 13 / 32pt で 49 / 30 / 12 行が一致・余白 3 行）・番犬 4 規則に注入 7 通りすべて file:line で FAILED・#1649 / #1652 の実 GUI スクリプト緑・workspace 5470 passed 0 failed・clippy 3 宇宙 0

## 2026-09-26（#1749: PWA の e2e がホームへ書かないようにし、モックの版をビルドの版から取るようにした）
- 一時 HOME の実測で `npm run e2e` だけでホームへ PNG 80 枚（`~/Desktop/tako-28{4,5}-evidence/` 16 枚・`~/dev/tako-evidence/<番号>/` 64 枚）。スクショは `e2e/support.js` の `evidencePath()`（既定 = outputDir）へ、モックの版は vite と同じ `workspace-version.js` の `TAKO_VERSION` へ寄せた（旧 0.8.12 等で「表示が古い」バナーが写っていた）
- 実測: 109 passed・一時 HOME 配下 0 件（初回 / 2 回連続 / スクショ spec 単独）・番犬 `issue1749_pwa_e2e_output_watchdog` の注入 4 通りが file:line 名指しで FAILED → 戻して緑

## 2026-09-26（#1506: setup --review のプランの問いの既定を前回値にし、Enter で倍率が退行しないようにした）
- `--review` は前回値を引き継がない経路なので `prompt_plan` へ前回値が届かず、既定が「不明」固定 = Enter・非 TTY の EOF だけで `max-5x` → `max`（GPT / Google も `unknown` へ）。問いを `plan_question`（表示と「番号 → 値」の 1 か所）へ寄せ、既定は**標準 setup が前回値から選ぶ値と同じ規則**で引く（Max 検出時は max 系だけ・選択肢に無い値は値そのものを既定に見せる）。範囲外の番号は答えなかった扱い（旧は選択肢数を問わず 1〜7 を受け、Max の問いで `4` を打つと `max`）
- 実測: `scripts/test-setup-plan-keep-previous-1506.sh` **45 PASS 0 FAIL**（修正前ビルドでは `--review` 系の 11 項目だけ FAIL・`--yes` / MCP と同じ起動 / 初回は修正前から緑）・単体 5 本（15 通りで「Enter の結果 = 標準 setup」を照合）・注入 3 通りすべて file:line で FAILED → 戻して緑・pty-answer を共有する 1499 / 1501 / 1504 / 1509 全緑・workspace 5541 passed 0 failed・clippy 3 宇宙 0

## 2026-09-26（#1657: Code Runner の実行ペインを使い回し、内部マーカーを画面から消して終わりを案内とバッジで伝えた）
- 再生ボタン / `tako run` は同じタブの「同じファイル + 同じプロファイル」の実行ペインを**その位置で差し替える**（`PaneTree::replace` + 旧ペインは close と同じ後始末。実行中なら止めて再実行 = `stopped_running`。`--new-pane` / MCP `new_pane` で増やす）。終了コードは側路ファイル `run-exit/<pane>.code` で運び（画面の `__TAKO_EXIT=` は書けなかったときの退避路だけ）、画面には「[tako] 終了コード N / Enter で…」、タイトルバーに実行中 / 完了 / 失敗 (N) のバッジ。読む側は `run_pane_exit_code` の 1 実装（`--wait`・カードの実行記録 #1724・`list` の `run`・バッジ）
- 実測: `scripts/test-run-pane-reuse-1657.sh` **44 PASS 0 FAIL**（5 回で 1 枚・器つきの capture-pane にマーカー無し・A/B `TAKO_1657_LEGACY=1` で 5 枚 + マーカー・visual-test のバッジ色）・`test-remote-command-card-1724.sh` 55 PASS・注入 4 通りが file:line 名指しで FAILED → 戻して緑・workspace 5562 passed 0 failed・clippy 3 宇宙 0

## 2026-09-26（#1750: ⌘K パレット・Web のアドレスバー・dock の URL 欄の IME をターミナルから入力欄へ戻した）
- 修正前の実測（一時プローブ・隔離 GUI・実マウスの入口）: 3 つとも ASCII と ⌘V は欄に入る一方、変換は**ターミナルペインに束縛**（下線・候補窓はターミナルのカーソル位置）、確定はパレット / アドレスバーで PTY へ、unmark は 3 つとも PTY へ。Web ペインにフォーカスがあるとアドレスバーの確定は消え、git のコミット欄が残ったままパレットを開くと変換は裏のコミット欄へ入った
- `AppTextInput` に `Palette` / `WebAddress` / `WebDockUrl` を足し（パレット最優先 = `handle_key` と同じ順）、4 経路を 1 挿入関数・開く / 閉じるを 1 本ずつ（閉じる出口が変換を捨てる）・Web の 2 欄は `TextField` へ・見えない欄は奪わない・長い URL は `inline_input_window` で詰める
- 実測: セルフテスト項目 155 緑 / `TAKO_1750_LEGACY=1` で FAILED・番犬 `issue1750_palette_web_ime_watchdog`（注入 15 通り）+ 実ソース注入 3 通りが `main.rs:<line>` で名指し → 戻して緑・項目 154 緑・workspace 5798 passed 0 failed・clippy 3 宇宙 0。実 IME は `manual-checks.md`

## 2026-09-26（#1742: 上下移動の桁を表示幅で覚え、選択中の ←→ で畳み、macOS の ⌃A・⌃E・⌃K を足した）
- 桁の記憶を文字数 → 表示幅（全角 2・タブは 4 桁ごとのタブストップ。`unicode-width` は alacritty 経由で既にツリー内の版を直接依存へ）、選択中の素の ←→ は選択の端へ畳む、⌃A（桁 0）/ ⌃E / ⌃K は `editor_keys` の macOS 列だけ（Windows に置かない理由は表の行）。操作は足さず `tako edit move` / `delete`・MCP の既存の口で同じ結果
- 実測: `scripts/test-editor-keys-1652.sh` **37 PASS 0 FAIL**（tako-vd の実打鍵 15 相 + CLI / MCP の字面照合。bash 3.2 で通す）・注入 3 通りが単体と実 GUI で名指しの FAILED → 戻して緑・workspace 5644 passed 0 failed・clippy 3 宇宙 0。描画は全角 13px / タブ 28px で桁の規則（桁 4 = 31.3px）と 3〜5px ずれる = 描画側の別件
