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

## 2026-09-13（#1449: スマホの「+」を 3 択にして、ターミナル / SSH も新規に立てられるようにした）
- 「+」を `LaunchSheet`（master / ターミナル / SSH）へ。**操作は 1 つも新設していない**（ターミナル = `POST /api/tabs` だけ / SSH = #1080 の `POST /api/ssh {target:"tab"}`）。role は 3 種とも **Manage 据え置き**（「interact 以上」案は既存 Interact 端末の権限が黙って広がるので不採用 = 脅威モデルへ明記）
- 経路の宣言を `remote_launch::LAUNCH_ROUTES`（role / 監査 / PWA の呼び口）へ集約し `required_role` が引く形に。番犬 6 本 + e2e 11 本（61 passed）+ 実経路 `scripts/test-remote-launch-1449.sh` 38 PASS（**HOME ごと隔離**して実 `~/.ssh/config` を読まない = #927）。注入 7 通りすべて名指し FAILED
- **既存テストの罠 2 つを直した**: #841 以降 `test-remote-master-launch.sh` が全 403 で落ちていた（`TAKO_REMOTE_TRUSTED_PEER_NAMES="curl"`）→ 34 PASS へ復旧 / tako ペインからの実行は `TAKO_SOCKET` 継承で **CLI が本番 GUI を触る**（実測でタブ + QR が本番へ出た）→ `unset` + 起動ガード。**install 不要 / daemon 再起動要**

## 2026-09-13（#1450 B1: 人がやることの正本と、返答を起票 master へ返す配送）
- `tako todo` / MCP `tako_todo`（add / list / show / update / done / dismiss / respond）を `Request::UserTask` の 1 経路へ。モデルと純粋操作は `tako_core::user_task`、永続は `<data_dir>/orchestrator/user-tasks.yaml`（#916 の番地 `SchemaId::UserTasks` + 共有分類 Local。**新規ファイルなので移行 Step は無し** = 指紋のみ更新）。起票は共有通知欄の**成功系** `notify_ui_info`（A/B `TAKO_1450_LEGACY=1`）、診断は起きた場所（dispatch）へ
- 返答の配送は**既存経路のみ**: 生きている master へ `Request::Send` / 居なければ `master_launch::plan` + `TabNew` + `queue_command_flow`（#640）+ `queue_prompt_flow`。**罠**: 作りたてのペインへ `Request::Send` を撃つと取り付けが次 tick なので落ちる（実測で launched が failed になった）
- 実測 `scripts/test-user-task-delivery.sh`（隔離 tako-app + claude スタブ・tako-vd）45 PASS 0 FAIL（`delivered` / `launched` / `failed` + 理由）・セルフテスト完走・workspace 4686 passed 0 failed。番犬 4 本・注入 7 通りで file:line 名指し。**事故**: 初版はスクリプトが `TAKO_SOCKET` を unset せず本番 GUI にペイン 2 枚を作った（即 close・関門を追加 = #1454 と同じ罠）。**install 要**

## 2026-09-14（#1446: SSH 追跡をプロセスの寿命を越えて残し、繋ぎ直さない理由を出す）
- 真因は判断ではなく**記憶**: `ssh_connect` はメモリだけで作る口も dispatch の 1 つだけ → GUI 再起動をまたいだペイン（32 秒無反応・診断 0 行・報告と同一画面）と手打ち `ssh` のペイン（24 秒無反応）が実測で再現。slave 説と版が古い説は否定（8 桁幅での誤測は 88 桁で取り直し）
- 追跡を `track_ssh_connect` の 1 実装へ寄せ、入口を 3 つに（dispatch / `layout.json` からの復元 = `PaneLayout.ssh`・器が生きたときだけ / #976 の検知からの引き取り）。復元・引き取りは見張りから始めて起点を取り直す。`gave_up` を手で繋ぎ直したら見張りを再開（エッジで見つけた穴）。撃たないときは通知欄 + persist.log、`ssh_connect.reconnect` を `list` / `read` へ
- 実測: 再起動後の切断 **1 秒**検知 → 復帰 **2 秒**（ゼロタッチ）/ 手打ちも `source=detected` で復帰 / slave 落ち 0 秒検知 4 秒復帰 / 上限後は撃たずペインも残る。A/B `TAKO_1446_LEGACY=1` は追跡ゼロ・診断も無言（報告の再現）。番犬 4 本・注入 8 通り。workspace 4730 passed 0 failed・clippy 3 宇宙 0・check-windows error 0。**install 要**

## 2026-09-14（#1451: スマホのファイル閲覧を Finder 風の全体閲覧へ広げ、ショートカットを足せるようにした）
- 認可は**案 (a)**（全体閲覧とショートカットは manage 以上・interact 以下は #1079 のまま）。**経路は 1 本も新設せず**、疑似ルート `fs` を一覧へ 1 件足すだけにしたので、プレビュー / 編集 / DL は `resolve_in_root` の 1 実装をそのまま通る。門は `local_roots_for` の 1 か所で、載らなければ `unknown_root` の 403。role は `FILE_ROUTES`（#1449 の作法）が正で、**表に無い `/api/files…` は安全側の Manage** へ落ちる
- ショートカットの正本は `tako_core::remote_shortcuts`（PWA / `tako remote shortcuts` / MCP の 3 口が同じ実装）。永続は `<data_dir>/remote/shortcuts.json`（番地 `SchemaId::RemoteShortcuts`・**新規なので移行 Step 無し = 指紋の追加のみ**）。既定はファイルに書かず毎回計算する
- 実測: 実経路 `scripts/test-remote-fs-1451.sh` **73 PASS 0 FAIL**（HOME ごと隔離した偽の木・実 `/` を一覧しない = #927）/ e2e 11 本（全 72 passed）/ 番犬 10 本・**注入 12 通りすべて file:line 名指しで FAILED**。**実バグ 2 件を実測で発見して直した**: ①既定と登録分で正規化基準が違い同じフォルダが 2 行に出る ②飛び先にツリーのルートを選ぶとフォルダを閉じた瞬間 403。一覧は「切ってから metadata」へ（旧は全件 x3 syscall）。**install 要 / 本番 daemon 再起動要**

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
