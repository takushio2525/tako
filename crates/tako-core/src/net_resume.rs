//! ネットワークが戻ったら、接続エラーで止まったエージェントのセッションをすぐ再開する
//! （FR-2.27.17 / Issue #2112）の純粋ロジック。
//!
//! 回線が切れると claude は 1 ターンの中では再試行するが、諦めて
//! `API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)` で
//! ターンを終えたあとは、人が何かを送るまで止まったままになる（2026-10-15 の回線断では
//! worker 4 本が回線の復帰後も 6 時間止まっていた）。tako が回線の復帰を見て「続き」を送る。
//!
//! # ここに置くもの / 置かないもの
//!
//! - 置く: 接続系エラーの文言の分類・発動判断・到達確認の間隔と先・再開の文面（すべて純関数）
//! - 置かない: 画面から停止を切り出す処理と到達確認の I/O（`tako-control::net_stop`）・
//!   送達（tako-app。#813 の自動復帰と同じ送達確認つきの経路）
//!
//! # 安全側に倒す設計
//!
//! - **既定 ON**（ユーザー判断 2026-10-15）。ただし**止まったセッションが無いあいだは
//!   到達確認を 1 回も走らせない**（常時のポーリングで CPU / 電力を食わない）
//! - **接続系だけ**を対象にする。使用制限（#813 の担当）・ログイン失効（#757）・
//!   利用阻害（#1106）・サーバー側の 5xx / 過負荷は含めない（回線が戻っても直らない）
//! - 人の下書きが入力欄にある・送達中・選択ダイアログ中のペインには送らない
//! - **同じ停止に 1 回**。続けて何度も止まるペインは [`MAX_RESUMES`] で打ち切る

use crate::i18n::{lang, Lang};

/// 設定の既定（settings.json の `network_resume`。項目の無い旧ファイルもこの値で読む）
pub const DEFAULT_ENABLED: bool = true;

/// 同じ停止を見続けてから再開してよくなるまでの秒数（止まった直後の一瞬の画面へ送らない）。
/// 回線が切れているあいだは停止が長く続くので、これが効くのは「止まった時点で
/// もう回線が戻っていた」ときだけ（短くしてある）
pub const STABLE_SECS: i64 = 3;

/// 到達確認 1 回の接続の待ち時間（ミリ秒）
pub const PROBE_TIMEOUT_MS: u64 = 3000;

/// 1 ペインが [`RESUME_WINDOW_SECS`] のうちに自動で再開される上限。
/// 到達確認の先へは届くのにエージェントの API だけ届かない環境で、
/// 「続き」→ 失敗 → 「続き」を無限に繰り返さないための歯止め
pub const MAX_RESUMES: usize = 3;

/// [`MAX_RESUMES`] を数える窓（秒）
pub const RESUME_WINDOW_SECS: i64 = 1800;

/// 到達確認の先の既定（claude の API）
pub const DEFAULT_PROBE_HOST: &str = "api.anthropic.com";

/// 到達確認の先を差し替える env（`host:port`。検証の注入と、既定の先へ直接出られない環境の逃げ道）
pub const PROBE_ENV: &str = "TAKO_NET_PROBE";

// --- 接続系エラーの分類 ---

/// 接続系エラーの種別。persist.log へは**画面の文字ではなくこの slug だけ**を残す
/// （診断ログにペイン内容を出さない = AGENTS.md の絶対ルール）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnErrorKind {
    /// 名前解決できない（`ENOTFOUND` / `EAI_AGAIN` / `Can't reach the API server`）
    Dns,
    /// 接続を拒否された（`ECONNREFUSED` / `Connection refused`）
    Refused,
    /// 接続を切られた（`ECONNRESET` / `socket hang up`）
    Reset,
    /// 時間切れ（`ETIMEDOUT` / `Request timed out`）
    Timeout,
    /// 経路が無い（`ENETUNREACH` / `EHOSTUNREACH` / `ENETDOWN`）
    Unreachable,
    /// 応答の途中で接続が失われた（`Connection lost mid-response` / `Unable to connect to API` 等）
    Lost,
    /// PC のスリープで応答が途切れた（`Your computer went to sleep mid-response`）
    Sleep,
    /// 応答が届かなくなった（`The response stopped arriving`）
    Stalled,
}

impl ConnErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dns => "dns",
            Self::Refused => "refused",
            Self::Reset => "reset",
            Self::Timeout => "timeout",
            Self::Unreachable => "unreachable",
            Self::Lost => "lost",
            Self::Sleep => "sleep",
            Self::Stalled => "stalled",
        }
    }
}

/// 種別ごとの手がかり（小文字で照合する）。**上から順に見る**（`Unable to connect to API
/// (ENOTFOUND …)` は具体的なコードの側で取る）。
///
/// 文言は実物から決めた（2026-10-15 に手元の claude の会話記録 = `isApiErrorMessage` を
/// 集計した 300 件超と、claude 2.1.294 を到達できない API 先へ向けて採った画面）:
/// `Can't reach the API server — check your internet or DNS (ENOTFOUND)` /
/// `Connection refused — a firewall or proxy may be blocking it (ECONNREFUSED)` /
/// `Connection lost mid-response.` / `Your computer went to sleep mid-response.` /
/// `The response stopped arriving.`。旧い版の `Unable to connect to API (ConnectionRefused)` /
/// `Connection closed mid-response` も受ける
const KIND_NEEDLES: &[(ConnErrorKind, &[&str])] = &[
    (
        ConnErrorKind::Dns,
        &[
            "enotfound",
            "eai_again",
            "can't reach the api server",
            "getaddrinfo",
        ],
    ),
    (
        ConnErrorKind::Refused,
        &["econnrefused", "connectionrefused", "connection refused"],
    ),
    (
        ConnErrorKind::Reset,
        &["econnreset", "socket hang up", "connection reset"],
    ),
    (
        ConnErrorKind::Unreachable,
        &[
            "enetunreach",
            "ehostunreach",
            "enetdown",
            "network is unreachable",
        ],
    ),
    (
        ConnErrorKind::Timeout,
        &["etimedout", "request timed out", "timed out"],
    ),
    (ConnErrorKind::Sleep, &["went to sleep"]),
    (ConnErrorKind::Stalled, &["stopped arriving"]),
    (
        ConnErrorKind::Lost,
        &[
            "connection lost",
            "connection closed",
            "connection error",
            "unable to connect to api",
            "fetch failed",
        ],
    ),
];

/// 接続系**ではない**ことを示す手がかり（どれか 1 つでもあれば対象外）。
/// サーバー側の失敗（`500 Internal server error` / `529 Overloaded` /
/// `Server error mid-response`）は回線が戻っても直らない。認証・上限・課金は別の担当
const NOT_CONNECTION_NEEDLES: &[&str] = &[
    "server error",
    "overloaded",
    "authentication",
    "/login",
    "oauth",
    "api key",
    "rate limit",
    "rate_limit",
    "usage limit",
    "credit",
    "billing",
];

/// `API Error` の 1 論理行（折り返しを結合したもの）が接続系のエラーなら種別を返す。
///
/// `API Error:` の直後が 3 桁の数（HTTP の状態コード）なら、サーバーが応答している =
/// 回線は通っているので対象外
pub fn connection_error_kind(text: &str) -> Option<ConnErrorKind> {
    let lower = text.to_lowercase();
    let at = lower.find("api error")?;
    let rest = lower[at + "api error".len()..].trim_start_matches([':', ' ', '(']);
    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 3 {
        return None;
    }
    if NOT_CONNECTION_NEEDLES.iter().any(|n| lower.contains(n)) {
        return None;
    }
    KIND_NEEDLES
        .iter()
        .find(|(_, needles)| needles.iter().any(|n| lower.contains(n)))
        .map(|(kind, _)| *kind)
}

// --- 発動判断 ---

/// 再開を見送る理由（状態照会と persist.log にそのまま載せる）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// この停止へはもう送った（同じ停止に 2 回送らない）
    AlreadyResumed,
    /// 入力欄に人の未送信の文字がある
    UserDraft,
    /// 選択ダイアログ・メニューが出ている（文字を送ると選択に化ける）
    Dialog,
    /// ほかの送達（master の指示・キューの救出等）が走っている
    Delivering,
    /// 直近 [`RESUME_WINDOW_SECS`] 秒で [`MAX_RESUMES`] 回再開した（打ち切り）
    TooManyResumes,
    /// 止まってまだ [`STABLE_SECS`] 秒経っていない
    ScreenUnstable,
}

impl Hold {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AlreadyResumed => "already_resumed",
            Self::UserDraft => "user_draft",
            Self::Dialog => "dialog",
            Self::Delivering => "delivering",
            Self::TooManyResumes => "too_many_resumes",
            Self::ScreenUnstable => "screen_unstable",
        }
    }
}

/// 判断の材料（呼び出し側が観測して詰める。ここでは I/O をしない）
#[derive(Debug, Clone, Copy)]
pub struct ResumeInput {
    pub now: i64,
    /// この停止を最初に見た時刻（unix 秒）
    pub stable_since: i64,
    /// この停止へもう送ったか
    pub already_sent: bool,
    pub user_draft: bool,
    pub dialog: bool,
    pub delivering: bool,
    /// このペインを直近 [`RESUME_WINDOW_SECS`] 秒のうちに再開した回数
    pub recent_resumes: usize,
}

/// 回線が戻ったら送ってよいか。`Err` は見送る理由。
///
/// 理由の順は「人が見て意味の強いもの」から（送ったものは送ったと言う → 人の手 → 機の都合）
pub fn decide(input: &ResumeInput) -> Result<(), Hold> {
    if input.already_sent {
        return Err(Hold::AlreadyResumed);
    }
    if input.dialog {
        return Err(Hold::Dialog);
    }
    if input.user_draft {
        return Err(Hold::UserDraft);
    }
    if input.delivering {
        return Err(Hold::Delivering);
    }
    if input.recent_resumes >= MAX_RESUMES {
        return Err(Hold::TooManyResumes);
    }
    if input.now - input.stable_since < STABLE_SECS {
        return Err(Hold::ScreenUnstable);
    }
    Ok(())
}

/// 直近の窓に入る再開の回数（`times` は unix 秒）
pub fn recent_resumes(times: &[i64], now: i64) -> usize {
    times
        .iter()
        .filter(|t| now - **t < RESUME_WINDOW_SECS)
        .count()
}

// --- 到達確認 ---

/// 次の到達確認までの間隔（秒）。`outage_secs` は最初に届かなかった時刻からの経過
/// （まだ一度も失敗していなければ `None` = すぐ確かめる）。
///
/// 回線断が長引くほど間を空ける（数時間の断でも数秒おきに起こし続けない）。
/// 長くても 30 秒なので、復帰から再開までの遅れは最大でもその程度
pub fn probe_interval_secs(outage_secs: Option<i64>) -> i64 {
    match outage_secs {
        None => 0,
        Some(s) if s < 600 => 3,
        Some(s) if s < 3600 => 10,
        Some(_) => 30,
    }
}

/// 到達確認の先をどこから決めたか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeSource {
    /// [`PROBE_ENV`] で明示された
    Override,
    /// `HTTPS_PROXY` のプロキシ（直接出られない環境では API ではなくプロキシへの到達を見る）
    Proxy,
    /// `ANTHROPIC_BASE_URL` の先
    BaseUrl,
    /// 既定（claude の API）
    Default,
}

impl ProbeSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Override => "override",
            Self::Proxy => "proxy",
            Self::BaseUrl => "base_url",
            Self::Default => "default",
        }
    }
}

/// 到達確認の先（名前解決 + TCP 接続だけを見る。データは 1 バイトも送らない）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeTarget {
    pub host: String,
    pub port: u16,
    pub source: ProbeSource,
}

impl ProbeTarget {
    /// 表示・記録用の `host:port`（URL の利用者名・パスワードは持たない）
    pub fn display(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// env から到達確認の先を決める（`get` は env の読み口。テストで差し替える）
pub fn probe_target_from(get: impl Fn(&str) -> Option<String>) -> ProbeTarget {
    let nonempty = |k: &str| get(k).filter(|v| !v.trim().is_empty());
    if let Some((host, port)) = nonempty(PROBE_ENV).and_then(|v| parse_endpoint(&v, 443)) {
        return ProbeTarget {
            host,
            port,
            source: ProbeSource::Override,
        };
    }
    for key in ["HTTPS_PROXY", "https_proxy"] {
        if let Some((host, port)) = nonempty(key).and_then(|v| parse_endpoint(&v, 8080)) {
            return ProbeTarget {
                host,
                port,
                source: ProbeSource::Proxy,
            };
        }
    }
    if let Some((host, port)) = nonempty("ANTHROPIC_BASE_URL").and_then(|v| parse_endpoint(&v, 443))
    {
        return ProbeTarget {
            host,
            port,
            source: ProbeSource::BaseUrl,
        };
    }
    ProbeTarget {
        host: DEFAULT_PROBE_HOST.to_string(),
        port: 443,
        source: ProbeSource::Default,
    }
}

/// 実行中の env から決める
pub fn probe_target() -> ProbeTarget {
    probe_target_from(|k| std::env::var(k).ok())
}

/// `host:port` / URL（`http://user:pass@proxy:3128/`）から host と port を取り出す。
/// ポートが無ければ scheme（http = 80 / https = 443）、それも無ければ `default_port`
fn parse_endpoint(raw: &str, default_port: u16) -> Option<(String, u16)> {
    let raw = raw.trim();
    let (scheme, rest) = match raw.split_once("://") {
        Some((s, r)) => (Some(s.to_ascii_lowercase()), r),
        None => (None, raw),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // 利用者名・パスワードは捨てる（記録にも表示にも出さない）
    let hostport = authority.rsplit('@').next().unwrap_or("");
    let fallback = match scheme.as_deref() {
        Some("http") => 80,
        Some("https") => 443,
        _ => default_port,
    };
    let (host, port) = if let Some(v6) = hostport.strip_prefix('[') {
        let (host, after) = v6.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(p) => p.parse().ok()?,
            None => fallback,
        };
        (host.to_string(), port)
    } else {
        match hostport.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.parse().ok()?),
            None => (hostport.to_string(), fallback),
        }
    };
    (!host.is_empty()).then_some((host, port))
}

// --- 再開の文面 ---

/// 回線が戻ったときに送る「続き」。#813 の継続ナッジと同じく【tako 自動通知】で始め、
/// AI 側が「人の指示」と取り違えないようにする
pub fn nudge_prompt() -> String {
    nudge_prompt_in(lang())
}

/// 言語を明示しての文面（テストは言語グローバルに触らない）
pub fn nudge_prompt_in(lang: Lang) -> String {
    match lang {
        Lang::Ja => "【tako 自動通知】ネットワークの接続が戻りました。\
             接続エラーで中断していた作業をそのまま続けてください。\
             どこまで進んでいたか分からなければ、まず直近の状況を確認してから再開してください。"
            .to_string(),
        Lang::En => "[tako auto-notice] The network connection is back. \
             Continue the work that was interrupted by the connection error. \
             If you are unsure where you left off, check your recent state first, then resume."
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 実物（2026-10-15 に会話記録から集計した claude の `API Error` の全種類）
    #[test]
    fn issue2112_接続系の実文言を種別つきで受ける() {
        let cases = [
            (
                "API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)",
                ConnErrorKind::Dns,
            ),
            (
                "API Error: Connection refused — a firewall or proxy may be blocking it (ECONNREFUSED)",
                ConnErrorKind::Refused,
            ),
            (
                "API Error: Connection lost mid-response. The response above may be incomplete.",
                ConnErrorKind::Lost,
            ),
            (
                "API Error: Your computer went to sleep mid-response. The response above may be incomplete.",
                ConnErrorKind::Sleep,
            ),
            (
                "API Error: The response stopped arriving. The response above may be incomplete.",
                ConnErrorKind::Stalled,
            ),
            // 旧い版の形（pane-logs / #757 の fixture）
            (
                "API Error: Unable to connect to API (ConnectionRefused)",
                ConnErrorKind::Refused,
            ),
            (
                "API Error: Unable to connect to API (ENOTFOUND api.anthropic.com)",
                ConnErrorKind::Dns,
            ),
            (
                "API Error: Connection closed mid-response.",
                ConnErrorKind::Lost,
            ),
            ("API Error: Request timed out.", ConnErrorKind::Timeout),
            ("API Error: read ECONNRESET", ConnErrorKind::Reset),
            ("API Error: connect EHOSTUNREACH", ConnErrorKind::Unreachable),
        ];
        for (text, want) in cases {
            assert_eq!(connection_error_kind(text), Some(want), "{text}");
        }
    }

    /// 回線が戻っても直らないもの（サーバー側・認証・上限）は受けない
    #[test]
    fn issue2112_接続系でないエラーは受けない() {
        for text in [
            "API Error: 500 Internal server error. This is a server-side issue, usually temporary — try again in a moment.",
            "API Error: 529 Overloaded. This is a server-side issue, usually temporary — try again in a moment.",
            "API Error: Server error mid-response. The response above may be incomplete.",
            "API Error: 401 {\"type\":\"error\",\"error\":{\"type\":\"authentication_error\",\"message\":\"OAuth token has expired.\"}} · Please run /login",
            "API Error: 429 rate_limit_error",
            "API Error: Unable to connect to API (ENOTFOUND) · OAuth refresh token is no longer valid; run /login to re-authenticate",
            // API Error ではない（作業出力の引用・会話の文）
            "ENOTFOUND が出たら DNS を疑う",
            "Connection refused",
            "You've hit your session limit · resets 3am",
        ] {
            assert_eq!(connection_error_kind(text), None, "{text}");
        }
    }

    #[test]
    fn issue2112_見送りの順は送った_ダイアログ_下書き_送達中_打ち切り_静止() {
        let base = ResumeInput {
            now: 100,
            stable_since: 0,
            already_sent: false,
            user_draft: false,
            dialog: false,
            delivering: false,
            recent_resumes: 0,
        };
        assert_eq!(decide(&base), Ok(()));
        let all = ResumeInput {
            already_sent: true,
            user_draft: true,
            dialog: true,
            delivering: true,
            recent_resumes: MAX_RESUMES,
            stable_since: 99,
            ..base
        };
        assert_eq!(decide(&all), Err(Hold::AlreadyResumed));
        let all = ResumeInput {
            already_sent: false,
            ..all
        };
        assert_eq!(decide(&all), Err(Hold::Dialog));
        let all = ResumeInput {
            dialog: false,
            ..all
        };
        assert_eq!(decide(&all), Err(Hold::UserDraft));
        let all = ResumeInput {
            user_draft: false,
            ..all
        };
        assert_eq!(decide(&all), Err(Hold::Delivering));
        let all = ResumeInput {
            delivering: false,
            ..all
        };
        assert_eq!(decide(&all), Err(Hold::TooManyResumes));
        let all = ResumeInput {
            recent_resumes: MAX_RESUMES - 1,
            ..all
        };
        assert_eq!(decide(&all), Err(Hold::ScreenUnstable));
        // 境界: ちょうど STABLE_SECS 経てば送ってよい
        let edge = ResumeInput {
            stable_since: 100 - STABLE_SECS,
            ..all
        };
        assert_eq!(decide(&edge), Ok(()));
    }

    #[test]
    fn issue2112_再開の回数は窓の中だけ数える() {
        let now = 10_000;
        let times = [
            now - RESUME_WINDOW_SECS,
            now - RESUME_WINDOW_SECS + 1,
            now - 5,
        ];
        assert_eq!(recent_resumes(&times, now), 2);
        assert_eq!(recent_resumes(&[], now), 0);
    }

    #[test]
    fn issue2112_到達確認の間隔は断が長引くほど空く() {
        assert_eq!(probe_interval_secs(None), 0);
        assert_eq!(probe_interval_secs(Some(0)), 3);
        assert_eq!(probe_interval_secs(Some(599)), 3);
        assert_eq!(probe_interval_secs(Some(600)), 10);
        assert_eq!(probe_interval_secs(Some(3599)), 10);
        assert_eq!(probe_interval_secs(Some(6 * 3600)), 30);
    }

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn issue2112_到達確認の先は明示_プロキシ_base_url_既定の順() {
        let t = probe_target_from(env(&[]));
        assert_eq!(
            (t.host.as_str(), t.port, t.source),
            ("api.anthropic.com", 443, ProbeSource::Default)
        );
        let t = probe_target_from(env(&[("ANTHROPIC_BASE_URL", "http://gw.internal/v1")]));
        assert_eq!(
            (t.host.as_str(), t.port, t.source),
            ("gw.internal", 80, ProbeSource::BaseUrl)
        );
        // 利用者名とパスワードは捨てる（表示にも出さない）
        let t = probe_target_from(env(&[
            ("ANTHROPIC_BASE_URL", "https://gw.internal"),
            ("HTTPS_PROXY", "http://user:secret@proxy.internal:3128/"),
        ]));
        assert_eq!(
            (t.host.as_str(), t.port, t.source),
            ("proxy.internal", 3128, ProbeSource::Proxy)
        );
        assert!(!t.display().contains("secret"));
        let t = probe_target_from(env(&[
            ("HTTPS_PROXY", "http://proxy.internal:3128"),
            (PROBE_ENV, "127.0.0.1:47999"),
        ]));
        assert_eq!(
            (t.host.as_str(), t.port, t.source),
            ("127.0.0.1", 47999, ProbeSource::Override)
        );
        let t = probe_target_from(env(&[(PROBE_ENV, "[::1]:8443")]));
        assert_eq!((t.host.as_str(), t.port), ("::1", 8443));
        assert_eq!(t.display(), "[::1]:8443");
        // 読めない値・空は次の候補へ落ちる
        let t = probe_target_from(env(&[(PROBE_ENV, "host:notaport"), ("HTTPS_PROXY", " ")]));
        assert_eq!(t.source, ProbeSource::Default);
    }

    #[test]
    fn issue2112_文面は自動通知の印で始まる() {
        assert!(nudge_prompt_in(Lang::Ja).starts_with("【tako 自動通知】"));
        assert!(nudge_prompt_in(Lang::En).starts_with("[tako auto-notice]"));
    }
}
