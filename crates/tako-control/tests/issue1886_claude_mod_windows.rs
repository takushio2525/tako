//! Windows の Claude Code が tako mod を読み、mod → `tako mod report` → 名前付きパイプの IPC →
//! tako 側の受け取り（`mod_turn`）まで通るか、ConPTY のペインへの env 注入が効くかの実測（#1886）。
//!
//! **本物の claude を起こす**。`scripts/windows/test-claude-mod-1886.ps1` が砂場へ claude を入れ（製品の口
//! `tako setup bootstrap install`）、偽の Messages API（`scripts/windows/lib/fake-anthropic-2433.ps1`）を立ててから
//! このテストを `--ignored` で名指しで回す。認証は使わない（ダミーの API キー）。claude を外部（claude.ai）から
//! 取るので、CI では `agent_install` の門の中でだけ走る（#2433 の段と同じ）。
//!
//! GUI は立てないが、GUI（`tako-app` の `spawn_session`）がペインを作るときと**同じ部品**で組む:
//!
//! | 段 | 部品 |
//! |---|---|
//! | 展開 | `claude_mod::install_in`（GUI 起動時の `install` の本体） |
//! | 注入 | `ModHub::decide` → `claude_mod_install::refine_injection` → `claude_mod::pane_env`（区切りは `LIST_SEP`） |
//! | ペイン | `TerminalSession::spawn`（ConPTY） |
//! | 受け口 | `ipc::IpcServer::start_at`（名前付きパイプ）。受けた報告は dispatch の代わりに `parse_report` → `ModHub::accept` → `claude_mod::turn_json`（応答の `mod_turn` を組む関数） |
//! | 写し | スクリプトが製品の口 `tako mod install`（MCP `tako_mod` の install・setup の段と同じ `run_action` → `sync_targets`）で置く。claude の版は GUI と同じ `bounded_claude_version` |
//!
//! 組み合わせ（どれも data dir・CLI・設定 dir のパスに空白と日本語を含める）:
//!
//! | case | 読ませ方 | 起こし方 |
//! |---|---|---|
//! | inline | env `CLAUDE_CODE_PLUGIN_DIRS` = 展開先だけ | ペインの command = claude.exe |
//! | inline-concat | env = 展開先 `;` 利用者の plugin（既に値がある場合の連結） | ペインの PowerShell 5.1 から打つ |
//! | skills-dir | `<設定 dir>/skills/tako` の写しだけ（env の展開先なし） | ペインの command = claude.exe |
//! | off | 注入しない（A/B `TAKO_1877_NO_MOD` と同じ判断の腕） | ペインの command = claude.exe |
//!
//! 各 case で、`claude -p --output-format stream-json`（init の行の `plugins`）と対話のペイン（報告の到着・
//! ターン状態・/exit で `ended`）を見る。`off` は対照（報告が来ないこと）で、この測り方に検出力があることを示す。
//! 信頼の確認が通るまで hooks module（mod）は読まれないので、どの case も製品の事前信頼
//! （`claude_tui::ensure_trusted_in`。鍵は claude の形 = `claude_project_key`）で確認が出ないことを見て、最後に
//! 修正前の鍵の形（A/B の腕 = `\` 区切り）では確認が出ることを確かめる。対話の claude は `--permission-mode default`
//! で起こす（2.1.319 の対話の既定は auto mode で、偽の API の経路ではサーバ側の安全確認が通らず標準でない確認が出る）。
//! 結果は `TAKO1886_*` の行で CI のログへ残す（パスは `<scratch>` / `<home>` / `<run>` に置き換える）。

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use serde_json::{json, Value};
use tako_control::claude_mod_install as mod_install;
use tako_control::dispatch::DispatchError;
use tako_control::ipc::IpcServer;
use tako_control::protocol::Request;
use tako_core::claude_mod::{self as cm, ModHub};
use tako_core::terminal::{SessionNotice, SpawnCommand, SpawnOptions, TerminalSession};
use tako_core::test_residue::ScratchDir;

/// 砂場に入れた本物の claude.exe（スクリプトが渡す）
const CLAUDE_EXE_ENV: &str = "TAKO_1886_CLAUDE_EXE";
/// ビルドした tako.exe（`TAKO_CLI` の元。空白と日本語を含む置き場へ写してから渡す）
const TAKO_EXE_ENV: &str = "TAKO_1886_TAKO_EXE";
/// 偽の Messages API の URL
const API_URL_ENV: &str = "TAKO_1886_API_URL";
/// claude を入れた砂場の USERPROFILE（claude の書き込みをここへ閉じ込める）
const HOME_ENV: &str = "TAKO_1886_HOME";
/// スクリプトが製品の口 `tako mod install` で skills/tako の写しを置いた設定 dir（空白と日本語を含む）
const SKILLS_CFG_ENV: &str = "TAKO_1886_SKILLS_CFG";

/// 偽の API へ渡すダミーの鍵（本物の認証は使わない。末尾 20 文字を事前承認に書く）
const DUMMY_KEY: &str = "sk-ant-tako1886-dummy-key-0000000000";
/// 空白と日本語を含む置き場の名前（利用者名に空白・日本語がある PC の data dir と同じ形）
const ODD_DIR: &str = "tako 1886 データ";
/// 利用者側の plugin（`;` 連結の 2 本目）。hooks module を持つので、読まれると Claude Code が
/// その dir に `.claude-plugin/types/` を書く = 読まれたかを外から判定できる
const PROBE_PLUGIN: &str = "probe1886";

/// 状態を待つ上限（混んだランナーで claude の起動が遅い回を許す。尽きたら画面を添えて落ちる）
const STATE_WAIT: Duration = Duration::from_secs(90);
/// 対照（off）で「報告が来ない」を見る窓（mod は起動から 1 秒で最初の報告を送る）
const QUIET_WINDOW: Duration = Duration::from_secs(20);

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!(
            "{name} が無い。このテストは scripts/windows/test-claude-mod-1886.ps1 が砂場を用意してから --ignored で回す"
        )
    })
}

/// スクリプトが渡す材料
struct Given {
    claude: PathBuf,
    tako: PathBuf,
    api_url: String,
    home: PathBuf,
    skills_cfg: PathBuf,
}

fn given() -> Given {
    let given = Given {
        claude: PathBuf::from(required(CLAUDE_EXE_ENV)),
        tako: PathBuf::from(required(TAKO_EXE_ENV)),
        api_url: required(API_URL_ENV),
        home: PathBuf::from(required(HOME_ENV)),
        skills_cfg: PathBuf::from(required(SKILLS_CFG_ENV)),
    };
    assert!(
        given.claude.is_file(),
        "claude.exe が無い: {}",
        given.claude.display()
    );
    assert!(
        given.tako.is_file(),
        "tako.exe が無い: {}",
        given.tako.display()
    );
    given
}

// --- ログ（パスを伏せる） -------------------------------------------------------

/// CI のログへ出す前にパスを伏せる（砂場の根・USERPROFILE・利用者名）
struct Masker {
    pairs: Vec<(String, &'static str)>,
}

impl Masker {
    fn new(scratch: &[&Path], home: &Path) -> Self {
        let mut pairs: Vec<(String, &'static str)> = scratch
            .iter()
            .map(|p| (p.display().to_string(), "<scratch>"))
            .collect();
        pairs.push((home.display().to_string(), "<home>"));
        // スクリプトの砂場の根（home と skills-dir の設定 dir の親）
        if let Some(run) = home.parent() {
            pairs.push((run.display().to_string(), "<run>"));
        }
        if let Some(profile) = tako_core::paths::home_dir() {
            pairs.push((profile.display().to_string(), "<userprofile>"));
        }
        if let Ok(user) = std::env::var("USERNAME") {
            if user.len() >= 3 {
                pairs.push((user, "<user>"));
            }
        }
        // JSON の中では `\` が `\\` で現れる
        let escaped: Vec<(String, &'static str)> = pairs
            .iter()
            .map(|(from, to)| (from.replace('\\', "\\\\"), *to))
            .collect();
        pairs.extend(escaped);
        // 長いものから置き換える（根の置き換えが中の置き換えを壊さない）
        pairs.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));
        Self { pairs }
    }

    fn mask(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (from, to) in &self.pairs {
            if !from.is_empty() {
                out = out.replace(from.as_str(), to);
            }
        }
        out
    }
}

// --- GUI の代わりの受け口 ------------------------------------------------------

/// 受けた報告 1 本（本文・プロンプトは mod がもともと載せない。値は判定に要るものだけ控える）
#[derive(Debug, Clone)]
struct Seen {
    pane: Option<u64>,
    accepted: String,
    turn: String,
    pending_tool: Option<String>,
    last_turn: Option<String>,
    has_session: bool,
    claude_version: Option<String>,
    mod_version: String,
    config_dir: Option<String>,
}

#[derive(Default)]
struct State {
    hub: ModHub,
    seen: Vec<Seen>,
    /// mod の報告以外に届いた要求の method（paste-guard 等。本文は控えない）
    others: Vec<String>,
}

/// GUI の受け口の代役。名前付きパイプの IPC（GUI と同じ `IpcServer`）で受け、mod の報告だけを
/// dispatch と同じ関数（`parse_report` → `ModHub::accept`）で hub へ入れる
struct Receiver {
    _server: IpcServer,
    endpoint: String,
    token: String,
    state: Arc<Mutex<State>>,
}

impl Receiver {
    fn start() -> Self {
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let pipe = format!(r"\\.\pipe\tako-1886-{}", std::process::id());
        let token = format!("tako1886-token-{}", std::process::id());
        let server = IpcServer::start_at(tx, token.clone(), &pipe)
            .expect("名前付きパイプの受け口を立てられること");
        let state = Arc::new(Mutex::new(State {
            hub: ModHub::new(true),
            ..State::default()
        }));
        let shared = Arc::clone(&state);
        std::thread::spawn(move || {
            while let Some(incoming) = futures::executor::block_on(rx.next()) {
                let reply = answer(&shared, incoming.request);
                let _ = incoming.reply.send(reply);
            }
        });
        Self {
            endpoint: server.endpoint().to_string(),
            _server: server,
            token,
            state,
        }
    }

    fn seen_for(&self, pane: u64) -> Vec<Seen> {
        let state = self.state.lock().unwrap();
        state
            .seen
            .iter()
            .filter(|s| s.pane == Some(pane))
            .cloned()
            .collect()
    }

    /// 応答の `mod_turn` / `mod_reason`（dispatch の `read_pane` と同じ 2 関数で組む）
    fn mod_turn(&self, pane: u64) -> (Value, Value) {
        let state = self.state.lock().unwrap();
        let now = Instant::now();
        let looked_up = state.hub.lookup(pane, now);
        (
            tako_control::claude_mod::turn_json(&looked_up, now),
            tako_control::claude_mod::reason_code(&looked_up),
        )
    }

    fn record_spawn(&self, pane: u64, injection: &cm::Injection) {
        self.state.lock().unwrap().hub.record_spawn(pane, injection);
    }
}

fn answer(state: &Mutex<State>, request: Request) -> Result<Value, DispatchError> {
    match request {
        Request::Mod {
            action: Some(action),
            report: Some(body),
            pane,
            ..
        } if action == "report" => {
            let report = cm::parse_report(body).map_err(DispatchError::InvalidParams)?;
            let mut state = state.lock().unwrap();
            let seen = Seen {
                pane,
                accepted: String::new(),
                turn: report.turn.as_str().to_string(),
                pending_tool: report.pending_tool.clone(),
                last_turn: report
                    .last_turn
                    .as_ref()
                    .map(|t| format!("{}:{}ms", t.reason, t.duration_ms)),
                has_session: report.session_id.as_deref().is_some_and(|s| !s.is_empty()),
                claude_version: report.claude_version.clone(),
                mod_version: report.mod_version.clone(),
                config_dir: report.config_dir.clone(),
            };
            let accepted = state
                .hub
                .accept(pane.unwrap_or(0), report, Instant::now())
                .as_str();
            state.seen.push(Seen {
                accepted: accepted.to_string(),
                ..seen
            });
            Ok(json!({ "accepted": accepted, "pane": pane }))
        }
        other => {
            let method = serde_json::to_value(&other)
                .ok()
                .and_then(|v| v["method"].as_str().map(str::to_string))
                .unwrap_or_else(|| "?".into());
            state.lock().unwrap().others.push(method);
            Err(DispatchError::Operation(
                "#1886 の受け口は mod の報告だけを受ける".into(),
            ))
        }
    }
}

// --- 砂場 ---------------------------------------------------------------------

struct Sandbox {
    _scratch: ScratchDir,
    /// 空白と日本語を含む根（data dir・CLI・設定 dir・作業 dir の親）
    odd: PathBuf,
    data_dir: PathBuf,
    plugin_dir: PathBuf,
    cli: PathBuf,
    work: PathBuf,
    probe: PathBuf,
    masker: Masker,
    claude_version: String,
}

fn sandbox(given: &Given) -> Sandbox {
    let scratch = ScratchDir::new("issue1886");
    // 長い形のパスで組む（TEMP は 8.3 の短縮形（`<利用者名の頭>~1`）のことがあり、ペインの PowerShell を経ると claude の
    // 作業 dir が長い形になって、事前信頼の鍵と区切り以外でも食い違う = この測りたいことと別の差）
    let root = tako_core::platform::path::canonicalize(scratch.path())
        .unwrap_or_else(|_| scratch.path().to_path_buf());
    let odd = root.join(ODD_DIR);
    let data_dir = odd.join("data");
    let work = odd.join("work");
    std::fs::create_dir_all(&work).unwrap();

    // 展開: GUI 起動時の `claude_mod::install()` の本体（`<data_dir>/claude-mod/tako/`）
    let mod_root = data_dir.join("claude-mod");
    let outcome = cm::install_in(&mod_root, env!("CARGO_PKG_VERSION"))
        .expect("tako mod を空白と日本語を含む data dir へ展開できること");
    let plugin_dir = mod_root.join(cm::MOD_NAME);
    println!(
        "TAKO1886_INSTALL outcome={outcome:?} plugin_json={}",
        plugin_dir
            .join(".claude-plugin")
            .join("plugin.json")
            .is_file()
    );

    // CLI: 空白と日本語を含む置き場へ写す（%LOCALAPPDATA%\Programs\tako の利用者名に空白・日本語がある PC と同じ形）
    let bin = odd.join("bin dir");
    std::fs::create_dir_all(&bin).unwrap();
    let cli = bin.join(tako_core::shell_integration::cli_file_name());
    std::fs::copy(&given.tako, &cli).expect("tako.exe を写せること");
    // 初回起動の重さ（Defender の走査）を mod の報告の上限の内側に持ち込まない
    let warm = Command::new(&cli)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .expect("写した tako.exe を起こせること");
    println!(
        "TAKO1886_CLI warm_exit={:?} version=[{}]",
        warm.status.code(),
        String::from_utf8_lossy(&warm.stdout).trim()
    );

    // 利用者側の plugin（hooks module を 1 つ持つだけの最小の mod）
    let probe = scratch.path().join("user plugins").join(PROBE_PLUGIN);
    write_probe_plugin(&probe);

    let version_out = Command::new(&given.claude)
        .arg("--version")
        .env("DISABLE_AUTOUPDATER", "1")
        .stdin(Stdio::null())
        .output()
        .expect("claude --version を起こせること");
    let version_text = String::from_utf8_lossy(&version_out.stdout).to_string();
    let (major, minor, patch) = cm::parse_version(version_text.trim())
        .unwrap_or_else(|| panic!("claude --version の版を読めない: {version_text}"));
    let claude_version = format!("{major}.{minor}.{patch}");
    let masker = Masker::new(&[scratch.path(), root.as_path()], &given.home);
    // GUI の版の控え（`quick_claude_version`）は置き場の symlink の先のパスから版を読み、
    // 読めなければ背景で PE の版情報 → `claude --version` へ落ちる。Windows の置き場の形を記録する
    let canonical = tako_core::platform::path::canonicalize(&given.claude).ok();
    println!(
        "TAKO1886_CLAUDE version={claude_version} raw=[{}] file_version={:?} canonical_has_versions_segment={}",
        version_text.trim(),
        tako_core::platform::exe::file_version(&given.claude),
        canonical.as_ref().is_some_and(|p| {
            p.components()
                .any(|c| c.as_os_str().to_string_lossy() == "versions")
        })
    );
    Sandbox {
        _scratch: scratch,
        odd,
        data_dir,
        plugin_dir,
        cli,
        work,
        probe,
        masker,
        claude_version,
    }
}

fn write_probe_plugin(dir: &Path) {
    std::fs::create_dir_all(dir.join(".claude-plugin")).unwrap();
    std::fs::create_dir_all(dir.join("hooks")).unwrap();
    std::fs::create_dir_all(dir.join("commands")).unwrap();
    std::fs::write(
        dir.join(".claude-plugin").join("plugin.json"),
        format!(
            r#"{{"name":"{PROBE_PLUGIN}","version":"0.0.1","description":"tako #1886 probe (a second entry in CLAUDE_CODE_PLUGIN_DIRS)"}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("hooks").join("hooks.json"),
        r#"{ "modules": ["./register.ts"] }"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("hooks").join("register.ts"),
        "import type { Register } from 'claude-code'\n\n\
         export const register: Register = on => {\n  \
         on('session.start', async ($, e, next) => next(e)).catch(($, e, next) => next(e))\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("commands").join(format!("{PROBE_PLUGIN}.md")),
        "---\ndescription: tako #1886 probe\n---\nprobe\n",
    )
    .unwrap();
}

/// オンボーディング済み・ダミーの鍵を承認済みにした `.claude.json`（信頼は書かない）
fn onboarded_config() -> Value {
    let tail: String = DUMMY_KEY
        .chars()
        .rev()
        .take(20)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    json!({
        "hasCompletedOnboarding": true,
        "theme": "dark",
        "customApiKeyResponses": { "approved": [tail], "rejected": [] },
    })
}

/// case ごとの設定 dir（空白と日本語を含む）。オンボーディング済み・ダミーの鍵を承認済みにし、
/// 作業 dir の信頼は製品の事前信頼（`claude_tui::ensure_trusted_in`）で書く。`existing` は
/// スクリプトが `tako mod install` で写しを置いた dir（skills-dir の回）
fn config_dir(sb: &Sandbox, case: &str, existing: Option<&Path>) -> PathBuf {
    let cfg = existing.map_or_else(|| sb.odd.join(format!("cfg {case}")), Path::to_path_buf);
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join(".claude.json"),
        serde_json::to_string_pretty(&onboarded_config()).unwrap(),
    )
    .unwrap();
    let cfg_text = cfg.display().to_string();
    tako_control::claude_tui::ensure_trusted_in(
        Some(cfg_text.as_str()),
        &sb.work.display().to_string(),
    )
    .expect("製品の事前信頼を書けること");
    cfg
}

// --- 1 つの組み合わせ ---------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Load {
    Inline,
    InlineConcat,
    SkillsDir,
    Off,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Launch {
    /// ペインの command = claude.exe（spawn / master / 実行ペインと同じ形）
    Direct,
    /// ペインの PowerShell 5.1 から打つ（利用者がペインで `claude` を打つ形。env はシェルを経て届く）
    ViaShell,
}

struct Case {
    name: &'static str,
    load: Load,
    launch: Launch,
    /// -p のペイン番号（対話とは分ける）
    print_pane: u64,
    pane: u64,
}

struct Outcome {
    name: &'static str,
    print_plugins: Vec<String>,
    print_reports: usize,
    reports: Vec<Seen>,
    first_mod_turn: Value,
    mod_reason: Value,
    saw_permission: Option<String>,
    saw_last_turn: Option<String>,
    saw_busy: bool,
    permission_dialog: bool,
    ended: bool,
    probe_loaded: bool,
    env_dump: Option<Vec<String>>,
    expected_env: Vec<String>,
    trust_dialog: bool,
    others: Vec<String>,
}

/// ペインの env（GUI の `spawn_session` と同じ順: TAKO_PANE_ID → IPC → mod の注入）と、
/// 砂場へ閉じ込める env（claude の書き込み先・偽の API）
fn pane_env(
    sb: &Sandbox,
    given: &Given,
    rx: &Receiver,
    case: &Case,
    pane: u64,
    cfg: &Path,
) -> (Vec<(String, String)>, cm::Injection) {
    let mut env: Vec<(String, String)> = vec![
        ("TAKO_PANE_ID".into(), pane.to_string()),
        ("TAKO_SOCKET".into(), rx.endpoint.clone()),
        ("TAKO_TOKEN".into(), rx.token.clone()),
        ("CLAUDE_CONFIG_DIR".into(), cfg.display().to_string()),
    ];
    // 注入の判断: GUI の ModHub と同じ材料（展開先・CLI・claude の版）
    let mut hub = ModHub::new(true);
    hub.plugin_dir = (case.load != Load::SkillsDir).then(|| sb.plugin_dir.clone());
    hub.cli = Some(sb.cli.clone());
    hub.claude_version = Some(sb.claude_version.clone());
    let injection = mod_install::refine_injection(hub.decide(case.load == Load::Off), &env);
    let inherited = (case.load == Load::InlineConcat).then(|| sb.probe.display().to_string());
    env.extend(cm::pane_env(
        &injection,
        inherited.as_deref(),
        None,
        cm::LIST_SEP,
    ));
    if case.load == Load::SkillsDir {
        // 写しから読ませる回: 展開先の env は無く、mod の起床条件（TAKO_PANE_ID / TAKO_CLI）だけ持つ
        // = 設計書 §9.4 の「env 注入なし・skills/tako の写しだけ」と同じ
        env.push((cm::CLI_ENV.into(), sb.cli.display().to_string()));
    }
    let home = given.home.display().to_string();
    env.extend([
        ("USERPROFILE".into(), home.clone()),
        ("HOME".into(), home.clone()),
        (
            "APPDATA".into(),
            given
                .home
                .join("AppData")
                .join("Roaming")
                .display()
                .to_string(),
        ),
        (
            "LOCALAPPDATA".into(),
            given
                .home
                .join("AppData")
                .join("Local")
                .display()
                .to_string(),
        ),
        ("ANTHROPIC_API_KEY".into(), DUMMY_KEY.into()),
        ("ANTHROPIC_BASE_URL".into(), given.api_url.clone()),
        ("DISABLE_AUTOUPDATER".into(), "1".into()),
        (
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".into(),
            "1".into(),
        ),
        (
            "TAKO_1886_CLAUDE".into(),
            given.claude.display().to_string(),
        ),
        // テストは pwsh 7 から起きるので、その PSModulePath を受け継いだペインの PowerShell 5.1 は標準モジュールを
        // 読めない（GUI のペインは GUI の env を継ぐのでこうならない）。5.1 の既定の置き場へ戻す
        ("PSModulePath".into(), windows_powershell_module_path()),
    ]);
    (env, injection)
}

fn windows_powershell_module_path() -> String {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let program_files =
        std::env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".into());
    format!(
        "{program_files}\\WindowsPowerShell\\Modules;{root}\\system32\\WindowsPowerShell\\v1.0\\Modules"
    )
}

fn plugin_dirs_value(env: &[(String, String)]) -> Option<String> {
    env.iter()
        .rev()
        .find(|(k, _)| k == cm::PLUGIN_DIRS_ENV)
        .map(|(_, v)| v.clone())
}

/// `claude -p --output-format stream-json` の init の行の `plugins`（名前 = パス）
fn run_print(sb: &Sandbox, given: &Given, env: &[(String, String)]) -> (Vec<String>, String) {
    let mut cmd = Command::new(&given.claude);
    cmd.args([
        "-p",
        "tako1886 probe",
        "--output-format",
        "stream-json",
        "--verbose",
        "--allowedTools",
        "Bash,PowerShell",
    ])
    .current_dir(&sb.work)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("claude -p を起こせること");
    // 読み手を先に立てる（stream-json の init の行はパイプの容量を超えうる = 読まずに待つと claude が書き込みで止まる）
    let drain = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = std::io::Read::read_to_end(&mut pipe, &mut buf);
            }
            buf
        })
    };
    let out_reader = drain(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
    );
    let err_reader = drain(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
    );
    let deadline = Instant::now() + STATE_WAIT;
    let status = loop {
        if let Some(status) = child.try_wait().expect("claude -p の状態") {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let stdout = String::from_utf8_lossy(&out_reader.join().unwrap_or_default()).to_string();
    let stderr = String::from_utf8_lossy(&err_reader.join().unwrap_or_default()).to_string();
    let mut plugins = Vec::new();
    let mut init_keys = String::new();
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v["type"] == "system" && v["subtype"] == "init" {
            init_keys = v
                .as_object()
                .map(|o| o.keys().cloned().collect::<Vec<_>>().join(","))
                .unwrap_or_default();
            for p in v["plugins"].as_array().into_iter().flatten() {
                plugins.push(format!(
                    "{}={}",
                    p["name"].as_str().unwrap_or("?"),
                    p["path"].as_str().unwrap_or("?")
                ));
            }
            let slash: Vec<String> = v["slash_commands"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| c.as_str())
                .filter(|c| c.contains(PROBE_PLUGIN) || c.contains("tako"))
                .map(str::to_string)
                .collect();
            println!("TAKO1886_PRINT_SLASH [{}]", slash.join(","));
        }
    }
    let summary = format!(
        "exit={:?} init_keys=[{init_keys}] stderr=[{}]",
        status.map(|s| s.code()),
        stderr
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(300)
            .collect::<String>()
    );
    (plugins, summary)
}

/// 対話のペイン（ConPTY）。イベントを捌きながら画面を返す
struct Pane {
    session: TerminalSession,
    rx: futures::channel::mpsc::UnboundedReceiver<tako_core::terminal::SessionEvent>,
    exited: bool,
    /// 画面の推移（空でない・直前と違う画面を最後の数枚だけ控える。落ちたときにログへ出す）
    screens: Vec<String>,
}

impl Pane {
    fn spawn(sb: &Sandbox, given: &Given, case: &Case, env: Vec<(String, String)>) -> Self {
        let command = match case.launch {
            Launch::Direct => SpawnCommand {
                program: given.claude.display().to_string(),
                // 落ちたときの手がかり（<設定 dir>/debug/ に書く。挙動は変えない）
                args: vec![
                    "--debug".into(),
                    // 2.1.319 の対話の既定は auto mode で、偽の API の経路ではサーバ側の安全確認が通らず
                    // 標準でない確認が出る。権限の確認（turn=permission）の経路を測るため既定のモードで起こす
                    "--permission-mode".into(),
                    "default".into(),
                ],
            },
            Launch::ViaShell => SpawnCommand {
                program: std::path::PathBuf::from(
                    std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into()),
                )
                .join("System32\\WindowsPowerShell\\v1.0\\powershell.exe")
                .display()
                .to_string(),
                args: vec!["-NoLogo".into(), "-NoProfile".into()],
            },
        };
        let (session, rx) = TerminalSession::spawn(
            120,
            40,
            SpawnOptions {
                command: Some(command),
                cwd: Some(sb.work.clone()),
                env,
                scrollback_lines: None,
            },
        )
        .expect("ConPTY のペインを起動できること");
        Self {
            session,
            rx,
            exited: false,
            screens: Vec::new(),
        }
    }

    fn pump(&mut self) -> String {
        while let Ok(ev) = self.rx.try_recv() {
            if let Some(SessionNotice::Exited) = self.session.process_event(ev) {
                self.exited = true;
            }
        }
        let screen = self.session.visible_lines().join("\n");
        if !screen.trim().is_empty() && self.screens.last() != Some(&screen) {
            self.screens.push(screen.clone());
            if self.screens.len() > 6 {
                self.screens.remove(0);
            }
        }
        screen
    }

    /// 落ちたときの材料（画面の推移・終了コード・alt screen）
    fn dump(&self, label: &str, masker: &Masker) {
        println!(
            "TAKO1886_PANE {label} exited={} exit_status={:?} alt_screen={} screens={}",
            self.exited,
            self.session.exit_status(),
            self.session.is_alt_screen(),
            self.screens.len()
        );
        for (i, screen) in self.screens.iter().enumerate() {
            let body: Vec<&str> = screen.lines().filter(|l| !l.trim().is_empty()).collect();
            println!(
                "TAKO1886_SCREEN {label} #{i}:\n{}",
                masker.mask(&body.join("\n"))
            );
        }
    }

    /// 文字を打ってから Enter を分けて送る（一括で書くと改行が送信と解釈されないことがある = claude_tui の作法）
    fn type_line(&mut self, text: &str) {
        self.session.write(text.as_bytes().to_vec());
        self.wait(Duration::from_millis(400));
        self.session.write(b"\r".to_vec());
    }

    fn wait(&mut self, span: Duration) {
        let until = Instant::now() + span;
        while Instant::now() < until {
            self.pump();
            std::thread::sleep(Duration::from_millis(30));
        }
    }
}

/// 対話の起動直後に出うる確認に答える（事前の設定で出ない想定。出たら記録して先へ進める）
fn answer_dialogs(pane: &mut Pane, screen: &str, trust_dialog: &mut bool, masker: &Masker) {
    let lower = screen.to_lowercase();
    let shown = || {
        let body: Vec<&str> = screen.lines().filter(|l| !l.trim().is_empty()).collect();
        masker.mask(&body.join("\n"))
    };
    if lower.contains("trust") && (lower.contains("folder") || lower.contains("files")) {
        if !*trust_dialog {
            println!("TAKO1886_DIALOG kind=trust screen=\n{}", shown());
        }
        *trust_dialog = true;
        // 既定の選択は「No, exit」なので、↓ で「Yes, I trust this folder」へ移ってから確定する
        pane.session.write(b"\x1b[B".to_vec());
        pane.wait(Duration::from_millis(400));
        pane.session.write(b"\r".to_vec());
        pane.wait(Duration::from_millis(800));
    } else if lower.contains("api key") && lower.contains("use this") {
        println!("TAKO1886_DIALOG kind=api_key screen=\n{}", shown());
        // 既定は「No」なので 1 を選ぶ
        pane.session.write(b"1".to_vec());
        pane.wait(Duration::from_millis(500));
    } else if lower.contains("press enter to continue") || lower.contains("text style") {
        println!("TAKO1886_DIALOG kind=continue screen=\n{}", shown());
        pane.session.write(b"\r".to_vec());
        pane.wait(Duration::from_millis(500));
    }
}

fn wait_report(
    rx: &Receiver,
    pane: &mut Pane,
    pane_id: u64,
    trust_dialog: &mut bool,
    pred: impl Fn(&[Seen]) -> bool,
    what: &str,
    masker: &Masker,
) -> bool {
    let deadline = Instant::now() + STATE_WAIT;
    let mut next_dialog_check = Instant::now() + Duration::from_secs(2);
    loop {
        let screen = pane.pump();
        if pred(&rx.seen_for(pane_id)) {
            return true;
        }
        if Instant::now() >= next_dialog_check {
            next_dialog_check = Instant::now() + Duration::from_secs(2);
            answer_dialogs(pane, &screen, trust_dialog, masker);
        }
        if Instant::now() >= deadline || pane.exited {
            println!("TAKO1886_TIMEOUT pane={pane_id} waiting_for=[{what}]");
            pane.dump(&format!("pane={pane_id}"), masker);
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// claude が `.claude.json` に書いた `projects` の鍵（事前信頼の鍵の形が claude と合っているか）
fn claude_json_projects(cfg: &Path, case: &str, masker: &Masker) {
    let Ok(text) = std::fs::read_to_string(cfg.join(".claude.json")) else {
        println!("TAKO1886_CLAUDE_JSON case={case} (読めない)");
        return;
    };
    let Ok(root) = serde_json::from_str::<Value>(&text) else {
        println!("TAKO1886_CLAUDE_JSON case={case} (JSON でない)");
        return;
    };
    for (key, entry) in root["projects"].as_object().into_iter().flatten() {
        println!(
            "TAKO1886_CLAUDE_JSON case={case} project=[{}] trust={}",
            masker.mask(key),
            entry["hasTrustDialogAccepted"]
        );
    }
}

/// `--debug` の記録のうち、読み込み・フック・信頼・終了に関わる行（落ちたときだけ）
fn debug_tail(cfg: &Path, case: &str, masker: &Masker) {
    let files: Vec<PathBuf> = std::fs::read_dir(cfg.join("debug"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    let pattern = ["plugin", "hook", "module", "trust", "exit", "error", "tako"];
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let lines: Vec<&str> = text
            .lines()
            .filter(|l| {
                let lower = l.to_lowercase();
                pattern.iter().any(|p| lower.contains(p))
            })
            .collect();
        let start = lines.len().saturating_sub(40);
        for line in &lines[start..] {
            let one: String = line.chars().take(300).collect();
            println!("TAKO1886_DEBUG case={case} | {}", masker.mask(&one));
        }
    }
}

fn run_case(sb: &Sandbox, given: &Given, rx: &Receiver, case: &Case) -> Outcome {
    let existing = (case.load == Load::SkillsDir).then_some(given.skills_cfg.as_path());
    let cfg = config_dir(sb, case.name, existing);
    if case.load == Load::SkillsDir {
        // 写しはスクリプトが製品の口 `tako mod install`（setup の段・MCP tako_mod と同じ run_action → sync_targets）で
        // 置いた。印と中身がこの tako の版のものか（読み直すだけ。書かない）
        let state = tako_core::claude_mod_install::read_copy(&cfg.join("skills").join("tako"));
        println!("TAKO1886_SKILLS case={} copy={state:?}", case.name);
        assert!(
            cfg.join("skills")
                .join("tako")
                .join(".tako-managed")
                .is_file(),
            "[#1886 {}] skills/tako の写しが置かれていない（tako mod install の段を見る）",
            case.name
        );
    }

    // -p（init の plugins）
    let (print_env, _) = pane_env(sb, given, rx, case, case.print_pane, &cfg);
    let (print_plugins, print_summary) = run_print(sb, given, &print_env);
    let print_plugins: Vec<String> = print_plugins.iter().map(|p| sb.masker.mask(p)).collect();
    println!(
        "TAKO1886_PRINT case={} plugin_dirs=[{}] plugins=[{}] {}",
        case.name,
        sb.masker
            .mask(&plugin_dirs_value(&print_env).unwrap_or_default()),
        print_plugins.join(" | "),
        sb.masker.mask(&print_summary)
    );
    // -p の報告（session.end の最後の 1 回が間に合うよう少し待つ）
    std::thread::sleep(Duration::from_secs(2));
    let print_seen = rx.seen_for(case.print_pane);
    for r in &print_seen {
        println!(
            "TAKO1886_PRINT_REPORT case={} accepted={} turn={} last_turn={:?} session_id={} claude_version={:?} mod_version={}",
            case.name, r.accepted, r.turn, r.last_turn, r.has_session, r.claude_version, r.mod_version
        );
    }
    let print_reports = print_seen.len();

    // 対話のペイン
    let (env, injection) = pane_env(sb, given, rx, case, case.pane, &cfg);
    rx.record_spawn(case.pane, &injection);
    let expected_env = vec![
        plugin_dirs_value(&env).unwrap_or_default(),
        env.iter()
            .rev()
            .find(|(k, _)| k == cm::CLI_ENV)
            .map(|(_, v)| v.clone())
            .unwrap_or_default(),
        case.pane.to_string(),
        rx.endpoint.clone(),
    ];
    println!(
        "TAKO1886_CASE case={} launch={:?} injection={} plugin_dirs=[{}] tako_cli=[{}]",
        case.name,
        case.launch,
        if injection.is_on() { "on" } else { "off" },
        sb.masker.mask(&expected_env[0]),
        sb.masker.mask(&expected_env[1]),
    );
    let env_out = sb.odd.join(format!("env {}.txt", case.name));
    let mut env = env;
    env.push(("TAKO_1886_ENV_OUT".into(), env_out.display().to_string()));
    let mut pane = Pane::spawn(sb, given, case, env);
    let mut trust_dialog = false;
    let mut env_dump = None;
    if case.launch == Launch::ViaShell {
        // ペインの PowerShell が受け取った env をそのまま書かせる（打つのは ASCII だけ。値は env から読む）
        let deadline = Instant::now() + STATE_WAIT;
        while !pane.pump().contains("PS ") {
            assert!(
                Instant::now() < deadline && !pane.exited,
                "[#1886 {}] ペインの PowerShell のプロンプトが出ない",
                case.name
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        pane.type_line(
            "[IO.File]::WriteAllText($env:TAKO_1886_ENV_OUT, (@($env:CLAUDE_CODE_PLUGIN_DIRS, $env:TAKO_CLI, $env:TAKO_PANE_ID, $env:TAKO_SOCKET) -join [char]10), (New-Object Text.UTF8Encoding $false))",
        );
        let deadline = Instant::now() + STATE_WAIT;
        let dump = loop {
            pane.pump();
            if let Ok(text) = std::fs::read_to_string(&env_out) {
                if !text.is_empty() {
                    break text;
                }
            }
            assert!(
                Instant::now() < deadline,
                "[#1886 {}] ペインの PowerShell が env を書き出さない",
                case.name
            );
            std::thread::sleep(Duration::from_millis(50));
        };
        env_dump = Some(dump.split('\n').map(str::to_string).collect::<Vec<_>>());
        pane.type_line("& $env:TAKO_1886_CLAUDE --debug --permission-mode default");
    }

    let mut saw_permission = None;
    let mut saw_last_turn = None;
    let mut saw_busy = false;
    let mut permission_dialog = false;
    let mut ended = false;
    let first_mod_turn;
    let mod_reason;
    if case.load == Load::Off {
        // 対照: 信頼の確認に答えて claude の入力欄が出てから、窓のあいだ報告が来ない
        // （確認が出たままだと mod は信頼前で読まれないので、対照として弱い）
        let deadline = Instant::now() + STATE_WAIT;
        let mut next_dialog_check = Instant::now() + Duration::from_secs(2);
        loop {
            let screen = pane.pump();
            let lower = screen.to_lowercase();
            if screen.contains('\u{276F}') && !lower.contains("trust this folder") {
                break;
            }
            if Instant::now() >= next_dialog_check {
                next_dialog_check = Instant::now() + Duration::from_secs(2);
                answer_dialogs(&mut pane, &screen, &mut trust_dialog, &sb.masker);
            }
            if Instant::now() >= deadline || pane.exited {
                println!(
                    "TAKO1886_TIMEOUT pane={} waiting_for=[claude の入力欄]",
                    case.pane
                );
                pane.dump(&format!("pane={}", case.pane), &sb.masker);
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        pane.wait(QUIET_WINDOW);
        let (t, r) = rx.mod_turn(case.pane);
        first_mod_turn = t;
        mod_reason = r;
    } else {
        let got = wait_report(
            rx,
            &mut pane,
            case.pane,
            &mut trust_dialog,
            |s| s.iter().any(|r| r.accepted == "stored"),
            "最初の報告",
            &sb.masker,
        );
        let (t, r) = rx.mod_turn(case.pane);
        first_mod_turn = t;
        mod_reason = r;
        println!(
            "TAKO1886_MOD_TURN case={} got={got} mod_turn={} mod_reason={}",
            case.name, first_mod_turn, mod_reason
        );
        if got && case.load == Load::Inline {
            // ターン: 偽の API はシェルのツールの tool_use を返す = 権限の確認が出る → 許す → 1 ターン終わる
            pane.wait(Duration::from_secs(2));
            pane.type_line("tako1886 hello");
            let deadline = Instant::now() + STATE_WAIT;
            loop {
                let screen = pane.pump();
                let seen = rx.seen_for(case.pane);
                saw_busy |= seen.iter().any(|r| r.turn == "busy");
                if saw_permission.is_none() {
                    if let Some(p) = seen.iter().find(|r| r.turn == "permission") {
                        saw_permission = Some(p.pending_tool.clone().unwrap_or_default());
                        let (t, _) = rx.mod_turn(case.pane);
                        println!(
                            "TAKO1886_MOD_TURN case={} at=permission mod_turn={t}",
                            case.name
                        );
                    }
                }
                let dialog = screen.to_lowercase().contains("do you want to proceed");
                if dialog && !permission_dialog {
                    permission_dialog = true;
                    // mod の報告（1 秒の flush）を待ってから許す
                    pane.wait(Duration::from_millis(2500));
                    continue;
                }
                if dialog {
                    // 既定の「Yes」で許す
                    pane.session.write(b"\r".to_vec());
                    pane.wait(Duration::from_millis(800));
                    continue;
                }
                if seen
                    .iter()
                    .any(|r| r.turn == "idle" && r.last_turn.is_some())
                {
                    saw_last_turn = seen
                        .iter()
                        .rev()
                        .find(|r| r.last_turn.is_some())
                        .and_then(|r| r.last_turn.clone());
                    let (t, _) = rx.mod_turn(case.pane);
                    println!(
                        "TAKO1886_MOD_TURN case={} at=after_turn mod_turn={t}",
                        case.name
                    );
                    break;
                }
                if Instant::now() >= deadline || pane.exited {
                    println!(
                        "TAKO1886_TIMEOUT pane={} waiting_for=[ターンの終わり]",
                        case.pane
                    );
                    pane.dump(&format!("pane={}", case.pane), &sb.masker);
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        if got {
            // 終わり: /exit → session.end の最後の報告（ended）。開いている確認があれば Esc で閉じてから打つ
            pane.session.write(b"\x1b".to_vec());
            pane.wait(Duration::from_millis(600));
            pane.type_line("/exit");
            ended = wait_report(
                rx,
                &mut pane,
                case.pane,
                &mut trust_dialog,
                |s| s.iter().any(|r| r.accepted == "ended"),
                "/exit の最後の報告（ended）",
                &sb.masker,
            );
        }
    }
    drop(pane);
    claude_json_projects(&cfg, case.name, &sb.masker);
    if case.load != Load::Off
        && !rx
            .seen_for(case.pane)
            .iter()
            .any(|r| r.accepted == "stored")
    {
        debug_tail(&cfg, case.name, &sb.masker);
    }

    let reports = rx.seen_for(case.pane);
    for r in &reports {
        println!(
            "TAKO1886_REPORT case={} pane={:?} accepted={} turn={} pending_tool={:?} last_turn={:?} session_id={} claude_version={:?} mod_version={} config_dir_is_case={}",
            case.name,
            r.pane,
            r.accepted,
            r.turn,
            r.pending_tool,
            r.last_turn,
            r.has_session,
            r.claude_version,
            r.mod_version,
            r.config_dir.as_deref() == Some(cfg.display().to_string().as_str())
        );
    }
    let probe_loaded = sb.probe.join(".claude-plugin").join("types").is_dir();
    let others = rx.state.lock().unwrap().others.clone();
    Outcome {
        name: case.name,
        print_plugins,
        print_reports,
        reports,
        first_mod_turn,
        mod_reason,
        saw_permission,
        saw_last_turn,
        saw_busy,
        permission_dialog,
        ended,
        probe_loaded,
        env_dump,
        expected_env,
        trust_dialog,
        others,
    }
}

/// 事前信頼の鍵の A/B（#1886）: 修正前の形（Windows でも作業 dir の文字列そのまま = `\\` 区切り）の鍵だけを
/// 書いた設定 dir で claude を起こすと、信頼の確認が出る（= 修正後の形で出ないことに検出力がある）
fn legacy_trust_shows_dialog(sb: &Sandbox, given: &Given, rx: &Receiver) -> bool {
    let cfg = sb.odd.join("cfg trust-legacy");
    std::fs::create_dir_all(&cfg).unwrap();
    let work = sb.work.display().to_string();
    let legacy_key = tako_control::claude_tui::claude_project_key_in(&work, true, true);
    let fixed_key = tako_control::claude_tui::claude_project_key_in(&work, true, false);
    let mut config = onboarded_config();
    config["projects"] = json!({ legacy_key.clone(): { "hasTrustDialogAccepted": true } });
    std::fs::write(
        cfg.join(".claude.json"),
        serde_json::to_string_pretty(&config).unwrap(),
    )
    .unwrap();
    let case = Case {
        name: "trust-legacy",
        load: Load::Off,
        launch: Launch::Direct,
        print_pane: 51,
        pane: 52,
    };
    let (env, _) = pane_env(sb, given, rx, &case, case.pane, &cfg);
    let mut pane = Pane::spawn(sb, given, &case, env);
    let deadline = Instant::now() + STATE_WAIT;
    let shown = loop {
        let screen = pane.pump().to_lowercase();
        if screen.contains("trust this folder") {
            break true;
        }
        // 入力欄が出た = 確認なしで通った
        if screen.contains('\u{276F}') && screen.contains("claude code v") {
            break false;
        }
        if Instant::now() >= deadline || pane.exited {
            pane.dump("trust-legacy", &sb.masker);
            break false;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    drop(pane);
    println!(
        "TAKO1886_TRUST_AB legacy_key_differs={} legacy_dialog={shown}",
        legacy_key != fixed_key
    );
    shown
}

/// 1 つの組み合わせが期待どおりか。外れたら理由を返す
fn verdict(sb: &Sandbox, case: &Case, o: &Outcome) -> Vec<String> {
    let mut why = Vec::new();
    let stored: Vec<&Seen> = o
        .reports
        .iter()
        .filter(|r| r.accepted == "stored")
        .collect();
    match case.load {
        Load::Off => {
            if !o.reports.is_empty() {
                why.push(format!(
                    "注入しないペインから報告が {} 本届いた（対照が崩れている）",
                    o.reports.len()
                ));
            }
            if !o.first_mod_turn.is_null() {
                why.push(format!(
                    "注入しないペインの mod_turn が null でない: {}",
                    o.first_mod_turn
                ));
            }
            // 注入しなかった理由（A/B の腕 = ab_off）がそのまま mod_reason に出る
            if o.mod_reason != json!("ab_off") {
                why.push(format!(
                    "注入しないペインの mod_reason が ab_off でない: {}",
                    o.mod_reason
                ));
            }
        }
        _ => {
            if stored.is_empty() {
                why.push("対話のペインから mod の報告が 1 本も届かない（読み込み・TAKO_CLI の起動・名前付きパイプのどこかで止まる）".into());
            }
            if o.first_mod_turn.is_null() {
                why.push(format!("mod_turn が null（mod_reason={}）", o.mod_reason));
            } else if o.first_mod_turn["turn"] != "idle" {
                why.push(format!(
                    "最初の mod_turn が idle でない: {}",
                    o.first_mod_turn
                ));
            }
            if let Some(first) = stored.first() {
                if first.mod_version != env!("CARGO_PKG_VERSION") {
                    why.push(format!(
                        "mod_version が展開した版でない: {}",
                        first.mod_version
                    ));
                }
                if first.claude_version.as_deref() != Some(sb.claude_version.as_str()) {
                    why.push(format!(
                        "報告の claude_version が {} でない: {:?}",
                        sb.claude_version, first.claude_version
                    ));
                }
                if !first.has_session {
                    why.push(
                        "報告に session_id が無い（会話カタログの速い経路 #2753 が効かない）"
                            .into(),
                    );
                }
                if first.config_dir.is_none() {
                    why.push("報告に config_dir が無い（ペインの CLAUDE_CONFIG_DIR が mod に届いていない）".into());
                }
            }
            if !o.ended {
                why.push("/exit の最後の報告（ended）が届かない".into());
            }
        }
    }
    // 製品の事前信頼（claude_tui::ensure_trusted_in の鍵 = claude の形）が効き、信頼の確認が出ない
    if o.trust_dialog {
        why.push("製品の事前信頼を書いたのに信頼の確認が出た（鍵の形が claude と合っていない。hooks module は確認が通るまで読まれない）".into());
    }
    // 読み込み元（-p の init の plugins = 名前=パス）
    let tako_entries: Vec<&String> = o
        .print_plugins
        .iter()
        .filter(|p| p.starts_with("tako="))
        .collect();
    let inline_entry = format!(
        "tako={}",
        sb.masker.mask(&sb.plugin_dir.display().to_string())
    );
    match case.load {
        Load::Inline | Load::InlineConcat => {
            if tako_entries != [&inline_entry] {
                why.push(format!(
                    "-p の init の plugins に env の展開先の tako が 1 つだけ載っていない: {:?}",
                    o.print_plugins
                ));
            }
        }
        Load::SkillsDir => {
            let from_copy = tako_entries.len() == 1
                && tako_entries[0].ends_with(&format!("cfg skills-cli{}skills{}tako", '\\', '\\'));
            if !from_copy {
                why.push(format!(
                    "-p の init の plugins の tako が skills/tako の写しから読まれていない: {:?}",
                    o.print_plugins
                ));
            }
        }
        Load::Off => {
            if !tako_entries.is_empty() {
                why.push(format!(
                    "注入しない回に tako が読まれた: {:?}",
                    o.print_plugins
                ));
            }
        }
    }
    match case.load {
        Load::Inline => {
            // busy は turn.start から権限の確認までの間だけで、1 秒の flush に畳まれて報告に乗らないことがある
            // （permission が来た = ターンは始まっている）ので記録だけ（表の列）
            match &o.saw_permission {
                Some(tool) if !tool.is_empty() => {}
                other => why.push(format!(
                    "権限の確認（画面に出た = {}）で turn=permission と pending_tool が報告されない: {other:?}",
                    o.permission_dialog
                )),
            }
            if o.saw_last_turn.is_none() {
                why.push("ターンの終わりの last_turn が報告されない".into());
            }
        }
        Load::InlineConcat => {
            if !o
                .print_plugins
                .iter()
                .any(|p| p.starts_with(&format!("{PROBE_PLUGIN}=")))
                || !o.probe_loaded
            {
                why.push(format!(
                    "`;` 連結の 2 本目（利用者の plugin）が読まれていない: plugins={:?} types={}",
                    o.print_plugins, o.probe_loaded
                ));
            }
            match &o.env_dump {
                Some(dump) if dump == &o.expected_env => {}
                other => why.push(format!(
                    "ペインの PowerShell が受け取った env が tako の渡した値と違う: 渡した {:?} / 受け取った {:?}",
                    o.expected_env.iter().map(|v| sb.masker.mask(v)).collect::<Vec<_>>(),
                    other.as_ref().map(|d| d.iter().map(|v| sb.masker.mask(v)).collect::<Vec<_>>())
                )),
            }
        }
        Load::SkillsDir | Load::Off => {}
    }
    why
}

#[test]
#[ignore = "本物の claude を砂場へ入れる scripts/windows/test-claude-mod-1886.ps1 が --ignored で名指しで回す（claude を外部から取るので CI では agent_install の門の中だけ。#1886）"]
fn issue1886_windows_claude_loads_tako_mod_and_reports() {
    let given = given();
    let sb = sandbox(&given);
    let rx = Receiver::start();
    println!(
        "TAKO1886_IPC endpoint_is_named_pipe={} data_dir=[{}]",
        rx.endpoint.starts_with(r"\\.\pipe\"),
        sb.masker.mask(&sb.data_dir.display().to_string())
    );
    let cases = [
        Case {
            name: "inline",
            load: Load::Inline,
            launch: Launch::Direct,
            print_pane: 11,
            pane: 12,
        },
        Case {
            name: "inline-concat",
            load: Load::InlineConcat,
            launch: Launch::ViaShell,
            print_pane: 21,
            pane: 22,
        },
        Case {
            name: "skills-dir",
            load: Load::SkillsDir,
            launch: Launch::Direct,
            print_pane: 31,
            pane: 32,
        },
        Case {
            name: "off",
            load: Load::Off,
            launch: Launch::Direct,
            print_pane: 41,
            pane: 42,
        },
    ];
    let mut failures = Vec::new();
    let mut rows = Vec::new();
    for case in &cases {
        let o = run_case(&sb, &given, &rx, case);
        let why = verdict(&sb, case, &o);
        rows.push(format!(
            "| {} | {:?} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            o.name,
            case.launch,
            o.print_plugins.len(),
            o.print_reports,
            o.reports.iter().filter(|r| r.accepted == "stored").count(),
            o.first_mod_turn["turn"].as_str().unwrap_or("null"),
            o.saw_busy,
            o.saw_permission.as_deref().unwrap_or("-"),
            o.saw_last_turn.as_deref().unwrap_or("-"),
            o.ended,
            o.probe_loaded,
            o.trust_dialog,
        ));
        for w in why {
            failures.push(format!("[#1886 {}] {w}", case.name));
        }
        if !o.others.is_empty() {
            println!("TAKO1886_OTHERS case={} methods={:?}", case.name, o.others);
        }
    }
    if !legacy_trust_shows_dialog(&sb, &given, &rx) {
        failures.push("[#1886 trust-legacy] 修正前の形の鍵だけでは信頼の確認が出るはずが出ない（事前信頼の A/B に検出力が無い）".into());
    }
    println!("== 表（#1886）==");
    println!("| case | 起こし方 | -p の plugins | -p の報告 | 対話の報告 | 最初の turn | busy | permission の tool | last_turn | ended | 利用者の plugin | 信頼の確認 |");
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|");
    for row in &rows {
        println!("{row}");
    }
    if failures.is_empty() {
        println!(
            "TAKO1886_VERDICT mod_loads=true report_reaches=true conpty_env=true claude={} sep={:?}",
            sb.claude_version,
            cm::LIST_SEP
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
