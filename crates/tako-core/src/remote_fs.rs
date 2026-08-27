//! remote_fs — SSH 先のディレクトリ・ファイルを SFTP で読む（#65 / #919）
//!
//! # なぜ russh / ssh2 ではなくシステムの `ssh` / `sftp` を呼ぶのか
//!
//! #919 要件 6 と #65 要件 1 は「認証は既存 SSH 設定を再利用する（`~/.ssh/config`・鍵・
//! **ControlMaster の共有で追加認証なし**）」。ControlMaster は OpenSSH のクライアント間で
//! 共有される私的な多重化プロトコルで、**russh / ssh2 からは相乗りできない**。crate を
//! 採ると `~/.ssh/config` の解決（`Match` / `Include` / `ProxyJump` / `IdentityAgent`）・
//! known_hosts の検証と更新・agent・FIDO 鍵・2FA を tako 側で作り直すことになり、
//! それでも「ユーザーが `ssh <host>` で入れる先に tako だけ入れない」状態が残る。
//!
//! さらに ssh2 は libssh2 + OpenSSL の C 依存を持ち込み、#467（Windows 移植）の
//! クロスビルドを重くする。Windows 10 以降は OpenSSH クライアントを同梱するので、
//! システムの `ssh` / `sftp` を呼ぶ形は両 OS で同じ経路になる。
//!
//! `git.rs` / `tmux.rs` が git / tmux の CLI を子プロセスで呼ぶのと同じ構え。
//!
//! # 何を子プロセスへ出すか
//!
//! - **接続の器**: `ssh -M -N -f`（ControlMaster）。以後の全操作がこのソケットへ相乗りし、
//!   再認証が起きない。パスワードしか無い相手でも、対話 SSH ペインで一度ログインすれば
//!   同じソケットを共有してツリーが開く（#65「パスワード認証しかない場合の UX」）
//! - **ディレクトリ一覧・取得**: `sftp -b -`（バッチ）。**ログインシェルに依存しない**ので
//!   相手が PowerShell（Windows OpenSSH）でも同じ経路で動く（実測済み）
//!
//! # 静かな失敗を作らない（#919）
//!
//! すべての失敗は [`RemoteError`] へ分類して返す。「何が起きたか」（[`RemoteError::summary`]）と
//! 「次に何をすべきか」（[`RemoteError::next_step`]）を型が持つので、呼び出し側が
//! 握り潰しても空の画面にはならない。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::i18n::Lang;
use crate::platform::support::Note;

/// 接続確立（ControlMaster）の待ち上限。ssh の既定（約 75 秒の TCP タイムアウト）だと
/// 「タブが真っ黒のまま無反応」が長すぎて壊れていると誤解される（#919 の実測）
pub const CONNECT_TIMEOUT_SECS: u32 = 10;

/// ControlMaster を維持する秒数。閉じてもしばらく生かしておくと、
/// ツリーの展開ごとに再認証が起きない
pub const CONTROL_PERSIST_SECS: u32 = 600;

/// 1 ディレクトリから読み取る最大エントリ数（巨大ディレクトリの暴走防止。#65 要件 4）
pub const MAX_ENTRIES: usize = 2000;

/// プレビューで取得するファイルサイズの上限（#65 要件 3）
pub const MAX_PREVIEW_BYTES: u64 = 8 * 1024 * 1024;

/// 転送中に相手が黙ったときの見切り（`ServerAliveInterval` × `ServerAliveCountMax` ≒ 15 秒）。
/// `Command` に待ち上限は無いので、**OpenSSH 側の keepalive で切らせる**。
/// これが無いと相手のスリープ・ネットワーク断で sftp が無期限に生き残る
pub const SERVER_ALIVE_INTERVAL_SECS: u32 = 5;
pub const SERVER_ALIVE_COUNT_MAX: u32 = 3;

/// symlink の実体がディレクトリかを確かめる件数の上限（1 ディレクトリあたり）
const MAX_SYMLINK_PROBES: usize = 64;

// --- リモートの位置 ---------------------------------------------------------

/// リモートの 1 か所（ホスト名 = `~/.ssh/config` の Host、パス = SFTP のパス）
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RemoteRef {
    pub host: String,
    pub path: String,
}

impl RemoteRef {
    pub fn new(host: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            path: path.into(),
        }
    }

    /// 表示用の短い名前（末尾の要素。ルートなら `/`）
    pub fn base_name(&self) -> String {
        base_name(&self.path)
    }

    /// `host:/path` 形式（ログ・一覧の表示用）
    pub fn label(&self) -> String {
        format!("{}:{}", self.host, self.path)
    }
}

/// リモートエントリの種別。SFTP の readdir は **lstat** 相当を返すので
/// symlink はそのまま `Symlink` で来る（実体がディレクトリかは別に確かめる）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteKind {
    File,
    Dir,
    /// 実体を確かめられていない symlink
    Symlink,
    /// mode 文字列が読めなかった（サーバー実装差）。ディレクトリとして開くのは試せる
    Unknown,
}

impl RemoteKind {
    /// ツリーでディレクトリとして扱うか（展開できるか）
    pub fn expandable(self) -> bool {
        matches!(self, RemoteKind::Dir | RemoteKind::Symlink)
    }
}

/// リモートの 1 エントリ
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    pub name: String,
    /// リモート側の絶対パス
    pub path: String,
    pub kind: RemoteKind,
    pub size: u64,
}

impl RemoteEntry {
    pub fn is_dir(&self) -> bool {
        self.kind.expandable()
    }
}

// --- 失敗の分類 -------------------------------------------------------------

/// 失敗の種類。**画面に理由を出すため**の分類なので、ユーザーが取れる次の手が
/// 変わらないものは 1 つにまとめている
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteErrorKind {
    /// `ssh` / `sftp` が見つからない
    ClientMissing,
    /// 名前解決できない
    HostUnresolved,
    /// 到達できない（no route / network unreachable）
    Unreachable,
    /// 接続を拒否された（相手に sshd が居ない・ポート違い）
    Refused,
    /// 接続がタイムアウトした
    Timeout,
    /// 認証できない（鍵が無い・パスワードが要る）
    AuthFailed,
    /// known_hosts と合わない
    HostKeyMismatch,
    /// sftp サブシステムが無い
    SftpUnavailable,
    /// パスが無い
    NotFound,
    /// パスの権限が無い
    PermissionDenied,
    /// ディレクトリではない
    NotDirectory,
    /// サイズ上限を超えた
    TooLarge,
    /// **開いた時点とリモートの実体が食い違っている**（#966。書き戻しを止めた）
    Conflict,
    /// 応答が読めない・想定外の失敗
    Other,
}

impl RemoteErrorKind {
    /// 種別の全列挙。**足したらここにも足す**（`種別の全列挙に漏れがない` が
    /// 網羅 match で足し忘れを落とす）
    pub const ALL: &'static [RemoteErrorKind] = &[
        RemoteErrorKind::ClientMissing,
        RemoteErrorKind::HostUnresolved,
        RemoteErrorKind::Unreachable,
        RemoteErrorKind::Refused,
        RemoteErrorKind::Timeout,
        RemoteErrorKind::AuthFailed,
        RemoteErrorKind::HostKeyMismatch,
        RemoteErrorKind::SftpUnavailable,
        RemoteErrorKind::NotFound,
        RemoteErrorKind::PermissionDenied,
        RemoteErrorKind::NotDirectory,
        RemoteErrorKind::TooLarge,
        RemoteErrorKind::Conflict,
        RemoteErrorKind::Other,
    ];

    /// 何が起きたか（1 行）
    pub fn summary_note(self) -> Note {
        match self {
            RemoteErrorKind::ClientMissing => Note::new(
                "ssh / sftp コマンドが見つかりません",
                "The ssh / sftp command was not found",
            ),
            RemoteErrorKind::HostUnresolved => Note::new(
                "ホスト名を解決できません",
                "Could not resolve the host name",
            ),
            RemoteErrorKind::Unreachable => {
                Note::new("ホストに到達できません", "The host is unreachable")
            }
            RemoteErrorKind::Refused => Note::new(
                "接続を拒否されました（相手側で SSH が待ち受けていません）",
                "Connection refused (no SSH server is listening)",
            ),
            RemoteErrorKind::Timeout => Note::new(
                "接続がタイムアウトしました（応答がありません）",
                "The connection timed out (no response)",
            ),
            RemoteErrorKind::AuthFailed => {
                Note::new("認証できませんでした", "Authentication failed")
            }
            RemoteErrorKind::HostKeyMismatch => Note::new(
                "ホスト鍵が known_hosts と一致しません",
                "The host key does not match known_hosts",
            ),
            RemoteErrorKind::SftpUnavailable => Note::new(
                "相手側で SFTP サブシステムが使えません",
                "The SFTP subsystem is unavailable on the remote host",
            ),
            RemoteErrorKind::NotFound => {
                Note::new("パスが見つかりません", "The path was not found")
            }
            RemoteErrorKind::PermissionDenied => Note::new(
                "パスへのアクセスが拒否されました",
                "Access to the path was denied",
            ),
            RemoteErrorKind::NotDirectory => {
                Note::new("ディレクトリではありません", "Not a directory")
            }
            RemoteErrorKind::TooLarge => Note::new(
                "ファイルが大きすぎます（プレビュー上限を超えています）",
                "The file is too large to preview",
            ),
            RemoteErrorKind::Conflict => Note::new(
                "開いたときからリモート側が変わっています（上書きしませんでした）",
                "The remote file changed since you opened it (nothing was overwritten)",
            ),
            RemoteErrorKind::Other => Note::new("接続に失敗しました", "The connection failed"),
        }
    }

    /// 次に何をすべきか（1 行）。**必ず何か出す**（空にしない）
    pub fn next_step_note(self) -> Note {
        match self {
            RemoteErrorKind::ClientMissing => Note::new(
                "OpenSSH クライアントを入れてください（Windows は「OpenSSH クライアント」の追加機能）",
                "Install the OpenSSH client (on Windows, the \"OpenSSH Client\" optional feature)",
            ),
            RemoteErrorKind::HostUnresolved => Note::new(
                "~/.ssh/config の Host 名と HostName を確認してください",
                "Check the Host and HostName entries in ~/.ssh/config",
            ),
            RemoteErrorKind::Unreachable | RemoteErrorKind::Timeout => Note::new(
                "相手の電源・ネットワーク（VPN / Tailscale）が生きているか確認してください",
                "Check that the host is powered on and the network (VPN / Tailscale) is up",
            ),
            RemoteErrorKind::Refused => Note::new(
                "相手の SSH サーバーとポート番号を確認してください",
                "Check the remote SSH server and its port number",
            ),
            RemoteErrorKind::AuthFailed => Note::new(
                "鍵認証が使えない相手なら、先に「リモート接続」で SSH ペインを開いてログインしてください（接続が共有され、以後ツリーが開けます）",
                "If the host has no key auth, first open an SSH pane from \"Open Remote\" and log in (the connection is shared, so the tree opens afterwards)",
            ),
            RemoteErrorKind::HostKeyMismatch => Note::new(
                "相手を作り直した場合は ~/.ssh/known_hosts の該当行を消してください",
                "If the host was rebuilt, remove its line from ~/.ssh/known_hosts",
            ),
            RemoteErrorKind::SftpUnavailable => Note::new(
                "相手の sshd_config で Subsystem sftp が有効か確認してください",
                "Check that Subsystem sftp is enabled in the remote sshd_config",
            ),
            RemoteErrorKind::NotFound => Note::new(
                "パスの綴りを確認してください",
                "Check the spelling of the path",
            ),
            RemoteErrorKind::PermissionDenied => Note::new(
                "そのパスを読める権限があるか確認してください",
                "Check that you have permission to read that path",
            ),
            RemoteErrorKind::NotDirectory => Note::new(
                "ディレクトリを指定してください",
                "Specify a directory instead",
            ),
            RemoteErrorKind::TooLarge => Note::new(
                "小さいファイルを選ぶか、SSH ペインで直接開いてください",
                "Pick a smaller file, or open it directly in an SSH pane",
            ),
            RemoteErrorKind::Conflict => Note::new(
                "リモートを読み直して編集をやり直すか、こちらの内容で上書きしてよければ強制保存してください（編集内容はローカルに残っています）",
                "Reload the remote file and redo your edit, or force-save to overwrite it with your version (your edit is kept locally)",
            ),
            RemoteErrorKind::Other => Note::new(
                "下の詳細を確認してください",
                "Check the details below",
            ),
        }
    }

    /// 機械可読な種別名（CLI / MCP 応答用）
    pub fn as_str(self) -> &'static str {
        match self {
            RemoteErrorKind::ClientMissing => "client_missing",
            RemoteErrorKind::HostUnresolved => "host_unresolved",
            RemoteErrorKind::Unreachable => "unreachable",
            RemoteErrorKind::Refused => "refused",
            RemoteErrorKind::Timeout => "timeout",
            RemoteErrorKind::AuthFailed => "auth_failed",
            RemoteErrorKind::HostKeyMismatch => "host_key_mismatch",
            RemoteErrorKind::SftpUnavailable => "sftp_unavailable",
            RemoteErrorKind::NotFound => "not_found",
            RemoteErrorKind::PermissionDenied => "permission_denied",
            RemoteErrorKind::NotDirectory => "not_directory",
            RemoteErrorKind::TooLarge => "too_large",
            RemoteErrorKind::Conflict => "conflict",
            RemoteErrorKind::Other => "other",
        }
    }
}

/// 失敗（種別 + 相手 + 生の詳細）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteError {
    pub kind: RemoteErrorKind,
    /// 接続先（表示用。`host` または `host:/path`）
    pub target: String,
    /// ssh / sftp が出した文言の要点（そのまま出す = 診断の最後の手がかりを消さない）
    pub detail: String,
}

impl RemoteError {
    pub fn new(
        kind: RemoteErrorKind,
        target: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            target: target.into(),
            detail: detail.into(),
        }
    }

    /// 何が起きたか（相手つき 1 行）
    pub fn summary(&self) -> String {
        self.summary_in(crate::i18n::lang())
    }

    /// 言語を明示しての要約。**言語グローバルに触らず解決できる**ようにするため、
    /// 実体はこちらの純粋関数に置く（`Note::text_in` と同じ作法）
    pub fn summary_in(&self, lang: Lang) -> String {
        let what = self.kind.summary_note().text_in(lang);
        match lang {
            Lang::Ja => format!("{}: {what}", self.target),
            Lang::En => format!("{}: {what}", self.target),
        }
    }

    /// 次に何をすべきか
    pub fn next_step(&self) -> &'static str {
        self.kind.next_step_note().text()
    }

    pub fn next_step_in(&self, lang: Lang) -> &'static str {
        self.kind.next_step_note().text_in(lang)
    }

    /// 画面・ペインへそのまま出せる複数行（要約 → 次の一手 → 詳細）。
    /// **どの経路で失敗しても空文字にならない**のが要点（#919 の無言失敗対策）
    pub fn report(&self) -> String {
        self.report_in(crate::i18n::lang())
    }

    pub fn report_in(&self, lang: Lang) -> String {
        let mut out = self.summary_in(lang);
        out.push('\n');
        out.push_str(self.next_step_in(lang));
        let detail = self.detail.trim();
        if !detail.is_empty() {
            out.push('\n');
            out.push_str(detail);
        }
        out
    }
}

impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.summary())?;
        let detail = self.detail.trim();
        if !detail.is_empty() {
            write!(f, " ({detail})")?;
        }
        Ok(())
    }
}

impl std::error::Error for RemoteError {}

/// ssh / sftp の出力から失敗の種類を決める（純粋関数）。
///
/// 判定順に意味がある: `Permission denied` は認証失敗（`Permission denied (publickey...)`）と
/// パスの権限で同じ語を使うので、**括弧つきの認証方式リスト**があるものを先に認証失敗へ倒す
pub fn classify_output(text: &str) -> RemoteErrorKind {
    let lower = text.to_ascii_lowercase();
    let has = |needle: &str| lower.contains(needle);

    if has("could not resolve hostname")
        || has("name or service not known")
        || has("nodename nor servname")
    {
        return RemoteErrorKind::HostUnresolved;
    }
    if has("connection timed out") || has("operation timed out") || has("timed out") {
        return RemoteErrorKind::Timeout;
    }
    if has("connection refused") {
        return RemoteErrorKind::Refused;
    }
    if has("no route to host") || has("network is unreachable") || has("host is down") {
        return RemoteErrorKind::Unreachable;
    }
    if has("host key verification failed") || has("remote host identification has changed") {
        return RemoteErrorKind::HostKeyMismatch;
    }
    // `Permission denied (publickey,password,...)` = 認証。括弧の中に方式が並ぶのが目印
    if has("permission denied (")
        || has("too many authentication failures")
        || has("no supported authentication methods")
        || has("host key verification")
        || (has("permission denied") && has("publickey"))
    {
        return RemoteErrorKind::AuthFailed;
    }
    if has("subsystem request failed") || has("this service allows sftp connections only") {
        return RemoteErrorKind::SftpUnavailable;
    }
    if has("not found") || has("no such file") {
        return RemoteErrorKind::NotFound;
    }
    if has("not a directory") {
        return RemoteErrorKind::NotDirectory;
    }
    if has("permission denied") {
        return RemoteErrorKind::PermissionDenied;
    }
    if has("batch mode") || has("passphrase") || has("password") {
        // BatchMode で止められた = 対話認証が要る
        return RemoteErrorKind::AuthFailed;
    }
    RemoteErrorKind::Other
}

// --- パス操作（純粋関数） ---------------------------------------------------

/// SFTP の引数として安全な形へ包む。
///
/// OpenSSH の sftp は**二重引用符の中では glob 展開をしない**（OpenSSH 10.2 で実測:
/// `ls "…/.b*"` は `.b*` を literal として扱い not found になる）。空白・`*`・`?`・`[` を
/// 含むパスをそのまま渡すと単語分割と glob 展開の両方に食われるので、常に包む
pub fn quote_sftp_arg(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 2);
    out.push('"');
    for ch in path.chars() {
        // 引用符の中で意味を持つのはこの 2 つだけ
        if ch == '"' || ch == '\\' {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    out
}

/// リモートの POSIX パスを連結する（`dir` の末尾スラッシュは吸収する）
pub fn join_remote(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        return name.to_string();
    }
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// 1 つ上のディレクトリ。ルート（`/`）なら None
pub fn parent_remote(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.rfind('/') {
        // `/foo` → `/`
        Some(0) => Some("/".to_string()),
        Some(idx) => Some(trimmed[..idx].to_string()),
        None => None,
    }
}

/// 末尾の要素（表示名）。ルートは `/` のまま返す
pub fn base_name(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_string();
    }
    match trimmed.rfind('/') {
        Some(idx) => trimmed[idx + 1..].to_string(),
        None => trimmed.to_string(),
    }
}

/// SFTP のパスを**相手のシェルが受け取れる形**へ直す。
///
/// Windows の OpenSSH sftp-server はドライブを `/C:/Users/...` の形で見せるが、
/// PowerShell に `/C:/Users/...` を渡すと先頭の `/` でルート相対と解釈されて失敗する。
/// SSH ペインへ `cd` を打つときはこちらを使う
pub fn shell_path(path: &str) -> String {
    let bytes = path.as_bytes();
    // `/C:/...` / `/C:` の形だけ先頭の `/` を落とす
    if bytes.len() >= 3
        && bytes[0] == b'/'
        && bytes[1].is_ascii_alphabetic()
        && bytes[2] == b':'
        && (bytes.len() == 3 || bytes[3] == b'/')
    {
        return path[1..].to_string();
    }
    path.to_string()
}

/// `sftp -b -` へ流すバッチ本文（1 行 1 コマンド + 終端の `bye`）
pub fn batch_script(commands: &[String]) -> String {
    let mut out = String::new();
    for c in commands {
        out.push_str(c);
        out.push('\n');
    }
    out.push_str("bye\n");
    out
}

/// `sftp> <cmd>` のエコー行かどうか。sftp はバッチモードで受け取った行を
/// **stdout へそのまま echo する**（`-q` でも消えない = 実測）ので、剥がしてから解析する
fn is_echo_line(line: &str) -> bool {
    line.starts_with("sftp> ")
}

/// mode 文字列（`drwxr-xr-x` / Windows の `drwx******`）から種別を読む。
/// mode に見えない場合は None（サーバー実装差で longname の形が違うとき）
fn kind_from_mode(field: &str) -> Option<RemoteKind> {
    let mut chars = field.chars();
    let first = chars.next()?;
    // 残り 9 文字が権限らしいこと（`rwx-` に加え Windows の `*`、setuid の `sStT`）
    let rest: Vec<char> = chars.collect();
    if rest.len() != 9 {
        return None;
    }
    if !rest
        .iter()
        .all(|c| matches!(c, 'r' | 'w' | 'x' | '-' | '*' | 's' | 'S' | 't' | 'T'))
    {
        return None;
    }
    Some(match first {
        'd' => RemoteKind::Dir,
        'l' => RemoteKind::Symlink,
        '-' | 'c' | 'b' | 'p' | 's' => RemoteKind::File,
        _ => return None,
    })
}

/// `ls -la` の 1 行を列へ分解した結果（純粋関数の内部表現）
struct LsRow {
    kind: RemoteKind,
    /// mode 欄（`-rw-r--r--` / Windows の `-rw-******`）
    mode: String,
    size: u64,
    /// 日時欄 3 列をまとめたもの（`Aug 27 14:58` / `Aug 27 2025`）。
    /// **分の分解能しかない**ので単独では競合検知に使わない（#966）
    mtime: String,
    /// 末尾の名前（`ls -l <file>` はフルパスを返すので末尾要素へ寄せる）
    name: String,
}

/// `ls -la` の 1 行を列へ分解する（純粋関数）。mode が読めない行は None。
///
/// 列の区切りを自前で辿るのは、**名前に空白が入る**（Windows の `Application Data`）ので
/// `splitn` では最後の列を切り出せないため。`?` が入ることもある（`ls -l <file>` の
/// nlink など）ので数値を仮定しない
fn parse_ls_row(line: &str) -> Option<LsRow> {
    let mode = line.split_whitespace().next().unwrap_or_default();
    let kind = kind_from_mode(mode)?;
    // mode の後ろ 7 列（nlink owner group size month day time）を辿って名前へ
    let mut size = 0u64;
    let mut date: Vec<String> = Vec::new();
    let mut skipped = 0usize;
    let mut rest_start = None;
    let mut cursor = mode.len();
    let bytes = line.as_bytes();
    while skipped < 7 && cursor < bytes.len() {
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let start = cursor;
        while cursor < bytes.len() && !bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if start == cursor {
            break;
        }
        match skipped {
            // 4 列目（mode から数えて 4 番目）が size
            3 => size = line[start..cursor].parse::<u64>().unwrap_or(0),
            // 5〜7 列目が日時（month day time-or-year）
            4..=6 => date.push(line[start..cursor].to_string()),
            _ => {}
        }
        skipped += 1;
        if skipped == 7 {
            // 残りが名前（先頭の空白だけ落とす。名前中の空白は残す）
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            rest_start = Some(cursor);
        }
    }
    let start = rest_start?;
    let name = &line[start..];
    // `ls -l <file>` は名前ではなくフルパスを返す（実測）。その場合は末尾要素を名前にする
    let name = if name.contains('/') {
        base_name(name)
    } else {
        name.to_string()
    };
    Some(LsRow {
        kind,
        mode: mode.to_string(),
        size,
        mtime: date.join(" "),
        name,
    })
}

/// `sftp` の `ls -la <dir>` 出力を解析する（純粋関数）。
///
/// 形は `mode nlink owner group size month day time-or-year name`（9 列目以降が名前 =
/// 空白を含む名前も落とさない）。`.` / `..` は落とす。mode が読めない行は
/// [`RemoteKind::Unknown`] として名前だけ拾う（黙って捨てない）
pub fn parse_ls_long(stdout: &str, dir: &str) -> Vec<RemoteEntry> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || is_echo_line(line) {
            continue;
        }
        // `Can't ls: "..." not found` のような診断行は名前として拾わない
        if line.starts_with("Can't ") || line.starts_with("Couldn't ") {
            continue;
        }
        let Some(row) = parse_ls_row(line) else {
            // mode に見えない = longname の形が違う。行全体を名前として扱う
            let name = line.trim().to_string();
            if name == "." || name == ".." || name.is_empty() {
                continue;
            }
            out.push(RemoteEntry {
                path: join_remote(dir, &name),
                name,
                kind: RemoteKind::Unknown,
                size: 0,
            });
            if out.len() >= MAX_ENTRIES {
                break;
            }
            continue;
        };
        if row.name.is_empty() || row.name == "." || row.name == ".." {
            continue;
        }
        out.push(RemoteEntry {
            path: join_remote(dir, &row.name),
            name: row.name,
            kind: row.kind,
            size: row.size,
        });
        if out.len() >= MAX_ENTRIES {
            break;
        }
    }
    out.sort_by(|a, b| {
        // ディレクトリを先に、次に名前（大文字小文字を無視）— ローカルツリーと同じ並び
        b.is_dir()
            .cmp(&a.is_dir())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
}

/// symlink の実体がディレクトリかを判定する行（`ls -1 <path>/` の結果）。
/// ディレクトリなら中身が並び、ファイルなら `Can't ls:` が出る（実測）
fn probe_says_directory(section: &str) -> bool {
    for line in section.lines() {
        let line = line.trim();
        if line.is_empty() || is_echo_line(line) {
            continue;
        }
        if line.starts_with("Can't ") || line.starts_with("Couldn't ") {
            return false;
        }
        return true;
    }
    // 中身が空のディレクトリ: エラーが出ていないなら開ける
    true
}

/// バッチ出力を `sftp> <cmd>` のエコーで切り分ける（純粋関数）。
/// 返すのは (コマンド, そのコマンドの出力) の並び
pub fn split_batch_sections(stdout: &str) -> Vec<(String, String)> {
    let mut sections: Vec<(String, String)> = Vec::new();
    for line in stdout.lines() {
        if let Some(cmd) = line.strip_prefix("sftp> ") {
            sections.push((cmd.trim().to_string(), String::new()));
        } else if let Some(last) = sections.last_mut() {
            last.1.push_str(line);
            last.1.push('\n');
        }
    }
    sections
}

// --- 子プロセス -------------------------------------------------------------

/// `ssh` の実体（見つからなければ None）
pub fn ssh_bin() -> Option<String> {
    resolve_bin("ssh")
}

/// `sftp` の実体
pub fn sftp_bin() -> Option<String> {
    resolve_bin("sftp")
}

fn resolve_bin(name: &str) -> Option<String> {
    crate::platform::exe::find(name)
}

/// ControlMaster のソケット置き場（`<data_dir>/ssh/`）
pub fn control_dir() -> Option<PathBuf> {
    crate::paths::data_dir().map(|d| d.join("ssh"))
}

/// unix domain socket のパス長上限（macOS の `sun_path` は 104 バイト）に対する安全域。
/// これを超えるなら短い置き場（temp）へ逃がす
const MAX_SOCKET_PATH: usize = 92;

/// ホストごとの ControlPath（純粋関数）。
///
/// ホスト名をハッシュへ落とすのは、`user@host:port` の記号がそのままパスへ出るのを
/// 避けるためと、長いホスト名でソケットのパス長上限を割らないため。
///
/// `<data_dir>` 側が上限を割る場合は temp へ逃がす: macOS の既定
/// （`~/Library/Application Support/tako/ssh/<16 桁>.sock` = 80 バイト）でも余裕が
/// 少なく、ユーザー名が長いと超える
pub fn control_path_in(data_dir: Option<&Path>, host: &str) -> PathBuf {
    let name = format!("{}.sock", short_hash(host));
    if let Some(dir) = data_dir {
        let candidate = dir.join("ssh").join(&name);
        if candidate.to_string_lossy().len() <= MAX_SOCKET_PATH {
            return candidate;
        }
    }
    std::env::temp_dir().join(format!("tako-ssh-{name}"))
}

/// ホストごとの ControlPath
pub fn control_path(host: &str) -> Option<PathBuf> {
    Some(control_path_in(crate::paths::data_dir().as_deref(), host))
}

/// `-o ControlPath=…` の値。**空白を含むパスは二重引用符で包む**。
///
/// OpenSSH の設定パーサは値を空白で切るので、素のまま渡すと
/// `keyword controlpath extra arguments at end of line` で全操作が失敗する。
/// macOS の既定 data_dir（`~/Library/Application Support/tako`）は空白を含むので
/// **既定構成で必ず踏む**（#833 と同型の罠）
pub fn control_path_option(path: &Path) -> String {
    format!("ControlPath=\"{}\"", path.display())
}

/// リモートファイルのキャッシュ置き場（`<data_dir>/remote-cache/`）
pub fn cache_dir() -> Option<PathBuf> {
    crate::paths::data_dir().map(|d| d.join("remote-cache"))
}

/// 安定した短いハッシュ（FNV-1a 64bit の 16 桁 hex）。
/// 暗号用途ではない（ソケット名の一意化だけ）
pub fn short_hash(s: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{hash:016x}")
}

/// 共通の `-o` オプション（ControlMaster への相乗り + 待ち上限）
fn common_opts(host: &str, batch: bool) -> Vec<String> {
    let mut opts = Vec::new();
    if let Some(cp) = control_path(host) {
        opts.push("-o".into());
        opts.push(control_path_option(&cp));
        opts.push("-o".into());
        opts.push("ControlMaster=auto".into());
        opts.push("-o".into());
        opts.push(format!("ControlPersist={CONTROL_PERSIST_SECS}"));
    }
    opts.push("-o".into());
    opts.push(format!("ConnectTimeout={CONNECT_TIMEOUT_SECS}"));
    opts.push("-o".into());
    opts.push(format!("ServerAliveInterval={SERVER_ALIVE_INTERVAL_SECS}"));
    opts.push("-o".into());
    opts.push(format!("ServerAliveCountMax={SERVER_ALIVE_COUNT_MAX}"));
    if batch {
        // 対話プロンプトで子プロセスが無言で止まるのを構造的に防ぐ
        opts.push("-o".into());
        opts.push("BatchMode=yes".into());
    }
    opts
}

/// SSH ペイン（対話）用の argv。
///
/// ツリー側と**同じ ControlPath** を使うのが要点: 相手がパスワード認証しか持たなくても、
/// ここで一度ログインすればソケットが共有され、以後ツリーが追加認証なしで開く（#65）。
/// `ConnectTimeout` を明示するのは、既定（約 75 秒）だと真っ黒な画面のまま
/// 「何も入力できない」に見えるため（#919 の実測）
pub fn ssh_pane_argv(host: &str, extra: &[String]) -> Vec<String> {
    let mut argv = vec![ssh_bin().unwrap_or_else(|| "ssh".into())];
    argv.extend(common_opts(host, false));
    argv.extend(extra.iter().cloned());
    argv.push(host.to_string());
    argv
}

/// SSH ペインの失敗行に添えるヒント（種別が分からない場所で使う汎用版）。
///
/// スクリプトの中では ssh の stderr を分類できない（理由は**その上の行に生で出ている**）。
/// なので [`RemoteErrorKind`] の個別ヒントは借りず、よくある原因を 1 行で並べる。
/// 特定の種別のヒントを流用すると、認証失敗に「電源を確認」のような誤った助言が出る
pub fn pane_failure_hint(lang: Lang) -> &'static str {
    match lang {
        Lang::Ja => "ネットワーク（VPN / Tailscale）・相手の電源・~/.ssh/config を確認してください",
        Lang::En => "Check the network (VPN / Tailscale), that the host is on, and ~/.ssh/config",
    }
}

/// ssh 自身の失敗を表す終了コード。**リモートのコマンドの終了コードと区別する**ため
/// これだけを「接続の失敗」として扱う（OpenSSH の man: "ssh exits with 255 if an
/// error occurred"）。リモートシェルが `exit 1` で終わったのを接続失敗と誤報しない
pub const SSH_ERROR_EXIT: i32 = 255;

/// SSH ペインで走らせるスクリプト（純粋関数。**macOS 上でも両方言をテストできる**）。
///
/// #919 の無言失敗を潰すのがこの関数の全目的:
///
/// - **接続前にバナーを出す**: 旧実装は `ssh` を直接ペインのプログラムにしていたため、
///   接続待ちのあいだ画面が**完全に空**だった（実測: TCP ブラックホールで 25 秒間 1 文字も
///   出ない）。打っても何も起きないので「何も入力できない」に見える
/// - **失敗しても消えない**: 旧実装は ssh が即死するとペインごと消え、タブまで閉じた
///   （実測: 名前解決できないホストは 1 秒でタブが消滅）。`255` のときだけ理由を出して
///   入力待ちで止まるので、**理由が画面に残る**
/// - 成功して普通に `exit` した場合は**従来どおり閉じる**（`255` 以外は素通し）
pub fn ssh_pane_script(
    dialect: crate::platform::shell_dialect::ShellDialect,
    argv: &[String],
    host: &str,
    cd_to: Option<&str>,
    lang: Lang,
) -> String {
    use crate::platform::shell_dialect::ShellDialect;

    let connecting = match lang {
        Lang::Ja => format!("tako: {host} へ接続しています…（中止は Ctrl+C）"),
        Lang::En => format!("tako: connecting to {host}… (Ctrl+C to cancel)"),
    };
    let failed = match lang {
        Lang::Ja => {
            format!("tako: {host} への接続に失敗しました（ssh exit 255）。理由は上の行です")
        }
        Lang::En => {
            format!("tako: could not connect to {host} (ssh exit 255). The reason is printed above")
        }
    };
    let next = pane_failure_hint(lang);
    let hold = match lang {
        Lang::Ja => "tako: Enter でこのペインを閉じます",
        Lang::En => "tako: press Enter to close this pane",
    };

    match dialect {
        ShellDialect::Posix => {
            let cmd = argv
                .iter()
                .map(|a| crate::shell::quote_for_shell(a))
                .collect::<Vec<_>>()
                .join(" ");
            let cd = cd_to
                .map(|d| {
                    // リモートで `cd` するのはログイン後なので、ここでは ssh の
                    // リモートコマンドとしては渡さない（相手のシェルが不明なため）。
                    // 呼び出し側が送達経路で打ち込む。ここは記録だけ
                    format!(
                        "printf '%s\\n' {};\n",
                        crate::shell::quote_for_shell(&match lang {
                            Lang::Ja => format!("tako: 開くフォルダ: {d}"),
                            Lang::En => format!("tako: folder: {d}"),
                        })
                    )
                })
                .unwrap_or_default();
            format!(
                "printf '%s\\n' {banner};\n{cd}{cmd}\n                 __tako_code=$?;\n                 if [ \"$__tako_code\" -eq {SSH_ERROR_EXIT} ]; then\n                 printf '%s\\n%s\\n%s\\n' {failed} {next} {hold};\n                 read -r __TAKO_DUMMY__ 2>/dev/null || true;\n                 fi\n",
                banner = crate::shell::quote_for_shell(&connecting),
                failed = crate::shell::quote_for_shell(&failed),
                next = crate::shell::quote_for_shell(next),
                hold = crate::shell::quote_for_shell(hold),
            )
        }
        ShellDialect::PowerShell => {
            let cmd = argv
                .iter()
                .map(|a| ShellDialect::PowerShell.quote_arg(a))
                .collect::<Vec<_>>()
                .join(" ");
            let cd = cd_to
                .map(|d| {
                    format!(
                        "Write-Host {};\n",
                        ShellDialect::PowerShell.quote_arg(&match lang {
                            Lang::Ja => format!("tako: 開くフォルダ: {d}"),
                            Lang::En => format!("tako: folder: {d}"),
                        })
                    )
                })
                .unwrap_or_default();
            // 5.1 と 7 の両方で通る書き方だけを使う（`&&` は 5.1 に無い）
            format!(
                "Write-Host {banner};\n{cd}& {cmd};\n                 $__tako_code = $LASTEXITCODE;\n                 if ($__tako_code -eq {SSH_ERROR_EXIT}) {{\n                 Write-Host {failed};\n                 Write-Host {next};\n                 Write-Host {hold};\n                 [void][System.Console]::ReadLine();\n                 }}\n",
                banner = ShellDialect::PowerShell.quote_arg(&connecting),
                failed = ShellDialect::PowerShell.quote_arg(&failed),
                next = ShellDialect::PowerShell.quote_arg(next),
                hold = ShellDialect::PowerShell.quote_arg(hold),
            )
        }
    }
}

fn spawn_capture(bin: &str, args: &[String], stdin_text: Option<&str>) -> std::io::Result<Output> {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .stdin(if stdin_text.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Windows でコンソールウィンドウを出さない（#586 と同じ理由）
    crate::platform::process::no_console_window(&mut cmd);
    let mut child = cmd.spawn()?;
    if let Some(text) = stdin_text {
        use std::io::Write;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
    }
    let out = child.wait_with_output()?;
    Ok(Output {
        status_ok: out.status.success(),
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

struct Output {
    status_ok: bool,
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Output {
    /// 失敗の詳細に出す文言（stderr 優先、無ければ stdout の診断行）
    fn diagnosis(&self) -> String {
        let err = self.stderr.trim();
        if !err.is_empty() {
            return first_lines(err, 4);
        }
        let diag: Vec<&str> = self
            .stdout
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("Can't ") || l.starts_with("Couldn't "))
            .collect();
        if !diag.is_empty() {
            return diag.join("\n");
        }
        match self.code {
            Some(c) => format!("exit {c}"),
            None => "終了コードなし".to_string(),
        }
    }

    fn classify(&self) -> RemoteErrorKind {
        let mut text = self.stderr.clone();
        text.push('\n');
        text.push_str(&self.stdout);
        classify_output(&text)
    }
}

fn first_lines(text: &str, n: usize) -> String {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .take(n)
        .collect::<Vec<_>>()
        .join("\n")
}

/// ControlMaster が生きているか（`ssh -O check`）
pub fn master_alive(host: &str) -> bool {
    let Some(ssh) = ssh_bin() else { return false };
    let Some(cp) = control_path(host) else {
        return false;
    };
    if !cp.exists() {
        return false;
    }
    let args = vec![
        "-O".to_string(),
        "check".to_string(),
        "-o".to_string(),
        control_path_option(&cp),
        host.to_string(),
    ];
    spawn_capture(&ssh, &args, None)
        .map(|o| o.status_ok)
        .unwrap_or(false)
}

/// ControlMaster を落とす（`ssh -O exit`）。閉じるときに呼ぶ
pub fn close_master(host: &str) {
    let Some(ssh) = ssh_bin() else { return };
    let Some(cp) = control_path(host) else { return };
    if !cp.exists() {
        return;
    }
    let args = vec![
        "-O".to_string(),
        "exit".to_string(),
        "-o".to_string(),
        control_path_option(&cp),
        host.to_string(),
    ];
    let _ = spawn_capture(&ssh, &args, None);
}

/// ControlMaster を確立する（既に生きていれば何もしない）。
///
/// `BatchMode=yes` なのでパスワードを聞かれる相手では失敗する。その場合の次の一手は
/// [`RemoteErrorKind::AuthFailed`] の `next_step`（対話 SSH ペインで先にログイン）が持つ
pub fn ensure_master(host: &str) -> Result<(), RemoteError> {
    if master_alive(host) {
        return Ok(());
    }
    let Some(ssh) = ssh_bin() else {
        return Err(RemoteError::new(
            RemoteErrorKind::ClientMissing,
            host,
            "ssh",
        ));
    };
    let Some(cp) = control_path(host) else {
        return Err(RemoteError::new(
            RemoteErrorKind::Other,
            host,
            "data_dir を解決できない",
        ));
    };
    if let Some(parent) = cp.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return Err(RemoteError::new(
                RemoteErrorKind::Other,
                host,
                format!("{}: {e}", parent.display()),
            ));
        }
    }
    // 残骸ソケット（マスターが死んでいるのにファイルだけ残る）は消してから張る
    if cp.exists() {
        let _ = std::fs::remove_file(&cp);
    }
    let mut args = vec![
        "-o".to_string(),
        control_path_option(&cp),
        "-o".to_string(),
        "ControlMaster=yes".to_string(),
        "-o".to_string(),
        format!("ControlPersist={CONTROL_PERSIST_SECS}"),
        "-o".to_string(),
        format!("ConnectTimeout={CONNECT_TIMEOUT_SECS}"),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        // 対話セッションは張らない（器だけ作る）
        "-N".to_string(),
        "-f".to_string(),
    ];
    args.push(host.to_string());
    let out = spawn_capture(&ssh, &args, None).map_err(|e| {
        RemoteError::new(RemoteErrorKind::ClientMissing, host, format!("{ssh}: {e}"))
    })?;
    if !out.status_ok {
        return Err(RemoteError::new(out.classify(), host, out.diagnosis()));
    }
    Ok(())
}

/// sftp のバッチを 1 回走らせる
fn sftp_batch(host: &str, target: &str, commands: &[String]) -> Result<Output, RemoteError> {
    let Some(sftp) = sftp_bin() else {
        return Err(RemoteError::new(
            RemoteErrorKind::ClientMissing,
            target,
            "sftp",
        ));
    };
    let mut args = common_opts(host, true);
    args.push("-b".into());
    args.push("-".into());
    args.push(host.to_string());
    let script = batch_script(commands);
    spawn_capture(&sftp, &args, Some(&script)).map_err(|e| {
        RemoteError::new(
            RemoteErrorKind::ClientMissing,
            target,
            format!("{sftp}: {e}"),
        )
    })
}

/// 接続を確立し、リモートのホームディレクトリ（sftp の初期 cwd）を返す。
///
/// **フォルダを開く前にここで失敗を捕まえる**のが #919 の要点: ペインを作ってから
/// ssh に失敗させると、ペインの死とともに理由も消える
pub fn connect(host: &str) -> Result<String, RemoteError> {
    ensure_master(host)?;
    let out = sftp_batch(host, host, &["pwd".to_string()])?;
    if !out.status_ok {
        return Err(RemoteError::new(out.classify(), host, out.diagnosis()));
    }
    for line in out.stdout.lines() {
        if let Some(rest) = line.trim().strip_prefix("Remote working directory:") {
            return Ok(rest.trim().to_string());
        }
    }
    // 文言が読めなくても接続自体は成立している。ルートを返して先へ進める
    Ok("/".to_string())
}

/// ディレクトリを一覧する。symlink は実体がディレクトリかまで確かめる
pub fn list_dir(host: &str, path: &str) -> Result<Vec<RemoteEntry>, RemoteError> {
    let target = format!("{host}:{path}");
    ensure_master(host)?;
    let cmd = format!("ls -la {}", quote_sftp_arg(path));
    let out = sftp_batch(host, &target, &[cmd])?;
    if !out.status_ok {
        return Err(RemoteError::new(out.classify(), target, out.diagnosis()));
    }
    let mut entries = parse_ls_long(&out.stdout, path);

    // symlink の実体を 1 回のバッチでまとめて確かめる
    let links: Vec<String> = entries
        .iter()
        .filter(|e| e.kind == RemoteKind::Symlink)
        .take(MAX_SYMLINK_PROBES)
        .map(|e| e.path.clone())
        .collect();
    if !links.is_empty() {
        let probes: Vec<String> = links
            .iter()
            // `-` 前置 = 失敗しても後続を打ち切らない（sftp のバッチ規則）
            .map(|p| format!("-ls -1 {}", quote_sftp_arg(&format!("{p}/"))))
            .collect();
        if let Ok(probe_out) = sftp_batch(host, &target, &probes) {
            let sections = split_batch_sections(&probe_out.stdout);
            for (link, (_, body)) in links.iter().zip(sections.iter()) {
                if !probe_says_directory(body) {
                    if let Some(e) = entries.iter_mut().find(|e| &e.path == link) {
                        e.kind = RemoteKind::File;
                    }
                }
            }
        }
    }
    Ok(entries)
}

/// リモートファイルの写しを置くローカルのパス（純粋関数）。
///
/// 拡張子を保つのはプレビューの種別判定が拡張子を見るため。パスをハッシュへ落とすのは
/// リモートのディレクトリ構造をローカルへ再現しないため（深いパス・記号を持ち込まない）
pub fn local_cache_path(cache: &Path, host: &str, path: &str) -> PathBuf {
    cache
        .join(short_hash(host))
        .join(format!("{}-{}", short_hash(path), base_name(path)))
}

/// リモートファイルをローカルのキャッシュへ落とし、パスと素性を返す。
///
/// プレビューは**この実体（ローカル）**を開くので、構文色・md・画像・PDF・目次・リンクの
/// 既存スタックがそのまま効く。サイズ上限は [`MAX_PREVIEW_BYTES`]（#65 要件 3）。
///
/// 素性（mode）も返すのは、**書けないファイルを読み取り専用として見せる**ため（#966）
pub fn fetch_file(
    host: &str,
    path: &str,
    max_bytes: u64,
) -> Result<(PathBuf, Option<RemoteStat>), RemoteError> {
    let target = format!("{host}:{path}");
    ensure_master(host)?;

    // サイズを先に見る（大きいファイルを掴んでから気づくのを避ける）
    let listing = sftp_batch(host, &target, &[format!("ls -la {}", quote_sftp_arg(path))])?;
    if !listing.status_ok {
        return Err(RemoteError::new(
            listing.classify(),
            target,
            listing.diagnosis(),
        ));
    }
    let stat = parse_stat(&listing.stdout);
    if let Some(stat) = &stat {
        if kind_from_mode(&stat.mode) == Some(RemoteKind::Dir) {
            return Err(RemoteError::new(
                RemoteErrorKind::NotDirectory,
                target,
                "ディレクトリはプレビューできない",
            ));
        }
        if stat.size > max_bytes {
            return Err(RemoteError::new(
                RemoteErrorKind::TooLarge,
                target,
                format!("{} バイト > 上限 {} バイト", stat.size, max_bytes),
            ));
        }
    }

    let Some(dir) = cache_dir() else {
        return Err(RemoteError::new(
            RemoteErrorKind::Other,
            target,
            "data_dir を解決できない",
        ));
    };
    let local = local_cache_path(&dir, host, path);
    if let Some(host_dir) = local.parent() {
        if let Err(e) = std::fs::create_dir_all(host_dir) {
            return Err(RemoteError::new(
                RemoteErrorKind::Other,
                target,
                format!("{}: {e}", host_dir.display()),
            ));
        }
    }
    let cmd = format!(
        "get {} {}",
        quote_sftp_arg(path),
        quote_sftp_arg(&local.to_string_lossy())
    );
    let out = sftp_batch(host, &target, &[cmd])?;
    if !out.status_ok || !local.exists() {
        return Err(RemoteError::new(out.classify(), target, out.diagnosis()));
    }
    // **取れた中身がそのときのリモートの内容**なので、ここで競合検知の基準を進める（#966）。
    // 編集を始めるときに作るのではなく取得のたびに更新するのは、「リモートを読み直して
    // 競合を解消する」導線（もう一度 open-file する）がそのまま効くようにするため
    if let Ok(bytes) = std::fs::read(&local) {
        if let Err(e) = write_baseline(host, path, &bytes) {
            tracing::warn!("開いた時点の記録を作れない {target}: {e}");
        }
    }
    Ok((local, stat))
}

// --- 書き戻し（#966。リモートフォルダ段階 2） -------------------------------
//
// # 「キャッシュを本物と思って保存する」事故を何で置き換えたか
//
// 段階 1（#919）は編集を**構造的に禁じて**いた。本体は `remote-cache/` に落ちた
// ローカルの写しなので、止めないと「保存できた気になる」（リモートには何も書かれない）。
// 禁止を外す代わりに置いたのがこの節の 3 つ:
//
// 1. **アトミックな書き戻し**: 同じディレクトリへ一時ファイルを `put` してから
//    `rename` で被せる（OpenSSH の `posix-rename@openssh.com` は既存を上書きする =
//    Linux / Windows の実機で実測）。途中で切れても**元のファイルは壊れない**
// 2. **競合検知**: 「開いた時点の内容」を [`baseline_path`] に持ち、書く前に
//    リモートの実体と突き合わせる。**サイズと mtime ではなく内容そのもの**を見る
//    （`ls -la` の日時は分の分解能しかなく、同じ分・同じサイズの書き換えを見逃す）
// 3. **失われない失敗**: 押し出しに失敗したら [`record_pending`] で
//    「書きたかった内容」をローカルへ退避し、[`push_pending`] で再試行できる。
//    切断中の保存が無言で消えないのはこれ
//
// # mode を戻すのはなぜか
//
// `put` は元のファイルの mode を引き継がない（実測: `-rwxr-xr-x` が `-rw-r--r--` に
// なる）。`rename` で被せると**実行権が落ちる**ので、POSIX として読める mode なら
// 書き戻したあとに `chmod` で戻す。Windows の sftp-server は mode 欄が `-rw-******`
// で意味を持たない（`chmod` は成功するが効かない = 実測）ので送らない

/// リモートの 1 ファイルの素性。競合検知の材料と、書けるかの見立てに使う
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteStat {
    pub size: u64,
    /// `ls -la` の日時欄（`Aug 27 14:58` / `Aug 27 2025`）。
    /// **分の分解能しかない**ので単独では競合検知に使わない
    pub mtime: String,
    /// mode 欄（`-rw-r--r--` / Windows の `-rw-******`）
    pub mode: String,
}

impl RemoteStat {
    /// 書き込めそうか。`Some(false)` は「**どの位置にも `w` が無い** = 確実に書けない」、
    /// `Some(true)` は「どこかに `w` がある」、`None` は「mode が読めない」
    /// （Windows の `*` 埋め）。
    ///
    /// 所有者が自分かは sftp からは分からないので、`Some(true)` は
    /// **書ける保証ではない**（実際の可否は書いてみて分類する）。
    /// 使い道は「確実に書けないものを読み取り専用として見せる」側だけ
    pub fn writable_hint(&self) -> Option<bool> {
        writable_hint(&self.mode)
    }

    /// `chmod` で戻すべき 8 進数 mode（POSIX として読めるときだけ）
    pub fn octal_mode(&self) -> Option<String> {
        octal_mode(&self.mode)
    }
}

/// mode 欄から「確実に書けないか」を読む（純粋関数）。[`RemoteStat::writable_hint`] の実体
pub fn writable_hint(mode: &str) -> Option<bool> {
    let perms: Vec<char> = mode.chars().skip(1).collect();
    if perms.len() != 9 {
        return None;
    }
    // Windows の sftp-server は権限を `*` で埋める = 判定材料が無い
    if perms.contains(&'*') {
        return None;
    }
    Some(perms.contains(&'w'))
}

/// mode 欄を 8 進数へ（純粋関数）。`*` が混ざる（Windows）・長さが違うときは None
pub fn octal_mode(mode: &str) -> Option<String> {
    let perms: Vec<char> = mode.chars().skip(1).collect();
    if perms.len() != 9 || perms.contains(&'*') {
        return None;
    }
    let mut digits = String::new();
    for chunk in perms.chunks(3) {
        let mut v = 0u8;
        if chunk[0] == 'r' {
            v += 4;
        }
        if chunk[1] == 'w' {
            v += 2;
        }
        // 実行ビットは `x` に加えて setuid / setgid / sticky の小文字（`s` / `t`）でも立つ。
        // 大文字（`S` / `T`）は実行ビットが落ちている形
        if matches!(chunk[2], 'x' | 's' | 't') {
            v += 1;
        }
        digits.push((b'0' + v) as char);
    }
    Some(digits)
}

/// 書き戻し用の一時パス（純粋関数）。
///
/// **同じディレクトリ**に置くのが要点: `rename` は同一ファイルシステム内でしか
/// 成立しないので、`/tmp` へ置いてから被せることはできない。
/// 名前をドット始まりにするのは、失敗して残ってもツリーの既定表示（#550）で
/// 目に入らないようにするため
pub fn temp_remote_path(path: &str) -> String {
    let name = base_name(path);
    let tmp = format!(".{}.tako-{}.tmp", name, short_hash(path));
    match parent_remote(path) {
        Some(dir) => join_remote(&dir, &tmp),
        None => join_remote("/", &tmp),
    }
}

/// 競合の説明（純粋関数）。同じなら None。
/// **何がどう変わったか**を出す（「変わりました」だけだと次の一手を選べない）
pub fn conflict_detail(baseline: &[u8], current: &[u8]) -> Option<String> {
    if baseline == current {
        return None;
    }
    let b = baseline.len();
    let c = current.len();
    if b == c {
        Some(format!("サイズは同じ（{c} バイト）だが内容が違う"))
    } else {
        Some(format!("開いた時点 {b} バイト → 現在 {c} バイト"))
    }
}

/// 「開いた時点のリモート内容」の置き場。[`fetch_file`] が落とす作業用の写しの隣。
///
/// ファイルとして持つのは ①8MiB までの内容をメモリに抱えない ②tako を再起動しても
/// 競合検知が続けられる ③ペインを閉じても再試行できる、の 3 つのため
pub fn baseline_path(host: &str, path: &str) -> Option<PathBuf> {
    let local = local_cache_path(&cache_dir()?, host, path);
    // 拡張子を**置き換えず後ろへ足す**（`README.md` → `README.md.tako-base`）。
    // `with_extension` だと `a.b.c` と `a.b.d` が同じ名前へ落ちうる
    Some(PathBuf::from(format!("{}.tako-base", local.display())))
}

/// 開いた時点の内容を記録する（編集セッションを作るときに呼ぶ）
pub fn write_baseline(host: &str, path: &str, bytes: &[u8]) -> std::io::Result<()> {
    let Some(base) = baseline_path(host, path) else {
        return Err(std::io::Error::other("data_dir を解決できない"));
    };
    if let Some(dir) = base.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(base, bytes)
}

/// 開いた時点の内容（無ければ None = 競合を判定する材料が無い）
pub fn read_baseline(host: &str, path: &str) -> Option<Vec<u8>> {
    std::fs::read(baseline_path(host, path)?).ok()
}

/// 書き戻しの結果（応答に載せる）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveReport {
    /// 書いたバイト数
    pub bytes: u64,
    /// 一時ファイル + rename を通ったか（常に true。false になる経路は作っていない）
    pub atomic: bool,
    /// `chmod` で戻した mode（POSIX として読めなかったときは None）
    pub mode_restored: Option<String>,
    /// 書き戻した後のリモートの素性（読めたときだけ）
    pub stat: Option<RemoteStat>,
    /// 競合検知でリモートの内容を実際に取って突き合わせたか
    /// （false = 開いた時点と同じサイズ・同じ内容だと分かる材料が無く force で通した）
    pub verified: bool,
}

/// 1 ファイルの素性を読む（`ls -la <path>` 1 回）
pub fn stat_file(host: &str, path: &str) -> Result<RemoteStat, RemoteError> {
    let target = format!("{host}:{path}");
    ensure_master(host)?;
    let out = sftp_batch(host, &target, &[format!("ls -la {}", quote_sftp_arg(path))])?;
    if !out.status_ok {
        return Err(RemoteError::new(out.classify(), target, out.diagnosis()));
    }
    parse_stat(&out.stdout).ok_or_else(|| {
        RemoteError::new(
            RemoteErrorKind::Other,
            target,
            format!("ls の出力を解析できない: {}", first_lines(&out.stdout, 2)),
        )
    })
}

/// `ls -la <file>` の出力から素性を読む（純粋関数）
pub fn parse_stat(stdout: &str) -> Option<RemoteStat> {
    for line in stdout.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || is_echo_line(line) {
            continue;
        }
        if line.starts_with("Can't ") || line.starts_with("Couldn't ") {
            continue;
        }
        if let Some(row) = parse_ls_row(line) {
            if row.name == "." || row.name == ".." {
                continue;
            }
            return Some(RemoteStat {
                size: row.size,
                mtime: row.mtime,
                mode: row.mode,
            });
        }
    }
    None
}

/// リモートのファイルへ書き戻す（#966）。
///
/// 手順は 3 バッチ:
///
/// 1. `ls -la` で素性を読む（消えていれば競合、ディレクトリなら拒否、
///    上限を超える大きさに化けていれば競合）
/// 2. `get` で**現在の内容**を取り、開いた時点（[`baseline_path`]）と突き合わせる。
///    食い違えば書かずに [`RemoteErrorKind::Conflict`] を返す（`force` で上書き可）
/// 3. 一時ファイルへ `put` → `rename` で被せる → POSIX mode なら `chmod` で戻す →
///    `ls -la` で書けたことを確かめる
///
/// 成功したら開いた時点の記録を**書いた内容へ更新する**（次の保存の基準になる）。
///
/// **UI スレッドで呼ばないこと**（ネットワーク I/O。GUI は背景へ投げる）
pub fn save_file(
    host: &str,
    path: &str,
    bytes: &[u8],
    force: bool,
) -> Result<SaveReport, RemoteError> {
    let target = format!("{host}:{path}");
    ensure_master(host)?;

    // (1) 素性
    let stat = stat_file(host, path)?;
    if kind_from_mode(&stat.mode) == Some(RemoteKind::Dir) {
        return Err(RemoteError::new(
            RemoteErrorKind::NotDirectory,
            target,
            "ディレクトリへは書き戻せない",
        ));
    }

    // (2) 競合検知
    let mut verified = false;
    match read_baseline(host, path) {
        Some(baseline) => {
            if stat.size > MAX_PREVIEW_BYTES {
                // 開いた時点は上限以下だったので、上限超えは変わった証拠
                if !force {
                    return Err(RemoteError::new(
                        RemoteErrorKind::Conflict,
                        target,
                        format!(
                            "リモート側が上限を超える大きさ（{} バイト）に変わっている",
                            stat.size
                        ),
                    ));
                }
            } else {
                let current = fetch_to_temp(host, path)?;
                let read = std::fs::read(&current);
                let _ = std::fs::remove_file(&current);
                let current = read.map_err(|e| {
                    RemoteError::new(
                        RemoteErrorKind::Other,
                        target.clone(),
                        format!("取得した内容を読めない: {e}"),
                    )
                })?;
                verified = true;
                if let Some(detail) = conflict_detail(&baseline, &current) {
                    if !force {
                        return Err(RemoteError::new(RemoteErrorKind::Conflict, target, detail));
                    }
                }
            }
        }
        None if !force => {
            return Err(RemoteError::new(
                RemoteErrorKind::Conflict,
                target,
                "開いた時点の内容が分からない（競合を判定できない）",
            ));
        }
        None => {}
    }

    // (3) 一時ファイルへ put → rename で被せる
    let local = write_temp(bytes).map_err(|e| {
        RemoteError::new(
            RemoteErrorKind::Other,
            target.clone(),
            format!("一時ファイルを作れない: {e}"),
        )
    })?;
    let remote_tmp = temp_remote_path(path);
    let mode_restored = stat.octal_mode();
    let mut commands = vec![
        format!(
            "put {} {}",
            quote_sftp_arg(&local.to_string_lossy()),
            quote_sftp_arg(&remote_tmp)
        ),
        format!(
            "rename {} {}",
            quote_sftp_arg(&remote_tmp),
            quote_sftp_arg(path)
        ),
    ];
    if let Some(octal) = &mode_restored {
        commands.push(format!("chmod {octal} {}", quote_sftp_arg(path)));
    }
    commands.push(format!("ls -la {}", quote_sftp_arg(path)));
    let out = sftp_batch(host, &target, &commands);
    let _ = std::fs::remove_file(&local);
    let out = out?;
    if !out.status_ok {
        // 一時ファイルが残っていたら片付ける（`-` 前置 = 失敗しても打ち切らない）
        let _ = sftp_batch(
            host,
            &target,
            &[format!("-rm {}", quote_sftp_arg(&remote_tmp))],
        );
        return Err(RemoteError::new(out.classify(), target, out.diagnosis()));
    }

    // 次の保存の基準を「いま書いた内容」へ進める
    if let Err(e) = write_baseline(host, path, bytes) {
        // ここで失敗しても書き戻し自体は成立している。次の保存が
        // 「開いた時点が分からない」になるだけなので、握り潰さず detail へ出す
        tracing::warn!("開いた時点の記録を更新できない {target}: {e}");
    }
    Ok(SaveReport {
        bytes: bytes.len() as u64,
        atomic: true,
        mode_restored,
        stat: parse_stat(&out.stdout),
        verified,
    })
}

/// リモートの現在の内容を一時ファイルへ落とす（競合検知用。キャッシュは汚さない）
fn fetch_to_temp(host: &str, path: &str) -> Result<PathBuf, RemoteError> {
    let target = format!("{host}:{path}");
    let dir = temp_dir_for_writes().map_err(|e| {
        RemoteError::new(
            RemoteErrorKind::Other,
            target.clone(),
            format!("一時ディレクトリを作れない: {e}"),
        )
    })?;
    let local = dir.join(format!("cur-{}", temp_token()));
    let _ = std::fs::remove_file(&local);
    let out = sftp_batch(
        host,
        &target,
        &[format!(
            "get {} {}",
            quote_sftp_arg(path),
            quote_sftp_arg(&local.to_string_lossy())
        )],
    )?;
    if !out.status_ok || !local.exists() {
        return Err(RemoteError::new(out.classify(), target, out.diagnosis()));
    }
    Ok(local)
}

/// 書き戻す内容を置く一時ファイル
fn write_temp(bytes: &[u8]) -> std::io::Result<PathBuf> {
    let dir = temp_dir_for_writes()?;
    let local = dir.join(format!("put-{}", temp_token()));
    std::fs::write(&local, bytes)?;
    Ok(local)
}

/// プロセス内で一意な一時ファイル名。
///
/// **内容やパスから作ってはいけない**: 2 枚のペインが同時に保存すると同じ名前へ
/// 落ちて、片方の内容がもう片方のファイルへ書かれる（同じ大きさのファイルで踏む）
fn temp_token() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

fn temp_dir_for_writes() -> std::io::Result<PathBuf> {
    let dir = match cache_dir() {
        Some(d) => d.join("tmp"),
        None => std::env::temp_dir().join("tako-remote-tmp"),
    };
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

// --- 送れなかった保存の退避と再試行（#966 受け入れ条件 3） -------------------

/// 押し出せなかった保存 1 件。**内容そのものは隣の `.body` に置く**（JSON に
/// 埋めると 8MiB のテキストが 1 行になり、壊れたときに手で救えない）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PendingWrite {
    pub host: String,
    pub path: String,
    /// 最後の失敗の種別（[`RemoteErrorKind::as_str`]）
    #[serde(default)]
    pub kind: String,
    /// 最後の失敗理由（要約 + 次の一手）
    #[serde(default)]
    pub error: String,
    /// 記録した時刻（UNIX 秒）
    #[serde(default)]
    pub at: u64,
    /// 試行回数
    #[serde(default)]
    pub attempts: u32,
    /// 書きたい内容のバイト数
    #[serde(default)]
    pub size: u64,
}

impl PendingWrite {
    pub fn label(&self) -> String {
        format!("{}:{}", self.host, self.path)
    }
}

/// 退避の置き場（`<data_dir>/remote-cache/pending/`）
pub fn pending_dir() -> Option<PathBuf> {
    Some(cache_dir()?.join("pending"))
}

/// 退避 1 件の識別子（host + path で決まる = 同じファイルを何度保存しても増えない）
pub fn pending_id(host: &str, path: &str) -> String {
    format!("{}-{}", short_hash(host), short_hash(path))
}

/// 押し出せなかった保存を退避する。**ここが「無言で消えない」の実体**
pub fn record_pending(
    host: &str,
    path: &str,
    bytes: &[u8],
    error: &RemoteError,
) -> std::io::Result<()> {
    let Some(dir) = pending_dir() else {
        return Err(std::io::Error::other("data_dir を解決できない"));
    };
    std::fs::create_dir_all(&dir)?;
    let id = pending_id(host, path);
    std::fs::write(dir.join(format!("{id}.body")), bytes)?;
    let attempts = read_pending(&dir, &id).map(|p| p.attempts).unwrap_or(0) + 1;
    let entry = PendingWrite {
        host: host.to_string(),
        path: path.to_string(),
        kind: error.kind.as_str().to_string(),
        error: format!("{} / {}", error.summary(), error.next_step()),
        at: unix_secs(),
        attempts,
        size: bytes.len() as u64,
    };
    let json = serde_json::to_vec_pretty(&entry).map_err(std::io::Error::other)?;
    std::fs::write(dir.join(format!("{id}.json")), json)
}

/// 退避を捨てる（押し出せた・ユーザーが要らないと言った）
pub fn clear_pending(host: &str, path: &str) {
    let Some(dir) = pending_dir() else { return };
    let id = pending_id(host, path);
    let _ = std::fs::remove_file(dir.join(format!("{id}.json")));
    let _ = std::fs::remove_file(dir.join(format!("{id}.body")));
}

/// 退避の一覧（新しいものが先）。**読めない断片は落とさず error として出す**
pub fn list_pending() -> Vec<PendingWrite> {
    let Some(dir) = pending_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<PendingWrite> = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Some(id) = p.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        match read_pending(&dir, id) {
            Some(entry) => out.push(entry),
            None => out.push(PendingWrite {
                host: String::new(),
                path: p.display().to_string(),
                kind: RemoteErrorKind::Other.as_str().to_string(),
                error: "退避の記録を読めない（内容は隣の .body に残っている）".into(),
                at: 0,
                attempts: 0,
                size: 0,
            }),
        }
    }
    out.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.path.cmp(&b.path)));
    out
}

/// この位置に押し出せていない保存が残っているか（応答へ載せる安価な確認）
pub fn has_pending(host: &str, path: &str) -> bool {
    pending_dir()
        .map(|d| d.join(format!("{}.body", pending_id(host, path))).exists())
        .unwrap_or(false)
}

/// 退避された内容（再試行に使う）
pub fn pending_body(host: &str, path: &str) -> Option<Vec<u8>> {
    let dir = pending_dir()?;
    std::fs::read(dir.join(format!("{}.body", pending_id(host, path)))).ok()
}

fn read_pending(dir: &Path, id: &str) -> Option<PendingWrite> {
    let text = std::fs::read_to_string(dir.join(format!("{id}.json"))).ok()?;
    serde_json::from_str(&text).ok()
}

/// 退避 1 件を押し出す（再試行）。押し出せたら退避を捨てる
pub fn push_pending(host: &str, path: &str, force: bool) -> Result<SaveReport, RemoteError> {
    let target = format!("{host}:{path}");
    let Some(bytes) = pending_body(host, path) else {
        return Err(RemoteError::new(
            RemoteErrorKind::NotFound,
            target,
            "退避された内容が無い（すでに押し出したか、退避を捨てた）",
        ));
    };
    match save_file(host, path, &bytes, force) {
        Ok(report) => {
            clear_pending(host, path);
            Ok(report)
        }
        Err(e) => {
            // 失敗しても退避は残す（試行回数と理由だけ進める）
            let _ = record_pending(host, path, &bytes, &e);
            Err(e)
        }
    }
}

fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// キャッシュに落ちたファイルか（プレビューの編集を止める判定に使う）
pub fn is_cached_remote(path: &Path) -> bool {
    match cache_dir() {
        Some(dir) => path.starts_with(dir),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 引用符で包むと空白と_glob_が_literal_になる() {
        assert_eq!(quote_sftp_arg("/home/a b"), "\"/home/a b\"");
        assert_eq!(quote_sftp_arg("/tmp/*.rs"), "\"/tmp/*.rs\"");
        // 引用符とバックスラッシュだけ逃がす
        assert_eq!(quote_sftp_arg("/a\"b"), "\"/a\\\"b\"");
        assert_eq!(quote_sftp_arg("/a\\b"), "\"/a\\\\b\"");
    }

    #[test]
    fn パス操作() {
        assert_eq!(join_remote("/home/x", "a.rs"), "/home/x/a.rs");
        assert_eq!(join_remote("/", "a.rs"), "/a.rs");
        assert_eq!(parent_remote("/home/x/a"), Some("/home/x".into()));
        assert_eq!(parent_remote("/home"), Some("/".into()));
        assert_eq!(parent_remote("/"), None);
        assert_eq!(base_name("/home/x/a.rs"), "a.rs");
        assert_eq!(base_name("/"), "/");
        assert_eq!(base_name("/home/x/"), "x");
    }

    #[test]
    fn windows_のドライブパスはシェル用に先頭スラッシュを落とす() {
        // sftp-server が見せる形 → PowerShell が受け取れる形
        assert_eq!(shell_path("/C:/Users/user/dev"), "C:/Users/user/dev");
        assert_eq!(shell_path("/C:"), "C:");
        // POSIX パスは触らない
        assert_eq!(shell_path("/home/user"), "/home/user");
        // 1 文字ディレクトリを誤爆しない
        assert_eq!(shell_path("/C/Users"), "/C/Users");
    }

    /// 実測した Linux（OpenSSH sftp）の `ls -la` 出力
    const LINUX_LS: &str = "\
sftp> ls -la \"/home/user\"
drwxr-xr-x   11 user      user          4096 May 28 12:50 .
drwxr-xr-x    3 root     root         4096 Apr 29 09:58 ..
-rw-------    1 user      user          1673 May 27 10:42 .bash_history
-rw-r--r--    1 user      user           220 Jan  7  2022 .bash_logout
drwxrwxr-x    3 user      user          4096 May 20 09:39 .local
lrwxrwxrwx    1 root     root            7 Apr 25 16:48 link-to-bin
sftp> bye
";

    /// 実測した Windows（OpenSSH sftp-server）の `ls -la` 出力。
    /// owner / group が `-`、mode の権限部が `*`、**名前に空白**が入る
    const WINDOWS_LS: &str = "\
sftp> ls -la \"/C:/Users/user\"
drwx******    1 -        -               0 Aug  5  2025 AppData
drwx******    1 -        -               0 Aug  5  2025 Application Data
-rw-******    1 -        -        12845056 Aug 22 03:43 NTUSER.DAT
drwx******    1 -        -           49152 Aug 23 22:34 dev
";

    #[test]
    fn linux_の_ls_出力を解析する() {
        let entries = parse_ls_long(LINUX_LS, "/home/user");
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        // `.` `..` と `sftp>` エコーは落ちる。ディレクトリ・symlink が先
        assert_eq!(
            names,
            vec![".local", "link-to-bin", ".bash_history", ".bash_logout"]
        );
        let local = entries.iter().find(|e| e.name == ".local").unwrap();
        assert_eq!(local.kind, RemoteKind::Dir);
        assert_eq!(local.path, "/home/user/.local");
        let link = entries.iter().find(|e| e.name == "link-to-bin").unwrap();
        assert_eq!(link.kind, RemoteKind::Symlink);
        let hist = entries.iter().find(|e| e.name == ".bash_history").unwrap();
        assert_eq!(hist.kind, RemoteKind::File);
        assert_eq!(hist.size, 1673);
    }

    #[test]
    fn windows_の_ls_出力は名前の空白を落とさない() {
        let entries = parse_ls_long(WINDOWS_LS, "/C:/Users/user");
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["AppData", "Application Data", "dev", "NTUSER.DAT"]
        );
        let spaced = entries
            .iter()
            .find(|e| e.name == "Application Data")
            .unwrap();
        assert_eq!(spaced.kind, RemoteKind::Dir);
        assert_eq!(spaced.path, "/C:/Users/user/Application Data");
        let dat = entries.iter().find(|e| e.name == "NTUSER.DAT").unwrap();
        assert_eq!(dat.size, 12_845_056);
    }

    #[test]
    fn ls_をファイルに掛けるとフルパスが返るので末尾要素を名前にする() {
        // 実測: `ls -l /etc/hostname` → nlink が `?`、名前はフルパス
        let out = "sftp> ls -la \"/etc/hostname\"\n\
                   -rw-r--r--    ? 0        0              15 Apr 29 09:58 /etc/hostname\n";
        let entries = parse_ls_long(out, "");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "hostname");
        assert_eq!(entries[0].size, 15);
        assert_eq!(entries[0].kind, RemoteKind::File);
    }

    #[test]
    fn 診断行はエントリにしない() {
        let out = "sftp> ls -la \"/no/such\"\nCan't ls: \"/no/such\" not found\n";
        assert!(parse_ls_long(out, "/no/such").is_empty());
    }

    #[test]
    fn mode_に見えない行も名前として拾う() {
        // サーバー実装差で longname の形が違っても黙って捨てない
        let out = "sftp> ls -la \"/x\"\nweird-entry\n";
        let entries = parse_ls_long(out, "/x");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "weird-entry");
        assert_eq!(entries[0].kind, RemoteKind::Unknown);
    }

    #[test]
    fn 実測した失敗文言を分類する() {
        // すべて OpenSSH 10.2 の実出力（#919 の before 実測より）
        assert_eq!(
            classify_output(
                "ssh: Could not resolve hostname nope.invalid: nodename nor servname provided, or not known"
            ),
            RemoteErrorKind::HostUnresolved
        );
        assert_eq!(
            classify_output("ssh: connect to host 10.0.0.1 port 22: Operation timed out"),
            RemoteErrorKind::Timeout
        );
        assert_eq!(
            classify_output("ssh: connect to host localhost port 22: Connection refused"),
            RemoteErrorKind::Refused
        );
        assert_eq!(
            classify_output(
                "user@host: Permission denied (publickey,password,keyboard-interactive)."
            ),
            RemoteErrorKind::AuthFailed
        );
        assert_eq!(
            classify_output("Host key verification failed."),
            RemoteErrorKind::HostKeyMismatch
        );
        assert_eq!(
            classify_output("subsystem request failed on channel 0"),
            RemoteErrorKind::SftpUnavailable
        );
        assert_eq!(
            classify_output("Can't ls: \"/no/such\" not found"),
            RemoteErrorKind::NotFound
        );
        assert_eq!(
            classify_output("ssh: connect to host h port 22: No route to host"),
            RemoteErrorKind::Unreachable
        );
    }

    #[test]
    fn 認証失敗とパス権限を取り違えない() {
        // 括弧つきの方式リスト = 認証
        assert_eq!(
            classify_output("Permission denied (publickey)."),
            RemoteErrorKind::AuthFailed
        );
        // パス側の権限は PermissionDenied
        assert_eq!(
            classify_output("Couldn't stat remote file: Permission denied"),
            RemoteErrorKind::PermissionDenied
        );
    }

    #[test]
    fn 失敗の報告は常に理由と次の一手を持つ() {
        // 静かな失敗を型で禁じる: どの種別でも空にならない
        for kind in [
            RemoteErrorKind::ClientMissing,
            RemoteErrorKind::HostUnresolved,
            RemoteErrorKind::Unreachable,
            RemoteErrorKind::Refused,
            RemoteErrorKind::Timeout,
            RemoteErrorKind::AuthFailed,
            RemoteErrorKind::HostKeyMismatch,
            RemoteErrorKind::SftpUnavailable,
            RemoteErrorKind::NotFound,
            RemoteErrorKind::PermissionDenied,
            RemoteErrorKind::NotDirectory,
            RemoteErrorKind::TooLarge,
            RemoteErrorKind::Other,
        ] {
            for lang in [Lang::Ja, Lang::En] {
                let err = RemoteError::new(kind, "win:/tmp", "detail here");
                let report = err.report_in(lang);
                assert!(
                    report.lines().count() >= 3,
                    "{kind:?}/{lang:?} の報告が 3 行未満: {report}"
                );
                assert!(!kind.summary_note().text_in(lang).is_empty());
                assert!(!kind.next_step_note().text_in(lang).is_empty());
                assert!(report.contains("win:/tmp"));
                assert!(report.contains("detail here"));
                assert!(!kind.as_str().is_empty());
            }
        }
    }

    #[test]
    fn バッチ本文は必ず_bye_で終わる() {
        let script = batch_script(&["pwd".into(), "ls -la \"/x\"".into()]);
        assert_eq!(script, "pwd\nls -la \"/x\"\nbye\n");
    }

    #[test]
    fn バッチ出力をコマンド単位に切り分ける() {
        let out =
            "sftp> -ls -1 \"/a/\"\n/a/x\n/a/y\nsftp> -ls -1 \"/b/\"\nCan't ls: \"/b/\" not found\n";
        let sections = split_batch_sections(out);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].0, "-ls -1 \"/a/\"");
        assert!(probe_says_directory(&sections[0].1));
        assert!(!probe_says_directory(&sections[1].1));
    }

    #[test]
    fn 中身が空のディレクトリも開けると判定する() {
        assert!(probe_says_directory(""));
        assert!(probe_says_directory("sftp> -ls -1 \"/empty/\"\n"));
    }

    #[test]
    fn ssh_ペインの_argv_はツリーと同じ_controlpath_を通る() {
        // 追加認証なしの共有（#65 / #919 要件 6）はここが一致していることが前提
        let argv = ssh_pane_argv("win", &["-t".to_string()]);
        assert_eq!(argv.last().unwrap(), "win");
        let joined = argv.join(" ");
        assert!(joined.contains("ControlMaster=auto"), "{joined}");
        assert!(joined.contains("ControlPersist="), "{joined}");
        assert!(
            joined.contains(&format!("ConnectTimeout={CONNECT_TIMEOUT_SECS}")),
            "{joined}"
        );
        assert!(joined.contains("-t"), "{joined}");
        if let Some(cp) = control_path("win") {
            assert!(joined.contains(&cp.display().to_string()), "{joined}");
        }
        // 対話ペインは BatchMode を立てない（パスワードを聞けなくなる）
        assert!(!joined.contains("BatchMode"), "{joined}");
    }

    #[test]
    fn ssh_ペインのスクリプトは接続前にバナーを出し失敗時だけ残る() {
        use crate::platform::shell_dialect::ShellDialect;
        let argv = vec!["/usr/bin/ssh".to_string(), "win".to_string()];
        for lang in [Lang::Ja, Lang::En] {
            let posix = ssh_pane_script(ShellDialect::Posix, &argv, "win", None, lang);
            // 接続前のバナー（旧実装は完全な空画面だった。#919 の実測）
            assert!(posix.contains("printf"), "{posix}");
            assert!(posix.contains("win"), "{posix}");
            // ssh 自身の失敗（255）だけ拾う = リモートの `exit 1` を誤報しない
            assert!(posix.contains(&format!("-eq {SSH_ERROR_EXIT}")), "{posix}");
            // 失敗時は入力待ちで止まる = 理由が画面に残る
            assert!(posix.contains("read -r"), "{posix}");
            assert!(posix.contains(pane_failure_hint(lang)), "{posix}");

            let ps = ssh_pane_script(ShellDialect::PowerShell, &argv, "win", None, lang);
            assert!(ps.contains("Write-Host"), "{ps}");
            assert!(ps.contains("$LASTEXITCODE"), "{ps}");
            assert!(ps.contains(&format!("-eq {SSH_ERROR_EXIT}")), "{ps}");
            assert!(ps.contains("ReadLine"), "{ps}");
            // 5.1 に無い `&&` は使わない（`default_shell()` が 5.1 へ落ちうる）
            assert!(!ps.contains("&&"), "{ps}");
        }
    }

    #[test]
    fn ssh_ペインのスクリプトは開くフォルダを見せる() {
        use crate::platform::shell_dialect::ShellDialect;
        let argv = vec!["ssh".to_string(), "win".to_string()];
        let with = ssh_pane_script(
            ShellDialect::Posix,
            &argv,
            "win",
            Some("/srv/app"),
            Lang::Ja,
        );
        assert!(with.contains("/srv/app"), "{with}");
        let without = ssh_pane_script(ShellDialect::Posix, &argv, "win", None, Lang::Ja);
        assert!(!without.contains("/srv/app"), "{without}");
    }

    #[test]
    fn ssh_ペインのスクリプトは引数を引用する() {
        use crate::platform::shell_dialect::ShellDialect;
        // 空白入りのパス（macOS の既定 data_dir 由来の ControlPath）が語割れしない
        let argv = vec![
            "/usr/bin/ssh".to_string(),
            "-o".to_string(),
            "ControlPath=\"/a b/c.sock\"".to_string(),
            "win".to_string(),
        ];
        let posix = ssh_pane_script(ShellDialect::Posix, &argv, "win", None, Lang::Ja);
        // 単引用符で 1 語に括られている（`/a b/c.sock` が 2 語へ割れない）
        assert!(posix.contains("'-o'") || posix.contains("-o"), "{posix}");
        assert!(posix.contains("'ControlPath=\"/a b/c.sock\"'"), "{posix}");
    }

    #[test]
    fn ホストごとに_controlpath_が分かれる() {
        let a = control_path("alpha");
        let b = control_path("beta");
        assert_ne!(a, b);
        // ソケットのパス長（macOS は約 104 バイト）に収まる短さ
        if let Some(p) = a {
            assert!(
                p.display().to_string().len() <= MAX_SOCKET_PATH,
                "{}",
                p.display()
            );
            assert_eq!(short_hash("alpha").len(), 16);
        }
    }

    #[test]
    fn 空白を含む_data_dir_でも_controlpath_が壊れない() {
        // macOS の既定 data_dir は `Application Support` を含む。素のまま `-o` へ渡すと
        // OpenSSH が `keyword controlpath extra arguments at end of line` で全操作を
        // 失敗させる（実測。#833 と同型の罠）
        let dir = PathBuf::from("/Users/u/Library/Application Support/tako");
        let cp = control_path_in(Some(&dir), "win");
        assert!(cp.to_string_lossy().contains(' '), "{}", cp.display());
        let opt = control_path_option(&cp);
        assert!(opt.starts_with("ControlPath=\""), "{opt}");
        assert!(opt.ends_with('"'), "{opt}");
        // 引用符の中に収まっている = 空白で切られない
        let inner = opt
            .trim_start_matches("ControlPath=\"")
            .trim_end_matches('"');
        assert_eq!(inner, cp.display().to_string());
    }

    #[test]
    fn 長すぎる_data_dir_では短い置き場へ逃がす() {
        // unix domain socket のパス長上限を割ると bind に失敗して接続できない
        let long = PathBuf::from(format!("/tmp/{}", "x".repeat(120)));
        let cp = control_path_in(Some(&long), "win");
        assert!(!cp.starts_with(&long), "{}", cp.display());
        assert!(
            cp.file_name()
                .map(|n| n.to_string_lossy().starts_with("tako-ssh-"))
                .unwrap_or(false),
            "{}",
            cp.display()
        );
        // data_dir が解決できないときも同じ逃げ場
        assert_eq!(control_path_in(None, "win"), cp);
    }

    #[test]
    fn 全ての_ssh_呼び出しが引用済みの_controlpath_を使う() {
        // 番犬: `ControlPath=` を自前で組み立てる箇所が増えたら落とす。
        // 1 箇所でも素のままだと空白入り data_dir の環境で全操作が無言で失敗する
        let src = include_str!("remote_fs.rs");
        let raw = src.matches("format!(\"ControlPath=").count();
        assert_eq!(
            raw, 1,
            "ControlPath の組み立ては control_path_option 1 箇所に集約する（見つかった箇所: {raw}）"
        );
    }

    #[test]
    fn エントリ数は上限で打ち切る() {
        let mut out = String::from("sftp> ls -la \"/big\"\n");
        for i in 0..(MAX_ENTRIES + 50) {
            out.push_str(&format!(
                "-rw-r--r--    1 u        g               1 Jan  1  2020 f{i}\n"
            ));
        }
        assert_eq!(parse_ls_long(&out, "/big").len(), MAX_ENTRIES);
    }

    // --- 書き戻し（#966） ---------------------------------------------------

    #[test]
    fn 種別の全列挙に漏れがない() {
        // 網羅 match: 種別を足すとここがコンパイルエラーになるので、
        // ALL への追加を忘れたまま素通りできない（#966 で Conflict を足したときの学び）
        for kind in RemoteErrorKind::ALL {
            let tag = match kind {
                RemoteErrorKind::ClientMissing => "client_missing",
                RemoteErrorKind::HostUnresolved => "host_unresolved",
                RemoteErrorKind::Unreachable => "unreachable",
                RemoteErrorKind::Refused => "refused",
                RemoteErrorKind::Timeout => "timeout",
                RemoteErrorKind::AuthFailed => "auth_failed",
                RemoteErrorKind::HostKeyMismatch => "host_key_mismatch",
                RemoteErrorKind::SftpUnavailable => "sftp_unavailable",
                RemoteErrorKind::NotFound => "not_found",
                RemoteErrorKind::PermissionDenied => "permission_denied",
                RemoteErrorKind::NotDirectory => "not_directory",
                RemoteErrorKind::TooLarge => "too_large",
                RemoteErrorKind::Conflict => "conflict",
                RemoteErrorKind::Other => "other",
            };
            assert_eq!(tag, kind.as_str(), "as_str と全列挙が食い違う: {kind:?}");
        }
        assert_eq!(
            RemoteErrorKind::ALL.len(),
            14,
            "種別を足したら ALL にも足す（網羅テストが回らなくなる）"
        );
    }

    #[test]
    fn 素性は_ls_の_mode_と日時とサイズを拾う() {
        // Linux（実測の形）
        let stat = parse_stat(
            "sftp> ls -la \"/tmp/t/target.txt\"\n\
             -rwxr-xr-x    1 user     user           16 Aug 27 14:57 /tmp/t/target.txt\n",
        )
        .expect("解析できる");
        assert_eq!(stat.size, 16);
        assert_eq!(stat.mode, "-rwxr-xr-x");
        assert_eq!(stat.mtime, "Aug 27 14:57");
        // Windows（実測の形。owner / group が `-`、権限が `*` 埋め）
        let win = parse_stat(
            "sftp> ls -la \"/C:/Users/u/t/target.txt\"\n\
             -rw-******    1 -        -              22 Aug 27 14:58 target.txt\n",
        )
        .expect("解析できる");
        assert_eq!(win.size, 22);
        assert_eq!(win.mode, "-rw-******");
        // 診断行だけなら None（黙って 0 バイトの素性を作らない）
        assert!(parse_stat("sftp> ls -la \"/nope\"\nCan't ls: \"/nope\" not found\n").is_none());
    }

    #[test]
    fn 書けないファイルだけを読み取り専用と見なす() {
        // どの位置にも w が無い = 確実に書けない
        assert_eq!(writable_hint("-r--r--r--"), Some(false));
        assert_eq!(writable_hint("-rw-r--r--"), Some(true));
        assert_eq!(writable_hint("----------"), Some(false));
        // Windows の `*` 埋めは判定材料が無い（読み取り専用にはしない）
        assert_eq!(writable_hint("-rw-******"), None);
        assert_eq!(writable_hint("*********"), None);
        // 形が違うものも None
        assert_eq!(writable_hint("-rw-"), None);
    }

    #[test]
    fn mode_を_8_進数へ戻す() {
        assert_eq!(octal_mode("-rwxr-xr-x").as_deref(), Some("755"));
        assert_eq!(octal_mode("-rw-r--r--").as_deref(), Some("644"));
        assert_eq!(octal_mode("-rw-------").as_deref(), Some("600"));
        // setuid / sticky の小文字は実行ビットが立っている形
        assert_eq!(octal_mode("-rwsr-xr-x").as_deref(), Some("755"));
        // 大文字は実行ビットが落ちている
        assert_eq!(octal_mode("-rwSr-xr-x").as_deref(), Some("655"));
        // Windows は送らない（chmod は成功するが効かない = 実測）
        assert_eq!(octal_mode("-rw-******"), None);
    }

    #[test]
    fn 書き戻しの一時パスは同じディレクトリのドット始まり() {
        // rename は同一ファイルシステム内でしか成立しないので、必ず同じディレクトリ
        let tmp = temp_remote_path("/srv/app/README.md");
        assert!(
            tmp.starts_with("/srv/app/."),
            "同じディレクトリのドット始まりでない: {tmp}"
        );
        assert!(tmp.ends_with(".tmp"), "{tmp}");
        assert!(tmp.contains("README.md"), "元の名前が分かる: {tmp}");
        // ルート直下でも壊れない
        assert!(temp_remote_path("/a.txt").starts_with("/."));
        // Windows のドライブ表記
        let win = temp_remote_path("/C:/Users/u/dev/a.txt");
        assert!(win.starts_with("/C:/Users/u/dev/."), "{win}");
        // パスごとに違う（別ファイルの保存が同じ一時ファイルを踏まない）
        assert_ne!(
            temp_remote_path("/a/README.md"),
            temp_remote_path("/b/README.md")
        );
    }

    #[test]
    fn 競合は何がどう変わったかを言う() {
        assert_eq!(conflict_detail(b"same", b"same"), None);
        // サイズが同じで内容が違う = mtime とサイズだけでは見逃す形
        let same_size = conflict_detail(b"abcd", b"abce").expect("競合");
        assert!(same_size.contains("サイズは同じ"), "{same_size}");
        let grown = conflict_detail(b"ab", b"abcd").expect("競合");
        assert!(grown.contains("2") && grown.contains("4"), "{grown}");
    }

    #[test]
    fn 開いた時点の記録は作業用の写しと別のファイルへ置く() {
        let cache = std::path::Path::new("/cache");
        let local = local_cache_path(cache, "win", "/C:/Users/u/a.md");
        // 拡張子は保つ（プレビューの種別判定が見る）
        assert!(local.to_string_lossy().ends_with("-a.md"), "{local:?}");
        // 別のパスは別の写しへ（同名ファイルがぶつからない）
        assert_ne!(local, local_cache_path(cache, "win", "/C:/Users/u/b/a.md"));
        // 別のホストも分かれる
        assert_ne!(local, local_cache_path(cache, "other", "/C:/Users/u/a.md"));
    }

    #[test]
    fn 退避の識別子はホストとパスで決まる() {
        // 同じファイルを何度保存しても退避が増えない
        assert_eq!(pending_id("win", "/a/b.md"), pending_id("win", "/a/b.md"));
        assert_ne!(pending_id("win", "/a/b.md"), pending_id("win", "/a/c.md"));
        assert_ne!(pending_id("win", "/a/b.md"), pending_id("lin", "/a/b.md"));
    }

    #[test]
    fn 書き戻しの_sftp_バッチは一時ファイルへ入れて_rename_で被せる() {
        // 直接 put で被せると、途中で切れたときに**元のファイルが壊れる**。
        // 組み立ての順序をここで固定する（put → rename → chmod → 確認）
        let path = "/srv/app/run.sh";
        let tmp = temp_remote_path(path);
        let commands = vec![
            format!(
                "put {} {}",
                quote_sftp_arg("/local/tmp"),
                quote_sftp_arg(&tmp)
            ),
            format!("rename {} {}", quote_sftp_arg(&tmp), quote_sftp_arg(path)),
            format!("chmod 755 {}", quote_sftp_arg(path)),
            format!("ls -la {}", quote_sftp_arg(path)),
        ];
        let script = batch_script(&commands);
        let put = script.find("put ").expect("put がある");
        let rename = script.find("rename ").expect("rename がある");
        let chmod = script.find("chmod ").expect("chmod がある");
        assert!(put < rename && rename < chmod, "順序が違う:\n{script}");
        // 対象を直接開く `put ... "/srv/app/run.sh"` が無いこと（= 非アトミックな上書き）
        assert!(
            !script.contains(&format!(
                "put {} {}",
                quote_sftp_arg("/local/tmp"),
                quote_sftp_arg(path)
            )),
            "対象を直接 put している:\n{script}"
        );
        assert!(script.ends_with("bye\n"));
    }

    #[test]
    fn 一時ファイル名はプロセス内で一意() {
        // 内容やパスから作ると、同じ大きさの 2 ファイルを同時に保存したときに
        // 片方の内容がもう片方へ書かれる
        let a = temp_token();
        let b = temp_token();
        assert_ne!(a, b, "同じ名前が 2 回出た: {a}");
        assert!(a.starts_with(&format!("{}-", std::process::id())), "{a}");
    }
}
