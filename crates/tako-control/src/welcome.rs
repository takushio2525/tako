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

use std::io;
use std::path::{Path, PathBuf};

use crate::settings::Settings;

/// バナーを出すか（純関数。テストと実運用が同じ判定を通る）。
///
/// `settings_path` はファイルの実在だけを見る。`settings` は破損時に既定値へ
/// 落ちている可能性があるため、判定の主軸はファイルの存在に置く。
pub fn should_show(settings_path: &Path, settings: &Settings) -> bool {
    !settings_path.exists() && !settings.welcome_dismissed
}

/// 起動時にバナーを出すか（実パス版）。
///
/// データディレクトリを解決できない環境では出さない（設定を永続化できない =
/// 「閉じても毎回出る」になるため、出さない側へ倒す）。
pub fn should_show_on_launch() -> bool {
    match crate::settings::settings_path() {
        Some(path) => should_show(&path, &crate::settings::load()),
        None => false,
    }
}

/// 初回起動か（settings.json がまだ無い）。status 応答の診断用
pub fn is_first_launch() -> bool {
    crate::settings::settings_path().is_some_and(|p| !p.exists())
}

/// 「以後出さない」を永続化する
pub fn mark_dismissed() -> io::Result<PathBuf> {
    let mut settings = crate::settings::load();
    settings.welcome_dismissed = true;
    crate::settings::save(&settings)
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
    // A/B: `TAKO_899_LEGACY=1` で #899 以前（POSIX 決め打ちのクォート）へ戻す。
    // 同一バイナリで実機の before/after を取るための逃げ道
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
}
