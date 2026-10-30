//! #1998: 共有 jobserver（札）に本物の cargo がどう従うかの回帰テスト
//!
//! 眠るだけのビルドスクリプトを持つ小さな workspace（依存なし・`--offline`）を作り、ビルドスクリプトの
//! 走った区間を記録して**同時に走った数の最大**（重なり）を数える。CPU は焼かない（眠るだけ）。
//!
//! - fifo が無いとき、cargo は黙って従来どおり（自前の並列で）走る
//! - 札 0 枚の fifo なら直列（= env を読んでいる。上の「従来どおり」が無視ではなく fifo の不在による）
//! - 順番待ちの取り分（`CARGO_BUILD_JOBS`）と札を重ねると札が上限になり、取り分は cargo に見られない
//!   （受け継いだ jobserver があると cargo は `-j` を見ない = 2026-10-31 の実測の固定。設計書 §1.6・§9.7）
//! - `CARGO_MAKEFLAGS` は素の `make` の並列度を変えない
//! - 包まない cargo を 3 本並べても、ビルドスクリプトの同時数は N + 3 以下
//! - 番人を作り直しても（GUI の再起動）札は N を超えず、cargo を kill して失った札は誰も持っていなくなってから戻る
//!
//! 札の番人は `tako_control::jobserver::Keeper` を直に使う（本番の GUI と同じ型。札を持ちうるプロセスの
//! 本数は、この機の他の cargo に左右されないよう起こした子の生死から渡す）。Windows は対象外（#2000）

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tako_control::jobserver::{Keeper, Mode};
use tako_core::platform::jobserver::{Fifo, SUPPORTED};
use tako_core::test_residue::ScratchDir;

/// 子の cargo へ外から漏れると測定が変わる env（走らせた人の env で結果を変えない = #2391）
const SCRUB: &[&str] = &[
    "CARGO_MAKEFLAGS",
    "MAKEFLAGS",
    "MFLAGS",
    "CARGO_BUILD_JOBS",
    "CARGO_TARGET_DIR",
    "CARGO_BUILD_TARGET_DIR",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_INCREMENTAL",
    "TAKO_QUEUE_JOB",
];

fn cargo_bin() -> PathBuf {
    std::env::var_os("CARGO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cargo"))
}

/// 眠るビルドスクリプトだけの workspace（`crates` 個のクレート）。走った区間を `log` へ 1 行ずつ足す
fn workspace(root: &Path, tag: &str, crates: usize, sleep_ms: u64, log: &Path) -> PathBuf {
    let dir = root.join(tag);
    let mut members = Vec::new();
    for i in 1..=crates {
        let name = format!("{tag}_c{i}");
        let c = dir.join(&name);
        std::fs::create_dir_all(c.join("src")).unwrap();
        std::fs::write(
            c.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n"
            ),
        )
        .unwrap();
        std::fs::write(c.join("src/lib.rs"), "").unwrap();
        std::fs::write(
            c.join("build.rs"),
            format!(
                r#"use std::io::Write;
fn now() -> u128 {{ std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() }}
fn main() {{
    let s = now();
    std::thread::sleep(std::time::Duration::from_millis({sleep_ms}));
    let e = now();
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open({log:?}).unwrap();
    // 1 行を 1 回の write で（`writeln!` は部品ごとに write するので、並んだビルドスクリプトの行が混ざる）
    let line = format!("{name} {{s}} {{e}}\n");
    f.write_all(line.as_bytes()).unwrap();
}}
"#
            ),
        )
        .unwrap();
        members.push(format!("\"{name}\""));
    }
    std::fs::write(
        dir.join("Cargo.toml"),
        format!(
            "[workspace]\nresolver = \"2\"\nmembers = [{}]\n",
            members.join(", ")
        ),
    )
    .unwrap();
    dir
}

/// `cargo build` を起こす（env は掃除してから `extra` を足す）
fn spawn_cargo(ws: &Path, target: &Path, extra: &[(&str, String)]) -> Child {
    let mut cmd = Command::new(cargo_bin());
    cmd.args(["build", "--offline", "-q"])
        .current_dir(ws)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    for k in SCRUB {
        cmd.env_remove(k);
    }
    cmd.env("CARGO_TARGET_DIR", target);
    for (k, v) in extra {
        cmd.env(k, v);
    }
    cmd.spawn().expect("cargo を起こせる")
}

/// 終わりを待って（終了コードの成否, stderr）
fn finish(child: Child) -> (bool, String) {
    let out = child.wait_with_output().expect("cargo の終わりを待てる");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// 区間の記録から同時に走った数の最大（端が触れているだけは重なりに数えない）
fn overlap(log: &Path) -> usize {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let mut events: Vec<(u128, i32)> = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace().skip(1);
        if let (Some(s), Some(e)) = (parts.next(), parts.next()) {
            let at = |v: &str| {
                v.parse::<u128>().unwrap_or_else(|err| {
                    panic!("区間の記録の行が壊れている（{err}）: {line:?}\n{text}")
                })
            };
            events.push((at(s), 1));
            events.push((at(e), -1));
        }
    }
    events.sort();
    let (mut cur, mut max) = (0i32, 0i32);
    for (_, d) in events {
        cur += d;
        max = max.max(cur);
    }
    max as usize
}

fn runs(log: &Path) -> usize {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .count()
}

/// 札 `tokens` 枚を入れた fifo（`Fifo` を持っている間だけ中身が残る）
fn fifo_with(dir: &Path, tokens: usize) -> (Fifo, PathBuf) {
    let path = dir.join("resources/jobserver.fifo");
    let (fifo, _) = Fifo::open_or_create(&path).unwrap();
    assert_eq!(fifo.put(tokens).unwrap(), tokens);
    (fifo, path)
}

fn makeflags(tokens: u32, path: &Path) -> (&'static str, String) {
    (
        tako_core::jobserver::ENV,
        tako_core::jobserver::makeflags(tokens, path),
    )
}

#[test]
fn fifoが無いときcargoは黙って従来どおり走る() {
    if !SUPPORTED {
        return;
    }
    let scratch = ScratchDir::new("js1998-missing");
    let root = scratch.path();
    // 置き場だけ渡して fifo は作らない（GUI が終了して fifo を消した後の器のペイン）
    let missing = root.join("gone/resources/jobserver.fifo");
    let log = root.join("missing.log");
    let ws = workspace(root, "m", 3, 1500, &log);
    let (key, value) = makeflags(4, &missing);
    let (ok, stderr) = finish(spawn_cargo(
        &ws,
        &root.join("t-missing"),
        &[(key, value), ("CARGO_BUILD_JOBS", "3".into())],
    ));
    assert!(ok, "fifo が無くても cargo は成功する: {stderr}");
    assert!(
        !stderr.to_lowercase().contains("jobserver"),
        "黙って走る（jobserver の警告を出さない）: {stderr}"
    );
    assert_eq!(runs(&log), 3);
    assert!(
        overlap(&log) >= 2,
        "自前の並列（CARGO_BUILD_JOBS=3）で走る: 重なり {}",
        overlap(&log)
    );

    // 対照: 同じ env で札 0 枚の fifo が在れば直列 = env は読まれている
    let (fifo, path) = fifo_with(&root.join("present"), 0);
    let log = root.join("present.log");
    let ws = workspace(root, "p", 3, 600, &log);
    let (key, value) = makeflags(4, &path);
    let (ok, stderr) = finish(spawn_cargo(
        &ws,
        &root.join("t-present"),
        &[(key, value), ("CARGO_BUILD_JOBS", "3".into())],
    ));
    assert!(ok, "{stderr}");
    assert_eq!(overlap(&log), 1, "札 0 枚 = 暗黙の 1 枚だけで直列");
    assert_eq!(fifo.available().unwrap(), 0, "札は増えも減りもしない");
}

#[test]
fn 取り分と札を重ねると札が上限で取り分は見られない() {
    if !SUPPORTED {
        return;
    }
    let scratch = ScratchDir::new("js1998-share");
    let root = scratch.path();
    // 取り分 8 + 札 0 枚 → 直列（札が厳しい = 札が効く）
    let (_fifo0, path0) = fifo_with(&root.join("b"), 0);
    let log = root.join("jobs8.log");
    let ws = workspace(root, "k", 3, 600, &log);
    let (key, value) = makeflags(4, &path0);
    let (ok, stderr) = finish(spawn_cargo(
        &ws,
        &root.join("t-jobs8"),
        &[(key, value), ("CARGO_BUILD_JOBS", "8".into())],
    ));
    assert!(ok, "{stderr}");
    assert_eq!(overlap(&log), 1, "CARGO_BUILD_JOBS=8 + 札 0 枚 = 直列");

    // 取り分 1 + 札 3 枚 → 札の数まで並ぶ（受け継いだ jobserver があると cargo は -j を見ない。
    // 2026-10-31 の実測: jobs 1 / 2 / 3 / 8 × 札 0 / 1 / 4 で重なりは札だけで決まった）。
    // 設計書 §1.6 の S0 の「jobs=1 + 札 4 = 直列」は所要時間からの推定で、区間を数えると直列ではない。
    // 将来の cargo が -j も見るようになったらここが落ちる = 設計書 §1.6・§9.4・§9.7 と FR-2.43.13 を直す
    let (fifo, path) = fifo_with(&root.join("a"), 3);
    let log = root.join("jobs1.log");
    let ws = workspace(root, "j", 4, 1500, &log);
    let (key, value) = makeflags(3, &path);
    let (ok, stderr) = finish(spawn_cargo(
        &ws,
        &root.join("t-jobs1"),
        &[(key, value), ("CARGO_BUILD_JOBS", "1".into())],
    ));
    assert!(ok, "{stderr}");
    let peak = overlap(&log);
    assert!(
        (2..=4).contains(&peak),
        "CARGO_BUILD_JOBS=1 + 札 3 枚: 取り分は見られず札 + 1 = 4 までに収まる（重なり {peak}）"
    );
    assert_eq!(fifo.available().unwrap(), 3, "札は全部返る");
}

#[test]
fn cargo_makeflagsは素のmakeの並列度を変えない() {
    if !SUPPORTED {
        return;
    }
    let has_make = Command::new("make")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !has_make {
        eprintln!("make が無いので飛ばす");
        return;
    }
    let scratch = ScratchDir::new("js1998-make");
    let root = scratch.path();
    let (_fifo, path) = fifo_with(&root.join("f"), 4);
    let log = root.join("make.log");
    // 3 つの的が 0.4 秒ずつ眠る。`-j` が無ければ make は直列
    let rule = |name: &str| {
        format!(
            "{name}:\n\t@s=$$(perl -MTime::HiRes=time -e 'printf \"%d\", time*1e6'); sleep 0.4; \
             e=$$(perl -MTime::HiRes=time -e 'printf \"%d\", time*1e6'); echo \"{name} $$s $$e\" >> {log}\n",
            log = log.display()
        )
    };
    std::fs::write(
        root.join("Makefile"),
        format!(
            ".PHONY: all a b c\nall: a b c\n{}{}{}",
            rule("a"),
            rule("b"),
            rule("c")
        ),
    )
    .unwrap();
    let (key, value) = makeflags(4, &path);
    let mut cmd = Command::new("make");
    cmd.current_dir(root).stdout(Stdio::null());
    for k in SCRUB {
        cmd.env_remove(k);
    }
    let status = cmd.env(key, value).status().unwrap();
    assert!(status.success(), "make が失敗した: {status:?}");
    assert_eq!(runs(&log), 3);
    assert_eq!(
        overlap(&log),
        1,
        "CARGO_MAKEFLAGS は make が読まない = 素の make は直列のまま"
    );
}

#[test]
fn 包まないcargoを3本並べても同時数は札とcargoの本数の和以下() {
    if !SUPPORTED {
        return;
    }
    let scratch = ScratchDir::new("js1998-three");
    let root = scratch.path();
    let tokens = 2u32;
    let mut keeper = Keeper::new(
        Mode::Keeper,
        Some(root.join("data/resources/jobserver.fifo")),
        tokens,
        false,
        true,
    );
    keeper.reconcile();
    keeper.tick(|| Some(0));
    assert_eq!(keeper.free(), Some(tokens as usize));
    let (key, value) = keeper.pane_env().expect("札を配っている");
    let log = root.join("three.log");
    let children: Vec<Child> = ["x", "y", "z"]
        .iter()
        .map(|tag| {
            let ws = workspace(root, tag, 4, 900, &log);
            spawn_cargo(&ws, &root.join(format!("t-{tag}")), &[(key, value.clone())])
        })
        .collect();
    let mut max_in_use = 0usize;
    let mut children: Vec<Option<Child>> = children.into_iter().map(Some).collect();
    let deadline = Instant::now() + Duration::from_secs(300);
    while children.iter().any(Option::is_some) && Instant::now() < deadline {
        let free = keeper.free().unwrap();
        assert!(free <= tokens as usize, "札は N を超えない: {free}");
        max_in_use = max_in_use.max(tokens as usize - free);
        for slot in children.iter_mut() {
            if let Some(child) = slot {
                if child.try_wait().unwrap().is_some() {
                    let (ok, stderr) = finish(slot.take().unwrap());
                    assert!(ok, "{stderr}");
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(children.iter().all(Option::is_none), "5 分で終わる");
    assert_eq!(runs(&log), 12);
    let peak = overlap(&log);
    assert!(
        peak <= tokens as usize + 3,
        "ビルドスクリプトの同時数 {peak} は N + 3 = {} 以下",
        tokens + 3
    );
    assert!(max_in_use >= 1, "札は実際に使われた");
    assert_eq!(keeper.free(), Some(tokens as usize), "終われば全部返る");
}

#[test]
fn 番人を作り直しても札はnを超えずkillで失った札は後で戻る() {
    if !SUPPORTED {
        return;
    }
    let scratch = ScratchDir::new("js1998-restart");
    let root = scratch.path();
    let tokens = 3u32;
    let path = root.join("data/resources/jobserver.fifo");
    let mut a = Keeper::new(Mode::Keeper, Some(path.clone()), tokens, false, true);
    a.reconcile();
    a.tick(|| Some(0));
    let (key, value) = a.pane_env().unwrap();

    // ① cargo が走っている間に GUI（番人）が終わり、新しい GUI が開き直す
    let log = root.join("restart.log");
    let ws = workspace(root, "r", 6, 1200, &log);
    let mut child = spawn_cargo(&ws, &root.join("t-r"), &[(key, value.clone())]);
    // 札が貸し出されるまで待つ
    let deadline = Instant::now() + Duration::from_secs(120);
    while a.free() == Some(tokens as usize) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(a.free() < Some(tokens as usize), "札が使われ始めた");
    assert_eq!(
        a.shutdown(|| Some(1)),
        "kept",
        "cargo が居るので fifo は残す"
    );
    let mut b = Keeper::new(Mode::Keeper, Some(path.clone()), tokens, false, true);
    b.reconcile();
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        let alive = child.try_wait().unwrap().is_none();
        b.tick(|| Some(usize::from(alive)));
        let free = b.free().unwrap();
        assert!(
            free <= tokens as usize,
            "再起動をまたいでも札は N を超えない: {free}"
        );
        if !alive || Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let (ok, stderr) = finish(child);
    assert!(ok, "{stderr}");
    assert!(
        overlap(&log) <= tokens as usize + 1,
        "1 本の cargo の同時数 {} は N + 1 以下",
        overlap(&log)
    );
    b.tick(|| Some(0));
    assert_eq!(b.free(), Some(tokens as usize), "終われば N 枚に揃う");

    // ② cargo を kill して札を持ち逃げさせる → 誰も持っていなくなってから戻る
    let log = root.join("kill.log");
    let ws = workspace(root, "k", 6, 1500, &log);
    let mut child = spawn_cargo(&ws, &root.join("t-k"), &[(key, value)]);
    let deadline = Instant::now() + Duration::from_secs(120);
    while b.free() == Some(tokens as usize) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    child.kill().unwrap();
    let _ = child.wait();
    // 孤児になったビルドスクリプトが眠り終えるまで待つ（その間は札を持ちうる = 足さない）
    std::thread::sleep(Duration::from_millis(1800));
    let leaked = tokens as usize - b.free().unwrap();
    b.tick(|| Some(1));
    assert_eq!(
        b.free(),
        Some(tokens as usize - leaked),
        "持ちうるプロセスが居る間は足さない"
    );
    b.tick(|| Some(0));
    assert_eq!(
        b.free(),
        Some(tokens as usize),
        "誰も持っていなくなったら N 枚へ"
    );
    if leaked > 0 {
        assert_eq!(b.last_recount().map(|r| r.added), Some(leaked));
    }
    eprintln!("kill で失った札: {leaked} 枚（数え直しで戻した）");
}
