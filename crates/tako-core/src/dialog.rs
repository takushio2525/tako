//! 選択肢ダイアログの**構造検知**（Issue #748）。
//!
//! エージェント TUI（claude / codex / agy）は「選択カーソル + 選択肢の並び」で
//! 入力欄を奪うダイアログを出す。種類は permission（ツール承認）だけではなく、
//! usage limit の対処選択・`/model` のモデル選択・`/mcp` のサーバー一覧・
//! plan モードの実行確認・AskUserQuestion の質問など多岐にわたる。
//!
//! **文言リストでは網羅できない**（#530 の教訓）。ここでは画面テキストの
//! 構造だけを見て「選択肢の並びが入力欄を奪っているか」を判定する。
//! 種別の分類（permission / usage_limit …）は文言に依るので、
//! そちらは `tako-control::claude_tui::detect_choice_dialog` の責務に分ける。
//!
//! # 検知の 3 経路（いずれも実採取画面が根拠。証拠は #748 / #1143）
//!
//! 1. **番号つき**: カーソル行の中身が `N. …` で、画面に番号つき行が 2 つ以上
//!    ```text
//!     Do you want to proceed?
//!     ❯ 1. Yes
//!       2. Yes, and don’t ask again for: perl *
//!       3. No
//!    ```
//! 2. **番号なし**（`/mcp` の一覧・agy の信頼ダイアログ）: カーソル行の中身の
//!    開始列に**揃った兄弟行が隣接して 2 つ以上**ある。ただしカーソル行が
//!    上下とも罫線で挟まれていれば入力欄とみなして棄却する（複数行入力の誤検知防止）
//!    ```text
//!       ❯ context7 · ✔ connected · 2 tools
//!         coplay-mcp · ✔ connected · 98 tools
//!         filesystem · ✔ connected · 14 tools
//!    ```
//! 3. **カーソルなしの番号つき**（#1143。狭いペインの `/model` セレクタ）:
//!    ダイアログがペインより高いと claude は箱を上端から描いて下を切り捨てるので、
//!    **選択カーソルの行が画面の外へ出る**（入力欄も描かれない）。番号つきの連なり
//!    そのものを anchor にする。詳細と拒否条件は [`detect_choice_list_in`]
//!    ```text
//!    ▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔   ← 箱の上端は見えている
//!       Select model
//!         1. Defaul…  Opus       ← ラベルは claude 自身が `…` で切り詰める
//!                     5
//!                     1M co      ← 説明列はセル単位で割れる（語境界ではない）
//!         2. Opus (…  Opus
//!                     …          ← 画面はここで尽きる（カーソルは 4. の行）
//!    ```
//!
//! 4. **空の入力欄の上の番号つき**（#1263。claude の auto mode 環境学習の確認）:
//!    番号つき 3 択の**下に空の入力欄が描かれる**形。最下部のカーソル行は入力欄なので
//!    経路 1 の起点にならず、旧実装はそこで打ち切っていた（`choice_dialog` が null =
//!    watch も `WORKER_DIALOG` を出せない）。詳細と拒否条件は
//!    [`numbered_above_empty_input`]
//!    ```text
//!      Teach auto mode about your environment?
//!
//!      ❯ 1. Yes
//!        2. Not now
//!        3. Don't show again
//!
//!      Enter to confirm · Esc to cancel
//!    ────────────────────────────────
//!    ❯                                ← 空の入力欄（ダイアログの外）
//!    ────────────────────────────────
//!    ```
//!
//! # 番号キーの実測（#748。claude v2.1.220）
//!
//! - 番号つきダイアログは**番号キーだけで確定する**（Enter 不要。permission /
//!   AskUserQuestion の実ダイアログで観測）。余分な Enter は入力欄へ抜けるので送らない
//! - 番号なしダイアログでは番号キーは**無反応**。`↑`/`↓` でカーソルを動かして Enter

/// 選択肢 1 個
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceOption {
    /// 画面に出ている番号（番号なしダイアログでは None）
    pub number: Option<u32>,
    /// 選択肢のテキスト（番号と選択カーソルを除いた 1 行ぶん）
    pub label: String,
    /// 選択カーソル（`❯` / `›` / `>`）が指しているか
    pub highlighted: bool,
    /// ラベルが TUI 自身に切り詰められているか（`…` を含む。#1143）。
    ///
    /// **true なら元のラベルは画面のどこにも残っていない**（折り返しと違って
    /// 結合では戻せない）。ラベル一致でこの選択肢を確定してはいけない
    /// = `respond` は番号での指定を要求する
    pub label_truncated: bool,
}

/// 画面に実在する選択肢の並び
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceList {
    /// 表示順の選択肢
    pub options: Vec<ChoiceOption>,
    /// ハイライト位置（`options` の添字）
    pub highlighted: Option<usize>,
    /// 番号キーで選べるか（false = 矢印移動が必要）
    pub numbered: bool,
    /// カーソル行より上のダイアログ本文（罫線で区切った直近ブロック）
    pub header: Vec<String>,
    /// 選択カーソルがある画面行の添字。
    /// **None = カーソルが 1 つも描かれていない**（#1143。ダイアログがペインより
    /// 高くてカーソル行が画面外へ出た形。番号キーでは確定できる）
    pub cursor_row: Option<usize>,
}

impl ChoiceList {
    /// ハイライトされている選択肢のラベル
    pub fn highlighted_label(&self) -> Option<&str> {
        self.highlighted
            .and_then(|i| self.options.get(i))
            .map(|o| o.label.as_str())
    }
}

/// ハイライト付きダイアログを目標の選択肢で確定するための「次の 1 手」（#1236）。
///
/// **矢印移動で確定する画面の規則を 1 実装に保つための型**。respond
/// （`tako-control::dispatch::respond_via` の番号なし経路）と自動承諾
/// （信頼 / Bypass ダイアログ）が同じ判断を通る。片方だけが「素の Enter を送る」
/// 実装を持つと、既定ハイライトが拒否側の画面（claude 2.x の信頼ダイアログは
/// `No, exit` が既定）でエージェントを終了させる（#1236 の事故）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmStep {
    /// ハイライトを目標へ動かす（`key` を `steps` 回送る）。
    /// **送ったら画面を採り直してハイライトを読み直す**（キーを飲まれることがある）
    Move {
        /// `"Down"` / `"Up"`（[`crate::backend::key_name_bytes`] の語彙）
        key: &'static str,
        /// 送る回数（1 以上）
        steps: usize,
    },
    /// ハイライトが目標に乗っている = Enter で確定してよい
    Confirm,
    /// 判断材料が足りない = **Enter を送ってはいけない**
    Blocked(ConfirmBlock),
}

/// 確定を見送る理由（診断ログに残す機械可読コード）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmBlock {
    /// 選択カーソルが画面に描かれていない（どこがハイライトか読めない。#1143 の形）
    NoHighlight,
    /// 目標 / ハイライトの添字が選択肢の範囲外（画面が書き換わった直後）
    OutOfRange,
}

impl ConfirmBlock {
    /// 診断ログ・応答に載せる機械可読 slug
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoHighlight => "no_highlight",
            Self::OutOfRange => "out_of_range",
        }
    }

    /// 人向けの理由（CLI / MCP のエラー本文）
    pub fn note(self) -> &'static str {
        match self {
            Self::NoHighlight => {
                "選択カーソルが画面に無くハイライト位置を特定できない（Enter は送らない）"
            }
            Self::OutOfRange => {
                "選択肢の並びが採取のあいだに変わった（添字が範囲外。Enter は送らない）"
            }
        }
    }
}

/// 目標の選択肢を確定するための次の 1 手を決める（#1236）。
///
/// `count` = 画面に見えている選択肢の数、`highlighted` = 現在のハイライト位置
/// （`None` = カーソルが描かれていない）、`target` = 確定したい選択肢の添字。
///
/// **ハイライトが読めなければ [`ConfirmStep::Confirm`] を返さない**のがこの関数の契約。
/// 呼び出し側は `Move` を送ったあと画面を採り直して**もう一度この関数に聞く**ことで、
/// 「ラベル一致を確認してから Enter」を構造的に守れる
pub fn confirm_step(count: usize, highlighted: Option<usize>, target: usize) -> ConfirmStep {
    if target >= count {
        return ConfirmStep::Blocked(ConfirmBlock::OutOfRange);
    }
    let Some(current) = highlighted else {
        return ConfirmStep::Blocked(ConfirmBlock::NoHighlight);
    };
    if current >= count {
        return ConfirmStep::Blocked(ConfirmBlock::OutOfRange);
    }
    match target.cmp(&current) {
        std::cmp::Ordering::Equal => ConfirmStep::Confirm,
        std::cmp::Ordering::Greater => ConfirmStep::Move {
            key: "Down",
            steps: target - current,
        },
        std::cmp::Ordering::Less => ConfirmStep::Move {
            key: "Up",
            steps: current - target,
        },
    }
}

/// 選択肢ダイアログが画面に実在するか（= 入力欄が奪われているか）。
///
/// `tako-control::claude_tui::is_choice_dialog` はこれに委譲する（実装を 1 本にして、
/// 送達フロー・入力欄判定・worker 状態がすべて同じ判定を通るようにする）
pub fn is_choice_dialog(lines: &[&str]) -> bool {
    detect_choice_list(lines).is_some()
}

/// 画面テキストから選択肢の並びを検知する。
///
/// 判定は最下部の選択カーソル行を起点にする（会話ログに残っている過去の
/// `❯ 送信済みメッセージ` を拾わないため）。
///
/// 狭いペイン（実発生は 21〜25 桁）ではラベルが折り返されて切り詰まるので、
/// **物理行で組んだうえで、折り返しを結合した行でも組み直して比べる**（#1131）。
/// 採るのは「選択肢の数が減っていない」ほうだけなので、結合が選択肢を畳んで
/// しまう形（番号なしの並び）では物理行の結果がそのまま残る。
/// 折り返しが 1 つも無い画面では入力が**同一の文字列**なので 1 ビットも変わらない
pub fn detect_choice_list(lines: &[&str]) -> Option<ChoiceList> {
    let raw = detect_choice_list_in(lines);
    let unwrapped = unwrap_dialog_lines(lines);
    if unwrapped.iter().zip(lines).all(|(a, b)| a == b) {
        return raw; // 結合が起きていない = 判定材料が同一
    }
    let refs: Vec<&str> = unwrapped.iter().map(String::as_str).collect();
    match (detect_choice_list_in(&refs), raw) {
        // 結合後のほうが選択肢を取りこぼしていなければそちらを採る
        // （ラベルが 1 本に戻っているのはこちらだけ）
        (Some(joined), Some(raw)) if joined.options.len() >= raw.options.len() => Some(joined),
        (Some(joined), None) => Some(joined),
        (_, raw) => raw,
    }
}

/// [`detect_choice_list`] の本体（渡された行をそのまま材料にする）
fn detect_choice_list_in(lines: &[&str]) -> Option<ChoiceList> {
    let bottom = lines.iter().rposition(|l| !l.trim().is_empty())? + 1;
    let scan_from = bottom.saturating_sub(SCAN_ROWS);
    let Some(cursor_row) = (scan_from..bottom)
        .rev()
        .find(|&i| cursor_content(lines[i]).is_some())
    else {
        // 経路 3: 選択カーソルが 1 つも描かれていない（#1143）
        return detect_cursorless_numbered(lines, bottom);
    };
    let content = cursor_content(lines[cursor_row])?;

    // 経路 1: 番号つき（カーソルが `N. …` を指している + 画面に 2 つ以上）
    if numbered_choice(content).is_some() {
        let numbered = numbered_block(lines, bottom, cursor_row);
        if numbered.len() >= 2 {
            return Some(numbered_list(lines, &numbered, cursor_row));
        }
    }

    // 経路 2: 番号なし（開始列の揃った兄弟行が隣接している）
    if content.trim().is_empty() {
        // 空のカーソル行 = 空の入力欄。**入力欄はダイアログの外**なので、その上に
        // 番号つきダイアログが在れば拾う（経路 4。#1263）
        return numbered_above_empty_input(lines, scan_from, cursor_row);
    }
    let indent = content_column(lines[cursor_row])?;
    if framed_as_input_box(lines, cursor_row, indent, scan_from, bottom) {
        return None; // 上下を罫線で挟まれている = 入力ボックス（複数行入力）
    }
    let mut rows = vec![cursor_row];
    for i in (scan_from..cursor_row).rev() {
        if aligned_sibling(lines[i], indent) {
            rows.push(i);
        } else {
            break;
        }
    }
    rows.reverse();
    for (i, line) in lines.iter().enumerate().take(bottom).skip(cursor_row + 1) {
        if aligned_sibling(line, indent) {
            rows.push(i);
        } else {
            break;
        }
    }
    // 兄弟が 2 つ未満なら選択肢の並びとみなさない。codex の起動 / 入力待ち画面は
    // 入力行の直下にモデル / cwd のステータス行が同じ桁で 1 行だけ並ぶため、
    // 「兄弟 1 つ」を無条件に許すとそれを選択肢と誤検知する（実採取 fixture で固定）。
    // **確定キーの案内が並びの直後にある**ときだけ兄弟 2 行を認める（#1223。
    // claude 2.x の信頼ダイアログは選択肢 2 つ・番号なしで案内が空行で切れる）
    let corroborated = rows.len() == 2 && confirm_hint_below(lines, rows[1], bottom);
    if rows.len() < 3 && !corroborated {
        return None;
    }
    let options: Vec<ChoiceOption> = rows
        .iter()
        .filter(|&&i| !is_key_hint(lines[i]))
        .map(|&i| {
            let label = cursor_content(lines[i])
                .unwrap_or_else(|| strip_indent(lines[i]))
                .trim()
                .to_string();
            ChoiceOption {
                number: None,
                label_truncated: label_is_truncated(&label),
                label,
                highlighted: i == cursor_row,
            }
        })
        .collect();
    if options.len() < 2 {
        return None;
    }
    let highlighted = options.iter().position(|o| o.highlighted);
    Some(ChoiceList {
        options,
        highlighted,
        numbered: false,
        header: header_block(lines, &rows),
        cursor_row: Some(cursor_row),
    })
}

// --- 番号つき経路の共通部と「入力欄の上のダイアログ」（#1263） ---

/// 画面の `to` 行までにある番号つき選択肢の行（選択カーソル行も含む）。
/// **画面全体が対象**なので、これをそのまま選択肢一覧にしてはいけない（#1293）。
/// 生きたダイアログの並びを採るのは [`numbered_block`]
fn numbered_rows(lines: &[&str], to: usize) -> Vec<usize> {
    (0..to)
        .filter(|&i| {
            let inner = cursor_content(lines[i]).unwrap_or_else(|| strip_indent(lines[i]));
            numbered_choice(inner).is_some()
        })
        .collect()
}

/// その行の番号（番号つき選択肢でなければ `None`）
fn numbered_value(line: &str) -> Option<u32> {
    let inner = cursor_content(line).unwrap_or_else(|| strip_indent(line));
    numbered_choice(inner).map(|(n, _)| n)
}

/// 選択カーソル行 `anchor` と**連続している**番号つき行だけを採る（#1293）。
///
/// 会話ログの番号つき箇条書き（claude は日常的に書く）は字面が選択肢と同じなので、
/// 画面全体から番号つき行を集めると生きたダイアログの一覧へ混ざる。実測（#1293）では
/// 3 択のダイアログが 6 択・番号重複・タイトルが会話本文、という一覧になり、master は
/// その一覧で番号を決めるので誤選択・監査ログの嘘・存在しない番号キーの送信が起きる。
///
/// カーソルの探索は元から末尾 [`SCAN_ROWS`] 行に絞ってあった（= 起点は生きた画面に限る）
/// のに、**収集だけが画面全体**という非対称がそのまま穴だった。ここでは起点から上下へ
/// 次の 2 つを両方満たすあいだだけ伸ばす:
///
/// - あいだに挟まる行が**ダイアログの一部だけ**（[`numbered_gap_ok`]。空行 /
///   説明列の折り返し / 確定キーの案内 / 罫線。実採取の AskUserQuestion は
///   選択肢のあいだに罫線が入る = 跨げないと 5 択が 4 択になる）
/// - 番号が**1 ずつ増える**（会話ログの `1. 2.` がダイアログの `1. 2. 3.` の
///   直上に空行なしで続く形は、あいだが空でも番号の並びで切れる）
fn numbered_block(lines: &[&str], to: usize, anchor: usize) -> Vec<usize> {
    let all = numbered_rows(lines, to);
    if legacy_numbered_block() {
        return all;
    }
    let Some(pos) = all.iter().position(|&i| i == anchor) else {
        return Vec::new();
    };
    let follows = |upper: usize, lower: usize| {
        numbered_gap_ok(lines, upper, lower)
            && matches!(
                (numbered_value(lines[upper]), numbered_value(lines[lower])),
                (Some(a), Some(b)) if a + 1 == b
            )
    };
    let mut above: Vec<usize> = Vec::new();
    let mut cur = anchor;
    for &i in all[..pos].iter().rev() {
        if !follows(i, cur) {
            break;
        }
        above.push(i);
        cur = i;
    }
    above.reverse();
    let mut block = above;
    block.push(anchor);
    let mut cur = anchor;
    for &i in &all[pos + 1..] {
        if !follows(cur, i) {
            break;
        }
        block.push(i);
        cur = i;
    }
    block
}

/// ダイアログの行のあいだに挟まってよい行の種類（#1263 / #1293 の共通判定）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GapLine {
    /// 空行
    Blank,
    /// 説明列 / ラベルの折り返し（中身の桁以上へ字下げされた行）
    Continuation,
    /// 確定キーの案内（`Enter to confirm · Esc to cancel` 等）
    KeyHint,
    /// 罫線（箱の境界。実採取の AskUserQuestion は選択肢のあいだにも引かれる）
    Rule,
    /// ダイアログの一部でない行（会話ログの本文・生成中インジケータ）
    Foreign,
}

/// ダイアログの行のあいだに挟まった 1 行を分類する（#1293 で 1 実装へ寄せた）。
///
/// `content_col` は**上側の行の中身が始まる桁**（[`content_start_column`]）。
/// 折り返しの続きは必ずそこ以上へ字下げされるので、それより浅い非空行は
/// 案内でも罫線でもなければダイアログの外（= 会話ログ）とみなす
fn gap_line_kind(line: &str, content_col: usize) -> GapLine {
    if line.trim().is_empty() {
        return GapLine::Blank;
    }
    if line.chars().take_while(|c| *c == ' ').count() >= content_col {
        return GapLine::Continuation;
    }
    if is_key_hint(line) {
        return GapLine::KeyHint;
    }
    if is_rule_line(line) {
        return GapLine::Rule;
    }
    GapLine::Foreign
}

/// `upper` 行と `lower` 行のあいだが、ダイアログの一部だけでできているか（#1293）
fn numbered_gap_ok(lines: &[&str], upper: usize, lower: usize) -> bool {
    let content_col = content_start_column(lines[upper]).unwrap_or(0);
    lines
        .get(upper + 1..lower)
        .unwrap_or_default()
        .iter()
        .all(|l| gap_line_kind(l, content_col) != GapLine::Foreign)
}

/// `TAKO_1293_LEGACY=1` で #1293 前（番号つき行を画面全体から拾う・本文の境界は
/// 罫線だけ）へ戻す（A/B 用）
fn legacy_numbered_block() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_1293_LEGACY").is_some())
}

/// 番号つきの行から `ChoiceList` を組む（経路 1 と経路 4 の共通部）
fn numbered_list(lines: &[&str], numbered: &[usize], cursor_row: usize) -> ChoiceList {
    let options = numbered_options(lines, numbered, Some(cursor_row));
    let highlighted = options.iter().position(|o| o.highlighted);
    ChoiceList {
        options,
        highlighted,
        numbered: true,
        header: header_block(lines, numbered),
        cursor_row: Some(cursor_row),
    }
}

/// 経路 4: **空の入力欄より上**にある番号つきダイアログを拾う（#1263）。
///
/// claude の auto mode 環境学習の確認（`Teach auto mode about your environment?`）は
/// 番号つき 3 択の**下に空の入力欄を描く**。判定の起点は「最下部の選択カーソル行」
/// なので、旧実装はその入力欄を起点に選び、中身が空なので選択肢なしと結論していた
/// （実測: 同じ 6 行から入力欄を外すと検知できる = 入力欄が原因）。
///
/// 入力欄を跨いで上を見るのは**番号つきに限る**。番号なし経路（開始列の揃った兄弟行）
/// まで開放すると、**複数行の user 発話の継続行**が選択肢と同じ桁に並ぶので会話ログを
/// 選択肢の並びと誤検知する（claude の発話は行頭 `❯` で始まり、継続行が同じ桁へ
/// 字下げされる = 番号なし経路の条件をそのまま満たす）。
///
/// 番号つきでも会話ログは誤検知しうる（**解決済みのダイアログが上流に流れている**形 =
/// #577 の fixture、ユーザーが番号つきリストで指示を書いた形）。字面は生きたダイアログと
/// 同じなので、区別は「**並びと入力欄のあいだがダイアログの一部だけでできているか**」で
/// 付ける（[`dialog_reaches_input_box`]）。
///
/// **確定キーの案内が 1 行も無い / あいだに他の行が挟まるダイアログはこの経路では
/// 拾わない**（= 旧挙動どおり非検知）。誤検知の代償（アイドル画面をダイアログと誤認して
/// 番号キーを送る）のほうが重いので安全側に倒してある
fn numbered_above_empty_input(
    lines: &[&str],
    scan_from: usize,
    input_row: usize,
) -> Option<ChoiceList> {
    if legacy_input_box_below() {
        return None;
    }
    // 入力欄より上で、番号つき選択肢を指している選択カーソル行
    let cursor_row = (scan_from..input_row)
        .rev()
        .find(|&i| cursor_content(lines[i]).is_some_and(|c| numbered_choice(c).is_some()))?;
    let numbered = numbered_block(lines, input_row, cursor_row);
    if numbered.len() < 2 {
        return None;
    }
    if !dialog_reaches_input_box(lines, *numbered.last()?, input_row) {
        return None;
    }
    Some(numbered_list(lines, &numbered, cursor_row))
}

/// 番号つきの並びと**空の入力欄のあいだ**が、ダイアログの一部だけでできているか（#1263）。
///
/// 通すのは 4 種だけ: 空行 / 選択肢の説明列の折り返し（ラベル列以降へ字下げされた行。
/// 実採取: `❯ 1. そのまま変更する（推奨）` の続きが同じ画面で 5 桁 = ラベル列）/
/// 確定キーの案内 / 罫線（入力ボックスの上端）。**そのうえで案内が 1 行は要る**。
///
/// これが「そのダイアログが**生きている**」ことの根拠になる。会話ログへ流れた
/// **解決済みの**ダイアログは字面が同じなので、案内の有無だけでは区別できない。
/// 区別が付くのは**あいだに何が挟まるか**で、実採取では claude の生成中インジケータや
/// 応答本文が入る（`✽ Misting… (1m 4s · ↓ 2.1k tokens)` = #577 の fixture）。
/// 旧実装は「最下部の選択カーソル行」を起点にすることで偶然この区別をしていたので、
/// 入力欄を跨いで上を見るならこの条件を明示的に置く必要がある
fn dialog_reaches_input_box(lines: &[&str], last_row: usize, input_row: usize) -> bool {
    let content_col = content_start_column(lines[last_row]).unwrap_or(0);
    let mut hint = false;
    for line in lines.get(last_row + 1..input_row).unwrap_or_default() {
        match gap_line_kind(line, content_col) {
            // ダイアログの一部でない行が挟まっている（= 会話ログの残骸）
            GapLine::Foreign => return false,
            GapLine::KeyHint => hint = true,
            GapLine::Blank | GapLine::Continuation | GapLine::Rule => {}
        }
    }
    hint
}

/// `TAKO_1263_LEGACY=1` で #1263 前（空の入力欄で打ち切る）へ戻す（A/B 用）
fn legacy_input_box_below() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_1263_LEGACY").is_some())
}

// --- 2 列レイアウトとカーソルなしの検知（#1143） ---

/// TUI がラベルを切り詰めるときに使う省略記号（実採取: `1. Defaul…` / `4. Sonn… ✔`）
const LABEL_ELLIPSIS: char = '…';

/// ラベルが TUI 自身に切り詰められているか。
///
/// claude は 2 列レイアウト（ラベル列 + 説明列）を狭幅へ潰すとき、ラベル列を
/// `…` で打ち切る（`1. Default (recommended)` → `1. Defaul…`）。`✔` のような
/// 印は打ち切りの**後ろ**に付く（`4. Sonn… ✔`）ので、末尾一致ではなく包含で見る。
///
/// 本物のラベルに `…` が入っている TUI があれば過検知になるが、そのときの代償は
/// 「ラベル一致での確定を断って番号を要求する」= 安全側だけ（#1143）
fn label_is_truncated(label: &str) -> bool {
    !legacy_cursorless_dialog() && label.contains(LABEL_ELLIPSIS)
}

/// 番号つき選択肢の行から `ChoiceOption` を組む（経路 1 / 経路 3 の共通部）。
///
/// 2 列レイアウトなら説明列を切り落とす（[`description_column`]）
fn numbered_options(
    lines: &[&str],
    rows: &[usize],
    cursor_row: Option<usize>,
) -> Vec<ChoiceOption> {
    let desc_col = description_column(lines, rows);
    rows.iter()
        .map(|&i| {
            let inner = cursor_content(lines[i]).unwrap_or_else(|| strip_indent(lines[i]));
            let (number, label) = numbered_choice(inner).unwrap_or((0, inner));
            let label = match desc_col.and_then(|c| label_before_column(lines[i], label, c)) {
                Some(cut) => cut,
                None => label,
            };
            let label = label.trim().to_string();
            ChoiceOption {
                number: Some(number),
                label_truncated: label_is_truncated(&label),
                label,
                highlighted: Some(i) == cursor_row,
            }
        })
        .collect()
}

/// 2 列レイアウト（ラベル列 + 説明列）の**説明列の桁**（#1143）。
///
/// 根拠は「ある選択肢の行に 2 桁以上の空白の切れ目があり、その直後の桁へ**次の行が
/// 字下げされている**」こと = 説明列がそこで折り返した証拠。
///
/// ```text
/// 25 桁:      1. Defaul…  Opus     ← 切れ目の後ろは 17 桁目
///                         5        ← 次の行が 17 桁目 = 説明列の折り返し
/// 80 桁:      1. Default (recommended)  Opus 5 with 1M context · Best for everyday,
///                                       complex tasks
/// ```
///
/// **列は 1 ダイアログに 1 つ**。説明が 1 行に収まる選択肢（折り返さないので証拠を
/// 出せない）にも同じ列を当てないと、同じ一覧の中でラベルの切り方が食い違う。
/// 証拠が食い違うときは切らない（`None`）
fn description_column(lines: &[&str], rows: &[usize]) -> Option<usize> {
    if legacy_cursorless_dialog() {
        return None;
    }
    let mut found: Option<usize> = None;
    for &i in rows {
        let Some(col) = column_gap(lines[i]) else {
            continue;
        };
        // 直後の行がその桁へ字下げされた「続き」か（番号つき・カーソル・罫線・
        // キー案内は説明列の折り返しではない）
        let Some(next) = lines.get(i + 1) else {
            continue;
        };
        if next.trim().is_empty()
            || cursor_content(next).is_some()
            || is_key_hint(next)
            || is_rule_line(next.trim_start())
            || numbered_choice(strip_indent(next)).is_some()
        {
            continue;
        }
        if next.chars().take_while(|c| *c == ' ').count() != col {
            continue;
        }
        match found {
            None => found = Some(col),
            Some(c) if c == col => {}
            // 証拠が食い違う = 2 列レイアウトと言い切れない
            Some(_) => return None,
        }
    }
    found
}

/// 行の中身にある最初の「2 桁以上の空白」の**直後の桁**（= 列の切れ目）。
/// 番号つき選択肢の行だけを対象にする（`N. ` の後ろから探す）
fn column_gap(line: &str) -> Option<usize> {
    let inner = cursor_content(line).unwrap_or_else(|| strip_indent(line));
    let (_, label) = numbered_choice(inner)?;
    let gap = label.find("  ")?;
    let after = label[gap..].len() - label[gap..].trim_start_matches(' ').len();
    if label[gap + after..].trim().is_empty() {
        return None; // 切れ目の後ろが空白だけ = 説明列ではない（行末の余白）
    }
    let base = line[..subslice_offset(line, label)].chars().count();
    Some(base + label[..gap + after].chars().count())
}

/// `label`（`line` の部分スライス）のうち、桁 `col` より手前だけを返す。
/// `col` に切れ目が無ければ `None`（この行は説明列を持たない = 切らない）
fn label_before_column<'a>(line: &str, label: &'a str, col: usize) -> Option<&'a str> {
    let base = line[..subslice_offset(line, label)].chars().count();
    let gap = label.find("  ")?;
    let after = label[gap..].len() - label[gap..].trim_start_matches(' ').len();
    if base + label[..gap + after].chars().count() != col {
        return None;
    }
    Some(&label[..gap])
}

/// 経路 3: 選択カーソルが画面に 1 つも描かれていない番号つき一覧（#1143）。
///
/// 狭いペインの `/model` セレクタは、ダイアログがペインより高くなると claude が
/// 箱を**上端から描いて下を切り捨てる**ため、選択カーソルの行もキー案内も画面の
/// 外へ出る（実採取 = 25 桁 × 40 行）。カーソルを anchor にできないので、
/// 番号つき選択肢の連なりそのものを anchor にする。
///
/// 応答本文の箇条書き（`1. …` / `2. …`）や、シェルで開いた Markdown を
/// ダイアログと誤認しないよう、次を**すべて**満たすときだけ採る:
///
/// - 入力欄のプロンプト（`❯` / `›` / `>`）が画面に 1 つも無い
///   （= 入力欄が奪われている。呼び出し元で確認済み）
/// - 同じ桁に揃った番号つき行が 2 つ以上あり、**番号が 1 ずつ増える**
/// - **TUI が描いた一覧**である証拠がある: 連なりの上に**罫線**がある（= 箱の中。
///   Markdown の `---` は [`is_rule_line`] の文字集合に無いので当たらない）か、
///   選択肢が**2 列レイアウト**（[`description_column`]）で描かれている
/// - 連なりのあとに続くのは空行・キー案内・罫線・**より深い字下げ**（説明列の
///   折り返し）だけ = 画面が選択肢一覧の途中で尽きている
fn detect_cursorless_numbered(lines: &[&str], bottom: usize) -> Option<ChoiceList> {
    if legacy_cursorless_dialog() {
        return None;
    }
    let numbered_at = |i: usize| numbered_choice(strip_indent(lines[i])).map(|(n, _)| n);
    let indent_of = |i: usize| lines[i].chars().take_while(|c| *c == ' ').count();

    // 下から連なりを拾う（同じ桁・番号が 1 ずつ減る）
    let last = (0..bottom).rev().find(|&i| numbered_at(i).is_some())?;
    let indent = indent_of(last);
    let mut rows = vec![last];
    let mut want = numbered_at(last)?;
    let mut prev = last;
    for i in (0..last).rev() {
        let Some(n) = numbered_at(i) else {
            continue;
        };
        if indent_of(i) != indent || n + 1 != want {
            break;
        }
        // 経路 1 / 4 と同じ限定（#1293）。あいだに会話ログの本文が挟まる並びは
        // 「同じ一覧」ではない
        if !legacy_numbered_block() && !numbered_gap_ok(lines, i, prev) {
            break;
        }
        rows.push(i);
        want = n;
        prev = i;
    }
    rows.reverse();
    if rows.len() < 2 {
        return None;
    }

    // 「TUI が描いた一覧」の証拠が要る。次のどちらかで足りる:
    //  a) 連なりの上に罫線がある（= ダイアログの箱の中）
    //  b) 選択肢が 2 列レイアウト（ラベル列 + 説明列）で描かれている
    // b を認めるのは、**画面の上が切られていても判定できる**ようにするため。
    // `worker_status` の `recent_output` は末尾 30 行に丸められる（`tail_join`）ので、
    // 25 桁の `/model` では箱の上端が窓の外に落ちる（実測: 罫線は画面の 3 行目 =
    // 窓に入らない）。2 列レイアウトは continuation 行が説明列の桁へぴたりと
    // 揃っていることが根拠なので、本文の箇条書き（折り返しても列は揃わない）や
    // Markdown の番号リストは満たさない
    let first = rows[0];
    let framed = (0..first).any(|i| is_rule_line(lines[i]));
    if !framed && description_column(lines, &rows).is_none() {
        return None;
    }

    // 連なりのあとが「説明列の折り返しだけ」で画面が尽きていること
    let content_col = content_start_column(lines[last]).unwrap_or(indent);
    let tail_is_continuation = ((last + 1)..bottom).all(|i| {
        let line = lines[i];
        line.trim().is_empty()
            || is_key_hint(line)
            || is_rule_line(line.trim_start())
            || indent_of(i) > content_col
    });
    if !tail_is_continuation {
        return None;
    }

    let options = numbered_options(lines, &rows, None);
    Some(ChoiceList {
        options,
        highlighted: None,
        numbered: true,
        header: header_block(lines, &rows),
        cursor_row: None,
    })
}

/// `TAKO_1143_LEGACY=1` で #1143 前（カーソルなし検知・説明列の切り落とし・
/// 切り詰め申告のすべて無し）へ戻す（A/B 用）
fn legacy_cursorless_dialog() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_1143_LEGACY").is_some())
}

// --- 折り返しの結合（#1131） ---

/// 結合する継続行の上限（#1131）。選択肢のラベルは 21 桁でも 3 行までに収まる
const MAX_WRAP_JOIN_LINES: usize = 8;

/// 結合後のラベルの文字数上限（#1131）
const MAX_WRAP_JOIN_CHARS: usize = 600;

/// ダイアログの本文が折り返す位置と画面幅の差（実採取 = 3 桁。
/// 25 桁のペインで本文の列 3 / 選択肢の列 8 の両方がこの値で一致する）
const DIALOG_RIGHT_PAD: usize = 3;

/// その行が**新しい要素**を始めるか（= 直前の行の折り返しの続きではないか）。
///
/// `indent` は直前の論理行の中身が始まる桁。続きは**必ずそれより深く**字下げされる
/// （実採取: 25 桁の確認ダイアログで `   ❯ 1. Yes, switch to` の続きが
/// `        Haiku 4.5` = 番号の後ろの列）。同じ桁の行は**次の選択肢**なので結合しない。
///
/// 加えて、深く字下げされていても次のものは続きにしない:
/// 選択カーソル行 / 番号つき選択肢 / 操作キーの案内 / 罫線
fn starts_new_dialog_block(
    line: &str,
    prev: &str,
    indent: usize,
    numbered: bool,
    usable: usize,
) -> bool {
    let stripped = line.trim_start();
    if stripped.is_empty() {
        return true;
    }
    let leading = line.chars().take_while(|c| *c == ' ').count();
    // 番号つきの選択肢は**ラベルの列**（`1. ` の後ろ）へ折り返し、兄弟の選択肢は
    // それより左の**番号の列**に並ぶ（実採取）。だから同じ桁は続きでよい。
    // 番号なしの並びは兄弟も続きも同じ桁に来て区別できないので、**深い行だけ**を
    // 続きとみなす（`/mcp` のサーバー一覧を 1 個へ畳まないための安全側）
    let deep_enough = if numbered {
        leading >= indent
    } else {
        leading > indent
    };
    if !deep_enough {
        return true;
    }
    // **折り返しの続きは「前の行に載らなかったから次の行に来た」もの**。
    // 前の行に最初の語がまだ入る余地があるなら、それは折り返しではなく
    // 別の要素（実採取: 80 桁の AskUserQuestion は選択肢の下に説明行が
    // ラベルと同じ桁で並ぶ）。桁は表示幅ではなく char 数で数えるので、
    // 全角の行では**控えめ**に見積もる = 結合しない側へ倒れる（安全側）
    let first_word = stripped
        .split(' ')
        .next()
        .unwrap_or(stripped)
        .chars()
        .count();
    // 前の行の長さは**行末の余白を落として**測る（実画面は幅ぶん空白で埋まることがあり、
    // 埋めたままだと「もう入らない」と誤判定して結合しすぎる側へ倒れる）
    if prev.trim_end().chars().count() + 1 + first_word <= usable {
        return true;
    }
    if cursor_content(line).is_some() || is_key_hint(line) || is_rule_line(stripped) {
        return true;
    }
    numbered_choice(strip_indent(line)).is_some()
}

/// 選択肢のラベルが折り返された画面を、1 論理行へ戻す（#1131）。
///
/// # なぜ必要か（実採取 2026-09-04。claude 2.1.258 の同じダイアログを 80 桁と 25 桁で）
///
/// ```text
/// 80 桁:  ❯ 1. Yes, switch to Haiku 4.5
///           2. No, go back
///
/// 25 桁:  ❯ 1. Yes, switch to      ← ラベルがここで折れる
///              Haiku 4.5           ← 続き（ラベルの列へ字下げ）
///           2. No, go back
/// ```
///
/// 構造検知（[`detect_choice_list`]）は 1 行 = 1 選択肢を前提にしているので、
/// 狭いペインではラベルが `Yes, switch to` に切り詰められる。**選択肢は見つかるのに
/// ラベルが違う**ので、`respond` のラベル一致検証と #813 の安全な選択肢の選別
/// （`safe_choice`）が静かに外れる。
///
/// 続きの判定は [`starts_new_dialog_block`]。**同じ桁の行は次の選択肢**なので、
/// 番号なしの並び（`/mcp` のサーバー一覧）を 1 個へ畳んでしまうことはない。
///
/// **2 列レイアウトの選択肢は結合しない**（#1143）。`/model` のように
/// 「ラベル列 + 説明列」で描かれる一覧では、選択肢の下に続くのは**説明列の
/// 折り返し**であってラベルの続きではない。しかも狭幅ではその折り返しが
/// 語境界ではなく**セル単位**で割れる（実採取 = `1M co` / `ntext`）ので、
/// 空白で継ぐと `1M co ntext` という壊れた語がラベルに入る。
/// 列の判定は [`description_column`]
pub fn unwrap_dialog_lines(lines: &[&str]) -> Vec<String> {
    if legacy_wrapped_dialog() {
        return lines.iter().map(|l| (*l).to_string()).collect();
    }
    // 画面の幅（罫線がペイン幅いっぱいに引かれるので最長行がそれに当たる）。
    // 右端の余白ぶんを引いた値が本文の折り返し位置になる（実採取で 3 桁）
    let usable = lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0)
        .saturating_sub(DIALOG_RIGHT_PAD);
    // 2 列レイアウトの説明列（#1143）。画面全体の番号つき行から 1 つだけ決める
    let numbered_rows: Vec<usize> = (0..lines.len())
        .filter(|&i| {
            numbered_choice(cursor_content(lines[i]).unwrap_or_else(|| strip_indent(lines[i])))
                .is_some()
        })
        .collect();
    let desc_col = description_column(lines, &numbered_rows);
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    // 続きを受け付けている論理行（`out` の添字と中身の桁）。None = 受け付けない
    let mut open: Option<(usize, usize, bool)> = None;
    let mut joined = 0usize;
    for line in lines {
        if let Some((row, indent, numbered)) = open {
            if joined < MAX_WRAP_JOIN_LINES
                && !starts_new_dialog_block(line, &out[row], indent, numbered, usable)
            {
                let tail = line.trim();
                let last = &mut out[row];
                if last.chars().count() + tail.chars().count() < MAX_WRAP_JOIN_CHARS {
                    // 行末の余白は情報を持たないので落としてから継ぐ。実画面はペイン幅ぶん
                    // 空白で埋まることがあり、そのまま繋ぐとラベルの途中に空白の塊が残って
                    // `respond` のラベル一致検証が外れる（#1131 が直したかったものと同じ形）
                    last.truncate(last.trim_end().len());
                    last.push(' ');
                    last.push_str(tail);
                    joined += 1;
                    // **行数は保つ**（画面の行番号と 1:1 のまま扱う）。結合した行は
                    // 空行にしておくと、罫線・兄弟行の走査が「そこで切れる」ので
                    // 元の並びと同じ意味になる
                    out.push(String::new());
                    continue;
                }
            }
        }
        joined = 0;
        let is_numbered =
            numbered_choice(cursor_content(line).unwrap_or_else(|| strip_indent(line))).is_some();
        // 2 列レイアウトの選択肢は続きを受け付けない（下に来るのは説明列の折り返し）
        let two_column = is_numbered && desc_col.is_some() && column_gap(line) == desc_col;
        open = if two_column {
            None
        } else {
            content_start_column(line).map(|c| (out.len(), c, is_numbered))
        };
        out.push((*line).to_string());
    }
    out
}

/// その行の「中身が始まる桁」（行頭空白 + 縦罫線 + 選択カーソル + 番号を除いた位置）。
/// 折り返しの続きはこの桁へ字下げされる（実採取）
fn content_start_column(line: &str) -> Option<usize> {
    let stripped = line.trim_start();
    if stripped.is_empty() {
        return None;
    }
    // 罫線と操作キーの案内は**続きを受け付けない**（本文ではないので折り返されない）。
    // 受け付けると、箱の境界である罫線が直後の見出しを吸って `header_block` の
    // 「罫線でそれまでを捨てる」が効かなくなる（実採取 fixture で踏んだ）
    if is_rule_line(stripped) || is_key_hint(line) {
        return None;
    }
    let after_cursor =
        cursor_content(line).map(|inner| line[..subslice_offset(line, inner)].chars().count());
    let base = match after_cursor {
        Some(c) => c,
        None => line.chars().take_while(|c| *c == ' ').count(),
    };
    // 番号つきなら番号とドットの後ろがラベルの列
    let inner = cursor_content(line).unwrap_or_else(|| strip_indent(line));
    match numbered_choice(inner) {
        Some((_, label)) => {
            let consumed = inner.chars().count() - label.chars().count();
            Some(base + consumed)
        }
        None => Some(base),
    }
}

/// `TAKO_1131_LEGACY=1` で #1131 前（折り返しを結合しない）へ戻す（A/B 用）
fn legacy_wrapped_dialog() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_1131_LEGACY").is_some())
}

/// 走査する下端からの行数。ダイアログは選択肢 + 説明で 20 行を超えることがある
/// （実採取: `/mcp` の 12 サーバー一覧で 20 行）ので入力欄判定より広く取る
const SCAN_ROWS: usize = 40;

/// 選択カーソル（`❯` / `›` / `>`）で始まる行なら、カーソルを除いた中身を返す。
///
/// 枠線つきで描かれる TUI（`│ ❯ hello │`）でも拾えるよう行頭の縦罫線は 1 つ剥がす。
/// ASCII の `>` はシェルの PS2・リダイレクトと衝突するため
/// 「`>` 単独 or `> `＋内容」だけをカーソルとみなす（`screen::starts_with_prompt` と同じ規則）
pub fn cursor_content(line: &str) -> Option<&str> {
    let t = strip_indent(line);
    t.strip_prefix('❯')
        .or_else(|| t.strip_prefix('›'))
        .or_else(|| match t.strip_prefix('>') {
            Some(rest) if rest.is_empty() || rest.starts_with(' ') => Some(rest),
            _ => None,
        })
        .map(str::trim)
}

/// 行頭の空白（と縦罫線 1 つ）を落とす
fn strip_indent(line: &str) -> &str {
    let t = line.trim_start();
    t.strip_prefix('│')
        .or_else(|| t.strip_prefix('┃'))
        .unwrap_or(t)
        .trim_start()
}

/// 番号つき選択肢なら `(番号, ラベル)` を返す。
/// 番号は 1 桁に限らない（10 個超の選択肢を持つ TUI のため。#748 のエッジ検証）
pub fn numbered_choice(inner: &str) -> Option<(u32, &str)> {
    let digits = inner.len() - inner.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 {
        return None;
    }
    let rest = &inner[digits..];
    let label = rest.strip_prefix(". ")?;
    inner[..digits].parse().ok().map(|n| (n, label))
}

/// `inner`（`line` の部分スライス）が `line` の何バイト目から始まるかを返す。
///
/// **`line.len() - inner.len()` で求めてはいけない**: [`cursor_content`] は `trim` で
/// **末尾も**削るので、行の右側が空白で埋まっている実画面ではその分だけ後ろへずれる。
/// 全角を含む行ではずれた位置が文字の途中を指し、`line[..offset]` が panic する
/// （#1131 の実装で実際に踏んだ。`origin/main` は同じ入力で落ちない = 回帰だった）
fn subslice_offset(line: &str, inner: &str) -> usize {
    // 部分スライスなのでアドレスの差が開始バイト位置になる（unsafe は要らない）
    inner.as_ptr() as usize - line.as_ptr() as usize
}

/// その行の「中身が始まる桁」（行頭空白 + 縦罫線 + 選択カーソルを除いた位置）。
/// 桁は**表示幅ではなく char 数**で数える（全角の選択肢でも兄弟行の空白は半角なので一致する）
fn content_column(line: &str) -> Option<usize> {
    let inner = cursor_content(line)?;
    Some(line[..subslice_offset(line, inner)].chars().count())
}

/// 選択カーソルのない兄弟行か（中身の開始桁がカーソル行と一致する非空行）
fn aligned_sibling(line: &str, indent: usize) -> bool {
    if line.trim().is_empty() || cursor_content(line).is_some() {
        return false;
    }
    let stripped = line.trim_start();
    let leading = line.len() - stripped.len();
    line[..leading].chars().count() == indent
}

/// 操作キーの案内行か（選択肢一覧から除くため。ラベル付けにのみ使う文言判定で、
/// **ダイアログの存在判定には使わない**）
pub fn is_key_hint(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("↑/↓")
        || t.starts_with("Press enter")
        || t.starts_with("Enter to")
        || t.starts_with("Esc ")
        || t.contains("to navigate")
        || t.contains("Navigate ·")
        || t.contains("enter Confirm")
        || t.contains("to cancel")
}

/// 兄弟 2 行だけの並びを「選択肢」と言い切れる追加の根拠（#1223）。
///
/// claude 2.x の信頼ダイアログは**選択肢 2 つ・番号なし**で、確定キーの案内が
/// **空行で切れて**描かれるので兄弟が 2 行しか繋がらない:
/// ```text
///  > No, exit
///    Yes, I trust this folder
///
///  Enter to confirm · Esc to cancel
/// ```
/// 一方 codex の入力待ち画面は入力行の直下にステータス行が同じ桁で 1 行だけ並ぶ
/// （= これも兄弟 2 行）ので、閾値 3 を単純に緩めるとステータス行を選択肢と誤検知する:
/// ```text
/// › Reply with exactly: PROBE_OK (nothing else)
///   gpt-5.6-sol high · /private/tmp/example/workdir
/// ```
/// 区別は**並びの直後に確定キーの案内があるか**で付く。ダイアログの下には必ず在り
/// （claude `Enter to confirm · Esc to cancel` / codex `Press enter to continue` /
/// agy `↑/↓ Navigate · enter Confirm` の実採取 3 種）、ステータス行の下には無い。
///
/// 案内との間には空行が挟まるので跨いで探し、**案内でない非空行に当たったら偽**
/// （そこで並びが終わっている = 選択肢の下ではない）。存在判定に文言を持ち込むのは
/// **兄弟 2 行のときだけ**で、3 行以上の経路は従来どおり構造だけで決める
fn confirm_hint_below(lines: &[&str], last_row: usize, to: usize) -> bool {
    if legacy_two_row_dialog() {
        return false;
    }
    lines
        .get(last_row + 1..to)
        .unwrap_or_default()
        .iter()
        .find(|l| !l.trim().is_empty())
        .is_some_and(|l| is_key_hint(l))
}

/// `TAKO_1223_LEGACY=1` で **#1223 前の挙動**（兄弟 2 行を一切認めない）へ戻す。
/// 同一バイナリで A/B を取る入口
fn legacy_two_row_dialog() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_1223_LEGACY").is_some())
}

/// 罫線だけでできた行か（`screen::is_frame_line` と同じ役割。ダイアログの箱の境界）。
/// claude は `────` / `╭─╮` のほか `▔▔▔`（`/model` / `/mcp`）も使う（実採取）
pub fn is_rule_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    let mut horizontal = 0usize;
    for c in t.chars() {
        match c {
            '─' | '━' | '═' | '╌' | '╍' | '┄' | '┈' | '⎯' | '▔' | '▁' | '▀' | '▄' => {
                horizontal += 1
            }
            '╭' | '╮' | '╰' | '╯' | '┌' | '┐' | '└' | '┘' | '├' | '┤' | '│' | '┃' | ' ' =>
                {}
            _ => return false,
        }
    }
    horizontal >= 3
}

/// カーソル行（と同じ桁に揃った行の塊）が上下とも罫線で挟まれているか
/// （= claude / codex / agy の入力ボックス）。
///
/// 番号なし経路の誤検知を防ぐ拒否条件。**複数行入力の継続行は選択肢と同じ桁**に
/// 描かれるので、塊の内側を飛ばして外側の境界を見る必要がある（実測: 3 行入力）
fn framed_as_input_box(
    lines: &[&str],
    cursor_row: usize,
    indent: usize,
    from: usize,
    to: usize,
) -> bool {
    let outside = |i: usize| !lines[i].trim().is_empty() && !aligned_sibling(lines[i], indent);
    let above = (from..cursor_row).rev().find(|&i| outside(i));
    let below = (cursor_row + 1..to).find(|&i| outside(i));
    match (above, below) {
        (Some(a), Some(b)) => is_rule_line(lines[a]) && is_rule_line(lines[b]),
        _ => false,
    }
}

/// 選択肢の直前にあるダイアログ本文を集める（= `ChoiceDialog.title` の材料）。
///
/// 画面全体を採ると上端のバナー・cwd・ユーザー発話まで入るため、**罫線に当たったら
/// それまでのブロックを捨てる**（ダイアログの箱の内側だけを残す。#425 の実採取由来）。
/// 空行は境界にしない（claude の実ダイアログは本体に空行を挟む）。
///
/// #1293 で境界を 2 つ足した。罫線を 1 本も引かない画面（auto mode の環境学習の確認）
/// では会話ログがそのまま本文へ入り、master が「（会話本文）を選んだ」という嘘の
/// 監査行を書くところまで繋がっていた:
///
/// - **並びが字下げして描かれているなら、0 桁から始まる非空行は箱の中ではありえない**。
///   実採取の会話ログ（claude の応答・発話・生成中インジケータ）は必ず 0 桁で、
///   ダイアログの箱は 1 桁以上（実採取 1〜5 桁）へ字下げされる。箱ごと 0 桁で描く TUI
///   （agy の信頼ダイアログ）ではこの規則は自動的に無効 = 従来どおり罫線だけが境界
/// - **選択肢に採らなかった番号つき行**（[`numbered_block`] が切った側）も境界にする。
///   別の一覧がそこで終わっている証拠なので、それより上は本文ではない
///
/// #633 で**起点**を足した。上の 3 つはどれも「境界に当たったら捨てる」形なので、
/// 罫線を引かず・箱ごと 0 桁で描く許可ダイアログ（agy の `Requesting permission for:` /
/// claude の `Claude wants to run:`）では捨てる材料が 1 つも無く、直前の発話が
/// そのまま `command` の先頭に残っていた。そこで
/// [`BODY_START_MARKERS`] に一致する行が在れば**そこから**集める
/// （[`body_start_row`]）。起点は選択肢に最も近い一致で、無ければ従来どおり画面の上端。
/// **起点を下げるだけなので結果は必ず従来の本文の接尾辞**（行が増えることはない）
fn header_block(lines: &[&str], option_rows: &[usize]) -> Vec<String> {
    let first_option_row = option_rows.first().copied().unwrap_or(0);
    // 箱の左端（並びの最も浅い行）。0 = 箱ごと 0 桁で描く TUI なので会話ログと区別できない
    let box_indent = if legacy_numbered_block() {
        0
    } else {
        option_rows
            .iter()
            .filter_map(|&i| lines.get(i))
            .map(|l| l.chars().take_while(|c| *c == ' ').count())
            .min()
            .unwrap_or(0)
    };
    let mut block: Vec<String> = Vec::new();
    let start = body_start_row(lines, first_option_row);
    for line in lines.iter().take(first_option_row).skip(start) {
        let t = line.trim();
        if t.is_empty() || is_key_hint(line) {
            continue;
        }
        if !legacy_numbered_block()
            && ((box_indent > 0 && line.chars().take_while(|c| *c == ' ').count() == 0)
                || numbered_value(line).is_some())
        {
            block.clear(); // 会話ログの境界（#1293）
            continue;
        }
        let desc = t
            .trim_start_matches("? ")
            .trim_start_matches("❯ ")
            .trim_start_matches("> ");
        if is_rule_line(desc) {
            block.clear();
        } else if !desc.contains("ctrl+g") {
            block.push(desc.to_string());
        }
    }
    block
}

/// ダイアログ本体の開始マーカー（#633）。**実採取の画面にだけ**由来する。
///
/// - `Claude wants to run:` — claude の Bash 承認（罫線なし・箱は 2 桁 / 選択肢は 0 桁）
/// - `Claude requested permissions to` — claude のファイル承認（先頭に `? ` が付く）
/// - `Requesting permission for:` — agy の許可（箱ごと 0 桁）
/// - `Bash command` — claude 2.x の罫線ボックスの見出し
///
/// 罫線で囲まれた形（claude 2.x）は罫線が起点になるのでマーカーは要らないが、
/// #633 の実測値（`⏺ … Bash command rm -rf build/ Do you want to proceed?`）は
/// **罫線がその画面に無かった**ことを示すので見出しも入れてある。
/// **採取していない見出し（`Edit file` 等）を当て推量で足さない**
/// — 一致しなければ従来のブロック抽出へ落ちるだけで、増やす利得より
/// 「会話文が偶然一致して起点が上へずれる」害のほうが大きい
pub const BODY_START_MARKERS: &[&str] = &[
    "Claude wants to run:",
    "Claude requested permissions to",
    "Requesting permission for:",
    "Bash command",
];

/// 本文を集め始める行（#633）。[`BODY_START_MARKERS`] の**最後の一致**
/// （= 選択肢に最も近い = 会話文の言及より下）。無ければ 0 = 従来どおり画面の上端
fn body_start_row(lines: &[&str], first_option_row: usize) -> usize {
    if legacy_body_anchor() {
        return 0;
    }
    lines
        .iter()
        .take(first_option_row)
        .rposition(|l| is_body_start_marker(l))
        .unwrap_or(0)
}

/// ダイアログ本体の開始行か（#633）。行頭の飾り（`? ` / `❯ ` / `> `）は
/// [`header_block`] と同じ規則で剥がしてから見る
fn is_body_start_marker(line: &str) -> bool {
    let t = line
        .trim()
        .trim_start_matches("? ")
        .trim_start_matches("❯ ")
        .trim_start_matches("> ");
    BODY_START_MARKERS.iter().any(|m| t.starts_with(m))
}

/// `TAKO_633_LEGACY=1` で #633 前（本文は常に画面の上端から集める）へ戻す（A/B 用）
fn legacy_body_anchor() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_633_LEGACY").is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &str) -> Vec<&str> {
        text.lines().collect()
    }

    // --- 実採取画面（claude v2.1.220 / 2026-08-04。個人情報はサニタイズ済み） ---

    /// Bash ツールの承認ダイアログ（`perl` は許可リスト外）
    const PERMISSION: &str = r#"❯ Bash ツールで「perl -e "print 42"」をそのまま実行して。他のツールは使わないで

  Running 1 shell command…
  ⎿  $ perl -e "print 42"

────────────────────────────────────────────────────────────────────────
 Bash command

   perl -e "print 42"
   perl で 42 を出力

 This command requires approval

 Do you want to proceed?
 ❯ 1. Yes
   2. Yes, and don’t ask again for: perl *
   3. No

 Esc to cancel · Tab to amend · ctrl+e to explain"#;

    /// `/model` のモデル選択（`▔▔▔` 罫線 + 深いインデント + `✔` つき既定）
    const MODEL_SELECT: &str = r#"✻ Churned for 9s

❯ Bash ツールで du -sh /tmp/work を実行して

▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔
   Select model
   Switch between Claude models. Your pick becomes the default for new sessions.

     1. Default (recommended)  Opus 5 with 1M context
     2. Opus (1M context)      Opus 5 with 1M context
   ❯ 3. Fable ✔                Fable 5 · Most capable for your hardest tasks
     4. Sonnet                 Sonnet 5 · Efficient for routine tasks
     5. Haiku                  Haiku 4.5 · Fastest for quick answers
     6. Opus 4.6 (1M)          Opus 4.6 with 1M context

   ● High effort (default) ←/→ to adjust

   Enter to set as default · s to use this session only · Esc to cancel"#;

    /// plan モードの実行確認（`Would you like to proceed?` = permission と別文言）
    const PLAN_CONFIRM: &str = r#"  ────────────────────────────────────────────────────────────────────
   Claude has written up a plan and is ready to execute. Would you like to proceed?

   ❯ 1. Yes, and use auto mode
     2. Yes, manually approve edits
     3. Tell Claude what to change
        shift+tab to approve with this feedback

   ctrl+g to edit in VS Code · ~/.claude/plans/example.md"#;

    /// `/mcp` のサーバー一覧（**番号なし** + セクション見出し混在 + 12 項目）
    const MCP_LIST: &str = r#"▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔
   Manage MCP servers
   12 servers

     User MCPs (<home>/.claude.json)
   ❯ context7 · ✔ connected · 2 tools
     coplay-mcp · ✔ connected · 98 tools
     filesystem · ✔ connected · 14 tools
     github · ✔ connected · 26 tools

     claude.ai
     claude.ai Canva · ✔ connected · 32 tools

   https://code.claude.com/docs/en/mcp for help
   ↑/↓ to navigate · Enter to confirm · Esc to cancel"#;

    /// AskUserQuestion の質問（全角ラベル + 説明の継続行 + 罫線をまたぐ選択肢）
    const ASK_USER_QUESTION: &str = r#"✻ Churned for 53s

❯ AskUserQuestion ツールで質問して
  ⎿  Invalid tool parameters
────────────────────────────────────────────────────────────────────────
 ☐ 進め方

この作業をどう進めますか?

❯ 1. そのまま変更する（推奨）
     println!("hi") を println!("hello") に 1 行編集し、コンパイル確認まで行う
  2. 変更してコミットまで
     編集後に /commit スキルでコミットを作成する
  3. 今回は見送る
     変更せず現状のままにする
  4. Type something.
────────────────────────────────────────────────────────────────────────
  5. Chat about this

Enter to select · ↑/↓ to navigate · Esc to cancel"#;

    /// 通常の入力欄（空 + dim プレースホルダ）
    const INPUT_READY: &str = r#"⏺ 直しました

────────────────────────────────────────────────────────────────────────
❯ Try "refactor <filepath>"
────────────────────────────────────────────────────────────────────────
  ctx   5% ░░░░░░░░░░"#;

    /// 複数行入力（継続行のインデントが選択肢と同じ 2 桁 = 番号なし経路の誤検知源）
    const INPUT_MULTILINE: &str = r#"────────────────────────────────────────────────────────────────────────
❯ 1 行目のテキスト
  2 行目のテキスト
  3 行目のテキスト
────────────────────────────────────────────────────────────────────────
  ctx  12% █░░░░░░░░░"#;

    /// codex の起動画面（入力行の直下にモデル / cwd のステータス行が同じ桁で並ぶ。
    /// 「兄弟 1 つ」を選択肢とみなすとこれが誤検知される = 閾値 3 行の根拠）
    const CODEX_READY: &str = r#"╭─────────────────────────────────────────────────╮
│ >_ OpenAI Codex (v0.144.1)                      │
│                                                 │
│ model:     gpt-5.6-sol high   /model to change  │
╰─────────────────────────────────────────────────╯

  Tip: When the composer is empty, press Esc to step back and edit your last message; Enter
  confirms.


› Summarize recent commits

  gpt-5.6-sol high · /private/tmp/example/workdir"#;

    /// claude 2.x の信頼ダイアログ（**番号なし・選択肢 2 つ**の実採取。#1223）。
    /// 確定キーの案内が**空行で切れる**ので兄弟が 2 行しか繋がらない = 閾値 3 で落ちていた形
    const CLAUDE_TRUST_NO_NUMBER: &str = r#" Quick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open source
 project, or work from your team). If not, take a moment to review what's in this folder first.

 Claude Code'll be able to read, edit, and execute files here.

 Security guide

 > No, exit
   Yes, I trust this folder

 Enter to confirm · Esc to cancel"#;

    /// agy の信頼ダイアログ（**番号なし**の実採取。番号なし経路が拾うべき下限）
    const AGY_TRUST: &str = r#"Accessing workspace:
/private/tmp/example/workdir
Do you trust the contents of this project?
Antigravity CLI requires permission to read, edit, and execute files here.
> Yes, I trust this folder
  No, exit
  ↑/↓ Navigate · enter Confirm
                                                    Claude Opus 4.6 (Thinking)"#;

    /// agy の空入力欄（罫線で挟まれた `>` 単独行）
    const AGY_READY: &str = r#"  Antigravity CLI 1.1.0
  Claude Opus 4.6 (Thinking)
  /private/tmp/example/workdir
────────────────────────────────────────────────────
>
────────────────────────────────────────────────────
? for shortcuts                                     Claude Opus 4.6 (Thinking)"#;

    /// 応答本文の箇条書き（入力欄は空 = ダイアログではない。#577）
    const QUESTION_IN_BODY: &str = r#"⏺ 移行スクリプトの準備ができました。

  Do you want to proceed?
  1. Yes, run the migration now
  2. No, stop here

────────────────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────────────────
  claude-opus-5 · ctx 23%"#;

    // --- #1263: auto mode の環境学習の確認（**番号つき 3 択の下に空の入力欄がある**形） ---
    //
    // ダイアログの 6 行は 2026-09-09 11:50 の worker ペインの実採取（Issue #1263 本文）。
    // 下の入力欄はその画面に在ったもので、枠の形は同じファイルの実採取 `INPUT_READY` に
    // 合わせてある（枠の有無で結果が変わらないことは `TEACH_AUTO_MODE_BARE` で固定する）
    const TEACH_AUTO_MODE: &str = r#"  Teach auto mode about your environment?

  Auto mode works better when it knows your environment. Takes about a minute.

  ❯ 1. Yes
    2. Not now
    3. Don't show again

  Enter to confirm · Esc to cancel
────────────────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────────────────
  claude-opus-5 · ctx 12%"#;

    /// 同じダイアログで**入力欄に枠が無い**形（枠の描き方に判定を依存させないための対）
    const TEACH_AUTO_MODE_BARE: &str = r#"  Teach auto mode about your environment?

  Auto mode works better when it knows your environment. Takes about a minute.

  ❯ 1. Yes
    2. Not now
    3. Don't show again

  Enter to confirm · Esc to cancel

❯"#;

    /// 会話ログに残った**番号つきの user 発話** + 空の入力欄（#1263 の拒否条件）。
    /// 番号つきの並びとカーソルは揃っているが、並びと入力欄のあいだに claude の応答本文が
    /// 挟まる（確定キーの案内も無い）
    const NUMBERED_USER_MESSAGE: &str = r#"❯ 1. まず A をやって
  2. 次に B をやって

⏺ 了解しました。順に進めます。

────────────────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────────────────
  claude-opus-5 · ctx 8%"#;

    /// **複数行の user 発話**（継続行が選択肢と同じ桁）+ 空の入力欄。
    /// 経路 4 を番号なしへ広げると誤検知する形なので、案内行まで足した最悪ケースで固定する
    /// （案内行は合成。実画面の発話の下に案内は出ない）
    const MULTILINE_USER_MESSAGE: &str = r#"❯ 1 行目のお願い
  2 行目のお願い
  3 行目のお願い

  Enter to confirm · Esc to cancel

────────────────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────────────────"#;

    // --- #1143: 狭いペインの `/model` セレクタ（実採取 2026-09-06。claude 2.1.258 を
    // 隔離した tmux セッションで幅・高さだけ変えて採った 3 枚。cwd 行はサニタイズ済み） ---
    //
    // 25 桁 × 40 行: ダイアログがペインより高いので claude は箱を上端から描いて
    // 下を切り捨てる。**選択カーソル `❯` も操作キーの案内も画面に無い**（Issue #1143 の再現形）。
    // ラベルは claude 自身が `…` で切り詰め、説明列はセル単位で割れる（`1M co` / `ntext`）
    const MODEL_25_NO_CURSOR: &str = r#"           k

▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔
   Select model
   Switch between
   Claude models. Your
   pick becomes the
   default for new
   sessions. For
   other/previous
   model names,
   specify with
   --model.

     1. Defaul…  Opus
                 5
                 with
                 1M co
                 ntext
                 ·
                 Best
                 for
                 every
                 day,
                 compl
                 ex
                 tasks
     2. Opus (…  Opus
                 5
                 with
                 1M co
                 ntext
                 ·
                 Best
                 for
                 every
                 day,
                 compl
                 ex
                 tasks ↓"#;

    // 25 桁 × 70 行: **同じ幅・同じダイアログ**で高さだけ足したもの。
    // カーソル（`❯ 4.`）が画面に入るので経路 1 で読める = 幅由来の切り詰めだけを切り分けられる
    const MODEL_25_CURSOR: &str = r#"           k

▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔
   Select model
   Switch between
   Claude models. Your
   pick becomes the
   default for new
   sessions. For
   other/previous
   model names,
   specify with
   --model.

     1. Defaul…  Opus
                 5
                 with
                 1M co
                 ntext
                 ·
                 Best
                 for
                 every
                 day,
                 compl
                 ex
                 tasks
     2. Opus (…  Opus
                 5
                 with
                 1M co
                 ntext
                 ·
                 Best
                 for
                 every
                 day,
                 compl
                 ex
                 tasks
     3. Fable    Fable
                 5.1
                 ·
                 Most
                 capab
                 le
                 for
                 your
                 harde
                 st
                 and
                 longe
                 st-ru
                 nning

                 tasks
   ❯ 4. Sonn… ✔  Sonne
                 t 5 ·
                 Effi
                 cient
                 for
                 routi
                 ne
                 tasks
     5. Haiku    Haiku
                 4.5
                 · Fas
                 test
                 for
                 quick ↓"#;

    // 80 桁 × 40 行: ラベルは切り詰められないが、説明列は 2 行へ折り返す。
    // 「説明列をラベルへ混ぜない」ことを 25 桁と同じ規則で確かめるための対
    const MODEL_80_WRAPPED: &str = r#"
 ▐▛███▛█   Claude Code v2.1.258
▝▜██████▀  Sonnet 5 with xhigh effort · Claude Max
  ▝▝ ▝▝    /…/example/workdir


❯ /model
  ⎿  Kept model as Sonnet 5

❯ /model
  ⎿  Kept model as Sonnet 5











▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔
   Select model
   Switch between Claude models. Your pick becomes the default for new
   sessions. For other/previous model names, specify with --model.

     1. Default (recommended)  Opus 5 with 1M context · Best for everyday,
                               complex tasks
     2. Opus (1M context)      Opus 5 with 1M context · Best for everyday,
                               complex tasks
     3. Fable                  Fable 5.1 · Most capable for your hardest and
                               longest-running tasks
   ❯ 4. Sonnet ✔               Sonnet 5 · Efficient for routine tasks
     5. Haiku                  Haiku 4.5 · Fastest for quick answers
     6. Opus 4.6 (1M)          Opus 4.6 with 1M context

   ◉ xHigh effort ←/→ to adjust

   Enter to set as default · s to use this session only · Esc to cancel"#;

    #[test]
    fn 実採取のpermissionダイアログを番号つきで検知する() {
        let list = detect_choice_list(&rows(PERMISSION)).expect("検知される");
        assert!(list.numbered);
        assert_eq!(list.options.len(), 3);
        assert_eq!(list.options[0].label, "Yes");
        assert_eq!(list.options[0].number, Some(1));
        assert_eq!(list.options[2].label, "No");
        assert_eq!(list.highlighted, Some(0));
        // 罫線より上のユーザー発話・ツールログは本文に混ぜない
        let header = list.header.join(" ");
        assert!(header.contains("perl -e \"print 42\""), "{header}");
        assert!(!header.contains("Bash ツールで"), "{header}");
    }

    #[test]
    fn 実採取のモデル選択を検知する() {
        let list = detect_choice_list(&rows(MODEL_SELECT)).expect("検知される");
        assert!(list.numbered);
        assert_eq!(list.options.len(), 6);
        assert_eq!(list.highlighted, Some(2), "❯ が 3. を指している");
        assert!(list.options[2].label.starts_with("Fable ✔"));
        assert!(list.header.join(" ").contains("Select model"));
    }

    #[test]
    fn 実採取のplan確認を検知する() {
        let list = detect_choice_list(&rows(PLAN_CONFIRM)).expect("検知される");
        assert_eq!(list.options.len(), 3);
        assert_eq!(list.highlighted, Some(0));
        // 選択肢の説明の継続行（shift+tab …）は選択肢に混ぜない
        assert!(list.options.iter().all(|o| !o.label.contains("shift+tab")));
    }

    #[test]
    fn 実採取のmcp一覧を番号なしで検知する() {
        let list = detect_choice_list(&rows(MCP_LIST)).expect("検知される");
        assert!(!list.numbered, "番号キーでは選べない");
        // ハイライトは context7。セクション見出しは構造上区別できないので混ざるが、
        // ハイライト位置とラベルが取れることが respond の前提
        assert_eq!(
            list.highlighted_label(),
            Some("context7 · ✔ connected · 2 tools")
        );
        assert!(list.options.len() >= 4, "{:?}", list.options);
        // キー案内行は選択肢に含めない
        assert!(list
            .options
            .iter()
            .all(|o| !o.label.contains("to navigate")));
    }

    #[test]
    fn 実採取のaskuserquestionを検知する() {
        let list = detect_choice_list(&rows(ASK_USER_QUESTION)).expect("検知される");
        assert!(list.numbered);
        // 罫線をまたぐ「5. Chat about this」まで拾う
        assert_eq!(list.options.len(), 5);
        assert_eq!(list.options[0].label, "そのまま変更する（推奨）");
        assert_eq!(list.options[4].label, "Chat about this");
        assert_eq!(list.highlighted, Some(0));
    }

    #[test]
    fn 通常の入力欄はダイアログと判定しない() {
        for (name, s) in [
            ("空 + プレースホルダ", INPUT_READY),
            ("複数行入力", INPUT_MULTILINE),
            ("本文の箇条書き", QUESTION_IN_BODY),
            ("codex 起動画面", CODEX_READY),
            ("agy 空入力欄", AGY_READY),
            // #1263: 入力欄の上を見る経路を足しても会話ログは拾わないこと
            ("番号つきの user 発話", NUMBERED_USER_MESSAGE),
            ("複数行の user 発話", MULTILINE_USER_MESSAGE),
        ] {
            assert!(
                detect_choice_list(&rows(s)).is_none(),
                "{name} は入力欄（ダイアログではない）"
            );
        }
    }

    #[test]
    fn 番号なしのagy信頼ダイアログを検知する() {
        let list = detect_choice_list(&rows(AGY_TRUST)).expect("検知される");
        assert!(!list.numbered);
        // キー案内行を落として選択肢 2 つ
        assert_eq!(list.options.len(), 2, "{:?}", list.options);
        assert_eq!(list.options[0].label, "Yes, I trust this folder");
        assert_eq!(list.options[1].label, "No, exit");
        assert_eq!(list.highlighted, Some(0));
    }

    #[test]
    fn 番号なし二択のclaude信頼ダイアログを検知する() {
        let list = detect_choice_list(&rows(CLAUDE_TRUST_NO_NUMBER)).expect("検知される");
        assert!(!list.numbered, "番号なし経路で拾う");
        assert_eq!(list.options.len(), 2, "{:?}", list.options);
        assert_eq!(list.options[0].label, "No, exit");
        assert_eq!(list.options[1].label, "Yes, I trust this folder");
        assert_eq!(list.highlighted, Some(0), "既定は No, exit");
        assert!(
            list.header.iter().any(|h| h.contains("Quick safety check")),
            "本文が header に入る: {:?}",
            list.header
        );
    }

    #[test]
    fn 番号なし二択はカーソルが二つ目でも位置が取れる() {
        let screen =
            "   No, exit\n > Yes, I trust this folder\n\n Enter to confirm · Esc to cancel";
        let list = detect_choice_list(&rows(screen)).expect("検知される");
        assert_eq!(list.options.len(), 2, "{:?}", list.options);
        assert_eq!(list.highlighted, Some(1));
        assert_eq!(list.highlighted_label(), Some("Yes, I trust this folder"));
    }

    #[test]
    fn 兄弟二行でも確定キーの案内が無ければ選択肢とみなさない() {
        // codex の入力待ち画面（入力行の直下にステータス行が同じ桁で 1 行だけ並ぶ）
        let codex_pending = "• DONE_PROBE\n────────────────────────────────────────────────────\n› Reply with exactly: PROBE_OK (nothing else)\n  gpt-5.6-sol high · /private/tmp/example/workdir";
        assert!(
            detect_choice_list(&rows(codex_pending)).is_none(),
            "ステータス行を選択肢と誤検知してはいけない"
        );
        // 案内でない非空行が並びの直後に来る形も選択肢ではない
        let with_status = " > No, exit\n   Yes, I trust this folder\n\n gpt-5.6-sol high · /private/tmp/example/workdir";
        assert!(
            detect_choice_list(&rows(with_status)).is_none(),
            "直後の非空行が案内でなければ根拠にならない"
        );
    }

    #[test]
    fn 番号なし二択の規則は三択以上と狭幅の並びを壊さない() {
        // 案内が空行の先にある 3 択（従来どおり構造だけで通る経路）
        let three = " Pick one?\n\n > A\n   B\n   C\n\n Enter to confirm · Esc to cancel";
        let list = detect_choice_list(&rows(three)).expect("検知される");
        assert_eq!(list.options.len(), 3, "{:?}", list.options);
        assert_eq!(list.highlighted, Some(0));

        // 狭幅で選択肢が折り返した信頼ダイアログ（合成 30 桁）。番号なしの並びは
        // 兄弟と続きが同じ桁に来るので**ラベルは行ごとに割れる**（`/mcp` の一覧を
        // 1 個へ畳まないための既存の安全側。#1131 のコメント参照）。それでも
        // ハイライト位置と「trust を含む選択肢が 1 つ」は保たれるので、
        // `respond` のラベル一致検証は成立する
        let narrow = " Quick safety check: Is this\n a project you created or\n one you trust?\n\n > No, exit\n   Yes, I trust this\n   folder\n\n Enter to confirm · Esc to\n cancel";
        let list = detect_choice_list(&rows(narrow)).expect("狭幅でも検知される");
        assert_eq!(list.highlighted, Some(0));
        assert_eq!(list.highlighted_label(), Some("No, exit"));
        assert_eq!(
            list.options
                .iter()
                .filter(|o| o.label.contains("trust"))
                .count(),
            1,
            "trust を含む選択肢は 1 つ = ラベル指定が曖昧にならない: {:?}",
            list.options
        );
    }

    #[test]
    fn 番号は多桁も解釈する() {
        assert_eq!(numbered_choice("1. Yes"), Some((1, "Yes")));
        assert_eq!(numbered_choice("12. 十二番目"), Some((12, "十二番目")));
        assert_eq!(numbered_choice("1.Yes"), None, "ドット直後の空白が必要");
        assert_eq!(numbered_choice("Yes"), None);
        assert_eq!(numbered_choice("1"), None);
    }

    #[test]
    fn 選択肢が十個を超えても全件と位置が取れる() {
        // 実 claude は 9 個までしか番号を振らない画面が多いので合成（多桁の回帰固定）
        let mut screen = vec![" どれにしますか?".to_string()];
        for n in 1..=12 {
            screen.push(if n == 11 {
                format!(" ❯ {n}. 選択肢 {n}")
            } else {
                format!("   {n}. 選択肢 {n}")
            });
        }
        screen.push(" Press enter to confirm".to_string());
        let refs: Vec<&str> = screen.iter().map(String::as_str).collect();
        let list = detect_choice_list(&refs).expect("検知される");
        assert_eq!(list.options.len(), 12);
        assert_eq!(list.options[11].number, Some(12));
        assert_eq!(list.highlighted, Some(10));
        assert_eq!(list.highlighted_label(), Some("選択肢 11"));
    }

    #[test]
    fn 全角混じりの選択肢もラベルが崩れない() {
        let refs = [
            " 全角の質問です？",
            " ❯ 1. 変更する（推奨）",
            "   2. 変更しない",
        ];
        let list = detect_choice_list(&refs).expect("検知される");
        assert_eq!(list.options[0].label, "変更する（推奨）");
        assert_eq!(list.options[1].label, "変更しない");
    }

    #[test]
    fn 番号なし経路は罫線に挟まれた入力欄を棄却する() {
        // INPUT_MULTILINE は「兄弟行が 2 つ」あるので、罫線の拒否条件が無いと誤検知する
        let lines = rows(INPUT_MULTILINE);
        let bottom = lines.iter().rposition(|l| !l.trim().is_empty()).unwrap() + 1;
        let cursor = (0..bottom)
            .rev()
            .find(|&i| cursor_content(lines[i]).is_some())
            .unwrap();
        let indent = content_column(lines[cursor]).unwrap();
        assert!(framed_as_input_box(&lines, cursor, indent, 0, bottom));
        assert!(detect_choice_list(&lines).is_none());
    }

    #[test]
    fn 罫線判定は実採取の罫線文字を網羅する() {
        assert!(is_rule_line("────────"));
        assert!(is_rule_line("▔▔▔▔▔▔▔▔"), "/model / /mcp の上罫線");
        assert!(is_rule_line("╭──────╮"));
        assert!(is_rule_line("╌╌╌╌╌╌╌╌"));
        assert!(!is_rule_line(""));
        assert!(!is_rule_line("Select model"));
        assert!(!is_rule_line("❯ 1. Yes"));
    }

    // --- #1131: ペイン幅で折り返された選択肢 ---

    /// 実採取（2026-09-04。claude 2.1.258 の**同じ確認ダイアログ**を 80 桁と 25 桁で
    /// 採った対。`tmux resize-window` で幅だけ変えているので中身は完全に同じ）。
    ///
    /// 25 桁ではラベルが折り返され、続きが**ラベルの列**（`❯ 1. ` の後ろ = 8 桁）へ
    /// 字下げされる。#1131 前の構造検知は 1 行 = 1 選択肢を前提にしていたので、
    /// 選択肢は見つかるのにラベルが `Yes, switch to` に切り詰められていた
    const CONFIRM_80: &str = r#"▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔
   Switch model?
   Your next response will be slower and use more tokens

   This conversation is cached for the current model. Switching to Haiku 4.5
   means the full history gets re-read on your next message.

   ❯ 1. Yes, switch to Haiku 4.5
     2. No, go back"#;

    const CONFIRM_25: &str = r#"▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔
   Switch model?
   Your next response
   will be slower and
   use more tokens

   This conversation
   is cached for the
   current model.
   Switching to Haiku
   4.5 means the full
   history gets
   re-read on your
   next message.

   ❯ 1. Yes, switch to
        Haiku 4.5
     2. No, go back"#;

    #[test]
    fn issue1131_実採取の確認ダイアログは幅が違っても同じ選択肢になる() {
        let wide = detect_choice_list(&rows(CONFIRM_80)).expect("80 桁で検知される");
        let narrow = detect_choice_list(&rows(CONFIRM_25)).expect("25 桁で検知されない");
        assert!(wide.numbered && narrow.numbered);
        assert_eq!(
            narrow.options.len(),
            wide.options.len(),
            "選択肢の数が幅で変わる: {:?}",
            narrow.options
        );
        for (n, w) in narrow.options.iter().zip(&wide.options) {
            assert_eq!(n.number, w.number, "番号が違う");
            assert_eq!(
                n.label, w.label,
                "ラベルが幅で変わる（折り返しが結合できていない）"
            );
            assert_eq!(n.highlighted, w.highlighted, "ハイライトが違う");
        }
        assert_eq!(narrow.highlighted, wide.highlighted);
        // 本文（title に使う）も同じ 1 本になる
        assert_eq!(
            narrow.header.join(" "),
            wide.header.join(" "),
            "本文が幅で変わる"
        );
    }

    /// claude TUI の折り返しを再現する（#1131）。
    /// 続きは**中身の列**へ字下げされ、右端に 3 桁の余白が残る
    /// （実採取の [`CONFIRM_25`] が本文の列 3 と選択肢の列 8 の両方でこの形に一致する）
    fn wrap_dialog_line(prefix: &str, text: &str, cols: usize) -> Vec<String> {
        let col = prefix.chars().count();
        let avail = cols.saturating_sub(col + 3).max(1);
        let cont = " ".repeat(col);
        let mut out: Vec<String> = Vec::new();
        let mut cur = String::new();
        for w in text.split(' ') {
            let next = if cur.is_empty() {
                w.to_string()
            } else {
                format!("{cur} {w}")
            };
            if next.chars().count() > avail && !cur.is_empty() {
                out.push(format!(
                    "{}{cur}",
                    if out.is_empty() { prefix } else { &cont }
                ));
                cur = w.to_string();
            } else {
                cur = next;
            }
        }
        out.push(format!(
            "{}{cur}",
            if out.is_empty() { prefix } else { &cont }
        ));
        out
    }

    #[test]
    fn issue1131_折り返しの生成器が実採取と一致する() {
        // 生成器を実採取へ固定する。ここがずれたら他の fixture も信用できない
        assert_eq!(
            wrap_dialog_line("   ❯ 1. ", "Yes, switch to Haiku 4.5", 25),
            vec![
                "   ❯ 1. Yes, switch to".to_string(),
                "        Haiku 4.5".into()
            ],
            "選択肢の列（8 桁）"
        );
        assert_eq!(
            wrap_dialog_line(
                "   ",
                "Your next response will be slower and use more tokens",
                25
            ),
            vec![
                "   Your next response".to_string(),
                "   will be slower and".into(),
                "   use more tokens".into(),
            ],
            "本文の列（3 桁）"
        );
    }

    #[test]
    fn issue1131_permissionダイアログを25桁でも同じ選択肢で読む() {
        // #748 の実採取（80 桁）の選択肢だけを 25 桁へ折り返す。
        // ラベルが 2 行に割れても番号・ラベル・ハイライトが変わらないこと
        let mut lines: Vec<String> = vec![
            "▔".repeat(25),
            "   Bash command".into(),
            "".into(),
            "   perl -e \"print 42\"".into(),
            "".into(),
            "   Do you want to".into(),
            "   proceed?".into(),
            "".into(),
        ];
        for (i, (prefix, label)) in [
            ("   ❯ 1. ", "Yes"),
            ("     2. ", "Yes, and don't ask again"),
            ("     3. ", "No, and tell Claude what to do differently"),
        ]
        .into_iter()
        .enumerate()
        {
            let _ = i;
            lines.extend(wrap_dialog_line(prefix, label, 25));
        }
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let list = detect_choice_list(&refs).expect("25 桁の permission が検知されない");
        assert!(list.numbered);
        assert_eq!(list.options.len(), 3, "{:?}", list.options);
        assert_eq!(list.options[0].label, "Yes");
        assert_eq!(list.options[1].label, "Yes, and don't ask again");
        assert_eq!(
            list.options[2].label, "No, and tell Claude what to do differently",
            "折り返したラベルが 1 本へ戻っていない"
        );
        assert_eq!(list.highlighted, Some(0));
        assert!(list.header.join(" ").contains("Do you want to proceed?"));
    }

    #[test]
    fn issue1131_上限ダイアログを25桁でも同じ選択肢で読む() {
        // #748 / #813 の実文言。ここが読めないと「解除まで待つ」の自動確定が効かない
        let mut lines: Vec<String> = vec!["▔".repeat(25)];
        lines.extend(wrap_dialog_line("   ", "What do you want to do?", 25));
        lines.push("".into());
        for (prefix, label) in [
            ("   ❯ 1. ", "Stop and wait for limit to reset"),
            (
                "     2. ",
                "Upgrade to Max 20x for higher session limits every month",
            ),
            ("     3. ", "Continue with usage credits"),
        ] {
            lines.extend(wrap_dialog_line(prefix, label, 25));
        }
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let list = detect_choice_list(&refs).expect("25 桁の上限ダイアログが検知されない");
        assert_eq!(list.options.len(), 3, "{:?}", list.options);
        assert_eq!(list.options[0].label, "Stop and wait for limit to reset");
        assert_eq!(
            list.options[1].label,
            "Upgrade to Max 20x for higher session limits every month"
        );
        assert!(list.header.join(" ").contains("What do you want to do?"));
    }

    #[test]
    fn issue1131_番号なしの並びは畳まない() {
        // 兄弟の選択肢は**同じ桁**に並ぶので、続きと取り違えて 1 個へ畳んではいけない
        let lines = rows(
            "▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔\n   Select a server\n\n   ❯ tako\n     linear\n     playwright",
        );
        let list = detect_choice_list(&lines).expect("検知される");
        assert!(!list.numbered);
        assert_eq!(list.options.len(), 3, "{:?}", list.options);
        assert_eq!(list.options[0].label, "tako");
        assert_eq!(list.options[2].label, "playwright");
    }

    #[test]
    fn issue1131_行末が空白で埋まった全角のダイアログでも落ちない() {
        // 実画面の行はペイン幅ぶん空白で埋まりうる。`cursor_content` は末尾も trim するので
        // `line.len() - inner.len()` で開始位置を求めると全角文字の途中を指して panic し、
        // 余白を落とさずに継ぐとラベルの途中へ空白の塊が残る（どちらも実装中に踏んだ。
        // `origin/main` は同じ入力で落ちない = #1131 が持ち込みかけた回帰）
        let pad = |s: &str| {
            let mut t = s.to_string();
            while t.chars().count() < 25 {
                t.push(' ');
            }
            t
        };
        let owned = [
            "▔".repeat(25),
            pad("   このダイアログは？"),
            String::new(),
            pad("   ❯ 1. はい、ぜんぶ進めますよ"),
            pad("        つづき"),
            pad("     2. いいえ"),
        ];
        let lines: Vec<&str> = owned.iter().map(String::as_str).collect();
        let list = detect_choice_list(&lines).expect("末尾空白つきでも検知される");
        assert!(list.numbered);
        assert_eq!(list.options.len(), 2, "{:?}", list.options);
        // 折り返しの続きは結合され、繋ぎ目に余白の塊が残らない
        assert_eq!(list.options[0].label, "はい、ぜんぶ進めますよ つづき");
        assert_eq!(list.options[1].label, "いいえ");
        for o in &list.options {
            assert!(
                !o.label.contains("  "),
                "ラベルに余白の塊が残っている: {:?}",
                o.label
            );
        }
    }

    // --- #1143: カーソルが画面外の番号つき一覧 / 切り詰められたラベル / 説明列 ---

    /// 選択肢を `番号 -> (ラベル, 切り詰めか)` にして比べやすくする
    fn options_of(list: &ChoiceList) -> Vec<(Option<u32>, String, bool)> {
        list.options
            .iter()
            .map(|o| (o.number, o.label.clone(), o.label_truncated))
            .collect()
    }

    #[test]
    fn issue1143_カーソルが画面外の狭いモデルセレクタを検知する() {
        let lines = rows(MODEL_25_NO_CURSOR);
        assert!(
            lines.iter().all(|l| cursor_content(l).is_none()),
            "この fixture の前提は「選択カーソルが 1 つも無い」こと"
        );
        let list = detect_choice_list(&lines).expect("25 桁の /model セレクタが検知されない");
        assert!(list.numbered, "番号キーで確定できる一覧として組む");
        assert_eq!(list.cursor_row, None, "カーソルは画面に無い");
        assert_eq!(list.highlighted, None, "位置を知らないなら知らないと言う");
        assert_eq!(
            options_of(&list),
            vec![
                (Some(1), "Defaul…".to_string(), true),
                (Some(2), "Opus (…".to_string(), true),
            ],
            "画面に出ている 2 件がそのまま番号つきで取れる"
        );
        assert!(
            list.header.iter().any(|h| h == "Select model"),
            "本文が取れない: {:?}",
            list.header
        );
    }

    #[test]
    fn issue1143_切り詰められたラベルは復元せずに申告する() {
        let list = detect_choice_list(&rows(MODEL_25_NO_CURSOR)).expect("検知される");
        for o in &list.options {
            assert!(
                o.label_truncated,
                "{:?} が切り詰め申告されていない",
                o.label
            );
            // 画面に残っていない文字を補完しない（`Default (recommended)` を作らない）
            assert!(
                o.label.contains('…') && o.label.chars().count() <= 8,
                "ラベルを推測で伸ばしている: {:?}",
                o.label
            );
        }
    }

    #[test]
    fn issue1143_同じ一覧はカーソルの見え方が変わってもラベルが変わらない() {
        // 25 桁 × 40 行（カーソル画面外）と 25 桁 × 70 行（カーソルあり）は
        // **同じダイアログ**。高さだけが違うので、見えている選択肢のラベルは一致する
        let short = detect_choice_list(&rows(MODEL_25_NO_CURSOR)).expect("40 行で検知される");
        let tall = detect_choice_list(&rows(MODEL_25_CURSOR)).expect("70 行で検知される");
        assert_eq!(tall.highlighted, Some(3), "70 行では ❯ が 4. を指す");
        assert_eq!(
            options_of(&short),
            options_of(&tall)[..short.options.len()].to_vec(),
            "高さでラベルが変わる"
        );
        assert_eq!(
            options_of(&tall)[3],
            (Some(4), "Sonn… ✔".to_string(), true),
            "切り詰めの後ろに付く印（✔）まで残す"
        );
    }

    #[test]
    fn issue1143_説明列はラベルへ混ぜない() {
        // 80 桁でも説明が 2 行へ折り返す。折り返しをラベルへ継ぐと、狭幅では
        // セル単位で割れた語（`1M co` + `ntext`）が入って壊れる
        let wide = detect_choice_list(&rows(MODEL_80_WRAPPED)).expect("80 桁で検知される");
        let labels: Vec<&str> = wide.options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "Default (recommended)",
                "Opus (1M context)",
                "Fable",
                "Sonnet ✔",
                "Haiku",
                "Opus 4.6 (1M)",
            ],
            "説明列がラベルに混ざっている"
        );
        assert!(
            wide.options.iter().all(|o| !o.label_truncated),
            "80 桁では切り詰められない"
        );
        // 狭いほうも「壊れた語」を持たない
        let narrow = detect_choice_list(&rows(MODEL_25_CURSOR)).expect("検知される");
        for o in &narrow.options {
            assert!(
                !o.label.contains("1M co") && !o.label.contains("capab"),
                "説明列の破片がラベルに残っている: {:?}",
                o.label
            );
        }
        // 説明が 1 行に収まる選択肢（折り返しの証拠を出せない）にも同じ列を当てる
        assert_eq!(
            detect_choice_list(&rows(MODEL_SELECT))
                .expect("検知される")
                .options[2]
                .label,
            "Fable ✔                Fable 5 · Most capable for your hardest tasks",
            "折り返しが 1 つも無い画面では列を決められないので #748 のまま"
        );
    }

    #[test]
    fn issue1143_画面の上が切られていても2列レイアウトなら読める() {
        // `worker_status` の `recent_output` は末尾 30 行に丸められる（`tail_join`）。
        // 25 桁の `/model` では箱の上端（罫線）が画面の 3 行目にあるので窓に入らない。
        // 実採取（25 桁 × 44 行）の末尾 30 行をそのまま材料にする
        let full = rows(MODEL_25_NO_CURSOR);
        let tail: Vec<&str> = full[full.len() - 30..].to_vec();
        assert!(
            !tail.iter().any(|l| is_rule_line(l)),
            "この検証の前提は「窓に罫線が無い」こと"
        );
        let list = detect_choice_list(&tail).expect("末尾 30 行だけでも検知される");
        assert!(list.numbered);
        assert_eq!(list.cursor_row, None);
        assert_eq!(
            list.options
                .iter()
                .map(|o| (o.number, o.label.clone()))
                .collect::<Vec<_>>(),
            vec![
                (Some(1), "Defaul…".to_string()),
                (Some(2), "Opus (…".to_string()),
            ],
            "窓に入っている選択肢だけを組む"
        );
        assert!(list.options.iter().all(|o| o.label_truncated));
    }

    #[test]
    fn issue1143_カーソルなし経路は本文の箇条書きを拾わない() {
        // 罫線の無い番号つき並び（応答本文・シェルで開いた Markdown）は
        // 入力欄が見えていなくてもダイアログではない
        let plain = [
            "  手順は次のとおりです。",
            "",
            "  1. 依存を入れる",
            "  2. ビルドする",
            "  3. テストを走らせる",
        ];
        assert!(
            detect_choice_list(&plain).is_none(),
            "罫線の無い箇条書きをダイアログと誤認した"
        );
        // 番号が連番でない（本文の引用など）
        let gaps = [
            "▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔",
            "  1. 最初の項目",
            "  7. 飛んだ項目",
        ];
        assert!(
            detect_choice_list(&gaps).is_none(),
            "番号が 1 ずつ増えない並びを一覧と誤認した"
        );
        // 折り返しつきの箇条書き（罫線が窓の外に落ちた形を模す）。
        // 続きの行は在るが、選択肢の行に「2 桁以上の空白の切れ目」が無いので
        // 2 列レイアウトの証拠にならない = 採らない
        let wrapped_prose = [
            "  1. 依存を入れる。これは長くて",
            "     次の行へ折り返す説明",
            "  2. ビルドする。こちらも長くて",
            "     折り返す",
        ];
        assert!(
            detect_choice_list(&wrapped_prose).is_none(),
            "折り返しただけの本文の箇条書きを一覧と誤認した"
        );
        // 一覧の後ろに本文が続く = 画面が一覧の途中で尽きていない
        let trailing = [
            "▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔",
            "  1. 最初の項目",
            "  2. 次の項目",
            "以上が手順です。",
        ];
        assert!(
            detect_choice_list(&trailing).is_none(),
            "一覧の後ろに本文がある画面をダイアログと誤認した"
        );
    }

    #[test]
    fn issue1143_入力欄が見えているなら従来どおりカーソルを起点にする() {
        // 経路 3 は「カーソルが 1 つも無い」ときだけ。空の入力欄が見えている
        // 画面（#577 の本文の箇条書き）は今までどおり棄却する
        assert!(detect_choice_list(&rows(QUESTION_IN_BODY)).is_none());
    }

    #[test]
    fn issue1143_列の切れ目は行末の余白と区別する() {
        // 行末がペイン幅ぶん空白で埋まっているだけの行を「2 列」と読むと
        // ラベルが空になる
        assert_eq!(column_gap("   ❯ 1. Yes           "), None);
        assert_eq!(column_gap("     1. Defaul…  Opus"), Some(17));
        assert_eq!(column_gap("     2. 番号なしの行"), None);
        assert_eq!(column_gap("   説明列を持たない本文  つづき"), None);
    }

    #[test]
    fn 部分スライスの開始位置は末尾のtrimに影響されない() {
        // `subslice_offset` の不変条件。ここが `line.len() - inner.len()` に戻ると
        // 上のダイアログが panic する
        let line = "   ❯ 1. はい、進める          ";
        let inner = cursor_content(line).expect("カーソル行");
        assert_eq!(inner, "1. はい、進める", "末尾は trim される");
        let offset = subslice_offset(line, inner);
        assert_eq!(&line[..offset], "   ❯ ");
        assert!(
            offset < line.len() - inner.len(),
            "末尾に空白があるぶん素朴な引き算より小さい: offset={offset} naive={}",
            line.len() - inner.len()
        );
    }

    // --- #1236: 矢印移動で確定する画面の「次の 1 手」 ---

    #[test]
    fn issue1236_ハイライトが目標に乗っていれば確定する() {
        assert_eq!(confirm_step(2, Some(1), 1), ConfirmStep::Confirm);
        // 選択肢が 1 つでも同じ（旧 claude の番号つき信頼ダイアログは既定が承諾側）
        assert_eq!(confirm_step(1, Some(0), 0), ConfirmStep::Confirm);
    }

    #[test]
    fn issue1236_目標までの向きと歩数を返す() {
        // 既定が `No, exit`（添字 0）で承諾側が下（添字 1）= claude 2.x の信頼ダイアログ
        assert_eq!(
            confirm_step(2, Some(0), 1),
            ConfirmStep::Move {
                key: "Down",
                steps: 1
            }
        );
        // 承諾側が上にある形（agy）でも同じ規則で戻れる
        assert_eq!(
            confirm_step(4, Some(3), 0),
            ConfirmStep::Move {
                key: "Up",
                steps: 3
            }
        );
    }

    #[test]
    fn issue1236_ハイライトが読めなければ確定しない() {
        // カーソルが画面外（#1143 の形）= どこが選ばれているか分からない
        assert_eq!(
            confirm_step(2, None, 1),
            ConfirmStep::Blocked(ConfirmBlock::NoHighlight)
        );
        // 目標が範囲外 / ハイライトが範囲外（採取のあいだに画面が書き換わった）
        assert_eq!(
            confirm_step(2, Some(0), 2),
            ConfirmStep::Blocked(ConfirmBlock::OutOfRange)
        );
        assert_eq!(
            confirm_step(2, Some(5), 1),
            ConfirmStep::Blocked(ConfirmBlock::OutOfRange)
        );
        assert_eq!(
            confirm_step(0, None, 0),
            ConfirmStep::Blocked(ConfirmBlock::OutOfRange)
        );
    }

    #[test]
    fn issue1236_実採取の番号なし信頼ダイアログから承諾側へ動ける() {
        // 画面 → 構造 → 次の 1 手 が繋がっていることの確認（fixture は #1223 由来）
        let lines = [
            " Quick safety check: Is this a project you created or one you trust?",
            "",
            " > No, exit",
            "   Yes, I trust this folder",
            "",
            " Enter to confirm · Esc to cancel",
        ];
        let list = detect_choice_list(&lines).expect("番号なし 2 択を検知する");
        assert_eq!(list.highlighted, Some(0), "既定は拒否側");
        let target = list
            .options
            .iter()
            .position(|o| o.label.starts_with("Yes,"))
            .expect("承諾側がある");
        assert_eq!(
            confirm_step(list.options.len(), list.highlighted, target),
            ConfirmStep::Move {
                key: "Down",
                steps: 1
            }
        );
    }

    // --- #1263: 空の入力欄より上のダイアログ ---

    /// **解決済みの** permission ダイアログが会話ログへ流れた画面（#577 の実採取型）。
    /// 字面は生きたダイアログと同じで確定キーの案内まで在るが、claude は生成中で
    /// 入力欄は健在 = ダイアログではない。区別は「あいだに生成中インジケータが挟まる」こと
    const RESOLVED_DIALOG_IN_LOG: &str = r#"⏺ Running 1 shell command…
────────────────────────────────────────────────
 Bash command
   for i in {1..12}; do echo $i; sleep 1; done; echo done
 Contains brace_expression
 Do you want to proceed?
 ❯ 1. Yes
   2. Yes, and don't ask again for echo commands
   3. No, and tell Claude what to do differently (esc)
 Esc to cancel · Tab to amend · ctrl+e to explain
✽ Misting… (1m 4s · ↓ 2.1k tokens)
────────────────────────────────────────────────
❯
────────────────────────────────────────────────
  ctx 23%"#;

    #[test]
    fn issue1263_会話ログへ流れた解決済みダイアログは拾わない() {
        assert!(
            detect_choice_list(&rows(RESOLVED_DIALOG_IN_LOG)).is_none(),
            "生成中インジケータを挟んだ残骸をダイアログと誤検知してはいけない（#577）"
        );
        // インジケータを取り除くと「入力欄まで途切れず届く」形になる = 判定材料はそこだけ
        let live = RESOLVED_DIALOG_IN_LOG.replace("✽ Misting… (1m 4s · ↓ 2.1k tokens)\n", "");
        let list = detect_choice_list(&rows(&live)).expect("生きたダイアログなら検知される");
        assert_eq!(list.options.len(), 3);
    }

    #[test]
    fn issue1263_実採取のauto_mode確認を検知する() {
        let list = detect_choice_list(&rows(TEACH_AUTO_MODE)).expect("検知される");
        assert!(list.numbered, "番号つき = 番号キーで確定できる");
        assert_eq!(list.highlighted, Some(0), "既定のハイライトは 1. Yes");
        assert_eq!(
            list.options
                .iter()
                .map(|o| (o.number, o.label.clone()))
                .collect::<Vec<_>>(),
            vec![
                (Some(1), "Yes".to_string()),
                (Some(2), "Not now".to_string()),
                (Some(3), "Don't show again".to_string()),
            ]
        );
        assert!(
            list.cursor_row.is_some(),
            "選択カーソルは画面に在る（#1143 の画面外とは別の形）"
        );
        assert!(
            list.header.iter().any(|h| h.contains("Teach auto mode")),
            "ダイアログ本文が header に入る: {:?}",
            list.header
        );
    }

    #[test]
    fn issue1263_入力欄の枠の有無で結果は変わらない() {
        let framed = detect_choice_list(&rows(TEACH_AUTO_MODE)).expect("枠つきで検知される");
        let bare = detect_choice_list(&rows(TEACH_AUTO_MODE_BARE)).expect("枠なしでも検知される");
        assert_eq!(options_of(&framed), options_of(&bare));
        assert_eq!(framed.highlighted, bare.highlighted);
    }

    #[test]
    fn issue1263_入力欄が無い形でも同じ選択肢になる() {
        // 入力欄を外した 6 行は元から検知できていた（= 入力欄が原因だった証拠）
        let without = TEACH_AUTO_MODE
            .lines()
            .take_while(|l| !l.starts_with('\u{2500}'))
            .collect::<Vec<_>>()
            .join("\n");
        let a = detect_choice_list(&rows(&without)).expect("入力欄なしで検知される");
        let b = detect_choice_list(&rows(TEACH_AUTO_MODE)).expect("入力欄ありでも検知される");
        assert_eq!(options_of(&a), options_of(&b));
        assert_eq!(a.highlighted, b.highlighted);
    }

    #[test]
    fn issue1263_ハイライトが二番目でも位置が取れる() {
        let moved = TEACH_AUTO_MODE
            .replace("  \u{276f} 1. Yes", "    1. Yes")
            .replace("    2. Not now", "  \u{276f} 2. Not now");
        let list = detect_choice_list(&rows(&moved)).expect("検知される");
        assert_eq!(list.highlighted, Some(1));
        assert_eq!(list.options.len(), 3);
    }

    #[test]
    fn issue1263_確定キーの案内が無い並びは拾わない() {
        // 案内行だけを落とす（他は実採取のまま）= 安全側の非検知を固定する
        let without_hint = TEACH_AUTO_MODE.replace("  Enter to confirm \u{b7} Esc to cancel\n", "");
        assert!(
            detect_choice_list(&rows(&without_hint)).is_none(),
            "案内が無ければ入力欄の上は見ない（誤検知より非検知を採る）"
        );
    }

    // --- #1293: 会話ログの番号つき箇条書きがダイアログの選択肢へ混ざる ---

    /// Issue #1293 の再現画面。claude は番号つきリストを日常的に書くので、
    /// **会話の途中で出るダイアログ**の上には番号つき箇条書きが残っている。
    /// 経路 4（#1263。空の入力欄の上を見る）はまさにこの形の画面を対象にしたので
    /// 露出が大きい。origin/main = 9ab6c26 の実測は options=6・番号重複・
    /// header が会話本文（`⏺ 直し方の候補は 3 つあります。`）
    const LOG_LIST_ABOVE_DIALOG: &str = r#"⏺ 直し方の候補は 3 つあります。

  1. 待ちを状態待ちへ寄せる
  2. 番犬テストを足す
  3. 何もしない

⏺ 1 を採ります。

  Teach auto mode about your environment?

  Auto mode works better when it knows your environment. Takes about a minute.

  ❯ 1. Yes
    2. Not now
    3. Don't show again

  Enter to confirm · Esc to cancel
────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────
  [model placeholder] · ctx 12%"#;

    /// 同じ形の**経路 1**（入力欄が描かれない全画面ダイアログ）。
    /// 起点が変わっても同じ限定が効くことを固定する
    const LOG_LIST_ABOVE_DIALOG_NO_INPUT: &str = r#"⏺ 直し方の候補は 3 つあります。

  1. 待ちを状態待ちへ寄せる
  2. 番犬テストを足す
  3. 何もしない

⏺ 1 を採ります。

  Teach auto mode about your environment?

  Auto mode works better when it knows your environment. Takes about a minute.

  ❯ 1. Yes
    2. Not now
    3. Don't show again

  Enter to confirm · Esc to cancel"#;

    /// 期待する 3 択（両経路で同じ）
    fn expect_teach_three(list: &ChoiceList, name: &str) {
        assert!(list.numbered, "{name}: 番号キーで確定できる一覧として組む");
        assert_eq!(
            list.options
                .iter()
                .map(|o| (o.number, o.label.clone()))
                .collect::<Vec<_>>(),
            vec![
                (Some(1), "Yes".to_string()),
                (Some(2), "Not now".to_string()),
                (Some(3), "Don't show again".to_string()),
            ],
            "{name}: 会話ログの箇条書きが混ざっている"
        );
        assert_eq!(list.highlighted, Some(0), "{name}: 既定は 1. Yes");
        let header = list.header.join(" ");
        assert!(
            header.contains("Teach auto mode about your environment?"),
            "{name}: ダイアログの説明文が本文に入っていない: {header:?}"
        );
        for log in ["直し方の候補", "待ちを状態待ちへ寄せる", "1 を採ります"]
        {
            assert!(
                !header.contains(log),
                "{name}: 会話本文が title に混ざっている（{log}）: {header:?}"
            );
        }
    }

    #[test]
    fn issue1293_会話ログの箇条書きは選択肢にもtitleにも混ざらない() {
        let list = detect_choice_list(&rows(LOG_LIST_ABOVE_DIALOG)).expect("経路 4 で検知される");
        expect_teach_three(&list, "経路 4");
        assert!(list.cursor_row.is_some(), "選択カーソルは画面に在る");
    }

    #[test]
    fn issue1293_入力欄が無い経路1でも同じ三択になる() {
        let list =
            detect_choice_list(&rows(LOG_LIST_ABOVE_DIALOG_NO_INPUT)).expect("経路 1 で検知される");
        expect_teach_three(&list, "経路 1");
        // 入力欄の有無で結果が変わらない
        let with_input = detect_choice_list(&rows(LOG_LIST_ABOVE_DIALOG)).expect("検知される");
        assert_eq!(options_of(&list), options_of(&with_input));
        assert_eq!(list.header, with_input.header);
    }

    #[test]
    fn issue1293_箇条書きがダイアログの直上に空行なしで続いても切れる() {
        // あいだに何も挟まらない = 「連続していない」根拠が番号の並びだけになる形
        let text = concat!(
            "⏺ 手順です。\n",
            "  1. まず A をやって\n",
            "  2. 次に B をやって\n",
            "  ❯ 1. Yes\n",
            "    2. Not now\n",
            "    3. Don't show again\n",
            "\n",
            "  Enter to confirm · Esc to cancel"
        );
        let list = detect_choice_list(&rows(text)).expect("検知される");
        assert_eq!(
            list.options.len(),
            3,
            "番号が 1 へ戻るところで切れていない: {:?}",
            list.options
        );
        assert_eq!(list.options[0].label, "Yes");
        assert_eq!(list.highlighted, Some(0));
    }

    #[test]
    fn issue1293_丸括弧の箇条書きは番号つき行として拾わない() {
        // `1)` 形式（`numbered_choice` は `N. ` だけを番号とみなす）は選択肢に
        // ならないのはもちろん、**並びをそこで切る側**（`GapLine::Foreign`）にも効く。
        // 実採取どおり claude の応答ブロックは 0 桁の marker で始まる形にする
        let text = concat!(
            "⏺ 直し方の候補は 2 つあります。\n",
            "  1) 待ちを状態待ちへ寄せる\n",
            "  2) 番犬テストを足す\n",
            "\n",
            "⏺ 1 を採ります。\n",
            "\n",
            "  Teach auto mode about your environment?\n",
            "\n",
            "  ❯ 1. Yes\n",
            "    2. Not now\n",
            "\n",
            "  Enter to confirm · Esc to cancel"
        );
        let list = detect_choice_list(&rows(text)).expect("検知される");
        assert_eq!(list.options.len(), 2, "{:?}", list.options);
        assert_eq!(list.options[0].label, "Yes");
        assert_eq!(list.options[1].label, "Not now");
        let header = list.header.join(" ");
        assert!(header.contains("Teach auto mode"), "{header:?}");
        assert!(!header.contains("待ちを状態待ちへ寄せる"), "{header:?}");
    }

    #[test]
    fn issue1293_箱と同じ桁の会話ログは本文に残る_限界の固定() {
        // **境界は 0 桁の marker と番号つき行だけ**。claude の応答ブロックは必ず
        // 0 桁の marker で始まるので実画面では効くが、その marker が画面の上へ
        // 流れ切った形では箱と同じ桁の残骸が本文に残る。選択肢は汚れない
        // （= master が選ぶ番号は正しい）ことだけを保証する範囲だと明示しておく
        let text = concat!(
            "  1) 会話ログの残骸\n",
            "\n",
            "  Teach auto mode about your environment?\n",
            "\n",
            "  ❯ 1. Yes\n",
            "    2. Not now\n",
            "\n",
            "  Enter to confirm · Esc to cancel"
        );
        let list = detect_choice_list(&rows(text)).expect("検知される");
        assert_eq!(
            list.options.len(),
            2,
            "選択肢は汚れない: {:?}",
            list.options
        );
        assert!(
            list.header.join(" ").contains("会話ログの残骸"),
            "この限界が消えたら（別の境界を足したら）ここを更新する: {:?}",
            list.header
        );
    }

    #[test]
    fn issue1293_二桁番号の一覧は会話ログの上でも全件取れる() {
        // 10 個以上の選択肢（2 桁）+ その上に会話ログの箇条書き。
        // 「連続の打ち切り」が多桁の一覧を途中で切らないこと
        let mut screen = vec![
            "⏺ 候補を挙げます。".to_string(),
            String::new(),
            "  1. 会話ログの一項目".into(),
            "  2. 会話ログの二項目".into(),
            String::new(),
            "⏺ ではモデルを選んでください。".into(),
            String::new(),
            "  どれにしますか?".into(),
        ];
        for n in 1..=12 {
            screen.push(if n == 11 {
                format!("  ❯ {n}. 選択肢 {n}")
            } else {
                format!("    {n}. 選択肢 {n}")
            });
        }
        screen.push("  Press enter to confirm".into());
        let refs: Vec<&str> = screen.iter().map(String::as_str).collect();
        let list = detect_choice_list(&refs).expect("検知される");
        assert_eq!(list.options.len(), 12, "{:?}", list.options);
        assert_eq!(list.options[0].number, Some(1));
        assert_eq!(list.options[11].number, Some(12));
        assert_eq!(list.highlighted, Some(10));
        assert_eq!(list.highlighted_label(), Some("選択肢 11"));
        let header = list.header.join(" ");
        assert!(header.contains("どれにしますか?"), "{header:?}");
        assert!(!header.contains("会話ログの一項目"), "{header:?}");
        assert!(!header.contains("候補を挙げます"), "{header:?}");
    }

    #[test]
    fn issue1293_番号ブロックの限定は既存の実採取ダイアログを変えない() {
        // 選択肢のあいだに罫線が入る AskUserQuestion（5 択）・説明列つきの /model（6 択）・
        // 説明行つきの plan 確認（3 択）は「連続」の側に残る
        for (name, text, want) in [
            ("permission", PERMISSION, 3),
            ("model select", MODEL_SELECT, 6),
            ("plan confirm", PLAN_CONFIRM, 3),
            ("AskUserQuestion", ASK_USER_QUESTION, 5),
            ("auto mode", TEACH_AUTO_MODE, 3),
        ] {
            let list = detect_choice_list(&rows(text)).unwrap_or_else(|| panic!("{name} 未検知"));
            assert_eq!(list.options.len(), want, "{name}: {:?}", list.options);
        }
    }

    #[test]
    fn issue1263_選択肢ごとに説明行が付いても案内へ辿り着ける() {
        // AskUserQuestion 型（各選択肢の下に説明行）+ 空の入力欄
        let text = concat!(
            "  質問です\n",
            "\n",
            "  ❯ 1. はい\n",
            "       そのまま進める\n",
            "    2. いいえ\n",
            "       やめる\n",
            "\n",
            "  Enter to confirm · Esc to cancel\n",
            "\n",
            "❯"
        );
        let list = detect_choice_list(&rows(text)).expect("説明行を跨いで案内を見つける");
        assert_eq!(list.options.len(), 2);
        assert_eq!(list.highlighted, Some(0));
    }

    // --- #633: 本文の起点（承認カードの command に直前の発話が混ざる） ---

    #[test]
    fn issue633_起点は選択肢に最も近いマーカー() {
        // 会話文が同じ語に触れていても、起点は**下**の本物の見出しになる
        let lines = rows(concat!(
            "⏺ Bash command の承認を求めます。\n",
            "  少し時間がかかります。\n",
            "\n",
            " Bash command\n",
            "   rm -rf build/\n",
            " Do you want to proceed?\n"
        ));
        assert_eq!(body_start_row(&lines, lines.len()), 3);
        // マーカーが 1 つも無ければ従来どおり画面の上端から
        assert_eq!(body_start_row(&rows("⏺ ふつうの応答\n  つづき\n"), 2), 0);
    }

    #[test]
    fn issue633_行頭の飾りを剥がしてから見る() {
        // claude のファイル承認は `? ` 付き・agy の選択カーソルは `> ` 付き
        assert!(is_body_start_marker(
            "? Claude requested permissions to write to .../main.aux"
        ));
        assert!(is_body_start_marker("  Claude wants to run:"));
        assert!(is_body_start_marker("Requesting permission for:"));
        assert!(is_body_start_marker(" Bash command"));
        // 語の途中や別の文言には当たらない
        assert!(!is_body_start_marker("  the Bash command failed"));
        assert!(!is_body_start_marker(" Select model"));
    }

    #[test]
    fn issue633_起点は既存の実採取ダイアログの本文を変えない() {
        // 罫線ボックス（PERMISSION）は見出しが起点、罫線を引かない画面
        // （TEACH_AUTO_MODE / MODEL_SELECT / MCP_LIST）はマーカー不一致で従来どおり
        for (name, text, want) in [
            ("permission", PERMISSION, "Bash command"),
            (
                "auto mode",
                TEACH_AUTO_MODE,
                "Teach auto mode about your environment?",
            ),
            ("model select", MODEL_SELECT, "Select model"),
            ("mcp list", MCP_LIST, "Manage MCP servers"),
        ] {
            let list = detect_choice_list(&rows(text)).unwrap_or_else(|| panic!("{name} 未検知"));
            assert_eq!(
                list.header.first().map(String::as_str),
                Some(want),
                "{name}: 本文の先頭が変わった: {:?}",
                list.header
            );
        }
    }
}
