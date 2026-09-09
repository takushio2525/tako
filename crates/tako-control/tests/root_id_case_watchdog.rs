//! 番犬: ルート id を**大文字化して「未知の id」の代わりに使っている**コードが無い（#1229）
//!
//! ## なぜ止めるのか
//!
//! `remote_files::root_id_of` はパスから **12 桁の小文字 16 進**を作る。
//! 16 進 12 桁が全部数字になる割合は (10/16)^12 ≒ **1/280** あるので、
//! 「id を大文字化すれば必ず別の綴りになる」という前提は成り立たない。
//!
//! `remote_files.rs` の `ツリーに出ていないルートは拒否される` はこの前提で
//! `fx.root_id().to_uppercase()` を「未知の id」として渡していた。fixture の
//! パスは pid を含む = 実行のたびに id の綴りが変わるので、数字だけの id を
//! 引いた回だけ**実在の id を渡したことになり**、`未知のルートが素通りした` で
//! 落ちていた（実測: 1000 回中 4 回）。
//!
//! ## 何を違反とするか（**誤検知しない形**）
//!
//! id を指す式の**直後**に `.to_uppercase()` / `.to_lowercase()` を繋いだ形だけを落とす:
//!
//! - `…root_id().to_uppercase()`
//! - `….id.to_uppercase()`
//! - `root_id_of(…).to_uppercase()`
//!
//! 直し方は 2 つ。**綴り違いが要るだけ**なら長さで必ず外れる形
//! （1 文字短い / 1 文字長い）を使う。**大小文字の区別を見たい**なら
//! 英字を含む id を明示して据える（`ルートidの照合は大小文字を区別する`）。
//!
//! いったん名前へ束縛してから大文字化する形は対象外にしてある。
//! 「数字だけの id は大文字化しても変わらない」ことを**わざと**確かめる
//! `root_idは小文字16進で数字だけにもなりうる` がその形だから

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

/// `crates/*/src/` と `crates/*/tests/` 配下の `.rs`（この番犬自身は除く）
fn sources() -> Vec<PathBuf> {
    let me = Path::new(file!())
        .file_name()
        .expect("自分のファイル名")
        .to_os_string();
    let mut out = Vec::new();
    let Ok(crates) = std::fs::read_dir(repo_root().join("crates")) else {
        return out;
    };
    for entry in crates.flatten() {
        let mut stack = vec![entry.path().join("src"), entry.path().join("tests")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                let path = e.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|x| x == "rs")
                    && path.file_name() != Some(me.as_os_str())
                {
                    out.push(path);
                }
            }
        }
    }
    out.sort();
    out
}

/// `//` から行末までを空白へ潰す（**バイト長を変えない** = 行番号がそのまま数えられる）。
/// 文字列中の `//` も潰すが、潰すのは常に「消す」方向なので偽の違反は増えない
fn without_comments(src: &str) -> String {
    src.lines()
        .map(|line| match line.find("//") {
            Some(i) => format!("{}{}", &line[..i], " ".repeat(line.len() - i)),
            None => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `head` の末尾が「ルート id を指す式」か
fn ends_with_root_id(head: &str) -> bool {
    let t = head.trim_end();
    if t.ends_with("root_id()") || t.ends_with(".id") {
        return true;
    }
    // `root_id_of(…)` の直後（引数はパス 1 本なので近くに必ず現れる）。
    // 日本語のコードが多いので、末尾 120 **文字**を char 境界で切り出す
    let tail = t
        .char_indices()
        .rev()
        .take(120)
        .last()
        .map(|(i, _)| &t[i..])
        .unwrap_or(t);
    t.ends_with(')') && tail.contains("root_id_of(")
}

#[test]
fn ルートidを大文字化して未知のidの代わりに使っていない() {
    let mut offenders: Vec<String> = Vec::new();
    for path in sources() {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let code = without_comments(&raw);
        for needle in [".to_uppercase()", ".to_lowercase()"] {
            let mut from = 0usize;
            while let Some(rel) = code[from..].find(needle) {
                let at = from + rel;
                if ends_with_root_id(&code[..at]) {
                    let line = code[..at].matches('\n').count() + 1;
                    let rel_path = path
                        .strip_prefix(repo_root())
                        .unwrap_or(&path)
                        .display()
                        .to_string();
                    offenders.push(format!(
                        "{rel_path}:{line}: {}",
                        code[..at + needle.len()]
                            .rsplit('\n')
                            .next()
                            .unwrap_or_default()
                            .trim()
                    ));
                }
                from = at + needle.len();
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "id は 12 桁の小文字 16 進なので約 1/280 で数字だけになり、大文字化しても\n\
         同じ綴り = 実在の id のままになる（#1229）。綴り違いが要るだけなら長さで\n\
         外れる形を、大小文字の区別を見たいなら英字を含む id を据えること:\n  {}",
        offenders.join("\n  ")
    );
}

/// 走査先を取り違えて**何も見ていない番犬**になっていないこと
#[test]
fn 番犬が走査対象を見つけている() {
    let files = sources();
    assert!(
        files.len() > 50,
        "crates 配下の .rs をほとんど拾えていない: {} 件",
        files.len()
    );
    assert!(
        files
            .iter()
            .any(|p| p.ends_with("tako-control/src/remote_files.rs")),
        "#1229 の現場（remote_files.rs）が走査対象に入っていない"
    );
    // 検出そのものが効くこと（本物のソースには無い形を合成して確かめる）
    assert!(ends_with_root_id("let x = fx.root_id()"));
    assert!(ends_with_root_id("let x = root.id"));
    assert!(ends_with_root_id("let x = root_id_of(&path)"));
    assert!(!ends_with_root_id("let x = names.first().unwrap()"));
    assert!(!ends_with_root_id("let x = name"));
}
