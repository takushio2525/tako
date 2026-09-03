//! シェル統合（FR-2.4.1）— OSC 7 / 133 を発行するスクリプトの書き出しと自動注入
//!
//! `shell-integration/` のスクリプトをバイナリへ埋め込み、初回 spawn 時にデータ
//! ディレクトリへ書き出す。**そこから先の「届け方」が OS で違う**ので、
//! 抽象境界（B13）はこのモジュール自身が持つ。
//!
//! - unix: シェルが拾う環境変数を注入するだけで済む。**シェル判定はしない**
//!   （zsh は `ZDOTDIR`、bash は `PROMPT_COMMAND`、fish は `XDG_DATA_DIRS` しか
//!   見ないため、3 点セットを常時注入しても互いに無害）。ユーザーのファイルは触らない
//! - Windows: PowerShell に `ZDOTDIR` 相当の環境変数が無いので、`$PROFILE` へ
//!   マーカーで囲んだ 1 ブロックを書く（#525）。配置は [`install`] /
//!   [`uninstall`] / [`status`] で、CLI・MCP へ 1:1 で出す
//!
//! 無効化は `TAKO_NO_SHELL_INTEGRATION=1`（FR-2.4.4 の設定 UI までの暫定）。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::paths::data_dir;

const ZSH_ZSHENV: &str = include_str!("../shell-integration/zshenv.zsh");
const BASH_SCRIPT: &str = include_str!("../shell-integration/tako.bash");
const FISH_SCRIPT: &str = include_str!("../shell-integration/tako.fish");
const POWERSHELL_SCRIPT: &str = include_str!("../shell-integration/tako.ps1");

/// `$PROFILE` へ書くブロックの開始・終了マーカー。**この 2 行が管理範囲の定義**で、
/// 再配置（冪等）も解除もこの区間だけを見る。文字列を変えると既存の配置を
/// 見失うので、変えるときは移行を用意すること
#[cfg_attr(not(windows), allow(dead_code))]
const BLOCK_BEGIN: &str = "# >>> tako shell integration >>>";
#[cfg_attr(not(windows), allow(dead_code))]
const BLOCK_END: &str = "# <<< tako shell integration <<<";

/// 同梱している zsh-autosuggestions（MIT）の本体。出所とライセンスは
/// `shell-integration/zsh-autosuggestions/PROVENANCE.md` と `THIRD-PARTY-NOTICES.md`。
/// **実行時ダウンロードはしない**（オフラインでも動き、供給元の改変にも影響されない）
const ZSH_AUTOSUGGESTIONS: &str =
    include_str!("../shell-integration/zsh-autosuggestions/zsh-autosuggestions.zsh");
const ZSH_AUTOSUGGESTIONS_LICENSE: &str =
    include_str!("../shell-integration/zsh-autosuggestions/LICENSE");

/// 同梱している zsh-autosuggestions のバージョン（更新時は PROVENANCE.md と揃える）
pub const AUTOSUGGEST_VERSION: &str = "v0.7.1";

/// 確定キーのヒントを出す既定回数（Issue #614）。**コマンドライン 1 本につき 1 回**消費する。
/// zshenv.zsh 側の既定値（状態ファイルが無い / 壊れているときのフォールバック）と揃えること
pub const AUTOSUGGEST_HINT_DEFAULT: u32 = 10;

/// tako CLI の実行ファイル名（Windows は `tako.exe`）
fn cli_file_name() -> String {
    format!("tako{}", std::env::consts::EXE_SUFFIX)
}

/// spawn する子シェルに注入する統合用環境変数。プロセス内で一度だけ書き出して使い回す
/// 統合が spawn 時に撒く環境変数の**名前**（値は data dir 配下なのでインスタンスごとに
/// 変わるが、名前は固定）。
///
/// #1105: 器（tmux / psmux）のサーバーは**最初のクライアントの環境を引き継ぎ**、後続の
/// セッションもその stale な値を見る（実測: `ZDOTDIR=A` で起動したサーバー上に
/// `ZDOTDIR=B` のプロセスからセッションを作ると、中のシェルは A を見る）。
/// つまり同じ socket 名に**別インスタンスのサーバー**が残っていると、シェル統合は
/// 前のインスタンスの（消えているかもしれない）置き場を指し、OSC 7 / 133 が
/// 一切届かなくなる = cwd 追従とコマンド状態が黙って死ぬ。
/// これを避けるため、器はこの名前のぶんを `-e` でセッション作成時に固定する
/// （[`crate::backend::session_pinned_env`]）。
///
/// Windows は `$PROFILE` 経由で spawn 時の注入が無いので、この表は POSIX 側だけを指す
/// （名前が載っていても `options.env` に無ければ何も起きない）
pub const INJECTED_KEYS: &[&str] = &[
    // zsh
    "ZDOTDIR",
    "TAKO_ORIG_ZDOTDIR",
    // bash
    "PROMPT_COMMAND",
    // fish
    "XDG_DATA_DIRS",
];

pub fn env() -> &'static [(String, String)] {
    static ENV: OnceLock<Vec<(String, String)>> = OnceLock::new();
    ENV.get_or_init(|| {
        if disabled() {
            return Vec::new();
        }
        match write_scripts() {
            Ok(env) => env,
            Err(e) => {
                tracing::warn!("シェル統合スクリプトを書き出せない（統合なしで継続）: {e}");
                Vec::new()
            }
        }
    })
}

fn disabled() -> bool {
    std::env::var_os("TAKO_NO_SHELL_INTEGRATION").is_some_and(|v| !v.is_empty())
}

/// スクリプト一式を置くディレクトリ（`<data_dir>/shell-integration`）
pub fn integration_root() -> Option<PathBuf> {
    data_dir().map(|d| d.join("shell-integration"))
}

/// 入力予測の ON/OFF 状態（Issue #600）。zsh 側が毎プロンプト読む値と同じもの。
///
/// **なぜ環境変数ではなくファイルなのか**: 環境変数は spawn 時に凍結するので、
/// 設定を切り替えても既存ペインのシェルには一生届かない。zsh 側は毎プロンプト
/// このファイルを読むので、稼働中のシェルにも次のプロンプトから反映される
pub fn autosuggest_state() -> bool {
    integration_root()
        .map(|r| autosuggest_state_in(&r))
        .unwrap_or(true)
}

/// 状態ファイルの中身。**不在は ON**（既定 ON。Issue #600）
pub fn autosuggest_state_in(root: &Path) -> bool {
    match std::fs::read_to_string(root.join("autosuggest")) {
        Ok(s) => s.trim() != "off",
        Err(_) => true,
    }
}

/// 状態ファイルを書く（`root` は `integration_root()` 相当）
pub fn write_autosuggest_state_in(root: &Path, enabled: bool) -> std::io::Result<()> {
    write_state_file(root, "autosuggest", if enabled { "on" } else { "off" })
}

/// 入力予測の ON/OFF をシェル側へ反映する。データディレクトリを解決できなければ何もしない
pub fn set_autosuggest(enabled: bool) {
    let Some(root) = integration_root() else {
        return;
    };
    if let Err(e) = write_autosuggest_state_in(&root, enabled) {
        tracing::warn!("入力予測の状態を書き出せない: {e}");
    }
}

/// 状態ファイルを 1 行 1 値で書く（部分書き込みを読ませないよう tmp → rename）。
/// tmp 名はプロセス固有にする（プライマリ / セカンダリが同じ data_dir を共有しうるため）
fn write_state_file(root: &Path, name: &str, body: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(root)?;
    let tmp = root.join(format!("{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, root.join(name))
}

/// 確定キーのヒントの残り回数（Issue #614）。`None` = 恒久 OFF。
///
/// **不在・壊れた値は既定回数**（zsh 側と同じ倒し方。予測が出ないより出る方が既定に忠実）。
/// 残り回数は zsh 側が 1 コマンドラインごとに 1 減らして書き戻すので、
/// この値は「あと何回チュートリアルが出るか」をそのまま表す
pub fn autosuggest_hint_state_in(root: &Path) -> Option<u32> {
    match std::fs::read_to_string(root.join("autosuggest-hint")) {
        Ok(s) if s.trim() == "off" => None,
        Ok(s) => Some(s.trim().parse().unwrap_or(AUTOSUGGEST_HINT_DEFAULT)),
        Err(_) => Some(AUTOSUGGEST_HINT_DEFAULT),
    }
}

/// ヒントの残り回数を書く。`None` = 恒久 OFF
pub fn write_autosuggest_hint_state_in(root: &Path, remaining: Option<u32>) -> std::io::Result<()> {
    let body = match remaining {
        Some(n) => n.to_string(),
        None => "off".to_string(),
    };
    write_state_file(root, "autosuggest-hint", &body)
}

/// Tab でも確定できるか（Issue #614）。**不在は ON**（既定 ON）
pub fn autosuggest_tab_state_in(root: &Path) -> bool {
    match std::fs::read_to_string(root.join("autosuggest-tab")) {
        Ok(s) => s.trim() != "off",
        Err(_) => true,
    }
}

/// Tab 確定の ON/OFF を書く
pub fn write_autosuggest_tab_state_in(root: &Path, enabled: bool) -> std::io::Result<()> {
    write_state_file(root, "autosuggest-tab", if enabled { "on" } else { "off" })
}

/// シェルへ渡すヒント文言（Issue #614）。`(Tab 確定あり, Tab 確定なし)`。
///
/// **i18n は Rust 側に閉じる**（`tako lang` の切替が同じ経路で効く）。zsh は
/// 状態ファイルの中身をそのまま出すだけで、文言の組み立てはしない
pub fn autosuggest_hint_texts() -> (String, String) {
    autosuggest_hint_texts_for(crate::i18n::lang())
}

/// `autosuggest_hint_texts` の純粋関数版。表示言語のグローバル状態を触らずにテストできる
pub fn autosuggest_hint_texts_for(lang: crate::i18n::Lang) -> (String, String) {
    match lang {
        crate::i18n::Lang::Ja => ("[→ か Tab で確定]".into(), "[→ で確定]".into()),
        crate::i18n::Lang::En => ("[→ or Tab to accept]".into(), "[→ to accept]".into()),
    }
}

/// ヒント文言を書く（1 行目 = Tab 確定あり、2 行目 = Tab 確定なし）
pub fn write_autosuggest_hint_text_in(
    root: &Path,
    texts: &(String, String),
) -> std::io::Result<()> {
    write_state_file(
        root,
        "autosuggest-hint-text",
        &format!("{}\n{}\n", texts.0, texts.1),
    )
}

/// ヒントの ON/OFF をシェル側へ反映する。ON は残り回数を既定値へ戻す
/// （= もう一度チュートリアルを見せる、が素直な意味）
pub fn set_autosuggest_hint(enabled: bool) {
    let Some(root) = integration_root() else {
        return;
    };
    let remaining = enabled.then_some(AUTOSUGGEST_HINT_DEFAULT);
    if let Err(e) = write_autosuggest_hint_state_in(&root, remaining) {
        tracing::warn!("入力予測ヒントの状態を書き出せない: {e}");
    }
}

/// Tab 確定の ON/OFF をシェル側へ反映する
pub fn set_autosuggest_tab(enabled: bool) {
    let Some(root) = integration_root() else {
        return;
    };
    if let Err(e) = write_autosuggest_tab_state_in(&root, enabled) {
        tracing::warn!("入力予測の Tab 確定の状態を書き出せない: {e}");
    }
}

/// 現在の表示言語でヒント文言を書き直す。起動時と `tako lang` の切替時に呼ぶ
pub fn refresh_autosuggest_hint_text() {
    let Some(root) = integration_root() else {
        return;
    };
    if let Err(e) = write_autosuggest_hint_text_in(&root, &autosuggest_hint_texts()) {
        tracing::warn!("入力予測ヒントの文言を書き出せない: {e}");
    }
}

/// tako CLI 実体が置かれているディレクトリ（Issue #601）。
///
/// **実行中バイナリの隣を見て決める**ので、`.app` 起動なら `Contents/MacOS`、
/// dev ビルド（`cargo run -p tako-app`）なら `target/debug` を指す。パスは直書きしない。
/// CLI が隣に無い（dev で `tako-cli` を未ビルド等）なら `None` = 注入するものが無い
pub fn cli_dir() -> Option<PathBuf> {
    resolve_cli_dir(std::env::current_exe().ok().as_deref(), &|p| p.is_file())
}

/// `cli_dir` の純粋関数版。`exists` を差し替えられるのでテストできる
fn resolve_cli_dir(exe: Option<&Path>, exists: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    let dir = exe?.parent()?;
    // 改行を含むパスは 1 行 1 値の状態ファイルに載せられない（シェル側が誤読する）
    if dir.to_string_lossy().contains(['\n', '\r']) {
        return None;
    }
    exists(&dir.join(cli_file_name())).then(|| dir.to_path_buf())
}

/// 状態ファイルに記録されている CLI ディレクトリ。**空 = 注入しない**
pub fn cli_dir_state_in(root: &Path) -> Option<PathBuf> {
    let raw = std::fs::read_to_string(root.join("cli-dir")).ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}

/// CLI ディレクトリをシェルが読む状態ファイルへ書く（`root` は `integration_root()` 相当）。
///
/// **なぜ環境変数ではなくファイルなのか**: 入力予測（#600）と同じ理由に加えて、
/// PATH そのものを spawn 時の env で足すと「ユーザーが自分で通した tako があるか」を
/// tako-app のプロセス環境（Dock 起動だと最小構成）で判定することになり必ず誤る。
/// 判定はシェル側が rc を読み終えた後に行うのが唯一正しく、そのとき参照する値をここに置く
pub fn write_cli_dir_in(root: &Path, dir: Option<&Path>) -> std::io::Result<()> {
    let body = dir.map(|d| d.display().to_string()).unwrap_or_default();
    write_state_file(root, "cli-dir", &body)
}

/// 起動時に CLI ディレクトリを解決し直してシェル側へ反映する。
/// アプリの置き場所が変わっても（zip 展開先 → /Applications 等）次に開くシェルから正しくなる
pub fn refresh_cli_dir() {
    let Some(root) = integration_root() else {
        return;
    };
    if let Err(e) = write_cli_dir_in(&root, cli_dir().as_deref()) {
        tracing::warn!("tako CLI のディレクトリを書き出せない: {e}");
    }
}

/// スクリプト一式をデータディレクトリへ書き出し、注入 env を返す
fn write_scripts() -> std::io::Result<Vec<(String, String)>> {
    let Some(base) = data_dir() else {
        return Ok(Vec::new());
    };
    let root = base.join("shell-integration");

    let zsh_dir = root.join("zsh");
    std::fs::create_dir_all(&zsh_dir)?;
    std::fs::write(zsh_dir.join(".zshenv"), ZSH_ZSHENV)?;

    // 入力予測（Issue #600）。zshenv.zsh が最初のプロンプトで読み込む。
    // 読み込むかどうかは `autosuggest` 状態ファイル側で決めるので、置くのは常に行う
    let autosuggest_dir = root.join("zsh-autosuggestions");
    std::fs::create_dir_all(&autosuggest_dir)?;
    std::fs::write(
        autosuggest_dir.join("zsh-autosuggestions.zsh"),
        ZSH_AUTOSUGGESTIONS,
    )?;
    // MIT の義務（著作権表示とライセンス全文の添付）を配置先でも満たす
    std::fs::write(autosuggest_dir.join("LICENSE"), ZSH_AUTOSUGGESTIONS_LICENSE)?;

    // 確定キーのヒント文言（Issue #614）。残り回数と Tab 確定の状態ファイルは
    // 「不在 = 既定」で読めるので書かない（残り回数を毎起動で書くと一生減らない）
    if let Err(e) = write_autosuggest_hint_text_in(&root, &autosuggest_hint_texts()) {
        tracing::warn!("入力予測ヒントの文言を書き出せない: {e}");
    }

    // tako CLI の PATH 注入（Issue #601）。シェル側が最初のプロンプトで読む。
    // ここで失敗してもシェル統合（OSC 7 / 133）自体は成立させたいので致命扱いしない
    if let Err(e) = write_cli_dir_in(&root, cli_dir().as_deref()) {
        tracing::warn!("tako CLI のディレクトリを書き出せない: {e}");
    }

    let bash_path = root.join("tako.bash");
    std::fs::write(&bash_path, BASH_SCRIPT)?;

    let fish_conf_dir = root.join("fish-data/fish/vendor_conf.d");
    std::fs::create_dir_all(&fish_conf_dir)?;
    std::fs::write(fish_conf_dir.join("tako.fish"), FISH_SCRIPT)?;

    // PowerShell（#525）。unix でも置くだけは行う（配置しない = 使われないだけで無害。
    // OS で「置くもの」を出し分けると、Windows でだけ通る経路が macOS のテストから消える）
    write_powershell_script(&root)?;

    // ここから先の届け方は OS で違う（B13）
    Ok(imp::injected_env(&root, &bash_path))
}

/// PowerShell 用スクリプトの書き出し。**BOM 付き UTF-8** で置く
/// （Windows PowerShell 5.1 は BOM 無しの `.ps1` を ANSI コードページとして読むため、
/// BOM が無いと日本語環境ではスクリプトが化ける）
fn write_powershell_script(root: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(root)?;
    let path = root.join("tako.ps1");
    let mut bytes = Vec::with_capacity(POWERSHELL_SCRIPT.len() + 3);
    bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    bytes.extend_from_slice(POWERSHELL_SCRIPT.as_bytes());
    std::fs::write(&path, bytes)?;
    Ok(path)
}

// --- 配置 API（B13）。Windows は `$PROFILE` へ、unix は env 注入で完結する ---

/// この環境でシェル統合が対象にするシェル。
///
/// **`tako setup` の環境チェックがここを引く**。対応シェルの知識はこのモジュールが持つ
/// （マトリクスのキーは MCP ツール名と 1:1 なので、ツールではないシェル統合は
/// あちらに載せられない）
pub fn shells() -> &'static str {
    imp::SHELLS
}

/// 統合の届け方。**ユーザーに「何をすれば効くのか」を説明するために種類が要る**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// 環境変数の注入だけで済む（ユーザーのファイルを触らない）
    Automatic,
    /// ユーザーの `$PROFILE` へブロックを書く必要がある
    Profile,
}

impl Delivery {
    /// 応答・診断に出す識別子
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::Profile => "profile",
        }
    }
}

/// 配置先 1 件（PowerShell のエディションごと）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileTarget {
    /// 表示名（`PowerShell 7` / `Windows PowerShell 5.1`）
    pub label: String,
    /// 解決に使った実行ファイル
    pub exe: String,
    /// `$PROFILE.CurrentUserAllHosts`
    pub path: PathBuf,
    /// 管理ブロックが入っているか
    pub installed: bool,
    /// 入っているブロックが現行の内容と一致するか（`installed` が false なら常に false）
    pub up_to_date: bool,
}

/// 現在の配置状態
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub delivery: Delivery,
    /// 統合スクリプト本体（データディレクトリ配下）
    pub script: Option<PathBuf>,
    /// 配置先。`Automatic` では常に空
    pub targets: Vec<ProfileTarget>,
    /// 配置できていても**器（永続バックエンド）が OSC を通さない**ために
    /// 統合が届かない場合の説明。届くなら `None`（#525）
    pub blocked_by_backend: Option<String>,
}

impl Status {
    /// 配置そのものが済んでいるか（器の事情は見ない）
    pub fn installed(&self) -> bool {
        match self.delivery {
            Delivery::Automatic => true,
            Delivery::Profile => {
                !self.targets.is_empty() && self.targets.iter().all(|t| t.installed && t.up_to_date)
            }
        }
    }

    /// この環境で統合が**実際にペインへ効く**か。
    /// 配置済みでも器が OSC を落とすなら効かない（psmux。#525）
    pub fn effective(&self) -> bool {
        self.installed() && self.blocked_by_backend.is_none()
    }

    /// 診断・API 応答用の構造化表現（CLI / MCP がこれをそのまま返す）
    pub fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "delivery": self.delivery.as_str(),
            "shells": shells(),
            "script": self.script.as_ref().map(|p| p.display().to_string()),
            "installed": self.installed(),
            "effective": self.effective(),
            "blocked_by_backend": self.blocked_by_backend,
            "osc_transport": osc_transport().as_str(),
            "targets": self.targets.iter().map(|t| serde_json::json!({
                "label": t.label,
                "exe": t.exe,
                "path": t.path.display().to_string(),
                "installed": t.installed,
                "up_to_date": t.up_to_date,
            })).collect::<Vec<_>>(),
        })
    }
}

/// OSC がどの経路で tako へ届いているか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OscTransport {
    /// ペインの PTY をそのまま読む（器なし / 器が素通しする = macOS の tmux）
    Pty,
    /// 器が素通ししないので、統合スクリプトがファイルへ書いたものを読む（#766）
    SideChannel,
}

impl OscTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pty => "pty",
            Self::SideChannel => "side-channel",
        }
    }
}

/// この構成で OSC が届く経路。
///
/// **器に尋ねる**（実装名で分岐しない）。将来 psmux が素通しに対応したり、
/// 別の器を足したりしても、この 1 箇所の判定がそのまま追従する
pub fn osc_transport() -> OscTransport {
    if crate::backend::capabilities().osc_passthrough {
        OscTransport::Pty
    } else {
        OscTransport::SideChannel
    }
}

/// このプラットフォームの統合スクリプトが側路（[`crate::osc_sink`]）に対応しているか。
///
/// 対応していない環境で書き先を渡しても、スクリプトが見ないので**空のファイルが
/// 増えるだけ**。呼び出し側（`spawn_session`）はここを見てから張る
pub fn side_channel_supported() -> bool {
    imp::SIDE_CHANNEL
}

/// 器が OSC を通さず、かつ側路も無いときの説明（届くなら `None`）。
///
/// #525 の時点では psmux の器では**必ず**ここが `Some` だった。#766 で
/// 側路（`osc_sink`）を入れたので、側路に対応した統合スクリプトを持つ
/// プラットフォームでは `None` になる。**器の能力申告（`osc_passthrough`）は
/// 変えていない** — psmux が素通ししないのは事実のままで、変わったのは
/// tako が素通しに依存しなくなったこと
fn backend_block() -> Option<String> {
    let caps = crate::backend::capabilities();
    if caps.osc_passthrough || imp::SIDE_CHANNEL {
        return None;
    }
    Some(format!(
        "永続バックエンド（{}）がシェルの出す OSC を外側へ通さないため、\
         この器を使っている間はペインの cwd 追従とコマンド実行状態が働かない",
        caps.label
    ))
}

/// 1 ファイルに対して行った（行わなかった）こと
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// 新しく書いた
    Installed,
    /// 既存のブロックを現行内容へ差し替えた
    Updated,
    /// 既に最新だったので触っていない
    Unchanged,
    /// ブロックを取り除いた
    Removed,
    /// もともと入っていなかった
    Absent,
    /// 解除の結果ファイルが空になったので削除した
    Deleted,
}

impl ChangeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::Updated => "updated",
            Self::Unchanged => "unchanged",
            Self::Removed => "removed",
            Self::Absent => "absent",
            Self::Deleted => "deleted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub label: String,
    pub path: PathBuf,
    pub kind: ChangeKind,
}

impl Change {
    pub fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "label": self.label,
            "path": self.path.display().to_string(),
            "kind": self.kind.as_str(),
        })
    }
}

/// 現在の配置状態を調べる
pub fn status() -> Status {
    imp::status()
}

/// 統合を配置する（冪等。2 回実行してもブロックは 1 個）
pub fn install() -> Result<Vec<Change>, String> {
    imp::install()
}

/// 統合を解除する。**加えた区切りごと取り除き、元のバイト列へ戻す**
pub fn uninstall() -> Result<Vec<Change>, String> {
    imp::uninstall()
}

// --- ここから下はプラットフォーム非依存の純粋関数（macOS 上でも全部テストできる） ---

/// `$PROFILE` へ書くブロック本文（末尾は改行 1 個）。
///
/// **本文は ASCII だけで書く**。書き込み先はユーザーのファイルで、符号は UTF-8 とは
/// 限らない（BOM 無しの `.ps1` は Windows PowerShell 5.1 では ANSI = 日本語環境なら
/// CP932）。日本語のコメントを混ぜると、そのファイルを開いたときだけ化ける
/// 「符号が混ざったファイル」を作ってしまう。日本語の説明は `tako setup` の出力と
/// ドキュメントが担当する（そちらは端末の符号が分かっているので安全に出せる）
#[cfg_attr(not(windows), allow(dead_code))]
fn profile_block(script: &Path) -> String {
    let literal = powershell_ascii_literal(&script.display().to_string());
    format!(
        "{BLOCK_BEGIN}\n\
         # Managed by `tako shell-integration`. Remove with `tako shell-integration uninstall`.\n\
         # Enables pane cwd tracking and command state (OSC 7 / 133) inside tako panes.\n\
         if ($env:TAKO_PANE_ID) {{\n\
         \x20   $__takoShellIntegrationScript = {literal}\n\
         \x20   if (Test-Path -LiteralPath $__takoShellIntegrationScript) {{ . $__takoShellIntegrationScript }}\n\
         }}\n\
         {BLOCK_END}\n"
    )
}

/// 管理ブロックのマーカー。読み書きの規則（区切り改行 1 個・元バイト列への完全復帰）は
/// [`crate::text_block`] が唯一の実装で、ここはその規則を使う側
const MARKERS: crate::text_block::BlockMarkers =
    crate::text_block::BlockMarkers::new(BLOCK_BEGIN, BLOCK_END);

/// ブロックの位置（開始バイト, 終了バイト = 末尾改行を含む）
#[cfg_attr(not(windows), allow(dead_code))]
fn find_block(text: &[u8]) -> Option<(usize, usize)> {
    MARKERS.find(text)
}

/// ブロックを配置した結果のファイル内容（あれば置換、無ければ追記）
#[cfg_attr(not(windows), allow(dead_code))]
fn apply_block(original: &[u8], block: &str) -> Vec<u8> {
    MARKERS.apply(original, block)
}

/// ブロックを取り除いた結果のファイル内容。[`apply_block`] が足した改行も戻す
#[cfg_attr(not(windows), allow(dead_code))]
fn remove_block(current: &[u8]) -> Vec<u8> {
    MARKERS.remove(current)
}

/// 文字列を **ASCII だけで書かれた** PowerShell の文字列式にする。
///
/// 5.1 は BOM 無しの `.ps1` を ANSI コードページとして読むため、既存プロファイルへ
/// 非 ASCII を追記すると化ける（実測: 検証機の `$PROFILE` は
/// `…\OneDrive\ドキュメント\PowerShell\` にある）。非 ASCII は `[char]0xNNNN` へ逃がす
/// **`shell_profile`（#868 の PATH 通し）と共有する**。同じ escape を 2 つ持つと
/// 必ず片方が腐るので、実装はここ 1 本に保つ
pub(crate) fn powershell_ascii_literal(value: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut literal = String::new();
    for unit in value.encode_utf16() {
        // 制御文字も literal に入れない（行が壊れる）
        if (0x20..0x7f).contains(&unit) {
            let ch = char::from(unit as u8);
            if ch == '\'' {
                literal.push_str("''");
            } else {
                literal.push(ch);
            }
        } else {
            if !literal.is_empty() {
                parts.push(format!("'{literal}'"));
                literal.clear();
            }
            parts.push(format!("[char]0x{unit:04X}"));
        }
    }
    if !literal.is_empty() || parts.is_empty() {
        parts.push(format!("'{literal}'"));
    }
    parts.join(" + ")
}

#[cfg(unix)]
mod imp {
    use std::path::Path;

    use super::{Change, Delivery, Status};

    pub(super) const SHELLS: &str = "zsh / bash / fish";
    /// 側路（#766）に対応した統合スクリプトを持っているか。
    /// unix の器（tmux）は DCS パススルーで OSC を通すので側路が要らず、
    /// zsh / bash / fish の正本にも書き先の扱いを入れていない。
    /// **通さない器を unix で採ることになったら、正本へ足してからここを true にする**
    pub(super) const SIDE_CHANNEL: bool = false;

    /// unix はシェルが拾う環境変数を撒くだけで届く（ユーザーのファイルを触らない）
    pub(super) fn injected_env(root: &Path, bash_path: &Path) -> Vec<(String, String)> {
        let mut env = Vec::new();
        let zsh_dir = root.join("zsh");
        // zsh: ZDOTDIR を統合ディレクトリへ向け、元の値は .zshenv が復元する
        if let Some(orig) = std::env::var_os("ZDOTDIR") {
            env.push((
                "TAKO_ORIG_ZDOTDIR".into(),
                orig.to_string_lossy().into_owned(),
            ));
        }
        env.push(("ZDOTDIR".into(), zsh_dir.display().to_string()));
        // bash: 最初のプロンプトで統合スクリプトを source させる（スクリプト側で置換）
        env.push((
            "PROMPT_COMMAND".into(),
            format!("source '{}'", bash_path.display()),
        ));
        // fish: vendor_conf.d の自動読み込みに乗せる
        let fish_data = root.join("fish-data").display().to_string();
        let xdg = match std::env::var("XDG_DATA_DIRS") {
            Ok(dirs) if !dirs.is_empty() => format!("{fish_data}:{dirs}"),
            // fish の既定検索パスを保つ（XDG_DATA_DIRS を上書きすると既定が消えるため明示）
            _ => format!("{fish_data}:/usr/local/share:/usr/share"),
        };
        env.push(("XDG_DATA_DIRS".into(), xdg));
        env
    }

    pub(super) fn status() -> Status {
        Status {
            delivery: Delivery::Automatic,
            script: super::integration_root().map(|r| r.join("tako.bash")),
            targets: Vec::new(),
            blocked_by_backend: super::backend_block(),
        }
    }

    /// 配置するものが無いので、スクリプトを最新にするだけ（冪等）
    pub(super) fn install() -> Result<Vec<Change>, String> {
        super::write_scripts().map_err(|e| format!("統合スクリプトを書き出せません: {e}"))?;
        Ok(Vec::new())
    }

    pub(super) fn uninstall() -> Result<Vec<Change>, String> {
        Err(
            "この環境のシェル統合は環境変数の注入だけで完結するため、解除する配置がありません"
                .to_string(),
        )
    }
}

#[cfg(windows)]
mod imp {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::{Change, ChangeKind, Delivery, ProfileTarget, Status};

    pub(super) const SHELLS: &str = "PowerShell 7 / Windows PowerShell 5.1";
    /// `tako.ps1` は `TAKO_OSC_SINK` があれば OSC を側路へ書く（#766）
    pub(super) const SIDE_CHANNEL: bool = true;

    /// Windows は `$PROFILE` 経由なので、spawn 時に注入する環境変数は無い。
    /// POSIX 用の 3 点セットを撒いても PowerShell は 1 つも見ないため、ペインの
    /// 環境を汚さないよう出さない
    pub(super) fn injected_env(_root: &Path, _bash_path: &Path) -> Vec<(String, String)> {
        Vec::new()
    }

    pub(super) fn status() -> Status {
        let script = super::integration_root().map(|r| r.join("tako.ps1"));
        let block = script.as_deref().map(super::profile_block);
        let targets = editions()
            .into_iter()
            .filter_map(|(label, exe)| {
                let path = profile_path_cached(&exe)?;
                let content = std::fs::read(&path).unwrap_or_default();
                let found = super::find_block(&content);
                let up_to_date = match (&found, &block) {
                    (Some((b, e)), Some(block)) => &content[*b..*e] == block.as_bytes(),
                    _ => false,
                };
                Some(ProfileTarget {
                    label,
                    exe,
                    path,
                    installed: found.is_some(),
                    up_to_date,
                })
            })
            .collect();
        Status {
            delivery: Delivery::Profile,
            script,
            targets,
            blocked_by_backend: super::backend_block(),
        }
    }

    pub(super) fn install() -> Result<Vec<Change>, String> {
        let root = super::integration_root()
            .ok_or_else(|| "データディレクトリを解決できません".to_string())?;
        let script = super::write_powershell_script(&root)
            .map_err(|e| format!("統合スクリプトを書き出せません: {e}"))?;
        let block = super::profile_block(&script);

        let targets = status().targets;
        if targets.is_empty() {
            return Err("PowerShell が見つからないため配置先がありません".to_string());
        }
        let mut changes = Vec::new();
        for target in targets {
            if target.installed && target.up_to_date {
                changes.push(Change {
                    label: target.label,
                    path: target.path,
                    kind: ChangeKind::Unchanged,
                });
                continue;
            }
            let kind = if target.installed {
                ChangeKind::Updated
            } else {
                ChangeKind::Installed
            };
            write_profile(&target.path, &block)?;
            changes.push(Change {
                label: target.label,
                path: target.path,
                kind,
            });
        }
        Ok(changes)
    }

    pub(super) fn uninstall() -> Result<Vec<Change>, String> {
        let mut changes = Vec::new();
        for target in status().targets {
            if !target.installed {
                changes.push(Change {
                    label: target.label,
                    path: target.path,
                    kind: ChangeKind::Absent,
                });
                continue;
            }
            let current = std::fs::read(&target.path)
                .map_err(|e| format!("{} を読めません: {e}", target.path.display()))?;
            // BOM もユーザーの記述もバイトのまま持ち回る（符号を勝手に変えない）
            let next = super::remove_block(&current);
            let kind = if split_bom(&next).1.iter().all(u8::is_ascii_whitespace) {
                // 配置のために作ったファイルを残さない
                std::fs::remove_file(&target.path)
                    .map_err(|e| format!("{} を削除できません: {e}", target.path.display()))?;
                ChangeKind::Deleted
            } else {
                std::fs::write(&target.path, &next)
                    .map_err(|e| format!("{} を書き換えられません: {e}", target.path.display()))?;
                ChangeKind::Removed
            };
            changes.push(Change {
                label: target.label,
                path: target.path,
                kind,
            });
        }
        if changes.is_empty() {
            return Err("PowerShell が見つからないため配置先がありません".to_string());
        }
        Ok(changes)
    }

    /// プロファイルへブロックを反映する。
    ///
    /// 新規作成は **BOM 付き UTF-8**（5.1 が BOM 無しを ANSI として読むため）。
    /// 既存ファイルは **エンコーディングを変えない**（ブロック本文は ASCII のみなので、
    /// UTF-8 でも ANSI でも同じバイト列になる）
    fn write_profile(path: &Path, block: &str) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("{} を作成できません: {e}", dir.display()))?;
        }
        let raw = match std::fs::read(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(format!("{} を読めません: {e}", path.display())),
        };
        // 中身が無い（新規 / 空）ときだけ BOM を付ける。既存ファイルには足さない
        // （ANSI のファイルへ BOM を付けると別の符号として読まれる）
        let out = if split_bom(&raw).1.is_empty() {
            let mut out = vec![0xEF, 0xBB, 0xBF];
            out.extend_from_slice(block.as_bytes());
            out
        } else {
            super::apply_block(&raw, block)
        };
        std::fs::write(path, out)
            .map_err(|e| format!("{} を書き換えられません: {e}", path.display()))
    }

    fn split_bom(raw: &[u8]) -> (&[u8], &[u8]) {
        if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
            raw.split_at(3)
        } else {
            (&[], raw)
        }
    }

    /// この環境に居る PowerShell のエディション。**両方に配置する**
    /// （片方だけだと、ペインで `powershell` と打った瞬間に統合が消える）
    fn editions() -> Vec<(String, String)> {
        let mut found = Vec::new();
        if let Some(pwsh) = crate::platform::exe::find("pwsh") {
            found.push(("PowerShell 7".to_string(), pwsh));
        }
        if let Some(root) = std::env::var_os("SystemRoot").and_then(|v| v.into_string().ok()) {
            let ps = format!("{root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
            if Path::new(&ps).exists() {
                found.push(("Windows PowerShell 5.1".to_string(), ps));
            }
        }
        found
    }

    /// 解決した `$PROFILE` のプロセス内キャッシュ。
    /// `status()` は setup の 1 回の実行中に何度も呼ばれるが、1 回あたり PowerShell の
    /// 起動（実測 200〜400ms）なので、素直に呼ぶと体感できるだけ遅くなる
    fn profile_path_cached(exe: &str) -> Option<PathBuf> {
        static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Option<PathBuf>>>> =
            std::sync::OnceLock::new();
        let cache = CACHE.get_or_init(Default::default);
        if let Some(hit) = cache.lock().ok().and_then(|c| c.get(exe).cloned()) {
            return hit;
        }
        let resolved = profile_path(exe);
        if let Ok(mut c) = cache.lock() {
            c.insert(exe.to_string(), resolved.clone());
        }
        resolved
    }

    /// `$PROFILE.CurrentUserAllHosts` を **PowerShell 自身に尋ねる**。
    ///
    /// `%USERPROFILE%\Documents` の決め打ちは OneDrive のフォルダーリダイレクトで外れる
    /// （実測: 検証機は `…\OneDrive\ドキュメント\PowerShell\profile.ps1`）。
    /// 受け取りは **UTF-8 バイトの 16 進**にする — 5.1 のリダイレクト出力は OEM
    /// コードページなので、生の文字列で受けると日本語のパスが壊れる
    fn profile_path(exe: &str) -> Option<PathBuf> {
        const SCRIPT: &str = "-join ([System.Text.Encoding]::UTF8.GetBytes($PROFILE.CurrentUserAllHosts) | ForEach-Object { '{0:x2}' -f $_ })";
        let mut cmd = Command::new(exe);
        cmd.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            SCRIPT,
        ]);
        crate::platform::process::no_console_window(&mut cmd);
        let out = cmd.output().ok()?;
        if !out.status.success() {
            return fallback_profile_path(exe);
        }
        let hex: String = String::from_utf8_lossy(&out.stdout)
            .chars()
            .filter(|c| c.is_ascii_hexdigit())
            .collect();
        decode_hex(&hex)
            .and_then(|b| String::from_utf8(b).ok())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| fallback_profile_path(exe))
    }

    /// PowerShell に尋ねられなかったときの保険。リダイレクトされていない既定の場所
    fn fallback_profile_path(exe: &str) -> Option<PathBuf> {
        let home = std::env::var_os("USERPROFILE").filter(|v| !v.is_empty())?;
        let dir = if exe.to_ascii_lowercase().contains("windowspowershell") {
            "WindowsPowerShell"
        } else {
            "PowerShell"
        };
        Some(
            PathBuf::from(home)
                .join("Documents")
                .join(dir)
                .join("profile.ps1"),
        )
    }

    fn decode_hex(hex: &str) -> Option<Vec<u8>> {
        if !hex.len().is_multiple_of(2) {
            return None;
        }
        let bytes = hex.as_bytes();
        let mut out = Vec::with_capacity(hex.len() / 2);
        for pair in bytes.chunks(2) {
            let s = std::str::from_utf8(pair).ok()?;
            out.push(u8::from_str_radix(s, 16).ok()?);
        }
        Some(out)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn 統合envはシェル3種ぶんのキーを含む() {
        let env = write_scripts().expect("書き出しに成功する");
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"ZDOTDIR"));
        assert!(keys.contains(&"PROMPT_COMMAND"));
        assert!(keys.contains(&"XDG_DATA_DIRS"));
        // 書き出されたファイルが実在する
        let zdotdir = env
            .iter()
            .find(|(k, _)| k == "ZDOTDIR")
            .map(|(_, v)| PathBuf::from(v))
            .unwrap();
        assert!(zdotdir.join(".zshenv").is_file());
        // 入力予測の同梱物も一緒に置かれる（Issue #600）
        let plugin_dir = zdotdir
            .parent()
            .expect("統合ディレクトリ")
            .join("zsh-autosuggestions");
        assert!(plugin_dir.join("zsh-autosuggestions.zsh").is_file());
        assert!(plugin_dir.join("LICENSE").is_file());
    }

    /// #600: 同梱物が本物であること。取り違え・空ファイル化を検出する
    #[test]
    fn 同梱したzsh_autosuggestionsが本物でバージョン表記と一致する() {
        assert!(
            ZSH_AUTOSUGGESTIONS.contains("_zsh_autosuggest_start"),
            "同梱物が zsh-autosuggestions 本体ではない"
        );
        // 上流はファイル冒頭にバージョンを書いている（`# v0.7.1`）
        assert!(
            ZSH_AUTOSUGGESTIONS.contains(&format!("# {AUTOSUGGEST_VERSION}")),
            "AUTOSUGGEST_VERSION({AUTOSUGGEST_VERSION}) が同梱物のバージョンと食い違っている"
        );
        assert!(
            ZSH_AUTOSUGGESTIONS_LICENSE.contains("Permission is hereby granted"),
            "MIT ライセンス全文が同梱されていない"
        );
    }

    /// #600: zshenv 側の不変条件。壊すと「tako の外に漏れる」「二重注入する」
    /// といった事故になるので、構造をテストで固定する
    #[test]
    fn zshenvの入力予測ブロックが不変条件を満たす() {
        // tako のペインの中でしか読み込まない（要件 2: 外の zsh に影響ゼロ）
        assert!(ZSH_ZSHENV.contains("-o interactive && -n ${TAKO_PANE_ID-}"));
        // 二重注入ガード: ユーザーが先に入れていたら手を出さない（要件 3）
        assert!(ZSH_ZSHENV.contains("_zsh_autosuggest_start"));
        assert!(ZSH_ZSHENV.contains("_tako_as_owner=user"));
        // 読み込みは precmd（= .zshrc の後）まで遅らせる。ここが .zshenv 直下に
        // 戻ると 1) 二重注入ガードが効かず 2) 他プラグインを包めなくなる
        assert!(ZSH_ZSHENV.contains("precmd_functions+=(_tako_autosuggest_sync)"));
        // 状態ファイルを毎プロンプト見る（既存ペインにも反映される）
        assert!(ZSH_ZSHENV.contains("_tako_as_state"));
        // 明示的な無効化の逃げ道
        assert!(ZSH_ZSHENV.contains("TAKO_NO_AUTOSUGGESTIONS"));
    }

    /// #614: ヒント + Tab 確定の不変条件。壊すと「確定したテキストにヒントが混入する」
    /// 「Tab の従来動作を奪う」という実害になるので、構造をテストで固定する
    #[test]
    fn zshenvの確定ヒントとtab確定が不変条件を満たす() {
        // ヒントは POSTDISPLAY へ足すので、プラグインがそれを読む前後で必ず外す /
        // 付け直す。関門は highlight_reset（入口）と highlight_apply（出口）の 2 つだけ
        assert!(ZSH_ZSHENV.contains("_zsh_autosuggest_highlight_reset() {"));
        assert!(ZSH_ZSHENV.contains("_zsh_autosuggest_highlight_apply() {"));
        assert!(ZSH_ZSHENV.contains("_tako_as_hint_strip"));
        assert!(ZSH_ZSHENV.contains("_tako_as_hint_apply"));
        // 包み込みは冪等（プラグインを読み直されても二重ラップしない）
        assert!(
            ZSH_ZSHENV.contains("!= *_tako_as_hint_strip*")
                && ZSH_ZSHENV.contains("!= *_tako_as_hint_apply*"),
            "ラップ済み判定が無いと再 source で無限再帰する"
        );
        // 出す / 出さないは**その行の最初の 1 回で決める**。残り回数を毎回見ると、
        // 消費した瞬間（残り 0）に同じ行の再描画から案内が消える（#614 で実際に踏んだ）
        assert!(ZSH_ZSHENV.contains("_tako_as_hint_line=show"));
        assert!(ZSH_ZSHENV.contains("_tako_as_hint_line=hide"));
        assert!(ZSH_ZSHENV.contains("[[ $_tako_as_hint_line == show ]] || return 0"));
        // Tab はプラグインに包ませない（包まれると POSTDISPLAY が空で渡る）
        assert!(ZSH_ZSHENV.contains("ZSH_AUTOSUGGEST_IGNORE_WIDGETS+=(tako-autosuggest-tab)"));
        // ゴーストが無いときは**元のバインドへ委譲**する（補完の非回帰）
        assert!(ZSH_ZSHENV.contains("_tako_as_tab_orig"));
        assert!(ZSH_ZSHENV.contains(r#"builtin bindkey '^I'"#));
        // 確定はカーソルが行末にあるときだけ（プラグイン本体の accept と同じ条件）
        assert!(ZSH_ZSHENV.contains("CURSOR == $#BUFFER"));
        // 明示的な無効化の逃げ道
        assert!(ZSH_ZSHENV.contains("TAKO_NO_AUTOSUGGEST_TAB"));
        assert!(ZSH_ZSHENV.contains("TAKO_NO_AUTOSUGGEST_HINT"));
        // 既定回数は Rust 側の定数と揃える（片方だけ変えると挙動が食い違う）
        assert!(
            ZSH_ZSHENV.contains(&format!("_tako_as_hint_left={AUTOSUGGEST_HINT_DEFAULT}")),
            "zsh 側の既定回数が AUTOSUGGEST_HINT_DEFAULT({AUTOSUGGEST_HINT_DEFAULT}) と食い違っている"
        );
    }

    /// #614: 状態ファイルの往復。**不在は既定回数 / Tab 確定 ON**（既定 ON）
    #[test]
    fn 確定ヒントとtab確定の状態ファイルは往復し不在は既定() {
        let root = std::env::temp_dir().join(format!("tako-hint-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        // 不在 = 既定
        assert_eq!(
            autosuggest_hint_state_in(&root),
            Some(AUTOSUGGEST_HINT_DEFAULT)
        );
        assert!(autosuggest_tab_state_in(&root));

        // 残り回数（zsh 側が減らして書き戻す値）を読める
        write_autosuggest_hint_state_in(&root, Some(3)).expect("書ける");
        assert_eq!(autosuggest_hint_state_in(&root), Some(3));
        // 0 は「もう出さない」だが恒久 OFF とは区別する（hint on で戻せる）
        write_autosuggest_hint_state_in(&root, Some(0)).expect("書ける");
        assert_eq!(autosuggest_hint_state_in(&root), Some(0));
        // 恒久 OFF
        write_autosuggest_hint_state_in(&root, None).expect("書ける");
        assert_eq!(
            std::fs::read_to_string(root.join("autosuggest-hint")).unwrap(),
            "off"
        );
        assert_eq!(autosuggest_hint_state_in(&root), None);
        // 壊れた値は既定へ倒す（zsh 側と同じ倒し方）
        std::fs::write(root.join("autosuggest-hint"), "garbage").unwrap();
        assert_eq!(
            autosuggest_hint_state_in(&root),
            Some(AUTOSUGGEST_HINT_DEFAULT)
        );

        write_autosuggest_tab_state_in(&root, false).expect("書ける");
        assert!(!autosuggest_tab_state_in(&root));
        write_autosuggest_tab_state_in(&root, true).expect("書ける");
        assert!(autosuggest_tab_state_in(&root));

        // 文言は 1 行目 = Tab 確定あり、2 行目 = Tab 確定なしの 2 行
        let texts = ("[A]".to_string(), "[B]".to_string());
        write_autosuggest_hint_text_in(&root, &texts).expect("書ける");
        let raw = std::fs::read_to_string(root.join("autosuggest-hint-text")).unwrap();
        let lines: Vec<&str> = raw.lines().collect();
        assert_eq!(lines, vec!["[A]", "[B]"]);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// #614: 文言は日英とも「Tab を含む版 / 含まない版」が別物であること。
    /// Tab 確定を OFF にしたのに「Tab で確定」と案内するのは嘘になる
    #[test]
    fn 確定ヒントの文言は日英とtab有無で出し分ける() {
        // `set_lang` はプロセスグローバルで並列テストと競合する（#608）ので触らない
        let (ja_tab, ja_no_tab) = autosuggest_hint_texts_for(crate::i18n::Lang::Ja);
        let (en_tab, en_no_tab) = autosuggest_hint_texts_for(crate::i18n::Lang::En);

        for (with, without) in [(&ja_tab, &ja_no_tab), (&en_tab, &en_no_tab)] {
            assert!(
                with.contains("Tab"),
                "Tab 確定ありの案内に Tab が無い: {with}"
            );
            assert!(
                !without.contains("Tab"),
                "Tab 確定なしの案内に Tab が残っている: {without}"
            );
            // 右矢印はどちらでも確定キー
            assert!(with.contains('→') && without.contains('→'));
        }
        assert_ne!(ja_tab, en_tab, "日英で同じ文言になっている");
    }

    /// #601: CLI ディレクトリは**実行中バイナリの隣**から決める。
    /// .app / dev ビルドのどちらでも同じ規則で解け、直書きのパスは持たない
    #[test]
    fn cliディレクトリは実行中バイナリの隣から解決する() {
        let app = PathBuf::from("/Applications/tako.app/Contents/MacOS/tako-app");
        let dev = PathBuf::from("/Users/x/dev/tako/target/debug/tako-app");
        // 隣に CLI があるときだけそのディレクトリを返す
        assert_eq!(
            resolve_cli_dir(Some(&app), &|p| p
                == Path::new("/Applications/tako.app/Contents/MacOS/tako")),
            Some(PathBuf::from("/Applications/tako.app/Contents/MacOS"))
        );
        assert_eq!(
            resolve_cli_dir(Some(&dev), &|p| p
                == Path::new("/Users/x/dev/tako/target/debug/tako")),
            Some(PathBuf::from("/Users/x/dev/tako/target/debug"))
        );
        // CLI が隣に無い（dev で tako-cli 未ビルド）なら注入しない
        assert_eq!(resolve_cli_dir(Some(&dev), &|_| false), None);
        assert_eq!(resolve_cli_dir(None, &|_| true), None);
        // 改行入りのパスは状態ファイル（1 行 1 値）に載せられないので捨てる
        let weird = PathBuf::from("/tmp/a\nb/tako-app");
        assert_eq!(resolve_cli_dir(Some(&weird), &|_| true), None);
    }

    #[test]
    fn cliディレクトリの状態ファイルは往復し空は注入なし() {
        let root = std::env::temp_dir().join(format!("tako-cli-dir-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // 不在 = 注入しない
        assert_eq!(cli_dir_state_in(&root), None);

        let dir = PathBuf::from("/Applications/tako.app/Contents/MacOS");
        write_cli_dir_in(&root, Some(&dir)).expect("書ける");
        assert_eq!(cli_dir_state_in(&root), Some(dir));

        // 解決できなければ空にする（古い値を残してシェルに誤った PATH を足させない）
        write_cli_dir_in(&root, None).expect("書ける");
        assert_eq!(std::fs::read_to_string(root.join("cli-dir")).unwrap(), "");
        assert_eq!(cli_dir_state_in(&root), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// #601: シェル側の不変条件。壊すと「外の PATH を書き換える」「ユーザーが自分で
    /// 通した tako を押しのける」といった事故になるので、構造をテストで固定する
    #[test]
    fn シェル統合のpath注入が不変条件を満たす() {
        // zsh: 判定は .zshrc の後（precmd）。ここが .zshenv 直下へ戻ると、
        // .zprofile / .zshrc で PATH を組むユーザーを必ず誤判定する
        assert!(ZSH_ZSHENV.contains("precmd_functions+=(_tako_path_sync)"));
        // 非対話（コマンドペイン・エージェントペイン）はフックが回らないので直接呼ぶ
        assert!(ZSH_ZSHENV.contains("if [[ ! -o interactive && -n ${TAKO_PANE_ID-} ]]; then"));
        // 足すのは末尾のみ（zsh の path 配列 += / bash・fish は $PATH の後ろ）
        assert!(ZSH_ZSHENV.contains("path+=(\"$dir\")"));
        assert!(BASH_SCRIPT.contains("export PATH=\"$PATH:$_tako_cli_dir\""));
        assert!(FISH_SCRIPT.contains("set -gx PATH $PATH $dir"));
        for (name, script) in [
            ("zsh", ZSH_ZSHENV),
            ("bash", BASH_SCRIPT),
            ("fish", FISH_SCRIPT),
        ] {
            // 状態ファイルを読む（tako が起動時に書いた実体の場所）
            assert!(
                script.contains("cli-dir"),
                "{name}: 状態ファイルを見ていない"
            );
            // 明示的な無効化の逃げ道
            assert!(
                script.contains("TAKO_NO_PATH_INJECTION"),
                "{name}: 逃げ道が無い"
            );
        }
        // 既存の tako を尊重する二重追加ガード（シェルごとの慣用句）
        assert!(ZSH_ZSHENV.contains("(( ${+commands[tako]} )) && return 0"));
        assert!(BASH_SCRIPT.contains("! command -v tako >/dev/null 2>&1"));
        assert!(FISH_SCRIPT.contains("type -q tako; and return"));
        // fish は自分の置き場所から統合ルートを逆算する。write_scripts の配置と一致させる
        assert!(FISH_SCRIPT.contains("/fish-data/fish/vendor_conf\\.d/[^/]*$"));
    }

    #[test]
    fn 入力予測の状態ファイルは往復し不在は既定on() {
        let root = std::env::temp_dir().join(format!("tako-as-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // 不在 = ON（既定 ON。Issue #600）
        assert!(autosuggest_state_in(&root));

        write_autosuggest_state_in(&root, false).expect("書ける");
        assert_eq!(
            std::fs::read_to_string(root.join("autosuggest")).unwrap(),
            "off"
        );
        assert!(!autosuggest_state_in(&root));

        write_autosuggest_state_in(&root, true).expect("書ける");
        assert_eq!(
            std::fs::read_to_string(root.join("autosuggest")).unwrap(),
            "on"
        );
        assert!(autosuggest_state_in(&root));

        // 壊れた値は ON 側へ倒す（予測が出ないより出る方が既定に忠実）
        std::fs::write(root.join("autosuggest"), "garbage").unwrap();
        assert!(autosuggest_state_in(&root));
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// PowerShell 統合の純粋関数のテスト（#525）。
///
/// **`unix` で切らない**。ここで検証するのは「バイト列の切った貼った」と
/// 「ASCII への逃がし方」だけで、どのプラットフォームでも同じ答えになる。
/// Windows 実機を待たずに macOS の `cargo test` で回せることが重要
/// （実機でしか走らないテストは書いた本人以外が壊れに気付けない）。
/// 実際に PowerShell を起動する e2e は `tests/shell_integration_powershell.rs`
#[cfg(test)]
mod powershell_tests {
    use super::*;

    fn block() -> String {
        profile_block(Path::new("C:\\tako\\tako.ps1"))
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[test]
    fn ブロックはマーカーで囲まれスクリプトを読み込む() {
        let b = block();
        assert!(b.starts_with(BLOCK_BEGIN), "{b}");
        assert!(b.ends_with(&format!("{BLOCK_END}\n")), "{b}");
        assert!(b.contains("'C:\\tako\\tako.ps1'"), "{b}");
        // ペインの外では何もしない（プロファイルに置き続けても無害であることの担保）
        assert!(b.contains("$env:TAKO_PANE_ID"), "{b}");
    }

    /// #766: 器が OSC を素通ししないときの側路。**正本のスクリプトが**
    /// `TAKO_OSC_SINK` を見て、束をまとめて 1 ファイルへ差し替えること。
    ///
    /// ここが崩れると Windows で状態ドットと cwd 追従が黙って死ぬ（器が既定なので
    /// **セッション完全復元を使っている人ほど効かない**）。スクリプトは実機テスト
    /// （`tests/shell_integration_powershell.rs`）が動作まで見るので、ここは
    /// 「経路が消えていないこと」の番犬
    #[test]
    fn ps1が側路の書き先を見て束ごと差し替える() {
        let s = POWERSHELL_SCRIPT;
        assert!(
            s.contains(crate::osc_sink::SINK_ENV),
            "統合スクリプトが側路の環境変数を見ていない"
        );
        // 束（133;D + 133;A + OSC 7）を 1 回で書く: バッファへ積んで flush する形
        assert!(s.contains("__takoSinkBuf"), "側路のバッファが無い");
        assert!(
            s.matches("__takoSinkFlush").count() >= 3,
            "flush の呼び出しが足りない（定義 + コマンド開始 + プロンプト）"
        );
        // 差し替えは .new へ書いて rename（読み手が半端な束を見ないため）
        assert!(
            s.contains("'.new'"),
            "一時ファイル経由の差し替えになっていない"
        );
        assert!(s.contains("Move-Item"), "rename による差し替えが無い");
        // BOM を付けない（先頭の ESC の前にバイトが入るとスキャンが崩れる）
        assert!(s.contains("UTF8Encoding($false)"), "BOM なし指定が無い");
        // 側路のときは DCS で包まない（器を通らないので二重化は無意味）
        assert!(
            s.contains("$global:__takoSinkBuf += $seq"),
            "側路へは素のバイト列を積むこと"
        );
    }

    #[test]
    fn 空プロファイルへの配置は区切りを足さない() {
        assert_eq!(apply_block(b"", &block()), block().into_bytes());
    }

    #[test]
    fn 二回配置してもブロックは一個() {
        let once = apply_block(b"Set-Alias ll Get-ChildItem\n", &block());
        let twice = apply_block(&once, &block());
        assert_eq!(once, twice);
        assert_eq!(
            text(&twice).matches(BLOCK_BEGIN).count(),
            1,
            "{}",
            text(&twice)
        );
    }

    #[test]
    fn 内容が変わったブロックは置換されユーザーの記述は残る() {
        let user = "Set-Alias ll Get-ChildItem\n";
        let old = apply_block(
            user.as_bytes(),
            &profile_block(Path::new("C:\\old\\tako.ps1")),
        );
        let new = text(&apply_block(&old, &block()));
        assert_eq!(new.matches(BLOCK_BEGIN).count(), 1, "{new}");
        assert!(new.contains("C:\\tako\\tako.ps1"), "{new}");
        assert!(!new.contains("C:\\old\\tako.ps1"), "{new}");
        assert!(new.starts_with(user), "{new}");
    }

    /// **受け入れ条件そのもの**: 解除で元のバイト列へ完全に戻る。
    /// 末尾の改行の有無・空ファイル・ブロックの後ろにユーザーが書き足した場合まで見る
    #[test]
    fn 解除は元のバイト列へ完全に戻す() {
        for original in [
            &b""[..],
            b"Set-Alias ll Get-ChildItem\n",
            b"Set-Alias ll Get-ChildItem",          // 改行で終わらない
            b"a\n\n\nb\n",                          // 連続改行
            b"function prompt { 'x> ' }\r\nls\r\n", // CRLF
            // BOM 無しの CP932（`# 日本語コメント` の ANSI 表現）。UTF-8 として妥当でない
            // バイト列でも壊さずに戻せること = ユーザーのプロファイルを破壊しない保証
            b"# \x93\xfa\x96\x7b\x8c\xea\r\nSet-Alias ll Get-ChildItem\r\n",
        ] {
            let installed = apply_block(original, &block());
            assert_eq!(
                remove_block(&installed),
                original,
                "元へ戻らない: {original:?}"
            );
        }
    }

    #[test]
    fn ブロックの後ろに書き足されていても解除できる() {
        let installed = apply_block(b"head\n", &block());
        let mut edited = installed;
        edited.extend_from_slice(b"tail\n");
        assert_eq!(remove_block(&edited), b"head\ntail\n");
    }

    #[test]
    fn 未配置のファイルは解除で変化しない() {
        let text = b"Set-Alias ll Get-ChildItem\n";
        assert_eq!(remove_block(text), text);
    }

    #[test]
    fn asciiのパスはそのまま読める形になる() {
        assert_eq!(
            powershell_ascii_literal("C:\\tako\\tako.ps1"),
            "'C:\\tako\\tako.ps1'"
        );
        // 単引用符は PowerShell 流に二重化
        assert_eq!(powershell_ascii_literal("it's"), "'it''s'");
        assert_eq!(powershell_ascii_literal(""), "''");
    }

    #[test]
    fn 非asciiのパスは_charエスケープへ逃がす() {
        // 実機の $PROFILE が OneDrive の「ドキュメント」配下にある構成を想定
        let got = powershell_ascii_literal("C:\\日\\a.ps1");
        assert!(got.is_ascii(), "非 ASCII が残っている: {got}");
        assert_eq!(got, "'C:\\' + [char]0x65E5 + '\\a.ps1'");
    }

    /// **ブロック全体が ASCII**であること。ユーザーのプロファイルは ANSI（CP932）のことが
    /// あり、そこへ UTF-8 の日本語を混ぜると符号が混在したファイルになる
    #[test]
    fn ブロック全体がasciiになる() {
        for path in ["C:\\tako\\tako.ps1", "C:\\ドキュメント\\tako.ps1"] {
            let b = profile_block(Path::new(path));
            assert!(
                b.is_ascii(),
                "ブロックに非 ASCII が混ざっている（{path}）:\n{b}"
            );
        }
    }

    #[test]
    fn サロゲートペアもエスケープされる() {
        let got = powershell_ascii_literal("a\u{1F419}b");
        assert!(got.is_ascii(), "{got}");
        // UTF-16 の 2 単位に分かれる
        assert_eq!(got, "'a' + [char]0xD83D + [char]0xDC19 + 'b'");
    }

    #[test]
    fn powershellスクリプトはasciiのみ() {
        // 5.1 は BOM 無しの .ps1 を ANSI として読む。非 ASCII を入れると
        // プロファイルへ並べたときに化けるので、同梱スクリプト自体を ASCII に保つ
        assert!(
            POWERSHELL_SCRIPT.is_ascii(),
            "tako.ps1 に非 ASCII が混ざっている"
        );
    }

    /// **#970 の番犬**（macOS でも走る）: OSC 7 を組む前に verbatim prefix を落として
    /// いること。落とさないと `\` → `/` の置換で `//?/C:/…` になり、ペインの cwd が
    /// `///?/C:/…`（実在しないパス）へ壊れて git 操作が全滅する。
    ///
    /// 実機（Windows）の回帰は
    /// `tests/shell_integration_powershell.rs::verbatimな場所へ移ってもcwdが壊れない`
    /// が実シェルで測る。こちらは**置換より前に通っているか**という順序を固定する
    /// （関数はあるのに呼んでいない、を検出するため）
    #[test]
    fn osc7を組む前にverbatimを剥がしている() {
        let cwd_fn = POWERSHELL_SCRIPT
            .split("function global:__takoCwdSequence")
            .nth(1)
            .expect("__takoCwdSequence が無い");
        let body = cwd_fn.split("\n    }").next().unwrap_or(cwd_fn);
        let strip_at = body
            .find("__takoStripVerbatim")
            .expect("__takoCwdSequence が __takoStripVerbatim を呼んでいない（#970）");
        let replace_at = body
            .find(".Replace(")
            .expect("__takoCwdSequence が区切りを置換していない");
        assert!(
            strip_at < replace_at,
            "verbatim を剥がすのは `\\` → `/` の置換より前でなければならない（#970）:\n{body}"
        );
        // 剥がす側の実装（prefix 2 種）も残っていること
        assert!(
            POWERSHELL_SCRIPT.contains(r"'\\?\UNC\'") && POWERSHELL_SCRIPT.contains(r"'\\?\'"),
            "__takoStripVerbatim が verbatim / verbatim UNC の両方を見ていない（#970）"
        );
    }
}
