//! 番犬: `[1m]` 警告（`ReplyNote::OneMModelWarning`）の解除の案内は、警告が名指すモデルを決めた段から
//! 組む。案内のコマンドをそのまま打つと、その警告が消える（Issue #2918）
//!
//! 修正前（実測）: 案内は出どころに関係なく
//! 「`tako orchestrator profiles set <プロファイル名> --clear-model` で解除してください」だった。
//! `--clear-model` が外すのは master / solo 本人の `model` だけなので、worker の `[1m]`
//! （ポリシーの段 = fixed の `worker_model`・`worker_agents.claude.model`・アカウントの `default_model`）は
//! 案内どおりに打っても消えなかった（master の model が外れるだけ）。solo の案内には `--solo` が無く、
//! 打つと同じ名前の master のプロファイルを触った。
//!
//! いまの扱い: 解除の手段は `orchestrator::OneMClear`（モデルを決めた段）が持ち、コマンドは
//! `OneMClear::command` の 1 か所で組む。プロファイル名も入れた「そのまま打てる」形。
//!
//! 実経路で見る（ビルドした `tako` を砂場の data dir / orchestrator の置き場 / HOME で叩く。GUI 無し）:
//! 1. `profiles show` の `warnings`（CLI と MCP は同じ dispatch の `profile_to_json`）の `[1m]` 警告の
//!    案内のコマンドが、出どころごとの期待と一致する（日英）
//! 2. `tako master` / `tako solo` の起動の警告（GUI へ繋ぐ手前で止まる）が同じコマンドを出す
//! 3. そのコマンドを `/bin/sh -c` でそのまま打つと、もう一度 `profiles show` したときにその警告が消える。
//!    アカウントは `default_model` 以外の欄（config_dir・description・default_effort）が残る。solo は
//!    同じ名前の master のプロファイルを触らない
//!
//! A/B: `TAKO_2918_BIN=<修正前の tako>` で別のバイナリを叩ける（修正前は 1〜3 が worker と solo で落ちる）。
//! claude の所在確認（ログインシェル）は `TAKO_983_LEGACY=1` で外す（本物の claude を探さない・起こさない）。
//!
//! unix だけで走らせる: 案内のコマンドを利用者のシェル（`/bin/sh`）でそのまま打つのが検査の芯。
//! PowerShell の引用の形は `orchestrator` の単体テスト（`one_mの解除のコマンドは段ごとに組む`）が見る
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use tako_core::test_residue::ScratchDir;

/// 叩くバイナリ（既定はこの木でビルドした `tako`。A/B は `TAKO_2918_BIN`）
fn tako() -> PathBuf {
    std::env::var_os("TAKO_2918_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_tako")))
}

/// accounts.yaml（`acct2918` は config_dir・空白入りの説明・effort を持つ。`acct2918m` は最小の形）
const ACCOUNTS: &str = "accounts:\n  acct2918:\n    config_dir: ~/.claude-acct2918\n    \
description: 大学 用 2918\n    default_model: claude-acct-2918[1m]\n    default_effort: low\n  \
acct2918m:\n    inherit: true\n    default_model: claude-acctm-2918[1m]\n";

/// 1 ケース
struct Case {
    name: &'static str,
    /// プロファイル名（solo のときは solo-profiles/ に置く）
    profile: &'static str,
    solo: bool,
    yaml: &'static str,
    /// 警告の出どころ（`OneMModelWarning` の `source`。show は master / worker、起動は master / worker / solo）
    show_source: &'static str,
    launch_source: &'static str,
    /// 警告が名指すモデル
    model: &'static str,
    /// 案内されるべきコマンド（そのまま打てる形）
    want: &'static str,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "master の model",
            profile: "c2918master",
            solo: false,
            yaml: "model: claude-m-2918[1m]\n",
            show_source: "master",
            launch_source: "master",
            model: "claude-m-2918[1m]",
            want: "tako orchestrator profiles set c2918master --clear-model",
        },
        Case {
            name: "ポリシーの段（fixed の worker_model）",
            profile: "c2918policy",
            solo: false,
            yaml: "worker_model_policy: fixed\nworker_model: claude-fixed-2918[1m]\n",
            show_source: "worker",
            launch_source: "worker",
            model: "claude-fixed-2918[1m]",
            want: "tako orchestrator profiles set c2918policy --clear-worker-model",
        },
        Case {
            name: "worker_agents.claude.model",
            profile: "c2918agent",
            solo: false,
            yaml: "worker_model_policy: inherit\nworker_agents:\n  claude:\n    \
model: claude-wa-2918[1m]\n",
            show_source: "worker",
            launch_source: "worker",
            model: "claude-wa-2918[1m]",
            want: "tako orchestrator profiles set c2918agent --agent claude --clear-agent-model",
        },
        Case {
            name: "アカウントの default_model（worker_account）",
            profile: "c2918acct",
            solo: false,
            yaml: "worker_model_policy: inherit\nworker_account: acct2918\n",
            show_source: "worker",
            launch_source: "worker",
            model: "claude-acct-2918[1m]",
            want: "tako orchestrator accounts add acct2918 --config-dir '~/.claude-acct2918' \
--description '大学 用 2918' --default-effort low",
        },
        Case {
            name: "アカウントの default_model（master_account・最小の形）",
            profile: "c2918acctm",
            solo: false,
            yaml: "worker_model_policy: inherit\nmaster_account: acct2918m\n",
            show_source: "worker",
            launch_source: "worker",
            model: "claude-acctm-2918[1m]",
            want: "tako orchestrator accounts add acct2918m --inherit",
        },
        Case {
            name: "solo の model",
            profile: "c2918solo",
            solo: true,
            yaml: "model: claude-s-2918[1m]\n",
            // `profiles show --solo` の見出しは master（#2918 では変えない）・`tako solo` は solo
            show_source: "master",
            launch_source: "solo",
            model: "claude-s-2918[1m]",
            want: "tako orchestrator profiles set c2918solo --solo --clear-model",
        },
    ]
}

/// 同じ名前の master のプロファイル（solo の案内が `--solo` を欠くと、こちらの model が外れる）
const SOLO_TWIN_MASTER: &str = "model: claude-twin-2918[1m]\n";

/// 砂場 1 つ（ケース × 表示言語ごとに作り直す = 打ったコマンドが次のケースへ残らない）
struct Sandbox {
    _dir: ScratchDir,
    root: PathBuf,
    data: PathBuf,
    orch: PathBuf,
    home: PathBuf,
    bin: PathBuf,
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|e| panic!("{} を作れる: {e}", parent.display()));
    }
    std::fs::write(path, text).unwrap_or_else(|e| panic!("{} を書ける: {e}", path.display()));
}

impl Sandbox {
    fn new(case: &Case, lang: &str) -> Self {
        let dir = ScratchDir::new("issue2918");
        let root = dir.path().to_path_buf();
        assert!(
            root.starts_with(std::env::temp_dir()),
            "一時 dir の外に書こうとしている: {}",
            root.display()
        );
        let data = root.join("data");
        let orch = root.join("orch");
        let home = root.join("home");
        let bin = root.join("bin");
        for d in [
            &data,
            &home,
            &bin,
            &root.join("disc"),
            &home.join(".claude-acct2918"),
        ] {
            std::fs::create_dir_all(d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
        }
        // 書かないと system = 実行環境のロケールで決まり、CI と手元で答えが変わる
        write(
            &data.join("settings.json"),
            &format!("{{\"language\":\"{lang}\"}}"),
        );
        write(&orch.join("accounts.yaml"), ACCOUNTS);
        let sub = if case.solo {
            "solo-profiles"
        } else {
            "profiles"
        };
        write(
            &orch.join(format!("{sub}/{}.yaml", case.profile)),
            case.yaml,
        );
        if case.solo {
            write(
                &orch.join(format!("profiles/{}.yaml", case.profile)),
                SOLO_TWIN_MASTER,
            );
        }
        // 案内の `tako …` をそのまま打てるよう、ビルドした tako を PATH の `tako` に置く
        std::os::unix::fs::symlink(tako(), bin.join("tako")).expect("tako の symlink を作れる");
        Self {
            _dir: dir,
            root,
            data,
            orch,
            home,
            bin,
        }
    }

    /// 砂場の env を当てた `Command`（本番の TAKO_* を全部落とす）
    fn command(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("TAKO_") {
                command.env_remove(key);
            }
        }
        command
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CLAUDECODE")
            .env("TAKO_ISOLATED", "1")
            .env("TAKO_DATA_DIR", &self.data)
            .env("TAKO_DISCOVERY_DIR", self.root.join("disc"))
            .env("TAKO_ORCHESTRATOR_DIR", &self.orch)
            .env("TAKO_SOCKET", self.root.join("none.sock"))
            .env("TAKO_983_LEGACY", "1")
            // 案内の値の引用はペインのシェルの方言で決まる（POSIX に固定する）
            .env("SHELL", "/bin/sh")
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("HOME", &self.home);
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(&tako())
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("tako {args:?} を起こせない: {e}"))
    }

    /// `profiles show <name> [--solo]` の `warnings`
    fn show_warnings(&self, case: &Case) -> Result<Vec<String>, String> {
        let mut args = vec!["orchestrator", "profiles", "show", case.profile];
        if case.solo {
            args.push("--solo");
        }
        let out = self.run(&args);
        let stdout = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() {
            return Err(format!(
                "profiles show が失敗: {}{stdout}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        let v: Value = serde_json::from_str(&stdout)
            .map_err(|e| format!("profiles show が JSON でない（{e}）: {stdout}"))?;
        Ok(v["warnings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|w| w.as_str().map(str::to_string))
            .collect())
    }

    /// `tako master -<name>` / `tako solo -<name>` の出力（GUI が無いので起動先の確保で止まる）
    fn launch_output(&self, case: &Case) -> String {
        let role = if case.solo { "solo" } else { "master" };
        let arg = format!("-{}", case.profile);
        let out = self.run(&[role, &arg]);
        // GUI が無いので起動そのものは必ず失敗する（成功したら本番の GUI へ届いている恐れ）
        assert!(
            !out.status.success(),
            "{}: 隔離のはずが起動に成功した:\n{}",
            case.name,
            String::from_utf8_lossy(&out.stderr)
        );
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    }

    /// 案内のコマンドを利用者のシェルでそのまま打つ
    fn type_command(&self, cmd: &str) -> Output {
        self.command(Path::new("/bin/sh"))
            .args(["-c", cmd])
            .output()
            .unwrap_or_else(|e| panic!("/bin/sh -c {cmd} を起こせない: {e}"))
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.orch.join(rel)).unwrap_or_default()
    }
}

/// `[1m]` 警告の頭（`OneMModelWarning` の 1 行目の書き出し）
fn one_m_head(source: &str, model: &str, lang: &str) -> String {
    match lang {
        "ja" => format!("[警告] {source} のモデル '{model}'"),
        _ => format!("[warning] the {source} model '{model}'"),
    }
}

/// `text` の中の `source` / `model` の `[1m]` 警告の案内のコマンド（最初のバッククオートの中）。
/// 警告が無ければ None、警告はあるのにコマンドが無ければ Some("")
fn hint_command(text: &str, source: &str, model: &str, lang: &str) -> Option<String> {
    let head = one_m_head(source, model, lang);
    let at = text.find(&head)?;
    let rest = &text[at + head.len()..];
    // 次の警告の頭までが 1 件
    let end = ["[警告]", "[warning]"]
        .iter()
        .filter_map(|h| rest.find(h))
        .min()
        .unwrap_or(rest.len());
    let body = &rest[..end];
    Some(
        body.split('`')
            .nth(1)
            .map(str::to_string)
            .unwrap_or_default(),
    )
}

fn check(case: &Case, lang: &str, failures: &mut Vec<String>) {
    let tag = format!("[{}（{lang}）]", case.name);
    let sb = Sandbox::new(case, lang);

    // 1. profiles show の案内
    let before = match sb.show_warnings(case) {
        Ok(w) => w,
        Err(e) => {
            failures.push(format!("{tag} {e}"));
            return;
        }
    };
    let joined = before.join("\n");
    let Some(shown) = hint_command(&joined, case.show_source, case.model, lang) else {
        failures.push(format!(
            "{tag} profiles show に {} の [1m] 警告（{}）が出ない: {before:?}",
            case.show_source, case.model
        ));
        return;
    };
    if shown != case.want {
        failures.push(format!(
            "{tag} profiles show の解除の案内が出どころに合わない\n  want: {}\n  got:  {shown}",
            case.want
        ));
    }

    // 2. tako master / tako solo の起動の警告が同じコマンドを出す
    let launch = sb.launch_output(case);
    match hint_command(&launch, case.launch_source, case.model, lang) {
        Some(cmd) if cmd == case.want => {}
        Some(cmd) => failures.push(format!(
            "{tag} 起動の警告の解除の案内が出どころに合わない\n  want: {}\n  got:  {cmd}",
            case.want
        )),
        None => failures.push(format!(
            "{tag} 起動の警告に {} の [1m] 警告（{}）が出ない:\n{launch}",
            case.launch_source, case.model
        )),
    }

    // 3. 案内のコマンドをそのまま打つと、その警告が消える
    if shown.is_empty() {
        failures.push(format!("{tag} 案内にコマンドが無い: {before:?}"));
        return;
    }
    let typed = sb.type_command(&shown);
    if !typed.status.success() {
        failures.push(format!(
            "{tag} 案内のコマンドが失敗した: {shown}\n{}{}",
            String::from_utf8_lossy(&typed.stdout),
            String::from_utf8_lossy(&typed.stderr)
        ));
        return;
    }
    let after = match sb.show_warnings(case) {
        Ok(w) => w,
        Err(e) => {
            failures.push(format!("{tag} 打った後の {e}"));
            return;
        }
    };
    if hint_command(&after.join("\n"), case.show_source, case.model, lang).is_some() {
        failures.push(format!(
            "{tag} 案内のコマンドを打っても警告が消えない: {shown}\n  前: {before:?}\n  後: {after:?}"
        ));
    }
    // 打つ前にあった他の警告は残る（触っていない欄の警告を巻き込まない）
    for w in before.iter().filter(|w| !w.contains(case.model)) {
        if !after.contains(w) {
            failures.push(format!("{tag} 関係の無い警告まで消えた: {w}"));
        }
    }

    // 巻き込みの確認: アカウントは default_model 以外の欄が書いてあるまま残る・solo は同じ名前の
    // master を触らない
    if let Some(account) = case
        .want
        .strip_prefix("tako orchestrator accounts add ")
        .and_then(|rest| rest.split_whitespace().next())
    {
        let path = sb.orch.join("accounts.yaml");
        let config = tako_control::orchestrator::AccountsConfig::load_from(&path)
            .unwrap_or_else(|e| panic!("{tag} accounts.yaml を読めない: {e}"));
        let got = config.accounts.get(account).cloned().unwrap_or_default();
        let want = if account == "acct2918" {
            (
                Some("~/.claude-acct2918"),
                false,
                Some("大学 用 2918"),
                Some("low"),
            )
        } else {
            (None, true, None, None)
        };
        let actual = (
            got.config_dir.as_deref(),
            got.inherit,
            got.description.as_deref(),
            got.default_effort.as_deref(),
        );
        if actual != want {
            failures.push(format!(
                "{tag} 登録し直しでアカウントの default_model 以外の欄が変わった: \
                 want={want:?} got={actual:?}\n{}",
                sb.read("accounts.yaml")
            ));
        }
        if got.default_model.is_some() {
            failures.push(format!(
                "{tag} アカウントの default_model が残っている: {:?}",
                got.default_model
            ));
        }
    }
    if case.solo {
        let twin = sb.read(&format!("profiles/{}.yaml", case.profile));
        if !twin.contains("claude-twin-2918[1m]") {
            failures.push(format!(
                "{tag} solo の案内で同じ名前の master のプロファイルの model が外れた:\n{twin}"
            ));
        }
    }
}

#[test]
fn one_mの解除の案内は出どころから組み打つと警告が消える() {
    let mut failures = Vec::new();
    let cases = cases();
    for case in &cases {
        for lang in ["ja", "en"] {
            check(case, lang, &mut failures);
        }
    }
    assert!(
        failures.is_empty(),
        "[1m] 警告の解除の案内が出どころと合わない（#2918）: {} 件\n{}",
        failures.len(),
        failures.join("\n")
    );
}
