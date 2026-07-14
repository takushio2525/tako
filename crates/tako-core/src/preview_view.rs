//! PDF・画像プレビューのズーム / パン / ページ状態（Issue #234）。
//!
//! GPUI の座標型へ依存せず、GUI・dispatch・CLI・MCP が同じ操作セマンティクスを使う。

pub const PREVIEW_ZOOM_MIN: f32 = 0.25;
pub const PREVIEW_ZOOM_MAX: f32 = 4.0;
pub const PREVIEW_ZOOM_STEP: f32 = 1.1;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreviewViewState {
    /// 1.0 = 幅フィット（100%）。
    pub zoom: f32,
    /// 左端から右へ移動した量（logical px）。
    pub pan_x: f32,
    /// 上端から下へ移動した量（logical px）。
    pub pan_y: f32,
    /// PDF の対象ページ（1 始まり）。画像は常に 1。
    pub page: usize,
}

impl Default for PreviewViewState {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            page: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PreviewZoomCommand {
    In,
    Out,
    Set(f32),
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PreviewViewUpdate {
    pub zoom: Option<PreviewZoomCommand>,
    pub page: Option<usize>,
    /// 現在位置へ加えるパン量（logical px）。
    pub pan_delta: Option<(f32, f32)>,
}

impl PreviewViewState {
    pub fn apply(&mut self, update: PreviewViewUpdate) -> Result<(), String> {
        let previous_zoom = self.zoom;
        let mut scale_pan = false;
        if let Some(command) = update.zoom {
            match command {
                PreviewZoomCommand::In => {
                    self.zoom = (self.zoom * PREVIEW_ZOOM_STEP).min(PREVIEW_ZOOM_MAX);
                    scale_pan = true;
                }
                PreviewZoomCommand::Out => {
                    self.zoom = (self.zoom / PREVIEW_ZOOM_STEP).max(PREVIEW_ZOOM_MIN);
                    scale_pan = true;
                }
                PreviewZoomCommand::Set(zoom) => {
                    if !zoom.is_finite() || !(PREVIEW_ZOOM_MIN..=PREVIEW_ZOOM_MAX).contains(&zoom) {
                        return Err(format!(
                            "ズーム倍率は {}%〜{}% の範囲で指定する",
                            (PREVIEW_ZOOM_MIN * 100.0) as u32,
                            (PREVIEW_ZOOM_MAX * 100.0) as u32
                        ));
                    }
                    self.zoom = zoom;
                    scale_pan = true;
                }
                PreviewZoomCommand::Reset => {
                    self.zoom = 1.0;
                    self.pan_x = 0.0;
                    self.pan_y = 0.0;
                }
            }
        }
        if scale_pan {
            let ratio = self.zoom / previous_zoom.max(f32::EPSILON);
            self.pan_x *= ratio;
            self.pan_y *= ratio;
        }
        if let Some(page) = update.page {
            if page == 0 {
                return Err("ページは 1 以上で指定する".into());
            }
            self.page = page;
        }
        if let Some((x, y)) = update.pan_delta {
            if !x.is_finite() || !y.is_finite() {
                return Err("パン量は有限の数で指定する".into());
            }
            self.pan_x += x;
            self.pan_y += y;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ズームとパンとページを同じ更新で適用できる() {
        let mut state = PreviewViewState::default();
        state
            .apply(PreviewViewUpdate {
                zoom: Some(PreviewZoomCommand::Set(1.5)),
                page: Some(3),
                pan_delta: Some((40.0, 25.0)),
            })
            .unwrap();
        assert_eq!(state.zoom, 1.5);
        assert_eq!(state.page, 3);
        assert_eq!((state.pan_x, state.pan_y), (40.0, 25.0));
    }

    #[test]
    fn リセットは倍率とパンだけを幅フィットへ戻す() {
        let mut state = PreviewViewState {
            zoom: 2.0,
            pan_x: 80.0,
            pan_y: 120.0,
            page: 4,
        };
        state
            .apply(PreviewViewUpdate {
                zoom: Some(PreviewZoomCommand::Reset),
                ..PreviewViewUpdate::default()
            })
            .unwrap();
        assert_eq!(
            state,
            PreviewViewState {
                page: 4,
                ..PreviewViewState::default()
            }
        );
    }

    #[test]
    fn 倍率変更は表示中の内容を保つようパンも倍率比で更新する() {
        let mut state = PreviewViewState {
            zoom: 1.0,
            pan_x: 40.0,
            pan_y: 600.0,
            page: 2,
        };
        state
            .apply(PreviewViewUpdate {
                zoom: Some(PreviewZoomCommand::Set(1.5)),
                pan_delta: Some((20.0, 30.0)),
                ..PreviewViewUpdate::default()
            })
            .unwrap();

        assert_eq!(state.zoom, 1.5);
        assert_eq!((state.pan_x, state.pan_y), (80.0, 930.0));
        assert_eq!(state.page, 2);
    }

    #[test]
    fn 不正な倍率とページを拒否する() {
        let mut state = PreviewViewState::default();
        assert!(state
            .apply(PreviewViewUpdate {
                zoom: Some(PreviewZoomCommand::Set(4.01)),
                ..PreviewViewUpdate::default()
            })
            .is_err());
        assert!(state
            .apply(PreviewViewUpdate {
                page: Some(0),
                ..PreviewViewUpdate::default()
            })
            .is_err());
    }
}
