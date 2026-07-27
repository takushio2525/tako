//! OS シェル連携（抽象境界 B8）
//!
//! 「ファイルマネージャで表示」「既定アプリで開く」「URL を開く」「ゴミ箱へ移す」
//! 「通知を出す」など、**OS のシェルに委ねる操作**をここへ集約する。
//!
//! - macOS: `open` 系（`-R` / `-a` / `-t` / `-n`）と `osascript`
//! - Windows: 表示は `explorer.exe /select,`、開く系は `ShellExecuteW`、
//!   ゴミ箱は `SHFileOperationW` + `FOF_ALLOWUNDO`（#617）。URL は `cmd /C start`（既存挙動）
//! - その他 unix: URL は `xdg-open`
//!
//! 呼び出し側（`dispatch` / UI / CLI）はこのモジュールだけを見る。
//! **呼び出し側に `#[cfg]` を書かない**（`.agent/plans/2026-07-windows-port-architecture.md` 原則 1）。
//!
//! ## Windows 実装の方針（#617）
//!
//! `windows-sys` を足さず、必要な関数だけを手書きで宣言する（B17 = `platform::ime` の
//! IMM32、`agents` の Toolhelp32、`sleep_guard` の IOKit と同じ方針）。
//! ゴミ箱は COM の `IFileOperation` でも実現できるが、単発の削除に COM の
//! インターフェース定義を持ち込むのは重い。`SHFileOperationW` は Vista 以降
//! 非推奨扱いながら現役で、二重 NUL 終端のパスと構造体 1 つで済む。
//!
//! **`FOF_ALLOWUNDO` は絶対パスでなければゴミ箱へ入らず、黙って完全削除になる**
//! （MSDN 明記）。相対パスを渡されても復元可能であることを保つため、絶対化は
//! 境界の内側で必ず行う。
//!
//! ## ここに置かないもの
//!
//! - **権限昇格**（`osascript … with administrator privileges`）は B9（スリープ防止 =
//!   `sleep_guard`）の内側に留める。「OS シェルに何かを開かせる」操作ではないうえ、
//!   汎用の昇格 API を B8 に置くと危険な踏み台になる。Windows の B9 実装
//!   （`SetThreadExecutionState`）は sudoers 相当を必要としないため、そもそも対応物が無い

use std::path::Path;

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

/// 「このアプリで開く…」= OS のアプリ選択 UI を出し、選ばれたアプリでパスを開く。
///
/// **1 操作として境界に置く**。macOS は「アプリ選択ダイアログ → `open -a`」の 2 段だが、
/// Windows の対応物は `openas` verb（「このファイルを開く方法を選んでください」）で
/// **選択と起動が分かれていない**ため、`pick_application()` のような部品に割ると
/// Windows 側が表現できない。キャンセルはエラー（呼び出し側は無視してよい）
pub fn open_with_dialog(path: &Path) -> Result<(), String> {
    imp::open_with_dialog(path)
}

/// パスをゴミ箱へ移す（**完全削除にはしない**）。
///
/// macOS は Finder に委ねる（ゴミ箱から戻せる = ユーザーの期待どおり）。
/// **パスは AppleScript ソースへ連結せず `osascript` の argv として渡す**ため、
/// ファイル名に含まれる `"` `\` 改行がスクリプト構文へ割り込む余地が構造的に無い
/// （#80。エスケープの正しさに依存しない）。
/// Windows は `SHFileOperationW` + `FOF_ALLOWUNDO` でごみ箱へ入れる（#617）。
///
/// ゴミ箱の概念が無い環境（その他 unix）では**削除せずエラーにする**。
/// UI にもコマンドにも確認ダイアログが無い操作なので、
/// 「ゴミ箱のつもりで完全削除」だけは起こさない
pub fn move_to_trash(path: &Path) -> Result<(), String> {
    imp::move_to_trash(path)
}

/// デスクトップ通知を出す（best-effort）。
/// メッセージは argv で渡すため本文の内容がスクリプト構文へ割り込まない。
///
/// 戻り値は**通知を出せたか**。Windows は通知の実装が無く常に `false` を返す（#617）。
/// 黙って消える経路を作らないため、呼び出し側が代替表示へ倒せるようにしてある
/// （本格的なトースト通知 = WinRT `ToastNotification` は別 Issue）
pub fn notify(title: &str, message: &str) -> bool {
    imp::notify(title, message)
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

    /// macOS は「選択」と「起動」が別操作なので、境界の内側で 2 段を組む
    pub fn open_with_dialog(path: &Path) -> Result<(), String> {
        let app = pick_application()?;
        open_with(&app.to_string_lossy(), path)
    }

    fn pick_application() -> Result<std::path::PathBuf, String> {
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
        Ok(std::path::PathBuf::from(app_path))
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

    pub fn notify(title: &str, message: &str) -> bool {
        use std::process::Stdio;
        // 本文・タイトルとも argv 渡しにして injection を避ける
        let script = "on run argv\n\
            display notification (item 1 of argv) with title (item 2 of argv)\n\
            end run";
        std::process::Command::new("osascript")
            .args(["-e", script, message, title])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok()
    }

    fn spawn_open(args: &[&std::ffi::OsStr], what: &str) -> Result<(), String> {
        std::process::Command::new("open")
            .args(args)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("{what}: {e}"))
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::process::CommandExt;

    // --- Win32 FFI --------------------------------------------------------
    // `windows-sys` を足さず、使う関数と構造体だけを宣言する（モジュール doc 参照）

    /// `SHFILEOPSTRUCTW`（shellapi.h）。
    ///
    /// shellapi.h はこの構造体を **32bit のときだけ** `pshpack1.h` で 1 バイト詰めにする
    /// （`windows-sys` も `target_arch = "x86"` にだけ `packed(1)` を付けている）。
    /// tako の Windows 配布は x64 だが、レイアウトを取り違えると引数が丸ごとずれるので
    /// 両方を宣言しておく
    #[repr(C)]
    #[cfg_attr(target_arch = "x86", repr(packed(1)))]
    struct ShFileOpStructW {
        hwnd: isize,
        w_func: u32,
        /// 二重 NUL 終端の UTF-16 文字列（複数パスを連結できる形式）
        p_from: *const u16,
        p_to: *const u16,
        /// `FILEOP_FLAGS` = `WORD`
        f_flags: u16,
        f_any_operations_aborted: i32,
        h_name_mappings: *mut core::ffi::c_void,
        lpsz_progress_title: *const u16,
    }

    /// `FO_DELETE`（shellapi.h）
    const FO_DELETE: u32 = 0x0003;
    /// 進捗ダイアログを出さない
    const FOF_SILENT: u16 = 0x0004;
    /// 「削除しますか？」を出さない（macOS の Finder 委譲と同じく、UI 側に確認が無い操作なので揃える）
    const FOF_NOCONFIRMATION: u16 = 0x0010;
    /// **これが無いとごみ箱ではなく完全削除になる**
    const FOF_ALLOWUNDO: u16 = 0x0040;
    /// エラーダイアログを出さない（戻り値でエラーを返すのでこちらが正）
    const FOF_NOERRORUI: u16 = 0x0400;
    /// ごみ箱に入れられない対象（容量超過・ごみ箱無効のドライブ等）のとき、
    /// `FOF_NOCONFIRMATION` を**部分的に打ち消して**「完全に削除しますか？」を出させる。
    /// 「ごみ箱に移動」と表示しておいて黙って完全削除する事故を防ぐための保険
    const FOF_WANTNUKEWARNING: u16 = 0x4000;

    /// `SW_SHOWNORMAL`
    const SW_SHOWNORMAL: i32 = 1;
    /// `ShellExecuteW` は「32 より大きければ成功」（返るのは歴史的経緯で HINSTANCE）
    const SHELL_EXECUTE_SUCCESS_MIN: isize = 32;

    /// `COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE`
    const COINIT_STA: u32 = 0x2 | 0x4;

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHFileOperationW(op: *mut ShFileOpStructW) -> i32;
        fn ShellExecuteW(
            hwnd: isize,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show_cmd: i32,
        ) -> isize;
    }

    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoInitializeEx(reserved: *mut core::ffi::c_void, co_init: u32) -> i32;
        fn CoUninitialize();
    }

    /// シェル API（`ShellExecuteW` / `SHFileOperationW`）はシェル拡張へ処理を委ねるため、
    /// 呼び出しスレッドで COM が初期化されている必要がある。tako は IPC / MCP の
    /// ワーカースレッドからもファイル操作を dispatch するので、未初期化でも動くように
    /// ここで STA を張り、**張った回数ちょうど**だけ解放する
    struct ComGuard {
        need_uninit: bool,
    }

    impl ComGuard {
        fn new() -> Self {
            // SAFETY: reserved は仕様どおり NULL。戻り値だけで釣り合わせを決める
            let hr = unsafe { CoInitializeEx(std::ptr::null_mut(), COINIT_STA) };
            // S_OK(0) / S_FALSE(1) はこのスレッドの参照カウントが増えている。
            // RPC_E_CHANGED_MODE（既に MTA。負値）は増えていないので触らない
            Self {
                need_uninit: hr >= 0,
            }
        }
    }

    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.need_uninit {
                // SAFETY: CoInitializeEx が成功した回数と 1:1 で呼ぶ
                unsafe { CoUninitialize() };
            }
        }
    }

    /// UTF-16 + NUL 終端。途中に NUL があると Win32 側でそこまでしか読まれず
    /// **別のパスを指してしまう**ので、切り詰めずエラーにする
    fn wide(s: &OsStr) -> Result<Vec<u16>, String> {
        let mut buf: Vec<u16> = s.encode_wide().collect();
        if buf.contains(&0) {
            return Err("パスに NUL 文字が含まれている".into());
        }
        buf.push(0);
        Ok(buf)
    }

    /// 相対パスを絶対パスへ直す。
    ///
    /// `std::fs::canonicalize` は使わない。返るのが `\\?\C:\…`（verbatim 形式）で、
    /// explorer も `SHFileOperationW` もこの形式を解釈できないため。
    /// `std::path::absolute` は `.` / `..` の正規化までで済み、シェルが読める形を保つ
    pub(super) fn absolute(path: &Path) -> Result<std::path::PathBuf, String> {
        std::path::absolute(path).map_err(|e| format!("パスを絶対化できない: {e}"))
    }

    /// `ShellExecuteW` の薄いラッパー。`verb` が `None` なら既定の動詞
    fn shell_execute(
        verb: Option<&str>,
        file: &OsStr,
        parameters: Option<&OsStr>,
        what: &str,
    ) -> Result<(), String> {
        let verb_w = verb.map(|v| wide(OsStr::new(v))).transpose()?;
        let file_w = wide(file)?;
        let params_w = parameters.map(wide).transpose()?;
        let _com = ComGuard::new();
        // SAFETY: 渡すポインタはいずれもこの呼び出しの間だけ生きていればよく、
        // 元のバッファ（verb_w / file_w / params_w）はこのスコープで保持している。
        // 省略可能な引数は仕様どおり NULL を渡す
        let rc = unsafe {
            ShellExecuteW(
                0,
                verb_w.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
                file_w.as_ptr(),
                params_w.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        if rc > SHELL_EXECUTE_SUCCESS_MIN {
            Ok(())
        } else {
            Err(format!("{what}: {}", describe_shell_execute_error(rc)))
        }
    }

    /// `ShellExecuteW` のエラー（shellapi.h の `SE_ERR_*` と Win32 エラーが混在する）
    fn describe_shell_execute_error(rc: isize) -> String {
        match rc {
            0 => "メモリまたはリソース不足".into(),
            2 => "ファイルが見つからない".into(),
            3 => "パスが見つからない".into(),
            5 => "アクセスが拒否された".into(),
            8 => "メモリ不足".into(),
            26 => "共有違反".into(),
            27 => "ファイルの関連付けが不完全または無効".into(),
            28 => "タイムアウト".into(),
            29 => "DDE トランザクションに失敗".into(),
            31 => "この種類のファイルに関連付けられたアプリが無い".into(),
            32 => "関連付けられた DLL が見つからない".into(),
            other => format!("ShellExecuteW が {other} を返した"),
        }
    }

    /// `SHFileOperationW` の戻り値（Win32 エラーコードではなく shell 独自の `DE_*`）
    fn describe_shfileop_error(code: i32) -> String {
        match code {
            0x71 => "同じファイルを指している".into(),
            0x74 => "ルートディレクトリは操作できない".into(),
            0x75 => "操作が取り消された".into(),
            0x78 => "アクセスが拒否された".into(),
            0x79 => "パスが MAX_PATH（260 文字）を超えている".into(),
            0x7C => "パスが不正".into(),
            0x402 => "パスが見つからない（または不正）".into(),
            other => format!("SHFileOperationW が 0x{other:X} を返した"),
        }
    }

    /// `explorer.exe` に渡すコマンドライン。
    ///
    /// **explorer は `CommandLineToArgvW` で引数を解釈しない**。生のコマンドラインを
    /// 自前で読み、先頭が `/select,` かどうかを見る。したがって
    ///
    /// - `/select,` とパスの間に空白を入れてはいけない（別の引数として捨てられる）
    /// - **引数全体を引用符でくくってもいけない**。`"/select,C:\a b\c.txt"` は
    ///   スイッチとして認識されず、既定フォルダ（ドキュメント）が開くだけになる
    ///   （実測: `Command::arg` の自動クォートで空白入りパスを渡すとこれが起きた）
    ///
    /// 正しい形は **パスだけを引用符でくくる** `/select,"C:\a b\c.txt"`。
    /// `raw_arg` で組み立てるため、引数の切れ目を壊しうる `"` を含むパスは拒否する
    /// （Windows のファイル名に `"` は使えないので、実在のパスを取りこぼすことはない）
    pub(super) fn select_argument(abs: &Path) -> Result<OsString, String> {
        if abs.as_os_str().encode_wide().any(|c| c == u16::from(b'"')) {
            return Err("パスに引用符が含まれている".into());
        }
        let mut arg = OsString::from("/select,\"");
        arg.push(abs.as_os_str());
        arg.push("\"");
        Ok(arg)
    }

    pub fn reveal(path: &Path) -> Result<(), String> {
        let abs = absolute(path)?;
        // 存在しないパスを渡すと explorer は**既定フォルダ（ドキュメント）を開いて**しまう。
        // 「押したら関係ない窓が出た」より「開けなかった」と言う方が親切なので先に弾く
        if std::fs::symlink_metadata(&abs).is_err() {
            return Err(format!("パスが存在しない: {}", abs.display()));
        }
        let mut cmd = std::process::Command::new("explorer.exe");
        // 自動クォートを避けるため raw_arg で渡す（上の doc の理由）
        cmd.raw_arg(select_argument(&abs)?);
        // explorer.exe は **成功しても終了コード 1 を返す**ので status は見ない
        // （待っても成否の判定に使えない。開けたかどうかは画面が正）
        tako_core::platform::process::no_console_window(&mut cmd)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("エクスプローラーを開けない: {e}"))
    }

    pub fn open_default(path: &Path) -> Result<(), String> {
        let abs = absolute(path)?;
        shell_execute(
            Some("open"),
            abs.as_os_str(),
            None,
            "デフォルトアプリで開けない",
        )
    }

    pub fn open_with(app: &str, path: &Path) -> Result<(), String> {
        let abs = absolute(path)?;
        shell_execute(
            Some("open"),
            OsStr::new(app),
            Some(&quoted_argument(abs.as_os_str())),
            &format!("アプリ '{app}' で開けない"),
        )
    }

    /// Windows の「このアプリで開く…」は `openas` verb そのもの。
    /// 選択ダイアログの表示とアプリ起動を OS が 1 操作で行う
    pub fn open_with_dialog(path: &Path) -> Result<(), String> {
        let abs = absolute(path)?;
        shell_execute(
            Some("openas"),
            abs.as_os_str(),
            None,
            "アプリ選択ダイアログを開けない",
        )
    }

    /// Windows に「既定のテキストエディタ」という概念（macOS の `open -t`）は無い。
    /// 拡張子の関連付け（`open_default`）だとテキストエディタで開くとは限らないので、
    /// 必ずテキストとして開ける メモ帳 を使う
    pub fn open_in_text_editor(path: &Path) -> Result<(), String> {
        let abs = absolute(path)?;
        shell_execute(
            Some("open"),
            OsStr::new("notepad.exe"),
            Some(&quoted_argument(abs.as_os_str())),
            "テキストエディタで開けない",
        )
    }

    /// `ShellExecuteW` の `lpParameters` は**生のコマンドライン文字列**なので、
    /// 空白を含むパスは引用符でくくる。Windows のパスに `"` は使えないため、
    /// くくるだけで引数の切れ目が壊れない
    fn quoted_argument(path: &OsStr) -> OsString {
        let mut quoted = OsString::from("\"");
        quoted.push(path);
        quoted.push("\"");
        quoted
    }

    /// URL は従来どおり `cmd /C start`（挙動を変えない）
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

    fn url_command(url: &str) -> std::process::Command {
        // `start` は cmd の内蔵コマンド。第 1 引数はウィンドウタイトル扱いなので空文字を挟む
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", "start", "", url]);
        tako_core::platform::process::no_console_window(&mut cmd);
        cmd
    }

    pub fn open_new_instance(_app: &Path) -> Result<(), String> {
        // 自動更新（B14）の Windows 経路はインストーラー起動 → 自プロセス終了なので
        // 「同じアプリをもう 1 つ起こす」対応物が無い（#528）
        Err("アプリの新規プロセス起動はこのプラットフォームでは未対応です".into())
    }

    pub fn move_to_trash(path: &Path) -> Result<(), String> {
        // FOF_ALLOWUNDO は**絶対パスでないとごみ箱へ入らず完全削除になる**（MSDN 明記）
        let abs = absolute(path)?;
        // 壊れたシンボリックリンクも対象にしたいので exists() ではなくメタデータで見る。
        // 存在しないパスに対する SHFileOperationW の戻り値に依存しないためでもある
        if std::fs::symlink_metadata(&abs).is_err() {
            return Err(format!("パスが存在しない: {}", abs.display()));
        }
        // pFrom は二重 NUL 終端（複数パスを並べられる形式。1 件でも終端は 2 つ要る）
        let mut from = wide(abs.as_os_str())?;
        from.push(0);

        let mut op = ShFileOpStructW {
            hwnd: 0,
            w_func: FO_DELETE,
            p_from: from.as_ptr(),
            p_to: std::ptr::null(),
            f_flags: trash_flags(),
            f_any_operations_aborted: 0,
            h_name_mappings: std::ptr::null_mut(),
            lpsz_progress_title: std::ptr::null(),
        };
        let _com = ComGuard::new();
        // SAFETY: op は呼び出しの間スタック上に生きており、p_from が指す `from` も同様。
        // 使わないポインタ（p_to / hNameMappings / lpszProgressTitle）は仕様どおり NULL
        let code = unsafe { SHFileOperationW(&mut op) };
        if code != 0 {
            return Err(format!(
                "ごみ箱への移動に失敗: {}",
                describe_shfileop_error(code)
            ));
        }
        // 「完全に削除しますか？」をユーザーが断ったときはここに落ちる（戻り値は 0）。
        // 消えていないので成功と報告してはいけない
        if op.f_any_operations_aborted != 0 {
            return Err("ごみ箱への移動が取り消された".into());
        }
        Ok(())
    }

    /// ごみ箱移動のフラグ。**`FOF_ALLOWUNDO` が抜けたら完全削除になる**ので
    /// テストから固定できるよう関数に切り出してある
    pub(super) const fn trash_flags() -> u16 {
        FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_WANTNUKEWARNING | FOF_SILENT | FOF_NOERRORUI
    }

    /// Windows のデスクトップ通知は未実装（#617 で明示的に見送り）。
    /// 本文（リモート接続のデバイス名等）はログにも出さない
    pub fn notify(_title: &str, _message: &str) -> bool {
        tracing::debug!("OS 通知は Windows 未対応のため送信していない");
        false
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
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

    pub fn open_with_dialog(_path: &Path) -> Result<(), String> {
        Err(unsupported("アプリ選択ダイアログ"))
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

    fn url_command(url: &str) -> std::process::Command {
        let mut cmd = std::process::Command::new("xdg-open");
        cmd.arg(url);
        cmd
    }

    pub fn open_new_instance(_app: &Path) -> Result<(), String> {
        Err(unsupported("アプリの新規プロセス起動"))
    }

    /// **削除へ劣化させない**。ゴミ箱のつもりで完全削除するくらいなら失敗した方がよい
    pub fn move_to_trash(_path: &Path) -> Result<(), String> {
        Err(unsupported("ゴミ箱への移動"))
    }

    pub fn notify(_title: &str, _message: &str) -> bool {
        false
    }

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

    /// 通知は「出せたか」を返す（#617）。出せない環境で黙って消えるのを防ぐため、
    /// 呼び出し側が代替表示へ倒せる形を型で固定する。
    /// 実際に通知を出すと `cargo test` のたびに通知センターへ積まれるので呼ばない
    #[test]
    fn 通知は成否を呼び出し側へ返す() {
        fn assert_returns_bool(_: fn(&str, &str) -> bool) {}
        assert_returns_bool(notify);
    }

    /// Windows のごみ箱移動（#617）。実際にごみ箱へ入れる e2e は `#[ignore]`（macOS 側と同じ方針）
    #[cfg(windows)]
    mod windows {
        use super::super::*;

        /// **最重要の不変条件**: `FOF_ALLOWUNDO` が落ちると完全削除になり復元できない。
        /// フラグの組み合わせを数値で固定して、後から誰かが外したら落ちるようにする
        #[test]
        fn ごみ箱移動のフラグにallowundoが必ず入る() {
            const FOF_ALLOWUNDO: u16 = 0x0040;
            const FOF_WANTNUKEWARNING: u16 = 0x4000;
            let flags = imp::trash_flags();
            assert_eq!(
                flags & FOF_ALLOWUNDO,
                FOF_ALLOWUNDO,
                "FOF_ALLOWUNDO が無い = 完全削除になる"
            );
            // ごみ箱に入れられない対象で黙って完全削除しないための保険も外させない
            assert_eq!(flags & FOF_WANTNUKEWARNING, FOF_WANTNUKEWARNING);
            // 0x4454 = ALLOWUNDO | NOCONFIRMATION | WANTNUKEWARNING | SILENT | NOERRORUI
            assert_eq!(flags, 0x4454, "フラグ構成を変えたら意図を確認すること");
        }

        /// `FOF_ALLOWUNDO` は**絶対パスでないと効かない**（相対パスだと完全削除に落ちる）。
        /// かつ `\\?\` 形式（`canonicalize` の戻り）はシェル API が解釈できない。
        /// 境界の絶対化がこの 2 条件を満たすことを固定する
        #[test]
        fn 絶対化はverbatim形式にならない() {
            let cwd = std::env::current_dir().unwrap();
            let abs = imp::absolute(Path::new("Cargo.toml")).unwrap();
            assert!(abs.is_absolute(), "相対パスのままだと完全削除になる");
            assert_eq!(abs, cwd.join("Cargo.toml"));
            assert!(
                !abs.to_string_lossy().starts_with(r"\\?\"),
                "verbatim 形式は explorer / SHFileOperationW が解釈できない: {abs:?}"
            );
            // 既に絶対パスなら素通し（`..` は正規化される）
            let dotted = cwd.join("src").join("..").join("Cargo.toml");
            assert_eq!(imp::absolute(&dotted).unwrap(), cwd.join("Cargo.toml"));
        }

        /// reveal のコマンドラインは `/select,"<パス>"`。
        ///
        /// **カンマの直後にパスを置く**（空白を挟むと explorer がスイッチを見失う）ことと、
        /// **引用符でパスだけを囲む**（引数全体を囲むと選択されず既定フォルダが開く。実測）
        /// ことが同時に成り立つこと。空白・日本語でも形が崩れない
        #[test]
        fn revealの引数はパスだけを引用符で囲む() {
            for name in ["a.txt", "スペース あり.txt", "日本語のファイル.md"] {
                let path = std::path::PathBuf::from(r"C:\Users\test\Desktop").join(name);
                let arg = imp::select_argument(&path).expect("正常なパスでは組める");
                let s = arg.to_string_lossy();
                assert_eq!(s, format!("/select,\"{}\"", path.to_string_lossy()));
                assert!(!s.starts_with('"'), "引数全体を囲むと選択されない: {s}");
            }
        }

        /// `raw_arg` で渡すので、引数の切れ目を壊しうる `"` を含むパスは拒否する
        #[test]
        fn 引用符を含むパスは拒否する() {
            let evil = std::path::PathBuf::from("C:\\tmp\\a\" /select,\"C:\\Windows");
            assert!(imp::select_argument(&evil).is_err());
        }

        /// 存在しないパスは（SHFileOperationW の戻り値に頼らず）事前にエラーで返す
        #[test]
        fn 存在しないパスは事前にエラーになる() {
            let missing =
                std::env::temp_dir().join(format!("tako-trash-missing-{}", std::process::id()));
            let err = move_to_trash(&missing).unwrap_err();
            assert!(err.contains("存在しない"), "{err}");
        }

        /// 実 UI の e2e: エクスプローラーが開き、対象が選択される。
        /// ウィンドウを開くので手元で明示実行する（開いた窓は目視で確認して閉じる）
        #[test]
        #[ignore = "エクスプローラーの窓を開くため手動確認用（cargo test -- --ignored）"]
        fn revealの実e2e() {
            let dir = std::env::temp_dir().join(format!("tako_reveal_e2e_{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("スペース あり.txt");
            std::fs::write(&file, b"tako").unwrap();
            reveal(&file).expect("エクスプローラーを開けない");
            // 窓が出るまで待つ。**ここでディレクトリを消さない**
            // （消すとエクスプローラーが窓を畳んでしまい、選択状態を確認できない）
            std::thread::sleep(std::time::Duration::from_secs(3));
            println!("選択されているはずのファイル: {}", file.display());
        }

        /// 実 UI の e2e: 既定アプリが起動する。アプリが立ち上がるので手元で明示実行する。
        ///
        /// **ディレクトリ**（ハンドラが必ず存在する = エクスプローラー）と**ファイル**の
        /// 両方を開く。ファイル側のハンドラは環境依存（拡張子の関連付け次第）なので、
        /// 経路そのものが生きているかはディレクトリ側で判定できるようにしてある
        #[test]
        #[ignore = "既定アプリを起動するため手動確認用（cargo test -- --ignored）"]
        fn デフォルトアプリで開く実e2e() {
            let dir = std::env::temp_dir().join(format!("tako_open_e2e_{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("開く テスト.txt");
            std::fs::write(&file, "tako open default\n").unwrap();

            open_default(&dir).expect("ディレクトリを既定アプリで開けない");
            open_default(&file).expect("ファイルを既定アプリで開けない");
            std::thread::sleep(std::time::Duration::from_secs(3));
            // アプリが開いたままなので消さない（呼び出し側で後始末する）
            println!("開いたディレクトリ: {}", dir.display());
            println!("開いたファイル: {}", file.display());
        }

        /// 実ファイルの e2e: スクラッチファイルが**ごみ箱へ入る**（消えるだけでなく復元可能）。
        /// ごみ箱に実物を積むので手元で明示実行する（`cargo test -- --ignored`）。
        /// ごみ箱に入ったことの確認は PowerShell の Shell.Application で行う（#617 の証拠）
        #[test]
        #[ignore = "ごみ箱に実ファイルを積むため手動確認用（cargo test -- --ignored）"]
        fn ごみ箱移動の実e2e() {
            let dir = std::env::temp_dir().join(format!("tako_trash_e2e_{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();

            // 空白・日本語つきのファイル / 読み取り専用ファイル / ディレクトリの 3 種
            let file = dir.join("スペース あり.txt");
            std::fs::write(&file, b"tako").unwrap();
            let readonly = dir.join("readonly.txt");
            std::fs::write(&readonly, b"tako").unwrap();
            let mut perm = std::fs::metadata(&readonly).unwrap().permissions();
            perm.set_readonly(true);
            std::fs::set_permissions(&readonly, perm).unwrap();
            let subdir = dir.join("サブフォルダ");
            std::fs::create_dir_all(subdir.join("nested")).unwrap();
            std::fs::write(subdir.join("nested").join("a.txt"), b"tako").unwrap();

            for target in [&file, &readonly, &subdir] {
                move_to_trash(target).unwrap_or_else(|e| panic!("{}: {e}", target.display()));
                assert!(!target.exists(), "消えていない: {}", target.display());
            }

            // MAX_PATH（260 文字）超え。SHFileOperationW は長いパスに対応しないので
            // **成功しても失敗しても「黙って完全削除」にならない**ことを確認する
            let mut deep = dir.clone();
            while deep.as_os_str().len() < 300 {
                deep.push("長い名前のフォルダ");
            }
            std::fs::create_dir_all(&deep).unwrap();
            let long_file = deep.join("long.txt");
            std::fs::write(&long_file, b"tako").unwrap();
            let long_result = move_to_trash(&long_file);
            println!("長いパス（{} 文字）の結果: {long_result:?}", {
                long_file.as_os_str().len()
            });
            if long_result.is_err() {
                assert!(long_file.exists(), "失敗したのにファイルが消えている");
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
