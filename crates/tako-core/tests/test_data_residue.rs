//! テストプロセスの data dir が**溜まらない**ことの実測（Issue #1296）
//!
//! #944 の隔離（`<TMPDIR>/tako-test-data-<pid>`）には消す仕掛けが無く、
//! macOS の `TMPDIR` は再起動でも消えないので `cargo test` のたびに積もっていた
//! （実測 2026-09-11: 2,188 件・`du` で 115 MB）。
//!
//! 「隔離のフラグが立った」ではなく**実際に子プロセスを起こして dir の有無を見る**
//! （番犬の作法は `test_write_isolation`（#944）と同じ）。子は目印の環境変数が
//! 無ければ即座に返るので、通常の `cargo test` では 1 度も走らない。
//!
//! ここで押さえるのは 3 つ:
//!
//! 1. 正常終了した子の dir は残らない（案 1 = `atexit`）
//! 2. SIGKILL された子の残骸は、**次の**テストプロセスの起動で掃かれる（案 2）
//! 3. そのとき**生きているテストプロセスの dir は消えない**（並行して走る
//!    別 worker の `cargo test` を巻き込まないこと）

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// 子の本体を起こす目印（無ければ子は即座に返る）
const CHILD_ENV: &str = "TAKO_1296_CHILD";

/// 子が「自分の data dir はここ」と親へ伝えるファイル名
const REPORT: &str = "child-data-dir.txt";

/// 生き続ける子に終わってよいと伝えるファイル名
const STOP: &str = "stop-live-child";

const TOUCH_TEST: &str = "子プロセス_data_dirを作ってすぐ終わる";
const LIVE_TEST: &str = "子プロセス_data_dirを作って生き続ける";

/// 子プロセスの待ちの上限（`.agent/conventions.md`「期限をつける」= #1271）
const CHILD_DEADLINE: Duration = Duration::from_secs(60);

/// 生き続ける子が**自分から諦める**までの上限。親の停止指示を取りこぼしても
/// 必ず終わるための保険なので、親の手順（子を 2 回起こす = 実測 0.5 秒未満）より
/// 桁で長く取る。短いとここが先に切れて「生きている子の置き場が消えた」ように見え、
/// 混んだ機でだけ落ちる番犬になる
const LIVE_MAX: Duration = Duration::from_secs(300);

// ---------------------------------------------------------------- 子プロセス

/// 子: data dir を作って親へ報告し、そのまま**正常終了**する。
/// 終了時に `atexit` が自分の dir を消すはず
#[test]
fn 子プロセス_data_dirを作ってすぐ終わる() {
    let Some(report) = child_report_path() else {
        return;
    };
    let dir = tako_core::paths::data_dir().expect("data dir");
    std::fs::write(&report, dir.to_string_lossy().as_bytes()).expect("報告を書ける");
}

/// 子: data dir を作って親へ報告し、停止ファイルが出来るまで生き続ける。
/// 「生きているプロセスの置き場は消さない」を実プロセスで再現するための器
#[test]
fn 子プロセス_data_dirを作って生き続ける() {
    let Some(report) = child_report_path() else {
        return;
    };
    let dir = tako_core::paths::data_dir().expect("data dir");
    let stop = report.with_file_name(STOP);
    std::fs::write(&report, dir.to_string_lossy().as_bytes()).expect("報告を書ける");
    let deadline = Instant::now() + LIVE_MAX;
    while Instant::now() < deadline && !stop.exists() {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 子として起こされているなら報告先を返す（そうでなければ `None` = 何もしない）
fn child_report_path() -> Option<PathBuf> {
    std::env::var_os(CHILD_ENV).map(PathBuf::from)
}

// ------------------------------------------------------------------ 親の道具

/// 使い捨ての `TMPDIR`。**本物の TMPDIR は触らない**
/// （並行して走る別 worker の `cargo test` の置き場がそこに居る）
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tako-1296-{tag}-{}-{}",
            std::process::id(),
            // 同一プロセス内で連番にならない値（テスト間で衝突しない）
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("一時ディレクトリを作れる");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// 残っている `tako-test-data-*` の名前一覧（数え方は Issue の `ls | wc -l` と同じ）
    fn residues(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.0) else {
            return Vec::new();
        };
        let mut out: Vec<String> = entries
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with("tako-test-data-"))
            .collect();
        out.sort();
        out
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 子テストを 1 本名指しで起こす（`TMPDIR` は使い捨てへ向ける）
fn child_command(test: &str, scratch: &Path, report: &Path, legacy: bool) -> Command {
    let exe = std::env::current_exe().expect("テストバイナリのパス");
    let mut cmd = Command::new(exe);
    cmd.args(["--exact", test, "--test-threads=1", "--nocapture"])
        .env(CHILD_ENV, report)
        // 一時ディレクトリの解決元は OS ごとに違う（unix = TMPDIR /
        // Windows = TMP・TEMP）。両方を使い捨てへ向ける
        .env("TMPDIR", scratch)
        .env("TMP", scratch)
        .env("TEMP", scratch)
        // 明示の TAKO_DATA_DIR が居ると隔離先の話にならない
        .env_remove("TAKO_DATA_DIR")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if legacy {
        cmd.env("TAKO_1296_LEGACY", "1");
    } else {
        cmd.env_remove("TAKO_1296_LEGACY");
    }
    cmd
}

/// 期限つきで子の終了を待つ（返らない子でテストごと固まらせない = #1271）
fn wait_with_deadline(child: &mut Child, what: &str) -> bool {
    let deadline = Instant::now() + CHILD_DEADLINE;
    loop {
        match child.try_wait().expect("子の状態を読める") {
            Some(status) => return status.success(),
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("{what} が {CHILD_DEADLINE:?} で終わらない");
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

/// 子が報告ファイルを書くまで待ち、その中身（data dir のパス）を返す
fn wait_for_report(report: &Path, what: &str) -> PathBuf {
    let deadline = Instant::now() + CHILD_DEADLINE;
    while Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(report) {
            if !text.trim().is_empty() {
                return PathBuf::from(text.trim());
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("{what} が報告を書かない");
}

/// 「確実に死んでいる pid」を作る（起こして看取る）。
/// 適当な大きい数を使うと **pid 再利用中の生きたプロセス**に当たりうる
fn reaped_pid(scratch: &Path) -> u32 {
    let report = scratch.join("reaped.txt");
    let mut child = child_command(TOUCH_TEST, scratch, &report, false)
        .spawn()
        .expect("子を起こせる");
    let pid = child.id();
    wait_with_deadline(&mut child, "看取り用の子");
    assert!(
        !tako_core::platform::process::pid_alive(pid),
        "看取った pid {pid} が生きている"
    );
    pid
}

/// `tako-test-data-<pid>` の形の残骸を手で作る（中身入り）
fn plant_residue(scratch: &Path, pid: u32) -> PathBuf {
    let dir = scratch.join(format!("tako-test-data-{pid}"));
    std::fs::create_dir_all(&dir).expect("残骸を作れる");
    let mut f = std::fs::File::create(dir.join("persist.log")).expect("中身を作れる");
    writeln!(f, "residue for pid {pid}").expect("書ける");
    dir
}

// -------------------------------------------------------------------- 受け入れ

/// 受け入れ 1: 正常終了したテストプロセスの data dir は残らない。
/// A/B（`TAKO_1296_LEGACY=1`）では**同じ手順で残る** = 検査に検出力がある
#[test]
fn 正常終了したテストプロセスのdata_dirは残らない() {
    let scratch = Scratch::new("exit");
    let report = scratch.path().join(REPORT);

    let mut child = child_command(TOUCH_TEST, scratch.path(), &report, false)
        .spawn()
        .expect("子を起こせる");
    let pid = child.id();
    assert!(
        wait_with_deadline(&mut child, "後始末する子"),
        "子が失敗した"
    );
    let dir = wait_for_report(&report, "後始末する子");

    assert_eq!(
        dir,
        scratch.path().join(format!("tako-test-data-{pid}")),
        "隔離先が使い捨ての TMPDIR の下に出来ていない"
    );
    assert!(
        !dir.exists(),
        "終了しても data dir が残っている: {}（残骸 {:?}）",
        dir.display(),
        scratch.residues()
    );
    assert!(
        scratch.residues().is_empty(),
        "残骸: {:?}",
        scratch.residues()
    );

    // A/B: 後始末を切ると同じ手順で残る
    let legacy_report = scratch.path().join("legacy.txt");
    let mut legacy = child_command(TOUCH_TEST, scratch.path(), &legacy_report, true)
        .spawn()
        .expect("子を起こせる");
    let legacy_pid = legacy.id();
    assert!(
        wait_with_deadline(&mut legacy, "legacy の子"),
        "子が失敗した"
    );
    let legacy_dir = wait_for_report(&legacy_report, "legacy の子");
    assert_eq!(
        legacy_dir,
        scratch.path().join(format!("tako-test-data-{legacy_pid}"))
    );
    assert!(
        legacy_dir.exists(),
        "TAKO_1296_LEGACY=1 でも消えている = A/B が効いていない"
    );
    let _ = std::fs::remove_dir_all(&legacy_dir);
}

/// 受け入れ 2: 死んだプロセスの残骸は次のテストプロセスの起動で掃かれ、
/// **生きているテストプロセスの置き場は残る**
#[test]
fn 死んだプロセスの残骸だけが次の起動で掃かれる() {
    let scratch = Scratch::new("sweep");

    // 生きている当事者（自分の data dir を持ったまま待つ）
    let live_report = scratch.path().join("live.txt");
    let mut live = child_command(LIVE_TEST, scratch.path(), &live_report, false)
        .spawn()
        .expect("生き続ける子を起こせる");
    let live_pid = live.id();
    let live_dir = wait_for_report(&live_report, "生き続ける子");
    assert!(live_dir.exists(), "生きている子の data dir が無い");
    assert!(tako_core::platform::process::pid_alive(live_pid));

    // 死んでいる pid の残骸（SIGKILL された回の再現）
    let dead_pid = reaped_pid(scratch.path());
    let dead_dir = plant_residue(scratch.path(), dead_pid);
    assert!(dead_dir.exists());

    // 次のテストプロセスを起こす = 起動時の掃除が走る
    let report = scratch.path().join(REPORT);
    let mut sweeper = child_command(TOUCH_TEST, scratch.path(), &report, false)
        .spawn()
        .expect("子を起こせる");
    let sweeper_pid = sweeper.id();
    assert!(
        wait_with_deadline(&mut sweeper, "掃除する子"),
        "子が失敗した"
    );

    assert!(
        !dead_dir.exists(),
        "pid {dead_pid} が居ないのに残骸が残っている（残り {:?}）",
        scratch.residues()
    );
    assert!(
        live_dir.exists(),
        "生きている pid {live_pid} の置き場を消した（残り {:?}）",
        scratch.residues()
    );
    assert!(
        !scratch
            .path()
            .join(format!("tako-test-data-{sweeper_pid}"))
            .exists(),
        "掃除した子が自分の置き場を残している"
    );
    assert_eq!(
        scratch.residues(),
        vec![format!("tako-test-data-{live_pid}")],
        "残ってよいのは生きている子の置き場だけ"
    );

    // A/B: 掃除を切ると死んだ pid の残骸が残ったまま
    let dead2 = plant_residue(scratch.path(), reaped_pid(scratch.path()));
    let legacy_report = scratch.path().join("legacy.txt");
    let mut legacy = child_command(TOUCH_TEST, scratch.path(), &legacy_report, true)
        .spawn()
        .expect("子を起こせる");
    assert!(
        wait_with_deadline(&mut legacy, "legacy の子"),
        "子が失敗した"
    );
    assert!(
        dead2.exists(),
        "TAKO_1296_LEGACY=1 でも掃かれている = A/B が効いていない"
    );

    std::fs::write(scratch.path().join(STOP), b"stop").expect("停止を伝えられる");
    let _ = live.wait();
}
