//! 実経路: orchestrator の設定ファイル（projects.yaml / accounts.yaml / profiles）の読み書きと
//! エージェント名の検証の断りが表示言語に従う（Issue #2365 の続き）。
//!
//! 修正前（実測。CLI / MCP と同じ dispatch の関数を直に呼んで）: 枠（`invalid parameter:` ほか）は
//! 英語なのに中身が「kind が不正: 'bogus'（master / solo）」「未対応のエージェント種別 'gpt'（…）」
//! 「プロジェクト 'ghost' が projects.yaml に見つからない」のように日本語のまま = 1 つの断りに日英が混ざった。
//!
//! `orchestrator projects` / `profiles` / `accounts` は CLI も MCP も同じ dispatch の関数を通る
//! （CLI は GUI を経由せず直に呼ぶ）。spawn / adopt は GUI の代わりのホストへ要求を通す。
//! ケースごとに設定の置き場（`TAKO_ORCHESTRATOR_DIR`）を一時の場所へ向け、
//!
//! - 英語表示: 断りに日本語（かな・漢字）が 1 字も無い
//! - 日本語表示: #2365 より前の文とバイト一致
//!
//! を見る。どのケースも断られて本番の設定は読み書きしない（`HOME` / `TAKO_DATA_DIR` も一時の場所）。
//! 表示言語はプロセス全体の状態なので、**専用の統合テストバイナリの 1 本のテスト関数**の中で順に
//! 切り替える。このファイルは修正前の木でもコンパイルできる API だけを使う（A/B: 修正前は英語表示の
//! 検査で落ちる）
//!
//! unix だけで走らせる: 読めないファイル（パーミッション 000）とロックファイルの置き場をディレクトリに
//! する形が unix の前提。文の日英の対は OS に依らない 1 か所（`ConfigError::text_in`）で、両 OS の番犬
//! （`issue1762_dispatch_error_lang.rs`）が表で見る
#![cfg(unix)]

use std::path::{Path, PathBuf};

use serde_json::Value;
use tako_control::dispatch::{
    dispatch_orchestrator_profiles, dispatch_orchestrator_projects,
    dispatch_orchestrator_projects_mcp, ProfilesParams,
};
use tako_control::protocol::Request;
use tako_control::{
    dispatch_orchestrator_accounts, DispatchError, PreviewHost, RemoteHost, SessionHost,
    SystemHost, TmuxHost, UiStateHost, WebViewHost, WorkspaceHost,
};
use tako_core::i18n::{self, Lang};
use tako_core::{Pane, PaneId, PaneOrigin, SpawnOptions, TerminalSession, Workspace};

/// GUI の代わり: ペインの木だけを持つ（spawn / adopt は設定の検証で断られ、ペインを作らない）
struct Host {
    workspace: Workspace,
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

impl UiStateHost for Host {}
impl PreviewHost for Host {}
impl TmuxHost for Host {}
impl WebViewHost for Host {}
impl RemoteHost for Host {}
impl SystemHost for Host {}

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

/// 設定の置き場の種類（ケースごとに `TAKO_ORCHESTRATOR_DIR` を向ける）
#[derive(Clone, Copy)]
enum Dir {
    /// 正しい projects.yaml / accounts.yaml / profiles（ただし default.yaml の worker_agent は不正）
    Ok,
    /// projects.yaml / accounts.yaml が YAML として壊れ、profiles/broken.yaml の形が違う
    Broken,
    /// projects.yaml を読めない（パーミッション 000）
    Unreadable,
    /// ロックファイルの置き場がディレクトリ（排他ロックを取れない）
    Locked,
}

/// 置き場を作る（`root/<name>`）。返すのは置き場のパス
fn make_dirs(root: &Path, here: &Path) -> [(PathBuf, Dir); 4] {
    use std::os::unix::fs::PermissionsExt;
    let ok = root.join("ok");
    std::fs::create_dir_all(ok.join("profiles")).unwrap();
    std::fs::write(
        ok.join("projects.yaml"),
        format!(
            "version: 2\nprojects:\n  gone:\n    cwd: /nonexistent2365/gone\n  here:\n    cwd: {}\n",
            here.display()
        ),
    )
    .unwrap();
    std::fs::write(
        ok.join("accounts.yaml"),
        "accounts:\n  both:\n    inherit: true\n    config_dir: /tmp/x2365\n  none: {}\n",
    )
    .unwrap();
    std::fs::write(ok.join("profiles/default.yaml"), "worker_agent: gpt\n").unwrap();

    let broken = root.join("broken");
    std::fs::create_dir_all(broken.join("profiles")).unwrap();
    std::fs::write(broken.join("projects.yaml"), BROKEN_PROJECTS).unwrap();
    std::fs::write(broken.join("accounts.yaml"), BROKEN_ACCOUNTS).unwrap();
    std::fs::write(broken.join("profiles/broken.yaml"), BROKEN_PROFILE).unwrap();

    let unreadable = root.join("unreadable");
    std::fs::create_dir_all(&unreadable).unwrap();
    let projects = unreadable.join("projects.yaml");
    std::fs::write(&projects, "projects: {}\n").unwrap();
    std::fs::set_permissions(&projects, std::fs::Permissions::from_mode(0o000)).unwrap();

    let locked = root.join("locked");
    std::fs::create_dir_all(locked.join("projects.yaml.lock")).unwrap();
    std::fs::create_dir_all(locked.join("accounts.yaml.lock")).unwrap();
    std::fs::create_dir_all(locked.join("profiles/x.yaml.lock")).unwrap();

    [
        (ok, Dir::Ok),
        (broken, Dir::Broken),
        (unreadable, Dir::Unreadable),
        (locked, Dir::Locked),
    ]
}

const BROKEN_PROJECTS: &str = "projects: [\n";
const BROKEN_ACCOUNTS: &str = "accounts:\n  a: [\n";
const BROKEN_PROFILE: &str = "effort: [1, 2]\n";

/// 要求を 1 つ通す呼び方（ホストとそのペイン ID を受ける）
type Invoke = Box<dyn Fn(&mut Host, u64) -> Result<Value, DispatchError>>;

/// 断りを引き出す呼び方
enum Call {
    /// 断り（`Err`）の文
    Err(Invoke),
    /// 成功の応答の中の文（一覧の壊れたエントリの `error`）。`(呼び方, JSON の配列の鍵, name)`
    Entry(Invoke, &'static str, &'static str),
}

struct Case {
    name: &'static str,
    dir: Dir,
    call: Call,
    /// #2365 より前の日本語（`<dir>` は置き場のパス・`<yaml:…>` は YAML の理由）
    old_ja: &'static str,
}

fn err(
    name: &'static str,
    dir: Dir,
    old_ja: &'static str,
    f: impl Fn(&mut Host, u64) -> Result<Value, DispatchError> + 'static,
) -> Case {
    Case {
        name,
        dir,
        call: Call::Err(Box::new(f)),
        old_ja,
    }
}

fn entry(
    name: &'static str,
    dir: Dir,
    old_ja: &'static str,
    list: &'static str,
    who: &'static str,
    f: impl Fn(&mut Host, u64) -> Result<Value, DispatchError> + 'static,
) -> Case {
    Case {
        name,
        dir,
        call: Call::Entry(Box::new(f), list, who),
        old_ja,
    }
}

fn profiles(action: &str, f: impl FnOnce(&mut ProfilesParams)) -> Result<Value, DispatchError> {
    let mut p = ProfilesParams {
        action: action.to_string(),
        ..Default::default()
    };
    f(&mut p);
    dispatch_orchestrator_profiles(p)
}

fn named(name: &str) -> impl FnOnce(&mut ProfilesParams) + '_ {
    move |p| p.name = Some(name.to_string())
}

fn accounts(action: &str, name: Option<&str>) -> Result<Value, DispatchError> {
    dispatch_orchestrator_accounts(action, name, None, None, None, None, None)
}

fn spawn(
    host: &mut Host,
    pane: u64,
    project: &str,
    agent: Option<&str>,
    account: Option<&str>,
) -> Result<Value, DispatchError> {
    let request = Request::OrchestratorSpawn {
        project: project.to_string(),
        prompt: "x2365".to_string(),
        label: None,
        model: None,
        effort: None,
        pane: Some(pane),
        tab: None,
        caller_role: None,
        agent: agent.map(str::to_string),
        caller_pid: None,
        task_type: None,
        account: account.map(str::to_string),
        limit_resume: None,
    };
    tako_control::dispatch(host, request, PaneOrigin::Cli)
}

fn adopt(host: &mut Host, pane: u64, name: &str) -> Result<Value, DispatchError> {
    let request = Request::OrchestratorAdopt {
        name: name.to_string(),
        pane: Some(pane),
        caller_role: Some("master".to_string()),
        caller_pid: None,
    };
    tako_control::dispatch(host, request, PaneOrigin::Cli)
}

const AGENTS_JA: &str = "（対応: claude / codex / agy）";

fn cases() -> Vec<Case> {
    vec![
        // ---- projects（CLI `tako orchestrator projects` / MCP `tako_orchestrator_projects`） ----
        err(
            "projects list（壊れた projects.yaml）",
            Dir::Broken,
            "projects.yaml のパースに失敗: <yaml:projects>",
            |_, _| dispatch_orchestrator_projects("list", None, None, None),
        ),
        err(
            "projects add（壊れた projects.yaml = 既定の用意で読む）",
            Dir::Broken,
            "projects.yaml のパースに失敗: <yaml:projects>",
            |_, _| {
                dispatch_orchestrator_projects(
                    "add",
                    Some("k2365".into()),
                    Some("/tmp".into()),
                    None,
                )
            },
        ),
        err(
            "projects mcp（読めない projects.yaml）",
            Dir::Unreadable,
            "projects.yaml の読み取りに失敗: Permission denied (os error 13)",
            |_, _| dispatch_orchestrator_projects_mcp("mcp", Some("k2365".into()), Vec::new()),
        ),
        err(
            "projects remove（ロックを取れない）",
            Dir::Locked,
            "ロックファイルを開けない (<dir>/projects.yaml.lock): Is a directory (os error 21)",
            |_, _| dispatch_orchestrator_projects("remove", Some("k2365".into()), None, None),
        ),
        // ---- profiles（CLI `tako orchestrator profiles` / MCP `tako_orchestrator_profiles`） ----
        err(
            "profiles list（kind が不正）",
            Dir::Ok,
            "無効なパラメータ: kind が不正: 'bogus'（master / solo）",
            |_, _| profiles("list", |p| p.kind = Some("bogus".into())),
        ),
        err(
            "profiles show（名前が . で始まる）",
            Dir::Ok,
            "プロファイル名は '.' で始められない: '.x'",
            |_, _| profiles("show", named(".x")),
        ),
        err(
            "profiles show（名前が - で始まる）",
            Dir::Ok,
            "プロファイル名は '-' で始められない（起動コマンドのオプションと紛れる）: '-x'",
            |_, _| profiles("show", named("-x")),
        ),
        err(
            "profiles show（使えない文字）",
            Dir::Ok,
            "プロファイル名に使えない文字が含まれる: ' '（英数字と - _ . のみ）",
            |_, _| profiles("show", named("a b")),
        ),
        err(
            "profiles show（名前が空）",
            Dir::Ok,
            "プロファイル名が空",
            |_, _| profiles("show", named("")),
        ),
        err(
            "profiles show（無い）",
            Dir::Ok,
            "master プロファイル 'nope' が見つからない: <dir>/profiles/nope.yaml",
            |_, _| profiles("show", named("nope")),
        ),
        err(
            "profiles show（形が違う）",
            Dir::Broken,
            "プロファイルのパースに失敗: <yaml:profile>",
            |_, _| profiles("show", named("broken")),
        ),
        entry(
            "profiles list の壊れたエントリ",
            Dir::Broken,
            "プロファイルのパースに失敗: <yaml:profile>",
            "profiles",
            "broken",
            |_, _| profiles("list", |_| {}),
        ),
        err(
            "profiles create（既にある）",
            Dir::Ok,
            "master プロファイル 'default' は既に存在する（上書きするなら set を使う）",
            |_, _| profiles("create", named("default")),
        ),
        err(
            "profiles delete（default）",
            Dir::Ok,
            "既定プロファイル 'default' は削除できない（tako master が参照する）",
            |_, _| profiles("delete", named("default")),
        ),
        err(
            "profiles delete（solo の無いもの）",
            Dir::Ok,
            "solo プロファイル 'ghost' が見つからない: <dir>/solo-profiles/ghost.yaml",
            |_, _| {
                profiles("delete", |p| {
                    p.name = Some("ghost".into());
                    p.kind = Some("solo".into());
                })
            },
        ),
        err(
            "profiles copy（複製元が無い）",
            Dir::Ok,
            "master プロファイル 'nope' が見つからない: <dir>/profiles/nope.yaml",
            |_, _| {
                profiles("copy", |p| {
                    p.name = Some("new2365".into());
                    p.from = Some("nope".into());
                })
            },
        ),
        err(
            "profiles set（worker_agent が不正）",
            Dir::Ok,
            "無効なパラメータ: 未対応のエージェント種別 'gpt'<agents>",
            |_, _| {
                profiles("set", |p| {
                    p.name = Some("x".into());
                    p.worker_agent = Some("gpt".into());
                })
            },
        ),
        err(
            "profiles set（agent が不正）",
            Dir::Ok,
            "無効なパラメータ: 未対応のエージェント種別 'gpt'<agents>",
            |_, _| {
                profiles("set", |p| {
                    p.name = Some("x".into());
                    p.agent = Some("gpt".into());
                    p.agent_model = Some("m".into());
                })
            },
        ),
        err(
            "profiles set（master にできない agent）",
            Dir::Ok,
            "無効なパラメータ: agy は master 非対応（master は claude / codex のみ。worker としては利用可能）",
            |_, _| {
                profiles("set", |p| {
                    p.name = Some("x".into());
                    p.master_agent = Some("agy".into());
                })
            },
        ),
        err(
            "profiles set（env に内部変数）",
            Dir::Ok,
            "プロファイルの env に tako 内部変数が含まれている（上書き禁止）: TAKO_SOCKET",
            |_, _| {
                profiles("set", |p| {
                    p.name = Some("x".into());
                    p.env_set = Some(vec!["TAKO_SOCKET=1".into()]);
                })
            },
        ),
        err(
            "profiles set（cwd が無い）",
            Dir::Ok,
            "無効なパラメータ: プロファイルの cwd が存在しない: /nonexistent2365/p（元の指定: /nonexistent2365/p）",
            |_, _| {
                profiles("set", |p| {
                    p.name = Some("x".into());
                    p.cwd = Some("/nonexistent2365/p".into());
                })
            },
        ),
        err(
            "profiles set（ctx_threshold が範囲の外）",
            Dir::Ok,
            "無効なパラメータ: ctx_threshold は 50〜60 の範囲で指定する: 10",
            |_, _| {
                profiles("set", |p| {
                    p.name = Some("x".into());
                    p.ctx_threshold = Some(10);
                })
            },
        ),
        err(
            "profiles set（形が違うファイル）",
            Dir::Broken,
            "プロファイルのパースに失敗: <yaml:profile>",
            |_, _| {
                profiles("set", |p| {
                    p.name = Some("broken".into());
                    p.model = Some("m".into());
                })
            },
        ),
        err(
            "profiles set（ロックを取れない）",
            Dir::Locked,
            "ロックファイルを開けない (<dir>/profiles/x.yaml.lock): Is a directory (os error 21)",
            |_, _| {
                profiles("set", |p| {
                    p.name = Some("x".into());
                    p.model = Some("m".into());
                })
            },
        ),
        // ---- accounts（CLI `tako orchestrator accounts` / MCP `tako_orchestrator_accounts`） ----
        err(
            "accounts show（無い）",
            Dir::Ok,
            "アカウント 'ghost' が accounts.yaml に見つからない",
            |_, _| accounts("show", Some("ghost")),
        ),
        err(
            "accounts show（inherit と config_dir の両方）",
            Dir::Ok,
            "アカウント 'both' は inherit: true と config_dir を同時に指定している（既定の資格情報を使うなら config_dir を消す）",
            |_, _| accounts("show", Some("both")),
        ),
        entry(
            "accounts list の壊れたエントリ（config_dir が無い）",
            Dir::Ok,
            "アカウント 'none' に config_dir が無い（別 config dir なら config_dir を、既定の資格情報なら inherit: true を指定する）",
            "accounts",
            "none",
            |_, _| accounts("list", None),
        ),
        err(
            "accounts list（壊れた accounts.yaml）",
            Dir::Broken,
            "accounts.yaml のパースに失敗: <yaml:accounts>",
            |_, _| accounts("list", None),
        ),
        err(
            "accounts remove（ロックを取れない）",
            Dir::Locked,
            "ロックファイルを開けない (<dir>/accounts.yaml.lock): Is a directory (os error 21)",
            |_, _| accounts("remove", Some("ghost")),
        ),
        // ---- spawn（GUI の代わりのホスト。ペインを作る前に断る） ----
        err(
            "spawn（プロジェクトが無い）",
            Dir::Ok,
            "プロジェクト 'ghost' が projects.yaml に見つからない",
            |h, pane| spawn(h, pane, "ghost", None, None),
        ),
        err(
            "spawn（プロジェクトの cwd が無い）",
            Dir::Ok,
            "cwd が存在しない: /nonexistent2365/gone",
            |h, pane| spawn(h, pane, "gone", None, None),
        ),
        err(
            "spawn（アカウントが無い）",
            Dir::Ok,
            "アカウント 'ghost' が accounts.yaml に見つからない",
            |h, pane| spawn(h, pane, "here", None, Some("ghost")),
        ),
        err(
            "spawn（agent が不正）",
            Dir::Ok,
            "無効なパラメータ: 未対応のエージェント種別 'gpt'<agents>",
            |h, pane| spawn(h, pane, "here", Some("gpt"), None),
        ),
        err(
            "spawn（プロファイルの worker_agent が不正）",
            Dir::Ok,
            "無効なパラメータ: プロファイルの worker_agent が不正: 未対応のエージェント種別 'gpt'<agents>",
            |h, pane| spawn(h, pane, "here", None, None),
        ),
        // ---- adopt（GUI の代わりのホスト。役割を書き換える前に断る） ----
        err(
            "adopt（名前が - で始まる）",
            Dir::Ok,
            "無効なパラメータ: プロファイル名は '-' で始められない（起動コマンドのオプションと紛れる）: '-x'",
            |h, pane| adopt(h, pane, "-x"),
        ),
        err(
            "adopt（形が違うプロファイル）",
            Dir::Broken,
            "プロファイルのパースに失敗: <yaml:profile>",
            |h, pane| adopt(h, pane, "broken"),
        ),
    ]
}

/// YAML の理由（#2365 より前の `yaml_error_reason` と同じ組み立て = 種類と位置の日本語）
fn yaml_ja<T: serde::de::DeserializeOwned>(text: &str) -> String {
    let Err(e) = serde_yaml::from_str::<T>(text) else {
        panic!("壊れた YAML のはずが読めた: {text}");
    };
    tako_core::load_issue::YamlReason::of(&e, text).text_in(Lang::Ja)
}

fn expand_old(old: &str, dir: &Path) -> String {
    use tako_control::orchestrator::{AccountsConfig, Profile, ProjectsConfig};
    old.replace("<dir>", &dir.display().to_string())
        .replace("<agents>", AGENTS_JA)
        .replace(
            "<yaml:projects>",
            &yaml_ja::<ProjectsConfig>(BROKEN_PROJECTS),
        )
        .replace(
            "<yaml:accounts>",
            &yaml_ja::<AccountsConfig>(BROKEN_ACCOUNTS),
        )
        .replace("<yaml:profile>", &yaml_ja::<Profile>(BROKEN_PROFILE))
}

struct Seen {
    name: &'static str,
    dir: PathBuf,
    text: String,
}

fn observe(dirs: &[(PathBuf, Dir)]) -> Vec<Seen> {
    let mut seen = Vec::new();
    for c in cases() {
        let dir = dirs
            .iter()
            .find(|(_, d)| std::mem::discriminant(d) == std::mem::discriminant(&c.dir))
            .map(|(p, _)| p.clone())
            .expect("置き場");
        std::env::set_var("TAKO_ORCHESTRATOR_DIR", &dir);
        let mut host = Host {
            workspace: Workspace::new("1", Pane::new(PaneOrigin::User)),
        };
        let pane = host
            .workspace
            .all_pane_ids()
            .into_iter()
            .next()
            .expect("ペイン")
            .as_u64();
        let text = match &c.call {
            Call::Err(f) => match f(&mut host, pane) {
                Err(e) => e.to_string(),
                Ok(v) => format!("<断られずに通った: {v}>"),
            },
            Call::Entry(f, list, who) => match f(&mut host, pane) {
                Ok(v) => v[*list]
                    .as_array()
                    .and_then(|a| a.iter().find(|e| e["name"] == *who))
                    .and_then(|e| e["error"].as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("<{who} の error が無い: {v}>")),
                Err(e) => format!("<断られた: {e}>"),
            },
        };
        seen.push(Seen {
            name: c.name,
            dir,
            text,
        });
    }
    seen
}

/// このバイナリのテストは 1 本だけ（`TAKO_ORCHESTRATOR_DIR` と表示言語をプロセスで 1 つ書き換えるので、
/// 並走する別のテストを作らない = #2400）
#[test]
fn orchestrator_の設定ファイルの断りは表示言語に従う() {
    let root = std::env::temp_dir().join(format!("tako-2365o-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("一時の置き場");
    // 本番の設定を読み書きしない（どの置き場も一時の場所の下）
    std::env::set_var("HOME", root.join("home"));
    std::env::set_var("TAKO_DATA_DIR", root.join("data"));
    let here = root.join("here");
    std::fs::create_dir_all(&here).unwrap();
    let dirs = make_dirs(&root, &here);

    let original = i18n::lang();
    let mut by_lang = Vec::new();
    for lang in [Lang::Ja, Lang::En] {
        i18n::set_lang(lang);
        by_lang.push((lang, observe(&dirs)));
    }
    i18n::set_lang(original);

    let olds: Vec<&str> = cases().iter().map(|c| c.old_ja).collect();
    let mut failures = Vec::new();
    for (lang, seen) in &by_lang {
        assert_eq!(seen.len(), olds.len(), "ケースの数");
        for (s, old) in seen.iter().zip(&olds) {
            println!("[{lang:?}] {}: {}", s.name, s.text);
            match lang {
                Lang::Ja => {
                    let want = expand_old(old, &s.dir);
                    if s.text != want {
                        failures.push(format!(
                            "[Ja] {}: #2365 より前の文と違う\n  want: {want}\n  got:  {}",
                            s.name, s.text
                        ));
                    }
                }
                Lang::En => {
                    if let Some(c) = has_japanese(&s.text) {
                        failures.push(format!(
                            "[En] {}: 英語表示の断りに日本語（{c}）が混ざる: {}",
                            s.name, s.text
                        ));
                    }
                    if s.text.starts_with('<') {
                        failures.push(format!("[En] {}: 断りの形が違う: {}", s.name, s.text));
                    }
                }
            }
        }
    }
    // 英語表示の文は日本語表示の文と違う（英語の書き忘れで日本語を写していない）
    let (ja, en) = (&by_lang[0].1, &by_lang[1].1);
    for (j, e) in ja.iter().zip(en) {
        if j.text == e.text {
            failures.push(format!("{}: 日英が同じ文: {}", j.name, j.text));
        }
    }
    // 断られたケースは何も書かない（profiles set / create の検証は書く前に断る）
    for name in ["x.yaml", "new2365.yaml"] {
        let written = dirs[0].0.join("profiles").join(name);
        if written.exists() {
            failures.push(format!("断ったのに書いた: {}", written.display()));
        }
    }
    // 読めないファイルを戻してから消す（000 のままでは remove_dir_all が落ちる）
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            dirs[2].0.join("projects.yaml"),
            std::fs::Permissions::from_mode(0o600),
        );
    }
    let _ = std::fs::remove_dir_all(&root);
    let total = by_lang[0].1.len();
    println!("ケース {total} 件 × 日英");
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
