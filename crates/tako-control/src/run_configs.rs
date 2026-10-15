//! Code Runner の実行設定の保存（`<data_dir>/run-configs.json`。Issue #1731 / #1726 S3）
//!
//! 「**このマシンでこのファイル / プロジェクトをどう走らせるか**」（選んだ実行環境・
//! プロファイル・引数・環境変数・作業ディレクトリ・実行前コマンド = #1732）を覚え、
//! `Run` / `RunResolve` がそれを重ねる。型と重ね方は
//! `tako_core::runner_config`、実行環境の解き方は `tako_core::runtime_env` の 1 実装で、
//! ここが持つのは**読み書き**と、`tako run-config` / MCP `tako_run_defaults`（`path`）の本体だけ。
//! 設計: `.agent/plans/2026-09-runner-settings.md` §4.2〜§4.5 / §5.5 / §8
//!
//! ## 置き場と共有
//!
//! 中身はマシン固有（interpreter の置き場・絶対パスのキー）なので `settings.json`（#513 で
//! 他のデバイスへ同期される）に相乗りせず、別ファイルにして共有カタログでは `Local` にする。
//! 共有したい実行の定義はファイル内の `tako:run` 宣言が担う。
//!
//! ## キー
//!
//! - `files`: ファイルの正規化済みパス（境界 B26 = `platform::path::canonicalize` を通したもの）
//! - `projects`: [`tako_core::project_root::scope_root`]（= `${workspaceRoot}` と同じ答え）
//!
//! `files` は [`FILES_MAX`] 件まで。超えたら `updated_at` の古いものから落とす
//! （消えたファイルは勝手に掃除しない。外付けドライブのように、いま見えないだけのことがある）。
//!
//! ## 書き込み
//!
//! `config_io` の排他ロック + tmp → rename（#169 の 3 層防御のうち 2 つ）。読む側はロックしない
//! （rename なので常に完全な内容が見える）。解釈できない内容は既定値へ落とす前に
//! `.unreadable.bak` へ退避する（#916。退避しないと直後の書き込みが元の内容を消す）。
//!
//! ## A/B
//!
//! `TAKO_1731_LEGACY=1` で `Run` / `RunResolve` が保存値を読まない（#1731 以前 = 自動選択と
//! 宣言の既定だけ）。ゲートは [`legacy_1731`] の 1 か所。
//! `TAKO_1732_LEGACY=1` は引数・環境変数・作業ディレクトリ・実行前コマンドだけを読まない
//! （#1732 以前 = 保存しても実行に効かない）。ゲートは [`legacy_1732`] の 1 か所
//!
//! ## 診断ログへ出さない
//!
//! 環境変数の値・引数・実行前コマンドはユーザーデータ（AGENTS.md の絶対ルール）。
//! このモジュールは persist.log / perf.log / stderr へ何も書かない
//! （番犬 `tests/issue1732_run_config_log_watchdog.rs`）

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tako_core::runner_config::{
    valid_env_name, FieldSource, RunConfig, RunConfigField, RunScope, RuntimeRef,
};
use tako_core::runtime_env::{self, QueryError, RealFs};

use crate::dispatch::DispatchError;
use crate::runtime_probe::{self, RuntimeUnresolved, Snapshot};

/// 置き場のファイル名（`<data_dir>/` 直下）
pub const FILENAME: &str = "run-configs.json";
/// スキーマの世代（形を変えるときは `migrations::SPECS` の target_version と一緒に上げる）
pub const VERSION: u32 = 1;
/// `files` の上限（`updated_at` の古いものから落とす）
pub const FILES_MAX: usize = 1_000;

/// #1731 以前（保存値を読まない）へ戻す A/B のゲート。**ここ 1 か所だけ**
pub fn legacy_1731() -> bool {
    std::env::var("TAKO_1731_LEGACY").is_ok_and(|v| v == "1")
}

/// #1732 以前（引数・環境変数・作業ディレクトリ・実行前コマンドを実行に重ねない）へ戻す
/// A/B のゲート。**ここ 1 か所だけ**（保存・読み出しの口はそのまま = 同じ保存値で比べられる）
pub fn legacy_1732() -> bool {
    std::env::var("TAKO_1732_LEGACY").is_ok_and(|v| v == "1")
}

/// `run-configs.json` の中身
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunConfigsFile {
    #[serde(default = "current_version")]
    pub version: u32,
    /// プロジェクトのルート → 設定
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub projects: BTreeMap<String, RunConfig>,
    /// ファイル → 設定
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, RunConfig>,
}

fn current_version() -> u32 {
    VERSION
}

impl Default for RunConfigsFile {
    fn default() -> Self {
        Self {
            version: VERSION,
            projects: BTreeMap::new(),
            files: BTreeMap::new(),
        }
    }
}

/// 既定の置き場
pub fn store_path() -> Option<PathBuf> {
    tako_core::paths::data_dir().map(|d| d.join(FILENAME))
}

/// 既定の置き場から読む（無ければ空）
pub fn load() -> RunConfigsFile {
    store_path().map(|p| load_from(&p)).unwrap_or_default()
}

/// 指定パスから読む。**解釈できない内容は空へ落とす前に退避する**（#916）
pub fn load_from(path: &Path) -> RunConfigsFile {
    let Ok(text) = std::fs::read_to_string(path) else {
        return RunConfigsFile::default();
    };
    match serde_json::from_str(&text) {
        Ok(file) => file,
        Err(_) => {
            let _ = tako_core::migration::quarantine_unreadable(path, &tako_core::migration::FsIo);
            RunConfigsFile::default()
        }
    }
}

/// 書き換える（排他ロック → 読む → `f` → 片付け → tmp + rename）。書いた後の中身も返す
pub fn update_at<R>(
    path: &Path,
    f: impl FnOnce(&mut RunConfigsFile) -> R,
) -> Result<(R, RunConfigsFile), String> {
    let _lock = crate::config_io::lock_exclusive(path)?;
    let mut store = load_from(path);
    let out = f(&mut store);
    store.tidy(FILES_MAX);
    let text = serde_json::to_string_pretty(&store).map_err(|e| e.to_string())? + "\n";
    crate::config_io::atomic_write(path, &text)?;
    Ok((out, store))
}

/// 保存のキー（境界 B26 を通したパスの表記）
pub fn key_of(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// 1 つの変更（応答の `updated`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub scope: RunScope,
    pub field: RunConfigField,
    /// `"set"` / `"reset"`
    pub action: &'static str,
}

impl Change {
    fn to_json(&self) -> Value {
        json!({ "scope": self.scope.as_str(), "field": self.field.as_str(), "action": self.action })
    }
}

/// 書き換えの指示（検証済み）
#[derive(Debug, Clone, Default)]
pub struct Edit {
    pub profile: Option<(RunScope, String)>,
    /// (保存先, kind, 参照)
    pub runtime: Option<(RunScope, String, RuntimeRef)>,
    /// 引数・作業ディレクトリ・実行前コマンド（#1732）
    pub args: Option<(RunScope, String)>,
    pub cwd: Option<(RunScope, String)>,
    pub before: Option<(RunScope, String)>,
    /// 設定する環境変数（名前は検証済み。キー単位で重ねる）
    pub env_set: Option<(RunScope, BTreeMap<String, String>)>,
    /// 消す環境変数の名前（`reset_scopes` のスコープから。名前は検めない = 壊れた名前も消せる）
    pub env_unset: Vec<String>,
    pub reset: Vec<RunConfigField>,
    /// 消すスコープ（保存先を省いた reset / 環境変数の削除は両方 = 実効値を既定へ戻す）
    pub reset_scopes: Vec<RunScope>,
    /// 実行環境の reset で消す kind（`None` = 全 kind。他言語の選択は残す）
    pub kind: Option<String>,
}

impl Edit {
    pub fn writes(&self) -> bool {
        self.profile.is_some()
            || self.runtime.is_some()
            || self.args.is_some()
            || self.cwd.is_some()
            || self.before.is_some()
            || self.env_set.is_some()
            || !self.env_unset.is_empty()
            || !self.reset.is_empty()
    }
}

impl RunConfigsFile {
    fn map_mut(&mut self, scope: RunScope) -> &mut BTreeMap<String, RunConfig> {
        match scope {
            RunScope::File => &mut self.files,
            RunScope::Project => &mut self.projects,
        }
    }

    /// 指示を当てる（reset → set の順）。実際に変わったものだけを返す
    pub fn apply(
        &mut self,
        file_key: &str,
        project_key: &str,
        edit: &Edit,
        now: &str,
    ) -> Vec<Change> {
        let key_for = |scope: RunScope| match scope {
            RunScope::File => file_key.to_string(),
            RunScope::Project => project_key.to_string(),
        };
        let mut changes = Vec::new();
        for scope in &edit.reset_scopes {
            let Some(cfg) = self.map_mut(*scope).get_mut(&key_for(*scope)) else {
                continue;
            };
            let mut touched = false;
            for field in &edit.reset {
                let had = match (field, &edit.kind) {
                    (RunConfigField::Runtime, Some(kind)) => cfg.runtime.remove(kind).is_some(),
                    _ => {
                        let had = cfg.has(*field);
                        cfg.clear(*field);
                        had
                    }
                };
                if had {
                    touched = true;
                    changes.push(Change {
                        scope: *scope,
                        field: *field,
                        action: "reset",
                    });
                }
            }
            // 環境変数のキー単位の削除（reset の後 = 同じ呼び出しの set が勝つ）
            let mut removed = false;
            for name in &edit.env_unset {
                removed |= cfg.env.remove(name).is_some();
            }
            if removed {
                touched = true;
                changes.push(Change {
                    scope: *scope,
                    field: RunConfigField::Env,
                    action: "reset",
                });
            }
            if touched {
                cfg.updated_at = Some(now.to_string());
            }
        }
        for (value, field) in [
            (&edit.args, RunConfigField::Args),
            (&edit.cwd, RunConfigField::Cwd),
            (&edit.before, RunConfigField::Before),
        ] {
            let Some((scope, v)) = value else { continue };
            let cfg = self.map_mut(*scope).entry(key_for(*scope)).or_default();
            if let Some(slot) = text_slot(cfg, field) {
                *slot = Some(v.clone());
            }
            cfg.updated_at = Some(now.to_string());
            changes.push(Change {
                scope: *scope,
                field,
                action: "set",
            });
        }
        if let Some((scope, vars)) = &edit.env_set {
            let cfg = self.map_mut(*scope).entry(key_for(*scope)).or_default();
            for (k, v) in vars {
                cfg.env.insert(k.clone(), v.clone());
            }
            cfg.updated_at = Some(now.to_string());
            changes.push(Change {
                scope: *scope,
                field: RunConfigField::Env,
                action: "set",
            });
        }
        if let Some((scope, name)) = &edit.profile {
            let cfg = self.map_mut(*scope).entry(key_for(*scope)).or_default();
            cfg.profile = Some(name.clone());
            cfg.updated_at = Some(now.to_string());
            changes.push(Change {
                scope: *scope,
                field: RunConfigField::Profile,
                action: "set",
            });
        }
        if let Some((scope, kind, r)) = &edit.runtime {
            let cfg = self.map_mut(*scope).entry(key_for(*scope)).or_default();
            cfg.runtime.insert(kind.clone(), r.clone());
            cfg.updated_at = Some(now.to_string());
            changes.push(Change {
                scope: *scope,
                field: RunConfigField::Runtime,
                action: "set",
            });
        }
        changes
    }

    /// 空になった項目を消し、`files` を上限まで刈る（`updated_at` の古い順。同時刻はキー順）
    pub fn tidy(&mut self, files_max: usize) {
        self.projects.retain(|_, c| !c.is_empty());
        self.files.retain(|_, c| !c.is_empty());
        if self.files.len() > files_max {
            let mut order: Vec<(String, String)> = self
                .files
                .iter()
                .map(|(k, c)| (c.updated_at.clone().unwrap_or_default(), k.clone()))
                .collect();
            order.sort();
            let drop = self.files.len() - files_max;
            for (_, key) in order.into_iter().take(drop) {
                self.files.remove(&key);
            }
        }
    }
}

/// 1 本の文字列の項目の欄（引数・作業ディレクトリ・実行前コマンド。#1732）
fn text_slot(cfg: &mut RunConfig, field: RunConfigField) -> Option<&mut Option<String>> {
    match field {
        RunConfigField::Args => Some(&mut cfg.args),
        RunConfigField::Cwd => Some(&mut cfg.cwd),
        RunConfigField::Before => Some(&mut cfg.before),
        RunConfigField::Profile | RunConfigField::Runtime | RunConfigField::Env => None,
    }
}

// ─── ファイルに効く保存値 ──────────────────────────────────────────────

/// あるファイルに効く保存値（ファイルのスコープ + プロジェクトのスコープ）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Saved {
    pub project_root: PathBuf,
    pub file: Option<RunConfig>,
    pub project: Option<RunConfig>,
}

impl Saved {
    pub fn of(store: &RunConfigsFile, file: &Path, project_root: &Path) -> Self {
        Self {
            project_root: project_root.to_path_buf(),
            file: store.files.get(&key_of(file)).cloned(),
            project: store.projects.get(&key_of(project_root)).cloned(),
        }
    }

    fn layers(&self) -> [(Option<&RunConfig>, RunScope); 2] {
        [
            (self.file.as_ref(), RunScope::File),
            (self.project.as_ref(), RunScope::Project),
        ]
    }

    /// その kind の実行環境（ファイル → プロジェクト）
    pub fn runtime(&self, kind: &str) -> Option<(&RuntimeRef, RunScope)> {
        self.layers()
            .into_iter()
            .find_map(|(c, s)| c.and_then(|c| c.runtime.get(kind)).map(|r| (r, s)))
    }

    /// 選んだプロファイル（ファイル → プロジェクト）
    pub fn profile(&self) -> Option<(&str, RunScope)> {
        self.layers()
            .into_iter()
            .find_map(|(c, s)| c.and_then(|c| c.profile.as_deref()).map(|p| (p, s)))
    }

    /// 解決へ渡す実行設定の層（#1732。引数・作業ディレクトリ・実行前コマンドはファイル →
    /// プロジェクトの順の先勝ち、環境変数はキー単位で重ねる = `runner_config::merge` と同じ）。
    ///
    /// 環境変数の名前が識別子でないもの（手で書き換えた保存値）は層に入れず、2 つ目で返す
    /// （名前はコマンド文字列の文へそのまま入るので、`Run` は走らせずに止める）
    pub fn layer(&self) -> (tako_core::ConfigLayer, Vec<String>) {
        if legacy_1732() {
            return Default::default();
        }
        let eff = tako_core::runner_config::merge(
            self.file.as_ref(),
            self.project.as_ref(),
            &tako_core::runner_config::Declared::default(),
            &BTreeMap::new(),
        );
        let (env, bad): (BTreeMap<_, _>, BTreeMap<_, _>) = eff
            .env
            .into_iter()
            .map(|(k, v)| (k, v.value))
            .partition(|(k, _)| valid_env_name(k));
        let layer = tako_core::ConfigLayer {
            args: eff.args.map(|s| s.value),
            cwd: eff.cwd.map(|s| s.value),
            before: eff.before.map(|s| s.value),
            env,
        };
        (layer, bad.into_keys().collect())
    }
}

/// 保存した環境変数の名前が使えないときのエラー（`Run`。既定へ落とさず、消し方を案内する）
pub fn bad_env_names(names: &[String], file: &Path) -> DispatchError {
    let f = tako_core::shell::quote_for_shell(&file.display().to_string());
    let list = names.join(" / ");
    let first = names.first().map(String::as_str).unwrap_or("<名前>");
    let q = tako_core::shell::quote_for_shell(first);
    DispatchError::Operation(tr(
        format!(
            "保存した環境変数の名前「{list}」は使えない（英字・数字・_ だけ・先頭は数字以外）。\
             `tako run-config {f} --unset-env {q}` で消す"
        ),
        format!(
            "The saved environment variable name \"{list}\" cannot be used (letters, digits and _ \
             only, not starting with a digit). Remove it with `tako run-config {f} --unset-env {q}`"
        ),
    ))
}

/// 保存した作業ディレクトリが今無いときのエラー（`Run`。既定へ落とさず、戻し方を案内する）
pub fn saved_cwd_missing(dir: &Path, file: &Path) -> DispatchError {
    let f = tako_core::shell::quote_for_shell(&file.display().to_string());
    let d = dir.display();
    DispatchError::Operation(tr(
        format!(
            "保存した作業ディレクトリが無い: {d}。`tako run-config {f} --reset cwd` で既定へ戻すか、\
             `tako run-config {f} --cwd <ディレクトリ>` で選び直す"
        ),
        format!(
            "The saved working directory does not exist: {d}. Run `tako run-config {f} --reset cwd` \
             to go back to the default, or `tako run-config {f} --cwd <dir>` to choose again"
        ),
    ))
}

/// `Run` / `RunResolve` が読む保存値（A/B 中は空。プロジェクトのルートだけは決める）
pub fn saved_for(file: &Path) -> Saved {
    let root = tako_core::project_root::scope_root(file);
    if legacy_1731() {
        return Saved {
            project_root: root,
            ..Saved::default()
        };
    }
    Saved::of(&load(), file, &root)
}

/// 保存した実行環境を検出の完成品へ当てる（無ければ何もしない = 自動選択）
pub fn apply_runtime(snap: &mut Snapshot, saved: &Saved) -> Result<(), RuntimeUnresolved> {
    let Some((r, scope)) = saved.runtime(snap.kind.id) else {
        return Ok(());
    };
    let r = r.clone();
    snap.choose(&r, scope.source(), &saved.project_root)
}

/// 保存したプロファイルが宣言から消えていたときのエラー（既定のプロファイルへ落とさない）
pub fn profile_gone(name: &str, scope: RunScope, file: &Path, cause: &str) -> DispatchError {
    let f = tako_core::shell::quote_for_shell(&file.display().to_string());
    let scope = scope.as_str();
    DispatchError::Operation(match tako_core::i18n::lang() {
        tako_core::i18n::Lang::Ja => format!(
            "保存したプロファイル「{name}」（{scope} に保存）で走らせられない（{cause}）。\
             `tako run-config {f} --reset profile` で既定へ戻すか、`tako run-config {f} --profile <名前>` で選び直す"
        ),
        tako_core::i18n::Lang::En => format!(
            "Cannot run with the saved profile \"{name}\" (saved for the {scope}): {cause}. Run \
             `tako run-config {f} --reset profile` to go back to the default, or \
             `tako run-config {f} --profile <name>` to choose again"
        ),
    })
}

// ─── `tako run-config` / MCP `tako_run_defaults`（path）の本体 ──────────

/// 呼び出しの材料（dispatch が対象ファイルを解いて詰める）
pub struct PathArgs<'a> {
    /// 境界 B26 を通したファイル
    pub file: &'a Path,
    /// ファイルの先頭（宣言のプロファイルを数える）
    pub head: &'a str,
    pub scope: Option<&'a str>,
    pub profile: Option<&'a str>,
    pub runtime: Option<&'a str>,
    pub reset: Option<&'a [String]>,
    /// 引数・作業ディレクトリ・実行前コマンド（#1732）
    pub args: Option<&'a str>,
    pub cwd: Option<&'a str>,
    pub before: Option<&'a str>,
    /// 環境変数（値 `None` = そのキーを消す）
    pub env: Option<&'a BTreeMap<String, Option<String>>>,
}

impl<'a> PathArgs<'a> {
    /// 項目を 1 つも渡さない（= 読むだけ）材料。先頭は空
    pub fn empty(file: &'a Path) -> Self {
        Self {
            file,
            head: "",
            scope: None,
            profile: None,
            runtime: None,
            reset: None,
            args: None,
            cwd: None,
            before: None,
            env: None,
        }
    }
}

/// 読む（項目を渡さない）か、検証して保存してから今の実効値を返す。
///
/// 実行環境の候補は `Run` と同じ材料（Tier F + 一覧で覚えた Tier P の答え）で、
/// **子プロセスは起こさない**（UI スレッドで呼ばれる）。道具の場所・版まで確かめた
/// 候補は `tako run <file> --list`（`RunResolve`）で見る
pub fn handle(a: PathArgs<'_>) -> Result<Value, DispatchError> {
    let invalid = DispatchError::InvalidParams;
    let scope = a
        .scope
        .map(|s| RunScope::parse(s).ok_or_else(|| invalid(msg_bad_scope(s))))
        .transpose()?;
    let reset = match a.reset {
        Some(words) => {
            RunConfigField::parse_reset(words).map_err(|bad| invalid(msg_bad_reset(&bad)))?
        }
        None => Vec::new(),
    };
    let root = tako_core::project_root::scope_root(a.file);
    let mut snap = runtime_probe::for_run(a.file);

    // 宣言のプロファイル（`--profile` の検証と応答の `profiles`）
    let settings = crate::settings::load();
    let resolution = tako_core::resolve_file(
        a.file,
        a.head,
        &settings.runner_defaults,
        None,
        None,
        &snap.as_ref().map(|s| s.words()).unwrap_or_default(),
        // 宣言のプロファイルを数えるだけ（コマンドは使わない）
        &tako_core::ConfigLayer::default(),
    )
    .ok();
    let profiles: Vec<String> = resolution
        .as_ref()
        .map(|r| r.all_profiles.iter().map(|p| p.profile.clone()).collect())
        .unwrap_or_default();

    let mut edit = Edit {
        reset,
        reset_scopes: scope.map_or_else(|| RunScope::ALL.to_vec(), |s| vec![s]),
        kind: snap.as_ref().map(|s| s.kind.id.to_string()),
        ..Edit::default()
    };
    if let Some(q) = a.runtime {
        let snap = snap
            .as_ref()
            .ok_or_else(|| invalid(msg_no_runtime_kind(a.file)))?;
        let r = query_to_ref(snap, q, a.file)?;
        // 今解けない参照は保存しない（保存した瞬間から `Run` がエラーになる）
        let mut probe = snap.clone();
        probe
            .choose(&r, FieldSource::File, &root)
            .map_err(|u| invalid(u.unresolved.reason))?;
        let to = scope.unwrap_or(RunConfigField::Runtime.default_scope());
        edit.runtime = Some((to, snap.kind.id.to_string(), r));
    }
    if let Some(name) = a.profile {
        if !profiles.iter().any(|p| p == name) {
            return Err(invalid(msg_unknown_profile(name, &profiles)));
        }
        let to = scope.unwrap_or(RunConfigField::Profile.default_scope());
        edit.profile = Some((to, name.to_string()));
    }
    // #1732: 引数・作業ディレクトリ・実行前コマンド・環境変数
    let to = |field: RunConfigField| scope.unwrap_or(field.default_scope());
    edit.args = a.args.map(|v| (to(RunConfigField::Args), v.to_string()));
    edit.before = a
        .before
        .map(|v| (to(RunConfigField::Before), v.to_string()));
    if let Some(cwd) = a.cwd {
        // 今解けない作業ディレクトリは保存しない（保存した瞬間から `Run` が「cwd が存在しない」で止まる）
        check_cwd(a.file, &root, cwd)?;
        edit.cwd = Some((to(RunConfigField::Cwd), cwd.to_string()));
    }
    if let Some(env) = a.env {
        let mut set = BTreeMap::new();
        for (name, value) in env {
            match value {
                Some(v) => {
                    if !valid_env_name(name) {
                        return Err(invalid(msg_bad_env_name(name)));
                    }
                    set.insert(name.clone(), v.clone());
                }
                None => edit.env_unset.push(name.clone()),
            }
        }
        if !set.is_empty() {
            edit.env_set = Some((to(RunConfigField::Env), set));
        }
    }

    let (changes, store) = if edit.writes() {
        let path = store_path().ok_or_else(|| DispatchError::Operation(msg_no_data_dir()))?;
        let now = crate::sessions::now_iso();
        update_at(&path, |st| {
            st.apply(&key_of(a.file), &key_of(&root), &edit, &now)
        })
        .map_err(DispatchError::Operation)?
    } else {
        (Vec::new(), load())
    };

    // 今の実効値（`Run` が重ねるものと同じ材料）
    let saved = Saved::of(&store, a.file, &root);
    let unresolved = snap
        .as_mut()
        .and_then(|s| apply_runtime(s, &saved).err())
        .map(|u| u.to_json(a.file));
    let declared_cwd = resolution.as_ref().and_then(|r| r.declared_cwd.clone());
    let eff = runtime_probe::effective_config(
        snap.as_ref(),
        declared_cwd.as_deref(),
        saved.file.as_ref(),
        saved.project.as_ref(),
    );
    Ok(json!({
        "path": a.file.display().to_string(),
        // プロジェクトのスコープのキー（= `${workspaceRoot}`。`RunResolve` の `scope_root` と同じ）
        "scope_root": root.display().to_string(),
        "saved": {
            "file": saved.file,
            "project": saved.project,
        },
        "config": runtime_probe::config_json(&eff),
        "runtime": runtime_probe::runtime_json(snap.as_ref()),
        "runtime_unresolved": unresolved,
        "runtimes": runtime_probe::runtimes_json(snap.as_ref()),
        "profiles": profiles,
        "updated": changes.iter().map(Change::to_json).collect::<Vec<_>>(),
    }))
}

/// `--runtime` の指定 → 参照（ID / 短縮 → パス。短縮が複数に当たったら選ばない = #1466）
fn query_to_ref(snap: &Snapshot, q: &str, file: &Path) -> Result<RuntimeRef, DispatchError> {
    let candidates = &snap.detection.candidates;
    match runtime_env::match_query(snap.kind, q, candidates) {
        Ok(r) => Ok(r),
        Err(QueryError::NotFound) => {
            let p = Path::new(q.trim());
            let base = file.parent().unwrap_or(Path::new("."));
            // 素直に足すだけ（実体へ寄せない: venv の python はシンボリックリンクで、
            // 寄せると環境の外の interpreter を指してしまう）
            let abs = without_cur_dir(&if p.is_absolute() {
                p.to_path_buf()
            } else {
                base.join(p)
            });
            runtime_env::ref_for_path(snap.kind, &abs, &RealFs).ok_or_else(|| {
                DispatchError::InvalidParams(runtime_env::msg_query_error(
                    q,
                    &QueryError::NotFound,
                    candidates,
                ))
            })
        }
        Err(e) => Err(DispatchError::InvalidParams(runtime_env::msg_query_error(
            q, &e, candidates,
        ))),
    }
}

/// 作業ディレクトリの指定が今解けるか（`Run` と同じ展開 = `runner::expand_literal` と
/// ファイルの段からの相対）。空は受けない（既定へ戻すのは `--reset cwd`）
fn check_cwd(file: &Path, root: &Path, cwd: &str) -> Result<(), DispatchError> {
    if cwd.trim().is_empty() {
        return Err(DispatchError::InvalidParams(msg_empty_cwd()));
    }
    let vars = tako_core::RunVars::new(file, root);
    let expanded = tako_core::runner::expand_literal(cwd, &vars);
    let p = Path::new(&expanded);
    let dir = if p.is_absolute() {
        p.to_path_buf()
    } else {
        file.parent().unwrap_or(Path::new(".")).join(p)
    };
    if dir.is_dir() {
        Ok(())
    } else {
        Err(DispatchError::InvalidParams(msg_cwd_missing(
            &dir.display().to_string(),
        )))
    }
}

/// `./` の成分だけを落とす（`..` は残す = シンボリックリンク越しの意味を変えない）
fn without_cur_dir(p: &Path) -> PathBuf {
    p.components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect()
}

// ─── 案内文（日英） ────────────────────────────────────────────────────

fn tr(ja: String, en: String) -> String {
    match tako_core::i18n::lang() {
        tako_core::i18n::Lang::Ja => ja,
        tako_core::i18n::Lang::En => en,
    }
}

fn msg_bad_scope(s: &str) -> String {
    let all = RunScope::VALUES.join(" / ");
    tr(
        format!("scope は {all} のどれか（指定: {s:?}）"),
        format!("scope must be one of {all} (got {s:?})"),
    )
}

fn msg_bad_reset(bad: &str) -> String {
    let all = RunConfigField::reset_values().join(" / ");
    tr(
        format!("reset は {all} のどれか（指定: {bad:?}）"),
        format!("reset must be one of {all} (got {bad:?})"),
    )
}

fn msg_no_runtime_kind(file: &Path) -> String {
    let ext = file
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_default();
    tr(
        format!("この種類のファイル（.{ext}）には選べる実行環境が無い"),
        format!("There are no runtimes to choose from for this kind of file (.{ext})"),
    )
}

fn msg_unknown_profile(name: &str, profiles: &[String]) -> String {
    let list = if profiles.is_empty() {
        "-".to_string()
    } else {
        profiles.join(" / ")
    };
    tr(
        format!("プロファイル「{name}」はこのファイルに無い。選べるもの: {list}"),
        format!("This file has no profile \"{name}\". Available: {list}"),
    )
}

fn msg_bad_env_name(name: &str) -> String {
    tr(
        format!("環境変数の名前「{name}」は使えない（英字・数字・_ だけ・先頭は数字以外）"),
        format!(
            "\"{name}\" cannot be used as an environment variable name \
             (letters, digits and _ only, not starting with a digit)"
        ),
    )
}

fn msg_empty_cwd() -> String {
    tr(
        "cwd は空にできない（既定へ戻すなら --reset cwd）".to_string(),
        "cwd cannot be empty (use --reset cwd to go back to the default)".to_string(),
    )
}

fn msg_cwd_missing(dir: &str) -> String {
    tr(
        format!("作業ディレクトリが無いので保存しない: {dir}"),
        format!("Not saved: the working directory does not exist: {dir}"),
    )
}

fn msg_no_data_dir() -> String {
    tr(
        "data dir を解決できないので実行設定を保存できない".to_string(),
        "Cannot resolve the data dir, so the run config cannot be saved".to_string(),
    )
}

#[cfg(test)]
mod tests;
