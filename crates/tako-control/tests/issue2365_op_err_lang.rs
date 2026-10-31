//! 実経路: dispatch の git の書き込み系（commit / stage / pull / ブランチの切替と作成 / マージ / 中止）の断りが
//! 中身ごと表示言語に従う（Issue #2365 の続き = `op_err` が載せる中身）。
//!
//! 修正前（実測。CLI / MCP と同じ background の本体 = `OffloadJob::GitWrite` を一時のリポジトリで走らせて）:
//! 英語表示でも「ブランチ 'nope' が見つかりません」「ブランチ名に空白・制御文字は使えません」のように
//! 日本語のまま = dispatch が tako-core の `git.rs` の `Err(String)`（日本語の固定文）を `op_err` で素通ししていた。
//!
//! どのケースも一時のリポジトリの中で断られて何も変えない（ブランチ名の検査・無いブランチ・空のメッセージ・
//! 進行中でない中止）。git 自身の stderr（無いパスの stage・追跡先の無い pull）は外部の出力なので、
//! 日英とも同じ文をそのまま載せる（`LANG=C` で git の英語に固定して比べる）。
//!
//! 見るもの:
//!
//! - 英語表示: 断りに日本語（かな・漢字）が 1 字も無い
//! - 日本語表示: #2365 より前の文とバイト一致
//! - 表示言語を切り替えた直後の 1 件目から切り替えた言語で返る
//!
//! 表示言語と env はプロセス全体の状態なので、**1 本のテスト関数を `run_alone!` の子**で走らせる
//! （#2400）。このファイルは修正前の木でもコンパイルできる API だけを使う（A/B: 修正前は英語表示の
//! 検査で落ちる）。Web ビューの navigate / eval の断りは GUI（tako-app）の口なので、隔離 GUI へ CLI で叩く
//! 検証（PR 本文）と番犬 `issue1762_dispatch_error_lang.rs` の表で見る
use std::path::{Path, PathBuf};
use std::process::Command;

use tako_control::dispatch::{OffloadJob, OffloadOutcome};
use tako_control::protocol::Request;
use tako_core::i18n::{self, Lang};

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

struct Case {
    name: &'static str,
    request: Request,
    /// #2365 より前の日本語の断り（git の stderr は日英とも同じ文）
    old_ja: String,
}

fn case(name: &'static str, request: Request, old_ja: impl Into<String>) -> Case {
    Case {
        name,
        request,
        old_ja: old_ja.into(),
    }
}

fn checkout(branch: &str) -> Request {
    Request::GitCheckout {
        pane: None,
        branch: branch.into(),
        confirm: true,
    }
}

fn cases() -> Vec<Case> {
    vec![
        case(
            "commit（空のメッセージ）",
            Request::GitCommit {
                pane: None,
                message: "  ".into(),
                all: false,
            },
            "コミットメッセージが空です",
        ),
        case(
            "checkout（空のブランチ名）",
            checkout(""),
            "ブランチ名が空です",
        ),
        case(
            "checkout（- で始まる）",
            checkout("-d"),
            "ブランチ名を - で始めることはできません",
        ),
        case(
            "checkout（空白）",
            checkout("a b"),
            "ブランチ名に空白・制御文字は使えません",
        ),
        case(
            "checkout（使えない記号）",
            checkout("a~1"),
            "ブランチ名に ~ は使えません",
        ),
        case(
            "checkout（..）",
            checkout("a..b"),
            "ブランチ名に .. や @{ は使えません",
        ),
        case(
            "checkout（/ の使い方）",
            checkout("x/"),
            "ブランチ名の / の使い方が不正です",
        ),
        case(
            "checkout（.lock で終わる）",
            checkout("x.lock"),
            "ブランチ名を . や .lock で終わらせることはできません",
        ),
        case(
            "checkout（要素が . で始まる）",
            checkout("a/.b"),
            "ブランチ名の各要素を . で始めることはできません",
        ),
        case(
            "checkout（無いブランチ）",
            checkout("nope"),
            "ブランチ 'nope' が見つかりません",
        ),
        case(
            "branch（既にある）",
            Request::GitBranchCreate {
                pane: None,
                name: "main".into(),
                start_point: None,
                checkout: None,
            },
            "ブランチ 'main' はすでに存在します",
        ),
        case(
            "branch（無い基点）",
            Request::GitBranchCreate {
                pane: None,
                name: "topic".into(),
                start_point: Some("nope".into()),
                checkout: None,
            },
            "基点 'nope' が見つかりません",
        ),
        case(
            "merge（無いブランチ）",
            Request::GitMerge {
                pane: None,
                branch: "nope".into(),
                confirm: true,
                no_ff: false,
            },
            "ブランチ 'nope' が見つかりません",
        ),
        case(
            "merge abort（進行中の操作が無い）",
            Request::GitMergeAbort { pane: None },
            "中止できる操作が進行中ではありません",
        ),
        // 境界: git 自身の stderr（外部の出力）。tako は枠を付けないので日英とも同じ文
        case(
            "stage（無いパス = git の stderr）",
            Request::GitStage {
                pane: None,
                paths: vec!["nope.txt".into()],
            },
            "fatal: pathspec 'nope.txt' did not match any files",
        ),
    ]
}

fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git を起こせる");
    assert!(
        out.status.success(),
        "git {args:?} が失敗: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// 一時のリポジトリ（main に 1 コミット）
fn make_repo(root: &Path) -> PathBuf {
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).expect("リポジトリの dir");
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.name", "tako-test"]);
    git(
        &repo,
        &["config", "user.email", "tako-test@example.invalid"],
    );
    std::fs::write(repo.join("a.txt"), "a\n").expect("fixture");
    git(&repo, &["add", "a.txt"]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    repo
}

/// CLI / MCP の git の書き込み系が通る background の本体（`prepare_offload` が作るジョブ）を走らせる
fn run(repo: &Path, request: &Request) -> String {
    let job = OffloadJob::GitWrite {
        cwd: repo.to_path_buf(),
        request: Box::new(request.clone()),
    };
    match job.run_staged() {
        OffloadOutcome::Reply(Err(e)) => e.to_string(),
        OffloadOutcome::Reply(Ok(v)) => format!("<断られずに通った: {v}>"),
        OffloadOutcome::OnUi(_) => "<UI スレッドへの続きになった>".into(),
    }
}

fn branch_list(repo: &Path) -> String {
    let out = Command::new("git")
        .args(["branch", "--list"])
        .current_dir(repo)
        .output()
        .expect("git branch");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn gitの書き込み系の断りは中身ごと表示言語に従う() {
    if tako_core::run_alone!(gitの書き込み系の断りは中身ごと表示言語に従う) {
        return;
    }
    let root = std::env::temp_dir().join(format!("tako-2365g-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("一時の置き場");
    let root = root.canonicalize().expect("正規化");
    std::fs::create_dir_all(root.join("home")).expect("HOME");
    std::fs::create_dir_all(root.join("data")).expect("data dir");
    // 本番の設定・データと利用者の git の設定を読まない。git の出力は英語に固定（stderr の比較のため）
    std::env::set_var("HOME", root.join("home"));
    std::env::set_var("TAKO_DATA_DIR", root.join("data"));
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    std::env::set_var("LANG", "C");
    std::env::remove_var("LC_MESSAGES");
    let repo = make_repo(&root);
    let branches_before = branch_list(&repo);

    let original = i18n::lang();
    let mut failures = Vec::new();
    let mut ja_count = 0;
    let mut en_count = 0;
    for lang in [Lang::En, Lang::Ja] {
        i18n::set_lang(lang);
        for c in cases() {
            let got = run(&repo, &c.request);
            match lang {
                Lang::En => {
                    en_count += 1;
                    if got.starts_with('<') {
                        failures.push(format!("[en] {}: 断られなかった: {got}", c.name));
                    } else if let Some(ch) = has_japanese(&got) {
                        failures.push(format!("[en] {}: 日本語（{ch}）が混ざる: {got}", c.name));
                    }
                }
                Lang::Ja => {
                    ja_count += 1;
                    if got != c.old_ja {
                        failures.push(format!(
                            "[ja] {}: #2365 より前と違う\n  want: {}\n  got:  {got}",
                            c.name, c.old_ja
                        ));
                    }
                }
            }
        }
    }

    // 表示言語を切り替えた直後の 1 件目から、切り替えた言語で返る（毎回切り替えて 1 件ずつ叩く）
    let probe = checkout("nope");
    for (i, lang) in [Lang::Ja, Lang::En, Lang::Ja, Lang::En]
        .into_iter()
        .enumerate()
    {
        i18n::set_lang(lang);
        let got = run(&repo, &probe);
        let ok = match lang {
            Lang::Ja => got == "ブランチ 'nope' が見つかりません",
            Lang::En => has_japanese(&got).is_none() && got.contains("'nope'"),
        };
        if !ok {
            failures.push(format!(
                "切り替え {i} 回目（{lang:?}）の直後の 1 件目: {got}"
            ));
        }
    }
    i18n::set_lang(original);

    // 断るだけで何も変えていない（ブランチの一覧が同じ）
    let branches_after = branch_list(&repo);
    if branches_before != branches_after {
        failures.push(format!(
            "断ったはずの要求がリポジトリを変えた:\n  before: {branches_before}\n  after:  {branches_after}"
        ));
    }
    let _ = std::fs::remove_dir_all(&root);
    eprintln!(
        "issue2365_op_err_lang: 英語 {en_count} 件・日本語 {ja_count} 件・失敗 {}",
        failures.len()
    );
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
