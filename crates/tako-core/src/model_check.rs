//! model_check — 宣言したモデルと実際に使われたモデルの突き合わせ（Issue #1638）
//!
//! spawn は `--model claude-opus-5-5` を渡すが、アカウントの entitlement やモデルの廃止で
//! claude CLI が**無言で**別のモデルへ解決することがある（2026-09-23 実測: univ アカウントで
//! `claude-opus-5-5` → `claude-opus-5`）。tako は宣言値を真として workers・ledger へ載せていたので、
//! 「worker は 5.5」がすべて嘘になっていた。
//!
//! ここは**比べ方だけ**を持つ純粋関数（実際のモデルの採取は tako-control が transcript から行う）。
//! 名前の揺れ（日付の接尾辞・`[1m]`・クラウドの前置き）は単価表と同じ
//! [`crate::usage::price::normalize_model`] で寄せる。

use crate::usage::price::normalize_model;

/// 宣言と実際の突き合わせの結果（応答の `model_check`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelCheck {
    /// 宣言どおりのモデルで動いている
    Match,
    /// 宣言と違うモデルで動いている（warnings に 1 行出す）
    Mismatch,
    /// 実際のモデルがまだ確定していない（会話の最初の応答がまだ・transcript がまだ無い）
    Pending,
    /// 宣言が無い（CLI の既定に委ねた）。実際のモデルだけが分かっている
    Undeclared,
    /// 宣言が `opusplan` / `default` 等で、1 つのモデルに決まらないので比べられない
    Unverifiable,
    /// この系統は実際のモデルを観測しない（`agent_support` の `worker_model_actual` が degraded）
    NotObserved,
}

impl ModelCheck {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Match => "match",
            Self::Mismatch => "mismatch",
            Self::Pending => "pending",
            Self::Undeclared => "undeclared",
            Self::Unverifiable => "unverifiable",
            Self::NotObserved => "not_observed",
        }
    }
}

/// claude の `--model` が受けるモデルの系統のエイリアス（`claude --model` の help の語彙）。
/// これらは「その系統の最新」へ解決されるので、実際のモデルの系統が合えば一致とみなす
const FAMILY_ALIASES: &[&str] = &["opus", "sonnet", "haiku", "fable"];

/// 宣言と実際を突き合わせる（観測できる系統の判定。観測しない系統は呼び出し側が
/// [`ModelCheck::NotObserved`] にする）
pub fn check(declared: Option<&str>, actual: Option<&str>) -> ModelCheck {
    let Some(actual) = actual.map(str::trim).filter(|a| !a.is_empty()) else {
        return ModelCheck::Pending;
    };
    let Some(declared) = declared.map(str::trim).filter(|d| !d.is_empty()) else {
        return ModelCheck::Undeclared;
    };
    let want = normalize_model(declared);
    let got = normalize_model(actual);
    if want.starts_with("claude-") {
        return if want == got {
            ModelCheck::Match
        } else {
            ModelCheck::Mismatch
        };
    }
    if FAMILY_ALIASES.contains(&want.as_str()) {
        return if got.starts_with(&format!("claude-{want}-")) {
            ModelCheck::Match
        } else {
            ModelCheck::Mismatch
        };
    }
    ModelCheck::Unverifiable
}

/// transcript の `message.model` が実際のモデルの名前か（claude が手元で作った応答は
/// `<synthetic>` を書く = API を通っていないのでモデルの証拠にならない）
pub fn is_real_model(model: &str) -> bool {
    let m = model.trim();
    !m.is_empty() && !m.starts_with('<')
}

/// 食い違いの 1 行（`warnings` と persist.log に同じ文を出す）。
///
/// `account` は transcript が属したアカウント名（accounts.yaml の逆引き）。引けなければ `None` で、
/// そのときは名指ししない（推測で別のアカウントを指さない）。
/// 名前・モデル名だけを載せる（ホームパス・会話の中身は載せない）
pub fn mismatch_warning(declared: &str, actual: &str, account: Option<&str>) -> String {
    let whose = account
        .map(|name| format!("アカウント {name} の "))
        .unwrap_or_default();
    format!(
        "モデルの食い違い: 宣言 {declared} → 実際 {actual}（{whose}entitlement かモデルの廃止で \
         claude が別のモデルへ解決した。ledger は実際のモデルで集計する）"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 宣言と実際が同じなら一致() {
        assert_eq!(
            check(Some("claude-opus-5-5"), Some("claude-opus-5-5")),
            ModelCheck::Match
        );
        // 日付の接尾辞・[1m]・前後の空白は同じモデル
        assert_eq!(
            check(Some("claude-opus-5-5[1m]"), Some("claude-opus-5-5")),
            ModelCheck::Match
        );
        assert_eq!(
            check(Some("claude-haiku-4-5"), Some("claude-haiku-4-5-20251001")),
            ModelCheck::Match
        );
    }

    /// 2026-09-23 の実発生（univ アカウントで 5.5 → 5）
    #[test]
    fn 無言のフォールバックは食い違い() {
        assert_eq!(
            check(Some("claude-opus-5-5"), Some("claude-opus-5")),
            ModelCheck::Mismatch
        );
        // 逆向きも同じ（前方一致で取り違えない）
        assert_eq!(
            check(Some("claude-opus-5"), Some("claude-opus-5-5")),
            ModelCheck::Mismatch
        );
    }

    #[test]
    fn 系統のエイリアスは系統が合えば一致() {
        assert_eq!(
            check(Some("opus"), Some("claude-opus-5-5")),
            ModelCheck::Match
        );
        assert_eq!(
            check(Some("opus[1m]"), Some("claude-opus-5")),
            ModelCheck::Match
        );
        assert_eq!(
            check(Some("sonnet"), Some("claude-opus-5")),
            ModelCheck::Mismatch
        );
        // `claude-opusx` のような別名を系統違いで拾わない（区切りの `-` まで見る）
        assert_eq!(
            check(Some("opus"), Some("claude-opusx-1")),
            ModelCheck::Mismatch
        );
    }

    #[test]
    fn 未確定_宣言なし_比べられない宣言() {
        assert_eq!(check(Some("claude-opus-5-5"), None), ModelCheck::Pending);
        assert_eq!(
            check(Some("claude-opus-5-5"), Some("  ")),
            ModelCheck::Pending
        );
        assert_eq!(check(None, Some("claude-opus-5")), ModelCheck::Undeclared);
        assert_eq!(
            check(Some(""), Some("claude-opus-5")),
            ModelCheck::Undeclared
        );
        assert_eq!(check(None, None), ModelCheck::Pending);
        assert_eq!(
            check(Some("opusplan"), Some("claude-sonnet-5")),
            ModelCheck::Unverifiable
        );
        assert_eq!(
            check(Some("default"), Some("claude-opus-5")),
            ModelCheck::Unverifiable
        );
    }

    #[test]
    fn 手元で作った応答はモデルの証拠にしない() {
        assert!(!is_real_model("<synthetic>"));
        assert!(!is_real_model(""));
        assert!(is_real_model("claude-opus-5"));
    }

    #[test]
    fn 食い違いの行はアカウント名とモデル名だけを載せる() {
        let line = mismatch_warning("claude-opus-5-5", "claude-opus-5", Some("univ"));
        assert!(
            line.contains("宣言 claude-opus-5-5 → 実際 claude-opus-5"),
            "{line}"
        );
        assert!(line.contains("アカウント univ の entitlement"), "{line}");
        // アカウントを引けなければ名指ししない
        let line = mismatch_warning("claude-opus-5-5", "claude-opus-5", None);
        assert!(!line.contains("アカウント"), "{line}");
        assert!(line.contains("（entitlement"), "{line}");
    }
}
