//! 実経路: `tako todo list` / `expand` / `collapse` と `tako sessions list` / `link` の人向けの表示が
//! 表示言語に従う（Issue #2365 の続き。前のスライスは `issue2365_cli_human_lang.rs`）。
//!
//! 修正前（実測）: 英語表示でも `todo list` は「ユーザータスクが無い」「期限:」「返答:」「未完了 N 件」、
//! `todo expand` / `collapse` は「(なし)」「表示」/「非表示」、`sessions list` は空のときの文・
//! 「（resume 不可）」・pending の見出し・末尾の resume の案内、`sessions link` は「アカウント:」と
//! 「公式リンクはありません」・入れ方の案内のように、枠だけ日本語の固定文のまま出た
//! （`main.rs` が `println!` / `eprintln!` へ直書きしていた）。
//!
//! **ビルドした `tako` をそのまま**叩く。どれも GUI へ問い合わせて応答から表示を組むので、GUI の代わりに
//! 決まった応答を返す受け口（偽の GUI）を立てる（URL が付く / 付かない・空 / 行ありをすべて通す）。見ること:
//! - 英語表示: 出力に日本語（かな・漢字）が 1 字も無い（応答の値は ASCII だけにしてある）
//! - 日本語表示: #2365 より前の出力とバイト一致（stdout / stderr とも）
//! - `--json` の出力は日英で同じ
//! - 英語表示でも列の桁（id / kind / status の幅）と字下げが崩れない
//!
//! A/B: `TAKO_2365_HUMAN_BIN=<修正前の tako>` で別のバイナリを叩ける（修正前は英語表示の検査で落ちる）。
//! 本番の data dir・GUI へ届かないよう、`TAKO_*` を全部外して一時の置き場と偽の GUI の口だけを渡す。
//!
//! unix だけで走らせる（偽の GUI が Unix domain socket）。文の日英の対は OS に依らない 1 か所
//! （`ReplyNote`）で、両 OS の番犬（`crates/tako-control/tests/issue1762_dispatch_error_lang.rs`）が表で見る
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};
use tako_control::protocol::ResponseEnvelope;
use tako_core::test_residue::ScratchDir;

/// 叩くバイナリ（既定はこの木でビルドした `tako`。A/B は `TAKO_2365_HUMAN_BIN`）
fn tako() -> PathBuf {
    std::env::var_os("TAKO_2365_HUMAN_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_tako")))
}

/// 1 言語分の砂場（表示言語の設定と、偽の GUI の受け口の置き場）
struct Sandbox {
    dir: ScratchDir,
}

impl Sandbox {
    fn new(lang: &str) -> Self {
        let sb = Self {
            // 偽の GUI の Unix domain socket が macOS の上限（104 バイト）に収まるよう短い名前にする
            dir: ScratchDir::new("i2365t"),
        };
        for d in ["data", "disc", "home", "tmp"] {
            let d = sb.path().join(d);
            std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
        }
        // 書かないと system = 実行環境のロケールで決まり、CI と手元で答えが変わる
        let settings = sb.path().join("data/settings.json");
        std::fs::write(&settings, format!("{{\"language\":\"{lang}\"}}"))
            .unwrap_or_else(|e| panic!("{} を書ける: {e}", settings.display()));
        sb
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// `TAKO_*` を外して隔離の置き場と偽の GUI の口だけを渡した `tako <args>` の標準出力と標準エラー
    fn run(&self, args: &[&str], gui: &str) -> (String, String) {
        let mut command = Command::new(tako());
        command
            .args(args)
            .current_dir(self.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, _) in std::env::vars_os() {
            let key = key.to_string_lossy().to_string();
            if key.starts_with("TAKO_") || key.starts_with("CLAUDE") || key == "CODEX_HOME" {
                command.env_remove(key);
            }
        }
        command
            .env("TAKO_ISOLATED", "1")
            .env("TAKO_DATA_DIR", self.path().join("data"))
            .env("TAKO_DISCOVERY_DIR", self.path().join("disc"))
            .env("HOME", self.path().join("home"))
            .env("TMPDIR", self.path().join("tmp"))
            .env("TAKO_SOCKET", gui)
            .env("TAKO_TOKEN", "t");
        let out = command.output().expect("tako が終わる");
        assert!(
            out.status.success(),
            "{args:?} が失敗した（{}）: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        (
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    }

    /// 決まった `result` を返す偽の GUI（受けた接続すべてへ同じ応答を返す）
    fn fake_gui(&self, tag: &str, result: Value) -> String {
        let path = self.path().join(format!("{tag}.sock"));
        let listener = UnixListener::bind(&path).expect("受け口を作れる");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let mut line = String::new();
                let _ =
                    BufReader::new(stream.try_clone().expect("複製できる")).read_line(&mut line);
                let reply = ResponseEnvelope::ok(1, result.clone());
                let mut stream = stream;
                let _ = writeln!(
                    stream,
                    "{}",
                    serde_json::to_string(&reply).expect("応答を組める")
                );
            }
        });
        path.display().to_string()
    }
}

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

/// 1 手順: 名前・引数・偽の GUI の応答・#2365 より前の日本語表示の stdout / stderr
struct Case {
    name: &'static str,
    args: &'static [&'static str],
    result: Value,
    stdout: &'static str,
    stderr: &'static str,
}

/// `tako todo list` の行ありの応答（期限・返答の有無、列の幅を超える id / kind / status）
fn todo_rows() -> Value {
    json!({
        "tasks": [
            {"id": "u-1", "kind": "review", "status": "open", "title": "Review the PR",
             "due": "2026-11-05", "responses": [{"decision": "approve"}, {"decision": "hold"}]},
            {"id": "u-2", "kind": "approval", "status": "done", "title": "Approve the release"},
            {"id": "u-123456", "kind": "very-long-kind", "status": "in_progress", "title": "Long",
             "responses": [{"decision": "approve"}]}
        ],
        "open_count": 2
    })
}

/// `tako sessions list` の行ありの応答（resume の可否・issues・pending の器あり / なし）
fn sessions_rows() -> Value {
    json!({
        "sessions": [
            {"short_id": "0123abcd", "last_seen_at": "2026-11-01T00:00:00Z", "kind": "pane",
             "project": "alpha", "label": "fix bug", "issues": [12, 34], "resumable": true},
            {"short_id": "89abcdef", "last_seen_at": "2026-11-02T00:00:00Z", "kind": "worker",
             "label": "w1", "resumable": false},
            {"short_id": "fedcba98", "last_seen_at": "2026-11-03T00:00:00Z", "kind": "master",
             "project": "beta", "issues": [], "resumable": true}
        ],
        "pending": [
            {"recorded_at": "2026-11-02T01:00:00Z", "agent": "codex", "tmux_session": "tako-w-3",
             "project": "beta"},
            {"recorded_at": "2026-11-02T02:00:00Z", "agent": "agy", "pane": 7, "label": "x1"}
        ]
    })
}

const SESSIONS_HINT_JA: &str =
    "(resume: tako sessions resume <id> / 詳細: tako sessions show <id>)\n";

/// 叩く手順（#2365 より前の日本語表示は origin/main の実測）
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "todo_list_empty",
            args: &["todo", "list"],
            result: json!({"tasks": [], "open_count": 0}),
            stdout: "ユーザータスクが無い\n未完了 0 件\n",
            stderr: "",
        },
        Case {
            name: "todo_list",
            args: &["todo", "list"],
            result: todo_rows(),
            stdout: "u-1     review      open       Review the PR  期限:2026-11-05  返答:2\n\
                     u-2     approval    done       Approve the release\n\
                     u-123456  very-long-kind  in_progress  Long  返答:1\n\
                     未完了 2 件\n",
            stderr: "",
        },
        Case {
            name: "todo_expand",
            args: &["todo", "expand", "u-1"],
            result: json!({"expanded": "u-1", "panel_visible": true, "panel_view": "tasks"}),
            stdout: "expanded: u-1\npanel: 表示 / view: tasks\n",
            stderr: "",
        },
        Case {
            name: "todo_collapse",
            args: &["todo", "collapse"],
            result: json!({"expanded": null, "panel_visible": false, "panel_view": "fleet"}),
            stdout: "expanded: (なし)\npanel: 非表示 / view: fleet\n",
            stderr: "",
        },
        // 画面の状態が載らない応答（GUI の版ずれ）でも「開いた」と言わない
        Case {
            name: "todo_collapse_bare",
            args: &["todo", "collapse"],
            result: json!({}),
            stdout: "expanded: (なし)\npanel: 非表示 / view: -\n",
            stderr: "",
        },
        Case {
            name: "sessions_list_empty",
            args: &["sessions", "list"],
            result: json!({"sessions": [], "pending": []}),
            stdout: "カタログにセッションが無い（claude ペインの検出後に記録される）\n",
            stderr: SESSIONS_HINT_JA,
        },
        Case {
            name: "sessions_list",
            args: &["sessions", "list"],
            result: sessions_rows(),
            stdout: "0123abcd  2026-11-01T00:00:00Z  pane    alpha: fix bug [#12,#34]\n\
                     89abcdef  2026-11-02T00:00:00Z  worker  w1（resume 不可）\n\
                     fedcba98  2026-11-03T00:00:00Z  master  beta\n\
                     --- session 未検出の spawn 記録（codex / agy・起動直後の claude）---\n\
                     2026-11-02T01:00:00Z  codex  tako-w-3  beta\n\
                     2026-11-02T02:00:00Z  agy  pane 7  x1\n",
            stderr: SESSIONS_HINT_JA,
        },
        Case {
            name: "sessions_link",
            args: &["sessions", "link", "--pane", "1"],
            result: json!({"pane": 1, "agent": "claude", "remote_link": {
                "url": "https://example.com/code/s1", "account_label": "univ", "state": "connected"}}),
            stdout: "https://example.com/code/s1\n",
            stderr: "アカウント: univ（このアカウントでログインした端末からだけ開けます）\n",
        },
        // アカウント名が分からない URL は URL だけ
        Case {
            name: "sessions_link_no_account",
            args: &["sessions", "link", "--pane", "1"],
            result: json!({"remote_link": {"url": "https://example.com/code/s1", "state": "connected"}}),
            stdout: "https://example.com/code/s1\n",
            stderr: "",
        },
        Case {
            name: "sessions_link_none",
            args: &["sessions", "link", "--pane", "1"],
            result: json!({"remote_link": {"url": null, "state": "not_connected"}}),
            stdout: "",
            stderr: "公式リンクはありません（state: not_connected）\n\
                     \x20 有効にする場合: tako orchestrator profiles set <名前> --remote-control true → 起動し直す\n",
        },
        // state の無い応答は unknown
        Case {
            name: "sessions_link_bare",
            args: &["sessions", "link", "--pane", "1"],
            result: json!({}),
            stdout: "",
            stderr: "公式リンクはありません（state: unknown）\n\
                     \x20 有効にする場合: tako orchestrator profiles set <名前> --remote-control true → 起動し直す\n",
        },
    ]
}

/// `--json` の手順（出力が日英で同じ）
const JSON_CASES: &[(&str, &[&str])] = &[
    ("todo_list", &["todo", "list", "--json"]),
    ("todo_expand", &["todo", "expand", "u-1", "--json"]),
    ("sessions_list", &["sessions", "list", "--json"]),
    (
        "sessions_link",
        &["sessions", "link", "--pane", "1", "--json"],
    ),
];

/// 1 言語分を叩いて (名前, stdout, stderr) を返す（`--json` は名前に `_json` を付ける）
fn run_all(lang: &str) -> Vec<(String, String, String)> {
    let sb = Sandbox::new(lang);
    let mut out = Vec::new();
    for (i, case) in cases().into_iter().enumerate() {
        let gui = sb.fake_gui(&format!("c{i}"), case.result.clone());
        let (o, e) = sb.run(case.args, &gui);
        out.push((case.name.to_string(), o, e));
        if let Some((_, args)) = JSON_CASES.iter().find(|(n, _)| *n == case.name) {
            let (o, e) = sb.run(args, &gui);
            out.push((format!("{}_json", case.name), o, e));
        }
    }
    out
}

fn find<'a>(all: &'a [(String, String, String)], name: &str) -> (&'a str, &'a str) {
    let (_, o, e) = all
        .iter()
        .find(|(n, _, _)| n == name)
        .unwrap_or_else(|| panic!("{name} を叩いていない"));
    (o, e)
}

#[test]
fn todoとsessionsの人向けの表示は英語表示に日本語を含まず日本語表示は2365より前とバイト一致() {
    let en = run_all("en");
    let ja = run_all("ja");
    let mut failures = Vec::new();
    for (name, o, e) in &en {
        if let Some(c) = has_japanese(o).or_else(|| has_japanese(e)) {
            failures.push(format!("{name}: 英語表示に日本語（{c}）が混ざる:\n{o}{e}"));
        }
    }
    for case in cases() {
        let (o, e) = find(&ja, case.name);
        for (stream, got, want) in [("stdout", o, case.stdout), ("stderr", e, case.stderr)] {
            if got != want {
                failures.push(format!(
                    "{}: 日本語表示の {stream} が #2365 より前と違う\n  want: {want:?}\n  got:  {got:?}",
                    case.name
                ));
            }
        }
    }
    for (name, _) in JSON_CASES {
        let name = format!("{name}_json");
        let (a, b) = (find(&ja, &name), find(&en, &name));
        if a != b || serde_json::from_str::<Value>(a.0).is_err() {
            failures.push(format!(
                "{name}: --json の出力が日英で違う（か JSON でない）\n  ja: {a:?}\n  en: {b:?}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} 件で落ちた:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// 英語表示の枠の見本（列の幅と字下げが日本語と同じ。ラベルは値の直前にそのまま付く）
#[test]
fn 英語表示の枠は列と字下げを保つ() {
    let en = run_all("en");
    let (o, _) = find(&en, "todo_list");
    assert_eq!(
        o,
        "u-1     review      open       Review the PR  due:2026-11-05  replies:2\n\
         u-2     approval    done       Approve the release\n\
         u-123456  very-long-kind  in_progress  Long  replies:1\n\
         Open: 2\n"
    );
    let (o, _) = find(&en, "todo_collapse");
    assert_eq!(o, "expanded: (none)\npanel: hidden / view: fleet\n");
    let (o, _) = find(&en, "todo_expand");
    assert_eq!(o, "expanded: u-1\npanel: shown / view: tasks\n");
    let (o, e) = find(&en, "sessions_list");
    let lines: Vec<&str> = o.lines().collect();
    assert_eq!(
        lines[1], "89abcdef  2026-11-02T00:00:00Z  worker  w1 (not resumable)",
        "{o}"
    );
    assert!(
        lines[3].starts_with("--- ") && lines[3].ends_with(" ---"),
        "{o}"
    );
    assert_eq!(
        e,
        "(resume: tako sessions resume <id> / details: tako sessions show <id>)\n"
    );
    let (_, e) = find(&en, "sessions_link_none");
    let lines: Vec<&str> = e.lines().collect();
    assert_eq!(lines[0], "No official link (state: not_connected)", "{e}");
    assert!(
        lines[1].starts_with("  to turn it on: tako orchestrator profiles set <name> "),
        "案内の行は日本語と同じ 2 桁の字下げ: {e}"
    );
}
