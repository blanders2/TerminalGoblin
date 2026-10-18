//! wgpu setup and the instanced quad renderer.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use winit::window::Window;

use crate::theme::Color;

pub const ATLAS_SIZE: u32 = 2048;
const NO_CLIP: [f32; 4] = [-1.0e6, -1.0e6, 2.0e6, 2.0e6];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn inset(&self, d: f32) -> Rect {
        Rect::new(
            self.x + d,
            self.y + d,
            (self.w - 2.0 * d).max(0.0),
            (self.h - 2.0 * d).max(0.0),
        )
    }
    pub fn center_y(&self) -> f32 {
        self.y + self.h / 2.0
    }
    fn arr(&self) -> [f32; 4] {
        [self.x, self.y, self.w, self.h]
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Instance {
    rect: [f32; 4],
    uv: [f32; 4],
    color: [f32; 4],
    radii: [f32; 4],
    clip: [f32; 4],
    border_color: [f32; 4],
    params: [f32; 4],
}

impl Instance {
    pub fn rect(r: Rect, color: Color) -> Instance {
        Instance {
            rect: r.arr(),
            uv: [0.0; 4],
            color,
            radii: [0.0; 4],
            clip: NO_CLIP,
            border_color: [0.0; 4],
            params: [0.0; 4],
        }
    }
    pub fn rounded(r: Rect, color: Color, radius: f32) -> Instance {
        Instance {
            radii: [radius; 4],
            ..Instance::rect(r, color)
        }
    }
    /// Per-corner radii: top-left, top-right, bottom-right, bottom-left.
    pub fn rounded_corners(r: Rect, color: Color, radii: [f32; 4]) -> Instance {
        Instance {
            radii,
            ..Instance::rect(r, color)
        }
    }
    pub fn border(mut self, width: f32, color: Color) -> Instance {
        self.params[1] = width;
        self.border_color = color;
        self
    }
    pub fn shadow(r: Rect, color: Color, radius: f32, blur: f32) -> Instance {
        Instance {
            radii: [radius; 4],
            params: [3.0, 0.0, blur, 0.0],
            ..Instance::rect(r, color)
        }
    }
    /// Fills `r` except inside the circle at `center`: the outside of a rounded corner.
    pub fn corner_filler(r: Rect, color: Color, center: (f32, f32), radius: f32) -> Instance {
        Instance {
            radii: [center.0, center.1, radius, 0.0],
            params: [4.0, 0.0, 0.0, 0.0],
            ..Instance::rect(r, color)
        }
    }
    pub fn glyph(r: Rect, uv: [f32; 4], color: Color, is_color: bool) -> Instance {
        Instance {
            uv,
            params: [if is_color { 2.0 } else { 1.0 }, 0.0, 0.0, 0.0],
            ..Instance::rect(r, color)
        }
    }
    /// Narrow the existing clip to its intersection with `r`.
    pub fn clip_within(self, r: Rect) -> Instance {
        let [x, y, w, h] = self.clip;
        let cur = Rect::new(x, y, w, h);
        let nx = cur.x.max(r.x);
        let ny = cur.y.max(r.y);
        let nr = cur.right().min(r.right());
        let nb = cur.bottom().min(r.bottom());
        self.clip(Rect::new(nx, ny, (nr - nx).max(0.0), (nb - ny).max(0.0)))
    }
    pub fn clip(mut self, clip: Rect) -> Instance {
        self.clip = clip.arr();
        self
    }
}

/// Pending atlas write produced by the glyph cache.
pub struct Upload {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    globals: wgpu::Buffer,
    atlas: wgpu::Texture,
    instances: wgpu::Buffer,
    instance_cap: usize,
    /// Whether the surface can show what is behind the window.
    pub supports_transparency: bool,
}

impl Renderer {
    pub fn new(window: Arc<Window>) -> anyhow::Result<Renderer> {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance.create_surface(window)?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            ..Default::default()
        }))?;
        log::info!("GPU adapter: {:?}", adapter.get_info().name);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("terminalgoblin"),
            required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        }))?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| matches!(f, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm))
            .unwrap_or(caps.formats[0]);
        let (alpha_mode, supports_transparency) = if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            (wgpu::CompositeAlphaMode::PreMultiplied, true)
        } else if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Inherit) {
            (wgpu::CompositeAlphaMode::Inherit, true)
        } else {
            log::warn!(
                "surface does not support transparency; alpha modes: {:?}",
                caps.alpha_modes
            );
            (caps.alpha_modes[0], false)
        };
        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else {
            wgpu::PresentMode::Fifo
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
            color_space: Default::default(),
        };
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let atlas_view = atlas.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
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
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bg"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        const ATTRS: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4,
            4 => Float32x4, 5 => Float32x4, 6 => Float32x4
        ];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quads"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &ATTRS,
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let instance_cap = 16 * 1024;
        let instances = Self::make_instance_buffer(&device, instance_cap);

        Ok(Renderer {
            surface,
            device,
            queue,
            config,
            pipeline,
            bind_group,
            globals,
            atlas,
            instances,
            instance_cap,
            supports_transparency,
        })
    }

    fn make_instance_buffer(device: &wgpu::Device, cap: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: (cap * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn upload(&self, uploads: &[Upload]) {
        for u in uploads {
            if u.w == 0 || u.h == 0 {
                continue;
            }
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.atlas,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: u.x, y: u.y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                &u.rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(u.w * 4),
                    rows_per_image: Some(u.h),
                },
                wgpu::Extent3d {
                    width: u.w,
                    height: u.h,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    pub fn render(&mut self, window: &Window, instances: &[Instance]) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                window.request_redraw();
                return;
            }
            other => {
                log::debug!("skipping frame: {other:?}");
                return;
            }
        };

        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let encoder = self.encode(&view, instances);
        self.queue.submit([encoder.finish()]);
        window.pre_present_notify();
        self.queue.present(frame);
    }

    fn encode(&mut self, view: &wgpu::TextureView, instances: &[Instance]) -> wgpu::CommandEncoder {
        if instances.len() > self.instance_cap {
            self.instance_cap = instances.len().next_power_of_two();
            self.instances = Self::make_instance_buffer(&self.device, self.instance_cap);
        }
        let globals = [
            self.config.width as f32,
            self.config.height as f32,
            ATLAS_SIZE as f32,
            ATLAS_SIZE as f32,
        ];
        self.queue
            .write_buffer(&self.globals, 0, bytemuck::cast_slice(&globals));
        self.queue
            .write_buffer(&self.instances, 0, bytemuck::cast_slice(instances));

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, self.instances.slice(..));
            pass.draw(0..4, 0..instances.len() as u32);
        }
        encoder
    }

    /// Render a frame offscreen and save it as a PNG, composited over a backdrop so
    /// transparency is visible. Used by `--screenshot`.
    pub fn screenshot(&mut self, instances: &[Instance], path: &std::path::Path) -> anyhow::Result<()> {
        let (w, h) = (self.config.width, self.config.height);
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("screenshot"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.encode(&view, instances);
        let row = (w * 4).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        let data = slice.get_mapped_range().map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let bgra = matches!(
            self.config.format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                let i = (y * row + x * 4) as usize;
                let (r, g, b, a) = if bgra {
                    (data[i + 2], data[i + 1], data[i], data[i + 3])
                } else {
                    (data[i], data[i + 1], data[i + 2], data[i + 3])
                };
                // A colourful backdrop standing in for the desktop behind the window.
                let fx = x as f32 / w as f32;
                let fy = y as f32 / h as f32;
                let back = [0.85 * fx + 0.1, 0.35 + 0.3 * fy, 0.9 - 0.6 * fx];
                let inv = 1.0 - a as f32 / 255.0;
                rgb.push((r as f32 + back[0] * 255.0 * inv).min(255.0) as u8);
                rgb.push((g as f32 + back[1] * 255.0 * inv).min(255.0) as u8);
                rgb.push((b as f32 + back[2] * 255.0 * inv).min(255.0) as u8);
            }
        }
        drop(data);
        let file = std::io::BufWriter::new(std::fs::File::create(path)?);
        let mut enc = png::Encoder::new(file, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgb)?;
        Ok(())
    }
}
