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

## 2026-09-11（#1353 / #1364: セルフテスト 3 項目の固定予算を状態待ちへ、番犬を状態読みへ広げた）
- 項目 22（固定 800ms）/ 44（固定 1 秒）/ 63（固定 6 秒窓 + 40 秒）を `wait_for_app_state` + `cli_state_budget` へ。**項目 63 の真因は待ち不足ではなく証拠源の取り違え + 再描画の不発**（旧は描画を `focused_pane()` で見て分割元で真になる偽陽性 = legacy 実測 `pane=Some(2)`。新ペインは誰も汚さないと `AnyView::cached` のまま一度も描かれず、シェルが 1 行も出さない）→ 作った ID のペイン + `wait_for_drawn_state`（毎周期 notify + draw）
- 番犬 `打ち込んだcliの結果を固定予算で待っていない` を追加（アンカー = 手前の `type_text` + `{cli}`）。修正前ソースで 3 項目を file:line 名指し（`38244` / `39998` / `41809`）。同型 16 件は `KNOWN_FIXED_CLI_WAITS` で段階導入 = #1375 で空にする
- 実測: 高負荷 4 回（load 73〜142）で 3 項目 0 FAILED・うち 2 回は完走。**人工負荷では旧も落ちない**（`yes` 68 本 load 134 で `waited=0.1s budget=0.8s`）ので注入で確定: `late` で旧 3/3 FAILED・新は待って通る / `never:44` は新でも FAILED

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

## 2026-09-11（#1353 / #1364: セルフテスト 3 項目の固定予算を状態待ちへ、番犬を状態読みへ広げた）
- 項目 22（固定 800ms）/ 44（固定 1 秒）/ 63（固定 6 秒窓 + 40 秒）を `wait_for_app_state` + `cli_state_budget` へ。**項目 63 の真因は待ち不足ではなく証拠源の取り違え + 再描画の不発**（旧は描画を `focused_pane()` で見て分割元で真になる偽陽性 = legacy 実測 `pane=Some(2)`。新ペインは誰も汚さないと `AnyView::cached` のまま一度も描かれず、シェルが 1 行も出さない）→ 作った ID のペイン + `wait_for_drawn_state`（毎周期 notify + draw）
- 番犬 `打ち込んだcliの結果を固定予算で待っていない` を追加（アンカー = 手前の `type_text` + `{cli}`）。修正前ソースで 3 項目を file:line 名指し（`38244` / `39998` / `41809`）。同型 16 件は `KNOWN_FIXED_CLI_WAITS` で段階導入 = #1375 で空にする
- 実測: 高負荷 4 回（load 73〜142）で 3 項目 0 FAILED・うち 2 回は完走。**人工負荷では旧も落ちない**（`yes` 68 本 load 134 で `waited=0.1s budget=0.8s`）ので注入で確定: `late` で旧 3/3 FAILED・新は待って通る / `never:44` は新でも FAILED

## 2026-09-11（#1362: 全選択 Cmd+A がターミナルでは効かないことを実測し docs を実態へ寄せた）
- 隔離 GUI（tako-vd・`CGEventPostToPid`）で確定: ターミナルの cmd+A は選択を作らず PTY にも届かない（`abc` に cmd+A → `x` で `abcx` 3/3。本物の Ctrl+A は `xabc` 3/3 = 検出力あり）。cmd+A → cmd+C も sentinel のまま 3/3
- 同じ経路でプレビュー本文 3/3・編集中バッファ 3/3 は全文が入るので「端末には実装が無い」が確定。表の行へ効く先を書き注記を 1 つ追加（コード変更なし = install 不要。案 (b) = ターミナルの全選択は別 Issue 候補として Issue へ残した）
- 番犬 2 本（説明文 ↔ `select_all_text` の両方向 / 効く先の列挙）。注入 5 通りで file:line 名指しの FAILED → 復帰後 7/7 緑
## 2026-09-11（#1383: clippy が単体形と workspace 形で違う lint を見る理由を確定し CI へ 1 本足した）
- 真因は feature unification。`--workspace` は必ず gpui を含むので `serde_json/preserve_order` が有効 = `Map` が IndexMap 実装 → `Value` が**有意な Drop** を持ち `unnecessary_lazy_evaluations` が黙る。gpui 抜きの `-p` 宇宙は BTreeMap 実装（insignificant）なので同じ行で落ちる
- `wait.rs:1099` を `then_some` へ（挙動不変）。CI の macOS ジョブへ `-p tako-core -p tako-control -p tako-cli` の clippy を追加（温まっていれば実測 10.5 秒）
- 番犬: 修正前の行を戻すと新ステップが EXIT=101・既存の workspace ステップは EXIT=0 で見逃す（実出力で確認）
