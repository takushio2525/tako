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
    /// （Windows は #525 で実機検証してから true にする）
    pub tako_can_run: bool,
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
            // #525 で Windows 実機の実測を経てから true にする。
            // 実機で確かめていない手順を tako が黙って走らせない
            tako_can_run: false,
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
    fn windowsはpowershell経路で自動実行しない() {
        let r = recipe(Platform::Windows, AgentKind::Claude);
        assert_eq!(r.source.url, "https://claude.ai/install.ps1");
        assert_eq!(r.source.interpreter, "powershell");
        assert!(
            !r.tako_can_run,
            "実機未検証の手順を tako が黙って実行しない（#525 で倒す）"
        );
        assert!(r.launcher_rel.ends_with(".exe"));
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
