//! API 換算の単価表（usage 分析 S2 #2201。設計書 `.agent/plans/2026-10-usage-analytics.md` §2.3）。
//!
//! **表はここ 1 か所**。索引（`buckets/`）にはトークン数だけを残し、金額は問い合わせのたびにこの表で
//! 計算する（表を直せば過去の分も直る）。表に無いモデルは 0 円にせず「単価不明」として別に数える
//! （[`price_for`] が `None`。`tako usage` の注記で気づける = S2 の受け入れ 5）。
//!
//! 表示では必ず「API 換算」と添える（サブスクリプションの請求額ではない）。

use super::index::Tokens;

/// 100 万トークンあたりの USD（入力・出力・キャッシュ作成 5 分・キャッシュ作成 1 時間・キャッシュ読み出し）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_5m: f64,
    pub cache_1h: f64,
    pub cache_read: f64,
}

/// fast（`usage.speed == "fast"`）の倍率
pub const FAST_MULTIPLIER: f64 = 2.0;

/// 単価表の版（claude-api のリファレンスの日付。応答に載せて出どころを示す）
pub const TABLE_AS_OF: &str = "2026-10-06";

/// 単価表（正規化したモデル名 → 単価）。読み出しの倍率がモデルで違うことに注意（§2.3）
const TABLE: &[(&str, Price)] = &[
    (
        "claude-opus-5-5",
        Price {
            input: 4.0,
            output: 20.0,
            cache_5m: 5.0,
            cache_1h: 8.0,
            cache_read: 0.20,
        },
    ),
    (
        "claude-opus-5",
        Price {
            input: 5.0,
            output: 25.0,
            cache_5m: 6.25,
            cache_1h: 10.0,
            cache_read: 0.5,
        },
    ),
    (
        "claude-fable-5-1",
        Price {
            input: 10.0,
            output: 50.0,
            cache_5m: 12.5,
            cache_1h: 20.0,
            cache_read: 0.25,
        },
    ),
    (
        "claude-fable-5",
        Price {
            input: 10.0,
            output: 50.0,
            cache_5m: 12.5,
            cache_1h: 20.0,
            cache_read: 1.0,
        },
    ),
    (
        "claude-haiku-5-5",
        Price {
            input: 0.10,
            output: 0.50,
            cache_5m: 0.125,
            cache_1h: 0.2,
            cache_read: 0.01,
        },
    ),
    (
        "claude-haiku-4-5",
        Price {
            input: 1.0,
            output: 5.0,
            cache_5m: 1.25,
            cache_1h: 2.0,
            cache_read: 0.1,
        },
    ),
];

/// transcript のモデル名を単価表の名前へ寄せる。日付の接尾辞（`-20251001`）・コンテキスト長の印
/// （`[1m]`）・クラウドの版（`@…` / `:0` / `-v1`）・前置き（`us.anthropic.`）を落とし、小文字にする
pub fn normalize_model(model: &str) -> String {
    let lower = model.trim().to_ascii_lowercase();
    let name = match lower.split_once("claude-") {
        Some((_, rest)) => format!("claude-{rest}"),
        None => lower.clone(),
    };
    let name = name.split(['[', '@', ':']).next().unwrap_or_default();
    let all_digits = |t: &str| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit());
    // `-v1` / `-v2` のような版（Bedrock）
    let name = match name.rsplit_once("-v") {
        Some((head, tail)) if all_digits(tail) => head,
        _ => name,
    };
    // `-YYYYMMDD`（8 桁の日付）
    let name = match name.rsplit_once('-') {
        Some((head, tail)) if all_digits(tail) && tail.chars().count() == 8 => head,
        _ => name,
    };
    name.to_string()
}

/// 単価（表に無ければ `None` = 単価不明）
pub fn price_for(model: &str) -> Option<Price> {
    let name = normalize_model(model);
    TABLE.iter().find(|(m, _)| *m == name).map(|(_, p)| *p)
}

/// API 換算（USD）。表に無いモデルは `None`
pub fn cost_usd(model: &str, fast: bool, tokens: &Tokens) -> Option<f64> {
    let p = price_for(model)?;
    let base = tokens.input as f64 * p.input
        + tokens.output as f64 * p.output
        + tokens.cache_5m as f64 * p.cache_5m
        + tokens.cache_1h as f64 * p.cache_1h
        + tokens.cache_read as f64 * p.cache_read;
    let mult = if fast { FAST_MULTIPLIER } else { 1.0 };
    Some(base / 1_000_000.0 * mult)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn モデル名を表の名前へ寄せる() {
        assert_eq!(
            normalize_model("claude-haiku-4-5-20251001"),
            "claude-haiku-4-5"
        );
        assert_eq!(normalize_model("claude-opus-5-5"), "claude-opus-5-5");
        assert_eq!(normalize_model("Claude-Opus-5-5[1m]"), "claude-opus-5-5");
        assert_eq!(
            normalize_model("us.anthropic.claude-opus-5-v1:0"),
            "claude-opus-5"
        );
        assert_eq!(
            normalize_model("claude-fable-5-1@20261001"),
            "claude-fable-5-1"
        );
        // 表に無い名前は落とすものだけ落としてそのまま
        assert_eq!(normalize_model("claude-sonnet-9-9"), "claude-sonnet-9-9");
        assert_eq!(normalize_model("gpt-x"), "gpt-x");
    }

    #[test]
    fn 成分ごとの単価とfastの倍率() {
        let t = Tokens {
            input: 1_000_000,
            output: 1_000_000,
            cache_5m: 1_000_000,
            cache_1h: 1_000_000,
            cache_read: 1_000_000,
        };
        let opus = cost_usd("claude-opus-5-5", false, &t).unwrap();
        assert!(
            (opus - (4.0 + 20.0 + 5.0 + 8.0 + 0.2)).abs() < 1e-9,
            "{opus}"
        );
        let fast = cost_usd("claude-opus-5-5", true, &t).unwrap();
        assert!((fast - opus * 2.0).abs() < 1e-9);
        // 読み出しの倍率はモデルで違う（Fable 5.1 は ×0.025・Fable 5 は ×0.1）
        let read = Tokens {
            cache_read: 1_000_000,
            ..Tokens::default()
        };
        assert!((cost_usd("claude-fable-5-1", false, &read).unwrap() - 0.25).abs() < 1e-9);
        assert!((cost_usd("claude-fable-5", false, &read).unwrap() - 1.0).abs() < 1e-9);
        assert!((cost_usd("claude-haiku-4-5-20251001", false, &read).unwrap() - 0.1).abs() < 1e-9);
        // 表に無いモデルは 0 円にしない
        assert_eq!(cost_usd("claude-sonnet-9-9", false, &t), None);
        assert_eq!(cost_usd("<synthetic>", false, &t), None);
    }
}
