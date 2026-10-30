//! 実経路: 会話のカタログ・会話の再開・端末ログ（`sessions` / `logs` / 会話を保った再起動の下見）と、
//! OS の設定・統合の操作（`sleep_guard` / `shell_integration`）の断りが表示言語に従う（Issue #2365 の続き）。
//!
//! 修正前（実測。CLI / MCP と同じ dispatch を GUI の代わりのホストへ通して）: 枠（`invalid parameter:`）は
//! 英語なのに中身が「セッション 'nope' がカタログに見つからない（…）」「残量下限は 5〜90 の範囲で…」
//! 「この環境のシェル統合は環境変数の注入だけで完結するため、…」のように日本語のまま = dispatch が
//! `sessions` / `resume_launch` / `pane_log` / `sleep_guard` / `shell_integration` の `Err(String)`
//! （日本語の固定文）を素通ししていた。
//!
//! ケースごとにカタログ（`TAKO_SESSIONS_FILE`）・orchestrator の設定の置き場（`TAKO_ORCHESTRATOR_DIR`）・
//! data dir（`TAKO_DATA_DIR`）を一時の場所へ向け、
//!
//! - 英語表示: 断りに日本語（かな・漢字）が 1 字も無い
//! - 日本語表示: #2365 より前の文とバイト一致
//!
//! を見る。どのケースも断られて何も起こさない（ペインを作らない・設定を書かない）。OS の設定を本当に
//! 変える操作（蓋閉じ継続の登録 / 解除 = 管理者権限の確認・フルディスクアクセスと Battery の
//! システム設定を開く）は叩かない（文の日英は番犬 `issue1762_dispatch_error_lang.rs` が表で見る）。
//! 表示言語と env はプロセス全体の状態なので、**1 本のテスト関数を `run_alone!` の子**で走らせる
//! （#2400）。このファイルは修正前の木でもコンパイルできる API だけを使う（A/B: 修正前は英語表示の
//! 検査で落ちる）
//!
//! unix だけで走らせる: 読めないファイル（パーミッション 000）と「通常ファイルの下のフォルダ」で
//! 書き出しを断らせる形が unix の前提
#![cfg(unix)]

use std::path::Path;

use serde_json::{json, Value};
use tako_control::protocol::Request;
use tako_control::{
    DispatchError, PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost, UiStateHost,
    WebViewHost, WorkspaceHost,
};
use tako_core::i18n::{self, Lang};
use tako_core::{Pane, PaneId, PaneOrigin, SpawnOptions, TerminalSession, Workspace};

/// GUI の代わり: ペインの木だけを持つ（どの要求も断られ、ペインを作らない）
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

// ---- 会話 ID（カタログの前方一致・短縮 ID = 先頭 8 文字） ----
/// master・壊れたプロファイル（`profiles/broken.yaml`）・会話を保った再起動の下見の対象ペイン
const A1: &str = "aaaa1111-2365-4000-8000-000000000001";
/// master・master_agent が codex のプロファイル
const A2: &str = "aaaa2222-2365-4000-8000-000000000002";
/// ローカル LLM（resume の手段を持たない系統）
const B1: &str = "bbbb1111-2365-4000-8000-000000000003";
/// 端末ログが読めない（パーミッション 000）
const C1: &str = "cccc1111-2365-4000-8000-000000000004";
/// worker（起動する側の master の既定プロファイルの env に内部変数）
const D1: &str = "dddd1111-2365-4000-8000-000000000005";
/// master・既定プロファイル（system prompt の書き出しで断る）
const E1: &str = "eeee1111-2365-4000-8000-000000000006";
/// 会話の記録（transcript）が無い master（会話を保った再起動の下見の対象ペイン）
const Z1: &str = "ffff1111-2365-4000-8000-000000000007";

/// GUI の代わりのホストのペイン（Workspace::new の最初のペイン）
fn host() -> (Host, u64) {
    let host = Host {
        workspace: Workspace::new("1", Pane::new(PaneOrigin::User)),
    };
    let pane = host
        .workspace
        .all_pane_ids()
        .into_iter()
        .next()
        .expect("ペイン")
        .as_u64();
    (host, pane)
}

const BROKEN_CATALOG: &str = "entries: [\n";
const BROKEN_PROFILE: &str = "effort: [1, 2]\n";

/// 置き場を作る（`root` の下。読めるカタログはケースごとに [`write_catalogs`] が書く）
fn make_fixture(root: &Path) {
    use std::os::unix::fs::PermissionsExt;
    // 会話の記録（claude の既定の設定 dir = `$HOME/.claude`）。Z1 だけ置かない
    let projects = root.join("home/.claude/projects/p2365");
    std::fs::create_dir_all(&projects).unwrap();
    for id in [A1, A2, B1, C1, D1, E1] {
        std::fs::write(projects.join(format!("{id}.jsonl")), "{}\n").unwrap();
    }
    // 端末ログ（読めない）
    std::fs::create_dir_all(root.join("logs")).unwrap();
    let log = root.join("logs/c1.log");
    std::fs::write(&log, "x\n").unwrap();
    std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o000)).unwrap();

    // カタログ（壊れたもの・読めないもの）
    let cat = root.join("cat");
    std::fs::create_dir_all(&cat).unwrap();
    std::fs::write(cat.join("broken.yaml"), BROKEN_CATALOG).unwrap();
    let unreadable = cat.join("unreadable.yaml");
    std::fs::write(&unreadable, "entries: {}\n").unwrap();
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();

    // orchestrator の設定の置き場
    let ok = root.join("orch/ok/profiles");
    std::fs::create_dir_all(&ok).unwrap();
    std::fs::write(ok.join("broken.yaml"), BROKEN_PROFILE).unwrap();
    std::fs::write(ok.join("codexm.yaml"), "master_agent: codex\n").unwrap();
    let envbad = root.join("orch/envbad/profiles");
    std::fs::create_dir_all(&envbad).unwrap();
    std::fs::write(envbad.join("default.yaml"), "env:\n  TAKO_SOCKET: x\n").unwrap();
    // system prompt の書き出し先がフォルダ
    std::fs::create_dir_all(root.join("orch/pw/_system_prompt_default.md")).unwrap();
    // 通常ファイル（この下にフォルダを作れない）
    std::fs::write(root.join("afile"), "x\n").unwrap();
    std::fs::create_dir_all(root.join("data")).unwrap();
}

/// 読めるカタログを書く。会話を保った再起動の下見はペインから会話を逆引きするので、
/// `pane` はそのケースの GUI の代わりのホストのペイン（ホストを作るたびに ID が進む）
fn write_catalogs(root: &Path, pane: u64) {
    let cat = root.join("cat");
    let log = root.join("logs/c1.log");
    std::fs::write(
        cat.join("ok.yaml"),
        format!(
            "entries:\n\
             \x20 {A1}:\n    kind: master\n    profile: broken\n    agent: claude\n    pane: {pane}\n\
             \x20 {A2}:\n    kind: master\n    profile: codexm\n    agent: claude\n\
             \x20 {B1}:\n    agent: local\n\
             \x20 {C1}:\n    kind: master\n    agent: claude\n    log_file: {}\n\
             \x20 {D1}:\n    kind: worker\n    project: p2365\n    agent: claude\n\
             \x20 {E1}:\n    kind: master\n    agent: claude\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::write(
        cat.join("nt.yaml"),
        format!("entries:\n  {Z1}:\n    kind: master\n    agent: claude\n    pane: {pane}\n"),
    )
    .unwrap();
}

/// ケースの env（`root` からの相対）
#[derive(Clone, Copy)]
struct Env {
    catalog: &'static str,
    orch: &'static str,
    data: &'static str,
}

const OK: Env = Env {
    catalog: "cat/ok.yaml",
    orch: "orch/ok",
    data: "data",
};

/// 断りを引き出す呼び方
#[derive(Clone, Copy)]
enum Pick {
    /// 断り（`Err`）の文
    Err,
    /// 成功の応答の中の文（会話を保った再起動の下見の `resume_error`）
    Field(&'static str),
}

struct Case {
    name: &'static str,
    env: Env,
    /// `{"method", "params"}`。`<pane>` は GUI の代わりのホストのペイン ID
    request: Value,
    pick: Pick,
    /// #2365 より前の日本語（`<root>` は一時の置き場・`<yaml:…>` は YAML の理由）
    old_ja: &'static str,
}

fn case(name: &'static str, env: Env, request: Value, old_ja: &'static str) -> Case {
    Case {
        name,
        env,
        request,
        pick: Pick::Err,
        old_ja,
    }
}

fn cases() -> Vec<Case> {
    let sessions = |params: Value| json!({ "method": "sessions", "params": params });
    let with = |catalog: &'static str| Env { catalog, ..OK };
    let orch = |orch: &'static str| Env { orch, ..OK };
    vec![
        // ---- カタログ（CLI `tako sessions` / MCP `tako_sessions`） ----
        case(
            "sessions list（壊れた sessions.yaml）",
            with("cat/broken.yaml"),
            sessions(json!({ "action": "list" })),
            "sessions.yaml のパースに失敗: <yaml:sessions>",
        ),
        case(
            "sessions list（読めない sessions.yaml）",
            with("cat/unreadable.yaml"),
            sessions(json!({ "action": "list" })),
            "sessions.yaml の読み取りに失敗: Permission denied (os error 13)",
        ),
        case(
            "sessions show（無い）",
            OK,
            sessions(json!({ "action": "show", "id": "nope" })),
            "セッション 'nope' がカタログに見つからない（tako sessions list で確認）",
        ),
        case(
            "sessions show（候補が複数）",
            OK,
            sessions(json!({ "action": "show", "id": "aaaa" })),
            "セッション 'aaaa' の候補が 2 件ある（もう少し長い ID を指定）: aaaa1111, aaaa2222",
        ),
        case(
            "sessions resume（無い）",
            OK,
            sessions(json!({ "action": "resume", "id": "nope", "pane": "<pane>" })),
            "セッション 'nope' がカタログに見つからない（tako sessions list で確認）",
        ),
        case(
            "sessions link（無い）",
            OK,
            sessions(json!({ "action": "link", "id": "nope" })),
            "セッション 'nope' がカタログに見つからない（tako sessions list で確認）",
        ),
        // ---- 端末ログ（CLI `tako logs show` / MCP `tako_logs`） ----
        case(
            "logs read（カタログに無い会話）",
            OK,
            json!({ "method": "logs", "params": { "action": "read", "session_id": "nope" } }),
            "セッション 'nope' がカタログに見つからない（tako sessions list で確認）",
        ),
        case(
            "logs read（読めないログ）",
            OK,
            json!({ "method": "logs", "params": { "action": "read", "session_id": C1 } }),
            "ログを読めない (<root>/logs/c1.log): Permission denied (os error 13)",
        ),
        // ---- 会話の再開（resume_launch） ----
        case(
            "sessions resume（壊れたプロファイル）",
            OK,
            sessions(json!({ "action": "resume", "id": A1, "pane": "<pane>" })),
            "プロファイルのパースに失敗: <yaml:profile>",
        ),
        case(
            "sessions resume（master が codex のプロファイル）",
            OK,
            sessions(json!({ "action": "resume", "id": A2, "pane": "<pane>" })),
            "プロファイル 'codexm' のエージェントは codex なので claude の会話を\
             その組み立てで再開できない（`tako sessions resume <id>` で開く）",
        ),
        case(
            "sessions resume（resume の手段を持たない系統）",
            OK,
            sessions(json!({ "action": "resume", "id": B1, "pane": "<pane>" })),
            "agent 'local' のセッションは resume 非対応（対応: claude / codex / agy）",
        ),
        case(
            "sessions resume（worker の起動元のプロファイルの env に内部変数）",
            orch("orch/envbad"),
            sessions(json!({ "action": "resume", "id": D1, "pane": "<pane>" })),
            "プロファイルの env に tako 内部変数が含まれている（上書き禁止）: TAKO_SOCKET",
        ),
        case(
            "sessions resume（system prompt の保存先を作れない）",
            orch("afile/orch"),
            sessions(json!({ "action": "resume", "id": E1, "pane": "<pane>" })),
            "system prompt の保存先を作れない: Not a directory (os error 20)",
        ),
        case(
            "sessions resume（system prompt を書き出せない）",
            orch("orch/pw"),
            sessions(json!({ "action": "resume", "id": E1, "pane": "<pane>" })),
            "system prompt の書き出しに失敗: Is a directory (os error 21)",
        ),
        Case {
            name: "session_restart の下見（壊れたプロファイル）の resume_error",
            env: OK,
            request: json!({ "method": "session_restart", "params": { "pane": "<pane>" } }),
            pick: Pick::Field("resume_error"),
            old_ja: "プロファイルのパースに失敗: <yaml:profile>",
        },
        Case {
            name: "session_restart の下見（会話の記録が無い）の resume_error",
            env: with("cat/nt.yaml"),
            request: json!({ "method": "session_restart", "params": { "pane": "<pane>" } }),
            pick: Pick::Field("resume_error"),
            old_ja: "会話 ffff1111 の記録（claude の transcript）が見つからない。\
                     削除されたか、登録されていない設定 dir にある（`tako orchestrator accounts list`）",
        },
        // ---- スリープ防止（MCP `tako_sleep_guard`。CLI は GUI を通さない手元の処理） ----
        case(
            "sleep_guard set（残量下限が範囲の外・下）",
            OK,
            json!({ "method": "sleep_guard", "params": { "action": "set", "lid_battery_floor": 1 } }),
            "無効なパラメータ: 残量下限は 5〜90 の範囲で指定してください（指定値: 1）",
        ),
        case(
            "sleep_guard set（残量下限が範囲の外・上）",
            OK,
            json!({ "method": "sleep_guard", "params": { "action": "set", "lid_battery_floor": 91 } }),
            "無効なパラメータ: 残量下限は 5〜90 の範囲で指定してください（指定値: 91）",
        ),
        // ---- シェル統合（MCP `tako_shell_integration`。CLI は GUI を通さない手元の処理） ----
        case(
            "shell_integration（不明な action）",
            OK,
            json!({ "method": "shell_integration", "params": { "action": "bogus" } }),
            "無効なパラメータ: 不明な action: \"bogus\"（status / install / uninstall のいずれか）",
        ),
        case(
            "shell_integration uninstall（解除する配置が無い）",
            OK,
            json!({ "method": "shell_integration", "params": { "action": "uninstall" } }),
            "無効なパラメータ: この環境のシェル統合は環境変数の注入だけで完結するため、解除する配置がありません",
        ),
        case(
            "shell_integration install（data dir に書けない）",
            Env {
                data: "afile/data",
                ..OK
            },
            json!({ "method": "shell_integration", "params": { "action": "install" } }),
            "無効なパラメータ: 統合スクリプトを書き出せません: Not a directory (os error 20)",
        ),
    ]
}

/// YAML の理由（#2365 より前の `yaml_error_reason` と同じ組み立て = 種類と位置の日本語）
fn yaml_ja<T: serde::de::DeserializeOwned>(text: &str) -> String {
    let Err(e) = serde_yaml::from_str::<T>(text) else {
        panic!("壊れた YAML のはずが読めた: {text}");
    };
    tako_core::load_issue::yaml_error_reason(&e, text)
}

fn expand_old(old: &str, root: &Path) -> String {
    use tako_control::orchestrator::Profile;
    use tako_control::sessions::SessionCatalog;
    old.replace("<root>", &root.display().to_string())
        .replace(
            "<yaml:sessions>",
            &yaml_ja::<SessionCatalog>(BROKEN_CATALOG),
        )
        .replace("<yaml:profile>", &yaml_ja::<Profile>(BROKEN_PROFILE))
}

/// `"<pane>"` をホストのペイン ID へ置き換える
fn with_pane(mut v: Value, pane: u64) -> Value {
    if let Some(params) = v.get_mut("params").and_then(Value::as_object_mut) {
        for value in params.values_mut() {
            if value == "<pane>" {
                *value = json!(pane);
            }
        }
    }
    v
}

fn observe(root: &Path, by_case: &[Case]) -> Vec<String> {
    by_case
        .iter()
        .map(|c| {
            std::env::set_var("TAKO_SESSIONS_FILE", root.join(c.env.catalog));
            std::env::set_var("TAKO_ORCHESTRATOR_DIR", root.join(c.env.orch));
            std::env::set_var("TAKO_DATA_DIR", root.join(c.env.data));
            let (mut host, pane) = host();
            write_catalogs(root, pane);
            let request: Request =
                serde_json::from_value(with_pane(c.request.clone(), pane)).expect("要求の形");
            let result: Result<Value, DispatchError> =
                tako_control::dispatch(&mut host, request, PaneOrigin::Cli);
            match (c.pick, result) {
                (Pick::Err, Err(e)) => e.to_string(),
                (Pick::Err, Ok(v)) => format!("<断られずに通った: {v}>"),
                (Pick::Field(key), Ok(v)) => v[key]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("<{key} が無い: {v}>")),
                (Pick::Field(_), Err(e)) => format!("<断られた: {e}>"),
            }
        })
        .collect()
}

#[test]
fn 会話のカタログと再開とosの設定の断りは表示言語に従う() {
    if tako_core::run_alone!(会話のカタログと再開とosの設定の断りは表示言語に従う)
    {
        return;
    }
    let root = std::env::temp_dir().join(format!("tako-2365s-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("一時の置き場");
    // 本番の設定・会話の記録を読み書きしない（どの置き場も一時の場所の下）
    std::env::set_var("HOME", root.join("home"));
    std::env::remove_var("CLAUDE_CONFIG_DIR");
    make_fixture(&root);

    let all = cases();
    let original = i18n::lang();
    let mut by_lang = Vec::new();
    for lang in [Lang::Ja, Lang::En] {
        i18n::set_lang(lang);
        by_lang.push((lang, observe(&root, &all)));
    }
    i18n::set_lang(original);

    let mut failures = Vec::new();
    let mut en_japanese = 0;
    for (lang, seen) in &by_lang {
        assert_eq!(seen.len(), all.len(), "ケースの数");
        for (text, c) in seen.iter().zip(&all) {
            println!("[{lang:?}] {}: {text}", c.name);
            match lang {
                Lang::Ja => {
                    let want = expand_old(c.old_ja, &root);
                    if *text != want {
                        failures.push(format!(
                            "[Ja] {}: #2365 より前の文と違う\n  want: {want}\n  got:  {text}",
                            c.name
                        ));
                    }
                }
                Lang::En => {
                    if let Some(ch) = has_japanese(text) {
                        en_japanese += 1;
                        failures.push(format!(
                            "[En] {}: 英語表示の断りに日本語（{ch}）が混ざる: {text}",
                            c.name
                        ));
                    }
                    if text.starts_with('<') {
                        failures.push(format!("[En] {}: 断りの形が違う: {text}", c.name));
                    }
                }
            }
        }
    }
    // 英語表示の文は日本語表示の文と違う（英語の書き忘れで日本語を写していない）
    let (ja, en) = (&by_lang[0].1, &by_lang[1].1);
    for ((j, e), c) in ja.iter().zip(en).zip(&all) {
        if j == e {
            failures.push(format!("{}: 日英が同じ文: {j}", c.name));
        }
    }
    // 断ったケースはペインを作らず、シェル統合のスクリプトも書かない
    if root.join("data/shell-integration").exists() {
        failures.push("断ったのにシェル統合のスクリプトを書いた".to_string());
    }
    // 読めないファイルを戻してから消す（000 のままでは remove_dir_all が落ちる）
    {
        use std::os::unix::fs::PermissionsExt;
        for f in ["logs/c1.log", "cat/unreadable.yaml"] {
            let _ = std::fs::set_permissions(root.join(f), std::fs::Permissions::from_mode(0o600));
        }
    }
    let _ = std::fs::remove_dir_all(&root);
    println!(
        "ケース {} 件 × 日英・英語表示の日本語入り {en_japanese} 件",
        all.len()
    );
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
