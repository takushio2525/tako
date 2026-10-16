//! 「読めなければ既定値へ落とす」データファイルを既定の置き場から読む口（#1824）
//!
//! `recent.json` / `remote/shortcuts.json` の読み手は tako-core にあり、壊れた中身は
//! `migration::quarantine_unreadable` で退避するが、診断ログ（persist.log）へは書けない。
//! 以前は 1 回目の破損でも何も残らず、「いつ壊れていたか」を後から追えなかった。
//! ここが core の読み手から理由（[`LoadIssue`]）を受け取り、**移行の記録と同じ形の 1 行**
//! （`移行できず: <種別> <パス>（理由・退避先・発生源 load）`）で残す。
//!
//! 記録は**壊れた中身ごとに 1 回**（settings.json / ui.json と同じ規則。#1819）:
//!
//! - 退避が今回新しく写した = この中身を初めて見た → 書く
//! - 同じ中身の退避が既にあった（起動時の移行か前の読み込みが写して記録した）→ 書かない。
//!   CLI は呼ぶたびに別プロセスなので、プロセス内の重複除けだけでは毎回 1 行増える
//! - 退避できなかった・読めなかった（権限・UTF-8 でない）→ プロセス内で同じ中身 / 理由につき 1 回
//! - 読めた（直った）→ 忘れる（同じ中身でまた壊れたら、それも記録する）
//!
//! **ファイルの中身は載せない**（AGENTS.md の絶対ルール）。載るのはパスと serde の理由
//! （行・列と期待した型）と退避先だけ。

use std::path::Path;

use tako_core::load_issue::LoadIssue;
use tako_core::migration::SchemaId;
use tako_core::recent::RecentList;
use tako_core::remote_shortcuts::ShortcutsFile;

use crate::settings::UnreadableSeen;

/// 記録の発生源（移行の記録の `runtime` / `setup` / `cli` と並ぶ）
const ORIGIN: &str = "load";

static RECENT_SEEN: UnreadableSeen = UnreadableSeen::new();
static SHORTCUTS_SEEN: UnreadableSeen = UnreadableSeen::new();

/// #1824 の A/B。`TAKO_1824_LEGACY=1` で修正前の挙動（読み手の破損を記録しない・
/// 復元に使えない layout.json を `layout.json.corrupt` へ rename して前回分を上書きする）
pub fn legacy() -> bool {
    std::env::var_os("TAKO_1824_LEGACY").is_some()
}

/// 最近使った場所（`<data_dir>/recent.json`）を読む。壊れていれば既定値（空）で、
/// 中身は core が退避し、ここが persist.log へ 1 行残す
pub fn load_recent() -> RecentList {
    let Some(path) = tako_core::recent::recent_path() else {
        return RecentList::default();
    };
    let (list, issue) = RecentList::load_from_checked(&path);
    report(SchemaId::Recent, &path, issue.as_ref(), &RECENT_SEEN);
    list
}

/// リモート閲覧のショートカット（`<data_dir>/remote/shortcuts.json`）を読む。
/// 壊れていれば登録分なし（既定のショートカットだけ）で、記録は [`load_recent`] と同じ
pub fn load_remote_shortcuts() -> ShortcutsFile {
    let Some(path) = tako_core::remote_shortcuts::shortcuts_path() else {
        return ShortcutsFile::default();
    };
    let (file, issue) = ShortcutsFile::load_from_checked(&path);
    report(
        SchemaId::RemoteShortcuts,
        &path,
        issue.as_ref(),
        &SHORTCUTS_SEEN,
    );
    file
}

/// 落とした理由を persist.log へ。書いた 1 行を返す（テストが配線ごと確かめる口）
fn report(
    id: SchemaId,
    path: &Path,
    issue: Option<&LoadIssue>,
    seen: &UnreadableSeen,
) -> Option<String> {
    let line = report_line(id, path, issue, seen)?;
    crate::diag::persist_log(&line);
    Some(line)
}

/// [`report`] の判断の本体（書かずに 1 行だけ組む）
fn report_line(
    id: SchemaId,
    path: &Path,
    issue: Option<&LoadIssue>,
    seen: &UnreadableSeen,
) -> Option<String> {
    let Some(issue) = issue else {
        seen.forget(path);
        return None;
    };
    if legacy() {
        return None;
    }
    match issue {
        LoadIssue::Unparsable { reason, quarantine } => {
            if quarantine.as_ref().is_some_and(|q| !q.copied) {
                return None;
            }
            let body = std::fs::read(path).unwrap_or_default();
            let key = format!("{reason}\n{}", tako_core::fnv::fnv1a64(&body));
            if !seen.first_sighting(path, &key) {
                return None;
            }
            Some(crate::migrations::unreadable_line(
                id,
                path,
                reason,
                quarantine.as_ref().map(|q| q.path.as_path()),
                quarantine.as_ref().is_some_and(|q| q.evicted),
                "退避できず",
                ORIGIN,
            ))
        }
        LoadIssue::ReadFailed { reason } => seen
            .first_sighting(path, reason)
            .then(|| crate::migrations::aborted_line(id, path, reason, ORIGIN)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一時 dir（テストごとに別。**本番の data dir に触れない**）
    fn tmp(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tako-1824-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("作れる");
        assert!(dir.starts_with(std::env::temp_dir()), "一時 dir の中だけ");
        dir
    }

    /// 1 回目の破損で「移行できず」と同じ形の 1 行が組まれ、同じ中身の読み直しでは組まれない
    #[test]
    fn 壊れた中身ごとに一度だけ記録する() {
        let dir = tmp("once");
        let path = dir.join("recent.json");
        let seen = UnreadableSeen::new();
        std::fs::write(&path, "[こわれた").expect("書ける");
        let (_, issue) = RecentList::load_from_checked(&path);
        let line =
            report_line(SchemaId::Recent, &path, issue.as_ref(), &seen).expect("1 回目は記録する");
        assert!(
            line.starts_with(&format!("移行できず: recent {}（", path.display())),
            "移行の記録と同じ形: {line}"
        );
        assert!(
            line.contains("recent.json.unreadable.bak・発生源 load）"),
            "{line}"
        );
        assert!(!line.contains("こわれた"), "中身を載せない: {line}");

        // 同じ中身をもう一度読む = 退避は写さない = 記録しない
        let (_, again) = RecentList::load_from_checked(&path);
        assert_eq!(
            report_line(SchemaId::Recent, &path, again.as_ref(), &seen),
            None
        );

        // 別の中身で 2 回目 = 2 本目へ写して記録する
        std::fs::write(&path, "").expect("書ける");
        let (_, second) = RecentList::load_from_checked(&path);
        let line = report_line(SchemaId::Recent, &path, second.as_ref(), &seen)
            .expect("別の中身は記録する");
        assert!(line.contains("recent.json.unreadable.2.bak"), "{line}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 退避が写せなかった（同じ中身を何度読んでも copied の判断ができない）ときは
    /// プロセス内の重複除けが効く。読めたら忘れる
    #[test]
    fn 退避できないときはプロセス内で一度() {
        let dir = tmp("noq");
        let path = dir.join("shortcuts.json");
        std::fs::write(&path, "{こわれた").expect("書ける");
        let seen = UnreadableSeen::new();
        let issue = LoadIssue::Unparsable {
            reason: "expected value".into(),
            quarantine: None,
        };
        let line = report_line(SchemaId::RemoteShortcuts, &path, Some(&issue), &seen)
            .expect("1 回目は記録する");
        assert!(line.contains("・退避できず・発生源 load）"), "{line}");
        assert_eq!(
            report_line(SchemaId::RemoteShortcuts, &path, Some(&issue), &seen),
            None
        );
        // 読めた = 直った → 次に同じ形で壊れたらまた記録する
        assert_eq!(
            report_line(SchemaId::RemoteShortcuts, &path, None, &seen),
            None
        );
        assert!(report_line(SchemaId::RemoteShortcuts, &path, Some(&issue), &seen).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 読めない（権限・UTF-8 でない）は「移行を中止」と同じ形で、理由ごとに 1 回
    #[test]
    fn 読めないファイルは中止の形で一度() {
        let dir = tmp("readfail");
        let path = dir.join("recent.json");
        std::fs::write(&path, [0xff_u8, 0xfe, b'{']).expect("書ける");
        let (list, issue) = RecentList::load_from_checked(&path);
        assert!(list.entries.is_empty());
        assert!(
            matches!(issue, Some(LoadIssue::ReadFailed { .. })),
            "{issue:?}"
        );
        let seen = UnreadableSeen::new();
        let line =
            report_line(SchemaId::Recent, &path, issue.as_ref(), &seen).expect("1 回目は記録する");
        assert!(
            line.starts_with(&format!(
                "移行を中止: recent {}（読み取りに失敗: ",
                path.display()
            )),
            "{line}"
        );
        assert_eq!(
            report_line(SchemaId::Recent, &path, issue.as_ref(), &seen),
            None
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 不在と正常は記録しない
    #[test]
    fn 不在と正常は記録しない() {
        let dir = tmp("ok");
        let path = dir.join("recent.json");
        let seen = UnreadableSeen::new();
        let (_, absent) = RecentList::load_from_checked(&path);
        assert_eq!(
            report_line(SchemaId::Recent, &path, absent.as_ref(), &seen),
            None
        );
        std::fs::write(&path, r#"{"entries": [{"type": "ssh", "host": "h"}]}"#).expect("書ける");
        let (list, ok) = RecentList::load_from_checked(&path);
        assert_eq!(list.entries.len(), 1);
        assert_eq!(
            report_line(SchemaId::Recent, &path, ok.as_ref(), &seen),
            None
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
