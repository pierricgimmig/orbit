//! WASM client for the Orbit live stream.
//!
//! Parsing of ELF/DWARF and protobuf stays on the service. This crate only
//! decodes the packed live frames and rasterizes **from the pixels**:
//! O(lanes × width × log n) via [`orbit_live_render`].

use orbit_live_event::InternTable;
use orbit_live_protocol::{decode_frame, LiveFrame};
use orbit_live_render::TrackIndex;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct LiveViewer {
    index: TrackIndex,
    intern: InternTable,
    leftover: Vec<u8>,
}

#[wasm_bindgen]
impl LiveViewer {
    #[wasm_bindgen(constructor)]
    pub fn new() -> LiveViewer {
        console_error_panic_hook::set_once();
        Self {
            index: TrackIndex::default(),
            intern: InternTable::default(),
            leftover: Vec::new(),
        }
    }

    /// Decode one or more length-prefixed live frames and insert events.
    pub fn ingest(&mut self, bytes: &[u8]) -> u32 {
        self.leftover.extend_from_slice(bytes);
        let mut consumed_frames = 0u32;
        loop {
            match decode_frame(&self.leftover) {
                Ok((frame, n)) => {
                    self.apply_frame(frame);
                    self.leftover.drain(..n);
                    consumed_frames += 1;
                }
                Err(_) => break,
            }
        }
        consumed_frames
    }

    pub fn event_count(&self) -> u32 {
        self.index.event_count() as u32
    }

    pub fn lane_count(&self) -> u32 {
        self.index.lane_count() as u32
    }

    /// `[t0, t1]` in nanoseconds, or empty if the index has no events.
    pub fn time_bounds(&self) -> Vec<f64> {
        match self.index.time_bounds() {
            Some((a, b)) => vec![a as f64, b as f64],
            None => vec![],
        }
    }

    /// Pixel-column rasterize. Returns packed RGBA8 (`lanes * width * 4`).
    pub fn rasterize(&self, t0: f64, t1: f64, width: u32) -> Vec<u8> {
        let width = width.max(1) as usize;
        let t0 = t0.max(0.0) as u64;
        let t1 = (t1 as u64).max(t0 + 1);
        self.index.rasterize_pixel(t0, t1, width).to_rgba8()
    }

    pub fn reset(&mut self) {
        self.index.clear();
        self.intern = InternTable::default();
        self.leftover.clear();
    }
}

impl LiveViewer {
    fn apply_frame(&mut self, frame: LiveFrame) {
        match frame {
            LiveFrame::EventBatch { events } => {
                for ev in events {
                    self.index.insert(ev);
                }
            }
            LiveFrame::InternedString { id, text } => {
                self.intern.insert_id(id, &text);
            }
            LiveFrame::CaptureStarted { .. } => {
                self.index.clear();
            }
            LiveFrame::CaptureFinished | LiveFrame::Hello { .. } | LiveFrame::Status { .. } => {}
        }
    }
}

#[cfg(feature = "webgpu")]
mod gpu {
    use super::*;
    use wasm_bindgen::JsCast;
    use wgpu::util::DeviceExt;

    const SHADER: &str = r#"
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

    pub struct GpuBlit {
        device: wgpu::Device,
        queue: wgpu::Queue,
        surface: wgpu::Surface<'static>,
        config: wgpu::SurfaceConfiguration,
        pipeline: wgpu::RenderPipeline,
        sampler: wgpu::Sampler,
        bind_layout: wgpu::BindGroupLayout,
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

            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("orbit-live"),
                source: wgpu::ShaderSource::Wgsl(SHADER.into()),
            });
            let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[&bind_layout],
                immediate_size: 0,
            });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("orbit-live-blit"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
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
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Nearest,
                min_filter: wgpu::FilterMode::Nearest,
                ..Default::default()
            });
            Ok(Self {
                device,
                queue,
                surface,
                config,
                pipeline,
                sampler,
                bind_layout,
            })
        }

        pub fn blit(&mut self, rgba: &[u8], width: u32, height: u32) -> Result<(), String> {
            if width == 0 || height == 0 {
                return Ok(());
            }
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
                layout: &self.bind_layout,
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
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.draw(0..3, 0..1);
            }
            self.queue.submit(Some(encoder.finish()));
            frame.present();
            let _ = self.config.width;
            Ok(())
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
        let rgba = viewer.rasterize(t0, t1, width);
        let lanes = viewer.lane_count().max(1);
        GPU.with(|g| {
            match g.borrow_mut().as_mut() {
                Some(gpu) => gpu
                    .blit(&rgba, width.max(1), lanes)
                    .map_err(|e| JsValue::from_str(&e)),
                None => Err(JsValue::from_str("WebGPU not initialized")),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_live_event::{kind, LiveEvent, LIVE_EVENT_SIZE};
    use orbit_live_protocol::{encode_frame, LiveFrame, VERSION};

    #[test]
    fn ingest_batch_and_rasterize() {
        let mut v = LiveViewer::new();
        let ev = LiveEvent {
            start_ns: 0,
            duration_ns: 100,
            tid: 1,
            pid: 1,
            kind: kind::API_SCOPE,
            depth: 0,
            extra: 0,
            _pad: 0,
            name_id: 1,
        };
        let bytes = encode_frame(&LiveFrame::EventBatch { events: vec![ev] });
        assert_eq!(v.ingest(&bytes), 1);
        assert_eq!(v.event_count(), 1);
        let pix = v.rasterize(0.0, 100.0, 8);
        assert_eq!(pix.len(), 8 * 4);
        assert!(pix.iter().any(|&b| b != 0));
    }

    #[test]
    fn hello_then_partial_frame_is_buffered() {
        let mut v = LiveViewer::new();
        let hello = encode_frame(&LiveFrame::Hello {
            version: VERSION,
            event_size: LIVE_EVENT_SIZE as u16,
        });
        assert_eq!(v.ingest(&hello[..3]), 0);
        assert_eq!(v.ingest(&hello[3..]), 1);
    }
}
