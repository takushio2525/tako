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

## 2026-10-13（#2020: sessions.yaml を走査ごとに全体書き直ししていたのを、last_seen_at だけの変化は溜めて検出集合の変化・他の項目の変化・600 秒・GUI の終了で書く形へ）
- `sessions::LastSeenThrottle`: 書く時は溜めた「最後に見えた時刻」ごと書くので、その時点の中身は修正前と同一（並び順・GC・#466 の逆引きは書かない間も不変）。`.bak` は last_seen_at 以外が変わったときだけ・同じ内容は書かない・GUI は会話 0 本の走査と `on_app_quit` でも書く。形式は不変・A/B `TAKO_2020_LEGACY=1`
- 実測: 本番（読むだけ）54 分で書き直し 121 回のうち、集合か last_seen_at 以外が変わったのは 10 回。`scripts/test-sessions-write-2020.sh`（隔離 GUI 2 本・偽 claude 24 本・500 件）の 30 分で新 7 回（`.bak` 3）/ 旧 58 回（`.bak` 58）・3.9 / 45.0 MB。注入 5 通りで新しい単体が file:line で落ちる

## 2026-10-13（#2046: 貼り付けに API キー・パスワードらしきものがあれば PTY へ流す前に「送る / やめる」を確かめる。既定 ON・旧い settings.json も ON）
- 判定は `tako_core::paste_guard::scan` の 1 実装（既知の形の表 14 行 + 秘密鍵・JWT・URL のパスワード → 代入の値 → 3 字種・短い語の割合・エントロピーの塊。除外表 13 行・先頭 64 KB・100 件で打ち切り）。関所は `paste_to_terminal` の 1 か所・Enter / Esc はやめる側・送るはボタンだけ。CLI `tako paste-guard` / MCP `tako_settings` action=paste_guard / 設定画面のトグル。A/B `TAKO_2046_LEGACY=1`・番犬 `issue2046_paste_guard_watchdog`
- 実測: `scripts/test-paste-guard-2046.sh` 20 PASS（visual-test `paste-guard` = やめると目印より前に 1 バイトも無い・送ると届く・off で素通し・ブラケットペーストも包んで届く・persist.log に本文なし。旧挙動は ② で FAILED）。実データ 9378 片で当たり 7（すべてテストの偽の鍵等）・ランダム 32 文字 97.6%・最適化ビルドで最悪寄り 0.5 ms

## 2026-10-13（#2031: 右パネル tasks の 2 秒ごとの一覧更新を UI スレッドから外し、置き場の版が同じなら読まない）
- dispatch の "list" と共有の 2 段: 配送の観測 `user_task_list_observe`（UI・I/O なし）→ 書き戻しと読み・解釈 `user_task_list_load` + `parse_list`（background。版 = 更新時刻・大きさ・inode を読む前に採り、同じなら前回の正本から組み直す・添付の実在は毎回）。2 秒ループ・タブ・「完了も表示」は `spawn_user_tasks_list`。A/B `TAKO_2031_LEGACY=1`・注入 `TAKO_2031_INJECT=slow:<ms>`・番犬 `issue1984_*` 9 節（注入 7 通り）
- 実測（読み 1500 ms の注入）: visual-test `tasks-tick` の打鍵の間隔の最大 3049 → 56 ms・`scripts/test-tasks-tick-2031.sh` 5 PASS（IPC 最大 2849 → 26 ms・測定区間の UI スレッドの読み 8 → 0 回）

## 2026-10-13（#1993: PC リソース管理 S1 = 機とプロセスの計測を背景で採り、`tako queue` / MCP `tako_queue` で読む）
- 境界 B29 `platform::resources`（sysctl / mach / libproc。mach tick は timebase で換算・他ユーザーの rusage は呼ばない）+ 純関数 `tako_core::resources`（Activity Monitor の式・分類・帰属・重い処理 = 重い祖先を持たない木）+ `tako_control::resources`（背景スレッド・`ipc::submit` が UI を通さず答える）。GUI 不在の CLI は機の値だけ。番犬 `issue1993_*`（+ #1968 / #1984）・カタログ +574 B・Windows は Pending（#2000）
- 実測: `scripts/test-resources-1993.sh` 10 項目緑（vm_stat の式と一致・器の子孫 / nohup の子の帰属）。Apple のプラットフォームバイナリは env を読めない（0 キー）。費用は走査 5 秒で +0.57% → 見ていない間 30 秒へ落として差がばらつきの中（0.85% 対 0.87%）

## 2026-10-13（#2011 S0: 描画と RAM の徹底的な軽量化の計測と計画 = 設計書 `.agent/plans/2026-10-lightweight.md` と計測スクリプト・子 Issue 7 本）
- 本番 835 MB はプレビュー（キャッシュ 173 MB + GPU のアトラス 約 150 MB）と描画（drawable 3 面 + 窓に依らない固定 約 150 MB）で過半。器のグリッドは 26 枚で 1.6 MB（履歴は tmux 側）。CPU の最大は監視の `claude agents --json` 60 本/分 = 子プロセス 24%（本体 18.7%）
- `scripts/measure-footprint-2011.sh`（同条件の footprint・アイドル CPU・描画回数・起動時間・電力）。#2009 と重なる分は実測を渡し、重ならない #2017〜#2023 を起票（S2 監視スキャン・S3 ペインログ・S4 malloc・S5 sessions.yaml・S6 MCP ブリッジ・S7 display link 提案・S8 番犬）

## 2026-10-13（#1691: エディタで対応する括弧を強調 = 文字列・コメントを飛ばす探索・VS Code の採り方・応答の `document.bracket_match`）
- `text_edit::brackets`（記号は `file_type` の `comment` 列 + 新しい `lexical` 列）・直後 → 直前・選択中と括弧に接しないときは採らない・種類ごとに数える。GUI は対の 2 文字に背景、CLI / MCP は編集系の応答（ツール増なし・カタログ +81 B）。A/B `TAKO_1691_LEGACY=1`
- 実測: release 5,000 行で 1 打鍵中央値 0.156 ms・最大 0.268 ms。単体 22 本 + dispatch / MCP・注入 3 通り・visual `bracket-match` 新 OK / 旧 FAILED・実 CLI / MCP stdio の e2e

## 2026-10-14（#2053 S1: docs サイトのスクロール連動モーションの土台・トップとインストールの作り直し）
- `docs/src/components/motion/`（Scrolly / Autoplay と tako の画面の再現部品 24 個）+ `src/scripts/motion.ts`（IntersectionObserver + rAF・gzip 1.9KB・依存なし）。reduce では手順ごとの静止図。トップ（splash・場面 5 + ヒーロー）とインストール（macOS 5 手順 / Windows 7 手順・既知の制限は対応表と突き合わせてビルドで拘束）。全ページのヘッダーに「ダウンロード」
- 手引きと S2 の構成案は `.agent/plans/2026-10-docs-motion.md`。Windows の数字は windows-support.md をビルド時に読む。Lighthouse（gzip 配信）トップ 60→57・インストール 57→57、実測 FCP は前後とも約 2.3 秒

## 2026-10-14（#2032: issue1944_lsp_fetch の取得中の検査のフレーク = 状態と進捗は別々に変わるので、揃った 1 回の読みを状態待ちで捕まえる）
- 真因: 製品は状態（取得中）と進捗（fetch のレジストリ）を別々に変えるので、進捗を置く前と、取り終えて進捗を消してから稼働へ移るまで（#1977 の片付けが走る）は「取得中なのに進捗が無い」。旧テストは各時点で両方を求めていた。配布元に最後の 4 KiB を止める `Serve::Held` を足し、status と画面の口をそれぞれ 1 回の読みで確かめる。注入 `TAKO_2032_INJECT_OLD_FILES=<n>`
- 実測: 旧 + 前の版の段 500 ファイル 5/5 FAILED・旧 + 進捗を置く前に 200 ms 3/3 FAILED（どちらも CI と同じ assert）・新は同じ注入で 5/5 / 3/3 ok・画面に進捗を出さない壊し方で新が名指しで落ちる・新 20/20 ok

## 2026-10-15（#2053 S2: ドキュメントサイトの機能ページをスクロール連動のモーションで作り直した = クイックスタート #2056 / かんたん表示 #2060 / 人がやること #2058 / エディタ #2062 / リモート #2061 / オーケストレーション #2059）
- 各ページの場面は `docs/src/components/scenes/`、再現部品は新しいファイルで足し共有部品は無変更（PR #2068 / #2066 / #2074 / #2076 / #2069 / #2067）。Windows で pending の機能は `WindowsLimits` で対応表と突き合わせ（対応へ上がるとビルドが落ちる）。各ページ 1440 / 375 × 明暗 × reduced-motion で横はみ出し 0
- 罠: Astro 6.4 + @astrojs/mdx 5.0.6 で `.mdx` の GFM の表が描かれない（#2082 で直す）。ページをまたぐ直しと共有部品の提案は #2077（S3）に集約。S2 の PR は progress を触らず、master がこの 1 件にまとめた

## 2026-10-15（#1994: 重い処理の順番待ち `tako queue <コマンド>` = 調停役・割り当ての条件・再起動をまたぐ・この repo の入口で採用）
- 判断は純関数 `tako_core::queue::admit`、調停役 `tako_control::queue` は本番 GUI の 1 つだけで `ipc::submit` の手前で答える。`settings.json` の `resources` 節（serde default）・`tako queue limits / cancel`・MCP `tako_queue` の action（+395 B）。`run_heavy` を build-app / isolated-gui / 検証スクリプトが通す（PR #2071）
- 実測: e2e `scripts/test-queue-1992.sh` 46 PASS。見込みは build 3.5 GB / test 7 GB。swap 10 GB の機では既定の警戒線で 1 件ずつになる（`tako queue limits --swap-guard-gb`）

## 2026-10-15（#2018: 器のペインログの取り込みを出力の合図で間引き、probe と capture を 1 回の tmux へ）
- 出力が続く器だけ 10 秒ごと・止まったら次の tick・30 秒の保険は相乗り、probe + capture を 1 回へ、閉じる直前に増分と画面を 1 回で読む。A/B `TAKO_2018_LEGACY=1`（PR #2080）
- 実測: 出力中 60 秒の tmux 68 → 10 本、子の CPU 約 1/6。欠け・重複 0（修正前は欠け 72 行）。出力中に器が自ら消えると最後の最大 10 秒が抜ける = #2085 で直す

## 2026-10-15（#1953: 既存の構文への別名で色の穴を埋めた = `.h` を C++・`.m` を本文で MATLAB / Objective-C・`.pde` を Java・`.jsonc` を JSON・`.json5` を JavaScript）
- `file_type` の行 + 純粋関数 `syntax_by_content`（先頭 200 行）。`.json5` は JSON だと JSON5 だけの書き方が `invalid` でダークで字が消える（144 字句中 113）ので JavaScript、`.S` / `.s` は ARM の `@` が消えるので寄せない。A/B `TAKO_1953_LEGACY=1`・構文 213 のまま（PR #2072）
- 実測: `scripts/test-syntax-alias-1953.sh` 38 PASS（CLI / MCP の `syntax` 新旧・visual `syntax-alias`）
