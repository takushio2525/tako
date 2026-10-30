//! remote_generation — 常駐 remote daemon の世代（どの実行ファイルで動いているか）を判定する（#2518）
//!
//! remote daemon（`tako remote serve`）は GUI とは別プロセスで、install（.app の差し替え）でも
//! GUI の再起動でも入れ替わらない。#2518 以前の `spawn_daemon` は稼働中の daemon の実行ファイルを
//! **パスの文字列**で比べていたので、同じパスのまま中身が替わる差し替え
//! （`bundle_install` の contents-swap）を検知できず、旧版のコードが動き続けた
//! （#2513 の段 1 で実測: install 3 回と GUI 再起動をまたいで旧版のまま約 2 日・
//! 消えた旧版の名前で exec 拒否 37,445 件）。
//!
//! ここは 2 つだけを持つ:
//!
//! 1. **記録** — daemon が起動時に自分の実行ファイルの識別（[`ExeStamp`]）を PID ファイルの
//!    4 行目へ残す（[`encode`] / [`decode`]）
//! 2. **判定** — 記録した識別と、いまそのパスに在るファイルの識別を比べる（[`judge`]）。
//!    比べるのは**ファイルの識別**（unix は dev + inode、加えて更新時刻とサイズ）で、
//!    パスの文字列ではない
//!
//! 判定を読む面はどれも [`judge`] の 1 実装を通る（`remote::daemon_generation`）:
//! `tako remote status` / MCP `tako_remote_status` の `serve_generation`（旧版なら `warnings`
//! にも 1 行）・GUI 起動時の自動復帰（`remote_autostart::probe`。旧版なら立て直す）・
//! `tako remote start`（`spawn_daemon`。旧版なら止めて起こし直す）。
//!
//! ## #2518 より前の daemon（PID ファイルが 3 行）
//!
//! 識別の記録が無いので、代わりに**ファイルの状態変更時刻（ctime）と daemon の起動時刻**を
//! 比べる。ctime はユーザー空間から戻せない（`ditto` は mtime を保つが ctime は複製した時刻に
//! なる）ので、「daemon の起動より後にそのパスへ置かれたファイル」を言える。属性の書き換え
//! （chmod・xattr）でも進むので旧版と取り違えうるが、そのときの損は「立て直しが 1 回余計に
//! 走る」だけで、立て直した daemon は識別を残すので 2 回目は起きない。
//! この経路が要るのは、この修正を入れた最初の install の時点で動いている daemon が
//! 必ず #2518 より前の版だから（記録の無い daemon を「分からない」で見送ると、
//! 最初の 1 回は従来どおり手で stop / start することになる）。

use std::path::Path;

/// A/B: #2518 前へ戻す（世代を判定せず、パスの文字列だけで比べる）。
/// 立てると install 後も旧版の daemon が残る = Issue の症状そのもの
pub const LEGACY_ENV: &str = "TAKO_2518_LEGACY";

/// #2518 前の挙動を再現するか
pub fn legacy_mode() -> bool {
    std::env::var(LEGACY_ENV).is_ok_and(|v| v == "1" || v == "true" || v == "on")
}

/// 実行ファイルの識別（daemon の起動時に取る）。
///
/// **パスは持たない**（パスは PID ファイルの 2 行目にある）。同じパスのまま中身が
/// 替わったことを言うための材料だけを持つ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExeStamp {
    /// ボリューム（unix の `st_dev`。unix 以外は 0）
    pub dev: u64,
    /// inode（unix の `st_ino`。unix 以外は 0）。contents-swap では新しいファイルなので必ず変わる
    pub ino: u64,
    /// 更新時刻（unix epoch からのナノ秒。取れなければ 0）
    pub mtime_ns: u128,
    /// サイズ（バイト）
    pub len: u64,
}

/// PID ファイルの 4 行目の書式の印。書式を変えるときは印を変え、[`decode`] は
/// 知らない印を「記録なし」として読む（古い版の GUI が新しい版の daemon を誤判定しない）
const STAMP_TAG: &str = "exe-v1";

/// 識別を PID ファイルの 1 行へ（`exe-v1:<dev>:<ino>:<mtime_ns>:<len>`）
pub fn encode(stamp: &ExeStamp) -> String {
    format!(
        "{STAMP_TAG}:{}:{}:{}:{}",
        stamp.dev, stamp.ino, stamp.mtime_ns, stamp.len
    )
}

/// PID ファイルの 1 行から識別を読む（印が違う・欄が欠ける・数でない = `None`）
pub fn decode(line: &str) -> Option<ExeStamp> {
    let mut parts = line.trim().split(':');
    if parts.next()? != STAMP_TAG {
        return None;
    }
    let dev = parts.next()?.parse().ok()?;
    let ino = parts.next()?.parse().ok()?;
    let mtime_ns = parts.next()?.parse().ok()?;
    let len = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(ExeStamp {
        dev,
        ino,
        mtime_ns,
        len,
    })
}

/// いまそのパスに在るファイルについて分かること（[`observe`] が集める）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Observed {
    /// ファイルの識別（無い・読めない = `None`）
    pub stamp: Option<ExeStamp>,
    /// 状態変更時刻（unix epoch 秒。unix 以外・取れない = `None`）
    pub ctime_secs: Option<i64>,
}

/// パスの先のファイルの識別を取る（symlink は辿る = 実体の識別）。
/// サブプロセスを起こさない（stat 1 回）
pub fn observe(path: &Path) -> Observed {
    let Ok(meta) = std::fs::metadata(path) else {
        return Observed::default();
    };
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    #[cfg(unix)]
    let (dev, ino, ctime_secs) = {
        use std::os::unix::fs::MetadataExt;
        (meta.dev(), meta.ino(), Some(meta.ctime()))
    };
    #[cfg(not(unix))]
    let (dev, ino, ctime_secs) = (0u64, 0u64, None);
    Observed {
        stamp: Some(ExeStamp {
            dev,
            ino,
            mtime_ns,
            len: meta.len(),
        }),
        ctime_secs,
    }
}

/// daemon 自身が起動時に残す 1 行（自分の実行ファイルの識別）。取れなければ空
pub fn self_stamp_line(exe: &Path) -> String {
    observe(exe).stamp.map(|s| encode(&s)).unwrap_or_default()
}

/// 判定の根拠
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    /// 起動時に記録した識別と比べた（#2518 以後の daemon）
    Stamp,
    /// 記録が無いので ctime と起動時刻で比べた（#2518 より前の daemon）
    ChangeTime,
}

impl Basis {
    /// JSON 用の識別子（ASCII 固定）
    pub fn tag(self) -> &'static str {
        match self {
            Self::Stamp => "stamp",
            Self::ChangeTime => "ctime",
        }
    }
}

/// daemon の世代の判定
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generation {
    /// daemon はいまそのパスに在るファイルで動いている
    Current(Basis),
    /// 同じパスのファイルが daemon の起動後に差し替わった（install の contents-swap 等）。
    /// **立て直せる**（そのパスに新しいファイルが在る）
    Replaced(Basis),
    /// daemon の実行ファイルがもう無い（.app を消した等）。旧版だが、起こし直す先が無いので
    /// 自動では立て直さない（止めてから起こせないと、動いていた daemon まで失う）
    Missing,
    /// 比べる材料が無い（PID ファイルにパスが無い・記録も ctime も取れない）
    Unknown,
}

impl Generation {
    /// JSON 用の識別子（ASCII 固定）
    pub fn tag(self) -> &'static str {
        match self {
            Self::Current(_) => "current",
            Self::Replaced(_) => "replaced",
            Self::Missing => "missing",
            Self::Unknown => "unknown",
        }
    }

    /// 旧版のまま動いているか
    pub fn is_stale(self) -> bool {
        matches!(self, Self::Replaced(_) | Self::Missing)
    }

    /// 止めて起こし直せば新版になるか（= 自動で立て直してよいか）
    pub fn renewable(self) -> bool {
        matches!(self, Self::Replaced(_))
    }

    /// 判定の根拠（材料が無い判定は `None`）
    pub fn basis(self) -> Option<Basis> {
        match self {
            Self::Current(b) | Self::Replaced(b) => Some(b),
            Self::Missing | Self::Unknown => None,
        }
    }
}

/// daemon の世代を判定する（純関数。#2518 の判定の 1 実装）。
///
/// - `recorded`: daemon が起動時に残した識別（PID ファイルの 4 行目。#2518 より前の daemon は `None`）
/// - `started_at`: daemon の起動時刻（PID ファイルの 3 行目。unix epoch 秒）
/// - `now`: いま daemon の実行ファイルのパスに在るもの（[`observe`]）
///
/// **パスの文字列は見ない**。同じパスのまま中身が替わるのが #2518 の症状なので、
/// パスが同じことは「同じ世代」の根拠にならない
pub fn judge(recorded: Option<&ExeStamp>, started_at: Option<u64>, now: &Observed) -> Generation {
    let Some(current) = now.stamp.as_ref() else {
        return Generation::Missing;
    };
    if let Some(recorded) = recorded {
        return if recorded == current {
            Generation::Current(Basis::Stamp)
        } else {
            Generation::Replaced(Basis::Stamp)
        };
    }
    match (started_at, now.ctime_secs) {
        (Some(started), Some(ctime)) => {
            // 秒の切り捨てどうしで比べる。同じ秒は「起動より前に置かれた」側へ倒す
            // （置いた直後に起動した daemon を旧版と言わない）
            if ctime > i64::try_from(started).unwrap_or(i64::MAX) {
                Generation::Replaced(Basis::ChangeTime)
            } else {
                Generation::Current(Basis::ChangeTime)
            }
        }
        _ => Generation::Unknown,
    }
}

/// 旧版のまま動いているときの説明（status の `serve_generation.detail` と `warnings` の 1 行）
pub fn stale_detail(generation: Generation, binary: &str, pid: u32) -> Option<String> {
    match generation {
        Generation::Replaced(_) => Some(format!(
            "稼働中のリモート daemon（PID {pid}）は旧版のまま動いています: \
             {binary} は daemon の起動後に差し替えられました。\
             `tako remote start` で新版へ入れ替わります（GUI を起動したときは自動で入れ替わります）"
        )),
        Generation::Missing => Some(format!(
            "稼働中のリモート daemon（PID {pid}）の実行ファイル {binary} がもうありません。\
             tako を入れ直してから `tako remote start` で入れ替えてください"
        )),
        Generation::Current(_) | Generation::Unknown => None,
    }
}

/// `tako remote status` / MCP `tako_remote_status` に載せる `serve_generation`（純関数）。
///
/// `stale` / `renewable` を**応答に持たせる**のは、読む側（GUI の自動復帰・`spawn_daemon`・
/// CLI）が判定をやり直さずに済むようにするため（#372 / #1473 と同じ。[`renewable_in_status`]）
pub fn status_field(generation: Generation, binary: &str, pid: u32) -> serde_json::Value {
    let mut out = serde_json::json!({
        "state": generation.tag(),
        "stale": generation.is_stale(),
        "renewable": generation.renewable(),
    });
    if let Some(basis) = generation.basis() {
        out["basis"] = serde_json::json!(basis.tag());
    }
    if let Some(detail) = stale_detail(generation, binary, pid) {
        out["detail"] = serde_json::json!(detail);
    }
    out
}

/// status の応答から「止めて起こし直せば新版になる旧版の daemon が動いている」を読む。
/// `serve_generation` が無い（daemon が居ない・`TAKO_2518_LEGACY=1`）なら false
pub fn renewable_in_status(status: &serde_json::Value) -> bool {
    status["running"].as_bool() == Some(true)
        && status["serve_generation"]["renewable"].as_bool() == Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(ino: u64) -> ExeStamp {
        ExeStamp {
            dev: 16777232,
            ino,
            mtime_ns: 1_761_800_000_123_456_789,
            len: 48_000_000,
        }
    }

    fn observed(s: Option<ExeStamp>, ctime: Option<i64>) -> Observed {
        Observed {
            stamp: s,
            ctime_secs: ctime,
        }
    }

    #[test]
    fn 識別は書いて読むと同じになる() {
        let s = stamp(240083285);
        assert_eq!(decode(&encode(&s)), Some(s));
        // 前後の空白（PID ファイルの改行）は読める
        assert_eq!(decode(&format!("  {}\n", encode(&s))), Some(s));
    }

    #[test]
    fn 印が違う行や欠けた行は記録なしとして読む() {
        for line in [
            "",
            "1761800000",
            "exe-v2:1:2:3:4",
            "exe-v1:1:2:3",
            "exe-v1:1:2:3:4:5",
            "exe-v1:a:2:3:4",
            "/Applications/tako.app/Contents/MacOS/tako",
        ] {
            assert_eq!(decode(line), None, "{line:?}");
        }
    }

    /// #2518 の症状そのもの: パスは同じまま、ファイルだけが別物になった
    #[test]
    fn 同じパスのまま中身が替わったら旧版と言う() {
        let before = stamp(100);
        let after = stamp(200);
        assert_eq!(
            judge(Some(&before), Some(10), &observed(Some(after), Some(5))),
            Generation::Replaced(Basis::Stamp),
            "inode が替わったのに ctime が古いことを理由に current と言わない"
        );
        let mut touched = before;
        touched.mtime_ns += 1;
        assert_eq!(
            judge(Some(&before), None, &observed(Some(touched), None)),
            Generation::Replaced(Basis::Stamp)
        );
        let mut resized = before;
        resized.len += 1;
        assert!(judge(Some(&before), None, &observed(Some(resized), None)).renewable());
    }

    #[test]
    fn 同じファイルなら現行と言う() {
        let s = stamp(100);
        assert_eq!(
            judge(Some(&s), Some(10), &observed(Some(s), Some(999))),
            Generation::Current(Basis::Stamp),
            "記録があるときは ctime（属性の書き換えで進む）を見ない"
        );
    }

    #[test]
    fn 記録の無い古いdaemonはctimeと起動時刻で比べる() {
        let s = stamp(100);
        assert_eq!(
            judge(None, Some(1000), &observed(Some(s), Some(1001))),
            Generation::Replaced(Basis::ChangeTime)
        );
        assert_eq!(
            judge(None, Some(1000), &observed(Some(s), Some(1000))),
            Generation::Current(Basis::ChangeTime),
            "同じ秒は起動より前に置かれた側へ倒す"
        );
        assert_eq!(
            judge(None, Some(1000), &observed(Some(s), Some(999))),
            Generation::Current(Basis::ChangeTime)
        );
        assert_eq!(
            judge(None, None, &observed(Some(s), Some(1001))),
            Generation::Unknown,
            "起動時刻が無ければ比べない"
        );
        assert_eq!(
            judge(None, Some(1000), &observed(Some(s), None)),
            Generation::Unknown,
            "ctime が取れない OS では比べない"
        );
    }

    #[test]
    fn ファイルが無ければ旧版だが立て直さない() {
        let g = judge(Some(&stamp(1)), Some(1), &observed(None, None));
        assert_eq!(g, Generation::Missing);
        assert!(g.is_stale());
        assert!(!g.renewable(), "起こし直す先が無いので止めない");
    }

    #[test]
    fn 旧版のときだけ説明を返す() {
        let bin = "/x/tako.app/Contents/MacOS/tako";
        for g in [
            Generation::Current(Basis::Stamp),
            Generation::Current(Basis::ChangeTime),
            Generation::Unknown,
        ] {
            assert!(!g.is_stale());
            assert_eq!(stale_detail(g, bin, 1), None);
        }
        for g in [Generation::Replaced(Basis::Stamp), Generation::Missing] {
            let d = stale_detail(g, bin, 42).expect("旧版は説明を持つ");
            assert!(d.contains("PID 42") && d.contains(bin), "{d}");
            assert!(d.contains("`tako remote start`"), "次の一手は最簡形: {d}");
        }
    }

    #[test]
    fn statusの欄は読む側が判定をやり直さずに済む形() {
        let bin = "/x/tako";
        let replaced = status_field(Generation::Replaced(Basis::Stamp), bin, 7);
        assert_eq!(replaced["state"], "replaced");
        assert_eq!(replaced["basis"], "stamp");
        assert_eq!(replaced["stale"], true);
        assert_eq!(replaced["renewable"], true);
        assert!(replaced["detail"].as_str().unwrap().contains("PID 7"));
        let status = serde_json::json!({ "running": true, "serve_generation": replaced });
        assert!(renewable_in_status(&status));

        let current = status_field(Generation::Current(Basis::ChangeTime), bin, 7);
        assert_eq!(current["basis"], "ctime");
        assert!(current.get("detail").is_none(), "現行なら説明を載せない");
        assert!(!renewable_in_status(
            &serde_json::json!({ "running": true, "serve_generation": current })
        ));

        let missing = status_field(Generation::Missing, bin, 7);
        assert_eq!(missing["stale"], true);
        assert!(!renewable_in_status(
            &serde_json::json!({ "running": true, "serve_generation": missing })
        ));
        // daemon が居ない応答・欄が無い応答（A/B）は立て直さない
        assert!(!renewable_in_status(
            &serde_json::json!({ "running": true })
        ));
        assert!(!renewable_in_status(&serde_json::json!({
            "running": false,
            "serve_generation": status_field(Generation::Replaced(Basis::Stamp), bin, 7),
        })));
    }

    #[test]
    fn 識別子はascii固定で一意() {
        let all = [
            Generation::Current(Basis::Stamp),
            Generation::Replaced(Basis::Stamp),
            Generation::Missing,
            Generation::Unknown,
        ];
        let tags: std::collections::BTreeSet<&str> = all.iter().map(|g| g.tag()).collect();
        assert_eq!(tags.len(), all.len());
        for t in tags
            .iter()
            .chain([Basis::Stamp.tag(), Basis::ChangeTime.tag()].iter())
        {
            assert!(t.chars().all(|c| c.is_ascii_lowercase()), "{t}");
        }
    }

    /// 実ファイルで: 同じパスへ別のファイルを rename で置く（contents-swap と同じ形）と
    /// 識別が変わる。書き換えずに読み直すだけなら変わらない
    #[test]
    fn 実ファイルの差し替えを識別で見分けられる() {
        let dir = std::env::temp_dir().join(format!("tako-2518-observe-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let exe = dir.join("tako");
        std::fs::write(&exe, b"old").unwrap();
        let first = observe(&exe);
        let recorded = first.stamp.expect("在るファイルは識別を持つ");
        assert_eq!(
            judge(Some(&recorded), None, &observe(&exe)),
            Generation::Current(Basis::Stamp),
            "読み直しただけで旧版と言わない"
        );
        // 中身もサイズも同じ新しいファイルを隣で作り、同じパスへ rename する
        let staged = dir.join("tako.new");
        std::fs::write(&staged, b"old").unwrap();
        std::fs::rename(&staged, &exe).unwrap();
        let after = observe(&exe);
        let _ = std::fs::remove_dir_all(&dir);
        if cfg!(unix) {
            assert_eq!(
                judge(Some(&recorded), None, &after),
                Generation::Replaced(Basis::Stamp),
                "同じパス・同じ中身でも別のファイル（inode）なら旧版: {recorded:?} → {:?}",
                after.stamp
            );
        }
        assert_eq!(
            judge(Some(&recorded), None, &observe(&dir.join("tako"))),
            Generation::Missing
        );
    }
}
