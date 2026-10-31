//! ペインの中から起こした remote daemon が、そのペインの env を持ったまま常駐しない（#2582・実プロセス）
//!
//! このビルドの `tako remote serve` を、ペインの中の claude の Bash の子が持つ env（ペインの識別・
//! 起こした GUI への接続・claude のセッションの印・`GIT_EDITOR=true`）で起こし、起動情報を返した daemon の
//! **exec の時点の env**（`ps eww` と資源の走査が見る値 = macOS の `KERN_PROCARGS2`）を読む。
//! 外す名前の正本は `tako_core::host_env::STRIP`、配線の番犬は
//! `crates/tako-control/tests/issue2582_daemon_inherited_pane_env_watchdog.rs`。
//!
//! 隔離: data dir・発見の置き場・state dir はテストの一時 dir、tailscale は偽物、
//! `TAKO_REMOTE_TEST_MODE=1`（実 serve を張らない）、tmux のソケットはテスト専用の名前
#![cfg(target_os = "macos")]

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::Value;
use tako_core::host_env;
use tako_core::test_residue::ScratchDir;

/// ペインの中の claude の Bash の子が持つ印（値はテスト用の置き換え。`TAKO_SOCKET` は別に一時 dir の中を指す）
const PANE_ENV: [(&str, &str); 9] = [
    ("TAKO_PANE_ID", "7"),
    ("TAKO_TAB_ID", "3"),
    ("TAKO_TOKEN", "token-2582-not-for-logs"),
    ("TAKO_ORCHESTRATOR_ROLE", "worker:t2582"),
    ("CLAUDECODE", "1"),
    ("CLAUDE_CODE_SESSION_ID", "session-2582-not-for-logs"),
    ("CLAUDE_CODE_ENTRYPOINT", "cli"),
    ("GIT_EDITOR", "true"),
    ("COREPACK_ENABLE_AUTO_PIN", "0"),
];

/// 隔離の指定（`host_env::KEPT`。外しすぎの見張り）
const ISOLATION: [&str; 5] = [
    "TAKO_ISOLATED",
    "TAKO_DATA_DIR",
    "TAKO_DISCOVERY_DIR",
    "TAKO_REMOTE_STATE_DIR",
    "TAKO_TMUX_SOCKET",
];

/// 終わるときに daemon を畳む（SIGTERM → 残れば落とす）
struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .args(["-TERM", &self.0.id().to_string()])
            .status();
        for _ in 0..50 {
            if matches!(self.0.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// 起動情報の行を待つ相手（起動の拒否は stderr に出て stdout が閉じる）
fn wait_started(child: &mut Child) -> Result<Value, String> {
    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                let _ = tx.send(v);
                return;
            }
        }
    });
    rx.recv_timeout(Duration::from_secs(60)).map_err(|_| {
        let mut err = String::new();
        if let Some(e) = child.stderr.take() {
            use std::io::Read as _;
            let _ = e.take(4096).read_to_string(&mut err);
        }
        format!("daemon が起動情報を返さなかった: {err}")
    })
}

fn fake_tailscale(dir: &Path) -> PathBuf {
    let path = dir.join("tailscale-fake");
    std::fs::write(
        &path,
        "#!/bin/sh\n\
         case \"$1 $2\" in\n\
         \"--version \"*) echo 1.0.0-fake ;;\n\
         \"status \"*) echo '{\"BackendState\":\"Running\",\"Self\":{\"DNSName\":\"fake-2582.ts.net.\"}}' ;;\n\
         \"serve status\") echo '{}' ;;\n\
         *) exit 1 ;;\n\
         esac\n",
    )
    .expect("偽 tailscale を書ける");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

/// 1 回の起動の置き場
struct Case {
    root: PathBuf,
    state: PathBuf,
    /// テスト専用の tmux ソケット名
    tmux: String,
}

impl Case {
    fn new(dir: &ScratchDir, name: &str) -> Self {
        let root = dir.join(name);
        let state = root.join("state");
        for d in [root.join("data"), root.join("disc"), state.clone()] {
            std::fs::create_dir_all(&d).expect("置き場を作れる");
        }
        let tmux = format!("t2582-{}-{}", std::process::id(), name)
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        Self { root, state, tmux }
    }

    /// daemon を起こす。`pane` はペインの中の claude の Bash の子の印を持たせるか、`extra` は足す env
    fn serve(&self, tailscale: &Path, pane: bool, extra: &[(&str, &str)]) -> (Daemon, Value) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_tako"));
        cmd.args(tako_control::remote::SERVE_ARGS)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // 手元のペイン（このテストを走らせたもの）の印・接続・隔離の指定・A/B（`TAKO_` で始まるもの）と、
        // claude のセッションの印ほか `STRIP` の名前を持ち込まない（#1716。必要なものは下で並べ直す）
        for (name, _) in std::env::vars_os() {
            let name = name.to_string_lossy();
            if name.starts_with("TAKO_") {
                cmd.env_remove(name.as_ref());
            }
        }
        for s in host_env::STRIP {
            for name in s.names {
                cmd.env_remove(name);
            }
        }
        cmd.env("TAKO_ISOLATED", "1")
            .env("TAKO_DATA_DIR", self.root.join("data"))
            .env("TAKO_DISCOVERY_DIR", self.root.join("disc"))
            .env("TAKO_REMOTE_STATE_DIR", &self.state)
            .env("TAKO_REMOTE_TEST_MODE", "1")
            .env("TAKO_TAILSCALE_BIN", tailscale)
            .env("TAKO_TMUX_SOCKET", &self.tmux);
        if pane {
            cmd.env("TAKO_SOCKET", self.root.join("none.sock"));
            for (name, value) in PANE_ENV {
                cmd.env(name, value);
            }
        }
        for (name, value) in extra {
            cmd.env(name, value);
        }
        let mut child = cmd.spawn().expect("tako remote serve を起こせる");
        let info = wait_started(&mut child);
        let daemon = Daemon(child);
        match info {
            Ok(info) => (daemon, info),
            Err(e) => panic!("{e}"),
        }
    }

    /// daemon の exec の時点の env（読む名前だけ）
    fn env_of(&self, daemon: &Daemon) -> HashMap<String, String> {
        let mut names: Vec<&str> = PANE_ENV.iter().map(|(n, _)| *n).collect();
        names.extend(["TAKO_SOCKET", host_env::REEXEC_ENV, "TAKO_REMOTE_TEST_MODE"]);
        names.extend(ISOLATION);
        tako_core::ports::process_env_vars(daemon.0.id(), &names)
            .expect("daemon の exec の時点の env を読める（KERN_PROCARGS2）")
    }

    /// audit.log の `daemon_startup_env` の行（全文と、その行）
    fn audit(&self) -> (String, Vec<Value>) {
        let text = std::fs::read_to_string(self.state.join("audit.log")).unwrap_or_default();
        let rows = text
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|v| v["event"] == "daemon_startup_env")
            .collect();
        (text, rows)
    }

    fn pid_file(&self) -> Option<u32> {
        std::fs::read_to_string(self.state.join("tako-remote.pid"))
            .ok()?
            .lines()
            .next()?
            .trim()
            .parse()
            .ok()
    }
}

#[test]
fn ペインの中から起こしたdaemonはペインの印を外し隔離の指定は残す() {
    if !tako_core::backend::capabilities().detached_access {
        eprintln!("tmux が無い（remote daemon は起動を拒む）ので走らせない");
        return;
    }
    let dir = ScratchDir::new("issue2582");
    let tailscale = fake_tailscale(&dir);

    // 修正後: ペインの中の CLI から起こした daemon
    let case = Case::new(&dir, "pane");
    let (daemon, _) = case.serve(&tailscale, true, &[]);
    let env = case.env_of(&daemon);
    for (name, _) in PANE_ENV {
        assert!(
            !env.contains_key(name),
            "daemon に {name} が残った: {env:?}"
        );
    }
    assert!(
        !env.contains_key("TAKO_SOCKET"),
        "daemon に TAKO_SOCKET が残った"
    );
    for name in ISOLATION {
        assert!(
            env.contains_key(name),
            "隔離の指定 {name} まで外した: {env:?}"
        );
    }
    assert_eq!(env.get("TAKO_ISOLATED").map(String::as_str), Some("1"));
    assert_eq!(
        env.get("TAKO_REMOTE_TEST_MODE").map(String::as_str),
        Some("1")
    );
    // 起こし直しても同じ pid（PID ファイル = #2518 / #2533 の材料と一致）。印は名前だけ
    assert_eq!(case.pid_file(), Some(daemon.0.id()), "PID ファイルの pid");
    let mark = env.get(host_env::REEXEC_ENV).expect("起こし直した印");
    assert!(
        mark.starts_with("7\t") && mark.contains("TAKO_PANE_ID") && mark.contains("GIT_EDITOR"),
        "{mark}"
    );
    let (text, rows) = case.audit();
    assert_eq!(rows.len(), 1, "audit.log の daemon_startup_env: {text}");
    assert_eq!(rows[0]["action"], "exec", "{}", rows[0]);
    assert_eq!(rows[0]["pane"], 7, "{}", rows[0]);
    let names: Vec<&str> = rows[0]["names"]
        .as_array()
        .expect("names")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for want in [
        "TAKO_PANE_ID",
        "TAKO_SOCKET",
        "TAKO_TOKEN",
        "CLAUDECODE",
        "GIT_EDITOR",
    ] {
        assert!(
            names.contains(&want),
            "{want} が外した名前に無い: {names:?}"
        );
    }
    for value in ["token-2582-not-for-logs", "session-2582-not-for-logs"] {
        assert!(!text.contains(value), "audit.log に env の値が載った");
        assert!(!mark.contains(value), "起こし直しの印に env の値が載った");
    }
    drop(daemon);

    // Dock 起動相当（GUI の自動復帰で起きた daemon = GUI が外した後の env）: 何もしない（修正前と同じ）
    let case = Case::new(&dir, "dock");
    let (daemon, _) = case.serve(&tailscale, false, &[]);
    let env = case.env_of(&daemon);
    assert!(
        !env.contains_key(host_env::REEXEC_ENV),
        "外すものが無いのに起こし直した"
    );
    for name in ISOLATION {
        assert!(env.contains_key(name), "隔離の指定 {name} が無い: {env:?}");
    }
    let (text, rows) = case.audit();
    assert!(
        rows.is_empty(),
        "外すものが無いのに audit.log へ書いた: {text}"
    );
    drop(daemon);

    // A/B: daemon の入口・GUI から受け継いだ入口のどちらでも外さない（#2582 前の形）
    for legacy in [host_env::DAEMON_LEGACY_ENV, host_env::LEGACY_ENV] {
        let case = Case::new(&dir, &format!("legacy-{legacy}"));
        let (daemon, _) = case.serve(&tailscale, true, &[(legacy, "1")]);
        let env = case.env_of(&daemon);
        assert_eq!(
            env.get("TAKO_PANE_ID").map(String::as_str),
            Some("7"),
            "{legacy}"
        );
        assert_eq!(
            env.get("GIT_EDITOR").map(String::as_str),
            Some("true"),
            "{legacy}"
        );
        assert!(!env.contains_key(host_env::REEXEC_ENV), "{legacy}");
        let (text, rows) = case.audit();
        assert_eq!(rows.len(), 1, "{legacy}: {text}");
        assert_eq!(rows[0]["action"], "legacy", "{legacy}: {}", rows[0]);
        drop(daemon);
    }
}
