//! 実経路: `tako search`（横断検索・置換・置換の取り消し）と `tako jump` の人向けの表示の枠が表示言語に従う
//! （Issue #2365 の続き）。
//!
//! 修正前（実測）: 英語表示でも `tako search` は「開いた: …（new-pane）」「右パネルの検索結果を閉じた」
//! 「7 件（3 ファイル・12 ms）」「上限 5000 件で打ち切り（…）」「探していない: バイナリ 1・読めない 2」、
//! `--replace` は「当てない: …」「編集中のペイン 5 に当てる: …」「ほか 3 行（…）」、`--undo` は
//! 「置換 #2（foo → bar）」「（退避: …）」、`tako jump` は「ジャンプ履歴は空（…）」
//! 「読み飛ばした（ファイルが無い）: …」「（閉じたペインの項目なので開き直した）」のように、枠だけ日本語の
//! 固定文のまま出た（`main.rs` の `print_search` / `print_replace` / `print_undo` / `print_jump` の直書き）。
//! 行の中身（抜粋・当てない理由・`note` / `next_step`・移れない理由）は GUI が表示言語で組む応答の値。
//!
//! **ビルドした `tako` をそのまま**叩く。GUI の代わりに決まった応答を返す受け口（偽の GUI）を 1 ケースに
//! 1 つ立てる（表示は CLI が応答から組むので、応答の中身は固定でよい。値は ASCII だけにしてある）。見ること:
//! - 英語表示: 出力に日本語（かな・漢字）が 1 字も無い
//! - 日本語表示: #2365 より前の出力とバイト一致（`<SB>` は砂場の dir）
//! - 行の数と字下げは日英で同じ・`--json` は日英で同じ（応答のまま）・終了コードは日英で同じ
//!
//! A/B: `TAKO_2365_HUMAN_BIN=<修正前の tako>` で別のバイナリを叩ける（修正前は英語表示の検査で落ちる）。
//! `I2365S_DUMP=<dir>` を付けると全ケースの出力を書き出す（修正前後の `diff -r` 用）。
//! 本番の data dir・GUI へ届かないよう、`TAKO_*` を全部外して一時の置き場と偽の GUI の口だけを渡す。
//!
//! unix だけで走らせる（偽の GUI が Unix domain socket）。文の日英の対は OS に依らない 1 か所
//! （`ReplyNote`）で、両 OS の番犬（`crates/tako-control/tests/issue1762_dispatch_error_lang.rs`）が表で見る
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use serde_json::{json, Value};
use tako_control::protocol::ResponseEnvelope;
use tako_core::test_residue::ScratchDir;

/// 叩くバイナリ（既定はこの木でビルドした `tako`。A/B は `TAKO_2365_HUMAN_BIN`）
fn tako() -> PathBuf {
    std::env::var_os("TAKO_2365_HUMAN_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_tako")))
}

/// 1 言語分の砂場: data（表示言語）・発見の置き場・ホーム・作業 dir（cwd = `w`）
struct Sandbox {
    dir: ScratchDir,
    /// 実体のパス（cwd は symlink を解いた形で見えるので、相対パスの表示はこちらを基準に組む）
    real: PathBuf,
}

impl Sandbox {
    fn new(lang: &str) -> Self {
        // 偽の GUI の Unix domain socket が macOS の上限（104 バイト）に収まるよう短い名前にする
        let dir = ScratchDir::new("i2365s");
        for d in ["data", "disc", "home", "tmp", "w/src"] {
            let d = dir.path().join(d);
            std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
        }
        // 書かないと system = 実行環境のロケールで決まり、CI と手元で答えが変わる
        std::fs::write(
            dir.path().join("data/settings.json"),
            format!("{{\"language\":\"{lang}\"}}"),
        )
        .expect("settings.json を書ける");
        let real = std::fs::canonicalize(dir.path()).expect("砂場の実体を引ける");
        Self { dir, real }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// 作業 dir（cwd）の中のファイルの絶対パス（表示は cwd からの相対へ縮む）
    fn work(&self, rel: &str) -> String {
        self.real.join("w").join(rel).display().to_string()
    }

    /// 決まった `result` を返す偽の GUI（受けた接続すべてへ同じ応答を返す）
    fn fake_gui(&self, tag: &str, result: Value) -> PathBuf {
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
        path
    }

    /// `TAKO_*` を外して隔離の置き場と偽の GUI だけを渡した `tako <args>`（`<SB>` に置換）。
    /// 受け口の名前は番号（ケース名だと socket のパスが上限を超えうる）
    fn run(&self, index: usize, case: &Case) -> Out {
        let socket = self.fake_gui(&format!("g{index}"), (case.result)(self));
        let mut command = Command::new(tako());
        command
            .args(case.args)
            .current_dir(self.path().join("w"))
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
            .env("TAKO_SOCKET", &socket)
            .env("TAKO_TOKEN", "t");
        let out = command.output().expect("tako が終わる");
        let real = self.real.display().to_string();
        let root = self.path().display().to_string();
        let norm = |b: &[u8]| {
            String::from_utf8_lossy(b)
                .replace(&real, "<SB>")
                .replace(&root, "<SB>")
        };
        Out {
            code: out.status.code().unwrap_or(-1),
            stdout: norm(&out.stdout),
            stderr: norm(&out.stderr),
        }
    }
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

/// 1 ケース: 名前・引数・偽の GUI が返す `result`・`--json` か
struct Case {
    name: &'static str,
    args: &'static [&'static str],
    result: fn(&Sandbox) -> Value,
    json: bool,
}

const fn case(
    name: &'static str,
    args: &'static [&'static str],
    result: fn(&Sandbox) -> Value,
) -> Case {
    Case {
        name,
        args,
        result,
        json: false,
    }
}

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

/// 見つかった検索の応答（打ち切り・残り・探していないファイルの内訳つき）
fn found(sb: &Sandbox) -> Value {
    json!({
        "status": "done",
        "items": [
            {"path": sb.work("src/a.rs"), "line": 12, "column": 9, "excerpt": "let foo = 1;"},
            {"path": sb.work("b.txt"), "line": 3, "column": 1, "excerpt": "foo bar"},
        ],
        "total": 7,
        "files_matched": 3,
        "elapsed_ms": 12,
        "stopped": "limit",
        "truncated": 5,
        "skipped": {"binary": 1, "too_large": 0, "unreadable": 2},
    })
}

fn jump_list(_: &Sandbox) -> Value {
    json!({
        "entries": [
            {"path": "/w/a.rs", "line": 3, "pane": 4, "pane_alive": true},
            {"path": "/w/b.rs", "pane": 7, "pane_alive": false},
        ],
        "history": {"current": 1, "len": 2},
    })
}

fn cases() -> Vec<Case> {
    vec![
        case(
            "s_open",
            &["search", "--open", "1"],
            |sb| json!({"path": sb.work("src/a.rs"), "line": 12, "column": 9, "landing": "new-pane"}),
        ),
        case(
            "s_clear",
            &["search", "--clear"],
            |_| json!({"cleared": true}),
        ),
        case(
            "s_clear_none",
            &["search", "--clear"],
            |_| json!({"cleared": false}),
        ),
        case(
            "s_results_empty",
            &["search", "--results"],
            |_| json!({"status": "empty"}),
        ),
        case(
            "s_results_running",
            &["search", "--results"],
            |_| json!({"status": "searching", "running": {"query": "foo"}}),
        ),
        case(
            "s_results_error",
            &["search", "--results"],
            |_| json!({"status": "error", "reason": "bad regex"}),
        ),
        case("s_found", &["search", "foo"], found),
        case("s_none_time", &["search", "foo"], |_| {
            json!({
                "status": "done", "items": [], "total": 0, "files_searched": 40,
                "stopped": "time", "skipped": {"too_large": 4},
            })
        }),
        case(
            "s_cancelled",
            &["search", "foo"],
            |_| json!({"status": "done", "items": [], "total": 0, "files_searched": 0, "stopped": "cancelled"}),
        ),
        case("r_preview", &["search", "foo", "--replace", "bar"], |sb| {
            json!({
                "items": [
                    {"path": sb.work("src/a.rs"), "line": 12, "excerpt": "let foo = 1;",
                     "replaced": "let bar = 1;", "status": "ok"},
                    {"path": sb.work("b.txt"), "line": 3, "excerpt": "foo bar", "status": "stale"},
                ],
                "files": [
                    {"path": sb.work("b.txt"), "reason": "read-only"},
                    {"path": sb.work("src/a.rs"), "via": "buffer", "pane": 5},
                    {"path": sb.work("c.rs"), "via": "file"},
                ],
                "truncated": 3,
                "note": "preview only (nothing was written)",
                "next_step": "tako search foo --replace bar --yes",
            })
        }),
        case(
            "u_nothing",
            &["search", "--undo"],
            |_| json!({"status": "nothing", "note": "there is no replacement to undo"}),
        ),
        case("u_files", &["search", "--undo", "--yes"], |sb| {
            json!({
                "status": "restored", "id": 2, "query": "foo", "replacement": "bar",
                "files": [
                    {"path": sb.work("src/a.rs"), "reason": "restored", "backup": sb.work("bk/1")},
                    {"path": sb.work("b.txt"), "reason": "edited in the pane after the replacement"},
                ],
                "next_step": "tako search --undo --yes",
            })
        }),
        case(
            "j_empty",
            &["jump", "list"],
            |_| json!({"entries": [], "history": {"len": 0}}),
        ),
        case("j_list", &["jump", "list"], jump_list),
        case("j_back_moved", &["jump", "back"], |_| {
            json!({
                "moved": true, "reopened": true,
                "location": {"path": "/w/a.rs", "line": 3}, "open": {"pane": 9},
                "dropped": [{"path": "/w/gone.rs", "line": 8}, {"path": "/w/gone2.rs"}],
            })
        }),
        case(
            "j_forward_here",
            &["jump", "forward"],
            |_| json!({"moved": true, "location": {"path": "/w/b.rs"}, "open": {"pane": 4}}),
        ),
        case(
            "j_back_reason",
            &["jump", "back"],
            |_| json!({"moved": false, "reason": "there is no earlier entry in the history", "dropped": []}),
        ),
        case(
            "j_back_old_gui",
            &["jump", "back"],
            |_| json!({"moved": false}),
        ),
        Case {
            json: true,
            ..case("s_found_json", &["search", "foo", "--json"], found)
        },
        Case {
            json: true,
            ..case("j_list_json", &["jump", "list", "--json"], jump_list)
        },
    ]
}

fn run_lang(lang: &str) -> Vec<(&'static str, bool, Out)> {
    let sb = Sandbox::new(lang);
    cases()
        .iter()
        .enumerate()
        .map(|(i, c)| (c.name, c.json, sb.run(i, c)))
        .collect()
}

type Runs = (
    Vec<(&'static str, bool, Out)>,
    Vec<(&'static str, bool, Out)>,
);

/// 日英の実行（テストで共有。日英は別の砂場で並行に走らせる）
fn runs() -> &'static Runs {
    static RUNS: OnceLock<Runs> = OnceLock::new();
    RUNS.get_or_init(|| {
        let ja = std::thread::spawn(|| run_lang("ja"));
        let en = run_lang("en");
        let ja = ja.join().expect("日本語の実行が終わる");
        if let Some(dir) = std::env::var_os("I2365S_DUMP") {
            let dir = PathBuf::from(dir);
            for (lang, outs) in [("ja", &ja), ("en", &en)] {
                for (name, _, out) in outs {
                    let d = dir.join(lang);
                    std::fs::create_dir_all(&d).expect("書き出し先を作れる");
                    std::fs::write(
                        d.join(format!("{name}.txt")),
                        format!(
                            "code={}\n--- stdout\n{}--- stderr\n{}",
                            out.code, out.stdout, out.stderr
                        ),
                    )
                    .expect("書き出せる");
                }
            }
        }
        (ja, en)
    })
}

// ---------------------------------------------------------------------------
// #2365 より前の日本語の出力（修正前の `tako` の書き出し `I2365S_DUMP` を写したもの）
// ---------------------------------------------------------------------------

/// (ケース, stdout)。終了コードはどれも 0・stderr はどれも空
fn old_ja() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "s_open",
            "開いた: <SB>/w/src/a.rs:12:9（new-pane）\n",
        ),
        (
            "s_clear",
            "右パネルの検索結果を閉じた\n",
        ),
        (
            "s_clear_none",
            "閉じる検索結果は無かった\n",
        ),
        (
            "s_results_empty",
            "右パネルに検索結果は無い（tako search <検索語> --show で出す）\n",
        ),
        (
            "s_results_running",
            "検索中: foo\n",
        ),
        (
            "s_results_error",
            "検索できなかった: bad regex\n",
        ),
        (
            "s_found",
            "src/a.rs:12:9: let foo = 1;\nb.txt:3:1: foo bar\n7 件（3 ファイル・12 ms） / 上限 2000 件で打ち切り（--path で範囲を狭めるか検索語を絞る） / ほか 5 件（--limit 0 で全部） / 探していない: バイナリ 1・読めない 2\n",
        ),
        (
            "s_none_time",
            "見つからなかった（40 ファイルを探した） / 所要の上限（30 秒）で打ち切り / 探していない: 大きすぎる 4\n",
        ),
        (
            "s_cancelled",
            "見つからなかった（0 ファイルを探した） / 取り消された（次の検索が始まった / 閉じられた）\n",
        ),
        (
            "r_preview",
            "src/a.rs:12: let foo = 1;\n  → let bar = 1;\nb.txt:3: foo bar\n  （stale）\n当てない: b.txt — read-only\n編集中のペイン 5 に当てる: src/a.rs\nほか 3 行（--limit 0 で全部）\npreview only (nothing was written)\ntako search foo --replace bar --yes\n",
        ),
        (
            "u_nothing",
            "there is no replacement to undo\n",
        ),
        (
            "u_files",
            "置換 #2（foo → bar）\nsrc/a.rs: restored（退避: <SB>/w/bk/1）\nb.txt: edited in the pane after the replacement\ntako search --undo --yes\n",
        ),
        (
            "j_empty",
            "ジャンプ履歴は空（行を指定して開くと積まれる: tako open <file> --line <行>）\n",
        ),
        (
            "j_list",
            "    0  /w/a.rs:3  pane 4\n>   1  /w/b.rs  pane 7（ペインは閉じた。戻ると開き直す）\n",
        ),
        (
            "j_back_moved",
            "読み飛ばした（ファイルが無い）: /w/gone.rs:8\n読み飛ばした（ファイルが無い）: /w/gone2.rs\n/w/a.rs:3  pane 9（閉じたペインの項目なので開き直した）\n",
        ),
        (
            "j_forward_here",
            "/w/b.rs  pane 4\n",
        ),
        (
            "j_back_reason",
            "there is no earlier entry in the history\n",
        ),
        (
            "j_back_old_gui",
            "移れる履歴が無い\n",
        ),
    ]
}

#[test]
fn searchとjumpの人向けの表示は表示言語に従い日本語は2365より前と同じ() {
    let (ja, en) = runs();
    let mut failures = Vec::new();
    assert_eq!(ja.len(), en.len());
    for ((name, json, j), (_, _, e)) in ja.iter().zip(en.iter()) {
        if j.code != 0 || e.code != 0 {
            failures.push(format!(
                "{name}: 終了コードが 0 でない（ja {} / en {}）\n{}{}",
                j.code, e.code, j.stderr, e.stderr
            ));
        }
        if *json {
            // `--json` は応答のまま = 日英で同じ
            let parse = |s: &str| serde_json::from_str::<Value>(s).unwrap_or(Value::Null);
            if parse(&j.stdout) != parse(&e.stdout) || parse(&j.stdout).is_null() {
                failures.push(format!(
                    "{name}: --json が日英で違う\n  ja: {}\n  en: {}",
                    j.stdout, e.stdout
                ));
            }
            continue;
        }
        for (stream, text) in [("stdout", &e.stdout), ("stderr", &e.stderr)] {
            if let Some(c) = has_japanese(text) {
                failures.push(format!(
                    "{name}: 英語表示の {stream} に日本語（{c}）が混ざる:\n{text}"
                ));
            }
        }
        if e.stdout.trim().is_empty() {
            failures.push(format!("{name}: 英語表示の出力が空"));
        }
    }
    let old = old_ja();
    for (name, json, _) in ja.iter() {
        if !*json && !old.iter().any(|(n, _)| n == name) {
            failures.push(format!("{name}: 修正前の日本語の出力が表に無い"));
        }
    }
    for (name, stdout) in old {
        let Some((_, _, j)) = ja.iter().find(|(n, _, _)| *n == name) else {
            failures.push(format!("{name}: ケースが無い"));
            continue;
        };
        if j.stdout != stdout || !j.stderr.is_empty() {
            failures.push(format!(
                "{name}: 日本語表示が #2365 より前と違う\n--- want\n{stdout}--- got\n{}--- stderr\n{}",
                j.stdout, j.stderr
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} 件:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// 英語表示でも行の数と字下げは日本語表示と同じ（文だけが入れ替わる。`--json` は除く）。
/// 英語の見本は数ケースだけ字面まで見る（行の中身 = 応答の値はそのまま並ぶ）
#[test]
fn 英語表示の行の数と字下げは日本語と同じ() {
    let (ja, en) = runs();
    let shape = |s: &str| -> Vec<usize> {
        s.lines()
            .map(|l| l.len() - l.trim_start_matches(' ').len())
            .collect()
    };
    let mut failures = Vec::new();
    for ((name, json, j), (_, _, e)) in ja.iter().zip(en.iter()) {
        if !*json && shape(&j.stdout) != shape(&e.stdout) {
            failures.push(format!(
                "{name}: 行の数か字下げが日英で違う\n--- ja\n{}--- en\n{}",
                j.stdout, e.stdout
            ));
        }
    }
    let en_of = |name: &str| {
        en.iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, _, o)| o.stdout.clone())
            .unwrap_or_default()
    };
    let s_found = format!(
        "src/a.rs:12:9: let foo = 1;\nb.txt:3:1: foo bar\n7 match(es) (3 file(s), 12 ms) / stopped at \
         the limit of {} matches (narrow it with --path or a more specific query) / 5 more (--limit 0 \
         shows all) / not searched: binary 1, unreadable 2\n",
        tako_core::workspace_search::MAX_RESULTS
    );
    for (name, want) in [
        ("s_found", s_found.as_str()),
        (
            "r_preview",
            "src/a.rs:12: let foo = 1;\n  → let bar = 1;\nb.txt:3: foo bar\n  (stale)\nnot applied: b.txt — \
             read-only\napplies to the buffer of the editing pane 5: src/a.rs\n3 more line(s) (--limit 0 shows \
             all)\npreview only (nothing was written)\ntako search foo --replace bar --yes\n",
        ),
        (
            "j_list",
            "    0  /w/a.rs:3  pane 4\n>   1  /w/b.rs  pane 7 (the pane was closed; moving here reopens it)\n",
        ),
        ("j_back_old_gui", "no history to move to\n"),
    ] {
        let got = en_of(name);
        if got != want {
            failures.push(format!("{name}: 英語表示\n--- want\n{want}--- got\n{got}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} 件:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}
