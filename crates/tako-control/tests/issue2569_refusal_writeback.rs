//! 実経路: 断った台帳・プロジェクト・アカウントの操作は置き場を書かない（Issue #2569）。
//!
//! 修正前（実測。`scripts/test-ledger-writeback-2569.sh` の CLI / MCP の日英）: 無いエントリへの
//! `ledger record` / `amend`、無いキーの `projects remove`、無い名前の `accounts remove` は断りを返すのに、
//! 読み → 変更 → 書きの「書き」を閉包の値によらず行っていた。台帳が無い置き場には 0 件の
//! `ledger.yaml`（`projects.yaml` / `accounts.yaml`）ができ、手で書いたファイルは tako の形へ書き直されて
//! コメントが消え、世代バックアップ（`.bak.1`）が 1 つ回った。
//!
//! ここでは CLI と MCP が共通で通る関数（台帳は `ledger::record_outcome_typed` /
//! `amend_entry_typed`、プロジェクトとアカウントは dispatch の関数）を直に呼び、
//!
//! - 台帳の無い置き場: 断りが返り、ファイルを作らない
//! - 手で書いたファイルの置き場: 断りが返り、バイトも mtime も変えず、`.bak.1` を作らない
//! - 断らない操作（あるキーの remove）は従来どおり書く
//!
//! を見る。設定の置き場（`TAKO_ORCHESTRATOR_DIR`）と HOME / data dir を一時の場所へ向けるので、
//! env を書き換えるテスト関数は `run_alone!` の子で走らせる（#2400）。
//!
//! 番犬（[`断る閉包を書き戻す口へ渡さない`]）: 閉包の値によらず書き戻す口（`Ledger` / `ProjectsConfig` /
//! `AccountsConfig` の `mutate` 系）へ、断る閉包（`?` / `Err(` / `.ok_or`）を渡している呼び出しを
//! `file:line` で名指して落とす。断るなら `try_mutate_typed`（閉包が `Err` なら書き戻さない）を使う

use std::path::{Path, PathBuf};

#[path = "common/code_view.rs"]
mod code_view;

use tako_control::dispatch::dispatch_orchestrator_projects;
use tako_control::dispatch_orchestrator_accounts;
use tako_control::orchestrator::ledger;

/// 手で書いたファイル（コメントと flow 形式 = tako が書く形とバイトが違う。書き戻すと形が変わる）
const HAND_LEDGER: &str = "# 手で書いた台帳（#2569）\nentries:\n  - {id: spawn_a, ts: t, project: tako, task_type: docs, model: opus}\n";
const HAND_PROJECTS: &str = "# 手で書いたプロジェクト一覧（#2569）\nversion: 2\nprojects:\n  alpha: {cwd: /tmp/alpha2569}\n";
const HAND_ACCOUNTS: &str =
    "# 手で書いたアカウント一覧（#2569）\naccounts:\n  main2569: {inherit: true}\n";

/// ファイルの指紋（中身・mtime）。無ければ None
fn fingerprint(path: &Path) -> Option<(Vec<u8>, std::time::SystemTime)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((std::fs::read(path).ok()?, meta.modified().ok()?))
}

/// 1 つの置き場で、断る操作を撃っても 3 つのファイルが書かれないことを見る。失敗は文で返す
fn refusals_write_nothing(dir: &Path) -> Vec<String> {
    let files = ["ledger.yaml", "projects.yaml", "accounts.yaml"];
    let before: Vec<_> = files.iter().map(|f| fingerprint(&dir.join(f))).collect();
    let mut failures = Vec::new();
    let refusals: [(&str, Result<(), String>); 5] = [
        (
            "ledger record",
            ledger::record_outcome_typed("nope2569", "pass", Some(1), Some("n"))
                .map_err(|e| e.to_string()),
        ),
        (
            "ledger amend",
            ledger::amend_entry_typed("nope2569", "x").map_err(|e| e.to_string()),
        ),
        (
            "ledger record（不正な outcome）",
            ledger::record_outcome_typed("spawn_a", "bogus2569", None, None)
                .map_err(|e| e.to_string()),
        ),
        (
            "projects remove",
            dispatch_orchestrator_projects("remove", Some("nope2569".into()), None, None)
                .map(|_| ())
                .map_err(|e| e.to_string()),
        ),
        (
            "accounts remove",
            dispatch_orchestrator_accounts(
                "remove",
                Some("nope2569"),
                None,
                None,
                None,
                None,
                None,
            )
            .map(|_| ())
            .map_err(|e| e.to_string()),
        ),
    ];
    for (name, got) in refusals {
        match got {
            Err(text) if text.contains("2569") => {}
            other => failures.push(format!(
                "{}: {name} が断られない / 名指さない: {other:?}",
                dir.display()
            )),
        }
    }
    for (file, was) in files.iter().zip(&before) {
        let path = dir.join(file);
        let now = fingerprint(&path);
        if &now != was {
            let what = if was.is_none() {
                "作った"
            } else {
                "書き換えた"
            };
            failures.push(format!("断ったのに{what}: {}", path.display()));
        }
        let mut bak = path.clone().into_os_string();
        bak.push(".bak.1");
        if std::path::Path::new(&bak).exists() {
            failures.push(format!("断ったのに世代を回した: {}", bak.to_string_lossy()));
        }
    }
    failures
}

#[test]
fn 断った台帳とプロジェクトとアカウントの操作は置き場を書かない() {
    if tako_core::run_alone!(断った台帳とプロジェクトとアカウントの操作は置き場を書かない)
    {
        return;
    }
    let root = tako_core::test_residue::ScratchDir::new("tako-2569");
    // 本番の設定・データを読み書きしない（どの置き場も一時の場所の下）
    std::env::set_var("HOME", root.path().join("home"));
    std::env::set_var("TAKO_DATA_DIR", root.path().join("data"));
    // 断りの文は表示言語に依らず名指しの id / キー（`2569`）を含むので、言語は既定のまま
    let mut failures = Vec::new();

    // 台帳・一覧の無い置き場
    let absent = root.path().join("absent");
    std::fs::create_dir_all(&absent).unwrap();
    std::env::set_var("TAKO_ORCHESTRATOR_DIR", &absent);
    failures.extend(refusals_write_nothing(&absent));

    // 手で書いたファイルの置き場
    let hand = root.path().join("hand");
    std::fs::create_dir_all(&hand).unwrap();
    std::fs::write(hand.join("ledger.yaml"), HAND_LEDGER).unwrap();
    std::fs::write(hand.join("projects.yaml"), HAND_PROJECTS).unwrap();
    std::fs::write(hand.join("accounts.yaml"), HAND_ACCOUNTS).unwrap();
    std::env::set_var("TAKO_ORCHESTRATOR_DIR", &hand);
    failures.extend(refusals_write_nothing(&hand));

    // 断らない操作は従来どおり書く（あるキー・あるエントリ）
    ledger::record_outcome_typed("spawn_a", "pass", None, None).expect("あるエントリの record");
    dispatch_orchestrator_projects("remove", Some("alpha".into()), None, None)
        .expect("あるキーの projects remove");
    dispatch_orchestrator_accounts("remove", Some("main2569"), None, None, None, None, None)
        .expect("ある名前の accounts remove");
    let read = |f: &str| std::fs::read_to_string(hand.join(f)).unwrap_or_default();
    if !read("ledger.yaml").contains("outcome: pass") {
        failures.push(format!(
            "record が台帳へ書かれない: {}",
            read("ledger.yaml")
        ));
    }
    if read("projects.yaml").contains("alpha") {
        failures.push(format!(
            "projects remove が書かれない: {}",
            read("projects.yaml")
        ));
    }
    if read("accounts.yaml").contains("main2569") {
        failures.push(format!(
            "accounts remove が書かれない: {}",
            read("accounts.yaml")
        ));
    }

    assert!(
        failures.is_empty(),
        "断った操作が置き場を書いた（#2569）:\n{}",
        failures.join("\n")
    );
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルートを解決できない")
        .to_path_buf()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// 閉包の値によらず書き戻す口（#2569）。`try_mutate_typed` / `try_mutate_at_typed` は綴りが違うので当たらない
const WRITE_BACK_CALLS: &[&str] = &[
    "Ledger::mutate(",
    "Ledger::mutate_typed(",
    "Ledger::mutate_at(",
    "Ledger::mutate_at_typed(",
    "ProjectsConfig::mutate(",
    "ProjectsConfig::mutate_typed(",
    "ProjectsConfig::mutate_at(",
    "ProjectsConfig::mutate_at_typed(",
    "AccountsConfig::mutate(",
    "AccountsConfig::mutate_typed(",
    "AccountsConfig::mutate_at(",
    "AccountsConfig::mutate_at_typed(",
];

/// 閉包の中の断りの印（コードだけの眺めで見る = コメント・文字列は潰してある）
const REFUSAL_MARKS: &[&str] = &["?", "Err(", ".ok_or"];

/// `open`（`(` の位置）に対応する `)` までの中身
fn call_args(code: &str, open: usize) -> &str {
    let mut depth = 0usize;
    for (i, c) in code[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return &code[open + 1..open + i];
                }
            }
            _ => {}
        }
    }
    &code[open..]
}

/// 書き戻す口へ断る閉包を渡している呼び出し（`rel:line 呼び出し 印`）
fn refusing_write_backs(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    for dir in [
        "crates/tako-control/src",
        "crates/tako-cli/src",
        "crates/tako-app/src",
    ] {
        rust_files(&root.join(dir), &mut files);
    }
    files.sort();
    let mut hits = Vec::new();
    for path in files {
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        let code = code_view::code_view(&src);
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string();
        for call in WRITE_BACK_CALLS {
            for (at, _) in code.match_indices(call) {
                let args = call_args(&code, at + call.len() - 1);
                if let Some(mark) = REFUSAL_MARKS.iter().find(|m| args.contains(**m)) {
                    let line = code[..at].matches('\n').count() + 1;
                    hits.push(format!("{rel}:{line} {call}…) の閉包に `{mark}`"));
                }
            }
        }
    }
    hits
}

/// 番犬: 閉包の値によらず書き戻す口へ、断る閉包を渡さない（#2569）。
/// 渡すと、断ったのに読んだ中身を書き戻す（無いファイルを作る・手で書いた形を変える・世代を回す）
#[test]
fn 断る閉包を書き戻す口へ渡さない() {
    let hits = refusing_write_backs(&repo_root());
    assert!(
        hits.is_empty(),
        "閉包の値によらず書き戻す口へ断る閉包を渡している（断るなら try_mutate_typed を使う。#2569）:\n{}",
        hits.join("\n")
    );
}

/// 番犬の自己検査: 断る閉包を渡す形は当たり、断らない閉包・`try_mutate_typed` は当たらない
#[test]
fn 番犬は断る閉包の形を見分ける() {
    let refusing = "Ledger::mutate_typed(|l| {\n    let e = l.find_mut(id).ok_or_else(|| x())?;\n    Ok(())\n})?";
    let plain = "Ledger::mutate_typed(|l| l.prune_by_project_prefix(&p)).map_err(f)?";
    let tried = "Ledger::try_mutate_typed(|l| { l.find_mut(id).ok_or_else(|| x())?; Ok(()) })?";
    let found = |src: &str| {
        let code = code_view::code_view(src);
        WRITE_BACK_CALLS.iter().any(|call| {
            code.match_indices(call).any(|(at, _)| {
                let args = call_args(&code, at + call.len() - 1);
                REFUSAL_MARKS.iter().any(|m| args.contains(*m))
            })
        })
    };
    assert!(found(refusing), "断る閉包を見逃した");
    assert!(
        !found(plain),
        "断らない閉包（呼び出しの外の `?`）を誤検知した"
    );
    assert!(!found(tried), "try_mutate_typed を誤検知した");
}
