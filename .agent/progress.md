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

## 2026-09-08（#1180: セルフテストの器の往復待ちを予算へ移し、期限つき状態の混在を根治した）
- Issue の grep（`0..25`）は失敗項目②（`0..20`）を漏らしていたので洗い直し、**18 か所**を `wait_for_backend_state`（状態待ち + `state_wait_budget`・駆動は毎周期）へ。番犬は region（tmux ゲート内の固定窓）+ `read_to_string` needle + #1162 へ `tako_control::dispatch(`
- **真因は待ちの長さだけではない**: 項目 73 は「待てば立つミラー」と「1.4 秒で消えるスクロールバー」を 1 回のホイールのあと同時に見ていた（実測 `bar=false`）ので予算では解けない → ホイールを毎周期打つ形へ
- 無負荷 + load 13.7〜16.1 で `TAKO_APP_SELF_TEST_OK` / A/B は `LEGACY=68-attach|73-wheel` + `INJECT=late` が FAILED（`waited=10.1s budget=10.0s` / `6.2s/6.0s`）・新経路は通過 / `INJECT=never` は新も FAILED

## 2026-09-08（#1081: 解説動画の読み間違いを全区間点検して v4 を合成）
- 全 47 区間の kana を機械で出して全件読み、誤読 5 件を修正（`3 つ`→みっつ / `1 行`×2 / `空のペイン` / `140 個`）
- **ユーザー辞書は空白でトークンが割れて当たらない**（実測）ので、合成直前の置換表 `reading-overrides.tsv` を正本に
- v4 = 9:59 / -14.9 LUFS / TP -1.5 dBTP / PII 0 件。点検は `check-readings.sh --diff`

## 2026-09-09（#1013: codex / agy の spawn に claude 語彙のモデル既定を渡さないようにした）
- 真因はアカウント（#504）の `default_model` を spawn の明示指定と同じ段へ畳んでいたこと。`AccountDefaults` で段を分け、継承の可否は新 1 マス `worker_model_default_inherit`（claude のみ対応）へ問う形に（#982）。応答に `model_source` / `effort_source` を追加
- 実 spawn（隔離 + tako-vd）: codex は自分の既定 `gpt-5.6-sol medium` で起動して `OK1013` を返した / agy も `--model` `--effort` なし / claude は `--model claude-opus-5 --effort max` のまま不変
- A/B `TAKO_1013_LEGACY=1` は Issue と同じ `codex --model claude-opus-5 …` を再現。番犬 3 本 + 単体 7 本。全 3613 件緑

## 2026-09-09（#1019: setup ディレクトリを data dir の境界へ寄せ、旧 Windows パスから自動移設）
- `setup_dir()` の macOS 直書きを `tako_control::setup::setup_dir`（= `data_dir()/setup`）へ集約。旧パスは `SchemaId::Setup` の番地 + 専用実装で移設（写す → 旧ごと `setup.pre-v1.bak` へ rename・**隔離中は移設しない**）
- A/B 実測: 同条件の `setup --yes` が旧バイナリは HOME 側へ 16 ファイル・新は `$TAKO_DATA_DIR/setup` へ。本番 dir は隔離 6 経路の前後でハッシュ・mtime とも不変
- 番犬 `setup_dir_boundary_watchdog` 4 本（修正前コードで 2 本が確定 FAILED）+ 単体 12 本。全 3629 件緑（main 取り込み後）。Windows 実機での移設は #467 配下で要確認

## 2026-09-09（#1187: tmux cleanup が黙って何もしない形をやめ、`--socket` を実装した）
- 見送りの判定を「別の tako-app がいるか」から**対象ソケットの所有者**へ（相手の初期環境を `KERN_PROCARGS2` で読み `tako-iso-<pid>` 等を復元）。応答を `{socket, killed, skipped, detail}` へ広げ、見送り・kill の両方を persist.log へ 1 行
- 実測（隔離 + tako-vd・他 tako-app 3 本稼働）: 省略時 `killed=[aaa,bbb]` / `--socket <別>` で `ccc` を kill / `--socket tako` は `skipped=peer_shares_socket`（pid 71082）で本番 17 セッションは不変
- A/B `TAKO_1187_LEGACY=1` は Issue と同じ `{"killed":[]}` + `--socket` 無視を再現。番犬 4 本（修正前ソースで全滅）+ 単体 12 本。全 3621 件緑

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
