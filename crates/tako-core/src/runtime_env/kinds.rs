//! 実行環境の表（**言語と道具の知識はこの 1 枚だけ**が持つ。Issue #1729）
//!
//! 行 = 言語 1 つ、行の中の `detectors` = **自動選択の順**。優先順の根拠は
//! `.agent/plans/2026-09-runner-settings.md` §5.2（要点は各行のコメント）。
//! OS で変わる置き場は [`PerOs`] の 2 列で書く（#1655 の作法）。
//!
//! Windows の列のうち conda の PATH 5 本と pyenv-win の置き場は、conda / pyenv-win が
//! activation で組む値を写したもの。**実機で確かめたものではない**ので、S2（#1730）以降の
//! Windows 実測で食い違えばここを直す（`platform::support::MATRIX` には実測分だけを書く = #591）。

use super::{
    Detector, EnvLayout, EnvValue, InProjectEnv, KnownDir, Marker, MarkerDirSpec, NamedEnvListSpec,
    PathLookupSpec, PerOs, ProbeCmd, RuntimeKind, VersionFileSpec, VersionSource, WrapperSpec,
};

/// venv（PEP 405。`pyvenv.cfg` を持つディレクトリ）の中の置き場
const VENV_LAYOUT: EnvLayout = EnvLayout {
    interpreter: PerOs {
        macos: &["bin", "python"],
        windows: &["Scripts", "python.exe"],
    },
    path_dirs: PerOs {
        macos: &[&["bin"]],
        windows: &[&["Scripts"]],
    },
    env: &[("VIRTUAL_ENV", EnvValue::Location)],
};

/// conda の env の中の置き場。Windows は `conda activate` が PATH へ足す 5 本
const CONDA_LAYOUT: EnvLayout = EnvLayout {
    interpreter: PerOs {
        macos: &["bin", "python"],
        windows: &["python.exe"],
    },
    path_dirs: PerOs {
        macos: &[&["bin"]],
        windows: &[
            &[],
            &["Library", "mingw-w64", "bin"],
            &["Library", "usr", "bin"],
            &["Library", "bin"],
            &["Scripts"],
        ],
    },
    env: &[
        ("CONDA_PREFIX", EnvValue::Location),
        ("CONDA_DEFAULT_ENV", EnvValue::Name),
    ],
};

/// pyenv（Windows は pyenv-win）の `versions/<版>` の中の置き場
const PYENV_LAYOUT: EnvLayout = EnvLayout {
    interpreter: PerOs {
        macos: &["bin", "python"],
        windows: &["python.exe"],
    },
    path_dirs: PerOs {
        macos: &[&["bin"]],
        windows: &[&[], &["Scripts"]],
    },
    env: &[],
};

/// 包む形の道具（uv / poetry / pipenv）を Tier F で探す置き場。
/// 公式の導入スクリプト（`~/.local/bin`）・cargo・Homebrew・Windows の poetry 公式導入先。
/// どちらの OS にも無い置き場は解けないか `is_file` が偽になるだけなので 1 本の列で持つ
const TOOL_DIRS: &[KnownDir] = &[
    KnownDir::Home(&[".local", "bin"]),
    KnownDir::Home(&[".cargo", "bin"]),
    KnownDir::Abs("/opt/homebrew/bin"),
    KnownDir::Abs("/usr/local/bin"),
    KnownDir::Var("APPDATA", &["Python", "Scripts"]),
];

const PYTHON_DETECTORS: &[Detector] = &[
    // 1. uv のプロジェクト: `uv run` が正規の走らせ方で、実行のたびにロックへ同期する
    //    （`.venv/bin/python` を直接叩くと、ロックに足した依存が入っていないまま走る）。
    //    `.venv` があれば activation も併用し、宣言が `pytest` を直接呼んでも効くようにする
    Detector::Wrapper(WrapperSpec {
        id: "uv",
        label: "uv run",
        markers: &[
            Marker::File("uv.lock"),
            Marker::TomlTable {
                file: "pyproject.toml",
                table: "tool.uv",
            },
        ],
        program: PerOs {
            macos: "uv",
            windows: "uv.exe",
        },
        run_args: &["run"],
        program_dirs: TOOL_DIRS,
        in_project_env: Some(InProjectEnv {
            names: &[".venv"],
            marker: "pyvenv.cfg",
            layout: VENV_LAYOUT,
        }),
        env_dir_probe: None,
    }),
    // 2. プロジェクト内の venv: そのプロジェクトの依存を入れた環境で最も具体的。
    //    VS Code の自動選択も「ワークスペース直下の .venv / venv → グローバル」の順。
    //    近い段が勝つ（monorepo のパッケージごとの venv）
    Detector::MarkerDir(MarkerDirSpec {
        id: "venv",
        names: &[".venv", "venv"],
        scan_children: true,
        marker: "pyvenv.cfg",
        layout: VENV_LAYOUT,
        version: VersionSource::KeyValue {
            file: "pyvenv.cfg",
            keys: &["version", "version_info"],
        },
    }),
    // 3. Poetry: venv の既定の置き場はハッシュ付きの名前でファイルシステムだけでは決まらない。
    //    `poetry run` ならツール自身が解く（in-project の `.venv` は 2 が先に拾う）
    Detector::Wrapper(WrapperSpec {
        id: "poetry",
        label: "poetry run",
        markers: &[
            Marker::File("poetry.lock"),
            Marker::TomlTable {
                file: "pyproject.toml",
                table: "tool.poetry",
            },
        ],
        program: PerOs {
            macos: "poetry",
            windows: "poetry.exe",
        },
        run_args: &["run"],
        program_dirs: TOOL_DIRS,
        in_project_env: None,
        env_dir_probe: Some(ProbeCmd {
            args: &["env", "info", "-p"],
        }),
    }),
    // 4. Pipenv: 3 と同じ理由で包む形
    Detector::Wrapper(WrapperSpec {
        id: "pipenv",
        label: "pipenv run",
        markers: &[Marker::File("Pipfile")],
        program: PerOs {
            macos: "pipenv",
            windows: "pipenv.exe",
        },
        run_args: &["run"],
        program_dirs: TOOL_DIRS,
        in_project_env: None,
        env_dir_probe: Some(ProbeCmd { args: &["--venv"] }),
    }),
    // 5. conda: プロジェクトの外にある環境なので venv より後ろ。
    //    `environment.yml` の名前と一覧が**ちょうど 1 つ**一致したときだけ自動で選ぶ（#1466）
    Detector::NamedEnvList(NamedEnvListSpec {
        id: "conda",
        label_prefix: "conda: ",
        list_file: KnownDir::Home(&[".conda", "environments.txt"]),
        project_files: &["environment.yml", "environment.yaml"],
        name_key: "name",
        layout: CONDA_LAYOUT,
        version: VersionSource::FileNamePrefix {
            dir: &["conda-meta"],
            prefix: "python-",
        },
    }),
    // 6. pyenv: `.python-version` は版しか決めない（依存は決めない）ので venv より後ろ
    Detector::VersionFile(VersionFileSpec {
        id: "pyenv",
        label_prefix: "pyenv ",
        files: &[".python-version"],
        skip_values: &["system"],
        root_env: &["PYENV_ROOT"],
        root_default: PerOs {
            macos: KnownDir::Home(&[".pyenv"]),
            windows: KnownDir::Home(&[".pyenv", "pyenv-win"]),
        },
        versions_dir: &["versions"],
        layout: PYENV_LAYOUT,
    }),
    // 7. システム: 解くのは実行ペインのシェル（今までと同じ）。ここは表示用に PATH を覗くだけ
    Detector::PathLookup(PathLookupSpec {
        id: "system",
        file_names: PerOs {
            macos: &["python3", "python"],
            windows: &["python.exe"],
        },
        exclude_dirs: PerOs {
            macos: &[],
            windows: &[KnownDir::Var("LOCALAPPDATA", &["Microsoft", "WindowsApps"])],
        },
    }),
];

/// 実行環境の表の正本
pub const KINDS: &[RuntimeKind] = &[RuntimeKind {
    id: "python",
    extensions: &["py"],
    variable: "python",
    // 組み込み既定表（`platform::runner_defaults` の `py` 行）と同じ名前。
    // 実行環境が見つからないときの展開が今と 1 バイトも変わらないための前提（テストで縛る）
    fallback: PerOs {
        macos: "python3",
        windows: "python",
    },
    wrapped_program: "python",
    detectors: PYTHON_DETECTORS,
}];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::support::Platform;

    const BOTH: [Platform; 2] = [Platform::MacOs, Platform::Windows];

    #[test]
    fn 検出器のidはkindの中で重複せず保存用の予約語とぶつからない() {
        for kind in KINDS {
            let mut seen = std::collections::BTreeSet::new();
            for det in kind.detectors {
                assert!(
                    seen.insert(det.id()),
                    "{}: 検出器 id の重複 {}",
                    kind.id,
                    det.id()
                );
                assert_ne!(
                    det.id(),
                    crate::runner_config::RuntimeRef::EXPLICIT_PATH,
                    "{}: 予約語と同じ id",
                    kind.id
                );
            }
        }
    }

    #[test]
    fn 拡張子はkindをまたいで重複しない() {
        let mut seen = std::collections::BTreeMap::new();
        for kind in KINDS {
            for ext in kind.extensions {
                assert_eq!(*ext, ext.to_ascii_lowercase(), "拡張子は小文字で書く");
                if let Some(prev) = seen.insert(*ext, kind.id) {
                    panic!("拡張子 {ext} が {prev} と {} の両方にある", kind.id);
                }
            }
        }
    }

    /// 実行環境が見つからないときの展開が、組み込み既定表の今の形と同じ名前になる
    #[test]
    fn fallbackは組み込み既定表の先頭の語と一致する() {
        for kind in KINDS {
            for ext in kind.extensions {
                let entry = crate::platform::runner_defaults::entry(ext)
                    .unwrap_or_else(|| panic!("{ext} が既定表に無い"));
                for p in BOTH {
                    let Some(cmd) = entry.get(p).command() else {
                        continue;
                    };
                    let first = cmd.split_whitespace().next().unwrap_or_default();
                    assert_eq!(
                        first,
                        kind.fallback.get(p),
                        "{ext} / {p:?}: 既定表の `{cmd}` と fallback がずれている"
                    );
                }
            }
        }
    }

    fn layouts() -> Vec<(String, EnvLayout)> {
        let mut out = Vec::new();
        for kind in KINDS {
            for det in kind.detectors {
                let layout = match det {
                    Detector::MarkerDir(s) => Some(s.layout),
                    Detector::NamedEnvList(s) => Some(s.layout),
                    Detector::VersionFile(s) => Some(s.layout),
                    Detector::Wrapper(s) => s.in_project_env.map(|e| e.layout),
                    Detector::PathLookup(_) | Detector::EnvSwitch(_) => None,
                };
                if let Some(l) = layout {
                    out.push((format!("{}/{}", kind.id, det.id()), l));
                }
            }
        }
        out
    }

    /// Windows の実行ファイルは `.exe` で終わり、macOS 側には付かない
    /// （付け忘れると Windows で「見つからない」、付け違えると macOS で同じ）
    #[test]
    fn interpreterの拡張子はosの列に合っている() {
        for (name, l) in layouts() {
            let win = l.interpreter.windows.last().copied().unwrap_or_default();
            let mac = l.interpreter.macos.last().copied().unwrap_or_default();
            assert!(
                win.ends_with(".exe"),
                "{name}: Windows の interpreter `{win}`"
            );
            assert!(
                !mac.ends_with(".exe"),
                "{name}: macOS の interpreter `{mac}`"
            );
        }
        for kind in KINDS {
            for det in kind.detectors {
                match det {
                    Detector::Wrapper(s) => {
                        assert!(s.program.windows.ends_with(".exe"), "{}", s.id);
                        assert!(!s.program.macos.ends_with(".exe"), "{}", s.id);
                    }
                    Detector::PathLookup(s) => {
                        assert!(
                            s.file_names.windows.iter().all(|n| n.ends_with(".exe")),
                            "{}",
                            s.id
                        );
                        assert!(
                            s.file_names.macos.iter().all(|n| !n.ends_with(".exe")),
                            "{}",
                            s.id
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    /// 成分に区切り文字を書かない（`"Scripts\\python.exe"` と書くと macOS で 1 成分に化ける）
    #[test]
    fn 成分に区切り文字を含めない() {
        for (name, l) in layouts() {
            let comps = l
                .interpreter
                .macos
                .iter()
                .chain(l.interpreter.windows)
                .chain(l.path_dirs.macos.iter().flat_map(|c| c.iter()))
                .chain(l.path_dirs.windows.iter().flat_map(|c| c.iter()));
            for c in comps {
                assert!(!c.contains('/') && !c.contains('\\'), "{name}: 成分 `{c}`");
            }
        }
    }
}
