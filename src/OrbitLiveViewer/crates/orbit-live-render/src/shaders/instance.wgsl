// Instanced scope rects: SDF rounded boxes.

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

// Just enough slack around the rect for the antialiased edge and, on a
// selected scope, its glow. There is no drop shadow: it expanded every quad
// by six pixels a side -- huge overdraw on a dense track -- to draw a faint
// offset copy that read as a rendering artifact more than a shadow.
const AA_PAD: f32 = 2.0;
const SIBLING_RGB: vec3<f32> = vec3(0.392, 0.710, 0.965);
const SELECTED_RGB: vec3<f32> = vec3(0.0, 0.502, 1.0);
const PULSE_PERIOD: f32 = 1.2;
const PULSE_RADIUS: f32 = 0.45;
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
  let pad = AA_PAD + select(0.0, 1.0, selected) + pulse * PULSE_RADIUS;
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





@fragment
fn fs_main(v: VsOut) -> @location(0) vec4<f32> {
  let hover = v.mark > 0.5 && v.mark < 1.5;
  let selected = v.mark > 1.5 && v.mark < 2.5;
  let sibling = v.mark > 2.5 && v.mark < 3.5;
  let dimmed = v.mark > 3.5 && v.mark < 4.5;
  let inactive = v.mark > 4.5 && v.mark < 5.5;
  let same_pid = v.mark > 5.5 && v.mark < 6.5;
  let pulse = select(0.0, selected_pulse(uni.time), selected);
  let d = sd_rounded_box(v.local, v.half_size, v.radius);
  let aa = fwidth(d) * 0.75;
  let fill = 1.0 - smoothstep(-aa, aa, d);
  let border = 1.0 - smoothstep(-aa, aa, abs(d) - 0.5);
  var rgb = v.color.rgb;
  if dimmed {
    let luma = dot(rgb, vec3(0.2126, 0.7152, 0.0722));
    rgb = mix(rgb, vec3(luma), 0.84) * 0.36;
  }
  // C++ Orbit's flat greys: (100,100,100) outside the selection,
  // (140,140,140) for the selected process's other threads on a core.
  if inactive {
    rgb = vec3(0.392, 0.392, 0.392);
  } else if same_pid {
    rgb = vec3(0.549, 0.549, 0.549);
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
  return vec4(rgb, fill * v.color.a);
}
