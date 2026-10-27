struct Uniforms {
    viewport: vec4<f32>,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct Instance {
    @location(0) endpoints: vec4<f32>,
    @location(1) start_color: vec4<f32>,
    @location(2) end_color: vec4<f32>,
    @location(3) style: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) endpoints: vec4<f32>,
    @location(1) @interpolate(flat) start_color: vec4<f32>,
    @location(2) @interpolate(flat) end_color: vec4<f32>,
    @location(3) @interpolate(flat) style: vec4<f32>,
}

struct LineInk {
    color: vec4<f32>,
    coverage: f32,
}

const COLOR_STEP: f32 = 1.0 / 255.0;
const COLOR_CEIL: f32 = 254.0 / 255.0;

@vertex
fn line_vertex(
    @builtin(vertex_index) vertex_index: u32,
    instance: Instance,
) -> VertexOutput {
    let corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
    );
    let padding = max(instance.style.x, 1.0) * 0.5 + 1.5;
    let low = min(instance.endpoints.xy, instance.endpoints.zw) - padding;
    let high = max(instance.endpoints.xy, instance.endpoints.zw) + padding;
    let point = mix(low, high, corners[vertex_index]);
    let clip = vec2(
        point.x / uniforms.viewport.x * 2.0 - 1.0,
        1.0 - point.y / uniforms.viewport.y * 2.0,
    );
    return VertexOutput(
        vec4(clip, 0.0, 1.0),
        instance.endpoints,
        instance.start_color,
        instance.end_color,
        instance.style,
    );
}

fn segment_distance(point: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let along = b - a;
    let length_squared = max(dot(along, along), 1e-6);
    let fraction = clamp(dot(point - a, along) / length_squared, 0.0, 1.0);
    return distance(point, a + along * fraction);
}

fn line_ink(vertex: VertexOutput) -> LineInk {
    let width = max(vertex.style.x, 0.7);
    let segment = vertex.endpoints.zw - vertex.endpoints.xy;
    let length_squared = dot(segment, segment);
    var coverage: f32;
    if vertex.style.y > 0.5 && length_squared > 1e-6 {
        // Flat-rectangle distance with a square cap: the stroke extends half
        // a width past each endpoint, unlike the round point-to-segment cap.
        let segment_length = sqrt(length_squared);
        let direction = segment / segment_length;
        let relative = vertex.position.xy - vertex.endpoints.xy;
        let along = dot(relative, direction);
        let across = abs(relative.x * direction.y - relative.y * direction.x);
        let local = vec2(
            abs(along - segment_length * 0.5) - segment_length * 0.5 - width * 0.5,
            across - width * 0.5,
        );
        let sdf = length(max(local, vec2(0.0))) + min(max(local.x, local.y), 0.0);
        coverage = clamp(0.5 - sdf, 0.0, 1.0);
    } else {
        let nearest = segment_distance(
            vertex.position.xy,
            vertex.endpoints.xy,
            vertex.endpoints.zw,
        );
        coverage = clamp(width * 0.5 + 0.5 - nearest, 0.0, 1.0);
    }
    let fraction = clamp(
        dot(vertex.position.xy - vertex.endpoints.xy, segment)
            / max(length_squared, 1e-6),
        0.0,
        1.0,
    );
    return LineInk(mix(vertex.start_color, vertex.end_color, fraction), coverage);
}

@fragment
fn line_fragment(vertex: VertexOutput) -> @location(0) vec4<f32> {
    let ink = line_ink(vertex);
    let alpha = ink.color.a * ink.coverage;
    return vec4(ink.color.rgb * alpha, alpha);
}

@fragment
fn line_additive_fragment(vertex: VertexOutput) -> @location(0) vec4<f32> {
    let ink = line_ink(vertex);
    let alpha = ink.color.a * ink.coverage;
    // An additive color with a zero component contributes nothing there.
    let rgb = max(ink.color.rgb, vec3<f32>(COLOR_STEP));
    return vec4(rgb * alpha, alpha);
}

@fragment
fn line_multiply_fragment(vertex: VertexOutput) -> @location(0) vec4<f32> {
    let ink = line_ink(vertex);
    let alpha = ink.color.a * ink.coverage;
    // A multiplicative color with a full component leaves that channel
    // untouched; step every full component down so the line always tints it.
    let rgb = min(ink.color.rgb, vec3<f32>(COLOR_CEIL));
    let multiplier = mix(vec3<f32>(1.0), rgb, alpha);
    return vec4(multiplier, 1.0);
}
