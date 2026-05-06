//! Custom wgpu pass for terminal panes.
//!
//! Status: scaffold. The full plan is to replace the per-cell
//! `Painter::text` calls in `view.rs` with a custom WGSL shader that
//! draws cell backgrounds + glyphs from a managed atlas, getting us
//! out of egui's general-purpose text path for the terminal grid.
//!
//! This first slice wires the `egui_wgpu::CallbackTrait` integration
//! end-to-end:
//!
//! 1. [`ensure_initialized`] is called once per frame from
//!    `main.rs::ui`. The first call grabs `frame.wgpu_render_state()`
//!    and pushes a [`GpuRenderResources`] (pipeline + shader) into
//!    `Renderer::callback_resources`. Subsequent calls early-return.
//!
//! 2. [`paint_test_pattern`] is called from `view.rs` when the
//!    `CRANE_GPU_TERM=1` env var is set. It submits a `PaintCallback`
//!    over the terminal pane's content rect. The callback runs a
//!    minimal shader that fills the rect with a diagnostic gradient
//!    so we can confirm the wgpu sub-pass actually fires inside the
//!    egui frame.
//!
//! Once the test pattern lands, follow-up sessions add: glyph atlas
//! (font rasterization via `fontdue`), per-cell instance data, SGR
//! color encoding, cursor + selection overlays. The legacy egui
//! renderer stays in place behind the same env gate so we can A/B.
//!
//! Toggle: `CRANE_GPU_TERM=1 cargo run`.

use eframe::wgpu;
use eframe::wgpu::util::DeviceExt;
use std::sync::atomic::{AtomicBool, Ordering};

const SHADER_SRC: &str = r#"
// Trivial pass-through shader: renders a fullscreen-of-the-quad
// gradient so we can visually confirm the callback ran inside the
// expected screen rect. Replaced by the cell-grid shader in the
// next pass.

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    // Two triangles forming a [0,1]² quad in clip space.
    var positions = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0),
        vec2( 1.0, -1.0),
        vec2(-1.0,  1.0),
        vec2(-1.0,  1.0),
        vec2( 1.0, -1.0),
        vec2( 1.0,  1.0),
    );
    var uvs = array<vec2<f32>, 6>(
        vec2(0.0, 1.0),
        vec2(1.0, 1.0),
        vec2(0.0, 0.0),
        vec2(0.0, 0.0),
        vec2(1.0, 1.0),
        vec2(1.0, 0.0),
    );
    var out: VsOut;
    out.pos = vec4<f32>(positions[vid], 0.0, 1.0);
    out.uv = uvs[vid];
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Diagonal magenta→cyan gradient so it's unmistakably the GPU
    // pass and not the egui Painter.
    let c = vec3<f32>(in.uv.x, 0.2, 1.0 - in.uv.x);
    return vec4<f32>(c, 0.35);
}
"#;

/// Resources shared across every terminal pane's callback. Lives in
/// `egui_wgpu::Renderer::callback_resources` for the app's lifetime.
pub struct GpuRenderResources {
    pub pipeline: wgpu::RenderPipeline,
    /// One reusable instance buffer; rebuilt per-frame in
    /// [`GpuTerminalCallback::prepare`]. Sized for the test pattern
    /// (no per-cell instance data yet).
    #[allow(dead_code)]
    pub instance_buffer: wgpu::Buffer,
}

/// Idempotent init guard. Setting it inside `ensure_initialized` past
/// the early-return ensures we only build the pipeline once per
/// process, even though `ensure_initialized` is called every frame.
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// First-frame initialization. Reads the wgpu device/queue out of
/// eframe's render state and stuffs a [`GpuRenderResources`] into
/// `callback_resources` so subsequent `paint` calls find it.
pub fn ensure_initialized(frame: &eframe::Frame) {
    if INITIALIZED.load(Ordering::Acquire) {
        return;
    }
    let Some(render_state) = frame.wgpu_render_state() else {
        // Not running under the wgpu backend (eframe was compiled
        // without the wgpu feature, or running headless). Nothing
        // to do; the legacy egui-painter path will handle drawing.
        return;
    };
    let device = &render_state.device;

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("crane.terminal.gpu_render.shader"),
        source: wgpu::ShaderSource::Wgsl(SHADER_SRC.into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("crane.terminal.gpu_render.layout"),
        bind_group_layouts: &[],
        immediate_size: 0,
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("crane.terminal.gpu_render.pipeline"),
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
            // Match the egui surface format so the blend math
            // composites correctly with whatever egui drew below us.
            targets: &[Some(wgpu::ColorTargetState {
                format: render_state.target_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    let instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("crane.terminal.gpu_render.instance_buffer"),
        contents: &[0u8; 16], // placeholder; resized when we have real instance data
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
    });

    let resources = GpuRenderResources {
        pipeline,
        instance_buffer,
    };

    render_state
        .renderer
        .write()
        .callback_resources
        .insert(resources);

    INITIALIZED.store(true, Ordering::Release);
}

/// `egui_wgpu::CallbackTrait` impl that runs our pipeline inside the
/// supplied screen-space rect. The first version draws the test
/// gradient with no inputs; the per-cell version will carry an
/// instance vec.
pub struct GpuTerminalCallback;

impl eframe::egui_wgpu::CallbackTrait for GpuTerminalCallback {
    fn paint(
        &self,
        _info: egui::epaint::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &eframe::egui_wgpu::CallbackResources,
    ) {
        let Some(resources) = callback_resources.get::<GpuRenderResources>() else {
            return;
        };
        render_pass.set_pipeline(&resources.pipeline);
        render_pass.draw(0..6, 0..1);
    }
}

/// Submit a paint callback over `rect`. No-op when not running under
/// the wgpu backend.
pub fn paint_test_pattern(painter: &egui::Painter, rect: egui::Rect) {
    let cb = eframe::egui_wgpu::Callback::new_paint_callback(rect, GpuTerminalCallback);
    painter.add(cb);
}

/// Runtime toggle. We use an env var rather than a Settings field so
/// turning the GPU path on/off doesn't require touching session state
/// while the renderer is still under construction.
pub fn enabled() -> bool {
    std::env::var("CRANE_GPU_TERM").ok().as_deref() == Some("1")
}
