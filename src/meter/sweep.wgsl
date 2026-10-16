fn cross(a: vec2<f32>, b: vec2<f32>) -> f32 {
    return a.x * b.y - a.y * b.x;
}

fn coverage(distance: f32, scale: f32) -> f32 {
    return clamp(0.5 + distance * scale, 0.0, 1.0);
}

fn stroke_coverage(point: vec2<f32>, direction: vec2<f32>, radius: f32, needle: Needle) -> f32 {
    let column_x = vec2(needle.inverse_x.x, needle.inverse_y.x);
    let column_y = vec2(needle.inverse_x.y, needle.inverse_y.y);
    let along = dot(point, direction);
    let along_support = 0.5 * (abs(dot(direction, column_x)) + abs(dot(direction, column_y)));
    let across_support = 0.5 * (abs(cross(direction, column_x)) + abs(cross(direction, column_y)));
    if abs(cross(direction, point)) > needle.shape.y * 0.5 + across_support
        || along < -along_support || along > radius + along_support {
        return 0.0;
    }
    // The standard 16-sample pattern keeps subpixel strokes consistent with the face rasterizer.
    let pattern = array<u32, 16>(1u, 8u, 4u, 11u, 15u, 7u, 3u, 12u, 0u, 9u, 5u, 13u, 2u, 10u, 6u, 14u);
    var hits = 0u;
    for (var index = 0u; index < 16u; index += 1u) {
        let offset = (vec2(f32(pattern[index]), f32(index)) + 0.5) / 16.0 - 0.5;
        let sample_point = point + vec2(dot(needle.inverse_x.xy, offset), dot(needle.inverse_y.xy, offset));
        let along = dot(sample_point, direction);
        if along >= 0.0 && along <= radius && abs(cross(direction, sample_point)) <= needle.shape.y * 0.5 {
            hits += 1u;
        }
    }
    return f32(hits) / 16.0;
}

fn wing_mask(point: vec2<f32>, tip: vec2<f32>, edge_angle: f32, needle: Needle) -> f32 {
    let radius = needle.shape.x;
    let scale = needle.inverse_x.w;
    let delta = edge_angle - needle.angles.x;
    if needle.angles.w == 0.0 || abs(delta) * radius * scale <= 0.5 {
        return 0.0;
    }
    let edge = radius * vec2(cos(edge_angle), sin(edge_angle));
    let direction = sign(cross(tip, edge));
    let d0 = direction * cross(tip, point) / radius;
    let d1 = direction * cross(edge - tip, point - tip) / length(edge - tip);
    let d2 = direction * cross(-edge, point - edge) / radius;
    let alpha = coverage(min(d0, min(d1, d2)), scale);
    return alpha;
}

fn wing(point: vec2<f32>, tip: vec2<f32>, edge_angle: f32, needle: Needle) -> f32 {
    let delta = edge_angle - needle.angles.x;
    if needle.angles.w == 0.0 || abs(delta) * needle.shape.x * needle.inverse_x.w <= 0.5 {
        return 0.0;
    }
    let alpha = wing_mask(point, tip, edge_angle, needle);
    let angle = atan2(point.y, point.x) + 6.28318530718;
    return alpha * needle.angles.w * linear_wing_fade((angle - needle.angles.x) / delta);
}

fn radial_opacity(point: vec2<f32>, needle: Needle) -> f32 {
    let peak = needle.shape.z;
    if peak == 1.0 {
        return 1.0;
    }
    let radius = length(point) / needle.shape.x;
    if radius <= peak {
        return 1.0;
    }
    let core = sqrt(peak);
    let blur = needle.angles.w;
    let core_alpha = 1.0 - blur + blur * core;
    if radius <= core {
        return mix(1.0, core_alpha, (radius - peak) / (core - peak));
    }
    return mix(core_alpha, 1.0 - blur + blur * peak, clamp((radius - core) / (1.0 - core), 0.0, 1.0));
}

