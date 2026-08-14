# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-15、v0.7.0 安定版リリース = 公開済み）

- `main` 直（`20ef2a1 [リリース] v0.7.0`）+ annotated tag `v0.7.0`。CI は macOS / Windows とも緑
- v0.6.0（2026-07-27）以来の安定版。間の v0.6.1〜v0.6.11 は全部 Pre-release（テスト版）
  だったので、**その 11 本ぶんをまとめて安定版チャンネルへ出したのが今回**

## 公開したもの

- GitHub Release `v0.7.0` = **Latest（安定版・prerelease フラグなし）**。
  アセットは `tako-v0.7.0-macos-arm64.zip`（17,580,848 B）。Windows 版は無し（設計どおり
  Known limitations 節も出ない）
- homebrew cask `takushio2525/homebrew-tako` を 0.7.0 へ（`2651bf2`）。sha256 は
  公開アセットを実ダウンロードして算出 → `brew fetch` で検証済み
- `/Applications/tako.app` = 0.7.0（`build-app.sh --install`）。**本番 GUI（pid 54739）は
  再起動していない**ので、反映はユーザーが再起動したとき

## CHANGELOG の構造（次に安定版を出す人へ）

- 夜間リリースは新しい節を**ファイル先頭へ差し込む**ので、手書きの `## [Unreleased]` は
  そのまま取り残されて中腹に埋もれる。今回も 2 ブロック（#749/#725/#739/#779/#745/#746 と
  #513/#614/#619）が化石化していた → `[0.7.0]` へ畳んで削除した
- **`[0.6.x]` の夜間節は残す**（実タグに対応する履歴なので）。安定版節はその上に積む
  = v0.6.0 のときと同じ形
- `release.sh` は `## [X.Y.Z]` から**次の `## [`** までを切り出す。節を分断しないこと

## 漏れゼロの確認方法（今回使った機械検査）

`git log v0.6.0..HEAD` の実質コミット（`[リリース]` / `[ドキュメント]` を除く）56 件に対し、
件名の `#N` のどれか 1 つが `[0.7.0]` 節に現れるかを総当たり → 未カバー 0 件。
節が参照する Issue 番号は 63 種。スキップした doc コミットが本当に
`.agent/` / docs / CHANGELOG しか触っていないことも別途確認した

## 踏み抜いた罠

- **`TAKO_ISOLATED=1` だけでは CLI の宛先は隔離されない**。隔離インスタンスを立てても
  `tako update check` は本番 GUI（0.6.11）へ届き `available: true` を返した。
  `TAKO_SOCKET` / `TAKO_TOKEN` を**明示**して初めて隔離側（0.7.0 → `available: false`）に
  当たる。socket は `lsof -p <pid>` で `…/tako-iso-data-<pid>/tako.sock` として拾える
- 隔離インスタンスの終了は **pid 指定の SIGTERM**（#770 の事故以来、キーストローク送出は禁止）

## 次の一手

- ユーザーが tako を再起動すると 0.7.0 が反映される（アップデート通知カードからでも可）
- 夜間リリースは正常判定へ復帰済み（`--dry-run` = `SKIP: 変更なし（v0.7.0 == origin/main）`）
- **未確認**: docs サイトのライブ反映。デプロイ先 URL がリポジトリに記録されていない
  （`ci.yml` に Pages ジョブ無し / wrangler 設定無し / README にも URL 無し）。
  ソースは commit 済みで `npm run build` は 24 ページ緑

## 現フェーズで Read すべき設計書

- リリース手順: `AGENTS.md`「リリース運用」/ `scripts/release.sh`（ノート生成は #594）/
  `scripts/nightly-release.sh`（夜間の判定条件）
- ノートのプラットフォーム表記規約: `.agent/conventions.md`「CHANGELOG / リリースノート」
