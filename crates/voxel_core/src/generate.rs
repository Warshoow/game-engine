//! Génération de contenu en jeu (#18, §3.6) : ce que le moteur envoie au
//! générateur, et la validation de ce qu'il rend.
//!
//! Le générateur (une IA) est hors de ce module : il prend un texte et rend
//! un texte. Sa réponse est une donnée **non fiable** — elle n'entre au
//! registre qu'après [`accept`].

use crate::registry::{ContentEntry, LoadError, Registry, parse};
use crate::texgen;

/// Namespace imposé aux entrées et textures générées : elles ne peuvent ni
/// écraser ni se faire passer pour le contenu du jeu.
pub const NAMESPACE: &str = "gen:";

/// Le texte envoyé au générateur : la demande du joueur, le format d'une
/// entrée et ce qui existe déjà (pour les recettes et les règles).
pub fn prompt(registry: &Registry, request: &str) -> String {
    let existing: Vec<&str> = registry.iter().map(|(_, e)| e.identifier.as_str()).collect();
    let textures: Vec<&str> = registry.textures().iter().map(|t| t.name.as_str()).collect();
    format!(
        r#"Tu crées UN bloc pour un jeu voxel, d'après la demande du joueur.
Réponds UNIQUEMENT par l'entrée en RON, sans texte autour.

Format (les champs optionnels peuvent être omis) :
(
    identifier: "gen:nom_en_minuscules",
    kind: Block((
        solid: true,
        color: (r, g, b),              // [0, 1], couleur moyenne du bloc
        texture: Some((side: "gen:nom_cote", top: Some("gen:nom_dessus"))),  // top/bottom optionnels
        generated_textures: [
            (name: "gen:nom_cote", pattern: Cracks, color: (r, g, b), noise: 0.2, seed: 17),
        ],
        drops: Some(["core:dirt"]),    // optionnel : ce que donne le bloc cassé ; absent = lui-même
        rules: [(on: Used, when: [EmptyHand], then: [Drop("core:stone")])],  // optionnel
        alpha: Mask,                   // optionnel : bloc à trous (verre, feuillage), avec holes > 0
    )),
    recipes: [(inputs: ["core:stone", "core:dirt"], count: 1)],  // optionnel ; station: "core:workbench" possible
)

Règles :
- identifier et noms de textures générées commencent par "gen:" et sont nouveaux.
- pattern : Plain, Cracks, Planks, Spots ou Frame. noise dans [0, 1].
  holes (optionnel, [0, 1]) : part de pixels transparents, les traits du motif restent ;
  Frame avec holes: 1.0 ne garde que le cadre. Seulement sur un bloc alpha: Mask.
- texture peut aussi citer une texture existante : {textures:?}.
- Hooks (on) : Used, Placed, Broken. Conditions (when) : Holding("id"), EmptyHand, HoldingAny.
  Effets (then) : ReplaceSelf("id"), Drop("id"), StoreHeld, Craft.
- Les identifiers cités (drops, règles, recettes) doivent exister : {existing:?}.
- Donne une recette sans station, faite d'entrées courantes, pour que le joueur puisse le fabriquer.

Demande du joueur : {request}
"#
    )
}

/// Valide la réponse du générateur contre le registre. Rend l'entrée prête
/// à être enregistrée, ou la raison du refus (affichée au joueur).
pub fn accept(registry: &Registry, answer: &str) -> Result<ContentEntry, String> {
    // Les modèles entourent souvent le RON de texte ou de ``` : on garde du
    // premier `(` au dernier `)`.
    let ron = match (answer.find('('), answer.rfind(')')) {
        (Some(start), Some(end)) if start < end => &answer[start..=end],
        _ => return Err("pas d'entrée RON dans la réponse".into()),
    };
    let entry: ContentEntry = parse(ron).map_err(|e: LoadError| e.to_string())?;
    if !entry.identifier.starts_with(NAMESPACE) {
        return Err(format!("{} : l'identifier doit commencer par {NAMESPACE}", entry.identifier));
    }
    for def in entry.block().into_iter().flat_map(|b| &b.generated_textures) {
        if !def.name.starts_with(NAMESPACE) || registry.texture_layer(&def.name).is_some() {
            return Err(format!("texture {} : doit être nouvelle et commencer par {NAMESPACE}", def.name));
        }
        texgen::paint(def, 16).map_err(|e| format!("texture {} : {e}", def.name))?;
    }
    // Les textures citées existent déjà ou sont calculées par l'entrée.
    if let Some(block) = entry.block() {
        let faces = block.texture.iter().flat_map(|f| [Some(&f.side), f.top.as_ref(), f.bottom.as_ref()]).flatten();
        for name in faces {
            let declared = block.generated_textures.iter().any(|g| &g.name == name);
            if !declared && registry.texture_layer(name).is_none() {
                return Err(format!("texture {name} inconnue"));
            }
        }
    }
    // Identifier neuf et références existantes : les mêmes contrôles qu'au
    // chargement d'un fichier de contenu.
    let mut trial = registry.clone();
    trial.register(entry.clone()).map_err(|_| format!("{} existe déjà", entry.identifier))?;
    trial.check_references().map_err(|e| e.to_string())?;
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Registry {
        Registry::from_ron(include_str!("../../../assets/content/core.ron")).unwrap()
    }

    const LAVA: &str = r#"Voici le bloc :
```ron
(
    identifier: "gen:lava",
    kind: Block((solid: true, color: (0.9, 0.3, 0.1),
        texture: Some((side: "gen:lava")),
        generated_textures: [(name: "gen:lava", pattern: Spots, color: (0.9, 0.3, 0.1), noise: 0.3, seed: 4)],
        rules: [(on: Used, then: [Drop("core:stone")])])),
    recipes: [(inputs: ["core:stone", "core:dirt"])],
)
```"#;

    #[test]
    fn a_valid_answer_is_accepted_and_its_texture_painted() {
        let mut reg = registry();
        let entry = accept(&reg, LAVA).unwrap();
        assert_eq!(entry.identifier, "gen:lava");
        reg.register(entry).unwrap();
        reg.load_textures(std::path::Path::new("../../assets/textures")).unwrap();
        let layer = reg.texture_layer("gen:lava").unwrap() as usize;
        assert_eq!(reg.textures()[layer].size, reg.textures()[0].size);
    }

    #[test]
    fn invalid_answers_are_refused() {
        let reg = registry();
        let cases = [
            ("pas de RON", "rien"),
            ("hors namespace", r#"(identifier: "core:lava", kind: Block((solid: true, color: (1.0, 0.0, 0.0))))"#),
            ("existe déjà", r#"(identifier: "gen:a", kind: Block((solid: true, color: (1.0, 0.0, 0.0))))"#),
            ("référence absente", r#"(identifier: "gen:b", kind: Block((solid: true, color: (1.0, 0.0, 0.0), drops: Some(["core:gold"]))))"#),
            ("vocabulaire inconnu", r#"(identifier: "gen:c", kind: Block((solid: true, color: (1.0, 0.0, 0.0), rules: [(on: Exploded, then: [])])))"#),
            ("texture inconnue", r#"(identifier: "gen:d", kind: Block((solid: true, color: (1.0, 0.0, 0.0), texture: Some((side: "gen:x")))))"#),
            ("texture du jeu écrasée", r#"(identifier: "gen:e", kind: Block((solid: true, color: (1.0, 0.0, 0.0),
                generated_textures: [(name: "stone", pattern: Plain, color: (1.0, 0.0, 0.0))])))"#),
            ("bruit hors bornes", r#"(identifier: "gen:f", kind: Block((solid: true, color: (1.0, 0.0, 0.0),
                generated_textures: [(name: "gen:f", pattern: Plain, color: (1.0, 0.0, 0.0), noise: 3.0)])))"#),
        ];
        let mut reg_with_a = reg.clone();
        reg_with_a.register(accept(&reg, cases[2].1).unwrap()).unwrap();
        for (why, answer) in cases {
            assert!(accept(&reg_with_a, answer).is_err(), "{why} : accepté");
        }
    }
}
