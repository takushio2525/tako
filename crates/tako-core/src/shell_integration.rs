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

/// tako CLI の実行ファイル名（Windows は `tako.exe`）
fn cli_file_name() -> String {
    format!("tako{}", std::env::consts::EXE_SUFFIX)
}

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

/// tako CLI 実体が置かれているディレクトリ（Issue #601）。
///
/// **実行中バイナリの隣を見て決める**ので、`.app` 起動なら `Contents/MacOS`、
/// dev ビルド（`cargo run -p tako-app`）なら `target/debug` を指す。パスは直書きしない。
/// CLI が隣に無い（dev で `tako-cli` を未ビルド等）なら `None` = 注入するものが無い
pub fn cli_dir() -> Option<PathBuf> {
    resolve_cli_dir(std::env::current_exe().ok().as_deref(), &|p| p.is_file())
}

/// `cli_dir` の純粋関数版。`exists` を差し替えられるのでテストできる
fn resolve_cli_dir(exe: Option<&Path>, exists: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    let dir = exe?.parent()?;
    // 改行を含むパスは 1 行 1 値の状態ファイルに載せられない（シェル側が誤読する）
    if dir.to_string_lossy().contains(['\n', '\r']) {
        return None;
    }
    exists(&dir.join(cli_file_name())).then(|| dir.to_path_buf())
}

/// 状態ファイルに記録されている CLI ディレクトリ。**空 = 注入しない**
pub fn cli_dir_state_in(root: &Path) -> Option<PathBuf> {
    let raw = std::fs::read_to_string(root.join("cli-dir")).ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}

/// CLI ディレクトリをシェルが読む状態ファイルへ書く（`root` は `integration_root()` 相当）。
///
/// **なぜ環境変数ではなくファイルなのか**: 入力予測（#600）と同じ理由に加えて、
/// PATH そのものを spawn 時の env で足すと「ユーザーが自分で通した tako があるか」を
/// tako-app のプロセス環境（Dock 起動だと最小構成）で判定することになり必ず誤る。
/// 判定はシェル側が rc を読み終えた後に行うのが唯一正しく、そのとき参照する値をここに置く
pub fn write_cli_dir_in(root: &Path, dir: Option<&Path>) -> std::io::Result<()> {
    std::fs::create_dir_all(root)?;
    // 部分書き込みをシェルに読ませないよう tmp → rename（autosuggest と同方式）
    let path = root.join("cli-dir");
    let tmp = root.join(format!("cli-dir.{}.tmp", std::process::id()));
    std::fs::write(
        &tmp,
        dir.map(|d| d.display().to_string()).unwrap_or_default(),
    )?;
    std::fs::rename(&tmp, &path)
}

/// 起動時に CLI ディレクトリを解決し直してシェル側へ反映する。
/// アプリの置き場所が変わっても（zip 展開先 → /Applications 等）次に開くシェルから正しくなる
pub fn refresh_cli_dir() {
    let Some(root) = integration_root() else {
        return;
    };
    if let Err(e) = write_cli_dir_in(&root, cli_dir().as_deref()) {
        tracing::warn!("tako CLI のディレクトリを書き出せない: {e}");
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

    // tako CLI の PATH 注入（Issue #601）。シェル側が最初のプロンプトで読む。
    // ここで失敗してもシェル統合（OSC 7 / 133）自体は成立させたいので致命扱いしない
    if let Err(e) = write_cli_dir_in(&root, cli_dir().as_deref()) {
        tracing::warn!("tako CLI のディレクトリを書き出せない: {e}");
    }

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

    /// #601: CLI ディレクトリは**実行中バイナリの隣**から決める。
    /// .app / dev ビルドのどちらでも同じ規則で解け、直書きのパスは持たない
    #[test]
    fn cliディレクトリは実行中バイナリの隣から解決する() {
        let app = PathBuf::from("/Applications/tako.app/Contents/MacOS/tako-app");
        let dev = PathBuf::from("/Users/x/dev/tako/target/debug/tako-app");
        // 隣に CLI があるときだけそのディレクトリを返す
        assert_eq!(
            resolve_cli_dir(Some(&app), &|p| p
                == Path::new("/Applications/tako.app/Contents/MacOS/tako")),
            Some(PathBuf::from("/Applications/tako.app/Contents/MacOS"))
        );
        assert_eq!(
            resolve_cli_dir(Some(&dev), &|p| p
                == Path::new("/Users/x/dev/tako/target/debug/tako")),
            Some(PathBuf::from("/Users/x/dev/tako/target/debug"))
        );
        // CLI が隣に無い（dev で tako-cli 未ビルド）なら注入しない
        assert_eq!(resolve_cli_dir(Some(&dev), &|_| false), None);
        assert_eq!(resolve_cli_dir(None, &|_| true), None);
        // 改行入りのパスは状態ファイル（1 行 1 値）に載せられないので捨てる
        let weird = PathBuf::from("/tmp/a\nb/tako-app");
        assert_eq!(resolve_cli_dir(Some(&weird), &|_| true), None);
    }

    #[test]
    fn cliディレクトリの状態ファイルは往復し空は注入なし() {
        let root = std::env::temp_dir().join(format!("tako-cli-dir-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // 不在 = 注入しない
        assert_eq!(cli_dir_state_in(&root), None);

        let dir = PathBuf::from("/Applications/tako.app/Contents/MacOS");
        write_cli_dir_in(&root, Some(&dir)).expect("書ける");
        assert_eq!(cli_dir_state_in(&root), Some(dir));

        // 解決できなければ空にする（古い値を残してシェルに誤った PATH を足させない）
        write_cli_dir_in(&root, None).expect("書ける");
        assert_eq!(std::fs::read_to_string(root.join("cli-dir")).unwrap(), "");
        assert_eq!(cli_dir_state_in(&root), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// #601: シェル側の不変条件。壊すと「外の PATH を書き換える」「ユーザーが自分で
    /// 通した tako を押しのける」といった事故になるので、構造をテストで固定する
    #[test]
    fn シェル統合のpath注入が不変条件を満たす() {
        // zsh: 判定は .zshrc の後（precmd）。ここが .zshenv 直下へ戻ると、
        // .zprofile / .zshrc で PATH を組むユーザーを必ず誤判定する
        assert!(ZSH_ZSHENV.contains("precmd_functions+=(_tako_path_sync)"));
        // 非対話（コマンドペイン・エージェントペイン）はフックが回らないので直接呼ぶ
        assert!(ZSH_ZSHENV.contains("if [[ ! -o interactive && -n ${TAKO_PANE_ID-} ]]; then"));
        // 足すのは末尾のみ（zsh の path 配列 += / bash・fish は $PATH の後ろ）
        assert!(ZSH_ZSHENV.contains("path+=(\"$dir\")"));
        assert!(BASH_SCRIPT.contains("export PATH=\"$PATH:$_tako_cli_dir\""));
        assert!(FISH_SCRIPT.contains("set -gx PATH $PATH $dir"));
        for (name, script) in [
            ("zsh", ZSH_ZSHENV),
            ("bash", BASH_SCRIPT),
            ("fish", FISH_SCRIPT),
        ] {
            // 状態ファイルを読む（tako が起動時に書いた実体の場所）
            assert!(
                script.contains("cli-dir"),
                "{name}: 状態ファイルを見ていない"
            );
            // 明示的な無効化の逃げ道
            assert!(
                script.contains("TAKO_NO_PATH_INJECTION"),
                "{name}: 逃げ道が無い"
            );
        }
        // 既存の tako を尊重する二重追加ガード（シェルごとの慣用句）
        assert!(ZSH_ZSHENV.contains("(( ${+commands[tako]} )) && return 0"));
        assert!(BASH_SCRIPT.contains("! command -v tako >/dev/null 2>&1"));
        assert!(FISH_SCRIPT.contains("type -q tako; and return"));
        // fish は自分の置き場所から統合ルートを逆算する。write_scripts の配置と一致させる
        assert!(FISH_SCRIPT.contains("/fish-data/fish/vendor_conf\\.d/[^/]*$"));
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
