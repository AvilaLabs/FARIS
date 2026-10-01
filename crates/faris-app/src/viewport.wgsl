struct Camera {
    eye: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    forward: vec4<f32>,
    // aspect, tan(fov / 2), near, far
    projection: vec4<f32>,
    // flat colour flag, target is sRGB, unused, hover highlight
    display: vec4<f32>,
    // component colour (sRGB as authored)
    color: vec4<f32>,
    // scene centre xyz, grid fade radius
    scene: vec4<f32>,
    // viewport size in physical pixels
    screen: vec4<f32>,
}
@group(0) @binding(0) var<uniform> camera: Camera;

const GHOST_DESATURATION: f32 = 0.55;
const MATERIAL_DESATURATION: f32 = 0.15;

// Authored colours are sRGB. Lighting is done in linear space; the result is
// encoded for the target unless the target format already does it in hardware.
fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let v = max(c, vec3<f32>(0.0));
    return select(pow((v + 0.055) / 1.055, vec3<f32>(2.4)), v / 12.92, v <= vec3<f32>(0.04045));
}

fn encode_linear(c: vec3<f32>) -> vec3<f32> {
    if camera.display.y > 0.5 {
        return c;
    }
    let v = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(1.055 * pow(v, vec3<f32>(1.0 / 2.4)) - 0.055, v * 12.92, v <= vec3<f32>(0.0031308));
}

// A colour already expressed in the display encoding (scientific scale, grid).
fn encode_authored(c: vec3<f32>) -> vec3<f32> {
    if camera.display.y > 0.5 {
        return to_linear(c);
    }
    return c;
}

fn project(position: vec3<f32>, bias: f32) -> vec4<f32> {
    let relative = position - camera.eye.xyz;
    let depth = dot(relative, camera.forward.xyz);
    let near = camera.projection.z;
    let far = camera.projection.w;
    return vec4<f32>(
        dot(relative, camera.right.xyz) / (camera.projection.x * camera.projection.y),
        dot(relative, camera.up.xyz) / camera.projection.y,
        far * (depth - bias) / (far - near) - far * near / (far - near),
        depth
    );
}

struct Surface {
    @builtin(position) position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec3<f32>,
    @location(2) world_position: vec3<f32>,
}

@vertex
fn vertex_component(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>) -> Surface {
    var surface: Surface;
    surface.position = project(position, 0.0);
    surface.normal = normal;
    surface.color = camera.color.xyz;
    surface.world_position = position;
    return surface;
}

// Lit shading in linear space: hemispheric ambient (world up), a key light
// attached to the camera from its upper left so cut faces are always lit, a
// soft cool fill, a Fresnel rim and a mild specular.
fn shade(color: vec3<f32>, raw_normal: vec3<f32>, world: vec3<f32>, desaturation: f32, highlight: f32) -> vec4<f32> {
    var albedo = to_linear(color);
    let luminance = dot(albedo, vec3<f32>(0.2126, 0.7152, 0.0722));
    albedo = mix(albedo, vec3<f32>(luminance), desaturation);

    let view = normalize(camera.eye.xyz - world);
    var normal = normalize(raw_normal);
    if dot(normal, view) < 0.0 {
        normal = -normal;
    }
    let forward = camera.forward.xyz;
    let key_dir = normalize(-forward * 0.8 - camera.right.xyz * 0.55 + camera.up.xyz * 0.6);
    let fill_dir = normalize(-forward * 0.7 + camera.right.xyz * 0.7 - camera.up.xyz * 0.25);

    let sky = vec3<f32>(0.20, 0.26, 0.36);
    let ground = vec3<f32>(0.10, 0.075, 0.06);
    let ambient = mix(ground, sky, clamp(normal.y * 0.5 + 0.5, 0.0, 1.0));
    // Soft wrap so faces turned away from the key keep their form.
    let key = clamp((dot(normal, key_dir) + 0.3) / 1.3, 0.0, 1.0);
    let fill = clamp((dot(normal, fill_dir) + 0.2) / 1.2, 0.0, 1.0);
    let key_colour = vec3<f32>(1.0, 0.96, 0.9) * 0.82;
    let fill_colour = vec3<f32>(0.55, 0.66, 0.92) * 0.3;

    let facing = clamp(dot(normal, view), 0.0, 1.0);
    let rim = pow(1.0 - facing, 3.0) * (0.35 + 0.65 * clamp(normal.y * 0.5 + 0.5, 0.0, 1.0));
    let half_vector = normalize(key_dir + view);
    let specular = pow(max(dot(normal, half_vector), 0.0), 48.0) * 0.14 * step(0.0, dot(normal, key_dir));

    var linear = albedo * (ambient + key_colour * key + fill_colour * fill)
        + vec3<f32>(0.42, 0.58, 0.95) * rim * 0.22
        + key_colour * specular;
    linear = linear + albedo * highlight * 0.35 + vec3<f32>(highlight * 0.05);
    // Alpha used only by the ghost pass: stronger toward grazing angles.
    let alpha = 0.11 + 0.2 * pow(1.0 - facing, 2.0);
    return vec4<f32>(linear, alpha);
}

@fragment
fn fragment_solid(surface: Surface) -> @location(0) vec4<f32> {
    if camera.display.x > 0.5 {
        // Numerical color encodes the fixed scientific scale directly; lighting
        // must not change the mapped value or visually suggest a different one.
        return vec4<f32>(encode_authored(surface.color), 1.0);
    }
    let lit = shade(surface.color, surface.normal, surface.world_position, MATERIAL_DESATURATION, camera.display.w);
    return vec4<f32>(encode_linear(lit.rgb), 1.0);
}

@fragment
fn fragment_ghost(surface: Surface) -> @location(0) vec4<f32> {
    let lit = shade(surface.color, surface.normal, surface.world_position, GHOST_DESATURATION, 0.0);
    return vec4<f32>(encode_linear(lit.rgb), lit.a);
}

// Spatial field bins: six vertices per quad in the order the app emits them
// (corners 0, 2, 1, 0, 3, 2 of a -/-, +/-, +/+, -/+ quad).
struct Bin {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) uv: vec2<f32>,
}

@vertex
fn vertex_slice(
    @builtin(vertex_index) index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
) -> Bin {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0),
    );
    var bin: Bin;
    bin.position = project(position, 0.0);
    bin.color = color;
    bin.uv = corners[index % 6u];
    return bin;
}

@fragment
fn fragment_slice(bin: Bin) -> @location(0) vec4<f32> {
    let width = max(fwidth(bin.uv), vec2<f32>(1e-6));
    let edge = min(bin.uv, vec2<f32>(1.0) - bin.uv) / width;
    let distance_px = min(edge.x, edge.y);
    let cell_px = 1.0 / max(width.x, width.y);
    // Dark bin borders, kept only while bins are large enough to read.
    let border = (1.0 - smoothstep(0.4, 1.6, distance_px)) * smoothstep(5.0, 14.0, cell_px);
    let colour = mix(bin.color, vec3<f32>(0.03, 0.035, 0.045), border * 0.85);
    return vec4<f32>(encode_authored(colour), 1.0);
}

// Constant-pixel-width lines. Each segment is six vertices; every vertex
// carries its own endpoint, the opposite endpoint, colour, width in pixels and
// a kind (0 grid with distance fade, 1 axis, 2 accent outline with depth bias).
struct Line {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) world_position: vec3<f32>,
    @location(2) across: vec2<f32>,
    @location(3) @interpolate(flat) kind: f32,
}

@vertex
fn vertex_line(
    @builtin(vertex_index) index: u32,
    @location(0) position: vec3<f32>,
    @location(1) other: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) style: vec2<f32>,
) -> Line {
    var sides = array<f32, 6>(-1.0, 1.0, 1.0, -1.0, 1.0, -1.0);
    let side = sides[index % 6u];
    let bias = select(0.0, 0.04, style.y > 1.5);
    let a = project(position, bias);
    let b = project(other, bias);
    let a_w = max(a.w, 0.05);
    let b_w = max(b.w, 0.05);
    let screen = camera.screen.xy;
    let ndc_a = a.xy / a_w;
    let ndc_b = b.xy / b_w;
    var direction = (ndc_b - ndc_a) * screen * 0.5;
    let length_px = length(direction);
    if length_px > 1e-4 {
        direction = direction / length_px;
    } else {
        direction = vec2<f32>(1.0, 0.0);
    }
    let normal = vec2<f32>(-direction.y, direction.x);
    // Half width plus one pixel of antialiasing fringe.
    let half_px = style.x * 0.5 + 0.5;
    let offset = normal * side * half_px * 2.0 / screen;
    var line: Line;
    line.position = vec4<f32>((ndc_a + offset) * a.w, a.z, a.w);
    line.color = color;
    line.world_position = position;
    line.across = vec2<f32>(side * half_px, style.x * 0.5);
    line.kind = style.y;
    return line;
}

@fragment
fn fragment_line(line: Line) -> @location(0) vec4<f32> {
    let coverage = clamp(line.across.y + 0.5 - abs(line.across.x), 0.0, 1.0);
    var alpha = coverage;
    if line.kind < 0.5 {
        let offset = line.world_position.xz - camera.scene.xz;
        let fade = 1.0 - smoothstep(0.3 * camera.scene.w, camera.scene.w, length(offset));
        alpha = alpha * fade * 0.55;
    } else if line.kind < 1.5 {
        alpha = alpha * 0.7;
    }
    return vec4<f32>(encode_authored(line.color), alpha);
}

struct Backdrop {
    @builtin(position) position: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

@vertex
fn vertex_background(@builtin(vertex_index) index: u32) -> Backdrop {
    var corners = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var backdrop: Backdrop;
    backdrop.position = vec4<f32>(corners[index], 1.0, 1.0);
    backdrop.ndc = corners[index];
    return backdrop;
}

@fragment
fn fragment_background(backdrop: Backdrop) -> @location(0) vec4<f32> {
    // Dark slate at the top, slightly lighter toward the bottom, with a faint
    // central lift so the model sits in a soft pool of light.
    let t = clamp(0.5 - backdrop.ndc.y * 0.5, 0.0, 1.0);
    let top = vec3<f32>(0.075, 0.088, 0.118);
    let bottom = vec3<f32>(0.172, 0.195, 0.245);
    let glow = (1.0 - clamp(length(backdrop.ndc * vec2<f32>(0.8, 1.0)), 0.0, 1.0)) * 0.025;
    return vec4<f32>(encode_authored(mix(top, bottom, t) + vec3<f32>(glow)), 1.0);
}
