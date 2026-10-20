//! 使用量の数値の書き方（#2203）。`tako usage` の既定出力と右パネルの usage ビューが
//! **同じ 1 実装**で数字を文字にする（画面と CLI の数字が桁・丸めで食い違わない）。

/// トークン数（1 万以上は「万」、1 億以上は「億」）
pub fn tokens(n: u64) -> String {
    if n >= 100_000_000 {
        format!("{:.2} 億", n as f64 / 100_000_000.0)
    } else if n >= 10_000 {
        format!("{:.1} 万", n as f64 / 10_000.0)
    } else {
        n.to_string()
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
pub fn range_label(window: &str, range: &serde_json::Value, now: i64, utc_offset: i32) -> String {
    let stamp = |v: &serde_json::Value| {
        v.as_i64()
            .map(|t| crate::user_task::response_stamp(t, now, utc_offset))
            .unwrap_or_else(|| "-".into())
    };
    let length = match window {
        "5h" => "5 時間",
        _ => "7 日",
    };
    match range["basis"].as_str() {
        Some("observed") => format!(
            "今の {window} の窓（{} 〜・解除 {}）",
            stamp(&range["from"]),
            stamp(&range["resets_at"])
        ),
        Some("rolling") => format!("直近 {length}（観測した今の窓が無い）"),
        Some("calendar") => {
            let name = match window {
                "day" => "今日",
                "week" => "今週",
                _ => "今月",
            };
            format!("{name}（{} 〜）", stamp(&range["from"]))
        }
        _ => "索引のすべて".to_string(),
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
}
