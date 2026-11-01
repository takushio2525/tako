//! 実経路: 読めないプロファイルの理由（#2256 の系統）が表示言語に従う（Issue #2365 の続き）。
//!
//! 修正前（実測。CLI / MCP と同じ dispatch を GUI 無しのホストで）: 英語表示でも、default.yaml を
//! 読めないときの理由（`<path> を読めない（YAML の構文が壊れている（3 行 11 列））。…組み込み既定で
//! worker を立てる…`）が日本語のまま応答の `warnings`・断りへ載った。check-health の
//! `profiles: check failed (…)` の括弧の中も「プロファイルディレクトリの読み取りに失敗: …」のまま。
//! 理由の文は persist.log（日本語の診断ログ）と共有する日本語の `String` だったため。
//!
//! いまの形: 理由は型（`orchestrator::{UnreadableProfile, ProfileFallback}` と `ConfigError` の
//! 3 種）で持ち、利用者へ返す口（応答の `warnings`・断り・check-health の行）は表示言語、
//! persist.log は日本語で組む。このテストは
//!
//! - 英語表示: その経路の理由の文に日本語（かな・漢字・全角）が 1 字も無い
//! - 日本語表示: 理由の文が #2365 より前と同じ形（「を読めない」＋経路ごとの続き）
//! - どちらの表示言語でも persist.log の行は日本語
//!
//! を、壊れた default.yaml / 読めない default.yaml（パーミッション 000）/ 読めない profiles/ の
//! 3 通りで見る。表示言語・env はプロセス全体の状態なので、**専用の統合テストバイナリの 1 本の
//! テスト関数**の中で順に切り替える（#2256 / #2233 のテストと同じ作法）。修正前の木でもコンパイル
//! できる API だけを使う（A/B: 修正前は英語表示の検査で落ちる）。
//!
//! unix だけ: 読めないファイル・ディレクトリ（パーミッション 000）が unix の前提。文の日英の対は
//! OS に依らない 1 か所（`config_error.rs`）で、両 OS の番犬（`issue1762_dispatch_error_lang.rs`）が
//! 表で見る
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tako_control::protocol::Request;
use tako_control::{
    PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost, UiStateHost, WebViewHost,
    WorkspaceHost,
};
use tako_core::i18n::{self, Lang};
use tako_core::test_residue::ScratchDir;
use tako_core::{Pane, PaneId, PaneOrigin, SpawnOptions, TerminalSession, Workspace};

/// 管轄つきの専用プロファイルがあるプロジェクト（spawn / adopt の採用先）
const PROJECT: &str = "p2365";
/// projects.yaml に登録だけあり、専用プロファイルが無いプロジェクト（#1825 で作れない）
const MISSING: &str = "m2365";
const BROKEN: &str = "effort: max\nprompt_blocks:\n  append: \"RULE\n  prepend: [\n";
const TARGET: &str = "projects:\n- p2365\n";

/// default.yaml の読めなさ（どちらも利用者に見せる理由が出る）
#[derive(Debug, Clone, Copy)]
enum Scenario {
    /// YAML として壊れている
    Broken,
    /// 読めない（パーミッション 000）
    Denied,
}

/// GUI の代わり: ペインの木と、コンフリクト解消の作業フォルダを持つセッション 1 つだけ
struct Host {
    workspace: Workspace,
    session: Option<(PaneId, TerminalSession)>,
}

impl Host {
    fn new() -> Self {
        Self {
            workspace: Workspace::new("1", Pane::new(PaneOrigin::User)),
            session: None,
        }
    }

    fn root(&self) -> u64 {
        self.workspace.active_tab().tree().focused().as_u64()
    }

    fn master() -> Self {
        let mut host = Self::new();
        let id = host.workspace.active_tab().tree().focused();
        host.workspace
            .active_tab_mut()
            .tree_mut()
            .get_mut(id)
            .expect("root")
            .set_role(Some("orchestrator-master".to_string()));
        host
    }

    /// root ペインの作業フォルダを `cwd` にする（解消エージェントは pane の cwd でリポジトリを探す）
    fn seed_cwd(&mut self, cwd: &Path) {
        let (session, _rx) = TerminalSession::spawn(
            80,
            24,
            SpawnOptions {
                cwd: Some(cwd.to_path_buf()),
                ..Default::default()
            },
        )
        .expect("PTY を張れる");
        self.session = Some((PaneId::from_raw(self.root()), session));
    }
}

impl WorkspaceHost for Host {
    fn workspace(&self) -> &Workspace {
        &self.workspace
    }
    fn workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspace
    }
}

impl SessionHost for Host {
    fn session(&self, pane: PaneId) -> Option<&TerminalSession> {
        self.session
            .as_ref()
            .filter(|(id, _)| *id == pane)
            .map(|(_, s)| s)
    }
    fn attach_session(&mut self, _pane: PaneId, _options: SpawnOptions) {}
    fn detach_session(
        &mut self,
        _pane: PaneId,
        _origin: tako_core::pane_log::CloseOrigin,
        _caller: Option<&str>,
    ) {
    }
}

impl TmuxHost for Host {}
impl UiStateHost for Host {}
impl PreviewHost for Host {}
impl WebViewHost for Host {}
impl RemoteHost for Host {}
impl SystemHost for Host {}

/// 1 ケースぶんの隔離した data dir（`TAKO_DATA_DIR` / `HOME` をここへ向ける）
struct Case {
    dir: ScratchDir,
}

impl Case {
    fn new(scenario: Scenario) -> Self {
        let dir = ScratchDir::new("issue2365pu");
        assert!(
            dir.path().starts_with(std::env::temp_dir()),
            "一時 dir の外に書こうとしている: {}",
            dir.path().display()
        );
        let case = Self { dir };
        for d in [case.profiles(), case.home(), case.work()] {
            std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
        }
        std::fs::write(case.default_yaml(), BROKEN).expect("default.yaml");
        if matches!(scenario, Scenario::Denied) {
            set_mode(&case.default_yaml(), 0o000);
        }
        std::fs::write(case.profiles().join(format!("{PROJECT}.yaml")), TARGET).expect("採用先");
        let projects = format!(
            "projects:\n  {PROJECT}:\n    cwd: '{w}'\n  {MISSING}:\n    cwd: '{w}'\n",
            w = case.work().display()
        );
        std::fs::write(case.orch().join("projects.yaml"), projects).expect("projects.yaml");
        std::env::set_var("TAKO_DATA_DIR", case.dir.path());
        std::env::set_var("HOME", case.home());
        case
    }

    fn orch(&self) -> PathBuf {
        self.dir.path().join("orchestrator")
    }

    fn profiles(&self) -> PathBuf {
        self.orch().join("profiles")
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    fn work(&self) -> PathBuf {
        self.dir.path().join("work")
    }

    fn default_yaml(&self) -> PathBuf {
        self.profiles().join("default.yaml")
    }

    fn persist_log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("persist.log")).unwrap_or_default()
    }
}

impl Drop for Case {
    /// 読めなくした所を戻す（戻さないと一時 dir を消せない）
    fn drop(&mut self) {
        set_mode(&self.profiles(), 0o755);
        if self.default_yaml().exists() {
            set_mode(&self.default_yaml(), 0o644);
        }
    }
}

fn set_mode(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .unwrap_or_else(|e| panic!("{} の権限を変えられる: {e}", path.display()));
}

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

fn dispatch(host: &mut Host, req: Request) -> Result<Value, String> {
    tako_control::dispatch::dispatch(host, req, PaneOrigin::User).map_err(|e| e.to_string())
}

/// default.yaml を名指しした警告（文字列の warnings）
fn default_warning(case: &Case, v: &Value) -> Option<String> {
    let path = case.default_yaml().display().to_string();
    v["warnings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| {
            w.as_str()
                .map(str::to_string)
                .or_else(|| w["message"].as_str().map(str::to_string))
        })
        .find(|w| w.contains(&path))
}

/// 理由の文 1 件の検査（英語は日本語なし・日本語は旧文の目印を含む）
fn check(failures: &mut Vec<String>, lang: Lang, label: &str, text: Option<String>, ja_mark: &str) {
    let Some(text) = text else {
        failures.push(format!("{lang:?} {label}: 理由が出ていない"));
        return;
    };
    match lang {
        Lang::En => {
            if let Some(c) = has_japanese(&text) {
                failures.push(format!("{lang:?} {label}: 英語表示に日本語（{c}）: {text}"));
            }
        }
        Lang::Ja => {
            if !text.contains("を読めない") || !text.contains(ja_mark) {
                failures.push(format!(
                    "{lang:?} {label}: 日本語表示が旧文の形でない（「を読めない」「{ja_mark}」）: {text}"
                ));
            }
        }
    }
}

/// persist.log の `tag` の行が日本語のまま（診断ログは日本語固定 = FR-2.42.21）
fn check_log(failures: &mut Vec<String>, lang: Lang, label: &str, case: &Case, tag: &str) {
    let log = case.persist_log();
    let lines: Vec<&str> = log.lines().filter(|l| l.contains(tag)).collect();
    if lines.is_empty() || !lines.iter().all(|l| l.contains("を読めない")) {
        failures.push(format!(
            "{lang:?} {label}: persist.log の {tag} の行が日本語でない: {lines:?}"
        ));
    }
}

fn spawn_request(root: u64) -> Request {
    Request::OrchestratorSpawn {
        project: PROJECT.into(),
        prompt: "2365".into(),
        label: None,
        model: None,
        effort: None,
        pane: Some(root),
        tab: None,
        caller_role: Some("master".into()),
        agent: None,
        caller_pid: None,
        task_type: None,
        account: None,
        limit_resume: None,
    }
}

/// コンフリクトの途中のリポジトリ（`git merge` が衝突で止まった状態）
fn conflicted_repo(dir: &Path) {
    let git = |args: &[&str]| {
        let out = tako_core::platform::process::no_console_window(&mut std::process::Command::new(
            tako_core::git::git_bin(),
        ))
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git を起こせる");
        out.status.success()
    };
    assert!(git(&["init", "-q", "-b", "main"]));
    for kv in [
        ["user.email", "test@example.invalid"],
        ["user.name", "testuser"],
        ["commit.gpgsign", "false"],
    ] {
        assert!(git(&["config", kv[0], kv[1]]));
    }
    let commit = |body: &str, msg: &str| {
        std::fs::write(dir.join("a.txt"), body).expect("書ける");
        assert!(git(&["add", "a.txt"]));
        assert!(git(&["commit", "-q", "-m", msg]));
    };
    commit("base\n", "base");
    assert!(git(&["checkout", "-q", "-b", "other"]));
    commit("other\n", "other");
    assert!(git(&["checkout", "-q", "main"]));
    commit("main\n", "main");
    assert!(!git(&["merge", "-q", "other"]), "衝突で止まるはず");
}

/// 1 つの (表示言語, 読めなさ) で、理由が出る経路を全部通す
fn run(lang: Lang, scenario: Scenario, failures: &mut Vec<String>) {
    i18n::set_lang(lang);
    let at = |name: &str| format!("{scenario:?}/{name}");

    // ① worker の spawn（起動。warnings の kind `profile_unreadable` と persist.log）
    {
        let case = Case::new(scenario);
        let mut host = Host::master();
        let root = host.root();
        match dispatch(&mut host, spawn_request(root)) {
            Ok(v) => {
                check(
                    failures,
                    lang,
                    &at("spawn"),
                    default_warning(&case, &v),
                    "組み込み既定で worker を",
                );
                check_log(failures, lang, &at("spawn"), &case, "[spawn]");
            }
            Err(e) => failures.push(format!("{lang:?} {}: spawn が止まった: {e}", at("spawn"))),
        }
    }

    // ② コンフリクト解消エージェント（起動）
    {
        let case = Case::new(scenario);
        conflicted_repo(&case.work());
        let mut host = Host::new();
        host.seed_cwd(&case.work());
        let root = host.root();
        match dispatch(
            &mut host,
            Request::GitResolveAgent {
                pane: Some(root),
                agent: Some("claude".into()),
                tab: None,
            },
        ) {
            Ok(v) => {
                check(
                    failures,
                    lang,
                    &at("git_resolve"),
                    default_warning(&case, &v),
                    "組み込み既定で コンフリクト解消エージェント を",
                );
                check_log(failures, lang, &at("git_resolve"), &case, "[git-resolve]");
            }
            Err(e) => failures.push(format!("{lang:?} {}: 止まった: {e}", at("git_resolve"))),
        }
    }

    // ③ adopt（判定。起動時のプロファイルを読めないので組み込み既定と比べた）
    {
        let case = Case::new(scenario);
        let mut host = Host::master();
        let root = host.root();
        match dispatch(
            &mut host,
            Request::OrchestratorAdopt {
                name: PROJECT.into(),
                pane: Some(root),
                caller_role: Some("master".into()),
                caller_pid: None,
            },
        ) {
            Ok(v) => check(
                failures,
                lang,
                &at("adopt"),
                default_warning(&case, &v),
                "組み込み既定と比べて採用を判断した",
            ),
            Err(e) => failures.push(format!("{lang:?} {}: 止まった: {e}", at("adopt"))),
        }
        // ③' 採用先が登録だけで専用プロファイルが無く、継承元を読めない（#1825）= 理由で断る
        let mut host = Host::master();
        let root = host.root();
        match dispatch(
            &mut host,
            Request::OrchestratorAdopt {
                name: MISSING.into(),
                pane: Some(root),
                caller_role: Some("master".into()),
                caller_pid: None,
            },
        ) {
            Ok(v) => failures.push(format!(
                "{lang:?} {}: 断らなかった: {v:#}",
                at("adopt_missing")
            )),
            Err(e) => check(
                failures,
                lang,
                &at("adopt_missing"),
                Some(e),
                "default を継承できない",
            ),
        }
    }

    // ④ guide（表示。warnings と local-rules の本文の Detail）
    {
        let case = Case::new(scenario);
        let mut host = Host::new();
        match dispatch(
            &mut host,
            Request::OrchestratorGuide {
                topic: Some("local-rules".into()),
                profile: Some("default".into()),
                caller_role: None,
                pane: None,
                caller_pid: None,
            },
        ) {
            Ok(v) => {
                check(
                    failures,
                    lang,
                    &at("guide"),
                    default_warning(&case, &v),
                    "local-rules",
                );
                let detail = v["text"]
                    .as_str()
                    .and_then(|t| t.split("Detail: ").nth(1))
                    .map(str::to_string);
                check(failures, lang, &at("guide_text"), detail, "local-rules");
            }
            Err(e) => failures.push(format!("{lang:?} {}: 止まった: {e}", at("guide"))),
        }
    }

    // ⑤ self / supervisor status（表示）
    {
        let case = Case::new(scenario);
        let mut host = Host::new();
        let root = host.root();
        match dispatch(
            &mut host,
            Request::orchestrator_self(None, Some(root), Some("master".into()), None),
        ) {
            Ok(v) => check(
                failures,
                lang,
                &at("self"),
                default_warning(&case, &v),
                "組み込み既定",
            ),
            Err(e) => failures.push(format!("{lang:?} {}: 止まった: {e}", at("self"))),
        }
        match dispatch(
            &mut host,
            Request::OrchestratorSupervisor {
                action: "status".into(),
                mode: None,
                auto_resume_dead: None,
                max_retries: None,
                lines: None,
            },
        ) {
            Ok(v) => check(
                failures,
                lang,
                &at("supervisor"),
                default_warning(&case, &v),
                "組み込み既定",
            ),
            Err(e) => failures.push(format!("{lang:?} {}: 止まった: {e}", at("supervisor"))),
        }
    }

    // ⑥ projects add（専用プロファイルを作らなかった理由 = warnings と profile_note・persist.log）
    {
        let case = Case::new(scenario);
        let mut host = Host::new();
        match dispatch(
            &mut host,
            Request::OrchestratorProjects {
                action: "add".into(),
                key: Some("a2365".into()),
                cwd: Some(case.work().display().to_string()),
                description: None,
                servers: Vec::new(),
            },
        ) {
            Ok(v) => {
                check(
                    failures,
                    lang,
                    &at("projects_add"),
                    default_warning(&case, &v),
                    "default を継承できない",
                );
                check(
                    failures,
                    lang,
                    &at("projects_add_note"),
                    v["profile_note"].as_str().map(str::to_string),
                    "default を継承できない",
                );
                check_log(failures, lang, &at("projects_add"), &case, "[projects add]");
            }
            Err(e) => failures.push(format!("{lang:?} {}: 止まった: {e}", at("projects_add"))),
        }
    }

    // ⑦ check-health（profiles/ を読めない = 一覧の失敗の理由）
    {
        let case = Case::new(scenario);
        set_mode(&case.profiles(), 0o000);
        let mut host = Host::new();
        let got = dispatch(&mut host, Request::CheckHealth);
        set_mode(&case.profiles(), 0o755);
        match got {
            Ok(v) => {
                let line = v["diagnostics"]["items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|i| i["key"] == "profiles")
                    .and_then(|i| i["summary"].as_str().map(str::to_string));
                match (lang, line) {
                    (_, None) => failures.push(format!(
                        "{lang:?} {}: profiles の項目が無い: {v:#}",
                        at("check_health")
                    )),
                    (Lang::En, Some(line)) => {
                        if let Some(c) = has_japanese(&line) {
                            failures.push(format!(
                                "{lang:?} {}: 英語表示に日本語（{c}）: {line}",
                                at("check_health")
                            ));
                        }
                    }
                    (Lang::Ja, Some(line)) => {
                        if !line.contains("プロファイルディレクトリの読み取りに失敗")
                        {
                            failures.push(format!(
                                "{lang:?} {}: 旧文の形でない: {line}",
                                at("check_health")
                            ));
                        }
                    }
                }
            }
            Err(e) => failures.push(format!("{lang:?} {}: 止まった: {e}", at("check_health"))),
        }
        drop(case);
    }
}

#[test]
fn issue2365_読めないプロファイルの理由は表示言語に従いpersist_logは日本語のまま() {
    for key in [
        "TAKO_SOCKET",
        "TAKO_PANE_ID",
        "TAKO_TOKEN",
        "TAKO_ORCHESTRATOR_DIR",
        "TAKO_ORCHESTRATOR_ROLE",
        "CLAUDE_CONFIG_DIR",
        "TAKO_2227_LEGACY",
        "TAKO_2233_LEGACY",
        "TAKO_2256_LEGACY",
        "TAKO_1825_LEGACY",
        "TAKO_1453_LEGACY",
    ] {
        std::env::remove_var(key);
    }
    std::env::set_var("TAKO_ISOLATED", "1");
    // claude の所在確認（ログインシェル）を外す
    std::env::set_var("TAKO_983_LEGACY", "1");
    // 実行時の自動移行（#916。1 プロセス 1 回）を先に済ませる（最初のケースだけ移行が
    // default.yaml を読みに行き、persist.log の行がこの経路のものか見分けられなくなる）
    {
        let dir = ScratchDir::new("issue2365pu-warm");
        std::env::set_var("TAKO_DATA_DIR", dir.path());
        std::env::set_var("HOME", dir.path());
        let _ = tako_control::migrations::ensure_migrated();
    }

    let mut failures = Vec::new();
    for lang in [Lang::En, Lang::Ja] {
        for scenario in [Scenario::Broken, Scenario::Denied] {
            run(lang, scenario, &mut failures);
        }
    }
    assert!(
        failures.is_empty(),
        "読めないプロファイルの理由が表示言語に従っていない（{} 件）:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
