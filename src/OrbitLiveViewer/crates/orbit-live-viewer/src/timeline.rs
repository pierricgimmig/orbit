//! One egui `PaintCallback` for the hybrid wgpu timeline.
//!
//! Do not emit millions of egui `RectShape`s. Zoomed-out frames upload the
//! pixel-column raster and blit it. Zoomed-in frames upload visible SDF
//! instances only.

use egui::PaintCallback;
use egui_wgpu::wgpu;
use egui_wgpu::wgpu::util::DeviceExt;
use egui_wgpu::{Callback, CallbackResources, CallbackTrait, ScreenDescriptor};
use orbit_live_event::LaneKey;
use orbit_live_render::{
    collect_instances_layout, stacked_layout, ScopeInstance, TimelineLod, TrackIndex,
    BLIT_RECT_WGSL, INSTANCE_WGSL,
};

pub const INSTANCE_STRIDE: u64 = 48;

#[derive(Clone, Copy, Debug)]
pub struct ViewUniforms {
    pub viewport: [f32; 2],
    pub origin: [f32; 2],
    pub dest: [f32; 4],
}

impl ViewUniforms {
    pub fn from_rect(rect: egui::Rect, ppp: f32, screen_px: [f32; 2]) -> Self {
        let dest = [
            rect.min.x * ppp,
            rect.min.y * ppp,
            rect.width() * ppp,
            rect.height() * ppp,
        ];
        Self {
            viewport: [screen_px[0].max(1.0), screen_px[1].max(1.0)],
            origin: [dest[0], dest[1]],
            dest,
        }
    }
}

#[derive(Clone)]
pub enum TimelinePayload {
    Empty,
    Pixel {
        rgba: Vec<u8>,
        width: u32,
        height: u32,
        overlay: Vec<ScopeInstance>,
    },
    Instanced {
        instances: Vec<ScopeInstance>,
    },
}

impl TimelinePayload {
    pub fn from_index(
        index: &TrackIndex,
        t0: u64,
        t1: u64,
        width_pts: f32,
        lod: TimelineLod,
        pixels_per_point: f32,
        layout: &[(LaneKey, f32)],
        overlay: &[ScopeInstance],
    ) -> Self {
        let width_pts = width_pts.max(1.0);
        let layout_owned;
        let layout = if layout.is_empty() {
            let keys: Vec<LaneKey> = index.lanes().map(|(k, _)| k).collect();
            layout_owned = stacked_layout(&keys, 0.0);
            layout_owned.as_slice()
        } else {
            layout
        };
        let s = pixels_per_point.max(0.01);
        match lod {
            TimelineLod::Instanced => {
                let mut frame = collect_instances_layout(index, t0, t1, width_pts, layout);
                for inst in &mut frame.instances {
                    inst.x *= s;
                    inst.y *= s;
                    inst.w *= s;
                    inst.h *= s;
                    inst.radius *= s;
                }
                Self::Instanced {
                    instances: frame.instances,
                }
            }
            TimelineLod::PixelColumns => {
                let width_px = (width_pts * pixels_per_point).round().max(1.0) as usize;
                let keys: Vec<LaneKey> = layout.iter().map(|(k, _)| *k).collect();
                let raster = index.rasterize_pixel_ordered(t0, t1, width_px, &keys);
                let (mut rgba, height) = raster.to_rgba8_scaled();
                crate::theme::remap_rgba8(&mut rgba);
                let overlay = overlay
                    .iter()
                    .cloned()
                    .map(|mut inst| {
                        inst.x *= s;
                        inst.y *= s;
                        inst.w *= s;
                        inst.h *= s;
                        inst.radius *= s;
                        inst
                    })
                    .collect();
                Self::Pixel {
                    rgba,
                    width: width_px as u32,
                    height: height.max(1),
                    overlay,
                }
            }
        }
    }
}

pub fn paint_callback(
    rect: egui::Rect,
    payload: TimelinePayload,
    view: ViewUniforms,
) -> PaintCallback {
    Callback::new_paint_callback(rect, TimelineCallback { payload, view })
}

struct TimelineCallback {
    payload: TimelinePayload,
    view: ViewUniforms,
}

impl CallbackTrait for TimelineCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(gpu) = callback_resources.get_mut::<TimelineGpu>() {
            gpu.upload(device, queue, &self.payload, self.view);
        }
        Vec::new()
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &CallbackResources,
    ) {
        // egui-wgpu sets a courtesy viewport to the callback clip rect. Both
        // blit and instance shaders emit NDC from full-framebuffer pixels
        // (`x / viewport.x * 2 - 1` plus `uni.origin` / `uni.dest`), so reset
        // to the real surface. Scissor stays the clip so we do not paint chrome.
        let [sw, sh] = info.screen_size_px;
        if sw > 0 && sh > 0 {
            render_pass.set_viewport(0.0, 0.0, sw as f32, sh as f32, 0.0, 1.0);
        }
        if let Some(gpu) = callback_resources.get::<TimelineGpu>() {
            gpu.draw(render_pass);
        }
    }
}

/// GPU objects stored in `Renderer::callback_resources`.
pub struct TimelineGpu {
    blit_pipeline: wgpu::RenderPipeline,
    inst_pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    blit_layout: wgpu::BindGroupLayout,
    blit_uni: wgpu::Buffer,
    inst_uni: wgpu::Buffer,
    inst_bind: wgpu::BindGroup,
    instance_buf: Option<wgpu::Buffer>,
    instance_count: u32,
    /// Kept alive so the WebGPU `GPUTexture` is not dropped while bound.
    column_tex: Option<wgpu::Texture>,
    column_bind: Option<wgpu::BindGroup>,
    lod: TimelineLod,
}

impl TimelineGpu {
    pub fn init(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("orbit-live-blit-rect"),
            source: wgpu::ShaderSource::Wgsl(BLIT_RECT_WGSL.into()),
        });
        let inst_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("orbit-live-sdf"),
            source: wgpu::ShaderSource::Wgsl(INSTANCE_WGSL.into()),
        });

        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("orbit-blit-rect"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
            ],
        });
        let inst_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("orbit-inst-uni"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let blit_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("orbit-blit-pl"),
            bind_group_layouts: &[&blit_layout],
            push_constant_ranges: &[],
        });
        let inst_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("orbit-inst-pl"),
            bind_group_layouts: &[&inst_layout],
            push_constant_ranges: &[],
        });

        let target = Some(wgpu::ColorTargetState {
            format,
            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
            write_mask: wgpu::ColorWrites::ALL,
        });

        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("orbit-live-blit-rect"),
            layout: Some(&blit_pl),
            vertex: wgpu::VertexState {
                module: &blit_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &blit_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[target.clone()],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let inst_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("orbit-live-sdf"),
            layout: Some(&inst_pl),
            vertex: wgpu::VertexState {
                module: &inst_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: INSTANCE_STRIDE,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x4,
                        },
                        wgpu::VertexAttribute {
                            offset: 16,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x4,
                        },
                        wgpu::VertexAttribute {
                            offset: 32,
                            shader_location: 2,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &inst_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[target],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let blit_uni = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("orbit-blit-uni"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let inst_uni = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("orbit-inst-uni"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let inst_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("orbit-inst-bind"),
            layout: &inst_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: inst_uni.as_entire_binding(),
            }],
        });

        Self {
            blit_pipeline,
            inst_pipeline,
            sampler,
            blit_layout,
            blit_uni,
            inst_uni,
            inst_bind,
            instance_buf: None,
            instance_count: 0,
            column_tex: None,
            column_bind: None,
            lod: TimelineLod::PixelColumns,
        }
    }

    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        payload: &TimelinePayload,
        view: ViewUniforms,
    ) {
        queue.write_buffer(
            &self.blit_uni,
            0,
            &pack_blit_uniforms(view.viewport, view.dest),
        );
        queue.write_buffer(
            &self.inst_uni,
            0,
            &pack_inst_uniforms(view.viewport, view.origin),
        );

        match payload {
            TimelinePayload::Empty => {
                self.lod = TimelineLod::PixelColumns;
                self.instance_count = 0;
                self.instance_buf = None;
                self.column_tex = None;
                self.column_bind = None;
            }
            TimelinePayload::Pixel {
                rgba,
                width,
                height,
                overlay,
            } => {
                self.lod = TimelineLod::PixelColumns;
                self.upload_instances(device, overlay);
                if *width == 0 || *height == 0 || rgba.is_empty() {
                    self.column_tex = None;
                    self.column_bind = None;
                    return;
                }
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("orbit-columns"),
                    size: wgpu::Extent3d {
                        width: *width,
                        height: *height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let (padded, bytes_per_row) = pack_rgba_aligned(rgba, *width, *height);
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &padded,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(bytes_per_row),
                        rows_per_image: Some(*height),
                    },
                    wgpu::Extent3d {
                        width: *width,
                        height: *height,
                        depth_or_array_layers: 1,
                    },
                );
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                self.column_bind = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("orbit-columns-bind"),
                    layout: &self.blit_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: self.blit_uni.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                }));
                self.column_tex = Some(texture);
            }
            TimelinePayload::Instanced { instances } => {
                self.lod = TimelineLod::Instanced;
                self.column_tex = None;
                self.column_bind = None;
                self.upload_instances(device, instances);
            }
        }
    }

    fn upload_instances(&mut self, device: &wgpu::Device, instances: &[ScopeInstance]) {
        let bytes = pack_instances(instances);
        self.instance_count = instances.len() as u32;
        if bytes.is_empty() {
            self.instance_buf = None;
            return;
        }
        self.instance_buf = Some(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("orbit-instances"),
                contents: &bytes,
                usage: wgpu::BufferUsages::VERTEX,
            }),
        );
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'static>) {
        if let Some(bind) = &self.column_bind {
            pass.set_pipeline(&self.blit_pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..6, 0..1);
        }
        if let Some(buf) = &self.instance_buf {
            if self.instance_count > 0 {
                pass.set_pipeline(&self.inst_pipeline);
                pass.set_bind_group(0, &self.inst_bind, &[]);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..6, 0..self.instance_count);
            }
        }
    }
}

/// WebGPU `write_texture` requires `bytes_per_row` to be a multiple of
/// `COPY_BYTES_PER_ROW_ALIGNMENT` (256). Pack tightly stored RGBA8 into an
/// aligned staging buffer. Returns `(padded_bytes, bytes_per_row)`.
pub fn pack_rgba_aligned(src: &[u8], width: u32, height: u32) -> (Vec<u8>, u32) {
    let src_stride = width.saturating_mul(4);
    let padded = src_stride.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    if padded == src_stride {
        return (src.to_vec(), src_stride);
    }
    let mut out = vec![0u8; padded as usize * height as usize];
    for y in 0..height as usize {
        let s = y * src_stride as usize;
        let d = y * padded as usize;
        let n = src_stride as usize;
        if s + n <= src.len() {
            out[d..d + n].copy_from_slice(&src[s..s + n]);
        }
    }
    (out, padded)
}

/// Pack dest-rect blit uniforms. WGSL aligns `vec4` to 16, so `viewport` is padded.
pub fn pack_blit_uniforms(viewport: [f32; 2], dest: [f32; 4]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[0..4].copy_from_slice(&viewport[0].to_le_bytes());
    out[4..8].copy_from_slice(&viewport[1].to_le_bytes());
    out[16..20].copy_from_slice(&dest[0].to_le_bytes());
    out[20..24].copy_from_slice(&dest[1].to_le_bytes());
    out[24..28].copy_from_slice(&dest[2].to_le_bytes());
    out[28..32].copy_from_slice(&dest[3].to_le_bytes());
    out
}

pub fn pack_inst_uniforms(viewport: [f32; 2], origin: [f32; 2]) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&viewport[0].to_le_bytes());
    out[4..8].copy_from_slice(&viewport[1].to_le_bytes());
    out[8..12].copy_from_slice(&origin[0].to_le_bytes());
    out[12..16].copy_from_slice(&origin[1].to_le_bytes());
    out
}

pub fn pack_instances(instances: &[ScopeInstance]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(instances.len() * INSTANCE_STRIDE as usize);
    for i in instances {
        bytes.extend_from_slice(&i.x.to_le_bytes());
        bytes.extend_from_slice(&i.y.to_le_bytes());
        bytes.extend_from_slice(&i.w.to_le_bytes());
        bytes.extend_from_slice(&i.h.to_le_bytes());
        let color = crate::theme::display_argb(i.color);
        let r = ((color >> 16) & 0xFF) as f32 / 255.0;
        let g = ((color >> 8) & 0xFF) as f32 / 255.0;
        let b = (color & 0xFF) as f32 / 255.0;
        let a = ((i.color >> 24) & 0xFF) as f32 / 255.0;
        bytes.extend_from_slice(&r.to_le_bytes());
        bytes.extend_from_slice(&g.to_le_bytes());
        bytes.extend_from_slice(&b.to_le_bytes());
        bytes.extend_from_slice(&a.to_le_bytes());
        bytes.extend_from_slice(&i.radius.to_le_bytes());
        bytes.extend_from_slice(&i.flags.to_le_bytes());
        bytes.extend_from_slice(&0f32.to_le_bytes());
        bytes.extend_from_slice(&0f32.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_live_event::{kind, thread_scope_color, LiveEvent};
    use orbit_live_render::{choose_lod, TrackIndex, INSTANCE_MIN_PX};

    #[test]
    fn pack_instances_is_48_bytes_each() {
        let inst = ScopeInstance {
            x: 1.0,
            y: 2.0,
            w: 10.0,
            h: 16.0,
            color: 0xFFE7_4435,
            radius: 3.0,
            name_id: 1,
            start_ns: 0,
            duration_ns: 10,
            pid: 1,
            tid: 1,
            kind: 1,
            depth: 0,
            extra: 0,
            flags: 2.0,
        };
        let bytes = pack_instances(&[inst]);
        assert_eq!(bytes.len(), 48);
        assert_eq!(&bytes[36..40], &2f32.to_le_bytes());
    }

    #[test]
    fn pack_instances_flags_sit_in_extra_y() {
        let inst = ScopeInstance {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
            color: 0xFFE7_4435,
            radius: 1.0,
            name_id: 0,
            start_ns: 0,
            duration_ns: 0,
            pid: 0,
            tid: 0,
            kind: 0,
            depth: 0,
            extra: 0,
            flags: 3.0,
        };
        let bytes = pack_instances(&[inst]);
        assert_eq!(bytes.len(), 48);
    }

    #[test]
    fn blit_uniform_has_vec4_padding() {
        let u = pack_blit_uniforms([1920.0, 1080.0], [10.0, 20.0, 100.0, 50.0]);
        assert_eq!(&u[16..20], &10f32.to_le_bytes());
        assert_eq!(&u[8..16], &[0u8; 8]);
    }

    #[test]
    fn pack_rgba_pads_bytes_per_row_to_copy_alignment() {
        let width = 100u32;
        let height = 2u32;
        let mut src = vec![0u8; (width * height * 4) as usize];
        src[0..4].copy_from_slice(&[0xE7, 0x44, 0x35, 0xFF]);
        src[(width * 4) as usize..(width * 4 + 4) as usize]
            .copy_from_slice(&[0x32, 0x32, 0x32, 0xFF]);
        let (padded, stride) = pack_rgba_aligned(&src, width, height);
        let expect = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        assert_eq!(stride, expect);
        assert_eq!(stride, 512);
        assert_eq!(padded.len(), stride as usize * height as usize);
        assert_eq!(&padded[0..4], &[0xE7, 0x44, 0x35, 0xFF]);
        let row1 = stride as usize;
        assert_eq!(&padded[row1..row1 + 4], &[0x32, 0x32, 0x32, 0xFF]);
        assert_eq!(&padded[400..404], &[0, 0, 0, 0]);
    }

    #[test]
    fn pixel_payload_uses_thread_palette_not_track_gray() {
        let mut idx = TrackIndex::default();
        idx.insert(LiveEvent {
            start_ns: 0,
            duration_ns: 100,
            tid: 1,
            pid: 1,
            kind: kind::API_SCOPE,
            depth: 1,
            extra: 0,
            _pad: 0,
            name_id: 1,
        });
        let p = TimelinePayload::from_index(
            &idx,
            0,
            100,
            8.0,
            TimelineLod::PixelColumns,
            1.0,
            &[],
            &[],
        );
        let TimelinePayload::Pixel {
            rgba,
            width,
            height,
            overlay,
        } = p
        else {
            panic!("expected pixel payload");
        };
        assert!(overlay.is_empty());
        assert!(width >= 8);
        assert!(height >= 16);
        let expect = crate::theme::display_argb(thread_scope_color(1, 1));
        assert_ne!(expect, crate::theme::DISPLAY_TRACK);
        assert_eq!(rgba[0], ((expect >> 16) & 0xFF) as u8);
        assert_eq!(rgba[1], ((expect >> 8) & 0xFF) as u8);
        assert_eq!(rgba[2], (expect & 0xFF) as u8);
        assert_eq!(rgba[3], 0xFF);
        assert!(
            rgba.chunks_exact(4).any(|c| c == [0, 0, 0, 0]),
            "empty columns stay transparent so process lane washes show through"
        );
    }

    #[test]
    fn payload_uses_pixel_columns_when_scopes_are_subpixel() {
        let mut idx = TrackIndex::default();
        idx.insert(LiveEvent {
            start_ns: 0,
            duration_ns: 8,
            tid: 1,
            pid: 1,
            kind: kind::API_SCOPE,
            depth: 0,
            extra: 0,
            _pad: 0,
            name_id: 1,
        });
        let lod = choose_lod(&idx, 0, 1_000_000, 200, INSTANCE_MIN_PX);
        assert_eq!(lod, TimelineLod::PixelColumns);
        let p = TimelinePayload::from_index(&idx, 0, 1_000_000, 200.0, lod, 1.0, &[], &[]);
        assert!(matches!(p, TimelinePayload::Pixel { .. }));
    }
}
