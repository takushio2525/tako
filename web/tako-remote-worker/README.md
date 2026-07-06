# tako-remote-relay

リモート接続機能（`tako remote start`）用の Cloudflare Workers KV リレー。
マシン ID → 最新 tunnel URL のマッピングを保存し、スマホ PWA が 2 回目以降も
QR コードの再スキャンなしで最新のトンネル URL を解決できるようにする。

## 公共インスタンスの位置づけ

tako に埋め込まれているデフォルト URL（`tako-remote-relay.takushio2525.workers.dev`）は、
作者が Cloudflare 無料枠で運用する**ベストエフォートの公共インスタンス**であり、SLA はない。
`TAKO_RELAY_URL` 環境変数で自前のインスタンスに差し替えられる（下記セルフホスト手順）。
リレーには tunnel URL と machineId しか保存されず、ターミナルの内容・トークンは通らない
（それらは cloudflared トンネルを通って tako 本体と直接やり取りされ、tako 側のトークン認証で保護される）。

## 登録の保護（first-write-wins）

`POST /api/register` は `secret`（hex 64 文字）を受け付ける:

- secret 付きで登録された machineId は、以後 **同じ secret でしか上書きできない**（不一致は 403）。
  secret の SHA-256 ハッシュだけを `secret:<machineId>` キーに TTL 30 日で保存し、登録のたびに延長する
- secret なしのレガシー登録は「secret 未登録の machineId」に限り許可（旧クライアント互換）。
  新しい tako クライアントは初回 `remote start` 時に `<data_dir>/relay_secret` を自動生成して送るため、
  一度でも新クライアントで登録すれば以後その machineId は保護される
- `GET /api/resolve/:machineId` は無認証のまま: machineId（UUID v4）自体が能力トークンであり、
  解決先の tako 本体には別途トークン認証がある

## セルフホスト / デプロイ

```bash
cd web/tako-remote-worker
npm ci
wrangler kv namespace create RELAY_KV            # 生成された id を wrangler.toml に記入
wrangler kv namespace create RELAY_KV --preview  # preview_id も同様
npm run deploy
```

tako 側は環境変数で差し替える:

```bash
TAKO_RELAY_URL=https://<your-worker>.workers.dev tako remote start
```

## 運用の推奨

- Cloudflare ダッシュボードで `/api/register` への**レートリミットルール**の設定を推奨
  （このコードはレートリミットを持たない。悪意ある大量登録は KV 書き込み無料枠を消費しうる）

## テスト

```bash
npm test   # Node 18+。wrangler 不要（KV をモックして fetch ハンドラを直接検証）
```
