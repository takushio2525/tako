//! #1944 の e2e: 未導入の言語サーバを tako の data dir へ取って起こす経路を、**ローカルの配布元**
//! （tiny_http）と**偽の言語サーバ（`tako-lsp-fake`）の gzip** で実プロセスごと測る。
//!
//! 実の配布元（npm / nodejs.org / GitHub）へは出ない: 取得物の URL は `https://example.test/…` で、
//! [`FetchConfig::base`]（本番の `TAKO_LSP_FETCH_BASE` と同じ口）でローカルへ向ける。ハッシュは
//! テストが組んだ gzip から計算して表に固定する（本番と同じく取得物全体を検証する）。
//!
//! 測るもの: 取って起こす（状態が取得中 → 稼働・出どころが managed）/ グローバル優先（配布元へ
//! 1 度も来ない）/ ハッシュ不一致・オフライン・404・大きすぎる・書き込めない（理由を出して未導入へ
//! 落ち、案内（導入コマンド）を残し、途中の物を置かない）/ 失敗の後は開き直しても取り直さない /
//! `install` で取り直すと待っていた文書のサーバが起きる / 2 つの器が同時に求めても 1 回だけ取る /
//! 取っている途中で閉じたら起こさない / 取っている途中の stop / シェル統合の合図で引き直して
//! restart を待たずに起こす（#1823 の 2）
//!
//! A/B（検出力の実証）: `TAKO_1944_LEGACY=1` は本番の `FetchConfig::from_env` を `None` にする
//! （このファイルは設定を自分で組むので、番犬 `issue1944_lsp_fetch_watchdog` がそちらを縛る）

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use tako_control::lsp::fetch::{self, FetchConfig, FetchError};
use tako_control::lsp::manager::DEFAULT_IDLE_GRACE;
use tako_control::lsp::{text, DocLink, Launch, LspConfig, LspManager};
use tako_core::lsp::fetch::{Archive, Asset, Digest, Fetch, PlatformAsset};
use tako_core::lsp::servers::{DocumentKind, InstallHint, ServerSpec};
use tako_core::lsp::state::RestartPolicy;
use tako_core::platform::child_cmd::ChildCmd;

const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");
const ID: &str = "fake-fetch";
const INSTALL_COMMAND: &str = "install-fake-fetch-yourself";

/// 使い捨ての置き場（固定名を使わない = 並行する cargo test 同士で消し合わない。#1666）
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tako-1944-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("proj")).unwrap();
        Self(dir)
    }

    fn root(&self) -> PathBuf {
        self.0.join("lsp-servers")
    }

    /// 開く文書（拡張子は表の行が受け持つもの）。`n` で別のプロジェクト（= 別の器）
    fn file(&self, n: usize) -> PathBuf {
        let dir = self.0.join(format!("proj{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("fetch.marker"), "").unwrap();
        dir.join("main.fakefetch")
    }

    fn log(&self) -> PathBuf {
        self.0.join("received.jsonl")
    }

    fn spawns(&self) -> PathBuf {
        self.0.join("spawns.txt")
    }

    fn spawn_count(&self) -> usize {
        std::fs::read_to_string(self.spawns())
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// --- ローカルの配布元 -------------------------------------------------------------

/// 1 つの URL の答え方
#[derive(Clone)]
enum Serve {
    Bytes(Vec<u8>),
    /// 4 KiB ずつ `pause` を挟んで送る（取っている途中を観測する）
    Slow {
        bytes: Vec<u8>,
        pause: Duration,
    },
    /// 中身は送らず、この大きさを申告する
    Declare(u64),
    Status(u16),
}

struct Origin {
    base: String,
    hits: Arc<AtomicUsize>,
    routes: Arc<Mutex<HashMap<String, Serve>>>,
    stop: Arc<AtomicBool>,
}

impl Origin {
    fn start() -> Self {
        Self::start_at("127.0.0.1:0")
    }

    fn start_at(addr: &str) -> Self {
        let server = Arc::new(tiny_http::Server::http(addr).expect("配布元を立てる"));
        let port = server.server_addr().to_ip().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let routes: Arc<Mutex<HashMap<String, Serve>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (h, r, st) = (Arc::clone(&hits), Arc::clone(&routes), Arc::clone(&stop));
        std::thread::spawn(move || {
            while !st.load(Ordering::SeqCst) {
                let Ok(Some(request)) = server.recv_timeout(Duration::from_millis(50)) else {
                    continue;
                };
                h.fetch_add(1, Ordering::SeqCst);
                let route = r.lock().unwrap().get(request.url()).cloned();
                // 応答は別スレッドで（遅い送りが他の要求を止めない）
                std::thread::spawn(move || {
                    let _ = match route {
                        None => request.respond(tiny_http::Response::empty(404)),
                        Some(Serve::Status(code)) => {
                            request.respond(tiny_http::Response::empty(code))
                        }
                        Some(Serve::Bytes(bytes)) => {
                            request.respond(tiny_http::Response::from_data(bytes))
                        }
                        Some(Serve::Slow { bytes, pause }) => {
                            let len = bytes.len();
                            request.respond(tiny_http::Response::new(
                                200.into(),
                                Vec::new(),
                                SlowReader {
                                    bytes,
                                    at: 0,
                                    pause,
                                },
                                Some(len),
                                None,
                            ))
                        }
                        // tiny_http は大きい応答を chunked で送る（Content-Length を付けない）ので閾値を外す
                        Some(Serve::Declare(len)) => request.respond(
                            tiny_http::Response::new(
                                200.into(),
                                Vec::new(),
                                std::io::empty(),
                                Some(len as usize),
                                None,
                            )
                            .with_chunked_threshold(usize::MAX),
                        ),
                    };
                });
            }
        });
        Self {
            base: format!("http://127.0.0.1:{port}"),
            hits,
            routes,
            stop,
        }
    }

    /// `https://example.test/<path>` を `<base>/example.test/<path>` で答える
    fn route(&self, path: &str, serve: Serve) {
        self.routes
            .lock()
            .unwrap()
            .insert(format!("/example.test/{path}"), serve);
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

impl Drop for Origin {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

struct SlowReader {
    bytes: Vec<u8>,
    at: usize,
    pause: Duration,
}

impl Read for SlowReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.at >= self.bytes.len() {
            return Ok(0);
        }
        std::thread::sleep(self.pause);
        let n = buf.len().min(4096).min(self.bytes.len() - self.at);
        buf[..n].copy_from_slice(&self.bytes[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// 偽サーバの実行ファイルを gzip にしたもの（取得物）
fn fake_gz() -> &'static [u8] {
    static GZ: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    GZ.get_or_init(|| {
        let bytes = std::fs::read(FAKE).expect("偽サーバを読む");
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(&bytes).unwrap();
        enc.finish().unwrap()
    })
}

fn leak<T>(value: T) -> &'static T {
    Box::leak(Box::new(value))
}

fn leak_str(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

/// 表の 1 行（取得物は `https://example.test/<asset>`。ハッシュは `digest_of` で `bytes` から）
fn spec_for(scratch: &Scratch, asset: &str, bytes: &[u8], size: u64) -> &'static ServerSpec {
    let args: Vec<&'static str> = vec![
        "--scenario",
        "normal",
        "--log",
        leak_str(scratch.log().display().to_string()),
        "--spawns",
        leak_str(scratch.spawns().display().to_string()),
    ];
    let assets: &'static [PlatformAsset] = Box::leak(Box::new([PlatformAsset {
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        asset: Asset {
            url: leak_str(format!("https://example.test/{asset}")),
            digest: Digest::Sha256(leak_str(fetch::digest_of(bytes, false))),
            archive: Archive::Gzip,
            size,
        },
    }]));
    leak(ServerSpec {
        id: ID,
        documents: Box::leak(Box::new([DocumentKind {
            extension: "fakefetch",
            language_id: "fakefetch",
        }])),
        program: "fake-fetch-server",
        args: Box::leak(args.into_boxed_slice()),
        root_markers: &["fetch.marker"],
        workspace_root: None,
        install: InstallHint {
            macos: INSTALL_COMMAND,
            windows: INSTALL_COMMAND,
        },
        fetch: Some(Fetch::Binary {
            version: "t1",
            assets,
            probe_args: &[],
        }),
    })
}

/// 本物の取得物で答える表の 1 行
fn good_spec(
    scratch: &Scratch,
    origin: &Origin,
    serve: impl Fn(Vec<u8>) -> Serve,
) -> &'static ServerSpec {
    let gz = fake_gz();
    origin.route("fake.gz", serve(gz.to_vec()));
    spec_for(scratch, "fake.gz", gz, gz.len() as u64)
}

/// グローバル（PATH）にあるかを切り替えられる解決
fn launcher(global: Arc<AtomicBool>, scratch: &Scratch) -> tako_control::lsp::manager::Launcher {
    let args = vec![
        "--scenario".to_string(),
        "normal".to_string(),
        "--log".to_string(),
        scratch.log().display().to_string(),
        "--spawns".to_string(),
        scratch.spawns().display().to_string(),
    ];
    Arc::new(move |spec: &ServerSpec| {
        if global.load(Ordering::SeqCst) {
            Launch::Found {
                plan: ChildCmd {
                    program: FAKE.to_string(),
                    args: args.clone(),
                },
                program_path: FAKE.to_string(),
            }
        } else {
            Launch::NotFound {
                program: spec.program.to_string(),
                override_env: None,
            }
        }
    })
}

fn manager(
    scratch: &Scratch,
    spec: &'static ServerSpec,
    global: Arc<AtomicBool>,
    base: &str,
    auto: bool,
) -> LspManager {
    let config = LspConfig {
        table: std::slice::from_ref(spec),
        launcher: launcher(global, scratch),
        request_timeout: Duration::from_secs(10),
        shutdown_timeout: Duration::from_secs(5),
        idle_grace: DEFAULT_IDLE_GRACE,
        restart: RestartPolicy {
            max_restarts: 3,
            base_delay_ms: 50,
        },
        raw_log_dir: None,
    };
    let mut fetch = FetchConfig::with_root(scratch.root(), Some(base.to_string()));
    fetch.auto = auto;
    LspManager::with_fetch(config, Some(fetch))
}

fn open(manager: &LspManager, path: &Path) -> DocLink {
    let mut link = DocLink::Unlinked;
    manager.sync(&mut link, true, path, "fn main() {}\n", 1);
    link
}

fn server_status(manager: &LspManager) -> Value {
    manager.status(Some(ID))["servers"]
        .as_array()
        .and_then(|a| a.first().cloned())
        .unwrap_or(Value::Null)
}

fn wait_for(what: &str, limit: Duration, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("待ちが上限を超えた: {what}");
}

fn wait_state(manager: &LspManager, state: &str) -> Value {
    let mut last = Value::Null;
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        last = server_status(manager);
        if last["state"] == state {
            return last;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("状態が {state} にならない: {last:#}");
}

fn partials(root: &Path) -> Vec<String> {
    let dir = root.join(ID);
    std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with(".partial-"))
                .collect()
        })
        .unwrap_or_default()
}

// --- テスト -------------------------------------------------------------------------

#[test]
fn 見つからなければ取って起こし取得中が見える() {
    let scratch = Scratch::new("fetch");
    let origin = Origin::start();
    let spec = good_spec(&scratch, &origin, |bytes| Serve::Slow {
        bytes,
        pause: Duration::from_millis(2),
    });
    let m = manager(&scratch, spec, Arc::default(), &origin.base, true);
    // UI への状態の知らせは診断の口とは別（#1679 の「診断の口は URI だけ」を崩さない）
    let mut diagnostics = m.diagnostics_events().expect("診断の口");
    let mut state = m.state_events().expect("状態の口");
    let _link = open(&m, &scratch.file(1));
    // 取っているあいだは「取得中」と進捗（UI の 1 行と同じ口）
    let fetching = wait_state(&m, "fetching");
    let mut saw_progress = fetching["fetch"].is_object();
    let started = Instant::now();
    while server_status(&m)["state"] == "fetching" && started.elapsed() < Duration::from_secs(60) {
        let status = server_status(&m);
        if status["fetch"]["bytes"].as_u64().unwrap_or(0) > 0 {
            saw_progress = true;
        }
        let doc = m.document_server(&scratch.file(1)).expect("画面の状態");
        assert!(
            doc.fetch.is_some() || server_status(&m)["state"] != "fetching",
            "取得中なら画面にも進捗"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        saw_progress,
        "取っている最中の進捗が status に出る: {fetching:#}"
    );
    let running = wait_state(&m, "running");
    assert_eq!(running["source"], "managed", "{running:#}");
    let program = running["program_path"].as_str().unwrap();
    assert!(
        Path::new(program).starts_with(scratch.root()),
        "置き場の中のものを起こした: {program}"
    );
    assert!(running["fetch"].is_null(), "取り終えたら進捗は消える");
    assert!(state.try_recv().is_ok(), "状態の口に知らせが来ている");
    assert!(
        m.take_refresh_wanted(),
        "状態が変わったので作り直しを求める"
    );
    assert!(!m.take_refresh_wanted(), "読むと倒れる");
    while let Ok(uri) = diagnostics.try_recv() {
        assert!(uri.starts_with("file://"), "診断の口は URI だけ: {uri:?}");
    }
    assert_eq!(origin.hits(), 1, "取得物は 1 回だけ取る");
    assert!(partials(&scratch.root()).is_empty(), "途中の段を残さない");
    let marker = scratch
        .root()
        .join(ID)
        .join("t1")
        .join(tako_core::lsp::fetch::MARKER);
    assert!(marker.is_file(), "済んだ印を書く");
    // 2 回目（開き直し・別のマネージャ）は取らずに置き場のものを使う
    drop(_link);
    let m2 = manager(&scratch, spec, Arc::default(), &origin.base, true);
    let _l2 = open(&m2, &scratch.file(2));
    let again = wait_state(&m2, "running");
    assert_eq!(again["source"], "managed");
    assert_eq!(origin.hits(), 1, "置き場にあれば取り直さない");
    m.shutdown_all(Duration::from_secs(5));
    m2.shutdown_all(Duration::from_secs(5));
}

#[test]
fn グローバルにあれば取らずにそれを使う() {
    let scratch = Scratch::new("global");
    let origin = Origin::start();
    let spec = good_spec(&scratch, &origin, Serve::Bytes);
    let m = manager(
        &scratch,
        spec,
        Arc::new(AtomicBool::new(true)),
        &origin.base,
        true,
    );
    let _link = open(&m, &scratch.file(1));
    let running = wait_state(&m, "running");
    assert_eq!(running["source"], "path", "{running:#}");
    assert_eq!(origin.hits(), 0, "配布元へ 1 度も来ない");
    assert!(!scratch.root().join(ID).exists(), "置き場に何も作らない");
    // install もグローバルがあれば取らない
    let installed = m.install(Some(ID));
    assert_eq!(installed["servers"][0]["status"], "global", "{installed:#}");
    assert_eq!(origin.hits(), 0);
    m.shutdown_all(Duration::from_secs(5));
}

/// 失敗の型 1 つ（名前・配布元の答え方・表の大きさ・`fetch_error` の種別）
type FailureCase = (&'static str, Box<dyn Fn(&Origin)>, u64, &'static str);

/// 失敗の型ごとに: 理由を出して未導入へ落ち、案内（導入コマンド）を残し、途中の物を置かない
#[test]
fn 取れなければ理由を出して今の案内へ落ちる() {
    let gz = fake_gz();
    let cases: Vec<FailureCase> = vec![
        (
            "digest",
            Box::new(|o: &Origin| {
                let mut tampered = fake_gz().to_vec();
                let last = tampered.len() - 1;
                tampered[last] ^= 0xff;
                o.route("fake.gz", Serve::Bytes(tampered));
            }),
            gz.len() as u64,
            "digest",
        ),
        ("404", Box::new(|_o: &Origin| {}), gz.len() as u64, "http"),
        (
            "503",
            Box::new(|o: &Origin| o.route("fake.gz", Serve::Status(503))),
            gz.len() as u64,
            "http",
        ),
        (
            "too-large",
            // 表の大きさ 1000 バイトに対して 10 MB を申告する（受ける前に打ち切る）
            Box::new(|o: &Origin| o.route("fake.gz", Serve::Declare(10_000_000))),
            1000,
            "too_large",
        ),
        (
            "too-large-stream",
            // 大きさを申告しない（chunked）まま 3 MB を送る（受けながら上限で打ち切る）
            Box::new(|o: &Origin| o.route("fake.gz", Serve::Bytes(vec![0u8; 3_000_000]))),
            1000,
            "too_large",
        ),
    ];
    for (label, setup, size, kind) in cases {
        let scratch = Scratch::new(label);
        let origin = Origin::start();
        setup(&origin);
        let spec = spec_for(&scratch, "fake.gz", gz, size);
        let m = manager(&scratch, spec, Arc::default(), &origin.base, true);
        let _link = open(&m, &scratch.file(1));
        let status = wait_state(&m, "not_installed");
        assert_eq!(status["fetch_error"], kind, "{label}: {status:#}");
        let reason = status["reason"].as_str().unwrap_or_default();
        assert!(!reason.is_empty(), "{label}: 理由を出す");
        assert_eq!(
            status["install_command"], INSTALL_COMMAND,
            "{label}: 今の案内を残す"
        );
        assert_eq!(
            status["next_step"],
            text::fill(
                text::FETCH_FAILED_NEXT_STEP,
                &[("name", ID), ("command", INSTALL_COMMAND)]
            ),
            "{label}"
        );
        assert!(
            !scratch.root().join(ID).join("t1").exists(),
            "{label}: 版の段を作らない"
        );
        assert!(
            partials(&scratch.root()).is_empty(),
            "{label}: 途中の段を残さない"
        );
        assert_eq!(scratch.spawn_count(), 0, "{label}: 何も起こさない");
        // 画面の状態も同じ理由
        let doc = m.document_server(&scratch.file(1)).expect("画面の状態");
        assert!(doc.fetch_failed && doc.can_install, "{label}");
        assert_eq!(doc.reason.as_deref(), Some(reason), "{label}");
    }
}

#[test]
fn ハッシュ不一致の理由は取得物を捨てたと言う() {
    let item = "x 1".to_string();
    let reason = FetchError::Digest { item: item.clone() }.reason();
    assert_eq!(
        reason,
        text::fill(text::FETCH_DIGEST_REASON, &[("item", &item)])
    );
}

#[test]
fn オフラインの後は開き直しても取り直さず_install_で取り直すと待っていた文書が起きる() {
    let scratch = Scratch::new("offline");
    // 閉じたポート = 接続できない（オフライン）
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let base = format!("http://127.0.0.1:{port}");
    let spec = spec_for(&scratch, "fake.gz", fake_gz(), fake_gz().len() as u64);
    let m = manager(&scratch, spec, Arc::default(), &base, true);
    let mut link = open(&m, &scratch.file(1));
    let status = wait_state(&m, "not_installed");
    assert_eq!(status["fetch_error"], "network", "{status:#}");
    // 開き直しても自動では取りに行かない（失敗の記録が残っている間は断る）
    let mut second = open(&m, &scratch.file(2));
    assert!(matches!(second, DocLink::Declined { .. }), "{second:?}");
    // ネットワークが戻った（同じポートに配布元が立った）→ install（画面の「もう一度取得」と同じ口）
    let origin = Origin::start_at(&format!("127.0.0.1:{port}"));
    origin.route("fake.gz", Serve::Bytes(fake_gz().to_vec()));
    assert_eq!(origin.hits(), 0, "失敗の後に自動で取り直していない");
    let result = m.install(Some(ID));
    assert_eq!(result["servers"][0]["status"], "installed", "{result:#}");
    assert_eq!(origin.hits(), 1);
    // 待っていた器が restart を待たずに起きる
    let running = wait_state(&m, "running");
    assert_eq!(running["source"], "managed");
    // 断っていた文書も、次の同期（UI は UI_REFRESH で直ちに）でつながる
    m.sync(&mut second, true, &scratch.file(2), "x\n", 1);
    assert!(matches!(second, DocLink::Open(_)), "{second:?}");
    m.sync(&mut link, true, &scratch.file(1), "fn main() {}\n", 2);
    m.shutdown_all(Duration::from_secs(5));
}

#[test]
fn 二つの器が同時に求めても一度だけ取る() {
    let scratch = Scratch::new("twice");
    let origin = Origin::start();
    let spec = good_spec(&scratch, &origin, |bytes| Serve::Slow {
        bytes,
        pause: Duration::from_millis(1),
    });
    let m = manager(&scratch, spec, Arc::default(), &origin.base, true);
    // 別のプロジェクト = 別の器（ServerKey）が同じサーバを同時に求める
    let _a = open(&m, &scratch.file(1));
    let _b = open(&m, &scratch.file(2));
    wait_for("2 つとも稼働", Duration::from_secs(60), || {
        let status = m.status(Some(ID));
        let servers = status["servers"].as_array().cloned().unwrap_or_default();
        servers.len() == 2 && servers.iter().all(|s| s["state"] == "running")
    });
    assert_eq!(origin.hits(), 1, "置き場ごとの錠で 1 本だけが取る");
    m.shutdown_all(Duration::from_secs(5));
}

#[test]
fn 取っている途中で閉じたら起こさない() {
    let scratch = Scratch::new("closed");
    let origin = Origin::start();
    let spec = good_spec(&scratch, &origin, |bytes| Serve::Slow {
        bytes,
        pause: Duration::from_millis(2),
    });
    let m = manager(&scratch, spec, Arc::default(), &origin.base, true);
    let link = open(&m, &scratch.file(1));
    wait_state(&m, "fetching");
    drop(link);
    let status = wait_state(&m, "not_started");
    assert!(status["pid"].is_null());
    // 取得物は置き場に残る（次に開いたときに取り直さない）
    assert!(tako_control::lsp::fetch::present(
        &FetchConfig::with_root(scratch.root(), None),
        spec
    ));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(scratch.spawn_count(), 0, "閉じた後に起こしていない");
    m.shutdown_all(Duration::from_secs(5));
}

#[test]
fn 取っている途中の_stop_は取れても起こさない() {
    let scratch = Scratch::new("stop");
    let origin = Origin::start();
    let spec = good_spec(&scratch, &origin, |bytes| Serve::Slow {
        bytes,
        pause: Duration::from_millis(2),
    });
    let m = manager(&scratch, spec, Arc::default(), &origin.base, true);
    let _link = open(&m, &scratch.file(1));
    wait_state(&m, "fetching");
    m.stop(Some(ID));
    assert_eq!(server_status(&m)["state"], "stopped");
    wait_for("取り終える", Duration::from_secs(60), || {
        tako_control::lsp::fetch::present(&FetchConfig::with_root(scratch.root(), None), spec)
    });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(server_status(&m)["state"], "stopped");
    assert_eq!(scratch.spawn_count(), 0);
    // restart で起きる（置き場のものを使う）
    m.restart(Some(ID));
    assert_eq!(wait_state(&m, "running")["source"], "managed");
    m.shutdown_all(Duration::from_secs(5));
}

#[cfg(unix)]
#[test]
fn 書き込めなければ理由を出す() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("readonly");
    let origin = Origin::start();
    let spec = good_spec(&scratch, &origin, Serve::Bytes);
    std::fs::create_dir_all(scratch.root()).unwrap();
    std::fs::set_permissions(scratch.root(), std::fs::Permissions::from_mode(0o555)).unwrap();
    let m = manager(&scratch, spec, Arc::default(), &origin.base, true);
    let _link = open(&m, &scratch.file(1));
    let status = wait_state(&m, "not_installed");
    std::fs::set_permissions(scratch.root(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(status["fetch_error"], "io", "{status:#}");
    assert_eq!(status["install_command"], INSTALL_COMMAND);
    m.shutdown_all(Duration::from_secs(5));
}

/// #1823 の 2: 未導入のまま（自動の取得を止めている）PATH に入ったら、シェル統合の合図で
/// 引き直して restart を待たずに起こす
#[test]
fn シェル統合の合図で引き直して入ったら起こす() {
    let scratch = Scratch::new("recheck");
    let origin = Origin::start();
    let spec = good_spec(&scratch, &origin, Serve::Bytes);
    let global = Arc::new(AtomicBool::new(false));
    let m = manager(&scratch, spec, Arc::clone(&global), &origin.base, false);
    let _link = open(&m, &scratch.file(1));
    let status = wait_state(&m, "not_installed");
    // 自動の取得を止めているときの次の一手は「tako が取って入れる」
    assert_eq!(
        status["next_step"],
        text::fill(
            text::FETCHABLE_NEXT_STEP,
            &[("name", ID), ("command", INSTALL_COMMAND)]
        )
    );
    assert_eq!(origin.hits(), 0, "自動では取らない");
    // 利用者が tako のターミナルで入れた（PATH に現れた）→ コマンドの終わりの合図
    global.store(true, Ordering::SeqCst);
    tako_core::shell_activity::note();
    let running = wait_state(&m, "running");
    assert_eq!(running["source"], "path", "{running:#}");
    assert_eq!(origin.hits(), 0);
    m.shutdown_all(Duration::from_secs(5));
}
