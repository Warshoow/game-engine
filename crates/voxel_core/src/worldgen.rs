//! Worldgen — pipeline pluggable (§4), volontairement simple : un bruit de
//! hauteur → sol/air, de la pierre sous une couche de sol, et des grottes
//! creusées par un bruit 3D. La *forme* figée est le trait
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

/// Épaisseur de la couche de `ground` au-dessus de la pierre, en mètres.
const SOIL_DEPTH_M: f32 = 1.0;
/// Taille caractéristique des grottes (longueur d'un coude de tunnel), en mètres.
const CAVE_SIZE_M: f32 = 16.0;
/// Demi-largeur de la bande « proche de 0,5 » dans chaque bruit de grotte.
/// Plus large = tunnels plus gros et plus nombreux.
const CAVE_BAND: f32 = 0.06;

/// Générateur : heightmap fBm → sol/air, pierre dessous, grottes.
#[derive(Debug, Clone)]
pub struct HeightmapGenerator {
    pub seed: u64,
    /// Matériaux à poser — injectés depuis le registre, jamais hardcodés.
    pub air: ContentId,
    pub ground: ContentId,
    /// Sous la couche de sol (et autour des grottes).
    pub stone: ContentId,
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

    /// Le point (mètres) est-il dans un tunnel ?
    ///
    /// Un bruit 3D vaut ~0,5 sur une **surface** ondulée de l'espace ; la
    /// bande `|n − 0,5| < CAVE_BAND` est une plaque épaisse autour de cette
    /// surface. Deux bruits indépendants donnent deux plaques, et leur
    /// **intersection** est un tube qui serpente : un tunnel. Un seul bruit
    /// seuillé (`n > s`) donnerait plutôt des bulles isolées.
    pub fn is_cave_m(&self, x_m: f32, y_m: f32, z_m: f32) -> bool {
        let (x, y, z) = (x_m / CAVE_SIZE_M, y_m / CAVE_SIZE_M, z_m / CAVE_SIZE_M);
        let a = fbm3(hash64(self.seed ^ 0xCA7E_0001), x, y, z, 2);
        if (a - 0.5).abs() >= CAVE_BAND {
            return false; // le second bruit n'est calculé que si besoin
        }
        let b = fbm3(hash64(self.seed ^ 0xCA7E_0002), x, y, z, 2);
        (b - 0.5).abs() < CAVE_BAND
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
                    if wy >= h || self.is_cave_m(wx, wy, wz) {
                        continue; // ciel ou grotte : reste de l'air
                    }
                    let material = if wy < h - SOIL_DEPTH_M { self.stone } else { self.ground };
                    chunk.set(x, y, z, material);
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

/// Valeur stable en [0, 1) au coin de grille 3D (ix, iy, iz).
fn lattice3(seed: u64, ix: i64, iy: i64, iz: i64) -> f32 {
    lattice(seed ^ (iy as u64).wrapping_mul(0x9FB2_1C65_1E98_DF25), ix, iz)
}

/// Value noise 3D, ~[0, 1] : le 2D avec un axe de plus — 8 coins au lieu
/// de 4, interpolés en x, puis y, puis z.
fn value_noise3(seed: u64, x: f32, y: f32, z: f32) -> f32 {
    let (ix, iy, iz) = (x.floor() as i64, y.floor() as i64, z.floor() as i64);
    let (sx, sy, sz) = (
        smooth(x - ix as f32),
        smooth(y - iy as f32),
        smooth(z - iz as f32),
    );
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let plane = |dz: i64| {
        let row = |dy: i64| {
            lerp(
                lattice3(seed, ix, iy + dy, iz + dz),
                lattice3(seed, ix + 1, iy + dy, iz + dz),
                sx,
            )
        };
        lerp(row(0), row(1), sy)
    };
    lerp(plane(0), plane(1), sz)
}

/// fBm 3D — même recette que [`fbm2`].
fn fbm3(seed: u64, x: f32, y: f32, z: f32, octaves: u32) -> f32 {
    let (mut total, mut amplitude, mut frequency, mut max) = (0.0, 1.0, 1.0, 0.0);
    for octave in 0..octaves {
        let s = hash64(seed ^ octave as u64);
        total += value_noise3(s, x * frequency, y * frequency, z * frequency) * amplitude;
        max += amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
    }
    total / max
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
    const STONE: ContentId = ContentId(2);

    fn generator(seed: u64) -> HeightmapGenerator {
        HeightmapGenerator {
            seed,
            air: AIR,
            ground: GROUND,
            stone: STONE,
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
    fn underground_is_stone_with_caves_and_sky_is_air() {
        let generator = generator(42);
        // ground_level 16 m ± 6 m → y ∈ [10, 22]. Chunks y=-1 (y monde
        // [-16, 0)) : sous terre. Chunk y=2 (y monde [32, 48)) : tout ciel.
        let sky = generator.generate_chunk(ChunkPos { x: 0, y: 2, z: 0 }, 16, 1.0);
        assert!(chunks_equal(&sky, &Chunk::filled(16, AIR)));

        let (mut stone, mut air) = (0, 0);
        for x in 0..4 {
            let deep = generator.generate_chunk(ChunkPos { x, y: -1, z: 0 }, 16, 1.0);
            for (zz, yy, xx) in (0..16).flat_map(|z| (0..16).flat_map(move |y| (0..16).map(move |x| (z, y, x)))) {
                match deep.get(xx, yy, zz) {
                    STONE => stone += 1,
                    AIR => air += 1,
                    other => panic!("sous terre : ni pierre ni grotte ({other:?})"),
                }
            }
        }
        // Des grottes, mais la roche domine.
        assert!(air > 0, "aucune grotte");
        assert!(air * 5 < stone, "trop de vide : {air} air / {stone} pierre");
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
    fn chunks_match_height_and_caves_in_world_coordinates() {
        // Chaque voxel doit valoir ce que disent height_m et is_cave_m à sa
        // position MONDE. Chunks hors origine, y négatif compris : attrape
        // un calcul fait en coordonnées locales au lieu de monde — qui
        // casserait les grottes et le relief aux frontières de chunk.
        let generator = generator(7);
        for pos in [ChunkPos { x: 2, y: 0, z: -3 }, ChunkPos { x: 2, y: -1, z: -3 }] {
            let chunk = generator.generate_chunk(pos, 16, 1.0);
            for (x, y, z) in (0..16u32).flat_map(|x| (0..16u32).flat_map(move |y| (0..16u32).map(move |z| (x, y, z)))) {
                let wx = (pos.x * 16 + x as i32) as f32;
                let wy = (pos.y * 16 + y as i32) as f32 + 0.5;
                let wz = (pos.z * 16 + z as i32) as f32;
                let h = generator.height_m(wx, wz);
                let expected = if wy >= h || generator.is_cave_m(wx, wy, wz) {
                    AIR
                } else if wy < h - SOIL_DEPTH_M {
                    STONE
                } else {
                    GROUND
                };
                assert_eq!(chunk.get(x, y, z), expected, "{pos:?} ({x},{y},{z}), h={h}");
            }
        }
    }
}
