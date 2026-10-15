//! Code Runner: ファイル内宣言パーサ + コマンド解決器（FR-3.18）
//!
//! ファイル先頭の `tako:run:` 等の宣言を解析し、実行コマンドを解決する。
//! GPUI 非依存。設計: `.agent/plans/2026-07-code-runner.md`
//!
//! ## 入口は [`resolve_file`] の 1 本（#1656）
//!
//! 解決の優先順位は「`command` の上書き → ファイル内宣言 → **ユーザーが設定した**拡張子既定 →
//! プロジェクト既定（[`crate::runner_project`]）→ 組み込みの拡張子既定」。
//! プロジェクト既定はファイルシステムを見る（上へ辿って `Cargo.toml` 等を探す）ので、
//! dispatch（CLI / MCP / 再生ボタン）はすべて [`resolve_file`] を通す。
//! ファイルシステムを見ない [`resolve_for`] はこのモジュールのテスト専用で、外へは出さない
//! （出すと、プロジェクトを見ない解決がどこかの入口に残る）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::platform::shell::run_pane_dialect_for;
use crate::platform::shell_dialect::ShellDialect;
use crate::platform::support::Platform;
use crate::project_root::{self, SearchBounds};
use crate::runner_project::{self, ProjectMatch};
use crate::runtime_env;
use crate::shell::quote_for_shell;

// --- 定数 ---

const SCAN_MAX_LINES: usize = 64;
const SCAN_MAX_BYTES: usize = 16 * 1024;
const PREFIX_MAX_LEN: usize = 16;
const PROFILE_NAME_MAX_LEN: usize = 32;

const MARKER: &str = "tako:";

// 行末クローザ（ブロックコメント等の終端記号）
const CLOSERS: &[&str] = &["-->", "*/", "#}", "--}}"];

/// 組み込み拡張子既定マップ（この OS ぶん）。
///
/// 表の正本は [`crate::platform::runner_defaults::TABLE`]（**両 OS ぶんを 1 枚で持つ**）。
/// ここは「実行中の OS の列を引く」だけの薄い入口で、OS を指定して引きたい
/// テスト・検査は [`builtin_defaults_for`] を使う（#1655）
pub fn builtin_defaults() -> Vec<(&'static str, &'static str)> {
    builtin_defaults_for(Platform::current())
}

/// 指定した OS の組み込み既定。**macOS 上から Windows の表を引ける**（#1655 / #1616 の作法）
pub fn builtin_defaults_for(platform: Platform) -> Vec<(&'static str, &'static str)> {
    crate::platform::runner_defaults::builtin_defaults(platform)
}

// --- 型定義 ---

/// コマンドの出典
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunSource {
    /// ファイル内宣言（`tako:run:` 系）
    Declaration,
    /// 拡張子既定（settings / 組み込み）
    ExtensionDefault,
    /// プロジェクト既定（`Cargo.toml` / `package.json` 等を上へ辿って決めたもの。#1656）
    ProjectDefault,
    /// CLI / MCP の明示 `--command` オーバーライド
    Override,
}

impl RunSource {
    /// CLI / MCP の応答に出す名前（`tako run --dry-run` / `tako_run_resolve` の `source`）
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Declaration => "declaration",
            Self::ExtensionDefault => "extension_default",
            Self::ProjectDefault => "project_default",
            Self::Override => "override",
        }
    }
}

/// 1 プロファイル分の解決済み実行計画
#[derive(Debug, Clone)]
pub struct RunPlan {
    pub profile: String,
    pub command: String,
    /// 展開前のコマンド（宣言・既定の書いたまま。プロジェクト既定は組み立て済みの形）。
    /// 「実行環境の変数を使っているか」を読む材料（#1730）
    pub template: String,
    pub cwd: std::path::PathBuf,
    /// `tako:shell` 指定（None = ログインシェル）
    pub shell: Option<String>,
    pub source: RunSource,
}

impl RunPlan {
    /// コマンドが実行環境の変数（`${<variable>}`）を使っているか。包む形の実行環境
    /// （`uv run python`）は、変数を使わない宣言（`tako:run: pytest`）には効かない
    pub fn uses_variable(&self, variable: &str) -> bool {
        self.source == RunSource::ProjectDefault
            || self.template.contains(&format!("${{{variable}}}"))
    }
}

/// パース結果: 1 プロファイル分の宣言内容（未展開）
#[derive(Debug, Clone)]
pub struct ProfileDecl {
    pub name: String,
    pub run: Option<String>,
    pub cwd: Option<String>,
    pub shell: Option<String>,
}

/// `parse_declarations` の戻り値
#[derive(Debug, Clone)]
pub struct Declarations {
    /// 出現順で保持（宣言順がドロップダウンの表示順）
    pub profiles: Vec<ProfileDecl>,
    /// 重複宣言等の警告
    pub warnings: Vec<String>,
}

/// `resolve` の戻り値
#[derive(Debug, Clone)]
pub struct Resolution {
    /// 指定されたプロファイルの実行計画（実行可能な場合）
    pub plan: RunPlan,
    /// 全検出プロファイル一覧（ドロップダウン用）
    pub all_profiles: Vec<RunPlan>,
    pub warnings: Vec<String>,
    /// ファイルの属するプロジェクト（見つからなければ `None`。宣言が勝った場合も載る）
    pub project: Option<ProjectMatch>,
    /// `${workspaceRoot}` の値（プロジェクトのルート → git のルート → ファイルのディレクトリ）
    pub workspace_root: PathBuf,
    /// 選んだプロファイルの宣言が明示した作業ディレクトリ（書いたままの値。`tako:cwd`）。
    /// 実行設定の項目の出典（`declaration`）を示す材料（#1726）
    pub declared_cwd: Option<String>,
    /// 実行前コマンド（[`ConfigLayer::before`] を展開したもの。空白だけなら `None`。#1732）
    pub before: Option<String>,
    /// 実行ペインへ渡す環境変数（[`ConfigLayer::env`] の値を [`expand_literal`] で展開したもの。#1732）
    pub env: Vec<(String, String)>,
}

/// 実行設定の層（#1726 / #1732。`tako-control::run_configs` がファイル → プロジェクトの
/// 保存値を重ねて作る）が解決へ渡すもの。
///
/// **空（`Default`）= 実行設定なし = #1732 以前と 1 バイトも変わらない**。`command` の上書き
/// （一回だけ全部変える逃げ道）では呼び出し側が空を渡す（保存した層を素通りする = 設計書 §4.3）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigLayer {
    /// 引数（`${args}`。テンプレートに無ければ**末尾へ足す**。全プロファイルに効く）
    pub args: Option<String>,
    /// 作業ディレクトリ（宣言の `tako:cwd` より強い。相対はファイルのディレクトリ基準 =
    /// `tako:cwd` と同じ意味論。全プロファイルに効く）
    pub cwd: Option<String>,
    /// 実行前コマンド（失敗したら本体を走らせない = `platform::shell::compose_run_script`）
    pub before: Option<String>,
    /// 環境変数（値はリテラル。tako の変数だけ展開する。名前は呼び出し側で
    /// `runner_config::valid_env_name` を通してあること）
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error("実行コマンドが見つからない: {0}")]
    NoCommand(String),
    #[error("指定されたプロファイル '{0}' はファイル内に宣言されていない")]
    ProfileNotFound(String),
    #[error("cwd が存在しない: {0}")]
    CwdNotFound(String),
}

// --- パーサ ---

/// ファイル先頭テキストから宣言を抽出（64 行 / 16 KiB ウィンドウ）
pub fn parse_declarations(head: &str) -> Declarations {
    let mut profiles: Vec<ProfileDecl> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut bytes_consumed: usize = 0;

    for (line_idx, line) in head.lines().enumerate() {
        if line_idx >= SCAN_MAX_LINES {
            break;
        }
        bytes_consumed += line.len() + 1; // +1 for newline
        if bytes_consumed > SCAN_MAX_BYTES {
            break;
        }

        // BOM 除去（先頭行のみ）。判定は `text::strip_bom` の 1 実装（#1202）
        let line = if line_idx == 0 {
            crate::text::strip_bom(line)
        } else {
            line
        };

        // 行内の `tako:` を探す
        let Some((prefix_len, rest)) = find_marker(line) else {
            continue;
        };

        // 接頭辞の長さ制限
        if prefix_len > PREFIX_MAX_LEN {
            continue;
        }

        // キーとプロファイル名のパース
        let Some((key, profile_name, value)) = parse_directive(rest) else {
            continue;
        };

        let profile_name = profile_name.unwrap_or("default");

        // プロファイルを探すか新規作成
        let decl = if let Some(pos) = profiles.iter().position(|p| p.name == profile_name) {
            &mut profiles[pos]
        } else {
            profiles.push(ProfileDecl {
                name: profile_name.to_string(),
                run: None,
                cwd: None,
                shell: None,
            });
            profiles.last_mut().unwrap()
        };

        match key {
            "run" => {
                if decl.run.is_some() {
                    warnings.push(format!(
                        "プロファイル '{}' の run が重複宣言（後勝ち、{}行目）",
                        profile_name,
                        line_idx + 1
                    ));
                }
                decl.run = Some(value.to_string());
            }
            "cwd" => {
                if decl.cwd.is_some() {
                    warnings.push(format!(
                        "プロファイル '{}' の cwd が重複宣言（後勝ち、{}行目）",
                        profile_name,
                        line_idx + 1
                    ));
                }
                decl.cwd = Some(value.to_string());
            }
            "shell" => {
                if decl.shell.is_some() {
                    warnings.push(format!(
                        "プロファイル '{}' の shell が重複宣言（後勝ち、{}行目）",
                        profile_name,
                        line_idx + 1
                    ));
                }
                decl.shell = Some(value.to_string());
            }
            _ => {}
        }
    }

    Declarations { profiles, warnings }
}

/// 行内の `tako:` マーカーを探し、(接頭辞の長さ, マーカー以降の文字列) を返す。
/// `tako:` の直前が行頭・空白・非英数字のいずれかであることを検証する
fn find_marker(line: &str) -> Option<(usize, &str)> {
    let mut search_from = 0;
    loop {
        let pos = line[search_from..].find(MARKER)?;
        let abs_pos = search_from + pos;

        // `tako:` の直前チェック: 行頭・空白・非英数字
        if abs_pos == 0 {
            return Some((0, &line[MARKER.len()..]));
        }

        let prev_char = line[..abs_pos].chars().next_back().unwrap();
        if prev_char.is_whitespace() || !prev_char.is_alphanumeric() {
            return Some((abs_pos, &line[abs_pos + MARKER.len()..]));
        }

        // `mytako:run` のように英数字が直前にある場合はスキップして次を探す
        search_from = abs_pos + MARKER.len();
        if search_from >= line.len() {
            return None;
        }
    }
}

/// `tako:` 以降の文字列からキー・プロファイル名・値を抽出。
/// 戻り値: (key, Option<profile_name>, value)
fn parse_directive(rest: &str) -> Option<(&str, Option<&str>, &str)> {
    // キー名（小文字英字）を取得
    let key_end = rest.find(|c: char| !c.is_ascii_lowercase())?;
    if key_end == 0 {
        return None;
    }
    let key = &rest[..key_end];

    // `run` / `cwd` / `shell` のみ
    if !matches!(key, "run" | "cwd" | "shell") {
        return None;
    }

    let after_key = &rest[key_end..];

    // プロファイル名（`[name]` 部分、省略可）
    let (profile_name, after_profile) = if after_key.starts_with('[') {
        let bracket_end = after_key.find(']')?;
        let name = &after_key[1..bracket_end];
        // プロファイル名の検証: [A-Za-z0-9_-]{1,32}
        if name.is_empty()
            || name.len() > PROFILE_NAME_MAX_LEN
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return None;
        }
        (Some(name), &after_key[bracket_end + 1..])
    } else {
        (None, after_key)
    };

    // `:` の後ろが値
    let after_profile = after_profile.strip_prefix(':')?;
    let value = strip_closers(after_profile.trim());

    Some((key, profile_name, value))
}

/// 行末クローザ（`-->`, `*/`, `#}`, `--}}`）を除去
fn strip_closers(value: &str) -> &str {
    let mut v = value;
    for closer in CLOSERS {
        if let Some(stripped) = v.strip_suffix(closer) {
            v = stripped.trim_end();
            break;
        }
    }
    v
}

// --- 変数展開 ---

/// 実行環境の層（#1730。`tako-control::runtime_probe`）が決めた変数の値。
///
/// 鍵は実行環境の表の `variable`（`${python}` の `python`）、値は `${…}` に入る**語の列**
/// （`[<interpreter>]` / `[<道具>, "run", "python"]`）。語の列のまま持つのは、引用の仕方が
/// 置き先のシェルで変わるため（`tako:shell` の宣言があればそのシェルの方言で囲む）。
/// **空 = 実行環境の層を通っていない**（`TAKO_1730_LEGACY=1` / UI の一覧）
pub type RuntimeWords = BTreeMap<String, Vec<String>>;

/// 変数展開の文脈（#1656 / #1730 / #1732）。
///
/// 展開に使う値はここへ集める（`expand_variables` の引数を増やさない）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunVars {
    /// 実行対象（`${file}` / `${fileDir}` / `${fileBase}` / `${fileNoExt}` / `${ext}` の元）
    pub path: PathBuf,
    /// `${workspaceRoot}`（プロジェクトのルート → git のルート → ファイルのディレクトリ）
    pub workspace_root: PathBuf,
    /// 実行環境の変数（`${python}` 等）。無い変数は表の `fallback`（[`Self::platform`] の列）で
    /// 展開する = 実行環境が見つからないときの展開が今と 1 バイトも変わらない
    pub runtime: RuntimeWords,
    /// `fallback` の列を引く OS
    pub platform: Platform,
    /// 実行環境の語を置くシェルの方言（PowerShell は囲んだパスを `&` で起こす）
    pub dialect: ShellDialect,
    /// `${args}`（実行設定の引数。#1732）。**シェルへそのまま渡る 1 本の文字列**なので引用しない
    /// （`--name "a b"` をシェルが 2 語に割る = JetBrains の Program arguments と同じ）。空 = 引数なし
    pub args: String,
    /// `${fileBase}` をファイルの**絶対パス**で展開する（#1732）。実行設定の作業ディレクトリが
    /// ファイルの段でないとき、組み込み既定の `python3 ${fileBase}` が移った先で見つからなく
    /// ならないようにする（プログラムの場所と作業ディレクトリを独立させる = JetBrains と同じ）
    pub base_absolute: bool,
}

impl RunVars {
    pub fn new(path: &Path, workspace_root: &Path) -> Self {
        let platform = Platform::current();
        Self {
            path: path.to_path_buf(),
            workspace_root: workspace_root.to_path_buf(),
            runtime: RuntimeWords::new(),
            platform,
            dialect: run_pane_dialect_for(platform),
            args: String::new(),
            base_absolute: false,
        }
    }

    /// プロジェクトを知らない文脈（`${workspaceRoot}` はファイルのディレクトリ）
    pub fn for_file(path: &Path) -> Self {
        Self::new(path, path.parent().unwrap_or(Path::new("")))
    }

    /// OS を決める（`fallback` の列と、実行ペインのシェルの方言）。macOS から Windows の
    /// 展開を検査するときに使う
    pub fn on(mut self, platform: Platform) -> Self {
        self.platform = platform;
        self.dialect = run_pane_dialect_for(platform);
        self
    }

    /// 実行環境の層が決めた値を載せる
    pub fn with_runtime(mut self, runtime: &RuntimeWords) -> Self {
        self.runtime = runtime.clone();
        self
    }

    /// 実行設定の引数を載せる（#1732）
    pub fn with_args(mut self, args: &str) -> Self {
        self.args = args.to_string();
        self
    }

    /// 語を置くシェルの方言を差し替える（`tako:shell` の宣言）
    fn in_dialect(&self, dialect: ShellDialect) -> Self {
        Self {
            dialect,
            ..self.clone()
        }
    }

    /// 実行環境の変数（表の `variable`）の値。表に無い名前は `None`（展開せず残す）
    fn runtime_value(&self, name: &str) -> Option<String> {
        let kind = runtime_env::KINDS.iter().find(|k| k.variable == name)?;
        Some(match self.runtime.get(name).filter(|w| !w.is_empty()) {
            Some(words) => self.dialect.command_words(words),
            None => runtime_env::fallback_value(kind, self.platform).to_string(),
        })
    }
}

/// `tako:shell` の宣言が決める方言（判定できないシェルは POSIX = `declared_shell_command` と同じ）
fn declared_dialect(shell: &str) -> ShellDialect {
    ShellDialect::from_program(shell).unwrap_or(ShellDialect::Posix)
}

/// 引数の変数（`${args}`。#1732）
pub const ARGS_VARIABLE: &str = "${args}";

/// コマンド・cwd の値中の変数を展開。展開値はシングルクオートで自動エスケープ。
///
/// `${args}` は引用せずにそのまま置く（[`RunVars::args`]）。**空なら直前の空白 1 つごと消す**:
/// Windows のコンパイル行（`if ($?) { .\a.exe ${args} }`）を引数なしで展開した結果が
/// `${args}` を書く前と 1 バイトも変わらない（`拡張子既定の解決結果を両osで固定する` が縛る）
pub fn expand_variables(template: &str, vars: &RunVars) -> String {
    expand_with(template, vars, true)
}

/// 値を**引用せずに**展開する（#1732。実行設定の環境変数の値・作業ディレクトリ）。
///
/// 環境変数の値はシェルの方言の `quote_arg` が最後に 1 回だけ囲む（ここで囲むと二重になる）。
/// 作業ディレクトリはパスとして使う。展開するのはパスの変数と `${args}` だけで、
/// 実行環境の変数（`${python}` = 語の列）は残す
pub fn expand_literal(template: &str, vars: &RunVars) -> String {
    expand_with(template, vars, false)
}

fn expand_with(template: &str, vars: &RunVars, quote: bool) -> String {
    let q = |s: &str| {
        if quote {
            quote_for_shell(s)
        } else {
            s.to_string()
        }
    };
    let path = vars.path.as_path();
    let file_str = path.to_string_lossy();
    let root_str = vars.workspace_root.to_string_lossy();
    let file_dir = path
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file_base = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file_no_ext = path
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    let mut result = String::with_capacity(template.len() * 2);
    let mut chars = template.char_indices().peekable();

    while let Some((i, c)) = chars.next() {
        if c == '$' && template[i + 1..].starts_with('{') {
            // ${...} パターンの検出
            if let Some(end) = template[i + 2..].find('}') {
                let var_name = &template[i + 2..i + 2 + end];
                let replacement = match var_name {
                    "file" => Some(q(&file_str)),
                    "fileDir" => Some(q(&file_dir)),
                    "fileBase" if vars.base_absolute => Some(q(&file_str)),
                    "fileBase" => Some(q(&file_base)),
                    "fileNoExt" => Some(q(&file_no_ext)),
                    "ext" => Some(q(&ext)),
                    "workspaceRoot" => Some(q(&root_str)),
                    "args" => {
                        if vars.args.is_empty() && result.ends_with(' ') {
                            result.pop();
                        }
                        Some(vars.args.clone())
                    }
                    // 実行環境の変数（`${python}`。#1730）。未知は展開せずそのまま
                    other if quote => vars.runtime_value(other),
                    _ => None,
                };
                if let Some(rep) = replacement {
                    result.push_str(&rep);
                    // ${...} の分だけスキップ
                    for _ in 0..end + 2 {
                        chars.next();
                    }
                    continue;
                }
            }
        }
        result.push(c);
    }

    result
}

// --- 解決器 ---

/// 組み込み既定と settings 由来のユーザー定義をマージ。
/// ユーザー定義が優先。空文字列は無効化
pub fn merged_defaults(user_defaults: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    merged_defaults_for(Platform::current(), user_defaults)
}

/// 指定した OS の組み込み既定にユーザー定義を重ねる（#1655）
pub fn merged_defaults_for(
    platform: Platform,
    user_defaults: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for (ext, cmd) in builtin_defaults_for(platform) {
        map.insert(ext.to_string(), cmd.to_string());
    }
    for (ext, cmd) in user_defaults {
        if cmd.is_empty() {
            map.remove(ext);
        } else {
            map.insert(ext.clone(), cmd.clone());
        }
    }
    map
}

/// 実ファイルの解決（実行中の OS 向け）。**dispatch / 再生ボタンはこれを通す**（#1656）。
///
/// `user_defaults` は settings.json の `runner_defaults`（ユーザーが設定した拡張子既定）。
/// 組み込み既定との重ね合わせと、プロジェクト既定との優先順位はここで決める。
///
/// `runtime` は実行環境の層（`tako-control::runtime_probe`。#1730）が決めた `${python}` 等の値。
/// 空なら実行環境の変数は表の `fallback`（今までの既定表の名前）で展開する。
/// `layer` は実行設定の引数・作業ディレクトリ・実行前コマンド・環境変数（#1732。空 = 無し）
pub fn resolve_file(
    path: &Path,
    head: &str,
    user_defaults: &BTreeMap<String, String>,
    profile: Option<&str>,
    command_override: Option<&str>,
    runtime: &RuntimeWords,
    layer: &ConfigLayer,
) -> Result<Resolution, RunnerError> {
    // A/B（`TAKO_1656_LEGACY=1`）: プロジェクトを見ない #1656 以前の解決へ戻す
    let bounds = SearchBounds::for_user();
    let detect = !std::env::var("TAKO_1656_LEGACY").is_ok_and(|v| v == "1");
    resolve_file_in(
        Platform::current(),
        path,
        head,
        user_defaults,
        profile,
        command_override,
        detect.then_some(&bounds),
        runtime,
        layer,
    )
}

/// [`resolve_file`] の OS と探索範囲を外から渡す版（テスト・検査用）。
///
/// `bounds` が `None` ならプロジェクトを探さない（`${workspaceRoot}` はファイルのディレクトリ）
#[allow(clippy::too_many_arguments)]
pub fn resolve_file_in(
    platform: Platform,
    path: &Path,
    head: &str,
    user_defaults: &BTreeMap<String, String>,
    profile: Option<&str>,
    command_override: Option<&str>,
    bounds: Option<&SearchBounds>,
    runtime: &RuntimeWords,
    layer: &ConfigLayer,
) -> Result<Resolution, RunnerError> {
    let file_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let project = bounds.and_then(|b| runner_project::detect(platform, path, head, b, runtime));
    let workspace_root = match (&project, bounds) {
        (Some(p), _) => p.root.clone(),
        // 実行設定のプロジェクトのキー（`project_root::scope_root`。#1731）と同じ落とし先
        (None, Some(b)) => project_root::fallback_root(&file_dir, b),
        (None, None) => file_dir,
    };
    resolve_core(
        platform,
        path,
        head,
        &merged_defaults_for(platform, user_defaults),
        user_defaults,
        project,
        workspace_root,
        profile,
        command_override,
        runtime,
        layer,
    )
}

/// 宣言 + 拡張子既定 + オーバーライドから解決（実行中の OS 向け・プロジェクトを見ない）
#[cfg(test)]
pub(crate) fn resolve(
    path: &Path,
    head: &str,
    ext_defaults: &BTreeMap<String, String>,
    profile: Option<&str>,
    command_override: Option<&str>,
) -> Result<Resolution, RunnerError> {
    resolve_for(
        Platform::current(),
        path,
        head,
        ext_defaults,
        profile,
        command_override,
    )
}

/// OS を指定しての解決（#1655。プロジェクトを見ない = ファイルシステムに触らない）。
///
/// `platform` が効くのは**既定が見つからなかったときの案内**だけ
/// （どの既定表を渡すかは呼び出し側が決める）。分けてあるおかげで、
/// macOS 上から「Windows で `.zsh` を実行しようとした人に何が出るか」まで検査できる
#[cfg(test)]
pub(crate) fn resolve_for(
    platform: Platform,
    path: &Path,
    head: &str,
    ext_defaults: &BTreeMap<String, String>,
    profile: Option<&str>,
    command_override: Option<&str>,
) -> Result<Resolution, RunnerError> {
    let file_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    resolve_core(
        platform,
        path,
        head,
        ext_defaults,
        &BTreeMap::new(),
        None,
        file_dir,
        profile,
        command_override,
        &RuntimeWords::new(),
        &ConfigLayer::default(),
    )
}

/// 解決の核（ファイルシステムに触らない）。
///
/// - `ext_defaults`: 組み込み + ユーザー設定を重ねた拡張子既定
/// - `user_defaults`: そのうちユーザーが設定したもの（**プロジェクト既定より強い**のは
///   こちらだけ。明示の設定を tako の推定で上書きしない）
#[allow(clippy::too_many_arguments)]
fn resolve_core(
    platform: Platform,
    path: &Path,
    head: &str,
    ext_defaults: &BTreeMap<String, String>,
    user_defaults: &BTreeMap<String, String>,
    project: Option<ProjectMatch>,
    workspace_root: PathBuf,
    profile: Option<&str>,
    command_override: Option<&str>,
    runtime: &RuntimeWords,
    layer: &ConfigLayer,
) -> Result<Resolution, RunnerError> {
    let file_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let args = layer.args.as_deref().unwrap_or("");
    let mut vars = RunVars::new(path, &workspace_root)
        .on(platform)
        .with_runtime(runtime)
        .with_args(args);
    // 実行設定の引数（#1732）: テンプレートに `${args}` が無ければ末尾へ足す。
    // 末尾へ足すと壊れる形（Windows の `; if ($?) { .\a.exe }`）は表の側が `${args}` を明示する
    let with_args = |template: &str| -> String {
        if args.is_empty() || template.contains(ARGS_VARIABLE) {
            template.to_string()
        } else {
            format!("{template} {ARGS_VARIABLE}")
        }
    };
    // 実行設定の作業ディレクトリ（#1732）: 宣言の `tako:cwd` より強い（保存値はこのマシンの
    // 利用者の明示 = 設計書 §4.3）。相対はファイルのディレクトリ基準（`tako:cwd` と同じ意味論）
    let layer_cwd = layer.cwd.as_deref().map(|c| {
        let expanded = expand_literal(c, &vars);
        let p = Path::new(&expanded);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            file_dir.join(p)
        }
    });
    // 作業ディレクトリが移ったら `${fileBase}` は絶対パス（移った先からもファイルが引ける）
    vars.base_absolute = layer_cwd.as_ref().is_some_and(|c| c != &file_dir);
    let vars = vars;
    let expand = |template: &str| expand_variables(template, &vars);
    let cwd_or = |fallback: PathBuf| layer_cwd.clone().unwrap_or(fallback);
    // 実行前コマンドは実行ペインのシェルで走る（`tako:shell` の内側ではない）ので `vars` の方言で展開
    let before = layer
        .before
        .as_deref()
        .filter(|b| !b.trim().is_empty())
        .map(expand);
    let env: Vec<(String, String)> = layer
        .env
        .iter()
        .map(|(k, v)| (k.clone(), expand_literal(v, &vars)))
        .collect();

    let decls = parse_declarations(head);
    let mut warnings = decls.warnings.clone();

    // 全プロファイルの RunPlan を構築（ドロップダウン用）
    let mut all_profiles: Vec<RunPlan> = Vec::new();
    // 宣言が明示した作業ディレクトリ（プロファイル名 → 書いたままの値。実行設定の出典に使う）
    let mut declared_cwds: BTreeMap<String, String> = BTreeMap::new();

    // 1. コマンドオーバーライド
    if let Some(cmd_override) = command_override {
        let plan = RunPlan {
            profile: profile.unwrap_or("default").to_string(),
            command: expand(&with_args(cmd_override)),
            template: cmd_override.to_string(),
            cwd: cwd_or(file_dir.clone()),
            shell: None,
            source: RunSource::Override,
        };
        all_profiles.push(plan.clone());
        return Ok(Resolution {
            plan,
            all_profiles,
            warnings,
            project,
            workspace_root,
            declared_cwd: None,
            before,
            env,
        });
    }

    // 2. 宣言プロファイル
    for decl in &decls.profiles {
        if let Some(run_cmd) = &decl.run {
            // cwd 解決: プロファイル別 → 共通 → ファイルのディレクトリ
            let raw_cwd = declared_cwd(&decls, &decl.name);

            let resolved_cwd = if let Some(cwd_str) = raw_cwd {
                declared_cwds.insert(decl.name.clone(), cwd_str.to_string());
                let expanded = expand(cwd_str);
                // シングルクオート除去（expand_variables がクオートするが cwd はパスとして使う）
                let cleaned = strip_quotes(&expanded);
                let cwd_path = Path::new(&cleaned);
                if cwd_path.is_absolute() {
                    cwd_path.to_path_buf()
                } else {
                    file_dir.join(cwd_path)
                }
            } else {
                file_dir.clone()
            };

            // `tako:shell` で内側のシェルが変わるなら、実行環境の語はそのシェルの方言で囲む
            let template = with_args(run_cmd);
            let command = match &decl.shell {
                Some(shell) => {
                    expand_variables(&template, &vars.in_dialect(declared_dialect(shell)))
                }
                None => expand(&template),
            };
            all_profiles.push(RunPlan {
                profile: decl.name.clone(),
                command,
                template: run_cmd.clone(),
                cwd: cwd_or(resolved_cwd),
                shell: decl.shell.clone(),
                source: RunSource::Declaration,
            });
        }
    }

    // 3〜5. 宣言がない場合のフォールバック:
    //   ユーザーが設定した拡張子既定 → プロジェクト既定 → 組み込みの拡張子既定
    if all_profiles.is_empty() {
        // 空文字列は「組み込みを無効化」の意味（設計 §2.1）で、プロジェクト既定は止めない
        let user_set = user_defaults.get(&ext).is_some_and(|c| !c.is_empty());
        match &project {
            Some(p) if !user_set => {
                // 既定の後ろに種別が持つプロファイル（arduino の `upload`。#1950）。cwd はどれも
                // プロジェクトのルート
                let named = std::iter::once(("default", &p.command))
                    .chain(p.profiles.iter().map(|(n, c)| (n.as_str(), c)));
                for (name, command) in named {
                    // 組み立て済みの完成形（展開しない）なので、引数はそのまま末尾へ
                    let with = if args.is_empty() {
                        command.clone()
                    } else {
                        format!("{command} {args}")
                    };
                    all_profiles.push(RunPlan {
                        profile: name.to_string(),
                        command: with,
                        template: command.clone(),
                        cwd: cwd_or(p.root.clone()),
                        shell: None,
                        source: RunSource::ProjectDefault,
                    });
                }
            }
            _ => {
                if let Some(default_cmd) = ext_defaults.get(&ext).filter(|_| !ext.is_empty()) {
                    all_profiles.push(RunPlan {
                        profile: "default".to_string(),
                        command: expand(&with_args(default_cmd)),
                        template: default_cmd.clone(),
                        cwd: cwd_or(file_dir.clone()),
                        shell: None,
                        source: RunSource::ExtensionDefault,
                    });
                }
            }
        }
    }

    // プロジェクト既定（`cargo run` / `python -m pkg.mod` / `go run ./cmd`）はルートで走る前提で
    // 組んであるので、作業ディレクトリをルート以外へ移すと見つからないことがある
    // （#1732。止めずに知らせる）
    let moved_from_root = matches!((&layer_cwd, &project), (Some(c), Some(p)) if c != &p.root);
    if moved_from_root
        && all_profiles
            .iter()
            .any(|p| p.source == RunSource::ProjectDefault)
    {
        warnings.push(cwd_moved_project_warning());
    }

    // 指定されたプロファイルの選択
    if let Some(profile_name) = profile {
        // 明示指定時は宣言からのみ探す（拡張子既定へフォールバックしない）
        let plan = all_profiles
            .iter()
            .find(|p| p.profile == profile_name)
            .cloned()
            .ok_or_else(|| RunnerError::ProfileNotFound(profile_name.to_string()))?;
        let declared_cwd = declared_cwds.get(&plan.profile).cloned();
        return Ok(Resolution {
            plan,
            all_profiles,
            warnings,
            project,
            workspace_root,
            declared_cwd,
            before,
            env,
        });
    }

    // 既定選択: 無添字 `tako:run:` があればそれ、なければ最初に宣言されたプロファイル
    let plan = all_profiles
        .iter()
        .find(|p| p.profile == "default")
        .or_else(|| all_profiles.first())
        .cloned()
        .ok_or_else(|| RunnerError::NoCommand(no_command_hint(platform, &ext)))?;
    let declared_cwd = declared_cwds.get(&plan.profile).cloned();

    Ok(Resolution {
        plan,
        all_profiles,
        warnings,
        project,
        workspace_root,
        declared_cwd,
        before,
        env,
    })
}

/// 作業ディレクトリを移したときのプロジェクト既定の注意（日英。#1732）
fn cwd_moved_project_warning() -> String {
    match crate::i18n::lang() {
        crate::i18n::Lang::Ja => "作業ディレクトリ（実行設定の cwd）を移した。プロジェクト既定のコマンドは\
             プロジェクトのルートで走る前提なので、見つからないときは `--reset cwd` で戻すか、\
             ファイル先頭の `tako:run:` で絶対パス（`${file}`）を使う"
            .to_string(),
        crate::i18n::Lang::En => "The working directory (run config cwd) was moved. Project default commands \
             assume the project root; if something is not found, go back with `--reset cwd` or write \
             `tako:run:` at the top of the file with absolute paths (`${file}`)"
            .to_string(),
    }
}

/// 宣言の作業ディレクトリ（書いたままの値）。プロファイル別 → 共通（無添字の `tako:cwd`）
fn declared_cwd<'a>(decls: &'a Declarations, profile: &str) -> Option<&'a str> {
    let common = decls
        .profiles
        .iter()
        .find(|p| p.name == "default")
        .and_then(|p| p.cwd.as_deref());
    decls
        .profiles
        .iter()
        .find(|p| p.name == profile)
        .and_then(|p| p.cwd.as_deref())
        .or(common)
}

/// 実行コマンドが見つからなかったときの案内（純粋関数。**OS を引数で受ける**）。
///
/// 組み込み表がその OS で**意図して既定を置いていない**拡張子（`.zsh` on Windows /
/// `.bat` on macOS / `.sql` のように接続先が決まらないもの）は、
/// `platform::runner_defaults` が持つ理由を先頭に足す。理由を持たせずに
/// 「設定してください」とだけ言うと、ユーザーには「tako がこの拡張子を
/// 知らない」のか「置かないと決めてある」のかが区別できない（#1655）
fn no_command_hint(platform: Platform, ext: &str) -> String {
    let lang = crate::i18n::lang();
    let base = match lang {
        crate::i18n::Lang::Ja => {
            let target = if ext.is_empty() { "<拡張子>" } else { ext };
            format!(
                "ファイル先頭に `tako:run: <コマンド>` を書くか、\
                 `tako run-default {target} \"<コマンド>\"` で拡張子既定を設定してください"
            )
        }
        crate::i18n::Lang::En => {
            let target = if ext.is_empty() { "<ext>" } else { ext };
            format!(
                "Add `tako:run: <command>` at the top of the file, \
                 or set an extension default with `tako run-default {target} \"<command>\"`"
            )
        }
    };
    match crate::platform::runner_defaults::absent_reason(platform, ext) {
        Some(note) => {
            let sep = match lang {
                crate::i18n::Lang::Ja => "。",
                crate::i18n::Lang::En => " ",
            };
            format!("{}{sep}{base}", note.text_in(lang))
        }
        None => base,
    }
}

/// シングルクオートの除去（expand_variables がパスをクオートするが cwd では不要）
fn strip_quotes(s: &str) -> String {
    if s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2 {
        // エスケープされたクオートも元に戻す
        s[1..s.len() - 1].replace("'\\''", "'")
    } else {
        s.to_string()
    }
}

/// UI 向けの軽量ヘッド読み取り（16 KiB 上限。読み取り失敗は None）
pub fn read_file_head_for_ui(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; SCAN_MAX_BYTES];
    let n = file.read(&mut buf).ok()?;
    buf.truncate(n);
    Some(String::from_utf8_lossy(&buf).into_owned())
}

// --- テスト ---

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // --- parse_declarations ---

    #[test]
    fn 接頭辞バリエーション_ハッシュ() {
        let head = "# tako:run: echo hello\n";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 1);
        assert_eq!(decls.profiles[0].run.as_deref(), Some("echo hello"));
    }

    #[test]
    fn 接頭辞バリエーション_ダブルスラッシュ() {
        let head = "// tako:run: gcc main.c\n";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 1);
        assert_eq!(decls.profiles[0].run.as_deref(), Some("gcc main.c"));
    }

    #[test]
    fn 接頭辞バリエーション_パーセント() {
        let head = "% tako:run: latexmk -pdf report.tex\n";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 1);
    }

    #[test]
    fn 接頭辞バリエーション_ダブルダッシュ() {
        let head = "-- tako:run: ghci main.hs\n";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 1);
    }

    #[test]
    fn 接頭辞バリエーション_セミコロン() {
        let head = "; tako:run: clisp main.lisp\n";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 1);
    }

    #[test]
    fn 接頭辞バリエーション_html() {
        let head = "<!-- tako:run: open index.html -->\n";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 1);
        assert_eq!(decls.profiles[0].run.as_deref(), Some("open index.html"));
    }

    #[test]
    fn 接頭辞バリエーション_css_block_comment() {
        let head = "/* tako:run: sass style.scss */\n";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 1);
        assert_eq!(decls.profiles[0].run.as_deref(), Some("sass style.scss"));
    }

    #[test]
    fn 誤検知拒否_mytako() {
        let head = "mytako:run: should not match\n";
        let decls = parse_declarations(head);
        assert!(decls.profiles.is_empty());
    }

    #[test]
    fn 誤検知拒否_65行目は範囲外() {
        let mut head = String::new();
        for i in 1..=64 {
            head.push_str(&format!("// line {i}\n"));
        }
        head.push_str("# tako:run: should be ignored\n");
        let decls = parse_declarations(&head);
        assert!(decls.profiles.is_empty());
    }

    #[test]
    fn 誤検知拒否_大文字は無視() {
        let head = "# TAKO:RUN: echo hello\n";
        let decls = parse_declarations(head);
        assert!(decls.profiles.is_empty());
    }

    #[test]
    fn プロファイル複数_順序保持() {
        let head = "\
// tako:run: echo default
// tako:run[test]: echo test
// tako:run[build]: echo build
";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 3);
        assert_eq!(decls.profiles[0].name, "default");
        assert_eq!(decls.profiles[1].name, "test");
        assert_eq!(decls.profiles[2].name, "build");
    }

    #[test]
    fn 後勝ち_警告あり() {
        let head = "\
# tako:run: first
# tako:run: second
";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 1);
        assert_eq!(decls.profiles[0].run.as_deref(), Some("second"));
        assert_eq!(decls.warnings.len(), 1);
        assert!(decls.warnings[0].contains("重複"));
    }

    #[test]
    fn cwd宣言() {
        let head = "\
# tako:run: echo hello
# tako:cwd: ../build
";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles[0].cwd.as_deref(), Some("../build"));
    }

    #[test]
    fn プロファイル別cwd() {
        let head = "\
// tako:run[build]: make
// tako:cwd[build]: ../..
// tako:run: cc main.c
";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 2);
        let build = decls.profiles.iter().find(|p| p.name == "build").unwrap();
        assert_eq!(build.cwd.as_deref(), Some("../.."));
        let default = decls.profiles.iter().find(|p| p.name == "default").unwrap();
        assert!(default.cwd.is_none());
    }

    #[test]
    fn shell宣言() {
        let head = "# tako:shell: fish\n# tako:run: echo hello\n";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles[0].shell.as_deref(), Some("fish"));
    }

    #[test]
    fn crlf許容() {
        let head = "# tako:run: echo hello\r\n# tako:cwd: .\r\n";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 1);
        assert_eq!(decls.profiles[0].run.as_deref(), Some("echo hello"));
    }

    #[test]
    fn bom許容() {
        let head = "\u{feff}# tako:run: echo hello\n";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 1);
    }

    #[test]
    fn jinja_closer() {
        let head = "{# tako:run: python3 app.py #}\n";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 1);
        assert_eq!(decls.profiles[0].run.as_deref(), Some("python3 app.py"));
    }

    // --- expand_variables ---

    #[test]
    fn 変数展開_基本() {
        let path = PathBuf::from("/Users/a/src/main.c");
        let result = expand_variables("cc ${fileBase} -o ${fileNoExt}", &RunVars::for_file(&path));
        assert_eq!(result, "cc main.c -o main");
    }

    #[test]
    fn 変数展開_フルパス() {
        let path = PathBuf::from("/Users/a/src/main.c");
        let result = expand_variables("${file}", &RunVars::for_file(&path));
        assert_eq!(result, "/Users/a/src/main.c");
    }

    #[test]
    fn 変数展開_拡張子() {
        let path = PathBuf::from("/tmp/test.PY");
        let result = expand_variables("${ext}", &RunVars::for_file(&path));
        assert_eq!(result, "py");
    }

    #[test]
    fn 変数展開_空白パスはクオート() {
        let path = PathBuf::from("/Users/a/my project/main tool.c");
        let result = expand_variables("cc ${fileBase}", &RunVars::for_file(&path));
        assert_eq!(result, "cc 'main tool.c'");
    }

    #[test]
    fn 変数展開_日本語パス() {
        let path = PathBuf::from("/Users/a/ドキュメント/テスト.py");
        let result = expand_variables("python3 ${fileBase}", &RunVars::for_file(&path));
        assert_eq!(result, "python3 'テスト.py'");
    }

    #[test]
    fn 変数展開_シングルクオート含み() {
        let path = PathBuf::from("/tmp/it's.sh");
        let result = expand_variables("bash ${fileBase}", &RunVars::for_file(&path));
        assert_eq!(result, "bash 'it'\\''s.sh'");
    }

    #[test]
    fn 変数展開_未知変数はそのまま() {
        let path = PathBuf::from("/tmp/test.sh");
        let result = expand_variables("echo ${HOME} ${fileBase}", &RunVars::for_file(&path));
        assert_eq!(result, "echo ${HOME} test.sh");
    }

    #[test]
    #[allow(non_snake_case)]
    fn 変数展開_fileDir() {
        let path = PathBuf::from("/Users/a/src/main.c");
        let result = expand_variables("${fileDir}", &RunVars::for_file(&path));
        assert_eq!(result, "/Users/a/src");
    }

    // --- 実行環境の変数（#1730）---

    fn rt(words: &[&str]) -> RuntimeWords {
        let kind = &runtime_env::KINDS[0];
        BTreeMap::from([(
            kind.variable.to_string(),
            words.iter().map(|w| w.to_string()).collect(),
        )])
    }

    /// 受け入れ条件: 実行環境が無いときの `py` の展開が #1730 以前と両 OS でバイト一致。
    /// 変更前の表の行（macOS `python3 ${fileBase}` / Windows `python ${fileBase}`）を
    /// ここへ写して、引用の要る名前（空白・日本語・`'`・`$`）まで突き合わせる
    #[test]
    fn 実行環境が無いときのpyの展開は1730以前と両osでバイト一致() {
        let before = [
            (Platform::MacOs, "python3 ${fileBase}"),
            (Platform::Windows, "python ${fileBase}"),
        ];
        let names = [
            "a.py",
            "my script.py",
            "データ解析.py",
            "it's.py",
            "$HOME.py",
            "a;b.py",
        ];
        for (platform, old_template) in before {
            let now = crate::platform::runner_defaults::entry("py")
                .and_then(|e| e.get(platform).command())
                .expect("py の行がある");
            for name in names {
                let path = PathBuf::from(format!("/tmp/プロジェクト x/{name}"));
                let vars = RunVars::for_file(&path).on(platform);
                assert_eq!(
                    expand_variables(now, &vars),
                    expand_variables(old_template, &vars),
                    "{platform:?} {name}"
                );
                // 空の実行環境（層を通ったが値が無い）も同じ
                let empty = vars.clone().with_runtime(&BTreeMap::from([(
                    runtime_env::KINDS[0].variable.to_string(),
                    Vec::new(),
                )]));
                assert_eq!(
                    expand_variables(now, &empty),
                    expand_variables(old_template, &vars)
                );
            }
        }
    }

    /// #1735: `js` / `mjs` / `rs` の行を `${node}` / `${rustc}` にしても、実行環境が無いときの
    /// 展開は変更前の行（ここへ写す）と両 OS でバイト一致
    #[test]
    fn 実行環境が無いときのjsとrsの展開は1735以前と両osでバイト一致() {
        let before: &[(&str, Platform, &str)] = &[
            ("js", Platform::MacOs, "node ${fileBase}"),
            ("js", Platform::Windows, "node ${fileBase}"),
            ("mjs", Platform::MacOs, "node ${fileBase}"),
            ("mjs", Platform::Windows, "node ${fileBase}"),
            (
                "rs",
                Platform::MacOs,
                "rustc ${fileBase} -o ${fileNoExt} && ./${fileNoExt}",
            ),
            (
                "rs",
                Platform::Windows,
                "rustc ${fileBase} -o ${fileNoExt}.exe; if ($?) { .\\${fileNoExt}.exe ${args} }",
            ),
        ];
        for (ext, platform, old_template) in before {
            let now = crate::platform::runner_defaults::entry(ext)
                .and_then(|e| e.get(*platform).command())
                .unwrap_or_else(|| panic!("{ext} の行がある"));
            assert_ne!(now, *old_template, "{ext}: 変数へ変わっていない");
            for name in ["a", "my script", "データ解析", "it's", "$HOME", "a;b"] {
                let path = PathBuf::from(format!("/tmp/プロジェクト x/{name}.{ext}"));
                let vars = RunVars::for_file(&path).on(*platform);
                assert_eq!(
                    expand_variables(now, &vars),
                    expand_variables(old_template, &vars),
                    "{platform:?} {name}.{ext}"
                );
            }
        }
    }

    /// #1950: `${arduino_cli}` も同じ口で展開される（宣言から IDE 同梱の実体を使える）
    #[test]
    fn arduino_cliの変数は見つけた実体へ無ければ名前のままへ展開される() {
        let path = PathBuf::from("/tmp/Blink/Blink.ino");
        let tpl = "${arduino_cli} compile --fqbn arduino:avr:uno";
        for platform in [Platform::MacOs, Platform::Windows] {
            let vars = RunVars::for_file(&path).on(platform);
            assert_eq!(
                expand_variables(tpl, &vars),
                "arduino-cli compile --fqbn arduino:avr:uno",
                "{platform:?}"
            );
        }
        let ide = "/Applications/Arduino IDE.app/Contents/Resources/app/lib/backend/resources/arduino-cli";
        let words = BTreeMap::from([("arduino_cli".to_string(), vec![ide.to_string()])]);
        let vars = RunVars::for_file(&path)
            .on(Platform::MacOs)
            .with_runtime(&words);
        assert_eq!(
            expand_variables(tpl, &vars),
            format!("'{ide}' compile --fqbn arduino:avr:uno")
        );
    }

    /// #1950: プロジェクト既定の追加プロファイル（arduino の `upload`）が一覧に並び、名指しで選べる
    #[test]
    fn プロジェクト既定の追加プロファイルを名指しで選べる() {
        let dir = std::env::temp_dir().join(format!(
            "tako-1950-runner-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("Blink")).unwrap();
        let dir = crate::platform::path::canonicalize(&dir).unwrap();
        let ino = dir.join("Blink/Blink.ino");
        std::fs::write(&ino, "void setup() {}\n").unwrap();
        let bounds = SearchBounds::with_ceilings([dir.clone()]);
        let resolve = |profile: Option<&str>| {
            resolve_file_in(
                Platform::MacOs,
                &ino,
                "",
                &BTreeMap::new(),
                profile,
                None,
                Some(&bounds),
                &RuntimeWords::new(),
                &ConfigLayer::default(),
            )
        };
        let r = resolve(None).unwrap();
        let names: Vec<&str> = r.all_profiles.iter().map(|p| p.profile.as_str()).collect();
        assert_eq!(names, vec!["default", "upload"]);
        assert_eq!(r.plan.command, "arduino-cli compile");
        assert!(r
            .all_profiles
            .iter()
            .all(|p| p.source == RunSource::ProjectDefault && p.cwd == dir.join("Blink")));
        let up = resolve(Some("upload")).unwrap();
        assert_eq!(up.plan.command, "arduino-cli compile --upload");
        assert!(matches!(
            resolve(Some("nope")),
            Err(RunnerError::ProfileNotFound(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 実行環境の変数は方言で囲む() {
        let path = PathBuf::from("/p/a.py");
        let venv = rt(&["/my proj/.venv/bin/python"]);
        let mac = RunVars::for_file(&path)
            .on(Platform::MacOs)
            .with_runtime(&venv);
        assert_eq!(
            expand_variables("${python} ${fileBase}", &mac),
            "'/my proj/.venv/bin/python' a.py"
        );
        let win = RunVars::for_file(&path)
            .on(Platform::Windows)
            .with_runtime(&rt(&[r"C:\a b\.venv\Scripts\python.exe"]));
        assert_eq!(
            expand_variables("${python} -m pytest", &win),
            r"& 'C:\a b\.venv\Scripts\python.exe' -m pytest"
        );
        // 包む形（道具 + run + python）
        let wrapped = RunVars::for_file(&path)
            .on(Platform::MacOs)
            .with_runtime(&rt(&["/h/.local/bin/uv", "run", "python"]));
        assert_eq!(
            expand_variables("${python} ${fileBase}", &wrapped),
            "/h/.local/bin/uv run python a.py"
        );
        // 表に無い名前は今までどおり残す（#1735 で `${node}` は表に入ったので言語名でない名前で見る）
        assert_eq!(
            expand_variables("${notInTable} ${HOME}", &wrapped),
            "${notInTable} ${HOME}"
        );
    }

    #[test]
    fn 実行環境の値は宣言にも拡張子既定にも効き宣言のシェルの方言で囲む() {
        let path = PathBuf::from("/p/a.py");
        let venv = rt(&["/v env/bin/python"]);
        let defaults = merged_defaults_for(Platform::MacOs, &BTreeMap::new());
        let res = resolve_core(
            Platform::MacOs,
            &path,
            "",
            &defaults,
            &BTreeMap::new(),
            None,
            PathBuf::from("/p"),
            None,
            None,
            &venv,
            &ConfigLayer::default(),
        )
        .unwrap();
        assert_eq!(res.plan.command, "'/v env/bin/python' a.py");
        assert_eq!(res.plan.template, "${python} ${fileBase}");
        assert!(res.plan.uses_variable("python"));

        // Windows でも `tako:shell: bash` の宣言なら、語は POSIX で囲む（`&` を付けない）
        let head = "# tako:shell: bash\n# tako:run: ${python} -m pytest\n# tako:run[raw]: pytest\n";
        let win = resolve_core(
            Platform::Windows,
            &path,
            head,
            &merged_defaults_for(Platform::Windows, &BTreeMap::new()),
            &BTreeMap::new(),
            None,
            PathBuf::from("/p"),
            None,
            None,
            &venv,
            &ConfigLayer::default(),
        )
        .unwrap();
        assert_eq!(win.plan.command, "'/v env/bin/python' -m pytest");
        let raw = win
            .all_profiles
            .iter()
            .find(|p| p.profile == "raw")
            .unwrap();
        assert!(!raw.uses_variable("python"), "変数を使わない宣言");
        // 上書きのコマンドも同じ変数で展開する
        let over = resolve_core(
            Platform::MacOs,
            &path,
            "",
            &defaults,
            &BTreeMap::new(),
            None,
            PathBuf::from("/p"),
            None,
            Some("${python} -V"),
            &venv,
            &ConfigLayer::default(),
        )
        .unwrap();
        assert_eq!(over.plan.command, "'/v env/bin/python' -V");
    }

    #[test]
    fn 宣言の作業ディレクトリは書いたままの値で返る() {
        let path = PathBuf::from("/tmp/src/main.c");
        let head = "// tako:cwd: ${fileDir}/..\n// tako:run: make\n// tako:run[t]: make t\n// tako:cwd[t]: build\n";
        let defaults = merged_defaults(&BTreeMap::new());
        let res = resolve(&path, head, &defaults, None, None).unwrap();
        assert_eq!(res.declared_cwd.as_deref(), Some("${fileDir}/.."));
        let res = resolve(&path, head, &defaults, Some("t"), None).unwrap();
        assert_eq!(res.declared_cwd.as_deref(), Some("build"));
        let res = resolve(&path, "// tako:run: make\n", &defaults, None, None).unwrap();
        assert_eq!(res.declared_cwd, None);
    }

    // --- resolve ---

    #[test]
    fn 解決_宣言優先() {
        let path = PathBuf::from("/tmp/test.py");
        let head = "# tako:run: python3 -u ${fileBase}\n";
        let defaults = merged_defaults(&BTreeMap::new());
        let res = resolve(&path, head, &defaults, None, None).unwrap();
        assert_eq!(res.plan.source, RunSource::Declaration);
        assert_eq!(res.plan.command, "python3 -u test.py");
    }

    #[test]
    fn 解決_拡張子既定フォールバック() {
        let path = PathBuf::from("/tmp/test.py");
        let head = "#!/usr/bin/env python3\n";
        // 見ているのは**フォールバックの仕組み**なので OS を固定する（Windows 列は
        // `python test.py` = 別の値。列そのものは `拡張子既定の解決結果を両osで固定する`）
        let defaults = merged_defaults_for(Platform::MacOs, &BTreeMap::new());
        let res = resolve_for(Platform::MacOs, &path, head, &defaults, None, None).unwrap();
        assert_eq!(res.plan.source, RunSource::ExtensionDefault);
        assert_eq!(res.plan.command, "python3 test.py");
    }

    #[test]
    fn 解決_command拡張子は既定でbash実行() {
        // #453: 日本語パス + 宣言なし .command が拡張子既定で解決されること
        let path =
            PathBuf::from("/Users/a/Documents/クラウドコンピューティング/docs/start-docs.command");
        let head = "#!/usr/bin/env bash\n# 起動スクリプト\nset -e\n";
        let defaults = merged_defaults(&BTreeMap::new());
        let res = resolve(&path, head, &defaults, None, None).unwrap();
        assert_eq!(res.plan.source, RunSource::ExtensionDefault);
        assert_eq!(res.plan.command, "bash start-docs.command");
        assert_eq!(res.all_profiles.len(), 1);
    }

    #[test]
    fn 解決_オーバーライド最優先() {
        let path = PathBuf::from("/tmp/test.py");
        let head = "# tako:run: python3 ${fileBase}\n";
        let defaults = merged_defaults(&BTreeMap::new());
        let res = resolve(&path, head, &defaults, None, Some("echo overridden")).unwrap();
        assert_eq!(res.plan.source, RunSource::Override);
        assert_eq!(res.plan.command, "echo overridden");
    }

    #[test]
    fn 解決_プロファイル指定() {
        let path = PathBuf::from("/tmp/main.c");
        let head = "\
// tako:run: cc ${fileBase} -o ${fileNoExt}
// tako:run[test]: make test
";
        let defaults = merged_defaults(&BTreeMap::new());
        let res = resolve(&path, head, &defaults, Some("test"), None).unwrap();
        assert_eq!(res.plan.profile, "test");
        assert_eq!(res.plan.command, "make test");
    }

    #[test]
    fn 解決_プロファイル不一致はエラー() {
        let path = PathBuf::from("/tmp/main.c");
        let head = "// tako:run: cc main.c\n";
        let defaults = merged_defaults(&BTreeMap::new());
        let err = resolve(&path, head, &defaults, Some("nonexistent"), None).unwrap_err();
        assert!(matches!(err, RunnerError::ProfileNotFound(_)));
    }

    #[test]
    fn 解決_宣言なし_既定なし_エラー() {
        let path = PathBuf::from("/tmp/Makefile");
        let head = "all:\n\techo hello\n";
        let defaults = merged_defaults(&BTreeMap::new());
        let err = resolve(&path, head, &defaults, None, None).unwrap_err();
        assert!(matches!(err, RunnerError::NoCommand(_)));
    }

    #[test]
    fn 解決_cwd_ファイルディレクトリ既定() {
        let path = PathBuf::from("/tmp/src/main.py");
        let head = "# tako:run: python3 ${fileBase}\n";
        let defaults = merged_defaults(&BTreeMap::new());
        let res = resolve(&path, head, &defaults, None, None).unwrap();
        assert_eq!(res.plan.cwd, PathBuf::from("/tmp/src"));
    }

    #[test]
    fn 解決_cwd_相対パス() {
        let path = PathBuf::from("/tmp/src/main.c");
        let head = "// tako:run[build]: make\n// tako:cwd[build]: ..\n";
        let defaults = merged_defaults(&BTreeMap::new());
        let res = resolve(&path, head, &defaults, Some("build"), None).unwrap();
        assert_eq!(res.plan.cwd, PathBuf::from("/tmp/src/.."));
    }

    #[test]
    fn 解決_ユーザー既定が組み込みを上書き() {
        let path = PathBuf::from("/tmp/test.py");
        let head = "";
        let mut user = BTreeMap::new();
        user.insert("py".to_string(), "python ${fileBase}".to_string());
        let defaults = merged_defaults(&user);
        let res = resolve(&path, head, &defaults, None, None).unwrap();
        assert_eq!(res.plan.command, "python test.py");
    }

    #[test]
    fn 解決_空文字列で無効化() {
        let path = PathBuf::from("/tmp/test.py");
        let head = "";
        let mut user = BTreeMap::new();
        user.insert("py".to_string(), String::new());
        let defaults = merged_defaults(&user);
        let err = resolve(&path, head, &defaults, None, None).unwrap_err();
        assert!(matches!(err, RunnerError::NoCommand(_)));
    }

    #[test]
    fn 全プロファイル一覧() {
        let path = PathBuf::from("/tmp/main.c");
        let head = "\
// tako:run: cc ${fileBase} -o ${fileNoExt} && ./${fileNoExt}
// tako:run[build]: make
// tako:run[test]: make test
";
        let defaults = merged_defaults(&BTreeMap::new());
        let res = resolve(&path, head, &defaults, None, None).unwrap();
        assert_eq!(res.all_profiles.len(), 3);
        assert_eq!(res.all_profiles[0].profile, "default");
        assert_eq!(res.all_profiles[1].profile, "build");
        assert_eq!(res.all_profiles[2].profile, "test");
    }

    // --- merged_defaults ---

    #[test]
    fn 組み込み既定の存在確認() {
        let defaults = merged_defaults(&BTreeMap::new());
        for ext in ["py", "c", "command", "tex"] {
            assert!(defaults.contains_key(ext), "この OS の一覧に .{ext} が無い");
        }
    }

    /// `tako run-default`（引数なし）の一覧が返すのは `merged_defaults` そのもの
    /// （`dispatch::RunnerDefaults` が同じ 1 本を呼ぶ）。**両 OS で走らせて**、
    /// この OS の一覧に #1655 で足した拡張子が並ぶことを固定する
    #[test]
    fn 一覧にこのosで使える追加拡張子が並ぶ() {
        let defaults = merged_defaults(&BTreeMap::new());
        // どちらの OS でも既定を持つ 15 件
        for ext in [
            "ps1", "tsx", "jsx", "kt", "cs", "fs", "dart", "r", "jl", "scala", "hs", "clj", "ex",
            "zig", "nim",
        ] {
            assert!(defaults.contains_key(ext), "一覧に .{ext} が無い");
        }
        // 意図して置かない拡張子は一覧に出さない（出すと「設定済み」に見える）
        for ext in ["sql", "ipynb", "vb"] {
            assert!(!defaults.contains_key(ext), "一覧に .{ext} が出ている");
        }
        // OS 固有（macOS では `.bat` を出さない / Windows では `.zsh` を出さない）
        let mac = merged_defaults_for(Platform::MacOs, &BTreeMap::new());
        let win = merged_defaults_for(Platform::Windows, &BTreeMap::new());
        assert!(!mac.contains_key("bat") && win.contains_key("bat"));
        assert!(mac.contains_key("zsh") && !win.contains_key("zsh"));
    }

    // --- builtin_defaults ---

    #[test]
    fn 組み込み既定テーブル_重複なし() {
        let builtins = builtin_defaults();
        let mut seen = std::collections::HashSet::new();
        for (ext, _) in builtins {
            assert!(seen.insert(ext), "拡張子 '{ext}' が重複");
        }
    }

    // --- OS 別の組み込み既定（#1655）---

    /// 拡張子 1 つぶんの**解決結果**（`None` = その OS では既定を置かない）。
    ///
    /// ここに書いてあるのは `tako run hello.<ext>` が実際に組み立てるコマンド
    /// （`${fileBase}` 等を展開した後）。表の書き方ではなく**出てくる値**を固定するので、
    /// 表の構造を変えても意味が保たれる。
    ///
    /// **macOS 列は #1655 以前と 1 マスも変えていない**（A/B の正本もここ）
    const 解決の期待値: &[(&str, Option<&str>, Option<&str>)] = &[
        // ext, macOS, Windows
        (
            "command",
            Some("bash hello.command"),
            Some("bash hello.command"),
        ),
        ("sh", Some("bash hello.sh"), Some("bash hello.sh")),
        ("bash", Some("bash hello.bash"), Some("bash hello.bash")),
        ("zsh", Some("zsh hello.zsh"), None),
        ("py", Some("python3 hello.py"), Some("python hello.py")),
        ("js", Some("node hello.js"), Some("node hello.js")),
        ("mjs", Some("node hello.mjs"), Some("node hello.mjs")),
        ("ts", Some("npx tsx hello.ts"), Some("npx tsx hello.ts")),
        ("rb", Some("ruby hello.rb"), Some("ruby hello.rb")),
        ("pl", Some("perl hello.pl"), Some("perl hello.pl")),
        ("php", Some("php hello.php"), Some("php hello.php")),
        ("lua", Some("lua hello.lua"), Some("lua hello.lua")),
        (
            "c",
            Some("cc hello.c -o hello && ./hello"),
            Some("gcc hello.c -o hello.exe; if ($?) { .\\hello.exe }"),
        ),
        (
            "cpp",
            Some("c++ hello.cpp -o hello && ./hello"),
            Some("g++ hello.cpp -o hello.exe; if ($?) { .\\hello.exe }"),
        ),
        (
            "cc",
            Some("c++ hello.cc -o hello && ./hello"),
            Some("g++ hello.cc -o hello.exe; if ($?) { .\\hello.exe }"),
        ),
        (
            "cxx",
            Some("c++ hello.cxx -o hello && ./hello"),
            Some("g++ hello.cxx -o hello.exe; if ($?) { .\\hello.exe }"),
        ),
        (
            "rs",
            Some("rustc hello.rs -o hello && ./hello"),
            Some("rustc hello.rs -o hello.exe; if ($?) { .\\hello.exe }"),
        ),
        ("go", Some("go run hello.go"), Some("go run hello.go")),
        ("java", Some("java hello.java"), Some("java hello.java")),
        (
            "swift",
            Some("swift hello.swift"),
            Some("swift hello.swift"),
        ),
        (
            "tex",
            Some("latexmk -pdf -interaction=nonstopmode hello.tex"),
            Some("latexmk -pdf -interaction=nonstopmode hello.tex"),
        ),
        // ── #1655 で追加した 20 拡張子 ──
        (
            "ps1",
            Some("pwsh -NoLogo -NoProfile -File hello.ps1"),
            Some("pwsh -NoLogo -NoProfile -File hello.ps1"),
        ),
        ("bat", None, Some("cmd /c hello.bat")),
        ("cmd", None, Some("cmd /c hello.cmd")),
        ("tsx", Some("npx tsx hello.tsx"), Some("npx tsx hello.tsx")),
        ("jsx", Some("npx tsx hello.jsx"), Some("npx tsx hello.jsx")),
        (
            "kt",
            Some("kotlinc hello.kt -include-runtime -d hello.jar && java -jar hello.jar"),
            Some("kotlinc hello.kt -include-runtime -d hello.jar; if ($?) { java -jar hello.jar }"),
        ),
        (
            "cs",
            Some("dotnet run hello.cs"),
            Some("dotnet run hello.cs"),
        ),
        (
            "fs",
            Some("dotnet fsi hello.fs"),
            Some("dotnet fsi hello.fs"),
        ),
        ("vb", None, None),
        (
            "dart",
            Some("dart run hello.dart"),
            Some("dart run hello.dart"),
        ),
        ("r", Some("Rscript hello.r"), Some("Rscript hello.r")),
        ("jl", Some("julia hello.jl"), Some("julia hello.jl")),
        (
            "scala",
            Some("scala-cli run hello.scala"),
            Some("scala-cli run hello.scala"),
        ),
        ("hs", Some("runghc hello.hs"), Some("runghc hello.hs")),
        ("clj", Some("clj -M hello.clj"), Some("clj -M hello.clj")),
        ("ex", Some("elixir hello.ex"), Some("elixir hello.ex")),
        ("zig", Some("zig run hello.zig"), Some("zig run hello.zig")),
        ("nim", Some("nim r hello.nim"), Some("nim r hello.nim")),
        ("sql", None, None),
        ("ipynb", None, None),
    ];

    fn 期待値の列<'a>(
        row: &(&'a str, Option<&'a str>, Option<&'a str>),
        platform: Platform,
    ) -> Option<&'a str> {
        match platform {
            Platform::MacOs => row.1,
            Platform::Windows => row.2,
        }
    }

    #[test]
    fn 拡張子既定の解決結果を両osで固定する() {
        for platform in [Platform::MacOs, Platform::Windows] {
            let defaults = merged_defaults_for(platform, &BTreeMap::new());
            for row in 解決の期待値 {
                let ext = row.0;
                let path = PathBuf::from(format!("/tmp/proj/hello.{ext}"));
                let got = resolve_for(platform, &path, "", &defaults, None, None);
                match 期待値の列(row, platform) {
                    Some(expected) => {
                        let res = got.unwrap_or_else(|e| {
                            panic!("{platform:?} の .{ext} が解決できない: {e}")
                        });
                        assert_eq!(
                            res.plan.command, expected,
                            "{platform:?} の .{ext} の解決結果"
                        );
                        assert_eq!(res.plan.source, RunSource::ExtensionDefault);
                        assert_eq!(res.plan.cwd, PathBuf::from("/tmp/proj"));
                    }
                    None => {
                        let err = got.err().unwrap_or_else(|| {
                            panic!("{platform:?} の .{ext} に既定が無いはずが解決できた")
                        });
                        let msg = err.to_string();
                        // 「知らない拡張子」ではなく「置かないと決めた理由」が出る
                        let note = crate::platform::runner_defaults::absent_reason(platform, ext)
                            .unwrap_or_else(|| panic!("{platform:?} の .{ext} に理由が無い"));
                        assert!(
                            msg.contains(note.text()),
                            "{platform:?} の .{ext} の案内に理由が載っていない: {msg}"
                        );
                        assert!(
                            msg.contains(&format!("tako run-default {ext}")),
                            "{platform:?} の .{ext} の案内に設定コマンドが無い: {msg}"
                        );
                    }
                }
            }
        }
    }

    /// 期待値の表と組み込み表が**同じ拡張子の集合**を見ている。
    ///
    /// 拡張子を足したのに期待値を書き忘れる（= 新しい既定が誰にも検査されない）形を落とす
    #[test]
    fn 期待値の表は組み込み表と同じ拡張子を網羅する() {
        let 表: std::collections::BTreeSet<&str> = crate::platform::runner_defaults::TABLE
            .iter()
            .map(|e| e.ext)
            .collect();
        let 期待: std::collections::BTreeSet<&str> = 解決の期待値.iter().map(|row| row.0).collect();
        assert_eq!(
            表, 期待,
            "組み込み表と期待値の表がずれている（拡張子を足したら期待値も足す）"
        );
    }

    /// 理由が無い（= 表にすら無い）拡張子は、理由抜きの案内だけが出る
    #[test]
    fn 表に無い拡張子の案内には理由が付かない() {
        let defaults = merged_defaults_for(Platform::Windows, &BTreeMap::new());
        let path = PathBuf::from("/tmp/proj/hello.xyz");
        let err = resolve_for(Platform::Windows, &path, "", &defaults, None, None)
            .expect_err("既定が無いので失敗する");
        let msg = err.to_string();
        assert!(msg.contains("tako run-default xyz"), "{msg}");
        assert!(msg.contains("tako:run:"), "{msg}");
    }

    /// 宣言（`tako:run:`）は OS に関係なくそのまま使われる（既定表より優先）
    #[test]
    fn 宣言は両osで既定より優先される() {
        let path = PathBuf::from("/tmp/proj/hello.py");
        let head = "# tako:run: uv run ${fileBase}\n";
        for platform in [Platform::MacOs, Platform::Windows] {
            let defaults = merged_defaults_for(platform, &BTreeMap::new());
            let res = resolve_for(platform, &path, head, &defaults, None, None).unwrap();
            assert_eq!(res.plan.command, "uv run hello.py");
            assert_eq!(res.plan.source, RunSource::Declaration);
        }
    }

    /// ユーザー定義は OS 別の既定を上書きし、空文字列は**その OS の既定ごと**消す
    #[test]
    fn ユーザー定義は両osの既定を上書きする() {
        let mut user = BTreeMap::new();
        user.insert("py".to_string(), "python -X utf8 ${fileBase}".to_string());
        user.insert("bat".to_string(), String::new());
        for platform in [Platform::MacOs, Platform::Windows] {
            let merged = merged_defaults_for(platform, &user);
            assert_eq!(
                merged.get("py").map(String::as_str),
                Some("python -X utf8 ${fileBase}")
            );
            assert!(
                !merged.contains_key("bat"),
                "{platform:?}: 無効化できていない"
            );
        }
    }

    // --- 接頭辞のバウンダリ ---

    #[test]
    fn 接頭辞_行頭直接() {
        let head = "tako:run: echo hello\n";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 1);
    }

    #[test]
    fn 接頭辞_長すぎは無視() {
        // 17 文字の接頭辞（制限は 16）
        let head = "0123456789ABCDEFG tako:run: echo hello\n";
        let decls = parse_declarations(head);
        assert!(decls.profiles.is_empty());
    }

    #[test]
    fn 共通cwdはプロファイル別にもフォールバック() {
        let path = PathBuf::from("/tmp/src/main.c");
        let head = "\
// tako:cwd: /tmp/build
// tako:run: cc ${fileBase}
// tako:run[test]: make test
";
        let defaults = merged_defaults(&BTreeMap::new());
        let res = resolve(&path, head, &defaults, Some("test"), None).unwrap();
        // test プロファイルには個別 cwd がないので共通 cwd が使われる
        assert_eq!(res.plan.cwd, PathBuf::from("/tmp/build"));
    }

    // --- プロジェクト既定（#1656）---

    /// 一時 dir にプロジェクトを組む箱。天井は箱の根なので実 HOME を読まない
    struct Proj {
        base: PathBuf,
        bounds: SearchBounds,
    }

    impl Proj {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "tako-1656-runner-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(&base).unwrap();
            let base = crate::platform::path::canonicalize(&base).unwrap();
            let bounds = SearchBounds::with_ceilings([base.clone()]);
            Self { base, bounds }
        }

        fn write(&self, rel: &str, content: &str) -> PathBuf {
            let path = self.base.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, content).unwrap();
            path
        }

        fn resolve(
            &self,
            platform: Platform,
            rel: &str,
            user: &BTreeMap<String, String>,
        ) -> Result<Resolution, RunnerError> {
            let file = self.base.join(rel);
            let head = std::fs::read_to_string(&file).unwrap_or_default();
            resolve_file_in(
                platform,
                &file,
                &head,
                user,
                None,
                None,
                Some(&self.bounds),
                &RuntimeWords::new(),
                &ConfigLayer::default(),
            )
        }
    }

    impl Drop for Proj {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    /// 受け入れ条件 1: cargo / npm / python / go のプロジェクト内のファイルは
    /// 種別の既定・cwd = ルート・source = ProjectDefault（両 OS の列を固定）
    #[test]
    fn プロジェクトの中のファイルはプロジェクト既定で解決する() {
        let p = Proj::new("kinds");
        p.write("rs/Cargo.toml", "[package]\nname = \"demo\"\n");
        p.write("rs/src/main.rs", "fn main() {}\n");
        p.write("js/package.json", r#"{"scripts":{"start":"node app.js"}}"#);
        p.write("js/app.js", "");
        p.write("py/pyproject.toml", "");
        p.write("py/pkg/cli.py", "");
        p.write("go/go.mod", "module m\n");
        p.write("go/cmd/srv/main.go", "package main\n");
        // (ファイル, ルート, macOS, Windows)
        let cases = [
            ("rs/src/main.rs", "rs", "cargo run", "cargo run"),
            ("js/app.js", "js", "npm run start", "npm run start"),
            (
                "py/pkg/cli.py",
                "py",
                "python3 -m pkg.cli",
                "python -m pkg.cli",
            ),
            (
                "go/cmd/srv/main.go",
                "go",
                "go run ./cmd/srv",
                "go run ./cmd/srv",
            ),
        ];
        for (rel, root, mac, win) in cases {
            for (platform, expected) in [(Platform::MacOs, mac), (Platform::Windows, win)] {
                let res = p
                    .resolve(platform, rel, &BTreeMap::new())
                    .unwrap_or_else(|e| panic!("{platform:?} の {rel} が解決できない: {e}"));
                assert_eq!(
                    res.plan.source,
                    RunSource::ProjectDefault,
                    "{platform:?} {rel}"
                );
                assert_eq!(res.plan.command, expected, "{platform:?} {rel}");
                assert_eq!(res.plan.cwd, p.base.join(root), "{platform:?} {rel}");
                assert_eq!(res.workspace_root, p.base.join(root));
                assert_eq!(res.all_profiles.len(), 1);
            }
        }
    }

    /// Issue の症状そのもの: cargo の lib crate の `.rs` が `rustc` 単体へ落ちない
    #[test]
    fn cargoのlib_crateはrustc単体へ落ちない() {
        let p = Proj::new("issue");
        p.write("repo/Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n");
        p.write(
            "repo/crates/tako-core/Cargo.toml",
            "[package]\nname = \"tako-core\"\n",
        );
        p.write("repo/crates/tako-core/src/lib.rs", "pub mod runner;\n");
        p.write("repo/crates/tako-core/src/runner.rs", "//! doc\n");
        for platform in [Platform::MacOs, Platform::Windows] {
            let res = p
                .resolve(
                    platform,
                    "repo/crates/tako-core/src/runner.rs",
                    &BTreeMap::new(),
                )
                .unwrap();
            assert_eq!(res.plan.command, "cargo test -p tako-core --lib runner::");
            assert_eq!(res.plan.cwd, p.base.join("repo"));
            assert!(!res.plan.command.contains("rustc"), "{}", res.plan.command);
            let project = res.project.expect("プロジェクトが載る");
            assert_eq!(project.kind, "cargo");
            assert_eq!(
                project.marker,
                p.base.join("repo/crates/tako-core/Cargo.toml")
            );
        }
    }

    /// 受け入れ条件 2（前半）: `tako:run` 宣言はプロジェクト既定より勝つ
    #[test]
    fn 宣言はプロジェクト既定より勝つ() {
        let p = Proj::new("decl");
        p.write("rs/Cargo.toml", "[package]\nname = \"demo\"\n");
        p.write(
            "rs/src/main.rs",
            "// tako:run: cargo run --release\n// tako:run[check]: cargo clippy\nfn main() {}\n",
        );
        for platform in [Platform::MacOs, Platform::Windows] {
            let res = p
                .resolve(platform, "rs/src/main.rs", &BTreeMap::new())
                .unwrap();
            assert_eq!(res.plan.source, RunSource::Declaration);
            assert_eq!(res.plan.command, "cargo run --release");
            // 宣言の cwd は従来どおりファイルのディレクトリ（プロジェクト既定の cwd を持ち込まない）
            assert_eq!(res.plan.cwd, p.base.join("rs/src"));
            assert_eq!(
                res.all_profiles.len(),
                2,
                "プロジェクト既定は一覧に混ざらない"
            );
            // 宣言が勝っても、どのプロジェクトの中かは応答に載る
            assert_eq!(res.project.map(|m| m.kind), Some("cargo"));
        }
    }

    /// ユーザーが `tako run-default` で設定した拡張子既定は、tako の推定（プロジェクト既定）より強い。
    /// 空文字列（組み込みの無効化）はプロジェクト既定を止めない
    #[test]
    fn ユーザーが設定した拡張子既定はプロジェクト既定より勝つ() {
        let p = Proj::new("user");
        p.write("py/pyproject.toml", "");
        p.write("py/pkg/cli.py", "");
        let mut user = BTreeMap::new();
        user.insert("py".to_string(), "python3.12 ${fileBase}".to_string());
        let res = p.resolve(Platform::MacOs, "py/pkg/cli.py", &user).unwrap();
        assert_eq!(res.plan.source, RunSource::ExtensionDefault);
        assert_eq!(res.plan.command, "python3.12 cli.py");
        assert_eq!(res.plan.cwd, p.base.join("py/pkg"));

        user.insert("py".to_string(), String::new());
        let res = p.resolve(Platform::MacOs, "py/pkg/cli.py", &user).unwrap();
        assert_eq!(res.plan.source, RunSource::ProjectDefault);
        assert_eq!(res.plan.command, "python3 -m pkg.cli");

        // 別の拡張子のユーザー設定は効かない
        let mut other = BTreeMap::new();
        other.insert("rb".to_string(), "ruby -w ${fileBase}".to_string());
        let res = p.resolve(Platform::MacOs, "py/pkg/cli.py", &other).unwrap();
        assert_eq!(res.plan.source, RunSource::ProjectDefault);
    }

    /// 受け入れ条件 2（後半）: マーカーが無ければ従来の拡張子既定（回帰なし）
    #[test]
    fn マーカーが無ければ従来の拡張子既定() {
        let p = Proj::new("nomarker");
        p.write("loose/hello.rs", "fn main() {}\n");
        p.write("loose/hello.py", "");
        p.write("loose/hello.go", "package main\n");
        p.write("loose/hello.js", "");
        for platform in [Platform::MacOs, Platform::Windows] {
            for ext in ["rs", "py", "go", "js"] {
                let rel = format!("loose/hello.{ext}");
                let res = p.resolve(platform, &rel, &BTreeMap::new()).unwrap();
                // #1655 の期待値の表と同じ値（拡張子既定の解決結果を両osで固定する）
                let row = 解決の期待値.iter().find(|r| r.0 == ext).unwrap();
                assert_eq!(
                    Some(res.plan.command.as_str()),
                    期待値の列(row, platform),
                    "{platform:?} {rel}"
                );
                assert_eq!(res.plan.source, RunSource::ExtensionDefault);
                assert_eq!(res.plan.cwd, p.base.join("loose"));
                assert!(res.project.is_none());
            }
        }
    }

    /// 探索しない（`bounds = None` = `TAKO_1656_LEGACY=1` の形）ときは #1656 以前と同じ
    #[test]
    fn 探索しなければ1656以前の解決() {
        let p = Proj::new("legacy");
        p.write("rs/Cargo.toml", "[package]\nname = \"demo\"\n");
        let file = p.write("rs/src/main.rs", "fn main() {}\n");
        let res = resolve_file_in(
            Platform::MacOs,
            &file,
            "fn main() {}\n",
            &BTreeMap::new(),
            None,
            None,
            None,
            &RuntimeWords::new(),
            &ConfigLayer::default(),
        )
        .unwrap();
        assert_eq!(res.plan.source, RunSource::ExtensionDefault);
        assert_eq!(res.plan.command, "rustc main.rs -o main && ./main");
        assert_eq!(res.workspace_root, p.base.join("rs/src"));
    }

    #[test]
    fn makefileは拡張子が無くてもプロジェクト既定で走る() {
        let p = Proj::new("make");
        p.write("c/Makefile", "run: app\n\t./app\n");
        let res = p
            .resolve(Platform::Windows, "c/Makefile", &BTreeMap::new())
            .unwrap();
        assert_eq!(res.plan.command, "make run");
        assert_eq!(res.plan.source, RunSource::ProjectDefault);
        // プロファイル名 default で明示しても引ける（再生ボタンの既定プロファイル）
        let file = p.base.join("c/Makefile");
        let res = resolve_file_in(
            Platform::MacOs,
            &file,
            "",
            &BTreeMap::new(),
            Some("default"),
            None,
            Some(&p.bounds),
            &RuntimeWords::new(),
            &ConfigLayer::default(),
        )
        .unwrap();
        assert_eq!(res.plan.command, "make run");
    }

    #[test]
    fn workspace_root変数はプロジェクトのルートへ展開される() {
        let p = Proj::new("wsroot");
        p.write("my proj/Cargo.toml", "[package]\nname = \"demo\"\n");
        p.write(
            "my proj/src/main.rs",
            "// tako:run: ls ${workspaceRoot}\n// tako:cwd: ${workspaceRoot}\n",
        );
        let res = p
            .resolve(Platform::MacOs, "my proj/src/main.rs", &BTreeMap::new())
            .unwrap();
        let root = p.base.join("my proj");
        assert_eq!(
            res.plan.command,
            format!("ls {}", quote_for_shell(&root.to_string_lossy()))
        );
        assert_eq!(res.plan.cwd, root, "cwd はクオートを外したパス");
        assert_eq!(res.workspace_root, root);
    }

    /// プロジェクトが無ければ git のルート、それも無ければファイルのディレクトリ
    #[test]
    fn workspace_root変数のフォールバック() {
        let p = Proj::new("wsroot-fallback");
        std::fs::create_dir_all(p.base.join("repo/.git")).unwrap();
        p.write("repo/docs/run.sh", "# tako:run: echo ${workspaceRoot}\n");
        p.write("plain/run.sh", "# tako:run: echo ${workspaceRoot}\n");
        let res = p
            .resolve(Platform::MacOs, "repo/docs/run.sh", &BTreeMap::new())
            .unwrap();
        assert_eq!(res.workspace_root, p.base.join("repo"));
        assert!(res.project.is_none());
        let res = p
            .resolve(Platform::MacOs, "plain/run.sh", &BTreeMap::new())
            .unwrap();
        assert_eq!(res.workspace_root, p.base.join("plain"));
        // プロジェクトを知らない展開はファイルのディレクトリ
        let path = PathBuf::from("/tmp/src/main.c");
        assert_eq!(
            expand_variables("${workspaceRoot}", &RunVars::for_file(&path)),
            "/tmp/src"
        );
    }

    /// `project_root::detect`（#1726 が実行設定のキーに使う）は**実行の cwd と同じ答え**を返す。
    /// ずれると「project に保存した設定が効かない」になる。先頭（head）を見る種別（Go）も含める
    #[test]
    fn project_root_detectは実行のcwdと同じ答え() {
        let p = Proj::new("same-root");
        p.write("ws/Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n");
        p.write("ws/crates/core/Cargo.toml", "[package]\nname = \"core\"\n");
        p.write("ws/crates/core/src/lib.rs", "");
        p.write("ws/crates/core/src/a.rs", "");
        p.write("go/go.mod", "module m\n");
        p.write("go/internal/x/x.go", "package x\n");
        p.write("py/pyproject.toml", "");
        p.write("py/pkg/cli.py", "");
        p.write("js/packages/web/package.json", "{}");
        p.write("js/packages/web/src/a.ts", "");
        p.write("c/Makefile", "all:\n");
        p.write("c/src/main.c", "");
        p.write("loose/x.rs", "");
        for rel in [
            "ws/crates/core/src/a.rs",
            "go/internal/x/x.go",
            "py/pkg/cli.py",
            "js/packages/web/src/a.ts",
            "c/src/main.c",
            "loose/x.rs",
        ] {
            let file = p.base.join(rel);
            let info = crate::project_root::detect_in(&file, &p.bounds);
            let res = p.resolve(Platform::current(), rel, &BTreeMap::new());
            let run_root = res.ok().and_then(|r| r.project).map(|m| (m.root, m.kind));
            assert_eq!(info.map(|i| (i.root, i.kind)), run_root, "{rel}");
        }
        // cargo の member は印の段（パッケージ）ではなくワークスペースのルート
        let info =
            crate::project_root::detect_in(&p.base.join("ws/crates/core/src/a.rs"), &p.bounds);
        assert_eq!(info.map(|i| i.root), Some(p.base.join("ws")));
    }

    /// 実行設定のプロジェクトのキー（`project_root::scope_root`。#1731）は、種別の印が
    /// **無い**ファイルでも `${workspaceRoot}` と同じ答え（リポジトリのルート → ファイルの段）
    #[test]
    fn 実行設定のプロジェクトのキーはworkspace_rootと同じ() {
        let p = Proj::new("scope-root");
        p.write("ws/Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n");
        p.write("ws/crates/core/Cargo.toml", "[package]\nname = \"core\"\n");
        p.write("ws/crates/core/src/a.rs", "");
        p.write("py/pyproject.toml", "");
        p.write("py/pkg/cli.py", "");
        p.write("repo/.git/HEAD", "");
        p.write("repo/tools/run.py", "");
        p.write("plain/.venv/pyvenv.cfg", "");
        p.write("plain/a.py", "");
        for (rel, want) in [
            ("ws/crates/core/src/a.rs", "ws"),
            ("py/pkg/cli.py", "py"),
            ("repo/tools/run.py", "repo"),
            ("plain/a.py", "plain"),
        ] {
            let file = p.base.join(rel);
            let key = crate::project_root::scope_root_in(&file, &p.bounds);
            let res = p
                .resolve(Platform::current(), rel, &BTreeMap::new())
                .unwrap_or_else(|e| panic!("{rel}: {e}"));
            assert_eq!(key, res.workspace_root, "{rel}");
            assert_eq!(key, p.base.join(want), "{rel}");
        }
    }

    #[test]
    fn 出典の名前() {
        assert_eq!(RunSource::Declaration.as_str(), "declaration");
        assert_eq!(RunSource::ExtensionDefault.as_str(), "extension_default");
        assert_eq!(RunSource::ProjectDefault.as_str(), "project_default");
        assert_eq!(RunSource::Override.as_str(), "override");
    }
}

/// #1732: 実行設定の引数・作業ディレクトリ・実行前コマンド・環境変数を解決へ重ねる
#[cfg(test)]
mod tests_1732 {
    use super::*;

    fn layer() -> ConfigLayer {
        ConfigLayer::default()
    }

    /// プロジェクトを探さない解決（ファイルシステムに触らない）
    fn resolve_with(platform: Platform, path: &str, head: &str, layer: &ConfigLayer) -> Resolution {
        resolve_file_in(
            platform,
            Path::new(path),
            head,
            &BTreeMap::new(),
            None,
            None,
            None,
            &RuntimeWords::new(),
            layer,
        )
        .unwrap()
    }

    fn with_args(args: &str) -> ConfigLayer {
        ConfigLayer {
            args: Some(args.into()),
            ..layer()
        }
    }

    #[test]
    fn 引数はテンプレートに無ければ末尾へ足す() {
        let res = resolve_with(
            Platform::MacOs,
            "/tmp/p/a.py",
            "",
            &with_args("--name 'a b' x"),
        );
        assert_eq!(res.plan.command, "python3 a.py --name 'a b' x");
        // 宣言でも同じ
        let res = resolve_with(
            Platform::MacOs,
            "/tmp/p/a.py",
            "# tako:run: python3 -u ${fileBase}\n",
            &with_args("-v"),
        );
        assert_eq!(res.plan.command, "python3 -u a.py -v");
        // テンプレートは書いたまま（実行環境の変数を使っているかの判定に使う）
        assert_eq!(res.plan.template, "python3 -u ${fileBase}");
    }

    #[test]
    fn 引数の変数はその位置に引用せず置く() {
        let head = "# tako:run: echo ${args} done\n";
        let res = resolve_with(Platform::MacOs, "/tmp/p/a.sh", head, &with_args("1 '2 3'"));
        assert_eq!(res.plan.command, "echo 1 '2 3' done");
        // 引数が無ければ直前の空白ごと消える
        let res = resolve_with(Platform::MacOs, "/tmp/p/a.sh", head, &layer());
        assert_eq!(res.plan.command, "echo done");
    }

    /// 引数なし（未設定・空文字）の展開は #1732 以前と 1 バイトも変わらない（両 OS の全行）
    #[test]
    fn 引数が無ければ組み込み表の展開は変わらない() {
        for platform in [Platform::MacOs, Platform::Windows] {
            for (ext, cmd) in builtin_defaults_for(platform) {
                let path = format!("/tmp/p/hello.{ext}");
                let none = resolve_with(platform, &path, "", &layer());
                let empty = resolve_with(platform, &path, "", &with_args(""));
                assert_eq!(none.plan.command, empty.plan.command, "{platform:?} .{ext}");
                assert!(
                    !none.plan.command.contains("${args}"),
                    "{platform:?} .{ext}: {} ({cmd})",
                    none.plan.command
                );
                assert!(
                    !none.plan.command.ends_with(' ') && !none.plan.command.contains("  "),
                    "{platform:?} .{ext}: 空白が残る: {:?}",
                    none.plan.command
                );
            }
        }
    }

    #[test]
    fn コンパイル系は生成物へ引数を渡す() {
        let mac = resolve_with(Platform::MacOs, "/tmp/p/hello.c", "", &with_args("a b"));
        assert_eq!(mac.plan.command, "cc hello.c -o hello && ./hello a b");
        let win = resolve_with(Platform::Windows, "/tmp/p/hello.c", "", &with_args("a b"));
        assert_eq!(
            win.plan.command,
            "gcc hello.c -o hello.exe; if ($?) { .\\hello.exe a b }"
        );
        let kt = resolve_with(Platform::Windows, "/tmp/p/hello.kt", "", &with_args("a"));
        assert_eq!(
            kt.plan.command,
            "kotlinc hello.kt -include-runtime -d hello.jar; if ($?) { java -jar hello.jar a }"
        );
    }

    /// `tako:shell` の宣言なら、引数は内側のシェルのコマンドに入る（包むのは dispatch）
    #[test]
    fn 宣言のシェルでも引数はコマンドの中() {
        let head = "# tako:run: echo hi\n# tako:shell: bash\n";
        let res = resolve_with(Platform::MacOs, "/tmp/p/a.sh", head, &with_args("x"));
        assert_eq!(res.plan.command, "echo hi x");
        assert_eq!(res.plan.shell.as_deref(), Some("bash"));
    }

    #[test]
    fn 作業ディレクトリは宣言より強く相対はファイルの段から() {
        let head = "# tako:run: make\n# tako:cwd: ..\n";
        let res = resolve_with(Platform::MacOs, "/tmp/p/src/a.sh", head, &layer());
        assert_eq!(res.plan.cwd, PathBuf::from("/tmp/p/src/.."));
        assert_eq!(res.declared_cwd.as_deref(), Some(".."));

        let l = ConfigLayer {
            cwd: Some("build".into()),
            ..layer()
        };
        let res = resolve_with(Platform::MacOs, "/tmp/p/src/a.sh", head, &l);
        assert_eq!(res.plan.cwd, PathBuf::from("/tmp/p/src/build"));
        // 宣言の値は出典の材料として残る
        assert_eq!(res.declared_cwd.as_deref(), Some(".."));
        // 絶対・変数（空白入りでも引用されない = パスとしてそのまま使える）
        let l = ConfigLayer {
            cwd: Some("${fileDir}/out".into()),
            ..layer()
        };
        let res = resolve_with(Platform::MacOs, "/tmp/my proj/a.sh", head, &l);
        assert_eq!(res.plan.cwd, PathBuf::from("/tmp/my proj/out"));
        // 全プロファイルに効く
        let multi = "# tako:run: a\n# tako:run[b]: b\n";
        let l = ConfigLayer {
            cwd: Some("/abs".into()),
            ..layer()
        };
        let res = resolve_with(Platform::MacOs, "/tmp/p/a.sh", multi, &l);
        assert!(res.all_profiles.iter().all(|p| p.cwd == Path::new("/abs")));
    }

    #[test]
    fn 実行前コマンドはパスの変数を引用して展開し空白だけなら無い() {
        let l = ConfigLayer {
            before: Some("make -C ${fileDir}".into()),
            ..layer()
        };
        let res = resolve_with(Platform::MacOs, "/tmp/my proj/a.py", "", &l);
        assert_eq!(res.before.as_deref(), Some("make -C '/tmp/my proj'"));
        let l = ConfigLayer {
            before: Some("  ".into()),
            ..layer()
        };
        assert_eq!(
            resolve_with(Platform::MacOs, "/tmp/p/a.py", "", &l).before,
            None
        );
        assert_eq!(
            resolve_with(Platform::MacOs, "/tmp/p/a.py", "", &layer()).before,
            None
        );
    }

    /// 環境変数の値はリテラル（tako の変数だけ引用せず展開。シェルの `$VAR` は残す）
    #[test]
    fn 環境変数の値はtakoの変数だけ引用せずに展開する() {
        let l = ConfigLayer {
            env: BTreeMap::from([
                ("DATA".to_string(), "${fileDir}/data".to_string()),
                ("RAW".to_string(), "$HOME ${python} it's".to_string()),
            ]),
            ..layer()
        };
        let res = resolve_with(Platform::MacOs, "/tmp/my proj/a.py", "", &l);
        assert_eq!(
            res.env,
            vec![
                ("DATA".to_string(), "/tmp/my proj/data".to_string()),
                ("RAW".to_string(), "$HOME ${python} it's".to_string()),
            ]
        );
    }

    /// プロジェクト既定（組み立て済みの完成形）にも引数と作業ディレクトリが効く
    #[test]
    fn プロジェクト既定にも引数と作業ディレクトリが効く() {
        let scratch = crate::test_residue::ScratchDir::new("runner-1732");
        let base = crate::platform::path::canonicalize(scratch.path()).unwrap();
        std::fs::create_dir_all(base.join("c")).unwrap();
        std::fs::write(base.join("c/Makefile"), "run:\n\techo run\n").unwrap();
        let file = base.join("c/Makefile");
        let bounds = SearchBounds::with_ceilings([base.clone()]);
        // 両 OS で絶対パス（Windows の `/abs` はドライブ無し = ルート相対で、`C:/abs` へ解ける）
        let elsewhere = base.join("elsewhere");
        let l = ConfigLayer {
            args: Some("V=1".into()),
            cwd: Some(elsewhere.display().to_string()),
            ..layer()
        };
        let res = resolve_file_in(
            Platform::MacOs,
            &file,
            "",
            &BTreeMap::new(),
            None,
            None,
            Some(&bounds),
            &RuntimeWords::new(),
            &l,
        )
        .unwrap();
        assert_eq!(res.plan.source, RunSource::ProjectDefault);
        assert_eq!(res.plan.command, "make run V=1");
        assert_eq!(res.plan.template, "make run");
        assert_eq!(res.plan.cwd, elsewhere);
        // ルートで走る前提のコマンドなので、移したことを止めずに知らせる
        assert!(
            res.warnings.iter().any(|w| w.contains("--reset cwd")),
            "{:?}",
            res.warnings
        );
        // ルートそのものを指定しただけなら知らせない
        let root = ConfigLayer {
            cwd: Some(base.join("c").display().to_string()),
            ..layer()
        };
        let res = resolve_file_in(
            Platform::MacOs,
            &file,
            "",
            &BTreeMap::new(),
            None,
            None,
            Some(&bounds),
            &RuntimeWords::new(),
            &root,
        )
        .unwrap();
        assert!(res.warnings.is_empty(), "{:?}", res.warnings);
    }

    /// 作業ディレクトリが移れば `${fileBase}` はファイルの絶対パス（移った先からも引ける）。
    /// 移っていなければ今まで通り（ファイル名だけ）
    #[test]
    fn 作業ディレクトリが移ればfile_baseは絶対パス() {
        let moved = ConfigLayer {
            cwd: Some("sub".into()),
            ..layer()
        };
        let res = resolve_with(Platform::MacOs, "/tmp/my p/a.py", "", &moved);
        assert_eq!(res.plan.command, "python3 '/tmp/my p/a.py'");
        assert_eq!(res.plan.cwd, PathBuf::from("/tmp/my p/sub"));
        // コンパイル系: ソースは絶対パス・生成物は移った先（`./hello` も移った先）
        let res = resolve_with(Platform::MacOs, "/tmp/p/hello.c", "", &moved);
        assert_eq!(res.plan.command, "cc /tmp/p/hello.c -o hello && ./hello");
        // 拡張子既定・宣言にはプロジェクト既定の注意を出さない
        assert!(res.warnings.is_empty(), "{:?}", res.warnings);

        let same = ConfigLayer {
            cwd: Some("${fileDir}".into()),
            ..layer()
        };
        let res = resolve_with(Platform::MacOs, "/tmp/p/a.py", "", &same);
        assert_eq!(res.plan.command, "python3 a.py");
        assert_eq!(res.plan.cwd, PathBuf::from("/tmp/p"));
    }
}
