//! ローカルタイムの UTC オフセット取得（境界。Issue #813）。
//!
//! 標準ライブラリはタイムゾーンを持たないので OS の C ランタイムに聞く。
//! 関数名も `tm` の中身もプラットフォームで違う（unix は `localtime_r` +
//! `tm_gmtoff`、Windows は `localtime_s` で `tm_gmtoff` を持たない）ため、
//! **ここだけが `cfg` を書いてよい**。呼び出し側は秒数を 1 つ受け取るだけ。
//!
//! 使い道は「画面に出ている `reset at 3am` のような**日付を持たない時刻表記**を
//! unix 秒へ直す」こと。判断そのものは `crate::limit_resume` の純関数が
//! オフセットを引数で受け取るので、テストは実機のタイムゾーンに依存しない。

/// いまのローカルタイムの UTC からのずれ（秒。日本なら +32400）。
/// 夏時間も含めた「その瞬間の」値を返す。取得できなければ 0（= UTC 扱い）
pub fn local_utc_offset() -> i32 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    offset_at(now)
}

/// unix 秒を指定してのオフセット（夏時間の切り替わりをまたぐ計算のため分離）
pub fn offset_at(unix_secs: i64) -> i32 {
    broken_down(unix_secs).unwrap_or(0)
}

#[cfg(unix)]
fn broken_down(unix_secs: i64) -> Option<i32> {
    let t = unix_secs as libc::time_t;
    // SAFETY: `localtime_r` は呼び出し側が用意した `tm` へ書き込むスレッド安全版。
    // 失敗（null 返り）は None にする
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return None;
        }
        Some(tm.tm_gmtoff as i32)
    }
}

/// Windows の `tm` は `tm_gmtoff` を持たないので、ローカルと UTC の
/// 分解時刻を突き合わせて差を出す（`localtime_s` / `gmtime_s` は引数順も unix と違う）
#[cfg(windows)]
fn broken_down(unix_secs: i64) -> Option<i32> {
    let t = unix_secs as libc::time_t;
    // SAFETY: どちらも呼び出し側の `tm` へ書き込む。戻り 0 が成功
    unsafe {
        let mut local: libc::tm = std::mem::zeroed();
        let mut utc: libc::tm = std::mem::zeroed();
        if libc::localtime_s(&mut local, &t) != 0 || libc::gmtime_s(&mut utc, &t) != 0 {
            return None;
        }
        Some(diff_secs(&local, &utc))
    }
}

/// 2 つの分解時刻の差（秒）。年をまたぐ場合は年内通日ではなく年の大小で寄せる
#[cfg(windows)]
fn diff_secs(local: &libc::tm, utc: &libc::tm) -> i32 {
    let day_delta = if local.tm_year != utc.tm_year {
        if local.tm_year > utc.tm_year {
            1
        } else {
            -1
        }
    } else {
        (local.tm_yday - utc.tm_yday).signum()
    };
    let hms = |tm: &libc::tm| tm.tm_hour * 3600 + tm.tm_min * 60 + tm.tm_sec;
    day_delta * 86_400 + hms(local) - hms(utc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn オフセットはありえる範囲に収まる() {
        // 実機のタイムゾーンに依存しないよう範囲だけ見る（UTC-12 〜 UTC+14）
        let off = local_utc_offset();
        assert!(
            (-12 * 3600..=14 * 3600).contains(&off),
            "ありえないオフセット: {off}"
        );
        // 秒単位の端数を持つタイムゾーンは現存しないので分の倍数になる
        assert_eq!(off % 60, 0, "分未満の端数がある: {off}");
    }

    #[test]
    fn 別の瞬間でも同じ範囲に収まる() {
        // 夏時間の有無で値が変わりうるが、範囲からは出ない
        for t in [0_i64, 1_000_000_000, 1_786_752_000] {
            let off = offset_at(t);
            assert!((-12 * 3600..=14 * 3600).contains(&off), "{t} → {off}");
        }
    }
}
