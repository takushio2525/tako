//! 番犬: tako-core の実 tmux テストが**同じ器の名前を 2 本で使わない**（Issue #1866）
//!
//! tako-core の lib テストは 1 つのプロセスで並列に走り、実 tmux のテストは
//! `TmuxTestGuard::new(vec![<器の名前>…])` で器を 1 本ずつ持つ（Drop で `kill-server` と
//! ソケットファイルの除去を行う）。名前がテスト同士で重なると、先に終わった側の Drop が
//! **隣のテストの生きている器を畳み、ソケットファイルまで消す**。残された側の tmux 呼び出しは
//! `error connecting to …(No such file or directory)` になり、kill-session が黙って届かず
//! 「セッションの終わりでクライアントも終わる」で落ちる（#1866 の
//! `tmux_backend::tests::issue1857_…` の落ち方と同じ形。ソケットファイルだけを消す注入で
//! 同じ行・同じ文言を再現した）。
//!
//! 今は全部一意。テストをコピーして増やしたときに器の名前の付け替えを忘れる形を、
//! **両方の file:line** を名指しして落とす。
//!
//! 見る形: `TmuxTestGuard::new(vec![a.clone(), b.clone()])` の各変数について、同じ関数の
//! 手前にある `let a = format!("<書式>", …)` の書式文字列。書式には pid（`{}`）が
//! 入っていること（入っていないと別プロセスの全体テストと取り合う = #1300 の型）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(reader) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in reader.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// 器 1 本の宣言（書式と、`let` の行番号 = 1 始まり）
#[derive(Debug, PartialEq, Eq)]
struct Socket {
    template: String,
    line: usize,
}

/// 名前を辿れなかった `TmuxTestGuard::new` の行（形が崩れた = 番犬が見られない）
#[derive(Debug, PartialEq, Eq)]
struct Untraced {
    var: String,
    line: usize,
}

/// `TmuxTestGuard::new(vec![…])` へ渡した変数を、手前の `let <変数> = format!("…"` まで辿る
fn guarded_sockets(src: &str) -> (Vec<Socket>, Vec<Untraced>) {
    let lines: Vec<&str> = src.lines().collect();
    let mut found = Vec::new();
    let mut untraced = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }
        let Some(at) = line.find("TmuxTestGuard::new(vec![") else {
            continue;
        };
        let args = &line[at + "TmuxTestGuard::new(vec![".len()..];
        let args = args.split(']').next().unwrap_or("");
        for var in args
            .split(',')
            .map(|a| a.trim().trim_end_matches(".clone()").trim())
            .filter(|a| !a.is_empty())
        {
            let needle = format!("let {var} = format!(\"");
            // 同じ関数の手前（テスト 1 本の中）だけを見る
            let traced = (0..i).rev().take(80).find_map(|j| {
                let at = lines[j].find(&needle)?;
                let rest = &lines[j][at + needle.len()..];
                let end = rest.find('"')?;
                Some(Socket {
                    template: rest[..end].to_string(),
                    line: j + 1,
                })
            });
            match traced {
                Some(socket) => found.push(socket),
                None => untraced.push(Untraced {
                    var: var.to_string(),
                    line: i + 1,
                }),
            }
        }
    }
    (found, untraced)
}

#[test]
fn tako_coreの実tmuxテストは器の名前を重ねない() {
    let root = repo_root();
    let mut files = Vec::new();
    collect(&root.join("crates/tako-core/src"), &mut files);
    files.sort();

    let mut by_template: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut problems: Vec<String> = Vec::new();
    for path in &files {
        let src = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("{} を読める: {e}", path.display()));
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string();
        let (sockets, untraced) = guarded_sockets(&src);
        for u in untraced {
            problems.push(format!(
                "  {rel}:{} `TmuxTestGuard::new` に渡した `{}` の名前を辿れない\
                 （手前に `let {} = format!(\"…\", std::process::id())` の形で置くこと）",
                u.line, u.var, u.var
            ));
        }
        for s in sockets {
            if !s.template.contains("{}") {
                problems.push(format!(
                    "  {rel}:{} 器の名前 `{}` に pid（`{{}}`）が無い（別プロセスと取り合う = #1300）",
                    s.line, s.template
                ));
            }
            by_template
                .entry(s.template)
                .or_default()
                .push(format!("{rel}:{}", s.line));
        }
    }
    for (template, places) in &by_template {
        if places.len() > 1 {
            problems.push(format!(
                "  器の名前 `{template}` を {} 本のテストが使っている（先に終わった側の Drop が \
                 隣の器を畳む）: {}",
                places.len(),
                places.join(" / ")
            ));
        }
    }
    // 走査そのものが空振りしていない（形が変わって 1 本も拾えないと、緑のまま見張りが消える）
    assert!(
        by_template.len() >= 20,
        "TmuxTestGuard の器を {} 本しか拾えない（走査の形が実装とずれている）",
        by_template.len()
    );
    assert!(
        problems.is_empty(),
        "tako-core の実 tmux テストの器の名前が重なっている・辿れない（#1866）:\n{}",
        problems.join("\n")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 渡した変数を手前のformatまで辿る() {
        let src = r#"
    fn a() {
        let backend = format!("tako-coretest-nestw-{}", std::process::id());
        let nested = format!("tako-coretest-nestw-in-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![backend.clone(), nested.clone()]);
    }
"#;
        let (found, untraced) = guarded_sockets(src);
        assert!(untraced.is_empty(), "{untraced:?}");
        assert_eq!(
            found,
            vec![
                Socket {
                    template: "tako-coretest-nestw-{}".into(),
                    line: 3
                },
                Socket {
                    template: "tako-coretest-nestw-in-{}".into(),
                    line: 4
                },
            ]
        );
    }

    #[test]
    fn 辿れない名前とコメントの行() {
        let src = r#"
    // TmuxTestGuard::new(vec![socket.clone()]) は説明文
    fn b(socket: String) {
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
    }
"#;
        let (found, untraced) = guarded_sockets(src);
        assert!(found.is_empty());
        assert_eq!(
            untraced,
            vec![Untraced {
                var: "socket".into(),
                line: 4
            }]
        );
    }
}
