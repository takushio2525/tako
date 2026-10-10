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
//! - **一次ソース（S2 #1880）**: 新鮮な報告は ctx%・使用制限・ターン状態の**先頭**に来る。
//!   引き当ては [`lookup`] の 1 本で、使えないときは理由（[`ModUnavailable`]）を返して
//!   呼び出し側が今の経路（画面 / transcript）へ落ちる。使用制限は**アカウント単位**なので
//!   [`ModHub::account_rate_limits`] で束ねる（最新の観測が勝つ。#1903）。**上限での停止は
//!   画面でしか判定しない**（[`limit_reset_at`] は解除時刻の手がかりだけを返す = #813 の安全条件）。
//!   画面下のステータスバーの 5h / 7d も同じ束ねた値を先に見る（[`bar_limits`]。#1903）
//! - **画面に出す（S3 #1881）**: 報告の応答に帯・サイドバーの材料（[`BandView`]）を載せる。
//!   何を出すか（worker の状態・要注意・閾値を超えた ctx / 使用制限）は [`band_view`] が決め、
//!   mod は幅に合わせて 1 行に詰めて描くだけ。帯を隠すトグルの正本は ui.json の `band.hidden`
//!   （#1960。[`crate::claude_mod_ui`]）で、応答の `band_request` で全 mod へ中継する
//! - **定型の UI 設定（S7-2 #1960）**: 報告の応答の `tako.view.ui` に検証済みの ui.json を載せる
//!   （[`BandView::ui`]。mod はファイルを読まない）
//! - **描画の主を mod へ（S7-3 #1962）**: 帯は他の mod の行を包んで 1 行（ボタン込み）、使用制限・ctx の
//!   バーは入力欄の下の行の末尾。描くか（[`usage_bar_decision`]）とボタンの CLI の引数は tako が決めて
//!   `view` に載せ、mod は描いたものを報告の `renders`（[`ModRenders`]）で返す。フォーカス中のペインの
//!   mod がバーを描いていれば、ステータスバーの claude の区画を引っ込める（[`claude_bar_owner`]）
//!
//! このモジュールは**純関数と素のデータだけ**を持つ（GUI 非依存。判断はここで閉じ、
//! tako-app は値を渡して結果を env へ足すだけにする）。

use std::collections::{BTreeMap, BTreeSet, HashMap};
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
/// 報告を**一次ソースとして見ない** A/B の入口（S2 #1880）。注入と報告の受け取りは続けるが、
/// ctx%・使用制限・ターン状態は #1880 前の経路（画面 / transcript）だけで決める
pub const S2_LEGACY_ENV: &str = "TAKO_1877_S2_LEGACY";
/// 使用制限の束ね方とステータスバーの 5h / 7d を **#1903 前**へ戻す A/B の入口。
/// 束ね方は S2 の「`resets_at` が遅い → % が大きい」（[`ModHub::account_rate_limits_by`]）、
/// ステータスバーは画面の値だけ（[`bar_limits`] を通さない）。報告の受け取りと S2 の一次ソース化は続ける
pub const LIMITS_LEGACY_ENV: &str = "TAKO_1903_LEGACY";
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
/// 帯・サイドバーを描かない A/B の入口（S3 #1881）。報告の受け取りと一次ソース化（S1 / S2）は
/// 続けるが、`tako mod report` の応答に帯・サイドバーの材料（`view`）を載せない = mod は何も描かない
pub const S3_LEGACY_ENV: &str = "TAKO_1877_S3_LEGACY";
/// 帯を S3（#1881）の描き方へ戻す A/B の入口（S7-3 #1962）。応答の `view` に `band_style: "s3"` を
/// 載せ、バーの判断（`usage_bar`）とボタンの引数（`button_args`）を載せない = mod は `next(e)` を
/// 包まず、ボタンとバーを描かず、`renders` を報告しない。ステータスバーの claude の区画も止めない
pub const S7_LEGACY_ENV: &str = "TAKO_1877_S7_LEGACY";

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
    /// ペインの claude の設定 dir に別の出どころの同名 `tako` がある（S7-1 #1959）。
    /// inline（env）は installed / skills-dir より先に読まれるので、注入すると利用者の `tako` を潰す。
    /// 中身は衝突の説明（[`crate::claude_mod_install::Conflict::describe`]）。`Box<str>` にするのは
    /// `String` の変種が 2 つになると enum にタグが要って 8 バイト増え、`ModSnapshot` 越しに
    /// `OffloadJob` の変種の大きさの差が clippy の閾値を越えるため（単体テストで大きさを固定）
    NameConflict(Box<str>),
    /// 利用者が Claude Code 側で tako mod を止めた（`/plugin` の disable。#1959）
    UserDisabled,
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
            OffReason::NameConflict(_) => "name_conflict",
            OffReason::UserDisabled => "user_disabled",
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
            OffReason::NameConflict(why) => {
                format!("{why}（利用者の tako を優先し、このペインには注入しない）")
            }
            OffReason::UserDisabled => {
                "Claude Code の /plugin で tako@skills-dir が止められている（利用者の選択を優先）"
                    .into()
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

/// S2 の A/B（[`S2_LEGACY_ENV`]）が立っているか
pub fn s2_legacy() -> bool {
    std::env::var_os(S2_LEGACY_ENV).is_some_and(|v| !v.is_empty())
}

/// S3 の A/B（[`S3_LEGACY_ENV`]）が立っているか
pub fn s3_legacy() -> bool {
    std::env::var_os(S3_LEGACY_ENV).is_some_and(|v| !v.is_empty())
}

/// S7-3 の A/B（[`S7_LEGACY_ENV`]）が立っているか
pub fn s7_legacy() -> bool {
    std::env::var_os(S7_LEGACY_ENV).is_some_and(|v| !v.is_empty())
}

/// #1903 の A/B（[`LIMITS_LEGACY_ENV`]）が立っているか
pub fn limits_legacy() -> bool {
    std::env::var_os(LIMITS_LEGACY_ENV).is_some_and(|v| !v.is_empty())
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

    /// 入力を待っている（権限ダイアログか質問が出ている）と mod が言っているか
    pub fn awaits_answer(self) -> bool {
        matches!(self, ModTurn::Permission | ModTurn::Question)
    }

    /// `worker_status` の語彙（idle / busy / waiting）へ写す（S2 #1880）。
    ///
    /// `permission` / `question` は**承認・回答の後もそのツールが返るまで残る**
    /// （mod API に承認の瞬間を知らせるイベントが無い = FR-2.42.7）。承認後のツール実行中は
    /// 画面が生成中を描くので、`screen_busy` なら busy を採る（待っていないものを waiting と
    /// 言うと、master が存在しないダイアログへ respond しに行く）
    pub fn status_word(self, screen_busy: bool) -> &'static str {
        match self {
            ModTurn::Idle => "idle",
            ModTurn::Busy => "busy",
            ModTurn::Permission | ModTurn::Question if screen_busy => "busy",
            ModTurn::Permission | ModTurn::Question => "waiting",
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
    /// この値を**観測した**時刻（epoch ms）。mod は % か `resets_at` が変わったときだけ打ち直す
    /// （#1903。heartbeat で打ち直すと放置したペインの古い値が「今」の観測に見える）。
    /// 報告の鮮度は tako の受信時刻（[`StoredReport::received`]）で別に測る
    pub observed_at: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModLastTurn {
    pub duration_ms: u64,
    pub reason: String,
}

/// 帯（プロンプトの上の 1 行）の状態（S3 #1881。mod が描いたもの）。
///
/// **描いた文字列は持たない**: 何を描いたかは区切りの種類（`segments`）だけで表す
/// （ペイン名・タブ名は tako が渡したもので、tako 側が既に知っている）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModBand {
    /// 利用者が帯を隠している（`$.store` に保存したトグル = 再起動後も保たれる）
    pub hidden: bool,
    /// 直近の描画で帯を描いたか（隠している・tako の材料が無い / 古い・調査票が帯を使っている
    /// ときは false）。**権限ダイアログ・質問の表示中は Claude Code が帯ごと隠す**ので、
    /// そのあいだ描画は呼ばれず、この値は最後に描いたときのまま
    pub shown: bool,
    /// 直近の描画で帯の本文に使えた桁（`AbovePrompt` の `bodyColumns`）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub columns: Option<u32>,
    /// 描いた区切りの種類（左から。`tako` / `pane` / `tab` / `workers` / `attention` / `ctx` /
    /// 使用制限の窓の種類）。幅が足りないと優先度の低い順に落ちる
    #[serde(default)]
    pub segments: Vec<String>,
    /// トグルを最後に変えた時刻（epoch ms。`tako mod band` の中継と突き合わせる）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toggled_at: Option<u64>,
}

/// mod が Claude Code の画面に描いているもの（S7-3 #1962。報告の `renders`）。
///
/// **無い = 何も描いていない**（S7-3 前の mod・A/B の `TAKO_1877_S7_LEGACY`）。tako 側の表示を止める
/// 条件（[`claude_bar_owner`]）はこれを見る = mod が描いたと言ったものだけを tako が引っ込める
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModRenders {
    /// 直近の帯の描画で tako の行を描いたか
    #[serde(default)]
    pub band: bool,
    /// 使用制限・ctx のバーを描いた置き場（`prompt_hint` / `band`）。描いていなければ `None`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_bar: Option<String>,
    /// 直近の帯の描画で描いたボタンの数
    #[serde(default)]
    pub buttons: u32,
    /// 起きてから帯（`AbovePrompt`）のフックが 1 度でも呼ばれたか
    #[serde(default)]
    pub band_hook: bool,
    /// 起きてから入力欄の下の行（`PromptHint`）のフックが 1 度でも呼ばれたか。これが真で
    /// [`Self::band_hook`] が偽なら、外側の他の mod が `next` を呼ばずに帯を描いている
    /// （[`band_hidden_by_other_mod`]）
    #[serde(default)]
    pub hint_hook: bool,
}

/// ボタンを押した結果（S7-3 #1962。報告の `last_press`）。**語彙の種類と成否だけ**を載せる
/// （ラベル・コマンド・入力欄へ入れる文は載せない）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModPress {
    /// `slash` / `tako` / `shell` / `prompt`
    pub kind: String,
    pub ok: bool,
    /// 押した時刻（epoch ms）
    pub at: u64,
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
    /// 帯の状態（S3 #1881。S3 前の mod は送らない）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub band: Option<ModBand>,
    /// mod が休眠に入った理由（S7-1 #1959。いまは `user_disabled` = 利用者が Claude Code 側で
    /// `tako@skills-dir` を止めた）。これを最後に mod は報告を止める
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dormant: Option<String>,
    /// 利用者の Claude Code に statusLine が設定されているか（S7-3 #1962。`$.settings.read()` の
    /// `statusLine` の有無だけで、中身は載せない）。画面の読み取りと合わせてバーを描くかを決める
    /// （[`usage_bar_decision`]）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_line: Option<bool>,
    /// mod が描いているもの（S7-3 #1962。無い = 何も描いていない）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renders: Option<ModRenders>,
    /// 最後に押されたボタン（S7-3 #1962）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_press: Option<ModPress>,
    #[serde(default)]
    pub ended: bool,
}

impl ModReport {
    /// ctx% の材料（`ctx_usage::resolve_full` の mod の段）
    pub fn ctx(&self) -> crate::ctx_usage::ModCtx {
        crate::ctx_usage::ModCtx {
            percent: self.context.as_ref().and_then(|c| c.percent),
            tokens: self.context.as_ref().and_then(|c| c.tokens),
            // 0 は「窓が分からない」と同じ（0 除算を作らない）
            window: self.context.as_ref().map(|c| c.window).filter(|w| *w > 0),
            model: self.model.clone(),
        }
    }
}

/// 文字列の上限（ツール名・モデル名など。おかしな値でメモリを食わせない）
const MAX_TEXT: usize = 256;
/// 使用制限の窓の数の上限
const MAX_RATE_LIMITS: usize = 16;
/// 帯の区切りの数の上限
const MAX_BAND_SEGMENTS: usize = 16;

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
        &mut report.dormant,
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
    if let Some(band) = &mut report.band {
        band.segments.truncate(MAX_BAND_SEGMENTS);
        for segment in &mut band.segments {
            clip(segment);
        }
    }
    if let Some(place) = report.renders.as_mut().and_then(|r| r.usage_bar.as_mut()) {
        clip(place);
    }
    if let Some(press) = &mut report.last_press {
        clip(&mut press.kind);
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

/// mod の報告を一次ソースに使えない理由（S2 #1880。応答の `ctx_mod_reason` / `mod_reason`）。
///
/// コードの語彙は `tako mod`（status）の行の `reason.code` と同じ（[`OffReason::code`] を含む）。
/// 読み手が「なぜ画面へ落ちたか」を 1 つの語彙で追えるようにする
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModUnavailable {
    /// [`S2_LEGACY_ENV`]（A/B。報告を見ない旧挙動）
    Legacy,
    /// 報告が 1 度も来ていない（mod が読まれていない・注入より前から動いていた claude・
    /// claude ではないペイン・mod を扱わない tako）
    Absent,
    /// 最後の報告から [`FRESH_FOR`] を過ぎた（claude が止まった・mod が止まった）
    Stale { age_secs: u64 },
    /// このペインには注入しなかった（`tako mod off` / 版の下限未満 等）
    NotInjected(OffReason),
}

impl ModUnavailable {
    pub fn code(&self) -> &'static str {
        match self {
            ModUnavailable::Legacy => "legacy_env",
            ModUnavailable::Absent => "mod_absent",
            ModUnavailable::Stale { .. } => "mod_stale",
            ModUnavailable::NotInjected(reason) => reason.code(),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            ModUnavailable::Legacy => {
                format!("{S2_LEGACY_ENV} が立っている（mod の報告を見ない旧挙動）")
            }
            ModUnavailable::Absent => "mod から報告が無い".into(),
            ModUnavailable::Stale { age_secs } => format!(
                "最後の報告から {age_secs} 秒（{} 秒で失効）",
                FRESH_FOR.as_secs()
            ),
            ModUnavailable::NotInjected(reason) => reason.describe(),
        }
    }
}

/// ペイン 1 つぶんの報告を一次ソースとして引き当てる（S2 #1880。**引き当てはこの 1 本**）。
///
/// `hub` が `None`（mod を扱わない tako）なら [`ModUnavailable::Absent`]。
/// `legacy` は [`s2_legacy`] の値（テストは env を触らずに両アームを検査できる）
pub fn lookup(
    hub: Option<&ModHub>,
    pane: u64,
    now: Instant,
    legacy: bool,
) -> Result<&StoredReport, ModUnavailable> {
    if legacy {
        return Err(ModUnavailable::Legacy);
    }
    hub.ok_or(ModUnavailable::Absent)?.lookup(pane, now)
}

/// [`lookup`] の結果を ctx% の材料へ
pub fn ctx_input(
    looked_up: &Result<&StoredReport, ModUnavailable>,
) -> crate::ctx_usage::ModCtxInput {
    match looked_up {
        Ok(stored) => crate::ctx_usage::ModCtxInput::Fresh(stored.report.ctx()),
        Err(why) => crate::ctx_usage::ModCtxInput::Unavailable(why.code()),
    }
}

/// mod の `resets_at`（ISO 8601。`2026-10-08T14:30:00.000Z` / `…+09:00`）→ unix 秒。
/// **秒精度**（小数部は捨てる）。タイムゾーンの無い表記・読めない表記は `None`
/// （ローカル時刻と取り違えて 9 時間ずれた復帰予定を作らない）
pub fn parse_resets_at(text: &str) -> Option<i64> {
    let (date, rest) = text.trim().split_once('T')?;
    let mut ymd = date.split('-');
    let (y, m, d): (i64, i64, i64) = (
        ymd.next()?.parse().ok()?,
        ymd.next()?.parse().ok()?,
        ymd.next()?.parse().ok()?,
    );
    if ymd.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let (clock, offset) = if let Some(c) = rest.strip_suffix(['Z', 'z']) {
        (c, 0)
    } else {
        let at = rest.rfind(['+', '-'])?;
        let (c, tz) = rest.split_at(at);
        let sign = if tz.starts_with('-') { -1 } else { 1 };
        let (oh, om) = tz[1..].split_once(':')?;
        let (oh, om): (i64, i64) = (oh.parse().ok()?, om.parse().ok()?);
        if oh > 23 || om > 59 {
            return None;
        }
        (c, sign * (oh * 3_600 + om * 60))
    };
    let clock = clock.split('.').next()?;
    let mut hms = clock.split(':');
    let (hh, mm, ss): (i64, i64, i64) = (
        hms.next()?.parse().ok()?,
        hms.next()?.parse().ok()?,
        hms.next()?.parse().ok()?,
    );
    if hms.next().is_some() || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let days = crate::limit_resume::days_from_civil(y, m, d);
    Some(days * 86_400 + hh * 3_600 + mm * 60 + ss - offset)
}

/// 上限に当たっている窓（`percent_used >= 100`）が解ける時刻（unix 秒）。
/// 複数当たっていれば**遅い方**（両方解けるまで続けられない）。当たっていなければ `None`。
///
/// **これは解除時刻の手がかりで、停止の判定ではない**: 停止は画面でしか判定しない
/// （`limit_stop::detect_limit_stop_with` の `LimitHint`。#813 の安全条件 = codex #985 と同じ型）
pub fn limit_reset_at(limits: &[ModRateLimit]) -> Option<i64> {
    limits
        .iter()
        .filter(|l| l.percent_used >= 100.0)
        .filter_map(|l| l.resets_at.as_deref().and_then(parse_resets_at))
        .max()
}

/// 同じ種類の窓の 2 つの観測のうち `a` を採るか（[`ModHub::account_rate_limits_by`] の順序）
fn newer_limit(a: &ModRateLimit, b: &ModRateLimit, legacy: bool) -> bool {
    let resets = |l: &ModRateLimit| l.resets_at.as_deref().and_then(parse_resets_at);
    let order = if legacy {
        resets(a)
            .cmp(&resets(b))
            .then(a.percent_used.total_cmp(&b.percent_used))
            .then(a.observed_at.cmp(&b.observed_at))
    } else {
        a.observed_at
            .cmp(&b.observed_at)
            .then(resets(a).cmp(&resets(b)))
            .then(a.percent_used.total_cmp(&b.percent_used))
    };
    order.is_gt()
}

/// ステータスバーの 5h / 7d の取得元（#1903。`tako limit-service --refresh` の `claude.source`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarLimitSource {
    /// tako mod の報告（アカウント単位で束ねた値）
    Mod,
    /// claude の画面（statusLine の `5h NN%` / `7d NN%`。#217）
    Screen,
}

impl BarLimitSource {
    pub fn as_str(self) -> &'static str {
        match self {
            BarLimitSource::Mod => "mod",
            BarLimitSource::Screen => "screen",
        }
    }
}

/// ステータスバーの 5h / 7d メーターに出す値
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BarLimits {
    pub five_hour: Option<u32>,
    pub seven_day: Option<u32>,
    /// どちらも無ければ `None`
    pub source: Option<BarLimitSource>,
}

/// ステータスバーの 5h / 7d を決める（#1903。**判断はこの 1 本**）。
///
/// `mod_limits` は束ね済みの使用制限（[`ModHub::account_rate_limits`]。呼び出し側がフォーカス順で
/// 最初に引けたペインのものを渡す）。`five_hour` / `seven_day` の窓が 1 つでもあれば **mod だけで**
/// 決め（画面の値と窓ごとに混ぜない = 出どころを 1 つに保つ）、無ければ画面の値のまま。
/// % は帯（S3）と同じく四捨五入（mod は小数 1 桁まで送る）
pub fn bar_limits(
    mod_limits: &[ModRateLimit],
    screen_five_hour: Option<u32>,
    screen_seven_day: Option<u32>,
) -> BarLimits {
    let percent = |kind: &str| {
        mod_limits
            .iter()
            .find(|l| l.kind == kind)
            // f64 → u32 の `as` は飽和する（負・NaN は 0）
            .map(|l| l.percent_used.round() as u32)
    };
    let (five_hour, seven_day) = (percent("five_hour"), percent("seven_day"));
    if five_hour.is_some() || seven_day.is_some() {
        return BarLimits {
            five_hour,
            seven_day,
            source: Some(BarLimitSource::Mod),
        };
    }
    BarLimits {
        five_hour: screen_five_hour,
        seven_day: screen_seven_day,
        source: (screen_five_hour.is_some() || screen_seven_day.is_some())
            .then_some(BarLimitSource::Screen),
    }
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
    /// mod が休眠を告げたペイン → 理由（S7-1 #1959。報告の `dormant`）。休眠した mod はもう
    /// 報告しないので鮮度では消さず、ペインが閉じるか次の報告が来るまで `tako mod` の行に出す
    pub dormant: HashMap<u64, String>,
    /// mod の報告で見えた設定 dir（`CLAUDE_CONFIG_DIR` の値。S7-1 #1959）。既定の dir は
    /// `config_dir` 無しで届くので入らない（既定は初めから置く先に入っている）
    pub reported_config_dirs: BTreeSet<String>,
    /// [`Self::reported_config_dirs`] のうち、まだ `skills/tako` を置きに行っていないもの
    /// （GUI が背景で片付ける = 起動時の差分検出の 2 段目）
    pub pending_config_dirs: Vec<String>,
}

/// [`ModHub::accept`] の結果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accepted {
    Stored,
    /// `ended: true` を受けて捨てた
    Ended,
    /// `dormant` を受けた（mod は休眠に入り、これを最後に報告を止める。#1959）
    Dormant,
}

impl Accepted {
    pub fn as_str(self) -> &'static str {
        match self {
            Accepted::Stored => "stored",
            Accepted::Ended => "ended",
            Accepted::Dormant => "dormant",
        }
    }
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
        if let Some(dir) = report.config_dir.as_ref().filter(|d| !d.is_empty()) {
            if self.reported_config_dirs.insert(dir.clone()) {
                self.pending_config_dirs.push(dir.clone());
            }
        }
        if report.ended {
            self.reports.remove(&pane);
            return Accepted::Ended;
        }
        if let Some(why) = report.dormant {
            // 休眠した mod の値は以後更新されないので一次ソースに使わない（画面の読み取りへ落ちる）
            self.reports.remove(&pane);
            self.dormant.insert(pane, why);
            return Accepted::Dormant;
        }
        self.dormant.remove(&pane);
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

    /// 一次ソースとしての引き当て（[`lookup`] の本体）。新鮮な報告 → 古い報告 →
    /// 注入の記録の順に見る（注入しなかったペインでも、利用者が自分で読ませた mod の報告が
    /// 新鮮なら使う = 値そのものは正しい）
    pub fn lookup(&self, pane: u64, now: Instant) -> Result<&StoredReport, ModUnavailable> {
        if let Some(stored) = self.reports.get(&pane) {
            if is_fresh(stored.received, now) {
                return Ok(stored);
            }
            return Err(ModUnavailable::Stale {
                age_secs: now.saturating_duration_since(stored.received).as_secs(),
            });
        }
        match self.injections.get(&pane).and_then(|i| i.off.clone()) {
            Some(off) => Err(ModUnavailable::NotInjected(off)),
            None => Err(ModUnavailable::Absent),
        }
    }

    /// このペインのアカウント（`config_dir`）の使用制限を、同じアカウントの新鮮な報告すべてから
    /// 束ねる（窓の種類ごとに 1 つ。使用制限は**アカウント単位**の値 = 設計書 §4.2）。
    ///
    /// 窓の種類ごとに**最新の観測**（`observed_at` が新しい）を採る（設計書 §6。#1903）。
    /// mod は値が変わったときだけ `observed_at` を打つ（heartbeat では打ち直さない）ので、
    /// 1 時間放置したペインの古い % は 1 時間前の観測のまま届き、動いているペインの値が勝つ。
    /// 同じ時刻なら `resets_at` が遅い（= 新しい窓）→ % が大きい方
    pub fn account_rate_limits(&self, pane: u64, now: Instant) -> Vec<ModRateLimit> {
        self.account_rate_limits_by(pane, now, limits_legacy())
    }

    /// [`Self::account_rate_limits`] の本体。`legacy`（[`LIMITS_LEGACY_ENV`]）は S2（#1880）の
    /// 束ね方 =「`resets_at` が遅い → % が大きい → `observed_at`」（mod が heartbeat のたびに
    /// 今の時刻を打っていた頃の回避策）。テストは env を触らずに両アームを見る
    pub fn account_rate_limits_by(
        &self,
        pane: u64,
        now: Instant,
        legacy: bool,
    ) -> Vec<ModRateLimit> {
        let Some(own) = self.fresh_report(pane, now) else {
            return Vec::new();
        };
        let account = own.config_dir.as_deref();
        let mut best: std::collections::BTreeMap<String, ModRateLimit> =
            std::collections::BTreeMap::new();
        for stored in self.reports.values() {
            if !is_fresh(stored.received, now) || stored.report.config_dir.as_deref() != account {
                continue;
            }
            for limit in &stored.report.rate_limits {
                let replace = best
                    .get(&limit.kind)
                    .is_none_or(|cur| newer_limit(limit, cur, legacy));
                if replace {
                    best.insert(limit.kind.clone(), limit.clone());
                }
            }
        }
        best.into_values().collect()
    }

    /// 閉じたペインの記録を捨てる（ペイン ID が再利用されたとき前任の報告を名乗らない）
    pub fn forget_pane(&mut self, pane: u64) {
        self.injections.remove(&pane);
        self.reports.remove(&pane);
        self.dormant.remove(&pane);
    }

    /// まだ置きに行っていない、報告で見えた設定 dir を取り出す（#1959）
    pub fn take_pending_config_dirs(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_config_dirs)
    }
}

// --- 帯とサイドバー（S3 #1881）-------------------------------------------------------
//
// mod は `tako mod report` の応答に載った [`BandView`] で、帯（プロンプトの上の 1 行）と
// `/tako` のサイドバーを描く。**何を出すかの判断はここ**（worker の状態・要注意・閾値）。
// mod は幅に合わせて詰めるだけにする（判断を TS 側に散らさない）

/// 帯に ctx% を出す閾値（%）。チャット表示の残量バーが警告色になる値と同じ。
/// 閾値未満では出さない（statusLine を持つ利用者の画面で情報が二重にならない既定）
pub const BAND_CTX_PERCENT: u32 = 80;
/// 帯に使用制限（5h / 7d 等）を出す閾値（%）
pub const BAND_LIMIT_PERCENT: f64 = 80.0;
/// 応答に並べる worker の上限（数は [`BandView::worker_count`] で全部数える = 応答を太らせない）
pub const BAND_MAX_WORKERS: usize = 24;
/// 検証用: 帯に ctx / 使用制限を出す閾値（%）を両方まとめて差し替える（`0` で常に出す =
/// 実画面で帯の区切りを全部並べて 1 行に収まるかを測る。`scripts/test-claude-mod-band-1881.sh`）
pub const BAND_THRESHOLD_ENV: &str = "TAKO_1881_BAND_THRESHOLD";

/// 帯の閾値（既定は [`BAND_CTX_PERCENT`] / [`BAND_LIMIT_PERCENT`]。[`BAND_THRESHOLD_ENV`] で差し替え）
pub fn band_thresholds() -> BandThresholds {
    match std::env::var(BAND_THRESHOLD_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
    {
        Some(p) => BandThresholds {
            ctx_percent: p,
            limit_percent: f64::from(p),
        },
        None => BandThresholds {
            ctx_percent: BAND_CTX_PERCENT,
            limit_percent: BAND_LIMIT_PERCENT,
        },
    }
}

/// 帯・サイドバーに出す worker の状態
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerState {
    Busy,
    Idle,
    /// 権限ダイアログ・質問・選択肢ダイアログで止まっている
    Waiting,
    /// 使用制限に当たっている
    Limited,
    /// エージェントが非ゼロで終わってシェルへ戻った
    Failed,
    Unknown,
}

/// 要注意（人の手が要る）の理由
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Attention {
    /// 権限ダイアログ（mod か画面）
    Permission,
    /// AskUserQuestion（mod）
    Question,
    /// そのほかの選択肢ダイアログ（画面から。mod の無い worker = codex / agy 等も拾う）
    Dialog,
    Limited,
    Failed,
}

/// worker 1 本の手掛かり（tako-control がホストから集める。判断は [`classify_worker`]）
#[derive(Debug, Clone, Default)]
pub struct WorkerFacts {
    pub pane: u64,
    /// 表示名（ペインのタイトル → role → `pane N`）
    pub name: String,
    /// 新鮮な mod の報告のターン（無い・古い・A/B なら None）
    pub mod_turn: Option<ModTurn>,
    /// そのアカウントの使用制限が上限（100%）に当たっている（mod の値）
    pub limited: bool,
    /// 画面に人の答えを待つ選択肢ダイアログがある（tako が自動で答える trust / bypass は除く）
    pub dialog_on_screen: bool,
    /// そのダイアログが権限ダイアログ（ツール実行の承認）
    pub permission_on_screen: bool,
    /// 画面の選択肢ダイアログが使用制限の対処の選択
    pub limit_dialog_on_screen: bool,
    /// 画面が生成中
    pub screen_busy: bool,
    /// シェル統合のコマンド状態（エージェントが動いている間は Running）
    pub command: crate::CommandState,
}

/// worker の状態と要注意の理由を決める（純関数）。
///
/// 順序: 使用制限 → mod の待ち（承認 / 質問）→ 画面のダイアログ → mod の busy / idle →
/// コマンド状態。mod の `permission` / `question` は承認・回答の後もツールが返るまで残るので、
/// 画面が生成中なら busy を採る（[`ModTurn::status_word`] と同じ規則）。**画面のダイアログは
/// mod の busy / idle より先**に見る: 組織アカウントでは classic 系が届かず、ルールの無い ask の
/// 権限ダイアログを mod が拾えない（FR-2.42.7。S2 の `worker_status` も画面を正にする）
pub fn classify_worker(f: &WorkerFacts) -> (WorkerState, Option<Attention>) {
    if f.limited || f.limit_dialog_on_screen {
        return (WorkerState::Limited, Some(Attention::Limited));
    }
    let mod_word = f
        .mod_turn
        .map(|turn| (turn, turn.status_word(f.screen_busy)));
    match mod_word {
        Some((ModTurn::Question, "waiting")) => {
            return (WorkerState::Waiting, Some(Attention::Question));
        }
        Some((_, "waiting")) => return (WorkerState::Waiting, Some(Attention::Permission)),
        _ => {}
    }
    if f.permission_on_screen {
        return (WorkerState::Waiting, Some(Attention::Permission));
    }
    if f.dialog_on_screen {
        return (WorkerState::Waiting, Some(Attention::Dialog));
    }
    match mod_word {
        Some((_, "busy")) => return (WorkerState::Busy, None),
        Some(_) => return (WorkerState::Idle, None),
        None => {}
    }
    match f.command {
        crate::CommandState::Failed(_) => (WorkerState::Failed, Some(Attention::Failed)),
        _ if f.screen_busy => (WorkerState::Busy, None),
        crate::CommandState::Running | crate::CommandState::Idle => (WorkerState::Idle, None),
        crate::CommandState::Unknown => (WorkerState::Unknown, None),
    }
}

/// 帯に出す警告（閾値を超えた ctx% と使用制限）
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BandWarning {
    /// `ctx` か使用制限の窓の種類（`five_hour` / `seven_day` …）
    pub kind: String,
    pub percent: f64,
    /// 使用制限の解除時刻（unix 秒）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
}

/// 閾値（既定は [`BAND_CTX_PERCENT`] / [`BAND_LIMIT_PERCENT`]）を超えたものだけを並べる（純関数）。
/// ctx が先、使用制限は % の大きい順
pub fn band_warnings(
    ctx_percent: Option<u32>,
    limits: &[ModRateLimit],
    thresholds: BandThresholds,
) -> Vec<BandWarning> {
    let mut out: Vec<BandWarning> = Vec::new();
    if let Some(p) = ctx_percent.filter(|p| *p >= thresholds.ctx_percent) {
        out.push(BandWarning {
            kind: "ctx".into(),
            percent: f64::from(p),
            resets_at: None,
        });
    }
    let mut over: Vec<&ModRateLimit> = limits
        .iter()
        .filter(|l| l.percent_used >= thresholds.limit_percent)
        .collect();
    over.sort_by(|a, b| b.percent_used.total_cmp(&a.percent_used));
    out.extend(over.into_iter().map(|l| BandWarning {
        kind: l.kind.clone(),
        percent: l.percent_used,
        resets_at: l.resets_at.as_deref().and_then(parse_resets_at),
    }));
    out
}

/// サイドバーの worker の行
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BandWorker {
    pub pane: u64,
    pub name: String,
    pub state: WorkerState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attention: Option<Attention>,
}

/// サイドバーの使用制限の行（閾値に関わらず全部）
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BandLimit {
    pub kind: String,
    pub percent: f64,
    /// 解除時刻（unix 秒）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
}

/// 帯のトグルの中継（応答の `band_request`）。正本は ui.json の `band.hidden` / `toggled_at`
/// （#1960。[`crate::claude_mod_ui::UiConfig::band_request`]）で、`tako mod band on|off`・
/// `tako mod ui set band.hidden`・mod の報告の取り込みのどれで変わっても同じ形で届く
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BandRequest {
    pub hidden: bool,
    /// 頼んだ時刻（epoch ms）。mod は自分の `$.store` の時刻より新しいときだけ従う
    /// （後から利用者が Claude Code の中で切り替えたら、そちらが勝つ）
    pub at: u64,
}

/// 帯に ctx / 使用制限を出す閾値（判断は tako 側。mod へはサイドバーで
/// 「何 % から帯に出るか」を見せるために渡す）
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct BandThresholds {
    pub ctx_percent: u32,
    pub limit_percent: f64,
}

/// 帯・サイドバーの材料（`tako mod report` の応答の `tako.view`。mod の契約は
/// `claude-mod/types/index.d.ts` の `TakoView`。キーを変えるときは両方を同じコミットで直す）
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BandView {
    pub pane: u64,
    /// このペインの名前（無ければ mod が `pane N` と描く）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab_title: Option<String>,
    /// tako の表示言語（`ja` / `en`）
    pub lang: &'static str,
    /// このペインに紐づく worker の数（[`crate::Workspace::workers_of`]）
    pub worker_count: usize,
    /// そのうち要注意の数
    pub attention: usize,
    /// 要注意を先に、最大 [`BAND_MAX_WORKERS`] 本
    pub workers: Vec<BandWorker>,
    /// 帯に出す警告（閾値超えだけ）
    pub warnings: Vec<BandWarning>,
    /// サイドバー用の ctx（閾値に関わらず。mod の報告の値）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ctx: Option<ModContext>,
    /// サイドバー用の使用制限（アカウント単位で束ねた値。閾値に関わらず）
    pub rate_limits: Vec<BandLimit>,
    pub thresholds: BandThresholds,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub band_request: Option<BandRequest>,
    /// 定型の UI 設定（#1960。検証済みの ui.json。mod の契約は `TakoUi`）
    pub ui: crate::claude_mod_ui::UiConfig,
    /// 使用制限・ctx のバーを描くか（S7-3 #1962。[`usage_bar_decision`]。A/B では載せない）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_bar: Option<UsageBarDecision>,
    /// `tako` / `shell` のボタンを押したときに mod が叩く CLI の引数（ボタンの id → tako の後ろの
    /// argv。S7-3 #1962。**組むのは tako** = `claude_mod_ui::button_args`。A/B では載せない）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub button_args: Option<BTreeMap<String, Vec<String>>>,
    /// 帯の描き方。`s3` = S7-3 前（`next` を包まない・ボタンとバーを描かない。A/B の
    /// `TAKO_1877_S7_LEGACY`）。無い = S7-3 の描き方
    #[serde(skip_serializing_if = "Option::is_none")]
    pub band_style: Option<&'static str>,
}

/// [`band_view`] の入力
#[derive(Debug, Clone, Default)]
pub struct BandInput {
    pub pane: u64,
    pub pane_title: Option<String>,
    pub tab_title: Option<String>,
    pub lang: &'static str,
    pub workers: Vec<WorkerFacts>,
    /// このペインの報告の ctx（引き当てられなければ None）
    pub ctx: Option<ModContext>,
    /// このペインのアカウントの使用制限（束ね済み）
    pub rate_limits: Vec<ModRateLimit>,
    pub thresholds: BandThresholds,
    /// 定型の UI 設定（#1960）。帯のトグルの中継（`band_request`）もここから作る
    pub ui: crate::claude_mod_ui::UiConfig,
    /// このペインの報告の `status_line`（利用者の statusLine の有無。S7-3 #1962）
    pub status_line: Option<bool>,
    /// このペインの画面の末尾に ctx / 使用制限の数値が出ているか（mod のバーは除く。
    /// [`screen_shows_usage`]）
    pub screen_shows_usage: bool,
    /// このペインの cwd（`open-cwd` のボタンの引数。分からなければそのボタンは描かれない）
    pub cwd: Option<String>,
    /// A/B の `TAKO_1877_S7_LEGACY`（[`s7_legacy`]）
    pub s7_legacy: bool,
}

impl Default for BandThresholds {
    fn default() -> Self {
        Self {
            ctx_percent: BAND_CTX_PERCENT,
            limit_percent: BAND_LIMIT_PERCENT,
        }
    }
}

/// 帯・サイドバーの材料を組む（純関数）
pub fn band_view(input: BandInput) -> BandView {
    let mut workers: Vec<BandWorker> = input
        .workers
        .iter()
        .map(|f| {
            let (state, attention) = classify_worker(f);
            BandWorker {
                pane: f.pane,
                name: f.name.clone(),
                state,
                attention,
            }
        })
        .collect();
    let worker_count = workers.len();
    let attention = workers.iter().filter(|w| w.attention.is_some()).count();
    // 要注意を先に、あとはペイン番号順（毎回同じ並び = 再描画で行が踊らない）
    workers.sort_by_key(|w| (w.attention.is_none(), w.pane));
    workers.truncate(BAND_MAX_WORKERS);
    let ctx_percent = input.ctx.as_ref().and_then(|c| c.percent);
    let (usage_bar, button_args, band_style) = if input.s7_legacy {
        (None, None, Some("s3"))
    } else {
        (
            Some(usage_bar_decision(
                input.ui.usage_bar.place,
                input.status_line,
                input.screen_shows_usage,
            )),
            Some(crate::claude_mod_ui::button_args(
                &input.ui.buttons,
                input.pane,
                input.cwd.as_deref(),
            )),
            None,
        )
    };
    // バーを描くなら帯の ctx / 使用制限の警告は出さない（同じ値を 2 か所で動かさない。§9.3）
    let warnings = if usage_bar.as_ref().is_some_and(|b| b.draw) {
        Vec::new()
    } else {
        band_warnings(ctx_percent, &input.rate_limits, input.thresholds)
    };
    BandView {
        pane: input.pane,
        pane_title: input.pane_title,
        tab_title: input.tab_title,
        lang: input.lang,
        worker_count,
        attention,
        workers,
        warnings,
        ctx: input.ctx,
        rate_limits: input
            .rate_limits
            .iter()
            .map(|l| BandLimit {
                kind: l.kind.clone(),
                percent: l.percent_used,
                resets_at: l.resets_at.as_deref().and_then(parse_resets_at),
            })
            .collect(),
        thresholds: input.thresholds,
        band_request: input.ui.band_request(),
        ui: input.ui,
        usage_bar,
        button_args,
        band_style,
    }
}

// --- S7-3（#1962）: バー・ボタン・帯の共存と、tako 側の表示を止める条件 -------------------
//
// 何を描くかの判断はここ（tako 側）。mod は応答の `view` に従って描き、描いたものを報告の
// `renders` で返す。tako は `renders` を見て自分の表示（ステータスバーの claude の区画）を引っ込める

/// mod のバーの棒の 8 段（`register.ts` の `BAR_GLYPHS` と同じ。単体テストが突き合わせる）。
/// バーの 1 項目は `<ラベル> <棒> <N>%`（例 `5h ▁ 4%`）
pub const BAR_GLYPHS: &str = "▁▂▃▄▅▆▇█";
/// 画面の末尾で statusLine の数値を探す行数（フッター = 入力欄の下の数行）
pub const USAGE_SCAN_LINES: usize = 8;

/// バー（使用制限・ctx）を mod に描かせるか（応答の `view.usage_bar`）
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UsageBarDecision {
    pub draw: bool,
    /// 描かせない理由（`off` = ui.json の置き場が off / `status_line` = 利用者の statusLine が既に
    /// ctx / 使用制限を出している）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
}

/// バーを描かせるかを決める（純関数。§9.5「利用者の statusLine」）。
///
/// statusLine が**設定されていて**画面の末尾に数値が出ているときだけ止める。設定の有無だけで
/// 止めると、ctx を出さない statusLine（ブランチ名だけ等）の利用者にバーが出ない。画面だけで
/// 止めると、mod 自身が描いたバーを見て止まり、次に消えて描き直す往復になる（画面の読み取りは
/// mod のバーを除いて数える = [`screen_shows_usage`]）
pub fn usage_bar_decision(
    place: crate::claude_mod_ui::UsageBarPlace,
    status_line: Option<bool>,
    screen_shows_usage: bool,
) -> UsageBarDecision {
    if place == crate::claude_mod_ui::UsageBarPlace::Off {
        return UsageBarDecision {
            draw: false,
            reason: Some("off"),
        };
    }
    if status_line == Some(true) && screen_shows_usage {
        return UsageBarDecision {
            draw: false,
            reason: Some("status_line"),
        };
    }
    UsageBarDecision {
        draw: true,
        reason: None,
    }
}

/// 画面の末尾（[`USAGE_SCAN_LINES`] 行）に ctx% か使用制限（`5h` / `7d` / `週`）の数値が出ているか。
/// **mod のバー（`5h ▁ 4%` の形）は除いて**数える（純関数）
pub fn screen_shows_usage(lines: &[String]) -> bool {
    use crate::terminal::extract_labeled_percent as labeled;
    lines.iter().rev().take(USAGE_SCAN_LINES).any(|line| {
        let line = strip_mod_bar(line);
        let lower = line.to_ascii_lowercase();
        labeled(&line, "5h").is_some()
            || labeled(&line, "7d").is_some()
            || labeled(&line, "週").is_some()
            || labeled(&lower, "ctx").is_some()
            || labeled(&lower, "context").is_some()
    })
}

/// 行から mod のバーの項目（`5h ▁ 4%` / `7d ▂ 22%` / `ctx ▁ 6%`）を取り除く
fn strip_mod_bar(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    'scan: while let Some(ch) = rest.chars().next() {
        for label in ["5h", "7d", "ctx"] {
            if let Some(len) = rest
                .strip_prefix(label)
                .and_then(bar_item_tail_len)
                .map(|tail| label.len() + tail)
            {
                rest = &rest[len..];
                continue 'scan;
            }
        }
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    out
}

/// ラベルの後ろの ` ▁ 4%`（空白・棒・空白・数字・`%`）のバイト長
fn bar_item_tail_len(s: &str) -> Option<usize> {
    let after_space = s.strip_prefix(' ')?;
    let glyph = after_space.chars().next()?;
    if !BAR_GLYPHS.contains(glyph) {
        return None;
    }
    let after_glyph = after_space[glyph.len_utf8()..].strip_prefix(' ')?;
    let digits = after_glyph.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let after = after_glyph[digits..].strip_prefix('%')?;
    Some(s.len() - after.len())
}

/// 画面下のステータスバーの claude の区画（5h / 7d / ctx）を誰が出すか
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeBarOwner {
    /// tako のステータスバーが今のまま出す（理由のコード）
    Tako(&'static str),
    /// フォーカス中のペインの mod が Claude Code の画面に描いている（置き場）
    Mod(String),
}

impl ClaudeBarOwner {
    /// ステータスバーから claude の区画を外すか
    pub fn yields(&self) -> bool {
        matches!(self, Self::Mod(_))
    }

    /// `drawn_by` の語（`mod` / `tako`）
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Tako(_) => "tako",
            Self::Mod(_) => "mod",
        }
    }

    /// `tako mod` / `tako limit-service --refresh` に載せる形
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Tako(reason) => serde_json::json!({ "drawn_by": "tako", "reason": reason }),
            Self::Mod(place) => serde_json::json!({ "drawn_by": "mod", "place": place }),
        }
    }
}

/// ステータスバーの claude の区画を止める条件（§9.6。**判断はこの 1 本**）。
///
/// `focused` は**フォーカス中のペイン**の新鮮な報告（45 秒以内。無い = mod なし・古い・codex / agy /
/// シェルのペイン）。報告の `renders.usage_bar` に置き場が載っている = mod がバーを描いたと
/// 言ったときだけ mod に任せる（ui.json の置き場が off なら mod は描かないので載らない）。
/// A/B（`legacy` = [`S7_LEGACY_ENV`]）では止めない。値の取り方（FR-2.42.18）は変えない
pub fn claude_bar_owner(focused: Option<&ModReport>, legacy: bool) -> ClaudeBarOwner {
    if legacy {
        return ClaudeBarOwner::Tako("legacy_env");
    }
    let Some(report) = focused else {
        return ClaudeBarOwner::Tako("no_fresh_report");
    };
    match report.renders.as_ref().and_then(|r| r.usage_bar.clone()) {
        Some(place) if !place.is_empty() => ClaudeBarOwner::Mod(place),
        _ => ClaudeBarOwner::Tako("mod_not_drawing"),
    }
}

/// 帯が他の mod に隠されているか（§9.5）。入力欄の下の行のフックは呼ばれているのに帯のフックが
/// 1 度も呼ばれていない = 外側の他の mod が `next` を呼ばずに帯を描いている（tako が env で
/// 外側にいれば起きないが、skills-dir で内側に読まれると起きる）
pub fn band_hidden_by_other_mod(report: &ModReport) -> bool {
    report
        .renders
        .as_ref()
        .is_some_and(|r| r.hint_hook && !r.band_hook)
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
    fn issue1959_報告の設定dirは初めて見たときだけ置きに行く先へ積む() {
        let t0 = Instant::now();
        let mut hub = ModHub::new(true);
        let report = parse_report(report_json()).unwrap();
        hub.accept(1, report.clone(), t0);
        hub.accept(2, report.clone(), t0);
        assert_eq!(hub.take_pending_config_dirs(), vec!["/cfg".to_string()]);
        hub.accept(3, report.clone(), t0);
        assert!(
            hub.take_pending_config_dirs().is_empty(),
            "同じ dir を 2 度積んだ"
        );
        // 既定の dir（config_dir 無し）と空は積まない
        let mut default_dir = report;
        default_dir.config_dir = None;
        hub.accept(4, default_dir.clone(), t0);
        default_dir.config_dir = Some(String::new());
        hub.accept(4, default_dir, t0);
        assert!(hub.take_pending_config_dirs().is_empty());
        assert_eq!(hub.reported_config_dirs.len(), 1);
    }

    #[test]
    fn issue1959_注入しない理由はstring_1つ分の大きさに収まる() {
        // 大きくなると ModSnapshot（ModUnavailable 越し）→ OffloadJob の変種の差が clippy の
        // large_enum_variant の閾値を越える。変種に String を足すなら Box<str> にする
        assert_eq!(
            std::mem::size_of::<OffReason>(),
            std::mem::size_of::<String>()
        );
    }

    #[test]
    fn issue1959_休眠の報告は一次ソースから外し理由を残す() {
        let t0 = Instant::now();
        let mut hub = ModHub::new(true);
        let report = parse_report(report_json()).unwrap();
        hub.accept(6, report.clone(), t0);
        let mut dormant = report.clone();
        dormant.dormant = Some("user_disabled".into());
        assert_eq!(hub.accept(6, dormant, t0), Accepted::Dormant);
        assert!(
            hub.fresh_report(6, t0).is_none(),
            "休眠した mod の値を使い続けた"
        );
        assert_eq!(
            hub.dormant.get(&6).map(String::as_str),
            Some("user_disabled")
        );
        // 利用者が戻して mod が報告を再開したら休眠の印は消える
        assert_eq!(hub.accept(6, report, t0), Accepted::Stored);
        assert!(hub.dormant.is_empty());
        hub.dormant.insert(7, "user_disabled".into());
        hub.forget_pane(7);
        assert!(hub.dormant.is_empty());
    }

    fn limit(kind: &str, pct: f64, resets: &str, observed: u64) -> ModRateLimit {
        ModRateLimit {
            kind: kind.into(),
            percent_used: pct,
            resets_at: Some(resets.into()),
            observed_at: observed,
        }
    }

    fn report_with(config_dir: Option<&str>, limits: Vec<ModRateLimit>) -> ModReport {
        let mut r = parse_report(report_json()).unwrap();
        r.config_dir = config_dir.map(str::to_string);
        r.rate_limits = limits;
        r
    }

    #[test]
    fn issue1880_引き当ては新鮮_古い_注入の記録の順に理由を返す() {
        let t0 = Instant::now();
        let mut hub = ModHub::new(true);
        hub.accept(1, parse_report(report_json()).unwrap(), t0);
        hub.record_spawn(2, &on());
        hub.record_spawn(3, &Injection::Off(OffReason::Disabled));
        hub.record_spawn(
            4,
            &Injection::Off(OffReason::ClaudeTooOld("2.1.280".into())),
        );
        let at = |pane, secs| lookup(Some(&hub), pane, t0 + Duration::from_secs(secs), false);
        assert!(at(1, 45).is_ok(), "45 秒までは新鮮");
        assert_eq!(
            at(1, 46).unwrap_err(),
            ModUnavailable::Stale { age_secs: 46 }
        );
        assert_eq!(at(1, 46).unwrap_err().code(), "mod_stale");
        assert_eq!(at(2, 0).unwrap_err().code(), "mod_absent");
        assert_eq!(at(3, 0).unwrap_err().code(), "disabled");
        assert_eq!(at(4, 0).unwrap_err().code(), "claude_too_old");
        assert_eq!(at(9, 0).unwrap_err().code(), "mod_absent");
        // A/B は報告があっても見ない
        assert_eq!(
            lookup(Some(&hub), 1, t0, true).unwrap_err().code(),
            "legacy_env"
        );
        // mod を扱わない tako
        assert_eq!(lookup(None, 1, t0, false).unwrap_err().code(), "mod_absent");
        // 注入しなかったペインでも新鮮な報告があれば使う（値そのものは正しい）
        hub.accept(3, parse_report(report_json()).unwrap(), t0);
        assert!(lookup(Some(&hub), 3, t0, false).is_ok());
    }

    #[test]
    fn issue1880_ctxの材料は欠けたまま渡す() {
        let mut r = parse_report(report_json()).unwrap();
        let ctx = r.ctx();
        assert_eq!(
            (ctx.percent, ctx.tokens, ctx.window),
            (Some(6), Some(59426), Some(1_000_000))
        );
        assert_eq!(ctx.model.as_deref(), Some("claude-haiku-5-5"));
        r.context = Some(ModContext {
            tokens: None,
            window: 0,
            percent: None,
        });
        let ctx = r.ctx();
        assert_eq!((ctx.percent, ctx.window), (None, None), "窓 0 は不明と同じ");
    }

    #[test]
    fn issue1880_resets_atは秒精度でタイムゾーンを守って読む() {
        // 2026-10-08T14:30:00Z
        let base = 1_791_469_800;
        assert_eq!(parse_resets_at("2026-10-08T14:30:00.000Z"), Some(base));
        assert_eq!(parse_resets_at("2026-10-08T14:30:00Z"), Some(base));
        assert_eq!(parse_resets_at("2026-10-08T14:30:07.999Z"), Some(base + 7));
        assert_eq!(parse_resets_at("2026-10-08T23:30:00+09:00"), Some(base));
        assert_eq!(parse_resets_at("2026-10-08T09:30:00-05:00"), Some(base));
        // タイムゾーンの無い表記・壊れた表記は読まない
        for bad in [
            "2026-10-08T14:30:00",
            "2026-10-08",
            "3am",
            "",
            "2026-13-08T14:30:00Z",
            "2026-10-08T25:30:00Z",
        ] {
            assert_eq!(parse_resets_at(bad), None, "{bad}");
        }
    }

    #[test]
    fn issue1880_解除時刻の手がかりは上限に当たった窓だけから採る() {
        let five = limit("five_hour", 100.0, "2026-10-08T14:30:00.000Z", 1);
        let week = limit("seven_day", 40.0, "2026-10-10T13:00:00.000Z", 1);
        assert_eq!(
            limit_reset_at(&[five.clone(), week.clone()]),
            parse_resets_at("2026-10-08T14:30:00Z")
        );
        // 当たっていなければ手がかり無し
        let mut under = five.clone();
        under.percent_used = 99.0;
        assert_eq!(limit_reset_at(&[under, week.clone()]), None);
        // 両方当たっていれば遅い方
        let mut week_full = week;
        week_full.percent_used = 100.0;
        assert_eq!(
            limit_reset_at(&[five, week_full]),
            parse_resets_at("2026-10-10T13:00:00Z")
        );
    }

    #[test]
    fn issue1880_使用制限はアカウントごとに束ねて新しい窓と大きい方を採る() {
        // S2 の束ね方（#1903 の A/B = legacy アーム）。mod が heartbeat のたびに今の時刻を
        // 打っていた頃の回避策なので、observed_at は放置したペインの方が新しい前提で組む
        let t0 = Instant::now();
        let mut hub = ModHub::new(true);
        // pane 1: 1 時間放置（古い 20%）でも heartbeat の observed_at は新しい
        hub.accept(
            1,
            report_with(
                Some("/acct-a"),
                vec![limit("five_hour", 20.0, "2026-10-08T14:30:00Z", 900)],
            ),
            t0,
        );
        // pane 2: 同じアカウント・同じ窓で 80%（observed_at は古い）
        hub.accept(
            2,
            report_with(
                Some("/acct-a"),
                vec![
                    limit("five_hour", 80.0, "2026-10-08T14:30:00Z", 500),
                    limit("seven_day", 10.0, "2026-10-10T13:00:00Z", 500),
                ],
            ),
            t0,
        );
        // pane 3: 別アカウントは混ぜない
        hub.accept(
            3,
            report_with(
                Some("/acct-b"),
                vec![limit("five_hour", 99.0, "2026-10-08T15:00:00Z", 999)],
            ),
            t0,
        );
        let got = hub.account_rate_limits_by(1, t0, true);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].kind, "five_hour");
        assert_eq!(got[0].percent_used, 80.0, "同じ窓なら大きい方 = 新しい観測");
        assert_eq!(got[1].kind, "seven_day");
        // 窓が替わった（resets_at が新しい）なら % が小さくても新しい窓を採る
        hub.accept(
            2,
            report_with(
                Some("/acct-a"),
                vec![limit("five_hour", 3.0, "2026-10-08T19:30:00Z", 1000)],
            ),
            t0,
        );
        assert_eq!(hub.account_rate_limits_by(1, t0, true)[0].percent_used, 3.0);
        // 古い報告は束ねない・自分が古ければ空
        let later = t0 + Duration::from_secs(60);
        assert!(hub.account_rate_limits_by(1, later, true).is_empty());
        hub.accept(1, report_with(Some("/acct-a"), vec![]), later);
        assert!(
            hub.account_rate_limits_by(1, later, true).is_empty(),
            "pane 2 の報告は失効している"
        );
    }

    #[test]
    fn issue1903_使用制限は最新の観測で束ねる() {
        // mod は値が変わったときだけ observed_at を打つ（#1903）= 放置したペインの値は古い時刻のまま
        let t0 = Instant::now();
        let mut hub = ModHub::new(true);
        // pane 1: 1 時間前に 80% を観測したきり放置（heartbeat で鮮度は保っている）
        hub.accept(
            1,
            report_with(
                Some("/acct-a"),
                vec![
                    limit("five_hour", 80.0, "2026-10-08T14:30:00Z", 1_000),
                    limit("seven_day", 30.0, "2026-10-10T13:00:00Z", 1_000),
                ],
            ),
            t0,
        );
        // pane 2: 動いている。上限の引き上げ（プラン変更・早めのリセット）で同じ窓の % が下がった
        hub.accept(
            2,
            report_with(
                Some("/acct-a"),
                vec![limit("five_hour", 20.0, "2026-10-08T14:30:00Z", 3_601_000)],
            ),
            t0,
        );
        // pane 3: 別アカウントのもっと新しい観測は混ぜない
        hub.accept(
            3,
            report_with(
                Some("/acct-b"),
                vec![limit("five_hour", 99.0, "2026-10-08T15:00:00Z", 9_999_000)],
            ),
            t0,
        );
        for pane in [1, 2] {
            let got = hub.account_rate_limits_by(pane, t0, false);
            assert_eq!(got.len(), 2, "pane {pane}");
            assert_eq!(
                (got[0].kind.as_str(), got[0].percent_used),
                ("five_hour", 20.0),
                "pane {pane}: 動いているペインの最新の観測"
            );
            assert_eq!(
                (got[1].kind.as_str(), got[1].percent_used),
                ("seven_day", 30.0),
                "pane {pane}: 他に観測が無い窓は放置したペインの値"
            );
        }
        // A/B: S2 の束ね方は同じ窓の大きい方 = 放置したペインの古い 80% が勝つ
        assert_eq!(
            hub.account_rate_limits_by(2, t0, true)[0].percent_used,
            80.0
        );
        // 同じ時刻なら新しい窓 → 大きい方
        hub.accept(
            1,
            report_with(
                Some("/acct-a"),
                vec![limit("five_hour", 5.0, "2026-10-08T19:30:00Z", 3_601_000)],
            ),
            t0,
        );
        assert_eq!(
            hub.account_rate_limits_by(2, t0, false)[0].percent_used,
            5.0
        );
        // 古い報告は束ねない（最新の観測でも失効した報告の値は採らない）
        let later = t0 + Duration::from_secs(60);
        hub.accept(
            2,
            report_with(
                Some("/acct-a"),
                vec![limit("five_hour", 40.0, "2026-10-08T14:30:00Z", 100)],
            ),
            later,
        );
        assert_eq!(
            hub.account_rate_limits_by(2, later, false)[0].percent_used,
            40.0
        );
    }

    #[test]
    fn issue1903_ステータスバーはmodの窓があればmodだけで決める() {
        let mod_limits = vec![
            limit("five_hour", 42.4, "2026-10-08T14:30:00Z", 1),
            limit("seven_day", 17.5, "2026-10-10T13:00:00Z", 1),
            limit("spend_limit", 3.0, "2026-10-10T13:00:00Z", 1),
        ];
        assert_eq!(
            bar_limits(&mod_limits, Some(23), Some(46)),
            BarLimits {
                five_hour: Some(42),
                seven_day: Some(18),
                source: Some(BarLimitSource::Mod),
            },
            "画面の値（23 / 46）より mod を先に見る・帯と同じ四捨五入"
        );
        // 片方の窓だけでも mod で決める（画面の 7d と混ぜない）
        assert_eq!(
            bar_limits(&mod_limits[..1], Some(23), Some(46)),
            BarLimits {
                five_hour: Some(42),
                seven_day: None,
                source: Some(BarLimitSource::Mod),
            }
        );
        // mod に 5h / 7d が無い（報告なし・初回応答の前・ゲートウェイの窓だけ）なら画面の値
        for none in [&[][..], &mod_limits[2..]] {
            assert_eq!(
                bar_limits(none, Some(23), Some(46)),
                BarLimits {
                    five_hour: Some(23),
                    seven_day: Some(46),
                    source: Some(BarLimitSource::Screen),
                }
            );
        }
        assert_eq!(bar_limits(&[], None, None), BarLimits::default());
        // 上限を超えた値・壊れた値でも落ちない（as は飽和）
        let odd = vec![
            limit("five_hour", 104.6, "2026-10-08T14:30:00Z", 1),
            limit("seven_day", -1.0, "2026-10-10T13:00:00Z", 1),
        ];
        let got = bar_limits(&odd, None, None);
        assert_eq!((got.five_hour, got.seven_day), (Some(105), Some(0)));
        assert_eq!(BarLimitSource::Mod.as_str(), "mod");
        assert_eq!(BarLimitSource::Screen.as_str(), "screen");
    }

    #[test]
    fn issue1880_ターン状態の写像は承認後のツール実行中をbusyへ倒す() {
        assert_eq!(ModTurn::Idle.status_word(false), "idle");
        assert_eq!(ModTurn::Busy.status_word(false), "busy");
        assert_eq!(ModTurn::Permission.status_word(false), "waiting");
        assert_eq!(ModTurn::Question.status_word(false), "waiting");
        // 承認後もツールが返るまで permission のまま = 画面が生成中なら busy
        assert_eq!(ModTurn::Permission.status_word(true), "busy");
        assert_eq!(ModTurn::Question.status_word(true), "busy");
        // idle は画面の busy で変えない（倒すのは呼び出し側の既存の補正 = 1 実装）
        assert_eq!(ModTurn::Idle.status_word(true), "idle");
    }

    #[test]
    fn modの置き場の見分け() {
        assert!(is_tako_mod_entry("/x/claude-mod/tako"));
        assert!(is_tako_mod_entry("/x/claude-mod/tako/"));
        assert!(is_tako_mod_entry(r"C:\x\claude-mod\tako"));
        assert!(!is_tako_mod_entry("/x/claude-mod/other"));
        assert!(!is_tako_mod_entry("/x/tako"));
    }

    fn facts(pane: u64) -> WorkerFacts {
        WorkerFacts {
            pane,
            name: format!("w{pane}"),
            ..WorkerFacts::default()
        }
    }

    #[test]
    fn issue1881_workerの状態は使用制限_mod_画面_コマンドの順に決まる() {
        use crate::CommandState as C;
        let with = |f: fn(&mut WorkerFacts)| {
            let mut w = facts(1);
            f(&mut w);
            classify_worker(&w)
        };
        // 使用制限が最優先（mod の値でも画面のダイアログでも）
        let got = with(|w| {
            w.limited = true;
            w.mod_turn = Some(ModTurn::Busy);
        });
        assert_eq!(got, (WorkerState::Limited, Some(Attention::Limited)));
        let got = with(|w| w.limit_dialog_on_screen = true);
        assert_eq!(got, (WorkerState::Limited, Some(Attention::Limited)));
        // mod が新鮮なら画面より先。承認後のツール実行中（画面が生成中）は busy
        let got = with(|w| w.mod_turn = Some(ModTurn::Permission));
        assert_eq!(got, (WorkerState::Waiting, Some(Attention::Permission)));
        let got = with(|w| w.mod_turn = Some(ModTurn::Question));
        assert_eq!(got, (WorkerState::Waiting, Some(Attention::Question)));
        let got = with(|w| {
            w.mod_turn = Some(ModTurn::Permission);
            w.screen_busy = true;
        });
        assert_eq!(got, (WorkerState::Busy, None));
        // 画面のダイアログは mod の busy / idle より先（組織アカウントではルールの無い ask を
        // mod が拾えない = FR-2.42.7）
        let got = with(|w| {
            w.mod_turn = Some(ModTurn::Busy);
            w.dialog_on_screen = true;
            w.permission_on_screen = true;
        });
        assert_eq!(got, (WorkerState::Waiting, Some(Attention::Permission)));
        let got = with(|w| {
            w.mod_turn = Some(ModTurn::Idle);
            w.dialog_on_screen = true;
        });
        assert_eq!(got, (WorkerState::Waiting, Some(Attention::Dialog)));
        assert_eq!(
            with(|w| w.mod_turn = Some(ModTurn::Idle)),
            (WorkerState::Idle, None)
        );
        assert_eq!(
            with(|w| w.mod_turn = Some(ModTurn::Busy)),
            (WorkerState::Busy, None)
        );
        // mod の無い worker（codex / agy / 古い claude）は画面とコマンド状態から
        let got = with(|w| w.dialog_on_screen = true);
        assert_eq!(got, (WorkerState::Waiting, Some(Attention::Dialog)));
        let got = with(|w| w.command = C::Failed(1));
        assert_eq!(got, (WorkerState::Failed, Some(Attention::Failed)));
        let got = with(|w| {
            w.command = C::Running;
            w.screen_busy = true;
        });
        assert_eq!(got, (WorkerState::Busy, None));
        assert_eq!(with(|w| w.command = C::Running), (WorkerState::Idle, None));
        assert_eq!(with(|_| {}), (WorkerState::Unknown, None));
    }

    #[test]
    fn issue1881_帯の警告は閾値以上だけでctxが先_使用制限は大きい順() {
        let th = BandThresholds::default();
        assert!(band_warnings(Some(BAND_CTX_PERCENT - 1), &[], th).is_empty());
        let limits = vec![
            limit(
                "five_hour",
                BAND_LIMIT_PERCENT - 0.5,
                "2026-10-08T19:30:00Z",
                1,
            ),
            limit("seven_day", 85.0, "2026-10-10T13:00:00Z", 1),
            limit("seven_day_opus", 100.0, "2026-10-10T13:00:00Z", 1),
        ];
        let got = band_warnings(Some(BAND_CTX_PERCENT), &limits, th);
        let kinds: Vec<&str> = got.iter().map(|w| w.kind.as_str()).collect();
        assert_eq!(kinds, ["ctx", "seven_day_opus", "seven_day"]);
        assert_eq!(got[0].percent, f64::from(BAND_CTX_PERCENT));
        assert_eq!(got[2].resets_at, parse_resets_at("2026-10-10T13:00:00Z"));
        // ctx が未観測（最初の応答の前）なら出さない（0% と描かない）
        assert!(band_warnings(None, &[], th).is_empty());
        // 検証用の閾値 0 なら全部出す
        let zero = BandThresholds {
            ctx_percent: 0,
            limit_percent: 0.0,
        };
        assert_eq!(band_warnings(Some(3), &limits, zero).len(), 4);
    }

    #[test]
    fn issue1881_帯の材料は要注意を数えて先に並べ上限で切る() {
        let mut workers: Vec<WorkerFacts> = (1..=(BAND_MAX_WORKERS as u64 + 5))
            .map(|p| {
                let mut f = facts(p);
                f.mod_turn = Some(ModTurn::Busy);
                f
            })
            .collect();
        let last = workers.len() - 1;
        workers[last].mod_turn = Some(ModTurn::Question);
        workers[3].command = crate::CommandState::Failed(2);
        workers[3].mod_turn = None;
        let view = band_view(BandInput {
            pane: 7,
            pane_title: Some("master".into()),
            tab_title: Some("main".into()),
            lang: "ja",
            workers,
            ctx: Some(ModContext {
                tokens: Some(850),
                window: 1000,
                percent: Some(85),
            }),
            rate_limits: vec![limit("five_hour", 3.0, "2026-10-08T19:30:00Z", 1)],
            thresholds: BandThresholds::default(),
            ui: {
                let mut ui = crate::claude_mod_ui::UiConfig::default();
                ui.band.hidden = true;
                ui.band.toggled_at = Some(42);
                ui
            },
            // S3 の帯の警告（閾値超え）を見る。S7-3 ではバーを描くと警告は帯から外れる
            // （issue1962_バーを描くなら帯の警告を外しabでは今の材料のまま）
            s7_legacy: true,
            ..BandInput::default()
        });
        assert_eq!(view.worker_count, BAND_MAX_WORKERS + 5, "数は全部数える");
        assert_eq!(view.attention, 2);
        assert_eq!(view.workers.len(), BAND_MAX_WORKERS, "並べるのは上限まで");
        assert_eq!(view.workers[0].pane, 4, "要注意が先（ペイン番号順）");
        assert_eq!(view.workers[0].attention, Some(Attention::Failed));
        assert_eq!(view.workers[1].attention, Some(Attention::Question));
        assert_eq!(view.warnings.len(), 1, "使用制限 3% は帯に出さない");
        assert_eq!(view.warnings[0].kind, "ctx");
        assert_eq!(
            view.rate_limits.len(),
            1,
            "サイドバーには閾値に関わらず出す"
        );
        let v = serde_json::to_value(&view).unwrap();
        assert_eq!(v["thresholds"]["ctx_percent"], BAND_CTX_PERCENT);
        assert_eq!(v["band_request"]["hidden"], true);
        assert_eq!(
            v["band_request"]["at"], 42,
            "中継は ui.json の toggled_at から作る"
        );
        // #1960: 検証済みの ui.json を `ui` に載せる（既定のボタンは /compact）
        assert_eq!(v["ui"]["schema_version"], 1);
        assert_eq!(v["ui"]["buttons"][0]["action"]["command"], "compact");
        assert_eq!(v["workers"][1]["state"], "waiting");
        assert!(v["workers"][4].get("attention").is_none());
    }

    #[test]
    fn issue1881_報告の帯の状態は読めて区切りは上限で切る() {
        let mut v = report_json();
        v["band"] = serde_json::json!({
            "hidden": false, "shown": true, "columns": 75,
            "segments": (0..40).map(|i| format!("s{i}")).collect::<Vec<_>>(),
        });
        let r = parse_report(v).unwrap();
        let band = r.band.unwrap();
        assert!(band.shown && !band.hidden);
        assert_eq!(band.columns, Some(75));
        assert_eq!(band.segments.len(), MAX_BAND_SEGMENTS);
        // S3 前の mod（band を送らない）も読める
        assert!(parse_report(report_json()).unwrap().band.is_none());
    }

    /// #1960: 中継の正本は ui.json。誰も切り替えていなければ載せない（mod の `$.store` のまま）
    #[test]
    fn issue1960_帯のトグルの中継はuijsonから作る() {
        let view = band_view(BandInput::default());
        assert_eq!(view.band_request, None);
        let mut ui = crate::claude_mod_ui::UiConfig::default();
        ui.band.hidden = false;
        ui.band.toggled_at = Some(20);
        let view = band_view(BandInput {
            ui,
            ..BandInput::default()
        });
        assert_eq!(
            view.band_request,
            Some(BandRequest {
                hidden: false,
                at: 20
            })
        );
    }

    fn renders(usage_bar: Option<&str>, band_hook: bool, hint_hook: bool) -> ModReport {
        let mut v = report_json();
        v["renders"] = serde_json::json!({
            "band": true, "usage_bar": usage_bar, "buttons": 1,
            "band_hook": band_hook, "hint_hook": hint_hook,
        });
        parse_report(v).unwrap()
    }

    /// #1962: renders / status_line / last_press は読めて、無い報告（S7-3 前の mod）も読める
    #[test]
    fn issue1962_報告のrendersは後方互換で読める() {
        let r = renders(Some("prompt_hint"), true, true);
        let got = r.renders.as_ref().unwrap();
        assert!(got.band && got.band_hook && got.hint_hook);
        assert_eq!(got.usage_bar.as_deref(), Some("prompt_hint"));
        assert_eq!(got.buttons, 1);
        let mut v = report_json();
        v["status_line"] = serde_json::json!(true);
        v["last_press"] = serde_json::json!({"kind": "slash", "ok": true, "at": 5});
        let r = parse_report(v).unwrap();
        assert_eq!(r.status_line, Some(true));
        assert_eq!(
            r.last_press,
            Some(ModPress {
                kind: "slash".into(),
                ok: true,
                at: 5
            })
        );
        let old = parse_report(report_json()).unwrap();
        assert!(old.renders.is_none() && old.status_line.is_none() && old.last_press.is_none());
        // 一部だけ送る mod（キーの欠け）も既定で読む
        let mut v = report_json();
        v["renders"] = serde_json::json!({"band": true});
        let r = parse_report(v).unwrap();
        assert_eq!(r.renders.unwrap().usage_bar, None);
        // 長すぎる置き場の語は切る
        let mut v = report_json();
        v["renders"] = serde_json::json!({"usage_bar": "x".repeat(1000)});
        assert_eq!(
            parse_report(v)
                .unwrap()
                .renders
                .unwrap()
                .usage_bar
                .unwrap()
                .len(),
            MAX_TEXT
        );
    }

    /// #1962: バーを描かせるか。statusLine が設定されていて画面に数値が出ているときだけ止める
    #[test]
    fn issue1962_バーはstatus_lineが数値を出しているときだけ止める() {
        use crate::claude_mod_ui::UsageBarPlace as P;
        let draw = |place, line, screen| usage_bar_decision(place, line, screen);
        assert_eq!(
            draw(P::PromptHint, None, false),
            UsageBarDecision {
                draw: true,
                reason: None
            }
        );
        assert!(
            draw(P::PromptHint, Some(false), true).draw,
            "statusLine なし"
        );
        assert!(
            draw(P::PromptHint, Some(true), false).draw,
            "ctx を出さない statusLine"
        );
        assert_eq!(
            draw(P::PromptHint, Some(true), true).reason,
            Some("status_line")
        );
        assert_eq!(draw(P::Band, Some(true), true).reason, Some("status_line"));
        assert_eq!(draw(P::Off, None, false).reason, Some("off"));
        assert!(!draw(P::Off, None, false).draw);
    }

    /// #1962: 画面の読み取りは mod 自身が描いたバーを数えない（描いたバーを見て止まる往復を作らない）
    #[test]
    fn issue1962_画面の数値はmodのバーを除いて数える() {
        let lines = |text: &[&str]| text.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // mod のバーだけ（PromptHint の末尾）
        assert!(!screen_shows_usage(&lines(&[
            "❯ ",
            "  ⏵⏵ auto mode on (shift+tab to cycle) · 5h ▁ 4% 7d ▂ 22% ctx ▁ 6%",
        ])));
        assert!(!screen_shows_usage(&lines(&["ctx █ 100% 5h ▄ 50%"])));
        // 利用者の statusLine（#217 の形）
        assert!(screen_shows_usage(&lines(&[
            "  ⏵⏵ auto mode on",
            "  [Opus 5] ctx 45% (90K/200K) 5h 12% 7d 3%",
        ])));
        assert!(screen_shows_usage(&lines(&["Context: 45%"])));
        assert!(screen_shows_usage(&lines(&["週 30%"])));
        // バーと statusLine が同じ行に並んでも statusLine の数値は残る
        assert!(screen_shows_usage(&lines(&["5h ▁ 4% | ctx 45%"])));
        // 数値の無い statusLine（ブランチ名だけ）
        assert!(!screen_shows_usage(&lines(&["main* ~/work"])));
        // 棒の形が崩れたもの（棒が無い・数字が無い）は除かない = 利用者の表示として数える
        assert!(screen_shows_usage(&lines(&["5h 4%"])));
        assert!(screen_shows_usage(&lines(&["ctx ▁ x 6%"])));
        // 走査は末尾 USAGE_SCAN_LINES 行だけ（会話の本文の ctx 45% は数えない）
        let mut many = lines(&["ctx 45%"]);
        many.extend((0..USAGE_SCAN_LINES).map(|_| String::new()));
        assert!(!screen_shows_usage(&many));
        assert!(!screen_shows_usage(&[]));
    }

    /// #1962: register.ts の棒の 8 段は Rust の BAR_GLYPHS と同じ（画面の読み取りが除く形と揃う）
    #[test]
    fn issue1962_バーの棒はregister_tsと揃う() {
        let want = format!("const BAR_GLYPHS = '{BAR_GLYPHS}'");
        assert!(
            REGISTER_TS.contains(&want),
            "register.ts の BAR_GLYPHS が {BAR_GLYPHS} と違う（{want} を探した）"
        );
    }

    /// #1962: ステータスバーの claude の区画を止める条件は「新鮮な報告 ∧ renders.usage_bar」だけ
    #[test]
    fn issue1962_ステータスバーのclaudeの区画はmodが描いたときだけ止める() {
        let drawing = renders(Some("prompt_hint"), true, true);
        assert_eq!(
            claude_bar_owner(Some(&drawing), false),
            ClaudeBarOwner::Mod("prompt_hint".into())
        );
        assert!(claude_bar_owner(Some(&drawing), false).yields());
        // A/B
        assert_eq!(
            claude_bar_owner(Some(&drawing), true),
            ClaudeBarOwner::Tako("legacy_env")
        );
        // 報告なし（mod なし・古い・codex / agy / シェルにフォーカス）
        assert_eq!(
            claude_bar_owner(None, false),
            ClaudeBarOwner::Tako("no_fresh_report")
        );
        // 描いていない（statusLine が出している・置き場 off・値がまだ無い）・S7-3 前の mod
        for report in [
            renders(None, true, true),
            parse_report(report_json()).unwrap(),
        ] {
            assert_eq!(
                claude_bar_owner(Some(&report), false),
                ClaudeBarOwner::Tako("mod_not_drawing")
            );
        }
        let v = ClaudeBarOwner::Mod("band".into()).to_json();
        assert_eq!(v["drawn_by"], "mod");
        assert_eq!(v["place"], "band");
        assert_eq!(ClaudeBarOwner::Tako("x").to_json()["reason"], "x");
    }

    /// #1962: 帯のフックが呼ばれず入力欄の下の行のフックだけが呼ばれた = 他の mod に隠された
    #[test]
    fn issue1962_帯が他のmodに隠されたことを見分ける() {
        assert!(band_hidden_by_other_mod(&renders(None, false, true)));
        assert!(!band_hidden_by_other_mod(&renders(None, true, true)));
        // どちらもまだ（起きた直後・PromptHint ごと他の mod が描いている）は言い切らない
        assert!(!band_hidden_by_other_mod(&renders(None, false, false)));
        assert!(!band_hidden_by_other_mod(
            &parse_report(report_json()).unwrap()
        ));
    }

    /// #1962: バーを描くなら帯の ctx / 使用制限の警告は出さない。A/B では今の材料（S3）のまま
    #[test]
    fn issue1962_バーを描くなら帯の警告を外しabでは今の材料のまま() {
        let input = || BandInput {
            pane: 7,
            lang: "ja",
            ctx: Some(ModContext {
                tokens: Some(850),
                window: 1000,
                percent: Some(85),
            }),
            rate_limits: vec![limit("five_hour", 92.0, "2026-10-08T19:30:00Z", 1)],
            cwd: Some("/w".into()),
            ..BandInput::default()
        };
        let view = band_view(input());
        assert_eq!(
            view.usage_bar,
            Some(UsageBarDecision {
                draw: true,
                reason: None
            })
        );
        assert!(
            view.warnings.is_empty(),
            "バーと帯で同じ値を 2 か所に出さない"
        );
        assert_eq!(view.band_style, None);
        // 既定のボタンは slash の compact だけ = CLI の引数は載らない（空の表）
        assert_eq!(view.button_args, Some(BTreeMap::new()));
        // statusLine が数値を出している → バーを描かないので帯の警告（閾値超え）は今のまま
        let view = band_view(BandInput {
            status_line: Some(true),
            screen_shows_usage: true,
            ..input()
        });
        assert_eq!(view.usage_bar.as_ref().unwrap().reason, Some("status_line"));
        assert_eq!(view.warnings.len(), 2);
        // tako の操作のボタンは tako が CLI の引数を組んで渡す
        let mut ui = crate::claude_mod_ui::UiConfig::default();
        ui.buttons.push(crate::claude_mod_ui::Button {
            id: "split-right".into(),
            label: "split right".into(),
            hotkey: "s".into(),
            action: crate::claude_mod_ui::ButtonAction::Tako {
                op: crate::claude_mod_ui::TakoOp::SplitRight,
            },
        });
        let view = band_view(BandInput { ui, ..input() });
        assert_eq!(
            view.button_args.unwrap()["split-right"],
            ["split", "--pane", "7", "--right"]
        );
        // A/B: S3 の描き方の印だけを載せ、バーの判断とボタンの引数は載せない
        let view = band_view(BandInput {
            s7_legacy: true,
            ..input()
        });
        assert_eq!(view.band_style, Some("s3"));
        assert_eq!(view.usage_bar, None);
        assert_eq!(view.button_args, None);
        assert_eq!(view.warnings.len(), 2);
        let v = serde_json::to_value(&view).unwrap();
        assert!(v.get("usage_bar").is_none() && v.get("button_args").is_none());
        assert_eq!(v["band_style"], "s3");
    }
}
