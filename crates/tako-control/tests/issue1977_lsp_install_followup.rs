//! #1977 の e2e: 言語サーバの自動導入（#1944）の続き。
//!
//! 1. **状態を送らないサーバ（pyright）の握手直後の空の補完**: pyright 1.1.414 は自動 import の索引が
//!    できるまで補完に `{"isIncomplete":true,"items":[]}` で答え、索引ができる頃に開いた文書の最初の
//!    診断を送る（実測: 両方とも握手から 0.5〜0.7 秒）。tako は状態（`experimental/serverStatus`）を
//!    送らないサーバを握手の直後の猶予（2 秒）しか読み込み中と分からず、遅い機・初回で索引が猶予を
//!    越えると空のまま確定していた。偽サーバの `quiet-loading`（同じ形。読み込みの終わりは合図）で、
//!    猶予の後に打った補完が最初の診断まで待って問い直すこと・A/B の旧腕（`TAKO_1977_LEGACY=1`）では
//!    空のまま返ることを固定する
//! 2. **古い版の置き場の片付け**: 表の版を上げた tako が起こしたとき、前の版の段を消す。古い tako が
//!    古い版で動いているあいだ（使っている印が生きている）は消さず、終わったら消す。今の版・他の
//!    サーバの段・tako の印の無い段は残る。印が壊れていれば mtime で比べ、書けなければ理由を返す
//! 3. **引き直し（シェル統合の合図）**: 合図を出したスレッド（本番は UI スレッドの OSC の処理）を
//!    待たせず、間隔は直前の 1 巡にかかった時間で伸びる
//!
//! 置き場はすべて一時 dir（作るときと消すときに一時 dir の配下であることを確かめる = 本番の
//! data dir を触らない）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_control::lsp::fetch::{self, FetchConfig};
use tako_control::lsp::manager::{recheck_interval, DEFAULT_IDLE_GRACE, RECHECK_INTERVAL};
use tako_control::lsp::{
    CompletionAnswer, CompletionRequest, DocLink, Launch, LspConfig, LspManager,
};
use tako_core::lsp::completion::Trigger;
use tako_core::lsp::fetch::{self as table, Fetch};
use tako_core::lsp::servers::{self, DocumentKind, InstallHint, ServerSpec};
use tako_core::lsp::state::RestartPolicy;
use tako_core::platform::child_cmd::ChildCmd;

#[path = "common/lsp_fake_e2e.rs"]
mod lsp_fake_e2e;

use lsp_fake_e2e::LoadingGate;

const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");

/// A/B の env（`TAKO_1977_LEGACY`）とシェル統合の合図（プロセス全体で 1 つ）を触るので、
/// このファイルのテストは 1 本ずつ走る
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 一時 dir の配下であることを確かめる（片付けは消す = 取り違えで実の置き場を消さない）
fn assert_temp(dir: &Path) {
    assert!(
        dir.starts_with(std::env::temp_dir()),
        "一時ディレクトリ以外を置き場にしようとしている: {}",
        dir.display()
    );
}

/// 使い捨ての置き場（固定名を使わない = 並行する cargo test 同士で消し合わない。#1666）
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tako-1977-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        assert_temp(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        Self(dir)
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }

    fn write(&self, rel: &str, body: &str) -> PathBuf {
        let path = self.path(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        path
    }

    fn log(&self) -> PathBuf {
        self.path("received.jsonl")
    }

    fn gate(&self) -> LoadingGate {
        LoadingGate::new(&self.0)
    }

    fn root(&self) -> PathBuf {
        let root = self.path("data/lsp-servers");
        assert_temp(&root);
        root
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        assert_temp(&self.0);
        #[cfg(unix)]
        {
            // 読み取り専用にした段が残っていても消せるように戻す
            use std::os::unix::fs::PermissionsExt;
            for entry in walk(&self.0) {
                if entry.is_dir() {
                    let _ =
                        std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(0o755));
                }
            }
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// ディレクトリの下を全部（symlink は辿らない）
fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![dir.to_path_buf()];
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

const TEXT: &str = "fn main() { ab }\n";

/// 打鍵の補完（GUI と同じ `superseding`）
fn typing(path: &Path) -> CompletionRequest {
    CompletionRequest {
        path: path.to_path_buf(),
        line: 0,
        column: 14,
        timeout: Duration::from_secs(60),
        document: None,
        trigger: Trigger::Word,
        superseding: true,
        ticket: None,
        resolve_top: 0,
    }
}

fn open_editing(manager: &LspManager, path: &Path, text: &str) -> DocLink {
    let mut link = DocLink::default();
    manager.sync(&mut link, true, path, text, 1);
    link
}

// --- 1. 状態を送らないサーバの握手直後の空 -------------------------------------------------------

/// pyright 型の偽サーバ（読み込みの終わりは合図 = 実時間に任せない。済んだら候補を 2 つ返す）
fn quiet_config(scratch: &Scratch) -> LspConfig {
    let completion = scratch.write(
        "completion.json",
        &json!({ "items": [{ "label": "abc" }, { "label": "abd" }] }).to_string(),
    );
    let mut args = vec![
        "--scenario".to_string(),
        "quiet-loading".to_string(),
        "--log".to_string(),
        scratch.log().display().to_string(),
        "--completion".to_string(),
        completion.display().to_string(),
    ];
    args.extend(scratch.gate().args());
    LspConfig {
        table: servers::SERVERS,
        launcher: Arc::new(move |_spec: &ServerSpec| Launch::Found {
            plan: ChildCmd {
                program: FAKE.to_string(),
                args: args.clone(),
            },
            program_path: FAKE.to_string(),
        }),
        request_timeout: Duration::from_secs(10),
        shutdown_timeout: Duration::from_secs(5),
        idle_grace: DEFAULT_IDLE_GRACE,
        restart: RestartPolicy {
            max_restarts: 3,
            base_delay_ms: 50,
        },
        raw_log_dir: None,
    }
}

/// 補完の答えの候補の名前
fn labels(answer: &CompletionAnswer) -> Vec<String> {
    answer.items.iter().map(|i| i.label.clone()).collect()
}

/// 受け入れ条件 1: 猶予が明けた後でも、最初の診断の前に返った空は確定させず、診断を待って問い直す。
/// 打った補完を背景で投げ、偽サーバへ届いてから（= 空の答えはもう返っている）読み込みを終わらせる
#[test]
fn 状態を送らないサーバの握手直後の空は最初の診断まで待って問い直す() {
    let _env = serial();
    let scratch = Scratch::new("warm");
    let main = scratch.write("src/main.rs", TEXT);
    let manager = LspManager::new(quiet_config(&scratch));
    let _link = open_editing(&manager, &main, TEXT);
    let warming = lsp_fake_e2e::wait_warming_known(&manager);
    assert_eq!(
        warming["loading"],
        json!(true),
        "猶予の後も最初の診断の前は読み込み中: {warming}"
    );
    let asking = {
        let (manager, request) = (manager.clone(), typing(&main));
        std::thread::spawn(move || manager.completion(&request))
    };
    lsp_fake_e2e::wait_received(&scratch.log(), "textDocument/completion", 1);
    scratch.gate().finish();
    let answer = asking.join().expect("補完のスレッド").expect("補完の答え");
    assert_eq!(
        labels(&answer),
        ["abc", "abd"],
        "索引ができた後の候補が出る"
    );
    assert!(
        answer.waited_for_loading.is_some(),
        "読み込みを待ったと分かる（`waited_for_loading`）"
    );
    lsp_fake_e2e::stop_and_settle(&manager, &scratch.log());
    assert_eq!(
        lsp_fake_e2e::received(&scratch.log(), "textDocument/completion"),
        2,
        "空の後に 1 回だけ問い直す"
    );
}

/// A/B（検出力の実証）: #1977 の前は、猶予が明けた後の空がそのまま 0 件で確定する（同じ順序）
#[test]
fn 旧挙動では猶予の後の空がそのまま確定する() {
    let _env = serial();
    std::env::set_var("TAKO_1977_LEGACY", "1");
    let scratch = Scratch::new("warm-legacy");
    let main = scratch.write("src/main.rs", TEXT);
    let manager = LspManager::new(quiet_config(&scratch));
    let _link = open_editing(&manager, &main, TEXT);
    let warming = lsp_fake_e2e::wait_warming_known(&manager);
    let asking = {
        let (manager, request) = (manager.clone(), typing(&main));
        std::thread::spawn(move || manager.completion(&request))
    };
    lsp_fake_e2e::wait_received(&scratch.log(), "textDocument/completion", 1);
    scratch.gate().finish();
    let answer = asking.join().expect("補完のスレッド").expect("補完の答え");
    lsp_fake_e2e::stop_and_settle(&manager, &scratch.log());
    std::env::remove_var("TAKO_1977_LEGACY");
    assert_eq!(
        warming["loading"],
        json!(false),
        "旧腕は猶予の後を読み込み中とみなさない: {warming}"
    );
    assert!(
        labels(&answer).is_empty(),
        "旧腕は空のまま: {:?}",
        labels(&answer)
    );
    assert!(answer.waited_for_loading.is_none());
    assert_eq!(
        lsp_fake_e2e::received(&scratch.log(), "textDocument/completion"),
        1,
        "旧腕は問い直さない"
    );
}

// --- 2. 古い版の置き場の片付け --------------------------------------------------------------------

const ID: &str = "fake-sweep";
const PROGRAM: &str = "fake-sweep-server";

fn leak_str(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

/// 版 `version` の表の 1 行（取得物は使わない = 置き場に前もって置く。起こすのは偽サーバの写し）
fn binary_spec(scratch: &Scratch, version: &'static str) -> &'static ServerSpec {
    let args: Vec<&'static str> = vec![
        "--scenario",
        "normal",
        "--log",
        leak_str(scratch.log().display().to_string()),
    ];
    let assets: &'static [table::PlatformAsset] = Box::leak(Box::new([table::PlatformAsset {
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        asset: table::Asset {
            url: "https://example.test/fake.gz",
            digest: table::Digest::Sha256(
                "0000000000000000000000000000000000000000000000000000000000000000",
            ),
            archive: table::Archive::Gzip,
            size: 1,
        },
    }]));
    Box::leak(Box::new(ServerSpec {
        id: ID,
        documents: Box::leak(Box::new([DocumentKind {
            extension: "fakesweep",
            language_id: "fakesweep",
        }])),
        program: PROGRAM,
        args: Box::leak(args.into_boxed_slice()),
        root_markers: &["sweep.marker"],
        workspace_root: None,
        install: InstallHint {
            macos: "install-fake-sweep-yourself",
            windows: "install-fake-sweep-yourself",
        },
        fetch: Some(Fetch::Binary {
            version,
            assets,
            probe_args: &[],
        }),
    }))
}

/// 置き場に済んだ版を置く（偽サーバの写し + 済んだ印。`installed_at` は印の中身）
fn place_version(
    root: &Path,
    id: &str,
    version: &str,
    installed_at: u64,
    program: bool,
) -> PathBuf {
    assert_temp(root);
    let dir = root.join(id).join(version);
    std::fs::create_dir_all(&dir).unwrap();
    if program {
        let dest = table::binary_program(&dir, PROGRAM, cfg!(windows));
        std::fs::copy(FAKE, &dest).expect("偽サーバを写す");
    } else {
        std::fs::write(dir.join("payload.bin"), vec![7u8; 4096]).unwrap();
    }
    std::fs::write(
        dir.join(table::MARKER),
        json!({ "label": version, "installed_at": installed_at }).to_string(),
    )
    .unwrap();
    dir
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// 置き場の下の段の一覧（`<id>/<版>`。`.` で始まる跡は除く）
fn listing(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for id in std::fs::read_dir(root).into_iter().flatten().flatten() {
        for version in std::fs::read_dir(id.path()).into_iter().flatten().flatten() {
            let name = version.file_name().to_string_lossy().into_owned();
            if !name.starts_with('.') {
                out.push(format!("{}/{name}", id.file_name().to_string_lossy()));
            }
        }
    }
    out.sort();
    out
}

/// 終わったプロセスの pid（偽サーバを即死させて刈り取る）
fn dead_pid() -> u32 {
    let mut child = std::process::Command::new(FAKE)
        .args(["--scenario", "die"])
        .spawn()
        .expect("偽サーバを起こす");
    let pid = child.id();
    child.wait().expect("刈り取る");
    assert!(!tako_core::platform::process::pid_alive(pid));
    pid
}

/// 置き場の版から起こす manager（グローバルには無い = 置き場のものを使う。自動では取らない）
fn managed_manager(spec: &'static ServerSpec, root: &Path) -> LspManager {
    let config = LspConfig {
        table: std::slice::from_ref(spec),
        launcher: Arc::new(|spec: &ServerSpec| Launch::NotFound {
            program: spec.program.to_string(),
            override_env: None,
        }),
        request_timeout: Duration::from_secs(10),
        shutdown_timeout: Duration::from_secs(5),
        idle_grace: DEFAULT_IDLE_GRACE,
        restart: RestartPolicy {
            max_restarts: 3,
            base_delay_ms: 50,
        },
        raw_log_dir: None,
    };
    let fetch = FetchConfig {
        auto: false,
        ..FetchConfig::with_root(root.to_path_buf(), None)
    };
    LspManager::with_fetch(config, Some(fetch))
}

fn wait_state(manager: &LspManager, state: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let status = manager.status(None)["servers"][0].clone();
        if status["state"] == json!(state) {
            return status;
        }
        assert!(Instant::now() < deadline, "{state} にならない: {status:#}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 片付けの結果の 1 列（`<id>/<版>[ 理由]`。並びは読んだ順に依らないよう整える）
fn names(cleanup: &Value, key: &str) -> Vec<String> {
    let mut out: Vec<String> = cleanup[key]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|d| {
            let reason = d["reason"]
                .as_str()
                .map(|r| format!(" {r}"))
                .unwrap_or_default();
            format!(
                "{}/{}{reason}",
                d["id"].as_str().unwrap(),
                d["version"].as_str().unwrap()
            )
        })
        .collect();
    out.sort();
    out
}

/// 受け入れ条件 2: 版を上げた tako が起こすと前の版の段が消え、今の版・他のサーバの段は残る。
/// 古い tako が古い版で動いているあいだは消さない（エッジ: 片付けの最中にそのサーバが動いている）
#[test]
fn 版を上げると古い版の段が消え動いているあいだは残す() {
    let _env = serial();
    let scratch = Scratch::new("upgrade");
    let root = scratch.root();
    let old = binary_spec(&scratch, "v1");
    let new = binary_spec(&scratch, "v2");
    let now = now_secs();
    place_version(&root, ID, "v1", now - 100, true);
    place_version(&root, ID, "v2", now, true);
    place_version(&root, "other-server", "v0", now - 1000, false);
    std::fs::create_dir_all(root.join(ID).join("hand-made")).unwrap();
    let before = listing(&root);
    assert_eq!(
        before,
        [
            "fake-sweep/hand-made",
            "fake-sweep/v1",
            "fake-sweep/v2",
            "other-server/v0"
        ]
    );

    // 古い tako（表の版 v1）が v1 から起こして動いている
    let old_doc = scratch.write("proj-old/main.fakesweep", "x\n");
    scratch.write("proj-old/sweep.marker", "");
    let old_manager = managed_manager(old, &root);
    let _old_link = open_editing(&old_manager, &old_doc, "x\n");
    let running = wait_state(&old_manager, "running");
    assert_eq!(running["source"], "managed", "{running:#}");
    let old_pid = running["pid"].as_u64().expect("pid");
    assert!(
        root.join(ID)
            .join("v1")
            .join(table::lease_name(std::process::id()))
            .is_file(),
        "起こす前に使っている印を置く"
    );

    // 新しい tako（表の版 v2）が起こす → v1 は使われているので残す
    let new_doc = scratch.write("proj-new/main.fakesweep", "y\n");
    scratch.write("proj-new/sweep.marker", "");
    let new_manager = managed_manager(new, &root);
    let _new_link = open_editing(&new_manager, &new_doc, "y\n");
    wait_state(&new_manager, "running");
    let cleanup = new_manager.status(None)["fetch"]["cleanup"].clone();
    assert_eq!(
        names(&cleanup, "removed"),
        Vec::<String>::new(),
        "{cleanup:#}"
    );
    assert_eq!(
        names(&cleanup, "kept"),
        ["fake-sweep/hand-made unmarked", "fake-sweep/v1 in_use"],
        "{cleanup:#}"
    );
    assert_eq!(listing(&root), before, "動いているあいだは何も消えない");
    let still = old_manager.status(None)["servers"][0].clone();
    assert_eq!(still["state"], "running");
    assert_eq!(
        still["pid"].as_u64(),
        Some(old_pid),
        "古い版のサーバは起こし直されていない"
    );
    old_manager.shutdown_all(Duration::from_secs(5));
    // 古い版のサーバが消えてから（Windows は動いている実行ファイルの段の rename を断る）
    let deadline = Instant::now() + Duration::from_secs(30);
    while tako_core::platform::process::pid_alive(old_pid as u32) {
        assert!(Instant::now() < deadline, "古い版のサーバが終わらない");
        std::thread::sleep(Duration::from_millis(20));
    }

    // 古い tako が終わった（印の pid が死んでいる）→ 次に起こしたとき v1 を消す
    let lease = root
        .join(ID)
        .join("v1")
        .join(table::lease_name(std::process::id()));
    std::fs::rename(&lease, lease.with_file_name(table::lease_name(dead_pid()))).unwrap();
    new_manager.restart(Some(ID));
    wait_state(&new_manager, "running");
    let cleanup = new_manager.status(None)["fetch"]["cleanup"].clone();
    assert_eq!(names(&cleanup, "removed"), ["fake-sweep/v1"], "{cleanup:#}");
    assert!(
        cleanup["freed_bytes"].as_u64().unwrap_or(0) > 0,
        "{cleanup:#}"
    );
    assert_eq!(
        listing(&root),
        ["fake-sweep/hand-made", "fake-sweep/v2", "other-server/v0"],
        "今の版・他のサーバ・tako の印の無い段は残る"
    );
    new_manager.shutdown_all(Duration::from_secs(5));
}

/// Node.js の段も同じ規則（npm のサーバを起こしたとき）。印が壊れていれば mtime で比べ、
/// 今の版より後に入った段（新しい tako が入れた）は残す
#[test]
fn node_の古い版と壊れた印の段を片付け新しい版は残す() {
    let _env = serial();
    let scratch = Scratch::new("node");
    let root = scratch.root();
    let spec: &'static ServerSpec = Box::leak(Box::new(ServerSpec {
        fetch: Some(Fetch::Npm {
            packages: Box::leak(Box::new([table::NpmPackage {
                name: "fake-npm",
                version: "2.0.0",
                integrity: "x",
                size: 1,
            }])),
            entry: "node_modules/fake-npm/index.js",
            node_major: 1,
        }),
        ..*binary_spec(&scratch, "unused")
    }));
    let current = table::install_dir(&root, ID, spec.fetch.as_ref().unwrap());
    let current_name = current.file_name().unwrap().to_string_lossy().into_owned();
    // 今の版の印は遠い未来（壊れた印の mtime = 今 が「前」になる）
    place_version(&root, ID, &current_name, u64::MAX / 4, false);
    place_version(&root, ID, "fake-npm-1.0.0", 100, false);
    let broken = place_version(&root, ID, "fake-npm-0.9.0", 0, false);
    std::fs::write(broken.join(table::MARKER), "{壊れた").unwrap();
    place_version(&root, "node", table::NODE.version, 2_000, false);
    place_version(&root, "node", "22.0.0", 1_000, false);
    place_version(&root, "node", "99.0.0", 3_000, false);
    let sweep = fetch::sweep_old_versions(&FetchConfig::with_root(root.clone(), None), spec);
    let json = sweep.to_json();
    assert_eq!(
        names(&json, "removed"),
        [
            "fake-sweep/fake-npm-0.9.0",
            "fake-sweep/fake-npm-1.0.0",
            "node/22.0.0"
        ],
        "{json:#}"
    );
    assert_eq!(
        listing(&root),
        [
            format!("fake-sweep/{current_name}"),
            format!("node/{}", table::NODE.version),
            "node/99.0.0".to_string(),
        ],
        "今の版と、今の版より後に入った Node.js は残る"
    );
}

/// 書けない置き場（エッジ: data dir が読み取り専用）: 消さずに理由を返し、置き場は変わらない
#[cfg(unix)]
#[test]
fn 書けない置き場では消さずに理由を返す() {
    use std::os::unix::fs::PermissionsExt;
    let _env = serial();
    let scratch = Scratch::new("readonly");
    let root = scratch.root();
    let spec = binary_spec(&scratch, "v2");
    place_version(&root, ID, "v1", 100, false);
    place_version(&root, ID, "v2", 200, false);
    let before = listing(&root);
    std::fs::set_permissions(root.join(ID), std::fs::Permissions::from_mode(0o555)).unwrap();
    let sweep = fetch::sweep_old_versions(&FetchConfig::with_root(root.clone(), None), spec);
    std::fs::set_permissions(root.join(ID), std::fs::Permissions::from_mode(0o755)).unwrap();
    let json = sweep.to_json();
    assert_eq!(names(&json, "removed"), Vec::<String>::new(), "{json:#}");
    assert_eq!(names(&json, "kept"), ["fake-sweep/v1 failed"], "{json:#}");
    assert!(
        json["kept"][0]["detail"]
            .as_str()
            .is_some_and(|d| !d.is_empty()),
        "{json:#}"
    );
    assert_eq!(listing(&root), before);
}

/// A/B: 旧腕（`TAKO_1977_LEGACY=1`）は起こしても片付けない
#[test]
fn 旧挙動では片付けない() {
    let _env = serial();
    let scratch = Scratch::new("sweep-legacy");
    let root = scratch.root();
    let spec = binary_spec(&scratch, "v2");
    place_version(&root, ID, "v1", 100, false);
    place_version(&root, ID, "v2", 200, true);
    let doc = scratch.write("proj/main.fakesweep", "y\n");
    scratch.write("proj/sweep.marker", "");
    std::env::set_var("TAKO_1977_LEGACY", "1");
    let manager = managed_manager(spec, &root);
    let _link = open_editing(&manager, &doc, "y\n");
    wait_state(&manager, "running");
    let cleanup = manager.status(None)["fetch"]["cleanup"].clone();
    std::env::remove_var("TAKO_1977_LEGACY");
    assert!(cleanup.is_null(), "{cleanup:#}");
    assert_eq!(listing(&root), ["fake-sweep/v1", "fake-sweep/v2"]);
    manager.shutdown_all(Duration::from_secs(5));
}

// --- 3. 引き直しのコスト ----------------------------------------------------------------------

/// 受け入れ条件 3: シェル統合の合図（本番は UI スレッドの OSC の処理）は引き直しを待たない =
/// 解決（ログインシェル）は合図を出したスレッドではなく背景の `lsp-recheck` で走る。間隔は直前の
/// 1 巡にかかった時間で伸びる（重い rc のログインシェルを 1 秒の解決で演じる）。
/// 実時間の上限では比べない（`.agent/conventions.md`「効果を測る単体テストは実時間で比べない」）:
/// 見るのは解決を呼んだスレッドと、合図が返った時点の状態（合図の処理はロックの中で同期に状態を変える）
#[test]
fn シェル統合の合図は引き直しを待たず間隔は直前の_1_巡で伸びる() {
    let _env = serial();
    let scratch = Scratch::new("recheck");
    let calls = Arc::new(AtomicUsize::new(0));
    // 解決を呼んだスレッドの名前（呼んだ順）
    let threads: Arc<Mutex<Vec<Option<String>>>> = Arc::default();
    let slow = Duration::from_millis(1_000);
    let config = LspConfig {
        table: std::slice::from_ref(binary_spec(&scratch, "v1")),
        launcher: {
            let (calls, threads) = (Arc::clone(&calls), Arc::clone(&threads));
            Arc::new(move |spec: &ServerSpec| {
                calls.fetch_add(1, Ordering::SeqCst);
                threads
                    .lock()
                    .unwrap()
                    .push(std::thread::current().name().map(str::to_string));
                std::thread::sleep(slow);
                Launch::NotFound {
                    program: spec.program.to_string(),
                    override_env: None,
                }
            })
        },
        request_timeout: Duration::from_secs(10),
        shutdown_timeout: Duration::from_secs(5),
        idle_grace: DEFAULT_IDLE_GRACE,
        restart: RestartPolicy {
            max_restarts: 3,
            base_delay_ms: 50,
        },
        raw_log_dir: None,
    };
    let manager = LspManager::new(config);
    let doc = scratch.write("proj/main.fakesweep", "x\n");
    scratch.write("proj/sweep.marker", "");
    let _link = open_editing(&manager, &doc, "x\n");
    wait_state(&manager, "not_installed");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let before = manager.status(None)["recheck"].clone();
    assert_eq!(before["targets"], json!([ID]), "{before:#}");
    assert_eq!(before["interval_secs"], json!(RECHECK_INTERVAL.as_secs()));

    // 合図を出す（このスレッド = 本番の UI スレッドの役）。解決はここで走らない
    let caller = std::thread::current().name().map(str::to_string);
    tako_core::shell_activity::note();
    let deadline = Instant::now() + Duration::from_secs(30);
    let after = loop {
        let recheck = manager.status(None)["recheck"].clone();
        if recheck["count"] == json!(1) {
            break recheck;
        }
        assert!(
            Instant::now() < deadline,
            "引き直しが終わらない: {recheck:#}"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(calls.load(Ordering::SeqCst), 2, "背景で 1 回だけ引き直した");
    let seen = threads.lock().unwrap().clone();
    assert_eq!(
        seen[1].as_deref(),
        Some("lsp-recheck"),
        "引き直しの解決は背景のスレッドで走る: {seen:?}"
    );
    assert!(
        !seen.contains(&caller),
        "合図を出したスレッドで解決した（UI スレッドを待たせる）: {seen:?} / {caller:?}"
    );
    let cost = after["last_cost_ms"].as_u64().expect("かかった時間");
    assert!(cost >= slow.as_millis() as u64, "{after:#}");
    let interval = after["interval_secs"].as_u64().unwrap();
    assert_eq!(
        Duration::from_secs(interval),
        Duration::from_secs(recheck_interval(Some(Duration::from_millis(cost))).as_secs()),
        "{after:#}"
    );
    assert!(
        interval >= 50,
        "1 秒の引き直しの間隔は 50 秒以上: {after:#}"
    );

    // 間隔の中の合図は引き直さない（明けたときに 1 回だけ = 待ちのスレッドを 1 本だけ立てる）。
    // 合図の処理はロックの中で同期に「待ち」を立てるので、返った時点の状態で見られる
    tako_core::shell_activity::note();
    tako_core::shell_activity::note();
    let within = manager.status(None)["recheck"].clone();
    assert_eq!(within["pending"], json!(true), "{within:#}");
    assert_eq!(within["running"], json!(false), "{within:#}");
    assert_eq!(within["count"], json!(1), "{within:#}");
    assert_eq!(calls.load(Ordering::SeqCst), 2, "間隔の中では引き直さない");
    manager.shutdown_all(Duration::from_secs(5));
}

/// 間隔の決め方（下限 15 秒・時間 × 50・上限 5 分。旧腕は固定の 15 秒）
#[test]
fn 引き直しの間隔は直前の_1_巡の_50_倍を下限と上限に収める() {
    let _env = serial();
    let secs = |ms: u64| recheck_interval(Some(Duration::from_millis(ms))).as_secs();
    assert_eq!(recheck_interval(None), RECHECK_INTERVAL);
    // 実測した普通の rc（0.3 秒）は下限のまま、重い rc（2 秒）は 100 秒
    assert_eq!(secs(300), 15);
    assert_eq!(secs(2_000), 100);
    assert_eq!(secs(60_000), 300);
    std::env::set_var("TAKO_1977_LEGACY", "1");
    let legacy = recheck_interval(Some(Duration::from_secs(2)));
    std::env::remove_var("TAKO_1977_LEGACY");
    assert_eq!(legacy, RECHECK_INTERVAL);
}
