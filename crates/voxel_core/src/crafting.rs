//! Recherche de recette (§3.1 : recettes portées par l'entrée produite).
//!
//! Pur : on donne la station et ce qui est posé dessus, on obtient ce qui
//! sort. Les entrées se comparent **sans ordre** (deux pierres et une terre,
//! dans n'importe quel ordre de pose).

use crate::registry::{ContentId, Registry};

/// La première recette (ordre du registre, donc déterministe) faite sur
/// `station` dont les entrées sont exactement `items`. Rend l'entrée
/// produite et sa quantité.
pub fn find(registry: &Registry, station: ContentId, items: &[ContentId]) -> Option<(ContentId, u32)> {
    let mut have = items.to_vec();
    have.sort_by_key(|c| c.0);
    registry.iter().find_map(|(product, entry)| {
        entry.recipes.iter().find_map(|r| {
            if registry.lookup(&r.station) != Some(station) {
                return None;
            }
            let mut need: Vec<ContentId> =
                r.inputs.iter().filter_map(|s| registry.lookup(s)).collect();
            need.sort_by_key(|c| c.0);
            (need == have).then_some((product, r.count))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_inputs_in_any_order_on_the_right_station() {
        let reg = Registry::from_ron(
            r#"[
                (identifier: "t:bench", kind: Block((solid: true, color: (1.0, 1.0, 1.0)))),
                (identifier: "t:other", kind: Block((solid: true, color: (1.0, 1.0, 1.0)))),
                (identifier: "t:stone", kind: Block((solid: true, color: (1.0, 1.0, 1.0)))),
                (identifier: "t:dirt",  kind: Block((solid: true, color: (1.0, 1.0, 1.0)))),
                (identifier: "t:lamp",  kind: Block((solid: true, color: (1.0, 1.0, 0.0))),
                    recipes: [(inputs: ["t:stone", "t:stone", "t:dirt"], count: 2, station: "t:bench")]),
            ]"#,
        )
        .unwrap();
        let id = |s| reg.lookup(s).unwrap();
        let (bench, stone, dirt) = (id("t:bench"), id("t:stone"), id("t:dirt"));

        assert_eq!(find(&reg, bench, &[dirt, stone, stone]), Some((id("t:lamp"), 2)));
        assert_eq!(find(&reg, id("t:other"), &[dirt, stone, stone]), None, "mauvaise station");
        assert_eq!(find(&reg, bench, &[stone, stone]), None, "il manque la terre");
        assert_eq!(find(&reg, bench, &[stone, stone, dirt, dirt]), None, "en trop");
    }
}
