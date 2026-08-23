# 設定・データファイルの自動マイグレーション（Issue #916）

> **原則（恒久・開発不変条件）**: 永続ファイルのスキーマや置き場を変えるときは自動移行を
> 同梱する。ユーザーや master へ手動の移行作業を要求してはならない（「移行手順を提示する」も不可）。
> 手順書は `.agent/conventions.md`「設定・データファイルのスキーマ変更」節が正本。
> ここには**なぜその形にしたか**と**棚卸しの実測**を残す。

## 層の分け方

| 層 | 置き場 | 中身 |
|---|---|---|
| 機構 | `tako-core::migration` | 版数の番地（`SchemaId`）/ 手順の型（`Step`）/ 並べ方（`plan`）/ ファイル駆動（`migrate_file`）/ 退避規約 / 結果（`MigrationReport`） |
| 登録簿 | `tako-control::migrations` | 18 種別の `SchemaSpec`（`detect` / `validate` / `steps`）/ 対象ファイルの解決 / 発火 / JSON |
| 発火 | tako-app / tako-cli / dispatch | GUI 起動・`tako master`・dispatch・`tako setup`・`tako migrate` |

**変換手順をテキスト → テキストの純粋関数にした**のは、tako-core が `serde_yaml` へ依存せずに
機構だけを持てるようにするため（YAML も JSON もテキスト）。実装側は `serde_*::Value` を
経由して未知のキーを落とさない。

## 設計判断（迷ったところ）

### 冪等性を「移行した記録」で作らない

sidecar（`schema-versions.json` 等）に「どこまで移行したか」を持つ案を捨てた。#513 の設定共有では
マシン A が移行して push すると、マシン B は「新形式のファイル + 古い記録」を持つので必ずズレる。
判定は**内容から**（`SchemaSpec::detect`）行い、`apply` は「もう当たっている」なら `Ok(None)` を返す。
これで共有経路でも二重適用が起きない。

### 一度だけの移行（`Step::once`）と印の持ち方

#27 の `[1m]` 除去は「移行後にユーザーが自分で `[1m]` を選び直したら尊重する」（#67）ので、
内容ベースの冪等性だけでは足りない。印は**退避ファイルの存在**（`backup_path`）で持つ:
状態ファイルを増やさず、「旧ファイルを消さない」原則とも噛み合う。
機構より前の手書き移行の印（`.backup-1m`）は `once_markers` で認める。

### 版数フィールドを持つ形式との付き合い方

`layout.json` / `task_checkpoints.yaml` / `acceptance_gates.yaml` / `control-*.json` は
`version` を持つ。前者は `LAYOUT_VERSION` と不一致なら**復元を拒否**する実装で、
後 2 つは**誰も読まない死に設定**だった。番地に載せて `detect_version_field` で読むようにしたので、
将来の bump は `Step` を足すだけで効く（拒否のままにならない）。

### Check モードで書かないことを型で担保する

`ReadOnlyIo::write` は必ず Err を返す。`migrate_file` の中で誤って書く実装が入ったら
セルフテスト項目 122 の `status_untouched` が落ちる。応答のキーも
`backup_planned` / `quarantine_planned` にして「退避済み」と読み違えないようにした
（AI が status の結果を「もう退避した」と報告する事故を避ける）。

### CLI をローカル処理にした理由

`tako migrate` は IPC を通さない。**壊れた設定で GUI が起動しないときの復旧手段**なので
GUI に依存しないことが本質（`tako recover` と同じ）。MCP は dispatch 経由で同じ
`report_json` を通るので 1:1 は保たれる。

## 棚卸しの調べ方（推測しない）

1. **git 履歴**: 永続構造体を持つ 10 ファイルの全コミットから削除フィールドを機械抽出
   （`git log --format=%h -- <file>` → 各 sha の diff から `^-\s+pub \w+:`）。ヒット 4 件を個別確認
2. **本番の実ファイル**（読み取りのみ）: 全 YAML / JSON のキーを走査して現行構造体と差分を取り、
   残骸の件数と最古 mtime を実測

結果の表は Issue #916 のコメントが正本。**発見の要点**:

- 「黙って既定値へ落ちる」= `unwrap_or_default()` / `.ok()` は、**直後の保存が利用者の内容を
  消す**ことと同義。被害の実例は `settings.json` の `theme_colors` / `theme_presets` /
  `runner_defaults` と `~/.claude.json` の MCP 登録・信頼済みフォルダ
- `instances/control-*.json` の残骸 160 件は**ソケットを固定パス化した副作用**
  （2026-06-23）。残骸の最古 mtime がその日と一致したのが決定的な証拠。
  「掃除の判断に使っている材料が、別の変更で意味を失っていた」型のバグ
- **テストが本番の設定を触っていた**（`_tako_822_set_.yaml`）。隔離が `OnceLock` の
  初期化ヘルパー任せで、そのヘルパーを通らないテストは本番へ書く。しかも**実行順で
  結果が変わる**ので気付きにくい

## 実機の作法（Windows）

- 新規 worktree は `web/tako-remote/dist/` を持たないので既存 worktree からコピーする
  （`rust_embed` が埋め込むので無いと即失敗）
- SSH から PowerShell へ `|` を含むパターンを渡すと壊れる。**スクリプトを scp して
  `-File` で実行する**のが確実
- 測定側のログ読みは `Get-Content -Encoding UTF8`。既定 cp932 だと persist.log が化けて
  「壊れた」と読み間違える（実際に一度踏んだ）
- 検証後は worktree を `git worktree remove --force` し、一時ディレクトリとスクリプトも消す

## 残した課題

- `remote.rs::is_process_alive`（unix 専用・非 Windows は常に false・ゾンビ判定込み）と
  `platform::process::pid_alive` が別物として併存する。統合すると remote daemon の
  Windows 挙動が変わるので別 Issue 向き
- 非 ASCII の往復は macOS でのみ実測（Windows の fixture は ASCII）。書き込みは
  同じ `config_io::atomic_write` を通る
- 本番のデータディレクトリでは実行していない（読み取りのみ）
