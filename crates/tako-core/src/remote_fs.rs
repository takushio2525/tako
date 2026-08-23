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
    /// 応答が読めない・想定外の失敗
    Other,
}

impl RemoteErrorKind {
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
        let mut fields = line.split_whitespace();
        let mode = fields.next().unwrap_or_default();
        let Some(kind) = kind_from_mode(mode) else {
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
        // mode の後ろ 7 列（nlink owner group size month day time）を飛ばして名前へ。
        // `?` が入ることもある（`ls -l <file>` の nlink など）ので数値を仮定しない。
        // 列の区切りを自前で辿るのは、**名前に空白が入る**（Windows の
        // `Application Data`）ので `splitn` では最後の列を切り出せないため
        let mut size = 0u64;
        let mut skipped = 0usize;
        let mut rest_start = None;
        let mut cursor = mode.len();
        let bytes = line.as_bytes();
        while skipped < 7 && cursor < bytes.len() {
            // 空白を飛ばす
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
            // 4 列目（mode から数えて 4 番目）が size
            if skipped == 3 {
                size = line[start..cursor].parse::<u64>().unwrap_or(0);
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
        let Some(start) = rest_start else { continue };
        let name = line[start..].to_string();
        if name.is_empty() || name == "." || name == ".." {
            continue;
        }
        // `ls -l <file>` は名前ではなくフルパスを返す（実測）。その場合は末尾要素を名前にする
        let name = if name.contains('/') {
            base_name(&name)
        } else {
            name
        };
        out.push(RemoteEntry {
            path: join_remote(dir, &name),
            name,
            kind,
            size,
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

/// リモートファイルをローカルのキャッシュへ落とし、そのパスを返す。
///
/// プレビューは**この実体（ローカル）**を開くので、構文色・md・画像・PDF・目次・リンクの
/// 既存スタックがそのまま効く。サイズ上限は [`MAX_PREVIEW_BYTES`]（#65 要件 3）
pub fn fetch_file(host: &str, path: &str, max_bytes: u64) -> Result<PathBuf, RemoteError> {
    let target = format!("{host}:{path}");
    ensure_master(host)?;

    // サイズを先に見る（大きいファイルを掴んでから気づくのを避ける）
    let stat = sftp_batch(host, &target, &[format!("ls -la {}", quote_sftp_arg(path))])?;
    if !stat.status_ok {
        return Err(RemoteError::new(stat.classify(), target, stat.diagnosis()));
    }
    let listed = parse_ls_long(&stat.stdout, "");
    if let Some(entry) = listed.first() {
        if entry.kind == RemoteKind::Dir {
            return Err(RemoteError::new(
                RemoteErrorKind::NotDirectory,
                target,
                "ディレクトリはプレビューできない",
            ));
        }
        if entry.size > max_bytes {
            return Err(RemoteError::new(
                RemoteErrorKind::TooLarge,
                target,
                format!("{} バイト > 上限 {} バイト", entry.size, max_bytes),
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
    let host_dir = dir.join(short_hash(host));
    if let Err(e) = std::fs::create_dir_all(&host_dir) {
        return Err(RemoteError::new(
            RemoteErrorKind::Other,
            target,
            format!("{}: {e}", host_dir.display()),
        ));
    }
    // 拡張子を保つ（プレビューの種別判定が拡張子を見るため）
    let local = host_dir.join(format!("{}-{}", short_hash(path), base_name(path)));
    let cmd = format!(
        "get {} {}",
        quote_sftp_arg(path),
        quote_sftp_arg(&local.to_string_lossy())
    );
    let out = sftp_batch(host, &target, &[cmd])?;
    if !out.status_ok || !local.exists() {
        return Err(RemoteError::new(out.classify(), target, out.diagnosis()));
    }
    Ok(local)
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
}
