//! config_share — AI 系設定の git ベース共有（Issue #513）
//!
//! `~/.claude` と tako のデータディレクトリに散らばる「AI 系の記憶ファイル」を、
//! 1 つの git リポジトリでデバイス間（mac ⇔ Windows）共有する。
//!
//! ## 全体像
//!
//! ```text
//!   実体（デバイスごと）                共有リポジトリ（git）
//!   <data_dir>/settings.json     ──push──▶  tako/settings.json
//!   <data_dir>/orchestrator/…    ◀──pull──   tako/orchestrator/…
//!   ~/.claude/CLAUDE.md                      claude/CLAUDE.md
//! ```
//!
//! - **何を共有するか**は [`catalog`] のホワイトリストが正本。未分類は共有されない
//! - **中身のデバイス依存**は [`portable`] が吸収する（ホームパスの `~` トークン化、
//!   `env` / `config_dir` 等ローカルフィールドの切り分け）
//! - **秘匿情報**は [`guard`] が書き出し直前に再検査して push を止める
//!
//! ## symlink ではなくコピー同期にした理由
//!
//! 「実体をリポジトリへの symlink にすれば同期不要」は最初に検討して**捨てた**。
//!
//! 1. tako 自身の設定書き込みは `config_io::atomic_write`（tmp → rename）で、
//!    **rename は symlink を実ファイルで置き換えてしまう**。最初の設定変更で配線が壊れる
//! 2. symlink 越しだとローカルフィールドの切り分けもホームパスの可搬化もできず、
//!    実パスやアカウントの資格情報パスがそのままコミットされる
//! 3. Windows の symlink は開発者モードか管理者権限が要る
//!
//! よって明示的な `push` / `pull` にした。差分は `status` で見える。

pub mod catalog;
pub mod env;
pub mod guard;
pub mod portable;

use catalog::{Class, Entry, Root};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// 共有リポジトリ内のメタデータファイル
const MANIFEST_FILE: &str = "manifest.yaml";
const README_FILE: &str = "README.md";
const GITIGNORE_FILE: &str = ".gitignore";

/// 配線状態の保存先ファイル名（`<data_dir>` 直下。カタログでは Local 分類）
const STATE_FILE: &str = "config-share.json";

/// 既定の共有リポジトリ配置先（ホーム直下。ユーザーが git で触りやすい場所に置く）
const DEFAULT_REPO_DIR: &str = "tako-config-sync";

/// 配線状態（このデバイスがどのリポジトリに繋がっているか）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShareState {
    /// 共有リポジトリの絶対パス
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_at: Option<String>,
}

fn data_dir() -> Result<PathBuf, String> {
    tako_core::paths::data_dir().ok_or_else(|| "データディレクトリを解決できません".to_string())
}

fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| "ホームディレクトリを解決できません（$HOME 未設定）".to_string())
}

/// 配線状態ファイルのパス
pub fn state_path() -> Result<PathBuf, String> {
    Ok(data_dir()?.join(STATE_FILE))
}

pub fn load_state() -> Result<Option<ShareState>, String> {
    let path = state_path()?;
    if !path.is_file() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("配線状態の読み取りに失敗 ({}): {e}", path.display()))?;
    serde_json::from_str(&content)
        .map(Some)
        .map_err(|e| format!("配線状態のパースに失敗 ({}): {e}", path.display()))
}

fn save_state(state: &ShareState) -> Result<(), String> {
    let path = state_path()?;
    let content = serde_json::to_string_pretty(state)
        .map_err(|e| format!("配線状態のシリアライズに失敗: {e}"))?;
    crate::config_io::atomic_write(&path, &content)
}

/// 配線済みリポジトリを取得する。未配線なら案内つきエラー
fn linked_repo() -> Result<PathBuf, String> {
    let state = load_state()?.ok_or_else(|| {
        "設定共有が未配線です。`tako config init` で新規リポジトリを作るか、\n\
         `tako config link <パス|URL>` で既存リポジトリに繋いでください"
            .to_string()
    })?;
    let repo = PathBuf::from(&state.repo);
    if !repo.join(".git").exists() {
        return Err(format!(
            "配線先が git リポジトリではありません: {}\n\
             `tako config link <パス|URL>` で繋ぎ直してください",
            repo.display()
        ));
    }
    Ok(repo)
}

// --- git 実行 ---------------------------------------------------------------

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    // #586: GUI プロセス（dispatch / MCP）から到達するのでコンソールウィンドウを出させない
    let output = tako_core::platform::process::no_console_window(&mut std::process::Command::new(
        tako_core::git::git_bin(),
    ))
    .current_dir(repo)
    .args(args)
    .output()
    .map_err(|e| format!("git の実行に失敗 ({}): {e}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "git {} が失敗しました:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// 失敗を許容する git（状態照会用）
fn git_opt(repo: &Path, args: &[&str]) -> Option<String> {
    git(repo, args).ok()
}

fn remote_url(repo: &Path) -> Option<String> {
    git_opt(repo, &["remote", "get-url", "origin"]).filter(|s| !s.is_empty())
}

// --- 共有対象の収集 ----------------------------------------------------------

/// 共有対象の 1 ファイル
#[derive(Debug, Clone)]
pub struct SharedFile {
    pub root: Root,
    /// ルートからの相対パス（区切りは `/`）
    pub rel: String,
    /// 実体の絶対パス
    pub live: PathBuf,
    /// リポジトリ内の相対パス（`tako/orchestrator/projects.yaml`）
    pub repo_rel: String,
    pub entry: &'static Entry,
}

/// 実体側を走査して共有対象を集める。
/// 併せて「未分類だったパス」も返す（status で可視化するため。共有はしない）
pub fn collect_shared_files() -> (Vec<SharedFile>, Vec<String>) {
    let mut files = Vec::new();
    let mut unclassified = Vec::new();
    for &root in Root::all() {
        let Some(dir) = root.live_dir() else {
            continue;
        };
        let mut found = Vec::new();
        walk(root, &dir, "", &mut found);
        for rel in found {
            match catalog::classify(root, &rel) {
                Some(entry) if entry.class.is_shared() => files.push(SharedFile {
                    root,
                    repo_rel: format!("{}/{}", root.as_str(), rel),
                    live: dir.join(&rel),
                    rel,
                    entry,
                }),
                Some(_) => {}
                None => unclassified.push(format!("{}/{}", root.as_str(), rel)),
            }
        }
    }
    files.sort_by(|a, b| a.repo_rel.cmp(&b.repo_rel));
    unclassified.sort();
    (files, unclassified)
}

/// 走査するファイル数の上限。`~/.claude` 配下には未分類の巨大ディレクトリが
/// 生えうるので、診断のための列挙が実運用の邪魔にならないところで打ち切る
const WALK_LIMIT: usize = 5000;

/// ディレクトリを再帰的に走査してファイルの相対パスを集める。
///
/// - symlink は辿らない（リポジトリ外へ抜けて意図しないファイルを共有するのを防ぐ）
/// - 共有しないと分類済みのディレクトリは**中へ入らない**
///   （`~/.claude/projects` のような巨大な会話履歴を毎回舐めない）
fn walk(root: Root, dir: &Path, prefix: &str, out: &mut Vec<String>) {
    if out.len() >= WALK_LIMIT {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= WALK_LIMIT {
            return;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            // ディレクトリ配下を代表する架空のファイルで分類を引く
            let probe = format!("{rel}/.probe");
            if catalog::classify(root, &probe).is_some_and(|e| !e.class.is_shared()) {
                continue;
            }
            walk(root, &entry.path(), &rel, out);
            continue;
        }
        if meta.is_file() {
            out.push(rel);
        }
    }
}

/// 実体の内容を共有用に変換する（ローカルフィールド除去 → ホームパスの可搬化）
pub fn export_content(entry: &Entry, live: &Path) -> Result<String, String> {
    let raw = std::fs::read_to_string(live)
        .map_err(|e| format!("読み取りに失敗 ({}): {e}", live.display()))?;
    let stripped = match portable::format_of(live) {
        Some(format) => portable::strip_local_fields(&raw, format, entry.local_fields)?,
        // 構造を持たないファイル（markdown 等）はフィールド操作の対象外
        None => raw,
    };
    let home = home_dir()?;
    Ok(portable::to_portable(&stripped, &home.to_string_lossy()))
}

/// 共有内容を実体用に変換する（可搬パスの展開 → ローカルフィールドの復元）
pub fn import_content(entry: &Entry, shared: &str, live: &Path) -> Result<String, String> {
    let Some(format) = portable::format_of(live) else {
        // markdown 等は `~/…` のままが正しい表記なので展開しない（portable の設計判断）
        return Ok(shared.to_string());
    };
    let home = home_dir()?;
    let expanded = portable::from_portable(shared, &home.to_string_lossy(), portable::native_sep());
    let local = std::fs::read_to_string(live).ok();
    portable::restore_local_fields(&expanded, local.as_deref(), format, entry.local_fields)
}

// --- マニフェスト ------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Manifest {
    #[serde(default = "manifest_schema")]
    schema: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    generated_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    generated_at: Option<String>,
    /// tako が書き出したファイル（リポジトリ相対）。
    /// **削除の伝播に使う**: 前回あって今回ないものだけを消す。
    /// 利用者が手で置いたファイルには触らない
    #[serde(default)]
    files: Vec<String>,
}

fn manifest_schema() -> u32 {
    1
}

fn load_manifest(repo: &Path) -> Manifest {
    std::fs::read_to_string(repo.join(MANIFEST_FILE))
        .ok()
        .and_then(|s| serde_yaml::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_manifest(repo: &Path, files: &[String]) -> Result<(), String> {
    let manifest = Manifest {
        schema: manifest_schema(),
        generated_by: Some(format!("tako {}", env!("CARGO_PKG_VERSION"))),
        generated_at: Some(crate::sessions::now_iso()),
        files: files.to_vec(),
    };
    let content =
        serde_yaml::to_string(&manifest).map_err(|e| format!("manifest の生成に失敗: {e}"))?;
    std::fs::write(repo.join(MANIFEST_FILE), content)
        .map_err(|e| format!("manifest の書き込みに失敗: {e}"))
}

/// 共有リポジトリの README。何が入っていて何が入っていないかを人が読める形で残す
fn readme_text() -> String {
    let mut out = String::from(
        "# tako config share\n\n\
         `tako config` が管理する AI 系設定の共有リポジトリです（tako Issue #513）。\n\
         This repository is managed by `tako config` and holds shared AI configuration.\n\n\
         ## 使い方 / Usage\n\n\
         ```sh\n\
         tako config link <このリポジトリのパス または URL>\n\
         tako config pull   # 取り込む / apply to this device\n\
         tako config push   # 書き出してコミット / export and commit\n\
         tako config status # 差分を見る / show differences\n\
         ```\n\n\
         ## 入っているもの / What is here\n\n",
    );
    for &root in Root::all() {
        for entry in catalog::shared_entries(root) {
            out.push_str(&format!(
                "- `{}/{}` — {}\n",
                root.as_str(),
                entry.path,
                entry.note.ja()
            ));
        }
    }
    out.push_str(
        "\n## 入らないもの / What never enters\n\n\
         秘匿情報（token・credentials）とマシンローカル状態（layout.json・sessions.yaml 等）は\n\
         tako 側のホワイトリストで構造的に除外されます。分類の全量は `tako config list` で確認できます。\n\
         Secrets and machine-local state are excluded by an allow-list on the tako side;\n\
         run `tako config list` for the full classification table.\n\n\
         絶対パスはホーム部分が `~` に置き換わって保存されます（mac ⇔ Windows 可搬性）。\n\
         Absolute paths are stored with the home prefix replaced by `~` for portability.\n",
    );
    out
}

/// 最終防壁の .gitignore。ホワイトリストを素通りしたものが万一あっても、
/// ここに列挙したものは git に載らない
fn gitignore_text() -> String {
    String::from(
        "# tako config share — 最終防壁 / last-resort guard\n\
         # 共有対象は tako が明示的に書き出します。ここは取り違えへの保険です。\n\
         token\n\
         *.token\n\
         control.json\n\
         relay_secret\n\
         machine_id\n\
         .claude.json\n\
         .credentials.json\n\
         credentials.json\n\
         layout.json*\n\
         sessions.yaml\n\
         workers.yaml\n\
         *.log\n\
         *.lock\n\
         *.bak\n\
         *.bak.*\n\
         .DS_Store\n",
    )
}

// --- 操作 --------------------------------------------------------------------

/// 共有分類の一覧（機械可読）。`tako config list` / MCP がそのまま返す
pub fn list() -> Value {
    list_in(tako_core::i18n::lang())
}

/// 言語を明示しての一覧（テストが言語グローバルに触らずに済むように分ける。#608）
pub fn list_in(lang: tako_core::i18n::Lang) -> Value {
    let entries: Vec<Value> = catalog::CATALOG
        .iter()
        .map(|e| {
            json!({
                "root": e.root.as_str(),
                "path": e.path,
                "class": e.class.as_str(),
                "note": e.note.text_in(lang),
                "local_fields": e.local_fields,
            })
        })
        .collect();
    let count = |c: Class| catalog::CATALOG.iter().filter(|e| e.class == c).count();
    json!({
        "entries": entries,
        "counts": {
            "shared": count(Class::Shared),
            "local": count(Class::Local),
            "secret": count(Class::Secret),
        },
        "unclassified_policy": "not_shared",
    })
}

/// 新しい共有リポジトリを作って配線する。
/// `remote` を渡すと origin として登録し、初回 push まで行う
pub fn init(path: Option<&str>, remote: Option<&str>) -> Result<Value, String> {
    let repo = resolve_repo_path(path)?;
    if repo.join(".git").exists() {
        // すでに git リポジトリなら init ではなく link 相当（作り直さない）
        save_state(&ShareState {
            repo: repo.to_string_lossy().to_string(),
            linked_at: Some(crate::sessions::now_iso()),
        })?;
        return Ok(json!({
            "action": "linked_existing",
            "repo": repo.to_string_lossy(),
            "hint": "既存の git リポジトリだったので配線だけ行いました。`tako config pull` で取り込めます",
        }));
    }
    if repo.exists()
        && std::fs::read_dir(&repo)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false)
    {
        return Err(format!(
            "{} は空でないディレクトリで、git リポジトリでもありません。\n\
             別の場所を --path で指定するか、そこで git init してから `tako config link` してください",
            repo.display()
        ));
    }
    std::fs::create_dir_all(&repo).map_err(|e| format!("ディレクトリの作成に失敗: {e}"))?;
    git(&repo, &["init"])?;
    if let Some(url) = remote {
        git(&repo, &["remote", "add", "origin", url])?;
    }
    save_state(&ShareState {
        repo: repo.to_string_lossy().to_string(),
        linked_at: Some(crate::sessions::now_iso()),
    })?;
    // 作った直後に中身を入れる（「作ったが空」で放置しない = 要件 4 の一気通貫）
    let pushed = push(Some("[機能追加] tako 設定共有を開始"), remote.is_none())?;
    Ok(json!({
        "action": "created",
        "repo": repo.to_string_lossy(),
        "remote": remote,
        "push": pushed,
    }))
}

/// 既存リポジトリに配線する。`target` はローカルパスまたは git URL
pub fn link(target: &str, path: Option<&str>) -> Result<Value, String> {
    let target = target.trim();
    if target.is_empty() {
        return Err("リポジトリのパスまたは URL を指定してください".into());
    }
    let repo = if is_git_url(target) {
        let dest = resolve_repo_path(path)?;
        if dest.exists()
            && std::fs::read_dir(&dest)
                .map(|mut d| d.next().is_some())
                .unwrap_or(false)
        {
            return Err(format!(
                "clone 先 {} が空ではありません。--path で別の場所を指定してください",
                dest.display()
            ));
        }
        let parent = dest
            .parent()
            .ok_or_else(|| "clone 先の親ディレクトリを解決できません".to_string())?;
        std::fs::create_dir_all(parent).map_err(|e| format!("ディレクトリの作成に失敗: {e}"))?;
        git(parent, &["clone", target, &dest.to_string_lossy()])?;
        dest
    } else {
        let repo = expand_path(target)?;
        if !repo.is_dir() {
            return Err(format!("ディレクトリが見つかりません: {}", repo.display()));
        }
        if !repo.join(".git").exists() {
            return Err(format!(
                "{} は git リポジトリではありません。\n\
                 新規に作るなら `tako config init --path {}` を使ってください",
                repo.display(),
                repo.display()
            ));
        }
        repo
    };
    save_state(&ShareState {
        repo: repo.to_string_lossy().to_string(),
        linked_at: Some(crate::sessions::now_iso()),
    })?;
    let has_content = repo.join(MANIFEST_FILE).is_file();
    Ok(json!({
        "action": "linked",
        "repo": repo.to_string_lossy(),
        "remote": remote_url(&repo),
        "hint": if has_content {
            "`tako config pull` でこのデバイスへ取り込めます"
        } else {
            "`tako config push` でこのデバイスの設定を書き出せます"
        },
    }))
}

/// 配線を外す（リポジトリ自体は消さない）
pub fn unlink() -> Result<Value, String> {
    let path = state_path()?;
    let state = load_state()?;
    if path.is_file() {
        std::fs::remove_file(&path).map_err(|e| format!("配線状態の削除に失敗: {e}"))?;
    }
    Ok(json!({
        "action": "unlinked",
        "repo": state.map(|s| s.repo),
        "hint": "リポジトリ自体は残っています。再接続は `tako config link <パス>`",
    }))
}

fn is_git_url(target: &str) -> bool {
    target.contains("://") || target.starts_with("git@") || target.ends_with(".git")
}

fn expand_path(raw: &str) -> Result<PathBuf, String> {
    let expanded = crate::orchestrator::expand_tilde(raw);
    let path = PathBuf::from(&expanded);
    if path.is_absolute() {
        return Ok(path);
    }
    std::env::current_dir()
        .map(|d| d.join(path))
        .map_err(|e| format!("カレントディレクトリを解決できません: {e}"))
}

fn resolve_repo_path(path: Option<&str>) -> Result<PathBuf, String> {
    match path {
        Some(p) => expand_path(p),
        None => Ok(home_dir()?.join(DEFAULT_REPO_DIR)),
    }
}

/// 現在の共有状態
pub fn status() -> Result<Value, String> {
    let Some(state) = load_state()? else {
        return Ok(json!({
            "linked": false,
            "hint": "`tako config init` で共有リポジトリを作れます（既存リポジトリは `tako config link <パス|URL>`）",
        }));
    };
    let repo = PathBuf::from(&state.repo);
    let repo_ok = repo.join(".git").exists();
    let (files, unclassified) = collect_shared_files();

    let mut entries = Vec::new();
    let mut summary = std::collections::BTreeMap::<&str, u32>::new();
    let mut non_portable = Vec::new();
    let mut seen_repo_paths = std::collections::BTreeSet::new();

    for file in &files {
        seen_repo_paths.insert(file.repo_rel.clone());
        let exported = export_content(file.entry, &file.live);
        let state_str = match (&exported, repo_ok) {
            (Err(_), _) => "error",
            (Ok(content), true) => {
                for p in portable::non_portable_absolute_paths(content) {
                    if !non_portable.contains(&p) {
                        non_portable.push(p);
                    }
                }
                match std::fs::read_to_string(repo.join(&file.repo_rel)) {
                    Ok(existing) if existing == *content => "same",
                    Ok(_) => "differs",
                    Err(_) => "local_only",
                }
            }
            (Ok(_), false) => "local_only",
        };
        *summary.entry(state_str).or_default() += 1;
        entries.push(json!({
            "path": file.repo_rel,
            "state": state_str,
            "error": exported.err(),
        }));
    }

    // リポジトリにしかないもの（別デバイスで追加された = pull 待ち）
    let mut repo_only = Vec::new();
    let mut untracked = Vec::new();
    if repo_ok {
        for repo_rel in walk_repo(&repo) {
            if seen_repo_paths.contains(&repo_rel) {
                continue;
            }
            match classify_repo_path(&repo_rel) {
                Some((_, entry)) if entry.class.is_shared() => repo_only.push(repo_rel),
                // tako が管理しないファイル。触らないが見えるようにする
                _ => untracked.push(repo_rel),
            }
        }
    }
    *summary.entry("repo_only").or_default() += repo_only.len() as u32;
    for path in &repo_only {
        entries.push(json!({ "path": path, "state": "repo_only" }));
    }

    let excluded_local = catalog::CATALOG
        .iter()
        .filter(|e| e.class == Class::Local)
        .count();
    let excluded_secret = catalog::CATALOG
        .iter()
        .filter(|e| e.class == Class::Secret)
        .count();

    Ok(json!({
        "linked": true,
        "repo": state.repo,
        "repo_is_git": repo_ok,
        "remote": repo_ok.then(|| remote_url(&repo)).flatten(),
        "branch": repo_ok.then(|| git_opt(&repo, &["rev-parse", "--abbrev-ref", "HEAD"])).flatten(),
        "dirty": repo_ok
            .then(|| git_opt(&repo, &["status", "--porcelain"]).map(|s| !s.is_empty()))
            .flatten(),
        "files": entries,
        "summary": summary,
        "excluded": { "local": excluded_local, "secret": excluded_secret },
        "unclassified": unclassified,
        "untracked_in_repo": untracked,
        "non_portable_paths": non_portable,
    }))
}

/// リポジトリ内の管理対象サブツリー（`tako/` と `claude/`）を走査する
fn walk_repo(repo: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for &root in Root::all() {
        let dir = repo.join(root.as_str());
        let mut found = Vec::new();
        walk_plain(&dir, "", &mut found);
        for rel in found {
            out.push(format!("{}/{}", root.as_str(), rel));
        }
    }
    out.sort();
    out
}

/// カタログ判定を挟まない素朴な走査（リポジトリ側は tako の分類ではなく実体をそのまま見る）
fn walk_plain(dir: &Path, prefix: &str, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".git" {
            continue;
        }
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            walk_plain(&entry.path(), &rel, out);
        } else if meta.is_file() {
            out.push(rel);
        }
    }
}

/// リポジトリ相対パス（`tako/orchestrator/projects.yaml`）を分類する
fn classify_repo_path(repo_rel: &str) -> Option<(Root, &'static Entry)> {
    let (root_name, rest) = repo_rel.split_once('/')?;
    let root = Root::parse(root_name)?;
    catalog::classify(root, rest).map(|e| (root, e))
}

/// 実体 → リポジトリ。変換・検査・コミット・push まで
pub fn push(message: Option<&str>, no_push: bool) -> Result<Value, String> {
    let repo = linked_repo()?;
    let (files, unclassified) = collect_shared_files();

    // 1. 変換して秘匿検査（1 件でも引っかかったら何も書かずに止める）
    let mut staged: Vec<(String, String)> = Vec::new();
    let mut findings = Vec::new();
    for file in &files {
        let content = export_content(file.entry, &file.live)?;
        findings.extend(guard::scan(&file.repo_rel, &content));
        staged.push((file.repo_rel.clone(), content));
    }
    if !findings.is_empty() {
        return Err(guard::describe(&findings));
    }

    // 2. 書き出し
    let mut written = Vec::new();
    for (repo_rel, content) in &staged {
        let dest = repo.join(repo_rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("ディレクトリの作成に失敗 ({}): {e}", parent.display()))?;
        }
        let changed = std::fs::read_to_string(&dest)
            .map(|c| c != *content)
            .unwrap_or(true);
        if changed {
            std::fs::write(&dest, content)
                .map_err(|e| format!("書き込みに失敗 ({}): {e}", dest.display()))?;
        }
        written.push(repo_rel.clone());
    }

    // 3. 前回の manifest にあって今回消えたものだけ削除する（手置きファイルは触らない）
    let previous = load_manifest(&repo);
    let current: std::collections::BTreeSet<&String> = written.iter().collect();
    let mut removed = Vec::new();
    for old in &previous.files {
        if current.contains(old) {
            continue;
        }
        let path = repo.join(old);
        if path.is_file() {
            std::fs::remove_file(&path)
                .map_err(|e| format!("削除に失敗 ({}): {e}", path.display()))?;
            removed.push(old.clone());
        }
    }

    // 4. メタデータ
    write_manifest(&repo, &written)?;
    std::fs::write(repo.join(README_FILE), readme_text())
        .map_err(|e| format!("README の書き込みに失敗: {e}"))?;
    std::fs::write(repo.join(GITIGNORE_FILE), gitignore_text())
        .map_err(|e| format!("gitignore の書き込みに失敗: {e}"))?;

    // 5. tako が書いたパスだけを stage する。
    //    `git add -A` にすると、利用者が手で置いたファイル（秘匿かもしれない）まで
    //    巻き込んでコミットしてしまう
    let mut pathspec: Vec<String> = written.clone();
    pathspec.extend(removed.iter().cloned());
    pathspec.push(MANIFEST_FILE.to_string());
    pathspec.push(README_FILE.to_string());
    pathspec.push(GITIGNORE_FILE.to_string());
    let mut args: Vec<&str> = vec!["add", "-A", "--"];
    args.extend(pathspec.iter().map(String::as_str));
    git(&repo, &args)?;

    let has_staged = git(&repo, &["diff", "--cached", "--name-only"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let mut commit_hash = None;
    if has_staged {
        let msg = message.unwrap_or("[改善] tako 設定共有を更新");
        git(&repo, &["commit", "-m", msg]).map_err(|e| {
            // 新しいデバイスでよく踏む。git のエラーは長いので、やることを 1 行で足す
            if e.contains("tell me who you are") || e.contains("empty ident") {
                format!(
                    "{e}\n\nこのデバイスに git の名前が未設定です。次を実行してから再試行してください:\n  \
                     git config --global user.name \"あなたの名前\"\n  \
                     git config --global user.email \"you@example.com\""
                )
            } else {
                e
            }
        })?;
        commit_hash = git_opt(&repo, &["rev-parse", "--short", "HEAD"]);
    }

    // 6. push（remote が無いローカル運用でも成立させる）
    let remote = remote_url(&repo);
    let mut pushed = false;
    let mut push_error = None;
    if !no_push && remote.is_some() && has_staged {
        match git(&repo, &["push"]) {
            Ok(_) => pushed = true,
            // 初回は upstream が無いので設定して再試行する
            Err(first) => match git(&repo, &["push", "-u", "origin", "HEAD"]) {
                Ok(_) => pushed = true,
                Err(second) => push_error = Some(format!("{first}\n{second}")),
            },
        }
    }

    // 管理外ファイル（手置き）は触らないが必ず報告する
    let untracked: Vec<String> = walk_repo(&repo)
        .into_iter()
        .filter(|p| !current.contains(p))
        .filter(|p| classify_repo_path(p).is_none_or(|(_, e)| !e.class.is_shared()))
        .collect();

    Ok(json!({
        "action": "push",
        "repo": repo.to_string_lossy(),
        "written": written.len(),
        "removed": removed,
        "committed": has_staged,
        "commit": commit_hash,
        "pushed": pushed,
        "push_error": push_error,
        "remote": remote,
        "unclassified": unclassified,
        "untracked_in_repo": untracked,
    }))
}

/// リポジトリ → 実体。取り込み前に git pull（remote があれば）
pub fn pull() -> Result<Value, String> {
    let repo = linked_repo()?;
    let remote = remote_url(&repo);
    let mut fetched = false;
    if remote.is_some() {
        // ff-only にして、勝手なマージコミットを作らない。
        // 分岐していたら人が解決する（案内を返す）
        match git(&repo, &["pull", "--ff-only"]) {
            Ok(_) => fetched = true,
            Err(e) => {
                return Err(format!(
                    "{e}\n\n\
                     両方のデバイスで同じ設定を変更した可能性があります。次のどちらかで解決してください:\n\
                     1. 共有リポジトリで解決する:\n   \
                        cd {} && git pull --rebase   # 競合したら中身を直して git rebase --continue\n\
                     2. このデバイスの内容を正とする:\n   \
                        cd {} && git rebase --abort（実行中なら）→ tako config push\n\
                     解決後にもう一度 `tako config pull` を実行してください",
                    repo.display(),
                    repo.display()
                ));
            }
        }
    }

    let mut applied = Vec::new();
    let mut needs_local = Vec::new();
    let mut ignored = Vec::new();
    for repo_rel in walk_repo(&repo) {
        // **リポジトリ側の内容を信用しない**: 分類が Shared のものだけを実体へ書く。
        // これが無いと、リポジトリに `tako/token` を置くだけで実体を上書きできてしまう
        let Some((root, entry)) = classify_repo_path(&repo_rel) else {
            ignored.push(json!({ "path": repo_rel, "reason": "unclassified" }));
            continue;
        };
        if !entry.class.is_shared() {
            ignored.push(json!({ "path": repo_rel, "reason": entry.class.as_str() }));
            continue;
        }
        let Some(root_dir) = root.live_dir() else {
            continue;
        };
        let rel = repo_rel
            .split_once('/')
            .map(|(_, rest)| rest.to_string())
            .unwrap_or_default();
        let live = root_dir.join(&rel);
        let shared = std::fs::read_to_string(repo.join(&repo_rel))
            .map_err(|e| format!("読み取りに失敗 ({repo_rel}): {e}"))?;
        let content = import_content(entry, &shared, &live)?;
        let before = std::fs::read_to_string(&live).ok();
        let action = if before.as_deref() == Some(content.as_str()) {
            "unchanged"
        } else if before.is_some() {
            "updated"
        } else {
            "created"
        };
        if action != "unchanged" {
            if let Some(parent) = live.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("ディレクトリの作成に失敗: {e}"))?;
            }
            // 既存内容は世代バックアップに退避してから置き換える（#169 の部品を再利用）
            crate::config_io::atomic_write_with_backup(&live, &content)?;
        }
        // このデバイスで埋める必要がある値（資格情報の場所・env）を知らせる。
        // 「無くて当然」のもの（inherit なアカウント）は免除する
        if let Some(format) = portable::format_of(&live) {
            for field in entry.local_fields {
                for missing in portable::missing_local_fields(&content, format, field) {
                    let exempt = entry.needs_local_unless.iter().any(|sibling| {
                        portable::is_truthy_at(
                            &content,
                            format,
                            &portable::sibling_path(&missing, sibling),
                        )
                    });
                    if !exempt {
                        needs_local.push(json!({ "path": repo_rel, "field": missing }));
                    }
                }
            }
        }
        applied.push(json!({ "path": repo_rel, "action": action }));
    }

    Ok(json!({
        "action": "pull",
        "repo": repo.to_string_lossy(),
        "fetched": fetched,
        "remote": remote,
        "applied": applied,
        "needs_local": needs_local,
        "ignored": ignored,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn リポジトリパスの分類ができる() {
        let (root, entry) = classify_repo_path("tako/orchestrator/projects.yaml").unwrap();
        assert_eq!(root, Root::TakoData);
        assert!(entry.class.is_shared());
        // 秘匿ファイルは Shared にならない = pull が実体へ書かない
        let (_, entry) = classify_repo_path("tako/token").unwrap();
        assert!(!entry.class.is_shared());
        // ルート名が不正なら分類できない
        assert!(classify_repo_path("evil/token").is_none());
        assert!(classify_repo_path("README.md").is_none());
    }

    #[test]
    fn git_urlの判定() {
        assert!(is_git_url("git@github.com:me/cfg.git"));
        assert!(is_git_url("https://github.com/me/cfg.git"));
        assert!(is_git_url("ssh://git@host/x"));
        assert!(!is_git_url("~/tako-config-sync"));
        assert!(!is_git_url("/Users/me/cfg"));
    }

    #[test]
    fn readmeに共有対象と除外方針が載る() {
        let text = readme_text();
        assert!(text.contains("orchestrator/projects.yaml"));
        assert!(text.contains("CLAUDE.md"));
        assert!(text.contains("tako config list"));
        // 秘匿ファイル名を「共有対象」として並べていないこと
        assert!(!text.contains("- `tako/token`"));
    }

    #[test]
    fn gitignoreが秘匿ファイルを網羅する() {
        let text = gitignore_text();
        for name in [
            "token",
            "control.json",
            ".claude.json",
            ".credentials.json",
            "sessions.yaml",
            "workers.yaml",
        ] {
            assert!(text.contains(name), "{name} が .gitignore に無い");
        }
    }

    #[test]
    fn 一覧に全分類が出て未分類方針が明示される() {
        let v = list_in(tako_core::i18n::Lang::Ja);
        assert_eq!(
            v["entries"].as_array().unwrap().len(),
            catalog::CATALOG.len()
        );
        assert_eq!(v["unclassified_policy"], "not_shared");
        assert!(v["counts"]["shared"].as_u64().unwrap() > 0);
        assert!(v["counts"]["secret"].as_u64().unwrap() > 0);
    }
}
