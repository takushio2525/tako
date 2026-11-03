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
//!   （行の欄は `migration_registry` の指紋で固定）。#1996 で移行の番地（`SchemaId::QueueHistory`・版 1）に
//!   載せた: 調停役は起動の読み込み（[`load_and_repair`]）で読めない行を見つけたら、元の中身を
//!   `<名前>.unreadable.bak` へ写してから読める行だけで書き直す（壊れた行の後ろへ追記が続いて
//!   読めない行が増えていかない）。`tako migrate status` も同じ判定（[`first_bad_line`]）で申告する
//! - **書く前に行を検査する**（[`row_leak`]。#1996）: 鍵と形は 16 桁の 16 進、表示名はパスの区切り・`=`・
//!   空白を含まない 2 語まで。外れた行は書かずに persist.log へ欄の名前だけ残す（中身は書かない）
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

/// 読めない最初の行（1 始まり）。空行は数えるが読めない行にはしない。すべて読めれば `None`
pub fn first_bad_line(text: &str) -> Option<usize> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .find(|(_, l)| serde_json::from_str::<HistoryRow>(l.trim()).is_err())
        .map(|(i, _)| i + 1)
}

/// 起動の読み込み（#1996）: [`load`] と同じ行を返し、読めない行のあるファイルは元の中身を
/// `<名前>.unreadable.bak`（2 本目以降は `.unreadable.<n>.bak`）へ写してから読める行だけで書き直す。
/// 写せなかったら書き直さない（元の中身を失わない。読むときに飛ばすだけ）
pub fn load_and_repair(dir: &Path) -> Vec<HistoryRow> {
    for name in [OLD_FILE, FILE] {
        let path = dir.join(name);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(line) = first_bad_line(&text) else {
            continue;
        };
        let Some(q) =
            tako_core::migration::quarantine_unreadable(&path, &tako_core::migration::FsIo)
        else {
            crate::diag::persist_log(&format!(
                "[queue] 過去の記録 {name} の {line} 行目が読めない（退避できないので書き直さず、読むときに飛ばす）"
            ));
            continue;
        };
        let good: String = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && serde_json::from_str::<HistoryRow>(l).is_ok())
            .map(|l| format!("{l}\n"))
            .collect();
        let saved = q
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match crate::config_io::atomic_write(&path, &good) {
            Ok(()) => crate::diag::persist_log(&format!(
                "[queue] 過去の記録 {name} の {line} 行目から読めない行があったので {saved} へ退避し、読める行だけに直した"
            )),
            Err(e) => crate::diag::persist_log(&format!(
                "[queue] 過去の記録 {name} を {saved} へ退避したが書き直せない（{e}）"
            )),
        }
    }
    load(dir)
}

/// 書いてはいけない中身が載った欄（#1996。[`row_leak`]）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Leak {
    /// 欄の名前（`key` / `shape` / `name`）
    pub field: &'static str,
    /// どう外れたか
    pub why: &'static str,
}

/// 表示名の上限（プログラム + サブコマンド）
const NAME_MAX: usize = 64;

fn is_hash(s: &str) -> bool {
    s.len() == 16
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// 過去の記録の行に生の argv・パスが載っていないか（#1996。純関数）。
/// 鍵と形は `q::history_keys` のハッシュ（16 桁の 16 進）、表示名はプログラム + サブコマンドの 2 語まで
/// （パスの区切り・`=`・空白・制御文字を含まない）。外れたら最初の欄を返す
pub fn row_leak(row: &HistoryRow) -> Option<Leak> {
    if !is_hash(&row.key) {
        return Some(Leak {
            field: "key",
            why: "16 桁の 16 進（ハッシュ）ではない",
        });
    }
    if row.shape.as_deref().is_some_and(|s| !is_hash(s)) {
        return Some(Leak {
            field: "shape",
            why: "16 桁の 16 進（ハッシュ）ではない",
        });
    }
    let words: Vec<&str> = row.name.split(' ').collect();
    let bad_word = |w: &&str| {
        w.is_empty()
            || w.chars()
                .any(|c| matches!(c, '/' | '\\' | '=' | '~') || c.is_whitespace() || c.is_control())
    };
    if row.name.len() > NAME_MAX
        || words.len() > 2
        || (!row.name.is_empty() && words.iter().any(bad_word))
    {
        return Some(Leak {
            field: "name",
            why: "プログラム + サブコマンドの 2 語ではない（パス・env・引数を含む）",
        });
    }
    None
}

/// メモリの行を上限へ詰める（古い方から捨てる）
pub fn trim(rows: &mut Vec<HistoryRow>) {
    if rows.len() > KEEP_ROWS {
        rows.drain(..rows.len() - KEEP_ROWS);
    }
}

/// 1 行足す（置き場が無ければ作る。大きくなっていたら先に回す）。失敗は理由（種類だけ）を返す。
/// 生の argv・パスが載った行（[`row_leak`]）は書かない
pub fn append(dir: &Path, row: &HistoryRow) -> Result<(), String> {
    if let Some(leak) = row_leak(row) {
        return Err(format!("書かない（{} が{}）", leak.field, leak.why));
    }
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

    /// 鍵は 16 桁の 16 進（append が [`row_leak`] で検査する）。`key` は末尾へ埋める短い印
    fn hex(tag: &str) -> String {
        let digits: String = tag.bytes().map(|b| format!("{:x}", b % 16)).collect();
        format!("{digits:0>16}")
    }

    fn row(key: &str, peak_mb: u64) -> HistoryRow {
        HistoryRow {
            at: 1,
            key: hex(key),
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
        assert_eq!(keys, vec![hex("a"), hex("b"), "c".to_string()]);
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
        assert_eq!(keys, vec![hex("old"), hex("new")], "古い方 → 新しい方の順");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn メモリの行は上限まで() {
        let mut rows: Vec<HistoryRow> =
            (0..KEEP_ROWS + 3).map(|i| row(&i.to_string(), 1)).collect();
        trim(&mut rows);
        assert_eq!(rows.len(), KEEP_ROWS);
        assert_eq!(rows[0].key, hex("3"), "古い方から捨てる");
    }

    #[test]
    fn issue1996_読めない行は退避してから読める行だけに直す() {
        let dir = tako_core::test_residue::ScratchDir::new("queue-history-1996-repair");
        append(&dir, &row("a", 1)).unwrap();
        // 書きかけの最終行の後ろへ追記が続いた形（読めない行が途中に挟まる）
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(dir.join(FILE))
            .unwrap();
        f.write_all(b"{\"key\":").unwrap();
        drop(f);
        append(&dir, &row("b", 2)).unwrap();
        let before = std::fs::read_to_string(dir.join(FILE)).unwrap();
        assert_eq!(first_bad_line(&before), Some(2), "2 行目が読めない");
        let rows = load_and_repair(&dir);
        assert_eq!(
            rows.len(),
            1,
            "読める行は a だけ（b は壊れた行に続けて書かれた）"
        );
        // 元の中身は 1 バイトも失わずに退避へ
        let saved = std::fs::read_to_string(dir.join(format!("{FILE}.unreadable.bak"))).unwrap();
        assert_eq!(saved, before);
        // 書き直した後は全行が読め、次の追記は読める行になる
        let after = std::fs::read_to_string(dir.join(FILE)).unwrap();
        assert_eq!(first_bad_line(&after), None);
        append(&dir, &row("c", 3)).unwrap();
        assert_eq!(load_and_repair(&dir).len(), 2);
        assert!(
            !dir.join(format!("{FILE}.unreadable.2.bak")).exists(),
            "読める間は退避を増やさない"
        );
    }

    #[test]
    fn issue1996_全行が読めれば触らない() {
        let dir = tako_core::test_residue::ScratchDir::new("queue-history-1996-clean");
        append(&dir, &row("a", 1)).unwrap();
        let before = std::fs::metadata(dir.join(FILE))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(load_and_repair(&dir).len(), 1);
        assert_eq!(
            std::fs::metadata(dir.join(FILE))
                .unwrap()
                .modified()
                .unwrap(),
            before
        );
        assert!(!dir.join(format!("{FILE}.unreadable.bak")).exists());
    }

    #[test]
    fn issue1996_生のargvとパスの載った行は書かず欄を名指す() {
        let ok = row("a", 1);
        assert_eq!(row_leak(&ok), None);
        let agent = HistoryRow {
            name: "claude".into(),
            ..ok.clone()
        };
        assert_eq!(row_leak(&agent), None, "agent の系統名");
        let cases: &[(HistoryRow, &str)] = &[
            (
                HistoryRow {
                    key: "cargo test --test secret".into(),
                    ..ok.clone()
                },
                "key",
            ),
            (
                HistoryRow {
                    shape: Some("/srv/proj/Cargo.toml".into()),
                    ..ok.clone()
                },
                "shape",
            ),
            (
                HistoryRow {
                    name: "/opt/tools/bin/cargo".into(),
                    ..ok.clone()
                },
                "name",
            ),
            (
                HistoryRow {
                    name: "cargo test --manifest-path".into(),
                    ..ok.clone()
                },
                "name",
            ),
            (
                HistoryRow {
                    name: "SECRET_TOKEN=abc cargo".into(),
                    ..ok.clone()
                },
                "name",
            ),
            (
                HistoryRow {
                    name: "C:\\work\\build.cmd".into(),
                    ..ok.clone()
                },
                "name",
            ),
        ];
        let dir = tako_core::test_residue::ScratchDir::new("queue-history-1996-leak");
        for (bad, field) in cases {
            assert_eq!(row_leak(bad).map(|l| l.field), Some(*field), "{bad:?}");
            let err = append(&dir, bad).unwrap_err();
            assert!(err.contains(field), "断りが欄を名指す: {err}");
        }
        assert!(!dir.join(FILE).exists(), "1 行も書いていない");
    }
}
