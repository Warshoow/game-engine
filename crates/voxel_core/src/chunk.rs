//! Chunk paletté — §3.2 du design doc.
//!
//! Un voxel = un `material_id`, rien d'autre. Le chunk stocke un **tableau
//! dense d'indices `u16`** dans une **palette locale** qui mappe vers les
//! [`ContentId`] globaux du registre.
//!
//! Pourquoi une palette ? Un chunk 32³ = 32 768 voxels. En stockant l'ID
//! global (`u32`) partout : 128 KiB/chunk. En stockant un index `u16` local :
//! 64 KiB — et surtout la porte est ouverte vers des largeurs adaptatives
//! (1/2/4 bits) plus tard, car un chunk réel utilise rarement plus de
//! quelques dizaines de matériaux. La densité mémoire est le nerf de la
//! guerre (§3.2).

use crate::registry::ContentId;

/// Arête de chunk par défaut (§5, reco 32³). Le format stocke ses propres
/// dims (`Chunk::size`) : cette constante est un défaut, pas une hypothèse.
pub const CHUNK_SIZE: u32 = 32;

/// Position d'un chunk dans la grille de chunks — `i32` par décision §3.4
/// (monde fini par *policy*, pas par *type*).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// Un chunk cubique : palette locale + tableau dense d'indices.
#[derive(Debug, Clone)]
pub struct Chunk {
    size: u32,
    /// index local (u16) → ID global du registre.
    palette: Vec<ContentId>,
    /// Tableau dense, layout x-majeur : index = x + size·(y + size·z).
    /// Parcourir x en boucle interne = accès mémoire séquentiels.
    voxels: Vec<u16>,
}

impl Chunk {
    /// Crée un chunk rempli d'un seul matériau (typiquement l'air).
    /// La palette démarre avec cette unique entrée : coût mémoire minimal.
    pub fn filled(size: u32, material: ContentId) -> Self {
        let volume = (size as usize).pow(3);
        Self {
            size,
            palette: vec![material],
            voxels: vec![0; volume],
        }
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    #[inline]
    fn index(&self, x: u32, y: u32, z: u32) -> usize {
        debug_assert!(x < self.size && y < self.size && z < self.size);
        (x + self.size * (y + self.size * z)) as usize
    }

    /// ID global du matériau en (x, y, z) — coordonnées locales au chunk.
    #[inline]
    pub fn get(&self, x: u32, y: u32, z: u32) -> ContentId {
        self.palette[self.voxels[self.index(x, y, z)] as usize]
    }

    /// Index **local** (dans la palette) du voxel — pour les chemins chauds
    /// (mesher) qui résolvent la palette une fois puis travaillent en local,
    /// au lieu de re-mapper local → global → local à chaque voxel.
    #[inline]
    pub fn get_local(&self, x: u32, y: u32, z: u32) -> u16 {
        self.voxels[self.index(x, y, z)]
    }

    /// Écrit un matériau. La palette grandit à la demande (lookup-or-append).
    ///
    /// Note : on ne compacte pas la palette quand un matériau disparaît du
    /// chunk — optimisation ultérieure, inutile pour la slice.
    pub fn set(&mut self, x: u32, y: u32, z: u32, material: ContentId) {
        let local = self.palette_index_of(material);
        let idx = self.index(x, y, z);
        self.voxels[idx] = local;
    }

    fn palette_index_of(&mut self, material: ContentId) -> u16 {
        if let Some(i) = self.palette.iter().position(|&m| m == material) {
            return i as u16;
        }
        assert!(
            self.palette.len() < u16::MAX as usize,
            "palette de chunk pleine (>65k matériaux)"
        );
        self.palette.push(material);
        (self.palette.len() - 1) as u16
    }

    /// La palette locale (lecture seule) — utile au mesher pour résoudre
    /// les defs une fois par matériau plutôt qu'une fois par voxel.
    pub fn palette(&self) -> &[ContentId] {
        &self.palette
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AIR: ContentId = ContentId(0);
    const STONE: ContentId = ContentId(1);
    const DIRT: ContentId = ContentId(7);

    #[test]
    fn filled_chunk_returns_fill_material_everywhere() {
        let c = Chunk::filled(8, AIR);
        assert_eq!(c.get(0, 0, 0), AIR);
        assert_eq!(c.get(7, 7, 7), AIR);
        assert_eq!(c.palette(), &[AIR]);
    }

    #[test]
    fn set_get_roundtrip() {
        let mut c = Chunk::filled(8, AIR);
        c.set(3, 4, 5, STONE);
        assert_eq!(c.get(3, 4, 5), STONE);
        // Les voisins ne sont pas touchés.
        assert_eq!(c.get(3, 4, 4), AIR);
        assert_eq!(c.get(2, 4, 5), AIR);
    }

    #[test]
    fn palette_grows_on_demand_and_dedups() {
        let mut c = Chunk::filled(8, AIR);
        c.set(0, 0, 0, STONE);
        c.set(1, 0, 0, DIRT);
        c.set(2, 0, 0, STONE); // déjà en palette : pas de nouvelle entrée
        assert_eq!(c.palette(), &[AIR, STONE, DIRT]);
        assert_eq!(c.get(0, 0, 0), STONE);
        assert_eq!(c.get(1, 0, 0), DIRT);
        assert_eq!(c.get(2, 0, 0), STONE);
    }

    #[test]
    fn palette_indices_stay_local_to_the_chunk() {
        // Deux chunks avec des historiques différents mappent le même ID
        // global sur des index locaux différents — et ça reste correct.
        let mut a = Chunk::filled(4, AIR);
        let mut b = Chunk::filled(4, STONE);
        a.set(0, 0, 0, DIRT);
        b.set(0, 0, 0, DIRT);
        assert_eq!(a.get(0, 0, 0), DIRT);
        assert_eq!(b.get(0, 0, 0), DIRT);
        assert_eq!(b.get(1, 0, 0), STONE);
    }
}
