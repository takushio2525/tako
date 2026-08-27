//! claude_session — claude の**構造化されたセッション台帳**（Issue #1011 / エピック #1001）
//!
//! ## 何を解決するか
//!
//! `claude agents --json` は 1 回で **CPU 0.22〜0.23 秒 / 2.0 G 命令 / ピーク RSS 339 MB**
//! を使う（Node の起動そのものが支配項）。tako はアカウント切替（#504 / #512）のために
//! **走査先の config ディレクトリごとに 1 本ずつ並列起動**する（#571 の対策として正しい設計）
//! ので、実測 4 アカウントで 1 スキャン **CPU 0.92 秒 / ピーク RSS 1.36 GB**。
//! master が worker を監視している間は TTL 5 秒が実効間隔になり **1 コアの約 18%** に達する。
//!
//! そこで **「そのアカウントに live な claude が居るか」を Node を起こさずに確かめる材料**が要る。
//! それがこのモジュール。#984（codex の rollout JSONL）と同じ「構造化ソース直読み」の型。
//!
//! ## 実測（2026-08-28 / claude 2.1.232 / macOS）
//!
//! 置き場は **`<config dir>/sessions/<pid>.json`**（1 セッション 1 ファイル）。中身は
//!
//! ```json
//! {"pid":15688,"sessionId":"…","cwd":"…","startedAt":1787842880870,
//!  "procStart":"Thu Aug 27 15:01:20 2026","version":"2.1.232","peerProtocol":1,
//!  "kind":"interactive","entrypoint":"cli","messagingSocketPath":"/tmp/cc-socks/15688.sock",
//!  "name":"…","nameSource":"derived","nameSince":…,"status":"busy",
//!  "updatedAt":…,"statusUpdatedAt":…}
//! ```
//!
//! 確かめたこと（この 4 つが揃わないとガードの材料に使えない）:
//!
//! 1. **`claude agents --json` の出力集合と完全一致**。既定 config dir で 14 件 / 14 件、
//!    別アカウントで 5 件 / 5 件が pid・sessionId・status まで一致した
//! 2. **フィールドも一致**（CLI の出力キーは `cwd` / `kind` / `name` / `pid` / `sessionId` /
//!    `startedAt` / `status` の 7 つだけで、ファイルはその上位集合）
//! 3. **`status` は逐次更新される**（同一ディレクトリ内に `idle` / `busy` / `shell` が並び
//!    `statusUpdatedAt` が動く）。起動時の 1 回書きではない
//! 4. **終了時にファイルは消える**（生きているセッション数とファイル数が一致。
//!    `claude -p` 相当の短命セッションも出現 → 消滅を観測した）
//!
//! CLI との差は 1 つだけ見つかっている: ファイルの `status: "shell"` を CLI は `"busy"` へ
//! 正規化する。**このモジュールは status を判断に使わない**（生存だけを見る）ので影響しない。
//!
//! ## なぜ「答え」にせず「ガードの材料」に留めるか
//!
//! 上記は**上流の内部レイアウト**であり、公開仕様ではない。`claude agents --json` を
//! これで置き換えると、claude 側がレイアウトを変えた瞬間に「エージェントが 1 件も居ない」と
//! 見えて **worker 監視が黙って壊れる**（#571 が潰した症状そのもの）。
//!
//! そこで #1011 では
//!
//! - **判断の答えは従来どおり `claude agents --json`**（1 バイトも解釈を変えない）
//! - このモジュールは **「起こさなくてよい Node を見分ける」ためだけ**に使う
//! - 見分けは**保守的**（読めない・パースできない・生存が判らない → 必ず起こす）
//! - **既定 config dir は必ず起こす**ので、レイアウト変更は次のスキャンで
//!   `agents_source_looks_complete` が検出して以後ガードを止める（自己検証）
//!
//! 「答え」に昇格させると Node 起動を **0 本**にできる（上の 1〜4 がその根拠）。
//! それは別スライスの判断材料として Issue へ記録した。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// セッション台帳のディレクトリ名（`<config dir>/sessions`）
const SESSIONS_DIR: &str = "sessions";

/// 台帳 1 件（**生存判定に必要な分だけ**を持つ。status は意図的に持たない）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEntry {
    /// そのセッションを走らせているプロセス
    pub pid: u32,
    /// claude の session_id（transcript 参照キー）
    pub session_id: String,
    /// `interactive` / それ以外（`claude -p` 等）
    pub kind: Option<String>,
}

/// 台帳を読んだ結果。
///
/// **3 通りを混ぜない**のが肝（混ぜると上流のレイアウト変更が
/// 「エージェント 0 件」に化けて worker を黙って見失う）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerRead {
    /// 台帳ディレクトリを読めた。中身は**生きている**セッションだけ（`pid` の昇順）
    Live(Vec<SessionEntry>),
    /// 台帳ディレクトリが**無い**（`NotFound`）。
    ///
    /// そのアカウントで claude が一度も起動していない状態がこれ（起動すれば
    /// 台帳が作られる）。ただし**台帳を書かない claude** でも同じ形になるので、
    /// これ単体では「live 0 件」と言い切れない。他の走査先で台帳の仕組みが
    /// 確認できているときに限り空と読んでよい（[`mechanism_confirmed`]）
    Missing,
    /// ディレクトリは在るのに解釈できない（列挙が `NotFound` 以外で失敗 /
    /// `*.json` が 1 件もセッションとして解釈できない）。**判断に使ってはいけない**
    Unreadable,
}

impl LedgerRead {
    /// 「live な claude は 1 件も居ない」と**それ単体で**言い切れるか。
    /// `Missing` / `Unreadable` では決して true にならない
    pub fn is_provably_empty(&self) -> bool {
        matches!(self, Self::Live(v) if v.is_empty())
    }

    /// 生きている pid（`Live` 以外なら空）
    pub fn live_pids(&self) -> Vec<u32> {
        match self {
            Self::Live(v) => v.iter().map(|e| e.pid).collect(),
            Self::Missing | Self::Unreadable => Vec::new(),
        }
    }
}

/// 走査先のどれかで台帳ディレクトリを読めたか（= **この claude は台帳を書く**）。
///
/// これが true のときだけ [`LedgerRead::Missing`] を「そのアカウントでは
/// claude が一度も起動していない = live 0 件」と読める。false のときに
/// `Missing` を空と読むと、台帳を書かない claude で全アカウントを取りこぼす
pub fn mechanism_confirmed(reads: &[LedgerRead]) -> bool {
    reads.iter().any(|r| matches!(r, LedgerRead::Live(_)))
}

/// `<config dir>/sessions`
pub fn ledger_dir(config_dir: &Path) -> PathBuf {
    config_dir.join(SESSIONS_DIR)
}

/// 台帳 1 ファイルの中身を [`SessionEntry`] へ（純関数）。
///
/// `pid` と `sessionId` の両方が引けるものだけを採る。片方でも欠けていれば
/// 「解釈できないファイル」として扱い、`Unreadable` 判定の材料にする
pub fn parse_session_json(text: &str) -> Option<SessionEntry> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let pid = u32::try_from(value["pid"].as_u64()?).ok()?;
    if pid == 0 {
        return None;
    }
    let session_id = value["sessionId"].as_str().filter(|s| !s.is_empty())?;
    Some(SessionEntry {
        pid,
        session_id: session_id.to_string(),
        kind: value["kind"].as_str().map(|s| s.to_string()),
    })
}

/// 台帳ディレクトリを読み、**生きているセッションだけ**を返す。
///
/// `alive` は pid の生存オラクル（テストから差し替えられる）。実運用の入口は
/// [`read_ledger`]。
///
/// `Unreadable` になるのは 3 通り:
/// - ディレクトリを列挙できない（無い / 権限が無い）
/// - `*.json` が 1 件以上あるのに **1 件も**セッションとして解釈できない
///   （= 上流がこの場所を別の用途に使い始めた疑い）
pub fn read_ledger_with(dir: &Path, alive: &dyn Fn(u32) -> bool) -> LedgerRead {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        // 「無い」と「読めない」を分ける。前者はそのアカウントで claude が
        // 一度も起動していないだけなので、台帳の仕組みが確認できていれば空と読める
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return LedgerRead::Missing,
        Err(_) => return LedgerRead::Unreadable,
    };
    let mut json_files = 0usize;
    let mut parsed: Vec<SessionEntry> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        json_files += 1;
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(session) = parse_session_json(&text) {
            parsed.push(session);
        }
    }
    if json_files > 0 && parsed.is_empty() {
        return LedgerRead::Unreadable;
    }
    parsed.retain(|s| alive(s.pid));
    parsed.sort_by_key(|s| s.pid);
    parsed.dedup_by_key(|s| s.pid);
    LedgerRead::Live(parsed)
}

/// 複数の config ディレクトリの台帳を**生存判定を 1 回だけ組んで**読む（実運用の入口）。
///
/// Windows の [`tako_core::platform::process::pid_alive`] は 1 回ごとに Toolhelp
/// スナップショットを採るので、pid ごとに呼ぶと走査先 × セッション数ぶん積む。
/// **答えを持っている環境ではスナップショットを 1 回だけ採って集合で引く**
/// （材料は「OS 名」ではなく「境界が答えを持っているか」。
/// `agents::capture_process_table` と同じ作法）
pub fn read_ledgers(config_dirs: &[Option<PathBuf>]) -> Vec<LedgerRead> {
    let snapshot = tako_core::platform::procinfo::snapshot();
    let live: Option<HashSet<u32>> = match snapshot.is_empty() {
        true => None,
        false => Some(snapshot.into_iter().map(|p| p.pid).collect()),
    };
    let alive: Box<dyn Fn(u32) -> bool> = match live {
        Some(set) => Box::new(move |pid| set.contains(&pid)),
        None => Box::new(tako_core::platform::process::pid_alive),
    };
    config_dirs
        .iter()
        .map(|dir| match dir {
            Some(dir) => read_ledger_with(&ledger_dir(dir), alive.as_ref()),
            // config ディレクトリが決まらない（ホームが取れない等）= 材料が無い
            None => LedgerRead::Unreadable,
        })
        .collect()
}

/// 1 ディレクトリぶんの [`read_ledgers`]
pub fn read_ledger(config_dir: &Path) -> LedgerRead {
    read_ledgers(&[Some(config_dir.to_path_buf())])
        .pop()
        .unwrap_or(LedgerRead::Unreadable)
}

/// 台帳が `claude agents --json` の結果を**取りこぼしていないか**（純関数。自己検証）。
///
/// `agents_pids` は実際に起こした `claude agents --json` が返した pid、
/// `ledger` は同じ走査先の台帳読み取り結果。
///
/// 台帳が知らない pid が CLI から返ってきたら、台帳は「その走査先の live を
/// 言い切れる材料」ではない = **以後ガードを使ってはいけない**。
///
/// `Missing` / `Unreadable` は「材料が無い」だけなので不完全とは言わない
/// （それらは台帳の中身を主張していないので、取りこぼしの証拠にならない）
pub fn agents_source_looks_complete(ledger: &LedgerRead, agents_pids: &[u32]) -> bool {
    let LedgerRead::Live(live) = ledger else {
        return true;
    };
    let known: HashSet<u32> = live.iter().map(|e| e.pid).collect();
    agents_pids.iter().all(|pid| known.contains(pid))
}

/// `claude agents --json` の出力（配列 JSON）から pid を拾う（純関数）
pub fn agents_json_pids(stdout: &[u8]) -> Vec<u32> {
    let Ok(text) = std::str::from_utf8(stdout) else {
        return Vec::new();
    };
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(text)
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|a| a["pid"].as_u64().and_then(|p| u32::try_from(p).ok()))
        .filter(|p| *p != 0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 実採取（2026-08-28 / claude 2.1.232）を最小化したもの。
    /// **実物のキー名と型をそのまま固定する**（上流が変わったら落ちるのが正しい）
    fn 実採取の台帳(pid: u32, sid: &str, status: &str) -> String {
        format!(
            r#"{{"pid":{pid},"sessionId":"{sid}","cwd":"/w","startedAt":1787842880870,
               "procStart":"Thu Aug 27 15:01:20 2026","version":"2.1.232","peerProtocol":1,
               "kind":"interactive","entrypoint":"cli",
               "messagingSocketPath":"/tmp/cc-socks/{pid}.sock","name":"tako-17",
               "nameSource":"derived","nameSince":1787842880870,"status":"{status}",
               "updatedAt":1787842881606,"statusUpdatedAt":1787842881606}}"#
        )
    }

    #[test]
    fn 実採取の台帳からpidとsession_idを引ける() {
        let e = parse_session_json(&実採取の台帳(15688, "eff157e5-edb0", "busy"))
            .expect("実物の形はパースできる");
        assert_eq!(e.pid, 15688);
        assert_eq!(e.session_id, "eff157e5-edb0");
        assert_eq!(e.kind.as_deref(), Some("interactive"));
    }

    #[test]
    fn pidかsession_idが欠けた台帳は解釈できないものとして落とす() {
        assert!(parse_session_json(r#"{"sessionId":"a"}"#).is_none());
        assert!(parse_session_json(r#"{"pid":1}"#).is_none());
        assert!(parse_session_json(r#"{"pid":0,"sessionId":"a"}"#).is_none());
        assert!(parse_session_json(r#"{"pid":1,"sessionId":""}"#).is_none());
        assert!(parse_session_json("not json").is_none());
    }

    #[test]
    fn 台帳ディレクトリが無いのはmissingで単体では言い切らない() {
        let tmp = std::env::temp_dir().join(format!("tako-1011-none-{}", std::process::id()));
        let r = read_ledger_with(&ledger_dir(&tmp), &|_| true);
        assert_eq!(r, LedgerRead::Missing);
        assert!(
            !r.is_provably_empty(),
            "台帳を書かない claude と区別できないので単体では空と言えない"
        );
        // 仕組みが確認できていなければ Missing だけでは何も言えない
        assert!(!mechanism_confirmed(&[
            LedgerRead::Missing,
            LedgerRead::Unreadable
        ]));
        assert!(mechanism_confirmed(&[
            LedgerRead::Missing,
            LedgerRead::Live(Vec::new())
        ]));
    }

    #[test]
    fn 台帳が空ならliveゼロと言い切れる() {
        let tmp = std::env::temp_dir().join(format!("tako-1011-empty-{}", std::process::id()));
        let dir = ledger_dir(&tmp);
        std::fs::create_dir_all(&dir).expect("作れる");
        let r = read_ledger_with(&dir, &|_| true);
        assert_eq!(r, LedgerRead::Live(Vec::new()));
        assert!(r.is_provably_empty());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn 死んだpidの台帳は数えない() {
        let tmp = std::env::temp_dir().join(format!("tako-1011-dead-{}", std::process::id()));
        let dir = ledger_dir(&tmp);
        std::fs::create_dir_all(&dir).expect("作れる");
        std::fs::write(dir.join("111.json"), 実採取の台帳(111, "aaa", "idle")).expect("書ける");
        std::fs::write(dir.join("222.json"), 実採取の台帳(222, "bbb", "busy")).expect("書ける");
        let r = read_ledger_with(&dir, &|pid| pid == 222);
        assert_eq!(r.live_pids(), vec![222]);
        assert!(!r.is_provably_empty());
        // 全部死んでいれば「言い切れる空」
        let all_dead = read_ledger_with(&dir, &|_| false);
        assert!(all_dead.is_provably_empty());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn json_はあるのに1件も解釈できないならunreadable() {
        let tmp = std::env::temp_dir().join(format!("tako-1011-alien-{}", std::process::id()));
        let dir = ledger_dir(&tmp);
        std::fs::create_dir_all(&dir).expect("作れる");
        // 上流がこの場所を別の用途に使い始めた想定
        std::fs::write(dir.join("index.json"), r#"{"schema":2,"entries":[]}"#).expect("書ける");
        let r = read_ledger_with(&dir, &|_| true);
        assert_eq!(r, LedgerRead::Unreadable);
        assert!(!r.is_provably_empty());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn 台帳が知らないpidをcliが返したら不完全と判断する() {
        let ledger = LedgerRead::Live(vec![SessionEntry {
            pid: 10,
            session_id: "a".into(),
            kind: None,
        }]);
        assert!(agents_source_looks_complete(&ledger, &[10]));
        assert!(!agents_source_looks_complete(&ledger, &[10, 11]));
        // 材料が無いだけの Missing / Unreadable を「不完全」と言うと永久にガードが止まる
        assert!(agents_source_looks_complete(
            &LedgerRead::Unreadable,
            &[10, 11]
        ));
        assert!(agents_source_looks_complete(
            &LedgerRead::Missing,
            &[10, 11]
        ));
    }

    #[test]
    fn agents_jsonからpidを拾う() {
        let out = br#"[{"pid":1,"sessionId":"a"},{"pid":0},{"sessionId":"b"},{"pid":2}]"#;
        assert_eq!(agents_json_pids(out), vec![1, 2]);
        assert!(agents_json_pids(b"[]").is_empty());
        assert!(agents_json_pids(b"not json").is_empty());
    }
}
