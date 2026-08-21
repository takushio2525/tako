//! transcript — Claude Code の会話ログ（transcript JSONL）の読み取りと正規化
//!
//! `<claude config dir>/projects/<プロジェクトスラグ>/<session-id>.jsonl` を探し、
//! スマホリモート UI が描画しやすい正規化 JSON へ変換する（Issue #23）。
//! config ディレクトリはアカウント（`CLAUDE_CONFIG_DIR`）ごとに分かれるため、
//! 既定の `~/.claude` だけでなく登録済みアカウントの分も走査する（Issue #652）。
//!
//! 正規化の方針:
//! - `type: "user"` と `type: "assistant"` だけを拾う。
//!   tool_result だけの user 行・system / attachment / ai-title 等の補助行・
//!   サブエージェントの会話（isSidechain）はスキップする
//! - assistant の 1 応答は複数 JSONL 行に分かれる（thinking 行 / tool_use 行 /
//!   text 行）ため、同一 `requestId` の行を 1 エントリへ統合する
//! - thinking は折りたたみ表示用に `thinking` フィールドへ分離、ツール使用は
//!   `tools: [{name, summary}]` のサマリにする
//! - **user 行の中身は「本物の発話」と「システムが差し込んだ内容」に分類する**
//!   （Issue #715。[`classify_user_content`]）。claude は画像添付のメタテキスト・
//!   `<task-notification>` 等の XML を user 発話と同じ形で transcript に書くため、
//!   素通しするとチャット UI に生 XML が並ぶ

use std::collections::VecDeque;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// ツールサマリ・テキスト切り詰めの最大文字数
const SUMMARY_MAX_CHARS: usize = 120;

/// システム通知の要約に使う最大文字数（薄い 1 行に収まる範囲）
const NOTICE_SUMMARY_MAX_CHARS: usize = 100;

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
    resume_env_prefix_for_in(location, crate::launch_cmd::launch_dialect())
}

/// 方言を明示して組み立てる（#867。macOS 上から PowerShell 側の出力を検証するため）
pub fn resume_env_prefix_for_in(
    location: &TranscriptLocation,
    dialect: crate::launch_cmd::ShellDialect,
) -> String {
    let key = crate::orchestrator::CLAUDE_CONFIG_DIR_ENV;
    if location.is_default {
        crate::launch_cmd::unset_prefix(dialect, key)
    } else {
        crate::launch_cmd::export_prefix(dialect, key, &location.config_dir.display().to_string())
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

/// **所在が分かっている** transcript の末尾 `tail` 件を正規化して返す（#702）。
///
/// [`read_messages`] は毎回 config ディレクトリを走査して所在を探すが、GUI モードの
/// チャットビューは 2 秒ごとに同じファイルを見るので、解決済みのパスを使い回して
/// `read_dir` を省く（同時に「更新の有無」を mtime で判断する呼び出し側の前提とも合う）
pub fn read_messages_at(path: &Path, tail: usize) -> Result<Vec<Value>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("transcript を開けない: {e}"))?;
    let reader = std::io::BufReader::new(file);
    Ok(normalize_lines(reader.lines().map_while(Result::ok), tail))
}

/// システム通知エントリ（`role: "system"`。#715）を落とした複製を返す。
///
/// リモート PWA のチャットは role が `user` 以外を全てエージェント発話として描くので、
/// 通知を渡すと「AI が言った」ように見える。PWA 側の描画には手を入れない方針
/// （#716 のスコープ外）なので、配る手前で落とす。生 XML が消える #715 の効果は
/// 正規化層の分類だけで得られており、通知の可視化は GUI モード側の役割
pub fn without_system_notices(mut value: Value) -> Value {
    if let Some(messages) = value["messages"].as_array_mut() {
        messages.retain(|m| m["role"] != "system");
    }
    value
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
        // #715: 画像メタ・システム XML を会話の見出しにしない（分類は正規化と共通）
        let UserContent::Speech { text, .. } = classify_user_content(&obj["message"]["content"])
        else {
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

// ─────────────── user 行の内容分類（Issue #715） ───────────────

/// claude が user 行として書くが**会話ではない**内容を囲む XML タグ。
///
/// 実 transcript 3409 本の user 本文を全数走査して確定した一覧
/// （`<task-notification>` = Monitor 等の自動通知 2068 件、`<local-command-caveat>` =
/// スラッシュコマンドの前置定型文 84 件、`<local-command-*>` / `<bash-std*>` =
/// コマンドの実行結果、`<system-reminder>` = ツール結果に添えられる注意書き）
const SYSTEM_BLOCK_TAGS: &[&str] = &[
    "task-notification",
    "system-reminder",
    "local-command-caveat",
    "local-command-stdout",
    "local-command-stderr",
    "bash-stdout",
    "bash-stderr",
];

/// スラッシュコマンド実行時に前置される定型文（タグに包まれていない古い形式のため、
/// 本文一致でも落とす）
const LOCAL_COMMAND_CAVEAT_HEAD: &str =
    "Caveat: The messages below were generated by the user while running local commands.";

/// user 行の中身の分類結果（[`classify_user_content`]）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserContent {
    /// 本物のユーザー発話。`images` は添付画像の枚数（本文が空でも枚数があれば表示する）
    Speech { text: String, images: usize },
    /// システムが差し込んだ通知。`summary` は**生 XML を含まない** 1 行の要約で、
    /// 必ず非空（伝える中身が無い通知は [`UserContent::Skip`] になる）。
    /// 「システム通知」等のラベルは表示側が i18n で付ける
    Notice { summary: String },
    /// 表示するものが無い（tool_result だけの行・空行）
    Skip,
}

/// user 行の `message.content` を分類する（Issue #715 の中核。純関数）。
///
/// content は claude の版によって**文字列と配列の両方**があり、配列には
/// `text` / `image` / `tool_result` ブロックが混ざる。旧実装は文字列だけを拾っていたため
/// ①配列形式の発話（画像を添えた質問など）が丸ごと消える ②文字列形式の
/// 画像メタ・システム XML が生のまま user 発話として表示される、の 2 つが同時に起きていた。
pub fn classify_user_content(content: &Value) -> UserContent {
    match content {
        Value::String(text) => classify_user_text(text, 0),
        Value::Array(blocks) => {
            // tool_result はツールの出力（会話ではない）。従来どおり行ごと落とす
            if blocks
                .iter()
                .any(|b| b["type"].as_str() == Some("tool_result"))
            {
                return UserContent::Skip;
            }
            let mut text = String::new();
            let mut images = 0usize;
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
                    Some("image") => images += 1,
                    _ => {}
                }
            }
            classify_user_text(&text, images)
        }
        _ => UserContent::Skip,
    }
}

/// テキスト本文の分類。`block_images` は content 配列から数えた実画像ブロック数
fn classify_user_text(text: &str, block_images: usize) -> UserContent {
    // ① システム XML ブロックを**どこにあっても**取り除く。
    //    `<system-reminder>` は本物の発話の後ろに付くことがあるので、
    //    「先頭がタグなら通知」ではなく「取り除いた残りが空なら通知」で判定する
    let mut notices: Vec<(&str, String)> = Vec::new();
    let mut rest = text.to_string();
    for tag in SYSTEM_BLOCK_TAGS {
        rest = strip_tagged_blocks(&rest, tag, &mut notices);
    }
    // ② スラッシュコマンドの前置定型文
    let had_caveat = rest.contains(LOCAL_COMMAND_CAVEAT_HEAD);
    if had_caveat {
        rest = strip_caveat(&rest);
    }
    // ③ 画像添付のメタテキスト（`[Image: original WxH, ...]` / `[Image #1]`）
    let (rest, meta_images) = strip_image_markers(&rest);
    let images = block_images.max(meta_images);
    // ④ スラッシュコマンド / bash モードの行は「ユーザーが打ったコマンド」として見せる
    if let Some(command) = user_command_label(&rest) {
        return UserContent::Speech {
            text: command,
            images,
        };
    }
    let rest = rest.trim();
    if !rest.is_empty() {
        return UserContent::Speech {
            text: rest.to_string(),
            images,
        };
    }
    if images > 0 {
        // 画像だけの発話（貼り付けただけ）。プレースホルダを出すため Speech で返す
        return UserContent::Speech {
            text: String::new(),
            images,
        };
    }
    // 伝える中身のある通知だけを残す（定型文だけの行は薄い 1 行すら出さない）
    for (_, inner) in notices {
        let summary = notice_summary(&inner);
        if !summary.is_empty() {
            return UserContent::Notice { summary };
        }
    }
    UserContent::Skip
}

/// `<tag>…</tag>` を全て取り除き、中身を `found` へ push する。
///
/// 閉じタグが無い（切り詰められた行など）ときは**開きタグ以降を全部捨てる**。
/// 「残せるところまで残す」より、生 XML の断片を絶対に表示させないことを優先する
fn strip_tagged_blocks<'a>(text: &str, tag: &'a str, found: &mut Vec<(&'a str, String)>) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(&open) {
        out.push_str(&rest[..start]);
        let after_open = &rest[start + open.len()..];
        match after_open.find(&close) {
            Some(end) => {
                found.push((tag, after_open[..end].to_string()));
                rest = &after_open[end + close.len()..];
            }
            None => {
                found.push((tag, after_open.to_string()));
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// スラッシュコマンドの前置定型文（`Caveat: …`）をその段落ごと取り除く
fn strip_caveat(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim_start().starts_with("Caveat:"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 画像添付のメタテキストを取り除き、(残り, 見つけた枚数) を返す。
///
/// claude は画像を貼ると `[Image: original 3024x1964, displayed at ...]` という
/// **座標変換の説明文**を user 発話として書く（実 transcript で 111 件観測）。
/// 本文中の参照 `[Image #1]` も表示上は不要なので落とす
fn strip_image_markers(text: &str) -> (String, usize) {
    let mut images = 0usize;
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let mut remaining = line;
        let mut kept = String::new();
        // 1 行に複数のマーカーが並ぶことがある（`[Image #1] [Image #2] 比べて`）
        while let Some(start) = remaining.find("[Image") {
            let after = &remaining[start..];
            match after.find(']') {
                Some(end) => {
                    kept.push_str(&remaining[..start]);
                    images += 1;
                    remaining = &after[end + 1..];
                }
                // 閉じ括弧が無い = マーカーではない普通の本文
                None => break,
            }
        }
        kept.push_str(remaining);
        let trimmed = kept.trim();
        // マーカーだけの行は行ごと落とす（空行が残ると吹き出しが間延びする）
        if !trimmed.is_empty() {
            out.push(kept);
        }
    }
    (out.join("\n"), images)
}

/// ユーザーが打ったコマンドの行を 1 行の発話へ直す。該当しなければ None。
///
/// - `<command-name>/model</command-name>` + `<command-args>…</command-args>`
///   = スラッシュコマンド → `/model …`
/// - `<bash-input>cmd</bash-input>` = claude の bash モード（`!` 始まり）→ `! cmd`
///
/// どちらも**ユーザーの操作そのもの**なので、通知として隠すのではなく発話として見せる
fn user_command_label(text: &str) -> Option<String> {
    if let Some(name) = inner_text(text, "command-name") {
        let name = name.trim();
        if !name.is_empty() {
            let args = inner_text(text, "command-args")
                .map(|a| a.trim().to_string())
                .unwrap_or_default();
            return Some(if args.is_empty() {
                name.to_string()
            } else {
                format!("{name} {args}")
            });
        }
    }
    let bash = inner_text(text, "bash-input")?;
    let bash = bash.trim();
    (!bash.is_empty()).then(|| format!("! {bash}"))
}

/// `<tag>…</tag>` の中身（最初の 1 個。閉じタグが無ければ None）
fn inner_text(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    Some(text[start..end].to_string())
}

/// `<tag>` 以降の中身（閉じタグが無ければ末尾まで）。
/// 切り詰められた通知からも要約を拾うための緩い版。**要約用にだけ使う**
/// （スラッシュコマンド名のような「意味が変わる」抽出には使わない）
fn inner_text_lossy(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..]
        .find(&close)
        .map(|e| e + start)
        .unwrap_or(text.len());
    Some(text[start..end].to_string())
}

/// システム通知の 1 行要約。`<summary>` があればそれを使い、無ければ
/// 中身の最初の意味のある行を取る。**残った山括弧は落とす**（生 XML を出さない）
fn notice_summary(inner: &str) -> String {
    // 定型文（`Caveat: …`）は「中身」に数えない。これだけの通知は何も伝えないので
    // 呼び出し側で Skip になる
    let inner = strip_caveat(inner);
    let candidate = inner_text_lossy(&inner, "summary")
        .map(|s| strip_all_tags(&s).trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            strip_all_tags(&inner)
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_default();
    truncate_chars(&candidate, NOTICE_SUMMARY_MAX_CHARS)
}

/// 残存する `<…>` を落とす（要約に生 XML を混ぜないための最後の関所）
fn strip_all_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
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
            // #737: 生成中に打たれた指示は claude のキューへ入り、その時点で
            // `queue-operation` 行として記録される。**本物の user 行になるのは
            // 配送された後**（長いターンでは数分後）で、ターン内へ差し込まれた場合は
            // 一生 user 行にならない。ここを読まないと「busy 中に送った発話が
            // 吹き出しとして出ない」（実測で確定した #737 追加要件 5 の根因）
            Some("queue-operation") => {
                match obj["operation"].as_str() {
                    // content を持つのは enqueue だけ（実 transcript 3416 本で
                    // enqueue 6555 件が全件 content つき）
                    Some("enqueue") => {
                        // 中身は user 行と同じ分類にかける。`<task-notification>` 等の
                        // システム注入がキューに入ることもあるため（実測 1760 件）、
                        // 通し方を user 行と揃えないと生 XML が吹き出しになる。
                        // 通知は本物の user 行として後から必ず来るのでここでは出さない
                        let UserContent::Speech { text, images } =
                            classify_user_content(&obj["content"])
                        else {
                            continue;
                        };
                        // `queued` = 表示用の「送信待ち」印（キューから出たら消える）。
                        // `from_queue` = 本物の user 行と突き合わせるための印
                        // （配送のされ方が 2 通りあるので、表示状態とは別に持つ）
                        let mut entry = json!({
                            "role": "user", "text": text,
                            "queued": true, "from_queue": true,
                        });
                        if images > 0 {
                            entry["attachments"] = json!(vec![json!({ "kind": "image" }); images]);
                        }
                        if let Some(ts) = obj["timestamp"].as_str() {
                            entry["timestamp"] = json!(ts);
                        }
                        out.push_back(entry);
                        last_request_id = None;
                        if out.len() > tail {
                            out.pop_front();
                        }
                    }
                    // キューから出た = 送信された。どのメッセージかは content が
                    // 無い（dequeue）ので特定できないため、FIFO で最も古い
                    // 「送信待ち」の印を落とす
                    Some("dequeue") | Some("remove") | Some("popAll") => {
                        if let Some(entry) = out.iter_mut().find(|e| e["queued"] == json!(true)) {
                            entry["queued"] = json!(false);
                        }
                    }
                    _ => {}
                }
            }
            Some("user") => {
                // #715: 本物の発話 / システム注入 / 表示なし を分類する
                let mut entry = match classify_user_content(&obj["message"]["content"]) {
                    UserContent::Skip => continue,
                    UserContent::Speech { text, images } => {
                        // #737: キュー経由で既に出してある発話が配送されてきた。
                        // 同じ本文を 2 回並べない（1 対 1 で消費するので、同じ文面を
                        // 2 回送った場合はきちんと 2 個出る）。**まだ手元に残っている
                        // ものだけ**を対象にするので、tail から押し出された後に
                        // 本物が来ても取り違えて消すことはない
                        if let Some(queued) = out
                            .iter_mut()
                            .find(|e| e["from_queue"] == json!(true) && e["text"] == json!(&text))
                        {
                            queued["from_queue"] = json!(false);
                            queued["queued"] = json!(false);
                            if let Some(ts) = obj["timestamp"].as_str() {
                                queued["timestamp"] = json!(ts);
                            }
                            last_request_id = None;
                            continue;
                        }
                        let mut entry = json!({ "role": "user", "text": text });
                        if images > 0 {
                            entry["attachments"] = json!(vec![json!({ "kind": "image" }); images]);
                        }
                        entry
                    }
                    UserContent::Notice { summary } => {
                        // 連続するシステム通知は 1 エントリへまとめる（Monitor の通知が
                        // 何十件も続くと、tail の枠を食い潰して会話が見えなくなる）
                        if let Some(prev) = out.back_mut() {
                            if prev["kind"] == "notice" {
                                let count = prev["count"].as_u64().unwrap_or(1) + 1;
                                prev["count"] = json!(count);
                                if !summary.is_empty() {
                                    prev["text"] = json!(summary);
                                }
                                last_request_id = None;
                                continue;
                            }
                        }
                        json!({ "role": "system", "kind": "notice", "text": summary, "count": 1 })
                    }
                };
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
    /// POSIX 形式を固定するスナップショット群なので構文を明示する（#867。
    /// 既定版は動いているシェルを見るので、Windows では PowerShell を返す）
    #[allow(dead_code)]
    const POSIX: crate::launch_cmd::ShellDialect = crate::launch_cmd::ShellDialect::Posix;

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
            resume_env_prefix_for_in(&default_loc, POSIX),
            "unset CLAUDE_CONFIG_DIR; "
        );

        let univ_loc = TranscriptLocation {
            path: PathBuf::from("/home/me/.claude-univ/projects/p/a.jsonl"),
            config_dir: PathBuf::from("/home/me/.claude-univ"),
            is_default: false,
        };
        assert_eq!(
            resume_env_prefix_for_in(&univ_loc, POSIX),
            "export CLAUDE_CONFIG_DIR=/home/me/.claude-univ; "
        );

        // 空白入りパスはクォートされる（シェルへ渡すため）
        let spaced = TranscriptLocation {
            path: PathBuf::from("/home/me/My Configs/.claude/projects/p/a.jsonl"),
            config_dir: PathBuf::from("/home/me/My Configs/.claude"),
            is_default: false,
        };
        assert_eq!(
            resume_env_prefix_for_in(&spaced, POSIX),
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

    // --- #715: user 行の内容分類（実 master セッションから採取したパターン） ---

    /// 画像添付のメタテキストは発話ではない（実測 111 件）。
    /// 「画像」プレースホルダの材料（枚数）だけ残し、座標変換の説明文は捨てる
    #[test]
    fn 画像メタテキストは発話にせず枚数だけ残す() {
        let meta = json!(
            "[Image: original 3024x1964, displayed at 2000x1299. Multiply coordinates by 1.51 to map to original image.]"
        );
        assert_eq!(
            classify_user_content(&meta),
            UserContent::Speech {
                text: String::new(),
                images: 1
            }
        );
        // 本文つき（`[Image #1] この子はなに？`）はマーカーだけ落として本文を残す
        let with_text = json!([
            { "type": "text", "text": "[Image #1] この子はなに？" },
            { "type": "image", "source": { "type": "base64", "data": "iVBO" } },
        ]);
        assert_eq!(
            classify_user_content(&with_text),
            UserContent::Speech {
                text: "この子はなに？".into(),
                images: 1
            }
        );
        // 画像だけ貼った発話（text ブロックなし）は枚数を数える
        let only_images = json!([
            { "type": "image", "source": {} },
            { "type": "image", "source": {} },
        ]);
        assert_eq!(
            classify_user_content(&only_images),
            UserContent::Speech {
                text: String::new(),
                images: 2
            }
        );
    }

    /// `<task-notification>` は生 XML を一切出さず、`<summary>` を 1 行要約にする
    #[test]
    fn task_notificationはシステム通知へ分類する() {
        let raw = json!(concat!(
            "<task-notification>\n<task-id>btexwkxuv</task-id>\n",
            "<summary>Monitor event: \"worker の完了監視\"</summary>\n",
            "<event>[Monitor timed out — re-arm if needed.]</event>\n</task-notification>"
        ));
        let UserContent::Notice { summary } = classify_user_content(&raw) else {
            panic!(
                "システム通知として分類されるべき: {:?}",
                classify_user_content(&raw)
            );
        };
        assert_eq!(summary, "Monitor event: \"worker の完了監視\"");
        assert!(!summary.contains('<'), "生 XML を要約に混ぜない: {summary}");
    }

    /// `<system-reminder>` は**本物の発話の後ろに付く**ことがある。
    /// そのときは通知にせず、注意書きだけ削って発話を残す
    #[test]
    fn system_reminderは削って発話を残す() {
        let raw = json!("これを直して\n<system-reminder>Prefer batch tools.</system-reminder>");
        assert_eq!(
            classify_user_content(&raw),
            UserContent::Speech {
                text: "これを直して".into(),
                images: 0
            }
        );
        // 単独で来たら通知
        let alone = json!("<system-reminder>Prefer batch tools.</system-reminder>");
        assert_eq!(
            classify_user_content(&alone),
            UserContent::Notice {
                summary: "Prefer batch tools.".into()
            }
        );
    }

    /// 閉じタグが無い（切り詰め等）ときも生 XML の断片を漏らさない
    #[test]
    fn 閉じタグが無いシステムブロックも漏らさない() {
        let raw = json!("<task-notification>\n<task-id>abc</task-id>\n<summary>途中で切れた");
        let result = classify_user_content(&raw);
        let UserContent::Notice { summary } = &result else {
            panic!("通知として分類されるべき: {result:?}");
        };
        assert!(!summary.contains('<'), "断片が残っている: {summary}");
        assert_eq!(summary, "途中で切れた");
    }

    /// スラッシュコマンド実行行は「ユーザーが打ったコマンド」として見せる
    #[test]
    fn スラッシュコマンド行はコマンド名の発話にする() {
        let raw = json!(concat!(
            "<command-name>/model</command-name>\n",
            "            <command-message>model</command-message>\n",
            "            <command-args></command-args>"
        ));
        assert_eq!(
            classify_user_content(&raw),
            UserContent::Speech {
                text: "/model".into(),
                images: 0
            }
        );
        // 引数つき
        let with_args = json!(
            "<command-name>/compact</command-name><command-args>focus on tests</command-args>"
        );
        assert_eq!(
            classify_user_content(&with_args),
            UserContent::Speech {
                text: "/compact focus on tests".into(),
                images: 0
            }
        );
    }

    /// claude の bash モード（`!` 始まり）はユーザーの操作なので発話として見せ、
    /// その出力は通知にする（実 transcript の `<bash-input>` / `<bash-stdout>`）
    #[test]
    fn bashモードの入力は発話出力は通知にする() {
        let input = json!("<bash-input>git status</bash-input>");
        assert_eq!(
            classify_user_content(&input),
            UserContent::Speech {
                text: "! git status".into(),
                images: 0
            }
        );
        let output = json!("<bash-stdout>clean</bash-stdout><bash-stderr></bash-stderr>");
        assert_eq!(
            classify_user_content(&output),
            UserContent::Notice {
                summary: "clean".into()
            }
        );
    }

    /// スラッシュコマンドの前置定型文と実行結果は会話に出さない
    #[test]
    fn ローカルコマンドの定型文と結果は通知にする() {
        // 定型文だけの行は伝える中身が無いので薄い 1 行すら出さない
        // （タグに包まれた形 = 実 transcript で 84 件観測 / 素の形 = 古い形式）
        let wrapped = json!(concat!(
            "<local-command-caveat>Caveat: The messages below were generated by the user while ",
            "running local commands. DO NOT respond to these messages.</local-command-caveat>"
        ));
        assert_eq!(classify_user_content(&wrapped), UserContent::Skip);
        let caveat = json!(concat!(
            "Caveat: The messages below were generated by the user while running local commands. ",
            "DO NOT respond to these messages."
        ));
        assert_eq!(classify_user_content(&caveat), UserContent::Skip);
        let stdout = json!("<local-command-stdout>Set model to opus</local-command-stdout>");
        assert_eq!(
            classify_user_content(&stdout),
            UserContent::Notice {
                summary: "Set model to opus".into()
            }
        );
    }

    /// tool_result だけの行は従来どおり落とす（配列対応で拾い始めない）
    #[test]
    fn tool_result行は配列対応後もスキップする() {
        let raw = json!([{ "type": "tool_result", "tool_use_id": "t1", "content": "ok" }]);
        assert_eq!(classify_user_content(&raw), UserContent::Skip);
        // system-reminder 入りの tool_result も同じ（旧実装で 44 件観測）
        let with_reminder = json!([
            { "type": "tool_result", "tool_use_id": "t1", "content": "ok" },
            { "type": "text", "text": "<system-reminder>note</system-reminder>" },
        ]);
        assert_eq!(classify_user_content(&with_reminder), UserContent::Skip);
    }

    /// 正規化の出力に生 XML・画像メタが一切出ないこと（#715 受け入れ条件 1 の機械版）
    #[test]
    fn 正規化出力にシステムテキストが混ざらない() {
        let raw = [
            r#"{"type":"user","message":{"content":"実装して"}}"#,
            r#"{"type":"user","message":{"content":"[Image: original 3024x1964, displayed at 2000x1299. Multiply coordinates by 1.51 to map to original image.]"}}"#,
            r#"{"type":"user","message":{"content":"<task-notification>\n<task-id>a</task-id>\n<summary>Monitor event</summary>\n</task-notification>"}}"#,
            r#"{"type":"assistant","requestId":"r1","message":{"content":[{"type":"text","text":"やります"}]}}"#,
        ];
        let msgs = normalize_lines(lines(&raw), 10);
        let dump = serde_json::to_string(&msgs).expect("シリアライズ");
        assert!(
            !dump.contains("task-notification"),
            "生 XML が残っている: {dump}"
        );
        assert!(
            !dump.contains("Multiply coordinates"),
            "画像メタが残っている: {dump}"
        );
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0]["text"], "実装して");
        // 画像だけの発話は本文が空 + attachments で表現する
        assert_eq!(msgs[1]["text"], "");
        assert_eq!(msgs[1]["attachments"][0]["kind"], "image");
        assert_eq!(msgs[2]["role"], "system");
        assert_eq!(msgs[2]["kind"], "notice");
        assert_eq!(msgs[2]["text"], "Monitor event");
        assert_eq!(msgs[3]["text"], "やります");
    }

    /// 連続するシステム通知は 1 エントリへまとめる（tail を食い潰さない）
    #[test]
    fn 連続するシステム通知をまとめる() {
        let notice = |n: u32| {
            format!(
                r#"{{"type":"user","message":{{"content":"<task-notification><summary>通知{n}</summary></task-notification>"}}}}"#
            )
        };
        let raw: Vec<String> = vec![
            r#"{"type":"user","message":{"content":"やって"}}"#.to_string(),
            notice(1),
            notice(2),
            notice(3),
            r#"{"type":"assistant","requestId":"r1","message":{"content":[{"type":"text","text":"了解"}]}}"#.to_string(),
        ];
        let msgs = normalize_lines(raw.into_iter(), 10);
        assert_eq!(msgs.len(), 3, "通知 3 件が 1 エントリへ: {msgs:?}");
        assert_eq!(msgs[1]["count"], 3);
        // まとめても最新の要約を出す
        assert_eq!(msgs[1]["text"], "通知3");
    }

    // ─────────── #737: busy 中に打たれた指示（queue-operation） ───────────

    /// 実 transcript の形（実測 3416 本より）で 1 行を組む
    fn queue_line(operation: &str, content: Option<&str>) -> String {
        let mut v = json!({
            "type": "queue-operation",
            "operation": operation,
            "timestamp": "2026-08-02T10:40:21.015Z",
            "sessionId": "s",
        });
        if let Some(c) = content {
            v["content"] = json!(c);
        }
        v.to_string()
    }

    fn user_line(text: &str) -> String {
        json!({
            "type": "user",
            "timestamp": "2026-08-02T10:45:00.000Z",
            "message": { "role": "user", "content": text },
        })
        .to_string()
    }

    /// busy 中に打たれた指示は enqueue の時点で吹き出しになる（配送を待たない）。
    /// これが無いと長いターンの最中は自分の発話がどこにも出ない（#737 追加要件 5）
    #[test]
    fn キューに入った指示はその時点で発話として出る() {
        let msgs = normalize_lines(
            vec![
                user_line("最初のお願い"),
                queue_line("enqueue", Some("busy中の追加指示です")),
            ]
            .into_iter(),
            50,
        );
        assert_eq!(msgs.len(), 2, "{msgs:#?}");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[1]["text"], "busy中の追加指示です");
        assert_eq!(msgs[1]["queued"], json!(true), "送信待ちの印が付く");
    }

    /// 配送されたら「送信待ち」は消え、**本物の user 行が来ても二重に並べない**。
    /// 実測した 2 通りの配送（dequeue → user 行 / ターン内差し込みで remove のみ）を両方見る
    #[test]
    fn キュー発話は配送後に二重化しない() {
        // 経路 A: enqueue → dequeue → 本物の user 行（実測 = 次のターンとして配送）
        let a = normalize_lines(
            vec![
                queue_line("enqueue", Some("あとで届く指示")),
                queue_line("dequeue", None),
                user_line("あとで届く指示"),
            ]
            .into_iter(),
            50,
        );
        assert_eq!(a.len(), 1, "同じ発話を 2 個並べない: {a:#?}");
        assert_eq!(a[0]["text"], "あとで届く指示");
        assert_eq!(
            a[0]["queued"],
            json!(false),
            "配送済みなので送信待ちは消える"
        );

        // 経路 B: enqueue → remove のみ（ターン内へ差し込まれ user 行にならない）
        let b = normalize_lines(
            vec![
                queue_line("enqueue", Some("ターン内へ差し込まれる指示")),
                queue_line("remove", Some("ターン内へ差し込まれる指示")),
            ]
            .into_iter(),
            50,
        );
        assert_eq!(b.len(), 1, "{b:#?}");
        assert_eq!(b[0]["text"], "ターン内へ差し込まれる指示");
        assert_eq!(b[0]["queued"], json!(false));
    }

    /// 同じ文面を 2 回送ったら 2 個出る（重複排除は 1 対 1 で消費する）
    #[test]
    fn 同じ文面を2回送れば2個出る() {
        let msgs = normalize_lines(
            vec![
                queue_line("enqueue", Some("もう一度")),
                queue_line("dequeue", None),
                user_line("もう一度"),
                user_line("もう一度"),
            ]
            .into_iter(),
            50,
        );
        assert_eq!(msgs.len(), 2, "2 回の発話は 2 個出る: {msgs:#?}");
    }

    /// キューへ入るのはユーザー発話だけではない（実測 = `<task-notification>` 1760 件）。
    /// システム注入がキュー経由で生 XML の吹き出しになってはいけない（#715 の保証）
    #[test]
    fn キュー経由のシステム注入は吹き出しにしない() {
        let msgs = normalize_lines(
            vec![queue_line(
                "enqueue",
                Some("<task-notification>\n<summary>Monitor event</summary>\n</task-notification>"),
            )]
            .into_iter(),
            50,
        );
        assert!(
            msgs.is_empty(),
            "システム注入はキュー経由でも発話にしない: {msgs:#?}"
        );
    }

    /// tail から押し出された後に本物の user 行が来ても、取り違えて消さない
    #[test]
    fn tail外へ出たキュー発話は本物を消さない() {
        let mut lines = vec![queue_line("enqueue", Some("古い指示"))];
        // tail=2 なので後続の 2 発話で押し出される
        lines.push(user_line("別の話1"));
        lines.push(user_line("別の話2"));
        lines.push(user_line("古い指示"));
        let msgs = normalize_lines(lines.into_iter(), 2);
        assert_eq!(msgs.len(), 2, "{msgs:#?}");
        assert_eq!(
            msgs[1]["text"], "古い指示",
            "本物の配送が消えてはいけない: {msgs:#?}"
        );
    }

    /// PWA へ配る手前でシステム通知を落とす（role 判定できないフロント向け）
    #[test]
    fn without_system_noticesは通知だけ落とす() {
        let value = json!({
            "session_id": "s",
            "messages": [
                { "role": "user", "text": "やって" },
                { "role": "system", "kind": "notice", "text": "通知" },
                { "role": "assistant", "text": "はい" },
            ],
        });
        let filtered = without_system_notices(value);
        let messages = filtered["messages"].as_array().expect("配列");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "assistant");
    }

    /// 会話の見出し（セッションカタログ）にもシステムテキストを出さない
    #[test]
    fn first_user_textはシステムテキストを飛ばす() {
        // classify を通しているので、画像メタ → 通知 → 本物の発話の順でも本物を拾う
        let raw = json!("[Image: original 100x100, displayed at 50x50.]");
        assert!(matches!(
            classify_user_content(&raw),
            UserContent::Speech { text, .. } if text.is_empty()
        ));
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
