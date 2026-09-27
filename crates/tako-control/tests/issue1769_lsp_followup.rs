//! #1769 の e2e（S1 の続き）: 偽の言語サーバ（`tako-lsp-fake`）を実プロセスで起こして測る。
//!
//! 1. **単独の `\r`**: 偽サーバは受けた本文を**自前の模型**で当てて持ち、位置を自前で数える
//!    （tako の変換を使わない）。`spec`（`\n` / `\r\n` / 単独の `\r`。pyright / TypeScript の実測）と
//!    `lf`（`\n` だけ。rust-analyzer / clangd の問い合わせの実測）の**両方の数え方**で、
//!    診断の位置・定義ジャンプの往復・編集後のサーバの本文が tako と一致することを見る
//! 2. **同じファイルの 2 ペイン目**: 1 URI = 1 文書を持ち手で共有する（didOpen は最初の 1 回・
//!    didClose は最後の 1 つが閉じたとき・どちらの編集も同じ文書の版を進める）
//! 3. **サーバの解決のキャッシュ**: 2 回目以降の `servers` は解決（ログインシェル）を起こさない。
//!    シェル統合の合図（cwd の変化・コマンドの終わり）と `restart` で引き直す

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_control::lsp::manager::DEFAULT_IDLE_GRACE;
use tako_control::lsp::{DocLink, GotoRequest, Launch, LspConfig, LspManager};
use tako_core::lsp::goto::GotoKind;
use tako_core::lsp::servers::{self, ServerSpec};
use tako_core::lsp::state::RestartPolicy;
use tako_core::platform::child_cmd::ChildCmd;
use tako_core::test_residue::ScratchDir;

const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");

// 本番コードの範囲取りは 1 実装（#1420）。配線の番犬（末尾）が使う
#[path = "common/production_range.rs"]
mod production_range;

/// 1 件ぶんの置き場（スコープを抜けると消える = #1312）
struct Scratch {
    dir: ScratchDir,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = ScratchDir::new(&format!("tako-1769-{tag}"));
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        Self { dir }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn log(&self) -> PathBuf {
        self.path("received.jsonl")
    }

    fn doc_log(&self) -> PathBuf {
        self.path("docs.jsonl")
    }
}

fn fake_args(scratch: &Scratch, breaks: &str, extra: &[&str]) -> Vec<String> {
    let rules = scratch.path("goto.json");
    std::fs::write(&rules, json!([{ "echo": true }]).to_string()).unwrap();
    let mut args: Vec<String> = [
        "--scenario",
        "normal",
        "--log",
        &scratch.log().display().to_string(),
        "--doc-log",
        &scratch.doc_log().display().to_string(),
        "--goto",
        &rules.display().to_string(),
        "--line-breaks",
        breaks,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.extend(extra.iter().map(|s| s.to_string()));
    args
}

fn config_with(args: Vec<String>) -> LspConfig {
    LspConfig {
        table: servers::SERVERS,
        launcher: Arc::new(move |_spec: &ServerSpec| Launch::Found {
            plan: ChildCmd {
                program: FAKE.to_string(),
                args: args.clone(),
            },
            program_path: FAKE.to_string(),
        }),
        request_timeout: Duration::from_secs(10),
        shutdown_timeout: Duration::from_secs(5),
        idle_grace: DEFAULT_IDLE_GRACE,
        restart: RestartPolicy {
            max_restarts: 3,
            base_delay_ms: 50,
        },
        raw_log_dir: None,
    }
}

fn read_jsonl(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn methods(scratch: &Scratch) -> Vec<String> {
    read_jsonl(&scratch.log())
        .iter()
        .filter_map(|m| m.get("method").and_then(Value::as_str).map(str::to_string))
        .collect()
}

fn count(scratch: &Scratch, method: &str) -> usize {
    methods(scratch).iter().filter(|m| *m == method).count()
}

fn wait_until(what: &str, limit: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while !done() {
        assert!(
            Instant::now() < deadline,
            "{what} が {limit:?} 以内に起きなかった"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// サーバが持つべき本文 = 単独の `\r` を `\n` に替えたもの（tako の実装を使わずに組む）
fn wire(text: &str) -> String {
    text.replace("\r\n", "\u{0}")
        .replace('\r', "\n")
        .replace('\u{0}', "\r\n")
}

/// tako の座標（`\n` 区切りの 0 起点の行・行内の UTF-8 バイト桁）での `needle` の出現位置
/// （tako の実装を使わずに組む）
fn tako_positions(text: &str, needle: &str) -> Vec<(usize, usize)> {
    text.match_indices(needle)
        .map(|(at, _)| {
            let line = text[..at].matches('\n').count();
            let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
            (line, at - start)
        })
        .collect()
}

/// 偽サーバが持っている本文（その版の didOpen / didChange を当てた後）
fn server_text(scratch: &Scratch, version: u64) -> Option<String> {
    read_jsonl(&scratch.doc_log())
        .iter()
        .rev()
        .find(|e| e["fake_doc"]["version"] == json!(version))
        .and_then(|e| e["fake_doc"]["text"].as_str().map(str::to_string))
}

/// 開いている文書の診断を `(番号, tako の行, 桁)` の並びで（`MARK#<番号>` の順）
fn mark_diagnostics(manager: &LspManager, link: &DocLink) -> Vec<(usize, usize, usize)> {
    let DocLink::Open(lease) = link else {
        return Vec::new();
    };
    let Some(doc) = manager.document_diagnostics(lease.uri()) else {
        return Vec::new();
    };
    let mut out: Vec<(usize, usize, usize)> = doc
        .diagnostics
        .items
        .iter()
        .filter_map(|d| {
            let n = d.message.strip_prefix("MARK#")?.parse().ok()?;
            Some((n, d.start.line, d.start.col))
        })
        .collect();
    out.sort();
    out
}

fn expected_marks(text: &str) -> Vec<(usize, usize, usize)> {
    tako_positions(text, "MARK")
        .into_iter()
        .enumerate()
        .map(|(n, (line, col))| (n, line, col))
        .collect()
}

// --- 1. 単独の `\r` ---------------------------------------------------------

/// CRLF と単独 CR と LF が混ざり、単独 CR の後ろに多バイト文字と絵文字が並ぶ本文
const MIXED: &str = "fn a() {}\rMARK x;\r\n// 日本😀\rlet MARK = 1;\n\rMARK\r";

/// 受け入れ条件 1（診断の向き = サーバ → tako）: どちらの数え方のサーバでも、サーバが
/// 自分の本文で数えた位置を tako の座標へ写すと、本文の中の実際の位置と一致する。
/// 編集（単独 CR の直後へ打つ / 単独 CR を CRLF にする / CRLF を単独 CR にする /
/// 単独 CR を消す / 末尾へ足す）のあとも、サーバの本文は今の本文の送る形と一致し続ける
#[test]
fn 単独の_cr_を含む本文で診断の位置とサーバの本文が一致する() {
    for breaks in ["spec", "lf"] {
        let scratch = Scratch::new(&format!("diag-{breaks}"));
        let manager = LspManager::new(config_with(fake_args(
            &scratch,
            breaks,
            &["--mark", "MARK"],
        )));
        let path = scratch.path("src/main.rs");
        let mut link = DocLink::default();
        let mut text = MIXED.to_string();
        let mut version = 1;
        manager.sync(&mut link, true, &path, &text, version);
        let edits: [&dyn Fn(&str) -> String; 6] = [
            // 単独 CR の直後へ打つ
            &|t| t.replacen("\rMARK x;", "\rzz MARK x;", 1),
            // 単独 CR を CRLF にする
            &|t| t.replacen("😀\rlet", "😀\r\nlet", 1),
            // CRLF を単独 CR にする
            &|t| t.replacen("x;\r\n", "x;\r", 1),
            // 単独 CR を消す
            &|t| t.replacen("{}\r", "{}", 1),
            // 末尾へ単独 CR と語を足す
            &|t| format!("{t}😀MARK\r"),
            // 多バイト文字の前の単独 CR の手前へ打つ
            &|t| t.replacen("\n\rMARK", "\n\r日MARK", 1),
        ];
        let mut step = 0;
        loop {
            let want = wire(&text);
            wait_until(
                &format!("{breaks} 版 {version} の本文"),
                Duration::from_secs(10),
                || server_text(&scratch, version).as_deref() == Some(want.as_str()),
            );
            let expected = expected_marks(&text);
            wait_until(
                &format!("{breaks} 版 {version} の診断 {expected:?}"),
                Duration::from_secs(10),
                || mark_diagnostics(&manager, &link) == expected,
            );
            let Some(edit) = edits.get(step) else {
                break;
            };
            text = edit(&text);
            version += 1;
            step += 1;
            manager.sync(&mut link, true, &path, &text, version);
        }
        // 開いた本文は送る形（単独 CR を LF に替えたもの）で届いている
        let opened = read_jsonl(&scratch.log())
            .into_iter()
            .find(|m| m["method"] == json!("textDocument/didOpen"))
            .unwrap();
        assert_eq!(opened["params"]["textDocument"]["text"], json!(wire(MIXED)));
        assert_eq!(count(&scratch, "textDocument/didChange"), edits.len());
        manager.shutdown_all(Duration::from_secs(2));
    }
}

fn goto(path: &Path, line: usize, column: usize) -> GotoRequest {
    GotoRequest {
        kind: GotoKind::Definition,
        path: path.to_path_buf(),
        line,
        column,
        timeout: Duration::from_secs(10),
        document: None,
    }
}

/// 受け入れ条件 1（問い合わせの向き = tako → サーバ → tako）: tako の座標で問うと、サーバは
/// 自分の本文でその位置の語を引き、その範囲を返す。着地は問うた語の頭 = 位置が往復で一致する。
/// 文書が開いていない（ディスクの中身を一時的に開く）ときと、編集中（送った写し）のときの両方
#[test]
fn 単独の_cr_の後ろの語へ位置を往復させても同じ語へ着く() {
    let text = "a\rMARKa b\r\n😀\rc MARKb\n\r日 MARKc\r";
    let words = ["MARKa", "MARKb", "MARKc"];
    for breaks in ["spec", "lf"] {
        let scratch = Scratch::new(&format!("goto-{breaks}"));
        let path = scratch.path("src/main.rs");
        std::fs::write(&path, text).unwrap();
        let manager = LspManager::new(config_with(fake_args(&scratch, breaks, &[])));
        let mut link = DocLink::default();
        for editing in [false, true] {
            if editing {
                manager.sync(&mut link, true, &path, text, 1);
            }
            for word in words {
                let (line, col) = tako_positions(text, word)[0];
                // 語の途中（頭から 2 バイト）を問う
                let answer = manager.goto(&goto(&path, line, col + 2)).unwrap();
                assert_eq!(answer.targets.len(), 1, "{breaks} {word}");
                let target = &answer.targets[0];
                assert_eq!(
                    (target.line, target.column),
                    (line, col),
                    "{breaks} 編集中={editing} {word}: 着地は語の頭"
                );
                let asked = read_jsonl(&scratch.doc_log())
                    .into_iter()
                    .rev()
                    .find_map(|e| e.get("fake_goto").cloned())
                    .unwrap();
                assert_eq!(asked["word"], json!(word), "{breaks} 編集中={editing}");
            }
        }
        // 一時的に開いた文書は閉じ、編集中の文書は開いたまま
        assert_eq!(count(&scratch, "textDocument/didOpen"), 1 + words.len());
        assert_eq!(count(&scratch, "textDocument/didClose"), words.len());
        manager.shutdown_all(Duration::from_secs(2));
    }
}

// --- 2. 同じファイルの 2 ペイン目 ---------------------------------------------

/// 受け入れ条件 2: 同じファイルを 2 ペインで開いて片方で編集すると、サーバには **1 文書**として
/// didChange が届き（didOpen は 1 回）、診断は両方のペインの持つ URI で読める。版は文書ごとに
/// 単調に進む（2 つ目のペインのバッファの版が若くても下回らない）。変わっていないペインの同期は
/// 何も送らない（相手の編集を巻き戻さない）。片方を閉じても文書は開いたまま、両方閉じると
/// didClose が 1 回。読み取り表示のペイン（編集していない）は持ち手にならない
#[test]
fn 同じファイルの2ペインは1つの文書を共有し最後が閉じたときだけ閉じる() {
    let scratch = Scratch::new("shared");
    let manager = LspManager::new(config_with(fake_args(
        &scratch,
        "spec",
        &["--mark", "MARK"],
    )));
    let path = scratch.path("src/main.rs");
    let (mut a, mut b, mut viewer) = (DocLink::default(), DocLink::default(), DocLink::default());
    manager.sync(&mut a, true, &path, "MARK a\n", 1);
    wait_until("didOpen", Duration::from_secs(10), || {
        count(&scratch, "textDocument/didOpen") == 1
    });
    // A だけが編集を進める（A のバッファの版は 5）
    manager.sync(&mut a, true, &path, "MARK a\nx\n", 5);
    // B はディスクから開いた若いバッファ（版 1）。読み取り表示のペインは編集していない
    manager.sync(&mut b, true, &path, "MARK a\n", 1);
    manager.sync(&mut viewer, false, &path, "MARK a\n", 1);
    assert!(
        matches!(viewer, DocLink::Unlinked),
        "読み取り表示は持ち手にならない"
    );
    let status = manager.status(None);
    assert_eq!(status["documents"], json!(1));
    assert_eq!(status["servers"][0]["views"], json!(2), "持ち手は A と B");
    // B で編集 → 1 文書として didChange（版は A の 5 を下回らない）
    manager.sync(&mut b, true, &path, "MARK a\nMARK b\n", 2);
    wait_until("B の編集の didChange", Duration::from_secs(10), || {
        count(&scratch, "textDocument/didChange") == 2
    });
    assert_eq!(
        count(&scratch, "textDocument/didOpen"),
        1,
        "2 つ目は didOpen しない"
    );
    // 変わっていない A の同期（カーソル移動だけ）は何も送らない = B の編集を巻き戻さない
    manager.sync(&mut a, true, &path, "MARK a\nx\n", 5);
    manager.sync(&mut b, true, &path, "MARK a\nMARK b\nMARK c\n", 3);
    wait_until(
        "B の 2 回目の didChange",
        Duration::from_secs(10),
        || count(&scratch, "textDocument/didChange") == 3,
    );
    let versions: Vec<u64> = read_jsonl(&scratch.log())
        .iter()
        .filter(|m| m["method"] == json!("textDocument/didChange"))
        .map(|m| m["params"]["textDocument"]["version"].as_u64().unwrap())
        .collect();
    assert_eq!(versions, vec![5, 6, 7], "版は文書ごとに単調に進む");
    assert_eq!(
        server_text(&scratch, 7).as_deref(),
        Some("MARK a\nMARK b\nMARK c\n"),
        "サーバが持つのは最後に編集したペインの本文"
    );
    // 診断は両方のペインが持つ URI で読める（同じ 1 つ）
    let uri = |link: &DocLink| match link {
        DocLink::Open(lease) => lease.uri().to_string(),
        other => panic!("開いていない: {other:?}"),
    };
    assert_eq!(uri(&a), uri(&b));
    let expected = expected_marks("MARK a\nMARK b\nMARK c\n");
    wait_until(
        "両方のペインの診断",
        Duration::from_secs(10),
        || mark_diagnostics(&manager, &a) == expected && mark_diagnostics(&manager, &b) == expected,
    );
    // A を閉じても文書は開いたまま（didClose なし・診断も残る）
    manager.sync(&mut a, false, &path, "", 5);
    manager.sync(&mut b, true, &path, "MARK a\n", 4);
    wait_until(
        "A を閉じた後の B の didChange",
        Duration::from_secs(10),
        || count(&scratch, "textDocument/didChange") == 4,
    );
    assert_eq!(count(&scratch, "textDocument/didClose"), 0);
    let status = manager.status(None);
    assert_eq!(status["documents"], json!(1));
    assert_eq!(status["servers"][0]["views"], json!(1));
    wait_until("B の診断", Duration::from_secs(10), || {
        mark_diagnostics(&manager, &b) == expected_marks("MARK a\n")
    });
    // B も閉じると didClose が 1 回・診断は捨てる
    manager.sync(&mut b, false, &path, "", 4);
    wait_until("didClose", Duration::from_secs(10), || {
        count(&scratch, "textDocument/didClose") == 1
    });
    assert_eq!(manager.status(None)["documents"], json!(0));
    assert_eq!(manager.diagnostics_retained(), (0, 0));
    // 開き直せば didOpen から（2 回目）
    manager.sync(&mut a, true, &path, "MARK a\n", 1);
    wait_until("開き直しの didOpen", Duration::from_secs(10), || {
        count(&scratch, "textDocument/didOpen") == 2
    });
    assert_eq!(count(&scratch, "textDocument/didClose"), 1);
    manager.shutdown_all(Duration::from_secs(2));
}

/// サーバが起きる前に 2 ペインが開いて両方が編集しても、握手の後の didOpen は 1 回で、
/// 本文は最後に編集したペインのもの
#[test]
fn 起動中に2ペインが編集しても握手の後の_did_open_は1回() {
    let scratch = Scratch::new("shared-starting");
    let manager = LspManager::new(config_with(fake_args(&scratch, "spec", &[])));
    let path = scratch.path("src/main.rs");
    let (mut a, mut b) = (DocLink::default(), DocLink::default());
    manager.sync(&mut a, true, &path, "fn a() {}\n", 1);
    manager.sync(&mut b, true, &path, "fn a() {}\n", 1);
    manager.sync(&mut b, true, &path, "fn b() {}\n", 2);
    wait_until("didOpen", Duration::from_secs(10), || {
        count(&scratch, "textDocument/didOpen") >= 1
    });
    // 起動中の編集は写しに溜まり、握手の後の didOpen が最新の本文を運ぶ（または直後の didChange）
    wait_until("サーバの本文", Duration::from_secs(10), || {
        read_jsonl(&scratch.doc_log())
            .last()
            .and_then(|e| e["fake_doc"]["text"].as_str().map(str::to_string))
            .as_deref()
            == Some("fn b() {}\n")
    });
    assert_eq!(count(&scratch, "textDocument/didOpen"), 1);
    manager.shutdown_all(Duration::from_secs(2));
}

// --- 3. サーバの解決のキャッシュ ----------------------------------------------

/// 数える launcher: 表の先頭 2 つは偽サーバで「見つかった」、3 つ目は `found_path`（消せる
/// 実行ファイル）で「見つかった」、残りは「見つからない」。呼ばれるたびに ID を記録する
fn counting_config(
    scratch: &Scratch,
    found_path: PathBuf,
    calls: Arc<std::sync::Mutex<Vec<&'static str>>>,
) -> LspConfig {
    let args = fake_args(scratch, "spec", &[]);
    let mut config = config_with(args.clone());
    config.launcher = Arc::new(move |spec: &ServerSpec| {
        calls.lock().unwrap().push(spec.id);
        let index = servers::SERVERS
            .iter()
            .position(|s| s.id == spec.id)
            .unwrap();
        match index {
            0 | 1 => Launch::Found {
                plan: ChildCmd {
                    program: FAKE.to_string(),
                    args: args.clone(),
                },
                program_path: FAKE.to_string(),
            },
            2 => Launch::Found {
                plan: ChildCmd {
                    program: FAKE.to_string(),
                    args: args.clone(),
                },
                program_path: found_path.display().to_string(),
            },
            _ => Launch::NotFound {
                program: spec.program.to_string(),
                override_env: None,
            },
        }
    });
    config
}

fn cached_flags(value: &Value) -> Vec<bool> {
    value["servers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["cached"].as_bool().unwrap())
        .collect()
}

/// `cached` を除いた行（解決の答えそのものは同じであること）
fn without_cached(value: &Value) -> Value {
    let mut value = value.clone();
    for row in value["servers"].as_array_mut().unwrap() {
        row.as_object_mut().unwrap().remove("cached");
    }
    value
}

/// 受け入れ条件 3: `servers` の 2 回目以降は解決（ログインシェル）を起こさない。
/// 引き直すのはシェル統合の合図（cwd の変化・コマンドの終わり）・`restart`（名指しならその 1 つ）・
/// 見つかったパスが実行できなくなったときだけ。起動もキャッシュを使う
#[test]
fn servers_の2回目以降は解決を起こさず合図と_restart_で引き直す() {
    let scratch = Scratch::new("resolve-cache");
    let removable = scratch.path("removable-server");
    std::fs::copy(FAKE, &removable).unwrap();
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let manager = LspManager::new(counting_config(
        &scratch,
        removable.clone(),
        Arc::clone(&calls),
    ));
    let n = servers::SERVERS.len();
    let calls_now = || calls.lock().unwrap().len();

    let first = manager.servers();
    assert_eq!(calls_now(), n, "1 回目は表のすべてを引く");
    assert_eq!(cached_flags(&first), vec![false; n]);
    let second = manager.servers();
    assert_eq!(calls_now(), n, "2 回目は 1 つも引かない");
    assert_eq!(cached_flags(&second), vec![true; n]);
    assert_eq!(
        without_cached(&first),
        without_cached(&second),
        "答えは同じ"
    );

    // シェル統合の合図（PATH が変わりうる出来事）で全部を引き直す
    tako_core::shell_activity::note();
    assert_eq!(cached_flags(&manager.servers()), vec![false; n]);
    assert_eq!(calls_now(), 2 * n);
    assert_eq!(cached_flags(&manager.servers()), vec![true; n]);

    // 名指しの restart はその 1 つだけ引き直す
    let named = servers::SERVERS[1].id;
    manager.restart(Some(named));
    let flags = cached_flags(&manager.servers());
    assert_eq!(calls_now(), 2 * n + 1);
    assert_eq!(calls.lock().unwrap().last(), Some(&named));
    assert_eq!(
        flags,
        (0..n).map(|i| i != 1).collect::<Vec<_>>(),
        "名指ししたものだけがいま引いた"
    );

    // 見つかったパスが消えたら（アンインストール）そのサーバだけ引き直す
    std::fs::remove_file(&removable).unwrap();
    let flags = cached_flags(&manager.servers());
    assert_eq!(calls_now(), 2 * n + 2);
    assert_eq!(flags, (0..n).map(|i| i != 2).collect::<Vec<_>>());

    // 名指しなしの restart は全部を引き直す
    manager.restart(None);
    assert_eq!(cached_flags(&manager.servers()), vec![false; n]);
    assert_eq!(calls_now(), 3 * n + 2);

    // 起動もキャッシュを使う（編集モードに入っても引かない）
    let mut link = DocLink::default();
    manager.sync(
        &mut link,
        true,
        &scratch.path("src/main.rs"),
        "fn main() {}\n",
        1,
    );
    wait_until("稼働", Duration::from_secs(10), || {
        manager.status(None)["servers"][0]["state"] == json!("running")
    });
    assert_eq!(calls_now(), 3 * n + 2, "起動で解決を起こさない");
    manager.shutdown_all(Duration::from_secs(2));
}

/// 解決のログインシェルを打ち切ったら、見つからないとは言わずに理由と次の一手を返す
/// （`servers` の行・起動しようとした文書の状態・定義ジャンプの答えのどれでも）
#[test]
fn 解決が打ち切られたら理由と次の一手を返す() {
    let scratch = Scratch::new("resolve-timeout");
    let mut config = config_with(fake_args(&scratch, "spec", &[]));
    config.launcher = Arc::new(|spec: &ServerSpec| Launch::TimedOut {
        program: spec.program.to_string(),
        waited_secs: 15,
    });
    let manager = LspManager::new(config);
    let spec = &servers::SERVERS[0];
    let reason = tako_control::lsp::text::fill(
        tako_control::lsp::text::RESOLVE_TIMEOUT_REASON,
        &[("program", spec.program), ("secs", "15")],
    );
    let next_step = tako_control::lsp::text::RESOLVE_TIMEOUT_NEXT_STEP.text();
    let row = manager.servers()["servers"][0].clone();
    assert_eq!(row["installed"], json!(false));
    assert_eq!(row["timed_out"], json!(true));
    assert_eq!(row["reason"], json!(reason));
    assert_eq!(row["next_step"], json!(next_step));

    let path = scratch.path("src/main.rs");
    std::fs::write(&path, "fn main() {}\n").unwrap();
    let mut link = DocLink::default();
    manager.sync(&mut link, true, &path, "fn main() {}\n", 1);
    wait_until("未導入", Duration::from_secs(10), || {
        manager.status(None)["servers"][0]["state"] == json!("not_installed")
    });
    let status = manager.status(None)["servers"][0].clone();
    assert_eq!(status["reason"], json!(reason));
    assert_eq!(status["next_step"], json!(next_step));
    let error = manager.goto(&goto(&path, 0, 3)).unwrap_err();
    let answer = error.to_json(GotoKind::Definition);
    assert_eq!(answer["reason"], json!(reason));
    assert!(read_jsonl(&scratch.log()).is_empty(), "何も起こしていない");
}

/// 既定の解決（`default_launch`）の子プロセス側。`CHILD_ENV` があるときだけ動く
const CHILD_ENV: &str = "TAKO_1769_LAUNCH_CHILD";

#[test]
fn 子プロセス_既定の解決を1回走らせて結果を書く() {
    let Ok(out) = std::env::var(CHILD_ENV) else {
        return;
    };
    let started = Instant::now();
    let launch = tako_control::lsp::manager::default_launch(&servers::SERVERS[0]);
    std::fs::write(
        out,
        format!("{launch:?}\n{}", started.elapsed().as_millis()),
    )
    .unwrap();
}

/// 既定の解決は、ログインシェルを #1532 の上限（`probe::output_with_timeout` = `probe_timeout`）で
/// 打ち切る。`$SHELL` を返らない偽物にした子プロセスで実際に `default_launch` を走らせる
/// （env を変えるので子に分ける = 並行する他のテストの env を汚さない）
#[cfg(unix)]
#[test]
fn 既定の解決はログインシェルを上限で打ち切る() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("default-launch");
    let shell = scratch.path("hanging-shell");
    // `exec` で置き換える = 打ち切りの kill が眠る本人に当たる（孫を残さない = #1748）
    std::fs::write(&shell, "#!/bin/sh\nexec sleep 30\n").unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = scratch.path("result.txt");
    let started = Instant::now();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "子プロセス_既定の解決を1回走らせて結果を書く",
            "--nocapture",
        ])
        .env(CHILD_ENV, &out)
        .env("SHELL", &shell)
        .env(tako_core::probe::PROBE_TIMEOUT_ENV, "1")
        .env_remove(servers::override_env_name(servers::SERVERS[0].id))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    let result = std::fs::read_to_string(&out).unwrap();
    assert!(
        result.starts_with("TimedOut"),
        "打ち切りとして返る: {result}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "上限（1 秒）で返る: {:?}",
        started.elapsed()
    );
}

/// 配線の番犬（#1769）: シェル統合の出来事を受ける 1 か所（`TerminalSession::process_osc_event`）が
/// 解決のキャッシュの合図（`shell_activity`）を進めている。外れると「コマンドを打っても
/// `tako lsp servers` が古い答えを返し続ける」が単体テストを緑のまま起きる（判定の純粋関数は
/// `tako_core::shell_activity` の単体が、キャッシュの規則は上の e2e が固定する）
#[test]
fn シェル統合の出来事は解決のキャッシュの合図を進める() {
    let rel = "crates/tako-core/src/terminal.rs";
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let src = std::fs::read_to_string(root.join(rel)).unwrap();
    let code = production_range::code_view::without_comments_checked(
        &production_range::production(&src, rel),
        rel,
    );
    let head = "fn process_osc_event(";
    let start = code
        .find(head)
        .unwrap_or_else(|| panic!("{rel} に `{head}` が無い（改名したなら番犬も追うこと）"));
    let body = &code[start..];
    let end = body[head.len()..]
        .find("\n    fn ")
        .or_else(|| body[head.len()..].find("\n    pub fn "))
        .map_or(body.len(), |i| i + head.len());
    let body = &body[..end];
    let line = code[..start].matches('\n').count() + 1;
    for needle in ["shell_activity::is_activity(", "shell_activity::note()"] {
        assert!(
            body.contains(needle),
            "{rel}:{line}（fn process_osc_event）が `{needle}` を呼んでいない = \
             シェル統合の cwd の変化・コマンドの終わりで LSP の解決のキャッシュが引き直されない"
        );
    }
}
