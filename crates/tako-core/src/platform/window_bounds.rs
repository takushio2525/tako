//! 窓の初期位置・寸法を**外から**決める（Issue #1442）
//!
//! ## なぜ要るのか
//!
//! `TAKO_DISPLAY` を付けた隔離起動は、保存フレームを無視して**必ず置き先の中央
//! 960x600** で開いていた（#1141 の「保存位置より置き先が勝つ」）。検証で 2 つの窓を
//! 並べたい・別 worker の窓と重ねたくない、という要求に応える手段が無く、回避策として
//! System Events（AX）で `first process whose unix id is <隔離 pid>` を掴んで動かすと、
//! **AX が複数の tako-app を同一プロセスとして返すため本番 tako の窓が動いた**
//! （ユーザーの窓を動かす事故が実際に起きた = #1442 の症状 2）。
//!
//! そこで**窓の位置は tako 自身の口で指定する**。起動時は [`ENV_WINDOW_BOUNDS`]、
//! 起動後は `tako window move` / `tako window resize`（= MCP `tako_window` の
//! `move` / `resize`）で、どちらも**このモジュールの 1 実装**を通る。
//!
//! ## 座標の空間（重要）
//!
//! 扱う矩形は **GPUI がウィンドウ矩形として読み書きする空間**で、`layout.json` の
//! `window` フレームと同じもの。指定した座標は**そのディスプレイの左上を原点**と
//! して解釈し、[`resolve`] が置き先の矩形の原点を足して GPUI の空間へ移す。
//!
//! - macOS: GPUI の `PlatformDisplay::bounds()` は原点が常に `(0,0)`（大きさだけ実値）
//!   なので、足す値は 0 = 指定した座標がそのままディスプレイ相対になる
//! - Windows: `bounds()` が仮想デスクトップのグローバル座標を返すので、
//!   モニタの原点が足されて**同じ「ディスプレイ内の座標」の意味**になる
//!
//! 置き先が決まっていない起動（通常起動で `TAKO_DISPLAY` 未指定）では足す原点が
//! 無いので、指定はそのまま GPUI の空間として扱い、収まりの検査も行わない。

use super::display::DisplayRect;

/// 窓の初期位置・寸法の指定（`x,y,w,h` または `w,h`）
pub const ENV_WINDOW_BOUNDS: &str = "TAKO_WINDOW_BOUNDS";

/// 指定が無いときの既定の幅。**変えると通常起動の窓の大きさが変わる**
pub const DEFAULT_WIDTH: f32 = 960.0;
/// 指定が無いときの既定の高さ。**変えると通常起動の窓の大きさが変わる**
pub const DEFAULT_HEIGHT: f32 = 600.0;

/// 受け付ける最小の幅。これ未満は「壊れた値」として撥ねる
/// （保存フレームの健全性検査も同じ下限を見る = 1 実装）
pub const MIN_WIDTH: f32 = 200.0;
/// 受け付ける最小の高さ
pub const MIN_HEIGHT: f32 = 150.0;

/// 窓の矩形（論理ピクセル）。GPUI の `Bounds<Pixels>` と同じ意味を持つ素の値
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// 左端
    pub x: f32,
    /// 上端
    pub y: f32,
    /// 幅
    pub width: f32,
    /// 高さ
    pub height: f32,
}

impl Rect {
    /// 素の 4 値から作る
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// 外から来た指定。位置だけ・大きさだけの指定も受ける
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Spec {
    /// 左上（ディスプレイ内の座標）。`None` なら [`OriginDefault`] に従う
    pub origin: Option<(f32, f32)>,
    /// 幅と高さ。`None` なら今の大きさを保つ
    pub size: Option<(f32, f32)>,
}

/// 位置の指定が無かったときにどこへ置くか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginDefault {
    /// 今の位置を保つ（`window resize` = 大きさだけ変える操作）
    Keep,
    /// 置き先の中央（起動時。大きさだけ指定したら中央に出るのが素直）
    Center,
}

/// `x,y,w,h` または `w,h` を解釈する。
///
/// **区切りは `,`**（空白は捨てる）。数値は整数でも小数でもよい。
/// 2 値なら大きさだけ、4 値なら位置と大きさ。それ以外の形は撥ねる
/// （黙って先頭 2 つを採る、のような救済はしない = 意図と違う窓が出る方が困る）。
pub fn parse(spec: &str) -> Result<Spec, String> {
    let parts: Vec<&str> = spec.split(',').map(str::trim).collect();
    let nums: Result<Vec<f32>, String> = parts
        .iter()
        .map(|p| {
            p.parse::<f32>()
                .map_err(|_| format!("数値として読めない: {p:?}"))
        })
        .collect();
    let nums = nums?;
    if nums.iter().any(|v| !v.is_finite()) {
        return Err(format!("有限の数値ではない: {spec:?}"));
    }
    match nums.len() {
        2 => Ok(Spec {
            origin: None,
            size: Some((nums[0], nums[1])),
        }),
        4 => Ok(Spec {
            origin: Some((nums[0], nums[1])),
            size: Some((nums[2], nums[3])),
        }),
        n => Err(format!(
            "値が {n} 個（`x,y,w,h` か `w,h` の形で指定する）: {spec:?}"
        )),
    }
}

/// 指定を実際の矩形へ落とす。
///
/// - `current` は今の矩形（大きさ・位置の指定が無かったぶんの供給元）
/// - `display` は置き先のディスプレイ矩形（GPUI の `PlatformDisplay::bounds()` 由来）。
///   `None` は「置き先が決まっていない」= 指定をそのまま使い、収まりも検査しない
///
/// 撥ねるのは 2 つだけ: **最小寸法未満**と**置き先からはみ出す**指定。
/// どちらも理由の文字列を返し、呼び出し側が診断へ出す（起動時は既定へ落ちる /
/// CLI・MCP からはエラーとして返る）。
pub fn resolve(
    spec: &Spec,
    current: Rect,
    display: Option<DisplayRect>,
    origin_default: OriginDefault,
) -> Result<Rect, String> {
    let (width, height) = spec.size.unwrap_or((current.width, current.height));
    if width < MIN_WIDTH || height < MIN_HEIGHT {
        return Err(format!(
            "寸法が小さすぎる: {width}x{height}（最小 {MIN_WIDTH}x{MIN_HEIGHT}）"
        ));
    }
    let (x, y) = match spec.origin {
        // 指定された座標は**ディスプレイ内の座標**なので、置き先の原点を足して
        // GPUI の空間へ移す（macOS は原点 0 なので素通り・Windows はモニタの原点）
        Some((rx, ry)) => match display {
            Some(d) => (d.x + rx, d.y + ry),
            None => (rx, ry),
        },
        None => match (origin_default, display) {
            (OriginDefault::Center, Some(d)) => (
                d.x + (d.width - width) / 2.0,
                d.y + (d.height - height) / 2.0,
            ),
            // 置き先が分からなければ中央も出せない。今の位置を保つ
            _ => (current.x, current.y),
        },
    };
    if let Some(d) = display {
        if !d.contains(x, y, width, height) {
            return Err(format!(
                "置き先のディスプレイ（{}x{} @ {},{}）からはみ出す: {width}x{height} @ {x},{y}",
                d.width, d.height, d.x, d.y
            ));
        }
    }
    Ok(Rect::new(x, y, width, height))
}

/// 既定の矩形（置き先の中央に [`DEFAULT_WIDTH`] x [`DEFAULT_HEIGHT`]）。
/// 置き先が分からなければ原点は `(0,0)`（GPUI 側が既定の面へ寄せる）
pub fn default_rect(display: Option<DisplayRect>) -> Rect {
    match display {
        Some(d) => Rect::new(
            d.x + (d.width - DEFAULT_WIDTH) / 2.0,
            d.y + (d.height - DEFAULT_HEIGHT) / 2.0,
            DEFAULT_WIDTH,
            DEFAULT_HEIGHT,
        ),
        None => Rect::new(0.0, 0.0, DEFAULT_WIDTH, DEFAULT_HEIGHT),
    }
}

/// `TAKO_WINDOW_BOUNDS` を読む（未設定・空なら `None`）。
/// **読む口はここ 1 つ**（呼び出し側で `std::env::var` を散らさない）
pub fn env_spec() -> Option<String> {
    match std::env::var(ENV_WINDOW_BOUNDS) {
        Ok(v) if !v.trim().is_empty() => Some(v.trim().to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vd() -> DisplayRect {
        // macOS の GPUI が返す形（原点は常に 0 / 大きさだけ実値）
        DisplayRect {
            x: 0.0,
            y: 0.0,
            width: 2560.0,
            height: 1440.0,
        }
    }

    fn monitor() -> DisplayRect {
        // Windows の GPUI が返す形（グローバル座標）
        DisplayRect {
            x: 1920.0,
            y: 0.0,
            width: 2560.0,
            height: 1440.0,
        }
    }

    #[test]
    fn 四値は位置と大きさ() {
        assert_eq!(
            parse("10,20,1400,900").unwrap(),
            Spec {
                origin: Some((10.0, 20.0)),
                size: Some((1400.0, 900.0)),
            }
        );
    }

    #[test]
    fn 二値は大きさだけ() {
        assert_eq!(
            parse(" 1400 , 900 ").unwrap(),
            Spec {
                origin: None,
                size: Some((1400.0, 900.0)),
            }
        );
    }

    #[test]
    fn 形が違えば撥ねる() {
        for bad in ["", "1400", "1,2,3", "1,2,3,4,5", "a,b", "1400,x"] {
            assert!(parse(bad).is_err(), "{bad:?} を受理してはいけない");
        }
    }

    #[test]
    fn 非有限は撥ねる() {
        for bad in ["inf,900", "1,2,NaN,900"] {
            assert!(parse(bad).is_err(), "{bad:?} を受理してはいけない");
        }
    }

    #[test]
    fn 指定した位置はディスプレイ内の座標() {
        let spec = parse("100,50,1400,900").unwrap();
        let got = resolve(
            &spec,
            default_rect(Some(vd())),
            Some(vd()),
            OriginDefault::Center,
        )
        .unwrap();
        assert_eq!(got, Rect::new(100.0, 50.0, 1400.0, 900.0));
    }

    #[test]
    fn windowsのグローバル座標では原点が足される() {
        let spec = parse("100,50,1400,900").unwrap();
        let got = resolve(
            &spec,
            default_rect(Some(monitor())),
            Some(monitor()),
            OriginDefault::Center,
        )
        .unwrap();
        // モニタの原点 1920 が足されて「そのディスプレイ内の 100,50」になる
        assert_eq!(got, Rect::new(2020.0, 50.0, 1400.0, 900.0));
    }

    #[test]
    fn 大きさだけの指定は中央へ() {
        let spec = parse("1400,900").unwrap();
        let got = resolve(
            &spec,
            default_rect(Some(vd())),
            Some(vd()),
            OriginDefault::Center,
        )
        .unwrap();
        assert_eq!(got, Rect::new(580.0, 270.0, 1400.0, 900.0));
    }

    #[test]
    fn 大きさだけの指定でkeepなら位置を保つ() {
        let spec = parse("1400,900").unwrap();
        let current = Rect::new(300.0, 200.0, 960.0, 600.0);
        let got = resolve(&spec, current, Some(vd()), OriginDefault::Keep).unwrap();
        assert_eq!(got, Rect::new(300.0, 200.0, 1400.0, 900.0));
    }

    #[test]
    fn はみ出す指定は撥ねる() {
        // 右へはみ出す / 下へはみ出す / 負の座標
        for spec in ["2000,100,1400,900", "100,1000,1400,900", "-10,10,400,300"] {
            let parsed = parse(spec).unwrap();
            let err = resolve(
                &parsed,
                default_rect(Some(vd())),
                Some(vd()),
                OriginDefault::Center,
            )
            .unwrap_err();
            assert!(err.contains("はみ出す"), "{spec:?} -> {err}");
        }
    }

    #[test]
    fn 最小寸法未満は撥ねる() {
        let parsed = parse("100,100,199,900").unwrap();
        let err = resolve(
            &parsed,
            default_rect(Some(vd())),
            Some(vd()),
            OriginDefault::Center,
        )
        .unwrap_err();
        assert!(err.contains("小さすぎる"), "{err}");
    }

    #[test]
    fn 置き先が無ければ指定をそのまま使い検査もしない() {
        let parsed = parse("5000,5000,1400,900").unwrap();
        let got = resolve(&parsed, default_rect(None), None, OriginDefault::Center).unwrap();
        assert_eq!(got, Rect::new(5000.0, 5000.0, 1400.0, 900.0));
    }

    #[test]
    fn 既定は中央の960x600() {
        assert_eq!(
            default_rect(Some(vd())),
            Rect::new(800.0, 420.0, 960.0, 600.0)
        );
        assert_eq!(DEFAULT_WIDTH, 960.0);
        assert_eq!(DEFAULT_HEIGHT, 600.0);
    }
}
