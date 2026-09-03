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
//! - tmux 不在環境では呼び出し側（tako-app）が `crate::backend::capabilities().survives_app_exit` を見て従来の直接 spawn へ
//!   無害に劣化する（ゼロコンフィグ原則）
//! - サーバーは専用 conf（`<data_dir>/tmux-backend.conf`）で起動し、ユーザーの
//!   `~/.tmux.conf` は読まない（status バー・prefix キー等が見えない裏方に徹する）

use std::path::{Path, PathBuf};
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

/// tmux バイナリが実在して動くか（`tmux -V` が成功するか）。プロセス内でキャッシュする。
/// バイナリは `tmux::tmux_bin`（ログインシェル解決込み）で引く（.app の最小 PATH 対策）。
///
/// これは**環境の事実**であって選択ではない。選択は `backend::choice()` が決める
pub fn tmux_binary_present() -> bool {
    static PRESENT: OnceLock<bool> = OnceLock::new();
    *PRESENT.get_or_init(|| {
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
/// - `update-environment`: 再 attach 時にセッション環境の TAKO_SOCKET / TAKO_TOKEN /
///   TAKO_MCP_URL を新インスタンスの値へ更新する（既存プロセスには届かないが、
///   それは CLI の control.json フォールバック = FR-2.2.9 が吸収する）。
///   TAKO_PANE_ID / TAKO_TAB_ID はペイン固有の値のため update-environment には入れず、
///   `wrap_options` で `new-session -e` により各セッションに直接注入する
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
set -g update-environment 'TAKO_SOCKET TAKO_TOKEN TAKO_MCP_URL'
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
/// 書けない環境では `/dev/null` を返し「ユーザー conf を読まない」ことだけは維持する。
///
/// **一時ファイル → rename で差し替える**（#625）。`wrap_options` はペインを spawn する
/// たびにここを通るので、複数ペインを同時に立てると「書き手が truncate している最中の
/// conf」を、別ペインが起動した tmux サーバーが `-f` で読みうる。読ませてしまうと
/// サーバーは既定設定（status on / mouse off / extended-keys off / prefix C-b）で
/// 立ち上がり、ステータスバーが出る・ホイールが素通しされない・Shift+Enter が
/// 素の Enter に劣化する（#28 / #167 と同じ症状クラス）。rename は同一ディレクトリ内で
/// 原子的なので、読み手は常に完全な conf を見る
fn ensure_conf() -> PathBuf {
    data_dir()
        .and_then(|dir| write_conf_in(&dir).ok())
        .unwrap_or_else(|| PathBuf::from("/dev/null"))
}

/// conf を `dir` へ原子的に置き、そのパスを返す
fn write_conf_in(dir: &Path) -> std::io::Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);

    std::fs::create_dir_all(dir)?;
    let path = dir.join("tmux-backend.conf");
    // tmp 名は **書き込み 1 回ごとに** 固有にする。プロセス固有までしか分けないと、
    // 同時に走った書き手同士が同じ tmp を奪い合い（A が truncate 中に B が rename）、
    // 途中状態がそのまま原子的に差し替わってしまう（この修正を作る過程で実測）。
    // data_dir はプライマリ / セカンダリでも共有されうるので pid も併記する
    let tmp = dir.join(format!(
        "tmux-backend.conf.{}.{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, BACKEND_CONF)?;
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(path)
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
    // ペイン固有の環境変数を tmux new-session -e で直接注入する（tmux 3.2+）。
    // tmux サーバーのグローバル環境は最初のクライアントから継承され、後続セッションも
    // その stale な値を使う。-e はセッション作成時に値を確定させるため、
    // シェル起動後の set-environment（タイミング問題）やクライアント環境の継承に依存しない。
    // **シェル統合の置き場（ZDOTDIR 等）もここに含める**（#1105）: 含めないと、同じ
    // socket 名に別インスタンスのサーバーが残っているときに前のインスタンスの
    // 置き場を指し、OSC 7 / 133 が一切届かなくなる（cwd 追従とコマンド状態が黙って死ぬ）
    for (key, val) in crate::backend::session_pinned_pairs(&options.env, socket) {
        args.push("-e".to_string());
        args.push(format!("{key}={val}"));
    }
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
    // 明示コマンドは残余引数が空白連結 + sh -c されるため、各語をクォートして 1 引数で渡す。
    // **第 1 語の書き方だけは器によって違う**ので組み立ては backend 側の 1 か所へ（#881）
    if let Some(inner) = &options.command {
        args.push(crate::backend::inner_command_line(inner));
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
            &crate::tmux::exact_target(session),
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

/// セッションの現在の作業ディレクトリを取得する（orphan 復帰時のタブ名推定用）
pub fn session_cwd(socket: &str, session: &str) -> Option<String> {
    let output = crate::tmux::tmux_command(Some(socket))
        .args([
            "list-panes",
            "-t",
            &crate::tmux::exact_target(session),
            "-F",
            "#{pane_current_path}",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    (!path.is_empty()).then_some(path)
}

/// セッション環境変数を読む（`tmux show-environment -t <session> <name>`）。
/// orphan 復元時に `TAKO_ORCHESTRATOR_ROLE` / `TAKO_PANE_ID` を取り出す用途（#210）
pub fn session_env(socket: &str, session: &str, name: &str) -> Option<String> {
    let output = crate::tmux::tmux_command(Some(socket))
        .args([
            "show-environment",
            "-t",
            &crate::tmux::exact_target(session),
            name,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&output.stdout).trim().to_string();
    // "NAME=value" 形式。`-NAME` は unset を意味する
    line.strip_prefix(&format!("{name}=")).map(str::to_string)
}

/// セッション環境変数を設定する（`tmux set-environment -t <session> <name> <value>`）。
/// orphan 復元後に TAKO_PANE_ID を新 pane ID に更新する用途（#210）
pub fn set_session_env(socket: &str, session: &str, name: &str, value: &str) {
    let _ = crate::tmux::tmux_command(Some(socket))
        .args([
            "set-environment",
            "-t",
            &crate::tmux::exact_target(session),
            name,
            value,
        ])
        .status();
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
/// layout.json に載っていない生存中の `tako-*` セッション（orphan）を返す。
/// cleanup_orphans と同じ list-sessions を読むが、kill せずに名前一覧だけ返す。
/// 起動時の自動復帰（#191）で、layout 復元では拾えなかったセッションを発見するのに使う
pub fn find_orphans(socket: &str, protected: &std::collections::HashSet<String>) -> Vec<String> {
    let listing = crate::tmux::run_tmux(
        Some(socket),
        &[
            "list-sessions",
            "-F",
            "#{session_name}\t#{session_attached}\t#{session_grouped}",
        ],
    )
    .unwrap_or_default();
    let mut orphans = Vec::new();
    for line in listing.lines() {
        let mut f = line.split('\t');
        let (Some(name), Some(_attached), Some(grouped)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        if !name.starts_with("tako-") {
            continue;
        }
        if name.starts_with("tako-view-") {
            continue;
        }
        if grouped != "0" {
            continue;
        }
        if protected.contains(name) {
            continue;
        }
        orphans.push(name.to_string());
    }
    orphans
}

/// 安全設計（誤爆防止の四重ガード）:
/// - **attached**（= いずれかのペイン/クライアントが使用中）は決して触らない
/// - **grouped**（= 表示中ビューの元セッション or その `tako-view-*` ラッパー）も触らない。
///   生きているビューの足元を崩さないため
/// - `protected`（現存ペイン・バックグラウンドペインの backend 名、表示中ビューの元/ラッパー名）は二重の安全網
/// - `min_idle_secs` を指定すると、最終アクティビティ（`session_activity`）がそれより
///   新しいセッションも触らない。起動時の自動実行が「直前まで動いていた実行中セッション」を
///   巻き込まないための猶予（Issue #113: 多重起動の layout.json 汚染で protected から
///   漏れた実行中 worker を、次回起動の自動 cleanup が実プロセスごと kill した）。
///   明示操作（`tako tmux cleanup` / MCP）は None = 従来どおり全対象
///
/// これらにより、ユーザーの実セッション（既定サーバー・非 `tako-` 名）や使用中ビューは
/// 構造上 kill されない。対象は「クラッシュ等で取り残された detached な裸のバックエンド
/// セッション」だけになる
pub fn cleanup_orphans(
    socket: &str,
    protected: &std::collections::HashSet<String>,
    min_idle_secs: Option<u64>,
) -> Vec<String> {
    let listing = crate::tmux::run_tmux(
        Some(socket),
        &[
            "list-sessions",
            "-F",
            "#{session_name}\t#{session_attached}\t#{session_grouped}\t#{session_activity}",
        ],
    )
    .unwrap_or_default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
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
            continue; // 現存/バックグラウンドペイン・表示中ビューが使用中
        }
        if let Some(min_idle) = min_idle_secs {
            // activity が取れない（古い tmux・パース不能 = 0）場合は「idle 十分」に倒し
            // 従来挙動（掃除する）へ劣化する
            let activity: u64 = f.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            if now.saturating_sub(activity) < min_idle {
                continue; // 直近までアクティブ = 実行中プロセスの可能性が高い。この回は見送る
            }
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
    remove_socket_file(socket);
}

/// tmux ソケットファイルを削除する。tmux は kill-server 後もファイルを残すことがある。
/// tmux は TMUX_TMPDIR → /tmp の順でソケットディレクトリを決定する（TMPDIR は使わない）。
/// macOS では /tmp → /private/tmp のシンボリックリンク解決でソケット名末尾に `=` が付くため
/// 両方試す
fn remove_socket_file(socket: &str) {
    let Some(base) = socket_dir() else { return };
    let _ = std::fs::remove_file(base.join(socket));
    let _ = std::fs::remove_file(base.join(format!("{socket}=")));
}

/// tmux がソケットを置くディレクトリ（`$TMUX_TMPDIR|/tmp` の `tmux-<uid>`）。
/// Windows には tmux もこのレイアウトも存在しないため `None`
#[cfg(unix)]
fn socket_dir() -> Option<std::path::PathBuf> {
    let uid = unsafe { libc::getuid() };
    let tmpdir = std::env::var("TMUX_TMPDIR").unwrap_or_else(|_| "/tmp".into());
    Some(std::path::Path::new(&tmpdir).join(format!("tmux-{uid}")))
}

#[cfg(windows)]
fn socket_dir() -> Option<std::path::PathBuf> {
    None
}

/// 語のリストを sh -c 安全な 1 つのコマンド文字列へ組み立てる
/// （terminal::login_shell_command とも共有する）
pub(crate) fn shell_quoted(command: &SpawnCommand) -> String {
    std::iter::once(&command.program)
        .chain(command.args.iter())
        .map(|w| crate::shell::quote_for_shell(w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// テスト用: tmux 隔離ソケットの後始末ガード。
/// 生成時に前回テストの残骸ソケット（tako-coretest-*）を掃除し、Drop でサーバー kill +
/// ソケットファイル削除を行う
#[cfg(test)]
pub(crate) struct TmuxTestGuard(Vec<String>);

#[cfg(test)]
impl TmuxTestGuard {
    pub fn new(sockets: Vec<String>) -> Self {
        Self::cleanup_stale_sockets();
        Self(sockets)
    }

    /// 前回テストの残骸（tako-coretest-* ソケット + ゾンビサーバー）を一括掃除する。
    /// テスト途中の kill -9 で Drop が走らずサーバーが残る場合の回収
    fn cleanup_stale_sockets() {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let Some(dir) = socket_dir() else { return };
            let Ok(entries) = std::fs::read_dir(&dir) else {
                return;
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                if is_stale_socket(name, process_alive) {
                    kill_server(name.trim_end_matches('='));
                }
            }
        });
    }
}

/// テストソケット名（`tako-coretest-<用途>-<pid>`）の所有プロセス ID
#[cfg(test)]
fn socket_owner_pid(name: &str) -> Option<u32> {
    name.rsplit('-').next()?.parse().ok()
}

/// 掃除してよい残骸ソケットか。
///
/// **「自分の pid を含まない = 残骸」で判定してはいけない**（#625 の隔離破れ）。
/// 別ブランチの worker が同時に `cargo test` を回すと、後から起動した側の掃除が
/// 先行プロセスの**生きている**サーバーを kill + ソケット削除してしまい、
/// 相手側の tmux e2e が `[server exited]` で総崩れになる（1 掃除で 5 本同時に
/// 落ちるのを実測）。所有プロセスが生きていれば残骸ではない。
///
/// 命名規約から外れて所有者を特定できない名前は**触らない**（安全側に倒す）
#[cfg(test)]
fn is_stale_socket(name: &str, is_alive: impl Fn(u32) -> bool) -> bool {
    // tmux は /tmp → /private/tmp の解決でソケット名末尾に `=` を付けることがある
    let base = name.trim_end_matches('=');
    if !base.starts_with("tako-coretest-") {
        return false;
    }
    socket_owner_pid(base).is_some_and(|pid| !is_alive(pid))
}

/// プロセスが生きているか。`kill(pid, 0)` はシグナルを送らず存在と権限だけを見る
/// （EPERM = 別ユーザーの生存プロセス）
#[cfg(all(test, unix))]
fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        // 0 はプロセスグループ指定になるため所有者判定には使わない（= 触らない）
        return true;
    }
    if unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Windows には tmux ソケットのレイアウトが無く `cleanup_stale_sockets` は
/// `socket_dir()` が None を返して早期 return するが、コンパイルは通す必要がある
#[cfg(all(test, windows))]
fn process_alive(_pid: u32) -> bool {
    true
}

#[cfg(test)]
impl Drop for TmuxTestGuard {
    fn drop(&mut self) {
        for socket in &self.0 {
            kill_server(socket);
        }
    }
}

/// テスト用: ペインが alt screen（`\033[?1049h`）へ切り替わり終えるのを待つ。
/// 切り替わったら `Some(true)`、10 秒待っても切り替わらなければ最後に観測した値、
/// ペインごと消えていれば `None`。
///
/// **「履歴ゼロ」を切替の証跡に使ってはいけない**（#625 のフレークの根因）。
/// 内側が非対話シェル（`sh -c '…'`）のペインはプロンプトを出さないので
/// スクロールバックが spawn 直後から 0 行であり、`history_size == 0` は
/// 切替を待たずに真になる。切替前にキーを書き込むと、その入力は**通常画面**へ
/// エコーされ、直後の `?1049h` が alt screen を消去するので画面から消える
/// （カーソル桁だけが進んだ状態が残る）。並列負荷でシェルの起動が遅れると
/// この窓が開き、テストが確率的に落ちていた。
/// tmux 自身の `#{alternate_on}` が切替の唯一の直接的な証跡になる
#[cfg(all(test, unix))]
pub(crate) fn wait_alt_screen(socket: &str, session: &str) -> Option<bool> {
    let mut last = None;
    for _ in 0..100 {
        last = alternate_on(socket, session);
        if last == Some(true) {
            return last;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    last
}

/// アクティブペインが alt screen 表示中か。ペイン不在・tmux 不在では None
#[cfg(all(test, unix))]
fn alternate_on(socket: &str, session: &str) -> Option<bool> {
    let output = crate::tmux::run_tmux(
        Some(socket),
        &[
            "list-panes",
            "-t",
            &crate::tmux::session_pane_target(session),
            "-F",
            "#{pane_active}\t#{alternate_on}",
        ],
    )
    .ok()?;
    output.lines().find_map(|line| {
        let mut f = line.split('\t');
        (f.next()? == "1").then(|| f.next() == Some("1"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn 単語のクォートはシェル安全() {
        use crate::shell::quote_for_shell;
        assert_eq!(quote_for_shell("/bin/zsh"), "/bin/zsh");
        assert_eq!(quote_for_shell("-l"), "-l");
        assert_eq!(quote_for_shell("a b"), "'a b'");
        assert_eq!(quote_for_shell("it's"), r#"'it'\''s'"#);
        assert_eq!(quote_for_shell(""), "''");
        // 先頭 = は zsh の equals 展開を踏むため必ず包む（途中の = は安全）
        assert_eq!(quote_for_shell("=dnd-src"), "'=dnd-src'");
        assert_eq!(quote_for_shell("TMUX="), "TMUX=");
        assert_eq!(
            shell_quoted(&SpawnCommand {
                program: "npm".into(),
                args: vec!["run".into(), "dev server".into()],
            }),
            "npm run 'dev server'"
        );
    }

    /// conf の差し替え中でも、読み手は「完全な conf」しか観測しない（#625）。
    /// `wrap_options` はペイン spawn のたびに conf を書き直すので、複数ペインを同時に
    /// 立てると別ペインが起動する tmux サーバーの `-f` 読み取りと重なる。途中状態を
    /// 読ませると既定設定のサーバーが立ち、ステータスバー表示・ホイール素通し不可・
    /// Shift+Enter 劣化を起こす
    #[test]
    fn conf差し替え中も読み手は完全な内容しか見ない() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::Arc;

        let dir = std::env::temp_dir().join(format!("tako-conf-625-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = write_conf_in(&dir).expect("conf を置ける");

        let stop = Arc::new(AtomicBool::new(false));
        let partial = Arc::new(AtomicUsize::new(0));
        let reads = Arc::new(AtomicUsize::new(0));
        let mut threads = Vec::new();
        for _ in 0..4 {
            let (d, s) = (dir.clone(), stop.clone());
            threads.push(std::thread::spawn(move || {
                while !s.load(Ordering::Relaxed) {
                    let _ = write_conf_in(&d);
                }
            }));
        }
        for _ in 0..2 {
            let (p, s, bad, n) = (path.clone(), stop.clone(), partial.clone(), reads.clone());
            threads.push(std::thread::spawn(move || {
                while !s.load(Ordering::Relaxed) {
                    if let Ok(body) = std::fs::read_to_string(&p) {
                        n.fetch_add(1, Ordering::Relaxed);
                        if body != BACKEND_CONF {
                            bad.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }));
        }
        std::thread::sleep(std::time::Duration::from_millis(700));
        stop.store(true, Ordering::Relaxed);
        for t in threads {
            t.join().expect("スレッドが panic しない");
        }
        let (n, bad) = (
            reads.load(Ordering::Relaxed),
            partial.load(Ordering::Relaxed),
        );
        let _ = std::fs::remove_dir_all(&dir);
        assert!(n > 100, "読み取り回数が少なすぎて検出力が無い: {n}");
        assert_eq!(bad, 0, "{n} 回中 {bad} 回、不完全な conf を観測した");
    }

    /// 残骸ソケットの判定（#625）。
    /// 「自分の pid を含まない = 残骸」だと、別 worker の `cargo test` が同時に走ったとき
    /// 相手の**生きている** tmux サーバーを kill してしまい、tmux e2e が総崩れになる
    #[test]
    fn 生きている別プロセスのテストソケットは残骸ではない() {
        // pid 4242 だけが生きている世界
        let alive = |pid: u32| pid == 4242;
        // 別プロセス（生存）のソケット = 触らない ← #625 の回帰
        assert!(!is_stale_socket("tako-coretest-scr0-4242", alive));
        // macOS の /private/tmp 解決で末尾に `=` が付いた形も同じ判定
        assert!(!is_stale_socket("tako-coretest-scr0-4242=", alive));
        // 所有プロセスが died → 残骸なので掃除してよい
        assert!(is_stale_socket("tako-coretest-scr0-999999", alive));
        assert!(is_stale_socket("tako-coretest-nestw-in-999999", alive));
        // テスト用でないソケット（本番の tako バックエンド等）は対象外
        assert!(!is_stale_socket("tako", |_| false));
        assert!(!is_stale_socket("tako-999999", |_| false));
        // 命名規約から外れて所有者を特定できないものは安全側に倒して触らない
        assert!(!is_stale_socket("tako-coretest-nopid", |_| false));
    }

    #[test]
    fn テストソケット名から所有pidを取れる() {
        assert_eq!(socket_owner_pid("tako-coretest-scr0-1234"), Some(1234));
        assert_eq!(socket_owner_pid("tako-coretest-nestw-in-77"), Some(77));
        assert_eq!(socket_owner_pid("tako-coretest-nopid"), None);
    }

    /// 自プロセスは当然「生きている」= 自分のソケットを掃除しない
    #[test]
    #[cfg(unix)]
    fn 自プロセスは生存判定される() {
        assert!(process_alive(std::process::id()));
        assert!(!is_stale_socket(
            &format!("tako-coretest-scr0-{}", std::process::id()),
            process_alive
        ));
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
        // TAKO_PANE_ID は -e フラグでセッション環境に直接注入される
        let e_pos = args.iter().position(|a| a == "-e").unwrap();
        assert_eq!(args[e_pos + 1], "TAKO_PANE_ID=3");
        // env / cwd は維持される（env はクライアントプロセスの環境にもなる）
        assert_eq!(wrapped.env.len(), 1);
        assert_eq!(wrapped.cwd.as_deref(), Some(std::path::Path::new("/tmp")));
    }

    #[test]
    fn コマンド未指定はtmuxの既定シェルに任せる() {
        let wrapped = wrap_options(SpawnOptions::default(), "tako-test", "tako-x");
        let command = wrapped.command.unwrap();
        // コマンドを渡さない（zsh -c ラッパーがシェル統合の ZDOTDIR を消費するのを
        // 避け、tmux がログインシェルを直接 spawn する経路に乗せる）。
        // #1105 でセッション名の後ろに `-e` 対が並ぶようになったので、
        // 「最後の語」ではなく「セッション名の後は `-e` 対だけ」を見る
        let s_at = command
            .args
            .iter()
            .position(|a| a == "-s")
            .expect("-s が要る");
        assert_eq!(command.args[s_at + 1], "tako-x");
        let tail = &command.args[s_at + 2..];
        assert!(
            tail.chunks(2).all(|c| c.len() == 2 && c[0] == "-e"),
            "セッション名の後に内側コマンドが足されている: {tail:?}"
        );
    }

    /// Issue #113 回帰: 「detached だが直近までアクティブ」なセッション（多重起動事故で
    /// layout.json から漏れた実行中 worker 相当）は、起動時経路（min_idle_secs 付き）では
    /// kill されず、明示操作（None = 従来挙動）では従来どおり kill される。
    /// 修正前の cleanup（猶予なし相当）ならこのセッションは消えていた
    #[test]
    #[cfg(unix)]
    fn cleanup_orphansは直近アクティブなdetachedセッションを猶予する() {
        if !crate::backend::capabilities().survives_app_exit {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let socket = format!("tako-coretest-{}-grace", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        // detached セッションを直接作る（クライアント無し = attached 0。
        // 作成直後なので session_activity は「今」= 実行中 worker の状態を再現）
        let session = "tako-grace-victim";
        let created = crate::tmux::tmux_command(Some(&socket))
            .args([
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                session,
                "sleep",
                "300",
            ])
            .output()
            .expect("tmux new-session を実行できる");
        assert!(
            created.status.success(),
            "detached セッションを作成できる: {}",
            String::from_utf8_lossy(&created.stderr)
        );
        let unprotected: std::collections::HashSet<String> = std::collections::HashSet::new();
        // 起動時経路（猶予 1 時間）: protected から漏れていても直近アクティブなら生き残る
        let killed = cleanup_orphans(&socket, &unprotected, Some(3600));
        assert!(
            killed.is_empty(),
            "猶予内のセッションは kill されない: {killed:?}"
        );
        assert!(
            crate::tmux::session_alive(Some(&socket), session),
            "セッションが生き残る"
        );
        // 明示操作（猶予なし）: 従来どおり kill される（= 修正前の起動時挙動でもある）
        let killed = cleanup_orphans(&socket, &unprotected, None);
        assert_eq!(
            killed,
            vec![session.to_string()],
            "明示 cleanup は従来どおり kill する"
        );
        assert!(
            !crate::tmux::session_alive(Some(&socket), session),
            "kill 後はセッションが消える"
        );
    }

    /// find_orphans は protected 外の tako-* セッションを返し、
    /// tako-view-* やユーザーセッションは除外する（Issue #191）
    #[test]
    #[cfg(unix)]
    fn find_orphansはprotected外のtakoセッションだけ返す() {
        if !crate::backend::capabilities().survives_app_exit {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let socket = format!("tako-coretest-{}-orphan", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        // orphan 候補（tako-*）、ビュー（tako-view-*）、ユーザーセッション（user-*）を作成
        for name in &["tako-orphan1", "tako-orphan2", "tako-view-x", "user-sess"] {
            let r = crate::tmux::tmux_command(Some(&socket))
                .args([
                    "-f",
                    "/dev/null",
                    "new-session",
                    "-d",
                    "-s",
                    name,
                    "sleep",
                    "300",
                ])
                .output()
                .expect("tmux new-session");
            assert!(r.status.success(), "セッション {name} の作成に失敗");
        }
        // orphan1 を protected に入れる
        let mut protected = std::collections::HashSet::new();
        protected.insert("tako-orphan1".to_string());
        let orphans = find_orphans(&socket, &protected);
        // orphan2 だけが返る（orphan1 は protected、tako-view-x は除外、user-sess は非 tako-*）
        assert_eq!(orphans, vec!["tako-orphan2".to_string()]);
    }

    /// session_cwd はセッションの現在の作業ディレクトリを返す（Issue #191）
    #[test]
    #[cfg(unix)]
    fn session_cwdはセッションのcwdを返す() {
        if !crate::backend::capabilities().survives_app_exit {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let socket = format!("tako-coretest-{}-cwd", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        let session = "tako-cwd-test";
        let r = crate::tmux::tmux_command(Some(&socket))
            .args([
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                session,
                "-c",
                "/tmp",
            ])
            .output()
            .expect("tmux new-session");
        assert!(r.status.success());
        let cwd = session_cwd(&socket, session);
        assert!(cwd.is_some(), "cwd が取得できる");
        let cwd = cwd.unwrap();
        // /tmp は /private/tmp のシンボリックリンクの場合がある
        assert!(cwd == "/tmp" || cwd == "/private/tmp", "cwd = {cwd}");
    }

    /// 永続化の根幹 e2e: クライアント（tako 側）を破棄してもセッションが生き、
    /// 同一コマンドで attach し直すと画面内容ごと戻る。tmux 不在環境ではスキップ
    #[test]
    #[cfg(unix)]
    fn セッションはクライアント切断後もattachで内容ごと戻る() {
        if !crate::backend::capabilities().survives_app_exit {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let socket = format!("tako-coretest-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
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
        // 画面を落とす: 失敗が「復元できなかった」のか「サーバーごと消えた
        // （= 外から kill された）」のかをログだけで切り分けられるようにする
        assert!(
            wait_for(&second, "TAKO-PERSIST-OK"),
            "再 attach で画面内容が復元される。画面: {:?}",
            second.visible_lines().join("\n")
        );
    }

    /// シェル統合の OSC 7 が tmux パススルー（allow-passthrough + スクリプトの包み直し）で
    /// tako 側の TapPty まで届くことの e2e（FR-2.4.1 × Phase 5.5 の共存検証）。
    /// zsh / tmux が無い環境ではスキップ
    #[test]
    #[cfg(unix)]
    fn osc7はtmuxパススルーで外へ届く() {
        if !crate::backend::capabilities().survives_app_exit {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        if !std::path::Path::new("/bin/zsh").exists() {
            eprintln!("skip: zsh が無い環境");
            return;
        }
        let socket = format!("tako-coretest-osc-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
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

    /// #1105 回帰: **器のサーバーが別インスタンスのシェル統合を指している**状態でも
    /// OSC 7 が届く。
    ///
    /// tmux サーバーのグローバル環境は最初のクライアントから継承されるので、同じ
    /// socket 名に前のインスタンスのサーバーが残っていると `ZDOTDIR` が前の（消えて
    /// いるかもしれない）置き場を指す。`wrap_options` が統合の置き場を `-e` で
    /// セッションへ固定しないと、統合が読み込まれず cwd 追従が黙って死ぬ。
    ///
    /// **`options.env` には統合を入れない**のが要点: production では
    /// [`crate::TerminalSession::spawn`] が**外側 PTY** の env へ足すので
    /// `wrap_options` からは見えない（既存の `osc7はtmuxパススルーで外へ届く` は
    /// テストが自分で `options.env` へ入れているため、この穴を踏まない）
    #[test]
    #[cfg(unix)]
    fn 器のサーバーが別インスタンスを指していてもosc7が届く() {
        if !crate::backend::capabilities().survives_app_exit {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        if !std::path::Path::new("/bin/zsh").exists() {
            eprintln!("skip: zsh が無い環境");
            return;
        }
        let socket = format!("tako-coretest-stale-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        // 前のインスタンス相当のサーバーを先に立てる。**tako の conf を読ませる**
        // （素通し設定は on = 原因を ZDOTDIR の継承だけに絞る）。
        // 置き場は空ディレクトリ = 統合が読み込めない値
        let stale = std::env::temp_dir().join(format!("tako-stale-zdotdir-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&stale);
        let conf = ensure_conf();
        let started = crate::tmux::tmux_command(Some(&socket))
            .args([
                "-f",
                &conf.display().to_string(),
                "new-session",
                "-d",
                "-s",
                "stale-pre",
                "sleep",
                "120",
            ])
            .env("ZDOTDIR", &stale)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !started {
            eprintln!("skip: 先行サーバーを立てられない");
            let _ = std::fs::remove_dir_all(&stale);
            return;
        }
        // production と同じ形: 統合は options.env に入れない
        let options = SpawnOptions {
            command: None,
            cwd: Some("/".into()),
            env: vec![
                ("TAKO_PANE_ID".into(), "1".into()),
                ("SHELL".into(), "/bin/zsh".into()),
            ],
        };
        let (mut session, mut rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, "tako-e2e-stale"))
                .expect("tmux クライアントを spawn できる");
        session.write(b"cd /private/tmp\r".to_vec());
        for _ in 0..100 {
            while let Ok(event) = rx.try_recv() {
                session.process_event(event);
            }
            if session.cwd() == Some(std::path::Path::new("/private/tmp")) {
                let _ = std::fs::remove_dir_all(&stale);
                return; // OSC 7 が届いた
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let screen = session.visible_lines().join("\n");
        let server_zdotdir = crate::tmux::show_environment(Some(&socket), None, "ZDOTDIR");
        let session_zdotdir =
            crate::tmux::show_environment(Some(&socket), Some("tako-e2e-stale"), "ZDOTDIR");
        let _ = std::fs::remove_dir_all(&stale);
        panic!(
            "OSC 7 が届かない（#1105）。server_zdotdir={server_zdotdir:?} \
             session_zdotdir={session_zdotdir:?} 画面: {screen:?}"
        );
    }

    /// #1105 回帰: **ソケット名が `tako` で始まらなくても** OSC 7 が届く。
    ///
    /// シェル統合は「自分が tako の器の中か」で OSC を DCS パススルーで包むかを
    /// 決める。判定材料がソケット名の接頭辞 `tako*` だったので、`TAKO_TMUX_SOCKET` に
    /// 別の名前を与えると包まずに素の OSC を出し、tmux がそれを飲んで
    /// **cwd 追従（OSC 7）とコマンド状態（OSC 133）が両方黙って死んでいた**
    /// （検証用のソケット名で踏んだ。#1105）。
    /// 器が名前を明示（`BACKEND_SOCKET_ENV`）すれば推測が要らない
    #[test]
    #[cfg(unix)]
    fn ソケット名がtakoで始まらなくてもosc7が届く() {
        if !crate::backend::capabilities().survives_app_exit {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        if !std::path::Path::new("/bin/zsh").exists() {
            eprintln!("skip: zsh が無い環境");
            return;
        }
        // **接頭辞をわざと外す**（`tako` で始まらない名前）
        let socket = format!("ct1105-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        let options = SpawnOptions {
            command: None,
            cwd: Some("/".into()),
            env: vec![
                ("TAKO_PANE_ID".into(), "1".into()),
                ("SHELL".into(), "/bin/zsh".into()),
            ],
        };
        let (mut session, mut rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, "tako-e2e-name"))
                .expect("tmux クライアントを spawn できる");
        session.write(b"cd /private/tmp\r".to_vec());
        for _ in 0..100 {
            while let Ok(event) = rx.try_recv() {
                session.process_event(event);
            }
            if session.cwd() == Some(std::path::Path::new("/private/tmp")) {
                return; // OSC 7 が届いた
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!(
            "OSC 7 が届かない（#1105。socket={socket} = tako で始まらない名前）。画面: {:?}",
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
        if !crate::backend::capabilities().survives_app_exit {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let socket = format!("tako-coretest-mouse-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
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

    /// マウスレポート洪水（トラックパッド慣性スクロール級）でも ESC 欠落断片が
    /// 内側アプリへテキストとして漏れない e2e（#167）。
    /// 転送レートが下流（tmux クライアント PTY / 内側 PTY）の処理能力を超えると
    /// macOS の tty 入力キューがバイトを黙って捨て、ESC を失った断片
    /// （例: `4;45;18M`）が平文として内側の入力欄に入る（実 claude で再現済み）。
    /// terminal.rs のホイール転送レート制限がこれを防ぐことを検証する。
    /// 内側は claude と同じ raw mode + 受信バイトの即時可視化（perl）
    #[test]
    #[cfg(unix)]
    fn マウスレポート洪水でも断片がテキスト化しない() {
        if !crate::backend::capabilities().survives_app_exit {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let socket = format!("tako-coretest-flood-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        // 内側アプリ: SGR マウス要求 + raw mode + ESC を「^[」に可視化して即時 echo
        // （claude 等の raw mode TUI が受け取るバイト列の観測装置）
        let inner = r#"stty raw -echo; printf '\033[?1000h\033[?1006h'; exec perl -e '$|=1; while (sysread(STDIN,$b,4096)) { $b =~ s/\x1b/^[/g; syswrite(STDOUT,$b) }'"#;
        let options = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), inner.into()],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
        };
        let session_name = "tako-e2e-flood";
        let (mut session, mut rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, session_name))
                .expect("tmux クライアントを spawn できる");

        // 内側のマウス要求が外側端末モードへ伝わるのを待つ。
        // rx（PtyWrite = tmux の端末クエリへの応答）を実運用の UI 層と同様に処理する
        // （捨てると tmux クライアントが応答待ちのままになり、経路の再現にならない）
        let mut mouse_on = false;
        for _ in 0..100 {
            while let Ok(event) = rx.try_recv() {
                session.process_event(event);
            }
            if session.mouse_reporting() {
                mouse_on = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(mouse_on, "内側アプリのマウス要求が外側端末モードへ伝わる");

        // 洪水: 慣性スクロール相当（2,100 イベント要求）を全速で連打
        for _ in 0..700 {
            session.scroll_wheel(3, 5, 5);
            while let Ok(event) = rx.try_recv() {
                session.process_event(event);
            }
        }

        // 配送が安定するまで待つ（capture の intact 数が変化しなくなるまで）
        let capture = || -> String {
            crate::tmux::tmux_command(Some(&socket))
                .args(["capture-pane", "-t", session_name, "-p", "-S", "-", "-J"])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default()
        };
        const INTACT: &str = "^[[<64;6;6M";
        let mut all = String::new();
        let mut last_count = usize::MAX;
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            while let Ok(event) = rx.try_recv() {
                session.process_event(event);
            }
            all = capture();
            let count = all.matches(INTACT).count();
            if count > 0 && count == last_count {
                break;
            }
            last_count = count;
        }

        let intact_count = all.matches(INTACT).count();
        // intact な SGR レポートを全部取り除いた残りに座標断片が残っていたら、
        // ESC 欠落断片がテキストとして内側へ届いている（= #167 の症状）
        let stripped = all.replace(INTACT, "");
        assert!(
            !stripped.contains("6;6M"),
            "ESC 欠落断片が内側へテキストとして漏れている（#167 再発）。\
             intact={intact_count} 残骸例: {:?}",
            stripped
                .lines()
                .filter(|l| l.contains("6;6M"))
                .take(3)
                .collect::<Vec<_>>()
        );
        // レート制限が全イベントを殺していないこと（正常なレポートは届く）
        assert!(
            intact_count > 0,
            "SGR レポートが 1 件も届いていない（転送が死んでいる）。画面: {:?}",
            session.visible_lines().join("\n")
        );
        // レート制限が生きていること（2,100 イベントの洪水がそのまま流れていない。
        // 制限が消えると飛行中バイト量が増え、書き込み停滞時の断片化リスクが戻る）
        assert!(
            intact_count < 200,
            "洪水がレート制限されずそのまま転送されている（#167 の防御が消失）: {intact_count}"
        );
        eprintln!("洪水 2100 イベント要求 → intact 配送 {intact_count} 件・断片ゼロ");
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
        if !crate::backend::capabilities().survives_app_exit {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let socket = format!("tako-coretest-esc-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
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
        if !crate::backend::capabilities().survives_app_exit {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let backend = format!("tako-coretest-nestw-{}", std::process::id());
        let nested = format!("tako-coretest-nestw-in-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![backend.clone(), nested.clone()]);
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
        if !crate::backend::capabilities().survives_app_exit {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let backend = format!("tako-coretest-nestk-{}", std::process::id());
        let nested = format!("tako-coretest-nestk-in-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![backend.clone(), nested.clone()]);
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
        if !crate::backend::capabilities().survives_app_exit {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let socket = format!("tako-coretest-cjk-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
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
        if !crate::backend::capabilities().survives_app_exit {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let socket = format!("tako-coretest-ind-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
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
        if !crate::backend::capabilities().survives_app_exit {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let socket = format!("tako-coretest-sync-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
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
        if !crate::backend::capabilities().survives_app_exit {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let socket = format!("tako-coretest-alt-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
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
        // 前提（alt screen ペインであること）の成立を待つ。これが無いと通常画面のまま
        // ホイールを送って「矢印に化けない」を見てしまい、検出力が落ちる（#625）
        assert_eq!(
            wait_alt_screen(&socket, "tako-e2e-alt"),
            Some(true),
            "alt-screen 切替が完了しない。画面: {:?}",
            session.visible_lines().join("\n")
        );
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
