//! シェル統合の配置操作（#525）— CLI・MCP・dispatch が共有する 1 実装
//!
//! 判定と書き込みの本体は `tako_core::shell_integration`（抽象境界 B13）。
//! ここは「action の受理」と「応答 JSON の形」だけに責任を持つ。
//! `platform::report` と同じで **GUI を必要としない**ので、CLI からはローカル呼び出しで、
//! MCP からは dispatch 経由で、まったく同じ結果になる。

use serde_json::{json, Value};
use tako_core::shell_integration as si;

use crate::setup_remaining::{Remaining, RemainingKind};

/// 受理する操作。**既定は `status`**（#322: 素のコマンドが一番安全な既定で動く）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Status,
    Install,
    Uninstall,
}

impl Action {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "status" => Some(Self::Status),
            "install" => Some(Self::Install),
            "uninstall" => Some(Self::Uninstall),
            _ => None,
        }
    }

    pub const VALUES: &'static [&'static str] = &["status", "install", "uninstall"];
}

/// 操作を実行して応答 JSON を返す。
///
/// **どの action でも最後に `status` を載せる**。install / uninstall の直後に
/// 「いま実際にどうなっているか」を別呼び出しで確かめさせないため
/// （AI が 1 往復で完結できる = 開発不変条件 5 の趣旨）
pub fn run(action: Option<&str>) -> Result<Value, String> {
    let raw = action.unwrap_or("status");
    let action = Action::parse(raw).ok_or_else(|| {
        format!(
            "不明な action: {raw:?}（{} のいずれか）",
            Action::VALUES.join(" / ")
        )
    })?;

    let changes = match action {
        Action::Status => Vec::new(),
        Action::Install => si::install()?,
        Action::Uninstall => si::uninstall()?,
    };

    let mut out = si::status().describe();
    out["action"] = json!(raw);
    out["changes"] = json!(changes.iter().map(|c| c.describe()).collect::<Vec<_>>());
    Ok(out)
}

// --- `tako setup` の段（#1504 / #1500 の棚卸し Z9）---

/// setup の段の結果。**表示と「残り」だけを持つ**（呼び手は出して積むだけ）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageOutcome {
    /// そのまま 1 行ずつ出す表示（段組みの字下げを含む）
    pub lines: Vec<String>,
    /// 配置できなかったときに人へ残る作業（#1501）。成功時は `None`
    pub remaining: Option<Remaining>,
}

/// `TAKO_1504_LEGACY=1` で #1504 前（setup がシェル統合に触らない）へ戻す。
///
/// **製品の既定経路ではない**。同一バイナリで A/B を取るための逃げ道で、
/// 実経路テスト `scripts/test-setup-shell-integration-1504.sh` が
/// 「旧アームでは段が 1 行も出ない」ことを毎回確かめる
pub fn legacy_no_setup_stage() -> bool {
    std::env::var_os("TAKO_1504_LEGACY").is_some()
}

/// A/B のときに出す 1 行（**何が無効なのかを名乗る**。黙って落とすと
/// 「配置されない」が仕様なのか壊れたのか分からない）
const LEGACY_LINE: &str =
    "  [legacy] シェル統合の配置は TAKO_1504_LEGACY=1 で無効（#1504 前の挙動）";

/// `tako setup` のシェル統合の段（#1504）。
///
/// ## なぜ setup の段なのか
///
/// [`si::install`] の呼び手は CLI `tako shell-integration` と MCP だけで、**setup も
/// installer も呼んでいなかった**。macOS / Linux は spawn 時の環境変数注入で届くので
/// 人の手はゼロだが、**Windows は `$PROFILE` へブロックを追記しないと 1 行も効かない**
/// （OSC 7 / 133 = ペインの cwd 追従とコマンド実行状態、入力予測、自動命名の素材）。
/// つまり Windows の利用者は `tako shell-integration install` を**自分で見つけて打つまで**
/// 機能が欠けたままで、「ゼロコンフィグで一般ユーザーが使える」（エピック #1500）に反する。
///
/// ## 作法（#868 / #1499 / #1501 / #1502 と揃える）
///
/// - **ユーザーのファイルを触る前に「何をどこへ書くか」を出して同意扱いで続行する**。
///   `[y/N]` は出さない（#262 の質問ゼロ）。先例は #1502 の PATH 設置で、どちらも
///   「tako の管理ブロック 1 個」+「`uninstall` / `undo-path` で元のバイト列へ戻せる」
/// - **`--yes` / 非 TTY で分岐しない**。聞かないので入力を読む口を持たない
///   （GUI の初回起動・パイプ経由でも同じ経路が同じ結果になる）
/// - **止めない**。配置に失敗しても残り作業 1 件として脇に置き、setup は完走する（#1501）
/// - **`cfg!(windows)` で分岐しない**。配置が要るかどうかは [`si::Delivery`] が
///   宣言している（= macOS 上でも Profile 経路の表示と残りを検査できる）
pub fn run_setup_stage() -> StageOutcome {
    if legacy_no_setup_stage() {
        return StageOutcome {
            lines: vec![LEGACY_LINE.to_string()],
            remaining: None,
        };
    }
    let before = si::status();
    // 書く前に予告する（ユーザーのファイルを触るのはここだけ）
    let mut lines = announce_lines(&before);
    match si::install() {
        Ok(changes) => {
            lines.extend(result_lines(&before, &changes));
            StageOutcome {
                lines,
                remaining: None,
            }
        }
        Err(e) => {
            let (failure, remaining) = failure_lines(&e);
            lines.extend(failure);
            StageOutcome {
                lines,
                remaining: Some(remaining),
            }
        }
    }
}

/// `tako setup --check` の 1 行（**読み取りだけ**。配置も書き込みもしない）
pub fn check_line() -> String {
    if legacy_no_setup_stage() {
        return LEGACY_LINE.to_string();
    }
    check_line_for(&si::status())
}

/// `tako setup --check` が積む残り作業（未配置のときだけ 1 件）
pub fn check_remaining() -> Option<Remaining> {
    if legacy_no_setup_stage() {
        return None;
    }
    check_remaining_for(&si::status())
}

/// 配置が要る環境か。**判断はここ 1 か所**（`cfg!(windows)` を散らさない）
fn needs_placement(status: &si::Status) -> bool {
    status.delivery == si::Delivery::Profile
}

/// 器が OSC を落として「配置しても効かない」ときだけ出す注記（#525）
fn backend_note(status: &si::Status) -> Option<String> {
    status
        .blocked_by_backend
        .as_deref()
        .map(|reason| format!("         [注意] {reason}"))
}

/// 配置先 1 件をどうするか（予告に出す 1 行）
fn target_plan(target: &si::ProfileTarget) -> String {
    let action = match (target.installed, target.up_to_date) {
        (true, true) => "変更なし",
        (true, false) => "既存のブロックを差し替え",
        _ => "新しく追記",
    };
    format!(
        "         {}: {}（{action}）",
        target.label,
        target.path.display()
    )
}

/// 書く前の予告（**ユーザーのファイルへ何をどこへ書くか**）。
///
/// 環境変数注入で完結する環境（macOS / Linux）と、既に最新が入っている環境では
/// 1 行も出さない（冪等な再実行を静かに保つ = FR-2.14.8）
fn announce_lines(status: &si::Status) -> Vec<String> {
    if !needs_placement(status) || status.installed() {
        return Vec::new();
    }
    let mut lines = vec![format!(
        "  [設定] シェル統合: {} のプロファイルへ tako の管理ブロックを置きます",
        si::shells()
    )];
    lines.extend(status.targets.iter().map(target_plan));
    lines.push(
        "         置くのは tako の管理ブロック 1 個だけで、ファイルの他の行はそのまま残します"
            .to_string(),
    );
    lines.push(
        "         これで効くもの: ペインの cwd 追従・コマンド実行状態（OSC 7 / 133）・入力予測"
            .to_string(),
    );
    lines.push("         元に戻すには tako shell-integration uninstall".to_string());
    lines
}

/// 変更 1 件の見え方（`ChangeKind` の写しはここだけ）
fn change_label(kind: si::ChangeKind) -> &'static str {
    match kind {
        si::ChangeKind::Installed => "新規",
        si::ChangeKind::Updated => "更新",
        si::ChangeKind::Unchanged => "変更なし",
        si::ChangeKind::Removed => "解除",
        si::ChangeKind::Absent => "無し",
        si::ChangeKind::Deleted => "削除",
    }
}

/// 配置できたときの表示。
///
/// - 環境変数注入で完結する環境: 「ファイルは触っていない」と言う 1 行だけ
/// - 配置した: どのプロファイルがどうなったかと、**既存ペインには効かない**ことを言う
/// - 既に最新だった: 「配置済み（変更なし）」の 1 行だけ（冪等な再実行）
fn result_lines(before: &si::Status, changes: &[si::Change]) -> Vec<String> {
    let mut lines = Vec::new();
    if !needs_placement(before) {
        lines.push(format!(
            "  [OK] シェル統合: 環境変数の注入で有効です（{}。設定ファイルは書き換えません）",
            si::shells()
        ));
        lines.extend(backend_note(before));
        return lines;
    }
    let detail = changes
        .iter()
        .map(|change| format!("{}: {}", change.label, change_label(change.kind)))
        .collect::<Vec<_>>()
        .join(" / ");
    let wrote = changes
        .iter()
        .any(|change| !matches!(change.kind, si::ChangeKind::Unchanged));
    if wrote {
        lines.push(format!("  [OK] シェル統合: 配置しました（{detail}）"));
        lines.push("         いま開いているペインには反映されません（開き直すと有効）".to_string());
    } else {
        lines.push(format!("  [OK] シェル統合: 配置済み（{detail}）"));
    }
    lines.extend(backend_note(before));
    lines
}

/// 配置できなかったときの表示と、人へ残る 1 件（**止めない** = #1501）
fn failure_lines(error: &str) -> (Vec<String>, Remaining) {
    let lines = vec![
        format!("  [警告] シェル統合の配置を見送りました: {error}"),
        "         ペインの cwd 追従・コマンド実行状態・入力予測は働きません".to_string(),
        "         あとから tako shell-integration install で配置できます".to_string(),
    ];
    let remaining =
        Remaining::with_detail(RemainingKind::ShellIntegration, vec![error.to_string()]);
    (lines, remaining)
}

/// `--check` の 1 行（複数行になることもある）。**判定は段と同じ材料を読む**
fn check_line_for(status: &si::Status) -> String {
    if !needs_placement(status) {
        let mut line = format!(
            "  [OK] シェル統合: 環境変数の注入で有効（{}）",
            si::shells()
        );
        if let Some(note) = backend_note(status) {
            line.push('\n');
            line.push_str(&note);
        }
        return line;
    }
    if status.installed() {
        let labels = status
            .targets
            .iter()
            .map(|target| target.label.clone())
            .collect::<Vec<_>>()
            .join(" / ");
        let mut line = format!("  [OK] シェル統合: 配置済み（{labels}）");
        if let Some(note) = backend_note(status) {
            line.push('\n');
            line.push_str(&note);
        }
        return line;
    }
    if status.targets.is_empty() {
        return format!(
            "  [不足] シェル統合: 未配置（{} が見つかりません）",
            si::shells()
        );
    }
    let mut line = "  [不足] シェル統合: 未配置（ペインの cwd 追従・コマンド実行状態が働きません）"
        .to_string();
    for target in &status.targets {
        line.push('\n');
        line.push_str(&target_plan(target));
    }
    line
}

/// `--check` が積む残り作業（**未配置のときだけ**。配置済み / 注入だけの環境は 0 件）
fn check_remaining_for(status: &si::Status) -> Option<Remaining> {
    (needs_placement(status) && !status.installed())
        .then(|| Remaining::new(RemainingKind::ShellIntegration))
}

#[cfg(test)]
mod stage_tests {
    use std::path::PathBuf;

    use super::*;

    /// Windows 形の状態を組む（**macOS 上で Profile 経路を検査するための材料**。
    /// 実機を持たない CI でも表示・冪等・失敗の分岐を全部通せる）
    fn windows_status(targets: Vec<si::ProfileTarget>) -> si::Status {
        si::Status {
            delivery: si::Delivery::Profile,
            script: Some(PathBuf::from(
                r"C:\Users\winuser\AppData\Roaming\tako\shell-integration\tako.ps1",
            )),
            targets,
            blocked_by_backend: None,
        }
    }

    fn target(label: &str, installed: bool, up_to_date: bool) -> si::ProfileTarget {
        si::ProfileTarget {
            label: label.to_string(),
            exe: "pwsh.exe".to_string(),
            path: PathBuf::from(format!(r"C:\Users\winuser\Documents\{label}\profile.ps1")),
            installed,
            up_to_date,
        }
    }

    fn unix_status() -> si::Status {
        si::Status {
            delivery: si::Delivery::Automatic,
            script: Some(PathBuf::from("/tmp/tako/shell-integration/tako.bash")),
            targets: Vec::new(),
            blocked_by_backend: None,
        }
    }

    #[test]
    fn 注入で完結する環境は何も予告しない() {
        assert!(
            announce_lines(&unix_status()).is_empty(),
            "macOS / Linux はユーザーのファイルを触らないので予告するものが無い"
        );
        let lines = result_lines(&unix_status(), &[]);
        assert_eq!(lines.len(), 1, "1 行だけ: {lines:?}");
        assert!(lines[0].contains("環境変数の注入で有効"), "{lines:?}");
        assert!(
            lines[0].contains("設定ファイルは書き換えません"),
            "触らないことを言う: {lines:?}"
        );
    }

    #[test]
    fn 未配置なら何をどこへ書くかを出す() {
        let status = windows_status(vec![
            target("PowerShell 7", false, false),
            target("Windows PowerShell 5.1", false, false),
        ]);
        let lines = announce_lines(&status);
        let text = lines.join("\n");
        // **書き先を 1 件も隠さない**（ユーザーのファイルを触る前の説明責任）
        for target in &status.targets {
            assert!(
                text.contains(&target.path.display().to_string()),
                "{} の書き先が出ていない: {text}",
                target.label
            );
            assert!(text.contains(&target.label), "{text}");
        }
        assert!(text.contains("管理ブロック 1 個"), "何を書くか: {text}");
        assert!(
            text.contains("他の行はそのまま残します"),
            "既存を壊さないこと: {text}"
        );
        assert!(
            text.contains("tako shell-integration uninstall"),
            "元に戻す 1 行（#322 の最簡形）: {text}"
        );
        // 聞かない（同意扱いで続行する = #262 の質問ゼロ）
        assert!(!text.contains("[y/N]"), "質問を足している: {text}");
    }

    #[test]
    fn 配置済みなら予告せず変更なしと言う() {
        let status = windows_status(vec![target("PowerShell 7", true, true)]);
        assert!(
            announce_lines(&status).is_empty(),
            "2 回目は静か（冪等な再実行 = FR-2.14.8）"
        );
        let changes = vec![si::Change {
            label: "PowerShell 7".to_string(),
            path: status.targets[0].path.clone(),
            kind: si::ChangeKind::Unchanged,
        }];
        let lines = result_lines(&status, &changes);
        let text = lines.join("\n");
        assert!(text.contains("配置済み"), "{text}");
        assert!(text.contains("変更なし"), "{text}");
        assert!(
            !text.contains("開き直すと有効"),
            "何も書いていないのに再起動を促している: {text}"
        );
    }

    #[test]
    fn 配置したらどこがどうなったかと既存ペインへ効かないことを言う() {
        let status = windows_status(vec![
            target("PowerShell 7", false, false),
            target("Windows PowerShell 5.1", true, false),
        ]);
        let changes = vec![
            si::Change {
                label: "PowerShell 7".to_string(),
                path: status.targets[0].path.clone(),
                kind: si::ChangeKind::Installed,
            },
            si::Change {
                label: "Windows PowerShell 5.1".to_string(),
                path: status.targets[1].path.clone(),
                kind: si::ChangeKind::Updated,
            },
        ];
        let text = result_lines(&status, &changes).join("\n");
        assert!(text.contains("配置しました"), "{text}");
        assert!(text.contains("PowerShell 7: 新規"), "{text}");
        assert!(text.contains("Windows PowerShell 5.1: 更新"), "{text}");
        assert!(
            text.contains("開き直すと有効"),
            "既存ペインには効かないことを言う: {text}"
        );
    }

    #[test]
    fn 失敗しても止めず残り1件へ落とす() {
        let (lines, remaining) = failure_lines("PowerShell が見つからないため配置先がありません");
        let text = lines.join("\n");
        assert!(text.starts_with("  [警告]"), "{text}");
        assert!(
            text.contains("PowerShell が見つからない"),
            "理由を出す: {text}"
        );
        assert!(
            text.contains("tako shell-integration install"),
            "次に打つ 1 行: {text}"
        );
        assert_eq!(remaining.kind, RemainingKind::ShellIntegration);
        assert_eq!(
            remaining.kind.command().as_deref(),
            Some("tako shell-integration install"),
            "残りの側も同じ最簡コマンドを出す"
        );
        assert!(
            remaining
                .detail
                .iter()
                .any(|line| line.contains("PowerShell が見つからない")),
            "理由を黙って捨てていない: {:?}",
            remaining.detail
        );
    }

    #[test]
    fn checkは配置状況を読むだけで残りも判断する() {
        // 注入だけで効く環境: OK・残り 0 件
        let unix = unix_status();
        assert!(
            check_line_for(&unix).contains("[OK]"),
            "{}",
            check_line_for(&unix)
        );
        assert!(check_remaining_for(&unix).is_none());

        // 配置済み: OK・残り 0 件
        let done = windows_status(vec![target("PowerShell 7", true, true)]);
        assert!(check_line_for(&done).contains("配置済み"));
        assert!(check_remaining_for(&done).is_none());

        // 未配置: 不足・残り 1 件（書き先も出す）
        let todo = windows_status(vec![target("PowerShell 7", false, false)]);
        let line = check_line_for(&todo);
        assert!(line.contains("[不足]"), "{line}");
        assert!(line.contains("profile.ps1"), "書き先を出す: {line}");
        assert_eq!(
            check_remaining_for(&todo).map(|r| r.kind),
            Some(RemainingKind::ShellIntegration)
        );

        // 配置先が 1 つも無い（PowerShell 不在）
        let none = windows_status(Vec::new());
        assert!(
            check_line_for(&none).contains("見つかりません"),
            "{}",
            check_line_for(&none)
        );
        assert_eq!(
            check_remaining_for(&none).map(|r| r.kind),
            Some(RemainingKind::ShellIntegration),
            "配置先が無いことも「残り」として見える"
        );
    }

    #[test]
    fn 器がoscを落とすなら配置しても効かないことを言う() {
        let mut status = windows_status(vec![target("PowerShell 7", true, true)]);
        status.blocked_by_backend = Some("永続バックエンドが OSC を通さない".to_string());
        let text = result_lines(&status, &[]).join("\n");
        assert!(text.contains("[注意]"), "{text}");
        assert!(text.contains("OSC を通さない"), "{text}");
        assert!(
            check_line_for(&status).contains("[注意]"),
            "--check でも同じ注記が出る: {}",
            check_line_for(&status)
        );
    }

    /// 実行環境そのものを相手にする 1 本（macOS では注入経路・Windows では配置経路）。
    /// **どちらでも「段は表示を返して人へ残りを丸投げしない」**ことだけを見る
    #[test]
    fn 実環境の段は表示を返す() {
        if legacy_no_setup_stage() {
            return; // A/B アームでは段が 1 行だけ返る（下のテストが見る）
        }
        let outcome = run_setup_stage();
        assert!(!outcome.lines.is_empty(), "段が何も言わずに終わっている");
        assert!(
            outcome.lines[0].starts_with("  ["),
            "段組みの字下げとタグが崩れている: {:?}",
            outcome.lines
        );
        if cfg!(unix) {
            assert!(
                outcome.remaining.is_none(),
                "unix は配置するものが無いので残りは出ない: {:?}",
                outcome.remaining
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_の受理と拒否() {
        assert_eq!(Action::parse("status"), Some(Action::Status));
        assert_eq!(Action::parse("install"), Some(Action::Install));
        assert_eq!(Action::parse("uninstall"), Some(Action::Uninstall));
        assert_eq!(Action::parse("Install"), None, "大文字は受理しない");
        assert_eq!(Action::parse(""), None);
        assert_eq!(Action::parse("remove"), None);
    }

    #[test]
    fn 既定は_status_で状態だけ返す() {
        let out = run(None).expect("status は常に成功する");
        assert_eq!(out["action"], "status");
        assert_eq!(out["changes"], json!([]));
        // 応答の形（CLI / MCP が読むキー）が揃っていること
        for key in [
            "delivery",
            "shells",
            "installed",
            "effective",
            "targets",
            "blocked_by_backend",
        ] {
            assert!(out.get(key).is_some(), "{key} が応答に無い: {out}");
        }
    }

    #[test]
    fn 不明な_action_は選択肢つきで拒否する() {
        let err = run(Some("enable")).expect_err("不明な action は失敗する");
        assert!(err.contains("enable"), "{err}");
        // 何を渡せばいいかがエラーだけで分かること
        for v in Action::VALUES {
            assert!(err.contains(v), "選択肢 {v} が案内に無い: {err}");
        }
    }

    /// unix は env 注入で完結するので配置対象を持たない。
    /// **「配置済み」と報告されるが「解除するものは無い」**という組み合わせを固定する
    #[cfg(unix)]
    #[test]
    fn unix_は自動配置で解除対象を持たない() {
        let out = run(None).unwrap();
        assert_eq!(out["delivery"], "automatic");
        assert_eq!(out["installed"], true);
        assert_eq!(out["targets"], json!([]));

        let err = run(Some("uninstall")).expect_err("解除する配置が無い");
        assert!(err.contains("環境変数"), "{err}");
    }
}
