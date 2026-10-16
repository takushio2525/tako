//! 「読めなければ既定値へ落とす」読み手が、落とした理由を呼び手へ返す型（#1824）
//!
//! `recent.json` / `remote/shortcuts.json` の読み手は tako-core にあり、診断ログ
//! （persist.log。書き手は tako-control の `diag`）へ直接は書けない。以前は壊れた中身を
//! 退避するだけで何も申告しなかったので、「いつ壊れていたか」を後から追えなかった。
//! ここでは**理由と退避の結果を返すだけ**にして、記録は tako-control の 1 つの口
//! （`data_files`）が「移行できず」と同じ形の 1 行で書く。

use crate::migration::Quarantine;

/// 既定値へ落とした理由
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadIssue {
    /// 中身を解釈できなかった。`quarantine` は退避の結果（写せなければ `None`）
    Unparsable {
        reason: String,
        quarantine: Option<Quarantine>,
    },
    /// 読むこと自体ができなかった（権限・UTF-8 でない等）。文字として読めないので退避できない
    ReadFailed { reason: String },
}

/// JSON を解釈できなかった理由を**中身を含めずに**言う（#1824）。
///
/// serde_json の文言は型が違うとき `invalid type: string "<ファイルの値>"` のように
/// 読んだ値をそのまま含む（実測）。診断ログ（persist.log）とテレメトリへファイルの中身を
/// 出さないため、種類と位置（行・列）だけにする
pub fn json_reason(e: &serde_json::Error) -> String {
    use serde_json::error::Category;
    let kind = match e.classify() {
        Category::Syntax => "JSON の構文が壊れている",
        Category::Eof => "JSON が途中で切れている",
        Category::Data => "JSON の形が期待と違う",
        Category::Io => "読み取りに失敗",
    };
    format!("{kind}（{} 行 {} 列）", e.line(), e.column())
}

impl LoadIssue {
    /// `std::fs::read_to_string` の失敗を理由へ。**不在は理由にしない**（既定値が正しい答え）
    pub fn from_read_error(e: &std::io::Error) -> Option<Self> {
        (e.kind() != std::io::ErrorKind::NotFound).then(|| Self::ReadFailed {
            reason: format!("読み取りに失敗: {e}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 型が違うと serde_json の文言は読んだ値を含むが、理由には載せない
    #[test]
    fn 理由に中身を載せない() {
        #[derive(Debug, serde::Deserialize)]
        #[allow(dead_code)]
        struct S {
            n: u32,
        }
        let cases = [
            (
                r#"{"n": "VALUE_MARK"}"#,
                "JSON の形が期待と違う（1 行 18 列）",
            ),
            (r#"{"n": 1,, }"#, "JSON の構文が壊れている（1 行 9 列）"),
            (r#"{"n": "#, "JSON が途中で切れている（1 行 6 列）"),
            ("", "JSON が途中で切れている（1 行 0 列）"),
        ];
        for (body, want) in cases {
            let e = serde_json::from_str::<S>(body).expect_err("読めない");
            assert_eq!(json_reason(&e), want, "{body:?}");
        }
        let raw = serde_json::from_str::<S>(r#"{"n": "VALUE_MARK"}"#).expect_err("読めない");
        assert!(
            raw.to_string().contains("VALUE_MARK"),
            "前提: 素の文言は値を含む"
        );
    }

    #[test]
    fn 不在は理由にしない() {
        let absent = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert_eq!(LoadIssue::from_read_error(&absent), None);
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert!(matches!(
            LoadIssue::from_read_error(&denied),
            Some(LoadIssue::ReadFailed { .. })
        ));
    }
}
