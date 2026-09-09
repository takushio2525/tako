//! 番犬: 実 claude を使う e2e は**事前信頼の後始末をする**（Issue #1022 / #612 / #577）
//!
//! これらの e2e（`#[ignore]` の手動実行専用）は、worker を**既定 config dir**で走らせるため
//! 事前信頼を実ファイル（`~/.claude/.claude.json`）へ書く。テストビルドの隔離（#944）は
//! 「config dir を明示した書き込み」までは倒さない（倒すと実 claude が信頼を読めず
//! e2e が成立しない）ので、**消すのは e2e 自身の責任**になる。
//!
//! 消さないと実行のたびにユーザーの生きた設定へ `/private/tmp/tako-e2e-<N>-<pid>/work` が
//! 積もる（#1022 の実測で 10 件・#1030 の実測では e2e 系だけで 65 件）。
//! `#[ignore]` なので CI は本体を走らせない = **この静的検査だけが再発を止められる**。

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルートを解決できない")
        .to_path_buf()
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(rust_files(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    out
}

/// `impl Drop for <名前> {` の本体（対応する `}` まで）を返す
fn drop_body<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    let at = text.find(marker)?;
    let rest = &text[at..];
    let open = rest.find('{')?;
    let mut depth = 0usize;
    for (i, c) in rest[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&rest[open..open + i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// 走査対象: `crates/*/src/**` にある `impl Drop for E2e…Guard`
fn e2e_guard_drops() -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for crate_dir in ["tako-core", "tako-control", "tako-app", "tako-cli"] {
        let src = repo_root().join("crates").join(crate_dir).join("src");
        for file in rust_files(&src) {
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            let mut from = 0usize;
            while let Some(found) = text[from..].find("impl Drop for E2e") {
                let at = from + found;
                let head_end = text[at..].find('{').map(|i| at + i).unwrap_or(text.len());
                let name = text[at + "impl Drop for ".len()..head_end]
                    .trim()
                    .to_string();
                from = head_end;
                if !name.ends_with("Guard") {
                    continue;
                }
                let Some(body) = drop_body(&text[at..], "impl Drop for") else {
                    continue;
                };
                let rel = file
                    .strip_prefix(repo_root())
                    .unwrap_or(&file)
                    .to_string_lossy()
                    .to_string();
                out.push((rel, name, body.to_string()));
            }
        }
    }
    out
}

/// **被覆**: e2e のガードは片付けで事前信頼のエントリも消すこと
#[test]
fn e2eのガードは事前信頼の後始末をしている() {
    let guards = e2e_guard_drops();
    assert!(
        !guards.is_empty(),
        "走査が 1 つも拾えていない（`impl Drop for E2e…Guard` の書き方を変えたら\n\
         この番犬も直す）"
    );
    let missing: Vec<String> = guards
        .iter()
        .filter(|(_, _, body)| !body.contains("remove_e2e_trust_entry"))
        .map(|(file, name, _)| format!("{file}: {name}"))
        .collect();
    assert!(
        missing.is_empty(),
        "事前信頼の後始末が無い e2e ガードがある: {missing:#?}\n\
         → `Drop` で `remove_e2e_trust_entry(&<信頼した cwd>)` を呼んでください\n\
           （消さないとユーザーの ~/.claude.json にテスト用パスが積もる。#1022 / #612）"
    );
}

/// 走査そのものが生きていること（対象を取り違えていない）
#[test]
fn 走査が実際にガードを拾えている() {
    let names: Vec<String> = e2e_guard_drops()
        .into_iter()
        .map(|(_, name, _)| name)
        .collect();
    for want in ["E2e571Guard", "E2e577Guard"] {
        assert!(
            names.iter().any(|n| n == want),
            "{want} を拾えていない（走査対象: {names:?}）"
        );
    }
}
