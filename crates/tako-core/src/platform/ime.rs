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
