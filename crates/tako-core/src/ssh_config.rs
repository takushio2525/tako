//! ~/.ssh/config の Host エントリをパースして一覧を返す。
//! ProxyJump / IdentityFile 等は ssh コマンド自体が解決するため、ここでは Host 名と
//! 宛先の組み立てに必要な `HostName` / `User` / `Port` の抽出のみを行う。
//! ワイルドカード（`*` / `?`）を含むパターンと否定（`!`）のパターンは除外する。
//!
//! ## 「どの Host にも属さない位置」を持つ（#1400）
//!
//! 結果は表示専用ではない: `dispatch::remote_ssh_argv` が [`SshHost::ssh_command`] から
//! `-p <port>` と `user@host` を取り出して実際の接続 argv にするので、**設定を取り違えると
//! 繋ぎ先が変わる**。移送前は `Match` 行を素通りさせていたため、
//!
//! ```text
//! Host prod
//!   HostName 10.x.x.x
//! Match host bastion
//!   User root
//!   Port 2222
//! ```
//!
//! で `prod` に `user=root` / `port=2222` が付き、argv が `ssh -p 2222 root@prod` へ化けていた
//! （本物の `ssh prod` は `Match host bastion` に一致しないのでこの設定を使わない =
//! **tako だけが別の宛先・別ユーザーへ繋ぐ**）。
//!
//! そこで読み進めるあいだの状態を [`Section`] の 2 値にし、`Match` と `Include` のあとは
//! **どの Host にも属さない位置**（[`Section::Unattached`]）へ倒す。以降の
//! `HostName` / `User` / `Port` は次の `Host` 行が来るまで捨てる。取りこぼす側（設定が
//! 付かない）へ倒すのは意図的で、**繋ぎ先が変わるより、既定のまま繋ぐほうが安全**という判断。

use std::path::{Component, Path, PathBuf};

/// `Include` の入れ子の上限（OpenSSH の `MAX_READCONF_DEPTH` と同じ 16 段）。
/// 循環参照は [`Parser::chain`] が先に捕まえるので、ここは連鎖が単純に深いときの安全弁
const MAX_INCLUDE_DEPTH: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshHost {
    pub name: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
}

impl SshHost {
    /// ssh コマンドの引数を組み立てる（`ssh [-p port] [user@]host`）
    pub fn ssh_command(&self) -> Vec<String> {
        let mut args = vec!["ssh".to_string()];
        if let Some(port) = self.port {
            args.push("-p".to_string());
            args.push(port.to_string());
        }
        let dest = if let Some(ref user) = self.user {
            format!("{user}@{}", self.name)
        } else {
            self.name.clone()
        };
        args.push(dest);
        args
    }
}

/// パースの結果（ホスト一覧 + 診断）。
///
/// `Include` を黙って飛ばさないために警告を**返す**形にしてある（ログへ書くのは
/// 呼び出し側 = [`parse_ssh_config`]）。こうしておくとテストが診断の中身を
/// ログファイル無しで固定できる
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SshConfigParse {
    pub hosts: Vec<SshHost>,
    /// 読めなかった / 循環した / 深すぎた `Include` の理由（パスは `~` 短縮済み）
    pub warnings: Vec<String>,
}

/// `~/.ssh/config` のデフォルトパス。
///
/// ホーム解決は正本（[`crate::paths::home_dir`]）を通す（#893）。ここが `HOME`
/// 決め打ちだったので、**Windows では ssh 系の補完・解決がまるごと無効**だった
/// （`%USERPROFILE%` しか無い環境で必ず `None` を返す = #870 と同型）
pub fn default_ssh_config_path() -> Option<PathBuf> {
    crate::paths::home_dir().map(|home| ssh_config_path_in(&home))
}

/// ホームを与えたときの `~/.ssh/config`（純粋。macOS から Windows のホームを検査できる）。
/// `.join(".ssh/config")` と違い区切りを OS 側へ任せるので、Windows でも
/// `C:\Users\winuser\.ssh\config` の形になる
pub fn ssh_config_path_in(home: &Path) -> PathBuf {
    home.join(".ssh").join("config")
}

/// 指定パスの SSH config から Host エントリを抽出する。
/// ワイルドカードホストは除外。ファイルが無ければ空を返す。
///
/// `Include` の診断は `tracing::warn!` へ流す（中身は理由とパスだけで、
/// ペイン内容・トークンは含まない = 規約）
pub fn parse_ssh_config(path: &Path) -> Vec<SshHost> {
    let home = crate::paths::home_dir();
    let parsed = parse_ssh_config_with_home(path, home.as_deref());
    for warning in &parsed.warnings {
        tracing::warn!("{warning}");
    }
    parsed.hosts
}

/// [`parse_ssh_config`] の中身（`home` を引数で受ける）。
///
/// `home` は `~` の展開と**相対 `Include` の起点**（OpenSSH と同じく `~/.ssh/`）に使う。
/// 実ホームを読まないのでテストは fixture の dir を渡せる（#944）
pub fn parse_ssh_config_with_home(path: &Path, home: Option<&Path>) -> SshConfigParse {
    let mut parser = Parser::new(home);
    // **最上位のファイルが無いのは正常**（ssh config を持たないユーザー）なので警告しない。
    // `Include` 先が読めないのは設定の取りこぼしなので、そちらだけ理由を残す
    if let Ok(content) = std::fs::read_to_string(path) {
        parser
            .chain
            .push(crate::platform::path::canonicalize_or_self(path));
        parser.parse_str(&content);
    }
    SshConfigParse {
        hosts: parser.hosts,
        warnings: parser.warnings,
    }
}

/// 読み進めるあいだの「いまどの塊に居るか」。
///
/// `Match` ブロックと `Include` のあとを [`Section::Unattached`] へ倒すのが #1400 の要点
#[derive(Debug)]
enum Section {
    /// どの `Host` にも属さない位置（先頭の大域設定・`Match` ブロック・`Include` の直後）。
    /// ここで読んだ `HostName` / `User` / `Port` は捨てる
    Unattached,
    /// `Host` 行で開いた塊。1 行に複数パターンが書けるので**パターンごとに 1 エントリ**を持ち、
    /// 以降の設定は全部へ同じように付ける
    Hosts(Vec<SshHost>),
}

struct Parser<'a> {
    /// `~` の展開と相対 `Include` の起点（`~/.ssh/`）
    home: Option<&'a Path>,
    hosts: Vec<SshHost>,
    warnings: Vec<String>,
    /// いま読んでいるファイルの連鎖（循環参照の検出用の比較キー。
    /// 境界 B26 = [`crate::platform::path::canonicalize_or_self`] を通す = #970）
    chain: Vec<PathBuf>,
}

impl<'a> Parser<'a> {
    fn new(home: Option<&'a Path>) -> Self {
        Self {
            home,
            hosts: Vec::new(),
            warnings: Vec::new(),
            chain: Vec::new(),
        }
    }

    /// 開いている塊を確定して [`Section::Unattached`] へ戻す
    fn close(&mut self, section: &mut Section) {
        if let Section::Hosts(entries) = std::mem::replace(section, Section::Unattached) {
            self.hosts.extend(entries);
        }
    }

    fn parse_str(&mut self, content: &str) {
        let mut section = Section::Unattached;

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            let (key, value) = match split_key_value(trimmed) {
                Some(kv) => kv,
                None => continue,
            };

            match key.to_ascii_lowercase().as_str() {
                "host" => {
                    self.close(&mut section);
                    // Host 行は空白区切りで複数パターンを持てる。**各パターンを独立した
                    // エントリとして扱う**（`Host web1 web2` は 2 件。移送前は先頭 1 つで
                    // `break` していたので `web2` が一覧から消えていた = #1400 の (2)）
                    let entries: Vec<SshHost> = split_patterns(value)
                        .into_iter()
                        .filter(|p| is_connectable_pattern(p))
                        .map(|pattern| SshHost {
                            name: pattern,
                            hostname: None,
                            user: None,
                            port: None,
                        })
                        .collect();
                    if !entries.is_empty() {
                        section = Section::Hosts(entries);
                    }
                }
                // `Match` の中の設定はどの Host にも属さない（#1400 の (1)）。
                // ここで塊を閉じるので、以降の `User` / `Port` は次の `Host` まで捨てる
                "match" => self.close(&mut section),
                // `Include` は**塊を閉じてから**読む。OpenSSH は取り込んだ側の `Host` 行が
                // 親ファイルの残りの解釈まで変えるので、親の塊を開いたまま続けると
                // 「取り込み先の Host 向けの設定」を親の Host へ付けてしまう（= (1) の再発）
                "include" => {
                    self.close(&mut section);
                    for spec in split_patterns(value) {
                        self.include(&spec);
                    }
                }
                "hostname" if !value.is_empty() => {
                    if let Section::Hosts(entries) = &mut section {
                        for entry in entries.iter_mut() {
                            entry.hostname = Some(value.to_string());
                        }
                    }
                }
                "user" if !value.is_empty() => {
                    if let Section::Hosts(entries) = &mut section {
                        for entry in entries.iter_mut() {
                            entry.user = Some(value.to_string());
                        }
                    }
                }
                "port" => {
                    if let (Section::Hosts(entries), Ok(port)) =
                        (&mut section, value.parse::<u16>())
                    {
                        for entry in entries.iter_mut() {
                            entry.port = Some(port);
                        }
                    }
                }
                _ => {}
            }
        }
        self.close(&mut section);
    }

    /// `Include` のパス指定 1 つを解決して読む（glob 展開・相対パス・深さ・循環をここで見る）
    fn include(&mut self, spec: &str) {
        let Some(resolved) = self.resolve_include_spec(spec) else {
            // ホームが解決できない環境（Windows で `HOME` も `%USERPROFILE%` も無い等）
            self.warnings.push(format!(
                "Include の相対パスを解決できません（ホームが不明。読み飛ばし）: {spec}"
            ));
            return;
        };
        let has_glob = spec.contains('*') || spec.contains('?');
        let matches = expand_include_glob(&resolved);
        if matches.is_empty() && has_glob {
            // glob が 1 つも当たらないのは**正常**（`Include config.d/*` で dir が空）なので
            // 警告しない。OpenSSH も同じ扱い
            return;
        }
        if matches.is_empty() {
            self.include_file(&resolved);
            return;
        }
        for path in matches {
            self.include_file(&path);
        }
    }

    /// 取り込み先 1 ファイルを読む（**読めない・循環・深すぎは黙って飛ばさず理由を残す**）
    fn include_file(&mut self, path: &Path) {
        let key = crate::platform::path::canonicalize_or_self(path);
        if self.chain.contains(&key) {
            self.warnings.push(format!(
                "Include が循環しています（読み飛ばし）: {}",
                display_path(path)
            ));
            return;
        }
        if self.chain.len() >= MAX_INCLUDE_DEPTH {
            self.warnings.push(format!(
                "Include の入れ子が {MAX_INCLUDE_DEPTH} 段を超えました（読み飛ばし）: {}",
                display_path(path)
            ));
            return;
        }
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                self.warnings.push(format!(
                    "Include を読めません（読み飛ばし）: {}: {}",
                    display_path(path),
                    e.kind()
                ));
                return;
            }
        };
        self.chain.push(key);
        self.parse_str(&content);
        self.chain.pop();
    }

    /// `Include` の値 1 つを実パスへ。相対は OpenSSH と同じく **`~/.ssh/` 起点**
    fn resolve_include_spec(&self, spec: &str) -> Option<PathBuf> {
        // `~` / `~/` はホーム起点。`~user` は解決しない（既知の限界。OpenSSH は解決する）
        if spec == "~" {
            return self.home.map(Path::to_path_buf);
        }
        if let Some(rest) = spec.strip_prefix("~/").or_else(|| spec.strip_prefix("~\\")) {
            return self.home.map(|home| home.join(rest));
        }
        let path = Path::new(spec);
        if path.is_absolute() {
            return Some(path.to_path_buf());
        }
        self.home.map(|home| home.join(".ssh").join(path))
    }
}

/// 接続先として一覧に出せるパターンか。
///
/// ワイルドカード（`*` / `?`）は「どのホストへ繋ぐか」が決まらないので除外し、
/// 否定パターン（`!web2`）も**繋ぐ相手ではない**ので除外する
fn is_connectable_pattern(pattern: &str) -> bool {
    !pattern.is_empty()
        && !pattern.contains('*')
        && !pattern.contains('?')
        && !pattern.starts_with('!')
}

/// 空白区切りのトークンへ割る（二重引用符で囲めば空白を含められる）。
/// `Host` のパターン列と `Include` のパス列の**両方**がこの形
fn split_patterns(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut started = false;
    for ch in value.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    out.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(current);
    }
    out
}

/// `Include` のパスを実ファイルへ展開する。
///
/// OpenSSH は `glob(3)` を通すので `*` / `?` が使える。**`[...]` の文字クラスは
/// 展開しない**（実在の ssh config でほぼ使われないので、当たらないという既知の限界に
/// してある）。glob を含まないパスはそのまま 1 件として返す（実在の確認は読むときに行い、
/// 読めなければ理由が残る）
fn expand_include_glob(spec: &Path) -> Vec<PathBuf> {
    let mut root = PathBuf::new();
    let mut segments: Vec<String> = Vec::new();
    for comp in spec.components() {
        match comp {
            // 先頭の `C:` / `/` は起点。それ以降を 1 成分ずつパターンとして見る
            Component::Prefix(_) | Component::RootDir if segments.is_empty() => {
                root.push(comp.as_os_str())
            }
            other => segments.push(other.as_os_str().to_string_lossy().into_owned()),
        }
    }
    if !segments.iter().any(|s| s.contains('*') || s.contains('?')) {
        return Vec::new();
    }

    let mut current = vec![root];
    for (i, segment) in segments.iter().enumerate() {
        let last = i + 1 == segments.len();
        let mut next = Vec::new();
        if segment.contains('*') || segment.contains('?') {
            for dir in &current {
                let Ok(entries) = std::fs::read_dir(dir) else {
                    continue;
                };
                let mut matched: Vec<PathBuf> = entries
                    .flatten()
                    .filter_map(|entry| {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        // glob(3) と同じで、パターンが `.` で始まらない限り隠しファイルは拾わない
                        if name.starts_with('.') && !segment.starts_with('.') {
                            return None;
                        }
                        if !glob_segment_matches(segment, &name) {
                            return None;
                        }
                        let path = entry.path();
                        // 最後の成分は読むので**ファイルだけ**、途中の成分は降りるので dir だけ
                        let usable = if last { path.is_file() } else { path.is_dir() };
                        usable.then_some(path)
                    })
                    .collect();
                // glob(3) は結果を並べ替えるので、取り込む順序が dir の走査順でぶれないようにする
                matched.sort();
                next.extend(matched);
            }
        } else {
            next.extend(current.iter().map(|dir| dir.join(segment)));
        }
        current = next;
    }
    current
}

/// パス 1 成分のワイルドカード照合（`*` = 0 文字以上・`?` = 1 文字）。
///
/// `*` の後戻りで最悪指数時間になるのを避けるため、動的計画法で 1 回走査する
fn glob_segment_matches(pattern: &str, name: &str) -> bool {
    let name: Vec<char> = name.chars().collect();
    // `reachable[j]` = パターンをここまで読んだとき `name` の先頭 j 文字を消費し得るか
    let mut reachable = vec![false; name.len() + 1];
    reachable[0] = true;
    for pc in pattern.chars() {
        let mut next = vec![false; name.len() + 1];
        match pc {
            '*' => {
                let mut seen = false;
                for (j, slot) in next.iter_mut().enumerate() {
                    seen |= reachable[j];
                    *slot = seen;
                }
            }
            // 1 文字消費 = 到達位置を 1 つずらす
            '?' => next[1..=name.len()].copy_from_slice(&reachable[..name.len()]),
            c => {
                for j in 1..=name.len() {
                    next[j] = reachable[j - 1] && name[j - 1] == c;
                }
            }
        }
        reachable = next;
    }
    reachable[name.len()]
}

/// 診断へ出すパス（実ホームを生で出さない = #927 / ログ規約）
fn display_path(path: &Path) -> String {
    crate::paths::shorten_home(&path.to_string_lossy())
}

/// "Key Value" / "Key=Value" / "Key = Value" を分割する。
///
/// キーワードは**最初のトークン**（空白か `=` で終わる）。行全体の最初の `=` で
/// 割っていたので、値に `=` を含む行（`Match exec "test -f a=b"`）では
/// キーワードが読めず `Match` の検知が抜けていた
fn split_key_value(line: &str) -> Option<(&str, &str)> {
    let key_end = line
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(line.len());
    let key = &line[..key_end];
    if key.is_empty() {
        return None;
    }
    let rest = line[key_end..].trim_start();
    let value = rest.strip_prefix('=').map_or(rest, str::trim_start);
    Some((key, value.trim_end()))
}

/// 文字列だけをパースする（`Include` の相対解決は行わない）。テスト用
#[cfg(test)]
fn parse_ssh_config_str(content: &str) -> Vec<SshHost> {
    let mut parser = Parser::new(None);
    parser.parse_str(content);
    parser.hosts
}

#[cfg(test)]
mod home_tests {
    use super::*;

    /// `~/.ssh/config` が **Windows のホームでも**組み立てられること（#893）。
    ///
    /// 移送前はここが `HOME` 決め打ちだったので、`%USERPROFILE%` しか無い環境では
    /// 必ず `None` を返し、ssh 系の補完・解決がまるごと無効だった（#870 と同型）
    #[test]
    fn windowsのホームでもsshconfigを組み立てる() {
        // `%USERPROFILE%` だけがある環境の解決結果を模す（正本と同じ純粋関数を通す）
        let home =
            crate::paths::home_from(None, Some(std::ffi::OsString::from("C:\\Users\\winuser")))
                .expect("USERPROFILE からホームが解決できる");
        assert_eq!(
            ssh_config_path_in(&home),
            std::path::PathBuf::from("C:\\Users\\winuser")
                .join(".ssh")
                .join("config")
        );
        // macOS / Linux 側は従来どおり
        assert_eq!(
            ssh_config_path_in(std::path::Path::new("/Users/testuser")),
            std::path::PathBuf::from("/Users/testuser/.ssh/config")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_hosts() {
        let config = "\
Host myserver
    HostName 192.168.1.10
    User admin
    Port 2222

Host devbox
    HostName dev.example.com

Host *
    ServerAliveInterval 60
";
        let hosts = parse_ssh_config_str(config);
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].name, "myserver");
        assert_eq!(hosts[0].hostname.as_deref(), Some("192.168.1.10"));
        assert_eq!(hosts[0].user.as_deref(), Some("admin"));
        assert_eq!(hosts[0].port, Some(2222));
        assert_eq!(hosts[1].name, "devbox");
        assert_eq!(hosts[1].hostname.as_deref(), Some("dev.example.com"));
        assert_eq!(hosts[1].user, None);
        assert_eq!(hosts[1].port, None);
    }

    #[test]
    fn skip_wildcards() {
        let config = "\
Host *.example.com
    User deploy

Host prod?
    HostName prod1.example.com

Host realhost
    HostName real.example.com
";
        let hosts = parse_ssh_config_str(config);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "realhost");
    }

    #[test]
    fn empty_config() {
        let hosts = parse_ssh_config_str("");
        assert!(hosts.is_empty());
    }

    #[test]
    fn equals_syntax() {
        let config = "Host=eqhost\nHostName=eq.example.com\nUser=equser\n";
        let hosts = parse_ssh_config_str(config);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "eqhost");
        assert_eq!(hosts[0].hostname.as_deref(), Some("eq.example.com"));
        assert_eq!(hosts[0].user.as_deref(), Some("equser"));
    }

    #[test]
    fn ssh_command_basic() {
        let host = SshHost {
            name: "myserver".to_string(),
            hostname: None,
            user: None,
            port: None,
        };
        assert_eq!(host.ssh_command(), vec!["ssh", "myserver"]);
    }

    #[test]
    fn ssh_command_with_user_and_port() {
        let host = SshHost {
            name: "myserver".to_string(),
            hostname: Some("1.2.3.4".to_string()),
            user: Some("admin".to_string()),
            port: Some(2222),
        };
        assert_eq!(
            host.ssh_command(),
            vec!["ssh", "-p", "2222", "admin@myserver"]
        );
    }

    #[test]
    fn nonexistent_file() {
        let hosts = parse_ssh_config(Path::new("/nonexistent/ssh/config"));
        assert!(hosts.is_empty());
    }

    #[test]
    fn multi_host_line() {
        let config = "Host alpha beta\n    HostName a.example.com\n";
        let hosts = parse_ssh_config_str(config);
        // **全パターンが独立したエントリ**（コメントどおり。#1400 の (2) で実装を合わせた）
        let names: Vec<&str> = hosts.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"]);
        assert!(hosts
            .iter()
            .all(|h| h.hostname.as_deref() == Some("a.example.com")));
    }

    /// ワイルドカードと否定は混ざっていても落とす（残りは全部エントリになる）
    #[test]
    fn multi_host_line_mixed_wildcards() {
        let config = "Host web1 *.example.com !web9 web2 prod?\n    User deploy\n";
        let hosts = parse_ssh_config_str(config);
        let names: Vec<&str> = hosts.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, vec!["web1", "web2"]);
        assert!(hosts.iter().all(|h| h.user.as_deref() == Some("deploy")));
    }
}

/// #1400 の 3 つの取りこぼしを Issue の入力で逐語に固定するテスト。
///
/// うち (1) は**実際に組み立てられる ssh の argv を変える**（`prod` へ繋ぐつもりが
/// `ssh -p 2222 root@prod` になる = tako だけが別の宛先・別ユーザーへ行く）ので、
/// 入力は Issue のものをそのまま使う。
#[cfg(test)]
mod issue1400_tests {
    use super::*;

    /// Issue [2]: `Match` ブロックの `User` / `Port` が直前の Host へ混入しない
    #[test]
    fn i1400_matchブロックの設定は直前のhostへ付かない() {
        // Issue の入力を逐語で（実ホスト名・実ユーザー名は書かない = #927）
        let config = "\
Host prod
  HostName 10.x.x.x
Match host bastion
  User root
  Port 2222
";
        let hosts = parse_ssh_config_str(config);
        assert_eq!(hosts.len(), 1, "Host は prod の 1 件だけ: {hosts:?}");
        assert_eq!(hosts[0].name, "prod");
        assert_eq!(hosts[0].hostname.as_deref(), Some("10.x.x.x"));
        assert_eq!(
            hosts[0].user, None,
            "Match host bastion の User が prod へ混入している: {hosts:?}"
        );
        assert_eq!(
            hosts[0].port, None,
            "Match host bastion の Port が prod へ混入している: {hosts:?}"
        );
    }

    /// Issue [2] の結合: パース結果から組む argv が `ssh prod` 相当になる
    /// （`dispatch::remote_ssh_argv` が `ssh_command()` から取り出す形と同じ切り出し）
    #[test]
    fn i1400_matchブロックのあとのargvに宛先が化けない() {
        let config = "\
Host prod
  HostName 10.x.x.x
Match host bastion
  User root
  Port 2222
";
        let hosts = parse_ssh_config_str(config);
        let host = hosts
            .iter()
            .find(|h| h.name == "prod")
            .expect("prod が居る");
        assert_eq!(
            host.ssh_command(),
            vec!["ssh", "prod"],
            "argv が別の宛先・ポートへ化けている"
        );
    }

    /// Issue [1]: 複数パターンの Host 行は**全パターン**が独立エントリになる
    /// （コメントがそう言っているので実装を合わせる側にした）
    #[test]
    fn i1400_複数パターンのhostは全部エントリになる() {
        let config = "Host web1 web2\n    HostName a.example.com\n";
        let hosts = parse_ssh_config_str(config);
        let names: Vec<&str> = hosts.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, vec!["web1", "web2"], "web2 が一覧から消えている");
        assert!(
            hosts
                .iter()
                .all(|h| h.hostname.as_deref() == Some("a.example.com")),
            "同じ設定が両方へ付く: {hosts:?}"
        );
    }

    /// Issue [3]: `Include` 配下の Host が一覧に出る（絶対パス + glob）
    #[test]
    fn i1400_include配下のhostが一覧に出る() {
        let scratch = crate::test_residue::ScratchDir::new("ssh-config-1400");
        let dir = scratch.path();
        let conf_d = dir.join("config.d");
        std::fs::create_dir_all(&conf_d).expect("fixture の dir を作る");
        std::fs::write(conf_d.join("10-a.conf"), "Host a\n  HostName a.internal\n")
            .expect("include 先を書く");
        let cfg = dir.join("config");
        std::fs::write(
            &cfg,
            format!("Include {}/config.d/*\nHost b\n", dir.display()),
        )
        .expect("config を書く");

        let hosts = parse_ssh_config(&cfg);
        let names: Vec<&str> = hosts.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"], "Include 配下の Host が出ていない");
    }

    /// fixture の `~` を作る（`<scratch>/.ssh/config` と `home` を返す）
    fn fixture_home(tag: &str) -> (crate::test_residue::ScratchDir, PathBuf) {
        let scratch = crate::test_residue::ScratchDir::new(tag);
        let ssh = scratch.path().join(".ssh");
        std::fs::create_dir_all(&ssh).expect("fixture の ~/.ssh を作る");
        let home = scratch.path().to_path_buf();
        (scratch, home)
    }

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("fixture の dir を作る");
        }
        std::fs::write(path, content).expect("fixture を書く");
    }

    fn names(hosts: &[SshHost]) -> Vec<&str> {
        hosts.iter().map(|h| h.name.as_str()).collect()
    }

    /// 相対 `Include` は **`~/.ssh/` 起点**（OpenSSH と同じ）。glob も当たる
    #[test]
    fn i1400_相対includeはsshディレクトリ起点で解決する() {
        let (scratch, home) = fixture_home("ssh-config-1400-rel");
        let ssh = scratch.path().join(".ssh");
        write(&ssh.join("config.d/20-b.conf"), "Host b\n  Port 2200\n");
        write(&ssh.join("config.d/10-a.conf"), "Host a\n  User deploy\n");
        // 隠しファイルは glob(3) と同じで拾わない
        write(&ssh.join("config.d/.hidden.conf"), "Host hidden\n");
        // dir は最後の成分では読まない
        std::fs::create_dir_all(ssh.join("config.d/sub.conf")).expect("dir を作る");
        write(&ssh.join("config"), "Include config.d/*\nHost local\n");

        let parsed = parse_ssh_config_with_home(&ssh.join("config"), Some(&home));
        // 取り込む順序は glob(3) と同じで並べ替え済み（dir の走査順でぶれない）
        assert_eq!(names(&parsed.hosts), vec!["a", "b", "local"]);
        assert_eq!(parsed.hosts[0].user.as_deref(), Some("deploy"));
        assert_eq!(parsed.hosts[1].port, Some(2200));
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    }

    /// `~/...` と絶対パス・1 行に複数指定・`*.conf` の部分 glob
    #[test]
    fn i1400_includeのチルダと複数指定と部分globが効く() {
        let (scratch, home) = fixture_home("ssh-config-1400-tilde");
        let ssh = scratch.path().join(".ssh");
        write(&ssh.join("extra/x.conf"), "Host x\n");
        write(&ssh.join("extra/y.txt"), "Host y\n");
        write(&scratch.path().join("abs.conf"), "Host z\n");
        write(
            &ssh.join("config"),
            &format!(
                "Include ~/.ssh/extra/*.conf \"{}\"\n",
                scratch.path().join("abs.conf").display()
            ),
        );

        let parsed = parse_ssh_config_with_home(&ssh.join("config"), Some(&home));
        // `*.conf` に当たらない `y.txt` は入らない
        assert_eq!(names(&parsed.hosts), vec!["x", "z"]);
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    }

    /// 当たらない glob は**正常**（警告なし）／実在しない literal は理由が残る
    #[test]
    fn i1400_読めないincludeは黙って飛ばさない() {
        let (scratch, home) = fixture_home("ssh-config-1400-missing");
        let ssh = scratch.path().join(".ssh");
        std::fs::create_dir_all(ssh.join("empty.d")).expect("空 dir");
        write(
            &ssh.join("config"),
            "Include empty.d/*\nInclude missing.conf\nHost keep\n",
        );

        let parsed = parse_ssh_config_with_home(&ssh.join("config"), Some(&home));
        assert_eq!(names(&parsed.hosts), vec!["keep"], "残りは読み進める");
        assert_eq!(parsed.warnings.len(), 1, "{:?}", parsed.warnings);
        assert!(
            parsed.warnings[0].contains("Include を読めません")
                && parsed.warnings[0].contains("missing.conf"),
            "{:?}",
            parsed.warnings
        );
        // 診断に実ホームを生で出さない（#927）
        assert!(
            !parsed.warnings[0].contains(&home.to_string_lossy().to_string())
                || crate::paths::home_dir().as_deref() != Some(home.as_path()),
            "{:?}",
            parsed.warnings
        );
    }

    /// ホームが解決できない環境では相対 `Include` を諦めるが、理由は残す
    #[test]
    fn i1400_ホーム不明なら相対includeの理由を残す() {
        let scratch = crate::test_residue::ScratchDir::new("ssh-config-1400-nohome");
        let cfg = scratch.path().join("config");
        write(&cfg, "Include config.d/*\nHost keep\n");

        let parsed = parse_ssh_config_with_home(&cfg, None);
        assert_eq!(names(&parsed.hosts), vec!["keep"]);
        assert_eq!(parsed.warnings.len(), 1, "{:?}", parsed.warnings);
        assert!(
            parsed.warnings[0].contains("ホームが不明"),
            "{:?}",
            parsed.warnings
        );
    }

    /// 循環参照は 1 回で止まり、Host を二重に積まない
    #[test]
    fn i1400_includeの循環参照で止まる() {
        let (scratch, home) = fixture_home("ssh-config-1400-cycle");
        let ssh = scratch.path().join(".ssh");
        // 自己参照 + 相互参照の両方
        write(&ssh.join("self.conf"), "Host s\nInclude self.conf\n");
        write(&ssh.join("a.conf"), "Host a\nInclude b.conf\n");
        write(&ssh.join("b.conf"), "Host b\nInclude a.conf\n");
        write(&ssh.join("config"), "Include self.conf\nInclude a.conf\n");

        let parsed = parse_ssh_config_with_home(&ssh.join("config"), Some(&home));
        assert_eq!(names(&parsed.hosts), vec!["s", "a", "b"], "二重に積まない");
        assert_eq!(parsed.warnings.len(), 2, "{:?}", parsed.warnings);
        assert!(
            parsed
                .warnings
                .iter()
                .all(|w| w.contains("Include が循環しています")),
            "{:?}",
            parsed.warnings
        );
    }

    /// 入れ子の深さ上限（循環でない単純な連鎖）で打ち切り、理由を残す
    #[test]
    fn i1400_includeの入れ子は上限で打ち切る() {
        let (scratch, home) = fixture_home("ssh-config-1400-depth");
        let ssh = scratch.path().join(".ssh");
        let chain = MAX_INCLUDE_DEPTH + 4;
        for i in 1..=chain {
            let next = if i < chain {
                format!("Include d{}.conf\n", i + 1)
            } else {
                String::new()
            };
            write(
                &ssh.join(format!("d{i}.conf")),
                &format!("Host h{i}\n{next}"),
            );
        }
        write(&ssh.join("config"), "Include d1.conf\n");

        let parsed = parse_ssh_config_with_home(&ssh.join("config"), Some(&home));
        // 最上位の config が連鎖の 1 段目を占めるので、読めるのは d1..d15
        let got = names(&parsed.hosts);
        assert_eq!(got.len(), MAX_INCLUDE_DEPTH - 1, "{got:?}");
        assert_eq!(got.first(), Some(&"h1"));
        assert_eq!(got.last(), Some(&"h15"));
        assert_eq!(parsed.warnings.len(), 1, "{:?}", parsed.warnings);
        assert!(
            parsed.warnings[0].contains("16 段を超えました"),
            "{:?}",
            parsed.warnings
        );
    }

    /// `Include` は塊を閉じる = 取り込みのあとの設定を直前の Host へ付けない
    #[test]
    fn i1400_includeのあとの設定は直前のhostへ付かない() {
        let (scratch, home) = fixture_home("ssh-config-1400-close");
        let ssh = scratch.path().join(".ssh");
        // 取り込み先が Host で終わる = OpenSSH でも親の残りは prod 向けではない
        write(&ssh.join("inc.conf"), "Host bastion\n  User root\n");
        write(
            &ssh.join("config"),
            "Host prod\n  HostName 10.x.x.x\nInclude inc.conf\n  User root\n  Port 2222\n",
        );

        let parsed = parse_ssh_config_with_home(&ssh.join("config"), Some(&home));
        let prod = parsed
            .hosts
            .iter()
            .find(|h| h.name == "prod")
            .expect("prod が居る");
        assert_eq!(prod.hostname.as_deref(), Some("10.x.x.x"), "{prod:?}");
        assert_eq!(
            prod.user, None,
            "Include 後の User が混入している: {prod:?}"
        );
        assert_eq!(
            prod.port, None,
            "Include 後の Port が混入している: {prod:?}"
        );
        assert_eq!(prod.ssh_command(), vec!["ssh", "prod"]);
        let bastion = parsed
            .hosts
            .iter()
            .find(|h| h.name == "bastion")
            .expect("取り込み先の Host は出る");
        assert_eq!(bastion.user.as_deref(), Some("root"));
    }

    /// `Match all` / `Host *` だけの config・空ファイル・CRLF
    #[test]
    fn i1400_match_allとワイルドカードのみとcrlfと空() {
        // `Match all` も塊を閉じる
        let hosts = parse_ssh_config_str("Host prod\nMatch all\n  User root\n  Port 2222\n");
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].user, None, "{hosts:?}");
        assert_eq!(hosts[0].port, None, "{hosts:?}");

        // 値に `=` を含む Match（キーワードは最初のトークンなので読める）
        let hosts = parse_ssh_config_str("Host prod\nMatch exec \"test -f a=b\"\n  User root\n");
        assert_eq!(hosts[0].user, None, "{hosts:?}");

        // キーワードだけの `Match` / `Host` も塊を閉じる
        let hosts = parse_ssh_config_str("Host prod\nMatch\n  User root\n");
        assert_eq!(hosts[0].user, None, "{hosts:?}");

        // `Host *` だけ = エントリ 0 件、以降の設定はどこにも付かない
        assert!(parse_ssh_config_str("Host *\n  User deploy\n  Port 2222\n").is_empty());

        // CRLF
        let hosts = parse_ssh_config_str(
            "Host prod\r\n  HostName 10.x.x.x\r\nMatch host b\r\n  Port 2222\r\n",
        );
        assert_eq!(names(&hosts), vec!["prod"]);
        assert_eq!(hosts[0].hostname.as_deref(), Some("10.x.x.x"), "{hosts:?}");
        assert_eq!(hosts[0].port, None, "{hosts:?}");

        // 空ファイル
        let (scratch, home) = fixture_home("ssh-config-1400-empty");
        let ssh = scratch.path().join(".ssh");
        write(&ssh.join("config"), "");
        let parsed = parse_ssh_config_with_home(&ssh.join("config"), Some(&home));
        assert!(parsed.hosts.is_empty());
        assert!(parsed.warnings.is_empty());
    }

    /// **結合**: パース結果 → 実際に起動される SSH ペインの argv。
    ///
    /// `dispatch::remote_ssh_argv`（`crates/tako-control/src/dispatch.rs`）は
    /// `ssh_command()` の**先頭 `ssh` と末尾の宛先を落とした中身**を
    /// `remote_fs::ssh_pane_argv` の `extra` へ渡し、`User` があれば末尾の宛先を
    /// `user@host` へ差し替える。ここでは同じ切り出しを通して、
    /// `Match` ブロックの設定が `-p` にも宛先にも混ざらないことを見る
    /// （番犬 `issue1400_ssh_config_watchdog` が dispatch 側がこの形のままかを見張る）
    #[test]
    fn i1400_結合_matchブロックはssh_paneのargvへ漏れない() {
        let config = "\
Host prod
  HostName 10.x.x.x
Match host bastion
  User root
  Port 2222
";
        let hosts = parse_ssh_config_str(config);
        let host = hosts
            .iter()
            .find(|h| h.name == "prod")
            .expect("prod が居る");

        // --- dispatch::remote_ssh_argv と同じ組み立て ---
        let cmd = host.ssh_command();
        let extra: Vec<String> = cmd[1..cmd.len().saturating_sub(1)].to_vec();
        // 多重化の有無で opts が変わるだけなので、純粋版で両方見る
        for multiplexing in [true, false] {
            let mut argv = crate::remote_fs::ssh_pane_argv_with("prod", &extra, multiplexing);
            if let Some(user) = &host.user {
                if let Some(last) = argv.last_mut() {
                    *last = format!("{user}@prod");
                }
            }
            assert!(
                !argv.iter().any(|a| a == "-p"),
                "Match の Port が argv へ漏れた: {argv:?}"
            );
            assert!(
                !argv.iter().any(|a| a.contains("root@")),
                "Match の User が宛先へ漏れた: {argv:?}"
            );
            assert_eq!(
                argv.last().map(String::as_str),
                Some("prod"),
                "宛先が化けている: {argv:?}"
            );
        }
    }

    /// 同じ組み立てで、**本当に Host 直下に在る** User / Port は argv へ載ること
    /// （上の検査が「常に載らない」だけで通ってしまわないようにする = 検出力の担保）
    #[test]
    fn i1400_結合_host直下のuserとportはargvへ載る() {
        let config = "Host prod\n  User deploy\n  Port 2222\n";
        let hosts = parse_ssh_config_str(config);
        let host = &hosts[0];
        let cmd = host.ssh_command();
        let extra: Vec<String> = cmd[1..cmd.len().saturating_sub(1)].to_vec();
        let mut argv = crate::remote_fs::ssh_pane_argv_with("prod", &extra, false);
        if let Some(user) = &host.user {
            if let Some(last) = argv.last_mut() {
                *last = format!("{user}@prod");
            }
        }
        assert!(
            argv.windows(2).any(|w| w[0] == "-p" && w[1] == "2222"),
            "{argv:?}"
        );
        assert_eq!(
            argv.last().map(String::as_str),
            Some("deploy@prod"),
            "{argv:?}"
        );
    }

    /// パス 1 成分の glob 照合（`*` / `?`。`[...]` は既知の限界で当たらない）
    #[test]
    fn i1400_glob成分の照合() {
        assert!(glob_segment_matches("*", "anything"));
        assert!(glob_segment_matches("*.conf", "10-a.conf"));
        assert!(!glob_segment_matches("*.conf", "a.conf.bak"));
        assert!(glob_segment_matches("a?c", "abc"));
        assert!(!glob_segment_matches("a?c", "ac"));
        assert!(glob_segment_matches("a*b*c", "aXXbYYc"));
        assert!(!glob_segment_matches("a*b*c", "aXXbYY"));
        assert!(glob_segment_matches("conf", "conf"));
        assert!(
            !glob_segment_matches("conf", "Conf"),
            "大文字小文字は区別する"
        );
        // `[...]` は展開しないので literal として当たらない（doc の既知の限界）
        assert!(!glob_segment_matches("[ab].conf", "a.conf"));
    }
}
