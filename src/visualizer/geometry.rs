use std::f32::consts::TAU;

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

/// Curated evolution path. The first scene is always the plain cube.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FigureKind {
    #[default]
    Cube,
    Octahedron,
    Icosahedron,
    Tesseract,
    Torus,
}

impl FigureKind {
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Cube => Self::Octahedron,
            Self::Octahedron => Self::Icosahedron,
            Self::Icosahedron => Self::Tesseract,
            Self::Tesseract => Self::Torus,
            Self::Torus => Self::Cube,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Cube => "Cube",
            Self::Octahedron => "Octahedron",
            Self::Icosahedron => "Icosahedron",
            Self::Tesseract => "Tesseract",
            Self::Torus => "Torus",
        }
    }

    /// Complex figures use two vivid accents; simple figures use one.
    pub fn is_complex(self) -> bool {
        matches!(self, Self::Tesseract | Self::Torus)
    }
}

enum Source {
    Three(Vec<Vec3>),
    Four(Vec<[f32; 4]>),
}

pub struct Wireframe {
    source: Source,
    edges: Vec<[u16; 2]>,
    vertex_count: usize,
}

impl Wireframe {
    pub fn build(kind: FigureKind) -> Self {
        match kind {
            FigureKind::Cube => cube(),
            FigureKind::Octahedron => octahedron(),
            FigureKind::Icosahedron => icosahedron(),
            FigureKind::Tesseract => tesseract(),
            FigureKind::Torus => torus(),
        }
    }

    pub fn vertex_count(&self) -> usize {
        self.vertex_count
    }

    pub fn edges(&self) -> &[[u16; 2]] {
        &self.edges
    }

    /// A 4D tesseract collapses to 3D with a phase-driven four-space rotation.
    pub fn vertex(&self, index: usize, phase: f32) -> Vec3 {
        match &self.source {
            Source::Three(vertices) => vertices[index],
            Source::Four(vertices) => {
                let vertex = vertices[index];
                let (sin_xw, cos_xw) = (phase * 0.9).sin_cos();
                let (sin_yz, cos_yz) = (phase * 0.63).sin_cos();
                let x = vertex[0] * cos_xw - vertex[3] * sin_xw;
                let w = vertex[0] * sin_xw + vertex[3] * cos_xw;
                let y = vertex[1] * cos_yz - vertex[2] * sin_yz;
                let z = vertex[1] * sin_yz + vertex[2] * cos_yz;
                let perspective = 0.8 / (2.4 - w).max(0.6);
                Vec3::new(x, y, z) * perspective
            }
        }
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

fn normalized(mut vertices: Vec<Vec3>) -> Vec<Vec3> {
    let radius = vertices
        .iter()
        .map(|vertex| vertex.length())
        .fold(0.0_f32, f32::max)
        .max(1e-6);
    for vertex in &mut vertices {
        *vertex = *vertex * (1.0 / radius);
    }
    vertices
}

/// Keep only pairs at the nearest vertex distance, which is the wireframe edge.
fn connected_edges(vertices: &[Vec3], tolerance: f32) -> Vec<[u16; 2]> {
    let nearest = vertices
        .iter()
        .enumerate()
        .flat_map(|(a, &first)| {
            vertices[a + 1..]
                .iter()
                .map(move |&second| (second - first).length())
        })
        .filter(|distance| *distance > 1e-5)
        .fold(f32::INFINITY, f32::min);
    (0..vertices.len())
        .flat_map(|a| (a + 1..vertices.len()).map(move |b| [a, b]))
        .filter(|&[a, b]| (vertices[a] - vertices[b]).length() <= nearest * tolerance)
        .map(|[a, b]| [a as u16, b as u16])
        .collect()
}

fn cube() -> Wireframe {
    let vertices = normalized(
        [-1.0, 1.0]
            .into_iter()
            .flat_map(|x| {
                [-1.0, 1.0]
                    .into_iter()
                    .flat_map(move |y| [-1.0, 1.0].into_iter().map(move |z| Vec3::new(x, y, z)))
            })
            .collect(),
    );
    let edges = connected_edges(&vertices, 1.1);
    Wireframe {
        source: Source::Three(vertices),
        edges,
        vertex_count: 8,
    }
}

fn octahedron() -> Wireframe {
    let vertices = vec![
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(-1.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.0, -1.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::new(0.0, 0.0, -1.0),
    ];
    let edges = connected_edges(&vertices, 1.05);
    Wireframe {
        source: Source::Three(vertices),
        edges,
        vertex_count: 6,
    }
}

fn icosahedron() -> Wireframe {
    let golden = (1.0 + 5.0_f32.sqrt()) / 2.0;
    let mut vertices = Vec::with_capacity(12);
    for &a in &[-1.0, 1.0] {
        for &b in &[-golden, golden] {
            vertices.push(Vec3::new(0.0, a, b));
            vertices.push(Vec3::new(a, b, 0.0));
            vertices.push(Vec3::new(b, 0.0, a));
        }
    }
    let vertices = normalized(vertices);
    let edges = connected_edges(&vertices, 1.1);
    Wireframe {
        source: Source::Three(vertices),
        edges,
        vertex_count: 12,
    }
}

fn tesseract() -> Wireframe {
    let mut vertices = Vec::with_capacity(16);
    for &x in &[-1.0, 1.0] {
        for &y in &[-1.0, 1.0] {
            for &z in &[-1.0, 1.0] {
                for &w in &[-1.0, 1.0] {
                    vertices.push([x, y, z, w]);
                }
            }
        }
    }
    let edges = (0..16)
        .flat_map(|a| (a + 1..16).map(move |b| [a, b]))
        .filter(|&[a, b]| {
            let a = vertices[a as usize];
            let b = vertices[b as usize];
            (0..4).filter(|&axis| a[axis] != b[axis]).count() == 1
        })
        .map(|[a, b]| [a as u16, b as u16])
        .collect();
    Wireframe {
        source: Source::Four(vertices),
        edges,
        vertex_count: 16,
    }
}

fn torus() -> Wireframe {
    const RINGS: usize = 12;
    const TUBES: usize = 8;
    let major = 0.72;
    let minor = 0.28;
    let mut vertices = Vec::with_capacity(RINGS * TUBES);
    for ring in 0..RINGS {
        let around = TAU * ring as f32 / RINGS as f32;
        for tube in 0..TUBES {
            let across = TAU * tube as f32 / TUBES as f32;
            let radius = major + minor * across.cos();
            vertices.push(Vec3::new(
                radius * around.cos(),
                minor * across.sin(),
                radius * around.sin(),
            ));
        }
    }
    let index = |ring: usize, tube: usize| ((ring % RINGS) * TUBES + tube % TUBES) as u16;
    let mut edges = Vec::with_capacity(RINGS * TUBES * 2);
    for ring in 0..RINGS {
        for tube in 0..TUBES {
            edges.push([index(ring, tube), index(ring + 1, tube)]);
            edges.push([index(ring, tube), index(ring, tube + 1)]);
        }
    }
    Wireframe {
        source: Source::Three(vertices),
        edges,
        vertex_count: RINGS * TUBES,
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
}

impl Default for Pose {
    fn default() -> Self {
        Self {
            yaw: 0.5,
            pitch: -0.24,
            roll: 0.0,
            scale: 1.0,
            distance: 4.2,
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
        let z = (rotated.z + pose.distance).max(0.4);
        let perspective = self.focal / z;
        let screen = Vec2::new(
            self.center.x + rotated.x * perspective,
            self.center.y + rotated.y * perspective,
        );
        Projected { screen }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Projected {
    pub screen: Vec2,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_evolution_step_has_a_connected_wireframe() {
        let mut kind = FigureKind::default();
        for _ in 0..5 {
            let wireframe = Wireframe::build(kind);
            let expected_edges = match kind {
                FigureKind::Cube => 12,
                FigureKind::Octahedron => 12,
                FigureKind::Icosahedron => 30,
                FigureKind::Tesseract => 32,
                FigureKind::Torus => 192,
            };
            assert!(wireframe.vertex_count() >= 6, "{}", kind.name());
            assert_eq!(wireframe.edges().len(), expected_edges, "{}", kind.name());
            for edge in wireframe.edges() {
                assert!((edge[0] as usize) < wireframe.vertex_count());
                assert!((edge[1] as usize) < wireframe.vertex_count());
                assert_ne!(edge[0], edge[1]);
            }
            kind = kind.next();
        }
        assert_eq!(kind, FigureKind::Cube);
        assert!(!FigureKind::Cube.is_complex());
        assert!(!FigureKind::Octahedron.is_complex());
        assert!(!FigureKind::Icosahedron.is_complex());
        assert!(FigureKind::Tesseract.is_complex());
        assert!(FigureKind::Torus.is_complex());
    }

    #[test]
    fn cube_projection_uses_perspective_and_stays_bounded() {
        let camera = Camera::new(1280.0, 440.0, 1.0);
        let wireframe = Wireframe::build(FigureKind::Cube);
        let pose = Pose::default();
        let mut near = 0.0;
        let mut far = 0.0;
        for index in 0..wireframe.vertex_count() {
            let projected = camera.project(wireframe.vertex(index, 0.0), pose);
            assert!(projected.screen.x.is_finite());
            assert!(projected.screen.y.is_finite());
            let radius =
                (projected.screen.x - camera.center.x).hypot(projected.screen.y - camera.center.y);
            assert!(radius < 440.0, "the figure must fit the window");
            let z = wireframe.vertex(index, 0.0).z;
            if z < 0.0 {
                near += radius;
            } else {
                far += radius;
            }
        }
        assert!(near > far, "the front face projects larger than the back");
    }

    #[test]
    fn every_figure_fits_the_viewport_at_its_default_pose() {
        let camera = Camera::new(1280.0, 440.0, 1.0);
        let pose = Pose::default();
        for kind in [
            FigureKind::Cube,
            FigureKind::Octahedron,
            FigureKind::Icosahedron,
            FigureKind::Tesseract,
            FigureKind::Torus,
        ] {
            let wireframe = Wireframe::build(kind);
            for index in 0..wireframe.vertex_count() {
                let projected = camera.project(wireframe.vertex(index, 1.7), pose);
                let dx = (projected.screen.x - camera.center.x).abs();
                let dy = (projected.screen.y - camera.center.y).abs();
                assert!(
                    dx < 420.0 && dy < 200.0,
                    "{} vertex {index} projects at {dx}x{dy}",
                    kind.name()
                );
            }
        }
    }

    #[test]
    fn deformation_and_four_space_projection_stay_finite() {
        let wireframe = Wireframe::build(FigureKind::Tesseract);
        let deformation = Deformation {
            twist: 1.3,
            bulge: 0.4,
            ripple: 0.5,
            shear: 0.7,
            jitter: 0.2,
            phase: 12.0,
        };
        for index in 0..wireframe.vertex_count() {
            let vertex = deformation.apply(wireframe.vertex(index, 4.2));
            assert!(vertex.x.is_finite() && vertex.y.is_finite() && vertex.z.is_finite());
            assert!(vertex.length() <= 1.6);
        }
    }
}
