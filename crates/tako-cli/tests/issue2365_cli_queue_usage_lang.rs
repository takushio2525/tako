//! 実経路: `tako queue`（状況・cancel / start / priority / limits）と `tako usage` の人向けの表示が表示言語に
//! 従う（Issue #2365 の続き。前のスライスは `issue2365_cli_sleep_recover_watch_lang.rs`）。
//!
//! 修正前（実測）: 英語表示でも queue は機の状態の行（「メモリ」「圧縮」「空き」「コア」）・重い処理と
//! ペインの上位・順番待ちの節（「実行」「待ち」「取り分」「直近」「直近の諦め」）・起動待ちと cargo の札の節・
//! cancel / start / priority / limits の結果、usage は見出し・ログインと枠の行・量の行（万・億）・期間・
//! % の列のラベル・按分の要約・注記が、日本語の固定文のまま出た（`main.rs` の直書き）。調停役（GUI）が組む
//! 待つ理由・見込み・札を配らない理由と usage の注記（応答の値）もそのまま並んだ。続けて `tako queue <コマンド>`
//! （順番待ちで走らせる）の進みの行（stderr の待ち・開始・諦め・素通しの理由・取り消し）も、修正前は
//! `queue_wrapper` の直書きで英語表示でも日本語だった（`r_*` のケース。偽の調停役が受付 → poll の順に答える）。
//!
//! **ビルドした `tako` をそのまま**叩く。queue は決まった応答を返す受け口（偽の調停役。`TAKO_QUEUE_SOCKET`）を
//! 立て、届かない（誰も居ない口）・応答しない（黙る口 + `TAKO_IPC_TIMEOUT_SECS=1`）は実際の経路で出す。
//! usage は一時の設定 dir（`TAKO_USAGE_CONFIG_DIRS`）に偽の会話記録、一時の data dir に偽の利用制限の
//! 観測（壊れた行を 1 行含む）を置く。見ること:
//! - 英語表示: 人向けの出力に日本語（かな・漢字）が 1 字も無い（`--json` の出力は応答の値のままなので除く）
//! - 日本語表示: #2365 より前の出力とバイト一致（時刻・ログインの鍵・機の数値だけ正規化して比べる）
//! - 終了コードは日英で同じ・`--json` は日英で同じ（届かないときの `note` も日本語のまま）
//!
//! A/B: `TAKO_2365_HUMAN_BIN=<修正前の tako>` で別のバイナリを叩ける（修正前は英語表示の検査で落ちる）。
//! `I2365Q_DUMP=<dir>` を付けると全ケースの出力（正規化済み）を書き出す（修正前後の `diff -r` 用。
//! `I2365Q_EXACT=1` を足すと usage の数字を正規化せずに書き出す = 量・単価まで比べる）。
//! 本番の data dir・GUI・順番待ちへ届かないよう、`TAKO_*` を全部外して一時の置き場と偽の口だけを渡す。
//!
//! unix だけで走らせる（偽の調停役が Unix domain socket）。文の日英の対は OS に依らない 1 か所
//! （`ReplyNote` / `Reason`）で、両 OS の番犬（`crates/tako-control/tests/issue1762_dispatch_error_lang.rs`）が見る
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tako_control::protocol::ResponseEnvelope;
use tako_core::test_residue::ScratchDir;

/// 叩くバイナリ（既定はこの木でビルドした `tako`。A/B は `TAKO_2365_HUMAN_BIN`）
fn tako() -> PathBuf {
    std::env::var_os("TAKO_2365_HUMAN_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_tako")))
}

/// 1 回の実行の結果（正規化済み）
#[derive(Debug, Clone, PartialEq)]
struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

/// 偽の調停役の振る舞い
enum Arbiter {
    /// 受けた要求すべてへ同じ `result` を返す
    Reply(Value),
    /// 受けた順に `result` を返す（尽きたら最後のものを返し続ける。`tako queue <コマンド>` の受付 → poll）
    Script(Vec<Value>),
    /// 受けた要求すべてを断る（順番待ちを受けない tako）
    Refuse(&'static str),
    /// 受けて読むが答えない（応答の上限で切れる）
    Silent,
    /// 誰も居ない口（届かない）
    None,
}

/// 出力の正規化
#[derive(Clone, Copy, PartialEq)]
enum Norm {
    /// 置き場だけ
    Plain,
    /// 機の数値（数字の並びを `9` 1 字へ）。届かない・応答しないときの機の状態はこのプロセスで採る
    Machine,
    /// 時刻（`HH:MM` / `MM/DD HH:MM` / `YYYY/MM/DD HH:MM`）・ログインの鍵・数字（量と単価は単価表で変わる。
    /// 数字まで同じことは修正前後の書き出し（`I2365Q_DUMP`）で見る）
    Usage,
}

struct Case {
    name: &'static str,
    args: Vec<&'static str>,
    arbiter: Arbiter,
    extra: Vec<(&'static str, &'static str)>,
    norm: Norm,
    /// `--json` の出力（応答の値のまま = 英語表示でも日本語が残る。日英で同じことを見る）
    json: bool,
}

fn case(name: &'static str, args: &[&'static str], arbiter: Arbiter) -> Case {
    Case {
        name,
        args: args.to_vec(),
        arbiter,
        extra: Vec::new(),
        norm: Norm::Plain,
        json: false,
    }
}

/// 1 言語分の砂場（表示言語の設定・偽の調停役の口・usage の置き場）
struct Sandbox {
    dir: ScratchDir,
    /// usage のログインの鍵（会話記録を置いた後に `tako usage --json` で引く。正規化に使う）
    login: OnceLock<String>,
}

impl Sandbox {
    fn new(lang: &str) -> Self {
        let sb = Self {
            // 偽の調停役の Unix domain socket が macOS の上限（104 バイト）に収まるよう短い名前にする
            dir: ScratchDir::new("i2365q"),
            login: OnceLock::new(),
        };
        for d in ["data/usage", "disc", "orch", "home", "tmp", "cfg"] {
            let d = sb.path().join(d);
            std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
        }
        // 書かないと system = 実行環境のロケールで決まり、CI と手元で答えが変わる
        sb.write(
            "data/settings.json",
            &format!("{{\"language\":\"{lang}\"}}"),
        );
        sb
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn write(&self, rel: &str, text: &str) {
        let p = self.path().join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("{} を作れる: {e}", parent.display()));
        }
        std::fs::write(&p, text).unwrap_or_else(|e| panic!("{} を書ける: {e}", p.display()));
    }

    /// 偽の調停役の口を立てて、そのパスを返す
    fn arbiter(&self, tag: &str, arbiter: &Arbiter) -> String {
        let path = self.path().join(format!("{tag}.sock"));
        let (script, refuse) = match arbiter {
            Arbiter::None => return path.display().to_string(),
            Arbiter::Reply(result) => (Some(vec![result.clone()]), None),
            Arbiter::Script(results) => (Some(results.clone()), None),
            Arbiter::Refuse(message) => (None, Some(*message)),
            Arbiter::Silent => (None, None),
        };
        let listener = UnixListener::bind(&path).expect("受け口を作れる");
        std::thread::spawn(move || {
            let mut served = 0usize;
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let mut line = String::new();
                let _ =
                    BufReader::new(stream.try_clone().expect("複製できる")).read_line(&mut line);
                let id = serde_json::from_str::<Value>(&line)
                    .ok()
                    .and_then(|v| v["id"].as_u64())
                    .unwrap_or(1);
                let mut stream = stream;
                let reply = match (&script, refuse) {
                    (Some(results), _) => {
                        let result = results[served.min(results.len() - 1)].clone();
                        served += 1;
                        Some(ResponseEnvelope::ok(id, result))
                    }
                    (None, Some(message)) => Some(ResponseEnvelope::err(id, -32000, message)),
                    // 繋いだまま答えない（CLI が応答の上限で切る）
                    (None, None) => None,
                };
                match reply {
                    Some(reply) => {
                        let _ = writeln!(
                            stream,
                            "{}",
                            serde_json::to_string(&reply).expect("応答を組める")
                        );
                    }
                    None => std::thread::sleep(Duration::from_secs(5)),
                }
            }
        });
        path.display().to_string()
    }

    /// `TAKO_*` を外して隔離の置き場と偽の口だけを渡した `tako <args>` の結果（正規化前）
    fn run_raw(&self, args: &[&str], queue_socket: &str, extra: &[(&str, &str)]) -> Out {
        let mut command = Command::new(tako());
        command
            .args(args)
            .current_dir(self.path())
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
            .env("TAKO_ORCHESTRATOR_DIR", self.path().join("orch"))
            .env("TAKO_USAGE_CONFIG_DIRS", self.path().join("cfg"))
            .env("HOME", self.path().join("home"))
            .env("TMPDIR", self.path().join("tmp"))
            .env("TZ", "UTC")
            // 誰も居ない名前（`-L`）= 本番の器のサーバー（`tako`）へ問い合わせない
            .env("TAKO_TMUX_SOCKET", format!("i2365q{}", std::process::id()))
            // GUI は誰も居ない口・順番待ちは偽の調停役（明示の口 = 本番の順番待ちへ並ばない）
            .env("TAKO_SOCKET", self.path().join("none.sock"))
            .env("TAKO_QUEUE_SOCKET", queue_socket)
            .env("TAKO_TOKEN", "t");
        for (k, v) in extra {
            command.env(k, v);
        }
        let out = command.output().expect("tako が終わる");
        let root = self.path().display().to_string();
        let clean = |b: &[u8]| String::from_utf8_lossy(b).replace(&root, "<SB>");
        Out {
            code: out.status.code().unwrap_or(-1),
            stdout: clean(&out.stdout),
            stderr: clean(&out.stderr),
        }
    }

    fn run(&self, c: &Case) -> Out {
        let socket = self.arbiter(c.name, &c.arbiter);
        let out = self.run_raw(&c.args, &socket, &c.extra);
        let norm = |s: &str| match c.norm {
            Norm::Plain => s.to_string(),
            Norm::Machine => collapse_digits(s),
            Norm::Usage => {
                let s = self
                    .login
                    .get()
                    .map_or_else(|| s.to_string(), |l| s.replace(l.as_str(), "<LOGIN>"));
                // 書き出しの A/B で数字まで比べるときは `I2365Q_EXACT=1`（比較の表は数字を正規化した形）
                if std::env::var_os("I2365Q_EXACT").is_some() {
                    stamps(&s)
                } else {
                    collapse_digits(&stamps(&s))
                }
            }
        };
        Out {
            code: out.code,
            stdout: norm(&out.stdout),
            stderr: norm(&out.stderr),
        }
    }

    // ---- usage の見本 ----

    /// 偽の会話記録（今から 170〜8 分前・Opus 5.5 と単価表に無いモデル）を置き、ログインの鍵を引いてから
    /// 利用制限の観測（7d だけ。5h は観測が無い = 注記）と壊れた行を置く
    fn seed_usage(&self) {
        self.write(
            "cfg/.claude.json",
            "{\"oauthAccount\":{\"accountUuid\":\"aaaaaaaa-2365-4365-8365-444444444444\"}}\n",
        );
        let now = now_secs();
        let line = |id: &str, session: &str, ago_min: i64, model: &str, out: u64, cwd: &str| {
            format!(
                "{{\"type\":\"assistant\",\"message\":{{\"id\":\"{id}\",\"model\":\"{model}\",\
                 \"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"x\"}}],\
                 \"usage\":{{\"input_tokens\":0,\"output_tokens\":{out},\"cache_read_input_tokens\":1000000,\
                 \"speed\":\"standard\"}}}},\"requestId\":\"req_{id}\",\"timestamp\":\"{}\",\
                 \"sessionId\":\"{session}\",\"cwd\":\"{cwd}\",\"uuid\":\"u-{id}\"}}\n",
                iso(now - ago_min * 60)
            )
        };
        let alpha = "/home/testuser/dev/alpha";
        let beta = "/home/testuser/dev/beta";
        self.write(
            "cfg/projects/-home-testuser-dev-alpha/s-alpha.jsonl",
            &[
                line("msg_a1", "s-alpha", 170, "claude-opus-5-5", 100_000, alpha),
                line("msg_a2", "s-alpha", 120, "claude-opus-5-5", 100_000, alpha),
                line("msg_a3", "s-alpha", 60, "claude-opus-5-5", 300_000, alpha),
            ]
            .concat(),
        );
        self.write(
            "cfg/projects/-home-testuser-dev-beta/s-beta.jsonl",
            &[
                line("msg_b1", "s-beta", 100, "claude-opus-5-5", 50_000, beta),
                line("msg_b2", "s-beta", 52, "claude-sonnet-9-9", 50_000, beta),
                line("msg_b3", "s-beta", 8, "claude-opus-5-5", 50_000, beta),
            ]
            .concat(),
        );
        let probe = self.run_raw(&["usage", "--json"], "none.sock", &[]);
        let payload: Value = serde_json::from_str(&probe.stdout)
            .unwrap_or_else(|e| panic!("usage --json を読める: {e}\n{probe:?}"));
        let login = payload["logins"][0]["login"]
            .as_str()
            .unwrap_or_else(|| panic!("会話記録からログインが立つ: {payload}"))
            .to_string();
        let resets = now + 3 * 86400;
        let obs = |percent: u64, ago_min: i64| {
            let at = (now - ago_min * 60) * 1000;
            format!(
                "{{\"login\":\"{login}\",\"key_from\":\"account\",\"kind\":\"seven_day\",\"percent\":{percent},\
                 \"resets_at\":{resets},\"observed_at\":{at},\"recorded_at\":{at},\"source\":\"mod\"}}\n"
            )
        };
        self.write(
            "data/usage/limits.jsonl",
            &[
                obs(10, 150),
                obs(14, 90),
                obs(17, 45),
                obs(20, 30),
                "{\"login\": \"broken\n".to_string(),
            ]
            .concat(),
        );
        let _ = self.login.set(login);
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// unix 秒 → `YYYY-MM-DDTHH:MM:SS.000Z`（UTC）
fn iso(t: i64) -> String {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    // civil_from_days（Howard Hinnant）
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// 数字の並びを `9` 1 字へ（機の数値は実行のたびに変わる）
fn collapse_digits(s: &str) -> String {
    let mut out = String::new();
    let mut prev_digit = false;
    for c in s.chars() {
        let digit = c.is_ascii_digit();
        if !(digit && prev_digit) {
            out.push(if digit { '9' } else { c });
        }
        prev_digit = digit;
    }
    out
}

/// 時刻（`HH:MM`・その前の `MM/DD ` / `YYYY/MM/DD `）を `<T>` へ
fn stamps(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let d = |i: usize| c.get(i).is_some_and(char::is_ascii_digit);
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        // YYYY/MM/DD HH:MM
        if (0..4).all(|k| d(i + k))
            && c.get(i + 4) == Some(&'/')
            && d(i + 5)
            && d(i + 6)
            && c.get(i + 7) == Some(&'/')
            && d(i + 8)
            && d(i + 9)
            && c.get(i + 10) == Some(&' ')
        {
            i += 11;
            continue;
        }
        // MM/DD HH:MM
        if d(i)
            && d(i + 1)
            && c.get(i + 2) == Some(&'/')
            && d(i + 3)
            && d(i + 4)
            && c.get(i + 5) == Some(&' ')
        {
            i += 6;
            continue;
        }
        if d(i) && d(i + 1) && c.get(i + 2) == Some(&':') && d(i + 3) && d(i + 4) {
            out.push_str("<T>");
            i += 5;
            continue;
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

// ---------------------------------------------------------------------------
// 見本の応答（調停役が組む文は日本語 = 応答の値のまま）
// ---------------------------------------------------------------------------

fn holders() -> Value {
    json!([
        {"id": 4240, "label": "cargo clippy", "memory_mb": 1229, "peak_mb": 2048, "pending_mb": 1536,
         "estimate_mb": 3584, "estimate_from": "history", "peak_passed": true, "stalled": true},
        {"id": 4239, "label": "cargo doc", "pending_mb": 0, "estimate_mb": 2048, "estimate_from": "shape"},
    ])
}

/// 全部入りの状況（機・重い処理・ペイン・tako の外・走査・順番待ち・起動待ち・cargo の札）
fn status_full() -> Value {
    json!({
        "gui": true,
        "machine": {
            "memory_mb": {"used": 20582, "total": 36864, "app": 10240, "wired": 3072,
                          "compressed": 2048, "available": 16282},
            "swap_mb": {"used": 1024, "total": 2048},
            "pressure": "normal",
            "cpu_percent": 35.2,
            "cpus": 18,
            "load": [3.12, 2.5, 2.0],
        },
        "heavy": [
            {"label": "cargo build", "memory_mb": 1229, "cpu_percent": 250.0, "pane": 3, "tab": "work", "role": "worker"},
            {"label": "npm run build", "memory_mb": 512, "external": "Terminal"},
            {"label": "rustc", "memory_mb": 300, "external": ""},
        ],
        "panes": [
            {"pane": 3, "tab": "work", "role": "worker", "memory_mb": 2048, "agent_mb": 1024, "heavy_mb": 1229, "agent": "claude"},
            {"pane": 4, "memory_mb": 300, "agent_mb": 0, "heavy_mb": 0},
        ],
        "external": {"memory_mb": 3072, "processes": 12, "heavy": 1},
        "scan": {"processes": 400, "readable": 390, "age_ms": 1500},
        "queue": {
            "arbiter": true,
            "role": "explicit",
            "headless": false,
            "limits": {"max_jobs": 3, "reserve_gb": 3.6},
            "running": [
                {"id": 4242, "label": "cargo build", "pane": 3, "share": 6, "memory_mb": 1229, "peak_mb": 2048,
                 "estimate_mb": 3584, "pending_mb": 1536, "stalled": false, "cancel": false, "ran_s": 65,
                 "progress": {"done": 120, "total": 345},
                 "eta": {"state": "remaining", "remaining_s": 240, "previous_s": null, "from": "progress",
                         "progress": {"done": 120, "total": 345}, "text": "あと約 4 分（進み 120/345 からの見込み）"}},
                {"id": 4241, "label": "cargo test", "share": 6, "memory_mb": 512, "ran_s": 720, "stalled": true, "cancel": true,
                 "eta": {"state": "overrun", "remaining_s": null, "previous_s": 600, "from": "history", "progress": null,
                         "text": "前回より長くかかっている（経過 12 分・前回 10 分）"}},
                {"id": 4240, "label": "cargo clippy", "pane": 4, "share": 6, "ran_s": 30,
                 "eta": {"state": "remaining", "remaining_s": 540, "previous_s": 600, "from": "history",
                         "text": "あと約 9 分（前回 10 分）"}},
                {"id": 4239, "label": "cargo doc", "share": 2, "ran_s": 45,
                 "eta": {"state": "unknown", "text": "経過 45 秒（見込みなし）"}},
            ],
            "waiting": [
                {"id": 4243, "position": 1, "label": "cargo test", "pane": 3, "waited_s": 75, "reason": "memory",
                 "text": "空きメモリ不足（空き 1.0 GB・必要 3.5 GB・予約 3.6 GB）", "start_s": 0,
                 "start_text": "空き次第すぐ開始の見込み", "priority": "high", "priority_from": "pane"},
                {"id": 4244, "position": 2, "label": "cargo build --release", "waited_s": 3700, "reason": "ahead",
                 "text": "前に並んでいる処理の後", "start_s": 1140, "start_text": "開始まで約 19 分",
                 "priority": "high", "priority_from": "job", "aged": true, "base_priority": "normal"},
                {"id": 4245, "position": 3, "label": "cargo bench", "waited_s": 5, "reason": "target_dir",
                 "text": "同じ target dir を使う処理が走っている", "priority": "low", "priority_from": "role"},
            ],
            "unmanaged": 1,
            "recent": [
                {"id": 4238, "label": "cargo fmt", "exit_code": 0, "ran_s": 125, "peak_mb": 2048},
                {"id": 4237, "label": "cargo test --workspace", "gave_up": true, "waited_s": 7200, "reason": "max_jobs",
                 "text": "同時数の上限（実行 3 + tako の外 1 / 上限 3）", "holders": holders()},
            ],
        },
        "launches": {
            "mode": "on",
            "waiting": [{"position": 1, "pane": 5, "kind": "worker", "label": "w-2365", "priority": "normal",
                         "waited_s": 30, "code": "pressure_warn",
                         "reason": "メモリ圧 warn（normal へ戻るまで agent を起こさない）"}],
            "starting": [{"pane": 6, "kind": "master", "label": "m", "since_s": 4, "memory_mb": 300}],
        },
        "jobserver": {"state": "active", "free": 10, "tokens": 18, "holders": 3,
                      "last_recount": {"free_before": 16, "added": 2, "removed": 0, "why": "idle_short"}},
    })
}

/// 調停役でない・機の状態を採れない・先行の tako の札を使う
fn status_bare() -> Value {
    json!({
        "gui": true,
        "machine": null,
        "machine_unavailable": "この OS ではまだ採れない（Windows は #2000 で対応）",
        "heavy": [],
        "queue": {"arbiter": false, "reason": "この tako は順番待ちの調停役ではない"},
        "launches": {"mode": "off", "reason": "この tako は起動待ちを持たない"},
        "jobserver": {"state": "follower", "tokens": 18},
    })
}

/// 待ちごとの理由を返さない旧い GUI（先頭の理由だけ）・wrapper が消えた直近・札を配らない（設定）
fn status_old_gui() -> Value {
    json!({
        "gui": true,
        "machine": {"memory_mb": {"used": 6000, "total": 8192, "app": 3000, "wired": 1000, "compressed": 500,
                                  "available": 2192}, "cpus": 8},
        "heavy": [],
        "queue": {
            "arbiter": true,
            "limits": {"max_jobs": 2, "reserve_gb": 2},
            "running": [{"id": 1, "label": "cargo build", "share": 9, "memory_mb": 800, "ran_s": 3}],
            "waiting": [{"id": 2, "position": 1, "label": "cargo test", "waited_s": 61, "priority": "normal"}],
            "blocked": {"reason": "max_jobs", "text": "同時数の上限（実行 1 / 上限 2）"},
            "recent": [{"id": 0, "label": "cargo check", "lost": true, "ran_s": 3}],
        },
        "jobserver": {"state": "off", "code": "setting",
                      "reason": "設定 resources.jobserver が off（`tako queue limits --jobserver on` で戻す）"},
    })
}

fn queue_cases() -> Vec<Case> {
    let mut v = vec![
        case("q_status", &["queue"], Arbiter::Reply(status_full())),
        case("q_bare", &["queue"], Arbiter::Reply(status_bare())),
        case("q_old_gui", &["queue"], Arbiter::Reply(status_old_gui())),
        case(
            "q_cancel_wait",
            &["queue", "cancel", "4243"],
            Arbiter::Reply(json!({"id": 4243, "label": "cargo test", "phase": "waiting"})),
        ),
        case(
            "q_cancel_run",
            &["queue", "cancel", "4242"],
            Arbiter::Reply(json!({"id": 4242, "label": "cargo build", "phase": "running"})),
        ),
        case("q_cancel_off", &["queue", "cancel", "4243"], Arbiter::None),
        case(
            "q_prio_job",
            &["queue", "priority", "4244", "top"],
            Arbiter::Reply(
                json!({"id": 4244, "label": "cargo build --release", "priority": "top",
                                  "priority_from": "job", "position": 1}),
            ),
        ),
        case(
            "q_prio_run",
            &["queue", "priority", "4242", "low"],
            Arbiter::Reply(
                json!({"id": 4242, "label": "cargo build", "priority": "low",
                                  "priority_from": "job",
                                  "note": "走っている処理は止めない（優先度は待ちの並びにだけ効く）"}),
            ),
        ),
        case(
            "q_prio_pane",
            &["queue", "priority", "--pane", "3", "high"],
            Arbiter::Reply(json!({"pane": 3, "priority": "high", "jobs": [
                {"id": 4243, "label": "cargo test", "phase": "waiting", "priority": "high", "priority_from": "pane"},
                {"id": 4242, "label": "cargo build", "phase": "running", "priority": "high", "priority_from": "pane"},
            ]})),
        ),
        case(
            "q_start_now",
            &["queue", "start", "5"],
            Arbiter::Reply(json!({"pane": 5, "state": "started", "waited_s": 75,
                                  "was": {"code": "pressure_warn",
                                          "reason": "メモリ圧 warn（normal へ戻るまで agent を起こさない）"}})),
        ),
        case(
            "q_start_done",
            &["queue", "start", "5"],
            Arbiter::Reply(json!({"pane": 5, "state": "already_started"})),
        ),
        case(
            "q_limits",
            &["queue", "limits"],
            Arbiter::Reply(
                json!({"max_jobs": 3, "reserve_gb": 3.6, "swap_guard_gb": 4, "max_wait_min": 120,
                                  "jobserver": true, "configured": {}}),
            ),
        ),
        case(
            "q_limits_set",
            &["queue", "limits", "--max-jobs", "2"],
            Arbiter::Reply(
                json!({"max_jobs": 2, "reserve_gb": 3.6, "swap_guard_gb": 4, "max_wait_min": 120,
                                  "configured": {"max_jobs": 2}}),
            ),
        ),
        Case {
            json: true,
            ..case(
                "q_json",
                &["queue", "--json"],
                Arbiter::Reply(status_full()),
            )
        },
    ];
    // 届かない（誰も居ない口）・応答しない（黙る口）: 機の状態はこのプロセスで採る（数値を正規化）
    v.push(Case {
        norm: Norm::Machine,
        ..case("q_offline", &["queue"], Arbiter::None)
    });
    v.push(Case {
        norm: Norm::Machine,
        extra: vec![("TAKO_IPC_TIMEOUT_SECS", "1")],
        ..case("q_silent", &["queue"], Arbiter::Silent)
    });
    v.push(Case {
        norm: Norm::Machine,
        json: true,
        ..case("q_offline_json", &["queue", "--json"], Arbiter::None)
    });
    v
}

/// `tako queue <コマンド>`（順番待ちで走らせる）の進みの行（stderr）。調停役の文（待つ理由・開始の見込み）は
/// 応答の値のまま = 日本語、英語の表示は同じ応答の構造の欄から組む。待った秒は実行のたびに揺れうるので
/// 数字を正規化する（数字まで同じことは `queue_wrapper` の単体テストで見る）。子はすぐ終わる `true` か、
/// 実行中の取り消しで止められる `sleep`
fn queue_run_cases() -> Vec<Case> {
    let run = |name, args: &[&'static str], arbiter| Case {
        norm: Norm::Machine,
        ..case(name, args, arbiter)
    };
    let max_jobs = json!({
        "state": "waiting", "position": 1, "waiting": 1, "running": 1, "max_jobs": 1, "unmanaged": 1,
        "reason": "max_jobs", "text": "同時数の上限（実行 1 + tako の外 1 / 上限 1）",
        "start_s": 0, "start_text": "空き次第すぐ開始の見込み",
    });
    let ahead = json!({
        "state": "waiting", "position": 2, "waiting": 2, "running": 1, "max_jobs": 1, "unmanaged": 0,
        "reason": "ahead", "text": "前に並んでいる処理の後",
        "head_reason": "memory", "head_text": "空きメモリ不足（空き 1.0 GB・必要 3.5 GB・予約 3.6 GB）",
        "start_s": 240, "start_text": "開始まで約 4 分",
    });
    let memory = json!({
        "state": "waiting", "position": 1, "waiting": 2, "running": 1, "max_jobs": 2, "unmanaged": 0,
        "reason": "memory", "text": "空きメモリ不足（空き 1.0 GB・必要 3.5 GB・予約 3.6 GB）",
        "holders": holders(),
    });
    let granted = |share: u64| json!({"state": "granted", "share": share});
    let mut silent = run("r_silent", &["queue", "true"], Arbiter::Silent);
    silent.extra = vec![("TAKO_IPC_TIMEOUT_SECS", "1")];
    vec![
        run(
            "r_wait_start",
            &["queue", "true"],
            Arbiter::Script(vec![max_jobs, granted(4)]),
        ),
        run(
            "r_ahead",
            &["queue", "true"],
            Arbiter::Script(vec![ahead, granted(0)]),
        ),
        run(
            "r_gave_up",
            &["queue", "--max-wait", "1s", "true"],
            Arbiter::Reply(memory),
        ),
        run(
            "r_cancelled",
            &["queue", "true"],
            Arbiter::Reply(json!({"state": "cancelled"})),
        ),
        run("r_offline", &["queue", "true"], Arbiter::None),
        silent,
        run(
            "r_rejected",
            &["queue", "true"],
            Arbiter::Refuse("unknown queue action"),
        ),
        run(
            "r_spawn_failed",
            &["queue", "/nonexistent-2365q/cmd"],
            Arbiter::Reply(granted(2)),
        ),
        run(
            "r_cancel_running",
            &["queue", "sleep", "30"],
            Arbiter::Script(vec![granted(2), json!({"state": "running", "cancel": true})]),
        ),
    ]
}

/// usage: 会話記録を置く前（観測も使用量も無い）
fn usage_empty_cases() -> Vec<Case> {
    vec![Case {
        norm: Norm::Usage,
        ..case("u_empty", &["usage"], Arbiter::None)
    }]
}

/// usage: 会話記録と観測を置いた後
fn usage_cases() -> Vec<Case> {
    let u = |name, args: &[&'static str]| Case {
        norm: Norm::Usage,
        ..case(name, args, Arbiter::None)
    };
    vec![
        u("u_full", &["usage"]),
        u("u_issue", &["usage", "--by", "issue"]),
        u("u_month", &["usage", "--window", "month"]),
        u("u_5h", &["usage", "--window", "5h"]),
        Case {
            extra: vec![("TAKO_2192_S1_LEGACY", "1")],
            ..u("u_s1_legacy", &["usage"])
        },
        Case {
            extra: vec![("TAKO_2192_S2_LEGACY", "1")],
            ..u("u_s2_legacy", &["usage"])
        },
        Case {
            json: true,
            ..u("u_json", &["usage", "--json"])
        },
    ]
}

/// 1 言語分を全部走らせる（usage は見本を置く前と後）
fn run_lang(lang: &str) -> Vec<(&'static str, bool, Out)> {
    let sb = Sandbox::new(lang);
    let mut outs = Vec::new();
    for c in queue_cases()
        .iter()
        .chain(queue_run_cases().iter())
        .chain(usage_empty_cases().iter())
    {
        outs.push((c.name, c.json, sb.run(c)));
    }
    sb.seed_usage();
    for c in usage_cases() {
        outs.push((c.name, c.json, sb.run(&c)));
    }
    outs
}

type Runs = (
    Vec<(&'static str, bool, Out)>,
    Vec<(&'static str, bool, Out)>,
);

/// 日英の実行（2 つのテストで共有。日英は別の砂場で並行に走らせる）
fn runs() -> &'static Runs {
    static RUNS: OnceLock<Runs> = OnceLock::new();
    RUNS.get_or_init(|| {
        let ja = std::thread::spawn(|| run_lang("ja"));
        let en = run_lang("en");
        let ja = ja.join().expect("日本語の実行が終わる");
        if let Some(dir) = std::env::var_os("I2365Q_DUMP") {
            let dir = PathBuf::from(dir);
            for (lang, outs) in [("ja", &ja), ("en", &en)] {
                for (name, _, out) in outs {
                    let d = dir.join(lang);
                    std::fs::create_dir_all(&d).expect("書き出し先を作れる");
                    std::fs::write(
                        d.join(format!("{name}.txt")),
                        format!(
                            "code={}\n--- stdout\n{}--- stderr\n{}",
                            out.code, out.stdout, out.stderr
                        ),
                    )
                    .expect("書き出せる");
                }
            }
        }
        (ja, en)
    })
}

// ---------------------------------------------------------------------------
// #2365 より前の日本語の出力（時刻・ログインの鍵・機の数値は正規化済み）
// ---------------------------------------------------------------------------

/// (ケース, 終了コード, stdout, stderr)。修正前の `tako` の書き出し（`I2365Q_DUMP`）を写したもの。
/// 届かない・応答しない（`q_offline` / `q_silent`）は機の状態の行が実行環境で変わるので先頭の 1 行だけ、
/// 月の暦の期間（`u_month`）は月の変わり目で使用量が入れ替わるので載せない（英語の検査だけ）
fn old_ja() -> Vec<(&'static str, i32, &'static str, &'static str)> {
    vec![
        (
            "q_status",
            0,
            r##"メモリ 20.1 GB / 36.0 GB（app 10.0 GB・wired 3.0 GB・圧縮 2.0 GB・空き 15.9 GB）  swap 1.0 GB / 2.0 GB  メモリ圧 normal
CPU 35%（18 コア）  load 3.1 / 2.5 / 2.0
重い処理 3 件:
  cargo build  1.2 GB  250%  ペイン 3 · work · worker
  npm run build  512 MB  -  tako の外（Terminal）
  rustc  300 MB  -  tako の外
ペインの上位（メモリの多い順）:
  2.0 GB（agent 1.0 GB・重い処理 1.2 GB）  ペイン 3 · work · worker  claude
  300 MB（agent 0 MB・重い処理 0 MB）  ペイン 4
tako の外: 3.0 GB・12 本（重い処理 1 件）
走査: 400 本（読めた 390）・1.5 秒前
順番待ち: 実行 4 / 上限 3・待ち 3（予約 3.6 GB）・順番待ちを通っていない重い処理 1
  実行  4242  cargo build  ペイン 3  1 分 5 秒  120/345  1.2 GB（最大 2.0 GB）  まだ使っていない見込み 1.5 GB（見込み 3.5 GB）  取り分 6  — あと約 4 分（進み 120/345 からの見込み）
  実行  4241  cargo test  12 分 0 秒  512 MB  取り分 6  [停滞（枠を返した）・取り消し中]  — 前回より長くかかっている（経過 12 分・前回 10 分）
  実行  4240  cargo clippy  ペイン 4  30 秒  取り分 6  — あと約 9 分（前回 10 分）
  実行  4239  cargo doc  45 秒  取り分 2  — 経過 45 秒（見込みなし）
  待ち 1  4243  cargo test  ペイン 3  high（ペインの既定）  待ち 1 分 15 秒・空き次第すぐ開始の見込み  — 空きメモリ不足（空き 1.0 GB・必要 3.5 GB・予約 3.6 GB）
  待ち 2  4244  cargo build --release  high（処理に付けた段 normal から待ちで上がった）  待ち 1 時間 1 分・開始まで約 19 分  — 前に並んでいる処理の後
  待ち 3  4245  cargo bench  low（役割の既定）  待ち 5 秒  — 同じ target dir を使う処理が走っている
  直近: 4238  cargo fmt  終了 0（2 分 5 秒・最大 2.0 GB）
  直近の諦め: 4237  cargo test --workspace  待ち 2 時間 0 分  — 同時数の上限（実行 3 + tako の外 1 / 上限 3）
    押さえていた処理: 4240 cargo clippy（使用 1.2 GB・最大 2.0 GB・まだ使っていない見込み 1.5 GB・見込み 3.5 GB（過去の記録）・ピーク後・停滞）
    押さえていた処理: 4239 cargo doc（使用 未計測・見込み 2.0 GB（同じ形の記録））
agent の起動待ち: 待ち 1・立ち上がり中 1（逼迫が解けると起きる。今すぐ起こすには tako queue start <ペイン>）
  待ち 1  ペイン 5  worker  w-2365  normal  待ち 30 秒  — メモリ圧 warn（normal へ戻るまで agent を起こさない）
  立ち上がり中  ペイン 6  master  m  4秒  300 MB
cargo の札: 空き 10 / 18（札を持ちうる cargo 系 3 本）
  直近の数え直し: 空き 16 → +2（誰も持っていないのに足りない分を足した）
"##,
            r##""##,
        ),
        (
            "q_bare",
            0,
            r##"機の状態: この OS ではまだ採れない（Windows は #2000 で対応）
重い処理: なし
順番待ち: この tako は順番待ちの調停役ではない
cargo の札: 先行の tako の札を使う（総数 18）
"##,
            r##""##,
        ),
        (
            "q_old_gui",
            0,
            r##"メモリ 5.9 GB / 8.0 GB（app 2.9 GB・wired 1000 MB・圧縮 500 MB・空き 2.1 GB）
CPU ?（8 コア）
重い処理: なし
順番待ち: 実行 1 / 上限 2・待ち 1（予約 2 GB）
  実行  1  cargo build  3 秒  800 MB  取り分 9
  待ち 1  2  cargo test  normal  待ち 1 分 1 秒
  先頭が待つ理由: 同時数の上限（実行 1 / 上限 2）
  直近: 0  cargo check  wrapper が消えた（3 秒）
cargo の札: 配っていない — 設定 resources.jobserver が off（`tako queue limits --jobserver on` で戻す）
"##,
            r##""##,
        ),
        (
            "q_cancel_wait",
            0,
            r##"取り消した: 4243 cargo test — 待ちから外した（その tako queue は 125 で終わる）
"##,
            r##""##,
        ),
        (
            "q_cancel_run",
            0,
            r##"取り消した: 4242 cargo build — 子を止める（SIGTERM → 10 秒で SIGKILL）
"##,
            r##""##,
        ),
        (
            "q_cancel_off",
            1,
            r##""##,
            r##"error: 順番待ちは無効（tako が起動していない）
"##,
        ),
        (
            "q_prio_job",
            0,
            r##"優先度: 4244 cargo build --release → top（処理に付けた段）・待ち 1 番目
"##,
            r##""##,
        ),
        (
            "q_prio_run",
            0,
            r##"優先度: 4242 cargo build → low（処理に付けた段）
  走っている処理は止めない（優先度は待ちの並びにだけ効く）
"##,
            r##""##,
        ),
        (
            "q_prio_pane",
            0,
            r##"ペイン 3 から出る処理の優先度の既定を high にした（順番待ちの中のそのペインの処理 2 件）
  4243  cargo test  waiting  high（ペインの既定）
  4242  cargo build  running  high（ペインの既定）
"##,
            r##""##,
        ),
        (
            "q_start_now",
            0,
            r##"ペイン 5 の agent を今すぐ起こす（待ち 1 分 15 秒・待っていた理由: メモリ圧 warn（normal へ戻るまで agent を起こさない））
"##,
            r##""##,
        ),
        (
            "q_start_done",
            0,
            r##"ペイン 5 の agent はもう起こしている
"##,
            r##""##,
        ),
        (
            "q_limits",
            0,
            r##"同時 3 件・予約 3.6 GB・swap の警戒線 4 GB・待ちの上限 120 分・cargo の札 on（すべて機に合わせた既定）
"##,
            r##""##,
        ),
        (
            "q_limits_set",
            0,
            r##"同時 2 件・予約 3.6 GB・swap の警戒線 4 GB・待ちの上限 120 分・cargo の札 -（設定: max_jobs=2）
"##,
            r##""##,
        ),
        (
            "q_offline",
            0,
            r##"順番待ちは無効（tako が起動していない）
"##,
            r##""##,
        ),
        (
            "q_silent",
            0,
            r##"順番待ちの状況は読めない（tako が 9 秒応答しない = GUI が固まっている可能性。tako queue <コマンド> は待たずに走る）
"##,
            r##""##,
        ),
        // `tako queue <コマンド>` の進みの行（stderr。待った秒は揺れうるので数字を正規化済み）
        (
            "r_wait_start",
            0,
            r##""##,
            r##"tako queue: 待ち 9 番目（待ち 9 件・実行 9/9・tako の外 9・9 秒・空き次第すぐ開始の見込み）— 同時数の上限（実行 9 + tako の外 9 / 上限 9）
tako queue: 開始（待ち 9 秒・取り分 9）
"##,
        ),
        (
            "r_ahead",
            0,
            r##""##,
            r##"tako queue: 待ち 9 番目（待ち 9 件・実行 9/9・9 秒・開始まで約 9 分）— 前に 9 件（先頭: 空きメモリ不足（空き 9.9 GB・必要 9.9 GB・予約 9.9 GB））
tako queue: 開始（待ち 9 秒）
"##,
        ),
        (
            "r_gave_up",
            75,
            r##""##,
            r##"tako queue: 待ち 9 番目（待ち 9 件・実行 9/9・9 秒）— 空きメモリ不足（空き 9.9 GB・必要 9.9 GB・予約 9.9 GB）
tako queue: 9 件待ちのまま 9 秒 たったので諦める。空いたらもう一度（`tako queue` で状況）
tako queue: 待っていた理由: 空きメモリ不足（空き 9.9 GB・必要 9.9 GB・予約 9.9 GB）
tako queue: 押さえていた処理: 9 cargo clippy（使用 9.9 GB・最大 9.9 GB・まだ使っていない見込み 9.9 GB・見込み 9.9 GB（過去の記録）・ピーク後・停滞）
tako queue: 押さえていた処理: 9 cargo doc（使用 未計測・見込み 9.9 GB（同じ形の記録））
"##,
        ),
        (
            "r_cancelled",
            125,
            r##""##,
            r##"tako queue: 待っている間に取り消された
"##,
        ),
        (
            "r_offline",
            0,
            r##""##,
            r##"tako queue: tako が起動していないので、順番待ちせずに走らせる
"##,
        ),
        (
            "r_silent",
            0,
            r##""##,
            r##"tako queue: tako が 9 秒応答しないので、順番待ちせずに走らせる（GUI が固まっている可能性）
"##,
        ),
        (
            "r_rejected",
            0,
            r##""##,
            r##"tako queue: 順番待ちを受けていない（unknown queue action）ので、そのまま走らせる
"##,
        ),
        (
            "r_spawn_failed",
            127,
            r##""##,
            r##"tako queue: /nonexistent-9q/cmd を起動できない: No such file or directory (os error 9)
"##,
        ),
        (
            "r_cancel_running",
            143,
            r##""##,
            r##"tako queue: 取り消されたので子を止める（SIGTERM → 9 秒で SIGKILL）
"##,
        ),
        (
            "u_empty",
            0,
            r##"利用制限の観測（tako mod の報告と codex の rollout。% か解除時刻が変わったときだけ記録）と使用量の内訳
  まだ観測が無い。tako mod の効いた claude のペインが利用制限を報告すると記録が始まる（mod の状態は tako mod）
"##,
            r##""##,
        ),
        (
            "u_full",
            0,
            r##"利用制限の観測（tako mod の報告と codex の rollout。% か解除時刻が変わったときだけ記録）と使用量の内訳
  ログイン A（鍵 <LOGIN>）  観測 9 点
    9d    9%  解除 <T>  最新の観測 <T>（9 点）
    今の 9d の窓（<T> 〜・解除 <T>）  API 換算 $9.9・トークン 9.9 万（出力 9.9 万）・応答 9・会話 9・単価不明 9.9 万
          9d            9h      （% は今の窓のうち各段が使った分・+ は推定）
       9.9% +9.9        -        未分類  API 換算 $9.9・トークン 9.9 万（出力 9.9 万）・応答 9・会話 9・単価不明 9.9 万
        9.9%             -        tako の外（区間にローカルの使用量が無いのに増えた分）
        +9.9             -        推定の合計（観測の区間の外 = 最後の観測より後・観測の無い窓）
    9d の按分: 今の窓 9.9% = 段 9.9 + tako の外 9.9・推定 +9.9・9% あたり API 換算 $9.9（区間 9 本・直近の窓の四分位 $9.9〜$9.9）
    9h の按分: このログインの観測が無いので % を出していない
  記録 9 点（<T> 〜 <T>）
  読めない行 9 行を飛ばした（書きかけ・壊れた行）
  注: api_usd は API 換算（モデル別の単価 × 9 成分。単価表 9-9-9）で、サブスクリプションの請求額ではない
  注: 単価表に無いモデル（claude-sonnet-9-9）は API 換算に含めず unpriced_tokens に数えた
  注: limit_share は観測した % の増分を、観測点の間の区間の使用量（API 換算）の比で段へ配った値。範囲は枠（9h / 9d）ごとの今の窓の、期間に重なる分。今の窓を丸ごと見るときは段の合計 + tako の外 = 最新の観測
  注: outside（tako の外）は区間にローカルの使用量が無いのに % が増えた分（claude.ai・別の PC など）。estimated（推定）は観測の区間の外（最後の観測より後・観測の無い窓）の使用量を、直近 9 日の区間の 9% あたりの API 換算の中央値で割った値
  注: 観測の無いログインと枠は % を出していない: ログイン A（9h）。tako mod の効いた claude のペインが利用制限を報告すると記録が始まる
"##,
            r##""##,
        ),
        (
            "u_issue",
            0,
            r##"利用制限の観測（tako mod の報告と codex の rollout。% か解除時刻が変わったときだけ記録）と使用量の内訳
  ログイン A（鍵 <LOGIN>）  観測 9 点
    9d    9%  解除 <T>  最新の観測 <T>（9 点）
    今の 9d の窓（<T> 〜・解除 <T>）  API 換算 $9.9・トークン 9.9 万（出力 9.9 万）・応答 9・会話 9・単価不明 9.9 万
          9d            9h      （% は今の窓のうち各段が使った分・+ は推定）
       9.9% +9.9        -        未分類 Issue なし  API 換算 $9.9・トークン 9.9 万（出力 9.9 万）・応答 9・会話 9・単価不明 9.9 万
        9.9%             -        tako の外（区間にローカルの使用量が無いのに増えた分）
        +9.9             -        推定の合計（観測の区間の外 = 最後の観測より後・観測の無い窓）
    9d の按分: 今の窓 9.9% = 段 9.9 + tako の外 9.9・推定 +9.9・9% あたり API 換算 $9.9（区間 9 本・直近の窓の四分位 $9.9〜$9.9）
    9h の按分: このログインの観測が無いので % を出していない
  記録 9 点（<T> 〜 <T>）
  読めない行 9 行を飛ばした（書きかけ・壊れた行）
  段: issue（既定は project）
  注: api_usd は API 換算（モデル別の単価 × 9 成分。単価表 9-9-9）で、サブスクリプションの請求額ではない
  注: 単価表に無いモデル（claude-sonnet-9-9）は API 換算に含めず unpriced_tokens に数えた
  注: limit_share は観測した % の増分を、観測点の間の区間の使用量（API 換算）の比で段へ配った値。範囲は枠（9h / 9d）ごとの今の窓の、期間に重なる分。今の窓を丸ごと見るときは段の合計 + tako の外 = 最新の観測
  注: outside（tako の外）は区間にローカルの使用量が無いのに % が増えた分（claude.ai・別の PC など）。estimated（推定）は観測の区間の外（最後の観測より後・観測の無い窓）の使用量を、直近 9 日の区間の 9% あたりの API 換算の中央値で割った値
  注: 観測の無いログインと枠は % を出していない: ログイン A（9h）。tako mod の効いた claude のペインが利用制限を報告すると記録が始まる
"##,
            r##""##,
        ),
        (
            "u_5h",
            0,
            r##"利用制限の観測（tako mod の報告と codex の rollout。% か解除時刻が変わったときだけ記録）と使用量の内訳
  ログイン A（鍵 <LOGIN>）  観測 9 点
    9d    9%  解除 <T>  最新の観測 <T>（9 点）
    直近 9 時間（観測した今の窓が無い）  API 換算 $9.9・トークン 9.9 万（出力 9.9 万）・応答 9・会話 9・単価不明 9.9 万
          9h            9d      （% は今の窓のうち各段が使った分・+ は推定）
           -         9.9% +9.9   未分類  API 換算 $9.9・トークン 9.9 万（出力 9.9 万）・応答 9・会話 9・単価不明 9.9 万
           -          9.9%        tako の外（区間にローカルの使用量が無いのに増えた分）
           -          +9.9        推定の合計（観測の区間の外 = 最後の観測より後・観測の無い窓）
    9h の按分: このログインの観測が無いので % を出していない
    9d の按分: 今の窓 9.9% = 段 9.9 + tako の外 9.9・推定 +9.9・9% あたり API 換算 $9.9（区間 9 本・直近の窓の四分位 $9.9〜$9.9）
  記録 9 点（<T> 〜 <T>）
  読めない行 9 行を飛ばした（書きかけ・壊れた行）
  注: api_usd は API 換算（モデル別の単価 × 9 成分。単価表 9-9-9）で、サブスクリプションの請求額ではない
  注: 単価表に無いモデル（claude-sonnet-9-9）は API 換算に含めず unpriced_tokens に数えた
  注: limit_share は観測した % の増分を、観測点の間の区間の使用量（API 換算）の比で段へ配った値。範囲は枠（9h / 9d）ごとの今の窓の、期間に重なる分。今の窓を丸ごと見るときは段の合計 + tako の外 = 最新の観測
  注: outside（tako の外）は区間にローカルの使用量が無いのに % が増えた分（claude.ai・別の PC など）。estimated（推定）は観測の区間の外（最後の観測より後・観測の無い窓）の使用量を、直近 9 日の区間の 9% あたりの API 換算の中央値で割った値
  注: 観測の無いログインと枠は % を出していない: ログイン A（9h）。tako mod の効いた claude のペインが利用制限を報告すると記録が始まる
"##,
            r##""##,
        ),
        (
            "u_s1_legacy",
            0,
            r##"利用制限の観測（tako mod の報告と codex の rollout。% か解除時刻が変わったときだけ記録）と使用量の内訳
  記録していない（TAKO_9_S9_LEGACY が立っている）
  ログイン A（鍵 <LOGIN>）  観測 9 点
    9d    9%  解除 <T>  最新の観測 <T>（9 点）
    今の 9d の窓（<T> 〜・解除 <T>）  API 換算 $9.9・トークン 9.9 万（出力 9.9 万）・応答 9・会話 9・単価不明 9.9 万
          9d            9h      （% は今の窓のうち各段が使った分・+ は推定）
       9.9% +9.9        -        未分類  API 換算 $9.9・トークン 9.9 万（出力 9.9 万）・応答 9・会話 9・単価不明 9.9 万
        9.9%             -        tako の外（区間にローカルの使用量が無いのに増えた分）
        +9.9             -        推定の合計（観測の区間の外 = 最後の観測より後・観測の無い窓）
    9d の按分: 今の窓 9.9% = 段 9.9 + tako の外 9.9・推定 +9.9・9% あたり API 換算 $9.9（区間 9 本・直近の窓の四分位 $9.9〜$9.9）
    9h の按分: このログインの観測が無いので % を出していない
  記録 9 点（<T> 〜 <T>）
  読めない行 9 行を飛ばした（書きかけ・壊れた行）
  注: api_usd は API 換算（モデル別の単価 × 9 成分。単価表 9-9-9）で、サブスクリプションの請求額ではない
  注: 単価表に無いモデル（claude-sonnet-9-9）は API 換算に含めず unpriced_tokens に数えた
  注: limit_share は観測した % の増分を、観測点の間の区間の使用量（API 換算）の比で段へ配った値。範囲は枠（9h / 9d）ごとの今の窓の、期間に重なる分。今の窓を丸ごと見るときは段の合計 + tako の外 = 最新の観測
  注: outside（tako の外）は区間にローカルの使用量が無いのに % が増えた分（claude.ai・別の PC など）。estimated（推定）は観測の区間の外（最後の観測より後・観測の無い窓）の使用量を、直近 9 日の区間の 9% あたりの API 換算の中央値で割った値
  注: 観測の無いログインと枠は % を出していない: ログイン A（9h）。tako mod の効いた claude のペインが利用制限を報告すると記録が始まる
"##,
            r##""##,
        ),
        (
            "u_s2_legacy",
            0,
            r##"利用制限の観測（tako mod の報告と codex の rollout。% か解除時刻が変わったときだけ記録）と使用量の内訳
  ログイン A（鍵 <LOGIN>）  観測 9 点・会話 9 本
    9d    9%  解除 <T>  最新の観測 <T>（9 点）
  記録 9 点（<T> 〜 <T>）
  読めない行 9 行を飛ばした（書きかけ・壊れた行）
"##,
            r##""##,
        ),
    ]
}

#[test]
fn queueとusageの人向けの表示は表示言語に従い日本語は2365より前と同じ() {
    let (ja, en) = runs();
    let mut failures = Vec::new();
    assert_eq!(ja.len(), en.len());
    for ((name, json, j), (_, _, e)) in ja.iter().zip(en.iter()) {
        if j.code != e.code {
            failures.push(format!(
                "{name}: 終了コードが日英で違う（{} / {}）",
                j.code, e.code
            ));
        }
        if *json {
            // `--json` は応答の値のまま = 日英で同じ（届かないときの `note` も日本語）。索引の読み進めの
            // 記録（経過ミリ秒）と、届かないときにこのプロセスが採る機の状態（日英の 2 回の実行の間で
            // メモリ圧の語や swap の有無が変わりうる）・届かなかった理由（接続の失敗の文 = #2365 より前から
            // 表示言語）は外して比べる
            let strip = |s: &str| {
                let mut v: Value = serde_json::from_str(s).unwrap_or(Value::Null);
                if let Some(index) = v.get_mut("index") {
                    index["update"] = Value::Null;
                }
                if v["gui"] == false {
                    v["reason"] = Value::Null;
                    v["machine"] = Value::Null;
                }
                v
            };
            if strip(&j.stdout) != strip(&e.stdout) || strip(&j.stdout).is_null() {
                failures.push(format!(
                    "{name}: --json が日英で違う\n  ja: {}\n  en: {}",
                    j.stdout, e.stdout
                ));
            }
            // 届かないときの `note` は英語表示でも #2365 より前の日本語のまま
            let note =
                serde_json::from_str::<Value>(&e.stdout).unwrap_or(Value::Null)["note"].clone();
            if *name == "q_offline_json" && note != "順番待ちは無効（tako が起動していない）"
            {
                failures.push(format!(
                    "{name}: 英語表示の --json の note が日本語のままでない: {note}"
                ));
            }
            continue;
        }
        for (stream, text) in [("stdout", &e.stdout), ("stderr", &e.stderr)] {
            if let Some(c) = has_japanese(text) {
                failures.push(format!(
                    "{name}: 英語表示の {stream} に日本語（{c}）が混ざる:\n{text}"
                ));
            }
        }
        if e.stdout.trim().is_empty() && e.stderr.trim().is_empty() {
            failures.push(format!("{name}: 英語表示の出力が空"));
        }
    }
    for (name, code, stdout, stderr) in old_ja() {
        let Some((_, _, j)) = ja.iter().find(|(n, _, _)| *n == name) else {
            failures.push(format!("{name}: ケースが無い"));
            continue;
        };
        let got = if matches!(name, "q_offline" | "q_silent") {
            j.stdout
                .lines()
                .next()
                .map(|l| format!("{l}\n"))
                .unwrap_or_default()
        } else {
            j.stdout.clone()
        };
        if j.code != code || got != stdout || j.stderr != stderr {
            failures.push(format!(
                "{name}: 日本語表示が #2365 より前と違う\n--- want (code={code})\n{stdout}--- stderr\n{stderr}\
                 --- got (code={})\n{}--- stderr\n{}",
                j.code, j.stdout, j.stderr
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} 件:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// 英語表示でも行の数と字下げ（節の入れ子）は日本語表示と同じ（文だけが入れ替わる。`--json` は除く）
#[test]
fn 英語表示の行の数と字下げは日本語と同じ() {
    let (ja, en) = runs();
    let shape = |s: &str| -> Vec<usize> {
        s.lines()
            .map(|l| l.len() - l.trim_start_matches(' ').len())
            .collect()
    };
    let mut failures = Vec::new();
    for ((name, json, j), (_, _, e)) in ja.iter().zip(en.iter()) {
        if *json {
            continue;
        }
        for (stream, a, b) in [
            ("stdout", &j.stdout, &e.stdout),
            ("stderr", &j.stderr, &e.stderr),
        ] {
            if shape(a) != shape(b) {
                failures.push(format!(
                    "{name}: {stream} の行の数か字下げが日英で違う\n--- ja\n{a}--- en\n{b}"
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} 件:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}
