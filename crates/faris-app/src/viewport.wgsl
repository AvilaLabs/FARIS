struct Camera {
    eye: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    forward: vec4<f32>,
    projection: vec4<f32>,
    display: vec4<f32>,
    color: vec4<f32>,
}
@group(0) @binding(0) var<uniform> camera: Camera;

struct Surface {
    @builtin(position) position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec3<f32>,
    @location(2) world_position: vec3<f32>,
}

@vertex
fn vertex_main(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) color: vec3<f32>) -> Surface {
    let relative = position - camera.eye.xyz;
    let depth = dot(relative, camera.forward.xyz);
    let near = camera.projection.z;
    let far = camera.projection.w;
    var surface: Surface;
    surface.position = vec4<f32>(
        dot(relative, camera.right.xyz) / (camera.projection.x * camera.projection.y),
        dot(relative, camera.up.xyz) / camera.projection.y,
        far * depth / (far - near) - far * near / (far - near),
        depth
    );
    surface.normal = normal;
    surface.color = color;
    surface.world_position = position;
    return surface;
}

@vertex
fn component_vertex_main(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>) -> Surface {
    let relative = position - camera.eye.xyz;
    let depth = dot(relative, camera.forward.xyz);
    let near = camera.projection.z;
    let far = camera.projection.w;
    var surface: Surface;
    surface.position = vec4<f32>(
        dot(relative, camera.right.xyz) / (camera.projection.x * camera.projection.y),
        dot(relative, camera.up.xyz) / camera.projection.y,
        far * depth / (far - near) - far * near / (far - near),
        depth
    );
    surface.normal = normal;
    surface.color = camera.color.xyz;
    surface.world_position = position;
    return surface;
}

@fragment
fn fragment_main(surface: Surface) -> @location(0) vec4<f32> {
    if camera.display.x > 0.5 {
        // Numerical color encodes the fixed scientific scale directly; lighting
        // must not change the mapped value or visually suggest a different one.
        return vec4<f32>(surface.color, 1.0);
    }
    let normal = normalize(surface.normal);
    let light = normalize(vec3<f32>(0.45, 0.78, -0.43));
    let fill = normalize(vec3<f32>(-0.63, 0.31, 0.71));
    let view = normalize(camera.eye.xyz - surface.world_position);
    let diffuse = 0.32
        + 0.52 * max(dot(normal, light), 0.0)
        + 0.16 * max(dot(normal, fill), 0.0);
    let half_vector = normalize(light + view);
    let specular = 0.12 * pow(max(dot(normal, half_vector), 0.0), 28.0);
    return vec4<f32>(surface.color * diffuse + vec3<f32>(specular), 1.0);
}
