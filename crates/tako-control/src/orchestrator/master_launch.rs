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
//!
//! ## 読めない default.yaml でも立てるが、黙らない（#2227）
//!
//! プロファイルの読み込みは [`load_profile`] の 1 実装（CLI / スマホ / 返答の配送が通る）。
//! `default` が読めないときは**組み込み既定で立てる**（#916 の「退避して既定値で動き、
//! 知らせる」と同じ方針。止めると YAML を直せない利用者が master を立てられず、
//! スマホ・返答の配送からの起動も失敗する）。その代わり、個人ルール・モデルが効いていない
//! ことを CLI の `警告:` 行（スマホの起動は応答の `warnings` = PWA の起動結果の画面）・
//! persist.log・master 本人の system prompt（[`with_startup_warning`]）へ出す。CLI の行は
//! claude の全画面 TUI に隠れうるので、どの経路でも利用者へ確実に届くのは master の最初の返答。
//! `-<key>` の専用プロファイルが #1825 で作られていないときは、理由と直し方を足して止める。
//!
//! 引き継ぎの後任（[`load_successor_profile`]）と `tako solo`（[`load_solo`]）も同じ形で読む
//! （#2233）。表示・判定だけの経路は立てないので `orchestrator::load_for_display`（読むだけ・
//! 応答の `warnings` へ理由）を通す。
//!
//! worker の spawn とコンフリクト解消エージェントは呼び出し元 master のプロファイルで立てるので
//! [`load_caller_profile`]（同じ退避つきの読み方・止めずに警告）を、adopt の起動時プロファイル・
//! `orchestrator guide`・自動復帰の既定は `orchestrator::read_for_judgment`（読むだけ）を通す（#2256）

use std::path::{Path, PathBuf};

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
    /// 立てるが利用者に必ず見せる理由（読めない default.yaml を組み込み既定で代えた等。#2227）
    pub warning: Option<String>,
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
            // 立てたが黙らせない理由（#2227。`projects add` の `warnings` と同じ形）
            "warnings": self.warning.iter().collect::<Vec<_>>(),
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
    // 読み込みは CLI と同じ 1 実装（#2227。読めない default は立てるが warning を持つ）
    let LaunchProfile { profile, warning } = load_profile(profile_name)?;

    profile.validate_env()?;
    let cwd = profile.resolve_cwd()?;
    profile.validate_projects()?;

    let agent = profile.resolve_master_agent()?;
    // #983: 実行ファイルが無ければここで落とす（タブを作ってから
    // `command not found` を出すと、tako 側は成功と報告してしまう）
    orchestrator::agent_cli::preflight(agent).map_err(|e| e.message())?;

    let prompt_content = with_startup_warning(
        profile.build_system_prompt(profile_name),
        warning.as_deref(),
    );
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
        warning,
    })
}

/// master の起動で使うプロファイル（[`load_profile`] の結果。#2227）
#[derive(Debug, Clone)]
pub struct LaunchProfile {
    pub profile: Profile,
    /// 立てるが利用者に必ず見せる理由（`None` = 黙ってよい）
    pub warning: Option<String>,
}

/// #2227 の A/B。`TAKO_2227_LEGACY=1` で修正前（読めない default.yaml は黙って組み込み既定・
/// `-<key>` は直し方の無い「見つからない」）へ戻す
pub fn legacy_profile_load() -> bool {
    std::env::var_os("TAKO_2227_LEGACY").is_some_and(|v| !v.is_empty() && v != "0")
}

/// master の起動で使うプロファイルを読む（#2227）。**CLI の `tako master` と [`plan`]
/// （スマホの「+ master」・返答の配送）が同じ 1 実装を通る**。
///
/// - `default` が読めないときは**組み込み既定で立てる**が、理由を `warning` に載せて
///   persist.log へも 1 行残す（中身は `.unreadable.bak` へ退避・default.yaml には触らない）。
///   無いだけなら従来どおり黙って組み込み既定（ゼロコンフィグの初回）
/// - `-<key>` が無く、そのキーが登録済みで継承元の default.yaml を読めない
///   （#1825 で作っていない）ときは、「見つからない」に理由と直し方を足して止める
pub fn load_profile(name: &str) -> Result<LaunchProfile, String> {
    if legacy_profile_load() {
        let profile = match Profile::load(name) {
            Ok(p) => p,
            Err(_) if name == tako_core::handoff::DEFAULT_PROFILE => Profile::default(),
            Err(e) => return Err(e),
        };
        return Ok(LaunchProfile {
            profile,
            warning: None,
        });
    }
    if name == tako_core::handoff::DEFAULT_PROFILE {
        let loaded = load_default_at(
            orchestrator::ProfileKind::Master,
            &orchestrator::ProfileKind::Master.path(name)?,
        );
        if let Some(w) = &loaded.warning {
            crate::diag::persist_log(&format!("[master] {name}: {w}"));
        }
        return Ok(loaded);
    }
    match Profile::load(name) {
        Ok(profile) => Ok(LaunchProfile {
            profile,
            warning: None,
        }),
        Err(e) => match orchestrator::missing_project_profile_reason(name) {
            Some(why) => {
                let msg = format!("{e}。{why}");
                crate::diag::persist_log(&format!("[master] {name}: {msg}"));
                Err(msg)
            }
            None => Err(e),
        },
    }
}

/// 引き継ぎで立てる後任 master のプロファイルを読む（#2233）。
///
/// 読み込みは CLI / スマホ / 返答の配送と同じ [`load_profile`]（読めない default は組み込み既定で
/// 立てて警告）。`load_profile` が止める形（専用プロファイルが無い・読めない）でも、**引き継ぎは
/// 止めずに組み込み既定で後任を立てて警告する**: 引き継ぎは前任が ctx の上限に近いときの継続の
/// 仕組みで、止めると人の居ない時間帯に master そのものを失いうる（`tako master -<key>` は
/// 人が打つので止めて直させる、と役割が違う）。
///
/// 以前は `Profile::load(..).unwrap_or_default()` で、後任だけ個人ルールの抜けた master が
/// 黙って立っていた（実測）。`TAKO_2233_LEGACY=1` で修正前へ戻す
pub fn load_successor_profile(name: &str) -> LaunchProfile {
    if orchestrator::legacy_silent_default_rest() {
        return LaunchProfile {
            profile: Profile::load(name).unwrap_or_default(),
            warning: None,
        };
    }
    match load_profile(name) {
        Ok(loaded) => loaded,
        Err(e) => {
            // 前任と後任が読む文なので、どのプロファイルか・何が抜けるかまで言う
            // （パース失敗の文にはプロファイルの名前が無い）
            let w = format!(
                "後任 master のプロファイル '{name}' を読めない（{e}）。引き継ぎは止めずに、\
                 個人ルール（prompt_blocks）・モデル・アカウントなどの設定を使わない組み込み既定で\
                 後任を立てる。プロファイルを直すと次の起動から効く"
            );
            crate::diag::persist_log(&format!("[master] {name}: {w}"));
            LaunchProfile {
                profile: Profile::default(),
                warning: Some(w),
            }
        }
    }
}

/// 呼び出し元 master のプロファイルで **master / solo 本人ではないエージェント**を立てる経路
/// （worker の spawn・コンフリクト解消エージェント）の読み込み（#2256）。`launching` は立てるものの
/// 名前（「worker」等。理由文に入る）。
///
/// 読み方は default の起動・継承元と同じ 1 実装（[`orchestrator::profile_base_at`] = 無い =
/// 組み込み既定・空 = 設定なし・読めない = 中身を退避して理由）。読めなくても**止めずに**組み込み
/// 既定で立てる: master は人の居ない時間帯にも spawn するので、止めると作業ごと止まる（master 自身も
/// 同じ組み込み既定で立ち、起動時に警告している = #2227）。代わりに何が効かないかを warning に持つ。
///
/// **ここでは記録しない**: background の先取り（`dispatch::probe_needs`）が同じ解決を通るので、
/// persist.log と応答の `warnings` へ出すのは立てた dispatch だけにする。
/// 以前は `Profile::load(..).unwrap_or_default()` で黙っていた。`TAKO_2256_LEGACY=1` で修正前へ戻す
pub fn load_caller_profile(name: &str, launching: &str) -> LaunchProfile {
    if orchestrator::legacy_silent_default_2256() {
        return LaunchProfile {
            profile: Profile::load(name).unwrap_or_default(),
            warning: None,
        };
    }
    let kind = orchestrator::ProfileKind::Master;
    // 名前が不正（修正前も `Profile::load` が断って既定だった）= 読むべきファイルが無い
    let Ok(path) = kind.path(name) else {
        return LaunchProfile {
            profile: kind.default_profile(),
            warning: None,
        };
    };
    match orchestrator::profile_base_at(kind, &path) {
        Ok(profile) => LaunchProfile {
            profile,
            warning: None,
        },
        Err(why) => LaunchProfile {
            profile: kind.default_profile(),
            warning: Some(format!(
                "{why}。呼び出し元 master のプロファイル '{name}' の設定（worker のモデル・effort・\
                 agent・アカウント・env・projects の制限）を使わず、組み込み既定で {launching} を\
                 立てる。直すと次から効く"
            )),
        },
    }
}

/// `tako solo` の起動で使うプロファイルを読む（#2233）。master の [`load_profile`] と同じ形:
/// `default` が読めないときは solo の組み込み既定で立てるが warning を持ち（中身は退避・
/// default.yaml には触らない）、persist.log へ 1 行残す。無いだけなら黙る。
/// `-<名前>` が読めなければ従来どおり止める。`TAKO_2233_LEGACY=1` で修正前（黙って既定）へ戻す
pub fn load_solo(name: &str) -> Result<LaunchProfile, String> {
    if orchestrator::legacy_silent_default_rest() {
        let profile = match orchestrator::load_solo_profile(name) {
            Ok(p) => p,
            Err(_) if name == tako_core::handoff::DEFAULT_PROFILE => {
                orchestrator::solo_default_profile()
            }
            Err(e) => return Err(e),
        };
        return Ok(LaunchProfile {
            profile,
            warning: None,
        });
    }
    if name == tako_core::handoff::DEFAULT_PROFILE {
        let loaded = load_default_at(
            orchestrator::ProfileKind::Solo,
            &orchestrator::ProfileKind::Solo.path(name)?,
        );
        if let Some(w) = &loaded.warning {
            crate::diag::persist_log(&format!("[solo] {name}: {w}"));
        }
        return Ok(loaded);
    }
    orchestrator::load_solo_profile(name).map(|profile| LaunchProfile {
        profile,
        warning: None,
    })
}

/// [`load_profile`] / [`load_solo`] の `default` の本体（パス指定。テストは一時 dir で回す）。
///
/// 読み方は専用プロファイルの継承元と同じ 1 実装（[`orchestrator::profile_base_at`]。
/// 無い = 種別の組み込み既定・空 = 設定なし・読めない = 退避して `Err`）。`Err` を握りつぶさず
/// warning にする
pub fn load_default_at(kind: orchestrator::ProfileKind, path: &Path) -> LaunchProfile {
    match orchestrator::profile_base_at(kind, path) {
        Ok(profile) => LaunchProfile {
            profile,
            warning: None,
        },
        Err(why) => LaunchProfile {
            profile: kind.default_profile(),
            warning: Some(format!(
                "{why}。default.yaml の個人ルール（prompt_blocks）・モデルなどの設定を使わず、\
                 組み込み既定で {} を立てる。default.yaml を直すと次の起動から効く",
                kind.as_str()
            )),
        },
    }
}

/// 起動時の警告を master / solo 本人の system prompt の末尾へも添える（#2227 / #2233）。
///
/// CLI の `警告:` 行は claude の全画面 TUI に隠れうる。本人が最初の返答で利用者へ
/// 伝えれば、どの起動経路（CLI / スマホ / 返答の配送 / 引き継ぎの後任）でも確実に届く。
/// 警告が無ければ不変
pub fn with_startup_warning(prompt: String, warning: Option<&str>) -> String {
    match warning {
        None => prompt,
        Some(w) => format!(
            "{prompt}\n\n## Startup warning from tako (#2227)\n\n\
             This session started on tako's built-in defaults because its profile could not be \
             read, so the user's personal rules (`prompt_blocks`) and model settings from it are \
             NOT in effect. Tell the user this in your first reply, quoting the detail below, \
             and do not guess the missing rules.\n\nDetail: {w}\n"
        ),
    }
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

    const M: orchestrator::ProfileKind = orchestrator::ProfileKind::Master;
    const S: orchestrator::ProfileKind = orchestrator::ProfileKind::Solo;

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
            warning: None,
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
        assert_eq!(
            full["warnings"],
            serde_json::json!([]),
            "警告が無ければ空の列"
        );
        let warned = MasterLaunchPlan {
            warning: Some("w2227".into()),
            ..plan
        };
        assert_eq!(warned.to_json()["warnings"], serde_json::json!(["w2227"]));
    }

    /// #2227 の default.yaml（**共有の隔離先ではなく使い捨て dir** に置く。共有の
    /// profiles/default.yaml を壊すと、並行する他のテストが壊れた default を読む）
    fn t2227_default(body: Option<&[u8]>) -> (tako_core::test_residue::ScratchDir, PathBuf) {
        let dir = tako_core::test_residue::ScratchDir::new("issue2227-default");
        assert!(
            dir.path().starts_with(std::env::temp_dir()),
            "一時 dir の外に書こうとしている: {}",
            dir.path().display()
        );
        let path = dir.path().join("default.yaml");
        if let Some(body) = body {
            std::fs::write(&path, body).expect("default.yaml を置ける");
        }
        (dir, path)
    }

    /// 読めない default（壊れた YAML / 型違い / UTF-8 でない）は組み込み既定で立てるが、
    /// 理由・直し方を warning に持ち、中身を退避して default.yaml には触らない
    #[test]
    fn issue2227_読めないdefaultは組み込み既定で立てて警告する() {
        for body in [
            &b"model: m2227\nprompt_blocks:\n  append: \"PERSONAL\n  prepend: [\n"[..],
            &b"model: m2227\nprompt_blocks:\n  append: [PERSONAL]\n"[..],
            &b"model: m2227\n\xff\xfe\x00broken\n"[..],
        ] {
            let (_dir, path) = t2227_default(Some(body));
            let loaded = load_default_at(M, &path);
            let w = loaded
                .warning
                .as_deref()
                .expect("黙って組み込み既定へ落としている");
            assert!(
                w.contains(&path.display().to_string())
                    && w.contains("組み込み既定")
                    && w.contains("default.yaml を直す"),
                "理由・直し方が無い: {w}"
            );
            assert_eq!(loaded.profile.model, None, "組み込み既定で立てていない");
            assert_eq!(
                std::fs::read(&path).expect("元が残る"),
                body,
                "default.yaml を書き換えている"
            );
        }
    }

    /// 回帰なし: 読める / 空 / 無い default は従来どおり黙って立つ（無い = ゼロコンフィグの初回）
    #[test]
    fn issue2227_読める空無いdefaultは警告しない() {
        let (_d1, good) = t2227_default(Some(b"model: m2227\nprompt_blocks:\n  append: P\n"));
        let loaded = load_default_at(M, &good);
        assert_eq!(loaded.warning, None);
        assert_eq!(loaded.profile.model.as_deref(), Some("m2227"));
        let (_d2, empty) = t2227_default(Some(b""));
        assert_eq!(load_default_at(M, &empty).warning, None, "空は設定なし");
        let (_d3, absent) = t2227_default(None);
        let loaded = load_default_at(M, &absent);
        assert_eq!(loaded.warning, None, "無いのは初回の正常な状態");
        assert!(!absent.exists(), "読むだけで作らない");
    }

    /// #2233: `tako solo` の default も同じ形（solo の組み込み既定で立てて警告・退避・元は不変）。
    /// 無い / 空は黙って solo の既定（effort が master より低い）
    #[test]
    fn issue2233_soloの読めないdefaultはsoloの既定で立てて警告する() {
        let body = &b"model: m2233\nprompt_blocks:\n  append: \"PERSONAL\n  prepend: [\n"[..];
        let (_dir, path) = t2227_default(Some(body));
        let loaded = load_default_at(S, &path);
        let w = loaded
            .warning
            .as_deref()
            .expect("黙って solo の既定へ落としている");
        assert!(
            w.contains(&path.display().to_string()) && w.contains("組み込み既定で solo を立てる"),
            "どのファイルか・何を立てるかが無い: {w}"
        );
        assert_eq!(loaded.profile.model, None);
        assert_eq!(
            loaded.profile.effort,
            orchestrator::solo_default_profile().effort,
            "solo ではなく master の既定で立てている"
        );
        assert!(
            path.with_file_name("default.yaml.unreadable.bak").is_file(),
            "中身を退避していない"
        );
        assert_eq!(std::fs::read(&path).expect("元が残る"), body);

        let (_d2, absent) = t2227_default(None);
        let loaded = load_default_at(S, &absent);
        assert_eq!(loaded.warning, None, "無いのは初回の正常な状態");
        assert_eq!(
            loaded.profile.effort,
            orchestrator::solo_default_profile().effort
        );
        let (_d3, good) = t2227_default(Some(b"effort: low\n"));
        let loaded = load_default_at(S, &good);
        assert_eq!(loaded.warning, None);
        assert_eq!(loaded.profile.effort, "low");
    }

    #[test]
    fn issue2227_警告はmaster本人のpromptにも載る() {
        assert_eq!(
            with_startup_warning("P".into(), None),
            "P",
            "警告が無ければ不変"
        );
        let p = with_startup_warning("P".into(), Some("d.yaml を読めない"));
        assert!(p.starts_with("P\n"), "元の prompt を残す: {p}");
        assert!(
            p.contains("Startup warning") && p.contains("d.yaml を読めない"),
            "{p}"
        );
    }
}
