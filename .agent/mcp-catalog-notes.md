# MCP カタログから外した説明の置き場（Issue #1711）

`tools/list` の応答（MCP ツールカタログ）は、MCP を繋いだエージェント全員が起動した瞬間に全文を
受け取る固定費で、予算は 200 KB（`tako_core::context_budget::MCP_CATALOG_MAX_BYTES`）。
#1711 の時点で 202,956 バイト（154 本）まで詰まり、LSP 統合（#1007）で口が増える前に
説明文だけを短くした（ツールの増減・引数・型・必須項目・挙動は変えていない）。

書き方の規約は `.agent/conventions.md`「MCP カタログの説明文は AI が使える情報だけ載せる
（Issue #1540）」。ここには、その規約に沿って**説明文から外したが情報としては捨てない**ものを
原文で残す。AI が行動を決めるのに要る情報（いつ使う / 何が返る / 前提ツール / 失敗時 /
誤用すると壊れる注意）は説明文に残してある。

- **説明文を直すときの参照先**。ここにある文を説明文へ戻したくなったら、まず
  「その文が無いと AI が誤った呼び方をするか」を考える（戻すと固定費が増える）
- **読むのは開発者と、tako のリポジトリで作業する AI だけ**。実行時の AI が引ける手順は
  `tako orchestrator guide <topic>`（MCP `tako_orchestrator_guide`）に置き、説明文からは
  topic 名で案内する

## 外し方の型（全ツール共通）

どれも**情報は同じツールの説明か schema、または下記の正本に残っている**。

| 型 | 例 | 残っている場所 |
|---|---|---|
| 引数ごとに繰り返していた共通の但し書き | `tako_orchestrator_profiles` の `clear_*` 13 本の「X の指定を解除して既定へ戻す」/ 同ツールと `tako_orchestrator_layout` の「省略で現状維持」/ `tako_sleep_guard` の「（set 時のみ有効）」 | 各ツールの description に 1 回だけ |
| schema が既に持つ値の再掲 | `ctx_threshold` の「50〜60」（`minimum` / `maximum`）/ `tako_orchestrator_run` の「既定 1800 秒 = 30 分」「省略時 true」「省略時 200」（`default`） | inputSchema の `minimum` / `maximum` / `default` |
| description と引数説明の二重掲載 | `tako_orchestrator_spawn` の「agent パラメータで claude / codex / agy から選べる」/ `tako_file_op` の「open_terminal / copy_relative_path / open_in_tako は pane でペインを指定する」/ `tako_open_remote` の「開き先は target で選ぶ」 | 片方（主に引数説明）だけ |
| 他ツールの説明の再掲 | `tako_orchestrator_spawn` の「codex / agy は画面推定で判定される」 | `tako_orchestrator_worker_status` の status_source |
| 手順書の再掲 | 引き継ぎファイルの 2 節構成の理由、リモートの手順 | guide の `handoff` / `remote` / `user-tasks`（説明文から topic 名で案内） |
| 要件番号 | `（FR-2.22）`（tako_show_command）/ `（FR-3.8）`（tako_web）/ `FR-3.18`（tako_run） | `.agent/requirements.md`。AI は番号から要件を引けない（#1540 の Issue 番号と同じ扱い） |
| 歴史的経緯 | 「別タブへ逃がす配置は廃止した」「旧挙動」 | git 履歴と `.agent/requirements.md` FR-2.20.7 |

## ツール別: 外した記述の原文

同じ情報がほかにあるものは「→」の後に在り処を書いた。無印はここが正本。

### tako_orchestrator_profiles

- 「プロファイルは profiles/<name>.yaml に保存され、master のエージェント種別・モデル・effort と子 worker のモデル決定に使われる。」（保存先は説明文に残した）
- 「model が null / 未指定のプロファイルはその CLI の既定モデルで起動する（プラン非依存・推奨）。」のうち「プラン非依存」→ `.agent/orchestrator.md`（プラン非依存）
- 「自動ハンドオフは ctx_threshold（50〜60%）と auto_handoff で調整する。」（引数説明と schema に残る）
- `master_agent`: 「tako master / solo がこの CLI で起動する」
- `agent_skip_permissions`: 「agy は既定でコマンド毎に許可が出るため自律 worker 運用ではほぼ必須」の理由部分（「agy の自律 worker ではほぼ必須」は残した）
- `bypass_sandbox`: 「false へ戻すと codex の既定（承認プロンプトが出る）に戻る」
- `cwd`: 「相対パスは起動時に解決できないので拒否する」の理由部分 → `.agent/commands.md`
- `remote_control`:
  - 「true にすると claude 起動コマンドへ --remote-control が付き、claude.ai と Claude モバイルアプリからその会話を操作できるようになる。」（要点は残した）
  - 「認証は claude.ai アカウントへ移るので tako の機器ペアリングと role（observe / interact / manage / admin）はその会話には効かない。」のうち「認証は claude.ai アカウントへ移る」と role の列挙
  - 「claude 以外の系統（codex / agy）には相当する仕組みが無いため何も起きない。」の理由部分
  - 「環境が不適格（DISABLE_TELEMETRY 等・エンドポイント差し替え・API キー認証・組織ポリシー）なときはフラグを付けず、応答の remote_control_blocked に理由と次の一手が入る」の不適格条件の列挙 → `.agent/requirements.md`（DISABLE_TELEMETRY）/ `.agent/commands.md`

### tako_orchestrator_spawn

- 「thinking / reasoning effort（3 系統とも指定できる。」の「3 系統とも指定できる」（系統ごとの渡し方は残した）
- `task_type`: 「spawn 時に自動記録され」
- `pane`: 「（省略時は呼び出し元。」（description の「pane か tab を必ず指定する」側に寄せた）

### tako_orchestrator_worker_status

- 「prompt_delivery=undelivered は spawn プロンプト未達（起動 ≠ 到達）」の「（起動 ≠ 到達）」
- 「倒さなかった理由は idle_override_blocked = input_draft_unreadable 等」の値の例 → `.agent/conventions.md`（input_draft_unreadable）

### tako_orchestrator_workers

- 「列挙のついでに」「（resume_command / report は closed でも引けるので 突然死からの復旧材料は失われない）」の後半の理由

### tako_orchestrator_self

- 「値域 50〜60。」→ `tako_orchestrator_profiles` の `ctx_threshold` の schema
- 「呼び出し元の名乗りは使わないので」の理由部分
- 「存在しないペインはエラー（たまたま既定 role で動いている無関係な master へは落とさない）。」の理由部分

### tako_orchestrator_handoff

- 「どれも決まらなければ本文を貼らずに一覧とパスだけを後任へ渡す（無関係なプロジェクトの長文で後任の文脈を食わない）。」の理由部分
- 「旧形式（プロファイル単位の混在ファイル）」の旧形式の中身
- 「旧 master のペインは後任が引き継ぎを確認したあとに後任自身が閉じる（初期プロンプトにその手順が入る: 実態突き合わせ → 旧ペインの入力欄にユーザーの未送達指示が残っていないか確認 → close）。この呼び出しでは閉じないので、後任の起動が失敗しても旧 master は失われない。」の手順と理由 → guide `handoff`（後任が照合して旧ペインを閉じる）/ `.agent/orchestrator.md`（未送達）
- 「引き継ぎの材料が 1 つも無ければエラーを返す（master は事前に書く必要がある）。」の括弧
- 「pane / tab はこのマシンでしか意味を持たないので、知識に混ぜると別デバイスで誤った指示の元になる。節分離前の旧書式もそのまま読める」→ guide `handoff`（Write each file in two sections）

### tako_orchestrator_handoffs

- `show`: 「内容と書式。」（show が返すもの）

### tako_orchestrator_run

- 「完了判定はバックグラウンドで OrchestratorWorkerStatus と同じロジックを繰り返す。」の型名（説明文ではツール名 tako_orchestrator_worker_status にした）
- `sync`: 「旧挙動」「既定 false = 非同期」（既定値は schema の `default`）

### tako_orchestrator_respond

- 「ダイアログが画面に存在しない場合はエラー（誤爆防止）。」の理由部分
- 「下見の結果に labels_truncated=true が付いていたら、ラベルは TUI 自身に `…` で打ち切られていて元の文字列が画面に残っていない（狭いペインの /model 等）。この画面では番号でのみ確定できる（ラベル指定はエラーになる。誤選択の確定を防ぐため）。」の例と理由
- 「cursor_visible=false はダイアログがペインより高く選択カーソルが画面外にある」の原因部分
- 「（下見。選択肢一覧・現在のハイライト・番号キーの可否）」→ 同じ説明の構造フィールドの列挙

### tako_orchestrator_report

- 「第 1 層: tmux scrollback（capture-pane -J で折返し結合。全 agent 共通）。第 2 層: 構造化ソース（claude の transcript JSONL。ペイン幅非依存の全文品質）。」の層の呼び名

### tako_orchestrator_adopt

- 「引き継ぎ（tako_orchestrator_handoff と自動ハンドオフ）の宛先」の括弧
- 「（会話の途中で system prompt は差し替えられないため）」→ `.agent/requirements.md` / `.agent/commands.md`

### tako_orchestrator_layout

- 「legacy は従来の右等分割」の「従来の」
- 「master_ratio は master 側へ残す取り分（0.1〜0.9。既定 0.5 = 画面半分）」の「= 画面半分」
- 「**worker は必ず spawn 元と同じタブへ置く**（別タブへ逃がす配置は廃止した）」の経緯 → `.agent/requirements.md` FR-2.20.7 / `.agent/orchestrator.md`
- 「床（min_worker_font_scale。既定 0.6 = 既定サイズの 60%）」の言い換え、「cols_short=true と実桁数を載せる」の「実桁数」（pane_cols として列挙は残した）
- 「auto_shrink_font=false で自動縮小を切れる（狭いまま置く。タブは分けない）」の「狭いまま置く」

### tako_orchestrator_accounts

- `default_effort` / `default_model` / `description`: 「任意。」（required に無いことと同じ）

### tako_orchestrator_guide

- 「長寿命セッションの起動時固定費を減らすための仕組みで、返る本文は prompt から移した原文そのまま。」→ `crates/tako-control/src/orchestrator/guide.rs` の冒頭コメント

### tako_send_input

- 「claude 等の全画面 TUI への改行つき送信は送達確認ループで配送される: 信頼ダイアログの自動承諾 → bracketed paste 貼り付け → 分離 Enter → 入力欄が空になったことの検証 + Enter 単独再送」の段の列挙 → `.agent/architecture.md` / `.agent/commands.md`（分離 Enter）
- 「応答は queued: true が即座に返り、実際の送達確認はバックグラウンドで行われる」の後半
- 「決着済みの前回の顛末は出ない（この送達はまだ 1 tick も回っていないので queued になる）。」の括弧の理由（「決着済みの前回の顛末は出ない」は説明文に残した。番犬 `prompt_delivery_watchdog.rs` が要求する）
- 「続きは tako_read_pane の delivery で読み、persist.log にも同じ理由が 1 行残る。」の persist.log → `tako_read_pane` の説明に残る
- 「選択肢ダイアログ表示中は送信を拒否してエラーを返す（入力欄が奪われており、…）」の「入力欄が奪われており」
- `await_prompt`: 「送信はバックグラウンドで行われ、応答は即座に返る（queued: true。顛末は応答の delivery と tako_read_pane の delivery で追う）」→ 同じツールの description

### tako_read_pane

- 「自動提案（ゴーストテキスト）」の「ゴーストテキスト」、「重要:」の見出し語（強調で置き換えた）

### tako_links

- 「GUI の修飾 + クリックと同じ判定（tako_core::links）」の実装名
- 「（画面の写しを貼って判定を再現できる）」

### tako_tmux_cleanup

- 「器の見つけ方はプラットフォームで違う: unix はソケットファイルを走査し、Windows（psmux）は名前付きパイプでファイルが無いので器のプロセスのコマンドライン（-L <名前>）から見つけ、回収は器の pid を名指しして行う。」→ `.agent/commands.md`（名前付きパイプ）

### tako_autosuggest

- 「ユーザーが自前で zsh-autosuggestions を導入しているペインでは二重注入を避けて何もしない。」の理由部分 → `.agent/requirements.md`（二重注入）
- 「チュートリアル」（「案内」にした）

### tako_open_file

- 「（mode=code でソース表示へ切替可能 = プレビューの目アイコントグルと同じ操作）」の後半 → `.agent/requirements.md` / `.agent/architecture.md`（目アイコン）
- 「direction を指定すると再利用せず必ずその方向へ分割して開く（表示位置を制御したいとき）。」の括弧
- 「new_tab を指定すると新しいタブ 1 枚をそのファイル専用にする（Finder の「このアプリケーションで開く」と同じ表示）。」の括弧 → `.agent/commands.md`
- 「line を指定すると開いた直後にその行へ飛ぶ（定義・参照の着地点）。」の括弧

### tako_file_op

- 「trash = ゴミ箱（Windows はごみ箱）へ移動。完全削除ではないので復元できる」の「（Windows はごみ箱）」と「完全削除ではないので」

### tako_setup_mcp

- 「agy = agy mcp add（~/.gemini/config/mcp_config.json。env はそのまま継承される）」の「env はそのまま継承される」
- 「tako と通信する env の転送設定 env_vars」の「tako と通信する」

### tako_setup_bootstrap

- 公式インストーラの URL: 「claude = https://claude.ai/install.sh、codex = https://chatgpt.com/codex/install.sh、agy = https://antigravity.google/cli/install.sh」→ 実行時は `install_plan` の取得元に入る。正本は `crates/tako-core/src/platform/agent_install.rs`
- 「action=status-all（読み取り専用）は 3 系統ぶんをまとめて返すので、「どれが使える状態か」を 1 回で把握できる。」の後半

### tako_sessions

- 「会話本文は claude の transcript（~/.claude/projects/）への参照のみ持つ。」のパス
- remote_link.state の括弧: 「not_connected（まだ繋いでいない）/ ineligible: <理由>（この環境では繋げない）」、「url を返さない（捏造しない）」の「捏造しない」

### tako_logs

- 「対象は pane（クローズ済み可）か session_id（カタログ経由）。」の「カタログ経由」（引数説明に残る）
- 「ログはユーザーローカル保存で、」

### tako_remote_folder

- 「（Zed / VSCode の Remote SSH 相当）」「= VSCode Remote 相当」
- 「保存は SFTP の一時ファイル + rename でアトミック。」
- 「認証は ~/.ssh/config・鍵・ControlMaster をそのまま使う（追加設定なし）。」の括弧 → guide `remote`（6. Authentication is inherited, not configured）
- 「各行の origin = explicit / auto と placement = leading / trailing でローカルの前後どちらに出ているかが分かる」の後半
- 「切断中の保存はここに残るので無言で消えない」の「無言で消えない」
- 「GUI の「リモートからフォルダを開く」と 1:1。」

### tako_remote_start

- 「HTTP API サーバーがローカルエンドポイントで開始される。」
- 「daemon の待ち受けは既定でループバック TCP（127.0.0.1 のエフェメラルポート）」の括弧 → `.agent/architecture.md`（エフェメラル）
- 「tailnet 内限定の恒久固定 URL」の「恒久」、「（WireGuard E2E 暗号化・…）」の WireGuard → `.agent/requirements.md` / `.agent/architecture.md`

### tako_web

- 「macOS の WKWebView を ペインとして表示し」（WKWebView は残した）

### tako_update

- 「action=open で GUI のアップデート専用画面を開く（現在 / 最新バージョン・チャンネル・配布物・リリースノート・更新ボタンが載る。」の画面の中身
- 「channel で stable / test を指定可。省略で全チャンネル同時チェック。」「channel で stable（既定）/ test を指定。」→ 引数 `channel` の説明

### tako_sleep_guard

- 「macOS のアイドルスリープを IOKit 電源アサーションで防止する。蓋閉じ防止は pmset disablesleep で制御（sudoers 登録が必要）。」の仕組み → `.agent/architecture.md`（IOKit）/ `.agent/requirements.md`（disablesleep）
- 「バッテリー駆動（テザリング中など）」の例 → `.agent/requirements.md`（テザリング）

### tako_session_restart

- 「mode=handoff: …（自動ハンドオフの手動版）」→ `.agent/commands.md`
- 「claude 以外の系統は tako 側の resume 未配線のため対象外」の理由 → `tako_agent_support` の session_restart_harness / _handoff（説明文から案内）
- 「アカウント（CLAUDE_CONFIG_DIR）」の括弧
- `mode`: 「handoff = 引き継ぎを書かせてセッション交代」の「引き継ぎを書かせて」

### tako_migrate

- 「冪等なので何度実行しても壊れない」の後半
- 「（「退避済み」と読み違えないため）」の理由部分（「退避済み」ではない、として残した）

### tako_show_command

- 「commands は 1 件でも複数でもよく、改行を含む複数行コマンドは 1 要素として渡す（改行はそのまま保たれる）。」の「（改行はそのまま保たれる）」
- 「会話に書いたコマンドの代わりに出すこと。」（冒頭の「必ずこれを使う」と同じ）

### tako_test_residue

- 対象 dir の中身: 「`tako-test-data-<pid>`（cargo test の data dir）/ `tako-agent-config-<pid>`（検証プロセスのエージェント設定）/ `tako-test-scratch-<pid>`（テスト本体が作る使い捨ての作業ディレクトリの親）/ `tako-test-orchestrator-<pid>` / `tako-test-supervisor-<pid>`（オーケストレーターのテスト隔離先）」
- 「macOS の TMPDIR は再起動でも消えないので、放っておくと数千件積もる。」

### tako_context_budget

- 「積もった作業ログが毎ターン全文読み込まれてコンテキストを食い潰すのを防ぐための機構。」→ `.agent/conventions.md`「起動時ロードの予算（Issue #1139）」

### tako_tree_folder

- 「AI が作業対象プロジェクトのフォルダをファイルツリーに明示追加する。」（「プロジェクトの指示を受けたらそのルートフォルダを追加し」として残した）
- 「（画面と同じ分類）」「でステージ済みと未ステージを分けて持つ」「= 画面に出ている範囲」
- `action`: 「add: フォルダを追加, remove: フォルダを削除, … git-status: ツリーの git 状態を取得」（enum と description で足りる）

### tako_todo

- `copy_texts`: 「投稿文 / タイトル / タグを分けて入れるとスマホからワンタップでコピーできる」の後半 → guide `user-tasks`（copy_texts は分けて入れる）
- `kind`: 「review=生成物のレビュー / confirm=確認 / permission=権限の確認 / post=投稿」→ guide `user-tasks`（Pick `kind` by what you are asking for の表）
- 「起票元（プロファイル・会話・ペイン）は自動で入るので指定は要らない。」の括弧 → guide `user-tasks`

### tako_preview_move / tako_preview_delete（#1652 で追加、#1711 の rebase で短くした）

- move: 「（GUI の修飾キー付き矢印・Home・Page Down と同じ）」→ 打鍵と値の対応の正本は `tako_core::platform::editor_keys`（要件は `.agent/requirements.md` FR-3.5 の #1652 の段）。値は `tako_core::text_edit::CursorMovement`
- delete: 「（GUI の修飾キー付き Backspace / Delete と同じ）」→ 同じく `tako_core::platform::editor_keys`。値は `tako_core::text_edit::DeleteMotion`

### tako_run

- 「ファイル先頭 64 行以内に以下の形式でコメント内に記述する」「`tako:cwd[name]: <ディレクトリ>` — プロファイル別作業ディレクトリ」は 1 行へ畳んだ（書式はすべて説明文に残る）
- 「## tako:run 宣言の書式」「## 変数展開」「## 解決優先順位」の見出しと番号付きの並び（同じ中身を 1 行ずつにした）
