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

## 2026-09-09（#1263: auto mode の環境学習ダイアログを select として検知できるようにした）
- 真因は**ダイアログの下に空の入力欄が描かれる**こと（実測: Issue の 6 行だけなら旧実装でも検知でき、入力欄を足すと `None`）。`dialog.rs` に経路 4 を足し、**入力欄はダイアログの外**とみなして上を見る。開放するのは番号つき経路だけ（番号なしまで広げると複数行の user 発話の継続行を選択肢と誤検知する）
- 生きたダイアログと**会話ログへ流れた残骸**の区別は「並びと入力欄のあいだがダイアログの一部だけでできているか」（#577 の fixture は `✽ Misting…` が挟まる = 非検知。この条件を入れる前は #577 のテストが実際に落ちた）。確定キーの案内が無い形は拾わない = 安全側
- 模擬 TUI（実 tmux・GUI 不要）で respond の実キー: ラベル `Don't show again` / 番号 `3` どちらも `keys=["3"]` で `resolved=true`。A/B `TAKO_1263_LEGACY=1` と修正前ソースで新テスト 6 本が FAILED。全 3928 件緑

## 2026-09-09（#1252: マウスレポート e2e のフレークを止めた・真因は偽アンカー）
- 真因は待ち不足ではなく**アンカーが偽**。conf の `set -g mouse on` で tmux は attach 時に外側端末のマウスを有効にするので、外側 `mouse_reporting()` は内側アプリと無関係に真（実測 20 ms・器側 `any=0`）。要求前のホイールは **tmux が食って copy-mode へ入り**（`pane_in_mode=1`）、以後は待ちを伸ばしても永久に届かない
- アンカーを器のペインの `#{mouse_sgr_flag}` へ（`wait_pane_mouse_ready`）・待ちは `wait_for_state`（状態待ち + `state_wait_budget`・診断は「待っていたもの / 届いたもの」）。予算政策は `tako_core::wait_budget` の 1 実装へ寄せ tako-app が委譲。番犬 `mouse_report_wait_watchdog`
- A/B（同一ビルド・交互・load 12〜18）: 旧待ち **7/75 FAILED** → 状態待ち **0/315**。`LEGACY=1 INJECT=late` は確定 FAILED（`inmode=true`）/ `INJECT=never` は新経路でも FAILED。副産物の osc7 系フレークは #1265 へ分離

## 2026-09-09（#818: 直接ペインのスクロールバック上限を設定可能にした）
- 正本は `tako_core::scrollback`（既定 10,000 / 100〜100,000 / 24 B・セル）。`settings.json` の `scrollback_lines`（`#[serde(default)]` = 旧ファイルはそのまま読める・移行 Step 不要）→ CLI `tako scrollback [lines]` / MCP `tako_scrollback` / 設定画面「ターミナル」節が同じ dispatch を通る
- **生存中のペインにもその場で当たる**（`Term::set_options` → `Grid::update_history` が `Row` を解放）。隔離 GUI 実測（119 桁・persist OFF・debug）: 起動 15 MB → 12,000 行で 46 MB（+31・理論 27.2）→ 上限 1,000 で 30 MB。上限を下げた後に作った新ペインは 12,000 行流しても +2.0 MB（理論 1.8）
- 番犬: 実 PTY の単体 3 本（上限で打ち切る / 既定なら残る / 下げると既存も縮む）+ dispatch 1 本 + MCP / CLI マッピング各 1 本

## 2026-09-09（#1114: psmux の永続化 e2e の間欠失敗を待ち不足として直した）
- 先にフェーズ別の余裕を実測: プロンプト待ちの固定 6 秒だけが**スイート並列（18 本）だけで既に超過**（6.1〜7.1s・`rounds=2` が 10/10）。4 多重 + 負荷 12 で 20〜34s になり 6 周を使い切って **9/16 FAILED**（全件 phase 1・画面は `PS …> Write-O` = 打鍵は届いていた）
- 固定窓を全廃して**期限 + `wait_budget::state_wait_budget`** へ。`machine_busy()` を Windows でも読めるようにし（`GetSystemTimes`）、`sleep(800ms)`→`exists` は「接続クライアントが 1 でなくなった」状態待ちへ。番犬 `psmux_e2e_wait_watchdog` が修正前の 9 か所を名指しで落とす
- A/B（修正前後の 2 バイナリを同一ラウンドで交互）: before **6/24** → after **0/24**。追加で after 0/100。副産物で **#1264**（main の Windows `cargo test --workspace` がコンパイル不能）を起票

## 2026-09-09（#1259: send_input が queued:true を返して無音で届かない問題を直した）
- 真因は**顛末の記録が後続 send では全経路空振り**すること。`record_prompt_delivery_at` は先頭で `if flow != SpawnPrompt { return }`（spawn 専用）・保留と打ち切りは `eprintln!`（GUI の stderr は誰も読めない）・`persist.log` へ書くのは貼り付け段へ到達した後の `try_peer` / `log_fallback` だけ。本番の pane 1627 は `送達:` 行が 1 本も無く（同時刻の他ペインは記録あり）、`WaitPromptReady` で `input_line` が唯一の入口という穴と、peer の背景試行・起動コマンド待ちの**上限なしの待ち**が重なっていた（Issue の見立て「busy だと送らない」は否定 = 送達フローは `has_running_children` を参照しない）
- 語彙と方針を `tako_core::prompt_delivery` の 1 本へ（`Stall` 11 種 + `Journal` の心拍 15 秒 + `peer_wait` の段階判定）。応答は `tako_send_input` / `tako_read_pane` の `delivery`、CLI は `[delivery]` 行、`persist.log` は `送達フロー: pane=… 理由=…`。peer は 25 秒で諦めるが**送る前の段階だけ**キー経路へ落ちる（判定は `PeerAttemptState` の CAS。段階を読むだけでは読んだ直後に背景スレッドが書き始めて二重投函になる = #790 の不変条件）
- A/B `TAKO_1259_LEGACY=1` で無音と無限待ちを再現（実ファイル検証）。番犬 6 本のうち 5 本が修正前 `main.rs` を行番号で名指し（6440 / 6445 / 6457 / 6472 / 6738 / 6797）。模擬 TUI e2e（`sleep 3600` の子を抱えた idle ペイン）で送達成立を実測

## 2026-09-09（#1273: 背景シェルが残る worker で watch が WORKER_IDLE を出さない問題を直した）
- 真因は #289 の**帰属ミス**（修正は原形のまま生きていた）。claude 2.1.258 の `agents --json` は `isLoading || delegatedActive` で状態を決め、背景シェル / Monitor が生きているあいだ**生 status が busy**（本番 4 ペイン同時観測: 申告のある 3 本が busy・無い 1 本だけ idle / `has_running_children` は 4 本とも true = 判別不能）。#289 の腕は `status == "idle"` 限定なので一度も入らず、busy → idle の経路がそもそも無かった
- claude 自身の描き分け（ターン終了 = `· <内訳> still running` / 完了待ち = suffix なし）を借り、**生成中の目印ゼロ + 空の入力欄 + 折りたたみでない + 申告あり**のときだけ倒す（`wait::input_waiting_with_background_work` の 1 実装。応答に `background_work` / `idle_despite_primary_busy`・MATRIX へ `worker_idle_with_background`）
- 番犬 6 本が故障注入 6 通りをそれぞれ 1 本だけ名指しで落とす。実 tmux の e2e 2 本（本物の `sleep 3600 &` 付き）+ 単体 12 本。A/B `TAKO_1273_LEGACY=1` で再現テスト 2 本が FAILED。狭いペインは行が切られて覆せない（安全側に劣化・#1277 で codex / agy を調査）

## 2026-09-09（#1264: Windows で cargo test がコンパイルできない問題を直し CI に番犬を置いた）
- #1199 の検証ブロックが 1 つ手前の関数に入っていた（`E0425` × 3）ので本来の関数へ移し、あわせて `assert_eq!(cwd_after_foreign, cwd)`（書いた後の値どうしで必ず通る）を**書く前の cwd** との比較へ直した。実機 A/B: `--no-run` が **exit 101 / 263s** → **exit 0 / 4s**・`shell_integration_powershell` 8/0
- CI の穴（macOS は `#![cfg(windows)]` を空クレート化・Windows は `cargo build` が `tests/` を作らず `cargo test` は `continue-on-error`）を Windows ジョブの blocking `cargo test --workspace --no-run` で塞いだ。番犬 `ci_windows_test_compile`。追加時間は実質ゼロ（実機で 2 回目の `--no-run` は 2s = 再コンパイル 0）
- **その CI ステップが初回で実在の回帰を捕まえた**: #818 が `SpawnOptions::scrollback_lines` を足したとき Windows 専用テスト 3 本（8 か所）が追従漏れ。同 PR で修正。ベースラインは 5 日ぶりに全数が採れて **19 → 24 件**（増えた 8 件は本 PR 以前から main に在ったもの。表は plan へ）

## 2026-09-09（#1274: ui_text の言語依存テストを言語固定の下で比較させフレークを止めた）
- 真因は「相対比較なら安全」という前提。表示言語はプロセス全体の AtomicU8 なので、ロック外で 2 点を読むと**読み取りのあいだに別テストが切り替えて日英を比べる**。`tests_support` を RAII の `lang_guard`（drop で言語復元 → ロック解放）へ寄せ、ui_text の 17 本 + ui_text 外の 3 本（`update_checker` 2 / `preview` / `right_panel`）を `for_each_lang` の区間へ入れた
- 番犬 `ui_text::lang_watchdog`（`tr!` を起点にした推移閉包 × ヘルパ区間を除いた残りの走査）が**修正前ソースの 17 本を file:line で名指し**。再現 `ui_text::lang_race` は `ui_text/` の外に置く（legacy 経路が番犬の禁止形そのもの）
- A/B `TAKO_1274_LEGACY=1`: 負荷下（load 20 / 18 コア）で legacy 40/40・3 本同時 30/30 FAILED（実出力は `"エクスプローラーで表示"` vs `"Reveal in Explorer"`）→ 修正後 0/40。負荷下 `cargo test -p tako-app` 12 回 0 FAILED・全 3997 件緑

## 2026-09-09（#1271: psmux の後始末に期限をつけて器のリークを止めた）
- 真因は二段: 後始末が素の `Command::output()` で**期限なし**に待つ（返らない回はテストごと固まり Drop が全部走らない）ことと、**本体が先に退役した後の `__warm__` サーバーは `display-message` に映らない**こと（実測: 注入時 10 ソケット中 9 個が `server_pid=None`・残骸は全部 `__warm__`）。消せるのは `kill-server` だけ
- psmux を叩く経路を `tests/common/psmux_ctl.rs` の 1 実装へ（期限つき + 出力は一時ファイル + pid を聞く → `kill-server` → `taskkill /PID /T /F` → 掃き掃除）。番犬が 3 ファイルの `.output()` / `.status()` / 殺していない `.wait()` を落とす（修正前ソースで 8 か所を名指し FAILED）
- 実機 A/B: 注入で旧アームは**完走せず器 +10/回**（0→10→20）→ 新アームは 18 passed で**器 0**（3 回）。12 回連続実行で残骸ゼロ。交互 6 回の所要は before 31.1s → after 22.5s（全ラウンドで速い）
