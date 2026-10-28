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
}

impl Wireframe {
    /// Builds a wireframe from an indexed mesh. Vertices must already be in
    /// model space; the caller is responsible for fitting them to the camera.
    /// The edge list may be empty when a coarse crease threshold hides every
    /// feature of a smooth model.
    pub fn from_edges(vertices: Vec<Vec3>, edges: Vec<[u16; 2]>) -> Self {
        assert!(!vertices.is_empty(), "a wireframe needs a vertex");
        assert!(
            vertices.len() <= u16::MAX as usize + 1,
            "wireframe vertices are indexed by u16"
        );
        let vertex_count = vertices.len();
        assert!(
            edges.iter().all(|&[a, b]| {
                (a as usize) < vertex_count && (b as usize) < vertex_count && a != b
            }),
            "every wireframe edge must join two distinct vertices"
        );
        Self { vertices, edges }
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
}

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub center: Vec2,
    pub focal: f32,
}

impl Camera {
    pub fn new(width: f32, height: f32, zoom: f32) -> Self {
        let minimum = width.min(height);
        Self {
            center: Vec2::new(width / 2.0, height / 2.0),
            focal: minimum.max(1.0) * 1.28 * zoom.clamp(0.35, 3.0),
        }
    }

    pub fn project(self, vertex: Vec3, pose: Pose) -> Projected {
        let rotated = vertex
            .rotate_y(pose.yaw)
            .rotate_x(pose.pitch)
            .rotate_z(pose.roll)
            * pose.scale;
        let depth = (rotated.z + pose.distance).max(0.4);
        let perspective = self.focal / depth;
        let screen = Vec2::new(
            self.center.x + rotated.x * perspective + pose.shake.x,
            self.center.y + rotated.y * perspective + pose.shake.y,
        );
        Projected { screen, depth }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Projected {
    pub screen: Vec2,
    /// View-space distance from the camera before projection.
    pub depth: f32,
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
    fn action_pose_offsets_pitch_and_leaves_yaw_alone() {
        let pose = Pose::default();
        let action = pose.with_action(1.2, Vec2::new(3.0, -4.0));
        assert_eq!(action.yaw, pose.yaw);
        assert!((action.pitch - pose.pitch - 1.2).abs() < 1e-6);
        assert_eq!(action.shake, Vec2::new(3.0, -4.0));
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
