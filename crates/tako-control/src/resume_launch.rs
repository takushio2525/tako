//! resume_launch — 会話の再開コマンドを「元の起動と同じ組み立て」で作る 1 実装（Issue #1967）
//!
//! # なぜ要るのか
//!
//! 再開（ペインの右クリックの「会話を保って再起動」= #1067 のハーネス更新 /
//! `tako sessions resume`）は、以前は `sessions::resume_command` がセッションカタログの
//! メタ（model / effort）だけで `claude --resume <id>` を組んでいた。ところが
//!
//! - master / solo の会話はカタログに model / effort が**一度も**記録されない
//!   （本番 10/9 のカタログで master 38 件中 0 件）
//! - system prompt（`--append-system-prompt-file`）・Remote Control・許可モード
//!   （`--dangerously-skip-permissions` / `worker_agents.<agent>.args`）はカタログに無い
//!
//! ので、master を再開すると **effort が medium・tako の運用規則（system prompt）が落ちた**
//! claude が立っていた（master takodev が手で付け直して復旧した）。
//!
//! # 方針
//!
//! 起動の正本は種別ごとに既にある。**同じ関数を通して組み、末尾に `--resume <id>` を足す**:
//!
//! | 種別 | 正本（元の起動） | 材料 |
//! |---|---|---|
//! | master | `orchestrator::build_master_cmd`（`tako master -<名前>` / 引き継ぎの後任） | プロファイル |
//! | solo | 同上（`tako solo -<名前>`） | solo プロファイル |
//! | worker | `orchestrator::agent::build_worker_cmd`（spawn） | 起動した master のプロファイル + 記録した model / effort |
//! | それ以外 | `sessions::resume_command`（カタログのメタ） | カタログ |
//!
//! アカウント（`CLAUDE_CONFIG_DIR`）だけは**会話の記録がある場所**へ寄せる。記録の無い
//! config dir で resume すると `No conversation found` で落ちるため（#652 と同じ理由）。
//! プロファイルのアカウントと記録の所在が食い違ったら warnings に出す（黙って寄せない）。
//!
//! claude 以外（codex / agy）は起動の引数を resume へ持ち込めない・持ち込まない系統なので
//! 従来どおり `sessions::resume_command` を通す（能力マトリクス `resume_launch_args`）。

use std::path::{Path, PathBuf};

use crate::orchestrator::{self, agent::WorkerAgent, EnvPlan, Profile, ProfileKind};
use crate::sessions::SessionEntry;
use crate::transcript::TranscriptLocation;
use tako_core::agent_support::Agent;

/// どの組み立てを通したか（応答の `recipe` と persist.log に出す安定した語彙）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeRecipe {
    /// master のプロファイルから `tako master` と同じ組み立て
    MasterProfile,
    /// solo のプロファイルから `tako solo` と同じ組み立て
    SoloProfile,
    /// 起動した master のプロファイル + 記録した model / effort から spawn と同じ組み立て
    WorkerProfile,
    /// カタログのメタだけ（claude 以外・role の無いペイン・`TAKO_1967_LEGACY=1`）
    Catalog,
}

impl ResumeRecipe {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MasterProfile => "master_profile",
            Self::SoloProfile => "solo_profile",
            Self::WorkerProfile => "worker_profile",
            Self::Catalog => "catalog",
        }
    }
}

/// カタログのエントリに無い手掛かり（呼び出し側が知っているものだけ詰める）
#[derive(Debug, Clone, Default)]
pub struct ResumeHints {
    /// worker を起動した master のプロファイル名（spawn の `resolve_caller_profile` と同じ考え方。
    /// `None` は既定プロファイル）
    pub worker_profile: Option<String>,
    /// worker レジストリに記録した model / effort（spawn 時に解決済みの値。カタログより優先）
    pub worker_model: Option<String>,
    pub worker_effort: Option<String>,
}

/// 組み上がった再開コマンド
#[derive(Debug, Clone)]
pub struct ResumeLaunch {
    /// シェルへ送る 1 行（**診断ログへは出さない**。env の並びとパスが載る）
    pub command: String,
    pub recipe: ResumeRecipe,
    /// 使ったプロファイル名（master / solo / worker のときだけ）
    pub profile: Option<String>,
    /// 黙って寄せない食い違い（プロファイルが無い・アカウントと記録の所在が違う 等）
    pub warnings: Vec<String>,
    /// 起動の直前に書き出す system prompt（パスと本文）。**下見では書かない**ので
    /// 実行する呼び出し側が [`ResumeLaunch::materialize`] を呼ぶ
    prompt_file: Option<(PathBuf, String)>,
}

impl ResumeLaunch {
    /// 起動に要るファイル（system prompt）を書き出す。`tako master` と同じく毎回書き直すので、
    /// ファイルが消えていても・プロファイルの規則が変わっていても今の内容で起動する
    pub fn materialize(&self) -> Result<(), String> {
        let Some((path, content)) = &self.prompt_file else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("system prompt の保存先を作れない: {e}"))?;
        }
        std::fs::write(path, content).map_err(|e| format!("system prompt の書き出しに失敗: {e}"))
    }

    /// 書き出す system prompt のパス（検証・応答用）
    pub fn prompt_path(&self) -> Option<&Path> {
        self.prompt_file.as_ref().map(|(p, _)| p.as_path())
    }
}

/// 再開コマンドを組む（I/O あり: 会話の記録を探す・プロファイルを読む）。
///
/// `Err` = 再開できない（会話 ID の形が不正・記録が無い・プロファイルが壊れている 等）。
/// 理由は利用者へそのまま出せる日本語
pub fn resume_launch(
    session_id: &str,
    entry: &SessionEntry,
    hints: &ResumeHints,
) -> Result<ResumeLaunch, String> {
    let location = crate::transcript::locate_transcript(session_id);
    resume_launch_in(
        session_id,
        entry,
        hints,
        location.as_ref(),
        &crate::dispatch::resolve_tako_binary(),
        crate::launch_cmd::launch_dialect(),
        tako_core::session_restart::legacy_1967(),
    )
}

/// [`resume_launch`] の本体（会話の所在・tako の実体パス・方言・旧挙動を引数で受ける）
pub fn resume_launch_in(
    session_id: &str,
    entry: &SessionEntry,
    hints: &ResumeHints,
    location: Option<&TranscriptLocation>,
    tako_bin: &str,
    dialect: crate::launch_cmd::ShellDialect,
    legacy: bool,
) -> Result<ResumeLaunch, String> {
    if !crate::transcript::is_valid_session_id(session_id) {
        return Err("session_id の形式が不正".into());
    }
    let agent = entry
        .agent
        .as_deref()
        .and_then(Agent::parse)
        .unwrap_or(Agent::Claude);
    let catalog = || -> Result<ResumeLaunch, String> {
        Ok(ResumeLaunch {
            command: crate::sessions::resume_command(session_id, entry)?,
            recipe: ResumeRecipe::Catalog,
            profile: None,
            warnings: Vec::new(),
            prompt_file: None,
        })
    };
    // claude 以外は起動の引数を resume へ持ち込めない（codex resume は --model を受けない
    // = agent_resume::ResumeSpec の accepts_launch_flags）。旧挙動の A/B もここ
    if legacy || agent != Agent::Claude {
        return catalog();
    }
    let location = location.ok_or_else(|| {
        format!(
            "会話 {} の記録（claude の transcript）が見つからない。\
             削除されたか、登録されていない設定 dir にある（`tako orchestrator accounts list`）",
            crate::sessions::short_id(session_id)
        )
    })?;
    match entry.kind.as_str() {
        "master" => resume_profile_launch(
            session_id,
            ProfileKind::Master,
            entry.profile.as_deref(),
            location,
            tako_bin,
            dialect,
        ),
        "solo" => resume_profile_launch(
            session_id,
            ProfileKind::Solo,
            entry.profile.as_deref(),
            location,
            tako_bin,
            dialect,
        ),
        "worker" => resume_worker_launch(session_id, entry, hints, location, tako_bin, dialect),
        // role の無いペイン（手動起動）は元の起動を知らない = カタログのメタが全て
        _ => catalog(),
    }
}

/// master / solo: `tako master -<名前>` / `tako solo -<名前>` と同じ組み立て + `--resume`
fn resume_profile_launch(
    session_id: &str,
    kind: ProfileKind,
    name: Option<&str>,
    location: &TranscriptLocation,
    tako_bin: &str,
    dialect: crate::launch_cmd::ShellDialect,
) -> Result<ResumeLaunch, String> {
    let name = name.filter(|n| !n.is_empty()).unwrap_or(DEFAULT_PROFILE);
    let mut warnings = Vec::new();
    let (profile, profile_name) = load_profile_for_resume(kind, name, &mut warnings)?;
    // CLI の組み立てと同じ: 系統の解決は起動の前に（codex master の会話は claude で再開しない）
    let master_agent = profile.resolve_master_agent()?;
    if master_agent != WorkerAgent::Claude {
        return Err(format!(
            "プロファイル '{profile_name}' のエージェントは {} なので claude の会話を\
             その組み立てで再開できない（`tako sessions resume <id>` で開く）",
            master_agent.as_str()
        ));
    }
    let mut plan = profile.resolved_env_plan_for_master()?;
    pin_config_dir(&mut plan, location, &mut warnings);
    let (role_env, prompt_name, prompt_content) = match kind {
        ProfileKind::Master => (
            tako_core::handoff::master_role_env(name),
            format!("_system_prompt_{profile_name}.md"),
            profile.build_system_prompt(&profile_name),
        ),
        // solo の role は `solo[:<名前>]`（CLI の `tako solo` と同じ。ペインの role も同じ綴り）
        ProfileKind::Solo => (
            if name == DEFAULT_PROFILE {
                "solo".to_string()
            } else {
                format!("solo:{name}")
            },
            format!("_solo_system_prompt_{profile_name}.md"),
            profile.build_solo_system_prompt(&profile_name),
        ),
    };
    let prompt_path = orchestrator::config_dir()
        .ok_or("ホームディレクトリが取得できない")?
        .join(prompt_name);
    let launch = orchestrator::build_master_cmd_with_plan_in(
        &role_env,
        &profile,
        &plan,
        &prompt_path,
        tako_bin,
        dialect,
    )?;
    Ok(ResumeLaunch {
        command: with_resume_flag(launch, session_id),
        recipe: match kind {
            ProfileKind::Master => ResumeRecipe::MasterProfile,
            ProfileKind::Solo => ResumeRecipe::SoloProfile,
        },
        profile: Some(profile_name),
        warnings,
        prompt_file: Some((prompt_path, prompt_content)),
    })
}

/// worker: spawn と同じ組み立て（起動した master のプロファイル + 記録した model / effort）+ `--resume`
fn resume_worker_launch(
    session_id: &str,
    entry: &SessionEntry,
    hints: &ResumeHints,
    location: &TranscriptLocation,
    tako_bin: &str,
    dialect: crate::launch_cmd::ShellDialect,
) -> Result<ResumeLaunch, String> {
    let mut warnings = Vec::new();
    let name = hints
        .worker_profile
        .as_deref()
        .filter(|n| !n.is_empty())
        .unwrap_or(DEFAULT_PROFILE);
    let (profile, profile_name) =
        load_profile_for_resume(ProfileKind::Master, name, &mut warnings)?;
    profile.validate_env()?;
    // spawn は env 計画にアカウントを混ぜる（`resolved_env_plan_with_account`）。
    // 再開ではアカウントを**会話の記録の所在**で決める（spawn 時の `--account` は記録に無い）
    let mut plan = profile.resolved_env_plan();
    pin_config_dir(&mut plan, location, &mut warnings);
    // 記録した値（spawn 時に解決済み）を明示指定として渡す。記録が無ければ spawn と同じ解決
    let model = hints.worker_model.as_deref().or(entry.model.as_deref());
    let effort = hints.worker_effort.as_deref().or(entry.effort.as_deref());
    let launch = profile.resolve_agent_launch(WorkerAgent::Claude, model, effort);
    let remote_control =
        orchestrator::remote_control_decision(&profile, WorkerAgent::Claude, &plan);
    // role は spawn と同じ綴り（ラベルが無ければ project だけ）
    let project = entry.project.as_deref().unwrap_or("resumed");
    let role = match entry.label.as_deref() {
        Some(l) => format!("worker:{project}:{l}"),
        None => format!("worker:{project}"),
    };
    let cmd = orchestrator::agent::build_worker_cmd_in(
        &orchestrator::agent::WorkerLaunch {
            agent: WorkerAgent::Claude,
            role: &role,
            model: launch.model.as_deref(),
            effort: launch.effort.as_deref(),
            skip_permissions: launch.skip_permissions,
            allow_sandbox_bypass: launch.allow_sandbox_bypass,
            remote_control: remote_control.enabled(),
            extra_args: &launch.extra_args,
            tako_bin: Some(tako_bin),
            env: &plan,
        },
        dialect,
    );
    Ok(ResumeLaunch {
        command: with_resume_flag(cmd, session_id),
        recipe: ResumeRecipe::WorkerProfile,
        profile: Some(profile_name),
        warnings,
        prompt_file: None,
    })
}

const DEFAULT_PROFILE: &str = "default";

/// プロファイルを読む。**壊れているものを既定へ黙って化けさせない**（#1967 の主題）。
///
/// ファイルが無いときだけ既定プロファイルで組む: `tako master <ラベル>`（`-` なし）の
/// master は role にラベルが載るがプロファイルは既定で起動している（CLI の旧い書式）ので、
/// その起動を再現すると既定になる。その場合も warnings に出す
fn load_profile_for_resume(
    kind: ProfileKind,
    name: &str,
    warnings: &mut Vec<String>,
) -> Result<(Profile, String), String> {
    let exists = kind.path(name).map(|p| p.is_file()).unwrap_or(false);
    if exists {
        return orchestrator::load_profile_of(kind, name).map(|p| (p, name.to_string()));
    }
    if name != DEFAULT_PROFILE {
        warnings.push(format!(
            "{} プロファイル '{name}' が見つからないので既定プロファイルの設定で再開した\
             （`{} {name}` のようにラベルだけで起動した master と同じ組み立て）",
            kind.as_str(),
            kind.launch_bin()
        ));
    }
    // 既定プロファイルが未作成なら組み込み既定（`tako master` / `tako solo` と同じ緩和）
    let profile = orchestrator::load_profile_of(kind, DEFAULT_PROFILE)
        .unwrap_or_else(|_| kind.default_profile());
    Ok((profile, DEFAULT_PROFILE.to_string()))
}

/// env 計画の `CLAUDE_CONFIG_DIR` を**会話の記録がある config dir** へ寄せる。
///
/// プロファイルのアカウントが同じ場所を指していれば計画には触らない（`tako master` と
/// 1 バイトも変わらない）。指していなければ寄せて warnings に出す
fn pin_config_dir(plan: &mut EnvPlan, location: &TranscriptLocation, warnings: &mut Vec<String>) {
    let key = orchestrator::CLAUDE_CONFIG_DIR_ENV;
    let wanted = location.config_dir.clone();
    let current = plan.claude_config_dir().map(PathBuf::from);
    if current.as_deref() == Some(wanted.as_path()) {
        return;
    }
    if let Some(current) = current {
        warnings.push(format!(
            "プロファイルの設定 dir（{}）に会話の記録が無いので、記録のある設定 dir で再開した",
            current.display()
        ));
    }
    plan.exports.retain(|(k, _)| k != key);
    plan.unsets.retain(|k| k != key);
    if location.is_default {
        plan.unsets.push(key.to_string());
    } else {
        plan.exports
            .push((key.to_string(), wanted.display().to_string()));
    }
}

/// 起動コマンドの末尾へ `--resume <id>` を足す（書式の正本は `agent_resume::resume_spec`）
fn with_resume_flag(mut cmd: String, session_id: &str) -> String {
    let spec =
        tako_core::agent_resume::resume_spec(Agent::Claude).expect("claude は resume の手段を持つ");
    if let Some(flag) = spec.id_flag {
        cmd.push(' ');
        cmd.push_str(flag);
    }
    cmd.push(' ');
    cmd.push_str(session_id);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch_cmd::ShellDialect;

    const ID: &str = "cb6d0127-b67d-44ac-b897-065b9f0678f0";

    fn location(dir: &str, is_default: bool) -> TranscriptLocation {
        TranscriptLocation {
            path: PathBuf::from(dir)
                .join("projects/p")
                .join(format!("{ID}.jsonl")),
            config_dir: PathBuf::from(dir),
            is_default,
        }
    }

    fn master_entry(profile: &str) -> SessionEntry {
        SessionEntry {
            kind: "master".into(),
            profile: Some(profile.into()),
            agent: Some("claude".into()),
            ..Default::default()
        }
    }

    /// 隔離した config dir（テストビルドは常に隔離先）へプロファイルを置く
    fn save_profile(name: &str, yaml: &str) {
        let profile: Profile = serde_yaml::from_str(yaml).unwrap();
        profile.save(name).unwrap();
    }

    /// #1967 の主題: master の再開コマンドは `tako master -<名前>` と**同じ組み立て**で、
    /// 末尾に `--resume` が付くだけ（model / effort / system prompt / Remote Control が落ちない）
    #[test]
    fn master_の再開は_tako_master_と同じ組み立てに_resume_を足すだけ() {
        save_profile(
            "rl1967a",
            "model: claude-opus-5-5\neffort: xhigh\nremote_control: true\n",
        );
        let loc = location("/tmp/rl1967a-config", false);
        let got = resume_launch_in(
            ID,
            &master_entry("rl1967a"),
            &ResumeHints::default(),
            Some(&loc),
            "/opt/tako",
            ShellDialect::Posix,
            false,
        )
        .unwrap();
        assert_eq!(got.recipe, ResumeRecipe::MasterProfile);
        assert_eq!(got.profile.as_deref(), Some("rl1967a"));
        // 元の起動（tako master -rl1967a）を同じ計画で組み、`--resume <id>` を足したものと一致
        let profile = Profile::load("rl1967a").unwrap();
        let mut plan = profile.resolved_env_plan_for_master().unwrap();
        plan.unsets
            .retain(|k| k != orchestrator::CLAUDE_CONFIG_DIR_ENV);
        plan.exports.push((
            orchestrator::CLAUDE_CONFIG_DIR_ENV.into(),
            "/tmp/rl1967a-config".into(),
        ));
        let original = orchestrator::build_master_cmd_with_plan_in(
            "master:rl1967a",
            &profile,
            &plan,
            got.prompt_path().unwrap(),
            "/opt/tako",
            ShellDialect::Posix,
        )
        .unwrap();
        assert_eq!(got.command, format!("{original} --resume {ID}"));
        for needle in [
            "TAKO_ORCHESTRATOR_ROLE='master:rl1967a'",
            "--model 'claude-opus-5-5'",
            "--effort xhigh",
            "--append-system-prompt-file",
            "_system_prompt_rl1967a.md",
        ] {
            assert!(got.command.contains(needle), "{needle}: {}", got.command);
        }
        // 下見では system prompt を書かない（materialize で初めて書く）
        let path = got.prompt_path().unwrap().to_path_buf();
        let _ = std::fs::remove_file(&path);
        assert!(!path.exists());
        got.materialize().unwrap();
        assert!(
            path.is_file(),
            "materialize で書き出す（消えていても起動できる）"
        );
    }

    /// 対照: 旧挙動（`TAKO_1967_LEGACY=1`）はカタログのメタだけ = 本番 10/9 の
    /// `TAKO_ORCHESTRATOR_ROLE='master:minige' claude --resume <id>`（model / effort / prompt 無し）
    #[test]
    fn 旧挙動はカタログのメタだけで組む() {
        save_profile("rl1967b", "model: claude-opus-5-5\neffort: xhigh\n");
        let loc = location("/tmp/rl1967b-config", false);
        let got = resume_launch_in(
            ID,
            &master_entry("rl1967b"),
            &ResumeHints::default(),
            Some(&loc),
            "/opt/tako",
            ShellDialect::Posix,
            true,
        );
        let old = got.unwrap();
        assert_eq!(old.recipe, ResumeRecipe::Catalog);
        assert!(
            old.command.ends_with(&format!(
                "TAKO_ORCHESTRATOR_ROLE='master:rl1967b' claude --resume {ID}"
            )),
            "{}",
            old.command
        );
    }

    #[test]
    fn 記録が無ければ組まない() {
        let err = resume_launch_in(
            ID,
            &master_entry("default"),
            &ResumeHints::default(),
            None,
            "/opt/tako",
            ShellDialect::Posix,
            false,
        )
        .unwrap_err();
        assert!(err.contains("記録"), "{err}");
        assert!(err.contains("cb6d0127"), "{err}");
    }

    /// 会話の記録が既定の config dir にあれば `unset`、別の dir なら `export` で寄せる。
    /// プロファイルのアカウントと食い違ったら warnings に出す（黙って寄せない）
    #[test]
    fn 設定_dir_は会話の記録の所在へ寄せる() {
        save_profile("rl1967c", "effort: high\n");
        let in_default = resume_launch_in(
            ID,
            &master_entry("rl1967c"),
            &ResumeHints::default(),
            Some(&location("/tmp/rl1967c-home/.claude", true)),
            "/opt/tako",
            ShellDialect::Posix,
            false,
        )
        .unwrap();
        assert!(
            in_default.command.starts_with("unset CLAUDE_CONFIG_DIR; "),
            "{}",
            in_default.command
        );
        let mut plan = EnvPlan {
            exports: vec![(
                orchestrator::CLAUDE_CONFIG_DIR_ENV.into(),
                "/tmp/other-account".into(),
            )],
            unsets: Vec::new(),
        };
        let mut warnings = Vec::new();
        pin_config_dir(
            &mut plan,
            &location("/tmp/rl1967c-acct", false),
            &mut warnings,
        );
        assert_eq!(
            plan.export_value(orchestrator::CLAUDE_CONFIG_DIR_ENV),
            Some("/tmp/rl1967c-acct")
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        // 同じ場所なら計画に触らず warnings も出さない
        let mut same = plan.clone();
        let mut none = Vec::new();
        pin_config_dir(&mut same, &location("/tmp/rl1967c-acct", false), &mut none);
        assert_eq!(same, plan);
        assert!(none.is_empty());
    }

    /// プロファイルが**壊れている**ときは既定へ化けさせない（#1967 の主題）。
    /// ファイルが無いとき（ラベルだけの旧い master）だけ既定で組み、warnings に出す
    #[test]
    fn 壊れたプロファイルを既定へ化けさせない() {
        let dir = orchestrator::profiles_dir().unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("rl1967d.yaml"), "effort: [壊れた\n").unwrap();
        let loc = location("/tmp/rl1967d-config", false);
        let err = resume_launch_in(
            ID,
            &master_entry("rl1967d"),
            &ResumeHints::default(),
            Some(&loc),
            "/opt/tako",
            ShellDialect::Posix,
            false,
        )
        .unwrap_err();
        assert!(err.contains("パース"), "{err}");
        let missing = resume_launch_in(
            ID,
            &master_entry("rl1967-label-only"),
            &ResumeHints::default(),
            Some(&loc),
            "/opt/tako",
            ShellDialect::Posix,
            false,
        )
        .unwrap();
        assert_eq!(missing.profile.as_deref(), Some("default"));
        assert_eq!(missing.warnings.len(), 1, "{:?}", missing.warnings);
        // role はラベルのまま（`tako master rl1967-label-only` の起動と同じ）
        assert!(
            missing
                .command
                .contains("TAKO_ORCHESTRATOR_ROLE='master:rl1967-label-only'"),
            "{}",
            missing.command
        );
    }

    #[test]
    fn solo_は_tako_solo_と同じ_role_と_prompt() {
        let dir = orchestrator::solo_profiles_dir().unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("rl1967e.yaml"),
            "effort: high\nmodel: claude-opus-5-5\n",
        )
        .unwrap();
        let entry = SessionEntry {
            kind: "solo".into(),
            profile: Some("rl1967e".into()),
            ..Default::default()
        };
        let got = resume_launch_in(
            ID,
            &entry,
            &ResumeHints::default(),
            Some(&location("/tmp/rl1967e", false)),
            "/opt/tako",
            ShellDialect::Posix,
            false,
        )
        .unwrap();
        assert_eq!(got.recipe, ResumeRecipe::SoloProfile);
        for needle in [
            "TAKO_ORCHESTRATOR_ROLE='solo:rl1967e'",
            "--effort high",
            "_solo_system_prompt_rl1967e.md",
        ] {
            assert!(got.command.contains(needle), "{needle}: {}", got.command);
        }
        assert!(got.command.ends_with(&format!(" --resume {ID}")));
    }

    /// worker: spawn と同じ組み立て（許可スキップ・追加引数・記録した model / effort）
    #[test]
    fn worker_は_spawn_と同じ組み立て() {
        save_profile(
            "rl1967f",
            "effort: xhigh\nworker_agents:\n  claude:\n    args: [\"--permission-mode\", \"auto\"]\n",
        );
        let entry = SessionEntry {
            kind: "worker".into(),
            project: Some("tako".into()),
            label: Some("#1967 検証".into()),
            agent: Some("claude".into()),
            ..Default::default()
        };
        let hints = ResumeHints {
            worker_profile: Some("rl1967f".into()),
            worker_model: Some("claude-opus-5-5".into()),
            worker_effort: Some("high".into()),
        };
        let got = resume_launch_in(
            ID,
            &entry,
            &hints,
            Some(&location("/tmp/rl1967f", false)),
            "/opt/tako",
            ShellDialect::Posix,
            false,
        )
        .unwrap();
        assert_eq!(got.recipe, ResumeRecipe::WorkerProfile);
        for needle in [
            "TAKO_ORCHESTRATOR_ROLE='worker:tako:#1967 検証'",
            "--model claude-opus-5-5",
            "--effort high",
            "--permission-mode auto",
        ] {
            assert!(got.command.contains(needle), "{needle}: {}", got.command);
        }
        assert!(got.command.ends_with(&format!(" --resume {ID}")));
    }

    /// claude 以外とロールの無いペインはカタログのメタで組む（元の起動を知らない / 持ち込めない）
    #[test]
    fn claude_以外とロールの無いペインはカタログ() {
        let pane = SessionEntry {
            kind: "pane".into(),
            agent: Some("claude".into()),
            ..Default::default()
        };
        let got = resume_launch_in(
            ID,
            &pane,
            &ResumeHints::default(),
            Some(&location("/tmp/rl1967g", false)),
            "/opt/tako",
            ShellDialect::Posix,
            false,
        );
        if let Ok(got) = got {
            assert_eq!(got.recipe, ResumeRecipe::Catalog);
        }
        let codex = SessionEntry {
            kind: "master".into(),
            agent: Some("codex".into()),
            ..Default::default()
        };
        let got = resume_launch_in(
            ID,
            &codex,
            &ResumeHints::default(),
            None,
            "/opt/tako",
            ShellDialect::Posix,
            false,
        );
        if let Ok(got) = got {
            assert_eq!(got.recipe, ResumeRecipe::Catalog);
        }
    }
}
