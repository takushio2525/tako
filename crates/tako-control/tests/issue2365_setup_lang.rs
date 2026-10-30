//! 実経路: setup 系（claude / codex への MCP 登録・`tako setup` の answers・アップデート追従の照会・
//! 共通ルール同期・依存の導入・エージェント CLI の導入の系統名と導入状況・モデル一覧）の断りが、枠と
//! 中身ごと表示言語に従う（Issue #2365 の続き）。
//!
//! 修正前（実測。CLI / MCP と同じ dispatch を GUI の代わりのホストへ通して）: 英語表示でも
//! 「未対応のエージェント種別 …」「codex CLI が見つからないので MCP 登録できません …」
//! 「… を JSON として解釈できないので書き換えを中止した …」「selected_agent は …」
//! 「config.yaml のパースに失敗: …」「共通ルールの正本パスが設定されていません …」「不明な依存: …」
//! 「不明なエージェント: …」のように日本語のまま = dispatch が自身の日本語の固定文と、中身の口
//! （`setup` / `agents_sync` / `setup_deps` / `setup_bootstrap` / `agent_mcp`）の `Err(String)` を
//! 素通ししていた。
//!
//! **本物の環境を 1 つも書き換えない**（setup は claude / codex の設定・シェルの rc を書く経路）:
//!
//! - `HOME` / `TAKO_DATA_DIR` / `TAKO_ORCHESTRATOR_DIR` は一時の置き場の下（`~/.claude.json` も一時）
//! - `PATH` は空の dir・`SHELL` は何も答えないシェル（`/usr/bin/false`）= claude / codex / agy / brew を
//!   引けない（最初に引けないことを確かめてから先へ進む。引けたら何も叩かずに止まる）
//! - `tako setup` を子で起こす経路（answers が通ったとき）へは届かせない: どのケースも answers の
//!   検証で断られる
//! - 失敗は注入で起こす: 壊れた `~/.claude.json`・フォルダの `~/.claude.json`・壊れた config.yaml・
//!   無い / 空の正本ファイル・`HOME` を外す
//!
//! 見るもの:
//!
//! - 英語表示: 断りに日本語（かな・漢字）が 1 字も無い
//! - 日本語表示: #2365 より前の文とバイト一致
//!
//! 表示言語と env はプロセス全体の状態なので、**1 本のテスト関数を `run_alone!` の子**で走らせる
//! （#2400）。このファイルは修正前の木でもコンパイルできる API だけを使う（A/B: 修正前は英語表示の
//! 検査で落ちる）。unix だけ: 引けなくする注入（`SHELL` のログインシェル）が unix の探索の経路
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

/// GUI の代わり: ペインの木だけを持つ（どの要求もペインを作らない）
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

/// フォルダへ書き込もうとしたときの OS のエラー文（EISDIR）
const EISDIR: &str = "Is a directory (os error 21)";

struct Case {
    name: &'static str,
    request: Value,
    /// #2365 より前の日本語の断り
    old_ja: String,
}

fn case(name: &'static str, request: Value, old_ja: impl Into<String>) -> Case {
    Case {
        name,
        request,
        old_ja: old_ja.into(),
    }
}

fn setup_run(answers: Value) -> Value {
    json!({ "method": "setup_run", "params": { "answers": answers } })
}

/// `HOME` が一時の置き場（`home`）を指している間のケース
fn cases(root: &Path, json_reason: &str, yaml_reason: &str, deps_old: &str) -> Vec<Case> {
    let claude_json = root.join("home").join(".claude.json").display().to_string();
    vec![
        case(
            "setup-mcp --agent（不明な名前）",
            json!({ "method": "setup_mcp", "params": { "agent": "tako-x" } }),
            "未対応のエージェント種別 'tako-x'（対応: claude / codex / agy）",
        ),
        case(
            "setup-mcp --agent codex（CLI を引けない）",
            json!({ "method": "setup_mcp", "params": { "agent": "codex" } }),
            "codex CLI が見つからないので MCP 登録できません。次の一手: codex を導入して PATH を通してから \
             tako setup-mcp --agent codex をやり直してください（導入の案内: https://developers.openai.com/codex/cli）",
        ),
        case(
            "setup-mcp --agent claude（~/.claude.json が壊れている = 書き換えを中止）",
            json!({ "method": "setup_mcp", "params": { "agent": "claude" } }),
            format!(
                "{claude_json} を JSON として解釈できないので書き換えを中止した（{json_reason}）。\
                 内容を直すか退避してからやり直してください"
            ),
        ),
        case(
            "tako setup answers（形が違う = serde の文を中身に持つ）",
            setup_run(json!({ "selected_agent": 1 })),
            "無効なパラメータ: setup answers が不正: invalid type: integer `1`, expected a string",
        ),
        case(
            "tako setup answers（selected_agent）",
            setup_run(json!({ "selected_agent": "tako-x" })),
            "無効なパラメータ: selected_agent は claude / codex / agy のいずれかです: tako-x",
        ),
        case(
            "tako setup answers（projects の cwd が空）",
            setup_run(json!({ "projects": { "k": { "cwd": " " } } })),
            "無効なパラメータ: projects.k.cwd は空にできません",
        ),
        case(
            "tako setup answers（mod_ui = tako mod ui の断りを包む）",
            setup_run(json!({ "mod_ui": { "buttons": [{ "kind": "slash", "value": "bash" }] } })),
            "無効なパラメータ: mod_ui: slash で送れるのは許可リストのコマンドだけ。使える値: compact / clear / \
             context / cost / model / effort / tako（ui.json は書いていない）",
        ),
        case(
            "tako setup answers（profile の master_agent = orchestrator の断りを包む）",
            setup_run(json!({ "profile": { "master_agent": "agy" } })),
            "無効なパラメータ: プロファイルの master_agent が不正: agy は master 非対応\
             （master は claude / codex のみ。worker としては利用可能）",
        ),
        case(
            "tako setup answers（sleep_guard.power）",
            setup_run(json!({ "sleep_guard": { "power": "battery" } })),
            "無効なパラメータ: sleep_guard.power は ac-only / always のいずれかです: battery",
        ),
        case(
            "setup changes（config.yaml が壊れている）",
            json!({ "method": "setup_changes" }),
            format!("config.yaml のパースに失敗: {yaml_reason}"),
        ),
        case(
            "agents sync-rules status（config.yaml が壊れている）",
            json!({ "method": "agents_sync_rules", "params": { "action": "status" } }),
            format!("config.yaml のパースに失敗: {yaml_reason}"),
        ),
        case(
            "setup deps install --dep（不明な名前）",
            json!({ "method": "setup_deps", "params": { "action": "install", "dep": "tako-x", "dry_run": true } }),
            deps_old,
        ),
        case(
            "setup bootstrap --agent（不明な名前）",
            json!({ "method": "setup_bootstrap", "params": { "action": "status", "agent": "tako-x" } }),
            "無効なパラメータ: 不明なエージェント: \"tako-x\"（claude / codex / agy のいずれか）",
        ),
        case(
            "setup models --agent（不明な名前）",
            json!({ "method": "setup_models", "params": { "agent": "tako-x" } }),
            "無効なパラメータ: 未対応のエージェント種別 'tako-x'（対応: claude / codex / agy）",
        ),
    ]
}

/// config.yaml を外し、`~/.claude.json` をフォルダにしたときのケース
/// （書き込みの OS のエラー = 英語の中身を包む・共通ルール同期の正本）
fn second_cases(root: &Path) -> Vec<Case> {
    let claude_json = root.join("home").join(".claude.json").display().to_string();
    let missing = root.join("no-rules.md").display().to_string();
    let empty = root.join("empty-rules.md").display().to_string();
    vec![
        case(
            "setup-mcp --agent claude（~/.claude.json がフォルダ = 書けない）",
            json!({ "method": "setup_mcp", "params": { "agent": "claude" } }),
            format!("{claude_json} への書き込みに失敗: {EISDIR}"),
        ),
        case(
            "agents sync-rules（正本のパスの設定が無い）",
            json!({ "method": "agents_sync_rules", "params": { "action": "sync" } }),
            "共通ルールの正本パスが設定されていません。\n\
             tako setup で設定するか、--source オプションで指定してください",
        ),
        case(
            "agents sync-rules --source（正本が無い）",
            json!({ "method": "agents_sync_rules", "params": { "action": "sync", "source": missing } }),
            format!(
                "正本ファイルが見つかりません: {missing}\n\
                 ファイルを作成するか、tako setup で正本のパスを設定してください"
            ),
        ),
        case(
            "agents sync-rules --source（正本が空）",
            json!({ "method": "agents_sync_rules", "params": { "action": "sync", "source": empty } }),
            "正本ファイルが空です",
        ),
    ]
}

/// `HOME` を外したときのケース（ホームを決められない = 何も書かない）
fn no_home_cases() -> Vec<Case> {
    vec![
        case(
            "setup bootstrap status（HOME が無い）",
            json!({ "method": "setup_bootstrap", "params": { "action": "status", "agent": "codex" } }),
            "ホームディレクトリを特定できません（HOME が未設定）",
        ),
        case(
            "setup bootstrap handoff（HOME が無い）",
            json!({ "method": "setup_bootstrap", "params": { "action": "handoff", "agent": "codex" } }),
            "ホームディレクトリを特定できません（HOME が未設定）",
        ),
    ]
}

fn host() -> Host {
    Host {
        workspace: Workspace::new("1", Pane::new(PaneOrigin::User)),
    }
}

fn run(request: &Value) -> String {
    let mut host = host();
    let request: Request = serde_json::from_value(request.clone()).expect("要求の形");
    let result: Result<Value, DispatchError> =
        tako_control::dispatch(&mut host, request, PaneOrigin::Cli);
    match result {
        Err(e) => e.to_string(),
        Ok(v) => format!("<断られずに通った: {v}>"),
    }
}

/// 日英の 2 回ぶん走らせる（表示言語を切り替えた直後の 1 件目も数える）
fn run_both(all: &[Case], by_lang: &mut Vec<(Lang, &'static str, String, String)>) {
    for lang in [Lang::Ja, Lang::En] {
        i18n::set_lang(lang);
        for c in all {
            by_lang.push((lang, c.name, run(&c.request), c.old_ja.clone()));
        }
    }
}

#[test]
fn setup系の断りは表示言語に従う() {
    if tako_core::run_alone!(setup系の断りは表示言語に従う) {
        return;
    }
    let root = std::env::temp_dir().join(format!("tako-2365s-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("一時の置き場");
    let root = root.canonicalize().expect("正規化");
    for dir in ["home", "data", "orch", "empty-path"] {
        std::fs::create_dir_all(root.join(dir)).expect("一時の dir");
    }
    // 本番の設定・データを読み書きしない（どの置き場も一時の場所の下）
    std::env::set_var("HOME", root.join("home"));
    std::env::set_var("TAKO_DATA_DIR", root.join("data"));
    std::env::set_var("TAKO_ORCHESTRATOR_DIR", root.join("orch"));
    std::env::remove_var("CLAUDE_CONFIG_DIR");
    std::env::remove_var("CODEX_HOME");
    // claude / codex / agy / brew を引けなくする（上の doc）
    std::env::set_var("PATH", root.join("empty-path"));
    std::env::set_var("SHELL", "/usr/bin/false");
    for name in ["claude", "codex", "agy", "brew"] {
        assert!(
            tako_core::platform::exe::find(name).is_none(),
            "{name} を引けてしまう（注入が効いていない）ので本物の CLI を起こしうる。先へ進まない"
        );
    }

    // 失敗の注入: 壊れた ~/.claude.json・壊れた config.yaml・空の正本
    let broken_json = "{ \"mcpServers\": ";
    std::fs::write(root.join("home").join(".claude.json"), broken_json).expect("壊れた JSON");
    let broken_yaml = "setup: [broken\n";
    std::fs::write(root.join("orch").join("config.yaml"), broken_yaml).expect("壊れた YAML");
    std::fs::write(root.join("empty-rules.md"), "  \n").expect("空の正本");
    let json_reason = tako_core::load_issue::json_error_reason(
        &serde_json::from_str::<Value>(broken_json).expect_err("壊れている"),
    );
    // 位置は読み先の型で決まる（config.yaml の読み手と同じ `SetupConfig` で読む）
    let yaml_reason = tako_core::load_issue::yaml_error_reason(
        &serde_yaml::from_str::<tako_control::setup::SetupConfig>(broken_yaml)
            .expect_err("壊れている"),
        broken_yaml,
    );
    // 依存の一覧はこの環境の対象で変わるので、修正前から日本語を返す従来の口で旧文を引く
    let deps_old = tako_control::setup_deps::install(
        Some("tako-x"),
        tako_control::setup_deps::DepInstallOptions {
            dry_run: true,
            interactive: false,
        },
    )
    .expect_err("不明な依存は断られる");

    let original = i18n::lang();
    let mut seen = Vec::new();
    let all = cases(&root, &json_reason, &yaml_reason, &deps_old);
    run_both(&all, &mut seen);

    // 何も書いていない（壊れた設定はそのまま）
    assert_eq!(
        std::fs::read_to_string(root.join("orch").join("config.yaml")).expect("config.yaml"),
        broken_yaml,
        "壊れた config.yaml を書き換えた"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("home").join(".claude.json")).expect(".claude.json"),
        broken_json,
        "壊れた ~/.claude.json を書き換えた"
    );

    // config.yaml を外し、~/.claude.json をフォルダへ差し替える（書き込みの OS のエラー）
    std::fs::remove_file(root.join("orch").join("config.yaml")).expect("config.yaml を外す");
    std::fs::remove_file(root.join("home").join(".claude.json")).expect("差し替え");
    std::fs::create_dir_all(root.join("home").join(".claude.json")).expect("フォルダ");
    let second = second_cases(&root);
    run_both(&second, &mut seen);

    // HOME を外す
    std::env::remove_var("HOME");
    let no_home = no_home_cases();
    run_both(&no_home, &mut seen);
    i18n::set_lang(original);

    let mut failures = Vec::new();
    let mut en_japanese = 0;
    let mut ja_same = 0;
    for (lang, name, text, old) in &seen {
        println!("[{lang:?}] {name}: {text}");
        match lang {
            Lang::Ja => {
                if text == old {
                    ja_same += 1;
                } else {
                    failures.push(format!(
                        "[Ja] {name}: #2365 より前の文と違う\n  want: {old}\n  got:  {text}"
                    ));
                }
            }
            Lang::En => {
                if let Some(ch) = has_japanese(text) {
                    en_japanese += 1;
                    failures.push(format!(
                        "[En] {name}: 英語表示に日本語（{ch}）が混ざる: {text}"
                    ));
                }
                if text.starts_with("<断られずに通った") {
                    failures.push(format!("[En] {name}: 断られなかった: {text}"));
                }
            }
        }
    }
    // MCP の登録先（フォルダ）へ何も書いていない
    assert!(
        std::fs::read_dir(root.join("home").join(".claude.json"))
            .expect("フォルダ")
            .next()
            .is_none(),
        "~/.claude.json のフォルダへ何か書いた"
    );
    let _ = std::fs::remove_dir_all(&root);
    let total = all.len() + second.len() + no_home.len();
    println!("英語表示で日本語入り: {en_japanese} / {total}（ケース数）");
    println!("日本語表示で #2365 より前とバイト一致: {ja_same} / {total}");
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
