//! Loads the binary glTF mesh backing the weapon showcase.

use std::{
    collections::{btree_map::Entry, BTreeMap},
    fs,
    path::Path,
};

use anyhow::{ensure, Context};

use super::{
    geometry::{Vec3, Wireframe},
    CreaseThreshold,
};

/// Directory scanned for the showcase model. Binary glTF keeps the geometry
/// and any external resources out of the crate's compiled assets.
const MODEL_DIRECTORY: &str = "assets/models";

/// A loaded model normalized to the same radius as the procedural figures.
pub struct Model {
    pub name: String,
    vertices: Vec<Vec3>,
    edges: Vec<ModelEdge>,
}

impl Model {
    fn load_file(path: &Path) -> anyhow::Result<Self> {
        let gltf = gltf::Gltf::open(path)
            .with_context(|| format!("could not open the model at {}", path.display()))?;
        let blob = gltf
            .blob
            .as_deref()
            .context("the showcase model must be a binary glTF (.glb)")?;
        ensure!(
            gltf.buffers()
                .all(|buffer| matches!(buffer.source(), gltf::buffer::Source::Bin)),
            "the showcase model must embed its buffers in the .glb"
        );
        let get_buffer = |buffer: gltf::Buffer<'_>| match buffer.source() {
            gltf::buffer::Source::Bin => Some(blob),
            gltf::buffer::Source::Uri(_) => None,
        };
        let scene = gltf
            .default_scene()
            .or_else(|| gltf.scenes().next())
            .context("the model has no scene to load")?;
        let mut builder = Builder::default();
        let mut stack: Vec<_> = scene
            .nodes()
            .map(|node| (node, Transform::IDENTITY))
            .collect();
        while let Some((node, parent)) = stack.pop() {
            let transform = parent.then(Transform::from_gltf(node.transform().matrix()));
            if let Some(mesh) = node.mesh() {
                builder.push_mesh(&mesh, transform, &get_buffer)?;
            }
            for child in node.children() {
                stack.push((child, transform));
            }
        }
        let (vertices, edges) = builder.finish()?;
        let model = Self {
            name: display_name(path),
            vertices,
            edges,
        };
        ensure!(
            model.edges_at(CreaseThreshold::MIN).next().is_some(),
            "the model has no feature edges at the {} degree minimum crease threshold",
            CreaseThreshold::MIN_DEGREES
        );
        Ok(model)
    }

    /// Builds the wireframe for one crease angle. The model's vertices are
    /// already normalized, so changing the threshold keeps the framing.
    pub fn wireframe(&self, crease: CreaseThreshold) -> Wireframe {
        Wireframe::from_edges(self.vertices.clone(), self.edges_at(crease).collect())
    }

    fn edges_at(&self, crease: CreaseThreshold) -> impl Iterator<Item = [u16; 2]> + '_ {
        let cosine_limit = crease.degrees().to_radians().cos();
        self.edges
            .iter()
            .filter(move |edge| edge.kind.visible_at(cosine_limit))
            .map(|edge| edge.points)
    }
}

/// Every bundled model in display order. Drops and the next-model hotkey step
/// through this list and wrap around.
pub struct ModelSet {
    models: Vec<Model>,
    current: usize,
}

impl ModelSet {
    pub fn load() -> anyhow::Result<Self> {
        let paths = model_paths()?;
        ensure!(
            !paths.is_empty(),
            "no binary glTF models in {MODEL_DIRECTORY}; add a .glb file there"
        );
        let models = paths
            .iter()
            .map(|path| {
                Model::load_file(path).with_context(|| format!("could not load {}", path.display()))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Self { models, current: 0 })
    }

    pub fn current(&self) -> &Model {
        &self.models[self.current]
    }

    /// Advances to the next model. Returns false when there is nothing to
    /// advance to, so a one-model showcase stays put.
    pub fn advance(&mut self) -> bool {
        if self.models.len() < 2 {
            return false;
        }
        self.current = (self.current + 1) % self.models.len();
        true
    }

    /// One-based position and total count for the HUD.
    pub fn position(&self) -> (usize, usize) {
        (self.current + 1, self.models.len())
    }
}

/// One unique geometric edge and how sharply its two faces meet.
#[derive(Clone, Copy)]
struct ModelEdge {
    points: [u16; 2],
    kind: EdgeKind,
}

#[derive(Clone, Copy)]
enum EdgeKind {
    /// Boundary or non-manifold edges stay at every threshold.
    Always,
    /// Absolute cosine of the angle between the two adjacent face normals.
    Crease(f32),
}

impl EdgeKind {
    fn visible_at(self, cosine_limit: f32) -> bool {
        match self {
            Self::Always => true,
            Self::Crease(cosine) => cosine < cosine_limit,
        }
    }
}

fn model_paths() -> anyhow::Result<Vec<std::path::PathBuf>> {
    let directory = fs::read_dir(MODEL_DIRECTORY)
        .with_context(|| format!("could not read the model directory {MODEL_DIRECTORY}"))?;
    let mut paths = Vec::new();
    for entry in directory {
        let entry = entry.with_context(|| format!("could not read {MODEL_DIRECTORY}"))?;
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("glb"))
        {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

fn display_name(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Model");
    stem.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Column-major 4x4 transform from a glTF node or one of its parents.
#[derive(Clone, Copy, Debug)]
struct Transform([[f32; 4]; 4]);

impl Transform {
    const IDENTITY: Self = Self([
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]);

    fn from_gltf(matrix: [[f32; 4]; 4]) -> Self {
        Self(matrix)
    }

    /// Applies this transform to a position in homogeneous coordinates.
    fn apply(self, vertex: [f32; 3]) -> Vec3 {
        let matrix = self.0;
        Vec3::new(
            matrix[0][0] * vertex[0]
                + matrix[1][0] * vertex[1]
                + matrix[2][0] * vertex[2]
                + matrix[3][0],
            matrix[0][1] * vertex[0]
                + matrix[1][1] * vertex[1]
                + matrix[2][1] * vertex[2]
                + matrix[3][1],
            matrix[0][2] * vertex[0]
                + matrix[1][2] * vertex[1]
                + matrix[2][2] * vertex[2]
                + matrix[3][2],
        )
    }

    /// Composes the parent transform with a child transform.
    fn then(self, child: Self) -> Self {
        let mut result = [[0.0; 4]; 4];
        for (column, result_column) in result.iter_mut().enumerate() {
            for (row, value) in result_column.iter_mut().enumerate() {
                *value = (0..4)
                    .map(|axis| self.0[axis][row] * child.0[column][axis])
                    .sum();
            }
        }
        Self(result)
    }
}

#[derive(Default)]
struct Builder {
    vertices: Vec<Vec3>,
    triangles: Vec<[u32; 3]>,
}

impl Builder {
    fn push_mesh<'s, F>(
        &mut self,
        mesh: &gltf::Mesh<'_>,
        transform: Transform,
        get_buffer: &F,
    ) -> anyhow::Result<()>
    where
        F: for<'a> Fn(gltf::Buffer<'a>) -> Option<&'s [u8]> + Clone,
    {
        for primitive in mesh.primitives() {
            ensure!(
                primitive.mode() == gltf::mesh::Mode::Triangles,
                "the showcase wireframe only supports triangle primitives"
            );
            let reader = primitive.reader(get_buffer);
            let positions: Vec<[f32; 3]> = reader
                .read_positions()
                .context("the model primitive has no positions")?
                .collect();
            ensure!(!positions.is_empty(), "the model primitive has no vertices");
            ensure!(
                positions.iter().flatten().all(|value| value.is_finite()),
                "the model contains a non-finite vertex position"
            );
            let base = u32::try_from(self.vertices.len())
                .context("the model has more vertices than an index can address")?;
            self.vertices
                .extend(positions.iter().map(|&position| transform.apply(position)));
            let indices: Vec<u32> = match reader.read_indices() {
                Some(indices) => indices.into_u32().collect(),
                None => {
                    (0..u32::try_from(positions.len()).expect("vertex counts fit u32")).collect()
                }
            };
            ensure!(
                indices.len().is_multiple_of(3),
                "the model primitive's index count is not a multiple of three"
            );
            ensure!(
                indices
                    .iter()
                    .all(|&index| (index as usize) < positions.len()),
                "the model primitive has an index past its vertex count"
            );
            self.triangles.extend(
                indices
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|triangle| [base + triangle[0], base + triangle[1], base + triangle[2]]),
            );
        }
        Ok(())
    }

    fn finish(self) -> anyhow::Result<(Vec<Vec3>, Vec<ModelEdge>)> {
        // glTF splits vertices along UV and normal seams. Welding positions
        // keeps those seams from producing doubled wireframe edges.
        let mut vertices = Vec::new();
        let mut remap = Vec::with_capacity(self.vertices.len());
        let mut by_position = BTreeMap::new();
        for vertex in &self.vertices {
            let key = [vertex.x.to_bits(), vertex.y.to_bits(), vertex.z.to_bits()];
            let index = match by_position.entry(key) {
                Entry::Occupied(entry) => *entry.get(),
                Entry::Vacant(entry) => {
                    let index = u32::try_from(vertices.len()).expect("vertex counts fit u32");
                    entry.insert(index);
                    vertices.push(*vertex);
                    index
                }
            };
            remap.push(index);
        }
        let triangles: Vec<[u32; 3]> = self
            .triangles
            .iter()
            .map(|triangle| {
                [
                    remap[triangle[0] as usize],
                    remap[triangle[1] as usize],
                    remap[triangle[2] as usize],
                ]
            })
            .filter(|[a, b, c]| a != b && b != c && c != a)
            .collect();
        ensure!(
            !triangles.is_empty(),
            "the model has no non-degenerate triangles"
        );
        ensure!(
            vertices.len() <= u16::MAX as usize + 1,
            "the model has {} distinct vertices, but the wireframe renderer indexes with u16",
            vertices.len()
        );
        let normals: Vec<Option<Vec3>> = triangles
            .iter()
            .map(|&[a, b, c]| {
                triangle_normal(
                    vertices[a as usize],
                    vertices[b as usize],
                    vertices[c as usize],
                )
            })
            .collect();
        let mut adjacency: BTreeMap<(u32, u32), Vec<Vec3>> = BTreeMap::new();
        for (index, triangle) in triangles.iter().enumerate() {
            let Some(normal) = normals[index] else {
                continue;
            };
            for (a, b) in [
                (triangle[0], triangle[1]),
                (triangle[1], triangle[2]),
                (triangle[2], triangle[0]),
            ] {
                adjacency
                    .entry((a.min(b), a.max(b)))
                    .or_default()
                    .push(normal);
            }
        }
        let edges = adjacency
            .into_iter()
            .map(|((a, b), normals)| {
                let kind = match normals.as_slice() {
                    [first, second] => EdgeKind::Crease(first.dot(*second).abs()),
                    _ => EdgeKind::Always,
                };
                ModelEdge {
                    points: [a as u16, b as u16],
                    kind,
                }
            })
            .collect();
        fit_to_unit_radius(&mut vertices)?;
        Ok((vertices, edges))
    }
}

/// Centers the wireframe on the origin and scales it to the radius used by the
/// procedural figures, so the existing camera framing applies.
fn fit_to_unit_radius(vertices: &mut [Vec3]) -> anyhow::Result<()> {
    let mut minimum = Vec3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
    let mut maximum = Vec3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for vertex in vertices.iter() {
        minimum.x = minimum.x.min(vertex.x);
        minimum.y = minimum.y.min(vertex.y);
        minimum.z = minimum.z.min(vertex.z);
        maximum.x = maximum.x.max(vertex.x);
        maximum.y = maximum.y.max(vertex.y);
        maximum.z = maximum.z.max(vertex.z);
    }
    let center = (minimum + maximum) * 0.5;
    let radius = vertices
        .iter()
        .map(|&vertex| (vertex - center).length())
        .fold(0.0_f32, f32::max);
    ensure!(radius > 1e-6, "the model has no spatial extent");
    let scale = 1.0 / radius;
    for vertex in vertices {
        *vertex = (*vertex - center) * scale;
    }
    Ok(())
}

fn triangle_normal(a: Vec3, b: Vec3, c: Vec3) -> Option<Vec3> {
    let normal = (b - a).cross(c - a);
    let length = normal.length();
    (length > 1e-12).then(|| normal * (1.0 / length))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_weapon_loads_as_a_lean_wireframe() -> anyhow::Result<()> {
        let models = ModelSet::load()?;
        let model = models.current();
        assert!(!model.name.is_empty());
        let wireframe = model.wireframe(CreaseThreshold::default());
        assert!(wireframe.vertex_count() > 1_000);
        assert!(wireframe.edges().len() > 1_000);
        for edge in wireframe.edges() {
            assert!((edge[0] as usize) < wireframe.vertex_count());
            assert!((edge[1] as usize) < wireframe.vertex_count());
            assert_ne!(edge[0], edge[1]);
        }
        let mut sorted = wireframe.edges().to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), wireframe.edges().len());
        for index in 0..wireframe.vertex_count() {
            let vertex = wireframe.vertex(index, 0.0);
            assert!(vertex.length() <= 1.0 + 1e-4);
        }
        Ok(())
    }

    #[test]
    fn model_set_advances_and_wraps() -> anyhow::Result<()> {
        let mut set = ModelSet::load()?;
        let (_, total) = set.position();
        assert!(total >= 1);
        let first = set.current().name.clone();
        if total == 1 {
            assert!(!set.advance());
            return Ok(());
        }
        assert!(set.advance());
        assert_ne!(set.current().name, first);
        for _ in 1..total {
            assert!(set.advance());
        }
        assert_eq!(set.current().name, first);
        Ok(())
    }

    #[test]
    fn every_bundled_model_loads() -> anyhow::Result<()> {
        let paths = model_paths()?;
        assert!(!paths.is_empty());
        for path in paths {
            let model = Model::load_file(&path)
                .with_context(|| format!("could not load {}", path.display()))?;
            assert!(!model.name.is_empty());
            let wireframe = model.wireframe(CreaseThreshold::MIN);
            assert!(wireframe.vertex_count() > 0);
            assert!(!wireframe.edges().is_empty());
        }
        Ok(())
    }

    #[test]
    fn a_higher_crease_threshold_keeps_fewer_edges() -> anyhow::Result<()> {
        let path = model_paths()?
            .into_iter()
            .next()
            .context("the showcase has a bundled model")?;
        let model = Model::load_file(&path)?;
        let fine = model.wireframe(CreaseThreshold::MIN);
        let coarse = model.wireframe(CreaseThreshold::MAX);
        assert!(fine.edges().len() > coarse.edges().len());
        Ok(())
    }

    #[test]
    fn display_name_title_cases_the_file_stem() {
        assert_eq!(display_name(Path::new("pist_deagle.glb")), "Pist Deagle");
    }

    #[test]
    fn node_transforms_compose_parent_with_child() {
        let parent = Transform::from_gltf([
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [2.0, 0.0, 0.0, 1.0],
        ]);
        let child = Transform::from_gltf([
            [2.0, 0.0, 0.0, 0.0],
            [0.0, 2.0, 0.0, 0.0],
            [0.0, 0.0, 2.0, 0.0],
            [0.0, 1.0, 0.0, 1.0],
        ]);
        let vertex = parent.then(child).apply([1.0, 0.0, 0.0]);
        assert!((vertex - Vec3::new(4.0, 1.0, 0.0)).length() < 1e-6);
    }
}
