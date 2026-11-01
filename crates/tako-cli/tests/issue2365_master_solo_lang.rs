//! 実経路: `tako master` / `tako solo` の起動の流れの断り・行と、移行の知らせが表示言語に従う
//! （Issue #2365 の続き）。
//!
//! 修正前（実測）: 英語表示でも `tako master -<無いプロファイル>` は「プロファイル '…' が見つからない」、
//! 1M モデルの警告は「[警告] master のモデル '…' は 1M コンテキスト版のため…」、MCP の登録パスが消えたときは
//! 「[警告] MCP 登録パスが消失しています」のように日本語の固定文のまま出た（CLI `main.rs` と、起動の流れが
//! 呼ぶ日本語の口 = `into_ja` が固定していた）。逆に**日本語表示でも**移行の知らせだけが
//! `[情報] Automatically migrated 1 config file(s) …` と英語で混ざった（表示言語を決める前に移行を
//! 走らせていた = 静的既定の英語で組まれていた）。
//!
//! GUI を通さない手元の処理なので、**ビルドした `tako` をそのまま**叩く（GUI 無し。起動先のペインを
//! 確保する段 = GUI へ繋ぐところで止まる）: 一時の data dir（settings.json の `language`）・orchestrator の
//! 置き場・HOME で `tako master` / `tako solo` を走らせ、標準出力と標準エラーをまとめて読む。見ること:
//! - 英語表示: 出力に日本語（かな・漢字）が 1 字も無い
//! - 日本語表示: #2365 より前の文（[`Case::ja`]）をそのまま含み、移行の知らせは日本語（英語の知らせが無い）
//!
//! A/B: `TAKO_2365_MASTER_BIN=<修正前の tako>` で別のバイナリを叩ける（修正前は英語表示の検査と、日本語表示の
//! 移行の知らせの検査で落ちる）。claude の所在確認（ログインシェル）は `TAKO_983_LEGACY=1` で外す（本物の
//! claude を探さない・CI のランナーに claude が無くても同じ経路を通る）。起動後の行（起動しました・
//! プロファイル・worker モデルポリシーほか）は GUI が要るので、番犬の `ReplyNote` の表（日英の対と旧文）が見る。
//!
//! unix だけで走らせる: 権限の無い cwd（`chmod 000`）と `~/.claude.json` の置き場が unix の前提。文の日英の対は
//! OS に依らない 1 か所（`Reason` / `ReplyNote` / `ConfigError`）で、両 OS の番犬
//! （`crates/tako-control/tests/issue1762_dispatch_error_lang.rs`）が表で見る
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tako_core::test_residue::ScratchDir;

/// 叩くバイナリ（既定はこの木でビルドした `tako`。A/B は `TAKO_2365_MASTER_BIN`）
fn tako() -> PathBuf {
    std::env::var_os("TAKO_2365_MASTER_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_tako")))
}

/// 1M コンテキスト版のモデル（#27 の移行が外さないよう、同名の `.backup-1m` の印を隣に置く）
const ONE_M: &str = "claude-opus-4-6[1m]";
/// 移行の知らせ（#2365 より前の日本語。英語表示でもこの言語で出ていた見出し `[情報]` を含む）
const MIGRATED_JA: &str =
    "[情報] 設定ファイル 1 件を新しい形式へ自動移行しました（旧内容は .bak へ退避）";
/// 修正前の日本語表示に混ざっていた英語の知らせ（静的既定の英語で組まれた文）
const MIGRATED_EN: &str = "Automatically migrated";

/// 砂場に置くもの
#[derive(Clone, Copy)]
enum Setup {
    /// 何も置かない
    None,
    /// 旧形式の projects.yaml（`version` が無い = 移行が走る）
    OldProjects,
    /// master のプロファイル `<name>.yaml` に本文を置く
    Master(&'static str, &'static str),
    /// solo のプロファイル `<name>.yaml` に本文を置く
    Solo(&'static str, &'static str),
    /// 1M モデルの master / solo のプロファイル（`.backup-1m` の印つき）
    OneM { solo: bool, key: &'static str },
    /// `~/.claude.json` の tako の MCP の登録パスが消えている
    McpPathGone,
    /// orchestrator の置き場をファイルの下にする（既定の用意で dir を作れない）
    BlockedOrchestrator,
    /// cwd が入れない dir（`chmod 000`）の master のプロファイル `np.yaml`
    NoPermCwd,
}

struct Case {
    name: &'static str,
    args: &'static [&'static str],
    setup: Setup,
    /// #2365 より前の日本語の文（日本語表示の出力にそのまま含まれる）
    ja: &'static str,
}

const fn case(
    name: &'static str,
    args: &'static [&'static str],
    setup: Setup,
    ja: &'static str,
) -> Case {
    Case {
        name,
        args,
        setup,
        ja,
    }
}

fn cases() -> Vec<Case> {
    vec![
        // ---- 移行の知らせ（日本語表示で英語が混ざっていた） ----
        case("master_migrated", &["master"], Setup::OldProjects, MIGRATED_JA),
        // ---- 断り（`Reason` と型の口 = `ConfigError::text_in`） ----
        case(
            "master_unknown_profile",
            &["master", "-nope2365"],
            Setup::None,
            "プロファイル 'nope2365' が見つからない: ",
        ),
        case(
            "solo_unknown_profile",
            &["solo", "-nope2365"],
            Setup::None,
            "solo プロファイル 'nope2365' が見つからない: ",
        ),
        case(
            "master_env_reserved",
            &["master", "-envr"],
            Setup::Master("envr", "env:\n  TAKO_SOCKET: x\n"),
            "プロファイルの env に tako 内部変数が含まれている（上書き禁止）: TAKO_SOCKET",
        ),
        case(
            "solo_env_reserved",
            &["solo", "-envr"],
            Setup::Solo("envr", "env:\n  TAKO_SOCKET: x\n"),
            "プロファイルの env に tako 内部変数が含まれている（上書き禁止）: TAKO_SOCKET",
        ),
        case(
            "master_cwd_missing",
            &["master", "-cwdm"],
            Setup::Master("cwdm", "cwd: /nonexistent-2365\n"),
            "プロファイルの cwd が存在しない: /nonexistent-2365（元の指定: /nonexistent-2365）",
        ),
        case(
            "solo_cwd_missing",
            &["solo", "-cwdm"],
            Setup::Solo("cwdm", "cwd: /nonexistent-2365\n"),
            "プロファイルの cwd が存在しない: /nonexistent-2365（元の指定: /nonexistent-2365）",
        ),
        case(
            "master_cwd_no_permission",
            &["master", "-np"],
            Setup::NoPermCwd,
            "プロファイルの cwd に移動できない: ",
        ),
        case(
            "master_projects_unregistered",
            &["master", "-prj"],
            Setup::Master("prj", "projects:\n- zzz2365\n"),
            "プロファイルの projects に projects.yaml に未登録のキーがある: zzz2365",
        ),
        case(
            "master_agent_unknown",
            &["master", "-agt"],
            Setup::Master("agt", "master_agent: foo2365\n"),
            "プロファイルの master_agent が不正: 未対応のエージェント種別 'foo2365'（対応: claude / codex / agy）",
        ),
        case(
            "solo_agent_unknown",
            &["solo", "-agt"],
            Setup::Solo("agt", "master_agent: foo2365\n"),
            "プロファイルの master_agent が不正: 未対応のエージェント種別 'foo2365'（対応: claude / codex / agy）",
        ),
        case(
            "master_account_unknown",
            &["master", "-acct"],
            Setup::Master("acct", "master_account: nobody2365\n"),
            "アカウント 'nobody2365' が accounts.yaml に見つからない",
        ),
        case(
            "master_setup_failed",
            &["master"],
            Setup::BlockedOrchestrator,
            "セットアップに失敗: ディレクトリの作成に失敗: ",
        ),
        case(
            "solo_setup_failed",
            &["solo"],
            Setup::BlockedOrchestrator,
            "セットアップに失敗: ディレクトリの作成に失敗: ",
        ),
        // ---- 起動の前に出す行（`ReplyNote`） ----
        case(
            "master_one_m_model",
            &["master", "-onem"],
            Setup::OneM {
                solo: false,
                key: "onem",
            },
            "[警告] master のモデル 'claude-opus-4-6[1m]' は 1M コンテキスト版のため、Pro プランでは起動できない可能性があります（Max / API プラン向け）。\n  起動に失敗する場合は `tako orchestrator profiles set <プロファイル名> --clear-model` で解除してください",
        ),
        case(
            "solo_one_m_model",
            &["solo", "-onem"],
            Setup::OneM {
                solo: true,
                key: "onem",
            },
            "[警告] solo のモデル 'claude-opus-4-6[1m]' は 1M コンテキスト版のため",
        ),
        case(
            "master_mcp_path_gone",
            &["master"],
            Setup::McpPathGone,
            "[警告] MCP 登録パスが消失しています: /nonexistent/tako-2365\n        tako MCP なしで起動します。tako setup-mcp で修復してください。",
        ),
        case(
            "solo_mcp_path_gone",
            &["solo"],
            Setup::McpPathGone,
            "[警告] MCP 登録パスが消失しています: /nonexistent/tako-2365",
        ),
    ]
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|e| panic!("{} を作れる: {e}", parent.display()));
    }
    std::fs::write(path, text).unwrap_or_else(|e| panic!("{} を書ける: {e}", path.display()));
}

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

/// 1 ケースを `lang` で叩き、標準出力と標準エラーをまとめて返す。
/// `None` = この環境では作れない場（root で `chmod 000` の dir にも入れる）
fn run(case: &Case, lang: &str) -> Option<String> {
    let dir = ScratchDir::new("issue2365-master");
    let root = dir.path();
    assert!(
        root.starts_with(std::env::temp_dir()),
        "一時 dir の外に書こうとしている: {}",
        root.display()
    );
    let data = root.join("data");
    let orch = root.join("orch");
    let home = root.join("home");
    for d in [&data, &home, &root.join("disc"), &orch.join("profiles")] {
        std::fs::create_dir_all(d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
    }
    // 書かないと system = 実行環境のロケールで決まり、CI と手元で答えが変わる
    write(
        &data.join("settings.json"),
        &format!("{{\"language\":\"{lang}\"}}"),
    );
    let mut orch_dir = orch.clone();
    let mut locked: Option<PathBuf> = None;
    match case.setup {
        Setup::None => {}
        Setup::OldProjects => write(&orch.join("projects.yaml"), "projects: {}\n"),
        Setup::Master(key, body) => write(&orch.join(format!("profiles/{key}.yaml")), body),
        Setup::Solo(key, body) => write(&orch.join(format!("solo-profiles/{key}.yaml")), body),
        Setup::OneM { solo, key } => {
            let sub = if solo { "solo-profiles" } else { "profiles" };
            write(
                &orch.join(format!("{sub}/{key}.yaml")),
                &format!("model: \"{ONE_M}\"\n"),
            );
            // #27 の手書きの移行の印（無いと移行が `[1m]` を外して警告まで届かない）
            write(&orch.join(format!("{sub}/{key}.yaml.backup-1m")), "");
        }
        Setup::McpPathGone => write(
            &home.join(".claude.json"),
            "{\"mcpServers\":{\"tako\":{\"command\":\"/nonexistent/tako-2365\"}}}",
        ),
        Setup::BlockedOrchestrator => {
            write(&root.join("blocker"), "");
            orch_dir = root.join("blocker").join("orch");
        }
        Setup::NoPermCwd => {
            let np = root.join("noperm");
            std::fs::create_dir_all(&np).expect("入れない dir を作れる");
            write(
                &orch.join("profiles/np.yaml"),
                &format!("cwd: \"{}\"\n", np.display()),
            );
            std::fs::set_permissions(&np, std::fs::Permissions::from_mode(0o000))
                .expect("権限を外せる");
            locked = Some(np.clone());
            // root は権限を外した dir にも入れる = この場を作れない
            if std::fs::read_dir(&np).is_ok() {
                std::fs::set_permissions(&np, std::fs::Permissions::from_mode(0o755))
                    .expect("権限を戻せる");
                return None;
            }
        }
    }
    let mut command = Command::new(tako());
    command
        .args(case.args)
        .current_dir(root)
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
        .env("TAKO_DATA_DIR", &data)
        .env("TAKO_DISCOVERY_DIR", root.join("disc"))
        .env("TAKO_ORCHESTRATOR_DIR", &orch_dir)
        .env("TAKO_SOCKET", root.join("none.sock"))
        .env("TAKO_983_LEGACY", "1")
        .env("HOME", &home);
    let output = command
        .output()
        .unwrap_or_else(|e| panic!("{}: tako を起こせない: {e}", case.name));
    if let Some(np) = locked {
        // 後片付け（ScratchDir の削除）が入れるよう戻す
        std::fs::set_permissions(&np, std::fs::Permissions::from_mode(0o755))
            .expect("権限を戻せる");
    }
    // GUI が無いので起動そのものは必ず失敗する（成功したら本番の GUI へ届いている恐れ）
    assert!(
        !output.status.success(),
        "{}: 隔離のはずが起動に成功した:\n{}",
        case.name,
        String::from_utf8_lossy(&output.stderr)
    );
    Some(format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}

#[test]
fn master_と_solo_の起動の流れの断りと行と移行の知らせは表示言語に従う() {
    let mut failures = Vec::new();
    let cases = cases();
    let mut ran = 0;
    for case in &cases {
        let (Some(en), Some(ja)) = (run(case, "en"), run(case, "ja")) else {
            continue;
        };
        ran += 1;
        if let Some(c) = has_japanese(&en) {
            failures.push(format!(
                "{}: 英語表示に日本語（{c}）が混ざる:\n{en}",
                case.name
            ));
        }
        if !ja.contains(case.ja) {
            failures.push(format!(
                "{}: 日本語表示が #2365 より前の文を含まない\n  want: {}\n  got:  {ja}",
                case.name, case.ja
            ));
        }
        // 移行の知らせは表示言語を決めてから組む（修正前は日本語表示でも英語で混ざった）
        if ja.contains(MIGRATED_EN) {
            failures.push(format!(
                "{}: 日本語表示に英語の移行の知らせが混ざる:\n{ja}",
                case.name
            ));
        }
    }
    assert!(ran > 0, "1 ケースも走らなかった");
    assert!(
        failures.is_empty(),
        "{} / {} ケースで落ちた:\n{}",
        failures.len(),
        ran * 2,
        failures.join("\n")
    );
}
