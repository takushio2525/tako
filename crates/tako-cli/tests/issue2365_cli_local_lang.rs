//! 実経路: CLI が手元で組む断りと、同じ出力に並ぶ注記・行が表示言語に従う（Issue #2365 の続き）。
//!
//! 修正前（実測）: 英語表示でも `tako orchestrator watch`（対象なし）は「ペイン ID または --worker を
//! 指定してください」、`tako queue --priority urgent` は「優先度は top / high / normal / low」、
//! `tako orchestrator layout` の `effective_policy` は「worker は常に spawn 元と同じタブへ置く…」の
//! ように日本語の固定文のまま出た（`main.rs` と dispatch の注記が日本語で固定していた）。
//!
//! GUI を通さない手元の処理なので、**ビルドした `tako` をそのまま**叩く（GUI 無し）: 一時の data dir
//! （settings.json の `language`）・orchestrator の置き場・HOME で `tako <引数>` を走らせ、標準出力と
//! 標準エラーをまとめて読む。見ること:
//! - 英語表示: 出力に日本語（かな・漢字）が 1 字も無い
//! - 日本語表示: #2365 より前の文（[`Case::ja`]）をそのまま含む
//!
//! A/B: `TAKO_2365_CLI_BIN=<修正前の tako>` で別のバイナリを叩ける（修正前は英語表示の検査で落ちる）。
//! 本番の data dir・GUI・順番待ちへ届かないよう、`TAKO_*` を全部外して一時の置き場と存在しない
//! ソケットを渡す（`tako queue` の断りは調停役へ繋ぐ前に出るが、念のため隔離の調停役の口を指す）。
//!
//! unix だけで走らせる: 消した作業ディレクトリでの起動（`current_dir()` の失敗）と `~/.claude` の
//! 展開が unix の前提。文の日英の対は OS に依らない 1 か所（`Reason` / `ReplyNote`）で、両 OS の番犬
//! （`crates/tako-control/tests/issue1762_dispatch_error_lang.rs`）が表で見る
#![cfg(unix)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tako_core::test_residue::ScratchDir;

/// 叩くバイナリ（既定はこの木でビルドした `tako`。A/B は `TAKO_2365_CLI_BIN`）
fn tako() -> PathBuf {
    std::env::var_os("TAKO_2365_CLI_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_tako")))
}

/// 1 ケースの砂場: data（表示言語）・discovery・orchestrator（プロファイル）・HOME・worker レジストリ
struct Sandbox {
    dir: ScratchDir,
}

impl Sandbox {
    fn new(lang: &str) -> Self {
        let dir = ScratchDir::new("issue2365-cli-local");
        let sb = Self { dir };
        for d in [
            sb.data(),
            sb.path().join("disc"),
            sb.path().join("home/.claude"),
            sb.orch().join("profiles"),
            sb.orch().join("solo-profiles"),
        ] {
            std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
        }
        // 書かないと system = 実行環境のロケールで決まり、CI と手元で答えが変わる
        write(
            &sb.data().join("settings.json"),
            &format!("{{\"language\":\"{lang}\"}}"),
        );
        // codex の worker で承認スキップ（既定）・サンドボックス解除なし / limit_resume を有効にした solo
        write(
            &sb.orch().join("profiles/cx.yaml"),
            "worker_agent: codex\nbypass_sandbox: false\n",
        );
        write(
            &sb.orch().join("solo-profiles/so.yaml"),
            "limit_resume: true\n",
        );
        write(
            &sb.orch().join("profiles/rc.yaml"),
            "remote_control: true\n",
        );
        // worker レジストリ: codex の worker 3・会話 ID の無い claude の worker 4
        write(
            &sb.data().join("workers.yaml"),
            "next_id: 5\nworkers:\n  '3':\n    label: cx\n    project: tako\n    agent: codex\n    \
             pane: 5\n    cwd: /tmp\n    spawned_at: '2026-10-29T00:00:00Z'\n    status: closed\n  \
             '4':\n    label: cl\n    project: tako\n    agent: claude\n    pane: 6\n    cwd: /tmp\n    \
             spawned_at: '2026-10-29T00:00:00Z'\n    status: closed\n",
        );
        sb
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }
    fn data(&self) -> PathBuf {
        self.path().join("data")
    }
    fn orch(&self) -> PathBuf {
        self.path().join("orch")
    }

    /// `TAKO_*` を外して隔離の置き場だけを渡した `tako`（`env` は追加の env）
    fn command(&self, args: &[&str], env: &[(&str, String)]) -> Command {
        let mut command = Command::new(tako());
        command
            .args(args)
            .current_dir(self.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("TAKO_") {
                command.env_remove(key);
            }
        }
        command
            .env_remove("ANTHROPIC_API_KEY")
            .env("TAKO_ISOLATED", "1")
            .env("TAKO_DATA_DIR", self.data())
            .env("TAKO_DISCOVERY_DIR", self.path().join("disc"))
            .env("TAKO_ORCHESTRATOR_DIR", self.orch())
            .env("TAKO_SOCKET", self.path().join("none.sock"))
            .env("HOME", self.path().join("home"));
        for (k, v) in env {
            command.env(k, v);
        }
        command
    }
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap_or_else(|e| panic!("{} を書ける: {e}", path.display()));
}

/// 叩き方
enum Via {
    /// 砂場の dir で `tako <args>`（`env` は追加の env、`stdin` は標準入力へ流すバイト列）
    Cli {
        args: &'static [&'static str],
        env: &'static [(&'static str, &'static str)],
        stdin: &'static [u8],
    },
    /// 消した作業ディレクトリから `tako <args>`（`current_dir()` が失敗する）
    GoneCwd(&'static [&'static str]),
}

struct Case {
    name: &'static str,
    via: Via,
    /// #2365 より前の日本語の文（日本語表示の出力にそのまま含まれる）
    ja: &'static str,
}

const fn cli(name: &'static str, args: &'static [&'static str], ja: &'static str) -> Case {
    Case {
        name,
        via: Via::Cli {
            args,
            env: &[],
            stdin: b"",
        },
        ja,
    }
}

/// 隔離の調停役の口（`tako queue` の断りは繋ぐ前に出るが、本番の順番待ちへ並ばないよう指す）
const QUEUE_SOCKET: &[(&str, &str)] = &[("TAKO_QUEUE_SOCKET", "@SANDBOX@/q.sock")];

/// 標準入力に流す UTF-8 でないバイト列（読み取りの失敗を起こす）
const NOT_UTF8: &[u8] = &[0xff, 0xfe, 0xfd];

fn cases() -> Vec<Case> {
    vec![
        // ---- CLI の手元の断り（Reason） ----
        cli(
            "watch_no_target",
            &["orchestrator", "watch"],
            "ペイン ID または --worker を指定してください（tako orchestrator watch <PANE_ID> / --worker <ID>）",
        ),
        cli(
            "resume_unknown",
            &["orchestrator", "resume", "99"],
            "worker 99 がレジストリに無い（`tako orchestrator workers --all` の worker_id を指定する）",
        ),
        cli(
            "resume_codex",
            &["orchestrator", "resume", "3"],
            "突然死からの復旧は claude の worker だけ（この worker は codex）",
        ),
        cli(
            "resume_no_session",
            &["orchestrator", "resume", "4"],
            "この worker は会話 ID が未検出なので再開できない（起動直後に落ちた・まだ走査されていない）",
        ),
        cli(
            "usage_show_no_gui",
            &["usage", "--show"],
            "tako が動いていないので右パネルには出せない（--show を外すとこのまま読める）",
        ),
        Case {
            name: "queue_priority",
            via: Via::Cli {
                args: &["queue", "--priority", "urgent", "true"],
                env: QUEUE_SOCKET,
                stdin: b"",
            },
            ja: "優先度は top / high / normal / low（受け取った値: urgent）",
        },
        Case {
            name: "queue_wait_unreadable",
            via: Via::Cli {
                args: &["queue", "--max-wait", "x", "true"],
                env: QUEUE_SOCKET,
                stdin: b"",
            },
            ja: "時間を読めない: x（例: 90s / 10m / 1h）",
        },
        Case {
            name: "queue_wait_unit",
            via: Via::Cli {
                args: &["queue", "--max-wait", "3d", "true"],
                env: QUEUE_SOCKET,
                stdin: b"",
            },
            ja: "時間の単位を読めない: d（s / m / h）",
        },
        Case {
            name: "queue_wait_zero",
            via: Via::Cli {
                args: &["queue", "--max-wait", "0s", "true"],
                env: QUEUE_SOCKET,
                stdin: b"",
            },
            ja: "待ちの上限は 1 秒以上",
        },
        cli(
            "queue_serve_no_socket",
            &["queue", "serve"],
            "TAKO_QUEUE_SOCKET が要る（本番の調停役と取り合わないよう、隔離の調停役は明示したときだけ立てる）",
        ),
        Case {
            name: "queue_serve_no_token",
            via: Via::Cli {
                args: &["queue", "serve"],
                env: QUEUE_SOCKET,
                stdin: b"",
            },
            ja: "TAKO_TOKEN が要る（繋ぐ側の tako queue も同じ値を持つ）",
        },
        Case {
            name: "handoffs_stdin",
            via: Via::Cli {
                args: &["orchestrator", "handoffs", "write", "--project", "tako"],
                env: &[],
                stdin: NOT_UTF8,
            },
            ja: "エラー: 標準入力の読み取りに失敗: stream did not contain valid UTF-8",
        },
        Case {
            name: "context_budget_gone_cwd",
            via: Via::GoneCwd(&["context-budget"]),
            ja: "cwd が取れない: No such file or directory (os error 2)",
        },
        Case {
            name: "setup_mcp_gone_cwd",
            via: Via::GoneCwd(&["setup-mcp", "--project"]),
            ja: "カレントディレクトリの取得に失敗: No such file or directory (os error 2)",
        },
        // ---- 同じ出力に並ぶ行・応答の注記（ReplyNote） ----
        cli(
            "telemetry_status",
            &["telemetry", "status"],
            "  直近のレポート件数: 0",
        ),
        cli(
            "layout_shrink",
            &["orchestrator", "layout"],
            "worker は常に spawn 元と同じタブへ置く（タブ分けなし）。下限 60 桁を割るときは worker ペインのフォントを自動で縮め、床は既定サイズの 60%（届かなければ床で置き cols_short を立てる）",
        ),
        cli(
            "layout_no_guarantee",
            &["orchestrator", "layout", "--min-worker-cols", "0"],
            "worker は常に spawn 元と同じタブへ置く（タブ分けなし）。桁数の保証なし（min_worker_cols=0）",
        ),
        cli(
            "layout_no_shrink",
            &["orchestrator", "layout", "--auto-shrink-font", "false"],
            "worker は常に spawn 元と同じタブへ置く（タブ分けなし）。下限 60 桁を割っても自動縮小はしない（auto_shrink_font=false）",
        ),
        cli(
            "profiles_codex",
            &["orchestrator", "profiles", "show", "cx"],
            "codex は承認スキップとサンドボックス解除が同じフラグ（--dangerously-bypass-approvals-and-sandbox）なので、bypass_sandbox が false のあいだ skip_permissions は効きません（コマンド実行に承認プロンプトが出ます）。",
        ),
        cli(
            "profiles_solo",
            &["orchestrator", "profiles", "show", "so", "--solo"],
            "solo プロファイルは worker を spawn しないので、limit_resume が効くのは `tako solo` で立てた本人のペインだけです（worker への配布はありません）。",
        ),
        Case {
            name: "profiles_remote_control",
            via: Via::Cli {
                args: &["orchestrator", "profiles", "show", "rc"],
                env: &[("ANTHROPIC_API_KEY", "dummy")],
                stdin: b"",
            },
            // 理由（どの env で止まったか）は実行環境で変わりうるので枠だけを見る
            ja: "remote_control: true ですが、この環境では有効にできません（",
        },
        cli(
            "accounts_default_dir",
            &["orchestrator", "accounts", "add", "ac", "--config-dir", "~/.claude"],
            "config_dir '~/.claude' は claude の既定パスです。既定パスを明示指定すると CLAUDE_CONFIG_DIR が設定された状態になり",
        ),
    ]
}

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

/// 1 ケースを `lang` で叩き、標準出力と標準エラーをまとめて返す
fn run(case: &Case, lang: &str) -> String {
    let sb = Sandbox::new(lang);
    let root = sb.path().display().to_string();
    let output = match &case.via {
        Via::Cli { args, env, stdin } => {
            let env: Vec<(&str, String)> = env
                .iter()
                .map(|(k, v)| (*k, v.replace("@SANDBOX@", &root)))
                .collect();
            let mut child = sb
                .command(args, &env)
                .spawn()
                .unwrap_or_else(|e| panic!("{}: tako を起こせない: {e}", case.name));
            child
                .stdin
                .take()
                .expect("stdin")
                .write_all(stdin)
                .expect("stdin へ書ける");
            child.wait_with_output().expect("tako が終わる")
        }
        Via::GoneCwd(args) => {
            let gone = sb.path().join("gone");
            std::fs::create_dir_all(&gone).expect("消す dir を作れる");
            // 同じ隔離の env（`TAKO_*` を外して置き場を渡す）を写した sh から、消した dir で exec する
            let base = sb.command(&[], &[]);
            let mut command = Command::new("/bin/sh");
            command
                .arg("-c")
                .arg("cd \"$1\" && rmdir \"$1\" && shift && exec \"$@\"")
                .arg("sh")
                .arg(&gone)
                .arg(tako())
                .args(*args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            for (key, value) in base.get_envs() {
                match value {
                    Some(v) => command.env(key, v),
                    None => command.env_remove(key),
                };
            }
            command.output().expect("tako が終わる")
        }
    };
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn cli_の手元の断りと注記は表示言語に従う() {
    let mut failures = Vec::new();
    let cases = cases();
    for case in &cases {
        let en = run(case, "en");
        if let Some(c) = has_japanese(&en) {
            failures.push(format!(
                "{}: 英語表示に日本語（{c}）が混ざる:\n{en}",
                case.name
            ));
        }
        let ja = run(case, "ja");
        if !ja.contains(case.ja) {
            failures.push(format!(
                "{}: 日本語表示が #2365 より前の文を含まない\n  want: {}\n  got:  {ja}",
                case.name, case.ja
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} / {} ケースで落ちた:\n{}",
        failures.len(),
        cases.len() * 2,
        failures.join("\n")
    );
}
