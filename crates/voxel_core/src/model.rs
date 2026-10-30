//! Modèles d'entités (#15, design doc §3.1) : des parties qui tournent
//! autour de leur pivot, faites de boîtes texturées en seizièmes de mètre.
//!
//! **Pourquoi des seizièmes de mètre, et pas de case** : une entité vit en
//! mètres (§2), sa taille ne suit pas la résolution du monde. À 16 pixels
//! par texture (une texture couvre 1 m), un seizième vaut un pixel, comme
//! un bloc à 1 vox/m.
//!
//! **Pourquoi des boîtes** : le même rendu que les blocs (texture array,
//! coordonnées en mètres), et une IA écrit « corps de (-4, 6, -7) à
//! (4, 14, 7) » sans peine. Options écartées : #59.

use serde::{Deserialize, Serialize};

use crate::mesher::{push_box, MeshData, NO_TEXTURE};
use crate::mob::Behavior;
use crate::registry::Registry;
use crate::spawn::SpawnRule;
use crate::texgen::GeneratedTexture;

/// Parties au plus par modèle.
pub const MAX_PARTS: usize = 16;
/// Boîtes au plus par modèle, toutes parties comprises : borne ce qu'une
/// entrée générée peut coûter.
pub const MAX_MODEL_BOXES: usize = 64;
/// Une unité de modèle, en mètres.
pub const UNIT_M: f32 = 1.0 / 16.0;
/// Coordonnées bornées à ±16 m.
const MAX_COORD: i16 = 256;
/// Côté ou hauteur de la boîte de collision au plus, en mètres.
const MAX_SIZE_M: f32 = 16.0;
/// Comportements, mouvements d'animation ou règles d'apparition, au plus
/// par entrée.
const MAX_BEHAVIORS: usize = 16;

/// Données d'une entrée `EntityType`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct EntityData {
    /// Boîte de collision en mètres, `(largeur, hauteur)`, séparée du
    /// visuel.
    pub size: [f32; 2],
    /// Dessiné pieds en y = 0, centré, regardant vers +Z.
    pub model: Vec<Part>,
    /// Textures calculées par le moteur, comme pour un bloc (§3.1).
    #[serde(default)]
    pub generated_textures: Vec<GeneratedTexture>,
    /// Ce que fait le mob, le premier applicable gagne (§3.6, #32).
    #[serde(default)]
    pub behaviors: Vec<Behavior>,
    /// Les mouvements de ses parties, à l'affichage seulement (#32).
    #[serde(default)]
    pub animation: Vec<Motion>,
    /// Où et combien il apparaît seul (§3.6, #60) ; vide : jamais.
    #[serde(default)]
    pub spawn: Vec<SpawnRule>,
}

/// Un mouvement d'animation : il tourne une partie autour de son pivot.
/// Angles en degrés. **Vocabulaire append-only** (§3.6).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub enum Motion {
    /// La partie se balance d'avant en arrière quand le mob marche : `angle`
    /// au plus, une période tous les `stride` mètres parcourus, décalée de
    /// `phase` (de 0 à 1 : 0,5 pour la patte opposée).
    Swing { part: String, angle: f32, stride: f32, phase: f32 },
    /// La partie se tourne vers ce que regarde le mob, `max` au plus de
    /// chaque côté et en hauteur.
    Look { part: String, max: f32 },
}

impl Motion {
    pub fn part(&self) -> &str {
        match self {
            Motion::Swing { part, .. } | Motion::Look { part, .. } => part,
        }
    }
}

/// Une partie du modèle : elle tourne d'un bloc autour de son pivot.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Part {
    pub name: String,
    /// La partie qui la porte, déclarée avant elle ; absente : le modèle.
    #[serde(default)]
    pub parent: Option<String>,
    /// Point autour duquel elle tourne, en seizièmes de mètre, dans le
    /// repère du modèle (pas de son parent).
    #[serde(default)]
    pub pivot: [i16; 3],
    pub boxes: Vec<ModelBox>,
}

/// Une boîte : coin bas, coin haut (seizièmes de mètre, repère du
/// modèle), nom de sa texture.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ModelBox(pub [i16; 3], pub [i16; 3], pub String);

impl EntityData {
    /// Ce qui cloche dans le modèle, s'il y a quelque chose.
    pub(crate) fn invalid(&self) -> Option<String> {
        let [w, h] = self.size;
        if !(w > 0.0 && w <= MAX_SIZE_M && h > 0.0 && h <= MAX_SIZE_M) {
            return Some(format!("size ({w}, {h}) hors de ]0, {MAX_SIZE_M}] m"));
        }
        if !(1..=MAX_PARTS).contains(&self.model.len()) {
            return Some(format!("model : de 1 à {MAX_PARTS} parties, pas {}", self.model.len()));
        }
        let boxes = self.model.iter().map(|p| p.boxes.len()).sum::<usize>();
        if !(1..=MAX_MODEL_BOXES).contains(&boxes) {
            return Some(format!("model : de 1 à {MAX_MODEL_BOXES} boîtes, pas {boxes}"));
        }
        let inside = |p: [i16; 3]| p.iter().all(|c| (-MAX_COORD..=MAX_COORD).contains(c));
        for (i, part) in self.model.iter().enumerate() {
            let before = &self.model[..i];
            if before.iter().any(|p| p.name == part.name) {
                return Some(format!("partie {} en double", part.name));
            }
            if let Some(parent) = &part.parent
                && !before.iter().any(|p| &p.name == parent)
            {
                return Some(format!("partie {} : parent {parent} absent ou déclaré après elle", part.name));
            }
            if !inside(part.pivot) {
                return Some(format!("partie {} : pivot hors de ±{MAX_COORD}", part.name));
            }
            if let Some(b) = part.boxes.iter().find(|b| !inside(b.0) || !inside(b.1) || (0..3).any(|a| b.0[a] >= b.1[a])) {
                return Some(format!("partie {} : boîte {b:?} vide ou hors de ±{MAX_COORD}", part.name));
            }
        }
        if [self.behaviors.len(), self.animation.len(), self.spawn.len()].iter().any(|&n| n > MAX_BEHAVIORS) {
            return Some(format!("au plus {MAX_BEHAVIORS} comportements, mouvements et règles d'apparition"));
        }
        if let Some(reason) = self.behaviors.iter().find_map(Behavior::invalid) {
            return Some(reason);
        }
        if let Some(reason) = self.spawn.iter().find_map(SpawnRule::invalid) {
            return Some(reason);
        }
        for motion in &self.animation {
            let fine = match motion {
                Motion::Swing { angle, stride, phase, .. } => {
                    (0.0..=180.0).contains(angle) && *stride > 0.0 && *stride <= 64.0 && (0.0..=1.0).contains(phase)
                }
                Motion::Look { max, .. } => (0.0..=180.0).contains(max),
            };
            if !fine || !self.model.iter().any(|p| p.name == motion.part()) {
                return Some(format!("mouvement {motion:?} : partie absente ou réglage hors bornes"));
            }
        }
        None
    }

    /// Les noms de textures cités (le même peut revenir).
    pub(crate) fn texture_names(&self) -> impl Iterator<Item = &String> {
        self.model.iter().flat_map(|p| &p.boxes).map(|b| &b.2)
    }
}

impl Part {
    /// Le pivot en mètres, dans le repère du modèle.
    pub fn pivot_m(&self) -> [f32; 3] {
        self.pivot.map(|c| f32::from(c) * UNIT_M)
    }

    /// Les boîtes de la partie, en mètres, **autour de son pivot** : le
    /// rendu la tourne en tournant son repère.
    pub fn mesh(&self, registry: &Registry) -> MeshData {
        let mut mesh = MeshData::default();
        let pivot = self.pivot_m();
        for ModelBox(lo, hi, texture) in &self.boxes {
            let layer = registry.texture_layer(texture).unwrap_or(NO_TEXTURE);
            let at = |p: &[i16; 3]| std::array::from_fn(|a| f32::from(p[a]) * UNIT_M - pivot[a]);
            push_box(&mut mesh, at(lo), at(hi), [layer; 3], [1.0; 4], 1.0);
        }
        mesh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pig() -> EntityData {
        crate::registry::parse(
            r#"(size: (0.9, 0.9), model: [
                (name: "body", pivot: (0, 6, 0), boxes: [((-5, 6, -8), (5, 14, 8), "hide")]),
                (name: "head", parent: "body", pivot: (0, 12, 8), boxes: [((-4, 8, 8), (4, 16, 14), "hide")]),
            ])"#,
        )
        .unwrap()
    }

    #[test]
    fn a_model_is_checked() {
        assert_eq!(pig().invalid(), None);
        let broken = |f: &dyn Fn(&mut EntityData)| {
            let mut e = pig();
            f(&mut e);
            e.invalid()
        };
        assert!(broken(&|e| e.size = [0.0, 1.0]).is_some(), "taille nulle");
        assert!(broken(&|e| e.model[1].parent = Some("tail".into())).is_some(), "parent absent");
        assert!(broken(&|e| e.model.swap(0, 1)).is_some(), "parent déclaré après");
        assert!(broken(&|e| e.model[1].name = "body".into()).is_some(), "nom en double");
        assert!(broken(&|e| e.model[0].boxes[0].1[1] = 6).is_some(), "boîte vide");
        assert!(broken(&|e| e.model[0].pivot[0] = 300).is_some(), "pivot trop loin");
        assert!(broken(&|e| e.model.clear()).is_some(), "sans partie");
        let look = |part: &str| Motion::Look { part: part.into(), max: 45.0 };
        assert!(broken(&|e| e.animation.push(look("tail"))).is_some(), "mouvement d'une partie absente");
        assert_eq!(broken(&|e| e.animation.push(look("head"))), None);
        let wander = |speed| Behavior::Wander { speed, range: 8.0, pause: 4.0 };
        assert!(broken(&|e| e.behaviors.push(wander(-1.0))).is_some(), "vitesse négative");
        assert_eq!(broken(&|e| e.behaviors.push(wander(1.0))), None);
    }

    #[test]
    fn a_part_is_meshed_around_its_pivot() {
        let mesh = pig().model[1].mesh(&Registry::new());
        assert_eq!(mesh.face_count(), 6);
        let (min, max) = mesh.positions.iter().fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), p| {
            (std::array::from_fn(|a| lo[a].min(p[a])), std::array::from_fn(|a| hi[a].max(p[a])))
        });
        // Tête de (-4, 8, 8) à (4, 16, 14), pivot (0, 12, 8) : en mètres.
        assert_eq!(min, [-0.25, -0.25, 0.0]);
        assert_eq!(max, [0.25, 0.25, 0.375]);
    }
}
