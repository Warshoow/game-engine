//! Blocs à forme (#14) : registre, rotation dans le monde et la save,
//! mesher, collision, visée — à travers l'API publique.

use voxel_core::chunk::{Chunk, ChunkPos};
use voxel_core::mesher::mesh_chunk_in_world;
use voxel_core::physics::{move_and_collide, move_and_step, Aabb};
use voxel_core::raycast::raycast;
use voxel_core::registry::{ContentId, Registry};
use voxel_core::shape::{Orient, Rotation};
use voxel_core::world::VoxelWorld;

const CONTENT: &str = r#"[
    (identifier: "t:air", kind: Block((solid: false, color: (0.0, 0.0, 0.0)))),
    (identifier: "t:stone", kind: Block((solid: true, color: (0.5, 0.5, 0.5)))),
    (identifier: "t:slab", kind: Block((solid: true, color: (0.5, 0.5, 0.5),
        shape: Some([((0, 0, 0), (16, 8, 16))]), orient: Half))),
    (identifier: "t:torch", kind: Block((solid: true, color: (1.0, 0.8, 0.2),
        shape: Some([((7, 0, 7), (9, 10, 9))]), collision: Some([]), orient: Face))),
]"#;

/// Monde 8³ à 1 vox/m, sol de pierre en y = 0 ; rend (monde, pierre, dalle, torche).
fn world() -> (VoxelWorld, ContentId, ContentId, ContentId) {
    let reg = Registry::from_ron(CONTENT).unwrap();
    let id = |name| reg.lookup(name).unwrap();
    let (air, stone, slab, torch) = (id("t:air"), id("t:stone"), id("t:slab"), id("t:torch"));
    let mut chunk = Chunk::filled(8, air);
    for x in 0..8 {
        for z in 0..8 {
            chunk.set(x, 0, z, stone);
        }
    }
    let mut w = VoxelWorld::new(reg, 8, 1.0);
    w.insert_chunk(ChunkPos { x: 0, y: 0, z: 0 }, chunk);
    (w, stone, slab, torch)
}

#[test]
fn bad_shapes_are_refused() {
    let block = |fields: &str| Registry::from_ron(&format!(r#"[(identifier: "t:x", kind: Block((color: (0.0, 0.0, 0.0), {fields})))]"#));
    assert!(block("solid: true, shape: Some([((0, 0, 0), (16, 8, 16))])").is_ok());
    assert!(block("solid: false, shape: Some([((0, 0, 0), (16, 8, 16))])").is_err(), "forme sans solid");
    assert!(block("solid: true, shape: Some([((0, 8, 0), (16, 8, 16))])").is_err(), "boîte vide");
    assert!(block("solid: true, shape: Some([((0, 0, 0), (17, 8, 16))])").is_err(), "hors de la case");
    assert!(block("solid: true, shape: Some([])").is_err(), "forme vide");
}

#[test]
fn rotation_is_cleared_with_its_block_and_saved_with_the_chunk() {
    let (mut w, _, slab, _) = world();
    let v = [2, 1, 2];
    let upside_down = Rotation::placed(Orient::Half, [0.0, 0.0, 1.0], [0, -1, 0], false);
    w.set_voxel(v, slab);
    w.set_rotation(v, upside_down);
    assert_eq!(w.boxes(v, true), vec![[[2.0, 1.5, 2.0], [3.0, 2.0, 3.0]]]);
    let state = w.chunk_state(ChunkPos { x: 0, y: 0, z: 0 });
    assert_eq!(state.rotations, vec![([2, 1, 2], upside_down)]);
    w.set_voxel(v, slab);
    assert_eq!(w.rotation(v), Rotation::IDENTITY);
}

#[test]
fn a_slab_hides_the_face_it_covers_and_keeps_the_others() {
    let (mut w, _, slab, _) = world();
    w.set_voxel([3, 1, 3], slab);
    let mesh = &mesh_chunk_in_world(&w, ChunkPos { x: 0, y: 0, z: 0 }, 1.0).unwrap()[0];
    let quads: Vec<_> = (0..mesh.face_count()).map(|f| (&mesh.positions[f * 4..f * 4 + 4], mesh.normals[f * 4])).collect();
    let inside = |q: &[[f32; 3]], lo: [f32; 3], hi: [f32; 3]| q.iter().all(|p| (0..3).all(|a| (lo[a]..=hi[a]).contains(&p[a])));
    // La dalle : 5 faces, sans son dessous posé sur la pierre.
    let slab_faces: Vec<_> = quads.iter().filter(|(q, _)| inside(q, [3.0, 1.0, 3.0], [4.0, 1.5, 4.0])).collect();
    assert_eq!(slab_faces.len(), 5);
    assert!(slab_faces.iter().all(|(_, n)| *n != [0.0, -1.0, 0.0]));
    // Le dessus de la pierre sous elle a disparu : aucun quad du sol ne
    // couvre le centre de la case.
    let covers_center = |q: &[[f32; 3]]| q.iter().all(|p| p[1] == 1.0) && {
        let (min, max) = (q.iter().fold([f32::MAX; 3], |m, p| [m[0].min(p[0]), 0.0, m[2].min(p[2])]), q.iter().fold([f32::MIN; 3], |m, p| [m[0].max(p[0]), 0.0, m[2].max(p[2])]));
        (min[0]..=max[0]).contains(&3.5) && (min[2]..=max[2]).contains(&3.5)
    };
    assert!(!quads.iter().any(|(q, n)| *n == [0.0, 1.0, 0.0] && covers_center(q)));
}

#[test]
fn a_slab_is_stepped_on_a_torch_is_walked_through() {
    let (mut w, _, slab, torch) = world();
    w.set_voxel([4, 1, 2], slab);
    w.set_voxel([2, 1, 5], torch);
    let player = Aabb::from_feet([2.5, 1.0, 2.5], 0.6, 1.8);
    // Vers la dalle : bloqué à plat, monté avec la marche.
    let flat = move_and_collide(&w, player, [2.0, 0.0, 0.0]);
    assert!(flat.collided[0]);
    let stepped = move_and_step(&w, player, [2.0, 0.0, 0.0], 0.6);
    assert!((stepped.aabb.min[1] - 1.5).abs() < 1e-3, "pieds à {}", stepped.aabb.min[1]);
    assert!((stepped.aabb.feet()[0] - 4.5).abs() < 1e-3);
    // Un bloc plein (1 m) ne se monte pas.
    w.set_voxel([4, 1, 2], w.registry.lookup("t:stone").unwrap());
    assert_eq!(move_and_step(&w, player, [2.0, 0.0, 0.0], 0.6).aabb.min[1], 1.0);
    // La torche ne bloque rien.
    assert!(!move_and_collide(&w, player, [0.0, 0.0, 3.0]).collided[2]);
}

#[test]
fn the_ray_hits_the_boxes_not_the_cell() {
    let (mut w, _, slab, torch) = world();
    w.set_voxel([3, 1, 3], slab);
    w.set_voxel([5, 1, 3], torch);
    // D'en haut : le dessus de la dalle, à mi-case.
    let hit = raycast(&w, [3.5, 4.0, 3.5], [0.0, -1.0, 0.0], 10.0).unwrap();
    assert_eq!((hit.voxel, hit.normal), ([3, 1, 3], [0, 1, 0]));
    assert!((hit.distance_m - 2.5).abs() < 1e-4);
    // Au ras, au-dessus de la dalle : le rayon passe et touche la torche,
    // qu'on vise même sans collision.
    let hit = raycast(&w, [0.5, 1.55, 3.5], [1.0, 0.0, 0.0], 10.0).unwrap();
    assert_eq!((hit.voxel, hit.normal), ([5, 1, 3], [-1, 0, 0]));
    assert!((hit.distance_m - (5.0 + 7.0 / 16.0 - 0.5)).abs() < 1e-4);
}
