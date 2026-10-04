//! Worldgen — pipeline pluggable (§4), volontairement bête pour la slice :
//! un bruit de hauteur → sol/air. La *forme* figée est le trait
//! [`WorldGenerator`] ; la richesse (biomes, features) viendra en implémentant
//! d'autres générateurs, pas en gonflant celui-ci.
//!
//! Deux invariants du doc s'appliquent ici :
//! - **Déterminisme (§2)** : tout dérive de la seed par hash entier — pas de
//!   RNG à état, pas de dépendance à l'ordre de génération des chunks. Le
//!   même (seed, position) donne toujours le même voxel, sur toute machine.
//! - **Gameplay en mètres (§2)** : hauteur du sol et amplitude du relief sont
//!   exprimées en mètres. `voxels_per_meter` ne fait que convertir à la fin.

use crate::chunk::{Chunk, ChunkPos};
use crate::registry::ContentId;

/// La forme figée du pipeline : un générateur produit un chunk depuis sa
/// position, de façon pure (pas d'état mutable → parallélisable, déterministe).
pub trait WorldGenerator {
    /// `chunk_size` et `voxels_per_meter` sont des métadonnées du monde (§3.5),
    /// passées par l'appelant : le générateur n'en garde pas de copie qui
    /// pourrait diverger de celle du monde.
    fn generate_chunk(&self, pos: ChunkPos, chunk_size: u32, voxels_per_meter: f32) -> Chunk;
}

/// Générateur de la tranche verticale : heightmap fBm → sol/air.
#[derive(Debug, Clone)]
pub struct HeightmapGenerator {
    pub seed: u64,
    /// Matériaux à poser — injectés depuis le registre, jamais hardcodés.
    pub air: ContentId,
    pub ground: ContentId,
    /// Altitude moyenne du sol, en **mètres**.
    pub ground_level_m: f32,
    /// Amplitude du relief autour de cette moyenne, en **mètres**.
    pub amplitude_m: f32,
    /// Largeur caractéristique des collines, en **mètres**.
    pub feature_size_m: f32,
}

impl HeightmapGenerator {
    /// Hauteur du sol en **mètres** à la position monde (x, z) en mètres.
    /// Publique pour que les tests (et plus tard le spawn du joueur)
    /// interrogent le terrain sans générer de chunk.
    pub fn height_m(&self, x_m: f32, z_m: f32) -> f32 {
        let n = fbm2(self.seed, x_m / self.feature_size_m, z_m / self.feature_size_m, 4);
        // fbm2 rend ~[0, 1] → recentré sur [-1, 1] puis mis à l'échelle.
        self.ground_level_m + (n * 2.0 - 1.0) * self.amplitude_m
    }
}

impl WorldGenerator for HeightmapGenerator {
    fn generate_chunk(&self, pos: ChunkPos, chunk_size: u32, voxels_per_meter: f32) -> Chunk {
        let mut chunk = Chunk::filled(chunk_size, self.air);
        let size = chunk_size as i64;
        let vpm = voxels_per_meter;

        for z in 0..chunk_size {
            for x in 0..chunk_size {
                // Coordonnées monde du centre de la colonne, en mètres.
                let wx = (pos.x as i64 * size + x as i64) as f32 / vpm;
                let wz = (pos.z as i64 * size + z as i64) as f32 / vpm;
                let h = self.height_m(wx, wz);
                // La colonne est solide sous la surface. On compare le centre
                // du voxel (y + 0.5) pour un arrondi symétrique.
                for y in 0..chunk_size {
                    let wy = ((pos.y as i64 * size + y as i64) as f32 + 0.5) / vpm;
                    if wy < h {
                        chunk.set(x, y, z, self.ground);
                    }
                }
            }
        }
        chunk
    }
}

// --- Bruit maison ---------------------------------------------------------
//
// Value noise 2D : un hash entier donne une valeur stable à chaque coin de
// cellule de la grille, interpolée en douceur entre les coins. Le fBm
// (fractional Brownian motion) superpose plusieurs octaves — chacune deux
// fois plus fine et deux fois plus faible — pour passer de « collines
// molles » à un relief avec du détail.
//
// Pourquoi maison plutôt qu'une crate ? Déterminisme garanti (hash entier,
// aucune table statique, aucun état), zéro dépendance, et c'est un des
// concepts qu'on veut comprendre en le construisant.

/// Hash 64-bit type SplitMix64 : avalanche forte, réversible, sans état.
/// C'est notre unique source d'« aléatoire » — tout le worldgen en dérive.
fn hash64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Valeur stable en [0, 1) au coin de grille (ix, iz) pour cette seed.
fn lattice(seed: u64, ix: i64, iz: i64) -> f32 {
    // Mélange coordonnées et seed en un seul u64 à hasher. Les multiplications
    // par de grands premiers évitent que (1,2) et (2,1) donnent le même hash.
    let h = hash64(
        seed ^ (ix as u64).wrapping_mul(0x8DA6_B343)
            ^ (iz as u64).wrapping_mul(0xD816_3841_5C58_35CB),
    );
    // 24 bits de poids fort → f32 uniforme dans [0, 1).
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// Interpolation smoothstep : dérivée nulle aux bornes, pas de cassure
/// visible aux frontières de cellules (contrairement au lerp brut).
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Value noise 2D, ~[0, 1].
fn value_noise2(seed: u64, x: f32, z: f32) -> f32 {
    let ix = x.floor() as i64;
    let iz = z.floor() as i64;
    let fx = x - ix as f32;
    let fz = z - iz as f32;

    let v00 = lattice(seed, ix, iz);
    let v10 = lattice(seed, ix + 1, iz);
    let v01 = lattice(seed, ix, iz + 1);
    let v11 = lattice(seed, ix + 1, iz + 1);

    let sx = smooth(fx);
    let sz = smooth(fz);
    let a = v00 + (v10 - v00) * sx;
    let b = v01 + (v11 - v01) * sx;
    a + (b - a) * sz
}

/// fBm : superposition d'octaves de value noise, normalisée vers ~[0, 1].
fn fbm2(seed: u64, x: f32, z: f32, octaves: u32) -> f32 {
    let mut total = 0.0;
    let mut amplitude = 1.0;
    let mut frequency = 1.0;
    let mut max = 0.0;
    for octave in 0..octaves {
        // Une seed dérivée par octave, sinon les octaves se ressemblent.
        let s = hash64(seed ^ octave as u64);
        total += value_noise2(s, x * frequency, z * frequency) * amplitude;
        max += amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
    }
    total / max
}

#[cfg(test)]
mod tests {
    use super::*;

    const AIR: ContentId = ContentId(0);
    const GROUND: ContentId = ContentId(1);

    fn generator(seed: u64) -> HeightmapGenerator {
        HeightmapGenerator {
            seed,
            air: AIR,
            ground: GROUND,
            ground_level_m: 16.0,
            amplitude_m: 6.0,
            feature_size_m: 24.0,
        }
    }

    fn chunks_equal(a: &Chunk, b: &Chunk) -> bool {
        let s = a.size();
        (0..s).all(|z| (0..s).all(|y| (0..s).all(|x| a.get(x, y, z) == b.get(x, y, z))))
    }

    #[test]
    fn same_seed_same_chunk() {
        let pos = ChunkPos { x: 3, y: 0, z: -2 };
        let a = generator(42).generate_chunk(pos, 16, 1.0);
        let b = generator(42).generate_chunk(pos, 16, 1.0);
        assert!(chunks_equal(&a, &b));
    }

    #[test]
    fn different_seeds_differ() {
        let pos = ChunkPos { x: 0, y: 0, z: 0 };
        let a = generator(1).generate_chunk(pos, 16, 1.0);
        let b = generator(2).generate_chunk(pos, 16, 1.0);
        assert!(!chunks_equal(&a, &b));
    }

    #[test]
    fn deep_chunk_is_solid_and_sky_chunk_is_air() {
        let generator = generator(42);
        // ground_level 16 m ± 6 m → y ∈ [10, 22]. Chunk y=-1 (y monde
        // [-16, 0)) : tout sous terre. Chunk y=2 (y monde [32, 48)) : tout ciel.
        let deep = generator.generate_chunk(ChunkPos { x: 0, y: -1, z: 0 }, 16, 1.0);
        let sky = generator.generate_chunk(ChunkPos { x: 0, y: 2, z: 0 }, 16, 1.0);
        assert!(chunks_equal(&deep, &Chunk::filled(16, GROUND)));
        assert!(chunks_equal(&sky, &Chunk::filled(16, AIR)));
    }

    #[test]
    fn surface_is_continuous_across_chunk_borders() {
        // La colonne monde (x=16, z=0) est la première du chunk x=1. Sa
        // surface doit coller à celle de (x=15) — dernière du chunk x=0 —
        // à un voxel près, sinon le bruit est discontinu aux frontières.
        let generator = generator(42);
        let left = generator.generate_chunk(ChunkPos { x: 0, y: 0, z: 0 }, 16, 1.0);
        let right = generator.generate_chunk(ChunkPos { x: 1, y: 0, z: 0 }, 16, 1.0);

        let surface = |chunk: &Chunk, x: u32| -> i32 {
            (0..16).rev().find(|&y| chunk.get(x, y, 0) == GROUND).map_or(-1, |y| y as i32)
        };
        let delta = (surface(&left, 15) - surface(&right, 0)).abs();
        assert!(delta <= 1, "surface discontinue au bord de chunk: Δ={delta}");
    }

    #[test]
    fn height_matches_generated_voxels() {
        // Le contenu du chunk doit être cohérent avec height_m : solide
        // sous la surface, air au-dessus.
        let generator = generator(7);
        let chunk = generator.generate_chunk(ChunkPos { x: 0, y: 0, z: 0 }, 16, 1.0);
        for (x, z) in [(0u32, 0u32), (5, 9), (15, 15)] {
            let h = generator.height_m(x as f32, z as f32);
            for y in 0..16u32 {
                let expected = if (y as f32 + 0.5) < h { GROUND } else { AIR };
                assert_eq!(chunk.get(x, y, z), expected, "à ({x},{y},{z}), h={h}");
            }
        }
    }
}
