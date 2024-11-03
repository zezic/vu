use std::{collections::HashMap, ops::Sub};

use femtovg::{renderer::OpenGl, Canvas, Color, FontId, Paint, Path, Solidity};

use crate::{db_to_normalized, normalized_to_db};

pub struct Mark {
    position: f32,
    label: Option<String>,
    style: MarkStyle,
}

#[derive(PartialEq, Eq, Hash)]
enum MarkStyle {
    Big,
    Medium,
    Inter,
    Under,
    UnderWarn,
}

pub fn generate_din_scale() -> Vec<Mark> {
    let mut marks = vec![];

    for val in [-100, -90, -80, -70, -60, -50, -40, -30, -20, -10, -5, 0, 5] {
        marks.push(Mark {
            position: db_to_normalized(val as f32),
            label: Some(if val > 0 {
                format!("+{}", val)
            } else {
                val.to_string()
            }),
            style: MarkStyle::Big,
        })
    }

    marks.push(Mark {
        position: db_to_normalized(-9 as f32),
        label: Some("-9".into()),
        style: MarkStyle::Medium,
    });

    for val in [-45, -35, -25, -15 - 4, -3, -2, -1, 1, 2, 3, 4] {
        marks.push(Mark {
            position: db_to_normalized(val as f32),
            label: None,
            style: MarkStyle::Inter,
        })
    }

    // marks.push(Mark {
    //     position: 0.0,
    //     label: None,
    //     style: MarkStyle::Inter,
    // });

    marks.push(Mark {
        position: 2.0,
        label: None,
        style: MarkStyle::Inter,
    });

    for val in [1, 2, 3, 5, 10, 20, 30, 50, 100, 200] {
        marks.push(Mark {
            position: val as f32 / 100.0,
            label: Some(val.to_string()),
            style: if val == 50 || val == 100 {
                MarkStyle::UnderWarn
            } else {
                MarkStyle::Under
            },
        })
    }

    marks
}

pub fn draw_scale(
    canvas: &mut Canvas<OpenGl>,
    font_id: FontId,
    marks: &[Mark],
    max_angle: f32,
    negative_db_range: f32,
    positive_db_range: f32,
    bend: f32,
) {
    let base_radius = 164.0;

    let zero_db = normalized_to_db(1.0, negative_db_range);
    let zero_db = zero_db + negative_db_range;
    let zero_db = zero_db / (negative_db_range + positive_db_range);
    let zero_db = zero_db.powf(bend);
    let mut path = Path::new();
    path.arc(
        0.0,
        0.0,
        base_radius + 7.5 / 2.0,
        (zero_db * max_angle * 2.0 - 90.0 - max_angle) * (std::f32::consts::PI / 180.0),
        (max_angle - 90.0) * (std::f32::consts::PI / 180.0),
        Solidity::Hole,
    );
    let mut paint = Paint::color(Color::rgb(220, 62, 73));
    paint.set_line_width(7.5);
    canvas.stroke_path(&path, &paint);

    let mut path = Path::new();
    path.arc(
        0.0,
        0.0,
        base_radius,
        (-max_angle - 90.0) * (std::f32::consts::PI / 180.0),
        (max_angle - 90.0) * (std::f32::consts::PI / 180.0),
        Solidity::Hole,
    );
    let mut paint = Paint::color(Color::rgb(220, 220, 220));
    paint.set_line_width(1.0);
    canvas.stroke_path(&path, &paint);

    let mut previous_label_places = HashMap::new();

    for mark in marks {
        let rms = normalized_to_db(mark.position, negative_db_range);
        if rms < -negative_db_range {
            continue;
        }
        let rms = rms + negative_db_range;
        let rms = rms / (negative_db_range + positive_db_range);
        let rms = rms.powf(bend);

        // Convert value from [0.0, 1.0] to angle range [-max_angle°, max_angle°] in radians
        let angle = (rms * max_angle * 2.0 - 90.0 - max_angle) * (std::f32::consts::PI / 180.0); // Convert degrees to radians

        let (mark_lo, mark_hi) = match mark.style {
            MarkStyle::Big => (0.0, 15.0),
            MarkStyle::Medium => (0.0, 10.0),
            MarkStyle::Inter => (0.0, 7.5),
            MarkStyle::Under => (-3.0, 0.0),
            MarkStyle::UnderWarn => (-3.0, 15.0),
        };
        let mark_lo = base_radius + mark_lo;
        let mark_hi = base_radius + mark_hi;

        let text_y = match mark.style {
            MarkStyle::Big => 20.0,
            MarkStyle::Medium => 10.0,
            MarkStyle::Inter => 0.0,
            MarkStyle::Under => -10.0,
            MarkStyle::UnderWarn => -10.0,
        };
        let text_y = base_radius + text_y;

        let text_paint = match mark.style {
            MarkStyle::Big => {
                let mut paint = Paint::color(Color::rgb(220, 220, 220));
                paint.set_text_align(femtovg::Align::Center);
                paint.set_font(&[font_id]);
                paint.set_font_size(16.0);
                paint
            }
            MarkStyle::Medium => {
                let mut paint = Paint::color(Color::rgb(220, 220, 220));
                paint.set_text_align(femtovg::Align::Center);
                paint.set_font(&[font_id]);
                paint.set_font_size(10.0);
                paint
            }
            MarkStyle::Inter => Paint::color(Color::white()),
            MarkStyle::Under => {
                let mut paint = Paint::color(Color::rgb(220, 220, 220));
                paint.set_text_align(femtovg::Align::Center);
                paint.set_font(&[font_id]);
                paint.set_font_size(6.0);
                paint
            }
            MarkStyle::UnderWarn => {
                let mut paint = Paint::color(Color::rgb(220, 62, 73));
                paint.set_text_align(femtovg::Align::Center);
                paint.set_font(&[font_id]);
                paint.set_font_size(6.0);
                paint
            }
        };

        let mark_paint = match mark.style {
            MarkStyle::Big => {
                let mut paint = Paint::color(Color::rgb(220, 220, 220));
                paint.set_line_width(1.0);
                paint
            }
            MarkStyle::Medium => {
                let mut paint = Paint::color(Color::rgb(220, 62, 73));
                paint.set_line_width(1.0);
                paint
            }
            MarkStyle::Inter => {
                let mut paint = Paint::color(Color::rgb(220, 220, 220));
                paint.set_line_width(1.0);
                paint
            }
            MarkStyle::Under => {
                let mut paint = Paint::color(Color::rgb(220, 220, 220));
                paint.set_line_width(1.0);
                paint
            }
            MarkStyle::UnderWarn => {
                let mut paint = Paint::color(Color::rgb(220, 62, 73));
                paint.set_line_width(1.0);
                paint
            }
        };

        canvas.save();
        canvas.rotate(angle + std::f32::consts::FRAC_PI_2);
        if let Some(label) = &mark.label {
            let prev_place = previous_label_places.get(&mark.style).unwrap_or(&10.0);
            if prev_place.sub(rms).abs() > 0.075 {
                canvas.fill_text(0.0, -text_y, label, &text_paint).unwrap();
                previous_label_places.insert(&mark.style, rms);
            }
        }

        let mut path = Path::new();
        path.move_to(0.0, -mark_lo);
        path.line_to(0.0, -mark_hi);

        canvas.stroke_path(&path, &mark_paint);

        canvas.restore();
    }
}
