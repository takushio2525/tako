//! AI コマンド提案カード（FR-2.22 / Issue #666、表示位置は #681）
//!
//! AI（master / solo / worker）が「ユーザーに実行してほしいコマンド」を渡すと、
//! 対象ペインの**生成時点のターミナル内容にアンカーした**ネイティブカードとして出す。
//!
//! **なぜ画面から拾わないか**: claude 等の TUI は会話をペイン幅に合わせて物理改行して
//! 描画するため、画面キャプチャからコマンドを復元するとその改行を除去できない
//! （どこが折り返しでどこが本物の改行かの区別が原理的に付かない）。ここに保管するのは
//! **AI が渡した論理文字列そのもの**なので、カードの表示を折り返しても、コピーと実行に
//! 使う文字列は常に完全なまま保たれる。
//!
//! GPUI 非依存。保管と操作（追加・解決・破棄）はこのモジュールに閉じ、
//! CLI / MCP / UI は `tako-control::dispatch` の 1 経路からここを叩く。
//! カードは揮発（セッション内使い捨て）で、レイアウト永続化の対象外。

use std::fmt;

use crate::PaneId;

/// 1 ペインに同時に置けるカード数の上限。超えたら**古いものから捨てる**
/// （AI が提示を重ねても画面がカードで埋まらないようにする）
pub const MAX_CARDS_PER_PANE: usize = 3;

/// 1 カードに載せられるコマンド数の上限
pub const MAX_COMMANDS_PER_CARD: usize = 10;

/// 1 コマンドの最大長（バイト）。長いワンライナーを想定して広く取るが、
/// 画面を壊すような巨大文字列は受け付けない
pub const MAX_COMMAND_BYTES: usize = 4096;

/// 説明ラベルの最大長（文字数）。カードの見出し 1〜2 行に収まる範囲
pub const MAX_LABEL_CHARS: usize = 200;

/// カード ID（プロセス生存期間中ユニーク）。ペイン ID との取り違えを型で防ぐ
/// （#428 の「ターゲット式取り違え」と同じ趣旨）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommandCardId(u64);

impl CommandCardId {
    pub fn as_u64(self) -> u64 {
        self.0
    }

    /// ワイヤ値（CLI / MCP の引数）から復元する
    pub fn from_raw(id: u64) -> Self {
        Self(id)
    }
}

impl fmt::Display for CommandCardId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// カードの操作エラー。文言は dispatch のエラーメッセージへそのまま流す
/// （UI 表示ではないので日本語で可。`.agent/conventions.md`「UI 文字列の i18n」）
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CommandCardError {
    #[error("コマンドが 1 件も指定されていない")]
    NoCommands,
    #[error("空のコマンドは提示できない（{index} 件目）")]
    EmptyCommand { index: usize },
    #[error("コマンドが長すぎる（{index} 件目: {len} バイト > 上限 {max}）")]
    CommandTooLong {
        index: usize,
        len: usize,
        max: usize,
    },
    #[error("コマンドに制御文字が含まれている（{index} 件目。改行とタブ以外は不可）")]
    ControlCharacter { index: usize },
    #[error("1 カードに載せられるコマンドは {max} 件まで（指定: {len} 件）")]
    TooManyCommands { len: usize, max: usize },
    #[error("ラベルが長すぎる（{len} 文字 > 上限 {max}）")]
    LabelTooLong { len: usize, max: usize },
    #[error("カードが見つからない（id={id}）")]
    CardNotFound { id: u64 },
    #[error("コマンド番号が範囲外（指定: {index}、このカードは 1〜{len}）")]
    IndexOutOfRange { index: usize, len: usize },
}

/// 表示中のコマンド提案カード 1 枚
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandCard {
    id: CommandCardId,
    pane: PaneId,
    /// 提示するコマンド（**論理文字列**。改行を含む複数行コマンドも 1 要素として持つ）
    commands: Vec<String>,
    /// 何のためのコマンドかの説明（任意）
    label: Option<String>,
}

impl CommandCard {
    pub fn id(&self) -> CommandCardId {
        self.id
    }

    pub fn pane(&self) -> PaneId {
        self.pane
    }

    pub fn commands(&self) -> &[String] {
        &self.commands
    }

    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    /// 1 始まりの番号でコマンドを取り出す（CLI / MCP / UI の番号は人間向けの 1 始まり）
    pub fn command(&self, index: usize) -> Result<&str, CommandCardError> {
        if index == 0 || index > self.commands.len() {
            return Err(CommandCardError::IndexOutOfRange {
                index,
                len: self.commands.len(),
            });
        }
        Ok(&self.commands[index - 1])
    }
}

/// ペイン単位のカード保管庫。GUI（tako-app）が 1 個だけ持ち、dispatch から操作する
#[derive(Debug, Default)]
pub struct CommandCards {
    cards: Vec<CommandCard>,
    next_id: u64,
}

impl CommandCards {
    pub fn new() -> Self {
        Self {
            cards: Vec::new(),
            next_id: 1,
        }
    }

    /// カードを追加する。ペインあたり `MAX_CARDS_PER_PANE` を超えた分は古い順に捨てる
    pub fn show(
        &mut self,
        pane: PaneId,
        commands: &[String],
        label: Option<&str>,
    ) -> Result<CommandCardId, CommandCardError> {
        let commands = normalize_commands(commands)?;
        let label = normalize_label(label)?;

        self.next_id = self.next_id.max(1);
        let id = CommandCardId(self.next_id);
        self.next_id += 1;
        self.cards.push(CommandCard {
            id,
            pane,
            commands,
            label,
        });

        // 同一ペインの古いカードから溢れさせる（追加順 = Vec の順序）
        while self.cards.iter().filter(|c| c.pane == pane).count() > MAX_CARDS_PER_PANE {
            if let Some(pos) = self.cards.iter().position(|c| c.pane == pane) {
                self.cards.remove(pos);
            } else {
                break;
            }
        }
        Ok(id)
    }

    /// 指定ペイン（`None` で全ペイン）のカードを古い順に返す
    pub fn list(&self, pane: Option<PaneId>) -> Vec<&CommandCard> {
        self.cards
            .iter()
            .filter(|c| pane.is_none_or(|p| c.pane == p))
            .collect()
    }

    pub fn get(&self, id: CommandCardId) -> Option<&CommandCard> {
        self.cards.iter().find(|c| c.id == id)
    }

    /// 指定ペインの最新カード（UI / CLI の「カード ID 省略時」の既定対象）
    pub fn latest_for(&self, pane: PaneId) -> Option<&CommandCard> {
        self.cards.iter().rev().find(|c| c.pane == pane)
    }

    /// 対象カードを解決する。`id` 省略時は `pane` の最新カード
    pub fn resolve(
        &self,
        pane: PaneId,
        id: Option<CommandCardId>,
    ) -> Result<&CommandCard, CommandCardError> {
        match id {
            Some(id) => self
                .get(id)
                .ok_or(CommandCardError::CardNotFound { id: id.as_u64() }),
            None => self
                .latest_for(pane)
                .ok_or(CommandCardError::CardNotFound { id: 0 }),
        }
    }

    /// カードを破棄する。`id` 指定でその 1 枚、省略で `pane` の全件。
    /// 戻り値は実際に消した枚数
    pub fn dismiss(&mut self, pane: Option<PaneId>, id: Option<CommandCardId>) -> usize {
        let before = self.cards.len();
        match id {
            Some(id) => self.cards.retain(|c| c.id != id),
            None => match pane {
                Some(pane) => self.cards.retain(|c| c.pane != pane),
                None => self.cards.clear(),
            },
        }
        before - self.cards.len()
    }

    /// 生きているペインだけ残す（ペインが閉じたカードの掃除）。戻り値は消した枚数
    pub fn retain_panes(&mut self, alive: impl Fn(PaneId) -> bool) -> usize {
        let before = self.cards.len();
        self.cards.retain(|c| alive(c.pane));
        before - self.cards.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    pub fn len(&self) -> usize {
        self.cards.len()
    }
}

/// 画面下部の「ライブ領域」（TUI の入力欄・フッター、シェルのプロンプト行）の上辺を
/// 探すとき、カーソル行から何行まで上を見るか（#681）。
///
/// claude の入力欄は「区切り罫線 + `❯` 行（複数行入力では折り返して数行）」なので、
/// カーソル行の直上が罫線とは限らない。一方で無制限に遡ると、通常シェルの出力に
/// たまたま含まれる `-----` を入力欄の上辺と誤認してカードが大きく浮いてしまう。
/// 実測（claude 2.1.220）の入力欄は罫線 + 2 行程度なので、余裕を見て 8 行で止める
const RULE_SEARCH_ROWS: usize = 8;

/// カードのアンカー（#681）: 生成時点のターミナル内容に対する位置。
///
/// **なぜ下部固定をやめたか**: 下端固定オーバーレイは claude の入力欄・フッター行に
/// ちょうど被る（2026-07-30 ユーザー実使用フィードバック）。生成時点の内容に紐付けて
/// おけば、新しい出力が流れればカードも一緒に上へ流れ、スクロールで戻れば一緒に戻る。
///
/// GPUI 非依存の数値だけを持つ。描画側（tako-app）は毎フレーム [`Self::viewport_row`] に
/// 「今の履歴行数」と「今のスクロール遡り量」を渡して描画行を求める。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CardAnchor {
    /// ライブ領域（入力欄・プロンプト）の上辺の行。**上に置くとき**この行の上端に
    /// カードの下端を合わせる。単位は行（小数可）で、**スクロール位置 0
    /// （最下部表示）のときのビューポート行**に正規化してある（0 = 最上行）
    pub base_row: f32,
    /// 最終内容行の 1 行下（= 内容の直後の空き行）。**下に置くとき**この行の上端に
    /// カードの上端を合わせる。`base_row` と同じ座標系
    pub tail_row: f32,
    /// 生成時点のスクロールバック行数。以後の増分が「上へ流れた行数」になる。
    /// tmux バックエンドペインは外側 alacritty に履歴が積まれない（alt screen）ため
    /// 常に 0 で、流れ量 0 = ライブ領域からの距離を保つ挙動になる
    pub base_history: usize,
}

/// カードの配置寸法（#681）。px 単位。`place` へまとめて渡す
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CardLayout {
    /// 1 行の高さ
    pub cell_height: f32,
    /// テキスト領域の高さ（上下パディングを除いた本文の高さ）
    pub area_height: f32,
    /// テキスト領域の上下パディング
    pub padding: f32,
    /// 下端の最小余白（ポート検知チップ FR-2.4.3 を避ける）
    pub min_bottom: f32,
    /// カードを潰さずに置くのに要する高さ
    pub min_height: f32,
    /// 完全に画面外へ出たと判断するまでの余裕（px）
    pub slack: f32,
}

/// カードの置き方（#681）。**内容（ライブ領域）を覆わない**ことを優先し、
/// 上に置けないときだけ内容の下の空きへ回す。
///
/// どちらも**テキスト領域の上端からの距離**で表す。下端基準にすると
/// 描画コンテナの実高さが必要になるが、`area_height`（レイアウト計算値）は
/// ウェルカムバナー等が上に載ったフレームで実高さより大きくなり得るため、
/// 行スタックと同じ「上端 + 行 × 行高」の基準に揃える
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CardPlacement {
    /// テキスト領域の上端からアンカー行の上端までの距離（px）。
    /// カードはこの位置に**下端**を合わせて上へ伸びる
    Above { space: f32 },
    /// テキスト領域の上端からカード**上端**までの距離（px）。カードは下へ伸びる
    Below { top: f32 },
}

impl CardAnchor {
    /// ライブ領域上辺の現在の描画行（小数。0 = 表示領域の最上行）。
    /// `history_now` は現在のスクロールバック行数、`scrolled_back` は現在の
    /// スクロール遡り量（行。0.0 = 最下部）
    pub fn viewport_row(&self, history_now: usize, scrolled_back: f32) -> f32 {
        self.base_row - self.flowed(history_now) + scrolled_back
    }

    /// 内容直後の空き行の現在の描画行（下に置くときの上端）
    pub fn tail_viewport_row(&self, history_now: usize, scrolled_back: f32) -> f32 {
        self.tail_row - self.flowed(history_now) + scrolled_back
    }

    fn flowed(&self, history_now: usize) -> f32 {
        history_now.saturating_sub(self.base_history) as f32
    }

    /// 生成時に「上へ置くか、内容の下へ置くか」を決める（#681）。
    ///
    /// 上（ライブ領域の直上）が既定。ただし**起動直後のシェルのようにプロンプトが
    /// 画面最上部にある**ときは上に空きが無く、カードがテキスト領域の上端で切り取られて
    /// 丸ごと見えなくなる（実機で確認）。その場合、内容の下が十分空いていればそちらへ置く
    /// = 何も覆わずに全体が見える。どちらも足りなければ上を選ぶ（切り取られても
    /// 入力欄を覆わない方を優先する）
    pub fn prefers_below(live_top_row: f32, tail_row: f32, layout: &CardLayout) -> bool {
        let above_space = live_top_row * layout.cell_height;
        if above_space >= layout.min_height {
            return false;
        }
        let below_space = layout.area_height - tail_row * layout.cell_height - layout.min_bottom;
        below_space >= layout.min_height
    }

    /// 現在の描画位置。完全に画面外へ流れ去っていれば None（描画を省く。
    /// 保管庫には残るので CLI / MCP の操作は効き続ける）
    pub fn place(
        &self,
        below: bool,
        history_now: usize,
        scrolled_back: f32,
        layout: &CardLayout,
    ) -> Option<CardPlacement> {
        let slack_rows = layout.slack / layout.cell_height.max(1.0);
        let rows = layout.area_height / layout.cell_height.max(1.0);
        if below {
            let row = self.tail_viewport_row(history_now, scrolled_back);
            // 上端が領域の下端より下 / 領域の上端よりはるかに上なら見えない
            if row >= rows || row < -slack_rows {
                return None;
            }
            return Some(CardPlacement::Below {
                top: layout.padding + row * layout.cell_height,
            });
        }
        let row = self.viewport_row(history_now, scrolled_back);
        // 下端が領域上端より上（上へ流れ切った）/ 領域下端よりはるかに下（遡り切った）
        if row <= 0.0 || row > rows + slack_rows {
            return None;
        }
        // 下端の最小余白（ポート検知チップ）は上限としてだけ効かせる
        let cap = (layout.area_height + layout.padding - layout.min_bottom).max(0.0);
        Some(CardPlacement::Above {
            space: (layout.padding + row * layout.cell_height).min(cap),
        })
    }
}

/// 画面下部の「ライブ領域」の先頭行を求める（#681）。カードはこの行より上に置く。
///
/// ライブ領域 = TUI が毎フレーム塗り直す入力欄・フッター（claude / codex 等）、
/// あるいは通常シェルのプロンプト行。**ここに被らないことが #681 の必須条件**。
///
/// カーソル行を起点に、直上へ続く罫線・区切り行（入力ボックスの上辺）を含める。
/// カーソル位置が分からない（スクロールバック中など）ときは画面最下部を返し、
/// 従来（#666）と同じ下端配置へ落ちる
pub fn live_region_top(line_texts: &[&str], cursor_row: Option<usize>) -> usize {
    let rows = line_texts.len();
    let Some(cursor) = cursor_row else {
        return rows;
    };
    let start = cursor.min(rows);
    let lower = start.saturating_sub(RULE_SEARCH_ROWS);
    for row in (lower..start).rev() {
        if is_rule_row(line_texts[row]) {
            return row;
        }
    }
    start
}

/// 罫線・区切りだけで構成された行か（入力ボックスの上辺の判定）
fn is_rule_row(text: &str) -> bool {
    let mut count = 0usize;
    for c in text.chars() {
        if c == ' ' || c == '\u{3000}' {
            continue;
        }
        if !is_rule_char(c) {
            return false;
        }
        count += 1;
    }
    count >= 3
}

fn is_rule_char(c: char) -> bool {
    matches!(
        c,
        '─' | '━'
            | '═'
            | '╌'
            | '╍'
            | '┄'
            | '┅'
            | '┈'
            | '┉'
            | '╭'
            | '╮'
            | '╰'
            | '╯'
            | '┌'
            | '┐'
            | '└'
            | '┘'
            | '│'
            | '┃'
            | '├'
            | '┤'
            | '┬'
            | '┴'
            | '┼'
            | '-'
            | '–'
            | '—'
            | '_'
            | '='
    )
}

/// 最終内容行の 1 行下（#681）。カードを内容の下に置くときの上端行で、
/// 全行が空なら 0（画面の最上部）
pub fn content_tail_row(line_texts: &[&str]) -> usize {
    line_texts
        .iter()
        .rposition(|t| !t.trim().is_empty())
        .map(|i| i + 1)
        .unwrap_or(0)
}

/// コマンド 1 件の正規化。**論理文字列を壊さないこと**が最優先で、
/// 折り返しのための加工は一切しない（表示側の責務）
fn normalize_command(raw: &str, index: usize) -> Result<String, CommandCardError> {
    // CRLF / CR を LF へ寄せる（Windows 由来の文字列をそのまま貼れるように）。
    // 行内の空白・インデントは意味を持つので触らない
    let unified = raw.replace("\r\n", "\n").replace('\r', "\n");
    // 前後の空白・改行だけ落とす（内側の改行は複数行コマンドとして保持）
    let trimmed = unified.trim().to_string();
    if trimmed.is_empty() {
        return Err(CommandCardError::EmptyCommand { index });
    }
    if trimmed.len() > MAX_COMMAND_BYTES {
        return Err(CommandCardError::CommandTooLong {
            index,
            len: trimmed.len(),
            max: MAX_COMMAND_BYTES,
        });
    }
    // ESC / BEL 等の制御文字は拒否する。カードのコマンドはクリップボードと
    // シェルへ渡る文字列なので、エスケープシーケンスの混入を構造的に防ぐ
    if trimmed
        .chars()
        .any(|c| c != '\n' && c != '\t' && c.is_control())
    {
        return Err(CommandCardError::ControlCharacter { index });
    }
    Ok(trimmed)
}

/// コマンド列の正規化 + 件数検査
fn normalize_commands(raw: &[String]) -> Result<Vec<String>, CommandCardError> {
    if raw.is_empty() {
        return Err(CommandCardError::NoCommands);
    }
    if raw.len() > MAX_COMMANDS_PER_CARD {
        return Err(CommandCardError::TooManyCommands {
            len: raw.len(),
            max: MAX_COMMANDS_PER_CARD,
        });
    }
    raw.iter()
        .enumerate()
        .map(|(i, c)| normalize_command(c, i + 1))
        .collect()
}

/// ラベルの正規化。空文字列はラベル無し扱い（`--label ""` で怒らない）
fn normalize_label(raw: Option<&str>) -> Result<Option<String>, CommandCardError> {
    let Some(label) = raw else { return Ok(None) };
    let label = label.replace(['\r', '\n'], " ").trim().to_string();
    if label.is_empty() {
        return Ok(None);
    }
    let chars = label.chars().count();
    if chars > MAX_LABEL_CHARS {
        return Err(CommandCardError::LabelTooLong {
            len: chars,
            max: MAX_LABEL_CHARS,
        });
    }
    // 制御文字（改行は上で空白化済み）は落とす
    Ok(Some(label.chars().filter(|c| !c.is_control()).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PaneOrigin;

    fn pane() -> PaneId {
        crate::Pane::new(PaneOrigin::User).id()
    }

    #[test]
    fn 長いコマンドは論理1行のまま保たれる() {
        let long = format!("cargo test --workspace -- {}", "x".repeat(400));
        let mut cards = CommandCards::new();
        let p = pane();
        let id = cards.show(p, std::slice::from_ref(&long), None).unwrap();
        let card = cards.get(id).unwrap();
        assert_eq!(card.command(1).unwrap(), long);
        assert!(
            !card.command(1).unwrap().contains('\n'),
            "折り返しのための改行が混入してはならない"
        );
    }

    #[test]
    fn 複数行コマンドは改行を保持する() {
        let multi = "cd /tmp \\\n  && ls -la\necho done";
        let mut cards = CommandCards::new();
        let id = cards.show(pane(), &[multi.to_string()], None).unwrap();
        assert_eq!(cards.get(id).unwrap().command(1).unwrap(), multi);
    }

    #[test]
    fn crlfはlfへ寄せ前後の空白だけ落とす() {
        let mut cards = CommandCards::new();
        let id = cards
            .show(pane(), &["  echo a\r\necho b  \r\n".to_string()], None)
            .unwrap();
        assert_eq!(cards.get(id).unwrap().command(1).unwrap(), "echo a\necho b");
    }

    #[test]
    fn 空コマンドと空白のみは拒否される() {
        let mut cards = CommandCards::new();
        assert_eq!(
            cards.show(pane(), &[String::new()], None),
            Err(CommandCardError::EmptyCommand { index: 1 })
        );
        assert_eq!(
            cards.show(pane(), &["   \n  ".to_string()], None),
            Err(CommandCardError::EmptyCommand { index: 1 })
        );
        assert_eq!(
            cards.show(pane(), &[], None),
            Err(CommandCardError::NoCommands)
        );
        assert!(cards.is_empty(), "失敗した show でカードを作らない");
    }

    #[test]
    fn 制御文字を含むコマンドは拒否される() {
        let mut cards = CommandCards::new();
        assert_eq!(
            cards.show(pane(), &["echo \x1b[31mred".to_string()], None),
            Err(CommandCardError::ControlCharacter { index: 1 })
        );
        // 改行とタブは正当な中身として通る
        assert!(cards.show(pane(), &["a\tb\nc".to_string()], None).is_ok());
    }

    #[test]
    fn 上限を超えるコマンドと件数は拒否される() {
        let mut cards = CommandCards::new();
        let too_long = "x".repeat(MAX_COMMAND_BYTES + 1);
        assert!(matches!(
            cards.show(pane(), &[too_long], None),
            Err(CommandCardError::CommandTooLong { index: 1, .. })
        ));
        let many: Vec<String> = (0..=MAX_COMMANDS_PER_CARD)
            .map(|i| format!("e {i}"))
            .collect();
        assert!(matches!(
            cards.show(pane(), &many, None),
            Err(CommandCardError::TooManyCommands { .. })
        ));
    }

    #[test]
    fn ラベルは改行を空白化し上限超過は拒否される() {
        let mut cards = CommandCards::new();
        let p = pane();
        let id = cards
            .show(p, &["ls".to_string()], Some(" 依存を\n入れる "))
            .unwrap();
        assert_eq!(cards.get(id).unwrap().label(), Some("依存を 入れる"));
        // 空ラベルはラベル無し
        let id = cards.show(p, &["ls".to_string()], Some("   ")).unwrap();
        assert_eq!(cards.get(id).unwrap().label(), None);
        let long: String = "あ".repeat(MAX_LABEL_CHARS + 1);
        assert!(matches!(
            cards.show(p, &["ls".to_string()], Some(&long)),
            Err(CommandCardError::LabelTooLong { .. })
        ));
    }

    #[test]
    fn ペインあたりの上限を超えると古いカードから消える() {
        let mut cards = CommandCards::new();
        let p = pane();
        let mut ids = Vec::new();
        for i in 0..(MAX_CARDS_PER_PANE + 2) {
            ids.push(cards.show(p, &[format!("echo {i}")], None).unwrap());
        }
        assert_eq!(cards.list(Some(p)).len(), MAX_CARDS_PER_PANE);
        assert!(cards.get(ids[0]).is_none(), "最古のカードが残っている");
        assert!(cards.get(*ids.last().unwrap()).is_some());
    }

    #[test]
    fn 他ペインのカードは溢れさせない() {
        let mut cards = CommandCards::new();
        let (a, b) = (pane(), pane());
        let keep = cards.show(b, &["echo b".to_string()], None).unwrap();
        for i in 0..(MAX_CARDS_PER_PANE + 2) {
            cards.show(a, &[format!("echo {i}")], None).unwrap();
        }
        assert!(cards.get(keep).is_some());
        assert_eq!(cards.list(Some(b)).len(), 1);
    }

    #[test]
    fn 対象解決はid指定と最新カードの両方で効く() {
        let mut cards = CommandCards::new();
        let p = pane();
        let first = cards.show(p, &["echo 1".to_string()], None).unwrap();
        let second = cards.show(p, &["echo 2".to_string()], None).unwrap();
        assert_eq!(cards.resolve(p, None).unwrap().id(), second);
        assert_eq!(cards.resolve(p, Some(first)).unwrap().id(), first);
        assert_eq!(
            cards.resolve(p, Some(CommandCardId::from_raw(9999))),
            Err(CommandCardError::CardNotFound { id: 9999 })
        );
        // カードが無いペインは「最新」も解決できない
        let empty = pane();
        assert!(cards.resolve(empty, None).is_err());
    }

    #[test]
    fn コマンド番号は1始まりで範囲外を拒否する() {
        let mut cards = CommandCards::new();
        let id = cards
            .show(pane(), &["a".to_string(), "b".to_string()], None)
            .unwrap();
        let card = cards.get(id).unwrap();
        assert_eq!(card.command(1).unwrap(), "a");
        assert_eq!(card.command(2).unwrap(), "b");
        assert_eq!(
            card.command(0),
            Err(CommandCardError::IndexOutOfRange { index: 0, len: 2 })
        );
        assert_eq!(
            card.command(3),
            Err(CommandCardError::IndexOutOfRange { index: 3, len: 2 })
        );
    }

    #[test]
    fn 破棄はid単位とペイン単位で効く() {
        let mut cards = CommandCards::new();
        let (a, b) = (pane(), pane());
        let a1 = cards.show(a, &["1".to_string()], None).unwrap();
        cards.show(a, &["2".to_string()], None).unwrap();
        cards.show(b, &["3".to_string()], None).unwrap();
        assert_eq!(cards.dismiss(None, Some(a1)), 1);
        assert_eq!(cards.list(Some(a)).len(), 1);
        assert_eq!(cards.dismiss(Some(a), None), 1);
        assert_eq!(cards.list(Some(a)).len(), 0);
        assert_eq!(cards.list(None).len(), 1, "他ペインは無傷");
        assert_eq!(cards.dismiss(None, Some(a1)), 0, "二重破棄は 0 件");
    }

    /// claude 2.1.220 の実画面（隔離 tmux 120x40 で 2026-07-30 に採取）。
    /// 末尾 9 行 = 区切り罫線 / `❯` 入力行 / 折り返し行 / 区切り罫線 / フッター 5 行
    fn claude_tail() -> Vec<&'static str> {
        vec![
            "  ⏺ 変更しました。次のコマンドで確認できます。",
            "",
            "────────────────────────────────────────",
            "❯ これは複数行になる長い入力のテストです。カードのアンカー位置を決めるために",
            "  入力欄の高さと罫線の位置を測ります。",
            "────────────────────────────────────────",
            "  [Opus 5 (1M context) · xH]  ▸ 2.1.220",
            "  ctx   0% ░░░░░░░░░░",
            "  ⏵⏵ auto mode on (shift+tab to cycle)",
        ]
    }

    #[test]
    fn claudeの入力欄より上にアンカーが決まる() {
        let lines = claude_tail();
        // カーソルは入力の最終行（折り返し行 = index 4）に居る
        let top = live_region_top(&lines, Some(4));
        assert_eq!(
            top, 2,
            "入力ボックスの上辺（区切り罫線）を live 領域に含める"
        );
        // 単一行入力（カーソルが `❯` 行）でも同じ行に決まる
        assert_eq!(live_region_top(&lines, Some(3)), 2);
        // 入力欄・フッターのどの行にも被らない
        for row in top..lines.len() {
            assert!(row >= top, "row {row} はカードの下端より下 = 覆わない");
        }
    }

    #[test]
    fn 通常シェルはプロンプト行がアンカーになる() {
        let lines = vec![
            "$ cargo build",
            "   Compiling tako-core",
            "    Finished",
            "$ ",
        ];
        // プロンプト行にカーソル。直上は罫線ではないのでプロンプト行がそのまま上辺
        assert_eq!(live_region_top(&lines, Some(3)), 3);
        // カーソル位置不明（スクロールバック中）は最下部 = 従来の下端配置へ落ちる
        assert_eq!(live_region_top(&lines, None), 4);
    }

    #[test]
    fn 罫線判定は区切り行だけを拾う() {
        assert!(is_rule_row("────────"));
        assert!(is_rule_row("  ╭──────╮  "));
        assert!(is_rule_row("========"));
        assert!(!is_rule_row(""), "空行は区切りではない");
        assert!(!is_rule_row("--"), "2 文字以下は本文の可能性が高い");
        assert!(!is_rule_row("── 見出し ──"), "文字が混ざる行は本文");
        assert!(!is_rule_row("❯ ls -la"));
    }

    #[test]
    fn 罫線探索は8行より上へは遡らない() {
        let mut lines = vec!["────────"];
        lines.extend(std::iter::repeat_n("output line", 9));
        // カーソル = 最下行（index 9）から 8 行上（index 1）までしか見ない
        assert_eq!(live_region_top(&lines, Some(9)), 9);
        // 罫線が探索窓に入れば拾う
        let near = vec!["a", "────────", "b", "c"];
        assert_eq!(live_region_top(&near, Some(3)), 1);
    }

    #[test]
    fn 内容の末尾行は最後の非空行の1行下() {
        let lines = claude_tail();
        // 最終行（フッター）が非空なので末尾 = 行数（下に空きが無い = 上へ置く）
        assert_eq!(content_tail_row(&lines), lines.len());
        // 起動直後のシェル: プロンプト行の 1 行下が末尾
        let fresh = vec!["direnv: unloading", "$ ", "", "", ""];
        assert_eq!(content_tail_row(&fresh), 2);
        assert_eq!(content_tail_row(&["", "", ""]), 0, "全行空なら最上部");
    }

    fn layout() -> CardLayout {
        // 17px 行 / 本文 340px（20 行）/ padding 10 / 最小余白 10 / 最小高さ 120
        CardLayout {
            cell_height: 17.0,
            area_height: 340.0,
            padding: 10.0,
            min_bottom: 10.0,
            min_height: 120.0,
            slack: 320.0,
        }
    }

    #[test]
    fn 起動直後のシェルではカードを内容の下へ置く() {
        let l = layout();
        // プロンプトが 1 行目（上の空き 17px < 120）で、下は空き十分 → 下配置
        assert!(CardAnchor::prefers_below(1.0, 2.0, &l));
        // claude のように下がフッターで埋まっていれば上配置（切り取られても入力欄は覆わない）
        assert!(!CardAnchor::prefers_below(1.0, 20.0, &l));
        // 画面中ほどにライブ領域があれば上配置
        assert!(!CardAnchor::prefers_below(12.0, 20.0, &l));
    }

    #[test]
    fn 配置は上下どちらもアンカー行から求まる() {
        let l = layout();
        let a = CardAnchor {
            base_row: 12.0,
            tail_row: 13.0,
            base_history: 0,
        };
        // 上配置: 上端からアンカー行の上端まで = 10 + 12*17 = 214px（下端をここに合わせる）
        assert_eq!(
            a.place(false, 0, 0.0, &l),
            Some(CardPlacement::Above { space: 214.0 })
        );
        // 下配置: 上端 = 10 + 13*17 = 231px
        assert_eq!(
            a.place(true, 0, 0.0, &l),
            Some(CardPlacement::Below { top: 231.0 })
        );
        // ポート検知チップ（最小余白 30px）が出ていれば下端はそこまで押し上がる
        // （最下行アンカー: 10 + 20*17 = 350 → 上限 340 + 10 - 30 = 320）
        let chip = CardLayout {
            min_bottom: 30.0,
            ..l
        };
        let bottom_anchor = CardAnchor {
            base_row: 20.0,
            tail_row: 20.0,
            base_history: 0,
        };
        assert_eq!(
            bottom_anchor.place(false, 0, 0.0, &chip),
            Some(CardPlacement::Above { space: 320.0 })
        );
    }

    #[test]
    fn 流れ去ったカードは配置を返さない() {
        let l = layout();
        let a = CardAnchor {
            base_row: 12.0,
            tail_row: 13.0,
            base_history: 0,
        };
        // 上配置: 12 行ぶん流れると下端が領域上端に達して見えない
        assert!(a.place(false, 12, 0.0, &l).is_none());
        assert!(a.place(false, 11, 0.0, &l).is_some());
        // 過去へ遡り切ると下へ抜ける（下端 20 行 + slack 320/17 ≈ 18.8 行 = 38.8 行まで）
        assert!(a.place(false, 0, 26.0, &l).is_some(), "row 38 はまだ描く");
        assert!(
            a.place(false, 0, 28.0, &l).is_none(),
            "row 40 は完全に画面外"
        );
        // 下配置: 上端が領域下端（20 行）に達したら見えない
        assert!(a.place(true, 0, 7.0, &l).is_none());
        assert!(a.place(true, 0, 6.0, &l).is_some());
    }

    #[test]
    fn アンカーは出力が流れると上へ移動する() {
        let a = CardAnchor {
            base_row: 30.0,
            tail_row: 31.0,
            base_history: 100,
        };
        // 出力なし・スクロールなし = 生成時の行
        assert_eq!(a.viewport_row(100, 0.0), 30.0);
        // 5 行流れた = 5 行上へ
        assert_eq!(a.viewport_row(105, 0.0), 25.0);
        // 上端より上へ流れ切ると負（描画しない領域）
        assert!(a.viewport_row(140, 0.0) < 0.0);
        // 過去へ 5 行スクロール = 内容と一緒に 5 行下へ戻る
        assert_eq!(a.viewport_row(105, 5.0), 30.0);
        // サブライン（行小数）でも連続に動く
        assert!((a.viewport_row(105, 5.5) - 30.5).abs() < f32::EPSILON);
        // 履歴が減る（clear / reset）方向では流れ量 0 に留める（負の巻き戻りを作らない）
        assert_eq!(a.viewport_row(90, 0.0), 30.0);
        // 末尾行も同じだけ動く（上下どちらの配置でも内容に付いて回る）
        assert_eq!(a.tail_viewport_row(105, 0.0), 26.0);
    }

    #[test]
    fn 閉じたペインのカードは掃除される() {
        let mut cards = CommandCards::new();
        let (alive, dead) = (pane(), pane());
        cards.show(alive, &["1".to_string()], None).unwrap();
        cards.show(dead, &["2".to_string()], None).unwrap();
        assert_eq!(cards.retain_panes(|p| p == alive), 1);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards.list(Some(alive)).len(), 1);
    }
}
