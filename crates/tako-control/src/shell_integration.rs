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

// --- `tako setup` の段（#1504 / #1500 の棚卸し Z9 / #2432）---

/// setup の段の結果。**表示と「残り」だけを持つ**（呼び手は出して積むだけ）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageOutcome {
    /// そのまま 1 行ずつ出す表示（段組みの字下げを含む）
    pub lines: Vec<String>,
    /// 用意できなかったときに人へ残る作業（#1501）。成功時は `None`
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

/// `tako setup` のシェル統合の段（#1504 / #2432）。
///
/// ## 何をする段か
///
/// [`si::install`] で統合スクリプトを data dir へ最新の形で書き出し、**この環境でどう
/// 届くか**を 1 行で言う。#1504 の時点の Windows は `$PROFILE` へブロックを書かないと
/// 1 行も効かなかったので、この段がその配置を担っていた。#2432 から**どの OS も
/// ユーザーのファイルを書かない**（Windows は tako がペインの PowerShell を起こすときの
/// 起動引数で読ませる = インストール直後・setup の前から効く）。以前の版が置いた
/// `$PROFILE` のブロックは、この段より先に走る移行の段（`migrations::setup_lines`）が取り除く
///
/// ## 作法（#868 / #1499 / #1501 / #1502 と揃える）
///
/// - **聞かない**（#262 の質問ゼロ）。ユーザーのファイルを書かないので予告も要らない
/// - **`--yes` / 非 TTY で分岐しない**。聞かないので入力を読む口を持たない
///   （GUI の初回起動・パイプ経由でも同じ経路が同じ結果になる）
/// - **止めない**。書き出しに失敗しても残り作業 1 件として脇に置き、setup は完走する（#1501）
/// - **`cfg!(windows)` で分岐しない**。どう届くかは [`si::Delivery`] が宣言している
///   （= macOS 上でも Windows の Launch 経路の表示を検査できる）
pub fn run_setup_stage() -> StageOutcome {
    if legacy_no_setup_stage() {
        return StageOutcome {
            lines: vec![LEGACY_LINE.to_string()],
            remaining: None,
        };
    }
    match si::install() {
        Ok(_) => StageOutcome {
            lines: state_lines(&si::status(), Phase::Setup),
            remaining: None,
        },
        Err(e) => {
            let (lines, remaining) = failure_lines(&e);
            StageOutcome {
                lines,
                remaining: Some(remaining),
            }
        }
    }
}

/// `tako setup --check` の 1 行（**読み取りだけ**。書き出しも書き込みもしない）
pub fn check_line() -> String {
    if legacy_no_setup_stage() {
        return LEGACY_LINE.to_string();
    }
    check_line_for(&si::status())
}

/// `tako setup --check` が積む残り作業。#2432 以降は人の手で配置するものが無いので、
/// 統合が効かない環境（既定シェルが PowerShell でない）でも `tako` のコマンドでは直せず、
/// 積まない（`[不足]` の行で知らせるだけ）
pub fn check_remaining() -> Option<Remaining> {
    if legacy_no_setup_stage() {
        return None;
    }
    check_remaining_for(&si::status())
}

/// 段（`tako setup`）と点検（`--check`）のどちらの文面か
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Setup,
    Check,
}

/// どう届くかの表示。**判断はここ 1 か所**（`cfg!(windows)` を散らさない）
fn state_lines(status: &si::Status, phase: Phase) -> Vec<String> {
    let mut lines = vec![delivery_line(status, phase)];
    if status.installed() {
        lines.extend(backend_note(status));
    }
    lines.extend(legacy_note(status));
    lines
}

/// 届け方の 1 行。`si::Delivery` の 2 値で分ける
fn delivery_line(status: &si::Status, phase: Phase) -> String {
    match (status.delivery, phase) {
        (si::Delivery::Automatic, Phase::Setup) => format!(
            "  [OK] シェル統合: 環境変数の注入で有効です（{}。設定ファイルは書き換えません）",
            si::shells()
        ),
        (si::Delivery::Automatic, Phase::Check) => format!(
            "  [OK] シェル統合: 環境変数の注入で有効（{}）",
            si::shells()
        ),
        (si::Delivery::Launch, _) if !status.installed() => format!(
            "  [不足] シェル統合: 既定のシェルが PowerShell でないため効きません（{} が見つかりません）",
            si::shells()
        ),
        (si::Delivery::Launch, Phase::Setup) => format!(
            "  [OK] シェル統合: tako が開く PowerShell へ起動時に読ませます（{}。$PROFILE は書き換えません）",
            si::shells()
        ),
        (si::Delivery::Launch, Phase::Check) => format!(
            "  [OK] シェル統合: tako が開く PowerShell へ起動時に読ませる（{}）",
            si::shells()
        ),
    }
}

/// 器が OSC を落として「届け方が整っていても効かない」ときだけ出す注記（#525）
fn backend_note(status: &si::Status) -> Option<String> {
    status
        .blocked_by_backend
        .as_deref()
        .map(|reason| format!("         [注意] {reason}"))
}

/// 以前の版が `$PROFILE` に置いたブロックが残っているときだけ出す注記（#2432）。
/// 通常は移行が取り除くので出ない。出ているなら移行が失敗している（理由は `tako migrate`）。
/// 直し方はブロックだけを外す `uninstall`（`tako migrate run` は利用者の判断が要る案まで当てる）
fn legacy_note(status: &si::Status) -> Vec<String> {
    if status.legacy_blocks.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![
        "         [注意] 以前の版が $PROFILE に置いた管理ブロックが残っています（取り除くには tako shell-integration uninstall）"
            .to_string(),
    ];
    lines.extend(
        status
            .legacy_blocks
            .iter()
            .map(|block| format!("           {}: {}", block.label, block.path.display())),
    );
    lines
}

/// 用意できなかったときの表示と、人へ残る 1 件（**止めない** = #1501）。
/// 「配置」はデータディレクトリへのスクリプトの配置（ユーザーのファイルではない）
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
    state_lines(status, Phase::Check).join("\n")
}

/// `--check` が積む残り作業。人の手で配置するものが無い（#2432）ので積まない
fn check_remaining_for(_status: &si::Status) -> Option<Remaining> {
    None
}

#[cfg(test)]
mod stage_tests {
    use std::path::PathBuf;

    use super::*;

    /// Windows 形の状態を組む（**macOS 上で Launch 経路を検査するための材料**。
    /// 実機を持たない CI でも表示・注記・失敗の分岐を全部通せる）
    fn windows_status(launch_shell: Option<&str>) -> si::Status {
        si::Status {
            delivery: si::Delivery::Launch,
            script: Some(PathBuf::from(
                r"C:\Users\winuser\AppData\Roaming\tako\shell-integration\tako.ps1",
            )),
            launch_shell: launch_shell.map(str::to_string),
            legacy_blocks: Vec::new(),
            blocked_by_backend: None,
        }
    }

    fn legacy(label: &str) -> si::LegacyBlock {
        si::LegacyBlock {
            label: label.to_string(),
            path: PathBuf::from(format!(r"C:\Users\winuser\Documents\{label}\profile.ps1")),
        }
    }

    fn unix_status() -> si::Status {
        si::Status {
            delivery: si::Delivery::Automatic,
            script: Some(PathBuf::from("/tmp/tako/shell-integration/tako.bash")),
            launch_shell: None,
            legacy_blocks: Vec::new(),
            blocked_by_backend: None,
        }
    }

    const PWSH: &str = r"C:\Program Files\PowerShell\7\pwsh.exe";

    #[test]
    fn 注入で完結する環境は1行だけ言う() {
        let lines = state_lines(&unix_status(), Phase::Setup);
        assert_eq!(lines.len(), 1, "1 行だけ: {lines:?}");
        assert!(lines[0].contains("環境変数の注入で有効"), "{lines:?}");
        assert!(
            lines[0].contains("設定ファイルは書き換えません"),
            "触らないことを言う: {lines:?}"
        );
    }

    /// #2432: Windows も**ユーザーのファイルを書かない**ことを言い切る。予告も
    /// 「開き直すと有効」も要らない（書いていないし、新しいペインから効く）
    #[test]
    fn windowsは起動時に読ませて_profileを書き換えないと言う() {
        let lines = state_lines(&windows_status(Some(PWSH)), Phase::Setup);
        let text = lines.join("\n");
        assert_eq!(lines.len(), 1, "{text}");
        assert!(text.starts_with("  [OK]"), "{text}");
        assert!(text.contains("起動時に読ませます"), "{text}");
        assert!(text.contains("$PROFILE は書き換えません"), "{text}");
        assert!(
            !text.contains("管理ブロック 1 個"),
            "旧方式の予告が残っている: {text}"
        );
        assert!(!text.contains("[y/N]"), "質問を足している: {text}");
    }

    /// 既定シェルが PowerShell でない（cmd.exe へ落ちた）環境は「不足」と言う。
    /// `tako` のコマンドでは直せないので残り作業には積まない
    #[test]
    fn powershellが無ければ不足と言い残りには積まない() {
        let status = windows_status(None);
        let line = check_line_for(&status);
        assert!(line.starts_with("  [不足]"), "{line}");
        assert!(line.contains("PowerShell でない"), "{line}");
        assert!(check_remaining_for(&status).is_none());
    }

    /// 以前の版のブロックが残っている（移行が失敗した）ときだけ、場所と直し方を出す
    #[test]
    fn 旧ブロックが残っていれば場所と直し方を出す() {
        let mut status = windows_status(Some(PWSH));
        status.legacy_blocks = vec![legacy("PowerShell"), legacy("WindowsPowerShell")];
        let text = check_line_for(&status);
        assert!(text.contains("[注意]"), "{text}");
        assert!(
            text.contains("tako shell-integration uninstall"),
            "直し方（ブロックだけを外す最簡形）: {text}"
        );
        for block in &status.legacy_blocks {
            assert!(
                text.contains(&block.path.display().to_string()),
                "{} の場所が出ていない: {text}",
                block.label
            );
        }
        // 残っていなければ注記は出ない
        assert!(!check_line_for(&windows_status(Some(PWSH))).contains("[注意]"));
    }

    #[test]
    fn 失敗しても止めず残り1件へ落とす() {
        let (lines, remaining) = failure_lines("統合スクリプトを書き出せません: 書き込み禁止");
        let text = lines.join("\n");
        assert!(text.starts_with("  [警告]"), "{text}");
        assert!(text.contains("書き込み禁止"), "理由を出す: {text}");
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
                .any(|line| line.contains("書き込み禁止")),
            "理由を黙って捨てていない: {:?}",
            remaining.detail
        );
    }

    #[test]
    fn checkは状況を読むだけで残りを積まない() {
        for status in [unix_status(), windows_status(Some(PWSH))] {
            let line = check_line_for(&status);
            assert!(line.contains("[OK]"), "{line}");
            assert!(check_remaining_for(&status).is_none());
        }
        assert!(check_line_for(&windows_status(Some(PWSH))).contains("起動時に読ませる"));
    }

    #[test]
    fn 器がoscを落とすなら効かないことを言う() {
        let mut status = windows_status(Some(PWSH));
        status.blocked_by_backend = Some("永続バックエンドが OSC を通さない".to_string());
        let text = state_lines(&status, Phase::Setup).join("\n");
        assert!(text.contains("[注意]"), "{text}");
        assert!(text.contains("OSC を通さない"), "{text}");
        assert!(
            check_line_for(&status).contains("[注意]"),
            "--check でも同じ注記が出る: {}",
            check_line_for(&status)
        );
    }

    /// 実行環境そのものを相手にする 1 本（macOS では注入経路・Windows では起動引数の経路）。
    /// **どちらでも「段は表示を返し、ユーザーのファイルを書かず、人へ残りを丸投げしない」**
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
        assert!(
            outcome.remaining.is_none(),
            "置くものが無いので残りは出ない: {:?}",
            outcome.remaining
        );
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
            "launch_shell",
            "legacy_blocks",
            "blocked_by_backend",
        ] {
            assert!(out.get(key).is_some(), "{key} が応答に無い: {out}");
        }
    }

    /// #2432: install はどの OS でもユーザーのファイルを書かない（変更 0 件）
    #[test]
    fn installはユーザーのファイルを書かない() {
        let out = run(Some("install")).expect("スクリプトを書き出せる");
        assert_eq!(out["action"], "install");
        assert_eq!(out["changes"], json!([]), "{out}");
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
    /// **「有効」と報告されるが「解除するものは無い」**という組み合わせを固定する
    #[cfg(unix)]
    #[test]
    fn unix_は自動配置で解除対象を持たない() {
        let out = run(None).unwrap();
        assert_eq!(out["delivery"], "automatic");
        assert_eq!(out["installed"], true);
        assert_eq!(out["legacy_blocks"], json!([]));
        assert_eq!(out["launch_shell"], json!(null));

        let err = run(Some("uninstall")).expect_err("解除する配置が無い");
        assert!(err.contains("環境変数"), "{err}");
    }

    /// Windows は起動引数で読ませるので配置対象を持たない（#2432）。検証プロセスは
    /// 本物の `$PROFILE` を見ないので、解除は「残っていない」で断る
    #[cfg(windows)]
    #[test]
    fn windows_は起動引数で届き解除対象を持たない() {
        let out = run(None).unwrap();
        assert_eq!(out["delivery"], "launch");
        assert_eq!(out["installed"], true, "{out}");
        assert!(out["launch_shell"].is_string(), "{out}");
        assert_eq!(out["legacy_blocks"], json!([]));

        let err = run(Some("uninstall")).expect_err("解除する配置が無い");
        assert!(err.contains("起動引数"), "{err}");
    }
}
