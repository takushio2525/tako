# threat model — tako remote（Tailscale Serve 方式）

> v0.6.0 で導入した Tailscale Serve ベースのリモート接続の脅威モデル。
> 旧方式（Quick Tunnel / Cloudflare relay）はすべて廃止済み。

## 信頼するもの

- **Tailscale のデーモンと tailnet のコントロールプレーン**: ノード認証・
  WireGuard 鍵交換・DERP 中継の中身を読めないことを前提とする
- **WireGuard のプロトコル**: E2E 暗号化により経路上の第三者が通信内容を復号できない
- **macOS のプロセス分離（制限付き）**: tako daemon（127.0.0.1 bind）にはローカルの
  任意プロセス（別 OS ユーザー含む）が到達できる。serve 経由の接続は `X-Forwarded-For` +
  `X-Forwarded-Host` が serve によって設定され、daemon は XFH が期待ホスト名と一致する
  ことを検証する（#287 P1-1）。XFF のみ / XFH 欠落 / XFH 不一致は拒否。
  残存リスク: 同一マシンの別ユーザーが tailnet IP **と** ts.net ホスト名の両方を
  知っている場合、XFF + XFH を偽装して identify を通過できる。
  ただしペアリング層（層②）が依然として最終防衛線として機能する
- **tako のペアリングレジストリ**: Mac ローカルの devices.json（0600）。
  ファイルの改ざんは macOS のファイル権限で防護

## 信頼しないもの

- **tailnet の他ノード（信頼できない端末が tailnet に参加した場合）**:
  二層目のペアリング認証が、Mac 所有者が明示承認していない端末を遮断する。
  未登録端末は画面データを 1 バイトも受け取れない
- **インターネット上の攻撃者**: daemon は 127.0.0.1 にのみ bind。serve は
  tailnet 内限定（Funnel は使わない）。公開 URL はインターネットに存在しない
- **DERP 中継サーバー**: WireGuard 暗号化により中身は読めない。メタデータ
  （接続時刻・IP・転送量）は Tailscale 社が見える

## 攻撃面と対策

### Tailscale アカウント侵害

攻撃者が Tailscale アカウントの資格情報を取得した場合:

- 新しいノードを tailnet に追加できる
- しかし tako の**ペアリング層が阻止**: 未承認端末は Mac 画面のダイアログで
  許可されない限り接続できない
- **推奨**: tailnet lock を有効化（管理画面で数クリック）。新規ノードの追加に
  既存ノードの署名が必要になり、アカウント侵害だけでは参加できなくなる

### ts.net ホスト名の CT log 露出

- Tailscale が ts.net ドメインの TLS 証明書を Let's Encrypt から取得する際、
  証明書は Certificate Transparency (CT) log に記録される
- CT log には `<マシン名>.<tailnet名>.ts.net` が含まれる
- これはマシン名と tailnet 名（多くの場合ユーザーのメールアドレスに紐づく名前）を
  公開情報として露出させる
- **対策**: CT log の露出は Tailscale の仕様であり tako 側では防げない。
  ドキュメントに事実として明記し、ユーザーが判断できるようにする

### ペアリング承認の人間依存

- ペアリングの承認と role 昇格は Mac 画面の GUI ダイアログでのみ行える
- MCP / CLI に承認 API を**意図的に作らない**（AI がリモートアクセス権を自律的に
  付与することを構造的に防ぐ）
- この例外は `.agent/requirements.md` に明記

### daemon の listen 範囲

- daemon は 127.0.0.1 にのみ bind（P0-1 で実装済み）
- LAN 上の別端末や同一ホストの別ネットワークインターフェースからは到達不能
- serve 経由（tailnet 内）のアクセスのみが到達する

### 入力操作の role 制限

- observe（画面閲覧のみ）は既定 role
- interact（入力・承認応答）は Mac 側で明示的に role 昇格が必要
- manage（close / resize）と admin（端末管理）はさらに上位
- role ごとに API アクセスが制限され、observe 端末が入力を試みても 403

### upload API（弾 5b）

- Interact role 必須
- サイズ上限 20MB
- 保存先はペイン cwd 配下の `.tako-remote-uploads/` 固定
- パス traversal 検証（`..` や絶対パスを拒否）
- 実行権限を付けない（0o600。所有者のみ読み書き可。#287 P2-1）
- 監査ログにファイル名を含めない（バイト数のみ。ペイン内容と同基準。#287 P2-2）
- シンボリックリンクの follow を拒否（リンク先への意図しない書き込み防止。#287 P2-4）

## 残存リスク（受容）

- Tailscale 社がコントロールプレーンを通じて tailnet の構成情報を見られる
  （通信内容は WireGuard で保護されており見えない）
- DERP 中継を経由する場合、Tailscale 社がトラフィックの量・タイミングを観測できる
  （P2P 接続時はこのリスクは軽減される）
- CT log に ts.net ホスト名が記録される（上記。macOS のホスト名を一般名にすることで
  軽減可能だが、完全な防止は不可能）
- ローカルの root / 管理者権限を持つ攻撃者は daemon のプロセスメモリや
  devices.json を直接読み書きできる（OS レベルの侵害であり tako の防護範囲外）
- 同一マシンの別 OS ユーザーが tailnet IP と ts.net ホスト名を知っている場合、
  daemon（127.0.0.1 bind）に XFF + XFH を偽装して接続し、層①の identity 検証を
  通過できる可能性がある。ペアリング層（層②）が最終防衛線。macOS の典型的な
  シングルユーザー運用ではこのリスクは事実上発生しない（#287 P1-1 で軽減済み、
  完全排除は Unix socket 化で対応予定）
