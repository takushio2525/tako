//! 狭いペインの背の高い AskUserQuestion の検知（Issue #1847 の 2 つ目の症状）
//!
//! # 症状（2026-09-29〜30 の実運用・claude 2.1.280・32 桁）
//!
//! 1 タブに worker を 8 体並べて各 32 桁まで縮んだ配置で、AskUserQuestion で止まった
//! worker の `status` は `waiting` なのに、`respond`（choice 省略の下見）が
//! 「ペイン画面に選択肢ダイアログが見つからない」で落ち、`read_pane` の
//! `choice_dialog` も `null` だった（tmux の send-keys で番号キーを直接送って回避）。
//!
//! # 真因（実物の claude で採った画面で名指しした）
//!
//! 32 桁では説明の折り返しで選択肢 1 つが 6〜10 行になり、4 択の箱が 45 行を超える。
//! ダイアログがペインより高いと claude は**選択カーソル `❯ 1.` を画面の最上段に**残して
//! 下を描く。判定はカーソルを下端から 40 行（`SCAN_ROWS`）しか探さず、カーソルが無いときの
//! 経路 3（#1143）は箱の上端の罫線か 2 列レイアウトを証拠に要るので、罫線が画面の外の
//! この形では両方とも外れて `None` だった。60 桁なら箱は 20 行台で窓に収まる。
//!
//! 修正は経路 5（`numbered_above_scan`）: 窓より上の選択カーソル行を起点に番号つきの並びを
//! 組み、並びから下端までがダイアログの一部だけ + 確定キーの案内があるときだけ採る。
//! A/B は `TAKO_1847_LEGACY=1`（`issue1847_legacy_ab` のテスト = 子プロセスで観る）。
//!
//! # fixture（すべて実採取・個人情報なし）
//!
//! claude 2.1.301 を一時の設定 dir + 偽の Messages API（AskUserQuestion の tool_use を返す）で
//! 隔離 tmux に立て、`capture-pane -p` で採った画面。ヘッダのパスだけ `/…/work` に置き換えた。
//! 当時の版 2.1.280 でも同じ手順で採り、32×46 は 1 バイトも違わず、32×70 はヘッダ以外同一。

use std::process::Command;

use tako_core::dialog::{detect_choice_list, is_choice_dialog, ChoiceList};

/// 32 桁 × 46 行: ダイアログがペインより高い = カーソルが最上段・箱の上端は画面の外（Issue の形）
const ASK_32X46: &str = include_str!("fixtures/issue1847-ask-32x46.txt");
/// 32 桁 × 70 行: 箱は全部見えるが、カーソルが下端から 40 行より上
const ASK_32X70: &str = include_str!("fixtures/issue1847-ask-32x70.txt");
/// 60 桁 × 50 行: 同じダイアログ（箱は 26 行で窓に収まる = 修正前から検知できる対照）
const ASK_60X50: &str = include_str!("fixtures/issue1847-ask-60x50.txt");

const LEGACY_ENV: &str = "TAKO_1847_LEGACY";
const LEGACY_TEST: &str = "issue1847_legacyで背の高いダイアログが非検知に戻る";

fn rows(s: &str) -> Vec<&str> {
    s.lines().collect()
}

fn numbers(list: &ChoiceList) -> Vec<Option<u32>> {
    list.options.iter().map(|o| o.number).collect()
}

/// 6 択（4 択 + `Type something.` + `Chat about this`）を番号つき・先頭ハイライトで読めている
fn assert_full_dialog(name: &str, screen: &str) -> ChoiceList {
    let list = detect_choice_list(&rows(screen))
        .unwrap_or_else(|| panic!("{name}: 選択肢ダイアログを検知できない"));
    assert!(list.numbered, "{name}: 番号つき");
    assert_eq!(
        numbers(&list),
        (1..=6).map(Some).collect::<Vec<_>>(),
        "{name}: 6 択を取りこぼさない（選択肢 1 も入る）: {:?}",
        list.options
    );
    assert_eq!(list.highlighted, Some(0), "{name}: ハイライトは選択肢 1");
    assert!(
        list.cursor_row.is_some(),
        "{name}: カーソルは画面に描かれている"
    );
    assert!(
        is_choice_dialog(&rows(screen)),
        "{name}: respond の実在検査も通る"
    );
    list
}

#[test]
fn issue1847_ペインより高い32桁のダイアログを検知する() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return; // 子プロセス（A/B の legacy の腕）では既定の腕を観ない
    }
    let list = assert_full_dialog("32x46", ASK_32X46);
    // 番号は画面の数字のまま（respond は番号キーを送る）
    assert_eq!(list.options[4].label, "Type something.");
    assert_eq!(list.options[5].label, "Chat about this");
}

#[test]
fn issue1847_カーソルが窓より上でも経路3の欠けた一覧へ落ちない() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    // 修正前は経路 3 へ落ち、選択肢 2〜6・カーソル無し（cursor_visible=false）と答えていた
    assert_full_dialog("32x70", ASK_32X70);
}

#[test]
fn issue1847_60桁の同じダイアログは修正前と同じに読める() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    let list = assert_full_dialog("60x50", ASK_60X50);
    // 60 桁ではラベルが折り返さない（経路 1 のまま = 今回の変更の外）
    assert_eq!(list.options[0].label, "status の判定を先に直す（推奨）");
    assert_eq!(list.options[3].label, "今回は調査だけにする");
}

/// 画面の最下部（確定キーの案内）を差し替える
fn replace_tail(screen: &str, tail: &str) -> String {
    let body: Vec<&str> = screen.lines().collect();
    let cut = body
        .iter()
        .rposition(|l| l.trim_start().starts_with("Enter to select"))
        .expect("案内の行がある");
    format!("{}\n{tail}", body[..cut].join("\n"))
}

#[test]
fn issue1847_下に入力欄があれば窓より上の番号つき行を拾わない() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    // 会話ログへ流れた解決済みのダイアログ（下に空の入力欄がある）の形。窓の中に
    // 選択カーソル（入力欄）があるので経路 5 へは来ない
    for (name, screen) in [("32x46", ASK_32X46), ("32x70", ASK_32X70)] {
        let rule = "─".repeat(32);
        let footer = format!("{rule}\n❯ \n{rule}\n  ⏵⏵ auto mode on (shift+tab");
        let resolved = format!("{screen}\n{footer}");
        assert_eq!(
            detect_choice_list(&rows(&resolved)),
            None,
            "{name}: 入力欄の上の古い並びをダイアログと読んだ"
        );
    }
}

#[test]
fn issue1847_並びの下が案内でなければ拾わない() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    for (name, screen) in [("32x46", ASK_32X46), ("32x70", ASK_32X70)] {
        // 並びの後に会話の本文（0 桁の行）が続く = 生きたダイアログではない
        let foreign = replace_tail(screen, "⏺ 了解です。続けます");
        assert_eq!(
            detect_choice_list(&rows(&foreign)),
            None,
            "{name}: 本文が続く並びをダイアログと読んだ"
        );
        // 確定キーの案内が 1 行も無い（経路 4 と同じく安全側 = 非検知）
        let no_hint = replace_tail(screen, "");
        assert_eq!(
            detect_choice_list(&rows(&no_hint))
                .filter(|l| l.cursor_row.is_some())
                .map(|l| numbers(&l)),
            None,
            "{name}: 案内の無い並びを経路 5 で拾った"
        );
    }
}

/// 同一バイナリの A/B: `TAKO_1847_LEGACY=1` で **#1847 の不検知が再現する**。
///
/// env は `OnceLock` でプロセス起動時に 1 度だけ読むので、legacy の腕は自分自身を
/// 子プロセスで起動して観る（`issue1447_legacy_ab` と同じ作法）
#[test]
fn issue1847_legacyで背の高いダイアログが非検知に戻る() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        // --- legacy（#1847 前）: Issue の症状そのもの ---
        assert_eq!(
            detect_choice_list(&rows(ASK_32X46)),
            None,
            "legacy で検知できてしまう（A/B の入口が効いていない）"
        );
        assert!(!is_choice_dialog(&rows(ASK_32X46)));
        // 32x70 は経路 3 の欠けた一覧（選択肢 1 が無い・カーソル無し）
        let partial = detect_choice_list(&rows(ASK_32X70)).expect("経路 3");
        assert_eq!(partial.cursor_row, None);
        assert_eq!(partial.options.first().and_then(|o| o.number), Some(2));
        eprintln!("[1847-ab] legacy: 32x46 -> None / 32x70 -> 選択肢 2〜6・カーソル無し");
        return;
    }
    let exe = std::env::current_exe().expect("テストバイナリの場所");
    let out = Command::new(exe)
        .args([LEGACY_TEST, "--exact", "--nocapture", "--test-threads=1"])
        .env(LEGACY_ENV, "1")
        .output()
        .expect("子プロセスを起動できる");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "legacy の腕が落ちた:\n{stdout}\n{stderr}"
    );
    assert!(
        stderr.contains("[1847-ab] legacy:") && stdout.contains("1 passed"),
        "legacy の腕が実際に走っていない（フィルタ外れ）:\n{stdout}\n{stderr}"
    );
}
