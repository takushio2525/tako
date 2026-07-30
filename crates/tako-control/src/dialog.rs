//! dialog — エージェント TUI の対話ダイアログ（AskUserQuestion）の読み取りと操作（Issue #662）
//!
//! master が worker の対話ダイアログを MCP から完全に操作するための部品。
//! 「内容の取得」と「応答キーの合成」を分け、どちらも純関数として書いてある
//! （画面文字列と transcript の JSON を渡せばテストできる）。
//!
//! # 内容の取得（**保留中の正はライブ画面**）
//!
//! - **ライブ画面（保留中の唯一のソース）**: 表示中の質問・選択肢・ハイライト・
//!   回答済みタブ・multiSelect のチェック状態。キー合成と送信前検証もここが根拠。
//!   画面は **1 問ずつしか映さない**ので、選択肢の解決も 1 問ずつ行う
//! - **transcript（補助）**: session JSONL の `tool_use`（name = `AskUserQuestion`）は
//!   質問と選択肢の全文を持つ。当初はこちらを第一ソースにする設計だったが、
//!   **claude はダイアログを表示している間 transcript に何も書かない**ことを
//!   隔離 E2E で実測した（`tool_use` の行は回答が確定してから `tool_result` と
//!   一緒に現れる）。保留中の内容取得には使えないので、回答後の照会と、
//!   将来 claude が挙動を変えた場合に備えて読める形だけ残してある
//!
//! ペイン幅の制約は残る（狭いペインではラベルが折り返しで欠ける）。
//! **番号指定は常に確実**なので、エラー文はそちらへ誘導する
//!
//! # 実測した操作モデル（claude v2.1.220。#662 に採取画面あり）
//!
//! | 画面 | 数字キー `N` | その他 |
//! |---|---|---|
//! | 単一選択の質問 | 選択 + **次の未回答質問へ自動前進** | Esc = 取消 |
//! | multiSelect の質問 | チェックの**トグル**（カーソルは動かない） | `Tab` = 確認画面へ |
//! | 確認画面 | `1` = 送信 / `2` = 取消 | Enter = ハイライト行を実行 |
//!
//! 確認画面が「選んだ結果」を表示するので、**送信前に意図と一致するかを検証できる**。
//! 撃ちっぱなしにしないための足場として使う。

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// transcript 側: 保留中の AskUserQuestion
// ---------------------------------------------------------------------------

/// AskUserQuestion の 1 つの選択肢
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogOption {
    pub label: String,
    pub description: String,
}

/// AskUserQuestion の 1 問
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogQuestion {
    pub question: String,
    pub header: String,
    pub multi_select: bool,
    pub options: Vec<DialogOption>,
}

/// 保留中（tool_result 未着）の AskUserQuestion
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingDialog {
    pub tool_use_id: String,
    pub questions: Vec<DialogQuestion>,
}

impl PendingDialog {
    pub fn to_json(&self) -> Value {
        json!({
            "tool_use_id": self.tool_use_id,
            "questions": self.questions.iter().enumerate().map(|(qi, q)| json!({
                "index": qi + 1,
                "question": q.question,
                "header": q.header,
                "multi_select": q.multi_select,
                "options": q.options.iter().enumerate().map(|(oi, o)| json!({
                    "index": oi + 1,
                    "label": o.label,
                    "description": o.description,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })
    }
}

/// transcript の JSONL 行から「保留中の AskUserQuestion」を取り出す。
///
/// 判定は **`tool_use` に対応する `tool_result` が未着**であること。ツール名で絞るので
/// 「ツール実行中」と「回答待ち」を取り違えない（#425 で承認カードが誤爆した教訓の逆で、
/// ここは名前が AskUserQuestion に限定されているため tool_result 未着 = 回答待ちが確定する）。
///
/// 同一セッションに複数あれば**最後のもの**を返す（TUI が表示しているのは最新）
pub fn pending_from_transcript_lines<I: Iterator<Item = String>>(
    lines: I,
) -> Option<PendingDialog> {
    let mut pending: Vec<PendingDialog> = Vec::new();
    let mut answered: Vec<String> = Vec::new();

    for line in lines {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(content) = v["message"]["content"].as_array() else {
            continue;
        };
        for block in content {
            match block["type"].as_str() {
                Some("tool_use") if block["name"].as_str() == Some("AskUserQuestion") => {
                    let Some(id) = block["id"].as_str() else {
                        continue;
                    };
                    let questions = parse_questions(&block["input"]);
                    if questions.is_empty() {
                        continue;
                    }
                    pending.push(PendingDialog {
                        tool_use_id: id.to_string(),
                        questions,
                    });
                }
                Some("tool_result") => {
                    if let Some(id) = block["tool_use_id"].as_str() {
                        answered.push(id.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    pending
        .into_iter()
        .rfind(|p| !answered.contains(&p.tool_use_id))
}

fn parse_questions(input: &Value) -> Vec<DialogQuestion> {
    input["questions"]
        .as_array()
        .map(|qs| {
            qs.iter()
                .map(|q| DialogQuestion {
                    question: q["question"].as_str().unwrap_or_default().to_string(),
                    header: q["header"].as_str().unwrap_or_default().to_string(),
                    multi_select: q["multiSelect"].as_bool().unwrap_or(false),
                    options: q["options"]
                        .as_array()
                        .map(|os| {
                            os.iter()
                                .map(|o| DialogOption {
                                    label: o["label"].as_str().unwrap_or_default().to_string(),
                                    description: o["description"]
                                        .as_str()
                                        .unwrap_or_default()
                                        .to_string(),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// session_id から保留中の AskUserQuestion を読む（transcript ファイル経由）
pub fn pending_for_session(session_id: &str) -> Option<PendingDialog> {
    use std::io::BufRead;
    let path = crate::transcript::find_transcript(session_id)?;
    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);
    pending_from_transcript_lines(reader.lines().map_while(Result::ok))
}

// ---------------------------------------------------------------------------
// 画面側: 今どこを表示しているか
// ---------------------------------------------------------------------------

/// ダイアログ画面の段階
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogStage {
    /// 質問を表示して選択肢を待っている
    Question,
    /// 全問回答後の確認画面（Submit answers / Cancel）
    Review,
}

/// 画面に見えている選択肢の 1 行
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenOption {
    /// 表示番号（`1.` の 1）
    pub number: usize,
    /// ラベル（画面幅で切り詰められている可能性がある）
    pub label: String,
    /// カーソル（`❯`）が乗っているか
    pub highlighted: bool,
    /// multiSelect のチェック状態（`[ ]` / `[✔]`）。単一選択では None
    pub checked: Option<bool>,
}

/// 質問タブ 1 個（`☐ 色` / `☒ 色`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionTab {
    pub header: String,
    /// 回答済み（`☒`）か
    pub answered: bool,
}

/// AskUserQuestion ダイアログのライブ画面状態
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogScreen {
    pub stage: DialogStage,
    /// 上部タブ（質問ごと）。表示幅で切れていることがある
    pub tabs: Vec<QuestionTab>,
    /// 表示中の質問文（幅で切り詰められている可能性がある）
    pub question: String,
    pub options: Vec<ScreenOption>,
    /// 確認画面に並んだ「→ 選択結果」の行
    pub review_answers: Vec<String>,
}

impl DialogScreen {
    pub fn to_json(&self) -> Value {
        json!({
            "stage": match self.stage {
                DialogStage::Question => "question",
                DialogStage::Review => "review",
            },
            "tabs": self.tabs.iter().map(|t| json!({
                "header": t.header,
                "answered": t.answered,
            })).collect::<Vec<_>>(),
            "question": self.question,
            "options": self.options.iter().map(|o| json!({
                "number": o.number,
                "label": o.label,
                "highlighted": o.highlighted,
                "checked": o.checked,
            })).collect::<Vec<_>>(),
            "review_answers": self.review_answers,
        })
    }

    /// 表示中が何問目か（0-based）。未回答の最初のタブを現在位置とみなす。
    /// 確認画面では None
    pub fn current_question_index(&self) -> Option<usize> {
        if self.stage == DialogStage::Review {
            return None;
        }
        self.tabs.iter().position(|t| !t.answered)
    }
}

/// ダイアログのタブ行（`←  ☐ 色  ☒ 食べ物  ✔ Submit  →`）のマーカー
const TAB_UNANSWERED: char = '☐';
const TAB_ANSWERED: char = '☒';

/// 画面テキストから AskUserQuestion ダイアログを解析する。
/// ダイアログが見えていなければ None
pub fn parse_dialog_screen(lines: &[String]) -> Option<DialogScreen> {
    let tabs = lines.iter().find_map(|l| parse_tab_line(l));
    // タブ行が無ければ AskUserQuestion ダイアログではない
    let tabs = tabs?;

    let is_review = lines
        .iter()
        .any(|l| l.contains("Review your answers") || l.contains("Ready to submit your answers?"));

    let options = parse_option_lines(lines);
    let review_answers = if is_review {
        lines
            .iter()
            .filter_map(|l| {
                let t = l.trim();
                t.strip_prefix("→ ").map(|s| s.trim().to_string())
            })
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        Vec::new()
    };

    // 質問文: タブ行の後、最初の選択肢行より前にある非空行のうち最後のもの
    // （区切り線・空行・ヘルプ行は除く）
    let question = extract_question_text(lines);

    Some(DialogScreen {
        stage: if is_review {
            DialogStage::Review
        } else {
            DialogStage::Question
        },
        tabs,
        question,
        options,
        review_answers,
    })
}

/// `←  ☐ 色  ☒ 食べ物  ✔ Submit  →` を [(色, false), (食べ物, true)] に。
/// `✔ Submit` はタブではなく送信ボタンなので除外する
fn parse_tab_line(line: &str) -> Option<Vec<QuestionTab>> {
    if !line.contains(TAB_UNANSWERED) && !line.contains(TAB_ANSWERED) {
        return None;
    }
    let mut tabs = Vec::new();
    let mut rest = line;
    while let Some(pos) = rest.find([TAB_UNANSWERED, TAB_ANSWERED]) {
        let marker = rest[pos..].chars().next()?;
        let after = &rest[pos + marker.len_utf8()..];
        // マーカーの後ろから次のマーカー（or 行末）までがヘッダー。
        // 末尾の `✔ Submit` / `→` は落とす
        let end = after
            .find([TAB_UNANSWERED, TAB_ANSWERED])
            .unwrap_or(after.len());
        let raw = &after[..end];
        let header = raw
            .split('✔')
            .next()
            .unwrap_or(raw)
            .trim_end_matches(['→', ' '])
            .trim()
            .to_string();
        if !header.is_empty() {
            tabs.push(QuestionTab {
                header,
                answered: marker == TAB_ANSWERED,
            });
        }
        rest = &after[end..];
    }
    (!tabs.is_empty()).then_some(tabs)
}

/// `❯ 1. 赤い夕焼け` / `  2. [ ] みかん` の行を拾う。
/// 選択肢の説明行（番号なしのインデント行）は無視する
fn parse_option_lines(lines: &[String]) -> Vec<ScreenOption> {
    let mut out = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        let (highlighted, inner) = match trimmed.strip_prefix("❯ ") {
            Some(rest) => (true, rest.trim_start()),
            None => (false, trimmed),
        };
        // 「N. 」で始まる行だけが選択肢（N は 1〜9）
        let bytes = inner.as_bytes();
        if bytes.len() < 3 || !bytes[0].is_ascii_digit() || bytes[1] != b'.' || bytes[2] != b' ' {
            continue;
        }
        let number = (bytes[0] - b'0') as usize;
        let body = inner[3..].trim();
        // multiSelect のチェックボックス
        let (checked, label) = if let Some(rest) = body.strip_prefix("[ ]") {
            (Some(false), rest.trim())
        } else if let Some(rest) = body.strip_prefix("[✔]") {
            (Some(true), rest.trim())
        } else if let Some(rest) = body
            .strip_prefix("[x]")
            .or_else(|| body.strip_prefix("[X]"))
        {
            (Some(true), rest.trim())
        } else {
            (None, body)
        };
        out.push(ScreenOption {
            number,
            label: label.to_string(),
            highlighted,
            checked,
        });
    }
    out
}

/// 質問文の抽出。タブ行の直後から最初の選択肢行までにある最後の非空行
fn extract_question_text(lines: &[String]) -> String {
    let mut after_tabs = false;
    let mut candidate = String::new();
    for line in lines {
        let t = line.trim();
        if parse_tab_line(line).is_some() {
            after_tabs = true;
            candidate.clear();
            continue;
        }
        if !after_tabs {
            continue;
        }
        // 選択肢行に到達したら終わり
        let inner = t.strip_prefix("❯ ").unwrap_or(t);
        let b = inner.as_bytes();
        if b.len() >= 3 && b[0].is_ascii_digit() && b[1] == b'.' && b[2] == b' ' {
            break;
        }
        if t.is_empty() || is_rule_line(t) {
            continue;
        }
        candidate = t.to_string();
    }
    candidate
}

/// 罫線だけの行か
fn is_rule_line(t: &str) -> bool {
    !t.is_empty() && t.chars().all(|c| c == '─' || c == '━' || c.is_whitespace())
}

// ---------------------------------------------------------------------------
// 応答キーの合成
// ---------------------------------------------------------------------------
//
// **解決の対象はライブ画面**であって transcript ではない。#662 の隔離 E2E で
// 「claude はダイアログを表示している間 transcript に何も書かない」ことを実測した
// （`tool_use:AskUserQuestion` の行は **回答が確定してから** `tool_result` と一緒に
// 現れる）。したがって保留中のダイアログの内容は画面からしか取れない。
//
// 画面は 1 問ずつしか映さないので、解決も 1 問ずつ「今映っている画面に対して」行う。

/// 1 問への回答指定（呼び出し側 = master が渡す）
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AnswerSpec {
    /// 対象の質問。1 始まりの番号、またはタブ見出しの前方一致。省略時は順番に割り当てる
    pub question: Option<String>,
    /// 選ぶ選択肢。1 始まりの番号、または label の前方一致。multiSelect では複数指定可
    pub options: Vec<String>,
}

/// 数字キーで選べる選択肢の上限。TUI は選択肢を `1.`〜`9.` と振り、
/// 選択は数字キー 1 発（実測モデル）。2 桁は 1 打鍵にならないのでここで打ち切る
pub const MAX_NUMBER_KEY_OPTION: usize = 9;

impl DialogScreen {
    /// この画面が multiSelect の質問か（チェックボックス付きの選択肢がある）
    pub fn is_multi_select(&self) -> bool {
        self.options.iter().any(|o| o.checked.is_some())
    }

    /// 「Type something.」「Chat about this」のような、質問の選択肢ではない項目を除いた一覧。
    /// 番号を撃ち間違えないよう **番号は画面のまま**保持する
    pub fn answer_options(&self) -> Vec<&ScreenOption> {
        self.options
            .iter()
            .filter(|o| !is_meta_option(&o.label))
            .collect()
    }
}

/// TUI が常に足す定型項目（回答の選択肢ではない）
fn is_meta_option(label: &str) -> bool {
    let l = label.trim().to_lowercase();
    l.starts_with("type something") || l.starts_with("chat about this")
}

/// 画面の選択肢一覧を「1. ラベル / 2. ラベル」の形にした案内文（エラーに添える）
fn screen_labels_hint(screen: &DialogScreen) -> String {
    screen
        .options
        .iter()
        .map(|o| format!("{}. {}", o.number, o.label))
        .collect::<Vec<_>>()
        .join(" / ")
}

/// 回答指定を、**今画面に映っている選択肢**へ解決する。
///
/// 番号指定（`"2"`）とラベル一致（完全 → 前方 → 部分）の両方を受ける。
/// ラベルは狭いペインだと折り返しで欠けることがあるため、**番号指定が最も確実**
pub fn resolve_screen_option<'a>(
    screen: &'a DialogScreen,
    spec: &str,
) -> Result<&'a ScreenOption, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("選択肢が空".to_string());
    }
    if let Ok(n) = spec.parse::<usize>() {
        return screen
            .options
            .iter()
            .find(|o| o.number == n)
            .ok_or_else(|| format!("選択肢 {n} は画面に無い: {}", screen_labels_hint(screen)));
    }

    let lower = spec.to_lowercase();
    let pick = |pred: &dyn Fn(&str) -> bool| -> Vec<&'a ScreenOption> {
        screen
            .options
            .iter()
            .filter(|o| pred(&o.label.to_lowercase()))
            .collect()
    };
    let exact = pick(&|l: &str| l == lower);
    if exact.len() == 1 {
        return Ok(exact[0]);
    }
    let prefix = pick(&|l: &str| l.starts_with(&lower));
    match prefix.len() {
        1 => return Ok(prefix[0]),
        n if n > 1 => {
            return Err(format!(
                "選択肢 '{spec}' が複数に一致して曖昧: {}",
                screen_labels_hint(screen)
            ))
        }
        _ => {}
    }
    let contains = pick(&|l: &str| l.contains(&lower));
    match contains.len() {
        1 => Ok(contains[0]),
        0 => Err(format!(
            "選択肢 '{spec}' に一致するものが無い: {}（狭いペインではラベルが折り返しで欠けます。番号で指定してください）",
            screen_labels_hint(screen)
        )),
        _ => Err(format!(
            "選択肢 '{spec}' が複数に一致して曖昧: {}",
            screen_labels_hint(screen)
        )),
    }
}

/// 質問の指定（番号 or タブ見出し）を index へ解決する
pub fn resolve_question_tab(tabs: &[QuestionTab], spec: &str) -> Result<usize, String> {
    let spec = spec.trim();
    if let Ok(n) = spec.parse::<usize>() {
        if n == 0 || n > tabs.len() {
            return Err(format!("質問 {n} は範囲外（1〜{}）", tabs.len()));
        }
        return Ok(n - 1);
    }
    let lower = spec.to_lowercase();
    let matches: Vec<usize> = tabs
        .iter()
        .enumerate()
        .filter(|(_, t)| t.header.to_lowercase().starts_with(&lower))
        .map(|(i, _)| i)
        .collect();
    match matches.len() {
        1 => Ok(matches[0]),
        0 => Err(format!(
            "質問 '{spec}' に一致するものが無い: {}",
            tabs.iter()
                .enumerate()
                .map(|(i, t)| format!("{}. {}", i + 1, t.header))
                .collect::<Vec<_>>()
                .join(" / ")
        )),
        _ => Err(format!("質問 '{spec}' が複数に一致して曖昧")),
    }
}

/// 回答指定の並びを「質問 index → 回答指定」へ割り当てる。
///
/// 画面は 1 問ずつしか映さないので、**ここでは選択肢まで解決しない**
/// （その質問が表示された時点で `resolve_screen_option` する）。
/// 全問に答えないと送信できないので、件数の不一致はここで弾く
pub fn assign_answers<'a>(
    tabs: &[QuestionTab],
    specs: &'a [AnswerSpec],
) -> Result<Vec<(usize, &'a AnswerSpec)>, String> {
    if specs.len() != tabs.len() {
        return Err(format!(
            "回答数が質問数と一致しない（質問 {} 問に対し回答 {} 件）。\
             全問に答えないと送信できない: {}",
            tabs.len(),
            specs.len(),
            tabs.iter()
                .enumerate()
                .map(|(i, t)| format!("{}. {}", i + 1, t.header))
                .collect::<Vec<_>>()
                .join(" / ")
        ));
    }
    let mut out: Vec<(usize, &AnswerSpec)> = Vec::new();
    for (i, spec) in specs.iter().enumerate() {
        let qi = match spec.question.as_deref() {
            Some(s) => resolve_question_tab(tabs, s)?,
            None => i,
        };
        if out.iter().any(|(q, _)| *q == qi) {
            return Err(format!("質問 {} への回答が重複している", qi + 1));
        }
        if spec.options.is_empty() {
            return Err(format!("質問 {} への選択肢が空", qi + 1));
        }
        out.push((qi, spec));
    }
    out.sort_by_key(|(qi, _)| *qi);
    Ok(out)
}

/// 今映っている質問に答えるためのキー列と、選んだラベル。
///
/// 単一選択は「番号キー 1 発」（選択 + 自動前進）。multiSelect は
/// 「番号キーで各選択肢をトグル → Tab で次へ」。実測モデルどおり
pub fn keys_for_screen_answer(
    screen: &DialogScreen,
    spec: &AnswerSpec,
) -> Result<(Vec<String>, Vec<String>), String> {
    if spec.options.len() > 1 && !screen.is_multi_select() {
        return Err(format!(
            "この質問は単一選択なのに {} 件指定されている",
            spec.options.len()
        ));
    }
    let mut keys = Vec::new();
    let mut labels = Vec::new();
    let mut seen: Vec<usize> = Vec::new();
    for s in &spec.options {
        let option = resolve_screen_option(screen, s)?;
        if seen.contains(&option.number) {
            return Err(format!("選択肢 {} が重複している", option.number));
        }
        if option.number > MAX_NUMBER_KEY_OPTION {
            return Err(format!(
                "選択肢 {} は数字キーで選べない（{MAX_NUMBER_KEY_OPTION} 番まで）。\
                 tako_send_keys で down / enter を送って操作すること",
                option.number
            ));
        }
        seen.push(option.number);
        keys.push(option.number.to_string());
        labels.push(option.label.clone());
    }
    if screen.is_multi_select() {
        // multiSelect はトグルするだけでは前進しないので明示的に Tab
        keys.push("tab".to_string());
    }
    Ok((keys, labels))
}

/// 確認画面で「送信」を選ぶキー。ハイライトは既定で `1. Submit answers` に乗っているので
/// Enter で足りるが、取消側に乗っている場合に備えて番号で撃つ
pub fn keys_for_submit(screen: &DialogScreen) -> Vec<String> {
    let submit = screen
        .options
        .iter()
        .find(|o| o.label.to_lowercase().starts_with("submit"));
    match submit {
        Some(o) => vec![o.number.to_string()],
        // 確認画面の選択肢が読めない（幅で切れた等）ときは既定の 1
        None => vec!["1".to_string()],
    }
}

/// **既に確認画面に居る状態**で呼ばれたときの検証（#662）。
///
/// この呼び出しでは 1 問も答えていない（前回の `dry_run` 等で選択済み）ため、
/// 「何を選んだか」を自分で知らない。確認画面のテキストと**呼び出し側の指定**を
/// 突き合わせるしかない:
///
/// - ラベル指定（`"青い海"`）: 確認画面に現れているかを見れば検証できる
/// - 番号指定（`"2"`）: 参照先の選択肢一覧がもう画面に無いので**検証不能**。
///   検証できないものを黙って送信すると「指定と違う回答を確定させる」事故になるため断る
pub fn verify_specs_against_review(
    specs: &[AnswerSpec],
    screen: &DialogScreen,
) -> Result<(), String> {
    if screen.stage != DialogStage::Review {
        return Err("確認画面に到達していない".to_string());
    }
    let joined = screen.review_answers.join(" / ");
    for spec in specs {
        for option in &spec.options {
            if option.trim().parse::<usize>().is_ok() {
                return Err(format!(
                    "既に確認画面まで進んでいるため、番号指定 '{option}' が\
                     何を指すか検証できません（選択肢一覧が画面に無い）。\
                     確認画面の内容は「{joined}」です。この内容でよければ\
                     ラベルで指定し直してください"
                ));
            }
            if !review_mentions(&joined, option) {
                return Err(format!(
                    "確認画面に '{option}' が見つからない（画面: {joined}）"
                ));
            }
        }
    }
    Ok(())
}

/// 確認画面の内容が意図と一致するかを検証する。
///
/// 画面はペイン幅で切り詰められている可能性があるので、**完全一致は求めない**。
/// 「選んだラベルの先頭部分が確認画面の該当行に現れているか」を見る。
/// 一致しなければ呼び出し側は submit してはいけない
pub fn verify_review(chosen_labels: &[String], screen: &DialogScreen) -> Result<(), String> {
    if screen.stage != DialogStage::Review {
        return Err("確認画面に到達していない".to_string());
    }
    let joined = screen.review_answers.join(" / ");
    for label in chosen_labels {
        if !review_mentions(&joined, label) {
            return Err(format!(
                "確認画面に '{label}' が見つからない（画面: {joined}）"
            ));
        }
    }
    Ok(())
}

/// 幅で切り詰められた確認画面でもラベルを照合できるようにする。
///
/// 全文が入っていればそれで良し。入っていなければ「先頭 6 文字ぶん」が
/// 現れているかを見る（`青い海` のような短いラベルは全文一致になる）
fn review_mentions(joined: &str, label: &str) -> bool {
    if label.is_empty() {
        return true;
    }
    if joined.contains(label) {
        return true;
    }
    let head: String = label.chars().take(6).collect();
    !head.is_empty() && joined.contains(&head)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- 実採取した画面（claude v2.1.220、99 桁ペイン。#662 に記録） ---

    /// 単一選択の 2 問ダイアログ（1 問目を表示中）
    const SINGLE_Q1: &str = "\
● I'll ask both questions together.\n\
─────────────────────────────────────────────\n\
←  ☐ 色  ☐ 食べ物  ✔ Submit  →\n\
\n\
好きな色はどれ?\n\
\n\
❯ 1. 赤い夕焼け\n\
     暖色系。夕暮れの空のような赤\n\
  2. 青い海\n\
     寒色系。深い海のような青\n\
  3. 緑の森\n\
     自然系。木々のような緑\n\
  4. Type something.\n\
─────────────────────────────────────────────\n\
  5. Chat about this\n\
\n\
Enter to select · Tab/Arrow keys to navigate · Esc to cancel";

    /// 1 問目に回答済みで 2 問目を表示中
    const SINGLE_Q2: &str = "\
←  ☒ 色  ☐ 食べ物  ✔ Submit  →\n\
\n\
好きな食べ物はどれ?\n\
\n\
❯ 1. 寿司\n\
     和食。新鮮な魚とご飯\n\
  2. ラーメン\n\
  3. カレー\n\
  4. Type something.\n\
  5. Chat about this\n\
\n\
Enter to select · Tab/Arrow keys to navigate · Esc to cancel";

    /// multiSelect の質問（1 番だけチェック済み）
    const MULTI: &str = "\
←  ☒ 果物  ✔ Submit  →\n\
\n\
好きな果物を全部選んで\n\
\n\
❯ 1. [✔] りんご\n\
  シャキシャキした食感。秋から冬が旬\n\
  2. [ ] みかん\n\
  手で剥ける柑橘。冬のこたつのお供\n\
  3. [ ] ぶどう\n\
  4. [ ] もも\n\
  5. [ ] Type something\n\
     Submit\n\
  6. Chat about this\n\
\n\
Enter to select · ↑/↓ to navigate · Esc to cancel";

    /// 確認画面
    const REVIEW: &str = "\
←  ☒ 色  ☒ 食べ物  ✔ Submit  →\n\
\n\
Review your answers\n\
\n\
 ● 好きな色はどれ?\n\
   → 青い海\n\
 ● 好きな食べ物はどれ?\n\
   → カレー\n\
\n\
Ready to submit your answers?\n\
\n\
❯ 1. Submit answers\n\
  2. Cancel";

    /// permission ダイアログ（AskUserQuestion ではない = このパーサは反応してはいけない）
    const PERMISSION: &str = "\
Bash command\n\
\n\
  cargo test --workspace\n\
\n\
Do you want to proceed?\n\
❯ 1. Yes\n\
  2. Yes, and don't ask again\n\
  3. No, and tell Claude what to do differently";

    /// 通常の入力待ち画面
    const IDLE: &str = "\
● 実装しました。\n\
─────────────────────────────────────────────\n\
❯ \n\
─────────────────────────────────────────────\n\
  ctx 13%";

    fn screen(s: &str) -> Vec<String> {
        s.lines().map(str::to_string).collect()
    }

    // --- transcript 側 ---

    #[test]
    fn transcriptから保留中のaskuserquestionを取り出す() {
        let lines = vec![
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"AskUserQuestion","input":{"questions":[{"question":"好きな色はどれ?","header":"色","multiSelect":false,"options":[{"label":"赤い夕焼け","description":"暖色系"},{"label":"青い海","description":"寒色系"}]}]}}]}}"#.to_string(),
        ];
        let p = pending_from_transcript_lines(lines.into_iter()).expect("保留中が取れる");
        assert_eq!(p.tool_use_id, "t1");
        assert_eq!(p.questions.len(), 1);
        assert_eq!(p.questions[0].header, "色");
        assert_eq!(p.questions[0].question, "好きな色はどれ?");
        assert!(!p.questions[0].multi_select);
        assert_eq!(p.questions[0].options.len(), 2);
        assert_eq!(p.questions[0].options[1].label, "青い海");
        assert_eq!(p.questions[0].options[0].description, "暖色系");
    }

    /// tool_result が届いた質問は保留中ではない（回答済みを再提示しない）
    #[test]
    fn 回答済みのaskuserquestionは保留中に出ない() {
        let lines = vec![
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"AskUserQuestion","input":{"questions":[{"question":"Q","header":"H","options":[{"label":"A"}]}]}}]}}"#.to_string(),
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"Your questions have been answered"}]}}"#.to_string(),
        ];
        assert!(pending_from_transcript_lines(lines.into_iter()).is_none());
    }

    /// 同一セッションに複数あれば最新（TUI が表示しているのは最後のもの）
    #[test]
    fn 複数保留中なら最後のものを返す() {
        let lines = vec![
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"AskUserQuestion","input":{"questions":[{"question":"Q1","header":"H1","options":[{"label":"A"}]}]}}]}}"#.to_string(),
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"AskUserQuestion","input":{"questions":[{"question":"Q2","header":"H2","options":[{"label":"B"}]}]}}]}}"#.to_string(),
        ];
        let p = pending_from_transcript_lines(lines.into_iter()).unwrap();
        assert_eq!(p.tool_use_id, "t2");
    }

    /// 他のツール（Bash 等）の tool_use は拾わない
    #[test]
    fn 他ツールのtool_useは拾わない() {
        let lines = vec![
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#.to_string(),
        ];
        assert!(pending_from_transcript_lines(lines.into_iter()).is_none());
    }

    #[test]
    fn 壊れたjsonl行は飛ばして継続する() {
        let lines = vec![
            "これは JSON ではない".to_string(),
            String::new(),
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t9","name":"AskUserQuestion","input":{"questions":[{"question":"Q","header":"H","options":[{"label":"A"}]}]}}]}}"#.to_string(),
        ];
        let p = pending_from_transcript_lines(lines.into_iter()).unwrap();
        assert_eq!(p.tool_use_id, "t9");
    }

    #[test]
    fn multiselectフラグを読む() {
        let lines = vec![
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"AskUserQuestion","input":{"questions":[{"question":"Q","header":"果物","multiSelect":true,"options":[{"label":"りんご"},{"label":"みかん"}]}]}}]}}"#.to_string(),
        ];
        let p = pending_from_transcript_lines(lines.into_iter()).unwrap();
        assert!(p.questions[0].multi_select);
    }

    // --- 画面側 ---

    #[test]
    fn 単一選択の質問画面を解析する() {
        let s = parse_dialog_screen(&screen(SINGLE_Q1)).expect("ダイアログが検知される");
        assert_eq!(s.stage, DialogStage::Question);
        assert_eq!(s.tabs.len(), 2);
        assert_eq!(s.tabs[0].header, "色");
        assert!(!s.tabs[0].answered);
        assert_eq!(s.tabs[1].header, "食べ物");
        assert!(!s.tabs[1].answered);
        assert_eq!(s.question, "好きな色はどれ?");
        // Type something / Chat about this も選択肢として見える（番号を撃ち間違えないため）
        assert_eq!(s.options.len(), 5);
        assert_eq!(s.options[0].number, 1);
        assert_eq!(s.options[0].label, "赤い夕焼け");
        assert!(s.options[0].highlighted);
        assert!(!s.options[1].highlighted);
        assert_eq!(s.options[1].label, "青い海");
        // 単一選択にチェックボックスは無い
        assert!(s.options.iter().all(|o| o.checked.is_none()));
        assert_eq!(s.current_question_index(), Some(0));
    }

    #[test]
    fn 回答済みタブと現在位置を追える() {
        let s = parse_dialog_screen(&screen(SINGLE_Q2)).unwrap();
        assert!(s.tabs[0].answered);
        assert!(!s.tabs[1].answered);
        assert_eq!(s.current_question_index(), Some(1));
        assert_eq!(s.question, "好きな食べ物はどれ?");
    }

    #[test]
    fn multiselect画面のチェック状態を読む() {
        let s = parse_dialog_screen(&screen(MULTI)).unwrap();
        assert_eq!(s.stage, DialogStage::Question);
        assert_eq!(s.options[0].label, "りんご");
        assert_eq!(s.options[0].checked, Some(true));
        assert_eq!(s.options[1].checked, Some(false));
        assert_eq!(s.options[1].label, "みかん");
    }

    #[test]
    fn 確認画面を解析する() {
        let s = parse_dialog_screen(&screen(REVIEW)).unwrap();
        assert_eq!(s.stage, DialogStage::Review);
        assert_eq!(s.review_answers, vec!["青い海", "カレー"]);
        assert_eq!(s.current_question_index(), None);
        assert_eq!(keys_for_submit(&s), vec!["1"]);
    }

    /// permission ダイアログ・通常画面には反応しない（誤爆防止）
    #[test]
    fn askuserquestion以外の画面では検知しない() {
        assert!(parse_dialog_screen(&screen(PERMISSION)).is_none());
        assert!(parse_dialog_screen(&screen(IDLE)).is_none());
        assert!(parse_dialog_screen(&[]).is_none());
    }

    // --- 回答の解決（**画面が対象**。transcript は保留中に書かれない = #662 実測） ---

    fn q1() -> DialogScreen {
        parse_dialog_screen(&screen(SINGLE_Q1)).unwrap()
    }
    fn multi() -> DialogScreen {
        parse_dialog_screen(&screen(MULTI)).unwrap()
    }

    #[test]
    fn 選択肢は番号でもラベルでも解決できる() {
        let s = q1();
        assert_eq!(resolve_screen_option(&s, "2").unwrap().label, "青い海");
        assert_eq!(resolve_screen_option(&s, "青い海").unwrap().number, 2);
        // 前方一致
        assert_eq!(resolve_screen_option(&s, "青い").unwrap().number, 2);
        // 部分一致
        assert_eq!(resolve_screen_option(&s, "夕焼け").unwrap().number, 1);
    }

    #[test]
    fn 画面に無い番号や不一致ラベルはエラー() {
        let s = q1();
        let e = resolve_screen_option(&s, "9").unwrap_err();
        assert!(e.contains("画面に無い"), "{e}");
        let e = resolve_screen_option(&s, "紫").unwrap_err();
        assert!(e.contains("一致するものが無い"), "{e}");
        // 候補と「番号で指定せよ」の案内が付く（master が撃ち直せる）
        assert!(e.contains("青い海"), "{e}");
        assert!(e.contains("番号で指定"), "{e}");
        assert!(resolve_screen_option(&s, "").is_err());
    }

    #[test]
    fn 曖昧なラベル指定は拒否する() {
        let s = DialogScreen {
            stage: DialogStage::Question,
            tabs: vec![QuestionTab {
                header: "色".into(),
                answered: false,
            }],
            question: "Q".into(),
            options: vec![
                ScreenOption {
                    number: 1,
                    label: "青い海".into(),
                    highlighted: true,
                    checked: None,
                },
                ScreenOption {
                    number: 2,
                    label: "青い空".into(),
                    highlighted: false,
                    checked: None,
                },
            ],
            review_answers: vec![],
        };
        let e = resolve_screen_option(&s, "青い").unwrap_err();
        assert!(e.contains("曖昧"), "{e}");
        // 完全一致なら曖昧でも解決できる
        assert_eq!(resolve_screen_option(&s, "青い海").unwrap().number, 1);
    }

    #[test]
    fn 質問はタブの番号でも見出しでも解決できる() {
        let tabs = q1().tabs;
        assert_eq!(resolve_question_tab(&tabs, "2").unwrap(), 1);
        assert_eq!(resolve_question_tab(&tabs, "食べ物").unwrap(), 1);
        assert!(resolve_question_tab(&tabs, "9").is_err());
        assert!(resolve_question_tab(&tabs, "音楽").is_err());
    }

    #[test]
    fn 回答は順番に割り当てられる() {
        let tabs = q1().tabs;
        let specs = vec![
            AnswerSpec {
                question: None,
                options: vec!["2".into()],
            },
            AnswerSpec {
                question: None,
                options: vec!["カレー".into()],
            },
        ];
        let assigned = assign_answers(&tabs, &specs).unwrap();
        assert_eq!(assigned[0].0, 0);
        assert_eq!(assigned[0].1.options, vec!["2"]);
        assert_eq!(assigned[1].0, 1);
        assert_eq!(assigned[1].1.options, vec!["カレー"]);
    }

    #[test]
    fn 質問を明示指定すると順不同でも並べ替える() {
        let tabs = q1().tabs;
        let specs = vec![
            AnswerSpec {
                question: Some("食べ物".into()),
                options: vec!["寿司".into()],
            },
            AnswerSpec {
                question: Some("色".into()),
                options: vec!["緑の森".into()],
            },
        ];
        let assigned = assign_answers(&tabs, &specs).unwrap();
        assert_eq!(assigned[0].0, 0);
        assert_eq!(assigned[0].1.options, vec!["緑の森"]);
        assert_eq!(assigned[1].0, 1);
        assert_eq!(assigned[1].1.options, vec!["寿司"]);
    }

    #[test]
    fn 回答数が質問数と違えばエラー() {
        let tabs = q1().tabs;
        let specs = vec![AnswerSpec {
            question: None,
            options: vec!["1".into()],
        }];
        let e = assign_answers(&tabs, &specs).unwrap_err();
        assert!(e.contains("回答数が質問数と一致しない"), "{e}");
        // どの質問が残っているかを示す
        assert!(e.contains("食べ物"), "{e}");
    }

    #[test]
    fn 同じ質問への重複回答はエラー() {
        let tabs = q1().tabs;
        let specs = vec![
            AnswerSpec {
                question: Some("色".into()),
                options: vec!["1".into()],
            },
            AnswerSpec {
                question: Some("色".into()),
                options: vec!["2".into()],
            },
        ];
        let e = assign_answers(&tabs, &specs).unwrap_err();
        assert!(e.contains("重複"), "{e}");
    }

    // --- キー合成 ---

    #[test]
    fn 単一選択のキーは番号一発() {
        let s = q1();
        let spec = AnswerSpec {
            question: None,
            options: vec!["青い海".into()],
        };
        let (keys, labels) = keys_for_screen_answer(&s, &spec).unwrap();
        assert_eq!(keys, vec!["2"]);
        assert_eq!(labels, vec!["青い海"]);
    }

    /// multiSelect は「番号でトグル → Tab で前進」（数字だけでは前進しない実測モデル）
    #[test]
    fn multiselectのキーはトグルとtab() {
        let s = multi();
        assert!(s.is_multi_select());
        let spec = AnswerSpec {
            question: None,
            options: vec!["りんご".into(), "ぶどう".into()],
        };
        let (keys, labels) = keys_for_screen_answer(&s, &spec).unwrap();
        assert_eq!(keys, vec!["1", "3", "tab"]);
        assert_eq!(labels, vec!["りんご", "ぶどう"]);
    }

    #[test]
    fn 単一選択に複数指定はエラー() {
        let s = q1();
        assert!(!s.is_multi_select());
        let spec = AnswerSpec {
            question: None,
            options: vec!["1".into(), "2".into()],
        };
        let e = keys_for_screen_answer(&s, &spec).unwrap_err();
        assert!(e.contains("単一選択"), "{e}");
    }

    #[test]
    fn 同じ選択肢の重複指定はエラー() {
        let s = multi();
        let spec = AnswerSpec {
            question: None,
            options: vec!["1".into(), "りんご".into()],
        };
        let e = keys_for_screen_answer(&s, &spec).unwrap_err();
        assert!(e.contains("重複"), "{e}");
    }

    /// 10 番目以降は数字キー 1 発で選べない。壊れたキー（"10"）を送らずに断り、
    /// 代替手段（tako_send_keys の矢印操作）を案内する
    #[test]
    fn 十番目以降の選択肢は数字キーで選べないと断る() {
        let mut s = q1();
        s.options.push(ScreenOption {
            number: 10,
            label: "十番目".into(),
            highlighted: false,
            checked: None,
        });
        let spec = AnswerSpec {
            question: None,
            options: vec!["十番目".into()],
        };
        let e = keys_for_screen_answer(&s, &spec).unwrap_err();
        assert!(e.contains("数字キーで選べない"), "{e}");
        assert!(e.contains("tako_send_keys"), "{e}");
    }

    /// TUI が常に足す定型項目は「回答の選択肢」から外して数える
    /// （番号自体は画面のまま保つので撃ち間違えない）
    #[test]
    fn 定型項目は回答選択肢から外れる() {
        let s = q1();
        let real: Vec<&str> = s
            .answer_options()
            .iter()
            .map(|o| o.label.as_str())
            .collect();
        assert_eq!(real, vec!["赤い夕焼け", "青い海", "緑の森"]);
        // 番号は画面のまま
        assert_eq!(s.answer_options()[1].number, 2);
        // それでも Type something を明示指定すれば選べる（自由記述へ入る導線）
        assert_eq!(
            resolve_screen_option(&s, "4").unwrap().label,
            "Type something."
        );
    }

    #[test]
    fn 送信キーは確認画面のsubmit番号を使う() {
        // Submit が 2 番に来ている画面でも正しく撃てる
        let s = DialogScreen {
            stage: DialogStage::Review,
            tabs: vec![],
            question: String::new(),
            options: vec![
                ScreenOption {
                    number: 1,
                    label: "Cancel".into(),
                    highlighted: true,
                    checked: None,
                },
                ScreenOption {
                    number: 2,
                    label: "Submit answers".into(),
                    highlighted: false,
                    checked: None,
                },
            ],
            review_answers: vec![],
        };
        assert_eq!(keys_for_submit(&s), vec!["2"]);
    }

    // --- 送信前の検証 ---

    #[test]
    fn 確認画面が意図と一致すれば検証を通る() {
        let s = parse_dialog_screen(&screen(REVIEW)).unwrap();
        verify_review(&["青い海".to_string(), "カレー".to_string()], &s)
            .expect("青い海 / カレー が一致する");
    }

    /// 別の選択肢が写っていたら submit させない（撃ちっぱなし防止の要）
    #[test]
    fn 確認画面が意図と違えば検証で落ちる() {
        let s = parse_dialog_screen(&screen(REVIEW)).unwrap();
        let e = verify_review(&["赤い夕焼け".to_string(), "カレー".to_string()], &s).unwrap_err();
        assert!(e.contains("赤い夕焼け"), "{e}");
    }

    #[test]
    fn 確認画面に到達していなければ検証で落ちる() {
        let s = q1();
        let e = verify_review(&[], &s).unwrap_err();
        assert!(e.contains("到達していない"), "{e}");
    }

    /// 既に確認画面まで進んでいる状態（前回の dry_run 等）での再呼び出し。
    /// このとき「選んだラベル」を持っていないので、空の `chosen_labels` で
    /// `verify_review` を通すと検証が素通りしてしまう。指定と画面を突き合わせる
    #[test]
    fn 確認画面から再開したときは指定と画面を突き合わせる() {
        let s = parse_dialog_screen(&screen(REVIEW)).unwrap();
        // ラベル指定は画面と照合できる
        let ok = vec![
            AnswerSpec {
                question: None,
                options: vec!["青い海".into()],
            },
            AnswerSpec {
                question: None,
                options: vec!["カレー".into()],
            },
        ];
        verify_specs_against_review(&ok, &s).expect("画面と一致する");

        // 画面と違うラベルは弾く
        let ng = vec![AnswerSpec {
            question: None,
            options: vec!["赤い夕焼け".into()],
        }];
        let e = verify_specs_against_review(&ng, &s).unwrap_err();
        assert!(e.contains("赤い夕焼け"), "{e}");

        // 番号指定は参照先が画面に無いので検証不能 → 送信しない
        let num = vec![AnswerSpec {
            question: None,
            options: vec!["2".into()],
        }];
        let e = verify_specs_against_review(&num, &s).unwrap_err();
        assert!(e.contains("検証できません"), "{e}");
        // 何が確定しようとしているかを示して、やり直し方を案内する
        assert!(e.contains("青い海"), "{e}");
        assert!(e.contains("ラベルで指定"), "{e}");
    }

    /// ペイン幅で切り詰められた確認画面でも照合できる
    #[test]
    fn 幅で切れた確認画面でも先頭一致で検証できる() {
        let s = DialogScreen {
            stage: DialogStage::Review,
            tabs: vec![],
            question: String::new(),
            options: vec![],
            // 画面幅で途中まで
            review_answers: vec!["既存ビルドで今".into()],
        };
        verify_review(&["既存ビルドで今すぐ差し替え（推奨）".to_string()], &s)
            .expect("先頭 6 文字で一致する");
    }

    #[test]
    fn json化に必要なフィールドが載る() {
        let p = PendingDialog {
            tool_use_id: "t1".into(),
            questions: vec![DialogQuestion {
                question: "好きな色はどれ?".into(),
                header: "色".into(),
                multi_select: false,
                options: vec![DialogOption {
                    label: "青い海".into(),
                    description: "寒色系".into(),
                }],
            }],
        };
        let v = p.to_json();
        assert_eq!(v["tool_use_id"], "t1");
        assert_eq!(v["questions"][0]["index"], 1);
        assert_eq!(v["questions"][0]["header"], "色");
        assert_eq!(v["questions"][0]["multi_select"], false);
        assert_eq!(v["questions"][0]["options"][0]["index"], 1);
        assert_eq!(v["questions"][0]["options"][0]["label"], "青い海");

        let s = parse_dialog_screen(&screen(REVIEW)).unwrap();
        let sv = s.to_json();
        assert_eq!(sv["stage"], "review");
        assert_eq!(sv["review_answers"][0], "青い海");
    }
}
