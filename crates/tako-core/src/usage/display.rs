//! 使用量の数値の書き方（#2203）。`tako usage` の既定出力と右パネルの usage ビューが
//! **同じ 1 実装**で数字を文字にする（画面と CLI の数字が桁・丸めで食い違わない）。
//!
//! 単位・期間・段の行の名前の文は表示言語を引数で受ける（#2365。`tako usage` と右パネルの usage ビューは
//! どちらも表示言語で組む）

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

/// 段の行の名前（応答の `groups[]` の 1 件。`by` は段の切り方）。日本語は応答の `name` をそのまま、英語は
/// 応答の `key` と段の切り方から言い換えの名前（未分類・Issue なし・合計 ほか）を組み直す（作る側と同じ
/// [`crate::usage::report::group_name_in`]）。`tako usage` と右パネルの usage ビューの 1 実装（#2365）
pub fn group_row_name_in(g: &serde_json::Value, by: &str, lang: Lang) -> String {
    use crate::usage::report;
    let name = g["name"].as_str().unwrap_or("-");
    if lang == Lang::Ja {
        return name.to_string();
    }
    match (
        report::By::parse(by),
        serde_json::from_value::<report::GroupKey>(g["key"].clone()),
    ) {
        (Some(by), Ok(key)) => report::group_name_in(by, &key, lang),
        _ => name.to_string(),
    }
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
        assert_eq!(tokens_in(9_999, Lang::Ja), "9999");
        assert_eq!(tokens_in(12_345, Lang::Ja), "1.2 万");
        assert_eq!(tokens_in(365_000_000, Lang::Ja), "3.65 億");
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
            range_label_in("5h", &r, now, 0, Lang::Ja),
            "直近 5 時間（観測した今の窓が無い）"
        );
        let r = serde_json::json!({ "basis": "all" });
        assert_eq!(range_label_in("all", &r, now, 0, Lang::Ja), "索引のすべて");
        let r =
            serde_json::json!({ "basis": "observed", "from": now - 3600, "resets_at": now + 3600 });
        assert!(range_label_in("7d", &r, now, 0, Lang::Ja).starts_with("今の 7d の窓（"));
    }

    /// #2365: 段の行の名前は日本語なら応答の `name` のまま、英語なら `key` と段の切り方から言い換える
    #[test]
    fn 段の行の名前は日本語は応答のまま英語は言い換える() {
        use serde_json::json;
        for (by, key, ja, en) in [
            ("project", json!({}), "未分類", "unclassified"),
            (
                "project",
                json!({"profile": "dev"}),
                "プロファイル: dev",
                "profile: dev",
            ),
            (
                "issue",
                json!({"project": "tako"}),
                "tako Issue なし",
                "tako no issue",
            ),
            ("login", json!({}), "合計", "total"),
            ("account", json!({}), "不明", "unknown"),
            ("task_type", json!({}), "種別なし", "no task type"),
            ("project", json!({"project": "tako"}), "tako", "tako"),
        ] {
            let g = json!({"key": key, "name": ja});
            assert_eq!(group_row_name_in(&g, by, Lang::Ja), ja, "{by} {key}");
            assert_eq!(group_row_name_in(&g, by, Lang::En), en, "{by} {key}");
        }
        // 段の切り方を読めない・`key` の無い旧い応答は `name` のまま（英語でも推し量らない）
        let g = json!({"name": "x"});
        assert_eq!(group_row_name_in(&g, "nosuch", Lang::En), "x");
    }

    /// #2365: 英語の口は日本語の文字を含まない（日本語は #2365 より前の文。上の [`桁の書き方`] ほか）
    #[test]
    fn 英語の口は日本語を含まない() {
        let ja = |s: &str| {
            s.chars().any(|c| {
                ('\u{3040}'..='\u{30ff}').contains(&c) || ('\u{4e00}'..='\u{9fff}').contains(&c)
            })
        };
        for n in [0, 9_999, 12_345, 1_234_567, 365_000_000, 2_500_000_000] {
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
            assert!(
                !ja(&range_label_in(window, &r, now, 0, Lang::En)),
                "{window} {basis}"
            );
        }
    }
}
