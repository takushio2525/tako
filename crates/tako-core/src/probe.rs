//! 外部コマンドの待ちに上限を持たせる 1 実装（Issue #1503）
//!
//! ## 何が起きていたのか
//!
//! `tako setup` はエージェント CLI の状態を**外部コマンドへ聞いて**決める
//! （`claude auth status --json` / `codex login status` / `agy models` /
//! `claude mcp list`）。この問い合わせ（= probe）が全部
//! `std::process::Command::output()` で、**待ち時間の上限が無かった**。
//!
//! `claude mcp list` は登録済みの MCP サーバへ 1 台ずつ繋いで健全性を見るので、
//! **サーバが 1 つ無応答だとそこで返らなくなる**。#1500 の棚卸し R4 では
//! 20 秒経っても返らない claude に対して `tako setup` が**無言で 6 分固まり続けた**。
//! dispatch（GUI / MCP 経路）は `tako setup` を子として `wait_with_output()` で
//! 待つので、そちらはボタンが永久に回る。
//!
//! ## この形（上限を持ち、超えたら打ち切って次へ進む）
//!
//! - **待ちの上限は 1 実装**（[`wait_with_timeout`]）。probe も setup 本体も
//!   ここを通す。呼び手が `.output()` / `.wait_with_output()` を直に書くと
//!   上限がその場で消えるので、番犬
//!   `crates/tako-control/tests/issue1503_probe_timeout_watchdog.rs` が
//!   file:line で落とす
//! - **無言にしない**。打ち切ったら [`TimeoutNotice`] を出す（何を何秒待ったか）。
//!   文面と読み取りが同じ場所にあるので、CLI が出した行を dispatch が
//!   [`parse_notices`] で拾って応答 JSON へ載せられる（開発不変条件: UI で
//!   分かることは MCP / CLI からも分かる）
//! - **上限は外せない**。env での上書き（[`PROBE_TIMEOUT_ENV`]）は値を変えるだけで、
//!   0 / 不正 / 空はすべて既定へ落ちる。「無制限」という指定は用意しない
//! - **読み切りにも同じ予算を掛ける**。`Command::output()` は子が終わっても
//!   パイプの書き手（子が残した孫プロセス）が居ると `read_to_end` で返らない。
//!   ここは吸い出しを別スレッドへ出し、**join しない**（予算内に溜まったぶんだけ取る）
//!
//! ## 分かっている限界
//!
//! - **打ち切りで kill するのは直接の子だけ**。子が残した孫は孤児として残る
//!   （`claude mcp list` は MCP サーバを子として起こす）。上限が無かったときは
//!   そもそも永久に待っていたので後退ではないが、孫まで畳むには生成時に
//!   プロセスグループを分ける必要があり、Windows は job object が要るので別の話
//! - **読み切りの猶予は「予算の残り」と 2 秒の小さいほう**。子が予算ぎりぎりで
//!   終わると、パイプに残ったぶんを読む時間がほとんど無い。「全体で予算を超えない」を
//!   契約にしてある（孫が書き手として残ると EOF は来ないので、2 秒で頭打ちにする）
//!
//! ## 上限の根拠（実測）
//!
//! | probe | 実測（MCP 13 サーバ登録の実機・3 回） |
//! |---|---|
//! | `claude auth status --json` | 0.12〜0.13 秒 |
//! | `claude mcp list` | 4.57 / 4.71 / 5.26 秒 |
//!
//! 既定 [`DEFAULT_PROBE_TIMEOUT`] = **15 秒**は、いちばん遅い probe の**約 3 倍**。
//! `tako setup` 本体（[`DEFAULT_SETUP_RUN_TIMEOUT`]）は導入器
//! （`curl | bash` でのエージェント CLI 取得）が走るので桁が違う。
//!
//! ## A/B
//!
//! `TAKO_1503_LEGACY=1` で上限を持たない #1503 前の待ちへ戻る（= 固まる）。
//! ゲートは [`legacy_unbounded`] の 1 か所だけ。
//!
//! ## 子プロセスでない相手を待つとき（Issue #1662）
//!
//! `tako run --wait` / `tako run-interactive --wait` は、実行ペインの終わりを IPC で
//! **2 秒ごとに聞き直す**形で待つ（相手は子プロセスではない）。ここも上限が無く、
//! サーバーや入力待ちを `--wait` で走らせると CLI が永久に返らなかった。
//! 聞き直しの上限は [`poll_with_timeout`] の 1 実装で、既定は
//! [`DEFAULT_RUN_WAIT_TIMEOUT`]（env [`RUN_WAIT_TIMEOUT_ENV`]。扱いは probe と同じで
//! 0 / 不正 / 空は既定へ落ちる = 外せない）

use std::io::Read;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// probe（読み取りだけの問い合わせ）の既定の上限。根拠はモジュール冒頭の実測表
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// `tako setup` 本体を子として待つときの既定の上限。
///
/// probe と桁が違うのは、この中で**導入器**（`curl | bash` によるエージェント CLI の
/// 取得。回線次第で分単位）が走るため。それでも有限にするのは、dispatch 経由だと
/// GUI / MCP 側のボタンが永久に回るから（#1503）
pub const DEFAULT_SETUP_RUN_TIMEOUT: Duration = Duration::from_secs(600);

/// probe の上限の上書き（秒）。0 / 不正 / 空は既定へ落ちる（**上限は外せない**）
pub const PROBE_TIMEOUT_ENV: &str = "TAKO_SETUP_PROBE_TIMEOUT_SECS";

/// `tako setup` 本体の上限の上書き（秒）。扱いは [`PROBE_TIMEOUT_ENV`] と同じ
pub const SETUP_RUN_TIMEOUT_ENV: &str = "TAKO_SETUP_RUN_TIMEOUT_SECS";

/// `tako run --wait` / `tako run-interactive --wait` が実行ペインの終わりを待つ
/// 既定の上限（#1662）。
///
/// 根拠（実測）: Code Runner で走らせる正当な実行でいちばん重いのは「冷えたビルド」で、
/// このリポジトリの新しい worktree で `cargo build -p tako-cli -p tako-app` が
/// **128.8 秒**だった。600 秒はその約 4.7 倍で、`--wait` を叩く主な呼び手
/// （AI エージェントのシェル実行ツール。Claude Code の Bash ツールは最長 600 秒）の
/// 上限とも揃う（それより長く待っても、呼び手のほうが先に打ち切られる）
pub const DEFAULT_RUN_WAIT_TIMEOUT: Duration = Duration::from_secs(600);

/// `--wait` の上限の上書き（秒）。扱いは [`PROBE_TIMEOUT_ENV`] と同じ（**0 は既定へ**）
pub const RUN_WAIT_TIMEOUT_ENV: &str = "TAKO_RUN_WAIT_TIMEOUT_SECS";

/// 打ち切りを知らせる 1 行の頭。CLI が出し、dispatch が拾う（**文面は 1 実装**）
const NOTICE_HEAD: &str = "[確認できません] ";
const NOTICE_OPEN: char = '（';
const NOTICE_TAIL: &str = " 秒応答なし）";

/// `try_wait` のポーリング間隔。いちばん速い probe（0.12 秒）に対して十分細かい
const POLL: Duration = Duration::from_millis(20);

/// **子が終わったあと**、パイプに残ったぶんを読み切るのに待つ上限。
///
/// 予算の残り全部を使わないのが要点。子が残した孫が書き手として居ると
/// EOF は永遠に来ないので、そこを予算いっぱい待つと「ちゃんと終わった子」の待ちが
/// 上限まで伸びる（実測: 並列 `cargo test --workspace` の負荷下で予算 30 秒の
/// 単体テストが 30 秒掛かった回がある）。パイプ容量は 64 KiB なので 2 秒あれば桁で足りる
const READ_GRACE: Duration = Duration::from_secs(2);

/// 待ちの結末。
///
/// `Command::output()` の `Result<Output>` と違い、「上限を超えたので打ち切った」を
/// **失敗と区別して**持つ。呼び手はこれを見て「確認できないまま次へ進む」を選べる
#[derive(Debug)]
pub enum Outcome {
    /// 上限内に終わった
    Done {
        status: ExitStatus,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    /// 上限を超えたので打ち切った（子は kill 済み）。出力は溜まったぶんだけ
    TimedOut {
        label: String,
        waited: Duration,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    /// 起動できない / 待ちそのものに失敗した
    Failed { label: String, reason: String },
}

impl Outcome {
    /// 上限内に終わったときだけ [`std::process::Output`] を返す。
    ///
    /// 既存の `Option<Output>` を返す呼び手（`setup.rs` の `command_output`）が
    /// 差分ゼロで乗るための口。打ち切り / 失敗はどちらも `None`
    pub fn into_output(self) -> Option<std::process::Output> {
        match self {
            Self::Done {
                status,
                stdout,
                stderr,
            } => Some(std::process::Output {
                status,
                stdout,
                stderr,
            }),
            _ => None,
        }
    }

    /// 打ち切ったときの知らせ（**無言にしないための材料**）
    pub fn timeout_notice(&self) -> Option<TimeoutNotice> {
        match self {
            Self::TimedOut { label, waited, .. } => Some(TimeoutNotice {
                label: label.clone(),
                waited_secs: waited.as_secs(),
            }),
            _ => None,
        }
    }
}

/// 「何を何秒待って諦めたか」。
///
/// [`std::fmt::Display`] が出す 1 行を [`parse_notices`] が読み戻す
/// （**文面と読み取りが同じ場所**なので、片方だけ変えるとラウンドトリップの
/// 単体テストが落ちる）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeoutNotice {
    /// 何を待ったか。`label` が作る「実行ファイル名 + 引数」で、**パスは含めない**
    /// （#927。診断やヘッドレスの応答 JSON へ実ホームパスを載せないため）
    pub label: String,
    /// 何秒待ったか
    pub waited_secs: u64,
}

impl std::fmt::Display for TimeoutNotice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{NOTICE_HEAD}{}{NOTICE_OPEN}{}{NOTICE_TAIL}。打ち切って次へ進みます",
            self.label, self.waited_secs
        )
    }
}

/// 出力テキストから打ち切りの知らせを拾う（行頭の字下げは無視する）。
///
/// dispatch は `tako setup` を子として起こすので、**子が出した行を読み戻して**
/// 応答 JSON へ載せる。`Display` と対になっているのでここだけを直せない
pub fn parse_notices(text: &str) -> Vec<TimeoutNotice> {
    text.lines().filter_map(parse_notice_line).collect()
}

fn parse_notice_line(line: &str) -> Option<TimeoutNotice> {
    let rest = line.trim_start().strip_prefix(NOTICE_HEAD)?;
    let tail_at = rest.rfind(NOTICE_TAIL)?;
    let (head, _) = rest.split_at(tail_at);
    let open_at = head.rfind(NOTICE_OPEN)?;
    let label = head[..open_at].trim();
    let secs = head[open_at + NOTICE_OPEN.len_utf8()..].trim();
    (!label.is_empty()).then_some(TimeoutNotice {
        label: label.to_string(),
        waited_secs: secs.parse().ok()?,
    })
}

/// 待ちの名札。**実行ファイル名と引数だけ**で、パスは落とす。
///
/// 名札は診断ログにも応答 JSON にも載るので、`/Users/<名前>/.local/bin/claude` を
/// そのまま持つと実ホームパスが漏れる（#927）
pub fn label(program: &str, args: &[&str]) -> String {
    let name = std::path::Path::new(program)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(program);
    if args.is_empty() {
        name.to_string()
    } else {
        format!("{name} {}", args.join(" "))
    }
}

/// probe の上限（env で上書き可。外すことはできない）
pub fn probe_timeout() -> Duration {
    parse_timeout_secs(
        std::env::var(PROBE_TIMEOUT_ENV).ok().as_deref(),
        DEFAULT_PROBE_TIMEOUT,
    )
}

/// `tako setup` 本体の上限（env で上書き可。外すことはできない）
pub fn setup_run_timeout() -> Duration {
    parse_timeout_secs(
        std::env::var(SETUP_RUN_TIMEOUT_ENV).ok().as_deref(),
        DEFAULT_SETUP_RUN_TIMEOUT,
    )
}

/// `--wait` の上限（env で上書き可。外すことはできない。#1662）
pub fn run_wait_timeout() -> Duration {
    parse_timeout_secs(
        std::env::var(RUN_WAIT_TIMEOUT_ENV).ok().as_deref(),
        DEFAULT_RUN_WAIT_TIMEOUT,
    )
}

/// 上限つきの聞き直し（[`poll_with_timeout`]）の結末
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Polled<T> {
    /// 上限内（上限ちょうどを含む）に答えが出た
    Done(T),
    /// 上限まで聞き直しても答えが出なかった。`waited` は実際に待った長さ
    TimedOut { waited: Duration },
}

/// 状態を**上限つきで**聞き直す 1 実装（#1662）。
///
/// `step` が `Some` を返したら [`Polled::Done`]、`Err` はそのまま返す。
/// 間隔の眠りは**残り時間で頭打ち**にするので上限を `interval` ぶん踏み越えず、
/// 上限に達した回も**打ち切る前に 1 回は聞く**ので、上限ちょうどに終わったものを
/// 取りこぼさない（どちらも `poll_with_clock` の単体テストが時計を差し替えて固定する）
pub fn poll_with_timeout<T, E>(
    budget: Duration,
    interval: Duration,
    step: impl FnMut() -> Result<Option<T>, E>,
) -> Result<Polled<T>, E> {
    let start = Instant::now();
    poll_with_clock(
        budget,
        interval,
        step,
        || start.elapsed(),
        std::thread::sleep,
    )
}

/// [`poll_with_timeout`] の本体（時計と眠りを差し替えられる形）
fn poll_with_clock<T, E>(
    budget: Duration,
    interval: Duration,
    mut step: impl FnMut() -> Result<Option<T>, E>,
    mut elapsed: impl FnMut() -> Duration,
    mut sleep: impl FnMut(Duration),
) -> Result<Polled<T>, E> {
    // 間隔 0 で空回りしない（呼び手の取り違えで CPU を 1 本食い潰さない）
    let interval = interval.max(POLL);
    loop {
        if let Some(value) = step()? {
            return Ok(Polled::Done(value));
        }
        let now = elapsed();
        if now >= budget {
            return Ok(Polled::TimedOut { waited: now });
        }
        sleep(interval.min(budget - now));
    }
}

/// env 値の解釈（純粋関数）。**0 / 不正 / 空はすべて既定**へ落とす。
///
/// 「0 = 無制限」を作らないのが要点。作ると #1503 が env 1 つで戻る
pub fn parse_timeout_secs(raw: Option<&str>, default: Duration) -> Duration {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .map(Duration::from_secs)
        .unwrap_or(default)
}

/// #1503 前の待ち（上限なし）へ戻す A/B のゲート。**ここ 1 か所だけ**
pub fn legacy_unbounded() -> bool {
    std::env::var("TAKO_1503_LEGACY").is_ok_and(|v| v == "1")
}

/// コマンドを起こして、上限つきで待つ。
///
/// **`Command` の組み立てごとここが持つ**（呼び手に `Command::new` を書かせない）。
/// コンソールウィンドウの抑止（#586 / #628）と `stdin` の遮断を呼び手が忘れる余地を
/// 無くすためで、番犬 `platform_parity` の「素の子プロセス起動」もこれで増えない。
/// `stdin` を塞ぐのは、probe が親の標準入力を持っていくと dispatch 経路で
/// setup へ流している answers を横取りしうるため。
///
/// 名札（`label`）は [`label`] が組む = 引数だけで、**パスは含めない**（#927）
pub fn output_with_timeout(program: &str, args: &[&str], budget: Duration) -> Outcome {
    let name = label(program, args);
    let mut cmd = Command::new(program);
    crate::platform::process::no_console_window(&mut cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match cmd.spawn() {
        Ok(child) => wait_with_timeout(child, &name, budget),
        Err(e) => Outcome::Failed {
            label: name,
            reason: e.to_string(),
        },
    }
}

/// 起こし済みの子を上限つきで待つ（**待ちの 1 実装**）。
///
/// `stdout` / `stderr` がパイプなら吸い出す。呼び手が既に `take()` していても動く
/// （その口は無いものとして扱う）
pub fn wait_with_timeout(mut child: Child, label: &str, budget: Duration) -> Outcome {
    // A/B は入口 1 か所。旧挙動 = 予算が尽きないので `try_wait` を回し続ける
    let budget = if legacy_unbounded() {
        Duration::MAX
    } else {
        budget
    };
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // 子は終わったが、パイプの書き手（子が残した孫）がまだ居ることがある。
                // `Command::output()` はここで無限に待つので、読み切りにも予算を掛ける。
                // 予算の残り全部ではなく [`READ_GRACE`] で頭打ちにする
                let grace = Instant::now();
                while !(out.finished() && err.finished())
                    && grace.elapsed() < READ_GRACE
                    && start.elapsed() < budget
                {
                    std::thread::sleep(POLL);
                }
                return Outcome::Done {
                    status,
                    stdout: out.snapshot(),
                    stderr: err.snapshot(),
                };
            }
            Ok(None) => {
                if start.elapsed() >= budget {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Outcome::TimedOut {
                        label: label.to_string(),
                        waited: budget,
                        stdout: out.snapshot(),
                        stderr: err.snapshot(),
                    };
                }
                std::thread::sleep(POLL);
            }
            Err(e) => {
                return Outcome::Failed {
                    label: label.to_string(),
                    reason: e.to_string(),
                }
            }
        }
    }
}

/// パイプを吸い出す係。**join しない**のが要点（孫が書き手として残っていると
/// `read` は返らないので、join すると上限そのものが意味を失う）
struct Drain {
    buf: Arc<Mutex<Vec<u8>>>,
    done: Arc<AtomicBool>,
}

impl Drain {
    fn finished(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    fn snapshot(&self) -> Vec<u8> {
        self.buf.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

fn drain<R: Read + Send + 'static>(src: Option<R>) -> Drain {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let done = Arc::new(AtomicBool::new(true));
    let Some(mut src) = src else {
        return Drain { buf, done };
    };
    done.store(false, Ordering::SeqCst);
    let (sink, flag) = (Arc::clone(&buf), Arc::clone(&done));
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match src.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Ok(mut guard) = sink.lock() {
                        guard.extend_from_slice(&chunk[..n]);
                    }
                }
            }
        }
        flag.store(true, Ordering::SeqCst);
    });
    Drain { buf, done }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// シェル片を走らせる子（両 OS）。`Command` を組まずに `(実行ファイル, 引数)` で返す
    /// （`output_with_timeout` が組み立てを持つので、テストもその形に合わせる）
    fn shell(script: &str) -> (&'static str, Vec<String>) {
        #[cfg(windows)]
        {
            ("cmd", vec!["/C".to_string(), script.to_string()])
        }
        #[cfg(not(windows))]
        {
            ("/bin/sh", vec!["-c".to_string(), script.to_string()])
        }
    }

    /// **返ってこない子**の script。`prefix` を先に出してから固まる。
    /// Windows の定番 sleep は `ping -n`（`timeout` は stdin を要求する）
    fn hang_script(prefix: Option<&str>) -> String {
        #[cfg(windows)]
        {
            match prefix {
                Some(p) => format!("echo {p}& ping -n 600 127.0.0.1 > NUL"),
                None => "ping -n 600 127.0.0.1 > NUL".to_string(),
            }
        }
        #[cfg(not(windows))]
        {
            match prefix {
                Some(p) => format!("printf %s {p}; sleep 600"),
                None => "sleep 600".to_string(),
            }
        }
    }

    /// `shell()` の戻りを `output_with_timeout` へ渡す
    fn run_shell(script: &str, budget: Duration) -> Outcome {
        let (program, args) = shell(script);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        output_with_timeout(program, &refs, budget)
    }

    #[test]
    fn 上限内に終わる子はdoneで出力ごと返る() {
        #[cfg(windows)]
        let script = "echo ok";
        #[cfg(not(windows))]
        let script = "printf %s ok";
        let outcome = run_shell(script, Duration::from_secs(30));
        match outcome {
            Outcome::Done { status, stdout, .. } => {
                assert!(status.success(), "終了コード");
                assert!(
                    String::from_utf8_lossy(&stdout).contains("ok"),
                    "stdout を取りこぼしている"
                );
            }
            other => panic!("Done を期待したが {other:?}"),
        }
    }

    #[test]
    fn 返らない子は上限で打ち切られる() {
        let budget = Duration::from_millis(300);
        let outcome = run_shell(&hang_script(None), budget);
        match outcome {
            Outcome::TimedOut { label, waited, .. } => {
                assert!(
                    label.contains("sh") || label.contains("cmd"),
                    "名札: {label}"
                );
                assert_eq!(waited, budget, "何秒待ったかを知らせる");
            }
            other => panic!("TimedOut を期待したが {other:?}"),
        }
    }

    /// #1503 の肝。`try_wait` のポーリングだけだと「少し出してから固まる子」で
    /// パイプが詰まり、**吸い出しの側**で止まって上限が効かなくなる
    #[test]
    fn 出力を少し出してから固まる子も打ち切られ溜まったぶんは残る() {
        // 予算は**桁で開ける**（子が 1 行 echo するのは実測 ~2ms。並列の
        // `cargo test --workspace` で詰まっても届く余裕を取る）
        let outcome = run_shell(&hang_script(Some("partial")), Duration::from_secs(5));
        match outcome {
            Outcome::TimedOut { stdout, .. } => {
                assert!(
                    String::from_utf8_lossy(&stdout).contains("partial"),
                    "打ち切る前に出ていたぶんは持ち帰る（実際は {:?}）",
                    String::from_utf8_lossy(&stdout)
                );
            }
            other => panic!("TimedOut を期待したが {other:?}"),
        }
    }

    /// 子が終わっても**パイプに残ったぶん**を取りこぼさない。
    ///
    /// `Command::output()` は読み切りを `read_to_end` に任せるので、子が残した孫が
    /// 書き手として居ると返らない。ここは吸い出しを別スレッドへ出して予算内で
    /// 待つ形にしたので、「終わったが最後の塊がまだパイプに在る」を落とさないこと。
    /// 量で見る（`.agent/conventions.md`「効果を測る単体テストは実時間で比べない」）。
    /// 大きな出力を作るのが両 OS で同じ書き方にならないので unix 限定
    /// （性質そのものは実装に platform 差が無く、配線は番犬が両 OS で見る）
    #[cfg(unix)]
    #[test]
    fn 終わった子のパイプに残ったぶんも取りこぼさない() {
        const CHUNKS: usize = 200; // 1024 B × 200 = 200 KiB（パイプ容量 64 KiB の 3 倍）
        match run_shell(
            "i=0; while [ $i -lt 200 ]; do printf %01024d 0; i=$((i+1)); done",
            Duration::from_secs(30),
        ) {
            Outcome::Done { stdout, .. } => assert_eq!(
                stdout.len(),
                CHUNKS * 1024,
                "読み切りの予算が無いと、子の終了後にパイプへ残ったぶんが欠ける"
            ),
            other => panic!("Done を期待したが {other:?}"),
        }
    }

    #[test]
    fn 起動できないコマンドはfailedで理由が付く() {
        match output_with_timeout("tako-1503-no-such-binary", &[], Duration::from_secs(5)) {
            Outcome::Failed { label, reason } => {
                assert_eq!(label, "tako-1503-no-such-binary");
                assert!(!reason.is_empty(), "理由が空");
            }
            other => panic!("Failed を期待したが {other:?}"),
        }
    }

    #[test]
    fn 打ち切りはintooutputでnoneになる() {
        let outcome = run_shell(&hang_script(None), Duration::from_millis(300));
        assert!(outcome.timeout_notice().is_some(), "知らせが要る");
        assert!(
            outcome.into_output().is_none(),
            "打ち切りを成功と読ませない"
        );
    }

    #[test]
    fn env上書きは値を変えるだけで上限は外せない() {
        let d = DEFAULT_PROBE_TIMEOUT;
        assert_eq!(parse_timeout_secs(Some("30"), d), Duration::from_secs(30));
        assert_eq!(parse_timeout_secs(Some(" 2 "), d), Duration::from_secs(2));
        // 0 を「無制限」にしない（作ると #1503 が env 1 つで戻る）
        assert_eq!(parse_timeout_secs(Some("0"), d), d);
        assert_eq!(parse_timeout_secs(Some("-1"), d), d);
        assert_eq!(parse_timeout_secs(Some("いつまでも"), d), d);
        assert_eq!(parse_timeout_secs(Some(""), d), d);
        assert_eq!(parse_timeout_secs(None, d), d);
    }

    #[test]
    fn 知らせは書いた形のまま読み戻せる() {
        let notice = TimeoutNotice {
            label: "claude mcp list".to_string(),
            waited_secs: 15,
        };
        let line = notice.to_string();
        assert!(line.contains("確認できません"), "{line}");
        assert!(line.contains("15 秒応答なし"), "{line}");
        // CLI は字下げして出すので、頭の空白は無視して拾える
        let text = format!("前の行\n  {line}\n残り 1 件\n");
        assert_eq!(parse_notices(&text), vec![notice]);
    }

    #[test]
    fn 知らせでない行は拾わない() {
        assert!(parse_notices("[OK] Claude MCP: tako が登録済み\n").is_empty());
        assert!(parse_notices("[確認できません] 秒応答なし）\n").is_empty());
    }

    #[test]
    fn 名札に実行ファイルのパスを含めない() {
        // #927: 名札は応答 JSON にも載るので、実ホームパスを持ち込まない
        let full = format!("{}testuser/.local/bin/claude", std::path::MAIN_SEPARATOR);
        assert_eq!(label(&full, &["mcp", "list"]), "claude mcp list");
        assert_eq!(label("date", &[]), "date");
    }

    #[test]
    fn 既定の上限は有限でprobeよりsetup本体が長い() {
        assert!(DEFAULT_PROBE_TIMEOUT > Duration::ZERO);
        assert!(
            DEFAULT_SETUP_RUN_TIMEOUT > DEFAULT_PROBE_TIMEOUT,
            "導入器が走る setup 本体のほうが長い"
        );
    }

    /// 差し替えた時計で [`poll_with_clock`] を回す（**実時間は使わない**）。
    /// `done_at` 以降の聞き直しで答えが出る。戻りは (結末, 聞いた時刻の列, 眠った長さの列)
    fn fake_poll(
        budget: Duration,
        interval: Duration,
        done_at: Option<Duration>,
    ) -> (Polled<Duration>, Vec<Duration>, Vec<Duration>) {
        use std::cell::{Cell, RefCell};
        let now = Cell::new(Duration::ZERO);
        let asked = RefCell::new(Vec::new());
        let slept = RefCell::new(Vec::new());
        let outcome = poll_with_clock::<_, ()>(
            budget,
            interval,
            || {
                asked.borrow_mut().push(now.get());
                Ok(done_at.filter(|at| now.get() >= *at).map(|_| now.get()))
            },
            || now.get(),
            |d| {
                // 上限の判定を外す回帰は**固まらずに落とす**（無限に聞き直すのを回数で止める）
                assert!(
                    slept.borrow().len() < 10_000,
                    "上限で打ち切らずに聞き直し続けている（#1662 の症状）"
                );
                slept.borrow_mut().push(d);
                now.set(now.get() + d);
            },
        )
        .unwrap();
        (outcome, asked.into_inner(), slept.into_inner())
    }

    #[test]
    fn 聞き直しは上限で打ち切られ上限を踏み越えない() {
        let s = Duration::from_secs;
        let (outcome, asked, slept) = fake_poll(s(5), s(2), None);
        assert_eq!(outcome, Polled::TimedOut { waited: s(5) });
        // 眠りは残り時間で頭打ち（2 + 2 + 1 = 5。2 + 2 + 2 = 6 へ踏み越えない）
        assert_eq!(slept, vec![s(2), s(2), s(1)]);
        // 上限に達した回も打ち切る前に 1 回は聞く
        assert_eq!(asked, vec![s(0), s(2), s(4), s(5)]);
    }

    #[test]
    fn 上限ちょうどに終わったものは取りこぼさない() {
        let s = Duration::from_secs;
        let (outcome, asked, _) = fake_poll(s(5), s(2), Some(s(5)));
        assert_eq!(outcome, Polled::Done(s(5)), "上限ちょうどの答えを捨てた");
        assert_eq!(asked.last(), Some(&s(5)));
        // 上限の手前で終われば、そこで返る（上限まで待たない）
        let (outcome, asked, _) = fake_poll(s(600), s(2), Some(s(3)));
        assert_eq!(outcome, Polled::Done(s(4)));
        assert_eq!(asked.len(), 3);
        // 最初の 1 回で答えが出れば眠らない
        let (outcome, _, slept) = fake_poll(s(600), s(2), Some(s(0)));
        assert_eq!(outcome, Polled::Done(s(0)));
        assert!(slept.is_empty(), "答えが出ているのに眠った");
    }

    #[test]
    fn 聞き直しの失敗はそのまま返り間隔0でも空回りしない() {
        let err: Result<Polled<()>, &str> = poll_with_clock(
            Duration::from_secs(5),
            Duration::from_secs(2),
            || Err("接続できない"),
            || Duration::ZERO,
            |_| panic!("失敗したのに眠った"),
        );
        assert_eq!(err, Err("接続できない"));
        let (_, _, slept) = fake_poll(Duration::from_millis(100), Duration::ZERO, None);
        assert!(
            slept.iter().all(|d| *d > Duration::ZERO),
            "間隔 0 で眠らずに回った: {slept:?}"
        );
    }

    #[test]
    fn waitの上限はenvで変えられ0は既定へ落ちる() {
        let d = DEFAULT_RUN_WAIT_TIMEOUT;
        assert_eq!(parse_timeout_secs(Some("3"), d), Duration::from_secs(3));
        assert_eq!(parse_timeout_secs(Some("0"), d), d, "0 を無制限にしない");
        assert_eq!(parse_timeout_secs(Some("abc"), d), d);
        assert!(d > Duration::ZERO && d < Duration::MAX, "既定は有限");
    }
}
