//! tako の外で変わったファイルを言語サーバへ知らせる（`workspace/didChangeWatchedFiles`。#2034）の純粋部分
//!
//! サーバは `client/registerCapability` で「見てほしいファイル」をグロブで登録してくる
//! （rust-analyzer なら根ごとに `**/*.rs` / `**/Cargo.{lock,toml}`）。tako はその根だけを OS の
//! ファイル監視（`tako_control::lsp::watch`）で見て、当たった変化をまとめて通知する。
//! 申告しないと、開いていないファイルの変化を拾えるかは**サーバ自前の監視次第**になる。rust-analyzer 1.95 は
//! 自前の監視へ落ちるが、根がシンボリックリンクを通る（macOS の `/var` → `/private/var` 等）と OS が実パスで
//! 知らせる変化を照合できず、`cargo fmt` / `git checkout` の後も古い行番号の参照を返す（実測: 実パスの根 =
//! 拾う・リンクを通る根 = 拾わない）。tako が見て知らせれば、根の綴りにもサーバにも依らず効く。
//!
//! ここが持つのは I/O の無い部分だけ: 登録の読み取り（[`Registrations`]）・パスとグロブの照合・
//! 監視する根の整理（[`plan_roots`]）・変化の種別（[`ChangeType::classify`]）。OS は引数
//! `windows` で明示する（macOS 上から Windows の形を検査できるように。`goto::path_of_uri` と同じ流儀）。
//!
//! ## グロブの読み方（LSP 3.17 の `GlobPattern`）
//!
//! - `RelativePattern { baseUri, pattern }`: `pattern` を **base からの相対パス**に当てる
//! - 文字列で `/` か `C:/` から始まる: **絶対パス**に当てる（相対パターンを申告しないクライアント向けの形）
//! - それ以外の文字列（`**/*.py` 等）: そのサーバの**ワークスペースの根からの相対パス**に当てる
//!   （根の外のファイルを、別のサーバの根を見ている監視から拾わない）
//!
//! 照合は `globset`（ripgrep のもの）で、`*` / `?` はパスの区切りを越えない（`literal_separator`）。
//! `{a,b}` / `[...]` / `[!...]` / `**` は LSP の定義どおり。Windows は大文字小文字を区別しない。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::platform::path::relative_under;

/// 通知の method（登録の `method` もこれ）
pub const METHOD: &str = "workspace/didChangeWatchedFiles";

/// `WatchKind` の既定（作成 | 変更 | 削除）
const ALL_KINDS: u8 = 7;

/// 監視から外す VCS の内部（VS Code の `files.watcherExclude` の既定と同じ 3 つ）。
/// `git checkout` が大量に書く場所で、サーバが `**/*` を登録していても知らせない
const EXCLUDED: &[&str] = &["/.git/objects/", "/.git/subtree-cache/", "/.hg/store/"];

/// 変化の種別（`FileChangeType`）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChangeType {
    Created,
    Changed,
    Deleted,
}

impl ChangeType {
    /// 通知に載せる番号（1 作成 / 2 変更 / 3 削除）
    pub fn code(self) -> u8 {
        match self {
            Self::Created => 1,
            Self::Changed => 2,
            Self::Deleted => 3,
        }
    }

    /// 登録の `kind`（ビットの組）で見るビット
    fn bit(self) -> u8 {
        match self {
            Self::Created => 1,
            Self::Changed => 2,
            Self::Deleted => 4,
        }
    }

    /// まとめる窓の中で作成を見たか・いま在るかから決める。
    ///
    /// OS のイベントの種別は当てにしない（原子的な保存は rename、`git checkout` は削除 + 作成で届き、
    /// 窓の中で順に畳むと取り違える）。**最後に在るかどうか**で削除を決め、在れば窓の中で作成を
    /// 見たかで作成と変更を分ける（消して作り直した = 在る + 作成 = 作成。サーバはどちらでも読み直す）
    pub fn classify(saw_create: bool, exists: bool) -> Self {
        match (exists, saw_create) {
            (false, _) => Self::Deleted,
            (true, true) => Self::Created,
            (true, false) => Self::Changed,
        }
    }
}

/// 監視する根 1 つ
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct WatchRoot {
    pub path: PathBuf,
    /// 配下まで見るか（`false` は直下のファイルだけ）
    pub recursive: bool,
}

/// 根を監視しない理由（`tako lsp status` の `file_watch.skipped`）
pub const SKIP_BROAD: &str = "broad";

/// 登録の `FileSystemWatcher` 1 つ
#[derive(Debug, Clone)]
struct Watcher {
    /// 照合の基点（`None` = 絶対パスに当てる）
    base: Option<PathBuf>,
    /// 登録された字面（status 用）
    pattern: String,
    matcher: globset::GlobMatcher,
    kind: u8,
    /// 監視する根
    root: WatchRoot,
}

/// 1 サーバの登録（`client/registerCapability` の id ごと）
#[derive(Debug, Clone, Default)]
pub struct Registrations {
    by_id: BTreeMap<String, Vec<Watcher>>,
    /// 読めなかった項目の数（壊れたグロブ・ローカルでない base）
    rejected: usize,
}

impl Registrations {
    /// `client/registerCapability` の params から自分宛て（[`METHOD`]）の登録を取り込む。
    /// 同じ id の登録は置き換える。取り込んだ登録の数を返す（他の method の登録は数えない）
    pub fn register(&mut self, params: &Value, workspace_root: &Path, windows: bool) -> usize {
        let mut taken = 0;
        let registrations = params
            .get("registrations")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for registration in registrations {
            if registration.get("method").and_then(Value::as_str) != Some(METHOD) {
                continue;
            }
            let Some(id) = registration.get("id").and_then(Value::as_str) else {
                continue;
            };
            let watchers = registration
                .pointer("/registerOptions/watchers")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let mut parsed = Vec::with_capacity(watchers.len());
            for watcher in watchers {
                match parse_watcher(watcher, workspace_root, windows) {
                    Some(w) => parsed.push(w),
                    None => self.rejected += 1,
                }
            }
            self.by_id.insert(id.to_string(), parsed);
            taken += 1;
        }
        taken
    }

    /// `client/unregisterCapability`。仕様の綴り（`unregisterations`）と正しい綴りの両方を読む。
    /// 外した登録の数を返す
    pub fn unregister(&mut self, params: &Value) -> usize {
        let items = params
            .get("unregisterations")
            .or_else(|| params.get("unregistrations"))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        items
            .iter()
            .filter(|item| item.get("method").and_then(Value::as_str) == Some(METHOD))
            .filter_map(|item| item.get("id").and_then(Value::as_str))
            .filter(|id| self.by_id.remove(*id).is_some())
            .count()
    }

    /// プロセスが変わった（登録はプロセスのもの = 起こし直したら新しいプロセスが登録し直す）
    pub fn clear(&mut self) {
        self.by_id.clear();
        self.rejected = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.values().all(Vec::is_empty)
    }

    /// 登録の数（id の数）
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// 項目（グロブ）の数
    pub fn watcher_count(&self) -> usize {
        self.by_id.values().map(Vec::len).sum()
    }

    /// 読めなかった項目の数
    pub fn rejected(&self) -> usize {
        self.rejected
    }

    /// 登録された字面（status 用。base つきは `<base>|<pattern>`）
    pub fn patterns(&self) -> Vec<String> {
        self.watchers()
            .map(|w| match &w.base {
                Some(base) => format!("{}|{}", base.display(), w.pattern),
                None => w.pattern.clone(),
            })
            .collect()
    }

    /// この変化を知らせるか（どれか 1 つの項目が種別とパスの両方で当たれば）
    pub fn wants(&self, path: &Path, change: ChangeType, windows: bool) -> bool {
        self.matched_kinds(path, windows) & change.bit() != 0
    }

    /// パスが当たった項目の種別のビットの和（0 = どの項目にも当たらない）。
    /// 種別を決める前（stat の前）に、知らせうるパスだけへ絞るのに使う
    pub fn matched_kinds(&self, path: &Path, windows: bool) -> u8 {
        if excluded(path, windows) {
            return 0;
        }
        let mut absolute: Option<String> = None;
        self.watchers()
            .filter(|w| match &w.base {
                Some(base) => relative_under(base, path)
                    .filter(|rel| !rel.is_empty())
                    .is_some_and(|rel| w.matcher.is_match(rel)),
                None => w
                    .matcher
                    .is_match(absolute.get_or_insert_with(|| slashed(path, windows))),
            })
            .fold(0, |kinds, w| kinds | w.kind)
    }

    /// 監視したい根（項目ごと。整理は [`plan_roots`]）
    pub fn roots(&self) -> Vec<WatchRoot> {
        self.watchers().map(|w| w.root.clone()).collect()
    }

    fn watchers(&self) -> impl Iterator<Item = &Watcher> {
        self.by_id.values().flatten()
    }
}

/// 根の集合を整える: 同じ根は 1 つへ（再帰が勝つ）・再帰で見る根の配下にある根は落とす・
/// **広すぎる根（HOME とその祖先・ファイルシステムの根）は監視しない**。
///
/// 広すぎる根を非再帰にしても費用は減らない（macOS の FSEvents は配下全体のイベントを届け、
/// 直下以外は受け取ってから捨てる）ので、外して理由を返す。HOME 直下のファイルを開いた
/// （印の無い）ときの根がこれに当たる
pub fn plan_roots(
    roots: Vec<WatchRoot>,
    home: Option<&Path>,
) -> (Vec<WatchRoot>, Vec<(PathBuf, &'static str)>) {
    let mut merged: BTreeMap<PathBuf, bool> = BTreeMap::new();
    let mut skipped: Vec<(PathBuf, &'static str)> = Vec::new();
    for root in roots {
        if is_broad(&root.path, home) {
            if !skipped.iter().any(|(p, _)| *p == root.path) {
                skipped.push((root.path, SKIP_BROAD));
            }
            continue;
        }
        *merged.entry(root.path).or_insert(false) |= root.recursive;
    }
    let recursive: Vec<PathBuf> = merged
        .iter()
        .filter(|(_, r)| **r)
        .map(|(p, _)| p.clone())
        .collect();
    let kept = merged
        .into_iter()
        .filter(|(path, _)| {
            !recursive.iter().any(|ancestor| {
                ancestor != path
                    && relative_under(ancestor, path).is_some_and(|rel| !rel.is_empty())
            })
        })
        .map(|(path, recursive)| WatchRoot { path, recursive })
        .collect();
    (kept, skipped)
}

/// HOME とその祖先・ファイルシステムの根か
fn is_broad(path: &Path, home: Option<&Path>) -> bool {
    if path.parent().is_none() {
        return true;
    }
    home.is_some_and(|home| relative_under(path, home).is_some())
}

/// VCS の内部か（[`EXCLUDED`]）
pub fn excluded(path: &Path, windows: bool) -> bool {
    let text = slashed(path, windows);
    EXCLUDED.iter().any(|part| text.contains(part))
}

/// 照合用の字面（Windows は `\` を `/` へ）
fn slashed(path: &Path, windows: bool) -> String {
    let text = path.to_string_lossy();
    if windows {
        text.replace('\\', "/")
    } else {
        text.into_owned()
    }
}

fn parse_watcher(watcher: &Value, workspace_root: &Path, windows: bool) -> Option<Watcher> {
    let kind = watcher
        .get("kind")
        .and_then(Value::as_u64)
        .map(|k| (k & u64::from(ALL_KINDS)) as u8)
        .unwrap_or(ALL_KINDS);
    let (base, pattern) = match watcher.get("globPattern")? {
        Value::String(pattern) if is_absolute_pattern(pattern) => (None, pattern.clone()),
        Value::String(pattern) => (Some(workspace_root.to_path_buf()), pattern.clone()),
        Value::Object(relative) => {
            // baseUri は URI か WorkspaceFolder（`{ uri, name }`）
            let uri = match relative.get("baseUri")? {
                Value::String(uri) => uri.as_str(),
                folder => folder.get("uri")?.as_str()?,
            };
            let base = super::goto::path_of_uri(uri, windows)?;
            (Some(base), relative.get("pattern")?.as_str()?.to_string())
        }
        _ => return None,
    };
    // Windows で `\` を区切りに書いてくるサーバがありうる（LSP のグロブは `/` 区切り）
    let glob_text = if windows {
        pattern.replace('\\', "/")
    } else {
        pattern.clone()
    };
    let matcher = globset::GlobBuilder::new(&glob_text)
        .literal_separator(true)
        .backslash_escape(false)
        .case_insensitive(windows)
        .build()
        .ok()?
        .compile_matcher();
    let root = match &base {
        Some(base) => WatchRoot {
            path: base.clone(),
            recursive: reaches_below(&glob_text),
        },
        None => absolute_root(&glob_text, windows)?,
    };
    Some(Watcher {
        base,
        pattern,
        matcher,
        kind,
        root,
    })
}

/// 絶対パスのパターンか（`/…`・`C:/…`・`C:\…`）
fn is_absolute_pattern(pattern: &str) -> bool {
    let bytes = pattern.as_bytes();
    pattern.starts_with('/')
        || pattern.starts_with('\\')
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'))
}

/// 直下より深いファイルに当たりうるか（`/` か `**` を含む）
fn reaches_below(pattern: &str) -> bool {
    pattern.contains('/') || pattern.contains("**")
}

/// 絶対パスのパターンで監視する根: グロブの記号より前の、最後の `/` までのディレクトリ。
/// 記号が無ければ（1 ファイルを名指し）その親を非再帰で
fn absolute_root(pattern: &str, windows: bool) -> Option<WatchRoot> {
    let meta = pattern.find(['*', '?', '[', '{']).unwrap_or(pattern.len());
    let cut = pattern[..meta].rfind('/')?;
    let dir = if cut == 0 { "/" } else { &pattern[..cut] };
    let rest = &pattern[cut + 1..];
    let separator = if windows { '\\' } else { '/' };
    Some(WatchRoot {
        path: PathBuf::from(crate::file_uri::native_separators_with(dir, separator).into_owned()),
        recursive: reaches_below(rest),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn register(watchers: Value, root: &str, windows: bool) -> Registrations {
        let mut regs = Registrations::default();
        let taken = regs.register(
            &json!({ "registrations": [
                { "id": "w", "method": METHOD, "registerOptions": { "watchers": watchers } },
                { "id": "x", "method": "textDocument/didSave" },
            ]}),
            Path::new(root),
            windows,
        );
        assert_eq!(taken, 1, "自分宛ての登録だけを取り込む");
        regs
    }

    #[test]
    fn 相対パターンは_base_からの相対パスに当てる() {
        let regs = register(
            json!([{ "globPattern": { "baseUri": "file:///w/p", "pattern": "**/*.rs" } }]),
            "/unused",
            false,
        );
        let wants = |p: &str| regs.wants(Path::new(p), ChangeType::Changed, false);
        assert!(wants("/w/p/src/lib.rs"));
        assert!(wants("/w/p/main.rs"), "`**/` は 0 段にも当たる");
        assert!(!wants("/w/other/src/lib.rs"), "base の外");
        assert!(!wants("/w/p/src/lib.rsx"));
        assert!(!wants("/w/p"), "base そのもの");
        assert_eq!(
            regs.roots(),
            vec![WatchRoot {
                path: PathBuf::from("/w/p"),
                recursive: true
            }]
        );
    }

    #[test]
    fn 波括弧と角括弧と否定() {
        let regs = register(
            json!([
                { "globPattern": { "baseUri": "file:///w", "pattern": "**/Cargo.{lock,toml}" } },
                { "globPattern": { "baseUri": "file:///w", "pattern": "gen/[!x]?.py" } },
            ]),
            "/unused",
            false,
        );
        let wants = |p: &str| regs.wants(Path::new(p), ChangeType::Created, false);
        assert!(wants("/w/Cargo.toml"));
        assert!(wants("/w/a/b/Cargo.lock"));
        assert!(!wants("/w/Cargo.json"));
        assert!(wants("/w/gen/ab.py"));
        assert!(!wants("/w/gen/xb.py"), "[!x]");
        assert!(!wants("/w/gen/sub/ab.py"), "`?` と `*` は区切りを越えない");
    }

    #[test]
    fn 文字列のパターンは根からの相対か絶対パス() {
        let regs = register(
            json!([
                { "globPattern": "**/*.{py,pyi}" },
                { "globPattern": "/etc/conf/settings.toml" },
            ]),
            "/w/proj",
            false,
        );
        let wants = |p: &str| regs.wants(Path::new(p), ChangeType::Changed, false);
        assert!(wants("/w/proj/pkg/a.py"));
        assert!(wants("/w/proj/b.pyi"));
        assert!(
            !wants("/w/elsewhere/a.py"),
            "根の外は別のサーバの監視から拾わない"
        );
        assert!(wants("/etc/conf/settings.toml"));
        assert!(!wants("/etc/conf/other.toml"));
        let mut roots = regs.roots();
        roots.sort();
        assert_eq!(
            roots,
            vec![
                WatchRoot {
                    path: PathBuf::from("/etc/conf"),
                    recursive: false
                },
                WatchRoot {
                    path: PathBuf::from("/w/proj"),
                    recursive: true
                },
            ],
            "名指しの 1 ファイルは親を非再帰で"
        );
    }

    #[test]
    fn 絶対パスのグロブはグロブの手前のディレクトリを根にする() {
        let regs = register(
            json!([{ "globPattern": "/w/p/**/*.rs" }, { "globPattern": "/w/q/*.toml" }]),
            "/unused",
            false,
        );
        assert!(regs.wants(Path::new("/w/p/a/b.rs"), ChangeType::Changed, false));
        assert!(regs.wants(Path::new("/w/q/x.toml"), ChangeType::Changed, false));
        assert!(!regs.wants(Path::new("/w/q/s/x.toml"), ChangeType::Changed, false));
        let mut roots = regs.roots();
        roots.sort();
        assert_eq!(
            roots,
            vec![
                WatchRoot {
                    path: PathBuf::from("/w/p"),
                    recursive: true
                },
                WatchRoot {
                    path: PathBuf::from("/w/q"),
                    recursive: false
                },
            ]
        );
    }

    #[test]
    fn kind_のビットで種別を絞る() {
        let regs = register(
            json!([{ "globPattern": { "baseUri": "file:///w", "pattern": "*.rs" }, "kind": 4 }]),
            "/unused",
            false,
        );
        let at = Path::new("/w/a.rs");
        assert!(regs.wants(at, ChangeType::Deleted, false));
        assert!(!regs.wants(at, ChangeType::Changed, false));
        assert!(!regs.wants(at, ChangeType::Created, false));
        assert_eq!(regs.matched_kinds(at, false), 4);
        assert_eq!(regs.matched_kinds(Path::new("/w/a.py"), false), 0);
        assert!(!regs.roots()[0].recursive, "`/` も `**` も無い = 直下だけ");
    }

    #[test]
    fn workspace_folder_形の_base_と_windows_のパス() {
        let regs = register(
            json!([
                { "globPattern": { "baseUri": { "uri": "file:///c%3A/Users/u/p", "name": "p" },
                                   "pattern": "**/*.RS" } },
                { "globPattern": "C:/Users/u/cfg/*.json" },
            ]),
            r"C:\unused",
            true,
        );
        assert!(
            regs.wants(
                Path::new(r"C:\Users\u\p\src\lib.rs"),
                ChangeType::Changed,
                true
            ),
            "大文字小文字を区別しない・区切りは両方"
        );
        assert!(regs.wants(
            Path::new(r"c:\users\u\cfg\a.JSON"),
            ChangeType::Changed,
            true
        ));
        let mut roots = regs.roots();
        roots.sort();
        assert_eq!(roots[0].path, PathBuf::from(r"C:\Users\u\cfg"));
        assert_eq!(roots[1].path, PathBuf::from(r"c:\Users\u\p"));
    }

    #[test]
    fn 読めない項目は数えて捨てる() {
        let regs = register(
            json!([
                { "globPattern": { "baseUri": "https://x/y", "pattern": "*.rs" } },
                { "globPattern": { "baseUri": "file:///w", "pattern": "a[" } },
                { "globPattern": 3 },
                { "globPattern": "**/*.rs" },
            ]),
            "/w",
            false,
        );
        assert_eq!((regs.watcher_count(), regs.rejected()), (1, 3));
    }

    #[test]
    fn 同じ_id_は置き換え_解除は両方の綴りを読む() {
        let mut regs = register(json!([{ "globPattern": "**/*.rs" }]), "/w", false);
        regs.register(
            &json!({ "registrations": [{ "id": "w", "method": METHOD,
                "registerOptions": { "watchers": [{ "globPattern": "**/*.py" }] } }]}),
            Path::new("/w"),
            false,
        );
        assert_eq!(regs.len(), 1);
        assert!(!regs.wants(Path::new("/w/a.rs"), ChangeType::Changed, false));
        assert!(regs.wants(Path::new("/w/a.py"), ChangeType::Changed, false));
        assert_eq!(
            regs.unregister(&json!({ "unregisterations": [{ "id": "other", "method": METHOD }] })),
            0
        );
        assert_eq!(
            regs.unregister(&json!({ "unregisterations": [{ "id": "w", "method": METHOD }] })),
            1
        );
        assert!(regs.is_empty());
        let mut regs = register(json!([{ "globPattern": "**/*.rs" }]), "/w", false);
        assert_eq!(
            regs.unregister(&json!({ "unregistrations": [{ "id": "w", "method": METHOD }] })),
            1
        );
    }

    #[test]
    fn vcs_の内部は知らせない() {
        let regs = register(
            json!([{ "globPattern": { "baseUri": "file:///w", "pattern": "**/*" } }]),
            "/unused",
            false,
        );
        assert!(regs.wants(Path::new("/w/.git/HEAD"), ChangeType::Changed, false));
        assert!(!regs.wants(
            Path::new("/w/.git/objects/ab/cdef"),
            ChangeType::Created,
            false
        ));
        assert!(excluded(Path::new(r"C:\w\.git\objects\ab"), true));
        assert!(!excluded(Path::new(r"C:\w\.git\objects\ab"), false));
    }

    #[test]
    fn 根の整理は重複と配下を畳み_広すぎる根を外す() {
        let root = |p: &str, recursive| WatchRoot {
            path: PathBuf::from(p),
            recursive,
        };
        let (kept, skipped) = plan_roots(
            vec![
                root("/h/u/p", true),
                root("/h/u/p/src", true),
                root("/h/u/p", false),
                root("/h/u/q", false),
                root("/h/u/q/sub", false),
                root("/h/u", true),
                root("/h", false),
                root("/", true),
            ],
            Some(Path::new("/h/u")),
        );
        assert_eq!(
            kept,
            vec![
                root("/h/u/p", true),
                root("/h/u/q", false),
                root("/h/u/q/sub", false),
            ],
            "非再帰の根はその配下を覆わない"
        );
        let skipped: Vec<_> = skipped.iter().map(|(p, _)| p.clone()).collect();
        assert_eq!(
            skipped,
            vec![
                PathBuf::from("/h/u"),
                PathBuf::from("/h"),
                PathBuf::from("/")
            ]
        );
    }

    #[test]
    fn 種別は最後に在るかと作成を見たかで決める() {
        assert_eq!(ChangeType::classify(false, true), ChangeType::Changed);
        assert_eq!(ChangeType::classify(true, true), ChangeType::Created);
        assert_eq!(ChangeType::classify(true, false), ChangeType::Deleted);
        assert_eq!(ChangeType::classify(false, false), ChangeType::Deleted);
        assert_eq!(
            [
                ChangeType::Created.code(),
                ChangeType::Changed.code(),
                ChangeType::Deleted.code()
            ],
            [1, 2, 3]
        );
    }
}
