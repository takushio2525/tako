---
title: エラーテレメトリ
description: クラッシュ時のエラーレポート自動送信 — 何が送られ、何が送られないか
---

tako には、クラッシュ（panic / 致命的エラー）の情報を自動送信して品質改善に役立てる仕組みがあります。**既定では無効**で、あなたが明示的に有効にしたときだけ動きます。

## 何が送られるか

有効にしている場合、クラッシュ時に送られるのは次の情報だけです。

| 項目 | 内容 | 例 |
|---|---|---|
| `version` | tako のバージョン | `0.6.0` |
| `os_version` | OS のバージョン | `macOS 26.0 (Darwin 25.2.0)` |
| `error_kind` | エラーの種別 | `panic`（クラッシュ）/ `restore_failed`（起動時のレイアウト復元の失敗） |
| `message` | エラーメッセージ（パスはマスク済み） | `index out of bounds at ~/src/main.rs:42` |
| `backtrace` | スタックトレース（パスはマスク済み。`panic` のときだけ） | `~/src/...` |

このほか、通信の性質上、あなたの **IP アドレス**が受け取り側（Cloudflare）に届きます。受け取り側は IP アドレスを連続送信の制限（1 分あたり 10 件）の判定にだけ使い、約 2 分で自動的に消えます。レポートの本体には保存しません。

## 何が送られないか

- 画面の内容・ターミナルの出力・入力したテキスト
- 現在の作業ディレクトリ
- ユーザー名・ホスト名・メールアドレス
- ファイルの中身・コマンド履歴
- その他あらゆる個人を特定できる情報

エラーメッセージとスタックトレースに含まれるパスは、送信前にすべてマスクされます。

- `/Users/<名前>/...` → `~/...`
- `/home/<名前>/...` → `/home/<user>/...`
- `/var/folders/<id>/<id>/...` → `/var/folders/<tmp>/...`

## 有効にする / 無効にする

```bash
tako telemetry status   # 現在の状態
tako telemetry on       # 有効化
tako telemetry off      # 無効化
```

MCP ツール `tako_telemetry` と、設定画面の「一般 → エラーレポート」からも同じ操作ができます。`tako setup` は有効にしません（既定の無効のまま）。

## 送った内容は自分で確認できます

送信されたレポートは、すべてローカルにも記録されます（`<データディレクトリ>/telemetry.log`）。**何が送られたか（あるいは有効にしていたら何が送られていたか）を、あとから自分の目で確認できます。** `tako telemetry status` にログファイルのパスと件数が表示されます。

## データの取り扱い

| 項目 | 内容 |
|---|---|
| 保存先 | Cloudflare Workers KV |
| 保持期間 | 90 日（自動削除） |
| 閲覧できる人 | プロジェクトのオーナーのみ |
| 書き込み口 | レート制限あり（10 リクエスト/分/IP）、認証不要 |
| 読み出し口 | 管理者トークンが必要（バイナリには含まれていません） |
| 受け付ける種別 | 現在の受け取り側が保存するのは `panic` だけです。`restore_failed` は受け付けずに捨てるため、送信に失敗した扱いでローカルの送信待ち（`<データディレクトリ>/telemetry_queue.jsonl`、最大 50 件）に残ります |

## 削除の依頼

送信済みレポートの削除依頼や、テレメトリのデータに関する質問は [GitHub Issues](https://github.com/takushio2525/tako/issues) からお願いします。

## ソースコード

送受信の実装はどちらも公開されています。

- 収集エンドポイント（Worker）: [`web/tako-error-collector/`](https://github.com/takushio2525/tako/tree/main/web/tako-error-collector)
- Rust 側のクライアント: [`crates/tako-control/src/telemetry.rs`](https://github.com/takushio2525/tako/tree/main/crates/tako-control/src/telemetry.rs)
