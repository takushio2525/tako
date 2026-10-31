//! 実経路: orchestrator の委任台帳（`tako_orchestrator_ledger`）・非同期 run の照会と回収・手順書・
//! 引き継ぎファイルの書き込み・supervisor の set_mode と、設定値の範囲（preview-cache / scrollback /
//! redraw-limit）・paste-guard の許可リストと規則の断りが表示言語に従う（Issue #2365 の続き）。
//!
//! 修正前（実測。CLI / MCP と同じ dispatch を GUI の代わりのホストへ通して）: 枠（`invalid parameter:`）は
//! 英語なのに中身が「不正な outcome 'bogus'。使用可能: …」「run_id '…' が見つからない」「max_mb は 256〜8192 の
//! 範囲で指定する」「規則 bogus は無い（…）」のように日本語のまま = dispatch が `ledger.rs` / `wait.rs` /
//! `guide.rs` / `handoff_store.rs` / プロファイルの書き込みと tako-core の検証の口の `Err(String)`
//! （日本語の固定文）を素通ししていた。
//!
//! ケースごとに orchestrator の置き場（`TAKO_ORCHESTRATOR_DIR`）を一時の場所へ向け、
//!
//! - 英語表示: 断りに日本語（かな・漢字）が 1 字も無い
//! - 日本語表示: #2365 より前の文とバイト一致
//! - 表示言語を切り替えた直後の 1 件目から、切り替えた言語で返る
//!
//! を見る。どのケースも断られて何も書かない（台帳・プロファイル・引き継ぎファイルを作らない）。
//! 表示言語と env はプロセス全体の状態なので、**1 本のテスト関数を `run_alone!` の子**で走らせる
//! （#2400）。このファイルは修正前の木でもコンパイルできる API だけを使う（A/B: 修正前は英語表示の
//! 検査で落ちる）
//!
//! unix だけで走らせる: 読めないファイル（パーミッション 000）と「ロックファイルの置き場がディレクトリ」
//! 「置き場のディレクトリの位置に通常ファイル」で断らせる形が unix の前提
#![cfg(unix)]

use std::path::{Path, PathBuf};

use tako_control::protocol::Request;
use tako_control::{
    PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost, UiStateHost, WebViewHost,
    WorkspaceHost,
};
use tako_core::i18n::{self, Lang};
use tako_core::paste_guard::{AllowEntry, PasteGuardStatus, Policy};
use tako_core::{Pane, PaneId, PaneOrigin, SpawnOptions, TerminalSession, Workspace};

/// GUI の代わり: ペインの木と paste-guard の許可リストだけを持つ（どの要求も断られ、何も変えない）
struct Host {
    workspace: Workspace,
    policy: Policy,
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
    fn paste_guard_status(&self) -> PasteGuardStatus {
        PasteGuardStatus {
            policy: self.policy.clone(),
            ..PasteGuardStatus::default()
        }
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

/// orchestrator の置き場の種類（ケースごとに `TAKO_ORCHESTRATOR_DIR` を向ける）
#[derive(Clone, Copy)]
enum Dir {
    /// 空（台帳は無い = 0 件として読む）
    Ok,
    /// ledger.yaml が YAML として壊れ、profiles/default.yaml の形が違う
    Broken,
    /// ledger.yaml を読めない（パーミッション 000）
    Unreadable,
    /// 台帳のロックファイルの置き場がディレクトリ・`handoff` の位置に通常ファイル
    Blocked,
}

const BROKEN_LEDGER: &str = "entries: [\n";
const BROKEN_PROFILE: &str = "effort: [1, 2]\n";

/// 置き場を作る（`root/<name>`）
fn make_dirs(root: &Path) -> [(PathBuf, Dir); 4] {
    use std::os::unix::fs::PermissionsExt;
    let ok = root.join("ok");
    std::fs::create_dir_all(&ok).unwrap();
    let broken = root.join("broken");
    std::fs::create_dir_all(broken.join("profiles")).unwrap();
    std::fs::write(broken.join("ledger.yaml"), BROKEN_LEDGER).unwrap();
    std::fs::write(broken.join("profiles/default.yaml"), BROKEN_PROFILE).unwrap();
    let unreadable = root.join("unreadable");
    std::fs::create_dir_all(&unreadable).unwrap();
    let ledger = unreadable.join("ledger.yaml");
    std::fs::write(&ledger, "entries: []\n").unwrap();
    std::fs::set_permissions(&ledger, std::fs::Permissions::from_mode(0o000)).unwrap();
    let blocked = root.join("blocked");
    std::fs::create_dir_all(blocked.join("ledger.yaml.lock")).unwrap();
    std::fs::write(blocked.join("handoff"), "not a folder\n").unwrap();
    [
        (ok, Dir::Ok),
        (broken, Dir::Broken),
        (unreadable, Dir::Unreadable),
        (blocked, Dir::Blocked),
    ]
}

/// paste-guard の許可リスト（GUI の保存済みの設定の代わり）
#[derive(Clone, Copy)]
enum Allow {
    /// 空
    Empty,
    /// 先頭 4 文字（abcd）が同じ 2 行
    Twins,
    /// 上限（200 件）まで埋まっている
    Full,
}

fn entry(fingerprint: String) -> AllowEntry {
    AllowEntry {
        fingerprint,
        rule: "google_api_key".into(),
        masked: "AIza…".into(),
    }
}

fn policy(allow: Allow) -> Policy {
    let allow = match allow {
        Allow::Empty => Vec::new(),
        Allow::Twins => vec![
            entry(format!("abcd{}", "0".repeat(60))),
            entry(format!("abcd{}", "1".repeat(60))),
        ],
        Allow::Full => (0..tako_core::paste_guard::MAX_ALLOW)
            .map(|i| entry(format!("{i:064x}")))
            .collect(),
    };
    Policy {
        allow,
        ..Policy::default()
    }
}

/// 規則に当たる見本の値（伏せる前の形をソースに並べない = 実行時に組む）
fn fake_google_key() -> String {
    format!("{}{}", "AI", "zaEXAMPLE-abcdefghijklmnopqrstuvwxyz0")
}

struct Case {
    name: &'static str,
    dir: Dir,
    allow: Allow,
    request: Request,
    /// #2365 より前の日本語（`<dir>` は置き場のパス・`<yaml:ledger>` / `<yaml:profile>` は YAML の理由・
    /// `<topics>` は手順書の topic の一覧・`<rules>` は規則の名前の一覧）
    old_ja: &'static str,
}

fn ledger(action: &str, id: Option<&str>, outcome: Option<&str>, note: Option<&str>) -> Request {
    Request::OrchestratorLedger {
        action: action.to_string(),
        id: id.map(str::to_string),
        outcome: outcome.map(str::to_string),
        rounds: None,
        note: note.map(str::to_string),
        project: if action == "prune" {
            Some("p2365".to_string())
        } else {
            None
        },
        task_type: None,
        limit: None,
    }
}

fn paste_guard(
    allow: Option<String>,
    unallow: Option<&str>,
    rules: Option<(&str, bool)>,
) -> Request {
    Request::PasteGuard {
        enabled: None,
        check: None,
        respond: None,
        mode: None,
        allow,
        unallow: unallow.map(str::to_string),
        rules: rules.map(|(name, on)| [(name.to_string(), on)].into_iter().collect()),
        pane: None,
    }
}

fn cases() -> Vec<Case> {
    vec![
        // ---- 委任台帳（MCP `tako_orchestrator_ledger`） ----
        Case {
            name: "ledger record（outcome が不正）",
            dir: Dir::Ok,
            allow: Allow::Empty,
            request: ledger("record", Some("spawn_1"), Some("bogus"), None),
            old_ja: "不正な outcome 'bogus'。使用可能: pass, rework, fail",
        },
        Case {
            name: "ledger record（エントリが無い）",
            dir: Dir::Ok,
            allow: Allow::Empty,
            request: ledger("record", Some("nope2365"), Some("pass"), None),
            old_ja: "エントリ 'nope2365' が見つからない",
        },
        Case {
            name: "ledger amend（エントリが無い）",
            dir: Dir::Ok,
            allow: Allow::Empty,
            request: ledger("amend", Some("nope2365"), None, Some("x")),
            old_ja: "エントリ 'nope2365' が見つからない",
        },
        Case {
            name: "ledger list（壊れた ledger.yaml）",
            dir: Dir::Broken,
            allow: Allow::Empty,
            request: ledger("list", None, None, None),
            old_ja: "ledger.yaml のパースに失敗: <yaml:ledger>",
        },
        Case {
            name: "ledger stats（読めない ledger.yaml）",
            dir: Dir::Unreadable,
            allow: Allow::Empty,
            request: ledger("stats", None, None, None),
            old_ja: "ledger.yaml の読み取りに失敗: Permission denied (os error 13)",
        },
        Case {
            name: "ledger prune（ロックを取れない）",
            dir: Dir::Blocked,
            allow: Allow::Empty,
            request: ledger("prune", None, None, None),
            old_ja:
                "ロックファイルを開けない (<dir>/ledger.yaml.lock): Is a directory (os error 21)",
        },
        // ---- 非同期 run（CLI `tako orchestrator run-status` / `run-result`・MCP） ----
        Case {
            name: "run_status（無い run_id）",
            dir: Dir::Ok,
            allow: Allow::Empty,
            request: Request::OrchestratorRunStatus {
                run_id: Some("run2365".into()),
            },
            old_ja: "run_id 'run2365' が見つからない",
        },
        Case {
            name: "run_result（無い run_id）",
            dir: Dir::Ok,
            allow: Allow::Empty,
            request: Request::OrchestratorRunResult {
                run_id: "run2365".into(),
            },
            old_ja: "run_id 'run2365' が見つからない",
        },
        // ---- 手順書（MCP `tako_orchestrator_guide`） ----
        Case {
            name: "guide（知らない topic）",
            dir: Dir::Ok,
            allow: Allow::Empty,
            request: Request::OrchestratorGuide {
                topic: Some("nope2365".into()),
                profile: Some("default".into()),
                caller_role: None,
                pane: None,
                caller_pid: None,
            },
            old_ja: "無効なパラメータ: 未知の topic: nope2365（引けるのは <topics>）",
        },
        // ---- 引き継ぎファイルの書き込み（CLI `tako orchestrator handoffs write`・MCP） ----
        Case {
            name: "handoffs write（キーがファイル名にならない）",
            dir: Dir::Ok,
            allow: Allow::Empty,
            request: Request::OrchestratorHandoffFiles {
                action: "write".into(),
                project: Some("../x".into()),
                profile: None,
                content: Some("x\n".into()),
            },
            old_ja: "プロジェクトキーがファイル名として使えない: ../x",
        },
        Case {
            name: "handoffs write（置き場を作れない）",
            dir: Dir::Blocked,
            allow: Allow::Empty,
            request: Request::OrchestratorHandoffFiles {
                action: "write".into(),
                project: Some("k2365".into()),
                profile: None,
                content: Some("x\n".into()),
            },
            old_ja:
                "引き継ぎの置き場を作れない: <dir>/handoff/projects (Not a directory (os error 20))",
        },
        // ---- supervisor（CLI `tako orchestrator supervisor set-mode`・MCP） ----
        Case {
            name: "supervisor set_mode（壊れた default.yaml）",
            dir: Dir::Broken,
            allow: Allow::Empty,
            request: Request::OrchestratorSupervisor {
                action: "set_mode".into(),
                mode: Some("auto".into()),
                auto_resume_dead: None,
                max_retries: None,
                lines: None,
            },
            old_ja: "プロファイルのパースに失敗: <yaml:profile>",
        },
        // ---- 設定値の範囲（CLI `tako preview-cache` / `scrollback` / `redraw-limit`・MCP） ----
        Case {
            name: "preview-cache（範囲の外）",
            dir: Dir::Ok,
            allow: Allow::Empty,
            request: Request::PreviewCache { max_mb: Some(1) },
            old_ja: "無効なパラメータ: max_mb は 256〜8192 の範囲で指定する",
        },
        Case {
            name: "scrollback（範囲の外）",
            dir: Dir::Ok,
            allow: Allow::Empty,
            request: Request::Scrollback {
                lines: Some(1),
                unfocused_fps: None,
                mouse_forward: None,
            },
            old_ja: "無効なパラメータ: lines は 100〜100000 の範囲で指定する（既定 10000）",
        },
        Case {
            name: "redraw-limit（範囲の外）",
            dir: Dir::Ok,
            allow: Allow::Empty,
            request: Request::Scrollback {
                lines: None,
                unfocused_fps: Some(999),
                mouse_forward: None,
            },
            old_ja: "無効なパラメータ: unfocused_fps は 1〜60 の範囲で指定する\
                     （既定 30。フォーカス中のペインは常に 60）",
        },
        // ---- paste-guard の許可リストと規則（CLI `tako paste-guard allow / unallow / rule`・MCP） ----
        Case {
            name: "paste-guard rule（知らない規則）",
            dir: Dir::Ok,
            allow: Allow::Empty,
            request: paste_guard(None, None, Some(("bogus", true))),
            old_ja: "無効なパラメータ: 規則 bogus は無い（<rules>）",
        },
        Case {
            name: "paste-guard unallow（ID の形が違う）",
            dir: Dir::Ok,
            allow: Allow::Empty,
            request: paste_guard(None, Some("xy"), None),
            old_ja:
                "無効なパラメータ: 外す行は ID（16 進 4 文字以上）か all で指定する（指定: xy）",
        },
        Case {
            name: "paste-guard unallow（当たる行が無い）",
            dir: Dir::Ok,
            allow: Allow::Empty,
            request: paste_guard(None, Some("ABCD"), None),
            old_ja: "無効なパラメータ: 許可リストに ID abcd の行が無い",
        },
        Case {
            name: "paste-guard unallow（2 行に当たる）",
            dir: Dir::Ok,
            allow: Allow::Twins,
            request: paste_guard(None, Some("abcd"), None),
            old_ja: "無効なパラメータ: ID abcd に 2 行が当たる（もっと長く指定する）",
        },
        Case {
            name: "paste-guard allow（許可リストが一杯）",
            dir: Dir::Ok,
            allow: Allow::Full,
            request: paste_guard(Some(fake_google_key()), None, None),
            old_ja: "無効なパラメータ: 許可リストは 200 件まで（今 200 件・足す 1 件）",
        },
    ]
}

/// GUI の代わりのホストへ 1 件通した断りの文（断られなければ `<…>` で返す）
fn run(dirs: &[(PathBuf, Dir)], c: &Case) -> String {
    let dir = dirs
        .iter()
        .find(|(_, d)| std::mem::discriminant(d) == std::mem::discriminant(&c.dir))
        .map(|(p, _)| p)
        .expect("置き場");
    std::env::set_var("TAKO_ORCHESTRATOR_DIR", dir);
    let mut host = Host {
        workspace: Workspace::new("1", Pane::new(PaneOrigin::User)),
        policy: policy(c.allow),
    };
    match tako_control::dispatch(&mut host, c.request.clone(), PaneOrigin::Mcp) {
        Err(e) => e.to_string(),
        Ok(v) => format!("<断られなかった: {v}>"),
    }
}

/// 旧文の印を、ケースの置き場と YAML の理由・一覧で展開する
fn expand_old(old: &str, dir: &Path) -> String {
    let yaml_ledger =
        serde_yaml::from_str::<tako_control::orchestrator::ledger::Ledger>(BROKEN_LEDGER)
            .expect_err("壊れている");
    let yaml_profile = serde_yaml::from_str::<tako_control::orchestrator::Profile>(BROKEN_PROFILE)
        .expect_err("形が違う");
    let rules: Vec<&str> = tako_core::paste_guard::Rule::ALL
        .iter()
        .map(|r| r.as_str())
        .collect();
    old.replace("<dir>", &dir.display().to_string())
        .replace(
            "<yaml:ledger>",
            &tako_core::load_issue::yaml_error_reason(&yaml_ledger, BROKEN_LEDGER),
        )
        .replace(
            "<yaml:profile>",
            &tako_core::load_issue::yaml_error_reason(&yaml_profile, BROKEN_PROFILE),
        )
        .replace(
            "<topics>",
            &tako_control::orchestrator::guide::topics().join(" / "),
        )
        .replace("<rules>", &rules.join(" / "))
}

/// 断られたあとも何も書いていない（台帳・プロファイル・引き継ぎファイル）
fn nothing_written(dirs: &[(PathBuf, Dir)]) -> Vec<String> {
    let mut out = Vec::new();
    for (dir, kind) in dirs {
        for rel in ["ledger.yaml", "profiles/default.yaml", "handoff"] {
            let path = dir.join(rel);
            let expected = match (kind, rel) {
                (Dir::Broken, "ledger.yaml") => Some(BROKEN_LEDGER),
                (Dir::Broken, "profiles/default.yaml") => Some(BROKEN_PROFILE),
                (Dir::Blocked, "handoff") => Some("not a folder\n"),
                (Dir::Unreadable, "ledger.yaml") => continue,
                _ => None,
            };
            match expected {
                Some(text) => {
                    let now = std::fs::read_to_string(&path).unwrap_or_default();
                    if now != text {
                        out.push(format!("断ったのに書き換えた: {}", path.display()));
                    }
                }
                None if path.exists() => out.push(format!("断ったのに作った: {}", path.display())),
                None => {}
            }
        }
    }
    out
}

#[test]
fn 台帳と非同期runと手順書と設定値の断りは表示言語に従う() {
    if tako_core::run_alone!(台帳と非同期runと手順書と設定値の断りは表示言語に従う)
    {
        return;
    }
    let root = std::env::temp_dir().join(format!("tako-2365l-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("一時の置き場");
    let root = root.canonicalize().expect("正規化");
    // 本番の設定・データを読み書きしない（どの置き場も一時の場所の下）
    std::env::set_var("HOME", root.join("home"));
    std::env::set_var("TAKO_DATA_DIR", root.join("data"));
    let dirs = make_dirs(&root);

    let original = i18n::lang();
    let mut failures = Vec::new();
    let (mut ja_count, mut en_count) = (0, 0);
    let mut by_lang: Vec<(Lang, Vec<String>)> = Vec::new();
    for lang in [Lang::En, Lang::Ja] {
        i18n::set_lang(lang);
        let mut seen = Vec::new();
        for c in cases() {
            let got = run(&dirs, &c);
            println!("[{lang:?}] {}: {got}", c.name);
            let dir = &dirs
                .iter()
                .find(|(_, d)| std::mem::discriminant(d) == std::mem::discriminant(&c.dir))
                .expect("置き場")
                .0;
            match lang {
                Lang::En => {
                    en_count += 1;
                    if got.starts_with('<') {
                        failures.push(format!("[en] {}: {got}", c.name));
                    } else if let Some(ch) = has_japanese(&got) {
                        failures.push(format!("[en] {}: 日本語（{ch}）が混ざる: {got}", c.name));
                    }
                }
                Lang::Ja => {
                    ja_count += 1;
                    let want = expand_old(c.old_ja, dir);
                    if got != want {
                        failures.push(format!(
                            "[ja] {}: #2365 より前と違う\n  want: {want}\n  got:  {got}",
                            c.name
                        ));
                    }
                }
            }
            seen.push(got);
        }
        by_lang.push((lang, seen));
    }
    // 英語表示の文は日本語表示の文と違う（英語の書き忘れで日本語を写していない）
    for ((c, en), ja) in cases().iter().zip(&by_lang[0].1).zip(&by_lang[1].1) {
        if en == ja {
            failures.push(format!("{}: 日英が同じ文: {en}", c.name));
        }
    }

    // 表示言語を切り替えた直後の 1 件目から、切り替えた言語で返る（毎回切り替えて 1 件ずつ叩く）
    let probes = cases();
    for (i, lang) in [Lang::Ja, Lang::En, Lang::Ja, Lang::En]
        .into_iter()
        .enumerate()
    {
        i18n::set_lang(lang);
        // 台帳（ConfigError）と設定値（tako-core）の 2 系統を交互に
        let c = &probes[if i % 2 == 0 { 1 } else { 12 }];
        let got = run(&dirs, c);
        let ok = match lang {
            Lang::Ja => got == expand_old(c.old_ja, &dirs[0].0),
            Lang::En => has_japanese(&got).is_none() && !got.starts_with('<'),
        };
        if !ok {
            failures.push(format!(
                "切り替え {i} 回目（{lang:?}）の直後の 1 件目（{}）: {got}",
                c.name
            ));
        }
    }
    i18n::set_lang(original);

    failures.extend(nothing_written(&dirs));
    // 読めないファイルを戻してから消す（000 のままでも消せるが、残骸を残さない）
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            dirs[2].0.join("ledger.yaml"),
            std::fs::Permissions::from_mode(0o600),
        );
    }
    let _ = std::fs::remove_dir_all(&root);
    eprintln!(
        "issue2365_ledger_values_lang: 英語 {en_count} 件・日本語 {ja_count} 件・失敗 {}",
        failures.len()
    );
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
