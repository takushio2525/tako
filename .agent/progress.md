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

## 2026-11-01（#2634: 器なしの `tako tmux open` のペインを閉じるたびに利用者の tmux サーバーが約 600ms 止まっていた → #2625 の印を器なしのビューにも立てる（`tmux_backend::hang_up_view_client_on_close`・A/B `TAKO_2634_LEGACY=1`）。GUI の終了は OS が親側を先に閉じるので止まらず変えない）
- 隔離 GUI で修正前 592〜606ms → 修正後 6ms（5/5）・legacy 592〜607ms・終了は器 3 枚で 6〜9ms・器は残り再起動で戻る・番犬が注入 6 通りを file:line で名指し・カタログ +0 B。PR #2636

## 2026-11-01（#2630: MCP `tako_remote_stop` / `tako_remote_status` と設定画面の停止・リモートタブの読み直しを `prepare_offload` → `OffloadJob::RemoteStop` / `RemoteStatus` で background へ（#1555 と同じ形・A/B `TAKO_2630_LEGACY=1`））
- 注入 3 秒で `tako list` の待ち 2.7 / 5.6 / 5.6 秒 → 17 / 17 / 22ms・perf.log の専有 0・応答は A/B と一致・カタログ +0 B。PR #2637

## 2026-11-01（#2633 (c): ログインシェルの問い合わせの打ち切りがグループへいきなり SIGKILL を送り、rc の `pyenv rehash` のロックを残していた → グループへ SIGTERM 1 回 → 猶予 1 秒 → 残りへ SIGKILL（刈り取り係・頭は判定まで刈らない）。A/B `TAKO_2633_LEGACY=1`・番犬 `issue2633_probe_group_term_watchdog`）
- 実経路で修正前 5/5 → 修正後 0/5、連鎖（後続の問い合わせの打ち切り）11/11 → 5/11。呼び手が返る時刻は同じ・カタログ +0 B。(a) は直さない（master が [提案] Issue へ）。PR #2642

## 2026-11-01（#1675: `tako context-budget fix` がアーカイブ済みのまま作業ログへ戻ったエントリを、#1645 の `revivals` の判定を共有して予算計算の前に落とし 1 回で収束するようにした。同じ見出しで本文の違うものは触らない・check は直るものを `fixable` / `revived_fixable` へ数えて `next_step` に fix）
- 修正前は予算内の二重で fix が「変更なし」のまま → 修正後 1 回目で 4→3 件・2 回目はバイト一致・二重でない 3 素材は修正前と出力・書き込みが一致（check --json に欄 1 つ）・カタログ +0 B。PR #2641

## 2026-11-01（#2639: `tako_orchestrator_respond` の 400ms の正体は `respond_via` の固定の sleep → 届き方の解決（`respond_access`）だけ UI スレッドで行い `OffloadJob::OrchestratorRespond` で background から応答（錠で 1 本ずつ・チャット表示の承認カードも同じ口・A/B `TAKO_2639_LEGACY=1`））
- 模擬 TUI で応答の間の `tako list` 308〜326 / 696〜766ms → 最長 17 / 20ms・perf.log の専有 0・応答と監査記録は A/B と一致・カタログ +0 B。PR #2643

## 2026-11-02（#2644: issue2576 の番犬が全体並列の負荷で落ちる間欠 → 作りたての偽シェルの初回 exec（macOS の検査・手元 130〜390ms）が確かめの上限 5 秒の内側で払われていた。偽シェルを作ったその場で 1 回起こして終わるまで待ってから測る（上限は延ばさない））
- 初回 exec を 6 秒にする注入で修正前は Issue と同じ `nu / Ja` の不明を含む FAILED → 修正後 8 / 8・同型の候補 4 本は同じ注入で全部 ok・カタログ +0 B。PR #2648

## 2026-11-02（#2365 の続き: 読めないプロファイルの理由（#2256 の系統）を表示言語へ。芯 `UnreadableProfile`・知らせ `ProfileFallback` 5 種・`ConfigError` 3 種、利用者へ返す口は表示言語・persist.log は日本語。check-health の profiles 行も型の口へ）
- 実経路（CLI + mcp_host の MCP）で英語表示の日本語入り 32/64 → 4/64（残りは projects add の枠）・日本語 56/64 sha256 一致（残り 8 は時刻と置き場だけ）・persist.log 一致・注入で名指し + E0004・カタログ +0 B。PR #2645
