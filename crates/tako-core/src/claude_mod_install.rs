//! claude_mod_install — tako mod を Claude Code の設定 dir の `skills/tako/` へ入れる
//! （エピック #1877 / S7-1 #1959。FR-2.42.25〜）
//!
//! 設計の正本は `.agent/plans/2026-10-tako-mod.md` §9.4（導入方式）/ §9.5（共存）/ §9.6（落ち方）。
//!
//! - **置き場**: `<設定 dir>/skills/tako/`。Claude Code 2.1.294 はここの plugin を `tako@skills-dir`
//!   として読み、settings 系のファイル（`settings*.json`・`plugins/*.json`）を 1 つも書かない。
//!   走っているセッションにもホットリロードで効き、`/plugin` から利用者が止められる
//! - **写し**（symlink にしない）: 中身は同梱の mod（[`crate::claude_mod::rendered_files`]）。
//!   data dir の展開先を辿らないので、tako を消しても写しは自立したまま休眠する
//!   （`TAKO_PANE_ID` / `TAKO_CLI` が無い claude では何もしない）
//! - **管理印** [`MARK_FILE`]: tako が置いた印・版・内容のハッシュ・置いたファイルの一覧。
//!   **印の無い `skills/tako` は利用者のもの**で触らない。印があっても中身が印のハッシュと
//!   違えば、利用者が手を入れたものとして触らない
//! - **同名の衝突**: 別の出どころの `tako`（`skills/<別名>/` の plugin.json・
//!   `plugins/installed_plugins.json`・settings の `enabledPlugins`）があれば、その設定 dir には
//!   置かない（**ファイルを読むだけ**）。Claude Code は同名の plugin を先に来た方だけ黙って読むので、
//!   置くと利用者の `tako` を潰しうる。既に置いた写しは退く（消す）
//! - **差し替え**: `skills/.tako.tmp-*` に全部書いてから rename で入れ替える。dot で始まる dir を
//!   Claude Code は plugin として読まない（2.1.294 で実測）ので、書きかけ・落ちた残骸が
//!   2 つ目の `tako` に見えることは無い。Claude Code が書く型定義（`.claude-plugin/types/`）は
//!   比較から外して引き継ぐ
//! - **番人**: 検証プロセス（テスト・`TAKO_ISOLATED`・セルフテスト）は一時 dir の外へ書かない
//!   （[`write_guard`]）。本物の `~/.claude*` へ検証が写しを置くと、走っている全セッションに効く
//!
//! 判断（[`classify`] / [`plan_sync`] / [`plan_uninstall`]）は純関数、ファイルの読み書きは
//! 明示したパスだけ。どの設定 dir を見るか（accounts.yaml・既定・mod の報告）は
//! `tako_control::claude_mod_install` が決める。

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::claude_mod::{self, ENGINE_TYPES_DIR, LIST_SEP, MOD_NAME};

/// 管理印のファイル名（`skills/tako/.tako-managed`）
pub const MARK_FILE: &str = ".tako-managed";
/// 管理印の書式の版
pub const MARK_SCHEMA: u32 = 1;
/// 管理印の `managed_by`
pub const MANAGED_BY: &str = "tako";
/// 設定 dir の中の置き場の親
pub const SKILLS_DIR: &str = "skills";
/// Claude Code が skills-dir の plugin に付ける id
pub const SKILLS_PLUGIN_ID: &str = "tako@skills-dir";
/// env（`CLAUDE_CODE_PLUGIN_DIRS`）から読まれた plugin の id
pub const INLINE_PLUGIN_ID: &str = "tako@inline";
/// skills-dir への導入を止める A/B の入口（#1959 前 = env の注入だけ・衝突でも注入を止めない）
pub const LEGACY_ENV: &str = "TAKO_1959_LEGACY";
/// 検証用: 置く先をこの一覧（区切りは [`LIST_SEP`]）に差し替える。accounts.yaml・既定・
/// プロセスの `CLAUDE_CONFIG_DIR` を見ない（mod の報告で見えた dir は足す）
pub const CONFIG_DIRS_ENV: &str = "TAKO_1959_CONFIG_DIRS";
/// 検証用: 差し替えの途中でプロセスを落とす（`staged` = 一時 dir を書いた後 /
/// `retired` = 旧い写しを退避した後）。書き込みの途中で落ちても壊れないことを実プロセスで確かめる
pub const INJECT_CRASH_ENV: &str = "TAKO_1959_INJECT_CRASH";
/// [`INJECT_CRASH_ENV`] で落ちるときの終了コード
pub const INJECT_CRASH_EXIT: i32 = 86;
/// 検証用: 置く写しの版を差し替える（**検証プロセスだけ**で効く = [`Expected::current`]）
pub const MOD_VERSION_ENV: &str = "TAKO_1959_MOD_VERSION";
/// 落ちたプロセスの一時 dir・退避を片付けるまでの猶予（並行して書いている別プロセスに触らない）
const LEFTOVER_AGE: Duration = Duration::from_secs(600);

/// 管理印（`skills/tako/.tako-managed` の JSON）。**在ること自体が「tako が置いた」の宣言**
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedMark {
    /// 印の書式の版（[`MARK_SCHEMA`]）
    pub schema: u32,
    /// 置いた者（常に [`MANAGED_BY`]）
    pub managed_by: String,
    /// 置いた tako の版
    pub tako_version: String,
    /// 置いたファイルの中身のハッシュ（[`content_hash`]）
    pub content_hash: String,
    /// 置いたファイル（相対パス）。中身が印と同じかはこの一覧だけで見る
    /// （Claude Code が書く型定義は入らない）
    pub files: Vec<String>,
}

/// 置いたファイルの中身のハッシュ（相対パスの順に並べた「パス・長さ・中身」の FNV-1a 64bit）
pub fn content_hash<'a>(files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> String {
    let mut sorted: Vec<(&str, &[u8])> = files.into_iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let mut buf = Vec::new();
    for (rel, body) in sorted {
        buf.extend_from_slice(rel.as_bytes());
        buf.push(0);
        buf.extend_from_slice(body.len().to_string().as_bytes());
        buf.push(0);
        buf.extend_from_slice(body);
    }
    format!("{:016x}", crate::fnv::fnv1a64(&buf))
}

/// いまの tako が置く写し（版・ファイル・ハッシュ）
#[derive(Debug, Clone)]
pub struct Expected {
    pub version: String,
    pub files: Vec<(&'static str, String)>,
    pub hash: String,
}

impl Expected {
    pub fn for_version(version: &str) -> Self {
        let files = claude_mod::rendered_files(version);
        let hash = content_hash(files.iter().map(|(rel, body)| (*rel, body.as_bytes())));
        Self {
            version: version.to_string(),
            files,
            hash,
        }
    }

    /// 実行中の tako の版で置く写し。検証プロセスだけは [`MOD_VERSION_ENV`] で版を差し替えられる
    /// （tako の版の更新・旧版へ戻したときの差し替えを 1 つのバイナリで確かめる）
    pub fn current() -> Self {
        let swapped = std::env::var(MOD_VERSION_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty() && crate::paths::is_verification_process());
        Self::for_version(swapped.as_deref().unwrap_or(env!("CARGO_PKG_VERSION")))
    }

    pub fn mark(&self) -> ManagedMark {
        ManagedMark {
            schema: MARK_SCHEMA,
            managed_by: MANAGED_BY.to_string(),
            tako_version: self.version.clone(),
            content_hash: self.hash.clone(),
            files: self.files.iter().map(|(rel, _)| rel.to_string()).collect(),
        }
    }
}

/// `<設定 dir>/skills/tako`
pub fn copy_dir(config_dir: &Path) -> PathBuf {
    config_dir.join(SKILLS_DIR).join(MOD_NAME)
}

// --- 読み取り（書かない）--------------------------------------------------------

/// `skills/tako` に何があるか
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyState {
    /// 無い
    Absent,
    /// tako が置いた写し（印が読めて、中身が印のハッシュと一致）
    Managed(ManagedMark),
    /// 印は読めるが中身が印と違う = 置いた後に誰かが手を入れた（触らない）
    Modified(ManagedMark),
    /// 印が壊れている（読めない・形が違う）。印の名前は tako しか使わないので、
    /// tako の写しとして置き直す・外す
    MarkUnreadable,
    /// 印が無い = 利用者のもの（触らない）
    Unmanaged,
    /// symlink（辿って書かない = 利用者のもの）
    Symlink,
}

impl CopyState {
    /// tako が置き直し・外してよい写しか
    pub fn is_ours(&self) -> bool {
        matches!(self, CopyState::Managed(_) | CopyState::MarkUnreadable)
    }

    pub fn mark(&self) -> Option<&ManagedMark> {
        match self {
            CopyState::Managed(m) | CopyState::Modified(m) => Some(m),
            _ => None,
        }
    }
}

/// 印を読む（`managed_by` が tako で、ファイル一覧が設定 dir の外を指さないものだけ）
pub fn parse_mark(text: &str) -> Option<ManagedMark> {
    let mark: ManagedMark = serde_json::from_str(text).ok()?;
    let sane = |rel: &String| {
        let path = Path::new(rel);
        !rel.is_empty()
            && path.is_relative()
            && path
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
    };
    (mark.managed_by == MANAGED_BY && mark.schema >= 1 && mark.files.iter().all(sane))
        .then_some(mark)
}

/// 印のファイル一覧の今の中身のハッシュ（読めないファイルがあれば `None`）
fn disk_hash(dest: &Path, files: &[String]) -> Option<String> {
    let mut bodies = Vec::new();
    for rel in files {
        bodies.push((rel.as_str(), std::fs::read(dest.join(rel)).ok()?));
    }
    Some(content_hash(
        bodies.iter().map(|(rel, body)| (*rel, body.as_slice())),
    ))
}

pub fn read_copy(dest: &Path) -> CopyState {
    let meta = match std::fs::symlink_metadata(dest) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return CopyState::Absent,
        // 読めない（権限等）ものは利用者のものとして触らない
        Err(_) => return CopyState::Unmanaged,
    };
    if meta.file_type().is_symlink() {
        return CopyState::Symlink;
    }
    if !meta.is_dir() {
        return CopyState::Unmanaged;
    }
    let text = match std::fs::read_to_string(dest.join(MARK_FILE)) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return CopyState::Unmanaged,
        Err(_) => return CopyState::MarkUnreadable,
    };
    let Some(mark) = parse_mark(&text) else {
        return CopyState::MarkUnreadable;
    };
    if disk_hash(dest, &mark.files).as_deref() == Some(mark.content_hash.as_str()) {
        CopyState::Managed(mark)
    } else {
        CopyState::Modified(mark)
    }
}

/// 置けない理由（同名の衝突・利用者のもの）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conflict {
    /// `skills/tako` が利用者のもの（印が無い・symlink）。`named_tako` = そこの plugin が
    /// `tako` を名乗っている（env の注入も止める理由になる）
    UserSkillsDir { named_tako: bool },
    /// tako の写しに利用者が手を入れた
    ModifiedCopy,
    /// 別の `skills/<名前>/` が plugin 名 `tako` を名乗っている
    SkillsSameName { dir: String },
    /// `plugins/installed_plugins.json` に `tako@<ほか>` が居る
    InstalledPlugin { id: String },
    /// settings.json の `enabledPlugins` に `tako@<ほか>`（有効）が居る
    EnabledPlugin { id: String },
}

impl Conflict {
    pub fn code(&self) -> &'static str {
        match self {
            Conflict::UserSkillsDir { .. } => "user_skills_dir",
            Conflict::ModifiedCopy => "modified_copy",
            Conflict::SkillsSameName { .. } => "skills_same_name",
            Conflict::InstalledPlugin { .. } => "installed_plugin",
            Conflict::EnabledPlugin { .. } => "enabled_plugin",
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Conflict::UserSkillsDir { named_tako: true } => {
                "skills/tako は利用者の plugin（tako の管理印が無い）なので触らない".into()
            }
            Conflict::UserSkillsDir { named_tako: false } => {
                "skills/tako は利用者のもの（tako の管理印が無い）なので触らない".into()
            }
            Conflict::ModifiedCopy => {
                "skills/tako の写しに手が入っている（管理印のハッシュと違う）ので上書きしない"
                    .into()
            }
            Conflict::SkillsSameName { dir } => {
                format!("skills/{dir} が同じ名前の plugin tako を名乗っている")
            }
            Conflict::InstalledPlugin { id } => {
                format!("同じ名前の plugin {id} が入っている（plugins/installed_plugins.json）")
            }
            Conflict::EnabledPlugin { id } => {
                format!(
                    "同じ名前の plugin {id} が有効になっている（settings.json の enabledPlugins）"
                )
            }
        }
    }

    /// その設定 dir の claude へ env（inline）で tako を読ませると、利用者の `tako` を潰すか。
    /// Claude Code は inline を installed / skills-dir より先に読む（同名なら inline だけが読まれる）
    pub fn blocks_injection(&self) -> bool {
        !matches!(self, Conflict::UserSkillsDir { named_tako: false })
    }
}

/// 1 つの設定 dir を読んだ結果（**書かない**）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inspection {
    pub config_dir_exists: bool,
    pub copy: CopyState,
    pub conflicts: Vec<Conflict>,
    /// 利用者が Claude Code 側で止めた（settings.json の `enabledPlugins["tako@skills-dir"] === false`）
    pub user_disabled: bool,
}

impl Inspection {
    /// env の注入を止める理由（衝突 > 利用者の停止）。`None` = 注入してよい
    pub fn injection_block(&self) -> Option<claude_mod::OffReason> {
        if let Some(conflict) = self.conflicts.iter().find(|c| c.blocks_injection()) {
            return Some(claude_mod::OffReason::NameConflict(
                conflict.describe().into_boxed_str(),
            ));
        }
        self.user_disabled
            .then_some(claude_mod::OffReason::UserDisabled)
    }
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// plugin.json の `name`（読めなければ `None`）
fn plugin_name(dir: &Path) -> Option<String> {
    read_json(&dir.join(".claude-plugin").join("plugin.json"))?
        .get("name")?
        .as_str()
        .map(str::to_string)
}

/// `tako@<ほか>` か（skills-dir と inline は tako 自身の読まれ方）
fn foreign_tako_id(id: &str) -> bool {
    id.starts_with("tako@") && id != SKILLS_PLUGIN_ID && id != INLINE_PLUGIN_ID
}

pub fn inspect(config_dir: &Path) -> Inspection {
    let config_dir_exists = config_dir.is_dir();
    let dest = copy_dir(config_dir);
    let copy = if config_dir_exists {
        read_copy(&dest)
    } else {
        CopyState::Absent
    };
    let mut conflicts = Vec::new();
    match &copy {
        CopyState::Unmanaged | CopyState::Symlink => conflicts.push(Conflict::UserSkillsDir {
            named_tako: plugin_name(&dest).as_deref() == Some(MOD_NAME),
        }),
        CopyState::Modified(_) => conflicts.push(Conflict::ModifiedCopy),
        _ => {}
    }
    if let Ok(entries) = std::fs::read_dir(config_dir.join(SKILLS_DIR)) {
        let mut same: Vec<String> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                (!name.starts_with('.') && name != MOD_NAME).then_some((name, entry.path()))
            })
            .filter(|(_, path)| plugin_name(path).as_deref() == Some(MOD_NAME))
            .map(|(name, _)| name)
            .collect();
        same.sort();
        conflicts.extend(same.into_iter().map(|dir| Conflict::SkillsSameName { dir }));
    }
    if let Some(installed) = read_json(&config_dir.join("plugins").join("installed_plugins.json")) {
        if let Some(plugins) = installed.get("plugins").and_then(|p| p.as_object()) {
            conflicts.extend(
                plugins
                    .keys()
                    .filter(|id| foreign_tako_id(id))
                    .map(|id| Conflict::InstalledPlugin { id: id.clone() }),
            );
        }
    }
    let mut user_disabled = false;
    if let Some(settings) = read_json(&config_dir.join("settings.json")) {
        if let Some(enabled) = settings.get("enabledPlugins").and_then(|p| p.as_object()) {
            user_disabled = enabled.get(SKILLS_PLUGIN_ID) == Some(&serde_json::Value::Bool(false));
            for (id, on) in enabled {
                let already = conflicts
                    .iter()
                    .any(|c| matches!(c, Conflict::InstalledPlugin { id: seen } if seen == id));
                if foreign_tako_id(id) && on.as_bool() == Some(true) && !already {
                    conflicts.push(Conflict::EnabledPlugin { id: id.clone() });
                }
            }
        }
    }
    Inspection {
        config_dir_exists,
        copy,
        conflicts,
        user_disabled,
    }
}

// --- 判断（純関数）---------------------------------------------------------------

/// 置かない（置き直さない）理由。設定 dir の中身ではなく tako 側の事情
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocked {
    /// 検証プロセスの番人（[`write_guard`]）
    Refused(String),
    /// A/B（[`LEGACY_ENV`] / `TAKO_1877_NO_MOD`）
    Legacy,
    /// `tako mod off`
    Disabled,
    /// claude の版が分からない
    ClaudeUnknown,
    /// claude の版が下限未満
    ClaudeTooOld(String),
}

impl Blocked {
    pub fn state(&self) -> State {
        match self {
            Blocked::Refused(_) => State::Refused,
            Blocked::Legacy => State::Legacy,
            Blocked::Disabled => State::Disabled,
            Blocked::ClaudeUnknown => State::ClaudeUnknown,
            Blocked::ClaudeTooOld(_) => State::ClaudeTooOld,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Blocked::Refused(why) => why.clone(),
            Blocked::Legacy => {
                format!(
                    "{LEGACY_ENV} / {} が立っている（A/B で skills-dir へ入れない）",
                    claude_mod::AB_OFF_ENV
                )
            }
            Blocked::Disabled => "tako mod off で止めている（`tako mod on` で戻る）".into(),
            Blocked::ClaudeUnknown => {
                "claude の版が分からない（claude が入っていないか判定前）ので入れない".into()
            }
            Blocked::ClaudeTooOld(v) => format!(
                "claude {v} は下限 {} 未満なので入れない（`claude update` で上がる）",
                claude_mod::MIN_CLAUDE_VERSION
            ),
        }
    }
}

/// tako 側の事情から「置いてよいか」を決める。理由が複数あれば番人 → A/B → 設定 → 版の順に 1 つ
pub fn gate(
    guard: Result<(), String>,
    legacy: bool,
    enabled: bool,
    claude_version: Option<&str>,
) -> Option<Blocked> {
    if let Err(why) = guard {
        return Some(Blocked::Refused(why));
    }
    if legacy {
        return Some(Blocked::Legacy);
    }
    if !enabled {
        return Some(Blocked::Disabled);
    }
    let Some(version) = claude_version.filter(|v| !v.trim().is_empty()) else {
        return Some(Blocked::ClaudeUnknown);
    };
    match claude_mod::version_supported(version) {
        Some(true) => None,
        Some(false) => Some(Blocked::ClaudeTooOld(version.trim().to_string())),
        None => Some(Blocked::ClaudeUnknown),
    }
}

/// 設定 dir ごとの導入状態（`tako mod` / MCP の `skills.targets[].state`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// いまの tako の写しが入っている
    Installed,
    /// tako の写しが入っているが版（中身）が古い・印が壊れている
    Outdated,
    /// 同名の衝突・利用者のもの（入れない）
    NameConflict,
    /// tako の写しはあるが利用者が Claude Code 側で止めた
    UserDisabled,
    ClaudeTooOld,
    ClaudeUnknown,
    /// 入っていない（入れてよい）
    NotInstalled,
    /// 設定 dir が無い（claude がまだ使っていない。tako は設定 dir を作らない）
    NoConfigDir,
    /// `tako mod off`
    Disabled,
    /// A/B
    Legacy,
    /// 検証プロセスの番人が書かせない
    Refused,
}

impl State {
    pub fn code(self) -> &'static str {
        match self {
            State::Installed => "installed",
            State::Outdated => "outdated",
            State::NameConflict => "name_conflict",
            State::UserDisabled => "user_disabled",
            State::ClaudeTooOld => "claude_too_old",
            State::ClaudeUnknown => "claude_unknown",
            State::NotInstalled => "not_installed",
            State::NoConfigDir => "no_config_dir",
            State::Disabled => "disabled",
            State::Legacy => "legacy",
            State::Refused => "refused",
        }
    }
}

/// 今の状態を 1 語に。tako の写しが最新なら tako 側の事情（版不足等）があっても `installed`
pub fn classify(inspection: &Inspection, expected_hash: &str, blocked: Option<&Blocked>) -> State {
    if !inspection.config_dir_exists {
        return State::NoConfigDir;
    }
    if !inspection.conflicts.is_empty() {
        return State::NameConflict;
    }
    let current =
        matches!(&inspection.copy, CopyState::Managed(m) if m.content_hash == expected_hash);
    if inspection.copy.is_ours() && inspection.user_disabled {
        return State::UserDisabled;
    }
    if current {
        return State::Installed;
    }
    if let Some(blocked) = blocked {
        return blocked.state();
    }
    if inspection.copy.is_ours() {
        State::Outdated
    } else {
        State::NotInstalled
    }
}

/// その設定 dir にすること
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// 何もしない
    Keep,
    /// いまの tako の写しを置く・置き直す
    Write,
    /// tako の写しを外す
    Remove,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Keep => "keep",
            Action::Write => "write",
            Action::Remove => "remove",
        }
    }
}

/// setup / GUI 起動時 / `tako mod install` の判断。
///
/// - 番人・A/B・設定 dir が無いときは何も書かない
/// - 衝突があれば置かない。**既に置いた tako の写しは退く**（利用者の `tako` と取り合わない）
/// - 版不足・未判定・`tako mod off` なら置かない・置き直さない（置いた写しはそのまま休眠）
/// - 中身が同じなら書かない（書くと読み込み中の claude が無駄にホットリロードする）
pub fn plan_sync(
    inspection: &Inspection,
    expected_hash: &str,
    blocked: Option<&Blocked>,
) -> Action {
    if matches!(blocked, Some(Blocked::Refused(_) | Blocked::Legacy))
        || !inspection.config_dir_exists
    {
        return Action::Keep;
    }
    if !inspection.conflicts.is_empty() {
        return if inspection.copy.is_ours() {
            Action::Remove
        } else {
            Action::Keep
        };
    }
    if blocked.is_some() {
        return Action::Keep;
    }
    match &inspection.copy {
        CopyState::Managed(m) if m.content_hash == expected_hash => Action::Keep,
        CopyState::Managed(_) | CopyState::MarkUnreadable | CopyState::Absent => Action::Write,
        _ => Action::Keep,
    }
}

/// `tako mod uninstall` / setup の undo の判断: 印つきの写しだけを外す（手の入った写しは残す）
pub fn plan_uninstall(inspection: &Inspection, refused: bool) -> Action {
    if !refused && inspection.copy.is_ours() {
        Action::Remove
    } else {
        Action::Keep
    }
}

// --- 番人 -----------------------------------------------------------------------

/// 検証プロセスが書いてよい親（一時 dir）
fn verification_roots() -> Vec<PathBuf> {
    let mut roots = vec![std::env::temp_dir()];
    if cfg!(unix) {
        roots.push(PathBuf::from("/tmp"));
        roots.push(PathBuf::from("/private/tmp"));
    }
    roots
        .iter()
        .map(|r| crate::platform::path::canonicalize_or_self(r))
        .collect()
}

/// 在る最も近い祖先を実体のパスへ直し、残りを足し直す（まだ無い設定 dir でも比べられる）
fn canonical_lenient(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut cursor = path.to_path_buf();
    loop {
        if let Ok(real) = crate::platform::path::canonicalize(&cursor) {
            let mut out = real;
            out.extend(rest.iter().rev());
            return out;
        }
        match (
            cursor.file_name().map(|n| n.to_os_string()),
            cursor.parent(),
        ) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                cursor = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// 書いてよい設定 dir か。**検証プロセスは一時 dir の外へ書かない**（`verification` は
/// [`crate::paths::is_verification_process`] の値。テストは env を触らずに両アームを検査できる）
pub fn write_guard(config_dir: &Path, verification: bool) -> Result<(), String> {
    if !verification {
        return Ok(());
    }
    // `..` は先に断る（まだ無い dir の手前で `..` が来ると実体のパスへ直せず、字面の比較が
    // 一時 dir の外への脱出を見逃す）
    let escapes = config_dir
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir));
    let real = canonical_lenient(config_dir);
    if !escapes
        && real.is_absolute()
        && verification_roots()
            .iter()
            .any(|root| real.starts_with(root))
    {
        return Ok(());
    }
    Err(format!(
        "検証プロセス（テスト・TAKO_ISOLATED・セルフテスト）は一時 dir の外の設定 dir へ書かない: {}",
        crate::paths::shorten_home(&config_dir.display().to_string())
    ))
}

/// A/B の入口（[`LEGACY_ENV`] か S1 の `TAKO_1877_NO_MOD`）が立っているか
pub fn legacy() -> bool {
    claude_mod::ab_off() || std::env::var_os(LEGACY_ENV).is_some_and(|v| !v.is_empty())
}

/// 検証用の置く先の差し替え（[`CONFIG_DIRS_ENV`]）。立っていなければ `None`
pub fn override_dirs() -> Option<Vec<PathBuf>> {
    let raw = std::env::var(CONFIG_DIRS_ENV).ok()?;
    let dirs: Vec<PathBuf> = raw
        .split(LIST_SEP)
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .collect();
    (!dirs.is_empty()).then_some(dirs)
}

/// 新しいペインの claude が使う設定 dir の見立て: ペインの env の `CLAUDE_CONFIG_DIR`
/// （worker のアカウント）→ tako 自身の env → 既定（`~/.claude`）
pub fn pane_config_dir(
    pane_env: &[(String, String)],
    process_env: Option<&str>,
    default: Option<&Path>,
) -> Option<PathBuf> {
    pane_env
        .iter()
        .rev()
        .find(|(k, _)| k == "CLAUDE_CONFIG_DIR")
        .map(|(_, v)| v.as_str())
        .or(process_env)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| default.map(Path::to_path_buf))
}

// --- 書き込み -----------------------------------------------------------------------

/// 検証プロセスだけで効く（製品の経路では env が残っていても落ちない）
fn inject_crash(point: &str) {
    if std::env::var(INJECT_CRASH_ENV).ok().as_deref() == Some(point)
        && crate::paths::is_verification_process()
    {
        std::process::exit(INJECT_CRASH_EXIT);
    }
}

fn unique_tag() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

fn staging_prefix() -> String {
    format!(".{MOD_NAME}.tmp-")
}

fn retired_prefix() -> String {
    format!(".{MOD_NAME}.old-")
}

/// 落ちたプロセスが残した一時 dir・退避を片付ける（[`LEFTOVER_AGE`] より古いものだけ）
pub fn sweep_leftovers(skills: &Path) {
    let Ok(entries) = std::fs::read_dir(skills) else {
        return;
    };
    let (tmp, old) = (staging_prefix(), retired_prefix());
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(&tmp) && !name.starts_with(&old) {
            continue;
        }
        let old_enough = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > LEFTOVER_AGE);
        if old_enough {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn mark_json(mark: &ManagedMark) -> String {
    let mut out = serde_json::to_string_pretty(mark).unwrap_or_default();
    out.push('\n');
    out
}

/// いまの tako の写しを `skills/tako` へ置く（置き直す）。
///
/// 一時 dir（`skills/.tako.tmp-*`）に全部書いて**印を最後に**置き、入れ替える直前に
/// `skills/tako` がまだ tako のもの（か無い）ことを確かめてから rename する
/// （確かめた後に利用者が置いたものを退避で消さない）
pub fn write_copy(config_dir: &Path, expected: &Expected) -> Result<(), String> {
    let skills = config_dir.join(SKILLS_DIR);
    std::fs::create_dir_all(&skills)
        .map_err(|e| format!("{} を作れない: {e}", skills.display()))?;
    sweep_leftovers(&skills);
    let tag = unique_tag();
    let tmp = skills.join(format!("{}{tag}", staging_prefix()));
    let write = || -> std::io::Result<()> {
        for (rel, body) in &expected.files {
            let path = tmp.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, body)?;
        }
        std::fs::write(tmp.join(MARK_FILE), mark_json(&expected.mark()))
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(format!("写しを書けない: {e}"));
    }
    inject_crash("staged");
    let dest = skills.join(MOD_NAME);
    let before = read_copy(&dest);
    if !matches!(before, CopyState::Absent) && !before.is_ours() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err("書いている間に利用者の skills/tako が現れたので触らない".into());
    }
    // Claude Code が書いた型定義は引き継ぐ（無ければ何もしない）
    let types = dest.join(ENGINE_TYPES_DIR);
    if types.is_dir() {
        let _ = std::fs::rename(&types, tmp.join(ENGINE_TYPES_DIR));
    }
    let old = skills.join(format!("{}{tag}", retired_prefix()));
    let moved_old = before.is_ours() && std::fs::rename(&dest, &old).is_ok();
    inject_crash("retired");
    if let Err(e) = std::fs::rename(&tmp, &dest) {
        let _ = std::fs::remove_dir_all(&tmp);
        // 並行して別のプロセス（GUI と `tako setup`）が先に置いた: 同じ中身なら成功扱い
        if matches!(read_copy(&dest), CopyState::Managed(m) if m.content_hash == expected.hash) {
            if moved_old {
                let _ = std::fs::remove_dir_all(&old);
            }
            return Ok(());
        }
        if moved_old {
            let _ = std::fs::rename(&old, &dest);
        }
        return Err(format!("写しを差し替えられない: {e}"));
    }
    if moved_old {
        let _ = std::fs::remove_dir_all(&old);
    }
    Ok(())
}

/// tako の写しを外す（退避名へ rename してから消す = Claude Code に半分消えた plugin を見せない）
pub fn remove_copy(config_dir: &Path) -> Result<(), String> {
    let skills = config_dir.join(SKILLS_DIR);
    let dest = skills.join(MOD_NAME);
    if !read_copy(&dest).is_ours() {
        return Err("skills/tako が tako の写しではないので外さない".into());
    }
    let old = skills.join(format!("{}{}", retired_prefix(), unique_tag()));
    std::fs::rename(&dest, &old).map_err(|e| format!("写しを外せない: {e}"))?;
    let _ = std::fs::remove_dir_all(&old);
    sweep_leftovers(&skills);
    Ok(())
}

/// 判断を実行した結果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// 何もしなかった
    Unchanged,
    /// 置いた・置き直した
    Written,
    /// 外した
    Removed,
    /// dry-run（判断だけ）
    Planned,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Unchanged => "unchanged",
            Outcome::Written => "written",
            Outcome::Removed => "removed",
            Outcome::Planned => "planned",
        }
    }
}

/// 判断を実行する（`dry_run` なら何も書かない）
pub fn apply(
    config_dir: &Path,
    action: Action,
    expected: &Expected,
    dry_run: bool,
) -> Result<Outcome, String> {
    match (action, dry_run) {
        (Action::Keep, _) => Ok(Outcome::Unchanged),
        (_, true) => Ok(Outcome::Planned),
        (Action::Write, false) => write_copy(config_dir, expected).map(|()| Outcome::Written),
        (Action::Remove, false) => remove_copy(config_dir).map(|()| Outcome::Removed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 使い捨ての設定 dir（Drop で消える = #1312 の 1 実装）
    fn scratch(name: &str) -> crate::test_residue::ScratchDir {
        crate::test_residue::ScratchDir::new(&format!("mod-install-{name}"))
    }

    fn sync(cfg: &Path, expected: &Expected) -> (Action, Outcome) {
        let inspection = inspect(cfg);
        let action = plan_sync(&inspection, &expected.hash, None);
        (action, apply(cfg, action, expected, false).unwrap())
    }

    #[test]
    fn 置いた写しに印が付き中身が同じなら書かない() {
        let cfg = scratch("idempotent");
        let v1 = Expected::for_version("1.0.0");
        assert_eq!(sync(&cfg, &v1), (Action::Write, Outcome::Written));
        let dest = copy_dir(&cfg);
        let mark = parse_mark(&std::fs::read_to_string(dest.join(MARK_FILE)).unwrap()).unwrap();
        assert_eq!(mark.tako_version, "1.0.0");
        assert_eq!(mark.content_hash, v1.hash);
        assert!(
            std::fs::read_to_string(dest.join(".claude-plugin/plugin.json"))
                .unwrap()
                .contains("\"version\": \"1.0.0\"")
        );
        assert_eq!(classify(&inspect(&cfg), &v1.hash, None), State::Installed);
        let before = std::fs::metadata(dest.join("hooks/register.ts"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(sync(&cfg, &v1), (Action::Keep, Outcome::Unchanged));
        let after = std::fs::metadata(dest.join("hooks/register.ts"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(before, after, "同じ中身なのに書いた");
    }

    #[test]
    fn 版が変われば差し替わり型定義は残り旧版へも戻る() {
        let cfg = scratch("upgrade");
        let v1 = Expected::for_version("1.0.0");
        let v2 = Expected::for_version("1.0.1");
        sync(&cfg, &v1);
        let types = copy_dir(&cfg).join(".claude-plugin/types/claude-code");
        std::fs::create_dir_all(&types).unwrap();
        std::fs::write(types.join("index.d.ts"), "// engine").unwrap();
        assert_eq!(classify(&inspect(&cfg), &v2.hash, None), State::Outdated);
        assert_eq!(sync(&cfg, &v2), (Action::Write, Outcome::Written));
        assert!(types.join("index.d.ts").is_file(), "型定義が消えた");
        // 型定義は比較から外れる（Claude Code が書いても「手が入った」にならない）
        assert_eq!(classify(&inspect(&cfg), &v2.hash, None), State::Installed);
        assert_eq!(sync(&cfg, &v1), (Action::Write, Outcome::Written));
        assert_eq!(classify(&inspect(&cfg), &v1.hash, None), State::Installed);
        // 一時 dir・退避は残らない
        let leftovers: Vec<_> = std::fs::read_dir(cfg.join(SKILLS_DIR))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(leftovers, vec!["tako".to_string()]);
    }

    #[test]
    fn 印の無いskills_takoは利用者のもの() {
        let cfg = scratch("unmanaged");
        let dest = copy_dir(&cfg);
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("SKILL.md"), "---\nname: tako\n---\n").unwrap();
        let v = Expected::for_version("1.0.0");
        let inspection = inspect(&cfg);
        assert_eq!(
            inspection.conflicts,
            vec![Conflict::UserSkillsDir { named_tako: false }]
        );
        assert_eq!(classify(&inspection, &v.hash, None), State::NameConflict);
        assert_eq!(plan_sync(&inspection, &v.hash, None), Action::Keep);
        assert_eq!(plan_uninstall(&inspection, false), Action::Keep);
        // plugin ではない skill（SKILL.md）なら env の注入は利用者のものを潰さない
        assert_eq!(inspection.injection_block(), None);
        assert!(remove_copy(&cfg).is_err());
        assert!(dest.join("SKILL.md").is_file());
        // plugin 名 tako を名乗っていれば注入も止める
        std::fs::create_dir_all(dest.join(".claude-plugin")).unwrap();
        std::fs::write(
            dest.join(".claude-plugin/plugin.json"),
            r#"{"name":"tako"}"#,
        )
        .unwrap();
        assert!(matches!(
            inspect(&cfg).injection_block(),
            Some(claude_mod::OffReason::NameConflict(_))
        ));
    }

    #[test]
    fn 手の入った写しは上書きも削除もしない() {
        let cfg = scratch("modified");
        let v = Expected::for_version("1.0.0");
        sync(&cfg, &v);
        let register = copy_dir(&cfg).join("hooks/register.ts");
        std::fs::write(&register, "// 利用者の改造").unwrap();
        let inspection = inspect(&cfg);
        assert!(matches!(inspection.copy, CopyState::Modified(_)));
        assert_eq!(inspection.conflicts, vec![Conflict::ModifiedCopy]);
        assert_eq!(plan_sync(&inspection, &v.hash, None), Action::Keep);
        assert_eq!(plan_uninstall(&inspection, false), Action::Keep);
        assert_eq!(
            std::fs::read_to_string(&register).unwrap(),
            "// 利用者の改造"
        );
    }

    #[test]
    fn 壊れた印はtakoの写しとして置き直し外せる() {
        let cfg = scratch("broken-mark");
        let v = Expected::for_version("1.0.0");
        sync(&cfg, &v);
        std::fs::write(copy_dir(&cfg).join(MARK_FILE), "{ 壊れた").unwrap();
        let inspection = inspect(&cfg);
        assert_eq!(inspection.copy, CopyState::MarkUnreadable);
        assert_eq!(classify(&inspection, &v.hash, None), State::Outdated);
        assert_eq!(sync(&cfg, &v), (Action::Write, Outcome::Written));
        assert_eq!(classify(&inspect(&cfg), &v.hash, None), State::Installed);
        std::fs::write(
            copy_dir(&cfg).join(MARK_FILE),
            r#"{"managed_by":"someone"}"#,
        )
        .unwrap();
        assert_eq!(plan_uninstall(&inspect(&cfg), false), Action::Remove);
        assert_eq!(
            apply(&cfg, Action::Remove, &v, false).unwrap(),
            Outcome::Removed
        );
        assert!(!copy_dir(&cfg).exists());
    }

    #[test]
    fn 印のファイル一覧が設定dirの外を指すものは印として読まない() {
        let mut mark = Expected::for_version("1.0.0").mark();
        mark.files.push("../../settings.json".into());
        assert!(parse_mark(&serde_json::to_string(&mark).unwrap()).is_none());
        let mut mark = Expected::for_version("1.0.0").mark();
        mark.files.push("/etc/passwd".into());
        assert!(parse_mark(&serde_json::to_string(&mark).unwrap()).is_none());
    }

    #[test]
    fn 別の出どころの同名takoがあれば置かず置いた写しは退く() {
        let v = Expected::for_version("1.0.0");
        // skills/<別名> が name: tako を名乗る
        let cfg = scratch("same-name");
        let other = cfg.join(SKILLS_DIR).join("my-tako/.claude-plugin");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("plugin.json"), r#"{"name":"tako"}"#).unwrap();
        let inspection = inspect(&cfg);
        assert_eq!(
            inspection.conflicts,
            vec![Conflict::SkillsSameName {
                dir: "my-tako".into()
            }]
        );
        assert_eq!(plan_sync(&inspection, &v.hash, None), Action::Keep);
        assert!(inspection.injection_block().is_some());

        // marketplace から入れた tako（installed_plugins.json）と、有効化だけ（settings.json）
        let cfg = scratch("installed");
        sync(&cfg, &v);
        std::fs::create_dir_all(cfg.join("plugins")).unwrap();
        std::fs::write(
            cfg.join("plugins/installed_plugins.json"),
            r#"{"version":2,"plugins":{"tako@my-market":[{"scope":"user"}],"other@x":[]}}"#,
        )
        .unwrap();
        std::fs::write(
            cfg.join("settings.json"),
            r#"{"enabledPlugins":{"tako@my-market":true,"tako@another":true,"tako@off":false,"tako@skills-dir":true}}"#,
        )
        .unwrap();
        let inspection = inspect(&cfg);
        assert_eq!(
            inspection.conflicts,
            vec![
                Conflict::InstalledPlugin {
                    id: "tako@my-market".into()
                },
                Conflict::EnabledPlugin {
                    id: "tako@another".into()
                },
            ]
        );
        assert_eq!(classify(&inspection, &v.hash, None), State::NameConflict);
        assert_eq!(sync(&cfg, &v), (Action::Remove, Outcome::Removed));
        assert!(!copy_dir(&cfg).exists(), "衝突しているのに写しが残った");
        // settings 系のファイルは読むだけ
        assert!(std::fs::read_to_string(cfg.join("settings.json"))
            .unwrap()
            .contains("tako@my-market"));
    }

    #[test]
    fn 利用者がclaude側で止めたらuser_disabledで注入も止める() {
        let cfg = scratch("user-disabled");
        let v = Expected::for_version("1.0.0");
        sync(&cfg, &v);
        std::fs::write(
            cfg.join("settings.json"),
            r#"{"enabledPlugins":{"tako@skills-dir":false}}"#,
        )
        .unwrap();
        let inspection = inspect(&cfg);
        assert!(inspection.user_disabled);
        assert!(inspection.conflicts.is_empty());
        assert_eq!(classify(&inspection, &v.hash, None), State::UserDisabled);
        assert_eq!(
            inspection.injection_block(),
            Some(claude_mod::OffReason::UserDisabled)
        );
        // 写しは最新のまま保つ（利用者が enable に戻したら最新が読まれる）
        assert_eq!(plan_sync(&inspection, &v.hash, None), Action::Keep);
        let v2 = Expected::for_version("1.0.1");
        assert_eq!(plan_sync(&inspection, &v2.hash, None), Action::Write);
    }

    #[test]
    fn 版の下限未満と未判定とoffでは置かない() {
        let v = Expected::for_version("1.0.0");
        let cfg = scratch("gated");
        let inspection = inspect(&cfg);
        for (blocked, state) in [
            (
                gate(Ok(()), false, true, Some("2.1.280")),
                State::ClaudeTooOld,
            ),
            (gate(Ok(()), false, true, None), State::ClaudeUnknown),
            (gate(Ok(()), false, true, Some("abc")), State::ClaudeUnknown),
            (gate(Ok(()), false, false, Some("2.1.294")), State::Disabled),
            (gate(Ok(()), true, true, Some("2.1.294")), State::Legacy),
            (
                gate(Err("番人".into()), false, true, Some("2.1.294")),
                State::Refused,
            ),
        ] {
            let blocked = blocked.expect("止める理由がある");
            assert_eq!(classify(&inspection, &v.hash, Some(&blocked)), state);
            assert_eq!(
                plan_sync(&inspection, &v.hash, Some(&blocked)),
                Action::Keep
            );
        }
        assert_eq!(
            gate(Ok(()), false, true, Some("2.1.294 (Claude Code)")),
            None
        );
        assert!(!copy_dir(&cfg).exists());
    }

    #[test]
    fn 設定dirが無ければ作らない() {
        let base = scratch("missing");
        let cfg = base.join("never-used");
        let v = Expected::for_version("1.0.0");
        let inspection = inspect(&cfg);
        assert_eq!(classify(&inspection, &v.hash, None), State::NoConfigDir);
        assert_eq!(plan_sync(&inspection, &v.hash, None), Action::Keep);
        assert!(!cfg.exists());
    }

    #[test]
    fn skillsが無くても作って置ける() {
        let cfg = scratch("no-skills");
        assert!(!cfg.join(SKILLS_DIR).exists());
        let v = Expected::for_version("1.0.0");
        assert_eq!(sync(&cfg, &v), (Action::Write, Outcome::Written));
        assert_eq!(classify(&inspect(&cfg), &v.hash, None), State::Installed);
    }

    #[test]
    fn 落ちた差し替えの残骸があっても次で置き直せる() {
        let cfg = scratch("crashed");
        let v = Expected::for_version("1.0.0");
        sync(&cfg, &v);
        let skills = cfg.join(SKILLS_DIR);
        // 「旧い写しを退避した後に落ちた」= skills/tako が無く、退避と一時 dir が残る
        std::fs::rename(skills.join("tako"), skills.join(".tako.old-1-0")).unwrap();
        std::fs::create_dir_all(skills.join(".tako.tmp-1-0/hooks")).unwrap();
        assert_eq!(classify(&inspect(&cfg), &v.hash, None), State::NotInstalled);
        assert_eq!(sync(&cfg, &v), (Action::Write, Outcome::Written));
        assert_eq!(classify(&inspect(&cfg), &v.hash, None), State::Installed);
        // 残骸は dot で始まる（Claude Code は plugin として読まない）。古くなれば片付く
        assert!(inspect(&cfg).conflicts.is_empty());
    }

    #[test]
    fn 外すと印つきの写しだけが消え他のskillsと設定は残る() {
        let cfg = scratch("uninstall");
        let v = Expected::for_version("1.0.0");
        sync(&cfg, &v);
        let other = cfg.join(SKILLS_DIR).join("mine");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("SKILL.md"), "x").unwrap();
        std::fs::write(cfg.join("settings.json"), "{}").unwrap();
        let inspection = inspect(&cfg);
        assert_eq!(plan_uninstall(&inspection, false), Action::Remove);
        assert_eq!(plan_uninstall(&inspection, true), Action::Keep);
        assert_eq!(
            apply(&cfg, Action::Remove, &v, true).unwrap(),
            Outcome::Planned
        );
        assert!(copy_dir(&cfg).exists(), "dry-run で消した");
        assert_eq!(
            apply(&cfg, Action::Remove, &v, false).unwrap(),
            Outcome::Removed
        );
        assert!(!copy_dir(&cfg).exists());
        assert!(other.join("SKILL.md").is_file());
        assert_eq!(
            std::fs::read_to_string(cfg.join("settings.json")).unwrap(),
            "{}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinkのskills_takoは辿って書かない() {
        let cfg = scratch("symlink");
        let target = scratch("symlink-target");
        std::fs::create_dir_all(cfg.join(SKILLS_DIR)).unwrap();
        std::os::unix::fs::symlink(&target, copy_dir(&cfg)).unwrap();
        let v = Expected::for_version("1.0.0");
        let inspection = inspect(&cfg);
        assert_eq!(inspection.copy, CopyState::Symlink);
        assert_eq!(plan_sync(&inspection, &v.hash, None), Action::Keep);
        assert!(write_copy(&cfg, &v).is_err());
        assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn 読み取り専用の設定dirは失敗を返し何も壊さない() {
        use std::os::unix::fs::PermissionsExt;
        let cfg = scratch("readonly");
        std::fs::create_dir_all(cfg.join(SKILLS_DIR)).unwrap();
        std::fs::set_permissions(cfg.join(SKILLS_DIR), std::fs::Permissions::from_mode(0o555))
            .unwrap();
        let v = Expected::for_version("1.0.0");
        let result = write_copy(&cfg, &v);
        std::fs::set_permissions(cfg.join(SKILLS_DIR), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        // root で走らせると書けてしまうので、そのときは書けたことだけを見る
        if result.is_err() {
            assert_eq!(classify(&inspect(&cfg), &v.hash, None), State::NotInstalled);
            assert_eq!(std::fs::read_dir(cfg.join(SKILLS_DIR)).unwrap().count(), 0);
        }
    }

    #[test]
    fn 検証プロセスは一時dirの外へ書かない() {
        let inside = scratch("guard");
        assert!(write_guard(&inside, true).is_ok());
        assert!(write_guard(&inside.join("not-yet/created"), true).is_ok());
        let home_like = PathBuf::from(if cfg!(windows) {
            r"C:\Users\testuser\.claude"
        } else {
            "/Users/testuser/.claude"
        });
        let err = write_guard(&home_like, true).unwrap_err();
        assert!(err.contains("検証プロセス"), "{err}");
        assert!(
            write_guard(&home_like, false).is_ok(),
            "製品の経路は止めない"
        );
        assert!(write_guard(Path::new("relative/.claude"), true).is_err());
        // `..` で一時 dir の外へ出るものも止める（在る dir 越し・まだ無い dir 越しの両方）
        let escape = inside.join("../../../../../../etc/.claude");
        assert!(write_guard(&escape, true).is_err());
        let escape = inside.join("not-yet/../../../../../../../.claude");
        assert!(write_guard(&escape, true).is_err());
    }

    #[test]
    fn ペインの設定dirはペインのenvから既定の順に見立てる() {
        let default = PathBuf::from("/h/.claude");
        let env = vec![("CLAUDE_CONFIG_DIR".to_string(), "/h/.claude-b".to_string())];
        assert_eq!(
            pane_config_dir(&env, Some("/h/.claude-a"), Some(&default)),
            Some(PathBuf::from("/h/.claude-b"))
        );
        assert_eq!(
            pane_config_dir(&[], Some("/h/.claude-a"), Some(&default)),
            Some(PathBuf::from("/h/.claude-a"))
        );
        assert_eq!(
            pane_config_dir(&[], Some(""), Some(&default)),
            Some(default.clone())
        );
        assert_eq!(pane_config_dir(&[], None, None), None);
    }

    #[test]
    fn ハッシュは並び順に依らずパスの付け替えで変わる() {
        let a = content_hash([("a", b"1".as_slice()), ("b", b"2".as_slice())]);
        let b = content_hash([("b", b"2".as_slice()), ("a", b"1".as_slice())]);
        assert_eq!(a, b);
        let c = content_hash([("a", b"2".as_slice()), ("b", b"1".as_slice())]);
        assert_ne!(a, c);
        let d = content_hash([("ab", b"".as_slice())]);
        let e = content_hash([("a", b"b".as_slice())]);
        assert_ne!(d, e, "パスと中身の境目がずれても同じ値にならない");
        assert_eq!(a.len(), 16);
    }
}
