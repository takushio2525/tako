---
title: キーボードショートカット
description: tako で使えるキーボードショートカット一覧（iTerm2 に近い操作体系）
---

tako のキーボードショートカットは iTerm2 に近い操作体系です。

## タブ

| ショートカット | 操作 |
|---|---|
| <kbd>Cmd</kbd>+<kbd>T</kbd> | 新しいタブを作成 |
| <kbd>Cmd</kbd>+<kbd>W</kbd> | 現在のペインを閉じる（最後のペインならタブごと） |
| <kbd>Cmd</kbd>+<kbd>1</kbd>〜<kbd>9</kbd> | タブを番号で切替 |
| <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>[</kbd> | 前のタブへ |
| <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>]</kbd> | 次のタブへ |

:::note[閉じる前に確認が入ることがあります]
エージェントや実行中のプロセスがあるペインを閉じるときは確認ダイアログが出ます。普通のシェルはそのまま閉じます（`tako confirm-close off` で無効化できます）。
:::

## ペイン

| ショートカット | 操作 |
|---|---|
| <kbd>Cmd</kbd>+<kbd>D</kbd> | 右にペイン分割 |
| <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd> | 下にペイン分割 |
| <kbd>Cmd</kbd>+<kbd>Alt</kbd>+<kbd>←</kbd> / <kbd>→</kbd> | 左 / 右のペインへフォーカス移動 |
| <kbd>Cmd</kbd>+<kbd>Alt</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | 上 / 下のペインへフォーカス移動 |

## リサイズ

| ショートカット | 操作 |
|---|---|
| <kbd>Ctrl</kbd>+<kbd>Cmd</kbd>+<kbd>←</kbd> | ペインを左に広げる |
| <kbd>Ctrl</kbd>+<kbd>Cmd</kbd>+<kbd>→</kbd> | ペインを右に広げる |
| <kbd>Ctrl</kbd>+<kbd>Cmd</kbd>+<kbd>↑</kbd> | ペインを上に広げる |
| <kbd>Ctrl</kbd>+<kbd>Cmd</kbd>+<kbd>↓</kbd> | ペインを下に広げる |

## コマンドパレット・設定

| ショートカット | 操作 |
|---|---|
| <kbd>Cmd</kbd>+<kbd>K</kbd> | コマンドパレットを開く |
| <kbd>Cmd</kbd>+<kbd>,</kbd> | 設定画面を開く |
| <kbd>Cmd</kbd>+<kbd>B</kbd> | ファイルツリー（左サイドバー）の表示 / 非表示 |

:::tip[まず <kbd>Cmd</kbd>+<kbd>K</kbd>]
やりたいことの名前を覚えていなくても、コマンドパレットから探せます。セットアップの実行や master の起動も、ここから直接行えます。
:::

## テキスト操作

| ショートカット | 操作 |
|---|---|
| <kbd>Cmd</kbd>+<kbd>C</kbd> | 選択テキストをコピー（選択なしの場合は Ctrl+C をペインへ送信） |
| <kbd>Cmd</kbd>+<kbd>V</kbd> | ペースト（ブラケットペースト対応） |
| <kbd>Cmd</kbd>+<kbd>A</kbd> | 全選択 |
| <kbd>→</kbd> または <kbd>Tab</kbd> | 入力予測（ゴーストテキスト）を確定 |

## プレビューの操作

プレビューペインにフォーカスがあるときに使えます。

| ショートカット | 操作 |
|---|---|
| <kbd>Cmd</kbd>+<kbd>S</kbd> | 編集内容を保存 |
| <kbd>Cmd</kbd>+<kbd>F</kbd> | プレビュー内を検索 |
| <kbd>Cmd</kbd>+<kbd>Z</kbd> | 編集を元に戻す |
| <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>Z</kbd> | 編集をやり直す |

## 表示

| ショートカット | 操作 |
|---|---|
| <kbd>Cmd</kbd>+<kbd>=</kbd> / <kbd>Cmd</kbd>+<kbd>+</kbd> | 文字サイズを拡大 |
| <kbd>Cmd</kbd>+<kbd>-</kbd> | 文字サイズを縮小 |
| <kbd>Cmd</kbd>+<kbd>0</kbd> | 文字サイズをリセット |
| <kbd>Ctrl</kbd>+<kbd>Cmd</kbd>+<kbd>F</kbd> | フルスクリーン切替 |

## ウィンドウ・アプリ

| ショートカット | 操作 |
|---|---|
| <kbd>Cmd</kbd>+<kbd>O</kbd> | ディレクトリを開く |
| <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>O</kbd> | リポジトリを開く |
| <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>N</kbd> | 新規ウィンドウ |
| <kbd>Cmd</kbd>+<kbd>M</kbd> | ウィンドウを最小化 |
| <kbd>Cmd</kbd>+<kbd>H</kbd> | tako を隠す |
| <kbd>Cmd</kbd>+<kbd>Alt</kbd>+<kbd>H</kbd> | ほかのアプリを隠す |
| <kbd>Cmd</kbd>+<kbd>Q</kbd> | tako を終了（tmux バックエンド有効時はプロセスは保持される） |

## マウス操作

| 操作 | 効果 |
|---|---|
| ペイン境界線をドラッグ | リサイズ |
| ペインタイトルバーをドラッグ | ペインの位置を移動（D&D） |
| タブをドラッグ | タブの並び替え（挿入位置がバーで表示される） |
| テキスト選択 | 自動コピー（copy-on-select） |
| <kbd>Cmd</kbd>+クリック | URL・ファイルパスを開く（ホバーで下線が出ます） |
| ファイルツリーからペインへドラッグ | パス入力 / プレビュー表示 |
