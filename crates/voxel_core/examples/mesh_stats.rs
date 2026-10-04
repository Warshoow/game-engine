//! Compare le coût du culling naïf et du greedy meshing sur les chunks
//! réels du worldgen (mêmes paramètres que le binaire).
//!
//! Headless : `cargo run -p voxel_core --example mesh_stats`

use voxel_core::chunk::{ChunkPos, CHUNK_SIZE};
use voxel_core::mesher::{mesh_chunk, mesh_chunk_naive};
use voxel_core::registry::{BlockData, ContentEntry, Kind, Registry};
use voxel_core::worldgen::{HeightmapGenerator, WorldGenerator};

fn main() {
    let mut registry = Registry::new();
    let air = registry
        .register(ContentEntry {
            identifier: "core:air".into(),
            kind: Kind::Block(BlockData { solid: false, color: [0.0; 3] }),
        })
        .unwrap();
    let grass = registry
        .register(ContentEntry {
            identifier: "core:grass".into(),
            kind: Kind::Block(BlockData { solid: true, color: [0.35, 0.6, 0.25] }),
        })
        .unwrap();

    // Mêmes paramètres que src/main.rs — on mesure le vrai terrain.
    let generator = HeightmapGenerator {
        seed: 42,
        air,
        ground: grass,
        ground_level_m: 16.0,
        amplitude_m: 6.0,
        feature_size_m: 24.0,
    };

    let (mut naive_total, mut greedy_total) = (0usize, 0usize);
    for cx in -2..=2 {
        for cz in -2..=2 {
            let pos = ChunkPos { x: cx, y: 0, z: cz };
            let chunk = generator.generate_chunk(pos, CHUNK_SIZE, 1.0);
            naive_total += mesh_chunk_naive(&chunk, &registry, 1.0).face_count();
            greedy_total += mesh_chunk(&chunk, &registry, 1.0).face_count();
        }
    }

    println!("25 chunks {CHUNK_SIZE}³ (seed 42) :");
    println!("  culling naïf : {naive_total} quads ({} triangles)", naive_total * 2);
    println!("  greedy       : {greedy_total} quads ({} triangles)", greedy_total * 2);
    println!(
        "  réduction    : ×{:.1} ({:.1} % de quads en moins)",
        naive_total as f64 / greedy_total as f64,
        100.0 * (1.0 - greedy_total as f64 / naive_total as f64)
    );
}
