//! ~/.ssh/config の Host エントリをパースして一覧を返す。
//! ProxyJump / IdentityFile 等は ssh コマンド自体が解決するため、ここでは Host 名の
//! 抽出のみを行う。ワイルドカード（*）を含むパターンは除外する。

use std::path::{Path, PathBuf};

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

/// ~/.ssh/config のデフォルトパス
pub fn default_ssh_config_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(|h| PathBuf::from(h).join(".ssh/config"))
}

/// 指定パスの SSH config から Host エントリを抽出する。
/// ワイルドカード（* を含む）ホストは除外。ファイルが無ければ空を返す。
pub fn parse_ssh_config(path: &Path) -> Vec<SshHost> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    parse_ssh_config_str(&content)
}

fn parse_ssh_config_str(content: &str) -> Vec<SshHost> {
    let mut hosts = Vec::new();
    let mut current: Option<SshHost> = None;

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
                if let Some(host) = current.take() {
                    hosts.push(host);
                }
                // Host 行は空白区切りで複数パターンを持てる。各パターンを独立した
                // エントリとして扱う（* を含むパターンは除外）
                for pattern in value.split_whitespace() {
                    if !pattern.contains('*') && !pattern.contains('?') {
                        current = Some(SshHost {
                            name: pattern.to_string(),
                            hostname: None,
                            user: None,
                            port: None,
                        });
                        break;
                    }
                }
            }
            "hostname" if current.is_some() => {
                current.as_mut().unwrap().hostname = Some(value.to_string());
            }
            "user" if current.is_some() => {
                current.as_mut().unwrap().user = Some(value.to_string());
            }
            "port" if current.is_some() => {
                if let Ok(p) = value.parse::<u16>() {
                    current.as_mut().unwrap().port = Some(p);
                }
            }
            _ => {}
        }
    }
    if let Some(host) = current {
        hosts.push(host);
    }
    hosts
}

/// "Key Value" または "Key=Value" を分割
fn split_key_value(line: &str) -> Option<(&str, &str)> {
    if let Some(eq_pos) = line.find('=') {
        let key = line[..eq_pos].trim();
        let value = line[eq_pos + 1..].trim();
        if key.is_empty() {
            return None;
        }
        Some((key, value))
    } else {
        let mut parts = line.splitn(2, char::is_whitespace);
        let key = parts.next()?;
        let value = parts.next()?.trim();
        Some((key, value))
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
        // 最初の非ワイルドカードパターンのみ
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "alpha");
    }
}
