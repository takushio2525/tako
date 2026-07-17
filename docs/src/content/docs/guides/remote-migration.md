---
title: リモート接続の移行ガイド（v0.5 → v0.6）
description: Quick Tunnel / Cloudflare relay から Tailscale Serve への移行手順
---

## 何が変わったか

v0.6.0 でリモート接続の transport を **Tailscale Serve** に一本化しました。

| 項目 | v0.5 以前（廃止） | v0.6 以降 |
|---|---|---|
| 接続経路 | Cloudflare Quick Tunnel（公開 URL） | Tailscale Serve（tailnet 内限定） |
| 暗号化 | TLS（Cloudflare が終端 = 平文を見られる） | WireGuard E2E（経路上の第三者が読めない） |
| URL | 毎回変わる | 恒久固定（`https://<mac>.<tailnet>.ts.net`） |
| 認証 | URL 埋め込みトークン | 二層（Tailscale identity + 機器ペアリング） |
| PWA 配信 | Cloudflare Pages（公開） | daemon 内蔵（同一 origin） |

## 移行手順

### 1. Tailscale をセットアップ

```bash
tako remote setup
```

対話ウィザードが Tailscale の導入・ログイン・HTTPS 有効化・serve 設定を順に案内します。

### 2. スマホ側

1. スマホに Tailscale アプリをインストール
2. Mac と同じアカウントでログイン
3. QR コード（またはウィザード末尾に表示される URL）を開く
4. Mac 画面のペアリング承認ダイアログで「許可」

### 3. 旧設定のクリーンアップ（任意）

旧方式の設定ファイルは自動で無視されますが、気になる場合は以下を手動で削除できます。

- `~/Library/Application Support/tako/remote/` 内の旧ファイル:
  - `tako-remote.pid` / `tako-remote.port` / `tako-remote.tunnel` — 旧 daemon の状態ファイル
  - `tako-remote.token` — 旧 bearer token（v0.6 では使われない）

### 4. Cloudflare 資産の停止（開発者向け）

自分で Cloudflare Workers / Pages をデプロイしていた場合:

1. **tako-remote Pages プロジェクト**: Cloudflare ダッシュボードで `tako-remote` Pages プロジェクトを削除
   （カスタムドメインを設定していた場合は先にドメインを外す）
2. **tako-remote-worker Worker**: Cloudflare ダッシュボードで `tako-remote-worker` Worker を削除
   （Workers KV のバインディングも自動で外れる）
3. **tako-error-collector Worker は削除しない**: テレメトリ用の別機能として現役

```bash
# wrangler CLI での削除（要認証。ダッシュボード操作でも可）
wrangler pages project delete tako-remote
wrangler delete tako-remote-worker
```

## よくある質問

**Q: 旧方式の URL（trycloudflare.com / tako-remote.pages.dev）はまだ使える？**
A: いいえ。v0.6 ではこれらの経路は完全に削除されています。

**Q: Tailscale のアカウントは無料？**
A: 個人利用は無料プランで十分です（最大 100 ノード）。

**Q: CT log にホスト名が載るのが気になる**
A: Tailscale が TLS 証明書を取得する際に `<mac名>.<tailnet>.ts.net` が公開ログに
記録されます。これは Tailscale の仕様です。ホスト名にセンシティブな情報を含めないことを
推奨します。
