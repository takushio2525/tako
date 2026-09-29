---
title: tmux バックエンド
seoTitle: tmux バックエンド — tako を閉じても AI エージェントとプロセスが消えない
description: 全ペインを tmux セッションの上で動かし、tako を閉じても実行中のプロセス・画面内容・タブ構成をそのまま復元します。tmux が無い環境での動き、既存の tmux セッションの取り込みと掃除も説明します
---

tako は全ペインの PTY を **tmux セッション経由**で管理します。これにより、tako を閉じて再起動しても、実行中のプロセス・画面内容・タブ構成がそのまま復元されます。

## 何が嬉しいのか

- **エージェントの長時間タスク中に tako を閉じても安全**。裏の tmux セッションがプロセスを保持し続ける
- 再起動すると、閉じた時点のタブ・ペイン構成がそっくりそのまま復元される
- 画面の出力内容（スクロールバック）も保たれる
- AI にとっての `TAKO_PANE_ID` も再起動をまたいで有効。操作が途切れない

## 仕組み

```
tako（GUI）─── attach ──→ tmux session（バックエンド）─── PTY ──→ シェル
```

各ペインは `tmux new-session` で tmux セッションを作成し、そこに attach して画面を描画します。tako を閉じると attach が外れるだけで、tmux セッション（= シェルとプロセス）は生き続けます。次回の起動時に `layout.json`（タブ・ペイン構成を記録したファイル）をもとに同じ構成で再 attach します。

## tmux がない場合

tmux がインストールされていない環境では、従来の直接 PTY 生成にフォールバックします。この場合、tako を閉じるとプロセスも終了し、再起動復元は使えません（タブ構成と作業ディレクトリだけは復元されます）。

tmux のインストールは必須ではありませんが、**[リモートアクセス](/features/remote/)とオーケストレーターの worker 管理には tmux が必要**です。これらは tako 本体とは別プロセスからペインへ到達する必要があるためです。

```bash
# macOS で tmux をインストール
brew install tmux
```

## 永続化の ON/OFF

tmux バックエンドは設定で制御できます。

```bash
# tmux バックエンドを無効化
tako persist off

# 有効化（デフォルト）
tako persist on
```

MCP ツール `tako_persist` からも同じ操作が可能です。

## 自分で作った tmux セッションを取り込む

tako の外で `tmux new` して作ったセッション（たとえば AI エージェントを動かしているもの）も、tako のペインとして開けます（attach を前提にする操作なので、psmux を使う Windows ではまだ使えません。[Windows 対応状況](/windows-support/)）。

```bash
tako tmux list               # 既定の tmux サーバーと tako のセッションの一覧
tako tmux open api --right   # 「api」セッションを今のタブの右側のペインへ取り込む
```

tmux で AI エージェントを動かすときの基本（外から画面を読む・指示を送るときの注意）は [tmux で AI エージェントを動かす](/guides/tmux-agents/)にまとめています。

<figure class="tako-shot">
<img src="/img/fleet-panel.webp" alt="fleet ビューにワークスペースとペインの一覧が表示され、稼働中・アイドルの件数が出ている画面" />
<figcaption>fleet ビューでは、tako が管理しているセッションと、その中のペインをまとめて確認できる</figcaption>
</figure>

## サイドバーの fleet ビュー

右サイドバーの **fleet ビュー**（`tako panel --view fleet`）では、全 tmux セッションの状態を一覧できます。

- **タブ枠ごと**にペインを入れ子表示
- 前面表示中 / バックグラウンドの**状態バッジ**
- バックグラウンドのペインを**ホバーでプレビュー**
- **orphan セッション**（tako から切り離されて残った tmux）の検出と一括クリーンアップ

```bash
# tmux セッション一覧を取得
tako tmux list

# orphan セッションの掃除
tako tmux cleanup
```
