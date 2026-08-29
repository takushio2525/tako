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
    // #586: GUI プロセスから到達するのでコンソールウィンドウを出させない
    tako_core::platform::process::no_console_window(&mut Command::new(bin))
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
    /// 応答した tailscaled のバージョン（`tailscale status --json` の `Version`）。
    /// CLI 側のバージョンと食い違うときは 2 系統の Tailscale が同居している（#1038）
    pub daemon_version: Option<String>,
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
    let variant = selected_variant();
    setup_status_on(cli_path, variant.socket_arg())
}

/// 話しかける tailscaled を明示した setup_status（系統の検出に使う。#1038）
pub fn setup_status_on(cli_path: Option<String>, socket: Option<&str>) -> SetupStatus {
    let mut status = SetupStatus::default();
    let Some(cli) = cli_path else {
        status.missing.push(MissingItem::CliNotFound);
        return status;
    };
    status.cli_path = Some(cli.clone());

    let output = match run_tailscale_on(&cli, socket, &["status", "--json"]) {
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
    // 応答したデーモンのバージョン（同居検出の材料。#1038）
    status.daemon_version = json["Version"]
        .as_str()
        .map(|v| v.to_string())
        .filter(|v| !v.is_empty());
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

/// UDS 版のプロキシ先表現。`tailscale serve unix:<path>` の引数そのまま
pub fn proxy_target_for_socket(path: &std::path::Path) -> String {
    format!("unix:{}", path.display())
}

/// `tailscale whois` で得た接続元ノードの情報（層①の identity 検証。#283）。
/// フィールドは 2026-07-17 実測の JSON 形式が正（`Node.StableID` / `UserProfile.LoginName`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhoisInfo {
    /// ノードの恒久 ID（`Node.StableID`）。機器ペアリングのデバイス識別子に使う
    pub stable_id: String,
    /// ノードの MagicDNS 名（`Node.Name`、末尾ドット除去済み）
    pub node_name: String,
    /// ノードのマシン名（`Node.Hostinfo.Hostname`。無ければ node_name の先頭ラベル）
    pub hostname: String,
    /// ノード所有ユーザーのログイン名（`UserProfile.LoginName`）
    pub login: String,
}

/// whois の失敗理由。PeerNotFound（= serve 経由でない直結 or 偽装 IP）と
/// 実行エラーを区別する（前者は認証拒否、後者は 503 相当）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhoisError {
    /// 指定 IP は tailnet 上のピアではない（`peer not found`）
    PeerNotFound,
    /// tailscale コマンドの実行失敗・出力の解釈失敗
    Failed(String),
}

/// `tailscale whois --json <ip>` で接続元 IP のノード情報を取得する。
/// serve が付与する `X-Forwarded-For` の IP を照合し、tailnet 上の実在ノードで
/// あることを検証する（弾 0 実測: ローカル直結の 127.0.0.1 等は `peer not found`）
pub fn whois(cli: &str, ip: &str) -> Result<WhoisInfo, WhoisError> {
    let output = run_tailscale(cli, &["whois", "--json", ip]).map_err(WhoisError::Failed)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stderr.contains("peer not found") || stdout.contains("peer not found") {
            return Err(WhoisError::PeerNotFound);
        }
        return Err(WhoisError::Failed(format!(
            "tailscale whois が失敗: {}",
            stderr.trim()
        )));
    }
    let json: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| WhoisError::Failed(format!("tailscale whois の JSON を解釈できない: {e}")))?;
    parse_whois(&json)
}

/// whois JSON から WhoisInfo を組み立てる（テスト可能な純関数部）
fn parse_whois(json: &Value) -> Result<WhoisInfo, WhoisError> {
    let stable_id = json["Node"]["StableID"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| WhoisError::Failed("whois 応答に Node.StableID が無い".into()))?
        .to_string();
    let node_name = json["Node"]["Name"]
        .as_str()
        .unwrap_or("")
        .trim_end_matches('.')
        .to_string();
    let hostname = json["Node"]["Hostinfo"]["Hostname"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            node_name
                .split('.')
                .next()
                .unwrap_or(&node_name)
                .to_string()
        });
    let login = json["UserProfile"]["LoginName"]
        .as_str()
        .unwrap_or("")
        .to_string();
    Ok(WhoisInfo {
        stable_id,
        node_name,
        hostname,
        login,
    })
}

/// `tailscale serve --bg --https=443 <target>` で serve を設定する。
/// 設定は tailscaled 側に永続化され、off するまで残る（弾 0 項目 3:
/// off → 再設定でも URL は不変）。
/// target は `proxy_target_for_port` / `proxy_target_for_socket` が組む文字列
pub fn serve_start_target(cli: &str, target: &str) -> Result<(), String> {
    let output = run_tailscale(cli, &["serve", "--bg", "--https=443", target])?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "tailscale serve の設定に失敗: {}{}",
            stderr.trim(),
            unix_target_hint(target)
        ));
    }
    Ok(())
}

/// unix: target 固有の追加ヒント。Windows の tailscale は unix socket target を
/// 持たない（#971）ので、失敗時に何をすればよいかまで書く
fn unix_target_hint(target: &str) -> &'static str {
    if target.starts_with("unix:") {
        "（この tailscale は unix socket の serve target に対応していない可能性があります。\
         TAKO_REMOTE_ENDPOINT の指定を外すとループバック TCP で待ち受けます）"
    } else {
        ""
    }
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

/// serve 設定が tako の管理形式であれば解除する。解除したら true。
/// `ours` は自分がいま使っている target（None = 自分の target が分からない状況でも
/// tako 形状の残骸だけは回収する）。ユーザーの独自設定には触れない
pub fn serve_stop_if_ours(cli: &str, ours: Option<&str>) -> Result<bool, String> {
    match serve_state(cli)? {
        ServeState::Proxy(ref target) if is_reclaimable_target(target, ours) => {
            serve_stop(cli)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// 既存 serve の target を tako 管理のもの（= 解除・張り替えてよい）と見なすか。
/// 判定は純関数なので Windows / macOS の両方の形をどこからでも検証できる
pub fn is_reclaimable_target(target: &str, ours: Option<&str>) -> bool {
    if ours == Some(target) {
        return true;
    }
    // tako が張る形は 2 つだけ:
    // ① ループバック TCP（ポートは毎回変わるので前回のポートは自分の残骸）
    // ② tako の socket ファイルを指す UDS（#1038 前の形。置き場が変わっていても拾う）
    // **ユーザーが自分で張った unix: 設定は掴まない**（socket 名で絞る）
    target.starts_with("http://127.0.0.1:")
        || (target.starts_with("unix:") && target.ends_with(TAKO_SOCKET_FILE_NAME))
}

/// tako の remote デーモンが使う socket のファイル名（`remote::socket_path` の末尾）。
/// serve 設定が「tako の残骸か」を名前で判定するために持つ
const TAKO_SOCKET_FILE_NAME: &str = "tako-remote.sock";

// --- Tailscale 系統（GUI 版 / standalone）の検出と選択（#1038）---------------
//
// macOS では 2 系統の Tailscale が同時に動きうる:
//   - GUI 版（`/Applications/Tailscale.app` のシステム拡張）= サンドボックス。
//     LocalAPI は CLI の既定探索で見つかる
//   - standalone（`tailscaled` を自分で動かす形）= LocalAPI は unix socket
// 両方入っていると **別デーモン・別ノード**として tailnet に二重登録される（実測）。
// どちらへ話しかけるかは `tailscale --socket <path>` で選べる（実測で確認）。
//
// tako は**決め打ちしない**: 1 つしか無ければそれ、2 つあれば選ばせる
// （非対話では「現にノード実体になっている方」を選び、根拠と変更手段を返す）。

/// standalone tailscaled の LocalAPI socket の既定パス候補。
/// 先に見つかったものを使う（macOS / Linux の配布で実際に使われている場所）
const STANDALONE_SOCKET_CANDIDATES: &[&str] = &[
    "/var/run/tailscaled.socket",
    "/var/run/tailscale/tailscaled.sock",
];

/// どの tailscaled に話しかけるか。
///
/// **tako が明示的に指定できるのは standalone 側だけ**（`--socket <path>`）。
/// GUI 版へ話しかける手段は「CLI の既定探索に任せる」ことなので、
/// `Default` は「GUI 版があればそれ / 無ければ standalone」を意味する。
/// 未選択を `gui` と名乗ると standalone しか無い機で嘘になるため、識別子は `auto`
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TailscaleVariant {
    /// CLI の既定探索に任せる。GUI 版が入っていればそれが応答する
    #[default]
    Default,
    /// standalone tailscaled の LocalAPI socket を明示する
    Standalone(String),
}

impl TailscaleVariant {
    /// `--socket` に渡す値（既定探索なら None）
    pub fn socket_arg(&self) -> Option<&str> {
        match self {
            Self::Default => None,
            Self::Standalone(path) => Some(path.as_str()),
        }
    }

    /// 設定ファイル / CLI 引数で使う短い識別子
    pub fn key(&self) -> &'static str {
        match self {
            Self::Default => "auto",
            Self::Standalone(_) => "standalone",
        }
    }

    /// 人間向けの説明
    pub fn describe(&self) -> String {
        match self {
            Self::Default => {
                "既定探索（GUI 版アプリが入っていればそれ / 無ければ standalone）".into()
            }
            Self::Standalone(path) => format!("standalone tailscaled（--socket {path}）"),
        }
    }

    /// 識別子から復元する。`standalone` は実在する socket を探して解決する
    pub fn parse(key: &str) -> Option<Self> {
        match key.trim().to_ascii_lowercase().as_str() {
            "auto" | "gui" | "default" | "app" => Some(Self::Default),
            "standalone" | "daemon" | "cli" => {
                Some(Self::Standalone(standalone_socket_path()?.to_string()))
            }
            _ => None,
        }
    }
}

/// 実在する standalone socket のパス（無ければ None）。
/// Windows には standalone / GUI の区別が無いので常に None
pub fn standalone_socket_path() -> Option<&'static str> {
    if cfg!(windows) {
        return None;
    }
    STANDALONE_SOCKET_CANDIDATES
        .iter()
        .find(|p| std::path::Path::new(p).exists())
        .copied()
}

/// 選択を保存するファイル。remote の state ディレクトリに 1 行のテキストで置く
/// （`tako-remote.url` / `.token` と同じ扱い。構造体を持たないので #916 の
/// スキーマ移行の対象にならない = 値は `gui` / `standalone` の 2 語だけ）
pub fn selection_path() -> std::path::PathBuf {
    crate::remote::state_dir().join("tailscale-variant")
}

/// 保存済みの選択を読む（未保存・壊れている場合は None）
pub fn saved_variant() -> Option<TailscaleVariant> {
    let raw = std::fs::read_to_string(selection_path()).ok()?;
    TailscaleVariant::parse(&raw)
}

/// 選択を保存する。以後の `tako remote start` はこの系統へ話しかける
pub fn save_variant(variant: &TailscaleVariant) -> Result<(), String> {
    let dir = selection_path()
        .parent()
        .map(|d| d.to_path_buf())
        .ok_or("state ディレクトリを解決できない")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("state ディレクトリの作成に失敗: {e}"))?;
    crate::remote::write_state_file(&selection_path(), variant.key())
        .map_err(|e| format!("Tailscale 系統の保存に失敗: {e}"))
}

/// 保存済みの選択を消す（自動判定へ戻す）
pub fn clear_variant() {
    let _ = std::fs::remove_file(selection_path());
}

/// 状態表示用のラベル。選択済みならその識別子、未選択なら `auto`、
/// `TAKO_TAILSCALE_SOCKET` による直接指定なら `override`
pub fn selection_label() -> &'static str {
    if std::env::var("TAKO_TAILSCALE_SOCKET").is_ok() {
        return "override";
    }
    match saved_variant() {
        Some(v) => v.key(),
        None => "auto",
    }
}

/// いま使う系統。優先順は 環境変数 → 保存済みの選択 → 既定探索。
/// `TAKO_TAILSCALE_SOCKET` は検証用の直接指定（空文字なら既定探索を強制）
pub fn selected_variant() -> TailscaleVariant {
    if let Ok(sock) = std::env::var("TAKO_TAILSCALE_SOCKET") {
        return if sock.is_empty() {
            TailscaleVariant::Default
        } else {
            TailscaleVariant::Standalone(sock)
        };
    }
    saved_variant().unwrap_or_default()
}

/// 検出した 1 系統
#[derive(Debug, Clone)]
pub struct VariantProbe {
    pub variant: TailscaleVariant,
    pub status: SetupStatus,
}

impl VariantProbe {
    pub fn node(&self) -> Option<&str> {
        self.status.dns_name.as_deref()
    }
    pub fn version(&self) -> Option<&str> {
        self.status.daemon_version.as_deref()
    }
    /// remote を張れる状態か（ログイン済み + HTTPS 証明書あり）
    pub fn ready(&self) -> bool {
        self.status.ready()
    }
    /// 1 行の要約（選択肢の表示用）
    pub fn summary(&self) -> String {
        let node = self.node().unwrap_or("(ノード名不明)");
        let ver = self.version().unwrap_or("?");
        let state = if self.ready() {
            "利用可能".to_string()
        } else {
            let items: Vec<String> = self.status.missing.iter().map(|m| m.describe()).collect();
            format!("利用不可: {}", items.join(" / "))
        };
        format!(
            "{} — ノード {node} / v{ver} / {state}",
            self.variant.describe()
        )
    }
}

/// 系統の検出結果
#[derive(Debug, Clone, Default)]
pub struct VariantSurvey {
    /// 到達できた系統（既定探索 → standalone の順）
    pub probes: Vec<VariantProbe>,
    /// 2 系統が別ノードとして同時に動いているか
    pub coexisting: bool,
}

impl VariantSurvey {
    pub fn get(&self, key: &str) -> Option<&VariantProbe> {
        self.probes.iter().find(|p| p.variant.key() == key)
    }
}

/// 同時稼働の判定（純関数）。到達できた 2 系統のノード名かバージョンが
/// 食い違えば「別デーモンが 2 つ動いている」= 二重ノード化している
pub fn is_coexisting(a: &SetupStatus, b: &SetupStatus) -> bool {
    let node_differs = match (a.dns_name.as_deref(), b.dns_name.as_deref()) {
        (Some(x), Some(y)) => x != y,
        _ => false,
    };
    let version_differs = match (a.daemon_version.as_deref(), b.daemon_version.as_deref()) {
        (Some(x), Some(y)) => x != y,
        _ => false,
    };
    node_differs || version_differs
}

/// 導入されている系統を検出する。`tailscale status --json` を最大 2 回叩くだけで、
/// プロセス一覧やアプリの実体パスには依存しない（環境差に強い形）
pub fn survey_variants() -> VariantSurvey {
    let cli = find_tailscale();
    let mut survey = VariantSurvey::default();
    let Some(cli) = cli else {
        return survey;
    };
    // ① CLI の既定探索（GUI 版が入っていればそれが応答する）
    let default_status = setup_status_on(Some(cli.clone()), None);
    let default_ok = default_status.daemon_running;
    if default_ok {
        survey.probes.push(VariantProbe {
            variant: TailscaleVariant::Default,
            status: default_status.clone(),
        });
    }
    // ② standalone の LocalAPI socket を明示
    if let Some(sock) = standalone_socket_path() {
        let st = setup_status_on(Some(cli), Some(sock));
        if st.daemon_running {
            if default_ok && is_coexisting(&default_status, &st) {
                survey.coexisting = true;
                survey.probes.push(VariantProbe {
                    variant: TailscaleVariant::Standalone(sock.to_string()),
                    status: st,
                });
            } else if !default_ok {
                survey.probes.push(VariantProbe {
                    variant: TailscaleVariant::Standalone(sock.to_string()),
                    status: st,
                });
            }
            // 既定探索と同じノードなら「同じデーモンを 2 通りで指しただけ」= 1 系統
        }
    }
    survey
}

/// 非対話で 1 つに決めるときの判断材料（純関数にするため最小化した要約）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantCandidate {
    pub key: &'static str,
    pub ready: bool,
    /// 既定探索で応答した系統か（= OS がノード実体として扱っている方）
    pub is_default_discovery: bool,
    pub node: Option<String>,
}

/// 非対話での選択（純関数）。**GUI を無条件で優先しない**:
/// ① 使える系統が 1 つだけならそれ
/// ② 複数使えるなら「既定探索が応答した系統」= 現にノード実体になっている方
/// ③ 使える系統が無ければ None（呼び出し側が不足項目を列挙して止める）
pub fn choose_variant(candidates: &[VariantCandidate]) -> Option<(&'static str, String)> {
    let usable: Vec<&VariantCandidate> = candidates.iter().filter(|c| c.ready).collect();
    match usable.len() {
        0 => None,
        1 => {
            let c = usable[0];
            let others = candidates.len() - 1;
            let reason = if others > 0 {
                format!(
                    "利用できる Tailscale 系統がこれだけのため（ノード {}）",
                    c.node.as_deref().unwrap_or("不明")
                )
            } else {
                format!(
                    "検出された Tailscale 系統がこれだけのため（ノード {}）",
                    c.node.as_deref().unwrap_or("不明")
                )
            };
            Some((c.key, reason))
        }
        _ => {
            let pick = usable
                .iter()
                .find(|c| c.is_default_discovery)
                .copied()
                .unwrap_or(usable[0]);
            Some((
                pick.key,
                format!(
                    "複数の Tailscale が同時に動いているため、現にノード実体として\
                     応答している系統（ノード {}）を選びました",
                    pick.node.as_deref().unwrap_or("不明")
                ),
            ))
        }
    }
}

/// 二重稼働の警告文（`tako remote status` の warnings に載せる）
pub fn coexistence_warning(survey: &VariantSurvey) -> Option<String> {
    if !survey.coexisting {
        return None;
    }
    let lines: Vec<String> = survey.probes.iter().map(|p| p.summary()).collect();
    let selected = selected_variant();
    Some(format!(
        "Tailscale が 2 系統同時に動いています（別ノードとして二重登録されます）:\n  - {}\n\
         いま使っているのは {}。`tako remote setup --tailscale <auto|standalone>` で切り替えられます。",
        lines.join("\n  - "),
        selected.describe()
    ))
}

/// tailscale コマンドをタイムアウト付きで実行する。
/// stdout / stderr は別スレッドで drain し pipe deadlock を避ける（remote.rs H-5 と同型）
fn run_tailscale(cli: &str, args: &[&str]) -> Result<std::process::Output, String> {
    let variant = selected_variant();
    run_tailscale_on(cli, variant.socket_arg(), args)
}

/// 話しかける tailscaled を明示して実行する（#1038: GUI 版 / standalone の同居）。
/// `socket` が `Some` なら `--socket <path>` を前置きする（CLI のグローバルフラグ）
pub(crate) fn run_tailscale_on(
    cli: &str,
    socket: Option<&str>,
    args: &[&str],
) -> Result<std::process::Output, String> {
    use std::io::Read;

    let mut argv: Vec<&str> = Vec::with_capacity(args.len() + 2);
    if let Some(sock) = socket {
        argv.push("--socket");
        argv.push(sock);
    }
    argv.extend_from_slice(args);

    // #586: GUI プロセスから到達するのでコンソールウィンドウを出させない
    let mut child = tako_core::platform::process::no_console_window(&mut Command::new(cli))
        .args(&argv)
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
    fn proxy_target_for_socketの形式() {
        let path = std::path::Path::new("/tmp/tako-remote.sock");
        assert_eq!(proxy_target_for_socket(path), "unix:/tmp/tako-remote.sock");
        let space_path = std::path::Path::new(
            "/Users/test/Library/Application Support/tako/remote/tako-remote.sock",
        );
        assert_eq!(
            proxy_target_for_socket(space_path),
            "unix:/Users/test/Library/Application Support/tako/remote/tako-remote.sock"
        );
    }

    #[test]
    fn serve_stateはunixソケットプロキシを認識する() {
        let serve = json!({
            "TCP": { "443": { "HTTPS": true } },
            "Web": {
                "mac.tail1234.ts.net:443": {
                    "Handlers": { "/": { "Proxy": "unix:/Users/test/Library/Application Support/tako/remote/tako-remote.sock" } }
                }
            }
        });
        assert_eq!(
            parse_serve_state(&serve),
            ServeState::Proxy(
                "unix:/Users/test/Library/Application Support/tako/remote/tako-remote.sock".into()
            )
        );
    }

    #[test]
    fn whoisのjsonからノード情報を組み立てる() {
        // 2026-07-17 実測の whois --json 形式（キーは抜粋）
        let json = json!({
            "Node": {
                "StableID": "nABCDEF123CNTRL",
                "Name": "iphone.tail1234.ts.net.",
                "Hostinfo": { "Hostname": "iPhone" },
            },
            "UserProfile": { "LoginName": "user@example.com" },
        });
        let info = parse_whois(&json).expect("パース成功");
        assert_eq!(info.stable_id, "nABCDEF123CNTRL");
        assert_eq!(info.node_name, "iphone.tail1234.ts.net");
        assert_eq!(info.hostname, "iPhone");
        assert_eq!(info.login, "user@example.com");
    }

    #[test]
    fn whoisのhostname欠落はnode_nameの先頭ラベルへフォールバックする() {
        let json = json!({
            "Node": {
                "StableID": "nXYZ",
                "Name": "ipad.tail1234.ts.net.",
                "Hostinfo": {},
            },
            "UserProfile": { "LoginName": "u@e.com" },
        });
        let info = parse_whois(&json).expect("パース成功");
        assert_eq!(info.hostname, "ipad");
    }

    #[test]
    fn whoisのstableid欠落はエラー() {
        let json = json!({ "Node": {}, "UserProfile": {} });
        assert!(matches!(parse_whois(&json), Err(WhoisError::Failed(_))));
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

    // --- #1038: 系統の検出・選択 -------------------------------------------

    #[test]
    fn tako形状のserve設定だけを張り替え対象と見なす() {
        // 自分がいま使っている target は当然回収してよい
        assert!(is_reclaimable_target(
            "http://127.0.0.1:100",
            Some("http://127.0.0.1:100")
        ));
        // tako が過去に張った形（UDS / ループバック TCP）は自分の残骸として回収する
        assert!(is_reclaimable_target("unix:/x/tako-remote.sock", None));
        assert!(is_reclaimable_target("http://127.0.0.1:65000", None));
        // ユーザーの独自設定には触らない
        assert!(!is_reclaimable_target("http://192.168.1.5:8080", None));
        assert!(!is_reclaimable_target("https://example.com", None));
        assert!(!is_reclaimable_target("http://localhost:3000", None));
        // ユーザーが自分で張った unix: 設定も掴まない（socket 名で絞る）
        assert!(!is_reclaimable_target("unix:/srv/my-app.sock", None));
    }

    #[test]
    fn unix_targetの失敗にだけ追加ヒントが付く() {
        assert!(unix_target_hint("unix:/x.sock").contains("TAKO_REMOTE_ENDPOINT"));
        assert_eq!(unix_target_hint("http://127.0.0.1:1"), "");
    }

    #[test]
    fn 既定探索の識別子はauto_で_guiは別名() {
        // 「既定探索」を gui と名乗ると standalone しか無い機で嘘になる。
        // ただしユーザーは「GUI 版を使う」と言うので別名として受理する
        assert_eq!(TailscaleVariant::Default.key(), "auto");
        assert_eq!(
            TailscaleVariant::parse("gui"),
            Some(TailscaleVariant::Default)
        );
        assert_eq!(
            TailscaleVariant::parse("auto"),
            Some(TailscaleVariant::Default)
        );
        assert!(!TailscaleVariant::Default.describe().starts_with("GUI 版"));
    }

    #[test]
    fn 系統の識別子は往復する() {
        assert_eq!(
            TailscaleVariant::parse("gui"),
            Some(TailscaleVariant::Default)
        );
        assert_eq!(
            TailscaleVariant::parse("Default"),
            Some(TailscaleVariant::Default)
        );
        assert_eq!(TailscaleVariant::Default.key(), "auto");
        assert_eq!(TailscaleVariant::Default.socket_arg(), None);
        assert_eq!(
            TailscaleVariant::Standalone("/var/run/x.sock".into()).socket_arg(),
            Some("/var/run/x.sock")
        );
        assert_eq!(
            TailscaleVariant::Standalone("/var/run/x.sock".into()).key(),
            "standalone"
        );
        assert_eq!(TailscaleVariant::parse("なにか"), None);
    }

    #[test]
    fn 別ノードか別バージョンなら同時稼働と判定する() {
        let mk = |node: &str, ver: &str| SetupStatus {
            dns_name: Some(node.into()),
            daemon_version: Some(ver.into()),
            ..Default::default()
        };
        // 実測の形: GUI 版と standalone は別ノード名・別バージョンで並ぶ
        // （ノード名は重複時に `-1` サフィックスが付く）
        assert!(is_coexisting(
            &mk("testhost-1.tail0.ts.net", "1.102.2"),
            &mk("testhost-2.tail0.ts.net", "1.98.8")
        ));
        // 同じデーモンを 2 通りで指しただけなら同時稼働ではない
        assert!(!is_coexisting(
            &mk("mac.tail0.ts.net", "1.98.8"),
            &mk("mac.tail0.ts.net", "1.98.8")
        ));
        // 材料が欠けていたら断定しない（誤警告を出さない）
        assert!(!is_coexisting(
            &mk("mac.tail0.ts.net", "1.98.8"),
            &SetupStatus::default()
        ));
    }

    fn candidate(key: &'static str, ready: bool, default_discovery: bool) -> VariantCandidate {
        VariantCandidate {
            key,
            ready,
            is_default_discovery: default_discovery,
            node: Some(format!("{key}-node")),
        }
    }

    #[test]
    fn 使える系統が1つならそれを選ぶ() {
        // standalone だけの機（GUI 版が入っていない人）は standalone に落ちる
        let picked = choose_variant(&[candidate("standalone", true, true)]).unwrap();
        assert_eq!(picked.0, "standalone");
        assert!(picked.1.contains("standalone-node"), "{}", picked.1);
        // GUI 版だけの機は GUI に落ちる
        assert_eq!(
            choose_variant(&[candidate("gui", true, true)]).unwrap().0,
            "gui"
        );
    }

    #[test]
    fn 使えない系統は候補から外す() {
        // GUI 版が未ログインなら standalone を選ぶ（GUI を無条件に優先しない）
        let picked = choose_variant(&[
            candidate("gui", false, true),
            candidate("standalone", true, false),
        ])
        .unwrap();
        assert_eq!(picked.0, "standalone");
    }

    #[test]
    fn 両方使えるときはノード実体として応答している方を選ぶ() {
        let picked = choose_variant(&[
            candidate("gui", true, true),
            candidate("standalone", true, false),
        ])
        .unwrap();
        assert_eq!(picked.0, "gui", "既定探索が応答している方");
        assert!(picked.1.contains("複数"), "{}", picked.1);
        // 既定探索が standalone 側なら standalone が選ばれる（GUI 決め打ちではない）
        let picked = choose_variant(&[
            candidate("gui", true, false),
            candidate("standalone", true, true),
        ])
        .unwrap();
        assert_eq!(picked.0, "standalone");
    }

    #[test]
    fn 使える系統が無ければ選ばない() {
        assert!(choose_variant(&[]).is_none());
        assert!(choose_variant(&[candidate("gui", false, true)]).is_none());
    }

    #[test]
    fn 同時稼働していなければ警告を出さない() {
        let survey = VariantSurvey::default();
        assert!(coexistence_warning(&survey).is_none());
    }
}
