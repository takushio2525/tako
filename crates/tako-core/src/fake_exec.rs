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

use std::path::Path;
use std::time::Duration;

/// 空打ちの引数（スタブはこれを受けたら本文を走らせずに終わる）
pub const WARM_ARG: &str = "tako-fake-exec-warm";

/// 空打ちを待つ上限。検査の列（他のビルドと重なると 6 秒の実測）を吸うためのもので、
/// 空打ちそのものは数 ms で終わる
pub const WARM_BUDGET: Duration = Duration::from_secs(120);

/// A/B: `1` なら空打ちしない（修正前の形）
pub const LEGACY_ENV: &str = "TAKO_2222_LEGACY";

/// 注入: 各スタブの初回の exec をこのミリ秒だけ遅らせる（検証専用）
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

/// 1 回空打ちして初回の検査を済ませる。終わらなかった回は stderr へ 1 行残して進む
#[cfg(unix)]
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
}
