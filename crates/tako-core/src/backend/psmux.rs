//! PsmuxBackend — psmux を器とするバックエンド（Windows の永続化。#519 M2 / 設計 §2.2 の案 B-1）
//!
//! [psmux](https://github.com/psmux/psmux)（MIT / Rust 製 / tmux 互換 CLI）を
//! 「生存の器」として使う。設計 §2.2 が後継として温存していた **B-1（器あり・到達なし）**
//! から出発し、#519 で**採取（読み）だけ**を足した形になっている:
//! `survives_app_exit = true` / `detached_capture = true` / `detached_access = false` /
//! `scrollback = InProcess`。tako を強制終了しても psmux サーバー・シェル・
//! scrollback が生き残ることは実測済み（適合検証 2026-07-27。M0 の罠 7 項目のうち 6 つを
//! psmux は既に越えている）。採取の実装と実測は [`PsmuxCapture`] を参照。
//!
//! ## なぜ [`super::TmuxBackend`] を流用できないのか
//!
//! psmux は tmux 互換を名乗るが、**コマンドごとに互換の深さが違う**。とりわけ
//! tako が全経路で使っている `=`（exact-match 接頭辞）付きターゲットの扱いが揃っておらず、
//! `kill-session -t =name` は **3/3 決定的に失敗する（各 5.1 秒ブロック）**。
//! そのまま流用すると、ペインを閉じるたびに器と pwsh がリークし、close が 5 秒固まる。
//!
//! 実測で分かっている非互換と、この実装での対処:
//!
//! | psmux の挙動 | ここでの対処 |
//! |---|---|
//! | `-t =name` の解釈がコマンドごとにバラバラ（kill-session は失敗） | **`=` を一切使わない**（[`Self::target`]） |
//! | `show-environment -t s NAME` が名前指定を無視して全変数を出す | 出力から `NAME=` 行を選ぶ（[`select_env_value`]） |
//! | `#{history_bytes}` が空 | 依存しない（[`parse_history_probe`] が 0 へ倒す。変化の検知にしか使わせない） |
//! | `pane_tty` が実在しない `/dev/pty1` を返す | [`SessionBackend::pane_tty`] は `None` を返す |
//! | `list-clients -F` が書式指定を無視（クライアント PID が取れない） | [`super::owner`] のオーナー記録で代替（#177 の強奪ガード） |
//! | `new-session -D` が他クライアントを切り離さない | 同上。**器に収束を期待しない** |
//! | 既定の warm pane 先読みが 1 セッションあたり pwsh を 1 本余計に常駐させる | conf に `set -g warm off`（+243MB → +131MB/session） |
//! | 器の中のシェルは tako の ConPTY の子ではない（コードページ固定が届かない） | [`SessionBackend::pane_pids`] で器へ `#{pane_pid}` を尋ね、器の内側にも B19 の固定を当てる（#659） |
//! | ホイールで copy mode に入り、滞在中の打鍵をシェルへ渡さない | `#{pane_in_mode}` で確かめてから、解除キー `q` を打鍵と同じ PTY へ前置する（#686） |
//! | `bind-key -T copy-mode …` が無反応（キーテーブル未実装。`list-keys -T copy-mode` が空） | conf での解決を諦め、上記の in-band 解除を採る |
//!
//! ## バージョン固定と起動時プローブ
//!
//! psmux は直近 30 日で 100+ コミットという速度で動いており、上表の挙動は変わりうる。
//! [`VERIFIED_VERSION`] 以外を見つけたら **器として使えるかを実際に試してから**採用する
//! （[`behavior_probe`]）。試して駄目なら器を配らない = 明示的な縮退へ倒す。
//! 「動くつもりで半端に壊れた永続化」が最悪なので、黙って使うことはしない。

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use super::owner::{Claim, OwnerRecords};
use super::{
    BackendCapabilities, BackendError, DetachedCapture, HistoryProbe, Holder, HolderKind,
    ScrollProbe, ScrollbackAuthority, SessionBackend, SessionInfo, SessionRef, SESSION_PREFIX,
};
use crate::paths::data_dir;
use crate::terminal::{SpawnCommand, SpawnOptions};

/// 適合検証を通したバージョン（`psmux -V` の 2 行目。2026-07-27 実測）。
/// これ以外は [`behavior_probe`] で器として使えるかを確かめてから採用する
pub const VERIFIED_VERSION: &str = "3.3.7";

/// psmux バックエンドの設定。**psmux が知らないオプションを 1 つでも書くと
/// `psmux: N config warning(s):` がペインへ出る**（実測）。表示の汚染はそのまま
/// ユーザーの目に入るので、ここには実測で受理を確認したものだけを置く。
///
/// tmux 版（`tmux_backend::BACKEND_CONF`）から落としたもの:
/// `extended-keys` / `extended-keys-format` / `copy-mode-position-format` /
/// `default-terminal` / `terminal-features`（いずれも psmux が知らない or 未確認）。
/// `escape-time` も外してある（psmux は ConPTY 直結で tmux の ESC 曖昧性回避が要らない）
const BACKEND_CONF: &str = "\
# tako psmux バックエンド設定（自動生成。手で編集しない。tako-core::backend::psmux）
set -g status off
set -g prefix None
set -g mouse on
set -g history-limit 10000
set -g warm off
set -g allow-passthrough on
set -g focus-events on
set -g set-clipboard on
set -g update-environment 'TAKO_SOCKET TAKO_TOKEN TAKO_MCP_URL'
";

pub struct PsmuxBackend {
    /// psmux 実行ファイル（[`super::binary`] が解決したもの）
    bin: String,
    /// 専用サーバーのソケット名（`-L`）。tmux 版と同じ解決を使う
    /// （`TAKO_TMUX_SOCKET` = `TAKO_ISOLATED` の一括隔離が psmux にもそのまま効く）
    socket: String,
    /// 実行ファイルが名乗ったバージョン（診断用）
    version: String,
    /// セッション単位のオーナー記録（#177 の強奪ガードの材料）
    owners: OwnerRecords,
    /// 役割 B の読み側（採取）。**器の一部としてここに持つ**ので、
    /// `detached_capture()` が借用を返せる
    capture: PsmuxCapture,
}

impl PsmuxBackend {
    pub fn new(bin: String, version: String) -> Self {
        Self::with_parts(
            bin,
            version,
            crate::tmux_backend::socket_name(),
            owner_dir(),
        )
    }

    /// ソケットとオーナー記録の置き場を明示しての構築。
    /// **隔離検証・統合テスト用**（プロセス全体の環境変数を触らずに器を隔離できる）
    pub fn with_parts(bin: String, version: String, socket: String, owner_dir: PathBuf) -> Self {
        Self {
            capture: PsmuxCapture {
                bin: bin.clone(),
                socket: socket.clone(),
            },
            bin,
            socket,
            version,
            owners: OwnerRecords::new(owner_dir),
        }
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn socket(&self) -> &str {
        &self.socket
    }

    /// **ターゲット指定に `=` を付けない**のがこの実装の中心的な約束（要件 1）。
    ///
    /// tmux では `=name` が exact-match を意味し、tako は取り違え防止のため全経路で
    /// 付けている。psmux はこれをコマンドごとに違う解釈で扱い、`kill-session` では
    /// 決定論的に失敗する（5.1 秒ブロック後にエラー）。
    ///
    /// **`=` を外しても取り違えは起きない**: tako が払い出す名前は
    /// `tako-<12 桁 16 進>` の固定長で、前方一致するのは同名だけである。
    /// 加えて [`Self::kill`] は接頭辞ガードを持つ
    fn target<'a>(&self, session: &'a SessionRef) -> &'a str {
        session.as_str()
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.bin);
        crate::platform::process::no_console_window(&mut command);
        command.args(["-L", self.socket.as_str()]);
        command
    }

    /// psmux CLI 実行。サーバー未起動（`no server running`）はエラー文字列で返す
    fn run(&self, args: &[&str]) -> Result<String, String> {
        run_with(&self.bin, Some(&self.socket), args)
    }
}

/// オーナー記録の置き場（`<data_dir>/backend-owners`）。
/// data_dir が無い環境では一時ディレクトリへ逃がす（記録できないより良い）
fn owner_dir() -> PathBuf {
    data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("backend-owners")
}

/// バイナリとソケットを明示しての psmux CLI 実行（[`behavior_probe`] と共有）
fn run_with(bin: &str, socket: Option<&str>, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new(bin);
    crate::platform::process::no_console_window(&mut command);
    if let Some(socket) = socket {
        command.args(["-L", socket]);
    }
    let output = command
        .args(args)
        .output()
        .map_err(|e| format!("psmux を実行できない: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// 専用 conf をデータディレクトリへ書き出す（毎起動上書き = バージョン更新追従）。
/// 書けない環境では `None` を返し、`-f` を付けずに起動する（ユーザー conf を読む
/// 可能性は残るが、器が作れないよりはよい）。
///
/// `pub` なのは統合テストが「この conf を psmux が警告なしで受理するか」を
/// 実バイナリで確かめるため（未知のオプションが 1 行でもあるとペインに警告が出る）
pub fn ensure_conf() -> Option<PathBuf> {
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("psmux-backend.conf");
    std::fs::write(&path, BACKEND_CONF).ok()?;
    Some(path)
}

impl SessionBackend for PsmuxBackend {
    /// **器あり・採取あり・送出なし**（設計 §2.2 の案 B-1 に採取を足した形。#519）。
    ///
    /// - `detached_capture = true`: `capture-pane` / `display-message` は実測で動く
    ///   （UTF-8・LF・`-S` のクランプまで tmux と同じ。[`PsmuxCapture`] 参照）
    /// - `detached_access = false`: `paste-buffer` が改行を literal `\n` に化けさせ、
    ///   `send-keys -H` が 16 進引数をリテラル文字として送る。送達経路として信頼できない
    /// - `scrollback = InProcess`: **履歴の所在ではなく UI の描き方**の申告。
    ///   ミラー経路は `#{mouse_any_flag}` と `send-keys -H` を要求するが psmux は
    ///   どちらも持たない（#654）。外側 alacritty を使う直接ペイン経路が正しい
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            survives_app_exit: true,
            detached_capture: true,
            detached_access: false,
            scrollback: ScrollbackAuthority::InProcess,
            // psmux 3.3.7 は allow-passthrough 相当のオプションを受理するが**素通ししない**
            // （実測: 素の OSC / DCS の ESC 二重化あり / 二重化なし のいずれも外へ出ず、
            // 同時に流した平文だけが届いた）。このため psmux ペインでは
            // cwd 追従とコマンド状態が働かない（#525）
            osc_passthrough: false,
            label: "psmux",
        }
    }

    fn reserve(&self, candidate: &str) -> Option<SessionRef> {
        SessionRef::new(candidate).ok()
    }

    /// `psmux -u -L <socket> -f <conf> new-session -A -D -s <name> [-e K=V]… [-c cwd] [command]`
    ///
    /// 形は tmux 版（`tmux_backend::wrap_options`）と同じで、実測で全項目の反映を確認済み
    /// （`-e` 2 本・`-c`・conf の `history-limit`）。**`-D` は psmux では現状効かない**が、
    /// psmux が実装したときに tmux と同じ収束挙動へ戻るよう残す。効かない前提の安全策は
    /// オーナー記録（下）であって、このフラグではない
    fn wrap_spawn(&self, options: SpawnOptions, session: &SessionRef) -> SpawnOptions {
        // 器を握るのはこの瞬間（= 実際に attach する時点）。#177 の強奪ガードの材料になる
        if let Claim::Foreign(pid) = self.owners.claim(session) {
            eprintln!(
                "warning: 器 {session} は別インスタンス（pid {pid}）が使用中のまま attach する\
                 （psmux は -D で切り離せないため、両方のクライアントが残る）"
            );
        }
        let mut args = vec!["-u".to_string(), "-L".to_string(), self.socket.clone()];
        if let Some(conf) = ensure_conf() {
            args.push("-f".to_string());
            args.push(conf.display().to_string());
        }
        args.extend([
            "new-session".to_string(),
            "-A".to_string(),
            "-D".to_string(),
            "-s".to_string(),
            session.as_str().to_string(),
        ]);
        // ペイン固有の環境変数はセッション作成時に確定させる（tmux 版と同じ理由。
        // サーバーのグローバル環境から stale な値を拾わせない）
        for (key, val) in &options.env {
            if key == "TAKO_PANE_ID" || key == "TAKO_TAB_ID" {
                args.push("-e".to_string());
                args.push(format!("{key}={val}"));
            }
        }
        if let Some(cwd) = &options.cwd {
            args.push("-c".to_string());
            args.push(cwd.display().to_string());
        }
        // 明示コマンドは psmux の引数パーサに合わせて 1 引数へ畳む。
        // 未指定なら渡さない（psmux の default-shell = pwsh がそのまま立つ）
        if let Some(inner) = &options.command {
            args.push(inner_command(inner));
        }
        SpawnOptions {
            command: Some(SpawnCommand {
                program: self.bin.clone(),
                args,
            }),
            ..options
        }
    }

    fn exists(&self, session: &SessionRef) -> bool {
        self.run(&["has-session", "-t", self.target(session)])
            .is_ok()
    }

    /// 器を破棄する。**接頭辞ガード付き**: `=` を使わない以上、万一おかしな名前が
    /// 渡ったときに psmux 側の緩いターゲット解決へ丸投げしない
    fn kill(&self, session: &SessionRef) -> Result<(), BackendError> {
        if !session.as_str().starts_with(SESSION_PREFIX) {
            return Err(BackendError::InvalidSession {
                name: session.as_str().to_string(),
                reason: "tako が作った器ではない（接頭辞が一致しない）",
            });
        }
        // 既に無い場合も psmux は exit 0 を返す（実測）。無害なので区別しない
        let result = self.run(&["kill-session", "-t", self.target(session)]);
        self.owners.release(session);
        result.map(|_| ()).map_err(BackendError::Operation)
    }

    fn list(&self) -> Vec<SessionInfo> {
        let out = self
            .run(&[
                "list-sessions",
                "-F",
                "#{session_name}\t#{session_attached}\t#{session_activity}",
            ])
            .unwrap_or_default();
        parse_sessions(&out)
    }

    /// **器に尋ねずオーナー記録で答える**（psmux の `list-clients -F` は書式指定を
    /// 無視してクライアント PID を返さないため）。返す PID は所有インスタンス
    /// （tako-app）そのもので、**生存はロックで確認済み**なので
    /// [`HolderKind::Owner`] を付ける
    fn foreign_holders(&self, sessions: &[SessionRef]) -> Vec<Holder> {
        self.owners
            .foreign_holders(sessions)
            .into_iter()
            .map(|(session, pid)| Holder {
                pid,
                session,
                kind: HolderKind::Owner,
            })
            .collect()
    }

    fn orphans(&self, protected: &HashSet<SessionRef>) -> Vec<SessionRef> {
        self.list()
            .into_iter()
            .filter(|info| self.is_orphan_candidate(info, protected))
            .map(|info| info.session)
            .collect()
    }

    fn cleanup_orphans(
        &self,
        protected: &HashSet<SessionRef>,
        min_idle: Option<Duration>,
    ) -> Vec<SessionRef> {
        let now = crate::backend::now_unix();
        let mut killed = Vec::new();
        for info in self.list() {
            if !self.is_orphan_candidate(&info, protected) {
                continue;
            }
            if info.attached {
                continue; // 使用中
            }
            if let Some(min_idle) = min_idle {
                // 直近までアクティブなら実行中プロセスの可能性が高い（#113 の猶予）
                if now.saturating_sub(info.last_activity.max(0) as u64) < min_idle.as_secs() {
                    continue;
                }
            }
            if self.kill(&info.session).is_ok() {
                killed.push(info.session);
            }
        }
        killed
    }

    /// psmux は実在しない Unix 風 tty（`/dev/pty1`）を返すため **`None` を返す**（要件 4）。
    /// Windows の listen ポート検知（FR-2.4.2）は tty の突き合わせではなく
    /// プロセス情報（境界 B5）で行う
    fn pane_tty(&self, _session: &SessionRef) -> Option<String> {
        None
    }

    /// `list-panes -F "#{pane_pid}"`（実測で正しい pid を返す。`pane_tty` と違って
    /// psmux も嘘をつかない）。**器の中のシェルの疑似コンソールを UTF-8 へ固定する**
    /// ために使う（#659）。セッションがまだ無い間はエラーになるので空を返す。
    ///
    /// 返すのは器がそのセッションに持つ**全ペイン**の pid。tako は 1 セッション =
    /// 1 ペインで使うが、ユーザーが器の中で分割した分もここに載る。コードページは
    /// 疑似コンソール単位なので、載った分だけ固定しておくのが素直（余分に固定しても害は無い）
    fn pane_pids(&self, session: &SessionRef) -> Vec<u32> {
        let out = self
            .run(&[
                "list-panes",
                "-t",
                self.target(session),
                "-F",
                "#{pane_pid}",
            ])
            .unwrap_or_default();
        parse_pane_pids(&out)
    }

    /// psmux は `#{pane_in_mode}` に答える（実測。`#{mouse_any_flag}` 等の
    /// 未対応変数と違って空文字にならない）。#686
    fn pane_in_mode(&self, session: &SessionRef) -> Option<bool> {
        let out = self
            .run(&[
                "display-message",
                "-p",
                "-t",
                &format!("{}:", self.target(session)),
                "#{pane_in_mode}",
            ])
            .ok()?;
        parse_in_mode(&out)
    }

    /// psmux の copy mode は `q` で抜ける（実測: in-band の `q` で `pane_in_mode`
    /// が 1 → 0。キーは copy mode に消費されシェルへは漏れない）。
    ///
    /// **`bind-key -T copy-mode Any send-keys -X cancel` は使えない**: psmux は
    /// copy-mode のキーテーブルを実装しておらず（`list-keys -T copy-mode` が空）、
    /// bind は成功を返すが何も起きない（実測）
    fn copy_mode_exit_bytes(&self) -> Option<&'static [u8]> {
        Some(COPY_MODE_EXIT)
    }

    fn session_cwd(&self, session: &SessionRef) -> Option<String> {
        let out = self
            .run(&[
                "list-panes",
                "-t",
                self.target(session),
                "-F",
                "#{pane_current_path}",
            ])
            .ok()?;
        let path = out.lines().next().unwrap_or("").trim().to_string();
        (!path.is_empty()).then_some(path)
    }

    /// psmux の `show-environment -t <session> <name>` は**名前指定を無視して
    /// 全変数を出す**（実測）。全行から `<name>=` 行を選ぶ（要件 2）
    fn session_env(&self, session: &SessionRef, name: &str) -> Option<String> {
        let out = self
            .run(&["show-environment", "-t", self.target(session), name])
            .ok()?;
        select_env_value(&out, name)
    }

    fn set_session_env(&self, session: &SessionRef, name: &str, value: &str) {
        let _ = self.run(&["set-environment", "-t", self.target(session), name, value]);
    }

    fn sync_config(&self) {
        let Some(conf) = ensure_conf() else { return };
        let _ = self
            .command()
            .arg("source-file")
            .arg(&conf)
            .output()
            .map(|_| ());
    }

    /// **採取だけ**を持つ（#519）。psmux の `capture-pane` / `display-message` は
    /// 実測で tmux 同等に動くので、ここを `None` のままにしておく理由が無い
    /// （report / read フォールバック / worker 監視が Windows で全滅していた）
    fn detached_capture(&self) -> Option<&dyn DetachedCapture> {
        Some(&self.capture)
    }

    /// **送出は持たない**。psmux 側に send-keys / paste-buffer は在るが、
    /// `paste-buffer` が改行を literal `\n` に化けさせ、`send-keys -H` が 16 進引数を
    /// リテラル文字として送る（#654 実測）。到達手段として信頼できないものを
    /// 「使える」と型で申告すると失敗が握り潰されるので、`None` のままにする。
    /// **送出の正は in-process の PTY**（#662 で respond をそちらへ寄せたのと同じ方針）
    fn detached(&self) -> Option<&dyn super::DetachedAccess> {
        None
    }
}

/// 役割 B の読み側（採取）の psmux 実装（#519）。
///
/// **読み取りしかしない**のが約束。psmux CLI へ投げるのは `capture-pane` /
/// `display-message` / `list-panes` の 3 つだけで、器の状態を変えるコマンド
/// （`copy-mode` / `send-keys` / `resize-window`）は 1 つも呼ばない。
/// 器へ書く経路を増やすと #212 / #168 で UI スレッドから排除した種類の
/// 同期サブプロセスが戻ってくるため、そこは in-process の PTY に任せる。
///
/// 実測（psmux 3.3.7 / 隔離ソケット / 2026-07-31）で分かった tmux との差:
///
/// | | psmux | ここでの扱い |
/// |---|---|---|
/// | `-J`（折返し結合） | **無視される**（エラーにはならない） | 付けたまま。結合されない前提で使う |
/// | `#{history_bytes}` | **空** | `bytes = 0`。呼び出し側は変化の検知にしか使わない |
/// | 複合コマンド（`;` 区切り） | **後半が黙って捨てられる** | **コマンドを分けて 2 回叩く** |
/// | `-t =name`（exact-match） | コマンドごとに解釈が違う | [`PsmuxBackend::target`] と同じく **`=` を使わない** |
/// | 出力 | UTF-8 / LF のみ | `from_utf8_lossy` + `lines()` で足りる |
pub struct PsmuxCapture {
    bin: String,
    socket: String,
}

impl PsmuxCapture {
    fn run(&self, args: &[&str]) -> Result<String, String> {
        run_with(&self.bin, Some(&self.socket), args)
    }

    /// `display-message -p` で 1 行のフォーマットを引く（読み取り専用）
    fn format(&self, session: &SessionRef, format: &str) -> Option<String> {
        self.run(&["display-message", "-p", "-t", session.as_str(), format])
            .ok()
    }
}

impl DetachedCapture for PsmuxCapture {
    fn capture_screen(&self, session: &SessionRef) -> Result<Vec<String>, BackendError> {
        self.run(&["capture-pane", "-t", session.as_str(), "-p"])
            .map(|out| out.lines().map(str::to_string).collect())
            .map_err(BackendError::Operation)
    }

    fn capture_history(&self, session: &SessionRef, lines: usize) -> Option<Vec<String>> {
        if lines == 0 {
            return Some(Vec::new());
        }
        let start = format!("-{lines}");
        let out = self
            .run(&[
                "capture-pane",
                "-p",
                "-t",
                session.as_str(),
                "-S",
                &start,
                "-E",
                "-1",
            ])
            .ok()?;
        Some(out.lines().map(|l| l.trim_end().to_string()).collect())
    }

    /// `-J` は psmux に無視されるので**折返しは行のまま残る**（中身は落ちない）。
    /// それでも付けておくのは、psmux が実装したときに tmux と同じ出力へ揃うため
    fn capture_history_joined(&self, session: &SessionRef, lines: usize) -> Option<String> {
        let start = format!("-{lines}");
        let out = self
            .run(&[
                "capture-pane",
                "-p",
                "-J",
                "-t",
                session.as_str(),
                "-S",
                &start,
            ])
            .ok()?;
        // 行単位で組み直してから末尾を落とす（CRLF 正規化。tmux 実装と同一）
        let text = out.lines().collect::<Vec<_>>().join("\n");
        let text = text.trim_end();
        (!text.is_empty()).then(|| text.to_string())
    }

    /// `#{history_bytes}` は psmux では空なので **0 を返す**。
    /// これは「バイト数 0」ではなく「観測できない」の意味で、
    /// 履歴飽和時のバイト差分検知（pane_log）はこの器では働かない
    fn history_probe(&self, session: &SessionRef) -> Option<HistoryProbe> {
        let out = self.format(
            session,
            "#{history_size}\t#{history_limit}\t#{history_bytes}\t#{alternate_on}",
        )?;
        parse_history_probe(&out)
    }

    fn history_probe_batch(&self) -> Vec<(SessionRef, HistoryProbe)> {
        let out = self
            .run(&[
                "list-panes",
                "-a",
                "-F",
                "#{session_name}\t#{history_size}\t#{history_limit}\t#{history_bytes}\t#{alternate_on}",
            ])
            .unwrap_or_default();
        parse_history_probe_batch(&out)
    }

    fn scroll_probe(&self, session: &SessionRef) -> Option<ScrollProbe> {
        let out = self.format(
            session,
            "#{scroll_position}\t#{history_size}\t#{pane_in_mode}\t#{alternate_on}",
        )?;
        super::parse_scroll_probe(&out)
    }
}

/// `#{history_size}\t#{history_limit}\t#{history_bytes}\t#{alternate_on}` のパース（純関数）。
/// **空フィールドで `None` を返さない**（psmux の `#{history_bytes}` は常に空）。
/// 器の応答として壊れているのは `history_size` すら読めない場合だけ
fn parse_history_probe(out: &str) -> Option<HistoryProbe> {
    let mut f = out.lines().next()?.split('\t');
    let history = f.next()?.trim().parse().ok()?;
    let limit = f.next().unwrap_or("").trim().parse().unwrap_or(0);
    let bytes = f.next().unwrap_or("").trim().parse().unwrap_or(0);
    let alternate = f.next().unwrap_or("").trim() == "1";
    Some(HistoryProbe {
        history,
        limit,
        bytes,
        alternate,
    })
}

/// `list-panes -a -F` の 5 列出力のパース（純関数）
fn parse_history_probe_batch(out: &str) -> Vec<(SessionRef, HistoryProbe)> {
    out.lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once('\t')?;
            Some((
                SessionRef::new(name.trim()).ok()?,
                parse_history_probe(rest)?,
            ))
        })
        .collect()
}

impl PsmuxBackend {
    /// 残骸候補か（kill 判定の共通部分）。
    /// **生きた別インスタンスが所有している器は候補にしない** — psmux は
    /// `session_attached` を頼りにできない（`-D` が効かず複数クライアントが残る）ため、
    /// tako 側のオーナー記録を二重の安全網にする
    fn is_orphan_candidate(&self, info: &SessionInfo, protected: &HashSet<SessionRef>) -> bool {
        let name = info.session.as_str();
        if !name.starts_with(SESSION_PREFIX) {
            return false;
        }
        if name.starts_with("tako-view-") {
            return false;
        }
        if protected.contains(&info.session) {
            return false;
        }
        let me = std::process::id();
        !matches!(self.owners.holder(&info.session), Some(pid) if pid != me)
    }
}

/// copy mode を抜けるキー（psmux 既定。実測で確定）
const COPY_MODE_EXIT: &[u8] = b"q";

/// `list-panes -F "#{pane_pid}"` の出力をパースする（純関数。#659）。
/// 数字以外の行（psmux が警告を混ぜた場合など）は捨てる
fn parse_pane_pids(out: &str) -> Vec<u32> {
    out.lines()
        .filter_map(|line| line.trim().parse::<u32>().ok())
        .filter(|pid| *pid != 0)
        .collect()
}

/// `display-message -p "#{pane_in_mode}"` の出力をパースする（純関数。#686）。
///
/// **未対応変数は空文字に展開される**（psmux の `#{mouse_any_flag}` で実測）ので、
/// 「0 でも 1 でもない」を `None` へ倒す。ここを `Some(false)` に倒すと、
/// 器が答えられなくなった日に「copy mode ではない」と誤って断定してしまう
fn parse_in_mode(out: &str) -> Option<bool> {
    match out.trim() {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

/// `list-sessions -F` の 3 列出力をパースする（純関数）
fn parse_sessions(out: &str) -> Vec<SessionInfo> {
    out.lines()
        .filter_map(|line| {
            let mut f = line.split('\t');
            let session = SessionRef::new(f.next()?.trim()).ok()?;
            let attached = f.next()?.trim() != "0";
            let last_activity = f.next().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
            Some(SessionInfo {
                session,
                attached,
                last_activity,
            })
        })
        .collect()
}

/// `show-environment` の出力から 1 変数を取り出す（純関数。要件 2）。
///
/// psmux は名前指定を無視して全変数を返す:
/// ```text
/// TAKO_TAB_ID=3
/// TAKO_PANE_ID=7
/// PSMUX_TARGET_SESSION=tako__tako-abc
/// ```
/// tmux 互換の「1 行だけ返る」出力でも同じ実装で通る。
/// `-NAME`（unset マーカー）は値なしとして扱う
fn select_env_value(out: &str, name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    out.lines()
        .find_map(|line| line.trim_end().strip_prefix(&prefix))
        .map(str::to_string)
}

/// psmux の `new-session` へ渡す内側コマンド文字列（純関数）。
///
/// 実測（2026-07-27）で確定した psmux の引数解釈:
///
/// - 引数は POSIX 風に単語分割され、**単一引用符の中はバックスラッシュも素通し**
///   （`cmd.exe /c 'echo C:\Foo\Bar'` → `C:\Foo\Bar` がそのまま届く）
/// - ところが**第 1 語（プログラム）だけは引用符が剥がれない**。
///   `'C:\Windows\System32\cmd.exe'` は起動に失敗し、引用符なしの
///   `C:\Windows\System32\cmd.exe` は成功する
/// - 従って **空白を含むプログラムパスは第 1 語として表現できない**。
///   その場合だけ `cmd.exe /c '<Windows 形式のコマンドライン>'` に包む
///
/// **注意（2026-08-21 に測り直した結果。#881）**: この `cmd.exe /c` の包みは
/// psmux 3.3.7 では**効かない**（器の中で即死する）。二重引用符版・`call` 版も同じ。
/// 生きるのは「1 語で書ける形」だけ（`pwsh.exe …` / 8.3 短縮名 `C:\PROGRA~1\…`）。
/// 実行ペイン（#875）は最初から 1 語で書ける形を渡して回避しているが、
/// 空白入りのプログラムパスを明示指定する他の経路は #881 が直るまで動かない
fn inner_command(command: &SpawnCommand) -> String {
    if bare_program_ok(&command.program) {
        let mut out = command.program.clone();
        for arg in &command.args {
            out.push(' ');
            out.push_str(&crate::shell::quote_for_shell(arg));
        }
        return out;
    }
    let line = windows_command_line(command);
    format!("cmd.exe /c {}", crate::shell::quote_for_shell(&line))
}

/// プログラムを引用符なしの第 1 語として書けるか（空白・引用符を含まない）
fn bare_program_ok(program: &str) -> bool {
    !program.is_empty()
        && !program
            .chars()
            .any(|c| c.is_whitespace() || c == '\'' || c == '"')
}

/// Windows 形式のコマンドライン（空白を含む語だけ `"` で囲む）
fn windows_command_line(command: &SpawnCommand) -> String {
    std::iter::once(&command.program)
        .chain(command.args.iter())
        .map(|word| {
            if word.is_empty() || word.chars().any(char::is_whitespace) {
                format!("\"{word}\"")
            } else {
                word.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// バージョンの扱い（要件 6）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VersionSupport {
    /// 適合検証を通したバージョン
    Verified,
    /// 未検証。器として使えるかを [`behavior_probe`] で確かめてから採用する
    Untested,
}

pub fn version_support(version: &str) -> VersionSupport {
    if version.trim() == VERIFIED_VERSION {
        VersionSupport::Verified
    } else {
        VersionSupport::Untested
    }
}

/// 未検証バージョンの psmux が **器として最低限使えるか**を実際に試す（要件 6）。
///
/// バージョン番号の一致だけを条件にすると、psmux が patch を上げた翌日に
/// 全ユーザーの永続化が黙って落ちる。逆に無条件で信じると
/// 「器は作れるが kill が効かない」半端な永続化になる。だから**測る**。
///
/// 検査するのは器の最小契約 3 つだけ:
/// 1. 使い捨てソケット上にセッションを作れる（`new-session -d`）
/// 2. 作ったセッションを見つけられる（`has-session`。**`=` なし**）
/// 3. 作ったセッションを壊せる（`kill-session`。**`=` なし**。ここが tmux 流用の死因）
///
/// 後始末は `kill-server -L <使い捨てソケット>` で行う。**`-L` を落とすと
/// 全ソケットのサーバーが死ぬ**（psmux 実測）ので絶対に省略しない
pub fn behavior_probe(bin: &str) -> Result<(), String> {
    let socket = format!("tako-probe-{}", std::process::id());
    let session = format!("{SESSION_PREFIX}probe{:x}", std::process::id());
    let result = (|| -> Result<(), String> {
        run_with(bin, Some(&socket), &["new-session", "-d", "-s", &session])
            .map_err(|e| format!("器を作れない: {e}"))?;
        run_with(bin, Some(&socket), &["has-session", "-t", &session])
            .map_err(|e| format!("作った器を見つけられない: {e}"))?;
        run_with(bin, Some(&socket), &["kill-session", "-t", &session])
            .map_err(|e| format!("器を壊せない: {e}"))?;
        if run_with(bin, Some(&socket), &["has-session", "-t", &session]).is_ok() {
            return Err("kill-session の後も器が残っている".to_string());
        }
        Ok(())
    })();
    // 使い捨てサーバーの後始末（**-L 必須**）
    let _ = run_with(bin, Some(&socket), &["kill-server"]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend() -> PsmuxBackend {
        PsmuxBackend::with_parts(
            "psmux".into(),
            VERIFIED_VERSION.into(),
            "tako-unit".into(),
            std::env::temp_dir().join(format!("tako-psmux-unit-{}", std::process::id())),
        )
    }

    fn session(name: &str) -> SessionRef {
        SessionRef::new(name).unwrap()
    }

    /// [`PsmuxCapture`] の定義から採取実装の終わりまでのソース。
    /// **読み取り専用の約束**を機械検査するための材料（下の 2 つのテストが使う）
    fn capture_impl_source() -> &'static str {
        let src = include_str!("psmux.rs");
        let start = src
            .find("pub struct PsmuxCapture")
            .expect("採取実装が見つからない");
        let end = src[start..]
            .find("fn parse_history_probe")
            .expect("採取実装の終端（次の自由関数）");
        &src[start..start + end]
    }

    /// **#519 の中核**: psmux は「器あり・採取あり・送出なし」という、
    /// これまで存在しなかった組み合わせを申告する。
    /// bool 集合で能力を持つ設計（設計 §3.6）がこれを表現できることの実証でもある
    #[test]
    fn 能力は器ありかつ採取ありかつ送出なし() {
        let caps = backend().capabilities();
        assert!(caps.survives_app_exit, "器は tako の終了を生き延びる");
        assert!(
            caps.detached_capture,
            "capture-pane は動く（役割 B の読み側）"
        );
        assert!(
            !caps.detached_access,
            "送出は信頼できないので持たない（役割 B の書き側）"
        );
        assert_eq!(caps.scrollback, ScrollbackAuthority::InProcess);
        assert_eq!(caps.label, "psmux");
        assert!(caps.degraded_note().is_none(), "器があるので縮退ではない");
        assert!(backend().detached().is_none(), "送出の入口は開けない");
        assert!(backend().detached_capture().is_some(), "採取の入口は開く");
    }

    /// **要件 1**: `=`（exact-match 接頭辞）を使わない。
    /// これを破ると `kill-session` が 5.1 秒ブロックした末に失敗し、
    /// ペインを閉じるたびに器と pwsh がリークする
    #[test]
    fn ターゲットにイコールを付けない() {
        let b = backend();
        let s = session("tako-0123456789ab");
        assert_eq!(b.target(&s), "tako-0123456789ab");
        assert!(!b.target(&s).starts_with('='));
        // wrap_spawn の argv にも `=` 付きターゲットは現れない
        let args = b
            .wrap_spawn(SpawnOptions::default(), &s)
            .command
            .unwrap()
            .args;
        assert!(
            !args.iter().any(|a| a.starts_with("=tako-")),
            "argv に = 付きターゲットが混ざっている: {args:?}"
        );
        // 採取側も同じ約束（`={name}` の組み立てをしていない）
        assert!(
            !capture_impl_source().contains("\"={"),
            "採取実装が = 付きターゲットを組み立てている"
        );
    }

    /// **要件 5**: conf に `set -g warm off` がある（常駐 +243MB → +131MB / session）。
    /// あわせて **psmux が知らないオプションを持ち込まない**ことも固定する
    /// （知らない行が 1 つでもあると `psmux: N config warning(s):` がペインに出る）
    #[test]
    fn confはwarm_offを含み未知のオプションを含まない() {
        assert!(
            BACKEND_CONF.contains("set -g warm off"),
            "warm off が無いと 1 セッションあたり pwsh が 1 本余計に常駐する"
        );
        for unknown in [
            "extended-keys",
            "copy-mode-position-format",
            "default-terminal",
            "terminal-features",
        ] {
            assert!(
                !BACKEND_CONF.contains(unknown),
                "psmux が知らない設定 {unknown} が conf に残っている（ペインへ警告が出る）"
            );
        }
    }

    /// **要件 2**: `show-environment` が全変数を返しても目的の 1 本を取り出せる
    #[test]
    fn 環境変数は全変数出力から目的の行を選ぶ() {
        let out = "TAKO_TAB_ID=3\nTAKO_PANE_ID=7\nPSMUX_TARGET_SESSION=tako__tako-abc\n";
        assert_eq!(select_env_value(out, "TAKO_PANE_ID"), Some("7".to_string()));
        assert_eq!(select_env_value(out, "TAKO_TAB_ID"), Some("3".to_string()));
        assert_eq!(select_env_value(out, "TAKO_TOKEN"), None);
        // 前方一致の別変数に釣られない
        assert_eq!(select_env_value("TAKO_PANE_ID_X=9\n", "TAKO_PANE_ID"), None);
        // tmux 互換の 1 行出力でも同じ実装で通る
        assert_eq!(
            select_env_value("TAKO_PANE_ID=7", "TAKO_PANE_ID"),
            Some("7".to_string())
        );
        // unset マーカー（`-NAME`）は値として拾わない
        assert_eq!(select_env_value("-TAKO_PANE_ID\n", "TAKO_PANE_ID"), None);
    }

    /// **要件 4**: 偽の tty を返さない
    #[test]
    fn pane_ttyは常にnone() {
        assert!(backend().pane_tty(&session("tako-0123456789ab")).is_none());
    }

    /// **#659**: 器の中のシェルの pid を取り出せる。
    /// これが取れないと、器の内側の疑似コンソールを UTF-8 に固定できない
    /// （psmux ペインだけ CP932 のまま残る = #655 の対処が届かない）
    #[test]
    fn 器の中のペインのpidを取り出す() {
        assert_eq!(parse_pane_pids("13144\n"), vec![13144]);
        // 器の中で分割された分もそのまま載る（コードページは疑似コンソール単位）
        assert_eq!(parse_pane_pids("13144\n18248\n"), vec![13144, 18248]);
        // 空・非数値・0 は捨てる（psmux が警告を混ぜても pid と誤認しない）
        assert!(parse_pane_pids("").is_empty());
        assert!(parse_pane_pids("psmux: no server running\n").is_empty());
        assert!(parse_pane_pids("0\n").is_empty());
        assert_eq!(parse_pane_pids(" 42 \r\n"), vec![42]);
    }

    /// **#686**: copy mode の在否は 0/1 のときだけ断定する。
    /// psmux は**未対応の書式変数を空文字に展開する**（`#{mouse_any_flag}` で実測）ので、
    /// 空を `false` に倒すと「答えられない器」を「copy mode ではない」と誤断定する
    #[test]
    fn copy_modeの判定は0と1のときだけ断定する() {
        assert_eq!(parse_in_mode("1\n"), Some(true));
        assert_eq!(parse_in_mode("0\n"), Some(false));
        assert_eq!(parse_in_mode(""), None);
        assert_eq!(parse_in_mode("\n"), None);
        assert_eq!(parse_in_mode("#{pane_in_mode}"), None);
    }

    /// **#686**: in-band 解除キーを申告する。ソケット経由の `send-keys -X cancel` は
    /// 打鍵との順序が保証できず、打鍵経路にサブプロセスを同期で挟むことにもなる
    #[test]
    fn copy_mode解除キーを申告する() {
        assert_eq!(backend().copy_mode_exit_bytes(), Some(&b"q"[..]));
    }

    /// **要件 3**: `#{history_bytes}` が空でも probe を落とさない。
    ///
    /// M2 では「役割 B を持たない」ことで `#{history_bytes}` 依存を構造的に避けていた。
    /// 採取を持たせた以上、**空フィールドで `None` を返さない**ことを固定して
    /// 同じ結果（bytes に依存しない）を保証する（#654 の `parse_history_state` と同じ設計）
    #[test]
    fn 履歴probeはhistory_bytesが空でも成立する() {
        let p = parse_history_probe("279\t10000\t\t0").expect("bytes が空でも読める");
        assert_eq!(p.history, 279);
        assert_eq!(p.limit, 10000);
        assert_eq!(
            p.bytes, 0,
            "観測できない値は 0（バイト数 0 の意味ではない）"
        );
        assert!(!p.alternate);
        // alt screen の申告は拾う（履歴 0 の理由を呼び出し側が説明できる）
        assert!(parse_history_probe("0\t10000\t\t1").unwrap().alternate);
        // history_size すら読めない出力は器の応答として壊れている
        assert!(parse_history_probe("").is_none());
        assert!(parse_history_probe("\t10000\t\t0").is_none());
    }

    /// `list-panes -a -F` の一括 probe（#369 の probe 一括化を psmux でも通す）
    #[test]
    fn 一括probeはセッション名ごとに履歴を返す() {
        let out = "tako-aaaaaaaaaaaa\t279\t10000\t\t0\n\
                   tako-bbbbbbbbbbbb\t0\t10000\t\t1\n\
                   壊れた行\n";
        let parsed = parse_history_probe_batch(out);
        assert_eq!(parsed.len(), 2, "壊れた行は捨てる: {parsed:?}");
        assert_eq!(parsed[0].0.as_str(), "tako-aaaaaaaaaaaa");
        assert_eq!(parsed[0].1.history, 279);
        assert!(parsed[1].1.alternate);
    }

    /// 器の表示位置の読み戻し（#687）。copy mode の外は 0 / 非モード
    #[test]
    fn スクロール位置の読み戻し() {
        let p = super::super::parse_scroll_probe("0\t279\t0\t0").unwrap();
        assert_eq!(p.position, 0);
        assert_eq!(p.history, 279);
        assert!(!p.in_mode);
        assert!(!p.alternate);

        let p = super::super::parse_scroll_probe("24\t279\t1\t0").unwrap();
        assert_eq!(p.position, 24);
        assert!(p.in_mode);

        // tmux は copy mode の外で #{scroll_position} を空にする
        let p = super::super::parse_scroll_probe("\t279\t0\t0").unwrap();
        assert_eq!(p.position, 0);
        assert_eq!(p.history, 279);

        // alt screen（内側 TUI）は履歴 0 + alternate=1
        let p = super::super::parse_scroll_probe("\t0\t0\t1").unwrap();
        assert!(p.alternate);
        assert_eq!(p.history, 0);

        assert!(super::super::parse_scroll_probe("").is_none());
    }

    /// **読み取り専用の約束**: 採取実装が器の状態を変えるコマンドを呼ばない。
    /// 送出・モード変更を足したくなったらここが落ちる（設計の意図を残すための番犬）
    #[test]
    fn 採取実装は読み取りコマンドしか使わない() {
        let body = capture_impl_source();
        for forbidden in [
            "send-keys",
            "copy-mode",
            "resize-window",
            "paste-buffer",
            "kill-session",
            "set-environment",
        ] {
            assert!(
                !body.contains(forbidden),
                "採取実装に器を変えるコマンド {forbidden} が入っている（読み取り専用の約束違反）"
            );
        }
        for expected in ["capture-pane", "display-message", "list-panes"] {
            assert!(body.contains(expected), "{expected} を使っていない");
        }
    }

    #[test]
    fn wrap_spawn_argv_スナップショット() {
        let b = backend();
        let s = session("tako-0123456789ab");
        let options = SpawnOptions {
            command: None,
            cwd: Some(PathBuf::from("C:\\work")),
            env: vec![
                ("TAKO_PANE_ID".into(), "7".into()),
                ("TAKO_TOKEN".into(), "secret".into()),
                ("TAKO_TAB_ID".into(), "3".into()),
            ],
        };
        let wrapped = b.wrap_spawn(options.clone(), &s);
        let cmd = wrapped.command.expect("psmux クライアントの起動コマンド");
        assert_eq!(cmd.program, "psmux");
        // conf のパスは data_dir 依存（無い環境では -f ごと落ちる）のでその区間を畳む
        let mut args = cmd.args.clone();
        if let Some(at) = args.iter().position(|a| a == "-f") {
            args.splice(at..=at + 1, ["-f".to_string(), "<conf>".to_string()]);
        } else {
            args.splice(3..3, ["-f".to_string(), "<conf>".to_string()]);
        }
        assert_eq!(
            args,
            vec![
                "-u",
                "-L",
                "tako-unit",
                "-f",
                "<conf>",
                "new-session",
                "-A",
                "-D",
                "-s",
                "tako-0123456789ab",
                "-e",
                "TAKO_PANE_ID=7",
                "-e",
                "TAKO_TAB_ID=3",
                "-c",
                "C:\\work",
            ]
        );
        // env / cwd は素通しで in-process の PTY へ渡る
        assert_eq!(wrapped.env, options.env);
        assert_eq!(wrapped.cwd, options.cwd);
        b.owners.release(&s);
    }

    /// 内側コマンドの組み立て（psmux の引数パーサ実測に基づく）
    #[test]
    fn 内側コマンドはプログラムを裸で引数を引用する() {
        // バックスラッシュを含むプログラムは**引用しない**（引用すると起動に失敗する）
        let cmd = SpawnCommand {
            program: "C:\\Windows\\System32\\cmd.exe".into(),
            args: vec!["/c".into(), "echo hi".into()],
        };
        assert_eq!(
            inner_command(&cmd),
            "C:\\Windows\\System32\\cmd.exe /c 'echo hi'"
        );

        // 空白を含むプログラムは第 1 語として書けないので cmd.exe /c '<Windows 行>' に包む
        let cmd = SpawnCommand {
            program: "C:\\Program Files\\PowerShell\\7\\pwsh.exe".into(),
            args: vec![
                "-NoLogo".into(),
                "-Command".into(),
                "Write-Output ok".into(),
            ],
        };
        assert_eq!(
            inner_command(&cmd),
            "cmd.exe /c '\"C:\\Program Files\\PowerShell\\7\\pwsh.exe\" -NoLogo -Command \"Write-Output ok\"'"
        );

        // 引数のバックスラッシュは単一引用符の中で素通しになる（実測）
        let cmd = SpawnCommand {
            program: "cmd.exe".into(),
            args: vec!["/c".into(), "echo C:\\Foo\\Bar".into()],
        };
        assert_eq!(inner_command(&cmd), "cmd.exe /c 'echo C:\\Foo\\Bar'");
    }

    #[test]
    fn 実行ペインのbase64引数は器の引数解釈を素通りする() {
        // #525: 実行ペインは PowerShell へスクリプトを base64（`-EncodedCommand`）で渡す。
        // その狙いは「psmux の引数解釈を通っても中身が書き換えられない」こと。
        // base64 の文字（`A-Za-z0-9+/=`）は `quote_for_shell` の安全集合に入るので
        // 引用符が付かない、という前提をここで固定する。
        // **崩れると引用符入り・日本語入りのコマンドだけが静かに壊れる**
        let encoded = "JABnAG8AbwBkACsAZQBuAGMALwBvAGQAZQBkAD0A";
        let cmd = SpawnCommand {
            program: "pwsh.exe".into(),
            args: vec!["-NoLogo".into(), "-EncodedCommand".into(), encoded.into()],
        };
        assert_eq!(
            inner_command(&cmd),
            format!("pwsh.exe -NoLogo -EncodedCommand {encoded}"),
        );

        // 空白入りのプログラムパス（pwsh 7 の既定インストール先）で cmd.exe 経由に
        // 包まれても、base64 はやはり素のまま通る
        let cmd = SpawnCommand {
            program: "C:\\Program Files\\PowerShell\\7\\pwsh.exe".into(),
            args: vec!["-NoLogo".into(), "-EncodedCommand".into(), encoded.into()],
        };
        let line = inner_command(&cmd);
        assert!(
            line.contains(&format!("-EncodedCommand {encoded}'")),
            "base64 が引用・改変された: {line}"
        );
    }

    #[test]
    fn list_sessionsの3列出力をパースする() {
        let out = "tako-aaaaaaaaaaaa\t0\t1785138302\ntako-bbbbbbbbbbbb\t1\t1785138999\n\
                   壊れた行\n";
        let parsed = parse_sessions(out);
        assert_eq!(parsed.len(), 2, "壊れた行は捨てる: {parsed:?}");
        assert_eq!(parsed[0].session.as_str(), "tako-aaaaaaaaaaaa");
        assert!(!parsed[0].attached);
        assert_eq!(parsed[0].last_activity, 1785138302);
        assert!(parsed[1].attached);
    }

    /// 接頭辞ガード: tako が作った器以外は壊さない（`=` を使わない代償の安全網）
    #[test]
    fn 接頭辞が違う名前はkillしない() {
        let b = backend();
        let err = b.kill(&session("user-session")).unwrap_err();
        assert!(matches!(err, BackendError::InvalidSession { .. }), "{err}");
    }

    /// **要件 6**: 検証済みバージョンだけを無条件に信じる
    #[test]
    fn バージョン判定は検証済みだけを通す() {
        assert_eq!(version_support(VERIFIED_VERSION), VersionSupport::Verified);
        assert_eq!(version_support(" 3.3.7 "), VersionSupport::Verified);
        assert_eq!(version_support("3.3.8"), VersionSupport::Untested);
        assert_eq!(version_support("3.4.0"), VersionSupport::Untested);
        assert_eq!(version_support(""), VersionSupport::Untested);
    }

    /// **要件 7**: 器を握っている別インスタンスをオーナー記録から報告し、
    /// その PID は「所有インスタンスそのもの（生存確認済み）」として渡す
    #[test]
    fn 別インスタンスの所有をholderとして報告する() {
        let b = backend();
        let s = session("tako-cccccccccccc");
        assert!(b.foreign_holders(std::slice::from_ref(&s)).is_empty());

        b.owners.claim_as(&s, 424242);
        let holders = b.foreign_holders(std::slice::from_ref(&s));
        assert_eq!(holders.len(), 1);
        assert_eq!(holders[0].pid, 424242);
        assert_eq!(
            holders[0].kind,
            HolderKind::Owner,
            "psmux の PID は所有インスタンスそのもの（クライアントではない）"
        );
        // 所有されている器は残骸掃除の対象にしない
        let info = SessionInfo {
            session: s.clone(),
            attached: false,
            last_activity: 0,
        };
        assert!(
            !b.is_orphan_candidate(&info, &HashSet::new()),
            "生きた別インスタンスの器を残骸と見なしてはいけない"
        );
        b.owners.release(&s);
        assert!(
            b.is_orphan_candidate(&info, &HashSet::new()),
            "所有者が居なくなれば残骸候補になる"
        );
    }

    #[test]
    fn 残骸候補の判定は接頭辞とprotectedを見る() {
        let b = backend();
        let info = |name: &str| SessionInfo {
            session: session(name),
            attached: false,
            last_activity: 0,
        };
        let empty = HashSet::new();
        assert!(b.is_orphan_candidate(&info("tako-dddddddddddd"), &empty));
        assert!(!b.is_orphan_candidate(&info("user-sess"), &empty));
        assert!(!b.is_orphan_candidate(&info("tako-view-x"), &empty));
        let protected: HashSet<SessionRef> = [session("tako-dddddddddddd")].into_iter().collect();
        assert!(!b.is_orphan_candidate(&info("tako-dddddddddddd"), &protected));
    }
}
