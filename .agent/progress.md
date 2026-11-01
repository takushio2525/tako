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

## 2026-11-01（#2365 の続き: dispatch の注記 32 種と fda の description を新しい `ReplyNote` へ、CLI の手元の断り 17 種を `Reason`・突然死からの復旧 3 種を `SessionRefusal` へ、同じ出力の行 17 種も `ReplyNote` へ）
- CLI 23 ケースで英語の日本語入り 23 → 0（日本語 20 / 23 sha256 一致・残り 3 は移行の知らせの言語の修正）・隔離 GUI 13 / 13 → 0 / 13（日本語 13 / 13 一致）・注入で名指し + E0004・カタログ +0 B。PR #2606

## 2026-11-01（#2597: tcsh / csh の利用者の明示コマンドのペインが `-l` で消える / 器経由は `Unmatched '''.` で止まる → ログインの層は表の csh 系の行を `exec -a` の段越しに・器の 1 行は csh の引用（A/B `TAKO_2597_LEGACY=1`））
- 隔離 GUI で修正前 9 本とも不成立（器なしの tcsh / csh 6 本 = `Unknown option`・器ありの tcsh 3 本 = `Unmatched`） → 修正後 tcsh / csh × 器なし / 器ありの 12 本が起きる・csh 系でない 6 種の argv は修正前と一致・カタログ +0 B。PR #2607

## 2026-11-01（#2603: 名前が dash / ksh のシェルは種別の分からないシェルの断りで手作業だった → 本物の dash と ksh93u+ が `-l -c`・argv[0]・対話ログインで `.profile` を読むと確かめて `ShellKind::Sh` へ寄せる（`SH_PROFILE_SHELLS`）・確かめていない mksh ほかは寄せない・A/B `TAKO_2603_LEGACY=1`）
- 砂場の CLI で修正前は断り（rc=1）・段は path → 修正後は `.profile` に書いて auth へ進み、本物の `dash -l` / `ksh -l` から claude を引ける。既知のシェル 8 種 × 3 通りの 24 ケースはバイト一致。注入 2 通りを番犬が file:line で名指し。カタログ +0 B。PR #2609

## 2026-11-01（#2608: GUI の自動復帰が daemon を入れ替える途中の remote status が、新版の待ち受けを PID ファイルの無い待ち受け（#2455）と読んで `tako remote stop` を案内していた → `spawn_daemon` が抜けるまで `tako-remote.starting` を置き、status は `starting`（replacing / starting）を返して stop を案内しない（書き手の死・pid の再利用・90 秒で失効・A/B `TAKO_2608_LEGACY=1`））
- 隔離 GUI の実経路で修正前 FAIL 6（区間の 35 読みが全部 stop の案内）→ 修正後 PASS 27・MCP も同じ・注入で番犬が file:line を名指し・カタログ +0 B。PR #2611

## 2026-11-01（#2365 の続き: check-health 一式と `tako setup --check` を表示言語へ。文は新しい `HealthText`（147 種）、共有の文は各モジュールの `_in(lang)`。`setup --check` は「setup は日本語」の枠から外した）
- 英語表示の日本語入り: CLI 経路 12/12 → 0/12・隔離 GUI の CLI / MCP 3/3 → 0/3（問題 0 件でも同じ）。日本語は全件一致（意図した差は旧文の空白 1 か所）。注入 7 通りを名指し / E0004・カタログ +0 B。PR #2612
- 09:09 に `31b5683` を install・GUI 再起動（画面ロック中・復元失敗 0・remote daemon は replaced-generation）。再起動直後 90 秒の status 180 回で stop の案内 0（区間は starting.state=replacing）の本番確認で #2608 を閉じた

## 2026-11-01（#2610: save_layout が layout.json の書き出しとペインログの Mutex 待ちでメインスレッドを止めていた（本番最大 99 秒）→ 専用の書き手スレッド 1 本（最新 1 件の合流・tmp + rename）+ `try_lock`、終了と起動し直しの前は書き終わりを待つ・A/B `TAKO_2610_LEGACY=1`）
- 注入 5 秒で IPC 5,023ms → 18ms / ペインログ 4,840ms → 22ms・SIGKILL / 終了 / 順序 / 書けない dir を実経路で確認・番犬が逆戻り 4 種を file:line で名指し・カタログ +0 B。PR #2614

## 2026-11-01（#2613: 起動・入れ替えの途中（#2608 の印が効いている間）の `tako remote start` が旧版を止めて daemon を 2 本にしていた / 印を消していた → `spawn_daemon` が状態を読む前に印を見て `RemoteRefusal::DaemonStarting` で断る・stop / `--force` も止めても戻るので `DaemonStartingStop` で断る（A/B `TAKO_2613_LEGACY=1`））
- 実経路で修正前 FAIL 28（daemon 2 本・区間の残り 26 回が stop の案内）→ 修正後 PASS 59 / 0・番犬が注入 7 通りを file:line で名指し・カタログ +0 B。PR #2616

## 2026-11-01（#2365 の続き: `tako master` / `tako solo` の起動の流れを表示言語へ。言語を移行より前に決め（日本語表示に英語の移行の知らせが混ざっていた）、断りは `Reason` 3 種と型の口、行は `ReplyNote` 26 種。host.rs の既定 26 種を `HostRefusal::Default*` へ・mcp_host が表示言語と切り替えに従う）
- GUI 無しの CLI 19/23 → 0/23（日本語 14/23 一致・残り 9 は移行の知らせの修正）・隔離 GUI 10/10 → 0/10（日本語 10/10 一致）・mcp_host 12/12 → 0/12・注入 6 か所を名指し + E0004・カタログ +0 B。PR #2617
- 11:29 に `00a3b4b` を install・GUI 再起動（画面ロック中・復元失敗 0・remote daemon は replaced-generation）。再起動直後の入れ替えの区間に打った remote start / stop が両方断られ、daemon は 1 本のまま新版へ移った本番確認で #2613 を閉じた

## 2026-11-01（#1623: setup --check / check-health のプロファイルの行が登録済みの名前を 1 行に全部並べていた（84 個で 449 字）→ 件数・先頭 3 個（default → 名前順）・他 N 個の 1 行、全件は `--json` の `items[].names`（A/B `TAKO_1623_LEGACY=1`））
- 実経路 0 / 1 / 3 / 4 / 84 個 × 日英で OK 63 / NG 0・旧形は #1623 前とバイト一致・プロファイル以外の行もバイト一致・注入で番犬が file:line を名指し・カタログ +0 B。PR #2621

## 2026-11-01（#2619: 隔離中（`TAKO_ISOLATED`）の remote start（CLI / MCP / daemon の直起動 / UDS の setup）が既定ノードへ serve を張っていた → 門 `remote_isolated_serve::check` で断る（検証モード・偽 tailscale・`TAKO_REMOTE_ISOLATED_SERVE=1` は通す・A/B `TAKO_2619_LEGACY=1`））
- 段 1 は 3 経路とも GUI 版ノードへ `serve_set`（daemon 1 本・本番 standalone は不変）→ 修正後は断り・daemon 0 本・両ノード不変、opt-in / A/B は従来どおり。番犬が注入 7 通りを file:line で名指し・カタログ +0 B。PR #2622

## 2026-11-01（#2618: ペインログの追記・ローテーション・全体上限の削除を Mutex を握ったまま呼び手のスレッドで行い、直接ペインの取り込み・閉じたときのフラッシュ・Mutex を待つ口がメインスレッドを止めていた → 専用の書き手スレッド 1 本（積んだ順の FIFO）へ積み、Mutex の内側はメモリだけ・終了は書き終わりを待つ・A/B `TAKO_2618_LEGACY=1`）
- 注入 5 秒で取り込み 4,825ms → 15ms / 閉じる IPC 15,031ms → 20ms / `logs status` 9,960ms → 30〜167ms・中身は修正前と一致・詰まった終了で 350 行 + マーカーを確認・番犬が逆戻り 4 種を file:line で名指し・カタログ +0 B。PR #2623

## 2026-11-01（#1556: セルフテストをリポジトリ外の cwd から起動すると項目 14 が cwd で `cargo build -p tako-cli` を回して `could not find Cargo.toml` で落ちていた → 根を実行ファイルの位置 → 実行時の `CARGO_MANIFEST_DIR` → cwd の順に辿ってそこで回し、見つからなければ日英の案内で止まる（A/B `TAKO_1556_LEGACY=1`・注入 `TAKO_1556_INJECT=noroot`））
- 隔離 GUI で同じ外の cwd から修正前は `could not find Cargo.toml` → 項目 14 で FAILED（9 秒）→ 修正後は根で回して項目 17 まで進む・注入で日英の案内・A/B は旧挙動・番犬が注入 5 通りを file:line で名指し・カタログ +0 B。PR #2626

## 2026-11-01（#1555 の残り: MCP `tako_remote_start` と設定画面の「開始」が daemon の起動を UI スレッドで待ち、起動途中の daemon が GUI の IPC へ送る probe と読み取りの上限 10 秒まで待ち合っていた（本番 10.5 秒の正体）→ `prepare_offload` の `OffloadJob::RemoteStart` で background から応答・A/B `TAKO_1555_LEGACY=1`）
- 隔離 GUI で start の間の `tako list` 17,947ms → 504ms・`dispatch:RemoteStart` 専有 18,141ms → 0 件・応答は一致・番犬が注入 6 通りを名指し・カタログ +0 B。PR #2628

## 2026-11-01（#2627: 隔離 GUI のペインの zsh が利用者の rc を読み、GUI の終了でペインへ重ねて届く SIGHUP が rc の `pyenv rehash` の EXIT の trap を断って shim のロックを残していた（後片付けの TERM / KILL ではない）→ `launch_isolated_gui` が既定でペインの zsh を砂場の ZDOTDIR へ向ける・opt-in は `TAKO_ISO_USER_RC=1` + `# tako:user-rc-ok <理由>`・A/B `TAKO_2627_LEGACY=1`）
- PR #2631

## 2026-11-01（#2625: 器のペインを閉じるたびに器の tmux サーバー全体が約 600ms 止まっていた（tty の最後の close がサーバーの中で読まれない出力を抱えて待つ。tako のメインスレッドは止まっていない）→ 器のペインの端末は閉じるときに PTY のスレッドが attach クライアントへ SIGHUP を送り、子の側が全部閉じるまで親側を読み捨ててから閉じる・A/B `TAKO_2625_LEGACY=1`）
- 閉じた後の外からの `tmux ls` 593〜607ms → 6〜10ms・器が消えるまで 633〜654ms → 24〜50ms・fd / 子 / ゾンビ・終了と復元・直接ペインは同じ・番犬 4 件・カタログ +0 B。PR #2629
- 18:45 に `70faa47` を install・GUI 再起動（画面ロック中・remote daemon は replaced-generation）。本番の `tmux -L tako ls` が閉じた後 598ms → 49ms の本番確認で #2625 を閉じた

## 2026-11-01（#2624: 段 1 で setup（CLI / MCP）は serve 以外に tailscale を書き換えないと確認（up / login / set / funnel なし）→ 門の外の 3 経路（stop の後始末・対話 setup の brew 導入・QR のビューア）と Windows の本物の置き場を #2619 の門 `allows_op` へ通す（A/B `TAKO_2624_LEGACY=1`・③ の自動復帰は画面ロック中に確かめられず見送り））
- 偽の tailscale / brew / open の記録で修正前 6 / 1 / 1 回 → 修正後 0 / 0 / 0 回、opt-in・A/B・本番の経路は従来どおり、両ノードの serve status は前後一致・番犬が注入 9 通りを file:line で名指し・カタログ +0 B。PR #2632

## 2026-11-01（#1721: 狭いペインで claude が背景作業の申告を折り返し、mod なしの worker が `busy / agents-auto / idle_override_blocked: null` のままだった → 折り返しをつないで読み、入力欄の直前の申告に限る・読めず状態行が切れていれば `status_line_truncated`・A/B `TAKO_1721_LEGACY=1`）
- 実 GUI の 36 / 24 桁で修正前 busy → 修正後 idle（生成中は busy のまま）・mod の経路は修正前から idle・番犬が注入 4 通りを file:line で名指し・カタログ +0 B。PR #2635
