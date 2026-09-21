//! リモート daemon の「起動していた」が消えず、GUI 起動で立て直されることの番犬（#1485）
//!
//! # なぜ要るか
//!
//! remote daemon は GUI とは別プロセス（`tako remote serve`）で、Mac を再起動すると
//! 消える。実測（2026-09-21）: 再起動後の GUI は「復元成功: 11 タブ / 23 ペイン」まで
//! 進むのに `tako remote status` は `running: false` のままで、**スマホからは「PWA が
//! 繋がらない」としか見えず PC 側にも何も出ない**。手で `tako remote start` を叩いたら
//! self_check 200 で即復旧した = 環境は健全で、単に誰も立て直していなかった。
//!
//! 真因は 2 つ: ①「ユーザーが起動していた」をどこにも永続していない
//! ②GUI の起動経路に立て直す処理が無い。
//!
//! # 何を縛るか
//!
//! 1. 意図を書く口が 1 実装（`remote::set_desired`）で、**起動が成った後だけ**通る
//! 2. 意図を消す口が 1 実装（`remote::clear_desired`）で、判定は**現況**（`daemon_status`）
//! 3. daemon の死で意図が消えない（`cleanup_state_files` は意図を消さない）
//! 4. `daemon_status` は**どの分岐でも**意図と直近の結果を載せる（止まっていても黙らない）
//! 5. GUI の起動が立て直しを呼ぶ（`spawn_remote_autostart`）
//! 6. 立て直しは**試行のたびに判断を引き直す**（待っている間に `stop` されたら降りる）
//! 7. 諦めたら無言にしない（`notify_ui_failure` の 1 実装を通る）
//! 8. 判断は純関数 1 本（`autostart_decision`）で、読む側（CLI）は**再計算しない**
//! 9. OS の可否は `platform::support` の 1 表（`cfg!(windows)` で分岐しない）
//! 10. A/B は `TAKO_1485_LEGACY` の 1 箇所に閉じる
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば 1〜10 はすべて無意味に緑になるので、[`走査が空振りしていない`]
//! で窓が採れていることを固定し、[`逆戻りを名指しできる`] で**修正前を再現した
//! 10 通りの注入**が file:line で名指しされることを確かめる。範囲取りは #1420 の
//! 1 実装を通す。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const REMOTE: &str = "crates/tako-control/src/remote.rs";
const AUTOSTART: &str = "crates/tako-control/src/remote_autostart.rs";
const PANEL: &str = "crates/tako-app/src/remote_panel.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(root: &Path, rel: &'static str) -> String {
    let raw =
        std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"));
    production_range::production(&raw, rel)
}

/// 1 つの違反（`ファイル:行 — 理由`）
#[derive(Debug)]
struct Offender {
    file: &'static str,
    line: usize,
    why: String,
}

impl Offender {
    fn report(&self) -> String {
        format!("{}:{} — {}", self.file, self.line, self.why)
    }
}

/// 関数の窓（宣言行の 1-based 行番号と本文）。
/// 終わりは**宣言行と同じ字下げの `}`**（「宣言行から N 行」では隣の関数を巻き込む）
fn fn_window(src: &str, needle: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)
        .unwrap_or(lines.len() - 1);
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 注釈（`//` で始まる行）を落とした窓。検査は**コードだけ**を見る
/// （アンチパターンを説明した注釈で落ちると、理由を書くほど落ちることになる）
fn code_only(window: &str) -> String {
    window
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 窓の中で `needle` を含む最初の**コードの**行（0-based の相対位置）
fn code_line_with(window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| !l.trim_start().starts_with("//") && l.contains(needle))
}

struct Sources {
    remote: String,
    autostart: String,
    panel: String,
    main: String,
    cli: String,
}

fn sources(root: &Path) -> Sources {
    Sources {
        remote: read(root, REMOTE),
        autostart: read(root, AUTOSTART),
        panel: read(root, PANEL),
        main: read(root, MAIN),
        cli: read(root, CLI),
    }
}

/// 意図の永続（`remote.rs`）の検査
fn scan_remote(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: REMOTE,
            line,
            why,
        })
    };

    // 1. 意図を書く口は 1 実装で、`spawn_daemon` が成功した後だけ通る
    match fn_window(src, "pub fn set_desired()") {
        None => push(
            0,
            "`set_desired` が無い（「起動していた」を残す 1 実装が消えている。#1485）".into(),
        ),
        Some((at, _)) => {
            let spawn = fn_window(src, "pub fn spawn_daemon()").map(|(_, w)| code_only(&w));
            match spawn {
                None => push(at, "`spawn_daemon` が見つからない（走査の空振り）".into()),
                Some(code) => {
                    if !code.contains("set_desired()") {
                        push(
                            at,
                            "`spawn_daemon` が意図を残していない。\
                             残さないと Mac の再起動後に立て直す手がかりが無い（#1485 の真因）"
                                .into(),
                        );
                    }
                }
            }
        }
    }

    // 2. 意図を消すのは停止の成功ではなく「止まっていること」
    match fn_window(src, "fn forget_if_stopped(") {
        None => push(
            0,
            "`forget_if_stopped` が無い（`tako remote stop` で意図が消えない = \
             止めたのに次回戻ってくる。#1485）"
                .into(),
        ),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("clear_desired()") {
                push(at, "停止経路が `clear_desired` を通っていない".into());
            }
            if !code.contains("daemon_status()") {
                push(
                    at,
                    "消す条件を現況（`daemon_status`）で見ていない。\
                     `Ok` だけを見ると「もともと止まっていた」環境で `stop` した人の\
                     意図を取りこぼし、次の GUI 起動で勝手に戻る（#1485）"
                        .into(),
                );
            }
        }
    }
    for (needle, why) in [
        (
            "pub fn daemon_stop()",
            "`daemon_stop` が `forget_if_stopped` を通っていない",
        ),
        (
            "pub fn daemon_force_stop()",
            "`daemon_force_stop` が `forget_if_stopped` を通っていない",
        ),
    ] {
        match fn_window(src, needle) {
            None => push(
                0,
                format!("{needle} が無い（#1485 の停止経路が欠けている）"),
            ),
            Some((at, window)) => {
                if !code_only(&window).contains("forget_if_stopped(") {
                    push(at, why.into());
                }
            }
        }
    }

    // 3. daemon の死で意図が消えない
    match fn_window(src, "fn cleanup_state_files()") {
        None => push(0, "`cleanup_state_files` が無い（走査の空振り）".into()),
        Some((at, window)) => {
            for needle in ["desired_path()", "autostart_path()"] {
                if let Some(rel) = code_line_with(&window, needle) {
                    push(
                        at + rel,
                        format!(
                            "`cleanup_state_files` が {needle} を消している。\
                             daemon の死（Mac の再起動）で**ユーザーの意図まで**消えるので、\
                             立て直す手がかりが無くなる（#1485）"
                        ),
                    );
                }
            }
        }
    }

    // 4. status はどの分岐でも意図を載せる（止まっていても黙らない）
    match fn_window(src, "pub fn daemon_status()") {
        None => push(0, "`daemon_status` が無い（走査の空振り）".into()),
        Some((at, window)) => {
            let code = code_only(&window);
            let returns = code.matches("running\": false").count();
            let wrapped = code.matches("with_autostart_fields(").count();
            if wrapped < returns + 1 {
                push(
                    at,
                    format!(
                        "`daemon_status` の {returns} 本の「止まっている」分岐 + 稼働中の応答が\
                         `with_autostart_fields` を通っていない（通っているのは {wrapped} 本）。\
                         止まっているときこそ「戻すつもりだったのか」が要る情報（#1485）"
                    ),
                );
            }
        }
    }
    match fn_window(src, "fn with_autostart_fields(") {
        None => push(
            0,
            "`with_autostart_fields` が無い（status へ載せる 1 実装が消えている。#1485）".into(),
        ),
        Some((at, window)) => {
            if !code_only(&window).contains("remote_autostart::status_fields(") {
                push(
                    at,
                    "載せる形を `remote_autostart::status_fields` の 1 実装から取っていない\
                     （CLI / MCP / GUI で名前と中身がずれる）"
                        .into(),
                );
            }
        }
    }
    out
}

/// 判断と記録（`remote_autostart.rs`）の検査
fn scan_autostart(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: AUTOSTART,
            line,
            why,
        })
    };

    // 8. 判断は理由を返す純関数 1 本
    match fn_window(src, "pub fn autostart_decision(") {
        None => push(
            0,
            "`autostart_decision` が無い（判断の 1 実装が消えている。#1485）".into(),
        ),
        Some((at, window)) => {
            let code = code_only(&window);
            for reason in [
                "AutostartSkip::Disabled",
                "AutostartSkip::Unsupported",
                "AutostartSkip::NotDesired",
                "AutostartSkip::AlreadyRunning",
            ] {
                if !code.contains(reason) {
                    push(
                        at,
                        format!("{reason} を返す枝が無い（理由が丸まる。#1485）"),
                    );
                }
            }
            if !window.contains("-> Result<(), AutostartSkip>") {
                push(
                    at,
                    "真偽値を返している。**理由**を返さないと、画面・CLI・診断が\
                     それぞれ理由を書き直すことになり片方だけ嘘になる（#372 / #1473）"
                        .into(),
                );
            }
        }
    }

    // 9. OS の可否はマトリクスの 1 表から取る
    match fn_window(src, "pub fn platform_supported()") {
        None => push(
            0,
            "`platform_supported` が無い（OS ゲートが散る。#1485 / #971）".into(),
        ),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("support::support_for(") {
                push(
                    at,
                    "OS の可否を `platform::support` の 1 表から取っていない。\
                     直書きすると Windows の serve が動いた日に片方だけ残る（#971）"
                        .into(),
                );
            }
            if code.contains("cfg!(") || code.contains("target_os") {
                push(
                    at,
                    "`cfg!` / `target_os` で分岐している（宣言とコードが二重化する。#515）".into(),
                );
            }
        }
    }

    // 材料集めは毎回取り直せる形（`probe`）で、判断そのものは純関数を通る
    match fn_window(src, "pub fn probe()") {
        None => push(0, "`probe` が無い（#1485 の材料集めが消えている）".into()),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("autostart_decision(") {
                push(at, "`probe` が純関数の判断を通っていない".into());
            }
            if !code.contains("read_desired()") || !code.contains("daemon_status()") {
                push(
                    at,
                    "`probe` が意図と現況を毎回読み直していない（待っている間の \
                     start / stop を見落とす）"
                        .into(),
                );
            }
        }
    }

    // 記録は「意図が在る環境だけ」＝ 関係ない人の data_dir を汚さない
    match fn_window(src, "pub fn record_outcome(") {
        None => push(
            0,
            "`record_outcome` が無い（記録の 1 実装が消えている）".into(),
        ),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("read_desired()") {
                push(
                    at,
                    "意図の有無を見ずに記録している（remote を使っていない人の \
                     data_dir に記録だけが生える）"
                        .into(),
                );
            }
            if !code.contains("write_last_autostart(") || !code.contains("persist_log(") {
                push(
                    at,
                    "記録がファイルと persist.log の両方へ行っていない\
                     （片方だけ残ると status とログで食い違う。#1485 ③）"
                        .into(),
                );
            }
        }
    }
    out
}

/// GUI の立て直し（`remote_panel.rs` / `main.rs`）の検査
fn scan_gui(panel: &str, main: &str) -> Vec<Offender> {
    let mut out = Vec::new();

    // 5. 起動経路が立て直しを呼ぶ
    if !code_only(main).contains("app.spawn_remote_autostart(cx)") {
        out.push(Offender {
            file: MAIN,
            line: 0,
            why: "GUI の起動が `spawn_remote_autostart` を呼んでいない。\
                  呼ばないと #1485 の症状（再起動後に running: false のまま）に戻る"
                .into(),
        });
    }

    match fn_window(panel, "pub(crate) fn spawn_remote_autostart(") {
        None => out.push(Offender {
            file: PANEL,
            line: 0,
            why: "`spawn_remote_autostart` が無い（立て直しの 1 実装が消えている。#1485）".into(),
        }),
        Some((at, window)) => {
            let code = code_only(&window);
            let mut push = |why: String| {
                out.push(Offender {
                    file: PANEL,
                    line: at,
                    why,
                })
            };
            // 6. 試行のたびに判断を引き直す（`probe` が 2 回以上 = 入口と待ちのあと）
            if code.matches("autostart::probe()").count() < 2 {
                push(
                    "判断を引き直していない。待っている間に `tako remote stop` された\
                     ときに止められず、止めたはずの daemon が戻る（#1485 ④）"
                        .into(),
                );
            }
            if !code.contains("autostart::backoff_secs(") {
                push(
                    "バックオフを持っていない。Mac の再起動直後は tailscaled がまだ\
                     上がっておらず 1 発目は落ちる（#1485 ②）"
                        .into(),
                );
            }
            if !code.contains("autostart::attempts_max()") {
                push("試行回数の上限が無い（諦めずに回り続ける）".into());
            }
            if !code.contains("remote::spawn_daemon()") {
                push(
                    "起動が `spawn_daemon`（起動ボタン / CLI / MCP と同一実体）を\
                     通っていない（経路が 2 本になる）"
                        .into(),
                );
            }
            // 4'. 権限を増やさない（戻すのは daemon だけ）
            for forbidden in ["devices_set_role", "devices_revoke", "DeviceRegistry"] {
                if code.contains(forbidden) {
                    push(format!(
                        "自動復帰が {forbidden} に触れている。戻すのは daemon だけで、\
                         端末のペアリングと role は触らない（#1485 ④）"
                    ));
                }
            }
            // 3'. 再試行の間を無音にしない（既定の合計は 77 秒。最後の結果だけ残すと
            //     その間ユーザーにも診断にも何も見えない = #1485 が消したい「黙る」）
            if !code.contains("autostart::log_attempt_failure(") {
                push(
                    "途中の試行の失敗を診断へ残していない（再試行の 1 分以上が無音になる。#1485 ③）"
                        .into(),
                );
            }
            // 7. 諦めたら無言にしない
            if !code.contains("notify_ui_failure(") {
                push("失敗を画面へ出していない（#1399 / #1446 ④ と同じ 1 実装を通す）".into());
            }
        }
    }

    match fn_window(panel, "async fn finish_remote_autostart_skip(") {
        None => out.push(Offender {
            file: PANEL,
            line: 0,
            why: "`finish_remote_autostart_skip` が無い（見送りの記録が消えている）".into(),
        }),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("is_noteworthy()") {
                out.push(Offender {
                    file: PANEL,
                    line: at,
                    why: "見送りの理由を選ばずに画面へ出している。`not-desired` は\
                          設定どおりの正常な動きなので、GUI 起動のたびにバナーが出る（#1485）"
                        .into(),
                });
            }
        }
    }
    out
}

/// 読む側（CLI）は状態を読むだけで再計算しない
fn scan_cli(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(src, "fn remote_status()") {
        None => out.push(Offender {
            file: CLI,
            line: 0,
            why: "`remote_status` が無い（走査の空振り）".into(),
        }),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("last_autostart") {
                out.push(Offender {
                    file: CLI,
                    line: at,
                    why: "`tako remote status` が自動復帰の結果を読んでいない\
                          （JSON に埋もれて「戻せなかった」に気づけない。#1485 ③）"
                        .into(),
                });
            }
            if let Some(rel) = code_line_with(&window, "autostart_decision(") {
                out.push(Offender {
                    file: CLI,
                    line: at + rel,
                    why: "CLI が判断を再計算している。読む側は**状態を読むだけ**にしないと、\
                          A/B や世代違いのバイナリで画面と CLI の答えが割れる（#372 / #1473）"
                        .into(),
                });
            }
        }
    }
    out
}

fn all(s: &Sources) -> Vec<String> {
    let mut found = Vec::new();
    found.extend(scan_remote(&s.remote));
    found.extend(scan_autostart(&s.autostart));
    found.extend(scan_gui(&s.panel, &s.main));
    found.extend(scan_cli(&s.cli));
    found.iter().map(Offender::report).collect()
}

#[test]
fn 起動していたという記憶が消えず立て直しが呼ばれる() {
    let root = workspace_root();
    let found = all(&sources(&root));
    assert!(
        found.is_empty(),
        "#1485 の不変条件が壊れている:\n  {}",
        found.join("\n  ")
    );
}

/// 走査が空振りしていない（窓が採れている）
#[test]
fn 走査が空振りしていない() {
    let root = workspace_root();
    let s = sources(&root);
    for (src, file, needle) in [
        (&s.remote, REMOTE, "pub fn spawn_daemon()"),
        (&s.remote, REMOTE, "pub fn set_desired()"),
        (&s.remote, REMOTE, "fn forget_if_stopped("),
        (&s.remote, REMOTE, "fn cleanup_state_files()"),
        (&s.remote, REMOTE, "pub fn daemon_status()"),
        (&s.remote, REMOTE, "fn with_autostart_fields("),
        (&s.autostart, AUTOSTART, "pub fn autostart_decision("),
        (&s.autostart, AUTOSTART, "pub fn platform_supported()"),
        (&s.autostart, AUTOSTART, "pub fn probe()"),
        (&s.autostart, AUTOSTART, "pub fn record_outcome("),
        (&s.panel, PANEL, "pub(crate) fn spawn_remote_autostart("),
        (&s.panel, PANEL, "async fn finish_remote_autostart_skip("),
        (&s.cli, CLI, "fn remote_status()"),
    ] {
        let (at, window) = fn_window(src, needle)
            .unwrap_or_else(|| panic!("{file} の {needle} の窓が採れない（走査が空振り）"));
        assert!(at > 0, "{file} の {needle} の行番号が採れていない");
        assert!(
            window.lines().count() > 2,
            "{file} の {needle} の窓が 2 行以下（字下げの規約が変わって窓が切れている）"
        );
    }
}

/// A/B の腕は 1 箇所に閉じ、判定と通知で同じものが効く
#[test]
fn abの腕は一つに閉じている() {
    let root = workspace_root();
    let s = sources(&root);
    let sidebar = read(root.as_path(), "crates/tako-app/src/sidebar.rs");
    // env の直読みは `remote_autostart::legacy_mode` の 1 箇所だけ
    // （注釈で名前を挙げるのは自由なので、数えるのは**コードの行だけ**）
    let reads: usize = [&s.remote, &s.autostart, &s.panel, &s.main, &s.cli, &sidebar]
        .iter()
        .map(|src| code_only(src).matches("TAKO_1485_LEGACY").count())
        .sum();
    assert_eq!(
        reads, 1,
        "`TAKO_1485_LEGACY` を読む場所が 1 箇所でない（腕が揃わなくなる）"
    );
    assert!(
        code_only(&sidebar).contains("tako_control::remote_autostart::legacy_mode()"),
        "通知の抑止が判定と同じ腕を引いていない（旧挙動では自動復帰しないので、\
         通知だけ残っても意味がない）"
    );
    let decision = fn_window(&s.autostart, "pub fn autostart_decision(")
        .expect("判断の窓")
        .1;
    assert!(
        code_only(&decision).contains("input.legacy"),
        "判断が A/B を見ていない（同一バイナリで旧挙動を再現できない）"
    );
}

/// 修正前を再現した注入を file:line で名指しできる
#[test]
fn 逆戻りを名指しできる() {
    let root = workspace_root();
    let base = sources(&root);

    // 注入 1: 意図を残さない（#1485 の真因そのもの）
    let mut s = sources(&root);
    s.remote = base.remote.replace("    set_desired();\n", "");
    assert!(s.remote != base.remote, "注入 1 の対象が見つからない");
    let found = all(&s);
    assert!(
        found.iter().any(|o| o.contains(REMOTE)),
        "意図を残さない形を名指しできていない: {found:?}"
    );

    // 注入 2: 停止で意図を消さない（止めたのに次回戻ってくる）
    let mut s = sources(&root);
    s.remote = base.remote.replace("        clear_desired();\n", "");
    assert!(s.remote != base.remote, "注入 2 の対象が見つからない");
    let found = all(&s);
    assert!(
        found.iter().any(|o| o.contains(REMOTE)),
        "意図が消えない形を名指しできていない: {found:?}"
    );

    // 注入 3: daemon の死で意図まで消す（Mac の再起動で手がかりが無くなる）
    let mut s = sources(&root);
    s.remote = base.remote.replace(
        "fn cleanup_state_files() {\n    let _ = std::fs::remove_file(pid_path());",
        "fn cleanup_state_files() {\n    let _ = std::fs::remove_file(desired_path());\n    let _ = std::fs::remove_file(pid_path());",
    );
    assert!(s.remote != base.remote, "注入 3 の対象が見つからない");
    // `clear_desired` も同じ 1 行を持つので、**`cleanup_state_files` より後**から探す
    // （先頭から探すと別の関数の行を期待値にしてしまう）
    let head = s
        .remote
        .lines()
        .position(|l| l.contains("fn cleanup_state_files()"))
        .expect("注入先の関数がある");
    let want = head
        + s.remote
            .lines()
            .skip(head)
            .position(|l| l.contains("remove_file(desired_path())"))
            .expect("注入した行がある");
    let found = all(&s);
    assert!(
        found
            .iter()
            .any(|o| o.contains(&format!("{REMOTE}:{}", want + 1))),
        "意図まで消す形を名指しできていない（期待 {REMOTE}:{}）: {found:?}",
        want + 1
    );

    // 注入 4: 止まっているときの応答から意図を落とす（黙る形へ戻る）
    let mut s = sources(&root);
    s.remote = base.remote.replace(
        "Err(_) => return with_autostart_fields(json!({ \"running\": false })),",
        "Err(_) => return json!({ \"running\": false }),",
    );
    assert!(s.remote != base.remote, "注入 4 の対象が見つからない");
    let found = all(&s);
    assert!(
        found.iter().any(|o| o.contains(REMOTE)),
        "止まっているときに黙る形を名指しできていない: {found:?}"
    );

    // 注入 5: GUI の起動から立て直しを落とす（症状そのもの）
    let mut s = sources(&root);
    s.main = base.main.replace("app.spawn_remote_autostart(cx);", "");
    assert!(s.main != base.main, "注入 5 の対象が見つからない");
    let found = all(&s);
    assert!(
        found.iter().any(|o| o.contains(MAIN)),
        "立て直しを呼ばない形を名指しできていない: {found:?}"
    );

    // 注入 6: 判断を 1 回しか引かない（待っている間の stop を見落とす）
    let mut s = sources(&root);
    s.panel = base.panel.replacen(
        ".spawn(async { autostart::probe() })",
        ".spawn(async { Ok(()) })",
        1,
    );
    assert!(s.panel != base.panel, "注入 6 の対象が見つからない");
    let found = all(&s);
    assert!(
        found.iter().any(|o| o.contains(PANEL)),
        "判断を引き直さない形を名指しできていない: {found:?}"
    );

    // 注入 6b: 再試行の間を無音へ戻す（最後の結果しか残らない形）
    let mut s = sources(&root);
    s.panel = base
        .panel
        .replace("autostart::log_attempt_failure(n, max, &detail);", "");
    assert!(s.panel != base.panel, "注入 6b の対象が見つからない");
    let found = all(&s);
    assert!(
        found.iter().any(|o| o.contains(PANEL)),
        "再試行の間を無音へ戻した形を名指しできていない: {found:?}"
    );

    // 注入 7: 諦めたときを無言へ戻す（#1485 の報告の見え方）
    let mut s = sources(&root);
    s.panel = base.panel.replace(
        "app.notify_ui_failure(\n                            crate::sidebar::NoticeArea::RemoteAutostart,",
        "#[allow(unused)]\n                        let _silent = (\n                            crate::sidebar::NoticeArea::RemoteAutostart,",
    );
    assert!(s.panel != base.panel, "注入 7 の対象が見つからない");
    let found = all(&s);
    assert!(
        found.iter().any(|o| o.contains(PANEL)),
        "無言へ戻した形を名指しできていない: {found:?}"
    );

    // 注入 8: OS ゲートを直書きへ戻す（宣言とコードが二重化する）
    let mut s = sources(&root);
    s.autostart = base.autostart.replace(
        "    support::support_for(support::Platform::current(), SUPPORT_KEY).is_some_and(|s| s.is_usable())",
        "    cfg!(target_os = \"macos\")",
    );
    assert!(s.autostart != base.autostart, "注入 8 の対象が見つからない");
    let found = all(&s);
    assert!(
        found.iter().any(|o| o.contains(AUTOSTART)),
        "OS ゲートの直書きを名指しできていない: {found:?}"
    );

    // 注入 9: CLI が判断を再計算する（画面と CLI の答えが割れる）
    let mut s = sources(&root);
    s.cli = base.cli.replace(
        "    if let Some(last) = status.get(\"last_autostart\") {",
        "    let _ = tako_control::remote_autostart::autostart_decision(&todo!());\n    if let Some(last) = status.get(\"last_autostart\") {",
    );
    assert!(s.cli != base.cli, "注入 9 の対象が見つからない");
    let want = s
        .cli
        .lines()
        .position(|l| l.contains("autostart_decision(&todo!())"))
        .expect("注入した行がある");
    let found = all(&s);
    assert!(
        found
            .iter()
            .any(|o| o.contains(&format!("{CLI}:{}", want + 1))),
        "CLI が再計算する形を名指しできていない（期待 {CLI}:{}）: {found:?}",
        want + 1
    );
}
