//! Le monde voxel — possède son contenu (§3.1) : registre + chunks + métadonnées.
//!
//! C'est la structure que la save sérialisera (§3.10) et que la physique et
//! le pose/casse interrogent. Elle parle deux langues, et la frontière est
//! nette :
//! - **coordonnées voxel monde** (`i64`) : la grille globale, pour
//!   lire/écrire des voxels ;
//! - **mètres** (`f32`) : le world-space où vivent entités et physique (§2).
//!   `voxels_per_meter` est l'unique pont entre les deux.

use std::collections::{HashMap, HashSet};

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
    /// Block-entities (§3.3) : canal creux position voxel → items posés sur
    /// le bloc. Seuls les blocs dont l'entrée déclare `storage` y ont une
    /// ligne, créée à la pose, supprimée quand le voxel change.
    // ponytail: une map pour tout le monde, filtrée par chunk à la save
    // (scan de toute la map) ; par chunk si les établis se comptent par
    // milliers.
    block_entities: HashMap<[i64; 3], Vec<ContentId>>,
    /// Chunks modifiés depuis le dernier [`Self::take_edited`] : à écrire
    /// dans la save.
    edited: HashSet<ChunkPos>,
}

impl VoxelWorld {
    pub fn new(registry: Registry, chunk_size: u32, voxels_per_meter: f32) -> Self {
        Self {
            registry,
            chunk_size,
            voxels_per_meter,
            chunks: HashMap::new(),
            block_entities: HashMap::new(),
            edited: HashSet::new(),
        }
    }

    /// Insère un chunk lu dans la save, avec ses block-entities (positions
    /// locales au chunk).
    pub fn insert_saved_chunk(&mut self, pos: ChunkPos, chunk: Chunk, entities: Vec<([u32; 3], Vec<ContentId>)>) {
        for (local, items) in entities {
            self.block_entities.insert(self.to_world(pos, local), items);
        }
        self.insert_chunk(pos, chunk);
    }

    /// Les block-entities du chunk `pos`, en positions locales.
    pub fn chunk_block_entities(&self, pos: ChunkPos) -> Vec<([u32; 3], Vec<ContentId>)> {
        self.block_entities
            .iter()
            .filter_map(|(&v, items)| {
                let (p, local) = self.split(v);
                (p == pos).then(|| (local, items.clone()))
            })
            .collect()
    }

    /// Positions monde des block-entities du chunk `pos`.
    pub fn block_entity_positions(&self, pos: ChunkPos) -> Vec<[i64; 3]> {
        self.block_entities.keys().copied().filter(|&v| self.split(v).0 == pos).collect()
    }

    /// Les chunks modifiés depuis le dernier appel, et oublie la liste.
    pub fn take_edited(&mut self) -> Vec<ChunkPos> {
        self.edited.drain().collect()
    }

    fn to_world(&self, pos: ChunkPos, local: [u32; 3]) -> [i64; 3] {
        let s = self.chunk_size as i64;
        [
            pos.x as i64 * s + local[0] as i64,
            pos.y as i64 * s + local[1] as i64,
            pos.z as i64 * s + local[2] as i64,
        ]
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
    ///
    /// Cycle de vie des block-entities (§3.3) : l'état de l'ancien bloc
    /// disparaît (récupérer son contenu avant, avec [`Self::take_stored`]),
    /// un nouvel état vide est créé si le nouveau bloc déclare `storage`.
    pub fn set_voxel(&mut self, v: [i64; 3], material: ContentId) -> Option<ChunkPos> {
        let (pos, l) = self.split(v);
        let chunk = self.chunks.get_mut(&pos)?;
        chunk.set(l[0], l[1], l[2], material);
        self.block_entities.remove(&v);
        if self.capacity(material).is_some() {
            self.block_entities.insert(v, Vec::new());
        }
        self.edited.insert(pos);
        Some(pos)
    }

    fn capacity(&self, id: ContentId) -> Option<u32> {
        self.registry.get(id)?.block()?.storage
    }

    /// Les items posés sur le bloc `v`, s'il a un état.
    pub fn stored(&self, v: [i64; 3]) -> Option<&[ContentId]> {
        self.block_entities.get(&v).map(Vec::as_slice)
    }

    /// Pose un item sur le bloc `v`. `false` si le bloc n'a pas d'état ou
    /// s'il est plein.
    pub fn store(&mut self, v: [i64; 3], item: ContentId) -> bool {
        let Some(capacity) = self.voxel(v).and_then(|id| self.capacity(id)) else { return false };
        let pos = self.split(v).0;
        match self.block_entities.get_mut(&v) {
            Some(items) if (items.len() as u32) < capacity => {
                items.push(item);
                self.edited.insert(pos);
                true
            }
            _ => false,
        }
    }

    /// Vide le bloc `v` et rend son contenu (vide s'il n'a pas d'état).
    pub fn take_stored(&mut self, v: [i64; 3]) -> Vec<ContentId> {
        let items = self.block_entities.get_mut(&v).map(std::mem::take).unwrap_or_default();
        if !items.is_empty() {
            self.edited.insert(self.split(v).0);
        }
        items
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

#[cfg(test)]
mod block_entity_tests {
    use super::*;
    use crate::chunk::Chunk;

    #[test]
    fn storage_lives_with_the_block() {
        let reg = Registry::from_ron(
            r#"[
                (identifier: "t:air", kind: Block((solid: false, color: (0.0, 0.0, 0.0)))),
                (identifier: "t:bench", kind: Block((solid: true, color: (1.0, 1.0, 1.0), storage: Some(2)))),
                (identifier: "t:stone", kind: Block((solid: true, color: (1.0, 1.0, 1.0)))),
            ]"#,
        )
        .unwrap();
        let id = |s| reg.lookup(s).unwrap();
        let (air, bench, stone) = (id("t:air"), id("t:bench"), id("t:stone"));
        let mut w = VoxelWorld::new(reg, 16, 1.0);
        w.insert_chunk(ChunkPos { x: 0, y: 0, z: 0 }, Chunk::filled(16, air));
        let v = [1, 2, 3];

        // Pas d'état sur un bloc sans `storage`.
        w.set_voxel(v, stone);
        assert_eq!(w.stored(v), None);
        assert!(!w.store(v, stone));

        // Établi posé : état vide, capacité 2.
        w.set_voxel(v, bench);
        assert_eq!(w.stored(v), Some(&[][..]));
        assert!(w.store(v, stone) && w.store(v, stone));
        assert!(!w.store(v, stone), "plein");
        assert_eq!(w.take_stored(v), vec![stone, stone]);
        assert_eq!(w.stored(v), Some(&[][..]));

        // Le voxel change : l'état disparaît avec l'établi.
        w.store(v, stone);
        w.set_voxel(v, air);
        assert_eq!(w.stored(v), None);
    }
}
