# tako remote 接続方式・認証設計の検討

- 作成日時: **2026-07-10 10:18:13 JST（Asia/Tokyo）**
- 対象コミット: `6277c82`
- 対象: `tako remote`
- 検討範囲: Cloudflare Tunnel / Cloudflare Access / 機器ペアリング / Tailscale / SSH / tako専用ページ
- 関連レビュー: `reviews/2026-07-10_gpt5.6solレビュー.md`

## 結論

Cloudflare Tunnelを廃止してSSH一本にする必要はない。

現在の「匿名Quick Tunnel + 公共relay + URL内bearer token」は正式な長期運用構成としては弱いが、
Cloudflare Tunnelそのものはtako remoteと相性がよい。

推奨する位置づけは次のとおり。

| 接続方式 | 用途 | 推奨度 |
|---|---|---:|
| named Cloudflare Tunnel + Access + tako機器ペアリング | ブラウザからどこでも接続する正式構成 | ◎ |
| Tailscale Serve | 自分のMacとスマホ間だけで安全に利用 | ◎ |
| Quick Tunnel | 初回体験、デモ、短時間接続 | △ β限定 |
| SSH | 上級者、復旧、CLI操作、port forwarding | ○ |
| tako運営の専用クラウド | 完全ゼロ設定の固定URL | 将来の別事業判断 |

基本方針は以下である。

> named Tunnelで通信を運ぶ  
> Cloudflare Accessで人と端末を認証する  
> takoで接続機器と操作権限を認可する

## 1. 推奨アーキテクチャ

認証を単一bearer tokenへ集中させず、3層に分離する。

```text
スマホ / ブラウザ
  │
  │ ① Cloudflare Access
  │   ユーザー認証・MFA・device posture
  ▼
https://mac-name.remote.example.com
  │
  │ ② named Cloudflare Tunnel
  │   MacからCloudflareへのoutbound接続のみ
  ▼
127.0.0.1 の tako remote daemon
  │
  │ ③ tako機器ペアリング
  │   端末公開鍵・権限・TTL・revoke
  ▼
tako core → dispatch → pane操作
```

### 各層の責任

#### Cloudflare Tunnel

- Macへ外部から直接到達するlisten portを作らない
- HTTPS trafficをCloudflare edgeからlocalhostへ運ぶ
- stable hostnameを提供する
- originは`127.0.0.1`だけで待ち受ける

#### Cloudflare Access

- 誰が入口へ到達できるか決める
- IdP / Cloudflare account / MFAでユーザーを認証する
- 必要ならCloudflare One Clientによるdevice postureを要求する
- session expiryと再認証を管理する

#### tako

- どの端末に何を許可するか決める
- read-only / input / destructive等の権限を分離する
- one-time pairingを行う
- 端末ごとのrevokeと監査を提供する
- terminal操作をcore + dispatch経路へ通す

## 2. Quick Tunnelの扱い

Quick Tunnelは正式運用基盤ではなく、短時間の簡易モードとして残す。

CloudflareはQuick Tunnelについて次を明記している。

- testing / development only
- SLAなし
- productionではnamed tunnelを推奨
- hostnameはprocess再起動で変わる
- anonymousで設定可能範囲が狭い

公式資料:

- https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/do-more-with-tunnels/trycloudflare/

### Quick Tunnelを残す場合の制約

- 既定1時間で自動停止
- 最大24時間などのhard limit
- 起動ごとにone-time pairing
- read-onlyを既定
- 起動中はMac側status barへ常時表示
- 外部接続開始時にmacOS通知
- 「簡易接続・本番用途非推奨」と表示
- 公共relayを安全性の根幹にしない
- remote daemonはlocalhost bind

## 3. Cloudflare Accessとtako機器認証

Cloudflare Accessとtako独自認証は競合せず、組み合わせるべきである。

### Accessだけでは不足する理由

Accessは主に「誰がアプリへ到達できるか」を判断する。

一方、takoでは次のようなapplication-level authorizationが必要になる。

- このiPhoneは画面閲覧のみ
- このiPadはinput可能
- close / resizeはMac側で毎回確認
- 端末Aだけrevoke
- remote token失効後も他端末は維持

したがってAccessは外側のgate、tako pairingは内側の権限制御として扱う。

### Cloudflare側の推奨設定

- self-hosted applicationとして登録
- named Tunnelのpublished hostnameと結び付ける
- 許可メールアドレスまたはCloudflare account memberを完全一致
- MFA付きIdPを優先
- session durationを短めに設定
- `Include Everyone`や全OTPユーザー許可を避ける
- `Protect with Access`を有効化
- originのtako daemonでもAccess JWTを検証

Access policy公式資料:

- https://developers.cloudflare.com/cloudflare-one/access-controls/policies/
- https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/self-hosted-public-app/

### Access JWTのorigin検証

Cloudflare Access通過後のrequestには`Cf-Access-Jwt-Assertion`が付く。
tako daemon側で以下を検証する。

- RS256 signature
- `iss`が設定したteam domain
- `aud`がtako remote applicationのAUD tag
- `exp`が有効
- 必要ならemail / identity claim

公式資料:

- https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/validating-json/

Cloudflare edgeだけの判定に依存せずoriginでも検証すれば、route設定ミスやAccess bypassに対する
二段目の防御になる。

### Cloudflare One Client / device posture

より強い構成ではスマホにCloudflare One Clientを入れ、登録済みdeviceだけを許可する。

利用可能な条件の例:

- Cloudflare accountが本人
- 登録済みiPhone / Android
- One Clientがactive
- device postureに合格
- client certificateが有効

CloudflareはmacOS / Windows / Linux / iOS / Android / ChromeOSでposture-only modeを提供している。

公式資料:

- https://developers.cloudflare.com/cloudflare-one/team-and-resources/devices/cloudflare-one-client/configure/modes/device-information-only/
- https://developers.cloudflare.com/cloudflare-one/team-and-resources/devices/device-registration/

これは安全性が高い一方、一般ユーザーには設定が重いため「高度なセキュリティモード」とする。

## 4. tako独自の機器ペアリング

QRに長寿命bearer tokenを入れない。
QRは短時間のone-time pairing codeだけを運ぶ。

### 推奨フロー

1. スマホPWAが端末鍵pairを生成する
2. Macが2分程度だけ有効なpairing codeをQR表示する
3. スマホがpairing codeとpublic keyを送る
4. Mac側に端末名、browser、時刻、要求権限を表示する
5. ユーザーが許可または拒否する
6. 許可時だけ端末public keyを登録する
7. 以後のrequestを端末private keyで署名する
8. Mac側から端末単位でrevokeできる

### browser側の鍵

- WebCryptoで生成
- non-extractable private keyを使用
- localStorageへ長寿命bearer tokenを保存しない
- pairing codeはsingle-use
- 再ペアリング時は新しい鍵を生成

### 権限モデル

| Role | 権限 |
|---|---|
| Observe | screen、pane state、agent stateのみ |
| Interact | terminal input、quick keys |
| Manage | close、resize、background等 |
| Admin | 新端末承認、権限変更、revoke |

read-onlyのObserveを既定とし、Interact以上はMac側で明示承認する。

### session設計

- pairing credential: 端末単位、revokeまで保持可能
- access session: 15分〜数時間の短期
- idle timeout: 15〜60分
- remote server lifetime: 1時間 / 4時間 / 手動停止から選択
- destructive action: requestごとにMac側確認も選択可能

## 5. PWAの配置

### 推奨: PWAをMac側daemonから配信

```text
https://my-mac.remote.example.com/
  ├─ /            PWA
  ├─ /api/*       REST API
  └─ /ws          WebSocket
```

利点:

- PWAとdaemonのversionが一致
- APIと同一originなのでCORS不要
- Access cookieを同一originで扱える
- 公共Pagesがtako credentialを読まない
- named Tunnelのstable hostnameによりrelay不要
- supply-chainのtrusted componentを減らせる

### 現行の固定Pages PWAのリスク

`tako-remote.pages.dev`のJavaScriptはQR内tokenを読み、terminalへ任意inputを送れる。

Pages account、deployment pipeline、service worker、PWA dependencyが侵害されると、全利用者のremote
credentialが危険になる。

固定Pagesを残すなら最低限必要なもの:

- CSP
- `frame-ancestors 'none'`
- `object-src 'none'`
- `base-uri 'none'`
- `Referrer-Policy: no-referrer`
- external fontの自己ホスト
- application bearer tokenを扱わない設計

## 6. tako専用ページの選択肢

### 方式A: BYO Cloudflare / ユーザー所有hostname

例:

```text
https://tako.user-example.com
https://my-mac.example.com
```

ユーザー自身がCloudflare account、domain、named Tunnel、Access policyを所有する。

#### 長所

- terminal dataはtako運営serverを通らない
- Access policyとincident対応をユーザーが所有
- OSS / local-first方針と整合
- tako運営側の侵害が全端末侵害になりにくい
- public relay不要

#### 短所

- Cloudflare accountとdomainが必要
- 初期設定が重い
- 完全ゼロコンフィグではない

最初の正式版としてはこの方式が最も現実的である。
`tako setup remote`でCloudflare CLI / APIを使ったwizardを提供する。

### 方式B: tako運営の`remote.tako.app`

例:

```text
https://abc123.remote.tako.app
https://remote.tako.app/m/abc123
```

技術的には可能だが、これはOSSの補助機能ではなくtako cloud serviceになる。

必要な要素:

- tako account
- user / machine ownership DB
- tunnel provisioning API
- tunnel token rotation
- wildcard DNS / Cloudflare for SaaS
- Access application / policy automation
- abuse prevention
- billing / quota
- privacy policy
- monitoring / incident response
- account deletion / data deletion
- SLAとservice終了方針

Cloudflareのremotely-managed tunnel tokenは、所持者がconnectorを実行できる強いsecretである。
Keychain保存、端末別発行、rotationが必要になる。

公式資料:

- https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/configure-tunnels/remote-tunnel-permissions/
- https://developers.cloudflare.com/cloudflare-for-platforms/cloudflare-for-saas/security/secure-with-access/

### `remote.tako.app`を始める判断基準

以下が確認できてから別プロジェクトとして検討する。

- BYO Cloudflare / Tailscale利用者が十分いる
- 固定URLとゼロ設定への明確な需要がある
- service運営コストを負担できる
- privacy / legal / abuse対応方針がある
- security incident時に全connectorを即失効できる

## 7. SSHとの比較

SSHは成熟した安全なprotocolだが、tako remote UIの完全な代替ではない。

### SSHが得意なこと

- full shell
- public key authentication
- port forwarding
- CLI client
- bastion / audit
- emergency recovery

### tako remoteが得意なこと

- 複数pane一覧
- agent状態の集約
- terminal preview
- read-only監視
- attention inbox
- quick keys
- pane間移動

browserはraw SSHを直接扱えない。WebSSHにするとSSHをbrowserへ変換するgatewayが必要になり、
そのgatewayが強いtrusted componentになる。

### 推奨するSSHの位置づけ

- 上級者向けtransport
- recovery path
- localhost remote APIへのSSH port forwarding
- 将来native mobile appを作る場合のbackend候補
- `tako remote start --transport ssh`

SSH一本化はtakoらしいagent monitoring UIを捨てるため推奨しない。

## 8. Tailscaleとの比較

個人利用の安全性と実装容易性だけを見ると、Tailscale Serveは非常に相性がよい。

```text
iPhone / Tailscale
  │ WireGuard + device identity
  ▼
Mac / Tailscale
  ▼
https://mac-name.<tailnet>.ts.net
  ▼
127.0.0.1 の tako remote
```

### 長所

- public internetへserviceを公開しない
- device / user identityを利用
- grants / ACLで接続制御
- HTTPS certificateを自動提供
- IPが変わっても接続を維持しやすい
- public relay不要

### 短所

- Macとスマホの両方にTailscaleが必要
- Tailscale accountが必要
- QRだけの完全ゼロ設定ではない

公式資料:

- https://tailscale.com/kb/1552/tailscale-services
- https://tailscale.com/docs/concepts/tailscale-identity
- https://tailscale.com/docs/features/access-control

Tailscale Funnelはpublic internetへ公開する機能で、現在betaである。今回の安全な個人利用にはServeを使う。

- https://tailscale.com/docs/features/tailscale-funnel

## 9. 製品としての推奨モード

```text
tako remote start --transport quick
  # β、短時間、1時間で停止

tako remote start --transport tailscale
  # 個人向け推奨、tailnet内のみ

tako remote start --transport cloudflare
  # BYO named Tunnel + Access

tako remote start --transport ssh
  # 上級者、port forwarding / recovery

tako remote start --transport lan --insecure
  # 明示opt-in、信頼LAN限定
```

### 推奨デフォルト

- Tailscaleが検出されたらTailscale Serveを推奨
- Cloudflare設定済みならnamed Tunnel + Access
- どちらも無ければQuick Tunnelをβとして案内
- insecure LANはメニューの奥に置き強い警告

## 10. 実装ロードマップ

### Phase 1: 現行remoteの封じ込め

- secure modeをlocalhost bind
- bearer tokenをURL / MCP / localStorageから除去
- one-time pairing code
- read-only既定
- remote server TTL / idle timeout
- Mac側status indicator / kill switch
- stopのprocess identity /終了確認
- stateをprivate data directoryへ移動
- APIへ`Cache-Control: no-store`

### Phase 2: tako認証

- browser device key
- Mac側approval dialog
- device registry / revoke
- role-based authorization
- signed request / short session
- connection audit metadata

### Phase 3: transport provider

- `RemoteTransport` abstraction
- Quick Tunnel provider
- Tailscale Serve provider
- BYO Cloudflare named Tunnel provider
- SSH forwarding provider
- custom HTTPS reverse proxy provider

### Phase 4: Cloudflare正式対応

- `tako setup remote`
- named Tunnel作成支援
- published hostname設定
- Access application設定
- user / email policy設定
- tunnel tokenをmacOS Keychainへ保存
- token-file / environmentでcloudflaredへ渡す
- Access JWT validation
- tunnel health / reconnect / rotation

### Phase 5: 専用クラウドの判断

BYO方式の利用実績を確認してから、`remote.tako.app`を別serviceとして設計する。

## 11. 最終推奨

1. Cloudflare Tunnelは継続してよい
2. Quick Tunnelを正式基盤にしない
3. 正式版はnamed Tunnel + Access + tako機器ペアリング
4. 個人利用の第一推奨としてTailscale Serveを用意する
5. SSHは上級者・復旧用transportとして残す
6. PWAは可能ならMac側daemonから配信する
7. `remote.tako.app`は今すぐ作らず、将来のSaaSとして別判断する

この構成なら、Cloudflareをtransportと外側のidentity-aware proxyとして活用しながら、
tako固有の端末権限と安全な操作承認をMac側に保持できる。
