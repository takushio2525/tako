//! Issue #652: アカウント（`CLAUDE_CONFIG_DIR`）の会話を resume する経路の実機 E2E。
//! 実 `claude` CLI + Anthropic API を使うため `#[ignore]`。CI では走らない。手動実行:
//!
//! ```sh
//! cargo test -p tako-control --test issue652_resume_e2e -- --ignored --test-threads=1
//! ```
//!
//! 前提: `claude` CLI が既定 config ディレクトリ（`~/.claude`）でログイン済み /
//! ネットワーク接続。
//!
//! 検証する事実（この 2 つが #652 の根因）:
//! 1. `claude --resume <id>` は**会話が保存されている config ディレクトリ**で
//!    実行しないと `No conversation found with session ID` で失敗する
//! 2. transcript は `<config dir>/projects/` 配下にあるので、既定 `~/.claude` だけを
//!    見る走査ではアカウントのペインが「会話なし」と判定され resume されない
//!
//! 認証を必要とせずに 1 を確かめるため、既定 config ディレクトリで作った使い捨て会話の
//! transcript を一時 config ディレクトリへ **move** して「アカウント運用」を再現する。
//! move 先は未認証なので resume は最終的に `Not logged in` で終わるが、
//! **`No conversation found` にならない**ことが「会話を見つけられた」証拠になる。

use std::path::{Path, PathBuf};
use std::process::Command;

use tako_control::transcript;

/// 会話が見つからなかったときに claude が出す文言（判定の要）
const NOT_FOUND: &str = "No conversation found with session ID";

/// テスト用の作業ディレクトリ。`claude` の project スラグはこのパスから決まる。
/// `$TMPDIR`（`/var/folders/...`）ではなく `/private/tmp` 直下に作る
/// （claude_tui_e2e と同じ理由: 祖先の信頼済みエントリの影響を避ける）
fn base_dir() -> PathBuf {
    PathBuf::from(format!("/private/tmp/tako-e2e-652-{}", std::process::id()))
}

/// パスから claude の project スラグ（`/` を `-` に置換）を作る
fn project_slug(dir: &Path) -> String {
    dir.display().to_string().replace('/', "-")
}

/// **一時ディレクトリ配下であることを検証してから**消す
/// （変数名の取り違えで実アカウントの config dir を消す事故を構造的に防ぐ）
fn remove_temp_dir(dir: &Path) {
    // Path::starts_with はコンポーネント単位の比較なので、接頭辞判定は文字列で行う
    assert!(
        dir.to_string_lossy()
            .starts_with("/private/tmp/tako-e2e-652-"),
        "テスト用一時ディレクトリ以外を削除しようとしている: {}",
        dir.display()
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// ログインシェル経由でコマンドを実行し、stdout + stderr を返す
fn run_in_shell(cwd: &Path, command: &str) -> String {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let output = Command::new(shell)
        .args(["-l", "-c", command])
        .current_dir(cwd)
        .output()
        .expect("シェルの起動に失敗");
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// 既定 config ディレクトリに使い捨て会話を 1 本作り、その session_id を返す
fn create_throwaway_session(cwd: &Path) -> String {
    let out = run_in_shell(
        cwd,
        "unset CLAUDE_CONFIG_DIR; claude -p --model haiku --output-format json \
         'reply with exactly: e2e-652' < /dev/null",
    );
    let value: serde_json::Value = serde_json::from_str(out.trim())
        .unwrap_or_else(|e| panic!("claude の JSON 出力をパースできない: {e}\n出力: {out}"));
    value["session_id"]
        .as_str()
        .unwrap_or_else(|| panic!("session_id が無い: {out}"))
        .to_string()
}

/// #652 の中核: resume は「会話が保存されている config ディレクトリ」でしか成立しない。
/// tako の走査（`locate_transcript_in`）とコマンド組み立て（`resume_env_prefix_for`）が
/// 実 claude の挙動と一致していることを通しで確かめる
#[test]
#[ignore = "実 claude CLI + API を使う"]
fn resumeは会話のあるconfigディレクトリでのみ成立する() {
    let base = base_dir();
    remove_temp_dir(&base);
    std::fs::create_dir_all(&base).expect("作業ディレクトリの作成");

    let session_id = create_throwaway_session(&base);
    assert!(
        transcript::is_valid_session_id(&session_id),
        "session_id の形式: {session_id}"
    );

    // 既定 config ディレクトリに transcript ができている
    let default_dir = PathBuf::from(std::env::var("HOME").expect("HOME")).join(".claude");
    let located = transcript::locate_transcript_in(
        std::slice::from_ref(&default_dir),
        Some(&default_dir),
        &session_id,
    )
    .expect("既定 config dir に transcript がある");
    assert!(located.is_default);
    assert_eq!(
        transcript::resume_env_prefix_for(&located),
        "unset CLAUDE_CONFIG_DIR; "
    );

    // アカウント運用の再現: transcript を別 config ディレクトリへ移す
    let alt_dir = base.join("altcfg");
    let slug = project_slug(&base);
    let alt_project = alt_dir.join("projects").join(&slug);
    std::fs::create_dir_all(&alt_project).expect("alt config dir の作成");
    let alt_path = alt_project.join(format!("{session_id}.jsonl"));
    std::fs::rename(&located.path, &alt_path).expect("transcript の移動");
    // 空になった既定側のプロジェクトディレクトリは片付ける（空のときだけ消える）
    if let Some(parent) = located.path.parent() {
        let _ = std::fs::remove_dir(parent);
    }

    // 既定だけを走査する旧実装では見つからない = resume を諦めていた
    assert!(
        transcript::locate_transcript_in(
            std::slice::from_ref(&default_dir),
            Some(&default_dir),
            &session_id
        )
        .is_none(),
        "既定 config dir だけの走査では見つからない"
    );
    // 全 config ディレクトリを走査すれば所在が分かる
    let located = transcript::locate_transcript_in(
        &[default_dir.clone(), alt_dir.clone()],
        Some(&default_dir),
        &session_id,
    )
    .expect("alt config dir に transcript がある");
    assert_eq!(located.path, alt_path);
    assert!(!located.is_default);
    let prefix = transcript::resume_env_prefix_for(&located);
    assert!(
        prefix.starts_with("export CLAUDE_CONFIG_DIR="),
        "アカウントの config dir を export する: {prefix}"
    );

    // (before) 既定 config ディレクトリでの resume は会話を見つけられない
    let before = run_in_shell(
        &base,
        &format!("unset CLAUDE_CONFIG_DIR; claude --resume {session_id} -p 'ok' < /dev/null"),
    );
    assert!(
        before.contains(NOT_FOUND),
        "既定 config dir では会話が見つからない想定: {before}"
    );

    // (after) tako が投入するコマンドと同じ前置なら会話を見つけられる。
    // 移動先の config dir は未認証なので `Not logged in` で終わるが、
    // それは「会話の解決を通り抜けた」証拠
    let after = run_in_shell(
        &base,
        &format!("{prefix}claude --resume {session_id} -p 'ok' < /dev/null"),
    );
    assert!(
        !after.contains(NOT_FOUND),
        "config dir を指定すれば会話を見つけられる想定: {after}"
    );

    remove_temp_dir(&base);
}
