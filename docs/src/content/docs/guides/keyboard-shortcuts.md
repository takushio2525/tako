---
title: キーボードショートカット
description: tako で使えるキーボードショートカット一覧（macOS / Windows・Linux 併記）
---

tako のキーボードショートカットは macOS では iTerm2 に近い操作体系です。

Windows / Linux では **<kbd>Cmd</kbd> をそのまま <kbd>Ctrl</kbd> に読み替えることはできません**。
<kbd>Ctrl</kbd>+英字 の多くは端末が使う制御コード（<kbd>Ctrl</kbd>+<kbd>C</kbd> = 中断、
<kbd>Ctrl</kbd>+<kbd>D</kbd> = EOF、<kbd>Ctrl</kbd>+<kbd>Z</kbd> = 中断シグナルなど）で、
これを tako が奪うとシェルやエージェント CLI がその操作をできなくなってしまいます。
そのため衝突するものは Windows Terminal や VS Code の慣習に合わせて
<kbd>Ctrl</kbd>+<kbd>Shift</kbd> 段へ逃がしています。

:::tip[キーを忘れたら]
コマンドパレット（macOS: <kbd>Cmd</kbd>+<kbd>K</kbd> / Windows・Linux:
<kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd>）を開くと、主な操作とその
ショートカットが一覧で出ます。表示されるキーは実行中のプラットフォームのものです。
:::

## タブ操作

| 操作 | macOS | Windows / Linux |
|---|---|---|
| 新しいタブを作成 | <kbd>Cmd</kbd>+<kbd>T</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>T</kbd> |
| 現在のペインを閉じる（最後のペインならタブごと） | <kbd>Cmd</kbd>+<kbd>W</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>W</kbd> |
| タブを番号で切替 | <kbd>Cmd</kbd>+<kbd>1</kbd>〜<kbd>9</kbd> | <kbd>Ctrl</kbd>+<kbd>1</kbd>〜<kbd>9</kbd> |
| 前のタブへ | <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>[</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Tab</kbd> |
| 次のタブへ | <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>]</kbd> | <kbd>Ctrl</kbd>+<kbd>Tab</kbd> |

## ペイン操作

| 操作 | macOS | Windows / Linux |
|---|---|---|
| 右にペイン分割 | <kbd>Cmd</kbd>+<kbd>D</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd> |
| 下にペイン分割 | <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>E</kbd> |
| 左のペインへフォーカス移動 | <kbd>Cmd</kbd>+<kbd>Alt</kbd>+<kbd>←</kbd> | <kbd>Alt</kbd>+<kbd>←</kbd> |
| 右のペインへフォーカス移動 | <kbd>Cmd</kbd>+<kbd>Alt</kbd>+<kbd>→</kbd> | <kbd>Alt</kbd>+<kbd>→</kbd> |
| 上のペインへフォーカス移動 | <kbd>Cmd</kbd>+<kbd>Alt</kbd>+<kbd>↑</kbd> | <kbd>Alt</kbd>+<kbd>↑</kbd> |
| 下のペインへフォーカス移動 | <kbd>Cmd</kbd>+<kbd>Alt</kbd>+<kbd>↓</kbd> | <kbd>Alt</kbd>+<kbd>↓</kbd> |

:::note[Windows で分割が <kbd>Ctrl</kbd>+<kbd>D</kbd> ではない理由]
<kbd>Ctrl</kbd>+<kbd>D</kbd> は端末の **EOF**（入力の終端）です。Claude Code や codex の終了、
Python・Node の REPL の終了、`cat > file` の入力終端に使うため、tako がこれを分割に奪うと
代わりの手段が無くなります。<kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd> は EOF と同じバイトを
送るので、分割に使っても <kbd>Ctrl</kbd>+<kbd>D</kbd> 側の EOF は無傷です。
下方向が <kbd>E</kbd> なのは <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd> が右方向で埋まるためで、
Terminator / Tilix も分割に <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>E</kbd> を使います。
:::

## リサイズ

| 操作 | macOS | Windows / Linux |
|---|---|---|
| ペインの幅を広げる | <kbd>Ctrl</kbd>+<kbd>Cmd</kbd>+<kbd>→</kbd> | <kbd>Alt</kbd>+<kbd>Shift</kbd>+<kbd>→</kbd> |
| ペインの幅を狭める | <kbd>Ctrl</kbd>+<kbd>Cmd</kbd>+<kbd>←</kbd> | <kbd>Alt</kbd>+<kbd>Shift</kbd>+<kbd>←</kbd> |
| ペインの高さを広げる | <kbd>Ctrl</kbd>+<kbd>Cmd</kbd>+<kbd>↓</kbd> | <kbd>Alt</kbd>+<kbd>Shift</kbd>+<kbd>↓</kbd> |
| ペインの高さを狭める | <kbd>Ctrl</kbd>+<kbd>Cmd</kbd>+<kbd>↑</kbd> | <kbd>Alt</kbd>+<kbd>Shift</kbd>+<kbd>↑</kbd> |

## サイドバー・パネル

| 操作 | macOS | Windows / Linux |
|---|---|---|
| ファイルツリー（左サイドバー）の表示/非表示 | <kbd>Cmd</kbd>+<kbd>B</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>B</kbd> |
| コマンドパレットを開く | <kbd>Cmd</kbd>+<kbd>K</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>K</kbd> |

## テキスト操作

| 操作 | macOS | Windows / Linux |
|---|---|---|
| 選択テキストをコピー（選択なしの場合は Ctrl+C をペインへ送信） | <kbd>Cmd</kbd>+<kbd>C</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>C</kbd> |
| ペースト（ブラケットペースト対応） | <kbd>Cmd</kbd>+<kbd>V</kbd> | <kbd>Ctrl</kbd>+<kbd>V</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>V</kbd> / <kbd>Shift</kbd>+<kbd>Insert</kbd> |
| 全選択 | <kbd>Cmd</kbd>+<kbd>A</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>A</kbd> |

## 表示

| 操作 | macOS | Windows / Linux |
|---|---|---|
| 文字サイズを拡大 | <kbd>Cmd</kbd>+<kbd>=</kbd> / <kbd>Cmd</kbd>+<kbd>+</kbd> | <kbd>Ctrl</kbd>+<kbd>=</kbd> / <kbd>Ctrl</kbd>+<kbd>+</kbd> |
| 文字サイズを縮小 | <kbd>Cmd</kbd>+<kbd>-</kbd> | <kbd>Ctrl</kbd>+<kbd>-</kbd> |
| 文字サイズをリセット | <kbd>Cmd</kbd>+<kbd>0</kbd> | <kbd>Ctrl</kbd>+<kbd>0</kbd> |
| 全画面表示の切替 | <kbd>Ctrl</kbd>+<kbd>Cmd</kbd>+<kbd>F</kbd> | <kbd>F11</kbd> |

## プレビュー（編集・検索）

| 操作 | macOS | Windows / Linux |
|---|---|---|
| 保存 | <kbd>Cmd</kbd>+<kbd>S</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>S</kbd> |
| 検索 | <kbd>Cmd</kbd>+<kbd>F</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>F</kbd> |
| 取り消し | <kbd>Cmd</kbd>+<kbd>Z</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Z</kbd> |
| やり直し | <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>Z</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Y</kbd> |

## アプリ

| 操作 | macOS | Windows / Linux |
|---|---|---|
| ディレクトリを開く | <kbd>Cmd</kbd>+<kbd>O</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>O</kbd> |
| リポジトリを開く | <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>O</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>R</kbd> |
| 新規ウィンドウ | <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>N</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>N</kbd> |
| 設定を開く | <kbd>Cmd</kbd>+<kbd>,</kbd> | <kbd>Ctrl</kbd>+<kbd>,</kbd> |
| tako を終了（永続バックエンド有効時はプロセスは保持される） | <kbd>Cmd</kbd>+<kbd>Q</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Q</kbd> |

macOS 固有の「アプリを隠す」（<kbd>Cmd</kbd>+<kbd>H</kbd>）・「他を隠す」
（<kbd>Cmd</kbd>+<kbd>Alt</kbd>+<kbd>H</kbd>）・「最小化」（<kbd>Cmd</kbd>+<kbd>M</kbd>）は
Windows / Linux には対応する概念が無いため割り当てていません。Windows の最小化・最大化は
タイトルバー右上のウィンドウコントロールと <kbd>Win</kbd>+<kbd>↓</kbd> を使ってください。

## マウス操作

| 操作 | 効果 |
|---|---|
| ペイン境界線をドラッグ | リサイズ |
| ペインタイトルバーをドラッグ | ペインの位置を移動（D&D） |
| タブをドラッグ | タブの並び替え |
| テキスト選択 | 自動コピー（copy-on-select） |
| ファイルツリーからペインへドラッグ | パス入力 / プレビュー表示 |
