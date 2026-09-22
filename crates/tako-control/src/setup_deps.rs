//! 任意依存ツールの検出とその場導入（#88 → #262 で失われ #1057 で復活）
//!
//! ## 経緯（これが無いと同じ退行を繰り返す）
//!
//! #88 は `tako setup` の依存チェックに「今すぐ `brew install tmux` を実行しますか？」の
//! その場導入を入れた。その後 #262 が標準 setup を**質問ゼロ**へ変えたとき、
//! 呼び出しが `run_dependency_check(false)` 固定になり、
//! **その場導入の経路は 1 度も通らない死んだコードになった**（#1057 の棚卸しで判明）。
//!
//! 同じことを繰り返さないために、導入の判断と実行を**ここ 1 か所**へ集約して
//! CLI・MCP・`--review` が同じ実装を通るようにする。UI から到達できない
//! 経路を作らないのが tako の開発不変条件（設計原則 5）。
//!
//! ## #262 との両立（#1499 で線を引き直した）
//!
//! #1057 が復活させたのは **`--review` 経路だけ**だった（標準 `tako setup` は
//! `run_dependency_check(false)` のまま）。実機の利用者は `--review` を付けないので
//! **検出止まりのまま**で、#88 の体験は事実上戻っていなかった（#1499 の実測）。
//!
//! #262 の「質問ゼロ」が守っているのは**設定値の個別確認**（前回値の再確認・
//! プロファイルの選び直し）であって、「無い物をいま入れるか」はそれに当たらない。
//! 無いものは入れない限り機能が欠けたままで、**利用者が次に打つコマンドを
//! 自分で考える必要がある**（#322 の「最も簡単なコマンド」の逆）。そこで:
//!
//! - 標準 `tako setup`: 未検出の任意依存だけは「何を・どの導入器で・どこへ」を
//!   見せて `[y/N]` を聞く（#1499）。**設定値の質問は増やさない**
//! - `--yes`: 同意扱いで導入まで進む（`-y` の一般的な意味。bootstrap 段が
//!   `--yes` で claude を実際に入れる先例と体験を揃える）
//! - 非 TTY（GUI の初回起動・パイプ経由）: 聞かずに案内 1 本へ落として**止まらない**
//! - `tako setup --check`: 読み取りだけ。何も導入しない
//! - `tako setup deps install`: 明示コマンド（非対話。`--dry-run` で計画だけ）
//! - MCP `tako_setup_deps`: 上と同じ実装を通る
//!
//! 判断は [`offer_for`] の 1 本（**理由を返す純粋関数**）に閉じてあり、CLI は
//! 読んで表示するだけ。A/B は `TAKO_1499_LEGACY=1`（標準 setup が聞かない
//! = #1499 前の挙動）

use serde_json::{json, Value};
use tako_core::platform::support::Platform;

/// 導入手段。**tako が実行を代行してよいか**は手段ごとに決まる
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepInstaller {
    /// Homebrew（macOS）
    Brew { pkg: &'static str },
    /// winget（Windows）。**案内だけ**（実機で実測していない手順を黙って走らせない）
    Winget { pkg: &'static str },
}

impl DepInstaller {
    /// 実行ファイル名
    pub fn program(self) -> &'static str {
        match self {
            Self::Brew { .. } => "brew",
            Self::Winget { .. } => "winget",
        }
    }

    /// 実行する引数（`program` の後ろ）
    pub fn args(self) -> Vec<String> {
        match self {
            Self::Brew { pkg } => vec!["install".into(), pkg.into()],
            Self::Winget { pkg } => vec![
                "install".into(),
                "--id".into(),
                pkg.into(),
                "--accept-source-agreements".into(),
                "--accept-package-agreements".into(),
            ],
        }
    }

    /// 人へ見せる 1 行（#322 の最簡形）
    pub fn command_line(self) -> String {
        format!("{} {}", self.program(), self.args().join(" "))
    }

    /// tako が実行を代行してよいか。false = 手順を案内するだけ
    pub fn tako_can_run(self) -> bool {
        match self {
            Self::Brew { .. } => true,
            // Windows のパッケージ導入は実機で通しの実測をしてから代行する（#525）
            Self::Winget { .. } => false,
        }
    }
}

/// tako が実行時に使う外部コマンド 1 件
#[derive(Debug, Clone, Copy)]
pub struct ExternalDep {
    /// 探す実行ファイル名（**この環境での名前**。Windows の器は psmux）
    pub bin: &'static str,
    /// 必須依存か（false = 任意。無くても tako 自体は動く）
    pub required: bool,
    /// 影響する機能
    pub purpose: &'static str,
    /// 導入手段（None = 手段が無いので案内のみ）
    pub installer: Option<DepInstaller>,
    /// 手段が使えないときの案内
    pub hint: &'static str,
}

/// そのプラットフォームの依存表（**純粋関数**なので macOS から Windows 側も検証できる）
pub fn deps(platform: Platform) -> Vec<ExternalDep> {
    let container = match platform {
        Platform::MacOs => ExternalDep {
            bin: "tmux",
            required: false,
            purpose: "リモート接続（tako remote）・再起動時のセッション完全復元・オーケストレーターの worker 管理",
            installer: Some(DepInstaller::Brew { pkg: "tmux" }),
            hint: "https://github.com/tmux/tmux/wiki/Installing",
        },
        // Windows の器は psmux（tmux 互換 CLI の別実装）。**`tmux` を探しても見つからない**
        Platform::Windows => ExternalDep {
            bin: "psmux",
            required: false,
            purpose: "再起動時のセッション完全復元・オーケストレーターの worker 管理",
            installer: Some(DepInstaller::Winget {
                pkg: "marlocarlo.psmux",
            }),
            hint: "https://github.com/marlocarlo/psmux",
        },
    };
    let git = ExternalDep {
        bin: "git",
        required: false,
        purpose: "git パネル（ブランチ・コミットグラフ・diff 表示）",
        installer: match platform {
            Platform::MacOs => Some(DepInstaller::Brew { pkg: "git" }),
            Platform::Windows => Some(DepInstaller::Winget { pkg: "Git.Git" }),
        },
        hint: match platform {
            Platform::MacOs => "xcode-select --install でも導入できます",
            Platform::Windows => "https://git-scm.com/download/win",
        },
    };
    let tailscale = ExternalDep {
        bin: "tailscale",
        required: false,
        purpose: "スマホからのリモート接続（tako remote。WireGuard E2E 暗号化）",
        installer: match platform {
            Platform::MacOs => Some(DepInstaller::Brew { pkg: "tailscale" }),
            Platform::Windows => Some(DepInstaller::Winget {
                pkg: "tailscale.tailscale",
            }),
        },
        hint: match platform {
            Platform::MacOs => "App Store で「Tailscale」を検索、または brew install tailscale",
            Platform::Windows => "https://tailscale.com/download/windows",
        },
    };
    vec![container, git, tailscale]
}

/// この環境の依存表
pub fn current_deps() -> Vec<ExternalDep> {
    let mut deps = deps(tako_core::platform::agent_install::current_platform());
    if let Some(bin) = test_required_dep() {
        for dep in deps.iter_mut() {
            if dep.bin == bin {
                dep.required = true;
            }
        }
    }
    deps
}

/// **テスト専用**: 表の 1 件を必須依存へ倒す（`TAKO_1501_TEST_REQUIRED_DEP=<bin>`）。
///
/// [`deps`] は現状すべて `required: false` なので、「必須依存が欠けている」経路を
/// 実機で作れない（実際に必須へ載るのは「エージェント CLI が 1 つも無い」の合成項目
/// だけ）。#1501 の受け入れ検査がその経路を**実走で**確かめるための逃げ道。
///
/// 認可のゲートではないので、誰が指定しても起きるのは**表示と残り作業が 1 行増える**
/// ことだけ（#1501 以降、必須依存の不足で setup は止まらない）。表に無い名前は無視する
fn test_required_dep() -> Option<String> {
    std::env::var("TAKO_1501_TEST_REQUIRED_DEP")
        .ok()
        .filter(|value| !value.is_empty())
}

/// 1 件ぶんの検出結果
#[derive(Debug, Clone)]
pub struct DepStatus {
    pub dep: ExternalDep,
    /// 解決できた実行ファイル
    pub found: Option<String>,
    /// 導入器の実行ファイル。**検出のときに 1 回だけ引く**（unix の
    /// `exe::find` はログインシェルを起こすので、依存ごとに何度も引くと
    /// `tako setup` が目に見えて遅くなる。表示・判断・実行が同じ値を見る）
    installer_found: Option<String>,
}

impl DepStatus {
    pub fn to_json(&self) -> Value {
        let installer = self.dep.installer;
        json!({
            "bin": self.dep.bin,
            "found": self.found,
            "required": self.dep.required,
            "purpose": self.dep.purpose,
            "install_command": installer.map(DepInstaller::command_line),
            // 手段があっても代行できないことがある（Windows の winget）
            "can_run": self.can_tako_install(),
            "installer_found": self.installer_path(),
            "install_dir": self
                .installer_path()
                .and_then(|program| installer.and_then(|i| install_dir(i, program))),
            "hint": self.dep.hint,
        })
    }

    /// tako がこの依存の導入を代行できるか（**JSON の `can_run` と同じ 1 実装**）。
    ///
    /// 手段があること・その手段を代行してよいこと・導入器がこの環境に在ることの
    /// 3 つが揃って初めて true。`tako setup` の `[y/N]` を出すかもこれを見る（#1499）
    pub fn can_tako_install(&self) -> bool {
        self.dep.installer.is_some_and(DepInstaller::tako_can_run) && self.installer_found.is_some()
    }

    /// 導入器の実行ファイル（この環境で解決できたもの）
    pub fn installer_path(&self) -> Option<&str> {
        self.installer_found.as_deref()
    }
}

/// 導入したコマンドが**どこへ置かれるか**（#1499 の「何をどこに入れるか」の一部）。
///
/// Homebrew は `<prefix>/bin/brew` に在り、パッケージの実行ファイルは同じ
/// `<prefix>/bin` へ symlink される。**導入器のパスから構造的に導ける**ので
/// `brew --prefix` を別プロセスで起こさない。winget は置き場がパッケージ任せ
/// なので答えない（**推測を人へ見せない**）
pub fn install_dir(installer: DepInstaller, program_path: &str) -> Option<String> {
    match installer {
        DepInstaller::Brew { .. } => {
            let parent = std::path::Path::new(program_path).parent()?;
            // `<prefix>/bin/brew` の形でないもの（スタブ・別配置）は答えない
            (parent.file_name()? == "bin").then(|| parent.to_string_lossy().into_owned())
        }
        DepInstaller::Winget { .. } => None,
    }
}

/// `tako setup` の依存チェック段で、未検出の依存 1 件に対して何をするか（#1499）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepOffer {
    /// 端末で「インストールしますか？ [y/N]」を聞いてから導入する
    Ask,
    /// 質問を省いてそのまま導入する（`--yes`）
    AutoInstall,
    /// 導入せず案内だけ出して続行する（**理由つき**。黙って飛ばさない）
    Guide(GuideReason),
}

/// 聞かずに案内で終える理由。**すべて人へ出せる文面を持つ**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuideReason {
    /// 手段が無い / tako が代行しない / 導入器がこの環境に無い
    CannotRun,
    /// 読み取りだけの経路（`tako setup --check`）
    CheckOnly,
    /// `TAKO_1499_LEGACY=1`（#1499 前 = 標準 setup は聞かない）
    Legacy,
    /// 端末が無いので聞けない（GUI の初回起動・パイプ経由）
    NoTerminal,
}

impl GuideReason {
    /// 機械可読の理由（`skipped[].reason` と同じ語彙を使う）
    pub fn slug(self) -> &'static str {
        match self {
            Self::CannotRun => "cannot_run",
            Self::CheckOnly => "check_only",
            Self::Legacy => "legacy",
            Self::NoTerminal => "no_terminal",
        }
    }

    /// 「聞けたはずなのに聞かなかった」ときだけ理由を人へ出す
    /// （`CannotRun` / `CheckOnly` は直前の表示が既に理由を語っている）
    pub fn note(self) -> Option<&'static str> {
        match self {
            Self::CannotRun | Self::CheckOnly => None,
            Self::Legacy => Some("（TAKO_1499_LEGACY=1 のため確認を省きました）"),
            Self::NoTerminal => Some("（端末が無いので確認を省きました）"),
        }
    }
}

/// [`offer_for`] が見る文脈。**この構造体の外の条件で判断を分けない**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepOfferContext {
    /// この段が導入まで行う経路か（`tako setup --check` は false）
    pub stage_installs: bool,
    /// `--review`（#1057 からある見直し経路。#1499 前はここだけが聞いた）
    pub review: bool,
    /// `--yes`（質問を省いて導入まで進む）
    pub assume_yes: bool,
    /// stdin が端末か
    pub stdin_is_terminal: bool,
    /// `TAKO_1499_LEGACY=1`
    pub legacy: bool,
}

/// 未検出の依存 1 件をその場でどう扱うか（**純粋関数**なので文脈を並べて検査できる）。
///
/// `can_run` は [`DepStatus::can_tako_install`]。判断の順序に意味がある:
/// 代行できないものは他の条件を見るまでもなく案内で終わり、読み取り専用の経路は
/// `--yes` より強い（`--check` が何かを入れたら読み取りではない）
pub fn offer_for(can_run: bool, ctx: DepOfferContext) -> DepOffer {
    if !can_run {
        return DepOffer::Guide(GuideReason::CannotRun);
    }
    if !ctx.stage_installs {
        return DepOffer::Guide(GuideReason::CheckOnly);
    }
    // #1499 前へ戻す A/B。`--review` はもともと聞いていたので据え置く
    if ctx.legacy && !ctx.review {
        return DepOffer::Guide(GuideReason::Legacy);
    }
    if ctx.assume_yes {
        return DepOffer::AutoInstall;
    }
    if !ctx.stdin_is_terminal {
        return DepOffer::Guide(GuideReason::NoTerminal);
    }
    DepOffer::Ask
}

/// #1499 の A/B（標準 `tako setup` が依存の導入を聞かない = 復活前の挙動）
pub fn legacy_mode() -> bool {
    std::env::var_os("TAKO_1499_LEGACY").is_some()
}

/// 器（tmux / psmux）は PATH の名前だけでは決まらない。
///
/// psmux は `psmux.exe` / `pmux.exe` / `tmux.exe` の 3 本を配り、
/// `TAKO_PSMUX_BIN` で明示指定もできる（#519 / #881）。**tako が実際に器として
/// 使うもの**を答えないと、動いているのに「見つかりません」と言ってしまう
fn resolve_container() -> Option<String> {
    match tako_core::backend::binary() {
        tako_core::backend::Binary::Tmux { bin } => Some(bin.clone()),
        tako_core::backend::Binary::Psmux { bin, .. } => Some(bin.clone()),
        tako_core::backend::Binary::Absent => None,
    }
}

/// 依存 1 件を解決する。**検出と導入後の確認が同じ規則を通る**
/// （別々にすると「入れたのに見つかりません」と言い出す側が生まれる）。
///
/// 器は `exe::find` を先に見る: `backend::binary()` はプロセス内で 1 回だけ
/// 解決してキャッシュするので、導入直後の再確認では答えが変わらない。
///
/// **`tako setup` の再検出もこれを通す**（#1499）。`exe::find` だけで確かめると
/// 器の別名・`TAKO_TMUX_BIN` 指定を取りこぼして「入れたのに見つかりません」になる
pub fn resolve(dep: &ExternalDep) -> Option<String> {
    let found = tako_core::platform::exe::find(dep.bin);
    if found.is_some() || !is_container(dep) {
        return found;
    }
    resolve_container()
}

/// 依存の検出（読み取りだけ）
pub fn status() -> Vec<DepStatus> {
    current_deps()
        .into_iter()
        .map(|dep| {
            let found = resolve(&dep);
            let installer_found = dep
                .installer
                .and_then(|i| tako_core::platform::exe::find(i.program()));
            DepStatus {
                dep,
                found,
                installer_found,
            }
        })
        .collect()
}

/// その依存が「永続化の器」か（名前の解決規則が他と違う）
fn is_container(dep: &ExternalDep) -> bool {
    matches!(dep.bin, "tmux" | "psmux")
}

/// 検出結果の JSON（`tako_setup_bootstrap` の `deps` と同じ形を保つ）
pub fn status_json() -> Value {
    Value::Array(status().iter().map(DepStatus::to_json).collect())
}

/// 導入実行の指定
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DepInstallOptions {
    /// 実行せず計画だけ返す
    pub dry_run: bool,
    /// 端末があるか。true = 出力を利用者へ流す
    pub interactive: bool,
}

/// 未導入の依存を導入する。`bin` 省略で**未導入のものすべて**。
///
/// 導入済みのものは触らない（**冪等**）。手段が無い / 代行できないものは
/// 実行せず理由つきで `skipped` に載せる（黙って飛ばさない）
pub fn install(bin: Option<&str>, opts: DepInstallOptions) -> Result<Value, String> {
    let all = status();
    if let Some(bin) = bin {
        if !all.iter().any(|s| s.dep.bin == bin) {
            return Err(format!(
                "不明な依存: {bin:?}（この環境の対象は {}）",
                all.iter()
                    .map(|s| s.dep.bin)
                    .collect::<Vec<_>>()
                    .join(" / ")
            ));
        }
    }
    let mut installed = Vec::new();
    let mut skipped = Vec::new();
    let mut planned = Vec::new();
    for state in &all {
        if bin.is_some_and(|b| b != state.dep.bin) {
            continue;
        }
        if let Some(path) = &state.found {
            skipped.push(json!({
                "bin": state.dep.bin,
                "reason": "already_installed",
                "detail": format!("{} は導入済みです（{path}）", state.dep.bin),
            }));
            continue;
        }
        let Some(installer) = state.dep.installer else {
            skipped.push(json!({
                "bin": state.dep.bin,
                "reason": "no_installer",
                "detail": format!("自動導入の手段がありません: {}", state.dep.hint),
            }));
            continue;
        };
        if !installer.tako_can_run() {
            skipped.push(json!({
                "bin": state.dep.bin,
                "reason": "not_delegable",
                "detail": format!(
                    "この環境では tako が導入を代行しません。次のコマンドを実行してください: {}",
                    installer.command_line()
                ),
            }));
            continue;
        }
        let Some(program) = state.installer_path() else {
            skipped.push(json!({
                "bin": state.dep.bin,
                "reason": "installer_missing",
                "detail": format!(
                    "{} が見つかりません。{} を導入するか、{} を参照してください",
                    installer.program(),
                    installer.program(),
                    state.dep.hint
                ),
            }));
            continue;
        };
        planned.push(json!({
            "bin": state.dep.bin,
            "command": installer.command_line(),
        }));
        if opts.dry_run {
            continue;
        }
        run_installer(program, installer, opts.interactive)?;
        // 「実行した」ではなく「引けるようになった」を確かめてから成功と言う
        match resolve(&state.dep) {
            Some(path) => installed.push(json!({ "bin": state.dep.bin, "path": path })),
            None => {
                return Err(format!(
                    "{} は正常終了しましたが {} が見つかりません。\n\
                     ターミナルを開き直してから `tako setup deps` で確認してください",
                    installer.command_line(),
                    state.dep.bin
                ))
            }
        }
    }
    Ok(json!({
        "performed": !opts.dry_run,
        "planned": planned,
        "installed": installed,
        "skipped": skipped,
        "deps": status_json(),
    }))
}

fn run_installer(program: &str, installer: DepInstaller, interactive: bool) -> Result<(), String> {
    let mut command = std::process::Command::new(program);
    // #586: GUI 内 dispatch から到達するのでコンソールウィンドウを出させない
    tako_core::platform::process::no_console_window(&mut command);
    command.args(installer.args());
    let (status, log) = if interactive {
        let status = command
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
            .map_err(|e| format!("{program} を起動できません: {e}"))?;
        (status, String::new())
    } else {
        let output = command
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("{program} を起動できません: {e}"))?;
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
        return Ok(());
    }
    let detail = if log.trim().is_empty() {
        String::new()
    } else {
        format!("\n{}", log.trim())
    };
    Err(format!(
        "{} が失敗しました（exit {}）。上に出ているエラーを確認してください{detail}",
        installer.command_line(),
        status.code().unwrap_or(-1)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 依存はすべて任意() {
        for platform in [Platform::MacOs, Platform::Windows] {
            for dep in deps(platform) {
                assert!(
                    !dep.required,
                    "{platform:?}: 必須の依存を増やさない（{}）",
                    dep.bin
                );
                assert!(!dep.purpose.is_empty(), "{} の purpose が空", dep.bin);
                assert!(!dep.hint.is_empty(), "{} の hint が空", dep.bin);
            }
        }
    }

    /// **プラットフォームごとに正しい名前と手段**（macOS から両方を検証する）。
    /// Windows で `tmux` を探すと器が動いていても見つからない（#519 / #881）
    #[test]
    fn 器の名前と導入手段はプラットフォームで変わる() {
        let mac = deps(Platform::MacOs);
        assert_eq!(mac[0].bin, "tmux");
        assert_eq!(
            mac[0].installer,
            Some(DepInstaller::Brew { pkg: "tmux" }),
            "macOS は brew"
        );

        let win = deps(Platform::Windows);
        assert_eq!(win[0].bin, "psmux", "Windows の器は psmux");
        assert!(
            matches!(win[0].installer, Some(DepInstaller::Winget { .. })),
            "Windows は winget"
        );
        // 別プラットフォームの手段が混ざっていない
        for dep in &win {
            assert!(
                !matches!(dep.installer, Some(DepInstaller::Brew { .. })),
                "{}: Windows に brew の案内が出ている",
                dep.bin
            );
        }
        for dep in &mac {
            assert!(
                !matches!(dep.installer, Some(DepInstaller::Winget { .. })),
                "{}: macOS に winget の案内が出ている",
                dep.bin
            );
        }
    }

    /// 代行してよいのは実測済みの手段だけ（#525 / #868 と同じ基準）
    #[test]
    fn 未実測の手段は代行しない() {
        assert!(DepInstaller::Brew { pkg: "tmux" }.tako_can_run());
        assert!(!DepInstaller::Winget {
            pkg: "marlocarlo.psmux"
        }
        .tako_can_run());
    }

    #[test]
    fn コマンド行は最簡形() {
        assert_eq!(
            DepInstaller::Brew { pkg: "tmux" }.command_line(),
            "brew install tmux"
        );
        // winget は非対話で通す引数が要る（人が打つときも同じ形になる）
        let winget = DepInstaller::Winget {
            pkg: "marlocarlo.psmux",
        };
        assert!(winget
            .command_line()
            .starts_with("winget install --id marlocarlo.psmux"));
    }

    /// 「どこへ入るか」は Homebrew の構造から導く（`brew --prefix` を起こさない）
    #[test]
    fn 入る場所は導入器のパスから導く() {
        let brew = DepInstaller::Brew { pkg: "tmux" };
        assert_eq!(
            install_dir(brew, "/opt/homebrew/bin/brew").as_deref(),
            Some("/opt/homebrew/bin")
        );
        assert_eq!(
            install_dir(brew, "/usr/local/bin/brew").as_deref(),
            Some("/usr/local/bin")
        );
        // `<prefix>/bin/brew` の形でないものは**答えない**（推測を人へ見せない）
        assert_eq!(install_dir(brew, "/tmp/stub/brew"), None);
        assert_eq!(install_dir(brew, "brew"), None);
        // winget は置き場がパッケージ任せなので答えない
        assert_eq!(
            install_dir(DepInstaller::Winget { pkg: "Git.Git" }, "C:\\w\\winget.exe"),
            None
        );
    }

    /// `can_run`（JSON）と `[y/N]` を出すかの判断が**同じ 1 実装**を見る（#1499）
    #[test]
    fn can_runの判断は1実装() {
        for state in status() {
            let json = state.to_json();
            assert_eq!(
                json["can_run"].as_bool(),
                Some(state.can_tako_install()),
                "{}: JSON の can_run と can_tako_install がずれている",
                state.dep.bin
            );
            assert_eq!(
                json["installer_found"].as_str(),
                state.installer_path(),
                "{}: 導入器のパスがずれている",
                state.dep.bin
            );
        }
    }

    #[test]
    fn 不明な依存名は拒否する() {
        let err = install(Some("nosuchtool"), DepInstallOptions::default())
            .expect_err("不明な名前は拒否する");
        assert!(err.contains("nosuchtool"), "{err}");
        // 対象の一覧を必ず添える（次の一手が分かる）
        for dep in current_deps() {
            assert!(err.contains(dep.bin), "{err}");
        }
    }

    /// dry_run は 1 つも実行しない（計画だけ返る）
    #[test]
    fn dry_runは実行しない() {
        let value = install(
            None,
            DepInstallOptions {
                dry_run: true,
                interactive: false,
            },
        )
        .expect("計画は作れる");
        assert_eq!(value["performed"], false);
        assert!(value["installed"].as_array().is_some_and(|a| a.is_empty()));
    }

    /// 導入済みのものは触らない（冪等）
    #[test]
    fn 導入済みは触らない() {
        let value = install(
            None,
            DepInstallOptions {
                dry_run: true,
                interactive: false,
            },
        )
        .expect("計画は作れる");
        let planned: Vec<String> = value["planned"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p["bin"].as_str().map(String::from))
            .collect();
        for state in status() {
            if state.found.is_some() {
                assert!(
                    !planned.contains(&state.dep.bin.to_string()),
                    "{} は導入済みなのに計画に入っている",
                    state.dep.bin
                );
            }
        }
    }
}
