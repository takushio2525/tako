//! Issue #652: アカウント（`CLAUDE_CONFIG_DIR`）の会話を resume する経路の実機 E2E。
//! 実 `claude` CLI + Anthropic API を使うため `#[ignore]`。CI では走らない。手動実行:
//!
//! ```pwsh
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

/// テスト用の一時ディレクトリ接頭辞。削除ガードもこの値で判定する。
///
/// unix では `$TMPDIR`（macOS の `/var/folders/...`）ではなく `/private/tmp` 直下に
/// 作る（claude_tui_e2e と同じ理由: 祖先の信頼済みエントリの影響を避ける）。
/// Windows には該当の作法が無いので OS の一時ディレクトリを使う
fn temp_prefix() -> PathBuf {
    let name = format!("tako-e2e-652-{}", std::process::id());
    #[cfg(windows)]
    {
        std::env::temp_dir().join(name)
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/private/tmp").join(name)
    }
}

/// **一時ディレクトリ配下であることを検証してから**消す
/// （変数名の取り違えで実アカウントの config dir を消す事故を構造的に防ぐ）
fn remove_temp_dir(dir: &Path) {
    let guard = temp_prefix();
    let guard = guard.to_string_lossy();
    assert!(
        dir.to_string_lossy().starts_with(guard.as_ref()),
        "テスト用一時ディレクトリ以外を削除しようとしている: {}",
        dir.display()
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// ペインのシェルと同じ方言でコマンドを実行し、stdout + stderr を返す。
///
/// tako は resume コマンドを**ペインのシェルへ文字列として流し込む**ので、
/// 検証も同じ経路（windows: PowerShell / unix: ログインシェル）で行う
fn run_in_shell(cwd: &Path, command: &str) -> String {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("pwsh");
        c.args(["-NoProfile", "-Command", command]);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let mut c = Command::new(shell);
        c.args(["-l", "-c", command]);
        c
    };
    let output = cmd
        .current_dir(cwd)
        .output()
        .expect("シェルの起動に失敗（claude CLI / pwsh は入っているか）");
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// 既定 config ディレクトリに使い捨て会話を 1 本作り、その session_id を返す
fn create_throwaway_session(cwd: &Path) -> String {
    // 既定 config dir で作る（= 認証済み）。前置は本番と同じ組み立て部品を使わず、
    // ここでは「既定へ倒す」ことだけできればよい
    let unset = transcript::resume_env_prefix_for(&transcript::TranscriptLocation {
        path: PathBuf::new(),
        config_dir: PathBuf::new(),
        is_default: true,
    });
    let out = run_in_shell(
        cwd,
        &format!(
            "{unset}claude -p --model haiku --output-format json 'reply with exactly: e2e-652'"
        ),
    );
    let value: serde_json::Value = serde_json::from_str(out.trim())
        .unwrap_or_else(|e| panic!("claude の JSON 出力をパースできない: {e}\n出力: {out}"));
    value["session_id"]
        .as_str()
        .unwrap_or_else(|| panic!("session_id が無い: {out}"))
        .to_string()
}

/// #652 の中核: resume は「会話が保存されている config ディレクトリ」でしか成立しない。
/// tako の走査（`locate_transcript`）とコマンド組み立て（`resume_env_prefix_for`）が
/// 実 claude の挙動と一致していることを通しで確かめる
#[test]
#[ignore = "実 claude CLI + API を使う"]
fn resumeは会話のあるconfigディレクトリでのみ成立する() {
    let base = temp_prefix();
    std::fs::create_dir_all(&base).expect("作業ディレクトリを作れない");

    let session_id = create_throwaway_session(&base);
    assert!(
        transcript::is_valid_session_id(&session_id),
        "session_id の形式が不正: {session_id}"
    );

    // 作った直後は既定 config ディレクトリに居る
    let before = transcript::locate_transcript(&session_id)
        .expect("作ったばかりの会話が見つからない（走査が壊れている）");
    assert!(
        before.is_default,
        "既定 config dir で作った会話が既定と判定されない: {before:?}"
    );

    // アカウント運用を再現: transcript を一時 config ディレクトリへ move する
    let account_dir = base.join(".claude-account");
    let slug = before
        .path
        .parent()
        .and_then(|p| p.file_name())
        .expect("スラグディレクトリが取れない")
        .to_owned();
    let moved_dir = account_dir.join("projects").join(&slug);
    std::fs::create_dir_all(&moved_dir).expect("移動先を作れない");
    let moved = moved_dir.join(format!("{session_id}.jsonl"));
    std::fs::rename(&before.path, &moved).expect("transcript を移動できない");

    // 1) 走査: 既定でない config ディレクトリでも所在つきで見つかる
    let default_dir = std::env::temp_dir().join("tako-652-not-a-real-default");
    let found = transcript::locate_transcript_in(
        &[default_dir.clone(), account_dir.clone()],
        Some(&default_dir),
        &session_id,
    )
    .expect("移動先の会話を見つけられない（#652 の根因 1 が再発）");
    assert_eq!(found.config_dir, account_dir);
    assert!(!found.is_default);

    // 2) 実 claude: 所在を指定した resume は「会話が無い」にならない
    let with_prefix = format!(
        "{}claude --resume {session_id} -p ok",
        transcript::resume_env_prefix_for(&found)
    );
    let out_with = run_in_shell(&base, &with_prefix);
    assert!(
        !out_with.contains(NOT_FOUND),
        "所在を指定したのに会話が見つからない。\nコマンド: {with_prefix}\n出力: {out_with}"
    );

    // 3) 修正前の挙動（前置なし = 既定 config dir で resume）は必ず失敗する。
    //    これが出なくなったら、このテストは #652 を検出できていない
    let out_without = run_in_shell(&base, &format!("claude --resume {session_id} -p ok"));
    assert!(
        out_without.contains(NOT_FOUND),
        "前置なしでも成功してしまい、検出力が無い。\n出力: {out_without}"
    );

    remove_temp_dir(&base);
}
