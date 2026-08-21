//! WGSL for the hybrid GPU timeline.
//!
//! Pixel-column path: nearest-neighbor blit of the CPU raster.
//! Instanced path: SDF rounded rects + an analytical drop shadow.
//!
//! Shadow integral is Evan Wallace's "Fast Rounded Rectangle Shadows"
//! (https://madebyevan.com/shaders/fast-rounded-rectangle-shadows/).
//! Copied into this crate (public article / MIT-clean algorithm).

pub const BLIT_WGSL: &str = r#"
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
  var p = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
  var o: VsOut;
  o.pos = vec4(p[i], 0.0, 1.0);
  o.uv = vec2(p[i].x * 0.5 + 0.5, 1.0 - (p[i].y * 0.5 + 0.5));
  return o;
}
@fragment
fn fs_main(v: VsOut) -> @location(0) vec4<f32> {
  return textureSampleLevel(tex, samp, v.uv, 0.0);
}
"#;

pub const INSTANCE_WGSL: &str = r#"
struct Uniforms {
  viewport: vec2<f32>,
  playhead_x: f32,
  _pad: f32,
};
@group(0) @binding(0) var<uniform> uni: Uniforms;

struct VsIn {
  @location(0) rect: vec4<f32>,
  @location(1) color: vec4<f32>,
  @location(2) extra: vec2<f32>,
};

struct VsOut {
  @builtin(position) pos: vec4<f32>,
  @location(0) local: vec2<f32>,
  @location(1) color: vec4<f32>,
  @location(2) half_size: vec2<f32>,
  @location(3) radius: f32,
  @location(4) pix: vec2<f32>,
};

const SHADOW_PAD: f32 = 10.0;
const SHADOW_SIGMA: f32 = 2.2;
const SHADE: f32 = 0.94;

@vertex
fn vs_main(inst: VsIn, @builtin(vertex_index) vid: u32) -> VsOut {
  var corners = array<vec2<f32>, 6>(
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
    vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0)
  );
  let uv = corners[vid];
  let pad = SHADOW_PAD;
  let x = inst.rect.x - pad + uv.x * (inst.rect.z + 2.0 * pad);
  let y = inst.rect.y - pad + uv.y * (inst.rect.w + 2.0 * pad);
  var o: VsOut;
  o.pos = vec4(
    (x / uni.viewport.x) * 2.0 - 1.0,
    1.0 - (y / uni.viewport.y) * 2.0,
    0.0,
    1.0
  );
  let cx = inst.rect.x + inst.rect.z * 0.5;
  let cy = inst.rect.y + inst.rect.w * 0.5;
  o.local = vec2(x - cx, y - cy);
  o.half_size = vec2(inst.rect.z, inst.rect.w) * 0.5;
  o.color = inst.color;
  o.radius = inst.extra.x;
  o.pix = vec2(x - inst.rect.x, y - inst.rect.y);
  return o;
}

fn sd_rounded_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
  let q = abs(p) - b + vec2(r);
  return min(max(q.x, q.y), 0.0) + length(max(q, vec2(0.0))) - r;
}

fn gaussian(x: f32, sigma: f32) -> f32 {
  let pi = 3.141592653589793;
  return exp(-(x * x) / (2.0 * sigma * sigma)) / (sqrt(2.0 * pi) * sigma);
}

fn erf_approx(x: f32) -> f32 {
  let s = sign(x);
  let a = abs(x);
  var t = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
  t = t * t;
  return s - s / (t * t);
}

fn rounded_box_shadow_x(x: f32, y: f32, sigma: f32, corner: f32, half: vec2<f32>) -> f32 {
  let delta = min(half.y - corner - abs(y), 0.0);
  let curved = half.x - corner + sqrt(max(0.0, corner * corner - delta * delta));
  return 0.5 + 0.5 * erf_approx((x + curved) * (sqrt(0.5) / sigma));
}

// https://madebyevan.com/shaders/fast-rounded-rectangle-shadows/
fn rounded_box_shadow(point: vec2<f32>, half: vec2<f32>, corner: f32, sigma: f32) -> f32 {
  let low = point.y - half.y;
  let high = point.y + half.y;
  let start = clamp(-3.0 * sigma, low, high);
  let end = clamp(3.0 * sigma, low, high);
  let step = (end - start) / 4.0;
  var y = start + step * 0.5;
  var value = 0.0;
  for (var i = 0; i < 4; i++) {
    value += rounded_box_shadow_x(point.x, point.y - y, sigma, corner, half)
      * gaussian(y, sigma) * step;
    y += step;
  }
  return value;
}

@fragment
fn fs_main(v: VsOut) -> @location(0) vec4<f32> {
  let d = sd_rounded_box(v.local, v.half_size, v.radius);
  let aa = fwidth(d) * 0.75;
  let fill = 1.0 - smoothstep(-aa, aa, d);
  let border = 1.0 - smoothstep(-aa, aa, abs(d) - 0.6);
  let shadow = rounded_box_shadow(v.local + vec2(0.8, 1.2), v.half_size, v.radius, SHADOW_SIGMA);
  let shade_mix = select(1.0, SHADE, v.pix.x < 3.0 && fill > 0.5);
  var rgb = v.color.rgb * shade_mix;
  rgb = mix(rgb, vec3(1.0), border * 0.35 * fill);
  let alpha = max(fill * v.color.a, shadow * 0.28 * (1.0 - fill));
  return vec4(rgb, alpha);
}
"#;
