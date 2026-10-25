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
    @location(0) @interpolate(flat) start: vec4<f32>,
    @location(1) @interpolate(flat) end: vec4<f32>,
    @location(2) @interpolate(flat) start_color: vec4<f32>,
    @location(3) @interpolate(flat) end_color: vec4<f32>,
    @location(4) @interpolate(flat) style: vec4<f32>,
    @location(5) blend_color: vec4<f32>,
    @location(6) @interpolate(flat) tri_a: vec2<f32>,
    @location(7) @interpolate(flat) tri_b: vec2<f32>,
    @location(8) @interpolate(flat) tri_c: vec2<f32>,
    @location(9) @interpolate(flat) third_color: vec4<f32>,
}

@vertex
fn ribbon_vertex(
    @builtin(vertex_index) vertex_index: u32,
    instance: Instance,
) -> VertexOutput {
    if instance.style.y > 0.5 {
        // Per-vertex opacity field for a triangle fallback or crossing fan.
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
            instance.start,
            instance.end,
            instance.start_color,
            instance.end_color,
            instance.style,
            colors[vertex_index],
            instance.start.xy,
            instance.start.zw,
            instance.end.xy,
            instance.third_color,
        );
    }
    let corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
    );
    let padding = max(instance.style.x, 1.0) * 0.5 + 2.0;
    let low = min(
        min(instance.start.xy, instance.start.zw),
        min(instance.end.xy, instance.end.zw),
    ) - padding;
    let high = max(
        max(instance.start.xy, instance.start.zw),
        max(instance.end.xy, instance.end.zw),
    ) + padding;
    let point = mix(low, high, corners[vertex_index]);
    let clip = vec2(
        point.x / uniforms.viewport.x * 2.0 - 1.0,
        1.0 - point.y / uniforms.viewport.y * 2.0,
    );
    return VertexOutput(
        vec4(clip, 0.0, 1.0),
        instance.start,
        instance.end,
        instance.start_color,
        instance.end_color,
        instance.style,
        vec4(0.0),
        vec2(0.0),
        vec2(0.0),
        vec2(0.0),
        vec4(0.0),
    );
}

fn cross2(a: vec2<f32>, b: vec2<f32>) -> f32 {
    return a.x * b.y - a.y * b.x;
}

fn segment_distance(point: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let along = b - a;
    let length_squared = max(dot(along, along), 1e-6);
    let fraction = clamp(dot(point - a, along) / length_squared, 0.0, 1.0);
    return distance(point, a + along * fraction);
}

@fragment
fn ribbon_fragment(vertex: VertexOutput) -> @location(0) vec4<f32> {
    if vertex.style.y > 0.5 {
        if vertex.style.z > 0.5 {
            let edge_a = vertex.tri_b - vertex.tri_a;
            let edge_b = vertex.tri_c - vertex.tri_a;
            let area = cross2(edge_a, edge_b);
            if abs(area) < 1e-6 {
                return vec4(0.0);
            }
            let offset = vertex.position.xy - vertex.tri_a;
            let am_weight = cross2(offset, edge_b) / area;
            let end_weight = cross2(edge_a, offset) / area;
            let x_weight = 1.0 - am_weight - end_weight;
            let peak = x_weight * vertex.start_color.a
                + am_weight * vertex.end_color.a
                + end_weight * vertex.third_color.a;
            let median_weight = am_weight + end_weight;
            let mask = select(0.0, am_weight / max(median_weight, 1e-6), median_weight > 1e-6);
            let alpha = peak * mask;
            return vec4(vertex.start_color.rgb * alpha, alpha);
        }
        let alpha = vertex.blend_color.a;
        return vec4(vertex.blend_color.rgb * alpha, alpha);
    }
    let point = vertex.position.xy;
    let a0 = vertex.start.xy;
    let b0 = vertex.start.zw;
    let a1 = vertex.end.xy;
    let b1 = vertex.end.zw;
    // The four corners in exposure order: old a, old b, new b, new a.
    let corners = array<vec2<f32>, 4>(a0, b0, b1, a1);
    let area = cross2(corners[1] - corners[0], corners[2] - corners[1])
        + cross2(corners[3] - corners[2], corners[0] - corners[3]);
    let winding = select(-1.0, 1.0, area >= 0.0);
    var inside = true;
    var nearest = 1e9;
    for (var index = 0u; index < 4u; index += 1u) {
        let a = corners[index];
        let b = corners[(index + 1u) % 4u];
        let edge = b - a;
        let offset = point - a;
        if winding * cross2(edge, offset) < 0.0 {
            inside = false;
        }
        let along = clamp(dot(offset, edge) / max(dot(edge, edge), 1e-6), 0.0, 1.0);
        nearest = min(nearest, distance(point, a + edge * along));
    }
    let signed_distance = select(-nearest, nearest, inside);
    // A small feather keeps the swept polygons soft without painting a wide
    // halo across the sharp line body that joins them.
    let coverage = clamp(signed_distance + 0.75, 0.0, 1.0);
    // The exposure fraction is measured against the two edge poses, so the
    // gradient follows the recorded path rather than a synthetic fade.
    let distance_old = segment_distance(point, a0, b0);
    let distance_new = segment_distance(point, a1, b1);
    let exposure = distance_old / max(distance_old + distance_new, 1e-4);
    let color = mix(vertex.start_color, vertex.end_color, exposure);
    let alpha = color.a * coverage;
    return vec4(color.rgb * alpha, alpha);
}
