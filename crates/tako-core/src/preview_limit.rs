//! プレビューが全文を読み込む上限と、上限に当たったときの理由（Issue #1660）。
//!
//! プレビュー（表示）と編集は**同じ上限**を使う。上限を超えたファイルは末尾を省略して
//! 表示し、編集は断る（省略した表示を保存すると元ファイルの末尾を失うため）。
//! 断るときは**理由と値を必ず言う**: GUI のフッタ・`tako edit start` のエラー・
//! `tako edit` の応答の `limit` がすべてここの [`Truncation`] 1 つから組まれるので、
//! 画面と CLI / MCP で食い違わない。
//!
//! 上限は計測で決めた（release 実測は `architecture.md`「大きいファイルの編集」）。
//! 1 MB / 5,000 行だった頃は tako 自身の `main.rs`（8 万行）が編集できなかった。

use serde_json::{json, Value};

/// 全文を読み込むバイト数の上限（10 MB。10 進）
pub const MAX_BYTES: usize = 10_000_000;
/// 全文を読み込む行数の上限（10 万行。数え方は `str::lines` と同じ = 末尾の改行の後ろは数えない）
pub const MAX_LINES: usize = 100_000;

/// 上限に当たった理由と、そのときの値
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Truncation {
    /// バイト数の上限を超えた。`size` はファイルの実寸（読めなければ読んだバイト数）
    Bytes { size: u64 },
    /// 行数の上限を超えた。`lines` は数えた行数（バイトは上限内なので実数）
    Lines { lines: usize },
}

impl Truncation {
    /// 読み込んだ結果から上限に当たったかを決める。
    ///
    /// `read_bytes` は実際に読んだバイト数（上限 + 1 まで読む）、`file_size` は
    /// メタデータの実寸、`lines` は読んだ本文の行数。**バイトの超過を先に見る**
    /// （バイトで切った本文の行数は実数ではないので、行数を理由にしない）
    pub fn judge(read_bytes: usize, file_size: Option<u64>, lines: usize) -> Option<Self> {
        if read_bytes > MAX_BYTES {
            let size = file_size
                .unwrap_or(read_bytes as u64)
                .max(read_bytes as u64);
            return Some(Self::Bytes { size });
        }
        (lines > MAX_LINES).then_some(Self::Lines { lines })
    }

    /// 機械可読の理由（`bytes` / `lines`）
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Bytes { .. } => "bytes",
            Self::Lines { .. } => "lines",
        }
    }

    /// 何を超えたか（日本語）。例: `12.3 MB（上限 10 MB）`
    pub fn detail_ja(&self) -> String {
        match self {
            Self::Bytes { size } => format!(
                "{}（上限 {}）",
                format_megabytes(*size),
                format_megabytes(MAX_BYTES as u64)
            ),
            Self::Lines { lines } => format!(
                "{} 行（上限 {} 行）",
                group_digits(*lines as u64),
                group_digits(MAX_LINES as u64)
            ),
        }
    }

    /// 何を超えたか（英語）。例: `12.3 MB (limit 10 MB)`
    pub fn detail_en(&self) -> String {
        match self {
            Self::Bytes { size } => format!(
                "{} (limit {})",
                format_megabytes(*size),
                format_megabytes(MAX_BYTES as u64)
            ),
            Self::Lines { lines } => format!(
                "{} lines (limit {} lines)",
                group_digits(*lines as u64),
                group_digits(MAX_LINES as u64)
            ),
        }
    }

    /// 編集を断る理由（CLI / MCP のエラーと `limit.message`。日本語）
    pub fn edit_refusal(&self) -> String {
        let what = match self {
            Self::Bytes { .. } => "大きさ",
            Self::Lines { .. } => "行数",
        };
        format!(
            "{what}が上限を超えるため末尾を省略して表示しており、編集できない: {}",
            self.detail_ja()
        )
    }

    /// `tako edit` の応答に載せる形（`limit`）
    pub fn to_json(&self) -> Value {
        let mut out = json!({
            "reason": self.reason(),
            "max_bytes": MAX_BYTES,
            "max_lines": MAX_LINES,
            "message": self.edit_refusal(),
        });
        match self {
            Self::Bytes { size } => out["bytes"] = json!(size),
            Self::Lines { lines } => out["lines"] = json!(lines),
        }
        out
    }
}

/// 3 桁ごとにカンマを入れる（`100000` → `100,000`）
fn group_digits(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// 10 進の MB を小数 1 桁で（整数ならそのまま）。`10_000_000` → `10 MB`
fn format_megabytes(bytes: u64) -> String {
    let tenths = (bytes + 50_000) / 100_000;
    if tenths.is_multiple_of(10) {
        format!("{} MB", tenths / 10)
    } else {
        format!("{}.{} MB", tenths / 10, tenths % 10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 上限の値は10万行と10mb() {
        // 値そのものを固定する（旧値 1 MB / 5,000 行へ戻す退行を落とす）
        assert_eq!(MAX_BYTES, 10_000_000);
        assert_eq!(MAX_LINES, 100_000);
    }

    #[test]
    fn ちょうど上限は省略しない_1つ超えたら理由を返す() {
        assert_eq!(
            Truncation::judge(MAX_BYTES, Some(MAX_BYTES as u64), MAX_LINES),
            None
        );
        assert_eq!(
            Truncation::judge(MAX_BYTES, Some(MAX_BYTES as u64), MAX_LINES + 1),
            Some(Truncation::Lines {
                lines: MAX_LINES + 1
            })
        );
        assert_eq!(
            Truncation::judge(MAX_BYTES + 1, Some(MAX_BYTES as u64 + 1), 1),
            Some(Truncation::Bytes {
                size: MAX_BYTES as u64 + 1
            })
        );
    }

    #[test]
    fn バイトの超過を行数より先に見る() {
        // バイトで切った本文の行数は実数ではないので理由にしない
        let got = Truncation::judge(MAX_BYTES + 1, Some(25_000_000), MAX_LINES * 3);
        assert_eq!(got, Some(Truncation::Bytes { size: 25_000_000 }));
    }

    #[test]
    fn 実寸が読めなければ読んだバイト数で言う() {
        let got = Truncation::judge(MAX_BYTES + 1, None, 1);
        assert_eq!(
            got,
            Some(Truncation::Bytes {
                size: MAX_BYTES as u64 + 1
            })
        );
    }

    #[test]
    fn 理由と値が文面と応答に載る() {
        let lines = Truncation::Lines { lines: 123_456 };
        assert_eq!(lines.detail_ja(), "123,456 行（上限 100,000 行）");
        assert_eq!(lines.detail_en(), "123,456 lines (limit 100,000 lines)");
        let json = lines.to_json();
        assert_eq!(json["reason"], "lines");
        assert_eq!(json["lines"], 123_456);
        assert_eq!(json["max_lines"], MAX_LINES);
        assert_eq!(json["max_bytes"], MAX_BYTES);
        assert!(json["message"].as_str().unwrap().contains("123,456 行"));

        let bytes = Truncation::Bytes { size: 12_345_678 };
        assert_eq!(bytes.detail_ja(), "12.3 MB（上限 10 MB）");
        assert_eq!(bytes.detail_en(), "12.3 MB (limit 10 MB)");
        let json = bytes.to_json();
        assert_eq!(json["reason"], "bytes");
        assert_eq!(json["bytes"], 12_345_678);
        assert!(json.get("lines").is_none());
        assert!(bytes.edit_refusal().contains("12.3 MB（上限 10 MB）"));
    }

    #[test]
    fn 数値の整形() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1_000), "1,000");
        assert_eq!(group_digits(84_733), "84,733");
        assert_eq!(group_digits(1_000_000), "1,000,000");
        assert_eq!(format_megabytes(10_000_000), "10 MB");
        assert_eq!(format_megabytes(10_000_001), "10 MB");
        assert_eq!(format_megabytes(10_060_000), "10.1 MB");
        assert_eq!(format_megabytes(999_999), "1 MB");
    }
}
