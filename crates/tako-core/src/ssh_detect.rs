//! ペインで走っている `ssh` の宛先を読み取る純粋ロジック（Issue #976 / #65 要件 1）。
//!
//! ユーザーがペインで `ssh <host>` に入ったら、明示的な「リモートからフォルダを開く」
//! 操作なしにツリーへリモートフォルダを出したい。そのための **「コマンド行 → 宛先」**
//! の判断をここに閉じ込める。
//!
//! # ここに置くもの / 置かないもの
//!
//! - 置く: コマンド行の解釈（宛先の抽出・見送り理由の判定）。すべて純関数なので
//!   **両プラットフォームぶんを macOS からテストできる**
//! - 置かない: プロセス表の採取と「どのペインの配下か」の判定
//!   （`tako-control::ssh_detect`）・実際の SFTP 接続（`remote_fs`）
//!
//! # 見送る側に倒す（fail-closed）
//!
//! 自動で接続しに行く機能なので、**宛先を取り違えたら別のマシンの中身を
//! そのホスト名で見せてしまう**。そこで「tako が `ssh <宛先>` を打ち直しても
//! 同じ相手に届く」と確信できない形は [`SkipReason`] で全部見送る:
//!
//! - `-p 2222` / `-o Port=` … ポートが違えば別のマシンかもしれない（`remote_fs` は
//!   宛先文字列しか運べないので、ポートを保てない = 取り違えになる）。
//!   **例外は「宛先の名前そのものがそのポートを持つ」形**: `~/.ssh/config` が
//!   その Host に `Port 2222` と書いているなら `ssh <host>` でも同じ相手に届くので、
//!   取り違えにならない（[`ConfiguredPorts`]。#1411）
//! - `-J` / `-W` / `-o ProxyJump=` / `-o ProxyCommand=` / `-o Hostname=` … 経路や
//!   実ホストが書き換わる
//! - `ssh <host> <コマンド>` … 対話セッションではない。`git` / `rsync` / `scp` が
//!   内部で使う形（`ssh host rsync --server …`）を拾って「ユーザーが入った」と
//!   誤解しないため
//! - `-N`（シェルを開かない）… ポート転送・ControlMaster であってセッションではない
//!
//! 逆に、**どのマシンに届くかを変えないもの**（`-A` / `-t` / `-v` /
//! `-o ControlPath=` 等）は見送らない。`-l <user>` と `-o User=` は宛先へ
//! `user@` として畳み込む（見送るより忠実に再現できる）。
//!
//! # tako 自身が開いたペインを自分で疑わない（#1411）
//!
//! 「リモート接続…」（#1006）と `remote-folder open`（#1041）が開くペインの `ssh` は
//! tako が組む（`remote_fs::ssh_pane_argv` + `ssh_config::SshHost::ssh_command`）。
//! そこに載る `-p <port>` は **config の `Port` の書き写し**でしかないので、
//! 「非既定ポートは全部見送る」だと**自分が書いた `-p` を自分で疑う**ことになり、
//! 非既定ポートの Host はツリーへ自動で並ばなかった（#1411 の実測）。
//!
//! 直したのは判定の緩め方ではなく**物差し**: 見るのは「ポートが 22 か」ではなく
//! 「**宛先の名前だけでそのポートへ行けるか**」（= このモジュールが元から掲げている
//! 「`ssh <宛先>` を打ち直しても同じ相手に届く」の直訳）。ユーザーが手で打った
//! `ssh -p 2222 host` で config に `Port` が無ければ従来どおり見送る。
//! `-F <別の config>` が載っている行も信用しない（こちらが読んだ config と
//! 宛先の解決が食い違いうる）。
//!
//! もう 1 つ同じ症状の原因だったのが**引用つきオプション値の平坦化**:
//! tako は空白を含む値を `-o ControlPath="…"` と引用して渡すので、macOS の既定
//! data_dir（`~/Library/Application Support/tako`）では `ps` の 1 行が空白で割れ、
//! **続きの語が宛先に見えて** `RemoteCommand` で見送られていた（ポートが 22 でも
//! 起きる）。開いたままの引用が閉じるまで語を足して戻す（[`quote_unbalanced`]）。

use crate::platform::support::Note;

/// ペインで確立している ssh セッション 1 件（コマンド行から読み取れた宛先）
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SshCommand {
    /// `ssh` / `sftp` へそのまま渡せる宛先（`host` または `user@host`）。
    /// `~/.ssh/config` の Host 別名はそのまま保つ（tako が解決し直さない）
    pub destination: String,
}

/// 自動追加を見送る理由。**理由を持ち帰る**ので `tako remote-folder auto` で
/// 「なぜ出てこないのか」を説明できる（黙って何もしない、をしない）。
///
/// `Hash` を持つのは、見送りログの重複抑止（#1258）が
/// **(ペイン, ssh の pid, この理由)** を鍵にするから
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkipReason {
    /// そもそも ssh ではない
    NotSsh,
    /// 宛先が書かれていない（`ssh` 単体・オプションだけ）
    NoDestination,
    /// ポートが既定と違う（別のマシンかもしれないので触らない）
    PortOverride,
    /// 経路・実ホストが書き換わっている（ProxyJump / ProxyCommand / Hostname / -W）
    RouteOverride,
    /// リモートコマンドの一発実行（対話セッションではない）
    RemoteCommand,
    /// シェルを開かない（`-N` = 転送・ControlMaster 専用）
    NoShell,
    /// サブシステム直叩き（`-s`。sftp 等）
    Subsystem,
}

impl SkipReason {
    /// 全列挙（**足したらここにも足す**。網羅テストが足し忘れを落とす）
    pub const ALL: &'static [SkipReason] = &[
        SkipReason::NotSsh,
        SkipReason::NoDestination,
        SkipReason::PortOverride,
        SkipReason::RouteOverride,
        SkipReason::RemoteCommand,
        SkipReason::NoShell,
        SkipReason::Subsystem,
    ];

    /// 見送った理由（日英）。UI の通知にも `auto` の応答にも出るので
    /// `RemoteError` と同じく **`Note` で両言語を持つ**（#435 の規約）
    pub fn note(self) -> Note {
        match self {
            SkipReason::NotSsh => Note::new("ssh ではありません", "Not an ssh command"),
            SkipReason::NoDestination => {
                Note::new("宛先が書かれていません", "No destination in the command")
            }
            SkipReason::PortOverride => Note::new(
                "既定と違うポートを指定しています（別のマシンかもしれないので触りません）",
                "A non-default port is specified (it may be a different machine, so tako leaves it alone)",
            ),
            SkipReason::RouteOverride => Note::new(
                "経路や実ホストが上書きされています（ProxyJump / Hostname 等）",
                "The route or real host is overridden (ProxyJump / Hostname, etc.)",
            ),
            SkipReason::RemoteCommand => Note::new(
                "リモートコマンドの実行です（対話セッションではありません）",
                "It runs a remote command (not an interactive session)",
            ),
            SkipReason::NoShell => Note::new(
                "シェルを開かない接続です（ポート転送・ControlMaster）",
                "The connection opens no shell (port forwarding / ControlMaster)",
            ),
            SkipReason::Subsystem => Note::new(
                "サブシステムを直に呼んでいます",
                "It invokes a subsystem directly",
            ),
        }
    }

    /// 現在の表示言語での理由
    pub fn label(self) -> &'static str {
        self.note().text()
    }
}

/// 値を取るオプション（短い形）。`-p 22` のように次の語を食う
const OPTS_WITH_VALUE: &[char] = &[
    'B', 'b', 'c', 'D', 'E', 'e', 'F', 'I', 'i', 'J', 'L', 'l', 'm', 'O', 'o', 'p', 'Q', 'R', 'S',
    'W', 'w',
];

/// `ssh` の実行ファイルか（絶対パス・`.exe` も受ける）
pub fn is_ssh_program(word: &str) -> bool {
    let base = word
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(word)
        .to_ascii_lowercase();
    base == "ssh" || base == "ssh.exe"
}

/// 既定の ssh ポート（文字列で持つのは `ps` から来る値と直に比べるため）
const DEFAULT_PORT: &str = "22";

/// `~/.ssh/config` が Host ごとに宣言しているポート（Issue #1411）。
///
/// `-p 2222` が「別のマシンかもしれない」印になるのは、**宛先の名前だけでは
/// そのポートへ行けない**ときだけ。config 自身が `Port 2222` と書いている Host なら
/// `ssh <host>` でも sftp でも同じ相手に届くので、印にはならない
/// （`remote_fs` は宛先文字列しか運べないが、ポートはその文字列の解決に含まれている）。
///
/// これが #1411 の直し方の骨: tako が自分で開いた SSH ペインの `-p` は
/// **config の `Port` をそのまま書き写したもの**（`ssh_config::SshHost::ssh_command`）
/// なので、この一致で「tako 自身が開いたペイン」は全部この側に入る。
/// 印（ControlPath 等）を信用する形にしないのは、印は宛先が同じことを証明しないのに対し、
/// ここでの一致は「宛先の名前だけで同じ相手に届く」という**モジュール doc の不変条件
/// そのもの**を確かめているから
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfiguredPorts {
    hosts: Vec<(String, u16)>,
}

impl ConfiguredPorts {
    /// パース済みの Host 一覧から（純粋）
    pub fn from_hosts(hosts: &[crate::ssh_config::SshHost]) -> Self {
        Self {
            hosts: hosts
                .iter()
                .filter_map(|h| h.port.map(|p| (h.name.clone(), p)))
                .collect(),
        }
    }

    /// `~/.ssh/config` を読む（**ファイルを触るのはこの関数だけ**）。
    /// 読めない / 無いときは空 = 何も信用しない（#1411 以前と同じ fail-closed）
    pub fn from_default_config() -> Self {
        match crate::ssh_config::default_ssh_config_path() {
            Some(path) => Self::from_hosts(&crate::ssh_config::parse_ssh_config(&path)),
            None => Self::default(),
        }
    }

    /// `(Host 名, ポート)` を直に渡す（テスト・検証用。**実ファイルを読まない**）
    pub fn from_pairs(pairs: &[(&str, u16)]) -> Self {
        Self {
            hosts: pairs.iter().map(|(n, p)| ((*n).to_string(), *p)).collect(),
        }
    }

    /// その Host 名に対して config が宣言しているポート。
    ///
    /// 突き合わせは **Host 名の完全一致**（`parse_ssh_config` はワイルドカードの
    /// パターンを除くので、`Host *` の `Port` は入ってこない = 見送り側に倒れる）。
    /// `remote_ssh_argv` が `h.name == ssh_host` で引くのと同じ規則
    pub fn port_of(&self, host: &str) -> Option<u16> {
        self.hosts
            .iter()
            .find(|(name, _)| name == host)
            .map(|(_, port)| *port)
    }

    /// 何も宣言されていないか（= 何も信用しない）
    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }
}

/// 検知の材料（Issue #1411）。**env とファイルを読むのは [`DetectContext::current`] だけ**で、
/// [`parse_ssh_command_with`] は純粋なまま（両プラットフォームぶんを macOS からテストできる）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectContext {
    /// 宛先の名前そのものが持つポート
    configured: ConfiguredPorts,
    /// `ps` が空白で繋いだ引用つきの値を復元するか。
    /// tako 自身が渡す `-o ControlPath="…"` は macOS の既定 data_dir
    /// （`~/Library/Application Support/tako`）に空白を含むので、復元しないと
    /// **続きの語が宛先に見える**（#1411 の実測: `RemoteCommand` で見送られていた）
    join_quoted: bool,
}

impl Default for DetectContext {
    /// 既定は [`DetectContext::strict`]（config を読まない = 非既定ポートは全部見送る）
    fn default() -> Self {
        Self::strict()
    }
}

impl DetectContext {
    /// #1411 以前（引用の復元もしない・config も見ない）。A/B の腕
    pub fn legacy() -> Self {
        Self {
            configured: ConfiguredPorts::default(),
            join_quoted: false,
        }
    }

    /// config を持たない既定（引用の復元だけ入る。純粋）
    pub fn strict() -> Self {
        Self {
            configured: ConfiguredPorts::default(),
            join_quoted: true,
        }
    }

    /// config を材料として渡す（純粋）
    pub fn with_configured(configured: ConfiguredPorts) -> Self {
        Self {
            configured,
            join_quoted: true,
        }
    }

    /// 実行時の材料（**env と `~/.ssh/config` を読むのはここだけ**）
    pub fn current() -> Self {
        if legacy_1411() {
            return Self::legacy();
        }
        Self::with_configured(ConfiguredPorts::from_default_config())
    }

    /// 宛先の名前そのものが持つポート
    pub fn configured(&self) -> &ConfiguredPorts {
        &self.configured
    }
}

/// A/B 用の逃げ道（`TAKO_1411_LEGACY=1`）。立てると #1411 以前の判定へ戻る
/// （tako 自身が開いた SSH ペインを自動検知が見送る状態を同一バイナリで再現できる）
pub fn legacy_1411() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_1411_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// 値に開いたままの二重引用符が残っているか（`ps` の平坦化を戻す判定）。
///
/// OpenSSH の設定パーサは値を空白で切るので、tako は空白を含む値を
/// `ControlPath="…"` と**引用つきで渡す**（`remote_fs::control_path_option`）。
/// ssh 自身が外す引用なので argv には残り、`ps` の 1 行では空白で割れて届く
fn quote_unbalanced(value: &str) -> bool {
    value.matches('"').count() % 2 == 1
}

/// 値を囲む二重引用符を落とす（ssh の設定パーサと同じ扱い）
fn unquote(value: &str) -> &str {
    value.trim().trim_matches('"')
}

/// コマンド行から宛先を読み取る。見送る形は [`SkipReason`] を返す。
///
/// **`~/.ssh/config` を読まない**ので非既定ポートは常に見送る
/// （config を材料として渡す形は [`parse_ssh_command_with`]。#1411）
pub fn parse_ssh_command(cmdline: &str) -> Result<SshCommand, SkipReason> {
    parse_ssh_command_with(cmdline, &DetectContext::strict())
}

/// [`parse_ssh_command`] の中身（**材料を引数で受ける純粋関数**）。
///
/// 入力は `ps` が返す 1 行（引数は空白区切り）。**引用は復元できない**が、
/// 必要なのは宛先 1 語だけで、空白入りの宛先は存在しないので実害が無い。
/// ただし**引用つきのオプション値**（tako 自身の `-o ControlPath="…"`）は
/// 空白で割れて届くので、開いたままの引用が閉じるまで語を足して戻す（#1411）
pub fn parse_ssh_command_with(
    cmdline: &str,
    ctx: &DetectContext,
) -> Result<SshCommand, SkipReason> {
    let mut words = cmdline.split_whitespace();
    let Some(program) = words.next() else {
        return Err(SkipReason::NotSsh);
    };
    if !is_ssh_program(program) {
        return Err(SkipReason::NotSsh);
    }

    let mut destination: Option<String> = None;
    let mut user_opt: Option<String> = None;
    // 明示されたポート（`-p` / `-o Port=` / `ssh://host:port`）。**判断は最後に 1 回**
    let mut ports: Vec<String> = Vec::new();
    // `-F <file>` = 別の config を使っている。そのときは `ctx` の中身
    // （こちらが読んだ `~/.ssh/config`）と宛先の解決が食い違いうるので信用しない
    let mut own_config = true;
    let mut has_remote_command = false;

    while let Some(word) = words.next() {
        if destination.is_some() {
            // 宛先の後ろに何か来たらリモートコマンド（`--` の後ろも同じ扱い）
            has_remote_command = true;
            break;
        }
        if word == "--" {
            continue;
        }
        if let Some(flags) = word.strip_prefix('-').filter(|w| !w.is_empty()) {
            // 短いオプションの束（`-tt` / `-Nf` / `-o Key=Value` / `-p2222`）を辿る
            for (i, c) in flags.char_indices() {
                match c {
                    'N' => return Err(SkipReason::NoShell),
                    's' => return Err(SkipReason::Subsystem),
                    'W' => return Err(SkipReason::RouteOverride),
                    c if OPTS_WITH_VALUE.contains(&c) => {
                        // 値は「同じ語の残り」か「次の語」
                        let rest = &flags[i + c.len_utf8()..];
                        let mut value = if rest.is_empty() {
                            words.next().unwrap_or("").to_string()
                        } else {
                            rest.to_string()
                        };
                        // 引用が開いたままなら閉じるまで語を足す（#1411）。
                        // 閉じないまま行が尽きたら宛先が無くなる = `NoDestination` で
                        // 見送られる（fail-closed のまま）
                        if ctx.join_quoted && quote_unbalanced(&value) {
                            for next in words.by_ref() {
                                value.push(' ');
                                value.push_str(next);
                                if !quote_unbalanced(&value) {
                                    break;
                                }
                            }
                        }
                        match c {
                            'p' => ports.push(unquote(&value).to_string()),
                            'J' => return Err(SkipReason::RouteOverride),
                            'F' => own_config = false,
                            'l' if !value.is_empty() => user_opt = Some(value.clone()),
                            'o' => {
                                if let Some(reason) =
                                    check_option(&value, &mut user_opt, &mut ports)
                                {
                                    return Err(reason);
                                }
                            }
                            _ => {}
                        }
                        break; // 値を食ったので束はここで終わり
                    }
                    _ => {} // どのマシンへ届くかを変えない旗（-A / -t / -v / -q …）
                }
            }
            continue;
        }
        destination = Some(word.to_string());
    }

    if has_remote_command {
        return Err(SkipReason::RemoteCommand);
    }
    let dest = destination.ok_or(SkipReason::NoDestination)?;
    let (dest, uri_port) = normalize_destination(&dest)?;
    ports.extend(uri_port);
    // 宛先が `user@host` を持つならそちらが優先（ssh の規則と同じ）
    let destination = match (dest.contains('@'), user_opt) {
        (false, Some(user)) => format!("{user}@{dest}"),
        _ => dest,
    };
    // **ポートの判断はここ 1 箇所**（#1411。オプションの解釈の途中で返すと
    // 「宛先が分かる前に見送る」= tako 自身のペインを見分けられない）
    let host_part = destination
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(&destination);
    if ports
        .iter()
        .any(|port| !port_reachable_by_name(port, host_part, own_config, ctx))
    {
        return Err(SkipReason::PortOverride);
    }
    Ok(SshCommand { destination })
}

/// その明示ポートが**宛先の名前だけで再現できる**か（#1411）。
///
/// 偽なら [`SkipReason::PortOverride`]（`remote_fs` は宛先文字列しか運べないので、
/// 名前だけで同じポートへ行けないならツリーは別のマシンを見にいく）
fn port_reachable_by_name(port: &str, host: &str, own_config: bool, ctx: &DetectContext) -> bool {
    let port = port.trim();
    if port == DEFAULT_PORT {
        return true;
    }
    if !own_config {
        return false;
    }
    ctx.configured
        .port_of(host)
        .is_some_and(|declared| declared.to_string() == port)
}

/// `-o Key=Value` の検査。届く相手が変わるものは見送り、`User=` だけ畳み込む。
/// `Port=` は判断せず持ち帰る（判断は [`parse_ssh_command_with`] の末尾で 1 回）
fn check_option(
    value: &str,
    user_opt: &mut Option<String>,
    ports: &mut Vec<String>,
) -> Option<SkipReason> {
    let (key, val) = value.split_once('=')?;
    let key = key.trim().to_ascii_lowercase();
    let val = unquote(val);
    match key.as_str() {
        "port" => {
            ports.push(val.to_string());
            None
        }
        "proxyjump" | "proxycommand" | "hostname" => Some(SkipReason::RouteOverride),
        "user" => {
            if !val.is_empty() {
                *user_opt = Some(val.to_string());
            }
            None
        }
        // `ssh -o RequestTTY=no host` 等は届く相手を変えないので見送らない
        _ => None,
    }
}

/// `ssh://user@host:port/` 形式も受ける。
/// ポートは**宛先から落として持ち帰る**（見送るかは呼び出し側が 1 回で決める）
fn normalize_destination(dest: &str) -> Result<(String, Option<String>), SkipReason> {
    let body = match dest.strip_prefix("ssh://") {
        Some(rest) => rest.trim_end_matches('/'),
        None => {
            // 素の宛先に `:port` は書けない（`host:port` は scp 記法で ssh では通らない）
            return match dest.is_empty() {
                true => Err(SkipReason::NoDestination),
                false => Ok((dest.to_string(), None)),
            };
        }
    };
    // URI 形式のポートは `@` の後ろにだけ現れる（IPv6 は `[::1]:22`）
    let host_part = body.rsplit_once('@').map(|(_, h)| h).unwrap_or(body);
    let port = match host_part.rsplit_once(':') {
        // `[::1]` のように `]` で終わるならポートではない
        Some((_, port)) if !port.is_empty() && !port.ends_with(']') => Some(port.to_string()),
        _ => None,
    };
    if let Some(port) = port {
        let trimmed = body.strip_suffix(&format!(":{port}")).unwrap_or(body);
        return Ok((trimmed.to_string(), Some(port)));
    }
    match body.is_empty() {
        true => Err(SkipReason::NoDestination),
        false => Ok((body.to_string(), None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest(cmdline: &str) -> String {
        parse_ssh_command(cmdline)
            .unwrap_or_else(|e| panic!("{cmdline:?} を見送った: {}", e.label()))
            .destination
    }

    fn skip(cmdline: &str) -> SkipReason {
        parse_ssh_command(cmdline).expect_err(&format!("{cmdline:?} を受けてしまった"))
    }

    #[test]
    fn 素のsshは宛先をそのまま採る() {
        assert_eq!(dest("ssh win"), "win");
        assert_eq!(dest("/usr/bin/ssh cloud-host"), "cloud-host");
        assert_eq!(dest("ssh.exe box"), "box");
        // `~/.ssh/config` の別名はそのまま保つ（tako が解決し直さない）
        assert_eq!(dest("ssh my-alias"), "my-alias");
    }

    #[test]
    fn user付きの宛先と_lオプションを畳み込む() {
        assert_eq!(dest("ssh admin@box"), "admin@box");
        assert_eq!(dest("ssh -l admin box"), "admin@box");
        assert_eq!(dest("ssh -o User=admin box"), "admin@box");
        // 宛先側の user が優先（ssh 自身の規則）
        assert_eq!(dest("ssh -l ignored real@box"), "real@box");
    }

    #[test]
    fn 届く相手を変えない旗は見送らない() {
        assert_eq!(dest("ssh -A -t -q win"), "win");
        assert_eq!(dest("ssh -tt win"), "win");
        assert_eq!(dest("ssh -vvv win"), "win");
        assert_eq!(dest("ssh -i /path/to/key win"), "win");
        assert_eq!(dest("ssh -F /dev/null win"), "win");
        // tako 自身の SSH ペイン（#919 の `ssh_pane_argv`）も検知対象にする
        assert_eq!(
            dest("ssh -o ControlPath=\"/tmp/x.sock\" -o ControlMaster=auto win"),
            "win"
        );
        assert_eq!(dest("ssh -o ConnectTimeout=10 win"), "win");
        // 既定ポートの明示は素の形と同じ
        assert_eq!(dest("ssh -p 22 win"), "win");
        assert_eq!(dest("ssh -o Port=22 win"), "win");
    }

    #[test]
    fn 相手が変わる形は見送る() {
        assert_eq!(skip("ssh -p 2222 win"), SkipReason::PortOverride);
        assert_eq!(skip("ssh -p2222 win"), SkipReason::PortOverride);
        assert_eq!(skip("ssh -o Port=2222 win"), SkipReason::PortOverride);
        assert_eq!(skip("ssh -J jump win"), SkipReason::RouteOverride);
        assert_eq!(skip("ssh -o ProxyJump=jump win"), SkipReason::RouteOverride);
        // ps は引数を空白で割って返すので、値に空白を含む形もこの並びで来る
        assert_eq!(
            skip("ssh -o ProxyCommand=nc win 22 win"),
            SkipReason::RouteOverride
        );
        assert_eq!(
            skip("ssh -o Hostname=10.0.0.1 win"),
            SkipReason::RouteOverride
        );
        assert_eq!(skip("ssh -W other:22 win"), SkipReason::RouteOverride);
    }

    #[test]
    fn 対話セッションでない形は見送る() {
        // git / rsync / scp が内部で使う形
        assert_eq!(
            skip("ssh box rsync --server -logDtpre.iLsfxCIvu . /tmp/x"),
            SkipReason::RemoteCommand
        );
        assert_eq!(
            skip("ssh git@github.com git-upload-pack 'repo.git'"),
            SkipReason::RemoteCommand
        );
        assert_eq!(skip("ssh win uptime"), SkipReason::RemoteCommand);
        // 転送・ControlMaster 専用
        assert_eq!(
            skip("ssh -N -f -L 8080:localhost:80 win"),
            SkipReason::NoShell
        );
        assert_eq!(skip("ssh -M -N -f win"), SkipReason::NoShell);
        assert_eq!(skip("ssh -Nf win"), SkipReason::NoShell);
        assert_eq!(skip("ssh -s box sftp"), SkipReason::Subsystem);
    }

    #[test]
    fn ssh以外と宛先無しは弾く() {
        assert_eq!(skip("sftp win"), SkipReason::NotSsh);
        assert_eq!(skip("scp a win:b"), SkipReason::NotSsh);
        assert_eq!(skip("sshd"), SkipReason::NotSsh);
        assert_eq!(skip("ssh-agent -s"), SkipReason::NotSsh);
        assert_eq!(skip("mosh win"), SkipReason::NotSsh);
        assert_eq!(skip(""), SkipReason::NotSsh);
        assert_eq!(skip("ssh"), SkipReason::NoDestination);
        assert_eq!(skip("ssh -A -q"), SkipReason::NoDestination);
        // 値を食うオプションの引数を宛先と間違えない
        assert_eq!(skip("ssh -o BatchMode=yes"), SkipReason::NoDestination);
        assert_eq!(skip("ssh -i /path/key"), SkipReason::NoDestination);
    }

    #[test]
    fn uri形式も受ける() {
        assert_eq!(dest("ssh ssh://win"), "win");
        assert_eq!(dest("ssh ssh://admin@win"), "admin@win");
        assert_eq!(dest("ssh ssh://admin@win:22/"), "admin@win");
        assert_eq!(skip("ssh ssh://win:2222"), SkipReason::PortOverride);
    }

    /// `~/.ssh/config` に `Host win` / `Port 2222` があるときの材料（**ファイルを読まない**）
    fn ctx_win_2222() -> DetectContext {
        DetectContext::with_configured(ConfiguredPorts::from_pairs(&[("win", 2222)]))
    }

    fn dest_with(cmdline: &str, ctx: &DetectContext) -> String {
        parse_ssh_command_with(cmdline, ctx)
            .unwrap_or_else(|e| panic!("{cmdline:?} を見送った: {}", e.label()))
            .destination
    }

    fn skip_with(cmdline: &str, ctx: &DetectContext) -> SkipReason {
        parse_ssh_command_with(cmdline, ctx).expect_err(&format!("{cmdline:?} を受けてしまった"))
    }

    /// macOS の既定 data_dir（**空白を含む**）で tako が渡す `-o ControlPath=…`。
    /// 形の正本は `remote_fs` 側なのでそこから組む（写すと食い違う）
    fn spaced_control_path_opt(host: &str) -> String {
        let dir = std::path::Path::new("/Users/testuser/Library/Application Support/tako");
        crate::remote_fs::control_path_option(&crate::remote_fs::control_path_in(Some(dir), host))
    }

    #[test]
    fn configがそのportを宣言している宛先は見送らない() {
        let ctx = ctx_win_2222();
        // tako 自身が開いたペイン（`-p` は config の `Port` の書き写し）
        assert_eq!(dest_with("ssh -p 2222 win", &ctx), "win");
        assert_eq!(dest_with("ssh -p2222 win", &ctx), "win");
        assert_eq!(dest_with("ssh -o Port=2222 win", &ctx), "win");
        assert_eq!(dest_with("ssh -p 2222 admin@win", &ctx), "admin@win");
        assert_eq!(dest_with("ssh -l admin -p 2222 win", &ctx), "admin@win");
        assert_eq!(dest_with("ssh ssh://win:2222", &ctx), "win");
        // 既定ポートは今までどおり素通し
        assert_eq!(dest_with("ssh -p 22 win", &ctx), "win");
    }

    #[test]
    fn 宛先の名前だけでは行けないportは従来どおり見送る() {
        let ctx = ctx_win_2222();
        // config が宣言しているのと**違う**ポート
        assert_eq!(skip_with("ssh -p 2223 win", &ctx), SkipReason::PortOverride);
        // config に載っていない Host（= ユーザーが手で打った `-p`）
        assert_eq!(skip_with("ssh -p 2222 box", &ctx), SkipReason::PortOverride);
        assert_eq!(
            skip_with("ssh -o Port=2222 box", &ctx),
            SkipReason::PortOverride
        );
        // 別の config を使っている行は信用しない（宛先の解決が食い違いうる）
        assert_eq!(
            skip_with("ssh -F /tmp/other-config -p 2222 win", &ctx),
            SkipReason::PortOverride
        );
        // 材料が空（`~/.ssh/config` が無い / 読めない）なら全部見送り
        assert_eq!(skip("ssh -p 2222 win"), SkipReason::PortOverride);
    }

    #[test]
    fn 引用つきのオプション値が空白で割れても宛先を採る() {
        // macOS の既定 data_dir は空白を含むので `ps` の 1 行が割れて届く（#1411）
        let cp = spaced_control_path_opt("win");
        assert!(cp.contains(' '), "空白を含む前提が崩れている: {cp}");
        let line = format!(
            "/usr/bin/ssh -o {cp} -o ControlMaster=auto -o ControlPersist=600 \
             -o ConnectTimeout=10 -o ServerAliveInterval=5 -o ServerAliveCountMax=3 win"
        );
        assert_eq!(dest(&line), "win");
        // 引用が閉じないまま行が尽きたら宛先が無くなる = 見送る（fail-closed のまま）
        assert_eq!(
            skip("ssh -o ControlPath=\"/a b/x.sock -o ControlMaster=auto win"),
            SkipReason::NoDestination
        );
        // 引用つきでも届く相手を変える値は見送る
        assert_eq!(
            skip("ssh -o ProxyCommand=\"nc %h %p\" win"),
            SkipReason::RouteOverride
        );
        assert_eq!(
            skip("ssh -o Hostname=\"10.0.0.1\" win"),
            SkipReason::RouteOverride
        );
        // 引用つきの `Port` も同じ物差しに乗る
        assert_eq!(
            dest_with("ssh -o Port=\"2222\" win", &ctx_win_2222()),
            "win"
        );
    }

    #[test]
    fn tako自身が組むargvをそのまま読める() {
        // #1411 の配線の検査: 組む側（`remote_fs` + `ssh_config`）と読む側が食い違わない
        let host = crate::ssh_config::SshHost {
            name: "win".to_string(),
            hostname: None,
            user: None,
            port: Some(2222),
        };
        let cmd = host.ssh_command();
        // `remote_ssh_argv`（dispatch）と同じ切り出し = 先頭の `ssh` と末尾の宛先を落とす
        let extra = cmd[1..cmd.len() - 1].to_vec();
        assert_eq!(extra, vec!["-p".to_string(), "2222".to_string()]);
        let argv = crate::remote_fs::ssh_pane_argv_with("win", &extra, true);
        let line = argv.join(" ");
        let ctx = DetectContext::with_configured(ConfiguredPorts::from_hosts(&[host]));
        assert_eq!(dest_with(&line, &ctx), "win");
        // 同じ行を材料なしで読むと従来どおり見送る（= 材料が効いていることの裏）
        assert_eq!(skip(&line), SkipReason::PortOverride);
    }

    #[test]
    fn ポートの指定が複数あるときは全部が名前で行けることを要求する() {
        let ctx = ctx_win_2222();
        // `-p` と `-o Port=` が食い違う形（ssh 自身の優先順位に頼らず**両方**を要求する）
        assert_eq!(
            skip_with("ssh -p 2222 -o Port=2223 win", &ctx),
            SkipReason::PortOverride
        );
        assert_eq!(
            skip_with("ssh -o Port=2223 -p 2222 win", &ctx),
            SkipReason::PortOverride
        );
        // 両方が config の宣言と同じなら受ける
        assert_eq!(dest_with("ssh -p 2222 -o Port=2222 win", &ctx), "win");
        // 片方が既定ポートでも、もう片方が名前で行けないなら見送る
        assert_eq!(
            skip_with("ssh -p 22 -o Port=2223 win", &ctx),
            SkipReason::PortOverride
        );
        // `ssh://host:port` と `-p` の組み合わせも同じ物差し
        assert_eq!(
            skip_with("ssh -p 2222 ssh://win:2223", &ctx),
            SkipReason::PortOverride
        );
        assert_eq!(dest_with("ssh -p 2222 ssh://win:2222", &ctx), "win");
    }

    #[test]
    fn configがport22を明示している宛先も受ける() {
        // 既定ポートは材料の有無に関わらず受ける（#1411 以前と同じ）
        let ctx = DetectContext::with_configured(ConfiguredPorts::from_pairs(&[("box", 22)]));
        assert_eq!(dest_with("ssh -p 22 box", &ctx), "box");
        assert_eq!(dest_with("ssh -o Port=22 box", &ctx), "box");
        assert_eq!(dest_with("ssh ssh://box:22/", &ctx), "box");
        // 宣言が 22 なのに 22 以外を打つ形は見送る
        assert_eq!(skip_with("ssh -p 2222 box", &ctx), SkipReason::PortOverride);
    }

    #[test]
    fn legacyアームではtako自身のペインを見送る() {
        let legacy = DetectContext::legacy();
        // ポートの側（#1411 以前 = 非既定ポートは材料があっても見送る）
        assert_eq!(
            parse_ssh_command_with("ssh -p 2222 win", &legacy),
            Err(SkipReason::PortOverride)
        );
        // 引用の側（空白で割れた値の続きを宛先と読み違える）
        let cp = spaced_control_path_opt("win");
        let line = format!("/usr/bin/ssh -o {cp} -o ControlMaster=auto win");
        assert_eq!(
            parse_ssh_command_with(&line, &legacy),
            Err(SkipReason::RemoteCommand)
        );
    }

    #[test]
    fn configured_portsはhost名の完全一致で引く() {
        let ports = ConfiguredPorts::from_pairs(&[("win", 2222), ("box", 22)]);
        assert_eq!(ports.port_of("win"), Some(2222));
        assert_eq!(ports.port_of("box"), Some(22));
        assert_eq!(ports.port_of("wi"), None);
        assert_eq!(ports.port_of("win2"), None);
        assert!(!ports.is_empty());
        // `Port` を書いていない Host は入らない
        let hosts = vec![
            crate::ssh_config::SshHost {
                name: "plain".to_string(),
                hostname: None,
                user: None,
                port: None,
            },
            crate::ssh_config::SshHost {
                name: "high".to_string(),
                hostname: None,
                user: None,
                port: Some(2222),
            },
        ];
        let ports = ConfiguredPorts::from_hosts(&hosts);
        assert_eq!(ports.port_of("plain"), None);
        assert_eq!(ports.port_of("high"), Some(2222));
        assert!(ConfiguredPorts::default().is_empty());
    }

    #[test]
    fn 見送り理由はすべて日英の説明を持つ() {
        // 網羅 match（足し忘れはコンパイルで落ちる）
        for reason in SkipReason::ALL {
            let note = reason.note();
            assert!(!note.ja().trim().is_empty(), "{reason:?} の日本語が空");
            assert!(!note.en().trim().is_empty(), "{reason:?} の英語が空");
            // 訳し漏れ検出（#435 の規約と同じ検査）
            assert!(
                !note
                    .en()
                    .chars()
                    .any(|c| matches!(c as u32, 0x3040..=0x30FF | 0x4E00..=0x9FFF)),
                "{reason:?} の英語に日本語が残っている: {:?}",
                note.en()
            );
        }
        assert_eq!(SkipReason::ALL.len(), 7, "ALL に足し忘れが無いか");
    }
}
