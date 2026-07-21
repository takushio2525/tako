# threat model — tako remote（Tailscale Serve 方式）

> v0.6.0 で導入した Tailscale Serve ベースのリモート接続の脅威モデル。
> 旧方式（Quick Tunnel / Cloudflare relay）はすべて廃止済み。

## 信頼するもの

- **Tailscale のデーモンと tailnet のコントロールプレーン**: ノード認証・
  WireGuard 鍵交換・DERP 中継の中身を読めないことを前提とする
- **WireGuard のプロトコル**: E2E 暗号化により経路上の第三者が通信内容を復号できない
- **UDS のファイルパーミッション（macOS DAC）**: tako daemon は TCP ポートを
  一切 listen せず、`<state_dir>/tako-remote.sock`（socket 0600、親ディレクトリ 0700）
  のみで待ち受ける。接続できる主体は tailscaled（serve のプロキシ元・システム権限）、
  同一 OS ユーザーのプロセス、root に限られ、**別 OS ユーザーは接続自体が不能**（#287 P1-2）。
  `X-Forwarded-For` / `X-Forwarded-Host` の検証（#287 P1-1）は多層防御として維持する
- **tako のペアリングレジストリ**: Mac ローカルの devices.json（0600）。
  ファイルの改ざんは macOS のファイル権限で防護

## 信頼しないもの

- **tailnet の他ノード（信頼できない端末が tailnet に参加した場合）**:
  二層目のペアリング認証が、Mac 所有者が明示承認していない端末を遮断する。
  未登録端末は画面データを 1 バイトも受け取れない
- **インターネット上の攻撃者**: daemon は UDS のみで listen し TCP ポートは一切開かない。
  serve は tailnet 内限定（Funnel は使わない）。公開 URL はインターネットに存在しない
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

- daemon は Unix domain socket（0600）のみで待ち受け、TCP ポートは一切開かない（#287 P1-2）
- LAN 上の別端末・同一ホストの別ユーザーからは接続不能（OS のファイルパーミッションで強制）
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
- 同一 OS ユーザーの悪意あるプロセスと root は、socket への接続・token /
  devices.json の直接読み取りが可能（OS レベルの侵害であり tako の防護範囲外。
  従来どおり）。別 OS ユーザーによる daemon への到達経路は UDS 化（#287 P1-2）で
  消滅した
