//! ファイル API を**実 HTTP** で通す e2e（#1079）
//!
//! `handle_files_request` を実 `tiny_http::Server` に載せ、実ファイル木に対して
//! 本物のリクエストを流す。tako app（IPC）と tailscale だけをスタブにしてあるので、
//! GUI も tailnet も無い CI で「認可が本当に効くか」を毎回検査できる。
//!
//! 隔離セルフテストや実機の通し検証はこれとは別に行うが、**認可の回帰**は
//! ここで止まるのが望ましい（実機は落ちていることがあるため）。

use serde_json::{json, Value};
use std::io::Read as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tako_control::remote_files;

/// 実ファイル木 + 実 HTTP サーバー
struct Harness {
    dir: PathBuf,
    port: u16,
    root_id: String,
    /// 監査ログへ流れた行（パスが漏れていないかを実測する）
    audit: Arc<Mutex<Vec<(String, Value)>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Harness {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("tako-1079-http-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.join("workspace");
        let outside = dir.join("private");
        std::fs::create_dir_all(root.join("docs")).expect("docs");
        std::fs::create_dir_all(&outside).expect("private");
        std::fs::write(root.join("readme.md"), "# 見出し\n本文\n").expect("readme");
        std::fs::write(root.join("docs").join("note.txt"), "note\n").expect("note");
        std::fs::write(outside.join("credentials"), "TOP SECRET\n").expect("secret");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("link-out")).expect("symlink");

        // app（IPC）のスタブ: ツリーに出ているのは workspace だけ
        let roots_payload = json!({
            "tabs": [{
                "tab": 1,
                "title": "作業",
                "roots": [root.display().to_string()],
            }]
        });
        let root_id = remote_files::roots_from_payload(&roots_payload)[0]
            .id
            .clone();

        let server = tiny_http::Server::http("127.0.0.1:0").expect("HTTP サーバー");
        let port = server.server_addr().to_ip().expect("ip").port();

        let audit: Arc<Mutex<Vec<(String, Value)>>> = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let audit_thread = Arc::clone(&audit);
        let stop_thread = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            while !stop_thread.load(std::sync::atomic::Ordering::Relaxed) {
                let Ok(Some(request)) = server.recv_timeout(std::time::Duration::from_millis(100))
                else {
                    continue;
                };
                let url = request.url().to_string();
                let path = url.split('?').next().unwrap_or("").to_string();
                let payload = roots_payload.clone();
                let audit_ref = Arc::clone(&audit_thread);
                let deps = remote_files::FilesDeps {
                    send: &|_req| Ok(payload.clone()),
                    audit: &|event, extra| {
                        audit_ref.lock().unwrap().push((event.to_string(), extra));
                    },
                    cors: Vec::new(),
                };
                remote_files::handle_files_request(request, &path, &url, &deps);
            }
        });

        Self {
            dir,
            port,
            root_id,
            audit,
            stop,
            handle: Some(handle),
        }
    }

    /// 生の HTTP を投げて (status, headers, body) を返す
    fn get(&self, target: &str) -> (u16, String, Vec<u8>) {
        use std::io::Write as _;
        let mut sock = std::net::TcpStream::connect(("127.0.0.1", self.port)).expect("connect");
        sock.set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .ok();
        write!(
            sock,
            "GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
        )
        .expect("送信");
        let mut raw = Vec::new();
        sock.read_to_end(&mut raw).expect("受信");
        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("ヘッダ終端");
        let head = String::from_utf8_lossy(&raw[..split]).to_string();
        let body = raw[split + 4..].to_vec();
        let status: u16 = head
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .expect("ステータス行");
        (status, head, body)
    }

    fn get_json(&self, target: &str) -> (u16, Value) {
        let (status, _, body) = self.get(target);
        let value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        (status, value)
    }

    fn files_url(&self, endpoint: &str, path: &str) -> String {
        format!("{endpoint}?root={}&path={}", self.root_id, urlencode(path))
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        assert!(
            self.dir.starts_with(std::env::temp_dir()),
            "一時ディレクトリ配下以外は消さない"
        );
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'/') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

// --- 受け入れ条件 1: ツリー外は 403 ---

#[test]
fn 実httpでツリー外のあらゆる形が403になる() {
    let h = Harness::new("deny");
    // 相対パスの各形（符号化して送るので、daemon 側が復号してから検査できているかも見る）
    let attacks = [
        "..",
        "../private/credentials",
        "docs/../../private/credentials",
        "..\\private\\credentials",
        "/etc/passwd",
        "C:\\Windows\\System32",
        "\\\\server\\share",
        // 先頭でない位置のドライブ形（Windows の `PathBuf::push` はここで全部捨てる）
        "docs/C:/Windows",
        "docs\\D:x",
    ];
    for attack in attacks {
        for endpoint in ["/api/files", "/api/files/content", "/api/files/download"] {
            let (status, body) = h.get_json(&h.files_url(endpoint, attack));
            assert_eq!(
                status, 403,
                "素通りした: {endpoint} path={attack:?} body={body}"
            );
            assert!(
                body["kind"].is_string(),
                "理由の種別が返らない: {endpoint} {attack:?}"
            );
        }
    }

    // ツリーに出ていないルート
    for endpoint in ["/api/files", "/api/files/content", "/api/files/download"] {
        let (status, _) = h.get_json(&format!("{endpoint}?root=deadbeef0000&path=readme.md"));
        assert_eq!(status, 403, "未知のルートが素通りした: {endpoint}");
    }

    // 秘密の中身が 1 バイトも漏れていない
    let (_, _, all) = h.get(&h.files_url("/api/files/download", "../private/credentials"));
    assert!(
        !String::from_utf8_lossy(&all).contains("TOP SECRET"),
        "秘密が応答に混ざった"
    );
}

#[cfg(unix)]
#[test]
fn 実httpでsymlink越えが403になる() {
    let h = Harness::new("symlink");
    for path in ["link-out", "link-out/credentials"] {
        for endpoint in ["/api/files", "/api/files/content", "/api/files/download"] {
            let (status, body) = h.get_json(&h.files_url(endpoint, path));
            assert_eq!(status, 403, "{endpoint} path={path} body={body}");
            assert_eq!(body["kind"], "escapes_root", "{endpoint} path={path}");
        }
    }
    // 一覧には出るが「押しても開けない」印が付く
    let (status, body) = h.get_json(&h.files_url("/api/files", ""));
    assert_eq!(status, 200);
    let link = body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "link-out")
        .expect("symlink が一覧に出る");
    assert_eq!(link["escapes_root"], true);
}

// --- 正常系 ---

#[test]
fn 実httpでルート一覧と中身が読める() {
    let h = Harness::new("ok");

    let (status, body) = h.get_json("/api/files");
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["roots"][0]["name"], "workspace");
    assert_eq!(body["roots"][0]["tab_title"], "作業");
    // ルート一覧に絶対パスを載せない（URL・履歴・スクショへ実パスを出さない）
    assert!(
        !body.to_string().contains(&h.dir.display().to_string()),
        "応答に実パスが載っている: {body}"
    );

    let (status, body) = h.get_json(&h.files_url("/api/files", ""));
    assert_eq!(status, 200, "{body}");
    let names: Vec<&str> = body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"readme.md"), "{names:?}");
    assert!(names.contains(&"docs"), "{names:?}");

    let (status, body) = h.get_json(&h.files_url("/api/files/content", "readme.md"));
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["text"], "# 見出し\n本文\n");
    assert_eq!(body["binary"], false);

    // 入れ子も辿れる
    let (status, body) = h.get_json(&h.files_url("/api/files/content", "docs/note.txt"));
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["text"], "note\n");
}

// --- 受け入れ条件 3 の土台: 保存できる形で返る ---

#[test]
fn ダウンロードは添付として本文をそのまま返す() {
    let h = Harness::new("download");
    let (status, head, body) = h.get(&h.files_url("/api/files/download", "readme.md"));
    assert_eq!(status, 200, "{head}");
    let lower = head.to_lowercase();
    assert!(
        lower.contains("content-disposition: attachment"),
        "添付として返っていない: {head}"
    );
    assert!(
        lower.contains("filename*=utf-8''"),
        "非 ASCII 名に耐える filename* が無い: {head}"
    );
    assert!(
        lower.contains("cache-control: no-store, private"),
        "機密がキャッシュされうる: {head}"
    );
    assert_eq!(
        String::from_utf8_lossy(&body),
        "# 見出し\n本文\n",
        "本文がバイト等価で返らない"
    );

    // 日本語のファイル名でもヘッダが壊れない
    std::fs::write(h.dir.join("workspace").join("報告書.txt"), "ok\n").unwrap();
    let (status, head, body) = h.get(&h.files_url("/api/files/download", "報告書.txt"));
    assert_eq!(status, 200, "{head}");
    assert_eq!(String::from_utf8_lossy(&body), "ok\n");
    assert!(head.to_lowercase().contains("filename*=utf-8''"), "{head}");
}

// --- 受け入れ条件 4: 監査にパスが出ない ---

#[test]
fn 実httpの監査行にパスが出ない() {
    let h = Harness::new("audit");
    let _ = h.get_json("/api/files");
    let _ = h.get_json(&h.files_url("/api/files", ""));
    let _ = h.get_json(&h.files_url("/api/files/content", "readme.md"));
    let _ = h.get(&h.files_url("/api/files/download", "docs/note.txt"));

    let rows = h.audit.lock().unwrap().clone();
    assert_eq!(rows.len(), 4, "4 操作ぶんの監査行が要る: {rows:?}");
    for (event, extra) in &rows {
        assert_eq!(event, "files");
        let text = extra.to_string();
        for leak in [
            "readme",
            "note",
            "docs",
            "workspace",
            "/",
            "\\",
            ".md",
            ".txt",
        ] {
            assert!(
                !text.contains(leak),
                "監査行にパスの断片 {leak:?} が出た: {text}"
            );
        }
    }
    // 種別と量は残っている（何も残さないのでは監査にならない）
    let kinds: Vec<&str> = rows
        .iter()
        .map(|(_, v)| v["kind"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(kinds, ["roots", "list", "content", "download"]);
    let download_bytes = rows[3].1["bytes"].as_u64().unwrap();
    assert_eq!(download_bytes, 5, "持ち出したバイト数は残す");
}
