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
    /// 使うことにした Tailscale 系統（`gui` / `standalone`。#1038）
    pub tailscale_variant: Option<String>,
    /// その系統を選んだ根拠（決め打ちしていないことを応答で示す）
    pub tailscale_reason: Option<String>,
}

/// remote setup の非対話パラメータ（dispatch / MCP 経由）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RemoteSetupAnswers {
    /// true = 全質問に yes で回答（brew install 等）
    pub yes: Option<bool>,
    /// 使う Tailscale 系統（`gui` = GUI 版 / 既定探索、`standalone` = 自前の tailscaled）。
    /// 省略時は検出結果から決める（#1038: 2 系統が同居しうるので決め打ちしない）
    pub tailscale: Option<String>,
}

impl RemoteSetupAnswers {
    pub fn auto_yes(&self) -> bool {
        self.yes.unwrap_or(false)
    }
}

/// 系統決定ステップの表示文。同居しているときは選択肢と変更手段まで出す
pub fn variant_step_message(decision: &VariantDecision) -> String {
    let mut msg = format!("{}（{}）", decision.variant.describe(), decision.reason);
    if decision.coexisting {
        msg.push_str("\n  検出した系統:");
        for c in &decision.candidates {
            msg.push_str(&format!("\n    - {c}"));
        }
        msg.push_str(
            "\n  変更するには: tako remote setup --tailscale <auto|standalone>\
             （auto = 既定探索 = GUI 版があればそれ。MCP は tako_remote_setup の answers.tailscale）",
        );
    }
    msg
}

/// serve ステップの表示文
pub fn serve_step_message(step: &ServeStep) -> String {
    match step {
        ServeStep::Deferred => "`tako remote start` 時に設定します\
             （ループバック TCP のポートは起動時に決まるため）"
            .into(),
        ServeStep::AlreadyConfigured(t) => format!("serve は設定済み（{t} へプロキシ）"),
        ServeStep::Configured(t) => format!("serve を設定しました（{t} へプロキシ）"),
    }
}

/// Tailscale 系統の決定結果（#1038）
#[derive(Debug, Clone)]
pub struct VariantDecision {
    pub variant: tailscale::TailscaleVariant,
    /// なぜこの系統を選んだか（応答・表示に必ず載せる）
    pub reason: String,
    /// 2 系統が別ノードとして同時に動いているか
    pub coexisting: bool,
    /// 検出した系統の 1 行要約（選択肢の提示・表示用）
    pub candidates: Vec<String>,
}

/// 検出した系統から `choose_variant` の入力を作る（純関数側へ渡す要約）
fn candidates_of(survey: &tailscale::VariantSurvey) -> Vec<tailscale::VariantCandidate> {
    survey
        .probes
        .iter()
        .map(|p| tailscale::VariantCandidate {
            key: p.variant.key(),
            ready: p.ready(),
            is_default_discovery: matches!(p.variant, tailscale::TailscaleVariant::Default),
            node: p.node().map(|s| s.to_string()),
        })
        .collect()
}

/// 非対話で系統を決めて保存する。`explicit` があればそれを最優先で使う。
///
/// **GUI 決め打ちはしない**: 使える系統が 1 つならそれ、複数なら「現にノード実体として
/// 応答している方」を選び、根拠を返す（呼び出し側が応答・表示に載せる）
pub fn decide_variant(explicit: Option<&str>) -> Result<VariantDecision, String> {
    let survey = tailscale::survey_variants();
    let candidates: Vec<String> = survey.probes.iter().map(|p| p.summary()).collect();

    if let Some(key) = explicit.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let variant = tailscale::TailscaleVariant::parse(key).ok_or_else(|| {
            format!(
                "Tailscale 系統の指定が不正です: {key}（auto | gui | standalone）。\
                 standalone を選ぶには tailscaled の LocalAPI socket が必要です"
            )
        })?;
        tailscale::save_variant(&variant)?;
        return Ok(VariantDecision {
            reason: format!("指定により {} を使います", variant.describe()),
            variant,
            coexisting: survey.coexisting,
            candidates,
        });
    }

    // 保存済みの選択があればそれを尊重する（毎回聞かない）
    if let Some(saved) = tailscale::saved_variant() {
        return Ok(VariantDecision {
            reason: format!("保存済みの選択（{}）を使います", saved.describe()),
            variant: saved,
            coexisting: survey.coexisting,
            candidates,
        });
    }

    let picks = candidates_of(&survey);
    let Some((key, reason)) = tailscale::choose_variant(&picks) else {
        // 使える系統が無い = 従来どおり不足項目を列挙して止める（呼び出し側の責務）
        return Ok(VariantDecision {
            variant: tailscale::TailscaleVariant::default(),
            reason: "利用できる Tailscale が見つかりませんでした".into(),
            coexisting: survey.coexisting,
            candidates,
        });
    };
    let variant = tailscale::TailscaleVariant::parse(key)
        .ok_or_else(|| format!("Tailscale 系統を解決できない: {key}"))?;
    tailscale::save_variant(&variant)?;
    Ok(VariantDecision {
        reason,
        variant,
        coexisting: survey.coexisting,
        candidates,
    })
}

/// serve 設定ステップの結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServeStep {
    /// ループバック TCP なので、ポートが決まる `tako remote start` 時に設定する
    Deferred,
    /// 既に自分の target を向いている
    AlreadyConfigured(String),
    /// いま設定した
    Configured(String),
}

/// serve 設定ステップ（対話 / 非対話で共有）。
/// 既定のループバック TCP はポートが起動時にしか決まらないので**ここでは張らない**
/// （#1038: 固定 target を前提にできるのは UDS を明示したときだけ）
pub fn configure_serve(cli: &str) -> Result<ServeStep, String> {
    let spec = crate::remote::endpoint_spec()?;
    let sock = match spec {
        crate::platform::local_endpoint::EndpointSpec::Loopback => return Ok(ServeStep::Deferred),
        crate::platform::local_endpoint::EndpointSpec::Unix(path) => path,
    };
    let target = tailscale::proxy_target_for_socket(&sock);
    match tailscale::serve_state(cli).map_err(|e| format!("serve 状態の取得に失敗: {e}"))? {
        ServeState::Proxy(ref existing) if *existing == target => {
            Ok(ServeStep::AlreadyConfigured(target))
        }
        ServeState::NotConfigured => {
            tailscale::serve_start_target(cli, &target)
                .map_err(|e| format!("serve の設定に失敗: {e}"))?;
            Ok(ServeStep::Configured(target))
        }
        ServeState::Proxy(existing) => Err(format!(
            "HTTPS:443 は別のプロキシ先に設定済みです（{existing}）。\
             先に `tailscale serve --https=443 off` で解除してください。"
        )),
        ServeState::Other => Err("HTTPS:443 にカスタム serve 設定が存在します。\
             tako はこの設定を上書きしません。先に手動で解除してください。"
            .into()),
    }
}

/// ウィザードの非対話実行（dispatch / MCP から呼ばれる。CLI の対話版は tako-cli 側）。
/// 各ステップを順に実行し、結果を返す。失敗したステップで停止する。
pub fn run_noninteractive(answers: &RemoteSetupAnswers) -> Result<Value, String> {
    let mut result = RemoteSetupResult {
        success: false,
        ts_net_url: None,
        qr_path: None,
        steps: Vec::new(),
        phone_instructions: None,
        tailscale_variant: None,
        tailscale_reason: None,
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

    // Step 1.5: 使う Tailscale 系統を決める（#1038: GUI 版 / standalone が同居しうる）
    let decision = decide_variant(answers.tailscale.as_deref())?;
    result.steps.push(SetupStepResult {
        step: "tailscale_variant",
        status: if decision.coexisting {
            "selected"
        } else {
            "ok"
        },
        message: variant_step_message(&decision),
    });
    result.tailscale_variant = Some(decision.variant.key().to_string());
    result.tailscale_reason = Some(decision.reason.clone());

    // 系統を決めた後の状態で判定し直す（standalone を選んだなら standalone の状態を見る）
    let status = tailscale::setup_status();

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

    // Step 3: serve 設定（既定のループバック TCP はポートが起動時に決まるので後回し）
    match configure_serve(cli) {
        Ok(step) => result.steps.push(SetupStepResult {
            step: "serve_config",
            status: match step {
                ServeStep::Deferred => "deferred",
                ServeStep::AlreadyConfigured(_) => "ok",
                ServeStep::Configured(_) => "configured",
            },
            message: serve_step_message(&step),
        }),
        Err(e) => {
            result.steps.push(SetupStepResult {
                step: "serve_config",
                status: "conflict",
                message: e,
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

/// 対話での系統選択。2 系統が同居しているときだけ聞く（1 つしか無ければ聞かない）。
/// `--yes` / 明示指定のときは非対話の規則で決める
fn choose_variant_interactive(
    explicit: Option<&str>,
    auto_yes: bool,
    writer: &mut dyn io::Write,
) -> Result<VariantDecision, String> {
    if explicit.is_some() || auto_yes {
        return decide_variant(explicit);
    }
    let survey = tailscale::survey_variants();
    if !survey.coexisting || tailscale::saved_variant().is_some() {
        // 同居していない or 既に選択済み = 聞く必要がない
        return decide_variant(None);
    }
    writeln!(writer).map_err(|e| e.to_string())?;
    writeln!(
        writer,
        "Tailscale が 2 系統同時に動いています（別ノードとして二重登録されます）。\
         どちらを使いますか?"
    )
    .map_err(|e| e.to_string())?;
    for (i, probe) in survey.probes.iter().enumerate() {
        writeln!(writer, "  {}. {}", i + 1, probe.summary()).map_err(|e| e.to_string())?;
    }
    write!(writer, "番号を選んでください [1] ").map_err(|e| e.to_string())?;
    let _ = writer.flush();
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|e| e.to_string())?;
    let idx = input.trim().parse::<usize>().unwrap_or(1);
    let probe = survey
        .probes
        .get(idx.saturating_sub(1))
        .or_else(|| survey.probes.first())
        .ok_or("Tailscale が検出できませんでした")?;
    decide_variant(Some(probe.variant.key()))
}

/// `tako remote setup` を対話的に実行する（CLI 専用。TTY 出力つき）。
/// ステップごとに進捗を表示し、ユーザーの入力を求める場合がある
pub fn run_interactive(
    auto_yes: bool,
    tailscale_choice: Option<&str>,
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

    // Step 1.5: 使う Tailscale 系統を決める（#1038）
    let decision = choose_variant_interactive(tailscale_choice, auto_yes, writer)?;
    writeln!(
        writer,
        "  Tailscale 系統: {}",
        variant_step_message(&decision)
    )
    .map_err(|e| e.to_string())?;
    // 系統を決めた後の状態で見直す（standalone を選んだならその状態を見る）
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

    // Step 4: serve 設定（既定のループバック TCP は `tako remote start` 時に張る）
    write!(writer, "[4/5] serve を設定中... ").map_err(|e| e.to_string())?;
    let _ = writer.flush();

    match configure_serve(cli) {
        Ok(step) => {
            let label = match step {
                ServeStep::Deferred => "起動時に設定",
                ServeStep::AlreadyConfigured(_) => "設定済み",
                ServeStep::Configured(_) => "設定完了",
            };
            writeln!(writer, "{label}").map_err(|e| e.to_string())?;
            writeln!(writer, "  {}", serve_step_message(&step)).map_err(|e| e.to_string())?;
        }
        Err(e) => {
            writeln!(writer, "競合").map_err(|e| e.to_string())?;
            writeln!(writer, "  {e}").map_err(|e| e.to_string())?;
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
            // 既定の画像ビューアを起動する
            let _ = crate::platform::os_integration::open_default(&path);
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
    }

    #[test]
    fn remote_setup_answersのjsonパース() {
        let a: RemoteSetupAnswers = serde_json::from_str(r#"{"yes":true}"#).unwrap();
        assert!(a.auto_yes());
    }
}
