//! IME アンカー矩形の補正（抽象境界 B17）
//!
//! `EntityInputHandler::bounds_for_range` が返すキャレット矩形を、各 OS の IME が
//! どう解釈するかは**揃っていない**。tako はセル上端 origin + セル高の矩形を返すが、
//!
//! - **macOS**: `gpui_macos/src/window.rs` が矩形のまま Cocoa へ渡す。Cocoa は矩形として
//!   扱い、候補ウィンドウをその下辺へ付ける。補正は要らない
//! - **Windows**: GPUI が矩形を**垂直中心の 1 点へ潰してから** IMM32 へ渡す（下記）
//!
//! この差を吸収するのがこの境界。呼び出し側（`bounds_for_range` の実装）は
//! 「カーソルセルの矩形」を素直に組み立て、最後にここを通すだけでよい。
//!
//! ## Windows で潰される仕組み（gpui rev `cafbf4b5` で確認）
//!
//! `gpui_windows` は 2 経路とも同一の整数演算でキャレット矩形を POINT にする:
//!
//! ```text
//! y = (origin.y * scale) as i32 + ((height * scale) as i32 / 2)
//! ```
//!
//! - `events.rs:585-590`（WM_IME_STARTCOMPOSITION 経路）
//! - `window.rs:962-968`（`invalidate_character_coordinates` 経路）
//!
//! その POINT が `COMPOSITIONFORM`（`CFS_POINT`）と `CANDIDATEFORM`（`CFS_CANDIDATEPOS`）の
//! **両方の `ptCurrentPos`** に渡る。したがってセル高 h の矩形をそのまま返すと、
//! IME は必ず **h/2 だけ下**を指す（125% DPI・`line_height` 17.0 で 10 物理px ≒ 半行。#582 の実害）。
//!
//! ## 補正の考え方
//!
//! 潰され方が既知なので、**潰した結果が狙いの Y になる矩形**を逆算して返す。
//! `height = 0` にすると `(0 * scale) as i32 / 2 == 0` で潰しが恒等になり、
//! `origin.y` がそのままアンカーになる。**スケール係数に依存しない**のが効く
//! （DPI ごとの場合分けも、GPUI 側の整数丸めの再現も要らない）。
//!
//! ## それだけでは足りなかった話（#582 再発）
//!
//! アンカーを入力行の下端へ正しく合わせても、**画面下端に余白が無いと候補ウィンドウが
//! 入力行を覆う**。GPUI が渡す `CANDIDATEFORM` は `CFS_CANDIDATEPOS` = **点だけ**で、
//! 「この矩形は覆うな」を伝えられないため、下に収まらないと IME は
//! **その点に候補ウィンドウの下端を合わせて上へ反転**するからである。
//! ターミナルの入力行はペイン下端にあるので、最大化して使うとこれが常態になる。
//!
//! 対処が [`set_candidate_exclusion`]（`CFS_EXCLUDE`）。基準点は今までと同じ値のまま、
//! 未確定文字列の矩形を「覆ってはいけない領域」として足す。
//! 実測（1920x1080 @ 125%・入力行のセルが物理 y=930..951）:
//!
//! | | 候補ウィンドウ | 入力行 |
//! |---|---|---|
//! | `CFS_CANDIDATEPOS` のみ | y=662..948 | **覆う** |
//! | `CFS_EXCLUDE` あり | y=642..928 | 覆わない |
//!
//! ## GPUI を rev bump するときの注意
//!
//! この補正は「GPUI が `y + height/2` に潰す」という**上流の実装への対抗**である。
//! 上流がこれを矩形のまま扱うよう修正したら、この補正は二重補正になって
//! **1 行ぶん下**へずれる。rev bump 時は `gpui_windows/src/window.rs` の
//! `update_ime_position` と `events.rs` の `retrieve_caret_position` を確認し、
//! `height / 2` が消えていたら `windows_anchor_rect_y` を恒等へ戻すこと。
//! 検知用に `collapse_to_ime_point_y` へ上流と同じ式を複製してある。

/// IME アンカーの基準をセルのどこに置くか。
///
/// Windows の慣例は**行の下端**。`CFS_CANDIDATEPOS` は候補ウィンドウをその点から
/// **下へ展開する**ため、行の上端や中央を渡すと候補ウィンドウが変換中の行に被る
/// （メモ帳・Windows Terminal はいずれも行の下端に候補ウィンドウの上端が付く）。
///
/// 実機の見た目で詰めるときはここを差し替える。単体テストが両基準の数値を
/// 固定してあるので、期待値の付け替えも 1 箇所で済む。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorBasis {
    /// セル上端。カーソルの真横に候補ウィンドウの上端が来る
    CellTop,
    /// セル下端（既定）。入力行のすぐ下に候補ウィンドウの上端が来る
    CellBottom,
}

/// このプラットフォームで採る基準。
///
/// macOS は矩形をそのまま渡すので基準の概念自体が無い（Cocoa が下辺を使う）。
/// ここは Windows の補正にだけ効く
pub const DEFAULT_ANCHOR_BASIS: AnchorBasis = AnchorBasis::CellBottom;

/// カーソルセルの矩形（の縦方向）を、この OS の IME が正しく解釈できる形へ補正する。
///
/// 引数・戻り値はどちらも論理ピクセルの `(origin_y, height)`。
/// `cell_height` は**ペイン単位**のセル高を渡すこと（ペインごとにフォントサイズが
/// 違ってよいので、テーマの `line_height` 決め打ちにしない）。
///
/// macOS では**恒等**（引数をそのまま返す）。Cocoa は矩形を矩形として扱うため、
/// 補正すると逆に狂う
pub fn anchor_rect_y(origin_y: f32, cell_height: f32) -> (f32, f32) {
    imp::anchor_rect_y(origin_y, cell_height, DEFAULT_ANCHOR_BASIS)
}

#[cfg(not(windows))]
mod imp {
    use super::AnchorBasis;

    /// macOS（および非 Windows）は補正しない。矩形のまま IME へ渡るのが正
    pub(super) fn anchor_rect_y(
        origin_y: f32,
        cell_height: f32,
        _basis: AnchorBasis,
    ) -> (f32, f32) {
        (origin_y, cell_height)
    }
}

#[cfg(windows)]
mod imp {
    use super::AnchorBasis;

    pub(super) fn anchor_rect_y(origin_y: f32, cell_height: f32, basis: AnchorBasis) -> (f32, f32) {
        super::windows_anchor_rect_y(origin_y, cell_height, basis)
    }
}

/// Windows 向けの補正本体（純粋関数。**macOS 上でもテストできる**ようにしてある）。
///
/// 狙いの Y を `origin.y` に置き、`height` を 0 にして GPUI の
/// 「`origin.y + height/2`」を恒等化する
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_anchor_rect_y(origin_y: f32, cell_height: f32, basis: AnchorBasis) -> (f32, f32) {
    // セル高が負・非有限なら補正のしようがない。素の値を返して
    // 「ずれるが壊れはしない」側に倒す（IME の位置決めのために描画を壊さない）
    if !cell_height.is_finite() || cell_height < 0.0 {
        return (origin_y, cell_height);
    }
    let anchor_y = match basis {
        AnchorBasis::CellTop => origin_y,
        AnchorBasis::CellBottom => origin_y + cell_height,
    };
    (anchor_y, 0.0)
}

/// 未確定文字列が画面上で占める矩形（論理ピクセル・ウィンドウ client 座標）。
///
/// 「候補ウィンドウがここに被ってはいけない」という**除外領域**を表す。
/// 呼び出し側は変換中の文字列の描画矩形をそのまま組めばよい
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompositionRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// IMM32 の `CANDIDATEFORM`（`CFS_EXCLUDE`）へ渡す値（物理ピクセル・client 座標）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateExclusion {
    /// 候補ウィンドウの左上を置きたい点（入力行の下端左）
    pub pt: (i32, i32),
    /// 覆ってはいけない矩形（left, top, right, bottom）
    pub area: (i32, i32, i32, i32),
}

/// 除外領域つきの候補ウィンドウ位置を組む（純粋関数。**macOS 上でもテストできる**）。
///
/// 物理ピクセルへの丸めは GPUI と同じ切り捨て（`as i32`）にしてある。
/// `pt` が `anchor_rect_y` 経由で GPUI が押し込む点と**同じ値**になり、
/// 「余白があるときの位置」は今までと 1px も変わらない
pub fn candidate_exclusion(
    rect: &CompositionRect,
    scale_factor: f32,
) -> Option<CandidateExclusion> {
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return None;
    }
    if !(rect.left.is_finite()
        && rect.top.is_finite()
        && rect.right.is_finite()
        && rect.bottom.is_finite())
    {
        return None;
    }
    let left = (rect.left * scale_factor) as i32;
    let top = (rect.top * scale_factor) as i32;
    // 潰れた矩形を渡すと IME 側が「除外領域なし」と解釈して被りが戻るので、
    // 最低 1px は厚みを持たせる（右下は左上より必ず大きい）
    let right = ((rect.right * scale_factor) as i32).max(left + 1);
    let bottom = ((rect.bottom * scale_factor) as i32).max(top + 1);
    Some(CandidateExclusion {
        pt: (left, bottom),
        area: (left, top, right, bottom),
    })
}

/// 候補ウィンドウが未確定文字列に被らないよう、IME へ除外領域を通知する。
///
/// `window_handle` は Windows の `HWND`（他 OS では `None` を渡す）。
/// 通知できたら `true`。
///
/// ## なぜ要るか（#582 再発）
///
/// GPUI Windows は `CANDIDATEFORM` に **`CFS_CANDIDATEPOS`（点だけ）** を渡す。
/// 点しか無いと、候補ウィンドウが画面下端に収まらないとき IME は
/// **その点に候補ウィンドウの下端を合わせて上へ反転**する。実測（1920x1080 @ 125%）:
/// 入力行のセルが物理 y=930..951 のとき候補ウィンドウは y=662..948 に出て、
/// 入力行と未確定文字列を丸ごと覆った。ターミナルの入力行はペイン下端にあるので、
/// 最大化して使っていると**これが常態**になる。
///
/// `CFS_EXCLUDE` は「`ptCurrentPos` に出すが `rcArea` は覆うな」という指定で、
/// 反転先が `rcArea` の**上**になる。macOS は Cocoa が矩形から自動で避けるため何もしない
pub fn set_candidate_exclusion(
    window_handle: Option<isize>,
    rect: &CompositionRect,
    scale_factor: f32,
) -> bool {
    let Some(handle) = window_handle else {
        return false;
    };
    let Some(form) = candidate_exclusion(rect, scale_factor) else {
        return false;
    };
    exclude_imp::set_candidate_exclusion(handle, form)
}

#[cfg(not(windows))]
mod exclude_imp {
    use super::CandidateExclusion;

    /// macOS（および非 Windows）は IMM32 が無い。呼ばれても何もしない
    pub(super) fn set_candidate_exclusion(_handle: isize, _form: CandidateExclusion) -> bool {
        false
    }
}

#[cfg(windows)]
mod exclude_imp {
    use super::CandidateExclusion;

    /// `CFS_EXCLUDE`（imm.h）。`ptCurrentPos` に出しつつ `rcArea` を覆わせない
    const CFS_EXCLUDE: u32 = 0x0080;

    #[repr(C)]
    struct PointI32 {
        x: i32,
        y: i32,
    }
    #[repr(C)]
    struct RectI32 {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    /// `CANDIDATEFORM`（imm.h）。`repr(C)` で C と同じレイアウトになる
    #[repr(C)]
    struct CandidateForm {
        dw_index: u32,
        dw_style: u32,
        pt_current_pos: PointI32,
        rc_area: RectI32,
    }

    // `windows-sys` を足さず必要な 3 関数だけ宣言する方針は
    // `tako-control::agents` の Toolhelp32 / sleep_guard の IOKit と同じ
    #[link(name = "imm32")]
    unsafe extern "system" {
        fn ImmGetContext(hwnd: isize) -> isize;
        fn ImmReleaseContext(hwnd: isize, himc: isize) -> i32;
        fn ImmSetCandidateWindow(himc: isize, form: *const CandidateForm) -> i32;
    }

    pub(super) fn set_candidate_exclusion(handle: isize, form: CandidateExclusion) -> bool {
        let payload = CandidateForm {
            // GPUI が押し込むのと同じ 0 番。別番号にすると上書きにならない
            dw_index: 0,
            dw_style: CFS_EXCLUDE,
            pt_current_pos: PointI32 {
                x: form.pt.0,
                y: form.pt.1,
            },
            rc_area: RectI32 {
                left: form.area.0,
                top: form.area.1,
                right: form.area.2,
                bottom: form.area.3,
            },
        };
        // SAFETY: HWND は GPUI が生きている間有効。IMC は取得直後に妥当性を検査し、
        // 復帰経路すべてで ImmReleaseContext する。payload はこの関数より長生きしない
        unsafe {
            let himc = ImmGetContext(handle);
            if himc == 0 {
                // IME が無効化されている（入力を受け付けないウィンドウ）
                return false;
            }
            let ok = ImmSetCandidateWindow(himc, &payload) != 0;
            ImmReleaseContext(handle, himc);
            ok
        }
    }
}

/// GPUI Windows がキャレット矩形を IMM32 の POINT へ潰すときの Y（物理ピクセル）。
///
/// **上流（`gpui_windows`）と同じ整数演算を意図的に複製している**。
/// 単体テストが「補正後の矩形を GPUI が潰した結果」を数値で固定できるようにするためで、
/// 製品コードからは呼ばない。上流の式が変わったらここも合わせて更新し、
/// テストの期待値で二重補正に気づけるようにする
#[cfg_attr(not(test), allow(dead_code))]
fn collapse_to_ime_point_y(origin_y: f32, height: f32, scale_factor: f32) -> i32 {
    (origin_y * scale_factor) as i32 + ((height * scale_factor) as i32 / 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ユーザー実機（1920x1080 @ 125%）と既定テーマの `line_height`
    const SCALE: f32 = 1.25;
    const CELL_H: f32 = 17.0;
    const CELL_TOP: f32 = 100.0;

    /// 補正なし（修正前の実装＝セル上端 origin + セル高の矩形）だと、
    /// GPUI に潰された結果が**セル中央**を指してしまうことの固定。
    ///
    /// このテストが「修正前の値」を示す対照であり、下の
    /// `補正後はセル下端を指す` が修正後の値を示す
    #[test]
    fn 補正なしだと半行下のセル中央を指してしまう() {
        let got = collapse_to_ime_point_y(CELL_TOP, CELL_H, SCALE);
        // (100 * 1.25) as i32 + ((17 * 1.25) as i32 / 2) = 125 + (21 / 2) = 125 + 10
        assert_eq!(got, 135);

        // セル上端は 125 物理px、下端は 146 物理px。135 はそのちょうど中間付近 = 半行下
        let cell_top_phys = (CELL_TOP * SCALE) as i32;
        let cell_bottom_phys = ((CELL_TOP + CELL_H) * SCALE) as i32;
        assert_eq!((cell_top_phys, cell_bottom_phys), (125, 146));
        assert!(got > cell_top_phys && got < cell_bottom_phys);
    }

    /// 受け入れ条件 1: 補正後の矩形を GPUI が `y + height/2` で潰した結果が狙いの Y になる
    #[test]
    fn 補正後はセル下端を指す() {
        let (y, h) = windows_anchor_rect_y(CELL_TOP, CELL_H, AnchorBasis::CellBottom);
        let got = collapse_to_ime_point_y(y, h, SCALE);

        // 狙い = セル下端（117.0 論理px → 146 物理px）
        assert_eq!(got, ((CELL_TOP + CELL_H) * SCALE) as i32);
        assert_eq!(got, 146);
        // 修正前の 135（半行下）とは別の値であること
        assert_ne!(got, collapse_to_ime_point_y(CELL_TOP, CELL_H, SCALE));
    }

    /// 実機の見た目調整で上端基準へ倒したときの数値も固定しておく
    #[test]
    fn 上端基準ならセル上端を指す() {
        let (y, h) = windows_anchor_rect_y(CELL_TOP, CELL_H, AnchorBasis::CellTop);
        let got = collapse_to_ime_point_y(y, h, SCALE);
        assert_eq!(got, (CELL_TOP * SCALE) as i32);
        assert_eq!(got, 125);
    }

    /// `height = 0` にした狙い: 潰しが恒等になり **DPI に依存しない**。
    /// 100% / 125% / 150% / 200% のどれでもセル下端ちょうどを指すこと
    #[test]
    fn どのdpiでもセル下端を指す() {
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            let (y, h) = windows_anchor_rect_y(CELL_TOP, CELL_H, AnchorBasis::CellBottom);
            let got = collapse_to_ime_point_y(y, h, scale);
            assert_eq!(
                got,
                ((CELL_TOP + CELL_H) * scale) as i32,
                "scale={scale} で狙いの Y からずれた"
            );
        }
    }

    /// ペインごとにフォントサイズが違ってよい（`cell_size_for_pane` はペイン単位）。
    /// セル高が変わってもその下端を指すこと
    #[test]
    fn ペイン単位のセル高に追従する() {
        for cell_h in [12.0_f32, 17.0, 24.0, 33.5] {
            let (y, h) = windows_anchor_rect_y(CELL_TOP, cell_h, AnchorBasis::CellBottom);
            let got = collapse_to_ime_point_y(y, h, SCALE);
            assert_eq!(
                got,
                ((CELL_TOP + cell_h) * SCALE) as i32,
                "cell_h={cell_h} で狙いの Y からずれた"
            );
        }
    }

    /// 補正は **`origin_y` に対する一様なシフト**でなければならない。
    ///
    /// GPUI の `compute_ime_candidate_bounds`（`gpui/src/platform.rs`）は
    /// 複数レンジの `bounds_for_range` を呼んで `origin.y` の**差**が 0.1px を超えるかで
    /// 行の折り返しを検出する。一様シフトなら差が保存されるのでこの検出を壊さない
    #[test]
    fn 補正はy座標の差を保存する() {
        let (a, _) = windows_anchor_rect_y(100.0, CELL_H, AnchorBasis::CellBottom);
        let (b, _) = windows_anchor_rect_y(117.0, CELL_H, AnchorBasis::CellBottom);
        assert!(
            ((b - a) - 17.0).abs() < f32::EPSILON,
            "行間の差 17.0 が保存されていない: {a} -> {b}"
        );
    }

    /// 実機（1920x1080 @ 125%）で計測した値をそのまま固定する。
    ///
    /// 計測時の実値: カーソルセル左上 = 論理 (155.9, 104.2)、セル 7.6x17.0、
    /// 未確定文字列「にほんご」の描画幅 41.3 論理px。
    /// GPUI が `CFS_CANDIDATEPOS` へ押し込んだ点は client 物理 (194, 151) で、
    /// セルの物理上端 130 / 下端 151 と一致していた（IMM32 の読み戻しで確認）
    #[test]
    fn 除外矩形は実機のセル物理位置と一致する() {
        let rect = CompositionRect {
            left: 155.9,
            top: 104.2,
            right: 155.9 + 41.3,
            bottom: 104.2 + 17.0,
        };
        let got = candidate_exclusion(&rect, SCALE).expect("正常値では組める");
        // 候補ウィンドウを置きたい点は GPUI が押し込むアンカーと同じ = 余白があるときの
        // 位置が変わらないことの担保
        assert_eq!(got.pt, (194, 151));
        // 覆わせない矩形はカーソルセルの実ピクセル位置そのもの
        assert_eq!(got.area, (194, 130, 246, 151));
    }

    /// `pt` は `anchor_rect_y` 経由で GPUI が押し込む点と**必ず同じ**でなければならない。
    /// ずれると「余白があるとき」の見た目が変わってしまう
    #[test]
    fn 除外指定の基準点はgpuiのアンカーと一致する() {
        for (top, cell_h) in [(104.2_f32, 17.0_f32), (0.0, 12.0), (700.5, 24.0)] {
            let (anchor_y, anchor_h) = windows_anchor_rect_y(top, cell_h, AnchorBasis::CellBottom);
            let pushed = collapse_to_ime_point_y(anchor_y, anchor_h, SCALE);
            let rect = CompositionRect {
                left: 10.0,
                top,
                right: 60.0,
                bottom: top + cell_h,
            };
            let got = candidate_exclusion(&rect, SCALE).expect("正常値では組める");
            assert_eq!(got.pt.1, pushed, "top={top} cell_h={cell_h}");
        }
    }

    /// 全角と半角が混ざった行でも、除外矩形は実描画幅（呼び出し側が shaping で出す）を
    /// そのまま物理へ写すだけであること
    #[test]
    fn 除外矩形は実描画幅をそのまま物理へ写す() {
        for width in [7.6_f32, 41.3, 120.0] {
            let rect = CompositionRect {
                left: 100.0,
                top: 200.0,
                right: 100.0 + width,
                bottom: 217.0,
            };
            let got = candidate_exclusion(&rect, SCALE).expect("正常値では組める");
            assert_eq!(got.area.0, 125);
            assert_eq!(
                got.area.2,
                ((100.0 + width) * SCALE) as i32,
                "width={width}"
            );
        }
    }

    /// 潰れた矩形を渡すと IME 側が「除外領域なし」と解釈して被りが戻るので、
    /// 最低 1px の厚みを保証する
    #[test]
    fn 潰れた矩形でも厚みを保つ() {
        let rect = CompositionRect {
            left: 100.0,
            top: 200.0,
            right: 100.0,
            bottom: 200.0,
        };
        let got = candidate_exclusion(&rect, SCALE).expect("正常値では組める");
        assert!(got.area.2 > got.area.0 && got.area.3 > got.area.1);
    }

    /// どの DPI でも除外矩形はセルの物理位置そのものになる（自前の丸めを持ち込まない）
    #[test]
    fn 除外矩形はどのdpiでもセルの物理位置() {
        let rect = CompositionRect {
            left: 100.0,
            top: 200.0,
            right: 140.0,
            bottom: 217.0,
        };
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            let got = candidate_exclusion(&rect, scale).expect("正常値では組める");
            assert_eq!(
                got.area,
                (
                    (100.0 * scale) as i32,
                    (200.0 * scale) as i32,
                    (140.0 * scale) as i32,
                    (217.0 * scale) as i32
                ),
                "scale={scale}"
            );
        }
    }

    /// 異常値では組まない（IME の位置決めのために不正な矩形を渡さない）
    #[test]
    fn 異常値では除外矩形を組まない() {
        let ok = CompositionRect {
            left: 0.0,
            top: 0.0,
            right: 10.0,
            bottom: 10.0,
        };
        assert!(candidate_exclusion(&ok, 0.0).is_none(), "scale 0 は不正");
        assert!(
            candidate_exclusion(&ok, -1.0).is_none(),
            "負のスケールは不正"
        );
        let nan = CompositionRect {
            left: f32::NAN,
            ..ok
        };
        assert!(candidate_exclusion(&nan, SCALE).is_none());
    }

    /// ハンドルが無い（macOS）なら通知しない。`cfg` を書かずに呼び出せることの担保
    #[test]
    fn ハンドルなしでは通知しない() {
        let rect = CompositionRect {
            left: 0.0,
            top: 0.0,
            right: 10.0,
            bottom: 10.0,
        };
        assert!(!set_candidate_exclusion(None, &rect, SCALE));
    }

    /// 異常なセル高で補正が暴走しないこと（描画より IME の位置決めを優先させない）
    #[test]
    fn 異常なセル高では補正しない() {
        assert_eq!(
            windows_anchor_rect_y(CELL_TOP, f32::NAN, AnchorBasis::CellBottom).0,
            CELL_TOP
        );
        assert_eq!(
            windows_anchor_rect_y(CELL_TOP, -5.0, AnchorBasis::CellBottom),
            (CELL_TOP, -5.0)
        );
    }

    /// 受け入れ条件 4: macOS の挙動が変わらないことを**構造で**担保する。
    /// 非 Windows では境界が恒等（＝呼び出し側が組み立てた矩形がそのまま IME へ行く）
    #[test]
    fn 非windowsでは補正が恒等() {
        let got = anchor_rect_y(CELL_TOP, CELL_H);
        #[cfg(not(windows))]
        assert_eq!(got, (CELL_TOP, CELL_H), "macOS では矩形を変えてはいけない");
        #[cfg(windows)]
        assert_eq!(got, (CELL_TOP + CELL_H, 0.0));
        let _ = got;
    }
}
