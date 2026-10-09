//! tako mod を Claude Code の設定 dir へ入れる口（S7-1 #1959。FR-2.42.25〜）
//!
//! 判断と書き込みは `tako_core::claude_mod_install`。ここは「**どの設定 dir を見るか**」を決め、
//! 結果を JSON（`tako mod` / MCP `tako_mod`）・setup の行・persist.log の行に組むだけ。
//!
//! | 発火点 | 呼ぶもの |
//! |---|---|
//! | `tako setup`（MCP `tako_setup` も同じ CLI を通る） | [`run_setup_stage`] |
//! | GUI 起動時（claude の版が分かった後に背景で 1 回） | [`sync_for_gui`] |
//! | GUI 起動後に mod の報告で新しい設定 dir が見えた | [`sync_for_gui`]（報告の dir だけ） |
//! | `tako mod install` / `uninstall`（MCP `tako_mod` の action） | [`run_action`] |
//! | `tako mod`（status） | [`status_json`]（読むだけ） |
//!
//! 置く先は accounts.yaml の設定 dir（`AccountsConfig::list_resolved`）+ 既定（`~/.claude`）+
//! このプロセスの `CLAUDE_CONFIG_DIR` + mod の報告で見えた dir。**検証プロセスでは既定を一時の
//! 置き場へ倒し**（外部エージェントの設定ホームと同じ = `agent_config_home`）、どの出どころの
//! dir でも一時 dir の外へは書かない（`tako_core::claude_mod_install::write_guard`）。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tako_core::claude_mod_install::{
    self as core, Action, Blocked, CopyState, Expected, Inspection, Outcome, State,
};

/// 置く先の出どころ
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// claude の既定（`~/.claude`）
    Default,
    /// accounts.yaml のアカウント（名前）
    Account(String),
    /// このプロセスの `CLAUDE_CONFIG_DIR`
    Env,
    /// mod の報告の `config_dir`（GUI 起動後に見えた dir）
    Reported,
    /// 検証用の差し替え（`TAKO_1959_CONFIG_DIRS`）
    Override,
}

impl Source {
    pub fn label(&self) -> String {
        match self {
            Source::Default => "default".into(),
            Source::Account(name) => format!("account:{name}"),
            Source::Env => "env".into(),
            Source::Reported => "reported".into(),
            Source::Override => "override".into(),
        }
    }
}

/// 置く先 1 つ（同じ dir を指す出どころはまとめる）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub dir: PathBuf,
    pub sources: Vec<Source>,
}

/// [`collect_targets`] の材料（呼び出し側が集めて渡す = 純関数で検査できる）
#[derive(Debug, Clone, Default)]
pub struct TargetInputs {
    /// 検証用の差し替え。`Some` なら既定・アカウント・env を見ない
    pub override_dirs: Option<Vec<PathBuf>>,
    pub default_dir: Option<PathBuf>,
    /// (アカウント名, 設定 dir)。`None` は inherit（= 既定の dir）
    pub accounts: Vec<(String, Option<PathBuf>)>,
    pub process_env: Option<String>,
    pub reported: Vec<String>,
}

/// 比べるための形（末尾の `/` や `.` の表記ゆれを吸収する。在る dir に限らないので
/// canonicalize には頼らない = `is_claude_default_config_dir` と同じ作法）
fn normalized(path: &Path) -> PathBuf {
    path.components().collect()
}

fn push(out: &mut Vec<Target>, dir: PathBuf, source: Source) {
    if dir.as_os_str().is_empty() {
        return;
    }
    let key = normalized(&dir);
    if let Some(found) = out.iter_mut().find(|t| normalized(&t.dir) == key) {
        if !found.sources.contains(&source) {
            found.sources.push(source);
        }
        return;
    }
    out.push(Target {
        dir: key,
        sources: vec![source],
    });
}

/// 置く先を集める（重複をまとめ、出どころを全部残す）
pub fn collect_targets(inputs: &TargetInputs) -> Vec<Target> {
    let mut out = Vec::new();
    match &inputs.override_dirs {
        Some(dirs) => {
            for dir in dirs {
                push(&mut out, dir.clone(), Source::Override);
            }
        }
        None => {
            if let Some(default) = &inputs.default_dir {
                push(&mut out, default.clone(), Source::Default);
            }
            for (name, dir) in &inputs.accounts {
                match (dir, &inputs.default_dir) {
                    (Some(dir), _) => push(&mut out, dir.clone(), Source::Account(name.clone())),
                    (None, Some(default)) => {
                        push(&mut out, default.clone(), Source::Account(name.clone()))
                    }
                    (None, None) => {}
                }
            }
            if let Some(env) = inputs.process_env.as_deref().filter(|v| !v.is_empty()) {
                push(&mut out, PathBuf::from(env), Source::Env);
            }
        }
    }
    for dir in &inputs.reported {
        push(&mut out, PathBuf::from(dir), Source::Reported);
    }
    out
}

/// claude の既定の設定 dir（`$HOME/.claude`）。**検証プロセスでは一時 dir の中に限る**:
/// HOME を一時 dir へ差し替えた検証（スクリプト）はその `~/.claude`、そうでなければ一時の置き場
/// （`agent_config_home` と同じ倒し方）。判定は書き込みの番人と同じ規則（`write_guard`）
pub fn default_config_dir() -> Option<PathBuf> {
    let home_default = crate::orchestrator::claude_default_config_dir();
    if !tako_core::paths::is_verification_process() {
        return home_default;
    }
    match home_default {
        Some(dir) if core::write_guard(&dir, true).is_ok() => Some(dir),
        _ => Some(tako_core::paths::verification_agent_home().join(".claude")),
    }
}

/// このプロセスから見た材料（accounts.yaml・既定・env・差し替え）
pub fn target_inputs(reported: &[String]) -> TargetInputs {
    let accounts = crate::orchestrator::AccountsConfig::load()
        .map(|cfg| {
            cfg.list_resolved()
                .into_iter()
                .filter_map(|(name, resolved)| resolved.ok().map(|r| (name, r)))
                .map(|(name, r)| (name, r.config_dir.path().map(PathBuf::from)))
                .collect()
        })
        .unwrap_or_default();
    TargetInputs {
        override_dirs: core::override_dirs(),
        default_dir: default_config_dir(),
        accounts,
        process_env: std::env::var(crate::orchestrator::CLAUDE_CONFIG_DIR_ENV).ok(),
        reported: reported.to_vec(),
    }
}

/// tako 側の事情（設定・A/B・claude の版・検証プロセスか）
#[derive(Debug, Clone)]
pub struct Context {
    pub enabled: bool,
    pub legacy: bool,
    pub claude_version: Option<String>,
    pub verification: bool,
}

impl Context {
    pub fn from_env(claude_version: Option<String>) -> Self {
        Self {
            enabled: crate::settings::load().claude_mod,
            legacy: core::legacy(),
            claude_version,
            verification: tako_core::paths::is_verification_process(),
        }
    }

    fn blocked(&self, dir: &Path) -> Option<Blocked> {
        core::gate(
            core::write_guard(dir, self.verification),
            self.legacy,
            self.enabled,
            self.claude_version.as_deref(),
        )
    }
}

/// 1 つの設定 dir の結果
#[derive(Debug, Clone)]
pub struct TargetResult {
    pub target: Target,
    pub action: Action,
    pub outcome: Result<Outcome, String>,
    pub blocked: Option<Blocked>,
    /// 実行後（dry-run なら実行前）の中身
    pub inspection: Inspection,
    pub state: State,
}

/// 置く・置き直す・衝突なら退く（setup / GUI 起動時 / `tako mod install`）
pub fn sync_targets(
    targets: &[Target],
    ctx: &Context,
    expected: &Expected,
    dry_run: bool,
) -> Vec<TargetResult> {
    targets
        .iter()
        .map(|target| {
            let blocked = ctx.blocked(&target.dir);
            let before = core::inspect(&target.dir);
            let action = core::plan_sync(&before, &expected.hash, blocked.as_ref());
            let outcome = core::apply(&target.dir, action, expected, dry_run);
            let inspection = if dry_run || action == Action::Keep {
                before
            } else {
                core::inspect(&target.dir)
            };
            let state = core::classify(&inspection, &expected.hash, blocked.as_ref());
            TargetResult {
                target: target.clone(),
                action,
                outcome,
                blocked,
                inspection,
                state,
            }
        })
        .collect()
}

/// 印つきの写しだけを外す（`tako mod uninstall`）。A/B・`tako mod off`・版では止めない
/// （外すのはいつでもできる）。止めるのは検証プロセスの番人だけ
pub fn uninstall_targets(
    targets: &[Target],
    ctx: &Context,
    expected: &Expected,
    dry_run: bool,
) -> Vec<TargetResult> {
    targets
        .iter()
        .map(|target| {
            let guard = core::write_guard(&target.dir, ctx.verification);
            let refused = guard.is_err();
            let before = core::inspect(&target.dir);
            let action = core::plan_uninstall(&before, refused);
            let outcome = core::apply(&target.dir, action, expected, dry_run);
            let inspection = if dry_run || action == Action::Keep {
                before
            } else {
                core::inspect(&target.dir)
            };
            let blocked = guard.err().map(Blocked::Refused);
            let state = core::classify(&inspection, &expected.hash, blocked.as_ref());
            TargetResult {
                target: target.clone(),
                action,
                outcome,
                blocked,
                inspection,
                state,
            }
        })
        .collect()
}

fn short(path: &Path) -> String {
    tako_core::paths::shorten_home(&path.display().to_string())
}

/// 状態の説明（`tako mod` の `reason.message`。installed なら無い）
fn state_message(
    result_state: State,
    inspection: &Inspection,
    blocked: Option<&Blocked>,
) -> Option<String> {
    match result_state {
        State::Installed => None,
        State::Outdated => Some("tako の写しが古い（tako mod install か次の GUI 起動で差し替わる）".into()),
        State::NameConflict => Some(
            inspection
                .conflicts
                .iter()
                .map(|c| c.describe())
                .collect::<Vec<_>>()
                .join(" / "),
        ),
        State::UserDisabled => Some(
            "Claude Code の /plugin で tako@skills-dir が止められている（tako のペインでも休眠する）"
                .into(),
        ),
        State::NotInstalled => Some("まだ入れていない（tako mod install / tako setup で入る）".into()),
        State::NoConfigDir => {
            Some("設定 dir が無い（claude がまだ使っていない。tako は設定 dir を作らない）".into())
        }
        State::ClaudeTooOld
        | State::ClaudeUnknown
        | State::Disabled
        | State::Legacy
        | State::Refused => blocked.map(Blocked::describe),
    }
}

fn target_json(result: &TargetResult) -> Value {
    let inspection = &result.inspection;
    let mut row = json!({
        "config_dir": short(&result.target.dir),
        "path": short(&core::copy_dir(&result.target.dir)),
        "sources": result.target.sources.iter().map(Source::label).collect::<Vec<_>>(),
        "state": result.state.code(),
        "placed": inspection.copy.is_ours(),
        "mark_version": inspection.copy.mark().map(|m| m.tako_version.clone()),
        "user_disabled": inspection.user_disabled,
        "conflicts": inspection
            .conflicts
            .iter()
            .map(|c| json!({
                "code": c.code(),
                "message": c.describe(),
                "blocks_injection": c.blocks_injection(),
            }))
            .collect::<Vec<_>>(),
        "action": result.action.as_str(),
    });
    if let Some(message) = state_message(result.state, inspection, result.blocked.as_ref()) {
        row["reason"] = json!({ "code": result.state.code(), "message": message });
    }
    match &result.outcome {
        Ok(outcome) => row["outcome"] = json!(outcome.as_str()),
        Err(e) => {
            row["outcome"] = json!("error");
            row["error"] = json!(e);
        }
    }
    row
}

fn skills_json(results: &[TargetResult], expected: &Expected) -> Value {
    let count = |state: State| results.iter().filter(|r| r.state == state).count();
    json!({
        "plugin_id": core::SKILLS_PLUGIN_ID,
        "mark_file": core::MARK_FILE,
        "mod_version": expected.version,
        "legacy": core::legacy(),
        "summary": {
            "installed": count(State::Installed),
            "outdated": count(State::Outdated),
            "name_conflict": count(State::NameConflict),
            "user_disabled": count(State::UserDisabled),
            "not_installed": count(State::NotInstalled),
        },
        "targets": results.iter().map(target_json).collect::<Vec<_>>(),
    })
}

/// `tako mod`（status）の `skills`（**読むだけ**。GUI の UI スレッドから呼ぶので書かない）
pub fn status_json(reported: &[String], claude_version: Option<&str>, enabled: bool) -> Value {
    let expected = Expected::current();
    let ctx = Context {
        enabled,
        legacy: core::legacy(),
        claude_version: claude_version.map(str::to_string),
        verification: tako_core::paths::is_verification_process(),
    };
    let targets = collect_targets(&target_inputs(reported));
    skills_json(&sync_targets(&targets, &ctx, &expected, true), &expected)
}

/// `tako mod install|uninstall`（CLI はローカルで、MCP は dispatch から呼ぶ）
pub fn run_action(
    action: &str,
    dry_run: bool,
    reported: &[String],
    ctx: &Context,
) -> Result<Value, String> {
    let expected = Expected::current();
    let targets = collect_targets(&target_inputs(reported));
    let results = match action {
        "install" => sync_targets(&targets, ctx, &expected, dry_run),
        "uninstall" => uninstall_targets(&targets, ctx, &expected, dry_run),
        other => return Err(format!("未知の action: {other}（install / uninstall）")),
    };
    if !dry_run {
        for line in log_lines(&results, None) {
            crate::diag::persist_log(&line);
        }
    }
    let mut out = json!({
        "action": action,
        "dry_run": dry_run,
        "claude_version": ctx.claude_version,
        "min_claude_version": tako_core::claude_mod::MIN_CLAUDE_VERSION,
        "skills": skills_json(&results, &expected),
    });
    out["applies_to"] = json!(match action {
        "install" => "走っている claude にもホットリロードで効く（tako の外の claude では休眠）",
        _ => "走っている claude からも外れる。tako のペインには env の注入（S1）で今までどおり読まれる",
    });
    Ok(out)
}

/// persist.log の行（書いた・外した・失敗・衝突で退いた）。変化の無い dir は書かない
pub fn log_lines(results: &[TargetResult], why: Option<&str>) -> Vec<String> {
    let lead = why.map(|w| format!("（{w}）")).unwrap_or_default();
    results
        .iter()
        .filter_map(|r| {
            let dir = short(&r.target.dir);
            match &r.outcome {
                Ok(Outcome::Written) => Some(format!(
                    "tako mod{lead}: {dir}/skills/tako へ写しを置いた（{} → {}）",
                    core::SKILLS_PLUGIN_ID,
                    r.state.code()
                )),
                Ok(Outcome::Removed) => Some(format!(
                    "tako mod{lead}: {dir}/skills/tako の写しを外した（{}）",
                    state_message(r.state, &r.inspection, r.blocked.as_ref())
                        .unwrap_or_else(|| r.state.code().into())
                )),
                Err(e) => Some(format!("tako mod{lead}: {dir} へ置けない: {e}")),
                Ok(_) if why.is_some() => {
                    Some(format!("tako mod{lead}: {dir} → {}", r.state.code()))
                }
                Ok(_) => None,
            }
        })
        .collect()
}

/// GUI の同期（起動時は `reported` が空 = 基本の置く先すべて。報告で dir が見えたら
/// `only_reported` で**その dir だけ**）。背景のスレッドから呼ぶ。persist.log へ書き、行を返す
pub fn sync_for_gui(reported: &[String], only_reported: bool, ctx: &Context) -> Vec<String> {
    let expected = Expected::current();
    let mut targets = collect_targets(&target_inputs(reported));
    if only_reported {
        targets.retain(|t| t.sources.contains(&Source::Reported));
    }
    let results = sync_targets(&targets, ctx, &expected, false);
    let why = only_reported.then_some("報告で見えた設定 dir");
    let lines = log_lines(&results, why);
    for line in &lines {
        crate::diag::persist_log(line);
    }
    lines
}

/// 新しいペインへ env で注入してよいか（その claude の設定 dir に同名の衝突・利用者の停止が
/// あれば止める理由）。**読むだけ**で、A/B（`TAKO_1959_LEGACY`）では #1959 前と同じく止めない
pub fn injection_block(config_dir: Option<&Path>) -> Option<tako_core::claude_mod::OffReason> {
    if core::legacy() {
        return None;
    }
    core::inspect(config_dir?).injection_block()
}

/// 新しいペインの注入の判断（`ModHub::decide`）に、その claude の設定 dir の衝突・利用者の停止を
/// 重ねる（#1959）。設定 dir はペインの env（worker のアカウント）→ tako 自身の env → 既定の順に
/// 見立てる。既定は**本物の** `~/.claude`（ペインの claude が実際に読む dir。読むだけなので
/// 検証プロセスでも倒さない）
pub fn refine_injection(
    injection: tako_core::claude_mod::Injection,
    pane_env: &[(String, String)],
) -> tako_core::claude_mod::Injection {
    if !injection.is_on() {
        return injection;
    }
    let config_dir = core::pane_config_dir(
        pane_env,
        std::env::var(crate::orchestrator::CLAUDE_CONFIG_DIR_ENV)
            .ok()
            .as_deref(),
        crate::orchestrator::claude_default_config_dir().as_deref(),
    );
    match injection_block(config_dir.as_deref()) {
        Some(reason) => tako_core::claude_mod::Injection::Off(reason),
        None => injection,
    }
}

/// `tako setup` の段。**止めない**（入れられなくても tako は env の注入と画面の読み取りで動く）。
/// 初めて置くときだけ「何をどこへ」を出してから置く（#262 の質問ゼロ。FR-2.14.14 と同じ作法）
pub fn run_setup_stage(claude_version: Option<String>) -> Vec<String> {
    let ctx = Context::from_env(claude_version);
    let expected = Expected::current();
    let targets = collect_targets(&target_inputs(&[]));
    let plan = sync_targets(&targets, &ctx, &expected, true);
    let mut lines = Vec::new();
    let first = plan
        .iter()
        .filter(|r| r.action == Action::Write && matches!(r.inspection.copy, CopyState::Absent))
        .map(|r| short(&core::copy_dir(&r.target.dir)))
        .collect::<Vec<_>>();
    if !first.is_empty() {
        lines.push(
            "  tako mod を Claude Code へ入れます（settings 系のファイルは書きません）:".into(),
        );
        for path in &first {
            lines.push(format!(
                "    置き場: {path}（管理印 {} つきの写し）",
                core::MARK_FILE
            ));
        }
        lines.push(format!(
            "    効くもの: tako のペインの claude の帯・/tako・状態の報告（/plugin に {} として出る。tako の外の claude では休眠）",
            core::SKILLS_PLUGIN_ID
        ));
        lines.push("    戻し方: tako mod uninstall".into());
    }
    let results = sync_targets(&targets, &ctx, &expected, false);
    for line in log_lines(&results, Some("setup")) {
        crate::diag::persist_log(&line);
    }
    // tako 側の事情（版・設定・A/B）で全部止まったなら 1 行にまとめる
    if let Some(blocked) = results.first().and_then(|r| r.blocked.clone()) {
        let same = results.iter().all(|r| r.blocked.as_ref() == Some(&blocked));
        let none_placed = results.iter().all(|r| r.action == Action::Keep);
        if same && none_placed && !matches!(blocked, Blocked::Refused(_)) {
            lines.push(format!(
                "  [skip] tako mod を Claude Code へ入れない: {}（tako のペインは今までどおり env で読ませる）",
                blocked.describe()
            ));
            return lines;
        }
    }
    for r in &results {
        let path = short(&core::copy_dir(&r.target.dir));
        let line = match (&r.outcome, r.state) {
            (Err(e), _) => {
                format!("  [warn] {path} へ入れられない（tako は env の注入で動くので続行）: {e}")
            }
            (Ok(Outcome::Written), _) => format!("  [ok] tako mod を Claude Code へ入れた: {path}"),
            (Ok(Outcome::Removed), _) => format!(
                "  [ok] {path} の tako の写しを外した: {}",
                state_message(r.state, &r.inspection, r.blocked.as_ref()).unwrap_or_default()
            ),
            (Ok(_), State::Installed) => format!("  [ok] tako mod（Claude Code）は最新: {path}"),
            (Ok(_), State::NoConfigDir) => continue,
            (Ok(_), state) => format!(
                "  [skip] {}: {}",
                short(&r.target.dir),
                state_message(state, &r.inspection, r.blocked.as_ref())
                    .unwrap_or_else(|| state.code().into())
            ),
        };
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 使い捨ての設定 dir（Drop で消える = #1312 の 1 実装）
    fn scratch(name: &str) -> tako_core::test_residue::ScratchDir {
        tako_core::test_residue::ScratchDir::new(&format!("mod-install-ctl-{name}"))
    }

    fn ctx() -> Context {
        Context {
            enabled: true,
            legacy: false,
            claude_version: Some("2.1.294".into()),
            verification: true,
        }
    }

    #[test]
    fn 置く先は既定とアカウントとenvと報告をまとめ出どころを全部残す() {
        let inputs = TargetInputs {
            override_dirs: None,
            default_dir: Some(PathBuf::from("/h/.claude")),
            accounts: vec![
                ("work".into(), Some(PathBuf::from("/h/.claude-work/"))),
                ("main".into(), None),
            ],
            process_env: Some("/h/.claude-work".into()),
            reported: vec!["/h/.claude-x".into(), "/h/.claude".into()],
        };
        let targets = collect_targets(&inputs);
        assert_eq!(
            targets,
            vec![
                Target {
                    dir: PathBuf::from("/h/.claude"),
                    sources: vec![
                        Source::Default,
                        Source::Account("main".into()),
                        Source::Reported
                    ],
                },
                Target {
                    dir: PathBuf::from("/h/.claude-work"),
                    sources: vec![Source::Account("work".into()), Source::Env],
                },
                Target {
                    dir: PathBuf::from("/h/.claude-x"),
                    sources: vec![Source::Reported],
                },
            ]
        );
    }

    #[test]
    fn 差し替えがあれば既定とアカウントとenvを見ない() {
        let inputs = TargetInputs {
            override_dirs: Some(vec![PathBuf::from("/t/a"), PathBuf::from("/t/b")]),
            default_dir: Some(PathBuf::from("/h/.claude")),
            accounts: vec![("work".into(), Some(PathBuf::from("/h/.claude-work")))],
            process_env: Some("/h/.claude-univ".into()),
            reported: vec!["/t/c".into()],
        };
        let dirs: Vec<PathBuf> = collect_targets(&inputs)
            .into_iter()
            .map(|t| t.dir)
            .collect();
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/t/a"),
                PathBuf::from("/t/b"),
                PathBuf::from("/t/c")
            ]
        );
    }

    #[test]
    fn 検証プロセスの既定の設定dirは一時の置き場() {
        // cargo test は検証プロセス = 本物の ~/.claude を置く先に入れない
        let dir = default_config_dir().unwrap();
        assert!(
            dir.starts_with(std::env::temp_dir()),
            "検証プロセスの既定が一時 dir の外: {}",
            dir.display()
        );
    }

    #[test]
    fn 置いて外すとjsonに設定dirごとの状態が出る() {
        let a = scratch("a");
        let b = scratch("b");
        // b は利用者の skills/tako を持つ
        std::fs::create_dir_all(b.join("skills/tako")).unwrap();
        let targets = vec![
            Target {
                dir: a.to_path_buf(),
                sources: vec![Source::Override],
            },
            Target {
                dir: b.to_path_buf(),
                sources: vec![Source::Override],
            },
        ];
        let expected = Expected::for_version("9.9.9");
        let results = sync_targets(&targets, &ctx(), &expected, false);
        let json = skills_json(&results, &expected);
        assert_eq!(json["targets"][0]["state"], "installed");
        assert_eq!(json["targets"][0]["outcome"], "written");
        assert_eq!(json["targets"][0]["mark_version"], "9.9.9");
        assert_eq!(json["targets"][1]["state"], "name_conflict");
        assert_eq!(
            json["targets"][1]["conflicts"][0]["code"],
            "user_skills_dir"
        );
        assert_eq!(json["summary"]["installed"], 1);
        let lines = log_lines(&results, None);
        assert_eq!(lines.len(), 1, "変化の無い dir まで書いた: {lines:?}");
        assert!(lines[0].contains("skills/tako へ写しを置いた"));

        let results = uninstall_targets(&targets, &ctx(), &expected, false);
        assert_eq!(results[0].outcome, Ok(Outcome::Removed));
        assert_eq!(results[1].outcome, Ok(Outcome::Unchanged));
        assert!(!a.join("skills/tako").exists());
        assert!(
            b.join("skills/tako").is_dir(),
            "利用者の skills/tako を消した"
        );
    }

    #[test]
    fn 検証プロセスは一時dirの外へ置かず外しもしない() {
        let outside = PathBuf::from(if cfg!(windows) {
            r"C:\Users\testuser\.claude-tako-1959-never"
        } else {
            "/Users/testuser/.claude-tako-1959-never"
        });
        let targets = vec![Target {
            dir: outside.clone(),
            sources: vec![Source::Reported],
        }];
        let expected = Expected::for_version("9.9.9");
        let results = sync_targets(&targets, &ctx(), &expected, false);
        assert_eq!(results[0].action, Action::Keep);
        assert!(matches!(results[0].blocked, Some(Blocked::Refused(_))));
        let results = uninstall_targets(&targets, &ctx(), &expected, false);
        assert_eq!(results[0].action, Action::Keep);
        assert!(!outside.exists());
    }
}
