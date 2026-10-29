//! Loads the binary glTF meshes backing the model visualizer.

use std::{
    collections::{btree_map::Entry, BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use anyhow::{ensure, Context};

use super::{
    geometry::{Vec3, Wireframe},
    CreaseThreshold,
};

/// Directory scanned for the models. Binary glTF keeps the geometry
/// and any external resources out of the crate's compiled assets.
const MODEL_DIRECTORY: &str = "assets/models";

/// The node whose subtree moves backward when the ejection port is focused.
pub(super) const SLIDE_NODE: &str = "slide";

/// A loaded model centered on the origin and normalized to unit radius.
pub struct Model {
    /// Title-cased file stem for the HUD.
    pub name: String,
    /// Exact file stem, used to match the authored weapon catalog.
    pub id: String,
    vertices: Vec<Vec3>,
    edges: Vec<ModelEdge>,
    vertex_parts: Vec<u16>,
    parts: Vec<ModelPart>,
    anchors: BTreeMap<String, Anchor>,
    node_parts: BTreeMap<String, Vec<u16>>,
}

impl Model {
    fn load_file(path: &Path) -> anyhow::Result<Self> {
        let gltf = gltf::Gltf::open(path)
            .with_context(|| format!("could not open the model at {}", path.display()))?;
        let blob = gltf
            .blob
            .as_deref()
            .context("the model must be a binary glTF (.glb)")?;
        ensure!(
            gltf.buffers()
                .all(|buffer| matches!(buffer.source(), gltf::buffer::Source::Bin)),
            "the model must embed its buffers in the .glb"
        );
        let get_buffer = |buffer: gltf::Buffer<'_>| match buffer.source() {
            gltf::buffer::Source::Bin => Some(blob),
            gltf::buffer::Source::Uri(_) => None,
        };
        let scene = gltf
            .default_scene()
            .or_else(|| gltf.scenes().next())
            .context("the model has no scene to load")?;
        let meshes: Vec<_> = gltf.meshes().collect();
        let mut nodes: Vec<Option<RawNode>> = (0..gltf.nodes().count()).map(|_| None).collect();
        let mut stack: Vec<_> = scene
            .nodes()
            .map(|node| (node, Transform::IDENTITY, None))
            .collect();
        while let Some((node, parent, parent_index)) = stack.pop() {
            let transform = parent.then(Transform::from_gltf(node.transform().matrix()));
            let index = node.index();
            ensure!(
                nodes[index].is_none(),
                "the model scene visits node {index} twice"
            );
            nodes[index] = Some(RawNode {
                name: node.name().unwrap_or("").to_owned(),
                parent: parent_index,
                transform,
                mesh: node.mesh().map(|mesh| mesh.index()),
            });
            for child in node.children() {
                stack.push((child, transform, Some(index)));
            }
        }

        let mut builder = Builder::default();
        let mut node_part = vec![None; nodes.len()];
        for (index, node) in nodes.iter().enumerate() {
            let Some(node) = node else {
                continue;
            };
            let Some(mesh_index) = node.mesh else {
                continue;
            };
            let mesh = meshes
                .get(mesh_index)
                .context("the model references an unknown mesh")?;
            let name = if node.name.is_empty() {
                mesh.name().unwrap_or("part").to_owned()
            } else {
                node.name.clone()
            };
            let part = builder.push_part(mesh, node.transform, name, &get_buffer)?;
            node_part[index] = Some(part);
        }
        let BuiltModel {
            mut vertices,
            vertex_parts,
            edges,
            parts,
        } = builder.finish()?;
        ensure!(
            !vertices.is_empty(),
            "the model has no mesh geometry to show"
        );
        let (model_center, model_scale) = fit_to_unit_radius(&mut vertices)?;

        let mut anchors = BTreeMap::new();
        let mut part_anchors = Vec::with_capacity(parts.len());
        for part in &parts {
            let range = part.start..part.start + part.len;
            let (center, radius, size) = bounds(&vertices[range]);
            part_anchors.push(Anchor {
                center,
                radius,
                size,
                part: Some(part.index),
            });
        }
        for (index, node) in nodes.iter().enumerate() {
            let Some(node) = node else {
                continue;
            };
            if node.name.is_empty() {
                continue;
            }
            let anchor = match node_part[index] {
                Some(part) => part_anchors[part as usize],
                None => {
                    let center = (node.transform.translation() - model_center) * model_scale;
                    let size = node.transform.scale() * model_scale;
                    Anchor {
                        center,
                        radius: size.length(),
                        size,
                        part: None,
                    }
                }
            };
            if let Entry::Vacant(entry) = anchors.entry(node.name.clone()) {
                entry.insert(anchor);
            }
        }

        let children = child_indices(&nodes);
        let mut node_parts: BTreeMap<String, Vec<u16>> = BTreeMap::new();
        for (index, node) in nodes.iter().enumerate() {
            let Some(node) = node else {
                continue;
            };
            if node.name.is_empty() {
                continue;
            }
            let mut parts = BTreeSet::new();
            collect_parts(index, &node_part, &children, &mut parts);
            node_parts
                .entry(node.name.clone())
                .or_default()
                .extend(parts);
        }
        for parts in node_parts.values_mut() {
            parts.sort_unstable();
            parts.dedup();
        }

        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .context("the model file name is not valid UTF-8")?
            .to_owned();
        Ok(Self {
            name: display_name(path),
            id,
            vertices,
            edges,
            vertex_parts,
            parts,
            anchors,
            node_parts,
        })
    }

    /// Builds the wireframe for one crease angle. The model's vertices are
    /// already normalized, so changing the threshold keeps the framing.
    pub fn wireframe(&self, crease: CreaseThreshold) -> Wireframe {
        let cosine_limit = crease.degrees().to_radians().cos();
        let mut edges = Vec::new();
        let mut edge_parts = Vec::new();
        for edge in &self.edges {
            if edge.kind.visible_at(cosine_limit) {
                edges.push(edge.points);
                edge_parts.push(edge.part);
            }
        }
        Wireframe::from_parts(
            self.vertices.clone(),
            edges,
            self.vertex_parts.clone(),
            edge_parts,
            u16::try_from(self.parts.len()).expect("the model has too many mesh parts"),
        )
    }

    /// The mesh part or empty box registered under a glTF node name.
    pub fn anchor(&self, name: &str) -> Option<Anchor> {
        self.anchors.get(name).copied()
    }

    /// Mesh parts belonging to a named node and all of its descendants.
    pub fn node_parts(&self, name: &str) -> &[u16] {
        self.node_parts.get(name).map_or(&[], Vec::as_slice)
    }

    pub fn part_name(&self, part: u16) -> &str {
        &self.parts[part as usize].name
    }

    pub fn part_count(&self) -> u16 {
        u16::try_from(self.parts.len()).expect("the model has too many mesh parts")
    }
}

/// One semantic target in the model: a mesh part or a scaled empty box.
#[derive(Clone, Copy, Debug)]
pub struct Anchor {
    /// Bounds center in normalized model space.
    pub center: Vec3,
    /// Radius used to frame the anchor. Mesh anchors use the farthest vertex
    /// from the bounds center; empty anchors use the full scaled-box diagonal.
    pub radius: f32,
    /// Full box or mesh AABB dimensions in normalized model space.
    pub size: Vec3,
    /// The mesh part this anchor highlights, or `None` for an empty.
    pub part: Option<u16>,
}

/// A loaded mesh node's vertex range in the model wireframe.
struct ModelPart {
    name: String,
    index: u16,
    start: usize,
    len: usize,
}

/// One unique geometric edge and how sharply its two faces meet.
#[derive(Clone, Copy)]
struct ModelEdge {
    points: [u16; 2],
    kind: EdgeKind,
    part: u16,
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
    /// advance to, so a one-model set stays put.
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

    fn translation(self) -> Vec3 {
        Vec3::new(self.0[3][0], self.0[3][1], self.0[3][2])
    }

    fn scale(self) -> Vec3 {
        let column = |index: usize| Vec3::new(self.0[index][0], self.0[index][1], self.0[index][2]);
        Vec3::new(column(0).length(), column(1).length(), column(2).length())
    }
}

struct RawNode {
    name: String,
    parent: Option<usize>,
    transform: Transform,
    mesh: Option<usize>,
}

fn child_indices(nodes: &[Option<RawNode>]) -> Vec<Vec<usize>> {
    let mut children = vec![Vec::new(); nodes.len()];
    for (index, node) in nodes.iter().enumerate() {
        if let Some(parent) = node.as_ref().and_then(|node| node.parent) {
            children[parent].push(index);
        }
    }
    children
}

fn collect_parts(
    node: usize,
    node_part: &[Option<u16>],
    children: &[Vec<usize>],
    parts: &mut BTreeSet<u16>,
) {
    if let Some(part) = node_part[node] {
        parts.insert(part);
    }
    for &child in &children[node] {
        collect_parts(child, node_part, children, parts);
    }
}

fn bounds(vertices: &[Vec3]) -> (Vec3, f32, Vec3) {
    let mut minimum = Vec3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
    let mut maximum = Vec3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for vertex in vertices {
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
    (center, radius, maximum - minimum)
}

struct BuiltModel {
    vertices: Vec<Vec3>,
    vertex_parts: Vec<u16>,
    edges: Vec<ModelEdge>,
    parts: Vec<ModelPart>,
}

#[derive(Default)]
struct Builder {
    vertices: Vec<Vec3>,
    vertex_parts: Vec<u16>,
    edges: Vec<ModelEdge>,
    parts: Vec<ModelPart>,
}

impl Builder {
    fn push_part<'s, F>(
        &mut self,
        mesh: &gltf::Mesh<'_>,
        transform: Transform,
        name: String,
        get_buffer: &F,
    ) -> anyhow::Result<u16>
    where
        F: for<'a> Fn(gltf::Buffer<'a>) -> Option<&'s [u8]> + Clone,
    {
        let mut local = LocalBuilder::default();
        local.push_mesh(mesh, transform, get_buffer)?;
        let (part_vertices, part_edges) = local.finish()?;
        let index = u16::try_from(self.parts.len()).context("the model has too many mesh parts")?;
        let start = self.vertices.len();
        let base = u16::try_from(start).context("the model has too many vertices")?;
        for vertex in part_vertices {
            self.vertices.push(vertex);
            self.vertex_parts.push(index);
        }
        for edge in part_edges {
            self.edges.push(ModelEdge {
                points: [
                    base.checked_add(edge.points[0])
                        .context("the model has too many vertices")?,
                    base.checked_add(edge.points[1])
                        .context("the model has too many vertices")?,
                ],
                kind: edge.kind,
                part: index,
            });
        }
        self.parts.push(ModelPart {
            name,
            index,
            start,
            len: self.vertices.len() - start,
        });
        Ok(index)
    }

    fn finish(self) -> anyhow::Result<BuiltModel> {
        ensure!(
            !self.vertices.is_empty(),
            "the model has no non-degenerate triangles"
        );
        ensure!(
            self.vertices.len() <= u16::MAX as usize + 1,
            "the model has {} distinct vertices, but the wireframe renderer indexes with u16",
            self.vertices.len()
        );
        Ok(BuiltModel {
            vertices: self.vertices,
            vertex_parts: self.vertex_parts,
            edges: self.edges,
            parts: self.parts,
        })
    }
}

#[derive(Default)]
struct LocalBuilder {
    vertices: Vec<Vec3>,
    triangles: Vec<[u32; 3]>,
}

impl LocalBuilder {
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
                "the wireframe only supports triangle primitives"
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
            "the model part has no non-degenerate triangles"
        );
        ensure!(
            vertices.len() <= u16::MAX as usize + 1,
            "the model part has {} distinct vertices, but the wireframe renderer indexes with u16",
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
                    points: [
                        u16::try_from(a).expect("part vertices fit u16"),
                        u16::try_from(b).expect("part vertices fit u16"),
                    ],
                    kind,
                    part: 0,
                }
            })
            .collect();
        Ok((vertices, edges))
    }
}

/// Centers the wireframe on the origin and scales it to unit radius so every
/// model uses the same camera framing. Returns the applied center and scale.
fn fit_to_unit_radius(vertices: &mut [Vec3]) -> anyhow::Result<(Vec3, f32)> {
    let (center, radius, _) = bounds(vertices);
    ensure!(radius > 1e-6, "the model has no spatial extent");
    let scale = 1.0 / radius;
    for vertex in vertices {
        *vertex = (*vertex - center) * scale;
    }
    Ok((center, scale))
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
    fn empty_anchor_radius_uses_the_full_scaled_diagonal() -> anyhow::Result<()> {
        let models = ModelSet::load()?;
        let empty = models
            .models
            .iter()
            .flat_map(|model| model.anchors.values())
            .find(|anchor| anchor.part.is_none())
            .expect("the bundled model has an empty anchor");
        assert!((empty.radius - empty.size.length()).abs() < 1e-6);
        Ok(())
    }

    #[test]
    fn bundled_models_load_as_lean_wireframes() -> anyhow::Result<()> {
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
        for (index, edge) in wireframe.edges().iter().enumerate() {
            assert_eq!(
                wireframe.edge_part(index),
                wireframe.vertex_part(edge[0] as usize)
            );
        }
        let mut sorted = wireframe.edges().to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), wireframe.edges().len());
        for index in 0..wireframe.vertex_count() {
            let vertex = wireframe.vertex(index);
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
    fn deagle_exposes_named_anchors_and_mesh_parts() -> anyhow::Result<()> {
        let path = model_paths()?
            .into_iter()
            .find(|path| path.file_stem().is_some_and(|stem| stem == "pist_deagle"))
            .context("the deagle model is bundled")?;
        let model = Model::load_file(&path)?;
        assert_eq!(model.id, "pist_deagle");
        assert!(model.part_count() >= 7);

        let slide = model.anchor("slide").context("the slide anchor exists")?;
        assert!(slide.part.is_some());
        assert!(slide.radius > 0.0);
        let slide_parts = model.node_parts(SLIDE_NODE);
        assert!(
            slide_parts.len() >= 2,
            "the slide subtree includes the rear sight"
        );
        assert!(slide_parts.contains(&slide.part.expect("a mesh anchor has a part")));

        let port = model
            .anchor("ejection_port")
            .context("the ejection port anchor exists")?;
        assert!(port.part.is_none());
        assert!(port.size.z > 0.0);
        assert!(port.radius > 0.0);

        for name in ["muzzle", "trigger_guard", "frame", "magazine"] {
            assert!(model.anchor(name).is_some(), "{name} is a POI anchor");
        }
        Ok(())
    }

    #[test]
    fn a_higher_crease_threshold_keeps_fewer_edges() -> anyhow::Result<()> {
        let path = model_paths()?
            .into_iter()
            .next()
            .context("the model directory contains a bundled model")?;
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
