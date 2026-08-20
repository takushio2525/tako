//! フォームのレイアウト健全性を実測矩形から数える（Issue #738）。
//!
//! 設定画面のフォームが崩れた（チップ群が重なり、隣の項目へ食い込んだ）ときに
//! 目視でしか気づけなかったのが #738 の教訓。**描き終わった矩形**を突き合わせて
//! 「重なっていない」「枠から出ていない」を数で押さえる。
//!
//! GPUI に依存しない純関数なので、GUI を起動しない unit test で検出力を確かめられる
//! （GPUI 側の実測は `settings_window` の probe が撮り、ここへ渡ってくる）。

/// 実測矩形（画面座標・論理 px）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            x0: x,
            y0: y,
            x1: x + w,
            y1: y + h,
        }
    }

    fn width(&self) -> f32 {
        self.x1 - self.x0
    }

    fn height(&self) -> f32 {
        self.y1 - self.y0
    }

    /// 2 矩形が重なっている量（両軸とも正なら重なっている）
    fn overlap_extent(&self, other: &Rect) -> (f32, f32) {
        (
            self.x1.min(other.x1) - self.x0.max(other.x0),
            self.y1.min(other.y1) - self.y0.max(other.y0),
        )
    }
}

/// 端数の丸め。同じ辺で接しているだけを「重なり」と呼ばないための許容
const EPS: f32 = 0.5;

/// 実測矩形から求めたフォームの健全性。
/// 「重なっていない」「枠から出ていない」は目視では見落とすので数で持つ
#[derive(Debug, Default)]
pub struct ProbeReport {
    /// 押せる部品（チップ・ボタン・入力欄・トグル）の数
    pub controls: usize,
    /// ラベル + コントロールの行の数
    pub rows: usize,
    /// 重なっている部品・行の組
    pub overlaps: Vec<String>,
    /// 自分が属するチップ群の枠から出てしまった部品
    pub escaped: Vec<String>,
    /// 本文の横幅からはみ出した部品
    pub outside: Vec<String>,
}

impl ProbeReport {
    /// 崩れていない = 3 種の違反がどれも無い
    pub fn is_clean(&self) -> bool {
        self.overlaps.is_empty() && self.escaped.is_empty() && self.outside.is_empty()
    }
}

/// 実測矩形を突き合わせて、重なり・枠外・横はみ出しを数える。
///
/// キーの接頭辞で役割を見分ける:
/// - `ctl:<id>` … 押せる部品（この同士が重なったら崩壊）
/// - `chips:<id>` … チップ群の枠（`ctl:<id>-<値>` はこの中に収まっていなければならない）
/// - `row:<ラベル>` … ラベル + コントロールの 1 行（行同士が重なったら崩壊）
///
/// `viewport` はタブ本文のスクロール枠。**横**にはみ出したら画面外だが、
/// 縦は「スクロールで画面外にある」が正常なので見ない
pub fn probe_report(probes: &[(String, Rect)], viewport: Rect) -> ProbeReport {
    let pick = |prefix: &str| -> Vec<(&str, Rect)> {
        probes
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(k, r)| (k.as_str(), *r))
            .collect()
    };
    let controls = pick("ctl:");
    let groups = pick("chips:");
    let rows = pick("row:");

    let mut report = ProbeReport {
        controls: controls.len(),
        rows: rows.len(),
        ..Default::default()
    };

    let collect_overlaps = |items: &[(&str, Rect)], label: &str, out: &mut Vec<String>| {
        for (i, (ka, a)) in items.iter().enumerate() {
            for (kb, b) in items.iter().skip(i + 1) {
                let (dx, dy) = a.overlap_extent(b);
                if dx > EPS && dy > EPS {
                    out.push(format!("{label} {ka} x {kb} ({dx:.1}x{dy:.1}px)"));
                }
            }
        }
    };
    let mut overlaps = Vec::new();
    collect_overlaps(&controls, "overlap", &mut overlaps);
    collect_overlaps(&rows, "row-overlap", &mut overlaps);
    report.overlaps = overlaps;

    for (key, b) in &controls {
        // 所属するチップ群は「最長一致する接頭辞」で決める
        // （prof-effort と prof-worker-effort を取り違えない）
        let id = key.trim_start_matches("ctl:");
        let group = groups
            .iter()
            .filter(|(gk, _)| id.starts_with(gk.trim_start_matches("chips:")))
            .max_by_key(|(gk, _)| gk.len());
        if let Some((gk, g)) = group {
            let (dx, dy) = b.overlap_extent(g);
            let inside = dx >= b.width() - EPS && dy >= b.height() - EPS;
            if !inside {
                report.escaped.push(format!(
                    "escaped {key} from {gk} (収まり {dx:.1}x{dy:.1}px)"
                ));
            }
        }
        if b.x0 < viewport.x0 - EPS || b.x1 > viewport.x1 + EPS {
            report.outside.push(format!(
                "outside {key} x={:.1}..{:.1} viewport={:.1}..{:.1}",
                b.x0, b.x1, viewport.x0, viewport.x1
            ));
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewport() -> Rect {
        Rect::new(180.0, 40.0, 580.0, 520.0)
    }

    /// 正しく折り返した 2 行のチップ群（重なりゼロ）
    fn wrapped_group() -> Vec<(String, Rect)> {
        vec![
            ("chips:eff".into(), Rect::new(500.0, 100.0, 240.0, 52.0)),
            ("ctl:eff-low".into(), Rect::new(500.0, 100.0, 60.0, 24.0)),
            ("ctl:eff-medium".into(), Rect::new(563.0, 100.0, 80.0, 24.0)),
            ("ctl:eff-high".into(), Rect::new(500.0, 128.0, 60.0, 24.0)),
            ("row:effort".into(), Rect::new(200.0, 96.0, 540.0, 60.0)),
        ]
    }

    #[test]
    fn 折り返したチップ群は違反ゼロ() {
        let report = probe_report(&wrapped_group(), viewport());
        assert_eq!(report.controls, 3);
        assert_eq!(report.rows, 1);
        assert!(report.is_clean(), "{report:?}");
    }

    #[test]
    fn 重なったチップを検出する() {
        let mut probes = wrapped_group();
        // 2 個目を 1 個目へ重ねる（#738 の症状）
        probes[2].1 = Rect::new(520.0, 100.0, 80.0, 24.0);
        let report = probe_report(&probes, viewport());
        assert_eq!(report.overlaps.len(), 1, "{report:?}");
        assert!(report.overlaps[0].contains("ctl:eff-low"));
        assert!(!report.is_clean());
    }

    #[test]
    fn 枠からはみ出したチップを検出する() {
        let mut probes = wrapped_group();
        // 折り返した 2 行目がチップ群の高さに数えられていない状態
        probes[0].1 = Rect::new(500.0, 100.0, 240.0, 24.0);
        let report = probe_report(&probes, viewport());
        assert_eq!(report.escaped.len(), 1, "{report:?}");
        assert!(report.escaped[0].contains("ctl:eff-high"));
    }

    #[test]
    fn 本文の幅から溢れたチップを検出する() {
        let mut probes = wrapped_group();
        probes[1].1 = Rect::new(700.0, 100.0, 120.0, 24.0);
        probes[0].1 = Rect::new(500.0, 100.0, 320.0, 52.0);
        let report = probe_report(&probes, viewport());
        assert_eq!(report.outside.len(), 1, "{report:?}");
        assert!(report.outside[0].contains("ctl:eff-low"));
    }

    #[test]
    fn 接する辺は重なりと呼ばない() {
        let probes = vec![
            ("ctl:a".into(), Rect::new(0.0, 0.0, 50.0, 20.0)),
            ("ctl:b".into(), Rect::new(50.0, 0.0, 50.0, 20.0)),
        ];
        let report = probe_report(&probes, Rect::new(0.0, 0.0, 200.0, 200.0));
        assert!(report.is_clean(), "{report:?}");
    }

    #[test]
    fn チップ群は最長一致で決める() {
        // prof-effort と prof-worker-effort を取り違えると、正しい配置でも
        // 「枠外」と誤判定してしまう
        let probes = vec![
            (
                "chips:prof-effort".into(),
                Rect::new(500.0, 100.0, 200.0, 24.0),
            ),
            (
                "chips:prof-worker-effort".into(),
                Rect::new(500.0, 200.0, 200.0, 24.0),
            ),
            (
                "ctl:prof-worker-effort-low".into(),
                Rect::new(520.0, 200.0, 60.0, 24.0),
            ),
        ];
        let report = probe_report(&probes, viewport());
        assert!(report.escaped.is_empty(), "{report:?}");
    }
}
