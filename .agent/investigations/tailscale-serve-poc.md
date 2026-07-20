# Tailscale Serve 実機 PoC レポート

> Issue: #279 | 計画: `.agent/plans/tako-remote-plan.md` §6 弾0
> 実測日: 2026-07-16 | 環境: macOS (Darwin 25.2.0, arm64)
> tailscale: 1.98.8 (brew CLI 版, TUN モード, sudo tailscaled)

## 総合判定: **成立**

Tailscale Serve は tako remote の transport として**全要件を満たす**。
WS 疎通・identity ヘッダ・HTTPS 証明書・偽装防止のすべてが実測で確認済み。
後続弾（#282〜）の設計はこの実測結果を正として進めてよい。

---

## 項目 1: HTTP / WebSocket の疎通（必須要件）

### 結論: **両方とも完全に動作する**

### HTTP 実測

```bash
$ curl -s --resolve "<hostname>.<tailnet>.ts.net:443:100.x.x.x" \
    "https://<hostname>.<tailnet>.ts.net/test"
```

```json
{
  "path": "/test",
  "method": "GET",
  "headers": {
    "Host": "<hostname>.<tailnet>.ts.net",
    "Tailscale-User-Login": "<user>@gmail.com",
    "Tailscale-User-Name": "<encoded-name>",
    "Tailscale-User-Profile-Pic": "https://lh3.googleusercontent.com/...",
    "Tailscale-Headers-Info": "https://tailscale.com/s/serve-headers",
    "X-Forwarded-For": "100.x.x.x",
    "X-Forwarded-Host": "<hostname>.<tailnet>.ts.net",
    "X-Forwarded-Proto": "https",
    "Accept-Encoding": "gzip"
  },
  "client_address": "127.0.0.1:58069"
}
```

- TLSv1.3 / AEAD-CHACHA20-POLY1305-SHA256, ALPN h2
- Let's Encrypt 証明書（CN=<hostname>.<tailnet>.ts.net, 有効期限 90 日）
- origin サーバーが受け取る `client_address` は `127.0.0.1`（serve がローカルプロキシするため）

### WebSocket 実測

Python の生ソケットで TLS + WS upgrade を実行:

```
TLS connected: TLSv1.3

=== WS Handshake Response ===
HTTP/1.0 101 Switching Protocols
Connection: Upgrade
Sec-Websocket-Accept: fqOfOdDz940yOHi4x+p25cwWdrI=
Upgrade: websocket

*** WebSocket upgrade SUCCESS ***
```

- **WS upgrade で identity ヘッダが付与される**（HTTP と同一セット）
- エコーテスト: 送信 → 受信 → 一致を確認
- WS 永続接続でのラウンドトリップ: **avg 0.19ms**（20 samples, min 0.16ms, max 0.24ms）

### tako remote への影響

tako remote は WS で画面プッシュ + input を行う。serve は WS を透過的にプロキシし、
upgrade 時に identity ヘッダも付与するため、**追加の認証チャネルを WS 用に作る必要がない**。

---

## 項目 2: identity ヘッダの実在と偽装可能性の境界

### 結論: **serve 経由のヘッダは信頼できる。whois による追加検証も可能だが必須ではない**

### 付与されるヘッダ一覧

| ヘッダ | 値の例 | 用途 |
|---|---|---|
| `Tailscale-User-Login` | `user@gmail.com` | ユーザー識別（メール） |
| `Tailscale-User-Name` | RFC 2047 エンコード済み | 表示名 |
| `Tailscale-User-Profile-Pic` | Google プロフ画像 URL | アバター |
| `Tailscale-Headers-Info` | `https://tailscale.com/s/serve-headers` | ドキュメント参照 |
| `X-Forwarded-For` | `100.x.x.x` | 接続元 tailscale IP |
| `X-Forwarded-Host` | `<hostname>.ts.net` | 元の Host |
| `X-Forwarded-Proto` | `https` | 元のプロトコル |

### 偽装テスト結果

| テスト | 結果 |
|---|---|
| serve 経由で偽 `Tailscale-User-Login: attacker@evil.com` を送信 | **上書きされた**。origin には正規の `user@gmail.com` が届く |
| serve 経由で偽 `X-Forwarded-For: 1.2.3.4` を送信 | **上書きされた**。origin には正規の tailscale IP が届く |
| localhost:18080 に直接偽ヘッダを送信 | **そのまま通る**。origin は `attacker@evil.com` を受け取る |

### whois による補助検証

```bash
$ tailscale whois 100.x.x.x    # → ノード情報 + ユーザー情報を返す (exit 0)
$ tailscale whois 127.0.0.1        # → "peer not found" (exit 1)
```

- `X-Forwarded-For` の IP で `whois` すれば、接続元ノードの正当性を検証できる
- `127.0.0.1` は `peer not found` → **serve 経由か直接接続かを whois で区別可能**

### 推奨実装方式

**ヘッダ信頼で十分。whois 照合は不要（ただし推奨）。**

根拠:
1. serve はクライアントの偽ヘッダを**完全に上書き**する（実測で確認）
2. daemon は `127.0.0.1` のみ bind するため、serve 以外の外部経路はない
3. ローカルプロセスは直接接続で偽装可能だが、同一マシン上のプロセスは
   既にマシンの全権限を持つため、防御対象外（threat model として明確化すべき）
4. 防御層を追加するなら: `X-Forwarded-For` で `whois` し、
   `peer not found` なら拒否（ローカル直接接続の排除）

---

## 項目 3: MagicDNS + HTTPS 証明書の有効化手順と URL 固定性

### 結論: **URL は固定。手順は管理画面の 2 操作のみ**

### 有効化手順

1. **MagicDNS**: https://login.tailscale.com/admin/dns → Enable MagicDNS
   - 新規 tailnet は**既定で有効**（今回も有効だった）
2. **HTTPS 証明書**: 同画面の HTTPS Certificates → Enable
   - または `tailscale serve` 初回実行時に誘導 URL が表示される:
     `https://login.tailscale.com/f/serve?node=<nodeID>`
   - 有効化はノード単位ではなく **tailnet 全体の設定**

### URL の構成と固定性

```
https://<hostname>.<tailnet-suffix>.ts.net
https://<hostname>.<tailnet>.ts.net
```

| 要素 | 値 | 変更条件 |
|---|---|---|
| hostname | `<hostname>` | `tailscale set --hostname` で変更可。**変更しなければ固定** |
| tailnet-suffix | `<tailnet>` | 管理画面 General → Tailnet name で変更可。**変更しなければ固定** |
| TLD | `.ts.net` | 不変 |

### 証明書の詳細

```
subject: CN=<hostname>.<tailnet>.ts.net
issuer: C=US, O=Let's Encrypt, CN=YE2
有効期間: 2026-07-16 〜 2026-10-14（90日）
SAN: DNS:<hostname>.<tailnet>.ts.net
```

- Let's Encrypt 発行。tailscaled が自動更新
- `tailscale cert <domain>` で手動取得も可能

### serve の off → 再設定で URL は変わらない

```bash
$ tailscale serve --https=443 off     # → 解除
$ tailscale serve --bg 18080          # → 同一 URL で再設定される
```

### tako remote への影響

- `tako remote start` は serve 設定のみ行い、URL は `tailscale status --json` の
  `Self.DNSName` から決定的に算出可能
- ユーザーがホスト名や tailnet 名を変えない限り URL は恒久固定
- QR コードは URL のみ（secret 不要）→ 一度生成すれば再利用可能

### 注意: brew CLI 版では MagicDNS の DNS 解決が効かない

brew CLI 版の tailscaled はシステムの DNS リゾルバを変更しない（System Extension ではないため）。
`dig <hostname>.<tailnet>.ts.net` は NXDOMAIN を返す。

- **影響**: 同一マシンから ts.net ドメインでアクセスするには `--resolve` が必要
- **運用上の問題**: なし（iPhone からのアクセスは App Store 版 Tailscale が DNS 解決する）
- **App Store 版**: System Extension として DNS リゾルバに統合されるため問題なし
- **tako remote setup の推奨**: App Store 版を推奨し、brew CLI 版はフォールバック

---

## 項目 4: iPhone 実機での PWA 動作

### 結論: **未実測（ユーザー協力が必要）**

以下の手順で検証可能:

#### 前提条件
- iPhone に Tailscale アプリをインストール + Mac と同一アカウントでログイン
- Mac で `tailscale serve --bg 18080` が稼働中

#### 検証手順

1. **Safari で URL を開く**:
   `https://<hostname>.<tailnet>.ts.net/`
   - テストサーバーが JSON を返すはず（本番は PWA を返す）
   - TLS エラーなし・identity ヘッダが iPhone のユーザー情報になることを確認

2. **PWA インストール**: 本番 PWA を daemon から配信する構成（弾 4 以降）のため、
   現時点ではテストサーバーの JSON 応答しか返せない。PWA テストは弾 5a で実施

3. **確認すべき項目**:
   - ts.net ドメインの DNS 解決（Tailscale アプリが解決するか）
   - HTTPS 証明書の検証（Safari が Let's Encrypt を信頼するか）
   - WS 接続の持続性（画面ロック後の再接続）
   - DERP 経由のレイテンシ（同一 LAN なら直接 P2P のはず）

---

## 項目 5: Tailscale の検出方法・CLI パスの差

### 結論: **3 種の導入形態を区別可能。setup は以下の優先順位で検出する**

### 導入形態の比較

| 項目 | brew CLI 版 | App Store 版 | brew cask 版 |
|---|---|---|---|
| パッケージ | `brew install tailscale` | Mac App Store | `brew install --cask tailscale-app` |
| CLI パス | `/opt/homebrew/bin/tailscale` | `/Applications/Tailscale.app/Contents/MacOS/Tailscale` | 同左（.pkg インストーラ） |
| デーモン管理 | `sudo tailscaled` または `brew services` | System Extension（root 不要・GUI 管理） | System Extension |
| ソケット | `/var/run/tailscaled.socket` | 同左 | 同左 |
| MagicDNS | DNS リゾルバ未統合 | System Extension で統合 | 同左 |
| root 要否 | `tailscaled` に root 必要 | 不要 | 不要 |

### 検出ロジック案（`tako remote setup` 用）

```
1. which tailscale
   → 見つかった: brew CLI 版（/opt/homebrew/bin/tailscale）
2. test -x /Applications/Tailscale.app/Contents/MacOS/Tailscale
   → 見つかった: App Store 版 or brew cask 版
3. mdfind 'kMDItemFSName == "Tailscale.app"'
   → 非標準パスの検出
4. すべて不在 → 未導入
```

### 状態判定ロジック案

```
tailscale status --json の BackendState:
  - "Running"  → ログイン済み・稼働中
  - "NeedsLogin" → デーモン起動済み・未ログイン
  - (接続失敗)  → デーモン未起動

tailscale serve status --json:
  - {} (空)    → serve 未設定
  - Web/TCP あり → serve 設定済み

tailscale status --json の CertDomains:
  - null       → HTTPS 証明書未有効化
  - [...]      → 有効化済み
```

### setup の推奨: **App Store 版を第一推奨**

理由:
- root 不要（`sudo tailscaled` が不要）
- MagicDNS の DNS 解決が自動で効く
- GUI でログイン管理（ブラウザ認証が不要なケースがある）
- 自動更新

brew CLI 版は上級者向けフォールバック（サーバー環境・headless 用途）。

---

## 項目 6: 各状態のエラー表現（setup の状態判定関数の設計材料）

### 状態遷移とエラー表現

| 状態 | コマンド | 出力 | exit code |
|---|---|---|---|
| **未導入** | `which tailscale` | `tailscale not found` | 1 |
| | `tailscale status` | `command not found: tailscale` | 127 |
| **デーモン未起動** | `tailscale status` | `failed to connect to local Tailscale service; is Tailscale running?` | 1 |
| | `tailscale serve status` | `getting serve config: Failed to connect to local Tailscale daemon ... dial unix /var/run/tailscaled.socket: connect: no such file or directory` | 1 |
| **未ログイン** | `tailscale status` | `Logged out.` | 1 |
| | `tailscale status --json` | `BackendState: "NeedsLogin"` | 0 |
| **ログイン済み・serve 未設定** | `tailscale serve status` | `No serve config` | 0 |
| | `tailscale serve status --json` | `{}` | 0 |
| **ログイン済み・HTTPS 未有効化** | `tailscale cert <domain>` | `HTTPS cert support is not enabled/configured for your tailnet` | 1 |
| | `tailscale status --json` の `CertDomains` | `null` | - |
| **ログイン済み・serve 有効化時に HTTPS 未設定** | `tailscale serve --bg 18080` | 誘導 URL を表示後に成功（自動有効化される場合あり） | 0 |
| **完全稼働** | `tailscale serve status` | `https://<host>.ts.net (tailnet only) \|-- / proxy http://127.0.0.1:<port>` | 0 |

### tailscaled の起動モードの違い

| モード | コマンド | root | TUN | serve | DNS 解決 |
|---|---|---|---|---|---|
| TUN（通常） | `sudo tailscaled` | 要 | ○ | ○ | brew CLI: × / App Store: ○ |
| userspace | `tailscaled --tun=userspace-networking` | 不要 | × | 設定可だが自機テスト不可 | × |

---

## 項目 7: レイテンシ実測

### 自機→自機（直接 P2P、同一ホスト）

| 経路 | min | avg | max | 備考 |
|---|---|---|---|---|
| **WS via serve（永続接続）** | 0.16ms | 0.19ms | 0.24ms | 20 samples |
| **HTTP via serve（毎回接続）** | 21.8ms | 43.0ms | 80.9ms | 20 samples。TLS ハンドシェイク含む |
| HTTP localhost 直接 | 1.2ms | 4.6ms | 16.9ms | 比較対照 |
| ping tailscale IP | 0.31ms | - | - | TUN ルーティングのみ |

### DERP リージョン別レイテンシ（netcheck）

| リージョン | RTT | 備考 |
|---|---|---|
| **Tokyo** | **12.3ms** | 最寄り |
| Hong Kong | 66.6ms | |
| Singapore | 75.3ms | |
| San Francisco | 111.8ms | |

### tako remote への影響

- **WS 永続接続のオーバーヘッドは無視できる**（0.19ms）。これは画面プッシュ・input の
  両方に適用される
- HTTP の 43ms は TLS ハンドシェイクが支配的。API 呼び出しは WS に載せれば回避可能
- 同一 LAN（直接 P2P）: **体感遅延なし**
- DERP 中継（日本国内）: +12ms 程度。十分実用的
- DERP 中継（海外）: 100ms〜。画面更新に若干の遅延があるが、
  テキストベースの AI 出力閲覧用途では許容範囲

### 未実測

- **iPhone → Mac の実レイテンシ**: 同一 LAN で直接 P2P になるか、DERP 経由になるかは
  ネットワーク環境依存。iPhone 実機テスト（項目 4）で計測が必要

---

## 補足: PoC 環境の注意事項

### ノード名について

PoC では statedir の違いにより 2 ノードが生成された:

| ノード | IP | 状態 | 経緯 |
|---|---|---|---|
| `<hostname>` | 100.y.y.y | offline | userspace-networking モード（初回 PoC 用。`/tmp/tailscale-poc/state`） |
| `<hostname>` | 100.x.x.x | **online** | TUN モード（本テスト用。`/var/lib/tailscale`） |

本番の `tako remote setup` では statedir を固定（App Store 版は自動管理、brew CLI 版は
`/var/lib/tailscale`）するため、この問題は発生しない。

PoC 完了後、管理画面（https://login.tailscale.com/admin/machines）から
offline の `<hostname>` を削除し、`<hostname>` を `<hostname>` にリネーム
することを推奨。

### テストサーバー

PoC 検証用の HTTP/WS テストサーバー（`127.0.0.1:18080`）はセッション終了後に停止する。
リポジトリへのコミット対象外。

---

## identity 検証の推奨実装方式（結論）

### 方式: **ヘッダ信頼 + 補助 whois**

1. **daemon は `127.0.0.1` のみ bind**（外部到達経路を構造的に排除）
2. **`Tailscale-User-Login` / `X-Forwarded-For` を信頼**
   - serve が偽ヘッダを完全上書きすることを実測で確認済み
3. **起動時に `tailscale serve status` で serve 設定を検証**
   - serve 未設定で daemon を起動させない
4. **（推奨・防御層追加）`X-Forwarded-For` で `tailscale whois` を照合**
   - `peer not found` → ローカル直接接続 → 拒否
   - 一致 → serve 経由の正規接続
5. **第二層（ペアリング）で最終認可**
   - identity が判明しても、ペアリング未承認なら画面データは返さない

### なぜ whois 必須ではないか

- ローカルプロセスによる偽装は、同一マシン上の攻撃 = マシンの全権限を既に持つ
- serve 経由の偽装は**不可能**（実測で確認）
- ペアリング層が最終防衛線として常に機能する
- ただし whois 照合のコスト（LocalAPI の Unix socket 呼び出し）は軽微なため、
  防御層として追加する価値はある
