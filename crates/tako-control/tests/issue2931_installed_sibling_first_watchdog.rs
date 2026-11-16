//! tako の実行ファイルの解決で「入れた本体の隣」が PATH より先に効いていることの番犬（#2931）
//!
//! Windows には macOS の `/Applications/tako.app`（① の安定パス）に当たる置き場が無いので、
//! #2931 より前は `resolve_tako_binary` の PATH の段（`which tako`）が「実行中の本体の隣」より先に勝ち、
//! PATH に残った古い開発ビルドの `tako.exe` が MCP に登録された（11/16 の Windows 実機）。
//! いまは ① の次に「実行中の本体がインストーラーで入れた先に居れば隣」（境界 B20 の記録と比べる）を置く。
//!
//! この形は 3 か所が揃って初めて効く。どれか 1 つが崩れても単体テストの純関数は緑のまま
//! （純関数は引数で記録を受けるので、本番の呼び手が記録を渡し忘れても気づけない）なので、
//! ソースを読んで `file:line` で名指す:
//!
//! 1. **判定順**: `resolve_tako_binary_with` の本体で、入れた先の比較（`runs_from_install_dir(`）が
//!    ① の安定パス（`is_file(STABLE_APP_BINARY)`）より後ろ・PATH（`resolve_in_path()`）より前にある
//! 2. **配線**: 本番の `resolve_tako_binary_with(` の呼び出しがすべて `tako_install_location()` を渡す
//!    （`None` を渡すと Windows でも修正前の順に戻る）
//! 3. **記録と A/B**: `tako_install_location` が境界 B20 の `install_info::install_location` を読み、
//!    A/B（`legacy_2931()` = `TAKO_2931_LEGACY=1`）の腕を持つ
//! 4. **判定の正本は 1 つ**: 「インストーラーで入れた本体か」（`fn runs_from_install_dir`）を定義してよいのは
//!    境界 B20（`crates/tako-core/src/platform/install_info.rs`）だけ（アプリ内更新 #2434 と答えを揃える）
//!
//! 走査はテスト領域を潰した本番コードの、さらにコメントと文字列を潰した眺めで行う
//! （説明文や期待値の文字列を拾わない・行番号は保たれる）。

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::code_view;

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const B20: &str = "crates/tako-core/src/platform/install_info.rs";

/// 名指し 1 件
#[derive(Debug, PartialEq, Eq)]
struct Offender {
    file: String,
    line: usize,
    why: String,
}

impl std::fmt::Display for Offender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{} {}", self.file, self.line, self.why)
    }
}

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// バイト位置の行番号（1 始まり）
fn line_at(text: &str, at: usize) -> usize {
    text[..at.min(text.len())].matches('\n').count() + 1
}

/// `open`（`(` か `{`）の位置から、対応する閉じ括弧の直後までの範囲（コードだけの眺めの上で数える）
fn matching_close(code: &str, open_at: usize) -> Option<usize> {
    let bytes = code.as_bytes();
    let (open, close) = match bytes.get(open_at)? {
        b'(' => (b'(', b')'),
        b'{' => (b'{', b'}'),
        _ => return None,
    };
    let mut depth = 0usize;
    for (i, &b) in bytes.iter().enumerate().skip(open_at) {
        if b == open {
            depth += 1;
        } else if b == close {
            depth -= 1;
            if depth == 0 {
                return Some(i + 1);
            }
        }
    }
    None
}

/// 本番コードのコードだけの眺め（テスト領域・コメント・文字列を潰す。バイト長は保たれる）
fn production_code(src: &str, rel: &str) -> String {
    code_view(&production_range::production(src, rel))
}

/// 1〜3 の走査（`code` は [`production_code`] の眺め）
fn scan_dispatch(code: &str, rel: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let offend = |at: usize, why: String| Offender {
        file: rel.to_string(),
        line: line_at(code, at),
        why,
    };

    // 1. 判定順
    const DEF: &str = "fn resolve_tako_binary_with(";
    let defs: Vec<usize> = code.match_indices(DEF).map(|(at, _)| at).collect();
    match defs.as_slice() {
        [def] => {
            let body_open = code[*def..].find('{').map(|i| def + i);
            let body = body_open
                .and_then(|open| matching_close(code, open).map(|end| (open, end)))
                .map(|(open, end)| (open, &code[open..end]));
            match body {
                None => out.push(offend(
                    *def,
                    "本体の `{ … }` を読めない（走査が空振り）".into(),
                )),
                Some((open, body)) => {
                    let find = |needle: &str| body.find(needle).map(|i| open + i);
                    let stable = find("is_file(STABLE_APP_BINARY)");
                    let installed = find("runs_from_install_dir(");
                    let path = find("resolve_in_path()");
                    match (stable, installed, path) {
                        (_, None, _) => out.push(offend(
                            *def,
                            "判定順に「入れた本体の隣」の段（`runs_from_install_dir(`）が無い。\
                             Windows では PATH の別の tako.exe が入れた本体より先に勝つ（#2931）"
                                .into(),
                        )),
                        (_, _, None) => out.push(offend(
                            *def,
                            "PATH の段（`resolve_in_path()`）が見つからない（走査が空振り）".into(),
                        )),
                        (stable, Some(installed), Some(path)) => {
                            if installed > path {
                                out.push(offend(
                                    installed,
                                    "「入れた本体の隣」の段が PATH（`resolve_in_path()`）より後ろにある。\
                                     Windows では PATH の別の tako.exe が入れた本体より先に勝つ（#2931 の\
                                     修正前の順）"
                                        .into(),
                                ));
                            }
                            match stable {
                                None => out.push(offend(
                                    *def,
                                    "① の安定パス（`is_file(STABLE_APP_BINARY)`）が見つからない\
                                     （macOS の判定順を変えた）"
                                        .into(),
                                )),
                                Some(stable) if stable > installed => out.push(offend(
                                    installed,
                                    "「入れた本体の隣」の段が ① の安定パスより前にある（macOS の判定順を\
                                     変えた。#2931 の範囲外）"
                                        .into(),
                                )),
                                Some(_) => {}
                            }
                        }
                    }
                }
            }
        }
        [] => out.push(offend(
            0,
            format!("`{DEF}` が見つからない（改名なら番犬も追従させる）"),
        )),
        many => out.push(offend(many[1], format!("`{DEF}` が {} 個ある", many.len()))),
    }

    // 2. 配線: 本番の呼び出しは記録を渡す
    const CALL: &str = "resolve_tako_binary_with(";
    let mut calls = 0usize;
    for (at, _) in code.match_indices(CALL) {
        // 定義そのもの（`fn ` の直後）は数えない
        if code[..at].ends_with("fn ") {
            continue;
        }
        calls += 1;
        let open = at + CALL.len() - 1;
        let Some(end) = matching_close(code, open) else {
            out.push(offend(at, "呼び出しの `( … )` を読めない".into()));
            continue;
        };
        if !code[open..end].contains("tako_install_location()") {
            out.push(offend(
                at,
                "`resolve_tako_binary_with` の呼び出しが `tako_install_location()` を渡していない。\
                 入れた本体の隣の段が効かず、Windows で PATH の別の tako.exe を選ぶ（#2931）"
                    .into(),
            ));
        }
    }
    if calls == 0 {
        out.push(offend(
            0,
            "本番コードに `resolve_tako_binary_with(` の呼び出しが無い（走査が空振り）".into(),
        ));
    }

    // 3. 記録と A/B
    const LOC: &str = "fn tako_install_location(";
    match code.find(LOC) {
        None => out.push(offend(0, format!("`{LOC}` が見つからない"))),
        Some(at) => {
            let body = code[at..]
                .find('{')
                .map(|i| at + i)
                .and_then(|open| matching_close(code, open).map(|end| &code[open..end]));
            match body {
                None => out.push(offend(at, "本体の `{ … }` を読めない".into())),
                Some(body) => {
                    if !body.contains("install_info::install_location") {
                        out.push(offend(
                            at,
                            "境界 B20 の記録（`install_info::install_location`）を読んでいない"
                                .into(),
                        ));
                    }
                    if !body.contains("legacy_2931()") {
                        out.push(offend(
                            at,
                            "A/B（`legacy_2931()` = `TAKO_2931_LEGACY=1`）の腕が無い".into(),
                        ));
                    }
                }
            }
        }
    }
    out
}

/// 4. 「インストーラーで入れた本体か」の判定を定義するファイル（`(rel, code)` の並びから）
fn scan_single_source(files: &[(String, String)]) -> Vec<Offender> {
    const DEF: &str = "fn runs_from_install_dir(";
    let mut out = Vec::new();
    let mut found_in_b20 = false;
    for (rel, code) in files {
        for (at, _) in code.match_indices(DEF) {
            if rel == B20 {
                found_in_b20 = true;
            } else {
                out.push(Offender {
                    file: rel.clone(),
                    line: line_at(code, at),
                    why: format!(
                        "「インストーラーで入れた本体か」の判定を境界 B20（{B20}）の外で定義している。\
                         アプリ内更新（#2434）と CLI の解決（#2931）の答えがずれる。正本を `use` する"
                    ),
                });
            }
        }
    }
    if !found_in_b20 {
        out.push(Offender {
            file: B20.to_string(),
            line: 0,
            why: format!("正本の `{DEF}` が見つからない"),
        });
    }
    out
}

fn report(offenders: &[Offender]) -> String {
    offenders
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn 入れた本体の隣がpathより先に効く配線が揃っている() {
    let src = read(DISPATCH);
    let code = production_code(&src, DISPATCH);
    let offenders = scan_dispatch(&code, DISPATCH);
    assert!(
        offenders.is_empty(),
        "#2931 の判定順・配線が崩れた:\n{}",
        report(&offenders)
    );
}

#[test]
fn 入れた本体かの判定は境界b20の1実装() {
    let files: Vec<(String, String)> = production_range::product_sources()
        .into_iter()
        .map(|rel| {
            // 小さいファイル・テストの厚いファイルも読むので下限の確認は付けない（`platform_parity` と同じ）
            let code = code_view(&production_range::scan(&read(&rel)).text);
            (rel, code)
        })
        .collect();
    let offenders = scan_single_source(&files);
    assert!(offenders.is_empty(), "{}", report(&offenders));
}

// ---- 番犬そのものの検査（注入した断片を名指すこと）----

/// 修正後の形の最小の断片（行番号の検査のため 1 行ずつ）
const GOOD: &str = "\
fn tako_install_location() -> Option<PathBuf> {
    install_location_for_resolve(legacy_2931(), tako_core::platform::install_info::install_location)
}
fn resolve_tako_binary_with(a: A, b: B) -> String {
    if is_file(STABLE_APP_BINARY) { return x; }
    if current_exe.is_some_and(|e| runs_from_install_dir(loc, e)) { return y; }
    if let Some(p) = resolve_in_path() { return p; }
    z
}
pub fn resolve_tako_binary() -> String {
    resolve_tako_binary_with(a, b, c, d, tako_install_location().as_deref())
}
";

fn scan_snippet(snippet: &str) -> Vec<Offender> {
    scan_dispatch(&code_view(snippet), "snippet.rs")
}

#[test]
fn 番犬_修正後の形は通す() {
    assert_eq!(scan_snippet(GOOD), Vec::new());
}

#[test]
fn 番犬_入れた本体の段をpathの後ろへ戻すと名指す() {
    let injected = GOOD.replace(
        "    if current_exe.is_some_and(|e| runs_from_install_dir(loc, e)) { return y; }\n    if let Some(p) = resolve_in_path() { return p; }\n",
        "    if let Some(p) = resolve_in_path() { return p; }\n    if current_exe.is_some_and(|e| runs_from_install_dir(loc, e)) { return y; }\n",
    );
    assert_ne!(injected, GOOD, "注入が当たっていない");
    let got = scan_snippet(&injected);
    assert_eq!(got.len(), 1, "{}", report(&got));
    assert_eq!(got[0].line, 7, "{}", report(&got));
    assert!(got[0].why.contains("PATH"), "{}", report(&got));
}

#[test]
fn 番犬_入れた本体の段を消すと名指す() {
    let injected = GOOD.replace(
        "    if current_exe.is_some_and(|e| runs_from_install_dir(loc, e)) { return y; }\n",
        "",
    );
    let got = scan_snippet(&injected);
    assert_eq!(got.len(), 1, "{}", report(&got));
    assert_eq!(got[0].line, 4, "{}", report(&got));
}

#[test]
fn 番犬_入れた本体の段を安定パスより前へ出すと名指す() {
    let injected = GOOD.replace(
        "    if is_file(STABLE_APP_BINARY) { return x; }\n    if current_exe.is_some_and(|e| runs_from_install_dir(loc, e)) { return y; }\n",
        "    if current_exe.is_some_and(|e| runs_from_install_dir(loc, e)) { return y; }\n    if is_file(STABLE_APP_BINARY) { return x; }\n",
    );
    assert_ne!(injected, GOOD, "注入が当たっていない");
    let got = scan_snippet(&injected);
    assert_eq!(got.len(), 1, "{}", report(&got));
    assert!(got[0].why.contains("macOS"), "{}", report(&got));
}

#[test]
fn 番犬_呼び出しが記録を渡さないと名指す() {
    let injected = GOOD.replace(
        "resolve_tako_binary_with(a, b, c, d, tako_install_location().as_deref())",
        "resolve_tako_binary_with(a, b, c, d, None)",
    );
    let got = scan_snippet(&injected);
    assert_eq!(got.len(), 1, "{}", report(&got));
    assert_eq!(got[0].line, 11, "{}", report(&got));
}

#[test]
fn 番犬_ab_の腕を消すと名指す() {
    let injected = GOOD.replace("legacy_2931()", "false");
    let got = scan_snippet(&injected);
    assert_eq!(got.len(), 1, "{}", report(&got));
    assert!(got[0].why.contains("A/B"), "{}", report(&got));
}

#[test]
fn 番犬_コメントの中の呼び出しは数えない() {
    let with_comment = format!("// resolve_tako_binary_with(a, b, c, d, None)\n{GOOD}");
    assert_eq!(scan_snippet(&with_comment), Vec::new());
}

#[test]
fn 番犬_判定を境界の外で定義すると名指す() {
    let files = vec![
        (
            B20.to_string(),
            "pub fn runs_from_install_dir(a: A) -> bool { x }\n".into(),
        ),
        (
            "crates/tako-app/src/platform/installer_update.rs".to_string(),
            "\n\npub fn runs_from_install_dir(a: A) -> bool { y }\n".into(),
        ),
    ];
    let got = scan_single_source(&files);
    assert_eq!(got.len(), 1, "{}", report(&got));
    assert_eq!(
        (got[0].file.as_str(), got[0].line),
        ("crates/tako-app/src/platform/installer_update.rs", 3)
    );
    // 正本が消えても名指す
    let got = scan_single_source(&files[1..]);
    assert!(got.iter().any(|o| o.file == B20), "{}", report(&got));
}
