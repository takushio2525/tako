//! agy_quota — agy（Antigravity CLI）の `/usage` 画面から利用枠を読む（ステータスバーの agy 表示）
//!
//! agy 1.5 系は claude と同じく「5 時間枠」と「週枠」を持ち、残りの割合を対話中の `/usage`
//! （見出し `Models & Quota`）に出す。枠はモデルの系統ごとに 2 組ある:
//!
//! - `GEMINI MODELS`: Gemini 系のモデル
//! - `CLAUDE AND GPT MODELS`: 他社モデル（Claude / GPT）
//!
//! 実測（agy 1.5.18・2026-11-27）の画面:
//!
//! ```text
//! GEMINI MODELS
//!   Weekly Limit Remaining
//!     [██████████████████████████████████████████████░░░░] 91.11%
//!     Refreshes in 6d 11h 41m
//!   Five Hour Limit Remaining
//!     [█████████████████████████████████████████░░░░░░░░░] 81.92%
//!     Refreshes in 2h 24m
//! CLAUDE AND GPT MODELS
//!   Weekly Limit Remaining
//!     [██████████████████████████████████████████████████] 100.00%
//!     Quota available
//! ```
//!
//! agy は残量を**ディスクにもログにも書かない**（`~/.gemini/antigravity-cli/` に値が無く、
//! ログは `doRefreshQuota` の開始行だけ）。statusLine のコマンドには JSON の `quota` で渡るが、
//! それはユーザーの statusLine 設定に依存する。なので tako は**画面に `/usage` が出たときに読み、
//! リセットまで覚えておく**形を取る（[`AgyLimitsMemory`]）。
//!
//! このモジュールは純関数と素のデータだけを持つ（GUI 非依存）。

use std::time::{Duration, Instant};

/// 枠の 1 つ（5 時間枠 / 週枠）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgyQuotaWindow {
    /// 使用率（0–100）。画面の「残り」を `100 - 残り` に直して四捨五入した値
    /// （claude の 5h / 7d と同じ向きにそろえる）
    pub used_percent: u32,
    /// リセットまでの時間（`Refreshes in …`）。`Quota available`（未使用）や読めないときは None
    pub resets_in: Option<Duration>,
}

/// モデルの系統 1 組ぶんの枠
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AgyQuotaGroup {
    pub five_hour: Option<AgyQuotaWindow>,
    pub weekly: Option<AgyQuotaWindow>,
}

impl AgyQuotaGroup {
    fn is_empty(&self) -> bool {
        self.five_hour.is_none() && self.weekly.is_none()
    }
}

/// `/usage` の系統（どちらの枠を表示するか）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgyModelGroup {
    /// Gemini 系（agy の既定）
    #[default]
    Gemini,
    /// Claude / GPT（他社モデル）
    ThirdParty,
}

impl AgyModelGroup {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gemini => "gemini",
            Self::ThirdParty => "third_party",
        }
    }
}

/// `/usage` 画面 1 枚から読めたもの
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AgyUsage {
    pub gemini: AgyQuotaGroup,
    pub third_party: AgyQuotaGroup,
    /// 画面のフッターに出ているモデルから決めた「いま使っている系統」
    pub active: AgyModelGroup,
}

impl AgyUsage {
    /// いま使っている系統の枠
    pub fn active_group(&self) -> AgyQuotaGroup {
        match self.active {
            AgyModelGroup::Gemini => self.gemini,
            AgyModelGroup::ThirdParty => self.third_party,
        }
    }
}

/// `/usage` モーダルの見出し
const MODAL_TITLE: &str = "Models & Quota";
/// モーダルの操作ヒント（`↑/↓ Scroll · … · esc Close`）の末尾
const MODAL_HINT: &str = "esc Close";
/// 操作ヒントより下に置ける中身のある行の上限。agy のフッター 1 行 + statusLine
/// （実測した 4 行の構成）に余裕を足した値
const MAX_LINES_BELOW_HINT: usize = 8;
const FIVE_HOUR_HEADER: &str = "Five Hour Limit Remaining";
const WEEKLY_HEADER: &str = "Weekly Limit Remaining";
/// 枠の見出しから % の行までの距離の上限（実測は直下の 1 行目）
const VALUE_LOOKAHEAD: usize = 3;

/// 画面の行から agy の `/usage` を読む。モーダルが出ていなければ None。
///
/// 文言だけで判定すると、**別のペインにこの画面が写っただけ**（claude の会話に `/usage` の
/// キャプチャを貼った・ツールの出力に出た）でも agy と読んでしまう。agy の `/usage` は
/// 入力欄の下に開き、画面の**最下部**を占めるモーダルなので、次をすべて満たすときだけ読む:
///
/// - 見出し（`Models & Quota`）の下に操作ヒント（`esc Close`）がある
/// - ヒントより下の中身のある行が [`MAX_LINES_BELOW_HINT`] 行以内（= ヒントが画面の末尾にある）
/// - 見出しより下に claude の入力欄（`❯`）が無い（claude の会話に写った画面は入力欄の上にある）
pub fn parse_usage_screen(lines: &[String]) -> Option<AgyUsage> {
    let title = lines.iter().rposition(|l| l.contains(MODAL_TITLE))?;
    let hint = title
        + lines[title..]
            .iter()
            .rposition(|l| l.contains(MODAL_HINT))?;
    let below_hint = lines[hint + 1..]
        .iter()
        .filter(|l| !l.trim().is_empty())
        .count();
    if below_hint > MAX_LINES_BELOW_HINT {
        return None;
    }
    if lines[title..]
        .iter()
        .any(|l| l.trim_start().starts_with('❯'))
    {
        return None;
    }
    let mut usage = AgyUsage::default();
    let mut group: Option<AgyModelGroup> = None;
    let modal = &lines[title..hint];
    for (i, line) in modal.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(g) = group_header(trimmed) {
            group = Some(g);
            continue;
        }
        let Some(g) = group else { continue };
        let is_five_hour = trimmed.starts_with(FIVE_HOUR_HEADER);
        let is_weekly = trimmed.starts_with(WEEKLY_HEADER);
        if !is_five_hour && !is_weekly {
            continue;
        }
        let Some(window) = read_window(&modal[i + 1..]) else {
            continue;
        };
        let slot = match g {
            AgyModelGroup::Gemini => &mut usage.gemini,
            AgyModelGroup::ThirdParty => &mut usage.third_party,
        };
        if is_five_hour {
            slot.five_hour = Some(window);
        } else {
            slot.weekly = Some(window);
        }
    }
    if usage.gemini.is_empty() && usage.third_party.is_empty() {
        return None;
    }
    usage.active = active_group_of(&lines[hint + 1..]);
    Some(usage)
}

/// 系統の見出し行（`GEMINI MODELS` / `CLAUDE AND GPT MODELS`）
fn group_header(trimmed: &str) -> Option<AgyModelGroup> {
    if !trimmed.ends_with("MODELS") {
        return None;
    }
    if trimmed.starts_with("GEMINI") {
        Some(AgyModelGroup::Gemini)
    } else if trimmed.contains("CLAUDE") || trimmed.contains("GPT") {
        Some(AgyModelGroup::ThirdParty)
    } else {
        None
    }
}

/// 枠の見出しの下から「% の行」と「Refreshes in … の行」を読む
fn read_window(after: &[String]) -> Option<AgyQuotaWindow> {
    let (offset, remaining) = after
        .iter()
        .take(VALUE_LOOKAHEAD)
        .enumerate()
        .find_map(|(i, l)| trailing_percent(l).map(|p| (i, p)))?;
    let used = (100.0 - remaining).round().clamp(0.0, 100.0) as u32;
    let resets_in = after
        .get(offset + 1)
        .and_then(|l| l.trim().strip_prefix("Refreshes in "))
        .and_then(parse_duration);
    Some(AgyQuotaWindow {
        used_percent: used,
        resets_in,
    })
}

/// 行末の `NN.NN%` を読む（バーの後ろの数値）
fn trailing_percent(line: &str) -> Option<f64> {
    let body = line.trim().strip_suffix('%')?;
    let start = body
        .rfind(|c: char| !(c.is_ascii_digit() || c == '.'))
        .map_or(0, |i| i + 1);
    let value: f64 = body[start..].parse().ok()?;
    (0.0..=100.0).contains(&value).then_some(value)
}

/// `6d 11h 41m` / `2h 24m` / `45m` / `30s` を時間へ直す。知らない単位が混ざれば None
fn parse_duration(text: &str) -> Option<Duration> {
    let mut secs: u64 = 0;
    let mut any = false;
    for token in text.split_whitespace() {
        let unit = token.chars().last()?;
        let n: u64 = token[..token.len() - unit.len_utf8()].parse().ok()?;
        secs += n * match unit {
            'd' => 86_400,
            'h' => 3_600,
            'm' => 60,
            's' => 1,
            _ => return None,
        };
        any = true;
    }
    any.then(|| Duration::from_secs(secs))
}

/// フッターに出ているモデル名から、いま使っている系統を決める。
///
/// agy のフッターは `auto · Gemini 4 Hydrogen · medium` の形で、モーダルの操作ヒント
/// （`… esc Close`）より下にある。呼び手はヒントより下の行だけを渡す（モーダルの本文の
/// `Models within this group: Claude Opus …` を拾わないため）。読めなければ Gemini（既定）
fn active_group_of(footer: &[String]) -> AgyModelGroup {
    let third_party = footer.iter().any(|l| {
        l.split('·')
            .map(str::trim)
            .any(|seg| seg.starts_with("Claude") || seg.starts_with("GPT"))
    });
    if third_party {
        AgyModelGroup::ThirdParty
    } else {
        AgyModelGroup::Gemini
    }
}

/// 最後に読めた `/usage` を覚えておく（ステータスバーの agy 表示の値の出どころ）。
///
/// `/usage` は閉じると画面から消えるので、読めた時点の値をリセットまで持つ。
/// リセット時刻（読めた時刻 + `Refreshes in`）を過ぎた枠は**値を出さない**（`--` に戻す）:
/// リセット後の使用率は画面をもう一度読むまで分からないため、古い値を出し続けない
#[derive(Debug, Clone, Copy)]
pub struct AgyLimitsMemory {
    pub observed_at: Instant,
    pub usage: AgyUsage,
}

impl AgyLimitsMemory {
    /// いま使っている系統の (5h, 7d) の使用率。リセットを過ぎた枠は None
    pub fn current(&self, now: Instant) -> (Option<u32>, Option<u32>) {
        let group = self.usage.active_group();
        let alive = |w: Option<AgyQuotaWindow>| {
            let w = w?;
            match w.resets_in {
                Some(d) if now >= self.observed_at + d => None,
                _ => Some(w.used_percent),
            }
        };
        (alive(group.five_hour), alive(group.weekly))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 実測（agy 1.5.18・2026-11-27・120x45 のペイン）の `/usage` 画面。
    /// アカウント行はプレースホルダへ置換済み（#927）
    const USAGE_SCREEN: &str = "\
────────────────────────────────────────────────────────────
>
────────────────────────────────────────────────────────────
└ Models & Quota

  Account: testuser@example.com

GEMINI MODELS
  Models within this group: Gemini 4 Neon, Gemini 4 Carbon, Gemini 4 Argon, Gemini 4 Hydrogen

  Weekly Limit Remaining
    [██████████████████████████████████████████████░░░░] 91.11%
    Refreshes in 6d 11h 41m

  Five Hour Limit Remaining
    [█████████████████████████████████████████░░░░░░░░░] 81.92%
    Refreshes in 2h 24m


CLAUDE AND GPT MODELS
  Models within this group: Claude Opus 5.5, Claude Sonnet 5.5, GPT-OSS 120B

  Weekly Limit Remaining
    [██████████████████████████████████████████████████] 100.00%
    Quota available

  Five Hour Limit Remaining
    [██████████████████████████████████████████████████] 100.00%
    Quota available

  │Within each group, models share a weekly limit and a 5-hour limit. Quota is
  (1–28 of 30 lines)

  ↑/↓ Scroll · pgup/pgdown Page · ctrl+end Bottom · ctrl+home Top · esc Close
                                                         auto · Gemini 4 Hydrogen · medium";

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    #[test]
    fn 実測のusage画面から両系統の枠を読む() {
        let u = parse_usage_screen(&lines(USAGE_SCREEN)).expect("読める");
        assert_eq!(
            u.gemini.five_hour,
            Some(AgyQuotaWindow {
                used_percent: 18,
                resets_in: Some(Duration::from_secs(2 * 3600 + 24 * 60)),
            })
        );
        assert_eq!(
            u.gemini.weekly,
            Some(AgyQuotaWindow {
                used_percent: 9,
                resets_in: Some(Duration::from_secs(6 * 86_400 + 11 * 3600 + 41 * 60)),
            })
        );
        let unused = Some(AgyQuotaWindow {
            used_percent: 0,
            resets_in: None,
        });
        assert_eq!(u.third_party.five_hour, unused);
        assert_eq!(u.third_party.weekly, unused);
        assert_eq!(u.active, AgyModelGroup::Gemini);
        assert_eq!(u.active_group(), u.gemini);
    }

    #[test]
    fn フッターのモデルがclaudeなら他社モデルの枠を出す() {
        let screen = USAGE_SCREEN.replace(
            "auto · Gemini 4 Hydrogen · medium",
            "auto · Claude Opus 5.5 · high",
        );
        let u = parse_usage_screen(&lines(&screen)).expect("読める");
        assert_eq!(u.active, AgyModelGroup::ThirdParty);
        assert_eq!(u.active_group(), u.third_party);
    }

    #[test]
    fn モーダルの見出しが無ければ読まない() {
        // 会話の本文に同じ文言が出ても拾わない
        let screen = USAGE_SCREEN.replace("└ Models & Quota", "");
        assert_eq!(parse_usage_screen(&lines(&screen)), None);
        assert_eq!(
            parse_usage_screen(&lines("> hello\n  auto · Gemini 4 Hydrogen · medium")),
            None
        );
    }

    #[test]
    fn claudeの会話に写ったusage画面は読まない() {
        // ツールの出力として写った画面の下に claude の入力欄とフッターがある
        let screen = format!(
            "{USAGE_SCREEN}\n\n⏺ agy の /usage を読みました\n\n\
             ────────────────\n❯ \n────────────────\n  ? for shortcuts"
        );
        assert_eq!(parse_usage_screen(&lines(&screen)), None);
        // 入力欄が無くても、ヒントの下に会話が続いていれば読まない
        let mut long = USAGE_SCREEN.to_string();
        for n in 0..12 {
            long.push_str(&format!("\n  会話の続き {n}"));
        }
        assert_eq!(parse_usage_screen(&lines(&long)), None);
    }

    #[test]
    fn 操作ヒントが無ければ読まない() {
        let screen = USAGE_SCREEN.replace("esc Close", "");
        assert_eq!(parse_usage_screen(&lines(&screen)), None);
    }

    #[test]
    fn 自前のstatusline4行があっても読める() {
        let screen = format!(
            "{USAGE_SCREEN}\n[Gemini 4 Hydrogen (Medium)]\nctx   0% ░░░░░░░░░░\n\
             5h   18% █░░░░░░░░░ (→2h24m)\n7d    9% ░░░░░░░░░░ (→6d11h)"
        );
        let u = parse_usage_screen(&lines(&screen)).expect("読める");
        assert_eq!(u.gemini.five_hour.map(|w| w.used_percent), Some(18));
    }

    #[test]
    fn スクロールで片方の系統しか見えなくても読める() {
        // 他社モデルの系統を丸ごと抜いた画面（ヒントとフッターは残る）
        let all: Vec<&str> = USAGE_SCREEN.lines().collect();
        let from = all
            .iter()
            .position(|l| l.starts_with("CLAUDE AND GPT"))
            .unwrap();
        let to = all
            .iter()
            .position(|l| l.contains("│Within each group"))
            .unwrap();
        let cut = [&all[..from], &all[to..]].concat().join("\n");
        let u = parse_usage_screen(&lines(&cut)).expect("読める");
        assert_eq!(u.gemini.five_hour.map(|w| w.used_percent), Some(18));
        assert_eq!(u.third_party, AgyQuotaGroup::default());
    }

    #[test]
    fn 時間の表記を読む() {
        assert_eq!(parse_duration("45m"), Some(Duration::from_secs(2700)));
        assert_eq!(parse_duration("1d 2h"), Some(Duration::from_secs(93_600)));
        assert_eq!(parse_duration("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_duration("soon"), None);
        assert_eq!(parse_duration(""), None);
    }

    #[test]
    fn リセットを過ぎた枠は値を出さない() {
        let u = parse_usage_screen(&lines(USAGE_SCREEN)).expect("読める");
        let t0 = Instant::now();
        let mem = AgyLimitsMemory {
            observed_at: t0,
            usage: u,
        };
        assert_eq!(mem.current(t0), (Some(18), Some(9)));
        // 5h 枠のリセット（2h24m）を過ぎたら 5h だけ消える
        let later = t0 + Duration::from_secs(3 * 3600);
        assert_eq!(mem.current(later), (None, Some(9)));
    }
}
