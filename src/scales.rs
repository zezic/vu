use std::collections::HashMap;

use vello::{
    kurbo::{Affine, Arc, Cap, Line, Stroke},
    peniko::Color,
    Scene,
};

use crate::{
    db_to_normalized, normalized_to_db,
    text::{Fonts, Text},
};

pub const MAX_ANGLE: f32 = 47.0;
const POSITIVE_DB_RANGE: f32 = 6.0;
const BASE_RADIUS: f64 = 164.0;
const INK: Color = Color::from_rgb8(220, 220, 220);
const WARNING: Color = Color::from_rgb8(220, 62, 73);

pub struct Scale {
    pub negative_db_range: f32,
    pub bend: f32,
}

impl Scale {
    pub fn new() -> Self {
        Self {
            negative_db_range: 53.4,
            bend: 2.0,
        }
    }

    pub fn position(&self, value: f32) -> f32 {
        let db = normalized_to_db(value, self.negative_db_range);
        ((db + self.negative_db_range).max(0.0) / (self.negative_db_range + POSITIVE_DB_RANGE))
            .powf(self.bend)
    }

    pub fn angle(position: f32) -> f64 {
        f64::from(position * MAX_ANGLE * 2.0 - 90.0 - MAX_ANGLE).to_radians()
    }
}

pub struct Mark {
    position: f32,
    label: Option<Text>,
    style: MarkStyle,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum MarkStyle {
    Big,
    Medium,
    Inter,
    Under,
    UnderWarn,
}

impl MarkStyle {
    fn label_size(self) -> f32 {
        match self {
            Self::Big => 16.0,
            Self::Medium => 10.0,
            Self::Under | Self::UnderWarn => 6.0,
            Self::Inter => unreachable!("intermediate marks have no labels"),
        }
    }
}

pub fn generate_din_scale(fonts: &mut Fonts) -> Vec<Mark> {
    let mut marks = Vec::new();
    for val in [-100, -90, -80, -70, -60, -50, -40, -30, -20, -10, -5, 0, 5] {
        let label = if val > 0 {
            format!("+{val}")
        } else {
            val.to_string()
        };
        marks.push(Mark {
            position: db_to_normalized(val as f32),
            label: Some(fonts.layout(&label, MarkStyle::Big.label_size())),
            style: MarkStyle::Big,
        });
    }
    marks.push(Mark {
        position: db_to_normalized(-9.0),
        label: Some(fonts.layout("-9", MarkStyle::Medium.label_size())),
        style: MarkStyle::Medium,
    });
    for val in [-45, -35, -25, -19, -3, -2, -1, 1, 2, 3, 4] {
        marks.push(Mark {
            position: db_to_normalized(val as f32),
            label: None,
            style: MarkStyle::Inter,
        });
    }
    marks.push(Mark {
        position: 2.0,
        label: None,
        style: MarkStyle::Inter,
    });
    for val in [1, 2, 3, 5, 10, 20, 30, 50, 100, 200] {
        let style = if val == 50 || val == 100 {
            MarkStyle::UnderWarn
        } else {
            MarkStyle::Under
        };
        marks.push(Mark {
            position: val as f32 / 100.0,
            label: Some(fonts.layout(&val.to_string(), style.label_size())),
            style,
        });
    }
    marks
}

pub fn draw_scale(scene: &mut Scene, transform: Affine, marks: &[Mark], scale: &Scale) {
    let start = Scale::angle(0.0);
    let end = Scale::angle(1.0);
    let zero = Scale::angle(scale.position(1.0));
    let warning_arc = Arc::new(
        (0.0, 0.0),
        (BASE_RADIUS + 3.75, BASE_RADIUS + 3.75),
        zero,
        end - zero,
        0.0,
    );
    scene.stroke(
        &Stroke::new(7.5).with_caps(Cap::Butt),
        transform,
        WARNING,
        None,
        &warning_arc,
    );
    let arc = Arc::new(
        (0.0, 0.0),
        (BASE_RADIUS, BASE_RADIUS),
        start,
        end - start,
        0.0,
    );
    scene.stroke(&Stroke::new(1.0), transform, INK, None, &arc);

    let mut previous_labels: HashMap<MarkStyle, f32> = HashMap::new();
    for mark in marks {
        if normalized_to_db(mark.position, scale.negative_db_range) < -scale.negative_db_range {
            continue;
        }
        let position = scale.position(mark.position);
        let (low, high, label_offset) = match mark.style {
            MarkStyle::Big => (0.0, 15.0, 20.0),
            MarkStyle::Medium => (0.0, 10.0, 10.0),
            MarkStyle::Inter => (0.0, 7.5, 0.0),
            MarkStyle::Under => (-3.0, 0.0, -10.0),
            MarkStyle::UnderWarn => (-3.0, 15.0, -10.0),
        };
        let rotated =
            transform * Affine::rotate(Scale::angle(position) + std::f64::consts::FRAC_PI_2);
        if let Some(label) = &mark.label {
            if previous_labels
                .get(&mark.style)
                .is_none_or(|previous| (previous - position).abs() > 0.075)
            {
                let color = if mark.style == MarkStyle::UnderWarn {
                    WARNING
                } else {
                    INK
                };
                label.draw(
                    scene,
                    rotated * Affine::translate((0.0, -BASE_RADIUS - label_offset)),
                    color,
                );
                previous_labels.insert(mark.style, position);
            }
        }
        let color = match mark.style {
            MarkStyle::Medium | MarkStyle::UnderWarn => WARNING,
            _ => INK,
        };
        scene.stroke(
            &Stroke::new(1.0),
            rotated,
            color,
            None,
            &Line::new((0.0, -BASE_RADIUS - low), (0.0, -BASE_RADIUS - high)),
        );
    }
}
