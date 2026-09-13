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
//! 既存 2 本は #1459 でここへ移し終えた。境界を丸める [`floor_char_boundary`] は
//! **このモジュールの外へ出さない**（`pub` を付けない）ので、キャレットを自前で
//! 丸める実装はコンパイラの時点で書けない。番犬
//! `issue1450b2_tasks_panel_watchdog` が `tasks_panel.rs` / `right_panel.rs` の
//! 双方を走査し、手書きのカーソル操作が生えたら `file:line` で名指しする。

/// バイトオフセットを直近の文字境界へ切り下げる（#494）。
///
/// `String::split_at` / `insert_str` は文字の途中のオフセットを渡すと panic し、
/// GPUI の描画中に落ちるとアプリ全体が巻き添えで死ぬ。キャレット位置は
/// マルチバイト文字・IME・貼り付けで境界を外し得るので、使う直前に必ず丸める。
///
/// **モジュール外へ公開しない**（#1459）。丸めが要る操作はすべて [`TextField`] の
/// メソッドとしてここに在るべきで、外へ出すと「呼び出し側で丸めてから自前で
/// drain する」手書き実装が再び生える。
fn floor_char_boundary(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

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

    /// 本文を丸ごと置き換え、キャレットを末尾へ置く。
    ///
    /// 打鍵ではなくプログラムからの復元（コミット失敗時の打ち直し回避など）用。
    /// 打った文字の検査（制御文字・上限）は [`Self::insert`] の側にある
    pub(crate) fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
    }

    pub(crate) fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// 文字境界へ丸めたキャレット位置（**描画用**。状態は変えない）。
    ///
    /// `cursor()` の生値は外から差し込まれた値を含み得るので、`split_at` など
    /// panic する操作の前はこちらを通す
    pub(crate) fn caret(&self) -> usize {
        floor_char_boundary(&self.text, self.cursor.min(self.text.len()))
    }

    /// キャレットで本文を前後に分ける（**描画用**。#494 の panic をここで閉じる）
    pub(crate) fn split_at_caret(&self) -> (&str, &str) {
        self.text.split_at(self.caret())
    }

    /// 文字境界へ丸めたキャレット位置を**書き戻す**。編集はすべてこれを起点にする
    fn normalize_caret(&mut self) -> usize {
        let caret = self.caret();
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
        let caret = self.normalize_caret();
        self.text.insert_str(caret, &filtered[..cut]);
        self.cursor = caret + cut;
        // 一部しか入らなかったときも「入り切らなかった」と申告する
        cut == filtered.len()
    }

    /// キャレットの手前 1 文字を消す
    pub(crate) fn backspace(&mut self) {
        let caret = self.normalize_caret();
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
        let caret = self.normalize_caret();
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
        let caret = self.normalize_caret();
        self.cursor = self.text[..caret]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
    }

    pub(crate) fn move_right(&mut self) {
        let caret = self.normalize_caret();
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

    /// #494: バイトオフセットを文字境界へ丸める（丸めないと split_at / insert_str が panic する）。
    /// #1459 で `right_panel.rs` からここへ移した（丸めの実装はこのモジュールに 1 つだけ）
    #[test]
    fn 文字境界への丸め() {
        let s = "あa\u{1F600}";
        // 「あ」= 0..3、「a」= 3..4、「\u{1F600}」= 4..8
        assert_eq!(floor_char_boundary(s, 0), 0);
        assert_eq!(floor_char_boundary(s, 1), 0);
        assert_eq!(floor_char_boundary(s, 2), 0);
        assert_eq!(floor_char_boundary(s, 3), 3);
        assert_eq!(floor_char_boundary(s, 5), 4);
        assert_eq!(floor_char_boundary(s, 7), 4);
        // 範囲外は末尾へクランプ
        assert_eq!(floor_char_boundary(s, 99), s.len());
        // 丸めた位置で split しても panic しない
        for i in 0..=s.len() + 5 {
            let _ = s.split_at(floor_char_boundary(s, i));
        }
    }

    /// 日本語 + 絵文字（4 バイト）+ 結合文字の混在で 1 文字ずつ進み・消える。
    /// バイト数がすべて違うので、どれか 1 つでも `len_utf8` を取り違えると落ちる
    #[test]
    fn 日本語と絵文字と結合文字が1文字ずつ動く() {
        // 「あ」3 / 「\u{1F600}」4 / 「a」1 / 「\u{0301}」2（結合アクセント）
        let mut f = field("あ\u{1F600}a\u{0301}");
        assert_eq!(f.cursor(), 10);
        for expect in [8, 7, 3, 0] {
            f.move_left();
            assert_eq!(f.cursor(), expect, "左へ 1 文字ずつ進んでいない");
        }
        for expect in [3, 7, 8, 10] {
            f.move_right();
            assert_eq!(f.cursor(), expect, "右へ 1 文字ずつ進んでいない");
        }
        // 結合文字は独立した 1 文字として消える（`char` 単位の削除）
        f.backspace();
        assert_eq!(f.text(), "あ\u{1F600}a");
        f.backspace();
        assert_eq!(f.text(), "あ\u{1F600}");
        f.move_home();
        f.delete_forward();
        assert_eq!(f.text(), "\u{1F600}");
        f.delete_forward();
        assert!(f.text().is_empty());
    }

    /// 描画側が使う「丸めたキャレット」は状態を変えず、割れたオフセットでも panic しない
    #[test]
    fn 描画用のキャレットは丸めるが状態を変えない() {
        let mut f = field("あい");
        f.cursor = 4; // 「い」の途中（不正なオフセット）
        assert_eq!(f.caret(), 3);
        assert_eq!(f.cursor(), 4, "描画用の丸めが状態を書き換えている");
        assert_eq!(f.split_at_caret(), ("あ", "い"));
        // どのオフセットでも split が panic しない
        for i in 0..=f.text().len() + 3 {
            f.cursor = i;
            let _ = f.split_at_caret();
        }
        // 空でも端でも壊れない
        let empty = TextField::default();
        assert_eq!(empty.caret(), 0);
        assert_eq!(empty.split_at_caret(), ("", ""));
        let end = field("ab");
        assert_eq!(end.split_at_caret(), ("ab", ""));
        let mut head = field("ab");
        head.move_home();
        assert_eq!(head.split_at_caret(), ("", "ab"));
    }

    /// 復元（コミット失敗時の打ち直し回避）は本文ごと差し替えてキャレットを末尾へ置く
    #[test]
    fn set_textは末尾へキャレットを置く() {
        let mut f = field("あ");
        f.move_home();
        f.set_text("日本語のメッセージ");
        assert_eq!(f.text(), "日本語のメッセージ");
        assert_eq!(f.cursor(), "日本語のメッセージ".len());
        // 空へ戻しても端の操作で壊れない
        f.set_text("");
        f.backspace();
        f.delete_forward();
        f.move_left();
        f.move_right();
        assert!(f.text().is_empty());
        assert_eq!(f.cursor(), 0);
    }
}
