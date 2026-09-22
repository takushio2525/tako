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

## 2026-09-21（#1490: 隔離 GUI の起動を scripts/lib の 1 実装へ寄せ、起動ごとに tako-vd を起こす）
- `scripts/lib/isolated-gui.sh` を新設（`isolated_gui_bins` / `launch_isolated_gui` / `wait_isolated_gui` / `stop_isolated_gui`）し、`virtual-display.sh ensure` を**起動の直前に毎回**通す形へ。`scripts/test-*.sh` 12 本を差し替え（-151 行）。Issue の grep は `"…virtual-display.sh" ensure` の引用符に当たらず全部 0 に見えていたが、正しく数えると「起動の直前に通していたのは 4 本 / 冒頭 1 回が 6 本 / 呼んでいないのが 2 本」で症状は実在
- 実測（眠った tako-vd から。`pmset displaysleepnow` で再現）: **master-launch が OK=0 rc=1 → OK=34 rc=0**・**1485 が OK=12 NG=14 → OK=26 NG=0**（14 NG は全部「窓を開かずに終了」= #1160 の中止）・**worker-min-width が OK=6 → OK=12**（3 腕のうち 1 腕しか走れていなかった）。他 9 本は前後一致・回帰 0
- 番犬 `issue1490_isolated_gui_launch_watchdog` 5 本（直書き 4 形 + 手書きの背景起動 / ensure を file:line で名指し）。注入で 2 本 FAILED → 戻して 5/5 緑。workspace 5021 passed 0 failed・clippy 3 宇宙 0・check-windows error 0

## 2026-09-21（#1493: main で赤かった test-remote-fs-1451.sh を現行契約へ合わせ、番犬で縛った）
- 真因は製品の回帰ではなく**テストの前提の追従漏れ**。#1452（`fd46c81`）が ①承認の呼び出し元ゲート（`remote_role.rs:432` `decide` を `remote.rs:5014` が引く）②「現より弱い role の要求は保留を作らない」（`remote_auth.rs:272`）の 2 つを入れ、`test-remote-launch-1449.sh` / `test-remote-master-launch.sh` には宣言を足したが、同日着地の `test-remote-fs-1451.sh` だけ漏れた。承認が 403 `upgrade_requires_gui` で端末が 1 台も登録されず **OK=18 NG=55**
- `TAKO_REMOTE_TRUSTED_ADMIN_NAMES="curl"` を宣言し、`pair_as` を「撃った」ではなく**「据わった」を確かめる**形へ（降格は `/api/admin/devices/role`）。番犬 `issue1493_admin_gate_script_watchdog` 3 本 + `.agent/conventions.md` に規約。**注入 A が素通りする穴（案内文の env 名を宣言と誤認）を実測で見つけて塞いだ**
- 実測: 1451 **73 PASS 0 FAIL**（#1451 の PR と一致）/ role-1452 58/0 / autostart-1485 26/0 / 注入 6 通りすべて file:line 名指し / workspace 5024 passed 0 failed / clippy 3 宇宙 0 / check-windows error 0

## 2026-09-21（#1491: たまり場の退避タブを「タブ形のカード 1 枚」にして、押せば復帰するようにした）
- タブ形の描画語彙を `tako-app::tab_shape` へ切り出し（寸法 / 状態ドット / 小バッジ / ボタンスロット）、タブバーと退避タブカードが**同じ 1 実装**を通る形に。退避タブは見出し + 「タブごと復帰」ボタン + ペインカード列 → **カード 1 枚**（状態ドット + タブ名 + ペイン数 + ×）になり、本体のどこを押しても既存の `unshelve_tab_clicked` で復帰。× はタブごと kill の 2 段確認（`stop_propagation` で本体と分離・発生源は `PaneButton` = #770 の `close:gui-tab` に混ぜない）
- 右パネルも同じカードで、配下ペイン行は `▸`（`CHEVRON_*`）で開く。ドロワー / 右パネルとも `render_shelved_tab_card` の 1 実装
- 実測: visual-test 項目 153（合成マウス）が `TAKO_VISUAL_TEST_OK`。A/B `TAKO_1491_LEGACY=1` は「タブ形カード 1 枚」で FAILED。実注入 2 通り（本体 on_click 削除 / 2 段確認飛ばし）で visual-test と番犬が `tab_shape.rs:203` を名指し。`test-shelve-tab-1487.sh` 24 PASS 0 FAIL・workspace 5024 passed 0 failed・clippy 3 宇宙 0・check-windows error 0。番犬 `issue1491_shelved_tab_card_watchdog` 3 本（注入 11 通り）

## 2026-09-21（#1496: 番犬の関数名追跡を 1 実装へ寄せ、pub(crate) fn の中の違反を正しく名指しする）
- 追跡が `strip_prefix("fn ")` だと `pub(crate) fn` / `async fn` が頭に見えず、中の違反が**手前の関数名**で報告されていた（検出は効くが名指しが嘘 = 緑のまま残る）。`tako_core::source_scan`（`fn_head_name` / `is_top_level_fn_head`）を新設し、同型 5 か所（#770 の 2 本 / remote_scrollback / issue841 / test_residue / setup_bootstrap）を差し替え。tako-core へ置いたのは tako-app の `#[cfg(test)]` と tako-control の tests の**両方**から引ける唯一の置き場だから
- 実測（実ファイルへの注入で A/B）: `main.rs:6202` が 旧 `shelved_tab_groups` → 新 `unshelve_tab_clicked`・`main.rs:8901` が 旧 `background_tab` → 新 `reattach_backgrounded_preview`。前者は Issue の症状そのもの
- 番犬 `issue1496_fn_head_watchdog` 2 本（直書きの再発 / 寄せ先の空振り）。注入 11 通りすべて FAILED + file:line 名指し（A 5 = 番犬が噛む / B 6 = 寄せた 5 か所の検出力が落ちていない）

## 2026-09-22（#1499: tako setup の依存チェック段で未検出の CLI 依存を [y/N] から入れられるようにした）
- 真因は回帰ではなく**#1057 の復活が `--review` 経路だけ**だったこと（`run_setup` は `run_dependency_check(review_mode && …)` のまま）。隔離環境でパイプ・実 PTY・`--review` の 3 通りを実走させ、前 2 つの出力が 1 文字も変わらないことで TTY 判定説を潰してから着手。当時の `標準setupは依存の質問をしない` が旧呼び出し形を文字列で固定しており、**直そうとすると番犬が止める**状態だった
- 判断を理由つき純粋関数 `setup_deps::offer_for`（`Ask` / `AutoInstall` / `Guide(cannot_run|check_only|legacy|no_terminal)`）1 本へ。CLI は表示と入力だけ。`interactive` 1 つが担っていた「依存を入れるか」と「設定値を見直すか」を `DepCheckMode` で分離（**#262 が守るのは後者**）。`--yes` は同意扱い・非 TTY は案内へ落として止まらない。再検出は `setup_deps::resolve`（検出と同じ規則）
- 実測: `scripts/test-setup-deps-prompt-1499.sh` **52 PASS 0 FAIL**（CI の macOS ジョブへ登録）。番犬 5 本・注入 7 通りすべて FAILED → 戻して緑。A/B `TAKO_1499_LEGACY=1` は端末でも聞かない = Issue の症状。workspace 5040 passed 0 failed・clippy 3 宇宙 0。**受け入れ検査 rework 1 回目**: CI の `/bin/bash`（3.2）は `set -u` 下の空配列 `"${arr[@]}"` を未定義扱いにして PASS=43 FAIL=9（手元の bash 5 では出ない）→ `${arr[@]+"${arr[@]}"}` へ直し規約を conventions へ

## 2026-09-22（#1502: tako CLI を外部ターミナルからも打てるようにした）
- `tako setup` の段として `$HOME/.local/bin/tako` へ symlink を張り、**その置き場所だけ**を `~/.zprofile` のマーカーブロック（1 組のまま）へ通す。**実体のディレクトリは PATH へ入れない**（`.app` の `Contents/MacOS` は tako-app ごと / dev の `target/debug` は依存クレートごと PATH へ出るうえ、`.app` を動かすと黙って切れる）。`$HOME/.local/bin` は 3 系統のランチャーと同じ置き場所なので macOS ではブロックの中身は 1 ディレクトリのまま = 既存ユーザーの profile の形が変わらない
- 途中で踏んだ真因 2 つ: ①claude が ready だとエージェントの PATH 段は走らないので「ついで」に載せると設置されない（= `run_setup` の独立した段にした）②「もう通っているか」を**プロセスの PATH** で測ると #601 の注入で必ず「通っている」に見える。`$SHELL -l -c` も**継承した PATH を `path_helper` が引き継ぐ**ので、launchd の既定 PATH へ戻してから起こす必要があった
- 実測: `scripts/test-tako-cli-path-1502.sh` **49 PASS 0 FAIL**（隔離 HOME + 隔離 GUI）。A/B `TAKO_1502_LEGACY=1` は 23 FAIL（B7 / B8「新しいログインシェルで tako が見つかる・動く」= Issue の症状）。番犬 `issue1502_tako_cli_path_watchdog` 6 本（注入 8 通りすべて FAILED → 戻して緑）

## 2026-09-22（#1516: `orchestrator self --pane N` が名指しどおりそのペインを答えるようにした）
- 真因は**入口が 2 種類の問いを同じ `pane` 欄へ混ぜていた**こと。受け手の解決順は「確かな順」= pid 祖先辿りが最優先で `pane` は stale になりうる env 由来として扱う契約（#288 / #210）なので、`--pane` は毎回黙って負けていた（pane 1954 から `--pane 1964` → `pane_id: 1954`）。MCP は caller_pid を持たないので pane_id は動くが `caller_role` が残り profile だけ呼び出し元のものになる
- 組み立てを `Request::orchestrator_self`（protocol.rs）の 1 本へ寄せ、**名指しなら呼び出し元の手掛かりを 1 つも載せない**。自分を名指しした場合は「私についての問い」に倒す（solo の profile は env の `solo:<名前>` にしか無い）。受け手は `named_pane` で名指しを見分け、解けなければ role 検索へ落とさず `PaneNotFound`（#1466）。応答の `role` は名乗り → 対象ペインのラベル、master / solo でなければ理由を `warnings` へ。`adopt` / `guide` / `handoff` の `--pane` は同型のまま（寄せるのは入口 1 か所で済む）
- 実測: 本番 GUI（旧バイナリ）に対して修正 CLI が `pane_id 1964 / profile <名指し先のもの> / profile_source pane_role`（出荷 CLI と `TAKO_1516_LEGACY=1` は 1967 = 症状）。`scripts/test-self-pane-1516.sh` **28 PASS 0 FAIL**（隔離 GUI・CLI / MCP 両経路 / A/B / エッジ 3 種）。実注入 6 通りすべて FAILED + file:line 名指し（CLI へ戻すと e2e が 11 NG）。番犬 `issue1516_named_pane_watchdog` 3 本・workspace 5047 passed 0 failed・clippy 3 宇宙 0・check-windows error 0
