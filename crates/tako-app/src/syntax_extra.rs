//! two-face に無い構文を構文セットの末尾へ足す（#1954）
//!
//! two-face（bat 由来・213 構文）は fancy-regex で組めない正規表現を含む構文を外している
//! （PowerShell は bat 同梱の定義がサブルーチン呼び出し `\g<1>` を使う）。tako は fancy-regex で
//! 組める別の定義を `syntaxes/<名前>/` に同梱し（出所とライセンスは同じ dir の `PROVENANCE.md`）、
//! ビルド時に `build.rs` が構文定義へ読んだもの（[`EXTRA`]）を起動時にここで足す。
//!
//! ## 足し方（既存の構文は 1 バイトも変えない）
//!
//! 1. 既存の構文セットを直列化の鏡（[`crate::syntax_prefilter`] の `SetMirror`）へ解く
//! 2. 足す構文がほかの構文を名指す参照（PowerShell → C# / 正規表現）を、syntect の builder に
//!    結ばせる。builder へは既存の構文と**同じ並び・同じ名前・同じ scope の身代わり**を入れ、
//!    名指される構文の身代わりにだけ本物と同じ名前の文脈（中身は空）を持たせる。builder は
//!    文脈を名前の順に番号付けするので、結んだ参照の番号は本物の構文セットのものと一致する
//! 3. 結んだ足す構文だけを鏡の末尾へ足して詰め直す
//!
//! syntect の `into_builder` で全体を作り直しても結果は 1 バイトも違わない（単体テストが
//! 突き合わせる）が、213 構文の文脈を全部解いて結び直すので release で 380 ms・山 +34 MB かかる。
//! この足し方は 6〜7 ms。どちらもプロセスで 1 回（[`crate::syntax_prefilter::built`] が使い回す）。
//!
//! 身代わりの文脈は中身が空なので、足す構文が**ほかの構文の文脈を `include` する**形は
//! 結べない（後方参照を使う文脈の印 `uses_backrefs` が include 先から伝わらない）。
//! その形が来たら足さずに two-face の構文セットのまま返す（[`appended`] が `Err`）。
//!
//! `TAKO_1954_LEGACY=1` で足さない（two-face の 213 構文のまま = #1954 以前）。同一バイナリの A/B 用

use std::collections::{BTreeSet, HashMap};
use std::sync::OnceLock;

use syntect::parsing::syntax_definition::{Context, ContextReference, MatchOperation, Pattern};
use syntect::parsing::{Scope, SyntaxDefinition, SyntaxSet, SyntaxSetBuilder};

use crate::syntax_prefilter::{LazyMirror, SetMirror};

/// `build.rs` が `syntaxes/` の YAML から作った構文定義（zlib + bincode の `Vec<SyntaxDefinition>`）
static EXTRA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/extra-syntaxes.bin"));

/// 足す構文の名前（`build.rs` の `SOURCES` と同じ並び。単体テストが引く）
#[cfg(test)]
pub(crate) const NAMES: &[&str] = &["PowerShell"];

/// 足す前へ戻す（`TAKO_1954_LEGACY=1`。同一バイナリで A/B を取る入口）
pub(crate) fn legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var("TAKO_1954_LEGACY").is_ok_and(|v| v == "1"))
}

/// 塗りの土台の構文セット = two-face + 足した構文。足せなければ two-face のまま
pub(crate) fn base_set() -> SyntaxSet {
    let base = two_face::syntax::extra_newlines();
    if legacy() {
        return base;
    }
    appended(&base).unwrap_or(base)
}

/// `base` の末尾へ足した構文セット
pub(crate) fn appended(base: &SyntaxSet) -> Result<SyntaxSet, String> {
    let bytes = append_bytes(base, definitions()?)?;
    syntect::dumps::from_uncompressed_data(&bytes)
        .map_err(|e| format!("足した構文セットを読めない: {e}"))
}

/// 同梱の構文定義（`build.rs` が読んだもの）
pub(crate) fn definitions() -> Result<Vec<SyntaxDefinition>, String> {
    syntect::dumps::from_reader(EXTRA).map_err(|e| format!("同梱の構文定義を読めない: {e}"))
}

/// `base` の末尾へ `defs` を足した構文セットの直列化（`from_uncompressed_data` で読める形）
pub(crate) fn append_bytes(
    base: &SyntaxSet,
    defs: Vec<SyntaxDefinition>,
) -> Result<Vec<u8>, String> {
    let original =
        bincode::serialize(base).map_err(|e| format!("構文セットを直列化できない: {e}"))?;
    let mut set: SetMirror =
        bincode::deserialize(&original).map_err(|e| format!("構文セットの形が想定と違う: {e}"))?;
    // 鏡の並びが syntect と食い違っていれば、往復で元のバイト列に戻らない（#1917 と同じ検査）
    if bincode::serialize(&set).ok().as_deref() != Some(&original[..]) {
        return Err("構文セットの直列化が往復で元に戻らない（syntect の形が変わった）".into());
    }
    let wanted = foreign_targets(&defs)?;
    let mut builder = SyntaxSetBuilder::new();
    for syntax in &set.syntaxes {
        let mut contexts = HashMap::new();
        if wanted.contains(&Target::Scope(syntax.scope))
            || wanted.contains(&Target::Name(syntax.name.clone()))
        {
            let lazy: LazyMirror =
                syntect::dumps::from_reader(&syntax.serialized_lazy_contexts[..])
                    .map_err(|e| format!("{} の文脈を読めない: {e}", syntax.name))?;
            for name in lazy.context_ids.into_keys() {
                contexts.insert(name, Context::new(false));
            }
        }
        builder.add(SyntaxDefinition {
            name: syntax.name.clone(),
            file_extensions: Vec::new(),
            scope: syntax.scope,
            first_line_match: None,
            hidden: syntax.hidden,
            variables: HashMap::new(),
            contexts,
        });
    }
    let existing = set.syntaxes.len();
    for def in defs {
        builder.add(def);
    }
    let linked = bincode::serialize(&builder.build())
        .map_err(|e| format!("結んだ構文を直列化できない: {e}"))?;
    let linked: SetMirror =
        bincode::deserialize(&linked).map_err(|e| format!("結んだ構文を読めない: {e}"))?;
    set.syntaxes
        .extend(linked.syntaxes.into_iter().skip(existing));
    bincode::serialize(&set).map_err(|e| format!("足した構文セットを直列化できない: {e}"))
}

/// 足す構文がほかの構文を名指す先
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Target {
    Scope(Scope),
    Name(String),
}

/// 足す構文が `push` / `set` / `embed` / `with_prototype` で名指すほかの構文。
/// ほかの構文の文脈を `include` する形は結べないので `Err`（モジュールの doc）
fn foreign_targets(defs: &[SyntaxDefinition]) -> Result<BTreeSet<Target>, String> {
    let target = |reference: &ContextReference| match reference {
        ContextReference::ByScope { scope, .. } => Some(Target::Scope(*scope)),
        ContextReference::File { name, .. } => Some(Target::Name(name.clone())),
        _ => None,
    };
    let mut out = BTreeSet::new();
    for def in defs {
        for (name, context) in &def.contexts {
            for pattern in &context.patterns {
                match pattern {
                    Pattern::Include(reference) => {
                        if target(reference).is_some() {
                            return Err(format!(
                                "{} の文脈 {name} がほかの構文を include している（足し方が未対応）",
                                def.name
                            ));
                        }
                    }
                    Pattern::Match(matcher) => {
                        if let MatchOperation::Push(refs) | MatchOperation::Set(refs) =
                            &matcher.operation
                        {
                            out.extend(refs.iter().filter_map(target));
                        }
                        out.extend(matcher.with_prototype.iter().filter_map(target));
                    }
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    use crate::syntax_prefilter::ReferenceMirror;

    fn mirror(set: &SyntaxSet) -> SetMirror {
        bincode::deserialize(&bincode::serialize(set).unwrap()).unwrap()
    }

    /// 文脈を展開した bincode（zlib の圧縮の強さに左右されない比較）
    fn raw(syntax: &ReferenceMirror) -> Vec<u8> {
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(&syntax.serialized_lazy_contexts[..])
            .read_to_end(&mut out)
            .unwrap();
        out
    }

    fn two_face() -> SyntaxSet {
        two_face::syntax::extra_newlines()
    }

    #[test]
    fn 同梱の構文はpowershellで拡張子はps1とpsm1とpsd1() {
        let defs = definitions().expect("build.rs が作った構文定義を読める");
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, NAMES);
        assert_eq!(defs[0].file_extensions, ["ps1", "psm1", "psd1"]);
        // two-face（fancy-regex 版）には無い = 足す意味がある
        for name in NAMES {
            assert!(two_face().find_syntax_by_name(name).is_none(), "{name}");
        }
    }

    /// 既存の 213 構文は名前・拡張子・変数・文脈のどれも 1 バイトも変わらない
    #[test]
    fn 既存の構文は1バイトも変えない() {
        let base = two_face();
        let added = appended(&base).expect("足せる");
        let (old, new) = (mirror(&base), mirror(&added));
        assert_eq!(new.syntaxes.len(), old.syntaxes.len() + NAMES.len());
        assert_eq!(old.path_syntaxes, new.path_syntaxes);
        for (o, n) in old.syntaxes.iter().zip(&new.syntaxes) {
            assert_eq!(o.name, n.name);
            assert_eq!(o.file_extensions, n.file_extensions, "{}", o.name);
            assert_eq!(o.scope, n.scope, "{}", o.name);
            assert_eq!(o.first_line_match, n.first_line_match, "{}", o.name);
            assert_eq!(o.hidden, n.hidden, "{}", o.name);
            assert_eq!(o.variables, n.variables, "{}", o.name);
            assert_eq!(
                o.serialized_lazy_contexts, n.serialized_lazy_contexts,
                "{} の文脈が変わった",
                o.name
            );
        }
        let tail: Vec<&str> = new.syntaxes[old.syntaxes.len()..]
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(tail, NAMES);
        // ほかの構文への参照がすべて結ばれている（結べない参照は塗る途中で panic する）
        let unlinked = added.find_unlinked_contexts();
        assert!(unlinked.is_empty(), "結ばれていない参照: {unlinked:?}");
    }

    /// 身代わりで結んだ足し方が、syntect の builder で全体を作り直したものと 1 バイトも違わない
    /// （身代わりの文脈の番号付けが本物と一致している = 参照先を取り違えていない）
    #[test]
    fn 足し方はbuilderで全体を作り直したものと一致する() {
        let mut builder = two_face().into_builder();
        for def in definitions().unwrap() {
            builder.add(def);
        }
        let rebuilt = mirror(&builder.build());
        let added = mirror(&appended(&two_face()).unwrap());
        assert_eq!(rebuilt.syntaxes.len(), added.syntaxes.len());
        for (a, b) in rebuilt.syntaxes.iter().zip(&added.syntaxes) {
            assert_eq!(a.name, b.name);
            assert_eq!(raw(a), raw(b), "{} の文脈が builder の結果と違う", a.name);
        }
    }

    /// 拡張子（大文字・ファイル名の綴りを含む）の解決は、足した構文の拡張子のほかは変わらない
    #[test]
    fn 拡張子の解決は足した構文のほか変わらない() {
        let (base, added) = (two_face(), appended(&two_face()).unwrap());
        let mut checked = 0;
        for syntax in base.syntaxes() {
            for ext in &syntax.file_extensions {
                for probe in [ext.clone(), ext.to_uppercase()] {
                    checked += 1;
                    assert_eq!(
                        base.find_syntax_by_extension(&probe).map(|s| &s.name),
                        added.find_syntax_by_extension(&probe).map(|s| &s.name),
                        "{probe}"
                    );
                }
            }
        }
        assert!(checked > 1000, "入力が縮退している: {checked}");
        for ext in ["ps1", "psm1", "psd1", "PS1"] {
            assert!(base.find_syntax_by_extension(ext).is_none(), "{ext}");
            assert_eq!(
                added.find_syntax_by_extension(ext).map(|s| s.name.as_str()),
                Some("PowerShell"),
                "{ext}"
            );
        }
    }

    /// 1 行目での判定（shebang）。`first_line_match` の変数を build.rs が展開していないと、
    /// 組めない正規表現で**どの形式の 1 行目の判定でも** panic する
    #[test]
    fn 一行目の判定はshebangのpwshを拾いほかの1行目は変えない() {
        let (base, added) = (two_face(), appended(&two_face()).unwrap());
        for line in [
            "#!/usr/bin/env pwsh",
            "#!/usr/local/bin/powershell -NoProfile",
        ] {
            assert_eq!(
                added
                    .find_syntax_by_first_line(line)
                    .map(|s| s.name.as_str()),
                Some("PowerShell"),
                "{line}"
            );
        }
        for line in [
            "#!/bin/bash",
            "#!/usr/bin/env python3",
            "<?php",
            "<?xml version=\"1.0\"?>",
            "# -*- mode: ruby -*-",
            "",
            "{{shebang_language}}",
        ] {
            assert_eq!(
                base.find_syntax_by_first_line(line).map(|s| &s.name),
                added.find_syntax_by_first_line(line).map(|s| &s.name),
                "{line}"
            );
        }
    }

    /// 足す構文の正規表現はすべて fancy-regex で組める（組めないと塗る途中で panic する。
    /// build.rs でも落とすが、実際に照合するエンジンでもう一度確かめる）
    #[test]
    fn 足す構文の正規表現はすべて組める() {
        let mut checked = 0;
        for def in definitions().unwrap() {
            for regex in def.first_line_match.iter() {
                assert!(!regex.contains("{{"), "変数が残っている: {regex}");
                assert!(
                    syntect::parsing::Regex::try_compile(regex).is_none(),
                    "{regex}"
                );
            }
            for context in def.contexts.values() {
                for pattern in &context.patterns {
                    if let Pattern::Match(m) = pattern {
                        checked += 1;
                        let regex = m.regex.regex_str();
                        assert!(
                            syntect::parsing::Regex::try_compile(regex).is_none(),
                            "{}: {regex}",
                            def.name
                        );
                    }
                }
            }
        }
        assert!(checked > 200, "正規表現が少なすぎる: {checked}");
    }

    /// ほかの構文の文脈を include する形は結べないので足さない（`Err`）
    #[test]
    fn ほかの構文をincludeする形は足さない() {
        // `include: scope:source.cs`（参照の型は crate の外で組めないので直列化の形から作る）
        let include: ContextReference = serde_json::from_str(
            r#"{"ByScope":{"scope":"source.cs","sub_context":null,"with_escape":false}}"#,
        )
        .unwrap();
        let mut main = Context::new(true);
        main.patterns.push(Pattern::Include(include));
        let def = SyntaxDefinition {
            name: "Probe".into(),
            file_extensions: vec!["probe".into()],
            scope: Scope::new("source.probe").unwrap(),
            first_line_match: None,
            hidden: false,
            variables: HashMap::new(),
            contexts: HashMap::from([("main".to_string(), main)]),
        };
        let err = append_bytes(&two_face(), vec![def]).unwrap_err();
        assert!(err.contains("include"), "{err}");
    }
}
