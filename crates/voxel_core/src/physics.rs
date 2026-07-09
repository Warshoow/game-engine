//! Collision AABB vs grille voxel — le minimum vital pour §7.4.
//!
//! L'approche est celle de Minecraft : **résolution axe par axe**. On déplace
//! la boîte sur X, on la bloque si elle chevauche un voxel solide, puis
//! pareil sur Y, puis Z. Trois passes 1D au lieu d'un vrai solveur 3D — et
//! le glissement le long des murs tombe gratuitement : l'axe bloqué se
//! clampe, les autres continuent.
//!
//! Tout est en **mètres** (§2). La grille voxel n'apparaît qu'à travers
//! `VoxelWorld::is_solid`, converti via `voxels_per_meter`.
//!
//! Chaque passe teste la **région balayée** (de la position de départ à
//! l'arrivée), pas seulement la destination — sinon un objet rapide
//! « saute » par-dessus un obstacle fin en un tick (tunneling). Le balayage
//! par axe reste approximatif pour un mouvement diagonal très rapide, mais
//! il est exact pour les vitesses de jeu à 64 ticks/s.

use crate::world::VoxelWorld;

/// Marge de peau : la boîte s'arrête un chouïa avant la face du voxel, pour
/// que les floats ne la fassent pas re-chevaucher au tick suivant.
const SKIN: f32 = 1e-4;

/// Boîte englobante alignée sur les axes, en mètres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Aabb {
    /// Boîte d'un personnage : `feet` = point au sol au centre de la boîte.
    pub fn from_feet(feet: [f32; 3], width: f32, height: f32) -> Self {
        let half = width / 2.0;
        Self {
            min: [feet[0] - half, feet[1], feet[2] - half],
            max: [feet[0] + half, feet[1] + height, feet[2] + half],
        }
    }

    pub fn feet(&self) -> [f32; 3] {
        [
            (self.min[0] + self.max[0]) / 2.0,
            self.min[1],
            (self.min[2] + self.max[2]) / 2.0,
        ]
    }

    fn translated(mut self, axis: usize, d: f32) -> Self {
        self.min[axis] += d;
        self.max[axis] += d;
        self
    }
}

/// Résultat d'un déplacement : la boîte arrivée et, par axe, si elle a tapé.
/// `collided[1]` avec un mouvement descendant = « au sol » (pour le saut).
#[derive(Debug, Clone, Copy)]
pub struct MoveResult {
    pub aabb: Aabb,
    pub collided: [bool; 3],
}

/// Déplace `aabb` de `delta` (mètres), en s'arrêtant contre les voxels
/// solides, axe par axe.
pub fn move_and_collide(world: &VoxelWorld, aabb: Aabb, delta: [f32; 3]) -> MoveResult {
    let mut result = MoveResult {
        aabb,
        collided: [false; 3],
    };

    for (axis, &d) in delta.iter().enumerate() {
        if d == 0.0 {
            continue;
        }
        let orig = result.aabb;

        // Région balayée : sur `axis`, tout le chemin départ → arrivée ;
        // sur les autres axes, l'emprise actuelle de la boîte.
        let mut swept = orig.translated(axis, d);
        swept.min[axis] = swept.min[axis].min(orig.min[axis]);
        swept.max[axis] = swept.max[axis].max(orig.max[axis]);

        match first_blocking_face(world, &orig, &swept, axis, d) {
            None => result.aabb = orig.translated(axis, d),
            Some(face) => {
                // Clampe le bord avant de la boîte contre la face, moins la
                // peau ; l'autre bord suit (l'étendue ne change pas).
                let extent = orig.max[axis] - orig.min[axis];
                let mut b = orig;
                if d > 0.0 {
                    b.max[axis] = face - SKIN;
                    b.min[axis] = b.max[axis] - extent;
                } else {
                    b.min[axis] = face + SKIN;
                    b.max[axis] = b.min[axis] + extent;
                }
                result.aabb = b;
                result.collided[axis] = true;
            }
        }
    }
    result
}

/// Parmi les voxels solides de la région balayée, trouve la face la plus
/// proche **devant** la boîte dans le sens du mouvement. Retourne sa
/// position en mètres.
fn first_blocking_face(
    world: &VoxelWorld,
    orig: &Aabb,
    swept: &Aabb,
    axis: usize,
    d: f32,
) -> Option<f32> {
    let vpm = world.voxels_per_meter;
    // Plage de voxels chevauchés par la région. Borne haute : une boîte dont
    // le max tombe pile sur une frontière (x = 3.0) ne chevauche PAS le
    // voxel 3 — d'où le retrait d'un epsilon avant le floor.
    let lo = |v: f32| (v * vpm).floor() as i64;
    let hi = |v: f32| ((v * vpm) - 1e-6).floor() as i64;

    let (l, h) = (
        [lo(swept.min[0]), lo(swept.min[1]), lo(swept.min[2])],
        [hi(swept.max[0]), hi(swept.max[1]), hi(swept.max[2])],
    );

    let mut best: Option<f32> = None;
    for x in l[0]..=h[0] {
        for y in l[1]..=h[1] {
            for z in l[2]..=h[2] {
                if !world.is_solid([x, y, z]) {
                    continue;
                }
                let c = [x, y, z][axis];
                if d > 0.0 {
                    let face = c as f32 / vpm; // face « min » du voxel
                    // Seuls les voxels DEVANT la boîte bloquent ; ceux déjà
                    // au niveau de la boîte (marge de peau) sont ignorés,
                    // sinon on resterait collé définitivement.
                    if face >= orig.max[axis] - 2.0 * SKIN {
                        best = Some(best.map_or(face, |b: f32| b.min(face)));
                    }
                } else {
                    let face = (c + 1) as f32 / vpm; // face « max » du voxel
                    if face <= orig.min[axis] + 2.0 * SKIN {
                        best = Some(best.map_or(face, |b: f32| b.max(face)));
                    }
                }
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{Chunk, ChunkPos};
    use crate::registry::{BlockData, ContentEntry, Kind, Registry};

    /// Monde 16³ (1 vox/m) avec un sol solide en y ∈ [0, 4) et un mur x = 8.
    fn world() -> VoxelWorld {
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
                block: Some(BlockData { solid: true, color: [0.5; 3] }),
            })
            .unwrap();

        let mut chunk = Chunk::filled(16, air);
        for z in 0..16 {
            for x in 0..16 {
                for y in 0..4 {
                    chunk.set(x, y, z, stone); // sol
                }
                for y in 4..12 {
                    chunk.set(8, y, z, stone); // mur x = 8
                }
            }
        }
        let mut w = VoxelWorld::new(reg, 16, 1.0);
        w.insert_chunk(ChunkPos { x: 0, y: 0, z: 0 }, chunk);
        w
    }

    const _: () = assert!(SKIN < 1e-2, "SKIN doit rester négligeable");

    fn player_at(feet: [f32; 3]) -> Aabb {
        Aabb::from_feet(feet, 0.6, 1.8)
    }

    #[test]
    fn falls_and_lands_on_floor_top() {
        let w = world();
        let r = move_and_collide(&w, player_at([4.0, 8.0, 4.0]), [0.0, -10.0, 0.0]);
        assert!(r.collided[1]);
        // Le dessus du sol est à y = 4 (voxels 0..4 pleins).
        assert!((r.aabb.min[1] - 4.0).abs() < 1e-3, "min.y = {}", r.aabb.min[1]);
    }

    #[test]
    fn free_move_is_unobstructed() {
        let w = world();
        let r = move_and_collide(&w, player_at([4.0, 4.5, 4.0]), [1.0, 0.0, 1.5]);
        assert_eq!(r.collided, [false; 3]);
        let feet = r.aabb.feet();
        assert!((feet[0] - 5.0).abs() < 1e-5 && (feet[2] - 5.5).abs() < 1e-5);
    }

    #[test]
    fn wall_blocks_x_but_lets_z_slide() {
        let w = world();
        // Marche en diagonale vers le mur x=8 : X se clampe, Z continue.
        let r = move_and_collide(&w, player_at([6.0, 4.5, 4.0]), [4.0, 0.0, 2.0]);
        assert!(r.collided[0] && !r.collided[2]);
        // La face du joueur (demi-largeur 0,3) s'arrête au mur x = 8.
        assert!((r.aabb.max[0] - 8.0).abs() < 1e-3, "max.x = {}", r.aabb.max[0]);
        assert!((r.aabb.feet()[2] - 6.0).abs() < 1e-5);
    }

    #[test]
    fn fast_move_does_not_tunnel_through_wall() {
        let w = world();
        // Destination x ≈ 14 : de l'air, au-delà du mur x = 8. Un test qui
        // ne regarderait que l'arrivée laisserait passer — c'est la région
        // balayée qui doit bloquer.
        let r = move_and_collide(&w, player_at([2.0, 4.5, 4.0]), [12.0, 0.0, 0.0]);
        assert!(r.collided[0]);
        assert!((r.aabb.max[0] - 8.0).abs() < 1e-3, "max.x = {}", r.aabb.max[0]);
    }

    #[test]
    fn landing_then_walking_does_not_sink() {
        // Après un atterrissage (avec la peau), marcher ne doit pas faire
        // re-chevaucher le sol — régression classique liée aux floats.
        let w = world();
        let landed = move_and_collide(&w, player_at([4.0, 6.0, 4.0]), [0.0, -5.0, 0.0]);
        let walked = move_and_collide(&w, landed.aabb, [0.5, 0.0, 0.0]);
        assert!(!walked.collided[0] && !walked.collided[2]);
        assert!(walked.aabb.min[1] >= 4.0 - 1e-3);
    }
}
