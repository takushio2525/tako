//! 初期化オプション（`initialize` の `initializationOptions`）の既定と利用者の上書き（#2035）
//!
//! 既定は検出表の [`super::servers::ServerSpec::init_options`] 列（言語の追加 = 行追加だけを保つ）、
//! 上書きは settings.json の `lsp_init_options`（サーバ ID → ドット区切りのキー → JSON の値）。
//! サーバへ送るのは両者を重ねた実効値を入れ子のオブジェクトへ組んだもの（[`to_object`]）。
//!
//! 最初の 1 つは rust-analyzer の `cargo.targetDir = true`: 初回の build script と `cargo check` を
//! `target/rust-analyzer` で走らせ、利用者の `cargo build` と target のロックを取り合わない。
//! 比べた案と採った理由は `.agent/plans/2026-09-lsp-s1.md` §29。

use std::collections::BTreeMap;

use serde_json::{Map, Value};

/// 既定 1 つ（検出表の列）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InitOption {
    /// ドット区切りの経路（`cargo.targetDir`）
    pub key: &'static str,
    /// JSON の字句（`true` / `"path"`）。読めない字句は表のテストが落とす
    pub value: &'static str,
}

/// 1 サーバぶんの上書き（ドット区切りのキー → 値）
pub type Overrides = BTreeMap<String, Value>;

/// 実効値 1 つの出どころ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// 検出表の既定
    Default,
    /// 利用者の上書き（settings.json の `lsp_init_options`）
    User,
}

impl Source {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::User => "user",
        }
    }
}

/// 実効値 1 つ
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub key: String,
    pub value: Value,
    pub source: Source,
    /// 表の既定（上書きしていても見せる。表に無いキーは `None`）
    pub default: Option<Value>,
}

/// 表の既定の値（字句を読む。読めなければ `None` = 表のテストが落とす）
pub fn default_value(option: &InitOption) -> Option<Value> {
    serde_json::from_str(option.value).ok()
}

/// 既定と上書きを重ねた実効値（キーの昇順）。上書きは同じキーの既定を置き換え、表に無いキーは足す
pub fn effective(defaults: &[InitOption], overrides: Option<&Overrides>) -> Vec<Entry> {
    let mut out: BTreeMap<String, Entry> = BTreeMap::new();
    for option in defaults {
        let Some(value) = default_value(option) else {
            continue;
        };
        out.insert(
            option.key.to_string(),
            Entry {
                key: option.key.to_string(),
                value: value.clone(),
                source: Source::Default,
                default: Some(value),
            },
        );
    }
    for (key, value) in overrides.into_iter().flatten() {
        let default = out.get(key).and_then(|e| e.default.clone());
        out.insert(
            key.clone(),
            Entry {
                key: key.clone(),
                value: value.clone(),
                source: Source::User,
                default,
            },
        );
    }
    out.into_values().collect()
}

/// 実効値を入れ子のオブジェクトへ組む（`cargo.targetDir` → `{"cargo": {"targetDir": …}}`）。
/// 空なら `None`（`initializationOptions` を送らない = 変更前と同じ）。
///
/// 経路が重なったら（`a` と `a.b`）**長いほうが勝つ**: キーの昇順に入れるので `a` の値を
/// `a.b` のオブジェクトが置き換える（[`validate_key`] は重なりを断らない = 上書きで既定の
/// オブジェクトの 1 枝だけを変えられる）
pub fn to_object(entries: &[Entry]) -> Option<Value> {
    if entries.is_empty() {
        return None;
    }
    let mut root = Map::new();
    for entry in entries {
        insert_path(&mut root, &entry.key, entry.value.clone());
    }
    Some(Value::Object(root))
}

fn insert_path(root: &mut Map<String, Value>, key: &str, value: Value) {
    let mut node = root;
    let mut segments = key.split('.').peekable();
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            node.insert(segment.to_string(), value);
            return;
        }
        let child = node
            .entry(segment.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !child.is_object() {
            *child = Value::Object(Map::new());
        }
        let Value::Object(map) = child else {
            return;
        };
        node = map;
    }
}

/// キーの形の検査（空でない段をドットでつなぐ。段は ASCII の英数字と `_` / `-`）
pub fn validate_key(key: &str) -> Result<(), OptionRefusal> {
    let ok = !key.is_empty()
        && key.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        });
    if ok {
        Ok(())
    } else {
        Err(OptionRefusal::KeyShape(key.to_string()))
    }
}

/// 初期化オプションの上書きを断る理由（#2365。CLI `tako lsp options --set` / MCP `tako_lsp_server` の
/// `set`）。日英の対は [`OptionRefusal::text_in`] の網羅 match の 1 か所で、日本語は #2365 より前の
/// 固定文とバイト一致
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionRefusal {
    /// キーの形が不正（ドット区切りの経路でない）
    KeyShape(String),
    /// CLI の `--set` が `KEY=VALUE` の形でない（中身は打った文字のまま）
    NotAssignment(String),
}

impl OptionRefusal {
    /// 表示言語を明示しての文言（(バリアント, 言語) の網羅 match）
    pub fn text_in(&self, lang: crate::i18n::Lang) -> String {
        use crate::i18n::Lang::{En, Ja};
        match (self, lang) {
            (Self::KeyShape(key), Ja) => {
                format!("キーの形が不正: {key:?}（ドット区切りの経路。例: cargo.targetDir）")
            }
            (Self::KeyShape(key), En) => {
                format!("malformed key: {key:?} (a dot-separated path, e.g. cargo.targetDir)")
            }
            (Self::NotAssignment(text), Ja) => {
                format!("--set は KEY=VALUE の形: {text:?}（例: cargo.targetDir=false）")
            }
            (Self::NotAssignment(text), En) => {
                format!("--set takes the form KEY=VALUE: {text:?} (e.g. cargo.targetDir=false)")
            }
        }
    }
}

impl std::fmt::Display for OptionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(crate::i18n::lang()))
    }
}

/// CLI の `--set KEY=VALUE` の値: JSON として読めればその値（`true` / `3` / `"x"` / `null`）、
/// 読めなければ文字列（`target/ra` を引用符なしで書ける）
pub fn parse_value(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()))
}

/// CLI の `--set KEY=VALUE` を 1 つ読む。断りは表示言語で組む型（呼び手は CLI の手元の処理だけ。#2365）
pub fn parse_assignment(text: &str) -> Result<(String, Value), OptionRefusal> {
    let (key, value) = text
        .split_once('=')
        .ok_or_else(|| OptionRefusal::NotAssignment(text.to_string()))?;
    let key = key.trim();
    validate_key(key)?;
    Ok((key.to_string(), parse_value(value.trim())))
}

/// 上書きを当てる: 値なら入れ、`null` なら消す（= 表の既定へ戻す）。キーは先に検査する
/// （1 つでも不正なら何も変えない）
pub fn apply(overrides: &mut Overrides, set: &Map<String, Value>) -> Result<(), OptionRefusal> {
    for key in set.keys() {
        validate_key(key)?;
    }
    for (key, value) in set {
        if value.is_null() {
            overrides.remove(key);
        } else {
            overrides.insert(key.clone(), value.clone());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TARGET_DIR: &[InitOption] = &[InitOption {
        key: "cargo.targetDir",
        value: "true",
    }];

    fn overrides(pairs: &[(&str, Value)]) -> Overrides {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn 既定だけなら表の値を入れ子で送る() {
        let entries = effective(TARGET_DIR, None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source, Source::Default);
        assert_eq!(
            to_object(&entries),
            Some(json!({"cargo": {"targetDir": true}}))
        );
    }

    #[test]
    fn 上書きは同じキーの既定を置き換え_既定の値も見せる() {
        let user = overrides(&[("cargo.targetDir", json!(false))]);
        let entries = effective(TARGET_DIR, Some(&user));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].value, json!(false));
        assert_eq!(entries[0].source, Source::User);
        assert_eq!(entries[0].default, Some(json!(true)));
        assert_eq!(
            to_object(&entries),
            Some(json!({"cargo": {"targetDir": false}}))
        );
    }

    #[test]
    fn 表に無いキーの上書きは足され_同じ枝にまとまる() {
        let user = overrides(&[
            ("cargo.features", json!("all")),
            ("check.command", json!("clippy")),
        ]);
        let entries = effective(TARGET_DIR, Some(&user));
        assert_eq!(entries.len(), 3);
        let added = entries.iter().find(|e| e.key == "cargo.features").unwrap();
        assert_eq!(added.default, None);
        assert_eq!(
            to_object(&entries),
            Some(json!({
                "cargo": {"targetDir": true, "features": "all"},
                "check": {"command": "clippy"},
            }))
        );
    }

    #[test]
    fn 何も無ければ送らない() {
        assert_eq!(to_object(&effective(&[], None)), None);
        assert_eq!(to_object(&effective(&[], Some(&Overrides::new()))), None);
    }

    #[test]
    fn 経路が重なったら長いほうが勝つ() {
        let user = overrides(&[("cargo", json!(1)), ("cargo.targetDir", json!("x"))]);
        let entries = effective(&[], Some(&user));
        assert_eq!(
            to_object(&entries),
            Some(json!({"cargo": {"targetDir": "x"}}))
        );
    }

    #[test]
    fn キーの形を検査する() {
        assert!(validate_key("cargo.targetDir").is_ok());
        assert!(validate_key("a_b-c.d1").is_ok());
        for bad in ["", ".", "a.", ".a", "a..b", "a b", "a/b", "ａ"] {
            assert!(validate_key(bad).is_err(), "{bad:?} を通した");
        }
    }

    #[test]
    fn set_の値は_json_で読め_読めなければ文字列() {
        assert_eq!(
            parse_assignment("cargo.targetDir=false").unwrap(),
            ("cargo.targetDir".into(), json!(false))
        );
        assert_eq!(
            parse_assignment("cargo.targetDir=target/ra").unwrap(),
            ("cargo.targetDir".into(), json!("target/ra"))
        );
        assert_eq!(
            parse_assignment("cargo.targetDir=null").unwrap().1,
            Value::Null
        );
        assert_eq!(
            parse_assignment(" a.b = \"q\" ").unwrap(),
            ("a.b".into(), json!("q"))
        );
        assert!(parse_assignment("cargo.targetDir").is_err());
        assert!(parse_assignment("=1").is_err());
    }

    #[test]
    fn null_は上書きを消し_不正なキーが混じれば何も変えない() {
        let mut user = overrides(&[("cargo.targetDir", json!(false))]);
        let set: Map<String, Value> =
            serde_json::from_value(json!({"cargo.targetDir": null, "a.b": 1})).unwrap();
        apply(&mut user, &set).unwrap();
        assert_eq!(user, overrides(&[("a.b", json!(1))]));
        let bad: Map<String, Value> =
            serde_json::from_value(json!({"c.d": 2, "bad key": 3})).unwrap();
        assert!(apply(&mut user, &bad).is_err());
        assert_eq!(user, overrides(&[("a.b", json!(1))]));
    }
}
