"""Prepare geometry-only Source weapon assets with Blender and SourceIO.

Run from the repository root:
  Blender --background --factory-startup --python tools/prepare_weapon_models.py -- \
    --source-dir ../cs2-weapons/CS2/models/weapons --blend-dir ../Weapons \
    --glb-dir assets/models --reference-dir ../Weaponz

Part selections in weapon_model_parts.json refer to the bundled source pack.
They preserve every source triangle; coincident vertices are used only to find
connected components, never to weld or simplify the exported geometry.
"""

import argparse
import hashlib
import json
import sys
from dataclasses import dataclass
from pathlib import Path

import addon_utils
import bpy
import numpy as np
from mathutils import Vector


@dataclass
class Geometry:
    vertices: np.ndarray
    faces: np.ndarray


def connected_components(vertices, faces):
    _, keys = np.unique(np.rint(vertices * 1e6).astype(np.int64), axis=0, return_inverse=True)
    parents = np.arange(keys.max() + 1)

    def root(index):
        while parents[index] != index:
            parents[index] = parents[parents[index]]
            index = parents[index]
        return index

    for a, b, c in keys[faces]:
        a, b, c = root(a), root(b), root(c)
        parents[b] = a
        parents[c] = a
    roots = np.array([root(index) for index in keys[faces[:, 0]]])
    identifiers, counts = np.unique(roots, return_counts=True)
    order = np.lexsort((identifiers, -counts))
    labels = {identifiers[index]: number for number, index in enumerate(order)}
    return np.array([labels[index] for index in roots], dtype=np.int32)


def inside(points, bounds):
    mask = np.ones(len(points), dtype=bool)
    for axis, limits in bounds.items():
        coordinate = points[:, 'xyz'.index(axis)]
        if limits[0] is not None:
            mask &= coordinate >= limits[0]
        if limits[1] is not None:
            mask &= coordinate <= limits[1]
    return mask


def source_hash(path):
    digest = hashlib.sha256()
    for suffix in ('.mdl', '.vvd', '.dx90.vtx'):
        companion = path.with_suffix(suffix)
        digest.update(suffix.encode())
        digest.update(companion.read_bytes())
    return digest.hexdigest()


def clear_scene():
    for obj in list(bpy.data.objects):
        bpy.data.objects.remove(obj, do_unlink=True)
    for mesh in list(bpy.data.meshes):
        if mesh.users == 0:
            bpy.data.meshes.remove(mesh)
    for armature in list(bpy.data.armatures):
        if armature.users == 0:
            bpy.data.armatures.remove(armature)
    for collection in list(bpy.data.collections):
        bpy.data.collections.remove(collection)


def import_source(path):
    clear_scene()
    result = bpy.ops.sourceio.mdl(
        filepath=str(path), directory=str(path.parent), files=[{'name': path.name}],
        import_textures=False, import_physics=False, import_animations=False,
    )
    assert result == {'FINISHED'}, (path, result)
    meshes = [obj for obj in bpy.context.scene.objects if obj.type == 'MESH']
    assert len(meshes) == 1, f'{path}: expected one source mesh'
    obj = meshes[0]
    assert all(len(poly.vertices) == 3 for poly in obj.data.polygons), path
    vertices = np.array([obj.matrix_world @ vertex.co for vertex in obj.data.vertices], dtype=np.float32)
    faces = np.array([poly.vertices[:] for poly in obj.data.polygons], dtype=np.int32)
    return Geometry(vertices, faces)


def label_parts(geometry, profile):
    components = connected_components(geometry.vertices, geometry.faces)
    assert len(geometry.faces) == profile['face_count']
    assert components.max() + 1 == profile['component_count']
    labels = np.full(len(components), 'parts', dtype=object)
    for part, identifiers in profile['components'].items():
        assert all(np.any(components == identifier) for identifier in identifiers), part
        labels[np.isin(components, identifiers)] = part
    centers = geometry.vertices[geometry.faces].mean(axis=1)
    for selection in profile.get('regions', []):
        mask = inside(centers, selection['bounds'])
        if 'components' in selection:
            mask &= np.isin(components, selection['components'])
        assert np.any(mask), selection
        labels[mask] = selection['part']
    for part, shell_id in profile.get('magazines', {}).items():
        shell_mask = components == shell_id
        shell_points = geometry.vertices[np.unique(geometry.faces[shell_mask])]
        low, high = shell_points.min(axis=0) - 0.0001, shell_points.max(axis=0) + 0.0001
        labels[shell_mask] = part
        for identifier in range(components.max() + 1):
            mask = components == identifier
            if identifier == shell_id or not np.all(labels[mask] == 'parts'):
                continue
            points = geometry.vertices[np.unique(geometry.faces[mask])]
            if np.all(points.min(axis=0) >= low) and np.all(points.max(axis=0) <= high):
                labels[mask] = part
    return labels, components


def create_mesh(name, geometry, face_mask):
    faces = geometry.faces[face_mask]
    indices, remapped = np.unique(faces, return_inverse=True)
    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata(geometry.vertices[indices].tolist(), [], remapped.reshape(-1, 3).tolist())
    mesh.update()
    obj = bpy.data.objects.new(name, mesh)
    bpy.context.scene.collection.objects.link(obj)
    return obj


def create_marker(name, points):
    low, high = points.min(axis=0), points.max(axis=0)
    obj = bpy.data.objects.new(name, None)
    bpy.context.scene.collection.objects.link(obj)
    obj.empty_display_type = 'CUBE'
    obj.empty_display_size = 1
    obj.location = (low + high) / 2
    # VU uses an empty's scale as its focus-box size.
    obj.scale = np.maximum((high - low) / 2, 0.001)
    return obj


def build_scene(geometry, selection_geometry, labels, components, profile):
    clear_scene()
    objects = {}
    split_parts = {'parts', *profile.get('magazines', {}).keys()}
    for part in sorted(set(labels)):
        mask = labels == part
        if part in split_parts:
            identifiers, counts = np.unique(components[mask], return_counts=True)
            order = np.lexsort((identifiers, -counts))
            root = None
            for number, index in enumerate(order):
                name = part if number == 0 else f'{part}.{number:03d}'
                obj = create_mesh(name, geometry, mask & (components == identifiers[index]))
                if root is None:
                    root = obj
                    objects[part] = obj
                else:
                    obj.parent = root
        else:
            objects[part] = create_mesh(part, geometry, mask)

    centers = selection_geometry.vertices[selection_geometry.faces].mean(axis=1)
    for name, selection in profile.get('anchors', {}).items():
        assert name not in objects, name
        mask = inside(centers, selection['bounds'])
        if 'components' in selection:
            mask &= np.isin(components, selection['components'])
        assert np.any(mask), (name, selection)
        points = geometry.vertices[np.unique(geometry.faces[mask])]
        objects[name] = create_marker(name, points)

    if profile.get('muzzle', True):
        part = profile.get('muzzle_part', 'barrel')
        points = geometry.vertices[np.unique(geometry.faces[labels == part])]
        assert len(points), (part, 'missing muzzle geometry')
        tip = points[points[:, 1] <= points[:, 1].min() + .003]
        objects['muzzle'] = create_marker('muzzle', tip)
    for child, parent in profile.get('parents', {}).items():
        objects[child].parent = objects[parent]
    assert sum(len(obj.data.polygons) for obj in bpy.context.scene.objects if obj.type == 'MESH') == len(geometry.faces)
    return objects


def strip_non_geometry():
    depsgraph = bpy.context.evaluated_depsgraph_get()
    for obj in bpy.context.scene.objects:
        if obj.type == 'MESH' and obj.modifiers:
            evaluated_mesh = bpy.data.meshes.new_from_object(obj.evaluated_get(depsgraph), depsgraph=depsgraph)
            for modifier in list(obj.modifiers):
                obj.modifiers.remove(modifier)
            obj.data = evaluated_mesh
    for obj in list(bpy.context.scene.objects):
        if obj.type not in {'MESH', 'EMPTY'}:
            bpy.data.objects.remove(obj, do_unlink=True)
            continue
        obj.animation_data_clear()
        if obj.type == 'MESH':
            obj.data.materials.clear()
            for layer in list(obj.data.uv_layers):
                obj.data.uv_layers.remove(layer)
            obj.vertex_groups.clear()
    for collection in (bpy.data.materials, bpy.data.textures, bpy.data.images,
                       bpy.data.node_groups, bpy.data.actions, bpy.data.texts):
        for block in list(collection):
            collection.remove(block, do_unlink=True)
    bpy.data.orphans_purge(do_recursive=True)
    assert not bpy.data.materials and not bpy.data.images and not bpy.data.textures


def save_blend(path):
    bpy.context.preferences.filepaths.save_version = 0
    bpy.context.scene.render.engine = 'BLENDER_WORKBENCH'
    bpy.context.scene.display.shading.light = 'STUDIO'
    bpy.context.scene.display.shading.color_type = 'RANDOM'
    bpy.context.scene.display.shading.show_cavity = True
    bpy.ops.object.select_all(action='DESELECT')
    points = [obj.matrix_world @ vertex.co for obj in bpy.context.scene.objects if obj.type == 'MESH'
              for vertex in obj.data.vertices]
    low = np.min(points, axis=0)
    high = np.max(points, axis=0)
    for screen in bpy.data.screens:
        for area in screen.areas:
            if area.type == 'VIEW_3D':
                area.spaces.active.clip_start = 0.0001
                area.spaces.active.region_3d.view_location = (low + high) / 2
                area.spaces.active.region_3d.view_distance = float(np.linalg.norm(high - low) * 1.5)
                area.spaces.active.region_3d.view_rotation = Vector((1, 0, 0)).to_track_quat('-Z', 'Y')
                area.spaces.active.region_3d.view_perspective = 'ORTHO'
    bpy.ops.wm.save_as_mainfile(filepath=str(path), compress=True)


def export_glb(path):
    result = bpy.ops.export_scene.gltf(
        filepath=str(path), export_format='GLB', export_materials='NONE',
        export_normals=False, export_texcoords=False, export_vertex_color='NONE',
        export_animations=False, export_skins=False, export_morph=False,
        export_cameras=False, export_lights=False, export_extras=False,
    )
    assert result == {'FINISHED'}, (path, result)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source-dir', type=Path, required=True)
    parser.add_argument('--blend-dir', type=Path, required=True)
    parser.add_argument('--glb-dir', type=Path, required=True)
    parser.add_argument('--reference-dir', type=Path, required=True)
    parser.add_argument('--sourceio-dir', type=Path)
    args = parser.parse_args(sys.argv[sys.argv.index('--') + 1:])
    assert args.blend_dir.resolve() != args.reference_dir.resolve(), 'Keep the authored references in their own directory'
    args.blend_dir.mkdir(parents=True, exist_ok=True)
    args.glb_dir.mkdir(parents=True, exist_ok=True)
    profiles = json.loads(Path(__file__).with_name('weapon_model_parts.json').read_text())
    catalog = json.loads((Path(__file__).resolve().parents[1] / 'assets/models-spec/weapon_visualizer_meme_spec.json').read_text())
    addon_dir = args.sourceio_dir or Path(bpy.utils.user_resource('SCRIPTS', path='addons'))
    addon = next(path for path in addon_dir.iterdir()
                 if path.is_dir() and path.name.lower().replace('-', '').startswith('sourceio'))
    sys.path.insert(0, str(addon.parent))
    addon_utils.enable(addon.name, default_set=False, persistent=False)
    sources = {path.stem.removeprefix('w_'): path.resolve() for path in sorted(args.source_dir.glob('w_*.mdl'))}
    references = {'pist_deagle', 'rif_ak47'}
    assert set(sources) == set(profiles) | references
    geometries = {}
    hashes = {}
    for name, path in sources.items():
        if name in references:
            continue
        hashes[name] = source_hash(path)
        assert hashes[name] == profiles[name]['source_sha256'], f'{name}: part recipe belongs to another source model'
        geometries[name] = import_source(path)
        print('IMPORTED', name, flush=True)

    # Detach SourceIO's text-backed streams before removing its log text blocks.
    from SourceIO.logger import SourceLogMan
    for logger in SourceLogMan().loggers.values():
        if logger._bpy_logger is not None:
            logger._bpy_logger.setStream(sys.stdout)

    report = {}
    for name, profile in profiles.items():
        geometry = geometries[name]
        selection_geometry = geometries[profile.get('selection_reference', name)]
        assert np.array_equal(geometry.faces, selection_geometry.faces), name
        labels, components = label_parts(selection_geometry, profile)
        objects = build_scene(geometry, selection_geometry, labels, components, profile)
        strip_non_geometry()
        blend_path = args.blend_dir / f'{name}.blend'
        glb_path = args.glb_dir / f'{name}.glb'
        save_blend(blend_path)
        export_glb(glb_path)
        expected = {poi['anchor']['name'] for poi in catalog['weapons'].get(name, {}).get('poi', [])}
        report[name] = {
            'source_sha256': hashes[name], 'triangles': len(geometry.faces),
            'mesh_objects': sum(obj.type == 'MESH' for obj in bpy.context.scene.objects),
            'named_parts': sorted(objects), 'missing_catalog_anchors': sorted(expected - set(objects)),
            'blend_bytes': blend_path.stat().st_size, 'glb_bytes': glb_path.stat().st_size,
        }
        print('PREPARED', name, report[name]['mesh_objects'], 'meshes', flush=True)

    for name in sorted(references):
        bpy.ops.wm.open_mainfile(filepath=str((args.reference_dir / f'{name}.blend').resolve()))
        strip_non_geometry()
        path = args.blend_dir / f'{name}.blend'
        save_blend(path)
        report[name] = {'authored_reference': True, 'blend_bytes': path.stat().st_size,
                        'original_blend_bytes': (args.reference_dir / f'{name}.blend').stat().st_size,
                        'mesh_objects': sum(obj.type == 'MESH' for obj in bpy.context.scene.objects)}
        print('COMPACTED', name, path.stat().st_size, 'bytes', flush=True)
    (args.blend_dir / 'preparation_report.json').write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
