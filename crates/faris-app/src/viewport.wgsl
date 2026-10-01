struct Camera {
    eye: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    forward: vec4<f32>,
    projection: vec4<f32>,
}
@group(0) @binding(0) var<uniform> camera: Camera;

struct Surface {
    @builtin(position) position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec3<f32>,
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
    return surface;
}

@fragment
fn fragment_main(surface: Surface) -> @location(0) vec4<f32> {
    let light = normalize(vec3<f32>(0.4, 0.8, -0.6));
    let diffuse = max(dot(normalize(surface.normal), light), 0.0);
    return vec4<f32>(surface.color * (0.42 + 0.58 * diffuse), 1.0);
}
