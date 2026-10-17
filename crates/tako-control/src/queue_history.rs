//! 順番待ちの過去の記録の読み書き（Issue #2254。置き場は #1996 の「過去の所要時間」と同じ = 設計書 §6）
//!
//! `<data_dir>/resources/history.jsonl` に、終わった処理ごとに 1 行（[`HistoryRow`]）を足す。
//! 調停役（`crate::queue`）が起動時に読み、見込みメモリ（`tako_core::queue::estimate`）と
//! ピーク後の判定（`RunningJob::expected_peak_at_s`）に使う。
//!
//! - **生の argv とパスは書かない**（鍵はハッシュ、表示名はプログラム + サブコマンドだけ。
//!   番犬 `issue2254_queue_reservation_watchdog`）
//! - [`ROTATE_BYTES`] を超えたら `history.1.jsonl` へ回して新しく始める（読むのは両方 = 最大で約 1 MB）
//! - 読めない行（書きかけの最終行・手で壊した行）は飛ばす。欄を足すときは serde の default で旧い行を読む
//!   （行の欄は `migration_registry` の指紋で固定）
//! - 共有しない（`config_share::catalog` の `resources/` = Local。鍵は repo の根のパスを含むハッシュで、
//!   別のマシンでは同じコマンドが別の鍵になる）

use std::io::Write as _;
use std::path::{Path, PathBuf};

use tako_core::queue::HistoryRow;

/// 置き場（`<data_dir>` の下）
pub const DIR: &str = "resources";
/// 記録（新しい方）
pub const FILE: &str = "history.jsonl";
/// 回した古い方
pub const OLD_FILE: &str = "history.1.jsonl";
/// これを超えたら回す
pub const ROTATE_BYTES: u64 = 512 * 1024;
/// メモリに持つ行の上限（古い方から捨てる）
pub const KEEP_ROWS: usize = 6000;

/// 置き場（`<data_dir>/resources`）。data dir が決まらなければ `None` = 記録しない
pub fn dir() -> Option<PathBuf> {
    tako_core::paths::data_dir().map(|d| d.join(DIR))
}

/// 古い順に読む（回した古い方 → 新しい方）。無い・読めないファイルは空
pub fn load(dir: &Path) -> Vec<HistoryRow> {
    let mut rows = Vec::new();
    for name in [OLD_FILE, FILE] {
        let Ok(text) = std::fs::read_to_string(dir.join(name)) else {
            continue;
        };
        rows.extend(
            text.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .filter_map(|l| serde_json::from_str::<HistoryRow>(l).ok()),
        );
    }
    trim(&mut rows);
    rows
}

/// メモリの行を上限へ詰める（古い方から捨てる）
pub fn trim(rows: &mut Vec<HistoryRow>) {
    if rows.len() > KEEP_ROWS {
        rows.drain(..rows.len() - KEEP_ROWS);
    }
}

/// 1 行足す（置き場が無ければ作る。大きくなっていたら先に回す）。失敗は理由（種類だけ）を返す
pub fn append(dir: &Path, row: &HistoryRow) -> Result<(), String> {
    let mut line = serde_json::to_string(row).map_err(|e| format!("行を組めない: {e}"))?;
    line.push('\n');
    std::fs::create_dir_all(dir).map_err(|e| format!("置き場を作れない: {}", e.kind()))?;
    let path = dir.join(FILE);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() >= ROTATE_BYTES) {
        std::fs::rename(&path, dir.join(OLD_FILE))
            .map_err(|e| format!("回せない: {}", e.kind()))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("開けない: {}", e.kind()))?;
    file.write_all(line.as_bytes())
        .map_err(|e| format!("書けない: {}", e.kind()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tako_core::queue::JobKind;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tako-queue-history-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn row(key: &str, peak_mb: u64) -> HistoryRow {
        HistoryRow {
            at: 1,
            key: key.into(),
            name: "cargo test".into(),
            kind: JobKind::Test,
            peak_mb,
            ..HistoryRow::default()
        }
    }

    #[test]
    fn 足した順に読み戻し読めない行は飛ばす() {
        let dir = temp_dir("roundtrip");
        assert!(load(&dir).is_empty(), "無ければ空");
        append(&dir, &row("a", 1)).unwrap();
        append(&dir, &row("b", 2)).unwrap();
        // 書きかけの最終行・壊れた行
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(dir.join(FILE))
            .unwrap();
        f.write_all(b"not json\n{\"key\":\"c\",\"peak_mb\":3}\n{\"key\":")
            .unwrap();
        let keys: Vec<String> = load(&dir).into_iter().map(|r| r.key).collect();
        assert_eq!(keys, vec!["a", "b", "c"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn 大きくなったら回して古い方も読む() {
        let dir = temp_dir("rotate");
        std::fs::create_dir_all(&dir).unwrap();
        // 上限ちょうどの古い記録（1 行目は読める行）
        let first = serde_json::to_string(&row("old", 1)).unwrap() + "\n";
        let pad = " ".repeat(ROTATE_BYTES as usize - first.len());
        std::fs::write(dir.join(FILE), format!("{first}{pad}")).unwrap();
        append(&dir, &row("new", 2)).unwrap();
        assert!(dir.join(OLD_FILE).exists(), "回した");
        assert!(std::fs::metadata(dir.join(FILE)).unwrap().len() < 200);
        let keys: Vec<String> = load(&dir).into_iter().map(|r| r.key).collect();
        assert_eq!(keys, vec!["old", "new"], "古い方 → 新しい方の順");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn メモリの行は上限まで() {
        let mut rows: Vec<HistoryRow> =
            (0..KEEP_ROWS + 3).map(|i| row(&i.to_string(), 1)).collect();
        trim(&mut rows);
        assert_eq!(rows.len(), KEEP_ROWS);
        assert_eq!(rows[0].key, "3", "古い方から捨てる");
    }
}
