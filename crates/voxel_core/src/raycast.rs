//! Raycast voxel — DDA d'Amanatides & Woo (1987).
//!
//! Le besoin (§7.3) : depuis l'œil du joueur, savoir **quel voxel** le
//! regard touche et **par quelle face** il y entre (casser cible le voxel,
//! poser cible son voisin côté face).
//!
//! Pourquoi DDA et pas « avancer par petits pas » ? L'échantillonnage à pas
//! fixe rate des voxels quand le rayon frôle un coin (et gaspille des tests
//! partout ailleurs). Le DDA visite **exactement** la suite de voxels
//! traversés : à chaque itération, on saute à la prochaine frontière de
//! grille, sur l'axe dont la frontière est la plus proche le long du rayon.
//!
//! Entrées/sorties en **mètres** (§2) ; la grille n'apparaît que via
//! `voxels_per_meter`.

use crate::world::VoxelWorld;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    /// Le voxel solide touché (coordonnées voxel monde).
    pub voxel: [i64; 3],
    /// Normale de la face d'entrée (±1 sur un seul axe) — le voxel voisin
    /// `voxel + normal` est la case où *poser*. `[0,0,0]` si l'origine du
    /// rayon était déjà dans un solide.
    pub normal: [i32; 3],
    /// Distance parcourue jusqu'à la face, en mètres.
    pub distance_m: f32,
}

/// Lance un rayon depuis `origin_m` dans `dir` (pas besoin d'être normalisé)
/// et retourne le premier voxel solide dans `max_m` mètres.
pub fn raycast(world: &VoxelWorld, origin_m: [f32; 3], dir: [f32; 3], max_m: f32) -> Option<RayHit> {
    let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    if len == 0.0 {
        return None;
    }
    let dir = [dir[0] / len, dir[1] / len, dir[2] / len];
    let vpm = world.voxels_per_meter();

    // Tout le DDA travaille en espace voxel (1 unité = 1 voxel) ; les
    // distances t restent alors homogènes et se reconvertissent en mètres
    // par un seul facteur à la fin.
    let origin = [origin_m[0] * vpm, origin_m[1] * vpm, origin_m[2] * vpm];
    let max_t = max_m * vpm;

    // Voxel de départ.
    let mut voxel = [
        origin[0].floor() as i64,
        origin[1].floor() as i64,
        origin[2].floor() as i64,
    ];
    if world.is_solid(voxel) {
        return Some(RayHit { voxel, normal: [0; 3], distance_m: 0.0 });
    }

    // Pour chaque axe : le sens de traversée (step), la distance t jusqu'à
    // la première frontière (t_max), et le coût t d'une cellule entière
    // (t_delta). Un axe où dir = 0 ne franchit jamais de frontière → ∞.
    let mut step = [0i64; 3];
    let mut t_max = [f32::INFINITY; 3];
    let mut t_delta = [f32::INFINITY; 3];
    for a in 0..3 {
        if dir[a] > 0.0 {
            step[a] = 1;
            t_delta[a] = 1.0 / dir[a];
            t_max[a] = (voxel[a] as f32 + 1.0 - origin[a]) / dir[a];
        } else if dir[a] < 0.0 {
            step[a] = -1;
            t_delta[a] = -1.0 / dir[a];
            t_max[a] = (voxel[a] as f32 - origin[a]) / dir[a];
        }
    }

    loop {
        // Prochaine frontière : l'axe au t_max minimal.
        let axis = if t_max[0] < t_max[1] {
            if t_max[0] < t_max[2] { 0 } else { 2 }
        } else if t_max[1] < t_max[2] { 1 } else { 2 };

        let t = t_max[axis];
        if t > max_t {
            return None;
        }
        voxel[axis] += step[axis];
        t_max[axis] += t_delta[axis];

        if world.is_solid(voxel) {
            let mut normal = [0i32; 3];
            normal[axis] = -step[axis] as i32; // on entre par la face opposée au pas
            return Some(RayHit { voxel, normal, distance_m: t / vpm });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{Chunk, ChunkPos};
    use crate::registry::{ContentEntry, Registry};

    /// Monde 16³ (1 vox/m), sol plein pour y < 4.
    fn world() -> VoxelWorld {
        let mut reg = Registry::new();
        let air = reg
            .register(ContentEntry::new_block("core:air", false, [0.0; 3]))
            .unwrap();
        let stone = reg
            .register(ContentEntry::new_block("core:stone", true, [0.5; 3]))
            .unwrap();
        let mut chunk = Chunk::filled(16, air);
        for z in 0..16 {
            for x in 0..16 {
                for y in 0..4 {
                    chunk.set(x, y, z, stone);
                }
            }
        }
        let mut w = VoxelWorld::new(reg, 16, 1.0);
        w.insert_chunk(ChunkPos { x: 0, y: 0, z: 0 }, chunk);
        w
    }

    #[test]
    fn straight_down_hits_floor_top_face() {
        let w = world();
        let hit = raycast(&w, [4.5, 8.0, 4.5], [0.0, -1.0, 0.0], 10.0).unwrap();
        assert_eq!(hit.voxel, [4, 3, 4]);
        assert_eq!(hit.normal, [0, 1, 0]); // face du dessus → on poserait en [4,4,4]
        assert!((hit.distance_m - 4.0).abs() < 1e-4);
    }

    #[test]
    fn diagonal_ray_hits_floor() {
        let w = world();
        // 45° vers le bas en x : touche le sol (y=3→4) après 4 m de descente.
        let hit = raycast(&w, [2.5, 8.0, 4.5], [1.0, -1.0, 0.0], 20.0).unwrap();
        assert_eq!(hit.normal, [0, 1, 0]);
        assert_eq!(hit.voxel[1], 3);
        // x a avancé d'autant que y a descendu (~4 m) : voxel x = 6.
        assert_eq!(hit.voxel[0], 6);
    }

    #[test]
    fn ray_out_of_range_misses() {
        let w = world();
        assert_eq!(raycast(&w, [4.5, 8.0, 4.5], [0.0, -1.0, 0.0], 3.0), None);
    }

    #[test]
    fn ray_away_from_ground_misses() {
        let w = world();
        assert_eq!(raycast(&w, [4.5, 8.0, 4.5], [0.0, 1.0, 0.0], 100.0), None);
    }

    #[test]
    fn origin_inside_solid_reports_zero_distance() {
        let w = world();
        let hit = raycast(&w, [4.5, 2.5, 4.5], [0.0, 1.0, 0.0], 10.0).unwrap();
        assert_eq!(hit.voxel, [4, 2, 4]);
        assert_eq!(hit.normal, [0, 0, 0]);
        assert_eq!(hit.distance_m, 0.0);
    }

    #[test]
    fn grazing_corner_does_not_skip_voxels() {
        // Rayon presque rasant au-dessus du sol qui finit par plonger : le
        // DDA doit toucher la face du DESSUS (normale +y), jamais un flanc
        // « de l'intérieur » — symptôme classique d'un pas d'échantillonnage.
        let w = world();
        let hit = raycast(&w, [0.1, 4.05, 0.1], [1.0, -0.01, 0.3], 30.0).unwrap();
        assert_eq!(hit.normal, [0, 1, 0]);
        assert_eq!(hit.voxel[1], 3);
    }

    #[test]
    fn resolution_independent_distances() {
        // Même géométrie en mètres, résolution double : la distance du hit
        // ne change pas (invariant §2 — la résolution ne fuit pas).
        let mut reg = Registry::new();
        let air = reg
            .register(ContentEntry::new_block("core:air", false, [0.0; 3]))
            .unwrap();
        let stone = reg
            .register(ContentEntry::new_block("core:stone", true, [0.5; 3]))
            .unwrap();
        // 2 vox/m : le sol y < 4 voxels = y < 2 m.
        let mut chunk = Chunk::filled(16, air);
        for z in 0..16 {
            for x in 0..16 {
                for y in 0..4 {
                    chunk.set(x, y, z, stone);
                }
            }
        }
        let mut w = VoxelWorld::new(reg, 16, 2.0);
        w.insert_chunk(ChunkPos { x: 0, y: 0, z: 0 }, chunk);

        let hit = raycast(&w, [2.0, 5.0, 2.0], [0.0, -1.0, 0.0], 10.0).unwrap();
        assert!((hit.distance_m - 3.0).abs() < 1e-4); // 5 m → sol à 2 m
        assert_eq!(hit.voxel, [4, 3, 4]); // en coordonnées VOXEL (2 vox/m)
    }
}
