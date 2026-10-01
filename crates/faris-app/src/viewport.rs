use crate::camera::{Camera, VERTICAL_FOV_RADIANS};
use eframe::{egui, egui_wgpu, wgpu};
use std::sync::Arc;
use wgpu::util::DeviceExt;

#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
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
}

struct Resources {
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    vertex_buffer: wgpu::Buffer,
    vertex_count: u32,
    context_buffer: wgpu::Buffer,
    context_count: u32,
    revision: u64,
}

pub fn initialize(state: &egui_wgpu::RenderState) {
    let device = &state.device;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("FARIS viewport shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("viewport.wgsl").into()),
    });
    let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
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
    let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("FARIS camera bind group"),
        layout: &bind_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: camera_buffer.as_entire_binding(),
        }],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("FARIS viewport pipeline layout"),
        bind_group_layouts: &[Some(&bind_layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("FARIS viewport pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vertex_main"),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3],
            })],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fragment_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: state.target_format,
                blend: None,
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
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("FARIS empty mesh"),
        size: 4,
        usage: wgpu::BufferUsages::VERTEX,
        mapped_at_creation: false,
    });
    let context_vertices = world_reference_geometry(-2.5, 6.0);
    let context_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("FARIS world grid and orientation axes"),
        contents: bytemuck::cast_slice(&context_vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    state.renderer.write().callback_resources.insert(Resources {
        pipeline,
        camera_buffer,
        camera_bind_group,
        vertex_buffer,
        vertex_count: 0,
        context_buffer,
        context_count: context_vertices.len() as u32,
        revision: u64::MAX,
    });
}

pub fn paint(
    rect: egui::Rect,
    camera: Camera,
    vertices: Arc<[Vertex]>,
    revision: u64,
    flat_color: bool,
) -> egui::PaintCallback {
    let (eye, right, up, forward) = camera.basis();
    let extend = |v: [f32; 3]| [v[0], v[1], v[2], 0.0];
    egui_wgpu::Callback::new_paint_callback(
        rect,
        ViewportCallback {
            vertices,
            revision,
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
                display: [if flat_color { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
            },
        },
    )
}

struct ViewportCallback {
    vertices: Arc<[Vertex]>,
    revision: u64,
    uniform: Uniform,
}

impl egui_wgpu::CallbackTrait for ViewportCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let resources = resources
            .get_mut::<Resources>()
            .expect("viewport resources initialized at startup");
        if resources.revision != self.revision {
            if !self.vertices.is_empty() {
                resources.vertex_buffer =
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("FARIS component meshes"),
                        contents: bytemuck::cast_slice(&self.vertices),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
            }
            resources.vertex_count = self.vertices.len() as u32;
            let (grid_y, extent) = world_reference_bounds(&self.vertices);
            let context_vertices = world_reference_geometry(grid_y, extent);
            resources.context_buffer =
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("FARIS world grid and orientation axes"),
                    contents: bytemuck::cast_slice(&context_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });
            resources.context_count = context_vertices.len() as u32;
            resources.revision = self.revision;
        }
        queue.write_buffer(
            &resources.camera_buffer,
            0,
            bytemuck::bytes_of(&self.uniform),
        );
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
        if resources.vertex_count == 0 {
            return;
        }
        pass.set_pipeline(&resources.pipeline);
        pass.set_bind_group(0, &resources.camera_bind_group, &[]);
        pass.set_vertex_buffer(0, resources.context_buffer.slice(..));
        pass.draw(0..resources.context_count, 0..1);
        pass.set_vertex_buffer(0, resources.vertex_buffer.slice(..));
        pass.draw(0..resources.vertex_count, 0..1);
    }
}

/// Static world reference in metres: a one-metre grid below the modeled torus
/// and RGB positive X/Y/Z axes. Geometry is drawn first and depth-tested against
/// the actual component meshes, so it cannot paint through the vessel.
fn world_reference_geometry(grid_y: f32, half_extent: f32) -> Vec<Vertex> {
    let half_extent = half_extent.clamp(4.0, 1000.0);
    let mut vertices = Vec::new();
    let grid = [0.20, 0.24, 0.29];
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
        add_ribbon(
            &mut vertices,
            [v, grid_y, -half_extent],
            [v, grid_y, half_extent],
            if tick == 0 { 0.012 } else { 0.006 },
            grid,
        );
        add_ribbon(
            &mut vertices,
            [-half_extent, grid_y, v],
            [half_extent, grid_y, v],
            if tick == 0 { 0.012 } else { 0.006 },
            grid,
        );
    }
    // Standard additive RGB axis convention: X red, Y green, Z blue.
    add_ribbon(
        &mut vertices,
        [0.0, 0.0, 0.0],
        [half_extent, 0.0, 0.0],
        0.025,
        [0.90, 0.28, 0.25],
    );
    add_ribbon(
        &mut vertices,
        [0.0, 0.0, 0.0],
        [0.0, half_extent, 0.0],
        0.025,
        [0.35, 0.84, 0.45],
    );
    add_ribbon(
        &mut vertices,
        [0.0, 0.0, 0.0],
        [0.0, 0.0, half_extent],
        0.025,
        [0.30, 0.56, 0.98],
    );
    vertices
}

fn world_reference_bounds(vertices: &[Vertex]) -> (f32, f32) {
    if vertices.is_empty() {
        return (-2.5, 6.0);
    }
    let mut minimum_y = f32::INFINITY;
    let mut horizontal_extent = 0.0_f32;
    for vertex in vertices {
        let p = vertex.position;
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

fn add_ribbon(
    out: &mut Vec<Vertex>,
    start: [f32; 3],
    end: [f32; 3],
    half_width: f32,
    color: [f32; 3],
) {
    let delta = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
    let length = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
    if !length.is_finite() || length <= f32::EPSILON || !half_width.is_finite() || half_width <= 0.0
    {
        return;
    }
    let tangent = [delta[0] / length, delta[1] / length, delta[2] / length];
    // Pick the least-parallel world axis to obtain a stable perpendicular.
    let reference = if tangent[1].abs() < 0.8 {
        [0.0, 1.0, 0.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let across = [
        tangent[1] * reference[2] - tangent[2] * reference[1],
        tangent[2] * reference[0] - tangent[0] * reference[2],
        tangent[0] * reference[1] - tangent[1] * reference[0],
    ];
    let magnitude = (across[0] * across[0] + across[1] * across[1] + across[2] * across[2]).sqrt();
    let offset = across.map(|x| x / magnitude * half_width);
    let points = [
        [
            start[0] - offset[0],
            start[1] - offset[1],
            start[2] - offset[2],
        ],
        [
            start[0] + offset[0],
            start[1] + offset[1],
            start[2] + offset[2],
        ],
        [end[0] + offset[0], end[1] + offset[1], end[2] + offset[2]],
        [end[0] - offset[0], end[1] - offset[1], end[2] - offset[2]],
    ];
    let normal = [0.37139067, 0.74278134, -0.557086];
    for index in [0, 1, 2, 0, 2, 3] {
        out.push(Vertex {
            position: points[index],
            normal,
            color,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_reference_is_finite_and_contains_all_positive_axes() {
        let vertices = world_reference_geometry(-2.5, 6.0);
        assert_eq!(vertices.len() % 3, 0);
        assert!(vertices.iter().all(|v| {
            v.position
                .into_iter()
                .chain(v.normal)
                .chain(v.color)
                .all(f32::is_finite)
        }));
        assert!(
            vertices
                .iter()
                .any(|v| v.position[0] > 5.9 && v.color[0] > 0.8)
        );
        assert!(
            vertices
                .iter()
                .any(|v| v.position[1] > 5.9 && v.color[1] > 0.8)
        );
        assert!(
            vertices
                .iter()
                .any(|v| v.position[2] > 5.9 && v.color[2] > 0.9)
        );
    }

    #[test]
    fn ribbons_ignore_degenerate_segments() {
        let mut vertices = Vec::new();
        add_ribbon(&mut vertices, [1.0; 3], [1.0; 3], 0.1, [1.0; 3]);
        add_ribbon(&mut vertices, [0.0; 3], [1.0, 0.0, 0.0], f32::NAN, [1.0; 3]);
        assert!(vertices.is_empty());
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
}
