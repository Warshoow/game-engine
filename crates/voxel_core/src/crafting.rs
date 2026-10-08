//! Recherche de recette (§3.1 : recettes portées par l'entrée produite).
//!
//! Pur : on donne la station et ce qui est posé dessus (ou l'inventaire,
//! pour une recette sans station), on obtient ce qui sort. Les entrées se comparent **sans ordre** (deux pierres et une terre,
//! dans n'importe quel ordre de pose).

use crate::registry::{ContentId, Recipe, Registry};

/// La première recette (ordre du registre, donc déterministe) faite sur
/// `station` dont les entrées sont exactement `items`. Rend l'entrée
/// produite et sa quantité.
pub fn find(registry: &Registry, station: ContentId, items: &[ContentId]) -> Option<(ContentId, u32)> {
    let mut have = items.to_vec();
    have.sort_by_key(|c| c.0);
    registry.iter().find_map(|(product, entry)| {
        entry.recipes.iter().find_map(|r| {
            if r.station.as_deref().and_then(|s| registry.lookup(s)) != Some(station) {
                return None;
            }
            let mut need: Vec<ContentId> =
                r.inputs.iter().filter_map(|s| registry.lookup(s)).collect();
            need.sort_by_key(|c| c.0);
            (need == have).then_some((product, r.count))
        })
    })
}

/// Les recettes **sans station** (ordre du registre), avec leur produit :
/// ce que le menu de fabrication liste.
pub fn without_station(registry: &Registry) -> impl Iterator<Item = (ContentId, &Recipe)> {
    registry
        .iter()
        .flat_map(|(product, entry)| entry.recipes.iter().map(move |r| (product, r)))
        .filter(|(_, r)| r.station.is_none())
}

/// Les items que `recipe` consomme si les piles `have` suffisent, `None`
/// sinon.
pub fn inputs_from(registry: &Registry, recipe: &Recipe, have: &[(ContentId, u32)]) -> Option<Vec<ContentId>> {
    let need: Vec<ContentId> = recipe.inputs.iter().map(|s| registry.lookup(s)).collect::<Option<_>>()?;
    let enough = need.iter().all(|id| {
        let wanted = need.iter().filter(|n| *n == id).count() as u32;
        have.iter().any(|&(c, n)| c == *id && n >= wanted)
    });
    enough.then_some(need)
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

    #[test]
    fn stationless_recipe_is_made_from_the_inventory() {
        let reg = Registry::from_ron(
            r#"[
                (identifier: "t:stone", kind: Block((solid: true, color: (1.0, 1.0, 1.0)))),
                (identifier: "t:bench", kind: Block((solid: true, color: (1.0, 1.0, 1.0))),
                    recipes: [(inputs: ["t:stone", "t:stone", "t:stone"])]),
                (identifier: "t:lamp",  kind: Block((solid: true, color: (1.0, 1.0, 0.0))),
                    recipes: [(inputs: ["t:stone"], station: "t:bench")]),
            ]"#,
        )
        .unwrap();
        let id = |s| reg.lookup(s).unwrap();
        let (stone, bench) = (id("t:stone"), id("t:bench"));

        let listed: Vec<_> = without_station(&reg).collect();
        assert_eq!(listed.len(), 1, "la lampe se fait sur l'établi : pas listée");
        let (product, recipe) = listed[0];
        assert_eq!(product, bench);
        assert_eq!(inputs_from(&reg, recipe, &[(stone, 2)]), None, "il manque une pierre");
        assert_eq!(inputs_from(&reg, recipe, &[(bench, 1), (stone, 5)]), Some(vec![stone; 3]));
        // Une recette à station ne se fait pas depuis l'inventaire, et
        // l'inverse : la recette de l'établi ne se fait pas sur l'établi.
        assert_eq!(find(&reg, bench, &[stone, stone, stone]), None);
        assert_eq!(find(&reg, bench, &[stone]), Some((id("t:lamp"), 1)));
    }
}
