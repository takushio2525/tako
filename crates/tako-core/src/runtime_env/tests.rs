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

fn env_slash(c: &Candidate, var: &str) -> Option<String> {
    env_of(c, var).map(slash)
}

/// 区切りを `/` に揃える（Windows のホストでは `join` が `\\` を使うので、期待値を
/// `/` 区切りの 1 通りで書けるようにする。偽の置き場の名前には `\\` を使わない）
fn slash(s: &str) -> String {
    s.replace('\\', "/")
}

fn ps(v: &[PathBuf]) -> Vec<String> {
    v.iter().map(|x| slash(&x.to_string_lossy())).collect()
}

/// `${python}` に入る語の列（区切りを `/` に揃えたもの）
fn prog(c: &Candidate) -> Vec<String> {
    c.applied.program.iter().map(|w| slash(w)).collect()
}

fn loc(c: &Candidate) -> Option<String> {
    c.location.as_deref().map(|l| slash(&l.to_string_lossy()))
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
    assert_eq!(prog(c), ["/h/.local/bin/uv", "run", "python"]);
    assert_eq!(ps(&c.applied.path_prepend), ["/h/proj/.venv/bin"]);
    assert_eq!(
        env_slash(c, "VIRTUAL_ENV").as_deref(),
        Some("/h/proj/.venv")
    );
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
    assert_eq!(prog(uv), ["uv", "run", "python"]);
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
    assert_eq!(prog(c), ["/h/proj/.venv/bin/python"]);
    assert_eq!(ps(&c.applied.path_prepend), ["/h/proj/.venv/bin"]);
    assert_eq!(
        env_slash(c, "VIRTUAL_ENV").as_deref(),
        Some("/h/proj/.venv")
    );
    assert_eq!(loc(c).as_deref(), Some("/h/proj/.venv"));
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
    assert_eq!(loc(auto(&d)).as_deref(), Some("/h/mono/pkg/a/.venv"));
    // 遠い段の venv も候補に残る
    assert!(d
        .candidates
        .iter()
        .any(|c| loc(c).as_deref() == Some("/h/mono/.venv")));
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
        assert_eq!(prog(c), ["/h/.local/bin/poetry", "run", "python"]);
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
    assert_eq!(prog(c), ["/opt/homebrew/bin/pipenv", "run", "python"]);
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
    assert_eq!(prog(auto(&d))[0], "/somewhere/bin/pipenv");
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
    assert_eq!(prog(c), ["/h/miniconda3/envs/ml/bin/python"]);
    assert_eq!(ps(&c.applied.path_prepend), ["/h/miniconda3/envs/ml/bin"]);
    assert_eq!(
        env_slash(c, "CONDA_PREFIX").as_deref(),
        Some("/h/miniconda3/envs/ml")
    );
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
    assert_eq!(prog(c), ["/h/.pyenv/versions/3.11.9/bin/python"]);
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
    assert_eq!(prog(auto(&d)), ["/opt/pyenv/versions/3.11.9/bin/python"]);
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
        assert_eq!(prog(c), [name], "{platform:?}");
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
    assert_eq!(prog(c), ["/w/proj/.venv/Scripts/python.exe"]);
    assert_eq!(ps(&c.applied.path_prepend), ["/w/proj/.venv/Scripts"]);
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
    assert_eq!(prog(c), [format!("{prefix}/python.exe")]);
    assert_eq!(
        ps(&c.applied.path_prepend),
        [
            prefix.to_string(),
            format!("{prefix}/Library/mingw-w64/bin"),
            format!("{prefix}/Library/usr/bin"),
            format!("{prefix}/Library/bin"),
            format!("{prefix}/Scripts"),
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
    let v = "/w/.pyenv/pyenv-win/versions/3.11.9";
    assert_eq!(prog(c), [format!("{v}/python.exe")]);
    assert_eq!(
        ps(&c.applied.path_prepend),
        [v.to_string(), format!("{v}/Scripts")]
    );
}

#[test]
fn windowsの道具はexeを探し見つからなければ名前のまま() {
    let found = FakeFs::new()
        .file("/w/proj/uv.lock", "")
        .file("/w/.local/bin/uv.exe", "");
    let d = detect_py(WIN, &["/w/proj"], &found, &home("/w"));
    assert_eq!(prog(auto(&d))[0], "/w/.local/bin/uv.exe");

    // macOS の名前（拡張子なし）だけが置いてあっても Windows では道具と見なさない
    let wrong = FakeFs::new()
        .file("/w/proj/uv.lock", "")
        .file("/w/.local/bin/uv", "");
    let d = detect_py(WIN, &["/w/proj"], &wrong, &home("/w"));
    let uv = d.candidates.iter().find(|c| c.manager == "uv").unwrap();
    assert!(!uv.auto_eligible);
    assert_eq!(prog(uv)[0], "uv", "PowerShell には拡張子なしの名前で渡す");
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
    assert_eq!(loc(c).as_deref(), Some("/w/Python312/python.exe"));
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    // 走らせるのは名前（解くのはシェル）で、見つけた場所は表示用
    assert_eq!(prog(c), ["python"]);
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

/// `%LOCALAPPDATA%` から組んだ置き場（`/` と `\\` が混ざりうる）と、`\\` で書かれた
/// PATH の項目を同じ置き場と見る。区切りの違いで別名を実体として拾わない
#[test]
fn windowsの除外は区切りが混ざっても効く() {
    let alias = p(r"\w\AppData\Local\Microsoft\WindowsApps");
    let fs = FakeFs::new()
        .file(alias.join("python.exe"), "")
        .file("/w/Python312/python.exe", "");
    let mut env = windows_env(&["/w/Python312"]);
    env.path_dirs.insert(0, alias);
    let d = detect_py(WIN, &["/w/proj"], &fs, &env);
    assert_eq!(loc(auto(&d)).as_deref(), Some("/w/Python312/python.exe"));
}

#[test]
fn macosでは同じ綴りの置き場を外す理由が無い() {
    // 除外の置き場は Windows の列にしか無い
    let fs = FakeFs::new().file("/usr/bin/python3", "");
    let mut env = home("/h");
    env.path_dirs = dirs(&["/usr/bin"]);
    let d = detect_py(MAC, &["/h/proj"], &fs, &env);
    assert_eq!(loc(auto(&d)).as_deref(), Some("/usr/bin/python3"));
}

// ─── 保存した参照を解く ─────────────────────────────────────────────────

#[test]
fn venvの相対の鍵はルートから解き鍵はそのまま返す() {
    let fs = FakeFs::new().file("/h/proj/.venv/pyvenv.cfg", "");
    let r = RuntimeRef::new("venv", ".venv");
    let c = resolve_ref(python(), MAC, &r, Some(&p("/h/proj")), &fs, &home("/h")).unwrap();
    assert_eq!(c.key, ".venv");
    assert_eq!(prog(&c), ["/h/proj/.venv/bin/python"]);
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
    assert_eq!(prog(&c), ["/opt/py/bin/python3.13"]);
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
    assert_eq!(prog(&ok.unwrap()), ["/h/miniconda3/envs/web/bin/python"]);

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
    assert_eq!(prog(&ok.unwrap()), ["/h/.local/bin/uv", "run", "python"]);
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
    assert_eq!(prog(&c), ["python3.12"]);
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
    version_args: &[],
    detectors: &[Detector::EnvSwitch(EnvSwitchSpec {
        id: "switch",
        label_prefix: "toy ",
        files: &["toy-toolchain.toml", "toy-toolchain"],
        toml_key: Some("channel"),
        var: "TOY_TOOLCHAIN",
    })],
    project_bins: &[],
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
        assert_eq!(prog(c), ["toyc"]);
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
        prog(c),
        [slash(
            &proj
                .join("myenv")
                .join("bin")
                .join("python")
                .to_string_lossy()
        )]
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

// ─── #1730: 壊れた環境を自動で選ばない ─────────────────────────────────

#[test]
fn interpreterの消えた環境は自動で選ばず次の候補へ進む() {
    // 印（pyvenv.cfg）はあるが bin/python が無い = 壊れた .venv
    let fs = FakeFs::new()
        .file("/h/proj/.venv/pyvenv.cfg", &venv_cfg("version = 3.12.4"))
        .file("/usr/bin/python3", "");
    let env = DetectEnv {
        path_dirs: vec![p("/usr/bin")],
        ..home("/h")
    };
    let mut d = detect_py(MAC, &["/h/proj"], &fs, &env);
    // S1 の検出だけなら壊れていても選ばれる（印しか見ない）
    assert_eq!(auto(&d).manager, "venv");
    d.demote_missing_interpreters(&fs);
    let c = auto(&d);
    assert_eq!(c.manager, "system", "{:?}", d.warnings);
    assert_eq!(
        prog(c),
        ["python3"],
        "解くのは実行ペインのシェル（今と同じ）"
    );
    // 候補の一覧には残り、外した理由が warnings に載る
    assert!(d.candidates.iter().any(|x| x.manager == "venv"));
    assert!(
        d.warnings.iter().any(|w| w.contains(".venv")),
        "{:?}",
        d.warnings
    );

    // 直れば（python を置けば）自動で選ばれる
    let fixed = FakeFs::new()
        .file("/h/proj/.venv/pyvenv.cfg", &venv_cfg("version = 3.12.4"))
        .file("/h/proj/.venv/bin/python", "");
    let mut d = detect_py(MAC, &["/h/proj"], &fixed, &env);
    d.demote_missing_interpreters(&fixed);
    assert_eq!(auto(&d).manager, "venv");
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
}

#[test]
fn 素の名前の候補は実在を確かめない() {
    // 何も無い = システムの名前のまま（解くのはシェル）。確かめて外すと実行できなくなる
    let fs = FakeFs::new();
    let mut d = detect_py(WIN, &["/h/proj"], &fs, &home("/h"));
    d.demote_missing_interpreters(&fs);
    let c = auto(&d);
    assert_eq!(c.manager, "system");
    assert_eq!(prog(c), ["python"]);
}

// ─── arduino（#1950）: PATH → Arduino IDE 同梱 → 名前のまま ──────────────

fn arduino() -> &'static RuntimeKind {
    kind_by_id("arduino").expect("arduino の行がある")
}

const IDE_MAC: &str =
    "/Applications/Arduino IDE.app/Contents/Resources/app/lib/backend/resources/arduino-cli";

#[test]
fn arduino_cliはpathに無くide同梱だけならそのフルパスを選ぶ() {
    let fs = FakeFs::new().file(IDE_MAC, "");
    let env = DetectEnv {
        path_dirs: dirs(&["/usr/bin", "/bin"]),
        ..home("/Users/testuser")
    };
    let d = detect(arduino(), MAC, &[], &fs, &env);
    let c = auto(&d);
    assert_eq!(c.manager, "arduino-ide");
    assert_eq!(c.label, "Arduino IDE");
    assert_eq!(prog(c), vec![IDE_MAC.to_string()]);
    assert!(!c.needs_probe);
    // PATH の候補は残す（見つからなかったので自動では選ばない）
    let sys = d.candidates.iter().find(|c| c.manager == "system").unwrap();
    assert!(!sys.auto_eligible && sys.needs_probe);
}

#[test]
fn arduino_cliはpathにあればide同梱より先に名前のまま選ぶ() {
    let fs = FakeFs::new()
        .file(IDE_MAC, "")
        .file("/opt/homebrew/bin/arduino-cli", "");
    let env = DetectEnv {
        path_dirs: dirs(&["/opt/homebrew/bin", "/usr/bin"]),
        ..home("/Users/testuser")
    };
    let d = detect(arduino(), MAC, &[], &fs, &env);
    let c = auto(&d);
    assert_eq!(c.manager, "system");
    // 名前のまま（解くのは実行ペインのシェル = 利用者の PATH）
    assert_eq!(prog(c), vec!["arduino-cli".to_string()]);
    assert_eq!(loc(c).as_deref(), Some("/opt/homebrew/bin/arduino-cli"));
}

#[test]
fn arduino_cliがどこにも無ければ自動で選ばずfallbackの名前になる() {
    let d = detect(
        arduino(),
        MAC,
        &[],
        &FakeFs::new(),
        &home("/Users/testuser"),
    );
    assert!(d.auto_candidate().is_none(), "{d:?}");
    assert_eq!(fallback_value(arduino(), MAC), "arduino-cli");
    assert_eq!(fallback_value(arduino(), WIN), "arduino-cli");
    // Tier P はログインシェルに名前を聞く（Dock 起動の痩せた PATH の取りこぼしを拾う）
    let plan = tier_p::plan(arduino(), MAC, &d, &FakeFs::new(), &home("/h"));
    assert_eq!(plan.lookups, vec!["arduino-cli".to_string()]);
}

/// Windows の列（未実機）: `%LOCALAPPDATA%\Programs\Arduino IDE\…\arduino-cli.exe`
#[test]
fn arduino_cliのwindowsの同梱はlocalappdataから組む() {
    let local = "/Users/winuser/AppData/Local";
    let exe =
        format!("{local}/Programs/Arduino IDE/resources/app/lib/backend/resources/arduino-cli.exe");
    let fs = FakeFs::new().file(&exe, "");
    let env = DetectEnv {
        vars: [("LOCALAPPDATA".to_string(), local.to_string())].into(),
        ..home("/Users/winuser")
    };
    let d = detect(arduino(), WIN, &[], &fs, &env);
    assert_eq!(prog(auto(&d)), vec![exe.clone()]);
    // macOS の列では同じ置き場を見ない
    assert!(detect(arduino(), MAC, &[], &fs, &env)
        .auto_candidate()
        .is_none());
    // 表が名指す環境変数として集められる（`DetectEnv::from_process` が読む）
    let names = table_env_names();
    assert!(
        names.contains(&"LOCALAPPDATA") && names.contains(&"ProgramFiles"),
        "{names:?}"
    );
}

#[test]
fn 保存したide同梱の参照はファイルが消えたら解けない() {
    let fs = FakeFs::new().file(IDE_MAC, "");
    let r = RuntimeRef::new("arduino-ide", IDE_MAC);
    let c = resolve_ref(arduino(), MAC, &r, None, &fs, &home("/h")).expect("解ける");
    assert_eq!(prog(&c), vec![IDE_MAC.to_string()]);
    let err = resolve_ref(arduino(), MAC, &r, None, &FakeFs::new(), &home("/h"))
        .expect_err("消えた置き場を既定へ落とさない");
    assert_eq!(err.manager, "arduino-ide");
}

// ─── 利用者の指定 → 保存する参照（#1731） ───────────────────────────────

fn mono() -> (FakeFs, Detection) {
    let fs = FakeFs::new()
        .file("/h/mono/.venv/pyvenv.cfg", "")
        .file("/h/mono/.venv/bin/python", "")
        .file("/h/mono/pkg/a/.venv/pyvenv.cfg", "")
        .file("/h/mono/pkg/a/.venv/bin/python", "")
        .file("/h/mono/.python-version", "3.12.4\n")
        .dir("/h/.pyenv/versions/3.12.4");
    let d = detect_py(
        MAC,
        &["/h/mono/pkg/a", "/h/mono/pkg", "/h/mono"],
        &fs,
        &home("/h"),
    );
    (fs, d)
}

#[test]
fn 短縮が2候補に当たると選ばずに候補を並べる() {
    let (_, d) = mono();
    let err = match_query(python(), ".venv", &d.candidates).expect_err("1 つを選ばない");
    let QueryError::Ambiguous(ids) = &err else {
        panic!("{err:?}")
    };
    // 鍵は `join` で組んだパス（Windows のホストでは `\\` 区切り）なので `/` に揃えて比べる
    assert_eq!(
        ids.iter().map(|id| slash(id)).collect::<Vec<_>>(),
        ["venv:/h/mono/pkg/a/.venv", "venv:/h/mono/.venv"]
    );
    let msg = slash(&msg_query_error(".venv", &err, &d.candidates));
    assert!(
        msg.contains("venv:/h/mono/pkg/a/.venv") && msg.contains("venv:/h/mono/.venv"),
        "{msg}"
    );
    // 検出器 ID の短縮も同じ（venv が 2 つ）
    assert!(matches!(
        match_query(python(), "venv", &d.candidates),
        Err(QueryError::Ambiguous(ids)) if ids.len() == 2
    ));
}

#[test]
fn idと一意な短縮は参照へ写る() {
    let (_, d) = mono();
    assert_eq!(
        match_query(python(), "venv:/h/mono/.venv", &d.candidates),
        Ok(RuntimeRef::new("venv", "/h/mono/.venv"))
    );
    // 版名（pyenv の鍵）・検出器 ID（system は 1 つ）
    assert_eq!(
        match_query(python(), "3.12.4", &d.candidates),
        Ok(RuntimeRef::new("pyenv", "3.12.4"))
    );
    assert_eq!(
        match_query(python(), " system ", &d.candidates)
            .unwrap()
            .manager,
        "system"
    );
    // 候補に無くても、表の検出器 ID の形なら ID として受ける（解けるかは呼び出し側が確かめる）
    assert_eq!(
        match_query(python(), "pyenv:3.10.1", &d.candidates),
        Ok(RuntimeRef::new("pyenv", "3.10.1"))
    );
    // Windows のドライブ（`C:`）は ID と読まない
    assert_eq!(
        match_query(python(), r"C:\py\python.exe", &d.candidates),
        Err(QueryError::NotFound)
    );
    assert_eq!(
        match_query(python(), "", &d.candidates),
        Err(QueryError::NotFound)
    );
    let msg = msg_query_error("nope", &QueryError::NotFound, &d.candidates);
    assert!(msg.contains("system:"), "選べる候補を並べる: {msg}");
}

#[test]
fn パスの指定はinterpreterか印のあるディレクトリ() {
    let (fs, _) = mono();
    assert_eq!(
        ref_for_path(python(), &p("/h/mono/.venv/bin/python"), &fs),
        Some(RuntimeRef::new(
            RuntimeRef::EXPLICIT_PATH,
            "/h/mono/.venv/bin/python"
        ))
    );
    assert_eq!(
        ref_for_path(python(), &p("/h/mono/.venv"), &fs),
        Some(RuntimeRef::new("venv", "/h/mono/.venv"))
    );
    // 印の無いディレクトリ・無いパスは写さない
    assert_eq!(ref_for_path(python(), &p("/h/mono/pkg"), &fs), None);
    assert_eq!(ref_for_path(python(), &p("/h/none"), &fs), None);
}

#[test]
fn interpreterの消えた環境は保存した選択として走らせない() {
    let fs = FakeFs::new().file("/h/proj/.venv/pyvenv.cfg", "");
    let r = RuntimeRef::new("venv", "/h/proj/.venv");
    let c = resolve_ref(python(), MAC, &r, None, &fs, &home("/h")).expect("印は残っている");
    let err = ensure_runnable(c, &fs).expect_err("python が無い");
    assert_eq!(
        (err.manager.as_str(), err.key.as_str()),
        ("venv", "/h/proj/.venv")
    );
    assert!(
        slash(&err.reason).contains("/h/proj/.venv/bin/python"),
        "{}",
        err.reason
    );
    // 素の名前（シェルが解く）は確かめない
    let sys = resolve_ref(
        python(),
        MAC,
        &RuntimeRef::new("system", "python3"),
        None,
        &FakeFs::new(),
        &home("/h"),
    )
    .unwrap();
    assert!(ensure_runnable(sys, &FakeFs::new()).is_ok());
}

#[test]
fn pyenvが入っていなければ版の不足を知らせない() {
    // 道具そのものが無い人に「その版が無い」を毎回出さない（#1735。node の行は 2 つの道具が
    // 同じ版のファイルを読むので、使っていない側が騒がないための規則を全行で揃えた）
    let fs = FakeFs::new().file("/h/proj/.python-version", "3.9.1");
    let d = detect_py(MAC, &["/h/proj"], &fs, &home("/h"));
    assert_eq!(auto(&d).manager, "system");
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
}

// ─── #1735: node（nvm / fnm / node_modules/.bin / PATH の node） ───────

fn node() -> &'static RuntimeKind {
    kind_by_id("node").expect("node の行がある")
}

fn detect_node(platform: Platform, d: &[&str], fs: &FakeFs, env: &DetectEnv) -> Detection {
    detect(node(), platform, &dirs(d), fs, env)
}

/// nvm に版を並べたホーム（`/h/.nvm/versions/node/<版>/bin/node`）
fn nvm_fs(versions: &[&str]) -> FakeFs {
    versions.iter().fold(FakeFs::new(), |fs, v| {
        fs.file(format!("/h/.nvm/versions/node/{v}/bin/node"), "")
    })
}

#[test]
fn nvmrcの番号はnvmの版の前方一致で一番新しいものを選ぶ() {
    let fs = nvm_fs(&["v18.20.4", "v20.9.0", "v20.11.1", "v22.1.0"]).file("/h/proj/.nvmrc", "20\n");
    let d = detect_node(MAC, &["/h/proj/src", "/h/proj"], &fs, &home("/h"));
    let c = auto(&d);
    assert_eq!(c.manager, "nvm");
    assert_eq!(
        (c.key.as_str(), c.label.as_str(), c.version.as_deref()),
        ("v20.11.1", "nvm v20.11.1", Some("20.11.1"))
    );
    assert_eq!(prog(c), ["/h/.nvm/versions/node/v20.11.1/bin/node"]);
    assert_eq!(
        ps(&c.applied.path_prepend),
        ["/h/.nvm/versions/node/v20.11.1/bin"]
    );
    assert_eq!(c.found_in, Some(p("/h/proj")));
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    // 他の版も（自動ではないが）選べるように並ぶ。並びは選んだ版 → 古い順
    let keys: Vec<&str> = d
        .candidates
        .iter()
        .filter(|x| x.manager == "nvm")
        .map(|x| x.key.as_str())
        .collect();
    assert_eq!(keys, ["v20.11.1", "v18.20.4", "v20.9.0", "v22.1.0"]);
    assert_eq!(d.candidates.iter().filter(|x| x.auto_eligible).count(), 2);
    // 最後の砦（PATH の node = 名前のまま）も並ぶ
    assert_eq!(d.candidates.last().map(|x| x.manager), Some("system"));
}

#[test]
fn nvmの版の照合は成分ごとで先頭のvを読み飛ばす() {
    let fs = nvm_fs(&["v9.11.2", "v10.24.1", "v20.1.0", "v20.11.1"]);
    for (want, hit) in [
        ("v20.11.1", Some("v20.11.1")),
        ("20.11", Some("v20.11.1")),
        ("20.1", Some("v20.1.0")),
        ("1", None),
        ("9", Some("v9.11.2")),
        ("10", Some("v10.24.1")),
        ("v20", Some("v20.11.1")),
        ("20.x", Some("v20.11.1")),
    ] {
        let fs =
            nvm_fs(&["v9.11.2", "v10.24.1", "v20.1.0", "v20.11.1"]).file("/h/proj/.nvmrc", want);
        let d = detect_node(MAC, &["/h/proj"], &fs, &home("/h"));
        let got = d
            .auto_candidate()
            .filter(|c| c.manager == "nvm")
            .map(|c| c.key.as_str());
        assert_eq!(got, hit, ".nvmrc = {want}");
    }
    // 版の並びは数字の大小（文字列の順なら v10 が v9 より前に来る）
    let d = detect_node(MAC, &["/h/proj"], &fs, &home("/h"));
    let keys: Vec<&str> = d
        .candidates
        .iter()
        .filter(|x| x.manager == "nvm")
        .map(|x| x.key.as_str())
        .collect();
    assert_eq!(keys, ["v9.11.2", "v10.24.1", "v20.1.0", "v20.11.1"]);
}

#[test]
fn nvm_dirがあればそちらを見る() {
    let fs = FakeFs::new()
        .file("/h/proj/.nvmrc", "v20.11.1")
        .file("/opt/nvm/versions/node/v20.11.1/bin/node", "");
    let mut env = home("/h");
    env.vars.insert("NVM_DIR".into(), "/opt/nvm".into());
    let d = detect_node(MAC, &["/h/proj"], &fs, &env);
    assert_eq!(prog(auto(&d)), ["/opt/nvm/versions/node/v20.11.1/bin/node"]);
}

#[test]
fn nvmrcとnode_versionは近い段が勝ち同じ段ならnvmrcが勝つ() {
    let base = nvm_fs(&["v18.20.4", "v20.11.1", "v22.1.0"]);
    // 同じ段に両方 → .nvmrc（fnm も先に読む = 実測）
    let fs = nvm_fs(&["v18.20.4", "v20.11.1", "v22.1.0"])
        .file("/h/proj/.nvmrc", "20")
        .file("/h/proj/.node-version", "22");
    let d = detect_node(MAC, &["/h/proj"], &fs, &home("/h"));
    assert_eq!(auto(&d).key, "v20.11.1");
    // .node-version だけでも読む
    let fs = nvm_fs(&["v18.20.4", "v20.11.1", "v22.1.0"]).file("/h/proj/.node-version", "22.1.0");
    let d = detect_node(MAC, &["/h/proj"], &fs, &home("/h"));
    assert_eq!(auto(&d).key, "v22.1.0");
    // 近い段の .node-version が遠い段の .nvmrc に勝つ
    let fs = base
        .file("/h/proj/.nvmrc", "20")
        .file("/h/proj/pkg/.node-version", "18");
    let d = detect_node(MAC, &["/h/proj/pkg", "/h/proj"], &fs, &home("/h"));
    assert_eq!(auto(&d).key, "v18.20.4");
    assert_eq!(auto(&d).found_in, Some(p("/h/proj/pkg")));
}

#[test]
fn nvmrcの別名は解かずに理由を出しシステムへ進む() {
    for alias in ["lts/*", "lts/iron", "node", "stable", "default"] {
        let fs = nvm_fs(&["v20.11.1"]).file("/h/proj/.nvmrc", &format!("{alias}\n"));
        let d = detect_node(MAC, &["/h/proj"], &fs, &home("/h"));
        assert_eq!(auto(&d).manager, "system", "{alias}");
        // 入っている版は候補に並ぶが自動ではない
        let nvm: Vec<&Candidate> = d.candidates.iter().filter(|c| c.manager == "nvm").collect();
        assert_eq!(nvm.len(), 1, "{alias}");
        assert!(!nvm[0].auto_eligible, "{alias}");
        // 理由は 1 つ（fnm は入っていないので騒がない）・別名をそのまま載せる
        assert_eq!(d.warnings.len(), 1, "{alias}: {:?}", d.warnings);
        assert!(d.warnings[0].contains(alias), "{:?}", d.warnings);
    }
}

#[test]
fn nvmrcのsystemは飛ばし理由も出さない() {
    let fs = nvm_fs(&["v20.11.1"]).file("/h/proj/.nvmrc", "system");
    let d = detect_node(MAC, &["/h/proj"], &fs, &home("/h"));
    assert_eq!(auto(&d).manager, "system");
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
}

#[test]
fn 指す版が入っていなければ候補に出さず理由を出す() {
    let fs = nvm_fs(&["v20.11.1"]).file("/h/proj/.nvmrc", "18");
    let d = detect_node(MAC, &["/h/proj"], &fs, &home("/h"));
    assert_eq!(auto(&d).manager, "system");
    assert!(d
        .candidates
        .iter()
        .all(|c| c.key != "v18" && !c.key.starts_with("v18")));
    assert_eq!(d.warnings.len(), 1, "{:?}", d.warnings);
    assert!(d.warnings[0].contains("18"), "{:?}", d.warnings);
}

#[test]
fn nvmもfnmも無ければ版のファイルがあっても騒がずシステムの名前のまま() {
    let fs = FakeFs::new().file("/h/proj/.nvmrc", "20");
    for (platform, name) in [(MAC, "node"), (WIN, "node")] {
        let d = detect_node(platform, &["/h/proj"], &fs, &home("/h"));
        let c = auto(&d);
        assert_eq!((c.manager, prog(c)), ("system", vec![name.to_string()]));
        assert!(c.applied.path_prepend.is_empty() && c.applied.env.is_empty());
        assert!(d.warnings.is_empty(), "{platform:?}: {:?}", d.warnings);
        assert_eq!(fallback_value(node(), platform), name);
    }
}

#[test]
fn fnmはnode_versionsの下のinstallationを使い作業用のディレクトリを版と読まない() {
    // fnm 1.38.1 の実測の形（macOS は `~/.local/share/fnm`。`.downloads` は作業用）
    let fs = FakeFs::new()
        .file("/h/proj/.node-version", "22")
        .file(
            "/h/.local/share/fnm/node-versions/v22.18.0/installation/bin/node",
            "",
        )
        .dir("/h/.local/share/fnm/node-versions/.downloads")
        .file("/h/.local/share/fnm/node-versions/.DS_Store", "");
    let d = detect_node(MAC, &["/h/proj"], &fs, &home("/h"));
    let c = auto(&d);
    assert_eq!((c.manager, c.key.as_str()), ("fnm", "v22.18.0"));
    assert_eq!(
        prog(c),
        ["/h/.local/share/fnm/node-versions/v22.18.0/installation/bin/node"]
    );
    assert_eq!(
        ps(&c.applied.path_prepend),
        ["/h/.local/share/fnm/node-versions/v22.18.0/installation/bin"]
    );
    let fnm: Vec<&str> = d
        .candidates
        .iter()
        .filter(|x| x.manager == "fnm")
        .map(|x| x.key.as_str())
        .collect();
    assert_eq!(fnm, ["v22.18.0"], "作業用のディレクトリが版に混ざった");
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
}

#[test]
fn fnmの置き場は環境変数と在る置き場を新しい順に見る() {
    let tail = "node-versions/v20.11.1/installation/bin/node";
    // 旧い置き場（~/.fnm）しか無い
    let fs = FakeFs::new()
        .file("/h/proj/.nvmrc", "20")
        .file(format!("/h/.fnm/{tail}"), "");
    let d = detect_node(MAC, &["/h/proj"], &fs, &home("/h"));
    assert_eq!(prog(auto(&d)), [format!("/h/.fnm/{tail}")]);
    // XDG_DATA_HOME
    let fs = FakeFs::new()
        .file("/h/proj/.nvmrc", "20")
        .file(format!("/x/data/fnm/{tail}"), "")
        .file(format!("/h/.fnm/{tail}"), "");
    let mut env = home("/h");
    env.vars.insert("XDG_DATA_HOME".into(), "/x/data".into());
    let d = detect_node(MAC, &["/h/proj"], &fs, &env);
    assert_eq!(prog(auto(&d)), [format!("/x/data/fnm/{tail}")]);
    // FNM_DIR が最優先
    env.vars.insert("FNM_DIR".into(), "/opt/fnm".into());
    let fs = fs.file(format!("/opt/fnm/{tail}"), "");
    let d = detect_node(MAC, &["/h/proj"], &fs, &env);
    assert_eq!(prog(auto(&d)), [format!("/opt/fnm/{tail}")]);
}

#[test]
fn nvmとfnmの両方に版があればnvmが先() {
    let fs = nvm_fs(&["v20.11.1"]).file("/h/proj/.nvmrc", "20").file(
        "/h/.local/share/fnm/node-versions/v20.11.1/installation/bin/node",
        "",
    );
    let d = detect_node(MAC, &["/h/proj"], &fs, &home("/h"));
    let managers: Vec<&str> = d.candidates.iter().map(|c| c.manager).collect();
    assert_eq!(managers, ["nvm", "fnm", "system"]);
    assert_eq!(auto(&d).manager, "nvm");
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
}

#[test]
fn node_modulesのbinは候補ではなく近い段から全部をpathの先頭へ足す() {
    let fs = nvm_fs(&["v20.11.1"])
        .file("/h/mono/.nvmrc", "20")
        .dir("/h/mono/node_modules/.bin")
        .dir("/h/mono/pkg/a/node_modules/.bin")
        // 中間の段には無い
        .dir("/h/mono/pkg/node_modules");
    let d = detect_node(
        MAC,
        &[
            "/h/mono/pkg/a/src",
            "/h/mono/pkg/a",
            "/h/mono/pkg",
            "/h/mono",
        ],
        &fs,
        &home("/h"),
    );
    assert_eq!(
        ps(&d.project_bins),
        [
            "/h/mono/pkg/a/node_modules/.bin",
            "/h/mono/node_modules/.bin"
        ]
    );
    // 候補の当て方は版の置き場だけ（重ねるのは呼び出し側 = どの候補を選んでも効く）
    assert_eq!(auto(&d).manager, "nvm");
    assert_eq!(
        ps(&auto(&d).applied.path_prepend),
        ["/h/.nvm/versions/node/v20.11.1/bin"]
    );
    assert!(d.candidates.iter().all(|c| !c.key.contains("node_modules")));
    // 表に道具の置き場を持たない行（python）は、同じディレクトリでも足さない
    let py = detect_py(MAC, &["/h/mono"], &fs, &home("/h"));
    assert!(py.project_bins.is_empty());
}

#[test]
fn windowsのnvmはnvm_homeの直下の版のnode_exe() {
    let fs = FakeFs::new()
        .file("/w/proj/.nvmrc", "20")
        .file("/w/nvm/v20.11.1/node.exe", "")
        .file("/w/nvm/nvm.exe", "")
        .file("/w/nvm/settings.txt", "")
        .dir("/w/nvm/nodejs-cache");
    let mut env = home("/w/user");
    env.vars.insert("NVM_HOME".into(), "/w/nvm".into());
    let d = detect_node(WIN, &["/w/proj"], &fs, &env);
    let c = auto(&d);
    assert_eq!((c.manager, c.key.as_str()), ("nvm", "v20.11.1"));
    assert_eq!(prog(c), ["/w/nvm/v20.11.1/node.exe"]);
    assert_eq!(ps(&c.applied.path_prepend), ["/w/nvm/v20.11.1"]);
    assert_eq!(
        d.candidates.iter().filter(|x| x.manager == "nvm").count(),
        1,
        "版の形でないディレクトリを版と読んだ"
    );
    // macOS の列の変数（NVM_DIR）は Windows では見ない
    let mut env = home("/w/user");
    env.vars.insert("NVM_DIR".into(), "/w/nvm".into());
    let d = detect_node(WIN, &["/w/proj"], &fs, &env);
    assert_eq!(auto(&d).manager, "system");
}

#[test]
fn windowsのnvmとfnmは既定の置き場をappdataから組む() {
    let fs = FakeFs::new().file("/w/proj/.node-version", "22").file(
        "/w/roaming/fnm/node-versions/v22.1.0/installation/node.exe",
        "",
    );
    let mut env = home("/w/user");
    env.vars.insert("APPDATA".into(), "/w/roaming".into());
    let d = detect_node(WIN, &["/w/proj"], &fs, &env);
    let c = auto(&d);
    assert_eq!((c.manager, c.key.as_str()), ("fnm", "v22.1.0"));
    assert_eq!(
        prog(c),
        ["/w/roaming/fnm/node-versions/v22.1.0/installation/node.exe"]
    );
    assert_eq!(
        ps(&c.applied.path_prepend),
        ["/w/roaming/fnm/node-versions/v22.1.0/installation"]
    );
    // nvm-windows の既定の置き場（%LOCALAPPDATA%\nvm）
    let fs = FakeFs::new()
        .file("/w/proj/.nvmrc", "20")
        .file("/w/local/nvm/v20.11.1/node.exe", "");
    let mut env = home("/w/user");
    env.vars.insert("LOCALAPPDATA".into(), "/w/local".into());
    let d = detect_node(WIN, &["/w/proj"], &fs, &env);
    assert_eq!(prog(auto(&d)), ["/w/local/nvm/v20.11.1/node.exe"]);
}

#[test]
fn windowsのシステムのnodeはnode_exeを探しnode_modulesのbinも足す() {
    let fs = FakeFs::new()
        .file("/w/pf/nodejs/node.exe", "")
        .dir("/w/proj/node_modules/.bin");
    let env = DetectEnv {
        home: Some(p("/w/user")),
        path_dirs: vec![p("/w/pf/nodejs")],
        ..DetectEnv::default()
    };
    let d = detect_node(WIN, &["/w/proj"], &fs, &env);
    let c = auto(&d);
    assert_eq!((c.manager, prog(c)), ("system", vec!["node".to_string()]));
    assert_eq!(loc(c).as_deref(), Some("/w/pf/nodejs/node.exe"));
    assert_eq!(ps(&d.project_bins), ["/w/proj/node_modules/.bin"]);
}

#[test]
fn nodeの候補を保存して解き直すと同じ当て方になる() {
    let fs = nvm_fs(&["v18.20.4", "v20.11.1"])
        .file("/h/proj/.nvmrc", "20")
        .file(
            "/h/.local/share/fnm/node-versions/v22.1.0/installation/bin/node",
            "",
        );
    let env = home("/h");
    let d = detect_node(MAC, &["/h/proj"], &fs, &env);
    assert_eq!(d.candidates.len(), 4, "{:?}", d.candidates);
    for c in &d.candidates {
        let back = resolve_ref(node(), MAC, &c.reference(), None, &fs, &env)
            .unwrap_or_else(|e| panic!("{}/{} が解けない: {}", c.manager, c.key, e.reason));
        assert_eq!(back.applied, c.applied, "{}/{}", c.manager, c.key);
    }
    // 消した版は既定へ落とさずに解けない
    let err = resolve_ref(
        node(),
        MAC,
        &RuntimeRef::new("nvm", "v16.0.0"),
        None,
        &fs,
        &env,
    )
    .expect_err("入っていない");
    assert!(err.reason.contains("v16.0.0"), "{}", err.reason);
}

// ─── #1735: Rust（rust-toolchain → RUSTUP_TOOLCHAIN） ────────────────────

fn rust() -> &'static RuntimeKind {
    kind_by_id("rust").expect("rust の行がある")
}

#[test]
fn rust_toolchainのchannelをrustup_toolchainへ入れる() {
    for (file, content, want) in [
        (
            "rust-toolchain.toml",
            "[toolchain]\nchannel = \"1.79\"\ncomponents = [\"rustfmt\"]\n",
            "1.79",
        ),
        (
            "rust-toolchain",
            "nightly-2024-05-01\n",
            "nightly-2024-05-01",
        ),
        (
            "rust-toolchain",
            "[toolchain]\nchannel = \"stable\"\n",
            "stable",
        ),
    ] {
        let fs = FakeFs::new().file(format!("/h/proj/{file}"), content);
        for platform in [MAC, WIN] {
            let d = detect(
                rust(),
                platform,
                &dirs(&["/h/proj/src", "/h/proj"]),
                &fs,
                &home("/h"),
            );
            let c = auto(&d);
            assert_eq!((c.manager, c.key.as_str()), ("rustup", want), "{file}");
            assert_eq!(
                c.applied.env,
                [("RUSTUP_TOOLCHAIN".to_string(), want.to_string())],
                "{file}"
            );
            // 呼ぶのは名前のまま（rustup のプロキシが env を見て toolchain を選ぶ）
            assert_eq!(prog(c), ["rustc"]);
            assert!(c.applied.path_prepend.is_empty());
            assert_eq!(c.found_in, Some(p("/h/proj")));
            assert!(d.project_bins.is_empty());
        }
    }
}

#[test]
fn rust_toolchainが同じ段に両方あれば旧い名前を使う() {
    // rustup 1.28.2 の実測: 「both … exist. Using `rust-toolchain`」
    let fs = FakeFs::new()
        .file("/h/proj/rust-toolchain", "1.95.0\n")
        .file(
            "/h/proj/rust-toolchain.toml",
            "[toolchain]\nchannel = \"stable\"\n",
        );
    let d = detect(rust(), MAC, &dirs(&["/h/proj"]), &fs, &home("/h"));
    assert_eq!(auto(&d).key, "1.95.0");
}

#[test]
fn rust_toolchainが無いか読めなければ実行環境は無く名前のまま() {
    for fs in [
        FakeFs::new().file("/h/proj/Cargo.toml", "[package]\nname = \"a\"\n"),
        // channel の無い toolchain（`path` で指す独自の toolchain）は rustup に任せる
        FakeFs::new().file(
            "/h/proj/rust-toolchain.toml",
            "[toolchain]\npath = \"/opt/my-toolchain\"\n",
        ),
    ] {
        let d = detect(rust(), MAC, &dirs(&["/h/proj"]), &fs, &home("/h"));
        assert_eq!(d.auto, None);
        assert!(d.candidates.is_empty());
        for platform in [MAC, WIN] {
            assert_eq!(fallback_value(rust(), platform), "rustc");
        }
    }
}

#[test]
fn rustupは保存した任意のtoolchainで上書きできる() {
    // 一覧に無い toolchain も選べる（rustup が解く。入っていなければ rustup が案内する）
    let c = resolve_ref(
        rust(),
        WIN,
        &RuntimeRef::new("rustup", "nightly"),
        None,
        &FakeFs::new(),
        &home("/h"),
    )
    .expect("解ける");
    assert_eq!(
        c.applied.env,
        [("RUSTUP_TOOLCHAIN".to_string(), "nightly".to_string())]
    );
    assert_eq!(prog(&c), ["rustc"]);
}

#[test]
fn 片方の道具が版を見つけたら他方の版の不足は知らせない() {
    // nvm に v20 があり、fnm は入っているが v20 が無い（実経路で見つけた誤報の形）
    let fnm_only_22 = |fs: FakeFs| {
        fs.file(
            "/h/.local/share/fnm/node-versions/v22.18.0/installation/bin/node",
            "",
        )
    };
    let fs = fnm_only_22(nvm_fs(&["v20.11.1"]).file("/h/proj/.nvmrc", "20"));
    let d = detect_node(MAC, &["/h/proj"], &fs, &home("/h"));
    assert_eq!(
        (auto(&d).manager, auto(&d).key.as_str()),
        ("nvm", "v20.11.1")
    );
    assert!(
        d.warnings.is_empty(),
        "走るのは nvm の版なのに騒いだ: {:?}",
        d.warnings
    );
    // どちらにも無ければ、入っている道具それぞれの理由を出す
    let fs = fnm_only_22(nvm_fs(&["v18.20.4"]).file("/h/proj/.nvmrc", "20"));
    let d = detect_node(MAC, &["/h/proj"], &fs, &home("/h"));
    assert_eq!(auto(&d).manager, "system");
    assert_eq!(d.warnings.len(), 2, "{:?}", d.warnings);
    assert!(
        d.warnings.iter().any(|w| w.starts_with("nvm ")),
        "{:?}",
        d.warnings
    );
    assert!(
        d.warnings.iter().any(|w| w.starts_with("fnm ")),
        "{:?}",
        d.warnings
    );
}
