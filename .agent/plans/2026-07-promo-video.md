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

## シーン表（v2 = 現行。合計 84 秒）

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
| S5a | 9s | setup-raw @2s | `tako setup --check` が足りない設定を列挙（Claude MCP 未登録など） | 「足りない設定は、tako が自分で教えてくれる」/「tako setup --check」 |
| S5b | 9s | setup-raw @13s | `tako setup-mcp` で登録 → `claude mcp list` に `tako … ✔ Connected` | 「Claude Code 連携は、コマンド 1 つ」/「一度登録すれば、どのプロジェクトでも設定不要」 |
| S6a | 12s | master-raw @4s | `tako master` が立ち、worker が spawn されて同じタブにグリッド配置される | 「master が worker を spawn し、同じタブに並べる」/「tako master」 |
| S6b | 12s | master-raw @22s | 4 体目の worker が増え、右パネル orch ビューで全 worker を俯瞰 | 「全員の進捗が 1 画面で分かる」/「worker ごとにモデルもエージェントも振り分けられる」 |
| S7 | 8s | outro-raw @4s | クロージング: テーマ切替 → ロゴ → GitHub URL | 「tako」/「AI エージェント時代の GUI ターミナル / github.com/takushio2525/tako」 |

## 2 本柱の訴求内容（実装で裏を取った事実のみ）

宣伝物なので**実在しない機能・未実装の挙動はテロップに書かない**。
下記は実装・README・実機実行で確認した範囲だけを採用している。

### ③ setup（導入の簡単さ）

確認した実機能（`tako setup --help` / `crates/tako-control/src/setup.rs` /
`resources/setup/changes.yaml` / README「Claude Code 連携」節 / 実行結果）:

- `tako setup --check` … 環境チェック。エージェント CLI（claude / codex / agy）の検出と認証状態、
  tmux / git / tailscale、フルディスクアクセス、**Claude MCP への tako 登録有無**、
  Codex MCP、グローバル指示ファイル、スリープ防止、プロファイルの有無を列挙する
- `tako setup-mcp` … Claude Code のユーザー設定へ tako MCP を登録（内部で
  `claude mcp add --scope user`）。**user スコープなので以後どのプロジェクトでも設定不要**
- `tako setup` … 質問ゼロの自動セットアップ（検出値 → 前回値 → 既定値で解決。`--yes` /
  `--answers` で完全非対話、`--review` のみ個別対話）。アップデート追従（`--changes`）も持つ
- 実測の payoff: `claude mcp list` が `tako: … ✔ Connected` を返す

採用したテロップ（S5a / S5b）は上記のうち **「不足を自分で教える」** と
**「連携はコマンド 1 つ・以後どのプロジェクトでも設定不要」** の 2 点に絞った。

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

## 実行手順（v2 を作り直すとき）

```sh
scripts/build-app.sh                  # dist/tako.app（/Applications には触れない）
export TAKO_PROMO_APP=$PWD/dist/tako.app/Contents/MacOS/tako-app
export TAKO_PROMO_CLI=$PWD/dist/tako.app/Contents/MacOS/tako
scripts/promo/record-scenes.sh all    # agent → preview → setup → master → outro
scripts/promo/make-bgm.py
scripts/promo/build-video.sh ~/Desktop/tako-promo/tako-intro-v2.mp4
```

収録後は `/private/tmp/tako-promo-frames/<シーン>/` の抽出フレームを**全数目視**して
PII が無いことを確認する（`promo_verify` が自動抽出する）。
