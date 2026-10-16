struct Needle {
    inverse_x: vec4<f32>,
    inverse_y: vec4<f32>,
    needle_bounds: vec4<f32>,
    glow_bounds: vec4<f32>,
    angles: vec4<f32>,
    shape: vec4<f32>,
    bend: vec4<f32>,
}

struct Uniforms {
    viewport: vec4<f32>,
    ink: vec4<f32>,
    light: vec4<f32>,
    lamp: vec4<f32>,
    halo: vec4<f32>,
    hot: vec4<f32>,
    needles: array<Needle, 2>,
}

@group(1) @binding(0) var background: texture_2d<f32>;

@vertex
fn background_vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corners = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    return vec4(corners[index], 0.0, 1.0);
}

@fragment
fn background_fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(background, vec2<i32>(position.xy), 0);
}

struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) channel: u32,
}

fn quad(index: u32, channel: u32, bounds: vec4<f32>) -> Vertex {
    let corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
    );
    let point = mix(bounds.xy, bounds.zw, corners[index]);
    return Vertex(vec4(point.x / data.viewport.x * 2.0 - 1.0,
                       1.0 - point.y / data.viewport.y * 2.0, 0.0, 1.0), channel);
}

@vertex
fn needle_vertex(@builtin(vertex_index) index: u32, @builtin(instance_index) channel: u32) -> Vertex {
    return quad(index, channel, data.needles[channel].needle_bounds);
}

@vertex
fn glow_vertex(@builtin(vertex_index) index: u32, @builtin(instance_index) channel: u32) -> Vertex {
    return quad(index, channel, data.needles[channel].glow_bounds);
}

fn local_point(screen: vec2<f32>, needle: Needle) -> vec2<f32> {
    let point = vec3(screen, 1.0);
    return vec2(dot(needle.inverse_x.xyz, point), dot(needle.inverse_y.xyz, point));
}

fn rounded_bend(point: vec2<f32>, start: vec2<f32>, hinge: vec2<f32>, end: vec2<f32>, needle: Needle) -> f32 {
    let support = needle.shape.y * 0.5 + 1.0 / needle.inverse_x.w;
    if distance(point, hinge) > needle.bend.z + support {
        return 0.0;
    }
    // Short chords approximate the quadratic fillet; distance coverage rounds their joins.
    var previous = start;
    var nearest = needle.bend.z + support;
    for (var index = 1u; index <= 8u; index += 1u) {
        let fraction = f32(index) / 8.0;
        let current = mix(mix(start, hinge, fraction), mix(hinge, end, fraction), fraction);
        let chord = current - previous;
        let along = clamp(dot(point - previous, chord) / dot(chord, chord), 0.0, 1.0);
        nearest = min(nearest, distance(point, previous + along * chord));
        previous = current;
    }
    return coverage(needle.shape.y * 0.5 - nearest, needle.inverse_x.w);
}

fn bent_stroke(point: vec2<f32>, angle: f32, needle: Needle) -> f32 {
    let direction = vec2(cos(angle), sin(angle));
    let hinge = direction * needle.bend.x;
    let outer_angle = angle - needle.bend.y;
    let outer = vec2(cos(outer_angle), sin(outer_angle));
    let start = hinge - needle.bend.z * direction;
    let end = hinge + needle.bend.z * outer;
    let inner_alpha = stroke_coverage(point, direction, needle.bend.x - needle.bend.z, needle);
    let outer_alpha = stroke_coverage(point - end, outer, needle.shape.x - needle.bend.x - needle.bend.z, needle);
    let curve_alpha = rounded_bend(point, start, hinge, end, needle);
    return max(curve_alpha, max(inner_alpha, outer_alpha));
}

// Straighten each radial cross-section of the bent shaft for its sweep gradient.
// The curve and outer section have increasing radius, so each radius has one phase.
fn unbend(point: vec2<f32>, needle: Needle) -> vec2<f32> {
    let radius = length(point);
    let hinge = vec2(needle.bend.x, 0.0);
    let start = hinge - vec2(needle.bend.z, 0.0);
    if radius <= start.x {
        return point;
    }
    let outer = vec2(cos(needle.bend.y), -sin(needle.bend.y));
    let end = hinge + needle.bend.z * outer;
    var shaft: vec2<f32>;
    if radius >= length(end) {
        let along = sqrt(radius * radius - needle.bend.x * needle.bend.x * outer.y * outer.y)
            - needle.bend.x * outer.x;
        shaft = hinge + along * outer;
    } else {
        var low = 0.0;
        var high = 1.0;
        // Invert radius on the quadratic fillet, without sampling motion poses.
        for (var index = 0u; index < 16u; index += 1u) {
            let fraction = (low + high) * 0.5;
            shaft = mix(mix(start, hinge, fraction), mix(hinge, end, fraction), fraction);
            if length(shaft) < radius {
                low = fraction;
            } else {
                high = fraction;
            }
        }
    }
    let direction = normalize(shaft);
    return vec2(dot(point, direction), cross(direction, point));
}

@fragment
fn needle_fragment(vertex: Vertex) -> @location(0) vec4<f32> {
    let needle = data.needles[vertex.channel];
    let point = local_point(vertex.position.xy, needle);
    var sweep = needle;
    var sweep_point = point;
    var stroke: f32;
    if needle.bend.y > 0.0 {
        stroke = bent_stroke(point, needle.angles.x, needle);
        sweep_point = unbend(point, needle);
        let hinge = vec2(needle.bend.x, 0.0);
        let outer = vec2(cos(needle.bend.y), -sin(needle.bend.y));
        sweep.shape.x = length(hinge + (needle.shape.x - needle.bend.x) * outer);
    } else {
        let direction = vec2(cos(needle.angles.x), sin(needle.angles.x));
        stroke = stroke_coverage(point, direction, needle.shape.x, needle);
    }
    let direction = vec2(cos(needle.angles.x), sin(needle.angles.x));
    let tip = direction * sweep.shape.x;
    let left = wing(sweep_point, tip, needle.angles.y, sweep);
    let right = wing(sweep_point, tip, needle.angles.z, sweep);
    let alpha = (1.0 - (1.0 - stroke) * (1.0 - left) * (1.0 - right)) * radial_opacity(sweep_point, sweep);
    return vec4(data.ink.rgb * alpha, alpha);
}

@fragment
fn glow_fragment(vertex: Vertex) -> @location(0) vec4<f32> {
    let needle = data.needles[vertex.channel];
    let strength = needle.shape.w;
    let radius = length(local_point(vertex.position.xy, needle) - data.light.xy);
    let squared = strength * strength;
    let fourth = squared * squared;
    let fade = 1.0 - clamp((radius - data.light.z) / (data.light.w - data.light.z), 0.0, 1.0);
    let glow_alpha = squared * 0.2 * fade * fade * fade;
    let glow_rgb = mix(data.halo.rgb, data.hot.rgb, fourth * 0.1) * glow_alpha;
    let lamp_alpha = strength * coverage(data.light.z - radius, needle.inverse_x.w);
    let lamp_rgb = mix(data.lamp.rgb, data.hot.rgb, squared) * lamp_alpha;
    let alpha = lamp_alpha + glow_alpha * (1.0 - lamp_alpha);
    return vec4(lamp_rgb + glow_rgb * (1.0 - lamp_alpha), alpha);
}
