//! 実行環境の検出の単体テスト（Issue #1729。偽のファイルシステムで両 OS の列まで固定する）
//!
//! パスは両 OS とも `/` 区切りの偽の置き場で組む（macOS 上で `C:\…` を `PathBuf` にすると
//! 1 成分に化けるため）。Windows の列で見るのは**成分の並び**（`Scripts` / `python.exe`）。

use std::path::PathBuf;

use super::fake_fs::FakeFs;
use super::*;
use crate::runner_config::RuntimeRef;

const MAC: Platform = Platform::MacOs;
const WIN: Platform = Platform::Windows;

fn python() -> &'static RuntimeKind {
    kind_by_id("python").expect("python の行がある")
}

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

fn dirs(list: &[&str]) -> Vec<PathBuf> {
    list.iter().map(|s| p(s)).collect()
}

fn home(h: &str) -> DetectEnv {
    DetectEnv {
        home: Some(p(h)),
        ..DetectEnv::default()
    }
}

fn detect_py(platform: Platform, d: &[&str], fs: &FakeFs, env: &DetectEnv) -> Detection {
    detect(python(), platform, &dirs(d), fs, env)
}

fn auto(d: &Detection) -> &Candidate {
    d.auto_candidate().expect("自動で選ぶ候補がある")
}

fn venv_cfg(version_line: &str) -> String {
    format!("home = /usr/bin\ninclude-system-site-packages = false\n{version_line}\n")
}

fn env_of<'a>(c: &'a Candidate, var: &str) -> Option<&'a str> {
    c.applied
        .env
        .iter()
        .find(|(k, _)| k == var)
        .map(|(_, v)| v.as_str())
}

fn ps(v: &[PathBuf]) -> Vec<String> {
    v.iter().map(|x| x.to_string_lossy().into_owned()).collect()
}

// ─── §5.2 の順を 1 行ずつ ───────────────────────────────────────────────

#[test]
fn uvのプロジェクトはuv_runで包みvenvがあればactivationも併用する() {
    let fs = FakeFs::new()
        .file("/h/proj/uv.lock", "")
        .file(
            "/h/proj/.venv/pyvenv.cfg",
            &venv_cfg("version_info = 3.12.4"),
        )
        .file("/h/.local/bin/uv", "");
    let d = detect_py(MAC, &["/h/proj/src", "/h/proj"], &fs, &home("/h"));
    let c = auto(&d);
    assert_eq!(c.manager, "uv");
    assert_eq!(c.applied.program, ["/h/.local/bin/uv", "run", "python"]);
    assert_eq!(ps(&c.applied.path_prepend), ["/h/proj/.venv/bin"]);
    assert_eq!(env_of(c, "VIRTUAL_ENV"), Some("/h/proj/.venv"));
    assert_eq!(c.found_in, Some(p("/h/proj")));
    assert!(!c.needs_probe);
    // .venv も候補として並ぶ（自動ではないが選べる）
    assert!(d
        .candidates
        .iter()
        .any(|x| x.manager == "venv" && x.label == ".venv"));
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
}

#[test]
fn pyprojectのtool_uvの表でもuvのプロジェクトと読む() {
    for pyproject in [
        "[project]\nname = \"a\"\n\n[tool.uv]\ndev-dependencies = []\n",
        "[project]\nname = \"a\"\n[tool.uv.sources]\nfoo = { path = \"../foo\" }\n",
        "[[tool.uv.index]]  # 独自の index\nurl = \"https://example.invalid\"\n",
    ] {
        let fs = FakeFs::new()
            .file("/h/proj/pyproject.toml", pyproject)
            .file("/h/.cargo/bin/uv", "");
        let d = detect_py(MAC, &["/h/proj"], &fs, &home("/h"));
        assert_eq!(auto(&d).manager, "uv", "{pyproject}");
    }
    // 名前が前方一致するだけの別の表（`[tool.uvicorn]`）は uv ではない
    let fs = FakeFs::new()
        .file("/h/proj/pyproject.toml", "[tool.uvicorn]\nport = 8000\n")
        .file("/h/.cargo/bin/uv", "");
    let d = detect_py(MAC, &["/h/proj"], &fs, &home("/h"));
    assert!(d.candidates.iter().all(|c| c.manager != "uv"));
}

#[test]
fn uvが見つからなければ自動では選ばずvenvへ落ちる() {
    let fs = FakeFs::new()
        .file("/h/proj/uv.lock", "")
        .file("/h/proj/.venv/pyvenv.cfg", &venv_cfg("version = 3.12.4"));
    let d = detect_py(MAC, &["/h/proj"], &fs, &home("/h"));
    assert_eq!(auto(&d).manager, "venv");
    let uv = d
        .candidates
        .iter()
        .find(|c| c.manager == "uv")
        .expect("uv も候補に並ぶ");
    assert!(!uv.auto_eligible);
    assert!(uv.needs_probe, "Tier P で確かめれば選べる");
    // 見つかっていない道具は名前のまま（解くのはシェル）
    assert_eq!(uv.applied.program, ["uv", "run", "python"]);
    assert_eq!(d.warnings.len(), 1, "{:?}", d.warnings);
}

#[test]
fn venvだけならactivationで使い版をpyvenv_cfgから読む() {
    let fs = FakeFs::new().file("/h/proj/.venv/pyvenv.cfg", &venv_cfg("version = 3.12.4"));
    let d = detect_py(MAC, &["/h/proj/pkg", "/h/proj"], &fs, &home("/h"));
    let c = auto(&d);
    assert_eq!(c.manager, "venv");
    assert_eq!(c.label, ".venv");
    assert_eq!(c.version.as_deref(), Some("3.12.4"));
    assert_eq!(c.applied.program, ["/h/proj/.venv/bin/python"]);
    assert_eq!(ps(&c.applied.path_prepend), ["/h/proj/.venv/bin"]);
    assert_eq!(env_of(c, "VIRTUAL_ENV"), Some("/h/proj/.venv"));
    assert_eq!(c.location, Some(p("/h/proj/.venv")));
}

#[test]
fn venvの名前違いもpyvenv_cfgで拾う() {
    let fs = FakeFs::new()
        .file(
            "/h/proj/env38/pyvenv.cfg",
            &venv_cfg("version_info = 3.8.10.final.0"),
        )
        .dir("/h/proj/notes")
        .file("/h/proj/main.py", "");
    let d = detect_py(MAC, &["/h/proj"], &fs, &home("/h"));
    let c = auto(&d);
    assert_eq!((c.manager, c.label.as_str()), ("venv", "env38"));
    assert_eq!(c.version.as_deref(), Some("3.8.10"));
}

#[test]
fn 同じ段では名前の並びが先に効く() {
    let fs = FakeFs::new()
        .file("/h/proj/.venv/pyvenv.cfg", "")
        .file("/h/proj/venv/pyvenv.cfg", "")
        .file("/h/proj/aaa/pyvenv.cfg", "");
    let d = detect_py(MAC, &["/h/proj"], &fs, &home("/h"));
    let labels: Vec<&str> = d
        .candidates
        .iter()
        .filter(|c| c.manager == "venv")
        .map(|c| c.label.as_str())
        .collect();
    // 名前の表（.venv → venv）が先、残りは名前順
    assert_eq!(labels, [".venv", "venv", "aaa"]);
    assert_eq!(auto(&d).label, ".venv");
}

#[test]
fn monorepoでは近い段のvenvが勝つ() {
    let fs = FakeFs::new()
        .file("/h/mono/.venv/pyvenv.cfg", "")
        .file("/h/mono/pkg/a/.venv/pyvenv.cfg", "");
    let d = detect_py(
        MAC,
        &["/h/mono/pkg/a", "/h/mono/pkg", "/h/mono"],
        &fs,
        &home("/h"),
    );
    assert_eq!(auto(&d).location, Some(p("/h/mono/pkg/a/.venv")));
    // 遠い段の venv も候補に残る
    assert!(d
        .candidates
        .iter()
        .any(|c| c.location == Some(p("/h/mono/.venv"))));
}

#[test]
fn poetryは包む形で選ぶ() {
    for (marker, content) in [
        ("poetry.lock", ""),
        ("pyproject.toml", "[tool.poetry]\nname = \"a\"\n"),
    ] {
        let fs = FakeFs::new()
            .file(format!("/h/proj/{marker}"), content)
            .file("/h/.local/bin/poetry", "");
        let d = detect_py(MAC, &["/h/proj"], &fs, &home("/h"));
        let c = auto(&d);
        assert_eq!(c.manager, "poetry", "{marker}");
        assert_eq!(c.applied.program, ["/h/.local/bin/poetry", "run", "python"]);
        // 走らせるのに Tier P は要らないが、環境の置き場は道具に聞かないと分からない
        assert!(c.auto_eligible && c.needs_probe);
        assert!(c.applied.path_prepend.is_empty());
    }
}

#[test]
fn poetryでもプロジェクト内のvenvがあればvenvが先() {
    let fs = FakeFs::new()
        .file("/h/proj/poetry.lock", "")
        .file("/h/proj/.venv/pyvenv.cfg", "")
        .file("/h/.local/bin/poetry", "");
    let d = detect_py(MAC, &["/h/proj"], &fs, &home("/h"));
    assert_eq!(auto(&d).manager, "venv");
}

#[test]
fn pipenvはpipfileで包む() {
    let fs = FakeFs::new()
        .file("/h/proj/Pipfile", "[packages]\n")
        .file("/opt/homebrew/bin/pipenv", "");
    let d = detect_py(MAC, &["/h/proj"], &fs, &home("/h"));
    let c = auto(&d);
    assert_eq!(c.manager, "pipenv");
    assert_eq!(
        c.applied.program,
        ["/opt/homebrew/bin/pipenv", "run", "python"]
    );
}

#[test]
fn 道具はguiプロセスのpathからも探す() {
    let fs = FakeFs::new()
        .file("/h/proj/Pipfile", "")
        .file("/somewhere/bin/pipenv", "");
    let env = DetectEnv {
        path_dirs: vec![p("/somewhere/bin")],
        ..home("/h")
    };
    let d = detect_py(MAC, &["/h/proj"], &fs, &env);
    assert_eq!(auto(&d).applied.program[0], "/somewhere/bin/pipenv");
}

fn conda_fs(list: &str) -> FakeFs {
    let mut fs = FakeFs::new()
        .file(
            "/h/proj/environment.yml",
            "name: ml  # 学習用\nchannels:\n  - conda-forge\ndependencies:\n  - python=3.10\n",
        )
        .file("/h/.conda/environments.txt", list);
    for line in list.lines().filter(|l| !l.trim().is_empty()) {
        fs = fs.dir(line.trim());
    }
    fs.file(
        "/h/miniconda3/envs/ml/conda-meta/python-3.10.14-h123_0.json",
        "{}",
    )
    .file(
        "/h/miniconda3/envs/ml/conda-meta/numpy-1.26.4-py310_0.json",
        "{}",
    )
}

#[test]
fn condaは宣言の名前が一覧とちょうど1つ一致したときだけ選ぶ() {
    let fs = conda_fs("/h/miniconda3\n/h/miniconda3/envs/ml\n/h/miniconda3/envs/web\n");
    let d = detect_py(MAC, &["/h/proj"], &fs, &home("/h"));
    let c = auto(&d);
    assert_eq!(c.manager, "conda");
    assert_eq!((c.key.as_str(), c.label.as_str()), ("ml", "conda: ml"));
    assert_eq!(c.version.as_deref(), Some("3.10.14"));
    assert_eq!(c.applied.program, ["/h/miniconda3/envs/ml/bin/python"]);
    assert_eq!(ps(&c.applied.path_prepend), ["/h/miniconda3/envs/ml/bin"]);
    assert_eq!(env_of(c, "CONDA_PREFIX"), Some("/h/miniconda3/envs/ml"));
    assert_eq!(env_of(c, "CONDA_DEFAULT_ENV"), Some("ml"));
    // 一致しない env も選べるように並ぶ（自動ではない）
    let others: Vec<&str> = d
        .candidates
        .iter()
        .filter(|x| x.manager == "conda" && !x.auto_eligible)
        .map(|x| x.key.as_str())
        .collect();
    assert_eq!(others, ["miniconda3", "web"]);
}

#[test]
fn condaの一致が2件なら自動では選ばない() {
    let fs = conda_fs("/h/miniconda3/envs/ml\n/h/anaconda3/envs/ml\n");
    let d = detect_py(MAC, &["/h/proj"], &fs, &home("/h"));
    assert_ne!(
        auto(&d).manager,
        "conda",
        "曖昧な候補から 1 つを選ばない（#1466）"
    );
    let conda: Vec<&Candidate> = d
        .candidates
        .iter()
        .filter(|c| c.manager == "conda")
        .collect();
    assert_eq!(conda.len(), 2);
    // 名前が一意でないので prefix で覚える
    assert!(
        conda.iter().all(|c| c.key.ends_with("/envs/ml")),
        "{conda:?}"
    );
    assert_eq!(d.warnings.len(), 1, "{:?}", d.warnings);
}

#[test]
fn 一覧に無いcondaの名前はwarningsで知らせる() {
    let fs = conda_fs("/h/miniconda3/envs/web\n");
    let d = detect_py(MAC, &["/h/proj"], &fs, &home("/h"));
    assert_ne!(auto(&d).manager, "conda");
    assert_eq!(d.warnings.len(), 1, "{:?}", d.warnings);
}

#[test]
fn 一覧の中で消えたenvは候補にしない() {
    let fs = FakeFs::new()
        .file(
            "/h/.conda/environments.txt",
            "/h/gone/envs/old\n/h/miniconda3/envs/ml\n",
        )
        .dir("/h/miniconda3/envs/ml");
    let d = detect_py(MAC, &["/h/proj"], &fs, &home("/h"));
    let keys: Vec<&str> = d
        .candidates
        .iter()
        .filter(|c| c.manager == "conda")
        .map(|c| c.key.as_str())
        .collect();
    assert_eq!(keys, ["ml"]);
}

#[test]
fn pyenvはpython_versionの版をversionsから引く() {
    let fs = FakeFs::new()
        .file("/h/proj/.python-version", "3.11.9\n")
        .dir("/h/.pyenv/versions/3.11.9")
        .dir("/h/.pyenv/versions/3.12.4");
    let d = detect_py(MAC, &["/h/proj/src", "/h/proj"], &fs, &home("/h"));
    let c = auto(&d);
    assert_eq!(c.manager, "pyenv");
    assert_eq!(
        (c.key.as_str(), c.label.as_str()),
        ("3.11.9", "pyenv 3.11.9")
    );
    assert_eq!(c.applied.program, ["/h/.pyenv/versions/3.11.9/bin/python"]);
    assert_eq!(
        ps(&c.applied.path_prepend),
        ["/h/.pyenv/versions/3.11.9/bin"]
    );
    assert_eq!(c.found_in, Some(p("/h/proj")));
    let other = d
        .candidates
        .iter()
        .find(|x| x.key == "3.12.4")
        .expect("他の版も並ぶ");
    assert!(!other.auto_eligible);
}

#[test]
fn pyenvのsystemは飛ばす() {
    let fs = FakeFs::new()
        .file("/h/proj/.python-version", "system\n")
        .dir("/h/.pyenv/versions/3.12.4");
    let d = detect_py(MAC, &["/h/proj"], &fs, &home("/h"));
    assert_eq!(auto(&d).manager, "system");
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
}

#[test]
fn pyenv_rootがあればそちらを見る() {
    let fs = FakeFs::new()
        .file("/h/proj/.python-version", "3.11.9")
        .dir("/opt/pyenv/versions/3.11.9");
    let mut env = home("/h");
    env.vars.insert("PYENV_ROOT".into(), "/opt/pyenv".into());
    let d = detect_py(MAC, &["/h/proj"], &fs, &env);
    assert_eq!(
        auto(&d).applied.program,
        ["/opt/pyenv/versions/3.11.9/bin/python"]
    );
}

#[test]
fn 入っていない版はwarningsで知らせ自動では選ばない() {
    let fs = FakeFs::new()
        .file("/h/proj/.python-version", "3.9.1")
        .dir("/h/.pyenv/versions/3.12.4");
    let d = detect_py(MAC, &["/h/proj"], &fs, &home("/h"));
    assert_eq!(auto(&d).manager, "system");
    assert_eq!(d.warnings.len(), 1, "{:?}", d.warnings);
}

#[test]
fn 何も無ければシステムの名前のまま今と同じ() {
    let fs = FakeFs::new().file("/h/proj/a.py", "");
    for (platform, name) in [(MAC, "python3"), (WIN, "python")] {
        let d = detect_py(platform, &["/h/proj"], &fs, &home("/h"));
        let c = auto(&d);
        assert_eq!(c.manager, "system");
        assert_eq!(c.applied.program, [name], "{platform:?}");
        assert!(c.applied.path_prepend.is_empty() && c.applied.env.is_empty());
        assert_eq!(fallback_value(python(), platform), name);
        assert_eq!(d.candidates.len(), 1);
    }
}

#[test]
fn 優先順は表のとおりuvからシステムまで() {
    // 全部そろったプロジェクトで、候補の並びが表の検出器の順になる
    let fs = conda_fs("/h/miniconda3/envs/ml\n")
        .file("/h/proj/uv.lock", "")
        .file("/h/proj/poetry.lock", "")
        .file("/h/proj/Pipfile", "")
        .file("/h/proj/.venv/pyvenv.cfg", "")
        .file("/h/proj/.python-version", "3.11.9")
        .dir("/h/.pyenv/versions/3.11.9")
        .file("/h/.local/bin/uv", "")
        .file("/h/.local/bin/poetry", "")
        .file("/h/.local/bin/pipenv", "");
    let d = detect_py(MAC, &["/h/proj"], &fs, &home("/h"));
    let order: Vec<&str> = d
        .candidates
        .iter()
        .filter(|c| c.auto_eligible)
        .map(|c| c.manager)
        .collect();
    let table: Vec<&str> = python().detectors.iter().map(Detector::id).collect();
    assert_eq!(order, table, "候補の並びが表の順と食い違う");
    assert_eq!(auto(&d).manager, table[0]);
}

// ─── Windows の列 ───────────────────────────────────────────────────────

#[test]
fn windowsのvenvはscriptsのpython_exe() {
    let fs = FakeFs::new().file("/w/proj/.venv/pyvenv.cfg", "");
    let d = detect_py(WIN, &["/w/proj"], &fs, &home("/w"));
    let c = auto(&d);
    let venv = p("/w/proj/.venv");
    assert_eq!(
        c.applied.program,
        [venv.join("Scripts").join("python.exe").to_string_lossy()]
    );
    assert_eq!(c.applied.path_prepend, [venv.join("Scripts")]);
}

#[test]
fn windowsのcondaは5本をpathへ足す() {
    let prefix = "/w/miniconda3/envs/ml";
    let fs = FakeFs::new()
        .file("/w/proj/environment.yml", "name: ml\n")
        .file("/w/.conda/environments.txt", &format!("{prefix}\n"))
        .dir(prefix);
    let d = detect_py(WIN, &["/w/proj"], &fs, &home("/w"));
    let c = auto(&d);
    let base = p(prefix);
    assert_eq!(
        c.applied.program,
        [base.join("python.exe").to_string_lossy()]
    );
    assert_eq!(
        c.applied.path_prepend,
        [
            base.clone(),
            base.join("Library").join("mingw-w64").join("bin"),
            base.join("Library").join("usr").join("bin"),
            base.join("Library").join("bin"),
            base.join("Scripts"),
        ]
    );
}

#[test]
fn windowsのpyenvはpyenv_winの置き場() {
    let fs = FakeFs::new()
        .file("/w/proj/.python-version", "3.11.9\n")
        .dir("/w/.pyenv/pyenv-win/versions/3.11.9");
    let d = detect_py(WIN, &["/w/proj"], &fs, &home("/w"));
    let c = auto(&d);
    let v = p("/w/.pyenv/pyenv-win/versions/3.11.9");
    assert_eq!(c.applied.program, [v.join("python.exe").to_string_lossy()]);
    assert_eq!(c.applied.path_prepend, [v.clone(), v.join("Scripts")]);
}

#[test]
fn windowsの道具はexeを探し見つからなければ名前のまま() {
    let found = FakeFs::new()
        .file("/w/proj/uv.lock", "")
        .file("/w/.local/bin/uv.exe", "");
    let d = detect_py(WIN, &["/w/proj"], &found, &home("/w"));
    assert_eq!(
        auto(&d).applied.program[0],
        p("/w/.local/bin").join("uv.exe").to_string_lossy()
    );

    // macOS の名前（拡張子なし）だけが置いてあっても Windows では道具と見なさない
    let wrong = FakeFs::new()
        .file("/w/proj/uv.lock", "")
        .file("/w/.local/bin/uv", "");
    let d = detect_py(WIN, &["/w/proj"], &wrong, &home("/w"));
    let uv = d.candidates.iter().find(|c| c.manager == "uv").unwrap();
    assert!(!uv.auto_eligible);
    assert_eq!(
        uv.applied.program[0], "uv",
        "PowerShell には拡張子なしの名前で渡す"
    );
}

#[test]
fn windowsの道具はappdataの置き場も見る() {
    let fs = FakeFs::new()
        .file("/w/proj/poetry.lock", "")
        .file("/w/AppData/Roaming/Python/Scripts/poetry.exe", "");
    let mut env = home("/w");
    env.vars
        .insert("APPDATA".into(), "/w/AppData/Roaming".into());
    let d = detect_py(WIN, &["/w/proj"], &fs, &env);
    assert_eq!(auto(&d).manager, "poetry");
}

fn windows_env(path_dirs: &[&str]) -> DetectEnv {
    let mut env = home("/w");
    env.vars
        .insert("LOCALAPPDATA".into(), "/w/AppData/Local".into());
    env.path_dirs = dirs(path_dirs);
    env
}

#[test]
fn windowsのapp_execution_aliasは実体として扱わない() {
    let fs = FakeFs::new()
        .file("/w/AppData/Local/Microsoft/WindowsApps/python.exe", "")
        .file("/w/Python312/python.exe", "");
    let env = windows_env(&["/w/AppData/Local/Microsoft/WindowsApps", "/w/Python312"]);
    let d = detect_py(WIN, &["/w/proj"], &fs, &env);
    let c = auto(&d);
    assert_eq!(c.manager, "system");
    assert_eq!(c.location, Some(p("/w/Python312/python.exe")));
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    // 走らせるのは名前（解くのはシェル）で、見つけた場所は表示用
    assert_eq!(c.applied.program, ["python"]);
}

#[test]
fn windowsで別名しか無ければ場所を持たずwarningsで知らせる() {
    let fs = FakeFs::new().file("/w/AppData/Local/Microsoft/WindowsApps/python.exe", "");
    // 大文字小文字だけ違う綴りでも同じ置き場として外す（Windows のファイルシステムは区別しない）
    let env = windows_env(&["/W/APPDATA/Local/Microsoft/WindowsApps"]);
    let fs = fs.file("/W/APPDATA/Local/Microsoft/WindowsApps/python.exe", "");
    let d = detect_py(WIN, &["/w/proj"], &fs, &env);
    let c = auto(&d);
    assert_eq!(c.location, None);
    assert!(c.needs_probe);
    assert_eq!(d.warnings.len(), 1, "{:?}", d.warnings);
}

#[test]
fn macosでは同じ綴りの置き場を外す理由が無い() {
    // 除外の置き場は Windows の列にしか無い
    let fs = FakeFs::new().file("/usr/bin/python3", "");
    let mut env = home("/h");
    env.path_dirs = dirs(&["/usr/bin"]);
    let d = detect_py(MAC, &["/h/proj"], &fs, &env);
    assert_eq!(auto(&d).location, Some(p("/usr/bin/python3")));
}

// ─── 保存した参照を解く ─────────────────────────────────────────────────

#[test]
fn venvの相対の鍵はルートから解き鍵はそのまま返す() {
    let fs = FakeFs::new().file("/h/proj/.venv/pyvenv.cfg", "");
    let r = RuntimeRef::new("venv", ".venv");
    let c = resolve_ref(python(), MAC, &r, Some(&p("/h/proj")), &fs, &home("/h")).unwrap();
    assert_eq!(c.key, ".venv");
    assert_eq!(c.applied.program, ["/h/proj/.venv/bin/python"]);
    assert_eq!(c.reference(), r);
}

#[test]
fn 消えたvenvは既定へ落とさずに解けない() {
    let fs = FakeFs::new().file("/h/proj/a.py", "");
    let r = RuntimeRef::new("venv", ".venv");
    let e = resolve_ref(python(), MAC, &r, Some(&p("/h/proj")), &fs, &home("/h")).unwrap_err();
    assert_eq!((e.manager.as_str(), e.key.as_str()), ("venv", ".venv"));
    assert!(!e.reason.is_empty());
}

#[test]
fn 表に無い種類は解けない() {
    let fs = FakeFs::new();
    let r = RuntimeRef::new("no-such-manager", "x");
    assert!(resolve_ref(python(), MAC, &r, None, &fs, &home("/h")).is_err());
}

#[test]
fn 指定したinterpreterはその隣をpathへ足す() {
    let fs = FakeFs::new().file("/opt/py/bin/python3.13", "");
    let r = RuntimeRef::new(RuntimeRef::EXPLICIT_PATH, "/opt/py/bin/python3.13");
    let c = resolve_ref(python(), MAC, &r, None, &fs, &home("/h")).unwrap();
    assert_eq!(c.applied.program, ["/opt/py/bin/python3.13"]);
    assert_eq!(ps(&c.applied.path_prepend), ["/opt/py/bin"]);

    let gone = RuntimeRef::new(RuntimeRef::EXPLICIT_PATH, "/opt/py/bin/python9");
    assert!(resolve_ref(python(), MAC, &gone, None, &fs, &home("/h")).is_err());
}

#[test]
fn condaは名前で一意なら解け曖昧なら解けない() {
    let fs = conda_fs("/h/miniconda3/envs/ml\n/h/miniconda3/envs/web\n");
    let ok = resolve_ref(
        python(),
        MAC,
        &RuntimeRef::new("conda", "web"),
        None,
        &fs,
        &home("/h"),
    );
    assert_eq!(
        ok.unwrap().applied.program,
        ["/h/miniconda3/envs/web/bin/python"]
    );

    let fs = conda_fs("/h/miniconda3/envs/ml\n/h/anaconda3/envs/ml\n");
    let e = resolve_ref(
        python(),
        MAC,
        &RuntimeRef::new("conda", "ml"),
        None,
        &fs,
        &home("/h"),
    );
    assert!(e.is_err(), "曖昧な名前から 1 つを選ばない（#1466）");
    // prefix の鍵なら一意に解ける
    let by_prefix = RuntimeRef::new("conda", "/h/anaconda3/envs/ml");
    let c = resolve_ref(python(), MAC, &by_prefix, None, &fs, &home("/h")).unwrap();
    assert_eq!(c.key, "/h/anaconda3/envs/ml");
    assert_eq!(env_of(&c, "CONDA_DEFAULT_ENV"), Some("ml"));
}

#[test]
fn pyenvは版が無ければ解けない() {
    let fs = FakeFs::new().dir("/h/.pyenv/versions/3.12.4");
    let ok = resolve_ref(
        python(),
        MAC,
        &RuntimeRef::new("pyenv", "3.12.4"),
        None,
        &fs,
        &home("/h"),
    );
    assert!(ok.is_ok());
    let ng = resolve_ref(
        python(),
        MAC,
        &RuntimeRef::new("pyenv", "3.9.1"),
        None,
        &fs,
        &home("/h"),
    );
    assert!(ng.is_err());
}

#[test]
fn uvは印の消えたディレクトリでは解けない() {
    let fs = FakeFs::new()
        .file("/h/proj/uv.lock", "")
        .file("/h/.local/bin/uv", "");
    let ok = resolve_ref(
        python(),
        MAC,
        &RuntimeRef::new("uv", "/h/proj"),
        None,
        &fs,
        &home("/h"),
    );
    assert_eq!(
        ok.unwrap().applied.program,
        ["/h/.local/bin/uv", "run", "python"]
    );
    let ng = resolve_ref(
        python(),
        MAC,
        &RuntimeRef::new("uv", "/h/other"),
        None,
        &fs,
        &home("/h"),
    );
    assert!(ng.is_err());
}

#[test]
fn システムは名前をそのまま使う() {
    let fs = FakeFs::new();
    let c = resolve_ref(
        python(),
        MAC,
        &RuntimeRef::new("system", "python3.12"),
        None,
        &fs,
        &home("/h"),
    )
    .unwrap();
    assert_eq!(c.applied.program, ["python3.12"]);
}

#[test]
fn 候補を保存して解き直すと同じ当て方になる() {
    let fs = conda_fs("/h/miniconda3/envs/ml\n")
        .file("/h/proj/uv.lock", "")
        .file("/h/proj/.venv/pyvenv.cfg", "")
        .file("/h/proj/.python-version", "3.11.9")
        .dir("/h/.pyenv/versions/3.11.9")
        .file("/h/.local/bin/uv", "");
    let env = home("/h");
    let d = detect_py(MAC, &["/h/proj"], &fs, &env);
    assert!(d.candidates.len() >= 5);
    for c in &d.candidates {
        let back = resolve_ref(python(), MAC, &c.reference(), None, &fs, &env)
            .unwrap_or_else(|e| panic!("{}/{} が解けない: {}", c.manager, c.key, e.reason));
        assert_eq!(back.applied, c.applied, "{}/{}", c.manager, c.key);
    }
}

// ─── 表と入口 ───────────────────────────────────────────────────────────

/// 表に無い言語（テストだけの行）でも同じ戦略が働く = 「言語を足す = 行を足すだけ」
const TOOLCHAIN_KIND: RuntimeKind = RuntimeKind {
    id: "toy",
    extensions: &["toy"],
    variable: "toy",
    fallback: PerOs::same("toyc"),
    wrapped_program: "toyc",
    detectors: &[Detector::EnvSwitch(EnvSwitchSpec {
        id: "switch",
        label_prefix: "toy ",
        files: &["toy-toolchain.toml", "toy-toolchain"],
        toml_key: Some("channel"),
        var: "TOY_TOOLCHAIN",
    })],
};

#[test]
fn 環境変数で切り替える戦略はtomlのキーと素の1行の両方を読む() {
    for (file, content) in [
        ("toy-toolchain.toml", "[toolchain]\nchannel = \"1.80\"\n"),
        ("toy-toolchain", "1.80\n"),
    ] {
        let fs = FakeFs::new().file(format!("/h/proj/{file}"), content);
        let d = detect(&TOOLCHAIN_KIND, MAC, &dirs(&["/h/proj"]), &fs, &home("/h"));
        let c = auto(&d);
        assert_eq!(
            c.applied.env,
            [("TOY_TOOLCHAIN".to_string(), "1.80".to_string())],
            "{file}"
        );
        assert_eq!(c.applied.program, ["toyc"]);
    }
    let none = detect(
        &TOOLCHAIN_KIND,
        MAC,
        &dirs(&["/h/proj"]),
        &FakeFs::new(),
        &home("/h"),
    );
    assert_eq!(none.auto, None);
    assert_eq!(fallback_value(&TOOLCHAIN_KIND, WIN), "toyc");
}

#[test]
fn 拡張子とidから行を引ける() {
    assert_eq!(kind_for_ext("PY").map(|k| k.id), Some("python"));
    assert!(kind_for_ext("zzz").is_none());
    assert_eq!(kind_by_id("python").map(|k| k.variable), Some("python"));
}

#[test]
fn 保存できるmanagerは表の検出器と指定のパス() {
    let ids = python().manager_ids();
    for det in python().detectors {
        assert!(ids.contains(&det.id()));
    }
    assert!(ids.contains(&RuntimeRef::EXPLICIT_PATH));
    assert_eq!(ids.len(), python().detectors.len() + 1);
}

#[test]
fn 読む環境変数は表から集まる() {
    let names = table_env_names();
    for want in ["PYENV_ROOT", "LOCALAPPDATA", "APPDATA"] {
        assert!(names.contains(&want), "{want} が {names:?} に無い");
    }
    let mut sorted = names.clone();
    sorted.dedup();
    assert_eq!(sorted, names, "重複がある");
}

#[test]
fn 子の走査は1段あたりの上限で止まる() {
    // 名前順で上限より後ろにある venv は拾わない（巨大なディレクトリで stat が膨らまない）
    let mut fs = FakeFs::new();
    for i in 0..SCAN_CHILDREN_MAX {
        fs = fs.dir(format!("/h/big/d{i:04}"));
    }
    let fs = fs.file("/h/big/zzz-env/pyvenv.cfg", "");
    let d = detect_py(MAC, &["/h/big"], &fs, &home("/h"));
    assert!(d.candidates.iter().all(|c| c.manager != "venv"));
}

#[test]
fn 大きなpyprojectは先頭だけを読む() {
    // 64 KiB より後ろの表は見ない（上限つきの先頭読み）
    let filler = "# x\n".repeat(20 * 1024);
    let fs = FakeFs::new()
        .file("/h/proj/pyproject.toml", &format!("{filler}[tool.uv]\n"))
        .file("/h/.local/bin/uv", "");
    let d = detect_py(MAC, &["/h/proj"], &fs, &home("/h"));
    assert!(d.candidates.iter().all(|c| c.manager != "uv"));
}

// ─── 本物のファイルシステム ─────────────────────────────────────────────

/// 偽の実装だけでなく、本物の stat / 先頭読み / 一覧でも同じ答えになる
#[test]
fn 本物のファイルシステムでも同じ順で選ぶ() {
    let scratch = crate::test_residue::ScratchDir::new("runtime-env-1729");
    let root = scratch.path();
    let proj = root.join("proj");
    let write = |rel: &str, content: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
    };
    write("proj/pyproject.toml", "[tool.poetry]\nname = \"a\"\n");
    write("proj/myenv/pyvenv.cfg", "version = 3.12.4\n");
    write("home/.local/bin/poetry", "");
    let env = DetectEnv {
        home: Some(root.join("home")),
        ..DetectEnv::default()
    };

    let d = detect(python(), MAC, std::slice::from_ref(&proj), &RealFs, &env);
    // 名前違いの venv を子の走査で拾い、poetry より先に選ぶ
    let c = auto(&d);
    assert_eq!((c.manager, c.label.as_str()), ("venv", "myenv"));
    assert_eq!(c.version.as_deref(), Some("3.12.4"));
    assert_eq!(
        c.applied.program,
        [proj
            .join("myenv")
            .join("bin")
            .join("python")
            .to_string_lossy()]
    );
    let poetry = d.candidates.iter().find(|x| x.manager == "poetry").unwrap();
    assert!(
        poetry.auto_eligible,
        "既知の置き場の道具を本物の stat で見つける"
    );

    // 先頭読みは上限で切れ、無いディレクトリの一覧は空
    assert_eq!(
        RealFs
            .read_head(&proj.join("myenv/pyvenv.cfg"), 7)
            .as_deref(),
        Some("version")
    );
    assert!(RealFs.list_dir(&root.join("no-such-dir")).is_empty());
    assert!(RealFs.read_head(&root.join("no-such-file"), 10).is_none());
}

#[test]
fn 似た名前を版や宣言と読み違えない() {
    // conda-meta の `python-dateutil-…` は python の版ではない / `names:` は `name:` ではない
    let fs = FakeFs::new()
        .file(
            "/h/proj/environment.yml",
            "names: other\n  name: nested\nname: 'ml'\n",
        )
        .file("/h/.conda/environments.txt", "/h/mc/envs/ml\n")
        .dir("/h/mc/envs/ml")
        .file(
            "/h/mc/envs/ml/conda-meta/python-dateutil-2.9.0-py_0.json",
            "{}",
        )
        .file("/h/mc/envs/ml/conda-meta/python-3.11.9-h1_0.json", "{}");
    let d = detect_py(MAC, &["/h/proj"], &fs, &home("/h"));
    let c = auto(&d);
    assert_eq!((c.manager, c.key.as_str()), ("conda", "ml"));
    assert_eq!(c.version.as_deref(), Some("3.11.9"));
}
