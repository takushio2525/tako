//! transcript — Claude Code の会話ログ（transcript JSONL）の読み取りと正規化
//!
//! `<claude config dir>/projects/<プロジェクトスラグ>/<session-id>.jsonl` を探し、
//! スマホリモート UI が描画しやすい正規化 JSON へ変換する（Issue #23）。
//! config ディレクトリはアカウント（`CLAUDE_CONFIG_DIR`）ごとに分かれるため、
//! 既定の `~/.claude` だけでなく登録済みアカウントの分も走査する（Issue #652）。
//!
//! 正規化の方針:
//! - `type: "user"`（本文が文字列のもの）と `type: "assistant"` だけを拾う。
//!   tool_result だけの user 行・system / attachment / ai-title 等の補助行・
//!   サブエージェントの会話（isSidechain）はスキップする
//! - assistant の 1 応答は複数 JSONL 行に分かれる（thinking 行 / tool_use 行 /
//!   text 行）ため、同一 `requestId` の行を 1 エントリへ統合する
//! - thinking は折りたたみ表示用に `thinking` フィールドへ分離、ツール使用は
//!   `tools: [{name, summary}]` のサマリにする

use std::collections::VecDeque;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// ツールサマリ・テキスト切り詰めの最大文字数
const SUMMARY_MAX_CHARS: usize = 120;

/// session_id の形式検証（UUID 想定: 英数とハイフンのみ）。
/// パストラバーサル防止のため、これ以外の文字を含む ID は拒否する
pub fn is_valid_session_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && session_id.len() <= 64
        && session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// transcript の所在（Issue #652）。どの claude config ディレクトリに属するかまで返す。
/// resume はその config ディレクトリで実行しないと会話を見つけられない
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptLocation {
    /// `<config dir>/projects/<プロジェクトスラグ>/<session-id>.jsonl`
    pub path: PathBuf,
    /// この transcript が属する claude config ディレクトリ
    pub config_dir: PathBuf,
    /// `config_dir` が claude の既定（`~/.claude`）か
    pub is_default: bool,
}

/// パス比較用の正規化（`a/./b` や末尾スラッシュの表記ゆれを吸収する。
/// 存在しないパスも比較できるよう canonicalize には頼らない）
fn normalize(path: &Path) -> PathBuf {
    path.components().collect()
}

fn push_dir(dirs: &mut Vec<PathBuf>, path: PathBuf) {
    let normalized = normalize(&path);
    if !dirs.contains(&normalized) {
        dirs.push(normalized);
    }
}

/// transcript を探す claude config ディレクトリの一覧（先頭が既定 `~/.claude`）。
///
/// claude は会話を `<config dir>/projects/` 配下へ保存するため、`CLAUDE_CONFIG_DIR` を
/// 使うアカウント（#504 / #512）の会話は既定ディレクトリには存在しない。既定だけを見ると
/// 別アカウントのペインは「会話が無い」と判定され resume されない（Issue #652 の根因）。
///
/// 走査順: 既定 → tako 自身のプロセス env の `CLAUDE_CONFIG_DIR`（アカウント env つきの
/// ペインから GUI を起動すると紛れ込む。#571）→ accounts.yaml の `config_dir`。
/// 同じ場所を指すものは重複排除する
pub fn claude_config_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(default) = crate::orchestrator::claude_default_config_dir() {
        push_dir(&mut dirs, default);
    }
    if let Some(env) = std::env::var_os(crate::orchestrator::CLAUDE_CONFIG_DIR_ENV) {
        let value = env.to_string_lossy().to_string();
        if !value.trim().is_empty() {
            push_dir(
                &mut dirs,
                PathBuf::from(crate::orchestrator::expand_tilde(&value)),
            );
        }
    }
    if let Ok(accounts) = crate::orchestrator::AccountsConfig::load() {
        for (_, resolved) in accounts.list_resolved() {
            let Ok(account) = resolved else { continue };
            let Some(path) = account.config_dir.path() else {
                continue; // inherit = 既定。先頭で走査済み
            };
            push_dir(&mut dirs, PathBuf::from(path));
        }
    }
    dirs
}

/// 与えられた config ディレクトリ群から transcript の所在を特定する（走査対象を
/// 引数で受け取る純粋版。テストから HOME を触らずに検証できる）
pub fn locate_transcript_in(
    dirs: &[PathBuf],
    default_dir: Option<&Path>,
    session_id: &str,
) -> Option<TranscriptLocation> {
    if !is_valid_session_id(session_id) {
        return None;
    }
    let default_dir = default_dir.map(normalize);
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir.join("projects")) else {
            continue;
        };
        for entry in entries.flatten() {
            let candidate = entry.path().join(format!("{session_id}.jsonl"));
            if candidate.is_file() {
                return Some(TranscriptLocation {
                    path: candidate,
                    is_default: default_dir.as_deref() == Some(dir.as_path()),
                    config_dir: dir.clone(),
                });
            }
        }
    }
    None
}

/// 全 config ディレクトリ（`claude_config_dirs`）から session_id の transcript を探す
pub fn locate_transcript(session_id: &str) -> Option<TranscriptLocation> {
    let default_dir = crate::orchestrator::claude_default_config_dir();
    locate_transcript_in(&claude_config_dirs(), default_dir.as_deref(), session_id)
}

/// session_id の transcript ファイルを探す（所在の config ディレクトリは問わない）
pub fn find_transcript(session_id: &str) -> Option<PathBuf> {
    locate_transcript(session_id).map(|l| l.path)
}

/// resume 実行時にコマンドへ前置するシェル env プレフィクス（Issue #652）。
///
/// claude は `CLAUDE_CONFIG_DIR` で会話の保存先が変わるため、resume は**その会話が
/// 保存されている config ディレクトリ**で実行しないと
/// `No conversation found with session ID` で失敗する。tako 自身のプロセス env や
/// ログインシェルの rc / direnv が別の値を持っていても勝てるよう、
/// `export` / `unset` を明示する（#500 / #512 と同型）
pub fn resume_env_prefix_for(location: &TranscriptLocation) -> String {
    let key = crate::orchestrator::CLAUDE_CONFIG_DIR_ENV;
    if location.is_default {
        format!("unset {key}; ")
    } else {
        format!(
            "export {key}={}; ",
            crate::orchestrator::agent::sh_quote(&location.config_dir.display().to_string())
        )
    }
}

/// session_id の transcript を探し、resume に必要な env プレフィクスを返す。
/// 見つからない = その会話は resume できないので None
pub fn resume_env_prefix(session_id: &str) -> Option<String> {
    locate_transcript(session_id)
        .as_ref()
        .map(resume_env_prefix_for)
}

/// transcript の末尾 `tail` 件を正規化 JSON で返す。
/// 返り値: `{ "session_id": ..., "messages": [...] }`
pub fn read_messages(session_id: &str, tail: usize) -> Result<Value, String> {
    if !is_valid_session_id(session_id) {
        return Err("session_id の形式が不正（英数とハイフンのみ）".into());
    }
    let path = find_transcript(session_id)
        .ok_or_else(|| format!("session {session_id} の transcript が見つからない"))?;
    let file = std::fs::File::open(&path).map_err(|e| format!("transcript を開けない: {e}"))?;
    let reader = std::io::BufReader::new(file);
    let messages = normalize_lines(reader.lines().map_while(Result::ok), tail);
    Ok(json!({
        "session_id": session_id,
        "messages": messages,
    }))
}

/// 会話の最初のユーザー発話を返す（`max_chars` で切り詰め）。
/// セッションカタログ（Issue #112）の `show` 用。ファイルは先頭から
/// ストリーム読みして最初の該当行で打ち切るため、巨大 transcript でも軽い
pub fn first_user_text(session_id: &str, max_chars: usize) -> Option<String> {
    let path = find_transcript(session_id)?;
    let file = std::fs::File::open(&path).ok()?;
    let reader = std::io::BufReader::new(file);
    for line in reader.lines().map_while(Result::ok) {
        let Ok(obj) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if obj["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        if obj["type"].as_str() != Some("user") {
            continue;
        }
        let Some(text) = obj["message"]["content"].as_str() else {
            continue;
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }
        return Some(truncate_chars(trimmed, max_chars));
    }
    None
}

/// JSONL の行イテレータを正規化メッセージ列（末尾 tail 件）へ変換する。
/// メモリは tail 件分のみ保持する（大きな transcript でも安全）
pub fn normalize_lines(lines: impl Iterator<Item = String>, tail: usize) -> Vec<Value> {
    let tail = tail.max(1);
    let mut out: VecDeque<Value> = VecDeque::with_capacity(tail + 1);
    // 直前の assistant エントリの requestId（複数行にまたがる応答の統合用）
    let mut last_request_id: Option<String> = None;

    for line in lines {
        let Ok(obj) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        // サブエージェント（サイドチェーン）の会話は本会話に混ぜない
        if obj["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        match obj["type"].as_str() {
            Some("user") => {
                // 本文が文字列の行だけがユーザー発話。配列は tool_result（スキップ）
                let Some(text) = obj["message"]["content"].as_str() else {
                    continue;
                };
                if text.trim().is_empty() {
                    continue;
                }
                let mut entry = json!({
                    "role": "user",
                    "text": text,
                });
                if let Some(ts) = obj["timestamp"].as_str() {
                    entry["timestamp"] = json!(ts);
                }
                out.push_back(entry);
                last_request_id = None;
                if out.len() > tail {
                    out.pop_front();
                }
            }
            Some("assistant") => {
                let request_id = obj["requestId"].as_str().map(|s| s.to_string());
                let Some(blocks) = obj["message"]["content"].as_array() else {
                    continue;
                };
                let mut text = String::new();
                let mut thinking = String::new();
                let mut tools: Vec<Value> = Vec::new();
                for block in blocks {
                    match block["type"].as_str() {
                        Some("text") => {
                            if let Some(t) = block["text"].as_str() {
                                if !text.is_empty() {
                                    text.push('\n');
                                }
                                text.push_str(t);
                            }
                        }
                        Some("thinking") => {
                            if let Some(t) = block["thinking"].as_str() {
                                if !thinking.is_empty() {
                                    thinking.push('\n');
                                }
                                thinking.push_str(t);
                            }
                        }
                        Some("tool_use") => {
                            let name = block["name"].as_str().unwrap_or("unknown");
                            tools.push(json!({
                                "name": name,
                                "summary": tool_summary(&block["input"]),
                            }));
                        }
                        _ => {}
                    }
                }
                if text.is_empty() && thinking.is_empty() && tools.is_empty() {
                    continue;
                }
                // 同一 requestId の連続 assistant 行は 1 エントリへ統合
                let merged = request_id.is_some()
                    && request_id == last_request_id
                    && matches!(out.back(), Some(prev) if prev["role"] == "assistant");
                if merged {
                    let prev = out.back_mut().expect("直前エントリの存在は検査済み");
                    merge_assistant(prev, &text, &thinking, tools);
                } else {
                    let mut entry = json!({ "role": "assistant" });
                    if !text.is_empty() {
                        entry["text"] = json!(text);
                    }
                    if !thinking.is_empty() {
                        entry["thinking"] = json!(thinking);
                    }
                    if !tools.is_empty() {
                        entry["tools"] = json!(tools);
                    }
                    if let Some(ts) = obj["timestamp"].as_str() {
                        entry["timestamp"] = json!(ts);
                    }
                    out.push_back(entry);
                    last_request_id = request_id;
                    if out.len() > tail {
                        out.pop_front();
                    }
                }
            }
            _ => {}
        }
    }
    // 承認待ち（approval）は transcript からは判定しない（#425 再設計）。
    // transcript は「ツール実行中（auto mode で承認不要）」と「承認待ちで停止」を
    // 区別できない — どちらも「末尾 tool_use + tool_result 未着」で同一に見えるため、
    // 実行に時間がかかるツールの間じゅう誤った承認カードが出ていた。
    // 承認待ちの正は画面の permission ダイアログ実在（remote::attach_permission_dialogs）

    // テキスト内の選択肢パターンを検出（「1. xxx 2. yyy」形式）
    if let Some(last) = out.back_mut() {
        if last["role"] == "assistant" {
            if let Some(text) = last["text"].as_str() {
                let choices = extract_choices(text);
                if !choices.is_empty() {
                    last["choices"] = json!(choices);
                }
            }
        }
    }

    out.into_iter().collect()
}

/// テキストから選択肢パターンを抽出する。
/// 「1. xxx\n2. yyy」形式を検出する（番号が 1 始まりで連続すること）
fn extract_choices(text: &str) -> Vec<String> {
    let mut choices = Vec::new();
    let mut expected = 1u32;
    for line in text.lines() {
        let trimmed = line.trim();
        // "N. text" または "N) text" を試す
        let rest = try_parse_numbered_line(trimmed, expected);
        if let Some(label) = rest {
            choices.push(label.to_string());
            expected += 1;
        }
    }
    if choices.len() < 2 {
        return Vec::new();
    }
    choices
}

fn try_parse_numbered_line(line: &str, expected: u32) -> Option<&str> {
    let prefix = expected.to_string();
    let rest = line.strip_prefix(&prefix)?;
    let rest = rest
        .strip_prefix(". ")
        .or_else(|| rest.strip_prefix(") "))?;
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed)
}

/// 既存 assistant エントリへ後続行の内容を統合する
fn merge_assistant(prev: &mut Value, text: &str, thinking: &str, tools: Vec<Value>) {
    if !text.is_empty() {
        let joined = match prev["text"].as_str() {
            Some(t) if !t.is_empty() => format!("{t}\n{text}"),
            _ => text.to_string(),
        };
        prev["text"] = json!(joined);
    }
    if !thinking.is_empty() {
        let joined = match prev["thinking"].as_str() {
            Some(t) if !t.is_empty() => format!("{t}\n{thinking}"),
            _ => thinking.to_string(),
        };
        prev["thinking"] = json!(joined);
    }
    if !tools.is_empty() {
        let mut merged = prev["tools"].as_array().cloned().unwrap_or_default();
        merged.extend(tools);
        prev["tools"] = json!(merged);
    }
}

/// tool_use の input から 1 行サマリを作る。
/// 代表的なフィールド（command / file_path / description / prompt）を優先し、
/// 無ければ input 全体の JSON を切り詰める
fn tool_summary(input: &Value) -> String {
    for key in ["command", "file_path", "description", "prompt"] {
        if let Some(v) = input[key].as_str() {
            return truncate_chars(v, SUMMARY_MAX_CHARS);
        }
    }
    truncate_chars(&input.to_string(), SUMMARY_MAX_CHARS)
}

/// transcript から直近 `count` 件の assistant テキスト（text ブロックのみ）を抽出する。
/// tool_use / thinking は含めない。report コマンド用の軽量版
pub fn last_assistant_texts(session_id: &str, count: usize) -> Result<Vec<String>, String> {
    if !is_valid_session_id(session_id) {
        return Err("session_id の形式が不正（英数とハイフンのみ）".into());
    }
    let path = find_transcript(session_id)
        .ok_or_else(|| format!("session {session_id} の transcript が見つからない"))?;
    let file = std::fs::File::open(&path).map_err(|e| format!("transcript を開けない: {e}"))?;
    let reader = std::io::BufReader::new(file);
    Ok(extract_assistant_texts(
        reader.lines().map_while(Result::ok),
        count.max(1),
    ))
}

/// JSONL 行ストリームから assistant の text ブロックだけ抽出し、末尾 count 件を返す
fn extract_assistant_texts(lines: impl Iterator<Item = String>, count: usize) -> Vec<String> {
    let mut out: VecDeque<String> = VecDeque::with_capacity(count + 1);
    let mut last_request_id: Option<String> = None;

    for line in lines {
        let Ok(obj) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if obj["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        match obj["type"].as_str() {
            Some("user") => {
                last_request_id = None;
            }
            Some("assistant") => {
                let request_id = obj["requestId"].as_str().map(|s| s.to_string());
                let Some(blocks) = obj["message"]["content"].as_array() else {
                    continue;
                };
                let mut text = String::new();
                for block in blocks {
                    if block["type"].as_str() == Some("text") {
                        if let Some(t) = block["text"].as_str() {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(t);
                        }
                    }
                }
                if text.is_empty() {
                    continue;
                }
                let merged =
                    request_id.is_some() && request_id == last_request_id && !out.is_empty();
                if merged {
                    let prev = out.back_mut().unwrap();
                    prev.push('\n');
                    prev.push_str(&text);
                } else {
                    out.push_back(text);
                    last_request_id = request_id;
                    if out.len() > count {
                        out.pop_front();
                    }
                }
            }
            _ => {}
        }
    }
    out.into_iter().collect()
}

/// 文字数ベースの切り詰め（マルチバイト安全）。超過時は … を付ける
fn truncate_chars(s: &str, max: usize) -> String {
    let s = s.trim().replace('\n', " ");
    if s.chars().count() <= max {
        s
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(raw: &[&str]) -> std::vec::IntoIter<String> {
        raw.iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    /// `<config dir>/projects/<スラグ>/<id>.jsonl` を作る（Issue #652 のテスト用）
    fn seed_transcript(config_dir: &Path, slug: &str, session_id: &str) -> PathBuf {
        let dir = config_dir.join("projects").join(slug);
        std::fs::create_dir_all(&dir).expect("create_dir_all");
        let path = dir.join(format!("{session_id}.jsonl"));
        std::fs::write(&path, "{}\n").expect("write");
        path
    }

    /// テスト用一時ディレクトリの後始末。**一時ディレクトリ配下であることを検証してから**
    /// 消す（変数名の取り違えで実アカウントの config dir を消す事故を構造的に防ぐ）
    fn remove_temp_dir(dir: &Path) {
        assert!(
            dir.starts_with(std::env::temp_dir()),
            "一時ディレクトリ以外を削除しようとしている: {}",
            dir.display()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Issue #652: アカウント（`CLAUDE_CONFIG_DIR`）の会話は既定 `~/.claude` に無い。
    /// 走査は全 config ディレクトリを見て、所在（既定か否か）まで返す
    #[test]
    fn transcriptを全configディレクトリから探し所在を返す() {
        let root = std::env::temp_dir().join(format!("tako-652-locate-{}", std::process::id()));
        remove_temp_dir(&root);
        let default_dir = root.join(".claude");
        let univ_dir = root.join(".claude-univ");
        let default_id = "11111111-2222-3333-4444-555555555555";
        let univ_id = "e16cde37-c0e0-4126-9ef4-9c6b0bfeccc4";
        let default_path = seed_transcript(&default_dir, "-tmp-proj", default_id);
        let univ_path = seed_transcript(&univ_dir, "-tmp-proj", univ_id);
        let dirs = vec![default_dir.clone(), univ_dir.clone()];

        let found = locate_transcript_in(&dirs, Some(&default_dir), default_id).expect("既定側");
        assert_eq!(found.path, default_path);
        assert_eq!(found.config_dir, default_dir);
        assert!(found.is_default);

        // 既定だけを見ていた旧実装が resume を諦めていたケース
        let found = locate_transcript_in(&dirs, Some(&default_dir), univ_id).expect("univ 側");
        assert_eq!(found.path, univ_path);
        assert_eq!(found.config_dir, univ_dir);
        assert!(!found.is_default);
        // 既定だけを走査対象にすると見つからない（= 修正前の挙動）
        assert!(locate_transcript_in(
            std::slice::from_ref(&default_dir),
            Some(&default_dir),
            univ_id
        )
        .is_none());

        // 実在しない ID / 不正な ID は None（パストラバーサル防止）
        assert!(locate_transcript_in(
            &dirs,
            Some(&default_dir),
            "99999999-0000-0000-0000-000000000000"
        )
        .is_none());
        assert!(locate_transcript_in(&dirs, Some(&default_dir), "../../etc/passwd").is_none());

        remove_temp_dir(&root);
    }

    /// resume 時の env プレフィクス。既定は明示 unset（tako 側 env / direnv の
    /// 漏れに勝つ）、アカウントは export（#500 / #512 と同型）
    #[test]
    fn resume_env_prefixは所在に応じてexportとunsetを出し分ける() {
        let default_loc = TranscriptLocation {
            path: PathBuf::from("/home/me/.claude/projects/p/a.jsonl"),
            config_dir: PathBuf::from("/home/me/.claude"),
            is_default: true,
        };
        assert_eq!(
            resume_env_prefix_for(&default_loc),
            "unset CLAUDE_CONFIG_DIR; "
        );

        let univ_loc = TranscriptLocation {
            path: PathBuf::from("/home/me/.claude-univ/projects/p/a.jsonl"),
            config_dir: PathBuf::from("/home/me/.claude-univ"),
            is_default: false,
        };
        assert_eq!(
            resume_env_prefix_for(&univ_loc),
            "export CLAUDE_CONFIG_DIR=/home/me/.claude-univ; "
        );

        // 空白入りパスはクォートされる（シェルへ渡すため）
        let spaced = TranscriptLocation {
            path: PathBuf::from("/home/me/My Configs/.claude/projects/p/a.jsonl"),
            config_dir: PathBuf::from("/home/me/My Configs/.claude"),
            is_default: false,
        };
        assert_eq!(
            resume_env_prefix_for(&spaced),
            "export CLAUDE_CONFIG_DIR='/home/me/My Configs/.claude'; "
        );
    }

    #[test]
    fn session_idの形式検証() {
        assert!(is_valid_session_id("a45899a8-96a6-4fa6-9bf6-71df53307878"));
        assert!(is_valid_session_id("abc123"));
        assert!(!is_valid_session_id(""));
        assert!(!is_valid_session_id("../../etc/passwd"));
        assert!(!is_valid_session_id("id/with/slash"));
        assert!(!is_valid_session_id(&"x".repeat(65)));
    }

    #[test]
    fn userとassistantを正規化する() {
        let raw = [
            r#"{"type":"user","message":{"content":"こんにちは"},"timestamp":"2026-01-01T00:00:00Z"}"#,
            r#"{"type":"assistant","requestId":"r1","message":{"content":[{"type":"text","text":"やあ"}]},"timestamp":"2026-01-01T00:00:01Z"}"#,
        ];
        let msgs = normalize_lines(lines(&raw), 10);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["text"], "こんにちは");
        assert_eq!(msgs[0]["timestamp"], "2026-01-01T00:00:00Z");
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[1]["text"], "やあ");
    }

    #[test]
    fn 補助行とtool_resultをスキップする() {
        let raw = [
            r#"{"type":"ai-title","title":"x"}"#,
            r#"{"type":"system","message":{"content":"sys"}}"#,
            // tool_result（content が配列）の user 行はスキップ
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#,
            r#"{"type":"user","message":{"content":"実発話"}}"#,
            "not-json",
        ];
        let msgs = normalize_lines(lines(&raw), 10);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["text"], "実発話");
    }

    #[test]
    fn sidechainをスキップする() {
        let raw = [
            r#"{"type":"user","isSidechain":true,"message":{"content":"サブエージェントへの指示"}}"#,
            r#"{"type":"user","isSidechain":false,"message":{"content":"本会話"}}"#,
        ];
        let msgs = normalize_lines(lines(&raw), 10);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["text"], "本会話");
    }

    #[test]
    fn 同一request_idのassistant行を統合する() {
        let raw = [
            r#"{"type":"assistant","requestId":"r1","message":{"content":[{"type":"thinking","thinking":"考える"}]}}"#,
            r#"{"type":"assistant","requestId":"r1","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"ls -la"}}]}}"#,
            r#"{"type":"assistant","requestId":"r1","message":{"content":[{"type":"text","text":"できた"}]}}"#,
            r#"{"type":"assistant","requestId":"r2","message":{"content":[{"type":"text","text":"別応答"}]}}"#,
        ];
        let msgs = normalize_lines(lines(&raw), 10);
        assert_eq!(msgs.len(), 2, "r1 の 3 行は 1 エントリへ統合: {msgs:?}");
        assert_eq!(msgs[0]["thinking"], "考える");
        assert_eq!(msgs[0]["text"], "できた");
        assert_eq!(msgs[0]["tools"][0]["name"], "Bash");
        assert_eq!(msgs[0]["tools"][0]["summary"], "ls -la");
        assert_eq!(msgs[1]["text"], "別応答");
    }

    #[test]
    fn userを挟むと統合しない() {
        let raw = [
            r#"{"type":"assistant","requestId":"r1","message":{"content":[{"type":"text","text":"一"}]}}"#,
            r#"{"type":"user","message":{"content":"割込"}}"#,
            r#"{"type":"assistant","requestId":"r1","message":{"content":[{"type":"text","text":"二"}]}}"#,
        ];
        let msgs = normalize_lines(lines(&raw), 10);
        assert_eq!(msgs.len(), 3);
    }

    #[test]
    fn tailで末尾だけ残す() {
        let raw: Vec<String> = (0..10)
            .map(|i| format!(r#"{{"type":"user","message":{{"content":"msg{i}"}}}}"#))
            .collect();
        let msgs = normalize_lines(raw.into_iter(), 3);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0]["text"], "msg7");
        assert_eq!(msgs[2]["text"], "msg9");
    }

    #[test]
    fn tool_summaryは代表フィールドを優先する() {
        assert_eq!(
            tool_summary(&serde_json::json!({"command": "cargo build"})),
            "cargo build"
        );
        assert_eq!(
            tool_summary(&serde_json::json!({"file_path": "/tmp/a.rs", "other": 1})),
            "/tmp/a.rs"
        );
        // 代表フィールドが無ければ JSON ダンプの切り詰め
        let s = tool_summary(&serde_json::json!({"x": "y"}));
        assert!(s.contains("\"x\""));
    }

    #[test]
    fn truncate_charsはマルチバイト安全() {
        assert_eq!(truncate_chars("短い", 10), "短い");
        let long = "あ".repeat(130);
        let out = truncate_chars(&long, 120);
        assert_eq!(out.chars().count(), 121); // 120 + …
        assert!(out.ends_with('…'));
        // 改行は空白へ
        assert_eq!(truncate_chars("a\nb", 10), "a b");
    }

    #[test]
    fn read_messagesは実ファイルを読める() {
        // HOME を一時ディレクトリに差し替えて ~/.claude/projects/ 構造を作る
        let tmp = std::env::temp_dir().join(format!("tako-transcript-test-{}", std::process::id()));
        let proj = tmp.join(".claude").join("projects").join("-tmp-proj");
        std::fs::create_dir_all(&proj).unwrap();
        let sid = "11111111-2222-3333-4444-555555555555";
        std::fs::write(
            proj.join(format!("{sid}.jsonl")),
            concat!(
                r#"{"type":"user","message":{"content":"やあ"}}"#,
                "\n",
                r#"{"type":"assistant","requestId":"r1","message":{"content":[{"type":"text","text":"はい"}]}}"#,
                "\n",
            ),
        )
        .unwrap();

        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &tmp);
        let result = read_messages(sid, 10);
        let missing = read_messages("99999999-9999-9999-9999-999999999999", 10);
        if let Some(h) = original_home {
            std::env::set_var("HOME", h);
        }
        let _ = std::fs::remove_dir_all(&tmp);

        let value = result.expect("読み取り成功");
        assert_eq!(value["session_id"], sid);
        assert_eq!(value["messages"].as_array().unwrap().len(), 2);
        assert!(missing.is_err());
    }

    #[test]
    fn extract_choicesは番号付きリストを抽出する() {
        let text = "設定を変更しますか？\n1. 変更する\n2. 詳細を見る\n3. キャンセル";
        let choices = extract_choices(text);
        assert_eq!(choices, vec!["変更する", "詳細を見る", "キャンセル"]);
    }

    #[test]
    fn extract_choicesは括弧形式も扱う() {
        let text = "選んでください:\n1) はい\n2) いいえ";
        let choices = extract_choices(text);
        assert_eq!(choices, vec!["はい", "いいえ"]);
    }

    #[test]
    fn extract_choicesは1項目だけなら空を返す() {
        let text = "1. これだけ";
        let choices = extract_choices(text);
        assert!(choices.is_empty());
    }

    #[test]
    fn extract_choicesは番号が飛んでいたら途中で止まる() {
        let text = "1. A\n3. C";
        let choices = extract_choices(text);
        assert!(choices.is_empty());
    }

    #[test]
    fn try_parse_numbered_lineのテスト() {
        assert_eq!(try_parse_numbered_line("1. hello", 1), Some("hello"));
        assert_eq!(try_parse_numbered_line("2) world", 2), Some("world"));
        assert_eq!(try_parse_numbered_line("1. hello", 2), None);
        assert_eq!(try_parse_numbered_line("not a number", 1), None);
    }

    #[test]
    fn extract_assistant_textsはテキストだけ抽出する() {
        let raw = [
            r#"{"type":"user","message":{"content":"やって"}}"#,
            r#"{"type":"assistant","requestId":"r1","message":{"content":[{"type":"thinking","thinking":"考え中"},{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#,
            r#"{"type":"assistant","requestId":"r1","message":{"content":[{"type":"text","text":"完了報告"}]}}"#,
            r#"{"type":"assistant","requestId":"r2","message":{"content":[{"type":"text","text":"補足"}]}}"#,
        ];
        let texts = extract_assistant_texts(lines(&raw), 10);
        assert_eq!(texts.len(), 2);
        assert_eq!(texts[0], "完了報告");
        assert_eq!(texts[1], "補足");
    }

    #[test]
    fn extract_assistant_textsはtail制限が効く() {
        let raw: Vec<String> = (0..5)
            .map(|i| {
                format!(
                    r#"{{"type":"assistant","requestId":"r{i}","message":{{"content":[{{"type":"text","text":"msg{i}"}}]}}}}"#
                )
            })
            .collect();
        let texts = extract_assistant_texts(raw.into_iter(), 2);
        assert_eq!(texts.len(), 2);
        assert_eq!(texts[0], "msg3");
        assert_eq!(texts[1], "msg4");
    }

    #[test]
    fn extract_assistant_textsは同一request_idを統合する() {
        let raw = [
            r#"{"type":"assistant","requestId":"r1","message":{"content":[{"type":"text","text":"前半"}]}}"#,
            r#"{"type":"assistant","requestId":"r1","message":{"content":[{"type":"text","text":"後半"}]}}"#,
        ];
        let texts = extract_assistant_texts(lines(&raw), 10);
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0], "前半\n後半");
    }

    #[test]
    fn extract_assistant_textsはsidechainを除外する() {
        let raw = [
            r#"{"type":"assistant","isSidechain":true,"requestId":"r1","message":{"content":[{"type":"text","text":"サブ"}]}}"#,
            r#"{"type":"assistant","requestId":"r2","message":{"content":[{"type":"text","text":"本"}]}}"#,
        ];
        let texts = extract_assistant_texts(lines(&raw), 10);
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0], "本");
    }

    // --- #425 再設計: transcript からは approval を一切付与しない ---
    // 「末尾 tool_use + tool_result 未着」は auto mode のツール実行中と承認待ち停止の
    // 両方で発生し区別不能。承認待ちは画面ダイアログ検知（remote 側）が正

    #[test]
    fn tool_use直後でもapprovalは付かない() {
        // 旧実装は tool_result 未着で approval を付けていた（実行中に誤爆する根因）
        let raw = [
            r#"{"type":"user","message":{"content":"ファイル作って"}}"#,
            r#"{"type":"assistant","requestId":"r1","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"touch foo"}}]}}"#,
        ];
        let msgs = normalize_lines(lines(&raw), 10);
        assert_eq!(msgs.len(), 2);
        assert!(
            msgs[1]["approval"].is_null(),
            "実行中と区別できないため transcript では付与しない: {msgs:?}"
        );
        // tool 自体の表示情報は維持される
        assert_eq!(msgs[1]["tools"][0]["name"], "Bash");
    }

    #[test]
    fn tool_result完了後もapprovalは付かない() {
        // auto mode: tool_use → tool_result → 応答テキスト
        let raw = [
            r#"{"type":"user","message":{"content":"ファイル作って"}}"#,
            r#"{"type":"assistant","requestId":"r1","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"touch foo"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#,
            r#"{"type":"assistant","requestId":"r1","message":{"content":[{"type":"text","text":"作りました"}]}}"#,
        ];
        let msgs = normalize_lines(lines(&raw), 10);
        // tool_use と text は同一 requestId なので 1 エントリに統合
        assert_eq!(msgs.len(), 2);
        assert!(msgs[1]["approval"].is_null());
    }

    #[test]
    fn 連続ツール呼び出しでもapprovalは付かない() {
        let raw = [
            r#"{"type":"assistant","requestId":"r1","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"a.rs"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"..."}]}}"#,
            r#"{"type":"assistant","requestId":"r1","message":{"content":[{"type":"text","text":"読んだ"}]}}"#,
            r#"{"type":"assistant","requestId":"r2","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"a.rs"}}]}}"#,
        ];
        let msgs = normalize_lines(lines(&raw), 10);
        assert_eq!(msgs.len(), 2);
        assert!(msgs[0]["approval"].is_null());
        assert!(msgs[1]["approval"].is_null());
        // tool 表示情報は維持
        assert_eq!(msgs[1]["tools"][0]["name"], "Edit");
    }
}
