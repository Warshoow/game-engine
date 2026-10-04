//! Le monde voxel — possède son contenu (§3.1) : registre + chunks + métadonnées.
//!
//! C'est la structure que la save sérialisera (§3.10) et que la physique et
//! le pose/casse interrogent. Elle parle deux langues, et la frontière est
//! nette :
//! - **coordonnées voxel monde** (`i64`) : la grille globale, pour
//!   lire/écrire des voxels ;
//! - **mètres** (`f32`) : le world-space où vivent entités et physique (§2).
//!   `voxels_per_meter` est l'unique pont entre les deux.

use std::collections::HashMap;

use crate::chunk::{Chunk, ChunkPos};
use crate::registry::{ContentId, Registry};

/// Nommé `VoxelWorld` (pas `World`) pour ne pas entrer en collision avec le
/// `World` de l'ECS côté app.
pub struct VoxelWorld {
    pub registry: Registry,
    chunk_size: u32,
    /// Gelé à la création (§3.5) : privé, sans setter — c'est le type qui
    /// porte l'invariant, pas la discipline. Changer la densité = autre monde.
    voxels_per_meter: f32,
    chunks: HashMap<ChunkPos, Chunk>,
}

impl VoxelWorld {
    pub fn new(registry: Registry, chunk_size: u32, voxels_per_meter: f32) -> Self {
        Self {
            registry,
            chunk_size,
            voxels_per_meter,
            chunks: HashMap::new(),
        }
    }

    pub fn voxels_per_meter(&self) -> f32 {
        self.voxels_per_meter
    }

    pub fn chunk_size(&self) -> u32 {
        self.chunk_size
    }

    pub fn insert_chunk(&mut self, pos: ChunkPos, chunk: Chunk) {
        debug_assert_eq!(chunk.size(), self.chunk_size);
        self.chunks.insert(pos, chunk);
    }

    pub fn chunk(&self, pos: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&pos)
    }

    /// Coordonnée voxel monde → (chunk, coordonnée locale).
    ///
    /// Division **euclidienne** obligatoire : pour x = −1 avec des chunks de
    /// 32, on veut le chunk −1 / local 31 — la division tronquée de Rust
    /// (`/`) donnerait chunk 0 / local −1. Classique source de bugs en
    /// coordonnées négatives.
    pub fn split(&self, v: [i64; 3]) -> (ChunkPos, [u32; 3]) {
        let s = self.chunk_size as i64;
        let pos = ChunkPos {
            x: v[0].div_euclid(s) as i32,
            y: v[1].div_euclid(s) as i32,
            z: v[2].div_euclid(s) as i32,
        };
        let local = [
            v[0].rem_euclid(s) as u32,
            v[1].rem_euclid(s) as u32,
            v[2].rem_euclid(s) as u32,
        ];
        (pos, local)
    }

    /// Le voxel en coordonnées voxel monde. `None` si le chunk n'est pas chargé.
    pub fn voxel(&self, v: [i64; 3]) -> Option<ContentId> {
        let (pos, l) = self.split(v);
        self.chunks.get(&pos).map(|c| c.get(l[0], l[1], l[2]))
    }

    /// Écrit un voxel ; retourne le chunk touché (à re-mesher) ou `None`
    /// si le chunk n'est pas chargé.
    pub fn set_voxel(&mut self, v: [i64; 3], material: ContentId) -> Option<ChunkPos> {
        let (pos, l) = self.split(v);
        let chunk = self.chunks.get_mut(&pos)?;
        chunk.set(l[0], l[1], l[2], material);
        Some(pos)
    }

    /// Solidité d'un voxel, résolue via le registre (data-driven).
    ///
    /// Un chunk **non chargé est traité comme de l'air** : assumé pour la
    /// slice (le monde généré couvre la zone de jeu). Quand le streaming
    /// arrivera, la physique devra plutôt refuser de simuler dans du
    /// non-chargé — sinon on tombe à travers le monde.
    pub fn is_solid(&self, v: [i64; 3]) -> bool {
        self.voxel(v)
            .and_then(|id| self.registry.get(id))
            .and_then(|e| e.block())
            .is_some_and(|b| b.solid)
    }

    /// Position monde en mètres → coordonnée du voxel qui la contient.
    pub fn voxel_at_m(&self, p: [f32; 3]) -> [i64; 3] {
        [
            (p[0] * self.voxels_per_meter).floor() as i64,
            (p[1] * self.voxels_per_meter).floor() as i64,
            (p[2] * self.voxels_per_meter).floor() as i64,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ContentEntry;

    fn world() -> (VoxelWorld, ContentId, ContentId) {
        world_with(1.0)
    }

    fn world_with(voxels_per_meter: f32) -> (VoxelWorld, ContentId, ContentId) {
        let mut reg = Registry::new();
        let air = reg
            .register(ContentEntry::new_block("core:air", false, [0.0; 3]))
            .unwrap();
        let stone = reg
            .register(ContentEntry::new_block("core:stone", true, [0.5; 3]))
            .unwrap();
        let mut w = VoxelWorld::new(reg, 8, voxels_per_meter);
        w.insert_chunk(ChunkPos { x: 0, y: 0, z: 0 }, Chunk::filled(8, air));
        (w, air, stone)
    }

    #[test]
    fn split_handles_negative_coords_euclidean() {
        let (w, _, _) = world();
        assert_eq!(w.split([-1, 0, 17]), (ChunkPos { x: -1, y: 0, z: 2 }, [7, 0, 1]));
        assert_eq!(w.split([-8, -9, 0]), (ChunkPos { x: -1, y: -2, z: 0 }, [0, 7, 0]));
    }

    #[test]
    fn set_and_read_voxel_through_world_coords() {
        let (mut w, _, stone) = world();
        let touched = w.set_voxel([3, 4, 5], stone);
        assert_eq!(touched, Some(ChunkPos { x: 0, y: 0, z: 0 }));
        assert_eq!(w.voxel([3, 4, 5]), Some(stone));
        assert!(w.is_solid([3, 4, 5]));
        assert!(!w.is_solid([3, 4, 6]));
    }

    #[test]
    fn unloaded_chunk_reads_as_air_and_rejects_writes() {
        let (mut w, _, stone) = world();
        assert_eq!(w.voxel([100, 0, 0]), None);
        assert!(!w.is_solid([100, 0, 0]));
        assert_eq!(w.set_voxel([100, 0, 0], stone), None);
    }

    #[test]
    fn meters_to_voxel_respects_resolution() {
        let (w, _, _) = world_with(2.0); // voxels de 0,5 m
        assert_eq!(w.voxel_at_m([1.6, -0.2, 0.0]), [3, -1, 0]);
    }
}
