//! welcome — 初回起動のウェルカムバナー（Issue #549）
//!
//! **何のためにあるか**: zip / cask で入れた tako を初めて起動しても、画面には
//! タブ 1 枚とシェル 1 ペインしか無く、docs が謳う `tako setup` → `tako master` の
//! 入口がどこにも示されていなかった。「ゼロコンフィグで一般ユーザーが使える」という
//! 設計原則に対し、実態は「docs を読んだ人だけがゼロコンフィグ」だった（#549）。
//!
//! ## 初回起動の判定
//!
//! 「settings.json がまだ無い」を初回起動とみなす。バージョンアップした既存ユーザーは
//! ファイルを持っているので出ない（新フィールドの serde default で判定すると、
//! 既存ユーザー全員に出てしまう）。壊れた settings.json は `load()` が既定値へ
//! フォールバックするが、**ファイル自体は存在する**ので初回とは扱わない
//! （設定を失った既存ユーザーへ的外れな案内を出さないため）。
//!
//! 閉じたら `welcome_dismissed` を書き込む。以後はファイルの存在と併せて二重に
//! 「出さない」が成立する。
//!
//! ## 初回起動の導線（#1510）
//!
//! #549 のバナーは `tako setup` → `tako master` を並べるだけで、どのボタンを押しても
//! 以後は出なかった。setup は claude を入れた後「次の 1 手: claude auth login」で止まり、
//! ログイン → setup のやり直し → `tako master` を人が自分で打つ必要があった
//! （10/27 の Windows 版の報告「何をすればいいか分からない」）。いまは次の形:
//!
//! - 初回起動で、**副作用の小さい setup の段**（PATH・MCP の登録・指示ファイル・プロファイル）を
//!   自動で走らせる。ネットワークから取って入れる段（エージェント CLI の公式インストーラ・
//!   brew / winget）は**走らせない**（[`auto_setup_answers`] の `install: false`）
//! - エージェント CLI の導入とログインは、バナーのボタン 1 つの同意で行う。ボタンは
//!   ペインを開いてコマンドを打つ = 利用者が自分で打つのと同じ経路（#1129 の
//!   「見えない子プロセスでログインを待たない」を守る）
//! - 残り作業があるあいだは「次にやること」を出し続け（`welcome_onboarding`）、
//!   終わったら「master を起動」を出す。master を起動するか「今後表示しない」で終わる
//!
//! 状態と操作は dispatch `Welcome`（CLI `tako welcome` / MCP `tako_welcome`）の 1 実装で、
//! GUI のボタンも同じ要求を投げる（UI に閉じたロジックを作らない）
//!
//! ## ボタン → setup → 対話（#2879。いまの既定 = [`Flow::Guided`]）
//!
//! 11/10 の利用者の指示で、#1510 の自動の段をやめた。master / worker の設定や GitHub との連携
//! （設定共有）は `tako setup` の最後に開く**対話アシスタント**でしか決められないのに、自動の段は
//! `launch_agent: none` で対話を通らず「master を起動」へ進んでいた。いまは次の形:
//!
//! - 初回起動では**何も走らせない**（[`startup_action`] は確かめるだけ。ファイルも書かない）
//! - 最初の段は「セットアップを実行」（[`NextStep::Setup`]）= setup のタブで端末の `tako setup`。
//!   エージェント CLI が無ければ setup 自身が導入の同意（`[Y/n]`）を聞き、ログインはバナーの
//!   「ログイン」、済んだら「セットアップを続ける」（[`NextStep::Continue`]）でもう一度 `tako setup` →
//!   最後に対話アシスタント。対話が済んだら（`config.yaml` の `setup.dialogue_done`）「master を起動」
//! - setup・対話が走っているあいだ（[`crate::setup::running_phase`]）は [`NextStep::Running`] で、
//!   ボタンはタブを見せるだけ（走っている setup の入力へコマンド行を打たない）
//!
//! A/B は `TAKO_2879_LEGACY=1`（#1510 の自動の段の形 = [`Flow::Auto`]）

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_core::platform::agent_install::AgentKind;
use tako_core::platform::shell_dialect::ShellDialect;

use crate::settings::Settings;
use crate::setup_bootstrap::{BootstrapState, InstallPlan, Step};
use crate::SetupRefusal;

/// バナーを出すか（純関数。テストと実運用が同じ判定を通る）。
///
/// `settings_path` はファイルの実在だけを見る。`settings` は破損時に既定値へ
/// 落ちている可能性があるため、判定の主軸はファイルの存在に置く。
pub fn should_show(settings_path: &Path, settings: &Settings) -> bool {
    !settings_path.exists() && !settings.welcome_dismissed
}

/// [`should_show`] に「導線が続いているあいだは毎回出す」を足したもの（純関数）。
///
/// `flow` = 導線の形（[`flow`]）。[`Flow::Banner`] なら [`should_show`] と同じ（= #549 の初回だけ）。
/// 続いている印は形ごとに別: #1510 の形は `welcome_onboarding`、#2879 の形は `welcome_guided`。
/// **既存の利用者には出さない**: どちらの印も初回起動で導線を始めたときだけ立つので、それより前から
/// 使っている人のファイルには無い。#1510 の印が立ったままの人（自動の段の後に master を押していない人）にも、
/// #2879 の形は出さない（初回起動を済ませた人へ対話を促し直さない = 11/10 の判断）
pub fn should_show_with(settings_path: &Path, settings: &Settings, flow: Flow) -> bool {
    if settings.welcome_dismissed {
        return false;
    }
    if !settings_path.exists() {
        return true;
    }
    match flow {
        Flow::Banner => false,
        Flow::Auto => settings.welcome_onboarding,
        Flow::Guided => settings.welcome_guided,
    }
}

/// 起動時にバナーを出すか（実パス版）。
///
/// データディレクトリを解決できない環境では出さない（設定を永続化できない =
/// 「閉じても毎回出る」になるため、出さない側へ倒す）。
pub fn should_show_on_launch() -> bool {
    match crate::settings::settings_path() {
        Some(path) => should_show_with(&path, &crate::settings::load(), flow()),
        None => false,
    }
}

/// 初回起動か（settings.json がまだ無い）。status 応答の診断用
pub fn is_first_launch() -> bool {
    crate::settings::settings_path().is_some_and(|p| !p.exists())
}

/// 「以後出さない」を永続化する
pub fn mark_dismissed() -> io::Result<PathBuf> {
    crate::settings::set_carry_welcome_guided(false);
    let mut settings = crate::settings::load();
    settings.welcome_dismissed = true;
    crate::settings::save(&settings)
}

/// 初回起動（settings.json がまだ無い）の GUI で、ほかの設定の保存にも導線の印を載せる（#2879）。
/// **ここでは書かない**（起動しただけではファイルを増やさない = 何も自動で走らせない約束の一部）。
/// 理由は `settings::CARRY_WELCOME_GUIDED`
pub fn carry_guided_on_first_launch() {
    if is_first_launch() {
        crate::settings::set_carry_welcome_guided(true);
    }
}

/// 導線を始めた印を書く（#2879。最初のボタンを押したとき）。既に立っていれば書かない。
/// setup の CLI が後から settings.json を書いても、読んでから書くので印は残る
pub fn mark_guided() -> io::Result<()> {
    let mut settings = crate::settings::load();
    if settings.welcome_guided {
        return Ok(());
    }
    settings.welcome_guided = true;
    crate::settings::save(&settings).map(|_| ())
}

/// 導線を始めた印を残す（初回起動。#1510）。**settings.json はここで初めてできる**
/// （起動処理は settings を読むだけで書かないので、初回の判定はこの前に済ませておく）
pub fn begin_onboarding() -> io::Result<PathBuf> {
    let mut settings = crate::settings::load();
    settings.welcome_onboarding = true;
    crate::settings::save(&settings)
}

/// 導線を終える（master を起動した。#1510 / #2879）。以後の起動ではバナーを出さない
pub fn finish_onboarding() -> io::Result<PathBuf> {
    crate::settings::set_carry_welcome_guided(false);
    let path = crate::settings::settings_path()
        .ok_or_else(|| io::Error::other("設定の置き場が決まらない"))?;
    match finished_settings(&crate::settings::load(), path.exists()) {
        Some(settings) => crate::settings::save(&settings),
        None => Ok(path),
    }
}

/// 導線を終えた settings（**純関数**。書く必要が無ければ None）。どちらの形の印も倒す。
/// settings.json がまだ無い（初回起動のまま master を押した）なら書いて初回を終える
/// （書かないと次の起動も「初回」と読んでバナーが出る）
pub fn finished_settings(settings: &Settings, file_exists: bool) -> Option<Settings> {
    if file_exists && !settings.welcome_onboarding && !settings.welcome_guided {
        return None;
    }
    let mut done = settings.clone();
    done.welcome_onboarding = false;
    done.welcome_guided = false;
    Some(done)
}

/// 案内するコマンド（#322「最も簡単なコマンドを提案する」原則。
/// 実行時に解決する実バイナリパスではなく、ユーザーが打つ最簡形を返す）
pub const SETUP_COMMAND: &str = "tako setup";
pub const MASTER_COMMAND: &str = "tako master";

/// バナー / パレットの「その場実行」がペインへ送るコマンド行（Issue #549）。
///
/// zip 配布では CLI が `/Applications/tako.app/Contents/MacOS/tako` にしか無く、
/// アプリはペインの PATH に何も注入しないため、素の `tako setup` は
/// `command not found` になりうる（#549 が指摘した点）。解決済みの実体パスで
/// 組み立てることで、PATH の状態に関係なくボタンが必ず動く。
/// 案内文（バナーの本文）は `SETUP_COMMAND` / `MASTER_COMMAND` の最簡形のままにする
pub fn launch_command_line(subcommand: &str) -> String {
    // A/B: `TAKO_899_LEGACY=1` で #899 以前の**クォート（症状 2）だけ**を戻す。
    // 同一バイナリで実機の before/after を取るための逃げ道。
    //
    // 行末（症状 1）の legacy 経路は**用意しない**: PTY へ LF を書く形は #897 の番犬
    // （`セルフテストがptyへ書くenterはcrである`）が構造的に禁じている不変条件なので、
    // env で戻せる穴を作らない。症状 2 だけ戻せば PowerShell では
    // `'C:\…\tako.exe' --version` が式として評価されて実行されないので、
    // CR で送っても実機の before は再現できる
    if std::env::var_os("TAKO_899_LEGACY").is_some() {
        let bin = crate::dispatch::resolve_tako_binary();
        let safe = !bin.is_empty()
            && bin
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-/".contains(&b));
        let quoted = if safe {
            bin
        } else {
            format!("'{}'", bin.replace('\'', r"'\''"))
        };
        return format!("{quoted} {subcommand}");
    }
    launch_command_line_in(default_dialect(), subcommand)
}

/// 方言を明示する版（**macOS 上から PowerShell 形も検査できる**）。
///
/// #899: 旧実装は POSIX 決め打ちのクォート 1 本で、Windows の絶対パスを
/// `'C:\…\tako.exe'` と囲んでいた。PowerShell は引用符付き文字列を式として
/// 評価するので実行されずそのまま表示される。境界
/// [`tako_core::platform::shell_dialect::ShellDialect::command_word`] へ寄せた
pub fn launch_command_line_in(
    dialect: tako_core::platform::shell_dialect::ShellDialect,
    subcommand: &str,
) -> String {
    format!(
        "{} {subcommand}",
        dialect.command_word(&crate::dispatch::resolve_tako_binary())
    )
}

/// ペインの既定シェルの方言。判定できないシェル（cmd.exe / fish）は POSIX へ倒す
/// （#867 / #873 の起動コマンドと同じ方針）
pub fn default_dialect() -> tako_core::platform::shell_dialect::ShellDialect {
    tako_core::platform::shell_dialect::for_default_shell()
        .unwrap_or(tako_core::platform::shell_dialect::ShellDialect::Posix)
}

// --- 初回起動の導線（Issue #1510）-------------------------------------------

/// A/B（`TAKO_1510_LEGACY=1`）: #1510 前のバナー（2 つのボタンを並べるだけ・押すと以後出さない・
/// 何も自動で走らせない）へ戻す。同一バイナリで修正前後を見比べるための逃げ道
pub fn legacy() -> bool {
    std::env::var_os("TAKO_1510_LEGACY").is_some()
}

/// 検証プロセス（隔離 GUI・セルフテスト）で導線を動かす明示（`1` / `true` / `on`）。
///
/// 自動の段は PATH・MCP の登録・指示ファイルを **HOME へ書く**。検証用 GUI は HOME を
/// 利用者のホームのまま起こされることが多い（#1970）ので、既定では動かさない。
/// HOME を一時の置き場へ差し替えた検証だけがこれを立てる
pub const FIRST_RUN_ENV: &str = "TAKO_FIRST_RUN";

/// 導線（初回起動の自動の段・状態の見張り・「次にやること」の表示）を動かすか（#1510）。
/// 偽なら #549 のバナーのまま（[`legacy`] と同じ見た目）
pub fn onboarding_enabled() -> bool {
    onboarding_enabled_in(
        legacy(),
        tako_core::paths::is_verification_process(),
        matches!(
            std::env::var(FIRST_RUN_ENV).ok().as_deref(),
            Some("1" | "true" | "on")
        ),
    )
}

/// [`onboarding_enabled`] の判断（純関数）
pub fn onboarding_enabled_in(legacy: bool, verification: bool, opted_in: bool) -> bool {
    !legacy && (!verification || opted_in)
}

/// A/B（`TAKO_2879_LEGACY=1`）: #2879 より前 = #1510 の自動の段の形（[`Flow::Auto`]）へ戻す
pub const LEGACY_2879_ENV: &str = "TAKO_2879_LEGACY";

/// [`LEGACY_2879_ENV`] が立っているか
pub fn legacy_2879() -> bool {
    std::env::var_os(LEGACY_2879_ENV).is_some()
}

/// 初回起動の導線の形
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Flow {
    /// #549 のバナー（2 行を並べるだけ。押すと以後出さない）。`TAKO_1510_LEGACY=1` と、
    /// 検証プロセスの既定（`TAKO_FIRST_RUN` を明示しない隔離 GUI・テスト）
    Banner,
    /// #1510 の形: 初回起動で副作用の小さい setup の段を自動で走らせる（`TAKO_2879_LEGACY=1`）。
    /// `Onboarding::default()` の既定（#1510 の単体テストがこの形を縛る）
    #[default]
    Auto,
    /// #2879 の形（製品の既定）: 何も自動で走らせず、ボタン → setup → 対話 → master
    Guided,
}

impl Flow {
    /// 機械が読む名前（CLI / MCP の `onboarding.flow`）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Banner => "banner",
            Self::Auto => "auto",
            Self::Guided => "guided",
        }
    }
}

/// いまの導線の形
pub fn flow() -> Flow {
    flow_in(
        legacy(),
        legacy_2879(),
        tako_core::paths::is_verification_process(),
        matches!(
            std::env::var(FIRST_RUN_ENV).ok().as_deref(),
            Some("1" | "true" | "on")
        ),
    )
}

/// [`flow`] の判断（純関数）
pub fn flow_in(legacy_1510: bool, legacy_2879: bool, verification: bool, opted_in: bool) -> Flow {
    if !onboarding_enabled_in(legacy_1510, verification, opted_in) {
        Flow::Banner
    } else if legacy_2879 {
        Flow::Auto
    } else {
        Flow::Guided
    }
}

/// 起動時に導線が何をするか（#2879）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Startup {
    /// 「次にやること」を確かめるだけ（何も走らせない）
    Probe,
    /// #1510 の自動の段を走らせる（[`Flow::Auto`] で setup がまだ一度も最後まで走っていないとき）
    Auto,
}

/// 起動時の入口の判断（**純関数**。GUI の `start_welcome_onboarding` はこれに従うだけ）。
///
/// **[`Flow::Guided`] では何があっても [`Startup::Auto`] を返さない**（11/10 の指示: 初回起動で
/// 何も自動で走らせない）。番犬: `issue2879_no_setup_on_launch_watchdog`
pub fn startup_action(flow: Flow, setup_completed: bool) -> Startup {
    match flow {
        Flow::Auto if !setup_completed => Startup::Auto,
        _ => Startup::Probe,
    }
}

/// 初回起動の自動の段に渡す answers（#1510）。
///
/// - `install: false` = エージェント CLI の公式インストーラも brew / winget の依存導入も
///   **走らせない**（同意なしにネットワークのスクリプトを実行しない。導入はボタンの同意で行う）
/// - `launch_agent: none` = 対話アシスタントを起こさない（GUI の裏で走るので端末が無い）
///
/// 走るのは PATH（tako CLI・導入済みのエージェント CLI）・シェル統合・MCP の登録・
/// 指示ファイル（無ければ作る）・プロファイル・テンプレ・データの移行で、どれも冪等
pub fn auto_setup_answers() -> crate::setup::SetupAnswers {
    crate::setup::SetupAnswers {
        install: Some(false),
        launch_agent: Some("none".into()),
        ..Default::default()
    }
}

/// 自動の段の全文の置き場（`<data_dir>/setup/first-run.log`。走るたびに上書き）
pub fn auto_log_path() -> Option<PathBuf> {
    crate::setup::setup_dir()
        .ok()
        .map(|dir| dir.join("first-run.log"))
}

/// 自動の段 1 回の結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoOutcome {
    /// setup が最後まで走った（exit 0。残り作業があっても setup は止まらない = #1501）
    pub completed: bool,
    /// 上限（`tako_core::probe::setup_run_timeout`）で打ち切った
    pub timed_out: bool,
    /// 要約の変更件数（[`parse_auto_summary`]。読めなければ None）
    pub changes: Option<usize>,
    /// 止まった理由の 1 行（完走なら None）
    pub error: Option<String>,
    /// 全文の置き場（書けなければ None）
    pub log: Option<PathBuf>,
}

impl AutoOutcome {
    pub fn to_json(&self) -> Value {
        json!({
            "completed": self.completed,
            "timed_out": self.timed_out,
            "changes": self.changes,
            "error": self.error,
            "log": self.log.as_ref().map(|p| p.display().to_string()),
        })
    }
}

/// 自動の段が起こす tako CLI。**この GUI と同じ版**（実行ファイルの隣）を先に見る。
///
/// `install` は #1510 で足した answers なので、古い CLI（版の違う `/Applications/tako.app` や
/// PATH の先の別の版）は `deny_unknown_fields` で断る = 自動の段が毎回失敗する（**ネットワークへは
/// 出ない**ので安全側だが、導線が進まない）。隣に無ければ従来の解決（[`crate::dispatch::resolve_tako_binary`]）
pub fn auto_setup_binary() -> String {
    auto_setup_binary_with(
        std::env::current_exe().ok().as_deref(),
        &crate::dispatch::tako_cli_file_name(),
        &|p| p.is_file(),
    )
    .unwrap_or_else(crate::dispatch::resolve_tako_binary)
}

/// [`auto_setup_binary`] の判断（純関数）
fn auto_setup_binary_with(
    current_exe: Option<&Path>,
    file_name: &str,
    is_file: &dyn Fn(&Path) -> bool,
) -> Option<String> {
    let sibling = current_exe?.parent()?.join(file_name);
    is_file(&sibling).then(|| sibling.display().to_string())
}

/// 自動の段を 1 回走らせる（**待つ**。GUI は background で呼ぶ）。
///
/// 起こすのは MCP `tako_setup` と同じ `tako setup --yes --answers -`（[`crate::dispatch`] の
/// 1 実装・上限つき）で、answers が [`auto_setup_answers`]。失敗しても止めない（結果を返すだけ）
pub fn run_auto_setup() -> AutoOutcome {
    let answers = serde_json::to_string(&auto_setup_answers())
        .unwrap_or_else(|_| r#"{"install":false,"launch_agent":"none"}"#.to_string());
    let reply = crate::dispatch::run_setup_cli(&auto_setup_binary(), &answers);
    let (output, completed, timed_out, error) = match reply {
        Ok(value) => {
            let timed_out = value["timed_out"].as_bool() == Some(true);
            (
                value["output"].as_str().unwrap_or_default().to_string(),
                value["completed"].as_bool() == Some(true),
                timed_out,
                timed_out.then(|| {
                    format!(
                        "上限（{} 秒）で打ち切りました",
                        value["waited_secs"].as_u64().unwrap_or_default()
                    )
                }),
            )
        }
        Err(e) => {
            let text = e.text_in(tako_core::i18n::Lang::Ja);
            let first = text.lines().next().unwrap_or_default().to_string();
            (text, false, false, Some(first))
        }
    };
    let log = auto_log_path().and_then(|path| {
        let dir = path.parent()?;
        std::fs::create_dir_all(dir).ok()?;
        std::fs::write(&path, &output).ok()?;
        Some(path)
    });
    AutoOutcome {
        completed,
        timed_out,
        changes: parse_auto_summary(&output),
        error,
        log,
    }
}

/// setup の出力から要約の変更件数を読む（純関数）。見出しの正本は
/// [`crate::setup::SUMMARY_UNCHANGED`] / [`crate::setup::SUMMARY_CHANGED_HEADER`]
pub fn parse_auto_summary(output: &str) -> Option<usize> {
    if output
        .lines()
        .any(|line| line.trim_end() == crate::setup::SUMMARY_UNCHANGED)
    {
        return Some(0);
    }
    let mut lines = output
        .lines()
        .skip_while(|line| line.trim_end() != crate::setup::SUMMARY_CHANGED_HEADER);
    lines.next()?;
    Some(lines.take_while(|line| line.starts_with("  - ")).count())
}

/// 「次にやること」（#1510）。導入状況（[`crate::setup_bootstrap::status_all`]）から
/// [`next_step`] が決める
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextStep {
    /// エージェント CLI を入れる（ネットワークのインストーラ = **ボタンの同意で**行う）
    Install { agent: AgentKind, plan: InstallPlan },
    /// PATH を通す（自動の段が受け持つ）
    Path { agent: AgentKind },
    /// PATH を、利用者が自分のシェルの設定ファイルで足す（#2576）。種別の分からないシェル（tcsh・nu 等）は
    /// tako が設定ファイルを書けず、`tako setup bootstrap path` も自動の段も同じ断りで止まる（堂々巡り）ので、
    /// 打つコマンドを持たない。`shell` / `dirs` はその断り（[`SetupRefusal::ShellProfileUnknown`]）の中身 =
    /// 足すディレクトリの一覧（`$HOME/…` の式・重複なし）
    PathByHand {
        agent: AgentKind,
        shell: String,
        dirs: Vec<String>,
    },
    /// ログインする（ブラウザ操作。ボタンでペインを開いてコマンドを打つ）
    Login {
        agent: AgentKind,
        /// 解決できた実行ファイル（入れた直後のタブの PATH に無くても打てるよう絶対パスで打つ）
        binary: Option<String>,
    },
    /// 準備ができた → master を起動する
    Master { agent: AgentKind },
    /// 導入状況を確かめられない（ログインシェルが応答しない等）
    Unknown { reason: String },
    /// まだ一度も setup が最後まで走っていない → 「セットアップを実行」（#2879。導入状況は見ない）
    Setup,
    /// 使える系統はあるが、この端末で対話のセットアップをまだ済ませていない →
    /// 「セットアップを続ける」= もう一度 `tako setup`（最後に対話アシスタント。#2879）
    Continue { agent: AgentKind },
    /// 端末の `tako setup`（またはその対話アシスタント）が走っている（#2879）。ボタンはタブを見せるだけ
    Running { phase: crate::setup::RunningPhase },
}

impl NextStep {
    /// 機械が読む名前（CLI / MCP の `step.kind`）
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Install { .. } => "install",
            // 段は同じ PATH（違うのは打つコマンドが無いこと = #2576）
            Self::Path { .. } | Self::PathByHand { .. } => "path",
            Self::Login { .. } => "login",
            Self::Master { .. } => "master",
            Self::Unknown { .. } => "unknown",
            Self::Setup => "setup",
            Self::Continue { .. } => "continue",
            Self::Running { .. } => "running",
        }
    }

    pub fn agent(&self) -> Option<AgentKind> {
        match self {
            Self::Install { agent, .. }
            | Self::Path { agent }
            | Self::PathByHand { agent, .. }
            | Self::Login { agent, .. }
            | Self::Master { agent }
            | Self::Continue { agent } => Some(*agent),
            Self::Unknown { .. } | Self::Setup | Self::Running { .. } => None,
        }
    }

    /// 人が自分で打つならこれ（#322 の最簡形。claude には `--agent` を付けない）。
    /// `None` = 打てるコマンドが無い（[`Self::PathByHand`]。そのコマンドも同じ断りで止まる = #2576）
    pub fn command(&self) -> Option<String> {
        let flag = |agent: &AgentKind| crate::setup_bootstrap::agent_flag(*agent);
        Some(match self {
            Self::Install { agent, .. } => format!("tako setup bootstrap install{}", flag(agent)),
            Self::Path { agent } => format!("tako setup bootstrap path{}", flag(agent)),
            Self::PathByHand { .. } => return None,
            Self::Login { agent, .. } => {
                crate::orchestrator::agent_cli::auth_command((*agent).into())
                    .unwrap_or(agent.as_str())
                    .to_string()
            }
            Self::Master { .. } => MASTER_COMMAND.to_string(),
            Self::Unknown { .. } | Self::Setup | Self::Continue { .. } => SETUP_COMMAND.to_string(),
            // 走っているあいだは打つものが無い（同じ setup をもう 1 本起こさない）
            Self::Running { .. } => return None,
        })
    }

    /// [`Self::PathByHand`] の断り（`tako setup bootstrap path` が返すのと同じ。足すディレクトリの一覧つき）
    pub fn path_by_hand_refusal(&self) -> Option<SetupRefusal> {
        match self {
            Self::PathByHand { shell, dirs, .. } => Some(SetupRefusal::ShellProfileUnknown {
                shell: shell.clone(),
                dirs: dirs.clone(),
            }),
            _ => None,
        }
    }

    /// 機械が読む形（文は表示言語）
    pub fn to_json(&self) -> Value {
        self.to_json_in(tako_core::i18n::lang())
    }

    /// 表示言語を明示しての [`Self::to_json`]（**言語グローバルに触らない**）
    pub fn to_json_in(&self, lang: tako_core::i18n::Lang) -> Value {
        let mut out = json!({
            "kind": self.kind(),
            "agent": self.agent().map(AgentKind::as_str),
            "command": self.command(),
        });
        match self {
            Self::Install { plan, .. } => out["install_plan"] = plan.to_json(),
            // #2576: 打つコマンドの代わりに、足すディレクトリと断りの全文を載せる
            Self::PathByHand { dirs, .. } => {
                out["dirs"] = json!(dirs);
                if let Some(refusal) = self.path_by_hand_refusal() {
                    out["note"] = json!(refusal.text_in(lang));
                }
            }
            Self::Unknown { reason } => out["reason"] = json!(reason),
            Self::Running { phase } => out["phase"] = json!(phase.as_str()),
            _ => {}
        }
        out
    }
}

/// master を起こせる系統か（agy は worker 専用。正本は能力マトリクスの 1 マス = #982）
fn can_lead(agent: AgentKind) -> bool {
    tako_core::agent_support::supports(agent.into(), tako_core::agent_support::keys::MASTER_LAUNCH)
}

/// 導入状況から「次にやること」を決める。
///
/// master を起こせる系統（claude / codex）だけを見る。1 つでも使えれば master、無ければ
/// [`crate::setup_bootstrap::choose_target`] と同じ順（途中まで入っているものを先に仕上げる →
/// 推奨 = claude）で 1 系統を選ぶ。確かめられなかった系統があれば導入へ進めない（#1532 / #1533）。
/// PATH の段で種別の分からないシェルなら、`tako setup bootstrap path` と同じ断り（足すディレクトリの一覧）を
/// 引いて [`NextStep::PathByHand`] にする（#2576。判定は `tako setup` の残りと同じ 1 実装 = #2564）
pub fn next_step(states: &[(AgentKind, Result<BootstrapState, String>)]) -> NextStep {
    next_step_with(states, &crate::setup_bootstrap::path_by_hand_refusal_for)
}

/// [`next_step`] の本体（**純関数**）。`by_hand` は PATH を利用者が自分のシェルの設定ファイルで足すしかないときの
/// 断り（[`crate::setup_bootstrap::path_by_hand_refusal_for`]。テストは差し替える）
fn next_step_with(
    states: &[(AgentKind, Result<BootstrapState, String>)],
    by_hand: &dyn Fn(&BootstrapState) -> Option<SetupRefusal>,
) -> NextStep {
    let leaders: Vec<(AgentKind, Result<BootstrapState, String>)> = states
        .iter()
        .filter(|(agent, _)| can_lead(*agent))
        .cloned()
        .collect();
    if let Some((agent, _)) = leaders
        .iter()
        .find(|(_, state)| state.as_ref().is_ok_and(|s| s.step == Step::Ready))
    {
        return NextStep::Master { agent: *agent };
    }
    if let Some(reason) = leaders.iter().find_map(|(agent, state)| {
        let notice = state.as_ref().ok()?.step_unconfirmed()?;
        Some(format!(
            "{}: ログインシェルが {} 秒で応答しないので導入状況を確かめられません",
            agent.as_str(),
            notice.waited_secs
        ))
    }) {
        return NextStep::Unknown { reason };
    }
    if leaders.is_empty() || leaders.iter().all(|(_, state)| state.is_err()) {
        let reason = leaders
            .iter()
            .find_map(|(_, state)| state.as_ref().err().cloned())
            .unwrap_or_else(|| "導入状況を読めません".to_string());
        return NextStep::Unknown { reason };
    }
    let target = crate::setup_bootstrap::choose_target(&leaders);
    let Some(state) = leaders
        .iter()
        .find(|(agent, _)| *agent == target)
        .and_then(|(_, state)| state.as_ref().ok())
    else {
        return NextStep::Unknown {
            reason: format!("{}: 導入状況を読めません", target.as_str()),
        };
    };
    match state.step {
        Step::Install => NextStep::Install {
            agent: target,
            plan: state.plan.clone(),
        },
        Step::Path => match by_hand(state) {
            Some(SetupRefusal::ShellProfileUnknown { shell, dirs }) => NextStep::PathByHand {
                agent: target,
                shell,
                dirs,
            },
            _ => NextStep::Path { agent: target },
        },
        Step::Auth => NextStep::Login {
            agent: target,
            binary: state.binary.clone(),
        },
        Step::Ready => NextStep::Master { agent: target },
    }
}

/// いまの「次にやること」を確かめる（**待つ**: ログインシェル・認証の問い合わせ。
/// UI スレッドでは呼ばない = dispatch の offload が background で呼ぶ）
pub fn probe() -> NextStep {
    next_step(&crate::setup_bootstrap::status_all_within(
        tako_core::probe::auth_probe_timeout(),
    ))
}

/// 対話のセットアップの進み具合（#2879。`config.yaml` の `setup` と、走っている印）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SetupProgress {
    /// setup が一度でも最後まで走ったか（`setup.completed`）
    pub completed: bool,
    /// この端末で対話のセットアップを済ませたか（`setup.dialogue_done == Some(true)`）
    pub dialogue_done: bool,
    /// いま端末の `tako setup` が走っているなら、その段（[`crate::setup::running_phase`]）
    pub running: Option<crate::setup::RunningPhase>,
}

impl SetupProgress {
    /// いまの進み具合を読む（ファイルを読むだけ。ログインシェルは起こさない）
    pub fn read() -> Result<Self, String> {
        let config = crate::setup::load_config()?;
        Ok(Self {
            completed: config.setup.completed,
            dialogue_done: config.setup.dialogue_done == Some(true),
            running: crate::setup::running_phase(),
        })
    }
}

/// #2879 の「次にやること」（**純関数**）。
///
/// 走っていれば [`NextStep::Running`]、一度も最後まで走っていなければ [`NextStep::Setup`]
/// （**この 2 つは導入状況を確かめない** = `states` を呼ばない。起動直後の確かめでログインシェルを起こさない）。
/// それ以外は導入状況から [`next_step`] と同じ段を出し、master を起こせる段でも対話を済ませていなければ
/// [`NextStep::Continue`]（対話を通らずに「master を起動」へ進まない）
pub fn next_step_guided(
    progress: &SetupProgress,
    states: impl FnOnce() -> Vec<(AgentKind, Result<BootstrapState, String>)>,
) -> NextStep {
    if let Some(phase) = progress.running {
        return NextStep::Running { phase };
    }
    if !progress.completed {
        return NextStep::Setup;
    }
    match next_step(&states()) {
        NextStep::Master { agent } if !progress.dialogue_done => NextStep::Continue { agent },
        other => other,
    }
}

/// #2879 の「次にやること」を確かめる（**待つことがある** = UI スレッドでは呼ばない）
pub fn probe_guided() -> NextStep {
    match SetupProgress::read() {
        Ok(progress) => next_step_guided(&progress, || {
            crate::setup_bootstrap::status_all_within(tako_core::probe::auth_probe_timeout())
        }),
        Err(reason) => NextStep::Unknown { reason },
    }
}

/// 導線の形に合った確かめ（dispatch の offload が background で呼ぶ）
pub fn probe_in(flow: Flow) -> NextStep {
    match flow {
        Flow::Guided => probe_guided(),
        Flow::Banner | Flow::Auto => probe(),
    }
}

/// 「次にやること」のボタンが何をするか（#1510）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// 導線のペイン（無ければ新しいタブ）へコマンド行を打つ（利用者が自分で打つのと同じ経路）
    Pane {
        /// 新しいタブを開くときの題
        title: &'static str,
        line: String,
    },
    /// master を起動する（新しいタブ。#549 の「master を起動」と同じ）
    Master,
    /// 自動の段をもう一度走らせる（PATH は自動の段が受け持つ。#1510 の形だけ）
    Auto,
    /// 導線のタブを見せるだけ（setup・対話が走っている。#2879）
    Focus,
    /// 確かめ直すだけ（自分で PATH を足し終えた。#2879 の形の [`NextStep::PathByHand`]）
    Recheck,
}

/// ボタンの中身（#1510 の形 = [`Flow::Auto`]。**純関数**。方言を引数で受けるので macOS 上から
/// PowerShell の形も検査できる）。導線の形を選ぶのは [`launch_for_flow`]
pub fn launch_for(step: &NextStep, dialect: ShellDialect) -> Launch {
    launch_for_flow(step, dialect, Flow::Auto)
}

/// ボタンの中身（**純関数**）。[`Flow::Guided`] では [`Launch::Auto`] を返さない（自動の段を走らせない）
pub fn launch_for_flow(step: &NextStep, dialect: ShellDialect, flow: Flow) -> Launch {
    let guided = flow == Flow::Guided;
    // #2879: ボタン起点の形の setup の段は **GUI と同じ版の CLI**（実行ファイルの隣を先に = 自動の段と同じ
    // [`auto_setup_binary`]）を打つ。導線は CLI が書く走っている印・対話が済んだ印で進むので、`/Applications` や
    // PATH の先の古い版を打つと段が「セットアップを続ける」から先へ進まない。#1510 の形（A/B）は従来の解決のまま
    let setup_tab = |sub: &str| Launch::Pane {
        title: "setup",
        line: if guided {
            format!("{} {sub}", dialect.command_word(&auto_setup_binary()))
        } else {
            launch_command_line_in(dialect, sub)
        },
    };
    match step {
        NextStep::Install { agent, .. } => setup_tab(&format!(
            "setup bootstrap install{}",
            crate::setup_bootstrap::agent_flag(*agent)
        )),
        // #2879: 自動の段が無いので、PATH を通すコマンドをタブで打つ
        NextStep::Path { agent } if guided => setup_tab(&format!(
            "setup bootstrap path{}",
            crate::setup_bootstrap::agent_flag(*agent)
        )),
        NextStep::Path { .. } => Launch::Auto,
        // #2576: 自動の段もそのシェルの設定ファイルは書けないが、走らせ直すと足し終えたかを確かめ直し
        // （足し終えていれば tako CLI の段は already_on_path）、続きの段（MCP 登録など）も当て直して次の段へ進む。
        // 同じ断りで止まるコマンドをタブへ打たない。#2879 の形は確かめ直すだけ（続きは「セットアップを続ける」）
        NextStep::PathByHand { .. } if guided => Launch::Recheck,
        NextStep::PathByHand { .. } => Launch::Auto,
        NextStep::Login { agent, binary } => Launch::Pane {
            title: "login",
            line: login_line(dialect, *agent, binary.as_deref()),
        },
        NextStep::Master { .. } => Launch::Master,
        NextStep::Unknown { .. } | NextStep::Setup | NextStep::Continue { .. } => {
            setup_tab("setup")
        }
        NextStep::Running { .. } => Launch::Focus,
    }
}

/// 同じ段のボタンをもう一度押したとき、導線のタブへコマンド行を打ち直すか（#2879）。
///
/// 「セットアップを実行 / 続ける」は、走っているあいだは [`NextStep::Running`] になるので、この段が
/// 出ている = 前の `tako setup` は終わっている（途中で止めた）= 打ち直してよい。導入・ログインは
/// 走っているかを知る印が無いので、従来どおり打ち直さずにタブを見せる
pub fn retypes_on_repress(step: &NextStep) -> bool {
    matches!(step, NextStep::Setup | NextStep::Continue { .. })
}

/// ログインのコマンド行。コマンドの正本は `agent_cli::auth_command`（3 系統とも実物で確認済み）で、
/// 先頭の CLI 名だけを解決済みの実行ファイルへ差し替える（入れた直後のタブは PATH に無いことがある）
fn login_line(dialect: ShellDialect, agent: AgentKind, binary: Option<&str>) -> String {
    let command =
        crate::orchestrator::agent_cli::auth_command(agent.into()).unwrap_or(agent.as_str());
    let mut words = command.split_whitespace();
    let name = words.next().unwrap_or(agent.as_str());
    let rest: Vec<&str> = words.collect();
    let head = dialect.command_word(binary.unwrap_or(name));
    if rest.is_empty() {
        head
    } else {
        format!("{head} {}", rest.join(" "))
    }
}

/// ボタンを押した後、ペインの仕事（導入・ログイン）の終わりを見張る長さ
pub const WATCH_FOR: Duration = Duration::from_secs(20 * 60);
/// 見張りの間隔（1 回の確かめはログインシェルと認証の問い合わせ = 秒の単位で重い）
pub const WATCH_INTERVAL: Duration = Duration::from_secs(5);

/// 自動の段の状態
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum AutoState {
    /// このプロセスではまだ走らせていない
    #[default]
    Idle,
    Running,
    Done(AutoOutcome),
}

/// 導線の状態（GUI が持つ。CLI / MCP は dispatch `Welcome` の status で読む）
#[derive(Debug, Clone, Default)]
pub struct Onboarding {
    /// 導線の形（GUI が起動時に [`flow`] で決める。既定は #1510 の形）
    pub flow: Flow,
    pub auto: AutoState,
    /// 最後に確かめた「次にやること」（まだ確かめていなければ None）
    pub step: Option<NextStep>,
    /// ボタンで開いたペインの終わりを見張る期限（見張っていなければ None）
    pub watch_until: Option<Instant>,
    /// PATH の段のために自動の段を走らせ直したか（同じ段で回り続けない）
    pub path_retried: bool,
    /// 自動の段が終わった直後の確かめか（自動の段が進めた段を「人が進めた」と読まない）
    pub after_auto: bool,
}

/// 確かめた結果を受けて GUI が次にすること
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Effects {
    /// 自動の段を走らせる（PATH が残っている / 入れた・ログインした直後に MCP 登録などを当て直す）
    pub run_auto: bool,
    /// 見張りを続ける
    pub watch: bool,
}

impl Onboarding {
    /// 形を決めて作る
    pub fn new(flow: Flow) -> Self {
        Self {
            flow,
            ..Self::default()
        }
    }

    pub fn auto_running(&self) -> bool {
        self.auto == AutoState::Running
    }

    /// 見張っているか
    pub fn watching(&self, now: Instant) -> bool {
        self.watch_until.is_some_and(|until| now < until)
    }

    /// ボタンを押した（ペインを開いた）。終わるまで見張る
    pub fn start_watch(&mut self, now: Instant) {
        self.watch_until = Some(now + WATCH_FOR);
    }

    /// 確かめた「次にやること」を受け取り、次にすることを決める（**純関数**。時刻は引数）
    pub fn apply_step(&mut self, next: NextStep, now: Instant) -> Effects {
        if self.flow == Flow::Guided {
            return self.apply_step_guided(next, now);
        }
        let previous = self.step.as_ref().map(NextStep::kind);
        let after_auto = std::mem::take(&mut self.after_auto);
        // 人の操作（ボタンで開いたペイン・外のターミナル）で段が進んだ
        let progressed = !after_auto
            && previous.is_some_and(|kind| {
                kind != next.kind() && matches!(kind, "install" | "path" | "login")
            });
        let mut effects = Effects::default();
        if !self.auto_running() {
            // 自分で足すしかない PATH（`PathByHand`）は走らせ直さない（自動の段も同じ断りで止まる = #2576）
            if matches!(next, NextStep::Path { .. }) && !self.path_retried {
                self.path_retried = true;
                effects.run_auto = true;
            } else if progressed {
                effects.run_auto = true;
            }
        }
        if progressed || !self.watching(now) {
            self.watch_until = None;
        }
        effects.watch = self.watching(now);
        self.step = Some(next);
        effects
    }

    /// #2879 の形の [`Self::apply_step`]。**自動の段は一切頼まない**（`run_auto` は常に偽）。
    /// setup・対話が走っているあいだ（[`NextStep::Running`]）は見張りを延ばし続け、終わって段が
    /// 変わったら見張りを止める（次のボタンは人が押す）
    fn apply_step_guided(&mut self, next: NextStep, now: Instant) -> Effects {
        let previous = self.step.as_ref().map(NextStep::kind);
        self.after_auto = false;
        if matches!(next, NextStep::Running { .. }) {
            self.start_watch(now);
        } else if previous.is_some_and(|kind| kind != next.kind()) || !self.watching(now) {
            self.watch_until = None;
        }
        let effects = Effects {
            run_auto: false,
            watch: self.watching(now),
        };
        self.step = Some(next);
        effects
    }

    /// 自動の段が終わった
    pub fn finish_auto(&mut self, outcome: AutoOutcome) {
        self.auto = AutoState::Done(outcome);
        self.after_auto = true;
    }

    pub fn to_json(&self, now: Instant) -> Value {
        let auto = match &self.auto {
            AutoState::Idle => json!({ "state": "idle" }),
            AutoState::Running => json!({ "state": "running" }),
            AutoState::Done(outcome) => {
                let mut value = outcome.to_json();
                value["state"] = json!("done");
                value
            }
        };
        json!({
            "flow": self.flow.as_str(),
            "auto": auto,
            "step": self.step.as_ref().map(NextStep::to_json),
            "watching": self.watching(now),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("tako-welcome-test-{}-{name}", std::process::id()))
    }

    #[test]
    fn settings_jsonが無ければ初回起動として出す() {
        let dir = temp_dir("fresh");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("settings.json");
        assert!(should_show(&path, &Settings::default()));
    }

    #[test]
    fn settings_jsonがあれば既存ユーザーとして出さない() {
        let dir = temp_dir("existing");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        // 旧バージョンが書いた settings.json（welcome_dismissed キーを持たない）
        std::fs::write(&path, r#"{"theme":"dark"}"#).unwrap();
        let settings: Settings = serde_json::from_str(r#"{"theme":"dark"}"#).unwrap();
        assert!(!settings.welcome_dismissed, "旧ファイルの既定は未 dismiss");
        assert!(
            !should_show(&path, &settings),
            "既存ユーザー（設定ファイルあり）には出さない"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 破損した_settings_jsonでも既存ユーザー扱いで出さない() {
        let dir = temp_dir("corrupt");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, "{ this is not json").unwrap();
        // load() 相当（破損 → 既定値）でも、ファイルが在る以上は初回ではない
        assert!(!should_show(&path, &Settings::default()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dismiss済みならファイルが無くても出さない() {
        let dir = temp_dir("dismissed");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("settings.json");
        let settings = Settings {
            welcome_dismissed: true,
            ..Settings::default()
        };
        assert!(!should_show(&path, &settings));
    }

    #[test]
    fn 案内コマンドは最簡形() {
        // #322: 既定値で済む引数を付けて見せない
        assert_eq!(SETUP_COMMAND, "tako setup");
        assert_eq!(MASTER_COMMAND, "tako master");
    }

    #[test]
    fn posixのコマンド語は安全な文字をそのまま通す() {
        use tako_core::platform::shell_dialect::ShellDialect::Posix;
        assert_eq!(
            Posix.command_word("/Applications/tako.app/Contents/MacOS/tako"),
            "/Applications/tako.app/Contents/MacOS/tako"
        );
        assert_eq!(Posix.command_word("tako"), "tako");
    }

    #[test]
    fn posixのコマンド語は空白とクォートを閉じ込める() {
        use tako_core::platform::shell_dialect::ShellDialect::Posix;
        assert_eq!(Posix.command_word("/My Apps/tako"), "'/My Apps/tako'");
        assert_eq!(Posix.command_word("/a'b/tako"), r"'/a'\''b/tako'");
        // シェル注入に使える文字はクォートの内側へ入る
        assert_eq!(Posix.command_word("/x; rm -rf /"), "'/x; rm -rf /'");
    }

    /// **#899 の本体**: PowerShell では `:` と `\` を素で通し、囲むときは
    /// 呼び出し演算子を付ける（囲むだけだと実行されずに表示される）
    #[test]
    fn powershellのコマンド語は実行される形になる() {
        use tako_core::platform::shell_dialect::ShellDialect::{Posix, PowerShell};
        // 典型的な Windows の絶対パスは素で通る = 最簡形（#322）
        assert_eq!(
            PowerShell.command_word(r"C:\Users\u\tako\tako.exe"),
            r"C:\Users\u\tako\tako.exe"
        );
        // 旧実装（POSIX 決め打ち）は同じパスを囲んでしまい実行されない
        assert_eq!(
            Posix.command_word(r"C:\Users\u\tako\tako.exe"),
            r"'C:\Users\u\tako\tako.exe'"
        );
        // 空白入りは囲むが、呼び出し演算子を付けるので実行される
        assert_eq!(
            PowerShell.command_word(r"C:\Program Files\tako\tako.exe"),
            r"& 'C:\Program Files\tako\tako.exe'"
        );
        // `$` を含むパスは二重引用符だと展開されるので単引用符（リテラル）で囲む
        assert_eq!(
            PowerShell.command_word(r"C:\Users\a$b\tako.exe"),
            r"& 'C:\Users\a$b\tako.exe'"
        );
    }

    #[test]
    fn 起動コマンド行は方言で組み立てる() {
        use tako_core::platform::shell_dialect::ShellDialect::{Posix, PowerShell};
        for d in [Posix, PowerShell] {
            let line = launch_command_line_in(d, "master");
            assert!(line.ends_with(" master"), "{d:?}: {line}");
            assert!(!line.starts_with(' '), "{d:?}: バイナリ部が空: {line}");
        }
    }

    #[test]
    fn 実行コマンド行はサブコマンドを付ける() {
        let line = launch_command_line("setup");
        assert!(line.ends_with(" setup"), "実際: {line}");
        assert!(!line.starts_with(' '), "バイナリ部が空: {line}");
    }

    // --- #1510: 初回起動の導線 ---------------------------------------------

    use crate::setup_bootstrap::{plan_from, recipe_for, BootstrapState, Step};

    /// 導入状況の fixture（**値だけで組む**。HOME もログインシェルも見ない）
    fn state(agent: AgentKind, step: Step) -> BootstrapState {
        let home = Path::new("/home/testuser");
        BootstrapState {
            agent,
            binary: (step != Step::Install)
                .then(|| format!("/home/testuser/.local/bin/{}", agent.as_str())),
            authenticated: step == Step::Ready,
            account_plan: None,
            launcher_dir: home.join(".local/bin"),
            launcher_dir_on_path: step != Step::Path,
            launcher_dir_on_process_path: step != Step::Path,
            profile: None,
            profile_has_block: false,
            // 既知のシェル（#2576 の「自分で足すしかない」は当たらない。当たる形は `next_step_with` で差し替える）
            shell: Some(tako_core::shell_profile::ShellKind::Zsh),
            step,
            plan: plan_from(&recipe_for(agent), home, &home.join(".codex")),
            auth_timeout: None,
            locate_timeout: None,
            path_timeout: None,
        }
    }

    fn states(list: &[(AgentKind, Step)]) -> Vec<(AgentKind, Result<BootstrapState, String>)> {
        list.iter()
            .map(|(agent, step)| (*agent, Ok(state(*agent, *step))))
            .collect()
    }

    /// **受け入れ条件 3**: 自動の段は同意なしにネットワークのスクリプトを走らせない。
    /// 判断の口（`allows_install`）を、GUI が実際に送る JSON（deny_unknown_fields で読み戻す）で縛る。
    /// setup 側がこの値で導入の段を飛ばすことは tako-cli の `issue1510_*` テストが縛る
    #[test]
    fn issue1510_自動の段のanswersはネットワークからの導入を許さない() {
        let answers = auto_setup_answers();
        assert!(!answers.allows_install(), "{answers:?}");
        assert_eq!(answers.launch_agent.as_deref(), Some("none"));
        answers
            .validate()
            .expect("自動の段の answers は setup の検証を通る");
        let wire = serde_json::to_string(&answers).unwrap();
        let back = crate::setup::SetupAnswers::from_json(&wire).unwrap();
        assert!(
            !back.allows_install(),
            "送った JSON で読み戻しても導入を許さない: {wire}"
        );
        // 省略（MCP tako_setup・ターミナルの --yes）は従来どおり
        assert!(crate::setup::SetupAnswers::default().allows_install());
        assert!(crate::setup::SetupAnswers::from_json(r#"{"install":true}"#)
            .unwrap()
            .allows_install());
    }

    #[test]
    fn issue1510_導線は検証プロセスでは明示したときだけ動く() {
        assert!(
            onboarding_enabled_in(false, false, false),
            "製品の通常の起動"
        );
        assert!(!onboarding_enabled_in(true, false, true), "A/B の旧挙動");
        assert!(
            !onboarding_enabled_in(false, true, false),
            "隔離 GUI・セルフテストの既定"
        );
        assert!(
            onboarding_enabled_in(false, true, true),
            "HOME を差し替えた検証の明示"
        );
    }

    #[test]
    fn issue1510_導線が続いているあいだは毎回出し既存ユーザーには出さない() {
        let dir = temp_dir("1510-show");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("settings.json");
        // 初回（ファイルが無い）は導線の有無にかかわらず出す（#549 のまま）
        for flow in [Flow::Auto, Flow::Banner, Flow::Guided] {
            assert!(should_show_with(&path, &Settings::default(), flow));
        }
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "{}").unwrap();
        let active = Settings {
            welcome_onboarding: true,
            ..Settings::default()
        };
        assert!(should_show_with(&path, &active, Flow::Auto), "導線の途中");
        assert!(
            !should_show_with(&path, &active, Flow::Banner),
            "導線を動かさない起動"
        );
        assert!(
            !should_show_with(&path, &Settings::default(), Flow::Auto),
            "#1510 より前から使っている人（welcome_onboarding が無い）"
        );
        let dismissed = Settings {
            welcome_onboarding: true,
            welcome_dismissed: true,
            ..Settings::default()
        };
        assert!(
            !should_show_with(&path, &dismissed, Flow::Auto),
            "今後表示しない"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn issue1510_次にやることは使える系統があればmaster() {
        use AgentKind::*;
        let step = next_step(&states(&[
            (Claude, Step::Install),
            (Codex, Step::Ready),
            (Agy, Step::Install),
        ]));
        assert_eq!(step, NextStep::Master { agent: Codex });
        assert_eq!(step.command().as_deref(), Some("tako master"));
    }

    #[test]
    fn issue1510_agyだけ使えてもmasterにはならない() {
        use AgentKind::*;
        // agy は worker 専用（能力マトリクスの master_launch）。claude を入れる段を出す
        let step = next_step(&states(&[
            (Claude, Step::Install),
            (Codex, Step::Install),
            (Agy, Step::Ready),
        ]));
        assert_eq!(step.kind(), "install", "{step:?}");
        assert_eq!(step.agent(), Some(Claude));
        assert_eq!(
            step.command().as_deref(),
            Some("tako setup bootstrap install")
        );
    }

    #[test]
    fn issue1510_途中まで入っている系統を先に仕上げる() {
        use AgentKind::*;
        let step = next_step(&states(&[(Claude, Step::Install), (Codex, Step::Auth)]));
        assert_eq!(
            step,
            NextStep::Login {
                agent: Codex,
                binary: Some("/home/testuser/.local/bin/codex".into())
            }
        );
        assert_eq!(step.command().as_deref(), Some("codex login"));
        assert_eq!(
            next_step(&states(&[(Claude, Step::Path)])),
            NextStep::Path { agent: Claude }
        );
    }

    #[test]
    fn issue1510_確かめられない系統があれば導入へ進めない() {
        use AgentKind::*;
        let mut unconfirmed = state(Claude, Step::Install);
        unconfirmed.locate_timeout = Some(tako_core::probe::TimeoutNotice {
            label: "zsh -l".into(),
            waited_secs: 10,
        });
        let step = next_step(&[(Claude, Ok(unconfirmed))]);
        assert_eq!(step.kind(), "unknown", "{step:?}");
        assert_eq!(step.command().as_deref(), Some("tako setup"));
        let step = next_step(&[(Claude, Err("読めない".into())), (Codex, Err("x".into()))]);
        assert_eq!(
            step,
            NextStep::Unknown {
                reason: "読めない".into()
            }
        );
    }

    #[test]
    fn issue1510_ボタンの中身は方言に合った打てる形() {
        use tako_core::platform::shell_dialect::ShellDialect::{Posix, PowerShell};
        use AgentKind::*;
        let install = next_step(&states(&[(Claude, Step::Install)]));
        for dialect in [Posix, PowerShell] {
            match launch_for(&install, dialect) {
                Launch::Pane { title, line } => {
                    assert_eq!(title, "setup");
                    assert!(
                        line.ends_with(" setup bootstrap install"),
                        "{dialect:?}: {line}"
                    );
                }
                other => panic!("{other:?}"),
            }
        }
        let codex = NextStep::Install {
            agent: Codex,
            plan: plan_from(
                &recipe_for(Codex),
                Path::new("/home/testuser"),
                Path::new("/home/testuser/.codex"),
            ),
        };
        assert!(matches!(
            launch_for(&codex, Posix),
            Launch::Pane { line, .. } if line.ends_with(" setup bootstrap install --agent codex")
        ));
        // ログイン: 入れた直後のタブでも打てるよう解決済みの実行ファイルで打つ
        let login = NextStep::Login {
            agent: Claude,
            binary: Some("/home/testuser/.local/bin/claude".into()),
        };
        assert_eq!(
            launch_for(&login, Posix),
            Launch::Pane {
                title: "login",
                line: "/home/testuser/.local/bin/claude auth login".into()
            }
        );
        let windows = NextStep::Login {
            agent: Claude,
            binary: Some(r"C:\Program Files\claude\claude.exe".into()),
        };
        assert!(matches!(
            launch_for(&windows, PowerShell),
            Launch::Pane { line, .. } if line == r"& 'C:\Program Files\claude\claude.exe' auth login"
        ));
        // agy は専用のログインコマンドを持たない = 引数なしの起動が入口
        let agy = NextStep::Login {
            agent: Agy,
            binary: None,
        };
        assert!(matches!(launch_for(&agy, Posix), Launch::Pane { line, .. } if line == "agy"));
        assert_eq!(
            launch_for(&NextStep::Path { agent: Claude }, Posix),
            Launch::Auto
        );
        assert_eq!(
            launch_for(&NextStep::Master { agent: Claude }, Posix),
            Launch::Master
        );
        assert!(matches!(
            launch_for(&NextStep::Unknown { reason: "x".into() }, Posix),
            Launch::Pane { line, .. } if line.ends_with(" setup")
        ));
    }

    /// 自動の段は GUI と同じ版の CLI（隣）を先に使う。`install` を知らない古い CLI を起こさない
    #[test]
    fn issue1510_自動の段はguiの隣のcliを先に使う() {
        let exe = Path::new("/opt/tako/tako-app");
        // 区切りは OS に任せる（Windows の join は `\`。期待値も同じ組み方で作る）
        let sibling = Path::new("/opt/tako").join("tako");
        assert_eq!(
            auto_setup_binary_with(Some(exe), "tako", &|p| p == sibling.as_path()),
            Some(sibling.display().to_string())
        );
        assert_eq!(auto_setup_binary_with(Some(exe), "tako", &|_| false), None);
        assert_eq!(auto_setup_binary_with(None, "tako", &|_| true), None);
    }

    #[test]
    fn issue1510_自動の段の要約の件数を読む() {
        let unchanged = format!("tako セットアップ\n\n{}\n", crate::setup::SUMMARY_UNCHANGED);
        assert_eq!(parse_auto_summary(&unchanged), Some(0));
        let changed = format!(
            "[OK] x\n\n{}\n  - setup.completed: (未設定) -> true [default]\n  - ~/.claude/CLAUDE.md: (未設定) -> 既定の開発ルールを作成 [default]\n\nセットアップが完了しました。\n",
            crate::setup::SUMMARY_CHANGED_HEADER
        );
        assert_eq!(parse_auto_summary(&changed), Some(2));
        assert_eq!(parse_auto_summary("error: 壊れた設定"), None);
    }

    #[test]
    fn issue1510_自動の段は段が人の手で進んだときだけ当て直す() {
        use AgentKind::*;
        let t0 = Instant::now();
        let install = next_step(&states(&[(Claude, Step::Install)]));
        let login = next_step(&states(&[(Claude, Step::Auth)]));
        let master = next_step(&states(&[(Claude, Step::Ready)]));

        let mut o = Onboarding::default();
        // 起動直後の 1 回目は当て直さない（自動の段の直後に確かめたもの）
        o.finish_auto(AutoOutcome {
            completed: true,
            timed_out: false,
            changes: Some(3),
            error: None,
            log: None,
        });
        assert_eq!(o.apply_step(install.clone(), t0), Effects::default());

        // ボタンを押して見張る。まだ同じ段なら見張りを続けるだけ
        o.start_watch(t0);
        assert_eq!(
            o.apply_step(install.clone(), t0 + WATCH_INTERVAL),
            Effects {
                run_auto: false,
                watch: true
            }
        );
        // 入った（ログインの段へ進んだ）= MCP の登録などを当て直し、見張りは止める
        assert_eq!(
            o.apply_step(login.clone(), t0 + WATCH_INTERVAL * 2),
            Effects {
                run_auto: true,
                watch: false
            }
        );
        // 走っている自動の段の直後に確かめた段は「人が進めた」と読まない
        o.auto = AutoState::Running;
        assert_eq!(o.apply_step(login.clone(), t0), Effects::default());
        o.finish_auto(AutoOutcome {
            completed: true,
            timed_out: false,
            changes: Some(0),
            error: None,
            log: None,
        });
        assert_eq!(o.apply_step(login.clone(), t0), Effects::default());
        // ログインした → master。もう一度当て直す（済んだ状態で MCP の健全性などを揃える）
        o.start_watch(t0);
        assert_eq!(
            o.apply_step(master.clone(), t0 + WATCH_INTERVAL),
            Effects {
                run_auto: true,
                watch: false
            }
        );
        assert_eq!(o.step.as_ref().map(NextStep::kind), Some("master"));
    }

    #[test]
    fn issue1510_pathの段は1回だけ自動の段で直しにいく() {
        let t0 = Instant::now();
        let path = NextStep::Path {
            agent: AgentKind::Claude,
        };
        let mut o = Onboarding::default();
        assert!(o.apply_step(path.clone(), t0).run_auto);
        // 直せなかった（同じ段のまま）なら回り続けない = ボタンで人が押す
        o.auto = AutoState::Running;
        o.finish_auto(AutoOutcome {
            completed: true,
            timed_out: false,
            changes: Some(0),
            error: None,
            log: None,
        });
        assert!(!o.apply_step(path.clone(), t0).run_auto);
        assert!(!o.apply_step(path, t0).run_auto);
    }

    /// #2576: 種別の分からないシェルの PATH の段は、同じ断りで止まる `tako setup bootstrap path` を示さず、
    /// 足すディレクトリの一覧（`tako setup bootstrap path` と同じ断りの全文）で閉じる。ボタンは確かめ直し
    /// （自動の段）で、確かめた直後の 1 回きりの走らせ直しもしない（自動の段も同じ断りで止まる）
    #[test]
    fn issue2576_自分で足すしかないpathは同じ断りで止まるコマンドを示さない() {
        use tako_core::i18n::Lang;
        use tako_core::platform::shell_dialect::ShellDialect::{Posix, PowerShell};
        let refusal = SetupRefusal::ShellProfileUnknown {
            shell: "/bin/tcsh".into(),
            dirs: vec!["$HOME/.local/bin".into(), "/opt/tako/bin".into()],
        };
        let path = states(&[
            (AgentKind::Claude, Step::Path),
            (AgentKind::Codex, Step::Install),
        ]);
        let step = next_step_with(&path, &|_| Some(refusal.clone()));
        assert_eq!(
            step,
            NextStep::PathByHand {
                agent: AgentKind::Claude,
                shell: "/bin/tcsh".into(),
                dirs: vec!["$HOME/.local/bin".into(), "/opt/tako/bin".into()],
            }
        );
        assert_eq!(step.kind(), "path");
        assert_eq!(step.agent(), Some(AgentKind::Claude));
        assert_eq!(step.command(), None);
        assert_eq!(step.path_by_hand_refusal(), Some(refusal.clone()));
        for lang in [Lang::Ja, Lang::En] {
            let value = step.to_json_in(lang);
            assert!(value["command"].is_null(), "{value}");
            assert_eq!(value["dirs"], json!(["$HOME/.local/bin", "/opt/tako/bin"]));
            assert_eq!(value["note"], json!(refusal.text_in(lang)));
            assert!(!value.to_string().contains("bootstrap path"), "{value}");
        }
        for dialect in [Posix, PowerShell] {
            assert_eq!(launch_for(&step, dialect), Launch::Auto);
        }
        let mut o = Onboarding::default();
        assert!(!o.apply_step(step, Instant::now()).run_auto);
        // 当たらない（既知のシェル・Windows）なら従来どおり `tako setup bootstrap path`。種別の分からないシェル
        // 以外の断りが来ても、段の写しは変えない
        for by_hand in [None, Some(SetupRefusal::ShellProfileUnknownForUndo)] {
            let step = next_step_with(&path, &|_| by_hand.clone());
            assert_eq!(
                step,
                NextStep::Path {
                    agent: AgentKind::Claude
                }
            );
            assert_eq!(
                step.to_json_in(Lang::En),
                json!({ "kind": "path", "agent": "claude", "command": "tako setup bootstrap path" })
            );
        }
        // 実物の判定（`path_by_hand_refusal_for`）は既知のシェルの導入状況では当たらない
        assert_eq!(
            next_step(&path),
            NextStep::Path {
                agent: AgentKind::Claude
            }
        );
        // Windows はユーザー環境変数へ書くので、`$SHELL` が無い（種別 None = Windows の実際の形）導入状況でも
        // 当たらない（Windows の CI で実物の判定を通す）
        #[cfg(windows)]
        {
            let mut windows = state(AgentKind::Claude, Step::Path);
            windows.shell = None;
            assert_eq!(
                next_step(&[(AgentKind::Claude, Ok(windows))]),
                NextStep::Path {
                    agent: AgentKind::Claude
                }
            );
        }
    }

    #[test]
    fn issue1510_見張りは期限で止まる() {
        let t0 = Instant::now();
        let install = next_step(&states(&[(AgentKind::Claude, Step::Install)]));
        let mut o = Onboarding::default();
        o.apply_step(install.clone(), t0);
        o.start_watch(t0);
        assert!(o.watching(t0 + WATCH_FOR - Duration::from_secs(1)));
        let fx = o.apply_step(install, t0 + WATCH_FOR);
        assert!(!fx.watch);
        assert!(o.watch_until.is_none());
    }

    #[test]
    fn issue1510_状態のjsonは機械が読む名前と最簡コマンドを持つ() {
        let t0 = Instant::now();
        let mut o = Onboarding::default();
        o.apply_step(
            next_step(&states(&[(AgentKind::Claude, Step::Install)])),
            t0,
        );
        let value = o.to_json(t0);
        assert_eq!(value["auto"]["state"], "idle");
        assert_eq!(value["step"]["kind"], "install");
        assert_eq!(value["step"]["agent"], "claude");
        assert_eq!(value["step"]["command"], "tako setup bootstrap install");
        assert!(value["step"]["install_plan"]["official_command"].is_string());
        assert_eq!(value["watching"], false);
    }

    // --- #2879: ボタン → setup → 対話 ---------------------------------------

    use crate::setup::RunningPhase;

    #[test]
    fn issue2879_導線の形は既定がボタン起点で旧挙動はabで戻る() {
        assert_eq!(
            flow_in(false, false, false, false),
            Flow::Guided,
            "製品の既定"
        );
        assert_eq!(
            flow_in(false, true, false, false),
            Flow::Auto,
            "TAKO_2879_LEGACY=1 = #1510 の自動の段"
        );
        assert_eq!(
            flow_in(true, false, false, false),
            Flow::Banner,
            "TAKO_1510_LEGACY=1 = #549"
        );
        assert_eq!(
            flow_in(false, false, true, false),
            Flow::Banner,
            "検証プロセスの既定"
        );
        assert_eq!(
            flow_in(false, false, true, true),
            Flow::Guided,
            "TAKO_FIRST_RUN=1"
        );
    }

    /// **要件 1 の判断の口**: ボタン起点の形では、起動時に何があっても自動の段を走らせない
    #[test]
    fn issue2879_起動時はsetupを走らせない() {
        for completed in [false, true] {
            assert_eq!(startup_action(Flow::Guided, completed), Startup::Probe);
            assert_eq!(startup_action(Flow::Banner, completed), Startup::Probe);
        }
        // A/B（#1510 の形）は従来どおり、一度も最後まで走っていなければ自動の段
        assert_eq!(startup_action(Flow::Auto, false), Startup::Auto);
        assert_eq!(startup_action(Flow::Auto, true), Startup::Probe);
    }

    /// 段の判定: 走っていれば running / 未実施なら setup（どちらも導入状況を確かめない）/
    /// 使える系統があっても対話がまだなら continue / 対話が済んでいれば master
    #[test]
    fn issue2879_次にやることはsetupから対話を経てmasterへ() {
        use AgentKind::*;
        let never = |_: ()| -> Vec<(AgentKind, Result<BootstrapState, String>)> {
            panic!("この段は導入状況を確かめない（ログインシェルを起こさない）")
        };
        let fresh = SetupProgress::default();
        assert_eq!(next_step_guided(&fresh, || never(())), NextStep::Setup);
        for phase in [RunningPhase::Setup, RunningPhase::Dialogue] {
            let running = SetupProgress {
                running: Some(phase),
                ..fresh
            };
            assert_eq!(
                next_step_guided(&running, || never(())),
                NextStep::Running { phase }
            );
        }
        let done_once = SetupProgress {
            completed: true,
            ..fresh
        };
        assert_eq!(
            next_step_guided(&done_once, || states(&[(Claude, Step::Ready)])),
            NextStep::Continue { agent: Claude },
            "対話を通らずに master へ進まない"
        );
        // 導入・ログインが残っていれば従来の段（ボタン 1 つの同意）
        assert_eq!(
            next_step_guided(&done_once, || states(&[(Claude, Step::Auth)])).kind(),
            "login"
        );
        assert_eq!(
            next_step_guided(&done_once, || states(&[(Claude, Step::Install)])).kind(),
            "install"
        );
        let talked = SetupProgress {
            completed: true,
            dialogue_done: true,
            running: None,
        };
        assert_eq!(
            next_step_guided(&talked, || states(&[(Claude, Step::Ready)])),
            NextStep::Master { agent: Claude }
        );
    }

    /// CLI / MCP の `tako welcome` が同じ段を同じ名前と最簡コマンドで返す（要件 5）
    #[test]
    fn issue2879_段の名前と最簡コマンド() {
        use tako_core::i18n::Lang;
        let cases = [
            (NextStep::Setup, "setup", Some("tako setup")),
            (
                NextStep::Continue {
                    agent: AgentKind::Claude,
                },
                "continue",
                Some("tako setup"),
            ),
            (
                NextStep::Running {
                    phase: RunningPhase::Dialogue,
                },
                "running",
                None,
            ),
        ];
        for (step, kind, command) in cases {
            assert_eq!(step.kind(), kind);
            assert_eq!(step.command().as_deref(), command);
            let value = step.to_json_in(Lang::En);
            assert_eq!(value["kind"], kind);
            assert_eq!(value["command"].as_str(), command);
        }
        assert_eq!(
            NextStep::Running {
                phase: RunningPhase::Setup
            }
            .to_json_in(Lang::Ja)["phase"],
            "setup"
        );
    }

    /// ボタンの中身: setup / continue は setup のタブで `tako setup`（方言どおり）、走っているあいだは
    /// タブを見せるだけ、PATH は `tako setup bootstrap path` を打つ。**自動の段（Launch::Auto）を返さない**
    #[test]
    fn issue2879_ボタンの中身は自動の段を使わない() {
        use tako_core::platform::shell_dialect::ShellDialect::{Posix, PowerShell};
        use AgentKind::*;
        let by_hand = NextStep::PathByHand {
            agent: Claude,
            shell: "/bin/tcsh".into(),
            dirs: vec!["$HOME/.local/bin".into()],
        };
        let all = [
            NextStep::Setup,
            NextStep::Continue { agent: Claude },
            NextStep::Running {
                phase: RunningPhase::Dialogue,
            },
            next_step(&states(&[(Claude, Step::Install)])),
            NextStep::Path { agent: Claude },
            by_hand.clone(),
            NextStep::Login {
                agent: Claude,
                binary: None,
            },
            NextStep::Master { agent: Claude },
            NextStep::Unknown { reason: "x".into() },
        ];
        for dialect in [Posix, PowerShell] {
            for step in &all {
                assert_ne!(
                    launch_for_flow(step, dialect, Flow::Guided),
                    Launch::Auto,
                    "{step:?}"
                );
            }
            for step in [NextStep::Setup, NextStep::Continue { agent: Claude }] {
                match launch_for_flow(&step, dialect, Flow::Guided) {
                    Launch::Pane { title, line } => {
                        assert_eq!(title, "setup");
                        assert!(line.ends_with(" setup"), "{dialect:?}: {line}");
                    }
                    other => panic!("{other:?}"),
                }
            }
            assert!(matches!(
                launch_for_flow(&NextStep::Path { agent: Codex }, dialect, Flow::Guided),
                Launch::Pane { line, .. } if line.ends_with(" setup bootstrap path --agent codex")
            ));
        }
        assert_eq!(launch_for_flow(&all[2], Posix, Flow::Guided), Launch::Focus);
        assert_eq!(
            launch_for_flow(&by_hand, Posix, Flow::Guided),
            Launch::Recheck
        );
        // #1510 の形（A/B）は従来どおり
        assert_eq!(
            launch_for(&NextStep::Path { agent: Claude }, Posix),
            Launch::Auto
        );
        // 同じ段を押し直したら打ち直すのは setup / continue だけ（走っている setup には打たない）
        assert!(retypes_on_repress(&NextStep::Setup));
        assert!(retypes_on_repress(&all[1]));
        assert!(!retypes_on_repress(&all[2]));
        assert!(!retypes_on_repress(&all[3]));
    }

    /// 遷移: 自動の段は一切頼まない。走っているあいだは見張りを延ばし、終わって段が変わったら止める
    #[test]
    fn issue2879_遷移は自動の段を頼まず走っているあいだ見張る() {
        use AgentKind::*;
        let t0 = Instant::now();
        let mut o = Onboarding::new(Flow::Guided);
        assert_eq!(o.apply_step(NextStep::Setup, t0), Effects::default());
        o.start_watch(t0);
        let running = NextStep::Running {
            phase: RunningPhase::Dialogue,
        };
        // 対話が見張りの期限（20 分）より長く続いても見張りを延ばし続ける
        let late = t0 + WATCH_FOR + WATCH_FOR;
        assert_eq!(
            o.apply_step(running.clone(), late),
            Effects {
                run_auto: false,
                watch: true
            }
        );
        let fx = o.apply_step(NextStep::Master { agent: Claude }, late + WATCH_INTERVAL);
        assert_eq!(fx, Effects::default(), "段が変わったら見張りを止める");
        // PATH の段・人の手で進んだ段でも自動の段を頼まない（#1510 の形との違い）
        let mut o = Onboarding::new(Flow::Guided);
        assert!(!o.apply_step(NextStep::Path { agent: Claude }, t0).run_auto);
        o.start_watch(t0);
        assert!(
            !o.apply_step(NextStep::Continue { agent: Claude }, t0)
                .run_auto
        );
        assert_eq!(o.to_json(t0)["flow"], "guided");
    }

    /// 既存の利用者（settings.json がある）には出さない。#1510 の印が立ったままの人にも出さない（要件 3）
    #[test]
    fn issue2879_初回起動を済ませた人には出さない() {
        let dir = tako_core::test_residue::ScratchDir::new("welcome-2879-show");
        let path = dir.path().join("settings.json");
        assert!(
            should_show_with(&path, &Settings::default(), Flow::Guided),
            "初回"
        );
        std::fs::write(&path, "{}").unwrap();
        assert!(!should_show_with(&path, &Settings::default(), Flow::Guided));
        let mid_1510 = Settings {
            welcome_onboarding: true,
            ..Settings::default()
        };
        assert!(
            !should_show_with(&path, &mid_1510, Flow::Guided),
            "#1510 の導線の途中の人に対話を促し直さない"
        );
        let guided = Settings {
            welcome_guided: true,
            ..Settings::default()
        };
        assert!(
            should_show_with(&path, &guided, Flow::Guided),
            "ボタンを押した後は出し続ける"
        );
        let dismissed = Settings {
            welcome_guided: true,
            welcome_dismissed: true,
            ..Settings::default()
        };
        assert!(!should_show_with(&path, &dismissed, Flow::Guided));
        // 印は serde の default で false（旧ファイルを読める）
        let old: Settings = serde_json::from_str(r#"{"welcome_onboarding":true}"#).unwrap();
        assert!(!old.welcome_guided);
    }

    /// 「master を起動」で導線を終える: どちらの形の印も倒し、初回起動のまま押したら書いて初回を終える
    #[test]
    fn issue2879_masterで導線を終える() {
        let guided = Settings {
            welcome_guided: true,
            ..Settings::default()
        };
        let done = finished_settings(&guided, true).expect("印を倒して書く");
        assert!(!done.welcome_guided && !done.welcome_onboarding);
        let both = Settings {
            welcome_guided: true,
            welcome_onboarding: true,
            ..Settings::default()
        };
        let done = finished_settings(&both, true).unwrap();
        assert!(!done.welcome_guided && !done.welcome_onboarding);
        assert!(
            finished_settings(&Settings::default(), false).is_some(),
            "settings.json が無ければ書く（次の起動を初回と読ませない）"
        );
        assert_eq!(
            finished_settings(&Settings::default(), true),
            None,
            "印も無くファイルも在れば書かない"
        );
    }
}
