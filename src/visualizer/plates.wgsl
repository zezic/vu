struct Uniforms {
    // Screen width, screen height, mask texels per screen pixel, padding.
    viewport: vec4<f32>,
    // Contrast plate ink.
    ink: vec4<f32>,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct Plate {
    // Screen bounds of the swept support: x0, y0, x1, y1.
    bounds: vec4<f32>,
    motion: vec2<f32>,
    edge_count: u32,
    padding: u32,
    // Edge normal xy and support offset, in screen space.
    // Keep the length in sync with MAX_PLATE_EDGES in render.rs.
    edges: array<vec4<f32>, 8>,
}

@group(0) @binding(1) var<storage, read> plates: array<Plate>;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) plate: u32,
}

@vertex
fn plate_vertex(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    let corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
    );
    let bounds = plates[instance_index].bounds;
    let point = mix(bounds.xy, bounds.zw, corners[vertex_index]);
    let clip = vec2(
        point.x / uniforms.viewport.x * 2.0 - 1.0,
        1.0 - point.y / uniforms.viewport.y * 2.0,
    );
    return VertexOutput(vec4(clip, 0.0, 1.0), instance_index);
}

@fragment
fn plate_fragment(vertex: VertexOutput) -> @location(0) vec4<f32> {
    let plate = plates[vertex.plate];
    // The mask is rasterized at a fraction of the screen resolution.
    let point = vertex.position.xy / uniforms.viewport.z;
    var low = -0.5;
    var high = 0.5;
    for (var index = 0u; index < plate.edge_count; index += 1u) {
        let edge = plate.edges[index];
        let normal = edge.xy;
        let along = dot(normal, plate.motion);
        let reach = edge.z - dot(normal, point);
        if (abs(along) < 1e-6) {
            if (reach < 0.0) {
                return vec4(0.0);
            }
        } else {
            let crossing = reach / along;
            if (along > 0.0) {
                high = min(high, crossing);
            } else {
                low = max(low, crossing);
            }
        }
    }
    let coverage = clamp(high - low, 0.0, 1.0);
    if (coverage <= 0.0) {
        return vec4(0.0);
    }
    return vec4(uniforms.ink.rgb, coverage);
}
