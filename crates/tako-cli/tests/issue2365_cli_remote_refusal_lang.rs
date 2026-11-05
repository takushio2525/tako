//! 実経路: `tako remote start` / `stop` の断り（daemon の起動と停止が返す理由）が表示言語に従う
//! （Issue #2365 の続き）。
//!
//! 修正前（実測）: 英語表示でも「リモートサーバーは既に起動中」「リモートサーバーが起動していない（PID
//! ファイルが無い）」「PID … が tako remote serve であることを確認できません…」「PID ファイルが無く、記録された
//! 待ち受け（…）は応答していますが、停止要求が通りませんでした: 管理トークンも無い…」ほかが日本語の固定文の
//! まま出た（`remote::spawn_daemon` / `daemon_stop` が `Err(String)` の日本語を返していた）。
//!
//! **ビルドした `tako` をそのまま**叩く（どれも GUI の要らない手元の処理）。見ること:
//! - 英語表示: stderr に日本語（かな・漢字・全角の記号）が 1 字も無い
//! - 日本語表示: #2365 より前の出力とバイト一致（数字を `0` に均し、砂場の dir を伏せる）
//! - stdout は日英で同じ（断りは stdout に何も書かない）・終了コードは日英で同じ
//!
//! 状態は state の置き場に記録（PID ファイル・待ち受けの記録・管理トークン）を置いて作る。生きている pid は
//! このテストが起こした `sleep`（tako remote serve ではない = 正体を確かめられない相手）、待ち受けはこのテストが
//! 開いた TCP（決まった HTTP の答えを返す偽の daemon）。本物の daemon を起こすケース（既に起動中・止めた後の
//! stop）は tmux が無い環境では daemon が起動を拒むので走らせない。**走ったかどうかを必ず stderr に 1 行出し**、
//! `I2365D_REQUIRE_DAEMON=1` なら走らなかったことを失敗にする（手元の受け入れで skip でないことを示す）。
//!
//! A/B: `TAKO_2365_REFUSAL_BIN=<修正前の tako>` で別のバイナリを叩ける（修正前は英語表示の検査で落ちる）。
//! `I2365D_DUMP=<dir>` を付けると全ケースの出力を書き出す（修正前後の `diff -r` 用）。
//! 本番の data dir・GUI・tailscale へ届かないよう、`TAKO_*` を全部外して一時の置き場だけを渡す。
//!
//! unix だけで走らせる（pid の生死・偽の tailscale が sh のスクリプト）。文の日英の対は OS に依らない 1 か所
//! （`RemoteRefusal` / `ShutdownReject` の `text_in`）で、両 OS の番犬
//! （`crates/tako-control/tests/issue1762_dispatch_error_lang.rs`）が表で見る
#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tako_core::test_residue::ScratchDir;

/// 叩くバイナリ（既定はこの木でビルドした `tako`。A/B は `TAKO_2365_REFUSAL_BIN`）
fn tako() -> PathBuf {
    std::env::var_os("TAKO_2365_REFUSAL_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_tako")))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 偽の tailscale（`status` はノード名だけ・serve の設定は空。`TAKO_REMOTE_TEST_MODE=1` は serve を張らない）
const FAKE_TAILSCALE: &str = "#!/bin/sh\n\
    case \"$1 $2\" in\n\
    \"--version \"*) echo 1.0.0-fake ;;\n\
    \"status \"*) echo '{\"BackendState\":\"Running\",\"Self\":{\"DNSName\":\"fake-2365d.tailfake.ts.net.\"}}' ;;\n\
    \"serve status\") echo '{}' ;;\n\
    *) exit 1 ;;\n\
    esac\n";

/// 1 ケース分の砂場: data（表示言語）・発見の置き場・remote の state・ホーム `hm`・実行ファイルの置き場 `app`・
/// 偽の tailscale・tmux のソケット名（daemon の分）
struct Sandbox {
    dir: ScratchDir,
    real: PathBuf,
    tmux: String,
}

impl Sandbox {
    fn new(lang: &str, name: &str) -> Self {
        let dir = ScratchDir::new("i2365d");
        for d in ["data", "disc", "state", "hm", "app"] {
            let d = dir.path().join(d);
            std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
        }
        // 書かないと system = 実行環境のロケールで決まり、CI と手元で答えが変わる
        std::fs::write(
            dir.path().join("data/settings.json"),
            format!("{{\"language\":\"{lang}\"}}"),
        )
        .expect("settings.json を書ける");
        let ts = dir.path().join("tailscale-fake");
        std::fs::write(&ts, FAKE_TAILSCALE).expect("偽 tailscale を書ける");
        std::fs::set_permissions(&ts, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let real = std::fs::canonicalize(dir.path()).expect("砂場の実体を引ける");
        // 同じプロセスの 2 本のテストが同じケースを並行で走らせるので、通し番号で器を分ける
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, Ordering::SeqCst);
        let tmux = format!("t2365d-{}-{seq}-{lang}-{name}", std::process::id());
        Self { dir, real, tmux }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn state(&self) -> PathBuf {
        self.path().join("state")
    }

    /// state の置き場へファイルを書く
    fn put_state(&self, name: &str, text: &str) {
        std::fs::write(self.state().join(name), text)
            .unwrap_or_else(|e| panic!("state/{name} を書ける: {e}"));
    }

    /// 叩くバイナリを砂場の `app/tako`（実体のパス）へ置く。隔離中の `serve_binary()` は CLI 自身
    /// （`current_exe`）なので、PID ファイルの実行ファイルと突き合わせる start のケースはこのパスで叩く
    fn own_binary(&self) -> PathBuf {
        let bin = self.real.join("app/tako");
        if !bin.exists() {
            std::fs::copy(tako(), &bin)
                .unwrap_or_else(|e| panic!("{} へ複製できる: {e}", bin.display()));
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        bin
    }

    /// `TAKO_*` を外して隔離の置き場だけを渡した `<bin> <args>`（出力の砂場の dir は `<SB>` に置換）
    fn run_with(&self, bin: &Path, args: &[&str]) -> Out {
        let mut command = Command::new(bin);
        command
            .args(args)
            .current_dir(self.path().join("hm"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, _) in std::env::vars_os() {
            let key = key.to_string_lossy().to_string();
            if key.starts_with("TAKO_") || key.starts_with("CLAUDE") || key == "CODEX_HOME" {
                command.env_remove(key);
            }
        }
        command
            .env("TAKO_ISOLATED", "1")
            .env("TAKO_DATA_DIR", self.path().join("data"))
            .env("TAKO_DISCOVERY_DIR", self.path().join("disc"))
            .env("TAKO_REMOTE_STATE_DIR", self.state())
            .env("TAKO_REMOTE_TEST_MODE", "1")
            .env("TAKO_TAILSCALE_BIN", self.path().join("tailscale-fake"))
            .env("TAKO_TMUX_SOCKET", &self.tmux)
            .env("HOME", self.path().join("hm"));
        let out = command.output().expect("tako が終わる");
        let real = self.real.display().to_string();
        let root = self.path().display().to_string();
        let norm = |b: &[u8]| {
            String::from_utf8_lossy(b)
                .replace(&real, "<SB>")
                .replace(&root, "<SB>")
        };
        Out {
            code: out.status.code().unwrap_or(-1),
            stdout: norm(&out.stdout),
            stderr: norm(&out.stderr),
        }
    }

    fn run(&self, args: &[&str]) -> Out {
        self.run_with(&tako(), args)
    }

    /// PID ファイルの pid（無ければ `None`）
    fn daemon_pid(&self) -> Option<u32> {
        std::fs::read_to_string(self.state().join("tako-remote.pid"))
            .ok()?
            .lines()
            .next()?
            .trim()
            .parse()
            .ok()
    }
}

#[derive(Clone)]
struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

/// 生きている pid の代わり（tako remote serve ではない = 正体を確かめられない相手）。終わるときに畳む
struct Alive(Child);

impl Alive {
    fn new() -> Self {
        let child = Command::new("sleep")
            .arg("120")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sleep を起こせる");
        Self(child)
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }
}

impl Drop for Alive {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// 終わった pid（PID ファイルに残った死んだ daemon の代わり）
fn dead_pid() -> u32 {
    let mut child = Command::new("true").spawn().expect("true を起こせる");
    let pid = child.id();
    let _ = child.wait();
    pid
}

/// 記録された待ち受けに居る偽の daemon（どの要求にも決まった HTTP の答えを返す）。終わるときに閉じる
struct Listener {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Listener {
    fn new(response: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("待ち受けを開ける");
        let port = listener.local_addr().expect("ポートを引ける").port();
        listener.set_nonblocking(true).expect("nonblocking");
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                        // 要求のヘッダの終わりまで読む（生死の確かめ = 繋いで閉じるだけの相手もいる）
                        let mut buf = Vec::new();
                        let mut chunk = [0u8; 1024];
                        while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                            match stream.read(&mut chunk) {
                                Ok(0) | Err(_) => break,
                                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                            }
                        }
                        if !buf.is_empty() {
                            let _ = stream.write_all(response.as_bytes());
                        }
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        });
        Self {
            port,
            stop,
            thread: Some(thread),
        }
    }

    /// state の置き場へ待ち受けの記録（port ファイル）を書く
    fn record(&self, sb: &Sandbox) {
        sb.put_state("tako-remote.port", &self.port.to_string());
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// 決まった HTTP の答え
fn http(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// 1 ケースの用意で手放してはいけないもの（生きている pid・待ち受け）
#[derive(Default)]
struct Held {
    alive: Vec<Alive>,
    listeners: Vec<Listener>,
}

/// state の置き場に記録を置いて作るケース（名前・叩く引数・用意）。用意は叩くバイナリのパスを返す
/// （PID ファイルの実行ファイルと突き合わせる start のケースは砂場の複製）
type Setup = fn(&Sandbox, &mut Held) -> PathBuf;

/// 止める daemon が居ない（PID ファイルが無い）
fn stop_no_pid(_: &Sandbox, _: &mut Held) -> PathBuf {
    tako()
}

/// PID ファイルの pid は既に終わっている
fn stop_exited(sb: &Sandbox, _: &mut Held) -> PathBuf {
    sb.put_state(
        "tako-remote.pid",
        &format!("{}\n/bin/sleep\n1\n", dead_pid()),
    );
    tako()
}

/// PID ファイルの pid が tako remote serve ではない（正体を確かめられないので撃たない）
fn stop_unverified(sb: &Sandbox, held: &mut Held) -> PathBuf {
    let alive = Alive::new();
    sb.put_state(
        "tako-remote.pid",
        &format!("{}\n/bin/sleep\n1\n", alive.pid()),
    );
    held.alive.push(alive);
    tako()
}

/// PID ファイルが無く、記録された待ち受けは応答するが管理トークンが無い
fn stop_pidless_no_token(sb: &Sandbox, held: &mut Held) -> PathBuf {
    let listener = Listener::new(http("404 Not Found", "{}"));
    listener.record(sb);
    held.listeners.push(listener);
    tako()
}

/// 管理トークンはあるが、待ち受けの相手が 401 で断る（別の data dir の daemon）
fn stop_pidless_401(sb: &Sandbox, held: &mut Held) -> PathBuf {
    let listener = Listener::new(http("401 Unauthorized", r#"{"error":"x"}"#));
    listener.record(sb);
    sb.put_state("tako-remote.token", "test-admin-token");
    held.listeners.push(listener);
    tako()
}

/// 管理トークンはあるが、待ち受けの相手に停止要求の受け口が無い（#1599 より前の daemon / tako 以外）
fn stop_pidless_404(sb: &Sandbox, held: &mut Held) -> PathBuf {
    let listener = Listener::new(http("404 Not Found", r#"{"error":"x"}"#));
    listener.record(sb);
    sb.put_state("tako-remote.token", "test-admin-token");
    held.listeners.push(listener);
    tako()
}

/// 停止要求を受け付けた pid（もう終わっている）の後も待ち受けが応答している
fn stop_outlived(sb: &Sandbox, held: &mut Held) -> PathBuf {
    let body = format!(r#"{{"shutting_down":true,"pid":{}}}"#, dead_pid());
    let listener = Listener::new(http("200 OK", &body));
    listener.record(sb);
    sb.put_state("tako-remote.token", "test-admin-token");
    held.listeners.push(listener);
    tako()
}

/// 停止要求を受け付けた pid が終わらず、その正体（tako の daemon か）も確かめられない（待ちの 5 秒の後に断る）
fn stop_stale_unverified(sb: &Sandbox, held: &mut Held) -> PathBuf {
    let alive = Alive::new();
    let body = format!(r#"{{"shutting_down":true,"pid":{}}}"#, alive.pid());
    let listener = Listener::new(http("200 OK", &body));
    listener.record(sb);
    sb.put_state("tako-remote.token", "test-admin-token");
    held.alive.push(alive);
    held.listeners.push(listener);
    tako()
}

/// 稼働中の daemon の serve バイナリが今の解決先と違う
fn start_binary_mismatch(sb: &Sandbox, held: &mut Held) -> PathBuf {
    let alive = Alive::new();
    sb.put_state(
        "tako-remote.pid",
        &format!("{}\n/opt/other/tako\n{}\n", alive.pid(), now() + 3600),
    );
    held.alive.push(alive);
    tako()
}

/// 稼働中の daemon が今の版のまま動いている（起動の時刻が実行ファイルより後）
fn start_already_running(sb: &Sandbox, held: &mut Held) -> PathBuf {
    let bin = sb.own_binary();
    let alive = Alive::new();
    sb.put_state(
        "tako-remote.pid",
        &format!("{}\n{}\n{}\n", alive.pid(), bin.display(), now() + 3600),
    );
    held.alive.push(alive);
    bin
}

/// 旧版のまま動いている daemon を入れ替えようとして、その正体を確かめられず止められない
fn start_renew_unverified(sb: &Sandbox, held: &mut Held) -> PathBuf {
    let bin = sb.own_binary();
    let alive = Alive::new();
    sb.put_state(
        "tako-remote.pid",
        &format!("{}\n{}\n1\n", alive.pid(), bin.display()),
    );
    held.alive.push(alive);
    bin
}

/// PID ファイルは無いが、記録された待ち受けで別の daemon が応答している
fn start_listener_busy(sb: &Sandbox, held: &mut Held) -> PathBuf {
    let listener = Listener::new(http("404 Not Found", "{}"));
    listener.record(sb);
    held.listeners.push(listener);
    tako()
}

const STATE_CASES: &[(&str, &[&str], Setup)] = &[
    ("stop-no-pid", &["remote", "stop"], stop_no_pid),
    (
        "stop-force-no-pid",
        &["remote", "stop", "--force"],
        stop_no_pid,
    ),
    ("stop-exited", &["remote", "stop"], stop_exited),
    ("stop-unverified", &["remote", "stop"], stop_unverified),
    (
        "stop-pidless-no-token",
        &["remote", "stop"],
        stop_pidless_no_token,
    ),
    ("stop-pidless-401", &["remote", "stop"], stop_pidless_401),
    ("stop-pidless-404", &["remote", "stop"], stop_pidless_404),
    ("stop-outlived", &["remote", "stop"], stop_outlived),
    (
        "stop-stale-unverified",
        &["remote", "stop"],
        stop_stale_unverified,
    ),
    (
        "start-binary-mismatch",
        &["remote", "start"],
        start_binary_mismatch,
    ),
    (
        "start-already-running",
        &["remote", "start"],
        start_already_running,
    ),
    (
        "start-renew-unverified",
        &["remote", "start"],
        start_renew_unverified,
    ),
    (
        "start-listener-busy",
        &["remote", "start"],
        start_listener_busy,
    ),
];

/// 起こした daemon を必ず畳む（stop が失敗した回も、覚えた pid を SIGTERM → 残れば SIGKILL）
struct Daemons<'a> {
    sb: &'a Sandbox,
    pids: Vec<u32>,
}

impl Drop for Daemons<'_> {
    fn drop(&mut self) {
        if let Some(pid) = self.sb.daemon_pid() {
            self.pids.push(pid);
        }
        let _ = self.sb.run(&["remote", "stop"]);
        for pid in &self.pids {
            let _ = Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .stderr(Stdio::null())
                .status();
        }
        for pid in &self.pids {
            for _ in 0..50 {
                let alive = Command::new("kill")
                    .args(["-0", &pid.to_string()])
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success());
                if !alive {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .stderr(Stdio::null())
                .status();
        }
        // 器のソケットファイルまで消す 1 実装（tmux は kill-server の後もファイルを残す = #2393）
        tako_core::tmux_backend::kill_server(&self.sb.tmux);
    }
}

/// 本物の daemon: 起こす → もう一度 start（既に起動中）→ stop → もう一度 stop（PID ファイルが無い）
fn run_daemon_cases(lang: &str) -> Vec<(String, Out)> {
    let sb = Sandbox::new(lang, "daemon");
    let mut daemons = Daemons {
        sb: &sb,
        pids: Vec::new(),
    };
    let mut outs = Vec::new();
    let start = sb.run(&["remote", "start"]);
    daemons.pids.extend(sb.daemon_pid());
    assert_eq!(
        start.code, 0,
        "[{lang}] daemon を起こせる（終了コード）: {}",
        start.stderr
    );
    outs.push((
        "daemon-start-again".to_string(),
        sb.run(&["remote", "start"]),
    ));
    let stop = sb.run(&["remote", "stop"]);
    assert_eq!(
        stop.code, 0,
        "[{lang}] daemon を止められる: {}",
        stop.stderr
    );
    outs.push(("daemon-stop-again".to_string(), sb.run(&["remote", "stop"])));
    drop(daemons);
    outs
}

/// 1 言語分の全ケースの出力（名前, 出力）と、本物の daemon のケースを走らせたか
fn run_lang(lang: &str) -> (Vec<(String, Out)>, bool) {
    let mut outs = Vec::new();
    for (name, args, setup) in STATE_CASES {
        let sb = Sandbox::new(lang, name);
        let mut held = Held::default();
        let bin = setup(&sb, &mut held);
        outs.push((name.to_string(), sb.run_with(&bin, args)));
        drop(held);
    }
    let daemon = tako_core::backend::capabilities().detached_access;
    if daemon {
        outs.extend(run_daemon_cases(lang));
    }
    if let Some(dir) = std::env::var_os("I2365D_DUMP").map(PathBuf::from) {
        let _ = std::fs::create_dir_all(&dir);
        for (name, out) in &outs {
            let _ = std::fs::write(
                dir.join(format!("{lang}-{name}.txt")),
                format!(
                    "exit={}\n--- stdout\n{}--- stderr\n{}",
                    out.code, out.stdout, out.stderr
                ),
            );
        }
    }
    (outs, daemon)
}

/// 本物の daemon のケースを走らせたかを 1 行で残す（`I2365D_REQUIRE_DAEMON=1` なら走らないことを失敗にする）
fn report_daemon_cases(ran: bool, n: usize) {
    if ran {
        eprintln!("[issue2365 remote refusal] 本物の daemon のケース: 走った（{n} 件）");
    } else {
        eprintln!(
            "[issue2365 remote refusal] 本物の daemon のケース: tmux が無い（remote daemon は起動を拒む）ので走らせていない"
        );
        assert!(
            std::env::var("I2365D_REQUIRE_DAEMON").as_deref() != Ok("1"),
            "I2365D_REQUIRE_DAEMON=1 なのに本物の daemon のケースを走らせていない（tmux が無い）"
        );
    }
}

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

/// 数字の並びを `0` 1 字に均す（pid・ポート・時刻は実行ごとに桁数も変わる）
fn norm(s: &str) -> String {
    let mut folded = String::with_capacity(s.len());
    let mut in_digits = false;
    for c in s.chars() {
        if c.is_ascii_digit() {
            if !in_digits {
                folded.push('0');
            }
            in_digits = true;
        } else {
            folded.push(c);
            in_digits = false;
        }
    }
    folded
}

/// #2365 より前の日本語表示の stderr（`norm` 済み。`<SB>` は砂場）。修正前のバイナリで採った出力と同じ
fn old_ja(name: &str) -> String {
    const PREFIX: &str = "PID 0 が tako remote serve であることを確認できません\
                          （PID 再利用または検証コマンド実行不能）。安全のため停止操作を中止しました。";
    let pidless = |reason: &str| {
        format!(
            "error: PID ファイルが無く、記録された待ち受け（0.0.0.0:0）は応答していますが、\
             停止要求が通りませんでした: {reason}\n\
             相手が tako の daemon か確かめられないので、何も止めていません。state ファイルは残しました。\n\
             \x20 待ち受けている相手を確かめる: lsof -nP -iTCP:0 -sTCP:LISTEN\n\
             \x20 tako の daemon なら止める: kill <PID>\n\
             \x20 tako の daemon でなければ記録を消す: rm <SB>/state/tako-remote.port\n\
             \x20 そのうえで起動し直す: tako remote start\n"
        )
    };
    let unverified = format!(
        "{PREFIX}\n\
         PID ファイルに記録された PID です。state ファイルは残しました。\n\
         \x20 この PID の正体を確認する: ps -p 0 -o args=\n\
         \x20 tako remote serve なら止める: kill 0\n\
         \x20 tako の daemon でなければ記録を消す: rm <SB>/state/tako-remote.pid\n\
         \x20 そのうえで起動し直す: tako remote start"
    );
    match name {
        "stop-no-pid" | "stop-force-no-pid" | "daemon-stop-again" => {
            "error: リモートサーバーが起動していない（PID ファイルが無い）\n".into()
        }
        "stop-exited" => "error: リモートサーバーが起動していない（プロセスは既に終了）\n".into(),
        "stop-unverified" => format!("error: {unverified}\n"),
        "stop-pidless-no-token" => pidless("管理トークンも無い"),
        "stop-pidless-401" => pidless("管理トークンが合わない (0)"),
        "stop-pidless-404" => pidless("停止要求の受け口が無い (0。#0 より前の daemon か、tako 以外)"),
        "stop-outlived" => "PID ファイルが消失していますが、記録された待ち受けの daemon（PID 0）が停止要求を\
                            受け付けました。終了を確かめます…\n\
                            error: 停止要求を受け付けた PID 0 は終わりましたが、記録された待ち受け（0.0.0.0:0）は\
                            まだ応答しています。応答したのは別のプロセスかもしれないので、止めたことにせず\
                            state ファイルを残しました。\n  待ち受けている相手を確かめる: lsof -nP -iTCP:0 -sTCP:LISTEN\n"
            .into(),
        "stop-stale-unverified" => format!(
            "PID ファイルが消失していますが、記録された待ち受けの daemon（PID 0）が停止要求を受け付けました。終了を確かめます…\n\
             error: {PREFIX}\n\
             記録された待ち受け（0.0.0.0:0）が答えた PID です。state ファイルは残しました。\n\
             \x20 この PID の正体を確認する: ps -p 0 -o args=\n\
             \x20 tako の daemon でなければ記録を消す: rm <SB>/state/tako-remote.port\n\
             \x20 そのうえで起動し直す: tako remote start\n"
        ),
        "start-binary-mismatch" => "error: リモートサーバーは既に起動中ですが、稼働中の serve バイナリ\
                                    （/opt/other/tako）が現在の解決先（<BIN>）と異なります。\
                                    `tako remote stop` してから `tako remote start` で差し替えてください\n"
            .into(),
        "start-already-running" | "daemon-start-again" => {
            "error: リモートサーバーは既に起動中\n".into()
        }
        "start-renew-unverified" => format!(
            "error: 旧版のまま動いているリモート daemon（PID 0）を止められなかったため、\
             入れ替えていません: {unverified}\n"
        ),
        "start-listener-busy" => {
            "error: 別の daemon が既にこの待ち受けで稼働中です: 0.0.0.0:0\n".into()
        }
        other => panic!("見本の無いケース: {other}"),
    }
}

/// 比べる前の均し: 数字・叩いたバイナリのパス（mismatch の「現在の解決先」）
fn comparable(out: &Out) -> String {
    let bin = tako().display().to_string();
    norm(&out.stderr.replace(&bin, "<BIN>"))
}

#[test]
fn remoteのstartとstopの断りは表示言語に従い日本語は2365より前と同じ() {
    let (ja, ran_ja) = run_lang("ja");
    let (en, ran_en) = run_lang("en");
    report_daemon_cases(ran_ja && ran_en, ja.len() - STATE_CASES.len());
    let mut failures = Vec::new();
    assert_eq!(ja.len(), en.len(), "日英でケースの数が違う");
    for ((name, j), (_, e)) in ja.iter().zip(&en) {
        // どのケースも断る（終了コード 1・stdout は空）
        if j.code == 0 || e.code != j.code {
            failures.push(format!(
                "{name}: 終了コード（ja {} / en {}）が断りの形でない・日英で違う\n  ja: {}\n  en: {}",
                j.code, e.code, j.stderr, e.stderr
            ));
        }
        if j.stdout != e.stdout || !j.stdout.is_empty() {
            failures.push(format!(
                "{name}: stdout が日英で違う・断りなのに何か書いた\n  ja: {}\n  en: {}",
                j.stdout, e.stdout
            ));
        }
        // 英語表示の断りの行（stderr の診断 = 「終了を確かめます…」の進みの行は日本語のまま）
        for line in e.stderr.lines() {
            if line.contains("終了を確かめます") {
                continue;
            }
            if let Some(c) = has_japanese(line) {
                failures.push(format!("{name}: 英語表示に日本語（{c}）: {line}"));
            }
        }
        let want = old_ja(name);
        let got = comparable(j);
        if got != want {
            failures.push(format!(
                "{name}: 日本語表示が #2365 より前と違う\n  want: {want:?}\n  got:  {got:?}"
            ));
        }
        if j.stderr.lines().count() != e.stderr.lines().count() {
            failures.push(format!(
                "{name}: 行の数が日英で違う（ja {} / en {}）\n  en: {}",
                j.stderr.lines().count(),
                e.stderr.lines().count(),
                e.stderr
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn 英語表示の断りの見本() {
    // 英語の文の要点（日英の対の取り違え・言い換えの書き忘れを拾う。全文の一致は番犬の表が見る）
    let want: &[(&str, &[&str])] = &[
        ("stop-no-pid", &["error: the remote server is not running (no PID file)"]),
        (
            "stop-exited",
            &["error: the remote server is not running (the process has already exited)"],
        ),
        (
            "stop-unverified",
            &[
                "error: cannot confirm that PID 0 is tako remote serve",
                "This is the PID recorded in the PID file. The state files were kept.",
                "  check what this PID is: ps -p 0 -o args=",
                "  if it is tako remote serve, stop it: kill 0",
                "  then start it again: tako remote start",
            ],
        ),
        (
            "stop-pidless-no-token",
            &[
                "the stop request did not go through: there is no admin token either",
                "  check who is listening: lsof -nP -iTCP:0 -sTCP:LISTEN",
            ],
        ),
        (
            "stop-pidless-401",
            &["did not go through: the admin token does not match (0)"],
        ),
        (
            "stop-pidless-404",
            &["did not go through: there is no stop endpoint (0: a daemon older than #0, or not tako)"],
        ),
        (
            "stop-outlived",
            &["error: PID 0, which accepted the stop request, has exited, but the recorded listener"],
        ),
        (
            "stop-stale-unverified",
            &["This is the PID that the recorded listener (0.0.0.0:0) answered with."],
        ),
        (
            "start-binary-mismatch",
            &["error: the remote server is already running, but its serve binary (/opt/other/tako) differs"],
        ),
        (
            "start-already-running",
            &["error: the remote server is already running\n"],
        ),
        (
            "start-renew-unverified",
            &["error: could not stop the remote daemon still running the old version (PID 0), so it was not swapped: cannot confirm"],
        ),
        (
            "start-listener-busy",
            &["error: another daemon is already running on this listener: 0.0.0.0:0"],
        ),
    ];
    let (en, ran) = run_lang("en");
    report_daemon_cases(ran, en.len() - STATE_CASES.len());
    let mut failures = Vec::new();
    for (name, needles) in want {
        let Some((_, out)) = en.iter().find(|(n, _)| n == name) else {
            failures.push(format!("{name}: ケースが無い"));
            continue;
        };
        let got = comparable(out);
        for needle in *needles {
            if !got.contains(needle) {
                failures.push(format!("{name}: `{needle}` が無い\n  got: {got}"));
            }
        }
    }
    if ran {
        for name in ["daemon-start-again", "daemon-stop-again"] {
            let out = &en.iter().find(|(n, _)| n == name).expect(name).1;
            let want = if name == "daemon-start-again" {
                "error: the remote server is already running\n"
            } else {
                "error: the remote server is not running (no PID file)\n"
            };
            if comparable(out) != want {
                failures.push(format!(
                    "{name}: 本物の daemon の断りが違う: {}",
                    out.stderr
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
