//! tako が `claude auth login` を自分で起こしていないかの番犬（#1129）
//!
//! ブラウザ操作待ちのプロセスは**自分では終わらない**。tako が起こすと寿命の
//! 持ち主が居なくなり、Windows は子プロセスの終了要求（#1067 の境界 B5）が
//! 未実装なので、ペイン close も隔離インスタンスの終了も孫を回収しない。
//! 実機ではセルフテストが打ち込む `tako setup` 経由で 1 日 46 本まで積み上がり、
//! `Win32_Processor.LoadPercentage` が 100% に張り付いた（#1129 の採取）。
//!
//! 「人が見ているか」を stdin が端末かどうかでは判別できない（セルフテストが
//! ペインへ打ち込む `tako setup` も PTY 上では端末に見える）ので、条件で絞らず
//! **構造的に起こさない**。案内だけ出すのは AGENTS.md / docs / MCP の説明
//! （#1057「認証は代行させない」）と同じ契約で、コードだけがそこから外れていた。
//!
//! 見た目には壊れているように見えない（`tako setup` は待っているだけ）ので、
//! 機械検査で止める。A/B 用の legacy 経路だけは `watchdog-allow` で逃がす。

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    // このファイルは <root>/crates/tako-control/tests/ にある
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // ビルド生成物と使い捨て検証コードは対象外
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "target" || name == "poc" || name.starts_with('.') {
                continue;
            }
            rust_sources(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// 行がコメントだけか（`//` で始まる。doc コメントも含む）
fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// `Command` へ `auth` + `login` を渡している行を探す。
///
/// 拾うのは**引数として渡している形**だけ（`args(["auth", "login"])` /
/// `.arg("auth").arg("login")`）。案内文の中の `` `claude auth login` `` は
/// 1 つの文字列なので当たらない = 文面は自由に書ける
fn auth_login_argv_sites(src: &str) -> Vec<usize> {
    let mut hits = Vec::new();
    let mut allow_next = false;
    for (i, line) in src.lines().enumerate() {
        if line.contains("watchdog-allow") {
            allow_next = true;
            continue;
        }
        let is_argv = (line.contains(".args(") || line.contains(".arg("))
            && line.contains("\"auth\"")
            && line.contains("\"login\"");
        if is_argv && !is_comment(line) && !allow_next {
            hits.push(i + 1);
        }
        if !line.trim().is_empty() {
            allow_next = false;
        }
    }
    hits
}

#[test]
fn 認証コマンドの起動が製品コードに残っていない() {
    let root = workspace_root();
    let mut files = Vec::new();
    rust_sources(&root.join("crates"), &mut files);
    assert!(!files.is_empty(), "走査対象の .rs が 1 つも無い");

    let mut offenders: Vec<String> = Vec::new();
    for file in &files {
        let Ok(src) = std::fs::read_to_string(file) else {
            continue;
        };
        for line in auth_login_argv_sites(&src) {
            offenders.push(format!("{}:{line}", file.display()));
        }
    }
    assert!(
        offenders.is_empty(),
        "tako が `claude auth login` を自分で起こしている（#1129）。\n\
         ブラウザ操作待ちで終わらないプロセスなので、tako は案内だけを出すこと\n\
         （文面の正本は `setup_bootstrap::auth_instructions`）。\n\
         A/B 用に意図して残す 1 か所だけ `watchdog-allow` を付ける:\n{}",
        offenders.join("\n")
    );
}

/// 逃げ道が効くこと（= 番犬自身の検出力の確認）
#[test]
fn 検出力と逃げ道が効く() {
    let bad = "    .args([\"auth\", \"login\"])\n";
    assert_eq!(auth_login_argv_sites(bad), vec![1], "起動を見逃している");

    let allowed = "    // watchdog-allow: A/B 用\n    .args([\"auth\", \"login\"])\n";
    assert!(
        auth_login_argv_sites(allowed).is_empty(),
        "watchdog-allow が効いていない"
    );

    // 案内文（1 つの文字列）は当たらない
    let guidance = "    \"`claude auth login` を実行してください\".to_string()\n";
    assert!(auth_login_argv_sites(guidance).is_empty());

    // 読み取りだけの `auth status` は対象外
    let status = "    .args([\"auth\", \"status\", \"--json\"])\n";
    assert!(auth_login_argv_sites(status).is_empty());
}

/// 案内の文面は 1 か所（`setup_bootstrap::auth_instructions`）から出す。
/// 実行するコマンドは #983 の `agent_cli::auth_command` が正本なので、
/// そこを変えれば案内も追従する
#[test]
fn 案内はコマンドの正本を引く() {
    let lines = tako_control::setup_bootstrap::auth_instructions();
    let joined = lines.join("\n");
    let cmd = tako_control::orchestrator::agent_cli::auth_command(
        tako_core::agent_support::Agent::Claude,
    )
    .expect("claude にはログインコマンドがある");
    assert!(joined.contains(cmd), "次の 1 手が入っていない: {joined}");
    assert!(
        joined.contains("tako setup"),
        "やり直し方が入っていない: {joined}"
    );
    assert!(
        joined.contains("代行しません"),
        "代行しないことが書かれていない: {joined}"
    );
}
