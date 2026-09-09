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

## 2026-09-09（#1246: .agent の md のコンフリクトマーカーを CI で落とす番犬）
- 走査を `.agent/` 配下の md 全部 + 規約 2 本（4 → 38 ファイル）へ広げ、#1241 / #1247 の狭い版を置換。旧版は `&&` が `||` より強く判定の前半が死んでいて `=======` を一度も拾えず、8 文字以上の罫線を誤検知していた
- `=======` は `<<<<<<<` が開いた領域の中だけで見て setext 見出しと弁別（git は必ずこの順で書くので検出力は落ちない）。実物へ 3 行挿入 → 行番号つきで名指し FAILED → 復元で緑
- `.gitattributes` に `.agent/progress*.md merge=union`。実測で merge / diff3 / rebase とも衝突ゼロ。移送 + 追記も安全（領域が重ならない）。両採用になるのは双方が同じ領域を書き換えたときだけで、限界は conventions へ表で明記

## 2026-09-09（#1034: 送達後に実行を断られた worker を完了と誤読しないようにした）
- `execution_refused`（`WorkerErrorKind` / `AgentCliProblem` の新分類・`recommended_action = retry_spawn`）を追加。**#983 のゲートは緩めず**、別ゲート「一次シグナルで作業を 1 歩も観測していない」で分類する（画面推定の busy は TUI の起動描画を拾うので使えない）
- 判定の文言は系統ごとに宣言（`execution_refused_patterns`）。実採取は agy のみで**版で文言が変わる**（1.1.22 = `Verifying your account` / 1.1.27 = `Unable to verify account eligibility`）ので共通語 `account eligibility` を軸にした。claude 2.1.258 / codex 0.153.0 のバイナリに一時的な検証待ちの文言が無いことは実物の走査で確認
- MATRIX に `worker_refusal_detect` を新設し docs を同期。番犬 4 本 + dispatch の e2e 1 本 + 単体 8 本。A/B は `TAKO_1034_LEGACY=1`

## 2026-09-09（#1236: 信頼ダイアログの自動承諾がハイライトを見ずに Enter を送る問題を直した）
- 送達フロー（`main.rs` の `drive_trust_accept`）と器越しの送達（`deliver_via_tmux`）を `claude_tui::accept_step` の 1 実装へ寄せ、移動の向き・歩数・「Enter を送ってよいか」は `tako_core::dialog::confirm_step`（respond の番号なし経路と共有）へ。**承諾側を特定できないあいだは Enter を送らない**（理由は `persist.log` の `[auto-accept]` 行）
- 模擬 TUI（実 tmux・GUI 不要）で実キーの A/B: 新 = `Down`→`Enter` で `Yes, I trust this folder` が確定 / `TAKO_1236_LEGACY=1` = `Enter` 1 発で **`No, exit` が確定**（= 事故の再現）。ベースラインの Windows 警告 10 件と一致
- 番犬 4 本（修正前ソースを 4 本すべてが名指しで FAILED）+ 単体 8 本 + `confirm_step` 4 本。全 3905 件緑

## 2026-09-09（#1248: CLI の --help が実装とずれていた 2 か所を正本から引く形に直した）
- `setup-mcp` の help（書き先は `~/.claude.json` / `<cwd>/.mcp.json`・claude 以外にも登録）と `--agent-effort` の「agy は無視」（#1002 で否定済み）を訂正。同じずれが残っていた protocol / MCP catalog / `orchestrator/agent.rs` / `mod.rs` / `.agent/orchestrator.md` も同時に直した
- 再発防止は**文どうしを比べない**形: 書き先は `dispatch::mcp_target_path`（`MCP_TARGET_FILE_*` へ切り出し）、能力は `agent_support` の `effort_control` と突き合わせる。番犬 3 本（CLI の実 `--help` 2 本 + 全 rs / md 走査 1 本）
- A/B: 旧文言へ戻すと 3 本とも FAILED（`main.rs:2023` / `agent.rs:165` を行番号で名指し）。全 3892 件緑

## 2026-09-09（#1258: ssh 自動追加の見送りログを同一プロセスで 1 回だけにした）
- 真因は「判定のたびに書く」こと。`apply_ssh_scan` は 2 秒 tick のたびに呼ばれ、走査を間引いた tick でも `scan` が `prev.skipped` を持ち越す（「見ていない」を「消えた」と読み替えない仕様）ので同じ行が積もっていた（実測 3 時間で 866 行）
- 鍵を **(ペイン, ssh の pid, 理由)** にした `ssh_detect::SshSkipLog` を通してから書く形へ（`SkippedSsh` に `pid` を追加・`remote-folder auto` の `skipped` にも `pid` を出す）。記憶は現に見送られている鍵だけ残すので打ち直しループでも伸びない
- A/B `TAKO_1258_LEGACY=1`: 60 回評価で既定 1 行 / legacy 60 行。番犬 3 本が修正前ソース（`ssh_folders.rs:208`）と pid 落ちを名指しで FAILED。全 3929 件緑

## 2026-09-09（#1081: GUI 章の撮り直しは画面ロックで着手できず・収録開始条件を 3 つに定義した）
- 10:51〜12:22 の 90 分（30 秒 × 180 回）待って **180 回すべて施錠のまま**。`screencapture` は終始 `could not create image from rect` で、収録には入っていない（素材・完成品は前回のまま・隔離 tako も起動なし）
- 収穫は判定条件: **HID idle 単独は誤発火する**（施錠中も伸びる。10:50 実測で idle 165 秒 > 120 秒でも撮れない）。「解錠 + tako-vd 使用可 + idle 120 秒以上 + `screencapture` の試し撮り」の 4 段を plan へ明文化
- 次: 解錠後に `record-explainer.sh guimode` → `pii-scan.sh` → `build-explainer.sh … v5.mp4`

## 2026-09-09（#1263: auto mode の環境学習ダイアログを select として検知できるようにした）
- 真因は**ダイアログの下に空の入力欄が描かれる**こと（実測: Issue の 6 行だけなら旧実装でも検知でき、入力欄を足すと `None`）。`dialog.rs` に経路 4 を足し、**入力欄はダイアログの外**とみなして上を見る。開放するのは番号つき経路だけ（番号なしまで広げると複数行の user 発話の継続行を選択肢と誤検知する）
- 生きたダイアログと**会話ログへ流れた残骸**の区別は「並びと入力欄のあいだがダイアログの一部だけでできているか」（#577 の fixture は `✽ Misting…` が挟まる = 非検知。この条件を入れる前は #577 のテストが実際に落ちた）。確定キーの案内が無い形は拾わない = 安全側
- 模擬 TUI（実 tmux・GUI 不要）で respond の実キー: ラベル `Don't show again` / 番号 `3` どちらも `keys=["3"]` で `resolved=true`。A/B `TAKO_1263_LEGACY=1` と修正前ソースで新テスト 6 本が FAILED。全 3928 件緑

## 2026-09-09（#1252: マウスレポート e2e のフレークを止めた・真因は偽アンカー）
- 真因は待ち不足ではなく**アンカーが偽**。conf の `set -g mouse on` で tmux は attach 時に外側端末のマウスを有効にするので、外側 `mouse_reporting()` は内側アプリと無関係に真（実測 20 ms・器側 `any=0`）。要求前のホイールは **tmux が食って copy-mode へ入り**（`pane_in_mode=1`）、以後は待ちを伸ばしても永久に届かない
- アンカーを器のペインの `#{mouse_sgr_flag}` へ（`wait_pane_mouse_ready`）・待ちは `wait_for_state`（状態待ち + `state_wait_budget`・診断は「待っていたもの / 届いたもの」）。予算政策は `tako_core::wait_budget` の 1 実装へ寄せ tako-app が委譲。番犬 `mouse_report_wait_watchdog`
- A/B（同一ビルド・交互・load 12〜18）: 旧待ち **7/75 FAILED** → 状態待ち **0/315**。`LEGACY=1 INJECT=late` は確定 FAILED（`inmode=true`）/ `INJECT=never` は新経路でも FAILED。副産物の osc7 系フレークは #1265 へ分離

## 2026-09-09（#1114: psmux の永続化 e2e の間欠失敗を待ち不足として直した）
- 先にフェーズ別の余裕を実測: プロンプト待ちの固定 6 秒だけが**スイート並列（18 本）だけで既に超過**（6.1〜7.1s・`rounds=2` が 10/10）。4 多重 + 負荷 12 で 20〜34s になり 6 周を使い切って **9/16 FAILED**（全件 phase 1・画面は `PS …> Write-O` = 打鍵は届いていた）
- 固定窓を全廃して**期限 + `wait_budget::state_wait_budget`** へ。`machine_busy()` を Windows でも読めるようにし（`GetSystemTimes`）、`sleep(800ms)`→`exists` は「接続クライアントが 1 でなくなった」状態待ちへ。番犬 `psmux_e2e_wait_watchdog` が修正前の 9 か所を名指しで落とす
- A/B（修正前後の 2 バイナリを同一ラウンドで交互）: before **6/24** → after **0/24**。追加で after 0/100。副産物で **#1264**（main の Windows `cargo test --workspace` がコンパイル不能）を起票

## 2026-09-09（#1259: send_input が queued:true を返して無音で届かない問題を直した）
- 真因は**顛末の記録が後続 send では全経路空振り**すること。`record_prompt_delivery_at` は先頭で `if flow != SpawnPrompt { return }`（spawn 専用）・保留と打ち切りは `eprintln!`（GUI の stderr は誰も読めない）・`persist.log` へ書くのは貼り付け段へ到達した後の `try_peer` / `log_fallback` だけ。本番の pane 1627 は `送達:` 行が 1 本も無く（同時刻の他ペインは記録あり）、`WaitPromptReady` で `input_line` が唯一の入口という穴と、peer の背景試行・起動コマンド待ちの**上限なしの待ち**が重なっていた（Issue の見立て「busy だと送らない」は否定 = 送達フローは `has_running_children` を参照しない）
- 語彙と方針を `tako_core::prompt_delivery` の 1 本へ（`Stall` 11 種 + `Journal` の心拍 15 秒 + `peer_wait` の段階判定）。応答は `tako_send_input` / `tako_read_pane` の `delivery`、CLI は `[delivery]` 行、`persist.log` は `送達フロー: pane=… 理由=…`。peer は 25 秒で諦めるが**送る前の段階だけ**キー経路へ落ちる（判定は `PeerAttemptState` の CAS。段階を読むだけでは読んだ直後に背景スレッドが書き始めて二重投函になる = #790 の不変条件）
- A/B `TAKO_1259_LEGACY=1` で無音と無限待ちを再現（実ファイル検証）。番犬 6 本のうち 5 本が修正前 `main.rs` を行番号で名指し（6440 / 6445 / 6457 / 6472 / 6738 / 6797）。模擬 TUI e2e（`sleep 3600` の子を抱えた idle ペイン）で送達成立を実測
