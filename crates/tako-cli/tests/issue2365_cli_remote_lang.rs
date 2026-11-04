//! 実経路: `tako remote start` / `stop` / `status` の人向けの表示（stderr の行）が表示言語に従う
//! （Issue #2365 の続き）。
//!
//! 修正前（実測）: 英語表示でも start は「QR コードを生成しました: …」「スマホでスキャンしてください。」
//! 「この URL は恒久固定で secret を含みません（…）。」ほか 7 行、入れ替えたときは「旧版のまま動いていた
//! リモート daemon（PID …）を止めて、新版で起動し直しました。」、stop は「リモートサーバーを停止しました」、
//! status は「前回の起動状態へ自動で戻せませんでした（N 回試行）: …」「  次の一手: …」「リモート公開が
//! 機能していません: …」と、旧版・実行ファイルの消失・起こしている途中の説明が日本語の固定文のまま出た
//! （`main.rs` の直書きと、応答の値の `serve_generation.detail` / `starting.detail` / `degraded` をそのまま出す形）。
//!
//! **ビルドした `tako` をそのまま**叩く（どれも GUI の要らない手元の処理）。見ること:
//! - 英語表示: stderr に日本語（かな・漢字・全角の記号）が 1 字も無い（daemon が自己検査で書く理由は
//!   中身が外から来る文なので、そのケースは ASCII の見本 `R` / `N` を置く）
//! - 日本語表示: #2365 より前の出力とバイト一致（数字を `0` に均し、QR の名前の乱数と砂場の dir を伏せる）
//! - stdout の JSON（応答の値）は日英で同じ = `detail` / `degraded` / `warnings` は日本語のまま
//! - 行の数と終了コードは日英で同じ
//!
//! status は state の置き場に記録（PID ファイル・自己検査の結果・自動復帰の記録・起こしている途中の印）を
//! 置いて各分岐を出す。生きている pid はこのテストが起こした `sleep`。start / stop と旧版の入れ替えは
//! 本物の daemon を起こす（偽の tailscale・`TAKO_REMOTE_TEST_MODE=1` = serve を張らない・置き場は砂場。
//! 実行ファイルはこのテストの複製で、`mv` で差し替えて install と同じ「同じパスの別のファイル」にする）。
//! tmux の無い環境は daemon が起動を拒むので、その部分だけ走らせない。
//!
//! A/B: `TAKO_2365_HUMAN_BIN=<修正前の tako>` で別のバイナリを叩ける（修正前は英語表示の検査で落ちる）。
//! `I2365R_DUMP=<dir>` を付けると全ケースの出力を書き出す（修正前後の `diff -r` 用）。
//! 本番の data dir・GUI・tailscale へ届かないよう、`TAKO_*` を全部外して一時の置き場だけを渡す。
//!
//! unix だけで走らせる（pid の生死・ctime の判定・偽の tailscale が sh のスクリプト）。文の日英の対は
//! OS に依らない 1 か所（`ReplyNote` / `remote_generation` / `remote_starting` / `remote_serve` の `*_in`）で、
//! 両 OS の番犬（`crates/tako-control/tests/issue1762_dispatch_error_lang.rs`）が表で見る
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tako_core::test_residue::ScratchDir;

/// 叩くバイナリ（既定はこの木でビルドした `tako`。A/B は `TAKO_2365_HUMAN_BIN`）
fn tako() -> PathBuf {
    std::env::var_os("TAKO_2365_HUMAN_BIN")
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
    \"status \"*) echo '{\"BackendState\":\"Running\",\"Self\":{\"DNSName\":\"fake-2365.tailfake.ts.net.\"}}' ;;\n\
    \"serve status\") echo '{}' ;;\n\
    *) exit 1 ;;\n\
    esac\n";

/// 1 ケース分の砂場: data（表示言語）・発見の置き場・remote の state・ホーム `hm`・偽の tailscale・
/// tmux のソケット名（daemon の分）
struct Sandbox {
    dir: ScratchDir,
    real: PathBuf,
    tmux: String,
}

impl Sandbox {
    fn new(lang: &str, name: &str) -> Self {
        let dir = ScratchDir::new("i2365r");
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
        let tmux = format!("t2365r-{}-{lang}-{name}", std::process::id());
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

/// 生きている pid の代わり（PID ファイル・起こしている途中の印の書き手）。終わるときに畳む
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

/// 起こした daemon を必ず畳む（stop が失敗した回も、覚えた pid を SIGTERM → 残れば SIGKILL）
struct Daemons<'a> {
    sb: &'a Sandbox,
    bin: PathBuf,
    pids: Vec<u32>,
}

impl Drop for Daemons<'_> {
    fn drop(&mut self) {
        if let Some(pid) = self.sb.daemon_pid() {
            self.pids.push(pid);
        }
        let _ = self.sb.run_with(&self.bin, &["remote", "stop"]);
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

// ---- status の分岐（state の置き場に記録を置く）----

/// 自動復帰が失敗した記録（GUI が書く。`detail` は GUI が記録した理由 = 中身が外から来るので ASCII の見本）
fn autostart_failed(sb: &Sandbox, _: &Alive) {
    sb.put_state(
        "tako-remote.autostart",
        r#"{"at":1,"result":"failed","attempts":5,"reason":"spawn-failed","detail":"D"}"#,
    );
}

/// 生きている daemon（PID ファイルは pid だけ = 世代は判定しない）の自己検査が止まっている
fn serve_stale(sb: &Sandbox, alive: &Alive) {
    sb.put_state("tako-remote.pid", &format!("{}\n", alive.pid()));
    sb.put_state(
        "tako-remote.serve",
        r#"{"ok":true,"checked_at":1,"state":"ok","target":"http://127.0.0.1:1"}"#,
    );
}

/// 生きている daemon が自己検査で劣化を書いた（理由と次の一手は daemon の文 = ASCII の見本）
fn serve_degraded(sb: &Sandbox, alive: &Alive) {
    sb.put_state("tako-remote.pid", &format!("{}\n", alive.pid()));
    sb.put_state(
        "tako-remote.serve",
        &format!(
            r#"{{"ok":false,"checked_at":{},"state":"foreign","target":"http://127.0.0.1:1","reason":"R","next_step":"N"}}"#,
            now()
        ),
    );
}

/// 生きている daemon の実行ファイルがもう無い（#2518 の `missing`）
fn generation_missing(sb: &Sandbox, alive: &Alive) {
    sb.put_state(
        "tako-remote.pid",
        &format!("{}\n{}/gone/tako\n1\n", alive.pid(), sb.path().display()),
    );
}

/// 生きている daemon の実行ファイルが起動の後に置かれた（識別の記録の無い daemon = ctime で判定）
fn generation_replaced(sb: &Sandbox, alive: &Alive) {
    let exe = sb.path().join("app/tako");
    std::fs::write(&exe, "x").expect("実行ファイルの代わりを書ける");
    sb.put_state(
        "tako-remote.pid",
        &format!("{}\n{}\n1\n", alive.pid(), exe.display()),
    );
}

/// GUI の自動復帰が旧版を新版へ入れ替えている途中（#2608 の印。書き手はこのテストの `sleep`）
fn starting(sb: &Sandbox, alive: &Alive) {
    sb.put_state(
        "tako-remote.starting",
        &format!(
            r#"{{"at":{},"pid":{},"via":"autostart","previous_pid":7}}"#,
            now(),
            alive.pid()
        ),
    );
}

/// 起こしている途中（入れ替えではない・CLI から）
fn starting_fresh(sb: &Sandbox, alive: &Alive) {
    sb.put_state(
        "tako-remote.starting",
        &format!(r#"{{"at":{},"pid":{},"via":"cli"}}"#, now(), alive.pid()),
    );
}

type Setup = fn(&Sandbox, &Alive);

/// status のケース（名前・用意）
const STATUS_CASES: &[(&str, Setup)] = &[
    ("status-autostart-failed", autostart_failed),
    ("status-serve-stale", serve_stale),
    ("status-serve-degraded", serve_degraded),
    ("status-generation-missing", generation_missing),
    ("status-generation-replaced", generation_replaced),
    ("status-starting-replacing", starting),
    ("status-starting-fresh", starting_fresh),
];

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

/// 数字の並びを `0` 1 字に均し（pid・経過・時刻は実行ごとに桁数も変わる）、QR の画像の名前の乱数
/// （16 桁の 16 進）を伏せる
fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("qr-") {
        out.push_str(&rest[..i + 3]);
        rest = &rest[i + 3..];
        let hex = rest.chars().take_while(char::is_ascii_hexdigit).count();
        if hex == 16 {
            out.push_str("<HEX>");
            rest = &rest[16..];
        }
    }
    out.push_str(rest);
    let mut folded = String::with_capacity(out.len());
    let mut in_digits = false;
    for c in out.chars() {
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

/// 1 言語分の全ケースの出力（名前, 出力）。daemon を起こすケースは tmux が無ければ空
fn run_lang(lang: &str) -> Vec<(String, Out)> {
    let mut outs = Vec::new();
    for (name, setup) in STATUS_CASES {
        let sb = Sandbox::new(lang, name);
        let alive = Alive::new();
        setup(&sb, &alive);
        outs.push((name.to_string(), sb.run(&["remote", "status"])));
    }
    if tako_core::backend::capabilities().detached_access {
        outs.extend(run_daemon_cases(lang));
    } else {
        eprintln!("tmux が無い（remote daemon は起動を拒む）ので start / stop は走らせない");
    }
    let dump = std::env::var_os("I2365R_DUMP").map(PathBuf::from);
    if let Some(dir) = &dump {
        let _ = std::fs::create_dir_all(dir);
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
    outs
}

/// 本物の daemon: 起こす → 実行ファイルを差し替える → status（旧版）→ start（入れ替え）→ stop
fn run_daemon_cases(lang: &str) -> Vec<(String, Out)> {
    let sb = Sandbox::new(lang, "daemon");
    let bin = sb.path().join("app/tako");
    let copy = |to: &Path| {
        std::fs::copy(tako(), to).unwrap_or_else(|e| panic!("{} へ複製できる: {e}", to.display()));
        std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    };
    copy(&bin);
    let mut daemons = Daemons {
        sb: &sb,
        bin: bin.clone(),
        pids: Vec::new(),
    };
    let mut outs = Vec::new();
    let start = sb.run_with(&bin, &["remote", "start"]);
    let first = sb.daemon_pid();
    daemons.pids.extend(first);
    assert_eq!(
        start.code, 0,
        "[{lang}] 1 本目の daemon を起こせる（終了コード）: {}",
        start.stderr
    );
    outs.push(("start".to_string(), start));
    // 識別の比較は秒の切り捨てなので、起動の秒が過ぎてから差し替える（install と同じ = 別のファイルを mv）
    std::thread::sleep(Duration::from_millis(1100));
    let next = sb.path().join("app/tako.next");
    copy(&next);
    std::fs::rename(&next, &bin).expect("差し替えられる");
    outs.push((
        "status-real-replaced".to_string(),
        sb.run_with(&bin, &["remote", "status"]),
    ));
    let renew = sb.run_with(&bin, &["remote", "start"]);
    daemons.pids.extend(sb.daemon_pid());
    assert_eq!(
        renew.code, 0,
        "[{lang}] 旧版を入れ替えられる（終了コード）: {}",
        renew.stderr
    );
    assert_ne!(sb.daemon_pid(), first, "[{lang}] 新しい daemon が立った");
    outs.push(("start-renewed".to_string(), renew));
    outs.push(("stop".to_string(), sb.run_with(&bin, &["remote", "stop"])));
    drop(daemons);
    outs
}

type Runs = (Vec<(String, Out)>, Vec<(String, Out)>);

/// 全ケースを日英で 1 回だけ流す（daemon を 4 本起こすので 1 回で済ませる）
fn runs() -> &'static Runs {
    static RUNS: OnceLock<Runs> = OnceLock::new();
    RUNS.get_or_init(|| (run_lang("ja"), run_lang("en")))
}

/// stdout の JSON から、実行ごとに変わる欄（時刻・経過・pid・砂場の外の値）を除いて比べられる形にする
fn json_shape(stdout: &str) -> Value {
    let mut v = serde_json::from_str::<Value>(stdout).unwrap_or(Value::Null);
    fn walk(v: &mut Value) {
        match v {
            Value::Object(map) => map.values_mut().for_each(walk),
            Value::Array(items) => items.iter_mut().for_each(walk),
            Value::String(s) => *s = norm(s),
            Value::Number(_) => *v = Value::from(0),
            _ => {}
        }
    }
    walk(&mut v);
    v
}

/// #2365 より前の日本語の stderr（修正前のバイナリで採った。`norm` を通した形）
fn old_ja() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "status-autostart-failed",
            "前回の起動状態へ自動で戻せませんでした（0 回試行）: D\n\
             \x20 次の一手: `tako remote start` で手動起動できます\n",
        ),
        (
            "status-serve-stale",
            "リモート公開が機能していません: serve 設定の自己検査が 0 秒前から更新されていません\
             （daemon の検査スレッドが止まっている可能性）\n\
             \x20 次の一手: `tako remote stop && tako remote start` で立て直してください\n",
        ),
        (
            "status-serve-degraded",
            "リモート公開が機能していません: R\n\x20 次の一手: N\n",
        ),
        (
            "status-generation-missing",
            "稼働中のリモート daemon（PID 0）の実行ファイル <SB>/gone/tako がもうありません。\
             tako を入れ直してから `tako remote start` で入れ替えてください\n",
        ),
        (
            "status-generation-replaced",
            "稼働中のリモート daemon（PID 0）は旧版のまま動いています: \
             <SB>/app/tako は daemon の起動後に差し替えられました。\
             `tako remote start` で新版へ入れ替わります（GUI を起動したときは自動で入れ替わります）\n",
        ),
        (
            "status-starting-replacing",
            "GUI 起動時の自動復帰がリモート daemon を新版へ入れ替えている途中です（旧版の PID 0・開始から 0 秒）。\
             新しい daemon は待ち受けを開いてから PID ファイルを書くまで数十秒かかることがあり、\
             その間は `running: false` に見えます。止めたり起動し直したりせず、しばらく待ってから \
             `tako remote status` を読み直してください\n",
        ),
        (
            "status-starting-fresh",
            "`tako remote start`がリモート daemon を起動している途中です（開始から 0 秒）。\
             新しい daemon は待ち受けを開いてから PID ファイルを書くまで数十秒かかることがあり、\
             その間は `running: false` に見えます。止めたり起動し直したりせず、しばらく待ってから \
             `tako remote status` を読み直してください\n",
        ),
        (
            "start",
            "\nQR コードを生成しました: <SB>/state/qr-<HEX>.png\n\
             スマホでスキャンしてください。\n\
             URL: http://tako-remote.test\n\
             この URL は恒久固定で secret を含みません（Tailscale MagicDNS 名。tailnet 内限定）。\n\
             スマホ側にも Tailscale アプリを入れ、同じアカウントでログインしてください。\n\
             初回アクセス時は Mac の画面にペアリング承認ダイアログが表示されます。\n\
             tako や Mac を再起動しても、この状態は自動で戻ります（`tako remote stop` で解除）。\n",
        ),
        (
            "status-real-replaced",
            "稼働中のリモート daemon（PID 0）は旧版のまま動いています: \
             <SB>/app/tako は daemon の起動後に差し替えられました。\
             `tako remote start` で新版へ入れ替わります（GUI を起動したときは自動で入れ替わります）\n",
        ),
        (
            "start-renewed",
            "旧版のまま動いていたリモート daemon（PID 0）を止めて、新版で起動し直しました。\n\
             \nQR コードを生成しました: <SB>/state/qr-<HEX>.png\n\
             スマホでスキャンしてください。\n\
             URL: http://tako-remote.test\n\
             この URL は恒久固定で secret を含みません（Tailscale MagicDNS 名。tailnet 内限定）。\n\
             スマホ側にも Tailscale アプリを入れ、同じアカウントでログインしてください。\n\
             初回アクセス時は Mac の画面にペアリング承認ダイアログが表示されます。\n\
             tako や Mac を再起動しても、この状態は自動で戻ります（`tako remote stop` で解除）。\n",
        ),
        ("stop", "リモートサーバーを停止しました\n"),
    ]
}

#[test]
fn remoteのstartとstopとstatusの人向けの表示は表示言語に従い日本語は2365より前と同じ() {
    let (ja, en) = runs();
    let mut failures = Vec::new();
    assert_eq!(ja.len(), en.len(), "日英でケースの数が同じ");
    for ((name, j), (_, e)) in ja.iter().zip(en.iter()) {
        if j.code != e.code {
            failures.push(format!(
                "{name}: 終了コードが日英で違う（ja {} / en {}）\n{}{}",
                j.code, e.code, j.stderr, e.stderr
            ));
        }
        if let Some(c) = has_japanese(&e.stderr) {
            failures.push(format!(
                "{name}: 英語表示の stderr に日本語（{c}）が混ざる:\n{}",
                e.stderr
            ));
        }
        if e.stderr.trim().is_empty() {
            failures.push(format!("{name}: 英語表示の stderr が空"));
        }
        if j.stderr.lines().count() != e.stderr.lines().count() {
            failures.push(format!(
                "{name}: 行の数が日英で違う\n--- ja\n{}--- en\n{}",
                j.stderr, e.stderr
            ));
        }
    }
    let old = old_ja();
    for (name, _) in ja.iter() {
        if !old.iter().any(|(n, _)| n == name) {
            failures.push(format!("{name}: 修正前の日本語の出力が表に無い"));
        }
    }
    for (name, want) in old {
        let Some((_, j)) = ja.iter().find(|(n, _)| n == name) else {
            if name.starts_with("status-") && name != "status-real-replaced" {
                failures.push(format!("{name}: ケースが無い"));
            }
            continue;
        };
        if norm(&j.stderr) != want {
            failures.push(format!(
                "{name}: 日本語表示が #2365 より前と違う\n--- want\n{want}--- got\n{}",
                norm(&j.stderr)
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

/// stdout の JSON（応答の値）は表示言語に依らない: `serve_generation.detail` / `starting.detail` /
/// `degraded` / `warnings` は日本語のまま（MCP `tako_remote_status` と GUI のリモートカードが読む値）
#[test]
fn stdoutのjsonは日英で同じで応答の説明は日本語のまま() {
    let (ja, en) = runs();
    let mut failures = Vec::new();
    for ((name, j), (_, e)) in ja.iter().zip(en.iter()) {
        let (sj, se) = (json_shape(&j.stdout), json_shape(&e.stdout));
        if sj.is_null() {
            failures.push(format!("{name}: stdout が JSON ではない:\n{}", j.stdout));
            continue;
        }
        if sj != se {
            failures.push(format!(
                "{name}: stdout の JSON が日英で違う\n--- ja\n{sj:#}\n--- en\n{se:#}"
            ));
        }
    }
    // 応答の説明は日本語のまま（英語表示でも）
    let status = |name: &str| {
        en.iter()
            .find(|(n, _)| n == name)
            .map(|(_, o)| serde_json::from_str::<Value>(&o.stdout).unwrap_or(Value::Null))
    };
    for (name, pointer) in [
        ("status-serve-stale", "/degraded/reason"),
        ("status-generation-missing", "/serve_generation/detail"),
        ("status-generation-replaced", "/serve_generation/detail"),
        ("status-starting-replacing", "/starting/detail"),
    ] {
        let value = status(name)
            .and_then(|v| v.pointer(pointer).and_then(Value::as_str).map(String::from))
            .unwrap_or_default();
        if has_japanese(&value).is_none() {
            failures.push(format!(
                "{name}: 英語表示の応答の {pointer} が日本語ではない（応答の値は変えない）: {value:?}"
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

/// 英語表示の stderr の見本（文を書き換えたらここも直す。数字は `0` に均した形）
#[test]
fn 英語表示の見本() {
    let (_, en) = runs();
    let mut failures = Vec::new();
    for (name, want) in [
        (
            "status-autostart-failed",
            "could not automatically restore the previous running state (0 attempt(s)): D\n\
             \x20 next step: you can start it by hand with `tako remote start`\n",
        ),
        (
            "status-serve-stale",
            "remote access is not working: the serve self-check has not been updated for 0 s \
             (the daemon's check thread may have stopped)\n\
             \x20 next step: rebuild it with `tako remote stop && tako remote start`\n",
        ),
        (
            "status-generation-missing",
            "the executable <SB>/gone/tako of the running remote daemon (PID 0) no longer exists. \
             Reinstall tako, then swap the daemon with `tako remote start`\n",
        ),
        (
            "status-starting-replacing",
            "The remote daemon is being replaced with the new version by the GUI's automatic restore \
             at launch (old PID 0, started 0 s ago). A new daemon can take tens of seconds between \
             opening its listener and writing the PID file, and looks like `running: false` until then. \
             Do not stop or restart it; wait a while, then check `tako remote status` again\n",
        ),
        ("stop", "stopped the remote server\n"),
    ] {
        let Some((_, e)) = en.iter().find(|(n, _)| n == name) else {
            if name.starts_with("status-") {
                failures.push(format!("{name}: ケースが無い"));
            }
            continue;
        };
        if norm(&e.stderr) != want {
            failures.push(format!(
                "{name}: 英語表示が見本と違う\n--- want\n{want}--- got\n{}",
                norm(&e.stderr)
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
