//! 設定共有の現状検出（Issue #793）
//!
//! **何のためにあるか**: `tako setup` が設定共有（#513）について
//! 「配線済みか」「既に dotfiles 等で自力共有していないか」「gh でリポジトリ作成まで
//! 代行できるか」を**質問せずに**判断するための材料を集める。
//!
//! ## 不変条件
//!
//! - **読み取りだけ**。リポジトリ作成・配線・commit などの副作用は一切起こさない
//!   （`--yes` / 非 TTY でも安全に呼べる。#793 受け入れ条件 5）
//! - **案内の種類は純粋関数で決める**（[`guidance`]）。「配線済みなら勧誘しない」
//!   「質問は増やさない」を機械検証できる形にしておく
//! - **gh の出力は保持しない**。`gh auth status` はトークンの断片を出しうるので、
//!   終了コードだけを見て中身は捨てる（AGENTS.md の絶対ルール）
//!
//! ## なぜ「既存の git 運用」を見るのか
//!
//! `~/.claude` を dotfiles リポジトリへの symlink にしている利用者は珍しくない。
//! そこへ**別の**共有リポジトリを配線すると、同じ CLAUDE.md が 2 箇所で管理され、
//! `tako config pull` の書き込み（`config_io::atomic_write` の rename）が
//! symlink を実ファイルへ置き換えて既存の配線を壊す。
//! 検出して「既存リポジトリへ相乗りする」案を先に出せるようにする（#793 受け入れ条件 3）。

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use super::catalog::{self, Root};

/// `gh auth status` の待ち上限。トークン検証で通信するため、オフラインでも setup を止めない
const GH_TIMEOUT: Duration = Duration::from_secs(5);

/// gh CLI の状態。「リポジトリ作成まで代行してよいか」の判断材料
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GhStatus {
    /// gh が入っていない
    Missing,
    /// gh はあるがログインしていない
    Unauthenticated,
    /// gh でログイン済み
    Authenticated,
    /// 確認できなかった（オフライン・タイムアウト等）。代行しない側に倒す
    Unknown,
}

impl GhStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Unauthenticated => "unauthenticated",
            Self::Authenticated => "authenticated",
            Self::Unknown => "unknown",
        }
    }

    /// `gh repo create` でのリポジトリ作成を提案してよいか。
    /// 確定でログイン済みのときだけ true（不明なら提案しない）
    pub fn can_create_repo(self) -> bool {
        matches!(self, Self::Authenticated)
    }
}

/// 外部 git 管理下にある共有対象の見つかり方
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalKind {
    /// 実体が symlink で、リンク先がリポジトリ配下（dotfiles でよくある形）
    Symlink,
    /// 実体そのものがリポジトリ配下にある
    InRepo,
}

impl ExternalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Symlink => "symlink",
            Self::InRepo => "in_repo",
        }
    }
}

/// 既に外部の git（dotfiles 等）で管理されている共有対象
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExternalManaged {
    /// 共有ルート（`tako` / `claude`）。リポジトリ内で tako が使うサブディレクトリ名でもある
    pub root: &'static str,
    /// 実体のパス（表示用にホームを `~` へ）
    pub path: String,
    pub kind: ExternalKind,
    /// 管理元リポジトリ（表示用にホームを `~` へ）
    pub repo: String,
    /// 管理元リポジトリ内での位置（`claude` 等。リポジトリ直下なら空）
    pub repo_rel: String,
    /// 相乗りしたとき、tako の書き出し先が既存ファイルと**同じ場所**になるか。
    /// `repo_rel == root` から決まる派生値だが、setup-context.yaml を読む AI が
    /// そのまま判断できるよう明示的に持つ
    pub same_place: bool,
}

impl ExternalManaged {
    /// 相乗りしたとき、tako の書き出し先が既存ファイルと同じ場所になるか。
    ///
    /// tako はリポジトリの `<root>/…`（`claude/CLAUDE.md` 等）へ書き出すので、
    /// 既存の置き場がそこと一致していれば重複コピーは生まれない。
    /// 一致しなければ、相乗りしても同じ内容が 2 箇所に並ぶ（= 二重管理の注意が要る）
    pub fn shares_place(root: &str, repo_rel: &str) -> bool {
        repo_rel == root
    }
}

/// setup が出す案内の種類。**純粋関数で決める**（[`guidance`]）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Guidance {
    /// 配線済み。状態を示すだけで勧誘はしない（#793 受け入れ条件 4 = 冪等）
    Linked,
    /// 配線先が git リポジトリとして生きていない
    Broken,
    /// 未配線だが既存の git 運用がある。まず相乗りを提案する
    AdoptExisting,
    /// 未配線で既存運用も無い。新規作成を案内する
    Fresh,
}

impl Guidance {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Linked => "linked",
            Self::Broken => "broken",
            Self::AdoptExisting => "adopt_existing",
            Self::Fresh => "fresh",
        }
    }

    /// 案内（未配線の利用者を誘う文言）を出す場面か
    pub fn invites_setup(self) -> bool {
        matches!(self, Self::AdoptExisting | Self::Fresh)
    }
}

/// 設定共有まわりの現状
#[derive(Debug, Clone, Serialize)]
pub struct ShareEnvironment {
    /// `config-share.json` があるか
    pub linked: bool,
    /// 配線先（表示用にホームを `~` へ）
    pub repo: Option<String>,
    /// 配線先が生きた git リポジトリか（`linked` のときだけ意味を持つ）
    pub repo_ok: bool,
    /// 既に外部 git 管理下にある共有対象
    pub external: Vec<ExternalManaged>,
    /// gh CLI の状態。配線済みのときは判定しない（不要な待ち時間を作らない）= `None`
    pub gh: Option<GhStatus>,
}

impl ShareEnvironment {
    pub fn guidance(&self) -> Guidance {
        guidance(self)
    }

    /// gh でリポジトリ作成を代行してよいか（未判定なら false）
    pub fn gh_can_create_repo(&self) -> bool {
        self.gh.is_some_and(GhStatus::can_create_repo)
    }

    /// 案内する「次の一手」。既定値で済む引数は付けない（#322 の最簡形）
    pub fn next_command(&self) -> String {
        match self.guidance() {
            Guidance::Linked => "tako config status".into(),
            Guidance::Broken => "tako config link <パス|URL>".into(),
            Guidance::AdoptExisting => match self.external.first() {
                Some(found) => format!("tako config link {}", found.repo),
                None => "tako config link <パス|URL>".into(),
            },
            Guidance::Fresh => "tako config init".into(),
        }
    }
}

/// 案内の種類を決める。**入力だけで決まる**ので単体テストで固定できる
pub fn guidance(env: &ShareEnvironment) -> Guidance {
    if env.linked {
        return if env.repo_ok {
            Guidance::Linked
        } else {
            Guidance::Broken
        };
    }
    if env.external.is_empty() {
        Guidance::Fresh
    } else {
        Guidance::AdoptExisting
    }
}

/// 現状を調べる。**読み取りのみ**で、配線もリポジトリ作成もしない
pub fn detect() -> ShareEnvironment {
    // 壊れた config-share.json は「未配線」と同じ扱い（`apply_config_share` と揃える）
    let state = super::load_state().ok().flatten();
    let repo_path = state.as_ref().map(|s| PathBuf::from(&s.repo));
    let repo_ok = repo_path
        .as_ref()
        .is_some_and(|repo| repo.join(".git").exists());
    let home = home_dir();
    let external = detect_external(repo_path.as_deref());
    ShareEnvironment {
        linked: state.is_some(),
        repo: repo_path
            .as_deref()
            .map(|repo| abbreviate_home(repo, home.as_deref())),
        repo_ok,
        // 配線済みなら gh の出番は無い。判定に行かない（`None` = 未判定）
        gh: state.is_none().then(detect_gh),
        external,
    }
}

/// 共有対象のうち、外部 git 管理下にあるものを列挙する。
/// `exclude_repo`（= 自分の共有リポジトリ）配下は「外部」ではないので除く
pub fn detect_external(exclude_repo: Option<&Path>) -> Vec<ExternalManaged> {
    let home = home_dir();
    let mut found = Vec::new();
    for root in Root::all() {
        let Some(dir) = root.live_dir() else { continue };
        // ルート自体が管理下なら配下も同じリポジトリ。子は調べない（git 起動を増やさない）
        if let Some(entry) = probe_path(root.as_str(), &dir, home.as_deref(), exclude_repo) {
            found.push(entry);
            continue;
        }
        // ルートが管理外なら、実体が配下にある子も管理外。
        // 残る可能性は「ファイル単位の symlink がリポジトリを指している」形だけなので、
        // symlink の子に絞って調べる
        for entry in catalog::shared_entries(*root) {
            let path = dir.join(entry.path.trim_end_matches('/'));
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.file_type().is_symlink() {
                continue;
            }
            if let Some(found_entry) =
                probe_path(root.as_str(), &path, home.as_deref(), exclude_repo)
            {
                found.push(found_entry);
            }
        }
    }
    found
}

/// 1 つのパスが外部 git 管理下かを調べる。実行するのは
/// `git rev-parse --show-toplevel` だけで、リポジトリの内容には触れない
pub fn probe_path(
    root: &'static str,
    path: &Path,
    home: Option<&Path>,
    exclude_repo: Option<&Path>,
) -> Option<ExternalManaged> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    let kind = if meta.file_type().is_symlink() {
        ExternalKind::Symlink
    } else {
        ExternalKind::InRepo
    };
    // `repo_root` は git が返す物理パス。突き合わせる側も実体へ寄せておかないと
    // `/var` → `/private/var` のような差で `strip_prefix` が外れる
    let resolved = std::fs::canonicalize(path).ok()?;
    let repo = tako_core::git::repo_root(&resolved)?;
    if exclude_repo.is_some_and(|excluded| same_dir(excluded, &repo)) {
        return None;
    }
    let repo_rel = resolved
        .strip_prefix(&repo)
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    Some(ExternalManaged {
        root,
        path: abbreviate_home(path, home),
        kind,
        repo: abbreviate_home(&repo, home),
        same_place: ExternalManaged::shares_place(root, &repo_rel),
        repo_rel,
    })
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(a) == canon(b)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// 表示用にホーム配下を `~/…` へ縮める（ホームパスをそのまま見せない。#513 の可搬表記と同じ形）
fn abbreviate_home(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rel) if rel.as_os_str().is_empty() => "~".to_string(),
        Some(rel) => format!("~/{}", rel.to_string_lossy().replace('\\', "/")),
        None => path.to_string_lossy().to_string(),
    }
}

fn detect_gh() -> GhStatus {
    let Some(bin) = find_gh() else {
        return GhStatus::Missing;
    };
    match run_status(&bin, &["auth", "status"], GH_TIMEOUT) {
        Some(true) => GhStatus::Authenticated,
        Some(false) => GhStatus::Unauthenticated,
        None => GhStatus::Unknown,
    }
}

fn find_gh() -> Option<String> {
    if command_succeeds("gh", &["--version"]) {
        return Some("gh".to_string());
    }
    // GUI 起動や Homebrew の PATH 差異に対応（setup の find_command と同じ手口）
    #[cfg(unix)]
    {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".into());
        let output = std::process::Command::new(shell)
            .args(["-l", "-c", "command -v gh"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() && Path::new(&path).is_file() {
                return Some(path);
            }
        }
    }
    None
}

fn command_succeeds(bin: &str, args: &[&str]) -> bool {
    std::process::Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// 終了コードだけを見るコマンド実行（上限つき）。
/// **出力は捨てる**: `gh auth status` はトークンの断片を出しうるため保持しない。
/// 上限を超えたら kill して `None`（= 判定不能）を返す
fn run_status(bin: &str, args: &[&str], limit: Duration) -> Option<bool> {
    let mut child = std::process::Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status.success()),
            Ok(None) => {
                if start.elapsed() >= limit {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_with(linked: bool, repo_ok: bool, external: Vec<ExternalManaged>) -> ShareEnvironment {
        ShareEnvironment {
            linked,
            repo: linked.then(|| "~/tako-config-sync".to_string()),
            repo_ok,
            external,
            gh: (!linked).then_some(GhStatus::Missing),
        }
    }

    fn managed(root: &'static str, repo_rel: &str) -> ExternalManaged {
        ExternalManaged {
            root,
            path: format!("~/.{root}"),
            kind: ExternalKind::Symlink,
            repo: "~/dotfiles".to_string(),
            same_place: ExternalManaged::shares_place(root, repo_rel),
            repo_rel: repo_rel.to_string(),
        }
    }

    #[test]
    fn 配線済みなら勧誘しない() {
        let env = env_with(true, true, vec![]);
        assert_eq!(env.guidance(), Guidance::Linked);
        assert!(
            !env.guidance().invites_setup(),
            "#793 受け入れ条件 4（冪等）"
        );
        assert_eq!(env.next_command(), "tako config status");
    }

    #[test]
    fn 配線済みでも既存の外部管理があれば勧誘しない() {
        let env = env_with(true, true, vec![managed("claude", "claude")]);
        assert_eq!(env.guidance(), Guidance::Linked);
        assert!(!env.guidance().invites_setup());
    }

    #[test]
    fn 配線先が壊れていれば繋ぎ直しを案内する() {
        let env = env_with(true, false, vec![]);
        assert_eq!(env.guidance(), Guidance::Broken);
        assert!(!env.guidance().invites_setup(), "壊れた配線の勧誘はしない");
        assert_eq!(env.next_command(), "tako config link <パス|URL>");
    }

    #[test]
    fn 未配線で既存運用が無ければ新規作成を案内する() {
        let env = env_with(false, false, vec![]);
        assert_eq!(env.guidance(), Guidance::Fresh);
        assert!(env.guidance().invites_setup());
        assert_eq!(env.next_command(), "tako config init");
    }

    #[test]
    fn 未配線で既存運用があれば相乗りを案内する() {
        let env = env_with(false, false, vec![managed("claude", "claude")]);
        assert_eq!(env.guidance(), Guidance::AdoptExisting);
        assert_eq!(env.next_command(), "tako config link ~/dotfiles");
    }

    #[test]
    fn 置き場が一致するかで重複の有無が分かれる() {
        assert!(
            managed("claude", "claude").same_place,
            "tako は claude/ へ書くので、既存も claude/ なら同じファイルになる"
        );
        assert!(
            !managed("claude", "home/.claude").same_place,
            "置き場が違えば相乗りしても内容が 2 箇所に並ぶ"
        );
        assert!(
            !managed("claude", "").same_place,
            "リポジトリ直下が実体なら、tako の claude/ は入れ子の別コピーになる"
        );
    }

    #[test]
    fn gh未判定ならリポジトリ作成を代行しない() {
        let mut env = env_with(false, false, vec![]);
        env.gh = None;
        assert!(!env.gh_can_create_repo());
        env.gh = Some(GhStatus::Unknown);
        assert!(!env.gh_can_create_repo(), "不明なら代行しない側へ倒す");
        env.gh = Some(GhStatus::Unauthenticated);
        assert!(!env.gh_can_create_repo());
        env.gh = Some(GhStatus::Authenticated);
        assert!(env.gh_can_create_repo());
    }

    // --- 実ファイルシステムでの検出（symlink + 実 git リポジトリ）---

    fn temp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("tako-793-{tag}-{}", std::process::id()))
    }

    fn remove_temp_dir(dir: &Path) {
        assert!(
            dir.starts_with(std::env::temp_dir()),
            "一時ディレクトリ以外を削除しようとしている: {}",
            dir.display()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    fn git_init(repo: &Path) {
        std::fs::create_dir_all(repo).expect("mkdir");
        let run = |args: &[&str]| {
            std::process::Command::new(tako_core::git::git_bin())
                .args(args)
                .current_dir(repo)
                .output()
                .expect("git");
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "test"]);
    }

    #[cfg(unix)]
    #[test]
    fn dotfilesへのsymlinkを外部管理として検出する() {
        let dir = temp_dir("symlink");
        remove_temp_dir(&dir);
        let repo = dir.join("dotfiles");
        git_init(&repo);
        std::fs::create_dir_all(repo.join("claude")).expect("mkdir");
        std::fs::write(repo.join("claude/CLAUDE.md"), "# rules\n").expect("write");
        let live = dir.join("home/.claude");
        std::fs::create_dir_all(live.parent().unwrap()).expect("mkdir");
        std::os::unix::fs::symlink(repo.join("claude"), &live).expect("symlink");

        let found = probe_path("claude", &live, Some(&dir.join("home")), None)
            .expect("symlink 先がリポジトリ配下なら検出される");
        assert_eq!(found.kind, ExternalKind::Symlink);
        assert_eq!(found.path, "~/.claude");
        assert_eq!(found.repo_rel, "claude");
        assert!(found.same_place, "既存も claude/ なので重複しない");
        remove_temp_dir(&dir);
    }

    #[test]
    fn リポジトリ配下の実体も外部管理として検出する() {
        let dir = temp_dir("inrepo");
        remove_temp_dir(&dir);
        let repo = dir.join("dotfiles");
        git_init(&repo);
        let live = repo.join("home/.claude");
        std::fs::create_dir_all(&live).expect("mkdir");

        let found = probe_path("claude", &live, None, None).expect("リポジトリ配下なら検出される");
        assert_eq!(found.kind, ExternalKind::InRepo);
        assert_eq!(found.repo_rel, "home/.claude");
        assert!(!found.same_place, "置き場が違うので相乗りは重複になる");
        remove_temp_dir(&dir);
    }

    #[test]
    fn 自分の共有リポジトリは外部管理に数えない() {
        let dir = temp_dir("exclude");
        remove_temp_dir(&dir);
        let repo = dir.join("tako-config-sync");
        git_init(&repo);
        let live = repo.join("claude");
        std::fs::create_dir_all(&live).expect("mkdir");

        assert!(
            probe_path("claude", &live, None, Some(&repo)).is_none(),
            "配線先のリポジトリを「別運用がある」と誤検出しない"
        );
        assert!(
            probe_path("claude", &live, None, None).is_some(),
            "除外を外せば検出される（検出そのものは働いている）"
        );
        remove_temp_dir(&dir);
    }

    #[test]
    fn git管理外のディレクトリは検出しない() {
        let dir = temp_dir("plain");
        remove_temp_dir(&dir);
        let live = dir.join("home/.claude");
        std::fs::create_dir_all(&live).expect("mkdir");
        assert!(probe_path("claude", &live, None, None).is_none());
        assert!(
            probe_path("claude", &dir.join("home/.missing"), None, None).is_none(),
            "存在しないパスは検出対象外"
        );
        remove_temp_dir(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn 上限を超えたコマンドは判定不能になる() {
        // 実行時間が上限を超えたら kill して None（= gh が Unknown）へ落ちる
        assert_eq!(
            run_status("sleep", &["5"], Duration::from_millis(150)),
            None
        );
        assert_eq!(
            run_status("true", &[], Duration::from_secs(5)),
            Some(true),
            "終了コードは素直に反映される"
        );
        assert_eq!(
            run_status("false", &[], Duration::from_secs(5)),
            Some(false)
        );
    }
}
