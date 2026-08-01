//! アカウントの高速切替（Issue #709）。
//!
//! `accounts.yaml`（#504）のレジストリと、config dir のログイン状態（#653）と、
//! プロファイルの `master_account` / `worker_account`（#504 / #653）は
//! それぞれ独立に実装されていて、**束ねて見る手段が無かった**。
//! ここが「一覧 → ログイン → 割り当て」を 1 本に束ねる層。
//!
//! 判定は極力純関数に寄せ、ファイルシステムを触るのは
//! `probe_status` / `collect_views` の 2 つだけに閉じる（テスト可能性のため）。

use super::{
    expand_tilde, read_account_login, AccountConfigDir, AccountsConfig, Profile, ResolvedAccount,
};
use std::path::Path;

/// アカウントのログイン状態（Issue #709）。
///
/// 「config dir がまだ無い」と「dir はあるがログインしていない」を区別する。
/// 前者は `login` で作るところから、後者は `/login` だけで済むため、
/// 次にやるべきことが変わる
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountStatus {
    /// ログイン済み（`oauthAccount.emailAddress` が読めた）
    LoggedIn { email: String },
    /// config dir はあるがログインしていない（`.claude.json` が無い / メールが読めない）
    LoggedOut,
    /// config dir がまだ存在しない（`login` が作る）
    Missing,
    /// エントリが壊れている（config_dir と inherit の排他違反など）
    Invalid { error: String },
}

impl AccountStatus {
    /// 機械可読の短い識別子（CLI / MCP 応答で使う）
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LoggedIn { .. } => "logged_in",
            Self::LoggedOut => "logged_out",
            Self::Missing => "missing",
            Self::Invalid { .. } => "invalid",
        }
    }

    pub fn email(&self) -> Option<&str> {
        match self {
            Self::LoggedIn { email } => Some(email),
            _ => None,
        }
    }

    pub fn is_logged_in(&self) -> bool {
        matches!(self, Self::LoggedIn { .. })
    }
}

/// config dir（`None` = 既定ログイン = `inherit`）のログイン状態を調べる。
///
/// `Missing` は **config dir 自体が無い**ときだけ。既定ログイン（`inherit`）は
/// `~/.claude.json` を見るので `Missing` にはならない（未ログインなら `LoggedOut`）
pub fn probe_status(config_dir: Option<&str>) -> AccountStatus {
    if let Some(dir) = config_dir {
        // config dir が無ければ「これから作る」= Missing。
        // `.claude.json` の有無より先に判定する（次の一手が変わるため）
        if !Path::new(&expand_tilde(dir)).is_dir() {
            return AccountStatus::Missing;
        }
    }
    let login = read_account_login(config_dir);
    match login.email {
        Some(email) => AccountStatus::LoggedIn { email },
        None => AccountStatus::LoggedOut,
    }
}

/// 一覧表示 1 件分（Issue #709）
#[derive(Debug, Clone)]
pub struct AccountView {
    pub name: String,
    /// 注入する `CLAUDE_CONFIG_DIR`。`None` = inherit（既定ログイン）
    pub config_dir: Option<String>,
    pub inherit: bool,
    pub description: Option<String>,
    pub default_model: Option<String>,
    pub default_effort: Option<String>,
    pub status: AccountStatus,
    /// このアカウントを `master_account` にしているプロファイル名
    pub master_of: Vec<String>,
    /// このアカウントを `worker_account` にしているプロファイル名
    pub worker_of: Vec<String>,
}

impl AccountView {
    /// 何かに割り当てられているか
    pub fn is_assigned(&self) -> bool {
        !self.master_of.is_empty() || !self.worker_of.is_empty()
    }
}

/// プロファイル名 → (master_account, worker_account) の対応表。
/// ファイル読み込みは呼び出し側で済ませ、ここは純関数にする（テスト可能性）
pub type ProfileAssignments = Vec<(String, Option<String>, Option<String>)>;

/// 読み込み済みのプロファイル割り当てから、アカウント名ごとの使用箇所を引く（純関数）
pub fn assignments_for(name: &str, assignments: &ProfileAssignments) -> (Vec<String>, Vec<String>) {
    let mut master_of = Vec::new();
    let mut worker_of = Vec::new();
    for (profile, master, worker) in assignments {
        if master.as_deref() == Some(name) {
            master_of.push(profile.clone());
        }
        if worker.as_deref() == Some(name) {
            worker_of.push(profile.clone());
        }
    }
    (master_of, worker_of)
}

/// 解決済みアカウント 1 件をビューへ落とす（純関数 + status プローブ）
fn view_of(
    name: &str,
    resolved: &Result<ResolvedAccount, String>,
    assignments: &ProfileAssignments,
) -> AccountView {
    let (master_of, worker_of) = assignments_for(name, assignments);
    match resolved {
        Ok(account) => {
            let config_dir = account.config_dir.path().map(str::to_string);
            AccountView {
                name: name.to_string(),
                status: probe_status(config_dir.as_deref()),
                config_dir,
                inherit: account.config_dir.is_inherit(),
                description: account.description.clone(),
                default_model: account.default_model.clone(),
                default_effort: account.default_effort.clone(),
                master_of,
                worker_of,
            }
        }
        Err(error) => AccountView {
            name: name.to_string(),
            config_dir: None,
            inherit: false,
            description: None,
            default_model: None,
            default_effort: None,
            status: AccountStatus::Invalid {
                error: error.clone(),
            },
            master_of,
            worker_of,
        },
    }
}

/// 全プロファイルの account 割り当てを読む（存在しない・壊れたプロファイルは飛ばす）
pub fn load_profile_assignments() -> ProfileAssignments {
    let names = super::list_profiles().unwrap_or_default();
    names
        .into_iter()
        .filter_map(|name| {
            let profile = Profile::load(&name).ok()?;
            Some((
                name,
                profile.master_account.clone(),
                profile.worker_account.clone(),
            ))
        })
        .collect()
}

/// 一覧（ログイン状態 + 割り当てつき）を作る
pub fn collect_views(accounts: &AccountsConfig) -> Vec<AccountView> {
    let assignments = load_profile_assignments();
    accounts
        .list_resolved()
        .iter()
        .map(|(name, resolved)| view_of(name, resolved, &assignments))
        .collect()
}

/// 割り当て先の役割（Issue #709）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AssignRoles {
    pub master: bool,
    pub worker: bool,
}

impl AssignRoles {
    pub fn is_empty(&self) -> bool {
        !self.master && !self.worker
    }

    /// 反映タイミングの案内（役割ごとに違う。UI / CLI で同じ文言を使う）
    pub fn applies_when(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.master {
            v.push("master: 次に tako master / tako solo を起動したときから");
        }
        if self.worker {
            v.push("worker: 次に spawn する worker から（起動済み worker は変わらない）");
        }
        v
    }
}

/// `login` で使う起動コマンドの材料（純関数で組み立て、テストで固定する）。
///
/// claude は未ログインなら起動時にログインを促すが、**すでに別アカウントで
/// ログイン済みの config dir を切り替えたい**場合もあるため `/login` を明示的に送る
pub const LOGIN_SLASH_COMMAND: &str = "/login";

/// `login` の対象として妥当かを判定する（純関数）。
///
/// `inherit` のアカウントは config dir を持たないので、tako 側で作るものが無い。
/// 既定ログインの `claude /login` はユーザーが普通に実行すればよいので、
/// ここでは「何を実行すべきか」を指示に変えて返す
pub fn login_plan(account: &ResolvedAccount) -> Result<LoginPlan, String> {
    match &account.config_dir {
        AccountConfigDir::Path(dir) => Ok(LoginPlan {
            config_dir: dir.clone(),
            needs_create: !Path::new(dir).is_dir(),
        }),
        AccountConfigDir::Inherit => Err(format!(
            "アカウント '{}' は inherit（既定の資格情報を使う）なので、tako が作る config dir がありません。\n  \
             既定ログインを切り替えるときは、通常のターミナルで claude を起動して /login してください",
            account.name
        )),
    }
}

/// `login` の実行計画
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginPlan {
    /// 作成・利用する config dir（expand_tilde 済み）
    pub config_dir: String,
    /// この呼び出しでディレクトリを新規作成する必要があるか
    pub needs_create: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::AccountEntry;

    /// テスト用の一時ディレクトリ（プロセス ID + カウンタで衝突を避ける）
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tako-acct-{}-{}-{}",
            tag,
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn config_dirが無ければmissing() {
        let base = temp_dir("missing");
        let absent = base.join("not-created-yet");
        assert_eq!(
            probe_status(Some(&absent.display().to_string())),
            AccountStatus::Missing
        );
    }

    #[test]
    fn config_dirはあるがclaude_jsonが無ければlogged_out() {
        let dir = temp_dir("logged-out");
        assert_eq!(
            probe_status(Some(&dir.display().to_string())),
            AccountStatus::LoggedOut
        );
    }

    #[test]
    fn oauth_emailが読めればlogged_in() {
        let dir = temp_dir("logged-in");
        std::fs::write(
            dir.join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"user@example.com"}}"#,
        )
        .unwrap();
        assert_eq!(
            probe_status(Some(&dir.display().to_string())),
            AccountStatus::LoggedIn {
                email: "user@example.com".into()
            }
        );
    }

    #[test]
    fn claude_jsonがあってもメールが無ければlogged_out() {
        let dir = temp_dir("no-email");
        // ログアウト直後の claude.json（oauthAccount が消える）を模す
        std::fs::write(dir.join(".claude.json"), r#"{"numStartups":3}"#).unwrap();
        assert_eq!(
            probe_status(Some(&dir.display().to_string())),
            AccountStatus::LoggedOut
        );
    }

    #[test]
    fn 壊れたjsonはlogged_out扱いで止まらない() {
        let dir = temp_dir("broken");
        std::fs::write(dir.join(".claude.json"), "not json at all").unwrap();
        assert_eq!(
            probe_status(Some(&dir.display().to_string())),
            AccountStatus::LoggedOut
        );
    }

    #[test]
    fn 割り当ては役割ごとに引ける() {
        let assignments: ProfileAssignments = vec![
            (
                "default".into(),
                Some("personal".into()),
                Some("univ".into()),
            ),
            ("side".into(), Some("personal".into()), None),
        ];
        let (master, worker) = assignments_for("personal", &assignments);
        assert_eq!(master, vec!["default", "side"]);
        assert!(worker.is_empty());

        let (master, worker) = assignments_for("univ", &assignments);
        assert!(master.is_empty());
        assert_eq!(worker, vec!["default"]);

        // 未割り当てのアカウントは両方空
        let (master, worker) = assignments_for("unused", &assignments);
        assert!(master.is_empty() && worker.is_empty());
    }

    #[test]
    fn 壊れたエントリはinvalidとして一覧に残る() {
        // config_dir も inherit も無いエントリ（resolve_entry が Err を返す）
        let entry = AccountEntry::default();
        let resolved = crate::orchestrator::AccountsConfig {
            accounts: [("broken".to_string(), entry)].into_iter().collect(),
        };
        let list = resolved.list_resolved();
        let view = view_of("broken", &list[0].1, &Vec::new());
        assert_eq!(view.status.as_str(), "invalid");
        // 直し方が本文に含まれること（握り潰さない）
        match view.status {
            AccountStatus::Invalid { ref error } => assert!(error.contains("inherit")),
            _ => panic!("invalid のはず"),
        }
    }

    #[test]
    fn login_planはinheritを拒否し理由を返す() {
        let inherit = ResolvedAccount {
            name: "default-login".into(),
            config_dir: AccountConfigDir::Inherit,
            description: None,
            default_model: None,
            default_effort: None,
        };
        let err = login_plan(&inherit).unwrap_err();
        assert!(err.contains("inherit"));

        let base = temp_dir("login-plan");
        let fresh = base.join("new-account");
        let path_acct = ResolvedAccount {
            name: "univ".into(),
            config_dir: AccountConfigDir::Path(fresh.display().to_string()),
            description: None,
            default_model: None,
            default_effort: None,
        };
        let plan = login_plan(&path_acct).unwrap();
        assert!(plan.needs_create, "未作成の dir は作る計画になる");

        std::fs::create_dir_all(&fresh).unwrap();
        let plan = login_plan(&path_acct).unwrap();
        assert!(!plan.needs_create, "既存の dir は作り直さない");
    }

    #[test]
    fn 反映タイミングの案内は役割ごとに出る() {
        let both = AssignRoles {
            master: true,
            worker: true,
        };
        assert_eq!(both.applies_when().len(), 2);
        let none = AssignRoles::default();
        assert!(none.is_empty() && none.applies_when().is_empty());
    }
}
