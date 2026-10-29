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
//! codex の worker の `CODEX_HOME` も同じ考え方で、rollout を見つけた置き場へ寄せる（#2473）。
//!
//! # worker は 3 つの入口が同じ 1 実装を通る（#2473）
//!
//! worker の再開は**再起動をまたぐ GUI の復元**（`sessions::restore_plan`）・`tako sessions resume`・
//! 会話を保った再起動のどれでも [`worker_resume_launch`] → `build_worker_cmd_in`（spawn と同じ関数に
//! 会話 ID を渡す = `WorkerLaunch::resume`）で組む。#2473 の前は GUI の復元だけがカタログのメタで組まれ、
//! 再起動のたびに `--permission-mode auto`（プロファイルの args）・`--remote-control`・プロファイルの env が
//! 落ち、`--model` は `claude agents` が報告した実モデルへ化けていた（隔離 GUI の実測）。
//! 対象の系統は能力マトリクス `resume_launch_args`（claude / codex。codex は `codex resume` が
//! `-m` / `-c` / プロファイルの args を受ける = 0.154.0 の --help）。agy と worker 以外の codex は
//! 従来どおり `sessions::resume_command`（カタログのメタ）を通る。ただし **worker の MCP の断片**
//! （絞り込み #2412 / #2418・tako の MCP の一時注入 #986）は**どの経路でも** spawn と同じ
//! `worker_mcp::prepare_for_resume` + `agent::worker_mcp_args` で載せる（#2447）。

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
    /// カタログのメタだけ（agy・worker 以外の codex・role の無いペイン・`TAKO_1967_LEGACY=1`）
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
    /// worker レジストリに記録した spawn の model / effort（spawn 時に解決済みの値。カタログより優先）。
    /// **`Some` は記録がある = 中の `None` も spawn が決めた値**（`--model` を付けずに CLI の既定へ委ねた）。
    /// カタログの model は `claude agents` が報告した実モデルで上書きされるので、記録があるのに
    /// カタログへ落ちると spawn に無い `--model` が付く（#2473 の実測）。`None` = 記録が無いときだけ
    /// カタログのメタへ落ちる（[`recorded_worker_launch`]）
    pub worker_launch: Option<RecordedLaunch>,
}

/// spawn がレジストリに記録した model / effort（#1967 / #2473）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordedLaunch {
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// レジストリから、この会話（引けなければこのペイン）を spawn したときの model / effort を引く
/// （`tako sessions resume` は会話 ID・会話を保った再起動はペイン・GUI の復元は両方 = 同じ 1 実装）。
/// 同一ペイン番号には世代が堆積するので最後に spawn したものを採る（#466 と同型）
pub fn recorded_worker_launch(
    registry: &crate::orchestrator::registry::WorkerRegistry,
    session_id: Option<&str>,
    pane: Option<u64>,
) -> Option<RecordedLaunch> {
    let latest = |pick: &dyn Fn(&crate::orchestrator::registry::WorkerEntry) -> bool| {
        registry
            .workers
            .values()
            .filter(|e| pick(e))
            .max_by_key(|e| e.spawned_at.clone())
            .map(|e| RecordedLaunch {
                model: e.model.clone(),
                effort: e.effort.clone(),
            })
    };
    session_id
        .and_then(|id| latest(&|e| e.session_id.as_deref() == Some(id)))
        .or_else(|| pane.and_then(|p| latest(&|e| e.pane == p)))
}

/// GUI の復元（再起動をまたぐ）で worker を組むための手掛かり（#2473）。`tako sessions resume` /
/// 会話を保った再起動と同じ材料: 起動した master のプロファイル = ペインの `spawned_by` を辿った
/// master の role（layout.json が role と spawned_by を保つ）、model / effort = レジストリの記録
pub fn restore_hints(
    workspace: &tako_core::Workspace,
    pane: tako_core::PaneId,
    session_id: Option<&str>,
    registry: Option<&crate::orchestrator::registry::WorkerRegistry>,
) -> ResumeHints {
    ResumeHints {
        worker_profile: crate::dispatch::find_master_suffix_from(workspace, pane).map(|s| {
            if s.is_empty() {
                DEFAULT_PROFILE.to_string()
            } else {
                s
            }
        }),
        worker_launch: registry
            .and_then(|reg| recorded_worker_launch(reg, session_id, Some(pane.as_u64()))),
    }
}

/// #2473 の A/B。`TAKO_2473_LEGACY=1` で**同一バイナリのまま**「GUI の復元と codex の worker の再開は
/// カタログのメタだけで組む」#2473 前へ戻す（claude の `tako sessions resume` / 会話を保った再起動は
/// #1967 のまま spawn と同じ組み立て）
pub fn legacy_2473() -> bool {
    std::env::var("TAKO_2473_LEGACY")
        .map(|v| v == "1")
        .unwrap_or(false)
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
    /// worker の MCP の絞り込み（#2412。spawn と同じ判断）。設定ファイルも下見では書かない
    worker_mcp: Option<crate::worker_mcp::WorkerMcp>,
}

impl ResumeLaunch {
    /// 起動に要るファイル（system prompt）を書き出す。`tako master` と同じく毎回書き直すので、
    /// ファイルが消えていても・プロファイルの規則が変わっていても今の内容で起動する
    pub fn materialize(&self) -> Result<(), String> {
        if let Some(mcp) = &self.worker_mcp {
            mcp.materialize()?;
        }
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
    let codex_home = codex_rollout_home(session_id, entry);
    resume_launch_in(
        session_id,
        entry,
        hints,
        location.as_ref(),
        codex_home.as_deref(),
        &crate::dispatch::resolve_tako_binary(),
        crate::launch_cmd::launch_dialect(),
        tako_core::session_restart::legacy_1967(),
    )
}

/// codex の会話なら、rollout が見つかった `CODEX_HOME`（[`pin_codex_home`] の材料）。
/// 見つからない・codex でないなら `None`（寄せない）
pub fn codex_rollout_home(session_id: &str, entry: &SessionEntry) -> Option<PathBuf> {
    if entry.agent.as_deref().and_then(Agent::parse) != Some(Agent::Codex) {
        return None;
    }
    crate::codex_session::find_rollout(session_id)?;
    crate::codex_session::codex_home()
}

/// [`resume_launch`] の本体（会話の所在・tako の実体パス・方言・旧挙動を引数で受ける）。
/// `codex_home` は codex の rollout を見つけた置き場（[`codex_rollout_home`]）
#[allow(clippy::too_many_arguments)]
pub fn resume_launch_in(
    session_id: &str,
    entry: &SessionEntry,
    hints: &ResumeHints,
    location: Option<&TranscriptLocation>,
    codex_home: Option<&Path>,
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
        // #2447: worker なら spawn と同じ MCP の断片（codex は `codex resume -c …`）。
        // 設定ファイルは下見では書かない（実行する側の materialize が書く）
        let worker_mcp = crate::sessions::resume_worker_mcp(agent, session_id, entry);
        let mcp_args = worker_mcp
            .as_ref()
            .map(|(worker, mcp)| crate::sessions::resume_mcp_args(*worker, mcp, tako_bin, dialect))
            .unwrap_or_default();
        let warnings = worker_mcp
            .as_ref()
            .map(|(_, mcp)| mcp.warnings.clone())
            .unwrap_or_default();
        Ok(ResumeLaunch {
            command: crate::sessions::resume_command(session_id, entry, &mcp_args)?,
            recipe: ResumeRecipe::Catalog,
            profile: None,
            warnings,
            prompt_file: None,
            worker_mcp: worker_mcp.map(|(_, mcp)| mcp),
        })
    };
    // 旧挙動の A/B（#1967）
    if legacy {
        return catalog();
    }
    // worker は spawn と同じ組み立て（GUI の復元と同じ 1 実装 = #2473）。対象外の系統（agy）はカタログ
    if let Some(launch) = worker_resume_launch(
        session_id, entry, hints, location, codex_home, tako_bin, dialect,
    ) {
        return launch;
    }
    // claude 以外の master / solo・worker 以外は起動の引数を resume へ持ち込めない（カタログの組み立ては
    // claude の `--effort` 書式 = agent_resume::ResumeSpec の accepts_launch_flags。MCP の断片だけは載せる = #2447）
    if agent != Agent::Claude {
        return catalog();
    }
    let location = location.ok_or_else(|| no_transcript(session_id))?;
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
        // role の無いペイン（手動起動）は元の起動を知らない = カタログのメタが全て
        _ => catalog(),
    }
}

/// claude の会話の記録が見つからないときの理由（利用者へそのまま出す）
fn no_transcript(session_id: &str) -> String {
    format!(
        "会話 {} の記録（claude の transcript）が見つからない。\
         削除されたか、登録されていない設定 dir にある（`tako orchestrator accounts list`）",
        crate::sessions::short_id(session_id)
    )
}

/// worker の再開コマンド（**GUI の復元・`tako sessions resume`・会話を保った再起動が共有する 1 実装**。#2473）。
///
/// spawn と同じ `build_worker_cmd_in` に会話 ID を渡して組む（[`resume_worker_launch`]）。
/// `None` = この会話は対象外 = worker でない・能力マトリクス `resume_launch_args` が使えない系統（agy）・
/// codex で `TAKO_2473_LEGACY=1`。呼び出し側はカタログのメタの組み立てへ落ちる
pub fn worker_resume_launch(
    session_id: &str,
    entry: &SessionEntry,
    hints: &ResumeHints,
    location: Option<&TranscriptLocation>,
    codex_home: Option<&Path>,
    tako_bin: &str,
    dialect: crate::launch_cmd::ShellDialect,
) -> Option<Result<ResumeLaunch, String>> {
    use tako_core::agent_support::{keys, supports};
    if entry.kind != "worker" {
        return None;
    }
    let agent = entry
        .agent
        .as_deref()
        .and_then(Agent::parse)
        .unwrap_or(Agent::Claude);
    if !supports(agent, keys::RESUME_LAUNCH_ARGS) {
        return None;
    }
    // claude は #1967 から。A/B（#2473）で外すのは #2473 で載せた codex だけ
    if agent != Agent::Claude && legacy_2473() {
        return None;
    }
    let worker = WorkerAgent::try_from(agent).ok()?;
    if !crate::transcript::is_valid_session_id(session_id) {
        return Some(Err("session_id の形式が不正".into()));
    }
    Some(resume_worker_launch(
        session_id, worker, entry, hints, location, codex_home, tako_bin, dialect,
    ))
}

/// 突然死からの復旧（`tako orchestrator resume <worker_id>`。#2470）の起動コマンド。
///
/// 材料は worker レジストリの spawn の記録（project / label / cwd / model / effort / 起動した master の
/// プロファイル）で、組み立ては GUI の復元・`tako sessions resume`・会話を保った再起動と同じ
/// [`worker_resume_launch`]（spawn の `build_worker_cmd_in` に会話 ID を渡す）。MCP の絞り込み（#2412 の
/// `--settings`）もここで spawn と同じ判断を通すので、user scope の `.claude.json` を読むのは
/// **打った時点の 1 回だけ**（照会の `registry::resume_command` は読まない）。
/// 設定ファイルは下見では書かないので、実行する側が [`ResumeLaunch::materialize`] を呼ぶ
pub fn registry_resume_launch(
    entry: &crate::orchestrator::registry::WorkerEntry,
) -> Result<ResumeLaunch, String> {
    let location = entry
        .session_id
        .as_deref()
        .and_then(crate::transcript::locate_transcript);
    registry_resume_launch_in(
        entry,
        location.as_ref(),
        &crate::dispatch::resolve_tako_binary(),
        crate::launch_cmd::launch_dialect(),
    )
}

/// [`registry_resume_launch`] の本体（会話の所在・tako の実体パス・方言を引数で受ける）
pub fn registry_resume_launch_in(
    entry: &crate::orchestrator::registry::WorkerEntry,
    location: Option<&TranscriptLocation>,
    tako_bin: &str,
    dialect: crate::launch_cmd::ShellDialect,
) -> Result<ResumeLaunch, String> {
    // 案内（`registry::resume_command`）と同じ条件。codex / agy の突然死からの復旧は能力マトリクスの
    // `worker_death_resume`（#975）
    if entry.agent != "claude" {
        return Err(format!(
            "突然死からの復旧は claude の worker だけ（この worker は {}）",
            if entry.agent.is_empty() {
                "agent 未記録"
            } else {
                entry.agent.as_str()
            }
        ));
    }
    let session_id = entry.session_id.as_deref().ok_or(
        "この worker は会話 ID が未検出なので再開できない（起動直後に落ちた・まだ走査されていない）",
    )?;
    // spawn の記録をカタログのメタの形へ写す（kind = worker なので spawn と同じ組み立てを通る）
    let catalog = SessionEntry {
        kind: "worker".into(),
        agent: Some(entry.agent.clone()),
        project: Some(entry.project.clone()).filter(|p| !p.is_empty()),
        label: entry.label.clone(),
        cwd: entry.cwd.clone(),
        model: entry.model.clone(),
        effort: entry.effort.clone(),
        ..Default::default()
    };
    let hints = ResumeHints {
        worker_profile: entry.profile.clone(),
        // レジストリの記録は spawn が決めた値そのもの（`None` も「--model を付けなかった」）
        worker_launch: Some(RecordedLaunch {
            model: entry.model.clone(),
            effort: entry.effort.clone(),
        }),
    };
    let mut launch = worker_resume_launch(
        session_id, &catalog, &hints, location, None, tako_bin, dialect,
    )
    .ok_or("この worker の系統は spawn と同じ組み立てで再開できない")??;
    if entry.profile.is_none() {
        launch.warnings.insert(
            0,
            "spawn した master のプロファイルの記録が無い（#2470 より前に spawn した worker）ので、\
             既定プロファイルの設定で組んだ"
                .to_string(),
        );
    }
    Ok(launch)
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
        worker_mcp: None,
    })
}

/// worker: spawn と同じ組み立て（起動した master のプロファイル + 記録した model / effort）に
/// 会話 ID を渡す（`WorkerLaunch::resume` = 書式は spawn と同じ関数の中で `resume_spec` から足す。#2473）
#[allow(clippy::too_many_arguments)]
fn resume_worker_launch(
    session_id: &str,
    worker: WorkerAgent,
    entry: &SessionEntry,
    hints: &ResumeHints,
    location: Option<&TranscriptLocation>,
    codex_home: Option<&Path>,
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
    // spawn と同じ env 計画（プロファイルの env + worker のアカウント）。そのうえで設定の置き場だけを
    // **会話の記録の所在**へ寄せる（spawn 時の `--account` は記録に無い / 記録の無い置き場で resume すると落ちる）
    let mut plan = worker_env_plan(&profile, &mut warnings);
    match worker {
        WorkerAgent::Claude => {
            let location = location.ok_or_else(|| no_transcript(session_id))?;
            pin_config_dir(&mut plan, location, &mut warnings);
        }
        WorkerAgent::Codex => {
            if let Some(home) = codex_home {
                pin_codex_home(&mut plan, home, &mut warnings);
            }
        }
        // 能力マトリクス `resume_launch_args` が agy を通さない（worker_resume_launch で弾く）
        WorkerAgent::Agy => {}
    }
    // 記録した値（spawn 時に解決済み）を明示指定として渡す。**記録があれば None も spawn の決めた値**
    // （カタログの model は `claude agents` の実モデルで上書きされるので混ぜない = #2473）。
    // 記録が無いときだけカタログのメタ、それも無ければ spawn と同じ解決
    let (model, effort) = match &hints.worker_launch {
        Some(recorded) => (recorded.model.as_deref(), recorded.effort.as_deref()),
        None => (entry.model.as_deref(), entry.effort.as_deref()),
    };
    let launch = profile.resolve_agent_launch(worker, model, effort);
    let remote_control = orchestrator::remote_control_decision(&profile, worker, &plan);
    // role は spawn と同じ綴り（ラベルが無ければ project だけ）
    let project = entry.project.as_deref().unwrap_or("resumed");
    let role = match entry.label.as_deref() {
        Some(l) => format!("worker:{project}:{l}"),
        None => format!("worker:{project}"),
    };
    // #2412: spawn と同じ絞り込み（プロジェクトの mcp_servers・会話の cwd の .mcp.json）。
    // 再開で user scope の MCP が戻ってくると、spawn で絞った意味が再起動のたびに消える。
    // 判断は再起動をまたぐ復元（#2447）と同じ `prepare_for_resume`。読む置き場は上で会話の所在へ寄せた計画
    let worker_mcp = crate::worker_mcp::prepare_for_resume(
        worker,
        entry.project.as_deref(),
        entry.cwd.as_deref(),
        &plan,
        &launch.extra_args,
    );
    warnings.extend(worker_mcp.warnings.iter().cloned());
    let cmd = orchestrator::agent::build_worker_cmd_in(
        &orchestrator::agent::WorkerLaunch {
            agent: worker,
            role: &role,
            model: launch.model.as_deref(),
            effort: launch.effort.as_deref(),
            skip_permissions: launch.skip_permissions,
            allow_sandbox_bypass: launch.allow_sandbox_bypass,
            remote_control: remote_control.enabled(),
            extra_args: &launch.extra_args,
            tako_bin: Some(tako_bin),
            mcp_settings: worker_mcp.settings_file.as_deref(),
            mcp_codex_off: &worker_mcp.codex_off,
            env: &plan,
            resume: Some(session_id),
        },
        dialect,
    );
    Ok(ResumeLaunch {
        command: cmd,
        recipe: ResumeRecipe::WorkerProfile,
        profile: Some(profile_name),
        warnings,
        prompt_file: None,
        worker_mcp: Some(worker_mcp),
    })
}

/// spawn と同じ env 計画（プロファイルの env + worker のアカウント = `resolved_env_plan_with_account`）。
/// spawn 時の `--account` は記録に無いのでプロファイルの `worker_account`（無ければ `master_account`）で引く。
/// アカウントが引けなくても再開は止めない（プロファイルの env だけで組み、理由を warnings へ）
fn worker_env_plan(profile: &Profile, warnings: &mut Vec<String>) -> EnvPlan {
    let Some(name) = profile.resolve_worker_account_name(None) else {
        return profile.resolved_env_plan();
    };
    match orchestrator::AccountsConfig::load().and_then(|accounts| accounts.resolve(name)) {
        Ok(account) => profile.resolved_env_plan_with_account(Some(&account)),
        Err(e) => {
            warnings.push(format!(
                "worker のアカウント '{name}' を引けないので、プロファイルの env だけで再開した: {e}"
            ));
            profile.resolved_env_plan()
        }
    }
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

/// env 計画の `CODEX_HOME` を **rollout を見つけた置き場**へ寄せる（#2473。claude の [`pin_config_dir`] と
/// 同じ考え方）。計画が `CODEX_HOME` を決めていなければ触らない（spawn も決めていない = 同じ env を継ぐ）。
/// 決めていて食い違うときだけ寄せて warnings に出す（記録の無い置き場で `codex resume` すると会話が無い）
fn pin_codex_home(plan: &mut EnvPlan, home: &Path, warnings: &mut Vec<String>) {
    const KEY: &str = "CODEX_HOME";
    let Some(current) = plan.export_value(KEY).map(PathBuf::from) else {
        return;
    };
    if current == home {
        return;
    }
    warnings.push(format!(
        "プロファイルの CODEX_HOME（{}）に会話の記録が無いので、記録のある置き場（{}）で再開した",
        current.display(),
        home.display()
    ));
    for (k, v) in plan.exports.iter_mut() {
        if k == KEY {
            *v = home.display().to_string();
        }
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
            None,
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
            None,
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
            None,
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
            None,
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
            None,
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
            None,
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
            worker_launch: Some(RecordedLaunch {
                model: Some("claude-opus-5-5".into()),
                effort: Some("high".into()),
            }),
        };
        let got = resume_launch_in(
            ID,
            &entry,
            &hints,
            Some(&location("/tmp/rl1967f", false)),
            None,
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
            None,
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
            None,
            "/opt/tako",
            ShellDialect::Posix,
            false,
        );
        if let Ok(got) = got {
            assert_eq!(got.recipe, ResumeRecipe::Catalog);
        }
    }

    // ---- #2473: worker の再開は spawn と同じ 1 実装 ----

    /// spawn（dispatch の `build_worker_cmd` と同じ材料）で組んだら、という起動コマンド。
    /// 再開の組み立てと**同じ解決**（プロファイル → resolve_agent_launch → remote_control_decision →
    /// prepare_for_resume）を辿り、`resume: None` で組む = 比べる相手
    fn spawn_cmd_for(
        profile_name: &str,
        worker: WorkerAgent,
        role: &str,
        recorded: &RecordedLaunch,
        plan: &EnvPlan,
    ) -> String {
        let profile = Profile::load(profile_name).unwrap();
        let launch = profile.resolve_agent_launch(
            worker,
            recorded.model.as_deref(),
            recorded.effort.as_deref(),
        );
        let rc = orchestrator::remote_control_decision(&profile, worker, plan);
        let mcp = crate::worker_mcp::prepare_for_resume(
            worker,
            Some("tako"),
            None,
            plan,
            &launch.extra_args,
        );
        orchestrator::agent::build_worker_cmd_in(
            &orchestrator::agent::WorkerLaunch {
                agent: worker,
                role,
                model: launch.model.as_deref(),
                effort: launch.effort.as_deref(),
                skip_permissions: launch.skip_permissions,
                allow_sandbox_bypass: launch.allow_sandbox_bypass,
                remote_control: rc.enabled(),
                extra_args: &launch.extra_args,
                tako_bin: Some("/opt/tako"),
                mcp_settings: mcp.settings_file.as_deref(),
                mcp_codex_off: &mcp.codex_off,
                env: plan,
                resume: None,
            },
            ShellDialect::Posix,
        )
    }

    fn worker_entry(agent: &str, label: &str) -> SessionEntry {
        SessionEntry {
            kind: "worker".into(),
            project: Some("tako".into()),
            label: Some(label.into()),
            agent: Some(agent.into()),
            // カタログの model は `claude agents` の実モデルで上書きされうる（記録があれば使わない）
            model: Some("claude-opus-5".into()),
            ..Default::default()
        }
    }

    /// 本番の worker と同じ形のプロファイル（model / effort / `--permission-mode auto` / Remote Control /
    /// env）で、claude の worker の再開は spawn の起動コマンドに `--resume <id>` が付いただけ。
    /// 許可スキップは付かない（プロファイルが opt-in していない = spawn も付けない）
    #[test]
    fn i2473_claude_の_worker_の再開は_spawn_の起動コマンドに_resume_を足しただけ() {
        save_profile(
            "rl2473a",
            "env:\n  T2473_ENV: from-profile\nremote_control: true\nbypass_sandbox: false\n\
             worker_agents:\n  claude:\n    model: claude-opus-5-5\n    effort: xhigh\n    \
             args: [\"--permission-mode\", \"auto\"]\n",
        );
        let recorded = RecordedLaunch {
            model: Some("claude-opus-5-5".into()),
            effort: Some("xhigh".into()),
        };
        let hints = ResumeHints {
            worker_profile: Some("rl2473a".into()),
            worker_launch: Some(recorded.clone()),
        };
        let loc = location("/tmp/rl2473a", false);
        let got = resume_launch_in(
            ID,
            &worker_entry("claude", "cl"),
            &hints,
            Some(&loc),
            None,
            "/opt/tako",
            ShellDialect::Posix,
            false,
        )
        .unwrap();
        assert_eq!(got.recipe, ResumeRecipe::WorkerProfile);
        // spawn は会話の config dir を（アカウント経由で）export する。再開は記録の所在へ寄せる = 同じ値
        let mut plan = Profile::load("rl2473a").unwrap().resolved_env_plan();
        plan.exports.push((
            orchestrator::CLAUDE_CONFIG_DIR_ENV.into(),
            "/tmp/rl2473a".into(),
        ));
        let spawn = spawn_cmd_for(
            "rl2473a",
            WorkerAgent::Claude,
            "worker:tako:cl",
            &recorded,
            &plan,
        );
        assert_eq!(got.command, format!("{spawn} --resume {ID}"));
        for needle in [
            "T2473_ENV=from-profile",
            "--model claude-opus-5-5",
            "--effort xhigh",
            "--permission-mode auto",
        ] {
            assert!(got.command.contains(needle), "{needle}: {}", got.command);
        }
        assert!(
            !got.command.contains("--dangerously-skip-permissions"),
            "{}",
            got.command
        );
        // Remote Control は spawn と同じ判定（opt-in + 適格）で付く
        let rc = orchestrator::remote_control_decision(
            &Profile::load("rl2473a").unwrap(),
            WorkerAgent::Claude,
            &plan,
        );
        assert_eq!(
            got.command.contains("--remote-control"),
            rc.enabled(),
            "{}",
            got.command
        );
    }

    /// codex の worker の再開も spawn と同じ組み立て: `codex resume` の後ろに spawn の引数
    /// （`--model` / `-c model_reasoning_effort=` / tako の MCP / プロファイルの args）がそのまま並ぶ
    #[test]
    fn i2473_codex_の_worker_の再開は_spawn_と同じ引数を_codex_resume_へ渡す() {
        save_profile(
            "rl2473b",
            "bypass_sandbox: false\nworker_agents:\n  codex:\n    model: gpt-5.6-terra\n    \
             effort: medium\n    args: [\"--search\"]\n",
        );
        let recorded = RecordedLaunch {
            model: Some("gpt-5.6-terra".into()),
            effort: Some("medium".into()),
        };
        let hints = ResumeHints {
            worker_profile: Some("rl2473b".into()),
            worker_launch: Some(recorded.clone()),
        };
        let got = resume_launch_in(
            ID,
            &worker_entry("codex", "cx"),
            &hints,
            None,
            None,
            "/opt/tako",
            ShellDialect::Posix,
            false,
        )
        .unwrap();
        assert_eq!(got.recipe, ResumeRecipe::WorkerProfile);
        let spawn = spawn_cmd_for(
            "rl2473b",
            WorkerAgent::Codex,
            "worker:tako:cx",
            &recorded,
            &Profile::load("rl2473b").unwrap().resolved_env_plan(),
        );
        let (head, tail) = spawn
            .split_once(" codex")
            .expect("spawn は codex を起動する");
        assert_eq!(got.command, format!("{head} codex resume{tail} {ID}"));
        for needle in [
            "codex resume --model gpt-5.6-terra -c model_reasoning_effort=medium",
            "mcp_servers.tako.command=",
            " --search ",
        ] {
            assert!(got.command.contains(needle), "{needle}: {}", got.command);
        }
        assert!(
            !got.command
                .contains("--dangerously-bypass-approvals-and-sandbox"),
            "{}",
            got.command
        );
    }

    /// レジストリに記録があれば、`None` の欄も spawn の決めた値（`--model` を付けずに CLI の既定へ委ねた）。
    /// カタログの model（`claude agents` の実モデル）を混ぜない。記録が無いときだけカタログへ落ちる
    #[test]
    fn i2473_記録の_none_は_spawn_の値でカタログの実モデルを混ぜない() {
        save_profile("rl2473c", "bypass_sandbox: false\n");
        let loc = location("/tmp/rl2473c", false);
        let run = |worker_launch: Option<RecordedLaunch>| {
            resume_launch_in(
                ID,
                &worker_entry("claude", "cl"),
                &ResumeHints {
                    worker_profile: Some("rl2473c".into()),
                    worker_launch,
                },
                Some(&loc),
                None,
                "/opt/tako",
                ShellDialect::Posix,
                false,
            )
            .unwrap()
            .command
        };
        let recorded = run(Some(RecordedLaunch {
            model: None,
            effort: Some("max".into()),
        }));
        assert!(!recorded.contains("--model"), "{recorded}");
        assert!(recorded.contains("--effort max"), "{recorded}");
        // 記録が無い（レジストリが読めない・掃除された）ときはカタログのメタ
        let fallback = run(None);
        assert!(fallback.contains("--model claude-opus-5 "), "{fallback}");
    }

    /// 許可スキップは spawn と同じ判断に従う（プロファイルの opt-in が無ければ付かない =
    /// 本番の worker の形。opt-in していれば spawn も再開も同じく付く = 片方だけ外れない）
    #[test]
    fn i2473_許可スキップは_spawn_と同じ判断で_既定では付かない() {
        let loc = location("/tmp/rl2473d", false);
        for (name, yaml, claude_skip, codex_bypass) in [
            (
                "rl2473d-off",
                "bypass_sandbox: false\nworker_agents:\n  claude:\n    args: [\"--permission-mode\", \"auto\"]\n",
                false,
                false,
            ),
            (
                "rl2473d-on",
                "bypass_sandbox: true\nworker_agents:\n  claude:\n    skip_permissions: true\n",
                true,
                true,
            ),
        ] {
            save_profile(name, yaml);
            let hints = ResumeHints {
                worker_profile: Some(name.into()),
                worker_launch: Some(RecordedLaunch::default()),
            };
            let claude = resume_launch_in(
                ID,
                &worker_entry("claude", "cl"),
                &hints,
                Some(&loc),
                None,
                "/opt/tako",
                ShellDialect::Posix,
                false,
            )
            .unwrap()
            .command;
            let codex = resume_launch_in(
                ID,
                &worker_entry("codex", "cx"),
                &hints,
                None,
                None,
                "/opt/tako",
                ShellDialect::Posix,
                false,
            )
            .unwrap()
            .command;
            assert_eq!(
                claude.contains("--dangerously-skip-permissions"),
                claude_skip,
                "{name}: {claude}"
            );
            assert_eq!(
                codex.contains("--dangerously-bypass-approvals-and-sandbox"),
                codex_bypass,
                "{name}: {codex}"
            );
        }
    }

    /// 能力マトリクス `resume_launch_args` が通さない系統（agy）と worker 以外の codex はカタログのメタ
    #[test]
    fn i2473_agy_の_worker_と_codex_の_master_はカタログ() {
        for entry in [
            worker_entry("agy", "ag"),
            SessionEntry {
                kind: "master".into(),
                agent: Some("codex".into()),
                ..Default::default()
            },
        ] {
            assert!(
                worker_resume_launch(
                    ID,
                    &entry,
                    &ResumeHints::default(),
                    None,
                    None,
                    "/opt/tako",
                    ShellDialect::Posix
                )
                .is_none(),
                "{entry:?}"
            );
            let got = resume_launch_in(
                ID,
                &entry,
                &ResumeHints::default(),
                None,
                None,
                "/opt/tako",
                ShellDialect::Posix,
                false,
            )
            .unwrap();
            assert_eq!(got.recipe, ResumeRecipe::Catalog, "{entry:?}");
        }
    }

    /// codex の `CODEX_HOME` は rollout を見つけた置き場へ寄せる（claude の設定 dir と同じ考え方）。
    /// 計画が決めていなければ触らない（spawn も決めていない）
    #[test]
    fn i2473_codex_home_は記録の所在へ寄せる() {
        let mut plan = EnvPlan {
            exports: vec![("CODEX_HOME".into(), "/tmp/a".into())],
            unsets: Vec::new(),
        };
        let mut warnings = Vec::new();
        pin_codex_home(&mut plan, Path::new("/tmp/b"), &mut warnings);
        assert_eq!(plan.export_value("CODEX_HOME"), Some("/tmp/b"));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let mut same = Vec::new();
        pin_codex_home(&mut plan, Path::new("/tmp/b"), &mut same);
        assert!(same.is_empty());
        let mut none = EnvPlan::default();
        pin_codex_home(&mut none, Path::new("/tmp/b"), &mut same);
        assert_eq!(none, EnvPlan::default(), "決めていない計画には足さない");
    }

    /// レジストリの引き方: 会話 ID が先・無ければペインの最後の世代・どちらも無ければ `None`
    #[test]
    fn i2473_レジストリの記録は会話_id_が先でペインは最後の世代() {
        use crate::orchestrator::registry::{WorkerEntry, WorkerRegistry};
        let mut reg = WorkerRegistry::default();
        let mut put = |key: &str, pane: u64, sid: Option<&str>, model: &str, at: &str| {
            reg.workers.insert(
                key.into(),
                WorkerEntry {
                    pane,
                    session_id: sid.map(str::to_string),
                    model: Some(model.into()),
                    spawned_at: at.into(),
                    ..Default::default()
                },
            );
        };
        put("a", 7, Some(ID), "by-session", "2026-10-01T00:00:00Z");
        put("b", 7, None, "pane-old", "2026-10-02T00:00:00Z");
        put("c", 7, None, "pane-new", "2026-10-03T00:00:00Z");
        let model = |sid: Option<&str>, pane: Option<u64>| {
            recorded_worker_launch(&reg, sid, pane).and_then(|r| r.model)
        };
        assert_eq!(model(Some(ID), Some(7)).as_deref(), Some("by-session"));
        assert_eq!(model(None, Some(7)).as_deref(), Some("pane-new"));
        assert_eq!(model(Some("other"), Some(7)).as_deref(), Some("pane-new"));
        assert_eq!(model(Some("other"), None), None);
        assert_eq!(model(None, Some(8)), None);
    }

    /// #2470: 突然死からの復旧（`tako orchestrator resume <worker_id>`）は、レジストリの spawn の記録を
    /// GUI の復元・`tako sessions resume` と同じ 1 実装（`resume_launch_in` の worker の腕）へ渡すだけ。
    /// プロファイルの args（`--permission-mode auto`）・env・role・model / effort・設定 dir が落ちない
    #[test]
    fn i2470_レジストリの記録から_spawn_と同じ組み立てで組む() {
        use crate::orchestrator::registry::WorkerEntry;
        save_profile(
            "rl2470a",
            "env:\n  T2470_ENV: from-profile\nbypass_sandbox: false\n\
             worker_agents:\n  claude:\n    model: claude-opus-5-5\n    effort: xhigh\n    \
             args: [\"--permission-mode\", \"auto\"]\n",
        );
        let entry = WorkerEntry {
            agent: "claude".into(),
            project: "tako".into(),
            label: Some("cl".into()),
            session_id: Some(ID.into()),
            cwd: Some("/tmp/rl2470a-proj".into()),
            model: Some("claude-opus-5-5".into()),
            effort: Some("xhigh".into()),
            profile: Some("rl2470a".into()),
            ..Default::default()
        };
        let loc = location("/tmp/rl2470a", false);
        let got = registry_resume_launch_in(&entry, Some(&loc), "/opt/tako", ShellDialect::Posix)
            .unwrap();
        // 同じ材料で `tako sessions resume` / GUI の復元の入口を通したものと 1 バイトも違わない
        let catalog = SessionEntry {
            kind: "worker".into(),
            agent: Some("claude".into()),
            project: Some("tako".into()),
            label: Some("cl".into()),
            cwd: entry.cwd.clone(),
            model: entry.model.clone(),
            effort: entry.effort.clone(),
            ..Default::default()
        };
        let hints = ResumeHints {
            worker_profile: Some("rl2470a".into()),
            worker_launch: Some(RecordedLaunch {
                model: entry.model.clone(),
                effort: entry.effort.clone(),
            }),
        };
        let via_sessions = resume_launch_in(
            ID,
            &catalog,
            &hints,
            Some(&loc),
            None,
            "/opt/tako",
            ShellDialect::Posix,
            false,
        )
        .unwrap();
        assert_eq!(got.command, via_sessions.command);
        assert_eq!(got.recipe, ResumeRecipe::WorkerProfile);
        assert_eq!(got.profile.as_deref(), Some("rl2470a"));
        for needle in [
            "T2470_ENV=from-profile",
            "TAKO_ORCHESTRATOR_ROLE='worker:tako:cl'",
            "CLAUDE_CONFIG_DIR=/tmp/rl2470a",
            "--model claude-opus-5-5",
            "--effort xhigh",
            "--permission-mode auto",
        ] {
            assert!(got.command.contains(needle), "{needle}: {}", got.command);
        }
        assert!(
            got.command.ends_with(&format!(" --resume {ID}")),
            "{}",
            got.command
        );
        assert!(got.warnings.is_empty(), "{:?}", got.warnings);
    }

    /// #2470 より前の記録（プロファイル名が無い）は既定プロファイルで組み、黙らない。
    /// codex・会話 ID 未検出・会話の記録が無いものは組まずに理由を返す
    #[test]
    fn i2470_記録が足りなければ黙らずに理由を返す() {
        use crate::orchestrator::registry::WorkerEntry;
        let entry = WorkerEntry {
            agent: "claude".into(),
            project: "tako".into(),
            session_id: Some(ID.into()),
            ..Default::default()
        };
        let loc = location("/tmp/rl2470b", true);
        let got = registry_resume_launch_in(&entry, Some(&loc), "/opt/tako", ShellDialect::Posix)
            .unwrap();
        assert_eq!(got.profile.as_deref(), Some("default"));
        assert!(
            got.warnings[0].contains("プロファイルの記録が無い"),
            "{:?}",
            got.warnings
        );
        // 会話が既定の設定 dir にあれば、親の env の CLAUDE_CONFIG_DIR を外して起動する（#652）
        assert!(
            got.command.contains("unset CLAUDE_CONFIG_DIR"),
            "{}",
            got.command
        );

        let codex = WorkerEntry {
            agent: "codex".into(),
            ..entry.clone()
        };
        let err = registry_resume_launch_in(&codex, Some(&loc), "/opt/tako", ShellDialect::Posix)
            .unwrap_err();
        assert!(err.contains("claude の worker だけ"), "{err}");
        let no_sid = WorkerEntry {
            session_id: None,
            ..entry.clone()
        };
        let err = registry_resume_launch_in(&no_sid, Some(&loc), "/opt/tako", ShellDialect::Posix)
            .unwrap_err();
        assert!(err.contains("会話 ID が未検出"), "{err}");
        let err =
            registry_resume_launch_in(&entry, None, "/opt/tako", ShellDialect::Posix).unwrap_err();
        assert!(err.contains("記録"), "{err}");
    }
}
