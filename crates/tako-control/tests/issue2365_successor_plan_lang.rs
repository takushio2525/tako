//! 実経路: 引き継ぎの後任の起動と `master_launch::plan`（スマホの「+ master」・返答の配送）が
//! 表示言語に従う（Issue #2365 の続き）。
//!
//! 修正前（実測。CLI / MCP と同じ dispatch を GUI 無しのホストで・`plan` は daemon と同じ関数を直に）:
//! 英語表示でも、
//!
//! - **引き継ぎの後任**: 応答の `warnings`（読めないプロファイルで後任を組み込み既定で立てた理由・
//!   起動フォルダの注意）と後任の system prompt の `Detail:` が日本語のまま
//! - **`plan`**: 断り（プロファイルが無い・env の上書き・cwd が無い・projects の未登録・
//!   master_agent の誤り・#1825 の「見つからない」）と、読めない default.yaml の `warnings`・
//!   本人の system prompt の `Detail:` が日本語のまま（`load_profile` と String 版の口 = `into_ja`）
//! - **返答の配送**（起票 master が居ないので `plan` で起こす）: 配送の記録の理由
//!   「master を起動できない: …」が日本語のまま
//!
//! いまの形: 後任の理由は型（`ProfileFallback::Successor`）で持ち、`plan` は CLI の `tako master` と
//! 同じ型の口 + `text_in(lang)` で組む。どちらも persist.log は日本語のまま。このテストは
//!
//! - 英語表示: 上の出力に日本語（かな・漢字・全角）が 1 字も無い
//! - 日本語表示: #2365 より前と同じ形（目印の語を含む）
//! - どちらの表示言語でも persist.log の行は日本語
//!
//! を見る。`TAKO_2365_DUMP=<path>` を付けると、検査の前に全出力を JSON Lines で書き出す
//! （修正前 / 修正後を同じテストで回して並べ、日本語表示のバイト一致を突き合わせるため）。
//! 修正前の木でもコンパイルできる API だけを使う（A/B: 修正前は英語表示の検査で落ちる）。
//!
//! 表示言語・env はプロセス全体の状態なので、**専用の統合テストバイナリの 1 本のテスト関数**の中で
//! 順に切り替える（#2645 / #2233 のテストと同じ作法）。claude の所在確認は `TAKO_983_LEGACY=1` で
//! 外す（本物の claude を探さない・起こさない。後任のペインは端末を持たない）。
//!
//! unix だけ: 前任 master の作業フォルダを持たせるセッション（PTY）を張る。文の日英の対は OS に
//! 依らない 1 か所（`config_error.rs` ほか）で、両 OS の番犬（`issue1762_dispatch_error_lang.rs`）が表で見る
#![cfg(unix)]

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tako_control::protocol::Request;
use tako_control::{
    PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost, UiStateHost, WebViewHost,
    WorkspaceHost,
};
use tako_core::i18n::{self, Lang};
use tako_core::test_residue::ScratchDir;
use tako_core::{Pane, PaneId, PaneOrigin, SpawnCommand, SpawnOptions, TerminalSession, Workspace};

const GOOD: &str = "effort: max\nprompt_blocks:\n  append: \"RULE_2365\"\n";
const BROKEN: &str = "effort: max\nprompt_blocks:\n  append: \"RULE_2365\n  prepend: [\n";
const MEMO: &str = "## 知識\n- 引き継ぎの材料（#2365）\n\n## 実行状態\n- なし\n";
/// projects.yaml に登録だけあり、専用プロファイルが無いプロジェクト（#1825 で作れない）
const REGISTERED: &str = "k2365";

/// GUI の代わり: ペインの木と、前任 master の作業フォルダを持たせるセッション 1 つだけ
struct Host {
    workspace: Workspace,
    session: Option<(PaneId, TerminalSession)>,
}

impl Host {
    /// root を前任 master にする（role が master = 後任が前任を閉じる手順を受け取る）
    fn master() -> Self {
        let mut host = Self {
            workspace: Workspace::new("1", Pane::new(PaneOrigin::User)),
            session: None,
        };
        let id = host.workspace.active_tab().tree().focused();
        host.workspace
            .active_tab_mut()
            .tree_mut()
            .get_mut(id)
            .expect("root")
            .set_role(Some("orchestrator-master".to_string()));
        host
    }

    fn root(&self) -> u64 {
        self.workspace.active_tab().tree().focused().as_u64()
    }

    /// 前任 master の作業フォルダを `cwd` にする（シェルは起こさない = 利用者の rc を読まない）
    fn seed_cwd(&mut self, cwd: &Path) {
        let (session, _rx) = TerminalSession::spawn(
            80,
            24,
            SpawnOptions {
                command: Some(SpawnCommand {
                    program: "/bin/sleep".into(),
                    args: vec!["30".into()],
                }),
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

/// 1 ケースぶんの隔離した data dir（`TAKO_DATA_DIR` / `HOME` / ユーザータスクの置き場をここへ向ける）
struct Case {
    dir: ScratchDir,
}

impl Case {
    /// `default_yaml` が `None` = default.yaml が無い
    fn new(default_yaml: Option<&str>) -> Self {
        let dir = ScratchDir::new("issue2365sp");
        assert!(
            dir.path().starts_with(std::env::temp_dir()),
            "一時 dir の外に書こうとしている: {}",
            dir.path().display()
        );
        let case = Self { dir };
        for d in [
            case.profiles(),
            case.handoff_dir(),
            case.home(),
            case.work(),
            case.work2(),
        ] {
            std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
        }
        if let Some(body) = default_yaml {
            std::fs::write(case.profiles().join("default.yaml"), body).expect("default.yaml");
        }
        let projects = format!(
            "projects:\n  {REGISTERED}:\n    cwd: '{}'\n",
            case.work().display()
        );
        std::fs::write(case.orch().join("projects.yaml"), projects).expect("projects.yaml");
        std::env::set_var("TAKO_DATA_DIR", case.dir.path());
        std::env::set_var("HOME", case.home());
        std::env::set_var(
            "TAKO_USER_TASKS_FILE",
            case.dir.path().join("user-tasks.yaml"),
        );
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

    fn work(&self) -> PathBuf {
        self.dir.path().join("work")
    }

    fn work2(&self) -> PathBuf {
        self.dir.path().join("work2")
    }

    fn profile(&self, name: &str, body: &str) {
        std::fs::write(self.profiles().join(format!("{name}.yaml")), body)
            .unwrap_or_else(|e| panic!("{name}.yaml を置ける: {e}"));
    }

    fn memo(&self, profile: &str) {
        std::fs::write(self.handoff_dir().join(format!("{profile}.md")), MEMO).expect("運用メモ");
    }

    /// 本人の system prompt の `Detail:` の行（警告が無ければ `None`）
    fn detail(&self, profile: &str) -> Option<String> {
        let path = self.orch().join(format!("_system_prompt_{profile}.md"));
        let prompt = std::fs::read_to_string(&path).ok()?;
        prompt
            .split("\nDetail: ")
            .nth(1)
            .map(|d| d.trim_end().to_string())
    }

    /// persist.log の `tag` の行
    fn log(&self, tag: &str) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join("persist.log"))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains(tag))
            // 行頭の時刻は落とす（修正前後で突き合わせるため）
            .map(|l| l.split_once("] ").map_or(l, |(_, rest)| rest).to_string())
            .collect()
    }

    /// 一時 dir の名前を伏せる（修正前後で突き合わせるため。macOS は /private 越しの実体もある）
    fn norm(&self, s: &str) -> String {
        let raw = self.dir.path().display().to_string();
        let real = std::fs::canonicalize(self.dir.path())
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| raw.clone());
        s.replace(&real, "<DIR>").replace(&raw, "<DIR>")
    }
}

/// 1 件の出力（`log` = persist.log の行。日英どちらの表示でも日本語のまま）
struct Out {
    label: String,
    lang: Lang,
    text: String,
    log: bool,
}

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

fn dispatch(host: &mut Host, req: Request) -> Result<Value, String> {
    tako_control::dispatch::dispatch(host, req, PaneOrigin::User).map_err(|e| e.to_string())
}

fn handoff(host: &mut Host, role: &str) -> Result<Value, String> {
    let pane = host.root();
    dispatch(
        host,
        Request::OrchestratorHandoff {
            pane: Some(pane),
            caller_role: Some(role.to_string()),
            tab: None,
            caller_pid: None,
            projects: None,
        },
    )
}

/// 応答 JSON のうち利用者へ見せる文（`warnings` と前任の扱いの注記）
fn handoff_texts(v: &Value) -> String {
    let warnings: Vec<&str> = v["warnings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    format!(
        "warnings={warnings:?} close={}",
        v["previous_master_close"].as_str().unwrap_or_default()
    )
}

struct Collector<'a> {
    lang: Lang,
    out: &'a mut Vec<Out>,
}

impl Collector<'_> {
    fn push(&mut self, case: &Case, label: &str, text: &str) {
        self.out.push(Out {
            label: label.to_string(),
            lang: self.lang,
            text: case.norm(text),
            log: false,
        });
    }

    fn push_log(&mut self, case: &Case, label: &str, tag: &str) {
        for (i, line) in case.log(tag).iter().enumerate() {
            self.out.push(Out {
                label: format!("{label}/log{i}"),
                lang: self.lang,
                text: case.norm(line),
                log: true,
            });
        }
    }

    fn push_detail(&mut self, case: &Case, label: &str, profile: &str) {
        match case.detail(profile) {
            Some(d) => self.push(case, &format!("{label}/detail"), &d),
            None => self.push(case, &format!("{label}/detail"), "<none>"),
        }
    }

    /// 引き継ぎ 1 回（応答の文・後任の Detail・persist.log）
    fn handoff(&mut self, case: &Case, host: &mut Host, label: &str, role: &str, profile: &str) {
        match handoff(host, role) {
            Ok(v) => {
                self.push(case, label, &handoff_texts(&v));
                self.push_detail(case, label, profile);
            }
            Err(e) => self.push(case, &format!("{label}/err"), &e),
        }
        self.push_log(case, label, &format!("[master] {profile}:"));
    }

    /// `plan` 1 回（daemon の `POST /api/tabs/:id/master` と返答の配送が呼ぶ関数）
    fn plan(&mut self, case: &Case, label: &str, profile: &str) {
        match tako_control::orchestrator::master_launch::plan(profile) {
            Ok(plan) => {
                let mut v = plan.to_json();
                // 起動で付ける名前・cwd は言語に依らない（伏せずに残す）
                v["profile"] = Value::String(plan.profile_name.clone());
                self.push(case, label, &v.to_string());
                self.push_detail(case, label, profile);
            }
            Err(e) => self.push(case, &format!("{label}/err"), &e),
        }
        self.push_log(case, label, &format!("[master] {profile}:"));
    }
}

/// 起票 master（`master:<profile>`）のタスクを 1 件立て、その master が居ない状態で返答する。
/// 返すのは配送の記録の理由（`plan` が落ちたときの「起動できない」）
fn respond_without_master(host: &mut Host, profile: &str) -> String {
    let root = host.root();
    let task = |action: &str| Request::UserTask {
        action: action.into(),
        id: None,
        title: None,
        body: None,
        kind: None,
        status: None,
        all: None,
        project: None,
        attachments: None,
        copy_texts: None,
        links: None,
        due: None,
        decision: None,
        comment: None,
        via: None,
        pane: Some(root),
        caller_role: Some(format!("master:{profile}")),
    };
    let mut add = task("add");
    if let Request::UserTask { title, .. } = &mut add {
        *title = Some("#2365 の返答".into());
    }
    let id = dispatch(host, add).expect("起票できる")["id"]
        .as_str()
        .expect("id")
        .to_string();
    // 起票した master を退かせる（返答の配送は `plan` で起こしに行く）
    let pane = PaneId::from_raw(root);
    host.workspace
        .active_tab_mut()
        .tree_mut()
        .get_mut(pane)
        .expect("root")
        .set_role(None);
    let mut respond = task("respond");
    if let Request::UserTask {
        id: tid,
        decision,
        comment,
        via,
        caller_role,
        pane,
        ..
    } = &mut respond
    {
        *tid = Some(id);
        *decision = Some("approve".into());
        *comment = Some("ok".into());
        *via = Some("pwa".into());
        *caller_role = None;
        *pane = None;
    }
    match dispatch(host, respond) {
        Ok(v) => v["delivery"]["reason"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("<state {}>", v["delivery"]["state"])),
        Err(e) => format!("<err {e}>"),
    }
}

/// 1 つの表示言語で、後任の起動・`plan`・返答の配送の出力を全部集める
fn run(lang: Lang, out: &mut Vec<Out>) {
    i18n::set_lang(lang);
    let mut c = Collector { lang, out };

    // ① 後任: 読めない default.yaml（組み込み既定で立てる理由）+ 前任の cwd が採れない（ホーム）
    {
        let case = Case::new(Some(BROKEN));
        case.memo("default");
        let mut host = Host::master();
        c.handoff(
            &case,
            &mut host,
            "handoff/default_broken",
            "master",
            "default",
        );
    }
    // ② 後任: 専用プロファイルが無い（「見つからない」を包んで組み込み既定で立てる）
    {
        let case = Case::new(Some(GOOD));
        case.memo("p2365s");
        let mut host = Host::master();
        c.handoff(
            &case,
            &mut host,
            "handoff/named_missing",
            "master:p2365s",
            "p2365s",
        );
    }
    // ③ 後任: 専用プロファイルが壊れている（パース失敗を包む）
    {
        let case = Case::new(Some(GOOD));
        case.memo("p2365b");
        case.profile("p2365b", BROKEN);
        let mut host = Host::master();
        c.handoff(
            &case,
            &mut host,
            "handoff/named_broken",
            "master:p2365b",
            "p2365b",
        );
    }
    // ④ 後任: 登録だけのプロジェクトで継承元の default.yaml が読めない（#1825 の理由を包む）
    {
        let case = Case::new(Some(BROKEN));
        case.memo(REGISTERED);
        let mut host = Host::master();
        c.handoff(
            &case,
            &mut host,
            "handoff/registered_missing",
            &format!("master:{REGISTERED}"),
            REGISTERED,
        );
    }
    // ⑤ 後任: 起動フォルダが前任と違う（プロファイルの cwd が決め手）
    {
        // プロファイルの cwd は一時 dir の中（dir を作った後に置く）
        let case = Case::new(None);
        case.profile("default", &format!("cwd: '{}'\n", case.work2().display()));
        case.memo("default");
        let mut host = Host::master();
        host.seed_cwd(&case.work());
        c.handoff(&case, &mut host, "handoff/cwd_differs", "master", "default");
    }
    // ⑥ 後任（境界・断り）: プロファイルの cwd が無い = 分割の前に断る
    {
        let case = Case::new(Some("cwd: '/nonexistent-2365'\n"));
        case.memo("default");
        let mut host = Host::master();
        c.handoff(&case, &mut host, "handoff/cwd_broken", "master", "default");
    }

    // ⑦ plan: 断りの全種と、読めない default.yaml で立てる警告
    {
        let case = Case::new(Some(GOOD));
        c.plan(&case, "plan/good", "default");
        c.plan(&case, "plan/not_found", "n2365");
        case.profile("e2365", "env:\n  TAKO_SOCKET: x\n");
        c.plan(&case, "plan/env_reserved", "e2365");
        case.profile(
            "c2365",
            &format!("cwd: '{}'\n", case.dir.path().join("nope").display()),
        );
        c.plan(&case, "plan/cwd_missing", "c2365");
        case.profile("j2365", "projects:\n- zz2365\n");
        c.plan(&case, "plan/projects_unknown", "j2365");
        case.profile("a2365", "master_agent: bogus2365\n");
        c.plan(&case, "plan/master_agent", "a2365");
    }
    {
        let case = Case::new(Some(BROKEN));
        c.plan(&case, "plan/default_broken", "default");
    }
    {
        let case = Case::new(Some(BROKEN));
        c.plan(&case, "plan/registered_missing", REGISTERED);
    }
    // ⑧ 境界: default.yaml が無い（初回）= 警告は出ない
    {
        let case = Case::new(None);
        c.plan(&case, "plan/default_absent", "default");
    }

    // ⑨ 返答の配送: 起票 master が居ないので `plan` で起こす → 組み立てが落ちた理由
    {
        let case = Case::new(Some(GOOD));
        let mut host = Host::master();
        let reason = respond_without_master(&mut host, "r2365");
        c.push(&case, "deliver/plan_failed", &reason);
    }
}

/// 目印: 日本語表示の出力に #2365 より前と同じ語があるか（ラベルの前方一致 → 目印）
const JA_MARKS: &[(&str, &str)] = &[
    (
        "handoff/default_broken/detail",
        "組み込み既定で master を立てる",
    ),
    (
        "handoff/default_broken",
        "前任 master の cwd を採取できなかった",
    ),
    (
        "handoff/named_missing/detail",
        "後任 master のプロファイル 'p2365s' を読めない",
    ),
    ("handoff/named_missing", "引き継ぎは止めずに"),
    (
        "handoff/named_broken",
        "後任 master のプロファイル 'p2365b' を読めない",
    ),
    (
        "handoff/registered_missing",
        "後任 master のプロファイル 'k2365' を読めない",
    ),
    (
        "handoff/cwd_differs",
        "後任 master の起動フォルダが前任と違います",
    ),
    ("handoff/cwd_broken/err", "cwd"),
    ("plan/not_found/err", "n2365"),
    ("plan/env_reserved/err", "TAKO_SOCKET"),
    ("plan/cwd_missing/err", "nope"),
    ("plan/projects_unknown/err", "zz2365"),
    ("plan/master_agent/err", "bogus2365"),
    (
        "plan/default_broken/detail",
        "組み込み既定で master を立てる",
    ),
    ("plan/default_broken", "組み込み既定で master を立てる"),
    ("plan/registered_missing/err", "k2365"),
    ("deliver/plan_failed", "master を起動できない"),
];

fn dump(out: &[Out]) {
    let Some(path) = std::env::var_os("TAKO_2365_DUMP") else {
        return;
    };
    let mut f = std::fs::File::create(&path).expect("書き出し先を作れる");
    for o in out {
        let line = serde_json::json!({
            "label": o.label,
            "lang": match o.lang { Lang::Ja => "ja", Lang::En => "en" },
            "log": o.log,
            "text": o.text,
        });
        writeln!(f, "{line}").expect("書ける");
    }
}

#[test]
fn issue2365_後任の起動とplanは表示言語に従いpersist_logは日本語のまま() {
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
        "TAKO_1055_LEGACY",
        "TAKO_2182_LEGACY",
    ] {
        std::env::remove_var(key);
    }
    std::env::set_var("TAKO_ISOLATED", "1");
    // claude の所在確認（ログインシェル）を外す = 本物の claude を探さない・起こさない
    std::env::set_var("TAKO_983_LEGACY", "1");
    // 実行時の自動移行（#916。1 プロセス 1 回）を先に済ませる（最初のケースだけ移行が
    // default.yaml を読みに行き、persist.log の行がこの経路のものか見分けられなくなる）
    {
        let dir = ScratchDir::new("issue2365sp-warm");
        std::env::set_var("TAKO_DATA_DIR", dir.path());
        std::env::set_var("HOME", dir.path());
        let _ = tako_control::migrations::ensure_migrated();
    }

    let mut out = Vec::new();
    for lang in [Lang::En, Lang::Ja] {
        run(lang, &mut out);
    }
    dump(&out);

    let mut failures = Vec::new();
    for o in &out {
        match (o.log, o.lang) {
            // persist.log は日英どちらの表示でも日本語（診断ログ = FR-2.42.21）
            (true, _) => {
                if has_japanese(&o.text).is_none() {
                    failures.push(format!(
                        "{:?} {}: persist.log の行が日本語でない: {}",
                        o.lang, o.label, o.text
                    ));
                }
            }
            (false, Lang::En) => {
                if let Some(ch) = has_japanese(&o.text) {
                    failures.push(format!(
                        "En {}: 英語表示に日本語（{ch}）: {}",
                        o.label, o.text
                    ));
                }
            }
            (false, Lang::Ja) => {
                if let Some((_, mark)) = JA_MARKS.iter().find(|(l, _)| o.label == *l) {
                    if !o.text.contains(mark) {
                        failures.push(format!(
                            "Ja {}: 日本語表示が #2365 より前の形でない（「{mark}」が無い）: {}",
                            o.label, o.text
                        ));
                    }
                }
            }
        }
    }
    // 走査が空振りしていない（目印のラベルが日英の両方で出ている）
    for (label, _) in JA_MARKS {
        for lang in [Lang::En, Lang::Ja] {
            if !out.iter().any(|o| o.lang == lang && o.label == *label) {
                failures.push(format!(
                    "{lang:?} {label}: 出力が無い（経路が通っていない）"
                ));
            }
        }
    }
    let en_total = out.iter().filter(|o| !o.log && o.lang == Lang::En).count();
    let en_ja = out
        .iter()
        .filter(|o| !o.log && o.lang == Lang::En && has_japanese(&o.text).is_some())
        .count();
    eprintln!("[2365] 英語表示の日本語入り: {en_ja} / {en_total}");
    assert!(
        failures.is_empty(),
        "後任の起動・plan が表示言語に従っていない（{} 件）:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
