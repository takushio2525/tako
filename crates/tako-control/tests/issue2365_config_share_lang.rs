//! 実経路: AI 系設定の共有（CLI `tako config` / MCP `tako_config_share`）の断りが表示言語に従う
//! （Issue #2365 の続き）。
//!
//! 修正前（実測。CLI と MCP が通る同じ口 = `dispatch::dispatch_config_share` を叩いて）: 英語表示でも
//! 「設定共有が未配線です。…」「ディレクトリが見つかりません: …」「git clone … が失敗しました:」のように
//! 日本語のまま = dispatch が `config_share`（`mod.rs` / `portable.rs` / `guard.rs`）の `Err(String)`
//! （日本語の固定文）を素通ししていた。応答の値に載る誤り文（`push` の `push_error`・`status` の
//! `files[].error`）も同じ日本語だった。
//!
//! 置き場（HOME / data / claude の設定 / git の設定）はすべて一時の場所へ向ける。共有リポジトリと
//! そのリモートも一時の場所（リモートは無いパス = 本物のリモートへ届かない・bare リポジトリ）。
//! git は一時の包み（`TAKO_GIT_BIN`）越しに起こし、包みを外して「git を起動できない」を作る。
//!
//! - 英語表示: 断りに日本語（かな・漢字）が 1 字も無い。外部の文（git 自身の stderr）は日英とも同じ文
//! - 日本語表示: #2365 より前の文とバイト一致
//! - 表示言語を切り替えた直後の 1 件目から、切り替えた言語で返る
//!
//! 表示言語と env はプロセス全体の状態なので、**1 本のテスト関数を `run_alone!` の子**で走らせる
//! （#2400）。このファイルは修正前の木でもコンパイルできる API だけを使う（A/B: 修正前は英語表示の
//! 検査で落ちる）
//!
//! unix だけで走らせる: 読めない置き場（パーミッション 000 / 0500）と git の包み（sh のスクリプト）が
//! unix の前提
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tako_control::dispatch::dispatch_config_share;
use tako_core::i18n::{self, Lang};

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

const EACCES: &str = "Permission denied (os error 13)";
const SECRET_MD: &str = "# rules\ntoken: EXAMPLEEXAMPLEEXAMPLEEXAMPLE\n";
const BROKEN_YAML: &str = "accounts:\n  - [unbalanced\n";

/// 一時の置き場
struct Ctx {
    root: PathBuf,
    data: PathBuf,
    claude: PathBuf,
    /// 中身のある・git でないディレクトリ
    plain: PathBuf,
    /// 名前を持つ git リポジトリ（リモートは無いパス）
    good: PathBuf,
    /// 名前を持たない git リポジトリ（`user.useConfigOnly` = 名前を推測しない）
    noident: PathBuf,
    /// 実在しない git のリモート（clone / pull / push はここへ向ける = 本物へ届かない）
    nope_git: PathBuf,
    /// git の包み（外すと「git を起動できない」）
    git_wrap: PathBuf,
    /// 権限で断る経路を作れるか（root ではパーミッションが効かない）
    perms: bool,
}

impl Ctx {
    fn state(&self) -> PathBuf {
        self.data.join("config-share.json")
    }

    fn link_state(&self, repo: &Path) {
        std::fs::create_dir_all(&self.data).unwrap();
        std::fs::write(
            self.state(),
            serde_json::json!({ "repo": repo.to_string_lossy() }).to_string(),
        )
        .unwrap();
    }

    fn unlink_state(&self) {
        let _ = std::fs::remove_file(self.state());
    }

    fn accounts(&self) -> PathBuf {
        self.data.join("orchestrator").join("accounts.yaml")
    }

    fn claude_md(&self) -> PathBuf {
        self.claude.join("CLAUDE.md")
    }

    /// 共有対象を 1 つだけ置く（毎回中身を変える = push がコミットまで進む）
    fn fresh_share(&self, n: usize) {
        let _ = std::fs::remove_file(self.accounts());
        std::fs::create_dir_all(&self.claude).unwrap();
        std::fs::write(self.claude_md(), format!("# rules\n- run {n}\n")).unwrap();
    }
}

fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git を起こせる")
}

fn git_ok(dir: &Path, args: &[&str]) {
    let out = git(dir, args);
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// git 自身の stderr（`config_share::git` と同じく前後の空白を落とす）
fn git_stderr(dir: &Path, args: &[&str]) -> String {
    String::from_utf8_lossy(&git(dir, args).stderr)
        .trim()
        .to_string()
}

fn set_mode(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// #2365 より前の日本語の文の形
enum Old {
    /// 文全体
    Exact(String),
    /// 頭と尻（間は一時ファイルの pid・連番）
    Around(String, String),
}

/// 前準備（`n` は何回目か）
type Prep = Box<dyn Fn(&Ctx, usize)>;
/// 叩く（断りの文か、応答の値の誤り文）
type Act = Box<dyn Fn(&Ctx) -> String>;
/// #2365 より前の文
type OldOf = Box<dyn Fn(&Ctx) -> Old>;
/// 後片付け（権限を戻す・env を戻す）
type After = Box<dyn Fn(&Ctx)>;

/// 1 件の叩き方
struct Case {
    name: &'static str,
    prep: Prep,
    act: Act,
    old: OldOf,
    after: After,
}

fn case(
    name: &'static str,
    prep: impl Fn(&Ctx, usize) + 'static,
    act: impl Fn(&Ctx) -> String + 'static,
    old: impl Fn(&Ctx) -> Old + 'static,
) -> Case {
    Case {
        name,
        prep: Box::new(prep),
        act: Box::new(act),
        old: Box::new(old),
        after: Box::new(|_| {}),
    }
}

impl Case {
    fn after(mut self, after: impl Fn(&Ctx) + 'static) -> Self {
        self.after = Box::new(after);
        self
    }
}

/// dispatch の口（CLI `tako config` と MCP `tako_config_share` が通る）。断りは表示言語の文、
/// 通ったら `<ok>` と応答
fn share(
    action: &str,
    target: Option<&str>,
    path: Option<&str>,
    remote: Option<&str>,
    message: Option<&str>,
) -> String {
    match dispatch_config_share(action, target, path, remote, message, false) {
        Err(e) => e.to_string(),
        Ok(v) => format!("<ok> {v}"),
    }
}

/// 通ったときの応答（値の誤り文を読む）
fn share_value(action: &str, message: Option<&str>) -> Result<Value, String> {
    dispatch_config_share(action, None, None, None, message, false).map_err(|e| e.to_string())
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

fn cases(ctx: &Ctx) -> Vec<Case> {
    let mut out = vec![
        case(
            "link（対象が空白だけ）",
            |c, _| c.unlink_state(),
            |_| share("link", Some("   "), None, None, None),
            |_| Old::Exact("リポジトリのパスまたは URL を指定してください".into()),
        ),
        case(
            "link（ディレクトリが無い）",
            |c, _| c.unlink_state(),
            |c| share("link", Some(&s(&c.root.join("nope"))), None, None, None),
            |c| {
                Old::Exact(format!(
                    "ディレクトリが見つかりません: {}",
                    c.root.join("nope").display()
                ))
            },
        ),
        case(
            "link（git でない）",
            |c, _| c.unlink_state(),
            |c| share("link", Some(&s(&c.plain)), None, None, None),
            |c| {
                let p = c.plain.display();
                Old::Exact(format!(
                    "{p} は git リポジトリではありません。\n\
                     新規に作るなら `tako config init --path {p}` を使ってください"
                ))
            },
        ),
        case(
            "link（clone 先が空でない）",
            |c, _| c.unlink_state(),
            |c| {
                share(
                    "link",
                    Some(&s(&c.nope_git)),
                    Some(&s(&c.plain)),
                    None,
                    None,
                )
            },
            |c| {
                Old::Exact(format!(
                    "clone 先 {} が空ではありません。--path で別の場所を指定してください",
                    c.plain.display()
                ))
            },
        ),
        case(
            "link（clone の失敗 = 無いリモート）",
            |c, _| {
                c.unlink_state();
                let _ = std::fs::remove_dir_all(c.root.join("clonedest"));
            },
            |c| {
                share(
                    "link",
                    Some(&s(&c.nope_git)),
                    Some(&s(&c.root.join("clonedest"))),
                    None,
                    None,
                )
            },
            |c| {
                let (url, dest) = (s(&c.nope_git), s(&c.root.join("clonedest")));
                let stderr = git_stderr(&c.root, &["clone", &url, &dest]);
                Old::Exact(format!("git clone {url} {dest} が失敗しました:\n{stderr}"))
            },
        ),
        case(
            "init（空でない・git でない）",
            |c, _| c.unlink_state(),
            |c| share("init", None, Some(&s(&c.plain)), None, None),
            |c| {
                Old::Exact(format!(
                    "{} は空でないディレクトリで、git リポジトリでもありません。\n\
                     別の場所を --path で指定するか、そこで git init してから `tako config link` してください",
                    c.plain.display()
                ))
            },
        ),
        case(
            "init（ディレクトリを作れない = 親がファイル）",
            |c, _| c.unlink_state(),
            |c| share("init", None, Some(&s(&c.root.join("afile/sub"))), None, None),
            |_| Old::Exact("ディレクトリの作成に失敗: Not a directory (os error 20)".into()),
        ),
        case(
            "init（HOME 未設定 = 既定の置き場を決められない）",
            |c, _| {
                c.unlink_state();
                std::env::set_var("HOME", "");
            },
            |_| share("init", None, None, None, None),
            |_| Old::Exact("ホームディレクトリを解決できません（$HOME 未設定）".into()),
        )
        .after(|c| std::env::set_var("HOME", c.root.join("home"))),
        case(
            "push（未配線）",
            |c, _| c.unlink_state(),
            |_| share("push", None, None, None, None),
            |_| {
                Old::Exact(
                    "設定共有が未配線です。`tako config init` で新規リポジトリを作るか、\n\
                     `tako config link <パス|URL>` で既存リポジトリに繋いでください"
                        .into(),
                )
            },
        ),
        case(
            "pull（未配線）",
            |c, _| c.unlink_state(),
            |_| share("pull", None, None, None, None),
            |_| {
                Old::Exact(
                    "設定共有が未配線です。`tako config init` で新規リポジトリを作るか、\n\
                     `tako config link <パス|URL>` で既存リポジトリに繋いでください"
                        .into(),
                )
            },
        ),
        case(
            "status（配線状態が壊れた JSON）",
            |c, _| {
                std::fs::create_dir_all(&c.data).unwrap();
                std::fs::write(c.state(), "{").unwrap();
            },
            |_| share("status", None, None, None, None),
            |c| {
                let e = serde_json::from_str::<Value>("{").unwrap_err();
                Old::Exact(format!(
                    "配線状態のパースに失敗 ({}): {}",
                    c.state().display(),
                    tako_core::load_issue::json_error_reason(&e)
                ))
            },
        )
        .after(|c| c.unlink_state()),
        case(
            "push（配線先が git でない）",
            |c, _| c.link_state(&c.plain),
            |_| share("push", None, None, None, None),
            |c| {
                Old::Exact(format!(
                    "配線先が git リポジトリではありません: {}\n\
                     `tako config link <パス|URL>` で繋ぎ直してください",
                    c.plain.display()
                ))
            },
        )
        .after(|c| c.unlink_state()),
        case(
            "push（秘匿情報らしき内容）",
            |c, _| {
                c.link_state(&c.good);
                let _ = std::fs::remove_file(c.accounts());
                std::fs::create_dir_all(&c.claude).unwrap();
                std::fs::write(c.claude_md(), SECRET_MD).unwrap();
            },
            |_| share("push", None, None, None, Some("sync")),
            |_| {
                Old::Exact(
                    "秘匿情報らしき内容が共有対象に含まれています。取り除いてから再実行してください:\n  \
                     - claude/CLAUDE.md:2 (secret-looking assignment)"
                        .into(),
                )
            },
        )
        .after(|c| c.fresh_share(0)),
        case(
            "push（壊れた YAML）",
            |c, _| {
                c.link_state(&c.good);
                std::fs::create_dir_all(c.accounts().parent().unwrap()).unwrap();
                std::fs::write(c.accounts(), BROKEN_YAML).unwrap();
            },
            |_| share("push", None, None, None, Some("sync")),
            |_| Old::Exact(broken_yaml_old()),
        ),
        case(
            "status の files[].error（壊れた YAML）",
            |c, _| {
                c.link_state(&c.good);
                std::fs::create_dir_all(c.accounts().parent().unwrap()).unwrap();
                std::fs::write(c.accounts(), BROKEN_YAML).unwrap();
            },
            |_| match share_value("status", None) {
                Err(e) => format!("<err> {e}"),
                Ok(v) => v["files"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|f| f["path"] == "tako/orchestrator/accounts.yaml")
                    .and_then(|f| f["error"].as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("<no error> {v}")),
            },
            |_| Old::Exact(broken_yaml_old()),
        )
        .after(|c| c.fresh_share(0)),
        case(
            "push（git の名前が未設定）",
            |c, n| {
                c.link_state(&c.noident);
                c.fresh_share(100 + n);
            },
            |_| share("push", None, None, None, Some("sync")),
            |c| {
                // push が stage まで済ませた状態で、同じ commit が返す git の文
                let stderr = git_stderr(&c.noident, &["commit", "-m", "sync"]);
                Old::Exact(format!(
                    "git commit -m sync が失敗しました:\n{stderr}\n\n\
                     このデバイスに git の名前が未設定です。次を実行してから再試行してください:\n  \
                     git config --global user.name \"あなたの名前\"\n  \
                     git config --global user.email \"you@example.com\""
                ))
            },
        ),
        case(
            "pull（ff-only の失敗 = 無いリモート）",
            |c, _| c.link_state(&c.good),
            |_| share("pull", None, None, None, None),
            |c| {
                let stderr = git_stderr(&c.good, &["pull", "--ff-only"]);
                let repo = c.good.display();
                Old::Exact(format!(
                    "git pull --ff-only が失敗しました:\n{stderr}\n\n\
                     両方のデバイスで同じ設定を変更した可能性があります。次のどちらかで解決してください:\n\
                     1. 共有リポジトリで解決する:\n   \
                        cd {repo} && git pull --rebase   # 競合したら中身を直して git rebase --continue\n\
                     2. このデバイスの内容を正とする:\n   \
                        cd {repo} && git rebase --abort（実行中なら）→ tako config push\n\
                     解決後にもう一度 `tako config pull` を実行してください"
                ))
            },
        ),
        case(
            "push の push_error（無いリモートへの push）",
            |c, n| {
                c.link_state(&c.good);
                c.fresh_share(200 + n);
            },
            |_| match share_value("push", Some("sync")) {
                Err(e) => format!("<err> {e}"),
                Ok(v) => v["push_error"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("<no push_error> {v}")),
            },
            |c| {
                let first = git_stderr(&c.good, &["push"]);
                let second = git_stderr(&c.good, &["push", "-u", "origin", "HEAD"]);
                Old::Exact(format!(
                    "git push が失敗しました:\n{first}\ngit push -u origin HEAD が失敗しました:\n{second}"
                ))
            },
        ),
        case(
            "init（git を起動できない）",
            |c, _| {
                c.unlink_state();
                let _ = std::fs::remove_dir_all(c.root.join("fresh"));
                std::fs::rename(&c.git_wrap, c.git_wrap.with_extension("off")).unwrap();
            },
            |c| share("init", None, Some(&s(&c.root.join("fresh"))), None, None),
            |_| {
                Old::Exact("git の実行に失敗 (init): No such file or directory (os error 2)".into())
            },
        )
        .after(|c| {
            std::fs::rename(c.git_wrap.with_extension("off"), &c.git_wrap).unwrap();
            let _ = std::fs::remove_dir_all(c.root.join("fresh"));
        }),
    ];
    if ctx.perms {
        out.extend([
            case(
                "status（配線状態が読めない）",
                |c, _| {
                    c.link_state(&c.good);
                    set_mode(&c.state(), 0o000);
                },
                |_| share("status", None, None, None, None),
                |c| {
                    Old::Exact(format!(
                        "配線状態の読み取りに失敗 ({}): {EACCES}",
                        c.state().display()
                    ))
                },
            )
            .after(|c| {
                set_mode(&c.state(), 0o644);
                c.unlink_state();
            }),
            case(
                "unlink（配線状態を消せない）",
                |c, _| {
                    c.link_state(&c.good);
                    set_mode(&c.data, 0o500);
                },
                |_| share("unlink", None, None, None, None),
                |_| Old::Exact(format!("配線状態の削除に失敗: {EACCES}")),
            )
            .after(|c| {
                set_mode(&c.data, 0o755);
                c.unlink_state();
            }),
            case(
                "link（配線状態を書けない）",
                |c, _| {
                    c.unlink_state();
                    set_mode(&c.data, 0o500);
                },
                |c| share("link", Some(&s(&c.good)), None, None, None),
                |c| {
                    Old::Around(
                        format!("一時ファイルの書き込みに失敗 ({}.tmp.", c.state().display()),
                        format!("): {EACCES}"),
                    )
                },
            )
            .after(|c| set_mode(&c.data, 0o755)),
            case(
                "push（共有対象を読めない）",
                |c, _| {
                    c.link_state(&c.good);
                    std::fs::create_dir_all(c.accounts().parent().unwrap()).unwrap();
                    std::fs::write(c.accounts(), "accounts: {}\n").unwrap();
                    set_mode(&c.accounts(), 0o000);
                },
                |_| share("push", None, None, None, Some("sync")),
                |c| {
                    Old::Exact(format!(
                        "読み取りに失敗 ({}): {EACCES}",
                        c.accounts().display()
                    ))
                },
            )
            .after(|c| {
                set_mode(&c.accounts(), 0o644);
                c.fresh_share(0);
            }),
        ]);
    }
    out
}

/// 壊れた accounts.yaml の断り（#2365 より前の `portable::strip_local_fields` の文）
fn broken_yaml_old() -> String {
    let e = serde_yaml::from_str::<serde_yaml::Value>(BROKEN_YAML).unwrap_err();
    format!(
        "YAML のパースに失敗: {}",
        tako_core::load_issue::yaml_error_reason(&e, BROKEN_YAML)
    )
}

fn matches_old(got: &str, old: &Old) -> Result<(), String> {
    match old {
        Old::Exact(want) if got == want => Ok(()),
        Old::Exact(want) => Err(format!("want: {want}\n  got:  {got}")),
        Old::Around(head, tail)
            if got.starts_with(head.as_str()) && got.ends_with(tail.as_str()) =>
        {
            Ok(())
        }
        Old::Around(head, tail) => Err(format!("want: {head}…{tail}\n  got:  {got}")),
    }
}

fn run(ctx: &Ctx, c: &Case, n: usize) -> (String, Old) {
    (c.prep)(ctx, n);
    let got = (c.act)(ctx);
    let old = (c.old)(ctx);
    (c.after)(ctx);
    (got, old)
}

fn setup() -> Ctx {
    let root = std::env::temp_dir().join(format!("t2365cs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("一時の置き場");
    let root = root.canonicalize().expect("正規化");
    // 本番の設定・データ・claude の設定・git の設定を読み書きしない（どの置き場も一時の場所の下）
    std::env::set_var("HOME", root.join("home"));
    std::env::remove_var("USERPROFILE");
    std::env::remove_var("CLAUDE_CONFIG_DIR");
    std::env::set_var("TAKO_DATA_DIR", root.join("data"));
    std::env::set_var("TAKO_DISCOVERY_DIR", root.join("discovery"));
    std::env::set_var("XDG_CONFIG_HOME", root.join("xdg"));
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    std::env::set_var("GIT_CONFIG_GLOBAL", root.join("gitconfig"));
    for name in [
        "GIT_AUTHOR_NAME",
        "GIT_AUTHOR_EMAIL",
        "GIT_COMMITTER_NAME",
        "GIT_COMMITTER_EMAIL",
        "EMAIL",
        "GIT_DIR",
        "GIT_WORK_TREE",
    ] {
        std::env::remove_var(name);
    }
    // git の文は英語に固定する（外部の文が日英で同じことを見る）
    std::env::set_var("LC_ALL", "C");
    std::env::set_var("LANG", "C");
    std::fs::create_dir_all(root.join("home")).unwrap();
    std::fs::write(root.join("gitconfig"), "[init]\n\tdefaultBranch = main\n").unwrap();

    // git の包み（`TAKO_GIT_BIN` は最初の解決で固定される = 外すと起動できない）
    let real = String::from_utf8(
        std::process::Command::new("/bin/sh")
            .args(["-c", "command -v git"])
            .output()
            .expect("git を探せる")
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let git_wrap = root.join("git-wrap");
    std::fs::write(&git_wrap, format!("#!/bin/sh\nexec \"{real}\" \"$@\"\n")).unwrap();
    set_mode(&git_wrap, 0o755);
    std::env::set_var("TAKO_GIT_BIN", &git_wrap);

    let plain = root.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(plain.join("x.txt"), "x\n").unwrap();
    std::fs::write(root.join("afile"), "file\n").unwrap();

    let nope_git = root.join("nope.git");
    let good = root.join("good");
    std::fs::create_dir_all(&good).unwrap();
    git_ok(&good, &["init", "-q"]);
    git_ok(&good, &["config", "user.name", "t"]);
    git_ok(&good, &["config", "user.email", "t@example.invalid"]);
    git_ok(&good, &["remote", "add", "origin", &s(&nope_git)]);
    let noident = root.join("noident");
    std::fs::create_dir_all(&noident).unwrap();
    git_ok(&noident, &["init", "-q"]);
    git_ok(&noident, &["config", "user.useConfigOnly", "true"]);

    let data = root.join("data");
    std::fs::create_dir_all(&data).unwrap();
    // 権限で断る経路を作れるか（root ではパーミッションが効かない）
    let probe = root.join("probe");
    std::fs::write(&probe, "x").unwrap();
    set_mode(&probe, 0o000);
    let perms = std::fs::read(&probe).is_err();
    set_mode(&probe, 0o644);

    let ctx = Ctx {
        claude: root.join("home").join(".claude"),
        data,
        plain,
        good,
        noident,
        nope_git,
        git_wrap,
        perms,
        root,
    };
    ctx.fresh_share(0);
    ctx
}

#[test]
fn config_shareの断りは表示言語に従う() {
    if tako_core::run_alone!(config_shareの断りは表示言語に従う) {
        return;
    }
    let ctx = setup();
    if !ctx.perms {
        eprintln!("パーミッションが効かない（root）ので権限のケースを外す");
    }

    let original = i18n::lang();
    let mut failures = Vec::new();
    let (mut ja_count, mut en_count) = (0, 0);
    let mut by_lang: Vec<Vec<String>> = Vec::new();
    for (round, lang) in [Lang::En, Lang::Ja].into_iter().enumerate() {
        i18n::set_lang(lang);
        let mut seen = Vec::new();
        for c in cases(&ctx) {
            let (got, old) = run(&ctx, &c, round);
            println!("[{lang:?}] {}: {got}", c.name);
            match lang {
                Lang::En => {
                    en_count += 1;
                    if got.starts_with('<') {
                        failures.push(format!("[en] {}: 断られていない: {got}", c.name));
                    } else if let Some(ch) = has_japanese(&got) {
                        failures.push(format!("[en] {}: 日本語（{ch}）が混ざる: {got}", c.name));
                    }
                }
                Lang::Ja => {
                    ja_count += 1;
                    if let Err(e) = matches_old(&got, &old) {
                        failures.push(format!("[ja] {}: #2365 より前と違う\n  {e}", c.name));
                    }
                }
            }
            seen.push(got);
        }
        by_lang.push(seen);
    }
    // 英語表示の文は日本語表示の文と違う（英語の書き忘れで日本語を写していない）
    for ((c, en), ja) in cases(&ctx).iter().zip(&by_lang[0]).zip(&by_lang[1]) {
        if en == ja {
            failures.push(format!("{}: 日英が同じ文: {en}", c.name));
        }
    }

    // 表示言語を切り替えた直後の 1 件目から、切り替えた言語で返る（毎回切り替えて 1 件ずつ叩く）
    let probes = cases(&ctx);
    let names = [
        "push（未配線）",
        "link（ディレクトリが無い）",
        "pull（ff-only の失敗 = 無いリモート）",
    ];
    let picks: Vec<usize> = names
        .iter()
        .filter_map(|name| probes.iter().position(|c| c.name == *name))
        .collect();
    assert_eq!(
        picks.len(),
        names.len(),
        "切り替えの見本の名前が cases に無い"
    );
    for (i, lang) in [Lang::Ja, Lang::En, Lang::Ja, Lang::En, Lang::Ja, Lang::En]
        .into_iter()
        .enumerate()
    {
        i18n::set_lang(lang);
        let c = &probes[picks[i % picks.len()]];
        let (got, old) = run(&ctx, c, 10 + i);
        let ok = match lang {
            Lang::Ja => matches_old(&got, &old).is_ok(),
            Lang::En => has_japanese(&got).is_none() && !got.starts_with('<'),
        };
        if !ok {
            failures.push(format!(
                "切り替え {i} 回目（{lang:?}）の直後の 1 件目（{}）: {got}",
                c.name
            ));
        }
    }

    // 断られない経路は今までどおり: 一時の bare リポジトリへ push が届き、応答に誤り文が無い
    let bare = ctx.root.join("bare.git");
    git_ok(&ctx.root, &["init", "-q", "--bare", &s(&bare)]);
    git_ok(&ctx.good, &["remote", "set-url", "origin", &s(&bare)]);
    ctx.link_state(&ctx.good);
    for (i, lang) in [Lang::En, Lang::Ja].into_iter().enumerate() {
        i18n::set_lang(lang);
        ctx.fresh_share(300 + i);
        match share_value("push", Some("sync")) {
            Ok(v) if v["pushed"] == true && v["push_error"].is_null() => {}
            other => failures.push(format!("[{lang:?}] bare への push が通らない: {other:?}")),
        }
    }
    i18n::set_lang(original);

    let _ = std::fs::remove_dir_all(&ctx.root);
    eprintln!(
        "issue2365_config_share_lang: 英語 {en_count} 件・日本語 {ja_count} 件・失敗 {}",
        failures.len()
    );
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
