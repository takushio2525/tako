//! リモート（スマホ）から**新しいタブ / ペインを立てる経路の宣言表**（#1449）。
//!
//! # なぜ表にするのか
//!
//! PWA の「+」は 3 種（master / ターミナル / SSH ターミナル）を起動するが、
//! **操作そのものは 1 つも新設していない**。どれも #1078 / #1080 で通した既存の
//! dispatch（`TabNew` / `OpenRemote`）をそのまま呼ぶ。
//!
//! だから危ないのは「経路が増えること」ではなく、**増えた経路が role の判断から
//! こぼれること**（#1405 の懸念: 未知の GET は `Observe` へ落ちる）。
//! ここに `kind`（何を立てるか）・`client_method`（PWA の呼び口）・
//! `role`・`audit_event` を 1 か所で宣言し、
//!
//! - [`role_for`] を `remote::required_role` が引く（= 表が実際の認可を決める）
//! - 番犬 `tests/issue1449_launch_routes_watchdog.rs` が
//!   「PWA が呼ぶ API がすべてこの表に在るか」をソース走査で照合する
//!
//! の 2 方向から縛る。**PWA に新しい呼び口が生えたら、表に載せるまで CI が落ちる。**
//!
//! # role をなぜ Manage のままにしたか（#1449 で検討して据え置き）
//!
//! 「新規起動は Interact 以上」という案もあったが、`POST /api/tabs` は #1078 が、
//! `/api/ssh*` は #1080 が **close / resize より強い操作**として意図的に Manage に
//! 置いた経路で、緩めると既にペアリング済みの Interact 端末の権限が黙って広がる。
//! 3 種を「新しいタブとプロセスを作る」という**同じ強さ**として揃える方を採った。

use crate::remote_auth::DeviceRole;

/// 「+」で立てられるもの（PWA の `LAUNCH_KINDS` と 1:1。`list` は選択肢を出すための補助）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchKind {
    /// オーケストレーター master の会話
    Master,
    /// 素のシェル 1 枚
    Terminal,
    /// `~/.ssh/config` のホストへの SSH
    Ssh,
    /// 選択肢を出すための一覧（起動そのものはしない）
    List,
}

impl LaunchKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Master => "master",
            Self::Terminal => "terminal",
            Self::Ssh => "ssh",
            Self::List => "list",
        }
    }
}

/// 1 本の経路の宣言
#[derive(Debug, Clone, Copy)]
pub struct LaunchRoute {
    /// 何を立てるための経路か
    pub kind: LaunchKind,
    /// PWA（`web/tako-remote/src/api.js`）の呼び口の名前
    pub client_method: &'static str,
    /// HTTP メソッド（`GET` / `POST`）
    pub method: &'static str,
    /// 具体形のパス（docs・番犬・テストが使う。可変部は代表値）
    pub sample_path: &'static str,
    /// 必要な role
    pub role: DeviceRole,
    /// 監査ログ（`<state_dir>/audit.log`）へ残すイベント名。
    /// 一覧を引くだけの経路は残さない（`None`）
    pub audit_event: Option<&'static str>,
    /// このパスに当たるか（可変部があるので関数で持つ）
    matches: fn(&str) -> bool,
}

impl LaunchRoute {
    pub fn matches(&self, method: &str, path: &str) -> bool {
        self.method.eq_ignore_ascii_case(method) && (self.matches)(path)
    }
}

fn is_tabs(path: &str) -> bool {
    path == "/api/tabs"
}
fn is_tab_master(path: &str) -> bool {
    path.starts_with("/api/tabs/") && path.ends_with("/master")
}
fn is_master_profiles(path: &str) -> bool {
    path == "/api/master/profiles"
}
fn is_pane_list(path: &str) -> bool {
    path == "/api/v2/panes"
}
fn is_ssh_hosts(path: &str) -> bool {
    path == "/api/ssh-hosts"
}
fn is_ssh_open(path: &str) -> bool {
    path == "/api/ssh"
}
fn is_pane_ssh(path: &str) -> bool {
    path.starts_with("/api/panes/") && path.ends_with("/ssh")
}

/// 経路表（**正本**）。
///
/// - `terminal` が `POST /api/tabs` だけなのは、`TabNew` が**シェル 1 枚付きのタブ**を
///   返すため（別途ペインを作る操作を足していない）
/// - `ssh` が `/api/tabs` を通らないのは、`OpenRemote { target: "tab" }` が
///   タブごと作るため（二重に作らない）
pub const LAUNCH_ROUTES: &[LaunchRoute] = &[
    LaunchRoute {
        kind: LaunchKind::List,
        client_method: "masterProfiles",
        method: "GET",
        sample_path: "/api/master/profiles",
        // 一覧はファイル直読みで、押せるかどうかとは別（#1078）。
        // 見えても起動は Manage で止まる
        role: DeviceRole::Observe,
        audit_event: None,
        matches: is_master_profiles,
    },
    LaunchRoute {
        kind: LaunchKind::List,
        client_method: "panes",
        method: "GET",
        sample_path: "/api/v2/panes",
        // master を立てたあと「Claude 公式へ繋がったか」を待つのに引く（#1078）。
        // 画面データそのものなので Observe（起動の可否とは別の判断）
        role: DeviceRole::Observe,
        audit_event: None,
        matches: is_pane_list,
    },
    LaunchRoute {
        kind: LaunchKind::Terminal,
        client_method: "createTab",
        method: "POST",
        sample_path: "/api/tabs",
        role: DeviceRole::Manage,
        audit_event: Some("tab_new"),
        matches: is_tabs,
    },
    LaunchRoute {
        kind: LaunchKind::Master,
        client_method: "launchMaster",
        method: "POST",
        sample_path: "/api/tabs/7/master",
        role: DeviceRole::Manage,
        audit_event: Some("master_launch"),
        matches: is_tab_master,
    },
    LaunchRoute {
        kind: LaunchKind::Ssh,
        client_method: "sshHosts",
        method: "GET",
        sample_path: "/api/ssh-hosts",
        // 一覧が GET なのに Observe でないのは #1080 の判断:
        // Host 名 / user / port は画面に映らない別の在庫情報で、
        // 押せない端末へ配る理由が無い
        role: DeviceRole::Manage,
        audit_event: None,
        matches: is_ssh_hosts,
    },
    LaunchRoute {
        kind: LaunchKind::Ssh,
        client_method: "sshOpen",
        method: "POST",
        sample_path: "/api/ssh",
        role: DeviceRole::Manage,
        audit_event: Some("ssh_open"),
        matches: is_ssh_open,
    },
    LaunchRoute {
        kind: LaunchKind::Ssh,
        client_method: "sshPane",
        method: "POST",
        sample_path: "/api/panes/7/ssh",
        role: DeviceRole::Manage,
        audit_event: Some("ssh_open"),
        matches: is_pane_ssh,
    },
];

/// この表に載っている経路なら必要 role を返す（`remote::required_role` が最初に引く）
pub fn role_for(method: &str, path: &str) -> Option<DeviceRole> {
    LAUNCH_ROUTES
        .iter()
        .find(|r| r.matches(method, path))
        .map(|r| r.role)
}

/// PWA の呼び口の名前から経路を引く（番犬が使う）
pub fn route_for_client_method(name: &str) -> Option<&'static LaunchRoute> {
    LAUNCH_ROUTES.iter().find(|r| r.client_method == name)
}

/// 起動（= 新しいタブ / ペインを作る）経路だけを返す。一覧を引くだけの経路は含まない
pub fn mutating_routes() -> impl Iterator<Item = &'static LaunchRoute> {
    LAUNCH_ROUTES.iter().filter(|r| r.audit_event.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 起動経路はすべてmanage以上で監査イベントを持つ() {
        for r in mutating_routes() {
            assert!(
                r.role >= DeviceRole::Manage,
                "{} ({} {}) が Manage 未満: 新しいタブとプロセスを作る経路は \
                 close / resize と同じ強さで扱う（#1078 / #1080 / #1449）",
                r.client_method,
                r.method,
                r.sample_path
            );
            assert!(
                r.audit_event.is_some(),
                "{} は監査イベントを持たない",
                r.client_method
            );
        }
    }

    #[test]
    fn 表の具体形が自分の判定に当たる() {
        for r in LAUNCH_ROUTES {
            assert!(
                r.matches(r.method, r.sample_path),
                "{} の sample_path（{}）が自分の matches に当たらない",
                r.client_method,
                r.sample_path
            );
            assert_eq!(
                role_for(r.method, r.sample_path),
                Some(r.role),
                "{} の role が表から引けない",
                r.client_method
            );
        }
    }

    #[test]
    fn 表に無い経路は何も返さない() {
        assert_eq!(role_for("POST", "/api/unknown"), None);
        assert_eq!(
            role_for("GET", "/api/tabs"),
            None,
            "GET /api/tabs は経路が無い"
        );
        assert_eq!(
            role_for("POST", "/api/master/profiles"),
            None,
            "一覧は GET だけ"
        );
    }

    #[test]
    fn 呼び口の名前は重複しない() {
        let mut names: Vec<&str> = LAUNCH_ROUTES.iter().map(|r| r.client_method).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(
            before,
            names.len(),
            "client_method が重複している: {names:?}"
        );
    }

    #[test]
    fn 起動できる3種すべてが表に在る() {
        for kind in [LaunchKind::Master, LaunchKind::Terminal, LaunchKind::Ssh] {
            assert!(
                LAUNCH_ROUTES.iter().any(|r| r.kind == kind),
                "{} を立てる経路が表に無い",
                kind.as_str()
            );
        }
    }
}
