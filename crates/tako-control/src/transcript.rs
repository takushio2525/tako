//! transcript — Claude Code の会話ログ（transcript JSONL）の読み取りと正規化
//!
//! `~/.claude/projects/<プロジェクトスラグ>/<session-id>.jsonl` を探し、
//! スマホリモート UI が描画しやすい正規化 JSON へ変換する（Issue #23）。
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
use std::path::PathBuf;

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

/// `~/.claude/projects/` 配下から session_id の transcript ファイルを探す
pub fn find_transcript(session_id: &str) -> Option<PathBuf> {
    if !is_valid_session_id(session_id) {
        return None;
    }
    let home = std::env::var("HOME").ok()?;
    let projects = PathBuf::from(home).join(".claude").join("projects");
    let entries = std::fs::read_dir(&projects).ok()?;
    for entry in entries.flatten() {
        let candidate = entry.path().join(format!("{session_id}.jsonl"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
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
    // 最後の tool_use に対する tool_result がまだ来ていないか
    let mut has_pending_tools = false;

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
                // tool_result を含む全 user 行で pending フラグをリセット
                has_pending_tools = false;
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
                if !tools.is_empty() {
                    has_pending_tools = true;
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
    // 最終 assistant エントリの tool_use に対する tool_result がまだ来ていない場合のみ
    // 承認待ちカードを表示する。auto mode で自動実行されたツールには付与しない（#425）
    if has_pending_tools {
        if let Some(last) = out.back_mut() {
            if last["role"] == "assistant" {
                if let Some(tools) = last["tools"].as_array() {
                    if let Some(last_tool) = tools.last() {
                        let tool_name = last_tool["name"].as_str().unwrap_or("");
                        let tool_summary = last_tool["summary"].as_str().unwrap_or("");
                        last["approval"] = json!({
                            "tool": tool_name,
                            "command": tool_summary,
                        });
                    }
                }
            }
        }
    }

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

    // --- #425: approval フィールドの条件付き付与 ---

    #[test]
    fn tool_use後にtool_resultが無ければapprovalが付く() {
        // ツール呼び出し直後（承認待ち）
        let raw = [
            r#"{"type":"user","message":{"content":"ファイル作って"}}"#,
            r#"{"type":"assistant","requestId":"r1","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"touch foo"}}]}}"#,
        ];
        let msgs = normalize_lines(lines(&raw), 10);
        assert_eq!(msgs.len(), 2);
        assert!(
            msgs[1]["approval"].is_object(),
            "tool_result 未到着なので approval が付く"
        );
        assert_eq!(msgs[1]["approval"]["tool"], "Bash");
    }

    #[test]
    fn tool_resultが来たらapprovalは付かない() {
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
        assert!(
            msgs[1]["approval"].is_null(),
            "tool_result 到着済みなので approval は付かない"
        );
    }

    #[test]
    fn tool_resultだけ来てテキスト応答前でもapprovalは付かない() {
        // tool_result は来たが応答テキストはまだ
        let raw = [
            r#"{"type":"user","message":{"content":"ls して"}}"#,
            r#"{"type":"assistant","requestId":"r1","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"file1\nfile2"}]}}"#,
        ];
        let msgs = normalize_lines(lines(&raw), 10);
        assert_eq!(msgs.len(), 2);
        assert!(
            msgs[1]["approval"].is_null(),
            "tool_result 到着でリセットされる"
        );
    }

    #[test]
    fn 連続ツール呼び出しで最後だけpendingならapproval付く() {
        // 1 つ目は完了、2 つ目は承認待ち
        let raw = [
            r#"{"type":"assistant","requestId":"r1","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"a.rs"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"..."}]}}"#,
            r#"{"type":"assistant","requestId":"r1","message":{"content":[{"type":"text","text":"読んだ"}]}}"#,
            r#"{"type":"assistant","requestId":"r2","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"a.rs"}}]}}"#,
        ];
        let msgs = normalize_lines(lines(&raw), 10);
        // r1 は統合、r2 は別エントリ
        assert_eq!(msgs.len(), 2);
        assert!(msgs[0]["approval"].is_null());
        assert!(
            msgs[1]["approval"].is_object(),
            "2 つ目は tool_result 未到着"
        );
        assert_eq!(msgs[1]["approval"]["tool"], "Edit");
    }
}
