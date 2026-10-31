// Textured dest-rect blit for the zoomed-out pixel-column LOD.

struct Uniforms {
  viewport: vec2<f32>,
  dest: vec4<f32>,
};
@group(0) @binding(0) var<uniform> uni: Uniforms;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> }

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
  var corners = array<vec2<f32>, 6>(
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
    vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0)
  );
  let uv = corners[vid];
  let x = uni.dest.x + uv.x * uni.dest.z;
  let y = uni.dest.y + uv.y * uni.dest.w;
  var o: VsOut;
  o.pos = vec4((x / uni.viewport.x) * 2.0 - 1.0, 1.0 - (y / uni.viewport.y) * 2.0, 0.0, 1.0);
  o.uv = uv;
  return o;
}

@fragment
fn fs_main(v: VsOut) -> @location(0) vec4<f32> {
  return textureSampleLevel(tex, samp, v.uv, 0.0);
}
