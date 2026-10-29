//! 実経路: action で操作を選ぶツールの断り（不明な action・不明な値・操作に要る引数の不足）が
//! 表示言語に従う（Issue #2365 の続き）。
//!
//! 修正前（実測。dispatch を GUI 無しのホストで）: 英語表示でも「不明な action: "bogus"（status / set の
//! いずれか）」「show には task_id が必要」のように日本語の固定文のまま返った。MCP のツールは action で
//! 操作を選ぶものが多く（`tako_theme` / `tako_lang` / `tako_task_gate` …）、綴りの打ち間違いと引数の
//! 書き忘れで最初に目にする断りがこれだった。
//!
//! dispatch は CLI / MCP / GUI が共有する 1 実装で、応答の文は `DispatchError` の表示（= IPC の受け口が
//! CLI / MCP へ返す文）そのもの。ここでは GUI の代わりのホストへ IPC と同じ形（`{"method", "params"}`）の
//! 要求を直接渡して断らせ、
//! - 英語表示: 応答に日本語（かな・漢字）が 1 字も無い
//! - 日本語表示: #2365 より前の文とバイト一致
//!
//! をツールごとに見る。どのケースも断られて何も書かない（設定・置き場に触る前に断る）。
//! 表示言語はプロセス全体の状態なので、**専用の統合テストバイナリの 1 本のテスト関数**の中で順に
//! 切り替える。このファイルは修正前の木でもコンパイルできる API だけを使う（A/B: 修正前は英語表示の
//! 検査で落ちる）

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

/// ケース: 名前・要求（IPC と同じ `{"method", "params"}`）
fn cases() -> Vec<(&'static str, Value)> {
    let req = |method: &str, params: Value| json!({ "method": method, "params": params });
    vec![
        // ---- 不明な action（`<key>: "<got>"（… のいずれか）`）----
        ("theme_action", req("theme", json!({ "action": "bogus" }))),
        ("lang_action", req("lang", json!({ "action": "bogus" }))),
        (
            "ui_mode_action",
            req("ui_mode", json!({ "action": "bogus" })),
        ),
        (
            "sleep_guard_action",
            req("sleep_guard", json!({ "action": "bogus" })),
        ),
        ("logs_action", req("logs", json!({ "action": "bogus" }))),
        (
            "task_gate_action",
            req("task_gate", json!({ "action": "bogus" })),
        ),
        (
            "task_checkpoint_action",
            req("task_checkpoint", json!({ "action": "bogus" })),
        ),
        (
            "sessions_action",
            req("sessions", json!({ "action": "bogus" })),
        ),
        (
            "limit_service_action",
            req("limit_service", json!({ "action": "bogus" })),
        ),
        (
            "telemetry_action",
            req("telemetry", json!({ "action": "bogus" })),
        ),
        (
            "welcome_action",
            req("welcome", json!({ "action": "bogus" })),
        ),
        (
            "stale_binary_action",
            req("stale_binary", json!({ "action": "bogus" })),
        ),
        (
            "settings_action",
            req("settings", json!({ "action": "bogus" })),
        ),
        (
            "recent_items_action",
            req("recent_items", json!({ "action": "bogus" })),
        ),
        (
            "agents_sync_rules_action",
            req("agents_sync_rules", json!({ "action": "bogus" })),
        ),
        (
            "setup_deps_action",
            req("setup_deps", json!({ "action": "bogus" })),
        ),
        (
            "setup_bootstrap_action",
            req("setup_bootstrap", json!({ "action": "bogus" })),
        ),
        ("fda_action", req("fda", json!({ "action": "bogus" }))),
        ("update_action", req("update", json!({ "action": "bogus" }))),
        (
            "workspace_search_action",
            req("workspace_search", json!({ "action": "bogus" })),
        ),
        (
            "user_task_action",
            req("user_task", json!({ "action": "bogus" })),
        ),
        (
            "show_command_action",
            req("show_command", json!({ "action": "bogus", "commands": [] })),
        ),
        (
            "remote_folder_action",
            req("remote_folder", json!({ "action": "bogus" })),
        ),
        // ---- 不明な action（括弧に候補だけ / 句点のあとに候補）----
        (
            "remote_devices_action",
            req("remote_devices", json!({ "action": "bogus" })),
        ),
        (
            "remote_setup_action",
            req("remote_setup", json!({ "action": "bogus" })),
        ),
        (
            "remote_shortcuts_action",
            req("remote_shortcuts", json!({ "action": "bogus" })),
        ),
        (
            "config_share_action",
            req("config_share", json!({ "action": "bogus" })),
        ),
        // ---- 不明な値 ----
        (
            "sleep_guard_mode",
            req("sleep_guard", json!({ "action": "set", "mode": "bogus" })),
        ),
        (
            "sleep_guard_power",
            req(
                "sleep_guard",
                json!({ "action": "set", "power_condition": "bogus" }),
            ),
        ),
        (
            "sleep_guard_lid_mode",
            req(
                "sleep_guard",
                json!({ "action": "set", "lid_sleep_mode": "bogus" }),
            ),
        ),
        (
            "sleep_guard_lid_power",
            req(
                "sleep_guard",
                json!({ "action": "set", "lid_power_condition": "bogus" }),
            ),
        ),
        (
            "lang_value",
            req("lang", json!({ "action": "set", "value": "bogus" })),
        ),
        (
            "ui_mode_mode",
            req("ui_mode", json!({ "action": "set", "mode": "bogus" })),
        ),
        (
            "limit_service_service",
            req(
                "limit_service",
                json!({ "action": "set", "service": "bogus" }),
            ),
        ),
        // ---- 操作に要る引数の不足 ----
        (
            "remote_devices_revoke",
            req(
                "remote_devices",
                json!({ "action": "revoke", "device_id": null }),
            ),
        ),
        (
            "remote_devices_role_id",
            req(
                "remote_devices",
                json!({ "action": "role", "device_id": null }),
            ),
        ),
        (
            "remote_devices_role_role",
            req(
                "remote_devices",
                json!({ "action": "role", "device_id": "d1" }),
            ),
        ),
        ("theme_set", req("theme", json!({ "action": "set" }))),
        (
            "theme_set_color_key",
            req("theme", json!({ "action": "set-color" })),
        ),
        (
            "theme_set_color_value",
            req("theme", json!({ "action": "set-color", "key": "accent" })),
        ),
        (
            "theme_reset_color",
            req("theme", json!({ "action": "reset-color" })),
        ),
        (
            "theme_save_preset",
            req("theme", json!({ "action": "save-preset" })),
        ),
        (
            "theme_delete_preset",
            req("theme", json!({ "action": "delete-preset" })),
        ),
        ("lang_set", req("lang", json!({ "action": "set" }))),
        ("ui_mode_set", req("ui_mode", json!({ "action": "set" }))),
        (
            "limit_service_set",
            req("limit_service", json!({ "action": "set" })),
        ),
        (
            "sessions_show",
            req("sessions", json!({ "action": "show" })),
        ),
        (
            "sessions_resume",
            req("sessions", json!({ "action": "resume" })),
        ),
        (
            "task_checkpoint_update_id",
            req("task_checkpoint", json!({ "action": "update" })),
        ),
        (
            "task_checkpoint_update_phase",
            req(
                "task_checkpoint",
                json!({ "action": "update", "task_id": "t1" }),
            ),
        ),
        (
            "task_checkpoint_resume",
            req("task_checkpoint", json!({ "action": "resume" })),
        ),
        (
            "task_gate_set_id",
            req("task_gate", json!({ "action": "set" })),
        ),
        (
            "task_gate_set_criteria",
            req("task_gate", json!({ "action": "set", "task_id": "t1" })),
        ),
        (
            "task_gate_show",
            req("task_gate", json!({ "action": "show" })),
        ),
        (
            "task_gate_record_id",
            req("task_gate", json!({ "action": "record_results" })),
        ),
        (
            "task_gate_record_results",
            req(
                "task_gate",
                json!({ "action": "record_results", "task_id": "t1" }),
            ),
        ),
        (
            "remote_shortcuts_add",
            req("remote_shortcuts", json!({ "action": "add" })),
        ),
        (
            "remote_shortcuts_remove",
            req("remote_shortcuts", json!({ "action": "remove" })),
        ),
        (
            "user_task_show",
            req("user_task", json!({ "action": "show" })),
        ),
        (
            "user_task_add",
            req("user_task", json!({ "action": "add" })),
        ),
        (
            "user_task_respond",
            req("user_task", json!({ "action": "respond", "id": "u-1" })),
        ),
        // ---- tako_theme に残っていた断り（色キー・予約名・名前の形・知らないテーマ）----
        (
            "theme_color_key",
            req(
                "theme",
                json!({ "action": "set-color", "key": "nosuch", "value": "#000000" }),
            ),
        ),
        (
            "theme_reserved_name",
            req("theme", json!({ "action": "save-preset", "name": "dark" })),
        ),
        (
            "theme_name_shape",
            req(
                "theme",
                json!({ "action": "save-preset", "name": "Bad Name" }),
            ),
        ),
        (
            "theme_unknown_target",
            req(
                "theme",
                json!({ "action": "colors", "target": "nope-2365" }),
            ),
        ),
        // ---- 未知の action（言い回しの違う旧形）----
        ("web_action", req("web", json!({ "action": "bogus" }))),
        (
            "context_budget_action",
            req(
                "context_budget",
                json!({ "action": "bogus", "cwd": "/nonexistent-2365", "dry_run": false }),
            ),
        ),
        (
            "handoff_files_action",
            req("orchestrator_handoff_files", json!({ "action": "bogus" })),
        ),
    ]
}

/// 1 つのケースの観測（断りの文）
struct Seen {
    name: &'static str,
    text: String,
}

fn observe() -> Vec<Seen> {
    let mut host = Host {
        workspace: Workspace::new("1", Pane::new(PaneOrigin::User)),
    };
    cases()
        .into_iter()
        .map(|(name, wire)| {
            let request: Request = serde_json::from_value(wire.clone())
                .unwrap_or_else(|e| panic!("{name}: 要求を組めない: {e} / {wire}"));
            let text = match tako_control::dispatch(&mut host, request, PaneOrigin::Cli) {
                Err(e) => e.to_string(),
                Ok(v) => format!("<断られずに通った: {v}>"),
            };
            Seen { name, text }
        })
        .collect()
}

/// #2365 より前の日本語の文（修正前の木で [`observe`] が返した文そのもの。前置き込み）
const OLD_JA: [(&str, &str); 68] = [
    ("theme_action", "無効なパラメータ: 不明な action: \"bogus\"（status / set / toggle / colors / set-color / reset-color / reset-colors / save-preset / delete-preset / set-font のいずれか）"),
    ("lang_action", "無効なパラメータ: 不明な action: \"bogus\"（status / set のいずれか）"),
    ("ui_mode_action", "無効なパラメータ: 不明な action: \"bogus\"（status / set / toggle / release / restore のいずれか）"),
    ("sleep_guard_action", "無効なパラメータ: 不明な action: \"bogus\"（status / set / install-lid-sleep / remove-lid-sleep / open-battery-settings のいずれか）"),
    ("logs_action", "無効なパラメータ: 不明な action: \"bogus\"（list / read / status / set のいずれか）"),
    ("task_gate_action", "無効なパラメータ: 不明な action: \"bogus\"（set / show / record_results のいずれか）"),
    ("task_checkpoint_action", "無効なパラメータ: 不明な action: \"bogus\"（checkpoint / list / update / resume のいずれか）"),
    ("sessions_action", "無効なパラメータ: 不明な action: \"bogus\"（list / show / resume / link / usage のいずれか）"),
    ("limit_service_action", "無効なパラメータ: 不明な action: \"bogus\"（status / set / refresh のいずれか）"),
    ("telemetry_action", "無効なパラメータ: 不明な action: \"bogus\"（status / on / off のいずれか）"),
    ("welcome_action", "無効なパラメータ: 不明な action: \"bogus\"（status / show / hide / dismiss / next / auto のいずれか）"),
    ("stale_binary_action", "無効なパラメータ: 不明な action: \"bogus\"（status / restart / dismiss のいずれか）"),
    ("settings_action", "無効なパラメータ: 不明な action: \"bogus\"（open / status のいずれか）"),
    ("recent_items_action", "無効なパラメータ: 不明な action: \"bogus\"（list / clear のいずれか）"),
    ("agents_sync_rules_action", "無効なパラメータ: 不明な action: \"bogus\"（sync / status のいずれか）"),
    ("setup_deps_action", "無効なパラメータ: 不明な action: \"bogus\"（status / install のいずれか）"),
    ("setup_bootstrap_action", "無効なパラメータ: 不明な action: \"bogus\"（status / status-all / install / path / undo-path / handoff のいずれか）"),
    ("fda_action", "無効なパラメータ: 不明な action: \"bogus\"（status / open のいずれか）"),
    ("update_action", "無効なパラメータ: 不明な action: \"bogus\"（status / check / apply / apply-zip / repair / open / card / card-dismiss / card-show のいずれか）"),
    ("workspace_search_action", "無効なパラメータ: 不明な action: \"bogus\"（search / results / open / clear のいずれか）"),
    ("user_task_action", "無効なパラメータ: 不明な action: \"bogus\"（add / list / show / update / done / dismiss / respond / expand / collapse のいずれか）"),
    ("show_command_action", "無効なパラメータ: 不明な action: \"bogus\"（show / list / copy / run / dismiss のいずれか）"),
    ("remote_folder_action", "無効なパラメータ: 不明な action: \"bogus\"（open / close / list / ls / open-file / ssh-pane / pending / push / auto のいずれか）"),
    ("remote_devices_action", "不明な action: bogus（list / revoke / role）"),
    ("remote_setup_action", "無効なパラメータ: 不明な action: bogus（run / check）"),
    ("remote_shortcuts_action", "無効なパラメータ: 不明な action: bogus（list / add / remove）"),
    ("config_share_action", "不明な action: bogus。status | init | link | unlink | push | pull | list"),
    ("sleep_guard_mode", "無効なパラメータ: 不明な mode: \"bogus\"（off / on / while-agents-running のいずれか）"),
    ("sleep_guard_power", "無効なパラメータ: 不明な power_condition: \"bogus\"（ac-only / always のいずれか）"),
    ("sleep_guard_lid_mode", "無効なパラメータ: 不明な lid_sleep_mode: \"bogus\"（off / while-agents-running のいずれか）"),
    ("sleep_guard_lid_power", "無効なパラメータ: 不明な lid_power_condition: \"bogus\"（ac-only = AC 電源に接続しているときだけ / always = バッテリー駆動でも）"),
    ("lang_value", "無効なパラメータ: 不明な value: \"bogus\"（system / ja / en のいずれか）"),
    ("ui_mode_mode", "無効なパラメータ: 不明な mode: \"bogus\"（terminal / gui のいずれか）"),
    ("limit_service_service", "無効なパラメータ: 不明な service: \"bogus\"（claude / codex / agy のいずれか）"),
    ("remote_devices_revoke", "revoke には device_id が必要"),
    ("remote_devices_role_id", "role には device_id が必要"),
    ("remote_devices_role_role", "role には role が必要（observe / interact / manage / admin）"),
    ("theme_set", "無効なパラメータ: set には mode が必要（dark / light / プリセット名）"),
    ("theme_set_color_key", "無効なパラメータ: set-color には key が必要"),
    ("theme_set_color_value", "無効なパラメータ: set-color には value が必要（#RRGGBB）"),
    ("theme_reset_color", "無効なパラメータ: reset-color には key が必要"),
    ("theme_save_preset", "無効なパラメータ: save-preset には name が必要"),
    ("theme_delete_preset", "無効なパラメータ: delete-preset には name が必要"),
    ("lang_set", "無効なパラメータ: set には value が必要（system / ja / en）"),
    ("ui_mode_set", "無効なパラメータ: set には mode が必要（terminal / gui）"),
    ("limit_service_set", "無効なパラメータ: set には service が必要（claude / codex / agy）"),
    ("sessions_show", "無効なパラメータ: show には id が必要"),
    ("sessions_resume", "無効なパラメータ: resume には id が必要"),
    ("task_checkpoint_update_id", "無効なパラメータ: update には task_id が必要"),
    ("task_checkpoint_update_phase", "無効なパラメータ: update には phase が必要"),
    ("task_checkpoint_resume", "無効なパラメータ: resume には task_id が必要"),
    ("task_gate_set_id", "無効なパラメータ: set には task_id が必要"),
    ("task_gate_set_criteria", "無効なパラメータ: set には criteria_json が必要"),
    ("task_gate_show", "無効なパラメータ: show には task_id が必要"),
    ("task_gate_record_id", "無効なパラメータ: record_results には task_id が必要"),
    ("task_gate_record_results", "無効なパラメータ: record_results には results_json が必要"),
    ("remote_shortcuts_add", "無効なパラメータ: add には path が必要（絶対パス）"),
    ("remote_shortcuts_remove", "無効なパラメータ: remove には path が必要（id でも可）"),
    ("user_task_show", "無効なパラメータ: show には id が必要"),
    ("user_task_add", "無効なパラメータ: add には title が必要"),
    ("user_task_respond", "無効なパラメータ: respond には decision が必要"),
    // ここから下の 7 つは修正前の dispatch の固定文を値で展開したもの（A/B の修正前の実行で一致を確かめた）
    ("theme_color_key", "無効なパラメータ: 未知の色キー: nosuch"),
    ("theme_reserved_name", "無効なパラメータ: dark は予約名"),
    ("theme_name_shape", "無効なパラメータ: 名前は [a-z0-9-]{1,32}"),
    (
        "theme_unknown_target",
        "無効なパラメータ: 未知のテーマ: \"nope-2365\"（target は dark / light / プリセット名。プリセット: なし）",
    ),
    (
        "web_action",
        "無効なパラメータ: 未知の action: bogus（open / list / show / hide / close / navigate / eval / eval_result / read）",
    ),
    ("context_budget_action", "無効なパラメータ: 未知の action: bogus（check / fix）"),
    (
        "handoff_files_action",
        "無効なパラメータ: 未知の action: bogus（list | show | write | migrate）",
    ),
];

#[test]
fn action_で選ぶツールの断りは表示言語に従う() {
    let original = i18n::lang();
    let mut by_lang = Vec::new();
    for lang in [Lang::Ja, Lang::En] {
        i18n::set_lang(lang);
        by_lang.push((lang, observe()));
    }
    i18n::set_lang(original);

    let mut failures = Vec::new();
    for (lang, seen) in &by_lang {
        assert_eq!(seen.len(), OLD_JA.len(), "ケースの数");
        for (s, (name, old)) in seen.iter().zip(OLD_JA) {
            assert_eq!(s.name, name, "ケースの並び");
            println!("[{lang:?}] {}: {}", s.name, s.text);
            match lang {
                Lang::Ja if s.text != old => failures.push(format!(
                    "[Ja] {}: #2365 より前の文と違う\n  want: {old}\n  got:  {}",
                    s.name, s.text
                )),
                Lang::En => {
                    if let Some(c) = has_japanese(&s.text) {
                        failures.push(format!(
                            "[En] {}: 英語表示の断りに日本語（{c}）が混ざる: {}",
                            s.name, s.text
                        ));
                    }
                }
                _ => {}
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
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
