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
    format!(
        "{} {subcommand}",
        shell_quote(&crate::dispatch::resolve_tako_binary())
    )
}

/// POSIX シェル向けの最小クォート。安全な文字だけなら素のまま返す
fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-/".contains(&b));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
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
    fn shell_quoteは安全な文字をそのまま通す() {
        assert_eq!(
            shell_quote("/Applications/tako.app/Contents/MacOS/tako"),
            "/Applications/tako.app/Contents/MacOS/tako"
        );
        assert_eq!(shell_quote("tako"), "tako");
    }

    #[test]
    fn shell_quoteは空白とクォートを閉じ込める() {
        assert_eq!(shell_quote("/My Apps/tako"), "'/My Apps/tako'");
        assert_eq!(shell_quote("/a'b/tako"), r"'/a'\''b/tako'");
        // シェル注入に使える文字はクォートの内側へ入る
        assert_eq!(shell_quote("/x; rm -rf /"), "'/x; rm -rf /'");
    }

    #[test]
    fn 実行コマンド行はサブコマンドを付ける() {
        let line = launch_command_line("setup");
        assert!(line.ends_with(" setup"), "実際: {line}");
        assert!(!line.starts_with(' '), "バイナリ部が空: {line}");
    }
}
