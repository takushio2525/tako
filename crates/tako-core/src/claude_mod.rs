//! claude_mod — tako が同梱する Claude Code の mod（エピック #1877 / S1 #1879。FR-2.42）
//!
//! 設計の正本は `.agent/plans/2026-10-tako-mod.md`（§3 置き場・注入 / §4 報告・鮮度 / §5 mod の規約）。
//!
//! - **同梱と展開**: `claude-mod/` をバイナリへ埋め込み、[`install`] で `<data_dir>/claude-mod/tako/`
//!   へ書き出す（`shell_integration` と同じ型）。**`.app` の中には置かない**: Claude Code は mod を
//!   読むたびにそのフォルダへ `.claude-plugin/types/` を書くので、署名済みの bundle が改変される
//! - **注入**: ペインの env に [`PLUGIN_DIRS_ENV`]（展開先）と [`CLI_ENV`]（実行中の tako CLI）を
//!   足すだけ。Claude Code の設定ファイルは 1 バイトも書かない。止めるのは tako の設定
//!   （`tako mod off`）と A/B の [`AB_OFF_ENV`]、版の下限 [`MIN_CLAUDE_VERSION`]
//! - **報告**: mod は `tako mod report` を叩き、状態（[`ModReport`]）を送ってくる。保持は GUI の
//!   メモリだけ（[`ModHub`]）で、[`FRESH_FOR`] を過ぎた報告は「無い」とみなす
//!
//! このモジュールは**純関数と素のデータだけ**を持つ（GUI 非依存。判断はここで閉じ、
//! tako-app は値を渡して結果を env へ足すだけにする）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// mod の読み込み先を Claude Code へ教える env（Claude Code 側の名前。区切りは [`LIST_SEP`]）
pub const PLUGIN_DIRS_ENV: &str = "CLAUDE_CODE_PLUGIN_DIRS";
/// mod が `$.process.run` に渡す tako CLI の絶対パス。PATH 先頭の古い `target` を掴む罠
/// （#432）を避けるため、実行中のバイナリの隣の CLI を名指しで渡す
pub const CLI_ENV: &str = "TAKO_CLI";
/// この機能が撒く env の名前。器（tmux）のセッション作成時に値を固定する対象
/// （[`crate::backend::session_pinned_env`]）。サーバーのグローバル環境に前の値が
/// 残っていると、`tako mod off` にしても新しいペインへ届いてしまうため
pub const INJECTED_KEYS: &[&str] = &[PLUGIN_DIRS_ENV, CLI_ENV];
/// 同一バイナリで注入を止める A/B の入口（#1877 の規約。値が空でなければ立つ）
pub const AB_OFF_ENV: &str = "TAKO_1877_NO_MOD";
/// 注入する Claude Code の版の下限（**実測した版**。2.1.287〜2.1.293 は未実測なので下げるなら
/// 実測してから。設計書 §1.6 / §3.4）
pub const MIN_CLAUDE_VERSION: &str = "2.1.294";
/// mod の名前（`plugin.json` の `name` と展開先のフォルダ名）
pub const MOD_NAME: &str = "tako";
/// `register.ts` の中で展開時に tako の版へ置き換える目印
pub const VERSION_PLACEHOLDER: &str = "__TAKO_MOD_VERSION__";
/// 報告の鮮度。mod の heartbeat（15 秒）を 2 回取りこぼしても新鮮なまま
pub const FRESH_FOR: Duration = Duration::from_secs(45);
/// mod が変化の無いときに送る間隔（`register.ts` の `HEARTBEAT_MS` と同じ値。単体テストが突き合わせる）
pub const HEARTBEAT: Duration = Duration::from_secs(15);
/// 報告の契約の版（`register.ts` / `types/index.d.ts` の `schema`）
pub const SCHEMA: u32 = 1;

/// env の一覧の区切り（PATH と同じ。macOS / Linux は `:`、Windows は `;`）
pub const LIST_SEP: char = if cfg!(windows) { ';' } else { ':' };

/// Claude Code が読み込みのたびに mod のフォルダへ書く型定義の置き場。
/// **比較から外し、差し替えでも消さない**（消しても次の読み込みで書き直されるが、
/// その間エディタの型が無くなるだけで得るものが無い）
pub const ENGINE_TYPES_DIR: &str = ".claude-plugin/types";

const PLUGIN_JSON: &str = include_str!("../claude-mod/.claude-plugin/plugin.json");
const HOOKS_JSON: &str = include_str!("../claude-mod/hooks/hooks.json");
const REGISTER_TS: &str = include_str!("../claude-mod/hooks/register.ts");
const TYPES_DTS: &str = include_str!("../claude-mod/types/index.d.ts");
const TSCONFIG_JSON: &str = include_str!("../claude-mod/tsconfig.json");

/// 展開するファイル（相対パス, 中身）。`tests/` は開発時の `claude plugin test` 用なので置かない
pub fn rendered_files(version: &str) -> Vec<(&'static str, String)> {
    vec![
        (".claude-plugin/plugin.json", render_plugin_json(version)),
        ("hooks/hooks.json", HOOKS_JSON.to_string()),
        (
            "hooks/register.ts",
            REGISTER_TS.replace(VERSION_PLACEHOLDER, version),
        ),
        ("types/index.d.ts", TYPES_DTS.to_string()),
        // 自前の tsconfig を置く（無いと Claude Code が mod のフォルダへ書く）
        ("tsconfig.json", TSCONFIG_JSON.to_string()),
    ]
}

/// `plugin.json` の `version` を tako の版へ揃える（ソースは `0.0.0` のまま置く =
/// リリースのたびにソースを書き換えない）
fn render_plugin_json(version: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(PLUGIN_JSON) {
        Ok(mut v) => {
            v["version"] = serde_json::Value::String(version.to_string());
            let mut out = serde_json::to_string_pretty(&v).unwrap_or_else(|_| PLUGIN_JSON.into());
            out.push('\n');
            out
        }
        // 埋め込みが壊れていることは単体テストが先に落とす。ここでは元のまま置く
        Err(_) => PLUGIN_JSON.to_string(),
    }
}

/// 展開先の親（`<data_dir>/claude-mod`）
pub fn mod_root() -> Option<PathBuf> {
    crate::paths::data_dir().map(|d| d.join("claude-mod"))
}

/// 展開先（`<data_dir>/claude-mod/tako`）。ペインの [`PLUGIN_DIRS_ENV`] に入る値
pub fn mod_dir() -> Option<PathBuf> {
    mod_root().map(|r| r.join(MOD_NAME))
}

/// [`install`] の結果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallOutcome {
    /// 中身が同じだったので書かなかった（書くと読み込み中の claude が無駄にホットリロードする）
    Unchanged,
    /// 一時 dir に書いてから差し替えた
    Written,
}

impl InstallOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            InstallOutcome::Unchanged => "unchanged",
            InstallOutcome::Written => "written",
        }
    }
}

/// 同梱の mod を `<data_dir>/claude-mod/tako/` へ展開する（冪等）。
/// 発火は GUI 起動時と `tako setup`。戻り値は展開先
pub fn install() -> Result<(PathBuf, InstallOutcome), String> {
    let root = mod_root().ok_or("データディレクトリが決まらない（HOME が無い）")?;
    let outcome = install_in(&root, env!("CARGO_PKG_VERSION"))?;
    Ok((root.join(MOD_NAME), outcome))
}

/// [`install`] の本体。`root` の下の [`MOD_NAME`] へ書く（テストは一時 dir を渡す）。
///
/// 差し替えは「一時 dir に全部書く → Claude Code の型定義だけ引き継ぐ → 旧フォルダを退避 →
/// 一時 dir を本来の名前へ rename → 退避を消す」。ファイルを 1 本ずつ上書きすると、
/// ホットリロードが新しい `register.ts` と古い `hooks.json` の組を読みうる
pub fn install_in(root: &Path, version: &str) -> Result<InstallOutcome, String> {
    let dest = root.join(MOD_NAME);
    let files = rendered_files(version);
    if matches_installed(&dest, &files) {
        return Ok(InstallOutcome::Unchanged);
    }
    std::fs::create_dir_all(root).map_err(|e| format!("{} を作れない: {e}", root.display()))?;
    sweep_leftovers(root);
    let tag = unique_tag();
    let tmp = root.join(format!(".{MOD_NAME}.tmp-{tag}"));
    let write = || -> std::io::Result<()> {
        for (rel, body) in &files {
            let path = tmp.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, body)?;
        }
        Ok(())
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(format!("mod を書き出せない: {e}"));
    }
    // Claude Code が書いた型定義は引き継ぐ（無ければ何もしない）
    let types = dest.join(ENGINE_TYPES_DIR);
    if types.is_dir() {
        let _ = std::fs::rename(&types, tmp.join(ENGINE_TYPES_DIR));
    }
    let old = root.join(format!(".{MOD_NAME}.old-{tag}"));
    let moved_old = std::fs::rename(&dest, &old).is_ok();
    if let Err(e) = std::fs::rename(&tmp, &dest) {
        // 並行して別のプロセス（GUI と `tako setup`）が先に置いた: 中身が同じなら成功扱い
        let _ = std::fs::remove_dir_all(&tmp);
        if matches_installed(&dest, &files) {
            if moved_old {
                let _ = std::fs::remove_dir_all(&old);
            }
            return Ok(InstallOutcome::Written);
        }
        if moved_old {
            let _ = std::fs::rename(&old, &dest);
        }
        return Err(format!("mod を差し替えられない: {e}"));
    }
    if moved_old {
        let _ = std::fs::remove_dir_all(&old);
    }
    Ok(InstallOutcome::Written)
}

/// 置いたファイルがすべて同じ中身か（Claude Code が書いた型定義は見ない）
fn matches_installed(dest: &Path, files: &[(&'static str, String)]) -> bool {
    files.iter().all(|(rel, body)| {
        std::fs::read(dest.join(rel)).is_ok_and(|bytes| bytes == body.as_bytes())
    })
}

/// 落ちたプロセスが残した一時 dir・退避を片付ける（10 分より古いものだけ =
/// 並行して展開中の別プロセスの一時 dir には触らない）
fn sweep_leftovers(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let tmp_prefix = format!(".{MOD_NAME}.tmp-");
    let old_prefix = format!(".{MOD_NAME}.old-");
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(&tmp_prefix) && !name.starts_with(&old_prefix) {
            continue;
        }
        let old_enough = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(600));
        if old_enough {
            let _ = std::fs::remove_dir_all(entry.path());
        }
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

// --- 版 ---------------------------------------------------------------------

/// `2.1.294` / `v2.1.294` / `2.1.294 (Claude Code)` を数の組へ。読めなければ `None`
pub fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let word = text.split_whitespace().next()?.trim_start_matches('v');
    let mut parts = word.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    // `2.1.294-beta` のような後ろ付きも数字の部分だけを見る
    let patch_text: String = parts
        .next()?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let patch = patch_text.parse().ok()?;
    Some((major, minor, patch))
}

/// 版が下限以上か。読めなければ `None`（= 未判定として注入しない）
pub fn version_supported(text: &str) -> Option<bool> {
    let have = parse_version(text)?;
    let min = parse_version(MIN_CLAUDE_VERSION)?;
    Some(have >= min)
}

// --- 注入の判断 ---------------------------------------------------------------

/// 注入しない理由。`tako mod`（status）と各ペインの行にそのまま出す
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OffReason {
    /// `TAKO_1877_NO_MOD=1`（A/B）
    AbOff,
    /// `tako mod off`
    Disabled,
    /// 展開できていない（data dir が無い・書けない）
    NotInstalled,
    /// 実行中のバイナリの隣に tako CLI が無い（dev で tako-cli を未ビルド等）
    NoCli,
    /// claude の版がまだ分からない（claude が入っていない・判定前）
    ClaudeUnknown,
    /// claude の版が下限未満
    ClaudeTooOld(String),
}

impl OffReason {
    pub fn code(&self) -> &'static str {
        match self {
            OffReason::AbOff => "ab_off",
            OffReason::Disabled => "disabled",
            OffReason::NotInstalled => "not_installed",
            OffReason::NoCli => "no_cli",
            OffReason::ClaudeUnknown => "claude_unknown",
            OffReason::ClaudeTooOld(_) => "claude_too_old",
        }
    }

    pub fn describe(&self) -> String {
        match self {
            OffReason::AbOff => format!("{AB_OFF_ENV} が立っている（A/B で注入を止めている）"),
            OffReason::Disabled => "tako mod off で止めている（`tako mod on` で戻る）".into(),
            OffReason::NotInstalled => "mod を data dir へ展開できていない".into(),
            OffReason::NoCli => "実行中の tako の隣に tako CLI が無い".into(),
            OffReason::ClaudeUnknown => {
                "claude の版がまだ分からない（claude が入っていないか判定前）".into()
            }
            OffReason::ClaudeTooOld(v) => {
                format!("claude {v} は下限 {MIN_CLAUDE_VERSION} 未満（`claude update` で上がる）")
            }
        }
    }
}

/// 新しく作るペインへ注入するかどうか
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Injection {
    On { plugin_dir: PathBuf, cli: PathBuf },
    Off(OffReason),
}

impl Injection {
    pub fn is_on(&self) -> bool {
        matches!(self, Injection::On { .. })
    }

    pub fn off_reason(&self) -> Option<&OffReason> {
        match self {
            Injection::Off(reason) => Some(reason),
            Injection::On { .. } => None,
        }
    }
}

/// 判断の材料（呼び出し側が集めて渡す）
#[derive(Debug, Clone, Copy)]
pub struct Inputs<'a> {
    pub enabled: bool,
    pub ab_off: bool,
    pub plugin_dir: Option<&'a Path>,
    pub cli: Option<&'a Path>,
    pub claude_version: Option<&'a str>,
}

/// 注入するかを決める（純関数）。理由が複数あるときは「利用者が直せる順」に 1 つだけ返す
pub fn decide(inputs: Inputs<'_>) -> Injection {
    if inputs.ab_off {
        return Injection::Off(OffReason::AbOff);
    }
    if !inputs.enabled {
        return Injection::Off(OffReason::Disabled);
    }
    let Some(plugin_dir) = inputs.plugin_dir else {
        return Injection::Off(OffReason::NotInstalled);
    };
    let Some(cli) = inputs.cli else {
        return Injection::Off(OffReason::NoCli);
    };
    let Some(version) = inputs.claude_version.filter(|v| !v.trim().is_empty()) else {
        return Injection::Off(OffReason::ClaudeUnknown);
    };
    match version_supported(version) {
        Some(true) => Injection::On {
            plugin_dir: plugin_dir.to_path_buf(),
            cli: cli.to_path_buf(),
        },
        Some(false) => Injection::Off(OffReason::ClaudeTooOld(version.trim().to_string())),
        None => Injection::Off(OffReason::ClaudeUnknown),
    }
}

/// A/B の入口が立っているか
pub fn ab_off() -> bool {
    std::env::var_os(AB_OFF_ENV).is_some_and(|v| !v.is_empty())
}

/// 一覧の 1 項目が tako の mod の置き場か（どのインスタンスの data dir でも）。
/// tako のペインから立てた tako（開発中の隔離起動など）は親の値を継承するので、
/// それを「利用者自身の plugin dir」と取り違えないために見分ける
fn is_tako_mod_entry(entry: &str) -> bool {
    let trimmed = entry.trim_end_matches(['/', '\\']);
    let mut parts = trimmed.rsplit(['/', '\\']);
    parts.next() == Some(MOD_NAME) && parts.next() == Some("claude-mod")
}

/// 継承した一覧から tako の mod の置き場と空の項目を除いたもの（利用者自身の分）
fn user_entries(inherited: Option<&str>, sep: char) -> Vec<String> {
    inherited
        .unwrap_or_default()
        .split(sep)
        .filter(|e| !e.is_empty() && !is_tako_mod_entry(e))
        .map(str::to_string)
        .collect()
}

/// ペインの env へ足す組。`inherited_*` は tako 自身のプロセスが持っていた値（子へ継承される分）。
///
/// - 注入する: [`PLUGIN_DIRS_ENV`] = 展開先 + 利用者自身の分（**上書きせず連結**）・[`CLI_ENV`]
/// - 注入しない: 継承した値に tako の mod が混ざっているときだけ、それを抜いた値で打ち消す
///   （tako のペインから立てた tako が親の mod を子へ渡さない = `tako mod off` と A/B が効く）
pub fn pane_env(
    injection: &Injection,
    inherited_plugin_dirs: Option<&str>,
    inherited_cli: Option<&str>,
    sep: char,
) -> Vec<(String, String)> {
    let user = user_entries(inherited_plugin_dirs, sep);
    match injection {
        Injection::On { plugin_dir, cli } => {
            let mut dirs = vec![plugin_dir.display().to_string()];
            dirs.extend(user);
            vec![
                (PLUGIN_DIRS_ENV.into(), dirs.join(&sep.to_string())),
                (CLI_ENV.into(), cli.display().to_string()),
            ]
        }
        Injection::Off(_) => {
            let inherited_tako = inherited_plugin_dirs
                .unwrap_or_default()
                .split(sep)
                .any(is_tako_mod_entry);
            let inherited_cli = inherited_cli.is_some_and(|v| !v.is_empty());
            if inherited_tako || inherited_cli {
                neutral_pairs(inherited_plugin_dirs, sep)
            } else {
                Vec::new()
            }
        }
    }
}

/// 「注入しない」を器（tmux）のセッションへ固定するための組（利用者自身の分だけを残し、
/// CLI は空）。器のサーバーは最初のクライアントの env を引き継ぐので、前に注入した値が
/// グローバル環境に残っていると、固定しない限り新しいペインへ漏れる
pub fn neutral_pairs(inherited_plugin_dirs: Option<&str>, sep: char) -> Vec<(String, String)> {
    vec![
        (
            PLUGIN_DIRS_ENV.into(),
            user_entries(inherited_plugin_dirs, sep).join(&sep.to_string()),
        ),
        (CLI_ENV.into(), String::new()),
    ]
}

/// tako 自身のプロセスが持つ値（[`pane_env`] / [`neutral_pairs`] の `inherited_*`）
pub fn inherited_env() -> (Option<String>, Option<String>) {
    (
        std::env::var(PLUGIN_DIRS_ENV).ok(),
        std::env::var(CLI_ENV).ok(),
    )
}

// --- 報告 --------------------------------------------------------------------

/// ターンの状態（`register.ts` の `TakoTurn`）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModTurn {
    Idle,
    Busy,
    /// 権限ダイアログ表示中（承認後はそのツールが返るまでこの値のまま = mod API に承認の瞬間を
    /// 知らせるイベントが無い）
    Permission,
    /// AskUserQuestion 表示中
    Question,
}

impl ModTurn {
    pub fn as_str(self) -> &'static str {
        match self {
            ModTurn::Idle => "idle",
            ModTurn::Busy => "busy",
            ModTurn::Permission => "permission",
            ModTurn::Question => "question",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModContext {
    /// 最初の API 応答までは欠ける（0 埋めしない = 未観測）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    pub window: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModRateLimit {
    pub kind: String,
    pub percent_used: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
    pub observed_at: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModLastTurn {
    pub duration_ms: u64,
    pub reason: String,
}

/// mod からの報告（`schema: 1`。設計書 §4.2 / `claude-mod/types/index.d.ts`）。
///
/// **本文・プロンプト・ツールの引数を持つフィールドを足さない**（AGENTS.md の絶対ルール。
/// 番犬 `issue1879_claude_mod_watchdog.rs` がフィールド名を走査する）。未知のキーは serde が
/// 捨てるので、mod 側が誤って本文を載せても tako のメモリには残らない
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModReport {
    pub schema: u32,
    pub mod_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// mod が報告を組んだ時刻（epoch ms。mod の時計なので鮮度の判定には使わない）
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ModContext>,
    #[serde(default)]
    pub rate_limits: Vec<ModRateLimit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub turn: ModTurn,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_turn: Option<ModLastTurn>,
    /// classic 系のイベント（PermissionRequest 等）が mod へ届いているか。組織アカウントでは
    /// 届かないことがあり（2.1.294 で実測）、そのとき `permission` は ask ルール由来と
    /// ExitPlanMode だけ = 画面と突き合わせる判断材料（S2）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classic_events: Option<bool>,
    /// 使用制限を束ねる鍵（アカウント単位）。**status には出さない**（ホームパスを含みうる）
    #[serde(default, skip_serializing)]
    pub config_dir: Option<String>,
    #[serde(default)]
    pub ended: bool,
}

/// 文字列の上限（ツール名・モデル名など。おかしな値でメモリを食わせない）
const MAX_TEXT: usize = 256;
/// 使用制限の窓の数の上限
const MAX_RATE_LIMITS: usize = 16;

/// 報告を読む。`schema` が違えば断る（mod と tako の版がずれたとき、黙って誤読しない）
pub fn parse_report(value: serde_json::Value) -> Result<ModReport, String> {
    let schema = value.get("schema").and_then(serde_json::Value::as_u64);
    if schema != Some(u64::from(SCHEMA)) {
        return Err(format!(
            "未対応の報告の形（schema={}。この tako が読めるのは {SCHEMA}）",
            schema.map_or_else(|| "なし".to_string(), |s| s.to_string())
        ));
    }
    let mut report: ModReport =
        serde_json::from_value(value).map_err(|e| format!("報告を読めない: {e}"))?;
    for text in [
        &mut report.claude_version,
        &mut report.session_id,
        &mut report.model,
        &mut report.effort,
        &mut report.pending_tool,
        &mut report.config_dir,
    ]
    .into_iter()
    .flatten()
    {
        clip(text);
    }
    clip(&mut report.mod_version);
    report.rate_limits.truncate(MAX_RATE_LIMITS);
    for limit in &mut report.rate_limits {
        clip(&mut limit.kind);
    }
    Ok(report)
}

fn clip(text: &mut String) {
    if text.len() > MAX_TEXT {
        let mut end = MAX_TEXT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
}

/// 報告の鮮度（[`FRESH_FOR`]。純関数）
pub fn is_fresh(received: Instant, now: Instant) -> bool {
    now.saturating_duration_since(received) <= FRESH_FOR
}

/// 1 ペインの最終報告
#[derive(Debug, Clone)]
pub struct StoredReport {
    pub report: ModReport,
    /// tako が受け取った時刻（鮮度の判定はこちら = tako の時計）
    pub received: Instant,
    /// このペインから受け取った報告の数（heartbeat も数える）
    pub count: u64,
}

/// 新しく作ったペインの注入の記録（status の行の材料）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneInjection {
    /// `None` = 注入した
    pub off: Option<OffReason>,
}

/// GUI のメモリに置く tako mod の状態（**永続化しない** = マイグレーション対象を増やさない）
#[derive(Debug, Clone, Default)]
pub struct ModHub {
    /// 設定 `claude_mod`（settings.json。既定 true）
    pub enabled: bool,
    /// 展開先（展開に失敗したら `None`）
    pub plugin_dir: Option<PathBuf>,
    /// 展開の結果（`written` / `unchanged`）か失敗の理由
    pub install_result: Option<Result<InstallOutcome, String>>,
    /// mod に渡す CLI（実行中のバイナリの隣）
    pub cli: Option<PathBuf>,
    /// 控えた claude の版（`None` = 未判定）
    pub claude_version: Option<String>,
    /// 新しく作ったペインごとの判断
    pub injections: HashMap<u64, PaneInjection>,
    /// ペインごとの最終報告
    pub reports: HashMap<u64, StoredReport>,
}

/// [`ModHub::accept`] の結果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accepted {
    Stored,
    /// `ended: true` を受けて捨てた
    Ended,
}

impl ModHub {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ..Self::default()
        }
    }

    /// いま新しいペインを作るなら注入するか
    pub fn decide(&self, ab_off: bool) -> Injection {
        decide(Inputs {
            enabled: self.enabled,
            ab_off,
            plugin_dir: self.plugin_dir.as_deref(),
            cli: self.cli.as_deref(),
            claude_version: self.claude_version.as_deref(),
        })
    }

    pub fn record_spawn(&mut self, pane: u64, injection: &Injection) {
        self.injections.insert(
            pane,
            PaneInjection {
                off: injection.off_reason().cloned(),
            },
        );
    }

    /// 報告を受け取る。`ended` ならそのペインの報告を捨てる
    pub fn accept(&mut self, pane: u64, report: ModReport, now: Instant) -> Accepted {
        if report.ended {
            self.reports.remove(&pane);
            return Accepted::Ended;
        }
        let count = self.reports.get(&pane).map_or(0, |s| s.count) + 1;
        self.reports.insert(
            pane,
            StoredReport {
                report,
                received: now,
                count,
            },
        );
        Accepted::Stored
    }

    /// 新鮮な報告（[`FRESH_FOR`] 以内）だけを返す。S2 以降の一次ソースの入口
    pub fn fresh_report(&self, pane: u64, now: Instant) -> Option<&ModReport> {
        self.reports
            .get(&pane)
            .filter(|s| is_fresh(s.received, now))
            .map(|s| &s.report)
    }

    /// 閉じたペインの記録を捨てる（ペイン ID が再利用されたとき前任の報告を名乗らない）
    pub fn forget_pane(&mut self, pane: u64) {
        self.injections.remove(&pane);
        self.reports.remove(&pane);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "tako-claude-mod-{}-{name}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&base).unwrap();
        assert!(base.starts_with(std::env::temp_dir()));
        base
    }

    #[test]
    fn 展開するplugin_jsonの版はtakoの版に揃う() {
        let files = rendered_files("9.8.7");
        let (_, plugin) = files
            .iter()
            .find(|(rel, _)| *rel == ".claude-plugin/plugin.json")
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(plugin).unwrap();
        assert_eq!(v["version"], "9.8.7");
        assert_eq!(v["name"], MOD_NAME, "名前は展開先のフォルダ名と同じ");
    }

    #[test]
    fn register_tsの版の目印は1つだけで展開後には残らない() {
        assert_eq!(REGISTER_TS.matches(VERSION_PLACEHOLDER).count(), 1);
        let files = rendered_files("9.8.7");
        let (_, register) = files
            .iter()
            .find(|(rel, _)| *rel == "hooks/register.ts")
            .unwrap();
        assert!(!register.contains(VERSION_PLACEHOLDER));
        assert!(register.contains("const MOD_VERSION = '9.8.7'"));
    }

    #[test]
    fn hooks_jsonはregister_tsを指す() {
        let v: serde_json::Value = serde_json::from_str(HOOKS_JSON).unwrap();
        assert_eq!(v["modules"][0], "./register.ts");
    }

    #[test]
    fn register_tsの鮮度の定数はrust側と揃う() {
        let heartbeat = format!(
            "const HEARTBEAT_MS = {}",
            group_thousands(HEARTBEAT.as_millis())
        );
        assert!(
            REGISTER_TS.contains(&heartbeat),
            "register.ts の HEARTBEAT_MS が {HEARTBEAT:?} と違う（{heartbeat} を探した）"
        );
        assert!(
            FRESH_FOR >= HEARTBEAT * 2,
            "heartbeat を 1 回落としただけで失効する"
        );
    }

    fn group_thousands(n: u128) -> String {
        let s = n.to_string();
        let mut out = String::new();
        for (i, c) in s.chars().enumerate() {
            if i > 0 && (s.len() - i).is_multiple_of(3) {
                out.push('_');
            }
            out.push(c);
        }
        out
    }

    #[test]
    fn 展開は冪等で同じ中身なら書かない() {
        let root = scratch("idempotent");
        assert_eq!(install_in(&root, "1.0.0").unwrap(), InstallOutcome::Written);
        let register = root.join("tako/hooks/register.ts");
        let before = std::fs::metadata(&register).unwrap().modified().unwrap();
        assert_eq!(
            install_in(&root, "1.0.0").unwrap(),
            InstallOutcome::Unchanged
        );
        let after = std::fs::metadata(&register).unwrap().modified().unwrap();
        assert_eq!(
            before, after,
            "同じ中身なのに書き直した（読み込み中の claude が無駄にリロードする）"
        );
        // 版が変われば差し替わり、Claude Code が書いた型定義は残る
        let types = root.join("tako/.claude-plugin/types/claude-code");
        std::fs::create_dir_all(&types).unwrap();
        std::fs::write(types.join("index.d.ts"), "// engine").unwrap();
        assert_eq!(install_in(&root, "1.0.1").unwrap(), InstallOutcome::Written);
        assert!(std::fs::read_to_string(&register)
            .unwrap()
            .contains("'1.0.1'"));
        assert_eq!(
            std::fs::read_to_string(types.join("index.d.ts")).unwrap(),
            "// engine"
        );
        // 一時 dir・退避を残さない
        let leftovers: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != MOD_NAME)
            .collect();
        assert!(leftovers.is_empty(), "残骸: {leftovers:?}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn 壊れた展開先は書き直される() {
        let root = scratch("repair");
        install_in(&root, "1.0.0").unwrap();
        std::fs::write(root.join("tako/hooks/hooks.json"), "{}").unwrap();
        assert_eq!(install_in(&root, "1.0.0").unwrap(), InstallOutcome::Written);
        assert_eq!(
            std::fs::read_to_string(root.join("tako/hooks/hooks.json")).unwrap(),
            HOOKS_JSON
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn 版の読み取りと下限() {
        assert_eq!(parse_version("2.1.294"), Some((2, 1, 294)));
        assert_eq!(parse_version("2.1.294 (Claude Code)"), Some((2, 1, 294)));
        assert_eq!(parse_version("v2.1.300-beta.1"), Some((2, 1, 300)));
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("claude"), None);
        assert_eq!(version_supported("2.1.294"), Some(true));
        assert_eq!(version_supported("2.1.295"), Some(true));
        assert_eq!(version_supported("2.2.0"), Some(true));
        assert_eq!(version_supported("2.1.293"), Some(false));
        assert_eq!(version_supported("2.1.280"), Some(false));
        assert_eq!(version_supported("garbage"), None);
    }

    fn inputs<'a>(dir: &'a Path, cli: &'a Path, version: Option<&'a str>) -> Inputs<'a> {
        Inputs {
            enabled: true,
            ab_off: false,
            plugin_dir: Some(dir),
            cli: Some(cli),
            claude_version: version,
        }
    }

    #[test]
    fn 注入の判断() {
        let dir = Path::new("/d/claude-mod/tako");
        let cli = Path::new("/a/tako");
        assert!(decide(inputs(dir, cli, Some("2.1.294"))).is_on());
        assert_eq!(
            decide(inputs(dir, cli, Some("2.1.280"))),
            Injection::Off(OffReason::ClaudeTooOld("2.1.280".into()))
        );
        assert_eq!(
            decide(inputs(dir, cli, None)),
            Injection::Off(OffReason::ClaudeUnknown)
        );
        assert_eq!(
            decide(inputs(dir, cli, Some(""))),
            Injection::Off(OffReason::ClaudeUnknown)
        );
        assert_eq!(
            decide(Inputs {
                enabled: false,
                ..inputs(dir, cli, Some("2.1.294"))
            }),
            Injection::Off(OffReason::Disabled)
        );
        // A/B は設定より先に効く
        assert_eq!(
            decide(Inputs {
                ab_off: true,
                enabled: false,
                ..inputs(dir, cli, Some("2.1.294"))
            }),
            Injection::Off(OffReason::AbOff)
        );
        assert_eq!(
            decide(Inputs {
                plugin_dir: None,
                ..inputs(dir, cli, Some("2.1.294"))
            }),
            Injection::Off(OffReason::NotInstalled)
        );
        assert_eq!(
            decide(Inputs {
                cli: None,
                ..inputs(dir, cli, Some("2.1.294"))
            }),
            Injection::Off(OffReason::NoCli)
        );
    }

    fn on() -> Injection {
        Injection::On {
            plugin_dir: PathBuf::from("/d/claude-mod/tako"),
            cli: PathBuf::from("/a/tako"),
        }
    }

    #[test]
    fn 注入は既存の値を上書きせず連結する() {
        assert_eq!(
            pane_env(&on(), None, None, ':'),
            vec![
                (PLUGIN_DIRS_ENV.into(), "/d/claude-mod/tako".into()),
                (CLI_ENV.into(), "/a/tako".into()),
            ]
        );
        assert_eq!(
            pane_env(&on(), Some("/mine/p1:/mine/p2"), None, ':')[0].1,
            "/d/claude-mod/tako:/mine/p1:/mine/p2"
        );
        // Windows の区切り
        let win = Injection::On {
            plugin_dir: PathBuf::from(r"C:\d\claude-mod\tako"),
            cli: PathBuf::from(r"C:\a\tako.exe"),
        };
        assert_eq!(
            pane_env(&win, Some(r"D:\mine"), None, ';')[0].1,
            r"C:\d\claude-mod\tako;D:\mine"
        );
        // 親（tako のペインから立てた tako）の mod は二重に載せない
        assert_eq!(
            pane_env(&on(), Some("/other/data/claude-mod/tako/:/mine"), None, ':')[0].1,
            "/d/claude-mod/tako:/mine"
        );
    }

    #[test]
    fn 注入しないときは継承したtakoの値だけを打ち消す() {
        let off = Injection::Off(OffReason::Disabled);
        // 何も継承していなければ何も足さない（旧挙動と同じ）
        assert!(pane_env(&off, None, None, ':').is_empty());
        assert!(pane_env(&off, Some("/mine"), None, ':').is_empty());
        // 親の mod を継承していたら打ち消す（利用者自身の分は残す）
        assert_eq!(
            pane_env(&off, Some("/p/claude-mod/tako:/mine"), Some("/p/tako"), ':'),
            vec![
                (PLUGIN_DIRS_ENV.into(), "/mine".into()),
                (CLI_ENV.into(), String::new()),
            ]
        );
        assert_eq!(
            neutral_pairs(None, ':'),
            vec![
                (PLUGIN_DIRS_ENV.into(), String::new()),
                (CLI_ENV.into(), String::new()),
            ]
        );
    }

    fn report_json() -> serde_json::Value {
        serde_json::json!({
            "schema": 1,
            "mod_version": "0.8.28",
            "claude_version": "2.1.294",
            "session_id": "s-1",
            "at": 1_791_452_867_009u64,
            "model": "claude-haiku-5-5",
            "effort": "medium",
            "context": {"tokens": 59426, "window": 1000000, "percent": 6},
            "rate_limits": [{"kind": "five_hour", "percent_used": 3, "resets_at": "2026-10-08T14:30:00.000Z", "observed_at": 1_791_452_867_009u64}],
            "cost_usd": 0.00698652,
            "turn": "permission",
            "pending_tool": "Bash",
            "last_turn": {"duration_ms": 1193, "reason": "answer"},
            "config_dir": "/cfg",
            "ended": false,
            // 本文系のキーは serde が捨てる（tako のメモリに残らない）
            "text": "secret",
            "tool_input": {"command": "secret"},
        })
    }

    #[test]
    fn 報告を読み本文系のキーは残さない() {
        let report = parse_report(report_json()).unwrap();
        assert_eq!(report.turn, ModTurn::Permission);
        assert_eq!(report.pending_tool.as_deref(), Some("Bash"));
        assert_eq!(report.context.as_ref().unwrap().percent, Some(6));
        let back = serde_json::to_string(&report).unwrap();
        assert!(!back.contains("secret"), "本文が残った: {back}");
        assert!(!back.contains("/cfg"), "config_dir を外へ出した: {back}");
    }

    #[test]
    fn 欠けた値は未観測のまま読む() {
        let report = parse_report(serde_json::json!({
            "schema": 1, "mod_version": "x", "at": 1, "turn": "idle",
            "context": {"window": 1000000}, "rate_limits": [],
        }))
        .unwrap();
        let ctx = report.context.unwrap();
        assert_eq!((ctx.tokens, ctx.percent), (None, None));
        assert!(!report.ended);
    }

    #[test]
    fn 形の違う報告は断る() {
        let mut v = report_json();
        v["schema"] = 2.into();
        assert!(parse_report(v).unwrap_err().contains("schema=2"));
        let mut v = report_json();
        v["turn"] = "thinking".into();
        assert!(parse_report(v).is_err());
        assert!(parse_report(serde_json::json!({"turn": "idle"})).is_err());
    }

    #[test]
    fn 長すぎる文字列は切り詰める() {
        let mut v = report_json();
        v["pending_tool"] = "ツ".repeat(500).into();
        let report = parse_report(v).unwrap();
        assert!(report.pending_tool.unwrap().len() <= MAX_TEXT);
    }

    #[test]
    fn 鮮度は45秒で切れendedで即座に捨てる() {
        let t0 = Instant::now();
        assert!(is_fresh(t0, t0 + Duration::from_secs(45)));
        assert!(!is_fresh(t0, t0 + Duration::from_secs(46)));
        let mut hub = ModHub::new(true);
        let report = parse_report(report_json()).unwrap();
        assert_eq!(hub.accept(3, report.clone(), t0), Accepted::Stored);
        assert_eq!(hub.accept(3, report.clone(), t0), Accepted::Stored);
        assert_eq!(hub.reports[&3].count, 2);
        assert!(hub.fresh_report(3, t0 + Duration::from_secs(30)).is_some());
        assert!(hub.fresh_report(3, t0 + Duration::from_secs(60)).is_none());
        let mut ended = report;
        ended.ended = true;
        assert_eq!(hub.accept(3, ended, t0), Accepted::Ended);
        assert!(hub.fresh_report(3, t0).is_none());
    }

    #[test]
    fn 閉じたペインの記録は残さない() {
        let mut hub = ModHub::new(true);
        hub.record_spawn(5, &on());
        hub.accept(5, parse_report(report_json()).unwrap(), Instant::now());
        hub.forget_pane(5);
        assert!(hub.injections.is_empty() && hub.reports.is_empty());
    }

    #[test]
    fn modの置き場の見分け() {
        assert!(is_tako_mod_entry("/x/claude-mod/tako"));
        assert!(is_tako_mod_entry("/x/claude-mod/tako/"));
        assert!(is_tako_mod_entry(r"C:\x\claude-mod\tako"));
        assert!(!is_tako_mod_entry("/x/claude-mod/other"));
        assert!(!is_tako_mod_entry("/x/tako"));
    }
}
