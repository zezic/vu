struct OverlayUniforms {
    opacity: vec4<f32>,
}

@group(0) @binding(0) var overlay_texture: texture_2d<f32>;
@group(0) @binding(1) var<uniform> uniforms: OverlayUniforms;

@vertex
fn overlay_vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corners = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    return vec4(corners[index], 0.0, 1.0);
}

// Vello rasterizes the foreground with straight alpha, so both fragments
// premultiply before the pipeline's blend state consumes the color.
@fragment
fn overlay_color_fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let ink = textureLoad(overlay_texture, vec2<i32>(position.xy), 0);
    let alpha = ink.a * uniforms.opacity.x;
    return vec4(ink.rgb * alpha, alpha);
}

@fragment
fn overlay_multiply_fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let ink = textureLoad(overlay_texture, vec2<i32>(position.xy), 0);
    let alpha = ink.a * uniforms.opacity.x;
    let color = ink.rgb * alpha;
    return vec4(vec3(1.0) - alpha + color, 1.0);
}
