//! visual-test `tasks-input`（#2016）の送信の間の判定（#2711 / #2721）。
//!
//! 押下 1 回の区間（`any.update(… dispatch_event(MouseDown / MouseUp))`）は、visual-test の
//! ビルドでは**同期の描き直し**を含む（`gpui/test-support` の `App::flush_effects` が update の
//! たびに dirty な窓をその場で描く。debug で約 48 ms。本番のビルドでは押下の update の中で描かない）。
//! 製品のハンドラ（`dispatch_event` の内側）は約 0.2 ms なので、区間全体の差で見ると、機が混んだ回の
//! 描き直しの揺れが判定を決めてしまう（#2721 の実測: load 17 の回で空押しの幅 269 ms・差 70.5 ms で落ちた）。
//! そこで次の 3 つで見る:
//!
//! 1. **時間に依らない確かめ**: 押下が返った時点で送信中に居て、まだ一覧に載っていない
//!    （ハンドラが往復を待たない = #2016 の核心。旧実装は返った時点でもう一覧に載っている）
//! 2. **主の判定**: 押下の配送そのもの（`dispatch_event` の内側）の「送信 − 直前の空押し」の中央値が
//!    [`LAG_MS`] 未満（測る区間が約 0.2 ms なので、機の混みの割り込みを受けにくい）
//! 3. **全体の判定**: 空押しの区間全体の幅が [`LAG_MS`] 未満の静かな回だけ、区間全体の差の中央値も
//!    [`LAG_MS`] 未満か（重い処理がハンドラの外 = 同じ update の後段へ移った回帰はここで拾う）。
//!    揺れた回は「未実測」と名指しして数えない
//!
//! A/B: `TAKO_2721_LEGACY=1` = #2711 の形（区間全体の差の中央値だけ・揺れの門なし）。
//! `TAKO_2711_LEGACY=tasks-input` = さらに前の形（区間全体・基準の空押しは 1 回目に 1 度だけ）

/// 送信が押下に足してよい時間（1 フレームの半分）
pub(super) const LAG_MS: f64 = 8.0;

/// 判定の形（A/B）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LagForm {
    /// #2721: 時間に依らない確かめ + 配送そのものの差 + 静かな回だけ区間全体の差
    Current,
    /// #2711（`TAKO_2721_LEGACY=1`）: 送信ごとに直前の空押しと組にした区間全体の差の中央値
    Legacy2721,
    /// #2711 の前（`TAKO_2711_LEGACY=tasks-input`）: 送信の区間全体の中央値 − 1 回目の空押し
    Legacy2711,
}

/// 送信 5 回ぶんの観測（添字 k が k 回目の送信。空押しは送信の直前に採る）
#[derive(Clone, Debug, Default)]
pub(super) struct Observed {
    /// 送信の押下の区間全体（同期の描き直しを含む）
    pub send_total: Vec<f64>,
    /// 空押しの区間全体（旧 #2711 の形は 1 回目だけ）
    pub idle_total: Vec<f64>,
    /// 送信の押下の配送そのもの（`dispatch_event` の内側）
    pub send_dispatch: Vec<f64>,
    /// 空押しの配送そのもの
    pub idle_dispatch: Vec<f64>,
    /// 押下が返った時点の（送信中に居るか, 一覧のやりとりの増分）
    pub at_return: Vec<(bool, usize)>,
}

/// 判定の結果（値は `TAKO_VISUAL_PIXEL` と落ちたときの文言へそのまま出す）
#[derive(Clone, Debug)]
pub(super) struct Verdict {
    pub form: LagForm,
    /// 時間に依らない確かめが通ったか
    pub state_ok: bool,
    /// 配送そのものの差の中央値（旧の形では見ないので NaN）
    pub handler_cost_ms: f64,
    /// 区間全体の差（今と #2711 は組の差の中央値、その前は中央値 − 1 回目の空押し）
    pub total_cost_ms: f64,
    /// 空押しの区間全体の幅（最大 − 最小）
    pub idle_spread_ms: f64,
    /// 区間全体でも判定したか（今の形で揺れた回は false = 未実測）
    pub total_judged: bool,
    /// 時間の判定が通ったか（主の判定と、判定した回の全体）
    pub time_ok: bool,
}

fn median(v: &[f64]) -> f64 {
    let mut v = v.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v.get(v.len() / 2).copied().unwrap_or(f64::NAN)
}

/// 閾値の内側か（NaN = 観測が欠けた回は通さない）
fn below(v: f64) -> bool {
    v.is_finite() && v < LAG_MS
}

/// 組にした差（送信 − 直前の空押し）の中央値。組が 1 つも無ければ NaN
fn paired_median(send: &[f64], idle: &[f64]) -> f64 {
    let diffs: Vec<f64> = send.iter().zip(idle).map(|(s, i)| s - i).collect();
    median(&diffs)
}

pub(super) fn judge(o: &Observed, form: LagForm) -> Verdict {
    let idle_spread_ms = match o.idle_total.len() {
        0 => f64::NAN,
        _ => {
            let max = o.idle_total.iter().copied().fold(f64::MIN, f64::max);
            let min = o.idle_total.iter().copied().fold(f64::MAX, f64::min);
            max - min
        }
    };
    // 旧の形の確かめは「返った時点で送信中か、もう一覧に載っている」（往復を握っても通る）
    let lenient = !o.at_return.is_empty() && o.at_return.iter().all(|&(s, n)| s || n > 0);
    match form {
        LagForm::Current => {
            let state_ok = !o.at_return.is_empty() && o.at_return.iter().all(|&(s, n)| s && n == 0);
            let handler_cost_ms = paired_median(&o.send_dispatch, &o.idle_dispatch);
            let total_cost_ms = paired_median(&o.send_total, &o.idle_total);
            let total_judged = below(idle_spread_ms);
            let time_ok = below(handler_cost_ms) && (!total_judged || below(total_cost_ms));
            Verdict {
                form,
                state_ok,
                handler_cost_ms,
                total_cost_ms,
                idle_spread_ms,
                total_judged,
                time_ok,
            }
        }
        LagForm::Legacy2721 => {
            let total_cost_ms = paired_median(&o.send_total, &o.idle_total);
            Verdict {
                form,
                state_ok: lenient,
                handler_cost_ms: f64::NAN,
                total_cost_ms,
                idle_spread_ms,
                total_judged: true,
                time_ok: below(total_cost_ms),
            }
        }
        LagForm::Legacy2711 => {
            let baseline = o.idle_total.first().copied().unwrap_or(f64::NAN);
            let total_cost_ms = median(&o.send_total) - baseline;
            Verdict {
                form,
                state_ok: lenient,
                handler_cost_ms: f64::NAN,
                total_cost_ms,
                idle_spread_ms,
                total_judged: true,
                time_ok: below(total_cost_ms),
            }
        }
    }
}

impl Verdict {
    /// 時間に依らない確かめの規則（落ちたときの文言）
    pub(super) fn state_rule(&self) -> &'static str {
        match self.form {
            LagForm::Current => {
                "押下が返った時点で送信中に居て、まだ一覧に載っていない（ハンドラが往復を待たない）"
            }
            _ => "押下が返った時点で送信中か、もう一覧に載っている（旧の形）",
        }
    }

    /// 時間の判定を 1 行で（何と何を比べたか・どこで落ちたか・全体を見たか）
    pub(super) fn time_text(&self) -> String {
        match self.form {
            LagForm::Current => {
                let failed = if !below(self.handler_cost_ms) {
                    "【配送そのもので超えた】"
                } else if self.total_judged && !below(self.total_cost_ms) {
                    "【区間全体で超えた = ハンドラの外の重さ】"
                } else {
                    ""
                };
                let total = if self.total_judged {
                    format!("区間全体 {:.1} ms", self.total_cost_ms)
                } else {
                    format!(
                        "区間全体は未実測（空押しの幅 {:.1} ms が {LAG_MS} ms 以上 = 機が混んでいた。差 {:.1} ms は数えない）",
                        self.idle_spread_ms, self.total_cost_ms
                    )
                };
                format!(
                    "{failed}送信のハンドラが押下に足す時間 {:.1} ms = 配送そのものの送信 − 直前の空押しの中央値・{total}",
                    self.handler_cost_ms
                )
            }
            LagForm::Legacy2721 => format!(
                "送信が押下に足す時間 {:.1} ms = 送信ごとに直前の空押しとの差の中央値（旧 #2721）",
                self.total_cost_ms
            ),
            LagForm::Legacy2711 => format!(
                "送信が押下に足す時間 {:.1} ms = 送信の中央値 − 1 回目の空押し（旧 #2711）",
                self.total_cost_ms
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 落ちた回（#2721 の本文。load 17.04）の区間全体。配送そのものは採っていなかったので、
    /// 段 1 の `replay`（同じ揺れの再生）で採れた値（送信 0.2 / 空押し 0.2）を置く
    fn crowded() -> Observed {
        Observed {
            send_total: vec![130.4, 259.7, 194.9, 333.5, 233.2],
            idle_total: vec![56.5, 303.4, 325.9, 226.8, 162.7],
            send_dispatch: vec![0.2; 5],
            idle_dispatch: vec![0.2; 5],
            at_return: vec![(true, 0); 5],
        }
    }

    /// 静かな機の 1 回（段 1 の注入なし #1）
    fn quiet() -> Observed {
        Observed {
            send_total: vec![48.0, 48.1, 48.7, 48.9, 48.7],
            idle_total: vec![47.1, 48.5, 48.3, 47.9, 48.4],
            send_dispatch: vec![0.2; 5],
            idle_dispatch: vec![0.2, 0.1, 0.1, 0.1, 0.1],
            at_return: vec![(true, 0); 5],
        }
    }

    #[test]
    fn 混んだ回は全体を未実測にして配送そのもので通し旧の形は落ちる() {
        let v = judge(&crowded(), LagForm::Current);
        assert!(v.state_ok && v.time_ok, "{v:?}");
        assert!(
            !v.total_judged,
            "空押しの幅 {} ms で全体を判定した",
            v.idle_spread_ms
        );
        assert!(v.time_text().contains("未実測"), "{}", v.time_text());
        let old = judge(&crowded(), LagForm::Legacy2721);
        assert!(!old.time_ok, "旧の形が落ちない: {old:?}");
        assert!(
            (old.total_cost_ms - 70.5).abs() < 0.05,
            "{}",
            old.total_cost_ms
        );
    }

    #[test]
    fn 静かな回は区間全体も判定して通る() {
        let v = judge(&quiet(), LagForm::Current);
        assert!(v.state_ok && v.time_ok && v.total_judged, "{v:?}");
        assert!(v.handler_cost_ms < 1.0 && v.total_cost_ms < 2.0, "{v:?}");
    }

    #[test]
    fn 往復を握る旧実装は状態と配送そのもので落ちる() {
        // 段 1 の TAKO_2016_LEGACY=1 + replay（配送そのもの 61.9〜106.0 ms・返った時点で一覧に載る）
        let mut o = crowded();
        o.send_dispatch = vec![65.4, 106.0, 71.4, 61.9, 67.4];
        o.at_return = vec![(false, 1); 5];
        let v = judge(&o, LagForm::Current);
        assert!(!v.state_ok, "返った時点で一覧に載っているのに通った");
        assert!(v
            .state_rule()
            .contains("送信中に居て、まだ一覧に載っていない"));
        assert!(
            !v.time_ok && v.time_text().contains("配送そのもので超えた"),
            "{}",
            v.time_text()
        );
        // 旧の形の確かめは往復を握っても通る（だから時間だけが頼りだった）
        assert!(judge(&o, LagForm::Legacy2721).state_ok);
    }

    #[test]
    fn ハンドラの中の重さは混んだ回でも落ちる() {
        let mut o = crowded();
        o.send_dispatch = vec![20.3; 5];
        let v = judge(&o, LagForm::Current);
        assert!(
            !v.time_ok && v.time_text().contains("配送そのもので超えた"),
            "{v:?}"
        );
    }

    #[test]
    fn ハンドラの外の重さは静かな回の全体で落ち混んだ回は未実測() {
        let mut o = quiet();
        o.send_total = o.send_total.iter().map(|v| v + 20.0).collect();
        let v = judge(&o, LagForm::Current);
        assert!(v.total_judged && !v.time_ok, "{v:?}");
        assert!(
            v.time_text().contains("区間全体で超えた"),
            "{}",
            v.time_text()
        );
        // 混んだ回では見えない（未実測として名指す = 通しても数えない）
        let mut c = crowded();
        c.send_total = c.send_total.iter().map(|v| v + 20.0).collect();
        let v = judge(&c, LagForm::Current);
        assert!(!v.total_judged && v.time_ok, "{v:?}");
    }

    #[test]
    fn 配送の区間へ時々割り込まれても中央値が吸収する() {
        let mut o = quiet();
        o.send_dispatch[1] += 100.0;
        o.idle_dispatch[3] += 100.0;
        let v = judge(&o, LagForm::Current);
        assert!(v.time_ok && v.handler_cost_ms < 1.0, "{v:?}");
    }

    #[test]
    fn 観測が欠けたら通さない() {
        let v = judge(&Observed::default(), LagForm::Current);
        assert!(!v.state_ok && !v.time_ok, "{v:?}");
        let mut o = quiet();
        o.idle_dispatch.clear();
        assert!(!judge(&o, LagForm::Current).time_ok);
    }

    #[test]
    fn 旧2711の形は1回目の空押しだけを基準にする() {
        let o = Observed {
            send_total: vec![52.5, 244.9, 129.9, 138.5, 146.6],
            idle_total: vec![50.7],
            at_return: vec![(true, 0); 5],
            ..Observed::default()
        };
        let v = judge(&o, LagForm::Legacy2711);
        assert!(!v.time_ok && (v.total_cost_ms - 87.8).abs() < 0.05, "{v:?}");
    }

    /// 番犬: tasks-input が押下を `click_timed`（区間全体と配送そのもの）で測り、判定をこの規則に通すか。
    /// 落ちた形の名前を返す（コメント行は見ない）
    fn bypasses(source: &str) -> Vec<&'static str> {
        let code: Vec<&str> = source
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect();
        let code = code.join("\n");
        let mut out = Vec::new();
        // 定義 1 + 空押し 1 + 送信 1
        if code.matches("click_timed(").count() < 3 {
            out.push("空押しと送信を click_timed で測っていない（配送そのものが採れない）");
        }
        if !code.contains("judge(&observed, form)") {
            out.push("判定を send_lag_2721::judge に通していない");
        }
        if !code.contains("check(\n        lag.state_ok") && !code.contains("check(lag.state_ok") {
            out.push("押下が返った時点の状態の確かめを check していない");
        }
        if code.contains("< 8.0") {
            out.push("時間を自前で 8 ms と比べている（区間全体の揺れで落ちる #2721 の形）");
        }
        out
    }

    #[test]
    fn tasks_inputは押下を配送そのものでも測り判定をこの規則に通す() {
        let found = bypasses(include_str!("tasks_input_2016.rs"));
        assert!(found.is_empty(), "tasks_input_2016.rs: {found:?}（#2721）");
    }

    #[test]
    fn 番犬は区間全体だけの旧の判定を見逃さない() {
        // #2711 の形（区間全体の組の差を自前で閾値と比べ、状態は緩い確かめだけ）
        let old = "let ms = t.elapsed();\nlet lag_ok = pending_shown && send_cost_ms < 8.0;\n\
                   check(lag_ok, \"…\");\n";
        let found = bypasses(old);
        assert_eq!(found.len(), 4, "{found:?}");
        // コメントの中の語は数えない
        let commented = "// click_timed( click_timed( click_timed( judge(&observed, form)\n";
        assert!(bypasses(commented).len() >= 3);
    }
}
