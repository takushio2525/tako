//! Code Runner のプロジェクト既定（Issue #1656 / FR-3.18）
//!
//! **何のためにあるか**: 解決器はファイル 1 枚しか見ておらず、cargo プロジェクトの中の
//! `.rs` を `rustc main.rs -o main && ./main`（cwd = そのディレクトリ）で走らせていた。
//! 依存も `mod` も解決できないので**必ず失敗する**。`package.json` / `pyproject.toml` /
//! `go.mod` の中のファイルも同じで、プロジェクトの流儀（`npm run` / `python -m` /
//! `go run ./…`）へ寄せる層が無かった。
//!
//! ## 表の形 — 種別の追加は [`KINDS`] への行追加だけ
//!
//! 1 行 = 1 種別で、**印**（上へ辿って探すファイル）・**受け持つ拡張子**・**計画関数**を持つ。
//! 種別ごとの判断（bin の推定、スクリプトの選択）は計画関数の中に閉じ、表の外に
//! `if kind == "cargo"` を散らさない（LSP の検出表 `.agent/plans/2026-09-lsp-s1.md` §3 と
//! 同じ思想）。上へ辿る範囲（HOME を見ない・`.git` で止まる・深さ上限）は
//! [`crate::project_root`] の 1 実装で、LSP のルート検出（#1678）と共有する。
//!
//! ## OS の列
//!
//! 生成するコマンドは**ほとんど OS 差が無い**（`cargo run` / `npm run` / `go run ./cmd/x` /
//! `make` は PowerShell 5.1 でもそのまま通る）。差が出るのは Python の解釈系だけで、
//! それも [`Platform`] を引数で受けて決める（#1616 / #1655 の作法 = 属性の `cfg` を使わず、
//! macOS の単体から Windows の解決結果まで固定できる形）。相対パスは `/` 区切りで渡す
//! （どのツールも Windows で `/` を受け付ける）。

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use crate::i18n::Lang;
use crate::platform::support::Platform;
use crate::project_root::{self, SearchBounds};
use crate::runner::RuntimeWords;
use crate::shell::quote_for_shell;

/// 種別 1 つぶんの行
pub struct ProjectKind {
    /// 安定 ID（`tako run --dry-run` / MCP `tako_run_resolve` の `project.kind` に出る）
    pub id: &'static str,
    /// 上へ辿って探す印。`*` で始まるものは接尾辞（`*.csproj`）
    pub markers: &'static [&'static str],
    /// この種別が受け持つ拡張子（小文字・ドットなし）
    pub exts: &'static [&'static str],
    /// 拡張子ではなくファイル名で受け持つもの（`Makefile` そのもの）
    pub file_names: &'static [&'static str],
    /// 印が見つかったときの計画。`None` = この印では決められない（上の段・次の種別を探す）
    plan: fn(&Probe<'_>) -> Option<Planned>,
    /// 子プロセスに聞いて案内を詰める（Tier P。`RunResolve` の background でだけ走る。#1950）
    pub followup: Option<Followup>,
    /// A/B: この環境変数が `1` なら行を無いものとして扱う（その種別を足す前の解決へ戻す）
    pub legacy_env: Option<&'static str>,
}

/// 案内（[`ProjectNote`]）を子プロセスの答えで詰める問い合わせ（#1950）。
///
/// 起こすのは実行環境の層（#1730）が見つけた実行ファイル（`${<variable>}` の実体）で、
/// 待ちは `probe::output_with_timeout` の 1 実装（呼ぶのは tako-control の `runtime_probe`）。
/// `Run`（UI スレッドから同期で呼ばれる）では起こさない
pub struct Followup {
    /// この ID の案内が出ているときだけ聞く
    pub note: &'static str,
    /// 実行ファイルへ渡す引数
    pub args: &'static [&'static str],
    /// 出力（stdout）で案内を詰め直す。読めない出力なら何も変えない
    pub refine: fn(&mut ProjectNote, &str),
}

impl ProjectKind {
    pub fn claims(&self, ext: &str, file_name: &str) -> bool {
        (!ext.is_empty() && self.exts.contains(&ext)) || self.file_names.contains(&file_name)
    }

    /// A/B（[`Self::legacy_env`]）で外されている
    fn disabled(&self) -> bool {
        self.disabled_with(|k| std::env::var(k).ok())
    }

    fn disabled_with(&self, get: impl Fn(&str) -> Option<String>) -> bool {
        self.legacy_env
            .is_some_and(|k| get(k).is_some_and(|v| v == "1"))
    }
}

/// 計画関数へ渡す材料
pub struct Probe<'a> {
    pub platform: Platform,
    /// 実行対象（絶対パス）
    pub file: &'a Path,
    /// ファイル先頭（`runner` が読んだ 16 KiB。Go の `package` 句を見る）。
    /// **コマンドの選び方にだけ使い、ルートの決定には使わない**
    /// （[`crate::project_root::detect`] は空の先頭で引いて、実行の cwd と同じ答えを返す）
    pub head: &'a str,
    /// 印のあったディレクトリ
    pub marker_dir: &'a Path,
    /// 見つかった印のファイル名
    pub marker: &'a str,
    pub bounds: &'a SearchBounds,
    /// 実行環境の層が決めた `${python}` 等の値（#1730。空 = 層を通っていない）
    pub runtime: &'a RuntimeWords,
}

/// 計画関数の戻り値
struct Planned {
    /// 実行する cwd（= `${workspaceRoot}`）
    root: PathBuf,
    command: String,
    /// 既定（`default`）の他に選べるプロファイル（名前, コマンド）。cwd は `root`
    profiles: Vec<(&'static str, String)>,
    /// 走らせる前に知らせること
    notes: Vec<ProjectNote>,
    /// 決め手になった印の上書き（`None` = 探索で見つかった印）
    marker: Option<PathBuf>,
}

impl Planned {
    fn new(root: PathBuf, command: String) -> Self {
        Self {
            root,
            command,
            profiles: Vec::new(),
            notes: Vec::new(),
            marker: None,
        }
    }
}

/// 走らせる前に知らせること（#1950）。走らせても失敗するのが分かっている形
/// （`arduino-cli` が英語の 1 行で落ちる）を、何をすれば直るかの人の言葉にする。
///
/// 実行ペインではコマンドの前に [`ProjectNote::lines`] を出し、CLI / MCP の応答には
/// `project.notes` として載る
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectNote {
    /// 安定 ID（`board_not_selected` / `sketch_name_mismatch` / `cli_not_found` / `port_not_selected`）
    pub id: &'static str,
    /// 人の言葉（日英は作った時点の `i18n::lang()`）
    pub message: String,
    /// 打てば直るコマンド（最簡形。#322）
    pub commands: Vec<String>,
    /// この名前の実行ファイルが実行ペインのシェルで見つかるなら出さない
    /// （GUI の PATH は Dock 起動だと痩せているので、tako が見つけられなかっただけかもしれない）
    pub unless_found: Option<String>,
    /// このプロファイルにだけ効く（`None` = すべて）
    pub profile: Option<&'static str>,
}

impl ProjectNote {
    pub fn applies_to(&self, profile: &str) -> bool {
        self.profile.is_none_or(|p| p == profile)
    }

    /// 実行ペインに出す行（`[tako] ` + 本文、コマンドは 2 字下げ）
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![format!("[tako] {}", self.message)];
        out.extend(self.commands.iter().map(|c| format!("  {c}")));
        out
    }
}

/// 検出結果（`runner::Resolution::project` に載って CLI / MCP へ出る）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMatch {
    /// [`ProjectKind::id`]
    pub kind: &'static str,
    /// 実行する cwd（cargo の workspace member ならワークスペースのルート）
    pub root: PathBuf,
    /// 決め手になった印（フルパス）
    pub marker: PathBuf,
    /// 組み立て済みのコマンド（変数展開は不要な完成形）
    pub command: String,
    /// 既定の他に選べるプロファイル（名前, コマンド。cwd は `root`）
    pub profiles: Vec<(String, String)>,
    /// 走らせる前に知らせること（[`ProjectNote`]）
    pub notes: Vec<ProjectNote>,
}

impl ProjectMatch {
    /// この種別の行
    pub fn kind_row(&self) -> Option<&'static ProjectKind> {
        KINDS.iter().find(|k| k.id == self.kind)
    }

    /// 子プロセスに聞けば詰められる案内があれば、その問い合わせ
    pub fn followup(&self) -> Option<&'static Followup> {
        let f = self.kind_row()?.followup.as_ref()?;
        self.notes.iter().any(|n| n.id == f.note).then_some(f)
    }

    /// [`Self::followup`] の答えで案内を詰める
    pub fn apply_followup(&mut self, stdout: &str) {
        let Some(f) = self.followup() else {
            return;
        };
        for note in self.notes.iter_mut().filter(|n| n.id == f.note) {
            (f.refine)(note, stdout);
        }
    }
}

/// 種別の表（**近い段の印が勝ち、同じ段なら表の上の行が勝つ**）
pub const KINDS: &[ProjectKind] = &[
    ProjectKind {
        id: "cargo",
        markers: &["Cargo.toml"],
        exts: &["rs"],
        file_names: &[],
        plan: plan_cargo,
        followup: None,
        legacy_env: None,
    },
    ProjectKind {
        id: "go",
        markers: &["go.mod"],
        exts: &["go"],
        file_names: &[],
        plan: plan_go,
        followup: None,
        legacy_env: None,
    },
    ProjectKind {
        id: "npm",
        markers: &["package.json"],
        exts: &["js", "mjs", "cjs", "ts", "mts", "cts", "tsx", "jsx"],
        file_names: &[],
        plan: plan_npm,
        followup: None,
        legacy_env: None,
    },
    ProjectKind {
        id: "python",
        markers: &["pyproject.toml", "setup.py"],
        exts: &["py"],
        file_names: &[],
        plan: plan_python,
        followup: None,
        legacy_env: None,
    },
    ProjectKind {
        id: "dotnet",
        markers: &["*.csproj", "*.fsproj", "*.vbproj"],
        exts: &["cs", "fs", "vb"],
        file_names: &[],
        plan: plan_dotnet,
        followup: None,
        legacy_env: None,
    },
    // C / C++ は複数ファイルをリンクして初めて動くので、単体の `cc a.c` より
    // プロジェクトの Makefile が正しい。`Makefile` 自身も印かつ実行対象になる
    ProjectKind {
        id: "make",
        markers: &MAKEFILES,
        exts: &["c", "cc", "cpp", "cxx", "h", "hh", "hpp", "hxx"],
        file_names: &MAKEFILES,
        plan: plan_make,
        followup: None,
        legacy_env: None,
    },
    // Arduino のスケッチ（#1950）。スケッチ = フォルダで、本体はフォルダと同じ名前の `.ino`。
    // 基板の正本は `sketch.yaml`（Arduino IDE / arduino-cli と同じファイル）。`.ino` 自身が
    // 必ず `*.ino` に当たるので、スケッチのフォルダで計画が走る（名前の不一致も案内できる）
    ProjectKind {
        id: "arduino",
        markers: &[SKETCH_YAML, "*.ino"],
        exts: &["ino"],
        file_names: &[],
        plan: plan_arduino,
        followup: Some(Followup {
            note: NOTE_BOARD,
            args: &["board", "list", "--format", "json"],
            refine: refine_board_note,
        }),
        legacy_env: Some("TAKO_1950_LEGACY"),
    },
];

const MAKEFILES: [&str; 3] = ["GNUmakefile", "Makefile", "makefile"];

/// ファイルの属するプロジェクトを探し、既定の実行コマンドを組み立てる。
///
/// 見つからなければ `None`（呼び出し側は拡張子既定へ落ちる）
pub fn detect(
    platform: Platform,
    file: &Path,
    head: &str,
    bounds: &SearchBounds,
    runtime: &RuntimeWords,
) -> Option<ProjectMatch> {
    let ext = file
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let claiming: Vec<&ProjectKind> = KINDS
        .iter()
        .filter(|k| k.claims(&ext, &name) && !k.disabled())
        .collect();
    if claiming.is_empty() {
        return None;
    }
    let start = file.parent()?;
    for dir in project_root::candidate_dirs(start, bounds) {
        for kind in &claiming {
            let Some(marker) = project_root::marker_in(&dir, kind.markers) else {
                continue;
            };
            let probe = Probe {
                platform,
                file,
                head,
                marker_dir: &dir,
                marker: &marker,
                bounds,
                runtime,
            };
            if let Some(planned) = (kind.plan)(&probe) {
                return Some(ProjectMatch {
                    kind: kind.id,
                    root: planned.root,
                    marker: planned.marker.unwrap_or_else(|| dir.join(&marker)),
                    command: planned.command,
                    profiles: planned
                        .profiles
                        .into_iter()
                        .map(|(name, cmd)| (name.to_string(), cmd))
                        .collect(),
                    notes: planned.notes,
                });
            }
        }
    }
    None
}

// ─── 共通の小道具 ──────────────────────────────────────────────────────

/// `file` の `root` からの相対パスを部品に分ける（`root` の外なら `None`）
fn rel_parts(root: &Path, file: &Path) -> Option<Vec<String>> {
    let rel = file.strip_prefix(root).ok()?;
    rel.components()
        .map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .filter(|parts| !parts.is_empty())
}

fn file_ext(file: &Path) -> String {
    file.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// `dir` 直下の、`suffixes` のどれかで終わるファイル名（名前順）
fn files_with_suffixes(dir: &Path, suffixes: &[&str]) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| suffixes.iter().any(|s| n.len() > s.len() && n.ends_with(s)))
        .collect();
    names.sort();
    names
}

// ─── cargo ─────────────────────────────────────────────────────────────

/// `Cargo.toml` から読む項目（tako-core は toml クレートに依存しないので、必要な
/// キーだけを拾う最小の読み手 [`CargoManifest::parse`] を持つ）
#[derive(Debug, Default, PartialEq, Eq)]
struct CargoManifest {
    /// `[package] name`（無ければ仮想マニフェスト）
    package_name: Option<String>,
    default_run: Option<String>,
    autobins: Option<bool>,
    /// `[workspace]`（`[workspace.package]` 等の子テーブルも含む）がある
    has_workspace: bool,
    workspace_exclude: Vec<String>,
    has_lib_table: bool,
    /// `[[bin]]` の並び
    bins: Vec<BinDecl>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct BinDecl {
    name: Option<String>,
    path: Option<String>,
}

impl CargoManifest {
    fn read(dir: &Path) -> Option<Self> {
        std::fs::read_to_string(dir.join("Cargo.toml"))
            .ok()
            .map(|t| Self::parse(&t))
    }

    fn parse(text: &str) -> Self {
        let mut m = Self::default();
        let mut table = String::new();
        let mut lines = text.lines();
        while let Some(raw) = lines.next() {
            let line = strip_toml_comment(raw);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(h) = line.strip_prefix("[[").and_then(|r| r.strip_suffix("]]")) {
                table = h.trim().to_string();
                if table == "bin" {
                    m.bins.push(BinDecl::default());
                }
                continue;
            }
            if let Some(h) = line.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                table = h.trim().to_string();
                if table == "workspace" || table.starts_with("workspace.") {
                    m.has_workspace = true;
                }
                if table == "lib" {
                    m.has_lib_table = true;
                }
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim().trim_matches('"');
            let mut value = value.trim().to_string();
            // 複数行の配列（`exclude = [` … `]`）は閉じ括弧まで繋ぐ
            if value.starts_with('[') && !value.contains(']') {
                for more in lines.by_ref() {
                    let more = strip_toml_comment(more);
                    value.push(' ');
                    value.push_str(more.trim());
                    if more.contains(']') {
                        break;
                    }
                }
            }
            let full = if table.is_empty() {
                key.to_string()
            } else {
                format!("{table}.{key}")
            };
            if full == "workspace" || full.starts_with("workspace.") {
                m.has_workspace = true;
            }
            match full.as_str() {
                "package.name" => m.package_name = toml_string(&value),
                "package.default-run" => m.default_run = toml_string(&value),
                "package.autobins" => m.autobins = value.parse().ok(),
                "workspace.exclude" => m.workspace_exclude = toml_string_array(&value),
                "bin.name" => {
                    if let Some(b) = m.bins.last_mut() {
                        b.name = toml_string(&value);
                    }
                }
                "bin.path" => {
                    if let Some(b) = m.bins.last_mut() {
                        b.path = toml_string(&value).map(|p| p.replace('\\', "/"));
                    }
                }
                _ if full == "lib" || full.starts_with("lib.") => m.has_lib_table = true,
                _ => {}
            }
        }
        m
    }
}

/// 引用の外にある `#` から後ろを落とす
fn strip_toml_comment(line: &str) -> &str {
    let mut in_basic = false;
    let mut in_literal = false;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if in_basic => escaped = true,
            '"' if !in_literal => in_basic = !in_basic,
            '\'' if !in_basic => in_literal = !in_literal,
            '#' if !in_basic && !in_literal => return &line[..i],
            _ => {}
        }
    }
    line
}

/// 値の先頭の文字列（`"…"` / `'…'`）
fn toml_string(value: &str) -> Option<String> {
    let value = value.trim_start();
    let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let body = &value[1..];
    let mut out = String::new();
    let mut escaped = false;
    for c in body.chars() {
        if escaped {
            out.push(c);
            escaped = false;
        } else if c == '\\' && quote == '"' {
            escaped = true;
        } else if c == quote {
            return Some(out);
        } else {
            out.push(c);
        }
    }
    None
}

/// 文字列の配列（`["a", 'b']`）
fn toml_string_array(value: &str) -> Vec<String> {
    let Some(inner) = value
        .trim()
        .strip_prefix('[')
        .and_then(|r| r.rsplit_once(']'))
        .map(|(inner, _)| inner)
    else {
        return Vec::new();
    };
    inner.split(',').filter_map(toml_string).collect()
}

/// パッケージの属するワークスペースのルート（属さなければパッケージ自身）。
///
/// cargo と同じく、パッケージのマニフェストから上へ辿って**最初に**見つかった
/// `[workspace]` を採る（入れ子のワークスペースは cargo が許さない）。そこで
/// `exclude` されているパッケージは単独のパッケージとして扱う
fn cargo_workspace_root(pkg_dir: &Path, own: &CargoManifest, bounds: &SearchBounds) -> PathBuf {
    if own.has_workspace {
        return pkg_dir.to_path_buf();
    }
    for dir in project_root::candidate_dirs(pkg_dir, bounds)
        .into_iter()
        .skip(1)
    {
        let Some(m) = CargoManifest::read(&dir) else {
            continue;
        };
        if !m.has_workspace {
            continue;
        }
        let rel = rel_parts(&dir, pkg_dir).map(|p| p.join("/"));
        let excluded = rel.is_some_and(|rel| {
            m.workspace_exclude.iter().any(|e| {
                let e = e.trim_end_matches('/');
                rel == e || rel.starts_with(&format!("{e}/"))
            })
        });
        return if excluded { pkg_dir.to_path_buf() } else { dir };
    }
    pkg_dir.to_path_buf()
}

/// ファイルがパッケージのどのターゲットに属するか
#[derive(Debug, PartialEq, Eq)]
enum CargoTarget {
    /// bin（名前が分からなければ `None`）
    Bin(Option<String>),
    Example(String),
    Test(String),
    /// `tests/` の下だがどの結合テストにも属さない共有部品（`tests/common/mod.rs`）
    AllTests,
    Bench(String),
    BuildScript,
    /// `src/` の下のモジュール。値はテスト名の絞り込み（`platform::runner_defaults::`）
    Module(Option<String>),
}

fn stem_rs(name: &str) -> Option<String> {
    name.strip_suffix(".rs")
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn cargo_target(
    pkg_dir: &Path,
    rel: &[String],
    m: &CargoManifest,
    pkg_name: &str,
) -> Option<CargoTarget> {
    let rel_str = rel.join("/");
    if let Some(b) = m.bins.iter().find(|b| b.path.as_deref() == Some(&rel_str)) {
        return Some(CargoTarget::Bin(b.name.clone()));
    }
    let parts: Vec<&str> = rel.iter().map(String::as_str).collect();
    let target = match parts.as_slice() {
        ["build.rs"] => CargoTarget::BuildScript,
        ["src", "main.rs"] => CargoTarget::Bin(main_bin_name(m, pkg_name)),
        ["src", "bin", file] => CargoTarget::Bin(Some(stem_rs(file)?)),
        ["src", "bin", dir, ..] => CargoTarget::Bin(Some((*dir).to_string())),
        ["examples", file] => CargoTarget::Example(stem_rs(file)?),
        ["examples", dir, ..] => CargoTarget::Example((*dir).to_string()),
        ["tests", file] => CargoTarget::Test(stem_rs(file)?),
        ["tests", dir, ..] => {
            if pkg_dir.join("tests").join(dir).join("main.rs").is_file() {
                CargoTarget::Test((*dir).to_string())
            } else {
                CargoTarget::AllTests
            }
        }
        ["benches", file] => CargoTarget::Bench(stem_rs(file)?),
        ["benches", dir, ..] => CargoTarget::Bench((*dir).to_string()),
        ["src", rest @ ..] => CargoTarget::Module(module_filter(rest)),
        // どのターゲットにも属さない `.rs`（`scripts/gen.rs` 等）はプロジェクトとして
        // 走らせる形が無い。上の段・拡張子既定（単体の rustc）へ任せる
        _ => return None,
    };
    Some(target)
}

/// `src/main.rs` の bin 名（`[[bin]]` が `src/main.rs` を名指していればその名前）
fn main_bin_name(m: &CargoManifest, pkg_name: &str) -> Option<String> {
    if let Some(b) = m
        .bins
        .iter()
        .find(|b| b.path.as_deref() == Some("src/main.rs"))
    {
        return b.name.clone();
    }
    (m.autobins != Some(false)).then(|| pkg_name.to_string())
}

/// パッケージの bin 名の集合（自動検出 + `[[bin]]`）
fn bin_names(pkg_dir: &Path, m: &CargoManifest, pkg_name: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let auto = m.autobins != Some(false);
    if pkg_dir.join("src/main.rs").is_file() {
        if let Some(name) = main_bin_name(m, pkg_name) {
            names.insert(name);
        }
    }
    if auto {
        if let Ok(entries) = std::fs::read_dir(pkg_dir.join("src/bin")) {
            for entry in entries.filter_map(Result::ok) {
                let name = entry.file_name().to_string_lossy().into_owned();
                let path = entry.path();
                if path.is_file() {
                    if let Some(stem) = stem_rs(&name) {
                        names.insert(stem);
                    }
                } else if path.join("main.rs").is_file() {
                    names.insert(name);
                }
            }
        }
    }
    names.extend(m.bins.iter().filter_map(|b| b.name.clone()));
    names
}

/// `src/` からの部品をテスト名の絞り込みへ（`["platform", "runner_defaults.rs"]` →
/// `platform::runner_defaults::`）。クレートのルート（`lib.rs`）や識別子にならない名前は `None`
fn module_filter(parts: &[&str]) -> Option<String> {
    let (last, dirs) = parts.split_last()?;
    let stem = last.strip_suffix(".rs")?;
    let mut segs: Vec<&str> = dirs.to_vec();
    let is_root = dirs.is_empty() && (stem == "lib" || stem == "main");
    if stem != "mod" && !is_root {
        segs.push(stem);
    }
    if segs.is_empty() || !segs.iter().all(|s| is_rust_ident(s)) {
        return None;
    }
    Some(format!("{}::", segs.join("::")))
}

fn is_rust_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn plan_cargo(p: &Probe<'_>) -> Option<Planned> {
    let pkg_dir = p.marker_dir;
    let manifest = CargoManifest::read(pkg_dir)?;
    // 仮想マニフェスト（`[workspace]` だけ）の直下にあるファイルはどのパッケージにも属さない
    let pkg_name = manifest.package_name.clone()?;
    let root = cargo_workspace_root(pkg_dir, &manifest, p.bounds);
    let rel = rel_parts(pkg_dir, p.file)?;
    let target = cargo_target(pkg_dir, &rel, &manifest, &pkg_name)?;

    // ワークスペースのルートから走らせるので、member なら `-p` で名指す
    // （ルートのパッケージ自身なら既定のメンバーなので付けない = #322 の最簡形）
    let pkg_flag = if root.as_path() == pkg_dir {
        String::new()
    } else {
        format!(" -p {}", quote_for_shell(&pkg_name))
    };
    let bins = bin_names(pkg_dir, &manifest, &pkg_name);
    // `--bin` は曖昧なときだけ付ける（bin が 1 つ / `default-run` の bin なら不要）
    let bin_flag = |name: &Option<String>| match name {
        Some(n) if bins.len() > 1 && manifest.default_run.as_deref() != Some(n.as_str()) => {
            format!(" --bin {}", quote_for_shell(n))
        }
        _ => String::new(),
    };
    let command = match target {
        CargoTarget::Bin(name) => format!("cargo run{pkg_flag}{}", bin_flag(&name)),
        CargoTarget::Example(n) => format!("cargo run{pkg_flag} --example {}", quote_for_shell(&n)),
        CargoTarget::Test(n) => format!("cargo test{pkg_flag} --test {}", quote_for_shell(&n)),
        CargoTarget::AllTests => format!("cargo test{pkg_flag}"),
        CargoTarget::Bench(n) => format!("cargo bench{pkg_flag} --bench {}", quote_for_shell(&n)),
        CargoTarget::BuildScript => format!("cargo build{pkg_flag}"),
        CargoTarget::Module(filter) => {
            if !bins.is_empty() {
                // bin を持つパッケージのモジュール = そのパッケージを走らせる
                let main = if pkg_dir.join("src/main.rs").is_file() {
                    main_bin_name(&manifest, &pkg_name)
                } else {
                    None
                };
                format!("cargo run{pkg_flag}{}", bin_flag(&main))
            } else if pkg_dir.join("src/lib.rs").is_file() || manifest.has_lib_table {
                // bin の無いパッケージ（ライブラリ）は `cargo run` が必ず
                // 「a bin target must be available」で落ちる。走らせる形は
                // そのモジュールのテストしか無い
                match filter {
                    Some(f) => format!("cargo test{pkg_flag} --lib {}", quote_for_shell(&f)),
                    None => format!("cargo test{pkg_flag} --lib"),
                }
            } else {
                format!("cargo build{pkg_flag}")
            }
        }
    };
    Some(Planned::new(root, command))
}

// ─── go ────────────────────────────────────────────────────────────────

/// ファイル先頭の `package` 句（コメント・ビルドタグを飛ばした最初の句）
fn go_package_clause(head: &str) -> Option<String> {
    let mut in_block = false;
    for (i, raw) in head.lines().enumerate() {
        let mut line = if i == 0 {
            crate::text::strip_bom(raw)
        } else {
            raw
        }
        .trim();
        loop {
            if in_block {
                match line.find("*/") {
                    Some(end) => {
                        line = line[end + 2..].trim_start();
                        in_block = false;
                    }
                    None => {
                        line = "";
                        break;
                    }
                }
            }
            match line.strip_prefix("/*") {
                Some(rest) => {
                    line = rest;
                    in_block = true;
                }
                None => break,
            }
        }
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let rest = line.strip_prefix("package")?;
        if !rest.starts_with(char::is_whitespace) {
            return None;
        }
        let name: String = rest
            .trim_start()
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        return (!name.is_empty()).then_some(name);
    }
    None
}

fn plan_go(p: &Probe<'_>) -> Option<Planned> {
    let root = p.marker_dir;
    let rel = rel_parts(root, p.file)?;
    let (name, dirs) = rel.split_last()?;
    // cwd はモジュールのルートのまま、ファイルのパッケージを `./<dir>` で名指す
    // （`go run .` をモジュールのルートで打つと、`cmd/x/main.go` でもルートの
    // パッケージが走ってしまう）
    let pkg = if dirs.is_empty() {
        ".".to_string()
    } else {
        format!("./{}", dirs.join("/"))
    };
    let is_test = name.ends_with("_test.go");
    let not_main = go_package_clause(p.head).is_some_and(|c| c != "main");
    // main でないパッケージは `go run` が「not a main package」で落ちる
    let verb = if is_test || not_main { "test" } else { "run" };
    Some(Planned::new(
        root.to_path_buf(),
        format!("go {verb} {}", quote_for_shell(&pkg)),
    ))
}

// ─── npm ───────────────────────────────────────────────────────────────

/// パッケージマネージャ（`packageManager` 欄 → 近いロックファイル → npm）
fn node_package_manager(
    pj: &serde_json::Value,
    root: &Path,
    bounds: &SearchBounds,
) -> &'static str {
    const KNOWN: &[&str] = &["npm", "pnpm", "yarn", "bun"];
    if let Some(declared) = pj.get("packageManager").and_then(|v| v.as_str()) {
        let name = declared.split('@').next().unwrap_or_default();
        if let Some(pm) = KNOWN.iter().find(|k| **k == name) {
            return pm;
        }
    }
    // monorepo ではロックファイルがパッケージより上（リポジトリのルート）にある
    const LOCKS: &[(&str, &str)] = &[
        ("pnpm-lock.yaml", "pnpm"),
        ("yarn.lock", "yarn"),
        ("bun.lock", "bun"),
        ("bun.lockb", "bun"),
        ("package-lock.json", "npm"),
    ];
    let names: Vec<&str> = LOCKS.iter().map(|(f, _)| *f).collect();
    project_root::find_upward(root, &names, bounds)
        .and_then(|found| LOCKS.iter().find(|(f, _)| *f == found.marker))
        .map_or("npm", |(_, pm)| pm)
}

/// そのファイルを走らせている npm script の名前（`"start": "node server.js"`）
fn script_running(pj: &serde_json::Value, rel: &str) -> Option<String> {
    let scripts = pj.get("scripts")?.as_object()?;
    let mut hits: Vec<&str> = scripts
        .iter()
        .filter(|(_, v)| v.as_str().is_some_and(|cmd| script_mentions(cmd, rel)))
        .map(|(k, _)| k.as_str())
        .collect();
    for preferred in ["start", "dev", "serve"] {
        if hits.contains(&preferred) {
            return Some(preferred.to_string());
        }
    }
    hits.sort_unstable();
    hits.first().map(|s| (*s).to_string())
}

fn script_mentions(cmd: &str, rel: &str) -> bool {
    cmd.split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')'))
        .map(|t| t.trim_matches(|c| c == '"' || c == '\''))
        .map(|t| t.strip_prefix("./").unwrap_or(t))
        .any(|t| t == rel)
}

fn plan_npm(p: &Probe<'_>) -> Option<Planned> {
    let root = p.marker_dir;
    let text = std::fs::read_to_string(root.join("package.json")).ok()?;
    let pj: serde_json::Value = serde_json::from_str(&text).ok()?;
    let rel = rel_parts(root, p.file)?.join("/");
    let command = if let Some(script) = script_running(&pj, &rel) {
        let pm = node_package_manager(&pj, root, p.bounds);
        format!("{pm} run {}", quote_for_shell(&script))
    } else {
        // script に載っていないファイルは単体で走らせる。cwd はパッケージのルート
        // （npm scripts と同じ = `.env` や設定ファイルを `process.cwd()` から読む流儀）。
        // 解釈系は拡張子既定と同じ選び方（node は TS / JSX を解さない）
        match file_ext(p.file).as_str() {
            "js" | "mjs" | "cjs" => format!("node {}", quote_for_shell(&rel)),
            _ => format!("npx tsx {}", quote_for_shell(&rel)),
        }
    };
    Some(Planned::new(root.to_path_buf(), command))
}

// ─── python ────────────────────────────────────────────────────────────

/// 解釈系。**実行環境の層（#1730）が値を決めていればそれを使う**
/// （`runtime_env` の表と自動選択の順 = `.venv` / uv / poetry / conda / pyenv / システムの 1 実装。
/// `tako:run` の宣言・拡張子既定の `${python}` と同じ答えになる）。
///
/// 層を通っていないとき（`TAKO_1730_LEGACY=1` / 再生ボタンの一覧）は #1656 の推定
/// （[`python_command_1656`]）のまま
fn interpreter(p: &Probe<'_>) -> String {
    runtime_value(p)
        .unwrap_or_else(|| python_command_1656(p.marker_dir, p.platform, &file_ext(p.file)))
}

/// 実行環境の層（#1730）が決めた、このファイルの kind の `${<variable>}` の値
/// （実行ペインのシェルの方言で囲んだもの）。層が決めていなければ `None`
fn runtime_value(p: &Probe<'_>) -> Option<String> {
    let words = crate::runtime_env::kind_for_ext(&file_ext(p.file))
        .and_then(|k| p.runtime.get(k.variable))
        .filter(|w| !w.is_empty())?;
    Some(crate::platform::shell::run_pane_dialect_for(p.platform).command_words(words))
}

/// #1656 の解釈系の推定（`uv.lock` → `uv run python` / `poetry.lock` → `poetry run python` /
/// `.venv` → その中の python / どれも無ければ OS の既定）
fn python_command_1656(root: &Path, platform: Platform, ext: &str) -> String {
    if root.join("uv.lock").is_file() {
        return "uv run python".to_string();
    }
    if root.join("poetry.lock").is_file() {
        return "poetry run python".to_string();
    }
    let (venv, spelled) = match platform {
        Platform::MacOs => (
            root.join(".venv").join("bin").join("python"),
            ".venv/bin/python",
        ),
        Platform::Windows => (
            root.join(".venv").join("Scripts").join("python.exe"),
            // PowerShell は区切りを含む相対パスをそのまま起動できるが、`.\` を付けて
            // 「カレントの下」であることを明示する（#1655 の `.\<名前>.exe` と同じ）
            r".\.venv\Scripts\python.exe",
        ),
    };
    if venv.exists() {
        return spelled.to_string();
    }
    // OS の既定は実行環境の表の `fallback`（拡張子既定の `${python}` が実行環境の無いときに
    // 展開される値）と**同じ 1 マス**から引く（2 か所に書かない）
    crate::runtime_env::kind_for_ext(ext)
        .map(|k| crate::runtime_env::fallback_value(k, platform))
        .unwrap_or("python")
        .to_string()
}

fn is_pytest_file(name: &str) -> bool {
    name.strip_suffix(".py")
        .is_some_and(|stem| stem.starts_with("test_") || stem.ends_with("_test"))
}

fn is_py_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c == '_' || c.is_alphabetic())
        && chars.all(|c| c == '_' || c.is_alphanumeric())
}

/// パッケージの中のファイルならモジュール名（`pkg.sub.mod`）。直下のスクリプト・
/// 識別子にならない名前は `None`（パスで走らせる）
fn python_module(parts: &[String]) -> Option<String> {
    if parts.len() < 2 {
        return None;
    }
    let (last, dirs) = parts.split_last()?;
    let stem = last.strip_suffix(".py")?;
    let mut segs: Vec<&str> = dirs.iter().map(String::as_str).collect();
    segs.push(stem);
    segs.iter().all(|s| is_py_ident(s)).then(|| segs.join("."))
}

fn plan_python(p: &Probe<'_>) -> Option<Planned> {
    let root = p.marker_dir;
    let rel = rel_parts(root, p.file)?;
    let py = interpreter(p);
    let rel_slash = rel.join("/");
    let name = rel.last()?;
    let command = if is_pytest_file(name) {
        format!("{py} -m pytest {}", quote_for_shell(&rel_slash))
    } else {
        // src レイアウト（`src/<pkg>/…`）は `src` の下がモジュールの根
        let module_parts = if rel.len() > 1 && rel[0] == "src" {
            &rel[1..]
        } else {
            &rel[..]
        };
        match python_module(module_parts) {
            // パッケージの中のファイルを単体で走らせると相対 import が
            // 「attempted relative import with no known parent package」で落ちる
            Some(module) => format!("{py} -m {module}"),
            None => format!("{py} {}", quote_for_shell(&rel_slash)),
        }
    };
    Some(Planned::new(root.to_path_buf(), command))
}

// ─── dotnet ────────────────────────────────────────────────────────────

fn plan_dotnet(p: &Probe<'_>) -> Option<Planned> {
    let root = p.marker_dir;
    let lang = match file_ext(p.file).as_str() {
        "cs" => ".csproj",
        "fs" => ".fsproj",
        "vb" => ".vbproj",
        _ => return None,
    };
    let projects = files_with_suffixes(root, &[".csproj", ".fsproj", ".vbproj"]);
    // 1 つなら名指し不要。複数なら言語の合うものが 1 つに決まるときだけ名指す
    let chosen = if projects.len() == 1 {
        None
    } else {
        let matching: Vec<&String> = projects.iter().filter(|n| n.ends_with(lang)).collect();
        match matching.as_slice() {
            [one] => Some((*one).clone()),
            _ => return None,
        }
    };
    let project_file = chosen.clone().or_else(|| projects.first().cloned())?;
    let text = std::fs::read_to_string(root.join(&project_file)).unwrap_or_default();
    let is_test = text.contains("Microsoft.NET.Test.Sdk")
        || text.contains("<IsTestProject>true</IsTestProject>");
    let verb = if is_test { "test" } else { "run" };
    let command = match chosen {
        Some(f) => format!("dotnet {verb} --project {}", quote_for_shell(&f)),
        None => format!("dotnet {verb}"),
    };
    Some(Planned::new(root.to_path_buf(), command))
}

// ─── make ──────────────────────────────────────────────────────────────

/// Makefile に `target:` の規則があるか（変数代入・`.PHONY` 行・レシピ行は数えない）
fn make_has_target(text: &str, target: &str) -> bool {
    text.lines().any(|line| {
        if line.starts_with(['\t', ' ', '#', '.']) {
            return false;
        }
        let Some((targets, after)) = line.split_once(':') else {
            return false;
        };
        if after.starts_with('=') || targets.contains('=') {
            return false;
        }
        targets.split_whitespace().any(|t| t == target)
    })
}

fn plan_make(p: &Probe<'_>) -> Option<Planned> {
    let text = std::fs::read_to_string(p.marker_dir.join(p.marker)).unwrap_or_default();
    // `run` の規則があれば走らせるところまで。無ければ既定のゴール（ビルド）
    let command = if make_has_target(&text, "run") {
        "make run"
    } else {
        "make"
    };
    Some(Planned::new(
        p.marker_dir.to_path_buf(),
        command.to_string(),
    ))
}

// ─── arduino ───────────────────────────────────────────────────────────
//
// 実測（Arduino IDE 2.3.5 同梱の arduino-cli 1.2.0。設計 §5.4）:
//
// | 呼び方 | 結果 |
// |---|---|
// | `sketch.yaml`（`default_fqbn`）のあるフォルダで引数なしの `arduino-cli compile` | exit 0 |
// | 基板なし | `Missing FQBN (Fully Qualified Board Name)` で exit 1 |
// | フォルダ名と `.ino` の名前が違う | `main file missing from sketch: <dir>/<フォルダ名>.ino` |
// | 大文字小文字だけ違う（`blinkc/Blinkc.ino`。大小を区別しない APFS でも） | `missing <dir>/blinkc.ino` |

/// スケッチの設定。**基板とポートの正本はこのファイル**（Arduino IDE / arduino-cli と同じものを
/// 見るので食い違わない。tako の保存先には持たない）。書くのは `arduino-cli board attach`
const SKETCH_YAML: &str = "sketch.yaml";

const NOTE_BOARD: &str = "board_not_selected";
const NOTE_NAME: &str = "sketch_name_mismatch";
const NOTE_CLI: &str = "cli_not_found";
const NOTE_PORT: &str = "port_not_selected";

/// 書き込みのプロファイル（`arduino-cli compile --upload`。ポートは `sketch.yaml` の `default_port`）
const PROFILE_UPLOAD: &str = "upload";

/// 案内のコマンドで基板の名前を入れてもらう場所（[`refine_board_note`] が実の名前に差し替える）
fn board_placeholder(lang: Lang) -> &'static str {
    match lang {
        Lang::Ja => "<基板>",
        Lang::En => "<board>",
    }
}

fn port_placeholder(lang: Lang) -> &'static str {
    match lang {
        Lang::Ja => "<ポート>",
        Lang::En => "<port>",
    }
}

/// `sketch.yaml` の最上段の鍵（tako-core は YAML クレートに依存しないので、使う 3 つだけを
/// 拾う最小の読み手。`profiles:` の下のような字下げした行は読まない）
#[derive(Debug, Default, PartialEq, Eq)]
struct SketchYaml {
    default_fqbn: Option<String>,
    default_port: Option<String>,
    /// `profiles:` の中から既定に選んだもの（基板もポートもそちらが持つ）
    default_profile: Option<String>,
}

impl SketchYaml {
    fn read(dir: &Path) -> Option<Self> {
        std::fs::read_to_string(dir.join(SKETCH_YAML))
            .ok()
            .map(|t| Self::parse(&t))
    }

    fn parse(text: &str) -> Self {
        let mut out = Self::default();
        for line in text.lines() {
            if line.starts_with([' ', '\t', '#', '-']) {
                continue;
            }
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = yaml_scalar(value);
            let slot = match key.trim() {
                "default_fqbn" => &mut out.default_fqbn,
                "default_port" => &mut out.default_port,
                "default_profile" => &mut out.default_profile,
                _ => continue,
            };
            *slot = value;
        }
        out
    }

    fn board_selected(&self) -> bool {
        self.default_fqbn.is_some() || self.default_profile.is_some()
    }

    fn port_selected(&self) -> bool {
        self.default_port.is_some() || self.default_profile.is_some()
    }
}

/// `sketch.yaml` の今の値（#1952。基板とポートの欄・`tako run-config` の読み = `sketch_board::sketch_defaults`）
pub(crate) fn sketch_yaml_defaults(sketch: &Path) -> crate::sketch_board::SketchDefaults {
    let y = SketchYaml::read(sketch).unwrap_or_default();
    crate::sketch_board::SketchDefaults {
        fqbn: y.default_fqbn,
        port: y.default_port,
        profile: y.default_profile,
    }
}

/// A/B（`TAKO_1950_LEGACY=1`）で arduino の行を外しているか（#1952 の基板の欄もそれに従う）
pub(crate) fn arduino_disabled() -> bool {
    KINDS.iter().any(|k| k.id == "arduino" && k.disabled())
}

/// スケッチのフォルダ `sketch` の基板が選ばれているか（`sketch.yaml` の `default_fqbn` か
/// `default_profile`）。Code Runner（#1950）と言語サーバの前提（#1951。`lsp::servers::Requirement`）が
/// 同じ 1 実装を読む。実測（arduino-language-server・同梱の arduino-cli 1.2.0）: どちらの鍵でも
/// 言語サーバは基板を解く（`default_profile` だけでも `build.fqbn` が決まる）
pub fn sketch_board_selected(sketch: &Path) -> bool {
    SketchYaml::read(sketch).is_some_and(|y| y.board_selected())
}

/// 基板が選ばれていないときの案内（文と、選ぶコマンド）。`cli` は打つ arduino-cli
/// （見つけたフルパスか名前。引用と PowerShell の呼び出し演算子は方言の `command_words` = Code Runner の
/// `${arduino_cli}` と同じ 1 実装）。Code Runner の実行ペインの案内と言語サーバの
/// 状態表示（#1951）が同じ文・同じコマンドを出す
pub fn board_not_selected_note(sketch: &Path, cli: &str, platform: Platform) -> (String, String) {
    let lang = crate::i18n::lang();
    let dialect = crate::platform::shell::run_pane_dialect_for(platform);
    (
        msg_board_not_selected(lang),
        board_attach_command(
            &dialect.command_words(&[cli.to_string()]),
            &dialect.quote_arg(&sketch.to_string_lossy()),
            lang,
        ),
    )
}

fn board_attach_command(cli: &str, sketch_arg: &str, lang: Lang) -> String {
    format!(
        "{cli} board attach -b {} {sketch_arg}",
        board_placeholder(lang)
    )
}

/// YAML の 1 行の値（引用符を外し、行末のコメントを落とす）。空なら `None`
fn yaml_scalar(raw: &str) -> Option<String> {
    let v = raw.trim();
    let v = match v.chars().next() {
        Some(q @ ('"' | '\'')) => v[1..].split(q).next().unwrap_or_default(),
        _ => v.split(" #").next().unwrap_or_default().trim(),
    };
    (!v.is_empty()).then(|| v.to_string())
}

/// スケッチの本体（フォルダと同じ名前の `.ino` / 旧形式の `.pde`）。**大文字小文字まで一致**を
/// 求める（arduino-cli は大小を区別しないディスクでも名前の違いを「missing」で落とす = 実測）
fn sketch_main_file(dir: &Path) -> Option<String> {
    let folder = dir.file_name()?.to_str()?;
    let names: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    [".ino", ".pde"]
        .iter()
        .map(|ext| format!("{folder}{ext}"))
        .find(|main| names.contains(main))
}

fn plan_arduino(p: &Probe<'_>) -> Option<Planned> {
    let sketch = p.file.parent()?;
    // スケッチはフォルダ単位。上の段の `.ino` / `sketch.yaml` を自分のスケッチと読まない
    if sketch != p.marker_dir {
        return None;
    }
    let lang = crate::i18n::lang();
    let dialect = crate::platform::shell::run_pane_dialect_for(p.platform);
    // `${arduino_cli}`（PATH → Arduino IDE 同梱 → 名前のまま）。層が見つけていなければ名前のまま
    let found = runtime_value(p);
    let cli = found.clone().unwrap_or_else(|| {
        crate::runtime_env::kind_for_ext(&file_ext(p.file))
            .map(|k| crate::runtime_env::fallback_value(k, p.platform))
            .unwrap_or("arduino-cli")
            .to_string()
    });
    let yaml = SketchYaml::read(sketch).unwrap_or_default();
    let main = sketch_main_file(sketch);
    let sketch_arg = dialect.quote_arg(&sketch.to_string_lossy());

    let mut planned = Planned::new(sketch.to_path_buf(), format!("{cli} compile"));
    planned
        .profiles
        .push((PROFILE_UPLOAD, format!("{cli} compile --upload")));
    planned.marker = Some(if sketch.join(SKETCH_YAML).is_file() {
        sketch.join(SKETCH_YAML)
    } else {
        sketch.join(main.as_deref().unwrap_or_else(|| {
            p.file
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
        }))
    });

    if found.is_none() {
        planned.notes.push(ProjectNote {
            id: NOTE_CLI,
            message: msg_cli_not_found(lang),
            commands: arduino_install_commands(p.platform),
            unless_found: Some(cli.clone()),
            profile: None,
        });
    }
    if main.is_none() {
        let folder = sketch
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stem = p
            .file
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        planned.notes.push(ProjectNote {
            id: NOTE_NAME,
            message: msg_name_mismatch(lang, &folder, &stem),
            commands: Vec::new(),
            unless_found: None,
            profile: None,
        });
    }
    if !yaml.board_selected() {
        planned.notes.push(ProjectNote {
            id: NOTE_BOARD,
            message: msg_board_not_selected(lang),
            commands: vec![board_attach_command(&cli, &sketch_arg, lang)],
            unless_found: None,
            profile: None,
        });
    } else if !yaml.port_selected() {
        planned.notes.push(ProjectNote {
            id: NOTE_PORT,
            message: msg_port_not_selected(lang),
            commands: vec![format!(
                "{cli} board attach -p {} {sketch_arg}",
                port_placeholder(lang)
            )],
            unless_found: None,
            profile: Some(PROFILE_UPLOAD),
        });
    }
    Some(planned)
}

/// arduino-cli を入れるコマンド（#322 の最簡形。言語サーバまで揃う IDE を先に = 設計 §5.5）
fn arduino_install_commands(platform: Platform) -> Vec<String> {
    match platform {
        Platform::MacOs => vec![
            "brew install --cask arduino-ide".to_string(),
            "brew install arduino-cli".to_string(),
        ],
        Platform::Windows => vec![
            "winget install --id ArduinoSA.IDE.stable".to_string(),
            "winget install --id ArduinoSA.CLI".to_string(),
        ],
    }
}

fn msg_cli_not_found(lang: Lang) -> String {
    match lang {
        Lang::Ja => "arduino-cli が見つかりません（PATH と Arduino IDE の同梱を探しました）。\
             Arduino IDE ごと入れる（言語サーバまで揃う）か、arduino-cli だけを入れてください:"
            .to_string(),
        Lang::En => {
            "arduino-cli was not found (searched PATH and the copy bundled with Arduino IDE). \
             Install Arduino IDE (also brings the language server) or arduino-cli alone:"
                .to_string()
        }
    }
}

fn msg_name_mismatch(lang: Lang, folder: &str, stem: &str) -> String {
    match lang {
        Lang::Ja => format!(
            "スケッチのフォルダ名と .ino の名前が違います。arduino-cli はフォルダと同じ名前の \
             {folder}.ino を本体として探します（大文字小文字も区別）。\
             フォルダを {stem} に改名するか、{stem}.ino を {folder}.ino に改名してください"
        ),
        Lang::En => format!(
            "The sketch folder name does not match the .ino name. arduino-cli looks for \
             {folder}.ino (same name as the folder, case-sensitive) as the main file. \
             Rename the folder to {stem}, or rename {stem}.ino to {folder}.ino"
        ),
    }
}

fn msg_board_not_selected(lang: Lang) -> String {
    match lang {
        Lang::Ja => "基板が選ばれていません（スケッチのフォルダの sketch.yaml に default_fqbn がありません）。\
             次のコマンドで選ぶと sketch.yaml に書かれ、Arduino IDE とも共有されます\
             （基板の名前の一覧は board listall・繋いだ基板は board list。例: arduino:avr:uno）:"
            .to_string(),
        Lang::En => "No board is selected (sketch.yaml in the sketch folder has no default_fqbn). \
             Select one with the command below; it is written to sketch.yaml and shared with \
             Arduino IDE (list names with board listall, the connected board with board list; \
             e.g. arduino:avr:uno):"
            .to_string(),
    }
}

fn msg_port_not_selected(lang: Lang) -> String {
    match lang {
        Lang::Ja => {
            "書き込み先のポートが選ばれていません（sketch.yaml に default_port がありません）。\
             繋いだ基板のポートは board list で分かります:"
                .to_string()
        }
        Lang::En => "No upload port is selected (sketch.yaml has no default_port). \
             Find the connected board's port with board list:"
            .to_string(),
    }
}

/// `arduino-cli board list --format json` の答えで基板の案内を詰める（#1950）。
///
/// 繋がっている基板が**ちょうど 1 台**で FQBN が 1 つに決まるときだけ、コマンドの基板の場所を
/// その FQBN（とポート）に差し替える。複数なら候補を本文に並べるだけ（どれかは人が決める）。
/// 形（実測 1.2.0 + 公式の JSON 出力）: `{"detected_ports":[{"matching_boards":[{"name","fqbn"}],
/// "port":{"address", …}}]}`。基板が繋がっていなければ `matching_boards` が無い
fn refine_board_note(note: &mut ProjectNote, stdout: &str) {
    // 読み方は基板の欄（#1952）と同じ 1 実装
    let Some(ports) = crate::sketch_board::parse_board_list(stdout) else {
        return;
    };
    let boards: Vec<(String, String, String)> = ports
        .iter()
        .flat_map(|p| {
            p.boards
                .iter()
                .map(|b| (b.name.clone(), b.fqbn.clone(), p.address.clone()))
        })
        .collect();
    let lang = crate::i18n::lang();
    match boards.as_slice() {
        [] => {}
        [(name, fqbn, address)] => {
            let port = if address.is_empty() {
                String::new()
            } else {
                format!("-p {} ", quote_for_shell(address))
            };
            // 案内を作った時点の言語の綴りで入っている（どちらでも差し替える）
            for c in &mut note.commands {
                for l in [Lang::Ja, Lang::En] {
                    let placeholder = format!("-b {}", board_placeholder(l));
                    *c = c.replace(&placeholder, &format!("{port}-b {fqbn}"));
                }
            }
            note.message.push_str(&match lang {
                Lang::Ja => format!("（繋がっている基板: {name} = {fqbn}）"),
                Lang::En => format!(" (connected board: {name} = {fqbn})"),
            });
        }
        many => {
            let list = many
                .iter()
                .take(4)
                .map(|(name, fqbn, _)| format!("{name} = {fqbn}"))
                .collect::<Vec<_>>()
                .join(" / ");
            note.message.push_str(&match lang {
                Lang::Ja => format!("（繋がっている基板の候補: {list}）"),
                Lang::En => format!(" (connected board candidates: {list})"),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一時 dir の中にプロジェクトを組む箱。天井は箱の根（= HOME 相当）なので、
    /// 実 HOME も箱の外の置き忘れも読まない
    struct Sandbox {
        base: PathBuf,
        bounds: SearchBounds,
    }

    impl Sandbox {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "tako-1656-proj-{tag}-{}-{:?}",
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

        fn dir(&self, rel: &str) -> PathBuf {
            let path = self.base.join(rel);
            std::fs::create_dir_all(&path).unwrap();
            path
        }

        fn detect(&self, platform: Platform, rel: &str) -> Option<ProjectMatch> {
            self.detect_with(platform, rel, &RuntimeWords::new())
        }

        fn detect_with(
            &self,
            platform: Platform,
            rel: &str,
            runtime: &RuntimeWords,
        ) -> Option<ProjectMatch> {
            let file = self.base.join(rel);
            let head = std::fs::read_to_string(&file).unwrap_or_default();
            detect(platform, &file, &head, &self.bounds, runtime)
        }

        /// 両 OS で同じ結果になることを確かめ、(kind, root の相対, command) を返す
        fn same_on_both(&self, rel: &str) -> (&'static str, String, String) {
            let mac = self
                .detect(Platform::MacOs, rel)
                .unwrap_or_else(|| panic!("{rel}: macOS で検出できない"));
            let win = self
                .detect(Platform::Windows, rel)
                .unwrap_or_else(|| panic!("{rel}: Windows で検出できない"));
            assert_eq!(mac, win, "{rel}: OS で結果が違う");
            (mac.kind, self.rel(&mac.root), mac.command)
        }

        fn rel(&self, path: &Path) -> String {
            let rel = path.strip_prefix(&self.base).unwrap();
            rel.components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    const PKG: &str = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n";

    // ─── cargo ───

    #[test]
    fn cargoの単独パッケージはcargo_run() {
        let s = Sandbox::new("cargo-single");
        s.write("demo/Cargo.toml", PKG);
        s.write("demo/src/main.rs", "fn main() {}\n");
        s.write("demo/src/util.rs", "");
        assert_eq!(
            s.same_on_both("demo/src/main.rs"),
            ("cargo", "demo".into(), "cargo run".into())
        );
        // bin を持つパッケージのモジュールもパッケージを走らせる
        assert_eq!(s.same_on_both("demo/src/util.rs").2, "cargo run");
    }

    /// Issue の例そのもの: ワークスペースの member で bin の無い lib crate
    /// （`crates/tako-core/src/runner.rs`）。`cargo run` は必ず落ちるのでテストへ倒す
    #[test]
    fn workspaceのlib_crateはそのモジュールのテスト() {
        let s = Sandbox::new("cargo-lib");
        s.write(
            "repo/Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
        );
        s.dir("repo/.git");
        s.write(
            "repo/crates/tako-core/Cargo.toml",
            "[package]\nname = \"tako-core\"\nversion.workspace = true\n",
        );
        s.write("repo/crates/tako-core/src/lib.rs", "pub mod runner;\n");
        s.write("repo/crates/tako-core/src/runner.rs", "");
        s.write("repo/crates/tako-core/src/platform/mod.rs", "");
        s.write("repo/crates/tako-core/src/platform/runner_defaults.rs", "");
        assert_eq!(
            s.same_on_both("repo/crates/tako-core/src/runner.rs"),
            (
                "cargo",
                "repo".into(),
                "cargo test -p tako-core --lib runner::".into()
            )
        );
        assert_eq!(
            s.same_on_both("repo/crates/tako-core/src/platform/runner_defaults.rs")
                .2,
            "cargo test -p tako-core --lib platform::runner_defaults::"
        );
        assert_eq!(
            s.same_on_both("repo/crates/tako-core/src/platform/mod.rs")
                .2,
            "cargo test -p tako-core --lib platform::"
        );
        assert_eq!(
            s.same_on_both("repo/crates/tako-core/src/lib.rs").2,
            "cargo test -p tako-core --lib"
        );
    }

    #[test]
    fn workspaceのbin_crateはルートから_p付きで走らせる() {
        let s = Sandbox::new("cargo-member-bin");
        s.write("ws/Cargo.toml", "[workspace]\nmembers = [\"crates/cli\"]\n");
        s.write(
            "ws/crates/cli/Cargo.toml",
            "[package]\nname = \"tako-cli\"\n\n[[bin]]\nname = \"tako\"\npath = \"src/main.rs\"\n",
        );
        s.write("ws/crates/cli/src/main.rs", "fn main() {}\n");
        // bin が 1 つなら `--bin` は付けない（#322 の最簡形）
        assert_eq!(
            s.same_on_both("ws/crates/cli/src/main.rs"),
            ("cargo", "ws".into(), "cargo run -p tako-cli".into())
        );
    }

    #[test]
    fn binが複数あるときだけ_binで名指す() {
        let s = Sandbox::new("cargo-bins");
        s.write("p/Cargo.toml", PKG);
        s.write("p/src/main.rs", "");
        s.write("p/src/bin/tool.rs", "");
        s.write("p/src/bin/multi/main.rs", "");
        s.write("p/src/bin/multi/helper.rs", "");
        assert_eq!(s.same_on_both("p/src/main.rs").2, "cargo run --bin demo");
        assert_eq!(
            s.same_on_both("p/src/bin/tool.rs").2,
            "cargo run --bin tool"
        );
        assert_eq!(
            s.same_on_both("p/src/bin/multi/helper.rs").2,
            "cargo run --bin multi"
        );
        // `default-run` の bin は名指さなくても走る
        s.write(
            "p/Cargo.toml",
            "[package]\nname = \"demo\"\ndefault-run = \"demo\"\n",
        );
        assert_eq!(s.same_on_both("p/src/main.rs").2, "cargo run");
        assert_eq!(
            s.same_on_both("p/src/bin/tool.rs").2,
            "cargo run --bin tool"
        );
    }

    #[test]
    fn cargoのターゲット種別() {
        let s = Sandbox::new("cargo-targets");
        s.write("p/Cargo.toml", PKG);
        s.write("p/src/lib.rs", "");
        s.write("p/examples/demo_ex.rs", "");
        s.write("p/tests/it.rs", "");
        s.write("p/tests/big/main.rs", "");
        s.write("p/tests/common/mod.rs", "");
        s.write("p/benches/speed.rs", "");
        s.write("p/build.rs", "");
        s.write("p/scripts/gen.rs", "");
        assert_eq!(
            s.same_on_both("p/examples/demo_ex.rs").2,
            "cargo run --example demo_ex"
        );
        assert_eq!(s.same_on_both("p/tests/it.rs").2, "cargo test --test it");
        assert_eq!(
            s.same_on_both("p/tests/big/main.rs").2,
            "cargo test --test big"
        );
        assert_eq!(s.same_on_both("p/tests/common/mod.rs").2, "cargo test");
        assert_eq!(
            s.same_on_both("p/benches/speed.rs").2,
            "cargo bench --bench speed"
        );
        assert_eq!(s.same_on_both("p/build.rs").2, "cargo build");
        // どのターゲットにも属さない `.rs` はプロジェクトとして走らせない
        assert_eq!(s.detect(Platform::MacOs, "p/scripts/gen.rs"), None);
    }

    #[test]
    fn 仮想マニフェスト直下のファイルはどのパッケージにも属さない() {
        let s = Sandbox::new("cargo-virtual");
        s.write("ws/Cargo.toml", "[workspace]\nmembers = []\n");
        s.write("ws/loose.rs", "fn main() {}\n");
        assert_eq!(s.detect(Platform::MacOs, "ws/loose.rs"), None);
    }

    #[test]
    fn workspaceからexcludeされたパッケージは単独で走らせる() {
        let s = Sandbox::new("cargo-exclude");
        s.write(
            "ws/Cargo.toml",
            "[workspace]\nmembers = [\"app\"]\nexclude = [\n  \"fixtures/standalone\", # 単独で走る\n]\n",
        );
        s.write("ws/fixtures/standalone/Cargo.toml", PKG);
        s.write("ws/fixtures/standalone/src/main.rs", "");
        assert_eq!(
            s.same_on_both("ws/fixtures/standalone/src/main.rs"),
            ("cargo", "ws/fixtures/standalone".into(), "cargo run".into())
        );
    }

    #[test]
    fn cargo_tomlの最小の読み手() {
        let m = CargoManifest::parse(
            "# comment\n[package]\nname = \"a#b\" # 引用の中の # は値\ndefault-run = 'main'\nautobins = false\n\n\
             [workspace.dependencies]\nserde = \"1\"\n\n[[bin]]\nname = \"x\"\npath = 'src\\cli.rs'\n\n[lib]\n",
        );
        assert_eq!(m.package_name.as_deref(), Some("a#b"));
        assert_eq!(m.default_run.as_deref(), Some("main"));
        assert_eq!(m.autobins, Some(false));
        assert!(m.has_workspace, "`[workspace.*]` もワークスペースの宣言");
        assert!(m.has_lib_table);
        assert_eq!(
            m.bins,
            vec![BinDecl {
                name: Some("x".into()),
                path: Some("src/cli.rs".into())
            }]
        );
        // ルートの dotted key もワークスペースの宣言
        assert!(CargoManifest::parse("workspace.members = [\"a\"]\n").has_workspace);
        assert!(!CargoManifest::parse(PKG).has_workspace);
    }

    // ─── go ───

    #[test]
    fn goはファイルのパッケージを名指す() {
        let s = Sandbox::new("go");
        s.write("m/go.mod", "module example.com/m\n\ngo 1.22\n");
        s.write(
            "m/main.go",
            "// Package main.\npackage main\n\nfunc main() {}\n",
        );
        s.write(
            "m/cmd/tool/main.go",
            "//go:build linux || darwin\n\n/* 複数行\n   コメント */\npackage main\n",
        );
        s.write("m/internal/calc/calc.go", "package calc\n");
        s.write("m/internal/calc/calc_test.go", "package calc_test\n");
        s.write("m/weird.go", "");
        assert_eq!(
            s.same_on_both("m/main.go"),
            ("go", "m".into(), "go run .".into())
        );
        assert_eq!(s.same_on_both("m/cmd/tool/main.go").2, "go run ./cmd/tool");
        // main でないパッケージは `go run` が落ちるのでテストへ
        assert_eq!(
            s.same_on_both("m/internal/calc/calc.go").2,
            "go test ./internal/calc"
        );
        assert_eq!(
            s.same_on_both("m/internal/calc/calc_test.go").2,
            "go test ./internal/calc"
        );
        // package 句が読めなければ Issue の既定どおり `go run`
        assert_eq!(s.same_on_both("m/weird.go").2, "go run .");
    }

    // ─── npm ───

    #[test]
    fn npmはファイルを走らせているscriptを選ぶ() {
        let s = Sandbox::new("npm");
        s.write(
            "app/package.json",
            r#"{"name":"app","scripts":{"build":"tsc","dev":"tsx watch src/index.ts","start":"node ./server.js && echo done","serve":"node server.js"}}"#,
        );
        s.write("app/server.js", "");
        s.write("app/src/index.ts", "");
        s.write("app/src/util.js", "");
        s.write("app/src/Card.jsx", "");
        // start / dev / serve の順で好む
        assert_eq!(
            s.same_on_both("app/server.js"),
            ("npm", "app".into(), "npm run start".into())
        );
        assert_eq!(s.same_on_both("app/src/index.ts").2, "npm run dev");
        // script に載っていないファイルはパッケージのルートから単体で走らせる
        assert_eq!(s.same_on_both("app/src/util.js").2, "node src/util.js");
        assert_eq!(s.same_on_both("app/src/Card.jsx").2, "npx tsx src/Card.jsx");
    }

    #[test]
    fn npmのパッケージマネージャはロックファイルとpackage_manager欄で決まる() {
        let s = Sandbox::new("npm-pm");
        // monorepo: ロックファイルはリポジトリのルート
        s.write("mono/pnpm-lock.yaml", "");
        s.write("mono/package.json", r#"{"private":true}"#);
        s.write(
            "mono/packages/web/package.json",
            r#"{"scripts":{"dev":"vite src/main.ts"}}"#,
        );
        s.write("mono/packages/web/src/main.ts", "");
        assert_eq!(
            s.same_on_both("mono/packages/web/src/main.ts"),
            ("npm", "mono/packages/web".into(), "pnpm run dev".into())
        );
        // `packageManager` 欄はロックファイルより強い
        s.write(
            "mono/packages/web/package.json",
            r#"{"packageManager":"yarn@4.1.0","scripts":{"dev":"vite src/main.ts"}}"#,
        );
        assert_eq!(
            s.same_on_both("mono/packages/web/src/main.ts").2,
            "yarn run dev"
        );
    }

    #[test]
    fn 壊れたpackage_jsonは検出しない() {
        let s = Sandbox::new("npm-broken");
        s.write("app/package.json", "{ not json");
        s.write("app/a.js", "");
        assert_eq!(s.detect(Platform::MacOs, "app/a.js"), None);
    }

    // ─── python ───

    #[test]
    fn pythonはパッケージの中ならモジュールとして走らせる() {
        let s = Sandbox::new("py");
        s.write("proj/pyproject.toml", "[project]\nname = \"proj\"\n");
        s.write("proj/pkg/__init__.py", "");
        s.write("proj/pkg/sub/mod.py", "from .. import x\n");
        s.write("proj/main.py", "");
        s.write("proj/tests/test_calc.py", "");
        s.write("proj/my-scripts/run.py", "");
        let cases = [
            (
                "proj/pkg/sub/mod.py",
                "python3 -m pkg.sub.mod",
                "python -m pkg.sub.mod",
            ),
            ("proj/main.py", "python3 main.py", "python main.py"),
            (
                "proj/tests/test_calc.py",
                "python3 -m pytest tests/test_calc.py",
                "python -m pytest tests/test_calc.py",
            ),
            // 識別子にならないディレクトリはモジュールにできないのでパスで
            (
                "proj/my-scripts/run.py",
                "python3 my-scripts/run.py",
                "python my-scripts/run.py",
            ),
        ];
        for (rel, mac, win) in cases {
            let m = s.detect(Platform::MacOs, rel).unwrap();
            let w = s.detect(Platform::Windows, rel).unwrap();
            assert_eq!((m.kind, m.command.as_str()), ("python", mac), "{rel}");
            assert_eq!(w.command, win, "{rel}");
            assert_eq!(s.rel(&m.root), "proj");
        }
    }

    #[test]
    fn pythonの解釈系は環境管理ツールの印で選ぶ() {
        let s = Sandbox::new("py-env");
        s.write("p/pyproject.toml", "");
        s.write("p/src/pkg/app.py", "");
        // src レイアウトは `src` の下がモジュールの根
        assert_eq!(
            s.detect(Platform::MacOs, "p/src/pkg/app.py")
                .unwrap()
                .command,
            "python3 -m pkg.app"
        );
        // .venv（OS ごとに置き場が違う）
        s.write("p/.venv/bin/python", "");
        assert_eq!(
            s.detect(Platform::MacOs, "p/src/pkg/app.py")
                .unwrap()
                .command,
            ".venv/bin/python -m pkg.app"
        );
        assert_eq!(
            s.detect(Platform::Windows, "p/src/pkg/app.py")
                .unwrap()
                .command,
            "python -m pkg.app",
            "Windows の .venv は Scripts\\python.exe"
        );
        s.write("p/.venv/Scripts/python.exe", "");
        assert_eq!(
            s.detect(Platform::Windows, "p/src/pkg/app.py")
                .unwrap()
                .command,
            r".\.venv\Scripts\python.exe -m pkg.app"
        );
        // poetry / uv は .venv より強い（uv が最優先）
        s.write("p/poetry.lock", "");
        assert_eq!(
            s.detect(Platform::Windows, "p/src/pkg/app.py")
                .unwrap()
                .command,
            "poetry run python -m pkg.app"
        );
        s.write("p/uv.lock", "");
        let (_, _, cmd) = s.same_on_both("p/src/pkg/app.py");
        assert_eq!(cmd, "uv run python -m pkg.app");
    }

    /// #1730: 実行環境の層が値を決めていれば、印の推定（#1656）ではなくその値で走らせる
    /// （宣言・拡張子既定の `${python}` と同じ答え）。値は実行ペインのシェルの方言で囲む
    #[test]
    fn pythonは実行環境の層が決めた値を使う() {
        let s = Sandbox::new("py-runtime");
        s.write("p/pyproject.toml", "");
        s.write("p/poetry.lock", "");
        s.write("p/pkg/app.py", "");
        let file = s.base.join("p/pkg/app.py");
        let kind = crate::runtime_env::kind_for_ext("py").unwrap();
        let words = |w: &[&str]| -> RuntimeWords {
            RuntimeWords::from([(
                kind.variable.to_string(),
                w.iter().map(|x| x.to_string()).collect(),
            )])
        };
        let venv = words(&["/my proj/.venv/bin/python"]);
        let m = detect(Platform::MacOs, &file, "", &s.bounds, &venv).unwrap();
        assert_eq!(m.command, "'/my proj/.venv/bin/python' -m pkg.app");
        let win = words(&[r"C:\a b\Scripts\python.exe"]);
        let m = detect(Platform::Windows, &file, "", &s.bounds, &win).unwrap();
        assert_eq!(m.command, r"& 'C:\a b\Scripts\python.exe' -m pkg.app");
        // 層を通っていない（空）なら #1656 の推定のまま
        let m = detect(Platform::MacOs, &file, "", &s.bounds, &RuntimeWords::new()).unwrap();
        assert_eq!(m.command, "poetry run python -m pkg.app");
    }

    // ─── dotnet ───

    #[test]
    fn dotnetはプロジェクトファイルで決まる() {
        let s = Sandbox::new("dotnet");
        s.write(
            "App/App.csproj",
            "<Project Sdk=\"Microsoft.NET.Sdk\"></Project>\n",
        );
        s.write("App/Program.cs", "");
        assert_eq!(
            s.same_on_both("App/Program.cs"),
            ("dotnet", "App".into(), "dotnet run".into())
        );
        // VB は単体ファイル実行が無いが、プロジェクトの中なら走る（#1655 の申し送り）
        s.write("Vb/Vb.vbproj", "<Project/>\n");
        s.write("Vb/Module1.vb", "");
        assert_eq!(s.same_on_both("Vb/Module1.vb").2, "dotnet run");
        // 複数あるなら言語の合うものを名指す
        s.write("Mixed/Core.csproj", "<Project/>\n");
        s.write("Mixed/Tools.fsproj", "<Project/>\n");
        s.write("Mixed/A.cs", "");
        assert_eq!(
            s.same_on_both("Mixed/A.cs").2,
            "dotnet run --project Core.csproj"
        );
        // テストプロジェクトは dotnet test
        s.write(
            "Tests/Tests.csproj",
            "<ItemGroup><PackageReference Include=\"Microsoft.NET.Test.Sdk\" /></ItemGroup>\n",
        );
        s.write("Tests/UnitTest1.cs", "");
        assert_eq!(s.same_on_both("Tests/UnitTest1.cs").2, "dotnet test");
    }

    #[test]
    fn 言語の合うプロジェクトが決まらなければ検出しない() {
        let s = Sandbox::new("dotnet-ambiguous");
        s.write("X/A.csproj", "");
        s.write("X/B.csproj", "");
        s.write("X/a.cs", "");
        assert_eq!(s.detect(Platform::MacOs, "X/a.cs"), None);
    }

    // ─── make ───

    #[test]
    fn makeはrun規則があれば走らせるところまで() {
        let s = Sandbox::new("make");
        s.write(
            "c/Makefile",
            "CC := cc\n.PHONY: all run\nall: app\nrun: app\n\t./app\n",
        );
        s.write("c/src/main.c", "");
        s.write("c/include/app.h", "");
        assert_eq!(
            s.same_on_both("c/src/main.c"),
            ("make", "c".into(), "make run".into())
        );
        assert_eq!(s.same_on_both("c/include/app.h").2, "make run");
        // Makefile 自身も印かつ実行対象
        assert_eq!(s.same_on_both("c/Makefile").2, "make run");
        // `run` の規則が無ければ既定のゴール
        s.write("d/makefile", ".PHONY: run\nrun := 1\nall:\n\techo run: x\n");
        s.write("d/a.cpp", "");
        assert_eq!(s.same_on_both("d/a.cpp").2, "make");
    }

    // ─── 表と境界 ───

    #[test]
    fn 種別は受け持つ拡張子だけに効く() {
        let s = Sandbox::new("claims");
        // Rust の cargo プロジェクトの根に Makefile があっても `.rs` は cargo、
        // `README.md` はどの種別も受け持たない
        s.write("r/Makefile", "run:\n\tcargo run\n");
        s.write("r/Cargo.toml", PKG);
        s.write("r/src/main.rs", "");
        s.write("r/README.md", "");
        assert_eq!(s.same_on_both("r/src/main.rs").0, "cargo");
        assert_eq!(s.detect(Platform::MacOs, "r/README.md"), None);
    }

    #[test]
    fn 近い段の印が勝つ() {
        let s = Sandbox::new("nearest");
        s.write("outer/Makefile", "run:\n");
        s.write("outer/web/package.json", "{}");
        s.write("outer/web/a.js", "");
        s.write("outer/web/b.c", "");
        assert_eq!(s.same_on_both("outer/web/a.js").0, "npm");
        // `.c` は npm が受け持たないので、上の段の Makefile へ届く
        assert_eq!(
            s.same_on_both("outer/web/b.c"),
            ("make", "outer".into(), "make run".into())
        );
    }

    #[test]
    fn 天井直下の印はプロジェクトと読まない() {
        let s = Sandbox::new("ceiling");
        // `~/package.json` の置き忘れ（ホームで `npm i` を打つと出来る）
        s.write("package.json", r#"{"scripts":{"start":"node a.js"}}"#);
        s.write("scratch/a.js", "");
        assert_eq!(s.detect(Platform::MacOs, "scratch/a.js"), None);
    }

    #[test]
    fn リポジトリの外の印はプロジェクトと読まない() {
        let s = Sandbox::new("outside-repo");
        // Makefile の計画は必ず成立するので、境界が効いていなければここで拾ってしまう
        // （外側の印が Cargo.toml だと、計画のほうが不成立で緑になり境界を検査できない）
        s.write("parent/Makefile", "run:\n");
        s.dir("parent/repo/.git");
        s.write("parent/repo/src/main.c", "");
        assert_eq!(s.detect(Platform::MacOs, "parent/repo/src/main.c"), None);
        // 対照: `.git` が無ければ同じ配置で外側の Makefile へ届く
        std::fs::remove_dir_all(s.base.join("parent/repo/.git")).unwrap();
        assert_eq!(s.same_on_both("parent/repo/src/main.c").1, "parent");
    }

    /// 表の行が自己矛盾していない（id の重複・印や拡張子の空・大文字の拡張子）
    #[test]
    fn 表の行が整っている() {
        let mut ids = BTreeSet::new();
        for k in KINDS {
            assert!(ids.insert(k.id), "id '{}' が重複", k.id);
            assert!(!k.markers.is_empty(), "{}: 印が無い", k.id);
            assert!(
                !k.exts.is_empty() || !k.file_names.is_empty(),
                "{}: 何も受け持っていない",
                k.id
            );
            for ext in k.exts {
                assert_eq!(
                    ext.to_ascii_lowercase(),
                    *ext,
                    "{}: 拡張子 '{ext}' が大文字",
                    k.id
                );
            }
        }
        assert_eq!(
            ids.into_iter().collect::<Vec<_>>(),
            vec!["arduino", "cargo", "dotnet", "go", "make", "npm", "python"]
        );
    }

    // ─── arduino（#1950）───

    const BLINK: &str = "void setup() {}\nvoid loop() {}\n";

    /// 既定（compile）の実行に出る案内の ID（書き込みのポートの案内は `upload` にだけ出る）
    fn ids(m: &ProjectMatch) -> Vec<&'static str> {
        m.notes
            .iter()
            .filter(|n| n.applies_to("default"))
            .map(|n| n.id)
            .collect()
    }

    fn note<'a>(m: &'a ProjectMatch, id: &str) -> &'a ProjectNote {
        m.notes
            .iter()
            .find(|n| n.id == id)
            .unwrap_or_else(|| panic!("案内 {id} が無い: {:?}", ids(m)))
    }

    /// `${arduino_cli}` の値（実行環境の層が見つけたもの）を渡す
    fn cli_words(path: &str) -> RuntimeWords {
        [("arduino_cli".to_string(), vec![path.to_string()])].into()
    }

    /// 受け入れ条件 1: `sketch.yaml` のあるスケッチは、両 OS で `arduino-cli compile`（cwd = スケッチ）
    #[test]
    fn arduinoはsketch_yamlのあるスケッチをそのフォルダでcompileする() {
        let s = Sandbox::new("arduino-ok");
        s.write("Blink/Blink.ino", BLINK);
        s.write("Blink/sketch.yaml", "default_fqbn: arduino:avr:uno\n");
        s.write("Blink/helpers.ino", "");
        // スケッチの別タブ（本体でない .ino）からでもスケッチ全体を compile する。
        // 案内の導入コマンドは OS で違うので、種別・cwd・コマンドを OS ごとに見る
        for rel in ["Blink/Blink.ino", "Blink/helpers.ino"] {
            for platform in [Platform::MacOs, Platform::Windows] {
                let m = s
                    .detect(platform, rel)
                    .unwrap_or_else(|| panic!("{rel} / {platform:?}: 検出できない"));
                assert_eq!(
                    (m.kind, s.rel(&m.root), m.command.as_str()),
                    ("arduino", "Blink".to_string(), "arduino-cli compile"),
                    "{rel} / {platform:?}"
                );
            }
        }
        let m = s.detect(Platform::MacOs, "Blink/Blink.ino").unwrap();
        assert_eq!(m.marker, s.base.join("Blink/sketch.yaml"));
        assert_eq!(
            m.profiles,
            vec![(
                "upload".to_string(),
                "arduino-cli compile --upload".to_string()
            )]
        );
        // 実行環境の層を通っていない（語が空）ので、道具の案内だけが条件つきで付く
        assert_eq!(ids(&m), vec!["cli_not_found"]);
        assert_eq!(
            note(&m, "cli_not_found").unless_found.as_deref(),
            Some("arduino-cli")
        );
        // 層が名前のまま（PATH で見つけた）を返せば案内は付かない
        let found = s
            .detect_with(
                Platform::MacOs,
                "Blink/Blink.ino",
                &cli_words("arduino-cli"),
            )
            .unwrap();
        assert!(ids(&found).is_empty(), "{:?}", found.notes);
        assert_eq!(found.command, "arduino-cli compile");
    }

    /// 受け入れ条件 2（コマンドの側）: 層が IDE 同梱のフルパスを決めたら、それを方言の形で使う
    #[test]
    fn arduinoはide同梱のフルパスを実行ペインの方言で囲む() {
        let s = Sandbox::new("arduino-ide");
        s.write("Blink/Blink.ino", BLINK);
        s.write("Blink/sketch.yaml", "default_fqbn: arduino:avr:uno\n");
        let mac = "/Applications/Arduino IDE.app/Contents/Resources/app/lib/backend/resources/arduino-cli";
        let m = s
            .detect_with(Platform::MacOs, "Blink/Blink.ino", &cli_words(mac))
            .unwrap();
        assert_eq!(m.command, format!("'{mac}' compile"));
        assert_eq!(m.profiles[0].1, format!("'{mac}' compile --upload"));
        assert!(ids(&m).is_empty(), "{:?}", m.notes);
        // 案内のコマンドも同じ実体を使う（PATH に無い道具の名前を案内しない）
        assert!(
            note(&m, "port_not_selected").commands[0].starts_with(&format!("'{mac}' board attach")),
            "{:?}",
            m.notes
        );
        let win = r"C:\Users\winuser\AppData\Local\Programs\Arduino IDE\resources\app\lib\backend\resources\arduino-cli.exe";
        let m = s
            .detect_with(Platform::Windows, "Blink/Blink.ino", &cli_words(win))
            .unwrap();
        assert_eq!(m.command, format!("& '{win}' compile"));
    }

    /// 受け入れ条件 3: 基板なし・名前の不一致・道具なしで、それぞれの案内が出る
    #[test]
    fn arduinoの3つの失敗はそれぞれ案内になる() {
        let s = Sandbox::new("arduino-fail");
        // 基板なし（`sketch.yaml` が無い = 授業のスケッチの形）
        s.write("Blink/Blink.ino", BLINK);
        let m = s
            .detect_with(
                Platform::MacOs,
                "Blink/Blink.ino",
                &cli_words("arduino-cli"),
            )
            .unwrap();
        assert_eq!(ids(&m), vec!["board_not_selected"]);
        let board = note(&m, "board_not_selected");
        assert!(board.unless_found.is_none() && board.profile.is_none());
        // スケッチのフォルダを名指す（利用者の端末の cwd はスケッチとは限らない）
        let sketch = quote_for_shell(&s.base.join("Blink").to_string_lossy());
        assert_eq!(board.commands.len(), 1);
        let cmd = &board.commands[0];
        assert!(
            cmd.starts_with("arduino-cli board attach -b <") && cmd.ends_with(&sketch),
            "{cmd}"
        );
        assert!(
            board.lines()[0].starts_with("[tako] "),
            "{:?}",
            board.lines()
        );
        assert!(board.lines()[1].starts_with("  arduino-cli board attach"));
        // `sketch.yaml` はあるが基板の鍵が無い形も同じ
        s.write("Blink/sketch.yaml", "default_port: /dev/cu.usbmodem1101\n");
        let m = s
            .detect_with(
                Platform::MacOs,
                "Blink/Blink.ino",
                &cli_words("arduino-cli"),
            )
            .unwrap();
        assert_eq!(ids(&m), vec!["board_not_selected"]);

        // 名前の不一致（大文字小文字だけの違いも arduino-cli は落とす = 実測）
        s.write("Blink2/Blink.ino", BLINK);
        s.write("Blink2/sketch.yaml", "default_fqbn: arduino:avr:uno\n");
        s.write("blinkc/Blinkc.ino", BLINK);
        s.write("blinkc/sketch.yaml", "default_fqbn: arduino:avr:uno\n");
        for rel in ["Blink2/Blink.ino", "blinkc/Blinkc.ino"] {
            let m = s
                .detect_with(Platform::MacOs, rel, &cli_words("arduino-cli"))
                .unwrap();
            assert_eq!(ids(&m), vec!["sketch_name_mismatch"], "{rel}");
            assert_eq!(m.command, "arduino-cli compile", "{rel}");
        }
        let mismatch = s
            .detect_with(
                Platform::MacOs,
                "Blink2/Blink.ino",
                &cli_words("arduino-cli"),
            )
            .unwrap();
        let msg = &note(&mismatch, "sketch_name_mismatch").message;
        assert!(msg.contains("Blink2.ino") && msg.contains("Blink"), "{msg}");
        // 旧形式の本体（`.pde`）でもスケッチとして成り立つ
        s.write("Old/Old.pde", BLINK);
        s.write("Old/extra.ino", "");
        s.write("Old/sketch.yaml", "default_fqbn: arduino:avr:uno\n");
        let old = s
            .detect_with(Platform::MacOs, "Old/extra.ino", &cli_words("arduino-cli"))
            .unwrap();
        assert!(ids(&old).is_empty(), "{:?}", old.notes);

        // 道具なし: OS ごとの導入コマンド（IDE ごと → CLI だけ）
        let mac = s.detect(Platform::MacOs, "Old/extra.ino").unwrap();
        assert_eq!(
            note(&mac, "cli_not_found").commands,
            vec![
                "brew install --cask arduino-ide",
                "brew install arduino-cli"
            ]
        );
        let win = s.detect(Platform::Windows, "Old/extra.ino").unwrap();
        assert_eq!(
            note(&win, "cli_not_found").commands,
            vec![
                "winget install --id ArduinoSA.IDE.stable",
                "winget install --id ArduinoSA.CLI"
            ]
        );
    }

    #[test]
    fn 書き込みのポートが無ければuploadにだけ案内を付ける() {
        let s = Sandbox::new("arduino-port");
        s.write("Blink/Blink.ino", BLINK);
        s.write("Blink/sketch.yaml", "default_fqbn: arduino:avr:uno\n");
        let m = s
            .detect_with(
                Platform::MacOs,
                "Blink/Blink.ino",
                &cli_words("arduino-cli"),
            )
            .unwrap();
        let port = note(&m, "port_not_selected");
        assert_eq!(port.profile, Some("upload"));
        assert!(port.applies_to("upload") && !port.applies_to("default"));
        assert!(port.commands[0].starts_with("arduino-cli board attach -p <"));
        // ポートまで選んであれば案内は無い
        s.write(
            "Blink/sketch.yaml",
            "default_fqbn: arduino:avr:uno\ndefault_port: /dev/cu.usbmodem1101\n",
        );
        let m = s
            .detect_with(
                Platform::MacOs,
                "Blink/Blink.ino",
                &cli_words("arduino-cli"),
            )
            .unwrap();
        assert!(m.notes.is_empty(), "{:?}", ids(&m));
    }

    /// スケッチはフォルダ単位: 上の段の `sketch.yaml` を自分の基板と読まない
    #[test]
    fn 上の段のsketch_yamlは自分のスケッチと読まない() {
        let s = Sandbox::new("arduino-nested");
        s.write("Outer/Outer.ino", BLINK);
        s.write("Outer/sketch.yaml", "default_fqbn: arduino:avr:uno\n");
        s.write("Outer/inner/inner.ino", BLINK);
        let m = s.detect(Platform::MacOs, "Outer/inner/inner.ino").unwrap();
        assert_eq!(s.rel(&m.root), "Outer/inner");
        assert!(ids(&m).contains(&"board_not_selected"), "{:?}", ids(&m));
    }

    /// #1951: 言語サーバの前提も同じ 1 実装（基板の判定・案内の文・選ぶコマンド）を使う
    #[test]
    fn 基板の判定と案内は言語サーバと同じ1実装() {
        let s = Sandbox::new("arduino-board-1951");
        s.write("Blink/Blink.ino", BLINK);
        let sketch = s.base.join("Blink");
        assert!(!sketch_board_selected(&sketch), "sketch.yaml が無い");
        s.write("Blink/sketch.yaml", "default_port: /dev/cu.usbmodem1101\n");
        assert!(
            !sketch_board_selected(&sketch),
            "ポートだけでは基板にならない"
        );
        s.write("Blink/sketch.yaml", "default_fqbn: arduino:avr:uno\n");
        assert!(sketch_board_selected(&sketch));
        s.write(
            "Blink/sketch.yaml",
            "profiles:\n  uno:\n    fqbn: arduino:avr:uno\ndefault_profile: uno\n",
        );
        assert!(
            sketch_board_selected(&sketch),
            "default_profile でも基板が決まる"
        );
        // 案内の文は Code Runner の実行ペインと同じ。コマンドは名前のままなら引用しない（#322）
        let (message, command) = board_not_selected_note(&sketch, "arduino-cli", Platform::MacOs);
        assert_eq!(message, msg_board_not_selected(crate::i18n::lang()));
        let sketch_arg = crate::shell::quote_for_shell(&sketch.to_string_lossy());
        assert_eq!(
            command,
            format!(
                "arduino-cli board attach -b {} {sketch_arg}",
                board_placeholder(crate::i18n::lang())
            )
        );
        // IDE 同梱のフルパス（空白を含む）は引用し、PowerShell では呼び出し演算子を付ける
        let ide = "/Applications/Arduino IDE.app/Contents/Resources/app/lib/backend/resources/arduino-cli";
        let (_, command) = board_not_selected_note(&sketch, ide, Platform::MacOs);
        assert!(
            command.starts_with(&format!("'{ide}' board attach -b ")),
            "{command}"
        );
        let win = r"C:\Users\winuser\AppData\Local\Programs\Arduino IDE\resources\app\lib\backend\resources\arduino-cli.exe";
        let (_, command) =
            board_not_selected_note(Path::new(r"C:\s\Blink"), win, Platform::Windows);
        assert!(
            command.starts_with(&format!("& '{win}' board attach -b ")),
            "{command}"
        );
    }

    #[test]
    fn sketch_yamlの最上段の鍵だけを読む() {
        let y = SketchYaml::parse(
            "# 基板\ndefault_fqbn: \"arduino:renesas_uno:unor4wifi\" # IDE と同じ\n\
             default_port: '/dev/cu.usbmodem 1'\nprofiles:\n  uno:\n    fqbn: arduino:avr:uno\n\
             default_profile:\n",
        );
        assert_eq!(
            y.default_fqbn.as_deref(),
            Some("arduino:renesas_uno:unor4wifi")
        );
        assert_eq!(y.default_port.as_deref(), Some("/dev/cu.usbmodem 1"));
        // 値の無い鍵・字下げした行（プロファイルの中）は選んだことにしない
        assert_eq!(y.default_profile, None);
        let p = SketchYaml::parse("default_profile: uno\n");
        assert!(p.board_selected() && p.port_selected());
        assert!(!SketchYaml::parse("  default_fqbn: arduino:avr:uno\n").board_selected());
    }

    fn board_note() -> ProjectNote {
        ProjectNote {
            id: "board_not_selected",
            message: "m".to_string(),
            commands: vec!["arduino-cli board attach -b <基板> /s/Blink".to_string()],
            unless_found: None,
            profile: None,
        }
    }

    #[test]
    fn 繋がった基板が1台ならその基板とポートを案内のコマンドへ入れる() {
        let one = r#"{"detected_ports":[
            {"port":{"address":"/dev/cu.debug-console","protocol":"serial"}},
            {"matching_boards":[{"name":"Arduino Uno","fqbn":"arduino:avr:uno"}],
             "port":{"address":"/dev/cu.usbmodem1101","protocol":"serial"}}]}"#;
        let mut n = board_note();
        refine_board_note(&mut n, one);
        assert_eq!(
            n.commands,
            vec!["arduino-cli board attach -p /dev/cu.usbmodem1101 -b arduino:avr:uno /s/Blink"]
        );
        assert!(n.message.contains("Arduino Uno") && n.message.contains("arduino:avr:uno"));
        // 英語の綴りの案内でも差し替わる
        let mut en = board_note();
        en.commands = vec!["arduino-cli board attach -b <board> /s/Blink".to_string()];
        refine_board_note(&mut en, one);
        assert!(
            en.commands[0].contains("-b arduino:avr:uno"),
            "{:?}",
            en.commands
        );
    }

    #[test]
    fn 基板が決まらない答えではコマンドを変えない() {
        // 実測（1.2.0・基板を繋いでいない）: ポートだけで matching_boards が無い
        let none = r#"{"detected_ports":[{"port":{"address":"/dev/cu.Bluetooth-Incoming-Port","label":"x","protocol":"serial","protocol_label":"Serial Port","properties":{}}}]}"#;
        let two = r#"{"detected_ports":[
            {"matching_boards":[{"name":"A","fqbn":"v:a:a"},{"name":"B","fqbn":"v:b:b"}],
             "port":{"address":"COM3"}}]}"#;
        for (stdout, lists) in [
            (none, false),
            (two, true),
            ("not json", false),
            ("{}", false),
        ] {
            let mut n = board_note();
            refine_board_note(&mut n, stdout);
            assert_eq!(n.commands, board_note().commands, "{stdout}");
            assert_eq!(
                n.message.contains("v:a:a / B = v:b:b"),
                lists,
                "{}",
                n.message
            );
        }
    }

    /// 子プロセスに聞くのは、聞いて詰められる案内が出ているときだけ
    #[test]
    fn 追いの問い合わせは基板の案内があるときだけ() {
        let s = Sandbox::new("arduino-followup");
        s.write("Blink/Blink.ino", BLINK);
        let mut m = s
            .detect_with(
                Platform::MacOs,
                "Blink/Blink.ino",
                &cli_words("arduino-cli"),
            )
            .unwrap();
        let f = m.followup().expect("基板の案内がある");
        assert_eq!(f.args, &["board", "list", "--format", "json"]);
        m.apply_followup(
            r#"{"detected_ports":[{"matching_boards":[{"name":"U","fqbn":"arduino:avr:uno"}],"port":{"address":"COM3"}}]}"#,
        );
        assert!(
            note(&m, "board_not_selected").commands[0].contains("-p COM3 -b arduino:avr:uno"),
            "{:?}",
            m.notes
        );
        s.write("Blink/sketch.yaml", "default_fqbn: arduino:avr:uno\n");
        let m = s
            .detect_with(
                Platform::MacOs,
                "Blink/Blink.ino",
                &cli_words("arduino-cli"),
            )
            .unwrap();
        assert!(m.followup().is_none());
        // 他の種別は持たない
        s.write("c/Makefile", "all:\n");
        s.write("c/a.c", "");
        assert!(s
            .detect(Platform::MacOs, "c/a.c")
            .unwrap()
            .followup()
            .is_none());
    }

    /// A/B の口は arduino の行だけが持ち、`1` のときだけ効く（#1950 以前 = 行が無い解決）
    #[test]
    fn arduinoの行だけがab_の口を持つ() {
        for k in KINDS {
            let on = k.disabled_with(|name| (name == "TAKO_1950_LEGACY").then(|| "1".into()));
            assert_eq!(on, k.id == "arduino", "{}", k.id);
            assert!(!k.disabled_with(|_| Some("0".into())), "{}", k.id);
            assert!(!k.disabled_with(|_| None), "{}", k.id);
        }
    }

    #[test]
    fn 案内の文面は日英で揃う() {
        for lang in [Lang::Ja, Lang::En] {
            let msgs = [
                msg_cli_not_found(lang),
                msg_name_mismatch(lang, "Blink2", "Blink"),
                msg_board_not_selected(lang),
                msg_port_not_selected(lang),
            ];
            for m in &msgs {
                assert!(!m.is_empty());
                let ascii = m.is_ascii();
                assert_eq!(ascii, lang == Lang::En, "{lang:?}: {m}");
            }
            assert!(msgs[2].contains("default_fqbn") && msgs[3].contains("default_port"));
        }
    }
}
