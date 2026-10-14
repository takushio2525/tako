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

## 2026-10-15（#1696: encoding_conpty の描画テスト 2 本がコードページの UTF-8 固定を状態で待ってから type を送る）
- `read_pane_code_pages` を足し、描画 2 本は 65001 を状態で待つ形へ。Windows CI で 12 回 × 2 本 × 2 巡が 0 失敗・注入（固定前を作る）で旧形 3/3 化け / 新形 3/3 通過。PR #2079
- 固定はプロンプトの 7〜34 ms 前（20/20）でシェルと同期していない = 製品側の差し止め（最初の書き込みの保留・診断・諦めの記録）は #2092

## 2026-10-15（#2057: docs にコマンドカードのページを新設 = 折り返しの比較・帯・コピー・実行・複数・スマホ・Claude Code の画面の中をスクロール連動で見せる）
- `features/command-card` に 5 場面（20 手順）。部品は `CommandCardFull`（複数・改行・失敗の赤）と `PhoneCommandCards`（全文の確認シート）を新しいファイルで足し、共有部品は不変。実行ペインは実物どおり下へ 35%（`dispatch.rs` の `Direction::Down`）
- 実測: Playwright で 1440 / 375 × ライト / ダーク / 動かさない = 120 枚・横はみ出し 0・手順の不一致 0。① は AI 側を 14.6em（≒ 21 桁）に固定し 1440 / 1024 / 768 / 375 で折り返り・はみ出し 0・良い側 1 行を確認

## 2026-10-15（#2052: IPC の `tako todo list` / 完了・却下の置き場の読み書きを UI スレッドから外し、パレット・`tako panel` で開いた瞬間に一覧を引き直す）
- `prepare_offload` の腕 `user_task_offload_prepare`: ① 送達フローの観測（生きている全ペインの決着したフロー・I/O なし）と展開状態の写し（UI）→ ② 書き戻し + 読み / 書き換え（background）→ ③ 完了・却下だけ開いていた行を畳む（UI）。段は dispatch の同期の腕と共有。右パネルのボタンは `dispatch_off_ui`、パレット / `set_panel` → `after_dispatch` は `spawn_user_tasks_list`。A/B `TAKO_2052_LEGACY=1`・番犬 `issue1984_*` 11 節（注入 10 通り）・カタログ増 0 B
- 実測（読み 1500 ms の注入・`scripts/test-user-task-offload-2052.sh` 9 PASS）: 一覧 ×6 / done の最中の IPC 最大 新 72 / 34 ms・旧 3073 / 2950 ms、測定区間の UI スレッドの読み 新 0・旧 16 回。visual `tasks-offload`: done のボタン 新 1 ms・旧 6025 ms、パレット / `tako panel` は新だけ開いた更新でジョブ（旧は ① で FAILED）

## 2026-10-15（#2063: docs「閉じても消えない」をスクロール連動のモーションで作り直す）
- 4 場面（閉じる / PC 再起動の resume / tako recover / CLI 更新の張り直し）・暗転の部品 Blackout を追加。Windows の対応状況はビルド時に対応表と突き合わせる（値が変わるとビルドが落ちる）。PR #2078

## 2026-10-15（#2084: docs だけの PR と main への push で CI の macOS / Windows の Rust ジョブを skip する）
- 判定ジョブ `changes`（`scripts/ci-change-scope.sh`・テスト 92 項目）+ docs の検査を `docs`（ubuntu）へ移設 + docs / .agent を読む番犬 27 本を `docs-watchdogs`（ubuntu・毎回）で。検証用 PR #2090 で 4 分 43 秒で緑（Rust 2 本は skip）。PR #2089
