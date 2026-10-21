//! Ticks de bloc (§3.9, #34) : des blocs qui changent d'eux-mêmes.
//!
//! **Tick aléatoire**, à la Minecraft : à chaque tick fixe, quelques voxels
//! tirés au hasard dans chaque chunk reçoivent le hook `Tick`. Rien n'est
//! mémorisé par bloc (pas de minuteur à sauver) et le coût ne dépend pas du
//! nombre de blocs qui poussent : il est fixe par chunk. Un bloc donné
//! reçoit un tick en moyenne toutes les `chunk_volume / picks` ticks fixes ;
//! la condition `Chance` règle la vitesse propre à chaque contenu.
//!
//! Le tirage dérive de la seed et du numéro de tick : même monde, même
//! tick → mêmes voxels (§2, déterminisme).

use crate::chunk::ChunkPos;
use crate::rules::{self, Hook};
use crate::world::VoxelWorld;
use crate::worldgen::hash64;

/// Les voxels qui reçoivent un tick ce coup-ci, parmi les chunks `chunks` :
/// `picks` tirages par chunk, dont on ne garde que les blocs qui ont une
/// règle `Tick`. Ordre stable (chunks triés), quel que soit l'ordre reçu.
pub fn pick(world: &VoxelWorld, chunks: &[ChunkPos], picks: u32, seed: u64) -> Vec<[i64; 3]> {
    let mut chunks = chunks.to_vec();
    chunks.sort_by_key(|p| (p.x, p.y, p.z));
    let size = world.chunk_size();
    let mut picked = Vec::new();
    for pos in chunks {
        let Some(chunk) = world.chunk(pos) else { continue };
        // La plupart des chunks n'ont rien qui pousse : on le voit à la
        // palette, sans tirer un seul voxel.
        let ticking: Vec<bool> =
            chunk.palette().iter().map(|&id| rules::has_rule(&world.registry, id, Hook::Tick)).collect();
        if !ticking.contains(&true) {
            continue;
        }
        let chunk_seed = [pos.x, pos.y, pos.z].iter().fold(seed, |h, &c| hash64(h ^ c as u64));
        for i in 0..picks {
            let h = hash64(chunk_seed ^ hash64(u64::from(i)));
            let [x, y, z] = [h, h >> 21, h >> 42].map(|v| (v % u64::from(size)) as u32);
            if ticking[chunk.get_local(x, y, z) as usize] {
                let s = i64::from(size);
                picked.push([
                    i64::from(pos.x) * s + i64::from(x),
                    i64::from(pos.y) * s + i64::from(y),
                    i64::from(pos.z) * s + i64::from(z),
                ]);
            }
        }
    }
    picked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Chunk;
    use crate::registry::Registry;

    #[test]
    fn only_ticking_blocks_are_picked_and_the_draw_repeats() {
        let reg = Registry::from_ron(
            r#"[
                (identifier: "t:air", kind: Block((solid: false, color: (0.0, 0.0, 0.0)))),
                (identifier: "t:seed", kind: Block((solid: true, color: (1.0, 1.0, 1.0),
                    rules: [(on: Tick, then: [ReplaceSelf("t:air")])]))),
            ]"#,
        )
        .unwrap();
        let (air, seed) = (reg.lookup("t:air").unwrap(), reg.lookup("t:seed").unwrap());
        let mut world = VoxelWorld::new(reg, 4, 1.0);
        let (a, b) = (ChunkPos { x: 0, y: 0, z: 0 }, ChunkPos { x: 1, y: 0, z: 0 });
        // Chunk `a` : moitié graines (x < 2). Chunk `b` : que de l'air.
        let mut chunk = Chunk::filled(4, air);
        for (x, y, z) in (0..2).flat_map(|x| (0..4).flat_map(move |y| (0..4).map(move |z| (x, y, z)))) {
            chunk.set(x, y, z, seed);
        }
        world.insert_chunk(a, chunk);
        world.insert_chunk(b, Chunk::filled(4, air));

        let picked = pick(&world, &[b, a], 64, 7);
        assert!(!picked.is_empty());
        assert!(picked.iter().all(|v| world.voxel(*v) == Some(seed)));
        // Environ la moitié des 64 tirages tombe sur une graine.
        assert!((16..48).contains(&picked.len()), "{}", picked.len());
        assert_eq!(picked, pick(&world, &[a, b], 64, 7));
        assert_ne!(picked, pick(&world, &[a, b], 64, 8));
    }
}
