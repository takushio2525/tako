//! 実行設定の保存（#1731）の単体テスト。実ディスクは一時 dir の中だけ
//! （`handle` の書き込み先 = data dir はテストプロセスでは隔離された置き場 = #944）

use super::*;

/// 一時 dir（`.git` を置いて上への探索をここで止める = 実 HOME の印を拾わない）
fn sandbox(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "tako-1731-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join(".git")).unwrap();
    tako_core::platform::path::canonicalize(&base).unwrap()
}

fn touch(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// 偽の venv（印と interpreter の置き場だけ。中身は走らせない）
fn venv(dir: &Path) {
    touch(&dir.join("pyvenv.cfg"), "version = 3.12.4\n");
    let interp = if cfg!(windows) {
        dir.join("Scripts").join("python.exe")
    } else {
        dir.join("bin").join("python")
    };
    touch(&interp, "");
}

fn interpreter(dir: &Path) -> PathBuf {
    if cfg!(windows) {
        dir.join("Scripts").join("python.exe")
    } else {
        dir.join("bin").join("python")
    }
}

fn call(
    file: &Path,
    scope: Option<&str>,
    profile: Option<&str>,
    runtime: Option<&str>,
    reset: Option<&[String]>,
) -> Result<Value, DispatchError> {
    let head = std::fs::read_to_string(file).unwrap();
    handle(PathArgs {
        file,
        head: &head,
        scope,
        profile,
        runtime,
        reset,
    })
}

fn rr(manager: &str, key: &str) -> RuntimeRef {
    RuntimeRef::new(manager, key)
}

#[test]
fn 往復できて空の項目は保存形に出ない() {
    assert_eq!(
        serde_json::to_string(&RunConfigsFile::default()).unwrap(),
        r#"{"version":1}"#
    );
    let mut f = RunConfigsFile::default();
    let mut p = RunConfig::default();
    p.runtime.insert("k".into(), rr("venv", "/w/p/.venv"));
    f.projects.insert("/w/p".into(), p);
    f.files.insert(
        "/w/p/a.py".into(),
        RunConfig {
            profile: Some("test".into()),
            updated_at: Some("2026-10-15T00:00:00Z".into()),
            ..RunConfig::default()
        },
    );
    let text = serde_json::to_string_pretty(&f).unwrap();
    let back: RunConfigsFile = serde_json::from_str(&text).unwrap();
    assert_eq!(back, f);
    // 版数の無い古い形（手で書いた等）も今の世代として読める
    let bare: RunConfigsFile = serde_json::from_str(r#"{"files":{}}"#).unwrap();
    assert_eq!(bare.version, VERSION);
}

#[test]
fn 読めない内容は退避してから空で始める() {
    let dir = sandbox("unreadable");
    let path = dir.join(FILENAME);
    std::fs::write(&path, "{ 壊れた").unwrap();
    assert_eq!(load_from(&path), RunConfigsFile::default());
    let kept: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("unreadable"))
        .collect();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(
        std::fs::read_to_string(dir.join(&kept[0])).unwrap(),
        "{ 壊れた"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn 書き換えはresetからsetの順で空になった項目は消える() {
    let dir = sandbox("update");
    let path = dir.join(FILENAME);
    let set = Edit {
        runtime: Some((RunScope::Project, "k".into(), rr("venv", "/p/.venv"))),
        profile: Some((RunScope::File, "test".into())),
        reset_scopes: RunScope::ALL.to_vec(),
        ..Edit::default()
    };
    let (changes, store) = update_at(&path, |st| st.apply("/p/a.py", "/p", &set, "T1")).unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(store.projects["/p"].runtime["k"], rr("venv", "/p/.venv"));
    assert_eq!(store.files["/p/a.py"].profile.as_deref(), Some("test"));
    assert_eq!(store.files["/p/a.py"].updated_at.as_deref(), Some("T1"));
    // 書いたものを読み戻せる（tmp + rename の結果）
    assert_eq!(load_from(&path), store);

    // 実行環境の reset は**その kind だけ**を両方のスコープから消す（他の kind は残す）
    update_at(&path, |st| {
        st.projects
            .get_mut("/p")
            .unwrap()
            .runtime
            .insert("other".into(), rr("m", "x"));
    })
    .unwrap();
    let reset = Edit {
        reset: vec![RunConfigField::Runtime],
        reset_scopes: RunScope::ALL.to_vec(),
        kind: Some("k".into()),
        ..Edit::default()
    };
    let (changes, store) = update_at(&path, |st| st.apply("/p/a.py", "/p", &reset, "T2")).unwrap();
    assert_eq!(
        changes,
        vec![Change {
            scope: RunScope::Project,
            field: RunConfigField::Runtime,
            action: "reset",
        }]
    );
    assert_eq!(
        store.projects["/p"].runtime.keys().collect::<Vec<_>>(),
        ["other"]
    );

    // 全項目の reset で空になったエントリは消える
    let all = Edit {
        reset: RunConfigField::ALL.to_vec(),
        reset_scopes: RunScope::ALL.to_vec(),
        ..Edit::default()
    };
    let (_, store) = update_at(&path, |st| st.apply("/p/a.py", "/p", &all, "T3")).unwrap();
    assert!(
        store.files.is_empty() && store.projects.is_empty(),
        "{store:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap().trim(),
        "{\n  \"version\": 1\n}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn 保存先を指定したresetはそのスコープだけ() {
    let mut st = RunConfigsFile::default();
    for scope in RunScope::ALL {
        let edit = Edit {
            profile: Some((*scope, "x".into())),
            ..Edit::default()
        };
        st.apply("/p/a.py", "/p", &edit, "T");
    }
    let edit = Edit {
        reset: vec![RunConfigField::Profile],
        reset_scopes: vec![RunScope::File],
        ..Edit::default()
    };
    st.apply("/p/a.py", "/p", &edit, "T");
    st.tidy(FILES_MAX);
    assert!(st.files.is_empty());
    assert_eq!(st.projects["/p"].profile.as_deref(), Some("x"));
}

#[test]
fn ファイルの上限は古いものから落とす() {
    let mut st = RunConfigsFile::default();
    for (key, at) in [("/a", "T3"), ("/b", "T1"), ("/c", "T2"), ("/d", "T4")] {
        st.files.insert(
            key.into(),
            RunConfig {
                args: Some(String::new()),
                updated_at: Some(at.into()),
                ..RunConfig::default()
            },
        );
    }
    // 時刻の無いもの（手で書いた）が最も古い
    st.files.insert(
        "/e".into(),
        RunConfig {
            args: Some(String::new()),
            ..RunConfig::default()
        },
    );
    st.tidy(3);
    assert_eq!(st.files.keys().collect::<Vec<_>>(), ["/a", "/c", "/d"]);
    assert_eq!(FILES_MAX, 1_000);
}

#[test]
fn 保存値はファイルがプロジェクトより強い() {
    let mut st = RunConfigsFile::default();
    let mut p = RunConfig {
        profile: Some("proj".into()),
        ..RunConfig::default()
    };
    p.runtime.insert("k".into(), rr("venv", "/p/.venv"));
    p.runtime.insert("j".into(), rr("m", "j"));
    st.projects.insert("/p".into(), p);
    let mut f = RunConfig::default();
    f.runtime.insert("k".into(), rr("venv", "/p/sub/.venv"));
    st.files.insert("/p/sub/a.py".into(), f);

    let saved = Saved::of(&st, Path::new("/p/sub/a.py"), Path::new("/p"));
    assert_eq!(
        saved.runtime("k"),
        Some((&rr("venv", "/p/sub/.venv"), RunScope::File))
    );
    assert_eq!(saved.runtime("j"), Some((&rr("m", "j"), RunScope::Project)));
    assert_eq!(saved.runtime("none"), None);
    assert_eq!(saved.profile(), Some(("proj", RunScope::Project)));
    // 他のファイルにはファイルのスコープが効かない
    let other = Saved::of(&st, Path::new("/p/b.py"), Path::new("/p"));
    assert_eq!(
        other.runtime("k"),
        Some((&rr("venv", "/p/.venv"), RunScope::Project))
    );
}

/// `tako run-config a.py --runtime .venv` → 保存 → `Run` と同じ重ね方で選ばれる →
/// interpreter が消えたら解けない（自動へ落ちない）→ `--reset runtime` で自動へ戻る
#[test]
fn 選ぶ_重ねる_消える_戻す() {
    let base = sandbox("flow");
    let proj = base.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let file = proj.join("a.py");
    touch(&file, "print(1)\n");
    venv(&proj.join(".venv"));
    venv(&proj.join("env2"));
    let file = tako_core::platform::path::canonicalize(&file).unwrap();
    let env2 = proj.join("env2").display().to_string();

    // 読むだけ: 自動選択は .venv
    let v = call(&file, None, None, None, None).unwrap();
    assert_eq!(v["runtime"]["source"], "auto", "{v:#}");
    assert_eq!(v["runtime"]["label"], ".venv");
    assert_eq!(v["updated"], json!([]));
    assert_eq!(v["project_root"], base.display().to_string());

    // 短縮（ディレクトリ名）で env2 を選ぶ → 既定でプロジェクトへ
    let v = call(&file, None, None, Some("env2"), None).unwrap();
    assert_eq!(
        v["updated"],
        json!([{ "scope": "project", "field": "runtime", "action": "set" }])
    );
    assert_eq!(v["runtime"]["source"], "project", "{v:#}");
    assert_eq!(v["runtime"]["key"], env2);
    assert_eq!(v["config"]["runtime"]["python"]["source"], "project");
    let selected: Vec<&Value> = v["runtimes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["selected"] == true)
        .collect();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0]["key"], env2);

    // `Run` / `RunResolve` と同じ重ね方（saved_for → apply_runtime）で env2 が走る
    let saved = saved_for(&file);
    let mut snap = runtime_probe::for_run(&file).unwrap();
    apply_runtime(&mut snap, &saved).unwrap();
    assert_eq!(
        snap.words().values().next().unwrap()[0],
        interpreter(&proj.join("env2")).display().to_string()
    );

    // interpreter を消すと解けない（自動の .venv へ落とさない）
    std::fs::remove_file(interpreter(&proj.join("env2"))).unwrap();
    let mut snap = runtime_probe::for_run(&file).unwrap();
    let err = apply_runtime(&mut snap, &saved).expect_err("自動へ落とさない");
    let j = err.to_json(&file);
    assert_eq!(j["manager"], "venv");
    assert_eq!(j["source"], "project");
    assert!(
        j["next_step"].as_str().unwrap().contains("--reset runtime"),
        "{j:#}"
    );
    assert!(err.message(&file).contains("--reset runtime"));
    let v = call(&file, None, None, None, None).unwrap();
    assert_eq!(v["runtime_unresolved"]["key"], env2, "{v:#}");

    // 解けない指定は保存しない
    let e = call(&file, None, None, Some("env2"), None).expect_err("保存しない");
    assert!(matches!(e, DispatchError::InvalidParams(_)), "{e:?}");

    // `--reset runtime` で自動へ戻る
    let v = call(&file, None, None, None, Some(&["runtime".to_string()])).unwrap();
    assert_eq!(
        v["updated"],
        json!([{ "scope": "project", "field": "runtime", "action": "reset" }])
    );
    assert_eq!(v["runtime"]["source"], "auto");
    assert_eq!(v["runtime_unresolved"], Value::Null);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn 短縮が2候補に当たると保存せずに候補を並べる() {
    let base = sandbox("ambiguous");
    let pkg = base.join("pkg");
    venv(&base.join(".venv"));
    venv(&pkg.join(".venv"));
    let file = pkg.join("a.py");
    touch(&file, "print(1)\n");
    let file = tako_core::platform::path::canonicalize(&file).unwrap();

    let e = call(&file, None, None, Some(".venv"), None).expect_err("1 つを選ばない");
    let DispatchError::InvalidParams(msg) = e else {
        panic!("{e:?}")
    };
    for dir in [base.join(".venv"), pkg.join(".venv")] {
        assert!(msg.contains(&format!("venv:{}", dir.display())), "{msg}");
    }
    // 保存されていない
    assert_eq!(saved_for(&file).runtime("python"), None);
    // ID なら選べる
    let id = format!("venv:{}", base.join(".venv").display());
    let v = call(&file, None, None, Some(&id), None).unwrap();
    assert_eq!(
        v["runtime"]["key"],
        base.join(".venv").display().to_string()
    );
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn プロファイルは宣言にあるものだけ選べてファイルに保存される() {
    let base = sandbox("profile");
    let file = base.join("t.py");
    touch(
        &file,
        "# tako:run: python3 ${fileBase}\n# tako:run[test]: python3 -m pytest ${fileBase}\n",
    );
    let file = tako_core::platform::path::canonicalize(&file).unwrap();

    let e = call(&file, None, Some("nope"), None, None).expect_err("宣言に無い");
    let DispatchError::InvalidParams(msg) = e else {
        panic!("{e:?}")
    };
    assert!(msg.contains("default") && msg.contains("test"), "{msg}");

    let v = call(&file, None, Some("test"), None, None).unwrap();
    assert_eq!(
        v["updated"],
        json!([{ "scope": "file", "field": "profile", "action": "set" }])
    );
    assert_eq!(v["config"]["profile"]["value"], "test");
    assert_eq!(v["config"]["profile"]["source"], "file");
    assert_eq!(saved_for(&file).profile(), Some(("test", RunScope::File)));

    // 引数の検証（保存先・項目名）
    assert!(matches!(
        call(&file, Some("global"), Some("test"), None, None),
        Err(DispatchError::InvalidParams(_))
    ));
    assert!(matches!(
        call(&file, None, None, None, Some(&["nope".to_string()])),
        Err(DispatchError::InvalidParams(_))
    ));
    // 実行環境の無い種類のファイル
    let sh = base.join("x.txt");
    touch(&sh, "");
    let sh = tako_core::platform::path::canonicalize(&sh).unwrap();
    assert!(matches!(
        call(&sh, None, None, Some("x"), None),
        Err(DispatchError::InvalidParams(_))
    ));
    let v = call(&file, None, None, None, Some(&["all".to_string()])).unwrap();
    assert_eq!(v["config"]["profile"]["source"], "default");
    let _ = std::fs::remove_dir_all(&base);
}
