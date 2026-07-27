//! シェル統合（FR-2.4.1）— OSC 7 / 133 を発行するスクリプトの書き出しと自動注入
//!
//! `shell-integration/` のスクリプトをバイナリへ埋め込み、初回 spawn 時にデータ
//! ディレクトリへ書き出して、シェルが拾う環境変数を組み立てる。
//! **シェル判定はしない**: zsh は `ZDOTDIR`、bash は `PROMPT_COMMAND`、fish は
//! `XDG_DATA_DIRS` しか見ないため、3 点セットを常時注入しても互いに無害。
//! 無効化は `TAKO_NO_SHELL_INTEGRATION=1`（FR-2.4.4 の設定 UI までの暫定）。
//! Windows（PowerShell）は Phase 6 で対応する。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::paths::data_dir;

const ZSH_ZSHENV: &str = include_str!("../shell-integration/zshenv.zsh");
const BASH_SCRIPT: &str = include_str!("../shell-integration/tako.bash");
const FISH_SCRIPT: &str = include_str!("../shell-integration/tako.fish");

/// 同梱している zsh-autosuggestions（MIT）の本体。出所とライセンスは
/// `shell-integration/zsh-autosuggestions/PROVENANCE.md` と `THIRD-PARTY-NOTICES.md`。
/// **実行時ダウンロードはしない**（オフラインでも動き、供給元の改変にも影響されない）
const ZSH_AUTOSUGGESTIONS: &str =
    include_str!("../shell-integration/zsh-autosuggestions/zsh-autosuggestions.zsh");
const ZSH_AUTOSUGGESTIONS_LICENSE: &str =
    include_str!("../shell-integration/zsh-autosuggestions/LICENSE");

/// 同梱している zsh-autosuggestions のバージョン（更新時は PROVENANCE.md と揃える）
pub const AUTOSUGGEST_VERSION: &str = "v0.7.1";

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

/// スクリプト一式を置くディレクトリ（`<data_dir>/shell-integration`）
pub fn integration_root() -> Option<PathBuf> {
    data_dir().map(|d| d.join("shell-integration"))
}

/// 入力予測の ON/OFF 状態（Issue #600）。zsh 側が毎プロンプト読む値と同じもの。
///
/// **なぜ環境変数ではなくファイルなのか**: 環境変数は spawn 時に凍結するので、
/// 設定を切り替えても既存ペインのシェルには一生届かない。zsh 側は毎プロンプト
/// このファイルを読むので、稼働中のシェルにも次のプロンプトから反映される
pub fn autosuggest_state() -> bool {
    integration_root()
        .map(|r| autosuggest_state_in(&r))
        .unwrap_or(true)
}

/// 状態ファイルの中身。**不在は ON**（既定 ON。Issue #600）
pub fn autosuggest_state_in(root: &Path) -> bool {
    match std::fs::read_to_string(root.join("autosuggest")) {
        Ok(s) => s.trim() != "off",
        Err(_) => true,
    }
}

/// 状態ファイルを書く（`root` は `integration_root()` 相当）
pub fn write_autosuggest_state_in(root: &Path, enabled: bool) -> std::io::Result<()> {
    std::fs::create_dir_all(root)?;
    // 部分書き込みを zsh に読ませないよう tmp → rename（settings.json と同方式）。
    // tmp 名はプロセス固有にする（プライマリ / セカンダリが同じ data_dir を共有しうるため。
    // 同時書き込みでも「どちらかの完全な値」になり、途中の状態は読まれない）
    let path = root.join("autosuggest");
    let tmp = root.join(format!("autosuggest.{}.tmp", std::process::id()));
    std::fs::write(&tmp, if enabled { "on" } else { "off" })?;
    std::fs::rename(&tmp, &path)
}

/// 入力予測の ON/OFF をシェル側へ反映する。データディレクトリを解決できなければ何もしない
pub fn set_autosuggest(enabled: bool) {
    let Some(root) = integration_root() else {
        return;
    };
    if let Err(e) = write_autosuggest_state_in(&root, enabled) {
        tracing::warn!("入力予測の状態を書き出せない: {e}");
    }
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

    // 入力予測（Issue #600）。zshenv.zsh が最初のプロンプトで読み込む。
    // 読み込むかどうかは `autosuggest` 状態ファイル側で決めるので、置くのは常に行う
    let autosuggest_dir = root.join("zsh-autosuggestions");
    std::fs::create_dir_all(&autosuggest_dir)?;
    std::fs::write(
        autosuggest_dir.join("zsh-autosuggestions.zsh"),
        ZSH_AUTOSUGGESTIONS,
    )?;
    // MIT の義務（著作権表示とライセンス全文の添付）を配置先でも満たす
    std::fs::write(autosuggest_dir.join("LICENSE"), ZSH_AUTOSUGGESTIONS_LICENSE)?;

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
        // 入力予測の同梱物も一緒に置かれる（Issue #600）
        let plugin_dir = zdotdir
            .parent()
            .expect("統合ディレクトリ")
            .join("zsh-autosuggestions");
        assert!(plugin_dir.join("zsh-autosuggestions.zsh").is_file());
        assert!(plugin_dir.join("LICENSE").is_file());
    }

    /// #600: 同梱物が本物であること。取り違え・空ファイル化を検出する
    #[test]
    fn 同梱したzsh_autosuggestionsが本物でバージョン表記と一致する() {
        assert!(
            ZSH_AUTOSUGGESTIONS.contains("_zsh_autosuggest_start"),
            "同梱物が zsh-autosuggestions 本体ではない"
        );
        // 上流はファイル冒頭にバージョンを書いている（`# v0.7.1`）
        assert!(
            ZSH_AUTOSUGGESTIONS.contains(&format!("# {AUTOSUGGEST_VERSION}")),
            "AUTOSUGGEST_VERSION({AUTOSUGGEST_VERSION}) が同梱物のバージョンと食い違っている"
        );
        assert!(
            ZSH_AUTOSUGGESTIONS_LICENSE.contains("Permission is hereby granted"),
            "MIT ライセンス全文が同梱されていない"
        );
    }

    /// #600: zshenv 側の不変条件。壊すと「tako の外に漏れる」「二重注入する」
    /// といった事故になるので、構造をテストで固定する
    #[test]
    fn zshenvの入力予測ブロックが不変条件を満たす() {
        // tako のペインの中でしか読み込まない（要件 2: 外の zsh に影響ゼロ）
        assert!(ZSH_ZSHENV.contains("-o interactive && -n ${TAKO_PANE_ID-}"));
        // 二重注入ガード: ユーザーが先に入れていたら手を出さない（要件 3）
        assert!(ZSH_ZSHENV.contains("_zsh_autosuggest_start"));
        assert!(ZSH_ZSHENV.contains("_tako_as_owner=user"));
        // 読み込みは precmd（= .zshrc の後）まで遅らせる。ここが .zshenv 直下に
        // 戻ると 1) 二重注入ガードが効かず 2) 他プラグインを包めなくなる
        assert!(ZSH_ZSHENV.contains("precmd_functions+=(_tako_autosuggest_sync)"));
        // 状態ファイルを毎プロンプト見る（既存ペインにも反映される）
        assert!(ZSH_ZSHENV.contains("_tako_as_state"));
        // 明示的な無効化の逃げ道
        assert!(ZSH_ZSHENV.contains("TAKO_NO_AUTOSUGGESTIONS"));
    }

    #[test]
    fn 入力予測の状態ファイルは往復し不在は既定on() {
        let root = std::env::temp_dir().join(format!("tako-as-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // 不在 = ON（既定 ON。Issue #600）
        assert!(autosuggest_state_in(&root));

        write_autosuggest_state_in(&root, false).expect("書ける");
        assert_eq!(
            std::fs::read_to_string(root.join("autosuggest")).unwrap(),
            "off"
        );
        assert!(!autosuggest_state_in(&root));

        write_autosuggest_state_in(&root, true).expect("書ける");
        assert_eq!(
            std::fs::read_to_string(root.join("autosuggest")).unwrap(),
            "on"
        );
        assert!(autosuggest_state_in(&root));

        // 壊れた値は ON 側へ倒す（予測が出ないより出る方が既定に忠実）
        std::fs::write(root.join("autosuggest"), "garbage").unwrap();
        assert!(autosuggest_state_in(&root));
        let _ = std::fs::remove_dir_all(&root);
    }
}
