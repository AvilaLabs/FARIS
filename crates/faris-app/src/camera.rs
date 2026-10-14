use eframe::egui;

pub const VERTICAL_FOV_RADIANS: f32 = 45.0_f32.to_radians();

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub target: [f32; 3],
}

/// First view of a scenario: looking into the cutaway quadrant from the
/// outboard side, so the poloidal cross-section of the layered shells and the
/// outboard port on the midplane are both in frame without moving the camera.
impl Default for Camera {
    fn default() -> Self {
        Self {
            yaw: -1.0,
            pitch: 0.3,
            distance: 13.0,
            target: [2.2, 0.0, -0.6],
        }
    }
}

/// A key press moves the camera as a pointer drag of this many points would.
const KEY_STEP_POINTS: f32 = 12.5;
/// Distance factor for one zoom-out key press.
const KEY_ZOOM_FACTOR: f32 = 1.1;

impl Camera {
    pub fn basis(&self) -> ([f32; 3], [f32; 3], [f32; 3], [f32; 3]) {
        let yaw = if self.yaw.is_finite() {
            self.yaw
        } else {
            -0.65
        };
        let pitch = if self.pitch.is_finite() {
            self.pitch.clamp(-1.35, 1.35)
        } else {
            0.5
        };
        let distance = if self.distance.is_finite() {
            self.distance.clamp(0.25, 100.0)
        } else {
            15.0
        };
        let offset = [
            distance * pitch.cos() * yaw.cos(),
            distance * pitch.sin(),
            distance * pitch.cos() * yaw.sin(),
        ];
        let target = self.target.map(|x| if x.is_finite() { x } else { 0.0 });
        let eye = add(target, offset);
        let forward = normalize(scale(offset, -1.0));
        let right = normalize(cross(forward, [0.0, 1.0, 0.0]));
        let up = cross(right, forward);
        (eye, right, up, forward)
    }

    /// One easing step of the displayed camera toward `goal` by `alpha` in
    /// (0, 1]. Returns the new camera and whether it has settled on the goal.
    pub fn eased_toward(&self, goal: &Camera, alpha: f32) -> (Camera, bool) {
        let finite = |camera: &Camera| {
            camera.yaw.is_finite()
                && camera.pitch.is_finite()
                && camera.distance.is_finite()
                && camera.target.iter().all(|x| x.is_finite())
        };
        if !finite(self) || !finite(goal) {
            return (*goal, true);
        }
        let alpha = alpha.clamp(0.0, 1.0);
        let step = |from: f32, to: f32| from + (to - from) * alpha;
        let next = Camera {
            yaw: step(self.yaw, goal.yaw),
            pitch: step(self.pitch, goal.pitch),
            distance: step(self.distance, goal.distance),
            target: std::array::from_fn(|axis| step(self.target[axis], goal.target[axis])),
        };
        let scale = goal.distance.max(1.0);
        let settled = (next.yaw - goal.yaw).abs() < 5e-4
            && (next.pitch - goal.pitch).abs() < 5e-4
            && (next.distance - goal.distance).abs() < 5e-4 * scale
            && (0..3).all(|axis| (next.target[axis] - goal.target[axis]).abs() < 5e-4 * scale);
        if settled {
            (*goal, true)
        } else {
            (next, false)
        }
    }

    /// Pointer-drag motion: orbit, or pan when `pan` is set. Keyboard steps
    /// use the same arithmetic through `nudge`.
    pub fn drag(&mut self, delta: egui::Vec2, pan: bool) {
        if pan {
            let (_, right, up, _) = self.basis();
            let per_point = self.distance * 0.002;
            self.target = add(
                self.target,
                add(
                    scale(right, -delta.x * per_point),
                    scale(up, delta.y * per_point),
                ),
            );
        } else {
            self.yaw -= delta.x * 0.008;
            self.pitch = (self.pitch + delta.y * 0.008).clamp(-1.35, 1.35);
        }
    }

    /// Multiply the distance to the target, inside the allowed range.
    pub fn zoom(&mut self, factor: f32) {
        self.distance = (self.distance * factor).clamp(0.25, 100.0);
    }

    /// One keyboard step: arrows orbit (pan with `pan`), plus and minus zoom.
    /// Returns whether the key means anything to the camera.
    pub fn nudge(&mut self, key: egui::Key, pan: bool) -> bool {
        let step = KEY_STEP_POINTS;
        match key {
            egui::Key::ArrowLeft => self.drag(egui::vec2(-step, 0.0), pan),
            egui::Key::ArrowRight => self.drag(egui::vec2(step, 0.0), pan),
            egui::Key::ArrowUp => self.drag(egui::vec2(0.0, -step), pan),
            egui::Key::ArrowDown => self.drag(egui::vec2(0.0, step), pan),
            egui::Key::Plus | egui::Key::Equals => self.zoom(KEY_ZOOM_FACTOR.recip()),
            egui::Key::Minus => self.zoom(KEY_ZOOM_FACTOR),
            _ => return false,
        }
        true
    }

    pub fn frame_bounds(&mut self, minimum: [f32; 3], maximum: [f32; 3]) {
        self.target = std::array::from_fn(|axis| (minimum[axis] + maximum[axis]) * 0.5);
        let extent = subtract(maximum, minimum);
        let radius = dot(extent, extent).sqrt() * 0.5;
        self.distance = (radius / (VERTICAL_FOV_RADIANS * 0.5).sin() * 1.15).clamp(0.25, 100.0);
    }

    pub fn ray(&self, rect: egui::Rect, point: egui::Pos2) -> ([f32; 3], [f32; 3]) {
        let (eye, right, up, forward) = self.basis();
        let width = rect.width().max(f32::EPSILON);
        let height = rect.height().max(f32::EPSILON);
        let x = 2.0 * (point.x - rect.left()) / width - 1.0;
        let y = 1.0 - 2.0 * (point.y - rect.top()) / height;
        let tangent = (VERTICAL_FOV_RADIANS * 0.5).tan();
        let horizontal = scale(right, x * tangent * width / height);
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
    fn camera_center_ray_reaches_the_orbit_target() {
        let camera = Camera::default();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let (origin, direction) = camera.ray(rect, rect.center());
        let endpoint = add(origin, scale(direction, camera.distance));
        let error = subtract(endpoint, camera.target);
        assert!(dot(error, error).sqrt() < 1e-4);
    }

    // Verifies: A11Y-001
    #[test]
    fn arrow_keys_move_the_camera_like_a_drag_and_plus_minus_zoom() {
        let mut keyed = Camera::default();
        let mut dragged = Camera::default();
        assert!(keyed.nudge(egui::Key::ArrowRight, false));
        dragged.drag(egui::vec2(KEY_STEP_POINTS, 0.0), false);
        assert_eq!(keyed, dragged);
        assert!(keyed.yaw < Camera::default().yaw);
        let mut panned = Camera::default();
        assert!(panned.nudge(egui::Key::ArrowUp, true));
        assert_ne!(panned.target, Camera::default().target);
        assert_eq!(panned.yaw, Camera::default().yaw);
        let mut zoomed = Camera::default();
        zoomed.nudge(egui::Key::Plus, false);
        assert!(zoomed.distance < Camera::default().distance);
        zoomed.nudge(egui::Key::Minus, false);
        assert!((zoomed.distance - Camera::default().distance).abs() < 1e-4);
        assert!(!zoomed.nudge(egui::Key::A, false));
        // The ends of the range hold.
        let mut limit = Camera::default();
        for _ in 0..200 {
            limit.nudge(egui::Key::ArrowUp, false);
            limit.nudge(egui::Key::Plus, false);
        }
        assert!(limit.pitch >= -1.35 && limit.distance >= 0.25);
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

    #[test]
    fn eased_camera_converges_monotonically_and_snaps_when_settled() {
        let goal = Camera {
            yaw: 2.0,
            pitch: -0.4,
            distance: 5.0,
            target: [1.0, 2.0, 3.0],
        };
        let mut shown = Camera::default();
        let mut previous_gap = f32::INFINITY;
        for _ in 0..200 {
            let (next, settled) = shown.eased_toward(&goal, 0.2);
            let gap = (next.yaw - goal.yaw).abs();
            assert!(gap <= previous_gap);
            previous_gap = gap;
            shown = next;
            if settled {
                break;
            }
        }
        assert_eq!(shown, goal);
        let (snapped, settled) = shown.eased_toward(
            &Camera {
                yaw: f32::NAN,
                ..goal
            },
            0.2,
        );
        assert!(settled && snapped.yaw.is_nan());
    }

    #[test]
    fn default_view_looks_into_the_cutaway_quadrant_toward_the_outboard_port() {
        let camera = Camera::default();
        let (eye, _, _, forward) = camera.basis();
        // The cutaway removes the +X, -Z toroidal quadrant; the eye sits in it.
        assert!(eye[0] > 0.0 && eye[2] < 0.0);
        // The outboard port (x 4.3..5.6, y = z = 0) lies in front of the camera.
        let to_port = subtract([4.9, 0.0, 0.0], eye);
        assert!(dot(to_port, forward) > 0.0);
    }

    // Verifies: VIS-002
    #[test]
    fn camera_basis_stays_orthonormal_at_extreme_inputs() {
        let camera = Camera {
            yaw: f32::INFINITY,
            pitch: 90.0,
            distance: f32::NAN,
            target: [f32::NAN; 3],
        };
        let (_, right, up, forward) = camera.basis();
        for vector in [right, up, forward] {
            assert!(vector.iter().all(|x| x.is_finite()));
            assert!((dot(vector, vector) - 1.0).abs() < 1e-5);
        }
        assert!(dot(right, up).abs() < 1e-5);
        assert!(dot(right, forward).abs() < 1e-5);
        assert!(dot(up, forward).abs() < 1e-5);
    }

    #[test]
    fn framing_a_local_field_keeps_render_and_pick_rays_on_its_center() {
        let mut camera = Camera::default();
        camera.frame_bounds([4.0, -0.45, -0.45], [5.58, 0.45, 0.45]);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let (origin, direction) = camera.ray(rect, rect.center());
        let endpoint = add(origin, scale(direction, camera.distance));
        let error = subtract(endpoint, camera.target);
        assert!(dot(error, error).sqrt() < 1e-4);
        assert!(camera.distance < 6.0);
    }
}
