# P1-2 根治設計: tako remote daemon の Unix domain socket 化

> Issue #287 の codex 公開前レビュー（2026-07-21 最新コメント）P1-2
> 「localhost の別 OS ユーザーが承認済み機器 identity を偽装可能」の根治設計。
> 対象コミット: `8c9a65b`（main）。読取専用調査に基づく設計のみ — 実装は後続 worker が本書に従う。
>
> ユーザー方針: リスク受容ではなく **根治（Unix domain socket 化）してから公開**。

## 0. 要旨

- **結論: Unix domain socket（UDS）化は両端とも既製サポートがあり、本命として成立する**
  - Tailscale Serve は `unix:` バックエンドを公式サポート（ローカル v1.98.8 の
    `tailscale serve --help` で確認。§2.1）
  - HTTP サーバーの tiny_http 0.12.0 は `Server::http_unix(path)` を提供（§2.4）
- 変更の核は 3 点: ① daemon の listen を `127.0.0.1:7749`（TCP）から
  `<state_dir>/tako-remote.sock`（UDS、socket 0600 + 親 dir 0700）へ、
  ② serve のプロキシ先を `http://127.0.0.1:<port>` から `unix:<socket path>` へ、
  ③ admin クライアント（CLI / GUI）の接続を `TcpStream` から `UnixStream` へ
- 効果: 別 OS ユーザーは **接続自体が不能**になり、XFF / XFH の偽装余地が消滅する。
  ヘッダ検証（推測可能な値の照合）ではなくファイルシステム権限（OS が強制する
  アクセス制御）が信頼境界になる — codex 推奨の第一候補どおり
- ポート概念が消えるため `--port` / MCP `port` パラメータ / port ファイルは削除する
  （公開前・利用者は所有者のみのため互換レイヤは作らない）

## 1. 現行アーキテクチャの整理

### 1.1 listen 経路と Tailscale Serve の接続

```
iPhone（tailnet） --WireGuard--> tailscaled --(serve proxy)--> 127.0.0.1:7749 TCP  <-- 問題の入口
                                                                    ^
                                     同一 Mac の別 OS ユーザーも connect 可能
```

- daemon 起動: `run_daemon()`（`crates/tako-control/src/remote.rs:919-1121`）が
  `tiny_http::Server::http("127.0.0.1:{port}")` で TCP bind（remote.rs:922-924、既定
  `DEFAULT_PORT = 7749` remote.rs:64）。実ポートは `server.server_addr().to_ip()` で確定
  （remote.rs:925-929）
- serve 設定: `establish_tailscale_serve(port)`（remote.rs:872-911）が setup 検証後、
  `tailscale.rs` の `serve_start(cli, port)` = `tailscale serve --bg --https=443
  http://127.0.0.1:<port>`（`crates/tako-control/src/tailscale.rs:346-356`）を実行。
  既存設定の照合は `serve_state()` / `parse_serve_state()`（tailscale.rs:214-256）と
  `proxy_target_for_port(port)` = `"http://127.0.0.1:{port}"`（tailscale.rs:259-261）
- HTTP 受付: 単一ループ `server.recv_timeout()` → `/ws` は `handle_ws_v2`、他は
  `handle_request_v2`（remote.rs:1088-1109）
- WS: `request.upgrade("websocket", response)` で生 stream を取得（remote.rs:3988）→
  `tungstenite::WebSocket::from_raw_socket(stream, Role::Server, None)`（remote.rs:4012）
- 終了時: `serve_stop_if_ours()` + `cleanup_state_files()`（remote.rs:1113-1118）

### 1.2 認証の全体像（二層 + Origin）

- 層①: `authorize_device()`（remote.rs:334-377）/ `identify_tailnet()`（remote.rs:381-401）が
  `remote_auth::identify()`（`crates/tako-control/src/remote_auth.rs:488-533`）を呼ぶ。
  判定材料は **リクエストヘッダのみ**: XFF 先頭 IP を `tailscale whois`（tailscale.rs:290-306）し、
  XFH が `expected_host`（daemon 起動時に base_url から導出、remote.rs:1049-1052）と一致するか
  （remote_auth.rs:505-515、`host_matches` remote_auth.rs:537-539）
- 層②: whois の `StableID` を devices.json（0600）と照合し role 認可（remote.rs:349-369）
- Origin 検証（#287 P1-1 / PR #450）: `check_request_origin()`（remote.rs:2196-2209、
  呼び出しは REST 入口 remote.rs:2909 / WS 入口 remote.rs:3925）+ WS subprotocol 必須
  （remote.rs:3928-3941、`WS_PROTOCOL` remote.rs:2343-2344）+ CORS は base_url のみエコー
  （remote.rs:2174-2194）

**P1-2 の核心**: TCP loopback は同一ホストの全ユーザーに開いており、層①の判定材料
（XFF = 承認済み端末の tailnet IP、XFH = 公開情報の ts.net ホスト名）はどちらも秘密でない。
両方を知る別 OS ユーザーは `127.0.0.1:7749` へ直接 connect し、ヘッダ偽装だけで承認済み
端末の StableID を whois 経由で引き継げる。層②も同じ偽装 StableID で通るため防衛線に
ならない（Issue #287 最新コメント P1-2 の指摘どおり）。

### 1.3 admin API

- サーバー側: `check_admin()`（remote.rs:407-421）= XFF 付きは拒否（serve 経由の管理操作を
  遮断）+ `X-Tako-Admin` トークンの定数時間比較
- クライアント側: `admin_request()`（remote.rs:1746-1803）が
  `TcpStream::connect(("127.0.0.1", port))`（remote.rs:1766）へ手書き HTTP/1.1 を送る。
  利用者は CLI `tako remote devices`（`devices_list` / `devices_revoke` remote.rs:1807-1829）と
  GUI のペアリング承認パネル（`crates/tako-app/src/remote_panel.rs:110-190` が
  `admin_request` / `daemon_status` を直接呼ぶ）

### 1.4 state ファイルとデーモン管理

- `state_dir()` = `TAKO_REMOTE_STATE_DIR` 環境変数 → 既定 `<data_dir>/remote`
  （remote.rs:82-89。data_dir は macOS で `~/Library/Application Support/tako`、
  `crates/tako-core/src/paths.rs:10-26`）。`ensure_state_dir()` が 0700 で作成
  （remote.rs:92-102）。pid / token / port / url の各ファイルは 0600
  （remote.rs:104-117、`write_secret_file` remote.rs:126-）
- 起動: `spawn_daemon()`（remote.rs:1505-1661）が `tako remote serve` を fork し stdout の
  起動情報 JSON（remote.rs:1033-1040: `running / port / bind_addr / url / transport`）を読む。
  stale 検出は `find_port_occupant(port)` = `lsof -t -i :<port>`（remote.rs:1675-1706）+
  `kill_stale_daemon`（remote.rs:1712-）
- 停止: `daemon_stop_impl()`（remote.rs:1370-1446）が pid 検証（`verify_pid_identity`
  remote.rs:1248-）後に SIGTERM / SIGKILL、`cleanup_serve_leftover()`（remote.rs:1355-1361）で
  serve 残骸回収
- 状態: `daemon_status()`（remote.rs:1155-1198）が pid / port / url ファイルから再構成
- 隔離: TAKO_ISOLATED 時は tako-app が `TAKO_REMOTE_STATE_DIR` を隔離側へ注入
  （`crates/tako-app/src/main.rs:14487-14492`、#445/#451）

### 1.5 dispatch / CLI / MCP の対応（1:1）

- protocol: `Request::RemoteStart { port: Option<u16> }` / `RemoteStop` / `RemoteStatus`
  （`crates/tako-control/src/protocol.rs:706-715`）→ dispatch（`dispatch.rs:2267-2275`）
- CLI: `RemoteCommand::Start { port }`（`--port` 既定 7749、
  `crates/tako-cli/src/main.rs:802-809`）、分岐は main.rs:2130-2143、実装は
  `remote_start()` main.rs:2885-2930
- MCP: `tako_remote_start` に `port` プロパティ（`crates/tako-control/src/mcp.rs:1599-1603`）

## 2. Unix domain socket 化の設計（本命）

### 2.1 Tailscale Serve の UDS サポート調査結果（最重要・確定）

**サポートあり。** 根拠 3 点:

1. **ローカル実測**（tailscale v1.98.8、`/opt/homebrew/bin/tailscale`）:
   `tailscale serve --help` に明記 —
   「On Unix-like systems, you can also specify a Unix domain socket
   (e.g., unix:/tmp/myservice.sock)」+ 使用例
   「Expose a service listening on a Unix socket (Linux/macOS/BSD only):
   $ tailscale serve unix:/var/run/myservice.sock」
2. **CLI パーサのソース裏取り**（tailscale/tailscale `cmd/tailscale/cli/serve_v2.go`）:
   HTTP/HTTPS ハンドラの `applyWebServe` は
   `ipn.ExpandProxyTargetValue(target, ["http","https","https+insecure","unix"], "http")` を
   使い、`unix` は正式サポートスキーム
3. **格納形式のソース裏取り**（`ipn/serve.go` の `ExpandProxyTargetValue`）:
   `unix:` プレフィックスは **url.Parse を通らず**、非空チェックのみで入力文字列が
   そのまま config に格納される（`strings.HasPrefix(target, "unix:")` の早期 return）。
   Windows は明示エラー。**スペースを含むパス（macOS の `Application Support`）でも
   CLI パース段階では壊れない**

含意:

- `tailscale serve status --json` の `Web.<host>:443.Handlers."/".Proxy` は
  `unix:<path>` 形式になる想定 → `parse_serve_state()` の照合値を追従させる（§2.3）
- CLI パーサは安全だが、**tailscaled 本体（プロキシ実行側）が実際に connect して
  中継できるか**は環境変種（brew 版 tailscaled / App Store 版 = Network Extension）で
  未実測。§7 の S0 スパイクで最初に確定させる（サンドボックスの都合で
  `~/Library` 配下の socket に届かない可能性を潰す）

### 2.2 socket のパスとパーミッション

- パス: **`state_dir().join("tako-remote.sock")`**（新設 `socket_path()`。pid_path 等
  remote.rs:104-117 と並置）。既定で
  `~/Library/Application Support/tako/remote/tako-remote.sock`
  - `TAKO_REMOTE_STATE_DIR` / TAKO_ISOLATED の隔離注入（main.rs:14487-14492）に
    **自動追従**する（パス派生のため追加作業なし）
  - スペース入りパスは §2.1 のとおり serve CLI では安全。S0 で proxy 実行まで実測
- パーミッションは **二重防護**:
  1. 親 dir 0700（`ensure_state_dir()` 済み、remote.rs:92-102）— 別ユーザーは
     ディレクトリ走査の時点で EACCES。bind 直後の race 窓も親 dir が塞ぐ
  2. socket 自体 0600 — bind 直後に `std::fs::set_permissions(&socket_path(), 0o600)`。
     BSD/macOS は `connect(2)` に socket ファイルへの write 権限を要求する
     （Linux と同挙動。POSIX 上は実装依存のため M5 で別ユーザー実測を必須とする）
- パス長: macOS の `sockaddr_un.sun_path` は 104 バイト。既定パスは約 80 バイト以下で
  収まるが、`TAKO_REMOTE_STATE_DIR` / `TAKO_DATA_DIR` で深いパスを指定された場合に
  溢れうる → 起動時に `socket_path()` のバイト長 > 100 なら明示エラー（§8-4）
- stale socket: `UnixListener::bind` は既存ファイルで `EADDRINUSE` になる。tiny_http は
  unlink しない（レジストリソース
  `tiny_http-0.12.0/src/connection.rs` `ConfigListenAddr::bind` = 素の
  `UnixListener::bind`）ため、**bind 前の回収と終了時の unlink は tako 側の責務**（§2.7）

### 2.3 Tailscale Serve 設定の切替（tailscale.rs）

- 新設 `proxy_target_for_socket(path: &Path) -> String` = `format!("unix:{}", path.display())`
- `serve_start(cli, port)`（tailscale.rs:346-356）→ `serve_start_unix(cli, socket_path)`:
  `tailscale serve --bg --https=443 unix:<path>`（`Command::args` の 1 引数渡し =
  シェル分割なし。スペース安全）
- `parse_serve_state()`（tailscale.rs:230-256）: `ServeState::Proxy(target)` の target が
  `unix:<path>` になるだけで構造は不変の想定。**S0 で `serve status --json` の実形式を
  実測し、想定と違えばここで追従**
- `serve_stop_if_ours(cli, port)`（tailscale.rs:374-382）→ `serve_stop_if_ours_unix(cli, path)`
  へ改め、加えて**旧 TCP 形式の残骸回収**を残す: 既存ユーザーのアップグレード時、
  tailscaled 側に `http://127.0.0.1:7749` 形式の設定が永続化されたまま
  （serve 設定は tailscaled 側に残る仕様。tailscale.rs:343-345 コメント）新バイナリが
  起動する。`establish_tailscale_serve` の分岐（remote.rs:886-909）を次のとおり再定義:

| serve_state の結果 | 挙動 |
|---|---|
| NotConfigured | `serve_start_unix` で設定 |
| Proxy(`unix:<自 socket_path>`) | 再利用（現行の同一ポート再利用と同じ） |
| Proxy(`http://127.0.0.1:<旧既定 7749 等>`) | **旧世代 tako の残骸と判定**し `serve_stop` → `serve_start_unix`（自動移行） |
| Proxy(その他) / Other | 現行どおりユーザー設定と見なし上書き拒否（remote.rs:895-908 の文言を UDS 版に更新） |

  旧形式の「自ポート」判定に使っていた port 引数が消えるため、旧残骸判定は
  「`http://127.0.0.1:` プレフィックス」で行う（tako 以外がこの形式で 443 serve を
  使っている可能性は上書き拒否側に倒したければ `:7749` 限定でもよい。実装時に
  `DEFAULT_PORT` 定数を移行判定専用に残す）

### 2.4 HTTP / WS 層の変更（tiny_http）

- **tiny_http 0.12.0 は UDS を公式サポート**（レジストリソース確認済み）:
  - `Server::http_unix(path)`（`tiny_http-0.12.0/src/lib.rs:220-228`）→
    `ConfigListenAddr::unix_from_path` → `UnixListener::bind`
  - 接続は `Connection` enum（Tcp / Unix 統合、`connection.rs`）で抽象化され、
    `Request` / `Response` / `respond` / `read_json_body` 等の上位 API は transport 非依存
- `run_daemon()` の変更（remote.rs:919-929）:
  - `tiny_http::Server::http(&addr)` → `tiny_http::Server::http_unix(&socket_path())`
  - `actual_port` の特定（remote.rs:925-929、`server_addr().to_ip()` は UDS で None）を削除。
    ポート概念ごと除去し、`run_daemon(port: Option<u16>)` → `run_daemon()` に
  - bind 成功直後に socket 0600 を設定（§2.2）
- WS は**変更不要の見込み**: `request.upgrade()`（remote.rs:3988）は tiny_http の
  `Connection` 抽象上で動き、`tungstenite::WebSocket::from_raw_socket`（remote.rs:4012）は
  Read + Write ジェネリック。M5 の WS e2e で実測確認
- 受付ループ（remote.rs:1088-1109）・`respond_inner`（remote.rs:2220-2251）・
  静的配信 `serve_embedded`（remote.rs:2271-）は無変更
- 起動情報 JSON（remote.rs:1033-1040）: `port` / `bind_addr` を削除し `socket` を追加。
  `url` / `transport: "tailscale-serve"` は不変

### 2.5 認証層（identify）の扱い — 変更最小

- `identify()`（remote_auth.rs:488-533）の**ロジックは変更しない**。
  XFF なし = Local、XFF あり = XFH 照合 + whois、の構造はそのまま
- 変わるのは前提: UDS 化後、daemon に接続できる主体は
  (a) tailscaled（serve のプロキシ元。root / システム権限で 0600 を貫通できる唯一の他者）、
  (b) 同一ユーザーのプロセス（CLI / GUI）、(c) root、に**接続レベルで**限定される。
  「XFF が付いた接続 = serve 経由」という従来仮定が、ヘッダの真正性ではなく
  ファイルシステム権限で担保されるようになる
- P1-1 で追加した XFH 検証（remote_auth.rs:503-515）は**多層防御として維持**する
  （削らない理由: 実装コストゼロで、万一 socket パーミッションの設定ミスや将来の
  transport 追加があっても一段の防御が残る）
- Origin 検証 / WS subprotocol 検証 / CORS（P1-1、§1.2）も不変

### 2.6 admin API の扱い — UDS へ一本化（localhost TCP は残さない）

- 方針: **admin 用に TCP を残す案は棄却**。残すと P1-2 の攻撃面（全ユーザー到達可能な
  TCP ポート）が admin 経路にそのまま残存し、根治にならない
- サーバー側 `check_admin()`（remote.rs:407-421）は無変更
  （XFF 付き拒否 + admin トークン検証は UDS 上でも意味を持つ多層防御:
  「socket に接続できる」より狭い「token ファイル 0600 を読める」に限定し続ける）
- クライアント側 `admin_request()`（remote.rs:1746-1803）:
  - `TcpStream::connect(("127.0.0.1", port))`（remote.rs:1766）→
    `std::os::unix::net::UnixStream::connect(socket_path())`
  - 手書き HTTP/1.1（remote.rs:1760-1764）は stream 型非依存。`Host:` ヘッダは
    `127.0.0.1:{port}` → 固定 `localhost` に変更（HTTP/1.1 の必須ヘッダとしてのみ機能）
  - `set_read_timeout` は UnixStream にも同 API があり無変更
- GUI（remote_panel.rs:110-190）と CLI devices（remote.rs:1807-1829）は
  `admin_request` / `daemon_status` を共用しているため**関数内部の差し替えだけで自動追従**

### 2.7 デーモン管理（spawn / stop / status / stale 回収）

- `daemon_status()`（remote.rs:1155-1198）: port ファイル読み（remote.rs:1168-1171）を
  廃止し、応答を `{ running, pid, socket, url, transport, serve_binary, devices }` に。
  `port` キー削除・`socket` キー追加
- stale 検出の再構成 — `find_port_occupant`（lsof -t -i :port、remote.rs:1675-1706）は
  UDS では使えない。置き換えは**接続試行方式**:
  1. `socket_path()` が存在しない → 素直に起動
  2. 存在する → `UnixStream::connect` 試行
     - `ECONNREFUSED` 等で失敗 = **stale socket（プロセス死亡の残骸）** → unlink して起動続行
       （SIGKILL / クラッシュで cleanup が走らなかったケースの自動回収。現行の
       stale 自動回収 remote.rs:1526-1539 と同等の体験を維持）
     - 接続成功 = 生きた daemon がいる → `GET /api/health` を UDS 経由で叩き応答を確認。
       tako 形式の応答なら「既に起動中」エラー（現行 remote.rs:1508-1524 と同じ分岐へ）。
       併せて **health 応答に `pid` フィールドを追加**し、pid ファイル消失 + プロセス生存の
       孤児ケースでも `verify_pid_identity`（remote.rs:1248-）→ `kill_stale_daemon` の
       自動回収（現行 remote.rs:1375-1384 相当）を維持できるようにする
- `daemon_stop_impl()`（remote.rs:1370-1446）: `find_port_occupant(DEFAULT_PORT)` 分岐
  （remote.rs:1375-1384）を上記 health 経由の特定に置換。`recorded_port()`
  （remote.rs:1364-1368）は不要化。`cleanup_serve_leftover()`（remote.rs:1355-1361）は
  socket パス版 + 旧 TCP 形式（§2.3 の移行表）の両方を回収する形へ
- `cleanup_state_files()`（remote.rs:172-）に socket の unlink を追加。
  `cleanup_legacy_state_files()`（remote.rs:193-）に旧 port ファイルの掃除を追加
- `spawn_daemon()`（remote.rs:1505-1661）: `--port` 組み立て（remote.rs:1550-1554）を削除。
  stdout JSON の読み取り（remote.rs:1578-1617）は不変

### 2.8 TAKO_ISOLATED / TAKO_REMOTE_TEST_MODE

- 隔離: socket は state_dir 派生のため、TAKO_ISOLATED の `TAKO_REMOTE_STATE_DIR` 注入
  （main.rs:14487-14492）と `TAKO_REMOTE_STATE_DIR` 手動指定（remote.rs:82-89）に
  自動追従。**本番と隔離で socket パスが必ず分かれる** = 多重起動衝突も構造的に回避
- `TAKO_REMOTE_TEST_MODE=1`（remote.rs:946-952）: serve を張らない点は不変。
  base_url は現行 `http://127.0.0.1:{actual_port}` → ポート消滅に伴い固定文字列
  `http://tako-remote.test` に変更（expected_host / Origin 照合のアンカーとしてのみ機能）。
  テストクライアントは `curl --unix-socket <path>` / `UnixStream` で直叩きし、
  fake XFF / XFH 注入による認証層の実測という本質は不変。既存 e2e スクリプト・
  テストの接続部の追従は M5 に含める

## 3. 代替案の比較（Serve が UDS 非対応だった場合の記録）

調査の結果 UDS がサポート確認済みのため**いずれも採用しない**が、判断根拠を記録する:

| 案 | 内容 | セキュリティ強度 | 実装コスト | 判定 |
|---|---|---|---|---|
| 本命: UDS 0600 | listen を UDS 化 | 高（OS 強制のアクセス制御。偽装の前提となる接続自体を遮断） | 中（本書 §2。tiny_http / serve とも既製サポート） | **採用** |
| 案 A: hop-by-hop secret | Serve だけが知るランダム secret をヘッダで必須化 | 中 | - | **不成立**。`tailscale serve` にはカスタムヘッダ注入機能が存在しない（v1.98.8 help / serve_v2.go に該当機能なし）。Serve に secret を持たせる手段がない |
| 案 B: peer credential 検証 | 接続元の uid を OS API で検証 | 高 | 高 | 棄却。TCP には peer credential の概念がなく **UDS 化が前提**になる（macOS は `getsockopt(SOL_LOCAL, LOCAL_PEERCRED)`）。その上で tiny_http は `Connection` を隠蔽し fd を公開しないため、検証には tiny_http の改造か自前 accept ループが必要。socket 0600 + dir 0700 で同じ主体制限を達成できるため過剰設計 |

案 B は「UDS 化後の任意強化」としても、0600 と同等の制限にしかならないため見送る。

## 4. XFF / XFH 偽装の完全遮断: 信頼根拠と残存リスク

- **信頼根拠**: UDS 経由の接続を「serve または同一ユーザー」と信頼する根拠は、
  socket 0600 + 親 dir 0700 により connect できる主体が
  (a) tailscaled（root / システム権限）、(b) 同一ユーザーのプロセス、(c) root、に
  OS レベルで限定されること。別 OS ユーザー（P1-2 の攻撃者）は XFF を偽装する以前に
  接続できない。「XFF 付きリクエスト = serve 経由」の仮定が初めて構造的に成立する
- **serve が XFF を付けて転送する構造は残る**が、それを模倣できる主体が上記 (b)(c) に
  縮小される。この 2 者は現行 threat model が既に防護範囲外と定義する主体
  （`.agent/threat-model-remote.md:91-92`: root は devices.json を直接読み書きできる。
  同一ユーザーのプロセスも同様に token / devices.json を直接読める = ヘッダ偽装より
  強い能力を最初から持つ）
- **残存リスク（UDS 化後も残るもの。§6 の threat model 更新に反映）**:
  1. 同一 OS ユーザーの悪意あるプロセス（マルウェア）— 防護範囲外（従来どおり）
  2. root / 管理者 — 防護範囲外（従来どおり）
  3. tailscaled 自体の侵害 — Tailscale を信頼する前提（従来どおり）
  4. socket パーミッションの実装ミス — M5 の別ユーザー実測 + 回帰テストで担保

## 5. MCP / CLI / dispatch への影響（1:1 維持）

ポート概念の削除を CLI / MCP / dispatch に対称に適用する（開発不変条件）:

| 層 | 現行 | 変更後 |
|---|---|---|
| CLI | `tako remote start --port <N>`（main.rs:802-809、既定 7749） | `tako remote start`（`--port` 削除。指定時は clap が unknown argument エラー） |
| CLI | `tako remote serve --port <N>`（内部用） | `tako remote serve` |
| CLI | `tako remote status` が port 表示 | socket パス + URL 表示 |
| protocol | `Request::RemoteStart { port: Option<u16> }`（protocol.rs:706） | `Request::RemoteStart {}`（フィールド削除。serde の未知フィールド無視により旧クライアントからの port 付き要求も受理される） |
| dispatch | `host.remote_start(port)`（dispatch.rs:2267） | `host.remote_start()` |
| MCP | `tako_remote_start` の `port` プロパティ（mcp.rs:1599-1603） | プロパティ削除 + description を UDS transport の記述に更新 |
| MCP | `tako_remote_status` の「ポート番号」記述（mcp.rs:1623-） | socket / URL の記述に更新 |
| 変更なし | `tako_remote_stop` / `devices` / `agents` / `messages` / `scrollback` / `setup`、GUI remote_panel | 接続先の内部差し替えのみ（§2.6） |

- 「最も簡単なコマンドを提案する」原則（#322）にも整合: 既定値で済んでいた引数が
  仕様ごと消え、`tako remote start` の最簡形は不変
- 互換レイヤ（`--port` を受けて警告する等）は作らない。リポジトリ公開前で利用者は
  所有者のみ、黙って無視するより明示エラーが驚き最小
- MCP スナップショット・セルフテストのツール数 / スキーマ期待値の更新を M3 に含める

## 6. threat model 文書（.agent/threat-model-remote.md）の更新方針

P1-2 指摘（記述が「ペアリングで防げる」と誤った安心を与える）の是正。更新箇所は 2 つ:

1. **「信頼するもの」の「macOS のプロセス分離（制限付き）」節（threat-model-remote.md:11-17）**
   を全面書き換え。草案:

   > - **UDS のファイルパーミッション（macOS DAC）**: tako daemon は TCP ポートを
   >   一切 listen せず、`<state_dir>/tako-remote.sock`（socket 0600、親ディレクトリ 0700）
   >   のみで待ち受ける。接続できる主体は tailscaled（serve のプロキシ元・システム権限）、
   >   同一 OS ユーザーのプロセス、root に限られ、**別 OS ユーザーは接続自体が不能**。
   >   `X-Forwarded-For` / `X-Forwarded-Host` の検証（#287 P1-1）は多層防御として維持する

2. **「残存リスク（受容）」の別ユーザー偽装項（threat-model-remote.md:93-97）**を削除し、
   次で置換。草案:

   > - 同一 OS ユーザーの悪意あるプロセスと root は、socket への接続・token /
   >   devices.json の直接読み取りが可能（OS レベルの侵害であり tako の防護範囲外。
   >   従来どおり）。別 OS ユーザーによる daemon への到達経路は UDS 化（#287 P1-2）で
   >   消滅した

- 「daemon の listen 範囲」節（threat-model-remote.md:60-64）の「127.0.0.1 にのみ bind」も
  「UDS のみ・TCP なし」へ更新する
- 注意: README.md:113-123 の旧 bearer token 記述は #287 の**別指摘（P2）**。本タスクでは
  threat model と、transport に言及する docs（`docs/` の remote ページ・remote.rs 冒頭
  コメント remote.rs:39-46）のみ更新し、README の認証説明全面書き直しは P2 側で行う
  （二重修正の衝突回避。ただし README に「127.0.0.1:7749」の記述が残る場合は
  M4 で機械的に置換する）

## 7. 実装マイルストーン

### S0: スパイク（M1 の冒頭で実施。コード変更なし・隔離環境のみ）

本設計の残る不確定要素 2 点を 30 分で確定させる。**本番 serve 設定
（`tailscale serve status` に現存する 443 設定）には触れない**。隔離ポート等の代替が
ないため、実施は本番 remote 停止中（`tako remote stop` 済み）に限定し、終了後に元の
設定へ戻す。

1. スペース入りパスの proxy 実行実測:
   `nc -lU "/tmp/tako s0 test/sock"`（スペース入り dir）を立て、
   `tailscale serve --bg --https=443 "unix:/tmp/tako s0 test/sock"` →
   tailnet 上の別端末（または自ノードの ts.net URL）から HTTPS アクセスし、
   nc 側にリクエストが届くこと（= tailscaled が実 connect できること）を確認 →
   `tailscale serve --https=443 off`
2. `tailscale serve status --json` の Proxy 表現を記録（`unix:<path>` 想定の確認）
   - 期待どおりでなければ `parse_serve_state` の設計（§2.3）を実測値に合わせて修正
3. 環境変種の記録: 実行した tailscaled の形態（brew standalone / App Store 版）を記録。
   App Store 版（Network Extension）で `~/Library` 配下 socket への connect が
   失敗する場合は、フォールバック案 = socket のみ `$TMPDIR` 配下
   （macOS の per-user 0700 保証領域）へ置く設計に差し替えて M1 に反映

受け入れ: 上記 1-3 の実測結果（コマンドと出力）が Issue #287 にコメントされている。

### M1: daemon listen の UDS 化 + Serve 設定切替

対象: remote.rs（`run_daemon` / `establish_tailscale_serve` / state ファイル）、
tailscale.rs（`serve_start_unix` / `proxy_target_for_socket` / `parse_serve_state` /
`serve_stop_if_ours_unix` + 旧形式移行）

受け入れ条件（隔離環境 `TAKO_ISOLATED=1` + 実 tailnet):
1. `tako remote start` 後、`tailscale serve status --json` の Proxy が
   `unix:<隔離 state_dir>/tako-remote.sock`
2. `lsof -nP -iTCP -sTCP:LISTEN | grep 7749` が空（TCP listen ゼロ）
3. `stat -f "%Lp" <socket>` = 600、親 dir = 700
4. `curl --unix-socket <socket> http://localhost/api/health` が 200
5. tailnet 上の別端末から `https://<hostname>.<tailnet>.ts.net/api/health` が 200
6. 旧 TCP 形式の serve 設定を手で作った状態から `tako remote start` → 自動で
   unix 形式へ張り替わる（§2.3 移行表）
7. `cargo test -p tako-control` 緑（`parse_serve_state` の unix 形式テスト追加を含む）

### M2: admin API クライアントの UDS 化

対象: remote.rs（`admin_request`）。GUI / CLI devices は共用関数のため自動追従（§2.6）

受け入れ条件:
1. daemon 稼働中に `tako remote devices list` が成功（UnixStream 経由）
2. GUI のペアリング承認パネルが従来どおり pending 一覧を取得できる（隔離実測）
3. 「XFF 付き admin 要求は拒否」の既存回帰テストが緑のまま
4. daemon 停止中の `devices list`（devices.json 直読フォールバック remote.rs:1811-1813）不変

### M3: デーモン管理・CLI / MCP / dispatch の更新

対象: remote.rs（`spawn_daemon` / `daemon_stop_impl` / `daemon_status` / stale 回収 /
health への pid 追加）、tako-cli（`--port` 削除・status 表示）、protocol.rs / dispatch.rs /
mcp.rs（port 除去）、セルフテスト・MCP スナップショット期待値

受け入れ条件:
1. `tako remote start` → `kill -9 <pid>` → socket 残存を確認 → 再 `start` が stale を
   自動回収して起動（unlink ログ + 起動成功）
2. `tako remote stop` で socket / pid / token / url ファイルがすべて消える
3. `tako remote status` に socket パスと URL が表示され、port が現れない
4. `tako remote start --port 7749` が unknown argument エラー
5. MCP `tako_remote_start`（引数なし）→ `tako_remote_status` → `tako_remote_stop` の
   e2e が通る（スナップショット更新済み）
6. 同一 socket パスでの二重 `tako remote serve` が「既に起動中」で拒否される
7. `cargo test --workspace` / `cargo fmt --check` / `cargo clippy -D warnings` 全緑

### M4: threat model / docs 更新

対象: `.agent/threat-model-remote.md`（§6 の草案適用）、remote.rs 冒頭コメント
（remote.rs:39-46 の transport 記述）、`docs/` の remote 関連ページ、
`.agent/plans/tako-remote-plan.md` §4 への追記（層①の前提変更）

受け入れ条件:
1. threat-model-remote.md に「XFF+XFH を偽装して identify を通過できる」記述が残っていない
   （grep で確認）
2. 「127.0.0.1」「7749」がコード外文書（README / docs / .agent）で transport 説明として
   残っていない（履歴的な記述・CHANGELOG を除く。grep リストを Issue に添付）
3. `npm run build`（docs）が緑

### M5: 隔離テスト + P1-1 統合検証 + 別 OS ユーザー実測

受け入れ条件（すべて機械検証、証拠を Issue #287 へ):
1. **P1-2 根治の直接実証**: 別 OS ユーザー（ゲスト等が無ければ
   `sudo -u nobody curl --unix-socket <socket> http://localhost/api/health`）が
   **Permission denied（接続不能）**になることを実測。同一ユーザーの同コマンドは 200
2. XFF / XFH 偽装の無効化: 同実測で、別ユーザーからはヘッダ偽装リクエスト自体が
   送信不能であることを確認（1 の帰結として記録）
3. P1-1 統合: evil Origin の REST / WS が 403 / 拒否、正規 Origin + subprotocol の WS が
   101 + 差分プッシュ受信（#450 の回帰）
4. TAKO_REMOTE_TEST_MODE の認証層テスト（fake XFF 注入）が UDS 経由で全緑
5. 実機: iPhone（承認済み端末）から PWA 接続 → 画面表示 → 入力送達（透過性の確認）
6. `TAKO_SELF_TEST=1` セルフテスト FAILED 0（既知除く）

## 8. エッジケースとリスク

1. **Tailscale バージョン依存**: `unix:` サポートの下限バージョンは公表資料からは不明
   （v1.98.8 で確認済み）。対策: `serve_start_unix` 失敗時のエラーメッセージに
   `tailscale version` の出力と「Tailscale の更新（brew upgrade tailscale / App Store）」の
   案内を含める。加えて `setup_status` の検査に「`tailscale serve --help` の出力が
   `unix:` を含むか」を追加し、未対応版は remote start 前に不足項目として列挙する
   （`MissingItem` 追加。tailscale.rs:54-66 の既存パターン）
2. **macOS の UDS パーミッション挙動**: BSD 系は connect に write 権限が必要だが
   POSIX 上は実装依存。M5-1 の別ユーザー実測を必須ゲートにする。万一 socket 0600 が
   効かない OS 挙動があっても、親 dir 0700 の search 権限で到達自体を防げる（二重防護）
3. **stale socket**: SIGKILL / クラッシュで unlink されず残る → §2.7 の接続試行方式で
   次回起動時に自動回収。終了時は `cleanup_state_files` で unlink
4. **socket パス長超過**: `sun_path` 104 バイト（macOS）。起動時にバイト長 > 100 で
   明示エラー（TAKO_REMOTE_STATE_DIR で深いパスを指定した場合のみ起こる）
5. **多重起動**: 隔離系はパスが分かれ衝突しない（§2.8）。同一パスは接続試行 →
   「既に起動中」拒否（M3-6）
6. **スペース入り既定パス**: CLI パーサは安全（§2.1 ソース確認済み）。proxy 実行と
   `serve status --json` 内の表現は S0 で実測確定
7. **tailscaled の環境変種**: brew standalone 版は root daemon で 0600 socket に
   connect 可能。App Store 版（Network Extension）のサンドボックスが `~/Library` 配下へ
   届かない可能性は S0-3 で確定し、必要なら socket を `$TMPDIR` 配下へ置く
   フォールバックに切り替える（設計変更は socket_path() の 1 関数に閉じる）
8. **リモート端末（PWA）への影響**: なし。外側 URL（`https://<hostname>.<tailnet>.ts.net`）・
   API・認証フロー・PWA 配信は不変で、変わるのは serve → daemon 間の内側 transport のみ
9. **Windows**: `unix:` serve target は Windows 非対応（§2.1）だが、remote daemon 自体が
   tmux 必須（remote.rs:934-939）で現状 macOS 限定のため影響なし。Phase 6 の Windows 対応
   時に named pipe / `AF_UNIX`（Win10+）を再検討（tiny_http の `http_unix` は
   `#[cfg(unix)]`）
10. **serve 設定の残骸**: 旧 TCP 形式・新 unix 形式とも tailscaled 側に永続化される。
    stop 時の `serve_stop_if_ours_unix` + 起動時の移行分岐（§2.3）で双方向に回収

## 参考（調査ソース一覧）

- Issue #287 最新コメント（codex レビュー 2026-07-21）の P1-2 セクション — 本設計の正
- `tailscale serve --help`（v1.98.8 ローカル実測）
- tailscale/tailscale ソース: `cmd/tailscale/cli/serve_v2.go`（applyWebServe）、
  `ipn/serve.go`（ExpandProxyTargetValue）
- tiny_http 0.12.0 レジストリソース: `src/lib.rs`（http_unix）、`src/connection.rs`
  （Listener / Connection / ConfigListenAddr）、`src/request.rs`（upgrade）
- 現行実装: `crates/tako-control/src/remote.rs` / `remote_auth.rs` / `tailscale.rs`、
  `crates/tako-cli/src/main.rs`、`crates/tako-control/src/{protocol,dispatch,mcp}.rs`、
  `crates/tako-app/src/remote_panel.rs`、`.agent/threat-model-remote.md`、
  `.agent/plans/tako-remote-plan.md`
