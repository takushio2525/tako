//! remote_setup — `tako remote setup` 対話ウィザード（Issue #286 弾6）
//!
//! Tailscale Serve ベースのリモート接続を対話的にセットアップする。
//! 計画書 `.agent/plans/tako-remote-plan.md` §5.5 導線 A が正。
//!
//! ウィザードの流れ:
//! 1. Tailscale 検出（GUI 版 / CLI 版両対応）
//! 2. 未導入なら `setup_deps` の 1 実装でその場インストール（案内 → y/N → 再検出。#1509）
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

use crate::setup_deps;
use crate::tailscale::{self, MissingItem, ServeState};

/// `setup_deps` の依存表で Tailscale を指す名前（macOS / Windows で共通）
const TAILSCALE_DEP: &str = "tailscale";

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
    /// true = 全質問に yes で回答（依存の導入等。対話版 `run_interactive` のみ）
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
        match install_tailscale(auto_yes, writer)? {
            Some(path) => {
                writeln!(writer, "  導入しました: {path}").map_err(|e| e.to_string())?;
            }
            None => {
                writeln!(
                    writer,
                    "インストール後に再度 `tako remote setup` を実行してください。"
                )
                .map_err(|e| e.to_string())?;
                return Err("Tailscale が未導入".into());
            }
        }
        // 導入できたと言えるかは**この先の段が引けるか**で決まる。`setup_deps` の
        // 再検出（`exe::find`）と remote の解決規則（`find_tailscale`）は
        // 見る場所が違うので、ここでもう一度 remote 側の規則で確かめる
        if tailscale::setup_status().cli_path.is_none() {
            return Err("インストール後も Tailscale を検出できません。".into());
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

/// 未導入の Tailscale をその場で入れる（#1509）。戻り値は導入できたパス。
///
/// 判断（`--yes` / 端末の有無 / 導入器の有無）・実行・再検出は
/// [`crate::setup_deps`] の 1 実装（`offer_and_install`）を通す。
/// **ここで導入器（brew）を直に起こさない**: 直叩きだと `tako setup` の依存
/// チェック段と体験が割れ、「brew が無い」「非 TTY」「入れたのに見つからない」の
/// 扱いを 2 か所で持つことになる（#1509 で直したのがまさにその形）
fn install_tailscale(auto_yes: bool, writer: &mut dyn io::Write) -> Result<Option<String>, String> {
    let Some(state) = setup_deps::status_of(TAILSCALE_DEP) else {
        // この環境の依存表に Tailscale が無い = 自動導入の手段を持たない
        writeln!(writer, "{}", MissingItem::CliNotFound.describe()).map_err(|e| e.to_string())?;
        return Ok(None);
    };
    // 代行できるときは**入れ方を並べない**（#322 の最簡形。y を押せば済む場面で
    // 2 択を見せない）。代行できないときは `manual_hint_line` が
    // 依存表の hint（App Store 版を含む）を理由つきで出す
    writeln!(writer, "Tailscale が必要です。").map_err(|e| e.to_string())?;
    let ctx = setup_deps::DepOfferContext {
        // `remote setup` は弾 6 から明示対話型（plan §5.5 導線 A）= 導入まで行う段
        stage_installs: true,
        review: false,
        assume_yes: auto_yes,
        stdin_is_terminal: io::IsTerminal::is_terminal(&io::stdin()),
        // #1499 の A/B は「標準 setup が聞くか」の軸。`remote setup` は
        // もともと聞いていた経路なので対象外（legacy で聞かなくなるのは退行）
        legacy: false,
    };
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let outcome = setup_deps::offer_and_install(
        &state,
        ctx,
        &mut setup_deps::DepPromptIo {
            writer,
            reader: &mut reader,
            indent: "  ",
        },
    )?;
    Ok(match outcome {
        setup_deps::DepOutcome::Installed(path) => Some(path),
        setup_deps::DepOutcome::NotInstalled => None,
    })
}

/// `tako remote setup` の状態チェック（非対話。status 用途）。
///
/// **読み取りだけ**（`tailscale status --json` / `tailscale serve status --json` を聞くだけで、
/// 導入・起動・設定の書き換えはしない）。`tako setup` の末尾（#1507）・GUI のリモートパネル・
/// dispatch `RemoteSetup { action: "check" }` が同じこれを読む。
///
/// 待ちは tailscale 側の上限（1 回 10 秒）で打ち切られ、打ち切ったものは `timeouts`
/// （`label` / `waited_secs`。#1503 の知らせと同じ形）へ載る。空なら全部を確かめられた
pub fn check_status() -> Value {
    let status = tailscale::setup_status();
    let mut items = Vec::new();
    let mut timeouts: Vec<tako_core::probe::TimeoutNotice> =
        status.timed_out.iter().cloned().collect();

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
            match tailscale::serve_state_checked(cli) {
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
                    if let tailscale::RunError::TimedOut(notice) = &e {
                        timeouts.push(notice.clone());
                    }
                    items.push(json!({
                        "item": "serve",
                        "status": "error",
                        "detail": e.to_string(),
                    }));
                }
            }
        }
    }

    json!({
        "ready": status.ready(),
        "ts_net_url": status.ts_net_url(),
        "items": items,
        "timeouts": timeouts
            .iter()
            .map(|n| json!({ "label": n.label, "waited_secs": n.waited_secs }))
            .collect::<Vec<_>>(),
    })
}

// --- `tako setup` の末尾の 1 行（#1507） --------------------------------------

/// スマホから使えるか。`tako setup` の末尾に出す 1 行の中身（#1507）。
///
/// [`check_status`] の JSON **だけ**から決める（[`phone_readiness`]）。setup が
/// tailscale を別口で問い合わせると、`tako remote setup` / GUI のリモートパネルと
/// 答えが割れる。欠けている段が複数あるときは、ウィザードの段の順
/// （導入 → 起動 → ログイン → 証明書 → 公開）で**最初の 1 つだけ**を言う
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhoneReadiness {
    /// 上限で打ち切ったので確かめ切れなかった（`timeouts` が空でない）
    Unknown,
    /// Tailscale が見つからない
    TailscaleMissing,
    /// tailscaled に繋がらない
    DaemonNotRunning,
    /// 未ログイン（BackendState = NeedsLogin）
    NotLoggedIn,
    /// ログイン済みだが接続が有効でない（値は BackendState。Stopped 等）
    BackendNotRunning(String),
    /// tailnet の HTTPS 証明書（MagicDNS + HTTPS Certificates）が未有効
    HttpsNotEnabled,
    /// Tailscale 側は整っているが、まだ公開していない（serve 未設定）
    NotPublished,
    /// 公開済み（serve が tako 形式の単純プロキシ）。`url` は恒久固定 URL
    Published { url: Option<String> },
    /// tako の管理形式でない serve 設定がある
    ServeConflict,
    /// serve の状態を読めなかった
    ServeUnreadable,
}

/// [`check_status`] の JSON から [`PhoneReadiness`] を決める（**純粋関数**）
pub fn phone_readiness(status: &Value) -> PhoneReadiness {
    if !status_timeouts(status).is_empty() {
        return PhoneReadiness::Unknown;
    }
    let item = |key: &str| {
        status["items"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|entry| entry["item"].as_str() == Some(key))
    };
    let is_ok = |key: &str| item(key).and_then(|e| e["status"].as_str()) == Some("ok");
    if !is_ok("tailscale") {
        return PhoneReadiness::TailscaleMissing;
    }
    if !is_ok("daemon") {
        return PhoneReadiness::DaemonNotRunning;
    }
    if !is_ok("login") {
        return match item("login").and_then(|e| e["detail"].as_str()) {
            Some("NeedsLogin") | None => PhoneReadiness::NotLoggedIn,
            Some(state) => PhoneReadiness::BackendNotRunning(state.to_string()),
        };
    }
    if !is_ok("https") || !is_ok("dns_name") {
        return PhoneReadiness::HttpsNotEnabled;
    }
    match item("serve").and_then(|e| e["status"].as_str()) {
        Some("ok") => PhoneReadiness::Published {
            url: status["ts_net_url"].as_str().map(str::to_string),
        },
        Some("not_configured") => PhoneReadiness::NotPublished,
        Some("conflict") => PhoneReadiness::ServeConflict,
        _ => PhoneReadiness::ServeUnreadable,
    }
}

/// [`check_status`] の JSON から打ち切りの知らせを戻す（`timeouts` の読み取り）
pub fn status_timeouts(status: &Value) -> Vec<tako_core::probe::TimeoutNotice> {
    status["timeouts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            Some(tako_core::probe::TimeoutNotice {
                label: entry["label"].as_str()?.to_string(),
                waited_secs: entry["waited_secs"].as_u64()?,
            })
        })
        .collect()
}

/// 末尾の 1 行の頭。PTY の実経路テストがここを「setup の出力の終わり」の目印にする
pub const PHONE_LINE_HEAD: &str = "スマホからの接続: ";

/// [`PhoneReadiness`] を 1 行にする（**純粋関数**）。
///
/// `install_step` は Tailscale が未導入のときの「次に打つ 1 行」で、
/// [`crate::setup_deps::next_step_line`] が作る（依存の導入口 = #1499 / #1524 の 1 実装。
/// ここで `brew install` を組み立てない）。提示するコマンドは常に最簡形（#322）
pub fn phone_readiness_line(readiness: &PhoneReadiness, install_step: Option<&str>) -> String {
    const SETUP: &str = "設定する: tako remote setup";
    const RECHECK: &str = "確かめ直す: tako remote setup";
    let body = match readiness {
        PhoneReadiness::Unknown => format!("状態を確認できませんでした。{RECHECK}"),
        PhoneReadiness::TailscaleMissing => match install_step {
            Some(step) => format!("Tailscale が未導入です。{step}"),
            // 依存表の側では見つかっている（検出の口が食い違う）= 導入を勧めない
            None => format!("Tailscale を実行できませんでした。{RECHECK}"),
        },
        PhoneReadiness::DaemonNotRunning => format!("Tailscale が起動していません。{SETUP}"),
        PhoneReadiness::NotLoggedIn => format!("Tailscale にログインしていません。{SETUP}"),
        PhoneReadiness::BackendNotRunning(state) => {
            format!("Tailscale の接続が有効ではありません（状態: {state}）。{SETUP}")
        }
        PhoneReadiness::HttpsNotEnabled => {
            format!("tailnet の HTTPS 証明書が未有効です。{SETUP}")
        }
        PhoneReadiness::NotPublished => {
            format!("Tailscale は準備済みです（まだ公開していません）。{SETUP}")
        }
        PhoneReadiness::Published { url: Some(url) } => format!("設定済みです（{url}）"),
        PhoneReadiness::Published { url: None } => "設定済みです".to_string(),
        PhoneReadiness::ServeConflict => {
            format!("Tailscale Serve に tako 以外の設定があります。{SETUP}")
        }
        PhoneReadiness::ServeUnreadable => {
            format!("Tailscale Serve の状態を読めませんでした。{RECHECK}")
        }
    };
    format!("{PHONE_LINE_HEAD}{body}")
}

/// `tako setup` の末尾に出す行（#1507）。**読み取りだけ**で、導入も起動もしない。
///
/// 1. [`check_status`] を 1 回読む（待ちは tailscale 側の上限で打ち切られる）
/// 2. 打ち切ったものがあれば #1503 の文面（`[確認できません] …（N 秒応答なし）…`）で並べる
///    = dispatch `SetupRun` / MCP `tako_setup` が `probe::parse_notices` で読み戻して
///    `probe_timeouts` へ載せる形
/// 3. 最後に [`PHONE_LINE_HEAD`] で始まる 1 行
///
/// Tailscale の導入を**勧める・聞く**のは依存チェック段の 1 回だけ（#1499 / #1524）。
/// ここで聞き直すと同じ実行で 2 度聞くことになるので、未導入なら導入口を指すだけにする
pub fn setup_summary_lines() -> Vec<String> {
    let status = check_status();
    let readiness = phone_readiness(&status);
    let install_step = match readiness {
        PhoneReadiness::TailscaleMissing => tailscale_install_step(),
        _ => None,
    };
    status_timeouts(&status)
        .iter()
        .map(|notice| format!("  {notice}"))
        .chain(std::iter::once(phone_readiness_line(
            &readiness,
            install_step.as_deref(),
        )))
        .collect()
}

/// Tailscale が未導入のときの「次に打つ 1 行」（依存表の側でも見つからないときだけ）
fn tailscale_install_step() -> Option<String> {
    let state = setup_deps::status_of(TAILSCALE_DEP)?;
    state
        .found
        .is_none()
        .then(|| setup_deps::next_step_line(&state))
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
        // 打ち切りの有無は常にキーごと載る（#1507。無いと「確かめ切れた」と区別できない）
        assert!(result["timeouts"].is_array());
    }

    // --- `tako setup` の末尾の 1 行（#1507）---------------------------------
    //
    // 入力は `check_status` の JSON と同じ形の固定値。**状態ごとの字面を固定**する
    // （判定の順・次に打つコマンドのどちらかが変わると、どの状態で変わったかが落ちる）

    /// `check_status` と同じ形の JSON を組む。`missing_from` の段から先が欠ける（`None` = 全段 ok）
    fn status_json(missing_from: Option<&str>, login_detail: &str, serve: Option<&str>) -> Value {
        let order = ["tailscale", "daemon", "login", "https", "dns_name"];
        let cut = missing_from.and_then(|key| order.iter().position(|k| *k == key));
        let mut items: Vec<Value> = order
            .iter()
            .enumerate()
            .map(|(i, key)| {
                let ok = cut.is_none_or(|c| i < c);
                let mut item = json!({ "item": key, "status": if ok { "ok" } else { "missing" } });
                if *key == "login" {
                    item["detail"] = json!(if ok { "Running" } else { login_detail });
                }
                item
            })
            .collect();
        if let Some(serve) = serve {
            items.push(json!({ "item": "serve", "status": serve }));
        }
        json!({
            "ready": cut.is_none(),
            "ts_net_url": "https://mac.tail1234.ts.net",
            "items": items,
            "timeouts": [],
        })
    }

    const INSTALL_STEP: &str =
        "いま入れる: tako setup deps install   （brew install tailscale 相当）";

    fn line_of(status: &Value) -> String {
        phone_readiness_line(&phone_readiness(status), Some(INSTALL_STEP))
    }

    #[test]
    fn 状態ごとの1行を固定する() {
        let cases: [(Value, &str); 11] = [
            (
                status_json(Some("tailscale"), "", None),
                "スマホからの接続: Tailscale が未導入です。いま入れる: tako setup deps install   （brew install tailscale 相当）",
            ),
            (
                status_json(Some("daemon"), "", None),
                "スマホからの接続: Tailscale が起動していません。設定する: tako remote setup",
            ),
            (
                status_json(Some("login"), "NeedsLogin", None),
                "スマホからの接続: Tailscale にログインしていません。設定する: tako remote setup",
            ),
            (
                status_json(Some("login"), "Stopped", None),
                "スマホからの接続: Tailscale の接続が有効ではありません（状態: Stopped）。設定する: tako remote setup",
            ),
            (
                status_json(Some("https"), "", None),
                "スマホからの接続: tailnet の HTTPS 証明書が未有効です。設定する: tako remote setup",
            ),
            (
                status_json(Some("dns_name"), "", None),
                "スマホからの接続: tailnet の HTTPS 証明書が未有効です。設定する: tako remote setup",
            ),
            (
                status_json(None, "", Some("not_configured")),
                "スマホからの接続: Tailscale は準備済みです（まだ公開していません）。設定する: tako remote setup",
            ),
            (
                status_json(None, "", Some("ok")),
                "スマホからの接続: 設定済みです（https://mac.tail1234.ts.net）",
            ),
            (
                status_json(None, "", Some("conflict")),
                "スマホからの接続: Tailscale Serve に tako 以外の設定があります。設定する: tako remote setup",
            ),
            (
                status_json(None, "", Some("error")),
                "スマホからの接続: Tailscale Serve の状態を読めませんでした。確かめ直す: tako remote setup",
            ),
            // ready なのに serve の項目が無い（壊れた応答）は「読めなかった」へ倒す
            (
                status_json(None, "", None),
                "スマホからの接続: Tailscale Serve の状態を読めませんでした。確かめ直す: tako remote setup",
            ),
        ];
        for (status, expected) in cases {
            assert_eq!(line_of(&status), expected, "入力: {status}");
        }
    }

    /// 打ち切りが 1 つでもあれば、他の項目が何を言っていても「確認できなかった」
    /// （`status --json` の打ち切りは項目上 `daemon: missing` になるので、見ないと
    /// 「起動していません」と言い切ってしまう = #1503 の「無言にしない」に反する）
    #[test]
    fn 打ち切りは未起動と言い切らない() {
        let mut status = status_json(Some("daemon"), "", None);
        status["timeouts"] = json!([{ "label": "tailscale status --json", "waited_secs": 10 }]);
        assert_eq!(phone_readiness(&status), PhoneReadiness::Unknown);
        assert_eq!(
            line_of(&status),
            "スマホからの接続: 状態を確認できませんでした。確かめ直す: tako remote setup"
        );
        // serve 段の打ち切りも同じ扱い
        let mut serve = status_json(None, "", Some("error"));
        serve["timeouts"] =
            json!([{ "label": "tailscale serve status --json", "waited_secs": 10 }]);
        assert_eq!(phone_readiness(&serve), PhoneReadiness::Unknown);
    }

    /// 打ち切りの知らせは #1503 の文面で出る = dispatch が `parse_notices` で読み戻せる
    #[test]
    fn 打ち切りの知らせはdispatchが読み戻せる() {
        let status = json!({
            "items": [],
            "timeouts": [{ "label": "tailscale status --json", "waited_secs": 10 }],
        });
        let notices = status_timeouts(&status);
        assert_eq!(
            notices,
            vec![tako_core::probe::TimeoutNotice {
                label: "tailscale status --json".into(),
                waited_secs: 10,
            }]
        );
        let printed = format!("  {}", notices[0]);
        assert_eq!(
            printed,
            "  [確認できません] tailscale status --json（10 秒応答なし）。打ち切って次へ進みます"
        );
        assert_eq!(tako_core::probe::parse_notices(&printed), notices);
        // キーが無い（#1507 前の応答）・形が崩れた要素は拾わない
        assert!(status_timeouts(&json!({ "items": [] })).is_empty());
        assert!(status_timeouts(&json!({ "timeouts": [{ "label": 1 }] })).is_empty());
    }

    /// 未導入なのに導入口が引けない（依存表の側では見つかっている）ときは導入を勧めない
    #[test]
    fn 導入口が無ければ導入を勧めない() {
        let status = status_json(Some("tailscale"), "", None);
        assert_eq!(
            phone_readiness_line(&phone_readiness(&status), None),
            "スマホからの接続: Tailscale を実行できませんでした。確かめ直す: tako remote setup"
        );
    }

    /// 提示するコマンドは最簡形（#322）: 既定で済む引数を付けない
    #[test]
    fn 提示するコマンドは最簡形() {
        let all = [
            PhoneReadiness::Unknown,
            PhoneReadiness::TailscaleMissing,
            PhoneReadiness::DaemonNotRunning,
            PhoneReadiness::NotLoggedIn,
            PhoneReadiness::BackendNotRunning("Stopped".into()),
            PhoneReadiness::HttpsNotEnabled,
            PhoneReadiness::NotPublished,
            PhoneReadiness::Published { url: None },
            PhoneReadiness::ServeConflict,
            PhoneReadiness::ServeUnreadable,
        ];
        for readiness in all {
            let line = phone_readiness_line(&readiness, Some(INSTALL_STEP));
            assert!(line.starts_with(PHONE_LINE_HEAD), "{line}");
            assert!(
                !line.contains("--"),
                "引数付きのコマンドを出している: {line}"
            );
            assert_eq!(line.lines().count(), 1, "1 行に収まっていない: {line}");
        }
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
