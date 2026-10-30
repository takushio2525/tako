//! cargo の全体並列の上限 = tako のペインへ注入する共有 jobserver（FR-2.43.13 / Issue #1998。
//! エピック #1992 の S6）の純関数
//!
//! 設計の正は `.agent/plans/2026-10-resource-manager.md`（§1.6・§9.4・§9.7）。OS への口は境界 B31
//! （`platform::jobserver`）、状態（札の番人）は `tako_control::jobserver`。
//!
//! ## 形
//!
//! - GUI が fifo を作って開いたまま持ち、札を論理 CPU 数（[`tokens`]）枚入れる。ペインの env に
//!   `CARGO_MAKEFLAGS=-j<N> --jobserver-auth=fifo:<path>`（[`makeflags`]）を足す。**`MAKEFLAGS` は使わない**
//!   （`-j` 無しの素の `make` まで並列にしてしまう = 並列に耐えない Makefile を壊す。§1.6 の実測）
//! - cargo は自分の 1 本（暗黙の札）は札なしで走らせ、2 本目以降の仕事の前に札を 1 枚取る。なので
//!   **cargo が K 本並ぶと、rustc とビルドスクリプトの同時数は N + K 以下**。札が 0 枚でも暗黙の札で
//!   直列には進む = **札待ちで永久に止まる経路は無い**
//! - fifo のパスに空白があると cargo（jobserver クレート）は値を空白で区切ってパスを途中で切り、
//!   黙って自前の並列へ戻る（macOS の data dir は `Library/Application Support` = 空白入り。2026-10-31 の
//!   実測で札 0 枚でも重なり 3）。[`fifo_path`] は空白の無い置き場を選ぶ
//! - 札の数え違い（cargo を kill した = 札の持ち逃げ / GUI の再起動で札を足しすぎた）は
//!   **札を持ちうるプロセス（[`holders`]）が 0 本のときだけ** N 枚へ揃える（[`plan`]）。0 本の間は誰も札を
//!   持っていないので、fifo の中身 = 全部の札。多すぎる分はいつ抜いてもよい（並列が減るだけ）

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::resources::ProcSample;

/// ペインへ足す env の名前（cargo だけが読む。GNU make は読まない）
pub const ENV: &str = "CARGO_MAKEFLAGS";
/// A/B: GUI のプロセスの env に `1` で共有 jobserver を作らず注入もしない（修正前と同じ）
pub const AB_OFF_ENV: &str = "TAKO_1992_NO_JOBSERVER";
/// 検証用の注入: 札の枚数（論理 CPU 数の代わり）。隔離 / テストのプロセスだけが読む
pub const INJECT_TOKENS_ENV: &str = "TAKO_1998_INJECT_TOKENS";
/// `<data_dir>/resources/`（順番待ちの過去の記録と同じ置き場）
pub const DIR: &str = "resources";
/// 設計書どおりの名前（`<data_dir>/resources/jobserver.fifo`）
pub const FILE_NAME: &str = "jobserver.fifo";
/// data dir のパスに空白があるときの逃がし先の名前の後ろ（`tako-<16 桁>.jobserver.fifo`）
pub const SHORT_SUFFIX: &str = ".jobserver.fifo";
/// `$TMPDIR` 自体に空白がある機のための最後の候補
pub const FALLBACK_TEMP: &str = "/tmp";

/// 走査の覚えがこれより新しく、札を持ちうるプロセスが 0 本なら、それを信じて札を足す
pub const FRESH_SCAN: Duration = Duration::from_secs(1);
/// 走査の覚えがこれより古ければ当てにしない（誰も見ていないときの走査は 30 秒ごと）
pub const STALE_SCAN: Duration = Duration::from_secs(35);

/// A/B の env の値が「切る」か
pub fn ab_off(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.trim() == "1")
}

/// 札の枚数: 論理 CPU 数（分からなければ 1）。検証用の注入があればそれ（1 以上）
pub fn tokens(cpus: Option<usize>, inject: Option<&str>) -> u32 {
    if let Some(n) = inject
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|n| *n >= 1)
    {
        return n;
    }
    cpus.and_then(|c| u32::try_from(c).ok()).unwrap_or(1).max(1)
}

fn has_whitespace(path: &Path) -> bool {
    path.to_str()
        .is_none_or(|s| s.chars().any(|c| c.is_ascii_whitespace()))
}

/// data dir ごとに安定した逃がし先の名前（区切りの揺れは正規化してからハッシュ = `ipc_socket` と同じ）
pub fn short_name(data_dir: &Path) -> String {
    let normalized: PathBuf = data_dir.components().collect();
    let key = normalized.as_os_str().as_encoded_bytes();
    format!("tako-{:016x}{SHORT_SUFFIX}", crate::fnv::fnv1a64(key))
}

/// fifo の置き場（**純関数**）。
///
/// 1. `<data_dir>/resources/jobserver.fifo` に空白（と UTF-8 でない文字）が無ければそこ
/// 2. あれば `<temp_dir>/tako-<data dir の 16 桁>.jobserver.fifo` → `/tmp/…`（ハッシュなので再起動をまたいで同じ）
/// 3. どれにも空白があれば `None`（注入しない = cargo は従来どおり）
pub fn fifo_path(data_dir: &Path, temp_dir: &Path) -> Option<PathBuf> {
    let preferred = data_dir.join(DIR).join(FILE_NAME);
    if !has_whitespace(&preferred) {
        return Some(preferred);
    }
    let name = short_name(data_dir);
    [temp_dir, Path::new(FALLBACK_TEMP)]
        .into_iter()
        .map(|base| base.join(&name))
        .find(|p| !has_whitespace(p))
}

/// ペインへ足す `CARGO_MAKEFLAGS` の値
pub fn makeflags(tokens: u32, path: &Path) -> String {
    format!("-j{tokens} --jobserver-auth=fifo:{}", path.display())
}

/// make 形式の値から jobserver の在りか（`--jobserver-auth=` / `--jobserver-fds=` の**最後の**もの）。
/// 区切りは空白（jobserver クレートと同じ読み方）
pub fn auth(value: &str) -> Option<&str> {
    value
        .split_ascii_whitespace()
        .filter_map(|arg| {
            arg.strip_prefix("--jobserver-fds=")
                .or_else(|| arg.strip_prefix("--jobserver-auth="))
        })
        .rfind(|s| !s.is_empty())
}

/// tako が作る fifo の名前か（`jobserver.fifo` / `tako-<16 桁>.jobserver.fifo`）
pub fn is_tako_fifo(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    name == FILE_NAME || (name.starts_with("tako-") && name.ends_with(SHORT_SUFFIX))
}

/// env の `CARGO_MAKEFLAGS` の値が指す tako の fifo のパス（tako の fifo でなければ `None`）
pub fn tako_fifo(value: &str) -> Option<&str> {
    auth(value)
        .and_then(|a| a.strip_prefix("fifo:"))
        .filter(|p| is_tako_fifo(p))
}

/// env の `CARGO_MAKEFLAGS` の値が tako の共有 jobserver を指すか
pub fn refers_to_tako(value: &str) -> bool {
    tako_fifo(value).is_some()
}

/// 2 つの fifo のパスが同じ置き場の系統か（同じ data dir から決まる fifo）。
///
/// 逃がし先の名前（`tako-<data dir の 16 桁>.jobserver.fifo`）は一時 dir が違っても同じ data dir なら
/// 同じ名前なので、名前で揃える（`$TMPDIR` の違う起動をまたいだ前の世代の cargo も数える）。
/// `<data_dir>/resources/jobserver.fifo` の形はパスごと一致したときだけ。
/// **別の data dir の tako（本番と隔離）の cargo は数えない**（互いの数え直しを止め合わない）
pub fn same_fifo(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let name = |p: &str| p.rsplit(['/', '\\']).next().unwrap_or(p).to_string();
    let (na, nb) = (name(a), name(b));
    na == nb && na.starts_with("tako-") && na.ends_with(SHORT_SUFFIX)
}

/// 札を持ちうるプロセスの名前か（cargo とその配下で jobserver を読むもの）。
///
/// `name` は実行ファイル名（macOS は 16 文字で切れる）、`argv` はコマンド行。cargo のサブコマンド
/// （`cargo-clippy` 等）とビルドスクリプト（`build-script-build`）を含む。ビルドスクリプトが起こす
/// make も札を取る。node のビルド系・テストの実行体は札を取らないので数えない
/// （常駐する dev サーバーが数え直しを止め続けないように）
pub fn is_holder(name: &str, argv: Option<&str>) -> bool {
    let base = argv
        .and_then(|a| a.split_whitespace().next())
        .map(|first| first.rsplit('/').next().unwrap_or(first));
    [Some(name), base].into_iter().flatten().any(|n| {
        matches!(
            n,
            "cargo" | "rustc" | "rustdoc" | "clippy-driver" | "make" | "gmake"
        ) || n.starts_with("cargo-")
            || n.starts_with("build-script-")
            || n.starts_with("build_script_")
    })
}

/// `fifo` の札を持ちうるプロセスの本数（走査の結果から。自分 = `own_pid` は数えない）。
///
/// 数えるのは env の `CARGO_MAKEFLAGS` が同じ置き場の系統の fifo（[`same_fifo`]）を指す cargo 系
/// （[`is_holder`]）。ペインの全プロセス（シェル・agent・MCP サーバー）が同じ env を持つので名前でも絞る
pub fn holders(samples: &[ProcSample], own_pid: u32, fifo: &str) -> usize {
    samples
        .iter()
        .filter(|s| s.pid != own_pid)
        .filter(|s| {
            s.jobserver_fifo
                .as_deref()
                .is_some_and(|f| same_fifo(f, fifo))
        })
        .filter(|s| is_holder(&s.name, s.argv.as_deref()))
        .count()
}

/// 走査の覚えをどう使うか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanUse {
    /// 札を持ちうるプロセスが居ない（新しい覚え）
    Idle,
    /// 居る（覚えの古さは問わない = 待つ側なので）
    Busy,
    /// 採り直してから決める（覚えが無い / 古すぎる / 0 本だが新しくない）
    Refresh,
}

/// 走査の覚え（札を持ちうる本数, 古さ）の使い方（純関数）
pub fn scan_use(snapshot: Option<(usize, Duration)>) -> ScanUse {
    match snapshot {
        None => ScanUse::Refresh,
        Some((_, age)) if age > STALE_SCAN => ScanUse::Refresh,
        Some((0, age)) if age > FRESH_SCAN => ScanUse::Refresh,
        Some((0, _)) => ScanUse::Idle,
        Some(_) => ScanUse::Busy,
    }
}

/// 数え直しの計画
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// ちょうど N 枚
    Keep,
    /// 誰も持っていないのに足りない（持ち逃げ / 作りたて / 中身が捨てられた）→ 足す
    Add(usize),
    /// N 枚より多い（再起動で足しすぎた / 数え直しの直前に始まった cargo が返した）→ 抜く
    Remove(usize),
    /// 足りないが、札を持ちうるプロセスが居る（貸し出し中かもしれない）→ 待つ
    Wait,
}

/// 数え直しの計画（純関数）。`idle` は「札を持ちうるプロセスが 0 本」と新しい走査で確かめたか
pub fn plan(free: usize, tokens: usize, idle: bool) -> Plan {
    if free > tokens {
        Plan::Remove(free - tokens)
    } else if free == tokens {
        Plan::Keep
    } else if idle {
        Plan::Add(tokens - free)
    } else {
        Plan::Wait
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(pid: u32, name: &str, argv: &str, fifo: Option<&str>) -> ProcSample {
        ProcSample {
            pid,
            ppid: 1,
            name: name.into(),
            argv: Some(argv.into()),
            jobserver_fifo: fifo.map(str::to_string),
            ..ProcSample::default()
        }
    }

    #[test]
    fn 空白の無いdata_dirは設計書どおりの置き場() {
        let p = fifo_path(Path::new("/home/u/.local/share/tako"), Path::new("/tmp/x")).unwrap();
        assert_eq!(
            p,
            Path::new("/home/u/.local/share/tako/resources/jobserver.fifo")
        );
    }

    #[test]
    fn 空白のあるdata_dirは一時dirのハッシュ名へ逃がす() {
        let data = Path::new("/Users/u/Library/Application Support/tako");
        let p = fifo_path(data, Path::new("/var/folders/ab/T")).unwrap();
        assert_eq!(p.parent().unwrap(), Path::new("/var/folders/ab/T"));
        let name = p.file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with("tako-") && name.ends_with(SHORT_SUFFIX),
            "{name}"
        );
        assert_eq!(name.len(), "tako-".len() + 16 + SHORT_SUFFIX.len());
        // 再起動をまたいで同じ（区切りの揺れも同じ）
        let again = fifo_path(
            Path::new("/Users/u/Library/Application Support/tako/"),
            Path::new("/var/folders/ab/T"),
        );
        assert_eq!(again.as_deref(), Some(p.as_path()));
        // 一時 dir にも空白があれば /tmp
        let p = fifo_path(data, Path::new("/odd dir/T")).unwrap();
        assert_eq!(p.parent().unwrap(), Path::new(FALLBACK_TEMP));
        assert!(is_tako_fifo(p.to_str().unwrap()));
    }

    #[test]
    fn makeflagsは値を空白で区切ってもパスが切れない() {
        let data = Path::new("/Users/u/Library/Application Support/tako");
        let path = fifo_path(data, Path::new("/var/folders/ab/T")).unwrap();
        let v = makeflags(18, &path);
        assert!(v.starts_with("-j18 --jobserver-auth=fifo:"), "{v}");
        assert_eq!(auth(&v), Some(format!("fifo:{}", path.display()).as_str()));
        assert!(refers_to_tako(&v));
        assert!(!v.contains("MAKEFLAGS"), "値に別の変数を混ぜない");
    }

    #[test]
    fn jobserverの在りかは最後のものを読む() {
        assert_eq!(
            auth("-j8 --jobserver-fds=3,4 --jobserver-auth=3,4"),
            Some("3,4")
        );
        assert_eq!(
            auth("--jobserver-auth=fifo:/a/jobserver.fifo -j --jobserver-auth=fifo:/b/x"),
            Some("fifo:/b/x")
        );
        assert_eq!(auth("-j8"), None);
        assert_eq!(auth(""), None);
        assert!(
            !refers_to_tako("-j --jobserver-auth=fifo:/tmp/GMfifo123"),
            "make 4.4 自身の fifo"
        );
        assert!(
            !refers_to_tako("-j --jobserver-auth=3,4"),
            "fd の jobserver"
        );
        assert!(refers_to_tako(
            "-j --jobserver-fds=fifo:/x/resources/jobserver.fifo --jobserver-auth=fifo:/x/resources/jobserver.fifo"
        ), "cargo が子へ渡す形（-j --jobserver-fds=… --jobserver-auth=…）");
    }

    #[test]
    fn 札の枚数は論理cpu数で注入があればそれ() {
        assert_eq!(tokens(Some(18), None), 18);
        assert_eq!(tokens(None, None), 1);
        assert_eq!(tokens(Some(0), None), 1);
        assert_eq!(tokens(Some(18), Some("4")), 4);
        assert_eq!(tokens(Some(18), Some("0")), 18, "0 枚は受けない");
        assert_eq!(tokens(Some(18), Some("x")), 18);
    }

    #[test]
    fn 札を持ちうるのはcargo系だけ() {
        for (name, argv) in [
            ("cargo", "cargo build"),
            ("rustc", "/x/rustc --crate-name a"),
            ("build-script-bui", "/t/debug/build/c1-1/build-script-build"),
            ("clippy-driver", "clippy-driver"),
            ("cargo-clippy", "cargo-clippy clippy"),
            ("make", "make -C x"),
        ] {
            assert!(is_holder(name, Some(argv)), "{name}");
        }
        for (name, argv) in [
            ("node", "node vite"),
            ("zsh", "-zsh"),
            ("claude", "claude"),
            ("c1-abcdef", "/t/debug/deps/c1-abcdef"),
            ("sccache", "sccache"),
        ] {
            assert!(!is_holder(name, Some(argv)), "{name}");
        }
    }

    #[test]
    fn 数えるのは同じ置き場の系統のfifoを指すcargo系で自分は数えない() {
        let data = Path::new("/Users/u/Library/Application Support/tako");
        let mine = fifo_path(data, Path::new("/var/folders/ab/T")).unwrap();
        let mine = mine.to_str().unwrap();
        // 同じ data dir で一時 dir だけ違う前の世代の GUI が配ったパス
        let older = fifo_path(data, Path::new("/tmp")).unwrap();
        let older = older.to_str().unwrap();
        // 別の data dir の tako（隔離の GUI）
        let other = "/tmp/tk1998-x/d-a/resources/jobserver.fifo";
        let samples = vec![
            sample(10, "zsh", "-zsh", Some(mine)),
            sample(11, "cargo", "cargo build", Some(mine)),
            sample(12, "rustc", "rustc", Some(older)),
            sample(13, "cargo", "cargo build", None),
            sample(14, "cargo", "cargo build", Some(other)),
            sample(99, "cargo", "cargo run -p tako-app", Some(mine)),
        ];
        assert_eq!(holders(&samples, 99, mine), 2);
        assert_eq!(
            holders(&samples, 99, other),
            1,
            "本番と隔離は互いを数えない"
        );
        assert!(same_fifo(mine, older));
        assert!(!same_fifo(mine, other));
        assert!(!same_fifo(
            other,
            "/tmp/tk1998-x/d-b/resources/jobserver.fifo"
        ));
        assert_eq!(
            tako_fifo(&makeflags(4, Path::new(other))),
            Some(other),
            "env の値から fifo のパスを取り出す"
        );
        assert_eq!(tako_fifo("-j --jobserver-auth=fifo:/tmp/GMfifo1"), None);
    }

    #[test]
    fn 走査の覚えは0本なら新しいときだけ信じる() {
        let s = Duration::from_secs;
        assert_eq!(scan_use(None), ScanUse::Refresh);
        assert_eq!(
            scan_use(Some((0, Duration::from_millis(300)))),
            ScanUse::Idle
        );
        assert_eq!(scan_use(Some((0, s(5)))), ScanUse::Refresh);
        assert_eq!(scan_use(Some((2, s(20)))), ScanUse::Busy);
        assert_eq!(scan_use(Some((2, s(40)))), ScanUse::Refresh);
    }

    #[test]
    fn 数え直しは誰も持っていないときだけ足し多すぎればいつでも抜く() {
        assert_eq!(plan(18, 18, false), Plan::Keep);
        assert_eq!(
            plan(20, 18, false),
            Plan::Remove(2),
            "多すぎる分は待たずに抜く"
        );
        assert_eq!(plan(15, 18, false), Plan::Wait, "貸し出し中かもしれない");
        assert_eq!(plan(15, 18, true), Plan::Add(3), "持ち逃げを戻す");
        assert_eq!(plan(0, 1, true), Plan::Add(1), "N = 1 の機");
    }

    #[test]
    fn ab_offは1だけ() {
        assert!(ab_off(Some("1")));
        assert!(!ab_off(Some("0")));
        assert!(!ab_off(None));
    }
}
