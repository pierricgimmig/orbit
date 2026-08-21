use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wgpu::util::DeviceExt;

use orbit_live_render::{
    choose_lod, collect_instances, BLIT_WGSL, INSTANCE_MIN_PX, INSTANCE_WGSL, TimelineLod,
};

use super::LiveViewer;

const CANVAS_CLEAR: wgpu::Color = wgpu::Color {
    r: 0x43 as f64 / 255.0,
    g: 0x43 as f64 / 255.0,
    b: 0x43 as f64 / 255.0,
    a: 1.0,
};

pub struct GpuBlit {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    blit_pipeline: wgpu::RenderPipeline,
    inst_pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    blit_layout: wgpu::BindGroupLayout,
    uniform_layout: wgpu::BindGroupLayout,
    uniform_buf: wgpu::Buffer,
}

impl GpuBlit {
    pub async fn new(canvas_id: &str) -> Result<Self, String> {
        let window = web_sys::window().ok_or("no window")?;
        let document = window.document().ok_or("no document")?;
        let canvas = document
            .get_element_by_id(canvas_id)
            .ok_or("canvas not found")?
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .map_err(|_| "not a canvas")?;

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::BROWSER_WEBGPU,
            ..Default::default()
        });
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
            .map_err(|e| format!("surface: {e}"))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .ok_or("no WebGPU adapter")?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default(), None)
            .await
            .map_err(|e| format!("device: {e}"))?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats[0];
        let width = canvas.width().max(1);
        let height = canvas.height().max(1);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("orbit-live-blit"),
            source: wgpu::ShaderSource::Wgsl(BLIT_WGSL.into()),
        });
        let inst_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("orbit-live-sdf"),
            source: wgpu::ShaderSource::Wgsl(INSTANCE_WGSL.into()),
        });
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
            ],
        });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("uni"),
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
            label: None,
            bind_group_layouts: &[&blit_layout],
            immediate_size: 0,
        });
        let inst_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&uniform_layout],
            immediate_size: 0,
        });
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("orbit-live-blit"),
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
                targets: &[Some(format.into())],
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
                    array_stride: 48,
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
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
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
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uni"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Ok(Self {
            device,
            queue,
            surface,
            config,
            blit_pipeline,
            inst_pipeline,
            sampler,
            blit_layout,
            uniform_layout,
            uniform_buf,
        })
    }

    fn begin_pass<'a>(
        &'a self,
        encoder: &'a mut wgpu::CommandEncoder,
        target: &'a wgpu::TextureView,
    ) -> wgpu::RenderPass<'a> {
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(CANVAS_CLEAR),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        })
    }

    pub fn blit(&mut self, rgba: &[u8], width: u32, height: u32) -> Result<(), String> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        self.ensure_size(width, height);
        let texture = self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some("lanes"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            rgba,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.blit_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let frame = self
            .surface
            .get_current_texture()
            .map_err(|e| format!("frame: {e}"))?;
        let target = frame.texture.create_view(&Default::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = self.begin_pass(&mut encoder, &target);
            pass.set_pipeline(&self.blit_pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(())
    }

    pub fn draw_instances(
        &mut self,
        instances: &[orbit_live_render::ScopeInstance],
        viewport_w: f32,
        viewport_h: f32,
    ) -> Result<(), String> {
        self.ensure_size(viewport_w.max(1.0) as u32, viewport_h.max(1.0) as u32);
        let mut bytes = Vec::with_capacity(instances.len().max(1) * 48);
        for i in instances {
            bytes.extend_from_slice(&i.x.to_le_bytes());
            bytes.extend_from_slice(&i.y.to_le_bytes());
            bytes.extend_from_slice(&i.w.to_le_bytes());
            bytes.extend_from_slice(&i.h.to_le_bytes());
            let r = ((i.color >> 16) & 0xFF) as f32 / 255.0;
            let g = ((i.color >> 8) & 0xFF) as f32 / 255.0;
            let b = (i.color & 0xFF) as f32 / 255.0;
            let a = ((i.color >> 24) & 0xFF) as f32 / 255.0;
            bytes.extend_from_slice(&r.to_le_bytes());
            bytes.extend_from_slice(&g.to_le_bytes());
            bytes.extend_from_slice(&b.to_le_bytes());
            bytes.extend_from_slice(&a.to_le_bytes());
            bytes.extend_from_slice(&i.radius.to_le_bytes());
            bytes.extend_from_slice(&0f32.to_le_bytes());
            bytes.extend_from_slice(&0f32.to_le_bytes());
            bytes.extend_from_slice(&0f32.to_le_bytes());
        }
        if bytes.is_empty() {
            bytes.extend_from_slice(&[0u8; 48]);
        }
        let inst_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("instances"),
                contents: &bytes,
                usage: wgpu::BufferUsages::VERTEX,
            });
        let uni = [
            viewport_w.to_le_bytes(),
            viewport_h.to_le_bytes(),
            (-1f32).to_le_bytes(),
            0f32.to_le_bytes(),
        ]
        .concat();
        self.queue.write_buffer(&self.uniform_buf, 0, &uni);
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.uniform_buf.as_entire_binding(),
            }],
        });
        let frame = self
            .surface
            .get_current_texture()
            .map_err(|e| format!("frame: {e}"))?;
        let target = frame.texture.create_view(&Default::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = self.begin_pass(&mut encoder, &target);
            if !instances.is_empty() {
                pass.set_pipeline(&self.inst_pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.set_vertex_buffer(0, inst_buf.slice(..));
                pass.draw(0..6, 0..instances.len() as u32);
            }
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(())
    }

    fn ensure_size(&mut self, w: u32, h: u32) {
        if w == self.config.width && h == self.config.height {
            return;
        }
        self.config.width = w.max(1);
        self.config.height = h.max(1);
        self.surface.configure(&self.device, &self.config);
    }
}

thread_local! {
    static GPU: std::cell::RefCell<Option<GpuBlit>> = const { std::cell::RefCell::new(None) };
}

#[wasm_bindgen]
pub async fn init_webgpu(canvas_id: String) -> Result<(), JsValue> {
    let gpu = GpuBlit::new(&canvas_id)
        .await
        .map_err(|e| JsValue::from_str(&e))?;
    GPU.with(|g| *g.borrow_mut() = Some(gpu));
    Ok(())
}

#[wasm_bindgen]
pub fn render_webgpu(viewer: &LiveViewer, t0: f64, t1: f64, width: u32) -> Result<(), JsValue> {
    let width_u = width.max(1) as usize;
    let t0u = t0.max(0.0) as u64;
    let t1u = (t1 as u64).max(t0u + 1);
    GPU.with(|g| {
        let mut g = g.borrow_mut();
        let gpu = g
            .as_mut()
            .ok_or_else(|| JsValue::from_str("WebGPU not initialized"))?;
        match choose_lod(viewer.track_index(), t0u, t1u, width_u, INSTANCE_MIN_PX) {
            TimelineLod::Instanced => {
                let frame = collect_instances(viewer.track_index(), t0u, t1u, width_u as f32, 0.0);
                gpu.draw_instances(&frame.instances, width_u as f32, frame.height.max(1.0))
                    .map_err(|e| JsValue::from_str(&e))
            }
            TimelineLod::PixelColumns => {
                let raster = viewer.track_index().rasterize_pixel(t0u, t1u, width_u);
                let (rgba, h) = raster.to_rgba8_scaled();
                gpu.blit(&rgba, width.max(1), h.max(1))
                    .map_err(|e| JsValue::from_str(&e))
            }
        }
    })
}
