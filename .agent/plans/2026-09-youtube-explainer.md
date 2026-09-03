# tako YouTube 解説動画 構成台本（#1081）

> YouTube 公開用の長尺解説動画「tako とは何か / どう使うか / 何がいいか」の台本・
> ナレーション原稿・収録手順・裏取りの正本。#470（106 秒ティザー）の資産
> （`.agent/plans/2026-07-promo-video.md` / `scripts/promo/`）を土台に、
> ナレーション駆動の合成パイプラインへ作り直した。
> 成果物（mp4 / 音声 / サムネ）はリポジトリ外 `~/Desktop/tako-promo/`（コミットしない）。

## 方針

- **正確性が最優先**: 訴求は AGENTS.md / docs / 実装で裏を取ったものだけ（下の「訴求と裏取り」）。
  実装中の刷新（#1077〜#1080 = リモート刷新の C〜H）には触れない。
  「できる」と言えないものは言わない（例: agy の利用上限復帰、Windows の自動 SSH 検知）
- **素材は実 tako の収録のみ**（隔離インスタンス `TAKO_ISOLATED=1` + デモ用 HOME / TAKO_DATA_DIR）。
  唯一の例外は 7 章のスマホ画面で、**PWA の UI そのものは本物**（`web/tako-remote` を
  そのまま配信）だが **API はモック**（実 daemon は tailscale serve を張る = 本番の
  Tailscale / remote 状態に触れるため）。テロップに「画面はデモ用データ」と明記する
- ナレーションは macOS 同梱の日本語 TTS（`say -v Kyoko`。この機で使える唯一の日本語音声）。
  区間の長さは**ナレーションが駆動**する（`max(min_dur, 音声秒 + 0.8)`）
- UI・テロップ・カードに絵文字を使わない（ブランド方針）
- PII ゼロ: 全フレームを Vision OCR にかけ、メール / 実ホームパス / ユーザー名 / ホスト名 /
  tailnet / トークン / UUID の各パターンが 0 件になるまで「完成」と言わない

## 構成（8 章 + オープニング / クロージング）

| # | 章 | 見せる実画面 | 主張（要約） |
|---|---|---|---|
| 0 | オープニング | 完成形（master + worker 3 体 + orch パネル） | AI エージェント時代のための GUI ターミナル |
| 1 | 課題 | 4 タブに散らばった処理 → 1 タブへ集約 | 1 作業が 4 つに分裂して散らばる。AI に操作させる繋ぎ込みも自前 |
| 2 | 思想 | 実 Claude Code が MCP でペインを割る / CLI が GUI を動かす | 1 グループ = 1 タブ / ゼロコンフィグ / AI フルコントロール |
| 3 | 導入 | brew カード → `bootstrap --dry-run` → `tako setup` → 対話アシスタント | brew 1 行。setup 一発。Claude 未導入からでも通る |
| 4 | 基本操作 | 分割 / タブ / ツリー / md ライブリロード / PDF / 画像 / コード / Code Runner | 普通のターミナル + エディタ風の確認手段 |
| 5 | AI に任せる | `tako master` → worker 3 体 → orch ビュー → かんたん表示 → 報告 | 分解・型つき指示・送達確認・監視・検収 |
| 6 | 再起動しても戻る | 終了直前 / 再起動後 | tmux バックエンドで画面ごと復元 |
| 7 | スマホから | PWA（一覧 / チャット + 承認カード）/ Remote Control の opt-in | tailnet 内固定 URL + 機器承認。Claude 公式 Remote Control へ委譲（既定 OFF） |
| 8 | Windows と OSS | 対応状況ページ / LICENSE / README / brew カード | 同一リリースに Windows 同梱。実機実測だけを「対応」。GPL-3.0 |

区間ごとの尺・テロップ・ナレーション原稿の**正本は `scripts/promo/explainer-timeline.tsv`**
（タブ区切り。`build-explainer.sh` がそのまま読む）。ナレーションの読みは TTS 向けに
外来語をカナで書いてある（`speech` 列）。テロップは通常表記（`caption` / `subtitle` 列）。

### ナレーション原稿（timeline.tsv の speech 列の写し。改稿は tsv 側で行う）

<!-- narration:begin -->
（`scripts/promo/narrate.sh --print` 相当。ここは合成後に実測秒つきで埋める）
<!-- narration:end -->

## 訴求と裏取り

「存在しない機能を言わない」ための表。主張ごとに参照先を 1 つ以上置く。

| 章 | 主張 | 裏取り |
|---|---|---|
| 1 | AI エージェント利用で 1 作業が「本体 + 子エージェント + dev サーバー + ログ」に分裂しタブ / ウインドウに散らばる | `README.md`「なぜ tako?」 |
| 1 | AI に操作させるには tmux / スクリプト / MCP 設定を自前で繋ぎ込む必要があった | `docs/src/content/docs/features/orchestration.md`「なぜターミナルに組み込むのか」 |
| 1, 2 | 1 グループ = 1 タブ。AI が起こした子プロセスのペインは同じタブに生える | `README.md`「なぜ tako?」/ `docs/.../features/tabs-and-panes.md`「AI からの操作」（呼び出し元と同じタブに生成） |
| 2 | 内蔵 MCP サーバー。初回登録だけで以後どのプロジェクトでも設定不要 | `docs/.../features/mcp-server.md`「仕組み」/ `README.md`「Claude Code 連携」 |
| 2 | 公開ツールは 140 個以上 | `crates/tako-control/testdata/mcp_tools_full_snapshot.json`（2026-09-03 時点で 144 ツール。docs の「128 個」は古い） |
| 2 | UI でできることはすべて CLI / MCP からもできる（不変条件） | `AGENTS.md`「機能実装時の必須ルール」設計原則 5 / `docs/.../features/mcp-server.md`「設計思想: AI フルコントロール」 |
| 2 | テーマ / 表示モード / パネルがコマンドで切り替わる | `tako theme` / `tako ui-mode` / `tako panel`（`tako --help`。収録で実演） |
| 3 | Homebrew 1 行でアプリと CLI が入る。zip 手動も可 | `README.md`「インストール」/ `docs/.../getting-started/index.md`「1. インストール」 |
| 3 | Claude 未導入でも `tako setup` から始められる（インストール → PATH → ログインの 3 段。実行前に計画を表示） | `AGENTS.md` コマンド表「自動セットアップ」（#868 / #1057）/ `tako setup bootstrap install --dry-run`（収録で実演） |
| 3 | 認証済み CLI が 1 つなら質問ゼロ。claude / codex / agy を検出、プラン別プロファイル生成、MCP 登録 | `docs/.../getting-started/index.md`「3. tako setup」/ `docs/.../getting-started/quickstart.md` |
| 3 | 検出後に対話アシスタントが自動起動。現状を読んでから答え、反映は同意した項目だけ | `crates/tako-cli/src/setup.rs`（`launch_setup_agent`）/ `resources/setup/system-prompt.md`（#470 台本の「③ setup」節に精査記録） |
| 4 | Cmd+D / Cmd+Shift+D の分割、境界ドラッグ、ペイン並べ替え | `docs/.../features/tabs-and-panes.md` |
| 4 | Cmd+B でファイルツリー。タブ内の cwd を自動でワークスペース表示 | `docs/.../features/file-preview.md`「ファイルツリー」 |
| 4 | md レンダリング / ライブリロード / PDF（選択・目次・ズーム）/ 画像 / 210+ 形式のハイライト / 軽い編集 / Code Runner | `docs/.../features/file-preview.md` / `AGENTS.md` コマンド表（#233 / #453 / #124 / #126） |
| 5 | `tako master` で今いるペインが司令塔になる。日本語で話しかけるだけ | `docs/.../features/orchestrator.md`「tako master で何が起きるか」 |
| 5 | 依頼を成果物ごとに分解（1 worker = 1 成果物）。指示は型で、送達確認つき | `docs/.../features/orchestration.md`「品質は仕組みで作り込まれる」/「監視と回収まで自動で起きる」 |
| 5 | worker は同じタブにグリッド配置。master の取り分を保つ | `AGENTS.md` コマンド表「worker spawn のレイアウト設定」（#165）/ `docs/.../features/orchestration.md` |
| 5 | 右パネル orch ビューで親子関係・稼働時間・ctx 使用率を俯瞰 | `docs/.../features/orchestration.md`（orch-panel-detail の説明）/ `tako panel --view orch` |
| 5 | かんたん表示: Claude 対話ペインがチャット画面、空ペインがボタン。表示だけの切替 | `docs/.../features/gui-mode.md` |
| 5 | 完了・入力待ち・消滅を自動検知。証拠を検査してから報告。片付けも master | `docs/.../features/orchestration.md` / `docs/.../features/orchestrator.md`「仕組みの補足」 |
| 5 | `tako solo` は worker を立てず 1 対 1 | `docs/.../features/orchestration.md`「1 対 1 で十分なら tako solo」 |
| 6 | 全ペインを tmux セッション経由で動かし、再起動で実行中プロセス・画面内容・タブ構成を復元。ペイン ID も維持 | `docs/.../features/tmux-backend.md` / `README.md`「セッション永続化」 |
| 7 | `tako remote` は tailnet 内だけの固定 URL。WireGuard で端から端まで暗号化。公開インターネットに存在しない | `docs/.../features/remote.md`「一言でいうと」「層① Tailscale identity」 |
| 7 | Mac 画面で承認するまで端末は何も見られない（機器ペアリング） | `docs/.../features/remote.md`「層② 機器ペアリング」 |
| 7 | エージェントの会話を読める。承認カードにその場で答えられる（Interact 以上） | `docs/.../features/remote.md`「スマホでの見え方」 |
| 7 | プロファイルの opt-in で `--remote-control` を付け、claude.ai / モバイルアプリから会話を操作できる。既定 OFF（transcript が Anthropic のサーバーにも保存） | `AGENTS.md` コマンド表「スマホから会話を操作する（Claude 公式 Remote Control。#1068 / #1069）」/ `tako orchestrator profiles set --help` |
| 8 | Windows 版はインストーラー + ポータブル zip を同じリリースに同梱（v0.7.9 以降） | `docs/.../getting-started/index.md`「方法 C: Windows」/ `AGENTS.md`「両 OS 同時リリース（#965）」 |
| 8 | 対応状況は実機で確かめたものだけを「対応」と書く。docs の表は自動生成 | `docs/.../windows-support.md`（生成物）/ `AGENTS.md`「プラットフォーム対応マトリクス（#515 / #591）」 |
| 8 | GPL-3.0-or-later のオープンソース。GitHub で公開 | `LICENSE` / `Cargo.toml` の `license` / `README.md` |
| 8 | ドキュメントサイト tako-docs.pages.dev | `README.md` 冒頭のリンク |

### 言わなかったこと（裏が取れない・実装中・過大になる）

- リモート刷新の後続（PWA からの「Claude で開く」/ スマホから master 起動 / リモートファイル / SSH 切替 = #1077〜#1080）
- Windows でのリモート（`tako remote` の tailscale serve は Windows 実機で未測 = #971）や自動 SSH 検知（Windows は argv が採れない）
- agy の利用上限自動復帰（agy は窓つき上限を持たない = Unsupported）
- Remote Control の実機画面（スマホ側の claude.ai アプリは収録対象外。CLI での opt-in だけを見せる）

## 収録パイプライン

| スクリプト | 役割 |
|---|---|
| `scripts/promo/lib.sh` | 隔離起動・デモ環境・ウインドウ単体キャプチャ（#470）+ **16:9 ウインドウ seed / ビート記録 / 追加素材**（#1081） |
| `scripts/promo/record-explainer.sh <scene\|all>` | 9 シーンの収録。CLI 操作の瞬間を `<scene>-beats.tsv` へ記録 |
| `scripts/promo/record-pwa.cjs` | PWA（`web/tako-remote`）を iPhone ビューポートでモック API つきに撮る（連番スクショ → mp4） |
| `scripts/promo/narrate.sh` | timeline.tsv の speech 列 → `say -v Kyoko` → 48kHz wav + durations.tsv |
| `scripts/promo/make-bgm.py` | BGM 合成（`TAKO_BGM_TOTAL=660 TAKO_BGM_PROFILE=explainer` で薄い長尺版） |
| `scripts/promo/titlecard.swift` / `caption.swift` | 章カード（全面）/ 下段テロップ（半透明パネル） |
| `scripts/promo/build-explainer.sh` | 切り出し → カード / テロップ → 連結 → ナレーション配置 → BGM ダッキング → mp4 + 章タイムスタンプ |
| `scripts/promo/ocr-frames.swift` / `pii-scan.sh` | 全フレーム Vision OCR → PII パターン検査 |
| `scripts/promo/thumbnail.swift` | サムネイル PNG（1280x720） |

実行順:

```sh
/Applications/tako.app を最新に（scripts/build-app.sh --install）
cd web/tako-remote && npx vite --port 5199 --strictPort &      # PWA 用 dev サーバー
scripts/promo/record-explainer.sh all                           # 9 シーン（実 claude を使う 3 つは後半）
NODE_PATH=web/tako-remote/node_modules node scripts/promo/record-pwa.cjs
scripts/promo/narrate.sh
TAKO_BGM_TOTAL=660 TAKO_BGM_PROFILE=explainer scripts/promo/make-bgm.py ~/Desktop/tako-promo/audio/bgm-explainer.wav
scripts/promo/build-explainer.sh                                # → ~/Desktop/tako-promo/tako-explainer-v1.mp4
scripts/promo/pii-scan.sh ~/Desktop/tako-promo/tako-explainer-v1.mp4
```

### #470 から引き継いだ罠と、今回わかったこと

- #470 の技術制約（`screencapture -v` は黒 / 画面全体を撮らずウインドウ単体 / 隠れると描画停止 →
  定期 activate + 異なるフレーム数検査 / デモ HOME はログインキーチェーンが外れる → 検索リストに
  実ユーザーのキーチェーンを指定 / `tko` に TAKO_DATA_DIR / `--await-prompt` は生成中を中断しうる）
  はすべて有効。詳細は `.agent/plans/2026-07-promo-video.md`
- **16:9 ウインドウ**: tako-app は初回起動時に `layout.json` の `window` フレームを読む
  （`TAKO_SELF_TEST` 以外）。タブが空のレイアウトは復元段で「空」として拒否され新規ワークスペース
  になるので、隔離 data_dir に `{"version":1,"active_tab":0,"tabs":[],"window":{...960x540...}}` を
  置くだけで 960x540pt（Retina 1920x1080px）で開く（`promo_seed_window_frame`）
- **claude 2.1.258 の起動画面にメールアドレスは出ない**（デモ HOME で実測。出るのはプラン名・
  cwd・Remote Control の案内 `/rc active`）。#470 v3 当時の「バナーにメールが出る」は現行では
  該当しないが、`promo_wait_pii_clear` の検査は残してある
- **Playwright の recordVideo は iPhone エミュレーションと組むとページが左上 1/4 に描かれる**
  （実測）。連番スクリーンショット（4 fps・3x）→ ffmpeg の方が確実で鮮明
- PWA の term ビューは WebSocket の画面プッシュ前提なので、モックでは読み込み中のまま。撮らない
- 文字サイズはペイン既定の 13 では 1080p で小さいので `tako theme --size 15` で撮る
- `say -v Kyoko` の `-r` は 160 と 175 で尺が変わらなかった（実測）。180 で使う

## 付随物（YouTube）

### タイトル案（3 つ）

1. tako — AI エージェント時代の GUI ターミナル【Claude Code を 1 つのタブで動かす】
2. Claude Code の並列作業を 1 画面で監視する OSS ターミナル「tako」を解説
3. AI に開発を任せるためのターミナル tako｜導入から master / worker の使い方まで 10 分で

### 説明文（章立てタイムスタンプ付き）

<!-- description:begin -->
（合成後に `~/Desktop/tako-promo/tako-explainer-chapters.txt` の実測値で埋める）
<!-- description:end -->

### サムネイル案

- 案 A: 完成形（master + worker 3 体 + orch パネル）を背景に「AI エージェントを / 1 つのタブで動かす」
- 案 B: かんたん表示（チャット画面）を背景に「Claude Code の司令塔を / ターミナルに」
- 生成: `thumbnail.swift`（背景フレーム + 2 行見出し + 小ラベル）。出力 `~/Desktop/tako-promo/tako-explainer-thumb-{a,b}.png`
