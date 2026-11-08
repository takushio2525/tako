//! setup_undo — tako が外へ書いた連携を外し、データは残す（`tako setup undo`。Issue #2788）
//!
//! tako を消した後に残ると害になるもの（消えた tako を指す MCP 登録 = agent の起動のたびに MCP の
//! 接続に失敗する、など）を、tako を消す**前**に 1 回で外す。Windows のアンインストーラー
//! （`installer/windows/tako.iss` の `usUninstall`）がこれを呼び、macOS は `brew uninstall` の前に
//! 利用者が打つ（cask の uninstall の段は `brew upgrade` でも走るので使わない。FR-2.14.20）。
//!
//! | 外すもの | 置き場 | 外す条件 |
//! |---|---|---|
//! | MCP 登録（claude） | 設定 dir ごとの `.claude.json`（既定は `~/.claude.json`）・旧 `settings.json` | 名前が tako で command が tako の実行ファイル |
//! | MCP 登録（codex） | `$CODEX_HOME/config.toml` の `[mcp_servers.tako]` | 同上 |
//! | MCP 登録（agy） | `~/.gemini/config/mcp_config.json` | 同上 |
//! | 指示ファイルの同期ブロック | `~/.claude/CLAUDE.md` / `~/.codex/AGENTS.md` / `~/.gemini/GEMINI.md` | 始まりと終わりの印の間（`tako agents sync-rules` が書いたもの） |
//! | tako mod の skills | 設定 dir ごとの `skills/tako` | 印つきの写し（`tako mod uninstall` と同じ 1 実装） |
//! | tako CLI の symlink（unix） | `~/.local/bin/tako` | symlink だけ |
//! | PATH のブロック（unix） | `.zprofile` ほか | 印の間。通している置き場に他のコマンドが残るなら残す |
//! | ユーザー環境変数 Path（Windows） | `HKCU\Environment\Path` | tako を入れた先のエントリだけ |
//! | 以前の版のシェル統合（Windows） | `$PROFILE` | 印の間（#2432 より前の版が書いたもの） |
//! | WebView2 のキャッシュ（Windows） | `%LOCALAPPDATA%\tako\WebView2`・入れた先の `tako-app.exe.WebView2` | データではない |
//!
//! **data dir（設定・レイアウト・ログ・会話のカタログ）は 1 バイトも変えない**（利用者の判断
//! 2026-11-08）。書き先が data dir の下なら断り（[`KeepReason::InsideDataDir`]）、persist.log にも
//! 書かない。利用者が自分で書いた本文・tako 以外の MCP 登録・手の入った mod の写しには触れない。
//!
//! 冪等で、1 つ失敗しても残りは進め、何を外して何を残したかを [`Report`] で返す。
//! 置き場は [`Targets`] に集めて渡す（製品は [`Targets::from_process`]、テストは砂場）。
//! 検証プロセス（テスト・`TAKO_ISOLATED`・セルフテスト）は一時 dir の外へ書かない
//! （`claude_mod_install::write_guard` と同じ番人）。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tako_core::claude_mod_install as mod_core;
use tako_core::i18n::Lang;
use tako_core::platform::user_path::{self, UserPathValue};
use tako_core::shell_profile::{self, ShellKind};

use crate::agent_mcp::{self, Unregister};
use crate::claude_mod_install::{self as mod_install, Source, Target};
use crate::SetupRefusal;

/// 片付けの 1 項目の種類（応答の `kind`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Mcp,
    Instructions,
    ModSkills,
    CliLink,
    PathBlock,
    UserPath,
    ShellIntegration,
    Cache,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Mcp => "mcp",
            Kind::Instructions => "instructions",
            Kind::ModSkills => "mod_skills",
            Kind::CliLink => "cli_link",
            Kind::PathBlock => "path_block",
            Kind::UserPath => "user_path",
            Kind::ShellIntegration => "shell_integration",
            Kind::Cache => "cache",
        }
    }
}

/// 外さずに残した理由
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeepReason {
    /// 名前は tako だが tako の実行ファイルを指していない
    NotTakoCommand { command: String },
    /// 読めない（OS の読み込みの誤り・UTF-8 でない）
    Unreadable { error: String },
    /// JSON / TOML として読めない（位置だけ。中身は引用しない = #2230）
    Broken { line: usize, column: usize },
    /// 同期ブロックの始まりの印だけがあって終わりの印が無い
    UnterminatedBlock,
    /// codex の登録が見出しの表の形でない
    UnsupportedShape,
    /// PATH のブロックが通している置き場に、まだ他のコマンドがある
    DirStillUsed { dirs: Vec<String> },
    /// tako が置いたものではない（手の入った mod の写し・symlink でない tako CLI）
    NotOurs,
    /// data dir の下（守る）
    InsideDataDir,
    /// 検証プロセスで一時 dir の外（書かない）
    OutsideSandbox,
}

impl KeepReason {
    pub fn code(&self) -> &'static str {
        match self {
            KeepReason::NotTakoCommand { .. } => "not_tako_command",
            KeepReason::Unreadable { .. } => "unreadable",
            KeepReason::Broken { .. } => "broken",
            KeepReason::UnterminatedBlock => "unterminated_block",
            KeepReason::UnsupportedShape => "unsupported_shape",
            KeepReason::DirStillUsed { .. } => "dir_still_used",
            KeepReason::NotOurs => "not_ours",
            KeepReason::InsideDataDir => "inside_data_dir",
            KeepReason::OutsideSandbox => "outside_sandbox",
        }
    }
}

/// 1 項目の結末
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// 外した
    Removed,
    /// 外す（dry-run。まだ何も変えていない）
    WouldRemove,
    /// もともと無い
    Absent,
    /// 外さずに残した
    Kept(KeepReason),
    /// 外そうとして失敗した（OS のエラー文）
    Failed(String),
}

impl Outcome {
    pub fn code(&self) -> &'static str {
        match self {
            Outcome::Removed => "removed",
            Outcome::WouldRemove => "would_remove",
            Outcome::Absent => "absent",
            Outcome::Kept(_) => "kept",
            Outcome::Failed(_) => "failed",
        }
    }
}

/// 片付けの 1 項目
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub kind: Kind,
    /// 対象の agent（MCP 登録・指示ファイル）
    pub agent: Option<&'static str>,
    /// 置き場（ファイル・dir。Windows の Path は `HKCU\Environment\Path: <dir>`）
    pub path: String,
    pub outcome: Outcome,
}

/// Windows のユーザー環境変数 Path の読み書き（製品はレジストリ・テストは偽物）
pub trait UserPathStore {
    fn read(&self) -> Result<UserPathValue, String>;
    fn write(&self, value: &UserPathValue) -> Result<(), String>;
}

/// 本物のユーザー環境変数 Path（境界 B23）
struct RegistryStore;

impl UserPathStore for RegistryStore {
    fn read(&self) -> Result<UserPathValue, String> {
        user_path::read()
    }
    fn write(&self, value: &UserPathValue) -> Result<(), String> {
        user_path::write(value)
    }
}

/// Windows の Path から外す tako の入れた先
pub struct UserPathTarget {
    pub tako_dir: PathBuf,
    pub store: Box<dyn UserPathStore>,
}

/// 片付けが見る置き場。製品は [`Targets::from_process`]、テストは砂場の値を直に入れる
#[derive(Default)]
pub struct Targets {
    /// 守る置き場（data dir）。この下には一切書かない
    pub protected: Vec<PathBuf>,
    /// 検証プロセスか（一時 dir の外へ書かない）
    pub verification: bool,
    /// claude の `.claude.json`（設定 dir ごと）
    pub claude_json: Vec<PathBuf>,
    /// 以前の版が MCP 登録を書いた claude の `settings.json`
    pub claude_settings: Vec<PathBuf>,
    /// tako mod の skills を探す claude の設定 dir
    pub claude_config_dirs: Vec<Target>,
    pub codex_config: Option<PathBuf>,
    pub agy_config: Option<PathBuf>,
    /// (agent, 指示ファイル)
    pub instructions: Vec<(&'static str, PathBuf)>,
    /// tako CLI の symlink（unix）
    pub cli_link: Option<PathBuf>,
    /// tako の PATH ブロックが入りうる profile（unix）
    pub profiles: Vec<(ShellKind, PathBuf)>,
    /// PATH のブロックが通している置き場（unix。他のコマンドが残るならブロックを残す）
    pub path_dirs: Vec<PathBuf>,
    /// Windows のユーザー環境変数 Path
    pub user_path: Option<UserPathTarget>,
    /// 以前の版がシェル統合のブロックを書いた `$PROFILE`（Windows）
    pub ps_profiles: Vec<PathBuf>,
    /// 消すキャッシュの dir（Windows の WebView2）
    pub caches: Vec<PathBuf>,
    /// キャッシュを消した後、空なら消す親（`%LOCALAPPDATA%\tako`）
    pub cache_parent: Option<PathBuf>,
}

impl Targets {
    /// このプロセスから見た置き場（CLI・dispatch・アンインストーラーが通る 1 本）
    pub fn from_process() -> Result<Self, SetupRefusal> {
        let home = tako_core::paths::home_dir().ok_or(SetupRefusal::BootstrapHomeUnresolved)?;
        let data_dir = tako_core::paths::data_dir();
        let verification = tako_core::paths::is_verification_process();
        // claude の設定 dir（既定・accounts.yaml・CLAUDE_CONFIG_DIR）= tako mod と同じ集め方
        let claude_config_dirs = mod_install::collect_targets(&mod_install::target_inputs(&[]));
        let mut targets = Targets {
            protected: data_dir.clone().into_iter().collect(),
            verification,
            claude_json: claude_json_paths(&claude_config_dirs),
            claude_settings: claude_config_dirs
                .iter()
                .map(|t| t.dir.join("settings.json"))
                .collect(),
            claude_config_dirs,
            codex_config: agent_mcp::config_path(crate::orchestrator::agent::WorkerAgent::Codex),
            agy_config: agent_mcp::config_path(crate::orchestrator::agent::WorkerAgent::Agy),
            instructions: crate::agents_sync::AgentKind::all()
                .iter()
                .filter_map(|a| Some((a.label(), a.target_path()?)))
                .collect(),
            ..Targets::default()
        };
        let placement = tako_core::tako_cli_path::plan_here(&home);
        if cfg!(windows) {
            // 検証プロセスは本物のレジストリに触らない
            if let Some(p) = placement.filter(|_| !verification) {
                targets.user_path = Some(UserPathTarget {
                    tako_dir: p.dir,
                    store: Box::new(RegistryStore),
                });
            }
            targets.ps_profiles = tako_core::shell_integration::legacy_blocks()
                .into_iter()
                .map(|b| b.path)
                .collect();
            let local = tako_core::paths::local_data_dir();
            if let Some(local) = local.filter(|l| Some(l) != data_dir.as_ref()) {
                targets
                    .caches
                    .push(local.join(tako_core::platform::webview2::DIR_NAME));
                targets.cache_parent = Some(local);
            }
            if let Some(exe_dir) = std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(Path::to_path_buf))
            {
                targets
                    .caches
                    .push(tako_core::platform::webview2::legacy_dir_beside(&exe_dir));
            }
        } else {
            targets.path_dirs = placement.iter().map(|p| p.dir.clone()).collect();
            targets.cli_link = placement.and_then(|p| p.link);
            for agent in crate::setup_bootstrap::bootstrap_agents() {
                let dir = crate::setup_bootstrap::recipe_for(agent).launcher_dir_in(&home);
                if !targets.path_dirs.contains(&dir) {
                    targets.path_dirs.push(dir);
                }
            }
            targets.profiles = shell_profile::PATH_BLOCK_PROFILES
                .iter()
                .map(|(shell, rel)| (*shell, home.join(rel)))
                .collect();
        }
        Ok(targets)
    }
}

/// claude の設定 dir から `.claude.json` の置き場を引く（実測 2026-11-08・claude 2.1.301:
/// `CLAUDE_CONFIG_DIR=X` なら `X/.claude.json`、既定（`~/.claude`）なら `~/.claude.json`）
pub fn claude_json_paths(dirs: &[Target]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    for t in dirs {
        if t.sources.contains(&Source::Default) {
            if let Some(parent) = t.dir.parent() {
                push(parent.join(".claude.json"));
            }
        }
        push(t.dir.join(".claude.json"));
    }
    out
}

/// 片付けの結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub dry_run: bool,
    pub items: Vec<Item>,
    /// 残したデータの置き場（data dir）
    pub kept_data: Option<PathBuf>,
}

/// 片付けを実行する（`dry_run` なら判断だけ）。1 つ失敗しても残りは進める
pub fn run(t: &Targets, dry_run: bool) -> Report {
    let mut items: Vec<Item> = Vec::new();
    // 1. MCP 登録
    let mcp_files = (t.claude_json.iter().chain(&t.claude_settings))
        .map(|p| ("claude", p, FileForm::Json))
        .chain(t.codex_config.iter().map(|p| ("codex", p, FileForm::Toml)))
        .chain(t.agy_config.iter().map(|p| ("agy", p, FileForm::Json)));
    for (agent, path, form) in mcp_files {
        items.push(unregister_file(t, Some(agent), path, form, dry_run));
    }
    // 2. 指示ファイルの同期ブロック
    for (agent, path) in &t.instructions {
        items.push(Item {
            kind: Kind::Instructions,
            agent: Some(agent),
            path: display(path),
            outcome: undo_synced_block(t, path, dry_run),
        });
    }
    // 3. tako mod の skills
    items.extend(undo_mod_skills(t, dry_run));
    // 4. PATH
    if let Some(link) = &t.cli_link {
        items.push(Item {
            kind: Kind::CliLink,
            agent: None,
            path: display(link),
            outcome: undo_cli_link(t, link, dry_run),
        });
    }
    let still_used = dirs_still_used(&t.path_dirs, t.cli_link.as_deref());
    for (shell, profile) in &t.profiles {
        items.push(Item {
            kind: Kind::PathBlock,
            agent: None,
            path: display(profile),
            outcome: undo_path_block(t, *shell, profile, &still_used, dry_run),
        });
    }
    if let Some(up) = &t.user_path {
        items.push(Item {
            kind: Kind::UserPath,
            agent: None,
            path: format!("HKCU\\Environment\\Path: {}", up.tako_dir.display()),
            outcome: undo_user_path(up, dry_run),
        });
    }
    // 5. 以前の版のシェル統合（Windows の $PROFILE）
    for path in &t.ps_profiles {
        items.push(Item {
            kind: Kind::ShellIntegration,
            agent: None,
            path: display(path),
            outcome: undo_ps_profile(t, path, dry_run),
        });
    }
    // 6. キャッシュ
    for dir in &t.caches {
        items.push(Item {
            kind: Kind::Cache,
            agent: None,
            path: display(dir),
            outcome: remove_cache(t, dir, dry_run),
        });
    }
    if let Some(parent) = t.cache_parent.as_deref().filter(|_| !dry_run) {
        let empty = std::fs::read_dir(parent).is_ok_and(|mut d| d.next().is_none());
        if empty && guard(t, parent).is_none() {
            let _ = std::fs::remove_dir(parent);
        }
    }
    Report {
        dry_run,
        items,
        kept_data: t.protected.first().cloned(),
    }
}

fn display(path: &Path) -> String {
    tako_core::paths::shorten_home(&path.display().to_string())
}

/// 書いてよいか（data dir の下・検証プロセスの一時 dir の外は断る）
fn guard(t: &Targets, path: &Path) -> Option<KeepReason> {
    if t.protected.iter().any(|p| path.starts_with(p)) {
        return Some(KeepReason::InsideDataDir);
    }
    if mod_core::write_guard(path, t.verification).is_err() {
        return Some(KeepReason::OutsideSandbox);
    }
    None
}

/// ファイルを読む（無ければ `Ok(None)`）
fn read_text(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read(path) {
        Ok(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| "not UTF-8".to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// 中身を書き換える（その場で書く = symlink を辿り、ファイルの権限を保つ。利用者の dotfiles の
/// symlink を実体のファイルで置き換えない）
fn rewrite(t: &Targets, path: &Path, body: &str, dry_run: bool) -> Outcome {
    if let Some(reason) = guard(t, path) {
        return Outcome::Kept(reason);
    }
    if dry_run {
        return Outcome::WouldRemove;
    }
    match std::fs::write(path, body) {
        Ok(()) => Outcome::Removed,
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

#[derive(Clone, Copy)]
enum FileForm {
    Json,
    Toml,
}

/// 設定ファイルから tako の MCP 登録を外す
fn unregister_file(
    t: &Targets,
    agent: Option<&'static str>,
    path: &Path,
    form: FileForm,
    dry_run: bool,
) -> Item {
    let outcome = match read_text(path) {
        Ok(None) => Outcome::Absent,
        Err(error) => Outcome::Kept(KeepReason::Unreadable { error }),
        Ok(Some(text)) => {
            let plan = match form {
                FileForm::Json => agent_mcp::unregister_json(&text),
                FileForm::Toml => agent_mcp::unregister_toml(&text),
            };
            match plan {
                Unregister::Absent => Outcome::Absent,
                Unregister::Removed(body) => rewrite(t, path, &body, dry_run),
                Unregister::NotTako { command } => {
                    Outcome::Kept(KeepReason::NotTakoCommand { command })
                }
                Unregister::Broken { line, column } => {
                    Outcome::Kept(KeepReason::Broken { line, column })
                }
                Unregister::UnsupportedShape => Outcome::Kept(KeepReason::UnsupportedShape),
            }
        }
    };
    Item {
        kind: Kind::Mcp,
        agent,
        path: display(path),
        outcome,
    }
}

/// 指示ファイルから同期ブロックを外す（ブロックだけのファイル = tako が作ったものは消す）
fn undo_synced_block(t: &Targets, path: &Path, dry_run: bool) -> Outcome {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Outcome::Absent,
        Err(e) => {
            return Outcome::Kept(KeepReason::Unreadable {
                error: e.to_string(),
            })
        }
    };
    let marker = crate::agents_sync::BEGIN_MARKER.as_bytes();
    if !bytes.windows(marker.len()).any(|w| w == marker) {
        return Outcome::Absent;
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return Outcome::Kept(KeepReason::Unreadable {
            error: "not UTF-8".into(),
        });
    };
    match crate::agents_sync::remove_synced_block(&text) {
        Ok(None) => Outcome::Absent,
        Err(crate::agents_sync::UnterminatedBlock) => Outcome::Kept(KeepReason::UnterminatedBlock),
        Ok(Some(rest)) if rest.is_empty() => remove_file(t, path, dry_run),
        Ok(Some(rest)) => rewrite(t, path, &rest, dry_run),
    }
}

fn remove_file(t: &Targets, path: &Path, dry_run: bool) -> Outcome {
    if let Some(reason) = guard(t, path) {
        return Outcome::Kept(reason);
    }
    if dry_run {
        return Outcome::WouldRemove;
    }
    match std::fs::remove_file(path) {
        Ok(()) => Outcome::Removed,
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

/// tako mod の skills（`tako mod uninstall` と同じ 1 実装 = 印つきの写しだけを外す）
fn undo_mod_skills(t: &Targets, dry_run: bool) -> Vec<Item> {
    let ctx = mod_install::Context {
        enabled: true,
        legacy: false,
        claude_version: None,
        verification: t.verification,
    };
    let expected = mod_core::Expected::current();
    let mut items = Vec::new();
    let mut eligible = Vec::new();
    for target in &t.claude_config_dirs {
        if t.protected.iter().any(|p| target.dir.starts_with(p)) {
            items.push(Item {
                kind: Kind::ModSkills,
                agent: Some("claude"),
                path: display(&mod_core::copy_dir(&target.dir)),
                outcome: Outcome::Kept(KeepReason::InsideDataDir),
            });
        } else {
            eligible.push(target.clone());
        }
    }
    for r in mod_install::uninstall_targets(&eligible, &ctx, &expected, dry_run) {
        let outcome = match (r.action, &r.outcome) {
            (_, Err(e)) => Outcome::Failed(e.clone()),
            (mod_core::Action::Remove, Ok(mod_core::Outcome::Planned)) => Outcome::WouldRemove,
            (mod_core::Action::Remove, Ok(_)) => Outcome::Removed,
            _ if r.blocked.is_some()
                && !matches!(r.inspection.copy, mod_core::CopyState::Absent) =>
            {
                Outcome::Kept(KeepReason::OutsideSandbox)
            }
            _ if !matches!(r.inspection.copy, mod_core::CopyState::Absent) => {
                Outcome::Kept(KeepReason::NotOurs)
            }
            _ => Outcome::Absent,
        };
        items.push(Item {
            kind: Kind::ModSkills,
            agent: Some("claude"),
            path: display(&mod_core::copy_dir(&r.target.dir)),
            outcome,
        });
    }
    items
}

/// tako CLI の symlink（symlink でなければ利用者の実体なので残す）
fn undo_cli_link(t: &Targets, link: &Path, dry_run: bool) -> Outcome {
    match std::fs::symlink_metadata(link) {
        Err(_) => Outcome::Absent,
        Ok(meta) if !meta.file_type().is_symlink() => Outcome::Kept(KeepReason::NotOurs),
        Ok(_) => {
            // 番人は symlink の置き場（親）で見る。リンクそのものを渡すと指す先（tako の実体 = 一時 dir の
            // 外）まで辿って「外へ書く」と取り違える。外すのはリンクだけで、指す先には触れない
            if let Some(reason) = guard(t, link.parent().unwrap_or(link)) {
                return Outcome::Kept(reason);
            }
            if dry_run {
                return Outcome::WouldRemove;
            }
            match tako_core::tako_cli_path::remove_link_typed(link) {
                Ok(tako_core::tako_cli_path::LinkChange::Removed) => Outcome::Removed,
                Ok(_) => Outcome::Absent,
                Err(e) => Outcome::Failed(e.text_in(tako_core::i18n::lang())),
            }
        }
    }
}

/// PATH のブロックが通している置き場のうち、tako CLI の symlink 以外がまだ在るもの。
/// ドットで始まる名前（`.DS_Store` 等）は数えない
fn dirs_still_used(dirs: &[PathBuf], cli_link: Option<&Path>) -> Vec<String> {
    let mut used: Vec<String> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let other = entries.flatten().any(|e| {
            let path = e.path();
            Some(path.as_path()) != cli_link && !e.file_name().to_string_lossy().starts_with('.')
        });
        let shown = display(dir);
        if other && !used.contains(&shown) {
            used.push(shown);
        }
    }
    used
}

fn undo_path_block(
    t: &Targets,
    shell: ShellKind,
    profile: &Path,
    still_used: &[String],
    dry_run: bool,
) -> Outcome {
    if !shell_profile::profile_has_block(profile) {
        return Outcome::Absent;
    }
    if !still_used.is_empty() {
        return Outcome::Kept(KeepReason::DirStillUsed {
            dirs: still_used.to_vec(),
        });
    }
    if let Some(reason) = guard(t, profile) {
        return Outcome::Kept(reason);
    }
    if dry_run {
        return Outcome::WouldRemove;
    }
    match shell_profile::remove_block_from_typed(shell, profile) {
        Ok(o) if o.change == shell_profile::PathChange::Removed => Outcome::Removed,
        Ok(_) => Outcome::Absent,
        Err(e) => Outcome::Failed(e.text_in(tako_core::i18n::lang())),
    }
}

/// Windows のユーザー環境変数 Path から tako を入れた先のエントリだけを外す。
/// ランチャーの置き場（`~\.local\bin` 等）は他の道具と共有し、誰が足したかの印も無いので触らない
fn undo_user_path(up: &UserPathTarget, dry_run: bool) -> Outcome {
    let current = match up.store.read() {
        Ok(v) => v,
        Err(error) => return Outcome::Kept(KeepReason::Unreadable { error }),
    };
    let Some(next) = user_path::remove_entry(&current.raw, &up.tako_dir) else {
        return Outcome::Absent;
    };
    if dry_run {
        return Outcome::WouldRemove;
    }
    match up.store.write(&UserPathValue {
        raw: next,
        kind: current.kind,
    }) {
        Ok(()) => Outcome::Removed,
        Err(e) => Outcome::Failed(e),
    }
}

fn undo_ps_profile(t: &Targets, path: &Path, dry_run: bool) -> Outcome {
    let candidate = [(String::new(), path.to_path_buf())];
    if tako_core::shell_integration::legacy_blocks_in(&candidate).is_empty() {
        return Outcome::Absent;
    }
    if let Some(reason) = guard(t, path) {
        return Outcome::Kept(reason);
    }
    if dry_run {
        return Outcome::WouldRemove;
    }
    match tako_core::shell_integration::strip_legacy_block_typed(path, None) {
        Ok(tako_core::shell_integration::ChangeKind::Absent) => Outcome::Absent,
        Ok(_) => Outcome::Removed,
        Err(e) => Outcome::Failed(e.text_in(tako_core::i18n::lang())),
    }
}

fn remove_cache(t: &Targets, dir: &Path, dry_run: bool) -> Outcome {
    if std::fs::symlink_metadata(dir).is_err() {
        return Outcome::Absent;
    }
    if let Some(reason) = guard(t, dir) {
        return Outcome::Kept(reason);
    }
    if dry_run {
        return Outcome::WouldRemove;
    }
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Outcome::Removed,
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

// --- 応答 ---

impl Report {
    fn count(&self, code: &str) -> usize {
        self.items
            .iter()
            .filter(|i| i.outcome.code() == code)
            .count()
    }

    /// 外せなかった項目の数（CLI の終了コード 1）
    pub fn failed(&self) -> usize {
        self.count("failed")
    }

    /// 応答の JSON（CLI `--json` / MCP `tako_setup` の `undo`）。理由の文は `lang`
    pub fn to_json(&self, lang: Lang) -> Value {
        let items: Vec<Value> = self
            .items
            .iter()
            .map(|item| {
                let mut row = json!({
                    "kind": item.kind.as_str(),
                    "path": item.path,
                    "outcome": item.outcome.code(),
                });
                if let Some(agent) = item.agent {
                    row["agent"] = json!(agent);
                }
                match &item.outcome {
                    Outcome::Kept(reason) => {
                        row["reason"] = json!({
                            "code": reason.code(),
                            "message": UndoText::Keep(reason.clone()).text_in(lang),
                        });
                    }
                    Outcome::Failed(error) => row["error"] = json!(error),
                    _ => {}
                }
                row
            })
            .collect();
        json!({
            "dry_run": self.dry_run,
            "summary": {
                "removed": self.count("removed"),
                "would_remove": self.count("would_remove"),
                "absent": self.count("absent"),
                "kept": self.count("kept"),
                "failed": self.failed(),
            },
            "items": items,
            "kept_data": self.kept_data.as_deref().map(display),
            "next": UndoText::Next.text_in(lang),
        })
    }

    /// 人向けの行（CLI。もともと無いものは件数だけ）
    pub fn render_lines(&self, lang: Lang) -> Vec<String> {
        let mut lines = vec![if self.dry_run {
            UndoText::HeaderDryRun.text_in(lang)
        } else {
            UndoText::Header.text_in(lang)
        }];
        for item in &self.items {
            let label = UndoText::Label(item.kind, item.agent).text_in(lang);
            let line = match &item.outcome {
                Outcome::Absent => continue,
                Outcome::Removed => UndoText::LineRemoved.text_in(lang),
                Outcome::WouldRemove => UndoText::LineWouldRemove.text_in(lang),
                Outcome::Kept(_) => UndoText::LineKept.text_in(lang),
                Outcome::Failed(_) => UndoText::LineFailed.text_in(lang),
            };
            lines.push(format!("  {line} {label}: {}", item.path));
            match &item.outcome {
                Outcome::Kept(reason) => lines.push(format!(
                    "      {}",
                    UndoText::Keep(reason.clone()).text_in(lang)
                )),
                Outcome::Failed(error) => lines.push(format!("      {error}")),
                _ => {}
            }
        }
        let absent = self.count("absent");
        if absent > 0 {
            lines.push(format!(
                "  {}",
                UndoText::AbsentCount { count: absent }.text_in(lang)
            ));
        }
        if let Some(data) = &self.kept_data {
            lines.push(
                UndoText::KeptData {
                    path: display(data),
                }
                .text_in(lang),
            );
        }
        lines.push(if self.dry_run {
            UndoText::NextDryRun.text_in(lang)
        } else {
            UndoText::Next.text_in(lang)
        });
        lines
    }
}

/// 利用者へ返す文（#2365。(バリアント, 言語) の網羅 match）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoText {
    Header,
    HeaderDryRun,
    LineRemoved,
    LineWouldRemove,
    LineKept,
    LineFailed,
    /// 項目の名前
    Label(Kind, Option<&'static str>),
    Keep(KeepReason),
    AbsentCount {
        count: usize,
    },
    /// 外せなかった項目の数（CLI の終了コード 1 の文）
    FailedCount {
        count: usize,
    },
    KeptData {
        path: String,
    },
    /// tako 本体の消し方（OS ごと）
    Next,
    NextDryRun,
}

impl UndoText {
    pub fn text_in(&self, lang: Lang) -> String {
        use Lang::{En, Ja};
        let agent = |a: &Option<&'static str>| a.map(|a| format!("（{a}）")).unwrap_or_default();
        let agent_en = |a: &Option<&'static str>| a.map(|a| format!(" ({a})")).unwrap_or_default();
        match (self, lang) {
            (Self::Header, Ja) => "tako の連携を外しました（データは残しています）:".into(),
            (Self::Header, En) => "Removed tako's integrations (your data is kept):".into(),
            (Self::HeaderDryRun, Ja) => {
                "外すもの（--dry-run。まだ何も変えていません）:".into()
            }
            (Self::HeaderDryRun, En) => {
                "What would be removed (--dry-run; nothing has changed yet):".into()
            }
            (Self::LineRemoved, Ja) => "[外した]".into(),
            (Self::LineRemoved, En) => "[removed]".into(),
            (Self::LineWouldRemove, Ja) => "[外す]".into(),
            (Self::LineWouldRemove, En) => "[would remove]".into(),
            (Self::LineKept, Ja) => "[残した]".into(),
            (Self::LineKept, En) => "[kept]".into(),
            (Self::LineFailed, Ja) => "[失敗]".into(),
            (Self::LineFailed, En) => "[failed]".into(),
            (Self::Label(kind, a), Ja) => match kind {
                Kind::Mcp => format!("MCP 登録{}", agent(a)),
                Kind::Instructions => format!("指示ファイルの同期ブロック{}", agent(a)),
                Kind::ModSkills => "tako mod の skills".into(),
                Kind::CliLink => "tako CLI のリンク".into(),
                Kind::PathBlock => "PATH のブロック".into(),
                Kind::UserPath => "ユーザー環境変数 Path".into(),
                Kind::ShellIntegration => "以前の版のシェル統合".into(),
                Kind::Cache => "WebView2 のキャッシュ".into(),
            },
            (Self::Label(kind, a), En) => match kind {
                Kind::Mcp => format!("MCP registration{}", agent_en(a)),
                Kind::Instructions => format!("synced rules block{}", agent_en(a)),
                Kind::ModSkills => "tako mod skills".into(),
                Kind::CliLink => "tako CLI link".into(),
                Kind::PathBlock => "PATH block".into(),
                Kind::UserPath => "user Path variable".into(),
                Kind::ShellIntegration => "shell integration from an older version".into(),
                Kind::Cache => "WebView2 cache".into(),
            },
            (Self::Keep(reason), Ja) => match reason {
                KeepReason::NotTakoCommand { command } => format!(
                    "名前は tako ですが tako の実行ファイルを指していない（{command}）ので残しました"
                ),
                KeepReason::Unreadable { error } => {
                    format!("読めないので触っていません（{error}）")
                }
                KeepReason::Broken { line, column } => {
                    format!("壊れていて読めない（{line} 行 {column} 列）ので触っていません")
                }
                KeepReason::UnterminatedBlock => {
                    "同期ブロックの終わりの印が無く、どこまでが tako のものか分からないので触っていません"
                        .into()
                }
                KeepReason::UnsupportedShape => {
                    "tako の登録が見出しの表（[mcp_servers.tako]）の形でないので触っていません".into()
                }
                KeepReason::DirStillUsed { dirs } => format!(
                    "通している置き場（{}）に他のコマンドが残っているので残しました。要らなければ「# >>> tako PATH >>>」から「# <<< tako PATH <<<」までの行を消してください",
                    dirs.join(", ")
                ),
                KeepReason::NotOurs => {
                    "tako が置いたものではない（手が入っている・利用者のもの）ので残しました".into()
                }
                KeepReason::InsideDataDir => "データの置き場の中なので触っていません".into(),
                KeepReason::OutsideSandbox => {
                    "検証用の起動なので、一時ディレクトリの外には書きません".into()
                }
            },
            (Self::Keep(reason), En) => match reason {
                KeepReason::NotTakoCommand { command } => format!(
                    "kept: it is named tako but does not point to the tako executable ({command})"
                ),
                KeepReason::Unreadable { error } => {
                    format!("left untouched: it cannot be read ({error})")
                }
                KeepReason::Broken { line, column } => {
                    format!("left untouched: it is broken (line {line}, column {column})")
                }
                KeepReason::UnterminatedBlock => {
                    "left untouched: the end marker of the synced block is missing, so tako cannot tell where its part ends"
                        .into()
                }
                KeepReason::UnsupportedShape => {
                    "left untouched: the tako entry is not a [mcp_servers.tako] table".into()
                }
                KeepReason::DirStillUsed { dirs } => format!(
                    "kept: other commands still live in {}. If you do not need it, delete the lines from \"# >>> tako PATH >>>\" to \"# <<< tako PATH <<<\"",
                    dirs.join(", ")
                ),
                KeepReason::NotOurs => {
                    "kept: tako did not put it there (it was edited or is yours)".into()
                }
                KeepReason::InsideDataDir => {
                    "left untouched: it is inside the data directory".into()
                }
                KeepReason::OutsideSandbox => {
                    "left untouched: a verification run does not write outside the temporary directory"
                        .into()
                }
            },
            (Self::AbsentCount { count }, Ja) => {
                format!("もともと無かったもの: {count} か所（変更なし）")
            }
            (Self::AbsentCount { count }, En) => {
                format!("{count} place(s) had nothing to remove (unchanged)")
            }
            (Self::FailedCount { count }, Ja) => {
                format!("{count} か所を外せませんでした（上の [失敗] の行）。直してから tako setup undo をやり直してください")
            }
            (Self::FailedCount { count }, En) => format!(
                "could not remove {count} item(s) (the [failed] lines above). Fix them and run tako setup undo again"
            ),
            (Self::KeptData { path }, Ja) => {
                format!("データ（設定・レイアウト・ログ・会話のカタログ）は残しています: {path}")
            }
            (Self::KeptData { path }, En) => {
                format!("Your data (settings, layout, logs, session catalog) is kept: {path}")
            }
            (Self::Next, Ja) => next_ja().into(),
            (Self::Next, En) => next_en().into(),
            (Self::NextDryRun, Ja) => "外すなら: tako setup undo".into(),
            (Self::NextDryRun, En) => "To remove them: tako setup undo".into(),
        }
    }
}

/// tako 本体の消し方（日本語）
fn next_ja() -> &'static str {
    if cfg!(windows) {
        "tako 本体を消すなら: 設定 → アプリ → インストールされているアプリ → tako → アンインストール"
    } else if cfg!(target_os = "macos") {
        "tako 本体を消すなら: brew uninstall --cask tako（Homebrew で入れた場合）"
    } else {
        "tako 本体は入れたときの方法で消してください"
    }
}

/// tako 本体の消し方（英語）
fn next_en() -> &'static str {
    if cfg!(windows) {
        "To remove tako itself: Settings > Apps > Installed apps > tako > Uninstall"
    } else if cfg!(target_os = "macos") {
        "To remove tako itself: brew uninstall --cask tako (if you installed it with Homebrew)"
    } else {
        "Remove tako itself the same way you installed it"
    }
}
