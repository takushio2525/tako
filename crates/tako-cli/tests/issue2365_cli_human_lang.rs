//! 実経路: CLI の人向けの表示（JSON でない出力の枠・見出し・ラベル）が表示言語に従う（Issue #2365 の続き）。
//!
//! 修正前（実測）: 英語表示でも `tako orchestrator projects list` は「登録済みプロジェクトはありません。」、
//! `projects add` は「追加しました: …」、`projects mcp` は「… の worker へ持ち込む user scope の MCP: …」、
//! `tako orchestrator guide` の一覧は「引ける手順書: N 件」と「約 N tok」、`tako sessions resume` は
//! 「復元しました: ペイン …」、`tako todo show` は「添付:」「コピー用」「リンク:」「返答」「配送:」の
//! ように、枠だけ日本語の固定文のまま出た（`main.rs` が `println!` / `eprintln!` へ直書きしていた）。
//!
//! **ビルドした `tako` をそのまま**叩く: projects / guide は GUI を通さない手元の処理なので一時の置き場
//! だけで、todo / sessions は GUI の代わりに決まった応答を返す受け口（偽の GUI）を立てて叩く
//! （表示は CLI が応答から組むので、応答の中身は固定でよい）。見ること:
//! - 英語表示: 出力に日本語（かな・漢字）が 1 字も無い（入力の値は ASCII だけにしてある）
//! - 日本語表示: #2365 より前の出力とバイト一致（`<SB>` は砂場の dir）。guide の一覧は同じ実行の
//!   `--json` の値を修正前の書式で並べたものと一致
//! - `todo show --json` と `sessions resume` の JSON は日英で同じ
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

/// 1 言語分の砂場: data（表示言語）・orchestrator の置き場・アカウントの設定 dir（user scope の MCP）・
/// CODEX_HOME・プロジェクトの作業 dir（.mcp.json / .codex/config.toml）
struct Sandbox {
    dir: ScratchDir,
}

impl Sandbox {
    fn new(lang: &str) -> Self {
        let sb = Self {
            // 偽の GUI の Unix domain socket が macOS の上限（104 バイト）に収まるよう短い名前にする
            dir: ScratchDir::new("i2365h"),
        };
        for d in [
            sb.path().join("data"),
            sb.path().join("disc"),
            sb.path().join("orch/profiles"),
            sb.path().join("acct"),
            sb.path().join("codex"),
            sb.path().join("home"),
            sb.path().join("tmp"),
            sb.path().join("w/alpha/.codex"),
            sb.path().join("w/beta"),
        ] {
            std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
        }
        // 書かないと system = 実行環境のロケールで決まり、CI と手元で答えが変わる
        sb.write(
            "data/settings.json",
            &format!("{{\"language\":\"{lang}\"}}"),
        );
        // user scope の MCP（隔離中のホームはプロセスごとの一時 dir なので、アカウントの設定 dir から読ませる）
        sb.write(
            "acct/.claude.json",
            r#"{"mcpServers":{"foo":{"command":"x"},"bar":{"command":"y"},"zed":{"command":"z"}}}"#,
        );
        let acct = sb.path().join("acct").display().to_string();
        sb.write(
            "orch/accounts.yaml",
            &format!("accounts:\n  main:\n    config_dir: {acct}\n"),
        );
        // codex の worker の構成（CODEX_HOME はプロファイルの env から引く）
        let codex = sb.path().join("codex").display().to_string();
        sb.write(
            "orch/profiles/default.yaml",
            &format!("env:\n  CODEX_HOME: {codex}\n"),
        );
        sb.write(
            "codex/config.toml",
            "[mcp_servers.baz]\ncommand = \"x\"\n[mcp_servers.qux]\ncommand = \"y\"\n",
        );
        // プロジェクト側の同名（止めない側）
        sb.write(
            "w/alpha/.mcp.json",
            r#"{"mcpServers":{"bar":{"command":"y"}}}"#,
        );
        sb.write(
            "w/alpha/.codex/config.toml",
            "[mcp_servers.qux]\ncommand = \"y\"\n",
        );
        sb
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.path().join(rel);
        std::fs::write(&path, text).unwrap_or_else(|e| panic!("{} を書ける: {e}", path.display()));
    }

    /// `TAKO_*` を外して隔離の置き場だけを渡した `tako <args>` の標準出力と標準エラー（`<SB>` に置換）
    fn run(&self, args: &[&str], gui: Option<&str>) -> (String, String) {
        let mut command = Command::new(tako());
        command
            .args(args)
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
            .env("TAKO_ORCHESTRATOR_DIR", self.path().join("orch"))
            .env("HOME", self.path().join("home"))
            // 隔離中の agent のホーム（`tako-agent-config-<pid>`）を砂場の中に作らせる
            .env("TMPDIR", self.path().join("tmp"));
        match gui {
            Some(socket) => command.env("TAKO_SOCKET", socket).env("TAKO_TOKEN", "t"),
            None => command.env("TAKO_SOCKET", self.path().join("none.sock")),
        };
        let out = command.output().expect("tako が終わる");
        assert!(
            out.status.success(),
            "{args:?} が失敗した（{}）: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        let root = self.path().display().to_string();
        let norm = |b: &[u8]| String::from_utf8_lossy(b).replace(&root, "<SB>");
        (norm(&out.stdout), norm(&out.stderr))
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

/// 1 手順: 名前・引数・#2365 より前の日本語表示の stdout / stderr（`<SB>` は砂場の dir）。
/// `None` は他モジュールの文（dispatch の警告）が混ざるので、`ja_lines` の行を含むことだけを見る
struct Step {
    name: &'static str,
    args: &'static [&'static str],
    /// 手順の前に砂場へ書くファイル（相対パス, 中身。`@SB@` は砂場の dir）
    before: &'static [(&'static str, &'static str)],
    stdout: Option<&'static str>,
    stderr: Option<&'static str>,
    ja_lines: &'static [&'static str],
}

const fn exact(
    name: &'static str,
    args: &'static [&'static str],
    stdout: &'static str,
    stderr: &'static str,
) -> Step {
    Step {
        name,
        args,
        before: &[],
        stdout: Some(stdout),
        stderr: Some(stderr),
        ja_lines: &[],
    }
}

const MCP_VIEW_JA: &str = "alpha の worker へ持ち込む user scope の MCP: （なし）
  今 spawn すると持ち込む: tako + （なし）
  止める（user scope）: foo, zed
  プロジェクトの .mcp.json と同名なので止めない: bar
  codex の worker で止める（config.toml）: baz
  codex: プロジェクトの .codex/config.toml と同名なので止めない: qux
";

const DEFAULT_PROFILE: &str = "env:\n  CODEX_HOME: @SB@/codex\n";

/// projects の一連の手順（同じ砂場で順に叩く。#2365 より前の出力は origin/main の実測）
fn project_steps() -> Vec<Step> {
    vec![
        exact(
            "list_empty",
            &["orchestrator", "projects", "list"],
            "",
            "登録済みプロジェクトはありません。\n\
             追加: tako orchestrator projects add --key <名前> --cwd <パス>\n",
        ),
        exact(
            "add_alpha",
            &[
                "orchestrator",
                "projects",
                "add",
                "--key",
                "alpha",
                "--cwd",
                "@SB@/w/alpha",
            ],
            "",
            "追加しました: alpha → <SB>/w/alpha\n\
             専用プロファイルを作りました: <SB>/orch/profiles/alpha.yaml（default から継承）\n",
        ),
        // 列の幅（16）を超えるキー
        exact(
            "add_long",
            &[
                "orchestrator",
                "projects",
                "add",
                "--key",
                "a-very-long-project-key-name",
                "--cwd",
                "@SB@/w/beta",
                "--description",
                "long description text",
            ],
            "",
            "追加しました: a-very-long-project-key-name → <SB>/w/beta\n\
             専用プロファイルを作りました: <SB>/orch/profiles/a-very-long-project-key-name.yaml\
             （default から継承）\n",
        ),
        // default を継承できない = 作らなかった理由は警告に載る（警告の文は dispatch の型の口）
        Step {
            name: "add_skipped",
            args: &[
                "orchestrator",
                "projects",
                "add",
                "--key",
                "beta",
                "--cwd",
                "@SB@/w/beta",
            ],
            before: &[("orch/profiles/default.yaml", "model: [\n")],
            stdout: Some(""),
            stderr: None,
            ja_lines: &["追加しました: beta → <SB>/w/beta"],
        },
        Step {
            name: "mcp_view",
            args: &["orchestrator", "projects", "mcp", "alpha"],
            before: &[("orch/profiles/default.yaml", DEFAULT_PROFILE)],
            stdout: Some(MCP_VIEW_JA),
            stderr: Some(""),
            ja_lines: &[],
        },
        // 警告の行（中身は dispatch の型の口）は行頭のラベルだけを見る
        Step {
            name: "mcp_add",
            args: &[
                "orchestrator",
                "projects",
                "mcp",
                "alpha",
                "add",
                "foo",
                "nosuch",
            ],
            before: &[],
            stdout: Some(
                "alpha の worker へ持ち込む user scope の MCP: foo, nosuch
  今 spawn すると持ち込む: tako + foo
  止める（user scope）: zed
  プロジェクトの .mcp.json と同名なので止めない: bar
  codex の worker で止める（config.toml）: baz
  codex: プロジェクトの .codex/config.toml と同名なので止めない: qux
",
            ),
            stderr: None,
            ja_lines: &["足しました: foo, nosuch"],
        },
        exact(
            "list",
            &["orchestrator", "projects", "list"],
            // 説明の無い行は区切りの 2 桁の空白で終わる（#2365 より前から）
            "a-very-long-project-key-name <SB>/w/beta  long description text\n\
             alpha            <SB>/w/alpha  \n\
             \x20                worker の MCP: tako + foo, nosuch\n\
             beta             <SB>/w/beta  \n",
            "",
        ),
        exact(
            "mcp_remove",
            &[
                "orchestrator",
                "projects",
                "mcp",
                "alpha",
                "remove",
                "foo",
                "nosuch",
            ],
            MCP_VIEW_JA,
            "外しました: foo, nosuch\n",
        ),
        exact(
            "remove",
            &["orchestrator", "projects", "remove", "--key", "beta"],
            "",
            "削除しました: beta\n",
        ),
    ]
}

/// 偽の GUI が返す `tako todo show` の応答（返答の時刻は年の違う固定の時刻 = 年月日つきの表記）
fn todo_result() -> Value {
    json!({
        "id": "u-1",
        "kind": "review",
        "status": "open",
        "title": "Review the PR",
        "body": "body text",
        "attachments": [
            {"path": "/w/alpha/file.txt", "exists": true},
            {"path": "/nonexistent/x.png", "exists": false}
        ],
        "copy_texts": [
            {"label": "msg", "text": "hello world, a long copy text body 0123456789 0123456789"}
        ],
        "links": ["https://example.com/pr/1"],
        "responses": [
            {"decision": "needs_change", "via": "cli", "at": TODO_AT, "comment": "line1\nline2 continued"},
            {"decision": "approve", "via": "pwa", "comment": ""}
        ],
        "delivery": {"state": "failed", "reason": "no destination", "pane": null}
    })
}

/// 返答の時刻（2025-01-01 00:00:00 UTC）
const TODO_AT: i64 = 1_735_689_600;

/// `tako todo show` の #2365 より前の日本語表示（返答の時刻は手元のタイムゾーンで組む）
fn todo_show_ja() -> String {
    let stamp = tako_core::user_task::response_stamp(
        TODO_AT,
        tako_core::user_task::unix_now(),
        tako_core::platform::clock::local_utc_offset(),
    );
    // 判断だけの返答（本文が空）は `: ` で終わる（#2365 より前から）
    format!(
        "u-1 [review/open] Review the PR\n\
         body text\n\
         \x20 添付: /w/alpha/file.txt\n\
         \x20 添付: /nonexistent/x.png（見つからない）\n\
         \x20 コピー用[msg]: hello world, a long copy text body 01234\n\
         \x20 リンク: https://example.com/pr/1\n\
         \x20 返答 {stamp} [cli] needs_change: line1\n\
         \x20     line2 continued\n\
         \x20 返答 - [pwa] approve: \n\
         \x20 配送: failed（no destination） pane:-\n"
    )
}

/// `tako orchestrator guide` の一覧を、同じ実行の `--json` の値から #2365 より前の書式で組む
fn guide_list_ja(json: &Value) -> String {
    let topics = json["topics"].as_array().expect("topics がある");
    let mut out = format!("引ける手順書: {} 件\n", json["count"].as_u64().unwrap_or(0));
    for g in topics {
        out.push_str(&format!(
            "  {:<16} {:>6} bytes / 約 {:>5} tok  {}\n",
            g["topic"].as_str().unwrap_or("?"),
            g["bytes"].as_u64().unwrap_or(0),
            g["est_tokens"].as_u64().unwrap_or(0),
            g["title"].as_str().unwrap_or("")
        ));
    }
    out.push_str(&format!(
        "\n{}\n",
        json["hint_ja"].as_str().expect("hint_ja がある")
    ));
    out
}

/// 1 言語分を叩いて (名前, stdout, stderr) を返す
fn run_all(lang: &str) -> Vec<(String, String, String)> {
    let sb = Sandbox::new(lang);
    let root = sb.path().display().to_string();
    let mut out = Vec::new();
    for step in project_steps() {
        for (rel, text) in step.before {
            sb.write(rel, &text.replace("@SB@", &root));
        }
        let args: Vec<String> = step.args.iter().map(|a| a.replace("@SB@", &root)).collect();
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let (o, e) = sb.run(&args, None);
        out.push((step.name.to_string(), o, e));
    }
    for (name, args) in [
        ("guide", &["orchestrator", "guide"][..]),
        ("guide_json", &["orchestrator", "guide", "--json"][..]),
    ] {
        let (o, e) = sb.run(args, None);
        out.push((name.to_string(), o, e));
    }
    let todo = sb.fake_gui("t", todo_result());
    for (name, args) in [
        ("todo_show", &["todo", "show", "u-1"][..]),
        ("todo_show_json", &["todo", "show", "u-1", "--json"][..]),
    ] {
        let (o, e) = sb.run(args, Some(&todo));
        out.push((name.to_string(), o, e));
    }
    let resumed = sb.fake_gui(
        "r",
        json!({
            "pane": 2,
            "session_id": "0123abcd-1111-2222-3333-444455556666",
            "cwd": "/w/alpha",
            "command": "claude --resume 0123abcd-1111-2222-3333-444455556666",
            "warnings": []
        }),
    );
    let (o, e) = sb.run(&["sessions", "resume", "0123abcd"], Some(&resumed));
    out.push(("sessions_resume".to_string(), o, e));
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
fn 人向けの表示は英語表示に日本語を含まず日本語表示は2365より前とバイト一致() {
    let en = run_all("en");
    let ja = run_all("ja");
    let mut failures = Vec::new();
    // 英語表示: JSON 以外の出力に日本語が無い（guide の --json は hint_ja を持つので外す）
    for (name, o, e) in &en {
        if name.ends_with("_json") {
            continue;
        }
        if let Some(c) = has_japanese(o).or_else(|| has_japanese(e)) {
            failures.push(format!("{name}: 英語表示に日本語（{c}）が混ざる:\n{o}{e}"));
        }
    }
    // 日本語表示: #2365 より前と同じ
    for step in project_steps() {
        let (o, e) = find(&ja, step.name);
        for (stream, got, want) in [("stdout", o, step.stdout), ("stderr", e, step.stderr)] {
            if let Some(want) = want {
                if got != want {
                    failures.push(format!(
                        "{}: 日本語表示の {stream} が #2365 より前と違う\n  want: {want:?}\n  got:  {got:?}",
                        step.name
                    ));
                }
            }
        }
        for line in step.ja_lines {
            if !o.lines().chain(e.lines()).any(|l| l == *line) {
                failures.push(format!(
                    "{}: 日本語表示に #2365 より前の行が無い\n  want: {line}\n  got:  {o}{e}",
                    step.name
                ));
            }
        }
    }
    let guide_json: Value =
        serde_json::from_str(find(&ja, "guide_json").0).expect("guide --json を読める");
    for (name, want) in [
        ("guide", guide_list_ja(&guide_json)),
        ("todo_show", todo_show_ja()),
    ] {
        let (o, _) = find(&ja, name);
        if o != want {
            failures.push(format!(
                "{name}: 日本語表示が #2365 より前と違う\n  want: {want:?}\n  got:  {o:?}"
            ));
        }
    }
    let (o, e) = find(&ja, "sessions_resume");
    let want_err = "復元しました: ペイン 2（session 0123abcd…, cwd /w/alpha）\n";
    if e != want_err || !o.starts_with("{\"pane\":2,") {
        failures.push(format!(
            "sessions_resume: 日本語表示が #2365 より前と違う\n  want: {want_err:?}\n  got:  {o:?} / {e:?}"
        ));
    }
    // JSON の出力（とそれだけを出す sessions resume の stdout）は日英で同じ。guide の --json は
    // 表示言語の hint と本文（platform の topic）を持つので外す（修正前後の一致は A/B で見る）
    for name in ["todo_show_json", "sessions_resume"] {
        let (a, b) = (find(&ja, name).0, find(&en, name).0);
        if a != b {
            failures.push(format!(
                "{name}: JSON の出力が日英で違う\n  ja: {a}\n  en: {b}"
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

/// 英語表示の枠の見本（字下げ・桁揃えが崩れない。guide の行は英語の行どうしで列が揃う）
#[test]
fn 英語表示の枠は字下げと列を保つ() {
    let en = run_all("en");
    let (o, _) = find(&en, "todo_show");
    let lines: Vec<&str> = o.lines().collect();
    assert_eq!(lines[2], "  Attachment: /w/alpha/file.txt", "{o}");
    assert_eq!(
        lines[3], "  Attachment: /nonexistent/x.png (not found)",
        "{o}"
    );
    assert_eq!(
        lines[7], "      line2 continued",
        "続きの字下げは日本語と同じ: {o}"
    );
    assert_eq!(
        lines[9], "  Delivery: failed (no destination) pane:-",
        "{o}"
    );
    let (o, _) = find(&en, "guide");
    let rows: Vec<&str> = o.lines().filter(|l| l.contains(" bytes / ")).collect();
    assert!(!rows.is_empty(), "{o}");
    let col = |l: &str, pat: &str| l.find(pat).unwrap_or_else(|| panic!("{pat} が無い: {l}"));
    let (bytes_col, tok_col) = (col(rows[0], " bytes / "), col(rows[0], " tok  "));
    for row in &rows {
        // topic が 16 桁に収まる行どうしは「bytes」と「tok」の位置が揃う
        if row.len() > 2 && row[2..].split(' ').next().is_some_and(|t| t.len() <= 16) {
            assert_eq!(col(row, " bytes / "), bytes_col, "{o}");
            assert_eq!(col(row, " tok  "), tok_col, "{o}");
        }
        assert!(row.contains(" bytes / ") && row.contains('~'), "{row}");
    }
    let (o, _) = find(&en, "list");
    assert!(
        o.contains("\n                 worker MCP: tako + foo, nosuch\n"),
        "MCP の行は 16 桁 + 1 の字下げ: {o}"
    );
}
