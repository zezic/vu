//! Viewport-relative presentation geometry.
//!
//! The layout is designed against a 16:9 frame but never letterboxes: at wider
//! ratios the side columns widen, and at taller ratios the layout keeps smaller
//! side columns. Every size is expressed in 1080p-relative units and converted
//! through [`Stage::px`], so a window resize rescales the whole composition.

use vello::kurbo::{Rect, Size};

/// Reference height for presentation typography.
const REFERENCE_HEIGHT: f64 = 1080.0;
const REFERENCE_WIDTH: f64 = 1920.0;

#[derive(Clone, Copy, Debug)]
pub struct Stage {
    pub width: f64,
    pub height: f64,
    /// Pixels per 1080p-relative unit.
    pub unit: f64,
    pub margin: f64,
    pub aspect: f64,
    pub left: Rect,
    pub right: Rect,
}

impl Stage {
    pub fn new(size: Size) -> Self {
        let width = size.width.max(1.0);
        let height = size.height.max(1.0);
        let aspect = width / height;
        let unit = (width / REFERENCE_WIDTH)
            .min(height / REFERENCE_HEIGHT)
            .max(0.06);
        let margin = 32.0 * unit;
        let center_half = (height * 0.33).min(width * 0.29);
        let gap = 12.0 * unit;
        let center_x0 = width * 0.5 - center_half;
        let center_x1 = width * 0.5 + center_half;
        let left = Rect::new(
            margin,
            margin,
            (center_x0 - gap).max(margin + 80.0 * unit),
            height - margin,
        );
        let right = Rect::new(
            (center_x1 + gap).min(width - margin - 80.0 * unit),
            margin,
            width - margin,
            height - margin,
        );
        Self {
            width,
            height,
            unit,
            margin,
            aspect,
            left,
            right,
        }
    }

    /// Convert a 1080p-relative design unit to pixels.
    pub fn px(&self, units: f64) -> f64 {
        units * self.unit
    }

    pub fn is_tall(&self) -> bool {
        self.aspect < 1.55
    }

    /// Width a normal-mode text column should occupy, capped for ultrawide
    /// windows so paragraphs keep a readable measure.
    pub fn column_width(&self) -> f64 {
        self.left.width().min(self.px(560.0)).max(self.px(180.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_follow_the_sixteen_by_nine_reference_and_the_window() {
        let full_hd = Stage::new(Size::new(1920.0, 1080.0));
        assert!((full_hd.unit - 1.0).abs() < 1e-9);
        let hd = Stage::new(Size::new(1280.0, 720.0));
        assert!((hd.unit - 2.0 / 3.0).abs() < 1e-6);
        let short = Stage::new(Size::new(1280.0, 440.0));
        assert!(short.unit < hd.unit);
        assert!(short.left.width() > 0.0);
        assert!(short.right.width() > 0.0);
        let four_three = Stage::new(Size::new(1024.0, 768.0));
        assert!(four_three.is_tall());
        assert!(four_three.left.x0 < four_three.right.x0);
    }

    #[test]
    fn columns_leave_the_center_clear() {
        let stage = Stage::new(Size::new(1920.0, 1080.0));
        assert!(stage.left.x1 < stage.width * 0.5);
        assert!(stage.right.x0 > stage.width * 0.5);
        assert!(stage.column_width() >= stage.px(180.0));
    }
}
