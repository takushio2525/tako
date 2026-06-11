//! シェル統合（FR-2.4.1）— OSC 7 / 133 を発行するスクリプトの書き出しと自動注入
//!
//! `shell-integration/` のスクリプトをバイナリへ埋め込み、初回 spawn 時にデータ
//! ディレクトリへ書き出して、シェルが拾う環境変数を組み立てる。
//! **シェル判定はしない**: zsh は `ZDOTDIR`、bash は `PROMPT_COMMAND`、fish は
//! `XDG_DATA_DIRS` しか見ないため、3 点セットを常時注入しても互いに無害。
//! 無効化は `TAKO_NO_SHELL_INTEGRATION=1`（FR-2.4.4 の設定 UI までの暫定）。
//! Windows（PowerShell）は Phase 6 で対応する。

use std::path::PathBuf;
use std::sync::OnceLock;

const ZSH_ZSHENV: &str = include_str!("../shell-integration/zshenv.zsh");
const BASH_SCRIPT: &str = include_str!("../shell-integration/tako.bash");
const FISH_SCRIPT: &str = include_str!("../shell-integration/tako.fish");

/// spawn する子シェルに注入する統合用環境変数。プロセス内で一度だけ書き出して使い回す
pub fn env() -> &'static [(String, String)] {
    static ENV: OnceLock<Vec<(String, String)>> = OnceLock::new();
    ENV.get_or_init(|| {
        if std::env::var_os("TAKO_NO_SHELL_INTEGRATION").is_some_and(|v| !v.is_empty()) {
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

/// スクリプト一式をデータディレクトリへ書き出し、注入 env を返す
fn write_scripts() -> std::io::Result<Vec<(String, String)>> {
    let Some(base) = data_dir() else {
        return Ok(Vec::new());
    };
    let root = base.join("shell-integration");

    let zsh_dir = root.join("zsh");
    std::fs::create_dir_all(&zsh_dir)?;
    std::fs::write(zsh_dir.join(".zshenv"), ZSH_ZSHENV)?;

    let bash_path = root.join("tako.bash");
    std::fs::write(&bash_path, BASH_SCRIPT)?;

    let fish_conf_dir = root.join("fish-data/fish/vendor_conf.d");
    std::fs::create_dir_all(&fish_conf_dir)?;
    std::fs::write(fish_conf_dir.join("tako.fish"), FISH_SCRIPT)?;

    let mut env = Vec::new();
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
    Ok(env)
}

/// tako のデータディレクトリ（スクリプト書き出し先）
fn data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(|h| PathBuf::from(h).join("Library/Application Support/tako"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|h| !h.is_empty())
                    .map(|h| PathBuf::from(h).join(".local/share"))
            })
            .map(|d| d.join("tako"))
    }
    #[cfg(windows)]
    {
        // Phase 6（PowerShell 統合）で対応
        None
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
    }
}
