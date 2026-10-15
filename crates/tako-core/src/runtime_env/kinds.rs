//! 実行環境の表（**言語と道具の知識はこの 1 枚だけ**が持つ。Issue #1729）
//!
//! 行 = 言語 1 つ、行の中の `detectors` = **自動選択の順**。優先順の根拠は
//! `.agent/plans/2026-09-runner-settings.md` §5.2（要点は各行のコメント）。
//! OS で変わる置き場は [`PerOs`] の 2 列で書く（#1655 の作法）。
//!
//! Windows の列のうち conda の PATH 5 本・pyenv-win の置き場・nvm-windows と fnm の置き場は、
//! それぞれの道具が activation / 導入で組む値を写したもの。**実機で確かめたものではない**ので、
//! Windows 実測で食い違えばここを直す（`platform::support::MATRIX` には実測分だけを書く = #591）。

use super::{
    BundledSpec, Detector, EnvLayout, EnvSwitchSpec, EnvValue, InProjectEnv, KnownDir, ListProbe,
    Marker, MarkerDirSpec, NamedEnvListSpec, PathLookupSpec, PerOs, ProbeCmd, RuntimeKind,
    VersionFileSpec, VersionMatch, VersionSource, WrapperSpec,
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
        // Tier P: 環境の置き場（`<キャッシュ>/virtualenvs/<名前>-<ハッシュ>-py<版>`）を聞く。
        // 中身は venv なので、分かれば activation も併用する
        env_dir_probe: Some(ProbeCmd {
            args: &["env", "info", "-p"],
            layout: VENV_LAYOUT,
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
        env_dir_probe: Some(ProbeCmd {
            args: &["--venv"],
            layout: VENV_LAYOUT,
        }),
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
        // Tier P: 一覧ファイルが無い（古い conda / 手で消した）ときだけ本体へ聞く
        list_probe: Some(ListProbe {
            program: PerOs {
                macos: "conda",
                windows: "conda.exe",
            },
            args: &["info", "--envs", "--json"],
            json_key: "envs",
        }),
    }),
    // 6. pyenv: `.python-version` は版しか決めない（依存は決めない）ので venv より後ろ
    Detector::VersionFile(VersionFileSpec {
        id: "pyenv",
        label_prefix: "pyenv ",
        files: &[".python-version"],
        skip_values: &["system"],
        root_env: PerOs::same(&["PYENV_ROOT"]),
        root_default: PerOs {
            macos: &[KnownDir::Home(&[".pyenv"])],
            windows: &[KnownDir::Home(&[".pyenv", "pyenv-win"])],
        },
        versions_dir: PerOs::same(&["versions"]),
        matching: VersionMatch::Exact,
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
        eligible_when_missing: true,
    }),
];

/// nvm の `versions/node/<版>`（Windows は nvm-windows の `<NVM_HOME>\<版>`）の中の置き場
const NVM_LAYOUT: EnvLayout = EnvLayout {
    interpreter: PerOs {
        macos: &["bin", "node"],
        windows: &["node.exe"],
    },
    path_dirs: PerOs {
        macos: &[&["bin"]],
        windows: &[&[]],
    },
    env: &[],
};

/// fnm の `node-versions/<版>` の中の置き場（macOS は fnm 1.38.1 の実測。npm / npx も同じ bin に並ぶ）
const FNM_LAYOUT: EnvLayout = EnvLayout {
    interpreter: PerOs {
        macos: &["installation", "bin", "node"],
        windows: &["installation", "node.exe"],
    },
    path_dirs: PerOs {
        macos: &[&["installation", "bin"]],
        windows: &[&["installation"]],
    },
    env: &[],
};

/// node の版を書くファイル。同じ段に両方あれば `.nvmrc`（nvm が読むのはこちらだけで、fnm も
/// 先に読む = fnm 1.38.1 の `fnm exec` で実測）
const NODE_VERSION_FILES: &[&str] = &[".nvmrc", ".node-version"];

/// node（#1735）。volta は `package.json` の `volta` をシム自身が読むので、ここでは手を出さない
/// （PATH の `node` = volta のシムが 3 で拾われ、そのまま正しい版で走る）
const NODE_DETECTORS: &[Detector] = &[
    // 1. nvm: 版を書いたファイル → 入っている版のうち**番号の前方一致で一番新しいもの**
    //    （`.nvmrc` の `20` → `v20.11.1`。nvm / fnm と同じ解き方）。`lts/*` 等の別名は解かない
    Detector::VersionFile(VersionFileSpec {
        id: "nvm",
        label_prefix: "nvm ",
        files: NODE_VERSION_FILES,
        skip_values: &["system"],
        root_env: PerOs {
            macos: &["NVM_DIR"],
            windows: &["NVM_HOME"],
        },
        // 導入スクリプトは `XDG_CONFIG_HOME` があればその下、無ければ `~/.nvm` へ入れる。
        // Windows（nvm-windows）の既定の置き場は版で違い、実機では確かめていない（`NVM_HOME` は導入時に入る）
        root_default: PerOs {
            macos: &[
                KnownDir::Var("XDG_CONFIG_HOME", &["nvm"]),
                KnownDir::Home(&[".nvm"]),
            ],
            windows: &[
                KnownDir::Var("LOCALAPPDATA", &["nvm"]),
                KnownDir::Var("APPDATA", &["nvm"]),
            ],
        },
        versions_dir: PerOs {
            macos: &["versions", "node"],
            windows: &[],
        },
        matching: VersionMatch::NumericPrefix,
        layout: NVM_LAYOUT,
    }),
    // 2. fnm: 1 と同じファイルを読み、同じ解き方をする。既定の置き場は fnm の版で変わってきたので
    //    新しい順に並べ、在る最初のものを使う（macOS の `~/.local/share/fnm` は 1.38.1 の実測。
    //    Windows の `%APPDATA%\fnm` は実機では確かめていない）
    Detector::VersionFile(VersionFileSpec {
        id: "fnm",
        label_prefix: "fnm ",
        files: NODE_VERSION_FILES,
        skip_values: &["system"],
        root_env: PerOs::same(&["FNM_DIR"]),
        root_default: PerOs {
            macos: &[
                KnownDir::Var("XDG_DATA_HOME", &["fnm"]),
                KnownDir::Home(&[".local", "share", "fnm"]),
                KnownDir::Home(&[".fnm"]),
                KnownDir::Home(&["Library", "Application Support", "fnm"]),
            ],
            windows: &[
                KnownDir::Var("APPDATA", &["fnm"]),
                KnownDir::Home(&[".fnm"]),
            ],
        },
        versions_dir: PerOs::same(&["node-versions"]),
        matching: VersionMatch::NumericPrefix,
        layout: FNM_LAYOUT,
    }),
    // 3. システム: 解くのは実行ペインのシェル（今までと同じ）。ここは表示用に PATH を覗くだけ
    Detector::PathLookup(PathLookupSpec {
        id: "system",
        file_names: PerOs {
            macos: &["node"],
            windows: &["node.exe"],
        },
        exclude_dirs: PerOs::same(&[]),
        eligible_when_missing: true,
    }),
];

/// Rust（#1735）。rustup は cwd から上の toolchain のファイルを**自分で読む**ので、この行は
/// 「どの toolchain で走るか」を一覧に見せ、利用者が別の toolchain を選んだ（保存した）ときに
/// `RUSTUP_TOOLCHAIN` で上書きする口。features は実行環境ではなく実行設定の引数（`--args`）
const RUST_DETECTORS: &[Detector] = &[Detector::EnvSwitch(EnvSwitchSpec {
    id: "rustup",
    label_prefix: "rustup ",
    // 同じ段に両方あれば rustup は旧い名前の `rust-toolchain` を使う（後方互換）ので、その順
    files: &["rust-toolchain", "rust-toolchain.toml"],
    toml_key: Some("channel"),
    var: "RUSTUP_TOOLCHAIN",
})];

/// Arduino のスケッチ（`.ino`）を compile / 書き込みする `arduino-cli` の探し方（#1950）。
/// 順は PATH → Arduino IDE 同梱 → 名前のまま（`fallback`。見つからないときの案内が自然になる）
const ARDUINO_DETECTORS: &[Detector] = &[
    // 1. PATH（`brew install arduino-cli` / 公式の導入スクリプト）。利用者が自分で入れた版を優先し、
    //    見つからなければ 2 へ譲る（Dock 起動の痩せた PATH でも、Tier P のログインシェルで見える）
    Detector::PathLookup(PathLookupSpec {
        id: "system",
        file_names: PerOs {
            macos: &["arduino-cli"],
            windows: &["arduino-cli.exe"],
        },
        exclude_dirs: PerOs::same(&[]),
        eligible_when_missing: false,
    }),
    // 2. Arduino IDE の同梱（PATH には出ない。授業で IDE を入れた人は追加の導入ゼロ）
    Detector::Bundled(BundledSpec {
        id: "arduino-ide",
        bundle: &crate::app_bundle::ARDUINO_IDE,
        file_name: PerOs {
            macos: "arduino-cli",
            windows: "arduino-cli.exe",
        },
    }),
];

/// 実行環境の表の正本
pub const KINDS: &[RuntimeKind] = &[
    RuntimeKind {
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
        // Tier P: `Python 3.12.4` の形で返る（2 系は stderr へ出すので、呼び手は両方を見る）
        version_args: &["--version"],
        detectors: PYTHON_DETECTORS,
        project_bins: &[],
    },
    // node（#1735）。TS / JSX は拡張子既定が `npx tsx`（node は TS を解さない）で `${node}` を
    // 使わないが、PATH の前置（版の bin と `node_modules/.bin`）で npx / tsx がその版の node で走る
    RuntimeKind {
        id: "node",
        extensions: &["js", "mjs", "cjs", "ts", "mts", "cts", "tsx", "jsx"],
        variable: "node",
        fallback: PerOs::same("node"),
        // 包む形の検出器を持たないので使われない（表の語として持つだけ）
        wrapped_program: "node",
        // Tier P: `v22.18.0` の形で返る
        version_args: &["--version"],
        detectors: NODE_DETECTORS,
        // `npm run` / `npx` と同じく、各段の `node_modules/.bin` をどの版を選んでも先頭へ
        project_bins: &[&["node_modules", ".bin"]],
    },
    // Rust（#1735）。走らせるのは拡張子既定の `rustc`（単体）かプロジェクト種別 `cargo` の
    // `cargo run`。どちらも実行ペインの先頭に `RUSTUP_TOOLCHAIN` が埋まる
    RuntimeKind {
        id: "rust",
        extensions: &["rs"],
        variable: "rustc",
        fallback: PerOs::same("rustc"),
        wrapped_program: "rustc",
        version_args: &["--version"],
        detectors: RUST_DETECTORS,
        project_bins: &[],
    },
    // Arduino のスケッチ（#1950）。走らせるのはプロジェクト種別 `arduino`（`runner_project` の
    // `KINDS`）で、拡張子既定の表には行を持たない（スケッチはフォルダ単位でしか走らない）。
    // 宣言（`tako:run: ${arduino_cli} compile --fqbn …`）からも同じ値を使える
    RuntimeKind {
        id: "arduino",
        extensions: &["ino"],
        variable: "arduino_cli",
        fallback: PerOs::same("arduino-cli"),
        // 包む形の検出器を持たないので使われない（表の語として持つだけ）
        wrapped_program: "arduino-cli",
        // Tier P: `arduino-cli  Version: 1.2.0 Commit: … Date: …` の形（実測 1.2.0）
        version_args: &["version"],
        detectors: ARDUINO_DETECTORS,
        project_bins: &[],
    },
];

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

    /// 組み込み既定表のこの kind の行は interpreter を `${<variable>}` で受け（#1730）、
    /// 実行環境が見つからないときは fallback（#1730 以前の既定表の先頭の語）へ展開される。
    ///
    /// interpreter を呼ばない行（TS の `npx tsx` = PATH の前置だけが効く。#1735）は変数を
    /// 使わなくてよいが、**interpreter を名前で直書きした行**（`node ${fileBase}`）は落とす
    /// （実行環境で選んだ版を素通りする）。
    ///
    /// 既定表に行を持たない拡張子（`.ino` = スケッチはフォルダ単位でしか走らない。#1950）は、
    /// プロジェクト種別が受け持っていること（でないと変数を使う既定がどこにも無い）
    #[test]
    fn 既定表の行は変数で受け実行環境が無ければfallbackへ展開される() {
        for kind in KINDS {
            for ext in kind.extensions {
                let Some(entry) = crate::platform::runner_defaults::entry(ext) else {
                    assert!(
                        crate::runner_project::KINDS
                            .iter()
                            .any(|k| k.claims(ext, "")),
                        "{ext} は既定表にもプロジェクト種別にも無い（{} の変数を使う既定が無い）",
                        kind.id
                    );
                    continue;
                };
                for p in BOTH {
                    let Some(cmd) = entry.get(p).command() else {
                        continue;
                    };
                    let var = format!("${{{}}}", kind.variable);
                    if !cmd.contains(&var) {
                        assert_ne!(
                            cmd.split_whitespace().next(),
                            Some(kind.fallback.get(p)),
                            "{ext} / {p:?}: 既定表の `{cmd}` が interpreter を名前で直書きしている（{var} で受ける）"
                        );
                        continue;
                    }
                    assert!(
                        cmd.starts_with(&format!("{var} ")),
                        "{ext} / {p:?}: 既定表の `{cmd}` が {var} で interpreter を受けていない"
                    );
                    let path = std::path::PathBuf::from(format!("/tmp/x/a.{ext}"));
                    let vars = crate::runner::RunVars::for_file(&path).on(p);
                    let expanded = crate::runner::expand_variables(cmd, &vars);
                    assert_eq!(
                        expanded.split_whitespace().next(),
                        Some(kind.fallback.get(p)),
                        "{ext} / {p:?}: 実行環境が無いときの展開 `{expanded}`"
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
                    Detector::PathLookup(_) | Detector::EnvSwitch(_) | Detector::Bundled(_) => None,
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
                    Detector::Bundled(s) => {
                        assert!(s.file_name.windows.ends_with(".exe"), "{}", s.id);
                        assert!(!s.file_name.macos.ends_with(".exe"), "{}", s.id);
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
        for kind in KINDS {
            let mut comps: Vec<&str> = kind
                .project_bins
                .iter()
                .flat_map(|c| c.iter())
                .copied()
                .collect();
            for det in kind.detectors {
                if let Detector::VersionFile(s) = det {
                    comps.extend(s.versions_dir.macos.iter().chain(s.versions_dir.windows));
                }
            }
            for c in comps {
                assert!(
                    !c.contains('/') && !c.contains('\\'),
                    "{}: 成分 `{c}`",
                    kind.id
                );
            }
        }
    }
}
