//! ゼロスタート導入（#868）
//!
//! 「エージェント CLI を入れたことがない人」が `tako setup` 一発で始められるようにする。
//! 検出型の setup（導入済み CLI を見つけて最適化する）の**手前**に、
//! 導入そのものを引き受ける段を足す。
//!
//! ## 段（順に進む。すべて冪等で、途中から再開できる）
//!
//! 1. [`Step::Install`] — エージェント CLI を公式インストーラで導入する
//! 2. [`Step::Path`] — ランチャーの置き場所をログインシェルの PATH へ通す
//! 3. [`Step::Auth`] — `claude auth login` の実行を**ユーザーへ依頼する**
//!    （tako は代行しない。#1129 の理由は [`auth_instructions`]）
//! 4. [`Step::Ready`] — 既存の検出型 setup へ引き継ぐ
//!
//! ## 設計の要点
//!
//! - **手順の正本はプラットフォーム境界**（[`tako_core::platform::agent_install`]）。
//!   ここは「いまどの段か」を判定して実行するだけで、URL やパスを持たない
//! - **失敗は黙って飲まない**。どの段で何が起きたかを [`BootstrapError`] の
//!   具体的な文面にして返す（受け入れ条件 3）
//! - **入れる前に何をどこに入れるか出す**（[`InstallPlan`]。受け入れ条件 4）
//! - Homebrew 自体が無い場合は**案内だけ**にする。Homebrew のインストーラは
//!   sudo でパスワードを求める（実物で確認: 2026-08-21 時点の install.sh に
//!   sudo 参照 49 箇所・`have_sudo_access`）。setup が黙って権限昇格を走らせない

use crate::orchestrator::agent::WorkerAgent;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tako_core::platform::agent_install::{self, AgentKind, InstallRecipe};
use tako_core::platform::user_path;
use tako_core::shell_profile::{self, PathChange, ShellKind};

/// 導入の進み具合。`status()` が「次に何をすべきか」として返す
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// CLI が無い（か、あるが自動導入できないプラットフォーム）
    Install,
    /// CLI はあるが PATH から引けない
    Path,
    /// CLI はあるが未認証
    Auth,
    /// 検出型 setup へ進める
    Ready,
}

impl Step {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Path => "path",
            Self::Auth => "auth",
            Self::Ready => "ready",
        }
    }

    /// 利用者向けの 1 行説明
    pub fn describe(self) -> &'static str {
        match self {
            Self::Install => "Claude Code をインストールします",
            Self::Path => "claude コマンドをどのターミナルからも使えるようにします",
            Self::Auth => "Claude アカウントにログインします",
            Self::Ready => "導入は済んでいます",
        }
    }
}

/// 「何をどこに入れるか」。**実行前に必ずこれを見せる**（受け入れ条件 4）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    pub agent: &'static str,
    /// 公式ドキュメントに載っているコマンド（利用者が手で打っても同じ結果になる形）
    pub official_command: String,
    /// 取得元 URL
    pub source_url: String,
    /// コマンド本体の置き場所
    pub launcher: PathBuf,
    /// 実体（バージョンごと）の置き場所
    pub payload: PathBuf,
    /// バックグラウンド自動更新が効くか
    pub auto_updates: bool,
    /// tako が実行を代行できるか。false = 手順を案内するだけ
    pub can_run: bool,
}

impl InstallPlan {
    /// 表示用の行（CLI・GUI・MCP のどこから出しても同じ文面になるよう 1 か所で作る）
    pub fn lines(&self) -> Vec<String> {
        vec![
            format!("実行するコマンド: {}", self.official_command),
            format!("取得元: {}", self.source_url),
            format!("コマンドの置き場所: {}", display_path(&self.launcher)),
            format!("本体の置き場所: {}", display_path(&self.payload)),
            if self.auto_updates {
                "以後の更新: Claude Code が自分でバックグラウンド更新します".to_string()
            } else {
                "以後の更新: 手動で更新が必要です".to_string()
            },
            "sudo（管理者権限）は使いません。ホームディレクトリの中だけで完結します".to_string(),
        ]
    }

    pub fn to_json(&self) -> Value {
        json!({
            "agent": self.agent,
            "official_command": self.official_command,
            "source_url": self.source_url,
            "launcher": self.launcher.display().to_string(),
            "payload": self.payload.display().to_string(),
            "auto_updates": self.auto_updates,
            "can_run": self.can_run,
            "lines": self.lines(),
        })
    }
}

/// 依存ツール 1 件の状態（tmux 等）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepState {
    pub bin: String,
    pub found: Option<String>,
    pub required: bool,
}

/// いまの導入状況
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapState {
    pub agent: &'static str,
    /// 解決できた実行ファイル（PATH 外のランチャーも含む）
    pub binary: Option<String>,
    pub authenticated: bool,
    /// ランチャーの置き場所が PATH に入っているか
    pub launcher_dir: PathBuf,
    pub launcher_dir_on_path: bool,
    /// PATH 追記の書き先（判定できないシェルなら None）
    pub profile: Option<PathBuf>,
    pub profile_has_block: bool,
    pub shell: Option<ShellKind>,
    pub step: Step,
    pub plan: InstallPlan,
}

impl BootstrapState {
    pub fn to_json(&self) -> Value {
        json!({
            "agent": self.agent,
            "installed": self.binary.is_some(),
            "binary": self.binary,
            "authenticated": self.authenticated,
            "launcher_dir": self.launcher_dir.display().to_string(),
            "launcher_dir_on_path": self.launcher_dir_on_path,
            "profile": self.profile.as_ref().map(|p| p.display().to_string()),
            "profile_has_block": self.profile_has_block,
            "shell": self.shell.map(ShellKind::as_str),
            "next_step": self.step.as_str(),
            "next_step_description": self.step.describe(),
            "install_plan": self.plan.to_json(),
            "deps": deps_json(),
            "homebrew": homebrew_json(),
        })
    }
}

/// ホームディレクトリ。**取得できない環境では何も書かない**
fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| "ホームディレクトリを特定できません（HOME が未設定）".to_string())
}

fn display_path(path: &Path) -> String {
    let text = path.display().to_string();
    match home_dir() {
        Ok(home) => {
            let home = home.display().to_string();
            match text.strip_prefix(&home) {
                Some(rest) => format!("~{rest}"),
                None => text,
            }
        }
        Err(_) => text,
    }
}

/// この環境の手順
pub fn recipe() -> InstallRecipe {
    agent_install::current_recipe(AgentKind::Claude)
}

/// `TAKO_1057_LEGACY=1` で #1057 前の挙動へ戻す（**同一バイナリで A/B が取れる**）。
///
/// 戻るのは 3 点: ①Windows は自動インストールを代行しない ②PATH の判定に
/// `exe::find`（PATH 外まで走査）を使う ③引き継ぎ先を探さない
pub fn legacy_mode() -> bool {
    std::env::var_os("TAKO_1057_LEGACY").is_some()
}

/// 認証の段でユーザーへ出す案内（理由 + 次の一手）。**文面の正本はここ 1 箇所**。
///
/// ## tako が `claude auth login` を代行しない理由（#1129）
///
/// ブラウザ操作待ちのプロセスは**自分では終わらない**。tako が起こすと寿命の
/// 持ち主が居なくなり、Windows は子プロセスの終了要求（#1067 の境界 B5）が
/// 未実装なので、ペイン close も隔離インスタンスの終了も孫を回収しない。
/// 実機ではこれで `claude auth login` が 1 日で 46 本まで積み上がり、
/// `Win32_Processor.LoadPercentage` が 100% に張り付いた（#1129 の採取）。
///
/// 「人が見ているか」を stdin が端末かどうかで判別することはできない
/// （セルフテストがペインへ打ち込む `tako setup` も PTY 上では端末に見える）。
/// だから条件で絞るのではなく**構造的に起こさない**。
/// これは AGENTS.md / docs / MCP の説明（#1057「認証は代行させない」）と同じ契約で、
/// コードだけがそこから外れていた。
pub fn auth_instructions() -> Vec<String> {
    let cmd = crate::orchestrator::agent_cli::auth_command(tako_core::agent_support::Agent::Claude)
        .unwrap_or("claude auth login");
    vec![
        "Claude アカウントへのログインが必要です。".to_string(),
        "ブラウザでの操作が要るため tako は代行しません。".to_string(),
        format!("次の 1 手: {cmd}"),
        "ログインしたら tako setup をやり直してください".to_string(),
    ]
}

/// `TAKO_1129_LEGACY=1` で修正前（tako が `claude auth login` を起こす）へ戻す。
/// 同一バイナリで A/B を取るためだけの逃げ道
pub fn legacy_auth_launch() -> bool {
    std::env::var_os("TAKO_1129_LEGACY").is_some()
}

/// 「何をどこに入れるか」
pub fn install_plan() -> Result<InstallPlan, String> {
    let home = home_dir()?;
    let r = recipe();
    Ok(InstallPlan {
        agent: r.agent.as_str(),
        official_command: r.source.official_command.to_string(),
        source_url: r.source.url.to_string(),
        launcher: r.launcher_path_in(&home),
        payload: r.payload_dir_in(&home),
        auto_updates: r.auto_updates,
        can_run: r.tako_can_run,
    })
}

/// エージェント CLI を解決する。PATH に無くても**インストーラが置く場所**を見る。
///
/// インストール直後は profile へ書いた PATH が現プロセスにも
/// `$SHELL -l -c` にも反映されない（シェルを開き直すまで）。ここで拾わないと
/// 「入れたのに見つかりません」で setup が止まる
pub fn resolve_binary() -> Option<String> {
    let r = recipe();
    if let Some(found) = tako_core::platform::exe::find(r.agent.as_str()) {
        return Some(found);
    }
    let home = home_dir().ok()?;
    let launcher = r.launcher_path_in(&home);
    is_executable(&launcher).then(|| launcher.display().to_string())
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// 認証済みか。`claude auth status --json` の `loggedIn` を見る。
/// **メールアドレスや組織名は読み捨てる**（診断ログへ個人情報を出さないため）
pub fn is_authenticated(binary: &str) -> bool {
    let Some(output) =
        tako_core::platform::process::no_console_window(&mut std::process::Command::new(binary))
            .args(["auth", "status", "--json"])
            .stdin(std::process::Stdio::null())
            .output()
            .ok()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    serde_json::from_slice::<Value>(&output.stdout)
        .ok()
        .and_then(|v| v["loggedIn"].as_bool())
        .unwrap_or(false)
}

/// ログインシェルの種別と profile
fn shell_target() -> (Option<ShellKind>, Option<PathBuf>) {
    let home = match home_dir() {
        Ok(home) => home,
        Err(_) => return (None, None),
    };
    let shell = std::env::var("SHELL").unwrap_or_default();
    let kind = ShellKind::from_shell_path(&shell).or_else(|| {
        // `$SHELL` が無い（GUI から起動した .app 等）ときは OS の既定を仮定する。
        // macOS は 10.15 以降 zsh が既定
        (!cfg!(windows)).then_some(ShellKind::Zsh)
    });
    let profile = kind.map(|k| home.join(k.login_profile_rel()));
    (kind, profile)
}

/// いまの導入状況を調べる（読み取りだけ。副作用なし）
pub fn status() -> Result<BootstrapState, String> {
    let home = home_dir()?;
    let r = recipe();
    let plan = install_plan()?;
    let binary = resolve_binary();
    let authenticated = binary.as_deref().is_some_and(is_authenticated);
    let launcher_dir = r.launcher_dir_in(&home);
    let on_path = launcher_dir_on_path(&launcher_dir);
    let (shell, profile) = shell_target();
    let profile_has_block = profile
        .as_deref()
        .is_some_and(shell_profile::profile_has_block);

    let step = if binary.is_none() {
        Step::Install
    } else if !on_path {
        Step::Path
    } else if !authenticated {
        Step::Auth
    } else {
        Step::Ready
    };

    Ok(BootstrapState {
        agent: r.agent.as_str(),
        binary,
        authenticated,
        launcher_dir,
        launcher_dir_on_path: on_path,
        profile,
        profile_has_block,
        shell,
        step,
        plan,
    })
}

/// ランチャーの置き場所が「**新しく開いたターミナルが見る PATH**」に入っているか。
///
/// ## Windows で `exe::find` を使わない理由（#1057）
///
/// [`tako_core::platform::exe::find`] は PATH の外（`~\.local\bin` 等）まで
/// 走査する（「入れたのに再ログインしていない」を拾う保険。#525）。
/// これを on_path の判定に使うと **「PATH に無いが tako からは見つかる」を
/// 「PATH に在る」と誤って答える**ので、PATH を通す段が丸ごと飛ぶ。
/// Windows は PATH をレジストリで持つので、そちらを直接見る（境界 B23）。
///
/// unix は `exe::find` がログインシェルの `command -v` なので PATH の実態を
/// そのまま反映する（`.app` の痩せた PATH 対策として必要）＝従来のまま
fn launcher_dir_on_path(dir: &Path) -> bool {
    let path_var = std::env::var("PATH").unwrap_or_default();
    if user_path::is_supported() && !legacy_mode() {
        return user_path::contains_entry(&path_var, dir)
            || user_path::read()
                .map(|value| user_path::contains_entry(&value.raw, dir))
                .unwrap_or(false);
    }
    shell_profile::path_contains(&path_var, dir)
        || login_shell_sees(dir)
        || tako_core::platform::exe::find(recipe().agent.as_str()).is_some()
}

/// ログインシェルの PATH に `dir` が入っているか（`.app` の痩せた PATH 対策）
///
/// # これは意図的に unix 専用（B21 へ機械的に寄せないこと）
///
/// `$SHELL -l -c` を直に起こしているので、#877 が片付けている
/// 「`$SHELL -l -c` の直書きの一族」に見える。**だが意図が違う**ので
/// [`tako_core::platform::child_cmd::user_env_cli`]（境界 B21）へは寄せられない。
///
/// | | 一族（agents 走査など） | ここ |
/// |---|---|---|
/// | 意図 | ユーザーの環境で **CLI を 1 回走らせる** | **ログインシェルの PATH を読む** |
/// | Windows の対応物 | PATH で解決した実体を直接起動 | **無い** |
///
/// Windows の PATH は**レジストリ由来でシェル profile 由来ではない**ため、
/// 「ログインシェルの PATH」という概念そのものが存在しない。`user_env_cli` は
/// Windows 側に `program` / `args` を要求するが、ここには渡せるものが無い。
///
/// なので Windows は false を返して先に落とし、判定は
/// [`tako_core::platform::exe::find`]（境界 B16。PATH + ユーザーが入れがちな場所を走査）と
/// [`resolve_binary`]（インストーラの置き場所を直接見る）に任せる。
/// **壊れている経路ではなく、Windows には要らない経路**。
///
/// 番犬（`agents走査がposixシェルの直起動へ戻っていない`）の走査範囲が
/// `tako-control/src/orchestrator/` から広がってここが引っかかるようになったら、
/// 寄せるのではなく**理由つきの許可**にするか、B21 側へ
/// 「ログインシェルの PATH を読む」専用の口（Windows は `None`）を足すこと（#868 / #877）
fn login_shell_sees(dir: &Path) -> bool {
    if cfg!(windows) {
        return false;
    }
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".into());
    // #586: GUI プロセス（dispatch）から到達するのでコンソールウィンドウを出させない
    let Ok(output) =
        tako_core::platform::process::no_console_window(&mut std::process::Command::new(shell))
            .args(["-l", "-c", "printf %s \"$PATH\""])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
    else {
        return false;
    };
    shell_profile::path_contains(&String::from_utf8_lossy(&output.stdout), dir)
}

// --- インストール実行 ---

/// インストーラを取得して実行する。
///
/// 公式の 1 行（`curl … | bash`）と結果は同じだが、**取得と実行を分ける**。
/// こうすると「取れなかった」「取れたが中身がスクリプトでない（プロキシの HTML
/// エラーページ）」「実行が失敗した」を切り分けて具体的に報告できる。
/// 公式のトラブルシュートにも `syntax error near unexpected token '<'` として
/// 載っている実在の失敗モードで、パイプのままだと利用者には理由が見えない
pub fn install(opts: InstallOptions) -> Result<Value, String> {
    let plan = install_plan()?;
    if opts.dry_run {
        return Ok(json!({
            "performed": false,
            "reason": "dry_run",
            "install_plan": plan.to_json(),
        }));
    }
    // セルフテストは開発者の実ホームで走る（`TAKO_ISOLATED=1` が隔離するのは tako の
    // データディレクトリだけで HOME ではない）。dry_run の扱いが将来壊れたときに
    // **実インストールが実ホームへ走る**ことがないよう、ここで構造的に止める。
    // 過去にテストが実環境を壊した事故があるので、経路自体を塞いでおく
    if std::env::var_os("TAKO_SELF_TEST").is_some() {
        return Err(
            "セルフテスト中は実インストールを行いません（dry_run でのみ呼べます）".to_string(),
        );
    }
    if !plan.can_run || (legacy_mode() && cfg!(windows)) {
        return Err(format!(
            "この環境では tako が自動インストールを代行できません（{}）。\n\
             次のコマンドを自分で実行してから `tako setup` をやり直してください:\n  {}",
            std::env::consts::OS,
            plan.official_command,
        ));
    }
    let script = fetch_installer(&plan)?;
    let result = run_installer(&script, opts.interactive);
    // 一時ファイルは成否にかかわらず片付ける
    let _ = std::fs::remove_file(&script);
    let log = result?;

    let binary = resolve_binary().ok_or_else(|| {
        format!(
            "インストーラは正常終了しましたが {} が見つかりません。\n\
             `{}` を手で実行して、出力に出るエラーを確認してください",
            display_path(&plan.launcher),
            plan.official_command
        )
    })?;
    Ok(json!({
        "performed": true,
        "binary": binary,
        "install_plan": plan.to_json(),
        // 対話実行では利用者が画面で見ているので空。捕捉実行では AI が診断に使える
        "output": log,
    }))
}

/// インストーラの取得手段。
///
/// curl / wget が無い環境でも詰まらないよう、最後の手段として PowerShell の
/// `Invoke-WebRequest` を置く（Windows 10 1803 以降は `curl.exe` が同梱されるので
/// 通常はそちらが選ばれる。実機実測では `C:\Windows\System32\curl.exe`）
enum Downloader {
    Curl(String),
    Wget(String),
    PowerShell(String),
}

impl Downloader {
    /// この環境で使えるものを 1 つ選ぶ
    fn detect() -> Option<Self> {
        if let Some(bin) = tako_core::platform::exe::find("curl") {
            return Some(Self::Curl(bin));
        }
        if let Some(bin) = tako_core::platform::exe::find("wget") {
            return Some(Self::Wget(bin));
        }
        if !cfg!(windows) {
            return None;
        }
        tako_core::platform::exe::find("powershell")
            .or_else(|| tako_core::platform::exe::find("pwsh"))
            .or_else(|| Some("powershell.exe".to_string()))
            .map(Self::PowerShell)
    }

    fn program(&self) -> &str {
        match self {
            Self::Curl(bin) | Self::Wget(bin) | Self::PowerShell(bin) => bin,
        }
    }
}

/// PowerShell 経路の取得スクリプト。**URL と保存先は env で渡す**ので
/// 引用符の入れ子もコードページも関与しない（`platform::user_path` と同じ作法）
const POWERSHELL_FETCH_SCRIPT: &str = "\
$ErrorActionPreference = 'Stop'\n\
$ProgressPreference = 'SilentlyContinue'\n\
Invoke-WebRequest -UseBasicParsing -Uri $env:TAKO_FETCH_URL -OutFile $env:TAKO_FETCH_DEST\n";

/// インストーラを一時ファイルへ取得する。取得できた中身が本物かまで見る
fn fetch_installer(plan: &InstallPlan) -> Result<PathBuf, String> {
    let runner = recipe().runner;
    let downloader = Downloader::detect().ok_or_else(|| {
        "curl も wget も見つかりません。どちらかを導入してから再実行してください\n\
         （macOS なら通常 curl が標準で入っています）"
            .to_string()
    })?;
    // 拡張子はインタプリタが要求するもの（PowerShell は `.ps1` 以外を実行しない）
    let dest = std::env::temp_dir().join(format!(
        "tako-claude-install-{}.{}",
        std::process::id(),
        runner.script_ext
    ));
    let bin = downloader.program().to_string();
    let mut command = std::process::Command::new(&bin);
    // #586: GUI プロセス（dispatch）から到達するのでコンソールウィンドウを出させない
    tako_core::platform::process::no_console_window(&mut command);
    match &downloader {
        Downloader::Curl(_) => {
            command.args(["-fsSL", &plan.source_url, "-o"]).arg(&dest);
        }
        Downloader::Wget(_) => {
            command.args(["-q", &plan.source_url, "-O"]).arg(&dest);
        }
        Downloader::PowerShell(_) => {
            command
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-EncodedCommand",
                    &tako_core::platform::shell::encode_powershell_command(POWERSHELL_FETCH_SCRIPT),
                ])
                .env("TAKO_FETCH_URL", &plan.source_url)
                .env("TAKO_FETCH_DEST", &dest);
        }
    }
    let output = command
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("{bin} を起動できません: {e}"))?;

    if !output.status.success() {
        let _ = std::fs::remove_file(&dest);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        return Err(format!(
            "インストーラを取得できませんでした: {}\n\
             {}\n\
             ネットワーク接続とプロキシ設定を確認してから再実行してください。\n\
             社内プロキシ環境では {} へのアクセス許可が要ることがあります",
            plan.source_url,
            if detail.is_empty() {
                format!("（{bin} が exit {}）", output.status.code().unwrap_or(-1))
            } else {
                format!("（{detail}）")
            },
            plan.source_url,
        ));
    }

    // 先頭だけ見る。`install.ps1` は `param(...)` → `Set-StrictMode` → …の順なので
    // 512 バイトあればどちらの署名も判定できる（実物で確認）
    let head = std::fs::read(&dest)
        .map_err(|e| format!("取得したインストーラを読めません: {e}"))?
        .into_iter()
        .take(512)
        .collect::<Vec<u8>>();
    if !agent_install::looks_like_installer(runner.signature, &head) {
        let _ = std::fs::remove_file(&dest);
        return Err(format!(
            "取得した内容がインストーラではありません（{} が HTML かエラーページを返しています）。\n\
             社内プロキシやネットワーク制限が疑われます。ブラウザで {} を開いて中身を確認するか、\n\
             ネットワークを変えて再実行してください",
            plan.source_url, plan.source_url
        ));
    }
    Ok(dest)
}

/// インストール実行の指定
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InstallOptions {
    /// 実行せず計画だけ返す
    pub dry_run: bool,
    /// 端末があるか。true = 出力を利用者へ流す（進捗が見える・インストーラの TUI が動く）、
    /// false = 捕捉して応答へ載せる（GUI 内 dispatch / MCP から呼ばれる経路）
    pub interactive: bool,
}

/// 取得したインストーラを実行する。
///
/// 端末があるときは出力をそのまま流す（インストーラは進捗と TUI を出す）。
/// GUI 内 dispatch から呼ばれたときは端末が無いので捕捉し、失敗時の診断へ回す
fn run_installer(script: &Path, interactive: bool) -> Result<String, String> {
    let runner = recipe().runner;
    // インタプリタと引数はプラットフォーム境界（B17）が持つデータから組む。
    // ここに `bash` / `powershell` を書かない = 経路の取り違えが起きない
    let shell = runner
        .candidates
        .iter()
        .find_map(|name| tako_core::platform::exe::find(name))
        .unwrap_or_else(|| runner.fallback.to_string());
    let mut command = std::process::Command::new(&shell);
    // #586: GUI プロセスから到達するのでコンソールウィンドウを出させない
    tako_core::platform::process::no_console_window(&mut command);
    command.args(runner.args_for(script));
    let (status, log) = if interactive {
        let status = command
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
            .map_err(|e| format!("{shell} を起動できません: {e}"))?;
        (status, String::new())
    } else {
        let output = command
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("{shell} を起動できません: {e}"))?;
        let mut log = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.trim().is_empty() {
            if !log.is_empty() {
                log.push('\n');
            }
            log.push_str(stderr.trim());
        }
        (output.status, log)
    };
    if status.success() {
        return Ok(log);
    }
    let code = status.code().unwrap_or(-1);
    let detail = if log.trim().is_empty() {
        String::new()
    } else {
        format!("\n{}", log.trim())
    };
    Err(match code {
        // インストーラ自身が説明を出す終了コード
        137 => format!(
            "インストールがメモリ不足で中断されました（exit 137）。\
             空きメモリを増やしてから再実行してください{detail}"
        ),
        c if c >= 128 => format!(
            "インストールが途中で強制終了しました（exit {c}）。\
             もう一度実行するか、上に出ているメッセージを確認してください{detail}"
        ),
        c => format!(
            "インストーラが失敗しました（exit {c}）。上に出ているエラーを確認してください。\
             解決しない場合は https://code.claude.com/docs/en/troubleshoot-install を参照してください{detail}"
        ),
    })
}

// --- PATH 通し ---

/// ランチャーの置き場所を「新しく開いたターミナルが見る PATH」へ通す（冪等）
pub fn ensure_path() -> Result<Value, String> {
    let home = home_dir()?;
    let r = recipe();
    let dir = r.launcher_dir_in(&home);
    if user_path::is_supported() {
        return ensure_user_path(&dir);
    }
    let (shell, _) = shell_target();
    let Some(shell) = shell else {
        let current = std::env::var("SHELL").unwrap_or_default();
        return Err(format!(
            "使っているシェル（{}）の設定ファイルが分かりません。\n\
             次の 1 行をご自身のシェルの設定ファイルへ追加してください:\n  \
             export PATH=\"{}:$PATH\"",
            if current.is_empty() {
                "不明"
            } else {
                &current
            },
            display_path(&dir),
        ));
    };
    let path_var = std::env::var("PATH").unwrap_or_default();
    let outcome = shell_profile::ensure_dir_on_path_in(&home, shell, &dir, Some(&path_var))?;
    // 実際にログインシェルから引けるようになったかを**確かめてから**返す。
    // 「書いたはず」で終わらせない（.zshrc へ書いて届かなかった類の失敗を検出する）
    let verified = outcome.change == PathChange::AlreadyOnPath || login_shell_sees(&dir);
    Ok(json!({
        "shell": shell.as_str(),
        "profile": outcome.profile.display().to_string(),
        "profile_display": display_path(&outcome.profile),
        "dir": dir.display().to_string(),
        "dir_display": display_path(&dir),
        "change": outcome.change.as_str(),
        "wrote": outcome.change.wrote(),
        "verified": verified,
        "note": if verified {
            "新しく開くターミナルから claude コマンドが使えます"
        } else {
            "設定は書きましたが、いまのシェルにはまだ反映されていません。\
             ターミナルを開き直すと有効になります"
        },
    }))
}

/// Windows のユーザー PATH（`HKCU\Environment\Path`）へ通す（冪等。境界 B23）。
///
/// **末尾へ足す**ので、ユーザーが自分で並べた優先順位は動かない。
/// 書いたあと**読み直して確かめる**（「書いたはず」で終わらせない）
fn ensure_user_path(dir: &Path) -> Result<Value, String> {
    let current = user_path::read()?;
    let process_has = user_path::contains_entry(&std::env::var("PATH").unwrap_or_default(), dir);
    let change = match user_path::append_entry(&current.raw, dir) {
        // レジストリに既に在る = 新しいターミナルからは引ける
        None => PathChange::AlreadyOnPath,
        Some(next) => {
            user_path::write(&user_path::UserPathValue {
                raw: next,
                kind: current.kind.clone(),
            })?;
            let after = user_path::read()?;
            if !user_path::contains_entry(&after.raw, dir) {
                return Err(format!(
                    "ユーザー PATH へ {} を追加できませんでした（書き込み後も反映されていません）。\n\
                     設定 → システム → バージョン情報 → 環境変数 から手で追加してください",
                    display_path(dir)
                ));
            }
            PathChange::Installed
        }
    };
    // レジストリへ入っていても**いまのプロセスには反映されない**（Windows は
    // 再ログイン / 新しいプロセス起動まで伝播しない。#525 実測）
    let verified = change == PathChange::AlreadyOnPath || process_has;
    Ok(json!({
        "shell": "windows-user-path",
        // 書き先は「ファイル」ではないので、人へはレジストリのキーを見せる
        "profile": "HKCU\\Environment\\Path",
        "profile_display": "ユーザー環境変数 Path",
        "dir": dir.display().to_string(),
        "dir_display": display_path(dir),
        "change": change.as_str(),
        "wrote": change.wrote(),
        "verified": verified,
        "note": if verified {
            "claude コマンドがどのターミナルからも使えます"
        } else {
            "ユーザー環境変数へ追加しました。いま開いているターミナルには反映されないので、\
             新しいターミナルを開いてください"
        },
    }))
}

/// 置いた PATH ブロックを取り除く（元のバイト列へ戻す）
pub fn undo_path() -> Result<Value, String> {
    let home = home_dir()?;
    if user_path::is_supported() {
        return undo_user_path(&recipe().launcher_dir_in(&home));
    }
    let (shell, _) = shell_target();
    let shell = shell.ok_or("使っているシェルの設定ファイルが分かりません")?;
    let outcome = shell_profile::remove_from_profile_in(&home, shell)?;
    Ok(json!({
        "shell": shell.as_str(),
        "profile": outcome.profile.display().to_string(),
        "change": outcome.change.as_str(),
    }))
}

/// Windows のユーザー PATH からランチャーの置き場所のエントリを取り除く。
/// **他のエントリは 1 つも触らない**（純粋関数側のテストで固定してある）。
///
/// ## unix との非対称（意図的）
///
/// unix はマーカーブロックを目印にするので「tako が置いた分」だけを外せる。
/// レジストリの PATH は 1 本の文字列で目印を置ける場所が無いため、
/// **誰が入れたかに関係なくランチャーの置き場所のエントリを外す**
/// （公式インストーラが入れた分も対象）。`undo-path` は AI / 利用者が
/// 明示的に叩くコマンドで、外した結果は `tako setup bootstrap path` で戻せるので
/// この非対称は受け入れる。目印のための状態ファイルは作らない
/// （#513 の共有カタログへ分類が要るものを、可逆な 1 操作のために増やさない）
fn undo_user_path(dir: &Path) -> Result<Value, String> {
    let current = user_path::read()?;
    let change = match user_path::remove_entry(&current.raw, dir) {
        None => PathChange::Absent,
        Some(next) => {
            user_path::write(&user_path::UserPathValue {
                raw: next,
                kind: current.kind.clone(),
            })?;
            PathChange::Removed
        }
    };
    Ok(json!({
        "shell": "windows-user-path",
        "profile": "HKCU\\Environment\\Path",
        "change": change.as_str(),
    }))
}

// --- 自動導入が通らなかったときの引き継ぎ（#1057）---

/// 引き継ぎ先の候補（**claude 以外**の導入済みエージェント CLI）。
///
/// claude を入れるための代行なので claude 自身は候補にしない。
/// 順序は codex → agy（master を務められる系統を先に置く）。
///
/// # なぜ能力マトリクス（#982）の 1 マスにしないか
///
/// 「この CLI へ導入の代行を頼めるか」を [`crate::agent_support`] のキーにすると
/// **claude が `Unsupported`** になる（claude を入れるための代行なので claude は
/// 対象になりえない）。マトリクスは `claudeは基準系なので全て対応済み` で
/// claude = 全 Supported を強制するため、これは表せない（#1002 が同じ壁に当たった）。
/// なので候補の集合はここで宣言し、テスト（`引き継ぎ先にclaudeを選ばない`）で拘束する
const HANDOFF_AGENTS: &[WorkerAgent] = &[WorkerAgent::Codex, WorkerAgent::Agy];

/// 引き継ぎ先 1 件
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffCandidate {
    pub agent: &'static str,
    pub path: String,
}

/// この環境で引き継げる相手
pub fn handoff_candidates() -> Vec<HandoffCandidate> {
    if legacy_mode() {
        // #1057 前は「案内だけ」で引き継ぎ先を探さなかった
        return Vec::new();
    }
    HANDOFF_AGENTS
        .iter()
        .filter_map(|agent| {
            crate::orchestrator::agent_cli::locate(*agent)
                .ok()
                .map(|path| HandoffCandidate {
                    agent: agent.as_str(),
                    path,
                })
        })
        .collect()
}

/// 代行を頼む指示文（**純粋関数**なので文面をテストで固定できる）。
///
/// 「何を・どこへ・どうやって入れるか」と「やってはいけないこと」を書く。
/// 認証はブラウザ操作が要るので**代行させない**（ユーザーへ依頼させる）
pub fn handoff_prompt_text(plan: &InstallPlan, reason: Option<&str>) -> String {
    let reason = reason.unwrap_or("tako の自動インストールが通らなかった");
    format!(
        "tako から引き継ぎ: Claude Code（claude CLI）の導入を代行してください。\n\
         \n\
         ## 状況\n\
         \n\
         - この環境で tako 自身の自動インストールが成立しませんでした（理由: {reason}）\n\
         - 公式の導入コマンド: {command}\n\
         - コマンドの置き場所: {launcher}\n\
         - 本体の置き場所: {payload}\n\
         - 管理者権限は不要です。ホームディレクトリの中だけで完結します\n\
         \n\
         ## やること（この順に）\n\
         \n\
         1. 上の公式コマンドを実行して claude を導入する。失敗したら出力のエラーを読み、\n\
         \x20  ネットワーク・プロキシ・ディスク容量などの原因を切り分けて対処する\n\
         2. `tako setup bootstrap status --json` で `next_step` を確認する\n\
         \x20  （`install` のままなら導入できていない）\n\
         3. `next_step` が `path` なら `tako setup bootstrap path` を実行する\n\
         4. `next_step` が `auth` なら**ユーザーへ `claude auth login` の実行を依頼する**\n\
         \x20  （ブラウザ操作が要るので代行しない）\n\
         5. `next_step` が `ready` になったら `tako setup` を実行して設定を完了させる\n\
         \n\
         ## 守ること\n\
         \n\
         - 上の手順以外でユーザーの設定ファイル・PATH・レジストリを書き換えない\n\
         - 別の入れ方（Homebrew・npm 等）へ勝手に切り替えない。公式の native インストーラを使う\n\
         - うまくいかないときは、何がどこで失敗したかを日本語で報告して止まる\n",
        command = plan.official_command,
        launcher = display_path(&plan.launcher),
        payload = display_path(&plan.payload),
    )
}

/// 引き継ぎの計画。CLI・MCP が同じ内容を見る（`available` が false なら
/// 従来どおり公式コマンドの案内へ落とす）
pub fn handoff_plan(reason: Option<&str>) -> Result<Value, String> {
    let plan = install_plan()?;
    let candidates = handoff_candidates();
    let prompt = handoff_prompt_text(&plan, reason);
    let recommended = candidates.first();
    Ok(json!({
        "available": !candidates.is_empty(),
        "reason": reason,
        "candidates": candidates
            .iter()
            .map(|c| json!({ "agent": c.agent, "path": c.path }))
            .collect::<Vec<_>>(),
        "recommended": recommended.map(|c| c.agent),
        "prompt": prompt,
        // そのまま打てる形（#322 の最簡形）。引数の prompt はシェルのクォートが
        // 要るので、CLI / GUI は argv で渡す。ここは「誰へ頼むか」を示す
        "launch_command": recommended.map(|c| format!("{} \"<上の prompt>\"", c.agent)),
        "install_plan": plan.to_json(),
        "fallback": format!(
            "引き継げる別のエージェント CLI がありません。\n\
             次のコマンドを自分で実行してから `tako setup` をやり直してください:\n  {}",
            plan.official_command
        ),
    }))
}

// --- 依存ツールと Homebrew ---

/// 依存の検出は [`crate::setup_deps`] が正本（CLI・MCP・`--review` が同じ実装を通る）。
/// ここは `status()` の応答へ載せるだけ
fn deps_json() -> Value {
    crate::setup_deps::status_json()
}

/// Homebrew の状態と、無い場合の案内。
///
/// **自動導入はしない**。Homebrew のインストーラは sudo でパスワードを求める
/// （実物で確認: install.sh に sudo 参照 49 箇所・`have_sudo_access`）。
/// setup が黙って権限昇格を走らせるべきではないうえ、brew で入れる依存は
/// すべて任意なので、無くてもゼロスタートは完走できる
fn homebrew_json() -> Value {
    let brew = tako_core::platform::exe::find("brew");
    json!({
        "found": brew,
        "auto_install": false,
        "reason": "Homebrew のインストーラは管理者パスワードを求めるため、tako は代行しません",
        "guidance": "https://brew.sh の手順を実行すると、tmux などの任意依存を導入できるようになります",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 取得物の見分け方は境界（B17）が持つ。ここではこの環境の署名で
    /// **HTML エラーページを弾けること**だけを確かめる
    /// （判定そのものの総当たりは `platform::agent_install` 側のテスト）
    #[test]
    fn この環境の署名でhtmlエラーページを弾く() {
        let signature = recipe().runner.signature;
        assert!(!agent_install::looks_like_installer(
            signature,
            b"<!DOCTYPE html>"
        ));
        assert!(!agent_install::looks_like_installer(signature, b""));
    }

    #[test]
    fn 段は導入状況から一意に決まる() {
        // Step の順序（Install → Path → Auth → Ready）を固定する
        assert_eq!(Step::Install.as_str(), "install");
        assert_eq!(Step::Path.as_str(), "path");
        assert_eq!(Step::Auth.as_str(), "auth");
        assert_eq!(Step::Ready.as_str(), "ready");
        for step in [Step::Install, Step::Path, Step::Auth, Step::Ready] {
            assert!(!step.describe().is_empty());
        }
    }

    /// 受け入れ条件 4: 実行前に「何をどこに入れるか」が必ず出ること
    #[test]
    fn 導入計画は何をどこに入れるかを必ず含む() {
        use tako_core::platform::support::Platform;
        // **両プラットフォームぶんを macOS から検証する**（#515 / #920）。
        // リテラルを書かず「その計画の中身が行に出ているか」を見るので、
        // 手順が変わってもテストがずれない
        let home = Path::new("/tmp/h");
        for platform in [Platform::MacOs, Platform::Windows] {
            let r = agent_install::recipe(platform, AgentKind::Claude);
            let plan = InstallPlan {
                agent: r.agent.as_str(),
                official_command: r.source.official_command.to_string(),
                source_url: r.source.url.to_string(),
                launcher: r.launcher_path_in(home),
                payload: r.payload_dir_in(home),
                auto_updates: r.auto_updates,
                can_run: r.tako_can_run,
            };
            // 置き場所の表示は**実行中の OS** の区切りになる（`launcher_path_in` は
            // `PathBuf`）。`launcher_rel` / `payload_rel` は `/` 区切りの静的文字列なので
            // 突き合わせる前に寄せる。これを忘れると Windows でだけ落ちる（#920 の原因）
            let lines = plan.lines().join("\n").replace('\\', "/");
            assert!(
                lines.contains(r.source.official_command),
                "{platform:?}: 公式コマンドが出ていない: {lines}"
            );
            assert!(
                lines.contains(r.launcher_rel),
                "{platform:?}: 置き場所が出ていない: {lines}"
            );
            assert!(
                lines.contains(r.payload_rel),
                "{platform:?}: 本体の置き場所が出ていない: {lines}"
            );
            assert!(
                lines.contains("管理者権限"),
                "{platform:?}: 権限の説明が無い: {lines}"
            );
            // **他方の手順が混ざっていない**（計画がプラットフォームに依らなくなる
            // 退行の検出。`install.sh` と `install.ps1` は互いに排他）
            let other = agent_install::recipe(
                match platform {
                    Platform::MacOs => Platform::Windows,
                    Platform::Windows => Platform::MacOs,
                },
                AgentKind::Claude,
            );
            assert!(
                !lines.contains(other.source.official_command),
                "{platform:?}: 別プラットフォームの手順が混ざっている: {lines}"
            );
            let json = plan.to_json();
            assert_eq!(json["can_run"], r.tako_can_run);
            assert!(json["lines"].as_array().is_some_and(|a| a.len() >= 5));
        }
    }

    /// Windows も PowerShell 経路で代行する（#1057。実機実測を経て倒した）
    #[test]
    fn windowsもpowershell経路で代行する() {
        let r = agent_install::recipe(
            tako_core::platform::support::Platform::Windows,
            AgentKind::Claude,
        );
        assert!(r.tako_can_run);
        assert!(r.source.official_command.contains("install.ps1"));
    }

    #[test]
    fn 依存はすべて任意でhomebrewは自動導入しない() {
        let brew = homebrew_json();
        assert_eq!(brew["auto_install"], false);
        assert!(brew["reason"]
            .as_str()
            .is_some_and(|s| s.contains("パスワード")));
        for dep in deps_json().as_array().unwrap() {
            assert_eq!(dep["required"], false, "必須の依存を増やさない");
        }
    }

    /// 引き継ぎの指示文には「何を・どこへ・どうやって」と禁止事項が入る（#1057）。
    /// **両プラットフォームぶんを macOS から検証する**
    #[test]
    fn 引き継ぎの指示文は導入計画から作られる() {
        use tako_core::platform::support::Platform;
        let home = Path::new("/tmp/h");
        for platform in [Platform::MacOs, Platform::Windows] {
            let r = agent_install::recipe(platform, AgentKind::Claude);
            let plan = InstallPlan {
                agent: r.agent.as_str(),
                official_command: r.source.official_command.to_string(),
                source_url: r.source.url.to_string(),
                launcher: r.launcher_path_in(home),
                payload: r.payload_dir_in(home),
                auto_updates: r.auto_updates,
                can_run: r.tako_can_run,
            };
            let prompt = handoff_prompt_text(&plan, Some("取得が 403 で失敗した"));
            assert!(prompt.contains(r.source.official_command), "{prompt}");
            assert!(prompt.contains("取得が 403 で失敗した"), "{prompt}");
            // 認証は代行させない（ブラウザ操作が要る）
            assert!(prompt.contains("claude auth login"), "{prompt}");
            assert!(prompt.contains("代行しない"), "{prompt}");
            // 次の一手が機械的に辿れる形で入っている
            assert!(prompt.contains("tako setup bootstrap status"), "{prompt}");
            assert!(prompt.contains("tako setup bootstrap path"), "{prompt}");
            // 勝手な入れ方への切り替えを禁じる
            assert!(prompt.contains("Homebrew"), "{prompt}");
            // 他方の手順が混ざらない
            let other = agent_install::recipe(
                match platform {
                    Platform::MacOs => Platform::Windows,
                    Platform::Windows => Platform::MacOs,
                },
                AgentKind::Claude,
            );
            assert!(
                !prompt.contains(other.source.official_command),
                "{platform:?}: 別プラットフォームの手順が混ざっている: {prompt}"
            );
        }
    }

    /// 引き継ぎ先に claude 自身を選ばない（入れる対象なので候補になりえない）
    #[test]
    fn 引き継ぎ先にclaudeを選ばない() {
        assert!(!HANDOFF_AGENTS.contains(&WorkerAgent::Claude));
        // master を務められる系統を先に置く（codex → agy）
        assert_eq!(HANDOFF_AGENTS.first(), Some(&WorkerAgent::Codex));
        for candidate in handoff_candidates() {
            assert_ne!(candidate.agent, "claude");
        }
    }

    /// 候補が居なければ従来の案内（公式コマンド）へ落ちる
    #[test]
    fn 候補が居なければ公式コマンドの案内へ落ちる() {
        let _guard = crate::orchestrator::agent_cli::test_force_missing(&[
            WorkerAgent::Codex,
            WorkerAgent::Agy,
        ]);
        let plan = handoff_plan(Some("テスト")).expect("計画は作れる");
        assert_eq!(plan["available"], false);
        assert!(plan["candidates"].as_array().is_some_and(|a| a.is_empty()));
        assert_eq!(plan["recommended"], serde_json::Value::Null);
        let fallback = plan["fallback"].as_str().unwrap_or_default();
        assert!(
            fallback.contains(recipe().source.official_command),
            "{fallback}"
        );
    }

    /// 候補が居れば「誰へ頼むか」が決まる
    #[test]
    fn 候補が居れば引き継ぎ先が決まる() {
        let _guard = crate::orchestrator::agent_cli::test_force_found(&[(
            WorkerAgent::Codex,
            "/tmp/fake/codex",
        )]);
        let plan = handoff_plan(None).expect("計画は作れる");
        assert_eq!(plan["available"], true);
        assert_eq!(plan["recommended"], "codex");
        assert!(plan["prompt"].as_str().is_some_and(|p| !p.is_empty()));
    }
}
