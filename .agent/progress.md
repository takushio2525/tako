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

## 2026-09-09（#1271: psmux の後始末に期限をつけて器のリークを止めた）
- 真因は二段: 後始末が素の `Command::output()` で**期限なし**に待つ（返らない回はテストごと固まり Drop が全部走らない）ことと、**本体が先に退役した後の `__warm__` サーバーは `display-message` に映らない**こと（実測: 注入時 10 ソケット中 9 個が `server_pid=None`・残骸は全部 `__warm__`）。消せるのは `kill-server` だけ
- psmux を叩く経路を `tests/common/psmux_ctl.rs` の 1 実装へ（期限つき + 出力は一時ファイル + pid を聞く → `kill-server` → `taskkill /PID /T /F` → 掃き掃除）。番犬が 3 ファイルの `.output()` / `.status()` / 殺していない `.wait()` を落とす（修正前ソースで 8 か所を名指し FAILED）
- 実機 A/B: 注入で旧アームは**完走せず器 +10/回**（0→10→20）→ 新アームは 18 passed で**器 0**（3 回）。12 回連続実行で残骸ゼロ。交互 6 回の所要は before 31.1s → after 22.5s（全ラウンドで速い）

## 2026-09-09（#1283: ~ 始まりのパスを地の文の中でも cmd+クリックで開けるようにした）
- 真因は見立て（`~` 展開 / `.mp4` のプレビュー非対応 / soft wrap）と別で、**トークンの区切りが ASCII の空白と `()[]{}<>,;` だけ**だったこと。日本語の文にパスが埋まると**文ごと 1 トークン**になり実在チェックで落ちる（実測トークン: `` 動画は`~/…mp4`、確認して。`` / `~/…mp4このリンクが` = **素の形だけが飛べていた**）
- 区切り集合は増やさず（`資料（最新）.pdf` のような実在名を落とす）**トークン全体 → 文字種の切り替わりで削った候補**を長い順に試す形へ（`links::path_candidates`。全 ASCII は候補を増やさない / `/` 終わりは採らない）。囲みの前に地の文がくっつく形（空白入りパス）は**途中のバッククォートで切る**ことで回収。`open_plan` を「拡張子 → 開き方」の正本にし（`.mp4` = 動画プレビュー）、開けなかったときの `eprintln!` を通知 + persist.log へ。CLI `tako links` / MCP `tako_links` を新設
- A/B `TAKO_1283_LEGACY=1` で 2 形が `[]` に戻る（素の形は残る）。隔離 GUI（tako-vd）の実画面で 3 形すべて検出 + `open=video`・セルフテスト項目 147 に 2 形を追加して `TAKO_APP_SELF_TEST_OK`

## 2026-09-11（#1261: 単体テストが実 agy / codex を起こして本番ホームへ書かないようにした）
- 起動元は空 HOME + 偽 CLI の shim で 2 本に特定（`setup_bootstrap::tests::状態は3系統ぶんまとめて返せる` = `agy models` / `codex login status` / `claude auth status --json`、`stale_binary::tests::test_check_stale_different_binary` = `claude --version`）。**tako が書かなくても実 CLI は起動しただけで自分のホームを作る**
- 問い合わせ起動を `tako_control::agent_probe::run` の 1 か所へ寄せた。判定は実行時（`paths::is_test_process`。#1253 の brew と同型）で、#586 のコンソール窓抑止もここが当てる
- 空 HOME の `cargo test --workspace` 実測: 36 → 1 ファイル（`.gemini` 32 / `.codex` 1 / `.claude` 2 が 0。残る `.zsh_history` は 0 バイトで修正前から同じ）。番犬 5 本が修正前ソースで FAILED

## 2026-09-11（#1265: osc7 系 3 テストの真因を測り直し、状態待ちへ寄せた）
- 見立て（固定 10 秒窓が短い）は**実測で否定**。修正前のまま CPU だけの負荷（load 11〜46 / PTY 99〜103・上限 511）で 150 回 = **0 FAILED**、OSC 7 の到達は 744〜1,171 ms（p50 858 ms・90 サンプル）で窓に 8 倍以上の余裕。元の 8/150 は PTY 404/511・tmux 135 本の枯渇（サーバーが `openpty` で止まる `sample`）の副作用だった。仮説②（`.zshenv` の競合）もテストの data dir が pid ごとに隔離（#944）されていて成立しない
- それでも固定窓は「窓が短い / 器が動いていない」を診断から消すので `wait_osc7_cwd` + `probe_osc7`（状態待ち + `state_wait_budget`・器のペインの `#{pane_dead}` / `#{pane_current_command}` / `#{pane_current_path}`・`ZDOTDIR` セッション/サーバー・`.zshenv` のバイト数）へ。**診断の採取にも期限**をつけた（器が応答しない場面でこそ要るのに素の `output()` では診断ごと固まる = #1271 の罠）
- A/B は `TAKO_1265_INJECT=late`（起動を 12 秒遅らせる）: 旧アーム（`TAKO_1265_LEGACY=1`）は **Issue と同じ画面**で 75/75 FAILED・新アーム 0/75。`nointegration` は両アーム FAILED（検出力）。番犬 `osc7_wait_watchdog` が固定回数窓の復活を落とす（修正前ソースで 9 件を名指し）

## 2026-09-11（#1277: codex / agy の「背景作業つき入力待ち」を MATRIX へ確定した）
- 隔離 tmux で実 CLI を起こして採取（codex-cli 0.154.0 / Antigravity CLI 1.2.0）。**両系統は一次シグナルが張り付かない**（背景作業が生きたまま rollout の `task_complete` / 実況 JSONL の終端が書かれ `read_turn_state` → idle）ので #1273 の覆す腕は要らず、MATRIX の `worker_idle_with_background` を 3 系統とも `Supported` へ
- 画面の申告は系統別に読めるようにした（codex = `1 background terminal running · /ps to view · /stop to close` / agy = フッターの `· 1 task(s) · /tasks`）。ただし**両系統の申告は生成中も同じ形で出る**ので `declaration_implies_turn_end` で claude 限定にし、番犬 3 本 + 単体 15 本 + A/B `TAKO_1277_LEGACY=1` で固定
- agy の実況行（`● [07:15:49] <コマンド> running`）は内訳にしない（コマンド全文が応答へ漏れる。#927）。docs 再生成で codex 34→35 / agy 23→24

## 2026-09-11（#1035: 番犬が gitignore 済みの未追跡ファイルで落ちないようにした）
- `no_personal_data` の走査対象を「public リポに出るファイル」= `git check-ignore` で**未追跡かつ ignore 済み**でないものへ限定。`.claude/settings.local.json` で手元が恒久的に赤い状態を解消（手元 2 failed → 6 passed）
- **索引を見る既定の `check-ignore` を使う**ので `git add -f` した tracked は ignore に一致しても走査に残る（`--no-index` 禁止 = `.gitignore` で隠す抜け道を塞ぐ）。`git` が無い / リポ外は全部走査（実測で旧挙動どおり 2 failed）
- 実測: tracked 注入 → ①②とも FAILED / 未追跡かつ非 ignore 注入 → ①②とも FAILED + 「CI は緑のまま」の案内行 / 未追跡かつ ignore 済み注入 → 緑。番犬 2 本追加

## 2026-09-11（#1293: 会話ログの番号つき箇条書きがダイアログの選択肢に混ざらないようにした）
- 真因は非対称。選択カーソルの探索は末尾 40 行に絞ってあるのに**収集（`numbered_rows`）だけが画面全体**だった。`numbered_block` で起点から上下へ「あいだがダイアログの一部だけ（`gap_line_kind`）」かつ「番号が 1 ずつ増える」あいだに限定し、経路 1〜4 すべてへ effect。`title` も同じ塊に限る（境界 = 罫線 + 0 桁の非空行 + 採らなかった番号つき行）
- A/B `TAKO_1293_LEGACY=1`: 単体 5 本が Issue と同じ `options=6`・`highlighted=Some(3)`・`header=["⏺ 直し方の候補は 3 つあります。"]` で FAILED → 修正後 3 択・header はダイアログの説明文のみ
- 実 tmux の模擬 TUI e2e（新規 2 本）: 下見が 3 択・`--choice 1` が `choice_text="Yes"` / `keys_sent=["1"]` / `resolved=true`。legacy は `choice_text="待ちを状態待ちへ寄せる"` = Issue の「監査ログが嘘になる」を再現

## 2026-09-11（#1282: `tako tmux cleanup --servers` を psmux の器でも使えるようにした）
- 器の列挙を 2 実装へ（unix = ソケットファイル走査 / Windows = 器のプロセスのコマンドライン `-L <名前>`。psmux は名前付きパイプで走査できるファイルが無く、実機に 24 個残っていても 0 件と答えていた）。コマンドラインは `NtQueryInformationProcess(ProcessCommandLineInformation)`・起動時刻は `GetProcessTimes` で、依存クレートは足していない
- 所有者は 2 段（ソケット名の pid = unix と同じ強さ / コマンドラインの隔離マーカー = 自分以外の生きた tako-app が居れば `peer_may_reattach` で見送る）。材料が無ければ従来どおり `owner_unknown`。回収は `taskkill /PID /T /F` の **pid 指定のみ**
- macOS: fmt / clippy / `test --workspace` 4074 件緑・クロスチェック エラー 0（警告は既存箇所のみ）。番犬 5 本（修正前ソースで 4 本 FAILED）+ 単体 11 本（応答の形は dispatch で固定）。FFI は CI の Windows ランナーで実行検査（既存の `cargo test --workspace` は #583 の失敗で tako-core まで届かないので blocking の別ステップを追加）。**Windows 実機実測は未取得**（機が offline。手順は plan の「#1282」節）

## 2026-09-11（#1294: peer 送達の「送ったかもしれない」を未達扱いにせず二重投函を止めた）
- `Stall::PeerSendStalled`（再送禁止の宣言）と registry の記録が逆を向いていた。送達の記録を 3 値（`tako_core::prompt_delivery::Confidence`）にし、`peer_send_stalled` / `peer_unconfirmed` は `Unverified`（#983 の `verify_then_resend`）へ。3 値目の宣言は `outcome_confidence` の表 1 本で、記録側と判定側が同じ表を引く（**既定は未達側**）
- A/B（`TAKO_1294_LEGACY=1`）: 旧アーム = `prompt_delivery=undelivered` / events `prompt_undelivered` / **自動再送 1 回**、新アーム = `unverified` / `prompt_delivery_unverified` / **0 回**。本当に未達（`paste_not_reflected`）は両アームとも 1 回で回帰なし
- 番犬 3 本（記録が bool を渡す形 / 判定が無条件 `OverdueSuspect` / 自動再送の引き金の一本化）が修正前ソースで file:line 名指し FAILED。単体 8 本・dispatch e2e 1 本

## 2026-09-11（#638: 状態ファイルの tmp 名を書き込み 1 回ごとに分けた）
- 真因は #625 と同型。tmp 名が pid 止まりなので A の rename 後に B が**本番ファイルになった同じ inode** へ書き込む（実測の途中状態 `len=8194 head="S\ns\nLLLLLLLL" tail=…lll` = 短い本文が長い本文の先頭を潰した形）+ B の rename は ENOENT で失敗
- `shell_integration::write_state_file` を `tmux_backend::write_conf_in` と同じ作法へ（pid + `AtomicU64` の seq・rename 失敗時は tmp を掃除）。tmp 名は純粋関数 `state_tmp_name` に出して両アームを単体で固定
- A/B `TAKO_638_LEGACY=1`: 8 スレッド × 300 書き込みの再現テストが旧アーム **50/50 FAILED** → 新アーム **0/100**（CPU 負荷 load 92〜113 下でも 0/50）
