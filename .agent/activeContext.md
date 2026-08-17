# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-17、#835 = Finder の「このアプリケーションで開く」で新しいタブ）

- ブランチ `feat/835-open-with`（worktree `~/dev/tako-wt-835`）
- #708 の続き。Finder から選んだものが**新しいタブ**で開くようにした

## やったこと

#708 は受け口（`application:openURLs:` → dispatch）まで作ってあったが、開く先が
**アクティブタブのプレビュー 1 枚の再利用**だったので、複数選択すると最後の 1 枚しか
残らず「選んでも何も起きない」に見えていた（旧挙動へ戻すとセルフテスト 116 が
`3->4` で落ちることを実測 = 3 ファイルが 1 ペインに潰れる）。

開き方を種別で振り分ける形へ:

| 渡されたもの | 動作 |
|---|---|
| ファイル（宣言外の形式も） | 新しいタブ 1 枚をそのファイル専用のプレビューに（PTY なし・タブ名 = ファイル名） |
| フォルダ | 新しいタブでそのフォルダにシェルを起動 |
| 存在しないパス | 警告して読み飛ばす |

複数選択は **1 ファイル = 1 タブ**（最後に開いたものが前に出る）。既存のタブ・ペインには
一切触らない。新しい操作系は作らず既存 dispatch を 2 本拡張した:
`OpenFile { new_tab }`（= `tako open --new-tab` / MCP `new_tab`）と
`TabNew { cwd }`（= `tako tab new --cwd` / MCP `cwd`）。MCP のツール数は不変。

## 副観点（LaunchServices の重複）

Finder に tako が 2 つ（0.7.2 / 0.7.1）出るのは **`~/dev/tako/dist/tako.app`（0.7.2 =
build-app.sh の生成物）が LS に自動登録されている**ため。0.7.1 は `/Applications`。
掃除方法は PR / Issue に記載（自動掃除はしない）。

## 次の一手

- PR（`Closes #835`）→ macOS CI 緑 → squash merge → `scripts/build-app.sh --install`
- install は他タスクと重ねない（`build-app.sh` 同時実行禁止）

## 現フェーズで Read すべき設計書

- `.agent/requirements.md` FR-3.22 = 何をどう開くかの正（宣言する UTI・`LSHandlerRank`
  Alternate 固定の理由・復元と競合しない理由）
- `crates/tako-app/src/open_files.rs` の冒頭 doc = 経路とタイミングの説明
