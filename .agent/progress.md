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

## 2026-09-09（#1133: Windows 実機の項目 80 のスタックオーバーフローを予約量の宣言で根治した）
- 真因の commit は無い（7 commit の伸びは合計 +12,288 B）。`-O0` の GPUI フレームが 1 関数 135〜828 KiB（製品の描画経路も含む）で、Windows/MSVC の既定 1 MiB を食い潰していた
- `build.rs` 2 本が `/stack:8388608` を宣言（正は `platform::stack`。Zed も同型）+ セルフテストが起動直後に実測して足りなければ項目 80 前に FAILED。番犬は macOS でも走る 3 本
- 実機は素の debug ビルドで項目 80 を 2/2 通過（完走は別件の負荷依存で項目 105 / 143 まで）。A/B は同一 exe へ `editbin /STACK:` で 1 MiB=クラッシュ / 2 MiB=通過 / 8 MiB=通過

## 2026-09-09（#1185: 取り込みペインの select-window が内側へ届くようにし、`open --window` を到達可能にした）
- 対象解決を `tako_core::tmux::window_target`（取り込みビュー優先 → バックエンド）へ集約。旧実装は二重ネストの**外側**（tako の backend）を見ていたので別セッションの window を切り替えて成功を返していた
- `--window` は CLI と MCP catalog の両方へ（protocol にあるのに 3 経路すべてから到達不能）。番犬 `mcp_param_reachability`（mapper が読む引数が catalog に在るか）が同型の再発を落とす
- 隔離 GUI 実測: ラッパーだけ `window_active` が 1 → 2 へ動き元は 0 のまま / `open --window 2` と MCP の `window:1` が実際にその window を表示 / A/B `TAKO_1185_LEGACY=1` は外側に当たって FAILED

## 2026-09-09（#1015: codex の背景ターミナル待ちを idle + 未達と誤検知しないようにした）
- 真因は**ペイン幅**（負荷でも rollout でもない）: 44 桁では codex が `• Waiting for background terminal (1m 04s •…` と自分で切るので `esc to interrupt` が消え、末尾の `›` を拾って idle になっていた。同じ理由で `prompt_undelivered`（自動再送）の抑制も外れる = 2 症状は同一原因
- 判定を「`•` で始まり `(` の直後が経過時間 + その直後が区切り」へ。**語では判定しない**（`Waited for …` は完了後も残る履歴行 = 永久 busy になる。実測 5 回残存）。未達の断定は rollout を実際に読めたときだけにし、読めないときは `prompt_delivery_unverified` へ降格
- 実採取 2 幅 + 実 worker（器つき隔離）で `busy` / `codex-session` / events 空。A/B `TAKO_1015_LEGACY=1` は Issue と同じ `status=idle` `prompt_delivery=undelivered` `resend_prompt` を再現。番犬 3 本 + 単体 9 本。全 3659 件緑（main 取り込み後）

## 2026-09-09（#1081: かんたん表示章を手順型デモへ差し替え）
- 旧 c5_gui（1 区間）を捨て、実クリック / 実キー入力で撮る `scene_guimode` と 11 区間の台本（`c5_gui1`〜11）を作った
- 実収録 5 回で罠を根治: 窓の重なりでクリックが吸われる（#1149 で seed が死亡）/ 押下の前面化でユーザーのキーが流入 / master が run を選ぶ
- 素材の検査は PII 0 件・15 秒以上の静止 0 件。worker の絵だけ撮り直しが残り（画面ロック待ち）

## 2026-09-09（#1188: tmux open で取り込んだセッションが永久に掃除されない問題を根治）
- tmux のセッショングループは**メンバーが 1 つになっても残る**（実測 3.6b: ビュー kill 後も `grouped=1` / `size=1`）ので、判定材料を `#{session_group_size}` > 1 へ。行のパースと orphan 判定を `tmux_cleanup`（`LIST_FORMAT` / `is_orphan`）へ出し cleanup と find が 1 実装を見る形に
- 実測（隔離 + tako-vd）: open 前 `grouped=0 size=` → open 中 `size=2` で cleanup は `killed=[]`（保護）→ close 後 `grouped=1 size=1` で `killed=[tako-blind-4]`
- A/B `TAKO_1188_LEGACY=1` は Issue と同じ `killed=[]` + 残存を再現。ガードを丸ごと外す注入は表示中ビューの保護が落ちて FAILED（= 消して直したのではない）。番犬 1 本 + 単体 6 本

## 2026-09-09（#893: ホーム解決と `~` 短縮の入口をワークスペース全体で 1 本に寄せた）
- `HOME` 決め打ち 15 箇所を `paths::home_dir()` へ、`~` 短縮 10 箇所を新設 `paths::shorten_home` へ。`ssh_config` は Windows で ssh 系が効くようになり `issue652_resume_e2e` の `.expect("HOME")` panic も消えた
- 番犬を 2 ファイル走査からワークスペース全体へ（`home_dir_watchdog.rs`）。テストの HOME 差し替えは Drop 復元の `HomeGuard` へ寄せた
- A/B は修正前コードでホーム解決 30 行 + `~` 短縮 10 行を名指しして FAILED。全 3674 件緑・`HOME`/`USERPROFILE` 両方なしでも panic せずエラー文で止まる（実測）

## 2026-09-09（#1190: kill / resize の既定ソケットを list と揃え、tmux の生エラーを包んだ）
- `socket` 省略時の解決を `tako_core::tmux::resolve_session_socket`（既定サーバー → tako バックエンド = list の並び）へ集約し、`kill` / `resize` / `open` の 3 つで共有。応答に解決後の `socket` を追加
- 生 stderr は `friendly_error` で日本語へ（4 分類 + `scrub_paths` で絶対パスを伏せる）。**kill 前の確認 / `--force` は #1196 の担当で入れていない**（省略でも backend へ届くようになった）
- 隔離 GUI 実測: `--socket` 省略の resize が 80x24 → 60x15・kill --window 1 が届いてセッションは生存 / A/B `TAKO_1190_LEGACY=1` は Issue と同じ `error connecting to /private/tmp/tmux-<uid>/default` を再現

## 2026-09-09（#925: 導入計画の権限説明を platform で出し分けた）
- `InstallPlan` に `platform: Platform` を足し（出どころは `InstallRecipe::platform`）、権限行を純粋関数 `privilege_line(platform)` へ。unix = `sudo（管理者権限）は使いません…` / Windows = `管理者権限は使いません…`
- `visible_texts()`（計画の表示 + 引き継ぎ指示文）を用意し「Windows に unix 固有語が出ない」を GUI 無しで固定。`to_json` に `platform` を追加
- A/B `TAKO_925_LEGACY=1` は Issue と同じ `sudo（管理者権限）…` を Windows 構成で出して新テスト 2 本が FAILED。#920 の項目 119（`管理者権限` で見る）は不変

## 2026-09-09（#1186: resize --reset が実際に window サイズを戻すようにした）
- `reset_window_size` を `resize-window -A` → `set-window-option -u window-size` の 2 段へ。**順序は逆にできない**（`-A` が window-size を manual にする = 実測）。`-A` 失敗時も解除は行い**エラーはそのまま返す**
- 真因は「オプション解除はその場でリサイズしない」こと。クライアントがその window を見ているあいだは偶然戻るので、**別 window / クライアント不在**のときだけ症状が出る（実測で切り分け）
- 隔離 GUI 実測: 見ていない window 1 が 119x21 → 60x15 → reset で 119x21・option 空 / A/B `TAKO_1186_LEGACY=1` は `{"reset":true}` を返しつつ 60x15 のまま

## 2026-09-09（#1192: 隔離・テストの tmux サーバーを所有者の生死で安全に回収できるようにした）
- `tako tmux cleanup --servers`（既定 dry-run・実削除は `--apply`）を追加。判定は名前ではなく**所有プロセスの生死**（自分 / 既定 / 生きた tako-app の env 復元 / attach 中 / 所有者不明 / 出来たて を除いた残りだけ回収可）。溜めない側は使い捨て backend（`tako-iso-<自分の pid>`）の終了時 self-kill
- 実測: 本番置き場へ dry-run = 総数 1838 / 生存 126 / 回収可 116 / 残骸 1708 / 保護 14（何も消していない）。隔離した置き場のダミーで `--apply` = 生きた所有者は `owner_alive` で無傷・死んだ所有者だけ kill + ソケット削除
- 番犬 2 本（修正前ソースで FAILED）+ 実物テスト 3 本 + 純粋 4 本。所有者の生死ガードを外す注入で必須テストが FAILED

## 2026-09-09（#1182: ターミナルのパスリンクに cmd+右クリックメニューを付けた）
- 並びは `tako_core::path_menu`（純粋関数）・文言はツリー（#314）の `sidebar::menu_*` を委譲で共有。新規操作は「tako で開く」= `FileOpKind::OpenInTako` の 1 つだけで **cmd+クリック自身もそこを通す**（CLI `tako file open-in-tako` / MCP `op=open_in_tako`）
- **前提として `links::combined_screen_text` の soft wrap 判定を直した**: 実画面の行は空白詰めなので旧判定では全行が折り返し扱いになり、隣接 2 行に何か書かれているだけでパスリンクが 1 つも検出されなかった（A/B: 旧式で新テストが FAILED）
- 項目 147（合成マウスの実配送 + 実矩形 + 前提の ui-mode 倒し・unix 限定）/ CLI・MCP e2e を隔離 GUI で実測 / 実フレーム PNG 2 枚 / `TAKO_1182_LEGACY=1` で 147 が確定 FAILED。全 3711 件緑

## 2026-09-09（#1191: `tako list` の `backend_windows` を右パネルの表示状態から切り離した）
- 採取が fleet ビューの 2 秒ポーリングだけだったのを `Request::List` の直前 1 回（`list-windows -a` = 6.9ms 中央値。`fetch_tmux_sessions` は 37ms）へ。backend ペインが無ければ tmux を起動せず、500ms 以内は使い回すので連打でも 2 回/秒（実測 889 req/30s → 56 回）
- `null`（backend でない / 採取不能）と `[]`（backend だが window 無し）を読み分け可能に。1 window の器も載る（旧実装は 2+ のみ）。待機時の追加コストは 0 回 / CPU 差は誤差
- A/B `TAKO_1191_LEGACY=1` は Issue の 2 症状（常に null / 開くと埋まり閉じると陳腐化）を再現。番犬 3 本 + dispatch 1 本 + tako-core 2 本 + セルフテスト項目 61g

## 2026-09-09（#1220: 一覧付与のコスト検査を実時間から量へ替え、番犬を tests 全体へ広げた）
- `warm <= cold`（実時間比較）を廃し、`claude_remote_link::scan_counters`（走査回数 / 読み出しバイト数 / 所在探索回数・**スレッドローカル**）で測る形へ。生きている会話への追記ぶんは予算へ足す
- 番犬 `test_timing_watchdog`（`crates/*/tests` の assert 条件部に実時間の値が 2 つ以上ある形だけを落とす。コメント / 文字列は潰して見る）を新設。修正前ファイルで `remote_link_live.rs:247: warm <= cold` を名指しして FAILED
- A/B（`yes` 36 / 72 本の負荷・交互 40 回）: 旧 2/40 FAILED（初回 34.3ms / 2 回目 45.7ms で反転）・新 0/40。memo 無効化の注入で新は確定 FAILED（20 走査 10 MB）・旧は 9/10 見逃し
