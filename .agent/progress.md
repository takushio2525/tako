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

## 2026-10-10（#1959: tako mod S7-1 = setup が Claude Code の設定 dir ごとに skills/tako へ管理印つきの写しを入れる）
- 判断と書き込みは `tako_core::claude_mod_install`（印 `.tako-managed`・衝突の読み取り・`skills/.tako.tmp-*` からの差し替え・検証プロセスは一時 dir の外へ書かない番人）、置く先の解決と JSON / setup の文面は `tako_control::claude_mod_install`。発火は setup の段・GUI 起動時・mod の報告の `config_dir` の差分検出、口は `tako mod install|uninstall [--dry-run]`（MCP `tako_mod`・カタログ +217 B）。衝突 / `/plugin` で止めた設定 dir のペインへは env も注入せず mod も休眠（`dormant`）。A/B `TAKO_1959_LEGACY=1`・MATRIX `claude_mod_install`・SPECS `claude_mod_mark`
- 実測: `scripts/test-mod-skills-1959.sh` CLI 62 PASS・GUI 13 PASS（実 claude 2.1.294・一時 HOME。plugin list に tako@skills-dir・env 注入と同時でも hooks module は tako@inline の 1 つ・本物の ~/.claude* は前後で同じ）・`claude plugin test` 30 本・番犬（注入 15 通りを file:line で名指し）

## 2026-10-10（#1979: 本番 GUI が `ps` の poll で 10 分固まった件 = UI スレッドから届く子プロセスの待ちに上限・`tako list` の採り直しを background へ・フォーカスの無いペインの再描画に上限）
- 真因（stripped の sample を同じ日の main のシンボル付き release と命令列で突き合わせ）: IPC → `prepare_offload` → `collect_worker_status_ctx` → `has_running_children` → `process_parent_map` → `ps` の `output()` → `poll(-1)`（= #1976 が外した経路）。型を塞いだ: 親子表は libproc（`PROC_PIDT_SHORTBSDINFO` + `KERN_PROCARGS2`）・tmux / `claude agents` は `probe::command_output_with_timeout`・打ち切り後は同期に wait しない・`OffloadJob::List`。再描画は `tako redraw-limit` / MCP `tako_scrollback` の `unfocused_fps`（既定 30・フォーカス中 60）。A/B `TAKO_1979_LEGACY=1`・番犬 `issue1979_*`・カタログ +330 B
- 実測: `scripts/test-ui-thread-wait-1979.sh` 修正後 15 PASS / 修正前は①②で UI が固まる（別の要求が 8 秒で返らない・メインスレッドに本番と同じ関数の並び）・GUI 無しの注入（修正後 0.03 / 2.4 秒・修正前 10 秒で返らず）・再描画の要求 55〜72 → 29.5 回/秒。CPU は蓋閉じでフレームが組まれず（`body_renders` 0）差が出ない = 蓋を開けた機では未測

## 2026-10-09（#1944: 未導入の言語サーバを開いた時点で data dir へ取って起こし、エディタに状態を出した）
- 検出表の各行に `fetch`（pyright 1.1.414・ts-ls 5.1.3 + typescript 5.9.3 = npm の tarball、rust-analyzer 2026-09-21 = GitHub の単体、Node.js 24.21.0 は足りる node が PATH に無いときだけ。clangd は取らない）を配布元の公開ハッシュで検証して `<data_dir>/lsp-servers` へ。解決は env → PATH（`probe_args` で動くか確かめる）→ 置き場 → 取得。状態「取得中」・タイトルの 1 行と失敗の帯（入れる / もう一度取得）・`tako lsp install`（GUI 無しでも）・シェル統合の合図で引き直し（#1823 の 2）。A/B `TAKO_1944_LEGACY=1`・カタログ +235 B
- 実測: まっさら（一時 HOME・最小 PATH）で .py を開くだけで Node.js 52.9 MB + pyright 4.2 MB を取り 11.6 秒で読み込み済み・診断 2 件・補完 34 件・常駐 155 MB（ts-ls 4.9 MB / 4.2 秒・RA 13.9 MB / 21 秒）。e2e 10 本・番犬（注入 10 通り）・`scripts/test-lsp-fetch-1944.sh` 25 PASS

## 2026-10-10（#1967: 再起動の後に会話が戻らない = 折り返した案内の途中までの ID・カタログだけの再開コマンド・送った後を見ない、を直した）
- 真因は `parse_resume_hint` が細いペインで折り返した案内の 1 行目を権威として ID を壊したこと（本番の zsh 履歴に `…065b9` の resume の実行が残っていた）。再開は `resume_launch` の 1 実装（master / solo = `build_master_cmd`、worker = `build_worker_cmd` + `--resume`）へ。シェルの戻り待ち・送った後 10 秒の見張り・1 回の打ち直し・`last_restart`・persist.log・重ねた再起動の拒否・シェルだけのペインの harness。MATRIX `resume_launch_args`・カタログ +362 B
- 実測: `scripts/test-session-restart-1967.sh`（隔離 GUI・偽 claude）で新旧 × 細く低い / 細く高いが緑。旧挙動は 2 行で #1940 型の化け（2 回起動）、34 行で本番と同じ途中までの ID の resume を再現。番犬 `issue1967_session_restart_watchdog`（注入 15 通りを file:line で名指し）

## 2026-10-09（#1943 S0: IDE の対応形式の棚卸し = 45 形式 × 6 列の対応表と .ino の設計・子 Issue 9 本）
- `.agent/plans/2026-10-language-coverage.md`: 表の正本は 10 か所（言語 1 つで 6 か所 + テスト 3）。45 形式中 色なし 6・誤った構文 2（`.h` `.m` → Objective-C）・言語サーバ 8・Finder 21・Windows 関連付け 0・構文名は CLI / MCP から読めない。挙動の表は分けたまま「拡張子 → 言語の同定」だけを `file_type` へ寄せる案
- `.ino` は IDE 2.3.5 同梱物で実測: clangd 単体は不可（嘘のエラー 10 件）・ALS は基板が無いと黙る・基板の正本は `sketch.yaml`・C++ で塗ると表示中 +19.8 MB（構文は増えない）。子 Issue #1948〜#1956（依存は #1943 のコメント）

## 2026-10-09（#1877 S7 設計 + 試作: 描画の主を mod へ・setup での導入・定型の UI 設定）
- 設計書 `.agent/plans/2026-10-tako-mod.md` §9。本物の認証を写さず偽 Messages API + 一時の設定 dir の実物 2.1.294 で試作: カード（ToolGroup を展開して ToolUse を描き替え・Copy は改行なしの論理文字列）・`/compact` のワンボタン・PromptHint の 5h / 7d / ctx バー・吹き出し。tako はクリックを TUI へ渡さない・S3 の帯が他の mod の帯を消すのを実測
- 導入は `<設定 dir>/skills/tako/` への管理印つきの写し（settings 系は書かない・ホットリロード）+ env 注入を残す（同名は env だけ読まれる）。marketplace は settings.json を書き直し外しても残骸 = 不採用。子 Issue #1958〜#1965

## 2026-10-10（#1962: tako mod S7-3 = 帯を next で包む 1 行・カスタムボタン・使用制限 / ctx のバーを mod で描き、描いている間はステータスバーの claude の区画を止める）
- 帯は他の mod の行を包み tako の行はボタン込み 1 行（`register.ts` の `bandRow` / `stackBand`）、バーは PromptHint の tail（描くかは `claude_mod::usage_bar_decision` = statusLine の有無 × 画面の数値・mod 自身のバーは数えない）、ボタンの argv は tako が組む（`TakoOp::cli_args`）。報告に `renders` / `status_line` / `last_press`、フォーカス中のペインの mod がバーを描けば `claude_bar_owner` で区画を外す（`tako mod` / `tako limit-service --refresh` の `status_bar`）。A/B `TAKO_1877_S7_LEGACY=1`
- 実測: `scripts/test-mod-band-buttons-1962.sh` 61 PASS 0 FAIL（実 claude 2.1.294・一時の設定 dir + 偽 Messages API・本物の ~/.claude* は前後で同じ）・`claude plugin test` 44 本・visual-test `mod-bar`（旧は ① で FAILED）・番犬 `issue1962_mod_band_buttons_watchdog`。罠: `kill -STOP` した claude 2.1.294 はすぐ起き直す

## 2026-10-09（#1933: 夜間の mod 検査のモックテストの間欠的な赤 = 落ちた段の手掛かりを出し、手掛かりで見つけた 2 つの赤を直した。CI の 1 回の原因は未確定）
- CI の 5 件は「Test 2 の注入前の回で検査本体が 0 / 3 以外で終わった」ときだけ揃う（注入で一致。記録・通知の漏れ / 未実測 / ロックは不一致）。`stage` / `dump_stages` が段の出力・記録・通知・スタブの呼び出し・温めを出す（`TAKO_1933_INJECT`）。手元で再現せず（夜間 300・単体 500・`run_step` 1500・git 並走 60 回で 0）
- 直した赤: ① `printf | grep -q` の assert が pipefail 下で取りこぼす（64 KB 超の注入で旧 5/5・新 0/5、Test 0）② 作りたてのスタブの初回起動（実測 1 回目 219 ms・2 回目 9 ms）が負荷下で Test 5 の上限 2 秒を超える → make_env で温める（初回 3 秒の注入で旧 4 FAIL・新 0）

## 2026-10-09（#1921: release-windows の rust-cache を外し、タグごとに一度も当たらないキャッシュを毎晩約 1.6 GB 保存して main / PR のキャッシュを押し出すのを止めた）
- 公式文書「タグの run は別のタグのキャッシュを読めない（読めるのは自分の ref と main）」と、v0.8.16〜v0.8.28 の 13 回すべての「No cache found.」+ 保存 1.60〜1.61 GB を実測。main は debug のキャッシュだけで release の target に効かない（依存の取得 約 60 秒 / その復元 約 55 秒）ので (a) save-if: false ではなく (b) 外す
- `scripts/test-cargo-root-manifest-key-1915.sh` に「タグで起動しうる workflow の rust-cache は 0 本」（判定の自己検査 8 通り）。34 PASS・注入 2 通り（修正前の yml / ci.yml にタグ起動）で名指しの FAIL・actionlint 0 件。所要は rust-cache の前後ステップ平均 108 秒ぶん縮む見込み

## 2026-10-09（#1918: セルフテストの器にも -f・tk-coretest の残骸を製品の掃除で回収・psmux の e2e も -f NUL）
- セルフテストの `-f` なし 6 か所（字面 48 / 68 / 73 / 103 + シェルへ打ち込む 1d / 61f。1d は固定名 `takoST` → pid 入り。103 / 61f は backend の conf）を直し、番犬を `mod self_test` とシェル文字列の `-L … new-session` へ広げた。`is_killable_socket_name` に `tk-coretest-` だけ足した（`tk*` は手書きなので据え置き）。psmux は CI で v3.3.7 / v3.3.8 を実測し `-f NUL` で既定の探索をしない → e2e を `Fixture::new_session` へ寄せた
- 実測: 偽の HOME の `.tmux.conf` がセルフテストで 5 → 0 回・番犬は土台の main.rs で 6 か所を file:line で名指し・回収の注入で新テスト 3 本が落ちる・psmux の `-f /dev/null` は v3.3.8 で設定警告
