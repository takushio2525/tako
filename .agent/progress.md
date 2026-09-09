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

## 2026-09-09（#1022: #571 e2e が本番の事前信頼エントリを残さないようにした）
- `E2e571Guard::drop` で `remove_e2e_trust_entry(&dir/work)` を呼ぶ形へ（#612 / #577 と同じ後始末）。この e2e は worker を既定 config dir で走らせるので隔離では倒せない
- 番犬 `e2e_trust_cleanup_watchdog`（`impl Drop for E2e…Guard` を走査して後始末の欠落を名指し）。`#[ignore]` の本体は CI で走らないのでここだけが再発を止める
- A/B: 後始末を外すと `crates/tako-control/src/dispatch.rs: E2e571Guard` を名指しで FAILED

## 2026-09-09（#992: agent 別のセットアップガイドと縮退の明記を docs へ入れた）
- 系統別ガイド 5 ページ（`docs/.../agents/` = 選び方 / claude / codex / agy / ローカル LLM）を新設し、サイドバーに「エージェント CLI」群を追加。#357 の agy 利用制限の調査結果と、getting-started の `settings.json` ドリフト（正は `~/.claude.json` / `.mcp.json`）もここで回収
- `gen-agent-support-docs.mjs` に**系統ごとの縮退ダイジェスト**（理由でまとめた degraded / pending / unsupported）を足して `agent-support.md` を再生成。生成物なので手書きと違ってずれない
- OG が #982 以降落ちていた（`agent-support` が `SECTIONS` 未分類で `npm run og` が例外で止まる）ので分類を足して再生成。24 → 30 枚

## 2026-09-09（#1137: 多重化が無いプラットフォームで無言の SSH 成功が畳まれないのを直した）
- `pane` 経路 + 多重化なし（Windows）は `master_socket` が常に false で成功の出口が 1 つも無かった。規則 ⑥「**打った行を除いて**中身が出たら畳む」を追加（ゲートは `ConnectInputs::multiplexing` の値 = **macOS は 1 ビットも不変**）
- 併発の穴（起点の陳腐化）も `ssh_progress::effective_from` で修正。`connecting` は rebase しないので相手が画面を消すと `new_lines` が全部空になっていた
- A/B: 規則 ⑥ 無効化の注入で `無言の接続成功でも畳む` が FAILED / 打った行の除外を外すと `打った行だけでは畳まない` が FAILED。Windows 実機で単体 32 件緑・macOS 実 pane 経路は従来どおり `connecting`→`connected`

## 2026-09-09（#1238: 再起動後の復元で codex / agy も会話ごと戻るようにした）
- 会話 ID は**生きたプロセスが開いているもの**から採れる（実測: codex 0.153.0 = `thread-writer-locks/<id>.lock`・起動直後から / agy 1.1.27 = `brain/<id>`・最初のターンの後）。`layout.json` へ `agent_resume`（系統 + ID）を `claude_session_id` と対称に保存し、`restore_plan` の分岐で `codex resume <id>` / `agy --conversation <id>` を投入する
- 規則は `tako_core::agent_resume` へ 1 本化（保持 = #1076 の「確認してから外す」を一般化 / 書式 = `resume_spec` / 可否 = `restore_support`）。ID を引く実装は系統ごとのモジュール（#984 / #1033）へ委譲。**Windows は lsof が無く ID を採れない**ので、内訳の理由を `ID なし` と分けて `resume 非対応` に
- 隔離 GUI 実測: tmux サーバー kill → 起動で `Claude resume 1 / agy resume 1 / codex resume 1 / 新規シェル 0` と 3 系統の会話が画面に復帰。A/B `TAKO_1238_LEGACY=1` は同じ layout で `新規シェル 3（ID なし 3）`。番犬 7 本 + 単体

## 2026-09-09（#777: 本番でも SIGTERM で layout を保存して終了するようにした）
- `SIGTERM` の quit 読み替えを隔離限定から本番へ（`quit_signal::install`）。拾うのは 2 秒 tick ではなく既存の **500ms ループ**（新しいタイマー無しで最悪待ちを 1/4 に）。握る以上は対で必要なウォッチドッグ（到着から 5 秒で `exit(143)`・専用スレッド）を同じ入口が立てる
- 連打は 1 度しか quit を撃たない（`QUIT_DISPATCHED`）・ハンドラはアトミック 1 回だけ・Windows は `SIGTERM` が無いので見張りごと立てない。読み替えと強制終了は `persist.log` へ 1 行
- 隔離 GUI 実測（各 5 回）: 直前の窓リサイズが layout に載る **新 5/5 → 旧 0/5**（`TAKO_777_LEGACY=1`）・終了まで中央値 287ms・器のセッションは全回生存。注入したハング 2 形は猶予 2000ms に対し 2135 / 2101ms で `exit=143`。連打は読み替え 1 回 / `on_app_quit` 1 回、起動 0.3 秒後の SIGTERM でも layout は壊れない。番犬 4 本（修正前ソースで全滅）+ 単体 5 本、セルフテスト `TAKO_APP_SELF_TEST_OK`。全 3893 件緑

## 2026-09-09（#1081 / #1284: 解説動画をスライド主軸へ作り直し、X 向けショートも作った）
- v6（#1081）= HTML/CSS スライド + SVG 図解 30 枚を主軸に、実 UI を要所へ挿す構成へ。実 UI の割合は **92.2%（v4・665 秒）→ 37.6%（v6・192 秒）**。8:29 / PII 7 カテゴリ 0 件。小さなサブタイトル行は全廃
- **v5 のブロッカーが構成側で解けた**: 唯一足りなかった `c5_gui10`（担当 AI のペインが生える実写）は図解 `s_c6_grow` が担うので、撮り直しゼロで完成した（画面ロック待ちは 2 回・7.5 時間で撮れずじまいだった）
- X 向け（#1284）= 無音再生前提・ナレーション無し・字幕 68px の 178 秒。**2:11 で 1 本として完結**させ 2:20 版（131 秒）も出した。PII 0 件
## 2026-09-09（#1137: 多重化が無いプラットフォームで無言の SSH 成功が畳まれないのを直した）
- `pane` 経路 + 多重化なし（Windows）は `master_socket` が常に false で成功の出口が 1 つも無かった。規則 ⑥「**打った行を除いて**中身が出たら畳む」を追加（ゲートは `ConnectInputs::multiplexing` の値 = **macOS は 1 ビットも不変**）
- 併発の穴（起点の陳腐化）も `ssh_progress::effective_from` で修正。`connecting` は rebase しないので相手が画面を消すと `new_lines` が全部空になっていた
- A/B: 規則 ⑥ 無効化の注入で `無言の接続成功でも畳む` が FAILED / 打った行の除外を外すと `打った行だけでは畳まない` が FAILED。Windows 実機で単体 32 件緑・macOS 実 pane 経路は従来どおり `connecting`→`connected`

## 2026-09-09（#1238: 再起動後の復元で codex / agy も会話ごと戻るようにした）
- 会話 ID は**生きたプロセスが開いているもの**から採れる（実測: codex 0.153.0 = `thread-writer-locks/<id>.lock`・起動直後から / agy 1.1.27 = `brain/<id>`・最初のターンの後）。`layout.json` へ `agent_resume`（系統 + ID）を `claude_session_id` と対称に保存し、`restore_plan` の分岐で `codex resume <id>` / `agy --conversation <id>` を投入する
- 規則は `tako_core::agent_resume` へ 1 本化（保持 = #1076 の「確認してから外す」を一般化 / 書式 = `resume_spec` / 可否 = `restore_support`）。ID を引く実装は系統ごとのモジュール（#984 / #1033）へ委譲。**Windows は lsof が無く ID を採れない**ので、内訳の理由を `ID なし` と分けて `resume 非対応` に
- 隔離 GUI 実測: tmux サーバー kill → 起動で `Claude resume 1 / agy resume 1 / codex resume 1 / 新規シェル 0` と 3 系統の会話が画面に復帰。A/B `TAKO_1238_LEGACY=1` は同じ layout で `新規シェル 3（ID なし 3）`。番犬 7 本 + 単体

## 2026-09-09（#1246: .agent の md のコンフリクトマーカーを CI で落とす番犬）
- 走査を `.agent/` 配下の md 全部 + 規約 2 本（4 → 38 ファイル）へ広げ、#1241 / #1247 の狭い版を置換。旧版は `&&` が `||` より強く判定の前半が死んでいて `=======` を一度も拾えず、8 文字以上の罫線を誤検知していた
- `=======` は `<<<<<<<` が開いた領域の中だけで見て setext 見出しと弁別（git は必ずこの順で書くので検出力は落ちない）。実物へ 3 行挿入 → 行番号つきで名指し FAILED → 復元で緑
- `.gitattributes` に `.agent/progress*.md merge=union`。実測で merge / diff3 / rebase とも衝突ゼロ。移送 + 追記も安全（領域が重ならない）。両採用になるのは双方が同じ領域を書き換えたときだけで、限界は conventions へ表で明記

## 2026-09-09（#1034: 送達後に実行を断られた worker を完了と誤読しないようにした）
- `execution_refused`（`WorkerErrorKind` / `AgentCliProblem` の新分類・`recommended_action = retry_spawn`）を追加。**#983 のゲートは緩めず**、別ゲート「一次シグナルで作業を 1 歩も観測していない」で分類する（画面推定の busy は TUI の起動描画を拾うので使えない）
- 判定の文言は系統ごとに宣言（`execution_refused_patterns`）。実採取は agy のみで**版で文言が変わる**（1.1.22 = `Verifying your account` / 1.1.27 = `Unable to verify account eligibility`）ので共通語 `account eligibility` を軸にした。claude 2.1.258 / codex 0.153.0 のバイナリに一時的な検証待ちの文言が無いことは実物の走査で確認
- MATRIX に `worker_refusal_detect` を新設し docs を同期。番犬 4 本 + dispatch の e2e 1 本 + 単体 8 本。A/B は `TAKO_1034_LEGACY=1`
## 2026-09-09（#1253: 隔離セルフテストの事前信頼とシェル履歴・単体テストの brew が本番へ書かないようにした）
- 判定を `cfg(test)` から実行時 `paths::is_verification_process()`（テストバイナリ **または** `TAKO_ISOLATED` / `TAKO_SELF_TEST` / `TAKO_VISUAL_TEST`）へ。GUI セルフテストは製品バイナリなので `cfg(test)` が 1 ビットも効いていなかった。書き先は `<temp>/tako-agent-config-<pid>/` で終了時に片付く
- 履歴は **`HISTFILE` だけでは対話 zsh に効かない**（macOS の `/etc/zshrc` が無条件で代入する）ので、rc の後に走る precmd で当て直す（`zshenv.zsh` の `TAKO_VERIFY_HISTFILE`）。brew は #944 と同型の門番で単体テストから起こさない
- 空 HOME の実測（`cargo test --workspace`）: Homebrew 698 → 0 ファイル / シェル履歴 1 → 0 行。A/B `TAKO_1253_LEGACY=1` で両方とも旧挙動が再現。番犬 3 本（走査）+ 挙動 2 本、修正前ソースで 3 本とも FAILED
