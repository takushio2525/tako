//! 壊れた `solo-profiles/default.yaml` で `tako solo` が黙って組み込み既定で立たない（Issue #2233）
//!
//! 修正前（実測）: `Err(_) if profile_name == "default" => solo_default_profile()` で読めない
//! default.yaml を握りつぶし、何も言わずに個人ルール・モデルの抜けた solo を立てに行っていた
//! （`警告:` 行も persist.log の行も system prompt の注意も無く、中身の退避もしない）。
//!
//! いまは `tako master`（#2227）と同じ形: 立てるが `警告:` 行・persist.log の `[solo] default:` の
//! 1 行・solo 本人の system prompt の 3 か所へ理由と直し方を出し、中身を `.unreadable.bak` へ退避する。
//!
//! 本物の CLI を隔離 data dir（`TAKO_DATA_DIR` = 使い捨て dir・`HOME` も使い捨て）で叩く。
//! GUI は無いので起動は「起動先ペインを確保できない」で終わるが、その手前の system prompt の
//! 書き出しまで進んだこと（= 立てに行ったこと）を prompt ファイルで見る（#2227 のテストと同じ作法）。
//! claude の所在確認（ログインシェル）は `TAKO_983_LEGACY=1` で外す。
//! A/B は `TAKO_2233_LEGACY=1`（修正前を同じバイナリで再現する）。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tako_core::test_residue::ScratchDir;

const RULE: &str = "PERSONAL_RULE_2233";
const GOOD: &str = "effort: low\nmodel: claude-test-model\nprompt_blocks:\n  append: \"PERSONAL_RULE_2233\"\nbypass_sandbox: false\n";
const BROKEN_YAML: &str =
    "effort: low\nmodel: claude-test-model\nprompt_blocks:\n  append: \"PERSONAL_RULE_2233\n  prepend: [\n";
const WRONG_TYPE: &str =
    "effort: low\nmodel: claude-test-model\nprompt_blocks:\n  append: [PERSONAL_RULE_2233]\n";

struct Env {
    dir: ScratchDir,
}

impl Env {
    /// `default_yaml` が `None` = solo の default.yaml が無い（ゼロコンフィグの初回）
    fn new(default_yaml: Option<&str>) -> Self {
        let dir = ScratchDir::new("issue2233-solo");
        assert!(
            dir.path().starts_with(std::env::temp_dir()),
            "一時 dir の外に書こうとしている: {}",
            dir.path().display()
        );
        let env = Self { dir };
        for d in [env.solo_profiles(), env.home()] {
            std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
        }
        if let Some(body) = default_yaml {
            std::fs::write(env.default_yaml(), body).expect("default.yaml を置ける");
        }
        // `警告:` の見出しは表示言語に従う（#2365）。書かないと system = 実行環境のロケールで決まり、
        // 英語の CI では `warning:` になる（中身の理由の文 = #2256 の系統は日本語のまま）
        std::fs::write(
            env.dir.path().join("settings.json"),
            "{\"language\":\"ja\"}",
        )
        .expect("settings.json を置ける");
        env
    }

    fn solo_profiles(&self) -> PathBuf {
        self.dir.path().join("orchestrator").join("solo-profiles")
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    fn default_yaml(&self) -> PathBuf {
        self.solo_profiles().join("default.yaml")
    }

    fn quarantine(&self) -> PathBuf {
        self.solo_profiles().join("default.yaml.unreadable.bak")
    }

    fn prompt_file(&self, profile: &str) -> PathBuf {
        self.dir
            .path()
            .join("orchestrator")
            .join(format!("_solo_system_prompt_{profile}.md"))
    }

    fn persist_log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("persist.log")).unwrap_or_default()
    }

    /// 隔離した `tako solo`（本番の GUI にも data dir にも HOME にも触らない）
    fn solo(&self, profile_arg: Option<&str>, legacy: bool) -> String {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_tako"));
        cmd.arg("solo")
            .args(profile_arg)
            .current_dir(self.dir.path())
            .env_remove("TAKO_SOCKET")
            .env_remove("TAKO_PANE_ID")
            .env_remove("TAKO_TOKEN")
            .env_remove("TAKO_ORCHESTRATOR_DIR")
            .env_remove("TAKO_2227_LEGACY")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env("HOME", self.home())
            .env("TAKO_ISOLATED", "1")
            .env("TAKO_DATA_DIR", self.dir.path())
            // claude の所在確認（ログインシェル）を外す。外さないと、この機械に claude が
            // 入っているかで「立てに行ったか」の観察点まで進むかが変わる
            .env("TAKO_983_LEGACY", "1")
            .stdin(Stdio::null());
        if legacy {
            cmd.env("TAKO_2233_LEGACY", "1");
        } else {
            cmd.env_remove("TAKO_2233_LEGACY");
        }
        let out = cmd.output().expect("tako を起こせない");
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        // GUI が無いので起動そのものは失敗する（成功したら本番の GUI へ届いている恐れ）
        assert_ne!(
            out.status.code(),
            Some(0),
            "隔離のはずが起動に成功した:\n{err}"
        );
        err
    }

    /// 立てに行った = system prompt の書き出しまで進んだ（読み込みで止まっていない）
    fn launched_prompt(&self, profile: &str) -> String {
        std::fs::read_to_string(self.prompt_file(profile)).unwrap_or_else(|e| {
            panic!(
                "{} が無い = 起動の組み立てまで進んでいない: {e}",
                self.prompt_file(profile).display()
            )
        })
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{} が読める: {e}", path.display()))
}

/// 修正後: 立てに行くが、画面・persist.log・solo 本人の prompt の 3 か所で黙らない
fn assert_warned_launch(broken: &str) {
    let env = Env::new(Some(broken));
    let err = env.solo(None, false);
    let line = err
        .lines()
        .find(|l| l.starts_with("警告: "))
        .unwrap_or_else(|| panic!("`警告:` 行が無い（黙って組み込み既定で立てている）:\n{err}"));
    assert!(
        line.contains(&env.default_yaml().display().to_string())
            && line.contains("組み込み既定で solo を立てる")
            && line.contains("default.yaml を直す"),
        "どのファイルか・何が抜けるか・直し方が無い: {line}"
    );
    assert!(
        line.contains("default.yaml.unreadable.bak"),
        "退避先を言っていない: {line}"
    );
    let log = env.persist_log();
    assert!(
        log.lines()
            .any(|l| l.contains("[solo] default:") && l.contains("組み込み既定")),
        "persist.log に残っていない:\n{log}"
    );
    let prompt = env.launched_prompt("default");
    assert!(
        prompt.contains("Startup warning") && prompt.contains("組み込み既定で solo を立てる"),
        "solo 本人へ渡っていない（CLI の行は claude の全画面に隠れうる）"
    );
    assert!(
        !prompt.contains(RULE),
        "読めないはずの個人ルールが prompt にある"
    );
    // #916 の安全要件: 退避して消さない（default.yaml そのものには触らない）
    assert_eq!(read(&env.quarantine()), broken, "退避の中身が元と違う");
    assert_eq!(
        read(&env.default_yaml()),
        broken,
        "default.yaml を書き換えた"
    );
}

#[test]
fn 壊れたyamlのsolo_defaultは立てるが警告する() {
    assert_warned_launch(BROKEN_YAML);
}

#[test]
fn 型違いのsolo_defaultでも立てるが警告する() {
    assert_warned_launch(WRONG_TYPE);
}

/// A/B: `TAKO_2233_LEGACY=1` は修正前（警告も persist.log の行も prompt の注意も退避も無く立てる）
#[test]
fn legacyでは修正前どおり黙って組み込み既定で立てる() {
    let env = Env::new(Some(BROKEN_YAML));
    let err = env.solo(None, true);
    assert!(
        !err.lines().any(|l| l.starts_with("警告: ")),
        "修正前の再現になっていない:\n{err}"
    );
    let prompt = env.launched_prompt("default");
    assert!(!prompt.contains("Startup warning"));
    assert!(!prompt.contains(RULE));
    assert!(!env.persist_log().contains("[solo] default:"));
    assert!(!env.quarantine().exists(), "修正前は退避しない");
}

/// 回帰なし: 読める default は個人ルールつきで黙って立つ
#[test]
fn 読めるsolo_defaultは警告しない() {
    let env = Env::new(Some(GOOD));
    let err = env.solo(None, false);
    assert!(!err.lines().any(|l| l.starts_with("警告: ")), "{err}");
    let prompt = env.launched_prompt("default");
    assert!(prompt.contains(RULE), "個人ルールが載っていない");
    assert!(!prompt.contains("Startup warning"));
    assert!(!env.persist_log().contains("[solo]"));
}

/// 回帰なし: 無い default（ゼロコンフィグの初回）は従来どおり作って黙って立つ
#[test]
fn 無いsolo_defaultは作って警告しない() {
    let env = Env::new(None);
    let err = env.solo(None, false);
    assert!(!err.lines().any(|l| l.starts_with("警告: ")), "{err}");
    assert!(
        env.default_yaml().is_file(),
        "初回の default.yaml を作っていない"
    );
    assert!(!env.launched_prompt("default").contains("Startup warning"));
    assert!(!env.quarantine().exists());
}

/// `-<名前>` が読めないときは従来どおり止める（立てない = 黙って既定にもしない）
#[test]
fn 読めない名前付きプロファイルは止める() {
    let env = Env::new(Some(GOOD));
    std::fs::write(env.solo_profiles().join("x2233.yaml"), BROKEN_YAML).expect("x2233.yaml");
    for legacy in [false, true] {
        let err = env.solo(Some("-x2233"), legacy);
        assert!(
            err.contains("プロファイルのパースに失敗"),
            "理由が無い（legacy={legacy}）:\n{err}"
        );
        assert!(!env.prompt_file("x2233").exists(), "止めたのに立てに行った");
    }
}
