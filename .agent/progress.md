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

## 2026-10-15（#2084: docs だけの PR と main への push で CI の macOS / Windows の Rust ジョブを skip する）
- 判定ジョブ `changes`（`scripts/ci-change-scope.sh`・テスト 92 項目）+ docs の検査を `docs`（ubuntu）へ移設 + docs / .agent を読む番犬 27 本を `docs-watchdogs`（ubuntu・毎回）で。検証用 PR #2090 で 4 分 43 秒で緑（Rust 2 本は skip）。PR #2089

## 2026-10-15（#1759: エディタの Enter の続き = Python の抜ける文の後ろを 1 段浅く・Markdown のリストの印を引き継ぐ / 括弧の自動閉じ = 入れた閉じ括弧の飛び越しと対・設定で切れる）
- 規則は `file_type` の列 `dedent_after` / `list_continuation` / `auto_close`。打鍵 1 文字は `EditorCommand::Type` → `TextBuffer::type_char` の 1 実装で GUI の文字入力と `tako edit type` / MCP `tako_preview_edit` の `command: "type"` が同じ口。設定 `editor_auto_close`（既定 ON・serde default）+ `tako edit auto-close` + MCP `auto_close`。カタログ +299 B・A/B `TAKO_1759_LEGACY=1`
- 実測: `scripts/test-editor-enter-1759.sh` 28 PASS（visual `enter-1759` 新 OK / 旧は ① で FAILED・CLI / MCP の同じ手順が GUI の打鍵とバイト一致）・番犬 `issue1759_enter_auto_close_watchdog`（注入 6 通り）

## 2026-10-15（#2082 / #2088: docs の .mdx で表が描かれない・コマンドの -- や "" が化けるのを直した）
- 真因: Astro 6.4 で `markdown.gfm` の既定が未設定 → Starlight が足す @astrojs/mdx 5.0.6 が GFM を外す / mdx の smartypants が JSX の子（<Cmd>）の `--` `"` を置き換える。`astro.config.mjs` で `mdx({ optimize: true, gfm: true, smartypants: false })`
- 表 15 個（5 ページ）・コマンド 3・コード 2 が直り、他 29 ページは出力が 1 バイトも変わらない。番犬 `docs/scripts/verify-markdown.mjs` を postbuild に（注入 3 通りで npm run build が落ちる）

## 2026-10-15（#2077: ドキュメントサイト刷新の仕上げ S3 = 共有部品の提案を反映しページをまたぐ直し）
- 共有部品の提案 20 件を反映（Starter / Phone / CommandCard を 1 本化、TakoWindow に remote・ctx・押した印、data-fx の log / drawer、ポインターの「手順:-」、Scrolly の intro）・初回バナーの列ずれ / WindowsLimits の字下げ / 隠したタブの空きを直した
- 見送り: ページ名「ファイルツリー＆プレビュー」（据え置き）・mdx の表と全角ダッシュ（#2083 で着地済み。more の <Cmd> の回避は外した）・progress（master）

## 2026-10-15（#2086: UserTask の起票・詳細・更新・返答（PWA を含む）・展開も IPC から UI スレッドで置き場を読み書きしない）
- #2052 の腕を門 `user_task_offloads` で全 action へ（A/B `TAKO_2086_LEGACY=1`）。add は workspace 由来の材料だけ UI で写し起票元の解決（プロファイル・セッションカタログ）と書き込みは background、respond は届いた順の札 → 積む（bg）→ 運ぶ（UI）→ 記録（bg。`OffloadLanding::Settled` で後処理の後）、expand の同期の引き直しは `list_poll.pending` へ。via を省いた返答だけ同期（MCP は `mcp` を埋める）。番犬 12 節（注入 10 通り）・カタログ増 0 B
- 実測（読み 1500 ms の注入・`scripts/test-user-task-offload-2086.sh` 16 PASS）: 最中の IPC 最大 新 17〜32 ms・旧 1402〜4421 ms、UI スレッドの読み 新 0・旧 10 回。PWA 実経路 69 PASS・配送 45 PASS

## 2026-10-15（#2021: MCP の stdio ブリッジ 1 本 約 7 MB → 1.09 MB）
- clap を通らない・カタログを使い捨ての子プロセスで組む・.app では libSystem だけの `tako-mcp` へ exec で引き渡す。登録は `tako mcp serve` のまま（移行なし）、応答はバイト単位で同じ。計測は `scripts/measure-mcp-bridge-2021.sh`、番犬は `issue2021_mcp_bridge`

## 2026-10-15（#2041: 残りの `claude agents --json` 走査を mod の報告で埋めた = GUI モードのチャット表示と /clear 後のレジストリの会話 ID）
- チャット表示は報告のあるペインを問わず、残りだけ絞って走査。`/clear` 後はレジストリの会話 ID を報告へ追従（`agents_scan.registry_followed`）。A/B `TAKO_2041_LEGACY=1`・番犬 `issue2041_*`。PR #2091
- 実測 `scripts/test-session-scan-mod-2041.sh`: chat 60 秒 8 → 0 本・/clear 後 25 秒 器あり 2 → 0 本 / 器なし 12 → 0 本

## 2026-10-15（#1950: `.ino` を再生ボタン / `tako run` で走らせる arduino 種別）
- `arduino-cli compile` / プロファイル `upload`。arduino-cli は PATH → Arduino IDE 同梱（`app_bundle`）で探し、基板未選択・名前の不一致・arduino-cli なしを実行ペインと `project.notes` で案内。PR #2093
- 実測: `scripts/test-runner-arduino-1950.sh` 25 PASS（IDE 同梱 1.2.0 で compile exit 0・A/B `TAKO_1950_LEGACY=1`）。upload は基板なしで未検証

## 2026-10-15（#2085: 器が自ら消えても最後の行までペインログへ = remain-on-exit + pane-died フックで消える前に全履歴と画面を書き出す）
- `<data_dir>/pane-died/` へ書き出し、ProcessExit の閉じ方で読む（子プロセス 0 本）。psmux は未実測で立てず `tako_logs` を Windows degraded に申告（PR #2095）
- 実測: `test-pane-log-pace-2018.sh both` ③ 新 0〜1499 欠け 0 / `TAKO_2085_LEGACY=1` 末尾 626 行欠け、⑤ 閉じるまで 152 ms（旧 151 ms）、tmux 起動本数・常駐は新旧同じ

## 2026-10-15（#1956: 言語サーバの検出表の第 2 波 = .tex texlab / .toml tombi（tako が取る）・.swift sourcekit-lsp・.dart dart・.css/.scss/.less と .html/.htm vscode-langservers-extracted）
- 候補 18 種の比較と採った順は language-coverage §8。workspace.configuration を申告しない条件で測り直し、YAML（ホバーが出ない）を見送って CSS / HTML を採った。taplo はハッシュ無し + ホバー不可で tombi（PR #2102）
- 実測: `scripts/test-lsp-servers-1956.sh` 37 OK（6 行とも開くだけで波線とホバー・visual-test `lsp-servers`・境界 2 つ）。e2e `issue1956_lsp_servers_wave2` 5 本

## 2026-10-15（#2050: 鍵らしき文字の確認を mod の送信時・スマホの入力・tako send へ広げ、許可リスト・規則ごとの無効化・警告だけの段を足した）
- mod は `prompt.submit` → `paste-guard check --pane`・選択ダイアログ、スマホは 409 → 確認シート、`tako send` / `tako_send_input` は止めずに警告。許可リストは指紋だけ。tako が流した値はペインごとに 10 分確認済み（PR #2105）
- 実経路 `scripts/test-paste-guard-2050.sh` 58 PASS（実物の claude・実 daemon・A/B）+ visual ⑫・カタログ +680 B

## 2026-10-15（#2098: tmux_backend の conf 差し替えテストを 700 ms の実時間の窓から読み取り・書き換え各 200 回の回数区切りへ・issue1857 の子の終わりの待ちを 10 → 60 秒）
- 届かないときはクライアントの状態を出す。注入: 読み取り 30 ms 遅延で旧 3/3「検出力が無い」・新 3/3 ok、非アトミック化で新 5/5「不完全な conf」、kill 12 秒遅延で旧 FAILED / 新 ok。Windows CI 新 38/38 ok（PR #2100）

## 2026-10-15（#1686: LSP の名前の変更 = 既定 dry-run・段階 1 は頼んだペインのバッファだけ・右クリックの欄）
- `tako_core::lsp::rename`（WorkspaceEdit の changes / documentChanges）+ prepareRename → rename + CLI `tako lsp rename [--yes]` / MCP `tako_lsp` の action=rename（+428 B）+ 右クリックの「名前の変更…」。A/B `TAKO_1686_LEGACY=1`（PR #2099）
- 実測: `scripts/test-lsp-rename-1686.sh`（偽サーバ + 実の rust-analyzer）で dry-run 後の sha256 一致・--yes で 2 箇所・undo 1 回で差分 0・別ファイルは not-applied・`1abc` は refused、visual-test 新 OK / 旧 ① FAILED

## 2026-10-15（#2092: Windows のペインは文字コード固定の決着まで入力を預かる = `encoding_pin::PinGate`・上限 直接 500 ms / 器 1500 ms）
- `check-health` の `encoding_pin` 節と persist.log へ諦めを記録。器の内側の固定が本番経路で呼ばれていなかったのを `spawn_in` で配線（PR #2103）
- Windows CI 実測（各 25 回）: 直接は固定 5〜16 ms / プロンプト 26〜46 ms、器は内側 154〜180 ms / プロンプト 226〜310 ms。注入の A/B で旧は化け・新は `日本語テスト`（直接・器とも 5 巡）

## 2026-10-15（#2104: 対応状況の 2 ページ（windows-support / agent-support）の md をコミットせず、件数を持たない写し `docs/src/data/*.json` から docs のビルドが組む）
- 写しは 1 行 = 1 フィールドで並列の PR が別々の行を直しても衝突しない（`docs/scripts/support-pages.mjs`・astro の integration）。番犬 `scripts/test-support-snapshot-merge-2104.sh`（CI の docs ジョブ）。PR #2108
- 実演: 実物の support.rs を直した 2 本は修正前が md で衝突・修正後は衝突なしで `--check` 緑。dist は前後で `diff -r` の差分 0

## 2026-10-15（#1964: tako mod でチャット風の描画 = 吹き出し・コードの Copy（クリック）と帯の `o: copy last code`（キー）・ツールの 1 行の要約）
- ターミナル表示の既定は Copy だけ。GUI モードは `chat.place mod` でそのペインを本物の画面のまま見せる（既定 = GPUI のチャットはユーザーの判断待ち = tako todo）。GPUI のチャットは報告の `chat_seq` で読み直す（#1883 の内容）。PR #2111
- A/B `TAKO_1964_LEGACY=1`・番犬 `issue1964_*`（注入 15 通り）・`scripts/test-mod-chat-1964.sh` 全段緑・カタログ増 0 B

## 2026-10-15（#1951: `.ino` の言語サーバ = arduino-language-server の行・伴走（arduino-cli / clangd）と空の設定ファイルを表の `Arg` で・基板が未選択なら unconfigured で起こさず選ぶコマンドを案内）
- 実測 `scripts/test-lsp-arduino-1951.sh` 23 OK（IDE 同梱物で診断・ホバー・補完・帯の画像・選べば合図で起きる・ALS なし・A/B）。常駐 ALS + clangd 72〜77 MB。PR #2109
- `.h` / `.cpp` を ALS へ回すと先に `.ino` を開いた時だけ正しい = #2113 に分けた

## 2026-10-15（#2106: CI の macOS で実 tmux のテストが 1 本も走っていなかった = ランナーの image に tmux が無く全部 skip のまま ok）
- tmux の導入段（11 秒）+ `tako_core::tmux_test_gate`（`TAKO_TEST_REQUIRE_TMUX=1` で skip を panic・台帳から件数を出す）+ 番犬 `issue2106_*`。CI で 52 本が実 tmux で走る。PR #2110
- 罠: 件数の `sort -u` が en_US.UTF-8 の照合で日本語のテスト名を潰した（`LC_ALL=C`）。issue1857 の負荷依存のフレークは #2114
