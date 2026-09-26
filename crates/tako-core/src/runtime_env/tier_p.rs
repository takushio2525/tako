//! Tier P（子プロセスへの問い合わせ）の**段取り**（純粋部分。Issue #1730）
//!
//! 何を聞くかは表と Tier F の結果から決まる。実際に聞くのは tako-control の
//! `runtime_probe`（待ちは `probe::output_with_timeout` の 1 実装だけを通す）で、
//! ここが持つのは「何を聞くか」と「答えをどう戻すか」だけ（子プロセスは起こさない）。
//!
//! ## 答えは Tier F の材料へ重ねて、もう一度 Tier F を回す
//!
//! - 実行ファイルの場所（ログインシェルの `command -v`）→ GUI の PATH の**前**へ足す
//!   （[`env_with_found`]）。Dock 起動の GUI の痩せた PATH に無かった道具が見えるようになる
//! - 道具へ聞いた env の一覧 → 一覧ファイルの代わりに見せる（[`OverlayFs`]）
//!
//! 検出の規則（どの順で・どれを自動で選ぶか）を 2 か所に書かないための形。重ねたあとに
//! 足すのは、Tier F では決まらない 2 つだけ（包む形の環境の置き場 = [`env_queries`]、
//! interpreter の版 = [`version_queries`]）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::detect::{bare_program, normalize_version};
use super::{is_path_like, Candidate, DetectEnv, Detection, Detector, FsProbe, RuntimeKind};
use crate::platform::support::Platform;

/// 一覧ファイルが無いときに道具へ聞く一覧
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListQuery {
    /// 答えを見せる一覧ファイルの置き場（[`OverlayFs`] の鍵）
    pub list_file: PathBuf,
    /// 道具の名前（[`Plan::lookups`] にも入る。場所が分かったものだけ聞く）
    pub program: String,
    pub args: &'static [&'static str],
    pub json_key: &'static str,
}

/// Tier F の結果から決まる、最初に聞くこと
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// 場所を聞く実行ファイル名（重複なし・素の名前）
    pub lookups: Vec<String>,
    pub lists: Vec<ListQuery>,
}

/// 最初に聞くこと。
///
/// - 場所が確かめられていない候補（`needs_probe` で、`${…}` の先頭が素の名前）の実行ファイル
///   = 既知の置き場にも GUI の PATH にも無かった道具・システムの interpreter
/// - 一覧ファイルが無い一覧型の検出器の道具（一覧もあとで聞く）
pub fn plan(
    kind: &RuntimeKind,
    platform: Platform,
    d: &Detection,
    fs: &dyn FsProbe,
    env: &DetectEnv,
) -> Plan {
    let mut out = Plan::default();
    fn push(name: &str, out: &mut Plan) {
        if !name.is_empty() && !out.lookups.iter().any(|n| n == name) {
            out.lookups.push(name.to_string());
        }
    }
    for c in &d.candidates {
        if !c.needs_probe {
            continue;
        }
        if let Some(first) = c.applied.program.first().filter(|w| !is_path_like(w)) {
            push(first, &mut out);
        }
    }
    for det in kind.detectors {
        let Detector::NamedEnvList(s) = det else {
            continue;
        };
        let (Some(probe), Some(list_file)) = (&s.list_probe, s.list_file.resolve(env)) else {
            continue;
        };
        if fs.is_file(&list_file) {
            continue;
        }
        let program = bare_program(probe.program.get(platform)).to_string();
        push(&program, &mut out);
        out.lists.push(ListQuery {
            list_file,
            program,
            args: probe.args,
            json_key: probe.json_key,
        });
    }
    out
}

/// 見つかった実行ファイルのディレクトリを GUI の PATH の**前**へ足した環境。
///
/// ログインシェルの答えは実行ペインのシェルが解くものと同じなので、GUI の PATH より優先する
pub fn env_with_found(env: &DetectEnv, found: &[PathBuf]) -> DetectEnv {
    let mut out = env.clone();
    let mut front: Vec<PathBuf> = Vec::new();
    for exe in found {
        if let Some(dir) = exe.parent().filter(|d| !d.as_os_str().is_empty()) {
            if !front.iter().any(|d| d == dir) {
                front.push(dir.to_path_buf());
            }
        }
    }
    out.path_dirs.retain(|d| !front.contains(d));
    front.extend(out.path_dirs);
    out.path_dirs = front;
    out
}

/// 包む形の候補の、環境の置き場の問い合わせ
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvQuery {
    /// `Detection::candidates` の添字
    pub index: usize,
    /// 道具の実行ファイル（見つかったものの絶対パス）
    pub program: String,
    pub args: &'static [&'static str],
    /// 答えはカレントディレクトリで決まるので、印のある段で聞く
    pub cwd: PathBuf,
}

/// 環境の置き場を道具に聞ける候補（道具が見つかっていて、まだ activation が無いもの）
pub fn env_queries(kind: &RuntimeKind, d: &Detection) -> Vec<EnvQuery> {
    d.candidates
        .iter()
        .enumerate()
        .filter_map(|(index, c)| {
            let Some(Detector::Wrapper(s)) = kind.detector(c.manager) else {
                return None;
            };
            let probe = s.env_dir_probe.as_ref()?;
            let program = c.applied.program.first().filter(|w| is_path_like(w))?;
            if !c.applied.path_prepend.is_empty() {
                return None;
            }
            Some(EnvQuery {
                index,
                program: program.clone(),
                args: probe.args,
                cwd: c.found_in.clone()?,
            })
        })
        .collect()
}

/// 包む形の候補の環境の置き場を覚えておくときの指紋に使うファイル
/// （印のある段の印のファイル = `poetry.lock` / `pyproject.toml` 等。表の `markers` から組む）。
/// 印が書き換わったら（依存を足した・環境を作り直した）答えを聞き直す
pub fn env_stamp_files(kind: &RuntimeKind, c: &Candidate) -> Vec<PathBuf> {
    let (Some(Detector::Wrapper(s)), Some(dir)) = (kind.detector(c.manager), &c.found_in) else {
        return Vec::new();
    };
    s.markers
        .iter()
        .map(|m| match m {
            super::Marker::File(f) => dir.join(f),
            super::Marker::TomlTable { file, .. } => dir.join(file),
        })
        .collect()
}

/// interpreter の版の問い合わせ
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionQuery {
    pub index: usize,
    pub program: String,
    pub args: &'static [&'static str],
}

/// 版の分からない候補のうち、interpreter を起こして聞けるもの。
///
/// **PATH から解く interpreter（`${…}` が素の名前で、置き場が PATH で見つかったもの）だけ**に聞く。
/// プロジェクトの中の実行ファイル（`.venv/bin/python` 等）は、一覧を取るだけでは起こさない
/// （一覧は開いただけのリポジトリでも取られうる。環境の中の版は Tier F が `pyvenv.cfg` /
/// `conda-meta` / ディレクトリ名から読む）
pub fn version_queries(kind: &RuntimeKind, d: &Detection, fs: &dyn FsProbe) -> Vec<VersionQuery> {
    if kind.version_args.is_empty() {
        return Vec::new();
    }
    d.candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.version.is_none())
        .filter_map(|(index, c)| {
            let program = interpreter_file(c, fs)?;
            Some(VersionQuery {
                index,
                program,
                args: kind.version_args,
            })
        })
        .collect()
}

/// PATH から解く候補の、PATH で見つかった実ファイル
fn interpreter_file(c: &Candidate, fs: &dyn FsProbe) -> Option<String> {
    match c.applied.program.as_slice() {
        [one] if !is_path_like(one) => c
            .location
            .as_deref()
            .filter(|l| fs.is_file(l))
            .map(|l| l.to_string_lossy().into_owned()),
        _ => None,
    }
}

/// 版を聞いた出力（`Python 3.12.4` / `3.12.4`）から版を採る。版らしい語が無ければ `None`
pub fn parse_version(text: &str) -> Option<String> {
    text.split_whitespace()
        .find(|w| w.starts_with(|c: char| c.is_ascii_digit()) && w.contains('.'))
        .and_then(normalize_version)
}

/// 一覧の問い合わせの出力（JSON オブジェクト）から置き場の配列を採る。読めなければ空
pub fn parse_list(json_key: &str, text: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| v.get(json_key).and_then(|a| a.as_array()).cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

/// 問い合わせの答えを、一覧ファイルの代わりに見せる重ね合わせ（読むだけ・stat は下へ委ねる）
pub struct OverlayFs<'a> {
    inner: &'a dyn FsProbe,
    files: BTreeMap<PathBuf, String>,
}

impl<'a> OverlayFs<'a> {
    pub fn new(inner: &'a dyn FsProbe) -> Self {
        Self {
            inner,
            files: BTreeMap::new(),
        }
    }

    /// 一覧ファイルの形（1 行 1 置き場）で見せる
    pub fn with_list(mut self, list_file: PathBuf, prefixes: &[String]) -> Self {
        self.files.insert(list_file, prefixes.join("\n"));
        self
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

impl FsProbe for OverlayFs<'_> {
    fn is_file(&self, path: &Path) -> bool {
        self.files.contains_key(path) || self.inner.is_file(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.inner.is_dir(path)
    }

    fn read_head(&self, path: &Path, max: usize) -> Option<String> {
        match self.files.get(path) {
            Some(text) => {
                let mut end = text.len().min(max);
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                Some(text[..end].to_string())
            }
            None => self.inner.read_head(path, max),
        }
    }

    fn list_dir(&self, path: &Path) -> Vec<String> {
        self.inner.list_dir(path)
    }
}

#[cfg(test)]
mod tests {
    use super::super::fake_fs::FakeFs;
    use super::super::KINDS;
    use super::*;

    const MAC: Platform = Platform::MacOs;

    fn kind() -> &'static RuntimeKind {
        // 表の先頭の行（言語名を書かずに引く）
        &KINDS[0]
    }

    fn home(h: &str) -> DetectEnv {
        DetectEnv {
            home: Some(PathBuf::from(h)),
            ..DetectEnv::default()
        }
    }

    fn slash(p: &Path) -> String {
        p.to_string_lossy().replace('\\', "/")
    }

    /// 表の検出器のうち、指定した戦略の最初のもの（名前を書かずに引く）
    fn first_wrapper_with_probe() -> &'static super::super::WrapperSpec {
        kind()
            .detectors
            .iter()
            .find_map(|d| match d {
                Detector::Wrapper(s) if s.env_dir_probe.is_some() => Some(s),
                _ => None,
            })
            .expect("問い合わせを持つ包む形の行がある")
    }

    fn named_list() -> &'static super::super::NamedEnvListSpec {
        kind()
            .detectors
            .iter()
            .find_map(|d| match d {
                Detector::NamedEnvList(s) => Some(s),
                _ => None,
            })
            .expect("一覧型の行がある")
    }

    #[test]
    fn 場所の分からない道具とシステムのinterpreterを聞く() {
        let w = first_wrapper_with_probe();
        let marker = match w.markers[0] {
            super::super::Marker::File(f) => f,
            super::super::Marker::TomlTable { file, .. } => file,
        };
        let fs = FakeFs::new().file(format!("/h/proj/{marker}"), "");
        let env = home("/h");
        let d = super::super::detect(kind(), MAC, &[PathBuf::from("/h/proj")], &fs, &env);
        let p = plan(kind(), MAC, &d, &fs, &env);
        assert!(
            p.lookups.iter().any(|n| n == w.program.macos),
            "{:?}",
            p.lookups
        );
        assert!(
            p.lookups.iter().any(|n| n == kind().fallback.macos),
            "システムの interpreter も聞く: {:?}",
            p.lookups
        );
        // 一覧ファイルが無いので一覧型の道具も聞く
        let list = named_list();
        let probe = list.list_probe.expect("一覧の問い合わせがある");
        assert_eq!(p.lists.len(), 1);
        assert_eq!(p.lists[0].program, probe.program.macos);
        assert_eq!(p.lists[0].args, probe.args);
        assert!(p.lookups.iter().any(|n| n == probe.program.macos));
    }

    #[test]
    fn 一覧ファイルがあれば一覧は聞かない() {
        let list = named_list();
        let file = list.list_file.resolve(&home("/h")).unwrap();
        let fs = FakeFs::new().file(&file, "");
        let d = super::super::detect(kind(), MAC, &[], &fs, &home("/h"));
        let p = plan(kind(), MAC, &d, &fs, &home("/h"));
        assert!(p.lists.is_empty());
    }

    #[test]
    fn 見つかった道具のディレクトリをpathの前へ足す() {
        let env = DetectEnv {
            path_dirs: vec![PathBuf::from("/usr/bin"), PathBuf::from("/b")],
            ..DetectEnv::default()
        };
        let got = env_with_found(
            &env,
            &[
                PathBuf::from("/b/tool"),
                PathBuf::from("/x/other"),
                PathBuf::from("/x/again"),
            ],
        );
        let dirs: Vec<String> = got.path_dirs.iter().map(|p| slash(p)).collect();
        assert_eq!(dirs, ["/b", "/x", "/usr/bin"]);
    }

    #[test]
    fn 見つかった道具で包む候補の環境を聞きactivationを重ねる() {
        let w = first_wrapper_with_probe();
        let marker = match w.markers[0] {
            super::super::Marker::File(f) => f,
            super::super::Marker::TomlTable { file, .. } => file,
        };
        let fs = FakeFs::new()
            .file(format!("/h/proj/{marker}"), "")
            .file(format!("/t/{}", w.program.macos), "");
        let env = DetectEnv {
            home: Some(PathBuf::from("/h")),
            path_dirs: vec![PathBuf::from("/t")],
            ..DetectEnv::default()
        };
        let mut d = super::super::detect(kind(), MAC, &[PathBuf::from("/h/proj")], &fs, &env);
        let qs = env_queries(kind(), &d);
        let q = qs
            .iter()
            .find(|q| d.candidates[q.index].manager == w.id)
            .expect("環境の置き場を聞く");
        assert_eq!(
            slash(Path::new(&q.program)),
            format!("/t/{}", w.program.macos)
        );
        assert_eq!(slash(&q.cwd), "/h/proj");
        let c = &mut d.candidates[q.index];
        assert!(super::super::apply_probed_env(
            kind(),
            MAC,
            c,
            Path::new("/cache/env-abc")
        ));
        let pre: Vec<String> = c.applied.path_prepend.iter().map(|p| slash(p)).collect();
        assert_eq!(pre, ["/cache/env-abc/bin"]);
        assert!(!c.needs_probe);
        // 重ねたあとは聞かない
        assert!(env_queries(kind(), &d)
            .iter()
            .all(|q| d.candidates[q.index].manager != w.id));
    }

    #[test]
    fn 版は実在するinterpreterにだけ聞く() {
        let fs = FakeFs::new().file("/usr/local/bin/python3", "");
        let env = DetectEnv {
            path_dirs: vec![PathBuf::from("/usr/local/bin")],
            ..DetectEnv::default()
        };
        let d = super::super::detect(kind(), MAC, &[], &fs, &env);
        let qs = version_queries(kind(), &d, &fs);
        assert_eq!(qs.len(), 1, "{qs:?}");
        assert_eq!(slash(Path::new(&qs[0].program)), "/usr/local/bin/python3");
        assert_eq!(qs[0].args, kind().version_args);
        // 見つかっていない（素の名前）なら聞かない
        let d = super::super::detect(kind(), MAC, &[], &FakeFs::new(), &DetectEnv::default());
        assert!(version_queries(kind(), &d, &FakeFs::new()).is_empty());
        // プロジェクトの中の interpreter は、版が分からなくても一覧を取るだけでは起こさない
        let (venv_dir, marker, interp) = kind()
            .detectors
            .iter()
            .find_map(|d| match d {
                Detector::MarkerDir(s) => Some((s.names[0], s.marker, s.layout.interpreter.macos)),
                _ => None,
            })
            .expect("環境のディレクトリ型の行がある");
        let fs = FakeFs::new()
            .file(format!("/h/proj/{venv_dir}/{marker}"), "")
            .file(format!("/h/proj/{venv_dir}/{}", interp.join("/")), "");
        let d = super::super::detect(
            kind(),
            MAC,
            &[PathBuf::from("/h/proj")],
            &fs,
            &DetectEnv::default(),
        );
        let local = d
            .candidates
            .iter()
            .position(|c| c.location.as_deref() == Some(Path::new(&format!("/h/proj/{venv_dir}"))))
            .expect("環境が候補に並ぶ");
        assert!(d.candidates[local].version.is_none(), "印に版が無い");
        assert!(
            version_queries(kind(), &d, &fs)
                .iter()
                .all(|q| q.index != local),
            "プロジェクトの中の実行ファイルを起こそうとしている"
        );
    }

    #[test]
    fn 版と一覧の出力を読む() {
        assert_eq!(parse_version("Python 3.12.4\n").as_deref(), Some("3.12.4"));
        assert_eq!(parse_version("3.11.9").as_deref(), Some("3.11.9"));
        assert_eq!(parse_version("Python 3.13.0rc1").as_deref(), Some("3.13"));
        assert_eq!(parse_version("command not found"), None);
        assert_eq!(parse_version(""), None);
        assert_eq!(
            parse_list("envs", r#"{"envs": ["/o/a", "/o/a/envs/ml", 3], "x": 1}"#),
            ["/o/a", "/o/a/envs/ml"]
        );
        assert!(parse_list("envs", "not json").is_empty());
        assert!(parse_list("envs", r#"{"other": []}"#).is_empty());
    }

    #[test]
    fn 重ね合わせは一覧ファイルの代わりを見せ他は下へ委ねる() {
        let base = FakeFs::new().file("/h/real.txt", "real");
        let o = OverlayFs::new(&base).with_list(
            PathBuf::from("/h/.list"),
            &["/o/a".to_string(), "/o/b".to_string()],
        );
        assert!(o.is_file(Path::new("/h/.list")));
        assert_eq!(
            o.read_head(Path::new("/h/.list"), 100).as_deref(),
            Some("/o/a\n/o/b")
        );
        assert_eq!(
            o.read_head(Path::new("/h/.list"), 3).as_deref(),
            Some("/o/")
        );
        assert_eq!(
            o.read_head(Path::new("/h/real.txt"), 100).as_deref(),
            Some("real")
        );
        assert!(!o.is_empty());
        assert!(OverlayFs::new(&base).is_empty());
    }
}
