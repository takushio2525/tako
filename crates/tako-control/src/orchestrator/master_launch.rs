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
//! ## タブ名はプロジェクト名（#2120）
//!
//! 起動でタブに付けるのは**管轄プロジェクトの名前を自動名として**（[`tab_naming`]）。
//! 以前は `master` / `master-<名前>` を手動名で付けていたため、master 自身の
//! `tako_rename_tab(source: "auto")` が全部弾かれてタブ名が役割名で固定された。
//! 汎用の master（管轄なし）はタブ名に触らず、採用（#1453）の時点でプロジェクト名になる。
//! 引き継ぎの後任は同じタブに立つのでタブ名に触らない（前任が付けた主題が残る）。
//! `tako solo` も同じ形で、作業フォルダのリポジトリ名を自動名で付ける（[`solo_tab_naming`]。#2127）
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
    /// 起動でタブへ付ける名前（#2120。`None` = 汎用 master なのでタブ名に触らない）
    pub tab_naming: Option<TabNaming>,
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
    /// 利用上限後の自動復帰（#813）をこの master のペインへ配るか（#1140。既定 false）。
    /// **適用するのは role を貼る dispatch 側**（`Request::Title`）なので、ここは
    /// 「起動した時点でどうなったか」を応答へ載せるための値
    pub limit_resume: bool,
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
            // 起動で付ける名前（#2120）。実際のタブ名は呼び出し側が上書きして返す
            "tab_title": self.tab_naming.as_ref().map(|n| n.title.as_str()),
            "role": self.pane_role,
            "agent": self.agent.as_str(),
            "model": self.model_label,
            "effort": self.effort,
            "cwd": self.cwd.as_ref().map(|p| p.display().to_string()),
            "limit_resume": self.limit_resume,
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
        tab_naming: tab_naming(profile_name, &profile),
        pane_role,
        role_env,
        command,
        cwd,
        agent,
        model_label: profile.master_model_label(),
        effort: profile.effort.clone(),
        limit_resume: orchestrator::resolve_master_limit_resume(&profile),
        remote_control_opt_in: profile.remote_control_enabled(),
        remote_control,
    })
}

/// ペインの表示名（`master` / `master-<名前>`）。CLI の `tako master -<名前>` と同じ形。
///
/// **タブ名には使わない**（#2120）。役割名でタブを固定すると、master が作業の主題で
/// 付け直せなくなる。タブ名は [`tab_naming`] が決める
pub fn pane_title_for(profile_name: &str) -> String {
    if profile_name.is_empty() || profile_name == "default" {
        "master".to_string()
    } else {
        format!("master-{profile_name}")
    }
}

/// 起動・採用でタブへ付ける名前（#2120）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabNaming {
    /// タブ名
    pub title: String,
    /// `Request::TabRename` の `source`。`Some("auto")` = 自動名（ユーザーの手動名は
    /// 上書きしない・master が後から付け直せる）。`None` = 手動名（#2120 前の A/B だけ）
    pub source: Option<String>,
}

impl TabNaming {
    /// 手動名か（`Request::TabNew` の `title` へ渡してよいのはこちらだけ。
    /// 明示タイトル付きの作成は手動名扱いになる）
    pub fn is_manual(&self) -> bool {
        self.source.is_none()
    }
}

/// master のタブの名前を決める（#2120）。
///
/// 管轄プロジェクトの名前を**自動名で**付ける（`takodev` の管轄が `[tako]` なら `tako`）。
/// 管轄が無い汎用の master は `None` = タブ名に触らない。
/// `TAKO_2120_LEGACY=1` のときだけ旧挙動（`master` / `master-<名前>` の手動名）を返す
pub fn tab_naming(profile_name: &str, profile: &Profile) -> Option<TabNaming> {
    if tako_core::master_tab::legacy() {
        return Some(TabNaming {
            title: pane_title_for(profile_name),
            source: None,
        });
    }
    tako_core::master_tab::initial_title(profile.projects.as_deref().unwrap_or_default()).map(
        |title| TabNaming {
            title,
            source: Some("auto".into()),
        },
    )
}

/// solo のタブの名前を決める（#2127。master の [`tab_naming`] と同じ形）。
///
/// solo には管轄プロジェクトが無いので、**solo が作業するフォルダ（`dir`）のリポジトリ名**を
/// 自動名で付ける（`tako_core::master_tab::solo_initial_title`）。HOME・ルートで立てた
/// solo と、フォルダが分からないときは `None` = タブ名に触らない。
/// `TAKO_2120_LEGACY=1` のときだけ旧挙動（`solo` / `solo-<名前>` の手動名）を返す
pub fn solo_tab_naming(suffix: Option<&str>, dir: Option<&std::path::Path>) -> Option<TabNaming> {
    if tako_core::master_tab::legacy() {
        return Some(TabNaming {
            title: tako_core::master_tab::legacy_solo_title(suffix),
            source: None,
        });
    }
    dir.and_then(tako_core::master_tab::solo_initial_title)
        .map(|title| TabNaming {
            title,
            source: Some("auto".into()),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// role とペイン名の語彙は CLI の `-<名前>` 起動と一致していなければならない。
    /// ずれると #761 の事故（後任 / 別経路の master が default 扱いになる）が再来する
    #[test]
    fn role_とペイン名は_cli_の形と一致する() {
        assert_eq!(pane_title_for("default"), "master");
        assert_eq!(pane_title_for(""), "master");
        assert_eq!(pane_title_for("dev"), "master-dev");
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

    /// #2120: タブ名は管轄プロジェクトの名前を自動名で。役割名・プロファイル名にしない
    #[test]
    fn タブ名は管轄プロジェクトの自動名() {
        if tako_core::master_tab::legacy() {
            return; // A/B の旧挙動で回しているときは対象外
        }
        let dedicated = Profile {
            projects: Some(vec!["tako".into()]),
            ..Profile::default()
        };
        let naming = tab_naming("takodev", &dedicated).expect("専用なら名前が付く");
        assert_eq!(naming.title, "tako");
        assert_eq!(naming.source.as_deref(), Some("auto"));
        assert!(!naming.is_manual());
        // 汎用（管轄なし）はタブ名に触らない = 採用の時点で付く
        assert_eq!(tab_naming("default", &Profile::default()), None);
        let empty = Profile {
            projects: Some(Vec::new()),
            ..Profile::default()
        };
        assert_eq!(tab_naming("fable", &empty), None);
    }

    /// #2127: solo のタブ名は作業フォルダのリポジトリ名を自動名で。役割名にしない
    #[test]
    fn soloのタブ名は作業フォルダのリポジトリの自動名() {
        if tako_core::master_tab::legacy() {
            return; // A/B の旧挙動で回しているときは対象外
        }
        let base =
            std::env::temp_dir().join(format!("tako-2127-solo-naming-{}", std::process::id()));
        let repo = base.join("my-app");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("src")).unwrap();
        let naming = solo_tab_naming(Some("fast"), Some(&repo.join("src")))
            .expect("リポジトリなら名前が付く");
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(naming.title, "my-app");
        assert_eq!(naming.source.as_deref(), Some("auto"));
        assert!(!naming.is_manual());
        // フォルダが分からなければタブ名に触らない
        assert_eq!(solo_tab_naming(None, None), None);
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
            tab_naming: None,
            pane_role: "orchestrator-master:dev".into(),
            role_env: "master:dev".into(),
            command: "claude".into(),
            cwd: None,
            agent: orchestrator::WorkerAgent::Claude,
            model_label: "CLI 既定".into(),
            effort: "high".into(),
            limit_resume: false,
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
