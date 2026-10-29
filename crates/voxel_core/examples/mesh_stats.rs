//! Compare le coût du culling naïf et du greedy meshing sur les chunks
//! réels du worldgen (mêmes paramètres que le binaire). Le greedy ne
//! fusionne que des faces de même ombre (#7) : avant l'ombrage, 88 655 quads.
//! Puis les mêmes chunks dans un monde : raccord entre chunks et lumière
//! (#23), qui coupe aussi la fusion.
//!
//! Headless : `cargo run -p voxel_core --example mesh_stats`

use voxel_core::chunk::{ChunkPos, CHUNK_SIZE};
use voxel_core::mesher::{mesh_chunk, mesh_chunk_in_world, mesh_chunk_naive, Region};
use voxel_core::registry::{ContentEntry, Registry};
use voxel_core::world::VoxelWorld;
use voxel_core::worldgen::{HeightmapGenerator, WorldGenerator};

fn main() {
    let mut registry = Registry::new();
    let air = registry
        .register(ContentEntry::new_block("core:air", false, [0.0; 3]))
        .unwrap();
    let grass = registry
        .register(ContentEntry::new_block("core:grass", true, [0.35, 0.6, 0.25]))
        .unwrap();
    let stone = registry
        .register(ContentEntry::new_block("core:stone", true, [0.55, 0.55, 0.58]))
        .unwrap();

    // Mêmes paramètres que src/main.rs — on mesure le vrai terrain.
    let generator = HeightmapGenerator {
        seed: 42,
        air,
        ground: grass,
        stone,
        ground_level_m: 16.0,
        amplitude_m: 6.0,
        feature_size_m: 24.0,
    };

    // Couche de surface (y = 0) et couche souterraine (y = -1, grottes).
    let (mut naive_total, mut greedy_total) = (0usize, 0usize);
    let mut gen_time = std::time::Duration::ZERO;
    let mut mesh_time = std::time::Duration::ZERO;
    let mut chunks = Vec::new();
    // Du haut vers le bas, comme le streaming : la lumière du ciel descend.
    for cy in [0, -1] {
        for cx in -2..=2 {
            for cz in -2..=2 {
                let pos = ChunkPos { x: cx, y: cy, z: cz };
                let start = std::time::Instant::now();
                let chunk = generator.generate_chunk(pos, CHUNK_SIZE, 1.0);
                gen_time += start.elapsed();
                naive_total += mesh_chunk_naive(&chunk, &registry, 1.0).face_count();
                let start = std::time::Instant::now();
                greedy_total += mesh_chunk(&chunk, &registry, 1.0).face_count();
                mesh_time += start.elapsed();
                chunks.push((pos, chunk));
            }
        }
    }

    let positions: Vec<ChunkPos> = chunks.iter().map(|(pos, _)| *pos).collect();
    let mut world = VoxelWorld::new(registry.clone(), CHUNK_SIZE, 1.0);
    let start = std::time::Instant::now();
    for (pos, chunk) in chunks {
        world.insert_chunk(pos, chunk);
    }
    let light_time = start.elapsed();
    let start = std::time::Instant::now();
    let world_total: usize = positions.iter().map(|&pos| mesh_chunk_in_world(&world, pos, 1.0).unwrap()[0].face_count()).sum();
    let world_time = start.elapsed();
    let start = std::time::Instant::now();
    let regions: Vec<Region> = positions.iter().map(|&pos| Region::copy(&world, pos).unwrap()).collect();
    let copy_time = start.elapsed();
    drop(regions);

    println!("50 chunks {CHUNK_SIZE}³ (seed 42, couches y = -1 et 0) :");
    println!("  génération   : {:.2} ms/chunk", gen_time.as_secs_f64() * 1000.0 / 50.0);
    println!("  greedy       : {:.2} ms/chunk", mesh_time.as_secs_f64() * 1000.0 / 50.0);
    println!("  culling naïf : {naive_total} quads ({} triangles)", naive_total * 2);
    println!("  greedy       : {greedy_total} quads ({} triangles)", greedy_total * 2);
    println!(
        "  réduction    : ×{:.1} ({:.1} % de quads en moins)",
        naive_total as f64 / greedy_total as f64,
        100.0 * (1.0 - greedy_total as f64 / naive_total as f64)
    );
    println!("Dans un monde (raccord entre chunks + lumière) :");
    println!("  lumière      : {:.2} ms/chunk à l'arrivée", light_time.as_secs_f64() * 1000.0 / 50.0);
    println!("  greedy       : {:.2} ms/chunk, {world_total} quads", world_time.as_secs_f64() * 1000.0 / 50.0);
    println!("    dont copie : {:.2} ms/chunk (dans l'image, le reste en tâche de fond)", copy_time.as_secs_f64() * 1000.0 / 50.0);
}
