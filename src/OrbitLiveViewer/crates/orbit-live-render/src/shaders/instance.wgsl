// Instanced scope rects: SDF rounded boxes + an analytical drop shadow.
//
// Shadow integral is Evan Wallace's "Fast Rounded Rectangle Shadows"
// (https://madebyevan.com/shaders/fast-rounded-rectangle-shadows/).
// Copied into this crate (public article / MIT-clean algorithm).

// Must be a multiple of 16 bytes: WebGL2 and other downlevel adapters lack
// DownlevelFlags::BUFFER_BINDINGS_NOT_16_BYTE_ALIGNED. Three trailing scalars
// (not a vec3, whose align-16 would push size to 48) land this at 32, matching
// the 32 bytes pack_inst_uniforms() writes in timeline.rs.
struct Uniforms {
  viewport: vec2<f32>,
  origin: vec2<f32>,
  time: f32,
  _pad0: f32,
  _pad1: f32,
  _pad2: f32,
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
  @location(5) mark: f32,
};

const SHADOW_PAD: f32 = 6.0;
const SHADOW_SIGMA: f32 = 1.35;
const SIBLING_RGB: vec3<f32> = vec3(0.392, 0.710, 0.965);
const SELECTED_RGB: vec3<f32> = vec3(0.0, 0.502, 1.0);
const PULSE_PERIOD: f32 = 1.2;
const PULSE_RADIUS: f32 = 0.45;
const PULSE_SIGMA: f32 = 0.18;
const PULSE_BRIGHT: f32 = 0.05;

fn selected_pulse(time: f32) -> f32 {
  return 0.5 + 0.5 * sin(time * 6.28318530718 / PULSE_PERIOD);
}

@vertex
fn vs_main(inst: VsIn, @builtin(vertex_index) vid: u32) -> VsOut {
  var corners = array<vec2<f32>, 6>(
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
    vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0)
  );
  let uv = corners[vid];
  let mark = inst.extra.y;
  let selected = mark > 1.5 && mark < 2.5;
  let pulse = select(0.0, selected_pulse(uni.time), selected);
  let lift = select(0.0, -0.8, selected);
  let pad = SHADOW_PAD + select(0.0, 1.0, selected) + pulse * PULSE_RADIUS;
  let x = uni.origin.x + inst.rect.x - pad + uv.x * (inst.rect.z + 2.0 * pad);
  let y = uni.origin.y + inst.rect.y + lift - pad + uv.y * (inst.rect.w + 2.0 * pad);
  var o: VsOut;
  o.pos = vec4(
    (x / uni.viewport.x) * 2.0 - 1.0,
    1.0 - (y / uni.viewport.y) * 2.0,
    0.0,
    1.0
  );
  let cx = inst.rect.x + inst.rect.z * 0.5;
  let cy = inst.rect.y + inst.rect.w * 0.5 + lift;
  o.local = vec2(x - (uni.origin.x + cx), y - (uni.origin.y + cy));
  o.half_size = vec2(inst.rect.z, inst.rect.w) * 0.5;
  o.color = inst.color;
  o.radius = inst.extra.x;
  o.pix = vec2(x - (uni.origin.x + inst.rect.x), y - (uni.origin.y + inst.rect.y + lift));
  o.mark = mark;
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
  let hover = v.mark > 0.5 && v.mark < 1.5;
  let selected = v.mark > 1.5 && v.mark < 2.5;
  let sibling = v.mark > 2.5 && v.mark < 3.5;
  let dimmed = v.mark > 3.5 && v.mark < 4.5;
  let pulse = select(0.0, selected_pulse(uni.time), selected);
  let d = sd_rounded_box(v.local, v.half_size, v.radius);
  let aa = fwidth(d) * 0.75;
  let fill = 1.0 - smoothstep(-aa, aa, d);
  let border = 1.0 - smoothstep(-aa, aa, abs(d) - 0.5);
  let sigma = SHADOW_SIGMA + select(0.0, 0.35, selected) + pulse * PULSE_SIGMA;
  let shadow = rounded_box_shadow(v.local + vec2(0.4, 0.7), v.half_size, v.radius, sigma);
  var rgb = v.color.rgb;
  if dimmed {
    let luma = dot(rgb, vec3(0.2126, 0.7152, 0.0722));
    rgb = mix(rgb, vec3(luma), 0.84) * 0.36;
  }
  if sibling {
    rgb = SIBLING_RGB;
  } else if selected {
    rgb = SELECTED_RGB * (1.0 + pulse * PULSE_BRIGHT);
  }
  let top = v.pix.y < 1.15 && fill > 0.5;
  rgb = rgb * select(1.0, 1.08, top);
  rgb = rgb * select(1.0, 1.05, hover);
  var rim = vec3(0.92, 0.93, 0.95);
  var rim_w = 0.18;
  if selected {
    rim_w = 0.40;
  } else if hover {
    rim_w = 0.36;
  }
  rgb = mix(rgb, rim, border * rim_w * fill);
  let shadow_a = select(0.10, 0.20, selected);
  let alpha = max(fill * v.color.a, shadow * shadow_a * (1.0 - fill));
  return vec4(rgb, alpha);
}
