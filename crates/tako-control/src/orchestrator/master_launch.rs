//! master 起動の「組み立てだけ」を切り出した層（Issue #1078 / エピック #1059 柱 1-D）
//!
//! ## なぜ切り出すのか
//!
//! `tako master` は **CLI 専用**（`tako-cli` の `CLI_ONLY`）で、呼び出し元ペインの
//! シェルへコマンドを流す形をとる。スマホ（リモート daemon）からは呼び出し元ペインが
//! 無いので同じ入口を使えない。一方で**組み立て**（プロファイルの検証 → system prompt →
//! 起動コマンド → role の語彙）はまったく同じでなければならない
//! （食い違うと「スマホから立てた master だけモデルが違う / prompt が付かない」になる。
//! #761 で実際に起きた事故がこれ）。
//!
//! そこで **組み立て = ここ / 流し込み = 呼び出し側**に分けた:
//!
//! | 呼び出し側 | ペインの用意 | 流し込み |
//! |---|---|---|
//! | `tako master`（CLI） | 呼び出し元ペイン or 新タブ | `Request::Send` |
//! | リモート daemon（#1078） | `Request::TabNew` で作ったタブの初期ペイン | `Request::Send` |
//! | 引き継ぎ（#193 / #917） | 退役 master のペインを分割 | `queue_command_flow` |
//!
//! ## role の語彙は 2 つある（#761）
//!
//! ペインに貼る表示用（`orchestrator-master[:<名前>]`）と、起動コマンドが注入する
//! `TAKO_ORCHESTRATOR_ROLE`（`master[:<名前>]`）。正本は `tako_core::handoff` の
//! `master_pane_role` / `master_role_env` で、ここもそれを通す
//! （CLI の `-<名前>` 起動と同じ形になる = `master_pane_role` の doc と #761 の記録が根拠）。

use std::path::PathBuf;

use crate::orchestrator::{self, Profile};

/// master を立てるための組み立て結果。**ペインを作る前に全部揃う**
/// （組み立てで失敗したときに空のタブ / ペインを残さないため）
#[derive(Debug, Clone)]
pub struct MasterLaunchPlan {
    /// プロファイル名（`default` を含む）
    pub profile_name: String,
    /// タブ名（`master` / `master-<名前>`）
    pub tab_title: String,
    /// ペインに貼る表示用 role（`orchestrator-master[:<名前>]`）
    pub pane_role: String,
    /// 起動コマンドへ注入される role（`master[:<名前>]`）
    pub role_env: String,
    /// ペインのシェルへ流すコマンド 1 行
    pub command: String,
    /// プロファイルが指定する起動フォルダ（未指定なら `None`）
    pub cwd: Option<PathBuf>,
    /// エージェント系統（claude / codex）
    pub agent: orchestrator::WorkerAgent,
    /// 表示用のモデル名（CLI 既定に任せる場合もラベルになる）
    pub model_label: String,
    /// effort
    pub effort: String,
    /// プロファイルが Remote Control を opt-in しているか（#1068。既定 false）
    pub remote_control_opt_in: bool,
    /// Remote Control（#1068）の決定。**opt-in していても環境が不適格ならフラグは付かない**
    pub remote_control: crate::claude_remote::RemoteControlDecision,
}

impl MasterLaunchPlan {
    /// 応答 JSON（`POST /api/tabs/:id/master` と将来の 1:1 経路が共有する形）。
    /// **コマンド本文は入れない**（起動コマンドには env の並びが載るので、
    /// 応答をそのまま画面へ出す経路で見せる情報ではない）
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "profile": self.profile_name,
            "tab_title": self.tab_title,
            "role": self.pane_role,
            "agent": self.agent.as_str(),
            "model": self.model_label,
            "effort": self.effort,
            "cwd": self.cwd.as_ref().map(|p| p.display().to_string()),
            "remote_control": self.remote_control_json(),
        })
    }

    /// Remote Control の見通し（#1078 の受け入れ条件 ②「opt-in していないプロファイルでは
    /// 公式リンクが出ず理由が出る」を**起動した時点で**返せるようにする）。
    ///
    /// 文言は #1077 の 1 実装（`ProfileHint` / `notes_for_kind`）を通すので、
    /// 一覧のカードに出る理由と同じ文になる
    pub fn remote_control_json(&self) -> serde_json::Value {
        let hint = crate::claude_remote_link::ProfileHint::Master(Some(&self.profile_name));
        let (state, guidance) = match (&self.remote_control.flag, &self.remote_control.blocked) {
            // フラグが付いた = 起動後に繋がる見込み。実際に繋がったかは
            // `remote_link.state` を見る（ここで connected と言わない）
            (Some(_), _) => ("enabled", None),
            (None, Some(blocked)) => (
                "ineligible",
                crate::claude_remote_link::RemoteLink::ineligible(blocked.kind(), None)
                    .guidance(hint),
            ),
            (None, None) => (
                "off",
                crate::claude_remote_link::RemoteLink::not_connected(None).guidance(hint),
            ),
        };
        serde_json::json!({
            "state": state,
            "opt_in": self.remote_control_opt_in,
            "reason": guidance.as_ref().map(|g| g.reason.clone()),
            "next_step": guidance.as_ref().map(|g| g.next_step.clone()),
            "enable_command": guidance.as_ref().and_then(|g| g.enable_command.clone()),
        })
    }
}

/// プロファイル名から master 起動の組み立てを行う。
///
/// **CLI の `tako master -<名前>` と同じ検証を同じ順で通す**:
/// プロファイル読み込み → env 検証（#500） → cwd 解決（#500 Part 5） →
/// projects 検証（#500 Part 7） → 系統解決 → **CLI の実在検査**（#983。
/// ペインを作る前に落とす） → system prompt の書き出し → 起動コマンド。
///
/// 表示言語は呼び出し側が初期化しておくこと（理由文が英語で凍る。#983 で踏んだ）
pub fn plan(profile_name: &str) -> Result<MasterLaunchPlan, String> {
    let profile = match Profile::load(profile_name) {
        Ok(p) => p,
        // 未設定の既定プロファイルは組み込み既定で立てる（CLI と同じ緩和）
        Err(_) if profile_name == "default" => Profile::default(),
        Err(e) => return Err(e),
    };

    profile.validate_env()?;
    let cwd = profile.resolve_cwd()?;
    profile.validate_projects()?;

    let agent = profile.resolve_master_agent()?;
    // #983: 実行ファイルが無ければここで落とす（タブを作ってから
    // `command not found` を出すと、tako 側は成功と報告してしまう）
    orchestrator::agent_cli::preflight(agent).map_err(|e| e.message())?;

    let prompt_content = profile.build_system_prompt(profile_name);
    let dir = orchestrator::config_dir().ok_or("ホームディレクトリが取得できない")?;
    let prompt_path = dir.join(format!("_system_prompt_{profile_name}.md"));
    if let Some(parent) = prompt_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("system prompt の保存先を作れない: {e}"))?;
    }
    std::fs::write(&prompt_path, &prompt_content)
        .map_err(|e| format!("system prompt の書き出しに失敗: {e}"))?;

    let role_env = tako_core::handoff::master_role_env(profile_name);
    let pane_role = tako_core::handoff::master_pane_role(profile_name);
    let tako_bin = crate::dispatch::resolve_tako_binary();
    let command = orchestrator::build_master_cmd(&role_env, &profile, &prompt_path, &tako_bin)?;
    let remote_control = orchestrator::master_remote_control_decision(&profile)?;

    Ok(MasterLaunchPlan {
        profile_name: profile_name.to_string(),
        tab_title: tab_title_for(profile_name),
        pane_role,
        role_env,
        command,
        cwd,
        agent,
        model_label: profile.master_model_label(),
        effort: profile.effort.clone(),
        remote_control_opt_in: profile.remote_control_enabled(),
        remote_control,
    })
}

/// タブ名。CLI の `tako master -<名前>` と同じ形（`master` / `master-<名前>`）
pub fn tab_title_for(profile_name: &str) -> String {
    if profile_name.is_empty() || profile_name == "default" {
        "master".to_string()
    } else {
        format!("master-{profile_name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// role とタブ名の語彙は CLI の `-<名前>` 起動と一致していなければならない。
    /// ずれると #761 の事故（後任 / 別経路の master が default 扱いになる）が再来する
    #[test]
    fn role_とタブ名は_cli_の形と一致する() {
        assert_eq!(tab_title_for("default"), "master");
        assert_eq!(tab_title_for(""), "master");
        assert_eq!(tab_title_for("dev"), "master-dev");
        // 表示用と env 用の 2 語彙（正本は tako_core::handoff）
        assert_eq!(
            tako_core::handoff::master_pane_role("default"),
            "orchestrator-master"
        );
        assert_eq!(
            tako_core::handoff::master_pane_role("dev"),
            "orchestrator-master:dev"
        );
        assert_eq!(tako_core::handoff::master_role_env("default"), "master");
        assert_eq!(tako_core::handoff::master_role_env("dev"), "master:dev");
        // #1077 の所在解決が表示用 role からプロファイル名を戻せる（往復が閉じている）
        for name in ["default", "dev"] {
            let role = tako_core::handoff::master_pane_role(name);
            assert_eq!(
                crate::claude_remote_link::ProfileHint::from_role(&role),
                crate::claude_remote_link::ProfileHint::Master(Some(name)),
                "{name}: 表示用 role からプロファイル名へ戻せない"
            );
        }
    }

    /// 存在しないプロファイルは**ペインを作る前に**落とす
    #[test]
    fn 未登録プロファイルは組み立てで落ちる() {
        let err = plan("この名前のプロファイルは無い-1078").expect_err("落ちるべき");
        assert!(!err.is_empty());
    }

    /// opt-in していないプロファイルでは理由と opt-in コマンドが返る（受け入れ条件 ②）
    #[test]
    fn opt_in_していなければ理由と有効化コマンドを返す() {
        let plan = MasterLaunchPlan {
            profile_name: "dev".into(),
            tab_title: "master-dev".into(),
            pane_role: "orchestrator-master:dev".into(),
            role_env: "master:dev".into(),
            command: "claude".into(),
            cwd: None,
            agent: orchestrator::WorkerAgent::Claude,
            model_label: "CLI 既定".into(),
            effort: "high".into(),
            remote_control_opt_in: false,
            remote_control: crate::claude_remote::RemoteControlDecision::off(),
        };
        let v = plan.remote_control_json();
        assert_eq!(v["state"], "off");
        assert_eq!(v["opt_in"], false);
        assert!(v["reason"].as_str().is_some_and(|s| !s.is_empty()));
        assert_eq!(
            v["enable_command"],
            "tako orchestrator profiles set dev --remote-control true"
        );
        // 応答にコマンド本文（env の並びが載る）を入れない
        let full = plan.to_json();
        assert!(full.get("command").is_none());
        assert_eq!(full["role"], "orchestrator-master:dev");
    }
}
