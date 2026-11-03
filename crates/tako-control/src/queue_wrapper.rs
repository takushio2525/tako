//! `tako queue <コマンド>` の本体（wrapper。FR-2.43.6 / Issue #1994。エピック #1992 の S2）
//!
//! 設計の正は `.agent/plans/2026-10-resource-manager.md`（§4.1・§4.2）。
//!
//! ```text
//! ① 受付（poll = 1 件の全体を送る upsert）→ 順位と待つ理由
//! ② 1 秒ごとの poll（心拍を兼ねる）→ waiting / granted{取り分}
//! ③ 子を起こす（env: TAKO_QUEUE_JOB=<受付番号>、CARGO_BUILD_JOBS / RUST_TEST_THREADS が
//!    未設定なら取り分）→ started（= 子の pid を載せた poll）。子は ① の前に PATH で引いておいた
//!    絶対パスで起こす（install をまたいだ待ちで PATH を空振りしない。argv[0] は打った形のまま。#2538）
//! ④ 2 秒ごとの心拍（取り消しの印を受け取る）
//! ⑤ done（終了コード）。自分の終了コードは子と同じ
//! ```
//!
//! - **入れ子は待たない**: `TAKO_QUEUE_JOB` がある中の `tako queue` はその枠の中で即実行
//!   （`tako queue bash scripts/test-x.sh` の中の `tako queue cargo build` が自分を待って固まらない）
//! - **tako が居なければ待たずに走る**（fail-open。1 行だけ知らせる。終了コードは変えない）。
//!   調停役でない tako（隔離 / 旧版で順番待ちを知らない）に当たったときも同じ
//! - 待ちの途中で調停役へ繋がらなくなったら 1 → 5 秒で繋ぎ直し、[`LOST_GRACE`] を過ぎたら走る。
//!   繋ぎ直した先へは元の受付時刻で登録し直る（正本はこちら）
//! - 繋がったまま応答しない調停役（GUI が固まった。#2123）は 1 往復の上限
//!   （`protocol::QUEUE_REPLY_TIMEOUT` = 30 秒）で切れて「届かない」と同じに扱う: 受付なら待たずに走り、
//!   待ちの途中なら [`LOST_GRACE`]（失敗した往復を始めた時刻から数える）を過ぎたら走る。
//!   待ちそのものは 1 秒ごとの短い往復の連続なので、応答が届いている限り長い待ちは切れない。
//!   届かなくても構わない心拍（`started`）と `done` は `protocol::QUEUE_NOTIFY_REPLY_TIMEOUT`（5 秒）で
//!   切る（子の終わり・シグナル・自分の終了を固まった GUI に待たせない）
//! - 実行中に調停役が居なくなっても子は止めない（繋がったら子の pid で登録し直す）
//! - 待ちの上限（既定 30 分・`--max-wait`）で 75、待ちの取り消しで 125、Ctrl+C / 親の SIGTERM で
//!   128 + 番号（待ちから外れて終わる）。実行中のシグナルは子へ転送する。待ちの上限で諦めたときは
//!   待っていた理由と、枠とメモリを押さえていた処理（[`gave_up_lines_in`]）を出す（#2254）
//! - 受付で過去の記録の鍵（`HistoryKeys`。repo の根 + 正規化した argv のハッシュ）を送る。
//!   調停役はそれで見込みメモリを引く（#2254）
//! - 実行中の取り消しは子とその子孫へ SIGTERM → 10 秒で SIGKILL（Windows は子を起こした直後に入れた
//!   Job Object ごと止める = コンソールのプロセスは止め GUI のアプリは残す。#2462。境界 B30）
//! - 受付で優先度の手がかり（#1996。`PriorityHint` = 付けた段・stdin が端末か・env の役割）を送り、
//!   応答の `job_priority`（受付番号での変更）を覚えて次の poll から送る（調停役を立て直しても戻らない）
//! - **cargo の進み**（#1996）: 自分の stderr が端末でなく、子が cargo の build / test 系で、利用者が
//!   `CARGO_TERM_PROGRESS_WHEN` を決めていないときだけ、`CARGO_TERM_PROGRESS_WHEN=always`（と幅 80）を足して
//!   子の stderr を受け、`Building [...] N/M` の行を外して（`eta::ProgressFilter`）残りをそのまま自分の
//!   stderr へ流す。読んだ `N/M` は心拍と done に載せる（終わりの見込み・過去の記録のユニット数）。
//!   端末のときは横取りしない（cargo 自身のバーを人がそのまま見る）。A/B は wrapper のプロセスの env
//!   `TAKO_1996_LEGACY=1`（stderr を受けず進みの env も足さない = 従来の素通し）
//!
//! 送受信（IPC の 1 往復）は呼び手（tako-cli の transport）が閉包で渡す。ここは状態の進め方と
//! OS 依存（シグナル・子の木）だけを持つ。

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

use serde_json::Value;
use tako_core::i18n::Lang;
use tako_core::queue::{self as q, CallerRole, JobKind, JobWire, Priority, PriorityHint};

use crate::protocol::Request;
use crate::queue::{unix_ms, ENV_JOB};
use crate::ReplyNote;

/// #1996 の A/B: `TAKO_1996_LEGACY=1` で cargo の stderr を受けず `CARGO_TERM_PROGRESS_WHEN` も足さない
/// （従来の素通し = 進みは読まない）。wrapper（`tako queue <コマンド>`）のプロセスの env で効く
pub const ENV_LEGACY_1996: &str = "TAKO_1996_LEGACY";

fn legacy_1996() -> bool {
    std::env::var_os(ENV_LEGACY_1996).is_some_and(|v| v == "1")
}

/// 待ちの間の poll の間隔
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// 実行中の心拍の間隔
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);
/// 待ちの途中で調停役を見失ってから、待たずに走るまでの猶予
pub const LOST_GRACE: Duration = Duration::from_secs(60);
/// 非 TTY のときの待ちの表示の間隔（AI の背景実行の出力で読める頻度）
pub const PLAIN_REPORT_EVERY: Duration = Duration::from_secs(60);
/// 取り消しで SIGTERM を送ってから SIGKILL までの猶予
pub const KILL_GRACE: Duration = Duration::from_secs(10);
/// 子の終わりを見る間隔
const CHILD_TICK: Duration = Duration::from_millis(100);

/// 1 往復の失敗
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendError {
    /// 調停役（tako）へ届かない
    Unreachable(String),
    /// 繋がったが上限まで応答が来ない（調停役の GUI が固まっている。#2123）。値は待った上限
    Timeout(Duration),
    /// 届いたが断られた（調停役でない tako・旧版で順番待ちを知らない・引数の誤り）
    Rejected(String),
}

/// wrapper の指定（普段は何も付けない = #322）
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// 処理に付ける段（`--priority` / env `TAKO_QUEUE_PRIORITY`）。`None` = 既定（ペイン > 役割。#1996）
    pub priority: Option<Priority>,
    /// 待ちの上限（未指定なら調停役が返す設定値。既定 30 分）
    pub max_wait: Option<Duration>,
    /// 表示名（未指定ならコマンド行を短くしたもの）
    pub label: Option<String>,
    /// 進みの行（stderr の待ち・開始・諦め・素通しの理由）の表示言語（#2365。CLI は `cli_lang()`）
    pub lang: Lang,
}

/// `--max-wait` の値を読む: `90s` / `10m` / `1h` / 単位なしは分。断りは表示言語で引ける型（#2365。呼び手は CLI）
pub fn parse_duration(s: &str) -> Result<Duration, crate::Reason> {
    let s = s.trim();
    let (num, unit) = match s.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((i, _)) => (&s[..i], &s[i..]),
        None => (s, "m"),
    };
    let n: u64 = num
        .parse()
        .map_err(|_| crate::Reason::QueueDurationUnreadable(s.to_string()))?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        other => return Err(crate::Reason::QueueDurationUnitUnknown(other.to_string())),
    };
    if secs == 0 {
        return Err(crate::Reason::QueueWaitTooShort);
    }
    Ok(Duration::from_secs(secs))
}

/// 終了の状態 → 終了コード（シグナルで終わったら 128 + 番号 = シェルと同じ）
pub fn exit_code_of(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    1
}

/// 繋ぎ直しの間隔（1 → 5 秒）
pub fn reconnect_delay(attempt: u32) -> Duration {
    Duration::from_secs(u64::from((1 + attempt / 2).min(5)))
}

/// 待ちの 1 行（純関数）。文は表示言語で組む（#2365。待つ理由・開始の見込みは日本語なら調停役の文、
/// 英語なら同じ応答の構造の欄 `reason` / `head_reason` / `start_s` から）
pub fn waiting_line(resp: &Value, waited: Duration, lang: Lang) -> String {
    let n = |k: &str| resp[k].as_u64().unwrap_or(0);
    let position = n("position");
    // 開始の見込み（#1996。前に並ぶ全件と実行中の全件に見込みがあるときだけ調停役が返す）
    let start = resp["start_text"].as_str().map(|ja| {
        ReplyNote::QueueStartEta {
            ja: ja.to_string(),
            start_s: n("start_s"),
        }
        .text_in(lang)
    });
    let reason = |code: &str, text: &str| {
        resp[text].as_str().map(|ja| {
            ReplyNote::QueueWaitReason {
                code: resp[code].as_str().map(str::to_string),
                ja: ja.to_string(),
            }
            .text_in(lang)
        })
    };
    // 前の処理の後ろで待っている（並びが止まっている）ときだけ「前に N 件」。target dir 待ちの先頭を
    // 追い越せる件（#2243）は 2 番目以降でも自分の理由（枠・メモリ…）で待っているので、それを出す。
    // 旧い調停役は 2 番目以降を常に ahead で返す
    let behind = resp["reason"].as_str().is_none_or(|r| r == "ahead");
    let why = if position > 1 && behind {
        ReplyNote::QueueAheadOf {
            ahead: position - 1,
            head: reason("head_reason", "head_text"),
        }
        .text_in(lang)
    } else {
        reason("reason", "text").unwrap_or_default()
    };
    let line = ReplyNote::QueueWaitLine(Box::new(crate::reply_note::QueueWaitLineFields {
        position,
        waiting: n("waiting"),
        running: n("running"),
        max_jobs: n("max_jobs"),
        unmanaged: n("unmanaged"),
        waited_s: waited.as_secs(),
        start,
        why,
    }));
    format!("tako queue: {}", line.text_in(lang))
}

/// MiB → 人が読む GB / MB
pub fn human_mb(mb: u64) -> String {
    if mb >= 1024 {
        format!("{:.1} GB", mb as f64 / 1024.0)
    } else {
        format!("{mb} MB")
    }
}

/// 待ちの上限で諦めたときの説明（純関数。#2254）: 待っていた理由と、枠とメモリを押さえていた処理
/// （最後の poll の `holders`。旧い調停役は返さない = 理由だけ）。表示言語で組む（#2365。`tako queue <コマンド>`
/// が諦めたときの行と、`tako queue` の状況の「直近の諦め」の下の行）。待つ理由は日本語なら調停役の 1 文、
/// 英語なら理由の種類（`reason` / `head_reason`）から組む
pub fn gave_up_lines_in(resp: &Value, lang: Lang) -> Vec<String> {
    let mut out = Vec::new();
    let behind = resp["reason"].as_str().is_none_or(|r| r == "ahead");
    let why = if behind {
        resp["head_text"].as_str().map(|t| {
            let head = ReplyNote::QueueWaitReason {
                code: resp["head_reason"].as_str().map(str::to_string),
                ja: t.to_string(),
            };
            ReplyNote::QueueBehindHead(head.text_in(lang)).text_in(lang)
        })
    } else {
        resp["text"].as_str().map(|t| {
            ReplyNote::QueueWaitReason {
                code: resp["reason"].as_str().map(str::to_string),
                ja: t.to_string(),
            }
            .text_in(lang)
        })
    };
    if let Some(why) = why {
        out.push(ReplyNote::QueueGaveUpWhy(why).text_in(lang));
    }
    for h in resp["holders"].as_array().into_iter().flatten() {
        let holder = ReplyNote::QueueHolder(Box::new(crate::reply_note::QueueHolderFields {
            id: h["id"].to_string(),
            label: h["label"].as_str().unwrap_or("?").to_string(),
            used: h["memory_mb"].as_u64().map(human_mb),
            peak: h["peak_mb"].as_u64().map(human_mb),
            pending: h["pending_mb"].as_u64().filter(|p| *p > 0).map(human_mb),
            estimate: h["estimate_mb"].as_u64().map(human_mb),
            estimate_from: h["estimate_from"].as_str().map(str::to_string),
            peak_passed: h["peak_passed"] == true,
            stalled: h["stalled"] == true,
        }));
        out.push(holder.text_in(lang));
    }
    out
}

/// 秒 → 「1 分 12 秒」/ `1m 12s`（#2365。英語は右パネルの resources ビューと同じ区切り）
pub fn human_secs_in(secs: u64, lang: Lang) -> String {
    match lang {
        Lang::Ja => {
            if secs >= 3600 {
                format!("{} 時間 {} 分", secs / 3600, (secs % 3600) / 60)
            } else if secs >= 60 {
                format!("{} 分 {} 秒", secs / 60, secs % 60)
            } else {
                format!("{secs} 秒")
            }
        }
        Lang::En => {
            if secs >= 3600 {
                format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
            } else if secs >= 60 {
                format!("{}m {}s", secs / 60, secs % 60)
            } else {
                format!("{secs}s")
            }
        }
    }
}

/// 走らせるコマンドから 1 件を組む（受付番号 = 自分の pid）
pub fn build_wire(argv: &[String], opts: &Options, cwd: &Path) -> JobWire {
    let kind = q::classify(argv);
    // `--target-dir <dir>` / `--target-dir=<dir>` は env の CARGO_TARGET_DIR より強い（cargo と同じ）
    let flag_target = argv
        .windows(2)
        .find(|w| w[0] == "--target-dir")
        .map(|w| w[1].clone())
        .or_else(|| {
            argv.iter()
                .find_map(|a| a.strip_prefix("--target-dir=").map(str::to_string))
        });
    let env_target = flag_target.or_else(|| std::env::var("CARGO_TARGET_DIR").ok());
    let target_dir = matches!(kind, JobKind::Build | JobKind::Test)
        .then(|| {
            q::cargo_target_dir(cwd, env_target.as_deref(), |dir| {
                std::fs::read_to_string(dir.join("Cargo.toml"))
                    .ok()
                    .map(|t| t.contains("[workspace]"))
            })
        })
        .flatten()
        .map(|p| {
            tako_core::platform::path::canonicalize_or_self(&p)
                .to_string_lossy()
                .into_owned()
        });
    // 過去の記録の鍵（#2254）: worktree は本体の根へ寄せる（Issue ごとの worktree で記録を引き継ぐ）
    let repo = q::repo_identity(cwd, |dir| {
        let git = dir.join(".git");
        let meta = std::fs::metadata(&git).ok()?;
        if meta.is_dir() {
            Some(q::GitEntry::Dir)
        } else {
            std::fs::read_to_string(&git).ok().map(q::GitEntry::File)
        }
    });
    JobWire {
        history: q::history_keys(argv, &repo.to_string_lossy()),
        id: u64::from(std::process::id()),
        enqueued_at_ms: unix_ms(),
        label: opts
            .label
            .clone()
            .unwrap_or_else(|| tako_core::resources::short_label(&argv.join(" "), 80)),
        kind,
        // 旧い調停役（手がかりを読まない = S2）はこれで並べる
        priority: opts.priority.unwrap_or_default(),
        target_dir,
        pane: std::env::var("TAKO_PANE_ID")
            .ok()
            .and_then(|v| v.trim().parse().ok()),
        child_pid: None,
        started_at_ms: None,
        exit_code: None,
        hint: Some(PriorityHint {
            set: opts.priority,
            tty: std::io::stdin().is_terminal(),
            role: std::env::var("TAKO_ORCHESTRATOR_ROLE")
                .ok()
                .as_deref()
                .and_then(CallerRole::from_env_role),
        }),
        progress: None,
    }
}

/// 応答の `job_priority`（受付番号での変更。#1996）を覚える。旧い調停役は欄を返さない = そのまま
pub fn adopt_job_priority(wire: &mut JobWire, resp: &Value) {
    let Some(v) = resp.get("job_priority") else {
        return;
    };
    let set = v.as_str().and_then(Priority::parse);
    if let Some(hint) = wire.hint.as_mut() {
        hint.set = set;
    }
}

/// cargo の進みを読むか（#1996。純関数）: 自分の stderr が端末でない・子が cargo の build / test 系・
/// 利用者が `CARGO_TERM_PROGRESS_WHEN` を決めていない
pub fn reads_cargo_progress(
    argv: &[String],
    stderr_tty: bool,
    already_set: &dyn Fn(&str) -> bool,
) -> bool {
    !stderr_tty && q::cargo_progress_target(argv) && !already_set("CARGO_TERM_PROGRESS_WHEN")
}

/// 進みを読む子へ足す env（純関数）。幅は利用者が決めていなければ 80
pub fn progress_env(already_set: &dyn Fn(&str) -> bool) -> Vec<(&'static str, &'static str)> {
    let mut v = vec![("CARGO_TERM_PROGRESS_WHEN", "always")];
    if !already_set("CARGO_TERM_PROGRESS_WIDTH") {
        v.push(("CARGO_TERM_PROGRESS_WIDTH", "80"));
    }
    v
}

fn request(action: &str, job: &JobWire) -> Request {
    Request::Queue {
        action: Some(action.into()),
        job: Some(job.clone()),
        id: None,
        limits: None,
        pane: None,
        priority: None,
    }
}

/// 待ちの表示（stderr。TTY なら同じ行を書き換え、非 TTY は 60 秒ごとに 1 行）
struct Display {
    tty: bool,
    /// 行の表示言語（#2365）
    lang: Lang,
    shown: bool,
    last_line: String,
    last_plain: Option<Instant>,
}

impl Display {
    fn new(lang: Lang) -> Self {
        Self {
            tty: std::io::stderr().is_terminal(),
            lang,
            shown: false,
            last_line: String::new(),
            last_plain: None,
        }
    }

    fn waiting(&mut self, resp: &Value, waited: Duration) {
        let line = waiting_line(resp, waited, self.lang);
        let mut err = std::io::stderr();
        if self.tty {
            if line != self.last_line {
                let _ = write!(err, "\r\x1b[2K{line}");
                let _ = err.flush();
            }
        } else if self
            .last_plain
            .is_none_or(|at| at.elapsed() >= PLAIN_REPORT_EVERY)
        {
            let _ = writeln!(err, "{line}");
            self.last_plain = Some(Instant::now());
        }
        self.shown = true;
        self.last_line = line;
    }

    /// 待ちの表示を閉じて 1 行を出す（待ちを出していなければ何も出さない = 即座に入れたときは静か）
    fn finish(&mut self, message: Option<&str>) {
        if !self.shown {
            return;
        }
        let mut err = std::io::stderr();
        if self.tty {
            let _ = write!(err, "\r\x1b[2K");
        }
        if let Some(m) = message {
            let _ = writeln!(err, "{m}");
        }
        let _ = err.flush();
        self.shown = false;
    }
}

fn note(message: &str) {
    eprintln!("tako queue: {message}");
}

/// 起こす子（#2538）。argv[0] を**待つ前**（install の差し替えで自分の実行ファイルが消える前）に
/// このプロセスの PATH で絶対パスへ引いて持ち、待ち明けはそこを起こす。素の名前のまま起こすと、
/// 差し替えの後の PATH の空振りが 1 本ずつ AppleSystemPolicy に拒否される（10/30 の本番で
/// install の前から待っていた包み 2 つが 41 本ずつ）
struct Program<'a> {
    argv: &'a [String],
    /// 待つ前に引いた絶対パス（引けない・相対エントリで当たる・Windows・A/B は `None` = 素の名前）
    pinned: Option<String>,
}

impl<'a> Program<'a> {
    fn pin(argv: &'a [String]) -> Self {
        Self {
            argv,
            pinned: tako_core::platform::exe::pin_before_wait(&argv[0]),
        }
    }

    /// 子のコマンド。持った絶対パスが起こす時点で消えていれば素の名前（= 修正前の起こし方）。
    /// argv[0] は利用者が打った形のまま渡す（`ps` の見え方・argv[0] で振る舞いを決める rustup の proxy 等）
    fn command(&self) -> Command {
        let program = self
            .pinned
            .as_deref()
            .filter(|p| tako_core::platform::exe::is_executable_file(Path::new(p)))
            .unwrap_or(self.argv[0].as_str());
        let mut cmd = Command::new(program);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.arg0(&self.argv[0]);
        }
        cmd.args(&self.argv[1..]);
        cmd
    }
}

/// 順番待ちせずに走らせる（入れ子・tako が居ない・調停役でない）
fn run_direct(program: &Program, lang: Lang) -> i32 {
    match program.command().spawn() {
        Ok(mut child) => wait_plain(&mut child),
        Err(e) => {
            note(
                &ReplyNote::QueueSpawnFailed {
                    program: program.argv[0].clone(),
                    error: e.to_string(),
                }
                .text_in(lang),
            );
            q::EXIT_SPAWN_FAILED
        }
    }
}

/// 子の終わりを待つ（シグナルは子へ転送する）
fn wait_plain(child: &mut Child) -> i32 {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return exit_code_of(status),
            Ok(None) => {}
            Err(_) => return 1,
        }
        for sig in signals::take() {
            signals::forward(child.id(), sig);
        }
        std::thread::sleep(CHILD_TICK);
    }
}

/// 取り分を渡す env（純関数）: 利用者が既に設定しているキーは渡さない。取り分 0 = 渡さない
pub fn share_env(share: u32, already_set: impl Fn(&str) -> bool) -> Vec<(&'static str, String)> {
    if share == 0 {
        return Vec::new();
    }
    ["CARGO_BUILD_JOBS", "RUST_TEST_THREADS"]
        .into_iter()
        .filter(|k| !already_set(k))
        .map(|k| (k, share.to_string()))
        .collect()
}

/// `tako queue <コマンド>` を走らせて終了コードを返す
pub fn run(
    argv: &[String],
    opts: &Options,
    send: &mut dyn FnMut(Request) -> Result<Value, SendError>,
) -> i32 {
    let nested = std::env::var(ENV_JOB).is_ok_and(|v| !v.is_empty());
    run_with(argv, opts, send, nested, &|k| std::env::var_os(k).is_some())
}

/// [`run`] の本体（入れ子か・利用者の env を外から渡す = テストが env に依らない）
fn run_with(
    argv: &[String],
    opts: &Options,
    send: &mut dyn FnMut(Request) -> Result<Value, SendError>,
    nested: bool,
    already_set: &dyn Fn(&str) -> bool,
) -> i32 {
    let lang = opts.lang;
    if argv.is_empty() {
        note(&ReplyNote::QueueNoCommand.text_in(lang));
        return 2;
    }
    // 待つ前に子の場所を引いておく（待ちの間に install で自分の実行ファイルが置き換わっても、
    // 待ち明けは PATH を空振りせずに起こす。#2538）
    let program = Program::pin(argv);
    signals::install();
    // 入れ子: 外側の枠の中で即実行
    if nested {
        return run_direct(&program, lang);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let mut wire = build_wire(argv, opts, &cwd);

    let first = match send(request("enqueue", &wire)) {
        Ok(v) => v,
        Err(SendError::Unreachable(_)) => {
            note(&ReplyNote::QueueRunOffline.text_in(lang));
            return run_direct(&program, lang);
        }
        Err(SendError::Timeout(limit)) => {
            note(&ReplyNote::QueueRunUnresponsive(limit.as_secs()).text_in(lang));
            return run_direct(&program, lang);
        }
        Err(SendError::Rejected(m)) => {
            note(&ReplyNote::QueueRunRejected(m).text_in(lang));
            return run_direct(&program, lang);
        }
    };

    // ---- 待つ ----
    let wait_start = Instant::now();
    let mut display = Display::new(lang);
    adopt_job_priority(&mut wire, &first);
    let mut resp = first;
    let mut lost_since: Option<Instant> = None;
    let mut attempts = 0u32;
    let max_wait = opts.max_wait.unwrap_or_else(|| {
        Duration::from_millis(
            resp["max_wait_ms"]
                .as_u64()
                .unwrap_or(u64::from(q::DEFAULT_MAX_WAIT_MIN) * 60_000),
        )
    });
    let share = loop {
        match resp["state"].as_str() {
            Some("granted") | Some("running") => {
                break resp["share"].as_u64().unwrap_or(0) as u32;
            }
            Some("cancelled") => {
                display.finish(None);
                note(&ReplyNote::QueueCancelledWhileWaiting.text_in(lang));
                return q::EXIT_CANCELLED;
            }
            _ => display.waiting(&resp, wait_start.elapsed()),
        }
        // 待つ間もシグナルと上限を見る（1 秒を 100 ms に刻む）
        let tick_end = Instant::now() + POLL_INTERVAL;
        while Instant::now() < tick_end {
            if let Some(sig) = signals::take().into_iter().next() {
                display.finish(None);
                withdraw(send, &wire);
                note(&ReplyNote::QueueLeftOnSignal.text_in(lang));
                return 128 + sig;
            }
            std::thread::sleep(CHILD_TICK);
        }
        if wait_start.elapsed() >= max_wait {
            display.finish(None);
            // 諦めたことを調停役へ知らせる（押さえていた処理を `recent` に残す。#2254）
            wire.exit_code = Some(q::EXIT_MAX_WAIT);
            withdraw(send, &wire);
            note(
                &ReplyNote::QueueGaveUpAfter {
                    waiting: resp["waiting"].as_u64().unwrap_or(0),
                    waited_s: max_wait.as_secs(),
                }
                .text_in(lang),
            );
            for line in gave_up_lines_in(&resp, lang) {
                note(&line);
            }
            return q::EXIT_MAX_WAIT;
        }
        // 見失った時刻は「失敗した往復を始めた時刻」（応答しない調停役は 1 往復が上限まで
        // かかるので、終わった時刻から数えると猶予が上限の分だけ伸びる。#2123）
        let polled_at = Instant::now();
        match send(request("poll", &wire)) {
            Ok(v) => {
                adopt_job_priority(&mut wire, &v);
                resp = v;
                lost_since = None;
                attempts = 0;
            }
            Err(failure @ (SendError::Unreachable(_) | SendError::Timeout(_))) => {
                let since = *lost_since.get_or_insert(polled_at);
                if since.elapsed() >= LOST_GRACE {
                    display.finish(None);
                    note(
                        &ReplyNote::QueueRunLost {
                            unresponsive: matches!(failure, SendError::Timeout(_)),
                            grace_s: LOST_GRACE.as_secs(),
                        }
                        .text_in(lang),
                    );
                    return run_direct(&program, lang);
                }
                std::thread::sleep(reconnect_delay(attempts).saturating_sub(POLL_INTERVAL));
                attempts += 1;
            }
            Err(SendError::Rejected(m)) => {
                display.finish(None);
                note(&ReplyNote::QueueRunRejected(m).text_in(lang));
                return run_direct(&program, lang);
            }
        }
    };
    let waited = wait_start.elapsed();
    if display.shown {
        let started = ReplyNote::QueueStarted {
            waited_s: waited.as_secs(),
            share,
        };
        display.finish(Some(&format!("tako queue: {}", started.text_in(lang))));
    }

    // ---- 走らせる ----
    let mut cmd = program.command();
    cmd.env(ENV_JOB, wire.id.to_string());
    cmd.envs(share_env(share, already_set));
    // #1996: 端末でない stderr の cargo は進みを強制して読む（その行は素通しから外す）。A/B は素通し
    let stderr_tty = std::io::stderr().is_terminal();
    let read_progress = !legacy_1996() && reads_cargo_progress(argv, stderr_tty, already_set);
    if read_progress {
        cmd.envs(progress_env(already_set));
        cmd.stderr(std::process::Stdio::piped());
    }
    // #2462 / #2697: 取り消しを子孫まで届かせる入れ物（Windows だけ。一時停止で起こし、入れてから動かす
    // = 子が孫を起こす前に入る。子はこの端末のコンソールを受け継ぐ）。子が普通に終わったら drop が
    // 何も止めずに手放す
    let spawned = tako_core::platform::job::KillScope::spawn(
        &mut cmd,
        tako_core::platform::job::Console::Inherit,
        |_| true,
    );
    let (mut child, mut descendants) = match spawned {
        Ok(spawned) => spawned,
        Err(e) => {
            note(
                &ReplyNote::QueueSpawnFailed {
                    program: argv[0].clone(),
                    error: e.to_string(),
                }
                .text_in(lang),
            );
            wire.exit_code = Some(q::EXIT_SPAWN_FAILED);
            let _ = send(request("done", &wire));
            return q::EXIT_SPAWN_FAILED;
        }
    };
    // #1996: 進みを読むなら子の stderr を受ける背景スレッド
    let tap = child.stderr.take().map(progress_tap::start);
    wire.child_pid = Some(child.id());
    wire.started_at_ms = Some(unix_ms());
    let mut cancel_at: Option<Instant> = None;
    let mut killed = false;
    let mut next_beat = Instant::now();
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break exit_code_of(status),
            Ok(None) => {}
            Err(_) => break 1,
        }
        for sig in signals::take() {
            signals::forward(child.id(), sig);
        }
        if Instant::now() >= next_beat {
            if let Some(t) = &tap {
                wire.progress = t.latest().or(wire.progress);
            }
            // 届かなくても子は止めない（繋がったら子の pid で登録し直る）
            let beat = send(request("started", &wire));
            // 次の心拍は往復が終わってから数える（応答しない調停役で 1 往復が上限まで
            // かかっても、次の心拍までの間は子の終わりとシグナルを見られる。#2123）
            next_beat = Instant::now() + HEARTBEAT_INTERVAL;
            if let Ok(v) = beat {
                adopt_job_priority(&mut wire, &v);
                if v["cancel"] == true && cancel_at.is_none() {
                    note(&ReplyNote::QueueStoppingCancelled(KILL_GRACE.as_secs()).text_in(lang));
                    tree::terminate(&mut child, &mut descendants, false);
                    cancel_at = Some(Instant::now());
                }
            }
        }
        if let Some(at) = cancel_at {
            if !killed && at.elapsed() >= KILL_GRACE {
                tree::terminate(&mut child, &mut descendants, true);
                killed = true;
            }
        }
        std::thread::sleep(CHILD_TICK);
    };
    // 子の stderr を読み切ってから（最後の N/M と途中の行を落とさない）。子孫が stderr を持ったまま
    // 残っても待ち続けない（[`progress_tap::DRAIN_GRACE`]）
    if let Some(t) = tap {
        wire.progress = t.finish(progress_tap::DRAIN_GRACE).or(wire.progress);
    }
    wire.exit_code = Some(code);
    let _ = send(request("done", &wire));
    code
}

/// 子の stderr を受けて cargo の進みの行を外し、残りを自分の stderr へ流す背景スレッド（#1996）
mod progress_tap {
    use std::io::{Read, Write};
    use std::process::ChildStderr;
    use std::sync::mpsc::{self, Receiver};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use tako_core::queue::eta::ProgressFilter;
    use tako_core::queue::Progress;

    /// 子が終わってから stderr を読み切るのを待つ上限（子孫が stderr を持ったまま残る場合）
    pub const DRAIN_GRACE: Duration = Duration::from_secs(2);

    pub struct Tap {
        latest: Arc<Mutex<Option<Progress>>>,
        done: Receiver<()>,
    }

    pub fn start(mut stderr: ChildStderr) -> Tap {
        let latest = Arc::new(Mutex::new(None));
        let (tx, done) = mpsc::channel();
        let shared = Arc::clone(&latest);
        let spawned = std::thread::Builder::new()
            .name("tako-queue-progress".into())
            .spawn(move || {
                let mut filter = ProgressFilter::new();
                let mut buf = [0u8; 8192];
                let mut out = std::io::stderr();
                loop {
                    match stderr.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let pass = filter.feed(&buf[..n]);
                            if !pass.is_empty() {
                                let _ = out.write_all(&pass);
                                let _ = out.flush();
                            }
                            if filter.last.is_some() {
                                *shared.lock().unwrap_or_else(|p| p.into_inner()) = filter.last;
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(_) => break,
                    }
                }
                let rest = filter.finish();
                if !rest.is_empty() {
                    let _ = out.write_all(&rest);
                    let _ = out.flush();
                }
                let _ = tx.send(());
            });
        if let Err(e) = spawned {
            // 読み手を立てられない（スレッドが足りない）: 子の stderr は誰も読まない = 詰まらないよう落とす
            eprintln!("tako queue: 進みの読み手を立てられない（{e}）");
        }
        Tap { latest, done }
    }

    impl Tap {
        pub fn latest(&self) -> Option<Progress> {
            *self.latest.lock().unwrap_or_else(|p| p.into_inner())
        }

        /// 読み切るまで（上限 `grace`）待って最後の進みを返す
        pub fn finish(self, grace: Duration) -> Option<Progress> {
            let _ = self.done.recv_timeout(grace);
            self.latest()
        }
    }
}

/// 待ちから外れる（届かなくても構わない。調停役は pid の生存でも外す）
fn withdraw(send: &mut dyn FnMut(Request) -> Result<Value, SendError>, wire: &JobWire) {
    let _ = send(request("done", wire));
}

/// シグナルの受け取りと転送（unix。Windows は何もしない = Ctrl+C はコンソールの全員に届く）
mod signals {
    #[cfg(unix)]
    mod imp {
        use std::sync::atomic::{AtomicU32, Ordering};

        static PENDING: AtomicU32 = AtomicU32::new(0);
        const WATCHED: [libc::c_int; 4] =
            [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT];

        extern "C" fn on_signal(sig: libc::c_int) {
            // async-signal-safe: アトミックへ立てるだけ
            if (0..32).contains(&sig) {
                PENDING.fetch_or(1 << sig, Ordering::SeqCst);
            }
        }

        pub fn install() {
            for sig in WATCHED {
                // SAFETY: ハンドラはアトミックの更新だけをする async-signal-safe な関数
                unsafe {
                    libc::signal(sig, on_signal as *const () as libc::sighandler_t);
                }
            }
        }

        pub fn take() -> Vec<i32> {
            let bits = PENDING.swap(0, Ordering::SeqCst);
            (0..32).filter(|s| bits & (1 << s) != 0).collect()
        }

        pub fn forward(pid: u32, sig: i32) {
            let Ok(pid) = libc::pid_t::try_from(pid) else {
                return;
            };
            if pid > 0 {
                // SAFETY: 自分が起こした子の pid へシグナルを送るだけ
                unsafe {
                    libc::kill(pid, sig);
                }
            }
        }
    }

    #[cfg(not(unix))]
    mod imp {
        pub fn install() {}
        pub fn take() -> Vec<i32> {
            Vec::new()
        }
        pub fn forward(_pid: u32, _sig: i32) {}
    }

    pub use imp::{forward, install, take};
}

/// 子とその子孫を止める（取り消し）
mod tree {
    use std::process::Child;

    use tako_core::platform::job::KillScope;

    /// unix: 子とその子孫（libproc の親子表）へ SIGTERM（`force` なら SIGKILL）。
    /// 親子表が採れなければ子だけ。入れ物（`KillScope`）は unix では空なので見ない
    #[cfg(unix)]
    pub fn terminate(child: &mut Child, _descendants: &mut KillScope, force: bool) {
        let root = child.id();
        let sig = if force { libc::SIGKILL } else { libc::SIGTERM };
        let mut targets = vec![root];
        if let Some(procs) = tako_core::platform::resources::processes() {
            let mut frontier = vec![root];
            while let Some(p) = frontier.pop() {
                for r in procs.iter().filter(|r| r.ppid == p && r.pid != p) {
                    if !targets.contains(&r.pid) {
                        targets.push(r.pid);
                        frontier.push(r.pid);
                    }
                }
            }
        }
        for pid in targets {
            if let Ok(pid) = libc::pid_t::try_from(pid) {
                if pid > 0 {
                    // SAFETY: 自分が起こした子とその子孫へ送るだけ
                    unsafe {
                        libc::kill(pid, sig);
                    }
                }
            }
        }
    }

    /// Windows: 子とその子孫を入れ物（Job Object。境界 B30。#2462）ごと止める。寿命はペインを
    /// 閉じるのと同じ（コンソールのプロセスは止め、GUI のアプリとその子孫は残す）。穏当な終了要求は
    /// 無いので `force` は見ない（1 回目で止まる。2 回目の入れ物は空）。入れ物が無い回
    /// （入れられなかった・A/B の `TAKO_2454_LEGACY=1`）は子だけ（孫は残る = #2462 の前）
    #[cfg(not(unix))]
    pub fn terminate(child: &mut Child, descendants: &mut KillScope, _force: bool) {
        descendants.stop();
        let _ = child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn 待ちの上限の書き方() {
        assert_eq!(parse_duration("90s").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_duration("10m").unwrap(), Duration::from_secs(600));
        assert_eq!(parse_duration("2h").unwrap(), Duration::from_secs(7200));
        assert_eq!(
            parse_duration("5").unwrap(),
            Duration::from_secs(300),
            "単位なしは分"
        );
        assert!(parse_duration("0s").is_err());
        assert!(parse_duration("x").is_err());
        assert!(parse_duration("3d").is_err());
    }

    #[test]
    fn 繋ぎ直しは1から5秒() {
        let secs: Vec<u64> = (0..12).map(|a| reconnect_delay(a).as_secs()).collect();
        assert_eq!(secs, vec![1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 5, 5]);
    }

    #[test]
    fn 待ちの1行は順位と理由を出す() {
        let head = json!({
            "position": 1, "waiting": 3, "running": 3, "max_jobs": 3, "unmanaged": 1,
            "text": "同時数の上限（実行 3 + tako の外 1 / 上限 3）",
        });
        assert_eq!(
            waiting_line(&head, Duration::from_secs(72), Lang::Ja),
            "tako queue: 待ち 1 番目（待ち 3 件・実行 3/3・tako の外 1・1 分 12 秒）— 同時数の上限（実行 3 + tako の外 1 / 上限 3）"
        );
        let behind = json!({
            "position": 3, "waiting": 3, "running": 1, "max_jobs": 3,
            "text": "前に並んでいる処理の後", "head_text": "メモリ圧 warn（実行中が終わってから 1 件ずつ）",
        });
        assert!(waiting_line(&behind, Duration::from_secs(5), Lang::Ja)
            .ends_with("— 前に 2 件（先頭: メモリ圧 warn（実行中が終わってから 1 件ずつ））"));
        // #2243: 先頭が target dir 待ちで、2 番目の自分は空き枠の無さで待っている → 自分の理由を出す
        // （「先頭: 同じ target dir …」と出すと、追い越せる先頭の後ろで待っているように読める）
        let own = json!({
            "position": 2, "waiting": 2, "running": 3, "max_jobs": 3,
            "reason": "max_jobs", "text": "同時数の上限（実行 3 / 上限 3）",
            "head_reason": "max_jobs", "head_text": "同時数の上限（実行 3 / 上限 3）",
        });
        assert!(waiting_line(&own, Duration::from_secs(5), Lang::Ja)
            .ends_with("— 同時数の上限（実行 3 / 上限 3）"));
        // 追い越しの上限に達した先頭の後ろ
        let held = json!({
            "position": 2, "waiting": 2, "running": 1, "max_jobs": 3,
            "reason": "ahead", "text": "前に並んでいる処理の後",
            "head_reason": "overtake_limit", "head_text": "上限",
        });
        assert!(waiting_line(&held, Duration::from_secs(5), Lang::Ja)
            .ends_with("— 前に 1 件（先頭: 上限）"));
    }

    /// #2365: 英語表示の待ちの 1 行は調停役の日本語の文を使わず、応答の構造の欄（`reason` /
    /// `head_reason` / `start_s`）から組む（日本語の行と同じ欄を同じ並びで言う）
    #[test]
    fn 英語の待ちの1行は構造の欄から組み日本語を含まない() {
        let head = json!({
            "position": 1, "waiting": 3, "running": 3, "max_jobs": 3, "unmanaged": 1,
            "reason": "max_jobs", "text": "同時数の上限（実行 3 + tako の外 1 / 上限 3）",
            "start_s": 0, "start_text": "空き次第すぐ開始の見込み",
        });
        assert_eq!(
            waiting_line(&head, Duration::from_secs(72), Lang::En),
            "tako queue: #1 in the queue (3 waiting, running 3/3, 1 outside tako, waited 1m 12s, \
             starts as soon as a slot frees) — the concurrent job limit is reached"
        );
        assert_eq!(
            waiting_line(&head, Duration::from_secs(72), Lang::Ja),
            "tako queue: 待ち 1 番目（待ち 3 件・実行 3/3・tako の外 1・1 分 12 秒・空き次第すぐ開始の見込み）\
             — 同時数の上限（実行 3 + tako の外 1 / 上限 3）"
        );
        let held = json!({
            "position": 3, "waiting": 3, "running": 1, "max_jobs": 3,
            "reason": "ahead", "text": "前に並んでいる処理の後",
            "head_reason": "memory", "head_text": "空きメモリ不足（空き 1.0 GB）",
            "start_s": 240, "start_text": "開始まで約 4 分",
        });
        assert_eq!(
            waiting_line(&held, Duration::from_secs(5), Lang::En),
            "tako queue: #3 in the queue (3 waiting, running 1/3, waited 5s, starts in about 4 min) \
             — 2 job(s) ahead (the head: not enough free memory)"
        );
        // 旧い調停役（理由の種類も先頭の理由も返さない）でも日本語は混ざらない
        let old = json!({"position": 2, "waiting": 2, "running": 1, "max_jobs": 1, "text": "x"});
        let line = waiting_line(&old, Duration::from_secs(0), Lang::En);
        assert_eq!(
            line,
            "tako queue: #2 in the queue (2 waiting, running 1/1, waited 0s) — 1 job(s) ahead"
        );
    }

    #[test]
    fn 受付の1件は短いラベルと区分を持つ() {
        let argv: Vec<String> = ["/usr/local/bin/cargo", "build", "--release"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let w = build_wire(&argv, &Options::default(), Path::new("/"));
        assert_eq!(w.id, u64::from(std::process::id()));
        assert_eq!(
            w.label, "cargo build --release",
            "絶対パスは basename へ畳む"
        );
        assert_eq!(w.kind, JobKind::Build);
        assert_eq!(w.priority, Priority::Normal);
        assert!(w.enqueued_at_ms > 0);
        let w = build_wire(
            &argv,
            &Options {
                label: Some("名前".into()),
                priority: Some(Priority::Low),
                ..Options::default()
            },
            Path::new("/"),
        );
        assert_eq!((w.label.as_str(), w.priority), ("名前", Priority::Low));
        assert_eq!(
            w.hint.map(|h| h.set),
            Some(Some(Priority::Low)),
            "付けた段は手がかりにも載る（#1996）"
        );
        // --target-dir は cwd の workspace より強い
        let argv: Vec<String> = ["cargo", "build", "--target-dir", "/x/dist-target"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let w = build_wire(&argv, &Options::default(), Path::new("/"));
        assert_eq!(w.target_dir.as_deref(), Some("/x/dist-target"));
        let argv: Vec<String> = ["cargo", "build", "--target-dir=/y/t"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let w = build_wire(&argv, &Options::default(), Path::new("/"));
        assert_eq!(w.target_dir.as_deref(), Some("/y/t"));
    }

    #[cfg(unix)]
    #[test]
    fn シグナルで終わった子は128足す番号() {
        let status = Command::new("sh")
            .args(["-c", "kill -TERM $$"])
            .status()
            .unwrap();
        assert_eq!(exit_code_of(status), 128 + 15);
        let status = Command::new("sh").args(["-c", "exit 7"]).status().unwrap();
        assert_eq!(exit_code_of(status), 7);
    }

    /// 調停役が居ないとき（届かない）は待たずに走り、子の終了コードを素通しする
    #[cfg(unix)]
    #[test]
    fn 届かなければ待たずに走る() {
        let argv: Vec<String> = ["sh", "-c", "exit 9"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut calls = 0;
        let mut send = |_req: Request| -> Result<Value, SendError> {
            calls += 1;
            Err(SendError::Unreachable("no".into()))
        };
        assert_eq!(
            run_with(&argv, &Options::default(), &mut send, false, &|_| false),
            9
        );
        // 待たなかったことは「受付の 1 回で諦めた」= 再試行も poll もしていないことで見る
        assert_eq!(calls, 1, "受付の 1 回だけ");
    }

    /// 繋がったまま応答しない調停役（#2123）も待たずに走り、子の終了コードを素通しする
    #[cfg(unix)]
    #[test]
    fn 応答しなければ待たずに走る() {
        let argv: Vec<String> = ["sh", "-c", "exit 8"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut calls = 0;
        let mut send = |_req: Request| -> Result<Value, SendError> {
            calls += 1;
            Err(SendError::Timeout(Duration::from_secs(30)))
        };
        assert_eq!(
            run_with(&argv, &Options::default(), &mut send, false, &|_| false),
            8
        );
        assert_eq!(calls, 1, "受付の 1 回で諦める（done も送らない）");
    }

    /// 実行中の心拍が応答しなくても子は止めず、終わりまで走らせて done を送る（#2123）
    #[cfg(unix)]
    #[test]
    fn 実行中に応答しなくなっても子は走りきる() {
        let argv: Vec<String> = ["sh", "-c", "sleep 0.3; exit 6"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut actions: Vec<String> = Vec::new();
        let mut send = |req: Request| -> Result<Value, SendError> {
            let Request::Queue { action, .. } = &req else {
                panic!()
            };
            let action = action.clone().unwrap_or_default();
            actions.push(action.clone());
            match action.as_str() {
                "enqueue" => Ok(json!({ "state": "granted", "share": 0 })),
                _ => Err(SendError::Timeout(Duration::from_millis(10))),
            }
        };
        assert_eq!(
            run_with(&argv, &Options::default(), &mut send, false, &|_| false),
            6
        );
        assert_eq!(actions.first().map(String::as_str), Some("enqueue"));
        assert_eq!(actions.last().map(String::as_str), Some("done"));
    }

    #[test]
    fn 取り分は利用者が決めていないキーにだけ渡す() {
        assert_eq!(
            share_env(6, |_| false),
            vec![
                ("CARGO_BUILD_JOBS", "6".to_string()),
                ("RUST_TEST_THREADS", "6".to_string())
            ]
        );
        assert_eq!(
            share_env(6, |k| k == "CARGO_BUILD_JOBS"),
            vec![("RUST_TEST_THREADS", "6".to_string())]
        );
        assert!(share_env(0, |_| false).is_empty(), "取り分なし = 渡さない");
    }

    /// 入れ子は調停役へ 1 度も繋がずに即実行する
    #[cfg(unix)]
    #[test]
    fn 入れ子は待たない() {
        let argv: Vec<String> = ["sh", "-c", "exit 5"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut calls = 0;
        let mut send = |_req: Request| -> Result<Value, SendError> {
            calls += 1;
            Ok(json!({ "state": "waiting" }))
        };
        assert_eq!(
            run_with(&argv, &Options::default(), &mut send, true, &|_| false),
            5
        );
        assert_eq!(calls, 0);
    }

    /// 割り当てを受けたら取り分と受付番号を env で渡し、終わったら done を送る
    #[cfg(unix)]
    #[test]
    fn 割り当てで取り分を渡しdoneで終わる() {
        let argv: Vec<String> = [
            "sh",
            "-c",
            "test \"$RUST_TEST_THREADS\" = 3 -a -n \"$TAKO_QUEUE_JOB\" && exit 4",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut actions: Vec<String> = Vec::new();
        let mut send = |req: Request| -> Result<Value, SendError> {
            let Request::Queue { action, .. } = &req else {
                panic!()
            };
            actions.push(action.clone().unwrap_or_default());
            Ok(match action.as_deref() {
                Some("enqueue") => json!({ "state": "granted", "share": 3 }),
                Some("done") => json!({ "state": "done" }),
                _ => json!({ "state": "running", "cancel": false }),
            })
        };
        // 利用者の env は「何も決めていない」として渡す（このテスト自体が順番待ちの中で回っても揺れない）
        assert_eq!(
            run_with(&argv, &Options::default(), &mut send, false, &|_| false),
            4
        );
        assert_eq!(actions.first().map(String::as_str), Some("enqueue"));
        assert_eq!(actions.last().map(String::as_str), Some("done"));
    }

    /// 待ちの上限で 75。外れるときに done を送る
    #[cfg(unix)]
    #[test]
    fn 待ちの上限で75() {
        let argv: Vec<String> = ["true"].iter().map(|s| s.to_string()).collect();
        let mut done = None;
        let mut send = |req: Request| -> Result<Value, SendError> {
            let Request::Queue { action, job, .. } = &req else {
                panic!()
            };
            if action.as_deref() == Some("done") {
                done = job.as_ref().map(|j| j.exit_code);
            }
            Ok(
                json!({ "state": "waiting", "position": 1, "waiting": 1, "running": 1, "max_jobs": 1 }),
            )
        };
        let opts = Options {
            max_wait: Some(Duration::from_secs(1)),
            ..Options::default()
        };
        assert_eq!(
            run_with(&argv, &opts, &mut send, false, &|_| false),
            q::EXIT_MAX_WAIT
        );
        assert_eq!(
            done,
            Some(Some(q::EXIT_MAX_WAIT)),
            "外れるときに done（諦めた = 75 を載せる。調停役が押さえていた処理を残す。#2254）"
        );
    }

    /// #2254: 諦めたときに、待っていた理由と押さえていた処理と量を出す
    #[test]
    fn 諦めたときは押さえていた処理と量を出す() {
        let resp = json!({
            "state": "waiting", "position": 1, "reason": "memory",
            "text": "空きメモリ不足（空き 8.0 GB・必要 13.7 GB = 実行中のまだ使っていない見込み 6.7 GB + この処理の見込み 7.0 GB・予約 3.6 GB）",
            "holders": [{
                "id": 4242, "label": "cargo test --workspace", "memory_mb": 300, "peak_mb": 2048,
                "estimate_mb": 7168, "estimate_from": "default", "pending_mb": 6868,
                "peak_passed": false, "stalled": false,
            }, {
                "id": 4343, "label": "cargo build", "memory_mb": null, "estimate_mb": 3584,
                "estimate_from": "history", "pending_mb": 0, "peak_passed": true, "stalled": true,
            }],
        });
        assert_eq!(
            gave_up_lines_in(&resp, Lang::Ja),
            vec![
                "待っていた理由: 空きメモリ不足（空き 8.0 GB・必要 13.7 GB = 実行中のまだ使っていない見込み 6.7 GB + \
                 この処理の見込み 7.0 GB・予約 3.6 GB）"
                    .to_string(),
                "押さえていた処理: 4242 cargo test --workspace（使用 300 MB・最大 2.0 GB・まだ使っていない見込み 6.7 GB・\
                 見込み 7.0 GB（区分の既定））"
                    .to_string(),
                "押さえていた処理: 4343 cargo build（使用 未計測・見込み 3.5 GB（過去の記録）・ピーク後・停滞）"
                    .to_string(),
            ]
        );
        // 並びが止まって調べなかった件は先頭の理由・旧い調停役（holders なし）は理由だけ
        let behind = json!({ "reason": "ahead", "head_text": "同時数の上限（実行 3 / 上限 3）" });
        assert_eq!(
            gave_up_lines_in(&behind, Lang::Ja),
            vec!["待っていた理由: 前に並んでいる処理の後（先頭: 同時数の上限（実行 3 / 上限 3））"]
        );
        assert!(gave_up_lines_in(&json!({}), Lang::Ja).is_empty());
    }
}
