//! マーカーで囲んだ管理ブロックの読み書き（プラットフォーム非依存の純粋関数）
//!
//! ユーザーが所有するテキストファイル（シェルの rc / `$PROFILE`）へ tako の設定を
//! 差し込むときの共通規則を 1 か所に置く。**扱いは常にバイト列**で、`String` へ
//! lossy 変換して書き戻すことはしない。ユーザーのファイルは UTF-8 とは限らず
//! （BOM 無しの `.ps1` は Windows PowerShell 5.1 では ANSI = 日本語環境なら CP932）、
//! 一度でも変換すると**ブロックの外側を壊す**。マーカーとブロック本文を ASCII に
//! 限れば、バイトのまま切った貼ったで元の符号を保てる。
//!
//! ## 不変条件（この 1 か所でだけ定義する）
//!
//! 追記時に足す区切りは**常に改行 1 個**（空ファイルなら 0 個）。これを守ると
//! [`BlockMarkers::remove`] が「ブロック + 直前の改行 1 個」を消すだけで
//! **元のバイト列へ完全に戻せる**（元ファイルが改行で終わっていてもいなくても）。
//!
//! この規則を 2 か所に書くと必ずドリフトするので、
//! [`crate::shell_integration`]（PowerShell）と [`crate::shell_profile`]（PATH 通し）は
//! どちらもここへ委譲する。

/// 管理ブロックの開始・終了マーカー。**どちらも ASCII で書くこと**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockMarkers {
    pub begin: &'static str,
    pub end: &'static str,
}

impl BlockMarkers {
    pub const fn new(begin: &'static str, end: &'static str) -> Self {
        Self { begin, end }
    }

    /// ブロックの位置（開始バイト, 終了バイト = 末尾改行を含む）
    pub fn find(&self, text: &[u8]) -> Option<(usize, usize)> {
        let begin = find_bytes(text, self.begin.as_bytes())?;
        let end_marker = find_bytes(&text[begin..], self.end.as_bytes())? + begin;
        let after = end_marker + self.end.len();
        // 終端マーカー行の改行まで飲む（無ければ EOF）
        let end = match find_bytes(&text[after..], b"\n") {
            Some(nl) => after + nl + 1,
            None => text.len(),
        };
        Some((begin, end))
    }

    /// ブロックが入っているか
    pub fn present(&self, text: &[u8]) -> bool {
        self.find(text).is_some()
    }

    /// 入っているブロックの本文（マーカー行を含む）
    pub fn extract<'a>(&self, text: &'a [u8]) -> Option<&'a [u8]> {
        let (begin, end) = self.find(text)?;
        Some(&text[begin..end])
    }

    /// ブロックを配置した結果のファイル内容（あれば置換、無ければ追記）
    pub fn apply(&self, original: &[u8], block: &str) -> Vec<u8> {
        let block = block.as_bytes();
        if let Some((begin, end)) = self.find(original) {
            let mut out = Vec::with_capacity(original.len() + block.len());
            out.extend_from_slice(&original[..begin]);
            out.extend_from_slice(block);
            out.extend_from_slice(&original[end..]);
            return out;
        }
        if original.is_empty() {
            return block.to_vec();
        }
        let mut out = Vec::with_capacity(original.len() + block.len() + 1);
        out.extend_from_slice(original);
        out.push(b'\n');
        out.extend_from_slice(block);
        out
    }

    /// ブロックを取り除いた結果のファイル内容。[`Self::apply`] が足した改行も戻す
    pub fn remove(&self, current: &[u8]) -> Vec<u8> {
        let Some((begin, end)) = self.find(current) else {
            return current.to_vec();
        };
        let mut head = &current[..begin];
        // apply が追記したときの区切り改行 1 個ぶんを戻す
        if let Some((b'\n', rest)) = head.split_last() {
            head = rest;
        }
        let mut out = Vec::with_capacity(head.len() + current.len() - end);
        out.extend_from_slice(head);
        out.extend_from_slice(&current[end..]);
        out
    }
}

/// バイト列中の部分列を探す（マーカーはすべて ASCII なので符号を問わない）
fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: BlockMarkers = BlockMarkers::new("# >>> begin >>>", "# <<< end <<<");

    fn block() -> String {
        format!("{}\nbody\n{}\n", M.begin, M.end)
    }

    #[test]
    fn 空ファイルへは区切り改行を足さない() {
        assert_eq!(M.apply(b"", &block()), block().into_bytes());
    }

    #[test]
    fn 追記と除去で元のバイト列へ完全に戻る() {
        // 末尾改行あり / なし / 空 のいずれでも往復すること
        for original in [&b"head\n"[..], &b"head"[..], &b""[..]] {
            let installed = M.apply(original, &block());
            assert!(M.present(&installed), "配置後にブロックが見つからない");
            assert_eq!(
                M.remove(&installed),
                original,
                "往復で元のバイト列に戻っていない: {original:?}"
            );
        }
    }

    #[test]
    fn 二回適用してもブロックは一個() {
        let once = M.apply(b"head\n", &block());
        let twice = M.apply(&once, &block());
        assert_eq!(once, twice);
        assert_eq!(M.remove(&twice), b"head\n");
    }

    #[test]
    fn 中身を差し替えても前後は保たれる() {
        let installed = M.apply(b"head\n", &block());
        let updated = M.apply(&installed, &format!("{}\nnew\n{}\n", M.begin, M.end));
        assert!(updated.starts_with(b"head\n"));
        assert!(String::from_utf8_lossy(&updated).contains("new"));
        assert!(!String::from_utf8_lossy(&updated).contains("body"));
        assert_eq!(M.remove(&updated), b"head\n");
    }

    #[test]
    fn 非utf8のバイト列を壊さない() {
        // CP932 の「あ」= 0x82 0xA0。lossy 変換すると壊れる
        let original = b"head\n\x82\xa0\n";
        let installed = M.apply(original, &block());
        assert_eq!(M.remove(&installed), original);
    }

    #[test]
    fn ブロックが無ければ除去は無変更() {
        assert_eq!(M.remove(b"head\n"), b"head\n");
    }

    /// 終端マーカーが行末改行なしで EOF に接していても、ブロックとして切り出せること。
    /// 除去結果が `head\n` ではなく `head` なのは仕様どおり:
    /// [`BlockMarkers::remove`] は [`BlockMarkers::apply`] が足す区切り改行 1 個を
    /// 戻すので、手で 1 個だけ改行を置いた入力ではその 1 個が消える
    /// （`apply` を通した往復は別テストで担保している）
    #[test]
    fn 終端マーカーに改行が無くても切り出せる() {
        let text = format!("head\n{}\nbody\n{}", M.begin, M.end);
        assert!(M.present(text.as_bytes()));
        assert_eq!(M.remove(text.as_bytes()), b"head");
    }

    #[test]
    fn extractはマーカー行を含む本文を返す() {
        let installed = M.apply(b"head\n", &block());
        let extracted = M.extract(&installed).expect("本文が取れること");
        assert_eq!(extracted, block().as_bytes());
    }
}
