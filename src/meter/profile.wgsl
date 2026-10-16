// VU's linear angular wing and its area, before geometry-specific coverage.
fn linear_wing_fade(fraction: f32) -> f32 {
    return 1.0 - clamp(fraction, 0.0, 1.0);
}

fn linear_wing_area(width: f32) -> f32 {
    return 0.5 * width;
}
