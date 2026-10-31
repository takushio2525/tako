//! 突然死の復旧案内どおりに打つと、spawn と同じ起動で worker が戻る（Issue #2470）
//!
//! 修正前（実測 = 下の `修正前の案内は_mcp_の絞り込みと許可モードと_env_を落とす`）:
//! 案内（worker_status / watch / workers の `resume_command`）は
//! `export CLAUDE_CONFIG_DIR=…; cd '<cwd>' && claude --model … --effort … --resume <id>` で、
//! #2412 の `--settings`（user scope の MCP を止める）・プロファイルの args（本番は全プロファイルが
//! `--permission-mode auto`）と env・`TAKO_ORCHESTRATOR_ROLE` が載らなかった。
//!
//! いまは案内が `tako orchestrator resume <worker_id>` で、打った時点で spawn と同じ組み立てを通し、
//! このシェルで起動する。本物の CLI を隔離 data dir（`TAKO_DATA_DIR` = 使い捨て dir・`HOME` も使い捨て・
//! GUI 無し）で叩き、PATH の先頭の偽 `claude` が受け取った argv / env / 作業 dir を採る。
//! 会話の記録はアカウントの設定 dir（`CLAUDE_CONFIG_DIR`）に置く（`--account` で spawn した worker と同じ形。
//! user scope の MCP はその dir の `.claude.json` から読まれる）。A/B は `TAKO_2470_LEGACY=1`
//! （修正前の案内を同じ走らせ方で走らせる）。
//!
//! 無人の自動復旧（supervisor の `recover_dead`）は案内を死んだペインのシェルへ打ち込む。そのシェルの PATH の
//! 先頭に `orchestrator resume` を知らない古い `tako`（開発用の `target/release/tako` 等）が居ても、
//! 動いている tako と同じ場所の CLI を絶対パスで打って戻すことを、本物の `/bin/sh` で見る
//! （`無人の復旧は_path_の先頭の古い_tako_に当たらない`）。
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tako_core::test_residue::ScratchDir;

const SID: &str = "5b3f0c1e-2470-4a7b-9c11-0123456789ab";
const PROFILE: &str = "p2470";

/// 偽 claude: 受け取った argv・作業 dir・見たい env を 1 行ずつ書いて終わる
const FAKE_CLAUDE: &str = r#"#!/bin/sh
{
  echo "pwd=$(pwd)"
  echo "CLAUDE_CONFIG_DIR=${CLAUDE_CONFIG_DIR-<unset>}"
  echo "TAKO_ORCHESTRATOR_ROLE=${TAKO_ORCHESTRATOR_ROLE-<unset>}"
  echo "T2470_ENV=${T2470_ENV-<unset>}"
  for a in "$@"; do echo "arg=$a"; done
} > "$T2470_OUT"
"#;

/// 偽の古い tako: `orchestrator resume` を知らない開発ビルド。呼ばれたことを残して clap と同じ形で落ちる
const STALE_TAKO: &str = r#"#!/bin/sh
echo "$*" >> "$T2470_STALE_LOG"
echo "error: unrecognized subcommand 'resume'" >&2
exit 2
"#;

struct Env {
    dir: ScratchDir,
}

impl Env {
    fn new() -> Self {
        let dir = ScratchDir::new("issue2470");
        assert!(
            dir.path().starts_with(std::env::temp_dir()),
            "一時 dir の外に書こうとしている: {}",
            dir.path().display()
        );
        let env = Self { dir };
        for d in [
            env.home(),
            env.proj(),
            env.bin(),
            env.profiles(),
            env.account().join("projects").join("-proj"),
        ] {
            std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
        }
        // 偽 claude
        let claude = env.bin().join("claude");
        std::fs::write(&claude, FAKE_CLAUDE).unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
        // 会話の記録（アカウントの設定 dir）と、その dir の user scope の MCP 2 つ
        std::fs::write(
            env.account()
                .join("projects")
                .join("-proj")
                .join(format!("{SID}.jsonl")),
            "{}\n",
        )
        .unwrap();
        std::fs::write(
            env.account().join(".claude.json"),
            r#"{"mcpServers":{"heavy-a":{"command":"node","args":["a.js"]},"heavy-b":{"command":"node","args":["b.js"]}}}"#,
        )
        .unwrap();
        // spawn した master のプロファイル（本番と同じ形: 許可モードの args と env）
        std::fs::write(
            env.profiles().join(format!("{PROFILE}.yaml")),
            "env:\n  T2470_ENV: from-profile\nbypass_sandbox: false\n\
             worker_agents:\n  claude:\n    model: claude-opus-5-5\n    effort: xhigh\n    \
             args: [\"--permission-mode\", \"auto\"]\n",
        )
        .unwrap();
        env.write_registry(Some(PROFILE), Some(SID));
        // 断り・案内は表示言語に従う（#2365）。下の検査は日本語の文を見るので表示言語を固定する
        // （書かないと system = 実行環境のロケールで決まり、CI と手元で答えが変わる）
        std::fs::write(
            env.dir.path().join("settings.json"),
            "{\"language\":\"ja\"}",
        )
        .unwrap();
        env
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }
    fn proj(&self) -> PathBuf {
        self.dir.path().join("proj")
    }
    fn bin(&self) -> PathBuf {
        self.dir.path().join("bin")
    }
    fn account(&self) -> PathBuf {
        self.dir.path().join("acct")
    }
    fn profiles(&self) -> PathBuf {
        self.dir.path().join("orchestrator").join("profiles")
    }
    fn out(&self) -> PathBuf {
        self.dir.path().join("claude-argv.txt")
    }

    /// worker 7 の spawn の記録（`profile` は #2470 で足した欄）
    fn write_registry(&self, profile: Option<&str>, sid: Option<&str>) {
        self.write_registry_with(profile, sid, "active");
    }

    fn write_registry_with(&self, profile: Option<&str>, sid: Option<&str>, status: &str) {
        let mut yaml = format!(
            "next_id: 7\nworkers:\n  '7':\n    label: cl\n    project: tako\n    agent: claude\n    \
             model: claude-opus-5-5\n    effort: xhigh\n    pane: 5\n    cwd: {}\n    \
             spawned_at: '2026-10-29T00:00:00Z'\n    status: {status}\n",
            self.proj().display()
        );
        if let Some(sid) = sid {
            yaml.push_str(&format!("    session_id: {sid}\n"));
        }
        if let Some(p) = profile {
            yaml.push_str(&format!("    profile: {p}\n"));
        }
        std::fs::write(self.dir.path().join("workers.yaml"), yaml).unwrap();
    }

    /// 隔離した tako（本番の GUI にも data dir にも HOME にも触らない。GUI は居ない）
    fn resume(&self, worker: &str, legacy: bool) -> (i32, String) {
        let _ = std::fs::remove_file(self.out());
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_tako"));
        cmd.args(["orchestrator", "resume", worker])
            .current_dir(self.dir.path())
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin().display()))
            .env("HOME", self.home())
            .env("SHELL", "/bin/sh")
            .env("TAKO_ISOLATED", "1")
            .env("TAKO_DATA_DIR", self.dir.path())
            // master のシェルが持っている別アカウントの値（会話の記録の所在へ寄せ直すことを見る）
            .env("CLAUDE_CONFIG_DIR", self.account())
            .env("T2470_OUT", self.out())
            .stdin(Stdio::null());
        if legacy {
            cmd.env("TAKO_2470_LEGACY", "1");
        }
        let out = cmd.output().expect("tako を起こせない");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn stale_log(&self) -> PathBuf {
        self.dir.path().join("stale-tako.log")
    }

    /// PATH の先頭（偽 claude と同じ dir）へ、`orchestrator resume` を知らない古い tako を置く
    fn install_stale_tako(&self) {
        let tako = self.bin().join("tako");
        std::fs::write(&tako, STALE_TAKO).unwrap();
        std::fs::set_permissions(&tako, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// 死んだ worker のペインのシェル: 打ち込まれた文字を本物の `/bin/sh` が読む。端末は Enter の
    /// `\r` を改行へ直して渡す（ICRNL）ので、同じ変換をしてから標準入力へ流す。env は `resume` と同じ
    fn pane_shell(&self, typed: &str) -> i32 {
        use std::io::Write;
        let _ = std::fs::remove_file(self.out());
        let mut child = Command::new("/bin/sh")
            .current_dir(self.dir.path())
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin().display()))
            .env("HOME", self.home())
            .env("SHELL", "/bin/sh")
            .env("TAKO_ISOLATED", "1")
            .env("TAKO_DATA_DIR", self.dir.path())
            .env("CLAUDE_CONFIG_DIR", self.account())
            .env("T2470_OUT", self.out())
            .env("T2470_STALE_LOG", self.stale_log())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("/bin/sh を起こせない");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(typed.replace('\r', "\n").as_bytes())
            .unwrap();
        child.wait().unwrap().code().unwrap_or(-1)
    }

    /// 偽 claude が受け取ったもの（起動されていなければ panic）
    fn captured(&self) -> Captured {
        let text = std::fs::read_to_string(self.out()).unwrap_or_else(|e| {
            panic!(
                "偽 claude が起動されていない（{}）: {e}",
                self.out().display()
            )
        });
        Captured(text)
    }
}

struct Captured(String);

impl Captured {
    fn value(&self, key: &str) -> Option<&str> {
        self.0
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{key}=")))
    }
    fn args(&self) -> Vec<&str> {
        self.0
            .lines()
            .filter_map(|l| l.strip_prefix("arg="))
            .collect()
    }
    /// `flag` の直後の引数
    fn arg_after(&self, flag: &str) -> Option<&str> {
        let args = self.args();
        args.iter()
            .position(|a| *a == flag)
            .and_then(|i| args.get(i + 1).copied())
    }
}

fn same_dir(a: &str, b: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(Path::new(a)) == canon(b)
}

/// 受け入れ条件 1: 案内どおりに打つと、spawn と同じ MCP の絞り込み・許可モード・env・role・
/// model / effort・アカウント・作業 dir で戻る（GUI 無し）
#[test]
fn 案内どおりに打つと_spawn_と同じ起動で戻る() {
    let env = Env::new();
    let (code, err) = env.resume("7", false);
    assert_eq!(code, 0, "復旧が失敗した:\n{err}");
    let got = env.captured();
    let args = got.args();

    // MCP の絞り込み（#2412）: --settings のファイルが実在し、user scope の 2 つを止めている
    let settings = got.arg_after("--settings").unwrap_or_else(|| {
        panic!("--settings が無い = user scope の MCP を全部持って戻る: {args:?}")
    });
    let body = std::fs::read_to_string(settings)
        .unwrap_or_else(|e| panic!("--settings のファイル {settings} が無い: {e}"));
    assert!(
        body.contains("\"heavy-a\"") && body.contains("\"heavy-b\""),
        "user scope の MCP を止めていない: {body}"
    );
    assert!(
        Path::new(settings).starts_with(env.dir.path()),
        "設定ファイルを隔離 data dir の外へ書いた: {settings}"
    );
    // プロファイルの args・model / effort・会話 ID
    assert_eq!(got.arg_after("--permission-mode"), Some("auto"), "{args:?}");
    assert_eq!(
        got.arg_after("--model"),
        Some("claude-opus-5-5"),
        "{args:?}"
    );
    assert_eq!(got.arg_after("--effort"), Some("xhigh"), "{args:?}");
    assert_eq!(got.arg_after("--resume"), Some(SID), "{args:?}");
    // プロファイルの env・role・アカウント（会話の記録がある設定 dir）・作業 dir
    assert_eq!(got.value("T2470_ENV"), Some("from-profile"), "{}", got.0);
    assert_eq!(
        got.value("TAKO_ORCHESTRATOR_ROLE"),
        Some("worker:tako:cl"),
        "{}",
        got.0
    );
    assert!(
        same_dir(got.value("CLAUDE_CONFIG_DIR").unwrap_or(""), &env.account()),
        "アカウントが会話の記録の設定 dir でない: {}",
        got.0
    );
    assert!(
        same_dir(got.value("pwd").unwrap_or(""), &env.proj()),
        "worker の作業 dir で起動していない: {}",
        got.0
    );
    assert!(
        err.contains("worker 7 の会話") && err.contains(PROFILE),
        "何を戻したかを言っていない:\n{err}"
    );

    // closed（ペインごと消えた・GC で倒れた）の worker も同じ記録から同じ起動で戻る
    env.write_registry_with(Some(PROFILE), Some(SID), "closed");
    let (code, err) = env.resume("7", false);
    assert_eq!(code, 0, "closed の worker を戻せない:\n{err}");
    let closed = env.captured();
    assert_eq!(
        closed.args(),
        args,
        "closed でも active と同じ起動になる: {:?}",
        closed.0
    );
}

/// 修正前（同じ走らせ方で修正前の案内を走らせる）は MCP の絞り込み・許可モード・env・role が落ちる
#[test]
fn 修正前の案内は_mcp_の絞り込みと許可モードと_env_を落とす() {
    let env = Env::new();
    let (code, err) = env.resume("7", true);
    assert_eq!(code, 0, "修正前の案内が走らない:\n{err}");
    let got = env.captured();
    let args = got.args();
    assert_eq!(got.arg_after("--resume"), Some(SID), "{args:?}");
    assert_eq!(
        got.arg_after("--model"),
        Some("claude-opus-5-5"),
        "{args:?}"
    );
    assert!(
        !args.contains(&"--settings"),
        "修正前の再現になっていない: {args:?}"
    );
    assert!(!args.contains(&"--permission-mode"), "{args:?}");
    assert_eq!(got.value("T2470_ENV"), Some("<unset>"), "{}", got.0);
    assert_eq!(
        got.value("TAKO_ORCHESTRATOR_ROLE"),
        Some("<unset>"),
        "{}",
        got.0
    );
}

/// 境界: 記録が足りない・作業 dir が消えた・知らない worker は起動せずに理由で止まる。
/// #2470 前の記録（プロファイル名が無い）は既定プロファイルで戻し、黙らない
#[test]
fn 記録が足りなければ起動せずに理由で止まる() {
    let env = Env::new();

    let (code, err) = env.resume("99", false);
    assert_ne!(code, 0);
    assert!(err.contains("worker 99 がレジストリに無い"), "{err}");

    env.write_registry(Some(PROFILE), None);
    let (code, err) = env.resume("7", false);
    assert_ne!(code, 0);
    assert!(err.contains("会話 ID が未検出"), "{err}");
    assert!(!env.out().exists(), "理由で止まるはずが起動した");

    // closed / ペインの無い worker でも、作業 dir が無ければ起動しない（cd に失敗して別の場所で戻さない）
    env.write_registry(Some(PROFILE), Some(SID));
    std::fs::remove_dir_all(env.proj()).unwrap();
    let (code, err) = env.resume("7", false);
    assert_ne!(code, 0);
    assert!(err.contains("作業 dir"), "{err}");
    assert!(!env.out().exists(), "作業 dir が無いのに起動した");
    std::fs::create_dir_all(env.proj()).unwrap();

    // #2470 前の記録: 既定プロファイル（ここでは未作成 = 組み込み既定）で戻すが警告する
    env.write_registry(None, Some(SID));
    let (code, err) = env.resume("7", false);
    assert_eq!(code, 0, "{err}");
    assert!(
        err.contains("警告:") && err.contains("プロファイルの記録が無い"),
        "黙って既定で戻した:\n{err}"
    );
    let got = env.captured();
    assert!(
        got.arg_after("--settings").is_some(),
        "既定プロファイルでも MCP の絞り込みは spawn と同じ: {:?}",
        got.args()
    );
}

/// 差し戻し（#2470）: 無人の自動復旧（supervisor の `recover_dead`）は、死んだペインのシェルの PATH の先頭に
/// `orchestrator resume` を知らない古い tako が居ても、動いている tako と同じ場所の CLI を絶対パスで打って戻す
#[test]
fn 無人の復旧は_path_の先頭の古い_tako_に当たらない() {
    use std::cell::RefCell;
    use tako_control::launch_cmd::ShellDialect;
    use tako_control::orchestrator::registry::ResumeShell;
    use tako_control::orchestrator::supervisor::{
        recover_dead, SupervisorContext, SupervisorMode, SupervisorState,
    };
    use tako_control::protocol::Request;

    // supervisor の監査ログ（data dir の supervisor.log）を本番へ書かない（テストのプロセスは一時 dir）
    let data = tako_core::paths::data_dir().expect("data dir");
    assert!(
        data.starts_with(std::env::temp_dir()),
        "本番の data dir へ書こうとしている: {}",
        data.display()
    );
    let env = Env::new();
    env.install_stale_tako();
    let guide = "tako orchestrator resume 7";

    // 修正前（PR の head の recover_dead は表示用の案内をそのまま打っていた）: 古い tako に当たって落ちる
    let code = env.pane_shell(&format!("{guide}\r"));
    assert_ne!(code, 0, "古い tako で通ってしまった");
    let stale = std::fs::read_to_string(env.stale_log()).unwrap_or_default();
    assert!(
        stale.contains("orchestrator resume 7"),
        "PATH の先頭の古い tako に当たっていない: {stale:?}"
    );
    assert!(!env.out().exists(), "古い tako のはずが claude が起動した");
    std::fs::remove_file(env.stale_log()).unwrap();

    // 修正後: supervisor の経路そのもの（recover_dead）が打つ 1 行を同じシェルへ流す。
    // 動いている tako と同じ場所の CLI = このテストが叩いている本物の CLI
    let typed = RefCell::new(Vec::<String>::new());
    let cli = PathBuf::from(env!("CARGO_BIN_EXE_tako"));
    let recovered = {
        let mut exec = |r: Request| -> Result<serde_json::Value, String> {
            match r {
                Request::Send { text, .. } => {
                    typed.borrow_mut().push(text.clone());
                    env.pane_shell(&text);
                    Ok(serde_json::json!({"ok": true}))
                }
                Request::OrchestratorWorkerStatus { .. } => Ok(serde_json::json!({
                    "status": if env.out().exists() { "busy" } else { "idle" }
                })),
                _ => Err("mock".into()),
            }
        };
        let mut ctx = SupervisorContext {
            exec: &mut exec,
            pane_id: 5,
            worker_id: "7".into(),
            mode: SupervisorMode::Auto,
            auto_resume_dead: true,
            max_retries: 3,
            resume_shell: ResumeShell {
                cli: Some(cli.clone()),
                dialect: ShellDialect::Posix,
            },
        };
        let mut state = SupervisorState::default();
        recover_dead(&mut ctx, Some(guide), &mut state)
    };
    let typed = typed.into_inner();
    assert_eq!(typed.len(), 1, "{typed:?}");
    assert!(
        typed[0].contains(&cli.display().to_string()) && !typed[0].starts_with("tako "),
        "無人の復旧が PATH の tako を打っている: {:?}",
        typed[0]
    );
    assert!(
        recovered,
        "supervisor の復旧が busy を確かめられなかった: {typed:?}"
    );
    assert!(
        !env.stale_log().exists(),
        "古い tako に当たった: {:?}",
        std::fs::read_to_string(env.stale_log())
    );
    let got = env.captured();
    assert!(
        got.arg_after("--settings").is_some(),
        "spawn と同じ起動で戻っていない: {:?}",
        got.args()
    );
    assert_eq!(got.arg_after("--permission-mode"), Some("auto"));
    assert_eq!(got.arg_after("--resume"), Some(SID));
}
