//! 実行環境の層の単体テスト（Issue #1730）。
//!
//! ファイルシステムは本物の一時 dir（事実の指紋 = mtime を本物で見るため）、
//! 子プロセスは偽の口（[`FakeProber`]）で差し替える = 実物の道具は起こさない。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::Mutex;
use std::time::Duration;

use tako_core::platform::exe::BoundedFind;
use tako_core::probe::{Outcome, TimeoutNotice};
use tako_core::project_root::{candidate_dirs, SearchBounds};
use tako_core::runtime_env::{
    self, Detector, EnvLayout, EnvValue, KnownDir, ListProbe, Marker, MarkerDirSpec,
    NamedEnvListSpec, PathLookupSpec, PerOs, ProbeCmd, RuntimeKind, VersionSource, WrapperSpec,
};
use tako_core::test_residue::ScratchDir;

use super::*;

const BUDGET: Duration = Duration::from_secs(1);

/// テストだけの表の行。道具の置き場を箱のホームの下だけにして、実機に入っている道具
/// （`/opt/homebrew/bin` の uv 等）で結果が変わらないようにする。戦略は本物の表と同じなので、
/// 「言語を足す = 行を足すだけ」で実行環境の層がそのまま働くことの検査にもなる
const TOY_ENV: EnvLayout = EnvLayout {
    interpreter: PerOs {
        macos: &["bin", "toy"],
        windows: &["Scripts", "toy.exe"],
    },
    path_dirs: PerOs {
        macos: &[&["bin"]],
        windows: &[&["Scripts"]],
    },
    env: &[("TOY_ENV", EnvValue::Location)],
};
const TOOL_DIRS: &[KnownDir] = &[KnownDir::Home(&[".toolbin"])];

static TOY: RuntimeKind = RuntimeKind {
    id: "toy",
    extensions: &["toy"],
    variable: "toy",
    fallback: PerOs {
        macos: "toy3",
        windows: "toy",
    },
    wrapped_program: "toy",
    version_args: &["--version"],
    detectors: &[
        Detector::Wrapper(WrapperSpec {
            id: "wrap",
            label: "wrap run",
            markers: &[Marker::File("wrap.lock")],
            program: PerOs {
                macos: "wrap",
                windows: "wrap.exe",
            },
            run_args: &["run"],
            program_dirs: TOOL_DIRS,
            in_project_env: None,
            env_dir_probe: None,
        }),
        Detector::MarkerDir(MarkerDirSpec {
            id: "tenv",
            names: &[".tenv"],
            scan_children: false,
            marker: "tenv.cfg",
            layout: TOY_ENV,
            version: VersionSource::KeyValue {
                file: "tenv.cfg",
                keys: &["version"],
            },
        }),
        Detector::Wrapper(WrapperSpec {
            id: "wrapenv",
            label: "wrapenv run",
            markers: &[Marker::File("wrapenv.lock")],
            program: PerOs {
                macos: "wrapenv",
                windows: "wrapenv.exe",
            },
            run_args: &["run"],
            program_dirs: TOOL_DIRS,
            in_project_env: None,
            env_dir_probe: Some(ProbeCmd {
                args: &["env", "path"],
                layout: TOY_ENV,
            }),
        }),
        Detector::NamedEnvList(NamedEnvListSpec {
            id: "named",
            label_prefix: "named: ",
            list_file: KnownDir::Home(&[".named", "envs.txt"]),
            project_files: &["named.yml"],
            name_key: "name",
            layout: TOY_ENV,
            version: VersionSource::None,
            list_probe: Some(ListProbe {
                program: PerOs {
                    macos: "namedtool",
                    windows: "namedtool.exe",
                },
                args: &["info", "--json"],
                json_key: "envs",
            }),
        }),
        Detector::PathLookup(PathLookupSpec {
            id: "system",
            file_names: PerOs {
                macos: &["toy3"],
                windows: &["toy.exe"],
            },
            exclude_dirs: PerOs {
                macos: &[],
                windows: &[],
            },
        }),
    ],
};

fn kind() -> &'static RuntimeKind {
    &TOY
}

fn ok_status() -> ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(0)
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt;
        ExitStatus::from_raw(0)
    }
}

/// 偽の問い合わせ口。答えを表から引き、呼ばれた問いを記録する
#[derive(Default)]
struct FakeProber {
    found: HashMap<String, PathBuf>,
    /// 固まる（打ち切られる）実行ファイル名
    hang: Vec<String>,
    /// (実行ファイルのパス, 引数の並び) → 標準出力
    outputs: HashMap<(String, String), String>,
    calls: Mutex<Vec<String>>,
}

impl FakeProber {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl Prober for FakeProber {
    fn find(&self, name: &str, budget: Duration) -> BoundedFind {
        self.calls.lock().unwrap().push(format!("find {name}"));
        if self.hang.iter().any(|h| h == name) {
            return BoundedFind {
                path: None,
                timeout: Some(TimeoutNotice {
                    label: name.to_string(),
                    waited_secs: budget.as_secs(),
                }),
            };
        }
        BoundedFind {
            path: self
                .found
                .get(name)
                .map(|p| p.to_string_lossy().into_owned()),
            timeout: None,
        }
    }

    fn output(
        &self,
        program: &str,
        args: &[&str],
        _cwd: Option<&Path>,
        budget: Duration,
    ) -> Outcome {
        let joined = args.join(" ");
        self.calls
            .lock()
            .unwrap()
            .push(format!("run {} {joined}", file_name(program)));
        if self.hang.iter().any(|h| h == file_name(program)) {
            return Outcome::TimedOut {
                label: tako_core::probe::label(program, args),
                waited: budget,
                stdout: Vec::new(),
                stderr: Vec::new(),
            };
        }
        match self.outputs.get(&(program.to_string(), joined)) {
            Some(out) => Outcome::Done {
                status: ok_status(),
                stdout: out.clone().into_bytes(),
                stderr: Vec::new(),
            },
            None => Outcome::Failed {
                label: tako_core::probe::label(program, args),
                reason: "no answer".into(),
            },
        }
    }
}

fn file_name(p: &str) -> &str {
    Path::new(p)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(p)
}

/// 一時 dir にホームとプロジェクトを組む箱（天井は箱の根なので実 HOME を読まない）
struct Sandbox {
    _scratch: ScratchDir,
    root: PathBuf,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let scratch = ScratchDir::new(tag);
        let root = tako_core::platform::path::canonicalize(scratch.path()).unwrap();
        Self {
            _scratch: scratch,
            root,
        }
    }

    fn write(&self, rel: &str, content: &str) -> PathBuf {
        let p = self.root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, content).unwrap();
        p
    }

    fn env(&self, path_dirs: &[&str]) -> DetectEnv {
        DetectEnv {
            home: Some(self.root.join("home")),
            vars: Default::default(),
            path_dirs: path_dirs.iter().map(|d| self.root.join(d)).collect(),
        }
    }

    fn dirs(&self, file_rel: &str) -> Vec<PathBuf> {
        let file = self.root.join(file_rel);
        candidate_dirs(
            file.parent().unwrap(),
            &SearchBounds::with_ceilings([self.root.clone()]),
        )
    }

    fn run(&self, file_rel: &str, env: &DetectEnv, facts: &mut Facts) -> Snapshot {
        detect_with(
            kind(),
            Platform::current(),
            &self.dirs(file_rel),
            &RealFs,
            env,
            facts,
            None,
        )
    }

    fn resolve(
        &self,
        file_rel: &str,
        env: &DetectEnv,
        facts: &mut Facts,
        prober: &FakeProber,
    ) -> Snapshot {
        detect_with(
            kind(),
            Platform::current(),
            &self.dirs(file_rel),
            &RealFs,
            env,
            facts,
            Some((prober, BUDGET)),
        )
    }
}

/// 表の中の、指定した戦略の最初の検出器（名前を書かずに引く）
fn venv_layout_rel() -> (String, String, String) {
    // (環境の置き場の名前, 印のファイル, interpreter の相対)
    kind()
        .detectors
        .iter()
        .find_map(|d| match d {
            Detector::MarkerDir(s) => Some((
                s.names[0].to_string(),
                s.marker.to_string(),
                s.layout.interpreter.get(Platform::current()).join("/"),
            )),
            _ => None,
        })
        .unwrap()
}

fn wrapper(with_probe: bool) -> &'static runtime_env::WrapperSpec {
    kind()
        .detectors
        .iter()
        .find_map(|d| match d {
            Detector::Wrapper(s) if s.env_dir_probe.is_some() == with_probe => Some(s),
            _ => None,
        })
        .unwrap()
}

fn marker_file(s: &runtime_env::WrapperSpec) -> &'static str {
    match s.markers[0] {
        runtime_env::Marker::File(f) => f,
        runtime_env::Marker::TomlTable { file, .. } => file,
    }
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

#[test]
fn venvがあれば設定なしでそのinterpreterとactivationで走る() {
    let s = Sandbox::new("rp-venv");
    let (venv, marker, interp) = venv_layout_rel();
    s.write(&format!("proj/{venv}/{marker}"), "version = 3.12.4\n");
    let py = s.write(&format!("proj/{venv}/{interp}"), "");
    s.write("proj/pkg/a.py", "");
    let mut facts = Facts::default();
    let snap = s.run("proj/pkg/a.py", &s.env(&[]), &mut facts);
    let c = snap.selected().expect("走らせる候補がある");
    assert_eq!(c.applied.program, [py.to_string_lossy().into_owned()]);
    assert_eq!(snap.words()[kind().variable], c.applied.program);
    assert!(!snap.path_prepend().is_empty());
    assert!(snap
        .env()
        .iter()
        .any(|(_, v)| *v == s.root.join(format!("proj/{venv}")).to_string_lossy()));
    // 子プロセスは起こしていない
    assert!(snap.tier_p_elapsed.is_none());
    assert_eq!(snap.tier_of(c), Tier::F);
    let rt = runtime_json(Some(&snap));
    assert_eq!(rt["source"], "auto");
    assert_eq!(rt["tier"], "F");
    assert_eq!(rt["version"], "3.12.4");
}

#[test]
fn venvが無ければシステムの名前のまま() {
    let s = Sandbox::new("rp-none");
    s.write("loose/a.py", "");
    let snap = s.run("loose/a.py", &s.env(&[]), &mut Facts::default());
    let c = snap.selected().unwrap();
    assert_eq!(
        c.applied.program,
        [runtime_env::fallback_value(kind(), Platform::current()).to_string()]
    );
    assert!(snap.path_prepend().is_empty() && snap.env().is_empty());
}

#[test]
fn 壊れたvenvは自動で選ばずシステムへ落ちて理由を載せる() {
    let s = Sandbox::new("rp-broken");
    let (venv, marker, _) = venv_layout_rel();
    s.write(&format!("proj/{venv}/{marker}"), "version = 3.12.4\n");
    s.write("proj/a.py", "");
    let snap = s.run("proj/a.py", &s.env(&[]), &mut Facts::default());
    let c = snap.selected().unwrap();
    assert_eq!(
        c.applied.program,
        [runtime_env::fallback_value(kind(), Platform::current()).to_string()]
    );
    let plan = runner_plan("${x} a.py");
    let w = warnings(Some(&snap), &plan);
    assert!(w.iter().any(|m| m.contains(&venv)), "{w:?}");
}

fn runner_plan(template: &str) -> RunPlan {
    RunPlan {
        profile: "default".into(),
        command: template.into(),
        template: template.into(),
        cwd: PathBuf::from("/"),
        shell: None,
        source: tako_core::RunSource::Declaration,
    }
}

/// 道具が既知の置き場にも GUI の PATH にも無い → `Run` は Tier F だけなので包む形を選ばない。
/// 一覧（Tier P）で見つかったら覚え、次の `Run` は子プロセス無しでそれを重ねる
#[test]
fn tier_pで見つけた道具を覚えてrunは子プロセス無しで重ねる() {
    let s = Sandbox::new("rp-tool");
    let w = wrapper(false);
    s.write(&format!("proj/{}", marker_file(w)), "");
    s.write("proj/a.py", "");
    let tool = s.write(&format!("tools/{}", exe(w.program.macos)), "");
    let env = s.env(&[]);
    let mut facts = Facts::default();

    let before = s.run("proj/a.py", &env, &mut facts);
    assert_ne!(
        before.selected().unwrap().manager,
        w.id,
        "Tier F だけでは選ばない"
    );

    let prober = FakeProber {
        found: HashMap::from([(w.program.macos.to_string(), tool.clone())]),
        ..FakeProber::default()
    };
    let listed = s.resolve("proj/a.py", &env, &mut facts, &prober);
    let c = listed.selected().unwrap();
    assert_eq!(c.manager, w.id);
    assert_eq!(c.applied.program[0], tool.to_string_lossy());
    assert_eq!(listed.tier_of(c), Tier::P);
    assert!(listed.tier_p_elapsed.is_some());
    assert!(prober
        .calls()
        .iter()
        .any(|c| c == &format!("find {}", w.program.macos)));

    // 覚えた事実だけで `Run` が同じ答えになる（問い合わせの口を渡していない）
    let again = s.run("proj/a.py", &env, &mut facts);
    assert_eq!(again.selected().unwrap().manager, w.id);
    assert!(again.cached);

    // 一覧をもう一度取っても、問いが同じなら聞き直さない
    let calls = prober.calls().len();
    let listed2 = s.resolve("proj/a.py", &env, &mut facts, &prober);
    assert_eq!(prober.calls().len(), calls, "{:?}", prober.calls());
    assert!(listed2.cached);
    // 字面一致（同じ事実から組むので runtimes は同じ）
    assert_eq!(runtimes_json(Some(&listed)), runtimes_json(Some(&listed2)));

    // 道具を消したら（ファイルが無い）重ねない
    std::fs::remove_file(&tool).unwrap();
    let gone = s.run("proj/a.py", &env, &mut facts);
    assert_ne!(gone.selected().unwrap().manager, w.id);
}

#[test]
fn 問い合わせが固まっても打ち切って次へ進み知らせを載せる() {
    let s = Sandbox::new("rp-hang");
    let w = wrapper(false);
    s.write(&format!("proj/{}", marker_file(w)), "");
    s.write("proj/a.py", "");
    let prober = FakeProber {
        hang: vec![w.program.macos.to_string()],
        ..FakeProber::default()
    };
    let snap = s.resolve("proj/a.py", &s.env(&[]), &mut Facts::default(), &prober);
    assert_ne!(snap.selected().unwrap().manager, w.id);
    assert_eq!(snap.timeouts.len(), 1, "{:?}", snap.timeouts);
    let probe = probe_json(Some(&snap));
    assert_eq!(probe["timeouts"][0]["label"], w.program.macos);
    let msgs = warnings(Some(&snap), &runner_plan("x"));
    assert!(
        msgs.iter().any(|m| m.contains(w.program.macos)),
        "打ち切りの知らせが warnings に無い: {msgs:?}"
    );
}

/// 包む形の環境の置き場は道具に聞き、印のファイルの mtime が変わるまで覚える
#[test]
fn 包む形の環境の置き場は印の指紋が変わるまで覚える() {
    let s = Sandbox::new("rp-envdir");
    let w = wrapper(true);
    let lock = s.write(&format!("proj/{}", marker_file(w)), "");
    s.write("proj/a.py", "");
    let tool = s.write(&format!("tools/{}", exe(w.program.macos)), "");
    let envdir = s.root.join("cache/env-1");
    std::fs::create_dir_all(&envdir).unwrap();
    let probe_args = w.env_dir_probe.unwrap().args.join(" ");
    let prober = FakeProber {
        found: HashMap::from([(w.program.macos.to_string(), tool.clone())]),
        outputs: HashMap::from([(
            (tool.to_string_lossy().into_owned(), probe_args.clone()),
            format!("{}\n", envdir.display()),
        )]),
        ..FakeProber::default()
    };
    let env = s.env(&["tools"]);
    let mut facts = Facts::default();
    let snap = s.resolve("proj/a.py", &env, &mut facts, &prober);
    let c = snap.selected().unwrap();
    assert_eq!(c.manager, w.id);
    assert_eq!(c.location.as_deref(), Some(envdir.as_path()));
    assert!(!c.applied.path_prepend.is_empty(), "activation を併用する");
    let asked = |p: &FakeProber| {
        p.calls()
            .iter()
            .filter(|c| c.ends_with(&probe_args))
            .count()
    };
    assert_eq!(asked(&prober), 1);

    // `Run` は覚えた答えを重ねる
    let run = s.run("proj/a.py", &env, &mut facts);
    assert_eq!(
        run.selected().unwrap().location.as_deref(),
        Some(envdir.as_path())
    );

    // 印を書き換えたら聞き直す（mtime の粒度に負けないよう、時刻を明示して進める）
    let later = std::time::SystemTime::now() + Duration::from_secs(5);
    std::fs::File::options()
        .write(true)
        .open(&lock)
        .unwrap()
        .set_modified(later)
        .unwrap();
    let _ = s.resolve("proj/a.py", &env, &mut facts, &prober);
    assert_eq!(asked(&prober), 2, "{:?}", prober.calls());
}

#[test]
fn 版は実在するinterpreterに聞いて覚える() {
    let s = Sandbox::new("rp-version");
    let fallback = runtime_env::fallback_value(kind(), Platform::current());
    let py = s.write(&format!("bin/{}", exe(fallback)), "");
    s.write("loose/a.py", "");
    let args = kind().version_args.join(" ");
    let prober = FakeProber {
        outputs: HashMap::from([(
            (py.to_string_lossy().into_owned(), args.clone()),
            "Python 3.12.4\n".to_string(),
        )]),
        ..FakeProber::default()
    };
    let env = s.env(&["bin"]);
    let mut facts = Facts::default();
    let snap = s.resolve("loose/a.py", &env, &mut facts, &prober);
    let c = snap.selected().unwrap();
    assert_eq!(c.version.as_deref(), Some("3.12.4"));
    assert_eq!(snap.tier_of(c), Tier::P);
    // `Run` は覚えた版を載せる（interpreter が変わっていないので）
    let run = s.run("loose/a.py", &env, &mut facts);
    assert_eq!(run.selected().unwrap().version.as_deref(), Some("3.12.4"));
}

#[test]
fn 一覧ファイルが無ければ道具に一覧を聞いて候補へ並べる() {
    let s = Sandbox::new("rp-list");
    let list = kind()
        .detectors
        .iter()
        .find_map(|d| match d {
            Detector::NamedEnvList(n) => Some(n),
            _ => None,
        })
        .unwrap();
    let probe = list.list_probe.unwrap();
    let tool = s.write(&format!("tools/{}", exe(probe.program.macos)), "");
    let prefix = s.root.join("opt/envs/ml");
    // interpreter も置く（無いと壊れた環境として自動では選ばない）
    s.write(
        &format!(
            "opt/envs/ml/{}",
            list.layout.interpreter.get(Platform::current()).join("/")
        ),
        "",
    );
    s.write(
        &format!("proj/{}", list.project_files[0]),
        &format!("{}: ml\n", list.name_key),
    );
    s.write("proj/a.py", "");
    let prober = FakeProber {
        found: HashMap::from([(probe.program.macos.to_string(), tool.clone())]),
        outputs: HashMap::from([(
            (tool.to_string_lossy().into_owned(), probe.args.join(" ")),
            serde_json::json!({ probe.json_key: [prefix.display().to_string()] }).to_string(),
        )]),
        ..FakeProber::default()
    };
    let snap = s.resolve("proj/a.py", &s.env(&[]), &mut Facts::default(), &prober);
    let c = snap.selected().unwrap();
    assert_eq!(c.manager, list.id, "{:?}", snap.detection.warnings);
    assert_eq!(c.location.as_deref(), Some(prefix.as_path()));
}

#[test]
fn 応答の形は候補と実効値を項目ごとに持つ() {
    let s = Sandbox::new("rp-json");
    let (venv, marker, interp) = venv_layout_rel();
    s.write(&format!("proj/{venv}/{marker}"), "");
    s.write(&format!("proj/{venv}/{interp}"), "");
    s.write("proj/a.py", "");
    let snap = s.run("proj/a.py", &s.env(&[]), &mut Facts::default());
    let list = runtimes_json(Some(&snap));
    let arr = list.as_array().unwrap();
    assert!(arr.len() >= 2, "venv とシステムが並ぶ: {list}");
    assert_eq!(arr.iter().filter(|c| c["auto"] == true).count(), 1);
    assert_eq!(arr[0]["selected"], true);
    for key in [
        "id", "kind", "manager", "key", "label", "version", "path", "program", "tier",
    ] {
        assert!(arr[0].get(key).is_some(), "{key} が無い: {}", arr[0]);
    }
    let eff = effective_config(Some(&snap), Some(".."));
    let cfg = config_json(&eff);
    for f in RunConfigField::ALL {
        assert!(
            cfg.get(f.as_str()).is_some(),
            "{} が無い: {cfg}",
            f.as_str()
        );
    }
    assert_eq!(cfg["cwd"]["source"], "declaration");
    assert_eq!(cfg["runtime"][kind().id]["source"], "auto");
    assert_eq!(
        cfg["runtime"][kind().id]["value"]["manager"],
        arr[0]["manager"]
    );
    let sources = config_sources_json(&eff);
    assert_eq!(sources["runtime"], "auto");
    assert_eq!(sources["args"], "default");
    // 実行環境の kind が無いとき
    assert_eq!(runtimes_json(None), serde_json::json!([]));
    assert!(runtime_json(None).is_null());
    assert_eq!(
        config_sources_json(&effective_config(None, None))["runtime"],
        "default"
    );
}

/// 包む形は、変数を使わない宣言（`tako:run: pytest`）には効かない。activation も無いなら案内する
#[test]
fn 包む形が効かない宣言には書き方を案内する() {
    let s = Sandbox::new("rp-hint");
    let w = wrapper(true);
    s.write(&format!("proj/{}", marker_file(w)), "");
    s.write("proj/a.py", "");
    s.write(&format!("tools/{}", exe(w.program.macos)), "");
    let snap = s.run("proj/a.py", &s.env(&["tools"]), &mut Facts::default());
    assert_eq!(snap.selected().unwrap().manager, w.id);
    let var = format!("${{{}}}", kind().variable);
    let raw = warnings(Some(&snap), &runner_plan("pytest -x"));
    assert!(raw.iter().any(|m| m.contains(&var)), "{raw:?}");
    let ok = warnings(Some(&snap), &runner_plan(&format!("{var} -m pytest")));
    assert!(ok.iter().all(|m| !m.contains(&var)), "{ok:?}");
}

#[test]
fn 覚える件数には上限がある() {
    let key = |i: usize| -> CacheKey { (kind().id, PathBuf::from(format!("/tako-1730-cap/{i}"))) };
    for i in 0..=CACHE_MAX {
        store_facts(key(i), Facts::default());
    }
    let len = cache().lock().unwrap().len();
    assert!(len <= CACHE_MAX, "{len}");
    assert!(
        cached_facts(&key(CACHE_MAX)).is_some(),
        "最後に入れたものは残る"
    );
}

#[test]
fn テストプロセスでは実物の道具を起こさない() {
    // 既定の口は塞いだ物（空 HOME でも道具は自分のホームを作る）
    let p = default_prober();
    assert!(p.find("sh", BUDGET).path.is_none());
    assert!(matches!(
        p.output("sh", &["-c", "true"], None, BUDGET),
        Outcome::Failed { .. }
    ));
}
