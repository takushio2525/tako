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

## 2026-09-14（#1459: 右パネルの手書きテキスト入力 2 本を TextField へ寄せた）
- git のコミット欄とブランチ名欄が持っていた同型の編集実装（`floor_char_boundary` で丸めてから backspace / delete / 左右 / Home / End / 挿入）を `TextField` の 1 実装へ。状態も `String` + カーソルから `TextField` 1 つへ畳んだ（`GitBranchInput.start_point` は不変）。割り当て（`⌘Enter` / `Esc` / `⌘V` / 1 行欄の上下→端）は各画面に残す
- **丸めは `text_field.rs` の非公開関数へ移した**ので、他ファイルが同じことをするには自前で書き直すしかない（= 番犬のマークに必ず掛かる）。B2 番犬の猶予表は空になり、走査は `tasks_panel.rs` + `right_panel.rs` の全面適用 + 「打鍵ハンドラが `handle_edit_key` を通す」の正検査つき
- 実測: 隔離セルフテスト（tako-vd）`TAKO_APP_SELF_TEST_OK`。A/B は委譲を切る注入で項目 79 / 82 が名指し FAILED（項目 81 まで通過を確認）。番犬は注入 11 通り + 実注入で `right_panel.rs:4643/4647` を file:line 名指し。workspace 4848 passed 0 failed・clippy 3 宇宙 0・check-windows error 0。**リファクタなので install 不要**

## 2026-09-14（#1450 B4: 人がやることの最初の中身を本番へ入れ、引き継ぎ手順書に「人待ちはここへ」を足した）
- 本番 `tako todo` へ **19 件**（post 2 = #1081 解説動画 v6 / #1284 X ショート・permission 4・review 12・confirm 1）。投稿 2 件は投稿文 / タイトル / タグを `copy_texts` に分け、動画とサムネを添付（`exists: true` を実測）。引き継ぎの「ユーザー確認待ち」18 項目のうち①は投稿 2 件へ畳んだ。登録は冪等（同じ title はスキップ）
- リポは `guides/handoff.md` に 1 段落（**人待ちは引き継ぎファイルではなく `tako_todo` へ**）。番犬の要求どおり `guides_added_after_1154.md` へ同文を宣言（宣言を外すと 7 行を file:line で名指し FAILED）。`user-tasks` 手順書と `.agent/orchestrator.md:917` は既に同じことを書いているので変更不要
- 実測で穴 2 件を発見して起票: **#1466**（worker 起票の返答が無関係な `default` master へ届く。B4 は role 明示で回避）/ **#1467**（MCP `tako_panel` の view enum に `tasks` が無い = 設計原則 5）

## 2026-09-14（#1467: MCP カタログの enum を正本から生成するようにした）
- `tako_panel` の `view` が手書きの写し（`["fleet","orch","git","tmux"]`）で #1450 B2 の `tasks` に追従していなかった。`PanelViewWire` へ `summary()` / `accepted_values()` / `values_summary()` を足し、catalog は `panel_view_schema()` で受理値も説明文も生成する。旧称 `tmux` は**落とさない**（enum から消すと今動いているクライアントが送れなくなる）
- 棚卸し: catalog の `"enum"` は 102 か所 / 値集合 75 種。正本が実行時に読めるのは 10 種（20 site）だけで、うち「MCP の正本」を名乗っていた 5 つ（Panel / ProfileKind / SessionRestartMode / UiMode / RemoteOpenTarget）を生成へ寄せた。残り 52 種は正本なし（action 動詞）か正本が非公開・列挙 API なし = Issue にコメント
- 番犬 `issue1467_mcp_enum_watchdog`（5 本・注入 8 通り + 実ファイル注入で `catalog.rs:695` を名指し）。スナップショット `mcp_tools_full_snapshot.json` は `tasks` の追加ぶんだけ差分。tools/list 実出力と隔離 GUI の MCP 呼び出しで実測。workspace 4875 passed 0 failed・clippy 3 宇宙 0・check-windows error 0。**install 要**

## 2026-09-14（#1466: worker が起票したユーザータスクの返答を spawn 元の master へ返した）
- 戻り先の解決順を「名乗った master → ペインの role → **spawn 元**（`spawned_by` を辿る `find_master_suffix_from` = worker spawn の既定と同じ 1 実装）→ 管轄プロファイル（**一意のときだけ**）」へ。判断は `user_tasks::resolve_origin_profile` の純粋関数 1 本で、dispatch は材料を集めるだけ。**新しい永続フィールドは 0**（`spawned_by` と role ラベルは既にある）
- **解けなければ `default` へ落とさない**（`origin_profile` を `Option` 化。配送は `failed` + `宛先不明: …` で残り `tako todo show` に出る）。名乗り（`created_by`）は戻り先ではなく**呼び出し元自身の役割**から作る（worker が `master:<profile>` を騙らないため）
- 実測 `scripts/test-user-task-origin-1466.sh` **28 PASS 0 FAIL**（実 spawn の worker → CLI / MCP とも `origin.profile` が spawn 元・返答が master のペインへ・role なし / solo / 管轄なしは `failed`・master を閉じても管轄から同じプロファイルを起こす）。A/B `TAKO_1466_LEGACY=1` で症状再現（無関係な default の master のペインへ届く）。#1450 B1 の e2e 45 PASS 0 FAIL（回帰なし）・番犬 4 本（注入 8 通り）・workspace 4889 passed 0 failed・clippy 3 宇宙 0・check-windows error 0。**install 要**

## 2026-09-14（#1472 B: スマホのタスク画面で添付をその場で見る・鳴らす）
- PWA `#/tasks` の詳細で画像を `<img>`（タップで全画面）・動画を `<video controls preload="metadata">` に。**経路は 1 本も増やしていない**: src は既存 `/api/files/download` に `disposition=inline` を足しただけで、認可は `resolve_in_root` のまま（observe は 403・interact はツリー配下だけ）。daemon 側は同じ経路に `Range`（206 + `Content-Range`・416・`Accept-Ranges` 常時）と inline の `Content-Type` を足し、**応答の送出は `respond_file` の 1 か所へ畳んだ**（枝ごとに書くと `no-store` の付け忘れが生えるため）。画像 / 動画の判定と MIME は `tako_core::open_plan` の 1 本（`preview_route` と `media_type` の一致を単体テストが拘束）で、daemon が `attachments[].preview` に載せる = PWA に拡張子の表を作らない
- 実測: 実経路 `scripts/test-remote-attachment-preview-1472.sh` **59 PASS 0 FAIL**（先頭 / 途中 / 末尾チャンクの**中身が実体と一致**・416・150 MB の部分取得で daemon RSS 23 MB・observe / interact の 403）+ e2e 新 8 本（実 PNG の `naturalWidth` と実 webm の `readyState` / `duration` を実測・**一覧では GET 0 本**）。A/B は daemon 24 件 FAILED / PWA e2e 6 本 FAILED。番犬 6 本・注入 8 通り file:line 名指し
- workspace 4901 passed 0 failed・clippy 3 宇宙 0・check-windows error 0・e2e 101 passed・docs 32 ページ。**install 不要 / 本番 remote daemon の再起動が要る**

## 2026-09-14（#1472 A: 人がやることの添付を、行を押すだけで既存プレビューで開けるようにした）
- #1450 B2 の「プレビューで開く」は**300px の帯の 10px ボタン**で、ユーザーには「押しても中身が見られない」に見えていた（Issue の前提「PC はパスの表示だけ」は不正確で、実測では `Request::OpenFile` 自体は png→image / mp4→video / md→markdown で正常動作）。**行そのもの**を押せるようにし、左に「画像 / 動画 / PDF …」の種別・ホバーで「プレビューで開く」を出す。**新しいビューアも拡張子の表も作らない**（`open_plan::preview_route` + `Request::OpenFile` = `tako open` / `tako_open_file` と同じ 1 経路）
- 画像添付にサムネイル。**縮小後だけを持ち**（320x180 枠）、上限はファイル 32 MB とヘッダの画素 64M の 2 段で**画素は decode の前**に見る（解凍爆弾）。読むのは背景スレッド・持つのは開いている 1 件ぶんだけ（`retain` で溜まらない）。動画のサムネは作らない（ffmpeg 依存を一覧の描画に混ぜない）
- 実測: visual-test 項目 151 新設（`TAKO_VISUAL_ONLY=task-attachment`）で**合成マウス**が実フレームの hitbox を押し、image / video が開く・2 回押してもペインが増えない・消えた添付は押せないを確認（ハンドラ直呼びの項目 150 では #496 型を検出できない）。注入 A/B は行の `on_click` を切ると項目 151 が `[]` で FAILED。番犬 `issue1472a_attachment_open_watchdog`（3 本・注入 9 通り）。**install 要**

## 2026-09-14（#1473: 蓋閉じ継続をバッテリー駆動でも opt-in で続けられるようにした）
- 蓋閉じ継続の電源条件を**アイドルスリープ側とは別の軸**（`lid_sleep_power`・既定 `ac-only` = 現状維持）にし、`always` のときだけバッテリーでも続ける。安全弁は 4 つ（エージェント稼働中のみ / 残量が下限（既定 20%・5〜90%）に**達したら**解除 / 温度は**バッテリーなら fair 以上・AC なら serious 以上**で解除 / 残量を読めない機械では継続しない）。Windows は同じ判定を通り `always` のときだけ電源プランのバッテリーレールも倒す（残量取得は未実装 = 実質 AC のみ・実機未検証）
- 判定は `lid_decision(&LidGuardInput) -> Result<(), LidSkipReason>` の 1 本で、真偽値ではなく**理由**を返す。理由は状態が運ぶ（`update` / `status` が `with_decision()` で埋める）ので、CLI・設定画面・通知欄・persist.log は**読むだけ**（読む側が再計算すると A/B や stale binary で判断が割れる = #372 と同じ理屈）。通知欄へ出すのは安全弁の解除と回復だけ
- 実測（隔離 GUI / tako-vd。実機はバッテリー 52% 駆動）: `always` + 下限 10% + エージェント 1 体で実機の `SleepDisabled=Yes` を 8 サンプル観測（persist.log に `lid-sleep: … reason=applied battery=15%`）・注入 15% では倒さず理由 `battery-floor`・MCP で書いた値を CLI が読む・範囲外（0 / 95）は両口とも拒否・A/B `TAKO_1473_LEGACY=1` は `always` でも「AC 未接続」で降りる。**検証後に `SleepDisabled=No`（検証前と同値）へ戻したことを確認**。workspace 4937 passed 0 failed・clippy 3 宇宙 0・check-windows error 0・docs 32 ページ・番犬 3 本（注入 9 通り + 実注入で `sleep_guard.rs:1162` を名指し）。**install 要 / 実機の蓋閉じは未検証**
