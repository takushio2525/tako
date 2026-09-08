//! サーバー（ソケット）単位の回収の実物テスト（#1192）
//!
//! **本番のソケット置き場は絶対に対象にしない**。tmux は `-S <パス>` で任意の場所に
//! サーバーを立てられるので、テストは一時ディレクトリに自分でダミーを起こし、そこだけを
//! 走査する（`scan_servers_in` / `cleanup_servers_from` にディレクトリを渡す形）。
//!
//! 一番大事な検査は **「所有者が生きているサーバーは `--apply` でも消えない」**。
//! 過去に接頭辞一致の一括 kill で別 worker の生きているバックエンドを落とした事故
//! （#625）があり、その再発をここで止める。

use std::path::{Path, PathBuf};
use std::process::Command;

use tako_core::tmux_cleanup::{
    cleanup_servers_from, scan_servers_in, ServerVerdict, SERVER_FRESH_SECS,
};

fn tmux_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 一時ディレクトリに `-S <dir>/<name>` でサーバーを立てる
fn spawn_server(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    let out = Command::new("tmux")
        .arg("-S")
        .arg(&path)
        .args([
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "dummy",
            "sleep",
            "300",
        ])
        .output()
        .expect("tmux を起動できる");
    assert!(
        out.status.success(),
        "ダミーサーバーを作れる: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    path
}

fn kill_server(path: &Path) {
    let _ = Command::new("tmux")
        .arg("-S")
        .arg(path)
        .arg("kill-server")
        .output();
    let _ = std::fs::remove_file(path);
}

/// 確実に生きていない pid（自分の pid から遠い上限側を探す）
fn dead_pid() -> u32 {
    for candidate in (900_000..999_999).rev() {
        if !tako_core::ports::process_alive(candidate) {
            return candidate;
        }
    }
    panic!("死んだ pid を用意できない");
}

/// #1192 の必須検査。生きた所有者のサーバーは `--apply` でも消えない
#[test]
fn 所有者が生きているサーバーはapplyでも消えない() {
    if !tmux_available() {
        eprintln!("skip: tmux が無い環境");
        return;
    }
    let dir = std::env::temp_dir().join(format!("tako-1192-alive-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("一時ディレクトリを作れる");
    // 所有 pid = このテストプロセス（確実に生きている）
    let alive = spawn_server(&dir, &format!("tako-1192test-{}", std::process::id()));
    let dead = spawn_server(&dir, &format!("tako-1192test-{}", dead_pid()));

    let entries = scan_servers_in(&dir, tako_core::ports::process_alive);
    assert_eq!(entries.len(), 2, "ダミー 2 本だけを見ている: {entries:?}");
    assert!(entries.iter().all(|e| e.running), "2 本とも生きている");

    // 出来たてでも判定できるよう猶予 0 で回す（本番は SERVER_FRESH_SECS）
    let outcome = cleanup_servers_from(entries, "tako", &[], true, 0);
    let verdict_of = |name: &str| {
        outcome
            .entries
            .iter()
            .find(|(e, _)| e.socket == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| panic!("{name} が結果に無い"))
    };
    let alive_name = alive.file_name().unwrap().to_str().unwrap().to_string();
    let dead_name = dead.file_name().unwrap().to_str().unwrap().to_string();

    assert!(
        matches!(verdict_of(&alive_name), ServerVerdict::OwnerAlive { .. }),
        "所有者が生きているサーバーを回収対象にしている: {:?}",
        verdict_of(&alive_name)
    );
    assert!(
        !outcome.killed.contains(&alive_name),
        "所有者が生きているサーバーを kill した（#625 の事故クラス）"
    );
    assert!(
        alive.exists(),
        "所有者が生きているサーバーのソケットを消した"
    );
    assert!(
        Command::new("tmux")
            .arg("-S")
            .arg(&alive)
            .args(["list-sessions", "-F", "#{session_name}"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false),
        "所有者が生きているサーバーが落ちている"
    );

    // 所有者が死んでいる方は kill + ソケット削除まで済む
    assert!(
        outcome.killed.contains(&dead_name),
        "所有者が死んだサーバーを回収していない: {:?}",
        verdict_of(&dead_name)
    );
    assert!(!dead.exists(), "kill 後にソケットファイルが残っている");

    kill_server(&alive);
    kill_server(&dead);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 残骸ソケット（サーバー不在）はファイルだけ消える。出来たては触らない
#[test]
fn 残骸ソケットは消えるが出来たては触らない() {
    let dir = std::env::temp_dir().join(format!("tako-1192-stale-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("一時ディレクトリを作れる");
    let stale = dir.join("tako-selftest-999999");
    std::fs::write(&stale, b"").expect("残骸ソケットを置ける");

    // 出来たて（猶予 60 秒）のあいだは触らない
    let entries = scan_servers_in(&dir, tako_core::ports::process_alive);
    let outcome = cleanup_servers_from(entries, "tako", &[], true, SERVER_FRESH_SECS);
    assert_eq!(outcome.count("too_fresh"), 1, "{:?}", outcome.entries);
    assert!(stale.exists(), "出来たての残骸を消してしまった");

    // 猶予を過ぎたら消す
    let entries = scan_servers_in(&dir, tako_core::ports::process_alive);
    let outcome = cleanup_servers_from(entries, "tako", &[], true, 0);
    assert_eq!(outcome.removed_sockets, vec!["tako-selftest-999999"]);
    assert!(!stale.exists(), "残骸ソケットが残っている");
    let _ = std::fs::remove_dir_all(&dir);
}

/// dry-run（既定）は 1 つも触らない
#[test]
fn dry_runは何も触らない() {
    let dir = std::env::temp_dir().join(format!("tako-1192-dry-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("一時ディレクトリを作れる");
    let stale = dir.join("tako-selftest-999998");
    std::fs::write(&stale, b"").expect("残骸ソケットを置ける");

    let entries = scan_servers_in(&dir, tako_core::ports::process_alive);
    let outcome = cleanup_servers_from(entries, "tako", &[], false, 0);
    assert!(!outcome.applied);
    assert_eq!(outcome.count("stale_socket"), 1, "判定は出る");
    assert!(outcome.removed_sockets.is_empty() && outcome.killed.is_empty());
    assert!(stale.exists(), "dry-run なのに消した");
    let _ = std::fs::remove_dir_all(&dir);
}
