//! 実経路の番犬: `TAKO_ISOLATED=1` の CLI は既定（本番）の置き場の発見ファイルから GUI へ繋がない（Issue #2531）。
//!
//! 修正前（実測）: `TAKO_SOCKET` が無い（か繋がらない）とき、CLI は `discovery::read_candidates()` で
//! `<TAKO_DISCOVERY_DIR か data dir>/control.json` を読み、data dir の既定（本番の置き場）に居る GUI へ
//! 縮退して繋いでいた。`TAKO_ISOLATED=1` が置き場を差し替えるのは tako-app の起動時だけだった。
//!
//! ここでは**ビルドした `tako` をそのまま**叩く。一時の HOME の既定の置き場に「本番役」の
//! 接続情報（このテストが張る偽の受け口を指す）を置き、受け口が受けた接続の数で判定する:
//! - 隔離の印あり・置き場の指定なし → 接続 0・断りで止まる（日本語 / 英語。英語に日本語が混ざらない）
//! - 置き場（`TAKO_DATA_DIR` / `TAKO_DISCOVERY_DIR`）を明示 → 従来どおり繋がる
//! - 隔離の印なし → 従来どおり既定の置き場から繋がる
//! - 順番待ち（`tako queue`）→ 隔離の印があっても既定の置き場の調停役へ並ぶ（マシン全体の 1 つ）
//!
//! 落ちたときは判定の正本（`discovery.rs` の `read_candidates` / `ipc.rs` の `send_request_classified`）を
//! file:line で名指す。A/B は `TAKO_2531_LEGACY=1`（同じバイナリで修正前の解決へ戻す。[`legacy_腕は既定の置き場へ繋がる`]）。
//! 既定の置き場は HOME から決まる unix だけで走らせる（Windows は `%APPDATA%`。判定は同じ 1 実装）

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// 叩くバイナリ（この木でビルドした `tako`）
fn tako() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tako"))
}

/// リポジトリの中のファイルの `fn <名前>(` の行（落ちたときの名指し用）
fn fn_line(rel: &str, name: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let text = std::fs::read_to_string(root.join(rel)).unwrap_or_default();
    let needle = format!("fn {name}(");
    match text.lines().position(|l| l.contains(&needle)) {
        Some(i) => format!("{rel}:{}（fn {name}）", i + 1),
        None => format!("{rel}（fn {name} が見つからない）"),
    }
}

fn gate_sites() -> String {
    format!(
        "{} / {}",
        fn_line("crates/tako-control/src/discovery.rs", "read_candidates"),
        fn_line("crates/tako-cli/src/ipc.rs", "send_request_classified")
    )
}

/// 一時の置き場一式（ソケットのパス長の上限があるので短い名前にする。終わったら消す）
struct Scratch {
    dir: PathBuf,
    accepted: Arc<AtomicUsize>,
    socket: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tk2531{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("一時 dir を作れる");
        let socket = dir.join("p.sock");
        let accepted = Arc::new(AtomicUsize::new(0));
        let listener = UnixListener::bind(&socket).expect("本番役の受け口を張れる");
        let counter = Arc::clone(&accepted);
        // 本番役の GUI の代わり: 受けた接続を数え、1 行の要求へ空の一覧で答える
        // （順番待ちの要求には「割り当て済み」= 待たずに走らせる。`state` / `share` は一覧には無害）
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                counter.fetch_add(1, Ordering::SeqCst);
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(&stream);
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() {
                        return;
                    }
                    let id = serde_json::from_str::<serde_json::Value>(&line)
                        .ok()
                        .and_then(|v| v["id"].as_u64())
                        .unwrap_or(0);
                    let reply = format!(
                        "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":\
                         {{\"tabs\":[],\"state\":\"granted\",\"share\":0}}}}\n"
                    );
                    let _ = (&stream).write_all(reply.as_bytes());
                });
            }
        });
        let scratch = Self {
            dir,
            accepted,
            socket,
        };
        // 一時 HOME の既定の置き場（= 本番の置き場の役）と、明示する側の置き場の両方に同じ接続情報
        scratch.write_control(&scratch.default_place());
        scratch.write_control(&scratch.dir.join("explicit"));
        scratch
    }

    fn home(&self) -> PathBuf {
        self.dir.join("h")
    }

    /// 既定の置き場（`tako_core::paths::default_data_dir` と同じ規則。XDG_DATA_HOME は子から外す）
    fn default_place(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.home().join("Library/Application Support/tako")
        } else {
            self.home().join(".local/share/tako")
        }
    }

    fn write_control(&self, dir: &Path) {
        std::fs::create_dir_all(dir).expect("置き場を作れる");
        let info = serde_json::json!({
            "version": 1,
            "pid": std::process::id(),
            "socket": self.socket.display().to_string(),
            "token": "tk2531",
            "mcp_url": null,
        });
        std::fs::write(dir.join("control.json"), info.to_string()).expect("control.json を書ける");
    }

    /// 表示言語を決める（CLI は既定の置き場の settings.json を読む）
    fn set_lang(&self, lang: &str) {
        std::fs::write(
            self.default_place().join("settings.json"),
            format!("{{\"language\":\"{lang}\"}}"),
        )
        .expect("settings.json を書ける");
    }

    /// `TAKO_*` と既定の置き場を動かす env を全部外し、HOME を一時の側へ向けた `tako <args>`
    fn run(&self, args: &[&str], envs: &[(&str, &str)]) -> (Output, usize) {
        let before = self.accepted.load(Ordering::SeqCst);
        let mut command = Command::new(tako());
        command
            .args(args)
            .current_dir(&self.dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("TAKO_") {
                command.env_remove(key);
            }
        }
        command
            .env_remove("XDG_DATA_HOME")
            .env_remove("LANG")
            .env_remove("LC_ALL")
            .env_remove("LC_MESSAGES")
            .env("HOME", self.home());
        for (key, value) in envs {
            command.env(key, value);
        }
        let output = command.output().expect("tako を起こせる");
        // 受け口のスレッドが数えるまでの僅かな遅れを吸う（接続は子の終了より前に受けている）
        std::thread::sleep(std::time::Duration::from_millis(50));
        let after = self.accepted.load(Ordering::SeqCst);
        (output, after - before)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn has_japanese(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(c, '\u{3040}'..='\u{30ff}' | '\u{3400}'..='\u{9fff}' | '\u{ff00}'..='\u{ffef}')
    })
}

const JA: &str = "TAKO_ISOLATED=1 なので既定（本番）の置き場の接続情報は読まない";
const EN: &str = "TAKO_ISOLATED=1 is set, so the connection info in the default (production) location is not read";

#[test]
fn 隔離の印で置き場の指定が無ければ既定の置き場から繋がず日英で断る() {
    let scratch = Scratch::new("a");
    for (lang, expect) in [("ja", JA), ("en", EN)] {
        scratch.set_lang(lang);
        for isolated in ["1", "true", "on"] {
            let (output, connected) = scratch.run(&["list"], &[("TAKO_ISOLATED", isolated)]);
            let err = stderr(&output);
            assert_eq!(
                connected,
                0,
                "TAKO_ISOLATED={isolated}・置き場の指定なしの CLI が既定（本番）の置き場の GUI へ繋いだ \
                 （判定の正本: {}）。stderr: {err}",
                gate_sites()
            );
            assert!(!output.status.success(), "断って非ゼロで終わる: {err}");
            assert!(
                err.contains(expect),
                "{lang}: 断りの文が無い（判定の正本: {}）: {err}",
                gate_sites()
            );
            if lang == "en" {
                assert!(!has_japanese(&err), "英語表示に日本語が混ざる: {err}");
            }
        }
        // 空の置き場の指定は「指定なし」と同じ（data_dir / base_dir と同じく空を未設定と読む）
        let (output, connected) = scratch.run(
            &["list"],
            &[
                ("TAKO_ISOLATED", "1"),
                ("TAKO_DATA_DIR", ""),
                ("TAKO_DISCOVERY_DIR", ""),
            ],
        );
        assert_eq!(
            connected,
            0,
            "空の置き場の指定で繋いだ: {}",
            stderr(&output)
        );
        assert!(stderr(&output).contains(expect), "{}", stderr(&output));
    }
}

#[test]
fn 死んだtako_socketの後も既定の置き場へ縮退せず両方の理由を出す() {
    let scratch = Scratch::new("b");
    let dead = scratch.dir.join("dead.sock").display().to_string();
    for (lang, expect, cause) in [
        ("ja", JA, "tako アプリへ接続できない"),
        ("en", EN, "cannot connect to the tako app"),
    ] {
        scratch.set_lang(lang);
        let (output, connected) = scratch.run(
            &["list"],
            &[
                ("TAKO_ISOLATED", "1"),
                ("TAKO_SOCKET", &dead),
                ("TAKO_TOKEN", "x"),
            ],
        );
        let err = stderr(&output);
        assert_eq!(
            connected,
            0,
            "死んだ TAKO_SOCKET から既定（本番）の置き場へ縮退した（判定の正本: {}）: {err}",
            gate_sites()
        );
        assert!(
            err.contains(cause),
            "{lang}: 先に試した接続の失敗が消えた: {err}"
        );
        assert!(err.contains(expect), "{lang}: 隔離の断りが無い: {err}");
        if lang == "en" {
            assert!(!has_japanese(&err), "英語表示に日本語が混ざる: {err}");
        }
    }
}

#[test]
fn 置き場を明示した隔離のcliと隔離の印の無いcliは従来どおり繋がる() {
    let scratch = Scratch::new("c");
    let explicit = scratch.dir.join("explicit").display().to_string();
    let cases: [(&str, Vec<(&str, &str)>); 6] = [
        (
            "TAKO_DATA_DIR を明示",
            vec![("TAKO_ISOLATED", "1"), ("TAKO_DATA_DIR", &explicit)],
        ),
        (
            "TAKO_DISCOVERY_DIR を明示",
            vec![("TAKO_ISOLATED", "1"), ("TAKO_DISCOVERY_DIR", &explicit)],
        ),
        ("隔離の印なし", vec![]),
        ("TAKO_ISOLATED=0", vec![("TAKO_ISOLATED", "0")]),
        ("TAKO_ISOLATED が空", vec![("TAKO_ISOLATED", "")]),
        (
            "TAKO_SOCKET / TAKO_TOKEN を明示",
            vec![
                ("TAKO_ISOLATED", "1"),
                ("TAKO_SOCKET", scratch.socket.to_str().unwrap()),
                ("TAKO_TOKEN", "tk2531"),
            ],
        ),
    ];
    for (name, envs) in cases {
        let (output, connected) = scratch.run(&["list"], &envs);
        assert!(
            output.status.success() && connected == 1,
            "{name}: 従来どおり繋がらない（接続 {connected}・判定の正本: {}）: {}",
            gate_sites(),
            stderr(&output)
        );
    }
}

#[test]
fn 順番待ちは隔離の印があっても既定の置き場の調停役へ並ぶ() {
    let scratch = Scratch::new("d");
    // 状況（`tako queue`）と順番待ちの実行（`tako queue <コマンド>`）。偽の受け口は空の結果しか返さないので
    // 応答の中身は見ない = 調停役へ届いたか（接続の数）だけを見る
    for args in [&["queue", "--json"][..], &["queue", "true"][..]] {
        let (output, connected) = scratch.run(args, &[("TAKO_ISOLATED", "1")]);
        assert!(
            connected >= 1,
            "{args:?}: 隔離の印で本番の順番待ちを素通りした（{}）: {}",
            fn_line("crates/tako-cli/src/main.rs", "queue_send"),
            stderr(&output)
        );
    }
}

#[test]
fn legacy_腕は既定の置き場へ繋がる() {
    // A/B の腕が生きている = 修正前の挙動を同じバイナリで再現できる（受け入れの A/B と同じ口）
    let scratch = Scratch::new("e");
    let (output, connected) = scratch.run(
        &["list"],
        &[("TAKO_ISOLATED", "1"), ("TAKO_2531_LEGACY", "1")],
    );
    assert!(
        output.status.success() && connected == 1,
        "TAKO_2531_LEGACY=1 で修正前（既定の置き場から繋ぐ）に戻らない（接続 {connected}）: {}",
        stderr(&output)
    );
}

/// `crates/*/src` の `.rs`（再帰）
fn source_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            source_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// その行を囲む関数の名前（上へ辿って最初の `fn <名前>(`。閉じ括弧は数えない素朴な判定）
fn enclosing_fn(lines: &[&str], at: usize) -> String {
    lines[..=at]
        .iter()
        .rev()
        .find_map(|line| {
            let (_, rest) = line.split_once("fn ")?;
            let name = rest.split('(').next()?.trim();
            (!name.is_empty() && name.chars().all(|c| c == '_' || c.is_alphanumeric()))
                .then(|| name.to_string())
        })
        .unwrap_or_default()
}

#[test]
fn 既定の置き場を隔離の印なしに読む口は順番待ちだけが使う() {
    // 隔離の印を見ない口（調停役を探す）を、順番待ち以外へ使い回すと #2531 の穴が戻る。
    // 許すのは ipc.rs の `send_request_machine_wide` の中の候補列と、main.rs の `queue_send` の中の呼び出しだけ
    // （discovery.rs は定義と自分のテスト）
    let allowed: &[(&str, &str, &str)] = &[
        (
            "crates/tako-cli/src/ipc.rs",
            "send_request_machine_wide",
            "read_candidates_machine_wide(",
        ),
        (
            "crates/tako-cli/src/main.rs",
            "queue_send",
            "send_request_machine_wide(",
        ),
    ];
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root.join("crates"))
        .expect("crates/ を読める")
        .flatten()
    {
        source_files(&entry.path().join("src"), &mut files);
    }
    let mut hits = Vec::new();
    let mut seen_queue_send = false;
    for file in files {
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        let rel = rel.trim_start_matches("./").to_string();
        if rel == "crates/tako-control/src/discovery.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            for needle in [
                "read_candidates_machine_wide(",
                "send_request_machine_wide(",
            ] {
                if !code.contains(needle) || code.contains(&format!("fn {needle}")) {
                    continue;
                }
                let func = enclosing_fn(&lines, i);
                if allowed
                    .iter()
                    .any(|(f, name, n)| *f == rel && *name == func && *n == needle)
                {
                    seen_queue_send |= func == "queue_send";
                    continue;
                }
                hits.push(format!("{rel}:{}（fn {func}）: {}", i + 1, code));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "隔離の印を見ない口（{}）を順番待ち以外が使っている。繋ぎ先の発見は read_candidates / \
         send_request_classified を通す（#2531）:\n{}",
        fn_line(
            "crates/tako-control/src/discovery.rs",
            "read_candidates_machine_wide"
        ),
        hits.join("\n")
    );
    assert!(
        seen_queue_send,
        "順番待ちの口（{}）が send_request_machine_wide を通っていない = 隔離の印で本番の順番待ちを素通りする",
        fn_line("crates/tako-cli/src/main.rs", "queue_send")
    );
}
