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

## 2026-10-09（#1893: LSP ホバーの続き = 右クリックの項目・⇧⌘H・CLI / MCP の全文の口・読み込み中のマウス）
- 右クリックに「ホバー情報を表示」（`MenuItem::Hover`。移動 → ホバー → 整形・`hoverProvider` の申告で出し分け）・キー ⇧⌘H / Ctrl+Shift+H（⌘K ⌘I はパレットの発火が遅れるので不採用）を編集メニューと同じ `request_lsp_hover` へ。manager は全文を返し、カードは 16,000 字・CLI `--full` / `--limit N` = MCP `limit`（0 = 全文。カタログ +60 B）。マウスも #1869 の `wait_loaded` で待ち、語の真下に「読み込み中」→ カード。取り消しの番号は UI で先に取る（`reserve_hover`。背景で取ると取り消しを追い越して待ち続ける）
- 実測: `scripts/test-lsp-hover-1893.sh`（visual-test `hover-1893` / `hover-loading` / `hover-loading-real`・A/B `TAKO_1893_LEGACY=1` で名指しの FAILED・#1684 / #1681 の節の回帰・CLI / MCP 字面一致）・実の rust-analyzer は暖機なしで 0.08 秒で「読み込み中」→ 2.2 秒でカード（旧は 95 秒出ない）・e2e 7 本・番犬の注入 12 通りを file:line で名指し・右クリック前のマウスのホバーが明示のカードを消していたのを直した

## 2026-10-09（#1892: tako mod の validate / test を夜間リリースの前段で毎晩回し、落ちたら通知する）
- 本体 `scripts/check-claude-mod.sh`（origin/main の mod を一時 dir へ取り出し、使い捨ての設定 dir で `claude plugin validate --strict` / `test`。各段 60 秒の上限でプロセスグループごと打ち切り・`.catch` 抜けと 0 本も不合格・claude が無ければ未実測 = exit 3）。`nightly-release.sh` はロック直後に毎晩呼び、**結果でリリースを止めない**（ERROR + 既存の通知）。`~/.claude-orchestrator/state/tako-mod-check` に検査した claude の版と前回合格の版・mod の木を記録し「更新で壊れた / mod の変更で壊れた」を出し分ける
- 実測: `scripts/test-nightly-mod-check-1892.sh` 100 PASS（claude の無い PATH で 92 PASS + 未実測 1）・回帰の注入 6 通りを名指しで FAIL・実物の claude 2.1.294 で壊れた登録が exit 1・利用者の設定の mtime 一致・既存の nightly 139 / retry 55 / promote 125 緑

## 2026-10-09（#1881: tako mod S3 = Claude Code の画面のプロンプトの上に帯 1 行と /tako のサイドバーを出した）
- 判断は tako（`claude_mod::band_view` / `classify_worker` / `band_warnings` を `tako mod report` の応答の `tako.view` へ）、mod は `bodyColumns` に合わせて優先度の低い区切りから落とし `Text` 1 本（truncate-end）で描く。worker は右パネル orch と同じ `Workspace::workers_of` へ寄せた。トグルは `$.store`（`/tako band on|off`・ボタン・`tako mod band on|off` の中継。新しい方が勝つ）。A/B `TAKO_1877_S3_LEGACY=1`・検証用 `TAKO_1881_BAND_THRESHOLD`・MATRIX `claude_mod_band`・カタログ +74 B
- 実測: `scripts/test-claude-mod-band-1881.sh` 49 PASS 0 FAIL（実 claude 2.1.294・組織 / 個人の設定 dir）で 80 / 144 / 300 桁 × 21 行の帯が 1 行・ダイアログ 4 回の後に戻る・再起動後もトグル保持・A/B で描かない・`claude plugin test` 26 本（terminal / desktop）・番犬 `issue1881_claude_mod_band_watchdog.rs`（注入 12 通りを file:line で名指し）

## 2026-10-09（#1901: debug ビルドでも構文の塗りの依存 8 つだけ opt-level 3 にした）
- `.cargo/config.toml` に `[profile.dev.package.*]` を syntect / fancy-regex / regex-automata / regex-syntax / aho-corasick / memchr / bit-set / bit-vec へ（どれを外しても遅くなるのを 1 MB の TS で実測。自分のクレートは未最適化のまま。ルートの Cargo.toml だと rust-cache のキーに入らず CI が毎回下流を作り直した = PR の初回 macOS 39 分）。commands.md の build 行・#1890 の「debug は 427.6 秒」2 か所に後の値を添えた
- 実測（JOBS=2・前後交互に 2 回ずつ）: 10 MB の塗り 53.8 → 4.3 秒（12.4 倍）・visual-test `large-file-decor` の debug は 1 節 30 分 → 152 秒・差分ビルド tako-core 9.5 / 7.8 → 9.3 / 8.4 秒・クリーンは 8 つで +36 秒の CPU（壁時計は負荷のぶれ以下）・全体テスト 6515 passed（tako-app 単体 30 → 12 秒）

## 2026-10-09（#1874: テストの tmux の器の名前を残骸掃除が拾う接頭辞へ揃え、`-f /dev/null` で起こすようにした）
- 名前 4 つ（`tako-coretest1857-` → `tako-coretest-1857-`・`ct1105-` → `tk-coretest-1105-`（tako で始めないのが #1105 の検査の中身なので `TEST_SOCKET_PREFIXES` を足した）・固定名 `tako-e2e-571` / `-577` に pid）と `-f` なしの起動 12 か所（tmux_e2e の 1 実装・dispatch 5・#1857 の keep・loc・scrollback_capture・remote_scrollback 2）。番犬 `issue1866_tmux_socket_name_watchdog.rs` に名前（定義まで辿る）と `-f` の 2 規則
- 実測: 読まれたら器の名前を記録する `.tmux.conf` を置いた偽 HOME で、修正前は 24 回読まれ修正後 0。途中で kill -9 した #1857 / #1105 の器を修正前の `TmuxTestGuard` は拾わず修正後は回収。番犬の注入（修正前の 5 ファイル・名前・接頭辞の正本）を file:line で名指し

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
