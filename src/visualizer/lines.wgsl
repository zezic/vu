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

@fragment
fn line_fragment(vertex: VertexOutput) -> @location(0) vec4<f32> {
    let width = max(vertex.style.x, 0.7);
    let nearest = segment_distance(
        vertex.position.xy,
        vertex.endpoints.xy,
        vertex.endpoints.zw,
    );
    let coverage = clamp(width * 0.5 + 0.5 - nearest, 0.0, 1.0);
    let along = vertex.endpoints.zw - vertex.endpoints.xy;
    let fraction = clamp(
        dot(vertex.position.xy - vertex.endpoints.xy, along)
            / max(dot(along, along), 1e-6),
        0.0,
        1.0,
    );
    let color = mix(vertex.start_color, vertex.end_color, fraction);
    let alpha = color.a * coverage;
    return vec4(color.rgb * alpha, alpha);
}
