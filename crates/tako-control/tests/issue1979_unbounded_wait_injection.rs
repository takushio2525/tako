//! 本番ハングの型（子は終わったのに孫がパイプの書き手を握り続ける）を注入して、
//! 親子表の採取と器への問い合わせが**固まらない**ことを修正前・後の腕で並べる（#1979）
//!
//! 2026-10-10 01:20 の本番ハングは、メインスレッドが `ps -axo pid=,ppid=,command=` の
//! `Command::output()` の `poll` で 10 分止まったもの。`output()` は子が終わっても、
//! パイプの書き手（別スレッドが同時に起こした長生きの子へ漏れたもの）が残っていると返らない。
//!
//! ここでは偽の `ps`（本物の出力を出して終わるが、孫の `sleep` が stdout を握り続ける）と
//! 偽の `tmux`（`list-windows` の出力のあと同じく孫が握り続ける）を置き、
//! **本番ハングと同じ関数**（`agents::process_parent_map` = 経路の
//! `has_running_children` → `process_parent_map` の部分 / `tmux::list_windows_by_session`
//! = `tako list` の採り直し）を別プロセスで呼ぶ。A/B の env（`TAKO_1979_LEGACY=1`）は
//! プロセスで 1 回だけ読む（`OnceLock`）ので、腕ごとにこのテストバイナリ自身を起こし直す。
//!
//! - 修正後: 親子表は libproc で引く（偽の `ps` は起きない）・器の問い合わせは読み切りの
//!   猶予（2 秒）で返る
//! - 修正前（`TAKO_1979_LEGACY=1`）: どちらも孫が終わるまで返らない = 固まる
//!
//! UI スレッドで呼ばれる経路そのもの（IPC → `prepare_offload` → …）は隔離 GUI の
//! `scripts/test-ui-thread-wait-1979.sh` が見る。ここは型の再現を GUI 無しで CI に載せる。

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 子の役（このテストバイナリを起こし直したときだけ動く）を選ぶ env
const ROLE_ENV: &str = "TAKO_1979_INJECTION_ROLE";

/// 偽の孫が居座る秒数。**判定はこの長さとの桁の差**で行う（実時間の細かい予算に頼らない）
const HOLD_SECS: u64 = 40;

/// 修正後の腕を待つ上限。孫が握る [`HOLD_SECS`] より十分短いので、ここまでに返れば
/// 「孫の終わり（EOF）を待たずに返った」ことになる（実測は 0.03 秒 / 2.4 秒。負荷で
/// 数秒遅れても判定は反転しない）
const FIXED_DEADLINE: Duration = Duration::from_secs(30);

/// 修正前の腕を「固まった」と判定する待ち時間。修正前の形は孫が終わる（[`HOLD_SECS`]）
/// までは原理的に返れないので、それより短く切ってよい
const LEGACY_DEADLINE: Duration = Duration::from_secs(10);

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "tako-1979-inject-{}-{name}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("一時 dir");
    dir
}

fn write_exec(path: &Path, body: &str) {
    std::fs::write(path, body).expect("偽のコマンドを書く");
    let mut perm = std::fs::metadata(path).expect("meta").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
    std::fs::set_permissions(path, perm).expect("chmod");
}

/// 偽のコマンドが残した孫を落とす（**自分が記録した pid だけ**）
fn kill_holders(dir: &Path) {
    let Ok(text) = std::fs::read_to_string(dir.join("holders")) else {
        return;
    };
    for pid in text.lines().filter_map(|l| l.trim().parse::<i32>().ok()) {
        // SAFETY: 自分が起こした sleep の pid だけに SIGTERM を送る
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    }
}

fn calls(dir: &Path, name: &str) -> usize {
    std::fs::read_to_string(dir.join(name))
        .map(|t| t.lines().count())
        .unwrap_or(0)
}

/// このテストバイナリを子の役で起こし、`deadline` まで待つ。返ったら所要、固まったら `None`
fn run_role(role: &str, envs: &[(&str, String)], deadline: Duration) -> Option<Duration> {
    let exe = std::env::current_exe().expect("テストバイナリ");
    let mut cmd = Command::new(exe);
    cmd.args(["--exact", "子の役", "--ignored", "--nocapture"])
        .env(ROLE_ENV, role)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let started = Instant::now();
    let mut child = cmd.spawn().expect("子の役を起こす");
    loop {
        if let Ok(Some(_)) = child.try_wait() {
            return Some(started.elapsed());
        }
        if started.elapsed() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 子の役。親が env で選んだ関数を 1 回呼ぶだけ（直接 `cargo test` したときは何もしない）
#[test]
#[ignore = "issue1979 の注入テストが自分を起こし直したときだけ動く"]
fn 子の役() {
    match std::env::var(ROLE_ENV).as_deref() {
        Ok("ps") => {
            let parents = tako_control::agents::process_parent_map();
            assert!(!parents.is_empty(), "親子表が空");
        }
        Ok("tmux") => {
            let windows = tako_core::tmux::list_windows_by_session(Some("tako-1979-inject"));
            assert!(windows.is_some(), "偽の tmux の出力を読めていない");
        }
        _ => {}
    }
}

/// ① 本番ハングの関数: 親子表の採取（`ps` の `output()`）
#[test]
fn 孫がパイプを握るpsでも親子表の採取は固まらない() {
    let dir = scratch("ps");
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    write_exec(
        &bin.join("ps"),
        &format!(
            "#!/bin/sh\necho called >> '{d}/ps.calls'\n/bin/ps \"$@\"\nsleep {HOLD_SECS} &\necho $! >> '{d}/holders'\n",
            d = dir.display()
        ),
    );
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let fixed = run_role("ps", &[("PATH", path.clone())], FIXED_DEADLINE);
    let fixed_calls = calls(&dir, "ps.calls");
    let legacy = run_role(
        "ps",
        &[("PATH", path), ("TAKO_1979_LEGACY", "1".into())],
        LEGACY_DEADLINE,
    );
    let legacy_calls = calls(&dir, "ps.calls") - fixed_calls;
    kill_holders(&dir);
    let _ = std::fs::remove_dir_all(&dir);

    eprintln!(
        "親子表の採取: 修正後 {fixed:?}（偽の ps {fixed_calls} 回）/ 修正前 {legacy:?}（偽の ps {legacy_calls} 回。None = {}秒で返らず = 固まった）",
        LEGACY_DEADLINE.as_secs()
    );
    fixed.expect("修正後の腕が固まった（親子表の採取が孫の終わりを待っている）");
    if cfg!(target_os = "macos") {
        assert_eq!(
            fixed_calls, 0,
            "macOS の修正後は ps を起こさない（libproc）"
        );
    }
    // 検出力: 修正前の形は同じ注入で固まる（注入が効いていることの確認でもある）
    assert!(
        legacy.is_none(),
        "修正前の腕が返った = 注入が本番ハングの型を再現できていない（{legacy:?}）"
    );
    assert!(legacy_calls >= 1, "修正前の腕で偽の ps が起きていない");
}

/// ② `tako list` の採り直しの関数: 器への問い合わせ（`tmux list-windows -a`）
#[test]
fn 孫がパイプを握るtmuxでも器への問い合わせは固まらない() {
    let dir = scratch("tmux");
    let fake = dir.join("tmux");
    // `-V` は本物の tmux を名乗る（バイナリ解決の検査を通す）。`list-windows` は 1 行出して
    // 終わるが、孫が stdout を握り続ける
    write_exec(
        &fake,
        &format!(
            "#!/bin/sh\ncase \" $* \" in\n  *\" -V \"*) echo 'tmux 3.5a'; exit 0 ;;\n  *\" list-windows \"*)\n    echo called >> '{d}/tmux.calls'\n    printf 'tako-1979-s\\t0\\tzsh\\t1\\t1\\n'\n    sleep {HOLD_SECS} &\n    echo $! >> '{d}/holders'\n    exit 0 ;;\nesac\nexit 1\n",
            d = dir.display()
        ),
    );
    let envs = |legacy: bool| {
        let mut v = vec![("TAKO_TMUX_BIN", fake.display().to_string())];
        if legacy {
            v.push(("TAKO_1979_LEGACY", "1".to_string()));
        }
        v
    };
    let fixed = run_role("tmux", &envs(false), FIXED_DEADLINE);
    let legacy = run_role("tmux", &envs(true), LEGACY_DEADLINE);
    let total_calls = calls(&dir, "tmux.calls");
    kill_holders(&dir);
    let _ = std::fs::remove_dir_all(&dir);

    eprintln!(
        "器への問い合わせ: 修正後 {fixed:?} / 修正前 {legacy:?}（None = {}秒で返らず）/ 偽の tmux の list-windows {total_calls} 回",
        LEGACY_DEADLINE.as_secs()
    );
    fixed.expect("修正後の腕が固まった（器への問い合わせが孫の終わりを待っている）");
    assert!(
        legacy.is_none(),
        "修正前の腕が返った = 注入が型を再現できていない（{legacy:?}）"
    );
    assert_eq!(total_calls, 2, "両方の腕で偽の tmux が 1 回ずつ起きる");
}
