//! Contenu du jeu : `assets/content/core.ron` et `assets/textures/`, lus au
//! démarrage et **relus en cours de partie** (touche R, #17).
//!
//! Recharger = le même chemin que charger une save (§3.10) : le contenu est
//! fusionné dans le registre courant par identifier. Les IDs ne bougent
//! pas (les chunks les citent), une entrée modifiée prend sa nouvelle
//! définition, une nouvelle est ajoutée à la fin, une entrée retirée du
//! fichier reste (append-only, §3.1).
//!
//! Un bloc généré en jeu (#18, `generate.rs`) entre par le même chemin :
//! son registre arrive dans [`Incoming`].

use bevy::prelude::*;

use voxel_core::registry::Registry;

use crate::inventory::Inventory;
use crate::items::ItemVisual;
use crate::save::WorldSave;
use crate::{assets_dir, texture_array, ChunkMesh, DirtyChunks, GameWorld};

/// Fusionne `core.ron` et les textures dans `base` (registre d'une save, ou
/// vide pour un monde neuf). Rien n'est touché en cas d'erreur.
pub fn load(mut base: Registry) -> Result<Registry, String> {
    let path = assets_dir().join("content/core.ron");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{} : {e}", path.display()))?;
    let content = Registry::from_ron(&text).map_err(|e| format!("{} : {e}", path.display()))?;
    base.merge(content).map_err(|e| e.to_string())?;
    base.load_textures(&assets_dir().join("textures")).map_err(|e| e.to_string())?;
    Ok(base)
}

/// Résultat du dernier rechargement, affiché par le HUD debug.
#[derive(Resource, Default)]
pub struct ReloadStatus(pub String);

/// Un registre déjà prêt à remplacer le courant (bloc généré).
#[derive(Resource, Default)]
pub struct Incoming(pub Option<Registry>);

/// Touche R : relit le contenu. Une erreur (fichier mal écrit, PNG absent)
/// laisse le registre en place et s'affiche — la partie continue.
// Un système Bevy prend ses ressources en paramètres : c'est la forme normale.
#[allow(clippy::too_many_arguments)]
pub fn apply_content(
    keys: Res<ButtonInput<KeyCode>>,
    mut incoming: ResMut<Incoming>,
    typing: Query<(), With<crate::ui::TextField>>,
    mut game: ResMut<GameWorld>,
    mut status: ResMut<ReloadStatus>,
    mut dirty: ResMut<DirtyChunks>,
    mut inventory: ResMut<Inventory>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<crate::VoxelMaterial>>,
    save: Option<Res<WorldSave>>,
    chunks: Query<&ChunkMesh>,
    mut visuals: Query<&mut ItemVisual>,
) {
    let registry = if let Some(registry) = incoming.0.take() {
        registry
    } else if keys.just_pressed(KeyCode::KeyR) && typing.is_empty() {
        match load(game.world.registry.clone()) {
            Ok(registry) => registry,
            Err(err) => {
                error!("contenu non rechargé : {err}");
                status.0 = format!("contenu NON rechargé : {err}");
                return;
            }
        }
    } else {
        return;
    };
    if let Some(save) = save
        && let Err(err) = save.0.write_registry(&registry)
    {
        error!("registre non sauvé : {err}");
    }
    // Les couches de texture et les couleurs sont dans les sommets : tout
    // chunk affiché est à re-mesher.
    let array = texture_array(&registry);
    game.world.registry = registry;
    dirty.0.extend(chunks.iter().map(|c| c.0));
    // Nouvelle image sous un nouveau handle : remplacer l'image sous le
    // même handle laissait le matériau sur l'ancienne (un bloc généré
    // ajoute une couche, son indice sortait de l'ancien array et le GPU le
    // ramenait à la dernière couche). L'ancienne image est libérée avec
    // son handle.
    let array = images.add(array);
    for handle in [&game.material, &game.blend_material] {
        if let Some(mut material) = materials.get_mut(handle) {
            material.extension.array = array.clone();
        }
    }
    // Couleurs et noms : cubes d'items, barre, menu de fabrication.
    visuals.iter_mut().for_each(|mut v| v.set_changed());
    inventory.set_changed();
    status.0 = format!("contenu rechargé ({} entrées)", game.world.registry.iter().count());
}
