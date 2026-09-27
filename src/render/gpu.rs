//! Running shader effects on the GPU with wgpu (Vulkan, Metal, DX12 or GL).
//! Without a graphics card, a software driver such as Mesa's lavapipe works too.

use crate::shader::{Program, Uniforms};
use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use wgpu::util::DeviceExt;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    sampler: wgpu::Sampler,
    pipelines: HashMap<u64, wgpu::RenderPipeline>,
    targets: Option<Targets>,
    errors: Arc<Mutex<Option<String>>>,
    pub adapter: String,
}

/// Textures and the readback buffer, reused while the image size stays the same.
/// Two textures take turns being read and written, so a chain of shaders
/// runs entirely on the GPU with one upload and one readback.
struct Targets {
    w: u32,
    h: u32,
    ping: [wgpu::Texture; 2],
    readback: wgpu::Buffer,
    padded_row: u32,
}

/// One shader pass in a chain.
pub struct Pass<'a> {
    pub program: &'a Program,
    pub params: &'a [u8],
    pub uniforms: Uniforms,
}

impl Gpu {
    pub fn new() -> Result<Gpu> {
        // WGPU_BACKEND=vulkan|gl|metal|dx12 picks a backend explicitly
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .or_else(|_| {
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                force_fallback_adapter: true,
                ..Default::default()
            }))
        })
        .map_err(|_| {
            anyhow!(
                "shader effects need a GPU driver, and none was found \
                 (Vulkan, Metal, DX12 or OpenGL; without a graphics card, a software driver \
                 like Mesa's lavapipe works — e.g. the mesa-vulkan-drivers package)"
            )
        })?;
        let info = adapter.get_info();
        let adapter_name = format!("{} ({:?})", info.name, info.backend);

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("clve"),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .context("can't open the GPU")?;

        // report GPU errors as normal errors instead of panicking
        let errors: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let sink = errors.clone();
        device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
            let mut slot = sink.lock().unwrap();
            if slot.is_none() {
                *slot = Some(e.to_string());
            }
        }));

        let uniform = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("clve effect"),
            entries: &[
                uniform(0),
                uniform(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("clve effect"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("clve"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Ok(Gpu {
            device,
            queue,
            layout,
            pipeline_layout,
            sampler,
            pipelines: HashMap::new(),
            targets: None,
            errors,
            adapter: adapter_name,
        })
    }

    fn take_error(&self) -> Result<()> {
        match self.errors.lock().unwrap().take() {
            Some(e) => bail!("GPU error: {e}"),
            None => Ok(()),
        }
    }

    fn pipeline(&mut self, prog: &Program) -> Result<()> {
        if self.pipelines.contains_key(&prog.hash) {
            return Ok(());
        }
        let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(&prog.name),
            source: wgpu::ShaderSource::Wgsl(prog.wgsl.as_str().into()),
        });
        let pipeline = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(&prog.name),
            layout: Some(&self.pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("clve_vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("clve_fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        self.take_error()
            .with_context(|| format!("effect '{}': the GPU driver rejected the shader", prog.name))?;
        self.pipelines.insert(prog.hash, pipeline);
        Ok(())
    }

    fn ensure_targets(&mut self, w: u32, h: u32) {
        let fits = self.targets.as_ref().is_some_and(|t| t.w == w && t.h == h);
        if !fits {
            let usage = wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC;
            let tex = |label| {
                self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: FORMAT,
                    usage,
                    view_formats: &[],
                })
            };
            let padded_row = (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
            self.targets = Some(Targets {
                w,
                h,
                ping: [tex("clve ping"), tex("clve pong")],
                readback: self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("clve readback"),
                    size: (padded_row * h) as u64,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                padded_row,
            });
        }
    }

    /// Runs a chain of shaders over a premultiplied RGBA8 image of size w x h, in place.
    pub fn run(&mut self, passes: &[Pass], rgba: &mut [u8], w: u32, h: u32) -> Result<()> {
        let Some(first) = passes.first() else { return Ok(()) };
        let max = self.device.limits().max_texture_dimension_2d;
        if w > max || h > max {
            bail!("effect '{}': image {w}x{h} is larger than the GPU allows ({max})", first.program.name);
        }
        for pass in passes {
            self.pipeline(pass.program)?;
        }
        self.ensure_targets(w, h);
        let t = self.targets.as_ref().unwrap();
        let device = &self.device;
        let extent = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
        let copy = |tex| wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        };

        self.queue.write_texture(
            copy(&t.ping[0]),
            rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            extent,
        );

        let views = [t.ping[0].create_view(&Default::default()), t.ping[1].create_view(&Default::default())];
        let mut enc = device.create_command_encoder(&Default::default());
        let mut cur = 0;
        for pass in passes {
            let ubuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("clve u"),
                contents: &pass.uniforms.bytes(),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let pbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("clve p"),
                contents: pass.params,
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("clve effect"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: pbuf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&views[cur]) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            });
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(&pass.program.name),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &views[1 - cur],
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_pipeline(&self.pipelines[&pass.program.hash]);
            rp.set_bind_group(0, &bind, &[]);
            rp.draw(0..3, 0..1);
            drop(rp);
            cur = 1 - cur;
        }
        enc.copy_texture_to_buffer(
            copy(&t.ping[cur]),
            wgpu::TexelCopyBufferInfo {
                buffer: &t.readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(t.padded_row), rows_per_image: Some(h) },
            },
            extent,
        );
        self.queue.submit([enc.finish()]);

        let slice = t.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| anyhow!("GPU stopped responding: {e}"))?;
        rx.recv()
            .context("GPU readback was dropped")?
            .map_err(|e| anyhow!("can't read the result from the GPU: {e}"))?;
        {
            let data = slice
                .get_mapped_range()
                .map_err(|e| anyhow!("can't read the result from the GPU: {e:?}"))?;
            let row = (w * 4) as usize;
            for (y, dst) in rgba.chunks_exact_mut(row).enumerate() {
                let start = y * t.padded_row as usize;
                dst.copy_from_slice(&data[start..start + row]);
            }
        }
        t.readback.unmap();
        let names: Vec<&str> = passes.iter().map(|p| p.program.name.as_str()).collect();
        self.take_error().with_context(|| format!("effect '{}'", names.join("', '")))
    }
}
