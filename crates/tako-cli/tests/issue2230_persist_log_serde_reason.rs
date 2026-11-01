//! 型が違う値を入れた設定ファイルの値が persist.log・CLI の行・master の system prompt に
//! 載らない（Issue #2230）
//!
//! 修正前は、読めない理由を serde_json / serde_yaml の**素の文言**で組んでいた。素の文言は型が
//! 違うと `invalid type: string "<ファイルの値>"` のように値（YAML はキーの道筋も）を引用するので、
//! settings.json の申告・`tako migrate` の記録（`移行できず:`）・`projects add` / `tako master` の
//! 警告（#1825 / #2227）の行にファイルの中身がそのまま書かれていた（実測）。いまは読む口が
//! `tako_core::load_issue::{json_error_reason, yaml_error_reason}` で種類と行・列だけを組む。
//!
//! 本物の CLI を隔離 data dir（`TAKO_DATA_DIR` = 使い捨て dir・`HOME` も使い捨て）で叩き、
//! 値に目印の固有文字列を入れて**出た回数を数える**。A/B は `TAKO_2230_LEGACY=1`
//! （修正前の素の文言を同じバイナリで再現する）。`tako master` は GUI が無いので起動先の
//! ペインを確保できずに終わる（その手前の persist.log と system prompt の書き出しまでは進む）。
//! claude の所在確認は `TAKO_983_LEGACY=1` で外す（本物の claude を探さない・起こさない）。

use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::Value;
use tako_core::test_residue::ScratchDir;

const KEY: &str = "p2230";
const RULE: &str = "PERSONAL_RULE_2230";

struct Env {
    dir: ScratchDir,
}

/// 1 回の CLI の結果
struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Env {
    fn new() -> Self {
        let dir = ScratchDir::new("issue2230");
        assert!(
            dir.path().starts_with(std::env::temp_dir()),
            "一時 dir の外に書こうとしている: {}",
            dir.path().display()
        );
        let env = Self { dir };
        for d in [env.profiles(), env.proj(), env.home()] {
            std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
        }
        env
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn profiles(&self) -> PathBuf {
        self.path("orchestrator/profiles")
    }

    fn proj(&self) -> PathBuf {
        self.path("proj")
    }

    fn home(&self) -> PathBuf {
        self.path("home")
    }

    fn put(&self, rel: &str, body: &[u8]) {
        let path = self.path(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("置き場を作れる");
        }
        std::fs::write(&path, body).unwrap_or_else(|e| panic!("{} を置ける: {e}", path.display()));
    }

    fn persist_log(&self) -> String {
        std::fs::read_to_string(self.path("persist.log")).unwrap_or_default()
    }

    fn prompt_file(&self) -> String {
        std::fs::read_to_string(self.path("orchestrator/_system_prompt_default.md"))
            .unwrap_or_default()
    }

    /// 隔離した tako（本番の GUI にも data dir にも HOME にも触らない）
    fn tako(&self, args: &[&str], legacy: bool) -> Out {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_tako"));
        cmd.args(args)
            .current_dir(self.dir.path())
            .env_remove("TAKO_SOCKET")
            .env_remove("TAKO_PANE_ID")
            .env_remove("TAKO_TOKEN")
            .env_remove("TAKO_ORCHESTRATOR_DIR")
            .env_remove("TAKO_1825_LEGACY")
            .env_remove("TAKO_2227_LEGACY")
            .env_remove("TAKO_1453_LEGACY")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env("HOME", self.home())
            .env("TAKO_ISOLATED", "1")
            .env("TAKO_DATA_DIR", self.dir.path())
            .env("TAKO_983_LEGACY", "1")
            .stdin(Stdio::null());
        if legacy {
            cmd.env("TAKO_2230_LEGACY", "1");
        } else {
            cmd.env_remove("TAKO_2230_LEGACY");
        }
        let out = cmd.output().expect("tako を起こせない");
        Out {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    fn migrate(&self, action: &str, legacy: bool) -> Value {
        let out = self.tako(&["migrate", action], legacy);
        assert_eq!(out.code, 0, "tako migrate {action} が失敗: {}", out.stderr);
        serde_json::from_str(&out.stdout)
            .unwrap_or_else(|e| panic!("migrate の応答が JSON でない（{e}）:\n{}", out.stdout))
    }

    fn add_project(&self, legacy: bool) -> Out {
        let proj = self.proj();
        let out = self.tako(
            &[
                "orchestrator",
                "projects",
                "add",
                "--key",
                KEY,
                "--cwd",
                proj.to_str().expect("UTF-8 のパス"),
            ],
            legacy,
        );
        assert_eq!(out.code, 0, "登録そのものは成功する: {}", out.stderr);
        out
    }

    fn master(&self, legacy: bool) -> Out {
        let out = self.tako(&["master"], legacy);
        assert_ne!(out.code, 0, "隔離のはずが起動に成功した:\n{}", out.stderr);
        out
    }
}

/// 応答の files から `schema` の 1 件の reason
fn reason_of<'a>(report: &'a Value, schema: &str) -> &'a str {
    report["files"]
        .as_array()
        .and_then(|files| files.iter().find(|f| f["schema"] == schema))
        .and_then(|f| f["reason"].as_str())
        .unwrap_or_else(|| panic!("{schema} の reason が無い: {report:#}"))
}

/// persist.log に `needle` を含む行があり、その行の理由が中身なしの形であること
fn assert_logged(log: &str, needle: &str, shape: &str) {
    let line = log
        .lines()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("persist.log に `{needle}` の行が無い:\n{log}"));
    assert!(
        line.contains(shape),
        "理由が「種類 + 行・列」の形でない（`{shape}` が無い）: {line}"
    );
}

/// settings.json（JSON）: settings の申告と移行の記録の 2 経路
#[test]
fn settings_jsonの型違いの値をpersist_logへ載せない() {
    const MARK: &str = "MARK_SETTINGS_2230";
    let body = format!("{{\"auto_rename\": \"{MARK}\"}}\n");
    for legacy in [true, false] {
        let env = Env::new();
        env.put("settings.json", body.as_bytes());
        let report = env.migrate("run", legacy);
        // settings の申告は settings.json を読む CLI が出す（`tako migrate` は表示言語を
        // ファイルに触らない読みで引くので申告しない = #2238）
        let out = env.tako(&["telemetry", "status"], legacy);
        assert_eq!(out.code, 0, "tako telemetry status が失敗: {}", out.stderr);
        let log = env.persist_log();
        let screen = reason_of(&report, "settings");
        if legacy {
            assert!(
                log.matches(MARK).count() >= 2,
                "前提: 修正前は申告と記録の両方に値が載る:\n{log}"
            );
            continue;
        }
        assert_eq!(
            log.matches(MARK).count(),
            0,
            "値が persist.log に載った:\n{log}"
        );
        let shape = "JSON の形が期待と違う（1 行 ";
        assert_logged(&log, "settings.json を解釈できないので既定値で動く", shape);
        assert_logged(&log, "移行できず: settings ", shape);
        assert!(
            screen.starts_with(shape) && !screen.contains(MARK),
            "migrate の応答の理由: {screen}"
        );
    }
}

/// profiles/default.yaml（YAML）: `projects add`・`tako master`・移行の記録の 3 経路と、
/// 画面（CLI の `警告:` 行）・master の system prompt
#[test]
fn profileの型違いの値をpersist_logとpromptへ載せない() {
    const MARK: &str = "MARK_PROFILE_2230";
    let body = format!(
        "model: claude-test-model\nprompt_blocks:\n  append: \"{RULE}\"\nbypass_sandbox: {MARK}\n"
    );
    for legacy in [true, false] {
        let env = Env::new();
        env.put("orchestrator/profiles/default.yaml", body.as_bytes());
        // 警告の行と system prompt の理由は表示言語に従う（#2365）。このテストは日本語の位置の文を
        // 見るので日本語表示で回す（書かないと英語の CI では英語の理由になる）
        env.put("settings.json", b"{\"language\":\"ja\"}");
        let added = env.add_project(legacy);
        let master = env.master(legacy);
        let report = env.migrate("run", legacy);
        let log = env.persist_log();
        let prompt = env.prompt_file();
        let screens = [added.stderr.as_str(), master.stderr.as_str(), &prompt];
        if legacy {
            assert!(
                log.matches(MARK).count() >= 3,
                "前提: 修正前は 3 経路すべてに値が載る:\n{log}"
            );
            assert!(
                screens.iter().all(|s| s.contains(MARK)),
                "前提: 修正前は警告と system prompt にも値が載る"
            );
            continue;
        }
        for (what, text) in [("persist.log", log.as_str())].into_iter().chain(
            ["projects add の画面", "tako master の画面", "system prompt"]
                .into_iter()
                .zip(screens),
        ) {
            assert_eq!(
                text.matches(MARK).count(),
                0,
                "値が {what} に載った:\n{text}"
            );
            assert_eq!(
                text.matches(RULE).count(),
                0,
                "個人ルールが {what} に載った"
            );
        }
        let shape = "YAML の形が期待と違う（4 行 17 列）";
        assert_logged(&log, &format!("[projects add] {KEY}:"), shape);
        assert_logged(&log, "[master] default:", shape);
        assert_logged(&log, "移行できず: profiles ", shape);
        assert!(
            added.stderr.contains("警告:") && added.stderr.contains(shape),
            "projects add の警告に位置が無い:\n{}",
            added.stderr
        );
        assert!(
            prompt.contains(shape),
            "system prompt に位置が無い:\n{prompt}"
        );
        let screen = reason_of(&report, "profiles");
        assert!(screen.starts_with(shape), "migrate の応答の理由: {screen}");
    }
}

/// projects.yaml（移行の対象・YAML）: 値とキー（プロジェクト名）の両方
#[test]
fn projects_yamlの型違いの値とキーをpersist_logへ載せない() {
    const VALUE: &str = "MARK_PROJECTS_2230";
    const KEYMARK: &str = "MARK_KEY_2230";
    for (body, mark, shape) in [
        (
            format!("projects: {VALUE}\n"),
            VALUE,
            "YAML の形が期待と違う（1 行 11 列）",
        ),
        (
            format!("projects:\n  {KEYMARK}:\n    cwd: [1]\n"),
            KEYMARK,
            "YAML の形が期待と違う（3 行 10 列）",
        ),
    ] {
        for legacy in [true, false] {
            let env = Env::new();
            env.put("orchestrator/projects.yaml", body.as_bytes());
            let report = env.migrate("run", legacy);
            let log = env.persist_log();
            if legacy {
                assert!(
                    log.contains(mark),
                    "前提: 修正前は値・キーが記録に載る:\n{log}"
                );
                continue;
            }
            assert_eq!(
                log.matches(mark).count(),
                0,
                "{mark} が persist.log に載った:\n{log}"
            );
            assert_logged(&log, "移行できず: projects ", shape);
            assert_eq!(reason_of(&report, "projects"), shape);
        }
    }
}

/// 型違いでない誤り（構文・空・UTF-8 でない）も中身を載せず、種類で見分けられる
#[test]
fn 構文の誤り_空_utf8でない中身も中身を載せない() {
    const MARK: &str = "MARK_OTHER_2230";
    let cases: [(&str, &str, Vec<u8>, &str); 5] = [
        (
            "settings.json",
            "settings",
            format!("{{\"auto_rename\": {MARK}}}\n").into_bytes(),
            "JSON の構文が壊れている（1 行 17 列）",
        ),
        (
            "settings.json",
            "settings",
            Vec::new(),
            "JSON が途中で切れている（1 行 0 列）",
        ),
        (
            "orchestrator/profiles/default.yaml",
            "profiles",
            format!("model: m\nprompt_blocks:\n  append: \"{MARK}\n").into_bytes(),
            "YAML の構文が壊れている（4 行 1 列）",
        ),
        // 空の projects.yaml は「登録なし」として読める（serde の default）ので、値の無い文書（`~`）で
        (
            "orchestrator/projects.yaml",
            "projects",
            b"~\n".to_vec(),
            "YAML が空（1 行 1 列）",
        ),
        (
            "settings.json",
            "settings",
            [
                format!("{{\"auto_rename\": \"{MARK}").as_bytes(),
                b"\xff\xfe\"}\n",
            ]
            .concat(),
            "読み取りに失敗",
        ),
    ];
    for (rel, schema, body, shape) in cases {
        let env = Env::new();
        env.put(rel, &body);
        let report = env.migrate("run", false);
        let log = env.persist_log();
        assert_eq!(
            log.matches(MARK).count(),
            0,
            "{rel}: 値が persist.log に載った:\n{log}"
        );
        let reason = reason_of(&report, schema);
        assert!(
            reason.starts_with(shape) && !reason.contains(MARK),
            "{rel} {body:?}: 理由 `{reason}`（期待 `{shape}`）"
        );
        assert!(
            log.lines()
                .any(|l| l.contains(&format!(" {schema} ")) && l.contains(shape)),
            "{rel}: persist.log に理由の行が無い:\n{log}"
        );
    }
}

/// 回帰なし: 読めるファイルは理由を出さず、`tako recover` の一覧も中身なしの形のまま（#1824）
#[test]
fn migrateとrecoverの出力は回帰しない() {
    let env = Env::new();
    env.put("settings.json", b"{\"auto_rename\": true}\n");
    env.put(
        "orchestrator/profiles/default.yaml",
        format!("model: m\nprompt_blocks:\n  append: \"{RULE}\"\nbypass_sandbox: false\n")
            .as_bytes(),
    );
    let status = env.migrate("status", false);
    assert_eq!(
        status["needs_attention"], 0,
        "読めるファイルに理由が出た: {status:#}"
    );
    let run = env.migrate("run", false);
    assert_eq!(run["needs_attention"], 0, "{run:#}");
    assert!(
        !env.persist_log().contains("移行できず"),
        "読めるファイルを記録した:\n{}",
        env.persist_log()
    );

    const MARK: &str = "MARK_LAYOUT_2230";
    env.put(
        "layout.json",
        format!("{{\"tabs\": \"{MARK}\"}}\n").as_bytes(),
    );
    let out = env.tako(&["recover"], false);
    let all = format!("{}{}", out.stdout, out.stderr);
    assert!(
        all.contains("レイアウトファイルを解釈できない（破損）: JSON の形が期待と違う（1 行 "),
        "recover の一覧の形が変わった:\n{all}"
    );
    assert!(!all.contains(MARK), "recover の一覧に値が載った:\n{all}");
}
