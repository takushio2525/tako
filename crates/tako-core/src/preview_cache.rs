//! プレビュー画像キャッシュの設定と統計（Issue #258 → #2009）。
//!
//! #2009: 上限（`max_mb`）は**天井**として残し、実際に効かせる上限（実効上限）は
//! 「いま見えているプレビューが要る量」に合わせて決める。見えていないプレビュー
//! （裏のタブ・退避中）は [`HIDDEN_RELEASE_AFTER_SECS`] 秒でデコード結果・ラスタを手放し、
//! 表に戻したら作り直す。判断はこのファイルの関数 1 本ずつに寄せ、GUI はそれを呼ぶだけ。

use std::sync::OnceLock;

pub const PREVIEW_CACHE_DEFAULT_MB: u64 = 512;
pub const PREVIEW_CACHE_MIN_MB: u64 = 256;
pub const PREVIEW_CACHE_MAX_MB: u64 = 8_192;

/// 見えていないプレビューのデコード結果・ラスタを手放すまでの猶予（秒。#2009）。
///
/// タブを行き来するたびに作り直さない長さ（往復はたいてい数秒〜十数秒）で、
/// 裏に回したまま忘れたものは 1 分以内に返る長さ
pub const HIDDEN_RELEASE_AFTER_SECS: u64 = 30;

/// 実効上限の下限（MiB。#2009）。見えている量が小さくても、表示中の PDF を
/// 少し戻る・タブを往復するぶんのデコード済みページは持つ
pub const EFFECTIVE_FLOOR_MB: u64 = 128;

/// 実効上限は「見えている量 × この倍率」（#2009）。表示中のページの前後と、
/// 直前に見ていたページを持てる余裕
pub const EFFECTIVE_VISIBLE_FACTOR: u64 = 2;

/// プレビュー 1 枚ぶんのメモリの持ち分（#2009。`tako preview-cache` の `panes`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewPaneMemory {
    pub pane: u64,
    /// "pdf" / "image" / "video"
    pub kind: &'static str,
    /// いずれかのウィンドウ・ピン・ホバー・ドロワーに描かれているか
    pub visible: bool,
    /// 見えなくなってからの秒数（見えているなら None）
    pub hidden_secs: Option<u64>,
    /// LRU に計上しているデコード済み画像の bytes（GPU テクスチャと同じ量）
    pub decoded_bytes: u64,
    /// 圧縮のまま抱えている元データの bytes（PDF のページ PNG・画像ファイル・動画のサムネ）
    pub source_bytes: u64,
    /// 見えていない間に手放したか（表に戻したら作り直す）
    pub released: bool,
    /// 動画プレイヤー（AVPlayer）を抱えているか
    pub video_player: bool,
    /// 動画の現在フレーム（CPU の BGRA と描画用の画像）の bytes
    pub video_frame_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewCacheStats {
    /// 上限（天井。設定値。`settings.json` に残る）
    pub max_bytes: u64,
    pub used_bytes: u64,
    pub entries: usize,
    /// 実際に効かせている上限（#2009。見えている量から [`effective_budget_bytes`] で決める）
    pub effective_max_bytes: u64,
    /// LRU のうち見えているプレビューのぶん / 見えていないぶん（#2009）
    pub visible_bytes: u64,
    pub hidden_bytes: u64,
    /// 圧縮のまま抱えている元データの合計（#2009）
    pub source_bytes: u64,
    /// 手放すまでの猶予（秒。#2009）
    pub release_after_secs: u64,
    /// 起動してから手放した回数 / 表に戻して作り直した回数（#2009。単調増加）
    pub released_total: u64,
    pub restored_total: u64,
    /// A/B（`TAKO_2009_LEGACY=1`）で修正前の挙動か
    pub legacy: bool,
    pub panes: Vec<PreviewPaneMemory>,
}

impl PreviewCacheStats {
    /// GUI を持たないホスト（テスト・CLI 単体）の既定値
    pub fn idle(max_bytes: u64) -> Self {
        Self {
            max_bytes,
            used_bytes: 0,
            entries: 0,
            effective_max_bytes: max_bytes,
            visible_bytes: 0,
            hidden_bytes: 0,
            source_bytes: 0,
            release_after_secs: HIDDEN_RELEASE_AFTER_SECS,
            released_total: 0,
            restored_total: 0,
            legacy: issue2009_legacy(),
            panes: Vec::new(),
        }
    }
}

/// 上限（MB）を検証して bytes へ。範囲外の断りは表示言語（`setting_range::RangeRefusal`。#2365）
pub fn preview_cache_bytes(max_mb: u64) -> Result<u64, String> {
    if !(PREVIEW_CACHE_MIN_MB..=PREVIEW_CACHE_MAX_MB).contains(&max_mb) {
        return Err(crate::setting_range::RangeRefusal::PreviewCacheMb.to_string());
    }
    Ok(max_mb * 1024 * 1024)
}

/// 実効上限（#2009）: 見えている量の [`EFFECTIVE_VISIBLE_FACTOR`] 倍、ただし
/// [`EFFECTIVE_FLOOR_MB`] を下回らず、設定の上限（天井）を超えない。
///
/// **見えている量そのものは必ず収まる**（天井まで）: 1 枚がこれを超えて LRU が
/// 入れた直後に追い出すと、表示中の画像が描けないまま作り直しを繰り返す
pub fn effective_budget_bytes(configured: u64, visible_need: u64) -> u64 {
    let floor = EFFECTIVE_FLOOR_MB * 1024 * 1024;
    visible_need
        .saturating_mul(EFFECTIVE_VISIBLE_FACTOR)
        .max(floor)
        .min(configured)
}

/// 見えなくなってから `hidden_secs` 秒たったプレビューを手放す時か（#2009）
pub fn release_due(hidden_secs: u64) -> bool {
    hidden_secs >= HIDDEN_RELEASE_AFTER_SECS
}

/// PDF で手放さずに残すページ（#2009）: 表示中のページとその前後。
/// 表に戻した最初のフレームで表示中のページを再ラスタ待ちにしないため
pub fn pdf_pages_to_keep(current: usize, total: usize) -> std::ops::Range<usize> {
    if total == 0 {
        return 0..0;
    }
    let current = current.min(total - 1);
    current.saturating_sub(1)..(current + 2).min(total)
}

/// #2009 の A/B（`TAKO_2009_LEGACY=1`）。同一バイナリのまま修正前の形へ戻す:
/// 見えていないプレビューも手放さない・実効上限を設定値に固定する。
/// 前後を同じ構成で並べて測るためのもの（製品の既定は常に新しい形）。**ゲートはこの 1 か所**
pub fn issue2009_legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_2009_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1024 * 1024;

    #[test]
    fn キャッシュ上限をmbからbytesへ検証変換する() {
        assert_eq!(preview_cache_bytes(512), Ok(512 * 1024 * 1024));
        assert!(preview_cache_bytes(255).is_err());
        assert!(preview_cache_bytes(8_193).is_err());
    }

    #[test]
    fn 実効上限は見えている量の2倍で下限と天井に収まる() {
        // 何も見えていない = 下限
        assert_eq!(effective_budget_bytes(512 * MB, 0), EFFECTIVE_FLOOR_MB * MB);
        // 小さい = 下限
        assert_eq!(
            effective_budget_bytes(512 * MB, 20 * MB),
            EFFECTIVE_FLOOR_MB * MB
        );
        // 大きい = 2 倍
        assert_eq!(effective_budget_bytes(512 * MB, 100 * MB), 200 * MB);
        // 天井を超えない
        assert_eq!(effective_budget_bytes(512 * MB, 400 * MB), 512 * MB);
        // 見えている量そのものは天井まで必ず収まる
        for need in [0, 1, 64 * MB, 129 * MB, 300 * MB, 511 * MB, 900 * MB] {
            assert!(effective_budget_bytes(512 * MB, need) >= need.min(512 * MB));
        }
    }

    #[test]
    fn 猶予を過ぎたら手放す() {
        assert!(!release_due(0));
        assert!(!release_due(HIDDEN_RELEASE_AFTER_SECS - 1));
        assert!(release_due(HIDDEN_RELEASE_AFTER_SECS));
    }

    #[test]
    fn pdfは表示中のページと前後だけ残す() {
        assert_eq!(pdf_pages_to_keep(0, 0), 0..0);
        assert_eq!(pdf_pages_to_keep(0, 1), 0..1);
        assert_eq!(pdf_pages_to_keep(0, 60), 0..2);
        assert_eq!(pdf_pages_to_keep(10, 60), 9..12);
        assert_eq!(pdf_pages_to_keep(59, 60), 58..60);
        // 範囲外の現在ページは末尾へ丸める
        assert_eq!(pdf_pages_to_keep(99, 60), 58..60);
    }
}
