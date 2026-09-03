//! ファイルの**書き込み** API を実 HTTP で通す e2e（#1084 / #1085）
//!
//! `handle_files_request` を実 `tiny_http::Server` に載せ、実ファイル木へ
//! 本物の `PUT` / `POST` を流す。tako app（IPC）は下の `FakeApp` に置き換える。
//!
//! # `FakeApp` は何を模しているか（そして何を模していないか）
//!
//! 模しているのは **daemon が依存している契約**だけ:
//!
//! - `PreviewEdit { enabled: Some(true) }` は編集セッションが無いときだけ作り、
//!   そのとき**ディスクの内容を基準（baseline）として覚える**
//! - `PreviewSave` は書く前にディスクと baseline を突き合わせ、違っていれば
//!   **書かずにエラー**（`TextBuffer::save` の `ExternalChanged`）
//! - `PreviewEdit { enabled: Some(false) }` は dirty でなければセッションを捨てる
//! - SSH 先は `open-file` の取得で**基準が進み**、押し出せなければ退避される（#966）
//!
//! 模していないのは GUI の見た目と実 SFTP。「Mac 側のプレビューに実際に反映されるか」は
//! 隔離 GUI + 実 dispatch で別に確かめる（この e2e の役目は
//! **daemon 側の認可と競合判定の回帰を CI で止めること**）。

use serde_json::{json, Value};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tako_control::protocol::Request;
use tako_control::remote_files;

// ---------------------------------------------------------------------------
// tako app のスタブ
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct FakePane {
    id: u64,
    /// プレビューしているローカルのパス（SSH 先は写しのパス）
    path: PathBuf,
    mode: String,
    editing: bool,
    /// 編集セッションの基準（`Some` = セッションあり）
    baseline: Option<Vec<u8>>,
    buffer: Option<String>,
    /// SSH 由来なら (host, リモートの絶対パス)
    remote: Option<(String, String)>,
    read_only: bool,
}

impl FakePane {
    fn dirty(&self) -> bool {
        match (&self.buffer, &self.baseline) {
            (Some(b), Some(base)) => b.as_bytes() != base.as_slice(),
            _ => false,
        }
    }
}

#[derive(Default)]
struct FakeAppState {
    panes: Vec<FakePane>,
    next_pane: u64,
    /// 押し出せていない保存（host, remote path, kind, error, **退避した内容**）。
    /// 実装は `remote_fs` の pending ストアへ内容ごと退避するので、
    /// 送り直しはペインのバッファではなく**これ**を送る（編集セッションは
    /// 保存が終わった時点で捨てられているため）
    pending: Vec<(String, String, String, String, Vec<u8>)>,
    /// SSH 先が落ちている（押し出しが失敗して退避される）
    disconnected: bool,
    /// リモート側の内容を横から書き換えた分（競合の実測用）
    remote_files: std::collections::HashMap<String, Vec<u8>>,
    /// `open-file` で取り込んだ時点の内容（#966 の baseline）
    remote_baseline: std::collections::HashMap<String, Vec<u8>>,
    /// 記録用: 呼ばれた Request の action 名
    calls: Vec<String>,
}

struct FakeApp {
    state: Mutex<FakeAppState>,
    /// ローカルのツリールート
    root: PathBuf,
    /// SFTP の写しの置き場
    cache: PathBuf,
    /// ツリーに出ている SSH 先（host, remote path）
    ssh_roots: Vec<(String, String)>,
}

impl FakeApp {
    fn roots_payload(&self) -> Value {
        json!({
            "tabs": [{
                "tab": 1,
                "title": "作業",
                "roots": [self.root.display().to_string()],
            }]
        })
    }

    fn remote_list_payload(&self) -> Value {
        let folders: Vec<Value> = self
            .ssh_roots
            .iter()
            .map(|(host, path)| {
                json!({
                    "host": host,
                    "path": path,
                    "label": format!("{host}:{path}"),
                    "state": "loaded",
                    "connected": !self.state.lock().unwrap().disconnected,
                    "origin": "explicit",
                    "placement": "leading",
                })
            })
            .collect();
        if folders.is_empty() {
            return json!({ "tabs": [], "loading_files": [] });
        }
        json!({
            "tabs": [{ "tab": 1, "title": "作業", "remote_folders": folders }],
            "loading_files": [],
        })
    }

    fn list_payload(&self) -> Value {
        let st = self.state.lock().unwrap();
        let panes: Vec<Value> = st
            .panes
            .iter()
            .map(|p| {
                json!({
                    "id": p.id,
                    "preview": {
                        "path": p.path.display().to_string(),
                        "mode": p.mode,
                        "editing": p.editing,
                        "dirty": p.dirty(),
                    },
                })
            })
            .collect();
        json!({ "tabs": [{ "id": 1, "title": "作業", "panes": panes }] })
    }

    fn mode_for(path: &Path) -> String {
        match path.extension().and_then(|e| e.to_str()) {
            Some("md") => "markdown".to_string(),
            Some("png") | Some("jpg") => "image".to_string(),
            Some("pdf") => "pdf".to_string(),
            _ => "code".to_string(),
        }
    }

    /// `OpenFile` 相当（既にそのパスを出しているペインがあれば再利用）
    fn open_file(&self, path: &Path, remote: Option<(String, String)>, read_only: bool) -> Value {
        let mut st = self.state.lock().unwrap();
        if let Some(p) = st.panes.iter().find(|p| p.path == path) {
            let (id, mode) = (p.id, p.mode.clone());
            return json!({ "tab": 1, "pane": id, "path": path.display().to_string(), "mode": mode, "created": false });
        }
        st.next_pane += 1;
        let id = st.next_pane;
        let mode = Self::mode_for(path);
        st.panes.push(FakePane {
            id,
            path: path.to_path_buf(),
            mode: mode.clone(),
            editing: false,
            baseline: None,
            buffer: None,
            remote,
            read_only,
        });
        json!({ "tab": 1, "pane": id, "path": path.display().to_string(), "mode": mode, "created": true })
    }

    /// #966 の `save_file` 相当（内容そのものでの競合検知 + 退避）
    fn push_remote(&self, host: &str, path: &str, bytes: &[u8]) -> Result<(), String> {
        let key = format!("{host}:{path}");
        let mut st = self.state.lock().unwrap();
        if st.disconnected {
            let err = "ssh: connect to host port 22: Operation timed out".to_string();
            st.pending.push((
                host.to_string(),
                path.to_string(),
                "unreachable".to_string(),
                err.clone(),
                bytes.to_vec(),
            ));
            return Err(err);
        }
        let current = st.remote_files.get(&key).cloned().unwrap_or_default();
        let baseline = st.remote_baseline.get(&key).cloned();
        match baseline {
            Some(base) if base != current => {
                let err = "リモート側の内容が開いた時点と違う".to_string();
                st.pending.push((
                    host.to_string(),
                    path.to_string(),
                    "conflict".to_string(),
                    err.clone(),
                    bytes.to_vec(),
                ));
                Err(err)
            }
            _ => {
                st.remote_files.insert(key.clone(), bytes.to_vec());
                st.remote_baseline.insert(key, bytes.to_vec());
                Ok(())
            }
        }
    }

    fn handle(&self, req: Request) -> Result<Value, String> {
        match req {
            Request::TreeFolder { ref action, .. } if action == "roots" => {
                self.state.lock().unwrap().calls.push("roots".into());
                Ok(self.roots_payload())
            }
            Request::List => {
                self.state.lock().unwrap().calls.push("list".into());
                Ok(self.list_payload())
            }
            Request::OpenFile { ref path, .. } => {
                self.state.lock().unwrap().calls.push("open_file".into());
                Ok(self.open_file(Path::new(path), None, false))
            }
            Request::PreviewEdit { pane, enabled } => {
                self.state.lock().unwrap().calls.push("preview_edit".into());
                let pane = pane.ok_or("pane が要る")?;
                let mut st = self.state.lock().unwrap();
                let p = st
                    .panes
                    .iter_mut()
                    .find(|p| p.id == pane)
                    .ok_or("プレビューペインではない")?;
                match enabled {
                    Some(true) => {
                        if p.read_only {
                            return Err("リモート側が読み取り専用".into());
                        }
                        if p.baseline.is_none() {
                            let bytes =
                                std::fs::read(&p.path).map_err(|e| format!("読み込めない: {e}"))?;
                            let text = String::from_utf8(bytes.clone()).map_err(|_| {
                                "UTF-8 テキストではないため編集できない".to_string()
                            })?;
                            p.baseline = Some(bytes);
                            p.buffer = Some(text);
                        }
                        p.editing = true;
                    }
                    Some(false) => {
                        p.editing = false;
                        if !p.dirty() {
                            p.baseline = None;
                            p.buffer = None;
                        }
                    }
                    None => {}
                }
                Ok(json!({ "pane": p.id, "editing": p.editing, "dirty": p.dirty() }))
            }
            Request::PreviewApply { pane, text } => {
                self.state
                    .lock()
                    .unwrap()
                    .calls
                    .push("preview_apply".into());
                let pane = pane.ok_or("pane が要る")?;
                let mut st = self.state.lock().unwrap();
                let p = st
                    .panes
                    .iter_mut()
                    .find(|p| p.id == pane)
                    .ok_or("プレビューペインではない")?;
                if p.baseline.is_none() {
                    return Err("編集モードを開始していない".into());
                }
                p.buffer = Some(text);
                Ok(json!({ "pane": p.id, "editing": p.editing, "dirty": p.dirty() }))
            }
            Request::PreviewSave { pane } => {
                self.state.lock().unwrap().calls.push("preview_save".into());
                let pane = pane.ok_or("pane が要る")?;
                // ローカルの写しへ書くところ（`TextBuffer::save` 相当）
                let (path, body, remote) = {
                    let st = self.state.lock().unwrap();
                    let p = st
                        .panes
                        .iter()
                        .find(|p| p.id == pane)
                        .ok_or("プレビューペインではない")?;
                    let base = p.baseline.clone().ok_or("編集モードを開始していない")?;
                    let body = p.buffer.clone().unwrap_or_default();
                    let current = std::fs::read(&p.path).map_err(|e| format!("{e}"))?;
                    if current != base {
                        return Err("ファイルが外部で変更されたため保存しなかった".into());
                    }
                    (p.path.clone(), body, p.remote.clone())
                };
                std::fs::write(&path, body.as_bytes()).map_err(|e| format!("{e}"))?;
                {
                    let mut st = self.state.lock().unwrap();
                    if let Some(p) = st.panes.iter_mut().find(|p| p.id == pane) {
                        p.baseline = Some(body.as_bytes().to_vec());
                    }
                }
                // #966: リモート由来なら**リモートへ書けるまでが保存**
                let mut out =
                    json!({ "pane": pane, "editing": true, "dirty": false, "saved": true });
                if let Some((host, rpath)) = remote {
                    self.push_remote(&host, &rpath, body.as_bytes())?;
                    out["remote"] = json!({
                        "host": host,
                        "path": rpath,
                        "label": format!("{host}:{rpath}"),
                        "state": "saved",
                        "read_only": false,
                        "pending_write": false,
                        "bytes": body.len(),
                        "conflict_checked": true,
                    });
                }
                Ok(out)
            }
            Request::RemoteFolder {
                ref action,
                ref host,
                ref path,
                ..
            } => {
                self.state
                    .lock()
                    .unwrap()
                    .calls
                    .push(format!("remote_folder:{action}"));
                match action.as_str() {
                    "list" => Ok(self.remote_list_payload()),
                    "ls" => {
                        let host = host.clone().ok_or("host が要る")?;
                        let dir = path.clone().ok_or("path が要る")?;
                        let st = self.state.lock().unwrap();
                        let prefix = format!("{host}:{}/", dir.trim_end_matches('/'));
                        let mut entries: Vec<Value> = Vec::new();
                        for key in st.remote_files.keys() {
                            if let Some(rest) = key.strip_prefix(&prefix) {
                                if rest.contains('/') {
                                    continue;
                                }
                                entries.push(json!({
                                    "name": rest,
                                    "path": format!("{}/{rest}", dir.trim_end_matches('/')),
                                    "kind": "file",
                                    "size": st.remote_files[key].len(),
                                }));
                            }
                        }
                        entries.sort_by_key(|e| e["name"].as_str().unwrap_or_default().to_string());
                        Ok(json!({ "host": host, "path": dir, "entries": entries }))
                    }
                    "open-file" => {
                        let host = host.clone().ok_or("host が要る")?;
                        let rpath = path.clone().ok_or("path が要る")?;
                        let key = format!("{host}:{rpath}");
                        let bytes = {
                            let st = self.state.lock().unwrap();
                            st.remote_files
                                .get(&key)
                                .cloned()
                                .ok_or_else(|| "そのファイルは無い".to_string())?
                        };
                        // 取得のたびに基準が進む（実装と同じ = 読み直せば競合が解ける）
                        let local = self.cache.join(remote_files::root_id_of(&key));
                        std::fs::create_dir_all(&self.cache).map_err(|e| format!("{e}"))?;
                        std::fs::write(&local, &bytes).map_err(|e| format!("{e}"))?;
                        self.state
                            .lock()
                            .unwrap()
                            .remote_baseline
                            .insert(key.clone(), bytes.clone());
                        let read_only = rpath.ends_with(".ro");
                        let mut out =
                            self.open_file(&local, Some((host.clone(), rpath.clone())), read_only);
                        // 種別はリモートの名前で決める（写しの名前は id なので拡張子が無い）
                        out["mode"] = json!(FakeApp::mode_for(Path::new(&rpath)));
                        if let Some(pane) = out["pane"].as_u64() {
                            let mut st = self.state.lock().unwrap();
                            if let Some(p) = st.panes.iter_mut().find(|p| p.id == pane) {
                                p.mode = out["mode"].as_str().unwrap_or("code").to_string();
                                p.read_only = read_only;
                            }
                        }
                        out["host"] = json!(host);
                        out["remote_path"] = json!(rpath);
                        out["cached_path"] = json!(local.display().to_string());
                        out["read_only"] = json!(read_only);
                        out["size"] = json!(bytes.len());
                        out["pending_write"] = json!(self
                            .state
                            .lock()
                            .unwrap()
                            .pending
                            .iter()
                            .any(|(h, p, _, _, _)| h == &host && p == &rpath));
                        Ok(out)
                    }
                    "pending" => {
                        let st = self.state.lock().unwrap();
                        let entries: Vec<Value> = st
                            .pending
                            .iter()
                            .filter(|(h, _, _, _, _)| {
                                host.as_deref().map(|x| x == h).unwrap_or(true)
                            })
                            .filter(|(_, p, _, _, _)| {
                                path.as_deref().map(|x| x == p).unwrap_or(true)
                            })
                            .map(|(h, p, kind, err, body)| {
                                json!({
                                    "host": h,
                                    "path": p,
                                    "label": format!("{h}:{p}"),
                                    "kind": kind,
                                    "error": err,
                                    "at": 0,
                                    "attempts": 1,
                                    "size": body.len(),
                                })
                            })
                            .collect();
                        Ok(json!({ "pending": entries }))
                    }
                    "push" => {
                        // 送り直す対象と**退避してあった内容**（実装と同じ経路）
                        let targets: Vec<(String, String, Vec<u8>)> = {
                            let st = self.state.lock().unwrap();
                            st.pending
                                .iter()
                                .filter(|(h, _, _, _, _)| {
                                    host.as_deref().map(|x| x == h).unwrap_or(true)
                                })
                                .filter(|(_, p, _, _, _)| {
                                    path.as_deref().map(|x| x == p).unwrap_or(true)
                                })
                                .map(|(h, p, _, _, body)| (h.clone(), p.clone(), body.clone()))
                                .collect()
                        };
                        if targets.is_empty() {
                            return Err("押し出せていない保存はありません".into());
                        }
                        let mut pushed = 0;
                        for (h, p, body) in &targets {
                            // 退避を消してから送る（実装と同じ順。失敗すれば
                            // `push_remote` が退避を作り直す）
                            self.state
                                .lock()
                                .unwrap()
                                .pending
                                .retain(|(ph, pp, _, _, _)| !(ph == h && pp == p));
                            self.push_remote(h, p, body)?;
                            pushed += 1;
                        }
                        Ok(json!({ "pushed": pushed }))
                    }
                    other => Err(format!("不明な action: {other}")),
                }
            }
            other => Err(format!("スタブが受けていない Request: {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------
// 実 HTTP のハーネス
// ---------------------------------------------------------------------------

struct Harness {
    dir: PathBuf,
    port: u16,
    app: Arc<FakeApp>,
    root_id: String,
    audit: Arc<Mutex<Vec<(String, Value)>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Harness {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tako-1084-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.join("workspace");
        std::fs::create_dir_all(root.join("docs")).expect("docs");
        std::fs::write(root.join("note.txt"), "1 行目\n").expect("note");
        std::fs::write(root.join("readme.md"), "# 見出し\n").expect("readme");
        std::fs::write(root.join("shot.png"), [0x89, 0x50, 0x4e, 0x47]).expect("png");
        std::fs::write(root.join("blob.bin"), [0u8, 1, 2, 3]).expect("bin");

        let app = Arc::new(FakeApp {
            state: Mutex::new(FakeAppState {
                remote_files: [
                    (
                        "linuxbox:/srv/app/config.toml".to_string(),
                        b"port = 8080\n".to_vec(),
                    ),
                    (
                        "linuxbox:/srv/app/notes.md".to_string(),
                        "# リモート\n".as_bytes().to_vec(),
                    ),
                    ("linuxbox:/srv/app/locked.ro".to_string(), b"ro\n".to_vec()),
                    ("winbox:/C:/work/app.txt".to_string(), b"windows\n".to_vec()),
                ]
                .into_iter()
                .collect(),
                ..Default::default()
            }),
            root: root.clone(),
            cache: dir.join("remote-cache"),
            ssh_roots: vec![
                ("linuxbox".to_string(), "/srv/app".to_string()),
                ("winbox".to_string(), "/C:/work".to_string()),
            ],
        });
        let root_id = remote_files::roots_from_payload(&app.roots_payload())[0]
            .id
            .clone();

        let server = tiny_http::Server::http("127.0.0.1:0").expect("HTTP サーバー");
        let port = server.server_addr().to_ip().expect("ip").port();
        let audit: Arc<Mutex<Vec<(String, Value)>>> = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let app_thread = Arc::clone(&app);
        let audit_thread = Arc::clone(&audit);
        let stop_thread = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            while !stop_thread.load(std::sync::atomic::Ordering::Relaxed) {
                let Ok(Some(request)) = server.recv_timeout(std::time::Duration::from_millis(50))
                else {
                    continue;
                };
                let url = request.url().to_string();
                let path = url.split('?').next().unwrap_or("").to_string();
                let app_ref = Arc::clone(&app_thread);
                let audit_ref = Arc::clone(&audit_thread);
                let deps = remote_files::FilesDeps {
                    send: &move |req| app_ref.handle(req),
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
            app,
            root_id,
            audit,
            stop,
            handle: Some(handle),
        }
    }

    fn request(&self, method: &str, target: &str, body: Option<&str>) -> (u16, Vec<u8>) {
        use std::io::Write as _;
        let mut sock = std::net::TcpStream::connect(("127.0.0.1", self.port)).expect("connect");
        sock.set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .ok();
        let body = body.unwrap_or("");
        write!(
            sock,
            "{method} {target} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .expect("送信");
        let mut raw = Vec::new();
        sock.read_to_end(&mut raw).expect("受信");
        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("ヘッダ終端");
        let head = String::from_utf8_lossy(&raw[..split]).to_string();
        let status: u16 = head
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .expect("ステータス行");
        (status, raw[split + 4..].to_vec())
    }

    fn json(&self, method: &str, target: &str, body: Option<&str>) -> (u16, Value) {
        let (status, raw) = self.request(method, target, body);
        (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
    }

    fn get(&self, target: &str) -> (u16, Value) {
        self.json("GET", target, None)
    }

    /// ローカルのファイルを読んで (etag, 本文) を返す
    fn read(&self, rel: &str) -> (String, String) {
        let (status, body) = self.get(&format!(
            "/api/files/content?root={}&path={}",
            self.root_id,
            urlencode(rel)
        ));
        assert_eq!(status, 200, "読み出し: {body}");
        (
            body["etag"].as_str().unwrap_or_default().to_string(),
            body["text"].as_str().unwrap_or_default().to_string(),
        )
    }

    fn put(&self, rel: &str, text: &str, etag: &str) -> (u16, Value) {
        self.json(
            "PUT",
            &format!(
                "/api/files/content?root={}&path={}",
                self.root_id,
                urlencode(rel)
            ),
            Some(&json!({ "text": text, "etag": etag }).to_string()),
        )
    }

    fn ssh_root_id(&self, host: &str) -> String {
        remote_files::ssh_roots_from_payload(&self.app.remote_list_payload())
            .into_iter()
            .find(|r| r.host == host)
            .map(|r| r.id)
            .expect("SSH 先ルート")
    }

    fn disk(&self, rel: &str) -> String {
        std::fs::read_to_string(self.dir.join("workspace").join(rel)).expect("実ファイル")
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

// ---------------------------------------------------------------------------
// #1084 受け入れ条件 1: 編集 → 保存 → 反映
// ---------------------------------------------------------------------------

#[test]
fn 実httpで編集して保存するとファイルとプレビューへ反映される() {
    let h = Harness::new("save");
    let (etag, text) = h.read("note.txt");
    assert_eq!(text, "1 行目\n");

    let (status, body) = h.put("note.txt", "1 行目\n2 行目を足した\n", &etag);
    assert_eq!(status, 200, "保存できる: {body}");
    assert_eq!(body["saved"].as_bool(), Some(true));
    assert_eq!(
        body["dirty"].as_bool(),
        Some(false),
        "保存後は未保存が残らない"
    );
    // 実ファイルが書き換わっている（「書けるまで成功と言わない」）
    assert_eq!(h.disk("note.txt"), "1 行目\n2 行目を足した\n");

    // PC 側の編集経路を通っている（PreviewEdit → Apply → Save の順）
    let calls = h.app.state.lock().unwrap().calls.clone();
    let seq: Vec<&str> = calls
        .iter()
        .map(String::as_str)
        .filter(|c| c.starts_with("preview_"))
        .collect();
    assert_eq!(
        seq,
        vec![
            "preview_edit",
            "preview_edit",
            "preview_apply",
            "preview_save",
            "preview_edit"
        ],
        "状態照会 → 編集 ON → 適用 → 保存 → 編集 OFF: {calls:?}"
    );
    // Mac 側のプレビューがそのファイルを出している
    // （**ロックはここで手放す**: 掴んだまま HTTP を投げるとサーバースレッドが
    //   同じ Mutex を待って自分で詰まる）
    let pane = body["pane"].as_u64().expect("ペイン");
    {
        let st = h.app.state.lock().unwrap();
        let p = st.panes.iter().find(|p| p.id == pane).expect("ペイン実在");
        assert!(p.path.ends_with("note.txt"));
        assert!(!p.editing, "スマホからの保存で編集モードのまま残さない");
    }

    // 応答の検証子で続けて保存できる（読み直さずに 2 回目が通る）
    let etag2 = body["etag"].as_str().expect("新しい検証子").to_string();
    let (status, body) = h.put("note.txt", "3 行目\n", &etag2);
    assert_eq!(status, 200, "続けて保存できる: {body}");
    assert_eq!(h.disk("note.txt"), "3 行目\n");
}

#[test]
fn 保存はmacのプレビューペインを新しく奪わない() {
    let h = Harness::new("reuse");
    let (etag, _) = h.read("note.txt");
    let (status, first) = h.put("note.txt", "a\n", &etag);
    assert_eq!(status, 200);
    let pane = first["pane"].as_u64().expect("ペイン");

    // 2 回目は同じペインを使う（`OpenFile` を呼び直してレイアウトを触らない）
    let (etag2, _) = h.read("note.txt");
    let (status, second) = h.put("note.txt", "b\n", &etag2);
    assert_eq!(status, 200);
    assert_eq!(second["pane"].as_u64(), Some(pane), "同じペインを使い回す");
    assert_eq!(
        h.app.state.lock().unwrap().panes.len(),
        1,
        "ペインが増えない"
    );
    let opens = h
        .app
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .filter(|c| *c == "open_file")
        .count();
    assert_eq!(opens, 1, "OpenFile は最初の 1 回だけ");
}

// ---------------------------------------------------------------------------
// #1084 受け入れ条件 2: 競合時に上書きしない
// ---------------------------------------------------------------------------

#[test]
fn 実httpで競合したら上書きせず409を返す() {
    let h = Harness::new("conflict");
    let (etag, _) = h.read("note.txt");

    // スマホが読んだあとに Mac 側（他の誰か）が書き換えた
    std::fs::write(h.dir.join("workspace").join("note.txt"), "他で変更\n").expect("横から変更");

    let (status, body) = h.put("note.txt", "スマホの編集\n", &etag);
    assert_eq!(status, 409, "競合は 409: {body}");
    assert_eq!(body["kind"].as_str(), Some("conflict"));
    // **1 バイトも上書きしていない**
    assert_eq!(
        h.disk("note.txt"),
        "他で変更\n",
        "他の変更を踏み潰していない"
    );
    // 編集セッションも作られていない（Mac 側の状態を変えない）
    assert!(
        h.app.state.lock().unwrap().panes.iter().all(|p| !p.editing),
        "競合のときは編集モードにしない"
    );

    // 読み直せば保存できる（次の一手が実際に通る）
    let (etag2, text) = h.read("note.txt");
    assert_eq!(text, "他で変更\n");
    let (status, _) = h.put("note.txt", "読み直して編集\n", &etag2);
    assert_eq!(status, 200);
    assert_eq!(h.disk("note.txt"), "読み直して編集\n");
}

#[test]
fn 検証子が無ければ書かない() {
    let h = Harness::new("noetag");
    let target = format!("/api/files/content?root={}&path=note.txt", h.root_id);
    for body in [
        json!({ "text": "x\n" }).to_string(),
        json!({ "text": "x\n", "etag": "" }).to_string(),
    ] {
        let (status, out) = h.json("PUT", &target, Some(&body));
        assert_eq!(status, 400, "{out}");
        assert_eq!(out["kind"].as_str(), Some("missing_etag"));
    }
    // 形が違う検証子も競合として扱う（当てずっぽうで上書きさせない）
    let (status, out) = h.json(
        "PUT",
        &target,
        Some(&json!({ "text": "x\n", "etag": "0-0000000000000000" }).to_string()),
    );
    assert_eq!(status, 409, "{out}");
    assert_eq!(out["kind"].as_str(), Some("conflict"));
    assert_eq!(h.disk("note.txt"), "1 行目\n", "書かれていない");
}

#[test]
fn mac側に未保存の編集があれば踏み潰さない() {
    let h = Harness::new("busy");
    let (etag, _) = h.read("note.txt");
    // Mac 側で編集中（未保存）にする
    let opened = h
        .app
        .open_file(&h.dir.join("workspace").join("note.txt"), None, false);
    let pane = opened["pane"].as_u64().expect("ペイン");
    h.app
        .handle(Request::PreviewEdit {
            pane: Some(pane),
            enabled: Some(true),
        })
        .expect("編集 ON");
    h.app
        .handle(Request::PreviewApply {
            pane: Some(pane),
            text: "Mac で書きかけ\n".into(),
        })
        .expect("適用");

    let (status, body) = h.put("note.txt", "スマホの編集\n", &etag);
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["kind"].as_str(), Some("busy_editing"));
    assert_eq!(h.disk("note.txt"), "1 行目\n", "ディスクも変えない");
    // Mac 側の書きかけが残っている
    let st = h.app.state.lock().unwrap();
    let p = st.panes.iter().find(|p| p.id == pane).expect("ペイン");
    assert_eq!(p.buffer.as_deref(), Some("Mac で書きかけ\n"));
    drop(st);
}

// ---------------------------------------------------------------------------
// #1084: 書けないものは書かない
// ---------------------------------------------------------------------------

#[test]
fn テキストでないものは編集を断る() {
    let h = Harness::new("nottext");
    // バイナリは読み出しの時点で検証子が付かない → 書き込みも通らない
    let (status, body) = h.get(&format!(
        "/api/files/content?root={}&path=blob.bin",
        h.root_id
    ));
    assert_eq!(status, 200);
    assert_eq!(body["binary"].as_bool(), Some(true));
    assert!(body["etag"].is_null());

    let (status, body) = h.put("blob.bin", "text\n", "0-0000000000000000");
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["kind"].as_str(), Some("not_text"));

    // 画像はテキストとして読めるとしても種別で断る（プレビューが壊れない）
    let (status, body) = h.put("shot.png", "text\n", "0-0000000000000000");
    assert!(matches!(status, 400 | 409), "{status}: {body}");
}

#[test]
fn ツリー外への書き込みは全部403() {
    let h = Harness::new("denywrite");
    let attacks = [
        "../escaped.txt",
        "docs/../../escaped.txt",
        "/etc/passwd",
        "C:/Windows/x.txt",
        "docs/C:/x.txt",
        "\\\\server\\share",
    ];
    for rel in attacks {
        let (status, body) = h.put(rel, "侵入\n", "0-0000000000000000");
        assert_eq!(status, 403, "{rel} は 403: {body}");
    }
    // 未知のルートも 403
    let (status, body) = h.json(
        "PUT",
        "/api/files/content?root=deadbeef0000&path=note.txt",
        Some(&json!({ "text": "x", "etag": "1-1" }).to_string()),
    );
    assert_eq!(status, 403, "{body}");
    // 1 つも書かれていない
    assert_eq!(h.disk("note.txt"), "1 行目\n");
    assert!(!h.dir.join("escaped.txt").exists());
}

#[test]
fn 受け持つパスの違うメソッドは405() {
    let h = Harness::new("method");
    let (status, body) = h.json("PUT", "/api/files", Some("{}"));
    assert_eq!(status, 405, "{body}");
    assert_eq!(body["kind"].as_str(), Some("method_not_allowed"));
}

#[test]
fn 監査にはパスが載らない() {
    let h = Harness::new("audit");
    let (etag, _) = h.read("note.txt");
    h.put("note.txt", "x\n", &etag);
    h.put("note.txt", "y\n", "0-0000000000000000");
    let rows = h.audit.lock().unwrap().clone();
    assert!(rows.len() >= 3, "監査行が出ている: {rows:?}");
    let kinds: Vec<String> = rows
        .iter()
        .map(|(_, v)| v["kind"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(kinds.contains(&"write".to_string()), "{kinds:?}");
    assert!(kinds.contains(&"write_denied".to_string()), "{kinds:?}");
    let dumped =
        serde_json::to_string(&rows.iter().map(|(_, v)| v).collect::<Vec<_>>()).expect("JSON");
    for leak in ["note.txt", "workspace", "1 行目"] {
        assert!(
            !dumped.contains(leak),
            "監査に {leak} が漏れている: {dumped}"
        );
    }
}

// ---------------------------------------------------------------------------
// #1085 受け入れ条件 1: SSH 先のプレビュー・編集・アップロード
// ---------------------------------------------------------------------------

#[test]
fn ssh先のルートが一覧に並ぶ() {
    let h = Harness::new("sshroots");
    let (status, body) = h.get("/api/files");
    assert_eq!(status, 200, "{body}");
    let roots = body["roots"].as_array().expect("ルート一覧");
    // SSH 先（placement=leading）→ ローカル の順（#1041 の並びを app の答えのまま使う）
    assert_eq!(roots.len(), 3, "{roots:?}");
    assert_eq!(roots[0]["ssh"].as_bool(), Some(true));
    assert_eq!(roots[0]["host"].as_str(), Some("linuxbox"));
    assert_eq!(roots[0]["name"].as_str(), Some("app"));
    assert_eq!(roots[1]["host"].as_str(), Some("winbox"));
    assert_eq!(roots[2]["ssh"].as_bool(), Some(false), "ローカルは後ろ");
    assert_eq!(roots[2]["name"].as_str(), Some("workspace"));
}

#[test]
fn ssh先のディレクトリとファイルが読める() {
    let h = Harness::new("sshread");
    let id = h.ssh_root_id("linuxbox");

    let (status, body) = h.get(&format!("/api/files?root={id}&path="));
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["ssh"].as_bool(), Some(true));
    assert_eq!(body["host"].as_str(), Some("linuxbox"));
    let names: Vec<&str> = body["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert!(names.contains(&"config.toml"), "{names:?}");

    let (status, body) = h.get(&format!("/api/files/content?root={id}&path=config.toml"));
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["text"].as_str(), Some("port = 8080\n"));
    assert_eq!(body["ssh"].as_bool(), Some(true));
    assert!(body["etag"].as_str().is_some(), "検証子が載る");
    assert_eq!(body["read_only"].as_bool(), Some(false));

    // Windows の相手（`/C:/...`）も同じ形で読める
    let win = h.ssh_root_id("winbox");
    let (status, body) = h.get(&format!("/api/files/content?root={win}&path=app.txt"));
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["text"].as_str(), Some("windows\n"));
}

#[test]
fn ssh先のファイルを編集して押し出せる() {
    let h = Harness::new("sshwrite");
    let id = h.ssh_root_id("linuxbox");
    let (status, read) = h.get(&format!("/api/files/content?root={id}&path=config.toml"));
    assert_eq!(status, 200, "{read}");
    let etag = read["etag"].as_str().expect("検証子").to_string();

    let (status, body) = h.json(
        "PUT",
        &format!("/api/files/content?root={id}&path=config.toml"),
        Some(&json!({ "text": "port = 9090\n", "etag": etag }).to_string()),
    );
    assert_eq!(status, 200, "押し出せる: {body}");
    assert_eq!(body["saved"].as_bool(), Some(true));
    // #966 の書き戻し状態が応答に載る（ローカルの写しへ書けただけでは成功と言わない）
    assert_eq!(body["remote"]["state"].as_str(), Some("saved"));
    assert_eq!(body["remote"]["conflict_checked"].as_bool(), Some(true));
    // リモート側の実体が書き換わっている
    assert_eq!(
        h.app
            .state
            .lock()
            .unwrap()
            .remote_files
            .get("linuxbox:/srv/app/config.toml")
            .map(|b| String::from_utf8_lossy(b).to_string()),
        Some("port = 9090\n".to_string())
    );
}

#[test]
fn ssh先の競合は上書きせず読み直しへ倒す() {
    let h = Harness::new("sshconflict");
    let id = h.ssh_root_id("linuxbox");
    let (_, read) = h.get(&format!("/api/files/content?root={id}&path=config.toml"));
    let etag = read["etag"].as_str().expect("検証子").to_string();

    // スマホが読んだあとにリモート側が変わった
    h.app.state.lock().unwrap().remote_files.insert(
        "linuxbox:/srv/app/config.toml".into(),
        b"port = 1234\n".to_vec(),
    );

    let (status, body) = h.json(
        "PUT",
        &format!("/api/files/content?root={id}&path=config.toml"),
        Some(&json!({ "text": "port = 9090\n", "etag": etag }).to_string()),
    );
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["kind"].as_str(), Some("conflict"));
    assert_eq!(
        h.app
            .state
            .lock()
            .unwrap()
            .remote_files
            .get("linuxbox:/srv/app/config.toml")
            .map(|b| String::from_utf8_lossy(b).to_string()),
        Some("port = 1234\n".to_string()),
        "踏み潰していない"
    );
}

#[test]
fn ssh先の読み取り専用は編集を断る() {
    let h = Harness::new("sshro");
    let id = h.ssh_root_id("linuxbox");
    let (status, read) = h.get(&format!("/api/files/content?root={id}&path=locked.ro"));
    assert_eq!(status, 200, "{read}");
    assert_eq!(
        read["read_only"].as_bool(),
        Some(true),
        "読み取り専用が見える"
    );

    let (status, body) = h.json(
        "PUT",
        &format!("/api/files/content?root={id}&path=locked.ro"),
        Some(&json!({ "text": "書き換え\n", "etag": read["etag"] }).to_string()),
    );
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["kind"].as_str(), Some("read_only"));
}

#[test]
fn ssh先のツリー外は403で閉じている() {
    let h = Harness::new("sshdeny");
    let id = h.ssh_root_id("linuxbox");
    for rel in ["..", "../../etc/passwd", "/etc/passwd", "sub/../../x"] {
        let (status, body) = h.get(&format!(
            "/api/files/content?root={id}&path={}",
            urlencode(rel)
        ));
        assert_eq!(status, 403, "{rel}: {body}");
        let (status, body) = h.json(
            "PUT",
            &format!("/api/files/content?root={id}&path={}", urlencode(rel)),
            Some(&json!({ "text": "x", "etag": "1-1" }).to_string()),
        );
        assert_eq!(status, 403, "{rel}: {body}");
    }
}

// ---------------------------------------------------------------------------
// #1085 受け入れ条件 2: 切断中の保存が pending → 復帰後に push
// ---------------------------------------------------------------------------

#[test]
fn 切断中の保存は退避され復帰後に送り直せる() {
    let h = Harness::new("sshpending");
    let id = h.ssh_root_id("linuxbox");
    let (_, read) = h.get(&format!("/api/files/content?root={id}&path=notes.md"));
    let etag = read["etag"].as_str().expect("検証子").to_string();

    // 回線が落ちた
    h.app.state.lock().unwrap().disconnected = true;
    let (status, body) = h.json(
        "PUT",
        &format!("/api/files/content?root={id}&path=notes.md"),
        Some(&json!({ "text": "# 切断中に書いた\n", "etag": etag }).to_string()),
    );
    assert_eq!(status, 502, "書けていないことを成功と言わない: {body}");
    assert_eq!(body["kind"].as_str(), Some("remote_pending"));
    assert_eq!(body["pending"].as_bool(), Some(true), "退避された");

    // 退避が一覧で見える
    let (status, pending) = h.get("/api/files/pending");
    assert_eq!(status, 200, "{pending}");
    let entries = pending["pending"].as_array().expect("pending");
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["host"].as_str(), Some("linuxbox"));
    assert_eq!(entries[0]["kind"].as_str(), Some("unreachable"));

    // 回線が戻ったら送り直せる（`force` は送っていない）
    h.app.state.lock().unwrap().disconnected = false;
    let (status, pushed) = h.json(
        "POST",
        &format!("/api/files/push?root={id}&path=notes.md"),
        Some("{}"),
    );
    assert_eq!(status, 200, "{pushed}");
    assert_eq!(
        h.app
            .state
            .lock()
            .unwrap()
            .remote_files
            .get("linuxbox:/srv/app/notes.md")
            .map(|b| String::from_utf8_lossy(b).to_string()),
        Some("# 切断中に書いた\n".to_string()),
        "内容が失われず届いた"
    );
    let (_, pending) = h.get("/api/files/pending");
    assert!(
        pending["pending"]
            .as_array()
            .map(Vec::is_empty)
            .unwrap_or(false),
        "送れたら退避は消える: {pending}"
    );
}

#[test]
fn 送り直す先が無ければ理由を返す() {
    let h = Harness::new("nopending");
    let (status, body) = h.json("POST", "/api/files/push", Some("{}"));
    assert_eq!(status, 502, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("押し出せていない"),
        "理由がそのまま出る: {body}"
    );
}

#[test]
fn 押し出しにforceは渡らない() {
    // #1085: スマホから競合を踏み潰す操作は出さない。
    // ボディに force を入れても無視される（受け取るキーは root / path だけ）
    let h = Harness::new("noforce");
    let id = h.ssh_root_id("linuxbox");
    let (_, read) = h.get(&format!("/api/files/content?root={id}&path=notes.md"));
    let etag = read["etag"].as_str().expect("検証子").to_string();
    h.app.state.lock().unwrap().disconnected = true;
    h.json(
        "PUT",
        &format!("/api/files/content?root={id}&path=notes.md"),
        Some(&json!({ "text": "退避される\n", "etag": etag }).to_string()),
    );
    // 競合を作ってから force つきで送り直す
    h.app.state.lock().unwrap().disconnected = false;
    h.app.state.lock().unwrap().remote_files.insert(
        "linuxbox:/srv/app/notes.md".into(),
        b"remote changed\n".to_vec(),
    );
    let (status, body) = h.json(
        "POST",
        &format!("/api/files/push?root={id}&path=notes.md"),
        Some(&json!({ "force": true }).to_string()),
    );
    assert_ne!(status, 200, "force を効かせない: {body}");
    assert_eq!(
        h.app
            .state
            .lock()
            .unwrap()
            .remote_files
            .get("linuxbox:/srv/app/notes.md")
            .map(|b| String::from_utf8_lossy(b).to_string()),
        Some("remote changed\n".to_string()),
        "リモートの変更が残っている"
    );
}
