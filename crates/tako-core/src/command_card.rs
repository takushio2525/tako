//! AI コマンド提案カード（FR-2.22 / Issue #666、表示位置は #703）
//!
//! AI（master / solo / worker）が「ユーザーに実行してほしいコマンド」を渡すと、
//! 対象ペインの**ターミナル領域の下に作った専用帯**へネイティブカードとして出す。
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

/// カード帯がテキスト領域を占めてよい上限の割合（#703）。
/// これを超えると「会話が読めない」問題を別の形で作り直してしまう
pub const MAX_BAND_RATIO: f32 = 0.34;

/// 上限の下駄（px）。狭いペインでも割合だけで決めるとカード 1 枚すら置けず、
/// 帯がボタンを切り落とした細い帯になる。カード 1 枚ぶんは置けるようにする。
///
/// **ただし下駄が端末を飲み込んではいけない**（[`MAX_BAND_SHARE`]）。半分の高さに
/// 分割したペインで下駄をそのまま効かせると、claude の取り分が 3 行まで潰れた（実測）
pub const MIN_BAND_HEIGHT: f32 = 112.0;

/// 下駄を効かせるときでも帯がテキスト領域の何割までしか占めてよいか（#703）
pub const MAX_BAND_SHARE: f32 = 0.5;

/// 帯をどれだけ広げてもターミナル領域に必ず残す行数（#703）。
/// エージェントの入力欄が消えるほど縮めては本末転倒なので、帯より端末を優先する
pub const MIN_TERMINAL_ROWS: usize = 3;

/// カード帯の高さ（行数）を決める（#703）。
///
/// **なぜ行単位か**: 帯はターミナル領域を縮めて作る（`rows = floor((領域高 - 帯高) /
/// 行高)`）。px で決めると 1px の測定揺れが行数を跨いだ瞬間に PTY resize を呼び、
/// TUI が画面を作り直す。行に量子化しておけば、揺れが行を跨がない限り resize は起きず、
/// ターミナル領域の下端も必ず行境界に落ちる（半端に切れた行が残らない）。
///
/// - `desired_height`: カードを潰さず描くのに要する高さ（px。実描画からの採取値）
/// - `area_height`: 帯を差し引く前のテキスト領域の高さ（px）
/// - `cell_height`: 1 行の高さ（px）
///
/// 0 を返したら帯を作らない（= ターミナル領域は従来どおり全部使う）。
/// カードは保管庫に残るので CLI / MCP の copy / run / dismiss は効き続ける
pub fn band_rows(desired_height: f32, area_height: f32, cell_height: f32) -> usize {
    let cell = cell_height.max(1.0);
    // NaN も「帯なし」へ倒す（`> 0.0` が false になる比較をそのまま使う）
    if desired_height.is_nan() || desired_height <= 0.0 || area_height <= 0.0 {
        return 0;
    }
    // 上限は「割合（下駄つき）」と「端末に必ず残す行数」の厳しい方。
    // 下駄自身も領域の `MAX_BAND_SHARE` までに抑える（低いペインで端末を飲み込まない）
    let floor = MIN_BAND_HEIGHT.min(area_height * MAX_BAND_SHARE);
    let soft_cap = (area_height * MAX_BAND_RATIO).max(floor);
    let hard_cap = area_height - MIN_TERMINAL_ROWS as f32 * cell;
    let cap_rows = (soft_cap.min(hard_cap) / cell).floor();
    if cap_rows.is_nan() || cap_rows < 1.0 {
        // 1 行も割けない極端に低いペイン。帯を作らず端末をそのまま残す
        return 0;
    }
    let want = (desired_height / cell).ceil().max(1.0);
    want.min(cap_rows) as usize
}

/// 帯の高さ（px）。`band_rows` の結果を px へ戻す 1 箇所の変換
pub fn band_height(rows: usize, cell_height: f32) -> f32 {
    rows as f32 * cell_height.max(1.0)
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

    /// 実測に近い寸法: 17px 行 / テキスト領域 340px（20 行）
    const CELL: f32 = 17.0;
    const AREA: f32 = 340.0;

    #[test]
    fn カードが無ければ帯を作らない() {
        assert_eq!(band_rows(0.0, AREA, CELL), 0);
        assert_eq!(band_rows(-1.0, AREA, CELL), 0);
        assert_eq!(band_rows(f32::NAN, AREA, CELL), 0, "NaN でも帯は作らない");
        // 帯 0 行 = ターミナル領域は 1px も削られない（既存の見た目が不変）
        assert_eq!(band_height(0, CELL), 0.0);
    }

    #[test]
    fn 帯は必要な高さを行単位で切り上げて確保する() {
        // 1 行に満たない要求でも 1 行
        assert_eq!(band_rows(1.0, AREA, CELL), 1);
        // 100px = 5.88 行 → 6 行（カードが縦に潰れない）
        assert_eq!(band_rows(100.0, AREA, CELL), 6);
        assert!((band_height(6, CELL) - 102.0).abs() < f32::EPSILON);
        // ちょうど行境界なら切り上げない
        assert_eq!(band_rows(85.0, AREA, CELL), 5);
    }

    #[test]
    fn 帯の高さには上限がある() {
        // 上限 = 340 * 0.34 = 115.6px → 6 行（102px）。要求がいくら大きくても超えない
        assert_eq!(band_rows(f32::MAX, AREA, CELL), 6);
        assert_eq!(band_rows(1000.0, AREA, CELL), 6);
        // 上限を超えたぶんは帯の中でスクロールさせる = ターミナルは 14 行残る
        let rows_left = ((AREA - band_height(band_rows(1000.0, AREA, CELL), CELL)) / CELL) as usize;
        assert_eq!(rows_left, 14);
    }

    #[test]
    fn 狭いペインでもカード1枚ぶんの下駄が効く() {
        // 割合だけだと 300 * 0.34 = 102px（6 行）。下駄 112px で 6 行（102px）まで許す
        assert_eq!(band_rows(1000.0, 300.0, CELL), 6);
        // それでもターミナルには最低 3 行残る
        assert!(300.0 - band_height(6, CELL) >= MIN_TERMINAL_ROWS as f32 * CELL);
    }

    #[test]
    fn 下駄は端末を飲み込まない() {
        // 半分の高さへ分割した 10 行（170px）のペイン。下駄をそのまま効かせると
        // 112px = 6 行を帯が取り、claude の取り分が 3 行まで潰れる（実測で踏んだ）。
        // 下駄は領域の半分（85px = 5 行）までに抑える
        assert_eq!(band_rows(1000.0, 170.0, CELL), 5);
        let left = ((170.0 - band_height(5, CELL)) / CELL).floor() as usize;
        assert_eq!(left, 5, "端末にも半分残る");
        // 広いペインでは従来どおり割合（357 * 0.34 = 121px = 7 行）が効く
        assert_eq!(band_rows(1000.0, 357.0, CELL), 7);
    }

    #[test]
    fn 極端に低いペインでは端末を優先する() {
        // 5 行（85px）: 端末に 3 行残すと帯は 2 行まで
        assert_eq!(band_rows(1000.0, 85.0, CELL), 2);
        // 3 行（51px）: 端末で使い切るので帯は作らない
        assert_eq!(band_rows(1000.0, 51.0, CELL), 0);
        assert_eq!(band_rows(1000.0, 10.0, CELL), 0);
        assert_eq!(band_rows(50.0, 0.0, CELL), 0);
    }

    #[test]
    fn 行高が壊れていても破綻しない() {
        // 0 / 負の行高は 1px 扱い（div by zero・無限ループを構造的に防ぐ）
        assert!(band_rows(50.0, AREA, 0.0) > 0);
        assert!(band_rows(50.0, AREA, -5.0) > 0);
        assert_eq!(band_height(3, 0.0), 3.0);
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
