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

## 2026-09-09（#1192: 隔離・テストの tmux サーバーを所有者の生死で安全に回収できるようにした）
- `tako tmux cleanup --servers`（既定 dry-run・実削除は `--apply`）を追加。判定は名前ではなく**所有プロセスの生死**（自分 / 既定 / 生きた tako-app の env 復元 / attach 中 / 所有者不明 / 出来たて を除いた残りだけ回収可）。溜めない側は使い捨て backend（`tako-iso-<自分の pid>`）の終了時 self-kill
- 実測: 本番置き場へ dry-run = 総数 1838 / 生存 126 / 回収可 116 / 残骸 1708 / 保護 14（何も消していない）。隔離した置き場のダミーで `--apply` = 生きた所有者は `owner_alive` で無傷・死んだ所有者だけ kill + ソケット削除
- 番犬 2 本（修正前ソースで FAILED）+ 実物テスト 3 本 + 純粋 4 本。所有者の生死ガードを外す注入で必須テストが FAILED

## 2026-09-09（#1182: ターミナルのパスリンクに cmd+右クリックメニューを付けた）
- 並びは `tako_core::path_menu`（純粋関数）・文言はツリー（#314）の `sidebar::menu_*` を委譲で共有。新規操作は「tako で開く」= `FileOpKind::OpenInTako` の 1 つだけで **cmd+クリック自身もそこを通す**（CLI `tako file open-in-tako` / MCP `op=open_in_tako`）
- **前提として `links::combined_screen_text` の soft wrap 判定を直した**: 実画面の行は空白詰めなので旧判定では全行が折り返し扱いになり、隣接 2 行に何か書かれているだけでパスリンクが 1 つも検出されなかった（A/B: 旧式で新テストが FAILED）
- 項目 147（合成マウスの実配送 + 実矩形 + 前提の ui-mode 倒し・unix 限定）/ CLI・MCP e2e を隔離 GUI で実測 / 実フレーム PNG 2 枚 / `TAKO_1182_LEGACY=1` で 147 が確定 FAILED。全 3711 件緑

## 2026-09-09（#1191: `tako list` の `backend_windows` を右パネルの表示状態から切り離した）
- 採取が fleet ビューの 2 秒ポーリングだけだったのを `Request::List` の直前 1 回（`list-windows -a` = 6.9ms 中央値。`fetch_tmux_sessions` は 37ms）へ。backend ペインが無ければ tmux を起動せず、500ms 以内は使い回すので連打でも 2 回/秒（実測 889 req/30s → 56 回）
- `null`（backend でない / 採取不能）と `[]`（backend だが window 無し）を読み分け可能に。1 window の器も載る（旧実装は 2+ のみ）。待機時の追加コストは 0 回 / CPU 差は誤差
- A/B `TAKO_1191_LEGACY=1` は Issue の 2 症状（常に null / 開くと埋まり閉じると陳腐化）を再現。番犬 3 本 + dispatch 1 本 + tako-core 2 本 + セルフテスト項目 61g

## 2026-09-09（#1220: 一覧付与のコスト検査を実時間から量へ替え、番犬を tests 全体へ広げた）
- `warm <= cold`（実時間比較）を廃し、`claude_remote_link::scan_counters`（走査回数 / 読み出しバイト数 / 所在探索回数・**スレッドローカル**）で測る形へ。生きている会話への追記ぶんは予算へ足す
- 番犬 `test_timing_watchdog`（`crates/*/tests` の assert 条件部に実時間の値が 2 つ以上ある形だけを落とす。コメント / 文字列は潰して見る）を新設。修正前ファイルで `remote_link_live.rs:247: warm <= cold` を名指しして FAILED
- A/B（`yes` 36 / 72 本の負荷・交互 40 回）: 旧 2/40 FAILED（初回 34.3ms / 2 回目 45.7ms で反転）・新 0/40。memo 無効化の注入で新は確定 FAILED（20 走査 10 MB）・旧は 9/10 見逃し

## 2026-09-09（#1206: README に Windows のインストール手順を足した）
- インストール節を macOS 2 節 + Windows 1 節へ（`.exe` = 標準・管理者権限不要 / `.zip` = ポータブル / SmartScreen / psmux）。冒頭を「macOS 11.0 以降 + Windows 10 1809 以降 x64」へ直し、永続化の説明に psmux を併記
- 数値・アセット名は `release_assets` と直近リリースの実アセットから引き、docs の「方法 C」と同文・同数値。マトリクス note（Issue の 3 点目）は生成物なので #1204 の担当（CI の `gen-windows-support-docs.mjs --check` が同期を強制している）

## 2026-09-09（#1199: リサイズでペインの cwd が起動時ディレクトリへ巻き戻るのを直した）
- 真因は「側路（#766）の待ち合わせがパスだけ」。psmux は `-e` を**サーバーのグローバル環境**へ入れ、
  プリウォーム済みシェルの一団（cwd = ホーム）もそれを継承するので、リサイズでプールのプロンプトが
  描き直るたびに `OSC 7 <ホーム>` が同じファイルへ落ちていた。関連コミット: `23db0c4`
- 待ち合わせ先へ書き手の pid を載せ（`<pane>@p<pid>.osc`）、tako は器へ聞いた `#{pane_pid}` ぶんだけ
  読む形へ。実機 A/B で 8/8 → 0/8

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

## 2026-09-09（#1223: 番号なし・選択肢 2 つの信頼ダイアログを検知して respond できるようにした）
- 番号なし経路の「兄弟 3 行以上」を、**兄弟 2 行のときだけ「並びの直後に確定キーの案内があるか」**で補強（`dialog::confirm_hint_below`）。codex の入力待ち画面（入力行 + 直下のステータス行）は案内が無いので従来どおり非検知
- 隔離 GUI 実測（実ペイン 79 桁・本文が折り返す状態）: 下見が `kind=trust` / 2 件 / `highlighted=0`、`--choice trust` が `Down`→ラベル一致検証→`Enter` で `resolved=true`。相手側 TUI も受領を表示
- A/B `TAKO_1223_LEGACY=1` は Issue と同じ「選択肢ダイアログが見つからない」を再現。案内の根拠を外す注入で新旧 2 テストが FAILED。全 3785 件緑
