//! auto mode の確認「Auto mode can't check actions in this session」の検知と応答（Issue #2841）
//!
//! # 何が起きていたか
//!
//! claude 2.1.319 / 2.1.322 は auto mode の安全確認（サーバー側の分類器）がこのセッションで届かないと
//! （`ANTHROPIC_BASE_URL` のゲートウェイ経由・実験的 beta の無効化・組織の方針ほか）、確認が要る最初の
//! 操作でこの確認を出して入力欄を奪う。**選択カーソルも番号も無く**、下端の確定キーの案内
//! `Enter to stay in auto mode · shift+Tab to leave for accept edits mode and stop · Esc to cancel`
//! のキーで答える形なので、選択肢の並びの構造検知（`tako_core::dialog`）に載らなかった。
//! 修正前は `choice_dialog` が null・状態照会が waiting へ上がらず・watch は `WORKER_DIALOG` を出さず・
//! respond は「ダイアログが無い」と断った = worker を `--permission-mode auto` で起こす運用では、
//! この確認で止まった worker に master が気付けない。
//!
//! # fixture（すべて実採取）
//!
//! 本物の claude 2.1.322 を一時の `CLAUDE_CONFIG_DIR` + 偽の Messages API
//! （`scripts/lib/fake-messages-api.mjs`。ダミーの API キー）へ繋ぎ、`--permission-mode auto` で
//! 隔離 tmux に起こして `Run this command: python3 -c "print(1)"` を送った画面。ヘッダのパスは `~/work`。
//!
//! - `auto-mode-110x40` / `80x40` / `60x40`: 確認（60 桁では案内が 2 行に折り返る）
//! - `auto-mode-80x16`: ペインより高い確認（claude は箱を上端から描いて下を切る = 案内が画面外）
//! - `stay-permission-110x40`: Enter（auto mode に留まる）の直後の操作ごとの承認（`❯ No` / `Yes`）
//! - `permission-again-110x40`: 留まった後の 2 回目の操作（確認は出ず承認だけ）
//! - `answered-110x40`: 留まって Yes で通した後の入力待ち（確認の文が会話に残る）
//!
//! # A/B
//!
//! `TAKO_2841_LEGACY=1` で修正前（検知しない）へ戻る（env は `OnceLock` で 1 度だけ読むので、
//! legacy の腕は `run_alone!` の子で観る）。

use std::sync::Mutex;

use tako_control::claude_tui::{self, DialogKind, AUTO_MODE_UNCHECKED_TITLE};
use tako_control::orchestrator::wait::{self, WorkerEventKind};
use tako_control::reach::DialogAccess;

const AUTO_110: &str = include_str!("fixtures/issue2841-auto-mode-110x40.txt");
const AUTO_80: &str = include_str!("fixtures/issue2841-auto-mode-80x40.txt");
const AUTO_60: &str = include_str!("fixtures/issue2841-auto-mode-60x40.txt");
const AUTO_80X16: &str = include_str!("fixtures/issue2841-auto-mode-80x16.txt");
const STAY_PERMISSION: &str = include_str!("fixtures/issue2841-stay-permission-110x40.txt");
const PERMISSION_AGAIN: &str = include_str!("fixtures/issue2841-permission-again-110x40.txt");
const ANSWERED: &str = include_str!("fixtures/issue2841-answered-110x40.txt");

/// 実測の 3 択（ラベル = 案内の動詞・キー = `key_name_bytes` の語彙）
const MEASURED: [(&str, &str); 3] = [
    ("stay in auto mode", "Enter"),
    ("leave for accept edits mode and stop", "BTab"),
    ("cancel", "Escape"),
];

const LEGACY_ENV: &str = "TAKO_2841_LEGACY";

fn lines(screen: &str) -> Vec<String> {
    screen.lines().map(str::to_string).collect()
}

/// 確認の画面（案内が見えている 3 幅と、案内が画面外の 1 枚）
fn auto_mode_screens() -> [(&'static str, &'static str, bool); 4] {
    [
        ("110x40", AUTO_110, true),
        ("80x40", AUTO_80, true),
        ("60x40", AUTO_60, true),
        ("80x16", AUTO_80X16, false),
    ]
}

/// 修正の芯: 4 枚とも auto mode の確認として、実測の 3 択とキーで読む
#[test]
fn issue2841_実採取の確認をキーつきの3択として読む() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    for (name, screen, hint_visible) in auto_mode_screens() {
        let screen = lines(screen);
        let dialog = claude_tui::detect_choice_dialog(&screen).unwrap_or_else(|| {
            panic!("{name}: auto mode の確認を検知しない（choice_dialog が null）")
        });
        assert_eq!(dialog.kind, DialogKind::AutoModeUnchecked, "{name}");
        assert_eq!(
            dialog.labels(),
            MEASURED.map(|(label, _)| label.to_string()).to_vec(),
            "{name}: 選択肢"
        );
        let keys: Vec<Option<String>> = MEASURED.map(|(_, k)| Some(k.to_string())).to_vec();
        assert_eq!(dialog.answer_keys.as_ref(), Some(&keys), "{name}: キー");
        assert_eq!(
            dialog.highlighted,
            Some(0),
            "{name}: 既定の答え = Enter = 留まる"
        );
        assert!(!dialog.numbered, "{name}: 番号は無い");
        assert_eq!(
            dialog.cursor_visible, hint_visible,
            "{name}: 案内が見えているか"
        );
        assert!(
            dialog.title.starts_with(AUTO_MODE_UNCHECKED_TITLE)
                && dialog.title.contains("safety check runs on the server"),
            "{name}: 見出しと原因の段落が title に入る: {}",
            dialog.title
        );
        assert!(
            !dialog.title.contains("You can stay"),
            "{name}: 答え方の説明は title に入れない（note が持つ）: {}",
            dialog.title
        );
        // 入力欄の判定・送達フローも同じ判定を通る（選択肢の並びのダイアログと同じ扱い）
        assert!(claude_tui::is_choice_dialog(&screen), "{name}");
        assert_eq!(claude_tui::input_line(&screen), None, "{name}");
        assert_eq!(
            claude_tui::detect(&screen),
            claude_tui::ClaudeScreen::ChoiceDialog,
            "{name}"
        );
        // 操作ごとの承認（permission）ではない = WORKER_PERMISSION へは行かない
        assert_eq!(
            claude_tui::detect_permission_dialog(&screen),
            None,
            "{name}"
        );
    }
}

/// 状態照会（worker_status の `choice_dialog`）と watch の材料（events）が同じ種別を名指し、
/// 応答に原因と答え方の案内（`note`）が載る
#[test]
fn issue2841_状態照会とwatchの材料が種別と案内を返す() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    for (name, screen, hint_visible) in auto_mode_screens() {
        let dialog = wait::choice_dialog_json(screen)
            .unwrap_or_else(|| panic!("{name}: worker_status の choice_dialog が null"));
        assert_eq!(dialog["kind"], "auto_mode_unchecked", "{name}");
        assert_eq!(dialog["recommended_action"], "respond", "{name}");
        assert_eq!(
            dialog["auto_accepted"], false,
            "{name}: tako は勝手に答えない"
        );
        assert_eq!(dialog["numbered"], false, "{name}");
        assert_eq!(dialog["cursor_visible"], hint_visible, "{name}");
        let keys: Vec<&str> = dialog["options"]
            .as_array()
            .expect("options")
            .iter()
            .map(|o| o["key"].as_str().expect("キーを持つ"))
            .collect();
        assert_eq!(keys, ["Enter", "BTab", "Escape"], "{name}");
        let note = dialog["note"]
            .as_str()
            .unwrap_or_else(|| panic!("{name}: 原因と答え方の案内（note）が無い: {dialog}"));
        for want in [
            "auto mode",
            "stay in auto mode",
            "WORKER_PERMISSION",
            "cancel",
        ] {
            assert!(
                note.contains(want),
                "{name}: note に「{want}」が無い: {note}"
            );
        }
        // watch: 停止側の状態ならダイアログの事象として種別つきで出る（question にしない）
        let events = wait::collect_worker_events_on("waiting", Some(screen), Some(screen), None);
        assert!(
            events.iter().any(|e| matches!(
                &e.kind,
                WorkerEventKind::ChoiceDialog { dialog_kind, recommended_action }
                    if dialog_kind == "auto_mode_unchecked" && recommended_action == "respond"
            )),
            "{name}: events に auto_mode_unchecked の choice_dialog が無い: {events:?}"
        );
        assert!(
            !events.iter().any(|e| matches!(
                e.kind,
                WorkerEventKind::Question | WorkerEventKind::PermissionDialog
            )),
            "{name}: 質問・承認として通知しない: {events:?}"
        );
        assert_eq!(wait::permission_dialog_json(screen), None, "{name}");
    }
    // 種別 → 案内の対応（watch の CLI は応答の kind から案内を引き直す）
    assert_eq!(
        DialogKind::from_slug("auto_mode_unchecked"),
        Some(DialogKind::AutoModeUnchecked)
    );
    assert!(DialogKind::AutoModeUnchecked.note().is_some());
    for other in [
        DialogKind::Permission,
        DialogKind::Trust,
        DialogKind::Bypass,
        DialogKind::UsageLimit,
        DialogKind::PlanConfirm,
        DialogKind::Select,
    ] {
        assert_eq!(DialogKind::from_slug(other.as_str()), Some(other));
        assert_eq!(other.note(), None, "{other:?} に案内は無い");
    }
}

/// 留まった後の承認・答えた後の入力待ちは奪わない（見出しの文は会話・承認ダイアログにも残る）
#[test]
fn issue2841_留まった後の承認と答えた後の会話はauto_modeの確認にしない() {
    for (name, screen) in [
        ("stay-permission", STAY_PERMISSION),
        ("permission-again", PERMISSION_AGAIN),
    ] {
        let screen = lines(screen);
        let dialog = claude_tui::detect_choice_dialog(&screen)
            .unwrap_or_else(|| panic!("{name}: 承認ダイアログを検知しない"));
        assert_eq!(dialog.kind, DialogKind::Permission, "{name}");
        assert_eq!(dialog.labels(), ["No", "Yes"], "{name}");
        assert_eq!(dialog.highlighted, Some(0), "{name}: 既定は No");
        assert_eq!(dialog.answer_keys, None, "{name}: 従来の矢印 + Enter の形");
        assert!(
            claude_tui::detect_permission_dialog(&screen).is_some(),
            "{name}: WORKER_PERMISSION で通知される"
        );
    }
    let answered = lines(ANSWERED);
    assert!(
        answered
            .iter()
            .any(|l| l.contains(AUTO_MODE_UNCHECKED_TITLE)),
        "前提: 答えた後も確認の文が会話に残っている"
    );
    assert_eq!(claude_tui::detect_choice_dialog(&answered), None);
    assert!(!claude_tui::is_choice_dialog(&answered));
    assert_eq!(claude_tui::input_line(&answered), Some(""));
    assert_eq!(wait::choice_dialog_json(ANSWERED), None);
}

/// 錨の取り違えを名指す: 罫線の無い見出しの行・入力欄の上の案内は採らない
#[test]
fn issue2841_錨の境界() {
    // 応答の本文として見出しの文だけが 1 行に出た（罫線の下ではない・案内も無い）
    let quoted = format!(
        "⏺ The dialog said:\n\n  {AUTO_MODE_UNCHECKED_TITLE}\n\n  and stopped.\n\n{}\n❯ \n{}\n",
        "─".repeat(40),
        "─".repeat(40)
    );
    assert_eq!(claude_tui::detect_choice_dialog(&lines(&quoted)), None);
    // 案内の文が会話に残り、その下に入力欄がある（答えた後）
    let hint_then_input = format!(
        "  Enter to stay in auto mode · shift+Tab to leave for accept edits mode and stop · Esc to cancel\n{}\n❯ \n{}\n",
        "─".repeat(40),
        "─".repeat(40)
    );
    assert_eq!(
        claude_tui::detect_choice_dialog(&lines(&hint_then_input)),
        None
    );
    // 確認の表示中に claude が終わり、シェルのプロンプトが下に出た画面（入力欄の記号ではない）
    let exited = format!(
        "{}\n%                                                    \nuser@host work % \n",
        AUTO_110.trim_end()
    );
    assert_eq!(claude_tui::detect_choice_dialog(&lines(&exited)), None);
    // 空の画面・別の案内（AskUserQuestion の下端）だけの画面
    assert_eq!(claude_tui::detect_choice_dialog(&[]), None);
    assert_eq!(
        claude_tui::detect_choice_dialog(&lines("  Esc to cancel · Tab to amend\n")),
        None
    );
}

/// 実測の画面の遷移を返す偽の claude（`respond_via` の届き方）。送られたキーを覚え、
/// 2.1.322 の実測どおりに画面を変える: Enter = 留まる → 操作ごとの承認、shift+Tab / Esc = ターンが止まり
/// 入力欄へ戻る（どちらも `Interrupted`。shift+Tab は accept edits へ移る）。
///
/// **描いた直後のキーは捨てる**（隔離 GUI の実測: 確認の検出の直後に送った Enter / shift+Tab は効かず、
/// 0.5 秒置けば効く。Esc だけは直後でも効く）。偽物は作った時点を「描いた時点」とし、
/// [`FAKE_MOUNT_GUARD`] より前に届いた Enter / shift+Tab では画面を変えない
struct FakeClaude {
    screen: Mutex<Vec<String>>,
    sent: Mutex<Vec<String>>,
    shown_at: std::time::Instant,
}

/// 偽の claude が描いた直後のキーを捨てる時間（実測の「0 秒は捨て 0.5 秒は通る」の内側）
const FAKE_MOUNT_GUARD: std::time::Duration = std::time::Duration::from_millis(300);

impl FakeClaude {
    fn showing(screen: &str) -> Self {
        Self {
            screen: Mutex::new(lines(screen)),
            sent: Mutex::new(Vec::new()),
            shown_at: std::time::Instant::now(),
        }
    }

    fn sent(&self) -> Vec<String> {
        self.sent.lock().unwrap().clone()
    }
}

impl DialogAccess for FakeClaude {
    fn capture(&self) -> Result<Vec<String>, String> {
        Ok(self.screen.lock().unwrap().clone())
    }

    fn send_key(&self, key: &str) -> Result<(), String> {
        self.sent.lock().unwrap().push(key.to_string());
        if key != "Escape" && self.shown_at.elapsed() < FAKE_MOUNT_GUARD {
            return Ok(()); // 描いた直後のキーは効かない（実測の claude と同じ）
        }
        let next = match key {
            "Enter" => Some(STAY_PERMISSION.to_string()),
            "BTab" => Some(ANSWERED.replace("auto mode on", "accept edits on")),
            "Escape" => Some(ANSWERED.to_string()),
            _ => None, // 番号キー・矢印はこの確認では無反応（実測）
        };
        if let Some(next) = next {
            *self.screen.lock().unwrap() = lines(&next);
        }
        Ok(())
    }

    fn route(&self) -> &'static str {
        "test-fake-2841"
    }
}

/// respond: 番号・ラベルのどちらでも、選んだ選択肢のキーを 1 つだけ送って先へ進む
#[test]
fn issue2841_respondは選んだ選択肢のキーを1つ送って解消する() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    let cases: [(&str, &str, &str, &str); 7] = [
        ("110x40", AUTO_110, "1", "Enter"),
        ("110x40", AUTO_110, "stay", "Enter"),
        ("110x40", AUTO_110, "leave", "BTab"),
        ("110x40", AUTO_110, "3", "Escape"),
        ("60x40", AUTO_60, "cancel", "Escape"),
        // 案内が画面外でも実測の並びで答えられる（番号で）
        ("80x16", AUTO_80X16, "1", "Enter"),
        ("80x16", AUTO_80X16, "2", "BTab"),
    ];
    for (name, screen, choice, key) in cases {
        let fake = FakeClaude::showing(screen);
        let out = tako_control::dispatch::respond_via(&fake, 7, Some(choice), Some("test"))
            .unwrap_or_else(|e| panic!("{name} --choice {choice}: 応答できない: {e}"));
        assert_eq!(fake.sent(), [key], "{name} --choice {choice}: 送ったキー");
        assert_eq!(out["keys_sent"], serde_json::json!([key]), "{name}");
        assert_eq!(out["kind"], "auto_mode_unchecked", "{name}");
        assert_eq!(out["resolved"], true, "{name} --choice {choice}: {out}");
        assert_eq!(out["stray_input"], false, "{name}");
    }
    // Enter（留まる）の先は操作ごとの承認 = 次の応答は従来の矢印 + Enter で通る
    let fake = FakeClaude::showing(AUTO_110);
    tako_control::dispatch::respond_via(&fake, 7, Some("stay"), Some("test")).expect("留まる");
    let next = claude_tui::detect_choice_dialog(&fake.capture().unwrap()).expect("承認が出る");
    assert_eq!(next.kind, DialogKind::Permission);
}

/// respond の下見（choice 省略）は何も送らず、キーと案内つきの構造を返す
#[test]
fn issue2841_respondの下見はキーと案内を返し何も送らない() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    let fake = FakeClaude::showing(AUTO_80);
    let out = tako_control::dispatch::respond_via(&fake, 7, None, Some("test")).expect("下見");
    assert!(fake.sent().is_empty(), "下見で送った: {:?}", fake.sent());
    assert_eq!(out["responded"], false);
    assert_eq!(out["kind"], "auto_mode_unchecked");
    assert_eq!(out["options"][0]["key"], "Enter");
    assert_eq!(out["options"][0]["highlighted"], true);
    assert!(out["note"].as_str().is_some_and(|n| !n.is_empty()), "{out}");
}

/// 案内の綴りを送れるキー名へ読めない選択肢は推測で送らない（断って何も送らない）
#[test]
fn issue2841_読めないキーの選択肢は何も送らずに断る() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    let odd = AUTO_110.replace("shift+Tab to leave", "F9 to leave");
    let dialog = claude_tui::detect_choice_dialog(&lines(&odd)).expect("案内の形は保っている");
    assert_eq!(
        dialog.answer_keys,
        Some(vec![Some("Enter".into()), None, Some("Escape".into())])
    );
    let fake = FakeClaude::showing(&odd);
    let err = tako_control::dispatch::respond_via(&fake, 7, Some("leave"), Some("test"))
        .expect_err("読めないキーは送らない");
    assert!(fake.sent().is_empty(), "送った: {:?}", fake.sent());
    let msg = err.to_string();
    assert!(
        msg.contains("leave for accept edits mode and stop"),
        "断りが選択肢を名指していない: {msg}"
    );
    // 読める選択肢には答えられる
    let fake = FakeClaude::showing(&odd);
    tako_control::dispatch::respond_via(&fake, 7, Some("stay"), Some("test"))
        .expect("Enter は送れる");
    assert_eq!(fake.sent(), ["Enter"]);
}

/// 同一バイナリの A/B: `TAKO_2841_LEGACY=1` で修正前（検知しない = master から見えない）へ戻る
#[test]
fn issue2841_legacyではauto_modeの確認を見落とす() {
    if tako_core::run_alone!(issue2841_legacyではauto_modeの確認を見落とす) {
        return;
    }
    std::env::set_var(LEGACY_ENV, "1");
    for (name, screen, _) in auto_mode_screens() {
        assert_eq!(
            claude_tui::detect_choice_dialog(&lines(screen)),
            None,
            "{name}: legacy でも検知してしまう（A/B の腕が効いていない）"
        );
        assert_eq!(wait::choice_dialog_json(screen), None, "{name}");
        let fake = FakeClaude::showing(screen);
        assert!(
            tako_control::dispatch::respond_via(&fake, 7, Some("1"), Some("test")).is_err(),
            "{name}: legacy の respond は「ダイアログが無い」で断る"
        );
        assert!(fake.sent().is_empty(), "{name}");
    }
    // 留まった後の承認は修正前から検知できていた（変えていない）
    let dialog = claude_tui::detect_choice_dialog(&lines(STAY_PERMISSION)).expect("従来の検知");
    assert_eq!(dialog.kind, DialogKind::Permission);
    eprintln!("[2841-ab] legacy: auto mode の確認 4 枚とも choice_dialog=null・respond は断る");
}
