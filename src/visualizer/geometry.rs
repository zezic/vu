#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn length(self) -> f32 {
        self.x.hypot(self.y)
    }

    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y
    }

    pub fn lerp(self, other: Self, fraction: f32) -> Self {
        Self::new(
            self.x + (other.x - self.x) * fraction,
            self.y + (other.y - self.y) * fraction,
        )
    }

    pub fn normalized(self) -> Self {
        let length = self.length();
        if length > 1e-6 {
            self * (1.0 / length)
        } else {
            Self::new(0.0, 0.0)
        }
    }

    pub fn perpendicular(self) -> Self {
        Self::new(-self.y, self.x)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }

    pub fn normalized(self) -> Self {
        let length = self.length();
        if length > 1e-6 {
            self * (1.0 / length)
        } else {
            Self::new(0.0, 0.0, 0.0)
        }
    }

    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    pub fn cross(self, other: Self) -> Self {
        Self::new(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        )
    }

    pub fn rotate_x(self, angle: f32) -> Self {
        let (sin, cos) = angle.sin_cos();
        Self::new(
            self.x,
            self.y * cos - self.z * sin,
            self.y * sin + self.z * cos,
        )
    }

    pub fn rotate_y(self, angle: f32) -> Self {
        let (sin, cos) = angle.sin_cos();
        Self::new(
            self.x * cos + self.z * sin,
            self.y,
            -self.x * sin + self.z * cos,
        )
    }

    pub fn rotate_z(self, angle: f32) -> Self {
        let (sin, cos) = angle.sin_cos();
        Self::new(
            self.x * cos - self.y * sin,
            self.x * sin + self.y * cos,
            self.z,
        )
    }
}

impl std::ops::Add for Vec2 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl std::ops::Mul<f32> for Vec2 {
    type Output = Self;
    fn mul(self, rhs: f32) -> Self {
        Self::new(self.x * rhs, self.y * rhs)
    }
}

impl std::ops::Sub for Vec2 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl std::ops::Add for Vec3 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z)
    }
}

impl std::ops::Sub for Vec3 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
}

impl std::ops::Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, rhs: f32) -> Self {
        Self::new(self.x * rhs, self.y * rhs, self.z * rhs)
    }
}

#[derive(Clone)]
pub struct Wireframe {
    vertices: Vec<Vec3>,
    edges: Vec<[u16; 2]>,
    /// Mesh part that owns each vertex.
    vertex_parts: Vec<u16>,
    /// Mesh part that owns each edge.
    edge_parts: Vec<u16>,
}

impl Wireframe {
    /// Builds a single-part wireframe from an indexed mesh. Vertices must
    /// already be in model space; the caller is responsible for fitting them
    /// to the camera. The edge list may be empty when a coarse crease
    /// threshold hides every feature of a smooth model.
    #[cfg(test)]
    pub fn from_edges(vertices: Vec<Vec3>, edges: Vec<[u16; 2]>) -> Self {
        let vertex_parts = vec![0; vertices.len()];
        let edge_parts = vec![0; edges.len()];
        Self::from_parts(vertices, edges, vertex_parts, edge_parts, 1)
    }

    /// Builds a wireframe whose vertices and edges remember the glTF mesh
    /// part they came from.
    pub fn from_parts(
        vertices: Vec<Vec3>,
        edges: Vec<[u16; 2]>,
        vertex_parts: Vec<u16>,
        edge_parts: Vec<u16>,
        part_count: u16,
    ) -> Self {
        assert!(!vertices.is_empty(), "a wireframe needs a vertex");
        assert!(part_count > 0, "a wireframe needs a mesh part");
        assert!(
            vertices.len() <= u16::MAX as usize + 1,
            "wireframe vertices are indexed by u16"
        );
        assert_eq!(
            vertex_parts.len(),
            vertices.len(),
            "every wireframe vertex must belong to a mesh part"
        );
        assert_eq!(
            edge_parts.len(),
            edges.len(),
            "every wireframe edge must belong to a mesh part"
        );
        let vertex_count = vertices.len();
        assert!(
            edges.iter().all(|&[a, b]| {
                (a as usize) < vertex_count && (b as usize) < vertex_count && a != b
            }),
            "every wireframe edge must join two distinct vertices"
        );
        assert!(
            vertex_parts.iter().all(|&part| part < part_count),
            "every wireframe vertex part must exist"
        );
        assert!(
            edge_parts.iter().all(|&part| part < part_count),
            "every wireframe edge part must exist"
        );
        Self {
            vertices,
            edges,
            vertex_parts,
            edge_parts,
        }
    }

    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    pub fn edges(&self) -> &[[u16; 2]] {
        &self.edges
    }

    pub fn vertex(&self, index: usize) -> Vec3 {
        self.vertices[index]
    }

    pub fn vertex_part(&self, index: usize) -> u16 {
        self.vertex_parts[index]
    }

    pub fn edge_part(&self, index: usize) -> u16 {
        self.edge_parts[index]
    }
}

/// Identity below three quarters of the limit, then a smooth approach to it.
pub fn soft_saturate(value: f32, limit: f32) -> f32 {
    assert!(limit > 0.0);
    let knee = limit * 0.75;
    if value <= knee {
        value
    } else {
        let excess = value - knee;
        let tail = limit - knee;
        knee + tail * excess / (excess + tail)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Deformation {
    pub twist: f32,
    pub bulge: f32,
    pub ripple: f32,
    pub shear: f32,
    pub jitter: f32,
    pub phase: f32,
}

impl Default for Deformation {
    fn default() -> Self {
        Self {
            twist: 0.0,
            bulge: 0.0,
            ripple: 0.0,
            shear: 0.0,
            jitter: 0.0,
            phase: 0.0,
        }
    }
}

impl Deformation {
    pub fn apply(self, vertex: Vec3) -> Vec3 {
        let mut value = vertex.rotate_y(vertex.y * self.twist);
        let radius_wave = (self.phase * 2.3 + value.length() * 7.0).sin();
        value = value * (1.0 + self.bulge + self.ripple * radius_wave * 0.28);
        value.x += value.y * self.shear;
        if self.jitter > 0.0 {
            value = value
                + Vec3::new(
                    (self.phase * 7.1 + vertex.x * 13.0).sin(),
                    (self.phase * 5.3 + vertex.y * 11.0).cos(),
                    (self.phase * 3.7 + vertex.z * 17.0).sin(),
                ) * self.jitter;
        }
        // Keep an extreme deformation from overflowing the perspective camera
        // without introducing a velocity kink at the limit.
        let radius = value.length();
        let compressed = soft_saturate(radius, 1.6);
        if radius > 1e-6 {
            value * (compressed / radius)
        } else {
            value
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub scale: f32,
    pub distance: f32,
    /// Screen-space offset applied after projection for the quake action.
    pub shake: Vec2,
}

impl Default for Pose {
    fn default() -> Self {
        Self {
            yaw: 0.5,
            pitch: -0.24,
            roll: 0.0,
            scale: 1.0,
            distance: 4.2,
            shake: Vec2::new(0.0, 0.0),
        }
    }
}

impl Pose {
    /// Adds the flip pitch offset and the quake screen offset on top of the
    /// base pose, leaving yaw and roll to their own animation.
    pub fn with_action(self, pitch_offset: f32, shake: Vec2) -> Self {
        Self {
            pitch: self.pitch + pitch_offset,
            shake,
            ..self
        }
    }

    /// Applies the model orientation and scale without projecting.
    pub fn orient(self, vertex: Vec3) -> Vec3 {
        vertex
            .rotate_y(self.yaw)
            .rotate_x(self.pitch)
            .rotate_z(self.roll)
            * self.scale
    }

    /// The view-space y component of the model's up axis. The camera maps
    /// positive view-space Y toward the top of the screen, so positive values
    /// are upright on screen.
    #[must_use]
    pub fn up_y(self) -> f32 {
        Vec3::new(0.0, 1.0, 0.0)
            .rotate_y(self.yaw)
            .rotate_x(self.pitch)
            .rotate_z(self.roll)
            .y
    }

    /// The model's up axis measured against screen up. The projection maps
    /// positive view-space Y upward, so this is [`Pose::up_y`].
    #[must_use]
    pub fn screen_up(self) -> f32 {
        self.up_y()
    }

    /// Whether the pose tips the model's up axis below the screen horizon.
    #[must_use]
    pub fn is_upside_down(self) -> bool {
        self.screen_up() < 0.0
    }
}

/// The physical-pixel rectangle of the render target.
#[derive(Clone, Copy, Debug)]
pub struct ScreenBounds {
    extent: Vec2,
}

impl ScreenBounds {
    pub fn new(extent: Vec2) -> Self {
        Self { extent }
    }

    /// Whether the axis-aligned box spanning `min` to `max` can cover a target
    /// pixel.
    pub fn intersects(self, min: Vec2, max: Vec2) -> bool {
        max.x >= 0.0 && max.y >= 0.0 && min.x <= self.extent.x && min.y <= self.extent.y
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub center: Vec2,
    pub focal: f32,
    /// Smaller viewport extent, used to convert between focal length and FOV.
    pub minimum: f32,
    /// Render target extent in physical pixels.
    pub viewport: Vec2,
}

impl Camera {
    pub fn new(width: f32, height: f32, zoom: f32) -> Self {
        let minimum = width.min(height).max(1.0);
        Self {
            center: Vec2::new(width / 2.0, height / 2.0),
            focal: minimum * 1.28 * zoom.clamp(0.35, 3.0),
            minimum,
            viewport: Vec2::new(width, height),
        }
    }

    /// Final focal length for a vertical/horizontal reference FOV.
    pub fn focal_for_fov(self, fov: f32) -> f32 {
        assert!(
            fov.is_finite() && fov > 0.0 && fov < std::f32::consts::PI,
            "camera FOV must be finite and between zero and pi"
        );
        self.minimum * 0.5 / (fov * 0.5).tan()
    }

    /// The FOV represented by the current focal length.
    pub fn fov(self) -> f32 {
        2.0 * (self.minimum * 0.5 / self.focal).atan()
    }

    pub fn project(self, vertex: Vec3, pose: Pose) -> Projected {
        let rotated = pose.orient(vertex);
        self.project_oriented(rotated, pose.distance, pose.shake, self.focal, 0.4)
    }

    /// Projects a vertex after blending toward a POI-focused view. The target
    /// and orbit are expressed in model space and blended by `focus.blend`.
    pub fn project_focused(self, vertex: Vec3, pose: Pose, focus: FocusProjection) -> Projected {
        let posed = pose.orient(vertex);
        let target = pose.orient(focus.target) * focus.blend;
        let relative = posed - target;
        let rotated = relative
            .rotate_y(focus.rotation_yaw)
            .rotate_x(focus.rotation_pitch);
        self.project_oriented(rotated, focus.distance, pose.shake, focus.focal, focus.near)
    }

    fn project_oriented(
        self,
        vertex: Vec3,
        distance: f32,
        shake: Vec2,
        focal: f32,
        near: f32,
    ) -> Projected {
        let raw_depth = vertex.z + distance;
        let depth = raw_depth.max(near);
        let perspective = focal / depth;
        let screen = Vec2::new(
            self.center.x + vertex.x * perspective + shake.x,
            self.center.y - vertex.y * perspective + shake.y,
        );
        Projected {
            screen,
            depth,
            visible: raw_depth > near,
        }
    }
}

/// Folds an unbounded angle into the closest representation around zero.
pub fn wrap_angle(angle: f32) -> f32 {
    (angle + std::f32::consts::TAU * 0.5).rem_euclid(std::f32::consts::TAU)
        - std::f32::consts::TAU * 0.5
}

/// A partially blended POI view applied after the model pose.
#[derive(Clone, Copy, Debug)]
pub struct FocusProjection {
    /// POI center in model space.
    pub target: Vec3,
    /// Effective focus rotation in view space. Callers apply the entry or
    /// exit blend before filling these.
    pub rotation_yaw: f32,
    pub rotation_pitch: f32,
    /// Effective camera distance for the current blend.
    pub distance: f32,
    /// Effective camera focal length for the current blend.
    pub focal: f32,
    /// Effective near plane for the current blend.
    pub near: f32,
    /// Zero keeps the normal view; one uses the focused view. Values outside
    /// the interval extrapolate the view without crossing the camera through
    /// the target.
    pub blend: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Projected {
    pub screen: Vec2,
    /// View-space distance from the camera before projection.
    pub depth: f32,
    /// Whether the vertex is in front of the near plane.
    pub visible: bool,
}

/// Maps one model's view depths onto the edge opacity ramp. The closest vertex
/// is always fully opaque; `fade` is how much opacity the farthest vertex loses.
#[derive(Clone, Copy, Debug)]
pub struct DepthOpacity {
    closest: f32,
    farthest: f32,
    fade: f32,
}

impl DepthOpacity {
    /// Places the ramp on fixed planes `radius` either side of `center`.
    /// Use this when the model fits a known sphere, so the ramp stays on that
    /// sphere instead of following the silhouette of the current pose.
    pub fn centered(center: f32, radius: f32, fade: f32) -> Self {
        assert!(
            center.is_finite() && radius.is_finite() && radius >= 0.0,
            "depth opacity planes need a finite center and a non-negative radius"
        );
        Self::between(center - radius, center + radius, fade)
    }

    fn between(closest: f32, farthest: f32, fade: f32) -> Self {
        assert!(
            fade.is_finite() && (0.0..=1.0).contains(&fade),
            "depth opacity fade must be finite and between 0 and 1"
        );
        Self {
            closest,
            farthest,
            fade,
        }
    }

    #[must_use]
    pub fn factor(self, depth: f32) -> f32 {
        let span = self.farthest - self.closest;
        if span <= 1e-6 {
            1.0
        } else {
            let closeness = ((depth - self.closest) / span).clamp(0.0, 1.0);
            1.0 - self.fade * closeness
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visualizer::showcase::framed_distance;

    fn cube() -> Wireframe {
        let mut vertices = Vec::new();
        for &x in &[-1.0, 1.0] {
            for &y in &[-1.0, 1.0] {
                for &z in &[-1.0, 1.0] {
                    vertices.push(Vec3::new(x, y, z).normalized());
                }
            }
        }
        let edges = (0..vertices.len())
            .flat_map(|a| (a + 1..vertices.len()).map(move |b| [a, b]))
            .filter(|&[a, b]| {
                let a = vertices[a];
                let b = vertices[b];
                (0..3)
                    .filter(|&axis| match axis {
                        0 => a.x != b.x,
                        1 => a.y != b.y,
                        _ => a.z != b.z,
                    })
                    .count()
                    == 1
            })
            .map(|[a, b]| [a as u16, b as u16])
            .collect();
        Wireframe::from_edges(vertices, edges)
    }

    #[test]
    fn cube_projection_uses_perspective_and_stays_bounded() {
        let camera = Camera::new(1280.0, 440.0, 1.0);
        let wireframe = cube();
        let pose = Pose::default();
        let mut near = 0.0;
        let mut far = 0.0;
        for index in 0..wireframe.vertex_count() {
            let projected = camera.project(wireframe.vertex(index), pose);
            assert!(projected.screen.x.is_finite());
            assert!(projected.screen.y.is_finite());
            let radius =
                (projected.screen.x - camera.center.x).hypot(projected.screen.y - camera.center.y);
            assert!(radius < 440.0, "the model must fit the window");
            if wireframe.vertex(index).z < 0.0 {
                near += radius;
            } else {
                far += radius;
            }
        }
        assert!(near > far, "the front face projects larger than the back");
    }

    #[test]
    fn focused_projection_centers_the_poi() {
        let camera = Camera::new(1280.0, 440.0, 1.0);
        let pose = Pose::default();
        let target = Vec3::new(0.2, 0.1, -0.1);
        let focus = FocusProjection {
            target,
            rotation_yaw: 0.6,
            rotation_pitch: 0.3,
            distance: 3.0,
            focal: camera.focal_for_fov(0.7),
            near: 0.4,
            blend: 1.0,
        };
        let projected = camera.project_focused(target, pose, focus);
        assert!((projected.screen.x - camera.center.x).abs() < 1e-4);
        assert!((projected.screen.y - camera.center.y).abs() < 1e-4);
    }

    #[test]
    fn zero_focus_blend_matches_the_normal_projection() {
        let camera = Camera::new(1280.0, 440.0, 1.0);
        let pose = Pose::default();
        let vertex = Vec3::new(0.4, -0.2, 0.3);
        let focus = FocusProjection {
            target: Vec3::new(0.2, 0.1, -0.1),
            rotation_yaw: 0.0,
            rotation_pitch: 0.0,
            distance: pose.distance,
            focal: camera.focal,
            near: 0.4,
            blend: 0.0,
        };
        let normal = camera.project(vertex, pose);
        let focused = camera.project_focused(vertex, pose, focus);
        assert!((normal.screen - focused.screen).length() < 1e-4);
        assert!((normal.depth - focused.depth).abs() < 1e-4);
    }

    #[test]
    fn focused_projection_marks_vertices_behind_the_camera_invisible() {
        let camera = Camera::new(1280.0, 440.0, 1.0);
        let pose = Pose::default();
        let focus = FocusProjection {
            target: Vec3::new(0.0, 0.0, 0.0),
            rotation_yaw: 0.0,
            rotation_pitch: 0.0,
            distance: 0.1,
            focal: camera.focal_for_fov(2.0),
            near: 0.005,
            blend: 1.0,
        };
        let behind = camera.project_focused(Vec3::new(0.0, 0.0, -1.0), pose, focus);
        assert!(!behind.visible);
        let in_front = camera.project_focused(Vec3::new(0.0, 0.0, 0.02), pose, focus);
        assert!(in_front.visible);
        assert!(
            in_front.depth < 0.4,
            "close focus uses a near plane below the normal one"
        );
    }

    #[test]
    fn camera_fov_round_trips_and_dolly_distance_shrinks_with_wide_fov() {
        let camera = Camera::new(1280.0, 440.0, 1.0);
        for fov in [
            30.0_f32.to_radians(),
            60.0_f32.to_radians(),
            120.0_f32.to_radians(),
        ] {
            let focal = camera.focal_for_fov(fov);
            let described = Camera { focal, ..camera };
            assert!((described.fov() - fov).abs() < 1e-4);
        }
        let narrow = framed_distance(1.0, 30.0_f32.to_radians());
        let wide = framed_distance(1.0, 120.0_f32.to_radians());
        assert!(narrow > wide, "wide FOV uses a closer camera");
    }

    #[test]
    fn action_pose_offsets_pitch_and_leaves_yaw_alone() {
        let pose = Pose::default();
        let action = pose.with_action(1.2, Vec2::new(3.0, -4.0));
        assert_eq!(action.yaw, pose.yaw);
        assert!((action.pitch - pose.pitch - 1.2).abs() < 1e-6);
        assert_eq!(action.shake, Vec2::new(3.0, -4.0));
    }

    #[test]
    fn upside_down_follows_the_screen_up_axis() {
        use std::f32::consts::{FRAC_PI_2, PI};
        assert!(!Pose::default().is_upside_down());
        assert!(Pose {
            pitch: PI,
            ..Pose::default()
        }
        .is_upside_down());
        assert!(Pose {
            roll: PI,
            ..Pose::default()
        }
        .is_upside_down());
        assert!(
            !Pose {
                pitch: PI,
                roll: PI,
                ..Pose::default()
            }
            .is_upside_down(),
            "a half turn around both axes leaves the up axis up"
        );
        assert!(
            !Pose {
                pitch: FRAC_PI_2 - 0.1,
                ..Pose::default()
            }
            .is_upside_down(),
            "a tilt above the horizon is not upside-down"
        );
        assert!(Pose {
            pitch: FRAC_PI_2 + 0.1,
            ..Pose::default()
        }
        .is_upside_down());
        assert!(Pose::default()
            .with_action(PI, Vec2::new(0.0, 0.0))
            .is_upside_down());
    }

    #[test]
    fn centered_depth_opacity_uses_fixed_planes() {
        let ramp = DepthOpacity::centered(4.2, 1.0, 0.5);
        assert!((ramp.factor(3.2) - 1.0).abs() < 1e-6);
        assert!((ramp.factor(4.2) - 0.75).abs() < 1e-6);
        assert!((ramp.factor(5.2) - 0.5).abs() < 1e-6);
        assert!((ramp.factor(2.0) - 1.0).abs() < 1e-6);
        assert!((ramp.factor(9.0) - 0.5).abs() < 1e-6);

        let flat = DepthOpacity::centered(4.2, 0.0, 0.5);
        assert!((flat.factor(4.2) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn deformation_stays_finite_and_bounded() {
        let wireframe = cube();
        let deformation = Deformation {
            twist: 1.3,
            bulge: 0.4,
            ripple: 0.5,
            shear: 0.7,
            jitter: 0.2,
            phase: 12.0,
        };
        for index in 0..wireframe.vertex_count() {
            let vertex = deformation.apply(wireframe.vertex(index));
            assert!(vertex.x.is_finite() && vertex.y.is_finite() && vertex.z.is_finite());
            assert!(vertex.length() <= 1.6);
        }
    }

    #[test]
    fn wireframe_validates_edges_against_vertices() {
        assert!(std::panic::catch_unwind(|| {
            Wireframe::from_edges(vec![Vec3::new(0.0, 0.0, 0.0)], vec![[0, 1]])
        })
        .is_err());
    }
}
