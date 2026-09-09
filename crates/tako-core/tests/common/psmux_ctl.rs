//! 器（psmux）を叩くときの**期限つき実行**と後始末（#1271。テスト専用）
//!
//! ## なぜ要るか
//!
//! `Command::output()` には**タイムアウトが無い**。psmux のクライアントが返らない
//! ことがあり（実測: `kill-server` と `send-keys -X scroll-up` が CPU 6% のまま
//! 返らない）、そこでテストプロセスが無限に待つと
//!
//! 1. その run が終わらない（バイナリ全体が固まる）
//! 2. **`Fixture::drop` が 1 つも走らない**ので器（サーバー）と中の pwsh が残る
//!
//! の 2 段で効く。#1114 の作業中の実測では **1 回の実行あたり器が約 2 個**残り、
//! 半日で 25 → 74 個まで積み上がって機が目に見えて遅くなった。
//!
//! ## 数え方の注意（#1271 の見落としの正体）
//!
//! psmux のパッケージは `psmux.exe` / `tmux.exe` / `pmux.exe` を同じ実体で配り、
//! **サーバーは起動したクライアントの実行ファイル名を名乗る**（実測）。テストの器は
//! `psmux.exe server …`、製品経路（`tmux_backend` は `tmux` として起動する）は
//! `tmux.exe server …` になるので、`Get-Process psmux` だけで数えると片方が
//! **0 に見える**。残骸は `tmux.exe` / `psmux.exe` / `pwsh.exe` の 3 つで数えること。
//!
//! ## ここが持つ約束
//!
//! - psmux を叩く経路は**すべて期限つき**（返らなければ子を殺して戻る）
//! - 後始末は [`kill_server`] の 4 段（pid を聞く → `kill-server` → **pid 指定**で止める
//!   → 掃き掃除）。**名前一致の一括 kill は絶対にしない**
//!   （`Stop-Process -Name` / `taskkill /IM` は他インスタンス・他ワーカーの器を巻き込む）
//! - 上限の政策は `tako_core::wait_budget::state_wait_budget` の 1 実装を通す
//!   （伸ばすだけ・4 倍で打ち切り。`.agent/conventions.md`「セルフテストの待ち条件の書き方」）
//!
//! 番犬 `crates/tako-control/tests/psmux_cleanup_timeout_watchdog.rs` が、
//! ここと呼び出し側へ素の `Command::output()` が戻ってきたら落とす。

// 2 つの統合テスト（`psmux_backend` / `shell_integration_powershell`）が同じ実装を
// 取り込むが、macOS では後者が `#![cfg(windows)]` で丸ごと消えるため、
// 片方からしか使われない項目が出る
#![allow(dead_code)]

use std::fs::File;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// 期限つき実行の素の上限。psmux の CLI は正常なら数十 ms で返るので、
/// ここは「返らない」を見分けるための上限であって待ち時間の見積もりではない
pub const BASE: Duration = Duration::from_secs(15);

/// 器の pid を聞く / pid 指定で止める側の素の上限（server を経由しないので短くてよい）
pub const SHORT_BASE: Duration = Duration::from_secs(8);

/// 子の終了を見に行く間隔（`try_wait` は安い syscall）
const POLL: Duration = Duration::from_millis(20);

/// pid の在籍を見に行く間隔。**Windows の在籍確認は全プロセスの列挙**（Toolhelp）
/// なので、`try_wait` と同じ 20ms で回すと自分で負荷を作る（#1114 の `STATE_POLL` と同じ粒度）
const ALIVE_POLL: Duration = Duration::from_millis(250);

/// 注入（A/B）で使う「返らないコマンド」。psmux の `run-shell` は **`-b` 無しだと
/// 中のコマンドが終わるまでクライアントを返さない**（実測: 12 秒待っても返らない）ので、
/// これで「後始末が返らない」状況を**器を生かしたまま**作れる。
/// `wait-for` は psmux では即座に rc=0 で返るので使えない（実測）
const INJECT_BLOCKER: &str = if cfg!(windows) {
    "powershell -NoProfile -Command Start-Sleep 3600"
} else {
    "sleep 3600"
};

/// この機の混み具合。**プロセスで 1 回だけ読む**
/// （Windows の読み手は 120ms ブロックするので毎周期は呼ばない）
pub fn busy() -> Option<f64> {
    static BUSY: std::sync::OnceLock<Option<f64>> = std::sync::OnceLock::new();
    *BUSY.get_or_init(tako_core::wait_budget::machine_busy)
}

/// 状態待ちの上限（**伸ばすだけ**・4 倍で打ち切り）。政策は `tako_core::wait_budget` の 1 実装
pub fn budget_for(base: Duration) -> Duration {
    tako_core::wait_budget::state_wait_budget(base, busy())
}

/// A/B の旧アーム（`TAKO_1271_LEGACY=1`）。後始末を**期限なし**の素の待ちへ戻す
fn legacy_1271() -> bool {
    std::env::var("TAKO_1271_LEGACY").is_ok_and(|v| v == "1")
}

/// 「後始末が返らない」を注入する（`TAKO_1271_INJECT=hang`）。
/// `kill-server` の代わりに**同じ psmux の返らないコマンド**（[`INJECT_BLOCKER`]）を
/// 呼ぶので、器は生きたまま残る = 期限と pid 指定の止めが効いているかを直接見られる
fn inject_hang() -> bool {
    std::env::var("TAKO_1271_INJECT").is_ok_and(|v| v == "hang")
}

/// 後始末で叩く引数（注入時だけ「返らないコマンド」へ差し替える）。
/// **`-L` 必須**（省くと全ソケットのサーバーが死ぬ）
fn cleanup_args(socket: &str) -> Vec<&str> {
    if inject_hang() {
        vec!["-L", socket, "run-shell", INJECT_BLOCKER]
    } else {
        vec!["-L", socket, "kill-server"]
    }
}

/// 実行 1 回の顛末
#[derive(Debug)]
pub struct Ran {
    /// 終了コードが 0 だったか（**期限切れは常に false**）
    pub ok: bool,
    /// stdout に続けて stderr（`Command::output()` の従来の連結順と同じ）
    pub text: String,
    /// 期限で打ち切ったか
    pub timed_out: bool,
    pub waited: Duration,
    pub budget: Duration,
}

/// 後始末 1 回の顛末
#[derive(Debug)]
pub struct Cleanup {
    /// 見つけた器（サーバー）の pid。**空 = pid 指定の止めが打てなかった**
    pub server_pids: Vec<u32>,
    /// 素直な `kill-server` が期限で打ち切られたか
    pub kill_timed_out: bool,
    /// pid 指定の強制終了まで進んだか
    pub forced: bool,
    /// 掃き掃除の `kill-server` まで進んだか
    pub swept: bool,
    /// **後始末を終えても、掴んだ pid がまだ生きている**（= 本当のリーク）
    pub leaked: bool,
    pub waited: Duration,
}

impl Cleanup {
    /// 素直に片付いた回は黙る（18 本並列のログを汚さない）
    fn abnormal(&self) -> bool {
        self.kill_timed_out || self.forced || self.leaked
    }

    fn diag(&self, socket: &str) -> String {
        format!(
            "TAKO_1271_CLEANUP socket={socket} server_pids={:?} kill_timed_out={} \
             forced={} swept={} leaked={} waited={:.1}s load={}",
            self.server_pids,
            self.kill_timed_out,
            self.forced,
            self.swept,
            self.leaked,
            self.waited.as_secs_f64(),
            busy()
                .map(|b| format!("{b:.2}"))
                .unwrap_or_else(|| "unknown".to_string()),
        )
    }
}

/// 組み立て済みの `Command` を**期限つきで**走らせる。
///
/// 返らなければ子を殺して戻る（`ok=false` / `timed_out=true`）。
///
/// 出力はパイプではなく**一時ファイル**へ採る。`Command::output()` は子の終了だけで
/// なく**パイプの EOF まで**待つので、psmux のクライアントが起こした器がその
/// ハンドルを引き継ぐと「子は死んだのに読み側が返らない」が起こりうる。ファイルなら
/// 書き手が残っていても読み側は止まらない。**この機序そのものは未確認**だが、
/// パイプからファイルへ替えると同じテストの所要が **31.1s → 22.5s** になった
/// （交互 6 ラウンドの実測。`.agent/plans/2026-08-windows-main-merge-wip.md` の #1271 の節）
pub fn wait_bounded(cmd: &mut Command, base: Duration) -> Ran {
    let budget = budget_for(base);
    let label = describe(cmd);
    let started = Instant::now();

    let (out_path, err_path) = capture_paths();
    cmd.stdin(Stdio::null());
    cmd.stdout(
        File::create(&out_path)
            .ok()
            .map(Stdio::from)
            .unwrap_or_else(Stdio::null),
    );
    cmd.stderr(
        File::create(&err_path)
            .ok()
            .map(Stdio::from)
            .unwrap_or_else(Stdio::null),
    );

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            take_capture(&out_path);
            take_capture(&err_path);
            return Ran {
                ok: false,
                text: format!("spawn できない: {err}"),
                timed_out: false,
                waited: started.elapsed(),
                budget,
            };
        }
    };

    let mut ok = false;
    let mut timed_out = false;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                ok = status.success();
                break;
            }
            Err(_) => break,
            Ok(None) => {}
        }
        if started.elapsed() >= budget {
            timed_out = true;
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        std::thread::sleep(POLL);
    }

    let waited = started.elapsed();
    let mut text = take_capture(&out_path);
    text.push_str(&take_capture(&err_path));
    if timed_out {
        eprintln!(
            "TAKO_1271_TIMEOUT cmd={label} waited={:.1}s budget={:.1}s load={}",
            waited.as_secs_f64(),
            budget.as_secs_f64(),
            busy()
                .map(|b| format!("{b:.2}"))
                .unwrap_or_else(|| "unknown".to_string()),
        );
    }
    Ran {
        ok,
        text,
        timed_out,
        waited,
        budget,
    }
}

/// psmux を期限つきで叩く（引数は呼び出し側が `-L` から組む）
pub fn psmux(bin: &str, args: &[&str], base: Duration) -> Ran {
    let mut cmd = Command::new(bin);
    cmd.args(args);
    wait_bounded(&mut cmd, base)
}

/// そのバイナリが psmux として動くか（`-V` の成否）。**期限つき**なので
/// 壊れた psmux を掴んでも `cargo test` が固まらない
pub fn probe(bin: &str) -> bool {
    psmux(bin, &["-V"], BASE).ok
}

/// 隔離ソケット上の器を落とす（#1271 の本体）。
///
/// 1. **先に**器の pid を聞く（この後 `kill-server` が返らなくなっても pid で止められる）
/// 2. 期限つきの `kill-server`
/// 3. 素直に済まなかったときだけ回収へ進む: 掴んだ pid が生きていれば **pid 指定**で
///    止め（`/T` で中の pwsh ごと）、器を聞き直し、最後に `kill-server` で掃く
///
/// 3 の最後の「掃き掃除」が要るのは、**器を聞いても見つからない残骸がある**から。
/// psmux は 1 ソケットにつき本体サーバーの子として `__warm__` サーバーを持つが、
/// 本体が先に退役すると（テストがセッションを閉じた後の `exit-empty`）
/// `display-message` は「器は居ない」と答えるのに `__warm__` は生き続ける。
/// **これを消せるのは `kill-server` だけ**（実測: 本体を pid で落とした後、
/// `display-message` は rc=1 / `kill-server` は rc=0 で `__warm__` が消える）
pub fn kill_server(bin: &str, socket: &str) -> Cleanup {
    let started = Instant::now();
    if legacy_1271() {
        legacy_kill_server(bin, socket);
        return Cleanup {
            server_pids: Vec::new(),
            kill_timed_out: false,
            forced: false,
            swept: false,
            leaked: false,
            waited: started.elapsed(),
        };
    }

    let mut pids: Vec<u32> = server_pid(bin, socket).into_iter().collect();
    let killed = psmux(bin, &cleanup_args(socket), BASE);

    let mut forced = false;
    let mut swept = false;
    if killed.timed_out || pids.iter().copied().any(pid_alive) {
        // 器は後から答えられるようになることがある（返らなかった呼び出しが
        // 器を起こしていた場合など）ので、もう一度だけ聞き直す
        if let Some(pid) = server_pid(bin, socket) {
            if !pids.contains(&pid) {
                pids.push(pid);
            }
        }
        for pid in pids.iter().copied().filter(|pid| pid_alive(*pid)) {
            forced = true;
            force_kill(pid);
        }
        // 器として名乗り出ない残骸（`__warm__`）はこれでしか消えない
        swept = true;
        let _ = psmux(bin, &["-L", socket, "kill-server"], SHORT_BASE);
    }

    let leaked = pids.iter().copied().any(still_alive_after);
    let done = Cleanup {
        server_pids: pids,
        kill_timed_out: killed.timed_out,
        forced,
        swept,
        leaked,
        waited: started.elapsed(),
    };
    if done.abnormal() {
        eprintln!("{}", done.diag(socket));
    }
    done
}

/// その pid が生きているか（`tako_core` の 1 実装へ委譲。`filter` へ関数として渡せる形）
fn pid_alive(pid: u32) -> bool {
    tako_core::platform::process::pid_alive(pid)
}

/// **A/B の旧アーム**（`TAKO_1271_LEGACY=1`）。期限も pid 指定の止めも持たない
/// 修正前そのままの形で、`kill-server` が返らないと**ここで永久に止まる**。
/// 番犬はこの `legacy_` 接頭辞の関数だけを対象外にする
fn legacy_kill_server(bin: &str, socket: &str) {
    let _ = Command::new(bin).args(cleanup_args(socket)).output();
}

/// 器（サーバー）の pid。器が居なければ `None`。
/// `display-message` は器を起こさないので、居ない相手に呼んでも副作用が無い
fn server_pid(bin: &str, socket: &str) -> Option<u32> {
    let ran = psmux(
        bin,
        &["-L", socket, "display-message", "-p", "#{pid}"],
        SHORT_BASE,
    );
    if !ran.ok {
        return None;
    }
    ran.text.trim().parse::<u32>().ok()
}

/// **pid 指定**の強制終了。名前一致（`/IM` / `-Name`）は他の器を巻き込むので使わない。
/// Windows は `/T` で子（器の中の pwsh）ごと落とす
fn force_kill(pid: u32) {
    let pid = pid.to_string();
    let mut cmd = if cfg!(windows) {
        let mut cmd = Command::new("taskkill");
        cmd.args(["/PID", &pid, "/T", "/F"]);
        cmd
    } else {
        let mut cmd = Command::new("kill");
        cmd.args(["-KILL", &pid]);
        cmd
    };
    let _ = wait_bounded(&mut cmd, SHORT_BASE);
}

/// 強制終了を打ってからも生き残っているか（**時間の比較ではなく在籍**で見る）
fn still_alive_after(pid: u32) -> bool {
    let budget = budget_for(SHORT_BASE);
    let started = Instant::now();
    loop {
        if !pid_alive(pid) {
            return false;
        }
        if started.elapsed() >= budget {
            return true;
        }
        std::thread::sleep(ALIVE_POLL);
    }
}

/// 診断に出す「何を叩いたか」。プログラムは**ファイル名だけ**にする
/// （`TAKO_PSMUX_BIN` に絶対パスを渡された場合に、診断行へ実ホームを載せないため）
fn describe(cmd: &Command) -> String {
    let program = std::path::Path::new(cmd.get_program());
    let mut out = program
        .file_name()
        .unwrap_or(cmd.get_program())
        .to_string_lossy()
        .into_owned();
    for arg in cmd.get_args() {
        out.push(' ');
        out.push_str(&arg.to_string_lossy());
    }
    out
}

/// 出力の採取先（プロセス内で一意）
fn capture_paths() -> (PathBuf, PathBuf) {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir();
    let stem = format!("tako-1271-{}-{seq}", std::process::id());
    (
        dir.join(format!("{stem}.out")),
        dir.join(format!("{stem}.err")),
    )
}

/// 採取先を読んで消す（読めなければ空）
fn take_capture(path: &PathBuf) -> String {
    let text = std::fs::read(path)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    let _ = std::fs::remove_file(path);
    text
}
