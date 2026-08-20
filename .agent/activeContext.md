# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-21、#837 = dist/tako.app の LS 重複登録）

- ブランチ `fix/837-ls-duplicate`（worktree `~/dev/tako-wt-837`）
- `scripts/build-app.sh` の生成物が Launch Services に登録され、Finder の
  「このアプリケーションで開く」に tako が 2 つ並ぶ問題の恒久対策

## 実測（Issue の対策案 3 つはいずれも成立しない）

macOS 26 / Darwin 25.4 で使い捨ての .app を置いて測った（証拠は Issue コメント）:

- **案 1（`lsregister -u`）は持続しない**: ファイルを一切触らなくても**約 40 秒後に
  自動で再登録**される（LS は Spotlight とは別に自力でディスク上の .app を拾う）
- **案 2（`dist.noindex/`）は効かない**: 親を `*.noindex` にしても、
  `.metadata_never_index` を置いても、`chflags hidden` を立てても登録される。
  ホーム配下は置き場所を変えても無効（ドット始まりの隠しディレクトリも 60 秒で登録された）
- **案 3（bundle id 変更）は症状を消さない**: 別 id でも `CFBundleName` が tako なので
  候補には 2 つ並ぶ。加えて DR 固定（#54）と release.sh の配布物が壊れる
- **効くのは「実体を消す + `-u`」の両方だけ**。実体を消しただけでは登録が残骸として残り、
  存在しないパスは再登録されないので `-u` が恒久的に効く

## 直し方

`--install` は「/Applications へ配置 → `-f` 登録 → **ビルド出力を削除** → LS に残る
tako.app のうち**実体が無いものを `-u`**（このビルド出力・他 worktree の残骸）→ 実体が
残る他の tako.app は消さずに掃除手順を提示」。`--install` を付けないビルドは配布物として
出力を残すので、片付け方をメモ表示する。番犬テスト（open_files.rs）が build-app.sh を
読んで「削除 → 登録解除」の順序ごと固定する。

## 次の一手

- 受け入れ検証（番犬テスト → `--verify` → `--install` の通しと LS の before/after）の完走確認
- PR（`Closes #837`）→ macOS CI 緑 → squash merge

## 現フェーズで Read すべき設計書

- `scripts/build-app.sh` 冒頭 +「Launch Services（Issue #837）」節 = 実測値と不変条件の正
- `crates/tako-app/src/open_files.rs` の番犬テスト 2 本（#708 の Alternate 固定 / #837 の後始末）
