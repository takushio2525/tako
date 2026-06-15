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

/// tmux が使えるか（`tmux -V` が成功するか）。プロセス内でキャッシュする。
/// バイナリは `tmux::tmux_bin`（ログインシェル解決込み）で引く（.app の最小 PATH 対策）
pub fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        crate::tmux::tmux_command(None)
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
/// - `copy-mode-position-format ''`: copy-mode（ホイールスクロール）右上の
///   位置インジケータを消す。tmux 3.6 の既定フォーマットは先頭行タイムスタンプ
///   （`15:13 [10/77]` のような時刻表示）を含み、通常ペインのスクロール中に
///   謎の時刻として見えてしまう（2026-06-12 実機バグ (2)）。
///   スクロール位置は tako 側のスクロールバー（FR-2.5.13）が示す
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
set -sq extended-keys-format csi-u
set -as terminal-features 'xterm*:extkeys:RGB'
set -g update-environment 'TAKO_SOCKET TAKO_TOKEN TAKO_MCP_URL TAKO_TAB_ID'
set -gq copy-mode-position-format ''
";

/// ユーザー自前 tmux サーバー（ネスト tmux）向けの推奨設定スニペット（FR-2.17.5）。
/// tako ペイン内で `tmux attach` するユーザーサーバーが既定値のままだと、
/// ホイールのスクロールバック遡り（mouse off で SGR を握り潰す）と
/// Shift+Enter（extended-keys off で kitty 要求を拒否 → 素の Enter に劣化）が
/// ネスト境界で死ぬ（2026-06-12 実機バグ (1)(4) の根因）。
/// FR-2.17 のワンタップ適用・診断はこの定義を正とする。
/// 品質はネストチェーン e2e（ホイール / CSI u）で保証する
pub const NESTED_TMUX_SNIPPET: &str = "\
# tako 連携: tako ペイン内で attach した tmux でもホイール遡りと Shift+Enter を通す
set -g mouse on
# always 必須: tmux はペインからの kitty keyboard 要求（\\e[>1u。Claude Code が使う）を
# 認識しない（modifyOtherKeys 形式のみ）ため、on では S-Enter が素の Enter に劣化する
set -s extended-keys always
set -sq extended-keys-format csi-u
# 外側端末（tako バックエンド = TERM tmux-256color / iTerm2 等 = xterm-256color）が
# 拡張キー対応であることを明示する。これが無いとネスト側が CSI u 入力を解釈せず捨てる
set -as terminal-features 'tmux*:extkeys'
set -as terminal-features 'xterm*:extkeys'
# copy-mode の右上インジケータ（時刻 + [位置/履歴] 表示）を出さない
set -gq copy-mode-position-format ''
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

/// 稼働中のバックエンドサーバーへ最新 conf を再適用する。
/// conf は `-f` でサーバー**起動時**にしか読まれず、サーバーは tako の再起動を
/// 生き残る（FR-5 の永続化）ため、tako のバージョン更新で変えた設定が
/// 既存サーバーへ届かない（2026-06-12 実機バグ (2) の温床）。
/// アプリ起動時・persist 有効化時に呼ぶ。サーバー不在なら何もしない（起動もしない）
pub fn sync_conf(socket: &str) {
    let conf = ensure_conf();
    let _ = crate::tmux::tmux_command(Some(socket))
        .arg("source-file")
        .arg(&conf)
        .output();
}

/// SpawnOptions を tmux セッション経由に書き換える。
/// `options.env`（TAKO_* 注入を含む）はクライアント経由でセッション作成時の環境になる。
/// `options.cwd` は `-c` で渡す（既存セッションへの attach では tmux が無視する）
pub fn wrap_options(options: SpawnOptions, socket: &str, session: &str) -> SpawnOptions {
    let mut args = vec![
        // UTF-8 を強制する。Finder 起動の .app は LANG / LC_CTYPE が無く、tmux が
        // 非 UTF-8 クライアント扱いで CJK を `_` に置換してしまう（2026-06-12 P0:
        // 日本語が全滅した実機リグレッション）。ロケール非依存の -u が確実
        "-u".to_string(),
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
    // 内側で動かすコマンド。**未指定時はあえて渡さない**: tmux はコマンド指定があると
    // `default-shell -c <コマンド>` で実行し、この非対話 zsh ラッパーが tako の
    // シェル統合 .zshenv を読んで ZDOTDIR を消費してしまう（内側の対話シェルに
    // 統合が届かなくなる。2026-06-12 のスパイクで判明）。未指定なら tmux が
    // default-shell（$SHELL → passwd の順で解決）をログインシェルとして直接 spawn
    // するので、直接 spawn 時と同じく統合が効く。
    // 明示コマンドは残余引数が空白連結 + sh -c されるため、各語をクォートして 1 引数で渡す
    if let Some(inner) = &options.command {
        args.push(shell_quoted(inner));
    }
    SpawnOptions {
        command: Some(SpawnCommand {
            program: crate::tmux::tmux_bin().to_string(),
            args,
        }),
        ..options
    }
}

/// バックエンドセッション内ペインの tty（`/dev/ttysNNN`）。
/// ペイン配下のプロセスはこの tty を制御端末に持つため、listen ポート検知（FR-2.4.2）と
/// tmuxview の tty 突き合わせ（FR-2.13.2）はこの tty に差し替えて維持する。
/// `list-panes` を使う（`display-message -p` はクライアント無しだと空を返す）。
/// セッション未作成・tmux 不在では None（呼び出し側がリトライする）
pub fn pane_tty(socket: &str, session: &str) -> Option<String> {
    let output = crate::tmux::tmux_command(Some(socket))
        .args([
            "list-panes",
            "-t",
            &format!("={session}"),
            "-F",
            "#{pane_tty}",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let tty = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    (!tty.is_empty()).then_some(tty)
}

/// セッションを破棄する（ペインの明示 close 時。tako 終了時は呼ばない = 永続化）。
/// セッションが既に無い（シェル exit で消えた後）のエラーは無害なので潰す
pub fn kill_session(socket: &str, session: &str) {
    let _ = crate::tmux::kill_session(Some(socket), session);
}

/// orphan セッションの一括クリーンアップ（FR-2.16.11）。backend socket 上の
/// `tako-` プレフィックス・**detached**・**非 grouped**・`protected` 外のセッションを
/// kill し、kill した名前を返す。
///
/// 安全設計（誤爆防止の三重ガード）:
/// - **attached**（= いずれかのペイン/クライアントが使用中）は決して触らない
/// - **grouped**（= 表示中ビューの元セッション or その `tako-view-*` ラッパー）も触らない。
///   生きているビューの足元を崩さないため
/// - `protected`（現存ペイン・退避ペインの backend 名、表示中ビューの元/ラッパー名）は二重の安全網
///
/// これらにより、ユーザーの実セッション（既定サーバー・非 `tako-` 名）や使用中ビューは
/// 構造上 kill されない。対象は「クラッシュ等で取り残された detached な裸のバックエンド
/// セッション」だけになる
pub fn cleanup_orphans(socket: &str, protected: &std::collections::HashSet<String>) -> Vec<String> {
    let listing = crate::tmux::run_tmux(
        Some(socket),
        &[
            "list-sessions",
            "-F",
            "#{session_name}\t#{session_attached}\t#{session_grouped}",
        ],
    )
    .unwrap_or_default();
    let mut killed = Vec::new();
    for line in listing.lines() {
        let mut f = line.split('\t');
        let (Some(name), Some(attached), Some(grouped)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        if !name.starts_with("tako-") {
            continue; // tako 由来でないものは対象外
        }
        if attached != "0" {
            continue; // 使用中
        }
        if grouped != "0" {
            continue; // 表示中ビュー関連（元 or ラッパー）
        }
        if protected.contains(name) {
            continue; // 現存/退避ペイン・表示中ビューが使用中
        }
        kill_session(socket, name);
        killed.push(name.to_string());
    }
    killed
}

/// バックエンドサーバーごと落とす（セルフテストの後片付け用）
pub fn kill_server(socket: &str) {
    let _ = crate::tmux::tmux_command(Some(socket))
        .arg("kill-server")
        .output();
}

/// 語のリストを sh -c 安全な 1 つのコマンド文字列へ組み立てる
/// （terminal::login_shell_command とも共有する）
pub(crate) fn shell_quoted(command: &SpawnCommand) -> String {
    std::iter::once(&command.program)
        .chain(command.args.iter())
        .map(|w| quote_word(w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 単語のシェルクォート。英数と無害な記号のみならそのまま、他は単引用符で包む。
/// 先頭 `=` は zsh の equals 展開（`=cmd` → コマンドのフルパス）に化けるため必ず包む
/// （例: `tmux attach -t =name` の完全一致指定。2026-06-13 D&D 実装で実測）
fn quote_word(word: &str) -> String {
    let safe = !word.is_empty()
        && !word.starts_with('=')
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
    use std::process::Command;

    #[test]
    fn 単語のクォートはシェル安全() {
        assert_eq!(quote_word("/bin/zsh"), "/bin/zsh");
        assert_eq!(quote_word("-l"), "-l");
        assert_eq!(quote_word("a b"), "'a b'");
        assert_eq!(quote_word("it's"), r#"'it'\''s'"#);
        assert_eq!(quote_word(""), "''");
        // 先頭 = は zsh の equals 展開を踏むため必ず包む（途中の = は安全）
        assert_eq!(quote_word("=dnd-src"), "'=dnd-src'");
        assert_eq!(quote_word("TMUX="), "TMUX=");
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
        // バイナリはログインシェル解決で絶対パスになることがある（.app の最小 PATH 対策）
        assert!(command.program.ends_with("tmux"));
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
    fn コマンド未指定はtmuxの既定シェルに任せる() {
        let wrapped = wrap_options(SpawnOptions::default(), "tako-test", "tako-x");
        let command = wrapped.command.unwrap();
        // コマンドを渡さない（zsh -c ラッパーがシェル統合の ZDOTDIR を消費するのを
        // 避け、tmux がログインシェルを直接 spawn する経路に乗せる）
        assert_eq!(command.args.last().unwrap(), "tako-x");
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

    /// シェル統合の OSC 7 が tmux パススルー（allow-passthrough + スクリプトの包み直し）で
    /// tako 側の TapPty まで届くことの e2e（FR-2.4.1 × Phase 5.5 の共存検証）。
    /// zsh / tmux が無い環境ではスキップ
    #[test]
    #[cfg(unix)]
    fn osc7はtmuxパススルーで外へ届く() {
        if !available() {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        if !std::path::Path::new("/bin/zsh").exists() {
            eprintln!("skip: zsh が無い環境");
            return;
        }
        let socket = format!("tako-coretest-osc-{}", std::process::id());
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                kill_server(&self.0);
            }
        }
        let _cleanup = Cleanup(socket.clone());
        // シェル統合（ZDOTDIR 等）+ TAKO_PANE_ID（統合スクリプトの発動条件）。
        // コマンドは指定しない = tmux の default-shell（SHELL 環境変数）経由で
        // ログインシェルが直接 spawn され、シェル統合が本番と同じ経路で効く
        let mut env: Vec<(String, String)> = crate::shell_integration::env().to_vec();
        env.push(("TAKO_PANE_ID".into(), "1".into()));
        env.push(("SHELL".into(), "/bin/zsh".into()));
        let options = SpawnOptions {
            command: None,
            cwd: Some("/".into()),
            env,
        };
        let (mut session, mut rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, "tako-e2e-osc"))
                .expect("tmux クライアントを spawn できる");
        session.write(b"cd /private/tmp\r".to_vec());
        for _ in 0..100 {
            while let Ok(event) = rx.try_recv() {
                session.process_event(event);
            }
            if session.cwd() == Some(std::path::Path::new("/private/tmp")) {
                return; // OSC 7 がパススルーで届いた
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!(
            "OSC 7 が届かない。画面: {:?}",
            session.visible_lines().join("\n")
        );
    }

    /// マウスレポートと拡張キー（CSI u）が tmux 越しでも**生のまま**内側アプリへ届く e2e。
    /// 「アプリがマウスレポートを要求したら必ず生のマウスイベントが届く」は tako の
    /// 存在意義に関わる保証（2026-06-12 実機リグレッションの再発防止）。
    /// 内側は受信バイトを可視化する `cat -v`（^[ = ESC）
    #[test]
    #[cfg(unix)]
    fn マウスレポートと拡張キーがtmux越しに生で届く() {
        if !available() {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let socket = format!("tako-coretest-mouse-{}", std::process::id());
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                kill_server(&self.0);
            }
        }
        let _cleanup = Cleanup(socket.clone());
        // 内側アプリ: SGR マウス + kitty keyboard を要求してから受信バイトを表示
        let options = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    r"printf '\033[?1000h\033[?1006h\033[>1u'; exec cat -v".into(),
                ],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
        };
        let (session, _rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, "tako-e2e-mouse"))
                .expect("tmux クライアントを spawn できる");

        // 内側のマウス要求が tmux → 外側端末（tako の Term）まで伝わる
        let mut mouse_on = false;
        for _ in 0..100 {
            if session.mouse_reporting() {
                mouse_on = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(
            mouse_on,
            "内側アプリのマウス要求が外側端末モードへ伝わる。画面: {:?}",
            session.visible_lines().join("\n")
        );

        // ホイール → 生の SGR マウスイベントが内側アプリへ届く（矢印キー変換は禁止）
        session.scroll_wheel(1, 5, 5);
        let mut delivered = false;
        for _ in 0..50 {
            let lines = session.visible_lines().join("\n");
            assert!(
                !lines.contains("^[[A") && !lines.contains("^[OA"),
                "ホイールが矢印キーに化けている（リグレッション）。画面: {lines:?}"
            );
            if lines.contains("[<64;6;6M") {
                delivered = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(
            delivered,
            "生の SGR ホイールイベントが届かない。画面: {:?}",
            session.visible_lines().join("\n")
        );

        // Shift+Enter（CSI u）も tmux 越しで**kitty 形式のまま**内側へ届く
        // （extended-keys always + extended-keys-format csi-u。FR の常用要件）
        session.write(b"\x1b[13;2u".to_vec());
        let mut key_delivered = false;
        for _ in 0..50 {
            if session.visible_lines().join("\n").contains("[13;2u") {
                key_delivered = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(
            key_delivered,
            "Shift+Enter（CSI u）が tmux 越しに kitty 形式で届かない。画面: {:?}",
            session.visible_lines().join("\n")
        );
        // 外側（tako の Term）には拡張キーモードが伝わらない（tmux の仕様）。
        // そのため UI 層はバックエンドペインで disambiguate を強制する（main.rs の
        // handle_key）。ここでは前提（伝わらない）が変わったら気づけるよう記録する
        eprintln!(
            "外側 disambiguate = {}（false 想定。true になったら main.rs の強制は不要）",
            session.disambiguate_keys()
        );

        // Esc 単押し（素の \e。UI 層 handle_key はバックエンドペインで Esc を
        // CSI 27u にしない = CsiUMode::ModifiedOnly）も内側ペインへ素のまま届く。
        // tmux は CSI 27u を内側の kitty 要求に関係なく素通しするため、CSI u に
        // すると非対応アプリで「27u」が文字化けする（2026-06-12 実機バグ）。
        // 素の \e は escape-time で正しく解釈され素のまま届く（その固定）
        session.write(b"\x1b".to_vec());
        session.write(b"ESC-RAW\r".to_vec());
        let mut esc_delivered = false;
        for _ in 0..50 {
            if session.visible_lines().join("\n").contains("^[ESC-RAW") {
                esc_delivered = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(
            esc_delivered,
            "Esc（素の \\e）が tmux 越しに素のまま届かない。画面: {:?}",
            session.visible_lines().join("\n")
        );
        assert!(
            !session.visible_lines().join("\n").contains("27u"),
            "Esc が CSI 27u 断片として漏れている（2026-06-12 実機バグの回帰）。画面: {:?}",
            session.visible_lines().join("\n")
        );
    }

    /// Esc 単押しが「kitty を要求していない」内側アプリ（素の zsh 相当）にも
    /// 素の \e のまま届き、「27u」が文字として漏れない e2e
    /// （2026-06-12 実機バグの再発防止）。
    /// 後半は前提のカナリア: tmux が受信 CSI 27u を非要求ペインへ素通しすること
    /// （= UI 層が Esc を CSI u で送ってはいけない理由）を観測ログに残す。
    /// tmux 側が将来「非要求ペインへはレガシー再エンコード」に変われば
    /// CsiUMode::ModifiedOnly の Esc 例外は不要にできる
    #[test]
    #[cfg(unix)]
    fn esc単押しは非kittyアプリにも素のescで届き27uが漏れない() {
        if !available() {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let socket = format!("tako-coretest-esc-{}", std::process::id());
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                kill_server(&self.0);
            }
        }
        let _cleanup = Cleanup(socket.clone());
        // 内側アプリ: kitty を**要求しない** cat -v（素の zsh で Esc を押した状況の再現）
        let options = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "echo TAKO-ESC-READY; exec cat -v".into()],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
        };
        let (session, _rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, "tako-e2e-esc"))
                .expect("tmux クライアントを spawn できる");
        let wait_for = |needle: &str| -> bool {
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
        };
        assert!(
            wait_for("TAKO-ESC-READY"),
            "内側アプリが立ち上がらない。画面: {:?}",
            session.visible_lines().join("\n")
        );
        // UI 層（handle_key の CsiUMode::ModifiedOnly）と同じバイト列: Esc は素の \e
        session.write(b"\x1b".to_vec());
        session.write(b"ESC-RAW\r".to_vec());
        assert!(
            wait_for("^[ESC-RAW"),
            "Esc（素の \\e）が内側へ素のまま届かない。画面: {:?}",
            session.visible_lines().join("\n")
        );
        assert!(
            !session.visible_lines().join("\n").contains("27u"),
            "Esc 単押しで「27u」が文字として漏れた（2026-06-12 実機バグの回帰）。画面: {:?}",
            session.visible_lines().join("\n")
        );
        // カナリア: CSI 27u は非要求ペインにも素通しされる（tmux 3.6 の実測挙動。
        // これが変わったら main.rs の Esc 例外を見直せる）
        session.write(b"\x1b[27u".to_vec());
        session.write(b"\r".to_vec());
        let passthrough = wait_for("^[[27u");
        eprintln!(
            "CSI 27u の非要求ペインへの素通し = {passthrough}（true 想定。false になったら \
             tmux が再エンコードするようになった = CsiUMode::ModifiedOnly の Esc 例外を再検討）"
        );
    }

    /// ネスト tmux（バックエンド → ユーザー自前 tmux → アプリ）のチェーン e2e 用ヘルパ。
    /// ユーザーサーバー側は NESTED_TMUX_SNIPPET（FR-2.17 の推奨設定）で起動する
    #[cfg(unix)]
    fn spawn_nested(
        backend_socket: &str,
        nested_socket: &str,
        inner_cmd: &str,
    ) -> crate::TerminalSession {
        let conf_path = std::env::temp_dir().join(format!("tako-nest-conf-{nested_socket}"));
        std::fs::write(&conf_path, NESTED_TMUX_SNIPPET).expect("ネスト conf を書ける");
        // バックエンドペインの中でユーザー tmux サーバーへ new-session する
        // （実機の「自前 tmux セッションを tako 内で attach」構成の再現）
        let options = SpawnOptions {
            command: Some(SpawnCommand {
                program: crate::tmux::tmux_bin().to_string(),
                args: vec![
                    "-u".into(),
                    "-L".into(),
                    nested_socket.into(),
                    "-f".into(),
                    conf_path.display().to_string(),
                    "new-session".into(),
                    "-A".into(),
                    "-s".into(),
                    "nest".into(),
                    inner_cmd.into(),
                ],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
        };
        let (session, _rx) = crate::TerminalSession::spawn(
            80,
            24,
            wrap_options(options, backend_socket, "tako-e2e-nest"),
        )
        .expect("ネスト構成を spawn できる");
        session
    }

    /// ネスト tmux 越しのホイールがユーザーサーバーの copy-mode スクロールに乗る e2e
    /// （2026-06-12 実機バグ (1) の再発防止。NESTED_TMUX_SNIPPET の mouse on が前提）。
    /// 経路: tako の SGR → バックエンド tmux（mouse_any=1 で send -M 生転送）→
    /// ネスト tmux（mouse on）→ copy-mode でネスト側スクロールバックを遡る
    #[test]
    #[cfg(unix)]
    fn ネストtmux越しのホイールで内側スクロールバックを遡れる() {
        if !available() {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let backend = format!("tako-coretest-nestw-{}", std::process::id());
        let nested = format!("tako-coretest-nestw-in-{}", std::process::id());
        struct Cleanup(String, String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                kill_server(&self.0);
                kill_server(&self.1);
            }
        }
        let _cleanup = Cleanup(backend.clone(), nested.clone());
        let session = spawn_nested(
            &backend,
            &nested,
            "i=0; while [ $i -lt 100 ]; do echo LINE-$i; i=$((i+1)); done; exec sleep 60",
        );
        // ネスト内の出力完了 + 外側のマウスモード（バックエンド mouse on）を待つ
        let mut ready = false;
        for _ in 0..100 {
            if session.mouse_reporting()
                && session
                    .visible_lines()
                    .iter()
                    .any(|l| l.trim_end() == "LINE-99")
            {
                ready = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(
            ready,
            "ネスト構成が立ち上がらない。画面: {:?}",
            session.visible_lines().join("\n")
        );
        // ホイール上 → ネスト tmux の copy-mode で遡る（過去の LINE-n が見える）
        session.scroll_wheel(3, 10, 10);
        let mut scrolled = false;
        for _ in 0..50 {
            let top_n = session
                .visible_lines()
                .first()
                .map(|l| l.trim_end().to_string())
                .and_then(|t| {
                    t.strip_prefix("LINE-")
                        .and_then(|s| s.parse::<usize>().ok())
                });
            if let Some(n) = top_n {
                if n < 77 {
                    scrolled = true;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(
            scrolled,
            "ネスト越しのホイールでスクロールバックを遡れない（バグ (1) の回帰）。画面: {:?}",
            session.visible_lines().join("\n")
        );
    }

    /// ネスト tmux 越しの CSI u（Shift+Enter）が最内のアプリへ kitty 形式のまま届く e2e
    /// （2026-06-12 実機バグ (4) の再発防止。NESTED_TMUX_SNIPPET の extended-keys on +
    /// バックエンド conf の extended-keys always が両輪）。
    /// 最内は kitty を要求して受信バイトを可視化する cat -v
    #[test]
    #[cfg(unix)]
    fn ネストtmux越しのcsi_uが最内アプリへ届く() {
        if !available() {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let backend = format!("tako-coretest-nestk-{}", std::process::id());
        let nested = format!("tako-coretest-nestk-in-{}", std::process::id());
        struct Cleanup(String, String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                kill_server(&self.0);
                kill_server(&self.1);
            }
        }
        let _cleanup = Cleanup(backend.clone(), nested.clone());
        let session = spawn_nested(
            &backend,
            &nested,
            r"printf '\033[>1u'; echo TAKO-NEST-'READY'; exec cat -v",
        );
        let wait_for = |needle: &str| -> bool {
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
        };
        assert!(
            wait_for("TAKO-NEST-READY"),
            "ネスト構成が立ち上がらない。画面: {:?}",
            session.visible_lines().join("\n")
        );
        // Shift+Enter（CSI u）。バックエンドペインは UI 層が CSI u 送出を常時有効化
        // するため、ここでも生の CSI u を書く（handle_key と同じバイト列）
        session.write(b"\x1b[13;2u".to_vec());
        assert!(
            wait_for("[13;2u"),
            "CSI u がネスト tmux 越しに素の Enter へ劣化した（バグ (4) の回帰）。画面: {:?}",
            session.visible_lines().join("\n")
        );
    }

    /// CJK が tmux 越しでも描画される e2e（2026-06-12 P0 リグレッションの再発防止）。
    /// Finder 起動の .app はロケール環境変数が無い（= POSIX ロケール）。それを LC_ALL=C の
    /// 強制で再現し、`-u`（UTF-8 強制）が効いて日本語が `_` に置換されないことを検証する
    #[test]
    #[cfg(unix)]
    fn cjkはロケール無し環境でもtmux越しに描画される() {
        if !available() {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let socket = format!("tako-coretest-cjk-{}", std::process::id());
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                kill_server(&self.0);
            }
        }
        let _cleanup = Cleanup(socket.clone());
        let options = SpawnOptions {
            // 出力経路を直接検証する（タイプ入力を経由しない）: 日本語を printf して待機
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    "printf '日本語テストOK\\n'; exec sleep 30".into(),
                ],
            }),
            cwd: Some(std::env::temp_dir()),
            // .app（Finder 起動）のロケール無し環境を再現する（テスト実行シェルの
            // LANG を C で上書き。子プロセスへは合成 env が優先で渡る）
            env: vec![("LC_ALL".into(), "C".into()), ("LANG".into(), "C".into())],
        };
        let (session, _rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, "tako-e2e-cjk"))
                .expect("tmux クライアントを spawn できる");
        for _ in 0..100 {
            let lines = session.visible_lines().join("\n");
            if lines.contains("日本語テストOK") {
                return; // CJK がそのまま描画された
            }
            // tmux が非 UTF-8 扱いすると _ に置換される（P0 の症状）
            assert!(
                !lines.contains("____"),
                "CJK が _ に置換されている（tmux のロケール退行）。画面: {lines:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!(
            "CJK 出力が現れない。画面: {:?}",
            session.visible_lines().join("\n")
        );
    }

    /// 通常画面・非マウスのペイン（素のシェルや Claude Code）へのホイールは
    /// バックエンド tmux の copy-mode でスクロールバックを遡り、かつ右上に
    /// 位置インジケータ（tmux 3.6 既定は先頭行タイムスタンプ = 時刻を含む）を
    /// **描かない**ことの e2e（2026-06-12 実機バグ (2) の再発防止。
    /// conf の `copy-mode-position-format ''` が回帰検知の対象）
    #[test]
    #[cfg(unix)]
    fn 通常ペインのホイールはcopy_modeで遡りインジケータを出さない() {
        if !available() {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let socket = format!("tako-coretest-ind-{}", std::process::id());
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                kill_server(&self.0);
            }
        }
        let _cleanup = Cleanup(socket.clone());
        // 100 行出力して待機する sh（通常画面・非マウス。Claude Code と同型）
        let options = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    "i=0; while [ $i -lt 100 ]; do echo LINE-$i; i=$((i+1)); done; exec sleep 60"
                        .into(),
                ],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
        };
        let (session, _rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, "tako-e2e-ind"))
                .expect("tmux クライアントを spawn できる");
        let wait_top = |pred: &dyn Fn(&str) -> bool| -> Option<String> {
            for _ in 0..100 {
                let lines = session.visible_lines();
                if let Some(top) = lines.first() {
                    if pred(top) {
                        return Some(top.clone());
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            session.visible_lines().first().cloned()
        };
        // 出力完了（最終行が見えている）を待つ
        for _ in 0..100 {
            if session
                .visible_lines()
                .iter()
                .any(|l| l.trim_end() == "LINE-99")
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        // ホイール上 → copy-mode で遡る（1 イベント目が copy-mode 入り、以後スクロール）
        session.scroll_wheel(3, 10, 10);
        let top = wait_top(&|top| {
            let t = top.trim_end();
            t.starts_with("LINE-") && t != "LINE-77"
        })
        .expect("先頭行が取れる");
        let t = top.trim_end();
        let n: usize = t
            .strip_prefix("LINE-")
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| {
                panic!("スクロール後の先頭行が LINE-n でない（インジケータ等の混入）: {top:?}")
            });
        assert!(n < 77, "ホイールで遡れていない。先頭行: {top:?}");
        // 行全体が LINE-n のみ = 右上に時刻 / [位置/履歴] インジケータが無い
        assert_eq!(
            t,
            format!("LINE-{n}"),
            "右上に位置インジケータが描かれている（バグ (2) の回帰）: {top:?}"
        );
        // ホイール下で最下部へ戻ると copy-mode が解けて元の画面（LINE-99）に戻る
        session.scroll_wheel(-30, 10, 10);
        let mut back = false;
        for _ in 0..50 {
            if session
                .visible_lines()
                .iter()
                .any(|l| l.trim_end() == "LINE-99")
            {
                back = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(back, "ホイール下で最下部へ戻らない");
    }

    /// sync_conf が**稼働中**のサーバーへ最新 conf を再適用することの e2e。
    /// サーバーは tako 再起動を生き残るため、これが無いと conf 更新が永久に届かない
    #[test]
    #[cfg(unix)]
    fn sync_confは稼働中サーバーへ設定を再適用する() {
        if !available() {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let socket = format!("tako-coretest-sync-{}", std::process::id());
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                kill_server(&self.0);
            }
        }
        let _cleanup = Cleanup(socket.clone());
        let tmux = crate::tmux::tmux_bin();
        // 旧バージョン相当: conf 無し（/dev/null）でサーバーを起動しておく
        let status = Command::new(tmux)
            .args([
                "-L",
                &socket,
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                "x",
            ])
            .arg("sleep 30")
            .status()
            .expect("tmux サーバーを起動できる");
        assert!(status.success());
        let show = |opt: &str| -> Option<String> {
            let out = Command::new(tmux)
                .args(["-L", &socket, "show-options", "-g", "-v", opt])
                .output()
                .ok()?;
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        };
        // 既定では copy-mode-position-format が空でない（tmux 3.6+。
        // オプション自体が無い古い tmux では検証をスキップ）
        let Some(before) = Command::new(tmux)
            .args([
                "-L",
                &socket,
                "show-options",
                "-g",
                "-w",
                "-v",
                "copy-mode-position-format",
            ])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        else {
            eprintln!("skip: copy-mode-position-format 非対応の tmux");
            return;
        };
        assert!(
            !before.is_empty(),
            "前提が変わった: 既定でインジケータが空（テストの意味が無い）"
        );
        sync_conf(&socket);
        let after = Command::new(tmux)
            .args([
                "-L",
                &socket,
                "show-options",
                "-g",
                "-w",
                "-v",
                "copy-mode-position-format",
            ])
            .output()
            .expect("show-options が動く");
        assert_eq!(
            String::from_utf8_lossy(&after.stdout).trim(),
            "",
            "sync_conf 後もインジケータ書式が既定のまま（再適用されていない）"
        );
        // 他の主要設定も同期されている（mouse on は wheel 配送の前提）
        assert_eq!(show("mouse").as_deref(), Some("on"));
    }

    /// マウス**非要求**の alt-screen アプリ（ペイン内 `tmux attach` のネストや全画面 TUI）への
    /// ホイールが矢印キーに化けない e2e（2026-06-12 実機リグレッション (1) の再発防止）。
    /// tmux の既定はこの構成でホイール → ↑↓ 変換（入力履歴が回る事故の元）なので、
    /// バックエンド conf がこれを抑止していることを検証する
    #[test]
    #[cfg(unix)]
    fn alt_screenの非マウスペインでホイールが矢印に化けない() {
        if !available() {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let socket = format!("tako-coretest-alt-{}", std::process::id());
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                kill_server(&self.0);
            }
        }
        let _cleanup = Cleanup(socket.clone());
        // 内側: alt screen に入るだけでマウスは要求しない（claude を内包する
        // ネスト tmux クライアントや less / vim 既定がこの形）
        let options = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), r"printf '\033[?1049h'; exec cat -v".into()],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
        };
        let (session, _rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, "tako-e2e-alt"))
                .expect("tmux クライアントを spawn できる");
        // 外側のマウスモード（バックエンドの mouse on）を待つ
        for _ in 0..100 {
            if session.mouse_reporting() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(session.mouse_reporting(), "バックエンドの mouse on が効く");
        // 上下ホイール → 矢印キーが内側へ送られないこと
        session.scroll_wheel(1, 5, 5);
        session.scroll_wheel(-1, 5, 5);
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let lines = session.visible_lines().join("\n");
        assert!(
            !lines.contains("^[[A") && !lines.contains("^[OA") && !lines.contains("^[[B"),
            "ホイールが矢印キーに化けている（リグレッション (1)）。画面: {lines:?}"
        );
    }
}
