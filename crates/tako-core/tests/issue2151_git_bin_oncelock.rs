//! #2151 の実物の `OnceLock`（`git::git_bin()` の static）を、戻らないログインシェルの下で 2 本の
//! スレッドから引く（同一バイナリの A/B）。
//!
//! `git_bin()` はプロセスで 1 回だけ解決し、env（`$SHELL` / `PATH` / 上限 / A/B）もその 1 回でしか
//! 読まないので、**このテストバイナリを子プロセスとして起こし直して**その中で引く（`--exact` で
//! [`子プロセスの本体`] だけを走らせる。普段の `cargo test` ではこの関数は何もしない）。
//!
//! 子の中では、スレッド A が最初の git 操作（`git::repo_root`）で解決を始め、その最中に
//! スレッド B が同じ操作を引く（`OnceLock` の初期化の完了を待つ側）。戻らないログインシェルは
//! `$SHELL` を差し替えるスタブ、PATH は空のディレクトリ、既知の場所の段は注入で飛ばす
//! （macOS には `/usr/bin/git` が必ずあるので、飛ばさないとログインシェルの段へ届かない）。
//!
//! - 修正後: 2 本とも上限（2 秒）で戻り、ログインシェルは 1 回しか起こされない（= B は A の解決を待った）
//! - 修正前（`TAKO_2151_LEGACY=1`）: 外側の締め切りまで 1 本も戻らない（同じ条件で症状が出る = 検査が
//!   見逃す側へ倒れていない）

#![cfg(unix)]

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tako_core::test_residue::ScratchDir;

/// 子の中で使う上限（秒）。既定の 5 秒を検証のために縮める
const BUDGET_SECS: u64 = 2;
/// 子が戻るのを待つ外側の締め切り。修正後は上限の 10 倍、修正前は戻らないことを見る長さ
const NEW_DEADLINE: Duration = Duration::from_secs(20);
const LEGACY_DEADLINE: Duration = Duration::from_secs(6);
/// B が引き始めるまでの間（A の解決が始まってから引く）
const B_DELAY: Duration = Duration::from_millis(200);

/// 子プロセスの中身。`TAKO_2151_CHILD_DIR` が無ければ何もしない（普段の `cargo test`）
#[test]
fn 子プロセスの本体() {
    let Some(dir) = std::env::var_os("TAKO_2151_CHILD_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let start = Instant::now();
    let spawn = |who: &'static str| {
        let dir = dir.clone();
        std::thread::spawn(move || {
            let began = start.elapsed();
            let root = tako_core::git::repo_root(&dir);
            println!(
                "2151: {who} began_ms={} done_ms={} root={}",
                began.as_millis(),
                start.elapsed().as_millis(),
                root.is_some()
            );
        })
    };
    let a = spawn("A");
    std::thread::sleep(B_DELAY);
    let b = spawn("B");
    a.join().expect("A");
    b.join().expect("B");
}

/// 戻らないログインシェル（自分の pid を `calls` へ追記してから眠る。`exec` = 打ち切りの kill が本人に当たる）
fn hang_shell(dir: &Path) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("hang-shell");
    let calls = dir.join("calls");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\necho $$ >> '{}'\nexec /bin/sleep 600\n",
            calls.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.display().to_string()
}

/// ログインシェルが起こされた回数と pid
fn shell_calls(dir: &Path) -> Vec<i32> {
    std::fs::read_to_string(dir.join("calls"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

/// 子を起こす（このテストバイナリの [`子プロセスの本体`] だけ）
fn spawn_child(dir: &Path, legacy: bool) -> Child {
    let empty = dir.join("empty-path");
    std::fs::create_dir_all(&empty).unwrap();
    let mut cmd = Command::new(std::env::current_exe().expect("テストバイナリ"));
    cmd.args([
        "--exact",
        "子プロセスの本体",
        "--nocapture",
        "--test-threads=1",
    ])
    .env("TAKO_2151_CHILD_DIR", dir)
    .env("SHELL", hang_shell(dir))
    .env("PATH", &empty)
    .env("TAKO_2151_INJECT", "skip-known-paths:git")
    .env("TAKO_BIN_RESOLVE_TIMEOUT_SECS", BUDGET_SECS.to_string())
    .env_remove("TAKO_GIT_BIN")
    .env_remove("TAKO_1503_LEGACY")
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::null());
    if legacy {
        cmd.env("TAKO_2151_LEGACY", "1");
    } else {
        cmd.env_remove("TAKO_2151_LEGACY");
    }
    cmd.spawn().expect("子を起こせる")
}

/// 締め切りまで子の終わりを待つ。戻ったら `Some(所要)`、締め切りなら kill して `None`
fn wait_until(child: &mut Child, deadline: Duration) -> Option<Duration> {
    let start = Instant::now();
    loop {
        if child.try_wait().expect("try_wait").is_some() {
            return Some(start.elapsed());
        }
        if start.elapsed() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 眠っているスタブを片付ける（修正前の腕で眠ったままの、自分が起こしたものだけ）
fn kill_stubs(pids: &[i32]) {
    for pid in pids {
        // SAFETY: スタブの pid へ SIGKILL を送るだけ（もう居なければ ESRCH で何も起きない）
        unsafe {
            libc::kill(*pid, libc::SIGKILL);
        }
    }
}

#[test]
fn 戻らないログインシェルでも最初のgit操作は上限で戻り待っていた別スレッドも一緒に戻る() {
    // 修正後
    let new_dir = ScratchDir::new("2151-oncelock-new");
    let mut child = spawn_child(new_dir.path(), false);
    let waited = wait_until(&mut child, NEW_DEADLINE);
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut out);
    }
    let calls = shell_calls(new_dir.path());
    // libtest の `test 子プロセスの本体 ... ` と同じ行に最初の 1 行がつながるので、行の途中からも拾う
    let lines: Vec<&str> = out
        .lines()
        .filter_map(|l| l.find("2151: ").map(|at| &l[at..]))
        .collect();
    eprintln!(
        "修正後: 子の所要 {waited:?}・ログインシェル {} 回\n  {}",
        calls.len(),
        lines.join("\n  ")
    );
    assert!(
        waited.is_some(),
        "修正後なのに {NEW_DEADLINE:?} 以内に戻らない（最初の git 操作がログインシェルを上限なしに待っている）"
    );
    for who in ["A", "B"] {
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with(&format!("2151: {who} "))),
            "スレッド {who} が戻っていない: {out}"
        );
    }
    assert_eq!(
        calls.len(),
        1,
        "ログインシェルは 1 回だけ（B は A の解決の完了を待つ側 = OnceLock）"
    );
    // 打ち切ったスタブは子の中で kill・刈り取り済み（ここでは送らない = 他人の pid を撃たない）
    let until = Instant::now() + Duration::from_secs(5);
    // 生死は境界の 1 実装で見る（#1557）
    while tako_core::platform::process::pid_alive(calls[0] as u32) {
        assert!(
            Instant::now() < until,
            "打ち切ったログインシェル {} が残っている",
            calls[0]
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    // 修正前（同じ条件で症状が出る）
    let legacy_dir = ScratchDir::new("2151-oncelock-legacy");
    let mut child = spawn_child(legacy_dir.path(), true);
    let waited = wait_until(&mut child, LEGACY_DEADLINE);
    let calls = shell_calls(legacy_dir.path());
    kill_stubs(&calls);
    eprintln!(
        "修正前: 子の所要 {waited:?}・ログインシェル {} 回",
        calls.len()
    );
    assert!(
        waited.is_none(),
        "修正前の腕（TAKO_2151_LEGACY=1）が {LEGACY_DEADLINE:?} 以内に戻った = 注入が症状を再現していない"
    );
    assert_eq!(calls.len(), 1, "修正前もログインシェルの段まで届いている");
}
