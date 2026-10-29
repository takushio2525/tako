//! 実経路: CLI と MCP の前段（dispatch へ送る前に手元で断る口）の断りが表示言語に従う（Issue #2365 の続き）。
//!
//! 修正前（実測）: 英語表示でも `tako edit move diagonal` は「移動の単位は … のどれか」、MCP の
//! `tako_window` の綴りの誤りは「action が不正: …」のように、前段が書いた日本語の固定文のまま返った。
//! 前段は GUI へ届く前に断るので、ここでは**ビルドした `tako` をそのまま**叩く（GUI 無し）:
//! - CLI: 一時の data dir（settings.json の `language`）で `tako <引数>` を走らせ、標準エラーの断りを読む
//! - MCP: 同じ data dir で `tako mcp serve` へ JSON-RPC の行を流し、断り（`error.message` /
//!   `isError` の本文）を読む。接続情報は存在しないソケット（GUI が居ない間の呼び出しと同じ）
//!
//! 見ること: 英語表示の断りに日本語（かな・漢字）が 1 字も無い / 日本語表示は #2365 より前の文と
//! バイト一致（[`cases`] の `ja`）。どのケースも手元で断られて何も書かない・どこへも送らない。
//!
//! A/B: `TAKO_2365_FRONT_BIN=<修正前の tako>` で別のバイナリを叩ける（このファイルは修正前の木でも
//! コンパイルできる口だけを使う）。修正前は英語表示の検査で落ち、件数を出す。
//! 本番の data dir・GUI へ届かないよう、`TAKO_*` を全部外して一時の data dir と存在しないソケットを渡す

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

/// 叩くバイナリ（既定はこの木でビルドした `tako`。A/B は `TAKO_2365_FRONT_BIN`）
fn tako() -> PathBuf {
    std::env::var_os("TAKO_2365_FRONT_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_tako")))
}

/// テストごとの一時 dir（終わったら消す）
struct Scratch(PathBuf);

impl Scratch {
    fn new(lang: &str) -> Self {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tk2365f-{lang}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let data = dir.join("data");
        std::fs::create_dir_all(&data).expect("data dir を作れる");
        // 書かないと system = 実行環境のロケールで決まり、CI と手元で答えが変わる
        std::fs::write(
            data.join("settings.json"),
            format!("{{\"language\":\"{lang}\"}}"),
        )
        .expect("settings.json を書ける");
        Self(dir)
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(tako());
        command
            .args(args)
            .current_dir(&self.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("TAKO_") {
                command.env_remove(key);
            }
        }
        command
            .env("TAKO_ISOLATED", "1")
            .env("TAKO_DATA_DIR", self.0.join("data"))
            .env("TAKO_DISCOVERY_DIR", self.0.join("disc"))
            .env("HOME", self.0.join("home"));
        command
    }

    /// 存在しないソケット（tako の中から呼んだが GUI が居ない形）
    fn socket(&self) -> String {
        if cfg!(windows) {
            format!(r"\\.\pipe\tako-2365f-none-{}", std::process::id())
        } else {
            self.0.join("none.sock").display().to_string()
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 引き出し方
enum Via {
    /// `tako <args>`（`stdin` は標準入力へ流すバイト列）
    Cli {
        args: &'static [&'static str],
        stdin: &'static [u8],
    },
    /// MCP の tools/call（`args` は arguments の JSON）
    Mcp {
        tool: &'static str,
        args: &'static str,
    },
    /// MCP の生の 1 行（JSON-RPC の口）
    Rpc(&'static str),
}

struct Case {
    name: &'static str,
    via: Via,
    /// #2365 より前の日本語の断り（バイト一致）
    ja: &'static str,
}

const fn cli(name: &'static str, args: &'static [&'static str], ja: &'static str) -> Case {
    Case {
        name,
        via: Via::Cli { args, stdin: b"" },
        ja,
    }
}

const fn mcp(name: &'static str, tool: &'static str, args: &'static str, ja: &'static str) -> Case {
    Case {
        name,
        via: Via::Mcp { tool, args },
        ja,
    }
}

/// 標準入力に流す UTF-8 でないバイト列（読み取りの失敗を起こす）
const NOT_UTF8: &[u8] = &[0xff, 0xfe, 0xfd];

fn cases() -> Vec<Case> {
    vec![
        // ---- CLI の前段（build_request とそれが呼ぶ補助）----
        cli(
            "resize",
            &["resize", "--pane", "1"],
            "--dx / --dy / --share-x / --share-y のどれか 1 つを指定する",
        ),
        cli(
            "paste_guard_text",
            &["paste-guard", "on", "abc"],
            "文字を渡せるのは `tako paste-guard check|allow|unallow|rule` だけ",
        ),
        cli(
            "paste_guard_unallow",
            &["paste-guard", "unallow"],
            "外す許可の ID（`tako paste-guard` の allow[].id）か all を渡す",
        ),
        Case {
            name: "paste_guard_stdin",
            via: Via::Cli {
                args: &["paste-guard", "check"],
                stdin: NOT_UTF8,
            },
            ja: "標準入力の読み取りに失敗: stream did not contain valid UTF-8",
        },
        Case {
            name: "links_stdin",
            via: Via::Cli {
                args: &["links", "--text", "-"],
                stdin: NOT_UTF8,
            },
            ja: "標準入力の読み取りに失敗: stream did not contain valid UTF-8",
        },
        cli(
            "edit_move",
            &["edit", "move", "diagonal", "--pane", "1"],
            "移動の単位は left / right / up / down / word-left / word-right / line-start / \
             smart-home / line-end / doc-start / doc-end / page-up / page-down のどれか\
             （\"diagonal\" は無い）",
        ),
        cli(
            "edit_delete",
            &["edit", "delete", "sideways", "--pane", "1"],
            "削除の単位は char-backward / char-forward / word-backward / word-forward / \
             to-line-start / to-line-end のどれか（\"sideways\" は無い）",
        ),
        cli(
            "edit_line",
            &["edit", "line", "shuffle", "--pane", "1"],
            "行の操作は duplicate-up / duplicate-down / move-up / move-down / indent / outdent / \
             comment / uncomment / toggle-comment のどれか（\"shuffle\" は無い）",
        ),
        cli(
            "edit_position_line",
            &["edit", "replace-range", "x:0", "1:0", "t", "--pane", "1"],
            "位置は 行:桁 で指定する（行が数値でない: x:0）",
        ),
        cli(
            "edit_position_column",
            &["edit", "replace-range", "1:y", "1:0", "t", "--pane", "1"],
            "位置は 行:桁 で指定する（桁が数値でない: 1:y）",
        ),
        cli(
            "lsp_format_range",
            &["lsp", "format", "--range", "1:0", "--pane", "1"],
            "範囲は 行:桁-行:桁 で指定する（`-` が無い: 1:0）",
        ),
        cli(
            "mod_dry_run",
            &["mod", "status", "--dry-run"],
            "--dry-run は install / uninstall にだけ付ける（tako mod install --dry-run）",
        ),
        cli(
            "mod_band",
            &["mod", "band"],
            "band には on か off を付ける（tako mod band off で帯を隠す）",
        ),
        cli(
            "mod_on_off",
            &["mod", "status", "on"],
            "on / off は band の後ろにだけ付ける（帯の切替は tako mod band on|off。\
             画面の UI 設定は tako mod ui）",
        ),
        Case {
            name: "mod_report_json",
            via: Via::Cli {
                args: &["mod", "report"],
                stdin: b"not json",
            },
            ja: "報告の JSON を読めない: expected ident at line 1 column 2",
        },
        Case {
            name: "mod_report_stdin",
            via: Via::Cli {
                args: &["mod", "report"],
                stdin: NOT_UTF8,
            },
            ja: "報告を stdin から読めない: stream did not contain valid UTF-8",
        },
        cli(
            "persist_reattach",
            &["persist", "reattach"],
            "reattach には --pane N（再接続するペインの ID）が要る",
        ),
        cli(
            "foreground",
            &["foreground"],
            "ペイン ID か --tab のどちらかを指定する（tako backgrounded で確認）",
        ),
        // `file move|copy` の引数不足（Reason::FileMoveArgs / FileCopyArgs）は clap が先に断る
        // （2 つ以上を必須にしている）ので、前段の文は二重の守り。文は番犬の Reason の表で見る
        cli(
            "run_config_env",
            &["run-config", "a.rs", "--env", "NOEQ"],
            "--env は K=V の形で指定する（指定: \"NOEQ\"）",
        ),
        // ---- MCP の前段（mcp/request.rs）----
        mcp(
            "click_shape",
            "tako_send_input",
            r#"{"pane":1,"click":"x"}"#,
            "click は {\"row\": 行, \"col\": 桁}（0 始まりの表示セル）で指定する",
        ),
        mcp(
            "click_cell",
            "tako_send_input",
            r#"{"pane":1,"click":{"row":-1,"col":0}}"#,
            "click.row を 0 以上の整数で指定する",
        ),
        mcp(
            "focus",
            "tako_focus_pane",
            "{}",
            "pane か direction のどちらか一方を指定する",
        ),
        mcp(
            "axis",
            "tako_resize_pane",
            r#"{"pane":1,"axis":"z"}"#,
            "axis は \"x\" か \"y\" を指定する",
        ),
        mcp(
            "window_action",
            "tako_window",
            r#"{"action":"bogus"}"#,
            "action が不正: bogus（list | new | close | move-tab | focus | minimize | maximize | \
             restore | move | resize）",
        ),
        mcp(
            "menu_open",
            "tako_menu",
            r#"{"action":"open"}"#,
            "open には menu が必要",
        ),
        mcp(
            "menu_invoke",
            "tako_menu",
            r#"{"action":"invoke"}"#,
            "invoke には path が必要",
        ),
        mcp(
            "menu_action",
            "tako_menu",
            r#"{"action":"bogus"}"#,
            "action が不正: bogus（list | open | close | invoke）",
        ),
        mcp(
            "persist_reattach",
            "tako_persist",
            r#"{"reattach":1,"enabled":true}"#,
            "enabled と reattach は併用できない（切替か再 attach のどちらか）",
        ),
        mcp(
            "open_file_mode",
            "tako_open_file",
            r#"{"pane":1,"path":"/x","mode":"bogus"}"#,
            "mode が不正: bogus（code | markdown）",
        ),
        mcp(
            "preview_edit_text",
            "tako_preview_edit",
            r#"{"pane":1,"text":"x"}"#,
            "text は command=type と組で渡す",
        ),
        mcp(
            "preview_save_action",
            "tako_preview_save",
            r#"{"pane":1,"action":"bogus"}"#,
            "action は save / overwrite / reload / diff のどれか（\"bogus\" は無い）",
        ),
        mcp(
            "file_op",
            "tako_file_op",
            r#"{"op":"bogus","path":"/x"}"#,
            "op が不正: bogus",
        ),
        mcp(
            "file_op_path",
            "tako_file_op",
            r#"{"op":"trash"}"#,
            "path（または paths）を指定する",
        ),
        mcp(
            "panel_view",
            "tako_panel",
            r#"{"view":"bogus"}"#,
            "view が不正: bogus（fleet | orch | git | tasks | diagnostics | references | search | \
             usage。tmux は fleet の旧称）",
        ),
        mcp(
            "queue_limits",
            "tako_queue",
            r#"{"limits":"x"}"#,
            "limits を解釈できない: invalid type: string \"x\", expected struct LimitsPatch",
        ),
        mcp(
            "adopt_name",
            "tako_orchestrator_adopt",
            "{}",
            "name を指定する（プロファイル名 / プロジェクトキー）",
        ),
        mcp(
            "remote_devices",
            "tako_remote_devices",
            "{}",
            "action を指定する（list / revoke / role）",
        ),
        mcp(
            "lsp_server_set",
            "tako_lsp_server",
            r#"{"action":"options","set":1}"#,
            "set はオブジェクト（キー → 値）",
        ),
        mcp(
            "lsp_rename",
            "tako_lsp",
            r#"{"action":"rename","pane":1,"line":1,"column":0}"#,
            "rename は new_name を指定する",
        ),
        mcp(
            "lsp_action",
            "tako_lsp",
            r#"{"action":"bogus"}"#,
            "action が不正: bogus（diagnostics / definition / declaration / type-definition / \
             implementation / format / format-on-save / completion / menu / hover / references / \
             incoming-calls / outgoing-calls / supertypes / subtypes / results / expand / collapse / \
             open-result / clear-results / rename）",
        ),
        mcp(
            "lsp_format_range",
            "tako_lsp",
            r#"{"action":"format","pane":1,"line":1}"#,
            "範囲の整形は line / column / end_line / end_column の 4 つをそろえて指定する\
             （省略で文書全体）",
        ),
        mcp(
            "remote_setup",
            "tako_remote_setup",
            "{}",
            "action を指定する（check / run）",
        ),
        mcp(
            "mod_action",
            "tako_mod",
            r#"{"action":"report"}"#,
            "action は status / on / off / band-on / band-off / ui / install / uninstall のどれか\
             （report は MCP に無い）",
        ),
        mcp(
            "mod_ui_args",
            "tako_mod",
            r#"{"action":"ui","op":5}"#,
            "action=ui の引数が読めない: invalid type: integer `5`, expected a string",
        ),
        mcp(
            "settings_rule",
            "tako_settings",
            r#"{"action":"paste_guard","rules":{"high_entropy":"x"}}"#,
            "rules.high_entropy は true / false で指定する",
        ),
        mcp(
            "settings_rules",
            "tako_settings",
            r#"{"action":"paste_guard","rules":1}"#,
            "rules は {\"規則名\": true / false} で指定する",
        ),
        mcp(
            "settings_paste_guard_only",
            "tako_settings",
            r#"{"action":"open","enabled":true}"#,
            "enabled は action=paste_guard のときだけ使える",
        ),
        mcp(
            "open_remote_target",
            "tako_open_remote",
            r#"{"host":"h","target":"bogus"}"#,
            "target は split | tab | pane のいずれか",
        ),
        // ---- MCP の JSON-RPC の口 ----
        Case {
            name: "rpc_method",
            via: Via::Rpc(r#"{"jsonrpc":"2.0","id":2,"method":"resources/list"}"#),
            ja: "メソッド resources/list は未対応",
        },
        Case {
            name: "rpc_not_json",
            via: Via::Rpc("not json"),
            ja: "JSON として解釈できない: expected ident at line 1 column 2",
        },
    ]
}

/// 1 ケースを走らせて断りの文を返す（断られなかったら `Err` に出力をそのまま）
fn refusal(scratch: &Scratch, via: &Via) -> Result<String, String> {
    match via {
        Via::Cli { args, stdin } => {
            let mut child = scratch.command(args).spawn().expect("tako を起動できる");
            {
                let mut input = child.stdin.take().expect("stdin");
                let _ = input.write_all(stdin);
            }
            let out = child.wait_with_output().expect("tako が終わる");
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            match stderr.trim_end().strip_prefix("error: ") {
                Some(message) if !out.status.success() => Ok(message.to_string()),
                _ => Err(format!(
                    "断られない（{}）stdout={} stderr={stderr}",
                    out.status,
                    String::from_utf8_lossy(&out.stdout)
                )),
            }
        }
        Via::Mcp { tool, args } => {
            let line = format!(
                r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"{tool}","arguments":{args}}}}}"#
            );
            rpc(scratch, &line)
        }
        Via::Rpc(line) => rpc(scratch, line),
    }
}

/// `tako mcp serve` へ initialize と 1 行を流し、その行への応答の断りを返す
fn rpc(scratch: &Scratch, line: &str) -> Result<String, String> {
    let mut child = scratch
        .command(&["mcp", "serve"])
        .env("TAKO_SOCKET", scratch.socket())
        .env("TAKO_TOKEN", "t2365")
        .env("TAKO_PANE_ID", "7")
        .spawn()
        .expect("ブリッジを起動できる");
    {
        let mut input = child.stdin.take().expect("stdin");
        writeln!(
            input,
            r#"{{"jsonrpc":"2.0","id":0,"method":"initialize","params":{{"protocolVersion":"2025-06-18","capabilities":{{}},"clientInfo":{{"name":"t2365","version":"0"}}}}}}"#
        )
        .expect("stdin へ書ける");
        writeln!(input, "{line}").expect("stdin へ書ける");
    }
    let out = child.wait_with_output().expect("ブリッジが終わる");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    for reply in stdout.lines() {
        let Ok(reply) = serde_json::from_str::<Value>(reply) else {
            continue;
        };
        if reply["id"] == Value::from(0) {
            continue;
        }
        if let Some(message) = reply["error"]["message"].as_str() {
            return Ok(message.to_string());
        }
        if reply["result"]["isError"] == Value::Bool(true) {
            let text = reply["result"]["content"][0]["text"].as_str().unwrap_or("");
            return Ok(text.to_string());
        }
        return Err(format!("断られない: {reply}"));
    }
    Err(format!("応答が無い: {stdout}"))
}

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

#[test]
fn 前段の断りは英語表示に日本語を含まず日本語表示は2365より前とバイト一致() {
    let all = cases();
    let en = Scratch::new("en");
    let ja = Scratch::new("ja");
    let mut failures = Vec::new();
    let mut en_japanese = 0;
    let mut ja_mismatch = 0;
    for case in &all {
        match refusal(&en, &case.via) {
            Ok(text) => {
                if let Some(c) = has_japanese(&text) {
                    en_japanese += 1;
                    failures.push(format!(
                        "[{}] 英語表示の断りに日本語（{c}）: {text}",
                        case.name
                    ));
                } else if text == case.ja {
                    failures.push(format!("[{}] 英語表示が日本語と同じ文: {text}", case.name));
                }
            }
            Err(e) => failures.push(format!("[{}] 英語表示: {e}", case.name)),
        }
        match refusal(&ja, &case.via) {
            Ok(text) if text == case.ja => {}
            Ok(text) => {
                ja_mismatch += 1;
                failures.push(format!(
                    "[{}] 日本語表示が #2365 より前と違う\n  want: {}\n  got:  {text}",
                    case.name, case.ja
                ));
            }
            Err(e) => failures.push(format!("[{}] 日本語表示: {e}", case.name)),
        }
    }
    eprintln!(
        "{}: {} ケース・英語表示で日本語入り {en_japanese} 件・日本語表示の不一致 {ja_mismatch} 件",
        tako().display(),
        all.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// 境界: 表示言語の設定が無い（system）ときは `TAKO_LANG` に従う（前段も設定と同じ解決）
#[test]
fn 表示言語の設定が無いときは_tako_lang_に従う() {
    let scratch = Scratch::new("en");
    std::fs::write(scratch.0.join("data/settings.json"), "{}").expect("settings.json を書ける");
    for (lang, want) in [
        ("en", "pass --env as K=V (got \"NOEQ\")"),
        ("ja", "--env は K=V の形で指定する（指定: \"NOEQ\"）"),
    ] {
        let out = scratch
            .command(&["run-config", "a.rs", "--env", "NOEQ"])
            .env("TAKO_LANG", lang)
            .stdin(Stdio::null())
            .output()
            .expect("tako が終わる");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            stderr.trim_end(),
            format!("error: {want}"),
            "TAKO_LANG={lang}"
        );
    }
}

/// 境界: 利用者が打った値は断りへそのまま埋める（言語で変えない）。日本語の値は英語表示でも日本語のまま
#[test]
fn 打たれた値は英語表示でもそのまま埋める() {
    let scratch = Scratch::new("en");
    let text = refusal(
        &scratch,
        &Via::Mcp {
            tool: "tako_window",
            args: r#"{"action":"閉じる"}"#,
        },
    )
    .expect("断られる");
    assert!(text.starts_with("invalid action: 閉じる ("), "{text}");
    assert!(has_japanese(text.trim_start_matches("invalid action: 閉じる")).is_none());
    let out = scratch
        .command(&["edit", "move", "\"斜め\"", "--pane", "1"])
        .stdin(Stdio::null())
        .output()
        .expect("tako が終わる");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(r#"(got "\"斜め\"")"#),
        "引用符つきの値はエスケープして埋める: {stderr}"
    );
}

/// 番犬自身の検査: 叩くバイナリが実在する（A/B の指定の打ち間違いで全ケースが空振りしない）
#[test]
fn 叩くバイナリが実在する() {
    assert!(Path::new(&tako()).is_file(), "{}", tako().display());
}
