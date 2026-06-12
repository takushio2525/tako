//! tmux_backend — 全ペインの PTY を tmux セッションとして保持する永続化層（Phase 5.5 / FR-5）
//!
//! シェルを直接 spawn する代わりに、tako 専用の tmux サーバー（`tmux -L tako`。
//! ユーザーの既定サーバーとは分離）上のセッションへ attach するクライアントを spawn する。
//! tako が終了してもセッション（実行中プロセス + 画面内容）は tmux サーバー側に残り、
//! 再起動時に同じセッション名へ attach し直すことで完全復元する。
//!
//! - `new-session -A` により「新規作成」と「再起動後の再 attach」が**同一コマンド**になる
//!   （セッションが生きていれば attach、消えていれば（再起動・kill 後）新規作成）
//! - `-D` で他クライアントを切り離す（多重起動時は最新インスタンスへ収束）
//! - tmux 不在環境では呼び出し側（tako-app）が `available()` を見て従来の直接 spawn へ
//!   無害に劣化する（ゼロコンフィグ原則）
//! - サーバーは専用 conf（`<data_dir>/tmux-backend.conf`）で起動し、ユーザーの
//!   `~/.tmux.conf` は読まない（status バー・prefix キー等が見えない裏方に徹する）

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use crate::paths::data_dir;
use crate::terminal::{SpawnCommand, SpawnOptions};

/// バックエンドセッション名の接頭辞。シェル統合スクリプトが「tako のバックエンド配下か」を
/// 判定する目印（ソケット名も同じ接頭辞）なので変更時はスクリプト側も揃えること
pub const SESSION_PREFIX: &str = "tako-";

/// 専用 tmux サーバーのソケット名（`tmux -L`）。ユーザーの既定サーバーと分離する。
/// `TAKO_TMUX_SOCKET` で差し替え可能（セルフテストの隔離に使う）
pub fn socket_name() -> String {
    std::env::var("TAKO_TMUX_SOCKET")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "tako".into())
}

/// tmux が使えるか（`tmux -V` が成功するか）。プロセス内でキャッシュする
pub fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        Command::new("tmux")
            .arg("-V")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

/// バックエンドサーバーの設定。見えない裏方として振る舞うための最小構成:
/// - `status off` / `prefix None`: tmux の UI・キー介入を消す（操作はすべて tako 側）
/// - `mouse on`: ホイールを tmux のスクロール（copy-mode）に写す。attach 構成では
///   スクロールバックを tmux が持つため、tako 側の自前スクロールバックの代替になる
/// - `allow-passthrough on`: シェル統合の OSC 7 / 133 をパススルーで外（tako）へ届かせる
/// - `extended-keys` + `terminal-features extkeys`: kitty keyboard / CSI u
///   （Shift+Enter 等の区別。FR の常用要件）を tmux 越しに維持する
/// - `update-environment`: 再 attach 時にセッション環境の TAKO_* を新インスタンスの値へ
///   更新する（既存プロセスには届かないが、それは CLI の control.json フォールバック
///   = FR-2.2.9 が吸収する）
const BACKEND_CONF: &str = "\
# tako tmux バックエンド設定（自動生成。手で編集しない。tako-core::tmux_backend）
set -g status off
set -g prefix None
set -g mouse on
set -g history-limit 10000
set -g allow-passthrough on
set -g focus-events on
set -g set-clipboard on
set -g default-terminal tmux-256color
set -s escape-time 10
set -s extended-keys always
set -as terminal-features 'xterm*:extkeys:RGB'
set -g update-environment 'TAKO_SOCKET TAKO_TOKEN TAKO_MCP_URL TAKO_TAB_ID'
";

/// 専用 conf をデータディレクトリへ書き出す（毎起動上書き = バージョン更新追従）。
/// 書けない環境では `/dev/null` を返し「ユーザー conf を読まない」ことだけは維持する
fn ensure_conf() -> PathBuf {
    fn write_conf() -> Option<PathBuf> {
        let dir = data_dir()?;
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join("tmux-backend.conf");
        std::fs::write(&path, BACKEND_CONF).ok()?;
        Some(path)
    }
    write_conf().unwrap_or_else(|| PathBuf::from("/dev/null"))
}

/// SpawnOptions を tmux セッション経由に書き換える。
/// `options.env`（TAKO_* 注入を含む）はクライアント経由でセッション作成時の環境になる。
/// `options.cwd` は `-c` で渡す（既存セッションへの attach では tmux が無視する）
pub fn wrap_options(options: SpawnOptions, socket: &str, session: &str) -> SpawnOptions {
    let mut args = vec![
        "-L".to_string(),
        socket.to_string(),
        "-f".to_string(),
        ensure_conf().display().to_string(),
        "new-session".to_string(),
        "-A".to_string(),
        "-D".to_string(),
        "-s".to_string(),
        session.to_string(),
    ];
    if let Some(cwd) = &options.cwd {
        args.push("-c".to_string());
        args.push(cwd.display().to_string());
    }
    // 内側で動かすコマンド。未指定なら既定シェル（`$SHELL -l`。直接 spawn 時と同じ解決）。
    // tmux は残余引数を空白連結して sh -c で実行するため、各語をクォートして 1 引数で渡す
    if let Some(inner) = options
        .command
        .clone()
        .or_else(crate::terminal::default_shell)
    {
        args.push(shell_quoted(&inner));
    }
    SpawnOptions {
        command: Some(SpawnCommand {
            program: "tmux".into(),
            args,
        }),
        ..options
    }
}

/// バックエンドセッション内ペインの tty（`/dev/ttysNNN`）。
/// ペイン配下のプロセスはこの tty を制御端末に持つため、listen ポート検知（FR-2.4.2）と
/// tmuxview の tty 突き合わせ（FR-2.13.2）はこの tty に差し替えて維持する。
/// セッション未作成・tmux 不在では None（呼び出し側がリトライする）
pub fn pane_tty(socket: &str, session: &str) -> Option<String> {
    let output = Command::new("tmux")
        .args([
            "-L",
            socket,
            "display-message",
            "-p",
            "-t",
            &format!("={session}"),
            "#{pane_tty}",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let tty = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!tty.is_empty()).then_some(tty)
}

/// セッションを破棄する（ペインの明示 close 時。tako 終了時は呼ばない = 永続化）。
/// セッションが既に無い（シェル exit で消えた後）のエラーは無害なので潰す
pub fn kill_session(socket: &str, session: &str) {
    let _ = crate::tmux::kill_session(Some(socket), session);
}

/// バックエンドサーバーごと落とす（セルフテストの後片付け用）
pub fn kill_server(socket: &str) {
    let _ = Command::new("tmux")
        .args(["-L", socket, "kill-server"])
        .output();
}

/// 語のリストを sh -c 安全な 1 つのコマンド文字列へ組み立てる
fn shell_quoted(command: &SpawnCommand) -> String {
    std::iter::once(&command.program)
        .chain(command.args.iter())
        .map(|w| quote_word(w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 単語のシェルクォート。英数と無害な記号のみならそのまま、他は単引用符で包む
fn quote_word(word: &str) -> String {
    let safe = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,@%+".contains(c));
    if safe {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 単語のクォートはシェル安全() {
        assert_eq!(quote_word("/bin/zsh"), "/bin/zsh");
        assert_eq!(quote_word("-l"), "-l");
        assert_eq!(quote_word("a b"), "'a b'");
        assert_eq!(quote_word("it's"), r#"'it'\''s'"#);
        assert_eq!(quote_word(""), "''");
        assert_eq!(
            shell_quoted(&SpawnCommand {
                program: "npm".into(),
                args: vec!["run".into(), "dev server".into()],
            }),
            "npm run 'dev server'"
        );
    }

    #[test]
    fn wrapはtmux_attach同一コマンドを組み立てる() {
        let options = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "echo hi".into()],
            }),
            cwd: Some("/tmp".into()),
            env: vec![("TAKO_PANE_ID".into(), "3".into())],
        };
        let wrapped = wrap_options(options, "tako-test", "tako-abc123");
        let command = wrapped.command.expect("tmux コマンドに置き換わる");
        assert_eq!(command.program, "tmux");
        let args = command.args;
        // -L <socket> と new-session -A -D -s <session> を含む
        let l = args.iter().position(|a| a == "-L").unwrap();
        assert_eq!(args[l + 1], "tako-test");
        assert!(args.contains(&"new-session".to_string()));
        assert!(args.contains(&"-A".to_string()));
        assert!(args.contains(&"-D".to_string()));
        let s = args.iter().position(|a| a == "-s").unwrap();
        assert_eq!(args[s + 1], "tako-abc123");
        let c = args.iter().position(|a| a == "-c").unwrap();
        assert_eq!(args[c + 1], "/tmp");
        // 内側コマンドはクォート済みの 1 引数
        assert_eq!(args.last().unwrap(), "/bin/sh -c 'echo hi'");
        // env / cwd は維持される（env はセッション作成時の環境になる）
        assert_eq!(wrapped.env.len(), 1);
        assert_eq!(wrapped.cwd.as_deref(), Some(std::path::Path::new("/tmp")));
    }

    #[test]
    fn コマンド未指定は既定シェルで包む() {
        #[cfg(unix)]
        {
            let wrapped = wrap_options(SpawnOptions::default(), "tako-test", "tako-x");
            let command = wrapped.command.unwrap();
            // 末尾の 1 引数が既定シェル（$SHELL -l）になっている
            assert!(command.args.last().unwrap().ends_with(" -l"));
        }
    }

    /// 永続化の根幹 e2e: クライアント（tako 側）を破棄してもセッションが生き、
    /// 同一コマンドで attach し直すと画面内容ごと戻る。tmux 不在環境ではスキップ
    #[test]
    #[cfg(unix)]
    fn セッションはクライアント切断後もattachで内容ごと戻る() {
        if !available() {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let socket = format!("tako-coretest-{}", std::process::id());
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                kill_server(&self.0);
            }
        }
        let _cleanup = Cleanup(socket.clone());
        let session = "tako-e2e-persist";
        // rc ファイルを読まない /bin/sh で決定的に
        let base = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec![],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
        };

        fn wait_for(session: &crate::TerminalSession, needle: &str) -> bool {
            for _ in 0..100 {
                if session
                    .visible_lines()
                    .iter()
                    .any(|line| line.contains(needle))
                {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            false
        }

        // 1 回目: セッション作成 + マーカー出力
        let (first, _rx1) =
            crate::TerminalSession::spawn(80, 24, wrap_options(base.clone(), &socket, session))
                .expect("tmux クライアントを spawn できる");
        // 入力エコーと区別するためクォートを挟む（出力にだけ素の文字列が現れる）
        first.write(b"echo TAKO-PERSIST-'OK'\r".to_vec());
        assert!(
            wait_for(&first, "TAKO-PERSIST-OK"),
            "1 回目のセッションでマーカーが出力される"
        );
        // クライアント破棄（tako 終了相当）。セッションはサーバー側に残る
        drop(first);

        // 2 回目: 同一コマンドで attach → 画面内容が戻っている
        let (second, _rx2) =
            crate::TerminalSession::spawn(80, 24, wrap_options(base, &socket, session))
                .expect("再 attach の tmux クライアントを spawn できる");
        assert!(
            wait_for(&second, "TAKO-PERSIST-OK"),
            "再 attach で画面内容が復元される"
        );
    }
}
