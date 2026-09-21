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

## 2026-09-14（#1473: 蓋閉じ継続をバッテリー駆動でも opt-in で続けられるようにした）
- 蓋閉じ継続の電源条件を**アイドルスリープ側とは別の軸**（`lid_sleep_power`・既定 `ac-only` = 現状維持）にし、`always` のときだけバッテリーでも続ける。安全弁は 4 つ（エージェント稼働中のみ / 残量が下限（既定 20%・5〜90%）に**達したら**解除 / 温度は**バッテリーなら fair 以上・AC なら serious 以上**で解除 / 残量を読めない機械では継続しない）。Windows は同じ判定を通り `always` のときだけ電源プランのバッテリーレールも倒す（残量取得は未実装 = 実質 AC のみ・実機未検証）
- 判定は `lid_decision(&LidGuardInput) -> Result<(), LidSkipReason>` の 1 本で、真偽値ではなく**理由**を返す。理由は状態が運ぶ（`update` / `status` が `with_decision()` で埋める）ので、CLI・設定画面・通知欄・persist.log は**読むだけ**（読む側が再計算すると A/B や stale binary で判断が割れる = #372 と同じ理屈）。通知欄へ出すのは安全弁の解除と回復だけ
- 実測（隔離 GUI / tako-vd。実機はバッテリー 52% 駆動）: `always` + 下限 10% + エージェント 1 体で実機の `SleepDisabled=Yes` を 8 サンプル観測（persist.log に `lid-sleep: … reason=applied battery=15%`）・注入 15% では倒さず理由 `battery-floor`・MCP で書いた値を CLI が読む・範囲外（0 / 95）は両口とも拒否・A/B `TAKO_1473_LEGACY=1` は `always` でも「AC 未接続」で降りる。**検証後に `SleepDisabled=No`（検証前と同値）へ戻したことを確認**。workspace 4937 passed 0 failed・clippy 3 宇宙 0・check-windows error 0・docs 32 ページ・番犬 3 本（注入 9 通り + 実注入で `sleep_guard.rs:1162` を名指し）。**install 要 / 実機の蓋閉じは未検証**

## 2026-09-14（#1477: master system prompt の取り分を tako と利用者で分け、追記を自動分割）
- system prompt 24 KB を **tako の base 18.5 KB + 追記 5.5 KB** に分割。委任の判断材料（`delegate_guidance` + judgment）を新 topic `delegation`（動的 guide）へ出し、behavior / monitoring / guides / context-budget の案内文を締めた。実測 base: default 20,732→17,480 / codex 22,695→17,507 / fable 22,507→17,493 / takodev 21,808→18,450（**ルールは 1 つも消していない**）
- 予算を超えた `prompt_blocks.append` は自動移行（`SchemaId::PromptAppend`）が見出し境界へ `<!-- tako:on-demand -->` を**1 行入れるだけ**。後ろは `tako orchestrator guide local-rules` で引き、prompt には生成した索引 1 行（`append index` piece）。`strip_marker(新) == 旧` が不変条件
- 実測: `scripts/test-prompt-append-split-1477.sh` **31 PASS 0 FAIL**（隔離 HOME + `TAKO_DATA_DIR`）。番犬 11 本（注入つき）・A/B `TAKO_1477_LEGACY=1` で base 18,772 / 追記 12,318 の分割前へ戻る

## 2026-09-16（#1479: tasks ビューを押した行の直下で開き、タブのバッジを幅で欠けさせない）
- A: 詳細を**行の器の子**へ移し（旧実装は列の末尾に 1 枚 + 「選択が無ければ先頭」で畳めなかった）、`toggle` / `collapse` の 1 実装で「再押下で閉じる・別項目で乗り換える・絞り込みで畳む・ポーリングでは畳まない・`done` で畳む（dispatch 側 1 か所）」に。展開した行は `scroll_to_item` で見える位置へ。矢印は `CHEVRON_*`
- B: タブ列を `panel_tab_density` の 3 段（Full / Compact / IconsOnly）+ belt（バッジ `flex_none` / ラベル `min_w(0)`）で詰め、件数は `panel_tab_badge`（0 件は出さない・100 以上は `99+`）。220〜900px × 0/9/22/150/4000 件を単体テストが 1px 刻みで固定、visual-test 項目 152 が**バッジの実矩形がタブ列の内側**かを 4 幅 × 4 件数で読む（A/B `TAKO_1479_LEGACY=1` は 10/16 で FAILED = 320px で 9px 溢れる）
- MCP / CLI 1:1: `tako todo expand <id>` / `collapse` + `list` の `expanded`。番犬 `issue1479_tasks_accordion_watchdog`（注入 11 通り）・実経路 `scripts/test-todo-expand-1479.sh`

## 2026-09-18（#1482: ドキュメントサイトを tako.takushio2525.com へ移行）
- `docs/astro.config.mjs` の `site` を新ドメインへ（canonical / og:url / og:image / sitemap はここ 1 か所が基点なのでまとめて追従）。About の `DOCUMENTATION_URL` は「`site` と同じ URL」という設定コメントの約束があるので同一コミットで動かす
- 旧ドメインの転送は `docs/functions/_middleware.js` の 301。`*.pages.dev` はゾーン外で Redirect Rules / Bulk Redirects を書けず `_redirects` はホスト名を条件にできないため、Pages Functions が唯一の経路。プレビュー配備を潰さないようホスト名は完全一致で見る
- OG 画像 31 枚を再生成（フッター文言。未生成だった features/user-tasks もここで揃った）。ビルド 32 ページ緑 / og:verify 緑。カスタムドメインの追加は master が実施

## 2026-09-18（#1481: Web ビューを見たあとターミナルへ戻ったとき打鍵が届かないのを直した）
- 真因は AppKit 側だった: 宛先を親へ返す `focus_parent()` が `sync_frame` の **hide 分岐にしか無く**（`webview.rs:268`）、Web ビューが見えたままフォーカスが別ペインへ移る経路（`on_pane_mouse_down` / dispatch `Focus`）は `PaneTree` のフォーカスしか動かさない。#326 の NSEvent monitor は **⌘ 修飾つきのキーだけ**（`webview.rs:582`）なので素の打鍵は救われない。破棄（× / `web close`）も宛先を持ったまま壊していた
- 宛先の読み・戻しを `webview.rs` の 1 実装へ（#326 の monitor も同じ関数を通る）。**戻す先は `contentView` ではなく「宛先を持っている WKWebView の superview」**（実測: contentView = `AccessKitSubclassOfNSView` で `makeFirstResponder:` は成功するのに打鍵は来ない）。呼ぶのは `clear_text_input_focus`（= #503 と同じ全経路）と破棄の直前で、**毎フレームの level 判定にはしない**（「宛先が webview でフォーカスは別ペイン」は「いまページをクリックした」と同じ状態なのでページへ打てなくなる）
- 実測: 項目 71 に #1481 群 8 件（宛先のクラス名を読む）。legacy `TAKO_1481_LEGACY=1` は `owner=WryWebView returns=0` で FAILED = Issue の症状そのまま / 修正アームは `TAKO_APP_SELF_TEST_OK` 完走。番犬 `issue1481_webview_key_focus_watchdog` 3 本（注入 8 通り）・workspace 4982 passed 0 failed・clippy 3 宇宙 0・check-windows error 0。**実マウスでのクリックと GUI 再起動後の復元は未検証**

## 2026-09-21（#1485: 「起動していた」を覚えて GUI 起動時に remote daemon を立て直す）
- `tako remote start` の成功だけが `<data_dir>/remote/tako-remote.desired`（移行の番地 `SchemaId::RemoteDesired`）を作り、GUI が persist 復元のあと `spawn_daemon` をバックオフ（2/5/10/20/40 秒）で数回試す。**消す条件は「止まっていること」の 1 つ**（`Ok` だけ見ると Mac 再起動後に stop した人の意図を落とす）。判断は理由を返す純関数 `autostart_decision` 1 本で、CLI / チップ / persist.log は読むだけ
- 無言にしない: 途中の失敗も persist.log へ 1 行ずつ、諦めたら `notify_ui_failure` の 1 実装 + `remote status` の `desired` / `last_autostart`（`running: false` の応答にも必ず載る）。OS ゲートは `platform::support` の `tako_remote_start` を引く（専用キーは足さない = T2 が落ちる）
- 実測: `scripts/test-remote-autostart-1485.sh` **26 PASS 0 FAIL**（隔離 GUI + 実 daemon）。A/B `TAKO_1485_LEGACY=1` は同スクリプトで 13 FAIL（①が「自動復帰しない」= Issue の症状）。番犬 4 本・注入 10 通り + 実注入で `remote.rs:339` を名指し。workspace 5002 passed 0 failed・clippy 3 宇宙 0・check-windows error 0

## 2026-09-21（#1487: タブを「ー」で送ったらタブ単位で退避・復帰できるようにした）
- `Workspace::shelved_tabs`（`Tab` 本体 + 由来ウィンドウ + 元の並び位置）を追加し、`shelve_tab` は `into_panes()` の平坦化をやめて**タブを分解せず**移す。判断は純粋関数 2 本（`unshelve_tab_placement` / `shelved_tab_fate`）。永続は `LayoutFile.shelved_tabs`（serde default = 旧ファイルそのまま読める・移行 Step 不要）
- 「バックグラウンドに居るペイン」を見る側は `all_background_panes()`、「まだ生きているペイン」を見る側は `all_pane_ids()` の 2 本へ寄せた（**受け入れ検査 rework 1 回目**: コマンドカードの `retain` / 退避バッジの件数 / `Pin { group_tab }` の検証 / 見出しタイトル / 「閉じたタブ」群の 5 か所が退避タブを見落としていた）
- 実測: `scripts/test-shelve-tab-1487.sh` **24 PASS 0 FAIL**（退避→復帰で tree/rect/title/title_source 一致・再起動往復・1 ペイン抜き・旧 layout.json・A/B `TAKO_1487_LEGACY=1`）+ visual-test 項目 153（合成マウス）。番犬 3 本（注入 15 通り・実注入で `workspace.rs` を名指し）

## 2026-09-21（#1490: 隔離 GUI の起動を scripts/lib の 1 実装へ寄せ、起動ごとに tako-vd を起こす）
- `scripts/lib/isolated-gui.sh` を新設（`isolated_gui_bins` / `launch_isolated_gui` / `wait_isolated_gui` / `stop_isolated_gui`）し、`virtual-display.sh ensure` を**起動の直前に毎回**通す形へ。`scripts/test-*.sh` 12 本を差し替え（-151 行）。Issue の grep は `"…virtual-display.sh" ensure` の引用符に当たらず全部 0 に見えていたが、正しく数えると「起動の直前に通していたのは 4 本 / 冒頭 1 回が 6 本 / 呼んでいないのが 2 本」で症状は実在
- 実測（眠った tako-vd から。`pmset displaysleepnow` で再現）: **master-launch が OK=0 rc=1 → OK=34 rc=0**・**1485 が OK=12 NG=14 → OK=26 NG=0**（14 NG は全部「窓を開かずに終了」= #1160 の中止）・**worker-min-width が OK=6 → OK=12**（3 腕のうち 1 腕しか走れていなかった）。他 9 本は前後一致・回帰 0
- 番犬 `issue1490_isolated_gui_launch_watchdog` 5 本（直書き 4 形 + 手書きの背景起動 / ensure を file:line で名指し）。注入で 2 本 FAILED → 戻して 5/5 緑。workspace 5021 passed 0 failed・clippy 3 宇宙 0・check-windows error 0
