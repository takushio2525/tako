//! 実 tmux を使う e2e の共通部品（Issue #1300。テスト専用）
//!
//! ## なぜ要るか（真因）
//!
//! 器（tmux サーバー）の名前が**固定**だと、同じ機で `cargo test --workspace` が
//! 2 本走った瞬間に**同じサーバーの同じセッション名**を取り合う。この機は worktree を
//! 分けた worker が並ぶので、これは例外ではなく日常的に起こる。
//!
//! 実測（#1300・2026-09-11）: 同一バイナリ 2 本を同時に走らせると片方が
//! **0.03 秒で** `duplicate session: tako1236new` に当たって落ちる。そのときの器は
//! PTY 103/511・ソケット 206 個で、**枯渇はしていない**（#1265 の「PTY 枯渇」とは
//! 別の真因）。取り合いは逆向きにも効く: 先に走っている側のセッションを、
//! 後から来た側の後始末（`kill-session -t <固定名>`）が**横から消せてしまう**。
//!
//! ## ここが持つ約束
//!
//! - 器の名前は**プロセスごと**（[`socket_for`]）。固定名は A/B の旧アームでしか作らない
//! - `tmux new-session` の失敗は**理由と器の状態**を連れて返る（[`new_session`]）。
//!   素の `assert!(status.success(), "tmux new-session が失敗した")` は
//!   「なぜ落ちたか」を 1 ビットも残さない = #1300 の観測がまさにこれだった
//! - tmux を叩く経路は**すべて期限つき**（返らなければ子を殺して戻る。#1271 と同じ理由。
//!   器が応答しない場面でこそ診断が要るのに、素の `output()` では診断ごと固まる）
//! - 後始末はソケットファイルまで消す。tmux は `kill-server` でファイルを**残す**
//!   （実測: この機の 208 個のうち 173 個が pid つきテストソケットの残骸）ので、
//!   プロセスごとの名前にするだけだと残骸が 1 回 1 個ずつ増えてしまう
//! - **器を畳むのは「誰も借りていない」が確定しているときだけ**（#1866）。
//!   借りる側は `tmux new-session` を叩く**前に**数に入り（予約）、返す側は
//!   「数を減らす → 0 か見る → `kill-server` → ソケットを消す」を**同じロックの中で**行う
//! - 器は `-f /dev/null` で起こす（#1874。利用者の `~/.tmux.conf` を読ませない）
//!
//! ## #1866: 器を畳む判定が、隣のテストの起動中の器を消していた
//!
//! 旧実装は「`new-session` が返ってから数を増やす」「数を減らしてから別に 0 か見て
//! 畳む」だった。同じバイナリの 3 本目のテストが `new-session` を叩いている最中
//! （器にセッションはできたが、まだ数に入っていない）に先の 2 本が返ると、
//! 最後に返った側が数 0 を見て器を畳み、ソケットファイルまで消す。3 本目は
//! `error connecting to …/tako-e2e-1259-<pid> (No such file or directory)` で落ちる
//! （PR #1862 の全体テストで観測。`new-session` の返りを遅らせる注入で main は 1/1・
//! 同じ数え方の旧アームは 3/3 で再現し、消したのは**同じプロセスの後始末**だったことを
//! 時系列で確かめた）。
//! 負荷で `new-session` が遅くなるほど窓が広がるので、全体テストの時だけ落ちていた。
//! A/B は `TAKO_1866_LEGACY=1`、固定は `issue1866_tmux_e2e_refcount.rs`。
//!
//! 番犬 `crates/tako-control/tests/tmux_e2e_watchdog.rs` が、固定名の器と
//! 診断なしの `new-session`、1 実装の外で器を畳む呼び出しが戻ってきたら落とす。

// 取り込む側（実 tmux e2e 8 本）ごとに使う項目が違う
#![allow(dead_code)]

use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// tmux の CLI は正常なら数十 ms で返る。ここは「返らない」を見分けるための上限で、
/// 待ち時間の見積もりではない
pub const BASE: Duration = Duration::from_secs(15);

/// 診断の採取に使う上限（応答しない器でも診断だけは返す。#1265 と同じ理由）
pub const DIAG_BASE: Duration = Duration::from_secs(5);

/// 子の終了を見に行く間隔（`try_wait` は安い syscall）
const POLL: Duration = Duration::from_millis(20);

/// **このプロセスが今この器に持っているセッション数**（起動中の予約を含む）。
///
/// 1 つのテストバイナリの中でテストは**並列に走る**（`claude_tui_e2e` は 7 本）。
/// 器はプロセス単位なので、1 本が終わるたびに `kill-server` すると
/// **同じバイナリの隣のテストのセッションまで巻き添えで消える**。最後の 1 本が
/// 返したときだけ器を畳む。
///
/// 原子変数ではなくロックにしているのは、「0 になった」と「器を畳む」の間に
/// 隣の予約を割り込ませないため（#1866。判定と kill を別々に行うと、その隙に
/// 起動した隣の器を消す）
static LIVE: Mutex<usize> = Mutex::new(0);

/// 数のロック。テストの panic で毒が入っても数そのものは正しいので中身を使う
fn live() -> MutexGuard<'static, usize> {
    LIVE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A/B の旧アーム（`TAKO_1866_LEGACY=1`）。#1866 を観測したときの数え方へ戻す:
/// 数に入るのは `new-session` が**返ってから**、器を畳む判定は数を減らした**後に別に**行う
pub fn legacy_1866() -> bool {
    std::env::var("TAKO_1866_LEGACY").is_ok_and(|v| v == "1")
}

/// `new-session` が成功して**呼び手へ返る直前**に呼ぶ差し込み口（#1866 の固定テスト用）。
///
/// 「器にセッションはできたが、呼び手はまだそれを使い始めていない」瞬間に
/// 隣の後始末を走らせるために使う。旧アームではこの時点でまだ数に入っていない
/// （= 修正前の窓そのもの）。既定は空
type AfterNewSession = Box<dyn Fn(&[&str]) + Send + Sync>;
static AFTER_NEW_SESSION: Mutex<Option<AfterNewSession>> = Mutex::new(None);

pub fn set_after_new_session(hook: impl Fn(&[&str]) + Send + Sync + 'static) {
    *AFTER_NEW_SESSION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(hook));
}

fn after_new_session(args: &[&str]) {
    let hook = AFTER_NEW_SESSION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(hook) = hook.as_ref() {
        hook(args);
    }
}

/// A/B の旧アーム（`TAKO_1300_LEGACY=1`）。器の名前を**固定**へ戻し、
/// `new-session` の失敗も素の 1 行だけにする（= #1300 を観測したときの形）
pub fn legacy_1300() -> bool {
    std::env::var("TAKO_1300_LEGACY").is_ok_and(|v| v == "1")
}

/// 「起動が失敗する」を注入する（`TAKO_1300_INJECT=duplicate`）。
/// 本番と同じ失敗（同名セッションの取り合い）を**器を壊さずに**作れるので、
/// 旧アーム（理由が出ない）と新アーム（理由が割れる）を同一バイナリで並べられる
fn inject_duplicate() -> bool {
    std::env::var("TAKO_1300_INJECT").is_ok_and(|v| v == "duplicate")
}

/// この機の混み具合。**プロセスで 1 回だけ読む**
pub fn busy() -> Option<f64> {
    static BUSY: std::sync::OnceLock<Option<f64>> = std::sync::OnceLock::new();
    *BUSY.get_or_init(tako_core::wait_budget::machine_busy)
}

/// 上限は `tako_core::wait_budget` の 1 実装を通す（伸ばすだけ・4 倍で打ち切り）
pub fn budget_for(base: Duration) -> Duration {
    tako_core::wait_budget::state_wait_budget(base, busy())
}

/// **このテストプロセス専用**の器の名前（`-L` に渡す）。
///
/// 1 テストバイナリ = 1 タグなのでプロセス内で 1 度だけ作って使い回す。
/// `tag` は Issue 番号など（`"1236"` → `tako-e2e-1236-<pid>`）
pub fn socket_for(tag: &str) -> &'static str {
    static NAME: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();
    let (first, name) = NAME.get_or_init(|| {
        let name = if legacy_1300() {
            // 旧アーム: 固定名 = 他プロセスと同じ器を取り合う形
            format!("tako-e2e-{tag}")
        } else {
            format!("tako-e2e-{tag}-{}", std::process::id())
        };
        (tag.to_string(), name)
    });
    // 1 バイナリ = 1 タグ。破ると**黙って最初のタグの器**を返して取り違えるので落とす
    assert_eq!(
        first, tag,
        "1 テストバイナリに 2 つのタグを使っている（器の名前は最初のものが返る）"
    );
    name.as_str()
}

/// 実行 1 回の顛末
#[derive(Debug)]
pub struct Ran {
    /// 終了コードが 0 だったか（**期限切れは常に false**）
    pub ok: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// 期限で打ち切ったか
    pub timed_out: bool,
    pub waited: Duration,
}

/// `tmux -L <socket> <args…>` を**期限つきで**走らせる。
///
/// 出力はパイプではなく一時ファイルへ採る（#1271: `output()` は子の終了だけでなく
/// **パイプの EOF まで**待つので、クライアントが起こしたサーバーがハンドルを
/// 引き継ぐと「子は死んだのに読み側が返らない」が起こりうる）
pub fn run_tmux(socket: &str, args: &[&str], base: Duration) -> Ran {
    let started = Instant::now();
    let budget = budget_for(base);
    let (out_path, err_path) = capture_paths();

    let mut cmd = Command::new("tmux");
    cmd.arg("-L").arg(socket).args(args);
    cmd.stdin(Stdio::null());
    cmd.stdout(sink(&out_path));
    cmd.stderr(sink(&err_path));

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            take_capture(&out_path);
            take_capture(&err_path);
            return Ran {
                ok: false,
                code: None,
                stdout: String::new(),
                stderr: format!("tmux を spawn できない: {err}"),
                timed_out: false,
                waited: started.elapsed(),
            };
        }
    };

    let deadline = Instant::now() + budget;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Ran {
                    ok: status.success(),
                    code: status.code(),
                    stdout: take_capture(&out_path),
                    stderr: take_capture(&err_path),
                    timed_out: false,
                    waited: started.elapsed(),
                };
            }
            Ok(None) => {}
            Err(err) => {
                return Ran {
                    ok: false,
                    code: None,
                    stdout: take_capture(&out_path),
                    stderr: format!("try_wait が失敗した: {err}"),
                    timed_out: false,
                    waited: started.elapsed(),
                };
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ran {
                ok: false,
                code: None,
                stdout: take_capture(&out_path),
                stderr: take_capture(&err_path),
                timed_out: true,
                waited: started.elapsed(),
            };
        }
        std::thread::sleep(POLL);
    }
}

/// 器を起こす `new-session` の頭。`-f /dev/null` で利用者の `~/.tmux.conf` を読ませない
/// （#1874: 無いと利用者の prefix・status・フック・`run-shell` までテストの器で動く）
const NEW_SESSION: [&str; 3] = ["-f", "/dev/null", "new-session"];

/// `tmux -L <socket> -f /dev/null new-session <args…>` を走らせ、失敗したら
/// **理由 + 器の状態**を `Err` で返す（そのまま `panic!` へ流せる 1 枚の診断）
pub fn new_session(socket: &str, args: &[&str]) -> Result<(), String> {
    let legacy = legacy_1866();
    if !legacy {
        // **器を叩く前に数に入る**（#1866）。こうしておけば、起動中に隣が最後の 1 本を
        // 返しても数は 0 にならず、器もソケットも畳まれない
        *live() += 1;
    }
    if inject_duplicate() {
        // 先に同じセッションを作っておく = 本番と同じ「取り合い」の形にする
        let mut first = NEW_SESSION.to_vec();
        first.extend_from_slice(args);
        let _ = run_tmux(socket, &first, BASE);
    }
    let mut full = NEW_SESSION.to_vec();
    full.extend_from_slice(args);
    let ran = run_tmux(socket, &full, BASE);
    if ran.ok {
        after_new_session(args);
        if legacy {
            // 旧アーム: 返ってから数に入る（この手前の窓で隣が器を畳める = #1866）
            *live() += 1;
        }
        return Ok(());
    }
    if legacy_1300() {
        // 旧アーム: 理由も器の状態も残さない（#1300 を観測したときの assert）
        if !legacy {
            unreserve(socket);
        }
        return Err("tmux new-session が失敗した".to_string());
    }
    let diag = format!(
        "tmux new-session が失敗した: {}\n  実行: tmux -L {socket} {}\n  器: {}\n  枯渇: {}\n  負荷: {}",
        reason(&ran),
        full.join(" "),
        sessions_on(socket),
        exhaustion(),
        load_line(),
    );
    // これから落ちるので、自分専用の器は畳んでおく（注入した残骸も道連れにする）。
    // **隣のテストがセッションを持っていれば畳まない**（旧アームも触らない）
    if legacy {
        kill_server_if_idle(socket);
    } else {
        unreserve(socket);
    }
    Err(diag)
}

/// 予約を 1 つ返し、それが最後なら器を畳む（失敗した `new_session` の後始末）
fn unreserve(socket: &str) {
    let mut live = live();
    *live = live.saturating_sub(1);
    retire_if_unused(socket, &live);
}

/// `tmux -L <socket> send-keys <args…>` を期限つきで送る。
/// 失敗は**理由つき**で返す（素の `assert!(status.success(), "send-keys が失敗した")`
/// は new-session と同じで、届かなかった理由を 1 ビットも残さない）
pub fn send_keys(socket: &str, args: &[&str]) -> Result<(), String> {
    let mut full = vec!["send-keys"];
    full.extend_from_slice(args);
    let ran = run_tmux(socket, &full, BASE);
    if ran.ok {
        return Ok(());
    }
    Err(format!(
        "tmux send-keys が失敗した: {}\n  実行: tmux -L {socket} {}\n  器: {}",
        reason(&ran),
        full.join(" "),
        sessions_on(socket),
    ))
}

/// 失敗の 1 行（終了コード / 期限切れ / stderr）
fn reason(ran: &Ran) -> String {
    if ran.timed_out {
        return format!(
            "期限 {:.1}s を過ぎても返らなかった（器が応答していない）",
            ran.waited.as_secs_f64()
        );
    }
    let code = ran
        .code
        .map(|c| c.to_string())
        .unwrap_or_else(|| "シグナルで終了".to_string());
    let stderr = ran.stderr.trim();
    if stderr.is_empty() {
        format!("exit={code} stderr=<空>")
    } else {
        format!("exit={code} stderr={stderr:?}")
    }
}

/// このソケットに載っているセッション（`duplicate session` の相手が見える）
fn sessions_on(socket: &str) -> String {
    let ran = run_tmux(
        socket,
        &[
            "list-sessions",
            "-F",
            "#{session_name}(created=#{session_created})",
        ],
        DIAG_BASE,
    );
    if ran.timed_out {
        return format!("socket={socket} list-sessions が期限内に返らない");
    }
    let names: Vec<&str> = ran.stdout.split_whitespace().collect();
    if names.is_empty() {
        format!(
            "socket={socket} セッション=0（{}）",
            ran.stderr.trim().replace('\n', " ")
        )
    } else {
        format!("socket={socket} セッション={} {names:?}", names.len())
    }
}

/// 枯渇の目安（#1265 の真因と切り分けるための数字）
fn exhaustion() -> String {
    let ptys = count_dev_ptys();
    let max = ptmx_max();
    let sockets = socket_file_count();
    let servers = live_tmux_servers();
    format!(
        "PTY={}/{} tmux ソケットファイル={} 生きた tmux サーバー={}",
        ptys.map(|n| n.to_string()).unwrap_or_else(|| "?".into()),
        max.map(|n| n.to_string()).unwrap_or_else(|| "?".into()),
        sockets.map(|n| n.to_string()).unwrap_or_else(|| "?".into()),
        servers.map(|n| n.to_string()).unwrap_or_else(|| "?".into()),
    )
}

fn load_line() -> String {
    busy()
        .map(|b| format!("load/CPU={b:.2}"))
        .unwrap_or_else(|| "load/CPU=unknown".to_string())
}

/// `/dev/ttys*` の数（= 使用中の PTY）
fn count_dev_ptys() -> Option<usize> {
    let read = std::fs::read_dir("/dev").ok()?;
    Some(
        read.flatten()
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with("ttys"))
            })
            .count(),
    )
}

/// PTY の上限（macOS の `kern.tty.ptmx_max`）
fn ptmx_max() -> Option<usize> {
    let ran = timed_capture("sysctl", &["-n", "kern.tty.ptmx_max"], DIAG_BASE)?;
    ran.trim().parse().ok()
}

/// ソケットディレクトリのファイル数（残骸を含む総数）
fn socket_file_count() -> Option<usize> {
    let dir = tako_core::tmux_backend::socket_dir()?;
    Some(std::fs::read_dir(dir).ok()?.flatten().count())
}

/// 生きている tmux サーバーのプロセス数（ソケット総数との差が「残骸」）
fn live_tmux_servers() -> Option<usize> {
    let out = timed_capture("ps", &["-eo", "comm="], DIAG_BASE)?;
    Some(
        out.lines()
            .filter(|l| {
                std::path::Path::new(l.trim())
                    .file_name()
                    .and_then(|n| n.to_str())
                    == Some("tmux")
            })
            .count(),
    )
}

/// 診断のための外部コマンドも**期限つき**（器が応答しない場面でこそ要る）
fn timed_capture(program: &str, args: &[&str], base: Duration) -> Option<String> {
    let budget = budget_for(base);
    let (out_path, err_path) = capture_paths();
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(sink(&out_path))
        .stderr(sink(&err_path))
        .spawn()
        .ok()?;
    let deadline = Instant::now() + budget;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(_) => break,
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            take_capture(&err_path);
            take_capture(&out_path);
            return None;
        }
        std::thread::sleep(POLL);
    }
    take_capture(&err_path);
    Some(take_capture(&out_path))
}

/// セッション 1 つを畳む（期限つき）。器には触らない
pub fn kill_session(socket: &str, session: &str) {
    let _ = run_tmux(socket, &["kill-session", "-t", session], BASE);
}

/// テストが借りたセッションを返す。**このプロセスの最後の 1 本**なら器ごと退役させ、
/// ソケットファイルまで消す（`EmuGuard` / `SessionGuard` の `Drop` はこれを呼ぶ）
pub fn release_session(socket: &str, session: &str) {
    kill_session(socket, session);
    if legacy_1866() {
        // 旧アーム: 減らしてから**ロックの外で**別に 0 か見る（#1866 の形）
        {
            let mut live = live();
            *live = live.saturating_sub(1);
        }
        kill_server_if_idle(socket);
        return;
    }
    // 減らす・0 か見る・畳むを 1 つのロックの中で行う（隣の予約を割り込ませない）
    let mut live = live();
    *live = live.saturating_sub(1);
    retire_if_unused(socket, &live);
}

/// **旧アーム（`TAKO_1866_LEGACY=1`）専用**。借りているセッションが 0 本なら器を畳む。
///
/// 数を読んでから畳むまでの間はロックを手放す（= #1866 の窓そのもの）ので、
/// 取り込む側からは呼べないようにしてある。器を返すのは [`release_session`] だけ
fn kill_server_if_idle(socket: &str) {
    let idle = *live() == 0;
    if idle {
        retire_if_unused(socket, &0);
    }
}

/// `live`（**ロック中の数**）が 0 なら器を畳む。呼び手はロックを持ったまま渡すので、
/// 判定から `kill-server`・ソケットの除去までの間に隣が予約を入れることはない
/// （予約はロックを待ち、畳み終わった後に新しい器を起こす）。
///
/// 旧アーム（固定名 = 他プロセスと共有）では器ごと落とすと他プロセスの巻き添えに
/// なるので触らない。
///
/// 製品側にも `tako_core::tmux_backend::kill_server` があるが、そちらは素の
/// `output()` で**期限が無い**（返らない器に当たるとテストごと固まって Drop が
/// 1 つも走らない = #1271）。ここは期限つきで叩き、ファイルの除去だけ 1 実装を借りる
fn retire_if_unused(socket: &str, live: &usize) {
    if legacy_1300() || *live > 0 {
        return;
    }
    let _ = run_tmux(socket, &["kill-server"], BASE);
    // tmux は `kill-server` でソケットファイルを残す（実測）。正本は tako-core
    tako_core::tmux_backend::remove_socket_file(socket);
}

fn sink(path: &PathBuf) -> Stdio {
    File::create(path)
        .ok()
        .map(Stdio::from)
        .unwrap_or_else(Stdio::null)
}

fn capture_paths() -> (PathBuf, PathBuf) {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let base = std::env::temp_dir().join(format!("tako-1300-cap-{}-{n}", std::process::id()));
    (base.with_extension("out"), base.with_extension("err"))
}

fn take_capture(path: &PathBuf) -> String {
    let mut text = String::new();
    if let Ok(mut f) = File::open(path) {
        let _ = f.read_to_string(&mut text);
    }
    let _ = std::fs::remove_file(path);
    text
}
