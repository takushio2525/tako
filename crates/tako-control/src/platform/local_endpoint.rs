//! remote デーモンのローカルエンドポイント（抽象境界 B4）
//!
//! `tako remote serve` のデーモンは「同一マシンからのみ到達できるローカル HTTP
//! エンドポイント」を張り、`tailscale serve` がそれを tailnet へプロキシする。
//! この「ローカルエンドポイント」の実体だけがプラットフォームで異なる…
//! **はずだったが、実測では同じ macOS の中でも tailscaled の導入形態で変わる**（#1038）。
//!
//! - ループバック TCP（`127.0.0.1:<エフェメラルポート>`）= **既定**。
//!   どの tailscaled 変種でも dial できる唯一の形（#1038 / #971）
//! - Unix domain socket（0600）= unix のみの互換経路。`TAKO_REMOTE_ENDPOINT=unix` で選ぶ。
//!   別 OS ユーザーは接続自体が不能になるので分離は強いが、**サンドボックス版の
//!   tailscaled（macOS の GUI 版 Tailscale.app のシステム拡張）は unix socket を
//!   一切 dial できない**ため、その環境では全リクエストが 502 になる
//!
//! ## ループバック TCP の接続元検証（#841 / FR-6.18）
//!
//! TCP は「同一マシンの別ユーザー / 別プロセスも接続できる」。UDS がカーネルで
//! 強制していた同一ユーザー限定がここでは効かないため、`X-Forwarded-For` を
//! identity の根拠として信じる層①（`remote_auth`）には偽装の余地があった。
//! そこで **XFF を読む前に接続元プロセスを引き当てて検証する**（[`verify_peer`]）。
//! 判断材料は 2 つで、どちらも取れなければ**拒否側へ倒す**:
//!
//! 1. **所有者ゲート**（カーネルが答える事実。名前と違って詐称できない）:
//!    接続元ソケットの所有ユーザーが自分自身か root であること。
//!    これで UDS 0600 が持っていた「別 OS ユーザーを排除する」性質が戻る。
//!    **Windows はソケットの所有者を引く手段が無い**ので、この段は強制できない
//!    （[`owner_check_kind`] が `unavailable` を返す。残存リスクは脅威モデル参照）
//! 2. **実行ファイルゲート**: 接続元の実行ファイル名が tailscale デーモンのものであること
//!    （[`looks_like_tailscale_daemon`]）
//!
//! **分岐はプラットフォームではなくエンドポイントの形**で行う。`TAKO_REMOTE_ENDPOINT=unix`
//! を明示した UDS 経路は、同一ユーザー限定をカーネルが強制しているので検証を通さない。
//!
//! 併せて ①バインドは 127.0.0.1 限定（LAN へは出さない）②ポートは毎回
//! エフェメラル ③管理 API は `X-Forwarded-For` が付いていると常に拒否、を維持する。
//!
//! 呼び出し側（`remote.rs`）はこのモジュールだけを見る。`cfg` はこのファイルの内側に閉じる。

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// ループバック TCP の接続先は常に 127.0.0.1（LAN へは出さない）
const LOOPBACK: Ipv4Addr = Ipv4Addr::LOCALHOST;
/// 生存確認の接続タイムアウト。ローカルなので短くてよい
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// 待ち受けの形（bind する前の指定）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointSpec {
    /// Unix domain socket（unix のみ）
    Unix(PathBuf),
    /// 127.0.0.1 のエフェメラルポート
    Loopback,
}

/// 待ち受け中のエンドポイント（bind して確定した実体）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    Unix(PathBuf),
    Loopback(u16),
}

impl Endpoint {
    /// 人間向けの表示（status / ログ用）。`tailscale serve` の target 表現とは別物
    pub fn describe(&self) -> String {
        match self {
            Self::Unix(p) => format!("unix:{}", p.display()),
            Self::Loopback(port) => format!("127.0.0.1:{port}"),
        }
    }

    /// 種別の短い識別子（JSON 応答用）
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::Unix(_) => "unix",
            Self::Loopback(_) => "loopback-tcp",
        }
    }

    /// ループバック TCP のポート（UDS なら None）
    pub fn port(&self) -> Option<u16> {
        match self {
            Self::Loopback(p) => Some(*p),
            Self::Unix(_) => None,
        }
    }

    /// UDS のパス（TCP なら None）
    pub fn socket_path(&self) -> Option<&Path> {
        match self {
            Self::Unix(p) => Some(p.as_path()),
            Self::Loopback(_) => None,
        }
    }
}

/// エンドポイントで既に別のデーモンが待ち受けているか。
/// 「接続できる = 生きている」「接続できない = stale」の判定に使う
pub fn probe_alive(endpoint: &Endpoint) -> bool {
    match endpoint {
        Endpoint::Loopback(port) => {
            TcpStream::connect_timeout(&SocketAddr::from((LOOPBACK, *port)), PROBE_TIMEOUT).is_ok()
        }
        Endpoint::Unix(path) => imp::probe_alive_unix(path),
    }
}

/// デーモン側の待ち受けを開始する。UDS は同一ユーザーのみ到達できるよう保護する。
/// 返り値の `Endpoint` が確定した実体（ループバックはここでポートが決まる）
pub fn bind(spec: &EndpointSpec) -> io::Result<(tiny_http::Server, Endpoint)> {
    match spec {
        EndpointSpec::Loopback => {
            // ポート 0 = OS にエフェメラルポートを選ばせる。バインドは 127.0.0.1 限定
            let server = tiny_http::Server::http(SocketAddr::from((LOOPBACK, 0)))
                .map_err(|e| io::Error::other(format!("remote API サーバーを起動できない: {e}")))?;
            let port = server
                .server_addr()
                .to_ip()
                .map(|a| a.port())
                .ok_or_else(|| io::Error::other("待ち受けポートを取得できない"))?;
            Ok((server, Endpoint::Loopback(port)))
        }
        EndpointSpec::Unix(path) => {
            let server = imp::bind_unix(path)?;
            Ok((server, Endpoint::Unix(path.clone())))
        }
    }
}

/// 組み立て済みの生 HTTP リクエストを送り、応答全体を文字列で返す（クライアント側）。
/// `read_timeout` が `None` のときは待ち続ける
pub fn request_raw(
    endpoint: &Endpoint,
    request: &str,
    read_timeout: Option<Duration>,
) -> io::Result<String> {
    match endpoint {
        Endpoint::Loopback(port) => {
            let mut stream =
                TcpStream::connect_timeout(&SocketAddr::from((LOOPBACK, *port)), PROBE_TIMEOUT)?;
            if let Some(t) = read_timeout {
                stream.set_read_timeout(Some(t)).ok();
            }
            roundtrip(&mut stream, request)
        }
        Endpoint::Unix(path) => imp::request_raw_unix(path, request, read_timeout),
    }
}

/// 接続済みストリームへ書いて全部読む（TCP / UDS 共通の HTTP/1.1 往復）
fn roundtrip<S: io::Read + io::Write>(stream: &mut S, request: &str) -> io::Result<String> {
    stream.write_all(request.as_bytes())?;
    stream.flush()?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    Ok(String::from_utf8_lossy(&response).into_owned())
}

/// エンドポイントのパス長上限（unix の `sun_path` 制約に由来）。
/// 制約が無いプラットフォームでは `None`
pub fn path_byte_limit() -> Option<usize> {
    imp::PATH_BYTE_LIMIT
}

/// この環境で Unix domain socket を待ち受けに使えるか
pub fn unix_supported() -> bool {
    cfg!(unix)
}

// --- 接続元プロセスの検証（#841）---

/// 検証を切って旧挙動（XFF を無条件に信じる）へ戻す A/B の逃げ道。
/// **本番で使うためのものではない**（回帰の実測と障害時の退避のみ）
const LEGACY_ENV: &str = "TAKO_841_LEGACY";
/// 追加で信頼する接続元の実行ファイル名（カンマ区切り）。
/// tailscale の導入形態は増えうるので、名前の集合を**設置場所に依らず**足せる口を残す
const TRUSTED_PEER_NAMES_ENV: &str = "TAKO_REMOTE_TRUSTED_PEER_NAMES";

/// 接続元プロセスの検証結果（#841）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerTrust {
    /// 検証の対象外。UDS はカーネルが 0600 で同一ユーザーに限定するので、
    /// 「serve 経由の identity を信じてよい」根拠がエンドポイントの形そのものにある
    NotRequired,
    /// 信頼できる（tailscale デーモンが張った接続）
    Trusted,
    /// 信頼できない
    Rejected(PeerReject),
}

impl PeerTrust {
    /// XFF を identity の根拠として読んでよいか
    pub fn accepts_forwarded_headers(&self) -> bool {
        !matches!(self, Self::Rejected(_))
    }

    /// 拒否理由（拒否でなければ `None`）
    pub fn reject(&self) -> Option<PeerReject> {
        match self {
            Self::Rejected(r) => Some(*r),
            _ => None,
        }
    }
}

/// 拒否の理由コード。**診断ログと status に載るのはこのコードと短い説明だけ**で、
/// ペインの内容・送信テキスト・トークンは決して載せない
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerReject {
    /// 接続元アドレスが取れない
    NoPeerAddr,
    /// 接続元がループバックでない（127.0.0.1 限定で bind しているので本来起きない）
    NotLoopback,
    /// 接続元ソケットの所有プロセスを特定できない（既に閉じた / 表を引けない）
    Unresolved,
    /// 所有ユーザーが自分でも root でもない（= 別 OS ユーザーのプロセス）
    OwnerUntrusted,
    /// 所有プロセスの実行ファイルを読めない
    ImageUnknown,
    /// 所有プロセスが tailscale デーモンではない
    NotTailscaleDaemon,
}

impl PeerReject {
    /// ログ・JSON 用の安定した理由コード
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoPeerAddr => "no_peer_addr",
            Self::NotLoopback => "not_loopback",
            Self::Unresolved => "peer_unresolved",
            Self::OwnerUntrusted => "owner_untrusted",
            Self::ImageUnknown => "image_unknown",
            Self::NotTailscaleDaemon => "not_tailscale_daemon",
        }
    }

    /// 人間向けの 1 行説明（応答本文にも使う。接続元の詳細は明かさない）
    pub fn describe(&self) -> &'static str {
        match self {
            Self::NoPeerAddr => "接続元アドレスを取得できない",
            Self::NotLoopback => "接続元がループバックではない",
            Self::Unresolved => "接続元プロセスを特定できない",
            Self::OwnerUntrusted => "接続元プロセスの所有ユーザーが一致しない",
            Self::ImageUnknown => "接続元プロセスの実行ファイルを読めない",
            Self::NotTailscaleDaemon => "接続元プロセスが tailscale デーモンではない",
        }
    }
}

/// この接続の `X-Forwarded-For` を identity の根拠として信じてよいか（#841）。
///
/// **分岐はエンドポイントの形で行う**（プラットフォームではない）。`peer` は
/// `tiny_http::Request::remote_addr()` の値で、UDS のときは `None` になる。
///
/// 材料が 1 つでも欠けたら [`PeerTrust::Rejected`] を返す（fail-closed）。
/// 「接続が既に閉じていて所有者を引けない」も拒否側 = 攻撃者が接続を即閉じることで
/// 検証を素通りさせられない
pub fn verify_peer(endpoint: &Endpoint, peer: Option<SocketAddr>) -> PeerTrust {
    let Endpoint::Loopback(local_port) = endpoint else {
        // UDS: 0600 の socket + 0700 の親ディレクトリで別ユーザーは接続自体が不能
        return PeerTrust::NotRequired;
    };
    if legacy_mode() {
        return PeerTrust::NotRequired;
    }
    let Some(peer) = peer else {
        return PeerTrust::Rejected(PeerReject::NoPeerAddr);
    };
    if !peer.ip().is_loopback() || !matches!(peer.ip(), IpAddr::V4(_)) {
        return PeerTrust::Rejected(PeerReject::NotLoopback);
    }
    let Some(found) = tako_core::platform::procinfo::loopback_tcp_peer(peer.port(), *local_port)
    else {
        return PeerTrust::Rejected(PeerReject::Unresolved);
    };
    // ① 所有者ゲート: 別 OS ユーザーのプロセスを排除する（UDS 0600 と同じ境界）。
    //    tailscaled は root で動くので root は通す（root は元から何でもできる）
    if let Some(uid) = found.uid {
        if uid != 0 && Some(uid) != tako_core::platform::procinfo::current_uid() {
            return PeerTrust::Rejected(PeerReject::OwnerUntrusted);
        }
    }
    // ② 実行ファイルゲート: 名前で見る（設置場所は環境ごとに違う = 受け入れ条件 4）
    let Some(path) = tako_core::platform::procinfo::image_path(found.pid) else {
        return PeerTrust::Rejected(PeerReject::ImageUnknown);
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !looks_like_tailscale_daemon(&name) {
        return PeerTrust::Rejected(PeerReject::NotTailscaleDaemon);
    }
    PeerTrust::Trusted
}

/// 実行ファイル名が tailscale デーモンのものか（純関数）。
///
/// **パスでは判定しない**。同じ macOS でも実測でこれだけ散る（2026-09-12）:
///
/// - Homebrew の standalone: `/opt/homebrew/Cellar/tailscale/<版>/bin/tailscaled`
/// - GUI 版のシステム拡張: `/Library/SystemExtensions/<UUID>/…/io.tailscale.ipn.macsys.network-extension`
/// - Windows の既定: `C:\Program Files\Tailscale\tailscaled.exe`
///
/// 版番号・UUID・インストール先がすべて環境依存なので、**不変なのは名前に
/// `tailscale` が入ること**だけ。拡張子と大文字小文字は落として比べる
/// （`procinfo::details_by_name` と同じ畳み方）。
/// 想定外の導入形態は [`TRUSTED_PEER_NAMES_ENV`] で名前を足せる
pub fn looks_like_tailscale_daemon(file_name: &str) -> bool {
    let stem = peer_name_stem(file_name);
    if stem.is_empty() {
        return false;
    }
    if stem.contains("tailscale") {
        return true;
    }
    extra_trusted_names().contains(&stem)
}

/// 所有者ゲートをこの OS で強制できるか（status の `owner_check` に出す）
pub fn owner_check_kind() -> &'static str {
    if cfg!(unix) {
        "uid"
    } else {
        "unavailable"
    }
}

/// 検証が切られているか（A/B の逃げ道。status にも出す）
pub fn legacy_mode() -> bool {
    std::env::var(LEGACY_ENV).is_ok_and(|v| v == "1")
}

/// 実行ファイル名から拡張子と大文字小文字を落とした比較用の語
fn peer_name_stem(name: &str) -> String {
    let name = name.trim();
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let lower = base.trim().to_ascii_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_string()
}

/// 環境変数で足された信頼名（カンマ区切り。空要素は無視）
fn extra_trusted_names() -> Vec<String> {
    std::env::var(TRUSTED_PEER_NAMES_ENV)
        .ok()
        .map(|v| {
            v.split(',')
                .map(peer_name_stem)
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::os::unix::net::UnixStream;

    /// macOS の `sun_path` は 104 バイト。余裕を見て 100 で弾く
    pub const PATH_BYTE_LIMIT: Option<usize> = Some(100);

    pub fn probe_alive_unix(endpoint: &Path) -> bool {
        UnixStream::connect(endpoint).is_ok()
    }

    pub fn bind_unix(endpoint: &Path) -> io::Result<tiny_http::Server> {
        let server = tiny_http::Server::http_unix(endpoint)
            .map_err(|e| io::Error::other(format!("remote API サーバーを起動できない: {e}")))?;
        // socket を 0600 に制限（別 OS ユーザーの接続を遮断）
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(endpoint, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| io::Error::other(format!("socket パーミッションの設定に失敗: {e}")))?;
        Ok(server)
    }

    pub fn request_raw_unix(
        endpoint: &Path,
        request: &str,
        read_timeout: Option<Duration>,
    ) -> io::Result<String> {
        let mut stream = UnixStream::connect(endpoint)?;
        if let Some(t) = read_timeout {
            stream.set_read_timeout(Some(t)).ok();
        }
        super::roundtrip(&mut stream, request)
    }
}

#[cfg(not(unix))]
mod imp {
    use super::*;

    pub const PATH_BYTE_LIMIT: Option<usize> = None;

    /// Windows の tailscale には unix socket の serve target が無く（#971）、
    /// AF_UNIX にも `SO_PEERCRED` 相当が無いので UDS 経路は用意しない。
    /// ループバック TCP が唯一の形（縮退ではなくその OS での正しい形）
    pub fn probe_alive_unix(_endpoint: &Path) -> bool {
        false
    }

    pub fn bind_unix(_endpoint: &Path) -> io::Result<tiny_http::Server> {
        Err(io::Error::other(UNSUPPORTED))
    }

    pub fn request_raw_unix(
        _endpoint: &Path,
        _request: &str,
        _read_timeout: Option<Duration>,
    ) -> io::Result<String> {
        Err(io::Error::other(UNSUPPORTED))
    }

    const UNSUPPORTED: &str = "この環境では Unix domain socket を待ち受けに使えません。\
        ループバック TCP（既定）をお使いください";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ループバックはバインドでポートが確定し表示できる() {
        let (server, ep) = bind(&EndpointSpec::Loopback).expect("bind できる");
        let port = ep.port().expect("ポートがある");
        assert!(port > 0, "エフェメラルポートが割り当たる");
        assert_eq!(ep, Endpoint::Loopback(port));
        assert_eq!(ep.describe(), format!("127.0.0.1:{port}"));
        assert_eq!(ep.kind_str(), "loopback-tcp");
        assert!(ep.socket_path().is_none());
        // バインド先は 127.0.0.1 限定（LAN へ露出しない）
        let addr = server.server_addr().to_ip().expect("ip アドレス");
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
        drop(server);
    }

    #[test]
    fn 待ち受けていないループバックポートはprobeでfalse() {
        // **解放したてのエフェメラルポートを使わない**: OS がすぐ再利用するので
        // 並列テストや混んだ CI では「別の誰かが listen 中」になりうる（実際 CI で落ちた）。
        // 特権ポートの 1 番なら非 root では bind できず、接続は拒否（またはタイムアウト）
        // に落ちるので、どちらの経路でも false が返る
        assert!(!probe_alive(&Endpoint::Loopback(1)));
    }

    #[test]
    fn 存在しないudsパスはprobeでfalse() {
        let missing = std::env::temp_dir().join(format!(
            "tako-local-endpoint-absent-{}.sock",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&missing);
        assert!(!probe_alive(&Endpoint::Unix(missing)));
    }

    #[test]
    fn 待ち受け中のループバックはprobeでtrue() {
        let (server, ep) = bind(&EndpointSpec::Loopback).expect("bind できる");
        assert!(probe_alive(&ep));
        drop(server);
    }

    // --- 接続元プロセスの検証（#841）---

    /// テスト中だけ環境変数を差し替える（drop で必ず戻す）。
    /// env はプロセス共有なので **`ENV_LOCK` を取ってから**使うこと
    struct EnvGuard(&'static str, Option<String>);
    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let old = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self(key, old)
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.1 {
                Some(v) => std::env::set_var(self.0, v),
                None => std::env::remove_var(self.0),
            }
        }
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 自分自身へループバック接続を張り、その接続を検証にかける（drop で閉じる）
    struct SelfConnection {
        _listener: std::net::TcpListener,
        _accepted: std::net::TcpStream,
        client: std::net::TcpStream,
        endpoint: Endpoint,
    }
    impl SelfConnection {
        fn new() -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
            let port = listener.local_addr().expect("addr").port();
            let client = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
            let (accepted, _) = listener.accept().expect("accept");
            Self {
                _listener: listener,
                _accepted: accepted,
                client,
                endpoint: Endpoint::Loopback(port),
            }
        }
        fn peer(&self) -> Option<SocketAddr> {
            self.client.local_addr().ok()
        }
        /// 自分の実行ファイル名（信頼名の追加で「正規の tailscaled」を代理する）
        fn own_image_name() -> String {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .expect("自分の実行ファイル名")
        }
    }

    #[test]
    fn tailscaleデーモンの名前判定は設置場所に依らない() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(TRUSTED_PEER_NAMES_ENV);
        // 実測で確認した 3 系統（Homebrew standalone / macOS GUI のシステム拡張 / Windows）
        assert!(looks_like_tailscale_daemon("tailscaled"));
        assert!(looks_like_tailscale_daemon(
            "io.tailscale.ipn.macsys.network-extension"
        ));
        assert!(looks_like_tailscale_daemon("tailscaled.exe"));
        assert!(looks_like_tailscale_daemon("Tailscale.EXE"), "大小は無視");
        assert!(looks_like_tailscale_daemon("tailscale-ipn.exe"));
        // 名前が違えば通さない
        assert!(!looks_like_tailscale_daemon("evil"));
        assert!(!looks_like_tailscale_daemon("bash"));
        assert!(!looks_like_tailscale_daemon(""));
        // 想定外の導入形態は環境変数で足せる（設置場所ではなく**名前**を足す口）
        let _env = EnvGuard::set(TRUSTED_PEER_NAMES_ENV, " ts-proxy.exe ,, other ");
        assert!(looks_like_tailscale_daemon("ts-proxy"));
        assert!(looks_like_tailscale_daemon("other.exe"));
        assert!(!looks_like_tailscale_daemon("evil"));
    }

    #[test]
    fn udsエンドポイントは接続元検証を通さない() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // **プラットフォームではなくエンドポイントの形**で分岐する（受け入れ条件 4）。
        // UDS は socket 0600 + 親 0700 で別ユーザーの接続自体が起きない
        let uds = Endpoint::Unix(PathBuf::from("/tmp/x.sock"));
        assert_eq!(verify_peer(&uds, None), PeerTrust::NotRequired);
        assert_eq!(
            verify_peer(&uds, Some("127.0.0.1:1".parse().expect("addr"))),
            PeerTrust::NotRequired
        );
        assert!(verify_peer(&uds, None).accepts_forwarded_headers());
    }

    #[test]
    fn tailscaledでないローカルプロセスの接続は拒否される() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(LEGACY_ENV);
        std::env::remove_var(TRUSTED_PEER_NAMES_ENV);
        let conn = SelfConnection::new();
        // このテストプロセスは tailscaled ではない = 実行ファイルゲートで落ちる
        assert_eq!(
            verify_peer(&conn.endpoint, conn.peer()),
            PeerTrust::Rejected(PeerReject::NotTailscaleDaemon),
            "所有者は自分なので所有者ゲートは通り、名前で落ちる"
        );
        assert!(!verify_peer(&conn.endpoint, conn.peer()).accepts_forwarded_headers());
    }

    #[test]
    fn 信頼名に載せた接続元は通る() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(LEGACY_ENV);
        let conn = SelfConnection::new();
        // 「正規の tailscaled からの接続」の代理（実 tailscaled は root で動くので
        // テストからは起こせない。名前の集合だけを差し替えて経路全体を通す）
        let _env = EnvGuard::set(TRUSTED_PEER_NAMES_ENV, &SelfConnection::own_image_name());
        assert_eq!(
            verify_peer(&conn.endpoint, conn.peer()),
            PeerTrust::Trusted,
            "所有者ゲート + 実行ファイルゲートの両方を通れば信頼する"
        );
    }

    #[test]
    fn 材料が無ければ拒否側へ倒れる() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(LEGACY_ENV);
        let ep = Endpoint::Loopback(40001);
        // 接続元アドレスが取れない
        assert_eq!(
            verify_peer(&ep, None),
            PeerTrust::Rejected(PeerReject::NoPeerAddr)
        );
        // ループバック以外（bind は 127.0.0.1 限定なので本来起きないが fail-closed）
        assert_eq!(
            verify_peer(&ep, Some("10.0.0.1:40002".parse().expect("addr"))),
            PeerTrust::Rejected(PeerReject::NotLoopback)
        );
        // 存在しない接続（= 接続が既に閉じた後と同じ）。
        // **攻撃者が接続を即閉じても素通りしない**ことをここで固定する
        assert_eq!(
            verify_peer(&ep, Some("127.0.0.1:40002".parse().expect("addr"))),
            PeerTrust::Rejected(PeerReject::Unresolved)
        );
    }

    #[test]
    fn 閉じた接続でも所有者を取り違えない() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(LEGACY_ENV);
        std::env::remove_var(TRUSTED_PEER_NAMES_ENV);
        // ブロックを抜けた時点で両端が閉じる。
        // 実測（macOS）: 閉じた直後の 4 つ組は TIME_WAIT として表に残り、所有者は
        // **元の持ち主のまま**引ける。4 つ組が予約されているあいだ別プロセスが
        // 同じ組を名乗ることはできないので、取り違えは起きない。
        // 表から消えたあとは `Unresolved` = 拒否側（`材料が無ければ拒否側へ倒れる`）
        let (endpoint, peer) = {
            let conn = SelfConnection::new();
            (conn.endpoint.clone(), conn.peer())
        };
        let trust = verify_peer(&endpoint, peer);
        assert!(
            matches!(
                trust,
                PeerTrust::Rejected(PeerReject::NotTailscaleDaemon)
                    | PeerTrust::Rejected(PeerReject::Unresolved)
            ),
            "閉じた接続が信頼へ転ぶことは無い: {trust:?}"
        );
    }

    #[test]
    fn 検証結果はキャッシュしない() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(LEGACY_ENV);
        std::env::remove_var(TRUSTED_PEER_NAMES_ENV);
        let conn = SelfConnection::new();
        assert_eq!(
            verify_peer(&conn.endpoint, conn.peer()),
            PeerTrust::Rejected(PeerReject::NotTailscaleDaemon)
        );
        // 同じ接続でも**毎回引き直す**。`procinfo::image_path` は「いまのファイル名」を
        // 返す（#936）ので、pid が別の実行ファイルに置き換わった直後から結果が変わる。
        // キャッシュを挟むと、その瞬間だけ古い判定で通してしまう
        let _env = EnvGuard::set(TRUSTED_PEER_NAMES_ENV, &SelfConnection::own_image_name());
        assert_eq!(
            verify_peer(&conn.endpoint, conn.peer()),
            PeerTrust::Trusted,
            "同じ接続でも材料が変われば判定も変わる（= 前回の答えを使い回していない）"
        );
    }

    #[test]
    fn legacyは検証を切って旧挙動へ戻す() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(TRUSTED_PEER_NAMES_ENV);
        let conn = SelfConnection::new();
        assert!(matches!(
            verify_peer(&conn.endpoint, conn.peer()),
            PeerTrust::Rejected(_)
        ));
        let _env = EnvGuard::set(LEGACY_ENV, "1");
        assert!(legacy_mode());
        assert_eq!(
            verify_peer(&conn.endpoint, conn.peer()),
            PeerTrust::NotRequired,
            "A/B の逃げ道: 検証前の挙動（XFF を無条件に信じる）を再現する"
        );
    }

    #[test]
    fn 理由コードは安定していて中身を明かさない() {
        for r in [
            PeerReject::NoPeerAddr,
            PeerReject::NotLoopback,
            PeerReject::Unresolved,
            PeerReject::OwnerUntrusted,
            PeerReject::ImageUnknown,
            PeerReject::NotTailscaleDaemon,
        ] {
            assert!(!r.code().is_empty());
            assert!(r.code().is_ascii(), "コードはログ・JSON 用の ascii");
            assert!(!r.describe().is_empty());
            // 接続元のパス・IP・pid は説明に載せない（診断ログへ漏らさない）
            assert!(!r.describe().contains('/'));
            assert!(!r.describe().contains("127."));
        }
        assert_eq!(
            PeerReject::NotTailscaleDaemon.code(),
            "not_tailscale_daemon"
        );
    }

    #[test]
    fn 所有者ゲートの種類はプラットフォームで決まる() {
        // unix はソケットの所有ユーザーを引けるので強制できる。
        // Windows は引く手段が無いので「強制できない」と名乗る（status に出る）
        assert_eq!(
            owner_check_kind(),
            if cfg!(unix) { "uid" } else { "unavailable" }
        );
    }

    #[test]
    fn udsの表示は接頭辞unixを持つ() {
        let ep = Endpoint::Unix(PathBuf::from("/tmp/x.sock"));
        assert_eq!(ep.describe(), "unix:/tmp/x.sock");
        assert_eq!(ep.kind_str(), "unix");
        assert!(ep.port().is_none());
        assert_eq!(ep.socket_path(), Some(Path::new("/tmp/x.sock")));
    }
}
