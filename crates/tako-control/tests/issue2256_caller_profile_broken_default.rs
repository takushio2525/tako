//! 壊れた `profiles/default.yaml` で、worker の spawn・コンフリクト解消エージェント・adopt の
//! 起動時プロファイル・`orchestrator guide`・master / solo 本人の自動復帰の既定が黙って組み込み
//! 既定へ落ちない（Issue #2256。#2227 / #2233 の残り）
//!
//! 修正前（実測。dispatch を GUI 無しのホストで）: 5 経路とも `Profile::load(..).unwrap_or_default()`
//! （guide は `load_profile_of(..).unwrap_or_else(..)`）で読み、何も言わずに組み込み既定で立てた /
//! 判断した / 答えた。とくに `guide local-rules` は「この環境に on-demand の個人ルールは無い」を
//! 返すので、master は個人ルールが**存在しない**と受け取って動いていた。
//!
//! いまの扱い（`.agent/orchestrator.md` の表）:
//!
//! - spawn / コンフリクト解消エージェント（起動）: `master_launch::load_caller_profile`
//!   （default の起動と同じ退避つきの読み方）。止めずに組み込み既定で立てるが、応答の `warnings`・
//!   persist.log（`[spawn]` / `[git-resolve]`）へ理由を出し、中身を `.unreadable.bak` へ退避する
//! - adopt の起動時プロファイル（判定）・guide（表示）: `orchestrator::read_for_judgment`
//!   （読むだけ）の理由を応答の `warnings` へ。guide の `local-rules` は「読めない」と返す
//! - 自動復帰の既定（判定・role を貼る瞬間に 1 回）: 同じく読むだけで、persist.log へ 1 行
//!
//! dispatch は CLI / MCP / GUI が共有する 1 実装なので、GUI の代わりのホストへ直接渡す（ペインの
//! 分割まで実際に起きる）。env（data dir・HOME・A/B）はプロセス全体の状態なので、**専用の統合
//! テストバイナリの 1 本のテスト関数**の中で順に見る（#2233 のテストと同じ作法）。claude の
//! 所在確認は `TAKO_983_LEGACY=1` で外す（本物の claude を探さない・起動しない）。
//! A/B は `TAKO_2256_LEGACY=1`（修正前を同じバイナリで再現する）。

use std::path::{Path, PathBuf};

use serde_json::Value;
use tako_control::protocol::Request;
use tako_control::{
    PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost, UiStateHost, WebViewHost,
    WorkspaceHost,
};
use tako_core::test_residue::ScratchDir;
use tako_core::{Pane, PaneId, PaneOrigin, SpawnOptions, TerminalSession, Workspace};

const PROJECT: &str = "p2256";
/// on-demand 部に置いた個人ルール（読めれば `guide local-rules` に出る）
const RULE: &str = "RULE_2256";
const GOOD: &str = "effort: max\nmodel: claude-test-model\nlimit_resume: true\nprompt_blocks:\n  append: \"always\\n<!-- tako:on-demand -->\\n## RULE_2256\\nbody\\n\"\n";
const BROKEN: &str =
    "effort: max\nmodel: claude-test-model\nlimit_resume: true\nprompt_blocks:\n  append: \"RULE_2256\n  prepend: [\n";
/// 採用先（管轄つきの専用プロファイル。#1453 で `projects add` が作る形）
const TARGET: &str = "projects:\n- p2256\n";

/// 4 通り（修正後の壊れた default / 修正前の A/B / 読める default / 無い default = 初回）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Variant {
    Broken,
    BrokenLegacy,
    Good,
    Absent,
}

const ALL: [Variant; 4] = [
    Variant::Broken,
    Variant::BrokenLegacy,
    Variant::Good,
    Variant::Absent,
];

impl Variant {
    fn body(self) -> Option<&'static str> {
        match self {
            Self::Broken | Self::BrokenLegacy => Some(BROKEN),
            Self::Good => Some(GOOD),
            Self::Absent => None,
        }
    }

    fn legacy(self) -> bool {
        self == Self::BrokenLegacy
    }

    /// 修正後に理由が出るべきか
    fn warns(self) -> bool {
        self == Self::Broken
    }
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

    fn pane_count(&self) -> usize {
        self.workspace.active_tab().tree().panes().len()
    }

    fn set_root_role(&mut self, role: &str) {
        let id = self.workspace.active_tab().tree().focused();
        self.workspace
            .active_tab_mut()
            .tree_mut()
            .get_mut(id)
            .expect("root")
            .set_role(Some(role.to_string()));
    }

    fn root_limit_autoresume(&self) -> bool {
        let id = self.workspace.active_tab().tree().focused();
        self.workspace
            .active_tab()
            .tree()
            .get(id)
            .expect("root")
            .limit_autoresume()
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
        assert_eq!(
            session.cwd(),
            Some(cwd),
            "起動時 cwd がセッションへ入っていない"
        );
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
    variant: Variant,
}

impl Case {
    fn new(variant: Variant) -> Self {
        let dir = ScratchDir::new("issue2256");
        assert!(
            dir.path().starts_with(std::env::temp_dir()),
            "一時 dir の外に書こうとしている: {}",
            dir.path().display()
        );
        let case = Self { dir, variant };
        for d in [
            case.profiles(),
            case.solo_profiles(),
            case.home(),
            case.work(),
        ] {
            std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
        }
        if let Some(body) = variant.body() {
            std::fs::write(case.default_yaml(), body).expect("default.yaml");
            std::fs::write(case.solo_profiles().join("default.yaml"), body).expect("solo");
        }
        std::fs::write(case.profiles().join(format!("{PROJECT}.yaml")), TARGET).expect("採用先");
        let projects = format!(
            "projects:\n  {PROJECT}:\n    cwd: '{}'\n",
            case.work().display()
        );
        std::fs::write(case.orch().join("projects.yaml"), projects).expect("projects.yaml");
        std::env::set_var("TAKO_DATA_DIR", case.dir.path());
        std::env::set_var("HOME", case.home());
        if variant.legacy() {
            std::env::set_var("TAKO_2256_LEGACY", "1");
        } else {
            std::env::remove_var("TAKO_2256_LEGACY");
        }
        case
    }

    fn orch(&self) -> PathBuf {
        self.dir.path().join("orchestrator")
    }

    fn profiles(&self) -> PathBuf {
        self.orch().join("profiles")
    }

    fn solo_profiles(&self) -> PathBuf {
        self.orch().join("solo-profiles")
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

    fn quarantine(&self) -> PathBuf {
        self.profiles().join("default.yaml.unreadable.bak")
    }

    fn persist_log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("persist.log")).unwrap_or_default()
    }

    /// default.yaml を名指しした persist.log の行（`tag` = 経路の目印）
    fn log_lines(&self, tag: &str) -> Vec<String> {
        let path = self.default_yaml().display().to_string();
        self.persist_log()
            .lines()
            .filter(|l| l.contains(tag) && l.contains(&path))
            .map(str::to_string)
            .collect()
    }

    /// 起動の経路の後始末の検査: 修正後は退避がある・それ以外は無い。元のファイルは触らない
    fn assert_quarantine(&self, expected: bool) {
        assert_eq!(
            self.quarantine().exists(),
            expected,
            "{:?}: 退避の有無が違う",
            self.variant
        );
        if expected {
            assert_eq!(
                std::fs::read_to_string(self.quarantine()).expect("退避がある"),
                BROKEN
            );
        }
        if let Some(body) = self.variant.body() {
            assert_eq!(
                std::fs::read_to_string(self.default_yaml()).expect("元が残る"),
                body,
                "default.yaml を書き換えた"
            );
        }
    }
}

fn dispatch(host: &mut Host, req: Request) -> Result<Value, String> {
    tako_control::dispatch::dispatch(host, req, PaneOrigin::User).map_err(|e| e.to_string())
}

fn str_warnings(v: &Value) -> Vec<String> {
    v["warnings"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|w| w.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// default.yaml を名指しした警告（文字列の warnings）
fn default_warning(case: &Case, v: &Value) -> Option<String> {
    let path = case.default_yaml().display().to_string();
    str_warnings(v).into_iter().find(|w| w.contains(&path))
}

/// ① worker の spawn（起動）: 止めずに立てるが、warnings（kind `profile_unreadable`）・persist.log・退避
fn spawn(variant: Variant) {
    let case = Case::new(variant);
    let mut host = Host::new();
    host.set_root_role("orchestrator-master");
    let before = host.pane_count();
    let root = host.root();
    let v = dispatch(
        &mut host,
        Request::OrchestratorSpawn {
            project: PROJECT.into(),
            prompt: "2256".into(),
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
        },
    )
    .unwrap_or_else(|e| panic!("{variant:?}: 読めない default でも spawn は止めない: {e}"));
    assert_eq!(
        host.pane_count(),
        before + 1,
        "{variant:?}: worker が立っていない"
    );
    let found = v["warnings"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|w| w["kind"] == "profile_unreadable")
        .cloned();
    let logged = case.log_lines("[spawn]");
    if variant.warns() {
        let w = found.unwrap_or_else(|| panic!("spawn が既定で立てたことを黙っている: {v:#}"));
        let msg = w["message"].as_str().unwrap_or_default();
        assert!(
            msg.contains(&case.default_yaml().display().to_string())
                && msg.contains("組み込み既定で worker を")
                && msg.contains("projects の制限"),
            "理由・何が効かないかが無い: {msg}"
        );
        assert_eq!(logged.len(), 1, "persist.log に 1 行だけ残す: {logged:?}");
    } else {
        assert_eq!(found, None, "{variant:?}: 黙るはず: {v:#}");
        assert!(logged.is_empty(), "{variant:?}: {logged:?}");
    }
    case.assert_quarantine(variant.warns());
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

/// ② コンフリクト解消エージェント（起動）: spawn と同じ扱い。warnings は読めないときだけ載る
fn git_resolve(variant: Variant) {
    let case = Case::new(variant);
    conflicted_repo(&case.work());
    let mut host = Host::new();
    host.seed_cwd(&case.work());
    let before = host.pane_count();
    let root = host.root();
    let v = dispatch(
        &mut host,
        Request::GitResolveAgent {
            pane: Some(root),
            agent: Some("claude".into()),
            tab: None,
        },
    )
    .unwrap_or_else(|e| panic!("{variant:?}: 解消エージェントは止めない: {e}"));
    assert_eq!(host.pane_count(), before + 1, "{variant:?}: 立っていない");
    let logged = case.log_lines("[git-resolve]");
    if variant.warns() {
        let w = default_warning(&case, &v)
            .unwrap_or_else(|| panic!("解消エージェントが既定で立てたことを黙っている: {v:#}"));
        assert!(
            w.contains("組み込み既定で コンフリクト解消エージェント を"),
            "{w}"
        );
        assert_eq!(logged.len(), 1, "{logged:?}");
    } else {
        assert!(v.get("warnings").is_none(), "{variant:?}: 黙るはず: {v:#}");
        assert!(logged.is_empty(), "{variant:?}: {logged:?}");
    }
    case.assert_quarantine(variant.warns());
}

/// ③ adopt の起動時プロファイル（判定）: 採用は止めないが、比べた相手が組み込み既定だと warnings へ。
/// 読むだけ（退避しない・persist.log に default.yaml を書かない）
fn adopt(variant: Variant) {
    let case = Case::new(variant);
    let mut host = Host::new();
    host.set_root_role("orchestrator-master");
    let root = host.root();
    let v = dispatch(
        &mut host,
        Request::OrchestratorAdopt {
            name: PROJECT.into(),
            pane: Some(root),
            caller_role: Some("master".into()),
            caller_pid: None,
        },
    )
    .unwrap_or_else(|e| panic!("{variant:?}: 採用は止めない: {e}"));
    assert_eq!(v["changed"], true, "{variant:?}: 採用していない: {v:#}");
    let w = default_warning(&case, &v);
    if variant.warns() {
        let w = w.unwrap_or_else(|| panic!("採用の判断が既定と比べたことを黙っている: {v:#}"));
        assert!(w.contains("組み込み既定と比べて採用を判断した"), "{w}");
    } else {
        assert_eq!(w, None, "{variant:?}: {v:#}");
    }
    case.assert_quarantine(false);
    assert!(
        case.log_lines("").is_empty(),
        "判定の経路が persist.log を埋めている"
    );
}

/// ④ guide（表示）: 既定値で答えるが warnings へ。local-rules は「無い」ではなく「読めない」
fn guide(variant: Variant) {
    let case = Case::new(variant);
    let mut host = Host::new();
    let ask = |host: &mut Host, topic: Option<&str>| {
        dispatch(
            host,
            Request::OrchestratorGuide {
                topic: topic.map(str::to_string),
                profile: Some("default".into()),
                caller_role: None,
                pane: None,
                caller_pid: None,
            },
        )
        .unwrap_or_else(|e| panic!("{variant:?}: guide は止まらない: {e}"))
    };
    let rules = ask(&mut host, Some("local-rules"));
    let list = ask(&mut host, None);
    let text = rules["text"].as_str().unwrap_or_default();
    let none = "no on-demand local rules";
    match variant {
        Variant::Broken => {
            assert!(
                text.contains("could NOT be read")
                    && text.contains(&case.default_yaml().display().to_string()),
                "local-rules が読めない理由を返していない: {text}"
            );
            assert!(!text.contains(none), "空のルールを正常として返した: {text}");
            for v in [&rules, &list] {
                let w = default_warning(&case, v)
                    .unwrap_or_else(|| panic!("guide が既定で組み立てたことを黙っている: {v:#}"));
                assert!(w.contains("local-rules"), "{w}");
            }
        }
        Variant::BrokenLegacy | Variant::Absent => {
            assert!(text.contains(none), "{variant:?}: {text}");
            assert!(rules.get("warnings").is_none() && list.get("warnings").is_none());
        }
        Variant::Good => {
            assert!(text.contains(RULE), "読める個人ルールが返らない: {text}");
            assert!(rules.get("warnings").is_none() && list.get("warnings").is_none());
        }
    }
    case.assert_quarantine(false);
    assert!(
        case.log_lines("").is_empty(),
        "表示の経路が persist.log を埋めている"
    );
}

/// ⑤ master / solo 本人の自動復帰の既定（判定・role を貼る瞬間に 1 回）: persist.log へ 1 行。
/// 読むだけ（退避しない）
fn limit_resume(variant: Variant) {
    let case = Case::new(variant);
    let solo_yaml = case.solo_profiles().join("default.yaml");
    for (role, path) in [
        ("orchestrator-master", case.default_yaml()),
        ("solo", solo_yaml.clone()),
    ] {
        let mut host = Host::new();
        let root = host.root();
        let before = case.persist_log();
        dispatch(
            &mut host,
            Request::Title {
                pane: Some(root),
                title: None,
                role: Some(role.into()),
            },
        )
        .expect("role は貼れる");
        let after = case.persist_log();
        let added = &after[before.len()..];
        let path = path.display().to_string();
        let lines: Vec<&str> = added
            .lines()
            .filter(|l| l.contains("[limit-autoresume]") && l.contains(&path))
            .collect();
        if variant.warns() {
            assert_eq!(
                lines.len(),
                1,
                "{role}: 既定で決めたことを黙っている:\n{added}"
            );
            assert!(lines[0].contains("全体の既定で決めた"), "{}", lines[0]);
            assert!(
                !host.root_limit_autoresume(),
                "読めないのにプロファイルの値が効いた"
            );
        } else {
            assert!(lines.is_empty(), "{variant:?} {role}: {added}");
        }
        if variant == Variant::Good {
            assert!(
                host.root_limit_autoresume(),
                "{role}: 読めるプロファイルの値が効いていない"
            );
        }
    }
    case.assert_quarantine(false);
    if variant.body().is_some() {
        assert_eq!(
            std::fs::read_to_string(&solo_yaml).expect("solo の元が残る"),
            variant.body().unwrap_or_default()
        );
    }
    assert!(
        !case
            .solo_profiles()
            .join("default.yaml.unreadable.bak")
            .exists(),
        "判定の経路が solo の退避を書いた"
    );
}

#[test]
fn issue2256_壊れたdefaultでspawn_解消_adopt_guide_自動復帰が黙らない() {
    for key in [
        "TAKO_SOCKET",
        "TAKO_PANE_ID",
        "TAKO_TOKEN",
        "TAKO_ORCHESTRATOR_DIR",
        "TAKO_ORCHESTRATOR_ROLE",
        "CLAUDE_CONFIG_DIR",
        "TAKO_2227_LEGACY",
        "TAKO_2233_LEGACY",
        "TAKO_1825_LEGACY",
        "TAKO_1140_LEGACY",
        "TAKO_1453_LEGACY",
    ] {
        std::env::remove_var(key);
    }
    std::env::set_var("TAKO_ISOLATED", "1");
    // claude の所在確認（ログインシェル）を外す
    std::env::set_var("TAKO_983_LEGACY", "1");
    // 実行時の自動移行（#916。1 プロセス 1 回）を default の無い data dir で先に済ませる。
    // 済ませないと最初のケースだけ移行が default.yaml を読みに行き、退避・persist.log の行が
    // 「この経路が出したもの」かどうかを見分けられない
    {
        let _warm = Case::new(Variant::Absent);
        let _ = tako_control::migrations::ensure_migrated();
    }

    // 経路ごとに結果を集める（1 経路の失敗で残りを見ずに終わらない = 修正前に戻すと
    // 5 経路それぞれが黙ることを 1 回の実行で名指しできる）
    let paths: [(&str, fn(Variant)); 5] = [
        ("spawn", spawn),
        ("git_resolve", git_resolve),
        ("adopt", adopt),
        ("guide", guide),
        ("limit_resume", limit_resume),
    ];
    let mut failed = Vec::new();
    for variant in ALL {
        for (name, path) in paths {
            if std::panic::catch_unwind(|| path(variant)).is_err() {
                failed.push(format!("{name}/{variant:?}"));
            }
        }
    }
    assert!(failed.is_empty(), "黙った / 崩れた経路: {failed:?}");
}
