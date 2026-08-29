---
title: git 連携
description: 変更のステージング・コミット・ブランチ操作・コンフリクト解消までをターミナルから出ずに
---

右サイドバーの **git ビュー**で、現在のリポジトリの状態を見て、そのまま操作できます。`git status` を打って確認して…という往復をしなくて済むのが狙いです。

ステータスバーの git ボタン、または `tako panel --show --view git` で開きます。

<figure class="tako-shot">
<img src="/img/git-panel.webp" alt="右パネルに git ビューが開き、変更ファイル・コミット履歴・diff が表示されている画面" />
<figcaption>右パネルの git ビュー。ステージングからコミット・ブランチ操作まで、ここで完結する</figcaption>
</figure>

## 見えるもの・できること

git ビューは上から順に並んだセクションで構成されます。

### 変更ファイル

ステージ済み / 未ステージの 2 セクションに分かれ、行の **+ / − ボタン**で個別にステージ・アンステージできます。まとめて操作するボタンもあります。ファイル名をクリックすると、その場でプレビューペインに開きます。

### コミット

メッセージを入力して commit します（<kbd>Cmd</kbd>+<kbd>Enter</kbd> でも実行）。日本語入力（IME）にも対応しています。空メッセージや変更ゼロの状態では理由付きで止まるので、うっかり空コミットができることはありません。

### ブランチ

現在のブランチと一覧を表示します。切替・作成・マージがここから行えます（後述）。

### リモート

push / pull を実行します。

### コミット履歴と diff

コミットの一覧を表示し、クリックするとその**直下**に詳細（メタ情報と変更ファイル一覧）が開きます。ファイルを選べば差分が読めます。追加行は緑、削除行は赤で表示されます。

<figure class="tako-shot tako-shot-narrow">
<img src="/img/git-panel-detail.webp" alt="git パネルの拡大。ブランチ名・コミットメッセージ入力欄・Pull / Push・変更ファイル・コミット履歴・diff が並んでいる" />
<figcaption>上から順に、ブランチ / コミットメッセージ / 変更ファイル / コミット履歴 / diff</figcaption>
</figure>

## 破壊的な操作は「予行演習」から

ブランチ切替とマージは、**既定では実行しません**。まず「何が起きるか」を出します。

- **切替**: 未コミットの変更があるとき、そのまま持ち越せるものと、切替を妨げるものを分けて提示します
- **マージ**: `git merge-tree` を使い、**作業ツリーに一切触れずに**マージ種別・取り込むコミット数・発生しそうなコンフリクトを事前に計算して見せます

内容に納得してから実行を選びます。CLI では `--yes` が実行の合図です。

```bash
tako git checkout main          # 何が起きるかを出すだけ
tako git checkout main --yes    # 実行する

tako git merge feature/x        # コンフリクトを予測して出すだけ
tako git merge feature/x --yes  # 実行する
```

## コンフリクトを AI に解かせる

マージがコンフリクトすると、git ビューに**コンフリクトカード**が出ます。進行中の操作・マージ元 / 先・未解決ファイルが一覧され、中止もそこからできます。

「解消エージェントを起動」を選ぶと、**同じタブに AI のペインが立ち上がり**、リポジトリ・未解決ファイル・マージの向きを含んだ解消用のプロンプトが自動で投入されます。

```bash
tako git conflicts              # 未解決の状態を JSON で確認
tako git resolve                # 解消エージェントを起動（claude / codex / agy）
tako git resolve --agent codex
tako git abort                  # merge / rebase / cherry-pick / revert を中止
```

投入されるプロンプトの文面は、`<データディレクトリ>/orchestrator/conflict-resolver.md` を置けば差し替えられます。

## 表示の追従

git ビューは 2 秒間隔で自動更新され、パネルを開いた瞬間にも即時取得します。

表示対象のリポジトリは、**ファイルツリーが表示しているリポジトリに追従**します。タブ内に複数のプロジェクトがある場合も、ツリーで見ているものと git ビューがずれることはありません。

## CLI / MCP から

GUI でできることは、すべて CLI と MCP からも同じようにできます（[開発の不変条件](/development/architecture/#設計原則)）。

```bash
# 読む
tako git log
tako git diff --target staged
tako git show a1b2c3d --file src/main.rs

# 記録する
tako git stage src/main.rs
tako git commit -m "[修正] ログイン失敗を直す"
tako git push

# ブランチ
tako git branch fix/login --from main
tako git merge feature/x --yes
```

MCP ツールは `tako_git_log` / `tako_git_diff` / `tako_git_show` / `tako_git_stage` / `tako_git_unstage` / `tako_git_commit` / `tako_git_push` / `tako_git_pull` / `tako_git_checkout` / `tako_git_branch_create` / `tako_git_merge` / `tako_git_merge_abort` / `tako_git_conflicts` / `tako_git_resolve_agent` の 14 個です。AI が「今の変更をレビューして、問題なければコミットして」といった作業を最後まで実行できます。

## ファイルツリーの色分け

サイドバーのファイルツリーでも、変更のあるファイルが色とバッジで分かります。フォルダには配下の変更件数が伝播するので、git ビューを開かなくても「どこに未コミットが残っているか」が見えます。詳しくは [ファイルツリー＆プレビュー](/features/file-preview/#git-の状態色とバッジ) を参照してください。

## 関連ページ

- [CLI リファレンス](/guides/cli-reference/#git) — `tako git` の全コマンド
- [ファイルツリー＆プレビュー](/features/file-preview/) — ツリーの git 色分け・チェンジログビューで履歴を読む
