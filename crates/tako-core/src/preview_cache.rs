//! プレビュー画像キャッシュの設定と統計（Issue #258）。

pub const PREVIEW_CACHE_DEFAULT_MB: u64 = 512;
pub const PREVIEW_CACHE_MIN_MB: u64 = 256;
pub const PREVIEW_CACHE_MAX_MB: u64 = 8_192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewCacheStats {
    pub max_bytes: u64,
    pub used_bytes: u64,
    pub entries: usize,
}

pub fn preview_cache_bytes(max_mb: u64) -> Result<u64, String> {
    if !(PREVIEW_CACHE_MIN_MB..=PREVIEW_CACHE_MAX_MB).contains(&max_mb) {
        return Err(format!(
            "max_mb は {PREVIEW_CACHE_MIN_MB}〜{PREVIEW_CACHE_MAX_MB} の範囲で指定する"
        ));
    }
    Ok(max_mb * 1024 * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn キャッシュ上限をmbからbytesへ検証変換する() {
        assert_eq!(preview_cache_bytes(512), Ok(512 * 1024 * 1024));
        assert!(preview_cache_bytes(255).is_err());
        assert!(preview_cache_bytes(8_193).is_err());
    }
}
