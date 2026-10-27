//! Web ビューの生成中に tako が落ちた印（#724 の W4）
//!
//! Windows の WebView2 は、生成とその直後の最初の読み込みのあいだに tako ごと落ちることがある
//! （#724: wry の入れ子のメッセージループで gpui の App が二重借用になる / COM のコールバックの
//! 中の unwrap。どちらも巻き戻せない panic = abort）。abort は `layout.json` を消さないので、
//! 落ちる直前の Web ビューは次の起動でまた開き直され、**起動のたびに同じ賭けをする**。
//!
//! そこで生成を始める前に `<data_dir>/webview-guard.json` へ「いま開こうとしている Web ビュー」
//! （[`GuardMark`] = ペインと URL）を書き、生成と最初の読み込みが済んだら消す。落ちれば印が残る。
//! 次の起動は [`split_restore`] で印に当たる Web ビューを**自動では開かず保留**にする
//! （URL はペイン / dock に残し、利用者が「開く」を押したとき = CLI / MCP の navigate で開く）。
//!
//! - 印の突き合わせはペインが先、当たらなければ URL（再起動をまたいでペイン ID は同じ =
//!   同一 ID 復元。dock へ外した後なら URL で当たる）。どちらにも当たらない印も
//!   dock の保留として残す（URL を失わない）
//! - 保留したものは開かれるか閉じられるまで印を残す（= 次の起動でも保留のまま）
//! - 読めない印は捨てる（保留しない側へ倒す = 起動を止めない）。版数の番地は持たない
//!   Local のランタイム状態（`config_share::catalog`）。欄の増減は `migration_registry` の指紋が止める
//! - 印は [`MAX_MARKS`] 件まで（古い順に捨てる）
//!
//! 書き込みは生成を始める・終える・保留を開く / 閉じるときだけ（毎フレームではない）。
//! 複数の tako が同じ data dir を使うので、読み・書き換え・保存は排他の内側で行う

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 置き場（`<data_dir>` の下）
pub const FILE: &str = "webview-guard.json";

/// 印の上限（古い順に捨てる）
pub const MAX_MARKS: usize = 16;

/// 開こうとしている Web ビュー 1 枚の印
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardMark {
    /// 表示先のペイン（dock 退避中なら None）
    #[serde(default)]
    pub pane: Option<u64>,
    /// 開こうとしている URL
    pub url: String,
}

/// `webview-guard.json` の中身
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardFile {
    /// 印（古い順）
    #[serde(default)]
    pub marks: Vec<GuardMark>,
}

/// 置き場（`TAKO_DATA_DIR` で隔離できる）。`join` の引数をリテラルで書くのは、
/// 共有分類カタログの被覆テスト（`config_share_catalog`）の走査に拾わせるため
pub fn path() -> Option<PathBuf> {
    tako_core::paths::data_dir().map(|d| d.join("webview-guard.json"))
}

/// 印を読む（無い・読めないなら空 = 何も保留しない）
pub fn load() -> Vec<GuardMark> {
    path().map(|p| load_from(&p)).unwrap_or_default()
}

/// [`load`] の置き場を指定する版
pub fn load_from(path: &Path) -> Vec<GuardMark> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<GuardFile>(&text).ok())
        .map(|f| f.marks)
        .unwrap_or_default()
}

/// 印を足す（同じ印は 1 つにまとめる）
pub fn begin(mark: &GuardMark) -> Result<(), String> {
    edit(|marks| add(marks, mark.clone()))
}

/// 印を外す（無ければ何もしない）
pub fn end(mark: &GuardMark) -> Result<(), String> {
    edit(|marks| {
        remove(marks, mark);
    })
}

/// 印を足す（純関数。同じ印は末尾へ寄せ、上限を超えたら古い順に捨てる）
pub fn add(marks: &mut Vec<GuardMark>, mark: GuardMark) {
    marks.retain(|m| *m != mark);
    marks.push(mark);
    if marks.len() > MAX_MARKS {
        let over = marks.len() - MAX_MARKS;
        marks.drain(..over);
    }
}

/// 印を外す（純関数）。外したら true
pub fn remove(marks: &mut Vec<GuardMark>, mark: &GuardMark) -> bool {
    let before = marks.len();
    marks.retain(|m| m != mark);
    marks.len() != before
}

fn edit(change: impl FnOnce(&mut Vec<GuardMark>)) -> Result<(), String> {
    let Some(path) = path() else {
        return Err(tako_core::paths::NoDataDir.to_string());
    };
    edit_at(&path, change)
}

/// 置き場を指定して印を書き換える（排他の内側で 読む → 変える → 書く）。
/// 印が空になったらファイルごと消す（平常時はファイルが無い）
pub fn edit_at(path: &Path, change: impl FnOnce(&mut Vec<GuardMark>)) -> Result<(), String> {
    let _lock = crate::config_io::lock_exclusive(path)?;
    let mut marks = load_from(path);
    let before = marks.clone();
    change(&mut marks);
    if marks == before && (path.exists() || marks.is_empty()) {
        return Ok(());
    }
    if marks.is_empty() {
        return match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{} を消せない: {e}", path.display())),
        };
    }
    let text = serde_json::to_string_pretty(&GuardFile { marks })
        .map_err(|e| format!("印を直列化できない: {e}"))?;
    crate::config_io::atomic_write(path, &text)
}

/// 起動時の復元待ち（ペイン, URL）を「自動で開く」と「保留する」に分けた結果
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RestoreSplit {
    /// 従来どおり開き直すもの（元の並び）
    pub auto: Vec<(Option<u64>, String)>,
    /// 保留するもの（ペイン, URL, 当たった印）。印に当たる復元待ちが無ければペインは None
    /// （= dock の保留として URL を残す）
    pub held: Vec<(Option<u64>, String, GuardMark)>,
}

/// 復元待ちを印で分ける（純関数）。
///
/// 印ごとに、まだ誰にも当たっていない復元待ちから **同じペイン → 同じ URL** の順で 1 つ選ぶ。
/// ペインが先なのは、生成中に最初の読み込みが進むと保存される URL が転送先へ変わりうるため
/// （ペイン ID は同一 ID 復元で再起動をまたいでも同じ）
pub fn split_restore(pending: Vec<(Option<u64>, String)>, marks: &[GuardMark]) -> RestoreSplit {
    let mut taken = vec![false; pending.len()];
    let mut held = Vec::new();
    for mark in marks {
        let by_pane = mark.pane.and_then(|p| {
            pending
                .iter()
                .enumerate()
                .position(|(i, (pane, _))| !taken[i] && *pane == Some(p))
        });
        let found = by_pane.or_else(|| {
            pending
                .iter()
                .enumerate()
                .position(|(i, (_, url))| !taken[i] && *url == mark.url)
        });
        match found {
            Some(i) => {
                taken[i] = true;
                let (pane, url) = pending[i].clone();
                held.push((pane, url, mark.clone()));
            }
            None => held.push((None, mark.url.clone(), mark.clone())),
        }
    }
    let auto = pending
        .into_iter()
        .zip(taken)
        .filter_map(|(p, t)| (!t).then_some(p))
        .collect();
    RestoreSplit { auto, held }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(pane: Option<u64>, url: &str) -> GuardMark {
        GuardMark {
            pane,
            url: url.into(),
        }
    }

    fn scratch(tag: &str) -> tako_core::test_residue::ScratchDir {
        tako_core::test_residue::ScratchDir::new(&format!("webview-guard-724-{tag}"))
    }

    #[test]
    fn 印が無ければすべて従来どおり開く() {
        let pending = vec![
            (Some(3), "https://example.com".to_string()),
            (None, "about:blank".to_string()),
        ];
        let split = split_restore(pending.clone(), &[]);
        assert_eq!(split.auto, pending);
        assert!(split.held.is_empty());
    }

    #[test]
    fn 生成中に落ちたwebビューは同じペインで保留になる() {
        let pending = vec![
            (Some(3), "https://a.example".to_string()),
            (Some(7), "https://b.example/".to_string()),
            (None, "https://c.example".to_string()),
        ];
        // 生成を始めた時点の URL は末尾の / が無い = 保存された URL（転送後）と違ってもペインで当たる
        let split = split_restore(pending, &[mark(Some(7), "https://b.example")]);
        assert_eq!(
            split.auto,
            vec![
                (Some(3), "https://a.example".to_string()),
                (None, "https://c.example".to_string()),
            ]
        );
        assert_eq!(
            split.held,
            vec![(
                Some(7),
                "https://b.example/".to_string(),
                mark(Some(7), "https://b.example")
            )]
        );
    }

    #[test]
    fn ペインで当たらなければurlで当てる() {
        // 保留中に dock へ外した（ペイン None で保存された）もの
        let pending = vec![(None, "https://a.example".to_string())];
        let split = split_restore(pending, &[mark(Some(9), "https://a.example")]);
        assert!(split.auto.is_empty());
        assert_eq!(split.held.len(), 1);
        assert_eq!(split.held[0].0, None);
    }

    #[test]
    fn どこにも当たらない印もurlを失わずdockの保留にする() {
        // 保存が間に合う前に落ちた（layout.json にまだ載っていない）もの
        let split = split_restore(vec![], &[mark(Some(5), "data:text/html,hi")]);
        assert!(split.auto.is_empty());
        assert_eq!(
            split.held,
            vec![(
                None,
                "data:text/html,hi".to_string(),
                mark(Some(5), "data:text/html,hi")
            )]
        );
    }

    #[test]
    fn 同じurlの2枚は印の数だけ保留する() {
        let pending = vec![
            (Some(1), "https://same.example".to_string()),
            (Some(2), "https://same.example".to_string()),
        ];
        let split = split_restore(pending, &[mark(None, "https://same.example")]);
        assert_eq!(split.held.len(), 1);
        assert_eq!(split.auto.len(), 1);
    }

    #[test]
    fn 印の書き込みと読み出しと消し方() {
        let dir = scratch("rw");
        let file = dir.path().join(FILE);
        assert!(load_from(&file).is_empty(), "平常時はファイルが無い");
        let a = mark(Some(3), "https://a.example");
        let b = mark(None, "about:blank");
        edit_at(&file, |m| add(m, a.clone())).unwrap();
        edit_at(&file, |m| add(m, b.clone())).unwrap();
        // 同じ印を足しても 1 つ
        edit_at(&file, |m| add(m, a.clone())).unwrap();
        assert_eq!(load_from(&file), vec![b.clone(), a.clone()]);
        // 外す
        edit_at(&file, |m| {
            remove(m, &b);
        })
        .unwrap();
        assert_eq!(load_from(&file), vec![a.clone()]);
        // 最後の 1 つを外すとファイルごと消える
        edit_at(&file, |m| {
            remove(m, &a);
        })
        .unwrap();
        assert!(!file.exists(), "印が空ならファイルを残さない");
        // 無い印を外しても何も作らない
        edit_at(&file, |m| {
            remove(m, &a);
        })
        .unwrap();
        assert!(!file.exists());
    }

    #[test]
    fn 読めない印は捨てて起動を止めない() {
        let dir = scratch("broken");
        let file = dir.path().join(FILE);
        std::fs::write(&file, "{ 書きかけ").unwrap();
        assert!(load_from(&file).is_empty());
        // 読めない中身の上に足すと、読める形で書き直る
        edit_at(&file, |m| add(m, mark(None, "about:blank"))).unwrap();
        assert_eq!(load_from(&file), vec![mark(None, "about:blank")]);
    }

    #[test]
    fn 欄が欠けた旧い印も読める() {
        let dir = scratch("old");
        let file = dir.path().join(FILE);
        std::fs::write(&file, r#"{"marks":[{"url":"about:blank"}]}"#).unwrap();
        assert_eq!(load_from(&file), vec![mark(None, "about:blank")]);
    }

    #[test]
    fn 置き場の名前は定数と同じ() {
        let p = path().expect("テストでも data dir は決まる");
        assert_eq!(p.file_name().and_then(|n| n.to_str()), Some(FILE));
    }

    #[test]
    fn 印は上限で古い順に捨てる() {
        let mut marks = Vec::new();
        for i in 0..(MAX_MARKS as u64 + 3) {
            add(&mut marks, mark(Some(i), "about:blank"));
        }
        assert_eq!(marks.len(), MAX_MARKS);
        assert_eq!(marks[0].pane, Some(3));
        assert_eq!(marks.last().unwrap().pane, Some(MAX_MARKS as u64 + 2));
    }
}
