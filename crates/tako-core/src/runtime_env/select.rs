//! 利用者が選んだ実行環境を、保存する参照へ写す（Issue #1731 / #1726 S3）
//!
//! `tako run-config a.py --runtime <指定>` / MCP `tako_run_defaults` の `runtime` が受ける形は
//! 3 つある（設計書 §8.2）。
//!
//! | 形 | 例 | 解き方 |
//! |---|---|---|
//! | 候補の ID | `<検出器 ID>:<鍵>` | そのまま参照にする（候補に無くても、表の検出器 ID なら受ける） |
//! | 短縮 | 環境のディレクトリ名・検出器 ID・版名 | 候補の鍵 / 鍵の末尾 / 検出器 ID / 表示名と突き合わせる |
//! | パス | interpreter の実体・環境のディレクトリ | 呼び出し側が絶対パスにしてから [`ref_for_path`] |
//!
//! **短縮が複数の候補に当たったら 1 つを選ばない**（#1466。monorepo の `.venv` が 2 段にあるとき、
//! 近い方を黙って選ぶと「保存したのに別の環境で走る」になる）。候補を並べて返し、呼び出し側が
//! ID で選び直してもらう。
//!
//! 言語・道具の名前は書かない（表の `Detector` の戦略で決める = 番犬
//! `crates/tako-control/tests/issue1729_runtime_env_table_watchdog.rs`）。

use std::path::Path;

use super::detect::{file_name, msg_interpreter_missing, path_str, tr};
use super::{is_path_like, Candidate, Detector, FsProbe, RuntimeKind, Unresolved};
use crate::runner_config::RuntimeRef;

/// 指定を参照へ写せなかった
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    /// 短縮が複数の候補に当たった（当たった候補の ID。並びは自動選択の順）
    Ambiguous(Vec<String>),
    /// どの候補にも当たらない
    NotFound,
}

/// 候補の ID（`<検出器 ID>:<鍵>`。応答の `runtimes[].id` と `--runtime` が受ける形）
pub fn candidate_id(c: &Candidate) -> String {
    format!("{}:{}", c.manager, c.key)
}

/// 指定（ID か短縮）を参照へ写す。パスの形は扱わない（[`ref_for_path`] へ）
pub fn match_query(
    kind: &RuntimeKind,
    query: &str,
    candidates: &[Candidate],
) -> Result<RuntimeRef, QueryError> {
    let q = query.trim();
    if q.is_empty() {
        return Err(QueryError::NotFound);
    }
    // 1. 候補の ID（`<検出器 ID>:<鍵>`）。Windows の `C:\…` を ID と読まないよう、
    //    前半が表の検出器 ID のときだけ ID として扱う
    if let Some((manager, key)) = q.split_once(':') {
        if !key.is_empty() && kind.manager_ids().contains(&manager) {
            return Ok(RuntimeRef::new(manager, key));
        }
    }
    // 2. 短縮
    let mut hits: Vec<&Candidate> = Vec::new();
    for c in candidates {
        let tail = file_name(Path::new(&c.key));
        let hit = c.key == q || c.manager == q || c.label == q || tail == q;
        if hit
            && !hits
                .iter()
                .any(|h| h.manager == c.manager && h.key == c.key)
        {
            hits.push(c);
        }
    }
    match hits.as_slice() {
        [one] => Ok(one.reference()),
        [] => Err(QueryError::NotFound),
        many => Err(QueryError::Ambiguous(
            many.iter().map(|c| candidate_id(c)).collect(),
        )),
    }
}

/// パスの指定を参照へ写す（`path` は呼び出し側が絶対パスにしたもの）。
///
/// - ファイル = interpreter の実体を直接指定（[`RuntimeRef::EXPLICIT_PATH`]）
/// - ディレクトリ = 印のファイルで環境と分かる検出器（表の [`Detector::MarkerDir`]）の鍵
///
/// どちらでもなければ `None`
pub fn ref_for_path(kind: &RuntimeKind, path: &Path, fs: &dyn FsProbe) -> Option<RuntimeRef> {
    if fs.is_file(path) {
        return Some(RuntimeRef::new(RuntimeRef::EXPLICIT_PATH, path_str(path)));
    }
    if !fs.is_dir(path) {
        return None;
    }
    kind.detectors.iter().find_map(|d| match d {
        Detector::MarkerDir(s) if fs.is_file(&path.join(s.marker)) => {
            Some(RuntimeRef::new(s.id, path_str(path)))
        }
        _ => None,
    })
}

/// 解いた候補が**今走らせられるか**（interpreter のパスが実在するか）。
///
/// 保存した `.venv` の `python` だけが消えた（印の `pyvenv.cfg` は残った）環境は、
/// 参照としては解けても走らせると「そのファイルは無い」で落ちる。自動選択なら次の候補へ
/// 進める（[`super::Detection::demote_missing_interpreters`]）が、**保存した選択は既定へ落とさず**
/// 解けないものとして返す（#1466）。素の名前（`python3`）は実行ペインのシェルが解くので見ない
pub fn ensure_runnable(c: Candidate, fs: &dyn FsProbe) -> Result<Candidate, Unresolved> {
    match c.applied.program.first() {
        Some(first) if is_path_like(first) && !fs.is_file(Path::new(first)) => Err(Unresolved {
            manager: c.manager.to_string(),
            key: c.key.clone(),
            reason: msg_interpreter_missing(&c.label, first),
        }),
        _ => Ok(c),
    }
}

/// [`QueryError`] の人向けの文（日英）。`ids` は当たった候補 / 選べる候補
pub fn msg_query_error(query: &str, err: &QueryError, candidates: &[Candidate]) -> String {
    let list = |ids: &[String]| -> String {
        if ids.is_empty() {
            "-".to_string()
        } else {
            ids.join(" / ")
        }
    };
    match err {
        QueryError::Ambiguous(ids) => tr(
            format!(
                "「{query}」に当たる実行環境が {} 件ある（1 つを選ばない）。ID で選び直す: {}",
                ids.len(),
                list(ids)
            ),
            format!(
                "\"{query}\" matches {} runtimes, so none is picked. Choose one by ID: {}",
                ids.len(),
                list(ids)
            ),
        ),
        QueryError::NotFound => {
            let ids: Vec<String> = candidates.iter().map(candidate_id).collect();
            tr(
                format!(
                    "「{query}」に当たる実行環境が無い。選べる候補: {}",
                    list(&ids)
                ),
                format!("No runtime matches \"{query}\". Candidates: {}", list(&ids)),
            )
        }
    }
}
