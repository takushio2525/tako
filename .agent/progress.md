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

## 2026-09-11（#632: 承認カード e2e 3 本は #1089 で修正済みと実測確定）
- 現状 main で `screenshots-5b.spec.js` は 9/9 PASS・PWA e2e 全 6 spec も 50/50 PASS。`cf85756`（#1089 / PR #1100）が #632 の「対応案」（モックへ `permission_dialog` / assert を `/respond` + `choice`）を既に実装していた
- A/B（`cf85756^` の spec を現行実装へ当てる）で `.approval-card` の 10 秒タイムアウト × 3 を再現 = 症状は実在。旧契約（`/input` へ `y`/`n`）の grep は 0 件、境界の選択肢 N=2 / N=1 も一時 spec で PASS
- コード変更なし（install 不要）。実出力を付けて #632 を close し、真因（PWA e2e が CI で 1 度も走らず、実行手順が package.json / README のどこにも無い）を #1357 として起票した

## 2026-09-11（#1365: 衝突で CI の run が作られない状態を待たずに名指しで案内するようにした）
- base が進んで衝突すると GitHub は merge コミットを作れず `pull_request` の run を作らないので、期待名が永久に未登録のまま `wait-pr-checks.sh` がタイムアウト（2400 秒）まで待っていた（#775 の PR #1359 で 3 回）。未登録が残るあいだだけ `gh pr view --json mergeable` を引き、**2 回連続**で衝突を観測したら終了コード 4 + 取り込み手順を出す形へ
- 判定・案内文・終了コードは `scripts/lib/pr-conflict.sh` の 1 実装で、`merge-pr.sh` の門（待つ前 / merge 直前）も同じ言い方になる（CONFLICTING の拒否は 1 → 4 へ変更）。`UNKNOWN` = 計算中・空文字・`gh pr view` が引けないときは待ちを続ける
- 実測（既定間隔 20 秒）: 21 秒・ポーリング 2 回で 4。A/B `TAKO_1365_LEGACY=1` は同じ入力で待ち続ける（上限 60 秒で打ち切り = 2）。モック 106 PASS / 0 FAIL（従来 72。Test 20〜23 を追加）

## 2026-09-11（#372: 器を持たないペインも sleep guard の busy に数えた）
- 走査対象が器のセッションだけで、tmux 未導入 / persist OFF（cask の既定）では常に空 = `busy_agents` が無条件に 0。全ペイン対象 + 器なしは PTY 直下の子から辿る二段構え（判定 `has_running_descendants` / 数え方 `busy_count()`）へ。CLI の `status` も IPC でアプリの値を採る（保持フラグと busy はプロセスローカル static）
- 隔離 GUI（persist OFF・tako-vd）: 修正前は `sleep 300` 稼働 75 秒で 0 のまま → 修正後 1 + pmset に assertion、停止で 2 秒で解放。器あり構成も回帰なし。A/B `TAKO_372_LEGACY=1`・番犬 4 本が修正前ソースで file:line 名指し FAILED

## 2026-09-11（#633: 承認カードの command を「承認を求めている操作」から始める）
- 実測で真因を絞った: 本文の境界は「当たったら捨てる」3 つ（罫線 / 0 桁の非空行 = #1293 / 番号つき行）しか無く、**罫線を引かず箱ごと 0 桁で描く**許可ダイアログ（agy / claude の箱なし 2 形）では捨てる材料が無い。Issue 実測値の `⏺` 行そのものは #1293 が既に切っていた
- `dialog::BODY_START_MARKERS` + `body_start_row` で**本体の開始マーカーを起点**にし、無ければ従来のブロック抽出へフォールバック（起点を下げるだけ = 結果は必ず従来の接尾辞）。FR-2.25.12 / conventions #1293 節に追記
- 番犬 `crates/tako-control/tests/issue633_permission_command_anchor.rs` 8 本 + 単体 3 本。A/B `TAKO_633_LEGACY=1` で 2 本 FAILED（agy に発話が混ざる / 罫線の無い画面が `⎿` 行から始まる）

## 2026-09-11（#651: 狭い実行ペインで割れた exit マーカーを拾えるようにした）
- 折り返しの判定を `TerminalSession::visible_lines_filled`（alacritty の `line_length()` = WRAPLINE + 占有列数）で**列**で持ち、`dispatch::find_exit_marker` が「埋まった行の行末 → 次の非空行の行頭」だけをまたいでマーカーを再構成する形へ。数字のあとは行の残りが空白であることを要求するので無関係な行は繋がない（案 2 の OSC 化は器越え + Windows 実機が要るので Issue へ理由を残して見送り）
- 隔離 GUI 実測（直接 PTY と tmux バックエンドの両方）: 幅 7 / 10 / 13 / 40 桁すべて `exited exit_code=0`・幅 10 桁の `--wait` が返る（修正前は 40 秒返らない）・3 桁の `127` は幅 13 桁で `__TAKO_EXIT=1` + `27` に割れても 127。legacy アーム（`TAKO_651_LEGACY=1`）は永久 running と **`exit_code=1` の誤報**を再現
- 番犬 2 本（製品コードへのマーカー literal 増殖 / 文字数で測る物差し）+ 実 PTY の単体テスト（幅 10 桁で割れる形と全角で埋まった行）

## 2026-09-11（#1372: exe::find が npm シムの裸スクリプトを PATHEXT より先に返さないようにした）
- 真因は `find_in_windows_path` が `<base>\<name>` を名前によらず採っていたこと。npm の cmd-shim は `<name>`（`#!/bin/sh` のスクリプト）/ `.cmd` / `.ps1` を置くので `find("claude")` が PE でないスクリプトを返し `Command::new` で起動できない一方、同じモジュールの `is_executable_file` は同じパスに false = 境界の中で答えが矛盾していた。土台をそのまま採るのは「名前が既に `PATHEXT` の拡張子を持つとき」だけへ（`resolve_with_pathext` へ切り出し、パス指定の経路も同じ判定を通す）
- 修正前ソースで新テスト 3 本が `exe.rs:411` / `:422` / `:466` を名指しで FAILED（`Some("…\\npm\\claude")` = Issue 実測値の逐語再現）。修正後は exe 15/15・workspace 4311 passed / 0 failed・`scripts/check-windows.sh` error 0。パス指定の経路だけ旧実装へ戻す A/B で整合テストが `C:\tools\claude` を名指しで落ちる
- 併せて `platform_parity.rs` の B16 免除 3 件の根拠を「`.exe` なら解決できる」へ訂正（std の `resolve_exe` は PATH 探索で拡張子が無いときに `.exe` を足すだけ = `.cmd` シムには届かない）。Windows 実機での `where.exe` 比較は未検証

## 2026-09-11（#1371: Windows の URL 起動から cmd.exe を外した）
- 真因は `cmd /C start "" <url>`。`std::process::Command` の Windows 実装は空白を含まない引数を引用符で囲まない（`Quote::Auto`）ので、cmd.exe が引用符の外の `&` をコマンド区切りとして解釈する。tako が拾う URL は構造的に空白を含まず `&` は正規の文字なので、クエリ文字列つきリンクは常にそこで切れ、残りが別コマンドとして走っていた（画面 / PDF / Markdown が第三者由来ならクリック 1 回で任意コマンド実行）
- `open_url` / `open_url_wait` を既存の `shell_execute`（`ShellExecuteW`）へ寄せ、引数の正本を境界の外の純粋関数 `windows_url_launch` に置いた（macOS からも Windows の形を検査できる）。待つ版は戻り値判定（> 32）で足りる = `SEE_MASK_NOCLOSEPROCESS` はハンドラ本体のハンドルを返すのでブラウザを閉じるまで返らない
- 番犬 5 本（シェル起動のソース走査 / ShellExecuteW への配線 / メタ文字 URL が 1 つの値のまま / `&` 以降が落ちない / 名指しの行番号がずれない）。注入 3 種（旧 cmd 経路・`&` で切る・lpParameters へ載せる）が file:line 名指しで FAILED。**Windows 実機は未検証**。到達経路側（PDF / 提案チップのスキーム検査なし）は #1376 へ分離

## 2026-09-11（#1373: 蓋閉じ継続の記録に所有者を持たせ、原子書き込み + fail-loud にした）
- `lid-guard.json` は data_dir に 1 つで複数の tako-app が共有するのに「書き換えるのはこのプロセスだけ」が前提だった。記録へ所有者（pid + 起動時刻）を足し、戻すのは「自分の / 所有者が死んだ / 所有者を持たない旧形式」だけへ（**生きた他プロセスの記録は倒す側も解除側も触らない**）。判定は `claim_for` → `decide` の純粋関数 2 本で、probe を引数に取るので macOS の CI で 4 通り全部を固定できる
- 書き込みを `config_io::atomic_write` + `<path>.lock` の flock（**書くと決まってから**取り、その下で読み直す）へ。読めない記録は「記録なし」へ丸めず `<name>.unreadable.bak` へ写して Err（元ファイルは触らない）。旧形式は `serde(default)` でそのまま読めるので移行は不要（指紋へ `SavedLidState` / `RecordOwner` を登録）
- A/B: 修正前ソースで番犬 6 本が file:line 名指し FAILED。注入 5 種それぞれで対応する番犬 / 単体が落ちる。**Windows 実機（2 プロセス構成）は未検証**（電源待ち）。`main.rs` の `apply_sleep_guard` がセカンダリで止まらない件は所有権で無害化したので別 Issue は立てない

## 2026-09-11（#1308: 実 PTY の fixture を待つテストを状態待ちへ寄せた）
- 真因は「固定窓の待ち + 尽きても結果を検査せず素通り」。混んだ機で素のシェルの起動が固定 10 秒窓を超えると、起動前の PTY へ打ち込んだ行はエコーされるだけで実行されず、それでも `dispatch` するので panic が**「器越しへ倒れている（#1200）」という無関係な原因**を名指ししていた（実測 `prompt_ok=false waited=10.03s dialog_seen=false waited=20.04s load=3.89`）
- 待ちを `i1308_wait_for_state`（状態待ち + `state_wait_budget`・**上限に達したらドライバ自身が panic**・届かないあいだは予算に比例した間隔で打ち直す）へ寄せ、呼び出し側が素通りできない構造にした。番犬 `issue1308_pty_wait_watchdog` 4 本が注入 5 通りを file:line で名指し
- 高負荷 A/B（全件走 18 スレッド）: legacy `TAKO_1308_LEGACY=1` は **3/35 FAILED**（69.5〜70.7 秒・load 66.8〜68.9・Issue と同一の panic 本文）、新アームは **0/35 FAILED**（load p50 96.9・最長 74 秒の run も通過）。注入 `TAKO_1308_INJECT=late`（`cat </dev/tty` で打鍵を食う）は legacy 確定 FAILED 30.09 秒 / 新 ok 15.50 秒
