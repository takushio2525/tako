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
        ..PathArgs::empty(file)
    })
}

/// #1732: 引数・環境変数・作業ディレクトリ・実行前コマンドだけを渡す
fn call_1732<'a>(
    file: &'a Path,
    scope: Option<&'a str>,
    edit: impl FnOnce(&mut PathArgs<'a>),
) -> Result<Value, DispatchError> {
    let mut a = PathArgs {
        scope,
        ..PathArgs::empty(file)
    };
    edit(&mut a);
    let head = std::fs::read_to_string(file).unwrap();
    handle(PathArgs { head: &head, ..a })
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
    assert_eq!(v["scope_root"], base.display().to_string());

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

/// 一覧の外の選択（interpreter のパスを直接指定）も保存でき、`runtime` にだけ載って選ばれる
#[test]
fn interpreterのパスを直接選べる() {
    let base = sandbox("explicit");
    let file = base.join("a.py");
    touch(&file, "print(1)\n");
    let elsewhere = base.join("tools").join("py");
    venv(&elsewhere);
    let interp = interpreter(&elsewhere);
    let file = tako_core::platform::path::canonicalize(&file).unwrap();

    // ファイルの段からの相対パスでも受ける（`./` は落とす）
    let rel = format!("./{}", interp.strip_prefix(&base).unwrap().display());
    let v = call(&file, Some("file"), None, Some(&rel), None).unwrap();
    assert_eq!(
        v["updated"],
        json!([{ "scope": "file", "field": "runtime", "action": "set" }])
    );
    assert_eq!(v["runtime"]["manager"], RuntimeRef::EXPLICIT_PATH, "{v:#}");
    assert_eq!(v["runtime"]["key"], interp.display().to_string());
    assert_eq!(v["runtime"]["source"], "file");
    assert_eq!(v["runtime"]["selected"], true);
    // 一覧の候補は自動選択の順のまま（指定のパスは並ばない）で、どれも選ばれていない
    assert!(v["runtimes"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["selected"] == false));

    // 無いパスは保存しない
    assert!(matches!(
        call(&file, None, None, Some("./nowhere/python"), None),
        Err(DispatchError::InvalidParams(_))
    ));
    let _ = std::fs::remove_dir_all(&base);
}

// ─── #1732: 引数・環境変数・作業ディレクトリ・実行前コマンド ─────────────

fn env_of(pairs: &[(&str, Option<&str>)]) -> BTreeMap<String, Option<String>> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.map(str::to_string)))
        .collect()
}

/// 保存先の既定はファイル・応答の config に出典つきで載る・`Saved::layer` が解決へ渡す形になる
#[test]
fn 引数と環境変数と作業ディレクトリと実行前を保存し既定はファイル() {
    let base = sandbox("s4-save");
    let file = base.join("proj").join("a.py");
    touch(&file, "print(1)\n");
    std::fs::create_dir_all(base.join("proj").join("build")).unwrap();
    let env = env_of(&[("APP_ENV", Some("dev")), ("NOTE", Some("it's $HOME 日本"))]);
    let v = call_1732(&file, None, |a| {
        a.args = Some("--name 'a b' x");
        a.env = Some(&env);
        a.cwd = Some("build");
        a.before = Some("make build");
    })
    .unwrap();
    let updated: Vec<(String, String)> = v["updated"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["field"].as_str().unwrap().to_string(),
                format!(
                    "{}:{}",
                    c["scope"].as_str().unwrap(),
                    c["action"].as_str().unwrap()
                ),
            )
        })
        .collect();
    for field in ["args", "cwd", "before", "env"] {
        assert!(
            updated.contains(&(field.to_string(), "file:set".to_string())),
            "{field}: {updated:?}"
        );
    }
    assert_eq!(
        v["config"]["args"],
        json!({ "value": "--name 'a b' x", "source": "file" })
    );
    assert_eq!(v["config"]["cwd"]["source"], "file");
    assert_eq!(v["config"]["before"]["value"], "make build");
    assert_eq!(
        v["config"]["env"]["NOTE"],
        json!({ "value": "it's $HOME 日本", "source": "file" })
    );
    assert_eq!(v["saved"]["file"]["env"]["APP_ENV"], "dev", "{v:#}");

    let (layer, bad) = saved_for(&file).layer();
    assert!(bad.is_empty());
    assert_eq!(layer.args.as_deref(), Some("--name 'a b' x"));
    assert_eq!(layer.cwd.as_deref(), Some("build"));
    assert_eq!(layer.before.as_deref(), Some("make build"));
    assert_eq!(
        layer.env.get("NOTE").map(String::as_str),
        Some("it's $HOME 日本")
    );

    // 読むだけの呼び出しは何も書き換えない
    let v = call_1732(&file, None, |_| {}).unwrap();
    assert_eq!(v["updated"], json!([]));
    let _ = std::fs::remove_dir_all(&base);
}

/// 環境変数はキー単位で重なる（ファイルが勝つ）。値 null は保存先を省くと両方から消す
#[test]
fn 環境変数はキー単位で重なりnullで両方から消える() {
    let base = sandbox("s4-env");
    let file = base.join("a.py");
    touch(&file, "print(1)\n");
    let p = env_of(&[("A", Some("project")), ("B", Some("project"))]);
    call_1732(&file, Some("project"), |a| a.env = Some(&p)).unwrap();
    let f = env_of(&[("A", Some("file"))]);
    call_1732(&file, None, |a| a.env = Some(&f)).unwrap();

    let (layer, _) = saved_for(&file).layer();
    assert_eq!(layer.env.get("A").map(String::as_str), Some("file"));
    assert_eq!(layer.env.get("B").map(String::as_str), Some("project"));

    let unset = env_of(&[("A", None)]);
    let v = call_1732(&file, None, |a| a.env = Some(&unset)).unwrap();
    assert_eq!(
        v["updated"],
        json!([
            { "scope": "file", "field": "env", "action": "reset" },
            { "scope": "project", "field": "env", "action": "reset" },
        ]),
        "{v:#}"
    );
    let (layer, _) = saved_for(&file).layer();
    assert_eq!(layer.env.get("A"), None);
    assert_eq!(layer.env.get("B").map(String::as_str), Some("project"));
    // 無い名前を消しても何も変わらない
    let none = env_of(&[("ZZZ", None)]);
    let v = call_1732(&file, None, |a| a.env = Some(&none)).unwrap();
    assert_eq!(v["updated"], json!([]));
    let _ = std::fs::remove_dir_all(&base);
}

/// 識別子でない名前は保存しない。手で書き換えた保存値は `layer` が分けて返し、消すことはできる
#[test]
fn 使えない環境変数の名前は保存せず手書きの名前は分けて返す() {
    let base = sandbox("s4-badenv");
    let file = base.join("a.py");
    touch(&file, "print(1)\n");
    for bad in ["1A", "A-B", "A=1; rm -rf ~; B", ""] {
        let env = env_of(&[(bad, Some("x"))]);
        let e = call_1732(&file, None, |a| a.env = Some(&env)).expect_err(bad);
        assert!(matches!(e, DispatchError::InvalidParams(_)), "{bad}: {e:?}");
    }
    // 手で書き換えた保存値（保存の口を通らない）
    let store = store_path().unwrap();
    update_at(&store, |st| {
        let cfg = st.files.entry(key_of(&file)).or_default();
        cfg.env.insert("A-B".into(), "x".into());
        cfg.env.insert("OK".into(), "y".into());
    })
    .unwrap();
    let (layer, bad) = saved_for(&file).layer();
    assert_eq!(bad, vec!["A-B".to_string()]);
    assert_eq!(layer.env.keys().collect::<Vec<_>>(), vec!["OK"]);
    let msg = bad_env_names(&bad, &file).to_string();
    assert!(msg.contains("--unset-env"), "{msg}");
    // 壊れた名前でも消せる
    let unset = env_of(&[("A-B", None)]);
    call_1732(&file, None, |a| a.env = Some(&unset)).unwrap();
    assert!(saved_for(&file).layer().1.is_empty());
    let _ = std::fs::remove_dir_all(&base);
}

/// 作業ディレクトリは今あるものだけ保存する（空は受けない = 既定へ戻すのは reset）
#[test]
fn 作業ディレクトリは今あるものだけ保存する() {
    let base = sandbox("s4-cwd");
    let file = base.join("src").join("a.py");
    touch(&file, "print(1)\n");
    for bad in ["nope", "", "  "] {
        let e = call_1732(&file, None, |a| a.cwd = Some(bad)).expect_err(bad);
        assert!(
            matches!(e, DispatchError::InvalidParams(_)),
            "{bad:?}: {e:?}"
        );
    }
    // 相対はファイルの段から・変数も展開して確かめる
    call_1732(&file, None, |a| a.cwd = Some("..")).unwrap();
    call_1732(&file, None, |a| a.cwd = Some("${fileDir}")).unwrap();
    let (layer, _) = saved_for(&file).layer();
    assert_eq!(layer.cwd.as_deref(), Some("${fileDir}"));
    // reset で既定へ
    let v = call_1732(&file, None, |a| a.reset = Some(&[])).unwrap();
    assert_eq!(v["updated"], json!([]), "空の reset は何もしない");
    let reset = ["cwd".to_string()];
    call_1732(&file, None, |a| a.reset = Some(&reset)).unwrap();
    assert_eq!(saved_for(&file).layer().0.cwd, None);
    let _ = std::fs::remove_dir_all(&base);
}

/// 空文字の引数・実行前コマンドは「無し」という明示の値（プロジェクトの値を塞ぐ）
#[test]
fn 空文字の引数と実行前はプロジェクトの値を塞ぐ() {
    let base = sandbox("s4-empty");
    let file = base.join("a.py");
    touch(&file, "print(1)\n");
    call_1732(&file, Some("project"), |a| {
        a.args = Some("--p");
        a.before = Some("false");
    })
    .unwrap();
    assert_eq!(saved_for(&file).layer().0.args.as_deref(), Some("--p"));
    call_1732(&file, None, |a| {
        a.args = Some("");
        a.before = Some("");
    })
    .unwrap();
    let (layer, _) = saved_for(&file).layer();
    assert_eq!(layer.args.as_deref(), Some(""));
    assert_eq!(layer.before.as_deref(), Some(""));
    let _ = std::fs::remove_dir_all(&base);
}
