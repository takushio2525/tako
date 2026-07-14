//! `tako setup` — 対話式セットアップコマンド。
//!
//! 依存ツールチェック（claude 必須 / tmux・cloudflared・git 任意。未導入は brew で
//! その場インストール可）→ MCP 登録確認 → リソースファイル書き出し →
//! config.yaml の初回/2回目判定 + アップデート追従（Issue #94）→
//! claude を setup cwd で起動する。IPC 不要（tako アプリ未起動でも動作）。
//!
//! config.yaml のスキーマと setup changelog は `tako_control::setup` にある
//! （MCP `tako_setup_changes` と共有。二重実装を作らない）。

use std::path::{Path, PathBuf};
use tako_control::setup::{load_config, pending_changes, ChangeKind, SetupChange, CHANGES_YAML};

// --- バイナリ埋め込みリソース ---

const SYSTEM_PROMPT: &str = include_str!("../../../resources/setup/system-prompt.md");
const TPL_00_LANGUAGE: &str =
    include_str!("../../../resources/setup/templates/sections/00-language.md");
const TPL_01_INTERACTION: &str =
    include_str!("../../../resources/setup/templates/sections/01-interaction-style.md");
const TPL_02_GIT: &str =
    include_str!("../../../resources/setup/templates/sections/02-git-workflow.md");
const TPL_03_CODE: &str =
    include_str!("../../../resources/setup/templates/sections/03-code-quality.md");
const TPL_04_SAFETY: &str =
    include_str!("../../../resources/setup/templates/sections/04-safety-rules.md");
const TPL_05_PROPOSAL: &str =
    include_str!("../../../resources/setup/templates/sections/05-proposal-quality.md");
const TPL_06_VERIFICATION: &str =
    include_str!("../../../resources/setup/templates/sections/06-completion-verification.md");
const CONFIG_DEFAULT: &str = include_str!("../../../resources/setup/templates/config-default.yaml");

// --- パスユーティリティ ---

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

fn setup_dir() -> Result<PathBuf, String> {
    home_dir()
        .map(|h| h.join("Library/Application Support/tako/setup"))
        .ok_or_else(|| "ホームディレクトリが取得できない（$HOME 未設定）".into())
}

// --- 環境チェック ---

fn login_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".into())
}

/// ログインシェル経由でコマンドを探す（GUI 起動や Homebrew の PATH 差異に対応）
fn find_command(name: &str) -> Option<String> {
    let output = std::process::Command::new(login_shell())
        .args(["-l", "-c", &format!("command -v {name}")])
        .output()
        .ok()?;
    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            return Some(path);
        }
    }
    None
}

// --- 依存ツールチェック ---

/// tako が実行時に使う外部コマンドの定義
struct ExternalDep {
    /// コマンド名
    bin: &'static str,
    /// 必須依存か（false = 任意。無くても tako 自体は動く）
    required: bool,
    /// 影響する機能の説明
    purpose: &'static str,
    /// brew でインストールする場合のパッケージ名（None = brew 非対応）
    brew_pkg: Option<&'static str>,
    /// brew 以外の導入案内
    install_hint: &'static str,
}

const EXTERNAL_DEPS: &[ExternalDep] = &[
    ExternalDep {
        bin: "claude",
        required: true,
        purpose: "setup の対話・tako master・オーケストレーター・タブの自動リネーム",
        brew_pkg: None,
        install_hint: "https://docs.anthropic.com/en/docs/claude-code",
    },
    ExternalDep {
        bin: "tmux",
        required: false,
        purpose: "リモート接続（tako remote）・再起動時のセッション完全復元・オーケストレーターの worker 管理",
        brew_pkg: Some("tmux"),
        install_hint: "https://github.com/tmux/tmux/wiki/Installing",
    },
    ExternalDep {
        bin: "cloudflared",
        required: false,
        purpose: "リモート接続（tako remote）のトンネル公開。未導入だと同一 LAN 内限定の URL になります",
        brew_pkg: Some("cloudflared"),
        install_hint: "https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/",
    },
    ExternalDep {
        bin: "git",
        required: false,
        purpose: "git パネル（ブランチ・コミットグラフ・diff 表示）",
        brew_pkg: Some("git"),
        install_hint: "xcode-select --install でも導入できます",
    },
];

/// 依存ツールのチェック段階。検出結果を ✓ / △ / ✗ で表示し、
/// interactive = true なら未導入の依存をその場で brew インストールできる。
/// 戻り値はチェック後も欠けている必須依存のコマンド名一覧。
fn run_dependency_check(interactive: bool) -> Vec<&'static str> {
    let brew = find_command("brew");
    let mut missing_required = Vec::new();
    for dep in EXTERNAL_DEPS {
        if let Some(path) = find_command(dep.bin) {
            eprintln!("  ✓ {}: {path}", dep.bin);
            continue;
        }
        let (mark, kind) = if dep.required {
            ("✗", "必須")
        } else {
            ("△", "任意")
        };
        eprintln!("  {mark} {}: 見つかりません（{kind}）", dep.bin);
        eprintln!("      用途: {}", dep.purpose);
        if !dep.required {
            eprintln!("      無くても tako 自体は動きますが、上記の機能が使えません");
        }
        let mut installed = false;
        match (dep.brew_pkg, brew.as_deref()) {
            (Some(pkg), Some(brew_bin)) => {
                eprintln!("      導入方法: brew install {pkg}");
                if interactive {
                    installed = offer_brew_install(pkg, brew_bin);
                }
            }
            (Some(pkg), None) => {
                eprintln!(
                    "      導入方法: brew install {pkg}（要 Homebrew）/ {}",
                    dep.install_hint
                );
            }
            (None, _) => {
                eprintln!("      導入方法: {}", dep.install_hint);
            }
        }
        if installed {
            match find_command(dep.bin) {
                Some(path) => eprintln!("  ✓ {}: {path}（インストール完了）", dep.bin),
                None => {
                    eprintln!(
                        "  ⚠ {}: インストール後も検出できません。シェルを開き直してから再実行してください",
                        dep.bin
                    );
                    if dep.required {
                        missing_required.push(dep.bin);
                    }
                }
            }
        } else if dep.required {
            missing_required.push(dep.bin);
        }
    }
    // FDA チェック（macOS のみ。任意だが強く推奨）
    #[cfg(target_os = "macos")]
    {
        run_fda_check(interactive);
    }
    // スリープ防止の設定案内
    run_sleep_guard_check(interactive);
    missing_required
}

/// 未導入の依存をその場で brew インストールするか確認して実行する。
/// インストールが成功したら true
fn offer_brew_install(pkg: &str, brew_bin: &str) -> bool {
    eprint!("      今すぐ brew install {pkg} を実行しますか？ [y/N]: ");
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    let answer = input.trim().to_ascii_lowercase();
    if answer != "y" && answer != "yes" {
        eprintln!("      スキップしました（後から brew install {pkg} で導入できます）");
        return false;
    }
    let status = std::process::Command::new(brew_bin)
        .args(["install", pkg])
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status();
    match status {
        Ok(s) if s.success() => true,
        _ => {
            eprintln!("      ⚠ brew install {pkg} が失敗しました。手動で導入してください");
            false
        }
    }
}

/// スリープ防止（Issue #173）の設定案内。
/// L0〜L3 の段階式で、ユーザーの利用スタイルに合わせたスリープ防止を設定する
fn run_sleep_guard_check(interactive: bool) {
    let settings = tako_control::settings::load();
    let mode = settings.sleep_guard_mode;
    let power = settings.sleep_guard_power;
    eprintln!();
    eprintln!(
        "  スリープ防止: mode={}, power={}",
        mode.as_str(),
        power.as_str()
    );
    if !interactive {
        eprintln!("      設定変更: tako sleep-guard set --mode <mode> --power <condition>");
        return;
    }
    eprintln!("      エージェントが長時間動いている間に PC がスリープすると作業が止まります。");
    eprintln!("      スリープ防止の稼働レベルを選んでください:");
    eprintln!();
    eprintln!("      [0] OS 任せ（機能オフ）");
    eprintln!("      [1] AC 接続時のみアイドルスリープ防止（推奨）");
    eprintln!("      [2] バッテリー時もアイドルスリープ防止（電池消耗に注意）");
    eprintln!("      [3] 蓋閉じでも稼働（案内のみ — 手動設定が必要）");
    eprintln!();
    let current_level = match (mode, power) {
        (tako_control::sleep_guard::SleepGuardMode::Off, _) => 0,
        (_, tako_control::sleep_guard::PowerCondition::AcOnly) => 1,
        (_, tako_control::sleep_guard::PowerCondition::Always) => 2,
    };
    eprint!("      レベルを選択 [0-3]（現在: L{current_level}、Enter でスキップ）: ");
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return;
    }
    let choice = input.trim();
    if choice.is_empty() {
        eprintln!("      現在の設定を維持します");
        return;
    }
    let mut new_settings = settings;
    match choice {
        "0" => {
            new_settings.sleep_guard_mode = tako_control::sleep_guard::SleepGuardMode::Off;
            eprintln!("      ✓ L0: スリープ防止を無効にしました（OS 任せ）");
        }
        "1" => {
            new_settings.sleep_guard_mode =
                tako_control::sleep_guard::SleepGuardMode::WhileAgentsRunning;
            new_settings.sleep_guard_power = tako_control::sleep_guard::PowerCondition::AcOnly;
            eprintln!("      ✓ L1: AC 接続時のみ、エージェント稼働中にスリープを防止します");
        }
        "2" => {
            new_settings.sleep_guard_mode =
                tako_control::sleep_guard::SleepGuardMode::WhileAgentsRunning;
            new_settings.sleep_guard_power = tako_control::sleep_guard::PowerCondition::Always;
            eprintln!("      ✓ L2: バッテリー時もエージェント稼働中にスリープを防止します");
            eprintln!("      ⚠ 電池消耗が速くなります。AC 接続での利用を推奨します");
        }
        "3" => {
            new_settings.sleep_guard_mode =
                tako_control::sleep_guard::SleepGuardMode::WhileAgentsRunning;
            new_settings.sleep_guard_power = tako_control::sleep_guard::PowerCondition::AcOnly;
            eprintln!("      ✓ L3: L1 の設定を適用しました（AC 接続時のみ防止）");
            eprintln!();
            eprintln!("      蓋閉じでの継続稼働:");
            eprintln!("      ─────────────────────────────────────────────");
            eprintln!("      tako sleep-guard install-lid-sleep");
            eprintln!("        初回のみ管理者パスワードが必要。以後 tako が");
            eprintln!("        エージェント稼働中だけ自動で蓋閉じ継続を有効にします。");
            eprintln!("        解除: tako sleep-guard remove-lid-sleep");
            eprintln!("      ─────────────────────────────────────────────");
        }
        other => {
            eprintln!("      不明な選択: {other}。現在の設定を維持します");
            return;
        }
    }
    if let Err(e) = tako_control::settings::save(&new_settings) {
        eprintln!("      ⚠ 設定の保存に失敗: {e}");
    }
}

/// FDA（フルディスクアクセス）の案内ステップ。
/// macOS の TCC（Transparency, Consent, and Control）による「ほかのアプリからの
/// データへのアクセス権を求められています」ダイアログを一括で消す方法を案内する。
#[cfg(target_os = "macos")]
fn run_fda_check(interactive: bool) {
    if tako_control::fda::is_granted() {
        eprintln!("  ✓ フルディスクアクセス: 付与済み（許可ダイアログは表示されません）");
        return;
    }
    eprintln!("  △ フルディスクアクセス: 未付与（推奨）");
    eprintln!("      macOS が「tako.app から、ほかのアプリからのデータへのアクセス権を");
    eprintln!("      求められています」と頻繁に表示する原因です。フルディスクアクセスを");
    eprintln!("      付与すると、このダイアログが出なくなります。");
    eprintln!(
        "      設定方法: システム設定 → プライバシーとセキュリティ → フルディスクアクセス → tako を追加"
    );
    if !interactive {
        eprintln!("      付与方法: tako fda open でシステム設定を開き、tako を追加してください");
        return;
    }
    eprint!("      今すぐシステム設定を開きますか？ [y/N]: ");
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return;
    }
    let answer = input.trim().to_ascii_lowercase();
    if answer != "y" && answer != "yes" {
        eprintln!("      スキップしました（後から tako fda open で設定画面を開けます）");
        return;
    }
    if let Err(e) = tako_control::fda::open_settings() {
        eprintln!("      ⚠ {e}");
        return;
    }
    eprintln!(
        "      システム設定を開きました。tako を「フルディスクアクセス」に追加してください。"
    );
    eprintln!("      ⚠ 付与後、tako アプリの再起動が必要です（⌘Q で終了 → 再度起動）。");
    eprintln!("        再起動するまで許可ダイアログが表示され続けることがあります。");

    // 再チェック（FDA は再起動後に有効になるため通常ここでは検出できないが、
    // 過去に付与済みで検出が遅延していた場合は拾える）
    eprintln!();
    eprint!("      設定しましたか？ 確認します... ");
    // 設定画面での操作を待つ猶予
    std::thread::sleep(std::time::Duration::from_secs(2));
    if tako_control::fda::is_granted() {
        eprintln!("✓ 付与を確認しました！ tako を再起動すると反映されます。");
    } else {
        eprintln!("まだ検出できません。");
        eprintln!("        付与後に tako を再起動すれば反映されます。今は先に進みます。");
    }
}

fn check_mcp_registered() -> bool {
    let output = std::process::Command::new(login_shell())
        .args(["-l", "-c", "claude mcp list 2>/dev/null"])
        .output();
    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            stdout.contains("tako")
        }
        _ => false,
    }
}

fn run_setup_mcp() -> Result<(), String> {
    let tako_bin = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| tako_control::dispatch::resolve_tako_binary());
    let settings_dir = home_dir()
        .ok_or("ホームディレクトリが取得できない")?
        .join(".claude");
    let settings_path = settings_dir.join("settings.json");
    match tako_control::dispatch::setup_mcp_settings(&tako_bin, &settings_path) {
        Ok(result) => {
            if result.already_existed {
                eprintln!("  MCP: 既に設定されています");
            } else {
                eprintln!("  MCP: 設定を追加しました");
            }
            Ok(())
        }
        Err(e) => Err(format!("MCP 設定の追加に失敗: {e}")),
    }
}

// --- リソース書き出し ---

fn write_resource(dir: &Path, rel_path: &str, content: &str) -> Result<(), String> {
    let path = dir.join(rel_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("ディレクトリの作成に失敗 ({}): {e}", parent.display()))?;
    }
    std::fs::write(&path, content)
        .map_err(|e| format!("ファイルの書き出しに失敗 ({}): {e}", path.display()))
}

fn write_all_resources(setup_dir: &Path) -> Result<(), String> {
    write_resource(
        setup_dir,
        "templates/sections/00-language.md",
        TPL_00_LANGUAGE,
    )?;
    write_resource(
        setup_dir,
        "templates/sections/01-interaction-style.md",
        TPL_01_INTERACTION,
    )?;
    write_resource(
        setup_dir,
        "templates/sections/02-git-workflow.md",
        TPL_02_GIT,
    )?;
    write_resource(
        setup_dir,
        "templates/sections/03-code-quality.md",
        TPL_03_CODE,
    )?;
    write_resource(
        setup_dir,
        "templates/sections/04-safety-rules.md",
        TPL_04_SAFETY,
    )?;
    write_resource(
        setup_dir,
        "templates/sections/05-proposal-quality.md",
        TPL_05_PROPOSAL,
    )?;
    write_resource(
        setup_dir,
        "templates/sections/06-completion-verification.md",
        TPL_06_VERIFICATION,
    )?;
    write_resource(setup_dir, "templates/config-default.yaml", CONFIG_DEFAULT)?;
    // setup changelog の全履歴（setup エージェントが Read できるように毎回最新を展開）
    write_resource(setup_dir, "changes.yaml", CHANGES_YAML)?;
    Ok(())
}

// --- アップデート追従（Issue #94） ---

/// pending-changes.md のパス（setup ディレクトリ直下。setup エージェントが Read する）
fn pending_changes_path(setup_dir: &Path) -> PathBuf {
    setup_dir.join("pending-changes.md")
}

/// 未適用の変更一覧を CLI に表示する
fn print_pending_changes(pending: &[SetupChange], applied_revision: u32) {
    eprintln!(
        "  ℹ 前回のセットアップ（rev {applied_revision}）以降、アップデートで setup に {} 件の変更が入っています:",
        pending.len()
    );
    for change in pending {
        let kind = match change.kind {
            ChangeKind::Auto => "自動適用",
            ChangeKind::Guided => "対話で確認",
        };
        eprintln!(
            "      [rev {} / v{} / {kind}] {}",
            change.revision, change.version, change.title
        );
    }
}

/// 未適用の変更に応じて pending-changes.md を書き出す / 追従不要なら消す（stale 防止）
fn sync_pending_changes_file(
    setup_dir: &Path,
    pending: &[SetupChange],
    applied_revision: u32,
) -> Result<(), String> {
    let path = pending_changes_path(setup_dir);
    if pending.is_empty() {
        if path.is_file() {
            std::fs::remove_file(&path)
                .map_err(|e| format!("pending-changes.md の削除に失敗: {e}"))?;
        }
        return Ok(());
    }
    let md = tako_control::setup::render_pending_markdown(pending, applied_revision);
    std::fs::write(&path, md).map_err(|e| format!("pending-changes.md の書き出しに失敗: {e}"))
}

// --- メインエントリ ---

/// `tako setup --check` — 環境チェックだけ実行して終了
pub fn run_check() -> Result<(), String> {
    eprintln!("tako セットアップ 環境チェック");
    eprintln!("─────────────────────────────");

    // 依存ツール（claude / tmux / git。--check では表示のみ）
    let _ = run_dependency_check(false);

    // MCP 登録
    if check_mcp_registered() {
        eprintln!("  ✓ MCP: tako が登録済み");
    } else {
        eprintln!("  ✗ MCP: tako が未登録（tako setup-mcp で登録できます）");
    }

    // config.yaml
    let config_path = tako_control::setup::config_yaml_path()?;
    if config_path.is_file() {
        let config = load_config()?;
        if config.setup.completed {
            eprintln!(
                "  ✓ セットアップ: 完了済み ({})",
                config.setup.completed_at.as_deref().unwrap_or("日時不明")
            );
            // アップデート追従状況（Issue #94）
            let pending = pending_changes(config.setup.applied_revision)?;
            if pending.is_empty() {
                eprintln!(
                    "  ✓ アップデート追従: 最新（rev {}）",
                    config.setup.applied_revision
                );
            } else {
                eprintln!(
                    "  △ アップデート追従: 未適用の setup 変更が {} 件（tako setup --changes で詳細）",
                    pending.len()
                );
            }
        } else {
            eprintln!("  △ セットアップ: 未完了");
        }
    } else {
        eprintln!("  △ config.yaml: 未作成");
    }

    // ~/.claude/CLAUDE.md
    if let Some(home) = home_dir() {
        let claude_md = home.join(".claude/CLAUDE.md");
        if claude_md.is_file() {
            eprintln!("  ✓ ~/.claude/CLAUDE.md: 存在します");
        } else {
            eprintln!("  △ ~/.claude/CLAUDE.md: 未作成");
        }
    }

    // エージェント共通ルール同期（Issue #136）
    match tako_control::agents_sync::status() {
        Ok(status) => {
            let st = status["status"].as_str().unwrap_or("unknown");
            match st {
                "not_configured" => {
                    eprintln!("  △ エージェント共通ルール同期: 未設定");
                }
                "up_to_date" => {
                    eprintln!("  ✓ エージェント共通ルール同期: 最新");
                }
                "outdated" => {
                    eprintln!(
                        "  △ エージェント共通ルール同期: ずれあり（tako agents sync-rules で同期）"
                    );
                }
                "source_missing" => {
                    let path = status["source_path"].as_str().unwrap_or("?");
                    eprintln!("  ✗ エージェント共通ルール同期: 正本が見つからない ({path})");
                }
                _ => {
                    eprintln!("  ? エージェント共通ルール同期: {st}");
                }
            }
        }
        Err(e) => eprintln!("  △ エージェント共通ルール同期: 確認失敗 ({e})"),
    }

    // スリープ防止（Issue #173）
    {
        let settings = tako_control::settings::load();
        let mode = settings.sleep_guard_mode;
        let power = settings.sleep_guard_power;
        match mode {
            tako_control::sleep_guard::SleepGuardMode::Off => {
                eprintln!("  △ スリープ防止: 無効（tako sleep-guard set --mode while-agents-running で有効化）");
            }
            _ => {
                eprintln!(
                    "  ✓ スリープ防止: mode={}, power={}",
                    mode.as_str(),
                    power.as_str()
                );
            }
        }
        let lid_mode = settings.lid_sleep_mode;
        let sudoers = tako_control::sleep_guard::is_sudoers_installed();
        match lid_mode {
            tako_control::sleep_guard::LidSleepMode::Off => {
                eprintln!("  △ 蓋閉じ防止: 未設定（tako sleep-guard install-lid-sleep で有効化）");
            }
            tako_control::sleep_guard::LidSleepMode::WhileAgentsRunning => {
                if sudoers {
                    eprintln!("  ✓ 蓋閉じ防止: while-agents-running（sudoers 登録済み）");
                } else {
                    eprintln!("  ✗ 蓋閉じ防止: while-agents-running だが sudoers 未登録（tako sleep-guard install-lid-sleep で登録）");
                }
            }
        }
    }

    // プロファイル一覧
    match tako_control::orchestrator::list_profiles() {
        Ok(profiles) if !profiles.is_empty() => {
            eprintln!(
                "  ✓ プロファイル: {} 個（{}）",
                profiles.len(),
                profiles.join(", ")
            );
        }
        Ok(_) => eprintln!("  △ プロファイル: 未作成（tako master で自動生成されます）"),
        Err(e) => eprintln!("  △ プロファイル: 確認失敗 ({e})"),
    }

    Ok(())
}

/// `tako setup --reset` — config.yaml の setup.completed を false にリセット
pub fn run_reset() -> Result<(), String> {
    // ロック付き read-modify-write（#169: 他フィールドの並行更新を巻き戻さない）
    tako_control::setup::mutate_config(|config| {
        config.setup.completed = false;
        config.setup.completed_at = None;
    })?;
    eprintln!("セットアップ状態をリセットしました。tako setup で再実行できます");
    Ok(())
}

/// `tako setup --changes` — アップデート追従状況の表示（Issue #94）。
/// MCP `tako_setup_changes` と同じ照会（`--json` で同一ペイロードを出力）
pub fn run_changes(json: bool) -> Result<(), String> {
    if json {
        let status = tako_control::setup::changes_status()?;
        println!(
            "{}",
            serde_json::to_string_pretty(&status).map_err(|e| format!("JSON 変換に失敗: {e}"))?
        );
        return Ok(());
    }
    let config = load_config()?;
    let current = tako_control::setup::current_revision()?;
    let applied = config.setup.applied_revision;
    eprintln!("tako setup アップデート追従状況");
    eprintln!("─────────────────────────────");
    eprintln!(
        "  現在の setup リビジョン: {current}（tako v{}）",
        env!("CARGO_PKG_VERSION")
    );
    if !config.setup.completed {
        eprintln!("  セットアップ: 未実施（tako setup を実行すると最新の設定で導入されます）");
        return Ok(());
    }
    match &config.setup.applied_version {
        Some(v) => eprintln!("  適用済みリビジョン: {applied}（tako v{v} で setup 実行）"),
        None => eprintln!("  適用済みリビジョン: {applied}"),
    }
    let pending = pending_changes(applied)?;
    if pending.is_empty() {
        eprintln!("  ✓ 最新です。追従が必要な変更はありません");
        return Ok(());
    }
    eprintln!("  未適用の変更: {} 件", pending.len());
    eprintln!();
    for change in &pending {
        let kind = match change.kind {
            ChangeKind::Auto => "auto（setup 再実行で自動適用）",
            ChangeKind::Guided => "guided（setup の対話で確認・適用）",
        };
        eprintln!(
            "  [rev {} / v{} / {}] {}",
            change.revision, change.version, change.date, change.title
        );
        eprintln!("      区分: {kind}");
        for line in change.description.lines() {
            eprintln!("      {line}");
        }
        eprintln!();
    }
    eprintln!("  `tako setup` を実行すると追従できます");
    Ok(())
}

/// `tako setup` — メインのセットアップフロー
pub fn run_setup() -> Result<(), String> {
    eprintln!("tako セットアップ");
    eprintln!("═════════════════");
    eprintln!();

    // 1. 依存ツールのチェック（必須 = claude、任意 = tmux / git。未導入はその場インストール可）
    let missing = run_dependency_check(true);
    if !missing.is_empty() {
        return Err(format!(
            "必須の依存ツールが不足しています: {}。\n\
             導入後に tako setup を再実行してください",
            missing.join(", ")
        ));
    }

    // 2. MCP 登録確認
    if !check_mcp_registered() {
        eprintln!("  △ MCP 未登録 → 自動登録します...");
        run_setup_mcp()?;
    } else {
        eprintln!("  ✓ MCP: tako が登録済み");
    }

    // 3. setup ディレクトリ + リソース書き出し
    let dir = setup_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("ディレクトリの作成に失敗: {e}"))?;
    write_all_resources(&dir)?;
    eprintln!("  ✓ テンプレートを展開: {}", dir.display());

    // CLAUDE.md（setup 用 system prompt）を書き出す
    let claude_md_path = dir.join("CLAUDE.md");
    std::fs::write(&claude_md_path, SYSTEM_PROMPT)
        .map_err(|e| format!("CLAUDE.md の書き出しに失敗: {e}"))?;

    // 4. config.yaml の初回 / 2 回目判定
    let config = load_config()?;
    let is_first_run = !config.setup.completed;

    // 4.5 アップデート追従（Issue #94）: 前回セットアップ以降に setup へ入った変更を検出。
    // 初回はすべて最新の内容で導入されるため対象外（完了時に最新リビジョンを記録するのみ）
    let pending = if is_first_run {
        Vec::new()
    } else {
        pending_changes(config.setup.applied_revision)?
    };
    if !pending.is_empty() {
        eprintln!();
        print_pending_changes(&pending, config.setup.applied_revision);
        eprintln!(
            "      詳細を pending-changes.md に書き出しました。claude が対話の中で追従を案内します"
        );
    }
    sync_pending_changes_file(&dir, &pending, config.setup.applied_revision)?;

    // 5. claude を setup cwd で起動
    // ~/.claude/CLAUDE.md が存在すればバックアップ
    if let Some(home) = home_dir() {
        let claude_md = home.join(".claude/CLAUDE.md");
        if claude_md.is_file() {
            let backup = find_backup_path(&home.join(".claude"), "CLAUDE.md");
            if let Err(e) = std::fs::copy(&claude_md, &backup) {
                eprintln!("  ⚠ CLAUDE.md のバックアップに失敗: {e}");
            } else {
                eprintln!(
                    "  ✓ CLAUDE.md をバックアップ: {}",
                    backup.file_name().unwrap_or_default().to_string_lossy()
                );
            }
        }
    }

    eprintln!();
    if is_first_run {
        eprintln!("初回セットアップを開始します。claude が対話で設定をガイドします。");
    } else {
        eprintln!("セットアップメニューを開きます。");
    }
    eprintln!("─────────────────────────────────────────────────────");
    eprintln!();

    let shell = login_shell();

    let greeting = if is_first_run {
        "tako のセットアップを始めます。いくつか質問に答えてください。"
    } else if !pending.is_empty() {
        "tako の設定を更新します。まず pending-changes.md を読んで、前回セットアップ以降のアップデート変更への追従から始めてください。"
    } else {
        "tako の設定を変更します。何をしますか？"
    };

    let claude_cmd = format!(
        "cd '{}' && claude --model 'claude-opus-4-6' '{}'",
        dir.display(),
        greeting.replace('\'', "'\\''"),
    );

    let status = std::process::Command::new(&shell)
        .args(["-l", "-c", &claude_cmd])
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|e| format!("claude の起動に失敗: {e}"))?;

    if status.success() {
        // セットアップ完了を記録（適用済み setup リビジョンを含む。Issue #94）。
        // claude 対話中に他プロセスが config.yaml を更新していても巻き戻さないよう、
        // 完了フィールドだけをロック付き read-modify-write で更新する（#169）
        let revision = tako_control::setup::current_revision()?;
        tako_control::setup::mutate_config(|config| {
            config.setup.completed = true;
            config.setup.completed_at = Some(now_iso8601());
            config.setup.applied_revision = revision;
            config.setup.applied_version = Some(env!("CARGO_PKG_VERSION").to_string());
        })?;
        // 追従が完了したので pending-changes.md を消す（stale 防止）
        sync_pending_changes_file(&dir, &[], revision)?;
        eprintln!();
        eprintln!("セットアップが完了しました。");
    } else {
        eprintln!();
        eprintln!(
            "claude が終了しました（exit code: {}）",
            status.code().unwrap_or(-1)
        );
    }

    // 6. デフォルトプロファイルの確認・作成
    use tako_control::orchestrator;
    if let Err(e) = orchestrator::ensure_defaults() {
        eprintln!("  ⚠ プロファイルの初期化に失敗: {e}");
    } else {
        eprintln!("  ✓ デフォルトのオーケストレータープロファイルを確認しました");
        // 旧バージョンが書き込んだ [1m] 既定値のマイグレーション（Issue #27）
        if let Some(notice) = orchestrator::migrate_legacy_default_profile() {
            eprintln!("  ℹ {notice}");
        }
        match orchestrator::list_profiles() {
            Ok(profiles) if profiles.len() > 1 => {
                eprintln!("    既存プロファイル: {}", profiles.join(", "));
            }
            _ => {}
        }
    }

    // 7. オーケストレータープロファイルの設定（対話・スキップ可能）
    eprintln!();
    eprintln!("━━━ オーケストレータープロファイル設定 ━━━");
    eprintln!();
    eprintln!("tako master で子 worker を管理するときのモデル・effort 設定を行います。");
    eprintln!("Pro プランではモデル指定が制限される場合があるため、既定のままでも構いません。");
    eprintln!();
    run_profile_setup()?;

    Ok(())
}

/// オーケストレータープロファイルの対話式設定
fn run_profile_setup() -> Result<(), String> {
    use tako_control::orchestrator;

    let stdin = std::io::stdin();
    let mut input = String::new();

    // 既定のままにする選択肢を最初に提示
    eprintln!("プロファイルを設定しますか？");
    eprintln!("  1) 既定のままにする（推奨: claude 既定モデル / max / inherit。全プランで動作）");
    eprintln!("  2) 設定する");
    eprint!("選択 [1]: ");
    input.clear();
    let _ = stdin.read_line(&mut input);
    let choice = input.trim();
    if choice.is_empty() || choice == "1" {
        eprintln!();
        eprintln!("  既定のプロファイルを維持します。");
        show_profile_paths()?;
        return Ok(());
    }

    // プロファイル名
    eprintln!();
    eprint!("プロファイル名 [default]: ");
    input.clear();
    let _ = stdin.read_line(&mut input);
    let profile_name = input.trim();
    let profile_name = if profile_name.is_empty() {
        "default"
    } else {
        profile_name
    }
    .to_string();

    // 既存プロファイルがあれば読み込む
    let mut profile = orchestrator::Profile::load(&profile_name).unwrap_or_default();

    // master のモデル
    eprintln!();
    eprintln!("master のモデル（未指定 = claude CLI の既定モデル。プラン非依存で推奨）:");
    eprintln!("  現在: {}", profile.model_label());
    eprintln!("  空欄 = 現状維持、`-` = 指定を解除して claude 既定に戻す");
    eprintln!("  注意: [1m] 付き（1M コンテキスト版）は Max / API プラン限定");
    eprint!("モデル [{}]: ", profile.model_label());
    input.clear();
    let _ = stdin.read_line(&mut input);
    let model_input = input.trim();
    if model_input == "-" {
        profile.model = None;
    } else if !model_input.is_empty() {
        profile.model = Some(model_input.to_string());
    }

    // master の effort
    eprintln!();
    eprintln!("master の effort:");
    eprintln!("  現在: {}", profile.effort);
    eprint!("effort [{}]: ", profile.effort);
    input.clear();
    let _ = stdin.read_line(&mut input);
    let effort_input = input.trim();
    if !effort_input.is_empty() {
        profile.effort = effort_input.to_string();
    }

    // 子 worker のモデル決定ポリシー
    eprintln!();
    eprintln!("子 worker のモデル決定ポリシー:");
    eprintln!("  1) inherit — master と同じモデル・effort を使う（推奨）");
    eprintln!("  2) fixed — 子 worker は別の固定モデルを使う");
    eprintln!("  3) delegate — master がタスク内容を見て判断する");
    eprint!("選択 [1]: ");
    input.clear();
    let _ = stdin.read_line(&mut input);
    let policy_choice = input.trim();
    match policy_choice {
        "2" => {
            profile.worker_model_policy = orchestrator::WorkerModelPolicy::Fixed;
            eprintln!();
            eprint!("子 worker のモデル [{}]: ", profile.model_label());
            input.clear();
            let _ = stdin.read_line(&mut input);
            let wm = input.trim();
            if !wm.is_empty() {
                profile.worker_model = Some(wm.to_string());
            }
            eprint!("子 worker の effort [{}]: ", profile.effort);
            input.clear();
            let _ = stdin.read_line(&mut input);
            let we = input.trim();
            if !we.is_empty() {
                profile.worker_effort = Some(we.to_string());
            }
        }
        "3" => {
            profile.worker_model_policy = orchestrator::WorkerModelPolicy::Delegate;
            eprintln!();
            eprintln!("振り分け方針のテキスト（master の system prompt に注入されます）。");
            eprintln!("空欄で既定の雛形を使います。ファイルパス（~/...）も指定可能。");
            eprint!("guidance: ");
            input.clear();
            let _ = stdin.read_line(&mut input);
            let guidance = input.trim();
            if !guidance.is_empty() {
                profile.delegate_guidance = Some(guidance.to_string());
            }
        }
        _ => {
            profile.worker_model_policy = orchestrator::WorkerModelPolicy::Inherit;
        }
    }

    // 保存
    let saved_path = profile
        .save(&profile_name)
        .map_err(|e| format!("プロファイルの保存に失敗: {e}"))?;
    eprintln!();
    eprintln!("  ✓ プロファイルを保存しました: {}", saved_path.display());
    let policy_desc = match profile.worker_model_policy {
        orchestrator::WorkerModelPolicy::Inherit => {
            format!("inherit（{} / {}）", profile.model_label(), profile.effort)
        }
        orchestrator::WorkerModelPolicy::Fixed => format!(
            "fixed（{} / {}）",
            profile.worker_model_label(),
            profile.resolve_worker_effort()
        ),
        orchestrator::WorkerModelPolicy::Delegate => "delegate（master が判断）".into(),
    };
    eprintln!(
        "    master: {} / {}、worker: {policy_desc}",
        profile.model_label(),
        profile.effort
    );
    if let Some(warning) = profile
        .model
        .as_deref()
        .and_then(|m| orchestrator::one_m_model_warning(m, "master"))
    {
        eprintln!("{warning}");
    }
    show_profile_paths()?;
    Ok(())
}

fn show_profile_paths() -> Result<(), String> {
    use tako_control::orchestrator;
    eprintln!();
    eprintln!("プロファイル設定の変更:");
    eprintln!(
        "  {}orchestrator/profiles/<名前>.yaml を編集",
        orchestrator::config_dir()
            .map(|d| format!("{}/", d.display()))
            .unwrap_or_default()
    );
    eprintln!("  tako master -<名前> で起動");
    Ok(())
}

fn find_backup_path(dir: &Path, filename: &str) -> PathBuf {
    let today = {
        let output = std::process::Command::new("date")
            .args(["+%Y-%m-%d"])
            .output();
        match output {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            _ => "unknown".into(),
        }
    };
    let base = dir.join(format!("{filename}.backup-{today}"));
    if !base.exists() {
        return base;
    }
    let mut n = 2u32;
    loop {
        let candidate = dir.join(format!("{filename}.backup-{today}-{n}"));
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

fn now_iso8601() -> String {
    let output = std::process::Command::new("date")
        .args(["+%Y-%m-%dT%H:%M:%S%z"])
        .output();
    match output {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            // +0900 → +09:00
            if s.len() >= 24 && !s.contains('+') {
                s
            } else if s.len() >= 24 {
                let (head, tail) = s.split_at(s.len() - 2);
                format!("{head}:{tail}")
            } else {
                s
            }
        }
        _ => "unknown".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tako_control::setup::SetupConfig;

    // config.yaml のスキーマ・後方互換のテストは tako_control::setup 側にある（Issue #94）

    #[test]
    fn config_from_default_yaml() {
        let config: SetupConfig = serde_yaml::from_str(CONFIG_DEFAULT).unwrap();
        assert!(config.orchestrator.auto_close);
        assert!(!config.setup.completed);
        // モデル設定キーはテンプレに含まれない（profiles/*.yaml が正。Issue #27）
        assert!(!CONFIG_DEFAULT.contains("master_model"));
        assert!(!CONFIG_DEFAULT.contains("worker_model"));
    }

    #[test]
    fn pending_changes_file_sync() {
        let tmp = std::env::temp_dir().join("tako-test-pending-sync");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let pending = pending_changes(0).unwrap();
        assert!(!pending.is_empty(), "初期エントリが存在する");
        // 未適用あり → pending-changes.md が書き出される
        sync_pending_changes_file(&tmp, &pending, 0).unwrap();
        let path = pending_changes_path(&tmp);
        assert!(path.is_file());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("rev 1"));
        // 追従完了（未適用ゼロ）→ 消える（stale 防止）
        sync_pending_changes_file(&tmp, &[], 4).unwrap();
        assert!(!path.exists());
        // 無い状態での再同期も no-op で成功する
        sync_pending_changes_file(&tmp, &[], 4).unwrap();
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn external_deps_table_is_consistent() {
        // claude は必須依存として先頭に置く（setup の対話自体が claude を使うため）
        assert_eq!(EXTERNAL_DEPS[0].bin, "claude");
        assert!(EXTERNAL_DEPS[0].required);
        // tmux は任意依存（remote / 永続化 / オーケストレーターが対象機能）
        let tmux = EXTERNAL_DEPS.iter().find(|d| d.bin == "tmux").unwrap();
        assert!(!tmux.required);
        assert!(tmux.purpose.contains("tako remote"));
        assert_eq!(tmux.brew_pkg, Some("tmux"));
        // cloudflared は任意依存（トンネル公開。未導入だと LAN 限定 URL = #89）
        let cf = EXTERNAL_DEPS
            .iter()
            .find(|d| d.bin == "cloudflared")
            .unwrap();
        assert!(!cf.required);
        assert_eq!(cf.brew_pkg, Some("cloudflared"));
        // 全依存に用途説明と導入案内がある
        for dep in EXTERNAL_DEPS {
            assert!(!dep.purpose.is_empty(), "{} の purpose が空", dep.bin);
            assert!(
                !dep.install_hint.is_empty(),
                "{} の install_hint が空",
                dep.bin
            );
        }
    }

    #[test]
    fn embedded_resources_not_empty() {
        assert!(!SYSTEM_PROMPT.is_empty());
        assert!(!TPL_00_LANGUAGE.is_empty());
        assert!(!TPL_01_INTERACTION.is_empty());
        assert!(!TPL_02_GIT.is_empty());
        assert!(!TPL_03_CODE.is_empty());
        assert!(!TPL_04_SAFETY.is_empty());
        assert!(!TPL_05_PROPOSAL.is_empty());
        assert!(!TPL_06_VERIFICATION.is_empty());
        assert!(!CONFIG_DEFAULT.is_empty());
        assert!(!CHANGES_YAML.is_empty());
    }

    #[test]
    fn system_prompt_mentions_update_follow_flow() {
        // setup エージェントがアップデート追従を実施できるよう、system prompt に
        // pending-changes.md への言及がある（Issue #94）
        assert!(SYSTEM_PROMPT.contains("pending-changes.md"));
        assert!(SYSTEM_PROMPT.contains("changes.yaml"));
    }
}
