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

use serde::Deserialize;

use crate::registry::{ContentId, Registry};

/// Événement du moteur auquel une règle se branche.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Hook {
    /// Clic droit sur le bloc.
    Used,
    /// Le bloc vient d'être posé.
    Placed,
    /// Le bloc vient d'être cassé (ses drops sont déjà tombés).
    Broken,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub enum Condition {
    /// Le joueur tient cette entrée en main.
    Holding(String),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub enum Effect {
    /// Remplace le bloc de la règle par cette entrée.
    ReplaceSelf(String),
    /// Fait tomber un exemplaire de cette entrée à la position du bloc.
    Drop(String),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
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
        let conditions = self.when.iter().map(|Condition::Holding(id)| id);
        let effects = self.then.iter().map(|e| match e {
            Effect::ReplaceSelf(id) | Effect::Drop(id) => id,
        });
        conditions.chain(effects)
    }
}

/// Ce qu'une règle demande au monde, identifiers résolus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    SetSelf(ContentId),
    Drop(ContentId),
}

/// Contexte d'évaluation : ce que les conditions peuvent interroger.
#[derive(Debug, Clone, Copy, Default)]
pub struct Context {
    pub holding: Option<ContentId>,
}

/// Le bloc `block` a-t-il au moins une règle sur `hook` (conditions mises à
/// part) ? Sert à décider si le clic droit *utilise* le bloc ou pose.
pub fn has_rule(registry: &Registry, block: ContentId, hook: Hook) -> bool {
    rules(registry, block).any(|r| r.on == hook)
}

/// Les actions déclenchées par `hook` sur `block`, dans l'ordre des règles
/// puis des effets.
pub fn actions(registry: &Registry, block: ContentId, hook: Hook, ctx: Context) -> Vec<Action> {
    // Références validées au chargement : un lookup raté ne peut venir que
    // d'un registre construit en code (tests) — l'effet est ignoré.
    let id = |s: &String| registry.lookup(s);
    rules(registry, block)
        .filter(|r| r.on == hook)
        .filter(|r| {
            r.when.iter().all(|c| match c {
                Condition::Holding(s) => ctx.holding.is_some() && ctx.holding == id(s),
            })
        })
        .flat_map(|r| &r.then)
        .filter_map(|e| match e {
            Effect::ReplaceSelf(s) => id(s).map(Action::SetSelf),
            Effect::Drop(s) => id(s).map(Action::Drop),
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
        let holding_flint = Context { holding: Some(id("t:flint")) };
        assert_eq!(
            actions(&reg, log, Hook::Used, holding_flint),
            vec![Action::SetSelf(id("t:lamp_lit")), Action::Drop(id("t:flint"))]
        );
        assert_eq!(actions(&reg, log, Hook::Broken, Context::default()), vec![Action::Drop(id("t:flint"))]);
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
