//! Règles de comportement « déclencheur → condition → effet » — §3.6, étage 2.
//!
//! Une règle est de la **donnée** dans l'entrée du registre :
//!
//! ```ron
//! rules: [(on: Used, when: [Holding("core:flint")], then: [ReplaceSelf("core:fire")])]
//! ```
//!
//! Le moteur code une fois chaque hook, condition et effet ; le contenu les
//! combine. Ce module ne fait qu'**évaluer** : il dit quelles actions une
//! règle déclenche, sans toucher au monde — c'est l'appelant qui applique.
//! Évaluation pure → testable sans fenêtre, et déterministe.
//!
//! **Vocabulaire append-only (§3.6)** : on ajoute des variantes, on n'en
//! renomme ni n'en supprime jamais. Une variante inconnue dans un fichier de
//! contenu est refusée au chargement (erreur de parse qui la nomme).

use serde::{Deserialize, Serialize};

use crate::registry::{ContentId, Registry};
use crate::worldgen::hash64;

/// Événement du moteur auquel une règle se branche.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
pub enum Hook {
    /// Clic droit sur le bloc.
    Used,
    /// Le bloc vient d'être posé.
    Placed,
    /// Le bloc vient d'être cassé (ses drops sont déjà tombés).
    Broken,
    /// De temps en temps, au hasard (#34) : en moyenne une fois par minute
    /// pour un bloc donné, voir [`crate::ticks`]. Pousse, dépérissement…
    Tick,
    /// Ces secondes après l'apparition du bloc (#34) : minuteur, voir
    /// [`crate::world::VoxelWorld::start_timers`]. Culture, feu…
    After(f32),
    /// Le bloc ou l'un de ses six voisins vient de changer (#28), au tick
    /// suivant. Lave au contact de l'eau…
    NeighborChanged,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub enum Condition {
    /// Le joueur tient cette entrée en main.
    Holding(String),
    /// Le joueur a la main vide.
    EmptyHand,
    /// Le joueur tient quelque chose, peu importe quoi.
    HoldingAny,
    /// Vraie avec cette probabilité, dans [0, 1] (#34). Un tirage par
    /// règle, déterministe : dérivé de [`Context::seed`].
    Chance(f32),
    /// Le bloc juste au-dessus est cette entrée (#34).
    Above(String),
    /// L'un des six voisins est cette entrée (#28).
    Touching(String),
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub enum Effect {
    /// Remplace le bloc de la règle par cette entrée.
    ReplaceSelf(String),
    /// Fait tomber un exemplaire de cette entrée à la position du bloc.
    Drop(String),
    /// Pose un exemplaire de ce que tient le joueur sur le bloc (son
    /// block-entity, §3.3), s'il a de la place.
    StoreHeld,
    /// Fabrique avec ce qui est posé sur le bloc : si c'est exactement les
    /// entrées d'une recette faite sur ce bloc, le produit tombe ; sinon les
    /// items posés sont rendus. Dans les deux cas le bloc est vidé.
    Craft,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Rule {
    pub on: Hook,
    /// Toutes doivent être vraies (vide : toujours).
    #[serde(default)]
    pub when: Vec<Condition>,
    pub then: Vec<Effect>,
}

impl Rule {
    /// Les identifiers que cite la règle (validés au chargement du registre).
    pub fn references(&self) -> impl Iterator<Item = &String> {
        let conditions = self.when.iter().filter_map(|c| match c {
            Condition::Holding(id) | Condition::Above(id) | Condition::Touching(id) => Some(id),
            Condition::EmptyHand | Condition::HoldingAny | Condition::Chance(_) => None,
        });
        let effects = self.then.iter().filter_map(|e| match e {
            Effect::ReplaceSelf(id) | Effect::Drop(id) => Some(id),
            Effect::StoreHeld | Effect::Craft => None,
        });
        conditions.chain(effects)
    }
}

/// Ce qu'une règle demande au monde, identifiers résolus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    SetSelf(ContentId),
    Drop(ContentId),
    StoreHeld,
    Craft,
}

/// Contexte d'évaluation : ce que les conditions peuvent interroger.
#[derive(Debug, Clone, Copy, Default)]
pub struct Context {
    pub holding: Option<ContentId>,
    /// Source du hasard de `Chance`, voir [`event_seed`] : jamais d'aléa
    /// libre (§3.6, déterminisme).
    pub seed: u64,
    /// Le bloc au-dessus de celui de la règle (`None` : non chargé).
    pub above: Option<ContentId>,
    /// Ses six voisins (`None` : non chargé).
    pub neighbors: [Option<ContentId>; 6],
}

/// Graine d'un événement : la même seed de monde, le même tick et le même
/// voxel donnent le même tirage.
pub fn event_seed(world_seed: u64, tick: u64, voxel: [i64; 3]) -> u64 {
    voxel.iter().fold(hash64(world_seed ^ hash64(tick)), |h, &c| hash64(h ^ c as u64))
}

/// Le bloc `block` a-t-il au moins une règle sur `hook` (conditions mises à
/// part) ? Sert à décider si le clic droit *utilise* le bloc ou pose.
pub fn has_rule(registry: &Registry, block: ContentId, hook: Hook) -> bool {
    rules(registry, block).any(|r| r.on == hook)
}

/// Les actions déclenchées par `hook` sur `block`, dans l'ordre des règles
/// puis des effets.
pub fn actions(registry: &Registry, block: ContentId, hook: Hook, ctx: Context) -> Vec<Action> {
    rules(registry, block)
        .enumerate()
        .filter(|(_, r)| r.on == hook)
        .flat_map(|(i, r)| fire(registry, i, r, ctx))
        .collect()
}

/// Les actions de la règle n° `rule` de `block` seule : celle dont le
/// minuteur est échu.
pub fn rule_actions(registry: &Registry, block: ContentId, rule: usize, ctx: Context) -> Vec<Action> {
    rules(registry, block).nth(rule).map(|r| fire(registry, rule, r, ctx)).unwrap_or_default()
}

/// Les règles `After` de `block` : (n° de règle, délai en secondes).
pub fn delays(registry: &Registry, block: ContentId) -> impl Iterator<Item = (usize, f32)> + '_ {
    rules(registry, block).enumerate().filter_map(|(i, r)| match r.on {
        Hook::After(s) => Some((i, s)),
        _ => None,
    })
}

/// Les actions de la règle `r` (n° `i`) si ses conditions tiennent.
fn fire(registry: &Registry, i: usize, r: &Rule, ctx: Context) -> Vec<Action> {
    // Références validées au chargement : un lookup raté ne peut venir que
    // d'un registre construit en code (tests) — l'effet est ignoré.
    let id = |s: &String| registry.lookup(s);
    // Dans [0, 1), propre à la règle : deux règles à `Chance` ne tombent
    // pas ensemble.
    let roll = (hash64(ctx.seed ^ hash64(i as u64)) >> 40) as f32 / (1u64 << 24) as f32;
    let holds = r.when.iter().all(|c| match c {
        Condition::Holding(s) => ctx.holding.is_some() && ctx.holding == id(s),
        Condition::EmptyHand => ctx.holding.is_none(),
        Condition::HoldingAny => ctx.holding.is_some(),
        Condition::Chance(p) => roll < *p,
        Condition::Above(s) => ctx.above.is_some() && ctx.above == id(s),
        Condition::Touching(s) => ctx.neighbors.iter().any(|n| n.is_some() && *n == id(s)),
    });
    if !holds {
        return Vec::new();
    }
    r.then
        .iter()
        .filter_map(|e| match e {
            Effect::ReplaceSelf(s) => id(s).map(Action::SetSelf),
            Effect::Drop(s) => id(s).map(Action::Drop),
            Effect::StoreHeld => Some(Action::StoreHeld),
            Effect::Craft => Some(Action::Craft),
        })
        .collect()
}

fn rules(registry: &Registry, block: ContentId) -> impl Iterator<Item = &Rule> {
    registry
        .get(block)
        .and_then(|e| e.block())
        .into_iter()
        .flat_map(|b| &b.rules)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::LoadError;

    fn registry() -> Registry {
        Registry::from_ron(
            r#"[
                (identifier: "t:lamp", kind: Block((solid: true, color: (0.3, 0.3, 0.0),
                    rules: [(on: Used, then: [ReplaceSelf("t:lamp_lit")])]))),
                (identifier: "t:lamp_lit", kind: Block((solid: true, color: (1.0, 1.0, 0.0),
                    rules: [(on: Used, then: [ReplaceSelf("t:lamp")])]))),
                (identifier: "t:flint", kind: Item),
                (identifier: "t:log", kind: Block((solid: true, color: (0.5, 0.3, 0.1),
                    rules: [
                        (on: Used, when: [Holding("t:flint")], then: [ReplaceSelf("t:lamp_lit"), Drop("t:flint")]),
                        (on: Broken, then: [Drop("t:flint")]),
                    ]))),
            ]"#,
        )
        .unwrap()
    }

    #[test]
    fn rule_without_condition_fires_on_its_hook_only() {
        let reg = registry();
        let id = |s| reg.lookup(s).unwrap();
        let ctx = Context::default();
        assert_eq!(actions(&reg, id("t:lamp"), Hook::Used, ctx), vec![Action::SetSelf(id("t:lamp_lit"))]);
        assert!(actions(&reg, id("t:lamp"), Hook::Broken, ctx).is_empty());
        assert!(has_rule(&reg, id("t:lamp"), Hook::Used));
        assert!(!has_rule(&reg, id("t:lamp"), Hook::Placed));
    }

    #[test]
    fn conditions_gate_effects_but_not_has_rule() {
        let reg = registry();
        let id = |s| reg.lookup(s).unwrap();
        let log = id("t:log");
        // Rien en main : la règle existe (le clic droit « utilise ») mais ne
        // fait rien.
        assert!(has_rule(&reg, log, Hook::Used));
        assert!(actions(&reg, log, Hook::Used, Context::default()).is_empty());
        let holding_flint = Context { holding: Some(id("t:flint")), ..Context::default() };
        assert_eq!(
            actions(&reg, log, Hook::Used, holding_flint),
            vec![Action::SetSelf(id("t:lamp_lit")), Action::Drop(id("t:flint"))]
        );
        assert_eq!(actions(&reg, log, Hook::Broken, Context::default()), vec![Action::Drop(id("t:flint"))]);
    }

    #[test]
    fn hand_conditions_split_store_and_craft() {
        let reg = Registry::from_ron(
            r#"[
                (identifier: "t:bench", kind: Block((solid: true, color: (1.0, 1.0, 1.0), storage: Some(9),
                    rules: [
                        (on: Used, when: [HoldingAny], then: [StoreHeld]),
                        (on: Used, when: [EmptyHand], then: [Craft]),
                    ]))),
                (identifier: "t:stone", kind: Block((solid: true, color: (1.0, 1.0, 1.0)))),
            ]"#,
        )
        .unwrap();
        let id = |s| reg.lookup(s).unwrap();
        let bench = id("t:bench");
        let holding = Context { holding: Some(id("t:stone")), ..Context::default() };
        assert_eq!(actions(&reg, bench, Hook::Used, holding), vec![Action::StoreHeld]);
        assert_eq!(actions(&reg, bench, Hook::Used, Context::default()), vec![Action::Craft]);
    }

    #[test]
    fn chance_fires_in_proportion_and_repeats_with_the_same_seed() {
        let reg = Registry::from_ron(
            r#"[
                (identifier: "t:seed", kind: Block((solid: true, color: (1.0, 1.0, 1.0),
                    rules: [(on: Tick, when: [Chance(0.25)], then: [ReplaceSelf("t:plant")])]))),
                (identifier: "t:plant", kind: Block((solid: true, color: (1.0, 1.0, 1.0)))),
            ]"#,
        )
        .unwrap();
        let seed = reg.lookup("t:seed").unwrap();
        let fires = |tick| {
            let ctx = Context { seed: event_seed(42, tick, [1, 2, 3]), ..Context::default() };
            !actions(&reg, seed, Hook::Tick, ctx).is_empty()
        };
        let hits = (0..4000).filter(|&t| fires(t)).count();
        assert!((800..1200).contains(&hits), "{hits}");
        assert!((0..100).all(|t| fires(t) == fires(t)));
    }

    #[test]
    fn above_reads_the_block_on_top() {
        let reg = Registry::from_ron(
            r#"[
                (identifier: "t:air", kind: Block((solid: false, color: (0.0, 0.0, 0.0)))),
                (identifier: "t:dirt", kind: Block((solid: true, color: (1.0, 1.0, 1.0),
                    rules: [(on: Tick, when: [Above("t:air")], then: [ReplaceSelf("t:grass")])]))),
                (identifier: "t:grass", kind: Block((solid: true, color: (1.0, 1.0, 1.0)))),
            ]"#,
        )
        .unwrap();
        let id = |s| reg.lookup(s).unwrap();
        let under = |above| Context { above, ..Context::default() };
        assert_eq!(actions(&reg, id("t:dirt"), Hook::Tick, under(Some(id("t:air")))), vec![Action::SetSelf(id("t:grass"))]);
        assert!(actions(&reg, id("t:dirt"), Hook::Tick, under(Some(id("t:dirt")))).is_empty());
        assert!(actions(&reg, id("t:dirt"), Hook::Tick, under(None)).is_empty());
    }

    #[test]
    fn touching_reads_all_six_neighbours() {
        let reg = Registry::from_ron(
            r#"[
                (identifier: "t:water", kind: Block((solid: false, color: (0.0, 0.0, 1.0)))),
                (identifier: "t:lava", kind: Block((solid: false, color: (1.0, 0.3, 0.0),
                    rules: [(on: NeighborChanged, when: [Touching("t:water")], then: [ReplaceSelf("t:stone")])]))),
                (identifier: "t:stone", kind: Block((solid: true, color: (0.5, 0.5, 0.5)))),
            ]"#,
        )
        .unwrap();
        let id = |s| reg.lookup(s).unwrap();
        let mut ctx = Context { neighbors: [Some(id("t:stone")); 6], ..Context::default() };
        assert!(actions(&reg, id("t:lava"), Hook::NeighborChanged, ctx).is_empty());
        ctx.neighbors[5] = Some(id("t:water"));
        assert_eq!(actions(&reg, id("t:lava"), Hook::NeighborChanged, ctx), vec![Action::SetSelf(id("t:stone"))]);
    }

    #[test]
    fn unknown_vocabulary_or_reference_is_refused_at_load() {
        // Hook inconnu de cette version du moteur : refusé, et nommé.
        let err = Registry::from_ron(
            r#"[(identifier: "t:x", kind: Block((solid: true, color: (1.0, 1.0, 1.0),
                rules: [(on: Ticked, then: [])])))]"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("Ticked"), "{err}");

        let err = Registry::from_ron(
            r#"[(identifier: "t:x", kind: Block((solid: true, color: (1.0, 1.0, 1.0),
                rules: [(on: Used, then: [ReplaceSelf("t:nope")])])))]"#,
        )
        .unwrap_err();
        assert!(matches!(err, LoadError::UnknownReference { .. }), "{err}");
    }
}
