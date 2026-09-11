---
title: キーボードショートカット
description: tako で使えるキーボードショートカット一覧（macOS / Windows 両対応）
---

tako のキーボードショートカットは iTerm2 に近い操作体系です。macOS と Windows でキーが違うので、各表に両方を載せています。

:::note[なぜ Windows だけキーが違うのか]
Windows では Command キーにあたる修飾が Win キーになり、多くの組み合わせを OS が先に奪います。押しても tako まで届かないので、Windows 版は <kbd>Ctrl</kbd>+<kbd>Shift</kbd> 段を中心に割り当て直してあります。
:::

表の `/` は「どちらを押しても同じ」という意味です。

## タブ

| 操作 | macOS | Windows |
|---|---|---|
| 新しいタブを作成 | <kbd>Cmd</kbd>+<kbd>T</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>T</kbd> |
| 現在のペインを閉じる（最後のペインならタブごと） | <kbd>Cmd</kbd>+<kbd>W</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>W</kbd> |
| タブを番号で切替 | <kbd>Cmd</kbd>+<kbd>1</kbd>〜<kbd>9</kbd> | <kbd>Ctrl</kbd>+<kbd>1</kbd>〜<kbd>9</kbd> |
| 前のタブへ | <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>[</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Tab</kbd> |
| 次のタブへ | <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>]</kbd> | <kbd>Ctrl</kbd>+<kbd>Tab</kbd> |

:::note[閉じる前に確認が入ることがあります]
エージェントや実行中のプロセスがあるペインを閉じるときは確認ダイアログが出ます。普通のシェルはそのまま閉じます（`tako confirm-close off` で無効化できます）。
:::

## ペイン

| 操作 | macOS | Windows |
|---|---|---|
| 右にペイン分割 | <kbd>Cmd</kbd>+<kbd>D</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd> |
| 下にペイン分割 | <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>E</kbd> |
| 左のペインへフォーカス移動 | <kbd>Cmd</kbd>+<kbd>Alt</kbd>+<kbd>←</kbd> | <kbd>Alt</kbd>+<kbd>←</kbd> |
| 右のペインへフォーカス移動 | <kbd>Cmd</kbd>+<kbd>Alt</kbd>+<kbd>→</kbd> | <kbd>Alt</kbd>+<kbd>→</kbd> |
| 上のペインへフォーカス移動 | <kbd>Cmd</kbd>+<kbd>Alt</kbd>+<kbd>↑</kbd> | <kbd>Alt</kbd>+<kbd>↑</kbd> |
| 下のペインへフォーカス移動 | <kbd>Cmd</kbd>+<kbd>Alt</kbd>+<kbd>↓</kbd> | <kbd>Alt</kbd>+<kbd>↓</kbd> |

## リサイズ

| 操作 | macOS | Windows |
|---|---|---|
| ペインを左に広げる | <kbd>Ctrl</kbd>+<kbd>Cmd</kbd>+<kbd>←</kbd> | <kbd>Alt</kbd>+<kbd>Shift</kbd>+<kbd>←</kbd> |
| ペインを右に広げる | <kbd>Ctrl</kbd>+<kbd>Cmd</kbd>+<kbd>→</kbd> | <kbd>Alt</kbd>+<kbd>Shift</kbd>+<kbd>→</kbd> |
| ペインを上に広げる | <kbd>Ctrl</kbd>+<kbd>Cmd</kbd>+<kbd>↑</kbd> | <kbd>Alt</kbd>+<kbd>Shift</kbd>+<kbd>↑</kbd> |
| ペインを下に広げる | <kbd>Ctrl</kbd>+<kbd>Cmd</kbd>+<kbd>↓</kbd> | <kbd>Alt</kbd>+<kbd>Shift</kbd>+<kbd>↓</kbd> |

## コマンドパレット・設定

| 操作 | macOS | Windows |
|---|---|---|
| コマンドパレットを開く | <kbd>Cmd</kbd>+<kbd>K</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>K</kbd> |
| 設定画面を開く | <kbd>Cmd</kbd>+<kbd>,</kbd> | <kbd>Ctrl</kbd>+<kbd>,</kbd> |
| ファイルツリー（左サイドバー）の表示 / 非表示 | <kbd>Cmd</kbd>+<kbd>B</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>B</kbd> |

:::tip[まずコマンドパレット]
やりたいことの名前を覚えていなくても、コマンドパレットから探せます（macOS は <kbd>Cmd</kbd>+<kbd>K</kbd>、Windows は <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd>）。セットアップの実行や master の起動も、ここから直接行えます。分割・新しいタブ・サイドバー・設定など主な項目には、その環境で実際に効く打鍵が併記されます。
:::

## テキスト操作

| 操作 | macOS | Windows |
|---|---|---|
| 選択テキストをコピー（選択が無いときは何も起きない） | <kbd>Cmd</kbd>+<kbd>C</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>C</kbd> |
| ペースト（ブラケットペースト対応） | <kbd>Cmd</kbd>+<kbd>V</kbd> | <kbd>Ctrl</kbd>+<kbd>V</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>V</kbd> / <kbd>Shift</kbd>+<kbd>Insert</kbd> |
| 全選択（プレビュー / チャット表示の本文。ターミナルの画面には効かない） | <kbd>Cmd</kbd>+<kbd>A</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>A</kbd> |
| 入力予測（ゴーストテキスト）を確定 | <kbd>→</kbd> / <kbd>Tab</kbd> | <kbd>→</kbd> / <kbd>Tab</kbd> |

:::note[コピーと「中断の Ctrl+C」は別のキー]
コピーの打鍵（macOS は <kbd>Cmd</kbd>+<kbd>C</kbd>、Windows は <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>C</kbd>）が効くのは、テキストを選択しているときだけです。選択が無いときは何も起きません（ペインへ打鍵が送られることもありません）。

ペインの中で走っているプログラムを中断する <kbd>Ctrl</kbd>+<kbd>C</kbd> は tako が横取りしないので、どちらの OS でもそのままペインへ届きます。
:::

:::note[全選択が効くのはプレビューとチャット表示です]
全選択の打鍵が選択するのは、プレビューペインの本文（編集中ならその編集テキスト）と、かんたん表示（GUI モード）のチャット本文です。ターミナルの画面には効かないので、押しても選択は作られず、続けてコピーの打鍵をしても何も入りません。

ターミナルの文字はマウスでドラッグして選んでください。選んだ時点でクリップボードへ入ります（下の「マウス操作」の copy-on-select）。
:::

## プレビューの操作

プレビューペインにフォーカスがあるときに使えます。

| 操作 | macOS | Windows |
|---|---|---|
| 編集内容を保存 | <kbd>Cmd</kbd>+<kbd>S</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>S</kbd> |
| プレビュー内を検索 | <kbd>Cmd</kbd>+<kbd>F</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>F</kbd> |
| 編集を元に戻す | <kbd>Cmd</kbd>+<kbd>Z</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Z</kbd> |
| 編集をやり直す | <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>Z</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Y</kbd> |

:::note[Windows の取り消し・やり直し]
ターミナルでは <kbd>Ctrl</kbd>+<kbd>Z</kbd>（プロセスの一時停止）を奪えないため、取り消しも <kbd>Shift</kbd> 段に置いています。やり直しは Windows 慣習どおり <kbd>Y</kbd> です。
:::

## 表示

| 操作 | macOS | Windows |
|---|---|---|
| 文字サイズを拡大 | <kbd>Cmd</kbd>+<kbd>=</kbd> / <kbd>Cmd</kbd>+<kbd>+</kbd> | <kbd>Ctrl</kbd>+<kbd>=</kbd> / <kbd>Ctrl</kbd>+<kbd>+</kbd> |
| 文字サイズを縮小 | <kbd>Cmd</kbd>+<kbd>-</kbd> | <kbd>Ctrl</kbd>+<kbd>-</kbd> |
| 文字サイズをリセット | <kbd>Cmd</kbd>+<kbd>0</kbd> | <kbd>Ctrl</kbd>+<kbd>0</kbd> |
| フルスクリーン切替 | <kbd>Ctrl</kbd>+<kbd>Cmd</kbd>+<kbd>F</kbd> | <kbd>F11</kbd> |

## ウィンドウ・アプリ

| 操作 | macOS | Windows |
|---|---|---|
| ディレクトリを開く | <kbd>Cmd</kbd>+<kbd>O</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>O</kbd> |
| リポジトリを開く | <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>O</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>R</kbd> |
| 新規ウィンドウ | <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>N</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>N</kbd> |
| ウィンドウを最小化 | <kbd>Cmd</kbd>+<kbd>M</kbd> | （macOS のみ） |
| tako を隠す | <kbd>Cmd</kbd>+<kbd>H</kbd> | （macOS のみ） |
| ほかのアプリを隠す | <kbd>Cmd</kbd>+<kbd>Alt</kbd>+<kbd>H</kbd> | （macOS のみ） |
| tako を終了（tmux バックエンド有効時はプロセスは保持される） | <kbd>Cmd</kbd>+<kbd>Q</kbd> | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Q</kbd> |

:::note[「隠す」「最小化」は macOS の概念です]
アプリを隠す・ほかを隠す・ウィンドウを最小化するの 3 つは macOS のウィンドウ管理に対応する操作で、Windows 版には該当するキーがありません。Windows でのウィンドウ操作は OS 標準の方法（タイトルバーの最小化ボタンなど）を使ってください。
:::

## マウス操作

| 操作 | 効果 |
|---|---|
| ペイン境界線をドラッグ | リサイズ |
| ペインタイトルバーをドラッグ | ペインの位置を移動（D&D） |
| タブをドラッグ | タブの並び替え（挿入位置がバーで表示される） |
| テキスト選択 | 自動コピー（copy-on-select） |
| <kbd>Cmd</kbd>+クリック | URL・ファイルパスを開く（ホバーで下線が出ます。**macOS のみ** — Windows では Win キーを OS が使うため効きません） |
| ファイルツリーからペインへドラッグ | パス入力 / プレビュー表示 |

Windows で画面上のパスを開きたいときは、`tako file open-in-tako <path>` や、リンクとして認識されている範囲を確認する `tako links` が使えます。
