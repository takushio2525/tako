//! 壊れた `profiles/default.yaml` で、引き継ぎの後任・`orchestrator self`・`supervisor status` が
//! 黙って組み込み既定へ落ちない（Issue #2233）
//!
//! 修正前（実測。CLI → IPC → dispatch を GUI 無しのホストで）:
//!
//! - **引き継ぎ**（`dispatch_orchestrator_handoff`）は `Profile::load(..).unwrap_or_default()` で
//!   読み、後任 master を個人ルール・モデル抜きの組み込み既定で立てに行った。応答の `warnings` は
//!   起動フォルダの注意だけ・後任の system prompt にも persist.log にも何も無く、退避もしない
//! - **self / supervisor status** は既定値（閾値 60・自動通知 ON・mode auto…）を、プロファイルの
//!   値であるかのように返した
//!
//! いまは後任を `master_launch::load_successor_profile`（= `tako master` と同じ `load_profile`）で
//! 読み、立てるが応答の `warnings`・persist.log・後任の system prompt に理由を出して中身を退避する。
//! 表示だけの経路は `orchestrator::load_for_display`（読むだけ）の理由を `warnings` へ出す。
//!
//! dispatch は CLI / MCP / GUI が共有する 1 実装なので、ここでは GUI の代わりのホストへ直接
//! 渡す（ペインの分割まで実際に起きる。端末は持たない）。env（data dir・HOME・A/B）は
//! プロセス全体の状態なので、**専用の統合テストバイナリの 1 本のテスト関数**の中で順に見る
//! （`issue2092_encoding_pin_health.rs` と同じ作法）。claude の所在確認は `TAKO_983_LEGACY=1` で
//! 外す（本物の claude を探さない・CI のランナーに claude が無くても同じ経路を通る）。
//! A/B は `TAKO_2233_LEGACY=1`（修正前を同じバイナリで再現する）。

use std::path::{Path, PathBuf};

use serde_json::Value;
use tako_control::protocol::Request;
use tako_control::{
    PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost, UiStateHost, WebViewHost,
    WorkspaceHost,
};
use tako_core::test_residue::ScratchDir;
use tako_core::{Pane, PaneId, PaneOrigin, SpawnOptions, TerminalSession, Workspace};

const RULE: &str = "PERSONAL_RULE_2233";
const GOOD: &str = "effort: max\nmodel: claude-test-model\nprompt_blocks:\n  append: \"PERSONAL_RULE_2233\"\nbypass_sandbox: false\n";
const BROKEN: &str =
    "effort: max\nmodel: claude-test-model\nprompt_blocks:\n  append: \"PERSONAL_RULE_2233\n  prepend: [\n";
const MEMO: &str = "## 知識\n- 引き継ぎの材料（#2233）\n\n## 実行状態\n- なし\n";

/// GUI の代わり: ペインの木だけを持つ（端末は持たない = attach は何もしない）
struct Host {
    workspace: Workspace,
}

impl Host {
    fn new() -> Self {
        Self {
            workspace: Workspace::new("1", Pane::new(PaneOrigin::User)),
        }
    }

    fn root(&self) -> u64 {
        self.workspace.active_tab().tree().focused().as_u64()
    }

    fn pane_count(&self) -> usize {
        self.workspace.active_tab().tree().panes().len()
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
    fn session(&self, _pane: PaneId) -> Option<&TerminalSession> {
        None
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
    /// `default_yaml` が `None` = default.yaml が無い（ゼロコンフィグの初回）
    fn new(default_yaml: Option<&str>, legacy: bool) -> Self {
        let dir = ScratchDir::new("issue2233");
        assert!(
            dir.path().starts_with(std::env::temp_dir()),
            "一時 dir の外に書こうとしている: {}",
            dir.path().display()
        );
        let case = Self { dir };
        for d in [case.profiles(), case.handoff_dir(), case.home()] {
            std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
        }
        if let Some(body) = default_yaml {
            std::fs::write(case.profiles().join("default.yaml"), body).expect("default.yaml");
        }
        std::fs::write(case.handoff_dir().join("default.md"), MEMO).expect("運用メモ");
        std::env::set_var("TAKO_DATA_DIR", case.dir.path());
        std::env::set_var("HOME", case.home());
        if legacy {
            std::env::set_var("TAKO_2233_LEGACY", "1");
        } else {
            std::env::remove_var("TAKO_2233_LEGACY");
        }
        case
    }

    fn orch(&self) -> PathBuf {
        self.dir.path().join("orchestrator")
    }

    fn profiles(&self) -> PathBuf {
        self.orch().join("profiles")
    }

    fn handoff_dir(&self) -> PathBuf {
        self.orch().join("handoff")
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    fn default_yaml(&self) -> PathBuf {
        self.profiles().join("default.yaml")
    }

    fn quarantine(&self, name: &str) -> PathBuf {
        self.profiles().join(format!("{name}.yaml.unreadable.bak"))
    }

    fn prompt(&self, profile: &str) -> String {
        let path = self.orch().join(format!("_system_prompt_{profile}.md"));
        std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "{} が無い = 後任の組み立てまで進んでいない: {e}",
                path.display()
            )
        })
    }

    fn persist_log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("persist.log")).unwrap_or_default()
    }
}

fn warnings(v: &Value) -> Vec<String> {
    v["warnings"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|w| w.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// 既定値で答えた / 立てた理由（default.yaml を名指しした警告）
fn default_warning(v: &Value, path: &Path) -> Option<String> {
    let path = path.display().to_string();
    warnings(v).into_iter().find(|w| w.contains(&path))
}

fn handoff(host: &mut Host, role: &str) -> Result<Value, String> {
    let pane = host.root();
    tako_control::dispatch::dispatch(
        host,
        Request::OrchestratorHandoff {
            pane: Some(pane),
            caller_role: Some(role.to_string()),
            tab: None,
            caller_pid: None,
            projects: None,
        },
        PaneOrigin::User,
    )
    .map_err(|e| e.to_string())
}

fn self_info(host: &mut Host) -> Value {
    let pane = host.root();
    tako_control::dispatch::dispatch(
        host,
        Request::orchestrator_self(None, Some(pane), Some("master".into()), None),
        PaneOrigin::User,
    )
    .expect("self は止まらない")
}

fn supervisor_status(host: &mut Host) -> Value {
    tako_control::dispatch::dispatch(
        host,
        Request::OrchestratorSupervisor {
            action: "status".into(),
            mode: None,
            auto_resume_dead: None,
            max_retries: None,
            lines: None,
        },
        PaneOrigin::User,
    )
    .expect("status は止まらない")
}

/// ① 修正後: 後任は立つが、応答・persist.log・後任の system prompt で黙らない。中身は退避
fn successor_warns() {
    let case = Case::new(Some(BROKEN), false);
    let mut host = Host::new();
    let before = host.pane_count();
    let v = handoff(&mut host, "master").expect("読めない default でも後任は立てる");
    assert_eq!(host.pane_count(), before + 1, "後任のペインが立っていない");
    let w = default_warning(&v, &case.default_yaml())
        .unwrap_or_else(|| panic!("応答の warnings が default.yaml を言わない: {v:#}"));
    assert!(
        w.contains("組み込み既定で master を立てる") && w.contains("default.yaml を直す"),
        "何が抜けるか・直し方が無い: {w}"
    );
    assert!(
        w.contains("default.yaml.unreadable.bak"),
        "退避先を言っていない: {w}"
    );
    let prompt = case.prompt("default");
    assert!(
        prompt.contains("Startup warning") && prompt.contains("組み込み既定で master を立てる"),
        "後任本人へ渡っていない（後任は前任の応答を読まない）"
    );
    assert!(!prompt.contains(RULE), "読めないはずの個人ルールがある");
    let log = case.persist_log();
    assert!(
        log.lines()
            .any(|l| l.contains("[master] default:") && l.contains("組み込み既定")),
        "persist.log に残っていない:\n{log}"
    );
    assert_eq!(
        std::fs::read_to_string(case.quarantine("default")).expect("退避がある"),
        BROKEN
    );
    assert_eq!(
        std::fs::read_to_string(case.default_yaml()).expect("元が残る"),
        BROKEN,
        "default.yaml を書き換えた"
    );
}

/// A/B: `TAKO_2233_LEGACY=1` は修正前（黙って組み込み既定の後任・退避も記録も無し）
fn successor_legacy_is_silent() {
    let case = Case::new(Some(BROKEN), true);
    let mut host = Host::new();
    let v = handoff(&mut host, "master").expect("修正前も立てる");
    assert_eq!(
        default_warning(&v, &case.default_yaml()),
        None,
        "修正前の再現になっていない: {v:#}"
    );
    let prompt = case.prompt("default");
    assert!(!prompt.contains("Startup warning"));
    assert!(!prompt.contains(RULE));
    assert!(!case.persist_log().contains("[master] default:"));
    assert!(!case.quarantine("default").exists(), "修正前は退避しない");
}

/// 回帰なし: 読める default は個人ルールつきで黙って立つ・無い default（初回）も黙る
fn successor_good_and_absent_are_quiet() {
    let case = Case::new(Some(GOOD), false);
    let mut host = Host::new();
    let v = handoff(&mut host, "master").expect("読める default");
    assert_eq!(default_warning(&v, &case.default_yaml()), None, "{v:#}");
    let prompt = case.prompt("default");
    assert!(prompt.contains(RULE), "個人ルールが後任に載っていない");
    assert!(!prompt.contains("Startup warning"));

    let case = Case::new(None, false);
    let mut host = Host::new();
    let v = handoff(&mut host, "master").expect("無い default");
    assert_eq!(default_warning(&v, &case.default_yaml()), None, "{v:#}");
    assert!(!case.prompt("default").contains("Startup warning"));
    assert!(!case.default_yaml().exists(), "読むだけで作らない");
}

/// 専用プロファイル（`master:<key>`）が読めない・無いときも、**引き継ぎは止めずに**組み込み既定で
/// 後任を立てて警告する（前任は ctx の上限に近いので、止めると master そのものを失いうる）。
/// 修正前は組み込み既定の後任を黙って立てていた
fn successor_named_profile_warns() {
    for (body, legacy) in [(Some(BROKEN), false), (None, false), (Some(BROKEN), true)] {
        let case = Case::new(Some(GOOD), legacy);
        let named = case.profiles().join("p2233.yaml");
        if let Some(body) = body {
            std::fs::write(&named, body).expect("p2233.yaml");
        }
        std::fs::write(case.handoff_dir().join("p2233.md"), MEMO).expect("運用メモ");
        let mut host = Host::new();
        let before = host.pane_count();
        let v = handoff(&mut host, "master:p2233").expect("引き継ぎは止めない");
        assert_eq!(host.pane_count(), before + 1, "後任のペインが立っていない");
        let prompt = case.prompt("p2233");
        assert!(!prompt.contains(RULE), "読めないはずの個人ルールがある");
        let w = warnings(&v).into_iter().find(|w| w.contains("'p2233'"));
        if legacy {
            assert_eq!(w, None, "修正前の再現になっていない: {v:#}");
            assert!(!prompt.contains("Startup warning"));
            assert!(!case.persist_log().contains("[master] p2233:"));
            continue;
        }
        let w = w.unwrap_or_else(|| panic!("後任を既定で立てたことを黙っている: {v:#}"));
        let why = if body.is_some() {
            "パース"
        } else {
            "見つからない"
        };
        assert!(
            w.contains(why) && w.contains("組み込み既定") && w.contains("引き継ぎは止めずに"),
            "理由・何で立てたかが無い: {w}"
        );
        assert!(
            prompt.contains("Startup warning") && prompt.contains("'p2233'"),
            "後任本人へ渡っていない"
        );
        assert!(
            case.persist_log()
                .lines()
                .any(|l| l.contains("[master] p2233:") && l.contains("組み込み既定")),
            "persist.log に残っていない"
        );
        if body.is_some() {
            assert_eq!(
                std::fs::read_to_string(&named).expect("元が残る"),
                BROKEN,
                "p2233.yaml を書き換えた"
            );
        }
    }
}

/// ② self / supervisor status: 既定値で答えるが理由を warnings へ（読むだけ・退避しない）
fn display_paths_warn() {
    let case = Case::new(Some(BROKEN), false);
    let mut host = Host::new();
    let me = self_info(&mut host);
    let w = default_warning(&me, &case.default_yaml())
        .unwrap_or_else(|| panic!("self が既定値で答えたことを黙っている: {me:#}"));
    assert!(w.contains("組み込み既定"), "{w}");
    assert_eq!(me["ctx_threshold"], 60, "既定値で答えていない");
    let sv = supervisor_status(&mut host);
    let w = default_warning(&sv, &case.default_yaml())
        .unwrap_or_else(|| panic!("supervisor status が黙っている: {sv:#}"));
    assert!(w.contains("組み込み既定"), "{w}");
    assert_eq!(sv["mode"], "auto");
    assert!(
        !case.quarantine("default").exists(),
        "表示だけの経路が退避を書いている"
    );
    assert!(
        !case.persist_log().contains("default.yaml"),
        "表示だけの経路が persist.log を埋めている"
    );
}

/// A/B と回帰なし: 修正前・読める default・無い default はどれも黙る（読める値はそのまま返る）
fn display_paths_quiet() {
    for (body, legacy) in [(Some(BROKEN), true), (Some(GOOD), false), (None, false)] {
        let case = Case::new(body, legacy);
        let mut host = Host::new();
        let me = self_info(&mut host);
        assert_eq!(default_warning(&me, &case.default_yaml()), None, "{me:#}");
        let sv = supervisor_status(&mut host);
        assert!(sv.get("warnings").is_none(), "{sv:#}");
    }
    let case = Case::new(
        Some("ctx_threshold: 55\nsupervisor_mode: notify_only\n"),
        false,
    );
    let mut host = Host::new();
    assert_eq!(self_info(&mut host)["ctx_threshold"], 55);
    assert_eq!(supervisor_status(&mut host)["mode"], "notify_only");
    drop(case);
}

#[test]
fn issue2233_壊れたdefaultで後任と表示の経路が黙らない() {
    for key in [
        "TAKO_SOCKET",
        "TAKO_PANE_ID",
        "TAKO_TOKEN",
        "TAKO_ORCHESTRATOR_DIR",
        "TAKO_ORCHESTRATOR_ROLE",
        "CLAUDE_CONFIG_DIR",
        "TAKO_2227_LEGACY",
        "TAKO_1825_LEGACY",
    ] {
        std::env::remove_var(key);
    }
    std::env::set_var("TAKO_ISOLATED", "1");
    // claude の所在確認（ログインシェル）を外す
    std::env::set_var("TAKO_983_LEGACY", "1");

    successor_warns();
    successor_legacy_is_silent();
    successor_good_and_absent_are_quiet();
    successor_named_profile_warns();
    display_paths_warn();
    display_paths_quiet();
}
