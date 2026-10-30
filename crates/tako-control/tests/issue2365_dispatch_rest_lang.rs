//! 実経路: dispatch の本体に残っていた断り（ジャンプ履歴・LSP・ワークスペース横断検索・変更履歴・
//! リモートフォルダ・config share・scrollback・コマンドカード・Code Runner・tmux・動画・
//! ファイルツリー・supervisor・ledger・セッション再起動）が表示言語に従う（Issue #2365 の続き）。
//!
//! 修正前（実測。dispatch を GUI 無しのホストで）: 英語表示でも「search には query（検索語）が要る」
//! 「ファイルツリーが閉じている（`tako panel --filetree on` で開く）」「これより前の履歴は無い」の
//! ように日本語の固定文のまま返った。
//!
//! dispatch は CLI / MCP / GUI が共有する 1 実装で、応答の文は `DispatchError` の表示（= IPC の受け口が
//! CLI / MCP へ返す文）そのもの。ここでは GUI の代わりのホストへ IPC と同じ形（`{"method", "params"}`）の
//! 要求を、IPC の受け口と同じ 3 段（`prepare_offload` → `run_staged` → `finish_offload`）で通して断らせ、
//!
//! - 英語表示: 応答に日本語（かな・漢字）が 1 字も無い
//! - 日本語表示: #2365 より前の文とバイト一致
//!
//! をケースごとに見る。どのケースも断られて何も書かない（読むのは一時の `TAKO_DATA_DIR` の下だけ）。
//! 表示言語はプロセス全体の状態なので、**専用の統合テストバイナリの 1 本のテスト関数**の中で順に
//! 切り替える。このファイルは修正前の木でもコンパイルできる API だけを使う（A/B: 修正前は英語表示の
//! 検査で落ちる）
//!
//! unix だけで走らせる: 読めないファイル（パーミッション 000）と `/` を範囲にした検索・存在しない
//! 絶対パスの形が unix の前提。文の日英の対は OS に依らない 1 か所（`Reason::text_in`）で、
//! 両 OS の番犬（`issue1762_dispatch_error_lang.rs`）が表で見る
#![cfg(unix)]

use std::path::PathBuf;

use serde_json::{json, Value};
use tako_control::dispatch::{finish_offload, prepare_offload, OffloadOutcome};
use tako_control::protocol::{PreviewModeWire, Request};
use tako_control::{
    ChangelogDiffTarget, DispatchError, PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost,
    UiStateHost, WebViewHost, WorkspaceHost,
};
use tako_core::i18n::{self, Lang};
use tako_core::jump_history::JumpHistory;
use tako_core::lsp::xref::XrefStore;
use tako_core::workspace_search::SearchStore;
use tako_core::{CommandCards, Pane, PaneId, PaneOrigin, SpawnOptions, TerminalSession, Workspace};

/// GUI の代わり: ペインの木と、ケースが要る置き場（ジャンプ履歴・プレビュー・変更履歴の行・
/// 検索の置き場・参照の置き場・カード・ファイルツリーの表示）だけを持つ
#[derive(Default)]
struct Setup {
    /// プレビューの状態（全ペインに同じものを返す）
    preview: Option<PreviewModeWire>,
    /// 変更履歴の 1 行（`repo_root` / `rel_path` の有無で断りを分ける）
    changelog: Option<(Option<PathBuf>, Option<String>)>,
    search: bool,
    xref: bool,
    cards: bool,
    filetree: bool,
}

struct Host {
    workspace: Workspace,
    jump: JumpHistory,
    setup: Setup,
    search: SearchStore,
    xref: XrefStore,
    cards: CommandCards,
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

impl UiStateHost for Host {
    fn workspace_search(&self) -> Option<&SearchStore> {
        self.setup.search.then_some(&self.search)
    }
    fn workspace_search_mut(&mut self) -> Option<&mut SearchStore> {
        if self.setup.search {
            Some(&mut self.search)
        } else {
            None
        }
    }
    fn filetree_visible(&self) -> bool {
        self.setup.filetree
    }
    fn command_cards(&self) -> Option<&CommandCards> {
        self.setup.cards.then_some(&self.cards)
    }
    fn command_cards_mut(&mut self) -> Option<&mut CommandCards> {
        if self.setup.cards {
            Some(&mut self.cards)
        } else {
            None
        }
    }
}

impl PreviewHost for Host {
    fn preview_state(&self, _pane: PaneId) -> Option<(String, PreviewModeWire)> {
        self.setup
            .preview
            .map(|mode| ("/tmp/x2365/notes.md".to_string(), mode))
    }
    fn jump_history(&self) -> Option<&JumpHistory> {
        Some(&self.jump)
    }
    fn jump_history_mut(&mut self) -> Option<&mut JumpHistory> {
        Some(&mut self.jump)
    }
    fn lsp_xref(&self) -> Option<&XrefStore> {
        self.setup.xref.then_some(&self.xref)
    }
    fn lsp_xref_mut(&mut self) -> Option<&mut XrefStore> {
        if self.setup.xref {
            Some(&mut self.xref)
        } else {
            None
        }
    }
    fn changelog_diff_target(
        &self,
        _pane: PaneId,
        hash: &str,
    ) -> Result<ChangelogDiffTarget, String> {
        let (repo_root, rel_path) = self.setup.changelog.clone().expect("変更履歴のケース");
        Ok(ChangelogDiffTarget {
            hash: hash.to_string(),
            expanded: false,
            repo_root,
            rel_path,
        })
    }
}

impl TmuxHost for Host {}
impl WebViewHost for Host {}
impl RemoteHost for Host {}
impl SystemHost for Host {}

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

/// IPC の受け口と同じ 3 段で要求を通す（background へ出ない要求は同期の dispatch）
fn staged(host: &mut Host, request: Request) -> Result<Value, DispatchError> {
    match prepare_offload(host, &request) {
        None => tako_control::dispatch(host, request, PaneOrigin::Cli),
        Some(Err(e)) => Err(e),
        Some(Ok(job)) => match job.run_staged() {
            OffloadOutcome::Reply(result) => result,
            OffloadOutcome::OnUi(next) => finish_offload(host, next, PaneOrigin::Cli),
        },
    }
}

/// 1 つのケース: 名前・ホストの用意・要求（`pane` はホストのペイン ID）。`reason` = 断りではなく
/// 応答の `reason` を見る（動かなかったジャンプ）
struct Case {
    name: &'static str,
    setup: Setup,
    wire: Box<dyn Fn(u64) -> Value>,
    reason: bool,
}

fn case(name: &'static str, setup: Setup, wire: impl Fn(u64) -> Value + 'static) -> Case {
    Case {
        name,
        setup,
        wire: Box::new(wire),
        reason: false,
    }
}

fn req(method: &str, params: Value) -> Value {
    json!({ "method": method, "params": params })
}

/// 読めないファイル（Code Runner の先頭の読み取りが断る）。パーミッション 000
fn unreadable_file(dir: &std::path::Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("locked2365.py");
    std::fs::write(&path, "print(1)\n").expect("読めないファイルを作る");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).expect("chmod 000");
    path
}

fn cases(locked: PathBuf) -> Vec<Case> {
    let d = Setup::default;
    let locked = locked.display().to_string();
    vec![
        // ---- ジャンプ履歴（空の履歴 = 動かなかった応答の reason） ----
        Case {
            reason: true,
            ..case("jump_back_empty", d(), |p| {
                req("jump", json!({ "action": "back", "pane": p }))
            })
        },
        Case {
            reason: true,
            ..case("jump_forward_empty", d(), |p| {
                req("jump", json!({ "action": "forward", "pane": p }))
            })
        },
        // ---- LSP ----
        case("goto_choice_zero", d(), |p| {
            req(
                "lsp_goto",
                json!({ "action": "definition", "pane": p, "line": 1, "column": 0, "choice": 0 }),
            )
        }),
        case("completion_choice_zero", d(), |p| {
            req(
                "lsp_completion",
                json!({ "pane": p, "line": 1, "column": 0, "choice": 0 }),
            )
        }),
        case("completion_limit_zero", d(), |p| {
            req(
                "lsp_completion",
                json!({ "pane": p, "line": 1, "column": 0, "limit": 0 }),
            )
        }),
        case("results_expand_no_item", Setup { xref: true, ..d() }, |p| {
            req("lsp_results", json!({ "action": "expand", "pane": p }))
        }),
        case("lsp_server_set", d(), |_| {
            req(
                "lsp_server",
                json!({ "action": "status", "set": { "check": true } }),
            )
        }),
        case("lsp_server_name", d(), |_| {
            req(
                "lsp_server",
                json!({ "action": "status", "name": "nope2365" }),
            )
        }),
        // ---- ワークスペース横断検索 ----
        case("search_no_query", d(), |p| {
            req("workspace_search", json!({ "action": "search", "pane": p }))
        }),
        case("search_relative_path", d(), |p| {
            req(
                "workspace_search",
                json!({ "action": "search", "query": "x", "path": "rel2365", "pane": p }),
            )
        }),
        // 範囲を省くとタブのツリーのルート（ペインの cwd・ピン留め）から決める。このホストは cwd も
        // ピン留めも持たない = 暗黙の範囲が空（明示の `path` は探すので使わない）
        case("search_no_roots", d(), |p| {
            req(
                "workspace_search",
                json!({ "action": "search", "query": "x", "pane": p }),
            )
        }),
        case("search_open_no_item", d(), |p| {
            req("workspace_search", json!({ "action": "open", "pane": p }))
        }),
        case("search_open_unavailable", d(), |p| {
            req(
                "workspace_search",
                json!({ "action": "open", "item": 1, "pane": p }),
            )
        }),
        case(
            "search_open_not_shown",
            Setup {
                search: true,
                ..d()
            },
            |p| {
                req(
                    "workspace_search",
                    json!({ "action": "open", "item": 1, "pane": p }),
                )
            },
        ),
        // ---- 変更履歴の 1 行の展開 ----
        case(
            "changelog_no_repo",
            Setup {
                preview: Some(PreviewModeWire::Markdown),
                changelog: Some((None, None)),
                ..d()
            },
            |p| {
                req(
                    "preview_changelog",
                    json!({ "pane": p, "expand": "abc2365" }),
                )
            },
        ),
        case(
            "changelog_no_path",
            Setup {
                preview: Some(PreviewModeWire::Markdown),
                changelog: Some((Some(PathBuf::from("/tmp/x2365")), None)),
                ..d()
            },
            |p| {
                req(
                    "preview_changelog",
                    json!({ "pane": p, "expand": "abc2365" }),
                )
            },
        ),
        // ---- リモートフォルダ・config share・scrollback ----
        case("remote_open_no_host", d(), |_| {
            req("remote_folder", json!({ "action": "open" }))
        }),
        case("remote_open_file_no_path", d(), |_| {
            req(
                "remote_folder",
                json!({ "action": "open-file", "host": "devbox2365" }),
            )
        }),
        // all = 名前を付けずに全部 = 「リモートフォルダ」を当てる腕
        case("remote_close_all_none", d(), |_| {
            req("remote_folder", json!({ "action": "close", "all": true }))
        }),
        case("remote_close_named", d(), |_| {
            req(
                "remote_folder",
                json!({ "action": "close", "host": "devbox2365" }),
            )
        }),
        case("remote_push_none", d(), |_| {
            req("remote_folder", json!({ "action": "push" }))
        }),
        case("config_share_link", d(), |_| {
            req("config_share", json!({ "action": "link" }))
        }),
        case("scrollback_no_vessel", d(), |p| {
            req("remote_scrollback", json!({ "pane_id": p.to_string() }))
        }),
        // ---- コマンドカード・Code Runner ----
        case("card_show_no_store", d(), |p| {
            req(
                "show_command",
                json!({ "action": "show", "commands": ["ls"], "pane": p }),
            )
        }),
        case("card_copy_no_card", Setup { cards: true, ..d() }, |p| {
            req("show_command", json!({ "action": "copy", "pane": p }))
        }),
        case("runner_scope_no_path", d(), |_| {
            req("runner_defaults", json!({ "scope": "file" }))
        }),
        case("run_unreadable", d(), move |p| {
            req("run", json!({ "path": locked, "pane": p }))
        }),
        // ---- tmux・動画 ----
        case("tmux_servers_socket", d(), |_| {
            req(
                "tmux_cleanup",
                json!({ "socket": "x2365", "servers": true }),
            )
        }),
        case(
            "video_not_video",
            Setup {
                preview: Some(PreviewModeWire::Markdown),
                ..d()
            },
            |p| req("video_seek", json!({ "pane": p, "seconds": 1.0 })),
        ),
        // ---- ファイルツリー ----
        case("tree_add_relative", d(), |_| {
            req("tree_folder", json!({ "action": "add", "path": "rel2365" }))
        }),
        case("tree_remove_not_pinned", d(), |_| {
            req(
                "tree_folder",
                json!({ "action": "remove", "path": "/nonexistent2365/a" }),
            )
        }),
        case("tree_bad_action", d(), |_| {
            req("tree_folder", json!({ "action": "bogus" }))
        }),
        case("tree_select_other_tab", d(), |_| {
            req("tree_selection", json!({ "tab": 99 }))
        }),
        case("tree_select_path_and_key", d(), |_| {
            req("tree_selection", json!({ "path": "/a", "key": "down" }))
        }),
        case("tree_select_closed", d(), |_| {
            req("tree_selection", json!({ "path": "/a" }))
        }),
        case("tree_select_bad_key", d(), |_| {
            req("tree_selection", json!({ "key": "bogus" }))
        }),
        case(
            "tree_select_not_visible",
            Setup {
                filetree: true,
                ..d()
            },
            |_| req("tree_selection", json!({ "path": "/nonexistent2365/a" })),
        ),
        case(
            "tree_select_nothing_selected",
            Setup {
                filetree: true,
                ..d()
            },
            |_| req("tree_selection", json!({ "key": "down" })),
        ),
        // ---- supervisor・ledger・セッション再起動 ----
        case("supervisor_bad_mode", d(), |_| {
            req(
                "orchestrator_supervisor",
                json!({ "action": "set_mode", "mode": "bogus" }),
            )
        }),
        case("supervisor_bad_action", d(), |_| {
            req("orchestrator_supervisor", json!({ "action": "bogus" }))
        }),
        case("ledger_prune_no_project", d(), |_| {
            req("orchestrator_ledger", json!({ "action": "prune" }))
        }),
        case("ledger_bad_action", d(), |_| {
            req("orchestrator_ledger", json!({ "action": "bogus" }))
        }),
        case("restart_bad_mode", d(), |p| {
            req("session_restart", json!({ "pane": p, "mode": "bogus" }))
        }),
    ]
}

/// #2365 より前の文（修正前の main の実測と同じ。`<pane>` はホストのペイン ID・`<locked>` は読めない
/// ファイルのパス・`<note>` は器を持たないペインの注記）
const OLD_JA: &[(&str, &str)] = &[
    ("jump_back_empty", "これより前の履歴は無い"),
    ("jump_forward_empty", "これより先の履歴は無い"),
    (
        "goto_choice_zero",
        "無効なパラメータ: choice は 1 始まり（候補の一覧の番号）",
    ),
    (
        "completion_choice_zero",
        "無効なパラメータ: choice は 1 始まり（候補の一覧の番号）",
    ),
    (
        "completion_limit_zero",
        "無効なパラメータ: limit は 1 以上（返す候補の件数）",
    ),
    (
        "results_expand_no_item",
        "無効なパラメータ: item を指定する（応答の id）",
    ),
    (
        "lsp_server_set",
        "無効なパラメータ: set は action=options のときだけ使える",
    ),
    ("lsp_server_name", "<server>"),
    (
        "search_no_query",
        "無効なパラメータ: search には query（検索語）が要る",
    ),
    (
        "search_relative_path",
        "無効なパラメータ: 相対パス rel2365 の基準（ペインの cwd）が分からない。絶対パスで渡す",
    ),
    (
        "search_no_roots",
        "無効なパラメータ: 探す範囲がホーム（またはルート）だけになる。フォルダへ cd するか、\
         ファイルツリーにフォルダを足す（tako tree add <フォルダ>）か、path で範囲を渡す",
    ),
    (
        "search_open_no_item",
        "無効なパラメータ: open には item（結果の id。1 始まり）が要る",
    ),
    (
        "search_open_unavailable",
        "この tako には右パネルの検索結果が無い（GUI のプライマリで使う。出さない検索は使える）",
    ),
    (
        "search_open_not_shown",
        "無効なパラメータ: 右パネルに検索結果が無い（tako search <検索語> --show で出してから開く）",
    ),
    ("changelog_no_repo", "リポジトリ情報がない"),
    ("changelog_no_path", "ファイルパス情報がない"),
    ("remote_open_no_host", "無効なパラメータ: host が必要"),
    ("remote_open_file_no_path", "無効なパラメータ: path が必要"),
    (
        "remote_close_all_none",
        "開いていない: リモートフォルダ（`list` で開いているものを確認できる）",
    ),
    (
        "remote_close_named",
        "開いていない: devbox2365（`list` で開いているものを確認できる）",
    ),
    (
        "remote_push_none",
        "押し出せていない保存はありません（`pending` で確認できます）",
    ),
    (
        "config_share_link",
        "link には対象（リポジトリのパスまたは URL）が必要です",
    ),
    (
        "scrollback_no_vessel",
        "ペイン <pane> は器（永続化バックエンド）を持たないので履歴を読めない: <note>",
    ),
    (
        "card_show_no_store",
        "コマンド提案カードは GUI（tako-app）が必要（この接続先には保管庫が無い）",
    ),
    (
        "card_copy_no_card",
        "このペインに表示中のコマンドカードが無い",
    ),
    (
        "runner_scope_no_path",
        "無効なパラメータ: scope / profile / runtime / reset / args / env / cwd / before / board / \
         port / target は path（対象ファイル）と一緒に渡す",
    ),
    (
        "run_unreadable",
        "ファイルを読めない（<locked>: Permission denied (os error 13)）",
    ),
    (
        "tmux_servers_socket",
        "--servers はソケット置き場全体を対象にするので --socket（x2365）とは併用できない。\
         1 つのサーバーの中を掃除するなら --servers を外す",
    ),
    ("video_not_video", "対象ペインは動画プレビューではない"),
    ("tree_add_relative", "無効なパラメータ: 絶対パスを指定する"),
    (
        "tree_remove_not_pinned",
        "指定フォルダはピン留めされていない: /nonexistent2365/a",
    ),
    (
        "tree_bad_action",
        "無効なパラメータ: action は add / remove / list / roots / git-status のいずれか（受け取った値: bogus）",
    ),
    (
        "tree_select_other_tab",
        "無効なパラメータ: ファイルツリーはアクティブタブ（<tab>）のものだけを出す（受け取った tab: 99）",
    ),
    (
        "tree_select_path_and_key",
        "無効なパラメータ: path（その行を選ぶ）と key（選んでいる行の上で押す）は同時に指定しない",
    ),
    (
        "tree_select_closed",
        "ファイルツリーが閉じている（`tako panel --filetree on` で開く）",
    ),
    (
        "tree_select_bad_key",
        "無効なパラメータ: key は up / down / left / right / enter / extend_up / extend_down / \
         extend_top / extend_bottom のいずれか（受け取った値: bogus）",
    ),
    (
        "tree_select_not_visible",
        "ファイルツリーに見えていない行: /nonexistent2365/a（畳んだフォルダの中・リモートの行は選べない）",
    ),
    (
        "tree_select_nothing_selected",
        "ファイルツリーの行を選んでいない（先に path で選ぶ）",
    ),
    (
        "supervisor_bad_mode",
        "mode は auto / notify_only / off のいずれかを指定",
    ),
    (
        "supervisor_bad_action",
        "supervisor の action は status / set_mode / history のいずれか（不明: 'bogus'）",
    ),
    (
        "ledger_prune_no_project",
        "無効なパラメータ: project（前方一致プレフィックス）は必須",
    ),
    (
        "ledger_bad_action",
        "無効なパラメータ: 不正な action 'bogus'。使用可能: list, stats, record, amend, prune",
    ),
    (
        "restart_bad_mode",
        "無効なパラメータ: mode が不正: \"bogus\"（harness | handoff。省略すると下見だけを返す）",
    ),
];

/// LSP のサーバの ID の並び（`サーバ名が不正` の候補。表は tako-core の 1 か所）
fn known_servers() -> String {
    tako_core::lsp::servers::SERVERS
        .iter()
        .map(|s| s.id)
        .collect::<Vec<_>>()
        .join(" / ")
}

/// 1 つのケースの観測
struct Seen {
    name: &'static str,
    text: String,
}

fn observe(locked: &std::path::Path) -> Vec<Seen> {
    let mut seen = Vec::new();
    for c in cases(locked.to_path_buf()) {
        let mut host = Host {
            workspace: Workspace::new("1", Pane::new(PaneOrigin::User)),
            jump: JumpHistory::default(),
            setup: c.setup,
            search: SearchStore::default(),
            xref: XrefStore::default(),
            cards: CommandCards::default(),
        };
        // ペインは 1 枚だけ
        let pane = host
            .workspace
            .all_pane_ids()
            .into_iter()
            .next()
            .expect("ペイン")
            .as_u64();
        let wire = (c.wire)(pane);
        let request: Request = serde_json::from_value(wire.clone())
            .unwrap_or_else(|e| panic!("{}: 要求を組めない: {e} / {wire}", c.name));
        let text = match staged(&mut host, request) {
            Ok(v) if c.reason => v["reason"]
                .as_str()
                .unwrap_or("<reason が無い>")
                .to_string(),
            Err(e) if !c.reason => e.to_string(),
            Ok(v) => format!("<断られずに通った: {v}>"),
            Err(e) => format!("<断られた: {e}>"),
        };
        seen.push(Seen { name: c.name, text });
    }
    seen
}

/// 旧文の差し込み（ペイン ID・タブ ID・読めないファイル・器の注記・サーバの一覧）
fn expand_old(old: &str, pane: u64, tab: u64, locked: &std::path::Path) -> String {
    if old == "<server>" {
        return format!(
            "無効なパラメータ: サーバ名が不正: nope2365（{}）",
            known_servers()
        );
    }
    old.replace("<pane>", &pane.to_string())
        .replace("<tab>", &tab.to_string())
        .replace("<locked>", &locked.display().to_string())
        .replace("<note>", &tako_control::reach::no_detached_capture_note())
}

#[test]
fn dispatch_の残りの断りは表示言語に従う() {
    // 読む置き場（リモートの保存待ち・ledger など）を一時の場所へ向ける。断る前に書く経路は無い
    let data = std::env::temp_dir().join(format!("tako-2365l-{}", std::process::id()));
    std::fs::create_dir_all(&data).expect("一時の data dir");
    std::env::set_var("TAKO_DATA_DIR", &data);
    let locked = unreadable_file(&data);
    // 応答に出るのは実行の前に境界（B26）で解いた形（macOS の一時置き場は /private を前に付ける）
    let shown_path = std::fs::canonicalize(&locked).expect("読めないファイルの実体");

    let original = i18n::lang();
    let mut by_lang = Vec::new();
    for lang in [Lang::Ja, Lang::En] {
        i18n::set_lang(lang);
        by_lang.push((lang, observe(&locked)));
    }
    i18n::set_lang(original);

    let mut failures = Vec::new();
    for (lang, seen) in &by_lang {
        assert_eq!(seen.len(), OLD_JA.len(), "ケースの数");
        for (s, (name, old)) in seen.iter().zip(OLD_JA) {
            assert_eq!(s.name, *name, "ケースの並び");
            println!("[{lang:?}] {}: {}", s.name, s.text);
            match lang {
                Lang::Ja => {
                    // 旧文の `<pane>` / `<tab>` は観測した文の同じ位置の数字で埋める
                    // （ケースごとにホストを作り直すので採番が進む）
                    let want = expand_with_seen(old, &s.text, &shown_path);
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
    let _ = std::fs::remove_dir_all(&data);
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// 旧文の `<pane>` / `<tab>` を、観測した文の同じ位置の数字で埋める（ケースごとにホストを作り直すと
/// 採番が進むので、固定の値を書けない）。数字以外の差し込みは [`expand_old`] と同じ
fn expand_with_seen(old: &str, seen: &str, locked: &std::path::Path) -> String {
    let number_at = |marker: &str| -> Option<u64> {
        let (before, _) = old.split_once(marker)?;
        let rest = seen.strip_prefix(before)?;
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    };
    let pane = number_at("<pane>").unwrap_or(0);
    let tab = number_at("<tab>").unwrap_or(0);
    expand_old(old, pane, tab, locked)
}

/// 番犬自身の検査: 差し込みは観測した数字で埋まり、前置きが違えば埋めない（= 比較で落ちる）
#[test]
fn 旧文の差し込みの境界() {
    let locked = std::path::Path::new("/tmp/l");
    assert_eq!(
        expand_with_seen("ペイン <pane> は器: x", "ペイン 42 は器: x", locked),
        "ペイン 42 は器: x"
    );
    assert_eq!(
        expand_with_seen("タブ（<tab>）だけ", "タブ（7）だけ", locked),
        "タブ（7）だけ"
    );
    assert_eq!(
        expand_with_seen("ペイン <pane> は器", "pane 42 has no vessel", locked),
        "ペイン 0 は器"
    );
    assert_eq!(
        expand_with_seen("ファイル（<locked>: e）", "", locked),
        "ファイル（/tmp/l: e）"
    );
}
