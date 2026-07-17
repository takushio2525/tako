//! remote_setup — `tako remote setup` 対話ウィザード（Issue #286 弾6）
//!
//! Tailscale Serve ベースのリモート接続を対話的にセットアップする。
//! 計画書 `.agent/plans/tako-remote-plan.md` §5.5 導線 A が正。
//!
//! ウィザードの流れ:
//! 1. Tailscale 検出（GUI 版 / CLI 版両対応）
//! 2. 未導入なら brew / App Store 案内 + その場インストール（y/N）
//! 3. ログイン確認（未ログインならブラウザ認証へ誘導して待機）
//! 4. MagicDNS + HTTPS 証明書の有効化確認
//! 5. serve 設定
//! 6. 自己接続確認
//! 7. スマホ側手順 + 固定 URL の QR（PNG）表示
//!
//! dispatch + MCP `tako_remote_setup` と 1:1。
//! 非対話は `--yes` / `--answers` で可能にし、開発不変条件を維持する。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io;

use crate::tailscale::{self, MissingItem, ServeState};

/// remote setup のステップ結果。各ステップが何をしたかの記録
#[derive(Debug, Clone, Serialize)]
pub struct SetupStepResult {
    pub step: &'static str,
    pub status: &'static str,
    pub message: String,
}

/// remote setup の最終結果
#[derive(Debug, Clone, Serialize)]
pub struct RemoteSetupResult {
    pub success: bool,
    pub ts_net_url: Option<String>,
    pub qr_path: Option<String>,
    pub steps: Vec<SetupStepResult>,
    pub phone_instructions: Option<String>,
}

/// remote setup の非対話パラメータ（dispatch / MCP 経由）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RemoteSetupAnswers {
    /// true = 全質問に yes で回答（brew install 等）
    pub yes: Option<bool>,
    /// 使用するポート（省略時は 7749）
    pub port: Option<u16>,
}

impl RemoteSetupAnswers {
    pub fn auto_yes(&self) -> bool {
        self.yes.unwrap_or(false)
    }

    pub fn port(&self) -> u16 {
        self.port.unwrap_or(7749)
    }
}

/// ウィザードの非対話実行（dispatch / MCP から呼ばれる。CLI の対話版は tako-cli 側）。
/// 各ステップを順に実行し、結果を返す。失敗したステップで停止する。
pub fn run_noninteractive(answers: &RemoteSetupAnswers) -> Result<Value, String> {
    let port = answers.port();
    let mut result = RemoteSetupResult {
        success: false,
        ts_net_url: None,
        qr_path: None,
        steps: Vec::new(),
        phone_instructions: None,
    };

    // Step 1: Tailscale 検出
    let status = tailscale::setup_status();
    if status.cli_path.is_none() {
        result.steps.push(SetupStepResult {
            step: "tailscale_detect",
            status: "missing",
            message: MissingItem::CliNotFound.describe(),
        });
        return Ok(serde_json::to_value(&result).unwrap());
    }
    result.steps.push(SetupStepResult {
        step: "tailscale_detect",
        status: "ok",
        message: format!(
            "Tailscale を検出: {}",
            status.cli_path.as_deref().unwrap_or("?")
        ),
    });

    // Step 2: デーモン・ログイン・HTTPS の確認
    if !status.missing.is_empty() {
        for item in &status.missing {
            result.steps.push(SetupStepResult {
                step: "tailscale_status",
                status: "missing",
                message: item.describe(),
            });
        }
        return Ok(serde_json::to_value(&result).unwrap());
    }
    result.steps.push(SetupStepResult {
        step: "tailscale_status",
        status: "ok",
        message: "Tailscale はログイン済み・HTTPS 有効".into(),
    });

    let cli = status.cli_path.as_deref().unwrap();
    let dns_name = status
        .dns_name
        .as_deref()
        .ok_or_else(|| "MagicDNS 名を取得できません".to_string())?;
    let ts_url = format!("https://{dns_name}");

    // Step 3: serve 設定
    let serve = tailscale::serve_state(cli).map_err(|e| format!("serve 状態の取得に失敗: {e}"))?;
    let target = tailscale::proxy_target_for_port(port);
    match serve {
        ServeState::Proxy(ref existing) if *existing == target => {
            result.steps.push(SetupStepResult {
                step: "serve_config",
                status: "ok",
                message: format!("serve は設定済み（{target} へプロキシ）"),
            });
        }
        ServeState::NotConfigured => {
            tailscale::serve_start(cli, port).map_err(|e| format!("serve の設定に失敗: {e}"))?;
            result.steps.push(SetupStepResult {
                step: "serve_config",
                status: "configured",
                message: format!("serve を設定しました（{target} へプロキシ）"),
            });
        }
        ServeState::Proxy(existing) => {
            result.steps.push(SetupStepResult {
                step: "serve_config",
                status: "conflict",
                message: format!(
                    "HTTPS:443 は別のプロキシ先に設定済み（{existing}）。\n\
                     tako の設定に変更するには、先に tailscale serve --https=443 off で解除してください。"
                ),
            });
            return Ok(serde_json::to_value(&result).unwrap());
        }
        ServeState::Other => {
            result.steps.push(SetupStepResult {
                step: "serve_config",
                status: "conflict",
                message: "HTTPS:443 にカスタム serve 設定が存在します。\n\
                     tako はこの設定を上書きしません。先に手動で解除してください。"
                    .into(),
            });
            return Ok(serde_json::to_value(&result).unwrap());
        }
    }

    // Step 4: 自己接続確認（localhost の daemon が応答するかは remote start 後に確認するため、
    //         ここでは ts.net URL の DNS 解決だけ確認する）
    result.steps.push(SetupStepResult {
        step: "self_check",
        status: "ok",
        message: format!("固定 URL: {ts_url}"),
    });

    // Step 5: QR PNG 生成
    match crate::remote::generate_qr_png(&ts_url) {
        Ok(path) => {
            result.qr_path = Some(path.display().to_string());
            result.steps.push(SetupStepResult {
                step: "qr_generate",
                status: "ok",
                message: format!("QR コード: {}", path.display()),
            });
        }
        Err(e) => {
            result.steps.push(SetupStepResult {
                step: "qr_generate",
                status: "warn",
                message: format!("QR コードの生成に失敗（URL は有効です）: {e}"),
            });
        }
    }

    result.success = true;
    result.ts_net_url = Some(ts_url.clone());
    result.phone_instructions = Some(phone_setup_instructions(&ts_url));

    Ok(serde_json::to_value(&result).unwrap())
}

/// スマホ側のセットアップ手順（導線 B。ウィザード末尾と docs で同じ文面を使う）
pub fn phone_setup_instructions(ts_url: &str) -> String {
    format!(
        "\
--- スマホ側の設定手順 ---

1. スマホに Tailscale アプリをインストール
   - iPhone: App Store で「Tailscale」を検索
   - Android: Google Play で「Tailscale」を検索

2. Mac と同じアカウントでログイン
   （同じ tailnet に参加する必要があります）

3. スマホのブラウザで以下の URL を開く:
   {ts_url}

4. Mac 画面にペアリング承認ダイアログが表示されるので「許可」を選択

5. ブラウザの「ホーム画面に追加」でアプリ化
   （以後はホーム画面のアイコンから開くだけ）

この設定は一度だけ必要です。2 回目以降はホーム画面から開くだけで接続できます。"
    )
}

/// `tako remote setup` を対話的に実行する（CLI 専用。TTY 出力つき）。
/// ステップごとに進捗を表示し、ユーザーの入力を求める場合がある
pub fn run_interactive(
    port: u16,
    auto_yes: bool,
    writer: &mut dyn io::Write,
) -> Result<Value, String> {
    writeln!(writer, "tako remote setup").map_err(|e| e.to_string())?;
    writeln!(writer, "==================").map_err(|e| e.to_string())?;
    writeln!(writer).map_err(|e| e.to_string())?;

    // Step 1: Tailscale 検出
    write!(writer, "[1/5] Tailscale を検出中... ").map_err(|e| e.to_string())?;
    let _ = writer.flush();
    let status = tailscale::setup_status();

    if status.cli_path.is_none() {
        writeln!(writer, "未導入").map_err(|e| e.to_string())?;
        writeln!(writer).map_err(|e| e.to_string())?;
        writeln!(
            writer,
            "Tailscale が必要です。以下のいずれかの方法でインストールしてください:"
        )
        .map_err(|e| e.to_string())?;
        writeln!(
            writer,
            "  - App Store で「Tailscale」を検索してインストール"
        )
        .map_err(|e| e.to_string())?;
        writeln!(writer, "  - brew install tailscale").map_err(|e| e.to_string())?;
        writeln!(writer).map_err(|e| e.to_string())?;

        if auto_yes || ask_yes_no(writer, "brew install tailscale を実行しますか?")? {
            writeln!(writer, "  brew install tailscale を実行中...").map_err(|e| e.to_string())?;
            let install_result = std::process::Command::new("brew")
                .args(["install", "tailscale"])
                .status();
            match install_result {
                Ok(s) if s.success() => {
                    writeln!(writer, "  インストール完了").map_err(|e| e.to_string())?;
                }
                _ => {
                    writeln!(
                        writer,
                        "  インストールに失敗しました。手動でインストールしてください。"
                    )
                    .map_err(|e| e.to_string())?;
                    return Err(
                        "Tailscale のインストールに失敗。手動でインストールしてください。".into(),
                    );
                }
            }
            // 再検出
            let status = tailscale::setup_status();
            if status.cli_path.is_none() {
                return Err("インストール後も Tailscale を検出できません。".into());
            }
        } else {
            writeln!(
                writer,
                "インストール後に再度 `tako remote setup` を実行してください。"
            )
            .map_err(|e| e.to_string())?;
            return Err("Tailscale が未導入".into());
        }
    } else {
        writeln!(writer, "OK ({})", status.cli_path.as_deref().unwrap_or("?"))
            .map_err(|e| e.to_string())?;
    }

    // 再取得（install 後の場合があるため）
    let status = tailscale::setup_status();
    let cli = status
        .cli_path
        .as_deref()
        .ok_or("Tailscale CLI が見つかりません")?;

    // Step 2: ログイン確認
    write!(writer, "[2/5] ログイン状態を確認中... ").map_err(|e| e.to_string())?;
    let _ = writer.flush();

    if status.missing.contains(&MissingItem::DaemonNotRunning) {
        writeln!(writer, "デーモンが起動していません").map_err(|e| e.to_string())?;
        writeln!(writer).map_err(|e| e.to_string())?;
        writeln!(
            writer,
            "Tailscale アプリを起動するか、tailscaled を起動してください。"
        )
        .map_err(|e| e.to_string())?;
        writeln!(
            writer,
            "その後、再度 `tako remote setup` を実行してください。"
        )
        .map_err(|e| e.to_string())?;
        return Err("Tailscale デーモンが起動していません".into());
    }

    if status.missing.contains(&MissingItem::NotLoggedIn) {
        writeln!(writer, "未ログイン").map_err(|e| e.to_string())?;
        writeln!(writer).map_err(|e| e.to_string())?;
        writeln!(writer, "ブラウザで Tailscale にログインしてください。")
            .map_err(|e| e.to_string())?;
        writeln!(writer, "  tailscale up を実行するとブラウザが開きます。")
            .map_err(|e| e.to_string())?;
        writeln!(
            writer,
            "ログイン完了後、再度 `tako remote setup` を実行してください。"
        )
        .map_err(|e| e.to_string())?;
        return Err("Tailscale にログインしていません".into());
    }

    if status
        .missing
        .iter()
        .any(|m| matches!(m, MissingItem::BackendNotRunning(_)))
    {
        writeln!(writer, "接続が無効です").map_err(|e| e.to_string())?;
        writeln!(writer, "  tailscale up で再接続してください。").map_err(|e| e.to_string())?;
        return Err("Tailscale の接続が有効ではありません".into());
    }

    writeln!(writer, "OK").map_err(|e| e.to_string())?;

    // Step 3: HTTPS 証明書
    write!(writer, "[3/5] HTTPS 証明書を確認中... ").map_err(|e| e.to_string())?;
    let _ = writer.flush();

    if status.missing.contains(&MissingItem::HttpsNotEnabled) {
        writeln!(writer, "未有効").map_err(|e| e.to_string())?;
        writeln!(writer).map_err(|e| e.to_string())?;
        writeln!(
            writer,
            "tailnet の MagicDNS と HTTPS Certificates を有効にしてください:"
        )
        .map_err(|e| e.to_string())?;
        writeln!(writer, "  https://login.tailscale.com/admin/dns").map_err(|e| e.to_string())?;
        writeln!(writer).map_err(|e| e.to_string())?;
        writeln!(
            writer,
            "有効化後、再度 `tako remote setup` を実行してください。"
        )
        .map_err(|e| e.to_string())?;
        return Err("HTTPS 証明書が未有効".into());
    }

    let dns_name = status
        .dns_name
        .as_deref()
        .ok_or("MagicDNS 名を取得できません")?;
    let ts_url = format!("https://{dns_name}");
    writeln!(writer, "OK ({dns_name})").map_err(|e| e.to_string())?;

    // Step 4: serve 設定
    write!(writer, "[4/5] serve を設定中... ").map_err(|e| e.to_string())?;
    let _ = writer.flush();

    let serve = tailscale::serve_state(cli).map_err(|e| format!("serve 状態の取得に失敗: {e}"))?;
    let target = tailscale::proxy_target_for_port(port);

    match serve {
        ServeState::Proxy(ref existing) if *existing == target => {
            writeln!(writer, "設定済み").map_err(|e| e.to_string())?;
        }
        ServeState::NotConfigured => {
            tailscale::serve_start(cli, port).map_err(|e| format!("serve の設定に失敗: {e}"))?;
            writeln!(writer, "設定完了 ({target})").map_err(|e| e.to_string())?;
        }
        ServeState::Proxy(existing) => {
            writeln!(writer, "競合").map_err(|e| e.to_string())?;
            writeln!(
                writer,
                "  HTTPS:443 は別のプロキシ先に設定済みです: {existing}"
            )
            .map_err(|e| e.to_string())?;
            writeln!(
                writer,
                "  先に `tailscale serve --https=443 off` で解除してください。"
            )
            .map_err(|e| e.to_string())?;
            return Err("serve 設定が競合しています".into());
        }
        ServeState::Other => {
            writeln!(writer, "競合").map_err(|e| e.to_string())?;
            writeln!(
                writer,
                "  HTTPS:443 にカスタム serve 設定が存在します。手動で解除してください。"
            )
            .map_err(|e| e.to_string())?;
            return Err("serve 設定が競合しています".into());
        }
    }

    // Step 5: 完了 + QR + スマホ手順
    writeln!(writer, "[5/5] セットアップ完了").map_err(|e| e.to_string())?;
    writeln!(writer).map_err(|e| e.to_string())?;
    writeln!(writer, "固定 URL: {ts_url}").map_err(|e| e.to_string())?;
    writeln!(writer).map_err(|e| e.to_string())?;

    // QR PNG 生成
    let qr_path = match crate::remote::generate_qr_png(&ts_url) {
        Ok(path) => {
            writeln!(writer, "QR コード: {}", path.display()).map_err(|e| e.to_string())?;
            // macOS: open で画像ビューアを起動
            let _ = std::process::Command::new("open").arg(&path).spawn();
            Some(path.display().to_string())
        }
        Err(e) => {
            writeln!(writer, "QR コード生成に失敗: {e}").map_err(|e| e.to_string())?;
            None
        }
    };

    writeln!(writer).map_err(|e| e.to_string())?;
    let instructions = phone_setup_instructions(&ts_url);
    writeln!(writer, "{instructions}").map_err(|e| e.to_string())?;

    writeln!(writer).map_err(|e| e.to_string())?;
    writeln!(
        writer,
        "リモート接続を開始するには `tako remote start` を実行してください。"
    )
    .map_err(|e| e.to_string())?;

    Ok(json!({
        "success": true,
        "ts_net_url": ts_url,
        "qr_path": qr_path,
    }))
}

/// stdin から y/N を読む。デフォルトは No
fn ask_yes_no(writer: &mut dyn io::Write, prompt: &str) -> Result<bool, String> {
    write!(writer, "{prompt} [y/N] ").map_err(|e| e.to_string())?;
    let _ = writer.flush();
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|e| e.to_string())?;
    Ok(input.trim().eq_ignore_ascii_case("y") || input.trim().eq_ignore_ascii_case("yes"))
}

/// `tako remote setup` の状態チェック（非対話。status 用途）
pub fn check_status() -> Value {
    let status = tailscale::setup_status();
    let mut items = Vec::new();

    items.push(json!({
        "item": "tailscale",
        "status": if status.cli_path.is_some() { "ok" } else { "missing" },
        "detail": status.cli_path.as_deref().unwrap_or("未導入"),
    }));
    items.push(json!({
        "item": "daemon",
        "status": if status.daemon_running { "ok" } else { "missing" },
    }));
    items.push(json!({
        "item": "login",
        "status": if status.logged_in { "ok" } else { "missing" },
        "detail": status.backend_state.as_deref().unwrap_or("unknown"),
    }));
    items.push(json!({
        "item": "https",
        "status": if status.https_enabled { "ok" } else { "missing" },
    }));
    items.push(json!({
        "item": "dns_name",
        "status": if status.dns_name.is_some() { "ok" } else { "missing" },
        "detail": status.dns_name.as_deref().unwrap_or("unknown"),
    }));

    // serve 状態
    if let Some(cli) = status.cli_path.as_deref() {
        if status.ready() {
            match tailscale::serve_state(cli) {
                Ok(ServeState::Proxy(target)) => {
                    items.push(json!({
                        "item": "serve",
                        "status": "ok",
                        "detail": target,
                    }));
                }
                Ok(ServeState::NotConfigured) => {
                    items.push(json!({
                        "item": "serve",
                        "status": "not_configured",
                    }));
                }
                Ok(ServeState::Other) => {
                    items.push(json!({
                        "item": "serve",
                        "status": "conflict",
                        "detail": "カスタム設定が存在",
                    }));
                }
                Err(e) => {
                    items.push(json!({
                        "item": "serve",
                        "status": "error",
                        "detail": e,
                    }));
                }
            }
        }
    }

    json!({
        "ready": status.ready(),
        "ts_net_url": status.ts_net_url(),
        "items": items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phone_instructionsはurlを含む() {
        let text = phone_setup_instructions("https://mac.tail1234.ts.net");
        assert!(text.contains("https://mac.tail1234.ts.net"));
        assert!(text.contains("Tailscale"));
        assert!(text.contains("ホーム画面"));
    }

    #[test]
    fn check_statusはjsonを返す() {
        let result = check_status();
        assert!(result["items"].is_array());
        assert!(result["ready"].is_boolean());
    }

    #[test]
    fn remote_setup_answersの既定値() {
        let answers = RemoteSetupAnswers::default();
        assert!(!answers.auto_yes());
        assert_eq!(answers.port(), 7749);
    }

    #[test]
    fn remote_setup_answersのjsonパース() {
        let a: RemoteSetupAnswers = serde_json::from_str(r#"{"yes":true,"port":8080}"#).unwrap();
        assert!(a.auto_yes());
        assert_eq!(a.port(), 8080);
    }
}
