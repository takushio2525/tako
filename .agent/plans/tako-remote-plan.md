# tako remote 実装計画

## 1. 概要

`tako remote start` コマンドで Mac 上の tako インスタンスへスマホからリモートアクセスする機能。
Mac 側で HTTP API サーバー + cloudflared Quick Tunnel を起動し、QR コードを表示。スマホは QR を
1 回スキャンするだけで接続でき、以降はブックマークから操作する。URL 変更問題は Cloudflare Workers KV
にマシン ID → 最新 tunnel URL のマッピングを保存して解決。スマホ向け PWA は Cloudflare Pages で
ホスティング（コスト $0）。

## 2. リポジトリ構造（関連部分のみ）

### 既存の接続点

- **CLI サブコマンド追加**: `crates/tako-cli/src/main.rs` の `enum Command` に variant を追加
  （clap derive。`Master` / `Orchestrator` 等と同パターン）
- **HTTP サーバー**: `crates/tako-control/src/mcp.rs` の `McpServer::start()` が tiny_http で
  `127.0.0.1:0` にバインドする既存パターン。remote 用サーバーもこれを踏襲
- **dispatch 共有**: `crates/tako-control/src/dispatch.rs` の `dispatch()` が全操作の一元窓口。
  MCP / IPC / CLI すべてがここを通る。remote API も同じ dispatch を呼ぶ
- **protocol**: `crates/tako-control/src/protocol.rs` の `enum Request` に操作を追加
- **MCP ツール**: `mcp.rs` の `tools()` / `handle_message()` がツール定義と実行。
  remote で公開する操作は `list_panes` / `read_pane` / `send_input` / `close_pane` 等、既存の
  dispatch Request をそのまま HTTP JSON API として公開する形

### 新規追加するもの

- `crates/tako-control/src/remote.rs` — remote HTTP API サーバー（Mac 側）
- `crates/tako-cli/src/main.rs` — `Remote` サブコマンド群
- `remote-pwa/` — スマホ向け PWA（Cloudflare Pages デプロイ）
- `workers/` — Cloudflare Workers（KV リレー）

## 3. アーキテクチャ

```
┌─────────────────────────────────────────────────────────────┐
│  Mac（tako アプリ）                                          │
│                                                             │
│  tako-app ── dispatch ─┬─ IPC (Unix socket)  ← tako CLI    │
│                        ├─ MCP (127.0.0.1:*)  ← Claude Code │
│                        └─ Remote API (:PORT)  ← NEW        │
│                             │                               │
│                        cloudflared tunnel                   │
│                             │                               │
└─────────────────┬───────────┘                               │
                  │ Quick Tunnel                              │
                  ▼                                           │
┌─────────────────────────────────────┐                       │
│  Cloudflare Edge                    │                       │
│                                     │                       │
│  *.trycloudflare.com ──proxy──► :PORT                       │
│                                     │                       │
│  Workers KV:                        │                       │
│    machine_id → latest_tunnel_url   │                       │
│                                     │                       │
│  Pages (PWA):                       │                       │
│    tako-remote.pages.dev            │                       │
│    └─ /connect?machine=XXXX         │                       │
│       → KV lookup → tunnel URL     │                       │
│       → WebSocket / REST で操作     │                       │
└─────────────────────────────────────┘
                  ▲
                  │ HTTPS
┌─────────────────┘
│  スマホ（ブラウザ / PWA）
│  - QR スキャン → PWA が開く
│  - ペイン一覧 / 画面読み取り / 入力送信
│  - ブックマークから再接続（KV 経由で URL 解決）
└──────────────────────────────────────
```

## 4. フェーズ分割

### Phase 1: 最小動作（Mac 側 HTTP API + QR）

**ゴール**: `tako remote start` で API サーバーが起動し、curl で操作できる

**成果物・ファイル一覧**:
- `crates/tako-control/src/remote.rs` — HTTP API サーバー（tiny_http）
- `crates/tako-control/src/protocol.rs` — `Request::RemoteStart` / `RemoteStop` / `RemoteStatus` 追加
- `crates/tako-control/src/dispatch.rs` — remote 操作のハンドラ
- `crates/tako-cli/src/main.rs` — `Remote` サブコマンド（`start` / `stop` / `status`）
- `crates/tako-control/src/mcp.rs` — `tako_remote_start` 等の MCP ツール追加

**やること**:
1. `remote.rs` に tiny_http ベースの HTTP JSON API サーバーを実装
   - エンドポイント: `GET /api/panes`、`GET /api/panes/:id/screen`、`POST /api/panes/:id/input`、
     `POST /api/panes/:id/close`、`GET /api/health`
   - 認証: Bearer トークン（`TAKO_TOKEN` と同じ仕組み。起動時に生成）
   - CORS ヘッダ付与（PWA からのアクセス用）
2. CLI `tako remote start [--port PORT]` → サーバー起動 + ターミナルに QR コード表示
   - QR は `qrcode` crate（テキスト出力）で URL を表示
   - `tako remote stop` → サーバー停止
   - `tako remote status` → 起動状態・URL・トークン表示
3. dispatch + MCP にも `RemoteStart` / `RemoteStop` / `RemoteStatus` を追加（開発不変条件）
4. Phase 1 時点では LAN 内アクセスのみ（cloudflared なし）

**依存追加候補**:
- `qrcode` crate（QR コードのテキスト生成。軽量・pure Rust）

### Phase 2: スマホ PWA（ペイン操作 UI）

**ゴール**: スマホブラウザでペイン一覧表示・画面閲覧・コマンド入力ができる

**成果物・ファイル一覧**:
- `remote-pwa/` — Vite + vanilla TS（or Preact）のモバイルファースト PWA
  - `index.html` / `src/main.ts` / `src/api.ts` / `src/components/`
  - `manifest.json`（PWA マニフェスト）
  - `vite.config.ts`
- `remote-pwa/wrangler.toml` — Cloudflare Pages 設定

**やること**:
1. モバイルファースト UI の実装
   - ペイン一覧画面（タブ → ペインの階層表示、状態ドット、タップでペイン詳細へ）
   - ペイン画面表示（`read_pane` の結果をモノスペースで表示、自動リフレッシュ）
   - 入力バー（下部固定、テキスト入力 + 送信ボタン + よく使うキーのショートカットバー）
   - ペイン操作（close / focus / split のボタン）
2. API クライアント層（`fetch` ベース、Bearer トークン付与）
3. PWA 化（`manifest.json` + service worker でオフラインシェル）
4. Cloudflare Pages へデプロイ（`wrangler pages deploy`）

**技術選定（Phase 2）**:
- フレームワーク: Preact（推奨）。React のエコシステム互換で 3kB。モバイル PWA には十分。
  vanilla TS でも可だがコンポーネント分割が煩雑になるため Preact を推奨
- ビルド: Vite（デファクト標準）
- CSS: Tailwind CSS（推奨）。モバイルファーストのユーティリティクラスが豊富

### Phase 3: cloudflared 統合 + Workers KV リレー

**ゴール**: LAN 外からスマホで接続でき、URL 変更後もブックマークが有効

**成果物・ファイル一覧**:
- `crates/tako-control/src/remote.rs` — cloudflared プロセス管理を追加
- `workers/kv-relay/` — Cloudflare Workers スクリプト（KV 読み書き）
  - `src/index.ts` / `wrangler.toml`
- `remote-pwa/src/connect.ts` — KV 経由の URL 解決ロジック

**やること**:
1. `tako remote start` に cloudflared Quick Tunnel 統合
   - `cloudflared tunnel --url http://127.0.0.1:PORT` を子プロセスで起動
   - stdout から tunnel URL をパース、QR に反映
   - `cloudflared` が PATH に無い場合のエラーメッセージ + インストール案内
   - `tako remote start --no-tunnel` で LAN のみモードも残す
2. マシン ID の生成・永続化（`~/.config/tako/machine_id`、UUID v4、初回起動時に生成）
3. Workers KV リレー
   - Worker: `PUT /relay/:machine_id` → KV に tunnel URL を書き込み（トークン認証）
   - Worker: `GET /relay/:machine_id` → KV から最新 URL を返す（認証不要）
   - tako 側: tunnel 起動後に Worker へ PUT で URL 登録
4. PWA の接続フロー更新
   - `https://tako-remote.pages.dev/connect?machine=XXXX`
   - KV から最新 tunnel URL を取得 → API サーバーへ接続
   - ブックマーク URL は `pages.dev/connect?machine=XXXX` で固定（tunnel URL が変わっても有効）

**依存追加候補**:
- `uuid` crate（マシン ID 生成）

### Phase 4: リッチ化（xterm.js、プッシュ通知、PWA 強化）

**ゴール**: ターミナル画面のリアルタイム描画 + 通知でネイティブアプリに近い体験

**成果物・ファイル一覧**:
- `crates/tako-control/src/remote.rs` — WebSocket エンドポイント追加
- `remote-pwa/src/components/Terminal.tsx` — xterm.js 統合
- `remote-pwa/src/push.ts` — Web Push 通知
- `workers/push/` — プッシュ通知の中継 Worker（任意）

**やること**:
1. WebSocket による画面ストリーミング
   - Mac 側: PTY 出力の差分を WebSocket で push（既存の `on_term_event` をフック）
   - PWA 側: xterm.js で受信・描画（フル ANSI / 256 色 / truecolor 対応）
   - キー入力も WebSocket 経由でリアルタイム送信
2. Web Push 通知
   - エージェントが入力待ち / エラー / 完了になったらスマホに通知
   - OSC 133 の状態変化をトリガーにする（既存の `CommandState` を活用）
   - VAPID キーは tako 側で生成・PWA に配布
3. PWA 強化
   - アプリアイコン / スプラッシュ / テーマカラー設定
   - ホーム画面追加で「アプリっぽい」体験
   - バックグラウンド→フォアグラウンド復帰時の自動再接続

**依存追加候補**:
- `tungstenite` crate（WebSocket サーバー。tokio 不要の同期版あり）

## 5. 技術選定

| 領域 | 選定 | 理由 |
|---|---|---|
| Mac 側 HTTP サーバー | tiny_http（既存） | MCP サーバーと同じパターン。tokio 不要の方針を維持 |
| QR コード生成 | `qrcode` crate | pure Rust、テキスト/SVG 出力、軽量 |
| PWA フレームワーク | Preact（推奨） | 3kB で React 互換。モバイル PWA に最適 |
| PWA ビルド | Vite | デファクト標準。HMR + 軽量 |
| CSS | Tailwind CSS | モバイルファースト、ユーティリティクラス |
| ターミナル描画（Phase 4） | xterm.js | ブラウザターミナルのデファクト標準 |
| WebSocket（Phase 4） | tungstenite | tokio 不要の同期 WebSocket。tiny_http と共存可 |
| トンネル | cloudflared（外部バイナリ） | Cloudflare の公式ツール。Quick Tunnel は設定不要 |
| KV リレー | Cloudflare Workers + KV | 無料枠で十分（10 万 req/日）。Pages と同一エコシステム |
| マシン ID | UUID v4（`uuid` crate） | 衝突確率無視可。永続化は 1 ファイル |

## 6. セキュリティ

### 認証

- **Bearer トークン**: 起動時に CSPRNG で生成（既存の `TAKO_TOKEN` と同方式）。
  QR コードの URL にトークンをフラグメント（`#token=XXX`）として埋め込む
  （フラグメントはサーバーに送信されないため cloudflared のログに残らない）
- **API リクエスト**: `Authorization: Bearer <token>` ヘッダ必須。無効なら 401
- **KV リレーの PUT**: 別途リレー用トークンで認証（tako 設定ファイルに保存）。
  GET は認証不要（machine_id は UUID で推測困難 + URL を知っていても API トークンが別途必要）

### パブリックリポでの秘密管理

- **コミットしないもの**: API トークン、VAPID 鍵、Workers の API トークン
- **環境変数**: Workers のシークレットは `wrangler secret put` で設定（KV にはトークンハッシュを保存）
- **`.gitignore`**: `remote-pwa/.env*`、`workers/*/.dev.vars`
- **CI/CD**: GitHub Actions secrets に Workers デプロイトークンを設定（リポ公開後）

### 攻撃面の最小化

- remote API サーバーは `tako remote start` で明示的に起動するまで動かない（既定 OFF）
- cloudflared tunnel は `tako remote stop` または tako 終了で即座に閉じる
- read_pane / send_input は既存の dispatch 経由なので、権限チェックは dispatch 層で一元管理
- Rate limiting: 認証失敗時の指数バックオフ（brute force 対策）

## 7. 未決定事項

- [ ] PWA のフレームワーク最終確定（Preact vs vanilla TS vs Svelte）
- [ ] xterm.js の WebSocket プロトコル設計（差分送信の粒度: 行単位 vs バイトストリーム）
- [ ] cloudflared のインストール案内の具体的な文言・自動インストール対応の要否
- [ ] Workers KV のリレー用ドメイン（`tako-relay.workers.dev` 等）
- [ ] Web Push 通知のトリガー条件の詳細（どの状態変化で通知するか）
- [ ] Phase 2 の PWA を tako リポ内に置くか別リポにするか
  - 推奨: tako リポ内の `remote-pwa/`（モノレポ。API 型定義の共有が楽）
- [ ] Phase 1 の段階で remote API を MCP ツールとしても公開するか
  - 推奨: する（開発不変条件。`tako_remote_start` / `stop` / `status` の 3 ツール）
