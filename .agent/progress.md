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

## 2026-09-12（#1403: remote daemon の HTTP 受信を少数ワーカーへ並列化した）
- 受信ループを `serve_http_requests` へ切り出し、`Arc<tiny_http::Server>` を既定 4 本（`TAKO_REMOTE_HTTP_WORKERS` で 1..=32）のワーカーが recv する形へ。合流は `thread::scope`（忘れられない）・1 本の panic は `catch_unwind` で監査ログへ・recv 破損は全員で降りる。daemon → app の IPC は `with_app_ipc` の 1 実装が往復の間ロックを握る（呼び出し 4 か所を集約）。波及で `append_audit` を 1 行 1 write へ（書く者が増えたので `writeln!` だと行が混ざる）
- 隔離 daemon（偽 tailscale の whois 3 秒・本番 pid 64092 は不可侵）の A/B: 修正前 B = **2.71 秒** → 新 **0.0004 秒**（A は 3.03 秒のまま）。`TAKO_1403_LEGACY=1` / `TAKO_REMOTE_HTTP_WORKERS=1` はどちらも 2.72 秒で旧挙動を再現。飽和の限界も実測（4 本同時で health 2.51 秒・6 本で溢れた 2 本が 6.04 秒）
- 番犬 `issue1403_http_workers_watchdog` 7 本 + 単体 6 本。注入 9 通りで file:line 名指し FAILED（IPC は `left: 4 / right: 1`）。**mid-file の `#[cfg(test)]` は禁止**（#1401 番犬の走査範囲が切れる）・`LEGACY_ARM` マーカーは実時間アサート用なので付けない。workspace 4487 passed 0 failed × 5・check-windows error 0

## 2026-09-12（#1398: ディレクトリへのシンボリックリンクがファイル扱いになるのを直した）
- 種別を「**辿った先**」で決める 1 実装（`filetree::entry_is_dir`。リンクのときだけ追加 `metadata`）へ寄せ、開く側（`OpenFile` の `is_file()` / `open_plan::route`）と向きを揃えた。辿って初めて起こる循環は `collect_rows` が canonical パスの照合で打ち切り、**打ち切りを行として見せる**（`RowNote::Error`・描画はリモート行と同じ `render_note_row` の 1 実装）
- 修正前ソースの実測: `link` 行が `("link", 1, is_dir=false)` で `toggle_dir` しても増えない → 修正後は展開でき `inner.txt` が depth 2 に出る。実注入の A/B は `file_type()` へ戻すと番犬が `filetree.rs:699` / `:690` を名指し FAILED、照合を落とすと祖先リンクの 2 周目（depth 3 に `README.md`）が出て loop テスト 2 本が FAILED
- 単体 10 本追加（切れたリンク / 相対 / `..` / 相互 ELOOP / 往復 / 500 件超 / git のしるし / 開く経路）+ 番犬 3 本（注入 5 通り）。workspace 4488 passed 0 failed・clippy 両宇宙 0・check-windows error 0。**install 要**

## 2026-09-12（#1375: セルフテストの「固定予算 + CLI の状態読み」16 件を状態待ちへ移し既知リストを空にした）
- 項目 18 / 19 / 21 / 23〜28 / 47 / 47b / 50 / 51b / 66b / 73c / 73f を `wait_for_cli_state`（`wait_for_app_state` + `cli_state_budget` の 1 実装。A/B の口・注入・診断行 `TAKO_SELF_TEST_1375` をここへ集約）へ寄せ、`KNOWN_FIXED_CLI_WAITS` を空にした。分割して新ペインを操作する 4 件は `split_focus_new_pane` で「着地 → アイドル」の 2 段に割り、以降は**返ったペイン ID** を見る（旧 73f は**打ったあとに**分割前のフォーカスを読んでいたので、着地が先だと窓を使い切るまで真にならない = 待ちを伸ばしても直らない形）
- 実測（隔離 GUI・tako-vd）: `INJECT=late` 全項目で 17 か所とも `ok=true`（`waited` = 旧予算 + 5 秒）で完走 / `LEGACY=all` は旧の固定予算（0.8〜15.0s）を再現して完走 / 項目ごとの `LEGACY+late` は **17/17 FAILED**（73f は Issue が観測した `73f: split で新ペインへフォーカスが移らない` そのまま）/ `never` は新経路でも **16/16 FAILED**。高負荷 3 回（load 6.5〜8.4）と load 10〜37 は完走、load 65〜80 の人工負荷では 73c が 4 倍上限（80 秒）を使い切って FAILED = 上限の政策どおり
- 予算の不等式は手書きの表をやめ**ソースから採った 21 か所**を検査（`cli_wait_budgets`）。番犬は空リストで緑

## 2026-09-12（#1397: 器なし（tmux 無し / persist OFF）でもチャットビューが立つようにした）
- 原因は 3 つ重なっていた: ①列挙が `backend_sessions` 起点 ②live 解決のキーが器のセッション名だけ ③**判定表が alt screen をチャットより先に見る**（器なしでは claude の TUI 自身が alt screen = Issue に無かった 3 つ目）。キーを `agents::LiveSessionKey`（器あり = セッション名 / 器なし = (ペイン ID, PTY 直下の子 pid)）へ広げ、列挙を `terminals` 起点に、表を「チャット確定 → alt screen」へ
- 隔離 GUI（tako-vd・tmux サーバー無し・persist OFF）の A/B: 新 = `pane_display=chat`（`alt_screen:true` / `claude_chat:true`）で実会話も読める。legacy（`TAKO_1397_LEGACY=1`）は実 claude TUI が生きたまま 21 サンプル（約 105 秒）すべて `terminal`。器あり（persist ON）と混在は前後どちらも `chat` で不変
- 番犬 9 本 + 単体 8 本（注入 7 通りで file:line 名指し）。波及で **#853 の fixture 保護が器の有無を問わず必須**になり、旧順序を固定していた項目 94 の alt screen 節（#702）を新規則へ寄せた（`main.rs` は 9 行）。隔離セルフテスト完走・legacy では項目 94 が落ちる

## 2026-09-12（#1417: 右パネル・プレビューの失敗も同じ通知欄へ出した）
- #1399 の番犬が `KNOWN_DISCARDED` に残していた同型 3 件（`right_panel.rs:TmuxSelectWindow` / `preview_render.rs:PreviewOutline` / `PreviewView`）を同じ 1 実装へ寄せた。出し口は `notify_tree_failure` → **`notify_ui_failure(area, ..)`** へ改名し、画面は `sidebar::NoticeArea`（persist.log の `area=` と A/B の逃げ道の選択）だけで区別する（3 実装目を作らない）。クリックの中身は `render` のクロージャから名前付きハンドラ 3 本へ切り出した（合成マウスが届かないのでセルフテストが叩ける名前が要る）
- 隔離 GUI（tako-vd）セルフテスト項目 84c の A/B: 新 = `win="window 切替 に失敗しました（9999:gone）: ペイン 3 に tmux セッションがない…"` / `outline="目次へ移動 に失敗しました（存在しない見出し）: …"` / `page="ページ移動 に失敗しました（ページ 999）: …"` / 上書き・閉じた直後の再失敗・成功時無言すべて取得 → 完走（`TAKO_APP_SELF_TEST_OK`）。legacy（`TAKO_1417_LEGACY=1`）は 3 つとも `None` で **FAILED**。persist.log は `area=right_panel op=window 切替 分類=operation` の形（本文は載せない）
- 番犬 9 本（`KNOWN_DISCARDED` は空・新規 1 本が別画面の 1 実装と直呼び増殖を縛る）。注入 7 通りで file:line 名指し FAILED（`right_panel.rs:231` / `preview_render.rs:448` / `:475` / `right_panel.rs:245`）。走査窓が隣の関数へ食い込む穴を `fn_body` で塞いだ（#1399 が踏んだのと同型）

## 2026-09-12（#1420: 番犬の走査範囲が途中の `#[cfg(test)]` で切れるのを直した）
- 範囲取りを「最初の `#[cfg(test)]` で切る」から「**テスト領域だけ空白へ潰す**」1 実装（`common/production_range.rs`）へ。バイト長と行番号が保たれるので `file:line` がずれず、途中のテスト用ヘルパでも本番コードが消えない。下限（既定 30%）を割ったら潰した先頭の領域を名指して落ちる
- 実測 A/B: `remote.rs` の検査対象の**後ろ**へ 4 行のヘルパを注入すると legacy は 5,903 → 3,124 行（70.7% → 37.1%）へ縮んで **7 本とも緑のまま**・**手前**へ注入すると「改名したら番犬も直す」で 6 本 FAILED（誤診）。新は両方 8 本緑・本番 70.7% を維持。縮める注入では `remote.rs:1505 走査範囲が全体の 18.3% まで縮んだ（下限 30.0%）` で FAILED
- 棚卸しで 8 本を寄せた（`platform_parity` は 4 クレートの src を丸ごと走査していて `orchestrator/mod.rs` を 1.5%・`mcp/mod.rs` を 10.5% しか見ていなかった）。番犬 `issue1420_production_range_watchdog` 12 本 + 実ファイル 254 本の不変条件。寄せられない 4 件（製品コード内の番犬）は `KNOWN_COARSE` へ件数つきで宣言。**テストのみ = install 不要**

## 2026-09-12（#1402: ツリーが上限超過ぶんを黙って捨てるのを直した）
- `read_dir_sorted` の戻り値を `DirListing`（entries + 切り詰め + 読み取り失敗）へ広げ、切り詰めと「読めない」を #1398 の器（`RowNote` / `render_note_row`）の行として出した。判断は CLI の `tree git-status` と同じ 1 実装 `tako_core::sidebar::Truncation`（`total > limit` の手書きを両側から排除）
- A/B（`TAKO_1402_LEGACY=1`）: legacy = 560 件で **行 501 / note 0**（Issue の実測そのもの）・権限なしが空と同じ → 新 = 行 502 で `Truncated{shown:500,total:560}`・読めないは Error 行。番犬 3 本（注入 6 通りで file:line 名指し）+ 単体 8 本
- 次: 上限そのもの（500）の見直しは別 Issue（暴走防止として妥当なことは perf 実測済み）

## 2026-09-12（#1425: save_layout の変化検出を JSON 直列化の前へ出した）
- 判定材料を「組んでから比べる」から「組む前の 128bit キー」へ（`layout::change_key` + 借用版 `PaneMetaRef`）。UI 側の付帯情報は `LayoutExtras` に 1 度だけ組んでキーと穴埋めが同じ値を読む。`collapsed` は HashSet 由来なので昇順に整える。載せ忘れは 3 段で止める（網羅的分解でコンパイル / `CHANGE_KEY_FIELDS` × #916 指紋の番犬 / 連続 30 スキップで必ず突き合わせる保険）
- 実測 A/B（23 ペイン・同一手順の隔離 GUI）: 70 秒アイドルで新 = 37 呼び出し中 **35 スキップ・capture 2 回**、legacy（`TAKO_1425_LEGACY=1`）= **capture 63 回**。`written` は両腕とも 26 で同一（保存結果は不変）。確保は 22 ペインで **319 回 / 29893 バイト → 0 回 / 0 バイト**（0.349ms → 0.065ms）
- 番犬 6 本 + 確保計測 1 本（注入 6 通りで file:line 名指し FAILED）。操作 13 種で保存を実測・再起動復元 23 ペイン成功。内訳は `tako persist` の `save_layout` で読める。**install 要**
