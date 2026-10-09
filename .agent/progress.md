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

## 2026-10-09（#1922: 偽の言語サーバを起こす LSP の e2e が Windows で間欠的に落ちるのを、読み込みの終わりと manager が知った状態で揃えて直した）
- 真因 3 つを注入で確定: ①manager が `quiescent: false` を処理する前に送る（`READY_POLL` 1 周期。知らせ遅延 20ms 以上で 10/10・CI と同じ :214 / :221。Windows の probe では修正前 100 回中 10 回・10 回ともこの順序）②読み込みが要求より先に済む ③上限つきの要求を起動ごと測る（起動遅延 1.2 秒で 1680:329）。`tests/common/lsp_fake_e2e.rs`（`LoadingGate` = `--loading-until`・`wait_loading_known`・`wait_running`）へ 4 ファイルを寄せ、偽サーバは知らせを 50ms 遅らせて送る
- 実測: Windows CI で 4 本（37 テスト）× 20 周・修正後の形 100 + 50 回・起動 2.2 秒遅延の注入がすべて緑。番犬 `issue1922_lsp_loading_wait_watchdog`（注入 11 通りを file:line で名指し）

## 2026-10-09（#1915: CI の rust-cache のキーにルートの Cargo.toml の指紋を混ぜた）
- rust-cache v2.9.2 はメンバーの Cargo.toml と Cargo.lock だけをキーに混ぜ、ルートの仮想マニフェストは入らない（ログの「Lockfiles considered」でも無い）。`scripts/lib/cargo-root-manifest-key.sh`（[workspace.package] の version・行全体のコメント・空行・CRLF を除いた 8 桁）を ci.yml の macOS / Windows と release-windows.yml の `key` へ渡す。規約は conventions.md「CI のビルドキャッシュのキー」
- 実測: テスト 27 PASS（注入 8 通りを名指し・本物を正当に変えた 4 通りで偽の赤なし）・actionlint 0 件。CI ログのキー比較は PR のコメント

## 2026-10-09（#1908: ファイルツリーの ↑↓ / ←→ / Enter / ⇧⌘↑↓（Win は Shift+Ctrl+Home / End）・選択の CLI / MCP・残り時間の数え下ろし）
- 正本 `tree_select::on_key`（`RowShape` → `KeyOutcome`）を画面のキー（#1895 の ⇧↑↓ も）と CLI `tako tree selection [<path>] [--key K]` / MCP `tako_tree_folder` の `selection` が dispatch `TreeSelection` で通る（カタログ +412 B）。`eta` は最後にバイトが進んだ時点までの平均で数え下ろし、止まったら旧式の伸び方へ連続につなぐ。Shift+Delete は CLI / MCP に完全削除の口が無いので扱わない（FR-3.40 ③）
- 実測: 単体の合成（1 MiB / 300 ms・150 ms ごと）で逆戻り合計 5.87 → 0 秒・表記の戻り 3 → 0 回、実 GUI の `eta_secs` は戻り 6 → 0 回（57 回読み）。visual-test `tree-keys` 緑・`TAKO_1908_LEGACY=1` で ① が名指しで FAILED・番犬 10 本（注入 11 通り）

## 2026-10-09（#1916: GUI 内の塗りが遅いのは App Nap（E コア落ち）と比べた入力の違い = 塗りの間は activity を握る）
- 真因: 実行器の優先度ではない（GCD / 専用スレッド・QoS 0x15 / 0x19 で同じ）。同じ入力ならテストスレッドと GUI は 12.0 秒で同じ（「4〜8 倍」の 3.6 倍は入力の違い）、隔離 GUI は約 30 秒で App Nap に間引かれ E コアで 26.9〜33.4 秒（命令数は同じ）。`disable_app_nap`（#173）は `/proc` 前提で空振り。`platform::user_work::UserWork`（NSProcessInfo の UserInitiated activity を数で束ねる）を塗りの 2 経路が握る。`tako edit` の応答に `highlighting`
- 実測: `scripts/test-highlight-app-nap-1916.sh` 8 PASS（間引かれた後の比 1.02 / 1.11 / 1.10・P コア 0.99。A/B `TAKO_1916_LEGACY=1` は 2.56 / 2.54 / 2.80・P コア 0.000 で ③ が FAILED）・番犬 3 本（注入 10 通りを file:line で名指し）

## 2026-10-09（#1903: tako mod S2 の続き = ステータスバーの 5h / 7d も mod から・observed_at を値の変化時だけ・MCP 説明文・検査スクリプトの 1 実装化）
- mod の `stampLimits` が窓ごとに値が変わったときだけ `observed_at` を打つ（`$.state` に置きホットリロードをまたぐ）→ 束ね方を最新の観測へ（A/B `TAKO_1903_LEGACY=1` = #1880 の順）。ステータスバーは `status_bar_limits`（フォーカス順で最初に引けたペインのアカウント → 無ければ画面。取得元は `tako limit-service --refresh` の `claude.source`）。MCP 説明文 +527 B。`check-claude-mod.sh` に肯定形の文言の自己検査、1879 の段 0 はそれを呼ぶだけ
- 実測: `scripts/test-mod-limits-1903.sh`（visual-test `mod-limits` で 42 / 18 の帯・旧は ① FAILED / fake 17 PASS = 46 秒で画面へ戻る・放置 80% と動いている 20% で 20）・`claude plugin test` 28 本（旧の打ち方の注入で #1903 の 1 本が名指しで落ちる）・番犬の注入 12 通りを file:line で名指し

## 2026-10-09（#1924: `"$( … "{…,…}" … )"` を bash 3.2 が波括弧展開して語を割る形を直し、番犬を足した）
- 真因: 3.2 の `brace_gobbler` は `"` の中の `$(` を知らず `"` の偶奇だけで読む。`test-tree-keyboard-copy-1895.sh` の 2 箇所（`mcp_call "{\"op\":…,…}"` を `mcp_text "$( … )"` の引数に入れた形）を jq で組む形へ。番犬 `shell_scripts.rs` は全 `.sh` を語へ分け 3.2 の写しで読む（実の 3.2 / 5 の出力の差と 48,000 通り突き合わせ、食い違い 3 件はどれも検査の外の事情）
- 実測: `/bin/bash scripts/test-tree-keyboard-copy-1895.sh` 修正前 PASS=32 FAIL=7 → 後 39 / 0（bash 5 も 39 / 0。隔離 GUI・tako-vd）・棚卸し 118 本で 2 箇所 → 0・注入 6 通りを file:line で名指し。`local` / 配列 / `[[ ]]` / case 等の表は `.agent/conventions.md` の #1924 節

## 2026-10-09（#1909: 補完の打鍵と説明の補いの取り消しの番号も UI スレッドで先に取る = ホバーの #1893 と同じ口へ）
- `reserve_completion` / `reserve_resolve` → 要求の `ticket`。manager の背景は補完・説明・ホバーとも `enter_lane` の 1 口から列へ入り、`supersede(` は UI の口と `enter_lane` だけ（番犬が他を名指し）。残りは `tako lsp status` / MCP の `inflight`。A/B `TAKO_1909_LEGACY=1`・注入 `TAKO_1909_INJECT_HOLD`（背景の走り出しを合図まで止める）
- 実測: visual-test `completion-cancel` で旧い形は閉じた後の背景がサーバへ届き `inflight=1` / `pending_requests=1`（③ で名指しの FAILED）、直した形は 0 / 0・問い合わせ 0。e2e 6 本（背景で取り直す注入で 5 本が落ちる）・番犬の注入 11 通りを file:line で名指し・`scripts/test-lsp-completion-cancel-1909.sh` 4 PASS 0 FAIL（2 回）

## 2026-10-09（#1917: TS の構文の塗りを release の 1 MB で 5.9 → 1.8 秒にした = 正規表現へ「当たらない行で VM を起こさない」等価な前置き）
- 真因: two-face の TS は先読み・後読みだらけで、fancy-regex は行の全バイト位置で VM を回す（正規表現 365 本に均等に散る・1 行 1 回ほぼ当たらない）。`syntax_prefilter.rs` が TS / TSX の 425 本を `\G(?=(?s:.)*?(?:必要条件))(?s:.)*?\K(?:元)` へ書き換え（構文セットの直列化を 2 構文だけ解いて詰め直す・往復検査・プロセスで 1 回 34 ms を起動時に別スレッドで）。A/B `TAKO_1917_LEGACY=1`
- 実測（release・交互 3 回）: 1 MB の TS 5.90 → 1.73〜1.83 秒・TSX 6.1 → 1.7 秒・TS / Rust 4.6 → 1.4 倍・Rust は不変・40 行の冷えた 1 回目だけ 82 → 100 ms。塗りの全記録が実在の TS 12 本で 1 行残らず一致。単体 9 本（バックトラック回数の番犬）・番犬 4 本

## 2026-10-09（#1913: 合成入力欄の組み立てを tako-core の `synthetic_input` へ寄せ、描く側 4 か所と #754 のテストが同じ形を使う）
- 描く側（visual-test の #719 / #718・セルフテストの #737 / #1067）は `chat_g3_command` / `autogrow_command` / `gui_input_paint` / `restart_tui_paint`、dialog のテストは同じ `*_lines` を呼ぶ（描くバイトは必ず行から作る）。寄せる前の原文を切り出した比較で 13 状態がバイト一致
- 実測: #754 の注入 A / A+B / E で同じ形を名指しして FAILED・番犬 `issue1913_synthetic_input_watchdog` 4 本（全戻し・1 か所戻し・テストへの手書き 1 行を file:line で名指し）

## 2026-10-09（#1940: spawn の起動コマンドが 2〜3 行のペインで化ける・届く前に delivered と言う・起動失敗が黙る を直した）
- 真因（実測）: worker ペインが 2〜3 行だと zsh は入力行を `<…` / `>....` に畳み全文一致が成立しない → `shell_send` が書き直し 10 回（45〜47 秒）の末に**消していない行へ**本文 + Enter を書き足し `autoexport` + 正しい引数で 2 回目の起動。会話の検出（起動直後）を到達とみなして delivered → 諦めた後に undelivered。claude は 2 行で空白・器の tmux も同寸法（capture でも読めない）
- 修正: 寸法で全文を出せないなら書き直さず Enter 1 回 + OSC 133;C で実行確認・書き切り前に必ず Ctrl+C・新しいペインは最初のプロンプトの印を待つ・起動フック中の先行入力を区別 / delivered は送達フローの確認だけ / 起動直後の終了を `agent_exited` で即決着し `workers` の `launch=failed` / 8 行未満で入力欄が無ければ peer だけ（`pane_too_short`）。A/B `TAKO_1940_LEGACY=1`・カタログ +280 B
- 実測: `scripts/test-spawn-launch-1940.sh`（隔離 GUI・86×2 行・direnv 3 秒）旧 = `autoexport` + 2 回起動 / 新 = 1 回・正しい引数、失敗は 7 秒で agent_exited（12 PASS）。e2e 8 条件・番犬 4 本（注入 10 通りを file:line で名指し）

## 2026-10-09（#1926: #173 の disable_app_nap は /proc 前提で一度も効いていなかった = 消して、利用者が待つ読み込みだけ App Nap を止める）
- 交互 2 周の実測で (2) を選択: 寿命の間止めるとアイドル 341〜365 → 856〜1,154 µW・裏の出力処理 2〜5 倍の電力、得をするのは待たれる重い処理だけ。エージェント稼働中は #173 のアサーションで App Nap の対象外（優先度 28）。`UserWork::begin_load` を PDF のラスタライズ（開く / ズーム）・Markdown の組み立て / 描き直しへ（A/B `TAKO_1926_LEGACY=1`）
- 実測: PDF 117 ページ 7.2〜8.0 → 3.0 秒・ズーム 49.9 → 20.8 秒。`scripts/test-preview-load-app-nap-1926.sh` 7 PASS（旧の腕で ①② が名指しで FAILED）・番犬 `issue1926_app_nap_watchdog` 4 本（注入 12 通りを file:line で名指し）
