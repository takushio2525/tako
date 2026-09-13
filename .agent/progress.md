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
