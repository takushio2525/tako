//! 検出の戦略の実装（**言語・道具の名前を書かない**。知識は `kinds.rs` の表だけが持つ）
//!
//! 行うのは stat と小さなファイルの先頭読みだけ（設計書の Tier F）。子プロセスは起こさない。

use std::path::{Path, PathBuf};

use super::{
    join, Applied, Candidate, DetectEnv, Detection, Detector, EnvLayout, EnvSwitchSpec, EnvValue,
    FsProbe, InProjectEnv, Marker, MarkerDirSpec, NamedEnvListSpec, PathLookupSpec, RuntimeKind,
    Unresolved, VersionFileSpec, VersionSource, WrapperSpec, SCAN_CHILDREN_MAX,
};
use crate::i18n::Lang;
use crate::platform::support::Platform;
use crate::runner_config::RuntimeRef;

/// 印・版・宣言の小さなファイルを読む上限
const SMALL_FILE_MAX: usize = 4 * 1024;
/// TOML（`pyproject.toml` 等）の表を探す上限
const TOML_HEAD_MAX: usize = 64 * 1024;
/// env の一覧ファイルを読む上限
const LIST_FILE_MAX: usize = 64 * 1024;

/// kind の全候補を**自動選択の順**に並べ、自動で選ぶ 1 つを決める。
///
/// `dirs` は見てよいディレクトリの列（**近い順**。ファイルのディレクトリから
/// `project_root::candidate_dirs` の境界まで）。
pub fn detect(
    kind: &RuntimeKind,
    platform: Platform,
    dirs: &[PathBuf],
    fs: &dyn FsProbe,
    env: &DetectEnv,
) -> Detection {
    let mut out = Detection::default();
    for det in kind.detectors {
        let found = match det {
            Detector::Wrapper(s) => {
                detect_wrapper(kind, s, platform, dirs, fs, env, &mut out.warnings)
            }
            Detector::MarkerDir(s) => detect_marker_dir(kind, s, platform, dirs, fs),
            Detector::NamedEnvList(s) => {
                detect_named_env_list(kind, s, platform, dirs, fs, env, &mut out.warnings)
            }
            Detector::VersionFile(s) => {
                detect_version_file(kind, s, platform, dirs, fs, env, &mut out.warnings)
            }
            Detector::PathLookup(s) => {
                detect_path_lookup(kind, s, platform, fs, env, &mut out.warnings)
            }
            Detector::EnvSwitch(s) => detect_env_switch(kind, s, platform, dirs, fs),
        };
        out.candidates.extend(found);
    }
    out.auto = out.candidates.iter().position(|c| c.auto_eligible);
    out
}

/// 保存した参照を解く。**解けなければ既定へ落とさずに `Err`**（#1466。黙って別の
/// interpreter で走ると「依存が無い」エラーになり、原因がたどれない）。
///
/// `root` は相対の鍵（プロジェクトルートからの venv の相対）を解く基準。
pub fn resolve_ref(
    kind: &RuntimeKind,
    platform: Platform,
    r: &RuntimeRef,
    root: Option<&Path>,
    fs: &dyn FsProbe,
    env: &DetectEnv,
) -> Result<Candidate, Unresolved> {
    let fail = |reason: String| Unresolved {
        manager: r.manager.clone(),
        key: r.key.clone(),
        reason,
    };
    let absolute = |key: &str| -> PathBuf {
        let k = Path::new(key);
        match root {
            Some(root) if !k.is_absolute() => root.join(k),
            _ => k.to_path_buf(),
        }
    };

    if r.manager == RuntimeRef::EXPLICIT_PATH {
        let path = absolute(&r.key);
        return if fs.is_file(&path) {
            Ok(explicit_candidate(kind, &path))
        } else {
            Err(fail(msg_missing(&path)))
        };
    }

    let Some(det) = kind.detector(&r.manager) else {
        return Err(fail(msg_unknown_manager(&r.manager, kind.id)));
    };
    match det {
        Detector::MarkerDir(s) => {
            let dir = absolute(&r.key);
            if fs.is_file(&dir.join(s.marker)) {
                Ok(marker_dir_candidate(kind, s, platform, &dir, fs))
            } else {
                Err(fail(msg_missing(&dir)))
            }
        }
        Detector::Wrapper(s) => {
            let dir = absolute(&r.key);
            if s.markers.iter().any(|m| marker_present(m, &dir, fs)) {
                let program = find_program(s, platform, fs, env);
                Ok(wrapper_candidate(
                    kind,
                    s,
                    platform,
                    &dir,
                    program.as_deref(),
                    fs,
                ))
            } else {
                Err(fail(msg_missing(&dir)))
            }
        }
        Detector::NamedEnvList(s) => {
            let prefixes = env_prefixes(s, fs, env);
            let hits: Vec<&String> = if looks_like_path(&r.key) {
                prefixes.iter().filter(|p| **p == r.key).collect()
            } else {
                prefixes
                    .iter()
                    .filter(|p| env_name_of(p) == r.key)
                    .collect()
            };
            match hits.as_slice() {
                [one] => Ok(named_env_candidate(
                    kind, s, platform, one, &r.key, None, fs, true,
                )),
                [] => Err(fail(msg_env_not_listed(s.label_prefix, &r.key))),
                many => Err(fail(msg_env_ambiguous(s.label_prefix, &r.key, many.len()))),
            }
        }
        Detector::VersionFile(s) => {
            let Some(root) = version_root(s, platform, env) else {
                return Err(fail(msg_not_installed(s.label_prefix, &r.key)));
            };
            let dir = join(&root, s.versions_dir).join(&r.key);
            if fs.is_dir(&dir) {
                Ok(version_candidate(
                    kind, s, platform, &dir, &r.key, None, true,
                ))
            } else {
                Err(fail(msg_not_installed(s.label_prefix, &r.key)))
            }
        }
        // 解くのは実行ペインのシェル。鍵（プログラム名）をそのまま使う
        Detector::PathLookup(s) => Ok(path_lookup_candidate(kind, s, &r.key, None)),
        Detector::EnvSwitch(s) => Ok(env_switch_candidate(kind, s, platform, &r.key, None)),
    }
    .map(|mut c| {
        // 保存した鍵のまま返す（相対の鍵を絶対へ書き換えて保存し直さない）
        c.key = r.key.clone();
        c
    })
}

// ─── 印を見つけたらツールで包む ─────────────────────────────────────────

fn detect_wrapper(
    kind: &RuntimeKind,
    s: &WrapperSpec,
    platform: Platform,
    dirs: &[PathBuf],
    fs: &dyn FsProbe,
    env: &DetectEnv,
    warnings: &mut Vec<String>,
) -> Vec<Candidate> {
    let Some(dir) = dirs
        .iter()
        .find(|d| s.markers.iter().any(|m| marker_present(m, d, fs)))
    else {
        return Vec::new();
    };
    let program = find_program(s, platform, fs, env);
    if program.is_none() {
        warnings.push(msg_tool_unconfirmed(s.label));
    }
    vec![wrapper_candidate(
        kind,
        s,
        platform,
        dir,
        program.as_deref(),
        fs,
    )]
}

fn wrapper_candidate(
    kind: &RuntimeKind,
    s: &WrapperSpec,
    platform: Platform,
    dir: &Path,
    program: Option<&Path>,
    fs: &dyn FsProbe,
) -> Candidate {
    let env_dir = s
        .in_project_env
        .as_ref()
        .and_then(|e| find_in_project_env(e, dir, fs));
    let activation = match (&s.in_project_env, &env_dir) {
        (Some(e), Some(d)) => Some(activate(&e.layout, platform, d, None)),
        _ => None,
    };
    // 見つけた場所があれば絶対パスで呼ぶ（実行ペインのシェルの PATH に道具の置き場が
    // 無くても走る）。見つかっていなければ名前のまま（解くのはシェル）
    let first = program
        .map(path_str)
        .unwrap_or_else(|| bare_program(s.program.get(platform)).to_string());
    let mut argv = vec![first];
    argv.extend(s.run_args.iter().map(|a| a.to_string()));
    argv.push(kind.wrapped_program.to_string());

    let (path_prepend, env_pairs) = activation
        .map(|a| (a.path_prepend, a.env))
        .unwrap_or_default();
    Candidate {
        kind: kind.id,
        manager: s.id,
        key: path_str(dir),
        label: s.label.to_string(),
        version: None,
        location: env_dir,
        found_in: Some(dir.to_path_buf()),
        auto_eligible: program.is_some(),
        // 道具が確かめられていない / 環境の置き場を道具に聞かないと分からない
        needs_probe: program.is_none() || (s.env_dir_probe.is_some() && path_prepend.is_empty()),
        applied: Applied {
            program: argv,
            path_prepend,
            env: env_pairs,
        },
    }
}

/// 道具の実行ファイルを既知の置き場と GUI プロセスの PATH から探す（stat だけ）
fn find_program(
    s: &WrapperSpec,
    platform: Platform,
    fs: &dyn FsProbe,
    env: &DetectEnv,
) -> Option<PathBuf> {
    let name = s.program.get(platform);
    s.program_dirs
        .iter()
        .filter_map(|d| d.resolve(env))
        .chain(env.path_dirs.iter().cloned())
        .map(|d| d.join(name))
        .find(|p| fs.is_file(p))
}

fn find_in_project_env(e: &InProjectEnv, dir: &Path, fs: &dyn FsProbe) -> Option<PathBuf> {
    e.names
        .iter()
        .map(|n| dir.join(n))
        .find(|d| fs.is_file(&d.join(e.marker)))
}

fn marker_present(m: &Marker, dir: &Path, fs: &dyn FsProbe) -> bool {
    match m {
        Marker::File(name) => fs.is_file(&dir.join(name)),
        Marker::TomlTable { file, table } => fs
            .read_head(&dir.join(file), TOML_HEAD_MAX)
            .is_some_and(|text| has_toml_table(&text, table)),
    }
}

/// `[table]` / `[table.sub]` / `[[table.sub]]` の見出しが在るか（TOML の構文解析はしない）
fn has_toml_table(text: &str, table: &str) -> bool {
    text.lines().any(|line| {
        let head = line.split('#').next().unwrap_or_default().trim();
        let inner = head
            .strip_prefix("[[")
            .and_then(|s| s.strip_suffix("]]"))
            .or_else(|| head.strip_prefix('[').and_then(|s| s.strip_suffix(']')));
        inner.is_some_and(|name| {
            let name = name.trim();
            name == table
                || name
                    .strip_prefix(table)
                    .is_some_and(|rest| rest.starts_with('.'))
        })
    })
}

// ─── 印のファイルを持つディレクトリ ─────────────────────────────────────

fn detect_marker_dir(
    kind: &RuntimeKind,
    s: &MarkerDirSpec,
    platform: Platform,
    dirs: &[PathBuf],
    fs: &dyn FsProbe,
) -> Vec<Candidate> {
    dirs.iter()
        .flat_map(|dir| marker_dirs_in(s, dir, fs))
        .map(|env_dir| marker_dir_candidate(kind, s, platform, &env_dir, fs))
        .collect()
}

/// 1 段ぶん: 先に `names` の並びで、次に（許されていれば）他の子を名前順で
fn marker_dirs_in(s: &MarkerDirSpec, dir: &Path, fs: &dyn FsProbe) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = s
        .names
        .iter()
        .map(|n| dir.join(n))
        .filter(|d| fs.is_file(&d.join(s.marker)))
        .collect();
    if s.scan_children {
        let mut children = fs.list_dir(dir);
        children.sort();
        found.extend(
            children
                .into_iter()
                .filter(|n| !s.names.contains(&n.as_str()))
                .take(SCAN_CHILDREN_MAX)
                .map(|n| dir.join(n))
                .filter(|d| fs.is_file(&d.join(s.marker))),
        );
    }
    found
}

fn marker_dir_candidate(
    kind: &RuntimeKind,
    s: &MarkerDirSpec,
    platform: Platform,
    env_dir: &Path,
    fs: &dyn FsProbe,
) -> Candidate {
    let a = activate(&s.layout, platform, env_dir, None);
    Candidate {
        kind: kind.id,
        manager: s.id,
        key: path_str(env_dir),
        label: file_name(env_dir),
        version: read_version(&s.version, env_dir, fs),
        location: Some(env_dir.to_path_buf()),
        found_in: env_dir.parent().map(Path::to_path_buf),
        auto_eligible: true,
        needs_probe: false,
        applied: Applied {
            program: vec![path_str(&a.interpreter)],
            path_prepend: a.path_prepend,
            env: a.env,
        },
    }
}

// ─── 一覧ファイルとプロジェクトの宣言の突き合わせ ───────────────────────

fn detect_named_env_list(
    kind: &RuntimeKind,
    s: &NamedEnvListSpec,
    platform: Platform,
    dirs: &[PathBuf],
    fs: &dyn FsProbe,
    env: &DetectEnv,
    warnings: &mut Vec<String>,
) -> Vec<Candidate> {
    let prefixes = env_prefixes(s, fs, env);
    let wanted = dirs.iter().find_map(|d| {
        s.project_files.iter().find_map(|f| {
            fs.read_head(&d.join(f), SMALL_FILE_MAX)
                .and_then(|t| yaml_top_level(&t, s.name_key))
                .map(|name| (d.clone(), name))
        })
    });

    let matched: Vec<&String> = match &wanted {
        Some((_, name)) => prefixes
            .iter()
            .filter(|p| env_name_of(p) == *name)
            .collect(),
        None => Vec::new(),
    };
    if let Some((_, name)) = &wanted {
        match matched.len() {
            0 => warnings.push(msg_env_not_listed(s.label_prefix, name)),
            1 => {}
            n => warnings.push(msg_env_ambiguous(s.label_prefix, name, n)),
        }
    }
    let unique = matched.len() == 1;
    let found_in = wanted.as_ref().map(|(d, _)| d.as_path());

    // 一致したものを先頭に（自動で選ぶ候補が上に来るように）
    let mut ordered: Vec<&String> = matched.clone();
    ordered.extend(prefixes.iter().filter(|p| !matched.contains(p)));
    ordered
        .into_iter()
        .map(|prefix| {
            let name = env_name_of(prefix);
            // 名前が一覧の中で一意なら名前で、そうでなければ prefix で覚える
            let key = if prefixes.iter().filter(|p| env_name_of(p) == name).count() == 1 {
                name.to_string()
            } else {
                prefix.clone()
            };
            let is_match = matched.contains(&prefix);
            named_env_candidate(
                kind,
                s,
                platform,
                prefix,
                &key,
                is_match.then_some(found_in).flatten(),
                fs,
                unique && is_match,
            )
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn named_env_candidate(
    kind: &RuntimeKind,
    s: &NamedEnvListSpec,
    platform: Platform,
    prefix: &str,
    key: &str,
    found_in: Option<&Path>,
    fs: &dyn FsProbe,
    auto_eligible: bool,
) -> Candidate {
    let location = PathBuf::from(prefix);
    let name = env_name_of(prefix);
    let a = activate(&s.layout, platform, &location, Some(name));
    Candidate {
        kind: kind.id,
        manager: s.id,
        key: key.to_string(),
        label: format!("{}{}", s.label_prefix, name),
        version: read_version(&s.version, &location, fs),
        location: Some(location),
        found_in: found_in.map(Path::to_path_buf),
        auto_eligible,
        needs_probe: false,
        applied: Applied {
            program: vec![path_str(&a.interpreter)],
            path_prepend: a.path_prepend,
            env: a.env,
        },
    }
}

/// 一覧ファイルの env の置き場（在るものだけ・重複なし・並びは一覧のまま）
fn env_prefixes(s: &NamedEnvListSpec, fs: &dyn FsProbe, env: &DetectEnv) -> Vec<String> {
    let Some(text) = s
        .list_file
        .resolve(env)
        .and_then(|f| fs.read_head(&f, LIST_FILE_MAX))
    else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') || out.iter().any(|p| p == line) {
            continue;
        }
        if fs.is_dir(Path::new(line)) {
            out.push(line.to_string());
        }
    }
    out
}

/// env の名前 = 置き場の最後の成分。一覧は書いた OS の区切りで並ぶので `/` と `\` の両方で割る
fn env_name_of(prefix: &str) -> &str {
    prefix
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(prefix)
}

fn looks_like_path(key: &str) -> bool {
    key.contains('/') || key.contains('\\')
}

/// YAML の最上位の `key: value`（インデントの無い行だけ。構文解析はしない）
fn yaml_top_level(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let rest = line.strip_prefix(key)?.strip_prefix(':')?;
        let value = rest.split('#').next().unwrap_or_default().trim();
        let value = value.trim_matches(|c| c == '"' || c == '\'');
        (!value.is_empty()).then(|| value.to_string())
    })
}

// ─── 版を書いたファイル → 道具の置き場の versions/<版> ──────────────────

fn detect_version_file(
    kind: &RuntimeKind,
    s: &VersionFileSpec,
    platform: Platform,
    dirs: &[PathBuf],
    fs: &dyn FsProbe,
    env: &DetectEnv,
    warnings: &mut Vec<String>,
) -> Vec<Candidate> {
    let Some(root) = version_root(s, platform, env) else {
        return Vec::new();
    };
    let versions = join(&root, s.versions_dir);
    let wanted = dirs
        .iter()
        .find_map(|d| {
            s.files.iter().find_map(|f| {
                fs.read_head(&d.join(f), SMALL_FILE_MAX)
                    .and_then(|t| first_value(&t))
                    .map(|v| (d.clone(), v))
            })
        })
        .filter(|(_, v)| !s.skip_values.contains(&v.as_str()));

    let mut installed: Vec<String> = fs
        .list_dir(&versions)
        .into_iter()
        .filter(|v| fs.is_dir(&versions.join(v)))
        .collect();
    installed.sort();
    if let Some((_, v)) = &wanted {
        if !installed.contains(v) {
            warnings.push(msg_not_installed(s.label_prefix, v));
        }
    }

    let wanted_version = wanted.as_ref().map(|(_, v)| v.as_str());
    let found_in = wanted.as_ref().map(|(d, _)| d.as_path());
    let mut ordered: Vec<&String> = installed
        .iter()
        .filter(|v| Some(v.as_str()) == wanted_version)
        .collect();
    ordered.extend(
        installed
            .iter()
            .filter(|v| Some(v.as_str()) != wanted_version),
    );
    ordered
        .into_iter()
        .map(|v| {
            let is_wanted = Some(v.as_str()) == wanted_version;
            version_candidate(
                kind,
                s,
                platform,
                &versions.join(v),
                v,
                is_wanted.then_some(found_in).flatten(),
                is_wanted,
            )
        })
        .collect()
}

fn version_root(s: &VersionFileSpec, platform: Platform, env: &DetectEnv) -> Option<PathBuf> {
    s.root_env
        .iter()
        .find_map(|v| env.var(v))
        .map(PathBuf::from)
        .or_else(|| s.root_default.get(platform).resolve(env))
}

fn version_candidate(
    kind: &RuntimeKind,
    s: &VersionFileSpec,
    platform: Platform,
    dir: &Path,
    version: &str,
    found_in: Option<&Path>,
    auto_eligible: bool,
) -> Candidate {
    let a = activate(&s.layout, platform, dir, None);
    Candidate {
        kind: kind.id,
        manager: s.id,
        key: version.to_string(),
        label: format!("{}{}", s.label_prefix, version),
        version: Some(version.to_string()),
        location: Some(dir.to_path_buf()),
        found_in: found_in.map(Path::to_path_buf),
        auto_eligible,
        needs_probe: false,
        applied: Applied {
            program: vec![path_str(&a.interpreter)],
            path_prepend: a.path_prepend,
            env: a.env,
        },
    }
}

/// 版のファイルの値（空行・コメントを飛ばした最初の語）
fn first_value(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .and_then(|l| l.split_whitespace().next())
        .map(str::to_string)
}

// ─── PATH 上の名前（最後の砦） ──────────────────────────────────────────

fn detect_path_lookup(
    kind: &RuntimeKind,
    s: &PathLookupSpec,
    platform: Platform,
    fs: &dyn FsProbe,
    env: &DetectEnv,
    warnings: &mut Vec<String>,
) -> Vec<Candidate> {
    let excluded: Vec<PathBuf> = s
        .exclude_dirs
        .get(platform)
        .iter()
        .filter_map(|d| d.resolve(env))
        .collect();
    let mut found = None;
    let mut alias_dir = None;
    'dirs: for dir in &env.path_dirs {
        for name in s.file_names.get(platform) {
            let path = dir.join(name);
            if !fs.is_file(&path) {
                continue;
            }
            if excluded.iter().any(|e| same_dir(platform, e, dir)) {
                alias_dir.get_or_insert_with(|| dir.clone());
                continue;
            }
            found = Some(path);
            break 'dirs;
        }
    }
    if found.is_none() {
        if let Some(dir) = &alias_dir {
            warnings.push(msg_alias_only(dir));
        }
    }
    vec![path_lookup_candidate(
        kind,
        s,
        kind.fallback.get(platform),
        found,
    )]
}

fn path_lookup_candidate(
    kind: &RuntimeKind,
    s: &PathLookupSpec,
    program: &str,
    location: Option<PathBuf>,
) -> Candidate {
    Candidate {
        kind: kind.id,
        manager: s.id,
        key: program.to_string(),
        label: program.to_string(),
        version: None,
        needs_probe: location.is_none(),
        location,
        found_in: None,
        // 何も見つからなければこれが走る（今までと同じ = 名前のまま、解くのはシェル）
        auto_eligible: true,
        applied: Applied {
            program: vec![program.to_string()],
            path_prepend: Vec::new(),
            env: Vec::new(),
        },
    }
}

/// ディレクトリの同一判定。Windows のファイルシステムは大文字小文字を区別しない
fn same_dir(platform: Platform, a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .trim_end_matches(['/', '\\'])
            .to_string()
    };
    match platform {
        Platform::Windows => norm(a).eq_ignore_ascii_case(&norm(b)),
        Platform::MacOs => norm(a) == norm(b),
    }
}

// ─── 環境変数で切り替える toolchain ─────────────────────────────────────

fn detect_env_switch(
    kind: &RuntimeKind,
    s: &EnvSwitchSpec,
    platform: Platform,
    dirs: &[PathBuf],
    fs: &dyn FsProbe,
) -> Vec<Candidate> {
    dirs.iter()
        .find_map(|d| {
            s.files.iter().find_map(|f| {
                let text = fs.read_head(&d.join(f), SMALL_FILE_MAX)?;
                // TOML の形（`channel = "…"`）ならそのキー、素の 1 行（`1.79`）ならその値。
                // 同じ行が両方の書き方のファイルを受け持てるように
                let value = s
                    .toml_key
                    .and_then(|key| toml_string_value(&text, key))
                    .or_else(|| (!text.contains('=')).then(|| first_value(&text)).flatten())?;
                Some(env_switch_candidate(kind, s, platform, &value, Some(d)))
            })
        })
        .into_iter()
        .collect()
}

fn env_switch_candidate(
    kind: &RuntimeKind,
    s: &EnvSwitchSpec,
    platform: Platform,
    value: &str,
    found_in: Option<&Path>,
) -> Candidate {
    Candidate {
        kind: kind.id,
        manager: s.id,
        key: value.to_string(),
        label: format!("{}{}", s.label_prefix, value),
        version: Some(value.to_string()),
        location: None,
        found_in: found_in.map(Path::to_path_buf),
        auto_eligible: true,
        needs_probe: false,
        applied: Applied {
            program: vec![kind.fallback.get(platform).to_string()],
            path_prepend: Vec::new(),
            env: vec![(s.var.to_string(), value.to_string())],
        },
    }
}

/// TOML の `key = "value"`（表を問わず最初の 1 つ。構文解析はしない）
fn toml_string_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let rest = line
            .trim()
            .strip_prefix(key)?
            .trim_start()
            .strip_prefix('=')?;
        let value = rest.split('#').next().unwrap_or_default().trim();
        let value = value.trim_matches(|c| c == '"' || c == '\'');
        (!value.is_empty()).then(|| value.to_string())
    })
}

// ─── ユーザーが指定した interpreter ─────────────────────────────────────

fn explicit_candidate(kind: &RuntimeKind, path: &Path) -> Candidate {
    Candidate {
        kind: kind.id,
        manager: RuntimeRef::EXPLICIT_PATH,
        key: path_str(path),
        label: path_str(path),
        version: None,
        location: Some(path.to_path_buf()),
        found_in: None,
        auto_eligible: false,
        needs_probe: false,
        applied: Applied {
            program: vec![path_str(path)],
            // interpreter の隣（venv の bin/ や Scripts\）にある pip / pytest も解けるように
            path_prepend: path.parent().map(Path::to_path_buf).into_iter().collect(),
            env: Vec::new(),
        },
    }
}

// ─── 共通 ──────────────────────────────────────────────────────────────

struct Activation {
    interpreter: PathBuf,
    path_prepend: Vec<PathBuf>,
    env: Vec<(String, String)>,
}

fn activate(
    layout: &EnvLayout,
    platform: Platform,
    location: &Path,
    name: Option<&str>,
) -> Activation {
    Activation {
        interpreter: join(location, layout.interpreter.get(platform)),
        path_prepend: layout
            .path_dirs
            .get(platform)
            .iter()
            .map(|c| join(location, c))
            .collect(),
        env: layout
            .env
            .iter()
            .filter_map(|(var, value)| {
                let v = match value {
                    EnvValue::Location => Some(path_str(location)),
                    EnvValue::Name => name.map(str::to_string),
                }?;
                Some((var.to_string(), v))
            })
            .collect(),
    }
}

fn read_version(source: &VersionSource, location: &Path, fs: &dyn FsProbe) -> Option<String> {
    match source {
        VersionSource::None => None,
        VersionSource::KeyValue { file, keys } => {
            let text = fs.read_head(&location.join(file), SMALL_FILE_MAX)?;
            keys.iter().find_map(|key| {
                text.lines().find_map(|line| {
                    let (k, v) = line.split_once('=')?;
                    (k.trim() == *key)
                        .then(|| normalize_version(v.trim()))
                        .flatten()
                })
            })
        }
        VersionSource::FileNamePrefix { dir, prefix } => {
            let mut names = fs.list_dir(&join(location, dir));
            names.sort();
            names.iter().find_map(|n| {
                let rest = n.strip_prefix(prefix)?;
                rest.starts_with(|c: char| c.is_ascii_digit())
                    .then(|| normalize_version(rest.split('-').next().unwrap_or(rest)))
                    .flatten()
            })
        }
        VersionSource::DirName => Some(file_name(location)),
    }
}

/// `3.12.4.final.0` / `3.12.4` → `3.12.4`（数字の成分を 3 つまで）
fn normalize_version(raw: &str) -> Option<String> {
    let parts: Vec<&str> = raw
        .split('.')
        .take_while(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        .take(3)
        .collect();
    (!parts.is_empty()).then(|| parts.join("."))
}

fn bare_program(name: &str) -> &str {
    name.strip_suffix(".exe").unwrap_or(name)
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path_str(p))
}

// ─── 案内文（日英。名前は表の値から組む） ──────────────────────────────

fn tr(ja: String, en: String) -> String {
    match crate::i18n::lang() {
        Lang::Ja => ja,
        Lang::En => en,
    }
}

fn msg_tool_unconfirmed(label: &str) -> String {
    tr(
        format!("{label} の印があるが、道具がまだ見つかっていないので自動では選ばない（確かめると選べる）"),
        format!("Found a project for {label}, but the tool itself is not confirmed yet, so it is not picked automatically"),
    )
}

fn msg_env_not_listed(prefix: &str, name: &str) -> String {
    tr(
        format!("{prefix}{name} が env の一覧に無い"),
        format!("{prefix}{name} is not in the environment list"),
    )
}

fn msg_env_ambiguous(prefix: &str, name: &str, n: usize) -> String {
    tr(
        format!("{prefix}{name} に一致する env が {n} 件あるので自動では選ばない（選んでください）"),
        format!("{n} environments match {prefix}{name}, so none is picked automatically (please choose one)"),
    )
}

fn msg_not_installed(prefix: &str, version: &str) -> String {
    tr(
        format!("{prefix}{version} が入っていない"),
        format!("{prefix}{version} is not installed"),
    )
}

fn msg_alias_only(dir: &Path) -> String {
    tr(
        format!(
            "PATH で見つかったのは実体の無い別名だけ（{}）",
            dir.display()
        ),
        format!(
            "Only an app execution alias was found on PATH ({})",
            dir.display()
        ),
    )
}

fn msg_missing(path: &Path) -> String {
    tr(
        format!("{} が見つからない", path.display()),
        format!("{} was not found", path.display()),
    )
}

fn msg_unknown_manager(manager: &str, kind: &str) -> String {
    tr(
        format!("`{manager}` は {kind} の実行環境の種類に無い"),
        format!("`{manager}` is not a known runtime manager for {kind}"),
    )
}
