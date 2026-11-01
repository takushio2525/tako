//! 壊れた `profiles/default.yaml` で `tako orchestrator guide local-rules` が「個人ルールは無い」を
//! 返さない（Issue #2256）
//!
//! 修正前（実測）: `load_profile_of(..).unwrap_or_else(|_| Profile::default())` で読めない default.yaml を
//! 握りつぶし、`local-rules` は「この環境に on-demand の個人ルールは無い（全部 prompt に載っている）」を
//! 返していた。master は個人ルールが**存在しない**と受け取って動く（影響がいちばん大きい経路）。
//!
//! いまは読むだけ（退避しない）で既定値から組み立てるが、本文は「読めない（推測で埋めず利用者へ
//! 伝えよ）」へ差し替え、理由を `警告:` 行（stderr）と `--json` の `warnings` へ出す。
//!
//! 本物の CLI を隔離 data dir（`TAKO_DATA_DIR` = 使い捨て dir・`HOME` も使い捨て）で叩く。guide は
//! GUI も IPC も要らないローカル処理なので終了コードは 0。A/B は `TAKO_2256_LEGACY=1`。

use std::path::PathBuf;
use std::process::{Command, Stdio};

use tako_core::test_residue::ScratchDir;

const GOOD: &str =
    "prompt_blocks:\n  append: \"always\\n<!-- tako:on-demand -->\\n## RULE_2256\\nbody\\n\"\n";
const BROKEN: &str = "prompt_blocks:\n  append: \"RULE_2256\n  prepend: [\n";
const NONE: &str = "no on-demand local rules";
const UNREADABLE: &str = "could NOT be read";

struct Env {
    dir: ScratchDir,
}

impl Env {
    fn new(default_yaml: Option<&str>) -> Self {
        let dir = ScratchDir::new("issue2256-guide");
        assert!(
            dir.path().starts_with(std::env::temp_dir()),
            "一時 dir の外に書こうとしている: {}",
            dir.path().display()
        );
        let env = Self { dir };
        for d in [env.profiles(), env.home()] {
            std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
        }
        if let Some(body) = default_yaml {
            std::fs::write(env.default_yaml(), body).expect("default.yaml を置ける");
        }
        // 警告の行（見出しと理由の文）は表示言語に従う（#2365）。書かないと system = 実行環境の
        // ロケールで決まり、英語の CI では `warning:` と英語の理由になる。このテストは日本語の文を見る
        // （英語表示は `issue2365_profile_unreadable_cli_lang.rs` が見る）
        std::fs::write(
            env.dir.path().join("settings.json"),
            "{\"language\":\"ja\"}",
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

    fn default_yaml(&self) -> PathBuf {
        self.profiles().join("default.yaml")
    }

    /// 隔離した `tako orchestrator guide <args>`（stdout, stderr）
    fn guide(&self, args: &[&str], legacy: bool) -> (String, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_tako"));
        cmd.args(["orchestrator", "guide"])
            .args(args)
            .current_dir(self.dir.path())
            .env_remove("TAKO_SOCKET")
            .env_remove("TAKO_PANE_ID")
            .env_remove("TAKO_TOKEN")
            .env_remove("TAKO_ORCHESTRATOR_DIR")
            .env_remove("TAKO_ORCHESTRATOR_ROLE")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env("HOME", self.home())
            .env("TAKO_ISOLATED", "1")
            .env("TAKO_DATA_DIR", self.dir.path())
            .stdin(Stdio::null());
        if legacy {
            cmd.env("TAKO_2256_LEGACY", "1");
        } else {
            cmd.env_remove("TAKO_2256_LEGACY");
        }
        let out = cmd.output().expect("tako を起こせない");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(out.status.code(), Some(0), "guide は止まらない:\n{stderr}");
        (stdout, stderr)
    }
}

#[test]
fn issue2256_壊れたdefaultでguide_local_rulesは読めないと返す() {
    let env = Env::new(Some(BROKEN));
    let path = env.default_yaml().display().to_string();
    let (out, err) = env.guide(&["local-rules"], false);
    assert!(
        out.contains(UNREADABLE) && out.contains(&path),
        "local-rules が読めない理由を返していない:\n{out}"
    );
    assert!(!out.contains(NONE), "空のルールを正常として返した:\n{out}");
    assert!(
        err.lines()
            .any(|l| l.starts_with("警告: ") && l.contains(&path)),
        "`警告:` 行が無い:\n{err}"
    );
    // --json は warnings に同じ理由
    let (json, _) = env.guide(&["local-rules", "--json"], false);
    let v: serde_json::Value = serde_json::from_str(&json).expect("JSON");
    assert!(
        v["warnings"][0]
            .as_str()
            .unwrap_or_default()
            .contains(&path),
        "{v:#}"
    );
    // 読むだけ: 退避しない・default.yaml に触らない
    assert!(!env.profiles().join("default.yaml.unreadable.bak").exists());
    assert_eq!(std::fs::read_to_string(env.default_yaml()).unwrap(), BROKEN);

    // A/B: 修正前は「無い」を返して黙る
    let (out, err) = env.guide(&["local-rules"], true);
    assert!(out.contains(NONE), "修正前の再現になっていない:\n{out}");
    assert!(!err.contains("警告:"), "{err}");
}

#[test]
fn issue2256_読める_無いdefaultのguideは黙る() {
    let env = Env::new(Some(GOOD));
    let (out, err) = env.guide(&["local-rules"], false);
    assert!(
        out.contains("RULE_2256"),
        "読める個人ルールが返らない:\n{out}"
    );
    assert!(!err.contains("警告:"), "{err}");

    let env = Env::new(None);
    let (out, err) = env.guide(&["local-rules"], false);
    assert!(out.contains(NONE), "{out}");
    assert!(!err.contains("警告:"), "{err}");
    let (list, err) = env.guide(&[], false);
    assert!(list.contains("local-rules"), "{list}");
    assert!(!err.contains("警告:"), "{err}");
}
