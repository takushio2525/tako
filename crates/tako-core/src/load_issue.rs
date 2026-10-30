//! 「読めなければ既定値へ落とす」読み手が、落とした理由を呼び手へ返す型（#1824）
//!
//! `recent.json` / `remote/shortcuts.json` の読み手は tako-core にあり、診断ログ
//! （persist.log。書き手は tako-control の `diag`）へ直接は書けない。以前は壊れた中身を
//! 退避するだけで何も申告しなかったので、「いつ壊れていたか」を後から追えなかった。
//! ここでは**理由と退避の結果を返すだけ**にして、記録は tako-control の 1 つの口
//! （`data_files`）が「移行できず」と同じ形の 1 行で書く。
//!
//! 退避も理由と同じ所（[`LoadIssue::read_failed`] / [`LoadIssue::unparsable`]）で済ませる。
//! 文字として読めない中身（UTF-8 でない・権限で読めない）は文字列の退避口を通れないので、
//! バイトのまま移す（`migration::quarantine_raw`。#2231）。読み手ごとに書き分けると、
//! 片方だけ退避を忘れる形が戻る。settings.json・run-configs.json・claude-mod/ui.json の
//! 読み手も同じ [`LoadIssue::read_failed`] を通る（#2238）
//!
//! 理由の文字列は**ファイルの中身を含めない**（種類と行・列だけ）。serde の素の文言は型が違うと
//! 読んだ値を引用するので、ファイルを読む口は JSON なら [`json_error_reason`]、YAML なら
//! [`yaml_error_reason`] で理由を組む（#2230）。

use std::path::Path;

use crate::i18n::Lang;
use crate::migration::Quarantine;

/// 既定値へ落とした理由
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadIssue {
    /// 中身を解釈できなかった。`quarantine` は退避の結果（写せなければ `None`）
    Unparsable {
        reason: String,
        quarantine: Option<Quarantine>,
    },
    /// 読むこと自体ができなかった（権限・UTF-8 でない等）。`quarantine` はバイトのまま移した
    /// 退避の結果（移せなければ `None`。#2231）
    ReadFailed {
        reason: String,
        quarantine: Option<Quarantine>,
    },
}

/// #2231 の A/B。`TAKO_2231_LEGACY=1` で修正前の挙動（文字として読めない中身を退避せずに
/// 既定値へ落とす = 呼び手の次の保存が元の中身を上書きする）
pub fn legacy_2231() -> bool {
    std::env::var_os("TAKO_2231_LEGACY").is_some()
}

/// #2238 の A/B。`TAKO_2238_LEGACY=1` で settings.json / run-configs.json / claude-mod/ui.json の
/// 読み手を修正前へ戻す（settings / run-configs は文字として読めない中身を退避せずに既定値へ
/// 落とす = 次の保存が消す。ui.json は UTF-8 でない中身を 1 本目が空いているときだけ写す =
/// 2 回目の中身が残らない）
pub fn legacy_2238() -> bool {
    std::env::var_os("TAKO_2238_LEGACY").is_some()
}

/// JSON を解釈できなかった理由を**中身を含めずに**言う（#1824）。
///
/// serde_json の文言は型が違うとき `invalid type: string "<ファイルの値>"` のように
/// 読んだ値をそのまま含む（実測）。診断ログ（persist.log）とテレメトリへファイルの中身を
/// 出さないため、種類と位置（行・列）だけにする
pub fn json_reason(e: &serde_json::Error) -> String {
    JsonReason::located(e).text_in(Lang::Ja)
}

/// JSON を解釈できなかった理由を、表示言語で引ける形で持つ（#2365。[`YamlReason`] の JSON 版）。
///
/// 中身は [`json_reason`] と同じ「種類と位置だけ」（ファイルの中身を引用しない = #2230）。
/// 利用者へ返す断り（蓋閉じ継続の記録の解釈 = `tako_control::platform::lid::LidError`）は包みと
/// 同じ言語で [`Self::text_in`] を引く。persist.log へ流れる [`json_reason`] /
/// [`json_error_reason`] は日本語のまま（診断ログは日本語固定 = FR-2.42.21）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonReason {
    /// 種類と位置（行・列。serde_json の `line()` / `column()` のまま）
    Located {
        kind: JsonProblem,
        line: usize,
        column: usize,
    },
    /// `TAKO_2230_LEGACY=1` の修正前の形（serde の素の文言。英語のまま）
    Raw(String),
}

/// [`JsonReason`] の種類（serde_json の `Category` と 1:1）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonProblem {
    /// 構文として読めない
    Syntax,
    /// 途中で切れている
    Eof,
    /// 読めるが形が期待と違う
    Data,
    /// 読み取りに失敗
    Io,
}

impl JsonProblem {
    /// (種類, 言語) の網羅 match。日本語は #2365 より前の [`json_reason`] とバイト一致
    pub fn text_in(self, lang: Lang) -> &'static str {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::Syntax, Ja) => "JSON の構文が壊れている",
            (Self::Syntax, En) => "the JSON syntax is broken",
            (Self::Eof, Ja) => "JSON が途中で切れている",
            (Self::Eof, En) => "the JSON is cut off",
            (Self::Data, Ja) => "JSON の形が期待と違う",
            (Self::Data, En) => "the JSON does not have the expected shape",
            (Self::Io, Ja) => "読み取りに失敗",
            (Self::Io, En) => "failed to read",
        }
    }
}

impl JsonReason {
    /// 種類と位置を採る
    pub fn located(e: &serde_json::Error) -> Self {
        use serde_json::error::Category;
        let kind = match e.classify() {
            Category::Syntax => JsonProblem::Syntax,
            Category::Eof => JsonProblem::Eof,
            Category::Data => JsonProblem::Data,
            Category::Io => JsonProblem::Io,
        };
        Self::Located {
            kind,
            line: e.line(),
            column: e.column(),
        }
    }

    /// [`json_error_reason`] の表示言語版（`TAKO_2230_LEGACY=1` なら素の文言）
    pub fn of(e: &serde_json::Error) -> Self {
        if legacy_raw_serde_reason() {
            return Self::Raw(e.to_string());
        }
        Self::located(e)
    }

    /// (形, 言語) の網羅 match
    pub fn text_in(&self, lang: Lang) -> String {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::Located { kind, line, column }, Ja) => {
                format!("{}（{line} 行 {column} 列）", kind.text_in(lang))
            }
            (Self::Located { kind, line, column }, En) => {
                format!("{} (line {line}, column {column})", kind.text_in(lang))
            }
            (Self::Raw(raw), _) => raw.clone(),
        }
    }
}

/// YAML を解釈できなかった理由を**中身を含めずに**言う（#2230。[`json_reason`] の YAML 版）。
///
/// serde_yaml の文言は型が違うと `n: invalid type: string "<ファイルの値>"` のように、読んだ値と
/// そこまでのキーの道筋（`projects.<キー>.cwd`）をそのまま含む（実測）。種類は文言を読まずに、
/// `text`（解釈しようとした中身）を型を問わずに読み直して決める: 構文として読めない /
/// 値が無い（空・コメントだけ・`~`）/ 形が違う。位置は serde_yaml の `location()`（1 始まり）で、
/// 位置を持たない誤り（複数の文書など）は種類だけ
pub fn yaml_reason(e: &serde_yaml::Error, text: &str) -> String {
    YamlReason::located(e, text).text_in(Lang::Ja)
}

/// YAML を解釈できなかった理由を、表示言語で引ける形で持つ（#2365）。
///
/// 中身は [`yaml_reason`] と同じ「種類と位置だけ」（ファイルの中身を引用しない = #2230）。
/// 利用者へ返す断り（置き場の「<ファイル> のパースに失敗」= `tako_control::Reason::StoreParseFailed`）は
/// 包みと同じ言語で [`Self::text_in`] を引く。persist.log へ流れる [`yaml_reason`] /
/// [`yaml_error_reason`] は日本語のまま（診断ログは日本語固定 = FR-2.42.21）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum YamlReason {
    /// 種類と位置（行・列。1 始まり。位置を持たない誤り = 複数の文書などは `None`）
    Located {
        kind: YamlProblem,
        at: Option<(usize, usize)>,
    },
    /// `TAKO_2230_LEGACY=1` の修正前の形（serde の素の文言。英語のまま）
    Raw(String),
}

/// [`YamlReason`] の種類
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YamlProblem {
    /// 構文として読めない
    Syntax,
    /// 値が無い（空・コメントだけ・`~`）
    Empty,
    /// 読めるが形が期待と違う
    Shape,
}

impl YamlProblem {
    /// (種類, 言語) の網羅 match。日本語は #2365 より前の [`yaml_reason`] とバイト一致
    pub fn text_in(self, lang: Lang) -> &'static str {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::Syntax, Ja) => "YAML の構文が壊れている",
            (Self::Syntax, En) => "the YAML syntax is broken",
            (Self::Empty, Ja) => "YAML が空",
            (Self::Empty, En) => "the YAML is empty",
            (Self::Shape, Ja) => "YAML の形が期待と違う",
            (Self::Shape, En) => "the YAML does not have the expected shape",
        }
    }
}

impl YamlReason {
    /// 種類と位置を採る（`text` は解釈しようとした中身。種類は文言を読まずに読み直して決める）
    pub fn located(e: &serde_yaml::Error, text: &str) -> Self {
        let kind = if serde_yaml::from_str::<serde::de::IgnoredAny>(text).is_err() {
            YamlProblem::Syntax
        } else if matches!(
            serde_yaml::from_str::<serde_yaml::Value>(text),
            Ok(serde_yaml::Value::Null)
        ) {
            YamlProblem::Empty
        } else {
            YamlProblem::Shape
        };
        Self::Located {
            kind,
            at: e.location().map(|at| (at.line(), at.column())),
        }
    }

    /// [`yaml_error_reason`] の表示言語版（`TAKO_2230_LEGACY=1` なら素の文言）
    pub fn of(e: &serde_yaml::Error, text: &str) -> Self {
        if legacy_raw_serde_reason() {
            return Self::Raw(e.to_string());
        }
        Self::located(e, text)
    }

    /// (形, 言語) の網羅 match
    pub fn text_in(&self, lang: Lang) -> String {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::Located { kind, at: None }, _) => kind.text_in(lang).to_string(),
            (
                Self::Located {
                    kind,
                    at: Some((line, column)),
                },
                Ja,
            ) => format!("{}（{line} 行 {column} 列）", kind.text_in(lang)),
            (
                Self::Located {
                    kind,
                    at: Some((line, column)),
                },
                En,
            ) => format!("{} (line {line}, column {column})", kind.text_in(lang)),
            (Self::Raw(raw), _) => raw.clone(),
        }
    }
}

/// #2230 の A/B。`TAKO_2230_LEGACY=1` で修正前（serde の素の文言 = 型が違うと値を引用する）へ戻す
pub fn legacy_raw_serde_reason() -> bool {
    std::env::var_os("TAKO_2230_LEGACY").is_some_and(|v| !v.is_empty() && v != "0")
}

/// **ファイルの中身**を JSON として解釈できなかった理由（#2230）。中身は [`json_reason`]。
///
/// 読む口の理由は persist.log（`migrations::record` / settings の申告）・応答の `warnings`・
/// CLI の行・master の system prompt へ同じ文字列のまま流れるので、作る所で中身を落とす。
/// 素の serde の文言を使ってよいのはファイルでない入力（IPC・HTTP・子プロセスの出力）だけで、
/// 番犬 `issue2230_serde_reason_watchdog` の許可表に理由つきで載せる
pub fn json_error_reason(e: &serde_json::Error) -> String {
    if legacy_raw_serde_reason() {
        return e.to_string();
    }
    json_reason(e)
}

/// [`json_error_reason`] の YAML 版。`text` は解釈しようとした中身（[`yaml_reason`] が読み直す）
pub fn yaml_error_reason(e: &serde_yaml::Error, text: &str) -> String {
    YamlReason::of(e, text).text_in(Lang::Ja)
}

#[cfg(test)]
mod yaml_reason_tests {
    use super::*;

    #[derive(Debug, serde::Deserialize)]
    #[allow(dead_code)]
    struct S {
        n: u32,
        #[serde(default)]
        map: std::collections::BTreeMap<String, Inner>,
    }

    #[derive(Debug, serde::Deserialize)]
    #[allow(dead_code)]
    struct Inner {
        v: u32,
    }

    fn reason(body: &str) -> String {
        let e = serde_yaml::from_str::<S>(body).expect_err("読めない");
        yaml_reason(&e, body)
    }

    /// 型が違うと serde_yaml の文言は値とキーの道筋を含むが、理由には載せない
    #[test]
    fn 理由に中身を載せない() {
        let nested = "n: 1\nmap:\n  KEY_MARK:\n    v: VALUE_MARK\n";
        let cases = [
            ("n: VALUE_MARK\n", "YAML の形が期待と違う（1 行 4 列）"),
            (nested, "YAML の形が期待と違う（4 行 8 列）"),
            ("VALUE_MARK\n", "YAML の形が期待と違う（1 行 1 列）"),
            ("n: 1\nn: 2\n", "YAML の形が期待と違う（1 行 1 列）"),
            (
                "n: 1\nmap:\n  k: \"VALUE_MARK\n",
                "YAML の構文が壊れている（4 行 1 列）",
            ),
            ("n: 1\n\tmap: {}\n", "YAML の構文が壊れている（2 行 1 列）"),
            ("n: 1\n---\nn: 2\n", "YAML の構文が壊れている"),
            ("", "YAML が空（1 行 1 列）"),
            ("# VALUE_MARK だけ\n", "YAML が空（2 行 1 列）"),
        ];
        for (body, want) in cases {
            assert_eq!(reason(body), want, "{body:?}");
        }
        let raw = serde_yaml::from_str::<S>(nested)
            .expect_err("読めない")
            .to_string();
        assert!(
            raw.contains("VALUE_MARK") && raw.contains("KEY_MARK"),
            "前提: 素の文言は値とキーを含む: {raw}"
        );
    }

    /// A/B の旧挙動は素の文言（修正前の再現）。既定は中身を含めない
    #[test]
    fn 既定は中身を含めない() {
        let yaml = serde_yaml::from_str::<S>("n: VALUE_MARK\n").expect_err("読めない");
        let json = serde_json::from_str::<S>(r#"{"n": "VALUE_MARK"}"#).expect_err("読めない");
        if legacy_raw_serde_reason() {
            return;
        }
        assert!(!yaml_error_reason(&yaml, "n: VALUE_MARK\n").contains("VALUE_MARK"));
        assert!(!json_error_reason(&json).contains("VALUE_MARK"));
        assert_eq!(json_error_reason(&json), json_reason(&json));
    }
}

impl LoadIssue {
    /// `std::fs::read_to_string` の失敗を理由へ。**不在は理由にしない**（既定値が正しい答え）。
    ///
    /// 読めなかった中身は既定値へ落とす前に**バイトのまま**退避へ移す（#2231）。移さないと、
    /// 呼び手の次の保存（load → 既定値 → save）が元の中身を上書きして消す
    pub fn read_failed(path: &Path, e: &std::io::Error) -> Option<Self> {
        if e.kind() == std::io::ErrorKind::NotFound {
            return None;
        }
        let quarantine = if legacy_2231() {
            None
        } else {
            crate::migration::quarantine_raw(path)
        };
        Some(Self::ReadFailed {
            reason: format!("読み取りに失敗: {e}"),
            quarantine,
        })
    }

    /// JSON を解釈できなかった理由へ。中身は既定値へ落とす前に退避する（#916）。
    ///
    /// 文字列の退避口（`quarantine_unreadable`）で写せなかったとき（満杯の詰め直しに読めない
    /// 退避が混ざる・置き場へ書けない）は、バイトのまま移す口へ落とす（#2231）
    pub fn unparsable(path: &Path, e: &serde_json::Error) -> Self {
        let quarantine = crate::migration::quarantine_unreadable(path, &crate::migration::FsIo)
            .or_else(|| {
                if legacy_2231() {
                    None
                } else {
                    crate::migration::quarantine_raw(path)
                }
            });
        Self::Unparsable {
            reason: json_reason(e),
            quarantine,
        }
    }

    /// 落とした理由（中身を含まない。#2230）
    pub fn reason(&self) -> &str {
        match self {
            Self::Unparsable { reason, .. } | Self::ReadFailed { reason, .. } => reason,
        }
    }

    /// 退避の結果（退避できなければ `None`）。読み手が自分の形で記録・応答を組むのに使う
    /// （型を組み直さずに読める口。#2238）
    pub fn quarantine(&self) -> Option<&Quarantine> {
        match self {
            Self::Unparsable { quarantine, .. } | Self::ReadFailed { quarantine, .. } => {
                quarantine.as_ref()
            }
        }
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
        let dir = std::env::temp_dir().join(format!("tako-load-issue-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("作れる");
        assert!(dir.starts_with(std::env::temp_dir()), "一時 dir の中だけ");
        let path = dir.join("recent.json");
        let absent = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert_eq!(LoadIssue::read_failed(&path, &absent), None);
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert!(matches!(
            LoadIssue::read_failed(&path, &denied),
            Some(LoadIssue::ReadFailed {
                quarantine: None,
                ..
            })
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
