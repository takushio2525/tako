---
title: キーボードショートカット
description: tako で使えるキーボードショートカット一覧
---

tako のキーボードショートカットは iTerm2 に近い操作体系です。

## タブ操作

| ショートカット | 操作 |
|---|---|
| <kbd>Cmd</kbd>+<kbd>T</kbd> | 新しいタブを作成 |
| <kbd>Cmd</kbd>+<kbd>W</kbd> | 現在のペインを閉じる（最後のペインならタブごと） |
| <kbd>Cmd</kbd>+<kbd>1</kbd>〜<kbd>9</kbd> | タブを番号で切替 |
| <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>[</kbd> | 前のタブへ |
| <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>]</kbd> | 次のタブへ |

## ペイン操作

| ショートカット | 操作 |
|---|---|
| <kbd>Cmd</kbd>+<kbd>D</kbd> | 右にペイン分割 |
| <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd> | 下にペイン分割 |
| <kbd>Cmd</kbd>+<kbd>Alt</kbd>+<kbd>←</kbd> | 左のペインへフォーカス移動 |
| <kbd>Cmd</kbd>+<kbd>Alt</kbd>+<kbd>→</kbd> | 右のペインへフォーカス移動 |
| <kbd>Cmd</kbd>+<kbd>Alt</kbd>+<kbd>↑</kbd> | 上のペインへフォーカス移動 |
| <kbd>Cmd</kbd>+<kbd>Alt</kbd>+<kbd>↓</kbd> | 下のペインへフォーカス移動 |

## リサイズ

| ショートカット | 操作 |
|---|---|
| <kbd>Ctrl</kbd>+<kbd>Cmd</kbd>+<kbd>←</kbd> | ペインを左に広げる |
| <kbd>Ctrl</kbd>+<kbd>Cmd</kbd>+<kbd>→</kbd> | ペインを右に広げる |
| <kbd>Ctrl</kbd>+<kbd>Cmd</kbd>+<kbd>↑</kbd> | ペインを上に広げる |
| <kbd>Ctrl</kbd>+<kbd>Cmd</kbd>+<kbd>↓</kbd> | ペインを下に広げる |

## サイドバー

| ショートカット | 操作 |
|---|---|
| <kbd>Cmd</kbd>+<kbd>B</kbd> | ファイルツリー（左サイドバー）の表示/非表示 |

## テキスト操作

| ショートカット | 操作 |
|---|---|
| <kbd>Cmd</kbd>+<kbd>C</kbd> | 選択テキストをコピー（選択なしの場合は Ctrl+C をペインへ送信） |
| <kbd>Cmd</kbd>+<kbd>V</kbd> | ペースト（ブラケットペースト対応） |
| <kbd>Cmd</kbd>+<kbd>A</kbd> | 全選択 |

## 表示

| ショートカット | 操作 |
|---|---|
| <kbd>Cmd</kbd>+<kbd>=</kbd> / <kbd>Cmd</kbd>+<kbd>+</kbd> | 文字サイズを拡大 |
| <kbd>Cmd</kbd>+<kbd>-</kbd> | 文字サイズを縮小 |
| <kbd>Cmd</kbd>+<kbd>0</kbd> | 文字サイズをリセット |

## アプリ

| ショートカット | 操作 |
|---|---|
| <kbd>Cmd</kbd>+<kbd>O</kbd> | ディレクトリを開く |
| <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>O</kbd> | リポジトリを開く |
| <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>N</kbd> | 新規ウィンドウ |
| <kbd>Cmd</kbd>+<kbd>Q</kbd> | tako を終了（tmux バックエンド有効時はプロセスは保持される） |

## マウス操作

| 操作 | 効果 |
|---|---|
| ペイン境界線をドラッグ | リサイズ |
| ペインタイトルバーをドラッグ | ペインの位置を移動（D&D） |
| タブをドラッグ | タブの並び替え |
| テキスト選択 | 自動コピー（copy-on-select） |
| ファイルツリーからペインへドラッグ | パス入力 / プレビュー表示 |
