//! 実経路: ファイルの移動・コピーの断りが、1 つの応答の中で表示言語にそろう（Issue #2365 の続き）。
//!
//! 修正前（実測。dispatch を GUI 無しのホストで）: 英語表示でも「移動できない: 移動先に同じ名前がある
//! （上書きしない）（… → …）」のように包みも理由も日本語のまま返った（`MoveRefusal` / `CopyRefusal` の
//! 理由と dispatch の包みが日本語の固定文だった）。
//!
//! dispatch は CLI（`tako file move|copy|paste`）/ MCP（`tako_file_op`）/ GUI のツリーが共有する
//! 1 実装で、応答の文は `DispatchError` の表示（= IPC の受け口が CLI / MCP へ返す文）そのもの。
//! ここでは GUI の代わりのホストへ直接渡し、一時 dir の実ファイルで断らせて、
//! - 英語表示: 応答に日本語（かな・漢字）が 1 字も無い（パスは ASCII の一時 dir を `<DIR>` へ置換）
//! - 日本語表示: #2365 より前の文とバイト一致
//!
//! を代表のケースで見る。表示言語はプロセス全体の状態なので、**専用の統合テストバイナリの
//! 1 本のテスト関数**の中で順に切り替える（ほかのテストと言語グローバルを取り合わない）。
//! このファイルは修正前の木でもコンパイルできる API だけを使う（A/B: 修正前は英語表示の検査で落ちる）

use serde_json::Value;
use tako_control::protocol::{FileOpKind, Request};
use tako_control::{
    PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost, UiStateHost, WebViewHost,
    WorkspaceHost,
};
use tako_core::file_clipboard::FileClipboard;
use tako_core::i18n::{self, Lang};
use tako_core::test_residue::ScratchDir;
use tako_core::{Pane, PaneId, PaneOrigin, SpawnOptions, TerminalSession, Workspace};

/// GUI の代わり: ペインの木と、ツリーのクリップボード（tako 側の中身）だけを持つ
struct Host {
    workspace: Workspace,
    clip: Option<FileClipboard>,
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

impl PreviewHost for Host {
    fn file_clipboard(&self) -> Option<FileClipboard> {
        self.clip.clone()
    }
    fn set_file_clipboard(&mut self, clip: Option<FileClipboard>) {
        self.clip = clip;
    }
}

impl TmuxHost for Host {}
impl UiStateHost for Host {}
impl WebViewHost for Host {}
impl RemoteHost for Host {}
impl SystemHost for Host {}

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

fn op(op: FileOpKind, path: &str, dest: Option<&str>) -> Request {
    Request::FileOp {
        op,
        path: path.to_string(),
        name: None,
        pane: None,
        dest: dest.map(str::to_string),
    }
}

fn many(op: FileOpKind, paths: &[&str], dest: Option<&str>) -> Request {
    Request::FileOpMany {
        op,
        paths: paths.iter().map(|p| p.to_string()).collect(),
        dest: dest.map(str::to_string),
    }
}

/// 1 つのケースの観測（応答の文。断りは `DispatchError` の表示・成功は応答の JSON の抜き出し）
struct Seen {
    name: &'static str,
    text: String,
}

/// 一時 dir に置き場を作り、代表のケースを順に断らせて応答の文を集める（パスは `<DIR>` へ置換）
fn observe() -> Vec<Seen> {
    let scratch = ScratchDir::new("issue2365-move-copy");
    let dir = tako_core::platform::path::canonicalize(scratch.path()).unwrap();
    let base = dir.display().to_string();
    for sub in ["src", "folder/inner", "dst"] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
    }
    for (file, body) in [
        ("src/a.txt", "a\n"),
        ("src/b.txt", "b\n"),
        ("src/c.txt", "c\n"),
        ("dst/a.txt", "existing\n"),
        ("folder/inner/x.txt", "x\n"),
    ] {
        std::fs::write(dir.join(file), body).unwrap();
    }
    let p = |rel: &str| format!("{base}/{rel}");
    let mut host = Host {
        workspace: Workspace::new("1", Pane::new(PaneOrigin::User)),
        clip: None,
    };
    let mut seen = Vec::new();
    let mut push = |name: &'static str, text: String| {
        seen.push(Seen {
            name,
            text: text.replace(&base, "<DIR>"),
        });
    };
    let err = |host: &mut Host, request: Request| {
        tako_control::dispatch(host, request, PaneOrigin::Cli)
            .expect_err("断るはずの操作が通った")
            .to_string()
    };

    // 移動（`tako file move` / `tako_file_op` の op=move）
    let t = err(
        &mut host,
        op(FileOpKind::Move, &p("src/a.txt"), Some(&p("dst"))),
    );
    push("move_name_taken", t);
    let t = err(
        &mut host,
        op(FileOpKind::Move, &p("folder"), Some(&p("folder/inner"))),
    );
    push("move_into_descendant", t);
    let t = err(&mut host, op(FileOpKind::Move, &p("nope"), Some(&p("dst"))));
    push("move_source_missing", t);
    // コピー（`tako file copy` / op=copy）
    let t = err(
        &mut host,
        op(FileOpKind::Copy, &p("folder"), Some(&p("folder/inner"))),
    );
    push("copy_into_descendant", t);
    let t = err(&mut host, op(FileOpKind::Copy, &p("nope"), Some(&p("dst"))));
    push("copy_source_missing", t);
    let t = err(
        &mut host,
        op(FileOpKind::Copy, &p("src/a.txt"), Some(&p("src/b.txt"))),
    );
    push("copy_dest_not_dir", t);
    // まとめた操作（`paths`）が全部断られた
    let t = err(
        &mut host,
        many(
            FileOpKind::Move,
            &[&p("nope"), &p("folder")],
            Some(&p("folder/inner")),
        ),
    );
    push("many_move_all_failed", t);
    let t = err(
        &mut host,
        many(
            FileOpKind::Copy,
            &[&p("nope"), &p("nope2")],
            Some(&p("dst")),
        ),
    );
    push("many_copy_all_failed", t);
    // 切り取りを貼る = 移動（配下へ）
    tako_control::dispatch(
        &mut host,
        op(FileOpKind::ClipboardCut, &p("folder"), None),
        PaneOrigin::Cli,
    )
    .unwrap();
    let t = err(&mut host, op(FileOpKind::Paste, &p("folder/inner"), None));
    push("paste_cut_into_descendant", t);
    // コピーを貼る: 2 件とも断られる（配下へ・置いた後に消えた）
    tako_control::dispatch(
        &mut host,
        many(
            FileOpKind::ClipboardCopy,
            &[&p("folder"), &p("src/c.txt")],
            None,
        ),
        PaneOrigin::Cli,
    )
    .unwrap();
    std::fs::remove_file(dir.join("src/c.txt")).unwrap();
    let t = err(&mut host, op(FileOpKind::Paste, &p("folder/inner"), None));
    push("paste_copy_all_failed", t);
    host.clip = None;
    let t = err(&mut host, op(FileOpKind::Paste, &p("dst"), None));
    push("paste_nothing", t);
    // 成功の応答に載る文（同じ場所への移動の note・一部だけ断られたときの failed[].reason）
    let unchanged = tako_control::dispatch(
        &mut host,
        op(FileOpKind::Move, &p("src/a.txt"), Some(&p("src"))),
        PaneOrigin::Cli,
    )
    .unwrap();
    push(
        "move_unchanged_note",
        unchanged["note"].as_str().unwrap_or_default().to_string(),
    );
    let partial = tako_control::dispatch(
        &mut host,
        many(
            FileOpKind::Move,
            &[&p("src/b.txt"), &p("nope")],
            Some(&p("dst")),
        ),
        PaneOrigin::Cli,
    )
    .unwrap();
    assert!(
        dir.join("dst/b.txt").is_file(),
        "移せるものは移す: {partial}"
    );
    push(
        "many_move_partial_reason",
        partial["failed"][0]["reason"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    );
    // 応答の JSON 全体（成功の行と断りの行が 1 つの応答に並ぶ）
    push("many_move_partial_json", strip_paths(&partial));
    seen
}

/// JSON の応答からパスの値を外した文（パスは言語に依らない。残りの文だけを見る）
fn strip_paths(v: &Value) -> String {
    let mut v = v.clone();
    if let Some(done) = v["done"].as_array_mut() {
        for d in done {
            for key in ["from", "to"] {
                d[key] = Value::from("<PATH>");
            }
        }
    }
    if let Some(failed) = v["failed"].as_array_mut() {
        for f in failed {
            f["path"] = Value::from("<PATH>");
        }
    }
    v.to_string()
}

/// #2365 より前の日本語の文（`<DIR>` = 一時 dir）
const OLD_JA: [(&str, &str); 14] = [
    (
        "move_name_taken",
        "移動できない: 移動先に同じ名前がある（上書きしない）（<DIR>/src/a.txt → <DIR>/dst）",
    ),
    (
        "move_into_descendant",
        "移動できない: フォルダを自分の配下へは移せない（<DIR>/folder → <DIR>/folder/inner）",
    ),
    (
        "move_source_missing",
        "移動できない: 移す元が見つからない（<DIR>/nope → <DIR>/dst）",
    ),
    (
        "copy_into_descendant",
        "コピーできない: フォルダを自分の配下へはコピーできない（<DIR>/folder → <DIR>/folder/inner）",
    ),
    (
        "copy_source_missing",
        "コピーできない: コピー元が見つからない（<DIR>/nope → <DIR>/dst）",
    ),
    (
        "copy_dest_not_dir",
        "コピーできない: 貼り付け先がフォルダではない（<DIR>/src/a.txt → <DIR>/src/b.txt）",
    ),
    (
        "many_move_all_failed",
        "2 件すべてできなかった: 移動できない: 移す元が見つからない（<DIR>/nope → <DIR>/folder/inner） / \
         移動できない: フォルダを自分の配下へは移せない（<DIR>/folder → <DIR>/folder/inner）",
    ),
    (
        "many_copy_all_failed",
        "2 件すべてできなかった: コピーできない: コピー元が見つからない（<DIR>/nope → <DIR>/dst） / \
         コピーできない: コピー元が見つからない（<DIR>/nope2 → <DIR>/dst）",
    ),
    (
        "paste_cut_into_descendant",
        "移動できない: フォルダを自分の配下へは移せない（<DIR>/folder → <DIR>/folder/inner）",
    ),
    (
        "paste_copy_all_failed",
        "貼り付けできない（2 件すべて）: コピーできない: フォルダを自分の配下へはコピーできない\
         （<DIR>/folder → <DIR>/folder/inner） / コピーできない: コピー元が見つからない\
         （<DIR>/src/c.txt → <DIR>/folder/inner）",
    ),
    ("paste_nothing", "貼り付けるファイルがクリップボードに無い"),
    ("move_unchanged_note", "既にそのフォルダにある（何もしていない）"),
    (
        "many_move_partial_reason",
        "移動できない: 移す元が見つからない（<DIR>/nope → <DIR>/dst）",
    ),
    // JSON 全体は日本語表示では日本語の理由を含む（英語表示の側だけ「日本語が無い」を見る）
    ("many_move_partial_json", ""),
];

#[test]
fn 移動とコピーの断りは1つの応答の中で表示言語にそろう() {
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
                Lang::Ja if !old.is_empty() && s.text != old => failures.push(format!(
                    "[Ja] {}: #2365 より前の文と違う\n  want: {old}\n  got:  {}",
                    s.name, s.text
                )),
                Lang::En => {
                    if let Some(c) = has_japanese(&s.text) {
                        failures.push(format!(
                            "[En] {}: 英語表示の応答に日本語（{c}）が混ざる: {}",
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
