use crate::camera::{Camera, VERTICAL_FOV_RADIANS, triangle_hit};
use eframe::{egui, egui_wgpu, wgpu};
use faris_engine::mesh::MeshVertex;
use std::sync::Arc;
use wgpu::util::DeviceExt;

/// Colored triangle vertex for spatial field bins.
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
}

/// One corner of a constant-pixel-width line segment (six per segment). `other`
/// is the opposite endpoint; `style` is (width in pixels, kind) with kind 0
/// grid, 1 axis, 2 accent outline.
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
struct LineVertex {
    position: [f32; 3],
    other: [f32; 3],
    color: [f32; 3],
    style: [f32; 2],
}

#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
struct Uniform {
    eye: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    forward: [f32; 4],
    projection: [f32; 4],
    display: [f32; 4],
    color: [f32; 4],
    scene: [f32; 4],
    screen: [f32; 4],
}

#[derive(Clone)]
pub struct ComponentDraw {
    pub vertices: Arc<[MeshVertex]>,
    pub color: [f32; 3],
}

/// Axis-aligned outboard port box in metres, drawn as an accent outline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PortBox {
    pub minimum: [f32; 3],
    pub maximum: [f32; 3],
}

/// Everything one viewport frame draws.
#[derive(Clone)]
pub struct Scene {
    /// Per-bin field quads (six vertices each) for the spatial flux view.
    pub slice: Arc<[Vertex]>,
    pub components: Arc<[ComponentDraw]>,
    pub revision: u64,
    /// Calculated-field view: unlit flat colour that encodes the fixed scale.
    pub flat_color: bool,
    /// Spatial view: components become faint ghost context around the slice.
    pub slice_view: bool,
    pub port: Option<PortBox>,
    /// Component under the cursor (index into `components`).
    pub hover: Option<usize>,
}

struct ComponentUniform {
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

struct Resources {
    target_srgb: bool,
    background_pipeline: wgpu::RenderPipeline,
    solid_pipeline: wgpu::RenderPipeline,
    ghost_pipeline: wgpu::RenderPipeline,
    slice_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    frame_buffer: wgpu::Buffer,
    frame_bind_group: wgpu::BindGroup,
    slice_buffer: wgpu::Buffer,
    slice_count: u32,
    slice_revision: u64,
    line_buffer: wgpu::Buffer,
    line_count: u32,
    previous_lines: Vec<LineVertex>,
    /// (floor height, grid half extent) of the rendered scene.
    bounds: (f32, f32),
    component_geometry_buffer: wgpu::Buffer,
    component_vertex_count: u32,
    component_ranges: Vec<(u32, u32)>,
    previous_component_vertices: Vec<Arc<[MeshVertex]>>,
    component_uniforms: Vec<ComponentUniform>,
}

fn create_pipeline(
    (device, layout, shader, format): (
        &wgpu::Device,
        &wgpu::PipelineLayout,
        &wgpu::ShaderModule,
        wgpu::TextureFormat,
    ),
    label: &str,
    entry_points: (&str, &str),
    buffers: &[Option<wgpu::VertexBufferLayout>],
    blend: Option<wgpu::BlendState>,
    depth: (bool, wgpu::CompareFunction),
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(entry_points.0),
            buffers,
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(entry_points.1),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(depth.0),
            depth_compare: Some(depth.1),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

pub fn initialize(state: &egui_wgpu::RenderState) {
    let device = &state.device;
    let format = state.target_format;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("FARIS viewport shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("viewport.wgsl").into()),
    });
    let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("FARIS camera"),
        size: std::mem::size_of::<Uniform>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("FARIS camera layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let frame_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("FARIS camera bind group"),
        layout: &bind_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: frame_buffer.as_entire_binding(),
        }],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("FARIS viewport pipeline layout"),
        bind_group_layouts: &[Some(&bind_layout)],
        immediate_size: 0,
    });
    let component_buffer = [Some(wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<[f32; 6]>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
    })];
    let slice_buffer_layout = [Some(wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3],
    })];
    let line_buffer_layout = [Some(wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<LineVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3, 3 => Float32x2],
    })];
    let pipeline = |label, entries, buffers: &[Option<wgpu::VertexBufferLayout>], blend, depth| {
        create_pipeline(
            (device, &pipeline_layout, &shader, format),
            label,
            entries,
            buffers,
            blend,
            depth,
        )
    };
    use wgpu::CompareFunction::{Always, Less};
    let background_pipeline = pipeline(
        "FARIS background gradient",
        ("vertex_background", "fragment_background"),
        &[],
        None,
        (false, Always),
    );
    let solid_pipeline = pipeline(
        "FARIS component pipeline",
        ("vertex_component", "fragment_solid"),
        &component_buffer,
        None,
        (true, Less),
    );
    let ghost_pipeline = pipeline(
        "FARIS component ghost pipeline",
        ("vertex_component", "fragment_ghost"),
        &component_buffer,
        Some(wgpu::BlendState::ALPHA_BLENDING),
        (false, Less),
    );
    let slice_pipeline = pipeline(
        "FARIS spatial field pipeline",
        ("vertex_slice", "fragment_slice"),
        &slice_buffer_layout,
        None,
        (true, Less),
    );
    let line_pipeline = pipeline(
        "FARIS line pipeline",
        ("vertex_line", "fragment_line"),
        &line_buffer_layout,
        Some(wgpu::BlendState::ALPHA_BLENDING),
        (false, Less),
    );
    let placeholder = |label, extra: wgpu::BufferUsages| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: 4,
            usage: wgpu::BufferUsages::VERTEX | extra,
            mapped_at_creation: false,
        })
    };
    state.renderer.write().callback_resources.insert(Resources {
        target_srgb: format.is_srgb(),
        background_pipeline,
        solid_pipeline,
        ghost_pipeline,
        slice_pipeline,
        line_pipeline,
        bind_layout,
        frame_buffer,
        frame_bind_group,
        slice_buffer: placeholder("FARIS spatial field bins", wgpu::BufferUsages::empty()),
        slice_count: 0,
        slice_revision: u64::MAX,
        line_buffer: placeholder(
            "FARIS grid, axes and port lines",
            wgpu::BufferUsages::empty(),
        ),
        line_count: 0,
        previous_lines: Vec::new(),
        bounds: (-2.5, 6.0),
        component_geometry_buffer: placeholder(
            "FARIS component geometry",
            wgpu::BufferUsages::empty(),
        ),
        component_vertex_count: 0,
        component_ranges: Vec::new(),
        previous_component_vertices: Vec::new(),
        component_uniforms: Vec::new(),
    });
}

pub fn paint(rect: egui::Rect, camera: Camera, scene: &Scene) -> egui::PaintCallback {
    let (eye, right, up, forward) = camera.basis();
    let extend = |v: [f32; 3]| [v[0], v[1], v[2], 0.0];
    egui_wgpu::Callback::new_paint_callback(
        rect,
        ViewportCallback {
            scene: scene.clone(),
            rect,
            uniform: Uniform {
                eye: extend(eye),
                right: extend(right),
                up: extend(up),
                forward: extend(forward),
                projection: [
                    rect.width() / rect.height().max(1.0),
                    (VERTICAL_FOV_RADIANS * 0.5).tan(),
                    0.1,
                    100.0,
                ],
                display: [if scene.flat_color { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
                color: [1.0, 1.0, 1.0, 1.0],
                scene: [0.0, 0.0, 0.0, 6.0],
                screen: [rect.width().max(1.0), rect.height().max(1.0), 0.0, 0.0],
            },
        },
    )
}

struct ViewportCallback {
    scene: Scene,
    rect: egui::Rect,
    uniform: Uniform,
}

impl egui_wgpu::CallbackTrait for ViewportCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let resources = resources
            .get_mut::<Resources>()
            .expect("viewport resources initialized at startup");
        let scene = &self.scene;
        let components = &scene.components;
        let same_geometry = resources.previous_component_vertices.len() == components.len()
            && resources
                .previous_component_vertices
                .iter()
                .zip(components.iter())
                .all(|(previous, current)| Arc::ptr_eq(previous, &current.vertices));
        if !same_geometry {
            let mut geometry = Vec::<[f32; 6]>::new();
            let mut ranges = Vec::with_capacity(components.len());
            for component in components.iter() {
                let start = geometry.len() as u32;
                geometry.extend(component.vertices.iter().map(mesh_geometry_attributes));
                ranges.push((start, geometry.len() as u32));
            }
            if !geometry.is_empty() {
                resources.component_geometry_buffer =
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("FARIS component geometry"),
                        contents: bytemuck::cast_slice(&geometry),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
            }
            resources.component_vertex_count = geometry.len() as u32;
            resources.component_ranges = ranges;
            if !components.is_empty() {
                resources.bounds = world_reference_bounds_positions(
                    components
                        .iter()
                        .flat_map(|component| component.vertices.iter().map(|v| v.position)),
                );
            }
        }
        if resources.slice_revision != scene.revision {
            if scene.slice.is_empty() {
                resources.slice_count = 0;
            } else {
                resources.slice_buffer =
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("FARIS spatial field bins"),
                        contents: bytemuck::cast_slice(&scene.slice),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                resources.slice_count = scene.slice.len() as u32;
                if components.is_empty() {
                    resources.bounds = world_reference_bounds(&scene.slice);
                }
            }
            resources.slice_revision = scene.revision;
        }
        while resources.component_uniforms.len() < components.len() {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("FARIS component color and camera"),
                size: std::mem::size_of::<Uniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("FARIS component color bind group"),
                layout: &resources.bind_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
            resources
                .component_uniforms
                .push(ComponentUniform { buffer, bind_group });
        }
        resources.component_uniforms.truncate(components.len());

        let (floor, extent) = resources.bounds;
        let mut uniform = self.uniform;
        uniform.display[1] = if resources.target_srgb { 1.0 } else { 0.0 };
        uniform.scene = [0.0, floor, 0.0, extent];
        uniform.screen = [
            (self.rect.width() * screen.pixels_per_point).max(1.0),
            (self.rect.height() * screen.pixels_per_point).max(1.0),
            0.0,
            0.0,
        ];
        queue.write_buffer(&resources.frame_buffer, 0, bytemuck::bytes_of(&uniform));
        for (index, (component, binding)) in components
            .iter()
            .zip(&resources.component_uniforms)
            .enumerate()
        {
            let mut per_component = uniform;
            per_component.color = [
                component.color[0],
                component.color[1],
                component.color[2],
                1.0,
            ];
            if scene.hover == Some(index) {
                per_component.display[3] = 1.0;
            }
            queue.write_buffer(&binding.buffer, 0, bytemuck::bytes_of(&per_component));
        }
        resources.previous_component_vertices = components
            .iter()
            .map(|component| Arc::clone(&component.vertices))
            .collect();

        let lines = reference_lines(floor, extent, scene.port);
        if bytemuck::cast_slice::<_, u8>(&lines)
            != bytemuck::cast_slice::<_, u8>(&resources.previous_lines)
        {
            resources.line_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("FARIS grid, axes and port lines"),
                contents: bytemuck::cast_slice(&lines),
                usage: wgpu::BufferUsages::VERTEX,
            });
            resources.line_count = lines.len() as u32;
            resources.previous_lines = lines;
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let resources = resources
            .get::<Resources>()
            .expect("viewport resources initialized at startup");
        let scene = &self.scene;
        pass.set_bind_group(0, &resources.frame_bind_group, &[]);
        pass.set_pipeline(&resources.background_pipeline);
        pass.draw(0..3, 0..1);
        let draw_components = |pass: &mut wgpu::RenderPass<'static>| {
            if resources.component_vertex_count == 0 {
                return;
            }
            pass.set_vertex_buffer(0, resources.component_geometry_buffer.slice(..));
            for (index, &(start, end)) in resources.component_ranges.iter().enumerate() {
                if start == end || scene.components.get(index).is_none() {
                    continue;
                }
                pass.set_bind_group(0, &resources.component_uniforms[index].bind_group, &[]);
                pass.draw(start..end, 0..1);
            }
            pass.set_bind_group(0, &resources.frame_bind_group, &[]);
        };
        if scene.slice_view {
            if resources.slice_count > 0 {
                pass.set_pipeline(&resources.slice_pipeline);
                pass.set_vertex_buffer(0, resources.slice_buffer.slice(..));
                pass.draw(0..resources.slice_count, 0..1);
            }
            // Faint context geometry, drawn after the opaque slice and
            // depth-tested against it without writing depth.
            pass.set_pipeline(&resources.ghost_pipeline);
            draw_components(pass);
        } else {
            pass.set_pipeline(&resources.solid_pipeline);
            draw_components(pass);
        }
        if resources.line_count > 0 {
            pass.set_pipeline(&resources.line_pipeline);
            pass.set_vertex_buffer(0, resources.line_buffer.slice(..));
            pass.draw(0..resources.line_count, 0..1);
        }
    }
}

fn mesh_geometry_attributes(vertex: &MeshVertex) -> [f32; 6] {
    [
        vertex.position[0],
        vertex.position[1],
        vertex.position[2],
        vertex.normal[0],
        vertex.normal[1],
        vertex.normal[2],
    ]
}

const GRID_COLOR: [f32; 3] = [0.62, 0.70, 0.82];
const PORT_COLOR: [f32; 3] = [1.0, 0.70, 0.18];

/// Floor grid, short orientation axes and the optional port outline, in
/// metres. Lines have constant pixel width; the grid fades with distance from
/// the scene centre and every line is depth-tested against the components.
fn reference_lines(grid_y: f32, half_extent: f32, port: Option<PortBox>) -> Vec<LineVertex> {
    let half_extent = half_extent.clamp(4.0, 1000.0);
    let mut lines = Vec::new();
    let step = if half_extent > 50.0 {
        10.0
    } else if half_extent > 20.0 {
        5.0
    } else {
        1.0
    };
    let count = (half_extent / step).floor() as i32;
    for tick in -count..=count {
        let v = tick as f32 * step;
        let width = if tick == 0 { 1.4 } else { 1.0 };
        add_segment(
            &mut lines,
            [v, grid_y, -half_extent],
            [v, grid_y, half_extent],
            GRID_COLOR,
            width,
            0.0,
        );
        add_segment(
            &mut lines,
            [-half_extent, grid_y, v],
            [half_extent, grid_y, v],
            GRID_COLOR,
            width,
            0.0,
        );
    }
    // Standard additive RGB axis convention: X red, Y green, Z blue.
    let axis = (half_extent * 0.3).clamp(1.0, 2.5);
    for (end, color) in [
        ([axis, 0.0, 0.0], [0.90, 0.28, 0.25]),
        ([0.0, axis, 0.0], [0.35, 0.84, 0.45]),
        ([0.0, 0.0, axis], [0.30, 0.56, 0.98]),
    ] {
        add_segment(&mut lines, [0.0; 3], end, color, 2.0, 1.0);
    }
    if let Some(port) = port {
        let (lo, hi) = (port.minimum, port.maximum);
        let corner = |i: usize| {
            [
                if i & 1 == 0 { lo[0] } else { hi[0] },
                if i & 2 == 0 { lo[1] } else { hi[1] },
                if i & 4 == 0 { lo[2] } else { hi[2] },
            ]
        };
        for i in 0..8_usize {
            for bit in [1_usize, 2, 4] {
                if i & bit == 0 {
                    add_segment(&mut lines, corner(i), corner(i | bit), PORT_COLOR, 2.5, 2.0);
                }
            }
        }
    }
    lines
}

fn world_reference_bounds(vertices: &[Vertex]) -> (f32, f32) {
    if vertices.is_empty() {
        return (-2.5, 6.0);
    }
    world_reference_bounds_positions(vertices.iter().map(|vertex| vertex.position))
}

fn world_reference_bounds_positions(positions: impl Iterator<Item = [f32; 3]>) -> (f32, f32) {
    let mut minimum_y = f32::INFINITY;
    let mut horizontal_extent = 0.0_f32;
    for p in positions {
        if !p.into_iter().all(f32::is_finite) {
            continue;
        }
        minimum_y = minimum_y.min(p[1]);
        horizontal_extent = horizontal_extent.max(p[0].abs()).max(p[2].abs());
    }
    if !minimum_y.is_finite() {
        return (-2.5, 6.0);
    }
    let extent = (horizontal_extent.ceil() + 1.0).clamp(4.0, 1000.0);
    (minimum_y - 0.2, extent)
}

fn add_segment(
    out: &mut Vec<LineVertex>,
    start: [f32; 3],
    end: [f32; 3],
    color: [f32; 3],
    width_px: f32,
    kind: f32,
) {
    let length = start
        .iter()
        .zip(end)
        .map(|(a, b)| (b - a) * (b - a))
        .sum::<f32>()
        .sqrt();
    if !length.is_finite() || length <= f32::EPSILON || !width_px.is_finite() || width_px <= 0.0 {
        return;
    }
    // Corner order matches `vertex_line`: (start, -), (start, +), (end, +),
    // (start, -), (end, +), (end, -).
    for at_start in [true, true, false, true, false, false] {
        let (position, other) = if at_start { (start, end) } else { (end, start) };
        out.push(LineVertex {
            position,
            other,
            color,
            style: [width_px, kind],
        });
    }
}

/// Projects a world point into the viewport rectangle with the exact camera
/// the shader uses. `None` when the point is behind the camera.
pub fn project(camera: &Camera, rect: egui::Rect, point: [f32; 3]) -> Option<egui::Pos2> {
    let (eye, right, up, forward) = camera.basis();
    let relative = crate::camera::subtract(point, eye);
    let depth = crate::camera::dot(relative, forward);
    if depth <= 0.1 {
        return None;
    }
    let tangent = (VERTICAL_FOV_RADIANS * 0.5).tan();
    let aspect = rect.width() / rect.height().max(1.0);
    let x = crate::camera::dot(relative, right) / (aspect * tangent) / depth;
    let y = crate::camera::dot(relative, up) / tangent / depth;
    Some(egui::pos2(
        rect.left() + (x + 1.0) * 0.5 * rect.width(),
        rect.top() + (1.0 - y) * 0.5 * rect.height(),
    ))
}

/// Screen-space annotations that need the same camera as the 3D pass.
pub fn paint_labels(painter: &egui::Painter, rect: egui::Rect, camera: Camera, scene: &Scene) {
    let Some(port) = scene.port else {
        return;
    };
    let painter = painter.with_clip_rect(rect);
    let anchor = [
        port.maximum[0],
        (port.minimum[1] + port.maximum[1]) * 0.5,
        (port.minimum[2] + port.maximum[2]) * 0.5,
    ];
    let Some(point) = project(&camera, rect, anchor) else {
        return;
    };
    let amber = egui::Color32::from_rgb(255, 178, 46);
    let galley = painter.layout_no_wrap(
        "Outboard port".to_owned(),
        egui::FontId::proportional(13.0),
        amber,
    );
    let padding = egui::vec2(7.0, 4.0);
    let size = galley.size() + padding * 2.0;
    let mut min = point + egui::vec2(26.0, -34.0 - size.y * 0.5);
    min.x = min
        .x
        .min(rect.right() - size.x - 6.0)
        .max(rect.left() + 6.0);
    min.y = min.y.clamp(
        rect.top() + 6.0,
        (rect.bottom() - size.y - 6.0).max(rect.top() + 6.0),
    );
    let label = egui::Rect::from_min_size(min, size);
    painter.line_segment(
        [point, label.left_center()],
        egui::Stroke::new(1.2, amber.gamma_multiply(0.8)),
    );
    painter.circle_filled(point, 3.5, amber);
    painter.rect_filled(
        label,
        5.0,
        egui::Color32::from_rgba_unmultiplied(18, 21, 28, 215),
    );
    painter.rect_stroke(
        label,
        5.0,
        egui::Stroke::new(1.0, amber.gamma_multiply(0.7)),
        egui::StrokeKind::Inside,
    );
    painter.galley(label.min + padding, galley, amber);
}

/// Displayed camera that eases toward the interaction target each frame. The
/// first call (and any non-finite target) snaps, so captures and restored
/// views are exact; repaints are requested only while moving.
pub fn eased_camera(ctx: &egui::Context, goal: Camera) -> Camera {
    let id = egui::Id::new("faris-viewport-displayed-camera");
    let current = ctx.data(|data| data.get_temp::<Camera>(id));
    let dt = ctx.input(|input| input.stable_dt).clamp(0.0, 0.1);
    let (next, settled) = match current {
        Some(current) => current.eased_toward(&goal, 1.0 - (-dt * 14.0).exp()),
        None => (goal, true),
    };
    ctx.data_mut(|data| data.insert_temp(id, next));
    if !settled {
        ctx.request_repaint();
    }
    next
}

#[derive(Clone, Default)]
struct HoverState {
    pointer: Option<egui::Pos2>,
    camera: Option<Camera>,
    checked_at: f64,
    hit: Option<usize>,
}

/// Component under the cursor. The CPU ray test runs only when the pointer or
/// camera moved, at most every 50 ms, and never while dragging.
pub fn hover_component(
    ctx: &egui::Context,
    response: &egui::Response,
    rect: egui::Rect,
    camera: Camera,
    components: &[ComponentDraw],
) -> Option<usize> {
    let id = egui::Id::new("faris-viewport-hover");
    let mut state = ctx
        .data(|data| data.get_temp::<HoverState>(id))
        .unwrap_or_default();
    let pointer = if response.hovered() && !response.dragged() {
        ctx.input(|input| input.pointer.hover_pos())
    } else {
        None
    };
    let now = ctx.input(|input| input.time);
    match pointer {
        None => {
            state.hit = None;
            state.pointer = None;
        }
        Some(point) => {
            let moved = state.pointer != Some(point) || state.camera != Some(camera);
            if moved && now - state.checked_at >= 0.05 {
                state.hit = pick_component(&camera, rect, point, components);
                state.pointer = Some(point);
                state.camera = Some(camera);
                state.checked_at = now;
            } else if moved {
                ctx.request_repaint_after(std::time::Duration::from_millis(60));
            }
        }
    }
    let hit = state.hit.filter(|index| *index < components.len());
    ctx.data_mut(|data| data.insert_temp(id, state));
    hit
}

pub fn pick_component(
    camera: &Camera,
    rect: egui::Rect,
    point: egui::Pos2,
    components: &[ComponentDraw],
) -> Option<usize> {
    let (origin, direction) = camera.ray(rect, point);
    let mut nearest = f32::INFINITY;
    let mut hit = None;
    for (index, component) in components.iter().enumerate() {
        for triangle in component.vertices.as_chunks::<3>().0 {
            if let Some(distance) = triangle_hit(
                origin,
                direction,
                [
                    triangle[0].position,
                    triangle[1].position,
                    triangle[2].position,
                ],
            ) && distance < nearest
            {
                nearest = distance;
                hit = Some(index);
            }
        }
    }
    hit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_lines_are_finite_in_whole_segments_with_all_positive_axes() {
        let lines = reference_lines(-2.5, 6.0, None);
        assert_eq!(lines.len() % 6, 0);
        assert!(lines.iter().all(|v| {
            v.position
                .into_iter()
                .chain(v.other)
                .chain(v.color)
                .chain(v.style)
                .all(f32::is_finite)
        }));
        assert!(lines.iter().any(|v| v.style[1] == 1.0 && v.color[0] > 0.8));
        assert!(lines.iter().any(|v| v.style[1] == 1.0 && v.color[1] > 0.8));
        assert!(lines.iter().any(|v| v.style[1] == 1.0 && v.color[2] > 0.9));
        assert!(lines.iter().all(|v| v.style[1] != 2.0));
    }

    #[test]
    fn port_outline_adds_twelve_edges_of_the_box() {
        let port = PortBox {
            minimum: [4.34, -0.15, -0.15],
            maximum: [5.59, 0.15, 0.15],
        };
        let without = reference_lines(-2.5, 6.0, None).len();
        let with = reference_lines(-2.5, 6.0, Some(port));
        assert_eq!(with.len() - without, 12 * 6);
        assert!(
            with.iter()
                .filter(|v| v.style[1] == 2.0)
                .all(|v| (4.34..=5.59).contains(&v.position[0]))
        );
    }

    #[test]
    fn segments_ignore_degenerate_input() {
        let mut lines = Vec::new();
        add_segment(&mut lines, [1.0; 3], [1.0; 3], [1.0; 3], 1.0, 0.0);
        add_segment(
            &mut lines,
            [0.0; 3],
            [1.0, 0.0, 0.0],
            [1.0; 3],
            f32::NAN,
            0.0,
        );
        assert!(lines.is_empty());
    }

    #[test]
    fn segment_corners_alternate_start_and_end_in_the_shader_order() {
        let mut lines = Vec::new();
        add_segment(&mut lines, [0.0; 3], [1.0, 0.0, 0.0], [1.0; 3], 1.0, 0.0);
        let at_start: Vec<bool> = lines.iter().map(|v| v.position[0] == 0.0).collect();
        assert_eq!(at_start, [true, true, false, true, false, false]);
    }

    #[test]
    fn world_reference_tracks_the_rendered_scene_bounds() {
        let mesh = [
            Vertex {
                position: [-7.3, -3.1, 5.2],
                normal: [0.0; 3],
                color: [0.0; 3],
            },
            Vertex {
                position: [7.3, 2.0, -5.2],
                normal: [0.0; 3],
                color: [0.0; 3],
            },
        ];
        let (floor, extent) = world_reference_bounds(&mesh);
        assert_eq!(floor, -3.3);
        assert_eq!(extent, 9.0);
        assert_eq!(world_reference_bounds(&[]), (-2.5, 6.0));
    }

    #[test]
    fn projection_matches_the_camera_ray_and_hides_points_behind() {
        let camera = Camera::default();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let centre = project(&camera, rect, camera.target).unwrap();
        assert!((centre - rect.center()).length() < 1e-2);
        let aside = crate::camera::add(camera.target, camera.basis().1);
        let projected = project(&camera, rect, aside).unwrap();
        let (_, direction) = camera.ray(rect, projected);
        let toward = crate::camera::subtract(aside, camera.basis().0);
        let length = crate::camera::dot(toward, toward).sqrt();
        let cosine = crate::camera::dot(direction, toward) / length;
        assert!(cosine > 0.99999);
        let behind = crate::camera::add(
            camera.basis().0,
            crate::camera::scale(camera.basis().3, -2.0),
        );
        assert!(project(&camera, rect, behind).is_none());
    }

    // Verifies: VIS-007
    #[test]
    fn pointer_pick_selects_the_nearest_component() {
        let square = |z: f32| -> Arc<[MeshVertex]> {
            let v = |x: f32, y: f32| MeshVertex {
                position: [x, y, z],
                normal: [0.0, 0.0, 1.0],
            };
            Arc::from([v(-1.0, -1.0), v(1.0, -1.0), v(0.0, 1.0)])
        };
        let components = [
            ComponentDraw {
                vertices: square(0.0),
                color: [1.0; 3],
            },
            ComponentDraw {
                vertices: square(1.0),
                color: [1.0; 3],
            },
        ];
        let camera = Camera {
            yaw: std::f32::consts::FRAC_PI_2,
            pitch: 0.0,
            distance: 6.0,
            target: [0.0, -0.2, 0.0],
        };
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        assert_eq!(
            pick_component(&camera, rect, rect.center(), &components),
            Some(1)
        );
        assert_eq!(
            pick_component(&camera, rect, egui::pos2(2.0, 2.0), &components),
            None
        );
    }
}
