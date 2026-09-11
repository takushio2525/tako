//! 承認カードの `command` が「承認を求めている操作」から始まることの番犬（Issue #633）
//!
//! `permission_dialog.command`（= `ChoiceDialog.title`）は、リモートのペイン選択画面
//! （#621）とチャットの承認カードが**1 行に詰めて**出す。ここに直前のアシスタント発話が
//! 混ざると、本題（実行されるコマンド）へ到達する前に省略される。
//!
//! # 実測で確定した真因（2026-09-11）
//!
//! `dialog::header_block` の境界は「罫線 / 0 桁の非空行（#1293）/ 選択肢に採らなかった
//! 番号つき行」の 3 つで、**どれも「当たったら捨てる」形**しかない。そのため
//! **罫線を引かず、箱ごと 0 桁で描く**許可ダイアログでは捨てる材料が 1 つも無く、
//! 上に在る会話はすべて本文へ入る。実採取 3 形での実測（修正前）:
//!
//! ```text
//! agy        : "⏺ ビルド成果物を消してから再ビルドします。 少し時間がかかります。 Requesting permission for: …"
//! claude(箱なし): "⏺ ビルド成果物を消してから再ビルドします。 少し時間がかかります。 Claude wants to run: …"
//! claude(箱つき): "Bash command Tip: … perl -e \"print 42\" … Do you want to proceed?"  ← 汚れない
//! ```
//!
//! Issue 本文の見当（「空行でリセットしないので巻き込まれる」）は**半分**当たっている。
//! 0 桁の `⏺` 行そのものは #1293 が既に切っているので、**いま残っているのは
//! 「境界になる行が 1 本も無い画面」**のほう。#633 の実測値が `Bash command` 見出しを
//! 含みながら `⏺` 行から始まっていたのも、その画面に罫線が無かったことを示す。
//!
//! 直し方は Issue の対応案どおり「本体の開始マーカーを**起点**にし、無ければ従来の
//! ブロック抽出へ落ちる」（`dialog::BODY_START_MARKERS` / `body_start_row`）。
//! 起点を下げるだけなので**結果は必ず従来の本文の接尾辞**で、拾えていた本文は減らない。
//!
//! A/B は `TAKO_633_LEGACY=1`（同一バイナリで修正前の「常に画面の上端から」へ戻す）。

use serde_json::Value;
use tako_control::claude_tui::detect_permission_dialog;
use tako_control::orchestrator::wait::{choice_dialog_json, permission_dialog_json};

fn screen(text: &str) -> Vec<String> {
    text.lines().map(str::to_string).collect()
}

fn command_of(text: &str) -> String {
    detect_permission_dialog(&screen(text))
        .unwrap_or_else(|| panic!("permission ダイアログとして検知されない:\n{text}"))
        .command
}

/// 会話ログの断片（この文字列が `command` に出たら汚染）。
/// 直前の発話 1 行 + **字下げされた継続行**（#1293 の 0 桁の規則では切れない側）
const TALK: &str = "⏺ ビルド成果物を消してから再ビルドします。\n  少し時間がかかります。\n\n";

/// 空行を挟まず密着した形（発話とダイアログのあいだに何も無い）
const TALK_TIGHT: &str = "⏺ ビルド成果物を消してから再ビルドします。\n  少し時間がかかります。\n";

/// 会話ログ由来の語（1 つでも `command` に出たら失敗）
const TALK_WORDS: &[&str] = &["ビルド成果物", "少し時間がかかります"];

// --- 実採取のダイアログ 3 形 ---

/// agy 1.x の許可ダイアログ（`claude_tui::tests::AGY_PERMISSION_DIALOG` と同じ実採取）。
/// **罫線なし・箱ごと 0 桁**なので #1293 の規則が自動的に無効になる形
const AGY: &str = r#"Requesting permission for:
   sleep 8
Full command:
   sleep 8 && echo AGY_DONE
Do you want to proceed?
> 1. Yes
  2. Yes, and always allow in this conversation for commands that start with 'sleep'
  3. Yes, and always allow for commands that start with 'sleep' (Persist to settings.json)
  4. No
  ↑/↓ Navigate · tab Amend · ctrl+g edit/expand command
esc to cancel                                       Claude Opus 4.6 (Thinking)"#;

/// claude の Bash 承認（箱なし。`claude_tui::tests::CLAUDE_BASH_PERMISSION` と同じ実採取）。
/// 本体が**空行を挟む**ので「空行を境界にする」では直せないことの証拠でもある
const CLAUDE_UNBOXED: &str = r#"  Claude wants to run:

  TAKO_ISOLATED=1 cargo run -p tako-app

  Allow this command?

❯ 1. Allow once
  2. Always allow for this session
  3. Deny

  Press enter to confirm · Esc to cancel"#;

/// claude のファイル承認（箱なし。`claude_tui::tests::CLAUDE_FILE_PERMISSION` と同じ実採取）
const CLAUDE_FILE: &str = r#"? Claude requested permissions to write to .../main.aux
  (suspicious Windows path pattern)
❯ 1. Allow once
  2. Always allow
  3. Deny

  Press enter to confirm"#;

/// claude v2.1.258 の Bash 承認（**罫線ボックス**）。2026-09-11 に隔離 tmux
/// （専用ソケット・一時ディレクトリ）で採取した実画面で、パスだけプレースホルダへ
/// 置き換えてある。ダイアログ本体が**空行を 3 つ挟む**形で、直前には
/// アシスタントの折り返し発話 4 行とツール結果行（`⎿`）が乗っている
const CLAUDE_BOXED: &str = r#" ▐▛███▛█   Claude Code v2.1.258
▝▜██████▀  Opus 5 (1M context) with xhigh effort · Claude Max
  ▝▝ ▝▝    /…/tmp/testuser-work

❯ この作業の狙いと手順を3文くらいで丁寧に説明してから、Bashツールで perl -e "print 42"
  をそのまま実行して。他のツールは使わないで

⏺ この作業は、Bash ツールが実際に動くかを最小コストで確かめる疎通確認です。perl -e "print 42" は
  Perl のワンライナー実行で、ファイルもディレクトリも一切触らず、標準出力に 42
  を出すだけなので副作用がありません。手順は単純で、指示どおりこのコマンドをそのまま Bash
  で1回実行し、出力が 42 になるかを確認するだけです。

⏺ Perl ワンライナーで 42 を出力
  ⎿  $ perl -e "print 42"

────────────────────────────────────────────────────────────────────────────────────────────────────
 Bash command
 Tip: auto mode handles these prompts for you — choose "switch to auto mode" below

   perl -e "print 42"
   Perl ワンライナーで 42 を出力

 This command requires approval

 Do you want to proceed?
 ❯ 1. Yes
   2. Yes, and don’t ask again for: perl *
   3. Yes, and switch to auto mode · auto mode handles these prompts for you
   4. No

 Esc to cancel · Tab to amend"#;

#[test]
fn issue633_箱なしの許可ダイアログに直前の発話が混ざらない() {
    for (name, dialog) in [
        ("agy", AGY),
        ("claude/Bash（箱なし）", CLAUDE_UNBOXED),
        ("claude/ファイル", CLAUDE_FILE),
    ] {
        for (shape, talk) in [("空行あり", TALK), ("密着", TALK_TIGHT)] {
            let command = command_of(&format!("{talk}{dialog}"));
            for word in TALK_WORDS {
                assert!(
                    !command.contains(word),
                    "{name}/{shape}: 会話本文が command に混ざっている（{word}）: {command:?}"
                );
            }
            // 直前の発話が無い素の画面と**同じ**値になる = 起点が本体の先頭に乗っている
            assert_eq!(
                command,
                command_of(dialog),
                "{name}/{shape}: 発話の有無で command が変わる"
            );
        }
    }
}

#[test]
fn issue633_箱なしの許可ダイアログは本体を全部残す() {
    // 起点を下げても「本文が減る」ことは無い（接尾辞であって切り詰めではない）
    for (name, dialog, must) in [
        (
            "agy",
            AGY,
            &[
                "Requesting permission for:",
                "sleep 8 && echo AGY_DONE",
                "Do you want to proceed?",
            ][..],
        ),
        (
            "claude/Bash（箱なし）",
            CLAUDE_UNBOXED,
            &[
                "Claude wants to run:",
                "TAKO_ISOLATED=1 cargo run -p tako-app",
                "Allow this command?",
            ][..],
        ),
        (
            "claude/ファイル",
            CLAUDE_FILE,
            &[
                "Claude requested permissions to write to",
                "suspicious Windows path pattern",
            ][..],
        ),
    ] {
        let command = command_of(&format!("{TALK}{dialog}"));
        for needle in must {
            assert!(
                command.contains(needle),
                "{name}: 本体の行が落ちている（{needle}）: {command:?}"
            );
        }
    }
}

#[test]
fn issue633_実採取の罫線ボックスは従来どおり見出しから始まる() {
    // 罫線が在る形は #1293 の時点で既に汚れない = ここは**回帰の番犬**
    let command = command_of(CLAUDE_BOXED);
    assert!(
        command.starts_with("Bash command"),
        "見出しから始まっていない: {command:?}"
    );
    // `Perl ワンライナーで 42 を出力` は**本体側にも在る**説明行なので禁止語に採らない
    for word in ["この作業は", "疎通確認", "をそのまま実行して", "⎿"] {
        assert!(
            !command.contains(word),
            "会話本文が command に混ざっている（{word}）: {command:?}"
        );
    }
    // 本体の空行を跨いで最後まで拾えている（空行を境界にしていない）
    assert!(command.contains(r#"perl -e "print 42""#), "{command:?}");
    assert!(
        command.contains("This command requires approval"),
        "{command:?}"
    );
    assert!(command.ends_with("Do you want to proceed?"), "{command:?}");
}

#[test]
fn issue633_罫線が画面外でも見出しが起点になる() {
    // #633 の実測値（`⏺ … Bash command rm -rf build/ Do you want to proceed?`）は
    // 罫線が**その画面に無かった**ことを示す。実採取から罫線 1 行だけを抜いて再現する
    let rule = CLAUDE_BOXED
        .lines()
        .find(|l| tako_core::dialog::is_rule_line(l))
        .expect("実採取に罫線が在る");
    let no_rule = CLAUDE_BOXED.replace(
        &format!(
            "{rule}
"
        ),
        "",
    );
    assert!(
        !no_rule.lines().any(tako_core::dialog::is_rule_line),
        "罫線が残っている = この検査の前提が崩れている"
    );
    // 修正前（`TAKO_633_LEGACY=1`）はここで会話ログ（ツール結果の `⎿` 行）が先頭に残る。
    // 0 桁の `⏺` 行は #1293 が切るので、残るのは**字下げされた**残骸のほう
    let command = command_of(&no_rule);
    assert!(
        command.starts_with("Bash command"),
        "見出しから始まっていない: {command:?}"
    );
    assert!(command.ends_with("Do you want to proceed?"), "{command:?}");
}

#[test]
fn issue633_見出しが画面の最上段でも拾える() {
    // 短いペインで罫線ごと流れ切り、見出しが 1 行目に来た形
    let from_heading: String = CLAUDE_BOXED
        .lines()
        .skip_while(|l| !l.trim_start().starts_with("Bash command"))
        .collect::<Vec<_>>()
        .join(
            "
",
        );
    let command = command_of(&from_heading);
    assert!(command.starts_with("Bash command"), "{command:?}");
    assert!(command.ends_with("Do you want to proceed?"), "{command:?}");
}

#[test]
fn issue633_選択肢が三個以上でも同じ値になる() {
    // 実採取 3 形の選択肢は 3 / 3 / 4 個。どれも `command` は本体だけ
    for (name, dialog, n) in [
        ("agy", AGY, 4),
        ("claude/Bash（箱なし）", CLAUDE_UNBOXED, 3),
        ("claude/罫線ボックス", CLAUDE_BOXED, 4),
    ] {
        let d = detect_permission_dialog(&screen(dialog)).expect(name);
        assert_eq!(d.options.len(), n, "{name}: 選択肢の数");
        assert_eq!(d.highlighted, Some(0), "{name}: 既定は先頭");
    }
}

#[test]
fn issue633_worker_statusとwatchとカードが同じcommandを返す() {
    // MCP `tako_orchestrator_worker_status`（dispatch）と CLI `tako orchestrator watch`
    // は `wait::permission_dialog_json` の 1 実装を、リモートのカード（remote.rs）と
    // チャット（tako-app）は `detect_permission_dialog` を通る。#748 の `choice_dialog`
    // も同じ本文を `title` に載せるので、4 つが 1 つの値からできていることを固定する
    for (name, dialog) in [
        ("agy", AGY),
        ("claude/Bash（箱なし）", CLAUDE_UNBOXED),
        ("claude/ファイル", CLAUDE_FILE),
        ("claude/罫線ボックス", CLAUDE_BOXED),
    ] {
        let text = format!("{TALK}{dialog}");
        let direct = command_of(&text);
        let via_wait: Value = permission_dialog_json(&text).expect(name);
        assert_eq!(via_wait["command"], Value::String(direct.clone()), "{name}");
        let choice: Value = choice_dialog_json(&text).expect(name);
        assert_eq!(choice["kind"], Value::String("permission".into()), "{name}");
        assert_eq!(choice["title"], Value::String(direct), "{name}");
    }
}

#[test]
fn issue633_起点マーカーは実採取に実在する() {
    // 文言のタイプミスで「死んだマーカー」にならないことを機械で保証する。
    // マーカーを増やすときはここへ実採取の画面を足すこと
    let captures = [AGY, CLAUDE_UNBOXED, CLAUDE_FILE, CLAUDE_BOXED];
    for marker in tako_core::dialog::BODY_START_MARKERS {
        assert!(
            captures.iter().any(|c| c.contains(marker)),
            "実採取に無いマーカーが載っている（当て推量で足さない）: {marker:?}"
        );
    }
}
