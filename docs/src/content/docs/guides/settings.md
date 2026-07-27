---
title: 設定とカスタマイズ
description: テーマ・表示言語・入力予測・スリープ防止など、tako の挙動を自分好みに調整する
---

tako の設定は、**設定画面・CLI・MCP の 3 経路すべてから同じことができます**。マウスで変えても、コマンドで変えても、AI に頼んでも結果は同じです。

## 設定画面を開く

<kbd>Cmd</kbd>+<kbd>,</kbd>、<kbd>Cmd</kbd>+<kbd>K</kbd> のコマンドパレット、または次のコマンドで開きます。独立したウィンドウで、一般 / 外観 / Code Runner / セットアップ / スリープ防止 / リモート / 高度 の 7 タブに分かれています。

```bash
tako settings
tako settings --tab 外観
```

## 見た目

### テーマ

ダーク / ライトを切り替えます。タブバー右のボタンからも変えられます。設定は保存され、次回起動時も維持されます。

```bash
tako theme          # 現在のテーマ
tako theme dark
tako theme light
tako theme toggle
```

配色そのものも変更できます。色キーの一覧は `tako theme colors` で確認でき、気に入った配色はプリセットとして保存できます。

```bash
tako theme colors
tako theme preset save mytheme
```

### 表示言語

UI の表示言語を日本語 / 英語で切り替えます。既定は OS のロケールに追従します。

```bash
tako lang           # 現在の言語
tako lang ja
tako lang en
tako lang system    # OS のロケールに追従
```

### サイドバーとパネル

```bash
tako panel --filetree on      # 左のファイルツリー（Cmd+B と同じ）
tako panel --show --view git  # 右パネルを git ビューで開く
tako panel --width 360
tako panel --show-hidden on   # ツリーに .git などのドット項目も並べる（既定 off）
```

## 入力まわり

### 入力予測（ゴーストテキスト）

コマンドを打ち始めると、履歴から続きが薄い文字で予測表示されます。**既定は ON**、確定は <kbd>→</kbd> または <kbd>Tab</kbd> です。最初の 10 回だけ確定キーの案内が出て、慣れた頃に自動で消えます。

```bash
tako autosuggest             # 現在状態
tako autosuggest off         # 予測そのものを止める
tako autosuggest tab off     # Tab 確定だけ無効化（→ は残る）
tako autosuggest hint off    # 案内を今すぐ止める
```

:::note[tako の外は何も変わりません]
この機能が効くのは **tako が開いたシェルの中だけ**です。`~/.zshrc` は書き換えないので、Terminal.app や iTerm2 の挙動は一切変わりません。すでに自分で zsh-autosuggestions を導入している場合、tako は二重に読み込まず何もしません。
:::

### 閉じる確認

× ボタンや <kbd>Cmd</kbd>+<kbd>W</kbd> でペインを閉じるときの確認ダイアログです。確認が入るのは**エージェントや実行中プロセスがあるペインだけ**なので、普通のシェルは今までどおり即座に閉じます。

```bash
tako confirm-close          # 現在状態
tako confirm-close on
tako confirm-close off
```

## 自動で動くもの

### タブ・ペインの自動リネーム

作業内容に応じて、AI がタブ名を自動で付けます。手動で付けた名前が上書きされることはありません。自動で付いた名前が気に入ったら、ピン印のワンクリックでそのまま固定できます。

```bash
tako autorename on
tako autorename off
```

### ポート検知

ペイン内のプロセスが TCP ポートを listen し始めると、「プレビューを開く？」の提案チップが出ます。**勝手にペインを分割することはありません**（提案のみ）。

```bash
tako portdetect on
tako portdetect off
```

### セッション永続化

tmux バックエンドによる復元です。有効だと、tako を閉じて再起動しても実行中プロセスと画面内容が戻ります。詳しくは [tmux バックエンド](/features/tmux-backend/)へ。

```bash
tako persist        # 現在状態と診断情報
tako persist on
tako persist off
```

### スリープ防止

エージェントに長時間タスクを任せているあいだ、Mac がスリープして作業が止まるのを防ぎます。

```bash
tako sleep-guard status
tako sleep-guard set --mode while-agents-running --power-condition ac-only
```

| `--mode` | 挙動 |
|---|---|
| `off` | 何もしない |
| `on` | 常にスリープを防ぐ |
| `while-agents-running` | エージェントが動いている間だけ防ぐ |

`--power-condition` は `ac-only`（電源接続時のみ）と `always` から選べます。

## プライバシー

### エラーレポートの自動送信（テレメトリ）

**既定は OFF** です。有効にすると、クラッシュ時のエラー情報だけが送られます。画面の内容・入力テキスト・作業ディレクトリ・ユーザー名などは送られず、パスはマスクされます。詳しくは[エラーテレメトリ](/features/telemetry/)へ。

```bash
tako telemetry status
tako telemetry on
tako telemetry off
```

### フルディスクアクセス（FDA）

macOS では、tako の中で動くエージェントが iCloud Drive などに触れるたびに許可ダイアログが出ることがあります。FDA を付与すると一括で出なくなります。

```bash
tako fda status
tako fda open      # システム設定の該当画面を開く
```

## この環境で何が使えるか

macOS / Windows で対応状況が違う機能があります。今いる環境で何が使えて、何が縮退していて、何が未実装かを一覧できます。

```bash
tako platform
tako platform --status pending    # 未実装のものだけ
tako platform --json
```

## 初回起動バナー

初回だけ、タブバーの下に `tako setup` / `tako master` への案内バナーが出ます。消したあとでも呼び戻せます。

```bash
tako welcome           # 表示状態
tako welcome show
tako welcome dismiss
```

## AI に頼んで変える

ここまでのすべては、tako 内の AI エージェントに日本語で頼んでも変えられます。

> 「ダークテーマにして、入力予測は切って」

対応する MCP ツール（`tako_theme` / `tako_autosuggest` など）が呼ばれ、設定画面から操作したのと同じ結果になります。設定ファイルを手で編集する必要はありません。

## 関連ページ

- [CLI リファレンス](/guides/cli-reference/#表示設定のトグル) — 各コマンドの全オプション
- [MCP ツール一覧](/guides/mcp-tools/#表示設定) — AI から操作するときのツール名
