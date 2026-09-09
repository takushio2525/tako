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

## 2026-09-09（#1229: remote_files のテストの約 1/350 フレークを直した）
- 真因は時間でも共有状態でもなく**入力の偶然**。`ツリーに出ていないルートは拒否される` が「未知の id」に `fx.root_id().to_uppercase()` を使っていたが、id は 12 桁の小文字 16 進なので (10/16)^12 ≒ 1/280 で数字だけになり、その回は大文字化しても実在の id のまま素通りしていた（fixture のパスに pid が入る = 綴りが毎回変わる）
- 綴り違いは長さで必ず外れる形（1 文字短い / 長い）へ替え、大小文字の区別は英字入りの id を据える別テストへ分離。番犬 `root_id_case_watchdog.rs` が修正前ソースの `remote_files.rs:2240` を名指しで落とす
- A/B（`remote_files::tests::` 全体・同一条件）: 修正前 12/4000 FAILED（落ちた 12 件の id はすべて数字だけ）/ 修正後 0/5000（高負荷 load 15.9・並列度 1 と既定の両方）。全 3848 件緑

## 2026-09-09（#1223: 番号なし・選択肢 2 つの信頼ダイアログを検知して respond できるようにした）
- 番号なし経路の「兄弟 3 行以上」を、**兄弟 2 行のときだけ「並びの直後に確定キーの案内があるか」**で補強（`dialog::confirm_hint_below`）。codex の入力待ち画面（入力行 + 直下のステータス行）は案内が無いので従来どおり非検知
- 隔離 GUI 実測（Windows 実機 + macOS）: 下見が `kind=trust` / 2 件 / `highlighted=0`、`--choice trust` が `Down`→ラベル一致検証→`Enter` で `resolved=true`。相手側 TUI も受領を表示
- A/B `TAKO_1223_LEGACY=1` は両 OS で Issue と同じ「選択肢ダイアログが見つからない」を再現。案内の根拠を外す注入で新旧 2 テストが FAILED。全 3785 件緑

## 2026-09-09（#1022: #571 e2e が本番の事前信頼エントリを残さないようにした）
- `E2e571Guard::drop` で `remove_e2e_trust_entry(&dir/work)` を呼ぶ形へ（#612 / #577 と同じ後始末）。この e2e は worker を既定 config dir で走らせるので隔離では倒せない
- 番犬 `e2e_trust_cleanup_watchdog`（`impl Drop for E2e…Guard` を走査して後始末の欠落を名指し）。`#[ignore]` の本体は CI で走らないのでここだけが再発を止める
- A/B: 後始末を外すと `crates/tako-control/src/dispatch.rs: E2e571Guard` を名指しで FAILED

## 2026-09-09（#992: agent 別のセットアップガイドと縮退の明記を docs へ入れた）
- 系統別ガイド 5 ページ（`docs/.../agents/` = 選び方 / claude / codex / agy / ローカル LLM）を新設し、サイドバーに「エージェント CLI」群を追加。#357 の agy 利用制限の調査結果と、getting-started の `settings.json` ドリフト（正は `~/.claude.json` / `.mcp.json`）もここで回収
- `gen-agent-support-docs.mjs` に**系統ごとの縮退ダイジェスト**（理由でまとめた degraded / pending / unsupported）を足して `agent-support.md` を再生成。生成物なので手書きと違ってずれない
- OG が #982 以降落ちていた（`agent-support` が `SECTIONS` 未分類で `npm run og` が例外で止まる）ので分類を足して再生成。24 → 30 枚

## 2026-09-09（#1137: 多重化が無いプラットフォームで無言の SSH 成功が畳まれないのを直した）
- `pane` 経路 + 多重化なし（Windows）は `master_socket` が常に false で成功の出口が 1 つも無かった。規則 ⑥「**打った行を除いて**中身が出たら畳む」を追加（ゲートは `ConnectInputs::multiplexing` の値 = **macOS は 1 ビットも不変**）
- 併発の穴（起点の陳腐化）も `ssh_progress::effective_from` で修正。`connecting` は rebase しないので相手が画面を消すと `new_lines` が全部空になっていた
- A/B: 規則 ⑥ 無効化の注入で `無言の接続成功でも畳む` が FAILED / 打った行の除外を外すと `打った行だけでは畳まない` が FAILED。Windows 実機で単体 32 件緑・macOS 実 pane 経路は従来どおり `connecting`→`connected`

## 2026-09-09（#1238: 再起動後の復元で codex / agy も会話ごと戻るようにした）
- 会話 ID は**生きたプロセスが開いているもの**から採れる（実測: codex 0.153.0 = `thread-writer-locks/<id>.lock`・起動直後から / agy 1.1.27 = `brain/<id>`・最初のターンの後）。`layout.json` へ `agent_resume`（系統 + ID）を `claude_session_id` と対称に保存し、`restore_plan` の分岐で `codex resume <id>` / `agy --conversation <id>` を投入する
- 規則は `tako_core::agent_resume` へ 1 本化（保持 = #1076 の「確認してから外す」を一般化 / 書式 = `resume_spec` / 可否 = `restore_support`）。ID を引く実装は系統ごとのモジュール（#984 / #1033）へ委譲。**Windows は lsof が無く ID を採れない**ので、内訳の理由を `ID なし` と分けて `resume 非対応` に
- 隔離 GUI 実測: tmux サーバー kill → 起動で `Claude resume 1 / agy resume 1 / codex resume 1 / 新規シェル 0` と 3 系統の会話が画面に復帰。A/B `TAKO_1238_LEGACY=1` は同じ layout で `新規シェル 3（ID なし 3）`。番犬 7 本 + 単体

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

## 2026-09-09（#1081: GUI 章の撮り直しは画面ロックで着手できず・収録開始条件を 3 つに定義した）
- 10:51〜12:22 の 90 分（30 秒 × 180 回）待って **180 回すべて施錠のまま**。`screencapture` は終始 `could not create image from rect` で、収録には入っていない（素材・完成品は前回のまま・隔離 tako も起動なし）
- 収穫は判定条件: **HID idle 単独は誤発火する**（施錠中も伸びる。10:50 実測で idle 165 秒 > 120 秒でも撮れない）。「解錠 + tako-vd 使用可 + idle 120 秒以上 + `screencapture` の試し撮り」の 4 段を plan へ明文化
- 次: 解錠後に `record-explainer.sh guimode` → `pii-scan.sh` → `build-explainer.sh … v5.mp4`

## 2026-09-09（#1263: auto mode の環境学習ダイアログを select として検知できるようにした）
- 真因は**ダイアログの下に空の入力欄が描かれる**こと（実測: Issue の 6 行だけなら旧実装でも検知でき、入力欄を足すと `None`）。`dialog.rs` に経路 4 を足し、**入力欄はダイアログの外**とみなして上を見る。開放するのは番号つき経路だけ（番号なしまで広げると複数行の user 発話の継続行を選択肢と誤検知する）
- 生きたダイアログと**会話ログへ流れた残骸**の区別は「並びと入力欄のあいだがダイアログの一部だけでできているか」（#577 の fixture は `✽ Misting…` が挟まる = 非検知。この条件を入れる前は #577 のテストが実際に落ちた）。確定キーの案内が無い形は拾わない = 安全側
- 模擬 TUI（実 tmux・GUI 不要）で respond の実キー: ラベル `Don't show again` / 番号 `3` どちらも `keys=["3"]` で `resolved=true`。A/B `TAKO_1263_LEGACY=1` と修正前ソースで新テスト 6 本が FAILED。全 3928 件緑
