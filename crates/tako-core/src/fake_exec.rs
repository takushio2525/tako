//! テストが作る偽の実行ファイル（スタブ）を書き、macOS の初回 exec の検査を
//! テストの上限の外で済ませる 1 実装（Issue #2222）
//!
//! ## 何が起きていたか
//!
//! macOS は**新しいファイル**（新しい inode）の初回の exec に検査を挟み、その検査は
//! システム全体で 1 本ずつ進む。実測: 単独で 0.13〜0.34 秒、6 本同時なら最後は 1.7 秒、
//! 他の worker のビルドと重なると 6 秒（#2180 / #1838）。コピーも新しいファイルなので
//! 検査を受け、同じパスへの上書き（inode が変わらない）は受けない（#2222 の実測）。
//!
//! テストが作りたてのスタブを短い上限で待つと、列の後ろで上限を食われて偽の赤になる。
//! 別々のテストの間欠 3 件（#2180 / #2185 / #1838）の真因がこれだった。
//!
//! ## 直し方
//!
//! [`write_script`] がスタブの 2 行目に空打ちの行（[`WARM_ARG`] を受けたら本文を走らせずに
//! 終わる）を差し込んで書き、実行権を付け、その場で 1 回空打ちする。初回の検査はそこで済み、
//! テストが測る時計の中の exec は数 ms で起きる。空打ちの待ちは上限つきの
//! [`crate::probe::output_with_timeout`] を通す（コンソール抑止済みの 1 実装 =
//! `platform_parity` の起動件数を増やさない）。
//!
//! 空打ちは判定の前提を整えるだけで、判定の代わりにはしない。上限内に終わらなかった回は
//! stderr へ 1 行残して進む（テストの判定はそれぞれの上限と状態で見る）。
//!
//! ## A/B と注入（検証専用）
//!
//! - [`LEGACY_ENV`]（`TAKO_2222_LEGACY=1`）: 空打ちしない = 修正前と同じく、初回の検査が
//!   テストの時計の中に入る
//! - [`INJECT_FIRST_EXEC_ENV`]（`TAKO_2222_INJECT_FIRST_EXEC_MS=<ms>`）: 各スタブの
//!   **初回の exec だけ**をその長さ遅らせる（印はスタブごとの `<パス>.first-exec`）。
//!   CPU を焼かずに「検査の列が長い回」を作る。空打ちがあれば遅れは上限の外で消化される
//!
//! シェルのテストには同じ規則の `scripts/lib/fake-exec.sh` がある（env と空打ちの引数は共通）。
//!
//! ## cargo が作る実行ファイル（Issue #2228）
//!
//! cargo がリンクしたテスト用の実行ファイル（`CARGO_BIN_EXE_*` = 偽の言語サーバ `tako-lsp-fake`・
//! 製品の `tako`）とその写しも新しいファイルなので、初回の exec に同じ検査を受ける。検査の長さは
//! **大きさに比例**する（実測: 2 MB の `tako-lsp-fake` 0.16〜0.35 秒・12 MB の `tako-mcp` 0.33〜0.54 秒・
//! 83 MB の debug の `tako` 1.4〜1.5 秒。2 回目からは 3〜9 ms）。テストの時計を回す前に
//! [`warm_binary`] で 1 回空打ちする。テスト専用の実行ファイルは `main` の 1 文目で
//! [`enter_test_binary`] を呼び、空打ちと注入（[`INJECT_FIRST_EXEC_ENV`]）を受ける。
//! A/B は [`LEGACY_BIN_ENV`]（`TAKO_2228_LEGACY=1` = 空打ちしない）。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 空打ちの引数（スタブはこれを受けたら本文を走らせずに終わる）
pub const WARM_ARG: &str = "tako-fake-exec-warm";

/// 空打ちを待つ上限。検査の列（他のビルドと重なると 6 秒の実測）を吸うためのもので、
/// 空打ちそのものは数 ms で終わる
pub const WARM_BUDGET: Duration = Duration::from_secs(120);

/// A/B: `1` なら空打ちしない（修正前の形）
pub const LEGACY_ENV: &str = "TAKO_2222_LEGACY";

/// A/B: `1` なら cargo が作る実行ファイルを空打ちしない（[`warm_binary`] の修正前の形。#2228）
pub const LEGACY_BIN_ENV: &str = "TAKO_2228_LEGACY";

/// 注入: 各スタブ・cargo が作る実行ファイルの初回の exec をこのミリ秒だけ遅らせる（検証専用）
pub const INJECT_FIRST_EXEC_ENV: &str = "TAKO_2222_INJECT_FIRST_EXEC_MS";

/// スタブの本文（1 行目は shebang）へ、空打ちの行（と注入の行）を 2 行目に差し込む（純関数）。
///
/// 注入の行は空打ちの行より前に置く（本物の検査も、スクリプトの 1 行目が走る前 = exec の段で起きる）。
/// shebang が無い本文は `None`（exec できないスタブは空打ちでも本番でも起きない）
pub fn script_text(text: &str, first_exec_delay: Option<Duration>) -> Option<String> {
    if !text.starts_with("#!") {
        return None;
    }
    let (shebang, rest) = text.split_once('\n').unwrap_or((text, ""));
    let mut out = format!("{shebang}\n");
    if let Some(delay) = first_exec_delay {
        let ms = delay.as_millis();
        out.push_str(&format!(
            "if [ ! -e \"$0.first-exec\" ]; then : > \"$0.first-exec\"; /bin/sleep {}.{:03}; fi\n",
            ms / 1000,
            ms % 1000
        ));
    }
    out.push_str(&format!("[ \"${{1:-}}\" = {WARM_ARG} ] && exit 0\n"));
    out.push_str(rest);
    Some(out)
}

/// 偽の実行ファイルを書き、実行権を付け、1 回空打ちする（unix）。
///
/// `text` は shebang から始まる本文。2 行目に空打ちの行が入る（[`script_text`]）。
/// Windows は書くだけ（shebang のスタブは起こせない。呼び手のテストは unix で走る）。
/// 書けない・shebang が無いときは panic する（テストの組み立ての誤り）
pub fn write_script(path: &Path, text: &str) {
    let body = script_text(text, inject_first_exec_delay()).unwrap_or_else(|| {
        panic!(
            "偽の実行ファイルの本文が shebang で始まらない: {}",
            path.display()
        )
    });
    std::fs::write(path, body)
        .unwrap_or_else(|e| panic!("偽の実行ファイルを書けない: {} ({e})", path.display()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|e| panic!("実行権を付けられない: {} ({e})", path.display()));
        if std::env::var(LEGACY_ENV).as_deref() != Ok("1") {
            warm(path);
        }
    }
}

/// cargo が作ったテスト用の実行ファイル（`CARGO_BIN_EXE_*`）とその写しを 1 回空打ちする（Issue #2228）。
///
/// **テストの時計（`wait_until` の締め切り・要求の上限）を回す前に呼ぶ**。同じファイル（パス・大きさ・
/// 更新時刻が同じ = cargo が作り直していない）はプロセスで 1 回だけ起こし、同時に呼んだ他のスレッドは
/// 空打ちが終わるまで待つ（待たずに時計を回すと、同じ検査の列の後ろに並ぶ）。写しは別のパス =
/// もう 1 回起こす（コピーは新しいファイル）。
///
/// 起こすときの引数は [`WARM_ARG`] だけで、テスト専用の実行ファイルは [`enter_test_binary`] で本文を
/// 走らせずに終わる。受け付けない実行ファイル（製品の `tako` = 知らない引数として断る）でも exec の
/// 段は済むので、終了コードは問わない。[`LEGACY_BIN_ENV`] が `1` なら起こさない（修正前の形）。
/// ファイルが無い・起こせないときは panic する（テストの組み立ての誤り）
pub fn warm_binary(path: impl AsRef<Path>) {
    let path = path.as_ref();
    if std::env::var(LEGACY_BIN_ENV).as_deref() == Ok("1") {
        return;
    }
    let id = file_identity(path).unwrap_or_else(|| {
        panic!(
            "空打ちする実行ファイルが無い（cargo がまだ作っていない？）: {}",
            path.display()
        )
    });
    let key = (path.to_path_buf(), id);
    static WARMED: Mutex<Vec<(PathBuf, String)>> = Mutex::new(Vec::new());
    // 空打ちの間は握ったまま = 同時に呼んだスレッドは終わるまで待つ（検査は 1 本ずつ進むので、
    // 別のファイルの空打ちを待たせても失うものは無い）
    let mut warmed = WARMED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if warmed.contains(&key) {
        return;
    }
    warm(path);
    warmed.push(key);
}

/// cargo が作るテスト用の実行ファイルの `main` の **1 文目**（Issue #2228）。
///
/// - [`INJECT_FIRST_EXEC_ENV`] があれば、この実行ファイルの**初回の exec**をその長さ遅らせる
///   （スクリプトの注入行と同じ役 = [`inject_first_exec_delay_here`]）
/// - 引数が [`WARM_ARG`] なら本文を走らせずに終わる（[`warm_binary`] の空打ち）
pub fn enter_test_binary() {
    inject_first_exec_delay_here();
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == WARM_ARG)
    {
        std::process::exit(0);
    }
}

/// 注入（[`INJECT_FIRST_EXEC_ENV`]。検証専用）: 今の実行ファイルの初回の exec の検査を演じる（Issue #2228）。
///
/// 印は `<実行ファイル>.first-exec` に「ファイルの識別（大きさ・更新時刻）と検査の終わる時刻」を書く。
/// 印が無い・識別が違う（cargo が作り直した）回が初回で、その回が終わる時刻を決めて眠る。**終わる前に
/// 起きた他の exec も、その時刻まで眠る**（本物の検査も、済むまでは同じファイルのどの exec も待たせる）。
/// env が無ければ何もしない。製品の `main` は debug ビルドでだけ呼ぶ（配布物の隣に印を書かない）
pub fn inject_first_exec_delay_here() {
    let Some(delay) = inject_first_exec_delay() else {
        return;
    };
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let wait = first_exec_wait(&exe, delay, unix_ms(SystemTime::now()));
    if !wait.is_zero() {
        std::thread::sleep(wait);
    }
}

/// 注入の印を読み書きして、今の exec が眠る長さを返す（`now_ms` は UNIX 時刻のミリ秒）
fn first_exec_wait(exe: &Path, delay: Duration, now_ms: u128) -> Duration {
    let Some(id) = file_identity(exe) else {
        return Duration::ZERO;
    };
    let marker = marker_of(exe);
    let recorded = std::fs::read_to_string(&marker).ok().and_then(|text| {
        let (seen, until) = text.trim().split_once(' ')?;
        (seen == id).then(|| until.parse::<u128>().ok()).flatten()
    });
    let until = recorded.unwrap_or_else(|| {
        let until = now_ms + delay.as_millis();
        // 書けなければ印なし = 次の exec も初回として眠る（検証が赤に倒れる側 = 見逃さない）
        let _ = std::fs::write(&marker, format!("{id} {until}\n"));
        until
    });
    Duration::from_millis(u64::try_from(until.saturating_sub(now_ms)).unwrap_or(u64::MAX))
}

/// 注入の印の置き場（スクリプトの注入行と同じ `<パス>.first-exec`）
fn marker_of(exe: &Path) -> PathBuf {
    let mut name = exe.as_os_str().to_owned();
    name.push(".first-exec");
    PathBuf::from(name)
}

/// ファイルの識別（大きさと更新時刻。cargo の作り直しで変わる）。読めなければ `None`
fn file_identity(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let nanos = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(format!("{}-{nanos}", meta.len()))
}

fn unix_ms(at: SystemTime) -> u128 {
    at.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 1 回空打ちして初回の検査を済ませる。終わらなかった回は stderr へ 1 行残して進む
fn warm(path: &Path) {
    let program = path
        .to_str()
        .unwrap_or_else(|| panic!("偽の実行ファイルのパスが UTF-8 でない: {}", path.display()));
    match crate::probe::output_with_timeout(program, &[WARM_ARG], WARM_BUDGET) {
        crate::probe::Outcome::Done { .. } => {}
        crate::probe::Outcome::TimedOut { waited, .. } => eprintln!(
            "fake_exec（#2222）: 空打ちが {} 秒で終わらない（初回 exec の検査の列が長すぎる）: {program}",
            waited.as_secs()
        ),
        crate::probe::Outcome::Failed { reason, .. } => {
            panic!("偽の実行ファイルを空打ちで起こせない: {program} ({reason})")
        }
    }
}

/// 注入の遅れ（[`INJECT_FIRST_EXEC_ENV`]。読めない値は注入なし）
fn inject_first_exec_delay() -> Option<Duration> {
    std::env::var(INJECT_FIRST_EXEC_ENV)
        .ok()?
        .trim()
        .parse()
        .ok()
        .map(Duration::from_millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 空打ちの行はshebangの直後に入る() {
        let got = script_text("#!/bin/sh\necho x\nexit 3\n", None).unwrap();
        let lines: Vec<&str> = got.lines().collect();
        assert_eq!(lines[0], "#!/bin/sh");
        assert_eq!(lines[1], format!("[ \"${{1:-}}\" = {WARM_ARG} ] && exit 0"));
        assert_eq!(&lines[2..], ["echo x", "exit 3"]);
        // shebang だけの本文（改行なし）でも空打ちの行は入る
        let bare = script_text("#!/bin/bash", None).unwrap();
        assert_eq!(bare.lines().count(), 2, "{bare}");
        // shebang が無い本文は exec できないので受け付けない
        assert_eq!(script_text("echo x\n", None), None);
        assert_eq!(script_text("", None), None);
    }

    #[test]
    fn 注入の行は空打ちの行より前に入る() {
        let got = script_text("#!/bin/sh\necho x\n", Some(Duration::from_millis(1500))).unwrap();
        let lines: Vec<&str> = got.lines().collect();
        assert_eq!(lines[0], "#!/bin/sh");
        assert!(
            lines[1].contains("/bin/sleep 1.500") && lines[1].contains("$0.first-exec"),
            "{got}"
        );
        assert!(lines[2].contains(WARM_ARG), "{got}");
        assert_eq!(lines[3], "echo x");
    }

    /// 書いた時点で空打ちしても本文は走らない。空打ちの後の本番の起動は本文を走らせる
    #[cfg(unix)]
    #[test]
    fn 書いた時点で空打ちし本文は走らない() {
        let dir = crate::test_residue::ScratchDir::new("fake-exec-2222");
        let ran = dir.path().join("ran");
        let stub = dir.path().join("stub");
        write_script(
            &stub,
            &format!("#!/bin/sh\necho \"$@\" >> '{}'\nexit 3\n", ran.display()),
        );
        assert!(!ran.exists(), "空打ちで本文が走った");
        let out = crate::probe::output_with_timeout(
            stub.to_str().unwrap(),
            &["real"],
            Duration::from_secs(60),
        )
        .into_output()
        .expect("本番の起動が終わる");
        assert_eq!(out.status.code(), Some(3));
        assert_eq!(std::fs::read_to_string(&ran).unwrap(), "real\n");
    }

    /// 注入の遅れはファイルごとの初回だけ（コピーは新しいファイル = 本物の検査と同じ扱い）。
    /// 所要は測らず、印の有無で見る
    #[cfg(unix)]
    #[test]
    fn 注入の遅れは各ファイルの初回だけ() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::test_residue::ScratchDir::new("fake-exec-2222-inject");
        let stub = dir.path().join("stub");
        let text = script_text("#!/bin/sh\nexit 0\n", Some(Duration::from_millis(10))).unwrap();
        std::fs::write(&stub, text).unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        let first = dir.path().join("stub.first-exec");
        assert!(!first.exists());
        warm(&stub);
        assert!(first.exists(), "初回の exec で印が付く");
        let copy = dir.path().join("copy");
        std::fs::copy(&stub, &copy).unwrap();
        assert!(
            !dir.path().join("copy.first-exec").exists(),
            "コピーはまだ初回を済ませていない"
        );
        warm(&copy);
        assert!(dir.path().join("copy.first-exec").exists());
    }

    /// cargo の実行ファイルの注入の印（#2228）: 初回は遅れの全部を眠り、検査が終わるまでに起きた
    /// 他の exec も同じ時刻まで眠り、済んだ後は眠らない。作り直した（識別が変わった）ファイルは
    /// 初回に戻る。時刻は引数で渡す（実時間で測らない）
    #[test]
    fn 注入の印はファイルの識別と検査の終わる時刻で決まる() {
        let dir = crate::test_residue::ScratchDir::new("fake-exec-2228-marker");
        let exe = dir.path().join("bin");
        std::fs::write(&exe, b"v1").unwrap();
        let delay = Duration::from_millis(5_000);
        assert_eq!(first_exec_wait(&exe, delay, 1_000_000), delay, "初回");
        assert!(
            dir.path().join("bin.first-exec").exists(),
            "印は <実行ファイル>.first-exec"
        );
        assert_eq!(
            first_exec_wait(&exe, delay, 1_002_000),
            Duration::from_millis(3_000),
            "検査の途中に起きた exec は終わる時刻まで"
        );
        assert_eq!(first_exec_wait(&exe, delay, 1_005_000), Duration::ZERO);
        assert_eq!(
            first_exec_wait(&exe, delay, 9_000_000),
            Duration::ZERO,
            "済んだ後"
        );
        // 大きさが変わる = cargo が作り直した
        std::fs::write(&exe, b"v2 relinked").unwrap();
        assert_eq!(
            first_exec_wait(&exe, delay, 9_000_000),
            delay,
            "作り直したファイルは初回に戻る"
        );
        let missing = dir.path().join("missing");
        assert_eq!(first_exec_wait(&missing, delay, 0), Duration::ZERO);
    }

    /// cargo の実行ファイルの空打ち（#2228）: 同じファイルは 1 回だけ起こし、同時に呼んだスレッドは
    /// 空打ちが済んでから返る（返った時点で起こした跡がある = 呼び手の時計の中に検査を持ち込まない）。
    /// 作り直したファイルはもう 1 回起こす。空打ちの行を持たない実行ファイル（製品の `tako` と同じく
    /// 引数を問わず走って終わる形）で見る
    #[cfg(unix)]
    #[test]
    fn 実行ファイルの空打ちは同じファイルに1回で同時の呼び手は済むまで待つ() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::test_residue::ScratchDir::new("fake-exec-2228-warm");
        let ran = dir.path().join("ran");
        let bin = dir.path().join("bin");
        // 眠りは同時の呼び手が空打ちの途中に重なる幅を作るだけ（判定は跡の有無で、時間は測らない）
        let body = format!(
            "#!/bin/sh\n/bin/sleep 0.2\necho \"$1\" >> '{}'\n",
            ran.display()
        );
        std::fs::write(&bin, &body).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let lines = || {
            std::fs::read_to_string(&ran)
                .unwrap_or_default()
                .lines()
                .count()
        };
        std::thread::scope(|s| {
            let callers: Vec<_> = (0..4)
                .map(|_| {
                    s.spawn(|| {
                        warm_binary(&bin);
                        lines()
                    })
                })
                .collect();
            for caller in callers {
                assert_eq!(caller.join().unwrap(), 1, "返った時点で空打ちが済んでいる");
            }
        });
        assert_eq!(
            std::fs::read_to_string(&ran).unwrap(),
            format!("{WARM_ARG}\n")
        );
        warm_binary(&bin);
        assert_eq!(lines(), 1, "同じファイルは 2 度起こさない");
        std::fs::write(&bin, format!("{body}# relinked\n")).unwrap();
        warm_binary(&bin);
        assert_eq!(lines(), 2, "作り直したファイルはもう 1 回起こす");
    }
}
