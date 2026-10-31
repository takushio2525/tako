//! 実経路: GUI だけが呼ぶ境界（更新の `.app` の差し替え = tako-core の `bundle_install`・OS のファイルの
//! クリップボード = `platform::file_clipboard`）の断りが表示言語に従う（Issue #2365 の続き・GUI 側）。
//!
//! 修正前（実測）: 英語表示でも「差し替え元が見つかりません: …」「置き場のパスが不正です: …」
//! 「作業用ディレクトリの作成に失敗: …」「書くファイルが無い」のように日本語のまま = 境界が日本語の
//! 固定文の `Err(String)` を返し、GUI の更新の断り（`UpdateRefusal::BundleReplace`）とファイルのコピーの
//! 応答の `os_note` がそれを素通ししていた。
//!
//! 差し替えは使い捨ての置き場（`ScratchDir`）の中だけで起こす（本物の `/Applications` に触らない。
//! 置き場のパスが不正な見本 `/` は、名前を取り出せずに手前で断る = 何も書かない）。クリップボードは
//! 書くファイルが無い断り（OS のクリップボードへ届く前に断る = ユーザーのクリップボードに触らない）。
//!
//! - 英語表示: 断りに日本語（かな・漢字）が 1 字も無い
//! - 日本語表示: #2365 より前の文とバイト一致
//! - 表示言語を切り替えた直後の 1 件目から、切り替えた言語で返る
//!
//! 表示言語はプロセス全体の状態なので、**1 本のテスト関数を `run_alone!` の子**で走らせる（#2400）。
//! このファイルは修正前の木でもコンパイルできる API（`Err` の `to_string()`）だけを使う
//! （A/B: 修正前は英語表示の検査で落ちる）。
//!
//! unix だけで走らせる: 書き込めない置き場（パーミッション 0555）が unix の前提
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tako_core::i18n::{self, Lang};
use tako_core::platform::bundle_install::replace_bundle_in_place;
use tako_core::test_residue::ScratchDir;

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

const EACCES: &str = "Permission denied (os error 13)";

/// 一時の置き場
struct Ctx {
    /// 差し替え元（中身のある `.app` の形）
    new_bundle: PathBuf,
    /// 無い差し替え元
    missing: PathBuf,
    /// 書き込めない親の下の置き場（作業用ディレクトリを作れない）
    ro_dest: PathBuf,
    /// 権限で断る経路を作れるか（root ではパーミッションが効かない）
    perms: bool,
}

struct Case {
    name: &'static str,
    /// 断りの文（断られなかったら `<ok>`）
    act: fn(&Ctx) -> String,
    /// #2365 より前の日本語の文
    old: fn(&Ctx) -> String,
    /// 権限の経路か（root では外す）
    needs_perms: bool,
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

fn refusal<T, E: ToString>(r: Result<T, E>) -> String {
    match r {
        Ok(_) => "<ok>".to_string(),
        Err(e) => e.to_string(),
    }
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "差し替え（差し替え元が無い）",
            act: |c| refusal(replace_bundle_in_place(&c.ro_dest, &c.missing)),
            old: |c| format!("差し替え元が見つかりません: {}", s(&c.missing)),
            needs_perms: false,
        },
        Case {
            name: "差し替え（置き場のパスが不正 = 名前が無い）",
            act: |c| refusal(replace_bundle_in_place(Path::new("/"), &c.new_bundle)),
            old: |_| "置き場のパスが不正です: /".to_string(),
            needs_perms: false,
        },
        Case {
            name: "差し替え（作業用ディレクトリを作れない）",
            act: |c| refusal(replace_bundle_in_place(&c.ro_dest, &c.new_bundle)),
            old: |c| {
                let parent = c.ro_dest.parent().expect("親");
                format!(
                    "作業用ディレクトリの作成に失敗: {EACCES}（{} に書き込めますか）",
                    s(parent)
                )
            },
            needs_perms: true,
        },
        Case {
            name: "ファイルのクリップボード（書くファイルが無い）",
            act: |_| refusal(tako_control::platform::file_clipboard::write(&[], false)),
            old: |_| "書くファイルが無い".to_string(),
            needs_perms: false,
        },
    ]
}

fn set_mode(p: &Path, mode: u32) {
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).expect("権限を変える");
}

fn setup(root: &Path) -> Ctx {
    let root = root.canonicalize().expect("正規化");
    let new_bundle = root.join("new").join("tako.app");
    std::fs::create_dir_all(new_bundle.join("Contents/MacOS")).expect("差し替え元");
    std::fs::write(new_bundle.join("Contents/version.txt"), "0.0.0").expect("版数");
    let ro = root.join("ro");
    std::fs::create_dir_all(&ro).expect("書き込めない親");
    set_mode(&ro, 0o555);
    // 権限で断る経路を作れるか（root ではパーミッションが効かない）
    let perms = std::fs::create_dir(ro.join("probe")).is_err();
    if !perms {
        let _ = std::fs::remove_dir(ro.join("probe"));
    }
    Ctx {
        new_bundle,
        missing: root.join("missing").join("tako.app"),
        ro_dest: ro.join("tako.app"),
        perms,
    }
}

#[test]
fn gui側の境界の断りは表示言語に従う() {
    if tako_core::run_alone!(gui側の境界の断りは表示言語に従う) {
        return;
    }
    let scratch = ScratchDir::new("t2365gui");
    let ctx = setup(scratch.path());
    if !ctx.perms {
        eprintln!("パーミッションが効かない（root）ので権限のケースを外す");
    }
    let cases: Vec<Case> = cases()
        .into_iter()
        .filter(|c| ctx.perms || !c.needs_perms)
        .collect();

    let original = i18n::lang();
    let mut failures = Vec::new();
    let mut by_lang: Vec<Vec<String>> = Vec::new();
    for lang in [Lang::En, Lang::Ja] {
        i18n::set_lang(lang);
        let mut seen = Vec::new();
        for c in &cases {
            let got = (c.act)(&ctx);
            println!("[{lang:?}] {}: {got}", c.name);
            if got == "<ok>" {
                failures.push(format!("[{lang:?}] {}: 断られていない", c.name));
            }
            match lang {
                Lang::En => {
                    if let Some(ch) = has_japanese(&got) {
                        failures.push(format!("[en] {}: 日本語（{ch}）が混ざる: {got}", c.name));
                    }
                }
                Lang::Ja => {
                    let old = (c.old)(&ctx);
                    if got != old {
                        failures.push(format!(
                            "[ja] {}: #2365 より前と違う\n  want: {old}\n  got:  {got}",
                            c.name
                        ));
                    }
                }
            }
            seen.push(got);
        }
        by_lang.push(seen);
    }
    // 英語表示の文は日本語表示の文と違う（英語の書き忘れで日本語を写していない）
    for ((c, en), ja) in cases.iter().zip(&by_lang[0]).zip(&by_lang[1]) {
        if en == ja {
            failures.push(format!("{}: 日英が同じ文: {en}", c.name));
        }
    }
    // 表示言語を切り替えた直後の 1 件目から、切り替えた言語で返る（毎回切り替えて 1 件ずつ叩く）
    for (i, lang) in [Lang::Ja, Lang::En, Lang::Ja, Lang::En, Lang::Ja, Lang::En]
        .into_iter()
        .enumerate()
    {
        i18n::set_lang(lang);
        let c = &cases[i % cases.len()];
        let got = (c.act)(&ctx);
        let ok = match lang {
            Lang::Ja => got == (c.old)(&ctx),
            Lang::En => has_japanese(&got).is_none() && got != "<ok>",
        };
        if !ok {
            failures.push(format!(
                "切り替え {i} 回目（{lang:?}）の直後の 1 件目（{}）: {got}",
                c.name
            ));
        }
    }
    i18n::set_lang(original);
    // 書き込めない親を戻してから片付ける（ScratchDir の Drop が消せるように）
    if let Some(parent) = ctx.ro_dest.parent() {
        set_mode(parent, 0o755);
    }
    // 差し替えは置き場（書き込めない親）に何も残していない
    let leftovers: Vec<_> = std::fs::read_dir(ctx.ro_dest.parent().expect("親"))
        .expect("読める")
        .flatten()
        .map(|e| e.file_name())
        .collect();
    if !leftovers.is_empty() {
        failures.push(format!(
            "断った差し替えが置き場に残骸を残した: {leftovers:?}"
        ));
    }
    eprintln!(
        "issue2365_gui_side_lang: ケース {} 件 × 日英・失敗 {}",
        cases.len(),
        failures.len()
    );
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
