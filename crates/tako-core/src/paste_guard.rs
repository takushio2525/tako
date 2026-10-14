//! paste_guard — 貼り付けの本文に API キー・パスワードらしきものがあるかの判定（Issue #2046）
//!
//! GUI の貼り付け（⌘V / Ctrl+V / Shift+Insert / メニューの「貼り付け」）は PTY へ流す前に
//! ここを通し、当たったら「送る / やめる」を確かめる。判定の正本はこの 1 実装で、
//! GUI・CLI `tako paste-guard check`・MCP `tako_settings`（action=paste_guard）が同じ
//! [`scan`] を引く。PTY の手前で止めるので、claude / codex / agy / 素のシェル / SSH 先の
//! どれへの貼り付けも同じように守れる。
//!
//! # 判定の三段（gitleaks / detect-secrets と同じ考え方）
//!
//! 1. **既知の形**（[`SHAPES`] と [`private_keys`] / [`jwts`] / [`url_credentials`]）:
//!    前置詞 + 本体の文字集合 + 長さ。誤検知はほぼ無い
//! 2. **代入の右辺**（[`assignments`]）: `password=…` / `"api_key": "…"` のように、
//!    鍵らしい名前へリテラルの値を入れている形。識別子・関数呼び出し・変数参照・
//!    数字だけの値は外す（ソースコードの `token: String` や `max_tokens: 4096` を拾わない）
//! 3. **形の分からないもの**（[`high_entropy`]）: 空白を含まない 20 文字以上の塊だけを、
//!    英大文字・英小文字・数字が混ざり、camelCase で語に分けると短い語ばかりになり
//!    （識別子は長い語の連なり）、文字が散らばっている（エントロピー）かで判定する。
//!    空白で区切られた普通の文章は塊に分かれて外れる。除外表（[`EXCLUSIONS`]）は
//!    行を足すだけで増やせる
//!
//! 閾値はランダム列と実データ（このリポジトリのソース・docs・git log）の分布を測って
//! 決めた（Issue #2046 の PR）: ランダムな英数 32 文字の見逃しは約 2%、
//! 実データで残るのは URL の中の ID とテスト用の偽の鍵だけ（URL は除外表で外れる）。
//!
//! # 守ること
//!
//! - [`Finding`] は**伏せ字だけを持ち、生の値を持たない**（`Debug` で出しても漏れない）。
//!   本文・検出値を診断ログ（persist.log / perf.log / stderr）へ出さない（AGENTS.md の
//!   絶対ルール）。ログへ書くのは [`log_line`] の 1 本だけで、そこは規則名と件数しか持たない
//! - 先頭 [`MAX_SCAN_BYTES`] だけを見る（巨大な貼り付けでも UI スレッドで数 ms に収める）
//! - regex を足さない（tako-core の依存に無い。表で書ける形だけを持つ）
//!
//! # 段・許可リスト・規則の無効化（Issue #2050）
//!
//! - [`Policy`]（settings.json の `paste_guard_policy` 節）を [`scan_with`] へ渡す。
//!   許可リスト（[`AllowEntry`]）に載せた値・無効にした規則の当たりは**範囲だけ取って報告しない**
//!   （後ろの段が同じ文字を拾い直さない = 無効にした `openai_api_key` の鍵を `high_entropy` が
//!   拾い直して出し直すことはない）
//! - 許可リストは**生の値を持たない**（[`fingerprint`] = 区切りの語つきの SHA-256 と伏せ字だけ。
//!   settings.json が `tako config` で他の機器へ渡っても値は渡らない）
//! - 当たったときの振る舞いは [`Mode`]（確認 / 警告だけ）。経路ごとの最終判断は [`Verdict`]
//! - [`Acks`]: tako が自分で流した値・人が確かめた値の指紋をペインごとにメモリだけで覚え、
//!   mod の送信時の確認（2 段目）で同じ値を二度確かめない

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// 既定は on（Issue #2046 のユーザー確定。`settings.json` に項目が無い人も on で読む）
pub const DEFAULT_ENABLED: bool = true;

/// 判定する先頭のバイト数。これより後ろは見ない（巨大な貼り付けの所要を抑える）
pub const MAX_SCAN_BYTES: usize = 64 * 1024;

/// 形の分からない塊として見る最小の長さ（文字）
pub const MIN_CHUNK_CHARS: usize = 20;

/// 形の分からない塊として見る最大の長さ（文字）。これより長いのはファイルの中身の塊
/// （base64 の画像・証明書の連結など）で、鍵として貼る長さではない
pub const MAX_CHUNK_CHARS: usize = 512;

/// camelCase で語に分けたとき、2 文字以下の語に属する英数字の割合の下限。
/// ランダムな英数は 0.3 を超え（32 文字の下位 1% で 0.25）、識別子は長い語の連なりで下回る
const SHORT_WORD_RATIO_MIN: f64 = 0.3;

/// 塊のエントロピー（ビット / 文字）の下限
const ENTROPY_MIN: f64 = 3.3;

/// 代入の右辺として見る値の最小の長さ（文字）
const ASSIGNED_VALUE_MIN_CHARS: usize = 6;

/// 1 回の判定で採る件数の上限。これに達したら残りは見ない（確認を出すには 1 件で足り、
/// 画面に並べるのは数件。上限が無いと塊だらけの貼り付けで重なり判定が件数の 2 乗で効く =
/// 実測で 64 KB の代入だらけが 6.6 ms → 上限で数百 µs）
pub const MAX_FINDINGS: usize = 100;

/// 1 回の判定で範囲として取る件数の上限（報告しない許可・無効の分も含む）。
/// 許可リストの値だらけの貼り付けでも重なり判定を件数の 2 乗にしない
const MAX_SPANS: usize = MAX_FINDINGS * 4;

/// 許可リストの上限（件）。settings.json を際限なく太らせない
pub const MAX_ALLOW: usize = 200;

/// 確認済みの値を覚えておく時間（[`Acks`]）。tako が流したプロンプトが入力欄に入ってから
/// 送信されるまで・人が確かめてから Enter を押すまでの余裕
pub const ACK_TTL: Duration = Duration::from_secs(10 * 60);

/// 確認済みの値の上限（件。古いものから捨てる）
pub const MAX_ACKS: usize = 512;

/// A/B（`TAKO_2046_LEGACY=1`）: #2046 前（確認なしで貼る）へ戻す。
/// **ゲートはこの 1 か所**で、tako-control / tako-app もここを読む
pub fn legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_2046_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// A/B（`TAKO_2050_LEGACY=1`）: #2050 前へ戻す = 段・許可リスト・規則の無効化を当てず
/// （常に確認・全規則）、`tako send` / MCP `tako_send_input`・スマホの入力・mod の送信時は
/// 判定しない（GUI の貼り付けだけを #2046 のまま確かめる）。**ゲートはこの 1 か所**
pub fn legacy_2050() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_2050_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// 当たった規則。`as_str` の綴りは CLI / MCP の応答に載る公開の名前
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rule {
    AnthropicApiKey,
    OpenAiApiKey,
    GithubToken,
    GithubPat,
    GitlabToken,
    SlackToken,
    SlackWebhook,
    DiscordWebhook,
    AwsAccessKey,
    GoogleApiKey,
    StripeKey,
    NpmToken,
    HuggingfaceToken,
    SendgridKey,
    PrivateKey,
    Jwt,
    UrlCredentials,
    Assignment,
    HighEntropy,
}

impl Rule {
    /// すべての規則（docs・テストの網羅用）
    pub const ALL: [Rule; 19] = [
        Rule::AnthropicApiKey,
        Rule::OpenAiApiKey,
        Rule::GithubToken,
        Rule::GithubPat,
        Rule::GitlabToken,
        Rule::SlackToken,
        Rule::SlackWebhook,
        Rule::DiscordWebhook,
        Rule::AwsAccessKey,
        Rule::GoogleApiKey,
        Rule::StripeKey,
        Rule::NpmToken,
        Rule::HuggingfaceToken,
        Rule::SendgridKey,
        Rule::PrivateKey,
        Rule::Jwt,
        Rule::UrlCredentials,
        Rule::Assignment,
        Rule::HighEntropy,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Rule::AnthropicApiKey => "anthropic_api_key",
            Rule::OpenAiApiKey => "openai_api_key",
            Rule::GithubToken => "github_token",
            Rule::GithubPat => "github_pat",
            Rule::GitlabToken => "gitlab_token",
            Rule::SlackToken => "slack_token",
            Rule::SlackWebhook => "slack_webhook",
            Rule::DiscordWebhook => "discord_webhook",
            Rule::AwsAccessKey => "aws_access_key",
            Rule::GoogleApiKey => "google_api_key",
            Rule::StripeKey => "stripe_key",
            Rule::NpmToken => "npm_token",
            Rule::HuggingfaceToken => "huggingface_token",
            Rule::SendgridKey => "sendgrid_key",
            Rule::PrivateKey => "private_key",
            Rule::Jwt => "jwt",
            Rule::UrlCredentials => "url_credentials",
            Rule::Assignment => "assignment",
            Rule::HighEntropy => "high_entropy",
        }
    }

    /// 公開の名前（[`Rule::as_str`]）から引く（規則の無効化の入口が綴りを検証する）
    pub fn parse(name: &str) -> Option<Rule> {
        Rule::ALL.into_iter().find(|r| r.as_str() == name)
    }
}

/// 当たった 1 か所。**生の値は持たない**（伏せ字と位置だけ）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub rule: Rule,
    /// 本文の何行目か（1 始まり）
    pub line: usize,
    /// 伏せ字（[`mask`]。前置詞 + 先頭少し + `…` + 末尾少し）
    pub masked: String,
    /// 本文の中の範囲（バイト。ASCII の位置なので文字境界）
    pub start: usize,
    pub end: usize,
}

/// 1 回の判定の結果
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanReport {
    pub findings: Vec<Finding>,
    /// 貼り付けの全体のバイト数
    pub total_bytes: usize,
    /// 実際に見たバイト数（[`MAX_SCAN_BYTES`] で打ち切る）
    pub scanned_bytes: usize,
    /// 許可リストの値なので報告しなかった当たりの数（#2050）
    pub allowed: usize,
    /// 無効にした規則なので報告しなかった当たりの数（#2050）
    pub disabled: usize,
}

impl ScanReport {
    /// 鍵らしきものが 1 つでもあったか
    pub fn hit(&self) -> bool {
        !self.findings.is_empty()
    }

    /// 先頭だけを見たか（後ろは判定していない）
    pub fn truncated(&self) -> bool {
        self.scanned_bytes < self.total_bytes
    }

    /// 当たった規則（重複なし・出てきた順）
    pub fn rules(&self) -> Vec<Rule> {
        let mut out = Vec::new();
        for f in &self.findings {
            if !out.contains(&f.rule) {
                out.push(f.rule);
            }
        }
        out
    }
}

/// 確認ダイアログへの答え（GUI のボタン・CLI `tako paste-guard send|cancel`・MCP の `respond`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// そのまま PTY へ送る
    Send,
    /// 送らずに捨てる（PTY へ 1 バイトも届かない）
    Cancel,
}

impl Answer {
    pub const VALUES: [&'static str; 2] = ["send", "cancel"];

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "send" => Some(Answer::Send),
            "cancel" => Some(Answer::Cancel),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Answer::Send => "send",
            Answer::Cancel => "cancel",
        }
    }
}

/// 確認待ちの貼り付けの要約（応答・画面用。**本文は持たない**）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSummary {
    /// 送り先のペイン（貼り付けた時点のフォーカス）
    pub pane: u64,
    pub findings: Vec<Finding>,
    pub total_bytes: usize,
    pub truncated: bool,
}

/// 起動からの累計（2 回読んだ差で効いているかを外から確かめる口）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PasteGuardStats {
    /// 判定した貼り付けの数
    pub checked: u64,
    /// 確認を出した数
    pub prompted: u64,
    /// 確認で「送る」を選んだ数
    pub sent: u64,
    /// 確認で「やめる」を選んだ数（その貼り付けは PTY へ届いていない）
    pub cancelled: u64,
    /// 「警告だけ」の段で止めずに送った数（#2050）
    pub warned: u64,
    /// 最後の判定の所要（マイクロ秒）
    pub last_scan_us: u64,
    /// 最後の判定で見たバイト数
    pub last_scanned_bytes: usize,
}

/// ホストが返す現在の状態（`ControlHost::paste_guard_status`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasteGuardStatus {
    pub enabled: bool,
    pub legacy: bool,
    pub pending: Option<PendingSummary>,
    pub stats: PasteGuardStats,
    /// 段・許可リスト・無効にした規則（#2050。settings.json の `paste_guard_policy`）
    pub policy: Policy,
}

impl Default for PasteGuardStatus {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_ENABLED,
            legacy: false,
            pending: None,
            stats: PasteGuardStats::default(),
            policy: Policy::default(),
        }
    }
}

/// 当たったときの振る舞い（#2050）。既定は確認（#2046 のまま）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// 送る前に「送る / やめる」を確かめる（人が答えられる経路だけ。AI の送信は警告だけ）
    #[default]
    Confirm,
    /// 止めずに送り、警告だけを出す
    Warn,
}

impl Mode {
    pub const VALUES: [&'static str; 2] = ["confirm", "warn"];

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "confirm" => Some(Mode::Confirm),
            "warn" => Some(Mode::Warn),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Confirm => "confirm",
            Mode::Warn => "warn",
        }
    }

    fn is_confirm(&self) -> bool {
        *self == Mode::Confirm
    }
}

/// 許可リストの 1 行（#2050）。**生の値は持たない**（[`fingerprint`] と伏せ字と規則名だけ）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowEntry {
    /// [`fingerprint`] の 16 進 64 文字
    pub fingerprint: String,
    /// 足したときに当たっていた規則（表示用）
    pub rule: String,
    /// 足したときの伏せ字（表示用。どの値かを人が見分ける）
    pub masked: String,
}

impl AllowEntry {
    /// 一覧・削除で使う短い ID（指紋の先頭 12 文字）
    pub fn id(&self) -> &str {
        self.fingerprint.get(..12).unwrap_or(&self.fingerprint)
    }
}

/// settings.json の `paste_guard_policy` 節（#2050）。
/// **旧ファイルは節ごと無くても既定（確認・許可なし・全規則）で読める**（各キーも default）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    #[serde(default, skip_serializing_if = "Mode::is_confirm")]
    pub mode: Mode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<AllowEntry>,
    /// 無効にした規則の名前（[`Rule::as_str`]）。知らない名前（新しい版が書いた規則）も残す
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_rules: Vec<String>,
}

impl Policy {
    pub fn is_default(&self) -> bool {
        *self == Policy::default()
    }

    /// その規則を無効にしているか
    pub fn rule_disabled(&self, rule: Rule) -> bool {
        self.disabled_rules.iter().any(|r| r == rule.as_str())
    }

    /// その値（当たった範囲の文字）を許可リストに載せているか
    pub fn allows(&self, value: &str) -> bool {
        if self.allow.is_empty() {
            return false;
        }
        let fp = fingerprint(value);
        self.allow.iter().any(|a| a.fingerprint == fp)
    }

    /// `text` の当たり（今の規則の無効化を当てた上で、まだ許可していないもの）を許可リストへ足す。
    /// 足した行を返す（同じ値は 2 回足さない・[`MAX_ALLOW`] を超えたら `Err`）
    pub fn allow_text(&mut self, text: &str) -> Result<Vec<AllowEntry>, String> {
        let report = scan_with(text, self);
        let mut added: Vec<AllowEntry> = Vec::new();
        for (f, fp) in report.findings.iter().zip(fingerprints(text, &report)) {
            if added.iter().any(|a| a.fingerprint == fp) {
                continue;
            }
            added.push(AllowEntry {
                fingerprint: fp,
                rule: f.rule.as_str().to_string(),
                masked: f.masked.clone(),
            });
        }
        if self.allow.len() + added.len() > MAX_ALLOW {
            return Err(format!(
                "許可リストは {MAX_ALLOW} 件まで（今 {} 件・足す {} 件）",
                self.allow.len(),
                added.len()
            ));
        }
        self.allow.extend(added.iter().cloned());
        Ok(added)
    }

    /// 許可リストから外す。`id` は [`AllowEntry::id`] か指紋の先頭（4 文字以上）、`all` で全部。
    /// 外した件数を返す（当たる行が無ければ `Err`）
    pub fn unallow(&mut self, id: &str) -> Result<usize, String> {
        let before = self.allow.len();
        if id == "all" {
            self.allow.clear();
            return Ok(before);
        }
        if id.len() < 4 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!(
                "外す行は ID（16 進 4 文字以上）か all で指定する（指定: {id}）"
            ));
        }
        let id = id.to_ascii_lowercase();
        let hits = self
            .allow
            .iter()
            .filter(|a| a.fingerprint.starts_with(&id))
            .count();
        match hits {
            0 => Err(format!("許可リストに ID {id} の行が無い")),
            1 => {
                self.allow.retain(|a| !a.fingerprint.starts_with(&id));
                Ok(1)
            }
            n => Err(format!("ID {id} に {n} 行が当たる（もっと長く指定する）")),
        }
    }

    /// 規則の有効 / 無効を切り替える（名前は [`Rule::as_str`]。知らない名前は `Err`）
    pub fn set_rule(&mut self, name: &str, enabled: bool) -> Result<(), String> {
        let rule = Rule::parse(name).ok_or_else(|| {
            let names: Vec<&str> = Rule::ALL.iter().map(|r| r.as_str()).collect();
            format!("規則 {name} は無い（{}）", names.join(" / "))
        })?;
        self.disabled_rules.retain(|r| r != rule.as_str());
        if !enabled {
            self.disabled_rules.push(rule.as_str().to_string());
            // 表示と比較を安定させる（ALL の並び順）
            self.disabled_rules.sort_by_key(|r| {
                Rule::parse(r)
                    .and_then(|r| Rule::ALL.iter().position(|x| *x == r))
                    .unwrap_or(usize::MAX)
            });
        }
        Ok(())
    }
}

/// 人の入力を中継する送信の印（#2050。`Request::Send` の `paste_guard`。付けるのはスマホの PWA の
/// 中継だけで、無い送信は AI・スクリプトの送信 = 止めずに警告だけ）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relay {
    /// 人の入力: 段に従う（確認の段なら送らずに伏せ字を返し、人に確かめてもらう）
    Human,
    /// 人が確かめた: 送る（そのペインの確認済みとして覚える）
    Confirmed,
}

impl Relay {
    pub const VALUES: [&'static str; 2] = ["human", "confirmed"];

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "human" => Some(Relay::Human),
            "confirmed" => Some(Relay::Confirmed),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Relay::Human => "human",
            Relay::Confirmed => "confirmed",
        }
    }
}

/// 経路ごとの最終判断（#2050）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 当たりが無い（か、すべて確認済み）= そのまま通す
    Pass,
    /// 通して警告だけ出す
    Warn,
    /// 送る前に確かめる
    Confirm,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Warn => "warn",
            Verdict::Confirm => "confirm",
        }
    }

    /// 当たりの有無と段から決める。`can_confirm` = 人が答えられる経路か
    /// （AI・スクリプトの送信は答えられないので確認の段でも警告だけ）
    pub fn of(hit: bool, mode: Mode, can_confirm: bool) -> Verdict {
        match (hit, mode) {
            (false, _) => Verdict::Pass,
            (true, Mode::Confirm) if can_confirm => Verdict::Confirm,
            (true, _) => Verdict::Warn,
        }
    }
}

/// 許可リスト・確認済みの値の指紋（#2050）: 区切りの語つきの SHA-256 の 16 進 64 文字。
/// 生の値の代わりに覚える（区切りの語は他の用途の SHA-256 と突き合わせられないようにするため）
pub fn fingerprint(value: &str) -> String {
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    h.update(b"tako.paste_guard.v1\n");
    h.update(value.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// 判定の当たりごとの指紋（`report` は `text` の判定であること）
pub fn fingerprints(text: &str, report: &ScanReport) -> Vec<String> {
    report
        .findings
        .iter()
        .map(|f| fingerprint(text.get(f.start..f.end).unwrap_or("")))
        .collect()
}

/// tako が自分で流した値・人が確かめた値（#2050）。ペインごとの指紋を [`ACK_TTL`] だけ
/// **メモリにだけ**覚える。mod の送信時の確認（2 段目）は、ここに載った値を二度確かめない
/// （GUI の確認で「送る」を選んだ鍵・worker へ流した初回プロンプトで、Enter のたびに止めない）。
/// `Debug` を付けない（指紋も診断ログへ出す理由が無い）
#[derive(Default)]
pub struct Acks {
    entries: Vec<(u64, String, Instant)>,
}

impl Acks {
    /// `text` の当たり（規則・許可リストを当てない全部）をそのペインの確認済みとして覚える
    pub fn note_text(&mut self, pane: u64, text: &str, now: Instant) {
        if text.is_empty() {
            return;
        }
        let report = scan(text);
        if !report.hit() {
            return;
        }
        self.note(pane, fingerprints(text, &report), now);
    }

    /// 指紋をそのペインの確認済みとして覚える
    pub fn note(&mut self, pane: u64, fingerprints: Vec<String>, now: Instant) {
        self.entries
            .retain(|(_, _, at)| now.saturating_duration_since(*at) < ACK_TTL);
        for fp in fingerprints {
            self.entries.retain(|(p, f, _)| !(*p == pane && *f == fp));
            self.entries.push((pane, fp, now));
        }
        if self.entries.len() > MAX_ACKS {
            let over = self.entries.len() - MAX_ACKS;
            self.entries.drain(..over);
        }
    }

    /// その指紋がそのペインの確認済みか
    pub fn covers(&self, pane: u64, fingerprint: &str, now: Instant) -> bool {
        self.entries.iter().any(|(p, f, at)| {
            *p == pane && f == fingerprint && now.saturating_duration_since(*at) < ACK_TTL
        })
    }

    /// 覚えている件数（期限切れを含まない。状態の応答用）
    pub fn len(&self, now: Instant) -> usize {
        self.entries
            .iter()
            .filter(|(_, _, at)| now.saturating_duration_since(*at) < ACK_TTL)
            .count()
    }

    pub fn is_empty(&self, now: Instant) -> bool {
        self.len(now) == 0
    }
}

/// 貼り付けの本文を判定する（**本文は先頭 [`MAX_SCAN_BYTES`] だけを見る**）。
/// 許可リスト・規則の無効化を当てない全部（[`scan_with`] の既定の [`Policy`]）
pub fn scan(text: &str) -> ScanReport {
    scan_with(text, &Policy::default())
}

/// [`scan`] に許可リストと規則の無効化（#2050）を当てる。段（[`Mode`]）は判定に関係しない
/// （当たったときの振る舞いは呼び手が [`Verdict::of`] で決める）
pub fn scan_with(text: &str, policy: &Policy) -> ScanReport {
    let total_bytes = text.len();
    let cut = text.floor_char_boundary(MAX_SCAN_BYTES.min(total_bytes));
    // 切り出し安全: floor_char_boundary で文字境界へ丸めた位置
    let body = &text[..cut];
    let mut spans = Spans {
        body,
        policy,
        list: Vec::new(),
        kept: 0,
        allowed: 0,
        disabled: 0,
    };
    known_shapes(body, &mut spans);
    private_keys(body, &mut spans);
    jwts(body, &mut spans);
    url_credentials(body, &mut spans);
    assignments(body, &mut spans);
    high_entropy(body, &mut spans);
    let (allowed, disabled) = (spans.allowed, spans.disabled);
    // 許可・無効の範囲は後ろの段に拾い直させないためだけに取った（報告しない）
    let mut spans: Vec<Span> = spans
        .list
        .into_iter()
        .filter_map(|(s, kept)| kept.then_some(s))
        .collect();
    spans.sort_by_key(|s| s.start);
    let mut findings = Vec::with_capacity(spans.len());
    let mut line = 1;
    let mut cursor = 0;
    for span in spans {
        line += body.as_bytes()[cursor..span.start]
            .iter()
            .filter(|&&b| b == b'\n')
            .count();
        cursor = span.start;
        findings.push(Finding {
            rule: span.rule,
            line,
            masked: span.masked,
            start: span.start,
            end: span.end,
        });
    }
    ScanReport {
        findings,
        total_bytes,
        scanned_bytes: cut,
        allowed,
        disabled,
    }
}

/// 伏せ字。`prefix_chars` は形が決まっている前置詞の長さ（`sk-ant-` など。秘密ではないので
/// そのまま見せる）。本体は末尾 4 文字まで（前置詞が無ければ先頭 3 文字も）を見せ、
/// **見せるのは本体の 1/3 未満**（短い値ほど見せない。6 文字未満は 1 文字も見せない）
pub fn mask(value: &str, prefix_chars: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    let p = prefix_chars.min(chars.len());
    let body = &chars[p..];
    let n = body.len();
    let tail = (n / 6).min(4);
    let head = if p > 0 { 0 } else { (n / 6).min(3) };
    let mut out: String = chars[..p].iter().collect();
    out.extend(&body[..head]);
    out.push('…');
    out.extend(&body[n - tail..]);
    out
}

/// 診断ログ（persist.log）へ書く 1 行。**規則名と件数とバイト数だけ**を持つ
/// （本文・伏せ字を含めない = 絶対ルール）。`outcome` は "prompted" / "send" / "cancel" 等
pub fn log_line(outcome: &str, pane: u64, report: &ScanReport) -> String {
    let rules: Vec<&str> = report.rules().iter().map(|r| r.as_str()).collect();
    format!(
        "貼り付けの確認: {outcome} pane={pane} rules=[{}] findings={} bytes={}",
        rules.join(","),
        report.findings.len(),
        report.total_bytes
    )
}

// ---------------------------------------------------------------------------
// 内部
// ---------------------------------------------------------------------------

struct Span {
    rule: Rule,
    start: usize,
    end: usize,
    masked: String,
}

/// 1 回の判定で取った範囲（#2050: 許可・無効の当たりも範囲としては取り、報告しない印を付ける）
struct Spans<'a> {
    body: &'a str,
    policy: &'a Policy,
    /// 取った範囲と、報告するか
    list: Vec<(Span, bool)>,
    /// 報告する数
    kept: usize,
    allowed: usize,
    disabled: usize,
}

/// 既に採った範囲と重ならなければ採る（先に呼ばれた段 = より確かな形が勝つ）。
/// 上限（[`MAX_FINDINGS`] / [`MAX_SPANS`]）に達していたら採らない。
/// 無効にした規則・許可リストの値は範囲だけ取って報告しない（後ろの段に拾い直させない）
fn push(spans: &mut Spans, span: Span) {
    if full(spans)
        || spans
            .list
            .iter()
            .any(|(s, _)| span.start < s.end && s.start < span.end)
    {
        return;
    }
    let kept = if spans.policy.rule_disabled(span.rule) {
        spans.disabled += 1;
        false
    } else if spans
        .policy
        .allows(spans.body.get(span.start..span.end).unwrap_or(""))
    {
        spans.allowed += 1;
        false
    } else {
        spans.kept += 1;
        true
    };
    spans.list.push((span, kept));
}

/// 上限に達したか（各段のループはこれで打ち切る）
fn full(spans: &Spans) -> bool {
    spans.kept >= MAX_FINDINGS || spans.list.len() >= MAX_SPANS
}

fn is_b64url(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

fn is_alnum(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

fn is_alnum_underscore(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_alnum_dash(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-'
}

fn is_upper_digit(b: u8) -> bool {
    b.is_ascii_uppercase() || b.is_ascii_digit()
}

fn is_url_path(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'/' || b == b'_' || b == b'-'
}

fn is_b64url_dot(b: u8) -> bool {
    is_b64url(b) || b == b'.'
}

/// `from` から `pred` を満たすバイトが続く終端
fn take_while(bytes: &[u8], from: usize, pred: fn(u8) -> bool) -> usize {
    let mut i = from;
    while i < bytes.len() && pred(bytes[i]) {
        i += 1;
    }
    i
}

/// 既知の形 1 行。前置詞 + 本体の文字集合 + 本体の長さ（文字）
struct Shape {
    rule: Rule,
    prefixes: &'static [&'static str],
    body: fn(u8) -> bool,
    min: usize,
    max: usize,
    /// 本体に英大文字・英小文字・数字のすべてが要る（`sk-` で始まる kebab-case の名前を外す）
    mixed: bool,
}

/// 既知の形の表。**上の行ほど優先**（同じ位置に当たったら上の行が採られる）。
/// 足すときは 1 行足し、`tests::known_shapes_each_hit` へ偽の例を 1 つ足す
const SHAPES: &[Shape] = &[
    Shape {
        rule: Rule::AnthropicApiKey,
        prefixes: &["sk-ant-"],
        body: is_b64url,
        min: 20,
        max: 300,
        mixed: false,
    },
    Shape {
        rule: Rule::OpenAiApiKey,
        prefixes: &["sk-"],
        body: is_b64url,
        min: 20,
        max: 300,
        mixed: true,
    },
    Shape {
        rule: Rule::GithubToken,
        prefixes: &["ghp_", "gho_", "ghu_", "ghs_", "ghr_"],
        body: is_alnum,
        min: 30,
        max: 255,
        mixed: false,
    },
    Shape {
        rule: Rule::GithubPat,
        prefixes: &["github_pat_"],
        body: is_alnum_underscore,
        min: 22,
        max: 255,
        mixed: false,
    },
    Shape {
        rule: Rule::GitlabToken,
        prefixes: &["glpat-"],
        body: is_b64url,
        min: 20,
        max: 100,
        mixed: false,
    },
    Shape {
        rule: Rule::SlackToken,
        prefixes: &["xoxb-", "xoxp-", "xoxa-", "xoxs-", "xoxr-", "xoxe-"],
        body: is_alnum_dash,
        min: 10,
        max: 300,
        mixed: false,
    },
    Shape {
        rule: Rule::SlackWebhook,
        prefixes: &["https://hooks.slack.com/services/"],
        body: is_url_path,
        min: 20,
        max: 200,
        mixed: false,
    },
    Shape {
        rule: Rule::DiscordWebhook,
        prefixes: &[
            "https://discord.com/api/webhooks/",
            "https://discordapp.com/api/webhooks/",
        ],
        body: is_url_path,
        min: 20,
        max: 200,
        mixed: false,
    },
    Shape {
        rule: Rule::AwsAccessKey,
        prefixes: &["AKIA", "ASIA"],
        body: is_upper_digit,
        min: 16,
        max: 16,
        mixed: false,
    },
    Shape {
        rule: Rule::GoogleApiKey,
        prefixes: &["AIza"],
        body: is_b64url,
        min: 35,
        max: 35,
        mixed: false,
    },
    Shape {
        rule: Rule::StripeKey,
        prefixes: &["sk_live_", "rk_live_", "sk_test_", "rk_test_"],
        body: is_alnum,
        min: 16,
        max: 255,
        mixed: false,
    },
    Shape {
        rule: Rule::NpmToken,
        prefixes: &["npm_"],
        body: is_alnum,
        min: 36,
        max: 36,
        mixed: false,
    },
    Shape {
        rule: Rule::HuggingfaceToken,
        prefixes: &["hf_"],
        body: is_alnum,
        min: 30,
        max: 40,
        mixed: false,
    },
    Shape {
        rule: Rule::SendgridKey,
        prefixes: &["SG."],
        body: is_b64url_dot,
        min: 60,
        max: 80,
        mixed: false,
    },
];

fn has_upper_lower_digit(bytes: &[u8]) -> bool {
    bytes.iter().any(u8::is_ascii_uppercase)
        && bytes.iter().any(u8::is_ascii_lowercase)
        && bytes.iter().any(u8::is_ascii_digit)
}

/// 第 1 段: 既知の形の表
fn known_shapes(body: &str, spans: &mut Spans) {
    let bytes = body.as_bytes();
    for shape in SHAPES {
        for prefix in shape.prefixes {
            for (start, _) in body.match_indices(prefix) {
                if full(spans) {
                    return;
                }
                // 前置詞の前が英数字なら語の途中（`task-…` の `sk-` など）
                if start > 0 && is_alnum(bytes[start - 1]) {
                    continue;
                }
                let body_start = start + prefix.len();
                let end = take_while(bytes, body_start, shape.body);
                let n = end - body_start;
                if n < shape.min || n > shape.max {
                    continue;
                }
                // 本体の直後も英数字なら、もっと長い語の一部
                if end < bytes.len() && is_alnum(bytes[end]) {
                    continue;
                }
                if shape.mixed && !has_upper_lower_digit(&bytes[body_start..end]) {
                    continue;
                }
                push(
                    spans,
                    Span {
                        rule: shape.rule,
                        start,
                        end,
                        // 切り出し安全: ASCII の前置詞を find した位置と、ASCII の本体の終端
                        masked: mask(&body[start..end], prefix.len()),
                    },
                );
            }
        }
    }
}

/// 第 1 段: `-----BEGIN … PRIVATE KEY-----` の塊（PGP の `PRIVATE KEY BLOCK` も）
fn private_keys(body: &str, spans: &mut Spans) {
    const BEGIN: &str = "-----BEGIN ";
    const DASHES: &str = "-----";
    for (start, _) in body.match_indices(BEGIN) {
        if full(spans) {
            return;
        }
        let label_from = start + BEGIN.len();
        // 見出しは BEGIN の後の最初の `-----` まで（JSON に `\n` で埋めた鍵でも本体を含めない）
        let Some(close) = body[label_from..].find(DASHES) else {
            continue;
        };
        if close > 60 {
            continue;
        }
        // 切り出し安全: ASCII の区切り `-----` を find した位置
        let label = &body[label_from..label_from + close];
        if !label.contains("PRIVATE KEY") {
            continue;
        }
        let header_end = label_from + close + DASHES.len();
        // 塊の終わり: 次の `-----END ` の見出しの閉じまで（無ければ本文の終わりまで）
        let end = body[header_end..]
            .find("-----END ")
            .and_then(|e| {
                let from = header_end + e + "-----END ".len();
                body[from..].find(DASHES).map(|c| from + c + DASHES.len())
            })
            .unwrap_or(body.len());
        push(
            spans,
            Span {
                rule: Rule::PrivateKey,
                start,
                end,
                // 見出しそのもの（鍵の種類が分かる。秘密ではない）
                masked: body[start..header_end].to_string(),
            },
        );
    }
}

/// 第 1 段: JWT（`eyJ…`.`eyJ…`.`…`）
fn jwts(body: &str, spans: &mut Spans) {
    const SEG_MIN: usize = 10;
    let bytes = body.as_bytes();
    for (start, _) in body.match_indices("eyJ") {
        if full(spans) {
            return;
        }
        if start > 0 && is_b64url(bytes[start - 1]) {
            continue;
        }
        let seg1 = take_while(bytes, start, is_b64url);
        if seg1 - start < SEG_MIN || bytes.get(seg1) != Some(&b'.') {
            continue;
        }
        let s2 = seg1 + 1;
        if !bytes[s2..].starts_with(b"eyJ") {
            continue;
        }
        let seg2 = take_while(bytes, s2, is_b64url);
        if seg2 - s2 < SEG_MIN || bytes.get(seg2) != Some(&b'.') {
            continue;
        }
        let s3 = seg2 + 1;
        let end = take_while(bytes, s3, is_b64url);
        if end - s3 < SEG_MIN {
            continue;
        }
        push(
            spans,
            Span {
                rule: Rule::Jwt,
                start,
                end,
                // 切り出し安全: ASCII の `eyJ` と base64url の終端
                masked: mask(&body[start..end], 3),
            },
        );
    }
}

/// 第 1 段: URL に埋め込んだパスワード（`scheme://user:password@host`）
fn url_credentials(body: &str, spans: &mut Spans) {
    for (sep, _) in body.match_indices("://") {
        if full(spans) {
            return;
        }
        let after = sep + 3;
        let rest = &body[after..];
        let stop = rest
            .find(|c: char| {
                c == '/' || c.is_whitespace() || matches!(c, '?' | '#' | '"' | '\'' | '>' | '<')
            })
            .unwrap_or(rest.len());
        let authority = &rest[..stop];
        let Some(at) = authority.rfind('@') else {
            continue;
        };
        let userinfo = &authority[..at];
        let Some(colon) = userinfo.find(':') else {
            continue;
        };
        let password = &userinfo[colon + 1..];
        if password.chars().count() < 3 || is_placeholder(password) {
            continue;
        }
        let start = after + colon + 1;
        push(
            spans,
            Span {
                rule: Rule::UrlCredentials,
                start,
                end: after + at,
                masked: mask(password, 0),
            },
        );
    }
}

/// 鍵らしい名前の語尾（小文字の英数だけにした形で見る。`DB_PASSWORD` / `apiKey` /
/// `client-secret` / `AWS_SECRET_ACCESS_KEY` がそれぞれ当たる）
const SECRET_KEY_SUFFIXES: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "passphrase",
    "secret",
    "secretkey",
    "secretkeybase",
    "token",
    "apikey",
    "accesskey",
    "privatekey",
    "credential",
    "credentials",
];

/// 値として「入れていない」ことを示す綴り（小文字で比べる）
const PLACEHOLDERS: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "token",
    "changeme",
    "change_me",
    "change-me",
    "placeholder",
    "redacted",
    "example",
    "dummy",
    "sample",
    "null",
    "none",
    "nil",
    "undefined",
    "true",
    "false",
    "required",
    "optional",
];

/// 値の中にあれば「入れていない」とみなす語（小文字で部分一致。テストの `"test-token"` /
/// `"stale-token"` / `"dummy-pass"` を拾わない。`secret` / `password` は本物の弱い
/// パスワードにも入るので入れない）
const PLACEHOLDER_WORDS: &[&str] = &[
    "example",
    "dummy",
    "sample",
    "placeholder",
    "changeme",
    "redacted",
    "xxxx",
    "test",
    "fake",
    "mock",
    "token",
];

fn is_placeholder(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    if PLACEHOLDERS.contains(&lower.as_str()) || PLACEHOLDER_WORDS.iter().any(|w| lower.contains(w))
    {
        return true;
    }
    // `your_password` / `<token>` / `${TOKEN}` / `{{ .Values.pw }}` / `$PASS` / `%(pw)s` / `!vault`
    if lower.starts_with("your")
        || lower.starts_with('<')
        || lower.starts_with('$')
        || lower.starts_with('%')
        || lower.starts_with('!')
        || lower.contains("${")
        || lower.contains("{{")
    {
        return true;
    }
    // `********` / `xxxxxxxx` / `........`（同じ 1 文字の繰り返し）
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => chars.all(|c| c == first),
        None => true,
    }
}

/// `v1.2.3` / `1.2.3`（版数）
fn is_version(value: &str) -> bool {
    let v = value.strip_prefix(['v', 'V']).unwrap_or(value);
    !v.is_empty()
        && v.contains('.')
        && v.split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// `a.b.c` の形の識別子の連なり（`req.body.password` / `os.environ` など）
fn is_dotted_identifier(value: &str) -> bool {
    value.contains('.')
        && value.split('.').all(|seg| {
            let b = seg.as_bytes();
            !b.is_empty()
                && (b[0].is_ascii_alphabetic() || b[0] == b'_')
                && b.iter().all(|&c| is_alnum_underscore(c))
        })
}

/// 代入の右辺が「値を書いている」か。quoted = 引用符で囲んである
fn literal_value(value: &str, quoted: bool) -> bool {
    // 鍵・パスワードは ASCII（日本語の文の中の `token: …` を拾わない）
    if !value.is_ascii()
        || value.chars().count() < ASSIGNED_VALUE_MIN_CHARS
        || value.chars().any(char::is_whitespace)
        || is_dotted_identifier(value)
        || is_placeholder(value)
        || is_version(value)
        || value.bytes().all(|b| b.is_ascii_digit())
        || value.starts_with('/')
        || value.starts_with("~/")
        || value.starts_with("./")
        || value.starts_with("../")
        || value.contains("://")
        // 書式の差し込み（`"header.{payload}.signature"` / `"{}"`）
        || (value.contains('{') && value.contains('}'))
    {
        return false;
    }
    // 数字か、英大文字と英小文字の両方を含むものだけを値とみなす（`"tok-abc"` / `hunter` /
    // `my-secret` のような単語の値を拾わない。すべて小文字の合言葉は見逃す側に倒す）
    let b = value.as_bytes();
    let complex = b.iter().any(u8::is_ascii_digit)
        || (b.iter().any(u8::is_ascii_uppercase) && b.iter().any(u8::is_ascii_lowercase));
    if !complex {
        return false;
    }
    if quoted {
        return true;
    }
    // 引用符の無い値: 式・型・変数参照を外す（`compute_token(x)` / `Option<String>` /
    // `catalog::CATALOG` / `password`）。数字か記号を含むものだけを値とみなす
    // 型名（`token: String,`）を拾わないよう、数字か記号を含むものだけを値とみなす
    !value.contains(['(', ')', '[', ']', '{', '}', '<', '>', ':'])
        && value
            .bytes()
            .any(|b| b.is_ascii_digit() || (b.is_ascii_punctuation() && b != b'_' && b != b'.'))
}

/// 第 2 段: 鍵らしい名前への代入（`KEY=value` / `key: value` / `"key": "value"` / `key := "v"` /
/// `key => 'v'` / `--password=value` / `?token=value`）
fn assignments(body: &str, spans: &mut Spans) {
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() && !full(spans) {
        let b = bytes[i];
        if b != b'=' && b != b':' {
            i += 1;
            continue;
        }
        let prev = if i > 0 { bytes[i - 1] } else { b'\n' };
        let next = bytes.get(i + 1).copied().unwrap_or(b'\n');
        // 比較・パス区切り（`==` / `!=` / `<=` / `>=` / `::`）は代入ではない
        let comparison = match b {
            b'=' => next == b'=' || matches!(prev, b'=' | b'!' | b'<' | b'>' | b':'),
            _ => next == b':' || prev == b':',
        };
        if comparison {
            i += 2;
            continue;
        }
        // 左辺の名前（空白と閉じ引用符を飛ばして後ろ向きに読む）
        let mut k = i;
        while k > 0 && matches!(bytes[k - 1], b' ' | b'\t') {
            k -= 1;
        }
        if k > 0 && matches!(bytes[k - 1], b'"' | b'\'') {
            k -= 1;
        }
        let key_end = k;
        while k > 0 && (is_alnum_underscore(bytes[k - 1]) || matches!(bytes[k - 1], b'-' | b'.')) {
            k -= 1;
        }
        let key: String = body[k..key_end]
            .bytes()
            .filter(u8::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_lowercase() as char)
            .collect();
        if key.is_empty() || !SECRET_KEY_SUFFIXES.iter().any(|s| key.ends_with(s)) {
            i += 1;
            continue;
        }
        // 演算子の残り（`:=` / `=>`）と空白を飛ばす
        let mut v = i + 1;
        if (b == b':' && next == b'=') || (b == b'=' && next == b'>') {
            v += 1;
        }
        while v < bytes.len() && matches!(bytes[v], b' ' | b'\t') {
            v += 1;
        }
        let (start, end, quoted) = match bytes.get(v) {
            Some(&q @ (b'"' | b'\'' | b'`')) => {
                let from = v + 1;
                let line_end = body[from..]
                    .find('\n')
                    .map(|n| from + n)
                    .unwrap_or(body.len());
                match body[from..line_end].find(q as char) {
                    Some(close) => (from, from + close, true),
                    None => {
                        i += 1;
                        continue;
                    }
                }
            }
            Some(_) => {
                let end = body[v..]
                    .find(|c: char| {
                        c.is_whitespace() || matches!(c, ',' | ';' | '&' | '"' | '\'' | '`')
                    })
                    .map(|n| v + n)
                    .unwrap_or(body.len());
                (v, end, false)
            }
            None => break,
        };
        // 切り出し安全: ASCII の区切り（引用符・空白・`,;&`）を find した位置
        let value = &body[start..end];
        // 引用符の無い値を採るのは、設定・コマンドの書き方だけ: 空白を挟まない代入
        // （`KEY=value` / `--password=value` / `?token=value`）か、名前が行頭にある形
        // （YAML / INI / .env の `key: value` / `key = value`）。ソースコードの
        // `let token = token0` / `token: Token,` は外す
        let config_style = quoted || {
            let tight = !matches!(prev, b' ' | b'\t') && !matches!(next, b' ' | b'\t');
            let line_start = body[..k].rfind('\n').map(|n| n + 1).unwrap_or(0);
            let lead = body[line_start..k].trim();
            tight || matches!(lead, "" | "-" | "export" | "set")
        };
        if config_style && literal_value(value, quoted) {
            push(
                spans,
                Span {
                    rule: Rule::Assignment,
                    start,
                    end,
                    masked: mask(value, 0),
                },
            );
        }
        i = end.max(i + 1);
    }
}

/// 形の分からない塊の候補 1 つ（除外表へ渡す文脈つき）
pub struct Candidate<'a> {
    /// 塊そのもの（`[A-Za-z0-9_\-+/=]` の連なり）
    pub run: &'a str,
    /// 塊を含む空白区切りの語
    pub word: &'a str,
    /// 1 つ前の語（`ssh-rsa AAAA…` の判定に使う）
    pub prev_word: Option<&'a str>,
    /// 塊の行が「この塊だけ」の行で、隣の行も同じ長さのそれなら真（折り返した base64 の塊）
    pub wrapped_block: bool,
    /// 秘密でない PEM（証明書・公開鍵）の塊の中なら真
    pub in_public_pem: bool,
    /// 塊の前の文脈（1 つ前の行の頭から塊の直前まで。ハッシュの見出しを探す）
    pub context: &'a str,
}

/// 除外表の 1 行（名前は応答に出さない。テストと docs が引く）
pub struct Exclusion {
    pub name: &'static str,
    pub matches: fn(&Candidate) -> bool,
}

/// 除外表。**行を足すだけで増やせる**（足したら `tests::exclusions_each_apply` へ例を足す）
pub const EXCLUSIONS: &[Exclusion] = &[
    // URL のパス・クエリ（`https://claude.ai/code/session_…` の ID 等）。
    // URL に埋め込んだ秘密は既知の形（webhook）と url_credentials / 代入の段が見る
    Exclusion {
        name: "url",
        matches: |c| c.word.contains("://"),
    },
    // base64 の画像・ファイル（`data:image/png;base64,…`）
    Exclusion {
        name: "data_uri",
        matches: |c| c.word.to_ascii_lowercase().contains("data:"),
    },
    // ファイルパス（絶対・ホーム起点・相対・Windows のドライブ）
    Exclusion {
        name: "file_path",
        matches: |c| {
            let w = c.word.trim_start_matches(['(', '[', '"', '\'', '`']);
            let b = w.as_bytes();
            c.run.starts_with('/')
                || w.starts_with("~/")
                || w.starts_with("./")
                || w.starts_with("../")
                || (b.len() >= 3
                    && b[0].is_ascii_alphabetic()
                    && b[1] == b':'
                    && matches!(b[2], b'\\' | b'/'))
        },
    },
    // UUID（8-4-4-4-12 の 16 進）
    Exclusion {
        name: "uuid",
        matches: |c| is_uuid(c.run),
    },
    // 16 進だけの塊（git の SHA 40 / sha256 64 / `0x…` の定数）
    Exclusion {
        name: "hex",
        matches: |c| {
            let r = c.run.strip_prefix("0x").unwrap_or(c.run);
            r.bytes().all(|b| b.is_ascii_hexdigit() || b == b'_')
        },
    },
    // npm / SRI の integrity（`sha512-…`）
    Exclusion {
        name: "integrity_hash",
        matches: |c| {
            ["sha1-", "sha256-", "sha384-", "sha512-"]
                .iter()
                .any(|p| c.word.starts_with(p) || c.run.starts_with(p))
        },
    },
    // SSH の公開鍵（`ssh-ed25519 AAAA… comment`）
    Exclusion {
        name: "ssh_public_key",
        matches: |c| {
            c.prev_word.is_some_and(|p| {
                p.starts_with("ssh-")
                    || p.starts_with("ecdsa-sha2-")
                    || p.starts_with("sk-ssh-")
                    || p.starts_with("sk-ecdsa-")
            })
        },
    },
    // 証明書・公開鍵の PEM の本体
    Exclusion {
        name: "public_pem",
        matches: |c| c.in_public_pem,
    },
    // 折り返した base64 の塊（`base64 file` の出力・MIME の本文）
    Exclusion {
        name: "wrapped_base64",
        matches: |c| c.wrapped_block,
    },
    // ハッシュ値（見出しが同じ行か 1 つ前の行にある: `integrity: "…"` / `sha512: …` /
    // `Sec-WebSocket-Accept: …` / `digest_of(…),\n "…"`）
    Exclusion {
        name: "digest_label",
        matches: |c| {
            let lower = c.context.to_ascii_lowercase();
            DIGEST_LABELS.iter().any(|l| lower.contains(l))
        },
    },
    // base64 の JSON の 1 片（`eyJ…`。JWT は 3 片そろったものを既知の形が採る）
    Exclusion {
        name: "base64_json",
        matches: |c| c.run.starts_with("eyJ"),
    },
    // 区切り（`-` `_` `/`）の多い名前（`tako-vX-windows-x86_64`）。ランダムな base64 の
    // 区切りは 3% 程度なので、3 つ以上かつ 12% 以上なら組み立てた名前とみなす
    Exclusion {
        name: "separated_name",
        matches: |c| {
            let seps = c
                .run
                .bytes()
                .filter(|b| matches!(b, b'-' | b'_' | b'/'))
                .count();
            seps >= 3 && seps * 100 >= c.run.len() * 12
        },
    },
    // base64 のファイルの頭（PNG / JPEG / GIF / PDF / zip）
    Exclusion {
        name: "base64_file",
        matches: |c| {
            ["iVBORw0KGgo", "/9j/", "R0lGOD", "JVBERi0", "UEsDB"]
                .iter()
                .any(|p| c.run.starts_with(p))
        },
    },
];

/// ハッシュ値の見出し（小文字で部分一致）
const DIGEST_LABELS: &[&str] = &[
    "sha1",
    "sha256",
    "sha384",
    "sha512",
    "integrity",
    "checksum",
    "digest",
    "hash",
    "sec-websocket",
    "etag",
    "fingerprint",
];

fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, &c)| match i {
            8 | 13 | 18 | 23 => c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

fn is_run_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'+' | b'/' | b'=')
}

/// camelCase で語に分けたとき、2 文字以下の語に属する英数字の割合
/// （記号で切る / 英字と数字の境で切る / 小文字→大文字で切る / `HTTPServer` は `HTTP` `Server`）
fn short_word_ratio(run: &[u8]) -> f64 {
    let mut total = 0usize;
    let mut short = 0usize;
    let mut cur = 0usize;
    let mut flush = |cur: &mut usize| {
        if *cur > 0 {
            total += *cur;
            if *cur <= 2 {
                short += *cur;
            }
            *cur = 0;
        }
    };
    for (i, &c) in run.iter().enumerate() {
        if !c.is_ascii_alphanumeric() {
            flush(&mut cur);
            continue;
        }
        if cur > 0 {
            let p = run[i - 1];
            let next_lower = run.get(i + 1).is_some_and(u8::is_ascii_lowercase);
            let boundary = p.is_ascii_digit() != c.is_ascii_digit()
                || (p.is_ascii_lowercase() && c.is_ascii_uppercase())
                || (p.is_ascii_uppercase() && c.is_ascii_uppercase() && next_lower);
            if boundary {
                flush(&mut cur);
            }
        }
        cur += 1;
    }
    flush(&mut cur);
    if total == 0 {
        0.0
    } else {
        short as f64 / total as f64
    }
}

/// シャノンエントロピー（ビット / 文字）
fn entropy(run: &[u8]) -> f64 {
    let mut counts = [0u32; 128];
    for &b in run {
        counts[usize::from(b & 0x7f)] += 1;
    }
    let n = run.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = f64::from(c) / n;
            -p * p.log2()
        })
        .sum()
}

/// 塊がランダムな鍵らしいか（第 3 段の本体）
fn looks_random(run: &[u8]) -> bool {
    has_upper_lower_digit(run)
        && short_word_ratio(run) >= SHORT_WORD_RATIO_MIN
        && entropy(run) >= ENTROPY_MIN
}

/// 秘密でない PEM（`-----BEGIN CERTIFICATE-----` 等）の本体の範囲
fn public_pem_ranges(body: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (start, _) in body.match_indices("-----BEGIN ") {
        let label_from = start + "-----BEGIN ".len();
        let Some(close) = body[label_from..].find("-----") else {
            continue;
        };
        // 切り出し安全: ASCII の `-----` を find した位置
        if body[label_from..label_from + close].contains("PRIVATE") {
            continue;
        }
        let end = body[label_from..]
            .find("-----END ")
            .map(|e| label_from + e)
            .unwrap_or(body.len());
        out.push((start, end));
    }
    out
}

/// 行（`\n` 区切り）が 1 つの塊だけでできているなら、その塊の長さ
fn sole_run_len(line: &str) -> Option<usize> {
    let t = line.trim_end_matches('\r');
    (t.len() >= 60 && t.bytes().all(is_run_byte)).then_some(t.len())
}

/// 第 3 段: 形の分からない塊
fn high_entropy(body: &str, spans: &mut Spans) {
    let pem = public_pem_ranges(body);
    let lines: Vec<(usize, &str)> = {
        let mut v = Vec::new();
        let mut at = 0;
        for line in body.split('\n') {
            v.push((at, line));
            at += line.len() + 1;
        }
        v
    };
    let mut prev_word: Option<&str> = None;
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() && !full(spans) {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let word_start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        // 切り出し安全: ASCII の空白の位置
        let word = &body[word_start..i];
        let wb = word.as_bytes();
        let mut j = 0;
        while j < wb.len() {
            if !is_run_byte(wb[j]) {
                j += 1;
                continue;
            }
            let run_start = j;
            while j < wb.len() && is_run_byte(wb[j]) {
                j += 1;
            }
            let n = j - run_start;
            if !(MIN_CHUNK_CHARS..=MAX_CHUNK_CHARS).contains(&n) {
                continue;
            }
            let run_bytes = &wb[run_start..j];
            if !looks_random(run_bytes) {
                continue;
            }
            let abs = word_start + run_start;
            // 切り出し安全: 塊は ASCII の連なり
            let run = &word[run_start..j];
            let wrapped_block = {
                let idx = lines
                    .partition_point(|(at, _)| *at <= abs)
                    .saturating_sub(1);
                let own = sole_run_len(lines[idx].1);
                own.is_some()
                    && [idx.checked_sub(1), Some(idx + 1)]
                        .into_iter()
                        .flatten()
                        .filter_map(|k| lines.get(k))
                        .any(|(_, l)| sole_run_len(l) == own)
            };
            let context = {
                let idx = lines
                    .partition_point(|(at, _)| *at <= abs)
                    .saturating_sub(1);
                let from = lines[idx.saturating_sub(1)].0;
                // 切り出し安全: 行頭（`\n` の直後）と塊の頭（ASCII）
                &body[from..abs]
            };
            let candidate = Candidate {
                run,
                word,
                prev_word,
                wrapped_block,
                in_public_pem: pem.iter().any(|&(s, e)| s <= abs && abs < e),
                context,
            };
            if EXCLUSIONS.iter().any(|x| (x.matches)(&candidate)) {
                continue;
            }
            push(
                spans,
                Span {
                    rule: Rule::HighEntropy,
                    start: abs,
                    end: abs + n,
                    masked: mask(run, 0),
                },
            );
        }
        prev_word = Some(word);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 偽の鍵の材料。**本物の鍵を書かない**（GitHub の secret scanning に当たらないよう、
    /// 前置詞と本体を実行時に連結し、本体は `EXAMPLE` を含む明らかな偽物にする）
    fn fake(prefix: &str, body: &str) -> String {
        format!("{prefix}{body}")
    }

    /// 決まった種から作る英数のランダム列（xorshift。テストを毎回同じにする）
    fn random_alnum(seed: &mut u64, len: usize) -> String {
        const ALNUM: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        (0..len)
            .map(|_| {
                *seed ^= *seed << 13;
                *seed ^= *seed >> 7;
                *seed ^= *seed << 17;
                ALNUM[(*seed % ALNUM.len() as u64) as usize] as char
            })
            .collect()
    }

    fn only(text: &str) -> Finding {
        let report = scan(text);
        assert_eq!(
            report.findings.len(),
            1,
            "1 件だけ当たるはず: {:?}",
            report.findings
        );
        report.findings.into_iter().next().unwrap()
    }

    /// 受け入れ条件 1: 既知の形 10 種以上がそれぞれ当たり、伏せ字が正しい
    #[test]
    fn known_shapes_each_hit() {
        let cases: Vec<(String, Rule, &str)> = vec![
            (
                fake("sk-ant-", "api03-EXAMPLEaaaaBBBBccccDDDDeeee1234ABCD"),
                Rule::AnthropicApiKey,
                "sk-ant-…ABCD",
            ),
            (
                fake("sk-", "proj-EXAMPLE1234abcdEFGHijklMNOP5678wxyz"),
                Rule::OpenAiApiKey,
                "sk-…wxyz",
            ),
            (
                fake("ghp_", "EXAMPLE0123456789abcdefghijklmnopqrst"),
                Rule::GithubToken,
                "ghp_…qrst",
            ),
            (
                fake("github_pat_", "11EXAMPLE0_abcdefghijklmnopqrstuvwxyz0123"),
                Rule::GithubPat,
                "github_pat_…0123",
            ),
            (
                fake("glpat-", "EXAMPLEabcdefghij1234"),
                Rule::GitlabToken,
                "glpat-…234",
            ),
            (
                fake("xoxb-", "1234567890-EXAMPLE-abcdefgh"),
                Rule::SlackToken,
                "xoxb-…efgh",
            ),
            (
                fake(
                    "https://hooks.slack.com/services/",
                    "T00000000/B00000000/EXAMPLEXXXXXXXXXXXXX",
                ),
                Rule::SlackWebhook,
                "https://hooks.slack.com/services/…XXXX",
            ),
            (
                fake(
                    "https://discord.com/api/webhooks/",
                    "000000000000000000/EXAMPLE-abcdefghij",
                ),
                Rule::DiscordWebhook,
                "https://discord.com/api/webhooks/…ghij",
            ),
            (
                fake("AKIA", "EXAMPLE000000000"),
                Rule::AwsAccessKey,
                "AKIA…00",
            ),
            (
                fake("AIza", "EXAMPLE-abcdefghijklmnopqrstuvwxyz0"),
                Rule::GoogleApiKey,
                "AIza…xyz0",
            ),
            (
                fake("sk_live_", "EXAMPLEabcdefghij0123"),
                Rule::StripeKey,
                "sk_live_…123",
            ),
            (
                fake("npm_", "EXAMPLEabcdefghijklmnopqrstuvwxy0123"),
                Rule::NpmToken,
                "npm_…0123",
            ),
            (
                fake("hf_", "EXAMPLEabcdefghijklmnopqrstuvwxyz"),
                Rule::HuggingfaceToken,
                "hf_…wxyz",
            ),
            (
                fake(
                    "SG.",
                    "EXAMPLEabcdefghijklmno.EXAMPLEabcdefghijklmnopqrstuvwxyz0123456789a",
                ),
                Rule::SendgridKey,
                "SG.…789a",
            ),
            (
                format!(
                    "{}\nMIIEXAMPLE\n-----END RSA PRIVATE KEY-----",
                    fake("-----BEGIN RSA ", "PRIVATE KEY-----")
                ),
                Rule::PrivateKey,
                concat!("-----BEGIN RSA ", "PRIVATE KEY-----"),
            ),
            (
                fake(
                    "eyJ",
                    "hbGciOiJIUzI1NiJ9.eyJzdWIiOiJFWEFNUExFIn0.EXAMPLEsignature0",
                ),
                Rule::Jwt,
                "eyJ…ure0",
            ),
            (
                fake("postgres://admin:", "Zq9pass99x@db.local:5432/app"),
                Rule::UrlCredentials,
                "Z…x",
            ),
            (
                fake("DB_PASSWORD=", "Pa55-w0rd-Zq9"),
                Rule::Assignment,
                "Pa…q9",
            ),
        ];
        let rules: std::collections::HashSet<Rule> = cases.iter().map(|c| c.1).collect();
        assert!(rules.len() >= 10, "既知の形が 10 種以上ある");
        for (text, rule, masked) in cases {
            let f = only(&text);
            assert_eq!(f.rule, rule, "{text}");
            assert_eq!(f.masked, masked, "{text}");
            // 伏せ字は生の値の全体を含まない（秘密鍵は見出しだけ = 本体を含まない）
            assert!(f.masked.contains('…') || rule == Rule::PrivateKey, "{text}");
            assert!(!f.masked.contains("MIIEXAMPLE"), "{text}");
        }
    }

    /// 文脈の中でも当たる（引用符・代入・日本語の文の中・前後の記号）
    #[test]
    fn known_shapes_hit_inside_context() {
        let key = fake("sk-ant-", "api03-EXAMPLEaaaaBBBBccccDDDDeeee1234ABCD");
        for text in [
            format!("export ANTHROPIC_API_KEY=\"{key}\""),
            format!("この鍵を使って: {key} でお願い"),
            format!("({key})"),
            format!("{{\"apiKey\": \"{key}\"}}"),
        ] {
            let f = only(&text);
            assert_eq!(f.rule, Rule::AnthropicApiKey, "{text}");
        }
        // 複数の鍵が混在 → それぞれ別の規則で当たる
        let mixed = format!(
            "{key}\n{}\n{}",
            fake("ghp_", "EXAMPLE0123456789abcdefghijklmnopqrst"),
            fake("AKIA", "EXAMPLE000000000")
        );
        let report = scan(&mixed);
        assert_eq!(
            report.rules(),
            vec![Rule::AnthropicApiKey, Rule::GithubToken, Rule::AwsAccessKey]
        );
        assert_eq!(
            report.findings.iter().map(|f| f.line).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    /// 前置詞が語の途中・本体が短い・本体の後ろが続く形は当たらない
    #[test]
    fn known_shapes_respect_boundaries() {
        for text in [
            // `task-…` の `sk-`
            format!("ta{}", fake("sk-", "EXAMPLE1234abcdEFGHijklMNOP5678wxyz")),
            // `sk-` の後ろが kebab-case の名前（大文字・数字が無い）
            "sk-learn-is-a-python-library-for-machine-learning".to_string(),
            // 本体が短い
            "ghp_short".to_string(),
            // AKIA の後ろに英字が続く（もっと長い語の一部）
            format!("{}xyz", fake("AKIA", "EXAMPLE000000000")),
        ] {
            assert!(!scan(&text).hit(), "{text}: {:?}", scan(&text).findings);
        }
    }

    /// 受け入れ条件 3: 高エントロピーの未知の形（ランダム 32 文字）が当たる
    #[test]
    fn random_32_chars_hit() {
        let mut seed = 0x2046_u64;
        let mut hits = 0;
        const N: usize = 1000;
        for _ in 0..N {
            let s = random_alnum(&mut seed, 32);
            if scan(&format!("token は {s} です")).hit() {
                hits += 1;
            }
        }
        // 閾値の見積もり（見逃し約 2%）より余裕を持って 95% 以上
        assert!(hits * 100 >= N * 95, "ランダム 32 文字の検出率 {hits}/{N}");
        // 決まった 1 本は必ず当たり、伏せ字は先頭 3 + 末尾 4
        let f = only("aZ3kQ9mWx7Lp2Rt8Vb4Nc6Yd1Hf5Js0G");
        assert_eq!(f.rule, Rule::HighEntropy);
        assert_eq!(f.masked, "aZ3…Js0G");
    }

    /// 受け入れ条件 2: 誤検知の検証コーパスで当たり 0
    #[test]
    fn corpus_has_no_false_positives() {
        let corpus: &[(&str, &str)] = &[
            (
                "日本語の長文",
                "tako は AI エージェントの集約監視に特化した GUI ターミナルです。\
                 貼り付けた文字に API キーやパスワードらしきものがあれば、送る前に確かめます。\
                 普通の長文は空白や句読点で区切られるので、塊として判定されません。\
                 設定画面の「一般」タブからオン・オフを切り替えられます（既定はオン）。",
            ),
            (
                "英語の長文",
                "The quick brown fox jumps over the lazy dog. Internationalization and \
                 localization (i18n and l10n) are means of adapting computer software to \
                 different languages, regional peculiarities and technical requirements. \
                 Counterrevolutionaries notwithstanding, electroencephalographically speaking.",
            ),
            (
                "ソースコード（Rust）",
                "pub struct Settings {\n    pub token: Option<String>,\n    pub password: String,\n}\n\
                 fn main() {\n    let token = compute_token(&config);\n    let api_key = std::env::var(\"API_KEY\")?;\n\
                 if password == other_password { return; }\n    let max_tokens: u32 = 4096;\n\
                 let secret_name = format!(\"{}-{}\", prefix, id);\n    HashMap::<String, Vec<u8>>::new();\n}\n",
            ),
            (
                "ソースコード（TS / Python / YAML）",
                "const token = await getToken();\nfunction login(password: string): Promise<void> {}\n\
                 api_key = os.getenv(\"API_KEY\")\npassword = input(\"Password: \")\n\
                 db:\n  password: ${DB_PASSWORD}\n  token: \"{{ .Values.token }}\"\n  secret: !vault |\n\
                 headers = { Authorization: `Bearer ${token}` }\nVERSION_TOKEN=v2.10.3\n\
                 const ThisIsOneVeryLongUnbreakableIdentifierToken = hasNewPixelBufferForItemTime;\n\
                 CVPixelBufferGetBytesPerRow(buffer); CMTimeMakeWithSeconds(seconds, scale);\n",
            ),
            (
                "git log",
                "commit 7471605c3e0a5a1f2b9d8e7f6a5b4c3d2e1f0a9b\nAuthor: Test User <testuser@example.com>\n\
                 Date:   Mon Oct 13 22:00:00 2026 +0900\n\n    [修正] pane を省いた tako_show_command (#1958) (#1986)\n\n\
                 Merge: 23f9917 dd8ec32\n 4 files changed, 120 insertions(+), 8 deletions(-)\n\
                 crates/tako-control/src/mcp/request.rs | 12 ++++++------\n",
            ),
            (
                "UUID / SHA",
                "session 95b181d1-274e-4bf7-8b04-13f2d4a6bea1 E88E3509-9E5F-4CD8-B5E8-54D1E118394B\n\
                 sha256: 2cfbd8fc8eb8f8232cc81eb338462abaf78081b4fda7a9fed91a6bf5b2f1ace0\n\
                 f02c3990f5aaff78cb586e4ed423760394f2c5aa 0xC2B2_AE3D_27D4_EB4F\n",
            ),
            (
                "URL",
                "https://claude.ai/code/session_01Y9N8RvVQ3J4RRg6qR7MWUb を開く\n\
                 https://github.com/takushio2525/tako/pull/1986/files#diff-a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6\n\
                 http://localhost:3000/api/v1/items?page=2&sort=desc\n",
            ),
            (
                "パス",
                "/Users/testuser/dev/tako/target/debug/deps/tako_control-16d08f68b5f56d87\n\
                 ~/Library/Caches/com.example.App9/aB3dE5fG7hJ9kL1mN3pQ5rS7tU9vW1xY\n\
                 ./node_modules/.pnpm/react-dom@18.3.1_react@18.3.1/node_modules/react-dom\n\
                 C:\\Users\\winuser\\AppData\\Local\\Temp\\aB3dE5fG7hJ9kL1mN3pQ5rS7tU9vW1xY\n",
            ),
            (
                "base64 の画像（data: URI）",
                "<img src=\"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==\">",
            ),
            (
                "SSH の公開鍵・integrity・証明書",
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEXAMPLEaB3dE5fG7hJ9kL1mN3pQ5rS7tU9vW1 testuser@host\n\
                 \"integrity\": \"sha512-r+pAcYtWdN8tKlYZPwiiHNA2QPjXnI02NrW5Sf2cVM3TRtuQ3V9EKKwOxqwaQ0krsaEXk/CbN90I5erBuf84Vg==\"\n\
                 -----BEGIN CERTIFICATE-----\nMIIDdzCCAl+gAwIBAgIEAgAAuTANBgkqhkiG9w0BAQUFADBaMQswCQYDVQQGEwJJ\n\
                 RTESMBAGA1UEChMJQmFsdGltb3JlMRMwEQYDVQQLEwpDeWJlclRydXN0MSIwIAYD\n-----END CERTIFICATE-----\n",
            ),
            (
                "空白・改行だけ・空",
                "   \n\n\t\n",
            ),
            (
                "実データで見つけた誤検知の型（ハッシュ・区切りの多い名前・base64 の JSON）",
                "integrity: \"jl1vZzPDinLr9eUt3J/t7V6FgNEw9QjvBPdysz9KfQDD41fQrC2Y4vKQdiaUpFT4bXlb1RHhLpp8wtm6M5TgSw==\",\n\
                 Sec-Websocket-Accept: fqOx8WbQ2Lr7Kd9TzPmN4sJdrI=\n\
                 Windows アセット（`tako-vX-windows-x86_64.exe`）\n\
                 digest_of(b\"abc\", true),\n    \"3a81oZNherrMQXNJriBBMRLm+k6JqX6iCp7u5ktV05ohkpkqJ0/BqDa6PCOj/uu9RU1EI2Q86A4qmslPpUyknw==\"\n\
                 let payload = \"eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9wbGFuX3R5cGUiOiJwbHVzIn19\";\n",
            ),
            (
                "docker compose（値が単語）",
                "environment:\n  - POSTGRES_PASSWORD=postgres\n  POSTGRES_USER: example\n  token_type: Bearer\n",
            ),
        ];
        for (name, text) in corpus {
            let report = scan(text);
            assert!(
                !report.hit(),
                "{name}: 誤検知 {:?}",
                report
                    .findings
                    .iter()
                    .map(|f| (f.rule, f.masked.clone()))
                    .collect::<Vec<_>>()
            );
        }
        assert!(!scan("").hit());
    }

    /// 除外表の各行が実際に効いている（行を外すと当たる塊を、その行が外している）
    #[test]
    fn exclusions_each_apply() {
        let random = "aZ3kQ9mWx7Lp2Rt8Vb4Nc6Yd1Hf5Js0G";
        assert!(scan(random).hit(), "前提: 素の塊は当たる");
        let cases: &[(&str, String)] = &[
            ("url", format!("https://example.com/{random}")),
            (
                "data_uri",
                format!("data:application/octet-stream;base64,{random}"),
            ),
            ("file_path", format!("/tmp/{random}")),
            // `~/` で始まるパスの入力（ホームの短縮ではない。home_dir_watchdog の字面一致を避けて組む）
            ("file_path", ["~/", random].concat()),
            ("file_path", format!("C:\\{random}")),
            ("integrity_hash", format!("sha512-{random}")),
            ("ssh_public_key", format!("ssh-rsa {random} testuser@host")),
            (
                "public_pem",
                format!("-----BEGIN PUBLIC KEY-----\n{random}\n-----END PUBLIC KEY-----"),
            ),
            ("base64_file", format!("iVBORw0KGgo{random}")),
        ];
        for (name, text) in cases {
            assert!(
                EXCLUSIONS.iter().any(|x| x.name == *name),
                "除外表に {name} がある"
            );
            assert!(!scan(text).hit(), "{name}: {text}");
        }
        // 折り返した base64（同じ長さの塊だけの行が続く）
        let mut seed = 7u64;
        let block: Vec<String> = (0..3).map(|_| random_alnum(&mut seed, 76)).collect();
        assert!(!scan(&block.join("\n")).hit(), "wrapped_base64");
        // 1 行だけなら当たる（折り返しの塊ではない）
        assert!(scan(&block[0]).hit());
        // UUID と 16 進は 3 字種の条件でも外れるが、除外表にも行がある（明示の宣言）
        assert!(is_uuid("95b181d1-274e-4bf7-8b04-13f2d4a6bea1"));
        assert!(EXCLUSIONS.iter().any(|x| x.name == "uuid"));
        assert!(EXCLUSIONS.iter().any(|x| x.name == "hex"));
    }

    /// 代入の段: 値を書いている形だけを拾う
    #[test]
    fn assignments_pick_literals_only() {
        let hits = [
            "password: \"s3cr3t-Zq9x\"",
            "export GITHUB_TOKEN=Zq9_x42_abc",
            "mysql -u root --password=Zq9pass99",
            "curl 'https://api.example.com/v1?api_key=Zq9123abc&x=1'",
            "client_secret := \"Zq9-0001x\"",
            "'apiKey' => 'Zq9key7x'",
            // INI / YAML（名前が行頭）
            "[client]\npassword = hunter22\n",
            "db:\n  password: Zq9!pass\n",
        ];
        for text in hits {
            let r = scan(text);
            assert!(
                r.findings.iter().any(|f| f.rule == Rule::Assignment),
                "{text}: {:?}",
                r.findings
            );
        }
        let misses = [
            "password = getPassword()",
            "token: Token,",
            "password: hunter",
            "PWD=/Users/testuser/dev",
            "max_tokens: 4096",
            "token = \"\"",
            "secret: ********",
            "password: your_password_here",
            "token == expected_token",
            "auth::token::refresh()",
            "description: \"the secret of a good token\"",
            // 実データ（このリポジトリのソース・docs・git log）で見つけた誤検知の型
            "let token = token0 as *const c_void;",
            "token: \"stale-token\".into(),",
            "admin_token: \"test-admin-token\".to_string(),",
            "{\"tokens\":{\"id_token\":\"header.payload.signature\"}}",
            "既定マスク時は URL 内の\n    token= も伏せ字にする",
            "let excluded_secret = catalog::CATALOG",
            "let token = \"動画は確認して。\";",
            "token: \"tok-abc\".into(),",
            "format!(r#\"{{\"id_token\":\"header.{payload}.signature\"}}\"#)",
            "password: \"my-secret-pass\"",
            "struct Client {\n    socket: String,\n    token: String,\n    admin_token: SecretString,\n}",
        ];
        for text in misses {
            assert!(!scan(text).hit(), "{text}: {:?}", scan(text).findings);
        }
    }

    /// 伏せ字は本体の 1/3 未満しか見せず、短い値ほど見せない
    #[test]
    fn mask_reveals_little() {
        assert_eq!(mask("abcde", 0), "…");
        assert_eq!(mask("hunter22", 0), "h…2");
        assert_eq!(mask("aZ3kQ9mWx7Lp2Rt8Vb4Nc6Yd1Hf5Js0G", 0), "aZ3…Js0G");
        assert_eq!(
            mask(&fake("ghp_", "EXAMPLE0123456789abcdefghijklmnopqrst"), 4),
            "ghp_…qrst"
        );
        // 非 ASCII の値でも文字の途中で切らない
        assert_eq!(mask("パスワード12345678", 0), "パス…78");
        for value in [
            "abcdefgh",
            "abcdefghijklmnop",
            "0123456789abcdefghijklmnopqrstuvwxyz",
        ] {
            let m = mask(value, 0);
            let shown = m.chars().count() - 1;
            assert!(shown * 3 < value.chars().count(), "{value} → {m}");
        }
    }

    /// 受け入れ条件 6 の単体側: 診断ログの行と `Debug` に本文・検出値が出ない
    #[test]
    fn log_line_and_debug_never_contain_the_secret() {
        let key = fake("sk-ant-", "api03-EXAMPLEaaaaBBBBccccDDDDeeee1234ABCD");
        let pw = "Pa55-w0rd-Zq9";
        let text = format!("token は {key} と DB_PASSWORD={pw}");
        let report = scan(&text);
        assert!(report.hit());
        let line = log_line("prompted", 3, &report);
        assert!(line.contains("anthropic_api_key"), "{line}");
        assert!(line.contains("assignment"), "{line}");
        let debug = format!("{report:?}");
        for secret in [key.as_str(), pw] {
            // 秘密の 8 文字以上の部分列がログ・Debug のどちらにも出ない
            let chars: Vec<char> = secret.chars().collect();
            for w in chars.windows(8) {
                let piece: String = w.iter().collect();
                // 前置詞（`sk-ant-a` 等の形が決まった部分）は秘密ではない
                if key.starts_with(&piece) && piece.starts_with("sk-ant-") {
                    continue;
                }
                assert!(!line.contains(&piece), "ログに {piece}: {line}");
                assert!(!debug.contains(&piece), "Debug に {piece}");
            }
        }
        // 伏せ字もログには出さない
        for f in &report.findings {
            assert!(!line.contains(&f.masked), "{line}");
        }
    }

    /// 巨大な貼り付けは先頭だけを見る（後ろの鍵は判定しない = 打ち切りを申告する）
    #[test]
    fn huge_paste_is_truncated_at_the_limit() {
        let filler = "lorem ipsum dolor sit amet ".repeat(40_000);
        let key = fake("ghp_", "EXAMPLE0123456789abcdefghijklmnopqrst");
        let head = scan(&format!("{key} {filler}"));
        assert!(head.hit());
        assert!(head.truncated());
        assert_eq!(head.scanned_bytes, MAX_SCAN_BYTES);
        let tail = scan(&format!("{filler} {key}"));
        assert!(!tail.hit(), "打ち切りより後ろは見ない");
        // 多バイト文字の途中で切らない
        let ja = "あ".repeat(MAX_SCAN_BYTES);
        let r = scan(&ja);
        assert!(r.scanned_bytes <= MAX_SCAN_BYTES && ja.is_char_boundary(r.scanned_bytes));
    }

    /// 塊だらけの貼り付けでも件数は上限で打ち切る（所要を件数の 2 乗にしない）
    #[test]
    fn findings_are_capped() {
        let dense = "password=abc123; ".repeat(2000);
        let r = scan(&dense);
        assert_eq!(r.findings.len(), MAX_FINDINGS);
        assert!(r.findings.iter().all(|f| f.rule == Rule::Assignment));
        let mut seed = 11u64;
        let noisy: Vec<String> = (0..1000).map(|_| random_alnum(&mut seed, 24)).collect();
        assert_eq!(scan(&noisy.join(" ")).findings.len(), MAX_FINDINGS);
    }

    #[test]
    fn rule_names_are_unique_snake_case() {
        let mut seen = std::collections::HashSet::new();
        for r in Rule::ALL {
            let s = r.as_str();
            assert!(seen.insert(s), "{s} が重複");
            assert!(
                s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
                "{s}"
            );
        }
        assert_eq!(Answer::parse("send"), Some(Answer::Send));
        assert_eq!(Answer::parse("cancel"), Some(Answer::Cancel));
        assert_eq!(Answer::parse("ok"), None);
        assert!(PasteGuardStatus::default().enabled);
    }

    // ---- #2050: 許可リスト・規則の無効化・段・確認済みの値 ----

    fn anthropic() -> String {
        fake("sk-ant-", "api03-EXAMPLEaaaaBBBBccccDDDDeeee1234ABCD")
    }

    fn github() -> String {
        fake("ghp_", "EXAMPLE0123456789abcdefghijklmnopqrst")
    }

    /// 受け入れ条件 2: 許可リストに載せた値は当たらない（ほかの値は今までどおり当たる）
    #[test]
    fn allowlisted_values_do_not_hit() {
        let public = fake("AIza", "EXAMPLE-abcdefghijklmnopqrstuvwxyz0");
        let mut policy = Policy::default();
        let added = policy.allow_text(&public).unwrap();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].rule, "google_api_key");
        assert_eq!(added[0].fingerprint.len(), 64);
        assert!(
            policy.allow_text(&public).unwrap().is_empty(),
            "同じ値は 2 回足さない"
        );
        assert_eq!(policy.allow.len(), 1);
        // 単独・文の中・代入の右辺のどれでも当たらない（範囲が同じなら指紋が同じ）
        for text in [
            public.clone(),
            format!("Firebase の公開キーは {public} です"),
            format!("GOOGLE_API_KEY={public}"),
        ] {
            let r = scan_with(&text, &policy);
            assert!(!r.hit(), "{:?}", r.findings);
            assert_eq!(r.allowed, 1);
            assert!(scan(&text).hit(), "許可リストを当てない判定では当たる");
        }
    }

    /// 境界: 許可リストの値と秘密が 1 回の貼り付けに混在 → 秘密だけが当たる
    #[test]
    fn allowlisted_value_mixed_with_a_secret() {
        let public = fake("AIza", "EXAMPLE-abcdefghijklmnopqrstuvwxyz0");
        let mut policy = Policy::default();
        policy.allow_text(&public).unwrap();
        let text = format!("public={public}\nsecret {}\n{public}", anthropic());
        let r = scan_with(&text, &policy);
        assert_eq!(r.rules(), vec![Rule::AnthropicApiKey]);
        assert_eq!(r.findings[0].line, 2);
        assert_eq!(r.allowed, 2);
        // 混ぜた貼り付けを丸ごと許可へ足すと、まだ許可していない方（秘密）だけが足される
        let mut p2 = policy.clone();
        let added = p2.allow_text(&text).unwrap();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].rule, "anthropic_api_key");
        assert!(!scan_with(&text, &p2).hit());
    }

    /// 受け入れ条件 2: 規則を無効にすると当たらない。後ろの段が同じ文字を拾い直さない
    #[test]
    fn disabled_rules_do_not_hit() {
        let key = fake("sk-", "proj-EXAMPLE1234abcdEFGHijklMNOP5678wxyz");
        let mut policy = Policy::default();
        policy.set_rule("openai_api_key", false).unwrap();
        let r = scan_with(&key, &policy);
        assert!(!r.hit(), "{:?}", r.findings);
        assert_eq!(r.disabled, 1);
        // 範囲を取らずに報告だけ落とすと、無効にした鍵を後ろの段が拾い直す。ランダムな本体の鍵
        // （実行時に作る）で確かめる: 前置詞を外した同じ本体は high_entropy が拾うが、無効にした
        // openai_api_key の鍵としては拾い直さない
        let mut seed = 2050u64;
        let random = fake("sk-", &random_alnum(&mut seed, 40));
        assert_eq!(scan(&random).rules(), vec![Rule::OpenAiApiKey]);
        assert_eq!(scan(&random["sk-".len()..]).rules(), vec![Rule::HighEntropy]);
        let r = scan_with(&random, &policy);
        assert!(!r.hit(), "拾い直した: {:?}", r.findings);
        assert_eq!(r.disabled, 1);
        // 代入の右辺に置いても同じ（既知の形の段が先に範囲を取る）
        assert!(!scan_with(&format!("OPENAI_API_KEY={key}"), &policy).hit());
        // ほかの規則は今までどおり
        assert_eq!(
            scan_with(&format!("{key} {}", github()), &policy).rules(),
            vec![Rule::GithubToken]
        );
        // 戻すと当たる
        policy.set_rule("openai_api_key", true).unwrap();
        assert!(policy.disabled_rules.is_empty());
        assert!(scan_with(&key, &policy).hit());
        // 知らない名前は断る（綴りを一覧で返す）
        let err = policy.set_rule("openai", false).unwrap_err();
        assert!(err.contains("openai_api_key"), "{err}");
    }

    /// 境界: 規則を全部無効にした状態では何も当たらない（件数の上限にも掛からない）
    #[test]
    fn all_rules_disabled_hits_nothing() {
        let mut policy = Policy::default();
        for r in Rule::ALL {
            policy.set_rule(r.as_str(), false).unwrap();
        }
        assert_eq!(policy.disabled_rules.len(), Rule::ALL.len());
        let jwt = fake(
            "eyJhbGciOiJIUzI1NiJ9.",
            "eyJzdWIiOiJFWEFNUExFIn0.EXAMPLEsignatureAbCdEf0123456789",
        );
        let text = format!(
            "{}\n{}\npassword=Zq9-EXAMPLE-pw\n{jwt}\n{}",
            anthropic(),
            github(),
            "password=abc123; ".repeat(500)
        );
        assert!(scan(&text).hit());
        let r = scan_with(&text, &policy);
        assert!(!r.hit(), "{:?}", r.rules());
        assert!(r.disabled > 0);
        // 並びは ALL の順で安定（表示・比較用）
        let names: Vec<&str> = Rule::ALL.iter().map(|r| r.as_str()).collect();
        assert_eq!(policy.disabled_rules, names);
    }

    /// 受け入れ条件 2: 「警告だけ」の段では止めずに警告。AI の送信は確認の段でも警告だけ
    #[test]
    fn verdict_by_mode_and_route() {
        assert_eq!(Verdict::of(false, Mode::Confirm, true), Verdict::Pass);
        assert_eq!(Verdict::of(true, Mode::Confirm, true), Verdict::Confirm);
        assert_eq!(Verdict::of(true, Mode::Warn, true), Verdict::Warn);
        assert_eq!(Verdict::of(true, Mode::Confirm, false), Verdict::Warn);
        assert_eq!(Verdict::of(true, Mode::Warn, false), Verdict::Warn);
        assert_eq!(Mode::parse("warn"), Some(Mode::Warn));
        assert_eq!(Mode::parse("confirm"), Some(Mode::Confirm));
        assert_eq!(Mode::parse("off"), None);
        for v in Mode::VALUES {
            assert_eq!(Mode::parse(v).unwrap().as_str(), v);
        }
    }

    /// 設定の節は旧ファイル（節が無い・キーが欠ける）でも既定で読め、既定は書き出さない（#916）
    #[test]
    fn policy_reads_old_files_and_round_trips() {
        let old: Policy = serde_json::from_str("{}").unwrap();
        assert!(old.is_default());
        assert_eq!(serde_json::to_string(&Policy::default()).unwrap(), "{}");
        let mut p = Policy {
            mode: Mode::Warn,
            ..Policy::default()
        };
        p.allow_text(&github()).unwrap();
        p.set_rule("high_entropy", false).unwrap();
        // 新しい版が足した知らない規則の名前は消さずに残す
        p.disabled_rules.push("future_rule".into());
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"mode\":\"warn\""), "{json}");
        let back: Policy = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
        // 設定ファイルにも Debug にも生の値を書かない（指紋と伏せ字だけ）
        let body = &github()["ghp_".len()..];
        assert!(!json.contains(body), "{json}");
        assert!(!format!("{p:?}").contains(body));
    }

    #[test]
    fn unallow_by_id_prefix_or_all() {
        let mut p = Policy::default();
        p.allow_text(&github()).unwrap();
        p.allow_text(&anthropic()).unwrap();
        assert_eq!(p.allow.len(), 2);
        let id = p.allow[0].id().to_string();
        assert_eq!(id.len(), 12);
        assert!(p.unallow("xyz!").is_err());
        assert!(p.unallow("ab").is_err(), "短すぎる ID は断る");
        assert!(
            p.unallow("ffffffffffff").is_err(),
            "当たる行が無い ID は断る"
        );
        assert_eq!(p.unallow(&id.to_ascii_uppercase()).unwrap(), 1);
        assert_eq!(p.allow.len(), 1);
        assert!(scan_with(&github(), &p).hit(), "外した値はまた当たる");
        assert_eq!(p.unallow("all").unwrap(), 1);
        assert!(p.allow.is_empty());
    }

    #[test]
    fn allowlist_is_capped() {
        let mut p = Policy::default();
        let mut seed = 5u64;
        for _ in 0..MAX_ALLOW {
            p.allow.push(AllowEntry {
                fingerprint: fingerprint(&random_alnum(&mut seed, 30)),
                rule: "high_entropy".into(),
                masked: "…".into(),
            });
        }
        let err = p.allow_text(&github()).unwrap_err();
        assert!(err.contains(&MAX_ALLOW.to_string()), "{err}");
        assert_eq!(p.allow.len(), MAX_ALLOW);
    }

    /// 確認済みの値（tako が流した・人が確かめた）はペインごと・期限つきで覚える
    #[test]
    fn acks_cover_per_pane_until_ttl() {
        let t0 = Instant::now();
        let mut acks = Acks::default();
        let text = format!("worker への指示: {} を使う", github());
        acks.note_text(7, &text, t0);
        acks.note_text(7, "普通の文章だけ", t0);
        assert_eq!(acks.len(t0), 1);
        let fp = fingerprint(&github());
        assert!(acks.covers(7, &fp, t0));
        assert!(!acks.covers(8, &fp, t0), "別のペインは確認済みでない");
        let later = t0 + ACK_TTL;
        assert!(!acks.covers(7, &fp, later), "期限を過ぎたら忘れる");
        assert!(acks.is_empty(later));
        // 規則の無効化・許可リストに関係なく、当たる値をすべて覚える
        let mut many = Acks::default();
        for i in 0..(MAX_ACKS + 10) {
            many.note(1, vec![format!("{i:064x}")], t0);
        }
        assert_eq!(many.len(t0), MAX_ACKS);
        assert!(
            !many.covers(1, &format!("{:064x}", 0), t0),
            "古いものから捨てる"
        );
    }

    /// 指紋は区切りの語つき（ただの SHA-256 と一致しない）・生の値を含まない
    #[test]
    fn fingerprints_follow_findings() {
        let text = format!("a {} b {}", github(), anthropic());
        let r = scan(&text);
        let fps = fingerprints(&text, &r);
        assert_eq!(fps.len(), 2);
        assert_eq!(fps[0], fingerprint(&github()));
        assert_ne!(fps[0], fps[1]);
        assert!(fps
            .iter()
            .all(|f| f.len() == 64 && f.bytes().all(|b| b.is_ascii_hexdigit())));
        assert!(!fps[0].contains("EXAMPLE"));
    }
}
