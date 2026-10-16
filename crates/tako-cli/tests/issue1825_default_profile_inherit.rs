//! 壊れた `profiles/default.yaml` で `tako orchestrator projects add` しても、
//! 個人ルール・モデルの抜けた専用プロファイルを黙って作らない（Issue #1825）
//!
//! 修正前は `project_profile_base()` が読めない default を組み込み既定へ黙って落とし、
//! 「専用プロファイルを作りました（default から継承）」と表示して、`prompt_blocks` も
//! `model` も無いプロファイルを作っていた（実測）。生成は既存に触らない（冪等）ので、
//! default.yaml を直して `tako migrate run` / `projects add` をやり直しても追いつかない
//! （これも実測）。いまは**作らずに**登録だけを済ませ、理由を応答の `warnings` と
//! persist.log に残し、壊れた中身は `default.yaml.unreadable.bak` へ退避する。
//! default.yaml を直せば `tako migrate run`（GUI 起動でも同じ）が正しく継承して作る。
//!
//! 本物の CLI を隔離 data dir（`TAKO_DATA_DIR` = 使い捨て dir）で叩く。
//! A/B は `TAKO_1825_LEGACY=1`（修正前の「黙って既定で作る」を同じバイナリで再現する）。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tako_core::test_residue::ScratchDir;

const KEY: &str = "p1825";
const RULE: &str = "PERSONAL_RULE_1825";
const GOOD: &str = "effort: max\nmodel: claude-test-model\nprompt_blocks:\n  append: \"PERSONAL_RULE_1825\"\nbypass_sandbox: false\n";
const BROKEN_YAML: &str =
    "effort: max\nmodel: claude-test-model\nprompt_blocks:\n  append: \"PERSONAL_RULE_1825\n  prepend: [\n";
const WRONG_TYPE: &str =
    "effort: max\nmodel: claude-test-model\nprompt_blocks:\n  append: [PERSONAL_RULE_1825]\n";

struct Env {
    dir: ScratchDir,
}

impl Env {
    fn new(default_yaml: &str) -> Self {
        let dir = ScratchDir::new("issue1825");
        assert!(
            dir.path().starts_with(std::env::temp_dir()),
            "一時 dir の外に書こうとしている: {}",
            dir.path().display()
        );
        let env = Self { dir };
        std::fs::create_dir_all(env.profiles()).expect("profiles を作れる");
        std::fs::create_dir_all(env.proj()).expect("proj を作れる");
        std::fs::write(env.default_yaml(), default_yaml).expect("default.yaml を置ける");
        env
    }

    fn profiles(&self) -> PathBuf {
        self.dir.path().join("orchestrator").join("profiles")
    }

    fn proj(&self) -> PathBuf {
        self.dir.path().join("proj")
    }

    fn default_yaml(&self) -> PathBuf {
        self.profiles().join("default.yaml")
    }

    fn generated(&self) -> PathBuf {
        self.profiles().join(format!("{KEY}.yaml"))
    }

    fn quarantine(&self) -> PathBuf {
        self.profiles().join("default.yaml.unreadable.bak")
    }

    fn persist_log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("persist.log")).unwrap_or_default()
    }

    /// 隔離した tako（本番の GUI にも data dir にも触らない）
    fn tako(&self, args: &[&str], legacy: bool) -> (i32, String, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_tako"));
        cmd.args(args)
            .env_remove("TAKO_SOCKET")
            .env_remove("TAKO_PANE_ID")
            .env_remove("TAKO_TOKEN")
            .env_remove("TAKO_ORCHESTRATOR_DIR")
            .env_remove("TAKO_1453_LEGACY")
            .env("TAKO_ISOLATED", "1")
            .env("TAKO_DATA_DIR", self.dir.path())
            .stdin(Stdio::null());
        if legacy {
            cmd.env("TAKO_1825_LEGACY", "1");
        } else {
            cmd.env_remove("TAKO_1825_LEGACY");
        }
        let out = cmd.output().expect("tako を起こせない");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn add(&self, legacy: bool) -> (i32, String) {
        let proj = self.proj();
        let (code, _, err) = self.tako(
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
        (code, err)
    }

    fn registered(&self) -> bool {
        std::fs::read_to_string(self.dir.path().join("orchestrator").join("projects.yaml"))
            .unwrap_or_default()
            .contains(KEY)
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{} が読める: {e}", path.display()))
}

/// 修正後: 作らない・登録は成功・警告・persist.log・退避。直したら migrate run で正しく作る
fn assert_not_inherited(broken: &str) {
    let env = Env::new(broken);
    let (code, err) = env.add(false);
    assert_eq!(code, 0, "登録そのものは成功させる: {err}");
    assert!(env.registered(), "projects.yaml に登録されていない");
    assert!(
        !env.generated().exists(),
        "個人ルールの抜けたプロファイルを作った:\n{}",
        read(&env.generated())
    );
    assert!(
        err.contains("警告:") && err.contains("default を継承できない"),
        "CLI が警告を出していない:\n{err}"
    );
    assert!(
        !err.contains("default から継承"),
        "継承していないのに「default から継承」と言っている:\n{err}"
    );
    let log = env.persist_log();
    assert!(
        log.lines()
            .any(|l| l.contains(&format!("[projects add] {KEY}:")) && l.contains("default")),
        "persist.log に残っていない:\n{log}"
    );
    assert_eq!(read(&env.quarantine()), broken, "退避の中身が元と違う");
    assert_eq!(
        read(&env.default_yaml()),
        broken,
        "default.yaml を書き換えた"
    );
    assert!(
        err.contains("default.yaml.unreadable.bak"),
        "退避先を言っていない:\n{err}"
    );

    // migrate status は「作る」ではなく「作れない」と予告する（run と食い違わない）
    // 種別は profiles に絞る（全種別は data dir の外 = Claude の設定 dir の印も対象に含む）
    let (_, status, _) = env.tako(&["migrate", "--schema", "profiles"], false);
    let status: serde_json::Value = serde_json::from_str(&status).expect("status は JSON");
    let entry = status["files"]
        .as_array()
        .and_then(|files| {
            files.iter().find(|f| {
                f["path"]
                    .as_str()
                    .is_some_and(|p| p.ends_with("p1825.yaml"))
            })
        })
        .unwrap_or_else(|| panic!("status に {KEY}.yaml が無い: {status:#}"));
    assert_eq!(
        entry["state"], "failed",
        "status が作ると予告している: {entry:#}"
    );

    // default.yaml を直すと、差分検出が**継承して**作る（抜けが恒久化しない）
    std::fs::write(env.default_yaml(), GOOD).expect("default.yaml を直せる");
    let (code, _, err) = env.tako(&["migrate", "run", "--schema", "profiles"], false);
    assert_eq!(code, 0, "migrate run が失敗: {err}");
    let made = read(&env.generated());
    assert!(
        made.contains(RULE) && made.contains("claude-test-model"),
        "直した後の生成が default を継承していない:\n{made}"
    );
}

#[test]
fn 壊れたyamlのdefaultでは作らずに警告し退避する() {
    assert_not_inherited(BROKEN_YAML);
}

#[test]
fn 型違いのdefaultでも作らずに警告し退避する() {
    assert_not_inherited(WRONG_TYPE);
}

/// A/B: `TAKO_1825_LEGACY=1` は修正前（黙って組み込み既定で作り「default から継承」と言う）
#[test]
fn legacyでは修正前の黙った生成を再現する() {
    let env = Env::new(BROKEN_YAML);
    let (code, err) = env.add(true);
    assert_eq!(code, 0, "{err}");
    let made = read(&env.generated());
    assert!(
        !made.contains(RULE) && !made.contains("claude-test-model"),
        "legacy なのに継承している（A/B になっていない）:\n{made}"
    );
    assert!(err.contains("default から継承"), "{err}");
    assert!(!err.contains("警告:"), "legacy なのに警告している: {err}");
    assert!(!env.quarantine().exists(), "legacy なのに退避している");
    assert!(!env.persist_log().contains("[projects add]"));
}

/// 回帰なし: 読める default は従来どおり継承する（警告も退避も出ない）
#[test]
fn 読めるdefaultは従来どおり継承する() {
    let env = Env::new(GOOD);
    let (code, err) = env.add(false);
    assert_eq!(code, 0, "{err}");
    let made = read(&env.generated());
    assert!(made.contains(RULE), "個人ルールを継承していない:\n{made}");
    assert!(
        made.contains("claude-test-model"),
        "モデルを継承していない:\n{made}"
    );
    assert!(err.contains("default から継承"), "{err}");
    assert!(!err.contains("警告:"), "読めるのに警告している: {err}");
    assert!(!env.quarantine().exists(), "読めるのに退避している");
}

/// 境界: 空の default.yaml は「設定なし」（移行の validate_profile と同じ判定）。
/// 継ぐ個人設定が無いので作ってよく、警告もしない
#[test]
fn 空のdefaultは設定なしとして作る() {
    let env = Env::new("");
    let (code, err) = env.add(false);
    assert_eq!(code, 0, "{err}");
    assert!(
        env.generated().exists(),
        "空の default で作っていない: {err}"
    );
    assert!(!err.contains("警告:"), "空なのに警告している: {err}");
    assert!(!env.quarantine().exists());
}
