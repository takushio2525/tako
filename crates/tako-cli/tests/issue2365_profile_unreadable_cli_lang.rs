//! 実経路（CLI の手元）: 読めないプロファイルの理由（#2256 の系統）が表示言語に従う（Issue #2365 の続き）。
//!
//! 修正前（実測。英語表示 = `settings.json` の `language: en`）:
//!
//! - `tako master`: `warning: <path> を読めない（YAML の構文が壊れている（5 行 1 列））。…組み込み既定で
//!   master を立てる…`（見出しだけ英語）
//! - `tako master -<key>`（登録済み・専用プロファイルが無い・継承元を読めない）:
//!   `error: プロファイル 'k' が見つからない: …。…default を継承できないので…`
//! - `tako orchestrator guide local-rules` / `projects add`: `警告: <日本語の理由>`
//! - `tako setup --check` / `check-health`（profiles/ を読めない）:
//!   `[info] profiles: check failed (プロファイルディレクトリの読み取りに失敗: Permission denied …)`
//!
//! 理由の文が persist.log（日本語の診断ログ）と共有する日本語の `String` だったため。いまは型で持ち、
//! 画面へ出す行は表示言語・persist.log は日本語で組む。ここでは同じ砂場を日英で回し、
//!
//! - 英語表示: 理由の行に日本語（かな・漢字・全角）が 1 字も無い
//! - 日本語表示: 理由の行が #2365 より前と同じ形（`警告: <path> を読めない（…）。…`）
//! - persist.log の行はどちらの表示言語でも日本語
//!
//! を見る。claude の所在確認（ログインシェル）は `TAKO_983_LEGACY=1` で外す。GUI には繋がない
//! （`TAKO_ISOLATED=1` + 一時の `TAKO_DATA_DIR`。`tako master` はタブを確保できずに止まるが、
//! 読み込みの警告の行はその前に出る）。
//!
//! unix だけ: 読めないディレクトリ（パーミッション 000）が unix の前提
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tako_core::test_residue::ScratchDir;

/// projects.yaml に登録だけあり、専用プロファイルが無いプロジェクト（#1825 で作れない）
const KEY: &str = "k2365";
const BROKEN_YAML: &str = "effort: max\nprompt_blocks:\n  append: \"RULE\n  prepend: [\n";

struct Env {
    dir: ScratchDir,
}

impl Env {
    fn new(lang: &str) -> Self {
        let dir = ScratchDir::new("issue2365-cli-pu");
        assert!(
            dir.path().starts_with(std::env::temp_dir()),
            "一時 dir の外に書こうとしている: {}",
            dir.path().display()
        );
        let env = Self { dir };
        for d in [env.profiles(), env.home(), env.proj()] {
            std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
        }
        std::fs::write(env.default_yaml(), BROKEN_YAML).expect("default.yaml を置ける");
        std::fs::write(
            env.dir.path().join("orchestrator").join("projects.yaml"),
            format!("projects:\n  {KEY}:\n    cwd: '{}'\n", env.proj().display()),
        )
        .expect("projects.yaml を置ける");
        std::fs::write(
            env.dir.path().join("settings.json"),
            format!("{{\"language\":\"{lang}\"}}"),
        )
        .expect("settings.json を置ける");
        env
    }

    fn profiles(&self) -> PathBuf {
        self.dir.path().join("orchestrator").join("profiles")
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    fn proj(&self) -> PathBuf {
        self.dir.path().join("proj")
    }

    fn default_yaml(&self) -> PathBuf {
        self.profiles().join("default.yaml")
    }

    fn persist_log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("persist.log")).unwrap_or_default()
    }

    /// 隔離した tako（本番の GUI にも data dir にも HOME にも触らない）。stdout + stderr を返す
    fn tako(&self, args: &[&str]) -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_tako"))
            .args(args)
            .current_dir(self.dir.path())
            .env_remove("TAKO_SOCKET")
            .env_remove("TAKO_PANE_ID")
            .env_remove("TAKO_TOKEN")
            .env_remove("TAKO_ORCHESTRATOR_DIR")
            .env_remove("TAKO_ORCHESTRATOR_ROLE")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("TAKO_LANG")
            .env_remove("TAKO_2227_LEGACY")
            .env_remove("TAKO_2256_LEGACY")
            .env_remove("TAKO_1825_LEGACY")
            .env_remove("TAKO_1453_LEGACY")
            .env("HOME", self.home())
            .env("TAKO_ISOLATED", "1")
            .env("TAKO_DATA_DIR", self.dir.path())
            .env("TAKO_983_LEGACY", "1")
            .stdin(Stdio::null())
            .output()
            .expect("tako を起こせない");
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    }
}

impl Drop for Env {
    /// 読めなくした所を戻す（戻さないと一時 dir を消せない）
    fn drop(&mut self) {
        set_mode(&self.profiles(), 0o755);
    }
}

fn set_mode(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .unwrap_or_else(|e| panic!("{} の権限を変えられる: {e}", path.display()));
}

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

/// 出力のうち `needle`（default.yaml のパス等）を含む行
fn line_with(out: &str, needle: &str) -> Option<String> {
    out.lines().find(|l| l.contains(needle)).map(str::to_string)
}

/// 1 行の検査（英語は日本語なし・日本語は旧文の目印を含む）
fn check(failures: &mut Vec<String>, lang: &str, label: &str, line: Option<String>, ja: &[&str]) {
    let Some(line) = line else {
        failures.push(format!("{lang} {label}: 理由の行が出ていない"));
        return;
    };
    if lang == "en" {
        if let Some(c) = has_japanese(&line) {
            failures.push(format!("{lang} {label}: 英語表示に日本語（{c}）: {line}"));
        }
    } else if let Some(missing) = ja.iter().find(|m| !line.contains(**m)) {
        failures.push(format!(
            "{lang} {label}: 日本語表示が旧文の形でない（「{missing}」が無い）: {line}"
        ));
    }
}

fn run(lang: &str, failures: &mut Vec<String>) {
    // ① tako master（読めない default = 組み込み既定で立てる警告の行）
    let env = Env::new(lang);
    let path = env.default_yaml().display().to_string();
    let out = env.tako(&["master"]);
    check(
        failures,
        lang,
        "master",
        line_with(&out, &path),
        &["警告: ", "を読めない", "組み込み既定で master を立てる"],
    );
    let log = env.persist_log();
    if !log
        .lines()
        .any(|l| l.contains("[master] default:") && l.contains("を読めない"))
    {
        failures.push(format!(
            "{lang} master: persist.log の行が日本語でない:\n{log}"
        ));
    }
    drop(env);

    // ② tako master -<key>（専用プロファイルが無く、継承元を読めない = 理由と直し方で止まる）
    let env = Env::new(lang);
    let path = env.default_yaml().display().to_string();
    let out = env.tako(&["master", &format!("-{KEY}")]);
    check(
        failures,
        lang,
        "master -<key>",
        line_with(&out, &path),
        &["が見つからない", "を読めない", "default を継承できない"],
    );
    drop(env);

    // ③ tako orchestrator guide local-rules（警告の行と本文の Detail）
    let env = Env::new(lang);
    let path = env.default_yaml().display().to_string();
    let out = env.tako(&["orchestrator", "guide", "local-rules"]);
    let warning = out
        .lines()
        .find(|l| l.contains(&path) && !l.starts_with("Detail: "))
        .map(str::to_string);
    check(
        failures,
        lang,
        "guide",
        warning,
        &["警告: ", "を読めない", "local-rules"],
    );
    let detail = out
        .lines()
        .find(|l| l.starts_with("Detail: "))
        .map(str::to_string);
    check(
        failures,
        lang,
        "guide の Detail",
        detail,
        &["を読めない", "local-rules"],
    );
    drop(env);

    // ④ tako orchestrator projects add（専用プロファイルを作らなかった理由の警告の行）
    let env = Env::new(lang);
    let path = env.default_yaml().display().to_string();
    let proj = env.proj().display().to_string();
    let out = env.tako(&[
        "orchestrator",
        "projects",
        "add",
        "--key",
        "a2365",
        "--cwd",
        &proj,
    ]);
    check(
        failures,
        lang,
        "projects add",
        line_with(&out, &path),
        &["警告: ", "を読めない", "default を継承できない"],
    );
    if !env
        .persist_log()
        .lines()
        .any(|l| l.contains("[projects add] a2365:") && l.contains("を読めない"))
    {
        failures.push(format!(
            "{lang} projects add: persist.log の行が日本語でない:\n{}",
            env.persist_log()
        ));
    }
    drop(env);

    // ⑤ tako setup --check / check-health（profiles/ を読めない）
    for args in [&["setup", "--check"][..], &["check-health"][..]] {
        let env = Env::new(lang);
        set_mode(&env.profiles(), 0o000);
        let out = env.tako(args);
        set_mode(&env.profiles(), 0o755);
        let marker = if lang == "en" {
            "profiles: check failed"
        } else {
            "プロファイル: 確認失敗"
        };
        check(
            failures,
            lang,
            &args.join(" "),
            line_with(&out, marker),
            &["プロファイルディレクトリの読み取りに失敗"],
        );
    }
}

#[test]
fn issue2365_読めないプロファイルの理由のcliの行は表示言語に従う() {
    let mut failures = Vec::new();
    for lang in ["en", "ja"] {
        run(lang, &mut failures);
    }
    assert!(
        failures.is_empty(),
        "読めないプロファイルの理由の行が表示言語に従っていない（{} 件）:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
