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
//!   宛先文字列しか運べないので、ポートを保てない = 取り違えになる）
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

use crate::platform::support::Note;

/// ペインで確立している ssh セッション 1 件（コマンド行から読み取れた宛先）
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SshCommand {
    /// `ssh` / `sftp` へそのまま渡せる宛先（`host` または `user@host`）。
    /// `~/.ssh/config` の Host 別名はそのまま保つ（tako が解決し直さない）
    pub destination: String,
}

/// 自動追加を見送る理由。**理由を持ち帰る**ので `tako remote-folder auto` で
/// 「なぜ出てこないのか」を説明できる（黙って何もしない、をしない）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// コマンド行から宛先を読み取る。見送る形は [`SkipReason`] を返す。
///
/// 入力は `ps` が返す 1 行（引数は空白区切り）。**引用は復元できない**が、
/// 必要なのは宛先 1 語だけで、空白入りの宛先は存在しないので実害が無い。
pub fn parse_ssh_command(cmdline: &str) -> Result<SshCommand, SkipReason> {
    let mut words = cmdline.split_whitespace();
    let Some(program) = words.next() else {
        return Err(SkipReason::NotSsh);
    };
    if !is_ssh_program(program) {
        return Err(SkipReason::NotSsh);
    }

    let mut destination: Option<String> = None;
    let mut user_opt: Option<String> = None;
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
                        let value = if rest.is_empty() {
                            words.next().unwrap_or("").to_string()
                        } else {
                            rest.to_string()
                        };
                        match c {
                            'p' if value.trim() != "22" => return Err(SkipReason::PortOverride),
                            'J' => return Err(SkipReason::RouteOverride),
                            'l' if !value.is_empty() => user_opt = Some(value.clone()),
                            'o' => {
                                if let Some(reason) = check_option(&value, &mut user_opt) {
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
    let dest = normalize_destination(&dest)?;
    // 宛先が `user@host` を持つならそちらが優先（ssh の規則と同じ）
    let destination = match (dest.contains('@'), user_opt) {
        (false, Some(user)) => format!("{user}@{dest}"),
        _ => dest,
    };
    Ok(SshCommand { destination })
}

/// `-o Key=Value` の検査。届く相手が変わるものは見送り、`User=` だけ畳み込む
fn check_option(value: &str, user_opt: &mut Option<String>) -> Option<SkipReason> {
    let (key, val) = value.split_once('=')?;
    let key = key.trim().to_ascii_lowercase();
    let val = val.trim();
    match key.as_str() {
        "port" => (val != "22").then_some(SkipReason::PortOverride),
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

/// `ssh://user@host:port/` 形式も受ける（ポート付きは見送り）
fn normalize_destination(dest: &str) -> Result<String, SkipReason> {
    let body = match dest.strip_prefix("ssh://") {
        Some(rest) => rest.trim_end_matches('/'),
        None => {
            // 素の宛先に `:port` は書けない（`host:port` は scp 記法で ssh では通らない）
            return match dest.is_empty() {
                true => Err(SkipReason::NoDestination),
                false => Ok(dest.to_string()),
            };
        }
    };
    // URI 形式のポートは `@` の後ろにだけ現れる（IPv6 は `[::1]:22`）
    let host_part = body.rsplit_once('@').map(|(_, h)| h).unwrap_or(body);
    let has_port = match host_part.rsplit_once(':') {
        // `[::1]` のように `]` で終わるならポートではない
        Some((_, port)) => !port.is_empty() && !port.ends_with(']'),
        None => false,
    };
    if has_port {
        let port = host_part.rsplit_once(':').map(|(_, p)| p).unwrap_or("");
        if port != "22" {
            return Err(SkipReason::PortOverride);
        }
        // 既定ポートの明示は宛先から落として素の形へ
        let trimmed = body.strip_suffix(":22").unwrap_or(body);
        return Ok(trimmed.to_string());
    }
    match body.is_empty() {
        true => Err(SkipReason::NoDestination),
        false => Ok(body.to_string()),
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
