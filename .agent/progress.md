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

## 2026-09-14（#1453: 素の master をその場で専用プロファイルへ寄せられるようにした）
- 新しい永続状態を 1 つも足さず、**ペインの role ラベル 1 つ**の書き換えで実現（`resolve_master_profile` が非既定の pane_role を優先する #854 が土台。role は layout.json に載るので GUI 再起動もまたぐ）。`self` / spawn 既定 / `handoff` の後任と宛先 / 自動ハンドオフ #749 は既存の 1 実装のまま追従。`projects add` は `profiles/<key>.yaml` を default から継承して作り（管轄と cwd だけ差し替え・冪等）、既存プロジェクトは migration の登録簿（`FileOutcome::Created` を新設）で揃う
- 実測（tako-vd 上の隔離 GUI・A〜G）: 生成 → 採用 → spawn 制限 → 引き継ぎまで通し。採用で `pane_id` 不変のまま `profile` が `default` → `<key>`・`successor_command="tako master -<key>"`・管轄外 spawn は `projects 制限` で拒否。`TAKO_1453_LEGACY=1` は生成も採用も起きず修正前を再現。拒否の 2 種（専用起動 master / `master_agent` の食い違い）は直す 1 コマンド付き
- **罠**: CLI の `projects add` が dispatch の写しを持っていて**MCP からだけ生成が効いていた**（実測で発覚）→ 1 本へ寄せ、番犬で縛った。system prompt は 19,679 → 19,912 B（予算 24,576 B 内）。番犬 4 本（注入 7 通りで file:line 名指し）+ 単体 14 本・セルフテスト項目 149（legacy 腕は 149a で FAILED）。workspace 4695 passed 0 failed・clippy 3 宇宙 0・check-windows error 0。**install 要**

## 2026-09-14（#1452: 権限が足りませんに導線を付け、端末ごとの権限編集を PC へ置いた）
- スマホは #283 からある `POST /api/pair` を押せる場所に出しただけ（**新 API は `POST /api/admin/devices/role` の 1 本**）。`GET /api/me` が登録済み端末にも `pending` / `requested_role` / `denied` を返すようにし、承認待ちの間だけ 2 秒ポーリング → 承認で**再読込なしで**続きができる。導線は `permission-request.jsx` へ切り出し `files.jsx` は 2 行差分（#1451 との衝突回避）
- 昇格を PC の人へ縛るのは 3 段: 方向（403 `upgrade_requires_gui`）/ 接続元プロセスの名前ゲート（#841 の `procinfo` を流用）/ **dispatch の手前での拒否**（MCP・CLI の IPC は tako-app の中で走るので名前ゲートだけでは AI を止められない = 実装中に見つけた穴）。経路表は `remote_role::ROLE_ROUTES`、依頼は `tako todo`（kind=permission）へ 1 件（**授権の正本は daemon のメモリのまま** = done しても権限は動かない）
- 実測 `scripts/test-remote-role-1452.sh` **58 PASS 0 FAIL**（CLI / MCP / curl の昇格が全部拒否・GUI を名乗った承認だけ 200・監査に `caller_check`・再起動をまたいで記録だけ残る）・e2e 71 passed（新 10 本）・番犬 9 本。**罠**: 承認にゲートが掛かったので #1449 / #1078 の実経路テストが全 403 になる（`TAKO_REMOTE_TRUSTED_ADMIN_NAMES` を宣言して復旧 = #841 と同じ形）。**install 要 / 本番 daemon 再起動要**

## 2026-09-14（#1447: preview パネルつき AskUserQuestion（2 カラム配置）を検知できるようにした）
- 真因は枠の同居**ではなく**折り返しの継続行の字下げ（実採取は中身の桁 5 に対して 4）。`gap_line_kind` が「ダイアログの外」に分類し `numbered_block` が選択肢 1 個で打ち切っていた。注入 A/B で名指し（枠を剥がしただけでは None のまま / 継続行を 5 桁へ揃えると修正前でも採れる）
- 下限を `dialog::wrap_indent_floor`（マーカー `N.` より右）の 1 実装へ寄せ、歯止めに「選択カーソル行・番号つき行は続きにしない」。副画面は判定前に `side_panel_cuts` → `strip_side_panel` で剥がす（3 条件つき。全幅の箱の右端では発火しない）。エッジで実バグ 2 件を発見して修正（パネルだけの行が下の選択肢ラベルへ結合 / 内側の余白を掴む）
- 実測（隔離 GUI・tako-vd）: `status` の `choice_dialog` が 3 択・`watch` が `WORKER_DIALOG: tako:1 (select)`・`respond --choice 2` が `resolved:true` / `keys_sent:["2"]`（模擬 TUI 側も `KEY=2 / CHOSE=自分で流す`）。`TAKO_1447_LEGACY=1` は `choice_dialog: null` / `WORKER_TIMEOUT` / 「画面に存在しない」で症状を再現。番犬 3 本・注入 8 通りで file:line 名指し。workspace 4805 passed 0 failed・clippy 3 宇宙 0・check-windows error 0。**install 要**

## 2026-09-14（#1445: 番犬の共有部品が合成 cfg のテストモジュールを本番として走査していた）
- `production_range.rs` のテスト領域検出をリテラル `#[cfg(test)]` から**cfg 述語の解釈**へ（`mentions_test`: `test` を正の位置に含むものだけ潰す）。`not(test)` は本番として残し、`cfg_attr` は入口の綴りごと対象外。#1441 の番犬が持っていた自前正規化は削除して 1 実装へ戻した
- 走査範囲は src 263 本中 **8 本**が変化（合計 74.34% → 74.04% / 新たに潰れたのは合成 cfg の 11 item ちょうど）。**下限 30% を跨いだファイルは 0 件**（表は Issue #1445）。A/B `TAKO_1445_LEGACY=1` では #1441 の番犬が `discovery.rs:343` をテスト内の直書きなのに本番違反として名指しで落ちる = 誤検出の再現
- 新設 `issue1445_cfg_predicate_watchdog`（14 本・注入 8 通りすべて file:line 名指し）。workspace 4819 passed 0 failed・clippy 3 宇宙 0・check-windows error 0。**テストのみ = install 不要**

## 2026-09-14（#1450 B2: 人がやることを右パネルで見て、その場で返す）
- 右パネルに 4 本目のビュー `tasks`。`PanelViewWire` へ 1 枝足すだけで CLI の possible values・`--help`・不正値の案内・MCP の説明が追従する（`tako panel --show --view tasks` / パレット `panel-tasks`）。一覧（既定 open・`updated_at` 降順・種類 / プロジェクト絞り込み・バッジは `open_count` をそのまま）+ 詳細（本文は `md_view::render_document` の共有描画・`exists` を見て「消えた添付」・`copy_texts` は 1 件ずつ・リンク）+ 返答フォーム（4 択 + コメント・判断未選択では送れない）+ スレッド + 配送状態。**B1 の API 以外は叩かない**
- **ポーリングは新タイマー無し**: 既存 2 秒ループが `tick_user_tasks` を呼ぶだけで、撃つ判断（`panel_visible` + A/B）はその 1 か所 = 止める処理を書かずに止まる。`list` を background へ逃がさないのは B1 が配送の決着を host から畳み込むため。3 本目の手書き入力を増やさないよう純ロジックを `text_field::TextField` へ切り出した（既存 git 2 本の移行は #1459）
- 実測: 隔離セルフテスト**項目 150**（view / 起票 / バッジ / 消えた添付 / 長い md / コピー 1 件 / 返答 + `failed` + 理由 / スレッド 12 件 / **閉じている間は撃たない** / 完了が `tako todo list` と一致）+ `TAKO_VISUAL_ONLY=tasks-panel`（**Metal の scene を読み戻すので画面収録権限が不要**・3 場面の指紋が全部別）+ 隔離 GUI で `sent → delivered` を実測（疑似 master へ実送達）。A/B `TAKO_1450B2_LEGACY=1` は項目 150 が「起票が一覧に載らない」で FAILED。番犬 5 本・注入 8 通り。workspace 4845 passed 0 failed・clippy 3 宇宙 0・check-windows error 0・docs build 32 ページ。**install 要**

## 2026-09-14（#1450 B3: 人がやることをスマホから片付けられるようにした）
- PWA `#/tasks`（一覧 + 詳細 + 添付の「端末に保存」+ copy_texts のワンタップコピー + 共有シート + 返答 / 完了 / 却下・ナビにバッジ）。daemon の受け口は `remote_tasks::TASK_ROUTES` の 4 本だけで、中身は B1 の `Request::UserTask` を**素通し**（一覧 Observe / 操作 Interact・**表に無い `/api/tasks…` は Manage の床**）。語彙と状態表示は B2 と同一（`sent` を「届いた」と書かない）
- **添付の配信経路は 1 本も足していない**: daemon が絶対パスを `shortcut_target` で `{root, path_rel}` へ解決し、PWA は既存の `/api/files/download` を叩く = 認可は `resolve_in_root` の 1 実装のまま（manage は `fs`・interact はツリー配下だけ・落とせない添付は理由 + #1452 の導線）。ポーリングは `usePolling` の 1 実装（cleanup + visibility。画面とバッジで共有）
- **実測で実バグ 2 件**: ①`value["tasks"]` の読みが serde_json の IndexMut で `null` を書き込み、単体応答に一覧のキーが生えていた ②ルートは canonicalize 済みなのに添付は素の綴りなので `/tmp/…` が解決できなかった（`resolve_target` で実体の綴りを引き直す）。実経路 `scripts/test-remote-tasks-1450b3.sh` **68 PASS 0 FAIL**（300 MB の添付を実転送・chunked を実測）・e2e 93 passed（新 11 本）・番犬 10 本（注入 8 通りで file:line 名指し）・workspace 4864 passed 0 failed・clippy 3 宇宙 0・check-windows error 0。**install 不要 / 本番 daemon 再起動要**

## 2026-09-14（#1459: 右パネルの手書きテキスト入力 2 本を TextField へ寄せた）
- git のコミット欄とブランチ名欄が持っていた同型の編集実装（`floor_char_boundary` で丸めてから backspace / delete / 左右 / Home / End / 挿入）を `TextField` の 1 実装へ。状態も `String` + カーソルから `TextField` 1 つへ畳んだ（`GitBranchInput.start_point` は不変）。割り当て（`⌘Enter` / `Esc` / `⌘V` / 1 行欄の上下→端）は各画面に残す
- **丸めは `text_field.rs` の非公開関数へ移した**ので、他ファイルが同じことをするには自前で書き直すしかない（= 番犬のマークに必ず掛かる）。B2 番犬の猶予表は空になり、走査は `tasks_panel.rs` + `right_panel.rs` の全面適用 + 「打鍵ハンドラが `handle_edit_key` を通す」の正検査つき
- 実測: 隔離セルフテスト（tako-vd）`TAKO_APP_SELF_TEST_OK`。A/B は委譲を切る注入で項目 79 / 82 が名指し FAILED（項目 81 まで通過を確認）。番犬は注入 11 通り + 実注入で `right_panel.rs:4643/4647` を file:line 名指し。workspace 4848 passed 0 failed・clippy 3 宇宙 0・check-windows error 0。**リファクタなので install 不要**
