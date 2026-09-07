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

## 2026-09-03（#1093: 組織クレジット上限の見出しを検知できず自動復帰が発火しない問題を根治）
- 症状は「解除後 1 時間以上 worker 3 体が止まったまま `supervisor.log` に記録ゼロ」。原因は
  **停止判定の文言が `hit your usage limit` 決め打ち**で、実際の見出し
  `You've hit your session limit · resets 7:50pm (Asia/Tokyo)` に 1 文字も当たらないこと。
  **Issue の推定と違い解除時刻パースは無罪**（既存の `resets ` アンカーで 19:50 を正しく読む =
  診断テストで実測）。壊れていたのは検知だけ
- 直し方は 1 文言の追加ではなく**テンプレートを規則にした**: claude 2.1.258 のバイナリから
  見出しの組み立て（`` `You've hit your ${限度の名前}${理由}` ``）と限度名の表（`nF` =
  `session limit` / `weekly limit` / `Opus limit` / `Sonnet limit` / `Fable limit` /
  `usage credit limit` + 各種 spend limit + `usage limit` / `limit`）を採り、
  **すべて `limit` で終わる**ことを使って `hit your` の直後（句読点の手前）に `limit` が
  在るかで判定する。版で名前が増えても追従する。正本は
  `tako_core::limit_resume::is_limit_exhausted_line` の 1 箇所で、`detect_worker_error` と
  ステータスバーの両方がここを通る（判定が散ると「復帰は動いたのにメーターは `--`」が再発する）
- **ステータスバーの `--` も同時に解消**: 上限中はフッターが `5h NN%` を出さないので、
  見出しから枠を読んで（`session limit` → 5h / `weekly` ・ `Opus` ・ `Sonnet` → 7d）
  100% を埋める。**フッターに数値があれば実データが勝ち**、枠へ対応づけられない上限
  （`usage credit limit` / spend limit）は `--` のまま（メーターに嘘を書かない）。
  走査窓を別に持つ理由は実測の幾何（フッター 6 行 + 入力欄 3 行 + 空行 = 見出しは 10〜15 行上。
  本体の 8 行窓では届かない）
- 検証: fmt / clippy（両 feature）/ `cargo test --workspace` 全緑 / Windows クロスチェック
  エラー 0・警告 12（**全件が未変更ファイル由来** = ベースライン同数）/ 隔離セルフテスト
  `TAKO_APP_SELF_TEST_OK`（項目 111 に正例 ④ を新設 = 検知・解除時刻・**メーター 100%**・
  解除前は撃たない・解除後の復帰・**`supervisor.log` への記録**を実測: `audit=0->1`)。
  **検出力は `TAKO_1093_LEGACY=1` の A/B**（unit 7 本 + 項目 111 が FAILED / 旧文言の
  回帰テスト 4 本は両アームで緑）
- 範囲外として申し送り: `You're out of usage credits · resets …` / `Your org is out of usage`
  は**別テンプレート**なので今回は受けていない（同じ穴が残る）。`paint_and_hold` の POSIX 経路は
  本文を素の単引用符で囲むので fixture に `'` を入れられない（実バイトの fixture は unit 側が持つ）

## 2026-09-07（#1143: 狭いペインの `/model` セレクタを選択肢ダイアログとして読めるようにした）
- 実採取で原因確定: カーソル `❯` は「描かれない」のではなく**ダイアログがペインより高いと画面外へ出る**（25×70 では出る）。
  経路 3（カーソルなしの番号つき連なりを anchor）+ `label_truncated` 申告 + 2 列レイアウトの説明列をラベルへ混ぜない、の 3 点で直した
- fmt / clippy / `cargo test --workspace` 全緑（3521 件）+ 隔離 GUI（tako-vd）の実 claude 25 桁 × 44 行ペインで
  CLI / MCP / watch の 3 経路を実測 / A/B `TAKO_1143_LEGACY=1` で新規 14 本中 10 本が FAILED。仕様は FR-2.25.11

## 2026-09-07（#1136: 夜間リリースが共有ツリーを detached のまま放置する問題を根治）
- リリース作業を**使い捨て worktree**（`git worktree add --detach` → trap で撤去）へ移し、
  install_root の HEAD を一切触らない形にした。成功・ビルド失敗・片肺・SIGTERM のどれでも main のまま
- 重複コミットの経路は「ビルド中に origin/main が進む → push 拒否 → `set -e` で無言死 →
  未 push のリリースコミットごと detached が残る」。押し出す前に先端を照合して中止するようにし、
  多重起動ロックを HOME 単位 + **リポジトリ単位**の 2 段にした（$HOME が違う並走を止められていなかった）
- 検証: test-nightly-reserve 129 PASS（新規 7 本・CI へも載せた）/ test-release-retry 55 PASS /
  A/B で修正前は 20 FAIL。失敗経路 4 種の HEAD before/after を隔離環境で実測

## 2026-09-07（#1154: master / solo の system prompt を起動時ロード予算の対象にして手順書へ分離）
- 手順の詳細を `orchestrator/guides/*.md` の 11 topic へ**原文そのまま**移し、`tako orchestrator guide <topic>` /
  MCP `tako_orchestrator_guide` で引く形に。予算 24 KB を `context_budget` へ追加し、超過は block 別バイト数で提案
- takodev の生成物 59,501 B / 40,687 tok → **32,626 B / 20,366 tok**（既定 blocks 単体 21,072 B）。全 topic の
  guide 出力が移送前ブロックと byte 一致 / A/B `TAKO_1154_LEGACY=1` で新テスト 3 本が FAILED / 全 3542 件緑

## 2026-09-07（#1160: 仮想ディスプレイが列挙で空のときメイン画面へ窓を開かないようにした）
- 原因は物差しの取り違え: `cx.displays()` = `CGGetActiveDisplayList` なので眠っている面は NSScreen に残ったまま
  列挙から落ちる（実測: 2 枚とも `CGDisplayIsActive=0`）。1 回引いて即 `NotFound` → 既定の面へ落ちていた
- 核（純粋）の `retry_policy` / `miss_for` でやり直し（空のあいだだけ・100ms × 検証 20 / 通常 3）と落とし所を決め、
  **列挙が空 + 検証用は窓を開かず終了**（コード 4 + stderr）。面が見えて外したときは落ちるが stderr で警告する
  （そこで止めると `build-app.sh --verify` と Windows 実機の検証が起動できない）。`ensure` は描画可能までを完了条件にした
- 実測: 眠ったままの隔離起動が exit=4 で無窓 / `ensure` 後は `やり直し=5 回` → tako-vd へ解決 / A/B は pre-fix で core 3・番犬 3・shell 17 件が FAILED

## 2026-09-07（#1162: セルフテスト項目 102 の `shown=false` を根治。実因は負荷ではなくペインの高さ）
- 落ちた回の実測は `size=Some((58, 9))` = 13 行の 25 桁 fixture の箱上端と `❯ 1.` が画面外。fixture ペインを
  **専用タブ（全高）**へ移し、実寸が届いてから描く（`notify_and_draw`）+ 出現判定を状態待ちへ（`wait_for_dispatch_state`）
- A/B `TAKO_1162_LEGACY=1` は load 2.5 でも 102 が確定 FAILED / `TAKO_1162_INJECT=nodialog` は 3 回送り直しても FAILED
- 項目 102 は 15/15 ok（判定時 load 3.4〜5.0）・完走 8 回。中断は #816 / #1058 / #694（別件の負荷フレーク）

## 2026-09-08（#1153: セルフテストの高負荷フレーク 4 系統を状態待ちへ寄せた）
- (e) #694 / (g) #1058 は `pane_display` の材料（`state=Running` / 猶予 #720 が role で 25 秒へ伸びる）を
  1 回読みしていたのが原因。113 #816 は固定 2500ms 窓の「40 行増えた」、#657 は総数の差分（#1124 の `tabs=8->4`）
- 全 4 系統を状態待ち + `state_wait_budget` へ。#657 は押す前のタブ ID 集合で判定。番犬
  `固定窓のあいだに増えた数を測っていない` を追加（origin/main の 657 のループを名指し）
- GUI 16 回（tako-vd）: 無負荷 1 + 人工高負荷 3 連続 OK（load 9.1〜15.9）/ A/B は `TAKO_1153_LEGACY=1` +
  遅れの注入 4 種で旧経路が確定 FAILED（(g) は Issue と同じ JSON）/ 回帰の注入 4 種は新経路でも FAILED

## 2026-09-08（#1165: セルフテストの画面エコー待ちを固定窓から状態待ちへ寄せた）
- 項目 1b の固定 8 × 800ms 窓が load 12.9 で尽き 1c 以降が全部走らなかった。同型 9 か所を
  `type_until_focused_text`（`state_wait_budget` + 送り直し 2 回 + `TAKO_SELF_TEST_1165` の診断）へ
- 番犬 `固定窓のあいだに画面の文字列を待っていない` を追加（pre-fix の実ファイルを 9 行名指し・旧番犬は見逃し）
- GUI（tako-vd）無負荷 2 + 高負荷 1 完走（判定時 load 21.9〜34.6）/ `INJECT=late` で旧のみ FAILED / `noecho` は新も FAILED

## 2026-09-08（#1167: 「追記ぶんだけ読む」の検査を実時間から読み出しバイト数へ替えた）
- `claude_remote_link` の効果テストが `Instant::elapsed` の全走査比較だったため高負荷で確率的に落ちていた。
  走査本体を `scan_source<R: Read + Seek>` へ出し、テストは**実際に読んだバイト数**を数える読み口を渡す形へ
- 実測 全走査 4,194,592 B / 追記ぶんだけ 355 B（上限 64 KiB）。番犬 `追記ぶんだけ読む検査を実時間で測っていない` +
  conventions の新節。A/B は注入 5ms で旧 19/20 FAILED・新 0/20、「全文を読むが consumed は正しい」注入で新が FAILED

## 2026-09-08（#995: セルフテスト項目 108 の高負荷フレークを窓ガードで根治した）
- 外から来る全体 notify で `output=(panes +2 chrome +2)` = 意図的な全体 notify と同値になっていた。#858 の
  ガードを 108 へ寄せ、判定（`redraw_window_clean`）と窓の作り方（`measure_output_redraw`）を 110 と 1 実装に統合
- **証人は測る量の外側から選ぶ**: 110 = クローム / 108 = 本体の増分が 2 以上（ヘッダは #803 の A/B で毎フレーム動く）
- A/B は `TAKO_995_LEGACY=1 INJECT=app` が Issue と同じ数字で FAILED・新経路は 2 回目で通過・`INJECT=chrome` は
  新経路でも FAILED。110 の既存注入 2 種は #858 の表を再現。全 3576 件緑

## 2026-09-08（#1122: 器つきペインの visual-test カーソルラウンドを実測で切り分けて根治した）
- 原因 2 つ（見立ての「tmux が描く」は外れ）: 検査側 = 測るあいだミラーが立って表示が tmux 履歴 / 製品側 = `parse_ansi_lines` が履歴行にカーソルを焼いていた
- 前者は製品と同じ `cancel_scroll_before_input` を通してから書く形へ・後者は `show_cursor=false` へ。器つき 5 通りが直接ペインと同一値（`fill=544 / stray=0`）で緑・**skip 無し**
- A/B は `TAKO_1122_LEGACY=1` が Issue と同じ `fill=272/0/0/0` で FAILED。全 3578 件緑。次に落ちる `subline` 節（#943 の前提ガード無し）は #1173 へ起票
