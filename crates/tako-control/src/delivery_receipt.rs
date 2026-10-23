//! delivery_receipt — 送達の**受領の証拠**（Issue #2330）
//!
//! keys 経路（貼り付け + 分離 Enter）の送達フローは、画面だけで「入力欄へ反映された」を
//! 確かめてから送達済みと言う（#530: 空の入力欄は送信の証拠にならない）。ところが
//! 狭いペインの claude は**入力欄の高さに上限があり、長い本文はカーソル（末尾）へ
//! スクロールして描く**。`❯` は見えている最初の行に付くので、本文の先頭も、先頭の
//! `❯` 行も画面から消える（10/20 の追試で実測: 54 桁で `❯ 段落2: …` から描かれていた）。
//! 送達フローは 10 秒待って Enter を送り（実際には送信される）、反映を見ていないので
//! `gave_up / paste_not_reflected` で諦めていた。10/18 の引き継ぎでは前任がこれを
//! 「後任に届いていない」と読んで後任を閉じ、タブから master が消えた。
//!
//! # 何を証拠にするか
//!
//! 諦める直前に**受け手の会話記録（transcript）**を見て、送った本文を運んだ利用者の発話
//! （`type: "user"`）か、生成中のキュー投函（`queue-operation` の `enqueue`）が
//! **送信の後に**入っていれば受領とみなす（顛末 `received`）。届いていない送信には
//! この記録が 1 行も出来ないので、本当に届いていない送達は従来どおり `gave_up` のまま
//! （判定を広く緩めない）。
//!
//! transcript の所在が引けないとき（会話を保存しない claude 等）だけ、tako mod の報告の
//! ターン遷移（送信前 idle → Enter の後に届いた報告で busy / permission / question）を
//! 証拠にする。ターンは別の理由（バックグラウンドの通知等）でも始まるので、transcript を
//! 読めるときは使わない（読めるのに本文が無い = 届いていない）。
//!
//! # 出さないもの
//!
//! 本文・transcript の中身は**照合に使うだけ**で、ログ・応答には出さない（AGENTS.md の
//! 絶対ルール）。persist.log へ書くのは証拠の種類とペイン番号だけ。

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::Value;
use tako_core::claude_mod::{ModTurn, StoredReport};

/// 諦める前に受領の証拠を待つ秒数。claude は Enter で送信した直後に利用者の発話を
/// transcript へ書く（実測で 1 秒以内）。tick 500ms で十数回見られる長さにする
pub const CONFIRM_WINDOW_SECS: u64 = 8;

/// 照合に使う本文の長さ（空白を除いた文字数）。先頭だけを見る（貼り付けの途中で
/// claude が空白・改行を整えても、空白を除いた並びは変わらない）
const NEEDLE_CHARS: usize = 48;

/// 所在を送信前に控えられなかった transcript は**末尾だけ**を読む
/// （会話記録は数十 MB になりうる。UI スレッドで丸読みしない）
const TAIL_BYTES: u64 = 512 * 1024;

/// 控えた位置から読む追記分の上限（送信後の数秒の追記なので普通は数 KB）
const APPENDED_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// 送信時刻の取り分（秒）。記録の時刻は claude の時計で、ここは秒未満を落として比べる
const SINCE_SLACK_SECS: i64 = 2;

/// 受領の証拠の種類（persist.log と応答の材料。本文は持たない）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    /// 受け手の transcript に送った本文が入った
    Transcript,
    /// transcript を引けず、tako mod の報告でターンが始まった
    ModTurn,
}

impl Evidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Evidence::Transcript => "transcript",
            Evidence::ModTurn => "mod_turn",
        }
    }
}

/// 照合用の針（空白を除いた先頭 [`NEEDLE_CHARS`] 文字）
pub fn needle_of(prompt: &str) -> String {
    squash(prompt).chars().take(NEEDLE_CHARS).collect()
}

/// 空白（改行・タブを含む）をすべて除く。claude の整形と画面の折り返しに依らない並びにする
fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// 利用者の発話の本文（文字列か、text 部品の連結）
fn user_text(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(parts) => {
            let texts: Vec<&str> = parts
                .iter()
                .filter(|p| p["type"].as_str() == Some("text"))
                .filter_map(|p| p["text"].as_str())
                .collect();
            (!texts.is_empty()).then(|| texts.join("\n"))
        }
        _ => None,
    }
}

/// transcript の 1 レコードが、送った本文を運んだ**受領の痕跡**か（純粋関数）。
///
/// - `type: "user"`（`isMeta` でない）… ターンとして取り込まれた
/// - `type: "queue-operation"` + `operation: "enqueue"` … 生成中でキューに入った（後で読まれる）
///
/// `since` があればそれより前の時刻のレコードは数えない（所在を送信前に控えられず、
/// 末尾を読み直すときに過去の同じ本文を拾わないため）
pub fn record_carries(record: &Value, needle: &str, since: Option<i64>) -> bool {
    if needle.is_empty() {
        return false;
    }
    if let Some(since) = since {
        let at = record["timestamp"]
            .as_str()
            .and_then(crate::sessions::parse_iso);
        if at.is_none_or(|at| at < since) {
            return false;
        }
    }
    let text = match record["type"].as_str() {
        Some("user") if !record["isMeta"].as_bool().unwrap_or(false) => {
            user_text(&record["message"]["content"])
        }
        Some("queue-operation") if record["operation"].as_str() == Some("enqueue") => {
            record["content"].as_str().map(str::to_string)
        }
        _ => None,
    };
    text.is_some_and(|t| squash(&t).contains(needle))
}

/// 行の並びに受領の痕跡があるか
fn lines_carry(text: &str, needle: &str, since: Option<i64>) -> bool {
    text.lines().any(|line| {
        serde_json::from_str::<Value>(line).is_ok_and(|r| record_carries(&r, needle, since))
    })
}

/// 控えた位置から後ろ（送信後の追記分）を読む。ファイルが縮んだ（compact 等）なら None
fn read_appended(path: &Path, from: u64) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    if len < from {
        return None;
    }
    f.seek(SeekFrom::Start(from)).ok()?;
    let mut buf = Vec::new();
    f.take(APPENDED_MAX_BYTES).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// 末尾 [`TAIL_BYTES`] を読む（途中から読んだときは最初の欠けた行を捨てる）
fn read_tail(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    Some(if start > 0 {
        text.split_once('\n')
            .map(|(_, rest)| rest.to_string())
            .unwrap_or_default()
    } else {
        text
    })
}

/// tako mod の報告のうち、ターン遷移の判定に使う値（送信前に控える）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModBaseline {
    /// それまでに受け取った報告の数（Enter の後に**新しい**報告が届いたかを見る）
    pub count: u64,
    /// 送信前は idle だったか（busy 中の送信はキューに入るのでターン遷移は証拠にならない）
    pub idle: bool,
}

impl ModBaseline {
    pub fn of(stored: &StoredReport) -> Self {
        Self {
            count: stored.count,
            idle: stored.report.turn == ModTurn::Idle,
        }
    }
}

/// ターン遷移を受領の証拠とみなすか（純粋関数）。
///
/// 送信前に idle で、Enter の**後に**届いた新しい報告がターンの開始（busy / permission /
/// question）を言っていること
pub fn mod_turn_started(
    baseline: Option<ModBaseline>,
    now: Option<&StoredReport>,
    enter_at: Option<Instant>,
) -> bool {
    let (Some(base), Some(now), Some(enter_at)) = (baseline, now, enter_at) else {
        return false;
    };
    base.idle
        && now.count > base.count
        && now.received >= enter_at
        && now.report.turn != ModTurn::Idle
}

/// 1 回の送達の受領を確かめる材料。**貼り付けの直前**に [`ReceiptProbe::capture`] で控える
#[derive(Debug, Clone)]
pub struct ReceiptProbe {
    needle: String,
    /// 控えた時刻（unix 秒）から取り分を引いたもの
    since: i64,
    /// 控えた時点の transcript の所在と長さ（送信後の追記だけを読む）
    cursor: Option<(PathBuf, u64)>,
    mod_baseline: Option<ModBaseline>,
}

impl ReceiptProbe {
    pub fn capture(prompt: &str, transcript: Option<&Path>, report: Option<&StoredReport>) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Self {
            needle: needle_of(prompt),
            since: now - SINCE_SLACK_SECS,
            cursor: transcript.and_then(|p| {
                std::fs::metadata(p)
                    .ok()
                    .map(|m| (p.to_path_buf(), m.len()))
            }),
            mod_baseline: report.map(ModBaseline::of),
        }
    }

    /// 受領の証拠を探す（None = まだ無い）。
    ///
    /// `transcript` は**いま**引けた所在（送信後に作られた会話もここで拾う）。
    /// 所在が引けたなら transcript だけを見る（本文が無ければ届いていない）。
    /// 引けないときだけ tako mod のターン遷移を見る
    pub fn check(
        &self,
        transcript: Option<&Path>,
        report: Option<&StoredReport>,
        enter_at: Option<Instant>,
    ) -> Option<Evidence> {
        if let Some(path) = transcript {
            let found = match &self.cursor {
                Some((at, len)) if at == path => match read_appended(path, *len) {
                    Some(text) => lines_carry(&text, &self.needle, None),
                    None => read_tail(path)
                        .is_some_and(|t| lines_carry(&t, &self.needle, Some(self.since))),
                },
                _ => {
                    read_tail(path).is_some_and(|t| lines_carry(&t, &self.needle, Some(self.since)))
                }
            };
            return found.then_some(Evidence::Transcript);
        }
        mod_turn_started(self.mod_baseline, report, enter_at).then_some(Evidence::ModTurn)
    }
}

/// 画面では確かめられなかった送達を受領の証拠で確かめたことを persist.log へ 1 行残す
/// （本文は書かない。証拠の種類とペイン番号だけ）
pub fn log_received(pane: u64, evidence: Evidence) {
    if tako_core::prompt_delivery::legacy_silent() {
        return;
    }
    crate::diag::persist_log(&format!(
        "送達: 画面では反映を確かめられなかったが受領を確かめた（pane={pane} 証拠={}）",
        evidence.as_str()
    ));
}

/// 検証用の注入（#2330）: `TAKO_2330_INJECT_DROP_PASTE=1` で送達フローの貼り付けを捨てる
/// （TUI が貼り付けを吸った = **本当に届かない**送信を作る。受領の証拠を作らないことを測る）
pub fn inject_drop_paste() -> bool {
    std::env::var_os("TAKO_2330_INJECT_DROP_PASTE").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PROMPT: &str = "【復旧の連絡】段落1: 引き継ぎで前任と後任が互いのペインを閉じ合い、\nタブから master が消えていた件を復旧した。";

    #[test]
    fn 針は空白を除いた先頭() {
        let n = needle_of(PROMPT);
        assert!(!n.contains(' ') && !n.contains('\n'), "{n}");
        assert_eq!(n.chars().count(), NEEDLE_CHARS);
        assert!(n.starts_with("【復旧の連絡】段落1:"));
        assert_eq!(needle_of("  ok \n"), "ok");
    }

    /// claude は貼り付けを展開した本文を利用者の発話として書く（文字列 / text 部品の両形）
    #[test]
    fn 送った本文を運んだ利用者の発話は受領() {
        let n = needle_of(PROMPT);
        let as_str = json!({"type": "user", "message": {"role": "user", "content": PROMPT}});
        assert!(record_carries(&as_str, &n, None));
        // 改行・空白が整えられていても（折り返し・CRLF）並びは同じ
        let reflowed = PROMPT
            .replace('\n', "\r\n  ")
            .replace("master ", "master\n");
        let parts = json!({"type": "user", "message": {"content": [
            {"type": "text", "text": reflowed}
        ]}});
        assert!(record_carries(&parts, &n, None));
        // 生成中に打った分はキューへ入る（後でターンになる）
        let queued = json!({"type": "queue-operation", "operation": "enqueue", "content": PROMPT});
        assert!(record_carries(&queued, &n, None));
    }

    /// 届いていないものを受領と読まない（判定を広く緩めない）
    #[test]
    fn 別の発話や本文を持たないレコードは受領ではない() {
        let n = needle_of(PROMPT);
        for rec in [
            json!({"type": "user", "message": {"content": "別の依頼"}}),
            json!({"type": "user", "isMeta": true, "message": {"content": PROMPT}}),
            json!({"type": "assistant", "message": {"content": [{"type": "text", "text": PROMPT}]}}),
            json!({"type": "queue-operation", "operation": "dequeue", "content": PROMPT}),
            json!({"type": "queue-operation", "operation": "remove", "content": PROMPT}),
            json!({"type": "user", "message": {"content": [{"type": "tool_result", "content": PROMPT}]}}),
        ] {
            assert!(!record_carries(&rec, &n, None), "{rec}");
        }
        assert!(!record_carries(
            &json!({"type": "user", "message": {"content": PROMPT}}),
            "",
            None
        ));
    }

    /// 末尾を読み直すときは送信より前の同じ本文を数えない
    #[test]
    fn 送信より前の時刻のレコードは数えない() {
        let n = needle_of(PROMPT);
        let since = crate::sessions::parse_iso("2026-10-20T01:30:10Z").unwrap();
        let old = json!({"type": "user", "timestamp": "2026-10-20T01:30:09.999Z", "message": {"content": PROMPT}});
        let new = json!({"type": "user", "timestamp": "2026-10-20T01:30:10.001Z", "message": {"content": PROMPT}});
        let none = json!({"type": "user", "message": {"content": PROMPT}});
        assert!(!record_carries(&old, &n, Some(since)));
        assert!(record_carries(&new, &n, Some(since)));
        assert!(
            !record_carries(&none, &n, Some(since)),
            "時刻の無いものは数えない"
        );
    }

    fn tmp_file(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tako-2330-receipt-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("session.jsonl")
    }

    fn line(v: Value) -> String {
        format!("{v}\n")
    }

    /// 控えた位置より後ろだけを読む: 送信前からあった同じ本文は数えない
    #[test]
    fn 追記分だけを見る() {
        let path = tmp_file("appended");
        let before = line(json!({"type": "user", "message": {"content": PROMPT}}));
        std::fs::write(&path, &before).unwrap();
        let probe = ReceiptProbe::capture(PROMPT, Some(&path), None);
        assert_eq!(
            probe.check(Some(&path), None, None),
            None,
            "送信前の発話は証拠にしない"
        );
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        std::io::Write::write_all(
            &mut f,
            line(json!({"type": "user", "message": {"content": PROMPT}})).as_bytes(),
        )
        .unwrap();
        assert_eq!(
            probe.check(Some(&path), None, None),
            Some(Evidence::Transcript)
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// 送信前に所在が引けなかった（初回の発話で作られた）会話は時刻で切って読む
    #[test]
    fn 送信後に作られた会話も時刻で拾う() {
        let path = tmp_file("created");
        let _ = std::fs::remove_file(&path);
        let probe = ReceiptProbe::capture(PROMPT, None, None);
        let now = crate::diag::format_utc(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
        );
        std::fs::write(
            &path,
            line(json!({"type": "user", "timestamp": now, "message": {"content": PROMPT}})),
        )
        .unwrap();
        assert_eq!(
            probe.check(Some(&path), None, None),
            Some(Evidence::Transcript)
        );
        // 1 時間前の同じ本文は数えない
        std::fs::write(
            &path,
            line(json!({"type": "user", "timestamp": "2020-01-01T00:00:00.000Z", "message": {"content": PROMPT}})),
        )
        .unwrap();
        assert_eq!(probe.check(Some(&path), None, None), None);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    fn stored(turn: ModTurn, count: u64, received: Instant) -> StoredReport {
        let report = tako_core::claude_mod::parse_report(json!({
            "schema": tako_core::claude_mod::SCHEMA,
            "mod_version": "test",
            "at": 0,
            "turn": turn.as_str(),
        }))
        .unwrap();
        StoredReport {
            report,
            received,
            count,
            prev_received: None,
        }
    }

    /// transcript を引けないときだけ mod のターン遷移を見る
    #[test]
    fn transcriptが無いときだけmodのターン遷移を見る() {
        let t0 = Instant::now();
        let base = stored(ModTurn::Idle, 3, t0);
        let probe = ReceiptProbe::capture(PROMPT, None, Some(&base));
        let enter = Instant::now();
        let busy = stored(ModTurn::Busy, 4, Instant::now());
        assert_eq!(
            probe.check(None, Some(&busy), Some(enter)),
            Some(Evidence::ModTurn)
        );
        // Enter より前の報告・新しくない報告・idle のままは証拠にしない
        assert_eq!(
            probe.check(None, Some(&stored(ModTurn::Busy, 4, t0)), Some(enter)),
            None
        );
        assert_eq!(
            probe.check(
                None,
                Some(&stored(ModTurn::Busy, 3, Instant::now())),
                Some(enter)
            ),
            None
        );
        assert_eq!(
            probe.check(
                None,
                Some(&stored(ModTurn::Idle, 5, Instant::now())),
                Some(enter)
            ),
            None
        );
        assert_eq!(
            probe.check(None, Some(&busy), None),
            None,
            "Enter を送っていない"
        );
        // 送信前から busy だった（キュー投函）ならターン遷移は証拠にならない
        let busy_probe = ReceiptProbe::capture(PROMPT, None, Some(&stored(ModTurn::Busy, 3, t0)));
        assert_eq!(busy_probe.check(None, Some(&busy), Some(enter)), None);
        // transcript を引けるなら mod は見ない（本文が無い = 届いていない）
        let path = tmp_file("modignored");
        std::fs::write(&path, "").unwrap();
        let with_path = ReceiptProbe::capture(PROMPT, Some(&path), Some(&base));
        assert_eq!(with_path.check(Some(&path), Some(&busy), Some(enter)), None);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
