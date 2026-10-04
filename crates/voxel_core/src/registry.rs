//! Registre de contenu — §3.1 du design doc.
//!
//! Un registre **unique**, **append-only**, à **kinds unifiés**, destiné à être
//! **sérialisé dans la save** (world-owned). Les IDs entiers sont stables :
//! une fois attribués, ils ne bougent jamais — c'est eux que les palettes de
//! chunk référencent.
//!
//! Append-only par *construction* : l'API n'expose ni suppression ni
//! réordonnancement. C'est le type qui porte l'invariant, pas la discipline.

/// ID entier stable d'une entrée du registre.
///
/// `u32` : la palette de chunk mappe ses indices locaux (`u16`) vers ces IDs
/// globaux, donc l'ID global peut être large sans coût mémoire per-voxel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentId(pub u32);

/// Catégorie d'entrée — kinds unifiés (§3.1) : une seule table pour tout.
///
/// Chaque variante porte ses propres données : un `Block` sans `BlockData`
/// (ou un `Item` avec) est irreprésentable — c'est le type qui porte
/// l'invariant, pas la discipline de l'appelant.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Block(BlockData),
    Item,
    EntityType,
}

/// Données spécifiques aux blocs, lues par le mesher et la physique.
///
/// Volontairement minimal pour la tranche verticale : le comportement riche
/// viendra par la couche script/data (§3.6), pas en gonflant cette struct.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockData {
    /// Un bloc non-solide (air…) n'est ni meshé ni collidable.
    pub solid: bool,
    /// Couleur de base RGB — suffit pour la slice (pas de textures encore).
    pub color: [f32; 3],
}

/// Une entrée de contenu : identifier stable + kind (qui porte ses données).
#[derive(Debug, Clone, PartialEq)]
pub struct ContentEntry {
    /// Identifiant namespacé, ex. `core:air`, `core:stone`.
    pub identifier: String,
    pub kind: Kind,
}

impl ContentEntry {
    /// Raccourci pour l'entrée la plus courante : un bloc.
    pub fn new_block(identifier: &str, solid: bool, color: [f32; 3]) -> Self {
        Self {
            identifier: identifier.to_string(),
            kind: Kind::Block(BlockData { solid, color }),
        }
    }

    /// Les données de bloc, si l'entrée en est un.
    pub fn block(&self) -> Option<&BlockData> {
        match &self.kind {
            Kind::Block(data) => Some(data),
            _ => None,
        }
    }
}

/// Registre append-only. L'ID d'une entrée est son index d'insertion.
#[derive(Debug, Default)]
pub struct Registry {
    entries: Vec<ContentEntry>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ajoute une entrée et retourne son ID stable.
    ///
    /// Erreur si l'identifier existe déjà : un identifier est unique à vie
    /// (le remapper casserait les saves qui le référencent).
    pub fn register(&mut self, entry: ContentEntry) -> Result<ContentId, RegistryError> {
        if self.lookup(&entry.identifier).is_some() {
            return Err(RegistryError::DuplicateIdentifier(entry.identifier));
        }
        let id = ContentId(self.entries.len() as u32);
        self.entries.push(entry);
        Ok(id)
    }

    pub fn get(&self, id: ContentId) -> Option<&ContentEntry> {
        self.entries.get(id.0 as usize)
    }

    /// Résout un identifier vers son ID (scan linéaire : le registre est
    /// petit et le lookup par nom est rare — le hot path passe par les IDs).
    pub fn lookup(&self, identifier: &str) -> Option<ContentId> {
        self.entries
            .iter()
            .position(|e| e.identifier == identifier)
            .map(|i| ContentId(i as u32))
    }

    /// Itère les entrées avec leur ID, dans l'ordre d'insertion (= ordre des
    /// IDs, par construction append-only). Lecture seule : permet au
    /// consommateur de *découvrir* le contenu (ex. construire une hotbar de
    /// blocs solides) sans nommer quoi que ce soit en dur.
    pub fn iter(&self) -> impl Iterator<Item = (ContentId, &ContentEntry)> {
        self.entries
            .iter()
            .enumerate()
            .map(|(i, e)| (ContentId(i as u32), e))
    }
}

#[derive(Debug, PartialEq)]
pub enum RegistryError {
    DuplicateIdentifier(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_follow_insertion_order() {
        let mut reg = Registry::new();
        let air = reg.register(ContentEntry::new_block("core:air", false, [1.0; 3])).unwrap();
        let stone = reg.register(ContentEntry::new_block("core:stone", true, [1.0; 3])).unwrap();
        assert_eq!(air, ContentId(0));
        assert_eq!(stone, ContentId(1));
    }

    #[test]
    fn ids_are_stable_after_appends() {
        let mut reg = Registry::new();
        let stone = reg.register(ContentEntry::new_block("core:stone", true, [1.0; 3])).unwrap();
        // On ajoute d'autres entrées : l'ID et la def de stone ne bougent pas.
        for i in 0..100 {
            reg.register(ContentEntry::new_block(&format!("core:gen_{i}"), true, [1.0; 3])).unwrap();
        }
        assert_eq!(reg.lookup("core:stone"), Some(stone));
        assert_eq!(reg.get(stone).unwrap().identifier, "core:stone");
    }

    #[test]
    fn duplicate_identifier_is_rejected() {
        let mut reg = Registry::new();
        reg.register(ContentEntry::new_block("core:stone", true, [1.0; 3])).unwrap();
        let err = reg.register(ContentEntry::new_block("core:stone", true, [1.0; 3])).unwrap_err();
        assert_eq!(
            err,
            RegistryError::DuplicateIdentifier("core:stone".to_string())
        );
    }

    #[test]
    fn iter_yields_ids_in_insertion_order() {
        let mut reg = Registry::new();
        let air = reg.register(ContentEntry::new_block("core:air", false, [1.0; 3])).unwrap();
        let stone = reg.register(ContentEntry::new_block("core:stone", true, [1.0; 3])).unwrap();
        let ids: Vec<ContentId> = reg.iter().map(|(id, _)| id).collect();
        assert_eq!(ids, vec![air, stone]);
        // Le cas d'usage : découvrir les blocs solides sans les nommer.
        let solids: Vec<ContentId> = reg
            .iter()
            .filter(|(_, e)| e.block().is_some_and(|b| b.solid))
            .map(|(id, _)| id)
            .collect();
        assert_eq!(solids, vec![stone]);
    }

    #[test]
    fn lookup_missing_returns_none() {
        let reg = Registry::new();
        assert_eq!(reg.lookup("core:nope"), None);
        assert_eq!(reg.get(ContentId(42)), None);
    }
}
