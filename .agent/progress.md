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

## 2026-09-11（#1362: 全選択 Cmd+A がターミナルでは効かないことを実測し docs を実態へ寄せた）
- 隔離 GUI（tako-vd・`CGEventPostToPid`）で確定: ターミナルの cmd+A は選択を作らず PTY にも届かない（`abc` に cmd+A → `x` で `abcx` 3/3。本物の Ctrl+A は `xabc` 3/3 = 検出力あり）。cmd+A → cmd+C も sentinel のまま 3/3
- 同じ経路でプレビュー本文 3/3・編集中バッファ 3/3 は全文が入るので「端末には実装が無い」が確定。表の行へ効く先を書き注記を 1 つ追加（コード変更なし = install 不要。案 (b) = ターミナルの全選択は別 Issue 候補として Issue へ残した）
- 番犬 2 本（説明文 ↔ `select_all_text` の両方向 / 効く先の列挙）。注入 5 通りで file:line 名指しの FAILED → 復帰後 7/7 緑

## 2026-09-11（#1383: clippy が単体形と workspace 形で違う lint を見る理由を確定し CI へ 1 本足した）
- 真因は feature unification。`--workspace` は必ず gpui を含むので `serde_json/preserve_order` が有効 = `Map` が IndexMap 実装 → `Value` が**有意な Drop** を持ち `unnecessary_lazy_evaluations` が黙る。gpui 抜きの `-p` 宇宙は BTreeMap 実装（insignificant）なので同じ行で落ちる
- `wait.rs:1099` を `then_some` へ（挙動不変）。CI の macOS ジョブへ `-p tako-core -p tako-control -p tako-cli` の clippy を追加（温まっていれば実測 10.5 秒）
- 番犬: 修正前の行を戻すと新ステップが EXIT=101・既存の workspace ステップは EXIT=0 で見逃す（実出力で確認）

## 2026-09-11（#1376: PDF のリンク注釈と提案チップにスキーム検査を通した）
- 判定を `tako_core::url_guard` の 1 実装へ（`check_browser_url` = http / https / `check_os_handler_url` = 境界 B8 の許可集合。`md_links::browser_url` は再公開）。経路側（(a)）= `follow_pdf_link` / `follow_preview_pdf_link` / `open_preview` と、保険（(b)）= `os_integration::open_url` / `open_url_wait` の両方を締めた。弾いたら通知欄 + 理由の分類だけを persist.log へ（リンク文字列は出さない）
- 隔離 GUI（tako-vd・`open` の PATH shim で OS ランチャの引数を観測）: 最小 PDF の `file:///Applications/Calculator.app` は `許可していないスキーム` で弾かれ **`open` は 1 回も起きない**、`https://example.com/ok?a=1&b=2` は `opened_url` + shim に 1 つの値のまま到達
- 番犬 8 本（経路 3 か所・B8・大域走査・診断の中身・1 実装・行番号の物差し）。修正前ソースで 6/8 が file:line 名指し FAILED（`main.rs:1096` / `11648` / `20751` = Issue 記載の 3 行 + `os_integration.rs:88` + `md_links.rs`）。Windows 実機は未検証

## 2026-09-12（#1387: NFD の結合文字が画面テキストから落ちるのを 3 経路まとめて直した）
- alacritty が `Cell::extra.zerowidth` に持つ 0 幅の結合文字をどこも読んでいなかった（grep 0 件）ので、`screen::cell_text` / `push_cell_text` / `cell_is_trailing_blank` の 1 実装へ寄せ、`resolve_cell`（Screen = 描画 / links）・`compose_grid_row`（tail_lines）・`history_plain_lines`（ペインログ）の 3 経路と #801 の空白セルの近道を通した。`text` へ積んだぶんは `cell_cols` へ同じ列を積む
- 実測（隔離 GUI・tako-vd）: `printf 'e\xcc\x81'` の画面が `U+0065 U+0301`・NFD 名のファイルを `find` で出すと `tako links` の target が `U+304B U+3099` を保って実在 true（濁点を落とした形は false）。legacy アームでは新テスト 6 本 + 番犬 3 本が file:line 名指しで FAILED（`画像かある.txt` / `VIS_か_e` を逐語再現）
- 番犬 `issue1387_combining_watchdog` 5 本 + 実 PTY の 4 経路一致テスト（visible_lines / tail_lines / history_plain_lines / selection_text）。hot path は 120x24 を 3000 回 snapshot で 68.6〜69.3ms → 65.3〜67.9ms

## 2026-09-12（#1370: IPC 由来のレイアウト変更のあと 1 フレーム強制描画するようにした）
- 真因は「cols / rows の書き手が描画の中だけ」+「macOS の gpui では notify でフレームが作られない」（display link は窓が可視でないと起動しない）。IPC ループ（全 dispatch が通る 1 箇所）で**レイアウトを変える Request のときだけ**応答を返す前に全ビューポートを 1 フレーム描く形へ。判定は `protocol::changes_layout` の純粋関数 1 実装（ワイルドカード無し = 155 バリアント全網羅・読み取り系は偽）
- 隔離 GUI（tako-vd・入力イベント無し）の A/B（`TAKO_1370_LEGACY=1`）: resize は legacy **0/5**（20 秒待っても cols/rows 不変・rect だけ動く）→ 新 **5/5**（応答直後に反映）。equalize / split（新ペインが 80x24 のまま 2 枚 → 0 枚）/ theme toggle（`ipc_frame` +0 → +1）も同様。**`list` × 30 で `ipc_frame` +0**（読み取りは描かない = #786 の固定費が乗らない）
- 隔離セルフテスト項目 22b（意図的に `notify_and_draw` を呼ばない）は新で ok（rows 13 → 5）・legacy で FAILED（25.1s 待って届かず）。番犬 `issue1370_ipc_redraw_watchdog` 8 本が注入 5 通りを file:line で名指し。全体は `TAKO_APP_SELF_TEST_OK` で完走

## 2026-09-12（#1390: terminal-core のテストの穴 4 件を埋めた）
- 項目 1/3 はテスト追加（スクロール中の `visible_lines_filled` / `set_scrollback_limit` の副作用）、項目 2 は `history_plain_lines` の末尾トリムを `compose_grid_row` の 1 実装へ寄せて履歴行をスクロールで可視化して突き合わせ、項目 4 は罫線剥がしを `strip_box_border` 1 実装へ（`screen.rs` / `terminal.rs` / #1387 の番犬）
- 項目 4 は「2 行目以降だけ剥がす」最小修正だと**枠線つきでプロンプト行にだけ本文がある箱が false** になるので、プロンプト行も同じ作法で見る形にした（1 行の箱の誤答は修正前から在った）
- 注入 6 通り（起点 1 ずらし / トリム規則 / ループ 2 実装化 / `kitty_keyboard` 落とし / `set_options` 直渡し / 罫線剥がし戻し）で file:line 名指しの FAILED。workspace 4381 passed 0 failed・check-windows error 0

## 2026-09-12（#1388: 読み手のいない「行に全角が在るか」の旗を落とした）
- `ScreenLine` の旗は #787 以降 production の読み手が 0（grep で確定）なのに毎行 `windows(2)` を走らせ、doc だけが「描画で使う」と言っていた。フィールドと書き手 4 か所（`screen.rs` / `links.rs` ×2 / `terminal_grid.rs`）を落とし、assert 3 件は「全角のぶん `cell_cols` が 2 列飛ぶ」へ置き換え
- 修正前ソースの実測: 右端だけが全角の行は旗が **false**（`windows(2)` 版の構造的な見落とし）/ 途中に全角なら true。`text` / `cell_cols` は前後で完全一致（右端全角・空行・全角のみ・途中全角の 4 ケースを `screen_from_lines` で固定）
- 番犬 `issue1388_has_wide_watchdog` 6 本（識別子の再登場 / 旗を組む形 / フィールド指紋 + 注入 3）。修正前ソースで 3 本が file:line 名指し FAILED（`screen.rs:55` / `476` / `481`・`links.rs:122` / `127`・`terminal_grid.rs:627`）

## 2026-09-12（#1389: visible_lines_filled の「行末が全角」の取りこぼしを限界として固定した）
- `line_length()` は末尾から `cell.c != ' '` を探すので全角の後続セル（`WIDE_CHAR_SPACER`）を空きと数える = 行末が全角の行は右端まで埋まっていても `filled=false`。挙動は変えず、doc の「既知の限界」+ `.agent/conventions.md` #1283 節（兄弟実装 `combined_screen_text` と**同じ穴**であること）+ 10 桁の実 PTY で 6 形を採る単体テストで固定した
- 実測（10 桁）: A 行末が全角・以降の出力なし = `line_length=9 filled=false WIDE_CHAR_SPACER` / E `CUP` で描き直し = 同じ / B 続きあり = `WRAPLINE` が立って 10・true / C・D・F は従来どおり。現行の消費者 `find_exit_marker` は全 ASCII のマーカー断片しか見ないので実害は無い（(b) 判定の修正は別 Issue 候補）
- 番犬 `issue1389_filled_wide_limit_watchdog` 5 本が doc / 規約 / 固定テストの欠落を file:line で名指し（注入 4 通りで確認）。製品コードの差分は doc コメントのみ・削除 0 行 = **install 不要**

## 2026-09-12（#1367: 器なしペインの busy を close 確認 / GUI 判定 / agent_running へ届けた）
- #372 で走査は器なしペインも数えていたのに**引く側**が器のセッション名（`busy_sessions`）を見たままで、tmux 未導入 / persist OFF（= cask の既定）では close 確認（#566）が出ず・`busy_children`（#694）が false でスターターが被り・`agent_running` も false だった。問う口を `RunningChildrenScanState::is_pane_busy` の 1 実装へ寄せ、3 経路を `TakoApp::pane_has_busy_children` から通した（旧キャッシュ `busy_backend_sessions` はフィールドごと削除）
- 隔離 GUI（tako-vd・persist OFF・実 Cmd+W = `CGEventPostToPid`）の A/B: legacy（`TAKO_1367_LEGACY=1`）は `busy_agents=1` なのに `busy_children=false` / `display=starter` / **確認なしで即 close**、新は `busy_children=true`（1.0s）/ `terminal` / ダイアログが出てペインが残る。停止後は確認なし・承認経路の監査ログ（`close:kbd`）も器なしで残る
- 番犬 5 本 + A/B + 単体 5 本 + セルフテスト項目 73g（legacy アームで `waited=93.1s busy=false` の FAILED を実測）。注入 5 通りが file:line 名指し。**チャットの列挙（`collect_chat_targets`）は器つき前提のまま**なので器なしの会話表示は別 Issue

## 2026-09-12（#1004: リモート / SSH の手順を新 topic `remote` へ明文化した）
- 手順の全文を `guides/remote.md`（6,903 B）へ置き、prompt 側は master の topic 表 1 行 + solo の `### Remote / SSH` 節だけ（`tako context-budget` 実測 master +83 B / solo +242 B = 依頼上限 500 B 内）。移送でない本文は fixture へ宣言する道（#1154 の `guides_added_after_1154.md`）に乗せた
- 実測で Issue 案と食い違った点を本文へ反映: ①`ssh_config` は **`Include` を読まない**（ssh 自身は読む = 一覧に出ないホストへ名前指定で繋がる）②config の `Port` が 22 以外だと **tako 自身が開いたペインも自動検知に見送られる**（`-p` を渡すため）。失敗 5 種（unresolved / refused / auth / hostkey / conflict 経路）を実 sshd 相手に end-to-end で確認
- 番犬 `remote_guide.rs` 7 本（topic 消失 / master・solo のトリガー消失 / トリガーの肥大 / 手順の prompt インライン化）を注入 5 通りで file:line 名指し FAILED。隔離 solo の実会話は `tako_ssh_hosts` → `tako_orchestrator_guide{topic:"remote"}` → `remote_folder ls` → 接続情報の確認要求へ到達
