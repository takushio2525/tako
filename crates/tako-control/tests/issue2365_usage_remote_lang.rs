//! 実経路: 対応マトリクスの参照（agent-support / platform）・設定の移行・使用量の内訳と、remote 系
//! （会話の末尾 = remote messages・端末の一覧 / 失効 / 降格・スクロールバック・リモートのセットアップ）の
//! 断りが表示言語に従う（Issue #2365 の続き）。
//!
//! 修正前（実測。CLI / MCP と同じ dispatch を GUI の代わりのホストへ通して）: 枠（`invalid parameter:`）は
//! 英語なのに中身が「未知の agent: bogus（…）」「不明な by: "bogus"（…）」「登録されていないデバイス: nope」
//! 「session_id の形式が不正（英数とハイフンのみ）」のように日本語のまま = dispatch が `agent_support` /
//! `platform` / `migrations` / `usage` / `transcript` / `remote` / `remote_auth` / `remote_setup` /
//! `tailscale` の `Err(String)`（日本語の固定文）を素通ししていた。
//!
//! 置き場（HOME / data / orchestrator / remote の state / tmux のソケット）はすべて一時の場所へ向け、
//! tailscale は偽物（`TAKO_TAILSCALE_BIN`）にする。本物の daemon・Tailscale・端末の登録には触れない。
//!
//! - 英語表示: 断りに日本語（かな・漢字）が 1 字も無い（OS の文は英語のまま載る）
//! - 日本語表示: #2365 より前の文とバイト一致
//! - 表示言語を切り替えた直後の 1 件目から、切り替えた言語で返る
//!
//! 表示言語と env はプロセス全体の状態なので、**1 本のテスト関数を `run_alone!` の子**で走らせる
//! （#2400）。このファイルは修正前の木でもコンパイルできる API だけを使う（A/B: 修正前は英語表示の
//! 検査で落ちる）
//!
//! unix だけで走らせる: 読めないファイル（パーミッション 000）・書けない置き場（0500）と、
//! 偽の tailscale（sh のスクリプト）が unix の前提
#![cfg(unix)]

use std::path::{Path, PathBuf};

use serde_json::json;
use tako_control::protocol::Request;
use tako_control::{
    PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost, UiStateHost, WebViewHost,
    WorkspaceHost,
};
use tako_core::i18n::{self, Lang};
use tako_core::{Pane, PaneId, PaneOrigin, SpawnOptions, TerminalSession, Workspace};

/// GUI の代わり: ペインの木だけを持つ（どの要求も断られ、何も変えない）
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

/// remote の state（`TAKO_REMOTE_STATE_DIR`）の種類
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    /// 空（端末の登録が無い）
    Empty,
    /// devices.json が JSON として壊れている
    Broken,
    /// devices.json を読めない（パーミッション 000）
    Unreadable,
    /// 登録済みの端末が 1 台（observe）
    Device,
    /// 登録済みの端末が 1 台・置き場に書けない（0500）
    Readonly,
    /// 置き場の位置に通常ファイル（系統の保存の置き場を作れない）
    IsFile,
}

const STATES: [State; 6] = [
    State::Empty,
    State::Broken,
    State::Unreadable,
    State::Device,
    State::Readonly,
    State::IsFile,
];

const BROKEN_DEVICES: &str = "{\"devices\": [\n";
const DEVICE_ID: &str = "dev-2365";
const DEVICES: &str = r#"{"devices":[{"id":"dev-2365","name":"phone","login":"testuser","node_name":"phone.tailfake.ts.net","role":"observe","created_at":1,"last_seen":1}]}"#;
/// 読めない記録の会話 ID（`$HOME/.claude/projects/p/<id>.jsonl` をパーミッション 000 で置く）
const LOCKED_SESSION: &str = "locked-2365";

fn state_dir(root: &Path, state: State) -> PathBuf {
    root.join("state").join(format!("{state:?}").to_lowercase())
}

/// 置き場を作る
fn make_states(root: &Path) {
    use std::os::unix::fs::PermissionsExt;
    for state in STATES {
        let dir = state_dir(root, state);
        match state {
            State::IsFile => {
                std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
                std::fs::write(&dir, "not a folder\n").unwrap();
                continue;
            }
            _ => std::fs::create_dir_all(&dir).unwrap(),
        }
        let devices = dir.join("devices.json");
        match state {
            State::Broken => std::fs::write(&devices, BROKEN_DEVICES).unwrap(),
            State::Unreadable => {
                std::fs::write(&devices, DEVICES).unwrap();
                std::fs::set_permissions(&devices, std::fs::Permissions::from_mode(0o000)).unwrap();
            }
            State::Device | State::Readonly => std::fs::write(&devices, DEVICES).unwrap(),
            State::Empty | State::IsFile => {}
        }
        if state == State::Readonly {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        }
    }
}

/// 偽の tailscale（`--version` と `status --json` にだけ答える。serve には触れない）
const FAKE_TAILSCALE: &str = r#"#!/bin/sh
while [ $# -gt 0 ]; do
  case "$1" in
    --socket) shift 2 ;;
    --socket=*) shift ;;
    *) break ;;
  esac
done
case "${1:-}" in
  --version|version) echo "1.0.0-fake"; exit 0 ;;
  status)
    printf '{"BackendState":"Running","Version":"1.0.0-fake","CertDomains":["fake-2365.tailfake.ts.net"],"Self":{"DNSName":"fake-2365.tailfake.ts.net."}}\n'
    exit 0 ;;
  *) exit 1 ;;
esac
"#;

struct Case {
    name: &'static str,
    state: State,
    request: Request,
    /// #2365 より前の日本語。`<…>` の印は [`expand_old`] が展開する。`None` は前置きだけを見る
    /// （中身が tako-core の器のエラーで、文の全体を固定しないもの）
    old_ja: Old,
}

enum Old {
    Exact(&'static str),
    Prefix(&'static str),
}

fn devices(action: &str, device_id: Option<&str>, role: Option<&str>) -> Request {
    Request::RemoteDevices {
        action: action.into(),
        device_id: device_id.map(str::to_string),
        role: role.map(str::to_string),
    }
}

fn usage(
    by: Option<&str>,
    window: Option<&str>,
    project: Option<&str>,
    issue: Option<&str>,
    show: Option<bool>,
) -> Request {
    Request::Sessions {
        action: "usage".into(),
        id: None,
        role: None,
        project: project.map(str::to_string),
        limit: None,
        pane: None,
        tab: None,
        direction: None,
        by: by.map(str::to_string),
        window: window.map(str::to_string),
        account: None,
        issue: issue.map(str::to_string),
        show,
    }
}

fn messages(session_id: &str) -> Request {
    Request::RemoteMessages {
        session_id: session_id.into(),
        tail: Some(5),
    }
}

fn scrollback(pane_id: &str) -> Request {
    Request::RemoteScrollback {
        pane_id: pane_id.into(),
        lines: Some(10),
    }
}

fn setup(answers: serde_json::Value) -> Request {
    Request::RemoteSetup {
        action: "run".into(),
        answers: Some(answers),
    }
}

fn cases() -> Vec<Case> {
    use Old::{Exact, Prefix};
    vec![
        // ---- 対応マトリクスの参照 ----
        Case {
            name: "agent-support（未知の agent）",
            state: State::Empty,
            request: Request::AgentSupport {
                agent: Some("bogus".into()),
                status: None,
            },
            old_ja: Exact("無効なパラメータ: 未知の agent: bogus（<agents>）"),
        },
        Case {
            name: "agent-support（未知の状態）",
            state: State::Empty,
            request: Request::AgentSupport {
                agent: None,
                status: Some("bogus".into()),
            },
            old_ja: Exact(
                "無効なパラメータ: 未知の状態: bogus（supported / degraded / pending / unsupported）",
            ),
        },
        Case {
            name: "platform（未知のプラットフォーム）",
            state: State::Empty,
            request: Request::Platform {
                platform: Some("bogus".into()),
                status: None,
                known_limitations: false,
            },
            old_ja: Exact("無効なパラメータ: 未知のプラットフォーム: bogus（macos / windows）"),
        },
        Case {
            name: "platform（未知の状態）",
            state: State::Empty,
            request: Request::Platform {
                platform: None,
                status: Some("bogus".into()),
                known_limitations: false,
            },
            old_ja: Exact(
                "無効なパラメータ: 未知の状態: bogus（supported / degraded / pending / unsupported）",
            ),
        },
        // ---- 設定の移行 ----
        Case {
            name: "migrate（不明な action）",
            state: State::Empty,
            request: Request::Migrate {
                action: Some("bogus".into()),
                schema: None,
            },
            old_ja: Exact("無効なパラメータ: 不明な action: bogus（status | run）"),
        },
        Case {
            name: "migrate（不明なファイル種別）",
            state: State::Empty,
            request: Request::Migrate {
                action: None,
                schema: Some("bogus".into()),
            },
            old_ja: Exact("無効なパラメータ: 不明なファイル種別: bogus（<schemas>）"),
        },
        // ---- 使用量の内訳 ----
        Case {
            name: "usage（不明な by）",
            state: State::Empty,
            request: usage(Some("bogus"), None, None, None, None),
            old_ja: Exact("不明な by: \"bogus\"（<by> のいずれか）"),
        },
        Case {
            name: "usage（不明な window）",
            state: State::Empty,
            request: usage(None, Some("1y"), None, None, None),
            old_ja: Exact("不明な window: \"1y\"（<windows> のいずれか）"),
        },
        Case {
            name: "usage（読めない project）",
            state: State::Empty,
            request: usage(None, None, Some("project:"), None, None),
            old_ja: Exact(
                "読めない project: \"project:\"（<キー> / project:<キー> / profile:<名前> / none）",
            ),
        },
        Case {
            name: "usage（読めない issue）",
            state: State::Empty,
            request: usage(None, None, None, Some("abc"), None),
            old_ja: Exact("読めない issue: \"abc\"（番号 / #番号 / none）"),
        },
        Case {
            name: "usage --show（不明な by = 画面を動かす前の検査）",
            state: State::Empty,
            request: usage(Some("bogus"), None, None, None, Some(true)),
            old_ja: Exact("無効なパラメータ: 不明な by: \"bogus\"（<by> のいずれか）"),
        },
        // ---- 会話の末尾（remote messages） ----
        Case {
            name: "remote messages（ID の形が不正）",
            state: State::Empty,
            request: messages("bad/id"),
            old_ja: Exact("session_id の形式が不正（英数とハイフンのみ）"),
        },
        Case {
            name: "remote messages（記録が無い）",
            state: State::Empty,
            request: messages("nosuch-2365"),
            old_ja: Exact("session nosuch-2365 の transcript が見つからない"),
        },
        Case {
            name: "remote messages（記録を読めない）",
            state: State::Empty,
            request: messages(LOCKED_SESSION),
            old_ja: Exact("transcript を開けない: Permission denied (os error 13)"),
        },
        // ---- 端末の一覧・失効・降格（daemon 停止中 = devices.json を直に読む） ----
        Case {
            name: "remote devices list（devices.json が壊れている）",
            state: State::Broken,
            request: devices("list", None, None),
            old_ja: Exact("devices.json の解釈に失敗（<state>/devices.json）: JSON が途中で切れている（2 行 0 列）"),
        },
        Case {
            name: "remote devices list（devices.json を読めない）",
            state: State::Unreadable,
            request: devices("list", None, None),
            old_ja: Exact("devices.json の読み取りに失敗: Permission denied (os error 13)"),
        },
        Case {
            name: "remote devices revoke（登録されていない）",
            state: State::Empty,
            request: devices("revoke", Some("nope"), None),
            old_ja: Exact("登録されていないデバイス: nope"),
        },
        Case {
            name: "remote devices role（不正な role）",
            state: State::Device,
            request: devices("role", Some(DEVICE_ID), Some("boss")),
            old_ja: Exact("不正な role: boss（observe / interact / manage / admin）"),
        },
        Case {
            name: "remote devices role（登録されていない）",
            state: State::Empty,
            request: devices("role", Some("nope"), Some("observe")),
            old_ja: Exact("登録されていないデバイス: nope"),
        },
        Case {
            name: "remote devices role（昇格 = 画面からだけ）",
            state: State::Device,
            request: devices("role", Some(DEVICE_ID), Some("admin")),
            old_ja: Exact(
                "権限を上げる操作は tako の画面（設定 → リモート、または承認ダイアログ）からのみ\
                 行えます。CLI / MCP / スマホからは降格と削除だけができます",
            ),
        },
        Case {
            name: "remote devices revoke（置き場に書けない）",
            state: State::Readonly,
            request: devices("revoke", Some(DEVICE_ID), None),
            old_ja: Exact("devices.json の書き込みに失敗: Permission denied (os error 13)"),
        },
        // ---- スクロールバック ----
        Case {
            name: "remote scrollback（対象指定が空）",
            state: State::Empty,
            request: scrollback("  "),
            old_ja: Exact("無効なパラメータ: ペイン ID / セッション名が空"),
        },
        Case {
            name: "remote scrollback（セッション名を取り出せない）",
            state: State::Empty,
            request: scrollback(":0.1"),
            old_ja: Exact("無効なパラメータ: 対象指定 ':0.1' からセッション名を取り出せない"),
        },
        Case {
            name: "remote scrollback（器にそのセッションが無い）",
            state: State::Empty,
            request: scrollback("nosuch-2365"),
            old_ja: Prefix("セッション nosuch-2365 の履歴を読めない: "),
        },
        // ---- リモートのセットアップ（偽の tailscale） ----
        Case {
            name: "remote setup（answers の形が違う）",
            state: State::Empty,
            request: setup(json!({ "yes": "x" })),
            old_ja: Exact(
                "無効なパラメータ: remote setup answers が不正: invalid type: string \"x\", expected a boolean",
            ),
        },
        Case {
            name: "remote setup（系統の指定が不正）",
            state: State::Empty,
            request: setup(json!({ "tailscale": "bogus" })),
            old_ja: Exact(
                "Tailscale 系統の指定が不正です: bogus（auto | gui | standalone）。\
                 standalone を選ぶには tailscaled の LocalAPI socket が必要です",
            ),
        },
        Case {
            name: "remote setup（系統を保存する置き場を作れない）",
            state: State::IsFile,
            request: setup(json!({ "tailscale": "gui" })),
            old_ja: Exact("state ディレクトリの作成に失敗: File exists (os error 17)"),
        },
    ]
}

/// GUI の代わりのホストへ 1 件通した断りの文（断られなければ `<…>` で返す）
fn run(root: &Path, c: &Case) -> String {
    std::env::set_var("TAKO_REMOTE_STATE_DIR", state_dir(root, c.state));
    let mut host = Host {
        workspace: Workspace::new("1", Pane::new(PaneOrigin::User)),
    };
    match tako_control::dispatch(&mut host, c.request.clone(), PaneOrigin::Mcp) {
        Err(e) => e.to_string(),
        Ok(v) => format!("<断られなかった: {v}>"),
    }
}

/// 旧文の印を、一覧と置き場で展開する
fn expand_old(old: &str, root: &Path, state: State) -> String {
    let agents: Vec<&str> = tako_core::agent_support::Agent::ALL
        .iter()
        .map(|a| a.as_str())
        .collect();
    let schemas: Vec<&str> = tako_core::migration::SchemaId::all()
        .iter()
        .map(|s| s.as_str())
        .collect();
    let by: Vec<&str> = tako_core::usage::report::By::ALL
        .iter()
        .map(|b| b.as_str())
        .collect();
    let windows: Vec<&str> = tako_core::usage::report::Window::ALL
        .iter()
        .map(|w| w.as_str())
        .collect();
    old.replace("<agents>", &agents.join(" / "))
        .replace("<schemas>", &schemas.join(" | "))
        .replace("<by>", &by.join(" / "))
        .replace("<windows>", &windows.join(" / "))
        .replace("<state>", &state_dir(root, state).display().to_string())
}

fn matches_old(got: &str, c: &Case, root: &Path) -> Result<(), String> {
    match c.old_ja {
        Old::Exact(old) => {
            let want = expand_old(old, root, c.state);
            if got == want {
                Ok(())
            } else {
                Err(format!("want: {want}\n  got:  {got}"))
            }
        }
        Old::Prefix(old) => {
            if got.starts_with(old) && got.len() > old.len() {
                Ok(())
            } else {
                Err(format!("want（前置き）: {old}…\n  got:  {got}"))
            }
        }
    }
}

/// 断られたあとも端末の登録と系統の選択を書き換えていない
fn nothing_written(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for state in STATES {
        let dir = state_dir(root, state);
        if state == State::IsFile || state == State::Unreadable {
            continue;
        }
        let devices = dir.join("devices.json");
        let expected = match state {
            State::Broken => Some(BROKEN_DEVICES),
            State::Device | State::Readonly => Some(DEVICES),
            _ => None,
        };
        match expected {
            Some(text) => {
                if std::fs::read_to_string(&devices).unwrap_or_default() != text {
                    out.push(format!("断ったのに書き換えた: {}", devices.display()));
                }
            }
            None if devices.exists() => {
                out.push(format!("断ったのに作った: {}", devices.display()))
            }
            None => {}
        }
        if dir.join("tailscale-variant").exists() && state != State::Empty {
            out.push(format!(
                "断ったのに系統を保存した: {}",
                dir.join("tailscale-variant").display()
            ));
        }
    }
    out
}

#[test]
fn 使用量と対応表と移行とremote系の断りは表示言語に従う() {
    if tako_core::run_alone!(使用量と対応表と移行とremote系の断りは表示言語に従う)
    {
        return;
    }
    let root = std::env::temp_dir().join(format!("tako-2365u-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("一時の置き場");
    let root = root.canonicalize().expect("正規化");
    // 本番の設定・データ・会話記録・器・Tailscale を読み書きしない（どの置き場も一時の場所の下）
    let home = root.join("home");
    std::env::set_var("HOME", &home);
    std::env::remove_var("CLAUDE_CONFIG_DIR");
    std::env::set_var("TAKO_DATA_DIR", root.join("data"));
    std::env::set_var("TAKO_DISCOVERY_DIR", root.join("discovery"));
    std::env::set_var("TAKO_ORCHESTRATOR_DIR", root.join("orchestrator"));
    // 器のソケット名（tmux の -L。長いパスを渡すと「File name too long」になる）
    std::env::set_var(
        "TAKO_TMUX_SOCKET",
        format!("tako-2365u-{}", std::process::id()),
    );
    let fake = root.join("fake-tailscale");
    std::fs::write(&fake, FAKE_TAILSCALE).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        // 読めない会話の記録
        let projects = home.join(".claude/projects/p");
        std::fs::create_dir_all(&projects).unwrap();
        let locked = projects.join(format!("{LOCKED_SESSION}.jsonl"));
        std::fs::write(&locked, "{}\n").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    }
    std::env::set_var("TAKO_TAILSCALE_BIN", &fake);
    make_states(&root);

    let original = i18n::lang();
    let mut failures = Vec::new();
    let (mut ja_count, mut en_count) = (0, 0);
    let mut by_lang: Vec<(Lang, Vec<String>)> = Vec::new();
    for lang in [Lang::En, Lang::Ja] {
        i18n::set_lang(lang);
        let mut seen = Vec::new();
        for c in cases() {
            let got = run(&root, &c);
            println!("[{lang:?}] {}: {got}", c.name);
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
                    if let Err(e) = matches_old(&got, &c, &root) {
                        failures.push(format!("[ja] {}: #2365 より前と違う\n  {e}", c.name));
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
        // 使用量（dispatch の検証）と端末の失効（remote_auth）を交互に
        let c = &probes[if i % 2 == 0 { 6 } else { 16 }];
        let got = run(&root, c);
        let ok = match lang {
            Lang::Ja => matches_old(&got, c, &root).is_ok(),
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

    failures.extend(nothing_written(&root));
    // 読めない・書けないものを戻してから消す（残骸を残さない）
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            state_dir(&root, State::Readonly),
            std::fs::Permissions::from_mode(0o700),
        );
        let _ = std::fs::set_permissions(
            state_dir(&root, State::Unreadable).join("devices.json"),
            std::fs::Permissions::from_mode(0o600),
        );
        let _ = std::fs::set_permissions(
            home.join(format!(".claude/projects/p/{LOCKED_SESSION}.jsonl")),
            std::fs::Permissions::from_mode(0o600),
        );
    }
    let _ = std::fs::remove_dir_all(&root);
    eprintln!(
        "issue2365_usage_remote_lang: 英語 {en_count} 件・日本語 {ja_count} 件・失敗 {}",
        failures.len()
    );
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
