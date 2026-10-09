//! GPU renderer: cell background quads -> glyphon text -> effect quads.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use glyphon::cosmic_text::{Align, Attrs, AttrsOwned, Family, Metrics, Shaping, Weight, Wrap};
use glyphon::{
    Buffer, Cache, Color as GColor, FontSystem, Resolution, SwashCache, TextArea, TextAtlas,
    TextBounds, TextRenderer, Viewport,
};
use unicode_width::UnicodeWidthStr;
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::fx::Instance;

const QUAD_SHADER: &str = r#"
struct Uniforms { screen: vec2f, _pad: vec2f };
@group(0) @binding(0) var<uniform> u: Uniforms;

struct Vin {
    @location(0) quad: vec2f,
    @location(1) pos: vec2f,
    @location(2) size: vec2f,
    @location(3) rot: f32,
    @location(4) kind: u32,
    @location(5) color: vec4f,
};
struct Vout {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f,
    @location(1) color: vec4f,
    @interpolate(flat) @location(2) kind: u32,
};

const PI: f32 = 3.14159265358979323846;

@vertex
fn vs(v: Vin) -> Vout {
    let c = cos(v.rot);
    let s = sin(v.rot);
    let p = v.quad * v.size;
    let pr = vec2f(p.x * c - p.y * s, p.x * s + p.y * c) + v.pos;
    let ndc = vec2f(pr.x / u.screen.x * 2.0 - 1.0, 1.0 - pr.y / u.screen.y * 2.0);
    var o: Vout;
    o.pos = vec4f(ndc, 0.0, 1.0);
    o.uv = v.quad;
    o.color = v.color;
    o.kind = v.kind;
    return o;
}

@fragment
fn fs(i: Vout) -> @location(0) vec4f {
    var a = i.color.a;
    let k = i.kind;
    switch k {
        case 0u: {
            // rect: crisp quad, alpha unchanged
        }
        case 1u, 6u: {
            // glow / soft disc
            let d = length(i.uv) * 2.0;
            a *= max(0.0, 1.0 - d * d);
        }
        case 2u: {
            // ring
            let d = length(i.uv);
            a *= 1.0 - smoothstep(0.35, 0.55, abs(d - 0.55));
        }
        case 3u: {
            // diamond
            let d = abs(i.uv.x) + abs(i.uv.y);
            a *= 1.0 - smoothstep(0.75, 1.05, d);
        }
        case 4u: {
            // 5-point star
            let r = length(i.uv);
            if (r > 1.05) {
                a = 0.0;
            } else {
                var ang = atan2(i.uv.y, i.uv.x);
                let fold = 5.0;
                let sector = 2.0 * PI / fold;
                ang = abs(fract(ang / sector + 0.5) - 0.5) * sector;
                let tip = cos(ang) / cos(sector * 0.5);
                a *= 1.0 - smoothstep(tip * 0.82, tip, r);
            }
        }
        case 5u: {
            // lightning bolt zigzag
            let u = i.uv.x;
            let v = i.uv.y;
            let zig = 0.5 * sign(u) * (abs(u) - 1.0);
            let dist = abs(v - zig);
            a *= 1.0 - smoothstep(0.08, 0.20, dist);
        }
        default: {}
    }
    return vec4f(i.color.rgb, a);
}
"#;

/// One styled span inside a terminal line.
#[derive(Clone)]
pub struct Span {
    pub text: String,
    pub fg: [u8; 4],
    pub bold: bool,
}

/// A display line's styled text plus its y offset.
#[derive(Clone)]
pub struct Line {
    pub top: f32,
    pub left: f32,
    pub spans: Vec<Span>,
    /// Per-line family override; used by the font picker to render each
    /// family name in its own typeface.
    pub family: Option<String>,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    /// Set by the device-lost / uncaptured-error callbacks; a frame that
    /// observes it treats the device as unrecoverable and bails so the app
    /// rebuilds the renderer instead of presenting to a dead device.
    gpu_fault: Arc<AtomicBool>,
    quad_pipeline: wgpu::RenderPipeline,
    quad_vb: wgpu::Buffer,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    bg_instances: wgpu::Buffer,
    fx_instances: wgpu::Buffer,
    inst_capacity: usize,
    font_system: FontSystem,
    swash: SwashCache,
    /// Per-screen-line text buffers; rebuilt only when the line changed.
    line_bufs: Vec<Option<Buffer>>,
    atlas: TextAtlas,
    viewport: Viewport,
    text_renderer: TextRenderer,
    overlay_text_renderer: TextRenderer,
    pub cell_w: f32,
    pub cell_h: f32,
    /// Configured size in points; multiplied by `scale_factor` to get
    /// physical pixels on HiDPI displays.
    font_size: f32,
    scale_factor: f32,
    font_family: Option<String>,
    font_families: Vec<String>,
}

const MAX_INSTANCES: usize = 1 << 16;

fn grid_text_buffer(font_system: &mut FontSystem, metrics: Metrics, width: f32) -> Buffer {
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_size(Some(width), Some(metrics.line_height));
    buffer.set_wrap(Wrap::None);
    buffer
}

fn align_to_cell_grid(buffer: &mut Buffer, font_system: &mut FontSystem, cell_w: f32) {
    buffer.shape_until_scroll(font_system, false);
    let mut changed = false;
    for line in &mut buffer.lines {
        let adjustments: Vec<_> = line
            .layout_opt()
            .into_iter()
            .flatten()
            .flat_map(|layout| &layout.glyphs)
            .filter_map(|glyph| {
                let width = line.text()[glyph.start..glyph.end].width() as f32 * cell_w;
                let spacing = (width - glyph.w) / glyph.font_size;
                if spacing.abs() < 0.0001 {
                    return None;
                }
                let attrs = line
                    .attrs_list()
                    .get_span(glyph.start)
                    .letter_spacing(spacing);
                Some((glyph.start..glyph.end, AttrsOwned::new(&attrs)))
            })
            .collect();
        if adjustments.is_empty() {
            continue;
        }
        let mut attrs = line.attrs_list().clone();
        for (range, adjustment) in adjustments {
            attrs.add_span(range, &adjustment.as_attrs());
        }
        changed |= line.set_attrs_list(attrs);
    }
    if changed {
        buffer.shape_until_scroll(font_system, false);
    }
}

/// What happened to a frame: presented to the window, or skipped without
/// error (the app escalates long skip streaks to a window rebuild).
#[derive(Debug)]
pub enum FrameOutcome {
    Presented,
    Skipped(&'static str),
}

impl Renderer {
    pub async fn new(
        window: Arc<Window>,
        font_size: f32,
        font_family: Option<String>,
    ) -> anyhow::Result<Self> {
        let size = window.inner_size();
        let scale_factor = window.scale_factor() as f32;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance.create_surface(window)?;
        // An adapter can be reported lost mid-enumeration when the GPU
        // configuration is changing (driver reset, hybrid-GPU switch), so
        // retry before giving up.
        let (adapter, device, queue) = {
            let mut last_err = None;
            let mut result = None;
            for attempt in 0..3 {
                if attempt > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                let adapter = match instance
                    .request_adapter(&wgpu::RequestAdapterOptions {
                        power_preference: wgpu::PowerPreference::HighPerformance,
                        compatible_surface: Some(&surface),
                        ..Default::default()
                    })
                    .await
                {
                    Ok(a) => a,
                    Err(e) => {
                        last_err = Some(e.into());
                        continue;
                    }
                };
                match adapter
                    .request_device(&wgpu::DeviceDescriptor::default())
                    .await
                {
                    Ok((d, q)) => {
                        result = Some((adapter, d, q));
                        break;
                    }
                    Err(e) => last_err = Some(e.into()),
                }
            }
            match result {
                Some(r) => r,
                None => return Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no GPU adapter"))),
            }
        };
        let gpu_fault = Arc::new(AtomicBool::new(false));
        {
            let f = gpu_fault.clone();
            device.set_device_lost_callback(move |reason, msg| {
                f.store(true, Ordering::Relaxed);
                eprintln!("dopaterm: GPU device lost ({reason:?}): {msg}");
            });
        }
        {
            let f = gpu_fault.clone();
            // Overrides the default handler, which panics on validation
            // errors: a panic inside the redraw callback would kill the
            // winit event loop and freeze the window.
            device.on_uncaptured_error(Arc::new(move |e| {
                // Validation errors are code bugs, not device faults —
                // log them without triggering a rebuild.
                if !matches!(e, wgpu::Error::Validation { .. }) {
                    f.store(true, Ordering::Relaxed);
                }
                eprintln!("dopaterm: wgpu error: {e}");
            }));
        }
        let info = adapter.get_info();
        eprintln!("dopaterm: GPU backend {:?} on {}", info.backend, info.name);

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.width.max(1),
            height: size.height.max(1),
            // AutoVsync prefers a non-blocking (mailbox) swapchain: Fifo can
            // block inside get_current_texture when the GPU/surface is
            // reconfigured underneath us (adapter switch, TDR, topology
            // change), which froze the whole event loop.
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
        };
        surface.configure(&device, &surface_config);

        // Instanced quad pipeline (shared by cell backgrounds and particles).
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quads"),
            source: wgpu::ShaderSource::Wgsl(QUAD_SHADER.into()),
        });
        let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("uniforms"),
            contents: bytemuck::cast_slice(&[size.width as f32, size.height as f32, 0.0, 0.0]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("quad uniforms"),
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
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("quad uniforms"),
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("quads"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let quad: [[f32; 2]; 4] = [[-0.5, -0.5], [0.5, -0.5], [-0.5, 0.5], [0.5, 0.5]];
        let quad_vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("quad vb"),
            contents: bytemuck::cast_slice(&quad),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let inst_attrs = wgpu::vertex_attr_array![
            1 => Float32x2,
            2 => Float32x2,
            3 => Float32,
            4 => Uint32,
            5 => Float32x4
        ];
        let inst_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Instance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &inst_attrs,
        };
        let quad_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quads"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 8,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2],
                    }),
                    Some(inst_layout),
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let mk_inst = |label: &str| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: (MAX_INSTANCES * std::mem::size_of::<Instance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };

        let font_system = FontSystem::new();
        let font_families = crate::fonts::monospace_families(font_system.db());
        let font_family = crate::fonts::resolve_family(font_family.as_deref(), &font_families)
            .or_else(|| crate::config::preferred_monospace_font(&font_families));
        let cache = Cache::new(&device);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let viewport = Viewport::new(&device, &cache);
        let swash = SwashCache::new();
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);
        let overlay_text_renderer =
            TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);

        let bg_instances = mk_inst("bg instances");
        let fx_instances = mk_inst("fx instances");
        let mut r = Self {
            device,
            queue,
            surface,
            surface_config,
            gpu_fault,
            quad_pipeline,
            quad_vb,
            uniform_buf,
            bind_group,
            bg_instances,
            fx_instances,
            inst_capacity: MAX_INSTANCES,
            font_system,
            swash,
            line_bufs: Vec::new(),
            atlas,
            viewport,
            text_renderer,
            overlay_text_renderer,
            cell_w: 8.0,
            cell_h: 16.0,
            font_size,
            scale_factor,
            font_family,
            font_families,
        };
        r.measure_cell();
        Ok(r)
    }

    /// Text metrics in physical pixels: configured points x DPI scale.
    fn text_metrics(&self) -> Metrics {
        let px = self.font_size * self.scale_factor;
        Metrics::new(px, px * 1.25)
    }

    fn measure_cell(&mut self) {
        let metrics = self.text_metrics();
        let fname = self.font_family.clone();
        let family = match fname.as_deref() {
            Some(n) => Family::Name(n),
            None => Family::Monospace,
        };
        let mut buf = Buffer::new(&mut self.font_system, metrics);
        buf.set_size(Some(1000.0), Some(metrics.line_height));
        buf.set_text(
            "MMMMMMMMMM",
            &Attrs::new().family(family),
            Shaping::Basic,
            None,
        );
        buf.shape_until_scroll(&mut self.font_system, false);
        let w = buf
            .layout_runs()
            .next()
            .map(|run| run.glyphs.iter().map(|g| g.w).sum::<f32>() / 10.0)
            .unwrap_or(self.font_size * 0.6);
        self.cell_w = w.round().max(1.0);
        self.cell_h = metrics.line_height.round().max(1.0);
        eprintln!(
            "dopaterm: font family={:?} cell_w={:.2} cell_h={:.2}",
            self.font_family, self.cell_w, self.cell_h
        );
    }

    pub fn font_families(&self) -> &[String] {
        &self.font_families
    }

    pub fn font_family(&self) -> Option<&str> {
        self.font_family.as_deref()
    }

    pub fn set_font(&mut self, size: f32, family: Option<&str>) {
        self.font_size = size.max(4.0);
        self.font_family = crate::fonts::resolve_family(family, &self.font_families);
        self.line_bufs.clear();
        self.measure_cell();
    }

    /// Update the DPI scale (window moved between monitors); re-measures
    /// the cell grid so text stays the same logical size.
    pub fn set_scale_factor(&mut self, scale_factor: f64) {
        let sf = (scale_factor as f32).max(0.1);
        if (sf - self.scale_factor).abs() < 0.01 {
            return;
        }
        self.scale_factor = sf;
        self.line_bufs.clear();
        self.measure_cell();
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        self.queue.write_buffer(
            &self.uniform_buf,
            0,
            bytemuck::cast_slice(&[width as f32, height as f32, 0.0, 0.0]),
        );
    }

    /// Draw one frame to the window surface. `lines` are styled text rows;
    /// `bg`/`fx` are quad instances drawn below/above text respectively.
    /// `dirty` marks which rows need their text buffer rebuilt.
    /// `overlay_lines`/`overlay_bg` render a settings UI on top of the
    /// terminal.
    pub fn render(
        &mut self,
        clear: [f32; 4],
        bg: &[Instance],
        lines: &[Option<Line>],
        dirty: &[bool],
        fx: &[Instance],
        overlay_bg: &[Instance],
        overlay_lines: &[Line],
    ) -> anyhow::Result<FrameOutcome> {
        use wgpu::CurrentSurfaceTexture as Cst;
        if self.gpu_fault.load(Ordering::Relaxed) {
            anyhow::bail!("GPU device faulted");
        }
        let frame = match self.surface.get_current_texture() {
            Cst::Success(t) | Cst::Suboptimal(t) => t,
            Cst::Lost | Cst::Outdated => {
                self.surface.configure(&self.device, &self.surface_config);
                match self.surface.get_current_texture() {
                    Cst::Success(t) | Cst::Suboptimal(t) => t,
                    _ => anyhow::bail!("surface lost"),
                }
            }
            Cst::Timeout => return Ok(FrameOutcome::Skipped("timeout")),
            Cst::Occluded => return Ok(FrameOutcome::Skipped("occluded")),
            Cst::Validation => return Ok(FrameOutcome::Skipped("validation")),
        };
        let view = frame.texture.create_view(&Default::default());
        let (w, h) = (self.surface_config.width, self.surface_config.height);
        self.draw(
            &view,
            w,
            h,
            clear,
            bg,
            lines,
            dirty,
            fx,
            overlay_bg,
            overlay_lines,
        )?;
        self.queue.present(frame);
        // A faulted device never recovers: keep the flag set so every
        // subsequent frame bails too (a stale surface may keep reporting
        // Success while silently dropping presents).
        if self.gpu_fault.load(Ordering::Relaxed) {
            anyhow::bail!("GPU device faulted during present");
        }
        Ok(FrameOutcome::Presented)
    }

    /// Render the same frame into an offscreen texture and read back RGBA8.
    pub fn screenshot(
        &mut self,
        width: u32,
        height: u32,
        clear: [f32; 4],
        bg: &[Instance],
        lines: &[Option<Line>],
        fx: &[Instance],
        overlay_bg: &[Instance],
        overlay_lines: &[Line],
    ) -> anyhow::Result<Vec<u8>> {
        let format = self.surface_config.format;
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("screenshot"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        // Text uses the viewport uniform: temporarily point it at the
        // offscreen resolution.
        // Screenshot forces a full relayout of every line.
        let all_dirty = vec![true; lines.len()];
        self.draw(
            &tex.create_view(&Default::default()),
            width,
            height,
            clear,
            bg,
            lines,
            &all_dirty,
            fx,
            overlay_bg,
            overlay_lines,
        )?;

        let bpp = 4u32;
        let padded = (width * bpp).div_ceil(256) * 256;
        let out = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screenshot out"),
            size: (padded * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("shot"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &out,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let slice = out.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        rx.recv()??;
        let data = slice.get_mapped_range()?.to_vec();
        out.unmap();

        // Strip row padding; convert BGRA -> RGBA when needed.
        let bgra = matches!(
            format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        let mut rgba = vec![0u8; (width * height * bpp) as usize];
        for y in 0..height as usize {
            let src = &data[y * padded as usize..y * padded as usize + (width * bpp) as usize];
            let dst = &mut rgba[y * (width * bpp) as usize..(y + 1) * (width * bpp) as usize];
            dst.copy_from_slice(src);
        }
        if bgra {
            for px in rgba.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
        }
        Ok(rgba)
    }

    /// Shared draw path: upload instances, shape text, encode one pass, submit.
    fn draw(
        &mut self,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        clear: [f32; 4],
        bg: &[Instance],
        lines: &[Option<Line>],
        dirty: &[bool],
        fx: &[Instance],
        overlay_bg: &[Instance],
        overlay_lines: &[Line],
    ) -> anyhow::Result<()> {
        self.queue.write_buffer(
            &self.uniform_buf,
            0,
            bytemuck::cast_slice(&[width as f32, height as f32, 0.0, 0.0]),
        );
        let metrics = self.text_metrics();
        let fname = self.font_family.clone();
        let family = match fname.as_deref() {
            Some(n) => Family::Name(n),
            None => Family::Monospace,
        };
        let default_attrs = Attrs::new().family(family);

        // Rebuild only dirty line buffers; keep the rest cached.
        if self.line_bufs.len() != lines.len() {
            self.line_bufs = vec![None; lines.len()];
        }
        for (i, line) in lines.iter().enumerate() {
            let is_dirty = dirty.get(i).copied().unwrap_or(true);
            match line {
                Some(line) if is_dirty => {
                    let mut buf = grid_text_buffer(&mut self.font_system, metrics, width as f32);
                    let spans: Vec<(&str, Attrs)> = line
                        .spans
                        .iter()
                        .map(|s| {
                            let mut a = Attrs::new()
                                .family(family)
                                .color(GColor::rgba(s.fg[0], s.fg[1], s.fg[2], s.fg[3]));
                            if s.bold {
                                a = a.weight(Weight::BOLD);
                            }
                            (s.text.as_str(), a)
                        })
                        .collect();
                    // Advanced shaping is required for font fallback:
                    // Basic never leaves the requested family, so glyphs
                    // missing from it (emoji, CJK, symbols) render blank.
                    buf.set_rich_text(spans, &default_attrs, Shaping::Advanced, Some(Align::Left));
                    align_to_cell_grid(&mut buf, &mut self.font_system, self.cell_w);
                    self.line_bufs[i] = Some(buf);
                }
                None if is_dirty => self.line_bufs[i] = None,
                _ => {}
            }
        }

        // Build overlay text buffers fresh each frame.
        let mut overlay_bufs: Vec<Buffer> = Vec::with_capacity(overlay_lines.len());
        for line in overlay_lines {
            let line_family = match line.family.as_deref() {
                Some(n) => Family::Name(n),
                None => family,
            };
            let mut buf = grid_text_buffer(&mut self.font_system, metrics, width as f32);
            let spans: Vec<(&str, Attrs)> = line
                .spans
                .iter()
                .map(|s| {
                    let mut a = Attrs::new()
                        .family(line_family)
                        .color(GColor::rgba(s.fg[0], s.fg[1], s.fg[2], s.fg[3]));
                    if s.bold {
                        a = a.weight(Weight::BOLD);
                    }
                    (s.text.as_str(), a)
                })
                .collect();
            let line_attrs = Attrs::new().family(line_family);
            buf.set_rich_text(spans, &line_attrs, Shaping::Advanced, Some(Align::Left));
            align_to_cell_grid(&mut buf, &mut self.font_system, self.cell_w);
            overlay_bufs.push(buf);
        }

        // Quads are authored in sRGB; convert to linear for the *_SRGB target.
        let lin = |list: &[Instance]| -> Vec<Instance> {
            list.iter()
                .take(self.inst_capacity)
                .map(|i| Instance {
                    color: crate::colors::srgb_to_linear(i.color),
                    ..*i
                })
                .collect()
        };
        let bg_lin = lin(bg);
        let overlay_bg_lin = lin(overlay_bg);
        let fx_lin = lin(fx);
        let bg_all: Vec<Instance> = bg_lin
            .iter()
            .chain(overlay_bg_lin.iter())
            .copied()
            .collect();
        let bg_count = bg_lin.len();
        let overlay_count = overlay_bg_lin.len();
        self.queue
            .write_buffer(&self.bg_instances, 0, bytemuck::cast_slice(&bg_all));
        self.queue
            .write_buffer(&self.fx_instances, 0, bytemuck::cast_slice(&fx_lin));

        self.viewport
            .update(&self.queue, Resolution { width, height });

        let terminal_areas: Vec<TextArea> = lines
            .iter()
            .zip(&self.line_bufs)
            .filter_map(|(line, buf)| {
                let (line, buf) = line.as_ref().zip(buf.as_ref())?;
                Some(TextArea {
                    buffer: buf,
                    left: line.left,
                    top: line.top,
                    scale: 1.0,
                    bounds: TextBounds {
                        left: 0,
                        top: 0,
                        right: width as i32,
                        bottom: (line.top + metrics.line_height) as i32,
                    },
                    default_color: GColor::rgb(200, 200, 200),
                    custom_glyphs: &[],
                })
            })
            .collect();

        // Prepare terminal text before starting the render pass so we never
        // update the atlas texture while it is bound for sampling.
        self.text_renderer.prepare(
            &self.device,
            &self.queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            terminal_areas.into_iter(),
            &mut self.swash,
        )?;

        if overlay_count > 0 {
            let overlay_areas: Vec<TextArea> = overlay_lines
                .iter()
                .zip(&overlay_bufs)
                .map(|(line, buf)| TextArea {
                    buffer: buf,
                    left: line.left,
                    top: line.top,
                    scale: 1.0,
                    bounds: TextBounds {
                        left: 0,
                        top: 0,
                        right: width as i32,
                        bottom: (line.top + metrics.line_height) as i32,
                    },
                    default_color: GColor::rgb(200, 200, 200),
                    custom_glyphs: &[],
                })
                .collect();

            self.overlay_text_renderer.prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                overlay_areas.into_iter(),
                &mut self.swash,
            )?;
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("term"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear[0] as f64,
                            g: clear[1] as f64,
                            b: clear[2] as f64,
                            a: clear[3] as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.quad_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, self.quad_vb.slice(..));

            if bg_count > 0 {
                pass.set_vertex_buffer(1, self.bg_instances.slice(..));
                pass.draw(0..4, 0..bg_count as u32);
            }

            self.text_renderer
                .render(&self.atlas, &self.viewport, &mut pass)?;

            if !fx_lin.is_empty() {
                // glyphon's render() clobbered vertex slot 0; restore ours.
                pass.set_pipeline(&self.quad_pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, self.quad_vb.slice(..));
                pass.set_vertex_buffer(1, self.fx_instances.slice(..));
                pass.draw(0..4, 0..fx_lin.len() as u32);
            }
        }

        if overlay_count > 0 {
            {
                let inst_size = std::mem::size_of::<Instance>() as u64;
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("overlay"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&self.quad_pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, self.quad_vb.slice(..));
                pass.set_vertex_buffer(1, self.bg_instances.slice((bg_count as u64) * inst_size..));
                pass.draw(0..4, 0..overlay_count as u32);

                self.overlay_text_renderer
                    .render(&self.atlas, &self.viewport, &mut pass)?;
            }
        }

        self.queue.submit([encoder.finish()]);
        self.atlas.trim();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_fullwidth_and_styled_text_matches_cell_columns() {
        let mut fonts = FontSystem::new();
        let mut buffer = grid_text_buffer(&mut fonts, Metrics::new(14.0, 18.0), 64.0);
        let attrs = Attrs::new().family(Family::Monospace);
        let accent = GColor::rgb(80, 160, 240);
        buffer.set_rich_text(
            vec![
                ("A日", attrs.clone()),
                ("B本X", attrs.clone().weight(Weight::BOLD).color(accent)),
            ],
            &attrs,
            Shaping::Advanced,
            Some(Align::Left),
        );
        align_to_cell_grid(&mut buffer, &mut fonts, 8.0);
        let run = buffer.layout_runs().next().unwrap();
        for glyph in run.glyphs {
            let column = run.text[..glyph.start].width();
            assert!(
                (glyph.x - column as f32 * 8.0).abs() < 0.05,
                "start={} x={} column={column}",
                glyph.start,
                glyph.x
            );
            if run.text[glyph.start..glyph.end].contains('B') {
                assert_eq!(glyph.color_opt, Some(accent));
            }
        }
    }

    #[test]
    fn text_advances_match_the_background_cell_grid() {
        let mut fonts = FontSystem::new();
        let mut buffer = grid_text_buffer(&mut fonts, Metrics::new(14.0, 18.0), 32.0);
        buffer.set_text(
            "AAAAAAAAAAMX",
            &Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
            None,
        );
        align_to_cell_grid(&mut buffer, &mut fonts, 8.0);
        let run = buffer.layout_runs().next().unwrap();
        assert_eq!(run.glyphs.len(), 12);
        for glyph in run.glyphs {
            assert!(
                (glyph.x - glyph.start as f32 * 8.0).abs() < 0.05,
                "start={} x={} w={} size={}",
                glyph.start,
                glyph.x,
                glyph.w,
                glyph.font_size
            );
        }
    }
}
