struct Uniforms {
    viewport: vec4<f32>,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct Instance {
    @location(0) start: vec4<f32>,
    @location(1) end: vec4<f32>,
    @location(2) start_color: vec4<f32>,
    @location(3) end_color: vec4<f32>,
    @location(4) third_color: vec4<f32>,
    @location(5) style: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) start_color: vec4<f32>,
    @location(1) @interpolate(flat) end_color: vec4<f32>,
    @location(2) @interpolate(flat) third_color: vec4<f32>,
    @location(3) @interpolate(flat) wing: f32,
    @location(4) @interpolate(flat) tri_a: vec2<f32>,
    @location(5) @interpolate(flat) tri_b: vec2<f32>,
    @location(6) @interpolate(flat) tri_c: vec2<f32>,
    @location(7) blend_color: vec4<f32>,
}

@vertex
fn ribbon_vertex(
    @builtin(vertex_index) vertex_index: u32,
    instance: Instance,
) -> VertexOutput {
    let points = array<vec2<f32>, 6>(
        instance.start.xy, instance.start.zw, instance.end.xy,
        instance.start.xy, instance.end.xy, instance.end.xy,
    );
    let colors = array<vec4<f32>, 6>(
        instance.start_color, instance.end_color, instance.third_color,
        instance.start_color, instance.third_color, instance.third_color,
    );
    let point = points[vertex_index];
    let clip = vec2(
        point.x / uniforms.viewport.x * 2.0 - 1.0,
        1.0 - point.y / uniforms.viewport.y * 2.0,
    );
    return VertexOutput(
        vec4(clip, 0.0, 1.0),
        instance.start_color,
        instance.end_color,
        instance.third_color,
        instance.style.x,
        instance.start.xy,
        instance.start.zw,
        instance.end.xy,
        colors[vertex_index],
    );
}

fn cross2(a: vec2<f32>, b: vec2<f32>) -> f32 {
    return a.x * b.y - a.y * b.x;
}

const COLOR_STEP: f32 = 1.0 / 255.0;
const COLOR_CEIL: f32 = 254.0 / 255.0;

fn ribbon_ink(vertex: VertexOutput) -> vec4<f32> {
    if vertex.wing > 0.5 {
        // Outer A/B -> X edge stays at zero; the base-to-X edge keeps the
        // body-edge U/V -> 100 ramp.
        let edge_a = vertex.tri_b - vertex.tri_a;
        let edge_b = vertex.tri_c - vertex.tri_a;
        let area = cross2(edge_a, edge_b);
        if abs(area) < 1e-6 {
            return vec4(0.0);
        }
        let offset = vertex.position.xy - vertex.tri_a;
        let base_weight = cross2(offset, edge_b) / area;
        let peak_weight = cross2(edge_a, offset) / area;
        let outer_weight = 1.0 - base_weight - peak_weight;
        let base_alpha = outer_weight * vertex.start_color.a
            + base_weight * vertex.end_color.a
            + peak_weight * vertex.third_color.a;
        let edge_weight = base_weight + outer_weight;
        // At the shared X endpoint the body edge is at peak; keep the wing
        // base at peak there too. Away from X the outer edge remains zero.
        let mask = select(1.0, base_weight / max(edge_weight, 1e-6), edge_weight > 1e-6);
        let alpha = base_alpha * mask;
        return vec4(vertex.start_color.rgb * alpha, alpha);
    }
    let alpha = vertex.blend_color.a;
    return vec4(vertex.blend_color.rgb * alpha, alpha);
}

@fragment
fn ribbon_fragment(vertex: VertexOutput) -> @location(0) vec4<f32> {
    return ribbon_ink(vertex);
}

@fragment
fn ribbon_additive_fragment(vertex: VertexOutput) -> @location(0) vec4<f32> {
    let ink = ribbon_ink(vertex);
    let alpha = ink.a;
    let color = ink.rgb / max(alpha, 1e-6);
    let rgb = max(color, vec3<f32>(COLOR_STEP));
    return vec4(rgb * alpha, alpha);
}

@fragment
fn ribbon_multiply_fragment(vertex: VertexOutput) -> @location(0) vec4<f32> {
    let ink = ribbon_ink(vertex);
    let alpha = ink.a;
    let color = ink.rgb / max(alpha, 1e-6);
    let rgb = min(color, vec3<f32>(COLOR_CEIL));
    let multiplier = mix(vec3<f32>(1.0), rgb, alpha);
    return vec4(multiplier, 1.0);
}
