//! Code Runner の実行環境（interpreter / toolchain）の検出（Issue #1726 / #1729 S1）
//!
//! 「この `.py` をどの python で走らせるか」を、プロジェクトの中身（`.venv` / `uv.lock` /
//! `environment.yml` / `.python-version` …）から決める。設計: `.agent/plans/2026-09-runner-settings.md` §5
//!
//! ## 言語を足す = 表に行を足すだけ
//!
//! 言語ごとの知識（印のファイル名・置き場・PATH へ足すディレクトリ）は [`kinds::KINDS`] の
//! 1 枚だけが持つ。ここ（`mod.rs`）と [`detect`] は**戦略**（[`Detector`]）の実装だけを持ち、
//! 言語や道具の名前を 1 つも書かない（番犬:
//! `crates/tako-control/tests/issue1729_runtime_env_table_watchdog.rs`）。
//! LSP の検出表（`.agent/plans/2026-09-lsp-s1.md` §3）と同じ規則。
//!
//! ## OS の差は「両方の列を持つ 1 枚の表」（#1655 / #1616 の作法）
//!
//! venv の `bin/python` と `Scripts\python.exe` のような差は [`PerOs`] の 2 列で持ち、
//! [`Platform`] を**引数で受けて**引く。属性の `#[cfg(windows)]` で分けないので、
//! macOS の単体テストから Windows の置き場まで検査できる。
//!
//! ## I/O は差し替えられる・子プロセスは起こさない
//!
//! ファイルシステムは [`FsProbe`] 越しに見る（テストは偽の実装を渡す。
//! `migration::MigrationIo` と同じ作法）。ここで行うのは **stat と小さなファイルの先頭読みだけ**
//! （設計書の Tier F）。`poetry env info -p` のような子プロセスでしか分からないものは
//! [`Candidate::needs_probe`] で「確かめる必要がある」と返すだけで、確かめるのは
//! tako-control の仕事（S2 = #1730。`probe::output_with_timeout` を通す）。
//!
//! ## どこまで上へ辿るかは呼び出し側が決める
//!
//! 検出は「見てよいディレクトリの列（近い順）」を引数で受ける。辿る範囲の 1 実装
//! （天井 = HOME / `.git` で止める / 深さ上限）は `project_root::candidate_dirs`（#1656）が持つ。
//! ここへ写しを置くと、Code Runner の cwd と実行環境で辿る範囲がずれうる。

mod detect;
pub mod kinds;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::platform::support::Platform;
use crate::runner_config::RuntimeRef;

pub use detect::{detect, resolve_ref};
pub use kinds::KINDS;

// ─── 表の型 ────────────────────────────────────────────────────────────

/// OS 別の値（両方の列を 1 行に並べる）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerOs<T> {
    pub macos: T,
    pub windows: T,
}

impl<T: Copy> PerOs<T> {
    /// 両 OS で同じ値
    pub const fn same(value: T) -> Self {
        Self {
            macos: value,
            windows: value,
        }
    }

    pub fn get(&self, platform: Platform) -> T {
        match platform {
            Platform::MacOs => self.macos,
            Platform::Windows => self.windows,
        }
    }
}

/// パスの成分の列（`["Scripts", "python.exe"]`）。区切り文字を書かずに成分で持つので、
/// どちらの OS の区切りでも同じ行を使える。空の列は「基準のディレクトリそのもの」
pub type Components = &'static [&'static str];

/// 既知のディレクトリ（道具の置き場・一覧ファイルの置き場）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownDir {
    /// ホームからの相対
    Home(Components),
    /// 環境変数の値からの相対（`%LOCALAPPDATA%` 等）。変数が無ければ解けない
    Var(&'static str, Components),
    /// 絶対パス
    Abs(&'static str),
}

impl KnownDir {
    pub fn resolve(&self, env: &DetectEnv) -> Option<PathBuf> {
        match self {
            Self::Home(c) => env.home.as_deref().map(|h| join(h, c)),
            Self::Var(var, c) => env.var(var).map(|v| join(Path::new(v), c)),
            Self::Abs(p) => Some(PathBuf::from(p)),
        }
    }
}

/// 環境変数へ入れる値の出どころ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvValue {
    /// 環境の置き場（venv のディレクトリ / conda の prefix）
    Location,
    /// 環境の名前（conda の env 名）
    Name,
}

/// 環境の中の置き場（interpreter と PATH の先頭へ足すディレクトリ）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvLayout {
    /// 環境の置き場からの interpreter の相対
    pub interpreter: PerOs<Components>,
    /// PATH の先頭へ足すディレクトリ（並びがそのまま PATH の順）
    pub path_dirs: PerOs<&'static [Components]>,
    /// 実行ペインへ渡す環境変数
    pub env: &'static [(&'static str, EnvValue)],
}

/// 版の読み方
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionSource {
    /// 読まない
    None,
    /// 環境の中の `key = value` 形式のファイル（`keys` の並びで最初に見つかったもの）
    KeyValue {
        file: &'static str,
        keys: &'static [&'static str],
    },
    /// ディレクトリの中の `<prefix><版>-…` という名前のファイル
    FileNamePrefix {
        dir: Components,
        prefix: &'static str,
    },
    /// ディレクトリの名前そのもの
    DirName,
}

/// 「このディレクトリはプロジェクトだ」の印
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    /// そのファイルが在る
    File(&'static str),
    /// TOML ファイルに、その表（`[tool.x]` / `[tool.x.y]`）が在る
    TomlTable {
        file: &'static str,
        table: &'static str,
    },
}

/// プロジェクトの中の環境（包む形の道具が作る `.venv` 等）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InProjectEnv {
    pub names: &'static [&'static str],
    pub marker: &'static str,
    pub layout: EnvLayout,
}

/// 子プロセスでしか分からない情報の問い合わせ（Tier P。実行は S2 = tako-control）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeCmd {
    pub args: &'static [&'static str],
}

/// 印を見つけたらツールで包む（`<道具> run python …`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WrapperSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// どれか 1 つで成立
    pub markers: &'static [Marker],
    /// 道具の実行ファイル名（Tier F で既知の置き場と PATH から探す）
    pub program: PerOs<&'static str>,
    /// 包むときに道具へ渡す語（`["run"]`）
    pub run_args: &'static [&'static str],
    pub program_dirs: &'static [KnownDir],
    /// 印のある段に環境があれば、包むのと同時に activation もする
    pub in_project_env: Option<InProjectEnv>,
    /// 環境の置き場を聞く問い合わせ（道具を起こす = Tier P）
    pub env_dir_probe: Option<ProbeCmd>,
}

/// 印のファイルを持つディレクトリが環境（venv = `pyvenv.cfg`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkerDirSpec {
    pub id: &'static str,
    /// 各段で先に見る名前（並びが優先順）
    pub names: &'static [&'static str],
    /// `names` に無い子ディレクトリも印で拾うか（名前順。1 段あたり [`SCAN_CHILDREN_MAX`] まで）
    pub scan_children: bool,
    pub marker: &'static str,
    pub layout: EnvLayout,
    pub version: VersionSource,
}

/// 一覧ファイル（道具が作る env の一覧）とプロジェクトの宣言の突き合わせ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NamedEnvListSpec {
    pub id: &'static str,
    pub label_prefix: &'static str,
    /// 1 行に 1 つ、env の置き場（prefix）を並べたファイル
    pub list_file: KnownDir,
    /// env の名前を宣言するプロジェクトのファイル（先頭から探す）
    pub project_files: &'static [&'static str],
    /// 宣言の中の名前のキー（`name: ml`）
    pub name_key: &'static str,
    pub layout: EnvLayout,
    pub version: VersionSource,
}

/// 版を書いたファイル → 道具の置き場の `versions/<版>`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionFileSpec {
    pub id: &'static str,
    pub label_prefix: &'static str,
    pub files: &'static [&'static str],
    /// この値なら「道具を使わない」の意味なので飛ばす
    pub skip_values: &'static [&'static str],
    /// 道具の置き場を指す環境変数（先頭から）
    pub root_env: &'static [&'static str],
    pub root_default: PerOs<KnownDir>,
    pub versions_dir: Components,
    pub layout: EnvLayout,
}

/// PATH 上の名前（最後の砦）。**解くのは実行ペインのシェル**で、ここでは表示用に
/// GUI プロセスの PATH を覗くだけ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathLookupSpec {
    pub id: &'static str,
    /// PATH の各ディレクトリで探すファイル名
    pub file_names: PerOs<&'static [&'static str]>,
    /// 実体ではない置き場（Windows の App Execution Alias = ストアを開くだけ）
    pub exclude_dirs: PerOs<&'static [KnownDir]>,
}

/// 環境変数で切り替える toolchain（ファイルの値をそのまま変数へ入れる）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvSwitchSpec {
    pub id: &'static str,
    pub label_prefix: &'static str,
    pub files: &'static [&'static str],
    /// TOML のキー（`channel = "…"`）。キーが無い素のファイル（`1.79` の 1 行）は 1 行目を使う
    pub toml_key: Option<&'static str>,
    pub var: &'static str,
}

/// 検出の戦略。**新しい言語は既存の戦略の組み合わせで書ける**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detector {
    Wrapper(WrapperSpec),
    MarkerDir(MarkerDirSpec),
    NamedEnvList(NamedEnvListSpec),
    VersionFile(VersionFileSpec),
    PathLookup(PathLookupSpec),
    EnvSwitch(EnvSwitchSpec),
}

impl Detector {
    pub fn id(&self) -> &'static str {
        match self {
            Self::Wrapper(s) => s.id,
            Self::MarkerDir(s) => s.id,
            Self::NamedEnvList(s) => s.id,
            Self::VersionFile(s) => s.id,
            Self::PathLookup(s) => s.id,
            Self::EnvSwitch(s) => s.id,
        }
    }
}

/// 実行環境の種類（言語 1 つぶんの行）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeKind {
    pub id: &'static str,
    /// この kind で走らせる拡張子（小文字・ドットなし）
    pub extensions: &'static [&'static str],
    /// コマンドテンプレートの変数名（`${<variable>}`）
    pub variable: &'static str,
    /// 実行環境が見つからないときの変数の値（既存の組み込み既定表と同じ名前）
    pub fallback: PerOs<&'static str>,
    /// 包む形のときに道具の後ろへ置くプログラム名（`<道具> run <これ>`）
    pub wrapped_program: &'static str,
    /// **自動選択の順**に並べた検出器
    pub detectors: &'static [Detector],
}

impl RuntimeKind {
    /// 保存できる `manager` の値（表の検出器 ID + [`RuntimeRef::EXPLICIT_PATH`]）。
    /// MCP の enum はここから作る（#1467 の「正本から生成」）
    pub fn manager_ids(&self) -> Vec<&'static str> {
        let mut ids: Vec<&'static str> = self.detectors.iter().map(Detector::id).collect();
        ids.push(RuntimeRef::EXPLICIT_PATH);
        ids
    }

    fn detector(&self, id: &str) -> Option<&'static Detector> {
        self.detectors.iter().find(|d| d.id() == id)
    }
}

/// 拡張子から kind を引く（大文字小文字は問わない）
pub fn kind_for_ext(ext: &str) -> Option<&'static RuntimeKind> {
    let ext = ext.to_ascii_lowercase();
    KINDS.iter().find(|k| k.extensions.contains(&ext.as_str()))
}

/// ID から kind を引く
pub fn kind_by_id(id: &str) -> Option<&'static RuntimeKind> {
    KINDS.iter().find(|k| k.id == id)
}

// ─── I/O の差し替え口 ──────────────────────────────────────────────────

/// 検出が見るファイルシステム（stat と先頭読みだけ）
pub trait FsProbe {
    fn is_file(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;
    /// 先頭 `max` バイトを UTF-8（壊れたバイトは置換）で読む。読めなければ `None`
    fn read_head(&self, path: &Path, max: usize) -> Option<String>;
    /// 直下の名前（並びは呼び出し側で決める）。読めなければ空
    fn list_dir(&self, path: &Path) -> Vec<String>;
}

/// 本物のファイルシステム
#[derive(Debug, Clone, Copy, Default)]
pub struct RealFs;

impl FsProbe for RealFs {
    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn read_head(&self, path: &Path, max: usize) -> Option<String> {
        use std::io::Read;
        let file = std::fs::File::open(path).ok()?;
        let mut buf = Vec::new();
        file.take(max as u64).read_to_end(&mut buf).ok()?;
        Some(String::from_utf8_lossy(&buf).into_owned())
    }

    fn list_dir(&self, path: &Path) -> Vec<String> {
        std::fs::read_dir(path)
            .map(|it| {
                it.filter_map(Result::ok)
                    .filter_map(|e| e.file_name().into_string().ok())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// 検出が読む環境（プロセスの環境変数を直接読まない = テストで差し替えられる）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DetectEnv {
    pub home: Option<PathBuf>,
    /// 表が名指す環境変数だけ（[`DetectEnv::from_process`] が集める）
    pub vars: BTreeMap<String, String>,
    /// GUI プロセスの PATH（Dock 起動だと痩せている。表示用に覗くだけで、解くのはシェル）
    pub path_dirs: Vec<PathBuf>,
}

impl DetectEnv {
    fn var(&self, name: &str) -> Option<&str> {
        self.vars
            .get(name)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    /// 実プロセスの環境から作る。読む環境変数の名前は**表から集める**
    /// （表に行を足せば、ここを直さずに読み先が増える）
    pub fn from_process() -> Self {
        let mut vars = BTreeMap::new();
        for name in table_env_names() {
            if let Ok(v) = std::env::var(name) {
                vars.insert(name.to_string(), v);
            }
        }
        Self {
            home: crate::paths::home_dir(),
            vars,
            path_dirs: std::env::var_os("PATH")
                .map(|p| std::env::split_paths(&p).collect())
                .unwrap_or_default(),
        }
    }
}

/// 表が名指す環境変数の名前（重複なし）
pub fn table_env_names() -> Vec<&'static str> {
    fn known(d: &KnownDir, out: &mut Vec<&'static str>) {
        if let KnownDir::Var(v, _) = d {
            out.push(v);
        }
    }
    let mut out = Vec::new();
    for kind in KINDS {
        for det in kind.detectors {
            match det {
                Detector::Wrapper(s) => s.program_dirs.iter().for_each(|d| known(d, &mut out)),
                Detector::NamedEnvList(s) => known(&s.list_file, &mut out),
                Detector::VersionFile(s) => {
                    out.extend(s.root_env.iter().copied());
                    known(&s.root_default.macos, &mut out);
                    known(&s.root_default.windows, &mut out);
                }
                Detector::PathLookup(s) => {
                    for d in s.exclude_dirs.macos.iter().chain(s.exclude_dirs.windows) {
                        known(d, &mut out);
                    }
                }
                Detector::MarkerDir(_) | Detector::EnvSwitch(_) => {}
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

// ─── 検出の結果 ────────────────────────────────────────────────────────

/// 実行環境の当て方（S2 がコマンドへ組み込む。引用はシェルの方言を知る境界 B1 の仕事）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// `${<variable>}` に入る語の列（`[<interpreter>]` / `[<道具>, "run", <wrapped>]` / `[<名前>]`）
    pub program: Vec<String>,
    /// PATH の先頭へ足すディレクトリ（並びがそのまま PATH の順）
    pub path_prepend: Vec<PathBuf>,
    /// 実行ペインへ渡す環境変数
    pub env: Vec<(String, String)>,
}

/// 実行環境の候補 1 つ
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub kind: &'static str,
    /// 検出器 ID（= 保存するときの [`RuntimeRef::manager`]）
    pub manager: &'static str,
    /// 保存するときの [`RuntimeRef::key`]
    pub key: String,
    /// UI に出す短い名前
    pub label: String,
    pub version: Option<String>,
    /// 環境の置き場（venv のディレクトリ / conda の prefix / 版のディレクトリ / 見つけた実行ファイル）
    pub location: Option<PathBuf>,
    /// 印を見つけたディレクトリ（プロジェクトの段）
    pub found_in: Option<PathBuf>,
    /// 何も選んでいないときに自動で選んでよいか
    pub auto_eligible: bool,
    /// Tier P（子プロセス）で確かめないと自動で選べない / 置き場が決まらない
    pub needs_probe: bool,
    pub applied: Applied,
}

impl Candidate {
    /// 保存する参照
    pub fn reference(&self) -> RuntimeRef {
        RuntimeRef::new(self.manager, self.key.clone())
    }
}

/// 検出の結果
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Detection {
    /// 自動選択の順に並べた全候補（ドロップダウン / `tako_run_resolve` の `runtimes`）
    pub candidates: Vec<Candidate>,
    /// 自動で選んだ候補（`candidates` の添字）。`None` = 既定（`fallback` の名前のまま）
    pub auto: Option<usize>,
    pub warnings: Vec<String>,
}

impl Detection {
    pub fn auto_candidate(&self) -> Option<&Candidate> {
        self.auto.and_then(|i| self.candidates.get(i))
    }
}

/// 保存した参照が解けなかった（**既定へ落とさない** = #1466。呼び出し側はエラーにする）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    pub manager: String,
    pub key: String,
    pub reason: String,
}

/// 実行環境が見つからないときの `${<variable>}` の値
pub fn fallback_value(kind: &RuntimeKind, platform: Platform) -> &'static str {
    kind.fallback.get(platform)
}

/// 1 段あたりに `scan_children` で見る子の上限（巨大なディレクトリで stat が膨らまないように）
pub const SCAN_CHILDREN_MAX: usize = 256;

/// 成分の列を足す
fn join(base: &Path, components: &[&str]) -> PathBuf {
    components.iter().fold(base.to_path_buf(), |p, c| p.join(c))
}

#[cfg(test)]
mod fake_fs;
#[cfg(test)]
mod tests;
