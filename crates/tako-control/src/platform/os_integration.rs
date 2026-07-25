//! OS シェル連携（抽象境界 B8）
//!
//! 「ファイルマネージャで表示」「既定アプリで開く」「URL を開く」「ゴミ箱へ移す」
//! 「通知を出す」など、**OS のシェルに委ねる操作**をここへ集約する。
//!
//! - macOS: `open` 系（`-R` / `-a` / `-t` / `-n`）と `osascript`
//! - Windows: URL は `cmd /C start`（既存挙動を引き継ぐ）。残りは `ShellExecuteW` /
//!   `SHFileOperation` での実装待ち（#522 の Windows 実機タスク）
//! - その他 unix: URL は `xdg-open`
//!
//! 呼び出し側（`dispatch` / UI / CLI）はこのモジュールだけを見る。
//! **呼び出し側に `#[cfg]` を書かない**（`.agent/plans/2026-07-windows-port-architecture.md` 原則 1）。
//!
//! ## ここに置かないもの
//!
//! - **権限昇格**（`osascript … with administrator privileges`）は B9（スリープ防止 =
//!   `sleep_guard`）の内側に留める。「OS シェルに何かを開かせる」操作ではないうえ、
//!   汎用の昇格 API を B8 に置くと危険な踏み台になる。Windows の B9 実装
//!   （`SetThreadExecutionState`）は sudoers 相当を必要としないため、そもそも対応物が無い

use std::path::{Path, PathBuf};

/// ファイルマネージャ（Finder / エクスプローラー）で対象を選択表示する
pub fn reveal(path: &Path) -> Result<(), String> {
    imp::reveal(path)
}

/// 既定アプリで開く
pub fn open_default(path: &Path) -> Result<(), String> {
    imp::open_default(path)
}

/// アプリ名（またはアプリのパス）を指定して開く
pub fn open_with(app: &str, path: &Path) -> Result<(), String> {
    imp::open_with(app, path)
}

/// 既定のテキストエディタで開く（macOS の `open -t` 相当）
pub fn open_in_text_editor(path: &Path) -> Result<(), String> {
    imp::open_in_text_editor(path)
}

/// URL を既定ブラウザ / ハンドラで開く（起動するだけで完了は待たない）。
/// `x-apple.systempreferences:` のような OS 固有スキームもここを通す
pub fn open_url(url: &str) -> Result<(), String> {
    imp::open_url(url)
}

/// URL を開き、**ハンドラの終了ステータスまで待つ**。
/// 候補 URL を順に試して最初に成功したものを採る用途（FDA のシステム設定パネル）で使う。
/// 成功可否が要らない場合は [`open_url`] を使う（待たない分ブロックしない）
pub fn open_url_wait(url: &str) -> Result<(), String> {
    imp::open_url_wait(url)
}

/// アプリケーションを**新しいプロセスとして**起動する（macOS の `open -n` 相当）。
/// 自動更新後の再起動（B14）が使う
pub fn open_new_instance(app: &Path) -> Result<(), String> {
    imp::open_new_instance(app)
}

/// OS のアプリ選択ダイアログを出し、選ばれたアプリのパスを返す。
/// キャンセルはエラー（呼び出し側は無視してよい）
pub fn pick_application() -> Result<PathBuf, String> {
    imp::pick_application()
}

/// パスをゴミ箱へ移す。
///
/// macOS は Finder に委ねる（ゴミ箱から戻せる = ユーザーの期待どおり）。
/// **パスは AppleScript ソースへ連結せず `osascript` の argv として渡す**ため、
/// ファイル名に含まれる `"` `\` 改行がスクリプト構文へ割り込む余地が構造的に無い
/// （#80。エスケープの正しさに依存しない）。
/// ゴミ箱の概念が無い環境では通常の削除へ劣化する
pub fn move_to_trash(path: &Path) -> Result<(), String> {
    imp::move_to_trash(path)
}

/// デスクトップ通知を出す（best-effort。失敗しても無視してよい）。
/// メッセージは argv で渡すため本文の内容がスクリプト構文へ割り込まない
pub fn notify(title: &str, message: &str) {
    imp::notify(title, message);
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    pub fn reveal(path: &Path) -> Result<(), String> {
        spawn_open(&["-R".as_ref(), path.as_os_str()], "Finder を開けない")
    }

    pub fn open_default(path: &Path) -> Result<(), String> {
        spawn_open(&[path.as_os_str()], "デフォルトアプリで開けない")
    }

    pub fn open_with(app: &str, path: &Path) -> Result<(), String> {
        spawn_open(
            &["-a".as_ref(), app.as_ref(), path.as_os_str()],
            &format!("アプリ '{app}' で開けない"),
        )
    }

    pub fn open_in_text_editor(path: &Path) -> Result<(), String> {
        spawn_open(
            &["-t".as_ref(), path.as_os_str()],
            "テキストエディタで開けない",
        )
    }

    pub fn open_url(url: &str) -> Result<(), String> {
        spawn_open(&[url.as_ref()], "URL を開けない")
    }

    pub fn open_url_wait(url: &str) -> Result<(), String> {
        let status = std::process::Command::new("open")
            .arg(url)
            .status()
            .map_err(|e| format!("open コマンドの実行に失敗: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("URL を開けない（終了コード {status}）: {url}"))
        }
    }

    pub fn open_new_instance(app: &Path) -> Result<(), String> {
        spawn_open(&["-n".as_ref(), app.as_os_str()], "アプリを起動できない")
    }

    pub fn pick_application() -> Result<PathBuf, String> {
        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg("POSIX path of (choose application as alias)")
            .output()
            .map_err(|e| format!("osascript 起動に失敗: {e}"))?;
        if !output.status.success() {
            return Err("アプリ選択がキャンセルされた".into());
        }
        let app_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if app_path.is_empty() {
            return Err("アプリパスが空".into());
        }
        Ok(PathBuf::from(app_path))
    }

    pub fn move_to_trash(path: &Path) -> Result<(), String> {
        // argv 経由でパスを受け取るため、スクリプト本体にパスは一切現れない
        const SCRIPT: &str = "on run argv\n\
            tell application \"Finder\" to delete (POSIX file (item 1 of argv) as alias)\n\
            end run";
        let out = std::process::Command::new("osascript")
            .arg("-e")
            .arg(SCRIPT)
            .arg(path)
            .output()
            .map_err(|e| format!("ゴミ箱への移動に失敗: {e}"))?;
        if !out.status.success() {
            let msg = String::from_utf8_lossy(&out.stderr);
            return Err(format!("ゴミ箱への移動に失敗: {msg}"));
        }
        Ok(())
    }

    pub fn notify(title: &str, message: &str) {
        use std::process::Stdio;
        // 本文・タイトルとも argv 渡しにして injection を避ける
        let script = "on run argv\n\
            display notification (item 1 of argv) with title (item 2 of argv)\n\
            end run";
        let _ = std::process::Command::new("osascript")
            .args(["-e", script, message, title])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }

    fn spawn_open(args: &[&std::ffi::OsStr], what: &str) -> Result<(), String> {
        std::process::Command::new("open")
            .args(args)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("{what}: {e}"))
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::*;

    pub fn reveal(_path: &Path) -> Result<(), String> {
        Err(unsupported("ファイルマネージャでの表示"))
    }

    pub fn open_default(_path: &Path) -> Result<(), String> {
        Err(unsupported("既定アプリで開く操作"))
    }

    pub fn open_with(_app: &str, _path: &Path) -> Result<(), String> {
        Err(unsupported("アプリを指定して開く操作"))
    }

    pub fn open_in_text_editor(_path: &Path) -> Result<(), String> {
        Err(unsupported("テキストエディタで開く操作"))
    }

    /// URL だけは非 macOS でも従来から動いていた（`open_preview` の cfg 分岐）。
    /// その挙動をそのままここへ引き取る
    pub fn open_url(url: &str) -> Result<(), String> {
        url_command(url)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("URL を開けない: {e}"))
    }

    pub fn open_url_wait(url: &str) -> Result<(), String> {
        let status = url_command(url)
            .status()
            .map_err(|e| format!("URL ハンドラの実行に失敗: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("URL を開けない（終了コード {status}）: {url}"))
        }
    }

    #[cfg(windows)]
    fn url_command(url: &str) -> std::process::Command {
        // `start` は cmd の内蔵コマンド。第 1 引数はウィンドウタイトル扱いなので空文字を挟む
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", "start", "", url]);
        cmd
    }

    #[cfg(not(windows))]
    fn url_command(url: &str) -> std::process::Command {
        let mut cmd = std::process::Command::new("xdg-open");
        cmd.arg(url);
        cmd
    }

    pub fn open_new_instance(_app: &Path) -> Result<(), String> {
        Err(unsupported("アプリの新規プロセス起動"))
    }

    pub fn pick_application() -> Result<PathBuf, String> {
        Err(unsupported("アプリ選択ダイアログ"))
    }

    /// ゴミ箱の概念が無い（未実装の）環境では通常の削除へ劣化する。
    /// Windows は `SHFileOperation` でゴミ箱へ入れられるので、実装時にここを差し替える
    pub fn move_to_trash(path: &Path) -> Result<(), String> {
        std::fs::remove_file(path)
            .or_else(|_| std::fs::remove_dir_all(path))
            .map_err(|e| format!("削除に失敗: {e}"))
    }

    pub fn notify(_title: &str, _message: &str) {}

    fn unsupported(what: &str) -> String {
        format!("{what}はこのプラットフォームでは未対応です")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ゴミ箱操作は「存在しないパス」を渡しても panic せずエラーで返る
    /// （dispatch 側は事前に存在チェックしているが、境界単体でも安全であること）
    #[test]
    fn 存在しないパスのゴミ箱移動はエラーになる() {
        let missing = std::env::temp_dir().join(format!("tako-no-such-{}", std::process::id()));
        assert!(move_to_trash(&missing).is_err());
    }

    /// `move_to_trash` の argv 渡しがインジェクションされないことを、Finder を使わず
    /// osascript の argv 挙動そのもので検証する（CI の macOS ランナーで決定的に通る）。
    /// 悪意ある文字列を argv item 1 に渡しても、AppleScript の構文（`do shell script`）
    /// として解釈されず、単なるデータとして扱われることを確認する。
    #[cfg(target_os = "macos")]
    #[test]
    fn trash_argvは悪意ある文字列をデータとして扱う() {
        // インジェクションが成功すると作られてしまう副作用ファイル（cwd 相対 = パスに / を含めない）
        let marker = std::env::temp_dir().join(format!("tako_trash_pwned_{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let marker_str = marker.display().to_string();
        // " で文字列を閉じ do shell script を差し込もうとする典型的なインジェクション文字列
        let evil = format!("x\" do shell script \"touch {marker_str}\" ignoring \"");

        // move_to_trash と同じ argv 渡し方式（Finder 部分だけ「argv をそのまま返す」に差し替え）
        let out = std::process::Command::new("osascript")
            .arg("-e")
            .arg("on run argv\nreturn item 1 of argv\nend run")
            .arg(&evil)
            .output()
            .expect("osascript の実行に失敗");
        assert!(out.status.success(), "osascript が失敗: {out:?}");

        // データとしてそのまま返る = スクリプト構文に割り込んでいない
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("do shell script"),
            "argv がデータとして扱われていない: {stdout:?}"
        );
        // 副作用ファイルが作られていない = インジェクション不成立
        assert!(
            !marker.exists(),
            "AppleScript インジェクションで副作用ファイルが作られた: {marker_str}"
        );
        let _ = std::fs::remove_file(&marker);
    }

    /// 実ファイルの e2e: 改行・引用符・バックスラッシュを含む悪意あるファイル名でも
    /// 安全にゴミ箱へ移動でき、かつインジェクションの副作用が起きないこと。
    /// 実際に Finder を操作しゴミ箱へ移すため、GUI セッションのある手元で明示実行する。
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "Finder を操作しファイルをゴミ箱へ移すため手動確認用（cargo test -- --ignored）"]
    fn ゴミ箱移動は悪意あるファイル名を安全に扱う() {
        let dir = std::env::temp_dir().join(format!("tako_trash_e2e_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = std::env::temp_dir().join("tako_trash_e2e_pwned");
        let _ = std::fs::remove_file(&marker);

        // 改行 / " / \ / do shell script を含むファイル名（/ と NUL 以外は macOS で合法）
        let evil_name = "ev\"il\n `do shell script` \\ .txt";
        let evil = dir.join(evil_name);
        std::fs::write(&evil, b"x").unwrap();
        assert!(evil.exists(), "テストファイルが作れていない");

        move_to_trash(&evil).expect("ゴミ箱への移動に失敗");

        assert!(!evil.exists(), "ファイルが削除されていない");
        assert!(!marker.exists(), "インジェクションの副作用が発生した");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 通知は戻り値を持たない（best-effort）ことを型で固定する。
    /// 実際に通知を出すと `cargo test` のたびに通知センターへ積まれるので呼ばない
    #[test]
    fn 通知は失敗を呼び出し側へ伝播しない() {
        fn assert_unit(_: fn(&str, &str)) {}
        assert_unit(notify);
    }
}
