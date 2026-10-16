//! 利用制限の % の区間按分（usage 分析 S3 #2202。設計書 `.agent/plans/2026-10-usage-analytics.md`
//! §2.2〜§2.4）。
//!
//! ログイン × 枠（5h / 7d）ごとに、観測した used_percent の増分 Δ を、観測点の間の区間に同じログインで
//! 使われたローカルの使用量（API 換算）の比で行へ配る純関数。**合計は観測と必ず一致する**（区間の Δ の和は
//! 端の値になり、1 区間の 1% の段の誤差は足しても積み上がらない）。
//!
//! # 手順（§2.4）
//!
//! 1. 観測の列を窓（解除時刻）ごとに作る（[`windows_for`]）。解除時刻の揺れ（窓の長さの半分まで）は同じ窓へ
//!    寄せ、窓の中では観測時刻の順に**値が上がった点だけ**を残す（同じ値は最初の点が正・下がった値は古い報告
//!    として捨てる・窓の外の時刻は捨てる）
//! 2. 窓の開始 `s = 解除時刻 − 長さ`（前の窓の解除時刻より前には置かない）を `(s, 0)` として先頭に置く
//! 3. 隣り合う点の区間ごとに、重なる行（端の刻みは重なった時間の比）へ Δ を API 換算の比で配る
//!    （[`allocate`]）。区間に単価表に無いモデルの行があれば、その区間は生トークン総量の比で配る（§2.3）
//! 4. 区間のローカルの使用量が 0 で Δ > 0 なら「tako の外」へ入れる（claude.ai・別の PC・tako が読めない設定 dir）
//! 5. 観測の区間の外（最後の観測より後・観測の無い窓）の使用量は、係数（直近 7 日の区間の「1% あたりの
//!    API 換算」の中央値）で割った値を**推定**として別に持つ。係数が無ければ推定できない（量だけ数える）
//!
//! **codex（S5 #2204）は推定しない**（[`allocate_with`]）: codex の rollout は使用量と観測を同じ行に持つので
//! 観測の無い使用量が無く、区間の外の使用量は「% が上がらなかった続き」か「その枠の無いプランの期間
//! （週の枠だけ）」のどちらか。係数で割ると後者を 5h の % へ数えてしまう（この機の rollout で延べ 15 万 %）。
//! 窓の寄せ方も違う（[`CODEX_RESET_JITTER_SECS`]）。
//!
//! 行（5 分刻み。35 日より古い月は日単位）の中の使用量は時間に一様とみなす。

use std::collections::BTreeMap;

use serde::Serialize;

use super::LimitObservation;

/// 5 時間の枠（mod の `kind`）
pub const FIVE_HOUR: &str = "five_hour";
/// 7 日の枠
pub const SEVEN_DAY: &str = "seven_day";
/// 按分する枠と長さ（秒）
pub const KINDS: [(&str, i64); 2] = [(FIVE_HOUR, 5 * 3600), (SEVEN_DAY, 7 * 86_400)];
/// 推定の係数を取る区間の新しさ（区間の終わりが今からこの秒数以内。無ければすべての区間へ広げる）
pub const COEFFICIENT_RECENT_SECS: i64 = 7 * 86_400;
/// codex の観測（`source: codex`）で同じ窓とみなす解除時刻の揺れ（秒。S5 #2204）。
///
/// codex は `resets_at` を報告のたびに計算し直すので秒単位で揺れ、同じログインの別の会話は数分ずれる
/// （この機の rollout で最大 298 秒）。一方で**週の枠が前の窓の解除より前に新しく始まる**（プランの切り替え・
/// 早い解除。実測で 1.5〜3.9 日ずれた解除時刻が続いた）ので、claude の「窓の長さの半分」で寄せると
/// 別の窓を 1 つに束ねて延べの % を落とす。0% の間の解除時刻は「今 + 長さ」で動くので、使い始める前の
/// 報告は小さな窓として残る（値の上がった点が無いので配る区間は無い）
pub const CODEX_RESET_JITTER_SECS: i64 = 3600;

/// 観測の点（unix 秒・%）
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Point {
    pub t: i64,
    pub percent: f64,
}

/// 1 つの窓の観測（§2.4 の 1・2）
#[derive(Debug, Clone, PartialEq)]
pub struct ObservedWindow {
    /// 窓の開始（解除時刻 − 長さ。前の窓の解除時刻より前には置かない）
    pub start: i64,
    pub resets_at: i64,
    /// 先頭は `(start, 0)`。観測時刻も % も狭義に増える
    pub points: Vec<Point>,
    /// この窓の観測の点数（同じ値の重複も数える）
    pub observations: usize,
    /// 使わなかった観測（下がった値 = 古い報告・窓の外の時刻）
    pub ignored: usize,
}

impl ObservedWindow {
    /// 最新の観測の %（値の上がった観測が無ければ 0）
    pub fn latest_percent(&self) -> f64 {
        self.points.last().map_or(0.0, |p| p.percent)
    }

    /// 値の上がった最後の観測の時刻（これより後は推定）
    pub fn last_observed_at(&self) -> i64 {
        self.points.last().map_or(self.start, |p| p.t)
    }
}

/// ログイン `login` の枠 `kind` の観測がすべて codex の rollout から来たか（S5 #2204。窓の寄せ方と推定の有無が違う）
pub fn codex_only(observations: &[LimitObservation], login: &str, kind: &str) -> bool {
    let mut any = false;
    for o in observations
        .iter()
        .filter(|o| o.login == login && o.kind == kind)
    {
        if o.source != super::SOURCE_CODEX {
            return false;
        }
        any = true;
    }
    any
}

/// ログイン `login` の枠 `kind`（長さ `len` 秒）の窓を古い順に組む。2 つ目は解除時刻の無い・% の壊れた観測の数
pub fn windows_for(
    observations: &[LimitObservation],
    login: &str,
    kind: &str,
    len: i64,
) -> (Vec<ObservedWindow>, usize) {
    let mut by_reset: BTreeMap<i64, Vec<(i64, f64)>> = BTreeMap::new();
    let mut unusable = 0usize;
    // codex の観測だけのログイン・枠は窓の寄せ方が違う（[`CODEX_RESET_JITTER_SECS`]）
    let codex = codex_only(observations, login, kind);
    for o in observations
        .iter()
        .filter(|o| o.login == login && o.kind == kind)
    {
        match o.resets_at {
            Some(r) if o.percent.is_finite() => by_reset
                .entry(r)
                .or_default()
                .push(((o.observed_at / 1000) as i64, o.percent)),
            _ => unusable += 1,
        }
    }
    // 解除時刻の揺れを同じ窓へ寄せる。続く窓の解除時刻は前の窓の解除時刻から長さ以上離れる
    // （新しい窓は前の窓が解けた後の最初の使用で始まる）ので、長さの半分以内は同じ窓とみなす。
    // codex は前の窓の解除より前に新しい窓が始まりうるので、揺れの上限で寄せる
    let tolerance = if codex {
        CODEX_RESET_JITTER_SECS.min(len / 2)
    } else {
        len / 2
    };
    // （束の解除時刻 = いちばん遅いもの, いちばん早いもの, 観測）
    let mut groups: Vec<(i64, i64, Vec<(i64, f64)>)> = Vec::new();
    for (r, pts) in by_reset {
        match groups.last_mut() {
            Some((gr, _, gpts)) if r - *gr <= tolerance => {
                *gr = r;
                gpts.extend(pts);
            }
            _ => groups.push((r, r, pts)),
        }
    }
    let mut out = Vec::with_capacity(groups.len());
    let mut prev_reset: Option<i64> = None;
    // codex: 前の窓の最後の点（窓の区間が重ならないよう、次の窓の開始をこれより前に置かない）
    let mut prev_last: Option<i64> = None;
    for (r, first_r, mut pts) in groups {
        let start = if codex {
            // 早く始まった窓は前の窓の解除を待たない。揺れの中でいちばん早い解除時刻から開始を取る
            prev_last.map_or(first_r - len, |p| (first_r - len).max(p))
        } else {
            prev_reset.map_or(r - len, |p| (r - len).max(p))
        };
        prev_reset = Some(r);
        pts.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        let observations = pts.len();
        let mut points = vec![Point {
            t: start,
            percent: 0.0,
        }];
        let mut ignored = 0usize;
        for (t, p) in pts {
            if t <= start || t >= r {
                ignored += 1;
                continue;
            }
            let Some(last) = points.last_mut() else {
                continue;
            };
            if p <= last.percent {
                // 同じ値は最初の点が正（mod は値が変わったときだけ観測時刻を打つ）。下がった値は古い報告
                if p < last.percent {
                    ignored += 1;
                }
                continue;
            }
            if t == last.t {
                // 同じ秒に 2 つの段（秒へ丸めた結果）: 高い方の 1 点にする
                last.percent = p;
                continue;
            }
            points.push(Point { t, percent: p });
        }
        prev_last = points.last().map(|p| p.t);
        out.push(ObservedWindow {
            start,
            resets_at: r,
            points,
            observations,
            ignored,
        });
    }
    (out, unusable)
}

/// 今の窓
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Current {
    /// 観測した窓（`windows` の添字）
    Observed {
        index: usize,
        start: i64,
        resets_at: i64,
    },
    /// 観測した今の窓が無い（観測が無い・解除時刻を過ぎた）→ 今から長さだけ遡る。ただし最後に解けた
    /// 窓の解除時刻より前へは遡らない（今の窓はそれより前に始まらない）
    Rolling { start: i64 },
}

/// 今の窓（解除時刻が今より後の、いちばん新しい窓）
pub fn current_window(windows: &[ObservedWindow], len: i64, now: i64) -> Current {
    match windows.iter().rposition(|w| w.resets_at > now) {
        Some(index) => Current::Observed {
            index,
            start: windows[index].start,
            resets_at: windows[index].resets_at,
        },
        None => {
            let last_reset = windows
                .iter()
                .map(|w| w.resets_at)
                .filter(|r| *r <= now)
                .max();
            Current::Rolling {
                start: last_reset.map_or(now - len, |r| (now - len).max(r)),
            }
        }
    }
}

/// 按分する行（1 つのログインの使用量の 1 刻み）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsageSlice {
    /// 刻みの頭（unix 秒）
    pub t: i64,
    /// 刻みの長さ（5 分。日単位へまとめた月は 1 日）
    pub span: i64,
    /// API 換算（USD。単価表に無いモデルは `None`）
    pub usd: Option<f64>,
    /// 生トークン総量
    pub tokens: u64,
}

impl UsageSlice {
    /// 使用量が時間に一様に散らばる範囲の終わり（今の刻みは今まで）
    fn end(&self, now: i64) -> i64 {
        let end = self.t + self.span.max(1);
        if self.t < now && end > now {
            now
        } else {
            end
        }
    }
}

/// 行の切れ端（時間の範囲ごとに配った %）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Piece {
    /// 行の添字（[`allocate`] に渡した `slices` の）
    pub slice: usize,
    pub from: i64,
    pub to: i64,
    /// 観測から配った %
    pub observed: f64,
    /// 推定の %
    pub estimated: f64,
    /// 推定できなかった（係数が無い）API 換算
    pub unestimated_usd: f64,
    /// 推定できなかった単価不明のトークン
    pub unestimated_tokens: f64,
}

/// tako の外（区間にローカルの使用量が無いのに % が増えた）
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct OutsideSpan {
    pub from: i64,
    pub to: i64,
    pub percent: f64,
}

/// 1% あたりの量の中央値と四分位（当てはまりの目安。幅が広いほど配り方が不確か）
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Spread {
    pub median: f64,
    pub p25: f64,
    pub p75: f64,
}

/// 推定の係数（1% あたりの量）
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Coefficient {
    /// 1% あたりの API 換算（USD）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usd_per_percent: Option<f64>,
    /// 1% あたりの生トークン（単価表に無いモデルの行の推定に使う）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_per_percent: Option<f64>,
    /// 係数を取った区間の数
    pub intervals: usize,
    /// 直近 7 日の区間から取れた（false = 無かったので古い区間まで広げた）
    pub recent: bool,
}

/// 窓ごとの当てはまり
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WindowFit {
    pub start: i64,
    pub resets_at: i64,
    pub observations: usize,
    pub latest_percent: f64,
    /// Δ > 0 でローカルの使用量のある区間の数
    pub intervals: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usd_per_percent: Option<Spread>,
}

/// 1 つのログイン・1 つの枠の按分の結果
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Allocation {
    pub pieces: Vec<Piece>,
    pub outside: Vec<OutsideSpan>,
    pub coefficient: Option<Coefficient>,
    /// 窓ごとの当てはまり（`windows` と同じ並び）
    pub fits: Vec<WindowFit>,
    /// 単価表に無いモデルの行を含み、生トークンの比で配った区間の数
    pub token_weighted_intervals: usize,
}

/// 観測の区間
struct Interval {
    from: i64,
    to: i64,
    delta: f64,
    window: usize,
}

/// 区間の重み（API 換算と生トークン。単価不明の行があれば生トークンで配る）
#[derive(Default, Clone, Copy)]
struct Weight {
    usd: f64,
    tokens: f64,
    unpriced: bool,
}

impl Weight {
    fn chosen(&self) -> f64 {
        if self.unpriced {
            self.tokens
        } else {
            self.usd
        }
    }
}

/// 並べた値の分位（線形補間）
fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let pos = q.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    let frac = pos - lo as f64;
    sorted[lo] + (sorted[hi] - sorted[lo]) * frac
}

fn spread(mut values: Vec<f64>) -> Option<Spread> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    Some(Spread {
        median: quantile(&values, 0.5),
        p25: quantile(&values, 0.25),
        p75: quantile(&values, 0.75),
    })
}

/// 区間按分の本体（§2.4 の 3〜5）。`slices` は 1 つのログインの全行（期間・絞り込みの前）
pub fn allocate(slices: &[UsageSlice], windows: &[ObservedWindow], now: i64) -> Allocation {
    allocate_with(slices, windows, now, true)
}

/// [`allocate`] に、観測の区間の外の使用量を推定するかを渡す（codex は `false` = 区間の外は配らない。
/// 観測は使用量と同じ行に載るので、区間の外は % の上がらなかった続きか、その枠の無いプランの期間）
pub fn allocate_with(
    slices: &[UsageSlice],
    windows: &[ObservedWindow],
    now: i64,
    estimate: bool,
) -> Allocation {
    let mut intervals: Vec<Interval> = Vec::new();
    for (wi, w) in windows.iter().enumerate() {
        for pair in w.points.windows(2) {
            intervals.push(Interval {
                from: pair[0].t,
                to: pair[1].t,
                delta: pair[1].percent - pair[0].percent,
                window: wi,
            });
        }
    }
    // 窓は重ならず（開始は前の窓の解除時刻より前に置かない）、窓の中の区間は続いているので、
    // 区間は時刻の順に並び重ならない
    intervals.sort_by_key(|iv| iv.from);

    // 行を区間で切る: 区間の中の切れ端（観測で配る）と外の切れ端（推定）
    struct Raw {
        slice: usize,
        from: i64,
        to: i64,
        frac: f64,
        interval: Option<usize>,
    }
    let mut raws: Vec<Raw> = Vec::new();
    let mut weights = vec![Weight::default(); intervals.len()];
    for (si, s) in slices.iter().enumerate() {
        if s.tokens == 0 {
            continue;
        }
        let (a, b) = (s.t, s.end(now));
        if b <= a {
            continue;
        }
        let len = (b - a) as f64;
        let mut cursor = a;
        let first = intervals.partition_point(|iv| iv.to <= a);
        for (ii, iv) in intervals.iter().enumerate().skip(first) {
            if iv.from >= b {
                break;
            }
            let lo = iv.from.max(a);
            let hi = iv.to.min(b);
            if lo > cursor {
                raws.push(Raw {
                    slice: si,
                    from: cursor,
                    to: lo,
                    frac: (lo - cursor) as f64 / len,
                    interval: None,
                });
            }
            if hi > lo {
                let frac = (hi - lo) as f64 / len;
                raws.push(Raw {
                    slice: si,
                    from: lo,
                    to: hi,
                    frac,
                    interval: Some(ii),
                });
                let w = &mut weights[ii];
                match s.usd {
                    Some(usd) => w.usd += usd * frac,
                    None => w.unpriced = true,
                }
                w.tokens += s.tokens as f64 * frac;
            }
            cursor = cursor.max(hi);
        }
        if cursor < b {
            raws.push(Raw {
                slice: si,
                from: cursor,
                to: b,
                frac: (b - cursor) as f64 / len,
                interval: None,
            });
        }
    }

    // 係数（§2.2 の B）: Δ > 0 でローカルの使用量のある区間の「1% あたりの量」の中央値。直近 7 日を優先
    let ratio_usd = |ii: usize| {
        let (iv, w) = (&intervals[ii], &weights[ii]);
        (iv.delta > 0.0 && !w.unpriced && w.usd > 0.0).then(|| w.usd / iv.delta)
    };
    let ratio_tokens = |ii: usize| {
        let (iv, w) = (&intervals[ii], &weights[ii]);
        (iv.delta > 0.0 && w.tokens > 0.0).then(|| w.tokens / iv.delta)
    };
    let pick = |recent: bool| -> Vec<usize> {
        (0..intervals.len())
            .filter(|&ii| !recent || intervals[ii].to >= now - COEFFICIENT_RECENT_SECS)
            .filter(|&ii| ratio_tokens(ii).is_some())
            .collect()
    };
    let recent = pick(true);
    let (chosen, is_recent) = if recent.is_empty() {
        (pick(false), false)
    } else {
        (recent, true)
    };
    let coefficient = (!chosen.is_empty()).then(|| Coefficient {
        usd_per_percent: spread(chosen.iter().filter_map(|&ii| ratio_usd(ii)).collect())
            .map(|s| s.median),
        tokens_per_percent: spread(chosen.iter().filter_map(|&ii| ratio_tokens(ii)).collect())
            .map(|s| s.median),
        intervals: chosen.len(),
        recent: is_recent,
    });

    let fits = windows
        .iter()
        .enumerate()
        .map(|(wi, w)| {
            let ids: Vec<usize> = (0..intervals.len())
                .filter(|&ii| intervals[ii].window == wi && ratio_tokens(ii).is_some())
                .collect();
            WindowFit {
                start: w.start,
                resets_at: w.resets_at,
                observations: w.observations,
                latest_percent: w.latest_percent(),
                intervals: ids.len(),
                usd_per_percent: spread(ids.iter().filter_map(|&ii| ratio_usd(ii)).collect()),
            }
        })
        .collect();

    let mut out = Allocation {
        coefficient,
        fits,
        token_weighted_intervals: weights
            .iter()
            .filter(|w| w.unpriced && w.tokens > 0.0)
            .count(),
        ..Allocation::default()
    };
    // 区間のローカルの使用量が 0 で Δ > 0 → tako の外
    for (iv, w) in intervals.iter().zip(&weights) {
        if iv.delta > 0.0 && w.chosen() <= 0.0 {
            out.outside.push(OutsideSpan {
                from: iv.from,
                to: iv.to,
                percent: iv.delta,
            });
        }
    }
    for r in raws {
        let s = &slices[r.slice];
        let mut piece = Piece {
            slice: r.slice,
            from: r.from,
            to: r.to,
            observed: 0.0,
            estimated: 0.0,
            unestimated_usd: 0.0,
            unestimated_tokens: 0.0,
        };
        match r.interval {
            Some(ii) => {
                let w = weights[ii];
                let total = w.chosen();
                if total > 0.0 {
                    let mine = if w.unpriced {
                        s.tokens as f64 * r.frac
                    } else {
                        s.usd.unwrap_or(0.0) * r.frac
                    };
                    piece.observed = intervals[ii].delta * mine / total;
                }
            }
            None if !estimate => {}
            None => {
                let c = out.coefficient;
                match (s.usd, c.and_then(|c| c.usd_per_percent)) {
                    (Some(usd), Some(per)) if per > 0.0 => piece.estimated = usd * r.frac / per,
                    (Some(usd), _) => piece.unestimated_usd = usd * r.frac,
                    (None, _) => match c.and_then(|c| c.tokens_per_percent) {
                        Some(per) if per > 0.0 => {
                            piece.estimated = s.tokens as f64 * r.frac / per;
                        }
                        _ => piece.unestimated_tokens = s.tokens as f64 * r.frac,
                    },
                }
            }
        }
        out.pieces.push(piece);
    }
    out
}

/// 切れ端 `[from, to)` のうち期間 `[lo, hi)` に重なる割合（長さ 0 の切れ端は中にあれば 1）
pub fn overlap_fraction(from: i64, to: i64, lo: i64, hi: i64) -> f64 {
    if to <= from {
        return if from >= lo && from < hi { 1.0 } else { 0.0 };
    }
    let a = from.max(lo);
    let b = to.min(hi);
    if b <= a {
        0.0
    } else {
        (b - a) as f64 / (to - from) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::KeyFrom;

    const H: i64 = 3600;
    const LEN5: i64 = 5 * H;
    /// 窓の解除時刻（5h）。窓の開始は R − 5h
    const R: i64 = 1_792_000_000;
    const S: i64 = R - LEN5;

    fn obs(kind: &str, percent: f64, resets_at: i64, observed_secs: i64) -> LimitObservation {
        LimitObservation {
            login: "L".into(),
            key_from: KeyFrom::Account,
            kind: kind.into(),
            percent,
            resets_at: Some(resets_at),
            observed_at: (observed_secs * 1000) as u64,
            recorded_at: 0,
            source: "mod".into(),
        }
    }

    fn slice(t: i64, usd: f64) -> UsageSlice {
        UsageSlice {
            t,
            span: 300,
            usd: Some(usd),
            tokens: (usd * 1000.0) as u64 + 1,
        }
    }

    #[test]
    fn 窓は解除時刻で切り_揺れは寄せ_値の上がった点だけを残す() {
        let rows = vec![
            obs(FIVE_HOUR, 10.0, R, S + 600),
            // 別のペインの同じ値（後の時刻）→ 最初の点が正
            obs(FIVE_HOUR, 10.0, R, S + 900),
            // 放置したペインの古い値（下がる）→ 捨てる
            obs(FIVE_HOUR, 8.0, R + 30, S + 1200),
            obs(FIVE_HOUR, 12.0, R + 30, S + 1800),
            // 窓の外の時刻（寄せた窓の解除 R+30 の後）→ 捨てる
            obs(FIVE_HOUR, 50.0, R, R + 60),
            // 次の窓（解除時刻が長さ以上離れる）
            obs(FIVE_HOUR, 3.0, R + LEN5, R + 600),
            // 別の枠・別のログイン・解除時刻の無い観測は混ざらない
            obs(SEVEN_DAY, 40.0, R, S + 700),
            LimitObservation {
                login: "other".into(),
                ..obs(FIVE_HOUR, 70.0, R, S + 700)
            },
            LimitObservation {
                resets_at: None,
                ..obs(FIVE_HOUR, 1.0, R, S + 100)
            },
        ];
        let (w, unusable) = windows_for(&rows, "L", FIVE_HOUR, LEN5);
        assert_eq!(unusable, 1, "解除時刻の無い観測");
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].start, S + 30, "揺れを寄せた窓は遅い方の解除時刻");
        assert_eq!(w[0].resets_at, R + 30);
        assert_eq!(
            w[0].points,
            vec![
                Point {
                    t: S + 30,
                    percent: 0.0
                },
                Point {
                    t: S + 600,
                    percent: 10.0
                },
                Point {
                    t: S + 1800,
                    percent: 12.0
                },
            ]
        );
        assert_eq!(w[0].observations, 5);
        assert_eq!(w[0].ignored, 2, "下がった値と窓の外の時刻");
        // 次の窓は前の窓の解除時刻より前に始まらない
        assert_eq!(w[1].start, R + 30);
        assert_eq!(w[1].latest_percent(), 3.0);
        // 今の窓
        assert_eq!(
            current_window(&w, LEN5, R + 60),
            Current::Observed {
                index: 1,
                start: R + 30,
                resets_at: R + LEN5
            }
        );
        // すべて解けた後は今から遡る（最後の解除時刻より前へは遡らない）
        assert_eq!(
            current_window(&w, LEN5, R + LEN5 + 600),
            Current::Rolling { start: R + LEN5 }
        );
        assert_eq!(
            current_window(&[], LEN5, R),
            Current::Rolling { start: R - LEN5 }
        );
    }

    /// S5 #2204: codex の観測は解除時刻の揺れを 1 時間まで寄せ、前の窓の解除より前に始まる窓
    /// （プランの切り替え・早い解除）を別の窓として配る。claude の寄せ方（長さの半分）だと束ねて延べを落とす
    #[test]
    fn codexの窓は揺れを1時間まで寄せ早く始まった窓を分ける() {
        const D: i64 = 86_400;
        const LEN7: i64 = 7 * D;
        let codex = |percent: f64, resets_at: i64, t: i64| LimitObservation {
            source: crate::usage::SOURCE_CODEX.into(),
            ..obs(SEVEN_DAY, percent, resets_at, t)
        };
        let s_a = R; // 窓 A の開始
        let r_a = s_a + LEN7;
        let t_p = s_a + 2 * D + 2 * H; // 0% の報告（使い始める前 = 解除時刻は今 + 長さ）
        let s_b = s_a + 2 * D + 12 * H; // 窓 B は A の解除を待たずに始まる
        let r_b = s_b + LEN7;
        let rows = vec![
            codex(0.0, r_a - 12, s_a + 5),
            codex(10.0, r_a, s_a + 3 * H),
            codex(10.0, r_a + 1, s_a + 4 * H),
            // 同じログインの別の会話の低い値（数分ずれた解除時刻）= 下がった値として捨てる
            codex(1.0, r_a + 298, s_a + 5 * H),
            codex(20.0, r_a, s_a + 6 * H),
            codex(40.0, r_a + 1, s_a + 2 * D),
            codex(0.0, t_p + LEN7, t_p),
            codex(1.0, r_b, s_b + 600),
            codex(33.0, r_b + 2, s_b + 3 * H),
            codex(76.0, r_b, s_b + 5 * H),
        ];
        let (w, _) = windows_for(&rows, "L", SEVEN_DAY, LEN7);
        assert_eq!(w.len(), 3, "A・使い始める前の 0%・B");
        assert_eq!(
            w[0].start,
            r_a - 12 - LEN7,
            "揺れの中でいちばん早い解除時刻から開始"
        );
        assert_eq!(w[0].resets_at, r_a + 298);
        assert_eq!(w[0].latest_percent(), 40.0);
        assert_eq!(w[0].ignored, 1, "別の会話の低い値");
        assert_eq!(w[1].points.len(), 1, "0% だけの窓は配る区間を持たない");
        assert_eq!(w[2].start, s_b, "B は A の解除（r_a）を待たずに始まる");
        assert_eq!(w[2].latest_percent(), 76.0);
        // 区間は重ならず、配った合計は窓ごとの最新の観測の和（延べ 116%）
        let slices: Vec<UsageSlice> = (0..60)
            .map(|i| UsageSlice {
                t: s_a + i * 2 * H,
                span: 300,
                usd: None,
                tokens: 1000,
            })
            .chain((0..30).map(|i| UsageSlice {
                t: s_b + 300 + i * 600,
                span: 300,
                usd: None,
                tokens: 1000,
            }))
            .collect();
        let alloc = allocate(&slices, &w, s_b + 10 * D);
        let observed: f64 = alloc.pieces.iter().map(|p| p.observed).sum::<f64>()
            + alloc.outside.iter().map(|o| o.percent).sum::<f64>();
        assert!((observed - 116.0).abs() < 1e-9, "{observed}");
        assert!(
            alloc.token_weighted_intervals > 0,
            "単価表に無いモデルは生トークンの比"
        );
        // 推定の有無: 既定（claude）は区間の外（A の後〜B の前・B の後）を推定し、codex は配らない
        let est = |a: &Allocation| {
            a.pieces
                .iter()
                .map(|p| p.estimated + p.unestimated_tokens + p.unestimated_usd)
                .sum::<f64>()
        };
        assert!(est(&alloc) > 0.0);
        let codex_alloc = allocate_with(&slices, &w, s_b + 10 * D, false);
        assert_eq!(est(&codex_alloc), 0.0, "codex は観測の区間の外を推定しない");
        let codex_observed: f64 = codex_alloc.pieces.iter().map(|p| p.observed).sum::<f64>()
            + codex_alloc.outside.iter().map(|o| o.percent).sum::<f64>();
        assert!((codex_observed - 116.0).abs() < 1e-9);
        assert!(codex_only(&rows, "L", SEVEN_DAY));
        assert!(
            !codex_only(&rows, "L", FIVE_HOUR),
            "観測の無い枠は codex と言わない"
        );
        // 同じ観測を claude（mod）の寄せ方で読むと A と B が 1 つに束なり、延べは 76% しか残らない
        let as_mod: Vec<LimitObservation> = rows
            .iter()
            .map(|o| LimitObservation {
                source: "mod".into(),
                ..o.clone()
            })
            .collect();
        let (m, _) = windows_for(&as_mod, "L", SEVEN_DAY, LEN7);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].latest_percent(), 76.0);
    }

    #[test]
    fn 区間のdeltaを重なった行の比で配り合計は観測と一致する() {
        // 区間 [S, S+1200) で 10%、[S+1200, S+2400) で 2%
        let w = vec![ObservedWindow {
            start: S,
            resets_at: R,
            points: vec![
                Point { t: S, percent: 0.0 },
                Point {
                    t: S + 1200,
                    percent: 10.0,
                },
                Point {
                    t: S + 2400,
                    percent: 12.0,
                },
            ],
            observations: 2,
            ignored: 0,
        }];
        // 刻み [S+900, S+1200) は 1 つ目の区間・[S+1200, S+1500) は 2 つ目・
        // [S+1050, S+1350) は半分ずつ
        let slices = vec![
            slice(S + 900, 3.0),
            slice(S + 1200, 1.0),
            UsageSlice {
                t: S + 1050,
                ..slice(0, 2.0)
            },
        ];
        let a = allocate(&slices, &w, R - 60);
        let total: f64 = a.pieces.iter().map(|p| p.observed).sum();
        assert!((total - 12.0).abs() < 1e-9, "合計 {total}");
        let by_slice = |i: usize| -> f64 {
            a.pieces
                .iter()
                .filter(|p| p.slice == i)
                .map(|p| p.observed)
                .sum()
        };
        // 1 つ目の区間: 重み 3 + 1（行 2 の半分）→ 10 × 3/4・10 × 1/4
        // 2 つ目の区間: 重み 1 + 1 → 2 × 1/2 ずつ
        assert!((by_slice(0) - 7.5).abs() < 1e-9);
        assert!((by_slice(1) - 1.0).abs() < 1e-9);
        assert!((by_slice(2) - 3.5).abs() < 1e-9);
        assert!(a.outside.is_empty());
        // 1% あたり: 4 USD / 10% = 0.4・2 USD / 2% = 1.0
        let fit = a.fits[0].usd_per_percent.unwrap();
        assert_eq!(a.fits[0].intervals, 2);
        assert!((fit.median - 0.7).abs() < 1e-9);
        assert!((fit.p25 - 0.55).abs() < 1e-9 && (fit.p75 - 0.85).abs() < 1e-9);
    }

    #[test]
    fn 使用量の無い区間はtakoの外_区間の外は係数で推定() {
        let w = vec![ObservedWindow {
            start: S,
            resets_at: R,
            points: vec![
                Point { t: S, percent: 0.0 },
                Point {
                    t: S + 600,
                    percent: 4.0,
                },
                // [S+600, S+1800) は使用量なしで 3% 増えた
                Point {
                    t: S + 1800,
                    percent: 7.0,
                },
            ],
            observations: 2,
            ignored: 0,
        }];
        let slices = vec![
            slice(S, 2.0),
            // 最後の観測より後
            slice(S + 2400, 1.0),
            // 窓より前（観測の無い窓）
            slice(S - 3 * H, 5.0),
        ];
        let a = allocate(&slices, &w, S + 3000);
        assert_eq!(
            a.outside,
            vec![OutsideSpan {
                from: S + 600,
                to: S + 1800,
                percent: 3.0
            }]
        );
        let c = a.coefficient.expect("係数");
        // 係数 = 2.0 USD / 4% = 0.5
        assert_eq!(c.usd_per_percent, Some(0.5));
        assert!(c.recent);
        let est: Vec<(usize, f64)> = a
            .pieces
            .iter()
            .filter(|p| p.estimated > 0.0)
            .map(|p| (p.slice, p.estimated))
            .collect();
        assert_eq!(est, vec![(1, 2.0), (2, 10.0)]);
        let observed: f64 = a.pieces.iter().map(|p| p.observed).sum();
        let outside: f64 = a.outside.iter().map(|o| o.percent).sum();
        assert!(
            (observed + outside - w[0].latest_percent()).abs() < 1e-9,
            "配った分 + tako の外 = 最新の観測"
        );
    }

    #[test]
    fn 単価不明の区間は生トークンで配り_係数が無ければ推定できない() {
        let w = vec![ObservedWindow {
            start: S,
            resets_at: R,
            points: vec![
                Point { t: S, percent: 0.0 },
                Point {
                    t: S + 600,
                    percent: 3.0,
                },
            ],
            observations: 1,
            ignored: 0,
        }];
        let slices = vec![
            UsageSlice {
                t: S,
                span: 300,
                usd: Some(100.0),
                tokens: 1000,
            },
            UsageSlice {
                t: S + 300,
                span: 300,
                usd: None,
                tokens: 2000,
            },
        ];
        let a = allocate(&slices, &w, S + 600);
        assert_eq!(a.token_weighted_intervals, 1);
        let by = |i: usize| -> f64 {
            a.pieces
                .iter()
                .filter(|p| p.slice == i)
                .map(|p| p.observed)
                .sum()
        };
        assert!((by(0) - 1.0).abs() < 1e-9, "トークン 1000 : 2000");
        assert!((by(1) - 2.0).abs() < 1e-9);
        // 観測の無いログイン: 係数が無いので推定せず量だけ数える
        let none = allocate(&slices, &[], S + 600);
        assert!(none.coefficient.is_none());
        let usd: f64 = none.pieces.iter().map(|p| p.unestimated_usd).sum();
        let tokens: f64 = none.pieces.iter().map(|p| p.unestimated_tokens).sum();
        assert!((usd - 100.0).abs() < 1e-9);
        assert!((tokens - 2000.0).abs() < 1e-9);
        assert!(none.pieces.iter().all(|p| p.estimated == 0.0));
    }

    #[test]
    fn 今の刻みは今までに一様_日単位の行は1日に一様() {
        let w = vec![ObservedWindow {
            start: S,
            resets_at: R,
            points: vec![
                Point { t: S, percent: 0.0 },
                Point {
                    t: S + 100,
                    percent: 1.0,
                },
            ],
            observations: 1,
            ignored: 0,
        }];
        // 今 = S+200。刻み [S, S+300) は [S, S+200) に散らばる → 半分が区間・半分が推定
        let a = allocate(&[slice(S, 1.0)], &w, S + 200);
        let piece = |obs: bool| a.pieces.iter().find(|p| (p.observed > 0.0) == obs).unwrap();
        assert_eq!((piece(true).from, piece(true).to), (S, S + 100));
        assert_eq!((piece(false).from, piece(false).to), (S + 100, S + 200));
        assert!(
            (piece(false).estimated - 1.0).abs() < 1e-9,
            "係数 0.5 USD / 1%"
        );
        // 日単位の行: 区間に重なるのは 100 秒 / 1 日
        let day = UsageSlice {
            t: S - 3600,
            span: 86_400,
            usd: Some(864.0),
            tokens: 1,
        };
        let a = allocate(&[day], &w, R);
        assert_eq!(a.pieces.len(), 3, "区間の前・区間・区間の後");
        assert!((a.pieces[1].observed - 1.0).abs() < 1e-9);
        assert!((overlap_fraction(0, 100, 50, 1000) - 0.5).abs() < 1e-12);
        assert_eq!(overlap_fraction(10, 10, 0, 20), 1.0);
        assert_eq!(overlap_fraction(10, 20, 30, 40), 0.0);
    }

    /// 受け入れ 1（§9 S3）: 既知の重みで観測を合成し、キーごとの配った % が真の消費と 1 ポイント以内で
    /// 一致し、合計が観測と一致する。混雑時間帯で係数を変え、観測の欠け・窓のまたぎ・古い値を持つペイン・
    /// 下がる報告・使用量 0 の区間（tako の外）・最後の観測より後・観測の無い窓を含める
    #[test]
    fn 合成データでキーごとの誤差が1ポイント以内() {
        // 3 つのキー（プロジェクト）が 5 分ごとに決まった重みで使う。1% あたりの API 換算は
        // 1 時間ごとに 1 倍 / 2 倍を行き来する（混雑時間帯）
        let keys = 3usize;
        let t0 = S;
        let per_percent = |t: i64| {
            if (t - t0).rem_euclid(2 * H) < H {
                1.0
            } else {
                2.0
            }
        };
        let usage = |k: usize, t: i64| -> f64 {
            match k {
                0 => 0.30,
                1 => 0.12 + 0.05 * ((t - t0).rem_euclid(900) / 300) as f64,
                _ if (t - t0) > H && (t - t0) < 3 * H => 0.50,
                _ => 0.0,
            }
        };
        // 窓 1 = [t0, t0+5h)・窓 2 = [t0+5h, t0+10h)（窓 1 の解除時刻ちょうどから）。
        // t0 より前の 2 時間は観測の無い窓。窓 2 の 7.5h より後は観測が途切れる（最後の観測より後）
        let now = t0 + 2 * LEN5 - 1800;
        let observed_until = t0 + LEN5 + 2 * H + 1800;
        // 窓 1 の [3.5h, 4h) はローカルの使用が止まり、外（tako が読めない）で 2% 使う
        let local_paused = |t: i64| (t - t0) >= 3 * H + 1800 && (t - t0) < 4 * H;
        let mut slices = Vec::new();
        let mut slice_key = Vec::new();
        let mut slice_truth = Vec::new();
        let mut cumulative = [0.0f64; 2];
        let mut observations = Vec::new();
        let mut t = t0 - 2 * H;
        while t < now {
            let window = (t - t0).div_euclid(LEN5);
            if !local_paused(t) {
                for k in 0..keys {
                    let usd = usage(k, t);
                    if usd > 0.0 {
                        slices.push(UsageSlice {
                            t,
                            span: 300,
                            usd: Some(usd),
                            tokens: (usd * 1e6) as u64,
                        });
                        slice_key.push(k);
                        slice_truth.push(usd / per_percent(t));
                    }
                }
            }
            if window >= 0 {
                let w = window as usize;
                let before = cumulative[w].floor();
                if local_paused(t) {
                    cumulative[w] += 2.0 / 6.0;
                } else {
                    cumulative[w] += (0..keys).map(|k| usage(k, t) / per_percent(t)).sum::<f64>();
                }
                let after = cumulative[w].floor();
                // 1% の段を跨いだ刻みの終わりで観測（mod は値が変わったときだけ打つ）
                if after > before && t + 300 <= observed_until {
                    let resets = t0 + (window + 1) * LEN5;
                    observations.push(obs(FIVE_HOUR, after, resets, t + 299));
                    // 放置したペインの古い値（下がる報告）と、別のペインの同じ値の重複
                    if after >= 3.0 {
                        observations.push(obs(FIVE_HOUR, after - 2.0, resets, t + 330));
                        observations.push(obs(FIVE_HOUR, after, resets, t + 360));
                    }
                }
            }
            t += 300;
        }
        let (windows, _) = windows_for(&observations, "L", FIVE_HOUR, LEN5);
        assert_eq!(windows.len(), 2, "窓のまたぎ");
        assert!(windows.iter().all(|w| w.ignored > 0), "下がる報告を捨てた");
        let a = allocate(&slices, &windows, now);

        // 合計は窓ごとに観測と一致する（段へ配った分 + tako の外 = 最新の観測）
        for w in &windows {
            let inside = |from: i64, to: i64| from >= w.start && to <= w.resets_at;
            let sum: f64 = a
                .pieces
                .iter()
                .filter(|p| inside(p.from, p.to))
                .map(|p| p.observed)
                .sum::<f64>()
                + a.outside
                    .iter()
                    .filter(|o| inside(o.from, o.to))
                    .map(|o| o.percent)
                    .sum::<f64>();
            assert!(
                (sum - w.latest_percent()).abs() < 1e-9,
                "配った合計 {sum} / 観測 {}",
                w.latest_percent()
            );
        }
        // 外の 2% は「tako の外」へ入る（1% の段の誤差の範囲）
        let outside: f64 = a.outside.iter().map(|o| o.percent).sum();
        assert!((outside - 2.0).abs() <= 1.0, "tako の外 {outside}");

        // 観測の区間（各窓の開始〜最後の観測）では、キーごとに配った % が真の消費と 1 ポイント以内
        for (wi, w) in windows.iter().enumerate() {
            let (lo, hi) = (w.start, w.last_observed_at());
            for k in 0..keys {
                let got: f64 = a
                    .pieces
                    .iter()
                    .filter(|p| slice_key[p.slice] == k && p.from >= lo && p.to <= hi)
                    .map(|p| p.observed)
                    .sum();
                let truth: f64 = (0..slices.len())
                    .filter(|&i| slice_key[i] == k)
                    .map(|i| {
                        let s = &slices[i];
                        slice_truth[i] * overlap_fraction(s.t, s.t + s.span, lo, hi)
                    })
                    .sum();
                assert!(
                    (got - truth).abs() <= 1.0,
                    "窓 {wi} のキー {k}: 配った {got:.3} / 真 {truth:.3}"
                );
            }
        }

        // 観測の区間の外（観測の無い窓・最後の観測より後）は推定 = 使用量 ÷ 係数（観測からは配らない）
        let c = a.coefficient.expect("係数");
        let per = c.usd_per_percent.expect("API 換算の係数");
        assert!(c.recent && c.intervals > 10);
        let mut before_window = 0.0;
        let mut tail = 0.0;
        for p in &a.pieces {
            let s = &slices[p.slice];
            let frac = (p.to - p.from) as f64 / s.span as f64;
            if p.to <= t0 || p.from >= observed_until {
                assert_eq!(p.observed, 0.0);
                assert!((p.estimated - s.usd.unwrap() * frac / per).abs() < 1e-9);
                if p.to <= t0 {
                    before_window += p.estimated;
                } else {
                    tail += p.estimated;
                }
            }
        }
        assert!(before_window > 0.0, "観測の無い窓は推定");
        assert!(tail > 0.0, "最後の観測より後は推定");
    }
}
