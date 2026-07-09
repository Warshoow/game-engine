//! Mesher blocky — étape 1 : culling naïf par faces visibles.
//!
//! L'idée : un chunk plein de pierre n'a pas besoin de 32³ cubes — seules les
//! faces **au contact de l'air** existent visuellement. On parcourt chaque
//! voxel solide et on émet une face uniquement si le voisin dans cette
//! direction est non-solide. C'est le « face culling » ; le greedy meshing
//! (étape 2) fusionnera ensuite les faces coplanaires adjacentes.
//!
//! Le mesher est pur et sans Bevy : il produit des tampons bruts
//! ([`MeshData`]) que l'app convertit en `Mesh` Bevy. Ça le rend testable
//! headless — on compte des faces, pas des pixels.
//!
//! Sorties en **mètres** (§2) : les positions sont `coordonnée voxel ×
//! voxel_size_m`, dans le repère local du chunk (l'origine du chunk est
//! placée par l'ECS, pas par le mesher).

use crate::chunk::Chunk;
use crate::registry::{ContentId, Registry};

/// Tampons de mesh bruts, agnostiques du moteur de rendu.
#[derive(Debug, Default, Clone)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub colors: Vec<[f32; 4]>,
    /// Triangles, en sens antihoraire (CCW) vu de l'extérieur — la
    /// convention de face avant de Bevy/wgpu.
    pub indices: Vec<u32>,
}

impl MeshData {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn face_count(&self) -> usize {
        // Une face = un quad = 2 triangles = 6 indices.
        self.indices.len() / 6
    }
}

/// Les 6 directions de face d'un cube. L'ordre des 4 coins de chaque face
/// est choisi pour un enroulement CCW vu de l'extérieur.
const FACES: [Face; 6] = [
    // +X
    Face { normal: [1.0, 0.0, 0.0], neighbor: [1, 0, 0],
           corners: [[1, 0, 0], [1, 1, 0], [1, 1, 1], [1, 0, 1]] },
    // -X
    Face { normal: [-1.0, 0.0, 0.0], neighbor: [-1, 0, 0],
           corners: [[0, 0, 1], [0, 1, 1], [0, 1, 0], [0, 0, 0]] },
    // +Y
    Face { normal: [0.0, 1.0, 0.0], neighbor: [0, 1, 0],
           corners: [[0, 1, 0], [0, 1, 1], [1, 1, 1], [1, 1, 0]] },
    // -Y
    Face { normal: [0.0, -1.0, 0.0], neighbor: [0, -1, 0],
           corners: [[0, 0, 1], [0, 0, 0], [1, 0, 0], [1, 0, 1]] },
    // +Z
    Face { normal: [0.0, 0.0, 1.0], neighbor: [0, 0, 1],
           corners: [[1, 0, 1], [1, 1, 1], [0, 1, 1], [0, 0, 1]] },
    // -Z
    Face { normal: [0.0, 0.0, -1.0], neighbor: [0, 0, -1],
           corners: [[0, 0, 0], [0, 1, 0], [1, 1, 0], [1, 0, 0]] },
];

struct Face {
    normal: [f32; 3],
    neighbor: [i32; 3],
    corners: [[u32; 3]; 4],
}

/// Meshe un chunk par culling naïf.
///
/// La solidité et la couleur sont résolues **une fois par entrée de palette**
/// (pas une fois par voxel) via le registre — c'est le chemin data-driven :
/// le mesher ne connaît aucun bloc par son nom.
///
/// Hors du chunk, le voisin est traité comme de l'air : les faces de bordure
/// sont émises même si le chunk adjacent les cache. Assumé pour la slice —
/// le raccord inter-chunks viendra avec le streaming.
pub fn mesh_chunk(chunk: &Chunk, registry: &Registry, voxel_size_m: f32) -> MeshData {
    let size = chunk.size();

    // Palette résolue : index local → (solide, couleur).
    let resolved: Vec<(bool, [f32; 4])> = chunk
        .palette()
        .iter()
        .map(|&id| resolve(registry, id))
        .collect();
    let solid_at = |x: i32, y: i32, z: i32| -> bool {
        if x < 0 || y < 0 || z < 0 || x >= size as i32 || y >= size as i32 || z >= size as i32 {
            return false; // hors chunk = air (voir doc de fonction)
        }
        resolved[chunk.get_local(x as u32, y as u32, z as u32) as usize].0
    };

    let mut mesh = MeshData::default();
    for z in 0..size {
        for y in 0..size {
            for x in 0..size {
                let (solid, color) = resolved[chunk.get_local(x, y, z) as usize];
                if !solid {
                    continue;
                }
                for face in &FACES {
                    let nx = x as i32 + face.neighbor[0];
                    let ny = y as i32 + face.neighbor[1];
                    let nz = z as i32 + face.neighbor[2];
                    if !solid_at(nx, ny, nz) {
                        emit_quad(&mut mesh, [x, y, z], face, color, voxel_size_m);
                    }
                }
            }
        }
    }
    mesh
}

fn resolve(registry: &Registry, id: ContentId) -> (bool, [f32; 4]) {
    match registry.get(id).and_then(|e| e.block.as_ref()) {
        Some(b) => (b.solid, [b.color[0], b.color[1], b.color[2], 1.0]),
        // ID inconnu du registre : on le rend visible et criard plutôt
        // qu'invisible — un bug de contenu doit se voir.
        None => (true, [1.0, 0.0, 1.0, 1.0]),
    }
}

fn emit_quad(mesh: &mut MeshData, voxel: [u32; 3], face: &Face, color: [f32; 4], scale: f32) {
    let base = mesh.positions.len() as u32;
    for corner in &face.corners {
        mesh.positions.push([
            (voxel[0] + corner[0]) as f32 * scale,
            (voxel[1] + corner[1]) as f32 * scale,
            (voxel[2] + corner[2]) as f32 * scale,
        ]);
        mesh.normals.push(face.normal);
        mesh.colors.push(color);
    }
    // Deux triangles CCW sur les coins [0,1,2] et [0,2,3].
    mesh.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{BlockData, ContentEntry, Kind, Registry};

    fn test_registry() -> (Registry, ContentId, ContentId) {
        let mut reg = Registry::new();
        let air = reg
            .register(ContentEntry {
                identifier: "core:air".into(),
                kind: Kind::Block,
                block: Some(BlockData { solid: false, color: [0.0; 3] }),
            })
            .unwrap();
        let stone = reg
            .register(ContentEntry {
                identifier: "core:stone".into(),
                kind: Kind::Block,
                block: Some(BlockData { solid: true, color: [0.5, 0.5, 0.5] }),
            })
            .unwrap();
        (reg, air, stone)
    }

    #[test]
    fn empty_chunk_produces_empty_mesh() {
        let (reg, air, _) = test_registry();
        let chunk = Chunk::filled(8, air);
        assert!(mesh_chunk(&chunk, &reg, 1.0).is_empty());
    }

    #[test]
    fn lone_voxel_has_six_faces() {
        let (reg, air, stone) = test_registry();
        let mut chunk = Chunk::filled(8, air);
        chunk.set(3, 3, 3, stone);
        let mesh = mesh_chunk(&chunk, &reg, 1.0);
        assert_eq!(mesh.face_count(), 6);
        assert_eq!(mesh.positions.len(), 24); // 6 faces × 4 sommets
    }

    #[test]
    fn touching_faces_are_culled() {
        // Deux voxels côte à côte : 12 faces − 2 au contact = 10.
        let (reg, air, stone) = test_registry();
        let mut chunk = Chunk::filled(8, air);
        chunk.set(3, 3, 3, stone);
        chunk.set(4, 3, 3, stone);
        assert_eq!(mesh_chunk(&chunk, &reg, 1.0).face_count(), 10);
    }

    #[test]
    fn buried_voxel_emits_nothing() {
        // Cube 3×3×3 plein : seule la surface (54 faces) est émise —
        // le voxel central ne contribue à rien.
        let (reg, air, stone) = test_registry();
        let mut chunk = Chunk::filled(8, air);
        for z in 0..3 {
            for y in 0..3 {
                for x in 0..3 {
                    chunk.set(x, y, z, stone);
                }
            }
        }
        assert_eq!(mesh_chunk(&chunk, &reg, 1.0).face_count(), 54);
    }

    #[test]
    fn positions_are_scaled_to_meters() {
        // voxel_size 0.5 m (2 vox/m) : un voxel en (2,0,0) s'étend de
        // x=1.0 m à x=1.5 m dans le mesh.
        let (reg, air, stone) = test_registry();
        let mut chunk = Chunk::filled(4, air);
        chunk.set(2, 0, 0, stone);
        let mesh = mesh_chunk(&chunk, &reg, 0.5);
        let xs: Vec<f32> = mesh.positions.iter().map(|p| p[0]).collect();
        assert!(xs.iter().all(|&x| (1.0..=1.5).contains(&x)));
        assert!(xs.contains(&1.0) && xs.contains(&1.5));
    }

    #[test]
    fn winding_is_ccw_seen_from_outside() {
        // Pour chaque triangle, la normale géométrique (produit vectoriel)
        // doit pointer dans le même sens que la normale déclarée — sinon
        // le back-face culling du GPU mangera la face.
        let (reg, air, stone) = test_registry();
        let mut chunk = Chunk::filled(4, air);
        chunk.set(1, 1, 1, stone);
        let mesh = mesh_chunk(&chunk, &reg, 1.0);
        for tri in mesh.indices.chunks(3) {
            let [a, b, c] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
            let (pa, pb, pc) = (mesh.positions[a], mesh.positions[b], mesh.positions[c]);
            let e1 = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
            let e2 = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
            let cross = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            let n = mesh.normals[a];
            let dot = cross[0] * n[0] + cross[1] * n[1] + cross[2] * n[2];
            assert!(dot > 0.0, "triangle enroulé à l'envers");
        }
    }
}
