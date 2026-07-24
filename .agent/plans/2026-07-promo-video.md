# tako 紹介動画 構成台本（#470）

> 実 UI のスクリーンレコーディングのみを素材にしたプロモーション動画の構成台本。
> Phase A = 本台本 + 収録パイプライン検証（サンプルクリップ 1 本）。
> Phase B（全シーン収録・編集・BGM・公開）はユーザーの構成承認後に着手する。

## 方針（ユーザー指示 2026-07-22）

- **忠実に UI を再現**: モック・アニメーション再現は使わない。素材はすべて実アプリ
  （/Applications/tako.app）の画面収録
- **強みを前面に**: 下記「訴求する強みの優先順」に従い、機能紹介ではなく
  「何が嬉しいか」を 1 シーン 1 メッセージで見せる
- テロップ・字幕に**絵文字を使わない**（tako のブランド方針。UI 本体も絵文字ゼロ）
- 収録は**隔離インスタンス**（TAKO_ISOLATED=1 + 明示ソケット）+ **デモ用ダミーデータ**で行い、
  個人情報（ユーザー名・ホスト名・実 tailnet 名・token・メール・実パス）を 1 フレームも写さない

## 訴求する強みの優先順（実装状況を確認済みのもののみ）

| 優先 | 強み | 一言メッセージ | 実装根拠 |
|---|---|---|---|
| 1 | AI がそのまま画面を操作（設定ゼロ MCP + CLI） | 「Claude Code がペインを割り、コマンドを流し、成果物を開く」 | Phase 2/3 完了・MCP 106 ツール |
| 2 | オーケストレーションが画面で見える | 「master が worker を並べ、全員をひと目で監視」 | `tako master` / spawn レイアウトエンジン（#165）/ 右パネル orch ビュー（#217） |
| 3 | 再起動しても全部戻る | 「エージェントも dev サーバーも、画面ごと復元」 | Phase 5.5 tmux バックエンド永続化 + #30/#177/#381 |
| 4 | ターミナルの中で成果物確認 | 「コード・Markdown・PDF・画像、プレビューは生きている」 | FR-3.2/3.3 + ライブリロード（#233）+ Code Runner（#453） |
| 5 | スマホからリモート監視・承認 | 「外出先はスマホから見る・答える」 | `tako remote`（PWA。#42/#63 ほか） |
| 6 | 速い・軽い | 「Rust + GPUI。Zed 級のネイティブ描画」 | 技術スタック（数値訴求は Phase B で実測して添える） |

補足: 設定画面（#459）・テーマ切替（#217）・日英 i18n（#435）は単独シーンにせず、
クロージングの「ちら見せ」素材として扱う（訴求の焦点を絞るため）。

## シーン表（v3 = 現行。合計 約 106 秒）

> v3（2026-07-24）では順序を保ったまま **③ setup の訴求を作り直し**（コマンド紹介 →
> 対話セットアップエージェント）、**④ master に S6c「プロジェクト文脈の解決」を追加**した。
>
> v2（2026-07-24）でユーザー指示により**本編の順序を固定**した:
> **① 画面操作 → ② プレビュー → ③ setup → ④ master**。
> ③ と ④ が動画の 2 本柱で、どちらも実 UI 収録が必須。
> restore（再起動復元）と remote（スマホ）は 4 本柱から外れるため v2 の本編からは外した
> （素材 `restore-before/after-raw.mp4` は残してあり、尺を伸ばす判断が出たら
> master とアウトロの間に補足として挟める）。

尺はテンポ優先の目安。ナレーションなし・テロップ + BGM。テロップは**背景パネル付き**（後述）。

| # | 尺 | 素材 | 内容（実 UI で収録する画） | テロップ本文 / 副題 |
|---|---|---|---|---|
| S1 | 5s | agent-raw @46s | フック: 1 タブに Claude Code + dev サーバー + README プレビューが並ぶ完成形 | 「エージェントも、その子プロセスも、1 つのタブに」 |
| S2 | 12s | agent-raw @32s | Claude Code に日本語で依頼 → tako MCP でペインが自動分割され dev サーバーが起動 | 「日本語で頼むと、AI がペインを割って動かす」/「設定ゼロの内蔵 MCP サーバー」 |
| S3 | 9s | agent-raw @62s | 起動したサーバーのログと README プレビューが同じタブに並ぶ | 「起動したサーバーも、開いた資料も、同じ画面に」/「1 グループ = 1 タブ」 |
| S4 | 12s | preview-raw @3s | Markdown プレビュー → 外部編集がライブリロードで即反映 → Code Runner でスクリプト実行 | 「成果物はターミナルの中で確認する」/「ライブリロードと Code Runner」 |
| S5a | 9s | setup-raw @3s | `tako setup` が質問ゼロで検出を終え、**対話アシスタントが自動で立ち上がる** | 「設定ファイルは、自分で書かなくていい」/「tako setup — 質問ゼロで検出し、対話アシスタントが立ち上がる」 |
| S5b | 11s | setup-raw @48s | 日本語で「品質重視で使いたい」と相談 → アシスタントが現状を読んで設定を整える | 「あとは日本語で相談するだけ」/「セットアップエージェントが、あなたの環境に合わせて設定を整える」 |
| S5c | 10s | setup-raw @95s | 続けてプロジェクト登録も会話で決まる | 「指示ファイルも、プロファイルも、会話で決まる」/「反映するのは同意した項目だけ」 |
| S6a | 12s | master-raw @6s | `tako master` が立ち、worker が spawn されて同じタブにグリッド配置される | 「master が worker を spawn し、同じタブに並べる」/「tako master」 |
| S6b | 12s | master-raw @38s | 4 体目の worker が増え、右パネル orch ビューで全 worker を俯瞰 | 「全員の進捗が 1 画面で分かる」/「worker ごとにモデルもエージェントも振り分けられる」 |
| S6c | 11s | project-raw @8s | **ホーム**で起動した master に「awesome-app の件」と言うだけで、登録済みプロジェクトが解決され、そのディレクトリで worker が立つ | 「ホームで起動しても、登録したプロジェクトを解決する」/「名前を言うだけで、worker はそのディレクトリで立ち上がる」 |
| S7 | 8s | outro-raw @4s | クロージング: テーマ切替 → ロゴ → GitHub URL | 「tako」/「AI エージェント時代の GUI ターミナル / github.com/takushio2525/tako」 |

## 2 本柱の訴求内容（実装で裏を取った事実のみ）

宣伝物なので**実在しない機能・未実装の挙動はテロップに書かない**。
下記は実装・README・実機実行で確認した範囲だけを採用している。

### ③ setup（対話セットアップエージェント。v3 で訴求を作り直し）

v2 は `tako setup --check` / `tako setup-mcp` という**コマンドの紹介**だった。
v3 では tako setup の一番の売りである **「設定ファイルを自分で書かず、対話アシスタントと
会話して自分の環境に合わせられる」** ことへ訴求を移した（ユーザー指示 2026-07-24）。

確認した実装（読んだソースを明記する。ここに書いた挙動以外はテロップに書かない）:

- `crates/tako-cli/src/setup.rs`
  - `tako setup` は検出（検出値 → 前回値 → 既定値）を**質問ゼロ**で終える
  - 検出後、既定で**対話アシスタントを自動起動する**（`launch_setup_agent`）。
    スキップされるのは `--yes` / 非 TTY / `--answers launch_agent=none` のときだけ
    （`skip_agent` の判定）。`--review` は個別見直しモードで起動する
  - 起動時に `setup-context.yaml`（選択エージェント・グローバル指示ファイルのパス・
    導入済み / 認証済み CLI・検出したプラン・推奨 profile のメモ・
    指示ファイルの充足度 `instruction_coverage`）と、同梱の推奨ルール一式
    （`resources/setup/templates/sections/` の 7 項目）を作業ディレクトリへ書き出し、
    アシスタントはそこから現状を読む
  - 作業ディレクトリは `~/Library/Application Support/tako/setup`（`setup_dir()`）
- `resources/setup/system-prompt.md`（アシスタントの指示。バイナリ同梱）
  - 「質問をする前に必ず現状を Read する」= `setup-context.yaml` / グローバル指示ファイル /
    `config.yaml` / `profiles/*.yaml` / `projects.yaml` / `pending-changes.md`
  - 既存のグローバル指示ファイルは同梱推奨ルールと**項目レベルで突き合わせ**、
    充足 / 不足 / 相違を一覧で提示し、**同意した項目だけ**反映する
    （「既存のカスタマイズを黙って削除・上書きしない」と明記されている）
  - オーケストレーション設定（master / worker のエージェント・モデル・effort・
    worker ポリシー）とプロジェクト登録も対話で決める
  - 認証情報・token・メールアドレス・account ID は表示・転記しない
- `resources/setup/changes.yaml` … アップデート追従（`tako setup --changes`）

採用したテロップ（S5a / S5b / S5c）は上記のうち **「質問ゼロの検出のあとアシスタントが
自動で立ち上がる」**「**日本語で相談するだけで環境に合わせて設定が整う**」
「**指示ファイル・プロファイル・プロジェクト登録が会話で決まり、反映は同意した項目だけ**」
の 3 点に絞った。`tako setup-mcp` 単体の紹介は本編から外した（対話に含まれるため）。

### ④ master（オーケストレーション）

確認した実機能（`crates/tako-control/src/orchestrator/` / AGENTS.md のコマンド表 /
`tako orchestrator --help`）:

- `tako master` … master system prompt 付きでエージェント CLI（claude 既定 / codex 可）を
  現在のペインにインライン起動する
- `tako orchestrator spawn` … worker を新しいペインへ spawn。**worker ごとに
  `--agent`（claude / codex / agy）・`--model`・`--effort` を指定できる**
  （プロファイル側の `worker_agent` / `worker_model` / `worker_model_policy` でも既定を振り分け可）
- spawn レイアウトエンジン（#165）… `master-reserved` ポリシーで master の取り分を保ったまま
  worker 領域を grid / spiral で自動配置する。close 時は worker 領域だけリフローする
- 右パネルの orch ビュー（#217）… master + worker のツリーを俯瞰する
- `tako orchestrator watch` / `workers` / `report` … 完了・エラー・許可待ち・突然死の検知と、
  ペインが消えたあとも追える worker レジストリ

採用したテロップ（S6a / S6b）は **「1 コマンドで worker が同じタブに並ぶ」** と
**「進捗が 1 画面で分かる・worker ごとにモデル/エージェントを振り分けられる」** の 2 点。

### ④' master のプロジェクト文脈解決（S6c。v3 で追加）

確認した実装:

- `crates/tako-control/src/orchestrator/mod.rs` … `projects.yaml` は
  `key` → `{ cwd, description }` のレジストリ。実体は data_dir 配下
  （`config_dir()` = `<data_dir>/orchestrator/`。通常は
  `~/Library/Application Support/tako/orchestrator/projects.yaml`）
- `crates/tako-control/src/orchestrator/default_system_prompt.md` の
  **Step 0 — Resolve target projects (before anything else)** … master は依頼を受けたら
  列挙やファイル探索より先に `tako_orchestrator_projects(action=list)` を引き、
  ① `key`（大小文字・スペース / ハイフン / アンダースコアの揺れを吸収）
  ② `cwd` のベース名 ③ `description` の部分一致 で突き合わせる。
  高確度の一致が 1 件なら採用し、その `key` と `cwd` をタスク中の spawn / run に使う。
  複数一致なら候補を出して聞き返す。**登録済みプロジェクトは web 検索や
  ホームディレクトリ探索より優先する**と明記されている
- `crates/tako-control/src/dispatch.rs` … spawn は `ProjectsConfig::resolve_cwd(project)` で
  登録 cwd を解決して worker を起動する。つまり **master 自身の cwd は関係なく**、
  ホームで `tako master` を起動しても worker はプロジェクトのディレクトリで立ち上がる

採用したテロップ（S6c）は **「ホームで起動しても、登録したプロジェクトを解決する」**
「**名前を言うだけで、worker はそのディレクトリで立ち上がる**」の 2 点。

**書かなかったこと（実装で裏が取れないため）**: 「最近何をやっていたかを自動で把握する」。
`projects.yaml` が保持するのは `key` / `cwd` / `description` だけで、進捗の記憶ではない。
最近の作業内容の理解は、master がそのプロジェクトの `.agent/progress.md` や git 履歴を
読んだ結果であって、レジストリが与えるものではない。宣伝物なので、
レジストリの機能として書くことは避けた。

## 収録パイプライン（Phase A で検証済み）

サンプルクリップ = S2 の簡略版（AI 操作によるペイン分割 + プレビュー + dev サーバー起動）。

- スクリプト: `scripts/promo/record-scenes.sh`（+ `lib.sh` / `winbounds.swift`。Phase A 時点では record-sample.sh という単体スクリプトだった）
- 出力: `~/Desktop/tako-promo/scenes/<シーン>-raw.mp4`（リポにはコミットしない）
- 検証結果: 1920x1200（Retina 2x）/ 60fps / 15.0 秒 / H.264。1fps 抽出の全 15 フレームを
  目視確認し PII ゼロ（プロンプトは ZDOTDIR 差し替えで「awesome-app ❯」のみ、
  パスは /private/tmp/tako-demo/ 配下のみ、タブ名・ステータスバーにも個人情報なし）

### 収録手順（再現方法）

1. `/Applications/tako.app` を最新にしておく（`scripts/build-app.sh --install`）
2. `scripts/promo/record-scenes.sh <シーン>` を実行する。スクリプトが行うこと:
   - `/private/tmp/tako-demo/` にダミープロジェクト（awesome-app）と
     クリーンプロンプトの ZDOTDIR を生成
   - `TAKO_ISOLATED=1` + 明示 `TAKO_TMUX_SOCKET` / `TAKO_DATA_DIR` /
     `TAKO_DISCOVERY_DIR` で隔離 GUI インスタンスを起動（本番の tako・tmux・
     remote 状態に触れない）。継承 `TAKO_*` 環境変数はすべて `env -u` で遮断
   - 隔離インスタンスの `<data_dir>/tako.sock` + `token` へ CLI を明示接続し、
     ペイン操作（cd / open / split / send / equalize）をタイムライン再生
   - `winbounds.swift`（CGWindowList）で隔離ウィンドウの ID を取得し、
     `screencapture -l<windowID>` の連番キャプチャ → ffmpeg 結合で mp4 化
   - ffprobe 検証 + 1fps フレーム抽出まで自動実行
3. 抽出フレームを**全数目視**し PII が無いことを確認する（必須。省略しない）

前提: ログイン状態（画面ロック中は不可）、macOS 画面収録権限、ffmpeg/ffprobe。
Phase A では `screencapture -v -R<矩形>` を使っていたが、黒画面と写り込みの問題で
Phase B の方式（ウィンドウ単体キャプチャ）へ差し替えた。下記「収録の技術制約」を参照。

## Phase B（2026-07-23 着手）

ユーザー承認: 構成はそのまま、BGM 必須。未回答項目は次の既定で進める。

- テロップ: **日本語のみ**
- S6（リモート）: PC のモバイル表示で代用し、後で実機素材に差し替えられるよう**独立クリップ**にする
- S2: 実 Claude Code の画面を写してよい（モデル名等の公開情報は可。PII は全数チェック）

### 制作パイプライン（Phase A から作り直した）

| スクリプト | 役割 |
|---|---|
| `scripts/promo/lib.sh` | 隔離インスタンス起動・デモ環境生成・収録・検証の共通処理 |
| `scripts/promo/record-scenes.sh` | シーン別収録（`agent` / `preview` / `setup` / `master` / `restore` / `outro` / `all`） |
| `scripts/promo/winbounds.swift` | 収録対象ウィンドウ ID と矩形の取得 |
| `scripts/promo/caption.swift` | テロップ PNG 生成（CoreText） |
| `scripts/promo/make-bgm.py` | BGM 合成 |
| `scripts/promo/build-video.sh` | 切り出し・テロップ合成・連結・BGM 合成 |

実行順: `record-scenes.sh all` → `make-bgm.py` → `build-video.sh`。
`build-video.sh` は素材が欠けたシーンを警告して飛ばすので、途中経過でも通し確認できる。

### 音源（BGM）— 出典とライセンス

**自作**（`scripts/promo/make-bgm.py` による波形合成）。外部音源は一切使っていない。

- **出典**: tako リポジトリ内の `scripts/promo/make-bgm.py`。Python 標準ライブラリ
  （`wave` / `math` / `array` / `random`）だけで波形から生成しており、サンプリング素材・
  ループ素材・学習済みモデルのいずれも用いていない
- **ライセンス**: 生成物・生成コードとも tako 本体と同じ **GPL-3.0-or-later**。
  再配布・改変・商用利用いずれも可（tako のライセンス条件に従う限り）
- **第三者権利**: 無し。クレジット表記の義務も無い
- **内容**: 90 秒 / 100 BPM / A マイナー / コード進行 Am-F-C-G。キック・ハイハット・
  ベース・アルペジオ・パッドを台本のシーン割りに合わせて出し入れする。
  v2 では構成変更に合わせて出し入れの位置も更新した（setup 節でいったん軽くし、
  master 節で最も厚くし、アウトロで抜く）。動画合成時は volume 0.85

外部音源を使わなかった理由: 再配布可能なライセンスの確認・クレジット表記・将来の差し替え
リスクをすべて回避でき、かつ台本のシーン割りに合わせて構成を作り込めるため。

### 収録の技術制約（Phase A から判明した重要な知見）

1. **`screencapture -v`（動画モード）は本環境で黒画面**。静止画モード（`-x`）は正常。
   → 動画は「連番静止画キャプチャ + ffmpeg 結合」で作る（実測 13〜16 fps）
2. **画面全体を撮って切り出す方式は使ってはいけない**。`ffmpeg -f avfoundation` や
   `screencapture -R` は対象ウィンドウの手前に別アプリのウィンドウが重なると、その中身ごと
   写り込む。2026-07-23 に実際に他アプリの内容が混入した素材を作ってしまい破棄した。
   → **`screencapture -l<windowID>` によるウィンドウ単体キャプチャが必須**。
   手前に何が来ても対象ウィンドウの内容しか撮れない
3. **画面ロック中は一切キャプチャできない**（macOS の仕様。権限の問題ではない）。
   別 Space に隔離ウィンドウがある場合も撮れない。
   → 収録中は**ログイン状態を保ち、隔離ウィンドウと同じ Space に留まる**必要がある。
   `lib.sh` の `promo_check_capturable` が事前に検査し、ロックと権限不足を切り分けて報告する
4. **本環境の ffmpeg に `drawtext` が無い**（libfreetype 無しビルド）。
   → テロップは `caption.swift`（CoreText）で透過 PNG を描き `overlay` で合成する
5. PNG を overlay するときは **`-loop 1 -framerate <fps>`** が要る。framerate を与えないと
   PTS が進まず `fade` が一切効かない。出力には必ず `-y` を付ける（付け忘れると
   前回の生成物が残り、修正が反映されていないように見える）
6. **収録ウィンドウが他のウィンドウの背後に完全に隠れると GPUI が描画を止める**
   （2026-07-24 に判明）。`screencapture -l<windowID>` は、そのウィンドウが今の Space に
   居て `onscreen=true` でも、直前に描画されたサーフェスをそのまま返す。つまり
   **同じ絵が延々と撮れ続けるのに気づけない**（v2 の setup シーンを実際に 2 回撮り直した）。
   対策は 2 つとも実装済み:
   - `promo_record_start` と収録ループ（20 フレームごと）が `winbounds --activate`
     （`NSRunningApplication.activate`）で対象アプリを最前面に戻し続ける
   - `promo_verify` が抽出フレームのハッシュ種類数を数え、「異なるフレームが 1/3 未満」なら
     **失敗として止める**（動いていない素材を合成まで進ませない）
7. **デモ HOME では実エージェント CLI が「Not logged in」になる**（2026-07-24 v3 で判明）。
   `HOME` を差し替えると macOS のキーチェーン検索リストから**ログインキーチェーンが外れる**
   （`security list-keychains` が `System.keychain` だけになるのを実測）。claude の認証情報は
   ログインキーチェーンにあるため、デモ HOME のままでは対話セッションが撮れない。
   → `promo_demo_home_agent_ready`（`lib.sh`）が、デモ HOME 側の検索リストに実ユーザーの
   ログインキーチェーンを指定する。**認証情報のコピー・書き出しはしない**（鍵は元の場所のまま。
   実 HOME 側の設定も変更しない）。併せて、収録中にオンボーディング・信頼ダイアログ・
   許可プロンプトが出ないよう、使い捨て HOME にだけ最小の設定
   （`.claude.json` の `hasCompletedOnboarding` と収録対象ディレクトリの信頼、
   `.claude/settings.json` の権限モード）を書く。**アカウント情報（メール等）は書かない**。
   権限モードに `bypassPermissions` を使ってはいけない（起動時に赤い警告ダイアログが出て
   Enter 待ちになり、収録に写り込む。実際に 1 回撮り直した）。`acceptEdits` + allow 一覧にする
8. **`tko`（CLI ラッパー）には `TAKO_DATA_DIR` も渡すこと**（2026-07-24 v3 で判明）。
   `tako orchestrator projects add` / `profiles set` のようなローカル設定を直接書く
   サブコマンドは IPC を経由せず**自分の** data_dir を見る。ソケットとトークンだけ渡すと
   隔離インスタンスを操作しているつもりで**本番の `projects.yaml` / `profiles/default.yaml` を
   書き換える**（v3 の 1 回目の収録で実際に汚染し、手で復旧した）。
   同時に、隔離側のレジストリが空になるため master のプロジェクト解決も撮れない
9. **`tako send --await-prompt` は生成中のエージェントを中断させることがある**
   （送達検証で Enter を撃ち直すため。v3 の project シーンで `Interrupted` になった）。
   長い応答を待つ収録では素の `tko send` を 1 回だけ使う

## v3 での変更（2026-07-24）

v2 を視聴したユーザーの指示 2 点を反映したのが v3
（`~/Desktop/tako-promo/tako-intro-v3.mp4`）。テロップ背景・構成順・絵文字ゼロ・
BGM・setup 以外のシーンは v2 のまま。

### 1. setup 節を「対話セットアップエージェント」の訴求へ作り直した

v2 の setup 節は `tako setup --check` → `tako setup-mcp` → `claude mcp list` という
**コマンドの紹介**だった。tako setup の一番の売りは「対話型のセットアップエージェントと
会話しながら自分の環境に合わせて設定を詰められる」ことなので、そこへ寄せた
（訴求の実装的な裏取りは上記「③ setup」節）。

- `record-scenes.sh setup` を全面的に書き直し、`tako setup` から始めて
  **対話アシスタントが自動起動し、日本語の相談で設定が決まっていく**ところを撮る
  （3 ビート: setup 実行 → 「品質重視にして」の相談 → プロジェクト登録）
- テロップは S5a / S5b / S5c の 3 枚に増やした
- 実 claude の対話をデモ HOME で撮るために「収録の技術制約 7」の対処が要った

### 2. master 節に S6c「プロジェクト文脈の解決」を足した

`record-scenes.sh project` を新設（素材 `project-raw.mp4`）。デモ用に 3 件の
プロジェクトを登録し、**プロジェクトディレクトリではなくホームで** `tako master` を起動して、
「awesome-app の件」と名前だけ言う。master は Step 0 でレジストリを引いて対象を特定し、
その cwd で worker を立ち上げる。訴求の裏取りは上記「④' master のプロジェクト文脈解決」節。

### 3. BGM を 115 秒へ伸ばし、セクションの出し入れを v3 の尺に合わせ直した

本編が 84 秒 → 約 106 秒になったため、`make-bgm.py` の `TOTAL` と
セクション境界（setup 36〜65s を軽く / master + プロジェクト文脈 65〜98s を最も厚く /
アウトロ 98s〜で抜く）を更新した。

## v2 での変更（2026-07-24）

v1（58 秒。`~/Desktop/tako-promo/tako-intro-v1.mp4`）に対するユーザー指摘 2 点と
追加指示 2 点を反映したのが v2（`~/Desktop/tako-promo/tako-intro-v2.mp4`）。

### 1. テロップに背景パネルを付けた（可読性）

v1 は白文字 + 影だけで、背景の UI（README プレビューの箇条書き・claude の ctx バー・
ログ）と重なった箇所が読めなかった。`caption.swift` を書き直し、
**テキストの外接矩形に合わせた角丸の半透明パネル**を敷く方式にした。

- 塗り: `#0a0d12` 相当（tako のダーク背景寄り）を **不透明度 0.84**
- 角丸半径 = フォント px × 0.28、左右パディング = 0.62、上下 = 0.34
- パネル自体にドロップシャドウ（明るい背景でも縁が溶けない）+ 白 14% の 1.5px 縁
- パネル幅はテキスト幅に追従（全幅の帯にはしない）。キャンバスは動画幅のまま透過なので
  `overlay=0:H-h-90` の合成位置は v1 から変更なし。フェードもキャンバス全体の alpha で効く

### 2. setup 節と master 節を新設（本編 4 本柱）

上記「シーン表（v2）」「2 本柱の訴求内容」を参照。収録シーンを 2 つ追加した。

- `record-scenes.sh setup` … デモ用 HOME（`/private/tmp/tako-demo/home`）と
  デモ用 PATH（`/private/tmp/tako-demo/bin` に必要なコマンドの symlink だけ）へ
  差し替えて隔離インスタンスを起動する（`promo_make_demo_home`）。
  これをやらないと `tako setup --check` が `/Users/<ユーザー名>/...` を画面に出す
- `record-scenes.sh master` … `tako master` で実 Claude Code の master を立て、
  master 自身に `tako_orchestrator_spawn` で worker を並べさせる。
  worker が出そろい、かつ**全ペインのテキストから PII が消えた**ことを
  `promo_wait_pii_clear` で機械確認してから収録を開始する
  （claude の起動バナーにはアカウントのメールアドレスが出るため）

### 3. 本編の順序を固定した

**画面操作 → プレビュー → setup → master**（ユーザー指示）。
restore（再起動復元）と remote（スマホ）は 4 本柱から外れるため本編から外した。
restore 素材は残してあるので、尺を伸ばす判断が出れば master とアウトロの間に挟める。

## 実行手順（v3 を作り直すとき）

```sh
scripts/build-app.sh                  # dist/tako.app（/Applications には触れない）
export TAKO_PROMO_APP=$PWD/dist/tako.app/Contents/MacOS/tako-app
export TAKO_PROMO_CLI=$PWD/dist/tako.app/Contents/MacOS/tako
scripts/promo/record-scenes.sh all    # agent → preview → setup → master → project → outro
scripts/promo/make-bgm.py
scripts/promo/build-video.sh          # 既定の出力先が ~/Desktop/tako-promo/tako-intro-v3.mp4
```

シーン単位で撮り直すときは `record-scenes.sh <scene>`（`setup` / `project` など）。
素材が揃っていないシーンは `build-video.sh` が警告して飛ばす。

収録後は `/private/tmp/tako-promo-frames/<シーン>/` の抽出フレームを**全数目視**して
PII が無いことを確認する（`promo_verify` が自動抽出する）。
