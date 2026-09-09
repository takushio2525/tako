//! エージェント CLI の公式インストール手順（抽象境界 B17。#868 / #525 / #989）
//!
//! 「各エージェント CLI をどう入れるか」のプラットフォーム差をここへ閉じ込める。
//!
//! ## なぜ純粋関数にするか
//!
//! 手順は [`Platform`] を**引数で受ける純粋関数**として書く。こうすると
//! **macOS 上から Windows 向けの手順を検証できる**（#515 と同じ作法）。
//! 実行してよいかどうかは [`crate::platform::support`] のマトリクスが決めるので、
//! 「手順を知っていること」と「その環境で自動実行してよいこと」を混ぜない。
//!
//! ## 調査の根拠（2026-08-21 実測）
//!
//! 経路は推測せず、公式ドキュメントとインストーラの実物を確認して決めた。
//!
//! - 公式 docs（`https://code.claude.com/docs/en/setup.md`）のインストールタブは
//!   **「Native Install (Recommended)」** が第一候補。Homebrew タブには
//!   「Homebrew installations do not auto-update」と明記があり、native は
//!   「automatically update in the background」。ゼロスタートの利用者を
//!   置いていかないので native を採る（brew は選ばない）
//! - `https://claude.ai/install.sh` は 302 で
//!   `https://downloads.claude.ai/claude-code-releases/bootstrap.sh` へ飛ぶ 217 行の bash。
//!   中身は「sudo 拒否 → OS/arch 判定（Rosetta 込み）→ `latest` 取得 →
//!   `manifest.json` の SHA256 で検証 → `$HOME/.claude/downloads/` へ落として
//!   `chmod +x` → `<binary> install` → 一時ファイル削除」。**署名検証は
//!   インストーラ自身が行う**ので tako は二重にやらない
//! - 設置先は `~/.local/bin/claude`（symlink）→ `~/.local/share/claude/versions/<version>`
//! - このスクリプトは Windows を**明示的に非対応**にしている
//!   （`MINGW*|MSYS*|CYGWIN*` で exit 1）。Windows の公式手順は PowerShell の
//!   `irm https://claude.ai/install.ps1 | iex`（docs の Native Install タブ）
//!
//! ## Windows 側の調査の根拠（2026-09-01 実測。#1057）
//!
//! `https://claude.ai/install.ps1` は 3,189 バイトの PowerShell スクリプトで、
//! `install.sh` と同じ形をしている（32bit 拒否 → arch 判定（ARM64 込み）→ `latest` 取得 →
//! `manifest.json` の SHA256 で検証 → `$env:USERPROFILE\.claude\downloads\` へ落として
//! `claude.exe install <target>` → 一時ファイル削除）。**署名検証はインストーラ自身が行う**。
//!
//! - 先頭は `param(...)` で **shebang を持たない**ので、取得物の見分け方は
//!   プラットフォームごとに変える（[`ScriptSignature`]）
//! - `param()` を持つので `-File` で走らせても既定 `$Target = "latest"` が効く
//! - 実機の ExecutionPolicy は `CurrentUser = RemoteSigned`（実測）。落としたファイルを
//!   `-File` で走らせるには **`-ExecutionPolicy Bypass`**（プロセス限定）が要る。
//!   公式の `irm | iex` は文字列を食わせるので ExecutionPolicy の対象外
//! - 設置先は `~\.local\bin\claude.exe`。ここは
//!   [`crate::platform::exe`] の走査対象なので、PATH が再ログインまで
//!   伝播しない Windows でも導入直後に検出できる
//!
//! ## codex の調査の根拠（2026-09-09 実測。#989）
//!
//! `https://chatgpt.com/codex/install.sh` は 302 で
//! `https://releases.openai.com/codex/install.sh` へ飛ぶ 1,209 行の `#!/bin/sh`。
//!
//! - **`sudo` の参照が 0 件**。`BIN_DIR="${CODEX_INSTALL_DIR:-$HOME/.local/bin}"` /
//!   `CODEX_HOME_DIR="${CODEX_HOME:-$HOME/.codex}"` /
//!   `STANDALONE_ROOT="$CODEX_HOME_DIR/packages/standalone"` で、ホームの中だけで完結する
//! - `codex-package_SHA256SUMS` を落として突き合わせる = **署名検証はインストーラ自身が行う**
//! - PATH も**インストーラ自身**がマーカーブロックで profile へ書く（`pick_profile` が
//!   darwin + zsh なら `~/.zprofile`）。tako の置き場所も同じ `~/.local/bin` なので
//!   [`InstallRecipe::launcher_dir_rel`] が返す 1 個のブロックで 3 系統ぶんを兼ねる
//! - **最後に `maybe_launch_codex_now` が `Start Codex now? [y/N]` を `/dev/tty` から聞く**。
//!   代行するときは [`InstallRecipe::installer_env`] の `CODEX_NON_INTERACTIVE=1` が必須
//!   （無いと codex の TUI が立ってインストーラが返らない）
//! - `uname -s` が Darwin / Linux 以外なら
//!   `install.sh supports macOS and Linux. Use install.ps1 on Windows.` で exit 1
//! - **背景の自動更新は無い**。`codex doctor` が
//!   `↑ updates 0.153.4 available (current 0.153.0, dismissed 0.144.5)` と知らせるだけで、
//!   更新は `codex update`（実測）
//! - Windows の設置先は `%LOCALAPPDATA%\Programs\OpenAI\Codex\bin`
//!   （`install.ps1` の `$defaultVisibleBinDir`。claude と違って `~\.local\bin` ではない）
//!
//! ## agy の調査の根拠（2026-09-09 実測。#989）
//!
//! 公式 docs（`https://antigravity.google/docs/cli/install.md`）の
//! 「macOS and Linux」が `curl -fsSL https://antigravity.google/cli/install.sh | bash`。
//! 取得物は 239 行の `#!/bin/bash`。
//!
//! - **`sudo` の参照が 0 件**。`TARGET_DIR="$HOME/.local/bin"` /
//!   `BINARY_PATH="$TARGET_DIR/agy"` で、マニフェストの `sha512` と突き合わせてから置く
//!   （**署名検証はインストーラ自身が行う**）
//! - **単一バイナリ**なのでバージョンごとの実体ディレクトリが無い
//!   （= [`InstallRecipe::payload_rel`] は `None`）。展開前の置き場は
//!   `~/.cache/antigravity/staging` で、`trap cleanup EXIT` が消す
//! - 既に在れば `Notice: 'agy' is already installed at …` で **exit 0**（= 冪等）
//! - 同じ出力に `The Antigravity CLI automatically self-updates in the background during
//!   regular runs.` と明記があるので `auto_updates: true`
//! - 対話プロンプトを持たない（`read` / `/dev/tty` の参照なし）ので env の追加は要らない
//! - 最後に `"$BINARY_PATH" install` を呼んで PATH を設定する
//!   （`agy install` = 「Configure environment paths and shell settings」。実測の `agy help install`）
//! - Windows の設置先は `%LOCALAPPDATA%\agy\bin`（公式 docs の Windows 節）
//!
//! ## Windows で codex / agy の実行を代行しない理由（#989）
//!
//! `install.ps1` は 3 系統とも実在するが、実機で通したのは claude だけ（#1057）。
//! 未実測の経路を `tako_can_run: true` にすると「代行できるはず」で失敗して詰まるので、
//! codex / agy の Windows は **状態照会と公式手順の案内まで**にしてある
//! （実行代行は #525 の範囲。宣言は [`crate::platform::support`] の
//! `tako_setup_bootstrap` 行と [`crate::agent_support`] のマトリクス）

use super::support::Platform;
use std::path::PathBuf;

/// 公式インストーラの手順を持っているエージェント CLI（#868 → #989 で 3 系統へ）。
///
/// **ローカル LLM（`agent_support::Agent::Local`）はここに無い**。Ollama は
/// 「エージェント CLI」ではなく runtime で、導入したあとにモデルを pull する段が
/// 増えるので手順の形が違う（#990 の担当）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    Claude,
    Codex,
    Agy,
}

impl AgentKind {
    /// 列挙の正本。並びは「基準系 → master を務められる系統 → worker 専用」
    pub const ALL: [Self; 3] = [Self::Claude, Self::Codex, Self::Agy];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Agy => "agy",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == value)
    }

    /// 利用者へ見せる製品名（コマンド名ではなく「何を入れるのか」）
    pub fn product(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex CLI",
            Self::Agy => "Antigravity CLI",
        }
    }
}

/// インストーラの取得元 URL とその素性
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallerSource {
    /// 取得 URL（公式）
    pub url: &'static str,
    /// 取得したものを何で実行するか（`bash` / `powershell` 等）
    pub interpreter: &'static str,
    /// 人間へ提示する 1 行コマンド（公式 docs に載っているそのままの形）
    pub official_command: &'static str,
}

/// 取得したものが本物のインストーラかの見分け方。
///
/// **プロキシが返す HTML エラーページを弾くため**に見る（公式のトラブルシュートに
/// `syntax error near unexpected token '<'` として載っている実在の失敗モード）。
/// 判定そのものは [`looks_like_installer`] の純粋関数なので、
/// **macOS から Windows 側の判定も検証できる**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptSignature {
    /// 先頭が shebang（`#!`）。`install.sh` はこの形
    Shebang,
    /// PowerShell スクリプト。`.ps1` は shebang を持たないので既知のマーカーで見る
    PowerShell,
}

/// PowerShell スクリプトだと分かる語（2026-09-01 に `install.ps1` の実物で確認）。
///
/// 実物の先頭は `param(` → `Set-StrictMode -Version Latest` →
/// `$ErrorActionPreference = "Stop"` の順で、どれか 1 つでも当たれば十分。
/// **どれか 1 つに絞らない**のは、上流が冒頭の書き方を変えても
/// 「HTML ではない」判定が生き残るようにするため
const POWERSHELL_MARKERS: &[&str] = &[
    "param(",
    "Set-StrictMode",
    "$ErrorActionPreference",
    "Invoke-RestMethod",
    "Invoke-WebRequest",
    "Write-Output",
    "#Requires",
];

/// 取得した先頭バイト列が本物のインストーラに見えるか（**純粋関数**）
pub fn looks_like_installer(signature: ScriptSignature, head: &[u8]) -> bool {
    let text = String::from_utf8_lossy(head);
    let trimmed = text.trim_start();
    match signature {
        ScriptSignature::Shebang => trimmed.starts_with("#!"),
        // HTML / XML / JSON のエラーページを弾いたうえで PowerShell の語を要求する
        ScriptSignature::PowerShell => {
            !trimmed.starts_with('<')
                && !trimmed.starts_with('{')
                && POWERSHELL_MARKERS.iter().any(|m| trimmed.contains(m))
        }
    }
}

/// 取得したインストーラを走らせるインタプリタ（**データだけ**。解決は呼び出し側）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpreterSpec {
    /// 実行ファイルの候補。前から順に探し、最初に見つかったものを使う
    pub candidates: &'static [&'static str],
    /// どれも見つからなかったときに名前で起こす最後の手段
    /// （Windows の `CreateProcess` と unix の絶対パスはどちらもこれで起動できる）
    pub fallback: &'static str,
    /// スクリプトのパスの**直前**に置く引数
    pub leading_args: &'static [&'static str],
    /// 取得先ファイルの拡張子。PowerShell は `.ps1` 以外を実行しない
    pub script_ext: &'static str,
    /// 取得したものの見分け方
    pub signature: ScriptSignature,
}

impl InterpreterSpec {
    /// スクリプトのパスを与えて argv を組む（`leading_args` + パス）
    pub fn args_for(&self, script: &std::path::Path) -> Vec<std::ffi::OsString> {
        let mut args: Vec<std::ffi::OsString> = self
            .leading_args
            .iter()
            .map(|a| std::ffi::OsString::from(*a))
            .collect();
        args.push(script.as_os_str().to_os_string());
        args
    }
}

/// 1 エージェントぶんのインストール手順
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallRecipe {
    pub agent: AgentKind,
    pub platform: Platform,
    pub source: InstallerSource,
    /// インストール後にランチャーが置かれるパス（`$HOME` からの相対）
    pub launcher_rel: &'static str,
    /// 実体（バージョンごとのディレクトリ）が置かれるパス（`$HOME` からの相対）。
    ///
    /// **単一バイナリの系統は `None`**（agy はランチャー自身が本体なので、
    /// 「本体の置き場所」を別に見せると同じパスを 2 回言うことになる）
    pub payload_rel: Option<&'static str>,
    /// バックグラウンド自動更新が効くか（機械が読む側）
    pub auto_updates: bool,
    /// 以後どう更新されるかの 1 行（人が読む側）。**`auto_updates` の真偽から
    /// 文面を組まない**のは、codex が「自動ではないが自分で知らせる」の第 3 の形だから
    /// （`codex doctor` の `↑ updates … available`。実測）
    pub update_note: &'static str,
    /// tako が実行を代行してよいか。false = 手順を案内するだけ
    pub tako_can_run: bool,
    /// インストールが失敗したときに案内する公式ページ（2026-09-09 に 200 を実測）。
    /// **系統ごとに違う**ので、claude のトラブルシュートを codex の失敗に出さない
    pub troubleshoot_url: &'static str,
    /// インストーラへ渡す環境変数。**対話プロンプトを黙らせるため**に使う
    /// （codex は `CODEX_NON_INTERACTIVE=1` が無いと最後に `Start Codex now?` を
    /// `/dev/tty` から聞いて返ってこない。実測）
    pub installer_env: &'static [(&'static str, &'static str)],
    /// 取得したインストーラの走らせ方（`source.interpreter` は人へ見せる名前、
    /// こちらは実際に組む argv）
    pub runner: InterpreterSpec,
}

impl InstallRecipe {
    /// PATH へ通すべきディレクトリ（`$HOME` からの相対）。
    ///
    /// **ランチャーのパスから導出する**（プラットフォームで `match` しない）。
    /// #989 で codex / agy を足した時点で「どの OS でも `.local/bin`」が
    /// 成り立たなくなった（Windows の codex は
    /// `%LOCALAPPDATA%\Programs\OpenAI\Codex\bin`、agy は `%LOCALAPPDATA%\agy\bin`）。
    /// 導出にすれば置き場所の正本が `launcher_rel` の 1 箇所になり、両者がずれない
    pub fn launcher_dir_rel(&self) -> &'static str {
        self.launcher_rel
            .rsplit_once('/')
            .map(|(dir, _)| dir)
            .unwrap_or(".")
    }

    /// 実際のランチャーパス（home を与えて解決する。**環境変数を読まない**ので
    /// 隔離 HOME のテストがそのまま書ける）
    pub fn launcher_path_in(&self, home: &std::path::Path) -> PathBuf {
        rel_join(home, self.launcher_rel)
    }

    /// PATH へ通すべきディレクトリの絶対パス
    pub fn launcher_dir_in(&self, home: &std::path::Path) -> PathBuf {
        rel_join(home, self.launcher_dir_rel())
    }

    /// 実体ディレクトリの絶対パス（単一バイナリの系統は `None`）
    pub fn payload_dir_in(&self, home: &std::path::Path) -> Option<PathBuf> {
        self.payload_rel.map(|rel| rel_join(home, rel))
    }
}

/// `/` 区切りの相対パスを結合する（Windows でも `\` に解決される）
fn rel_join(home: &std::path::Path, rel: &str) -> PathBuf {
    let mut path = home.to_path_buf();
    for part in rel.split('/') {
        path.push(part);
    }
    path
}

/// 指定プラットフォーム向けのインストール手順。
///
/// **`Platform` を引数で受ける純粋関数**なので、macOS の `cargo test` から
/// Windows 向けの内容も検証できる
pub fn recipe(platform: Platform, agent: AgentKind) -> InstallRecipe {
    match (platform, agent) {
        (Platform::MacOs, AgentKind::Claude) => InstallRecipe {
            agent,
            platform,
            source: InstallerSource {
                url: "https://claude.ai/install.sh",
                interpreter: "bash",
                official_command: "curl -fsSL https://claude.ai/install.sh | bash",
            },
            launcher_rel: ".local/bin/claude",
            payload_rel: Some(".local/share/claude/versions"),
            auto_updates: true,
            update_note: "Claude Code が自分でバックグラウンド更新します",
            troubleshoot_url: "https://code.claude.com/docs/en/troubleshoot-install",
            tako_can_run: true,
            installer_env: &[],
            runner: InterpreterSpec {
                candidates: &["bash"],
                fallback: "/bin/bash",
                leading_args: &[],
                script_ext: "sh",
                signature: ScriptSignature::Shebang,
            },
        },
        (Platform::Windows, AgentKind::Claude) => InstallRecipe {
            agent,
            platform,
            source: InstallerSource {
                url: "https://claude.ai/install.ps1",
                interpreter: "powershell",
                official_command: "irm https://claude.ai/install.ps1 | iex",
            },
            launcher_rel: ".local/bin/claude.exe",
            payload_rel: Some(".local/share/claude/versions"),
            auto_updates: true,
            update_note: "Claude Code が自分でバックグラウンド更新します",
            troubleshoot_url: "https://code.claude.com/docs/en/troubleshoot-install",
            // #1057 で Windows 11 実機の通し実測を経て true へ倒した
            // （記録は `.agent/plans/2026-08-windows-main-merge-wip.md` の #1057 節）
            tako_can_run: true,
            installer_env: &[],
            runner: WINDOWS_POWERSHELL_RUNNER,
        },
        // codex（2026-09-09 実測。モジュールドキュメントの「codex の調査の根拠」）
        (Platform::MacOs, AgentKind::Codex) => InstallRecipe {
            agent,
            platform,
            source: InstallerSource {
                url: "https://chatgpt.com/codex/install.sh",
                // 取得物の shebang は `#!/bin/sh`（公式の 1 行も `| sh`）
                interpreter: "sh",
                official_command: "curl -fsSL https://chatgpt.com/codex/install.sh | sh",
            },
            launcher_rel: ".local/bin/codex",
            payload_rel: Some(".codex/packages/standalone"),
            // 背景更新は無い。`codex doctor` が知らせるだけ（実測）
            auto_updates: false,
            update_note: "codex が新しい版を知らせます（`codex update` で更新します）",
            troubleshoot_url: "https://learn.chatgpt.com/docs/codex/cli",
            tako_can_run: true,
            // これが無いと最後の `Start Codex now? [y/N]` で返ってこない（実測）
            installer_env: &[("CODEX_NON_INTERACTIVE", "1")],
            runner: InterpreterSpec {
                candidates: &["sh"],
                fallback: "/bin/sh",
                leading_args: &[],
                script_ext: "sh",
                signature: ScriptSignature::Shebang,
            },
        },
        (Platform::Windows, AgentKind::Codex) => InstallRecipe {
            agent,
            platform,
            source: InstallerSource {
                url: "https://chatgpt.com/codex/install.ps1",
                interpreter: "powershell",
                official_command: "irm https://chatgpt.com/codex/install.ps1 | iex",
            },
            // `install.ps1` の `$defaultVisibleBinDir`（claude と違い `~\.local\bin` ではない）
            launcher_rel: "AppData/Local/Programs/OpenAI/Codex/bin/codex.exe",
            payload_rel: Some(".codex/packages/standalone"),
            auto_updates: false,
            update_note: "codex が新しい版を知らせます（`codex update` で更新します）",
            troubleshoot_url: "https://learn.chatgpt.com/docs/codex/cli",
            // Windows は状態照会と案内まで（実行代行は #525 の範囲。#989）
            tako_can_run: false,
            installer_env: &[("CODEX_NON_INTERACTIVE", "1")],
            runner: WINDOWS_POWERSHELL_RUNNER,
        },
        // agy（2026-09-09 実測。モジュールドキュメントの「agy の調査の根拠」）
        (Platform::MacOs, AgentKind::Agy) => InstallRecipe {
            agent,
            platform,
            source: InstallerSource {
                url: "https://antigravity.google/cli/install.sh",
                // 取得物の shebang は `#!/bin/bash`（`set -euo pipefail` を使う）
                interpreter: "bash",
                official_command: "curl -fsSL https://antigravity.google/cli/install.sh | bash",
            },
            launcher_rel: ".local/bin/agy",
            // 単一バイナリなので実体ディレクトリが無い
            payload_rel: None,
            auto_updates: true,
            update_note: "Antigravity CLI が実行のたびに自分で背景更新します",
            troubleshoot_url: "https://antigravity.google/docs/cli/troubleshooting",
            tako_can_run: true,
            installer_env: &[],
            runner: InterpreterSpec {
                candidates: &["bash"],
                fallback: "/bin/bash",
                leading_args: &[],
                script_ext: "sh",
                signature: ScriptSignature::Shebang,
            },
        },
        (Platform::Windows, AgentKind::Agy) => InstallRecipe {
            agent,
            platform,
            source: InstallerSource {
                url: "https://antigravity.google/cli/install.ps1",
                interpreter: "powershell",
                official_command: "irm https://antigravity.google/cli/install.ps1 | iex",
            },
            // 公式 docs の Windows 節（`C:\Users\<username>\AppData\Local\agy\bin`）
            launcher_rel: "AppData/Local/agy/bin/agy.exe",
            payload_rel: None,
            auto_updates: true,
            update_note: "Antigravity CLI が実行のたびに自分で背景更新します",
            troubleshoot_url: "https://antigravity.google/docs/cli/troubleshooting",
            // Windows は状態照会と案内まで（実行代行は #525 の範囲。#989）
            tako_can_run: false,
            installer_env: &[],
            runner: WINDOWS_POWERSHELL_RUNNER,
        },
    }
}

/// Windows の `install.ps1` を走らせる形（3 系統で同じなので 1 箇所に置く）。
///
/// - **5.1（`powershell.exe`）を先に置く**。Windows へ必ず同梱されている側を既定に
///   するとマシンごとの差が出ない（pwsh 7 は任意導入）。3 系統の `install.ps1` は
///   どれも `Set-StrictMode` / `Invoke-RestMethod` 止まりなので 5.1 で動く
/// - `-ExecutionPolicy Bypass` は**このプロセスだけ**に効く（マシンの設定は変えない）。
///   公式の `irm | iex` は文字列を食わせるので ExecutionPolicy の対象外だが、
///   ファイルへ落として `-File` で走らせるこちらは既定の `RemoteSigned` に弾かれる
///   （claude で実機実測。#1057）
/// - `-NoProfile` はユーザーの profile を挟まないため
const WINDOWS_POWERSHELL_RUNNER: InterpreterSpec = InterpreterSpec {
    candidates: &["powershell", "pwsh"],
    leading_args: &[
        "-NoLogo",
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
    ],
    fallback: "powershell.exe",
    script_ext: "ps1",
    signature: ScriptSignature::PowerShell,
};

/// この実行環境のプラットフォーム
pub fn current_platform() -> Platform {
    if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::MacOs
    }
}

/// この実行環境向けの手順
pub fn current_recipe(agent: AgentKind) -> InstallRecipe {
    recipe(current_platform(), agent)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 公式 docs（2026-08-21 実測）と食い違ったらここが落ちる。
    /// **経路を推測で変えないための固定**
    #[test]
    fn macosは公式のnativeインストーラを使う() {
        let r = recipe(Platform::MacOs, AgentKind::Claude);
        assert_eq!(r.source.url, "https://claude.ai/install.sh");
        assert_eq!(r.source.interpreter, "bash");
        assert_eq!(
            r.source.official_command,
            "curl -fsSL https://claude.ai/install.sh | bash"
        );
        assert!(r.auto_updates, "native は自動更新が効くのが採用理由");
        assert!(r.tako_can_run, "macOS は tako が実行を代行する");
    }

    /// install.sh は Windows を明示的に弾く（実物で確認）。
    /// **Windows で誤って bash 経路を走らせない**ための固定
    #[test]
    fn windowsはpowershell経路で実行する() {
        let r = recipe(Platform::Windows, AgentKind::Claude);
        assert_eq!(r.source.url, "https://claude.ai/install.ps1");
        assert_eq!(r.source.interpreter, "powershell");
        assert!(r.tako_can_run, "#1057 の実機実測を経て代行する");
        assert!(r.launcher_rel.ends_with(".exe"));
        // bash 経路が混ざっていない（`install.sh` は Windows を明示的に弾くので、
        // ここが bash に化けると必ず失敗する）
        assert!(!r.runner.candidates.contains(&"bash"));
        assert_eq!(r.runner.script_ext, "ps1");
    }

    /// 走らせ方はプラットフォームごとに排他（**macOS から両方を検証する**）
    #[test]
    fn 走らせ方はプラットフォームごとに決まる() {
        let mac = recipe(Platform::MacOs, AgentKind::Claude).runner;
        assert_eq!(mac.candidates, &["bash"]);
        assert!(mac.leading_args.is_empty(), "bash はパスを直接受ける");
        assert_eq!(mac.script_ext, "sh");
        assert_eq!(mac.signature, ScriptSignature::Shebang);

        let win = recipe(Platform::Windows, AgentKind::Claude).runner;
        assert_eq!(win.candidates, &["powershell", "pwsh"]);
        // `-File` はスクリプトパスの直前でなければならない
        assert_eq!(win.leading_args.last(), Some(&"-File"));
        // 既定の ExecutionPolicy（RemoteSigned）で弾かれないようにする（実機実測）
        assert!(win.leading_args.contains(&"Bypass"));
        assert_eq!(win.signature, ScriptSignature::PowerShell);
    }

    #[test]
    fn argvはleading引数のあとにスクリプトパスを置く() {
        let script = std::path::Path::new("/tmp/tako-install.ps1");
        let win = recipe(Platform::Windows, AgentKind::Claude).runner;
        let args = win.args_for(script);
        assert_eq!(
            args.last().map(|a| a.to_string_lossy().to_string()),
            Some(script.display().to_string())
        );
        assert_eq!(args.len(), win.leading_args.len() + 1);

        let mac = recipe(Platform::MacOs, AgentKind::Claude).runner;
        assert_eq!(mac.args_for(script).len(), 1, "bash は引数 1 個");
    }

    /// 取得したものの見分け方（**実物の先頭で確認した形**を固定する）。
    /// プロキシの HTML / JSON エラーページを弾けることが要点
    #[test]
    fn 取得したものがインストーラかを署名で見分ける() {
        // `install.sh`（2026-08-21 実物）
        assert!(looks_like_installer(
            ScriptSignature::Shebang,
            b"#!/bin/bash\nset -e\n"
        ));
        assert!(looks_like_installer(
            ScriptSignature::Shebang,
            b"\n#!/usr/bin/env bash\n"
        ));
        // `install.ps1`（2026-09-01 実物の先頭。shebang を持たない）
        let ps1 = b"param(\n    [Parameter(Position=0)]\n    [string]$Target = \"latest\"\n)\n\nSet-StrictMode -Version Latest\n$ErrorActionPreference = \"Stop\"\n";
        assert!(looks_like_installer(ScriptSignature::PowerShell, ps1));
        // 署名が逆だと通らない（プラットフォームの取り違えを検出する）
        assert!(!looks_like_installer(ScriptSignature::Shebang, ps1));
        assert!(!looks_like_installer(
            ScriptSignature::PowerShell,
            b"#!/bin/bash\nset -e\n"
        ));
        // HTML / JSON のエラーページ・空はどちらの署名でも弾く
        for signature in [ScriptSignature::Shebang, ScriptSignature::PowerShell] {
            assert!(!looks_like_installer(signature, b"<!DOCTYPE html>"));
            assert!(!looks_like_installer(signature, b"<html><body>403"));
            assert!(!looks_like_installer(
                signature,
                b"{\"error\":\"forbidden\"}"
            ));
            assert!(!looks_like_installer(signature, b""));
        }
    }

    #[test]
    fn 設置先はhomeを与えて解決する() {
        let home = std::path::Path::new("/tmp/h");
        let r = recipe(Platform::MacOs, AgentKind::Claude);
        assert_eq!(r.launcher_path_in(home), home.join(".local/bin/claude"));
        assert_eq!(r.launcher_dir_in(home), home.join(".local/bin"));
        assert_eq!(
            r.payload_dir_in(home),
            Some(home.join(".local/share/claude/versions"))
        );
    }

    /// macOS は 3 系統とも `~/.local/bin`（= tako の PATH ブロック 1 個で足りる。#989 の
    /// やること 4）。**Windows は違う**ので、そこを取り違えないように両方を固定する
    #[test]
    fn macosの置き場は3系統とも同じでwindowsは系統ごとに違う() {
        for agent in AgentKind::ALL {
            assert_eq!(
                recipe(Platform::MacOs, agent).launcher_dir_rel(),
                ".local/bin",
                "{} の macOS の置き場",
                agent.as_str()
            );
        }
        assert_eq!(
            recipe(Platform::Windows, AgentKind::Claude).launcher_dir_rel(),
            ".local/bin"
        );
        assert_eq!(
            recipe(Platform::Windows, AgentKind::Codex).launcher_dir_rel(),
            "AppData/Local/Programs/OpenAI/Codex/bin"
        );
        assert_eq!(
            recipe(Platform::Windows, AgentKind::Agy).launcher_dir_rel(),
            "AppData/Local/agy/bin"
        );
    }

    /// 3 系統 × 2 プラットフォームの手順が**全部揃っていて素性が矛盾しない**
    /// （#989。1 マスでも埋め忘れると `recipe` が組めないので、埋まっている中身を見る）
    #[test]
    fn 全系統全プラットフォームの手順が揃っている() {
        let home = std::path::Path::new("/tmp/h");
        for platform in [Platform::MacOs, Platform::Windows] {
            for agent in AgentKind::ALL {
                let r = recipe(platform, agent);
                let who = format!("{}/{}", platform.as_str(), agent.as_str());
                assert_eq!(r.agent, agent, "{who}");
                assert_eq!(r.platform, platform, "{who}");
                assert!(r.source.url.starts_with("https://"), "{who} は https 必須");
                // 公式の 1 行に取得元 URL がそのまま出ている（案内と実行がずれない）
                assert!(
                    r.source.official_command.contains(r.source.url),
                    "{who} の official_command に URL が無い"
                );
                assert!(!r.update_note.is_empty(), "{who} の更新の説明が空");
                assert!(
                    r.troubleshoot_url.starts_with("https://"),
                    "{who} のトラブルシュート URL"
                );
                // ランチャーはコマンド名で終わる（`.exe` は Windows だけ）
                assert!(
                    r.launcher_rel.ends_with(agent.as_str())
                        || r.launcher_rel.ends_with(&format!("{}.exe", agent.as_str())),
                    "{who} の launcher_rel={}",
                    r.launcher_rel
                );
                assert_eq!(
                    r.launcher_rel.ends_with(".exe"),
                    platform == Platform::Windows,
                    "{who} の拡張子"
                );
                // 置き場所は必ずホームの中（管理者権限が要らないことの裏付け）
                assert!(
                    !r.launcher_rel.starts_with('/') && !r.launcher_rel.contains(':'),
                    "{who} の置き場所がホームの外"
                );
                assert!(r.launcher_dir_in(home).starts_with(home), "{who}");
                if let Some(payload) = r.payload_dir_in(home) {
                    assert!(payload.starts_with(home), "{who} の本体がホームの外");
                }
                assert!(!agent.product().is_empty(), "{who} の製品名が空");
            }
        }
    }

    /// 失敗の案内は系統ごとに違う（claude のページを codex の失敗に出さない。#989）
    #[test]
    fn トラブルシュートの案内は系統ごとに違う() {
        for platform in [Platform::MacOs, Platform::Windows] {
            let urls: Vec<&str> = AgentKind::ALL
                .into_iter()
                .map(|a| recipe(platform, a).troubleshoot_url)
                .collect();
            for (i, a) in urls.iter().enumerate() {
                for b in urls.iter().skip(i + 1) {
                    assert_ne!(a, b, "{platform:?}: 案内先が同じ");
                }
            }
            // 他系統の失敗に claude のページを出さない（実際に踏んだ退行。#989）
            assert!(!recipe(platform, AgentKind::Codex)
                .troubleshoot_url
                .contains("claude"));
            assert!(!recipe(platform, AgentKind::Agy)
                .troubleshoot_url
                .contains("claude"));
        }
    }

    /// 単一バイナリの系統は「本体の置き場所」を持たない（同じパスを 2 回言わない）
    #[test]
    fn 単一バイナリのagyは本体の置き場所を持たない() {
        for platform in [Platform::MacOs, Platform::Windows] {
            assert!(recipe(platform, AgentKind::Agy).payload_rel.is_none());
            assert!(recipe(platform, AgentKind::Claude).payload_rel.is_some());
            assert!(recipe(platform, AgentKind::Codex).payload_rel.is_some());
        }
    }

    /// 代行してよいのは**実機で通した組み合わせだけ**（過大申告しない。#989 / #1057）
    #[test]
    fn windowsで代行するのはclaudeだけ() {
        for agent in AgentKind::ALL {
            assert!(
                recipe(Platform::MacOs, agent).tako_can_run,
                "macOS は 3 系統とも代行する（{}）",
                agent.as_str()
            );
        }
        assert!(recipe(Platform::Windows, AgentKind::Claude).tako_can_run);
        assert!(
            !recipe(Platform::Windows, AgentKind::Codex).tako_can_run,
            "Windows の codex は案内まで（#525）"
        );
        assert!(
            !recipe(Platform::Windows, AgentKind::Agy).tako_can_run,
            "Windows の agy は案内まで（#525）"
        );
    }

    /// codex のインストーラは最後に `Start Codex now? [y/N]` を聞く（実測）。
    /// **この env が落ちると代行が返ってこない**ので、両プラットフォームで固定する
    #[test]
    fn codexのインストーラは非対話envを必ず渡す() {
        for platform in [Platform::MacOs, Platform::Windows] {
            assert_eq!(
                recipe(platform, AgentKind::Codex).installer_env,
                &[("CODEX_NON_INTERACTIVE", "1")],
                "{}",
                platform.as_str()
            );
            // 他の系統は対話プロンプトを持たないので env を足さない（推測で増やさない）
            assert!(recipe(platform, AgentKind::Claude).installer_env.is_empty());
            assert!(recipe(platform, AgentKind::Agy).installer_env.is_empty());
        }
    }

    /// codex は「自動更新ではないが自分で知らせる」第 3 の形（実測）。
    /// `auto_updates` の真偽から文面を組むと嘘になるので、両方を別に固定する
    #[test]
    fn 更新のされ方は系統ごとに違う() {
        let mac = |a| recipe(Platform::MacOs, a);
        assert!(mac(AgentKind::Claude).auto_updates);
        assert!(mac(AgentKind::Agy).auto_updates);
        assert!(!mac(AgentKind::Codex).auto_updates);
        assert!(mac(AgentKind::Codex).update_note.contains("codex update"));
    }

    #[test]
    fn 系統名は文字列と往復する() {
        for agent in AgentKind::ALL {
            assert_eq!(AgentKind::parse(agent.as_str()), Some(agent));
        }
        assert_eq!(AgentKind::parse("ollama"), None);
        assert_eq!(AgentKind::parse(""), None);
    }
}
