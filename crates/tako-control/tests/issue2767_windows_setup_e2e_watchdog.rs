//! **#2767 の番犬**: Windows の CI の `tako setup` の通し検査とインストーラーの一巡が、
//! 正本からずれず CI から外れない。
//!
//! 検査の本体は Windows でしか走らない（`scripts/windows/test-setup-windows-2767.ps1` と
//! `installer/windows/lib/verify-assets.ps1` の `Test-TakoInstalledPayload`。ユーザー環境変数 Path を
//! レジストリで読み戻す）。macOS の開発機では 1 行も動かないので、ここで次を固定する:
//!
//! - 初回起動の自動の段の写し（スクリプトの `$AutoSetupAnswers` と起こし方）が、GUI の正本
//!   （`welcome::auto_setup_answers` と `dispatch::run_setup_cli_within` の引数）と同じ答えを渡す。
//!   正本だけが変わると、Windows の CI は GUI が実際には渡さない答えで緑のまま回り続ける
//! - 2 つの検査が ci.yml の Windows のジョブに載っている
//! - インストーラーの一巡が Path への追加のタスクを選び、検査の後に Path を戻す

use std::path::{Path, PathBuf};

use tako_control::setup::SetupAnswers;

const SCRIPT: &str = "scripts/windows/test-setup-windows-2767.ps1";
const VERIFY_ASSETS: &str = "installer/windows/lib/verify-assets.ps1";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const CI: &str = ".github/workflows/ci.yml";

/// GUI の自動の段が `tako setup` へ渡す引数（`dispatch::run_setup_cli_within` の字面）
const GUI_SETUP_ARGS: &str = r#".args(["setup", "--yes", "--answers", "-"])"#;
/// スクリプトの写し（同じ 4 つを同じ順で）
const SCRIPT_SETUP_ARGS: &str = "@('setup', '--yes', '--answers', '-')";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

/// `$Name = '<単引用符の文字列>'` の中身と行番号（PowerShell の `''` は `'`）
fn ps_single_quoted(text: &str, name: &str) -> Option<(usize, String)> {
    let prefix = format!("${name} = '");
    text.lines().enumerate().find_map(|(i, line)| {
        let rest = line.trim_start().strip_prefix(&prefix)?;
        let body = rest.trim_end().strip_suffix('\'')?;
        Some((i + 1, body.replace("''", "'")))
    })
}

#[test]
fn 自動の段の答えの写しがguiの正本と同じ答えになる() {
    let script = read(SCRIPT);
    let (line, literal) = ps_single_quoted(&script, "AutoSetupAnswers")
        .unwrap_or_else(|| panic!("{SCRIPT} に $AutoSetupAnswers = '…' の行が無い"));
    let parsed = SetupAnswers::from_json(&literal).unwrap_or_else(|e| {
        panic!("{SCRIPT}:{line}: $AutoSetupAnswers を setup が受け付けない: {e}")
    });
    let want = serde_json::to_value(tako_control::welcome::auto_setup_answers()).unwrap();
    let got = serde_json::to_value(parsed).unwrap();
    assert_eq!(
        got, want,
        "{SCRIPT}:{line}: $AutoSetupAnswers が GUI の初回起動の自動の段（welcome::auto_setup_answers）と\
         違う答えになっている。正本に合わせて写しを直すこと"
    );
}

#[test]
fn 自動の段の起こし方の写しがguiの正本と同じ引数になる() {
    assert!(
        read(DISPATCH).contains(GUI_SETUP_ARGS),
        "{DISPATCH} の run_setup_cli_within の引数が {GUI_SETUP_ARGS} でなくなった。\
         {SCRIPT} の自動の段の写し（{SCRIPT_SETUP_ARGS}）も同じ形へ直し、この番犬の字面も更新すること"
    );
    let script = read(SCRIPT);
    let hits = script
        .lines()
        .filter(|l| l.contains(SCRIPT_SETUP_ARGS) && l.contains("$AutoSetupAnswers"))
        .count();
    assert!(
        hits >= 1,
        "{SCRIPT} が自動の段を GUI と同じ引数（{SCRIPT_SETUP_ARGS} + 標準入力の $AutoSetupAnswers）で起こしていない"
    );
}

/// ci.yml の `windows:` ジョブの範囲（次のジョブの見出しまで）
fn windows_job(ci: &str) -> &str {
    let start = ci
        .find("\n  windows:\n")
        .expect("ci.yml に windows ジョブが無い")
        + 1;
    let rest = &ci[start..];
    let mut offset = 0;
    for (i, line) in rest.split_inclusive('\n').enumerate() {
        if i > 0 && is_job_key(line) {
            return &rest[..offset];
        }
        offset += line.len();
    }
    rest
}

/// ジョブの見出し（字下げ 2 の `name:`。コメントは除く）
fn is_job_key(line: &str) -> bool {
    let t = line.trim_end();
    t.starts_with("  ")
        && !t.starts_with("   ")
        && t.ends_with(':')
        && !t.trim_start().starts_with('#')
}

#[test]
fn 二つの検査がciのwindowsジョブに載っている() {
    let ci = read(CI);
    let job = windows_job(&ci);
    assert!(
        job.contains(SCRIPT),
        "{CI} の windows ジョブに {SCRIPT}（setup の通し検査）が無い（#2767）"
    );
    assert!(
        job.contains("Test-TakoInstalledPayload"),
        "{CI} の windows ジョブにインストーラーの一巡（Test-TakoInstalledPayload）が無い（#2767）"
    );
    assert!(
        !ci.replace(job, "").contains(SCRIPT),
        "{SCRIPT} が windows 以外のジョブにある（Windows 専用。レジストリを読む）"
    );
}

/// `function <name> {` から次の行頭の `}` まで
fn ps_function<'a>(text: &'a str, name: &str) -> Option<(usize, Vec<&'a str>)> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.trim_start().starts_with(&format!("function {name}")))?;
    let end = lines[start..].iter().position(|l| *l == "}")? + start;
    Some((start + 1, lines[start..=end].to_vec()))
}

#[test]
fn インストーラーの一巡はpathへの追加を選んで最後に戻す() {
    let text = read(VERIFY_ASSETS);
    let (line, body) = ps_function(&text, "Test-TakoInstalledPayload")
        .unwrap_or_else(|| panic!("{VERIFY_ASSETS} に Test-TakoInstalledPayload が無い"));
    let merge = body
        .iter()
        .find(|l| l.contains("/MERGETASKS="))
        .unwrap_or_else(|| {
            panic!("{VERIFY_ASSETS}:{line}: 無人インストールが /MERGETASKS でタスクを決めていない")
        });
    assert!(
        merge.contains("addtopath") && !merge.contains("!addtopath"),
        "{VERIFY_ASSETS}:{line}: 一巡の無人インストールが Path への追加（addtopath）を選んでいない: {}",
        merge.trim()
    );
    for (needle, why) in [
        (
            "Get-TakoUserPath",
            "ユーザー環境変数 Path を読み戻していない",
        ),
        (
            "Get-TakoPathEntryCount",
            "インストール先が Path に入った / 外れたを数えていない",
        ),
        ("UninstallString", "アンインストーラーを走らせていない"),
        (
            "Restore-TakoUserPath",
            "検査の後に Path を検査前へ戻していない",
        ),
    ] {
        assert!(
            body.iter().any(|l| l.contains(needle)),
            "{VERIFY_ASSETS}:{line}: Test-TakoInstalledPayload が {why}（{needle} が無い）"
        );
    }
}
