//! 実経路: tmux のセッション操作（kill / resize / select-window / open）・コミットの詳細（git show）・
//! エディタへの打鍵（edit type）・言語サーバの初期化オプション・worker の解決とレジストリの読み取り・
//! worker 配置の設定（orchestrator layout の config.yaml）・エージェント一覧（remote agents）の断りが
//! 表示言語に従う（Issue #2365 の続き）。
//!
//! 修正前（実測。CLI / MCP と同じ口 = `prepare_offload` → background の本体、無ければ dispatch を、
//! GUI の代わりのホストへ通して）: 英語表示でも「tmux セッション … が見つからない（…）」
//! 「window 9 の kill に失敗した: その window が …に無い」「コミット HEAD..HEAD の情報を解析できない」
//! 「command=type には打つ文字（text）が要る」「worker 'nosuch' がレジストリに見つからない」のように
//! 日本語のまま = dispatch が tako-core の `tmux.rs` / `git.rs` / `editor_keys.rs` / `init_options.rs` と
//! tako-control の `registry.rs` / `setup.rs` / `agents.rs` の `Err(String)`（日本語の固定文）を素通ししていた。
//!
//! 置き場（HOME / data / orchestrator / workers.yaml / tmux の TMUX_TMPDIR とソケット）はすべて一時の場所へ
//! 向ける。tmux は本物を一時の TMUX_TMPDIR で起こし（本番の `tmux -L tako` と既定のサーバーには届かない）、
//! 失敗の形（理由なし・パス入り・応答なし）は本物の前に置いた偽物がソケット名で選ぶ。
//! claude は PATH の先頭の偽物（すぐ非ゼロで終わる）とログインシェルの偽物で、本物を起こさない。
//!
//! - 英語表示: 断りに日本語（かな・漢字）が 1 字も無い。外部の文（git 自身の stderr）は日英とも同じ文
//! - 日本語表示: #2365 より前の文とバイト一致
//! - 表示言語を切り替えた直後の 1 件目から、切り替えた言語で返る
//!
//! 表示言語と env はプロセス全体の状態なので、**1 本のテスト関数を `run_alone!` の子**で走らせる
//! （#2400）。このファイルは修正前の木でもコンパイルできる API だけを使う（A/B: 修正前は英語表示の
//! 検査で落ちる）
//!
//! unix だけで走らせる: 読めないファイル（パーミッション 000）・書けない置き場（0500）と、
//! 偽の tmux / claude / シェル（sh のスクリプト）が unix の前提
#![cfg(unix)]

use std::path::{Path, PathBuf};

use serde_json::json;
use tako_control::dispatch::{OffloadJob, OffloadOutcome};
use tako_control::protocol::Request;
use tako_control::{
    PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost, UiStateHost, WebViewHost,
    WorkspaceHost,
};
use tako_core::i18n::{self, Lang};
use tako_core::{Pane, PaneId, PaneOrigin, SpawnOptions, TerminalSession, Workspace};

/// GUI の代わり: ペインの木と、器のセッション名（select-window とクリックの器の経路が引く）だけを持つ
struct Host {
    workspace: Workspace,
    backend: Option<String>,
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

impl TmuxHost for Host {
    fn backend_session(&self, _pane: PaneId) -> Option<String> {
        self.backend.clone()
    }
}

impl UiStateHost for Host {}
impl PreviewHost for Host {}
impl WebViewHost for Host {}
impl RemoteHost for Host {}
impl SystemHost for Host {}

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

/// 器のソケット名（`TAKO_TMUX_SOCKET`。TMUX_TMPDIR が一時の場所なのでプロセスごとに分けなくてよい）
const SOCK: &str = "tk";
/// 器に置くセッション
const SESSION: &str = "s2365";
/// どこにも無いセッション
const NOSUCH: &str = "nosuch-2365";

/// 偽の tmux: `-L` の名前の末尾で失敗の形を選び、それ以外は本物へ渡す（`-V` も本物が答える）
const FAKE_TMUX: &str = r#"#!/bin/sh
sock=""
prev=""
for a in "$@"; do
  [ "$prev" = "-L" ] && sock=$a
  prev=$a
done
case "$sock" in
  *-silent) exit 1 ;;
  *-path) echo "lost server /private/tmp/tmux-0/x oops" >&2; exit 1 ;;
  *-hang) exec sleep 8 ;;
esac
exec "@REAL@" "$@"
"#;

/// 偽の claude とログインシェル（すぐ非ゼロで終わる = エージェントを列挙できない）
const FAILING: &str = "#!/bin/sh\nexit 1\n";

/// 置き場の種類（config.yaml / workers.yaml）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Place {
    /// 何も置かない
    None,
    /// YAML として壊れている
    Broken,
    /// 読めない（パーミッション 000）
    Unreadable,
    /// config.yaml のロックの位置にディレクトリ
    LockIsDir,
    /// config.yaml の置き場に書けない（0500。ロックファイルは先に置く）
    Readonly,
    /// workers.yaml に `10` と `11` の 2 件
    TwoWorkers,
}

const CONFIG_PLACES: [Place; 4] = [
    Place::Broken,
    Place::Unreadable,
    Place::LockIsDir,
    Place::Readonly,
];
const WORKER_PLACES: [Place; 4] = [
    Place::None,
    Place::Broken,
    Place::Unreadable,
    Place::TwoWorkers,
];

const BROKEN_YAML: &str = "workers: [\n";
const VALID_CONFIG: &str = "spawn_layout:\n  policy: master-reserved\n";
const TWO_WORKERS: &str = "workers:\n  \"10\":\n    pane: 5\n  \"11\":\n    pane: 6\nnext_id: 12\n";

fn config_dir(root: &Path, place: Place) -> PathBuf {
    root.join("cfg").join(format!("{place:?}").to_lowercase())
}

fn workers_file(root: &Path, place: Place) -> PathBuf {
    root.join("wk")
        .join(format!("{place:?}").to_lowercase())
        .join("workers.yaml")
}

fn make_places(root: &Path) {
    use std::os::unix::fs::PermissionsExt;
    for place in CONFIG_PLACES {
        let dir = config_dir(root, place);
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config.yaml");
        match place {
            Place::Broken => std::fs::write(&config, BROKEN_YAML).unwrap(),
            Place::Unreadable => {
                std::fs::write(&config, VALID_CONFIG).unwrap();
                std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o000)).unwrap();
            }
            Place::LockIsDir => {
                std::fs::write(&config, VALID_CONFIG).unwrap();
                std::fs::create_dir_all(dir.join("config.yaml.lock")).unwrap();
            }
            Place::Readonly => {
                std::fs::write(&config, VALID_CONFIG).unwrap();
                std::fs::write(dir.join("config.yaml.lock"), "").unwrap();
                std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
            }
            _ => unreachable!(),
        }
    }
    for place in WORKER_PLACES {
        let file = workers_file(root, place);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        match place {
            Place::None => {}
            Place::Broken => std::fs::write(&file, BROKEN_YAML).unwrap(),
            Place::Unreadable => {
                std::fs::write(&file, TWO_WORKERS).unwrap();
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
            }
            Place::TwoWorkers => std::fs::write(&file, TWO_WORKERS).unwrap(),
            _ => unreachable!(),
        }
    }
}

/// 書き換えた権限を戻す（残骸を消せるように）
fn restore_places(root: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(
        config_dir(root, Place::Readonly),
        std::fs::Permissions::from_mode(0o700),
    );
    let _ = std::fs::set_permissions(
        config_dir(root, Place::Unreadable).join("config.yaml"),
        std::fs::Permissions::from_mode(0o600),
    );
    let _ = std::fs::set_permissions(
        workers_file(root, Place::Unreadable),
        std::fs::Permissions::from_mode(0o600),
    );
}

/// 要求の通し方
enum Via {
    /// CLI / MCP と同じ口（`prepare_offload` → background、無ければ dispatch）
    Request(Box<Request>),
    /// コミットの詳細の background の本体（cwd = 一時のリポジトリ。GUI はペインの cwd を渡す）
    GitShow(&'static str),
}

struct Case {
    name: &'static str,
    via: Via,
    /// 器のセッションを持つペインか（select-window・クリック）
    backend: bool,
    /// config.yaml の置き場（orchestrator layout）
    config: Option<Place>,
    /// workers.yaml の置き場（worker の解決）
    workers: Option<Place>,
    old_ja: Old,
}

enum Old {
    /// #2365 より前の日本語（`<…>` の印は [`expand_old`] が展開する）
    Exact(&'static str),
    /// 前置きだけを見る（後ろに OS / YAML の文が続く）
    Prefix(&'static str),
    /// 外部の文（git 自身の stderr）: 日英とも同じ文で、日本語を含まない
    External,
}

/// CLI / MCP と同じ口で通す要求
fn req(request: Request) -> Via {
    Via::Request(Box::new(request))
}

fn kill(socket: Option<&str>, session: &str, window: Option<u32>) -> Request {
    Request::TmuxKill {
        socket: socket.map(str::to_string),
        session: session.into(),
        window,
    }
}

fn resize(socket: &str, reset: bool) -> Request {
    Request::TmuxResize {
        socket: Some(socket.into()),
        session: SESSION.into(),
        window: 9,
        cols: (!reset).then_some(80),
        rows: (!reset).then_some(24),
        reset,
    }
}

fn layout(policy: Option<&str>) -> Request {
    Request::OrchestratorLayout {
        policy: policy.map(str::to_string),
        master_ratio: None,
        algorithm: None,
        min_worker_cols: None,
        auto_shrink_font: None,
        min_worker_font_scale: None,
    }
}

fn plain(name: &'static str, via: Via, old_ja: Old) -> Case {
    Case {
        name,
        via,
        backend: false,
        config: None,
        workers: None,
        old_ja,
    }
}

fn cases(tmux: bool) -> Vec<Case> {
    use Old::{Exact, External, Prefix};
    let mut out = Vec::new();
    if tmux {
        let sock = |suffix: &str| format!("{SOCK}-{suffix}");
        out.extend([
            plain(
                "tmux kill（どこにも無いセッション）",
                req(kill(None, NOSUCH, None)),
                Exact(
                    "tmux セッション nosuch-2365 が見つからない（既定サーバーと tako バックエンド\
                     （socket: tk）のどちらにも無い）。対象は `tako tmux list` で確認すること",
                ),
            ),
            plain(
                "tmux kill（器に無いセッション）",
                req(kill(Some(SOCK), NOSUCH, None)),
                Exact(
                    "セッション nosuch-2365 の kill に失敗した: tmux セッション nosuch-2365 が \
                     socket: tk に無い。対象は `tako tmux list` で確認すること",
                ),
            ),
            plain(
                "tmux kill（無い window）",
                req(kill(Some(SOCK), SESSION, Some(9))),
                Exact(
                    "window 9 の kill に失敗した: その window が tmux セッション s2365\
                     （socket: tk）に無い。対象は `tako tmux list` で確認すること",
                ),
            ),
            plain(
                "tmux resize（無い window）",
                req(resize(SOCK, false)),
                Exact(
                    "window 9 のリサイズに失敗した: その window が tmux セッション s2365\
                     （socket: tk）に無い。対象は `tako tmux list` で確認すること",
                ),
            ),
            plain(
                "tmux resize --reset（無い window）",
                req(resize(SOCK, true)),
                Exact(
                    "window 9 のサイズ解除に失敗した: その window が tmux セッション s2365\
                     （socket: tk）に無い。対象は `tako tmux list` で確認すること",
                ),
            ),
            plain(
                "tmux kill（サーバーが動いていない）",
                req(kill(Some(&sock("dead")), SESSION, None)),
                Exact(
                    "セッション s2365 の kill に失敗した: tmux サーバー（socket: tk-dead）が\
                     動いていない。対象は `tako tmux list` で確認すること",
                ),
            ),
            plain(
                "tmux kill（tmux が理由を返さない）",
                req(kill(Some(&sock("silent")), SESSION, None)),
                Exact(
                    "セッション s2365 の kill に失敗した: tmux が理由を返さずに失敗した\
                     （socket: tk-silent）。対象は `tako tmux list` で確認すること",
                ),
            ),
            plain(
                "tmux kill（tmux の文にパス）",
                req(kill(Some(&sock("path")), SESSION, None)),
                Exact(
                    "セッション s2365 の kill に失敗した: tmux が失敗した（socket: tk-path）: \
                     lost server <path> oops。対象は `tako tmux list` で確認すること",
                ),
            ),
            plain(
                "tmux kill（tmux が応答しない）",
                req(kill(Some(&sock("hang")), SESSION, None)),
                Exact(
                    "セッション s2365 の kill に失敗した: tmux が失敗した（socket: tk-hang）: \
                     tmux が 5 秒応答しないので打ち切った。対象は `tako tmux list` で確認すること",
                ),
            ),
            Case {
                name: "tmux select-window（無い window）",
                via: req(Request::TmuxSelectWindow {
                    pane: None,
                    window: 9,
                }),
                backend: true,
                config: None,
                workers: None,
                old_ja: Exact(
                    "window 9 の選択に失敗した: その window が tmux セッション s2365\
                     （socket: tk）に無い。対象は `tako tmux list` で確認すること",
                ),
            },
            plain(
                "tmux open（どこにも無いセッション）",
                req(Request::TmuxOpen {
                    socket: None,
                    session: NOSUCH.into(),
                    window: None,
                    pane: None,
                    direction: None,
                }),
                Exact(
                    "tmux セッション nosuch-2365 が見つからない（既定サーバーと tako バックエンド\
                     （socket: tk）のどちらにも無い）。対象は `tako tmux list` で確認すること",
                ),
            ),
        ]);
    }
    out.extend([
        // ---- コミットの詳細 ----
        plain(
            "git show（コミットが無い範囲）",
            Via::GitShow("HEAD..HEAD"),
            Exact("コミット HEAD..HEAD の情報を解析できない"),
        ),
        plain(
            "git show（無いリビジョン = git 自身の stderr）",
            Via::GitShow("nosuch2365"),
            External,
        ),
        // ---- エディタへの打鍵 ----
        plain(
            "edit type（打つ文字が空）",
            req(Request::PreviewEditCommand {
                pane: None,
                command: "type".into(),
                text: Some(String::new()),
                expected_version: None,
            }),
            Exact("command=type には打つ文字（text）が要る"),
        ),
        plain(
            "edit type（上限を超える）",
            req(Request::PreviewEditCommand {
                pane: None,
                command: "type".into(),
                text: Some("x".repeat(1001)),
                expected_version: None,
            }),
            Exact(
                "command=type は 1 回 1000 文字まで（1001 文字）。\
                 長い本文は tako edit replace-range（MCP は tako_preview_edit_range）で入れる",
            ),
        ),
        // ---- 言語サーバの初期化オプション ----
        plain(
            "lsp options（キーの形が不正）",
            req(Request::LspServer {
                action: "options".into(),
                name: Some("rust-analyzer".into()),
                set: Some(
                    json!({ "bad key": 1 })
                        .as_object()
                        .cloned()
                        .expect("オブジェクト"),
                ),
            }),
            Exact(
                "無効なパラメータ: キーの形が不正: \"bad key\"（ドット区切りの経路。例: cargo.targetDir）",
            ),
        ),
        // ---- worker の解決とレジストリ ----
        Case {
            name: "worker-status（レジストリに無い worker）",
            via: req(Request::OrchestratorWorkerStatus {
                pane_id: None,
                session_id: None,
                tmux_session: None,
                worker: Some("nosuch".into()),
            }),
            backend: false,
            config: None,
            workers: Some(Place::None),
            old_ja: Exact(
                "無効なパラメータ: worker 'nosuch' がレジストリに見つからない（tako orchestrator workers で確認）",
            ),
        },
        Case {
            name: "report（worker の候補が 2 件）",
            via: req(Request::OrchestratorReport {
                pane_id: None,
                lines: None,
                messages: None,
                worker: Some("1".into()),
            }),
            backend: false,
            config: None,
            workers: Some(Place::TwoWorkers),
            old_ja: Exact("無効なパラメータ: worker '1' の候補が 2 件ある（完全な ID を指定）: 10, 11"),
        },
        Case {
            name: "worker-status（workers.yaml が壊れている）",
            via: req(Request::OrchestratorWorkerStatus {
                pane_id: None,
                session_id: None,
                tmux_session: None,
                worker: Some("10".into()),
            }),
            backend: false,
            config: None,
            workers: Some(Place::Broken),
            old_ja: Prefix("worker レジストリを読めない: workers.yaml のパースに失敗: "),
        },
        Case {
            name: "worker-status（workers.yaml を読めない）",
            via: req(Request::OrchestratorWorkerStatus {
                pane_id: None,
                session_id: None,
                tmux_session: None,
                worker: Some("10".into()),
            }),
            backend: false,
            config: None,
            workers: Some(Place::Unreadable),
            old_ja: Exact(
                "worker レジストリを読めない: workers.yaml の読み取りに失敗: Permission denied (os error 13)",
            ),
        },
        Case {
            name: "workers（workers.yaml が壊れている）",
            via: req(Request::OrchestratorWorkers { all: Some(true) }),
            backend: false,
            config: None,
            workers: Some(Place::Broken),
            old_ja: Prefix("worker レジストリを読めない: workers.yaml のパースに失敗: "),
        },
        // ---- worker 配置の設定（config.yaml） ----
        Case {
            name: "layout --policy（config.yaml が壊れている）",
            via: req(layout(Some("legacy"))),
            backend: false,
            config: Some(Place::Broken),
            workers: None,
            old_ja: Prefix("config.yaml "),
        },
        Case {
            name: "layout（読むだけ・config.yaml が壊れている）",
            via: req(layout(None)),
            backend: false,
            config: Some(Place::Broken),
            workers: None,
            old_ja: Prefix("config.yaml "),
        },
        Case {
            name: "layout（読むだけ・config.yaml を読めない）",
            via: req(layout(None)),
            backend: false,
            config: Some(Place::Unreadable),
            workers: None,
            old_ja: Prefix("config.yaml "),
        },
        Case {
            name: "layout --policy（ロックの位置にディレクトリ）",
            via: req(layout(Some("legacy"))),
            backend: false,
            config: Some(Place::LockIsDir),
            workers: None,
            old_ja: Exact(
                "ロックファイルを開けない (<cfg>/config.yaml.lock): Is a directory (os error 21)",
            ),
        },
        Case {
            name: "layout --policy（置き場に書けない）",
            via: req(layout(Some("legacy"))),
            backend: false,
            config: Some(Place::Readonly),
            workers: None,
            old_ja: Prefix(""),
        },
        // ---- エージェント一覧 ----
        plain(
            "remote agents（claude agents --json が失敗）",
            req(Request::RemoteAgents),
            Exact("claude agents --json の実行に失敗（claude CLI が見つからないか異常終了）"),
        ),
    ]);
    out
}

/// GUI の代わりのホストへ 1 件通した断りの文（断られなければ `<…>` で返す）
fn run(root: &Path, c: &Case) -> String {
    let base_cfg = root.join("orchestrator");
    std::env::set_var(
        "TAKO_ORCHESTRATOR_DIR",
        c.config.map_or(base_cfg, |p| config_dir(root, p)),
    );
    std::env::set_var(
        "TAKO_WORKERS_FILE",
        workers_file(root, c.workers.unwrap_or(Place::None)),
    );
    let reply = |r: Result<serde_json::Value, tako_control::DispatchError>| match r {
        Err(e) => e.to_string(),
        Ok(v) => format!("<断られなかった: {v}>"),
    };
    match &c.via {
        Via::GitShow(hash) => match (OffloadJob::GitShow {
            cwd: root.join("repo"),
            hash: hash.to_string(),
            file: None,
        })
        .run_staged()
        {
            OffloadOutcome::Reply(r) => reply(r),
            OffloadOutcome::OnUi(_) => "<UI スレッドの続きへ進んだ>".to_string(),
        },
        Via::Request(request) => {
            let pane = Pane::new(PaneOrigin::User);
            let request = with_pane((**request).clone(), pane.id().as_u64());
            let mut host = Host {
                workspace: Workspace::new("1", pane),
                backend: c.backend.then(|| SESSION.to_string()),
            };
            // IPC と同じ順: UI スレッドの下ごしらえ → background → （要れば）UI スレッドの続き
            match tako_control::dispatch::prepare_offload(&mut host, &request) {
                Some(Err(e)) => e.to_string(),
                Some(Ok(job)) => match job.run_staged() {
                    OffloadOutcome::Reply(r) => reply(r),
                    OffloadOutcome::OnUi(next) => reply(tako_control::dispatch::finish_offload(
                        &mut host,
                        next,
                        PaneOrigin::Mcp,
                    )),
                },
                None => reply(tako_control::dispatch(&mut host, request, PaneOrigin::Mcp)),
            }
        }
    }
}

/// ペインを取る要求へ GUI の代わりのペインを渡す（CLI は `--pane` か呼び出し元のペインを渡す）
fn with_pane(request: Request, id: u64) -> Request {
    match request {
        Request::TmuxSelectWindow { pane: None, window } => Request::TmuxSelectWindow {
            pane: Some(id),
            window,
        },
        Request::PreviewEditCommand {
            pane: None,
            command,
            text,
            expected_version,
        } => Request::PreviewEditCommand {
            pane: Some(id),
            command,
            text,
            expected_version,
        },
        other => other,
    }
}

fn expand_old(old: &str, root: &Path, c: &Case) -> String {
    let cfg = c.config.map(|p| config_dir(root, p)).unwrap_or_default();
    old.replace("<cfg>", &cfg.display().to_string())
}

fn matches_old(got: &str, c: &Case, root: &Path) -> Result<(), String> {
    match c.old_ja {
        Old::Exact(old) => {
            let want = expand_old(old, root, c);
            if got == want {
                Ok(())
            } else {
                Err(format!("want: {want}\n  got:  {got}"))
            }
        }
        Old::Prefix(old) => {
            if got.starts_with(old) && got.len() > old.len() && !got.starts_with('<') {
                Ok(())
            } else {
                Err(format!("want（前置き）: {old}…\n  got:  {got}"))
            }
        }
        Old::External => {
            if got.starts_with('<') || got.is_empty() {
                Err(format!("外部の文が返らない: {got}"))
            } else {
                Ok(())
            }
        }
    }
}

/// 本物の tmux の場所（偽物が渡す先。無ければ tmux のケースを外す）
fn real_tmux() -> Option<PathBuf> {
    let out = std::process::Command::new("sh")
        .args(["-c", "command -v tmux"])
        .output()
        .ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !path.is_empty()).then(|| PathBuf::from(path))
}

fn write_exec(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// 一時のリポジトリ（コミット 1 つ）
fn make_repo(repo: &Path) {
    std::fs::create_dir_all(repo).unwrap();
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["add", "a.txt"],
        vec![
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    ] {
        let status = std::process::Command::new("git")
            .args(&args)
            .current_dir(repo)
            .status()
            .expect("git を起こせる");
        assert!(status.success(), "git {args:?}");
    }
}

#[test]
fn tmuxとgitとworkerの解決ほかの断りは表示言語に従う() {
    if tako_core::run_alone!(tmuxとgitとworkerの解決ほかの断りは表示言語に従う) {
        return;
    }
    // ソケットのパスの上限（104 バイト）に収まるよう、一時の置き場の名前は短くする
    let root = std::env::temp_dir().join(format!("t2365-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("一時の置き場");
    let root = root.canonicalize().expect("正規化");
    let real = real_tmux();
    // 本番の設定・データ・器・claude を読み書きしない（どの置き場も一時の場所の下）
    std::env::set_var("HOME", root.join("home"));
    std::env::remove_var("CLAUDE_CONFIG_DIR");
    std::env::set_var("TAKO_DATA_DIR", root.join("data"));
    std::env::set_var("TAKO_DISCOVERY_DIR", root.join("discovery"));
    // tmux: 既定のサーバーも `-L` のソケットも一時の TMUX_TMPDIR の下（本番の `tmux -L tako` に届かない）
    std::env::remove_var("TMUX");
    std::env::set_var("TMUX_TMPDIR", &root);
    std::env::set_var("TAKO_TMUX_SOCKET", SOCK);
    // git の文は英語に固定する（外部の文が日英で同じことを見る）
    std::env::set_var("LANG", "C");
    std::env::remove_var("LC_ALL");
    std::env::remove_var("LC_MESSAGES");
    let fakebin = root.join("fakebin");
    std::fs::create_dir_all(&fakebin).unwrap();
    write_exec(&fakebin.join("claude"), FAILING);
    write_exec(&root.join("fake-shell"), FAILING);
    std::env::set_var("SHELL", root.join("fake-shell"));
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs = vec![fakebin.clone()];
    dirs.extend(std::env::split_paths(&path));
    std::env::set_var("PATH", std::env::join_paths(dirs).unwrap());
    if let Some(real) = &real {
        let fake = root.join("fake-tmux");
        write_exec(
            &fake,
            &FAKE_TMUX.replace("@REAL@", &real.display().to_string()),
        );
        std::env::set_var("TAKO_TMUX_BIN", &fake);
        // 器（本物の tmux。設定ファイルは読まない）
        let status = std::process::Command::new(real)
            .env("SHELL", "/bin/sh")
            .args([
                "-L",
                SOCK,
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                SESSION,
            ])
            .arg("sleep 600")
            .status()
            .expect("tmux を起こせる");
        assert!(status.success(), "器のセッションを作れない");
    } else {
        eprintln!("tmux が無いので tmux のケースを外す");
    }
    make_repo(&root.join("repo"));
    make_places(&root);

    let original = i18n::lang();
    let mut failures = Vec::new();
    let (mut ja_count, mut en_count) = (0, 0);
    let mut by_lang: Vec<(Lang, Vec<String>)> = Vec::new();
    for lang in [Lang::En, Lang::Ja] {
        i18n::set_lang(lang);
        let mut seen = Vec::new();
        for c in cases(real.is_some()) {
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
    // 英語表示の文は日本語表示の文と違う（英語の書き忘れで日本語を写していない）。外部の文は同じ
    for ((c, en), ja) in cases(real.is_some())
        .iter()
        .zip(&by_lang[0].1)
        .zip(&by_lang[1].1)
    {
        match c.old_ja {
            Old::External if en != ja => {
                failures.push(format!(
                    "{}: 外部の文に手を加えている\n  en: {en}\n  ja: {ja}",
                    c.name
                ));
            }
            Old::External => {}
            _ if en == ja => failures.push(format!("{}: 日英が同じ文: {en}", c.name)),
            _ => {}
        }
    }

    // 表示言語を切り替えた直後の 1 件目から、切り替えた言語で返る（毎回切り替えて 1 件ずつ叩く）
    let probes = cases(real.is_some());
    let mut names = vec![
        "edit type（打つ文字が空）",
        "worker-status（レジストリに無い worker）",
    ];
    if real.is_some() {
        names.push("tmux kill（無い window）");
    }
    let picks: Vec<usize> = names
        .iter()
        .filter_map(|name| probes.iter().position(|c| c.name == *name))
        .collect();
    for (i, lang) in [Lang::Ja, Lang::En, Lang::Ja, Lang::En, Lang::Ja, Lang::En]
        .into_iter()
        .enumerate()
    {
        i18n::set_lang(lang);
        let c = &probes[picks[i % picks.len()]];
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

    // 断られたあとも config.yaml / workers.yaml を書き換えていない
    let config = config_dir(&root, Place::Broken).join("config.yaml");
    if std::fs::read_to_string(&config).unwrap_or_default() != BROKEN_YAML {
        failures.push(format!("断ったのに書き換えた: {}", config.display()));
    }
    if std::fs::read_to_string(workers_file(&root, Place::TwoWorkers)).unwrap_or_default()
        != TWO_WORKERS
    {
        failures.push("断ったのに workers.yaml を書き換えた".into());
    }

    if let Some(real) = &real {
        let _ = std::process::Command::new(real)
            .args(["-L", SOCK, "kill-server"])
            .status();
    }
    restore_places(&root);
    let _ = std::fs::remove_dir_all(&root);
    eprintln!(
        "issue2365_tmux_git_lang: 英語 {en_count} 件・日本語 {ja_count} 件・失敗 {}",
        failures.len()
    );
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
