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
//! - **unix の打ち切りで kill するのは直接の子だけ**（[`group_output_with_timeout`] で起こした子を除く）。
//!   子が残した孫は孤児として残る（`claude mcp list` は MCP サーバを子として起こす）。上限が
//!   無かったときはそもそも永久に待っていたので後退ではないが、孫まで畳むには生成時に
//!   プロセスグループを分ける必要がある。**Windows は #2462 で孫まで畳む**: 待ちの 1 実装が子を
//!   起こした直後に Job Object（境界 B30 の `platform::job::KillScope`）へ入れ、打ち切りで
//!   コンソールのプロセスを止める（GUI のアプリとその子孫は残す・普通に終わったら何も止めない・
//!   器の CLI は入れない）。A/B は `TAKO_2454_LEGACY=1`（Job を作らない = #2462 の前）
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
//!
//! ## UI スレッドから届く待ち（Issue #1979）
//!
//! 2026-10-10 に本番の GUI が 10 分固まった。メインスレッドが worker の状態照会の中で
//! `ps` の `Command::output()` の `poll` に止まっていた。macOS の std はパイプを `pipe()` →
//! `set_cloexec` の 2 手で作るので、別スレッドが同時に起こした長生きの子へ書き手が漏れると、
//! **子が終わっても** EOF が来ない。器への問い合わせ（`tmux::run_tmux` ほか）と
//! `claude agents --json` の走査も、組み立て済みの `Command` を渡せる
//! [`command_output_with_timeout`] でこの待ちを通す。打ち切った子の刈り取りは**同期に待たない**
//! （カーネルの中で止まった子は SIGKILL でも出てくるまで終わらない。`reap_in_background`）。
//! 速いコマンドの待ちが伸びないよう、`try_wait` の間隔は 1ms から倍々で 20ms まで伸ばす。
//! A/B は `issue1979_legacy`（`TAKO_1979_LEGACY=1`）の 1 か所
//!
//! ## ログインシェルに尋ねる待ち（Issue #1532 / #1533）
//!
//! 実行ファイルの検出（`platform::exe::find`）とログインシェルの PATH の読み取り
//! （`platform::exe::login_shell_path`）は `$SHELL -l -c …` を起こす。rc（`.zprofile` など）が
//! 入力を待つ・眠り続けると子が返らないので、上限は [`login_shell_timeout`]（既定
//! [`DEFAULT_LOGIN_SHELL_TIMEOUT`]）。rc が起こした孫（`sleep` / `read` を待つ子）は直接の子を
//! kill しても残るので、ログインシェルは**自分のプロセスグループ**で起こし、打ち切ったら
//! グループごと畳む（[`group_output_with_timeout`]）。A/B は `issue1532_legacy`
//! （`TAKO_1532_LEGACY=1`）の 1 か所
//!
//! ## グループの畳み方は TERM → 猶予 → KILL（Issue #2633）
//!
//! 畳むときにいきなり SIGKILL を送ると、rc の途中の処理は後片付けを走らせられない。実測では
//! `.zprofile` の `pyenv rehash` がロック（`<pyenv root>/shims/.pyenv-shim`）を持つ間に打ち切ると、
//! EXIT の trap が走らずにロックが残り、以後のログインシェルの rehash が残ったロックを待つ
//! （pyenv の待ちは 60 秒・ここの上限は 5 秒なので、tako の問い合わせが自分でロックを片付けることは
//! 無く、残っている間の問い合わせは全部打ち切りになる）。いまはグループへ **SIGTERM を 1 回だけ**送って
//! 呼び手へはその場で返し（上限の契約は変えない）、刈り取り係（[`reap_group_in_background`]）が
//! 猶予（[`GROUP_TERM_GRACE`]）の間グループを見張って、残っていれば SIGKILL で畳む。頭（直接の子）は
//! 判定が済むまで刈り取らないので、グループの番号は別のグループへ使い回されない。
//! A/B は [`issue2633_legacy`]（`TAKO_2633_LEGACY=1` = 猶予を置かずにグループごと SIGKILL）の 1 か所

use std::io::Read;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
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

/// Code Runner の実行環境の問い合わせ（Tier P）1 回あたりの既定の上限（#1730）。
///
/// 根拠（実測 2026-09-27・macOS）: ログインシェル（zsh `-l -i`）の `command -v python3` が
/// 0.34〜0.35 秒、`conda info --envs --json` が 0.34〜0.41 秒、`python3 --version` は
/// 0.01 秒未満。この機械に無い `poetry env info -p` は Python の起動 + poetry の import で
/// 1.5 秒前後が上限の見込みで、5 秒はその約 3 倍。問い合わせは一覧の取得（`RunResolve`。
/// background で走る）と再検出のときだけで、**実行（`Run`）の経路では走らない**
pub const DEFAULT_RUNTIME_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// 実行環境の問い合わせの上限の上書き（秒）。扱いは [`PROBE_TIMEOUT_ENV`] と同じ（**0 は既定へ**）
pub const RUNTIME_PROBE_TIMEOUT_ENV: &str = "TAKO_RUN_PROBE_TIMEOUT_SECS";

/// 環境診断（`tako check-health` / `tako setup --check`）が認証を確かめる問い合わせの既定の上限（#2255）。
///
/// 根拠（実測）: いちばん遅い認証の問い合わせは `agy models`（ネットワーク取得）の 3.0 秒
/// （#1505 の 3 系統そろった Mac）。`claude auth status --json` は 0.12〜0.13 秒（上の表）。
/// 10 秒は `agy models` の約 3 倍で、[`DEFAULT_PROBE_TIMEOUT`] の「いちばん遅い probe の約 3 倍」と
/// 揃える。3 系統は並べて聞くので、全部が返らなくても認証の確認の全体はこれで終わる
pub const DEFAULT_AUTH_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// 診断の認証確認の上限の上書き（秒）。扱いは [`PROBE_TIMEOUT_ENV`] と同じ（**0 は既定へ**）
pub const AUTH_PROBE_TIMEOUT_ENV: &str = "TAKO_AUTH_PROBE_TIMEOUT_SECS";

/// ログインシェルに 1 つ尋ねる（`command -v <名前>` / `printf %s "$PATH"`）1 回あたりの既定の上限
/// （#1532 / #1533）。
///
/// 根拠（実測 2026-10-16・macOS・zsh。`bin_resolve` の冒頭と同じ表）: `zsh -l -c 'command -v git'` が
/// 0.14〜0.16 秒、対話の rc まで読む `-l -i` でも 0.34〜0.35 秒（#1730）。5 秒はその 15 倍弱で、
/// 同じ種類の問い合わせ（git / tmux の場所の解決 = `bin_resolve::DEFAULT_RESOLVE_TIMEOUT`、
/// Code Runner の実行環境の検出 = [`DEFAULT_RUNTIME_PROBE_TIMEOUT`]）と揃える
pub const DEFAULT_LOGIN_SHELL_TIMEOUT: Duration = Duration::from_secs(5);

/// ログインシェルの問い合わせの上限の上書き（秒）。扱いは [`PROBE_TIMEOUT_ENV`] と同じ（**0 は既定へ**）
pub const LOGIN_SHELL_TIMEOUT_ENV: &str = "TAKO_LOGIN_SHELL_TIMEOUT_SECS";

/// 打ち切りを知らせる 1 行の頭。CLI が出し、dispatch が拾う（**文面は 1 実装**）
const NOTICE_HEAD: &str = "[確認できません] ";
const NOTICE_OPEN: char = '（';
const NOTICE_TAIL: &str = " 秒応答なし）";

/// `try_wait` のポーリング間隔の上限。いちばん速い probe（0.12 秒）に対して十分細かい
const POLL: Duration = Duration::from_millis(20);

/// `try_wait` の最初の間隔（#1979）。ここから倍々で [`POLL`] まで伸ばす。
///
/// tmux の問い合わせ（実測 0.01 秒未満）もこの待ちを通るようになったので、最初から
/// 20ms 眠ると「速いコマンドほど待ちのほうが長い」になる（UI スレッドの経路で効く）
const POLL_FIRST: Duration = Duration::from_millis(1);

/// **子が終わったあと**、パイプに残ったぶんを読み切るのに待つ上限。
///
/// 予算の残り全部を使わないのが要点。子が残した孫が書き手として居ると
/// EOF は永遠に来ないので、そこを予算いっぱい待つと「ちゃんと終わった子」の待ちが
/// 上限まで伸びる（実測: 並列 `cargo test --workspace` の負荷下で予算 30 秒の
/// 単体テストが 30 秒掛かった回がある）。パイプ容量は 64 KiB なので 2 秒あれば桁で足りる
const READ_GRACE: Duration = Duration::from_secs(2);

/// 打ち切ったグループへ SIGTERM を送ってから、SIGKILL で畳むまでの猶予（#2633）。
///
/// rc の後片付け（EXIT の trap の `rm` 1 回）に要るのはミリ秒の桁。待つのは呼び手ではなく
/// 刈り取り係なので、ここを伸ばしても呼び手の待ちは伸びない（伸びるのは刈り取り係の寿命だけ）
pub const GROUP_TERM_GRACE: Duration = Duration::from_secs(1);

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

/// 実行環境の問い合わせの上限（env で上書き可。外すことはできない。#1730）
pub fn runtime_probe_timeout() -> Duration {
    parse_timeout_secs(
        std::env::var(RUNTIME_PROBE_TIMEOUT_ENV).ok().as_deref(),
        DEFAULT_RUNTIME_PROBE_TIMEOUT,
    )
}

/// 診断の認証確認の上限（env で上書き可。外すことはできない。#2255）
pub fn auth_probe_timeout() -> Duration {
    parse_timeout_secs(
        std::env::var(AUTH_PROBE_TIMEOUT_ENV).ok().as_deref(),
        DEFAULT_AUTH_PROBE_TIMEOUT,
    )
}

/// ログインシェルの問い合わせの上限（env で上書き可。外すことはできない。#1532 / #1533）
pub fn login_shell_timeout() -> Duration {
    parse_timeout_secs(
        std::env::var(LOGIN_SHELL_TIMEOUT_ENV).ok().as_deref(),
        DEFAULT_LOGIN_SHELL_TIMEOUT,
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

/// #1979 の A/B（`TAKO_1979_LEGACY=1`）。同一バイナリのまま修正前の形へ戻す:
/// 器への問い合わせ（`tmux::run_tmux`）と親子表の採取（`ps`）を上限なしの
/// `Command::output()` で待つ・`tako list` の `list-windows` を UI スレッドで待つ・
/// フォーカスの無いペインの再描画に上限を設けない。前後を同じ構成で並べて測るためのもの
/// （製品の既定は常に新しい形）。**ゲートはこの 1 か所**で、tako-control / tako-app も
/// ここを読む
pub fn issue1979_legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_1979_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// #1984 の A/B（`TAKO_1984_LEGACY=1`）。同一バイナリのまま修正前の形へ戻す:
/// スマホの一覧（`RemoteAgents`）・呼び出し元の pid 祖先辿り（`resolve_caller_pane`）・
/// spawn / handoff の CLI の所在確認・セッション再起動の計画・tmux 一覧（`TmuxList`）を
/// UI スレッドの同期の dispatch で待つ。**ゲートはこの 1 か所**で、tako-control / tako-app も
/// ここを読む
pub fn issue1984_legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_1984_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// #1989 の A/B（`TAKO_1989_LEGACY=1`）。同一バイナリのまま修正前の形へ戻す:
/// コンフリクト解消エージェント（`GitResolveAgent`）・`StaleBinary status`・tmux の書き込み系
/// （kill / resize / select-window / open / cleanup）を UI スレッドの同期の dispatch で待ち、
/// agent CLI の所在確認（`agent_cli::locate`）を上限なしで待つ。**ゲートはこの 1 か所**
pub fn issue1989_legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_1989_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// #1991 の A/B（`TAKO_1991_LEGACY=1`）。同一バイナリのまま修正前の形へ戻す:
/// git の書き込み系（commit / pull / push / stage / unstage / checkout / branch / merge /
/// merge-abort）と conflicts を UI スレッドの同期の dispatch で待ち、起動時の orphan の
/// 一括回収（器の一覧と kill）を UI スレッドで待つ。**ゲートはこの 1 か所**で、
/// tako-control / tako-app もここを読む
pub fn issue1991_legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_1991_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// #2052 の A/B（`TAKO_2052_LEGACY=1`）。同一バイナリのまま修正前の形へ戻す:
/// ユーザータスクの一覧（CLI / MCP / PWA の `tako todo list`）と完了・却下（IPC と右パネルの
/// ボタン）を UI スレッドの同期の dispatch で待ち、そのたびに置き場を読んで配送を畳み込む。
/// コマンドパレット・`tako panel` で tasks ビューを開いても一覧を引き直さない。
/// **ゲートはこの 1 か所**で、tako-control / tako-app もここを読む
pub fn issue2052_legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_2052_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// #2086 の A/B（`TAKO_2086_LEGACY=1`）。同一バイナリのまま修正前の形へ戻す:
/// ユーザータスクの起票・詳細・更新・返答・展開 / 折りたたみ（IPC = CLI / MCP / PWA）を
/// UI スレッドの同期の dispatch で待ち、そのたびに置き場を読み書きする。AI からの展開で
/// 一覧に居ない行を開いたら、UI スレッドで一覧を引き直す。#2052 の一覧・完了・却下とは
/// **別の軸**（片方のアームがもう片方の回帰を隠さない）。**ゲートはこの 1 か所**で、
/// tako-control / tako-app もここを読む
pub fn issue2086_legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_2086_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// #2014 の A/B（`TAKO_2014_LEGACY=1`）。同一バイナリのまま修正前の形へ戻す:
/// プレビューの変更履歴の切替（IPC = CLI / MCP と GUI の「履歴」ボタン）が UI スレッドの同期の
/// dispatch で `git rev-parse` / `git log --follow` を**上限なしで**待つ。**ゲートはこの 1 か所**で、
/// tako-control / tako-app もここを読む
pub fn issue2014_legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_2014_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// #2150 の A/B（`TAKO_2150_LEGACY=1`）。同一バイナリのまま修正前の形へ戻す:
/// プレビューの変更履歴の行の diff の展開（IPC = CLI / MCP の `--expand` と GUI のコミット行の
/// クリック）が UI スレッドの同期の dispatch で `git diff` / `git diff-tree` を**上限なしで**待つ。
/// **ゲートはこの 1 か所**で、tako-control / tako-app もここを読む
pub fn issue2150_legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_2150_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// #2151 の A/B（`TAKO_2151_LEGACY=1`）。同一バイナリのまま修正前の形へ戻す:
/// git / tmux の場所の解決（`bin_resolve::resolve_bin`）が `--version` とログインシェルの
/// `command -v` を**上限なしで**待ち、打ち切りの知らせも出さない。**ゲートはこの 1 か所**
pub fn issue2151_legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_2151_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// #2182 の A/B（`TAKO_2182_LEGACY=1`）。同一バイナリのまま修正前の形へ戻す:
/// タスクへの返答の配送（IPC = CLI / MCP / PWA と右パネルの「返す」）で起票 master が居ないとき、
/// 起動の組み立て（`master_launch::plan` = CLI の所在確認のログインシェルと tako の実行ファイルの
/// 解決）を UI スレッドで待つ。**ゲートはこの 1 か所**で、tako-control / tako-app もここを読む
pub fn issue2182_legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_2182_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// #2255 の A/B（`TAKO_2255_LEGACY=1`）。同一バイナリのまま修正前の形へ戻す:
/// 環境診断（`tako check-health` / `tako setup --check`）の認証確認（`claude auth status --json` /
/// `codex login status` / `agy models`）を**上限なしで 1 系統ずつ**待つ。返らない CLI が 1 つあると
/// check-health が返らない（GUI 経由は CLI の応答の上限 300 秒の後、CLI 自身の診断が同じ問い合わせで
/// また止まる）。**ゲートはこの 1 か所**
pub fn issue2255_legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_2255_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// #1532 / #1533 の A/B（`TAKO_1532_LEGACY=1`）。同一バイナリのまま修正前の形へ戻す:
/// ログインシェルに尋ねる問い合わせ（実行ファイルの検出 `platform::exe::find` の unix 実装・
/// setup / 環境診断のログインシェルの PATH の読み取り）と check-health の `tako --version` を
/// **上限なしで**待ち、打ち切りの知らせも「確認できない」の区別も出さない。**ゲートはこの 1 か所**で、
/// tako-control / tako-app もここを読む
pub fn issue1532_legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_1532_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// #2633 の A/B（`TAKO_2633_LEGACY=1`）。打ち切ったグループ（[`group_output_with_timeout`] の子）を
/// 修正前の形 = 猶予を置かずにグループごと SIGKILL で畳む（rc の EXIT の trap は走らない）。
/// **ゲートはこの 1 か所**
pub fn issue2633_legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_2633_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
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
    output_with_timeout_in(program, args, None, budget)
}

/// [`output_with_timeout`] の作業ディレクトリを指定する版（#1730）。
///
/// 答えがカレントディレクトリで決まる問い合わせ（`poetry env info -p` /
/// `pipenv --venv` = そのプロジェクトの環境の置き場）のためにある。待ちの扱いは同じ
pub fn output_with_timeout_in(
    program: &str,
    args: &[&str],
    cwd: Option<&std::path::Path>,
    budget: Duration,
) -> Outcome {
    let name = label(program, args);
    let mut cmd = Command::new(program);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
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

/// **組み立て済みの** `Command` を上限つきで待つ（#1979）。
///
/// 環境の手当てを雛形が持つ子（`tmux::tmux_command` の UTF-8 ロケール注入等）のための口。
/// `stdin` の遮断と stdout / stderr のパイプ化はここで行う。コンソールウィンドウの抑止
/// （#586）は**雛形の側**が済ませている前提（`tmux_command` は済ませている）。
///
/// `Command::output()` を UI スレッドで呼ぶと、子（`ps` / `tmux`）が終わらないとき・
/// パイプの書き手が別の長生きのプロセスへ漏れたときに**永久に返らない**
/// （2026-10-10 の本番ハング: メインスレッドが `ps` の `output()` の `poll` で 10 分）。
/// 名札（`label`）にパスを含めないのは [`label`] と同じ（#927）
pub fn command_output_with_timeout(
    command: &mut Command,
    label: &str,
    budget: Duration,
) -> Outcome {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match command.spawn() {
        Ok(child) => wait_with_timeout(child, label, budget),
        Err(e) => Outcome::Failed {
            label: label.to_string(),
            reason: e.to_string(),
        },
    }
}

/// **組み立て済みの** `Command` を**自分のプロセスグループで**起こし、上限つきで待つ（#1532 / #1533）。
///
/// ログインシェル（`$SHELL -l -c …`）のための口。rc が起こした孫（`sleep` / 入力を待つ `read` /
/// パスフレーズを聞く `ssh-add`）は直接の子を kill しても残り、パイプの書き手として居座る。
/// 打ち切ったら**グループへ** SIGTERM を 1 回送り、猶予（[`GROUP_TERM_GRACE`]）の後も残るものは
/// SIGKILL で畳むので、孫も残らない。rc の途中の処理の後片付け（EXIT の trap）は走れる（#2633）。
///
/// グループを分けるので、rc が端末（`/dev/tty`）を読もうとするとバックグラウンドの読み取りとして
/// 止められる（SIGTTIN）。利用者の打鍵を横取りせず、止まったまま上限で打ち切られ、猶予の後に畳まれる。
/// 普通に終わったときは何も畳まない（rc が意図して残す常駐 = `ssh-agent` 等は従来どおり残る）。
/// Windows はグループを分けない（ログインシェルを起こさない）= [`command_output_with_timeout`] と同じ
pub fn group_output_with_timeout(command: &mut Command, label: &str, budget: Duration) -> Outcome {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match command.spawn() {
        // グループごと畳むのは待ちの 1 実装の打ち切りの kill（`kill_on_timeout`）が、
        // 子がグループを率いていることを見て決める
        Ok(child) => wait_with_timeout(child, label, budget),
        Err(e) => Outcome::Failed {
            label: label.to_string(),
            reason: e.to_string(),
        },
    }
}

/// 打ち切りの kill。子が**自分のプロセスグループを率いている**（[`group_output_with_timeout`] で
/// 起こした）ならグループへ **SIGTERM を 1 回だけ**送って `true` を返す（#2633）。直接の子にも
/// SIGKILL は送らない = rc の途中の処理（`pyenv rehash` のロック）が EXIT の trap で後片付けを終えられる。
/// 残ったもの（TERM を無視する・止まっている）は呼び手が [`reap_group_in_background`] へ渡し、
/// [`GROUP_TERM_GRACE`] の後に SIGKILL で畳む（rc が起こした孫も残さない = #1532）。
/// A/B（[`issue2633_legacy`]）は修正前の形 = 猶予を置かずにグループごと SIGKILL。
/// それ以外の子（親のグループに居る）は直接の子だけを kill して `false`（孫は残る = モジュール冒頭の
/// 「分かっている限界」）。Windows の孫は呼び手（[`wait_with_timeout`]）が先に入れ物（`kill_scope`）ごと
/// 止める（#2462）。
///
/// グループへ送るのは子がまだ刈り取られていないとき（`try_wait` が `None` を返した直後）だけなので、
/// 番号が別のグループへ使い回されていることは無い
fn kill_on_timeout(child: &mut Child) -> bool {
    #[cfg(unix)]
    if let Ok(pid) = libc::pid_t::try_from(child.id()) {
        // SAFETY: プロセスの属性を読むだけでメモリには触れない
        let leads_group = unsafe { libc::getpgid(pid) == pid };
        if leads_group {
            if issue2633_legacy() {
                // SAFETY: 負の pid（= グループ宛て）へシグナルを送るだけでメモリには触れない
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            } else {
                // SAFETY: 同上
                unsafe {
                    libc::kill(-pid, libc::SIGTERM);
                }
                return true;
            }
        }
    }
    let _ = child.kill();
    false
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
    // #2462: 打ち切りの kill を孫まで届かせる入れ物（Windows だけ）。普通に終わったら drop が
    // 何も止めずに手放す（返り方はどれでもよい）。呼び手が起こした直後にここへ来る前提
    let mut descendants = kill_scope(&mut child);
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let start = Instant::now();
    let mut nap = POLL_FIRST;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // 子は終わったが、パイプの書き手（子が残した孫）がまだ居ることがある。
                // `Command::output()` はここで無限に待つので、読み切りにも予算を掛ける。
                // 予算の残り全部ではなく [`READ_GRACE`] で頭打ちにする
                let grace = Instant::now();
                let mut nap = POLL_FIRST;
                while !(out.finished() && err.finished())
                    && grace.elapsed() < READ_GRACE
                    && start.elapsed() < budget
                {
                    std::thread::sleep(nap);
                    nap = (nap * 2).min(POLL);
                }
                return Outcome::Done {
                    status,
                    stdout: out.snapshot(),
                    stderr: err.snapshot(),
                };
            }
            Ok(None) => {
                if start.elapsed() >= budget {
                    // #2462: 子孫は入れ物ごと（コンソールは止め GUI のアプリは残す）。直下の子は次の kill
                    descendants.stop();
                    let group = kill_on_timeout(&mut child);
                    // 刈り取り（`wait`）は**ここで待たない**（#1979）。カーネルの中で止まった
                    // 子は SIGKILL を受けても出てくるまで終わらず、ここで `wait` すると
                    // 上限そのものが意味を失う。刈り取り係へ渡して呼び手には今すぐ返す。
                    // TERM を送ったグループの猶予と SIGKILL も刈り取り係が持つ（#2633）
                    if group {
                        reap_group_in_background(child);
                    } else {
                        reap_in_background(child);
                    }
                    return Outcome::TimedOut {
                        label: label.to_string(),
                        waited: budget,
                        stdout: out.snapshot(),
                        stderr: err.snapshot(),
                    };
                }
                std::thread::sleep(nap);
                nap = (nap * 2).min(POLL);
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

/// 待つ子を子孫ごと入れ物へ入れる（#2462。Windows だけ。境界 B30 の [`KillScope`]）。
///
/// Windows の `TerminateProcess` は子孫を道連れにしないので、`.cmd` の CLI（`claude.cmd` → cmd.exe →
/// node.exe）・`claude mcp list` が起こす MCP サーバ・テストの `cmd /C "… ping"` の孫が打ち切りの
/// 後も残っていた。入れないのは 2 つ:
///
/// - **器の CLI**（tmux / psmux。実行ファイル名を [`crate::backend::is_plumbing_process`] で見る）:
///   器のサーバーはクライアントの子として起きるので、1 回の問い合わせ（`new-session` など）の
///   打ち切りで器ごと消える（`platform::job` の「器は入れない」と同じ理由）。名前が引けない子も
///   器の側へ倒す（入れ損ねても従来の止め方に戻るだけ）
/// - **既に終わった子**（入れる前に終わった = 孫を起こす暇が無い。割り当ての失敗の警告を出さない）
///
/// [`KillScope`]: crate::platform::job::KillScope
fn kill_scope(child: &mut Child) -> crate::platform::job::KillScope {
    use crate::platform::job::KillScope;
    if !crate::platform::job::SUPPORTED || matches!(child.try_wait(), Ok(Some(_))) {
        return KillScope::empty();
    }
    let pid = child.id();
    let container = crate::platform::procinfo::image_path(pid).is_none_or(|path| {
        path.file_name()
            .is_none_or(|name| crate::backend::is_plumbing_process(&name.to_string_lossy()))
    });
    if container {
        return KillScope::empty();
    }
    KillScope::contain(pid)
}

/// 打ち切った子の刈り取りを別スレッドへ渡す（#1979）。
///
/// kill した子は普通はすぐ終わるが、カーネルの中で止まっている子は出てくるまで
/// 終わらない。呼び手（UI スレッドのこともある）をそこで待たせないため、`wait` は
/// 刈り取り係が持つ。刈り取らないとゾンビが残るので、手放しはしない
fn reap_in_background(mut child: Child) {
    let spawned = std::thread::Builder::new()
        .name("tako-probe-reap".into())
        .spawn(move || {
            let _ = child.wait();
        });
    // スレッドを作れない（資源切れ）ときは刈り取りを諦める。ここで同期に待つと
    // 上限の意味が消えるので、ゾンビ 1 つを残すほうを選ぶ
    drop(spawned);
}

/// 打ち切りで SIGTERM を送ったグループ（[`kill_on_timeout`] が `true` を返した子）を刈り取る（#2633）。
///
/// 呼び手へはもう返してある（ここは別スレッド = 上限の契約にも #1979 の「刈り取りを待たない」にも
/// 触れない）。グループに生きたプロセスが居なくなるか [`GROUP_TERM_GRACE`] を過ぎるまで見張り、
/// 過ぎてもまだ居れば SIGKILL で畳んでから頭（直接の子）を刈り取る。TERM を無視する・止まった
/// （SIGTTIN）メンバーもここで消える。
///
/// **頭は判定が済むまで刈り取らない**: 刈り取ると頭の番号（= グループの番号）が空き、後から生まれた
/// 別のグループへ SIGKILL を送りうる。生き死には `kill(-pgid, 0)` で見る（生きたメンバーが居る間だけ
/// 0。macOS は刈り取り前の頭だけが残ると EPERM、刈り取った後は ESRCH を返す = どちらも「居ない」）
#[cfg(unix)]
fn reap_group_in_background(mut child: Child) {
    let pgid = libc::pid_t::try_from(child.id()).ok();
    let spawned = std::thread::Builder::new()
        .name("tako-probe-reap".into())
        .spawn(move || {
            if let Some(pgid) = pgid {
                let start = Instant::now();
                // SAFETY: グループ宛てに 0 番（居るかどうかの確かめ）を送るだけでメモリには触れない
                while unsafe { libc::kill(-pgid, 0) } == 0 {
                    if start.elapsed() >= GROUP_TERM_GRACE {
                        // SAFETY: 頭はまだ刈り取っていない = グループの番号は使い回されていない
                        unsafe {
                            libc::kill(-pgid, libc::SIGKILL);
                        }
                        break;
                    }
                    std::thread::sleep(POLL);
                }
            }
            let _ = child.wait();
        });
    // スレッドを作れない（資源切れ）ときは猶予を諦めてその場で畳む。子は刈り取られずにゾンビとして
    // 残る（= 番号は使い回されない。[`reap_in_background`] と同じくゾンビ 1 つを残すほうを選ぶ）
    if let (Err(_), Some(pgid)) = (spawned, pgid) {
        // SAFETY: 負の pid（= グループ宛て）へシグナルを送るだけでメモリには触れない
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
}

/// unix 以外はグループを分けない（[`kill_on_timeout`] が `true` を返さない）ので、ここへは来ない
#[cfg(not(unix))]
fn reap_group_in_background(child: Child) {
    reap_in_background(child);
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
    /// Windows の定番 sleep は `ping -n`（`timeout` は stdin を要求する）。
    ///
    /// 固まる本体は `exec` で子そのものと置き換える（#2448）。打ち切りの kill は直接の子にしか
    /// 届かないので、macOS の `/bin/sh`（bash 3.2）が並びの最後の `sleep` を子として起こすと、
    /// kill は sh に当たって `sleep 600` が親を失って 10 分残る（テストは緑のまま）。番犬は
    /// `test_residue` の一巡。Windows の `cmd /C` には `exec` が無く `ping` は孫のままだが、
    /// 打ち切りが入れ物（Job）ごと止める（#2462。実プロセスの検査は `tests/issue2462_probe_job_windows.rs`）
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
                Some(p) => format!("printf %s {p}; exec sleep 600"),
                None => "exec sleep 600".to_string(),
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

    /// #1979: 子は終わったのに**孫がパイプの書き手を握り続ける**（= 別スレッドの長生きの子へ
    /// 書き手が漏れた形の再現）でも、読み切りの猶予で返り、子が出したぶんは全部持ち帰る。
    /// `Command::output()` はこの形で孫が終わるまで返らない（2026-10-10 の本番ハングの型）。
    ///
    /// 「返った」は実時間ではなく**仕組み**で見る: 返った時点で孫がまだ生きている
    /// = パイプの EOF を待たずに返った（孫は 30 秒握るので、負荷で遅れても反転しない）
    #[cfg(unix)]
    #[test]
    fn 孫がパイプを握り続けても読み切りの猶予で返る() {
        let dir = std::env::temp_dir().join(format!(
            "tako-1979-probe-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("一時 dir");
        let pid_file = dir.join("holder.pid");
        let script = format!(
            "printf %s table; sleep 30 & echo $! > '{}'",
            pid_file.display()
        );
        let outcome = run_shell(&script, Duration::from_secs(60));
        let holder: Option<i32> = std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|s| s.trim().parse().ok());
        // SAFETY: シグナル 0 は送らずに生存だけを見る
        let holder_alive = holder.is_some_and(|pid| unsafe { libc::kill(pid, 0) } == 0);
        if let Some(pid) = holder {
            // SAFETY: 自分が起こした sleep だけを落とす
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        match outcome {
            Outcome::Done { status, stdout, .. } => {
                assert!(status.success());
                assert_eq!(
                    String::from_utf8_lossy(&stdout),
                    "table",
                    "子の出力を取りこぼさない"
                );
            }
            other => panic!("Done を期待したが {other:?}"),
        }
        assert!(
            holder.is_some(),
            "孫の pid を書けていない（注入が効いていない）"
        );
        assert!(
            holder_alive,
            "孫がパイプを手放す（= 終わる）まで待ってから返った = `output()` と同じ待ち方"
        );
    }

    /// #1979: 組み立て済みの `Command`（tmux の雛形）も同じ待ちを通り、上限で打ち切られる
    #[cfg(unix)]
    #[test]
    fn 組み立て済みのcommandも上限で打ち切られる() {
        let mut command = Command::new("/bin/sh");
        // `exec`: 打ち切りの kill を眠る本人に当てる（sh の 1 文の最適化に頼らない。#2448）
        crate::platform::process::no_console_window(&mut command).args(["-c", "exec sleep 600"]);
        let outcome =
            command_output_with_timeout(&mut command, "sh sleep", Duration::from_millis(300));
        assert!(
            matches!(outcome, Outcome::TimedOut { .. }),
            "打ち切りになる: {outcome:?}"
        );
        let mut ok = Command::new("/bin/sh");
        crate::platform::process::no_console_window(&mut ok).args(["-c", "printf %s ok"]);
        match command_output_with_timeout(&mut ok, "sh printf", Duration::from_secs(30)) {
            Outcome::Done { stdout, .. } => assert_eq!(stdout, b"ok"),
            other => panic!("Done を期待したが {other:?}"),
        }
    }

    /// #2462 の境界の端: 待つ前に終わっていた子を渡しても、入れ物へ入れようとせず
    /// （入れられない子で警告を出さない）終了コードどおりの Done で返る
    #[test]
    fn 待つ前に終わった子もdoneで返る() {
        // `cmd /C` でも `sh -c` でも同じ綴り
        let (program, args) = shell("exit 3");
        let mut command = Command::new(program);
        crate::platform::process::no_console_window(&mut command)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("すぐ終わる子を起こせる");
        // 終わるまで待ってから渡す（`try_wait` は終了コードを覚えているので待ちの 1 実装も同じ値を読む）
        let status = child.wait().expect("看取れる");
        assert_eq!(status.code(), Some(3));
        match wait_with_timeout(child, "exit 3", Duration::from_secs(30)) {
            Outcome::Done { status, .. } => assert_eq!(status.code(), Some(3)),
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
