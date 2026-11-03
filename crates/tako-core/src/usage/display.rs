//! 使用量の数値の書き方（#2203）。`tako usage` の既定出力と右パネルの usage ビューが
//! **同じ 1 実装**で数字を文字にする（画面と CLI の数字が桁・丸めで食い違わない）。
//!
//! 単位と期間の文は表示言語を引数で受ける口（`*_in`）が正本（#2365。`tako usage` は表示言語で組む）。
//! 言語を受けない口は日本語の口で、右パネルの usage ビューが今もこちらを呼ぶ

use crate::i18n::Lang;

/// トークン数（日本語は 1 万以上を「万」・1 億以上を「億」、英語は 1 万以上を K・100 万以上を M・
/// 10 億以上を B。1 万未満はどちらもそのまま）
pub fn tokens_in(n: u64, lang: Lang) -> String {
    match lang {
        Lang::Ja => {
            if n >= 100_000_000 {
                format!("{:.2} 億", n as f64 / 100_000_000.0)
            } else if n >= 10_000 {
                format!("{:.1} 万", n as f64 / 10_000.0)
            } else {
                n.to_string()
            }
        }
        Lang::En => {
            if n >= 1_000_000_000 {
                format!("{:.2}B", n as f64 / 1_000_000_000.0)
            } else if n >= 1_000_000 {
                format!("{:.1}M", n as f64 / 1_000_000.0)
            } else if n >= 10_000 {
                format!("{:.1}K", n as f64 / 1_000.0)
            } else {
                n.to_string()
            }
        }
    }
}

/// トークン数の日本語の口（[`tokens_in`]）
pub fn tokens(n: u64) -> String {
    tokens_in(n, Lang::Ja)
}

/// 段の利用制限の %（小数第 1 位）
pub fn share_percent(v: f64) -> String {
    format!("{v:.1}%")
}

/// 推定の上乗せ（`+0.4`）。0.05 未満は出さない
pub fn estimated(v: f64) -> Option<String> {
    (v >= 0.05).then(|| format!("+{v:.1}"))
}

/// API 換算（ドル。小数第 2 位）
pub fn api_usd(v: f64) -> String {
    format!("${v:.2}")
}

/// 観測した利用制限の %（整数ならそのまま、端数があれば小数第 1 位）
pub fn observed_percent(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{v:.0}%")
    } else {
        format!("{v:.1}%")
    }
}

/// 期間の表記（`tako usage` の見出しと右パネルの usage ビューが同じ文を出す）。
/// `range` は応答の `logins[].usage.range`（`basis` / `from` / `resets_at`）
pub fn range_label_in(
    window: &str,
    range: &serde_json::Value,
    now: i64,
    utc_offset: i32,
    lang: Lang,
) -> String {
    let stamp = |v: &serde_json::Value| {
        v.as_i64()
            .map(|t| crate::user_task::response_stamp(t, now, utc_offset))
            .unwrap_or_else(|| "-".into())
    };
    match (range["basis"].as_str(), lang) {
        (Some("observed"), Lang::Ja) => format!(
            "今の {window} の窓（{} 〜・解除 {}）",
            stamp(&range["from"]),
            stamp(&range["resets_at"])
        ),
        (Some("observed"), Lang::En) => format!(
            "the current {window} window (since {}, resets {})",
            stamp(&range["from"]),
            stamp(&range["resets_at"])
        ),
        (Some("rolling"), Lang::Ja) => {
            let length = match window {
                "5h" => "5 時間",
                _ => "7 日",
            };
            format!("直近 {length}（観測した今の窓が無い）")
        }
        (Some("rolling"), Lang::En) => {
            let length = match window {
                "5h" => "5 hours",
                _ => "7 days",
            };
            format!("the last {length} (no observed current window)")
        }
        (Some("calendar"), Lang::Ja) => {
            let name = match window {
                "day" => "今日",
                "week" => "今週",
                _ => "今月",
            };
            format!("{name}（{} 〜）", stamp(&range["from"]))
        }
        (Some("calendar"), Lang::En) => {
            let name = match window {
                "day" => "today",
                "week" => "this week",
                _ => "this month",
            };
            format!("{name} (since {})", stamp(&range["from"]))
        }
        (_, Lang::Ja) => "索引のすべて".to_string(),
        (_, Lang::En) => "the whole index".to_string(),
    }
}

/// 期間の表記の日本語の口（[`range_label_in`]）
pub fn range_label(window: &str, range: &serde_json::Value, now: i64, utc_offset: i32) -> String {
    range_label_in(window, range, now, utc_offset, Lang::Ja)
}

/// 枠の短い名前（`five_hour` → `5h`）
pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        "five_hour" => "5h",
        _ => "7d",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 桁の書き方() {
        assert_eq!(tokens(9_999), "9999");
        assert_eq!(tokens(12_345), "1.2 万");
        assert_eq!(tokens(365_000_000), "3.65 億");
        assert_eq!(share_percent(30.94), "30.9%");
        assert_eq!(share_percent(0.0), "0.0%");
        assert_eq!(estimated(0.04), None);
        assert_eq!(estimated(0.44).as_deref(), Some("+0.4"));
        assert_eq!(api_usd(20.6), "$20.60");
        assert_eq!(observed_percent(83.0), "83%");
        assert_eq!(observed_percent(12.5), "12.5%");
        assert_eq!(kind_label("five_hour"), "5h");
        assert_eq!(kind_label("seven_day"), "7d");
        let now = 1_792_000_000;
        let r = serde_json::json!({ "basis": "rolling" });
        assert_eq!(
            range_label("5h", &r, now, 0),
            "直近 5 時間（観測した今の窓が無い）"
        );
        let r = serde_json::json!({ "basis": "all" });
        assert_eq!(range_label("all", &r, now, 0), "索引のすべて");
        let r =
            serde_json::json!({ "basis": "observed", "from": now - 3600, "resets_at": now + 3600 });
        assert!(range_label("7d", &r, now, 0).starts_with("今の 7d の窓（"));
    }

    /// #2365: 英語の口は日本語の文字を含まず、日本語の口は言語を受けない口と同じ
    #[test]
    fn 英語の口は日本語を含まない() {
        let ja = |s: &str| {
            s.chars().any(|c| {
                ('\u{3040}'..='\u{30ff}').contains(&c) || ('\u{4e00}'..='\u{9fff}').contains(&c)
            })
        };
        for n in [0, 9_999, 12_345, 1_234_567, 365_000_000, 2_500_000_000] {
            assert_eq!(tokens_in(n, Lang::Ja), tokens(n));
            assert!(!ja(&tokens_in(n, Lang::En)), "{n}");
        }
        assert_eq!(tokens_in(12_345, Lang::En), "12.3K");
        assert_eq!(tokens_in(1_234_567, Lang::En), "1.2M");
        assert_eq!(tokens_in(2_500_000_000, Lang::En), "2.50B");
        let now = 1_792_000_000;
        for (window, basis) in [
            ("5h", "observed"),
            ("7d", "rolling"),
            ("5h", "rolling"),
            ("day", "calendar"),
            ("week", "calendar"),
            ("month", "calendar"),
            ("all", "all"),
        ] {
            let r =
                serde_json::json!({ "basis": basis, "from": now - 3600, "resets_at": now + 3600 });
            assert_eq!(
                range_label_in(window, &r, now, 0, Lang::Ja),
                range_label(window, &r, now, 0)
            );
            assert!(
                !ja(&range_label_in(window, &r, now, 0, Lang::En)),
                "{window} {basis}"
            );
        }
    }
}
