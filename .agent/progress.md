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

## 2026-09-09（#1200: respond が生きているペインへ in-process 経路で届くようにした）
- 真因は「到達判定を通っていなかった」。器のセッション名を必須にし `reach::detached_session` へ直行 →
  入力送出を持たない psmux では必ず失敗・器なしのペインは対象外（セルフテストも skip されていた）
- 届き方を `reach::DialogAccess` へ出し `dialog_access` が in-process を先に見る形へ。手順は
  `respond_via` の 1 実装で共有し監査へ `route=`。実機 A/B で Issue と同一エラー → `DELIVERED=1`

## 2026-09-09（#1203: UI の macOS キー表記を正本経由にし、Windows で実際のキーを出すようにした）
- 6 か所（タブバー `⌘K` / git コミット欄 / プレビュー保存 / 確認ダイアログ / 設定 / `ui-mode` の next_step）と走査で見つかった同型 4 件を `tako_core::platform::keys` と `keybindings::shortcut_hint_for(action, platform)` へ。macOS の文言は 1 文字も不変
- **バインド表に無い「修飾 + クリック / Enter」は Windows で案内ごと落とす**（Win キーは押せない = #763）。`key_bindings()` の `cfg` をやめ `bindings_for(platform)` にしたので macOS の CI から Windows 側を検証できる
- 番犬 2 本立て（ソース走査 `ui_key_notation.rs` の規則 A / B + 組み上がった文言の Windows 検査）。A/B 5 アームが確定 FAILED

## 2026-09-09（#1204: マトリクスの tako_welcome / tako_ui_mode の過小申告を実挙動へ直した）
- `WIN_WELCOME_INJECTION` / `WIN_STARTER_INJECTION`（#899 修正前の宣言）を削除し、両方の Windows を `Supported` + `Evidence::Measured`（2026-09-09 の実機でボタンから setup / master が実際に走った）へ
- 番犬 `ボタン投入を根拠にした宣言が実装と一致する` が宣言と裏づけ（PowerShell 方言で POSIX クォートに囲まれない = #899 の本体）を縛る
- 実出力 `tako platform --platform windows` = 両方 `supported` / Known limitations から消えた / docs 再生成。A/B は Degraded へ戻すとテストと docs `--check` の両方が FAILED

## 2026-09-09（#1076: 再起動後の claude resume が「起動途中の未検出」で自壊していたのを直した）
- 真因は 2 つ: ①`claude agents --json` のスキャン結果でマップを丸ごと置き換え（+ 子プロセス不在で全消し）ていたので、**再起動直後の数秒**で `layout.json` の `claude_session_id` を全部捨てていた（実測: 復元直後 6 件 → 6 秒後 0 件）②復元だけが独自の最小形 `claude --resume <id>` を組んでいて役割 env / `--model` / `--effort` が落ちていた
- 保持規則を `tako_core::claude_resume::ResumeIds`（**確認してから外す**）へ、判断とコマンドを `tako_control::sessions::restore_plan`（`resume_command` と共有）へ集約。`persist.log` に「復元の内訳」（役割つき / 役割なし / 新規シェルの理由 3 分類）を 1 行追加
- 隔離 GUI 実測: 修正後は 76 秒後も 6 件のまま・実会話が resume されて継続（`⏺ repro-1076`）/ A/B `TAKO_1076_LEGACY=1` は同じ layout で 6 秒後 0 件 + `役割つき 0`。再 attach（8 ペイン）に回帰なし。番犬 4 本（修正前ソースで全滅）+ 単体 15 本。全 3766 件緑

## 2026-09-09（#986: codex / agy worker から tako の MCP を呼べるようにした）
- codex は spawn の起動コマンドへ `-c mcp_servers.tako.*` を一時注入（正本 `agent::codex_mcp_args` を master / worker / git resolve が共有）。`caller_pane` は `TAKO_PANE_ID` が無ければ **pid 祖先辿り**へ落ちる（`tako mcp serve` → `Request::ResolvePane`）
- 実測（隔離 GUI + tako-vd / codex-cli 0.153.0 / agy 1.1.27）: 実 worker が `tako_list_panes` を呼び、**pane 省略**の `tako_set_title` が codex=pane2 / agy=pane3 と自分のペインへ当たった。効いている経路は env の指紋で判別（一時注入 5 個 / 恒久登録 4 個）。同時 2 本でも取り違えなし
- Issue の前提「agy は親 env を渡さない」は**実測で否定**（`TAKO_*` が 15 個届く）ので docs / コメントを訂正。番犬 3 本 + 単体 9 本、A/B は `TAKO_986_LEGACY=1`。全 3775 件緑

## 2026-09-09（#1202: Markdown プレビューが UTF-8 BOM を剥がさず先頭行の見出しを潰す問題を直した）
- `tako_core::text::strip_bom` を新設し、`runner.rs:131` の既存処理と Markdown パーサ入口（`parse_markdown_blocks`）が 1 実装を共有。プレビュー / チャット / 更新ノート / md_view はここが唯一の入口
- **剥がすのはパーサへ渡す本文だけ**（編集・保存の生テキストには触らないので BOM 付きファイルは保存しても BOM を保つ）
- 隔離 GUI 実測: BOM 付き `README.md` の `preview-outline` が BOM 無しと同一（`sample project` level=1 block=0）。A/B 3 アーム（パース / 描画 / 先頭 1 個だけ）が確定 FAILED。全 3769 件緑

## 2026-09-09（#1033: agy に一次シグナルを与え、完了検知を claude 同等にした）
- 前提の訂正: 「agy の会話は SQLite だけ」は実態とズレ。`brain/<id>/.system_generated/logs/transcript.jsonl` が逐次追記される平文 JSONL（**依存追加なし**）。ペイン → 会話は生きた agy が開いたままの `brain/<id>` を lsof で引く
- `agy_session` を新設し `status_source=agy-session` / `report` の transcript 層 / 送達の一次証拠（`USER_INPUT`）へ配線。補正層の権威判定は `is_live_log_source` の 1 実装へ寄せた（**ここを忘れると `has_children` で永久 busy** = #571 / #984 と同じ罠を実機で踏んだ）
- 隔離 GUI での A/B 実測（北極星と同じ測り方・各 3 標本）: 検知遅延の中央値 39.46s → **13.92s**（claude 15.40 / codex 11.68 と同水準）。`report` は scrollback/messages 0 → transcript/`transcript_agent=agy`/messages 1〜2。偽 idle は増えず watch の偽イベント 0 件

## 2026-09-09（#989: ゼロスタート導入を claude 専用から 3 系統へ広げた）
- `agent_install::AgentKind` を 3 値・`recipe(platform, agent)` を 6 マスへ（codex / agy の公式手順は実物で確認。codex は代行時 `CODEX_NON_INTERACTIVE=1` が必須 = 無いと `Start Codex now?` で返らない・agy は単一バイナリ）。`setup_bootstrap` の全操作を `_for(agent)` へ寄せ、**引数なしの claude 既定入口は残していない**
- setup は「1 つでも使える系統があれば素通り / 無いときだけ途中まで入っているものを優先して仕上げる」形へ（単一選択を強制しない）。認証誘導・失敗案内・CLI 解決のフォールバックも系統ごと。Windows で代行するのは claude だけ（宣言 2 か所）
- まっさら HOME + PATH 剥ぎで 3 系統の実インストール通し + 冪等（2 回目は `unchanged`）。A/B `TAKO_989_LEGACY=1` は「codex だけの HOME で claude を勧める」を再現。番犬 4 本（修正前ソースで claude 既定入口 11 個を名指しして FAILED）+ セルフテスト項目 119 拡張

## 2026-09-09（#944: cargo test が本番の data dir と HOME 配下の設定へ書かないようにした）
- 置き場を決める側を倒した: `paths::data_dir()` が**実行時に**テストバイナリを見分けて隔離先へ（`cfg(test)` はクレートを跨がない）＋ `orchestrator::agent_config_home()` の `cfg(test)` 隔離＋単体テストからは `claude agents --json` を起こさない
- 「メインスレッド専有」は `mark_main_thread()` 済みのプロセスだけが名乗る。`setup` の移設と #577 e2e の後始末も隔離に合わせた（実ユーザーの setup / 残骸掃除が壊れる穴を先に塞いだ）
- 番犬 `test_write_isolation`（空 HOME で子を起こし 0 ファイル）+ A/B `TAKO_944_LEGACY=1`。実測 36 → 0 ファイル。副産物で `ensure_trusted` が置き場ごと無い環境で黙って失敗する穴も直した

## 2026-09-09（#1030: テスト由来の事前信頼エントリの掃除口を用意した）
- 書き先は #944 で塞ぎ済み。残骸掃除に `scripts/clean-trust-residue.sh`（dry-run 既定・`--apply` で退避つき削除）と偽 HOME のモックテスト 10 件（CI 登録）を追加
- 番犬へ `claudeの設定エントリはテスト実行で増えない`（種を置いた HOME で子を回し件数不変を実測。A/B `TAKO_944_LEGACY=1` で増える）
- dry-run 実測: `~/.claude.json` 2,573 件中 2,216 件 / `~/.claude/.claude.json` 874 件中 736 件が対象。**うち約 2,145 件は GUI セルフテスト由来**で射程外（Issue へ報告済み）

## 2026-09-09（#1229: remote_files のテストの約 1/350 フレークを直した）
- 真因は時間でも共有状態でもなく**入力の偶然**。`ツリーに出ていないルートは拒否される` が「未知の id」に `fx.root_id().to_uppercase()` を使っていたが、id は 12 桁の小文字 16 進なので (10/16)^12 ≒ 1/280 で数字だけになり、その回は大文字化しても実在の id のまま素通りしていた（fixture のパスに pid が入る = 綴りが毎回変わる）
- 綴り違いは長さで必ず外れる形（1 文字短い / 長い）へ替え、大小文字の区別は英字入りの id を据える別テストへ分離。番犬 `root_id_case_watchdog.rs` が修正前ソースの `remote_files.rs:2240` を名指しで落とす
- A/B（`remote_files::tests::` 全体・同一条件）: 修正前 12/4000 FAILED（落ちた 12 件の id はすべて数字だけ）/ 修正後 0/5000（高負荷 load 15.9・並列度 1 と既定の両方）。全 3848 件緑

## 2026-09-09（#1238: 再起動後の復元で codex / agy も会話ごと戻るようにした）
- 会話 ID は**生きたプロセスが開いているもの**から採れる（実測: codex 0.153.0 = `thread-writer-locks/<id>.lock`・起動直後から / agy 1.1.27 = `brain/<id>`・最初のターンの後）。`layout.json` へ `agent_resume`（系統 + ID）を `claude_session_id` と対称に保存し、`restore_plan` の分岐で `codex resume <id>` / `agy --conversation <id>` を投入する
- 規則は `tako_core::agent_resume` へ 1 本化（保持 = #1076 の「確認してから外す」を一般化 / 書式 = `resume_spec` / 可否 = `restore_support`）。ID を引く実装は系統ごとのモジュール（#984 / #1033）へ委譲。**Windows は lsof が無く ID を採れない**ので、内訳の理由を `ID なし` と分けて `resume 非対応` に
- 隔離 GUI 実測: tmux サーバー kill → 起動で `Claude resume 1 / agy resume 1 / codex resume 1 / 新規シェル 0` と 3 系統の会話が画面に復帰。A/B `TAKO_1238_LEGACY=1` は同じ layout で `新規シェル 3（ID なし 3）`。番犬 7 本 + 単体
