//! fnv — FNV-1a 64bit（安定ハッシュの 1 実装）
//!
//! 用途はどれも「短くて再現する名前がほしい」であって暗号学的強度ではない
//! （id・検証子・ソケット名）。`DefaultHasher` を使わないのは、
//! **std の版が変わると値が変わりうる**と std 自身が明記しているため。
//! ソケット名（`ipc_socket::short_name`）は**再起動をまたいで同じ値**である
//! ことが存在理由なので、ここは自前の固定式で持つ。

/// FNV-1a 64bit。同じ入力は未来の版でも同じ値を返す
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 既知のベクタと一致する() {
        // FNV-1a 64bit の公開テストベクタ
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x85944171f73967e8);
    }

    #[test]
    fn 入力が違えば値が違う() {
        assert_ne!(fnv1a64(b"/tmp/a"), fnv1a64(b"/tmp/b"));
    }
}
