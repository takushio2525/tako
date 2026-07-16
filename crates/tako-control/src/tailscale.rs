//! tailscale — Tailscale CLI の検出・setup 状態判定・serve 管理・ts.net URL 解決
//!
//! tako remote の transport は Tailscale Serve 一本（Issue #282 / 計画 §1）。
//! 状態判定・コマンド仕様の実測根拠は `.agent/investigations/tailscale-serve-poc.md`
//! （弾 0）が正。setup 状態の判定関数（`setup_status`）は `tako remote start` の
//! 起動前チェックと、弾 6 の `tako remote setup` ウィザードの両方が共有する。

use std::process::{Command, Stdio};

use serde_json::Value;

/// tailscale CLI の探索候補。PATH → brew 標準 → App Store 版 / brew cask 版
/// （App Store 版は .app 同梱バイナリが CLI を兼ねる。弾 0 項目 5）
const TAILSCALE_CANDIDATES: &[&str] = &[
    "tailscale",
    "/opt/homebrew/bin/tailscale",
    "/usr/local/bin/tailscale",
    "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
];

/// tailscale コマンドの実行タイムアウト。LocalAPI の Unix socket 呼び出しは
/// 通常数十 ms で返るが、デーモンの応答不能時に remote start を永久に
/// ブロックさせないための上限
const TAILSCALE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// tailscale CLI のパスを解決する。`TAKO_TAILSCALE_BIN` で差し替え可能
/// （テスト・検証用。存在しないパスを指定すれば「未導入」を偽装できる）
pub fn find_tailscale() -> Option<String> {
    if let Ok(bin) = std::env::var("TAKO_TAILSCALE_BIN") {
        // 明示指定は候補探索をせず、そのパスが実行可能かだけ確認する
        if runnable(&bin) {
            return Some(bin);
        }
        return None;
    }
    TAILSCALE_CANDIDATES
        .iter()
        .find(|c| runnable(c))
        .map(|c| c.to_string())
}

/// コマンドが実行可能か（`--version` が起動できるか）を確認する
fn runnable(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// setup の不足項目。`tako remote start` はこれが 1 つでもあれば起動を拒否し、
/// 弾 6 の `tako remote setup` ウィザードはこれを埋める手順を案内する
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissingItem {
    /// tailscale CLI が見つからない（未導入）
    CliNotFound,
    /// tailscaled デーモンが起動していない（LocalAPI に接続できない）
    DaemonNotRunning,
    /// 未ログイン（BackendState = NeedsLogin）
    NotLoggedIn,
    /// ログイン済みだが接続が有効でない（BackendState = Stopped 等）
    BackendNotRunning(String),
    /// tailnet の HTTPS 証明書（MagicDNS + HTTPS Certificates）が未有効
    HttpsNotEnabled,
}

impl MissingItem {
    /// 不足項目の 1 行説明（ユーザー向け表示用）
    pub fn describe(&self) -> String {
        match self {
            Self::CliNotFound => {
                "Tailscale が未導入（App Store 版アプリ または brew install tailscale）".into()
            }
            Self::DaemonNotRunning => {
                "Tailscale デーモンが起動していない（アプリを起動するか tailscaled を起動）"
                    .into()
            }
            Self::NotLoggedIn => {
                "Tailscale にログインしていない（tailscale up でブラウザ認証）".into()
            }
            Self::BackendNotRunning(state) => {
                format!("Tailscale の接続が有効でない（状態: {state}。tailscale up で再接続）")
            }
            Self::HttpsNotEnabled => "tailnet の HTTPS 証明書が未有効\
                 （https://login.tailscale.com/admin/dns で MagicDNS と HTTPS Certificates を有効化）"
                .into(),
        }
    }
}

/// Tailscale の setup 状態。`setup_status()` で取得する
#[derive(Debug, Clone, Default)]
pub struct SetupStatus {
    /// tailscale CLI のパス（未導入なら None）
    pub cli_path: Option<String>,
    /// tailscaled デーモンに接続できるか
    pub daemon_running: bool,
    /// ログイン済みで接続が有効か（BackendState = Running）
    pub logged_in: bool,
    /// `tailscale status --json` の BackendState（取得できた場合）
    pub backend_state: Option<String>,
    /// tailnet の HTTPS 証明書が有効か（CertDomains が非空）
    pub https_enabled: bool,
    /// このノードの MagicDNS 名（末尾ドット除去済み。例: `mac.tail1234.ts.net`）
    pub dns_name: Option<String>,
    /// 不足項目の列挙（空 = remote start 可能）
    pub missing: Vec<MissingItem>,
}

impl SetupStatus {
    /// remote start に必要な条件がすべて揃っているか
    pub fn ready(&self) -> bool {
        self.missing.is_empty()
    }

    /// このノードの恒久固定 URL（`https://<dns_name>`）。dns_name 未取得なら None
    pub fn ts_net_url(&self) -> Option<String> {
        self.dns_name.as_ref().map(|d| format!("https://{d}"))
    }
}

/// Tailscale の setup 状態を判定する。判定基準は弾 0 実測レポート項目 6 の表が正:
/// - 未導入: CLI が見つからない
/// - デーモン未起動: `tailscale status --json` が失敗（LocalAPI に接続できない）
/// - 未ログイン: BackendState = "NeedsLogin"
/// - HTTPS 未有効: CertDomains が null / 空
pub fn setup_status() -> SetupStatus {
    setup_status_with(find_tailscale())
}

/// setup_status の本体（CLI パスを引数化。テストで注入可能にするため分離）
pub fn setup_status_with(cli_path: Option<String>) -> SetupStatus {
    let mut status = SetupStatus::default();
    let Some(cli) = cli_path else {
        status.missing.push(MissingItem::CliNotFound);
        return status;
    };
    status.cli_path = Some(cli.clone());

    let output = match run_tailscale(&cli, &["status", "--json"]) {
        Ok(o) => o,
        Err(_) => {
            status.missing.push(MissingItem::DaemonNotRunning);
            return status;
        }
    };
    // `tailscale status --json` はデーモン未起動時に exit != 0 +
    // "failed to connect to local Tailscale service" を stderr に出す（弾 0 項目 6）
    if !output.status.success() {
        status.missing.push(MissingItem::DaemonNotRunning);
        return status;
    }
    status.daemon_running = true;

    let json: Value = match serde_json::from_slice(&output.stdout) {
        Ok(v) => v,
        Err(_) => {
            // JSON が壊れている = デーモン応答が異常。未起動と同じ扱いで停止させる
            status.missing.push(MissingItem::DaemonNotRunning);
            return status;
        }
    };
    apply_status_json(&mut status, &json);
    status
}

/// `tailscale status --json` のパース結果を SetupStatus に反映する（テスト可能な純関数部）
fn apply_status_json(status: &mut SetupStatus, json: &Value) {
    let backend_state = json["BackendState"].as_str().unwrap_or("");
    status.backend_state = Some(backend_state.to_string());
    match backend_state {
        "Running" => status.logged_in = true,
        "NeedsLogin" => {
            status.missing.push(MissingItem::NotLoggedIn);
            return;
        }
        other => {
            status
                .missing
                .push(MissingItem::BackendNotRunning(other.to_string()));
            return;
        }
    }

    // HTTPS 証明書: CertDomains が非空なら有効（弾 0 項目 6）
    status.https_enabled = json["CertDomains"]
        .as_array()
        .is_some_and(|a| !a.is_empty());
    // MagicDNS 名（末尾ドット付きで返る: "mac.tail1234.ts.net."）
    status.dns_name = json["Self"]["DNSName"]
        .as_str()
        .map(|d| d.trim_end_matches('.').to_string())
        .filter(|d| !d.is_empty());

    if !status.https_enabled || status.dns_name.is_none() {
        status.missing.push(MissingItem::HttpsNotEnabled);
    }
}

/// serve 設定の状態。`serve_proxy_target` で取得する
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServeState {
    /// serve 未設定
    NotConfigured,
    /// HTTPS:443 の "/" が単純プロキシとして設定済み（値はプロキシ先 URL）
    Proxy(String),
    /// tako の管理形式でない serve 設定が存在する（複数ハンドラ・パス分け等）
    Other,
}

/// `tailscale serve status --json` を読み、HTTPS:443 の serve 設定を判定する。
/// 弾 0 項目 6: serve 未設定なら `{}`（exit 0）が返る
pub fn serve_state(cli: &str) -> Result<ServeState, String> {
    let output = run_tailscale(cli, &["serve", "status", "--json"])?;
    if !output.status.success() {
        return Err(format!(
            "tailscale serve status が失敗: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let json: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("tailscale serve status の JSON を解釈できない: {e}"))?;
    Ok(parse_serve_state(&json))
}

/// serve status JSON から HTTPS:443 の状態を判定する（テスト可能な純関数部）。
/// JSON 形式（弾 0 実測）:
/// `{"TCP":{"443":{"HTTPS":true}},"Web":{"<host>.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:18080"}}}}}`
fn parse_serve_state(json: &Value) -> ServeState {
    let web = json["Web"].as_object();
    let Some(web) = web else {
        return ServeState::NotConfigured;
    };
    // :443 の Web エントリを探す（ホスト名は環境依存のためサフィックスで判定）
    let entry_443 = web.iter().find(|(k, _)| k.ends_with(":443"));
    let Some((_, entry)) = entry_443 else {
        // 443 以外（8443 等）だけの設定 = tako の管理形式でない
        return if web.is_empty() {
            ServeState::NotConfigured
        } else {
            ServeState::Other
        };
    };
    let Some(handlers) = entry["Handlers"].as_object() else {
        return ServeState::Other;
    };
    // tako の設定は "/" 1 本の単純プロキシのみ。それ以外は Other
    if handlers.len() != 1 {
        return ServeState::Other;
    }
    match handlers.get("/").and_then(|h| h["Proxy"].as_str()) {
        Some(proxy) => ServeState::Proxy(proxy.to_string()),
        None => ServeState::Other,
    }
}

/// tako が設定する serve のプロキシ先表現。serve_state の照合にも使う
pub fn proxy_target_for_port(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// `tailscale serve --bg --https=443 <target>` で serve を設定する。
/// 設定は tailscaled 側に永続化され、off するまで残る（弾 0 項目 3:
/// off → 再設定でも URL は不変）
pub fn serve_start(cli: &str, port: u16) -> Result<(), String> {
    let target = proxy_target_for_port(port);
    let output = run_tailscale(cli, &["serve", "--bg", "--https=443", &target])?;
    if !output.status.success() {
        return Err(format!(
            "tailscale serve の設定に失敗: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// HTTPS:443 の serve 設定を解除する。
/// 呼び出し側の契約: serve_state で tako 自身の設定（Proxy が自ポート）で
/// あることを確認してから呼ぶ（ユーザーの既存 serve 設定を壊さないため）
pub fn serve_stop(cli: &str) -> Result<(), String> {
    let output = run_tailscale(cli, &["serve", "--https=443", "off"])?;
    if !output.status.success() {
        return Err(format!(
            "tailscale serve の解除に失敗: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// serve 設定が指定ポートへの tako 管理プロキシである場合のみ解除する。
/// 解除した場合 true。tako の設定でなければ何もせず false
pub fn serve_stop_if_ours(cli: &str, port: u16) -> Result<bool, String> {
    match serve_state(cli)? {
        ServeState::Proxy(target) if target == proxy_target_for_port(port) => {
            serve_stop(cli)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// tailscale コマンドをタイムアウト付きで実行する。
/// stdout / stderr は別スレッドで drain し pipe deadlock を避ける（remote.rs H-5 と同型）
fn run_tailscale(cli: &str, args: &[&str]) -> Result<std::process::Output, String> {
    use std::io::Read;

    let mut child = Command::new(cli)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("tailscale の起動に失敗 ({cli}): {e}"))?;

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let stdout_handle = std::thread::Builder::new()
        .name("tailscale-stdout-drain".into())
        .spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = stdout_pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        })
        .map_err(|e| format!("stdout drain スレッドの起動に失敗: {e}"))?;
    let stderr_handle = std::thread::Builder::new()
        .name("tailscale-stderr-drain".into())
        .spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = stderr_pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        })
        .map_err(|e| format!("stderr drain スレッドの起動に失敗: {e}"))?;

    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if start.elapsed() > TAILSCALE_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "tailscale {} がタイムアウト（{}秒）",
                        args.first().unwrap_or(&""),
                        TAILSCALE_TIMEOUT.as_secs()
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => {
                let _ = child.kill();
                return Err(format!("tailscale の待機に失敗: {e}"));
            }
        }
    };

    let stdout = stdout_handle.join().unwrap_or_default();
    let stderr = stderr_handle.join().unwrap_or_default();
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn status_from(json: &Value) -> SetupStatus {
        let mut status = SetupStatus {
            cli_path: Some("/fake/tailscale".into()),
            daemon_running: true,
            ..Default::default()
        };
        apply_status_json(&mut status, json);
        status
    }

    #[test]
    fn 完全稼働ならreadyでdns_nameとurlが取れる() {
        let status = status_from(&json!({
            "BackendState": "Running",
            "CertDomains": ["mac.tail1234.ts.net"],
            "Self": { "DNSName": "mac.tail1234.ts.net." },
        }));
        assert!(status.ready(), "missing: {:?}", status.missing);
        assert!(status.logged_in);
        assert!(status.https_enabled);
        assert_eq!(status.dns_name.as_deref(), Some("mac.tail1234.ts.net"));
        assert_eq!(
            status.ts_net_url().as_deref(),
            Some("https://mac.tail1234.ts.net")
        );
    }

    #[test]
    fn needslogin_は未ログインを報告する() {
        let status = status_from(&json!({
            "BackendState": "NeedsLogin",
        }));
        assert!(!status.ready());
        assert_eq!(status.missing, vec![MissingItem::NotLoggedIn]);
        assert!(!status.logged_in);
    }

    #[test]
    fn stopped_は接続無効を状態名つきで報告する() {
        let status = status_from(&json!({
            "BackendState": "Stopped",
        }));
        assert_eq!(
            status.missing,
            vec![MissingItem::BackendNotRunning("Stopped".into())]
        );
    }

    #[test]
    fn certdomainsが無ければhttps未有効() {
        // null のケース（弾 0 項目 6: HTTPS 未有効化）
        let status = status_from(&json!({
            "BackendState": "Running",
            "CertDomains": null,
            "Self": { "DNSName": "mac.tail1234.ts.net." },
        }));
        assert!(!status.https_enabled);
        assert_eq!(status.missing, vec![MissingItem::HttpsNotEnabled]);

        // 空配列のケース
        let status = status_from(&json!({
            "BackendState": "Running",
            "CertDomains": [],
            "Self": { "DNSName": "mac.tail1234.ts.net." },
        }));
        assert_eq!(status.missing, vec![MissingItem::HttpsNotEnabled]);
    }

    #[test]
    fn dnsnameが空でもhttps未有効として停止する() {
        let status = status_from(&json!({
            "BackendState": "Running",
            "CertDomains": ["x.ts.net"],
            "Self": { "DNSName": "" },
        }));
        assert_eq!(status.missing, vec![MissingItem::HttpsNotEnabled]);
        assert!(status.dns_name.is_none());
    }

    #[test]
    fn cli不在はclinotfoundのみを返す() {
        let status = setup_status_with(None);
        assert_eq!(status.missing, vec![MissingItem::CliNotFound]);
        assert!(status.cli_path.is_none());
        assert!(!status.daemon_running);
    }

    #[test]
    fn 実行不能なパスはデーモン未起動を返す() {
        let status = setup_status_with(Some("/nonexistent/tailscale-bin".into()));
        assert_eq!(status.missing, vec![MissingItem::DaemonNotRunning]);
        assert_eq!(
            status.cli_path.as_deref(),
            Some("/nonexistent/tailscale-bin")
        );
    }

    #[test]
    fn serve_stateは未設定と単純プロキシと他形式を区別する() {
        // 未設定（弾 0: `{}` が返る）
        assert_eq!(parse_serve_state(&json!({})), ServeState::NotConfigured);
        assert_eq!(
            parse_serve_state(&json!({ "Web": {} })),
            ServeState::NotConfigured
        );

        // tako 形式の単純プロキシ
        let serve = json!({
            "TCP": { "443": { "HTTPS": true } },
            "Web": {
                "mac.tail1234.ts.net:443": {
                    "Handlers": { "/": { "Proxy": "http://127.0.0.1:7749" } }
                }
            }
        });
        assert_eq!(
            parse_serve_state(&serve),
            ServeState::Proxy("http://127.0.0.1:7749".into())
        );

        // パス分けハンドラ = 他形式
        let multi = json!({
            "Web": {
                "mac.tail1234.ts.net:443": {
                    "Handlers": {
                        "/": { "Proxy": "http://127.0.0.1:7749" },
                        "/other": { "Path": "/srv" }
                    }
                }
            }
        });
        assert_eq!(parse_serve_state(&multi), ServeState::Other);

        // 443 以外のポートだけ = 他形式
        let alt_port = json!({
            "Web": {
                "mac.tail1234.ts.net:8443": {
                    "Handlers": { "/": { "Proxy": "http://127.0.0.1:9999" } }
                }
            }
        });
        assert_eq!(parse_serve_state(&alt_port), ServeState::Other);

        // "/" が Proxy でない（静的パス配信）= 他形式
        let path_serve = json!({
            "Web": {
                "mac.tail1234.ts.net:443": {
                    "Handlers": { "/": { "Path": "/srv/www" } }
                }
            }
        });
        assert_eq!(parse_serve_state(&path_serve), ServeState::Other);
    }

    #[test]
    fn proxy_target_for_portの形式() {
        assert_eq!(proxy_target_for_port(7749), "http://127.0.0.1:7749");
    }

    #[test]
    fn missing_itemのdescribeは対処を含む() {
        assert!(MissingItem::CliNotFound.describe().contains("brew"));
        assert!(MissingItem::NotLoggedIn.describe().contains("tailscale up"));
        assert!(MissingItem::HttpsNotEnabled.describe().contains("MagicDNS"));
        assert!(MissingItem::BackendNotRunning("Stopped".into())
            .describe()
            .contains("Stopped"));
    }
}
