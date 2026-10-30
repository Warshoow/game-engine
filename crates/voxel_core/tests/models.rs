//! Modèles d'entités (#15) à travers l'API publique : chargement avec ses
//! textures, et item au sol dessiné comme son bloc.

use voxel_core::mesher::mesh_block_item;
use voxel_core::registry::Registry;

const CONTENT: &str = r#"[
    (identifier: "t:air", kind: Block((solid: false, color: (0.0, 0.0, 0.0)))),
    (identifier: "t:slab", kind: Block((solid: true, color: (0.5, 0.5, 0.5),
        texture: Some((side: "plank")),
        generated_textures: [(name: "plank", pattern: Planks, color: (0.6, 0.4, 0.2))],
        shape: Some([((0, 0, 0), (16, 8, 16))])))),
    (identifier: "t:pig", kind: EntityType((size: (0.9, 0.9), model: [
        (name: "body", pivot: (0, 6, 0), boxes: [((-5, 6, -8), (5, 14, 8), "hide")]),
        (name: "head", parent: "body", pivot: (0, 12, 8), boxes: [((-4, 8, 8), (4, 16, 14), "snout")]),
    ], generated_textures: [
        (name: "hide", pattern: Spots, color: (0.9, 0.6, 0.6), seed: 1),
        (name: "snout", pattern: Plain, color: (0.8, 0.5, 0.5)),
    ]))),
]"#;

fn registry() -> Registry {
    let mut reg = Registry::from_ron(CONTENT).unwrap();
    reg.load_textures(std::path::Path::new("")).unwrap();
    reg
}

#[test]
fn an_entity_loads_with_its_own_textures() {
    let reg = registry();
    let pig = reg.get(reg.lookup("t:pig").unwrap()).unwrap().entity().unwrap();
    let head = pig.model[1].mesh(&reg);
    assert!(head.layers.iter().all(|&l| l == reg.texture_layer("snout").unwrap()));
    // Une texture citée mais ni PNG ni calculée : refusée au chargement.
    let mut missing = Registry::from_ron(&CONTENT.replace("\"snout\")]", "\"tusk\")]")).unwrap();
    assert!(missing.load_textures(std::path::Path::new("")).is_err());
    // Un modèle invalide est refusé comme un bloc invalide.
    assert!(Registry::from_ron(&CONTENT.replace("parent: \"body\"", "parent: \"tail\"")).is_err());
}

#[test]
fn a_block_item_is_its_block_in_small() {
    let reg = registry();
    let mesh = mesh_block_item(&reg, reg.lookup("t:slab").unwrap(), 0.25).unwrap();
    assert_eq!(mesh.face_count(), 6);
    let ys: Vec<f32> = mesh.positions.iter().map(|p| p[1]).collect();
    assert_eq!(ys.iter().cloned().fold(f32::MAX, f32::min), -0.125);
    assert_eq!(ys.iter().cloned().fold(f32::MIN, f32::max), 0.0, "une dalle : la moitié basse");
    // Le dessus montre toute la texture, pas un quart.
    let top: Vec<[f32; 2]> = (0..mesh.positions.len()).filter(|&i| mesh.normals[i] == [0.0, 1.0, 0.0]).map(|i| mesh.uvs[i]).collect();
    let span = |a: usize| top.iter().map(|uv| uv[a]).fold(f32::MIN, f32::max) - top.iter().map(|uv| uv[a]).fold(f32::MAX, f32::min);
    assert_eq!((span(0), span(1)), (1.0, 1.0));
    assert!(mesh_block_item(&reg, reg.lookup("t:pig").unwrap(), 0.25).is_none());
}
