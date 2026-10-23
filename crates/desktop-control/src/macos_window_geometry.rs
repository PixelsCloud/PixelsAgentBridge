//! AppKit screen frames use bottom-left points; Accessibility uses top-left
//! points relative to the primary screen. Neither side uses backing pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    fn valid(self) -> bool {
        [
            self.x,
            self.y,
            self.width,
            self.height,
            self.x + self.width,
            self.y + self.height,
        ]
        .into_iter()
        .all(f64::is_finite)
            && self.width > 0.0
            && self.height > 0.0
    }

    fn ax(self, primary_top: f64) -> Self {
        Self {
            y: primary_top - self.y - self.height,
            ..self
        }
    }

    fn intersection(self, other: Self) -> f64 {
        let width = (self.x + self.width).min(other.x + other.width) - self.x.max(other.x);
        let height = (self.y + self.height).min(other.y + other.height) - self.y.max(other.y);
        width.max(0.0) * height.max(0.0)
    }
}

/// Screens are (full frame, visible frame) in NSScreen.screens order. The first
/// screen defines the global origin; NSScreen.mainScreen follows keyboard focus
/// and must not be used as that origin on a multi-monitor desktop.
pub(crate) fn maximize_rect(window: Rect, screens: &[(Rect, Rect)]) -> Result<Rect, String> {
    let primary = screens.first().ok_or("no available screen")?.0;
    if !window.valid() || !primary.valid() {
        return Err("invalid window/screen geometry".into());
    }
    let primary_top = primary.y + primary.height;
    let mut selected = None;
    let mut largest = 0.0;
    for &(frame, visible) in screens {
        if !frame.valid()
            || !visible.valid()
            || visible.x < frame.x
            || visible.y < frame.y
            || visible.x + visible.width > frame.x + frame.width
            || visible.y + visible.height > frame.y + frame.height
        {
            return Err("invalid visible screen geometry".into());
        }
        let area = window.intersection(frame.ax(primary_top));
        if area > largest {
            largest = area;
            selected = Some(visible.ax(primary_top));
        }
    }
    selected.ok_or_else(|| "window does not intersect an available screen".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn excludes_menu_and_dock_without_scaling_points() {
        let screens = [(
            rect(0.0, 0.0, 1920.0, 1080.0),
            rect(0.0, 70.0, 1920.0, 980.0),
        )];
        assert_eq!(
            maximize_rect(rect(500.0, 150.0, 600.0, 500.0), &screens).unwrap(),
            rect(0.0, 30.0, 1920.0, 980.0)
        );
    }

    #[test]
    fn selects_most_intersecting_screen_with_negative_origin() {
        let screens = [
            (
                rect(0.0, 0.0, 1920.0, 1080.0),
                rect(0.0, 70.0, 1920.0, 980.0),
            ),
            (
                rect(-1440.0, 180.0, 1440.0, 900.0),
                rect(-1380.0, 180.0, 1380.0, 875.0),
            ),
        ];
        assert_eq!(
            maximize_rect(rect(-500.0, 100.0, 600.0, 400.0), &screens).unwrap(),
            rect(-1380.0, 25.0, 1380.0, 875.0)
        );
    }

    #[test]
    fn screen_above_primary_preserves_negative_ax_y() {
        let screens = [
            (
                rect(0.0, 0.0, 1920.0, 1080.0),
                rect(0.0, 0.0, 1920.0, 1050.0),
            ),
            (
                rect(0.0, 1080.0, 1280.0, 800.0),
                rect(0.0, 1080.0, 1280.0, 770.0),
            ),
        ];
        assert_eq!(
            maximize_rect(rect(20.0, -600.0, 500.0, 400.0), &screens).unwrap(),
            rect(0.0, -770.0, 1280.0, 770.0)
        );
    }

    #[test]
    fn uses_current_dock_settings_on_every_call() {
        let frame = rect(0.0, 0.0, 1920.0, 1080.0);
        let window = rect(20.0, 100.0, 600.0, 400.0);
        assert_eq!(
            maximize_rect(window, &[(frame, rect(0.0, 70.0, 1920.0, 980.0))])
                .unwrap()
                .height,
            980.0
        );
        assert_eq!(
            maximize_rect(window, &[(frame, rect(0.0, 1.0, 1920.0, 1049.0))])
                .unwrap()
                .height,
            1049.0
        );
    }

    #[test]
    fn rejects_missing_invalid_and_offscreen_geometry() {
        let frame = rect(0.0, 0.0, 1920.0, 1080.0);
        assert!(maximize_rect(frame, &[]).is_err());
        assert!(maximize_rect(frame, &[(frame, rect(0.0, 0.0, f64::NAN, 900.0))]).is_err());
        assert!(maximize_rect(frame, &[(frame, rect(0.0, 0.0, 1921.0, 1080.0))]).is_err());
        assert!(maximize_rect(rect(4000.0, 4000.0, 500.0, 400.0), &[(frame, frame)]).is_err());
    }
}
