//! Claude 公式 Remote Control の session URL 解決（Issue #1069 / エピック #1059）
//!
//! ## 何をする層か
//!
//! Remote Control に繋がった会話には **claude.ai/code のディープリンク**がある。
//! tako はそれをローカルの transcript から読んで、PWA / CLI / MCP へ同じ値を渡す。
//! スマホは「tako の一覧からタップ → Claude アプリでその会話が開く」だけで済む。
//!
//! ## 取得経路は 2 段（正 → 予備）
//!
//! | 段 | 行 | 中身 |
//! |---|---|---|
//! | 正 | `{"type":"system","subtype":"bridge_status", …, "url":"https://claude.ai/code/session_…"}` | **完成形の URL がそのまま入る** |
//! | 予備 | `{"type":"bridge-session","bridgeSessionId":"cse_…", …}` | id だけ。`cse_` → `session_` へ変換して組む |
//!
//! **`bridge_status` は常に出るわけではない**。バイナリ側の実装には条件が付いている:
//!
//! ```text
//! if(Qr) t((si)=>si.some((qs)=>qs.type==="system"&&qs.subtype==="bridge_status"&&qs.url===_o)
//!             ? si : [...si, oVf(_o)])
//! ```
//!
//! 実測（claude 2.1.232 / 2026-09-02）でこの条件の意味がはっきりした:
//!
//! | セッションの起こし方 | `bridge_status` | `bridge-session` |
//! |---|---|---|
//! | **`--remote-control` つきで起動**（#1068 の opt-in。tako の spawn 経路） | **1 行** | 2 行 |
//! | アカウント既定の自動接続（この機の既存 transcript 84 件） | **0 行** | 各 1 行以上 |
//!
//! つまり **正段は tako 自身が繋いだセッションでは効き、自動接続だと効かない**。
//! レポート（`research/2026-09-01-remote-renewal-claude-official.md` §4）の
//! 「① を推奨」は URL が完成形で入る点では正しいが、**常に在るわけではない**ので
//! 2 段構えが要る（自動接続のセッションを一覧に出すのは予備段の仕事）。
//!
//! ## URL の組み立て（バイナリから採取した実装）
//!
//! ```text
//! function RIr(e,t){ if(yCn(e,t)) return "http://localhost:4000";
//!                    if(Feb(e,t)) return "https://claude-ai.staging.ant.dev";
//!                    return "https://claude.ai" }
//! function WS(e,t,r){ let o=toCompatSessionId(e), s=`${RIr(o,t)}/code/${o}`; … }
//! function V3(e){ if(!e.startsWith("cse_")) return e; return "session_"+e.slice(4) }
//! ```
//!
//! 本番の base は常に `https://claude.ai`（他の 2 つは Anthropic 社内の dev / staging）。
//! だから id から組むときは `https://claude.ai/code/session_<body>` で正しい。
//!
//! ## 出さないもの
//!
//! `bridge-session` 行には `ownerAccountUuid` / `ownerOrganizationUuid` が入っている。
//! **どちらも保持しない・返さない・ログにも出さない**（AGENTS.md の絶対ルール）。
//! アカウントの区別は tako 側の accounts.yaml の**名前**で表す（§5 の
//! 「どのアカウントのセッションか」を出す要件は、UUID ではなく名前で満たす）。
//!
//! **URL 自体も診断ログへ出さない**。開くには claude.ai ログインが要る（実測 403）ので
//! 秘密ではないが、id は `claude -p --cloud <id>` の宛先になる = ペイン内容と同基準で扱う
//! （番犬 = `crates/tako-control/tests/remote_link_watchdog.rs`）。

use serde_json::{json, Value};

/// claude.ai の本番ホスト。dev / staging の base は Anthropic 社内向けなので採らない
const CLAUDE_AI_BASE: &str = "https://claude.ai";

/// 互換 id の接頭辞（`toCompatSessionId` の出力）
const COMPAT_PREFIX: &str = "session_";
/// インフラ id の接頭辞（`toInfraSessionId` の出力）
const INFRA_PREFIX: &str = "cse_";

/// Remote Control の接続状態。**URL を捏造しない**ことをこの型で表す
/// （`Connected` だけが URL を持つ）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkState {
    /// 繋がっている。URL が読めた
    Connected,
    /// 会話は在るが Remote Control に繋がっていない
    NotConnected,
    /// 繋げられない理由がローカルで確定している（#1068 の判定）
    Ineligible { reason: String },
    /// 判断材料が無い（transcript が見つからない・読めない・claude 以外）
    Unknown,
}

impl LinkState {
    /// 応答 JSON の `state`。`ineligible` は理由を付ける
    /// （レポート §9-B の `ineligible: <理由>`）
    pub fn as_wire(&self) -> String {
        match self {
            Self::Connected => "connected".into(),
            Self::NotConnected => "not_connected".into(),
            Self::Ineligible { reason } => format!("ineligible: {reason}"),
            Self::Unknown => "unknown".into(),
        }
    }
}

/// 1 セッションぶんの公式リンク
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteLink {
    /// `https://claude.ai/code/session_…`。繋がっていなければ `None`
    pub url: Option<String>,
    /// 互換形式（`session_…`）のセッション id。繋がっていなければ `None`
    pub session_id: Option<String>,
    /// どの tako アカウント配下のセッションか（accounts.yaml の**名前**）。
    /// **UUID ではない**。スマホが別アカウントでログインしていると
    /// 一覧に出ないので、この表示が無いと切り分け不能になる（レポート §5）
    pub account_label: Option<String>,
    pub state: LinkState,
}

impl RemoteLink {
    /// 判断材料が無いとき（**URL も id も持たない**）
    pub fn unknown() -> Self {
        Self {
            url: None,
            session_id: None,
            account_label: None,
            state: LinkState::Unknown,
        }
    }

    /// 会話は読めたが繋がっていないとき
    pub fn not_connected(account_label: Option<String>) -> Self {
        Self {
            url: None,
            session_id: None,
            account_label,
            state: LinkState::NotConnected,
        }
    }

    /// ローカルで不適格が確定しているとき（#1068 の判定を持ち込む）
    pub fn ineligible(reason: impl Into<String>, account_label: Option<String>) -> Self {
        Self {
            url: None,
            session_id: None,
            account_label,
            state: LinkState::Ineligible {
                reason: reason.into(),
            },
        }
    }

    pub fn is_connected(&self) -> bool {
        matches!(self.state, LinkState::Connected)
    }

    /// API / CLI / MCP が返す形。**3 経路が同じ 1 実装を通る**ので値が食い違わない
    pub fn to_json(&self) -> Value {
        json!({
            "url": self.url,
            "session_id": self.session_id,
            "account_label": self.account_label,
            "state": self.state.as_wire(),
        })
    }
}

/// `cse_…` / `session_…` を互換形式（`session_…`）へそろえる
/// （バイナリの `toCompatSessionId`）。**接頭辞が無い id はそのまま返す**
pub fn to_compat_session_id(id: &str) -> String {
    match id.strip_prefix(INFRA_PREFIX) {
        Some(body) => format!("{COMPAT_PREFIX}{body}"),
        None => id.to_string(),
    }
}

/// bridge session id の形として妥当か。
/// **パスを組むのではなく URL を組む**ので traversal の心配は無いが、
/// 上流の書式が変わったときに壊れた URL を出さないために形を検査する
/// （英数と `_` だけ・接頭辞つき・長さの上限）
pub fn is_valid_bridge_session_id(id: &str) -> bool {
    let Some(body) = id
        .strip_prefix(COMPAT_PREFIX)
        .or_else(|| id.strip_prefix(INFRA_PREFIX))
    else {
        return false;
    };
    !body.is_empty()
        && body.len() <= 64
        && body.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// bridge session id から公式リンクを組む（**純粋関数**）。
/// 形が妥当でなければ `None`（URL を捏造しない）
pub fn url_for_bridge_session_id(id: &str) -> Option<String> {
    if !is_valid_bridge_session_id(id) {
        return None;
    }
    Some(format!(
        "{CLAUDE_AI_BASE}/code/{}",
        to_compat_session_id(id)
    ))
}

/// `bridge_status` 行の `url` が使える形か（**純粋関数**）。
///
/// 上流が dev / staging の base を出した場合（Anthropic 社内）はそのまま通す:
/// URL は claude 自身が組んだ完成形なので、tako が書式を疑って捨てる方が害が大きい。
/// **ただし http/https 以外は通さない**（#680 と同じ基準。`javascript:` を
/// PWA のリンクや `open_url` に渡さない）
pub fn sanitize_status_url(url: &str) -> Option<String> {
    let trimmed = url.trim();
    if trimmed.len() > 2048 {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return None;
    }
    // `/code/<id>` を含まないものは Remote Control のセッション URL ではない
    if !trimmed.contains("/code/") {
        return None;
    }
    Some(trimmed.to_string())
}

/// URL から `session_…` / `cse_…` を取り出す（`/code/` の後・`?` の手前）。
/// docs: 「The ID is the part of the session's URL at claude.ai/code between
/// `/code/` and any `?`.」
pub fn session_id_from_url(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("/code/")?;
    let id = rest.split(['?', '#', '/']).next()?;
    if is_valid_bridge_session_id(id) {
        Some(to_compat_session_id(id))
    } else {
        None
    }
}

/// transcript の 1 行から採れる手がかり
#[derive(Debug, Clone, PartialEq, Eq)]
enum Clue {
    /// `bridge_status` の完成形 URL（正）
    StatusUrl(String),
    /// `bridge-session` の id（予備）
    SessionId(String),
}

/// transcript の 1 行を見る（**純粋関数**。ファイルに触らない）
fn clue_from_line(line: &str) -> Option<Clue> {
    // 安い前置フィルタ。transcript は 1 行が数十 KB になることもあるので、
    // 関係ない行で serde_json を回さない
    if !line.contains("bridge") {
        return None;
    }
    let value: Value = serde_json::from_str(line).ok()?;
    let ty = value.get("type").and_then(Value::as_str)?;
    match ty {
        "system" if value.get("subtype").and_then(Value::as_str) == Some("bridge_status") => {
            let url = value.get("url").and_then(Value::as_str)?;
            sanitize_status_url(url).map(Clue::StatusUrl)
        }
        "bridge-session" => {
            // **`ownerAccountUuid` / `ownerOrganizationUuid` は読まない**（保持しない）
            let id = value.get("bridgeSessionId").and_then(Value::as_str)?;
            is_valid_bridge_session_id(id).then(|| Clue::SessionId(id.to_string()))
        }
        _ => None,
    }
}

/// transcript の行から公式リンクを抽出する（**純粋関数**）。
///
/// **末尾に近いものが正**（同じ会話が繋ぎ直されると id が変わる）ので、
/// 最後に見つかった手がかりを採る。`bridge_status` の URL は
/// 同じ位置の `bridge-session` の id よりも強い（完成形なので base も正しい）
pub fn extract_link(lines: impl Iterator<Item = String>) -> Option<RemoteLink> {
    let mut last_url: Option<String> = None;
    let mut last_id: Option<String> = None;
    for line in lines {
        match clue_from_line(&line) {
            Some(Clue::StatusUrl(url)) => last_url = Some(url),
            Some(Clue::SessionId(id)) => last_id = Some(id),
            None => {}
        }
    }
    // 正: bridge_status の URL
    if let Some(url) = last_url {
        let session_id = session_id_from_url(&url);
        return Some(RemoteLink {
            url: Some(url),
            session_id,
            account_label: None,
            state: LinkState::Connected,
        });
    }
    // 予備: bridge-session の id から組む
    let id = last_id?;
    let url = url_for_bridge_session_id(&id)?;
    Some(RemoteLink {
        url: Some(url),
        session_id: Some(to_compat_session_id(&id)),
        account_label: None,
        state: LinkState::Connected,
    })
}

// --- I/O 側（transcript を探して読む） ---------------------------------------

/// transcript を読む上限行数。`bridge-session` / `bridge_status` は接続時に
/// 1 行だけ足されるので全部読む必要があるが、巨大な会話で無制限に読まないよう
/// 上限を置く（実測: 1 会話 = 数千行）
const MAX_SCAN_LINES: usize = 200_000;

/// claude の config dir から tako のアカウント名を引く（**UUID は使わない**）。
///
/// 既定の config dir（`~/.claude`）は accounts.yaml に載っていないことがあるので、
/// その場合は `default` を返す
pub fn account_label_for_config_dir(config_dir: &std::path::Path) -> Option<String> {
    if let Ok(accounts) = crate::orchestrator::AccountsConfig::load() {
        for (name, resolved) in accounts.list_resolved() {
            let Ok(account) = resolved else { continue };
            match account.config_dir.path() {
                Some(path) => {
                    if std::path::Path::new(path) == config_dir {
                        return Some(name.to_string());
                    }
                }
                None => {
                    // inherit = 既定の config dir を使うアカウント
                    if crate::orchestrator::claude_default_config_dir().as_deref()
                        == Some(config_dir)
                    {
                        return Some(name.to_string());
                    }
                }
            }
        }
    }
    if crate::orchestrator::claude_default_config_dir().as_deref() == Some(config_dir) {
        return Some("default".to_string());
    }
    None
}

/// 解決結果の memo（**ペイン一覧は数秒ごとにポーリングされる**）。
///
/// `/api/v2/panes` と `/api/agents` はペインの数ぶん transcript を探して読む。
/// 素直に毎回全部読むと実測 **1 件 4.5ms / 20 件 91ms**（8MB 級の会話なら 1 件 20ms）
/// かかったので、2 段で削る:
///
/// 1. **mtime が動いていなければ読まない**（アイドルなペイン）
/// 2. 動いていたら **`scanned_len` から先だけ読む**（生きている会話。mtime は
///    動き続けるので 1 だけでは効かない）。4MB の会話に 1 行追記された状況で
///    全走査 17.3ms → 追記ぶんだけ 203µs
///
/// 追記ぶんに手がかりが無ければ前回の答えを引き継ぐ。あれば差し替える
/// （**繋ぎ直しで id が変わる**ので、後ろにあるものが正）。
/// **daemon / GUI どちらの経路でも同じ物を通る**ので値が食い違わない
type MemoKey = String;
struct Memo {
    path: std::path::PathBuf,
    mtime: std::time::SystemTime,
    /// この時点までに走査したバイト数（**transcript は追記のみ**なので、
    /// 次回はここから先だけ読めば足りる）
    scanned_len: u64,
    /// 走査した範囲で見つかった最後の手がかり（無ければ `None` = 未接続）
    link: Option<RemoteLink>,
}

fn memo() -> &'static std::sync::Mutex<std::collections::HashMap<MemoKey, Memo>> {
    static MEMO: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<MemoKey, Memo>>> =
        std::sync::OnceLock::new();
    MEMO.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// memo の状態（`(path, mtime, scanned_len, link)`）を取り出す
fn memo_state(
    session_id: &str,
) -> Option<(
    std::path::PathBuf,
    std::time::SystemTime,
    u64,
    Option<RemoteLink>,
)> {
    let guard = memo().lock().ok()?;
    let e = guard.get(session_id)?;
    Some((e.path.clone(), e.mtime, e.scanned_len, e.link.clone()))
}

fn remember(
    session_id: &str,
    path: &std::path::Path,
    mtime: std::time::SystemTime,
    scanned_len: u64,
    link: &Option<RemoteLink>,
) {
    if let Ok(mut guard) = memo().lock() {
        // 上限を置く（長寿命の daemon で無限に増えないように）。
        // 超えたら丸ごと捨てる（LRU を持つほどの規模ではない）
        if guard.len() >= 512 {
            guard.clear();
        }
        guard.insert(
            session_id.to_string(),
            Memo {
                path: path.to_path_buf(),
                mtime,
                scanned_len,
                link: link.clone(),
            },
        );
    }
}

/// `from` バイト目から末尾までを行として走査する（**追記ぶんだけ読む**）。
///
/// `extract_link` は「最後の手がかり」を採るので、前回までに手がかりが無かった
/// 範囲を読み直す必要はない。前回の手がかりより後ろに新しいものがあれば
/// そちらが正（繋ぎ直し）なので、追記ぶんで見つかったらそれを採る。
///
/// 返す `consumed` は **`\n` で終わった行までのバイト位置**。
/// claude が書いている途中の行（改行がまだ来ていない）は数えないので、
/// 次回そこから読み直される = **境界で手がかりを取りこぼさない**
fn scan_from(path: &std::path::Path, from: u64) -> Result<(Option<RemoteLink>, u64), String> {
    use std::io::{BufRead, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).map_err(|e| format!("transcript を開けない: {e}"))?;
    if from > 0 {
        file.seek(SeekFrom::Start(from))
            .map_err(|e| format!("transcript を seek できない: {e}"))?;
    }
    let mut reader = std::io::BufReader::new(file);
    let mut consumed = from;
    let mut last: Option<RemoteLink> = None;
    let mut buf = String::new();
    for _ in 0..MAX_SCAN_LINES {
        buf.clear();
        let read = match reader.read_line(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            // 不正な UTF-8 等はそこで打ち切る（読めた範囲までを確定させる）
            Err(_) => break,
        };
        if !buf.ends_with('\n') {
            // 書き込み途中の行。**consumed へ含めない**（次回読み直す）
            break;
        }
        consumed += read as u64;
        if let Some(found) = extract_link(std::iter::once(buf.trim_end().to_string())) {
            last = Some(found);
        }
    }
    Ok((last, consumed))
}

/// 所在（`locate_transcript`）の memo。**生きている会話は mtime が動き続けるので
/// リンクの memo が効かない**が、**所在は変わらない**ので分けて持つ。
///
/// `locate_transcript` は config dir ごとに `projects/` を `read_dir` し、
/// その中の全プロジェクトへ `is_file()` を打つ（この機は 100 プロジェクト）。
/// ポーリング経路ではこれが cold の支配項になる
fn located(session_id: &str) -> Option<crate::transcript::TranscriptLocation> {
    static PATHS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<MemoKey, crate::transcript::TranscriptLocation>>,
    > = std::sync::OnceLock::new();
    let paths = PATHS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    // memo が指すファイルが今も在るなら使う（消えていれば探し直す）
    if let Ok(guard) = paths.lock() {
        if let Some(hit) = guard.get(session_id) {
            if hit.path.is_file() {
                return Some(hit.clone());
            }
        }
    }
    let found = crate::transcript::locate_transcript(session_id)?;
    if let Ok(mut guard) = paths.lock() {
        if guard.len() >= 512 {
            guard.clear();
        }
        guard.insert(session_id.to_string(), found.clone());
    }
    Some(found)
}

/// セッション id（claude の会話 UUID）から公式リンクを解決する。
///
/// 見つからない / 読めないときは `Unknown`（**URL を捏造しない**）。
/// 会話は読めたが繋がっていないときは `NotConnected`
pub fn link_for_session(session_id: &str) -> RemoteLink {
    let Some(location) = located(session_id) else {
        // 所在が分からないものは memo しない（あとで現れうる）
        return RemoteLink::unknown();
    };
    let Ok(meta) = std::fs::metadata(&location.path) else {
        return RemoteLink::unknown();
    };
    let (len, mtime) = (meta.len(), meta.modified().ok());

    let account_label = account_label_for_config_dir(&location.config_dir);

    // memo の状態から「どこから読むか」と「前回の答え」を決める
    let prior = memo_state(session_id).filter(|(p, ..)| p == &location.path);
    let (from, prev_link) = match &prior {
        // mtime が変わっていない = 何も起きていない。そのまま返す
        Some((_, m, _, link)) if Some(*m) == mtime => {
            return finish(link.clone(), account_label);
        }
        // 追記されている（**transcript は追記のみ**）= 増えたぶんだけ読む
        Some((_, _, scanned, link)) if len >= *scanned => (*scanned, link.clone()),
        // 縮んだ・別物になった = 全部読み直す
        _ => (0, None),
    };

    let (found, consumed) = match scan_from(&location.path, from) {
        Ok(v) => v,
        // 読めなかった = 判断材料が無い（繋がっていないとは言えない）。
        // 一時的な失敗を固定したくないので memo しない
        Err(_) => return RemoteLink::unknown(),
    };
    // 追記ぶんに手がかりが無ければ前回の答えを引き継ぐ
    let link = found.or(prev_link);
    if let Some(mtime) = mtime {
        // **走査し終えたバイト位置**を覚える（ファイル長ではない。
        // 書き込み途中の行を「読んだ」ことにすると手がかりを飛ばす）。
        // mtime も一緒に持つので、追記が無い周期は読みに行かない
        remember(session_id, &location.path, mtime, consumed, &link);
    }
    finish(link, account_label)
}

/// 抽出結果へアカウント名を載せて公開形へ落とす
fn finish(link: Option<RemoteLink>, account_label: Option<String>) -> RemoteLink {
    match link {
        Some(mut link) => {
            link.account_label = account_label;
            link
        }
        None => RemoteLink::not_connected(account_label),
    }
}

/// **この機で Remote Control がそもそも成立しないか**を #1068 の判定で見る。
///
/// `link_for_session` が「繋がっていない」を返すとき、それが
/// 「まだ繋いでいない」のか「この環境では繋げない」のかを言い分けるために使う
/// （レポート §6 の設計上の帰結 3 = PWA が理由を出せる必要がある）。
///
/// **限界**: プロファイルごとの env（`profile.env`）はここでは分からないので、
/// 見えるのは**マシン全体に効く阻害要因**だけ（tako 自身のプロセス env と組織ポリシー）。
/// プロファイル env による不適格は spawn 時の応答（#1068 の warnings）が正
pub fn machine_ineligible_reason() -> Option<String> {
    let plan = crate::orchestrator::EnvPlan::default();
    let facts = crate::claude_remote::collect_facts("claude", &plan);
    crate::claude_remote::eligibility(&facts)
        .blocker()
        .map(|b| b.kind().to_string())
}

/// agent 系統とセッション id から公式リンクを解決する（**3 経路が通る入口**）。
///
/// - claude 以外 → `ineligible: agent_unsupported`（マトリクスの宣言と同じ判断）
/// - セッション id が無い → `unknown`（会話が特定できていない = 判断材料が無い）
/// - 繋がっていない → マシン全体の阻害要因があれば `ineligible: <種別>`、無ければ `not_connected`
pub fn link_for_agent_session(agent: &str, session_id: Option<&str>) -> RemoteLink {
    if agent != "claude" {
        return RemoteLink::ineligible("agent_unsupported", None);
    }
    let Some(session_id) = session_id.filter(|s| !s.is_empty()) else {
        return RemoteLink::unknown();
    };
    let link = link_for_session(session_id);
    if matches!(link.state, LinkState::NotConnected) {
        if let Some(reason) = machine_ineligible_reason() {
            return RemoteLink::ineligible(reason, link.account_label);
        }
    }
    link
}

/// `claude agents --json` 由来の一覧（`agents::list_agents_with_panes`）の各行へ
/// 公式リンクを付ける（#1069）。
///
/// **HTTP の `/api/agents` と dispatch の `RemoteAgents`（CLI / MCP）が同じ 1 実装を通る**
/// ので、スマホの一覧と AI が見る値が食い違わない（開発不変条件）。
/// この一覧は claude のセッション列挙なので系統は常に claude
pub fn attach_to_agents(result: &mut Value) {
    let Some(agents) = result["agents"].as_array_mut() else {
        return;
    };
    for agent in agents {
        // **キーは `session_id`**（`agents::list_agents_*` が claude の
        // camelCase `sessionId` を正規化した後の名前）。ここを `sessionId` と
        // 書くと全件 `unknown` になる = 実測で踏んだ回帰なのでテストで固定する
        let sid = agent["session_id"].as_str().map(str::to_string);
        agent["remote_link"] = link_for_agent_session("claude", sid.as_deref()).to_json();
    }
}

/// 所在が分かっている transcript を**全部**走査する（`transcript::read_messages_at` と同じ形）。
///
/// 通常の解決経路（[`link_for_session`]）は **追記ぶんだけ**読む
/// （[`scan_from`]）ので、ここを通るのはセッションごとの初回と、
/// テストから直接呼ぶときだけ。
///
/// **末尾だけ読む近道は採らない**。実測（この機の実 transcript 25 本）で
/// `bridge-session` は「先頭 2〜11 行目」と「末尾の 99% 地点」の両方に出るので、
/// 窓を切ると「先頭にしか無い会話」を取りこぼす。追記ぶんだけ読む形なら
/// 定常状態はどちらにしても最小コストになる
pub fn read_link_at(path: &std::path::Path) -> Result<Option<RemoteLink>, String> {
    scan_from(path, 0).map(|(link, _)| link)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 実物の形（claude 2.1.232 のバイナリから採取した組み立て）。
    /// **id は架空**（実採取の値はリポに置かない。#927）
    const FAKE_COMPAT_ID: &str = "session_01AAAAAAAAAAAAAAAAAAAAAA";
    const FAKE_INFRA_ID: &str = "cse_01AAAAAAAAAAAAAAAAAAAAAA";

    fn bridge_session_line(id: &str) -> String {
        // 実物と同じキー構成（アカウント UUID も入っている = 読まないことの検証用）
        json!({
            "type": "bridge-session",
            "sessionId": "11111111-2222-3333-4444-555555555555",
            "bridgeSessionId": id,
            "lastSequenceNum": 0,
            "ownerAccountUuid": "66666666-7777-8888-9999-000000000000",
            "ownerOrganizationUuid": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
        })
        .to_string()
    }

    fn bridge_status_line(url: &str) -> String {
        json!({
            "type": "system",
            "subtype": "bridge_status",
            "content": format!("/remote-control is active · Continue here, on your phone, or at {url}"),
            "url": url,
            "isMeta": false,
            "timestamp": "2026-09-02T00:00:00.000Z",
            "uuid": "cccccccc-dddd-eeee-ffff-000000000000",
        })
        .to_string()
    }

    #[test]
    fn 互換_id_への変換は接頭辞だけを差し替える() {
        assert_eq!(to_compat_session_id(FAKE_INFRA_ID), FAKE_COMPAT_ID);
        // 既に互換形式ならそのまま
        assert_eq!(to_compat_session_id(FAKE_COMPAT_ID), FAKE_COMPAT_ID);
        // 接頭辞が無いものは触らない
        assert_eq!(to_compat_session_id("bare"), "bare");
    }

    #[test]
    fn id_の形が妥当なものだけ_url_を組む() {
        assert_eq!(
            url_for_bridge_session_id(FAKE_INFRA_ID).as_deref(),
            Some("https://claude.ai/code/session_01AAAAAAAAAAAAAAAAAAAAAA")
        );
        assert_eq!(
            url_for_bridge_session_id(FAKE_COMPAT_ID).as_deref(),
            Some("https://claude.ai/code/session_01AAAAAAAAAAAAAAAAAAAAAA")
        );
        // 接頭辞なし・空・記号入り・長すぎるものは組まない
        for bad in [
            "",
            "bare",
            "session_",
            "cse_",
            "session_../../etc/passwd",
            "session_a/b",
            "session_a?b",
        ] {
            assert_eq!(url_for_bridge_session_id(bad), None, "{bad:?} を通した");
        }
        let long = format!("session_{}", "a".repeat(65));
        assert_eq!(url_for_bridge_session_id(&long), None);
    }

    #[test]
    fn bridge_session_行から_url_を組める() {
        let link = extract_link(std::iter::once(bridge_session_line(FAKE_INFRA_ID)))
            .expect("予備段で解決できなければならない");
        assert!(link.is_connected());
        assert_eq!(
            link.url.as_deref(),
            Some("https://claude.ai/code/session_01AAAAAAAAAAAAAAAAAAAAAA")
        );
        assert_eq!(link.session_id.as_deref(), Some(FAKE_COMPAT_ID));
    }

    #[test]
    fn bridge_status_の_url_が予備段より優先される() {
        // 実運用では両方在りうる。完成形の URL を持つ正段が勝つ
        let other = "https://claude.ai/code/session_01BBBBBBBBBBBBBBBBBBBBBB?from=cli&m=0";
        let lines = vec![
            bridge_session_line(FAKE_INFRA_ID),
            bridge_status_line(other),
        ];
        let link = extract_link(lines.into_iter()).expect("解決できなければならない");
        assert_eq!(link.url.as_deref(), Some(other));
        // id は URL から取る（`?` の手前まで）
        assert_eq!(
            link.session_id.as_deref(),
            Some("session_01BBBBBBBBBBBBBBBBBBBBBB")
        );
    }

    #[test]
    fn 繋ぎ直しでは末尾の手がかりが勝つ() {
        let lines = vec![
            bridge_session_line("cse_01AAAAAAAAAAAAAAAAAAAAAA"),
            bridge_session_line("cse_01CCCCCCCCCCCCCCCCCCCCCC"),
        ];
        let link = extract_link(lines.into_iter()).unwrap();
        assert_eq!(
            link.session_id.as_deref(),
            Some("session_01CCCCCCCCCCCCCCCCCCCCCC")
        );
    }

    #[test]
    fn bridge_の行が無ければ何も返さない() {
        let lines = vec![
            json!({"type": "user", "message": {"role": "user", "content": "hi"}}).to_string(),
            json!({"type": "assistant", "message": {"role": "assistant", "content": []}})
                .to_string(),
            // 紛らわしいが別物（`bridge` を含むだけの行）
            json!({"type": "system", "subtype": "informational", "content": "bridge"}).to_string(),
            String::new(),
            "{ broken json".to_string(),
        ];
        assert_eq!(extract_link(lines.into_iter()), None);
    }

    #[test]
    fn アカウント_uuid_を保持しない() {
        // 実物の `bridge-session` 行にはアカウント UUID が入っている。
        // 抽出結果のどのフィールドにも出てはいけない（AGENTS.md の絶対ルール）
        let line = bridge_session_line(FAKE_INFRA_ID);
        assert!(
            line.contains("ownerAccountUuid"),
            "前提: 材料に UUID が在る"
        );
        let link = extract_link(std::iter::once(line)).unwrap();
        let rendered = link.to_json().to_string();
        for uuid in [
            "66666666-7777-8888-9999-000000000000",
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            "11111111-2222-3333-4444-555555555555",
        ] {
            assert!(
                !rendered.contains(uuid),
                "UUID が応答に混ざっている: {rendered}"
            );
        }
        assert!(link.account_label.is_none(), "抽出段では名前を付けない");
    }

    #[test]
    fn 危険なスキームの_url_は捨てる() {
        for bad in [
            "javascript:alert(1)//code/x",
            "file:///etc/passwd/code/x",
            "data:text/html,/code/x",
            "  ",
            // `/code/` が無い = セッション URL ではない
            "https://claude.ai/chat/abc",
        ] {
            assert_eq!(sanitize_status_url(bad), None, "{bad:?} を通した");
        }
        assert_eq!(
            sanitize_status_url("  https://claude.ai/code/session_01A  ").as_deref(),
            Some("https://claude.ai/code/session_01A")
        );
        // 社内 base（dev / staging）は claude 自身が組んだ完成形なので通す
        assert!(sanitize_status_url("http://localhost:4000/code/session_01A").is_some());
    }

    #[test]
    fn 危険なスキームの行は抽出でも捨てられる() {
        let link = extract_link(std::iter::once(bridge_status_line(
            "javascript:alert(1)//code/x",
        )));
        assert_eq!(link, None, "javascript: を URL として採ってはいけない");
    }

    #[test]
    fn url_からの_id_抽出はクエリを落とす() {
        assert_eq!(
            session_id_from_url("https://claude.ai/code/session_01A?from=cli&m=0").as_deref(),
            Some("session_01A")
        );
        assert_eq!(
            session_id_from_url("https://claude.ai/code/cse_01A").as_deref(),
            Some("session_01A")
        );
        assert_eq!(
            session_id_from_url("https://claude.ai/code/").as_deref(),
            None
        );
        assert_eq!(
            session_id_from_url("https://claude.ai/other").as_deref(),
            None
        );
    }

    #[test]
    fn 状態は_4_通りとも_wire_表現を持つ() {
        assert_eq!(LinkState::Connected.as_wire(), "connected");
        assert_eq!(LinkState::NotConnected.as_wire(), "not_connected");
        assert_eq!(LinkState::Unknown.as_wire(), "unknown");
        assert_eq!(
            LinkState::Ineligible {
                reason: "DISABLE_TELEMETRY".into()
            }
            .as_wire(),
            "ineligible: DISABLE_TELEMETRY"
        );
    }

    #[test]
    fn 繋がっていない状態は_url_を持たない() {
        for link in [
            RemoteLink::unknown(),
            RemoteLink::not_connected(Some("univ".into())),
            RemoteLink::ineligible("DISABLE_TELEMETRY", None),
        ] {
            assert!(link.url.is_none(), "URL を捏造している: {link:?}");
            assert!(link.session_id.is_none(), "id を捏造している: {link:?}");
            assert!(!link.is_connected());
            // 応答の形は 4 キー固定（PWA / CLI / MCP が同じ形を読む）
            let v = link.to_json();
            for key in ["url", "session_id", "account_label", "state"] {
                assert!(v.get(key).is_some(), "{key} が無い");
            }
        }
    }

    /// `attach_to_agents` が読むキーは **`session_id`**（正規化後の名前）。
    /// `sessionId`（claude の生の名前）と書くと全件 `unknown` になる
    /// （実測で踏んだ回帰。`agents::list_agents_*` の正規化を通った形で来る）
    #[test]
    fn agents_一覧の付与は正規化後のキーを読む() {
        // `agents::normalize` が出す形（`session_id` / `started_at` / `ctx_percent`）
        let mut result = json!({
            "agents": [
                { "session_id": "11111111-2222-3333-4444-555555555555", "status": "idle" },
                { "session_id": "", "status": "idle" },
                { "status": "idle" },
            ]
        });
        attach_to_agents(&mut result);
        let rows = result["agents"].as_array().unwrap();
        // どの行にも remote_link が付く（値は状態で変わる）
        for row in rows {
            assert!(row["remote_link"].is_object(), "付いていない: {row}");
            for key in ["url", "session_id", "account_label", "state"] {
                assert!(row["remote_link"].get(key).is_some(), "{key} が無い");
            }
        }
        // session_id が無い / 空の行は unknown（transcript を探しに行かない）
        assert_eq!(rows[1]["remote_link"]["state"], "unknown");
        assert_eq!(rows[2]["remote_link"]["state"], "unknown");
        // **camelCase では読まない**ことの対照: `sessionId` だけの行は unknown のまま
        let mut camel =
            json!({ "agents": [{ "sessionId": "11111111-2222-3333-4444-555555555555" }] });
        attach_to_agents(&mut camel);
        assert_eq!(
            camel["agents"][0]["remote_link"]["state"], "unknown",
            "正規化前のキーを読んでいる（実装が `sessionId` を見ている）"
        );
    }

    /// 追記ぶんだけ読む走査（`scan_from`）の不変条件。
    ///
    /// ① `consumed` は **`\n` で終わった行までしか進まない**（書き込み途中の行を
    /// 「読んだ」ことにすると、その行の手がかりを永久に飛ばす）
    /// ② 追記ぶんに手がかりがあれば拾う ③ 追記ぶんに無ければ `None`（呼び出し側が
    /// 前回の答えを引き継ぐ）
    #[test]
    fn 追記ぶんの走査は完結した行までしか進まない() {
        let dir = std::env::temp_dir().join("tako-1069-scan-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("t.jsonl");

        // 完結した 1 行 + **改行の無い書き込み途中の行**
        let complete = format!("{}\n", bridge_session_line(FAKE_INFRA_ID));
        let partial = "{\"type\":\"bridge-session\",\"bridgeSessionId\":\"cse_01ZZ";
        std::fs::write(&path, format!("{complete}{partial}")).unwrap();

        let (link, consumed) = scan_from(&path, 0).unwrap();
        assert_eq!(
            link.as_ref().and_then(|l| l.session_id.as_deref()),
            Some(FAKE_COMPAT_ID)
        );
        assert_eq!(
            consumed,
            complete.len() as u64,
            "書き込み途中の行を consumed に含めている（次回その行を読み飛ばす）"
        );

        // 途中の行が完成した = 次回そこから読み直して拾える
        let finished = format!(
            "{}{}\n",
            complete,
            bridge_session_line("cse_01ZZZZZZZZZZZZZZZZZZZZZZ")
        );
        std::fs::write(&path, &finished).unwrap();
        let (link2, consumed2) = scan_from(&path, consumed).unwrap();
        assert_eq!(
            link2.as_ref().and_then(|l| l.session_id.as_deref()),
            Some("session_01ZZZZZZZZZZZZZZZZZZZZZZ"),
            "追記ぶんの手がかりを拾えていない"
        );
        assert_eq!(consumed2, finished.len() as u64);

        // 手がかりの無い追記は None（呼び出し側が前回の答えを引き継ぐ）
        let grown = format!(
            "{finished}{}\n",
            json!({"type": "user", "message": {"role": "user", "content": "hi"}})
        );
        std::fs::write(&path, &grown).unwrap();
        let (link3, consumed3) = scan_from(&path, consumed2).unwrap();
        assert_eq!(link3, None, "手がかりが無いのに何か返している");
        assert_eq!(consumed3, grown.len() as u64);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **追記ぶんだけ読む形の効果**（`/api/v2/panes` は PWA がポーリングし、
    /// 生きている会話は mtime が動き続けるので「毎回読む」経路になる）。
    ///
    /// 4 MB の会話へ 1 行追記された状況を作り、全走査と追記ぶんだけの走査を比べる
    #[test]
    fn 追記ぶんだけ読むと定常コストが増えない() {
        let dir = std::env::temp_dir().join("tako-1069-incremental-bench");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("t.jsonl");
        let one = json!({"type": "user", "message": {"role": "user", "content": "x".repeat(300)}})
            .to_string();
        let mut body = String::new();
        while body.len() < 4 * 1024 * 1024 {
            body.push_str(&one);
            body.push('\n');
        }
        body.push_str(&bridge_session_line(FAKE_INFRA_ID));
        body.push('\n');
        std::fs::write(&path, &body).unwrap();

        // 全走査（初回に 1 回だけ通る）
        let t0 = std::time::Instant::now();
        let (link, consumed) = scan_from(&path, 0).unwrap();
        let full = t0.elapsed();
        assert_eq!(
            link.as_ref().and_then(|l| l.session_id.as_deref()),
            Some(FAKE_COMPAT_ID)
        );
        assert_eq!(consumed, body.len() as u64);

        // 1 行追記（= 会話が 1 ターン進んだ状況）→ 追記ぶんだけ読む
        let appended = format!("{body}{one}\n");
        std::fs::write(&path, &appended).unwrap();
        let t1 = std::time::Instant::now();
        let (found, consumed2) = scan_from(&path, consumed).unwrap();
        let incremental = t1.elapsed();
        assert_eq!(found, None, "追記ぶんに手がかりは無い");
        assert_eq!(consumed2, appended.len() as u64);

        eprintln!("4MB の会話: 全走査 {full:?} / 追記ぶんだけ {incremental:?}");
        assert!(
            incremental * 5 < full,
            "追記ぶんだけ読んでいるはずが速くなっていない（全走査 {full:?} / 追記 {incremental:?}）"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 読めないファイルは_unknown_になる() {
        let missing = std::env::temp_dir().join("tako-1069-missing.jsonl");
        let _ = std::fs::remove_file(&missing);
        assert!(read_link_at(&missing).is_err());
    }

    #[test]
    fn 実ファイルからも同じ結果になる() {
        let dir = std::env::temp_dir().join("tako-1069-read-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("t.jsonl");
        let body = format!(
            "{}\n{}\n",
            json!({"type": "user", "message": {"role": "user", "content": "hi"}}),
            bridge_session_line(FAKE_INFRA_ID)
        );
        std::fs::write(&path, body).unwrap();
        let link = read_link_at(&path).unwrap().expect("解決できる");
        assert_eq!(link.session_id.as_deref(), Some(FAKE_COMPAT_ID));

        // 繋がっていない会話は None（= 呼び出し側が NotConnected へ倒す）
        let plain = dir.join("plain.jsonl");
        std::fs::write(
            &plain,
            format!(
                "{}\n",
                json!({"type": "user", "message": {"role": "user", "content": "hi"}})
            ),
        )
        .unwrap();
        assert_eq!(read_link_at(&plain).unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
