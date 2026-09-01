//! エージェント CLI の公式インストール手順（抽象境界 B17。#868 / #525）
//!
//! 「Claude Code をどう入れるか」のプラットフォーム差をここへ閉じ込める。
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

use super::support::Platform;
use std::path::PathBuf;

/// 自動インストールに対応するエージェント CLI。
/// codex / agy は将来拡張（#868 の Out of scope）で、いまは列挙だけしない
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    Claude,
}

impl AgentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
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
    /// 実体（バージョンごとのディレクトリ）が置かれるパス（`$HOME` からの相対）
    pub payload_rel: &'static str,
    /// バックグラウンド自動更新が効くか
    pub auto_updates: bool,
    /// tako が実行を代行してよいか。false = 手順を案内するだけ
    pub tako_can_run: bool,
    /// 取得したインストーラの走らせ方（`source.interpreter` は人へ見せる名前、
    /// こちらは実際に組む argv）
    pub runner: InterpreterSpec,
}

impl InstallRecipe {
    /// PATH へ通すべきディレクトリ（`$HOME` からの相対）
    pub fn launcher_dir_rel(&self) -> &'static str {
        match self.platform {
            // どちらも `<home>/.local/bin/<name>` 形式
            Platform::MacOs | Platform::Windows => ".local/bin",
        }
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

    /// 実体ディレクトリの絶対パス
    pub fn payload_dir_in(&self, home: &std::path::Path) -> PathBuf {
        rel_join(home, self.payload_rel)
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
            payload_rel: ".local/share/claude/versions",
            auto_updates: true,
            tako_can_run: true,
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
            payload_rel: ".local/share/claude/versions",
            auto_updates: true,
            // #1057 で Windows 11 実機の通し実測を経て true へ倒した
            // （記録は `.agent/plans/2026-08-windows-main-merge-wip.md` の #1057 節）
            tako_can_run: true,
            runner: InterpreterSpec {
                // **5.1（`powershell.exe`）を先に置く**。Windows へ必ず同梱されている
                // 側を既定にするとマシンごとの差が出ない（pwsh 7 は任意導入）。
                // `install.ps1` は 5.1 / 7 のどちらでも動く（`Set-StrictMode` /
                // `Invoke-RestMethod` / `Get-FileHash` はいずれも 5.1 に在る）
                candidates: &["powershell", "pwsh"],
                fallback: "powershell.exe",
                // `-ExecutionPolicy Bypass` は**このプロセスだけ**に効く（マシンの
                // 設定は変えない）。公式の `irm | iex` は文字列を食わせるので
                // ExecutionPolicy の対象外だが、ファイルへ落として `-File` で
                // 走らせるこちらは既定の `RemoteSigned` に弾かれる（実機実測）。
                // `-NoProfile` はユーザーの profile を挟まないため
                leading_args: &[
                    "-NoLogo",
                    "-NoProfile",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                ],
                script_ext: "ps1",
                signature: ScriptSignature::PowerShell,
            },
        },
    }
}

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
            home.join(".local/share/claude/versions")
        );
    }

    #[test]
    fn 両プラットフォームで同じランチャーディレクトリを使う() {
        for p in [Platform::MacOs, Platform::Windows] {
            assert_eq!(
                recipe(p, AgentKind::Claude).launcher_dir_rel(),
                ".local/bin"
            );
        }
    }
}
