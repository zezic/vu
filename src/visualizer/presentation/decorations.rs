//! Optional side decorations for text rows.
//!
//! A decoration is a short horizontal barcode of line-height rectangles, or a
//! small matrix of squares. Both sit in the margin on the text's aligned side
//! and use only the scene's three colors.

use vello::{
    kurbo::{Affine, Rect},
    peniko::Fill,
    Scene,
};

use super::super::Rng;
use super::{color::ScenePalette, stage::Stage};

#[derive(Clone, Debug)]
pub enum Decoration {
    Bars(Bars),
    Squares(Squares),
}

#[derive(Clone, Debug)]
pub struct Bars {
    /// Widths in stage units.
    widths: Vec<f64>,
    gap: f64,
    height: f64,
}

#[derive(Clone, Debug)]
pub struct Squares {
    cols: usize,
    rows: usize,
    cell: f64,
    gap: f64,
    filled: Vec<bool>,
}

impl Decoration {
    /// `line_height` is in stage units. `max_width` caps the strip so it stays
    /// inside the reserved margin.
    pub fn random(rng: &mut Rng, line_height: f64, max_width: f64) -> Option<Self> {
        if rng.unit() > 0.65 {
            return None;
        }
        if rng.unit() < 0.55 {
            let gap = line_height * (0.05 + f64::from(rng.unit()) * 0.05);
            let mut widths = Vec::new();
            let mut total = 0.0;
            let count = 3 + rng.range(4);
            while widths.len() < count {
                let width = if !widths.is_empty() && rng.unit() < 0.35 {
                    widths[rng.range(widths.len())]
                } else {
                    line_height * (0.1 + f64::from(rng.unit()) * 0.26)
                };
                let added = if widths.is_empty() {
                    width
                } else {
                    width + gap
                };
                if total + added > max_width {
                    break;
                }
                total += added;
                widths.push(width);
            }
            if widths.is_empty() {
                None
            } else {
                Some(Self::Bars(Bars {
                    widths,
                    gap,
                    height: line_height,
                }))
            }
        } else {
            let cols = 2 + rng.range(3);
            let rows = 2 + rng.range(3);
            let cell = line_height * (0.1 + f64::from(rng.unit()) * 0.26);
            let gap = line_height * (0.08 + f64::from(rng.unit()) * 0.1);
            let width = cols as f64 * cell + (cols - 1) as f64 * gap;
            if width > max_width {
                return None;
            }
            let mut filled = Vec::with_capacity(cols * rows);
            for _ in 0..cols * rows {
                filled.push(rng.unit() < 0.55);
            }
            Some(Self::Squares(Squares {
                cols,
                rows,
                cell,
                gap,
                filled,
            }))
        }
    }

    pub fn width(&self, stage: &Stage) -> f64 {
        match self {
            Self::Bars(bars) => {
                let widths: f64 = bars.widths.iter().sum();
                let gaps = bars.gap * (bars.widths.len().saturating_sub(1)) as f64;
                stage.px(widths + gaps)
            }
            Self::Squares(squares) => stage.px(squares.cols as f64 * squares.cell
                + (squares.cols.saturating_sub(1)) as f64 * squares.gap),
        }
    }

    pub fn height(&self, stage: &Stage) -> f64 {
        match self {
            Self::Bars(bars) => stage.px(bars.height),
            Self::Squares(squares) => stage.px(squares.rows as f64 * squares.cell
                + (squares.rows.saturating_sub(1)) as f64 * squares.gap),
        }
    }

    pub fn draw(
        &self,
        scene: &mut Scene,
        stage: &Stage,
        palette: ScenePalette,
        text_origin: vello::kurbo::Point,
        alpha: f32,
    ) {
        let width = self.width(stage);
        let gap = stage.px(6.0);
        let x0 = text_origin.x - width - gap;
        match self {
            Self::Bars(bars) => {
                let height = stage.px(bars.height);
                let y = text_origin.y;
                let mut x = x0;
                for (index, width) in bars.widths.iter().enumerate() {
                    let width = stage.px(*width);
                    let color = if index % 3 == 1 {
                        palette.accent.with_alpha(0.35 * alpha)
                    } else {
                        palette.muted.with_alpha(0.5 * alpha)
                    };
                    scene.fill(
                        Fill::NonZero,
                        Affine::IDENTITY,
                        color,
                        None,
                        &Rect::new(x, y, x + width, y + height),
                    );
                    x += width + stage.px(bars.gap);
                }
            }
            Self::Squares(squares) => {
                let cell = stage.px(squares.cell);
                let gap = stage.px(squares.gap);
                let y0 = text_origin.y;
                let mut index = 0;
                for row in 0..squares.rows {
                    for col in 0..squares.cols {
                        let x = x0 + col as f64 * (cell + gap);
                        let y = y0 + row as f64 * (cell + gap);
                        let filled = squares.filled.get(index).copied().unwrap_or(false);
                        if filled {
                            scene.fill(
                                Fill::NonZero,
                                Affine::IDENTITY,
                                palette.accent.with_alpha(0.28 * alpha),
                                None,
                                &Rect::new(x, y, x + cell, y + cell),
                            );
                        } else {
                            scene.stroke(
                                &vello::kurbo::Stroke::new((stage.unit * 0.8).max(0.6)),
                                Affine::IDENTITY,
                                palette.muted.with_alpha(0.55 * alpha),
                                None,
                                &Rect::new(x, y, x + cell, y + cell),
                            );
                        }
                        index += 1;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vello::kurbo::Size;

    #[test]
    fn decorations_stay_inside_the_reserved_margin() {
        let mut rng = Rng::new(23);
        let stage = Stage::new(Size::new(1920.0, 1080.0));
        let mut found = 0;
        for _ in 0..300 {
            if let Some(decoration) = Decoration::random(&mut rng, 42.0, 42.0) {
                found += 1;
                assert!(decoration.width(&stage) <= stage.px(42.0) + 1e-6);
                assert!(decoration.height(&stage) > 0.0);
            }
        }
        assert!(found > 30);
    }
}
