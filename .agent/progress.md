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

## 2026-09-08（#771: 実 claude e2e の待ちを固定窓から負荷追従の予算へ替え、真因を #1175 へ切り出した）
- 101c の固定 300 秒窓を `wait_for_claude_state`（`state_wait_budget` + 診断 2 行 + `prompt_flow=` + `101c-SCREEN`）へ。
  同型 14 か所（45c / 95c ×11 / 97c ×2 / 101c）を同じ予算へ。番犬 `実claudeの応答を固定窓で待っていない`（待ちがループ末尾にある形を #1153 / #1165 は見逃す）
- **実測で待ちは無罪**（予算 465〜701s に対し実際 93〜242s で `ok=true`）。落ちているのは目印の観測側 = スクロールするビューポートを見ている → #1175 へ起票

## 2026-09-08（#1173: 器つきペインの visual-test を完走させた）
- `subline` は待てば器つきでも直接ペインと同一値（`mirrored=true mirror_pos=0.500 direct=13961 shifted=0`）=
  製品は動いているので #943 型の skip は要らなかった。後続 3 件も器つきだけ落ちる同型（描画途中の resize /
  固定 800ms 窓 / 注入 fixture の `pin_chat_fixture` 漏れ）
- 到達点 器つき 72 行 → **158 行 + `TAKO_VISUAL_TEST_OK`**（直接も 149 行で OK・A/B 両アーム緑）
- A/B は `TAKO_1173_LEGACY` を段ごとに指定（`subline` `rows` `chat-g3` `pin` が確定 FAILED）

## 2026-09-08（#1177: 器つきペインの `term-grid scroll` を skip せず実測できるようにした）
- #943 の前提ガードを外し「ミラーが立つまで待つ + 期待値の出どころを器で切り替える」へ（`settle_scroll_mirror` は `subline` 節と 1 実装）
- 器つき `mirrored=true mirror_pos=0.500 fract=0.500 shift=17 expected=17` = 直接ペインと全数値一致・両方 `TAKO_VISUAL_TEST_OK`（checkpoint 122）
- A/B `TAKO_943_LEGACY=1` は #943 の報告と同一数値で FAILED / 注入 `nofract` は上限待ちで FAILED / 番犬 1 本追加 / 全 3588 件緑

## 2026-09-08（#1175: 101c の引き継ぎ到達判定を後任の transcript から採るようにした）
- 引き継ぎ本文は claude の TUI で 1 度だけ流れる User 発話なので `visible_lines()` 判定は後任が長く働くほど確実に落ちていた。証拠源を `chat_state` の発話へ移し、`saw_done` は Assistant 限定に（旧は手順書の「引き継ぎ完了」を後任の申告と誤読）。前提（GUI / persist）は項目内で上げて戻すので起動レシピは不変
- 実 claude e2e 2 回連続 OK（`chat=1/6`・`1/5`・load 2.5〜3.0）/ `TAKO_1175_LEGACY=1` は Issue と同じ `saw_marker=false` で FAILED / `INJECT=nomarker` は新経路でも FAILED
- 番犬 `実claudeの発話をビューポートで判定していない`（修正前の 52907 / 52908 を名指し）+ 純粋関数の単体 5 本。全 3595 件緑

## 2026-09-08（#1180: セルフテストの器の往復待ちを予算へ移し、期限つき状態の混在を根治した）
- Issue の grep（`0..25`）は失敗項目②（`0..20`）を漏らしていたので洗い直し、**18 か所**を `wait_for_backend_state`（状態待ち + `state_wait_budget`・駆動は毎周期）へ。番犬は region（tmux ゲート内の固定窓）+ `read_to_string` needle + #1162 へ `tako_control::dispatch(`
- **真因は待ちの長さだけではない**: 項目 73 は「待てば立つミラー」と「1.4 秒で消えるスクロールバー」を 1 回のホイールのあと同時に見ていた（実測 `bar=false`）ので予算では解けない → ホイールを毎周期打つ形へ
- 無負荷 + load 13.7〜16.1 で `TAKO_APP_SELF_TEST_OK` / A/B は `LEGACY=68-attach|73-wheel` + `INJECT=late` が FAILED（`waited=10.1s budget=10.0s` / `6.2s/6.0s`）・新経路は通過 / `INJECT=never` は新も FAILED
