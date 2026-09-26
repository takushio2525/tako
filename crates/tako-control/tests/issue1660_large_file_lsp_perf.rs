//! #1660 の計測: 大きいファイル（10 万行 / 10 MB）を編集しているとき、言語サーバへの同期
//! （`LspManager::sync` = 打鍵のたびに UI スレッドから呼ばれる）が 1 打鍵あたりいくらかかるか。
//!
//! 実時間は環境で揺れるので**通常のテストでは走らせない**（`.agent/conventions.md`
//! 「効果を測る単体テストは実時間で比べない」）。数字は PR 本文へ載せる:
//!
//! `cargo test -p tako-control --release --test issue1660_large_file_lsp_perf -- --ignored --nocapture`

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tako_control::lsp::manager::DEFAULT_IDLE_GRACE;
use tako_control::lsp::{DocLink, Launch, LspConfig, LspManager};
use tako_core::lsp::servers::{self, ServerSpec};
use tako_core::lsp::state::RestartPolicy;
use tako_core::platform::child_cmd::ChildCmd;

const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tako-1660-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn config(log: PathBuf, scenario: &str) -> LspConfig {
    let args = vec![
        "--scenario".to_string(),
        scenario.to_string(),
        "--log".to_string(),
        log.display().to_string(),
    ];
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

/// 1 行 100 バイト（改行込み）× 10 万行 = 10 MB。日本語を混ぜて UTF-16 の換算を効かせる
fn large_text() -> String {
    let mut text = String::with_capacity(10_000_000);
    for i in 0..100_000 {
        let mut line = format!("    let v{i} = \"日本語の文字列\"; // ");
        while line.len() < 99 {
            line.push('~');
        }
        text.push_str(&line);
        text.push('\n');
    }
    text
}

fn stats(label: &str, mut samples: Vec<Duration>) {
    samples.sort();
    let median = samples[samples.len() / 2];
    let worst = *samples.last().unwrap();
    eprintln!(
        "[perf] {label}: 中央値 {median:?} / 最悪 {worst:?}（{} 回）",
        samples.len()
    );
}

fn measure(scenario: &str) {
    let scratch = Scratch::new(scenario);
    let log = scratch.0.join("received.jsonl");
    let manager = LspManager::new(config(log.clone(), scenario));
    let path = scratch.0.join("src").join("main.rs");
    let mut text = large_text();
    std::fs::write(&path, &text).unwrap();
    let mut link = DocLink::default();
    let mut version = 0u64;
    let t = Instant::now();
    manager.sync(&mut link, true, &path, &text, version);
    eprintln!(
        "[perf] {scenario}: 初回 sync（didOpen を積む）{:?}",
        t.elapsed()
    );
    // 握手と didOpen が済むまで待つ（済む前の sync は写しを進めるだけで測りたいものと違う）
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let opened = std::fs::read_to_string(&log)
            .map(|s| s.contains("textDocument/didOpen"))
            .unwrap_or(false);
        if opened {
            break;
        }
        assert!(Instant::now() < deadline, "didOpen が届かない");
        std::thread::sleep(Duration::from_millis(50));
    }
    for (label, fraction) in [("先頭", 0.0001), ("中央", 0.5), ("末尾", 0.9999)] {
        let mut at = (text.len() as f64 * fraction) as usize;
        while !text.is_char_boundary(at) {
            at += 1;
        }
        let mut samples = Vec::new();
        for _ in 0..100 {
            text.insert(at, 'x');
            at += 1;
            version += 1;
            let t = Instant::now();
            manager.sync(&mut link, true, &path, &text, version);
            samples.push(t.elapsed());
            // 送信キューを溢れさせない（打鍵の間隔 ≒ 人の速さの上限）
            std::thread::sleep(Duration::from_millis(5));
        }
        stats(&format!("{scenario}: {label} 1 打鍵の sync"), samples);
    }
    manager.shutdown_all(Duration::from_secs(5));
}

#[test]
#[ignore = "性能計測（--release で --ignored 指定のとき手動実行）"]
fn perf_大きいファイルの打鍵ごとのlsp同期() {
    measure("normal");
    measure("full-sync");
}
