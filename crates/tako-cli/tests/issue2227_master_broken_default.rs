//! 壊れた `profiles/default.yaml` で `tako master` が黙って組み込み既定で立たず、
//! `tako master -<key>` が直し方の無い「見つからない」で止まらない（Issue #2227）
//!
//! 修正前（実測）:
//!
//! - `tako master` は `[情報] 1 config file(s) could not be read…`（移行の汎用の一言。
//!   どのファイルか・個人ルールとモデルが抜けることは言わない）だけを出して、
//!   組み込み既定の master を立てに行っていた。スマホ・返答の配送（`master_launch::plan`）は
//!   その一言すら出さない
//! - `tako master -<key>` は #1825 で専用プロファイルが作られていないので
//!   「プロファイル '<key>' が見つからない」だけで止まり、理由も直し方も出なかった
//!
//! いまは ① 立てるが `警告:` 行・persist.log の `[master] default:` の 1 行・master 本人の
//! system prompt の 3 か所へ理由と直し方を出し、② `projects add` と同じ理由と直し方
//! （default.yaml を直したら `tako migrate run`）を足して止まる。
//!
//! 本物の CLI を隔離 data dir（`TAKO_DATA_DIR` = 使い捨て dir・`HOME` も使い捨て）で叩く。
//! GUI は無いので起動は「起動先ペインを確保できない」で終わるが、その手前の
//! system prompt の書き出しまで進んだこと（= 立てに行ったこと）を prompt ファイルで見る。
//! claude の所在確認（ログインシェル）は `TAKO_983_LEGACY=1` で外す（本物の claude を
//! 探さない・CI のランナーに claude が無くても同じ経路を通る）。
//! A/B は `TAKO_2227_LEGACY=1`（修正前を同じバイナリで再現する）。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tako_core::test_residue::ScratchDir;

const KEY: &str = "p2227";
const RULE: &str = "PERSONAL_RULE_2227";
const GOOD: &str = "effort: max\nmodel: claude-test-model\nprompt_blocks:\n  append: \"PERSONAL_RULE_2227\"\nbypass_sandbox: false\n";
const BROKEN_YAML: &str =
    "effort: max\nmodel: claude-test-model\nprompt_blocks:\n  append: \"PERSONAL_RULE_2227\n  prepend: [\n";
const WRONG_TYPE: &str =
    "effort: max\nmodel: claude-test-model\nprompt_blocks:\n  append: [PERSONAL_RULE_2227]\n";

struct Env {
    dir: ScratchDir,
}

impl Env {
    /// `default_yaml` が `None` = default.yaml が無い（ゼロコンフィグの初回）
    fn new(default_yaml: Option<&str>) -> Self {
        let dir = ScratchDir::new("issue2227");
        assert!(
            dir.path().starts_with(std::env::temp_dir()),
            "一時 dir の外に書こうとしている: {}",
            dir.path().display()
        );
        let env = Self { dir };
        for d in [env.profiles(), env.proj(), env.home()] {
            std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
        }
        if let Some(body) = default_yaml {
            std::fs::write(env.default_yaml(), body).expect("default.yaml を置ける");
        }
        env
    }

    fn profiles(&self) -> PathBuf {
        self.dir.path().join("orchestrator").join("profiles")
    }

    fn proj(&self) -> PathBuf {
        self.dir.path().join("proj")
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    fn default_yaml(&self) -> PathBuf {
        self.profiles().join("default.yaml")
    }

    fn quarantine(&self) -> PathBuf {
        self.profiles().join("default.yaml.unreadable.bak")
    }

    fn prompt_file(&self, profile: &str) -> PathBuf {
        self.dir
            .path()
            .join("orchestrator")
            .join(format!("_system_prompt_{profile}.md"))
    }

    fn persist_log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("persist.log")).unwrap_or_default()
    }

    /// 隔離した tako（本番の GUI にも data dir にも HOME にも触らない）
    fn tako(&self, args: &[&str], legacy: bool) -> (i32, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_tako"));
        cmd.args(args)
            .current_dir(self.dir.path())
            .env_remove("TAKO_SOCKET")
            .env_remove("TAKO_PANE_ID")
            .env_remove("TAKO_TOKEN")
            .env_remove("TAKO_ORCHESTRATOR_DIR")
            .env_remove("TAKO_1825_LEGACY")
            .env_remove("TAKO_1453_LEGACY")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env("HOME", self.home())
            .env("TAKO_ISOLATED", "1")
            .env("TAKO_DATA_DIR", self.dir.path())
            // claude の所在確認（ログインシェル）を外す。外さないと、この機械に claude が
            // 入っているかで「立てに行ったか」の観察点まで進むかが変わる
            .env("TAKO_983_LEGACY", "1")
            .stdin(Stdio::null());
        if legacy {
            cmd.env("TAKO_2227_LEGACY", "1");
        } else {
            cmd.env_remove("TAKO_2227_LEGACY");
        }
        let out = cmd.output().expect("tako を起こせない");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn master(&self, profile_arg: Option<&str>, legacy: bool) -> String {
        let mut args = vec!["master"];
        args.extend(profile_arg);
        let (code, err) = self.tako(&args, legacy);
        // GUI が無いので起動そのものは失敗する（成功したら本番の GUI へ届いている恐れ）
        assert_ne!(code, 0, "隔離のはずが起動に成功した:\n{err}");
        err
    }

    /// `-<key>` の場の用意（#1825 の `projects add`。壊れた default では専用プロファイルを作らない）
    fn add_project(&self) {
        let proj = self.proj();
        let (code, err) = self.tako(
            &[
                "orchestrator",
                "projects",
                "add",
                "--key",
                KEY,
                "--cwd",
                proj.to_str().expect("UTF-8 のパス"),
            ],
            false,
        );
        assert_eq!(code, 0, "projects add が失敗: {err}");
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

/// ① 修正後: 立てに行くが、画面・persist.log・master 本人の prompt の 3 か所で黙らない
fn assert_warned_launch(broken: &str) {
    let env = Env::new(Some(broken));
    let err = env.master(None, false);
    let line = err
        .lines()
        .find(|l| l.starts_with("警告: "))
        .unwrap_or_else(|| panic!("`警告:` 行が無い（黙って組み込み既定で立てている）:\n{err}"));
    assert!(
        line.contains(&env.default_yaml().display().to_string())
            && line.contains("組み込み既定")
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
            .any(|l| l.contains("[master] default:") && l.contains("組み込み既定")),
        "persist.log に残っていない:\n{log}"
    );
    let prompt = env.launched_prompt("default");
    assert!(
        prompt.contains("Startup warning") && prompt.contains("組み込み既定"),
        "master 本人へ渡っていない（CLI の行は claude の全画面に隠れうる）"
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
fn 壊れたyamlのdefaultでmasterは立てるが警告する() {
    assert_warned_launch(BROKEN_YAML);
}

#[test]
fn 型違いのdefaultでもmasterは立てるが警告する() {
    assert_warned_launch(WRONG_TYPE);
}

/// A/B: `TAKO_2227_LEGACY=1` は修正前（警告も persist.log の行も prompt の注意も出さずに立てる）
#[test]
fn legacyでは修正前の黙った起動を再現する() {
    let env = Env::new(Some(BROKEN_YAML));
    let err = env.master(None, true);
    assert!(
        !err.contains("警告: "),
        "legacy なのに警告している（A/B になっていない）:\n{err}"
    );
    assert!(!env.persist_log().contains("[master] default:"));
    let prompt = env.launched_prompt("default");
    assert!(
        !prompt.contains("Startup warning"),
        "legacy なのに prompt に注意がある"
    );
}

/// 回帰なし: 読める default は従来どおり黙って立ち、個人ルールが prompt に入る
#[test]
fn 読めるdefaultは従来どおり黙って立つ() {
    let env = Env::new(Some(GOOD));
    let err = env.master(None, false);
    assert!(!err.contains("警告: "), "読めるのに警告している:\n{err}");
    let prompt = env.launched_prompt("default");
    assert!(prompt.contains(RULE), "個人ルールが prompt に入っていない");
    assert!(!prompt.contains("Startup warning"));
    assert!(!env.quarantine().exists(), "読めるのに退避している");
    assert!(!env.persist_log().contains("[master]"));
}

/// 境界: 空の default.yaml は「設定なし」（#1825 / 移行の validate_profile と同じ判定）。警告しない
#[test]
fn 空のdefaultは設定なしとして黙って立つ() {
    let env = Env::new(Some(""));
    let err = env.master(None, false);
    assert!(!err.contains("警告: "), "空なのに警告している:\n{err}");
    assert!(!env.launched_prompt("default").contains("Startup warning"));
    assert!(!env.quarantine().exists());
}

/// 境界: default.yaml が無い（ゼロコンフィグの初回）は従来どおり既定を作って黙って立つ
#[test]
fn 無いdefaultはゼロコンフィグのまま立つ() {
    let env = Env::new(None);
    let err = env.master(None, false);
    assert!(!err.contains("警告: "), "初回なのに警告している:\n{err}");
    assert!(!env.launched_prompt("default").contains("Startup warning"));
    assert!(
        env.default_yaml().is_file(),
        "初回の既定 default.yaml を作っていない"
    );
}

/// ② 修正後: 「見つからない」に `projects add` と同じ理由と直し方を足して止まる。
/// 直した後は `tako master -<key>` そのものが継承して作って立てに行く
#[test]
fn 未生成のkeyは理由と直し方で止まり直せば立つ() {
    let env = Env::new(Some(BROKEN_YAML));
    env.add_project();
    let err = env.master(Some(&format!("-{KEY}")), false);
    assert!(
        err.contains(&format!("プロファイル '{KEY}' が見つからない"))
            && err.contains(&env.default_yaml().display().to_string())
            && err.contains("default を継承できない")
            && err.contains("tako migrate run"),
        "理由と直し方が無い:\n{err}"
    );
    assert!(
        !env.prompt_file(KEY).exists(),
        "止まるはずが起動の組み立てまで進んだ"
    );
    let log = env.persist_log();
    assert!(
        log.lines()
            .any(|l| l.contains(&format!("[master] {KEY}:")) && l.contains("tako migrate run")),
        "persist.log に残っていない:\n{log}"
    );

    // 直すと、起動時の差分検出（#916）が継承して作り、そのまま立てに行く
    std::fs::write(env.default_yaml(), GOOD).expect("default.yaml を直せる");
    let err = env.master(Some(&format!("-{KEY}")), false);
    assert!(!err.contains("見つからない"), "直したのに止まる:\n{err}");
    assert!(
        env.launched_prompt(KEY).contains(RULE),
        "直した後の専用 master に個人ルールが無い"
    );
}

/// A/B: `TAKO_2227_LEGACY=1` は修正前（直し方の無い「見つからない」）
#[test]
fn legacyでは未生成のkeyが直し方なしで止まる() {
    let env = Env::new(Some(BROKEN_YAML));
    env.add_project();
    let err = env.master(Some(&format!("-{KEY}")), true);
    assert!(
        err.contains(&format!("プロファイル '{KEY}' が見つからない")),
        "{err}"
    );
    assert!(
        !err.contains("tako migrate run"),
        "legacy なのに直し方を出している（A/B になっていない）:\n{err}"
    );
}

/// 境界: 登録していない名前は従来どおりの「見つからない」だけ（#1825 の理由を誤って足さない）
#[test]
fn 未登録の名前は従来の見つからないのまま() {
    let env = Env::new(Some(BROKEN_YAML));
    let err = env.master(Some("-nope2227"), false);
    assert!(
        err.contains("プロファイル 'nope2227' が見つからない"),
        "{err}"
    );
    assert!(
        !err.contains("default を継承できない"),
        "未登録の名前に #1825 の理由を足している:\n{err}"
    );
}
