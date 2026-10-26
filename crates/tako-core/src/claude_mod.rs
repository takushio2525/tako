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
//! - **コマンドカード（S7-4 #1963）**: 応答の `view.cards` にこのペインのカード（論理文字列と、押したときに
//!   叩く CLI の引数・帯のホットキー）を載せ、mod が会話の行（`tako_show_command` の行）と帯の 1 行に描く。
//!   mod が帯に描いたカードの id は報告の `renders.cards` で返り、GPUI のカード帯はそのカードを出さない
//!   （[`card_owner`]。store には残る = スマホ・CLI は今と同じ）
//! - **チャット風の描画（S7-6 #1964）**: 応答の `view.chat`（[`chat_view`]）で、吹き出し・コードのコピー・
//!   ツールの要約のどれを描くかを tako が決める（ui.json の `chat` × 表示モード。ターミナル表示では既定で
//!   コードのコピーだけ）。mod は描いている組を報告の `renders.chat` で返し、GUI モードでそのペインの
//!   チャットを mod が持つと決まれば（[`chat_owner`]）GPUI のチャットビューの代わりにターミナル表示
//!   （= mod の描いた本物の Claude Code）を出す。`chat.place = auto` の GUI モードでの持ち主は
//!   **判断点**（#1964 の受け入れ 4）が決まるまで tako（[`AUTO_CHAT_OWNER_GUI`]）
//! - **報告の無い claude の理由（#1891）**: [`classify_no_report`] が材料（[`NoReportEvidence`] =
//!   ペインの claude のコマンド行と起動時の env）から候補を並べずに 1 つに決める。材料は `tako mod` と
//!   `tako check-health` の背景の段が同じ 1 実装で採る（tako-control の `claude_mod`）
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
/// 監視（`worker_status` / `orchestrator self` / `report`）が mod の報告を答えにして
/// `claude agents --json` を起こさない鮮度（#2017。[`monitor_plan`]）。
///
/// heartbeat（15 秒）+ 余裕 5 秒（flush は 1 秒刻み・`tako mod report` の往復）。生きている mod は
/// 無変化でも 15 秒ごとに報告するので、これを過ぎた報告は「mod が止まった / claude が `ended` を
/// 送れずに落ちた」の疑い = 走査へ落として gone を拾う。値を一次ソースに使う期限 [`FRESH_FOR`]
/// （45 秒）より短くするのは、落ちた claude を走査が拾うまでの遅れを heartbeat 1 回ぶんに抑えるため
pub const MONITOR_SKIP_FOR: Duration = Duration::from_secs(20);
/// #2017 の A/B（同一バイナリで修正前 = 監視は mod の有無にかかわらず全アカウントを
/// ログインシェル経由で走査する）
pub const MONITOR_LEGACY_ENV: &str = "TAKO_2017_LEGACY";
/// #2041 の A/B（同一バイナリで修正前 = GUI モードのチャット表示は mod の報告の有無にかかわらず
/// 全アカウントを走査し、worker_status はレジストリの古い会話 ID を名指しして走査し続ける）
pub const SESSION_LEGACY_ENV: &str = "TAKO_2041_LEGACY";
/// #1891 の A/B（同一バイナリで修正前 = 報告の無い claude のペインを調べず候補を並べ、
/// `tako check-health` に mod の節を載せない）
pub const NO_REPORT_LEGACY_ENV: &str = "TAKO_1891_LEGACY";
/// 報告の契約の版（`register.ts` / `types/index.d.ts` の `schema`）
pub const SCHEMA: u32 = 1;
/// 帯・サイドバーを描かない A/B の入口（S3 #1881）。報告の受け取りと一次ソース化（S1 / S2）は
/// 続けるが、`tako mod report` の応答に帯・サイドバーの材料（`view`）を載せない = mod は何も描かない
pub const S3_LEGACY_ENV: &str = "TAKO_1877_S3_LEGACY";
/// 帯を S3（#1881）の描き方へ戻す A/B の入口（S7-3 #1962）。応答の `view` に `band_style: "s3"` を
/// 載せ、バーの判断（`usage_bar`）とボタンの引数（`button_args`）を載せない = mod は `next(e)` を
/// 包まず、ボタンとバーを描かず、`renders` を報告しない。ステータスバーの claude の区画も止めない
pub const S7_LEGACY_ENV: &str = "TAKO_1877_S7_LEGACY";
/// コマンドカードを mod で描く前（S7-4 #1963）へ戻す A/B の入口。応答の `view` に `cards` を載せない
/// （mod はカードを描かない）・GPUI のカード帯は mod の報告を見ずに今のまま出す
pub const CARDS_LEGACY_ENV: &str = "TAKO_1963_LEGACY";
/// 会話の行のカードにクリックで押す「実行 / コピー」を置く前（#2159）へ戻す A/B の入口。応答の
/// `view.cards.click` を常に偽にする（mod は会話の行にボタンを置かず、帯で押す案内だけを描く = #1963 の形）
pub const CARD_CLICK_LEGACY_ENV: &str = "TAKO_2159_LEGACY";
/// チャット風の描画を mod でする前（S7-6 #1964）へ戻す A/B の入口。応答の `view` に `chat` を載せない
/// （mod はチャットを描かない）・GUI モードのチャット表示は mod の報告を見ずに GPUI のチャットビューのまま
pub const CHAT_LEGACY_ENV: &str = "TAKO_1964_LEGACY";

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

/// [`install`] が失敗した理由（#2365。日英の正本は [`Self::text_in`]。`tako mod` の応答の `install` は
/// 表示言語、setup の行と GUI の stderr は日本語で引く）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallError {
    /// データディレクトリが決まらない（HOME が無い）
    NoDataDir,
    /// 展開先の親（`<data_dir>/claude-mod`）を作れない
    CreateRoot { path: String, error: String },
    /// 一時 dir へ書き出せない
    Write(String),
    /// 一時 dir を本来の名前へ差し替えられない
    Replace(String),
}

impl InstallError {
    /// 表示言語を明示しての文言（#2365）。(バリアント, 言語) の網羅 match で、日本語は #2365 より前の
    /// 固定文とバイト一致（`tests/issue1762_dispatch_error_lang.rs` が突き合わせる）。中身の OS の文は
    /// どちらの言語でもそのまま
    pub fn text_in(&self, lang: crate::i18n::Lang) -> String {
        use crate::i18n::Lang::{En, Ja};
        match (self, lang) {
            (Self::NoDataDir, Ja) => crate::paths::NoDataDir.text_in(Ja).into(),
            (Self::NoDataDir, En) => crate::paths::NoDataDir.text_in(En).into(),
            (Self::CreateRoot { path, error }, Ja) => format!("{path} を作れない: {error}"),
            (Self::CreateRoot { path, error }, En) => format!("cannot create {path}: {error}"),
            (Self::Write(e), Ja) => format!("mod を書き出せない: {e}"),
            (Self::Write(e), En) => format!("cannot write out the mod: {e}"),
            (Self::Replace(e), Ja) => format!("mod を差し替えられない: {e}"),
            (Self::Replace(e), En) => format!("cannot replace the mod: {e}"),
        }
    }
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(crate::i18n::lang()))
    }
}

/// 同梱の mod を `<data_dir>/claude-mod/tako/` へ展開する（冪等）。
/// 発火は GUI 起動時と `tako setup`。戻り値は展開先
pub fn install() -> Result<(PathBuf, InstallOutcome), InstallError> {
    let root = mod_root().ok_or(InstallError::NoDataDir)?;
    let outcome = install_in(&root, env!("CARGO_PKG_VERSION"))?;
    Ok((root.join(MOD_NAME), outcome))
}

/// [`install`] の本体。`root` の下の [`MOD_NAME`] へ書く（テストは一時 dir を渡す）。
///
/// 差し替えは「一時 dir に全部書く → Claude Code の型定義だけ引き継ぐ → 旧フォルダを退避 →
/// 一時 dir を本来の名前へ rename → 退避を消す」。ファイルを 1 本ずつ上書きすると、
/// ホットリロードが新しい `register.ts` と古い `hooks.json` の組を読みうる
pub fn install_in(root: &Path, version: &str) -> Result<InstallOutcome, InstallError> {
    let dest = root.join(MOD_NAME);
    let files = rendered_files(version);
    if matches_installed(&dest, &files) {
        return Ok(InstallOutcome::Unchanged);
    }
    std::fs::create_dir_all(root).map_err(|e| InstallError::CreateRoot {
        path: root.display().to_string(),
        error: e.to_string(),
    })?;
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
        return Err(InstallError::Write(e.to_string()));
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
        return Err(InstallError::Replace(e.to_string()));
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
    /// 中身は衝突そのもの（説明は応答を組むときに表示言語で引く。#2365）。`Box` にするのは
    /// 大きな変種があると enum にタグが要って大きくなり、`ModSnapshot` 越しに
    /// `OffloadJob` の変種の大きさの差が clippy の閾値を越えるため（単体テストで大きさを固定）
    NameConflict(Box<crate::claude_mod_install::Conflict>),
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

    /// 表示言語での説明（`tako mod` の応答。`tako check-health` は [`Self::describe_in`] の日本語）
    pub fn describe(&self) -> String {
        self.describe_in(crate::i18n::lang())
    }

    /// 表示言語を明示しての説明（#2365）。(バリアント, 言語) の網羅 match で、日本語は #2365 より前の
    /// 固定文とバイト一致（`tests/issue1762_dispatch_error_lang.rs` が突き合わせる）
    pub fn describe_in(&self, lang: crate::i18n::Lang) -> String {
        use crate::i18n::Lang::{En, Ja};
        match (self, lang) {
            (Self::AbOff, Ja) => format!("{AB_OFF_ENV} が立っている（A/B で注入を止めている）"),
            (Self::AbOff, En) => format!("{AB_OFF_ENV} is set (injection is stopped for A/B)"),
            (Self::Disabled, Ja) => "tako mod off で止めている（`tako mod on` で戻る）".into(),
            (Self::Disabled, En) => {
                "stopped with tako mod off (`tako mod on` turns it back on)".into()
            }
            (Self::NotInstalled, Ja) => "mod を data dir へ展開できていない".into(),
            (Self::NotInstalled, En) => "the mod could not be unpacked into the data dir".into(),
            (Self::NoCli, Ja) => "実行中の tako の隣に tako CLI が無い".into(),
            (Self::NoCli, En) => "no tako CLI next to the running tako".into(),
            (Self::ClaudeUnknown, Ja) => {
                "claude の版がまだ分からない（claude が入っていないか判定前）".into()
            }
            (Self::ClaudeUnknown, En) => "the claude version is not known yet (claude is not \
                 installed or has not been checked yet)"
                .into(),
            (Self::ClaudeTooOld(v), Ja) => {
                format!("claude {v} は下限 {MIN_CLAUDE_VERSION} 未満（`claude update` で上がる）")
            }
            (Self::ClaudeTooOld(v), En) => format!(
                "claude {v} is below the minimum {MIN_CLAUDE_VERSION} (`claude update` upgrades it)"
            ),
            (Self::NameConflict(conflict), Ja) => format!(
                "{}（利用者の tako を優先し、このペインには注入しない）",
                conflict.describe_in(Ja)
            ),
            (Self::NameConflict(conflict), En) => format!(
                "{} (the user's tako takes precedence; not injected into this pane)",
                conflict.describe_in(En)
            ),
            (Self::UserDisabled, Ja) => {
                "Claude Code の /plugin で tako@skills-dir が止められている（利用者の選択を優先）"
                    .into()
            }
            (Self::UserDisabled, En) => "tako@skills-dir is disabled in Claude Code's /plugin \
                 (the user's choice takes precedence)"
                .into(),
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

/// S7-4 の A/B（[`CARDS_LEGACY_ENV`]）が立っているか
pub fn cards_legacy() -> bool {
    std::env::var_os(CARDS_LEGACY_ENV).is_some_and(|v| !v.is_empty())
}

/// #2159 の A/B（[`CARD_CLICK_LEGACY_ENV`]）が立っているか
pub fn card_click_legacy() -> bool {
    std::env::var_os(CARD_CLICK_LEGACY_ENV).is_some_and(|v| !v.is_empty())
}

/// S7-6 の A/B（[`CHAT_LEGACY_ENV`]）が立っているか
pub fn chat_legacy() -> bool {
    std::env::var_os(CHAT_LEGACY_ENV).is_some_and(|v| !v.is_empty())
}

/// #1903 の A/B（[`LIMITS_LEGACY_ENV`]）が立っているか
pub fn limits_legacy() -> bool {
    std::env::var_os(LIMITS_LEGACY_ENV).is_some_and(|v| !v.is_empty())
}

/// #2017 の A/B（[`MONITOR_LEGACY_ENV`]）が立っているか
pub fn monitor_legacy() -> bool {
    std::env::var_os(MONITOR_LEGACY_ENV).is_some_and(|v| !v.is_empty())
}

/// #2041 の A/B（[`SESSION_LEGACY_ENV`]）が立っているか
pub fn session_legacy() -> bool {
    std::env::var_os(SESSION_LEGACY_ENV).is_some_and(|v| !v.is_empty())
}

/// #1891 の A/B（[`NO_REPORT_LEGACY_ENV`]）が立っているか
pub fn no_report_legacy() -> bool {
    std::env::var_os(NO_REPORT_LEGACY_ENV).is_some_and(|v| !v.is_empty())
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
    /// 直近の帯の描画で**押せる形**（Copy / Run のボタンつき）で描いたコマンドカードの id
    /// （S7-4 #1963）。無い = カードを描かない mod（S7-4 前・応答に `view.cards` が無かった）。
    /// GPUI のカード帯はここに載った id を出さない（[`card_owner`]）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cards: Option<Vec<u64>>,
    /// チャット風の描画で今あてている組（S7-6 #1964。応答の `view.chat` をそのまま受けて描いている）。
    /// 無い = チャットを描かない mod（S7-6 前・応答に `view.chat` が無かった）。GUI モードで
    /// チャットを mod に持たせる条件（[`chat_owner`]）はこれがあること
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat: Option<ModChatRenders>,
}

/// チャット風の描画で mod が今あてている組（S7-6 #1964。報告の `renders.chat`）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModChatRenders {
    /// 利用者の発話を吹き出しに描く
    #[serde(default)]
    pub bubble: bool,
    /// AI の返答のコードブロックの下にコピーのボタンを描く
    #[serde(default)]
    pub code_copy: bool,
    /// ツールの呼び出しを 1 行の要約に描く
    #[serde(default)]
    pub tool_summary: bool,
    /// 会話の行が足されるたびにすぐ報告している（応答の `view.chat.notify` を受けた。GPUI のチャットは
    /// これが真の報告の `chat_seq` だけを読み直しの合図にする = [`chat_reread`]）
    #[serde(default)]
    pub notify: bool,
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
    /// 会話の行が足された回数（S7-6 #1964。mod が起きてからの `session.append` のうち本筋の会話 =
    /// サブエージェントでない user / assistant の行。**数だけ**で本文・uuid は載せない）。GPUI のチャットは
    /// これが変わったときだけ transcript を読み直す（[`chat_reread`]。`renders.chat.notify` が真のときだけ）。
    /// S7-6 前の mod は送らない
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_seq: Option<u64>,
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
/// 報告の `renders.cards` の上限（1 ペインに置けるカードは [`crate::command_card::MAX_CARDS_PER_PANE`]）
const MAX_RENDERED_CARDS: usize = 16;

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
    if let Some(cards) = report.renders.as_mut().and_then(|r| r.cards.as_mut()) {
        cards.truncate(MAX_RENDERED_CARDS);
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
            // この説明は日本語の文だけを持つので、理由も日本語で引く（1 つの文に日英を混ぜない。#2365）
            ModUnavailable::NotInjected(reason) => reason.describe_in(crate::i18n::Lang::Ja),
        }
    }
}

// --- 報告の無い claude の理由（#1891） -------------------------------------------

/// 報告の無い claude を調べるペインの上限（#1891）。調べるのは `tako mod` / `check-health` の背景の段で、
/// 1 回の走査（プロセスの表 + 器の `list-panes -a`）を共有してペインごとに argv と起動時の env を読む。
/// 上限は「固まった GUI の切り分けにも使う `check-health` を、ペインの多い機で遅くしない」ため
pub const NO_REPORT_PROBE_LIMIT: usize = 8;

/// 報告の無い claude を調べた材料（#1891。背景の段が実プロセスから採る）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NoReportEvidence {
    /// ペインの claude のコマンド行（空白で繋いだもの。見つからない・読めない環境では `None`）
    pub argv: Option<String>,
    /// claude の**起動時**の env に、この tako の展開先を含む [`PLUGIN_DIRS_ENV`] があったか。
    /// 読めない（claude のプロセスが見つからない・Apple のプラットフォームバイナリ・macOS 以外）なら `None`
    pub injected_env: Option<bool>,
}

/// 報告の無い claude の理由（#1891。候補を並べず、材料から 1 つに決める）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoReportCause {
    /// `claude --safe-mode`（mod を読まない）
    SafeMode,
    /// `claude --bare`（mod を読まない）
    Bare,
    /// 起動時の env に注入の印が無い = 注入より前から動いていた claude（tako の再起動前から等）
    BeforeInjection,
    /// 注入は届いているのに報告が無い（設定・組織のポリシー・mod の読み込みの失敗のどれか）
    NotLoaded,
    /// 調べられなかった（上限の外・A/B・claude のプロセスが見つからない・macOS 以外）
    Undetermined,
}

impl NoReportCause {
    pub fn code(self) -> &'static str {
        match self {
            NoReportCause::SafeMode => "safe_mode",
            NoReportCause::Bare => "bare",
            NoReportCause::BeforeInjection => "before_injection",
            NoReportCause::NotLoaded => "not_loaded",
            NoReportCause::Undetermined => "undetermined",
        }
    }

    /// 表示言語での説明（`tako mod` の応答。`tako check-health` は [`Self::describe_in`] の日本語）
    pub fn describe(self) -> &'static str {
        self.describe_in(crate::i18n::lang())
    }

    /// 表示言語を明示しての説明（#2365）。(バリアント, 言語) の網羅 match で、日本語は #2365 より前の
    /// 固定文とバイト一致（`tests/issue1762_dispatch_error_lang.rs` が突き合わせる）
    pub fn describe_in(self, lang: crate::i18n::Lang) -> &'static str {
        use crate::i18n::Lang::{En, Ja};
        match (self, lang) {
            (Self::SafeMode, Ja) => {
                "claude が --safe-mode で起動されていて mod を読まない（--safe-mode を外して起動し直せば載る）"
            }
            (Self::SafeMode, En) => {
                "claude was started with --safe-mode and does not load mods (restart it without \
                 --safe-mode to load it)"
            }
            (Self::Bare, Ja) => {
                "claude が --bare で起動されていて mod を読まない（--bare を外して起動し直せば載る）"
            }
            (Self::Bare, En) => {
                "claude was started with --bare and does not load mods (restart it without --bare \
                 to load it)"
            }
            (Self::BeforeInjection, Ja) => {
                "注入より前から動いていた claude（起動時の環境に tako の注入の印が無い）。\
                 新しいペインで claude を起動すれば載る"
            }
            (Self::BeforeInjection, En) => {
                "this claude was running before the injection (its startup environment has no \
                 tako injection mark); starting claude in a new pane loads it"
            }
            (Self::NotLoaded, Ja) => {
                "claude には注入が届いているが mod から報告が無い。disableAllHooks / 組織の \
                 allowManagedModsOnly で mod が読まれていないか、mod の読み込みに失敗した"
            }
            (Self::NotLoaded, En) => {
                "the injection reached claude but the mod sends no reports; disableAllHooks / \
                 the organization's allowManagedModsOnly may keep the mod from loading, or \
                 loading the mod failed"
            }
            (Self::Undetermined, Ja) => {
                "claude は動いているが mod から報告が無い。--safe-mode / --bare / \
                 disableAllHooks / 組織の allowManagedModsOnly で mod が読まれていないか、\
                 注入より前から動いていた claude"
            }
            (Self::Undetermined, En) => {
                "claude is running but the mod sends no reports; --safe-mode / --bare / \
                 disableAllHooks / the organization's allowManagedModsOnly may keep the mod \
                 from loading, or this claude was running before the injection"
            }
        }
    }
}

/// 報告の無い claude の理由を材料から 1 つに決める（#1891。**判定はこの 1 本**。純関数）。
///
/// 順は「確実なものから」: コマンド行のフラグは注入の有無に関係なく mod を止めるので先に見る。
/// フラグは `--` より前の語だけを見る（プロンプトの本文の中の `--bare` を拾わない）
pub fn classify_no_report(evidence: Option<&NoReportEvidence>) -> NoReportCause {
    let Some(evidence) = evidence else {
        return NoReportCause::Undetermined;
    };
    let flag = |name: &str| {
        evidence.argv.as_deref().is_some_and(|argv| {
            argv.split_whitespace()
                .skip(1)
                .take_while(|w| *w != "--")
                .any(|w| w == name)
        })
    };
    if flag("--safe-mode") {
        return NoReportCause::SafeMode;
    }
    if flag("--bare") {
        return NoReportCause::Bare;
    }
    match evidence.injected_env {
        Some(false) => NoReportCause::BeforeInjection,
        Some(true) => NoReportCause::NotLoaded,
        None => NoReportCause::Undetermined,
    }
}

/// 起動時の env（`names` に [`PLUGIN_DIRS_ENV`] と `PATH` を挙げて読んだもの）に、この tako の
/// 展開先が載っていたか（#1891。純関数）。
///
/// `PATH` も無い = env をそもそも読めていない（Apple のプラットフォームバイナリは 0 キーで返る）
/// ので `None`。展開先が分からなければ判定しない
pub fn env_has_injection(env: &HashMap<String, String>, plugin_dir: Option<&Path>) -> Option<bool> {
    let plugin_dir = plugin_dir?;
    if !env.contains_key("PATH") {
        return None;
    }
    Some(
        env.get(PLUGIN_DIRS_ENV)
            .is_some_and(|dirs| dirs.split(LIST_SEP).any(|d| Path::new(d) == plugin_dir)),
    )
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

/// 監視がペインの会話 ID・状態をどこから採るか（#2017。[`monitor_plan`] の答え）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonitorPlan {
    /// mod の報告が答え。`claude agents --json` を起こさない
    Mod {
        /// 報告が運んだ会話 ID（`register.ts` の `$.session.id()`）
        session_id: String,
    },
    /// 走査へ落とす。`why` は応答の `agents_scan.reason`（[`ModUnavailable::code`] と同じ語彙 +
    /// `mod_aging` / `mod_no_session` / `session_mismatch`）
    Scan { why: &'static str },
}

impl MonitorPlan {
    /// 走査を省いたか
    pub fn skips_scan(&self) -> bool {
        matches!(self, MonitorPlan::Mod { .. })
    }

    /// 応答の `agents_scan`（CLI / MCP の両方が同じ JSON を読む）
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            MonitorPlan::Mod { .. } => {
                serde_json::json!({ "skipped": true, "reason": "mod_fresh" })
            }
            MonitorPlan::Scan { why } => serde_json::json!({ "skipped": false, "reason": why }),
        }
    }
}

/// 監視の状態を mod の報告で答えるか、`claude agents --json` の走査へ落とすか（#2017。純関数）。
///
/// mod の報告で答えるのは次がすべて揃うときだけ（どれか欠けたら従来どおり走査 = 判定は変えない）:
///
/// 1. 引き当てが成功（[`lookup`]。`FRESH_FOR` 以内・注入済み・A/B の S2 が立っていない）
/// 2. 最後の報告が [`MONITOR_SKIP_FOR`] 以内（heartbeat が届き続けている = claude は生きている）
/// 3. 報告が会話 ID を持つ（`claude agents --json` が返す答えの片方。無ければ答えにならない）
/// 4. 呼び出し側が会話 ID を名指ししている（レジストリの記録）なら、報告の会話 ID と同じ
///    （`/clear` 等で食い違うなら、名指しの会話について走査が答える = 修正前と同じ判定）
///
/// `legacy` は [`monitor_legacy`] の値（テストは env を触らずに両アームを検査できる）
pub fn monitor_plan(
    looked_up: &Result<&StoredReport, ModUnavailable>,
    now: Instant,
    expected_session: Option<&str>,
    legacy: bool,
) -> MonitorPlan {
    if legacy {
        return MonitorPlan::Scan { why: "legacy_env" };
    }
    let stored = match looked_up {
        Ok(stored) => stored,
        Err(why) => return MonitorPlan::Scan { why: why.code() },
    };
    if now.saturating_duration_since(stored.received) > MONITOR_SKIP_FOR {
        return MonitorPlan::Scan { why: "mod_aging" };
    }
    let Some(session_id) = stored
        .report
        .session_id
        .as_deref()
        .filter(|s| !s.is_empty())
    else {
        return MonitorPlan::Scan {
            why: "mod_no_session",
        };
    };
    if expected_session.is_some_and(|expected| expected != session_id) {
        return MonitorPlan::Scan {
            why: "session_mismatch",
        };
    }
    MonitorPlan::Mod {
        session_id: session_id.to_string(),
    }
}

/// レジストリが記録した worker の会話 ID を、mod の報告が運ぶ今の会話 ID へ追従させるか（#2041。純関数）。
///
/// `/clear` と `/resume` は**同じ claude のプロセスで別の会話が続く**（mod は次の報告で新しい会話 ID を
/// 運ぶ = `register.ts` の `session.end`）。レジストリは spawn 直後の会話 ID のままなので、名指しが
/// 食い違って [`monitor_plan`] が `session_mismatch` へ落ち、監視が 5 秒ごとに走査し続けていた
/// （走査は古い会話 ID について答え、応答の `resolved_session_id` も `/clear` 前の会話を指したまま。
/// 器のあるペインはセッションカタログの 30 秒走査が書き戻すまで、器の無いペインはずっと）。
///
/// 追従する（= 報告の会話 ID を返す）のは次がすべて揃うときだけ:
///
/// 1. 名指し（`expected`）が**レジストリの記録（`recorded`）そのもの**。呼び出し側が別の会話 ID を
///    明示したなら、その会話について走査が答える（#2017 の判定のまま）
/// 2. 報告が監視の答えになる（[`monitor_plan`] が名指しなしで `Mod`。20 秒以内・会話 ID つき）
/// 3. 報告の会話 ID が記録と違う
///
/// ペインの取り違え（再利用されたペイン ID）は呼び出し側が先に弾く（器の名前が食い違えば報告ごと
/// 捨てる = `verify_ctx_pane_identity`）。`legacy` は [`session_legacy`] の値
pub fn follow_session(
    looked_up: &Result<&StoredReport, ModUnavailable>,
    now: Instant,
    expected: Option<&str>,
    recorded: Option<&str>,
    legacy: bool,
) -> Option<String> {
    if legacy {
        return None;
    }
    let recorded = recorded.filter(|r| !r.is_empty())?;
    if expected != Some(recorded) {
        return None;
    }
    match monitor_plan(looked_up, now, None, false) {
        MonitorPlan::Mod { session_id } if session_id != recorded => Some(session_id),
        _ => None,
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
        // 切り出し安全: tz の先頭は rfind した ASCII の `+` / `-`
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
    /// 1 つ前の報告を受け取った時刻（S7-4 #1963。[`card_owner`] が「mod がカードを載せた応答を
    /// 受けてから 1 周報告したか」を見る。初めての報告では `None`）
    pub prev_received: Option<Instant>,
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
    pub install_result: Option<Result<InstallOutcome, InstallError>>,
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
    /// ui.json の `cards.place`（S7-4 #1963）。報告の応答を組むたびに ui.json から写す
    /// （GPUI の描画のたびにファイルを読まない。`tako mod ui` の変更は次の報告 = 最大 15 秒で効く）
    pub cards_place: crate::claude_mod_ui::Place,
    /// ui.json の `chat.place`（S7-6 #1964。[`Self::cards_place`] と同じく報告のたびに写す）
    pub chat_place: crate::claude_mod_ui::Place,
    /// 利用制限の観測と会話 → ログインの対応の材料（usage 分析 S1 #2200）。報告を受けるたびに
    /// 同じペインの前の報告と比べ、値が変わった枠・初めての会話だけを積む（heartbeat では積まない）。
    /// GUI の定期処理が汲み出し、鍵の解決と `<data_dir>/usage/` への追記は背景で行う
    /// （UI スレッドでファイルを読み書きしない）。上限は [`crate::usage::MAX_PENDING`]
    pub pending_usage: Vec<crate::usage::PendingUsage>,
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
        let prev = self.reports.get(&pane);
        if let Some(pending) = crate::usage::pending_from_report(prev.map(|s| &s.report), &report) {
            if self.pending_usage.len() >= crate::usage::MAX_PENDING {
                self.pending_usage.remove(0);
            }
            self.pending_usage.push(pending);
        }
        let count = prev.map_or(0, |s| s.count) + 1;
        let prev_received = prev.map(|s| s.received);
        self.reports.insert(
            pane,
            StoredReport {
                report,
                received: now,
                count,
                prev_received,
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

    /// 積んだ利用制限の観測の材料を取り出す（usage 分析 S1 #2200）
    pub fn take_pending_usage(&mut self) -> Vec<crate::usage::PendingUsage> {
        std::mem::take(&mut self.pending_usage)
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
    /// このペインのコマンドカード（S7-4 #1963。[`cards_view`]）。無い = mod はカードを描かない
    /// （ui.json の `cards.place` が `tako`・A/B の `TAKO_1963_LEGACY` / `TAKO_1877_S7_LEGACY`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cards: Option<CardsView>,
    /// チャット風の描画（S7-6 #1964。[`chat_view`]）。無い = mod はチャットを描かない（ui.json の
    /// `chat.place` が `tako`・A/B の `TAKO_1964_LEGACY` / `TAKO_1877_S7_LEGACY`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat: Option<ChatView>,
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
    /// このペインのコマンドカード（古い順。S7-4 #1963）
    pub cards: Vec<crate::CommandCard>,
    /// A/B の `TAKO_1963_LEGACY`（[`cards_legacy`]）
    pub cards_legacy: bool,
    /// tako がこのペインの TUI へ左クリックを渡すか（#2159。`tako mouse` が on ∧ #1961 / #2159 の A/B で
    /// ない。[`CardsView::click`] になる）
    pub card_click: bool,
    /// tako の表示モードが GUI ライク表示か（`ui_mode = gui`。S7-6 #1964）
    pub ui_mode_gui: bool,
    /// A/B の `TAKO_1964_LEGACY`（[`chat_legacy`]）
    pub chat_legacy: bool,
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
    let cards = cards_view(
        &input.cards,
        &input.ui,
        input.s7_legacy || input.cards_legacy,
        input.card_click,
    );
    let chat = chat_view(
        &input.ui,
        input.ui_mode_gui,
        input.s7_legacy || input.chat_legacy,
    );
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
        cards,
        chat,
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

// --- S7-4（#1963）: コマンドカードを mod で描く -------------------------------------------
//
// カードの正本は tako の store（`command_card`）。tako は応答の `view.cards` にこのペインのカードと、
// 押したときに叩く CLI の引数・帯のホットキーを載せる（mod の中でシェルの文字列も CLI の引数も
// 組まない = カスタムボタンと同じ規則）。mod は会話の行（`tako_show_command` の行）にカードを、
// 帯の tako の行に `Copy` / `Run` を描き、帯に押せる形で描いたカードの id を `renders.cards` で返す。
// GPUI のカード帯は [`card_owner`] が `Mod` と言ったカードを出さない

/// mod がカードを拾うまで GPUI の帯へ出さずに待つ上限（S7-4 #1963）。
///
/// AI が自分のペインへ出したカードは、mod が `tako_show_command` の呼び出しを見てすぐ報告し
/// （1 秒の flush）、その応答で描いて次の報告（さらに 1 秒）で `renders.cards` に載せる = 実測で
/// 1〜2.5 秒。その間に GPUI の帯を出すと viewport が一瞬縮んで戻る（claude が画面を組み直す）。
/// 上限を過ぎても拾われなければ GPUI の帯へ出す（別のペインから出したカードは mod が次の heartbeat
/// = 最大 15 秒まで知らないので、待たせすぎない）
pub const CARD_PICKUP_GRACE: Duration = Duration::from_secs(5);

/// 帯のカードのボタンのホットキー（tako が ui.json のボタンと重ならない英小文字を選ぶ）
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CardHotkeys {
    pub copy: String,
    pub run: String,
    pub next: String,
    pub dismiss: String,
}

/// コマンドの実行記録（`tako show-command --list` の `runs[]` と同じ語彙。#1724）
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CardRunView {
    pub pane: u64,
    /// `running` / `exited` / `closed`
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub count: u32,
}

/// カードのコマンド 1 件
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CardCommandView {
    /// **論理文字列**（AI が渡したまま。画面の折り返しは混ざらない = Copy に渡すのはこれ）
    pub text: String,
    /// Copy の代わりの経路（`$.ui.copy` が拒まれたとき）で叩く CLI の引数（tako の後ろの argv）
    pub copy_args: Vec<String>,
    /// Run で叩く CLI の引数（新しいペインで実行 = スマホ・GPUI のカードと同じ dispatch の `run`）
    pub run_args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<CardRunView>,
}

/// カード 1 枚
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CardView {
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub commands: Vec<CardCommandView>,
    /// 閉じるで叩く CLI の引数
    pub dismiss_args: Vec<String>,
}

/// 応答の `view.cards`（mod の契約は `types/index.d.ts` の `TakoCards`）
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CardsView {
    pub hotkeys: CardHotkeys,
    /// 会話の行のカードにクリックで押す「実行 / コピー」を置いてよいか（#2159）。tako がこのペインの TUI へ
    /// 左クリックを渡すとき（`tako mouse` が on ∧ A/B でない）だけ真。mod はさらに全画面（`isFullscreen`）の
    /// ときだけ置く（main screen はマウス報告を要求しない = 押せない）。偽なら帯で押す案内だけ（#1963 の形）
    pub click: bool,
    /// 古い順（store の並び）。帯は新しいものから選ぶ
    pub items: Vec<CardView>,
}

/// カードの操作で叩く CLI の引数（tako の後ろの argv。`tako show-command` のフラグそのまま）
pub fn card_cli_args(op: &str, card: u64, index: Option<usize>) -> Vec<String> {
    let mut argv = vec!["show-command".to_string(), format!("--{op}")];
    argv.push("--card".into());
    argv.push(card.to_string());
    if let Some(index) = index {
        argv.push("--index".into());
        argv.push(index.to_string());
    }
    argv
}

/// 帯のカードのボタンのホットキーを選ぶ（純関数）。ui.json のボタンの hotkey と重ならず、互いにも
/// 重ならない英小文字。数字は使わない（エンジンの仕様で、空の入力欄で打った数字が帯のボタンを押す =
/// 打鍵の弾みで Run が走る）。好みの字（copy = y / run = r / next = n / dismiss = x）が埋まっていれば
/// 次の候補、それも埋まっていれば a〜z の空き
pub fn card_hotkeys(buttons: &[crate::claude_mod_ui::Button]) -> CardHotkeys {
    let mut taken: BTreeSet<char> = buttons
        .iter()
        .filter_map(|b| {
            let mut chars = b.hotkey.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(c),
                _ => None,
            }
        })
        .collect();
    let mut pick = |prefer: &[char]| -> String {
        let c = prefer
            .iter()
            .copied()
            .chain('a'..='z')
            .find(|c| !taken.contains(c))
            .unwrap_or('z');
        taken.insert(c);
        c.to_string()
    };
    CardHotkeys {
        copy: pick(&['y', 'o', 'p']),
        run: pick(&['r', 'u', 'e']),
        next: pick(&['n', 'j', 't']),
        dismiss: pick(&['x', 'd', 'q']),
    }
}

/// 応答の `view.cards` を組む（純関数）。`off` は描かせない理由（ui.json の `cards.place` が `tako`・
/// A/B）で、そのときは載せない = mod はカードを描かない
pub fn cards_view(
    cards: &[crate::CommandCard],
    ui: &crate::claude_mod_ui::UiConfig,
    legacy: bool,
    click: bool,
) -> Option<CardsView> {
    if legacy || ui.cards.place == crate::claude_mod_ui::Place::Tako {
        return None;
    }
    let items = cards
        .iter()
        .map(|card| {
            let id = card.id().as_u64();
            CardView {
                id,
                label: card.label().map(str::to_string),
                commands: card
                    .commands()
                    .iter()
                    .zip(card.runs())
                    .enumerate()
                    .map(|(i, (text, run))| CardCommandView {
                        text: text.clone(),
                        copy_args: card_cli_args("copy", id, Some(i + 1)),
                        run_args: card_cli_args("run", id, Some(i + 1)),
                        run: run.map(|r| CardRunView {
                            pane: r.pane.as_u64(),
                            state: r.state.as_str(),
                            exit_code: r.state.exit_code(),
                            count: r.count,
                        }),
                    })
                    .collect(),
                dismiss_args: card_cli_args("dismiss", id, None),
            }
        })
        .collect();
    Some(CardsView {
        hotkeys: card_hotkeys(&ui.buttons),
        click,
        items,
    })
}

/// GPUI のカード帯にそのカードを出すのは誰か（§9.6。**判断はこの 1 本**）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardOwner {
    /// GPUI のカード帯が今のまま出す（理由のコード）
    Tako(&'static str),
    /// mod が Claude Code の画面に描く（`drawn` = 報告の `renders.cards` に載った /
    /// `pending` = mod がまだ拾っていない（[`CARD_PICKUP_GRACE`] の間）/ `place_mod` = ui.json が mod）
    Mod(&'static str),
}

impl CardOwner {
    /// GPUI のカード帯から外すか
    pub fn yields(self) -> bool {
        matches!(self, Self::Mod(_))
    }

    /// `drawn_by` の語（`mod` / `tako`）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tako(_) => "tako",
            Self::Mod(_) => "mod",
        }
    }

    /// 理由のコード
    pub fn reason(self) -> &'static str {
        match self {
            Self::Tako(r) | Self::Mod(r) => r,
        }
    }
}

/// [`card_owner`] の入力
#[derive(Debug, Clone, Copy)]
pub struct CardOwnerInput<'a> {
    /// ui.json の `cards.place`（[`ModHub::cards_place`]）
    pub place: crate::claude_mod_ui::Place,
    /// カードのペインの**新鮮な**報告（45 秒以内。無い = mod なし・古い・終わった・休眠）
    pub stored: Option<&'a StoredReport>,
    /// A/B（[`cards_legacy`] か [`s7_legacy`]）
    pub legacy: bool,
    pub card: u64,
    /// カードを出した時刻（[`crate::CommandCard::shown_at`]）
    pub shown_at: Instant,
    pub now: Instant,
}

/// GPUI のカード帯からそのカードを外すか（§9.6「新鮮な報告 ∧ `renders.cards` ∧ 置き場が mod 側」）。
///
/// - A/B・`cards.place = tako`・新鮮な報告が無い（mod なし・止まった・45 秒途切れた）・カードを描かない
///   mod（`renders.cards` が無い）→ `Tako`（今の表示）
/// - 報告の `renders.cards` に載っている → `Mod("drawn")`
/// - `cards.place = mod` → `Mod("place_mod")`（mod が効いている間は帯を出さない。帯を隠している等で
///   mod が描けなくても出さない = 利用者がそう選んだ）
/// - mod がまだ拾っていない（カードを載せた応答を受けてから 1 周報告していない）かつ
///   [`CARD_PICKUP_GRACE`] 以内 → `Mod("pending")`（viewport を一瞬縮めない）
/// - それ以外（拾ったのに描いていない = 帯を隠している・区切りに card が無い・幅が足りない・他の mod に
///   帯を隠された）→ `Tako("mod_not_drawn")`
pub fn card_owner(input: CardOwnerInput<'_>) -> CardOwner {
    use crate::claude_mod_ui::Place;
    if input.legacy {
        return CardOwner::Tako("legacy_env");
    }
    if input.place == Place::Tako {
        return CardOwner::Tako("place_tako");
    }
    let Some(stored) = input.stored else {
        return CardOwner::Tako("no_fresh_report");
    };
    let Some(drawn) = stored
        .report
        .renders
        .as_ref()
        .and_then(|r| r.cards.as_ref())
    else {
        return CardOwner::Tako("mod_not_drawing_cards");
    };
    if drawn.contains(&input.card) {
        return CardOwner::Mod("drawn");
    }
    if input.place == Place::Mod {
        return CardOwner::Mod("place_mod");
    }
    // カードを載せた応答は「カードの後に届いた最初の報告」への応答。mod がそれを描いて返すのは
    // その次の報告 = 1 つ前の報告がカードより後なら、mod は 1 周見たうえで描かなかった
    let seen = stored.prev_received.is_some_and(|p| p > input.shown_at);
    if !seen && input.now.saturating_duration_since(input.shown_at) < CARD_PICKUP_GRACE {
        return CardOwner::Mod("pending");
    }
    CardOwner::Tako("mod_not_drawn")
}

// --- S7-6（#1964）: チャット風の描画を mod で描く ------------------------------------------
//
// 吹き出し（利用者の発話）・コードのコピー（AI の返答のコードブロックの下のボタン）・ツールの要約（ツールの
// 呼び出しを 1 行に）を mod が Claude Code の会話の行に描く。どれを描くかは tako が ui.json の `chat` と表示
// モードから決めて応答の `view.chat` に載せ（[`chat_view`]）、mod は今あてている組を報告の `renders.chat` で
// 返す。GUI モードでそのペインのチャットを mod が持つなら（[`chat_owner`]）、GPUI のチャットビューの代わりに
// ターミナル表示を出す（mod の描画は本物の Claude Code の画面そのもの = 入力欄のミラーも画面の推測も要らない）

/// `chat.place = auto` の GUI モードでチャットを持つのは誰か（S7-6 #1964 の**判断点** = 受け入れ 4）。
///
/// 判断が出るまでは今の GPUI のチャットビュー（tako）のまま。mod の描画は `tako mod ui set chat.place mod`
/// で選べる。判断が出たらこの 1 行を変える（ui.json の値は `auto` のままなので書き換え・移行は要らない）
pub const AUTO_CHAT_OWNER_GUI: crate::claude_mod_ui::Place = crate::claude_mod_ui::Place::Tako;

/// 応答の `view.chat`（mod の契約は `types/index.d.ts` の `TakoChat`）
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChatView {
    /// どの組か: `gui` = GUI モードで mod がチャットを持つ（ui.json の `gui_only` も描く）/
    /// `terminal` = ターミナル表示の利用者・GPUI のチャットの裏（`always` だけ描く）/
    /// `off` = ui.json の `chat.place = tako`（何も描かない = S7-6 前の見た目。通知だけは受ける）
    pub mode: &'static str,
    /// 利用者の発話を吹き出しに描く
    pub bubble: bool,
    /// AI の返答のコードブロックの下にコピーのボタン（と帯の「最新のコードをコピー」）を描く
    pub code_copy: bool,
    /// ツールの呼び出しを 1 行の要約に描く
    pub tool_summary: bool,
    /// 帯の「最新のコードをコピー」のホットキー（[`chat_hotkey`]）
    pub hotkey: String,
    /// 会話の行が足されたらすぐ報告してほしいか（GUI モードで GPUI のチャットが表に出ているペイン =
    /// transcript を通知で読み直す。#1883 の置き換え）。偽なら `chat_seq` は他の報告に相乗りするだけ
    /// （ターミナル表示・mod がチャットを持つペインで報告を増やさない）
    pub notify: bool,
}

/// `chat.place` を表示モードの中で解く（`auto` は GUI モードで [`AUTO_CHAT_OWNER_GUI`]）
pub fn resolve_chat_place(place: crate::claude_mod_ui::Place) -> crate::claude_mod_ui::Place {
    use crate::claude_mod_ui::Place;
    match place {
        Place::Auto => AUTO_CHAT_OWNER_GUI,
        other => other,
    }
}

/// 帯の「最新のコードをコピー」のホットキーを選ぶ（純関数）。ui.json のボタンとカードのボタン
/// （[`card_hotkeys`]）と重ならない英小文字。数字は使わない（空の入力欄で打った数字が帯のボタンを押す
/// エンジンの仕様 = カードと同じ理由）。好みは o（copy の 2 字目）→ k → g、埋まっていれば a〜z の空き
pub fn chat_hotkey(buttons: &[crate::claude_mod_ui::Button]) -> String {
    let cards = card_hotkeys(buttons);
    let mut taken: BTreeSet<char> = buttons
        .iter()
        .filter_map(|b| {
            let mut chars = b.hotkey.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(c),
                _ => None,
            }
        })
        .collect();
    for key in [&cards.copy, &cards.run, &cards.next, &cards.dismiss] {
        taken.extend(key.chars());
    }
    ['o', 'k', 'g']
        .into_iter()
        .chain('a'..='z')
        .find(|c| !taken.contains(c))
        .unwrap_or('z')
        .to_string()
}

/// 応答の `view.chat` を組む（純関数）。`legacy` は A/B（[`chat_legacy`] か [`s7_legacy`]）。
///
/// - A/B → 載せない = mod はチャットを描かず、通知もしない（S7-6 前）
/// - `chat.place = tako` → `off` の組（何も描かない = S7-6 前の見た目。GPUI のチャットのための通知だけ）
/// - GUI モードでチャットを mod が持つ（`mod`。`auto` は [`AUTO_CHAT_OWNER_GUI`]）→ `gui` の組:
///   `bubble` / `tool_summary` は `off` 以外で描く・`code_copy` は ui.json のまま
/// - それ以外（ターミナル表示・GUI モードでも GPUI のチャットが表に出る）→ `terminal` の組:
///   `bubble` / `tool_summary` は `always` のときだけ（**ターミナル表示の利用者の見た目を勝手に変えない**。
///   既定では `code_copy` だけ）
pub fn chat_view(
    ui: &crate::claude_mod_ui::UiConfig,
    ui_mode_gui: bool,
    legacy: bool,
) -> Option<ChatView> {
    use crate::claude_mod_ui::{ChatShow, Place};
    if legacy {
        return None;
    }
    let off = ui.chat.place == Place::Tako;
    let gui = ui_mode_gui && resolve_chat_place(ui.chat.place) == Place::Mod;
    let show = |s: ChatShow| match s {
        _ if off => false,
        ChatShow::Always => true,
        ChatShow::GuiOnly => gui,
        ChatShow::Off => false,
    };
    Some(ChatView {
        mode: if off {
            "off"
        } else if gui {
            "gui"
        } else {
            "terminal"
        },
        bubble: show(ui.chat.bubble),
        code_copy: !off && ui.chat.code_copy,
        tool_summary: show(ui.chat.tool_summary),
        hotkey: chat_hotkey(&ui.buttons),
        // GPUI のチャットが表に出るのは GUI モードで mod がチャットを持たないとき
        notify: ui_mode_gui && !gui,
    })
}

/// GUI モードでそのペインのチャットを誰が出すか（§9.6。**判断はこの 1 本**）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatOwner {
    /// GPUI のチャットビューが今のまま出す（理由のコード）
    Tako(&'static str),
    /// mod が Claude Code の会話の行に描く = GPUI はそのペインをターミナル表示にする（理由のコード）
    Mod(&'static str),
}

impl ChatOwner {
    /// GPUI のチャットビューを引っ込めるか
    pub fn yields(self) -> bool {
        matches!(self, Self::Mod(_))
    }

    /// `drawn_by` の語（`mod` / `tako`）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tako(_) => "tako",
            Self::Mod(_) => "mod",
        }
    }

    /// 理由のコード
    pub fn reason(self) -> &'static str {
        match self {
            Self::Tako(r) | Self::Mod(r) => r,
        }
    }
}

/// [`chat_owner`] の入力
#[derive(Debug, Clone, Copy)]
pub struct ChatOwnerInput<'a> {
    /// ui.json の `chat.place`（[`ModHub::chat_place`]）
    pub place: crate::claude_mod_ui::Place,
    /// tako の表示モードが GUI ライク表示か
    pub ui_mode_gui: bool,
    /// そのペインの**新鮮な**報告（45 秒以内。無い = mod なし・古い・終わった・休眠）
    pub stored: Option<&'a StoredReport>,
    /// A/B（[`chat_legacy`] か [`s7_legacy`]）
    pub legacy: bool,
}

/// GUI モードで GPUI のチャットビューを引っ込め、ターミナル表示（mod の描画）にするか
/// （§9.6「新鮮な報告 ∧ `renders.chat` ∧ 置き場が mod」）。
///
/// - A/B・ターミナル表示・`chat.place = tako`・`auto`（判断点が決まるまで = [`AUTO_CHAT_OWNER_GUI`]）→ `Tako`
/// - 新鮮な報告が無い（mod なし・止まった・45 秒途切れた・claude が終わった = 最後の報告で即座に消える・休眠）・
///   チャットを描かない mod（`renders.chat` が無い = S7-6 前）→ `Tako`（今の GPUI のチャット）
/// - それ以外 → `Mod("drawn")`。mod があてている組が `terminal` のまま（表示モードを変えた直後 = 次の報告の
///   応答まで最大 15 秒）でもターミナル表示にする（GPUI のチャット → 本物の画面 → 吹き出しが出る、の順で
///   切り替わる。表示が 2 回入れ替わるより、本物の画面へ 1 回で移る方を採る）
pub fn chat_owner(input: ChatOwnerInput<'_>) -> ChatOwner {
    use crate::claude_mod_ui::Place;
    if input.legacy {
        return ChatOwner::Tako("legacy_env");
    }
    if !input.ui_mode_gui {
        return ChatOwner::Tako("mode_terminal");
    }
    match input.place {
        Place::Tako => return ChatOwner::Tako("place_tako"),
        Place::Auto if resolve_chat_place(Place::Auto) == Place::Tako => {
            return ChatOwner::Tako("place_auto")
        }
        _ => {}
    }
    let Some(stored) = input.stored else {
        return ChatOwner::Tako("no_fresh_report");
    };
    if stored
        .report
        .renders
        .as_ref()
        .and_then(|r| r.chat)
        .is_none()
    {
        return ChatOwner::Tako("mod_not_drawing_chat");
    }
    ChatOwner::Mod("drawn")
}

/// GPUI のチャットが transcript を読み直すかの合図（S7-6 #1964 のフォールバック側 = #1883 の置き換え）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatReread {
    /// mod の報告の `chat_seq` が前回読んだときから変わった（= 会話の行が足された）→ 読み直す
    Changed(u64),
    /// 変わっていない → 読み直さない（ファイルの stat もしない = transcript をポーリングしない）
    Same(u64),
    /// 新しい報告に `chat_seq` が無い（mod なし・古い・S7-6 前の mod・A/B）→ 今までどおり mtime とサイズで見る
    Stamp,
}

impl ChatReread {
    /// 今回控える `chat_seq`（次の判断の `prev_seq`）
    pub fn seq(self) -> Option<u64> {
        match self {
            Self::Changed(seq) | Self::Same(seq) => Some(seq),
            Self::Stamp => None,
        }
    }
}

/// transcript を読み直すかを決める（純関数）。新しい報告（[`MONITOR_SKIP_FOR`] = 20 秒以内）の `chat_seq`
/// だけを合図にする: `renders.chat.notify` が真の mod は本筋の会話の行が足されるたびに dirty を立てて
/// 1 秒以内に報告するので、通知から 1 tick（2 秒）以内に読み直せる。報告が途切れかけた（20 秒）・通知して
/// いない mod（表示モードを変えた直後 = 次の応答まで・`chat_seq` を送らない S7-6 前の mod）・報告なし・A/B は
/// `Stamp`（今の経路）へ落とす。前回が mtime の判断だった（`prev_seq` が無い）なら `Changed` = 読む
pub fn chat_reread(
    looked_up: &Result<&StoredReport, ModUnavailable>,
    now: Instant,
    prev_seq: Option<u64>,
    legacy: bool,
) -> ChatReread {
    if legacy {
        return ChatReread::Stamp;
    }
    let Ok(stored) = looked_up else {
        return ChatReread::Stamp;
    };
    if now.saturating_duration_since(stored.received) > MONITOR_SKIP_FOR {
        return ChatReread::Stamp;
    }
    let notifying = stored
        .report
        .renders
        .as_ref()
        .and_then(|r| r.chat)
        .is_some_and(|c| c.notify);
    let Some(seq) = stored.report.chat_seq.filter(|_| notifying) else {
        return ChatReread::Stamp;
    };
    match prev_seq {
        Some(prev) if prev == seq => ChatReread::Same(seq),
        _ => ChatReread::Changed(seq),
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

    /// #2200: 報告を受けるたびに観測の材料を積むのは「値が変わった・会話が新しい」ときだけ
    /// （heartbeat では積まない = UI スレッドの仕事を比べて積むだけにする）
    #[test]
    fn issue2200_観測の材料は値が変わったときだけ積む() {
        let t0 = Instant::now();
        let mut hub = ModHub::new(true);
        let r = |pct| {
            report_with(
                None,
                vec![limit("five_hour", pct, "2026-10-16T18:20:00.000Z", 1)],
            )
        };
        hub.accept(1, r(10.0), t0);
        assert_eq!(hub.pending_usage.len(), 1, "初めての報告");
        for _ in 0..5 {
            hub.accept(1, r(10.0), t0);
        }
        assert_eq!(hub.pending_usage.len(), 1, "同じ値の heartbeat で増えた");
        hub.accept(1, r(11.0), t0);
        assert_eq!(hub.pending_usage.len(), 2);
        let taken = hub.take_pending_usage();
        assert_eq!(taken[1].limits[0].percent_used, 11.0);
        assert!(hub.pending_usage.is_empty());
        // 汲み出されないまま溜まっても上限で古いものから捨てる
        for i in 0..(crate::usage::MAX_PENDING + 10) {
            hub.accept(2, r(i as f64), t0);
        }
        assert_eq!(hub.pending_usage.len(), crate::usage::MAX_PENDING);
        assert_eq!(
            hub.pending_usage.last().unwrap().limits[0].percent_used,
            (crate::usage::MAX_PENDING + 9) as f64
        );
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

    // --- S7-4（#1963）-----------------------------------------------------------------

    /// renders.cards を持つ報告（`cards` が None ならカードを描かない mod）
    fn card_report(cards: Option<Vec<u64>>) -> ModReport {
        let mut v = report_json();
        v["renders"] = serde_json::json!({
            "band": true, "buttons": 1, "band_hook": true, "hint_hook": true, "cards": cards,
        });
        parse_report(v).unwrap()
    }

    fn stored(report: ModReport, received: Instant, prev: Option<Instant>) -> StoredReport {
        StoredReport {
            report,
            received,
            count: 2,
            prev_received: prev,
        }
    }

    /// #1963: renders.cards は後方互換で読め（無い = カードを描かない mod）、上限で切られる
    #[test]
    fn issue1963_報告のrenders_cardsは後方互換で読める() {
        let r = card_report(Some(vec![4, 5]));
        assert_eq!(r.renders.unwrap().cards, Some(vec![4, 5]));
        assert_eq!(renders(None, true, true).renders.unwrap().cards, None);
        let r = card_report(Some((0..40).collect()));
        assert_eq!(
            r.renders.unwrap().cards.unwrap().len(),
            MAX_RENDERED_CARDS,
            "おかしな数の id でメモリを食わせない"
        );
    }

    /// #1963: GPUI のカード帯から外すのは「新鮮な報告 ∧ renders.cards ∧ 置き場が mod 側」と、
    /// mod がまだ拾っていない新しいカードの猶予の間だけ
    #[test]
    fn issue1963_カードの持ち主は報告と置き場と猶予で決まる() {
        use crate::claude_mod_ui::Place;
        let now = Instant::now();
        let shown = now - Duration::from_secs(1);
        let before = shown - Duration::from_secs(1);
        let after = shown + Duration::from_millis(500);
        fn input(
            stored: Option<&StoredReport>,
            shown: Instant,
            now: Instant,
        ) -> CardOwnerInput<'_> {
            CardOwnerInput {
                place: crate::claude_mod_ui::Place::Auto,
                stored,
                legacy: false,
                card: 5,
                shown_at: shown,
                now,
            }
        }
        let base = |stored| input(stored, shown, now);
        // 描いた
        let drawn = stored(card_report(Some(vec![4, 5])), now, Some(after));
        assert_eq!(card_owner(base(Some(&drawn))), CardOwner::Mod("drawn"));
        assert!(card_owner(base(Some(&drawn))).yields());
        // A/B・置き場 tako は描いていても GPUI の帯のまま
        assert_eq!(
            card_owner(CardOwnerInput {
                legacy: true,
                ..base(Some(&drawn))
            }),
            CardOwner::Tako("legacy_env")
        );
        assert_eq!(
            card_owner(CardOwnerInput {
                place: Place::Tako,
                ..base(Some(&drawn))
            }),
            CardOwner::Tako("place_tako")
        );
        // 新鮮な報告が無い（mod なし・止まった・45 秒途切れた・終わった）
        assert_eq!(card_owner(base(None)), CardOwner::Tako("no_fresh_report"));
        // カードを描かない mod（S7-4 前・応答に view.cards が無かった）
        let old = stored(renders(None, true, true), now, Some(after));
        assert_eq!(
            card_owner(base(Some(&old))),
            CardOwner::Tako("mod_not_drawing_cards")
        );
        // mod がまだ拾っていない: カードの後の報告が 1 つだけ（その応答でカードを受け取ったところ）
        let first = stored(card_report(Some(vec![4])), after, Some(before));
        assert_eq!(card_owner(base(Some(&first))), CardOwner::Mod("pending"));
        // 猶予を過ぎたら GPUI の帯へ出す（別のペインから出して mod が heartbeat まで知らない等）
        assert_eq!(
            card_owner(CardOwnerInput {
                now: shown + CARD_PICKUP_GRACE,
                ..base(Some(&first))
            }),
            CardOwner::Tako("mod_not_drawn")
        );
        // 拾ったのに描かなかった（帯を隠している・幅が足りない）= 猶予の中でも GPUI の帯へ
        let seen = stored(card_report(Some(vec![4])), now, Some(after));
        assert_eq!(
            card_owner(base(Some(&seen))),
            CardOwner::Tako("mod_not_drawn")
        );
        // 置き場 mod は拾ったのに描かなくても外す（mod が効いている間は GPUI の帯を出さない）
        assert_eq!(
            card_owner(CardOwnerInput {
                place: Place::Mod,
                ..base(Some(&seen))
            }),
            CardOwner::Mod("place_mod")
        );
        // 置き場 mod でも mod が居なければ GPUI の帯（カードを見失わない）
        assert_eq!(
            card_owner(CardOwnerInput {
                place: Place::Mod,
                ..base(None)
            }),
            CardOwner::Tako("no_fresh_report")
        );
        // 初めての報告（1 つ前が無い）は拾っていない扱い
        let fresh = stored(card_report(Some(vec![])), after, None);
        assert_eq!(card_owner(base(Some(&fresh))), CardOwner::Mod("pending"));
        assert_eq!(CardOwner::Mod("drawn").as_str(), "mod");
        assert_eq!(CardOwner::Tako("x").reason(), "x");
    }

    /// #1963: 報告を受けるたびに 1 つ前の受信時刻を控える（拾ったかの判断の材料）
    #[test]
    fn issue1963_報告は1つ前の受信時刻を持つ() {
        let mut hub = ModHub::new(true);
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_secs(1);
        hub.accept(3, card_report(Some(vec![])), t0);
        assert_eq!(hub.reports[&3].prev_received, None);
        hub.accept(3, card_report(Some(vec![])), t1);
        assert_eq!(hub.reports[&3].prev_received, Some(t0));
        assert_eq!(hub.reports[&3].received, t1);
    }

    /// #1963: 帯のカードのホットキーは ui.json のボタンと重ならない英小文字（数字は使わない）
    #[test]
    fn issue1963_カードのホットキーはボタンと重ならない英字() {
        use crate::claude_mod_ui::{Button, ButtonAction, SlashCommand};
        let button = |hotkey: &str| Button {
            id: format!("b-{hotkey}"),
            label: "b".into(),
            hotkey: hotkey.into(),
            action: ButtonAction::Slash {
                command: SlashCommand::Compact,
            },
        };
        let keys = card_hotkeys(&[button("c")]);
        assert_eq!(
            (
                keys.copy.as_str(),
                keys.run.as_str(),
                keys.next.as_str(),
                keys.dismiss.as_str()
            ),
            ("y", "r", "n", "x")
        );
        // 好みの字が埋まっていれば次の候補、それも埋まっていれば a〜z の空き
        let taken: Vec<Button> = ["y", "o", "p", "r", "u", "e", "n", "j", "t", "x"]
            .iter()
            .map(|k| button(k))
            .collect();
        let keys = card_hotkeys(&taken);
        let got = [&keys.copy, &keys.run, &keys.next, &keys.dismiss];
        assert_eq!(keys.dismiss, "d");
        for key in got {
            assert!(
                key.len() == 1 && key.chars().all(|c| c.is_ascii_lowercase()),
                "{key}"
            );
            assert!(
                !taken.iter().any(|b| &b.hotkey == key),
                "{key} がボタンと重なる"
            );
        }
        let mut uniq: Vec<&String> = vec![&keys.copy, &keys.run, &keys.next, &keys.dismiss];
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), 4, "互いにも重ならない");
    }

    /// #1963: 応答の view.cards は store の論理文字列と tako が組んだ CLI の引数。置き場 tako・A/B では載せない
    #[test]
    fn issue1963_view_cardsは置き場とabで出し分ける() {
        let pane = crate::Pane::new(crate::PaneOrigin::User).id();
        let run_pane = crate::Pane::new(crate::PaneOrigin::User).id();
        let long = format!("cargo build -p tako-cli && {}", "x".repeat(140));
        let mut store = crate::CommandCards::new();
        let id = store
            .show(pane, &[long.clone(), "a \\\n  && b".into()], Some("ビルド"))
            .unwrap();
        store.record_run(id, 1, run_pane).unwrap();
        let cards: Vec<crate::CommandCard> = store.list(Some(pane)).into_iter().cloned().collect();
        let input =
            |ui: crate::claude_mod_ui::UiConfig, cards_legacy: bool, s7_legacy: bool| BandInput {
                pane: pane.as_u64(),
                lang: "ja",
                ui,
                cards: cards.clone(),
                cards_legacy,
                s7_legacy,
                ..BandInput::default()
            };
        let view = band_view(input(Default::default(), false, false));
        let got = view.cards.expect("auto は載せる");
        let card = &got.items[0];
        assert_eq!(card.id, id.as_u64());
        assert_eq!(card.label.as_deref(), Some("ビルド"));
        assert_eq!(
            card.commands[0].text, long,
            "論理文字列のまま（折り返しの改行が無い）"
        );
        assert!(!card.commands[0].text.contains('\n'));
        assert_eq!(
            card.commands[1].text, "a \\\n  && b",
            "複数行は本物の改行を保つ"
        );
        let n = id.as_u64().to_string();
        assert_eq!(
            card.commands[1].run_args,
            [
                "show-command",
                "--run",
                "--card",
                n.as_str(),
                "--index",
                "2"
            ]
        );
        assert_eq!(
            card.commands[0].copy_args,
            [
                "show-command",
                "--copy",
                "--card",
                n.as_str(),
                "--index",
                "1"
            ]
        );
        assert_eq!(
            card.dismiss_args,
            ["show-command", "--dismiss", "--card", n.as_str()]
        );
        let run = card.commands[0].run.as_ref().expect("実行記録");
        assert_eq!(
            (run.pane, run.state, run.count),
            (run_pane.as_u64(), "running", 1)
        );
        assert!(card.commands[1].run.is_none());
        // JSON の形（mod の契約 = TakoCards）
        let v = serde_json::to_value(&got).unwrap();
        assert_eq!(v["hotkeys"]["copy"], "y");
        assert!(v["items"][0]["commands"][1].get("run").is_none());
        // 置き場 tako・A/B（#1963 / S7-3）では載せない = mod はカードを描かない
        let mut tako = crate::claude_mod_ui::UiConfig::default();
        tako.cards.place = crate::claude_mod_ui::Place::Tako;
        for (ui, legacy, s7) in [
            (tako, false, false),
            (Default::default(), true, false),
            (Default::default(), false, true),
        ] {
            let v = serde_json::to_value(band_view(input(ui, legacy, s7))).unwrap();
            assert!(v.get("cards").is_none(), "{v}");
        }
        // カードが 0 枚でも載せる（mod は「描かせる」と分かる = renders.cards を空で返す）
        let empty = band_view(BandInput {
            cards: Vec::new(),
            ..input(Default::default(), false, false)
        });
        assert_eq!(empty.cards.map(|c| c.items.len()), Some(0));
    }

    /// #2159: view.cards.click は「tako がこのペインへクリックを渡すか」をそのまま運ぶ（mod は真のときだけ
    /// 会話の行に「実行 / コピー」のボタンを置く）。カードを載せない置き場・A/B では cards ごと無い
    #[test]
    fn issue2159_view_cards_clickはクリックの転送をそのまま運ぶ() {
        let pane = crate::Pane::new(crate::PaneOrigin::User).id();
        let mut store = crate::CommandCards::new();
        store.show(pane, &["npm test".to_string()], None).unwrap();
        let cards: Vec<crate::CommandCard> = store.list(Some(pane)).into_iter().cloned().collect();
        let input = |card_click: bool| BandInput {
            pane: pane.as_u64(),
            lang: "ja",
            cards: cards.clone(),
            card_click,
            ..BandInput::default()
        };
        let on = band_view(input(true)).cards.expect("auto は載せる");
        assert!(on.click);
        let off = band_view(input(false))
            .cards
            .expect("クリックが届かなくても載せる");
        assert!(
            !off.click,
            "クリックが届かない = 帯で押す案内だけ（#1963 の形）"
        );
        // JSON の形（mod の契約 = TakoCards.click）
        let v = serde_json::to_value(&on).unwrap();
        assert_eq!(v["click"], serde_json::json!(true));
        // 既定の BandInput（呼び出し側が知らない = 古い経路）は押せない側
        assert!(!BandInput::default().card_click);
        // 置き場 tako では cards ごと載せない（click も無い）
        let mut tako = crate::claude_mod_ui::UiConfig::default();
        tako.cards.place = crate::claude_mod_ui::Place::Tako;
        let v = serde_json::to_value(band_view(BandInput {
            ui: tako,
            ..input(true)
        }))
        .unwrap();
        assert!(v.get("cards").is_none(), "{v}");
    }

    // --- #2017: 監視の走査を mod の報告で置き換える判断 ---------------------------

    fn stored_2017(session_id: Option<&str>, received: Instant) -> StoredReport {
        let mut v = report_json();
        match session_id {
            Some(sid) => v["session_id"] = serde_json::json!(sid),
            None => {
                v.as_object_mut().unwrap().remove("session_id");
            }
        }
        stored(parse_report(v).unwrap(), received, None)
    }

    /// 閾値の関係: 生きている mod（heartbeat 15 秒）は走査へ落ちず、値を使う期限（45 秒）より先に落とす
    #[test]
    fn issue2017_走査を省く鮮度はheartbeatより長くfresh_forより短い() {
        assert!(
            MONITOR_SKIP_FOR > HEARTBEAT,
            "生きている mod の報告の合間で走査へ落ちる"
        );
        assert!(
            MONITOR_SKIP_FOR < FRESH_FOR,
            "落ちた claude（ended を送れない）を走査が拾うのが FRESH_FOR まで遅れる"
        );
    }

    /// 新しい報告に会話 ID があれば走査しない。閾値ちょうどは省き、1 ms 過ぎたら落とす
    #[test]
    fn issue2017_新しい報告があれば走査しない_閾値ちょうどの境界() {
        let t0 = Instant::now();
        let s = stored_2017(Some("s-1"), t0);
        let plan = |now: Instant| monitor_plan(&Ok(&s), now, None, false);
        assert_eq!(
            plan(t0),
            MonitorPlan::Mod {
                session_id: "s-1".into()
            }
        );
        assert_eq!(
            plan(t0 + MONITOR_SKIP_FOR),
            MonitorPlan::Mod {
                session_id: "s-1".into()
            },
            "閾値ちょうどは省く（is_fresh と同じ <=）"
        );
        assert_eq!(
            plan(t0 + MONITOR_SKIP_FOR + Duration::from_millis(1)),
            MonitorPlan::Scan { why: "mod_aging" },
            "heartbeat が途絶えた報告で走査を省くと、落ちた claude の gone を拾えない"
        );
    }

    /// mod が使えない・答えにならないときは必ず走査へ落ちる（理由つき）
    #[test]
    fn issue2017_modが無い古い止まった報告は走査へ落ちる() {
        let t0 = Instant::now();
        let now = t0 + Duration::from_secs(1);
        let cases: Vec<(Result<&StoredReport, ModUnavailable>, &str)> = vec![
            (Err(ModUnavailable::Absent), "mod_absent"),
            (Err(ModUnavailable::Stale { age_secs: 50 }), "mod_stale"),
            (Err(ModUnavailable::Legacy), "legacy_env"),
            (
                Err(ModUnavailable::NotInjected(OffReason::Disabled)),
                OffReason::Disabled.code(),
            ),
        ];
        for (looked_up, why) in cases {
            assert_eq!(
                monitor_plan(&looked_up, now, None, false),
                MonitorPlan::Scan { why },
                "{why}"
            );
        }
        let no_sid = stored_2017(None, t0);
        assert_eq!(
            monitor_plan(&Ok(&no_sid), now, None, false),
            MonitorPlan::Scan {
                why: "mod_no_session"
            }
        );
        let empty_sid = stored_2017(Some(""), t0);
        assert_eq!(
            monitor_plan(&Ok(&empty_sid), now, None, false),
            MonitorPlan::Scan {
                why: "mod_no_session"
            }
        );
    }

    /// 名指しの会話 ID（レジストリ）と報告が食い違えば走査（修正前と同じ判定）。一致なら省く
    #[test]
    fn issue2017_名指しの会話と食い違えば走査へ落ちる() {
        let t0 = Instant::now();
        let s = stored_2017(Some("s-new"), t0);
        assert_eq!(
            monitor_plan(&Ok(&s), t0, Some("s-old"), false),
            MonitorPlan::Scan {
                why: "session_mismatch"
            }
        );
        assert!(monitor_plan(&Ok(&s), t0, Some("s-new"), false).skips_scan());
    }

    /// A/B: 立っていれば新しい報告があっても走査（修正前）
    #[test]
    fn issue2017_legacyでは新しい報告があっても走査する() {
        let t0 = Instant::now();
        let s = stored_2017(Some("s-1"), t0);
        assert_eq!(
            monitor_plan(&Ok(&s), t0, None, true),
            MonitorPlan::Scan { why: "legacy_env" }
        );
    }

    /// 引き当て（ModHub::lookup）と通しで: 報告が止まると 20 秒で走査、45 秒で引き当て自体が失効
    #[test]
    fn issue2017_報告が止まると走査へ落ちる_hubを通す() {
        let t0 = Instant::now();
        let mut hub = ModHub::new(true);
        hub.accept(7, parse_report(report_json()).unwrap(), t0);
        let plan = |hub: &ModHub, now: Instant| monitor_plan(&hub.lookup(7, now), now, None, false);
        assert!(plan(&hub, t0 + HEARTBEAT).skips_scan());
        assert_eq!(
            plan(&hub, t0 + Duration::from_secs(30)),
            MonitorPlan::Scan { why: "mod_aging" }
        );
        assert_eq!(
            plan(&hub, t0 + Duration::from_secs(50)),
            MonitorPlan::Scan { why: "mod_stale" }
        );
        // ended（claude の正常終了）は報告ごと捨てる = 走査へ落ちて gone を拾う
        let mut ended = parse_report(report_json()).unwrap();
        ended.ended = true;
        hub.accept(7, ended, t0 + Duration::from_secs(1));
        assert_eq!(
            plan(&hub, t0 + Duration::from_secs(2)),
            MonitorPlan::Scan { why: "mod_absent" }
        );
    }

    #[test]
    fn issue2017_応答のagents_scanの形() {
        let skipped = MonitorPlan::Mod {
            session_id: "s".into(),
        }
        .to_json();
        assert_eq!(skipped["skipped"], true);
        assert_eq!(skipped["reason"], "mod_fresh");
        let ran = MonitorPlan::Scan { why: "mod_absent" }.to_json();
        assert_eq!(ran["skipped"], false);
        assert_eq!(ran["reason"], "mod_absent");
    }

    // --- #2041: レジストリの会話 ID を mod の報告へ追従させる判断 ---------------------

    /// `/clear` の後（報告 = 新しい会話・名指し = レジストリの古い記録）は報告の会話 ID へ追従する
    #[test]
    fn issue2041_clearの後はレジストリの記録を報告の会話へ追従させる() {
        let t0 = Instant::now();
        let s = stored_2017(Some("s-new"), t0);
        assert_eq!(
            follow_session(&Ok(&s), t0, Some("s-old"), Some("s-old"), false),
            Some("s-new".to_string())
        );
        // 追従した後は名指しと報告が一致する = 監視は走査しない
        assert!(monitor_plan(&Ok(&s), t0, Some("s-new"), false).skips_scan());
    }

    /// 追従しない: 一致している / 明示の名指し / 記録が無い / 報告が答えにならない / A/B
    #[test]
    fn issue2041_追従しない条件() {
        let t0 = Instant::now();
        let s = stored_2017(Some("s-new"), t0);
        let follow = |looked_up: &Result<&StoredReport, ModUnavailable>,
                      now: Instant,
                      expected: Option<&str>,
                      recorded: Option<&str>| {
            follow_session(looked_up, now, expected, recorded, false)
        };
        assert_eq!(
            follow(&Ok(&s), t0, Some("s-new"), Some("s-new")),
            None,
            "一致"
        );
        assert_eq!(
            follow(&Ok(&s), t0, Some("s-asked"), Some("s-old")),
            None,
            "呼び出し側が記録と違う会話を明示した = その会話について走査が答える（#2017 のまま）"
        );
        assert_eq!(follow(&Ok(&s), t0, None, Some("s-old")), None, "名指しなし");
        assert_eq!(follow(&Ok(&s), t0, Some(""), Some("")), None, "空の記録");
        assert_eq!(follow(&Ok(&s), t0, Some("s-old"), None), None, "記録なし");
        assert_eq!(
            follow(
                &Ok(&s),
                t0 + MONITOR_SKIP_FOR + Duration::from_millis(1),
                Some("s-old"),
                Some("s-old")
            ),
            None,
            "heartbeat が途絶えた報告で記録を書き換えない（mod_aging）"
        );
        let no_sid = stored_2017(None, t0);
        assert_eq!(
            follow(&Ok(&no_sid), t0, Some("s-old"), Some("s-old")),
            None,
            "会話 ID の無い報告"
        );
        assert_eq!(
            follow(
                &Err(ModUnavailable::Absent),
                t0,
                Some("s-old"),
                Some("s-old")
            ),
            None,
            "報告なし"
        );
        assert_eq!(
            follow_session(&Ok(&s), t0, Some("s-old"), Some("s-old"), true),
            None,
            "A/B（TAKO_2041_LEGACY）は追従しない = 修正前"
        );
    }

    // --- S7-6（#1964）-----------------------------------------------------------------

    /// renders.chat / chat_seq を持つ報告（`chat` が None ならチャットを描かない mod）
    fn chat_report(chat: Option<(bool, bool, bool)>, seq: Option<u64>) -> ModReport {
        chat_report_notify(chat, seq, true)
    }

    fn chat_report_notify(
        chat: Option<(bool, bool, bool)>,
        seq: Option<u64>,
        notify: bool,
    ) -> ModReport {
        let mut v = report_json();
        v["renders"] = serde_json::json!({
            "band": true, "buttons": 1, "band_hook": true, "hint_hook": true,
        });
        if let Some((bubble, code_copy, tool_summary)) = chat {
            v["renders"]["chat"] = serde_json::json!({
                "bubble": bubble, "code_copy": code_copy, "tool_summary": tool_summary, "notify": notify,
            });
        }
        if let Some(seq) = seq {
            v["chat_seq"] = serde_json::json!(seq);
        }
        parse_report(v).unwrap()
    }

    /// #1964: renders.chat と chat_seq は後方互換で読める（無い = チャットを描かない mod / 数えない mod）
    #[test]
    fn issue1964_報告のrenders_chatとchat_seqは後方互換で読める() {
        let r = chat_report(Some((true, true, false)), Some(7));
        assert_eq!(
            r.renders.as_ref().unwrap().chat,
            Some(ModChatRenders {
                bubble: true,
                code_copy: true,
                tool_summary: false,
                notify: true,
            })
        );
        assert_eq!(r.chat_seq, Some(7));
        let old = parse_report(report_json()).unwrap();
        assert_eq!(old.chat_seq, None);
        assert_eq!(old.renders.as_ref().and_then(|r| r.chat), None);
        // 本文の欄を持たない（数と真偽だけ）
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(
            v["renders"]["chat"],
            serde_json::json!({"bubble": true, "code_copy": true, "tool_summary": false, "notify": true})
        );
    }

    /// #1964: view.chat は ui.json の chat × 表示モードで決まる。ターミナル表示の既定はコードのコピーだけ・
    /// GUI モードでも `auto` は判断点が決まるまで GPUI のチャットの裏 = terminal の組
    #[test]
    fn issue1964_view_chatは置き場と表示モードで組が決まる() {
        use crate::claude_mod_ui::{ChatShow, Place, UiConfig};
        let ui = UiConfig::default();
        assert_eq!(ui.chat.place, Place::Auto, "既定の置き場");
        // ターミナル表示: 既定はコードのコピーだけ（受け入れ 3 = 吹き出しを出さない）・通知しない
        let v = chat_view(&ui, false, false).unwrap();
        assert_eq!(
            (v.mode, v.bubble, v.code_copy, v.tool_summary, v.notify),
            ("terminal", false, true, false, false)
        );
        // GUI モード + auto（判断点が決まるまで GPUI のチャット = mod は terminal の組 + 通知）
        assert_eq!(AUTO_CHAT_OWNER_GUI, Place::Tako);
        let v = chat_view(&ui, true, false).unwrap();
        assert_eq!(
            (v.mode, v.bubble, v.tool_summary, v.notify),
            ("terminal", false, false, true)
        );
        // GUI モード + mod: gui_only も描く・GPUI のチャットは裏なので通知しない
        let mut on = ui.clone();
        on.chat.place = Place::Mod;
        let v = chat_view(&on, true, false).unwrap();
        assert_eq!(
            (v.mode, v.bubble, v.code_copy, v.tool_summary, v.notify),
            ("gui", true, true, true, false)
        );
        // mod でもターミナル表示なら gui_only は描かない
        let v = chat_view(&on, false, false).unwrap();
        assert_eq!(
            (v.mode, v.bubble, v.tool_summary),
            ("terminal", false, false)
        );
        // always はターミナル表示でも描く・off は GUI でも描かない
        let mut mixed = on.clone();
        mixed.chat.bubble = ChatShow::Always;
        mixed.chat.tool_summary = ChatShow::Off;
        mixed.chat.code_copy = false;
        let v = chat_view(&mixed, false, false).unwrap();
        assert_eq!(
            (v.bubble, v.code_copy, v.tool_summary),
            (true, false, false)
        );
        let v = chat_view(&mixed, true, false).unwrap();
        assert_eq!(
            (v.bubble, v.code_copy, v.tool_summary),
            (true, false, false)
        );
        // place tako = 何も描かない（S7-6 前の見た目）。GUI モードなら GPUI のチャットのために通知だけ
        let mut off = ui.clone();
        off.chat.place = Place::Tako;
        off.chat.bubble = ChatShow::Always;
        let v = chat_view(&off, true, false).unwrap();
        assert_eq!(
            (v.mode, v.bubble, v.code_copy, v.tool_summary, v.notify),
            ("off", false, false, false, true)
        );
        assert!(!chat_view(&off, false, false).unwrap().notify);
        // A/B は載せない（描かない・通知もしない）
        assert_eq!(chat_view(&on, true, true), None);
        // band_view は表示モードと A/B（S7 / #1964）を通す
        let base = |ui: UiConfig, gui: bool, chat_legacy: bool, s7_legacy: bool| BandInput {
            pane: 7,
            lang: "ja",
            ui,
            ui_mode_gui: gui,
            chat_legacy,
            s7_legacy,
            ..BandInput::default()
        };
        assert_eq!(
            band_view(base(on.clone(), true, false, false))
                .chat
                .map(|c| c.mode),
            Some("gui")
        );
        assert_eq!(band_view(base(on.clone(), true, true, false)).chat, None);
        assert_eq!(band_view(base(on.clone(), true, false, true)).chat, None);
        let v = serde_json::to_value(band_view(base(ui, false, false, false))).unwrap();
        assert_eq!(v["chat"]["mode"], "terminal");
        assert_eq!(v["chat"]["hotkey"], "o");
    }

    /// #1964: 帯の「最新のコードをコピー」のホットキーはボタンとカードのホットキーに重ならない英小文字
    #[test]
    fn issue1964_コードのコピーのホットキーはボタンとカードに重ならない() {
        use crate::claude_mod_ui::{Button, ButtonAction, SlashCommand};
        let b = |hotkey: &str| Button {
            id: format!("b-{hotkey}"),
            label: "x".into(),
            hotkey: hotkey.into(),
            action: ButtonAction::Slash {
                command: SlashCommand::Compact,
            },
        };
        assert_eq!(chat_hotkey(&[]), "o");
        // y を取られるとカードの copy が o へ移る = コードは k
        assert_eq!(card_hotkeys(&[b("y")]).copy, "o");
        assert_eq!(chat_hotkey(&[b("y")]), "k");
        assert_eq!(chat_hotkey(&[b("o"), b("k"), b("g")]), "a");
        let key = chat_hotkey(&[b("c")]);
        let cards = card_hotkeys(&[b("c")]);
        for taken in ["c", &cards.copy, &cards.run, &cards.next, &cards.dismiss] {
            assert_ne!(key, taken);
        }
        assert!(
            key.chars().all(|c| c.is_ascii_lowercase()),
            "数字は使わない"
        );
    }

    /// #1964: GUI モードで GPUI のチャットを引っ込めるのは「GUI ∧ 置き場が mod ∧ 新鮮な報告 ∧ renders.chat」だけ
    #[test]
    fn issue1964_チャットの持ち主は表示モードと置き場と報告で決まる() {
        use crate::claude_mod_ui::Place;
        let now = Instant::now();
        let drawing = stored(chat_report(Some((true, true, true)), Some(3)), now, None);
        let old_mod = stored(chat_report(None, None), now, None);
        let input = |place, gui, stored, legacy| ChatOwnerInput {
            place,
            ui_mode_gui: gui,
            stored,
            legacy,
        };
        assert_eq!(
            chat_owner(input(Place::Mod, true, Some(&drawing), false)),
            ChatOwner::Mod("drawn")
        );
        assert!(chat_owner(input(Place::Mod, true, Some(&drawing), false)).yields());
        // 判断点が決まるまで auto は GPUI のチャットのまま
        assert_eq!(
            chat_owner(input(Place::Auto, true, Some(&drawing), false)),
            ChatOwner::Tako("place_auto")
        );
        assert_eq!(
            chat_owner(input(Place::Tako, true, Some(&drawing), false)),
            ChatOwner::Tako("place_tako")
        );
        assert_eq!(
            chat_owner(input(Place::Mod, false, Some(&drawing), false)),
            ChatOwner::Tako("mode_terminal")
        );
        assert_eq!(
            chat_owner(input(Place::Mod, true, Some(&drawing), true)),
            ChatOwner::Tako("legacy_env")
        );
        // mod が止まった・古い・終わった（新鮮な報告が無い）= GPUI のチャットへ戻る（受け入れ 2）
        assert_eq!(
            chat_owner(input(Place::Mod, true, None, false)),
            ChatOwner::Tako("no_fresh_report")
        );
        // チャットを描かない mod（S7-6 前）
        assert_eq!(
            chat_owner(input(Place::Mod, true, Some(&old_mod), false)),
            ChatOwner::Tako("mod_not_drawing_chat")
        );
        assert_eq!(ChatOwner::Mod("drawn").as_str(), "mod");
        assert_eq!(ChatOwner::Tako("x").reason(), "x");
    }

    /// #1964 のフォールバック側（#1883 の置き換え）: 新しい報告の chat_seq が変わったときだけ読み直す。
    /// 数えない mod・報告が途切れかけ・報告なし・A/B は mtime の判断へ落ちる
    #[test]
    fn issue1964_transcriptの読み直しはchat_seqの変化で決まる() {
        let now = Instant::now();
        let s = stored(chat_report(Some((false, true, false)), Some(5)), now, None);
        assert_eq!(
            chat_reread(&Ok(&s), now, Some(4), false),
            ChatReread::Changed(5)
        );
        assert_eq!(
            chat_reread(&Ok(&s), now, Some(5), false),
            ChatReread::Same(5)
        );
        assert_eq!(
            chat_reread(&Ok(&s), now, None, false),
            ChatReread::Changed(5),
            "前回が mtime の判断だった（mod が来た直後）は読む"
        );
        assert_eq!(chat_reread(&Ok(&s), now, Some(5), true), ChatReread::Stamp);
        assert_eq!(
            chat_reread(
                &Ok(&s),
                now + MONITOR_SKIP_FOR + Duration::from_millis(1),
                Some(5),
                false
            ),
            ChatReread::Stamp,
            "heartbeat が途絶えかけた報告を合図にしない"
        );
        let no_seq = stored(chat_report(None, None), now, None);
        assert_eq!(
            chat_reread(&Ok(&no_seq), now, Some(5), false),
            ChatReread::Stamp
        );
        // 数えていても通知していない（ターミナル表示の応答を受けたまま = 表示モードを変えた直後）は合図にしない
        let quiet_mod = stored(
            chat_report_notify(Some((false, true, false)), Some(5), false),
            now,
            None,
        );
        assert_eq!(
            chat_reread(&Ok(&quiet_mod), now, Some(4), false),
            ChatReread::Stamp
        );
        assert_eq!(
            chat_reread(&Err(ModUnavailable::Absent), now, Some(5), false),
            ChatReread::Stamp
        );
        assert_eq!(ChatReread::Same(5).seq(), Some(5));
        assert_eq!(ChatReread::Stamp.seq(), None);
    }

    // --- #1891: 報告の無い claude の理由 ---

    fn evidence(argv: Option<&str>, injected_env: Option<bool>) -> NoReportEvidence {
        NoReportEvidence {
            argv: argv.map(str::to_string),
            injected_env,
        }
    }

    #[test]
    fn 報告の無いclaudeの理由は材料から1つに決まる() {
        use NoReportCause::*;
        let cases: &[(Option<NoReportEvidence>, NoReportCause)] = &[
            // 調べていない（上限の外・A/B・macOS 以外）
            (None, Undetermined),
            // フラグは注入の有無に関係なく先に効く
            (
                Some(evidence(Some("claude --safe-mode"), Some(true))),
                SafeMode,
            ),
            (
                Some(evidence(Some("claude --model x --safe-mode"), Some(false))),
                SafeMode,
            ),
            (
                Some(evidence(Some("/x/claude/versions/2.1.294 --bare"), None)),
                Bare,
            ),
            // フラグが無ければ起動時の env の印で分ける
            (Some(evidence(Some("claude"), Some(false))), BeforeInjection),
            (
                Some(evidence(Some("claude --resume abc"), Some(true))),
                NotLoaded,
            ),
            (Some(evidence(Some("claude"), None)), Undetermined),
            // claude のプロセスが見つからない（SSH 先 等）= 材料なし
            (Some(evidence(None, None)), Undetermined),
            // argv[0] とプロンプトの本文（`--` の後）の語はフラグと読まない
            (Some(evidence(Some("--safe-mode"), Some(true))), NotLoaded),
            (
                Some(evidence(Some("claude -- --bare"), Some(false))),
                BeforeInjection,
            ),
            // 似た綴りのフラグは別物
            (
                Some(evidence(Some("claude --safe-mode-x"), Some(true))),
                NotLoaded,
            ),
        ];
        for (input, want) in cases {
            assert_eq!(classify_no_report(input.as_ref()), *want, "{input:?}");
        }
        // 語彙は重ならない（`tako mod` / `check-health` の `reason.cause`）
        let codes: BTreeSet<&str> = [SafeMode, Bare, BeforeInjection, NotLoaded, Undetermined]
            .iter()
            .map(|c| c.code())
            .collect();
        assert_eq!(codes.len(), 5);
        // 判別できなかったときは S1 の候補の文言のまま（`--safe-mode` を含む。日本語の字面で見る =
        // 表示言語のグローバルに依らない。#2365）
        let ja = crate::i18n::Lang::Ja;
        assert!(Undetermined.describe_in(ja).contains("--safe-mode"));
        assert!(BeforeInjection
            .describe_in(ja)
            .contains("新しいペインで claude を起動すれば載る"));
    }

    #[test]
    fn 起動時のenvの印はこのtakoの展開先だけを数える() {
        let dir = Path::new("/data/claude-mod/tako");
        let env = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect()
        };
        let sep = LIST_SEP.to_string();
        let ours = format!("/user/mods{sep}/data/claude-mod/tako/");
        assert_eq!(
            env_has_injection(
                &env(&[("PATH", "/bin"), (PLUGIN_DIRS_ENV, &ours)]),
                Some(dir)
            ),
            Some(true)
        );
        // 別のインスタンスの展開先だけ = この tako の注入は届いていない
        assert_eq!(
            env_has_injection(
                &env(&[
                    ("PATH", "/bin"),
                    (PLUGIN_DIRS_ENV, "/other/claude-mod/tako")
                ]),
                Some(dir)
            ),
            Some(false)
        );
        assert_eq!(
            env_has_injection(&env(&[("PATH", "/bin")]), Some(dir)),
            Some(false)
        );
        // env を読めていない（0 キー）・展開先が分からない = 判定しない
        assert_eq!(env_has_injection(&env(&[]), Some(dir)), None);
        assert_eq!(
            env_has_injection(&env(&[("PATH", "/bin"), (PLUGIN_DIRS_ENV, &ours)]), None),
            None
        );
    }
}
