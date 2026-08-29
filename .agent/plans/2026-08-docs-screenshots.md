# docs のスクリーンショット拡充（#1027）

> docs（tako-docs.pages.dev）は文字ばかりで画像がほぼ無い。全ページに実画面を入れて
> 「見て分かる」サイトにする。**公開サイトなので PII 混入の防止が最優先**。

## 1. 棚卸し（before）

`docs/src/content/docs/` の全 25 ページを機械的に走査した結果、**実質的な画像は 1 枚だけ**
（トップのマスコット SVG）。スクリーンショットは **全ページで 0 枚**。

| 区分 | ページ数 | before の画像 |
|---|---|---|
| はじめに | 6 | 0（index.mdx のマスコット SVG のみ） |
| AI と使う | 3 | 0 |
| 機能紹介 | 9 | 0 |
| 使い方ガイド | 5 | 0 |
| 開発者向け | 2 | 0 |
| **合計** | **25** | **0 枚（マスコットを除く）** |

## 2. 構成表（after の計画）

「何を見せるか」を先に決めてから撮る。★ = Issue が「必ず実画面を見せる」と名指ししたページ。

| ページ | after | 何を見せるか |
|---|---|---|
| ★ features/gui-mode | 3 | スターター 3 ボタン / チャットビュー / 表示モード切替 |
| ★ features/tabs-and-panes | 3 | 複数タブのタブバー / 4 分割レイアウト / 複数ウィンドウ |
| ★ features/file-preview | 4 | ファイルツリー / コードのハイライト / md レンダリング / 表つき md |
| ★ features/git-integration | 2 | git パネル全体 / コミット詳細と diff |
| ★ features/remote | 2 | スマホの画面 / リモート状態のインジケータ |
| ★ features/tmux-backend | 2 | fleet パネル / 再起動後の復元 |
| ★ features/orchestration | 2 | master + worker が並ぶタブ / orch パネル |
| ★ features/orchestrator | 3 | master 起動直後 / worker への指示 / 検収 |
| ★ features/mcp-server | 2 | AI がペインを操作している様子 / ツール一覧 |
| features/shelving | 1 | たまり場ドロワー |
| features/port-detection | 1 | 検知チップ |
| features/telemetry | 1 | 設定画面のエラーレポート |
| getting-started/index | 3 | setup の実行 / 設定画面 / ウェルカムバナー |
| getting-started/quickstart | 3 | 3 ステップの各画面 |
| guides/settings | 3 | 一般 / 外観 / プロファイル |
| guides/keyboard-shortcuts | 1 | コマンドパレット |
| development/architecture | 1〜2 | 自作 SVG の構成図（3 層制御プレーン） |
| index.mdx | 1 | 全体像のヒーロー画像 |
| **合計** | **約 37 枚** | |

### 実績（2026-08-29 時点）

**22 図 / 12 ページ / 画像 17 枚（788 KB）**。内訳は
gui-mode 3 / file-preview 3 / tabs-and-panes 2 / git-integration 2 /
orchestration 2 / orchestrator 2 / settings 2 / quickstart 2 /
mcp-server 1 / tmux-backend 1 / port-detection 1 / getting-started 1。

**撮れなかったもの**（理由つき）:

- **リモートアクセス**: tailnet の実機が要る。実 URL・実ホスト名が写るので
  デモ環境では代替できない
- **たまり場のドロワー**: 開くのに実クリックが要る（合成クリックは GPUI に届かない）。
  退避したこと自体はステータスバーの `BG 1` に出る
- **コマンドパレット・右クリックメニュー**: 同上（キー入力・クリックが要る）
- **アーキテクチャ図**: 自作 SVG。スクショではないので別途

CLI リファレンス / MCP ツール一覧 / リリースノート / Windows 対応状況 /
エージェント別対応状況 は**一覧表そのものが主役**なので、画像は足さない（足すと
かえって読みにくくなる）。

## 3. 撮影環境（PII 対策）

**本番の tako・本番のホームには一切触れない。**

- 隔離インスタンス: `TAKO_ISOLATED=1` + 個別の隔離変数を固定パスで明示
  （discovery / data / tmux socket / sessions / pane log / workers / orchestrator）。
  CLI 側は `env -u TAKO_SOCKET -u TAKO_TOKEN` を必ず通す（本番 GUI を触らないため）
- **デモ用の HOME** を作り、そこに架空のプロジェクト（`my-app` / `api-server` / `notes`）を置く。
  HOME を差し替えるとプロンプトの `~` がデモ側を指すので、実ホームパスが画面に出ない
- デモ HOME の `.zshrc` で `PROMPT='%1~ ❯ '` にする（zsh の既定プロンプトは
  `ユーザー名@ホスト名` = PII そのもの）
- git のコミットは `demo <demo@example.com>` 名義で作る
- 撮影は **ウィンドウ単体**（`screencapture -l<windowID>`）。画面全体を撮らないので、
  他アプリのウィンドウが重なっても写り込まない（`scripts/promo/winbounds.swift` を流用）

## 4. PII 検査（二重）

1. **機械検査**: Vision の OCR で画像から文字を起こし、検出語（実ユーザー名 /
   実ホスト名 / ComputerName / `/Users/` / 実ホームパス / git の author 名）を全文検索する。
   検出語は**環境から組み立てる**（リポジトリに実値を書かない = #927 の番犬と同じ方針）
   - **OCR が 0 行のときは「検出なし」と言わない**（空振りを合格と読まない）
   - 検出力は PII 入りの合成画像で実証する（実際に 5 件を検出することを確認済み）
2. **目視**: 全枚数を 1 枚ずつ人（AI）が見る。機械検査は文字しか見ないので、
   画像として写り込むもの（顔・実在のロゴ等）は目で見るしかない

**機械検査だけでは足りないことが実際に起きた**。claude の起動ボックスに出る
実アカウントのメールアドレス（`<名前>NNNN@example.com` の形）は、検出語である
ユーザー名の部分文字列ではないので
素通りした（目視で発見）。対策として ①ユーザー名の**先頭 8 文字**も検出語に入れる
②メールアドレスを正規表現で拾う ③`Welcome back` の行を拾う、を足した。
それでも **OCR は `@` を `c` と読み違える**ことがあり（実測）、
正規表現が空振りする場合がある。**最後の砦は 1 枚ずつの目視**という前提を崩さないこと。

## 5. 実際に踏んだ落とし穴（再発防止）

- **`ls -la` / `ll` はオーナー列に実ユーザー名が出る**。デモ画面で使わない
  → `git status --short` / `git log --oneline` / `ls`（`-l` なし）を使う。
  これで 3 枚を撮り直しにした（機械検査が捕まえた）
- **persist の復元は古いシェルを生かしたまま戻す**。`.zshrc` を置いたら
  layout.json ごと消して起動し直さないと、旧プロンプト（`ユーザー名@ホスト名`）が残る
- **合成キー入力・合成クリックは GPUI に届かない**。ウィンドウを閉じる目的で
  `keystroke` を送ると、代わりに**ペインへ文字が混入**する（実際に混入した）
- `tako open` の相対パスは**呼び出した CLI プロセスの cwd** で解決される。
  デモを撮るときは絶対パスで渡す
- winbounds に渡す pid は `pgrep -x tako-app` で採る（`pgrep -f` だと
  起動用の bash ラッパーを掴んで座標が採れない）
- **claude の認証はキーチェーン依存で、`HOME` を差し替えると必ず落ちる**
  （`CLAUDE_CONFIG_DIR` を実物に向けても直らない）。AI の画面を撮るときは
  `HOME` を実物にしたインスタンスを別に立て、**ペインでは claude を直接動かす**
  （シェルを挟まない = 実ユーザー名入りのプロンプトが画面に出ない）。
  プロジェクトは `/private/tmp` 配下に置けばパスにもユーザー名は出ない
- **claude の起動ボックスには実アカウントのメールアドレスと表示名が出る**。
  会話が伸びて画面外へ流れるまでは撮らない（`/clear` では消えず再描画される）
- 送達確認つきの `tako send` は、まれに**未確定の IME 文字列や 1 文字**を
  ペインへ残す。撮る前に器（tmux）経由で `send-keys C-u` を打って行を空にする
