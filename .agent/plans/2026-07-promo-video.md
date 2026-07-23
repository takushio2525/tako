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

## シーン表（合計 72 秒）

尺はテンポ優先の目安。ナレーションなし・テロップ + BGM 前提（トーンはユーザー確認事項）。

| # | 時間 | 尺 | 内容（実 UI で収録する画） | テロップ文言（案） |
|---|---|---|---|---|
| S1 | 0:00–0:06 | 6s | フック: 1 タブに master + worker 3 体 + dev サーバーが並ぶ完成形。状態ドットが明滅 | 「AI エージェント開発、画面はもう散らからない」 |
| S2 | 0:06–0:18 | 12s | Claude Code に日本語で依頼 →「dev サーバーを隣で起動して」→ ペインが自動分割されサーバーログが流れ、README プレビューが開く | 「Claude Code がそのまま画面を操作する」「設定ゼロの内蔵 MCP」 |
| S3 | 0:18–0:32 | 14s | `tako master` 起動 → worker が 3 体 spawn されグリッド自動配置 → 右パネル orch ビューで俯瞰 → 完了した worker に緑ドット | 「master が worker を並べて監視」「1 グループ = 1 タブ」 |
| S4 | 0:32–0:44 | 12s | ファイルツリーから Markdown を開く → ライブリロードで編集が即反映 → Code Runner の再生ボタンでスクリプト実行 → PDF/画像プレビュー | 「成果物はターミナルの中で確認」 |
| S5 | 0:44–0:54 | 10s | Cmd+Q で終了 → 再起動 → 全タブ・全ペインが画面内容ごと復元、サーバーログが続きから流れる | 「再起動しても、全部戻ってくる」 |
| S6 | 0:54–1:04 | 10s | スマホ実機（または DevTools モバイル表示）: PWA でペイン一覧 → ライブ画面 → 承認ダイアログに応答 | 「外出先はスマホから」 |
| S7 | 1:04–1:12 | 8s | クロージング: テーマ切替 + 設定画面のちら見せ → tako ロゴ → GitHub URL | 「tako — AI エージェント時代の GUI ターミナル」「github.com/takushio2525/tako」 |

各シーンの実装確認: S1〜S5・S7 はすべて既存機能（CLI/MCP から再現可能）。
S6 のみ実機スマホ収録 or シミュレータの選択が残る（下記「Phase B への引き継ぎ」）。

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
| `scripts/promo/record-scenes.sh` | シーン別収録（`agent` / `preview` / `restore` / `outro` / `all`） |
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
- **内容**: 80 秒 / 100 BPM / A マイナー / コード進行 Am-F-C-G。キック・ハイハット・
  ベース・アルペジオ・パッドを台本のシーン割りに合わせて出し入れし、S5（再起動シーン）で
  いったん抜いて復帰させる。実測 mean -17.6 dB / max -1.9 dB、動画合成時は volume 0.85

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

### 進捗と残作業（2026-07-23 時点）

- 完了: パイプライン一式、BGM（80 秒）、S4（プレビュー）素材、編集合成の通し確認
- **未収録: S1/S2/S3（agent）・S5（restore）・S6（remote）・S7（outro）**
  → 収録中にユーザーが離席し画面ロックがかかったため中断。ロック解除後に
  `scripts/promo/record-scenes.sh all` で再開できる
- 収録が全部揃ったら `scripts/promo/build-video.sh` で
  `~/Desktop/tako-promo/tako-intro-v1.mp4` が完成する
