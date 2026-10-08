//! Registre de contenu — §3.1 du design doc.
//!
//! Un registre **unique**, **append-only**, à **kinds unifiés**, destiné à être
//! **sérialisé dans la save** (world-owned). Les IDs entiers sont stables :
//! une fois attribués, ils ne bougent jamais — c'est eux que les palettes de
//! chunk référencent.
//!
//! Append-only par *construction* : l'API n'expose ni suppression ni
//! réordonnancement. C'est le type qui porte l'invariant, pas la discipline.
//!
//! Le contenu s'écrit en donnée (RON, voir `assets/content/`) et se charge
//! par [`Registry::from_ron`] : ajouter un bloc ne demande pas de recompiler.

use std::fmt;

use serde::Deserialize;

use crate::rules::Rule;

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
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub enum Kind {
    Block(BlockData),
    Item,
    EntityType,
}

/// Données spécifiques aux blocs, lues par le mesher et la physique.
///
/// Volontairement minimal pour la tranche verticale : le comportement riche
/// viendra par la couche script/data (§3.6), pas en gonflant cette struct.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct BlockData {
    /// Un bloc non-solide (air…) n'est ni meshé ni collidable.
    pub solid: bool,
    /// Couleur de base RGB — suffit pour la slice (pas de textures encore).
    pub color: [f32; 3],
    /// Ce que le bloc donne quand on le casse, par identifier. Absent : le
    /// bloc lui-même ; `Some([])` : rien (verre…) ; sinon ces entrées
    /// (l'herbe donne de la terre, un minerai une gemme).
    #[serde(default)]
    pub drops: Option<Vec<String>>,
    /// Comportement en donnée (§3.6) — voir [`crate::rules`].
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Block-entity (§3.3) : le bloc garde jusqu'à N items posés sur lui
    /// (un établi). Absent : pas d'état, jamais de ligne dans le canal creux.
    #[serde(default)]
    pub storage: Option<u32>,
}

/// Une façon de fabriquer l'entrée qui la porte (§3.1 : les recettes sont
/// une donnée de l'entrée produite).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Recipe {
    /// Items consommés, sans ordre (un identifier par exemplaire).
    pub inputs: Vec<String>,
    /// Exemplaires produits.
    #[serde(default = "one")]
    pub count: u32,
    /// Le bloc sur lequel la recette se fait.
    pub station: String,
}

fn one() -> u32 {
    1
}

/// Une entrée de contenu : identifier stable + kind (qui porte ses données).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ContentEntry {
    /// Identifiant namespacé, ex. `core:air`, `core:stone`.
    pub identifier: String,
    pub kind: Kind,
    #[serde(default)]
    pub recipes: Vec<Recipe>,
}

impl ContentEntry {
    /// Raccourci pour l'entrée la plus courante : un bloc.
    pub fn new_block(identifier: &str, solid: bool, color: [f32; 3]) -> Self {
        Self {
            identifier: identifier.to_string(),
            kind: Kind::Block(BlockData {
                solid,
                color,
                drops: None,
                rules: Vec::new(),
                storage: None,
            }),
            recipes: Vec::new(),
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

    /// Construit un registre depuis une liste d'entrées en RON. L'ordre du
    /// fichier fixe les IDs (append-only : on ajoute à la fin).
    pub fn from_ron(text: &str) -> Result<Self, LoadError> {
        let entries: Vec<ContentEntry> = ron::from_str(text).map_err(LoadError::Parse)?;
        let mut registry = Self::new();
        for entry in entries {
            registry.register(entry).map_err(LoadError::Registry)?;
        }
        // Une référence vers une entrée inexistante est refusée au
        // chargement, pas découverte à la première casse.
        for (_, entry) in registry.iter() {
            let block = entry.block();
            let drops = block.into_iter().flat_map(|b| b.drops.iter().flatten());
            let rules = block.into_iter().flat_map(|b| b.rules.iter().flat_map(Rule::references));
            let recipes = entry
                .recipes
                .iter()
                .flat_map(|r| r.inputs.iter().chain([&r.station]));
            for target in drops.chain(rules).chain(recipes) {
                if registry.lookup(target).is_none() {
                    return Err(LoadError::UnknownReference {
                        from: entry.identifier.clone(),
                        to: target.clone(),
                    });
                }
            }
        }
        Ok(registry)
    }

    /// Ce que donne le bloc `id` quand on le casse (voir [`BlockData::drops`]).
    /// Vide si `id` n'est pas un bloc.
    pub fn drops(&self, id: ContentId) -> Vec<ContentId> {
        match self.get(id).and_then(ContentEntry::block) {
            None => Vec::new(),
            Some(BlockData { drops: None, .. }) => vec![id],
            Some(BlockData { drops: Some(list), .. }) => {
                // Références validées au chargement.
                list.iter().filter_map(|t| self.lookup(t)).collect()
            }
        }
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

/// Échec de chargement d'un fichier de contenu.
#[derive(Debug)]
pub enum LoadError {
    /// RON invalide ou entrée mal formée (ligne:colonne dans le message).
    Parse(ron::error::SpannedError),
    Registry(RegistryError),
    /// Une entrée cite un identifier qui n'existe pas dans le registre.
    UnknownReference { from: String, to: String },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Parse(err) => write!(f, "{err}"),
            Self::Registry(RegistryError::DuplicateIdentifier(id)) => {
                write!(f, "identifier en double : {id}")
            }
            Self::UnknownReference { from, to } => {
                write!(f, "{from} cite {to}, absent du registre")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_content_file_loads() {
        // Le fichier livré avec le jeu : un RON cassé doit casser les tests,
        // pas le démarrage.
        let reg = Registry::from_ron(include_str!("../../../assets/content/core.ron")).unwrap();
        let air = reg.lookup("core:air").expect("core:air");
        assert_eq!(air, ContentId(0));
        assert!(!reg.get(air).unwrap().block().unwrap().solid);
        assert!(reg.lookup("core:grass").is_some() && reg.lookup("core:stone").is_some());
    }

    #[test]
    fn drops_default_to_self_and_can_be_empty_or_other() {
        let reg = Registry::from_ron(
            r#"[
                (identifier: "a:dirt",  kind: Block((solid: true, color: (1.0, 1.0, 1.0)))),
                (identifier: "a:grass", kind: Block((solid: true, color: (1.0, 1.0, 1.0), drops: Some(["a:dirt"])))),
                (identifier: "a:glass", kind: Block((solid: true, color: (1.0, 1.0, 1.0), drops: Some([])))),
                (identifier: "a:gem",   kind: Item),
            ]"#,
        )
        .unwrap();
        let id = |s| reg.lookup(s).unwrap();
        assert_eq!(reg.drops(id("a:dirt")), vec![id("a:dirt")]);
        assert_eq!(reg.drops(id("a:grass")), vec![id("a:dirt")]);
        assert!(reg.drops(id("a:glass")).is_empty());
        assert!(reg.drops(id("a:gem")).is_empty()); // pas un bloc
    }

    #[test]
    fn ron_order_gives_ids_and_errors_are_reported() {
        let reg = Registry::from_ron(
            r#"[
                (identifier: "a:x", kind: Block((solid: true, color: (1.0, 0.0, 0.0)))),
                (identifier: "a:y", kind: Item),
            ]"#,
        )
        .unwrap();
        assert_eq!(reg.lookup("a:y"), Some(ContentId(1)));
        assert_eq!(reg.get(ContentId(0)).unwrap().block().unwrap().color, [1.0, 0.0, 0.0]);

        let dup = Registry::from_ron(r#"[(identifier: "a:x", kind: Item), (identifier: "a:x", kind: Item)]"#);
        assert!(matches!(dup, Err(LoadError::Registry(RegistryError::DuplicateIdentifier(_)))));

        let dangling = Registry::from_ron(
            r#"[(identifier: "a:x", kind: Block((solid: true, color: (1.0, 0.0, 0.0), drops: Some(["a:nope"]))))]"#,
        );
        assert!(matches!(dangling, Err(LoadError::UnknownReference { .. })));

        // Champ manquant (`solid`) : erreur de parse avec position.
        let bad = Registry::from_ron(r#"[(identifier: "a:x", kind: Block((color: (1.0, 0.0, 0.0))))]"#);
        let msg = bad.unwrap_err().to_string();
        assert!(msg.contains("solid"), "{msg}");
    }

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
