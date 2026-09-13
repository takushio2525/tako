//! 1 行 / 複数行のアプリ内テキスト入力の**純ロジック**（Issue #1450 の分割 B2）
//!
//! 右サイドバーには #1450 B2 の時点で**手書きのテキスト入力が 2 本**あった
//! （git のコミットメッセージ欄 = `right_panel::handle_git_commit_key`、
//! 新規ブランチ名欄 = `right_panel::handle_git_branch_input_key`）。中身は
//! 「`floor_char_boundary` で文字境界へ丸めてから backspace / delete / left /
//! right / home / end / 挿入する」同じコードで、状態の持ち方（`String` +
//! バイトオフセットのカーソル）まで同型だった。**片方だけ直した境界バグが
//! もう片方に残る**形なので、ユーザータスクの返答コメント欄で 3 本目が要ったとき、
//! コピーで増やさずここへ切り出した。
//!
//! ここは GPUI に依存しない（`Context` も `Keystroke` も受けない）ので、
//! マルチバイト境界・空・末尾の検出力を GUI を起動しない単体テストで確かめられる。
//! キーの**割り当て**（`enter` が改行か送信か、`escape` が何を閉じるか）は
//! 画面ごとに違うので**ここには置かない** —— 各画面が編集操作だけをここへ委ねる。
//!
//! 既存 2 本の移行は #1459 で追う（番犬 `issue1450b2_tasks_panel_watchdog` が
//! 「**新しい**手書きカーソル操作を足せない」を縛り、既存 2 本は名指しの猶予にしてある）。

use crate::right_panel::floor_char_boundary;

/// テキスト入力の状態（本文とキャレット位置）。
///
/// `cursor` は**バイトオフセット**で、常に文字境界に丸められている
/// （境界を割ったまま `split` / `drain` すると panic する = #494 の再発防止）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TextField {
    text: String,
    cursor: usize,
}

impl TextField {
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn cursor(&self) -> usize {
        self.cursor
    }

    pub(crate) fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// 文字境界へ丸めたキャレット位置。以降の操作はすべてこれを起点にする
    fn caret(&mut self) -> usize {
        let caret = floor_char_boundary(&self.text, self.cursor.min(self.text.len()));
        self.cursor = caret;
        caret
    }

    /// キャレット位置へ挿入する。
    ///
    /// - 制御文字は落とす（改行は `multiline` のときだけ残す）
    /// - `max_len` バイトを超えるぶんは**入らない**。1 文字も入らなかったときは
    ///   `false` を返すので、呼び出し側は「打ったのに増えない」を黙って捨てずに
    ///   画面へ理由を出せる（#1399 系の「無言で失敗する UI」を作らないため）
    pub(crate) fn insert(&mut self, text: &str, max_len: usize, multiline: bool) -> bool {
        let filtered: String = text
            .chars()
            .filter(|c| !c.is_control() || (multiline && *c == '\n'))
            .collect();
        if filtered.is_empty() {
            // そもそも入れるものが無い（制御文字だけの入力）。失敗ではない
            return true;
        }
        let room = max_len.saturating_sub(self.text.len());
        let cut = floor_char_boundary(&filtered, room.min(filtered.len()));
        if cut == 0 {
            return false;
        }
        let caret = self.caret();
        self.text.insert_str(caret, &filtered[..cut]);
        self.cursor = caret + cut;
        // 一部しか入らなかったときも「入り切らなかった」と申告する
        cut == filtered.len()
    }

    /// キャレットの手前 1 文字を消す
    pub(crate) fn backspace(&mut self) {
        let caret = self.caret();
        if caret == 0 {
            return;
        }
        let prev = self.text[..caret]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.text.drain(prev..caret);
        self.cursor = prev;
    }

    /// キャレットの直後 1 文字を消す
    pub(crate) fn delete_forward(&mut self) {
        let caret = self.caret();
        if caret >= self.text.len() {
            return;
        }
        let next = caret
            + self.text[caret..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(0);
        self.text.drain(caret..next);
    }

    pub(crate) fn move_left(&mut self) {
        let caret = self.caret();
        self.cursor = self.text[..caret]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
    }

    pub(crate) fn move_right(&mut self) {
        let caret = self.caret();
        if caret >= self.text.len() {
            return;
        }
        self.cursor = caret
            + self.text[caret..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(0);
    }

    pub(crate) fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub(crate) fn move_end(&mut self) {
        self.cursor = self.text.len();
    }

    /// 編集キーを 1 つ処理する。**扱ったら `true`**。
    ///
    /// `enter` / `escape` / `⌘V` のような「画面ごとに意味が違うキー」は
    /// ここでは扱わない（呼び出し側が先に自分の割り当てを見てから、
    /// 残りをここへ渡す）。だから戻り値は「編集操作だったか」であって
    /// 「打鍵を消費したか」ではない
    pub(crate) fn handle_edit_key(&mut self, key: &str) -> bool {
        match key {
            "backspace" => self.backspace(),
            "delete" => self.delete_forward(),
            "left" => self.move_left(),
            "right" => self.move_right(),
            "home" => self.move_home(),
            "end" => self.move_end(),
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: usize = 64;

    fn field(text: &str) -> TextField {
        let mut f = TextField::default();
        assert!(f.insert(text, MAX, true));
        f
    }

    #[test]
    fn 挿入はキャレット位置へ入りキャレットが進む() {
        let mut f = field("abc");
        assert_eq!(f.text(), "abc");
        assert_eq!(f.cursor(), 3);
        f.move_home();
        assert!(f.insert("X", MAX, false));
        assert_eq!(f.text(), "Xabc");
        assert_eq!(f.cursor(), 1);
    }

    /// #494 の再発防止: マルチバイト文字の途中をキャレットにしても panic しない
    #[test]
    fn キャレットは文字境界へ丸められる() {
        let mut f = field("あいう");
        f.cursor = 1; // 「あ」の途中（不正なオフセット）
        f.backspace();
        // 丸められた先（0）の手前には何も無いので消えない
        assert_eq!(f.text(), "あいう");
        f.cursor = 4; // 「い」の途中
        f.backspace();
        assert_eq!(f.text(), "いう");
    }

    #[test]
    fn backspaceとdeleteはマルチバイトを1文字ずつ消す() {
        let mut f = field("あbい");
        f.backspace();
        assert_eq!(f.text(), "あb");
        f.move_home();
        f.delete_forward();
        assert_eq!(f.text(), "b");
        // 端では何も起きない
        f.move_home();
        f.backspace();
        assert_eq!(f.text(), "b");
        f.move_end();
        f.delete_forward();
        assert_eq!(f.text(), "b");
    }

    #[test]
    fn 左右移動はマルチバイトを1文字ずつまたぐ() {
        let mut f = field("あい");
        assert_eq!(f.cursor(), 6);
        f.move_left();
        assert_eq!(f.cursor(), 3);
        f.move_left();
        assert_eq!(f.cursor(), 0);
        // 端を越えない
        f.move_left();
        assert_eq!(f.cursor(), 0);
        f.move_right();
        assert_eq!(f.cursor(), 3);
        f.move_end();
        f.move_right();
        assert_eq!(f.cursor(), 6);
    }

    #[test]
    fn 空のフィールドはどの操作でも壊れない() {
        let mut f = TextField::default();
        assert!(f.text().is_empty());
        f.backspace();
        f.delete_forward();
        f.move_left();
        f.move_right();
        f.move_home();
        f.move_end();
        assert_eq!(f.text(), "");
        assert_eq!(f.cursor(), 0);
    }

    /// 上限に当たったら**入らなかったと申告する**（黙って切り捨てない）
    #[test]
    fn 上限に当たったら失敗を返す() {
        let mut f = TextField::default();
        assert!(f.insert("12345", 5, false));
        assert!(!f.insert("6", 5, false));
        assert_eq!(f.text(), "12345");
        // 途中まで入る場合も「入り切らなかった」側に倒す
        let mut g = TextField::default();
        assert!(!g.insert("あいう", 4, false));
        assert_eq!(g.text(), "あ");
    }

    /// 制御文字は落とす。改行は複数行のときだけ残す
    #[test]
    fn 制御文字は落とし改行は複数行のときだけ残す() {
        let mut single = TextField::default();
        assert!(single.insert("a\nb\tc", MAX, false));
        assert_eq!(single.text(), "abc");
        let mut multi = TextField::default();
        assert!(multi.insert("a\nb\tc", MAX, true));
        assert_eq!(multi.text(), "a\nbc");
    }

    #[test]
    fn 編集キーだけを扱い割り当てキーは呼び出し側へ返す() {
        let mut f = field("ab");
        assert!(f.handle_edit_key("backspace"));
        assert_eq!(f.text(), "a");
        assert!(f.handle_edit_key("home"));
        assert_eq!(f.cursor(), 0);
        // 画面ごとに意味が変わるキーはここでは扱わない
        assert!(!f.handle_edit_key("enter"));
        assert!(!f.handle_edit_key("escape"));
        assert!(!f.handle_edit_key("v"));
    }

    #[test]
    fn clearで本文もキャレットも戻る() {
        let mut f = field("あいう");
        f.clear();
        assert!(f.text().is_empty());
        assert_eq!(f.cursor(), 0);
    }
}
