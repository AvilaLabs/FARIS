use eframe::egui;

#[derive(Clone, Copy)]
pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            yaw: -0.65,
            pitch: 0.5,
            distance: 15.0,
        }
    }
}

impl Camera {
    pub fn basis(&self) -> ([f32; 3], [f32; 3], [f32; 3], [f32; 3]) {
        let eye = [
            self.distance * self.pitch.cos() * self.yaw.cos(),
            self.distance * self.pitch.sin(),
            self.distance * self.pitch.cos() * self.yaw.sin(),
        ];
        let forward = normalize(scale(eye, -1.0));
        let right = normalize(cross(forward, [0.0, 1.0, 0.0]));
        let up = cross(right, forward);
        (eye, right, up, forward)
    }

    pub fn ray(&self, rect: egui::Rect, point: egui::Pos2) -> ([f32; 3], [f32; 3]) {
        let (eye, right, up, forward) = self.basis();
        let x = 2.0 * (point.x - rect.left()) / rect.width() - 1.0;
        let y = 1.0 - 2.0 * (point.y - rect.top()) / rect.height();
        let tangent = (45.0_f32.to_radians() * 0.5).tan();
        let horizontal = scale(right, x * tangent * rect.width() / rect.height());
        let vertical = scale(up, y * tangent);
        (eye, normalize(add(add(forward, horizontal), vertical)))
    }
}

pub fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn subtract(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn scale(a: [f32; 3], factor: f32) -> [f32; 3] {
    [a[0] * factor, a[1] * factor, a[2] * factor]
}
pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn normalize(a: [f32; 3]) -> [f32; 3] {
    scale(a, dot(a, a).sqrt().recip())
}

pub fn triangle_hit(origin: [f32; 3], direction: [f32; 3], triangle: [[f32; 3]; 3]) -> Option<f32> {
    let edge1 = subtract(triangle[1], triangle[0]);
    let edge2 = subtract(triangle[2], triangle[0]);
    let p = cross(direction, edge2);
    let determinant = dot(edge1, p);
    if determinant.abs() < 1e-7 {
        return None;
    }
    let inverse = determinant.recip();
    let t = subtract(origin, triangle[0]);
    let u = dot(t, p) * inverse;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(t, edge1);
    let v = dot(direction, q) * inverse;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let distance = dot(edge2, q) * inverse;
    (distance > 0.0).then_some(distance)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_center_ray_reaches_the_scene_origin() {
        let camera = Camera::default();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let (origin, direction) = camera.ray(rect, rect.center());
        let endpoint = add(origin, scale(direction, camera.distance));
        assert!(dot(endpoint, endpoint).sqrt() < 1e-4);
    }

    #[test]
    fn picking_returns_distance_only_for_forward_intersections() {
        let triangle = [[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [0.0, 1.0, 0.0]];
        assert_eq!(
            triangle_hit([0.0, 0.0, 2.0], [0.0, 0.0, -1.0], triangle),
            Some(2.0)
        );
        assert!(triangle_hit([0.0, 0.0, 2.0], [0.0, 0.0, 1.0], triangle).is_none());
        assert!(triangle_hit([3.0, 0.0, 2.0], [0.0, 0.0, -1.0], triangle).is_none());
    }
}
