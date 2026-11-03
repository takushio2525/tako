//! 順番待ちの終わりの見込み（純関数。FR-2.43.14 / Issue #1996。エピック #1992 の S4）
//!
//! 設計の正は `.agent/plans/2026-10-resource-manager.md`（§6・§4.10）。材料は 2 つ:
//!
//! - **過去の所要時間**（[`duration_history`]）: `<data_dir>/resources/history.jsonl` の同じ鍵の行。
//!   1 回でも記録があれば（= 同じコマンドの 2 回目から）「残り = 中央値 − 経過」。**こちらを優先する**
//!   （cargo のユニットは大きさが偏り、最後の本体と link が重いので `N/M` の線形は楽観に倒れる）
//! - **cargo の `N/M`**（[`ProgressFilter`]）: wrapper の stderr が端末でないときだけ wrapper が
//!   `CARGO_TERM_PROGRESS_WHEN=always` を足し、`Building [...] N/M` の行を自分で読んで**素通しから外す**
//!   （AI の出力に進みの行を混ぜない）。記録が無く `N/M ≥ 0.2` のときだけ線形で見込む
//!
//! 表示は [`Eta::text`]（「あと約 3 分（前回 7 分）」/「前回より長くかかっている（経過 9 分・前回 7 分）」/
//! 「経過 4 分（見込みなし）」）、待ちの開始の見込みは [`start_after`]（前に並ぶ全件と、枠を持つ実行中の
//! 全件に見込みがあるときだけ）

use serde::{Deserialize, Serialize};

use super::{HistoryKeys, HistoryRow, HISTORY_EXACT_RUNS};

/// cargo の進み（`Building [...] N/M` の `N` / `M`）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Progress {
    pub done: u32,
    pub total: u32,
}

/// `N/M` だけで見込みを出す下限（記録が無いとき。これ未満は「見込みなし」）
pub const PROGRESS_MIN_RATIO: f64 = 0.2;

// ===== 過去の所要時間 =====

/// 同じ鍵の過去の所要時間
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DurationHistory {
    /// 見込みの所要時間（直近 [`HISTORY_EXACT_RUNS`] 回の中央値。秒）
    pub expected_s: u64,
    /// 前回の所要時間（秒）
    pub previous_s: u64,
    /// 使った記録の数
    pub runs: u32,
}

/// 同じ鍵の所要時間（純関数）: 直近 [`HISTORY_EXACT_RUNS`] 回（成功した回 = 終了コード 0 を優先。
/// 1 回も無ければ終了コードを問わない）の中央値と前回。`history` は古い順。記録が無ければ `None`
pub fn duration_history(
    keys: Option<&HistoryKeys>,
    history: &[HistoryRow],
) -> Option<DurationHistory> {
    let keys = keys.filter(|k| !k.key.is_empty())?;
    let same: Vec<&HistoryRow> = history
        .iter()
        .rev()
        .filter(|r| r.key == keys.key && r.ran_s > 0)
        .collect();
    let ok: Vec<&HistoryRow> = same
        .iter()
        .copied()
        .filter(|r| r.exit_code == Some(0))
        .take(HISTORY_EXACT_RUNS)
        .collect();
    let rows = if ok.is_empty() {
        same.into_iter().take(HISTORY_EXACT_RUNS).collect()
    } else {
        ok
    };
    let previous_s = rows.first()?.ran_s;
    let mut secs: Vec<u64> = rows.iter().map(|r| r.ran_s).collect();
    secs.sort_unstable();
    let n = secs.len();
    let expected_s = if n % 2 == 1 {
        secs[n / 2]
    } else {
        (secs[n / 2 - 1] + secs[n / 2]).div_ceil(2)
    };
    Some(DurationHistory {
        expected_s,
        previous_s,
        runs: n as u32,
    })
}

// ===== 見込み =====

/// 見込みの出どころ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EtaFrom {
    /// 過去の所要時間
    History,
    /// cargo の `N/M` の線形
    Progress,
}

/// 実行中の 1 件の終わりの見込み
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eta {
    /// あと `remaining_s` 秒
    Remaining {
        remaining_s: u64,
        from: EtaFrom,
        /// 前回の所要時間（過去の記録から出したときだけ）
        previous_s: Option<u64>,
        progress: Option<Progress>,
    },
    /// 過去の記録の見込みを超えた
    Overrun {
        ran_s: u64,
        previous_s: u64,
        progress: Option<Progress>,
    },
    /// 材料が無い
    Unknown {
        ran_s: u64,
        progress: Option<Progress>,
    },
}

/// 終わりの見込み（純関数）: 過去の記録 → `N/M` の線形（`N/M ≥ 0.2`）→ 見込みなし
pub fn eta(ran_s: u64, progress: Option<Progress>, history: Option<&DurationHistory>) -> Eta {
    if let Some(h) = history {
        if ran_s < h.expected_s {
            return Eta::Remaining {
                remaining_s: h.expected_s - ran_s,
                from: EtaFrom::History,
                previous_s: Some(h.previous_s),
                progress,
            };
        }
        return Eta::Overrun {
            ran_s,
            previous_s: h.previous_s,
            progress,
        };
    }
    if let Some(p) = progress.filter(|p| p.total > 0 && p.done > 0) {
        let ratio = f64::from(p.done) / f64::from(p.total);
        if ratio >= PROGRESS_MIN_RATIO && p.done < p.total {
            let total_s = (ran_s as f64 / ratio).ceil() as u64;
            return Eta::Remaining {
                remaining_s: total_s.saturating_sub(ran_s).max(1),
                from: EtaFrom::Progress,
                previous_s: None,
                progress,
            };
        }
    }
    Eta::Unknown { ran_s, progress }
}

/// 秒 → 「N 秒」/「N 分」（60 秒以上は分へ丸める。見込みの表示用）
pub fn approx(secs: u64) -> String {
    if secs < 60 {
        format!("{secs} 秒")
    } else {
        format!("{} 分", (secs + 30) / 60)
    }
}

impl Eta {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Remaining { .. } => "remaining",
            Self::Overrun { .. } => "overrun",
            Self::Unknown { .. } => "unknown",
        }
    }

    /// 残りの秒数（見込みがあるときだけ）
    pub fn remaining_s(&self) -> Option<u64> {
        match self {
            Self::Remaining { remaining_s, .. } => Some(*remaining_s),
            _ => None,
        }
    }

    /// 人が読む 1 文
    pub fn text(&self) -> String {
        match self {
            Self::Remaining {
                remaining_s,
                from: EtaFrom::History,
                previous_s,
                ..
            } => match previous_s {
                Some(prev) => format!("あと約 {}（前回 {}）", approx(*remaining_s), approx(*prev)),
                None => format!("あと約 {}", approx(*remaining_s)),
            },
            Self::Remaining {
                remaining_s,
                from: EtaFrom::Progress,
                progress,
                ..
            } => {
                let p = progress.unwrap_or_default();
                format!(
                    "あと約 {}（進み {}/{} からの見込み）",
                    approx(*remaining_s),
                    p.done,
                    p.total
                )
            }
            Self::Overrun {
                ran_s, previous_s, ..
            } => format!(
                "前回より長くかかっている（経過 {}・前回 {}）",
                approx(*ran_s),
                approx(*previous_s)
            ),
            Self::Unknown { ran_s, .. } => format!("経過 {}（見込みなし）", approx(*ran_s)),
        }
    }
}

// ===== 待ちの開始の見込み =====

/// 待ちの 1 件が始まるまでの見込み（純関数。秒）。
///
/// 枠 `slots` 本を、枠を持つ実行中（停滞を除く）の残り `running` と前に並ぶ件の所要時間 `ahead`
/// （並びの順）で順に埋める。どれか 1 件でも見込みが無ければ `None`（= 順位だけを出す）。
/// `floor` は自分と同じ target dir の実行中の残り（その dir が空くまで始まらない）。
/// メモリの条件は見ない（見込みは「約」）
pub fn start_after(
    slots: u32,
    running: &[Option<u64>],
    ahead: &[Option<u64>],
    floor: Option<u64>,
) -> Option<u64> {
    let slots = slots.max(1) as usize;
    let mut running: Vec<u64> = running.iter().copied().collect::<Option<Vec<u64>>>()?;
    let ahead: Vec<u64> = ahead.iter().copied().collect::<Option<Vec<u64>>>()?;
    running.sort_unstable();
    // 枠が空く時刻: 実行中が枠より少なければ空いている枠は 0、多ければ「枠の数を割るまで終わった時刻」から
    let mut free: Vec<u64> = if running.len() < slots {
        let mut v = vec![0; slots - running.len()];
        v.extend(running);
        v
    } else {
        running[running.len() - slots..].to_vec()
    };
    for d in ahead {
        free.sort_unstable();
        free[0] = free[0].saturating_add(d);
    }
    let start = free.into_iter().min().unwrap_or(0);
    Some(floor.map_or(start, |f| start.max(f)))
}

/// 待ちの開始の見込みの 1 文
pub fn start_text(secs: u64) -> String {
    if secs == 0 {
        "空き次第すぐ開始の見込み".into()
    } else {
        format!("開始まで約 {}", approx(secs))
    }
}

// ===== cargo の進みの行の読み取り =====

/// 進みの行の長さの上限（`CARGO_TERM_PROGRESS_WIDTH=80` の行 + 色の制御列。超えたら進みの行ではない）
const BAR_MAX_BYTES: usize = 256;

/// stderr の素通しから cargo の進みの行（`    Building [=====>   ] 3/9: name…\r`）を外して読む。
///
/// cargo は端末でない出力へ進みを強制すると、行を `\r` で終えて次の行で上書きする（2026-11-03 の実測。
/// 普通のメッセージは `\n` で終わる）。ここは `\r` で終わる区切りのうち進みの形（前置きの空白・語・
/// ` [` …`] `）のものと空白だけのもの（消し）を落とし、それ以外は 1 バイトも変えずに返す。
/// 進みの形になりえないと分かった時点でその行は溜めずに流す（`\n` を待たない = 途中の行も遅らせない）
#[derive(Debug, Default)]
pub struct ProgressFilter {
    /// 進みの行かもしれない区切りの溜め
    pending: Vec<u8>,
    /// いまの区切りは進みの行ではないと決まった（区切りの終わりまでそのまま流す）
    passthrough: bool,
    /// 最後に読んだ `Building` の `N/M`
    pub last: Option<Progress>,
    /// 落とした進みの行の数
    pub dropped: u64,
}

impl ProgressFilter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 読んだ分を渡し、素通しへ書く分を返す
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(chunk.len());
        for &b in chunk {
            if self.passthrough {
                out.push(b);
                if b == b'\r' || b == b'\n' {
                    self.passthrough = false;
                }
                continue;
            }
            match b {
                b'\n' => {
                    out.append(&mut self.pending);
                    out.push(b);
                }
                b'\r' => {
                    match classify(&self.pending) {
                        Segment::Bar(p) => {
                            if p.is_some() {
                                self.last = p;
                            }
                            self.dropped += 1;
                        }
                        Segment::Blank => {}
                        Segment::Text => {
                            out.append(&mut self.pending);
                            out.push(b);
                        }
                    }
                    self.pending.clear();
                }
                _ => {
                    self.pending.push(b);
                    if !could_be_bar(&self.pending) {
                        out.append(&mut self.pending);
                        self.passthrough = true;
                    }
                }
            }
        }
        out
    }

    /// 終わり（子の stderr が閉じた）: 溜めていた分を返す
    pub fn finish(&mut self) -> Vec<u8> {
        self.passthrough = false;
        std::mem::take(&mut self.pending)
    }
}

enum Segment {
    /// 進みの行（`Building` なら `N/M`）
    Bar(Option<Progress>),
    /// 空白だけ（前の行の消し）。空の区切り（`\r` だけ）は含めない
    Blank,
    Text,
}

/// 色の制御列（`ESC [ … 文字`）を外した見える部分。終わっていない制御列は `incomplete` = true
fn visible(bytes: &[u8]) -> (Vec<u8>, bool) {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b {
            if i + 1 >= bytes.len() {
                return (out, true);
            }
            if bytes[i + 1] != b'[' {
                // CSI でない ESC は進みの行に出ない
                out.push(0x1b);
                i += 1;
                continue;
            }
            let mut j = i + 2;
            while j < bytes.len() && !(0x40..=0x7e).contains(&bytes[j]) {
                j += 1;
            }
            if j >= bytes.len() {
                return (out, true);
            }
            i = j + 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    (out, false)
}

/// 進みの行の形を先頭から照らす。`Ok(Some(pos))` = `] ` の直後まで揃った（その位置）、
/// `Ok(None)` = ここまでは形どおり（続きしだい）、`Err(())` = 形から外れた
fn match_bar(v: &[u8]) -> Result<Option<usize>, ()> {
    let mut i = 0;
    while i < v.len() && v[i] == b' ' {
        i += 1;
    }
    if i == v.len() {
        return Ok(None);
    }
    let word = i;
    while i < v.len() && v[i].is_ascii_alphabetic() {
        i += 1;
    }
    if i == v.len() {
        return Ok(None);
    }
    if i == word || v[i] != b' ' {
        return Err(());
    }
    i += 1;
    if i == v.len() {
        return Ok(None);
    }
    if v[i] != b'[' {
        return Err(());
    }
    i += 1;
    while i < v.len() && matches!(v[i], b'=' | b'>' | b' ') {
        i += 1;
    }
    if i == v.len() {
        return Ok(None);
    }
    if v[i] != b']' {
        return Err(());
    }
    i += 1;
    if i == v.len() {
        return Ok(None);
    }
    if v[i] != b' ' {
        return Err(());
    }
    Ok(Some(i + 1))
}

/// 溜めている区切りが進みの行（か消し）になりうるか
fn could_be_bar(pending: &[u8]) -> bool {
    if pending.len() > BAR_MAX_BYTES {
        return false;
    }
    let (v, _) = visible(pending);
    match_bar(&v).is_ok()
}

fn classify(pending: &[u8]) -> Segment {
    if pending.is_empty() || pending.len() > BAR_MAX_BYTES {
        return Segment::Text;
    }
    let (v, incomplete) = visible(pending);
    if incomplete {
        return Segment::Text;
    }
    if v.iter().all(|b| *b == b' ') {
        return Segment::Blank;
    }
    let Ok(Some(rest_at)) = match_bar(&v) else {
        return Segment::Text;
    };
    let label = String::from_utf8_lossy(&v[..rest_at]);
    let rest = String::from_utf8_lossy(&v[rest_at..]);
    let progress = (label.trim_start().starts_with("Building "))
        .then(|| parse_counts(&rest))
        .flatten();
    Segment::Bar(progress)
}

/// `3/9: name…` / `3/9` の `N/M`
fn parse_counts(rest: &str) -> Option<Progress> {
    let head = rest.split([':', ' ']).next()?;
    let (n, m) = head.split_once('/')?;
    let done: u32 = n.parse().ok()?;
    let total: u32 = m.parse().ok()?;
    (total > 0 && done <= total).then_some(Progress { done, total })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::JobKind;

    fn row(key: &str, ran_s: u64, exit: i32) -> HistoryRow {
        HistoryRow {
            key: key.into(),
            kind: JobKind::Build,
            peak_mb: 100,
            ran_s,
            exit_code: Some(exit),
            ..HistoryRow::default()
        }
    }

    fn keys(key: &str) -> HistoryKeys {
        HistoryKeys {
            key: key.into(),
            shape: None,
            name: "cargo build".into(),
        }
    }

    #[test]
    fn 同じ鍵の記録が1回あれば見込みが出る() {
        assert_eq!(duration_history(Some(&keys("a")), &[]), None, "記録が無い");
        let h = duration_history(Some(&keys("a")), &[row("b", 99, 0), row("a", 420, 0)]).unwrap();
        assert_eq!(
            h,
            DurationHistory {
                expected_s: 420,
                previous_s: 420,
                runs: 1
            }
        );
        assert_eq!(
            duration_history(None, &[row("a", 1, 0)]),
            None,
            "鍵の無い旧い wrapper"
        );
    }

    #[test]
    fn 中央値は直近5回で成功した回を優先する() {
        // 古い順。失敗で早く終わった回（10 秒）は成功した回があれば使わない
        let rows: Vec<HistoryRow> = [
            (1000, 0),
            (300, 0),
            (10, 101),
            (400, 0),
            (500, 0),
            (600, 0),
            (700, 0),
        ]
        .iter()
        .map(|(s, e)| row("a", *s, *e))
        .collect();
        let h = duration_history(Some(&keys("a")), &rows).unwrap();
        // 成功した直近 5 回 = 700, 600, 500, 400, 300 → 中央値 500。前回 = 700
        assert_eq!((h.expected_s, h.previous_s, h.runs), (500, 700, 5));
        // 失敗しかなければ失敗した回で
        let failed = [row("f", 30, 1), row("f", 50, 1)];
        let h = duration_history(Some(&keys("f")), &failed).unwrap();
        assert_eq!((h.expected_s, h.previous_s, h.runs), (40, 50, 2));
    }

    #[test]
    fn 見込みは記録を優先し無ければ進みの線形() {
        let h = DurationHistory {
            expected_s: 420,
            previous_s: 400,
            runs: 3,
        };
        let p = Some(Progress { done: 8, total: 10 });
        // 記録があれば進みが楽観でも記録を使う
        let e = eta(60, p, Some(&h));
        assert_eq!(e.remaining_s(), Some(360));
        assert_eq!(e.text(), "あと約 6 分（前回 7 分）");
        // 記録を超えた
        let e = eta(540, p, Some(&h));
        assert_eq!(e.code(), "overrun");
        assert_eq!(e.text(), "前回より長くかかっている（経過 9 分・前回 7 分）");
        // 記録が無ければ進みの線形（2/10 = 0.2 ちょうどから）
        let e = eta(60, Some(Progress { done: 2, total: 10 }), None);
        assert_eq!(e.remaining_s(), Some(240));
        assert_eq!(e.text(), "あと約 4 分（進み 2/10 からの見込み）");
        // 0.2 未満・進みなし・終わった進み（N = M）は見込みなし
        assert_eq!(
            eta(60, Some(Progress { done: 1, total: 10 }), None).code(),
            "unknown"
        );
        assert_eq!(eta(240, None, None).text(), "経過 4 分（見込みなし）");
        assert_eq!(
            eta(30, Some(Progress { done: 9, total: 9 }), None).code(),
            "unknown"
        );
        assert_eq!(eta(30, None, None).text(), "経過 30 秒（見込みなし）");
    }

    #[test]
    fn 開始の見込みは前の全件と実行中の全件に見込みがあるときだけ() {
        // 枠 2・実行中 1 件（残り 100）= 空いている枠がある → すぐ
        assert_eq!(start_after(2, &[Some(100)], &[], None), Some(0));
        // 枠 1・実行中（残り 100）・前に 1 件（200）→ 300
        assert_eq!(start_after(1, &[Some(100)], &[Some(200)], None), Some(300));
        // 枠 2・実行中 2 件（100 / 50）・前に 2 件（30 / 500）: 1 件目は 50 から（→ 80）・2 件目は 80 から
        // （→ 580）、自分は残りの枠が空く 100 から
        assert_eq!(
            start_after(2, &[Some(100), Some(50)], &[Some(30), Some(500)], None),
            Some(100)
        );
        // 実行中が枠より多い（停滞の戻り・外の処理）: 枠 1・実行中 2 件 → 2 件目が終わってから
        assert_eq!(start_after(1, &[Some(10), Some(40)], &[], None), Some(40));
        // どれかに見込みが無ければ順位だけ
        assert_eq!(start_after(1, &[None], &[], None), None);
        assert_eq!(start_after(3, &[Some(1)], &[None], None), None);
        // 同じ target dir の実行中が居れば、その残りより前には始まらない
        assert_eq!(start_after(3, &[Some(500)], &[], Some(500)), Some(500));
        assert_eq!(start_text(0), "空き次第すぐ開始の見込み");
        assert_eq!(start_text(150), "開始まで約 3 分");
    }

    /// 2026-11-03 の実測（`CARGO_TERM_PROGRESS_WHEN=always`・`WIDTH=80`・端末でない stderr）の形
    const MEASURED: &[u8] = b"warning: a `-j` argument was passed\n   Compiling sleep_c v0.1.0 (/x/c)\n    Building [                            ] 0/9: sleep_c(build.rs), sleep_...\r    Building [========>                   ] 3/9: sleep_a(build), sleep_c(b...\r    Building [========================>    ] 8/9: sleep_b                     \r    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.17s\n";

    #[test]
    fn 進みの行だけを外してnmを読む() {
        let mut f = ProgressFilter::new();
        let mut out = f.feed(MEASURED);
        out.extend(f.finish());
        let text = String::from_utf8(out).unwrap();
        assert_eq!(
            text,
            "warning: a `-j` argument was passed\n   Compiling sleep_c v0.1.0 (/x/c)\n    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.17s\n"
        );
        assert_eq!(f.last, Some(Progress { done: 8, total: 9 }));
        assert_eq!(f.dropped, 3);
    }

    #[test]
    fn 細切れに届いても同じ() {
        let mut f = ProgressFilter::new();
        let mut out = Vec::new();
        for b in MEASURED {
            out.extend(f.feed(std::slice::from_ref(b)));
        }
        out.extend(f.finish());
        let mut whole = ProgressFilter::new();
        let mut expect = whole.feed(MEASURED);
        expect.extend(whole.finish());
        assert_eq!(out, expect);
        assert_eq!(f.last, Some(Progress { done: 8, total: 9 }));
    }

    #[test]
    fn 進みの形でない行は1バイトも変えず溜めない() {
        // CRLF・途中の行（改行なし）・角括弧を含む普通の行・空白だけの \n 行・色つきの進み
        let input: &[u8] = b"a\r\n\r\n[x] not a bar\r  \nwarn [==] x\nprompt> ";
        let mut f = ProgressFilter::new();
        let out = f.feed(input);
        // 途中の行（`prompt> `）は改行を待たずに流れる（溜めていない）
        assert_eq!(out, input);
        assert!(f.finish().is_empty());
        assert_eq!(f.dropped, 0);
        // 色の制御列つきの進みも外す
        let mut f = ProgressFilter::new();
        let colored = b"\x1b[1m\x1b[36m    Building\x1b[0m [=>  ] 1/4: a\rok\n";
        assert_eq!(f.feed(colored), b"ok\n");
        assert_eq!(f.last, Some(Progress { done: 1, total: 4 }));
        // Building でない進み（Downloading）も外すが N/M は読まない
        let mut f = ProgressFilter::new();
        assert_eq!(
            f.feed(b"  Downloading [=> ] 2/10: serde\rdone\n"),
            b"done\n"
        );
        assert_eq!(f.last, None);
        assert_eq!(f.dropped, 1);
    }

    #[test]
    fn 長すぎる区切りは進みの行とみなさない() {
        let mut long = b"    Building [".to_vec();
        long.extend(std::iter::repeat_n(b'=', BAR_MAX_BYTES));
        long.extend(b"] 1/2\r");
        let mut f = ProgressFilter::new();
        let mut out = f.feed(&long);
        out.extend(f.finish());
        assert_eq!(out, long, "上限を超えたら流す");
        assert_eq!(f.last, None);
    }
}
