//! 実経路: dispatch が呼ぶ置き場（受け入れゲート・タスクのチェックポイント・人がやること）の断りが
//! 表示言語に従う（Issue #2365 の続き）。
//!
//! 修正前（実測。dispatch を GUI 無しのホストで）: 英語表示でも「ゲートが見つからない: …」
//! 「不明な phase: …」「タスクが見つからない: …」「user-tasks.yaml のパースに失敗: YAML の構文が
//! 壊れている（…）」のように、置き場（`acceptance_gates` / `task_checkpoints` / `user_tasks` と
//! tako-core の `TaskStore`）が書いた日本語の固定文のまま返った。`tako todo` / `tako task` と MCP の
//! 同じツールは GUI の dispatch を通るので、ここでは GUI の代わりのホストへ IPC と同じ形の要求を渡し、
//! - 英語表示: 応答に日本語（かな・漢字）が 1 字も無い
//! - 日本語表示: #2365 より前の文とバイト一致
//!
//! を見る。置き場はテストのプロセスの隔離先（`paths::data_dir` の一時 dir）にあり、ケースごとに
//! 消してから作る（壊れたファイル・読めないファイル・置き場の親がファイル）。
//!
//! #2365 の続き（全部の置き場が共有する書き出し `config_io`）: 置き場の断りを移した後も、排他・
//! 書き出しの断り（「ロックファイルを開けない (…)」「バックアップの作成に失敗 (…)」「一時ファイルの
//! 書き込みに失敗 (…)」ほか）は `config_io` の日本語の固定文のまま英語表示にも出ていた。ロックの
//! 位置にフォルダ・世代バックアップの位置にフォルダ・書き込めない置き場のフォルダ（unix）を仕込み、
//! 実行設定（`tako run-config` = `runner_defaults` の `path`）の置き場も同じく見る。
//! 表示言語はプロセス全体の状態なので、**専用の統合テストバイナリの 1 本のテスト関数**の中で順に
//! 切り替える。このファイルは修正前の木でもコンパイルできる API だけを使う（A/B: 修正前は英語表示の
//! 検査で落ち、件数を出す）

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tako_control::protocol::Request;
use tako_control::{
    PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost, UiStateHost, WebViewHost,
    WorkspaceHost,
};
use tako_core::i18n::{self, Lang};
use tako_core::{Pane, PaneId, PaneOrigin, SpawnOptions, TerminalSession, Workspace};

/// GUI の代わり: ペインの木だけを持つ
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

impl PreviewHost for Host {}
impl TmuxHost for Host {}
impl UiStateHost for Host {}
impl WebViewHost for Host {}
impl RemoteHost for Host {}
impl SystemHost for Host {}

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

/// 3 つの置き場（テストのプロセスの隔離先の下でなければ止める = 本番の data dir に触らない）
struct Stores {
    gates: PathBuf,
    checkpoints: PathBuf,
    user_tasks: PathBuf,
    run_configs: PathBuf,
    /// 実行設定を書くケースが指すファイル（置き場の外。中身は読むだけ）
    script: PathBuf,
}

impl Stores {
    fn resolve() -> Self {
        let stores = Self {
            gates: tako_control::acceptance_gates::store_path().expect("ゲートの置き場"),
            checkpoints: tako_control::task_checkpoints::store_path()
                .expect("チェックポイントの置き場"),
            user_tasks: tako_control::user_tasks::store_path().expect("ユーザータスクの置き場"),
            run_configs: tako_control::run_configs::store_path().expect("実行設定の置き場"),
            script: std::env::temp_dir()
                .join(format!("tako-2365-run-{}", std::process::id()))
                .join("hello.py"),
        };
        let temp = std::env::temp_dir();
        for path in [
            &stores.gates,
            &stores.checkpoints,
            &stores.user_tasks,
            &stores.run_configs,
            &stores.script,
        ] {
            assert!(
                path.starts_with(&temp),
                "置き場がテストの隔離先の外を指している（本番の data dir に触らない）: {}",
                path.display()
            );
        }
        stores
    }

    /// ケースの前後で状態を持ち越さない（権限を戻してから消す。置き場の親に置いたファイルも消す）
    fn reset(&self) {
        for path in [
            &self.gates,
            &self.checkpoints,
            &self.user_tasks,
            &self.run_configs,
        ] {
            if let Some(parent) = path.parent() {
                make_writable_dir(parent);
            }
            make_readable(path);
            let _ = std::fs::remove_file(path);
            // 排他・世代バックアップの位置に仕込んだもの（フォルダ・ファイル）も消す
            for suffix in [".lock", ".bak.1", ".bak.2", ".bak.3"] {
                let mut name = path.file_name().unwrap_or_default().to_os_string();
                name.push(suffix);
                let sibling = path.with_file_name(name);
                let _ = std::fs::remove_dir_all(&sibling);
                let _ = std::fs::remove_file(&sibling);
            }
        }
        if let Some(parent) = self.user_tasks.parent() {
            if parent.is_file() {
                let _ = std::fs::remove_file(parent);
            }
        }
    }
}

#[cfg(unix)]
fn make_readable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644));
}

#[cfg(not(unix))]
fn make_readable(_path: &Path) {}

/// 書き込めなくした置き場のフォルダを戻す（`ReadOnlyParent` の後片付け）
#[cfg(unix)]
fn make_writable_dir(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if dir.is_dir() {
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755));
    }
}

#[cfg(not(unix))]
fn make_writable_dir(_dir: &Path) {}

/// ケースの前に置き場へ仕込むもの
enum Setup {
    /// 何も置かない（置き場のファイルが無い）
    Nothing,
    /// 中身を書く
    Write(fn(&Stores) -> &Path, &'static str),
    /// 中身を書いて読めなくする（unix）
    Unreadable(fn(&Stores) -> &Path),
    /// 置き場の親フォルダの位置にファイルを置く（フォルダを作れない）
    ParentIsFile(fn(&Stores) -> &Path),
    /// 排他のロックファイルの位置にフォルダを置く（ロックファイルを開けない）
    LockIsDir(fn(&Stores) -> &Path),
    /// 中身を書き、世代バックアップ `.bak.<n>` の位置にフォルダを置く（n = 1: 作れない /
    /// n = 2: `.bak.1` はファイルにして繰り下げられない）
    BackupIsDir(fn(&Stores) -> &Path, u32),
    /// 置き場のフォルダを書き込めなくする（ロックファイルだけ先に置く = 一時ファイルで止まる。unix）
    ReadOnlyParent(fn(&Stores) -> &Path),
}

struct Case {
    name: &'static str,
    setup: Setup,
    wire: Value,
}

fn gates(s: &Stores) -> &Path {
    &s.gates
}
fn checkpoints(s: &Stores) -> &Path {
    &s.checkpoints
}
fn user_tasks(s: &Stores) -> &Path {
    &s.user_tasks
}
fn run_configs(s: &Stores) -> &Path {
    &s.run_configs
}

/// 構文の壊れた YAML（`[` が閉じていない）
const BROKEN: &str = "version: 1\nitems: [\n  - a\n";

fn cases() -> Vec<Case> {
    let req = |method: &str, params: Value| json!({ "method": method, "params": params });
    let case = |name, setup, wire| Case { name, setup, wire };
    let mut out = vec![
        // ---- 受け入れゲート（acceptance_gates）----
        case(
            "gate_criteria_not_json",
            Setup::Nothing,
            req(
                "task_gate",
                json!({ "action": "set", "task_id": "t1", "criteria_json": "not json" }),
            ),
        ),
        case(
            "gate_criteria_empty",
            Setup::Nothing,
            req(
                "task_gate",
                json!({ "action": "set", "task_id": "t1", "criteria_json": "[]" }),
            ),
        ),
        case(
            "gate_criterion_no_id",
            Setup::Nothing,
            req(
                "task_gate",
                json!({ "action": "set", "task_id": "t1", "criteria_json": "[{}]" }),
            ),
        ),
        case(
            "gate_criterion_kind",
            Setup::Nothing,
            req(
                "task_gate",
                json!({
                    "action": "set",
                    "task_id": "t1",
                    "criteria_json": r#"[{"id":"a","kind":{"type":"bogus"}}]"#,
                }),
            ),
        ),
        case(
            "gate_criterion_status",
            Setup::Nothing,
            req(
                "task_gate",
                json!({
                    "action": "set",
                    "task_id": "t1",
                    "criteria_json":
                        r#"[{"id":"a","kind":{"type":"custom","description":"d"},"status":"weird"}]"#,
                }),
            ),
        ),
        case(
            "gate_show_missing",
            Setup::Nothing,
            req("task_gate", json!({ "action": "show", "task_id": "nope" })),
        ),
        case(
            "gate_results_not_json",
            Setup::Nothing,
            req(
                "task_gate",
                json!({ "action": "record_results", "task_id": "t1", "results_json": "x" }),
            ),
        ),
        case(
            "gate_result_no_id",
            Setup::Nothing,
            req(
                "task_gate",
                json!({ "action": "record_results", "task_id": "t1", "results_json": "[{}]" }),
            ),
        ),
        case(
            "gate_record_missing",
            Setup::Nothing,
            req(
                "task_gate",
                json!({ "action": "record_results", "task_id": "nope", "results_json": "[]" }),
            ),
        ),
        case(
            "gate_store_broken",
            Setup::Write(gates, BROKEN),
            req("task_gate", json!({ "action": "show", "task_id": "t1" })),
        ),
        // ---- タスクのチェックポイント（task_checkpoints）----
        case(
            "checkpoint_list_phase",
            Setup::Nothing,
            req(
                "task_checkpoint",
                json!({ "action": "list", "phase": "flying" }),
            ),
        ),
        case(
            "checkpoint_checkpoint_phase",
            Setup::Nothing,
            req(
                "task_checkpoint",
                json!({ "action": "checkpoint", "task_id": "t1", "phase": "flying" }),
            ),
        ),
        case(
            "checkpoint_update_phase",
            Setup::Nothing,
            req(
                "task_checkpoint",
                json!({ "action": "update", "task_id": "t1", "phase": "flying" }),
            ),
        ),
        case(
            "checkpoint_update_missing",
            Setup::Nothing,
            req(
                "task_checkpoint",
                json!({ "action": "update", "task_id": "task-404", "phase": "done" }),
            ),
        ),
        case(
            "checkpoint_store_broken",
            Setup::Write(checkpoints, BROKEN),
            req("task_checkpoint", json!({ "action": "list" })),
        ),
        // ---- 人がやること（user_tasks と tako-core の TaskStore）----
        case(
            "todo_show_missing",
            Setup::Nothing,
            req("user_task", json!({ "action": "show", "id": "u-404" })),
        ),
        case(
            "todo_update_missing",
            Setup::Nothing,
            req(
                "user_task",
                json!({ "action": "update", "id": "u-404", "title": "x" }),
            ),
        ),
        case(
            "todo_update_clear_title",
            Setup::Nothing,
            req(
                "user_task",
                json!({ "action": "update", "id": "u-404", "title": "  " }),
            ),
        ),
        case(
            "todo_done_missing",
            Setup::Nothing,
            req("user_task", json!({ "action": "done", "id": "u-404" })),
        ),
        case(
            "todo_respond_missing",
            Setup::Nothing,
            req(
                "user_task",
                json!({ "action": "respond", "id": "u-404", "decision": "approve", "via": "cli" }),
            ),
        ),
        case(
            "todo_add_empty_title",
            Setup::Nothing,
            req("user_task", json!({ "action": "add", "title": "   " })),
        ),
        case(
            "todo_list_status",
            Setup::Nothing,
            req("user_task", json!({ "action": "list", "status": "bogus" })),
        ),
        case(
            "todo_add_kind",
            Setup::Nothing,
            req(
                "user_task",
                json!({ "action": "add", "title": "t", "kind": "bogus" }),
            ),
        ),
        case(
            "todo_respond_decision",
            Setup::Nothing,
            req(
                "user_task",
                json!({ "action": "respond", "id": "u-1", "decision": "bogus" }),
            ),
        ),
        case(
            "todo_respond_via",
            Setup::Nothing,
            req(
                "user_task",
                json!({ "action": "respond", "id": "u-1", "decision": "approve", "via": "bogus" }),
            ),
        ),
        case(
            "todo_store_broken",
            Setup::Write(user_tasks, BROKEN),
            req("user_task", json!({ "action": "show", "id": "u-1" })),
        ),
        case(
            "todo_store_dir",
            Setup::ParentIsFile(user_tasks),
            req("user_task", json!({ "action": "add", "title": "t" })),
        ),
        // ---- 全部の置き場が共有する書き出し（config_io の排他・世代バックアップ）----
        case(
            "gate_lock_is_dir",
            Setup::LockIsDir(gates),
            req(
                "task_gate",
                json!({
                    "action": "set",
                    "task_id": "t1",
                    "criteria_json": r#"[{"id":"a","kind":{"type":"custom","description":"d"}}]"#,
                }),
            ),
        ),
        case(
            "checkpoint_backup_is_dir",
            Setup::BackupIsDir(checkpoints, 1),
            req(
                "task_checkpoint",
                json!({ "action": "checkpoint", "task_id": "t1", "phase": "running" }),
            ),
        ),
        case(
            "todo_backup_shift",
            Setup::BackupIsDir(user_tasks, 2),
            req("user_task", json!({ "action": "add", "title": "t" })),
        ),
        case(
            "run_config_lock_is_dir",
            Setup::LockIsDir(run_configs),
            Value::Null,
        ),
    ];
    if cfg!(unix) {
        out.extend([
            case(
                "gate_store_unreadable",
                Setup::Unreadable(gates),
                req("task_gate", json!({ "action": "show", "task_id": "t1" })),
            ),
            case(
                "checkpoint_store_unreadable",
                Setup::Unreadable(checkpoints),
                req("task_checkpoint", json!({ "action": "list" })),
            ),
            case(
                "todo_store_unreadable",
                Setup::Unreadable(user_tasks),
                req("user_task", json!({ "action": "show", "id": "u-1" })),
            ),
            case(
                "todo_tmp_unwritable",
                Setup::ReadOnlyParent(user_tasks),
                req("user_task", json!({ "action": "add", "title": "t" })),
            ),
        ]);
    }
    out
}

/// 仕込む（親フォルダは作っておく。`ParentIsFile` は親の位置にファイルを置く）
fn prepare(stores: &Stores, setup: &Setup) {
    stores.reset();
    let write = |path: &Path, body: &str| {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("置き場の親を作れる");
        }
        std::fs::write(path, body).expect("置き場を書ける");
    };
    match setup {
        Setup::Nothing => {}
        Setup::Write(which, body) => write(which(stores), body),
        Setup::Unreadable(which) => {
            write(which(stores), "version: 1\n");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(which(stores), std::fs::Permissions::from_mode(0o000))
                    .expect("読めなくできる");
            }
        }
        Setup::ParentIsFile(which) => {
            let parent = which(stores).parent().expect("親がある");
            let _ = std::fs::remove_dir_all(parent);
            std::fs::write(parent, "not a folder").expect("親の位置にファイルを置ける");
        }
        Setup::LockIsDir(which) => {
            std::fs::create_dir_all(sibling(which(stores), ".lock"))
                .expect("ロックの位置にフォルダ");
        }
        Setup::BackupIsDir(which, generation) => {
            write(which(stores), "version: 1\n");
            if *generation > 1 {
                std::fs::write(sibling(which(stores), ".bak.1"), "version: 1\n").expect("bak.1");
            }
            std::fs::create_dir_all(sibling(which(stores), &format!(".bak.{generation}")))
                .expect("世代バックアップの位置にフォルダ");
        }
        Setup::ReadOnlyParent(which) => {
            let path = which(stores);
            let parent = path.parent().expect("親がある");
            std::fs::create_dir_all(parent).expect("置き場の親を作れる");
            std::fs::write(sibling(path, ".lock"), "").expect("ロックファイル");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o555))
                    .expect("書き込めなくできる");
            }
        }
    }
}

/// 置き場の隣のファイル（`<名前><suffix>`。`config_io` の `.lock` / `.bak.<n>` と同じ置き方）
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// 実行設定を書く要求（`tako run-config <file> --args x` と同じ形。ファイルはここで作る）
fn run_config_request(stores: &Stores) -> Value {
    if let Some(dir) = stores.script.parent() {
        std::fs::create_dir_all(dir).expect("スクリプトの置き場");
    }
    std::fs::write(&stores.script, "print('hi')\n").expect("スクリプト");
    json!({
        "method": "runner_defaults",
        "params": { "path": stores.script.display().to_string(), "args": "x" },
    })
}

/// 一時ファイルの名前（`<名前>.tmp.<pid>.<連番>`）の数字を伏せる（連番は書き込みの回数で変わる）
fn mask_tmp(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(".tmp.") {
        out.push_str(&rest[..at + ".tmp.".len()]);
        rest = &rest[at + ".tmp.".len()..];
        let digits = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        // 末尾の `.` は数字の区切りではない（`… (os error 13).` のような文の終わり）
        let digits = rest[..digits].trim_end_matches('.').len();
        out.push('N');
        rest = &rest[digits..];
    }
    out.push_str(rest);
    out
}

struct Seen {
    name: &'static str,
    text: String,
}

fn observe(stores: &Stores) -> Vec<Seen> {
    let mut host = Host {
        workspace: Workspace::new("1", Pane::new(PaneOrigin::User)),
    };
    let seen = cases()
        .into_iter()
        .map(|case| {
            prepare(stores, &case.setup);
            let wire = if case.wire.is_null() {
                run_config_request(stores)
            } else {
                case.wire.clone()
            };
            let request: Request = serde_json::from_value(wire.clone())
                .unwrap_or_else(|e| panic!("{}: 要求を組めない: {e} / {wire}", case.name));
            let text = match tako_control::dispatch(&mut host, request, PaneOrigin::Cli) {
                Err(e) => e.to_string(),
                Ok(v) => format!("<断られずに通った: {v}>"),
            };
            Seen {
                name: case.name,
                text: mask_tmp(&text),
            }
        })
        .collect();
    stores.reset();
    seen
}

/// #2365 より前の日本語の文（修正前の木で [`observe`] が返した文そのもの。前置き込み）。
/// OS の文（`os error N`）は実行環境の std の文を埋める
fn old_ja(name: &str, stores: &Stores) -> String {
    let os = |code: i32| std::io::Error::from_raw_os_error(code).to_string();
    // フォルダをファイルとして開く・複製先にする・上書きの rename 先にする（unix は EISDIR = 21・
    // Windows は ERROR_ACCESS_DENIED = 5）
    let is_dir = || os(if cfg!(windows) { 5 } else { 21 });
    match name {
        "gate_criteria_not_json" => {
            "criteria の JSON パースに失敗: expected ident at line 1 column 2".into()
        }
        "gate_criteria_empty" => "criteria を 1 つ以上指定する".into(),
        "gate_criterion_no_id" => "criterion に id が必要".into(),
        "gate_criterion_kind" => "criterion kind のパースに失敗 (id=a): unknown variant `bogus`, \
                                  expected one of `command`, `pr_merged`, `custom`"
            .into(),
        "gate_criterion_status" => "不明な status: weird".into(),
        "gate_show_missing" => "ゲートが見つからない: nope".into(),
        "gate_results_not_json" => {
            "results の JSON パースに失敗: expected value at line 1 column 1".into()
        }
        "gate_result_no_id" => "result に id が必要".into(),
        "gate_record_missing" => "ゲートが見つからない: nope".into(),
        "gate_store_broken" => {
            "acceptance_gates.yaml のパースに失敗: YAML の構文が壊れている（3 行 3 列）".into()
        }
        "checkpoint_list_phase" | "checkpoint_checkpoint_phase" | "checkpoint_update_phase" => {
            "不明な phase: flying".into()
        }
        "checkpoint_update_missing" => "チェックポイントが見つからない: task-404".into(),
        "checkpoint_store_broken" => {
            "task_checkpoints.yaml のパースに失敗: YAML の構文が壊れている（3 行 3 列）".into()
        }
        "todo_show_missing"
        | "todo_update_missing"
        | "todo_done_missing"
        | "todo_respond_missing" => "タスクが見つからない: u-404".into(),
        "todo_update_clear_title" => "タイトルを空にはできない".into(),
        "todo_add_empty_title" => "タイトルが空（一覧で選べないタスクは作れない）".into(),
        "todo_list_status" => {
            "無効なパラメータ: 不明な status: bogus（open / done / dismissed のいずれか）".into()
        }
        "todo_add_kind" => "無効なパラメータ: 不明な kind: bogus\
                            （review / confirm / permission / post / other のいずれか）"
            .into(),
        "todo_respond_decision" => "無効なパラメータ: 不明な decision: bogus\
                                    （approve / reject / needs_change / answered のいずれか）"
            .into(),
        "todo_respond_via" => {
            "無効なパラメータ: 不明な via: bogus（pc / pwa / cli / mcp のいずれか）".into()
        }
        "todo_store_broken" => {
            "user-tasks.yaml のパースに失敗: YAML の構文が壊れている（3 行 3 列）".into()
        }
        // 親の位置にファイルがある = 既に在る（unix は EEXIST = 17・Windows は ERROR_ALREADY_EXISTS = 183）
        "todo_store_dir" => format!(
            "user-tasks.yaml の置き場を作れない: {}",
            os(if cfg!(windows) { 183 } else { 17 })
        ),
        "gate_store_unreadable" => format!("acceptance_gates.yaml の読み取りに失敗: {}", os(13)),
        "checkpoint_store_unreadable" => {
            format!("task_checkpoints.yaml の読み取りに失敗: {}", os(13))
        }
        "todo_store_unreadable" => format!("user-tasks.yaml の読み取りに失敗: {}", os(13)),
        // 全部の置き場が共有する書き出し（config_io の排他・世代バックアップ・一時ファイル）
        "gate_lock_is_dir" => format!(
            "ロックファイルを開けない ({}): {}",
            sibling(&stores.gates, ".lock").display(),
            is_dir()
        ),
        "checkpoint_backup_is_dir" => format!(
            "バックアップの作成に失敗 ({}): {}",
            stores.checkpoints.display(),
            is_dir()
        ),
        "todo_backup_shift" => format!(
            "バックアップの繰り下げに失敗 ({}): {}",
            sibling(&stores.user_tasks, ".bak.1").display(),
            is_dir()
        ),
        "run_config_lock_is_dir" => format!(
            "ロックファイルを開けない ({}): {}",
            sibling(&stores.run_configs, ".lock").display(),
            is_dir()
        ),
        "todo_tmp_unwritable" => format!(
            "一時ファイルの書き込みに失敗 ({}N): {}",
            sibling(&stores.user_tasks, ".tmp.").display(),
            os(13)
        ),
        other => panic!("旧文の表に無いケース: {other}"),
    }
}

#[test]
fn 置き場の断りは表示言語に従う() {
    let stores = Stores::resolve();
    let original = i18n::lang();
    let mut by_lang = Vec::new();
    for lang in [Lang::Ja, Lang::En] {
        i18n::set_lang(lang);
        by_lang.push((lang, observe(&stores)));
    }
    i18n::set_lang(original);
    if let Some(dir) = stores.script.parent() {
        let _ = std::fs::remove_dir_all(dir);
    }

    let mut failures = Vec::new();
    let (mut en_japanese, mut ja_mismatch) = (0, 0);
    for (lang, seen) in &by_lang {
        for s in seen {
            println!("[{lang:?}] {}: {}", s.name, s.text);
            match lang {
                Lang::Ja => {
                    let old = old_ja(s.name, &stores);
                    if s.text != old {
                        ja_mismatch += 1;
                        failures.push(format!(
                            "[Ja] {}: #2365 より前の文と違う\n  want: {old}\n  got:  {}",
                            s.name, s.text
                        ));
                    }
                }
                Lang::En => {
                    if let Some(c) = has_japanese(&s.text) {
                        en_japanese += 1;
                        failures.push(format!(
                            "[En] {}: 英語表示の断りに日本語（{c}）が混ざる: {}",
                            s.name, s.text
                        ));
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
    println!(
        "{} ケース・英語表示で日本語入り {en_japanese} 件・日本語表示の不一致 {ja_mismatch} 件",
        ja.len()
    );
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
