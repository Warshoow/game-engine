//! Persistance côté app (jalon 5, #8) : branche `voxel_core::save` sur
//! l'ECS. Le monde est chargé dans `setup_world` (main.rs), les chunks par
//! le streaming ; ici, ce qui écrit, et la restauration du joueur et des
//! items au sol.

use bevy::prelude::*;

use voxel_core::save::{PlayerSave, Save};

use crate::inventory::Inventory;
use crate::items::{DroppedItem, ItemVisual};
use crate::player::{Player, PlayerCamera};
use crate::GameWorld;

/// Toutes les combien de secondes joueur et items au sol sont écrits (en
/// plus de la fermeture) : ce qu'on perd au pire si le jeu plante.
const PLAYER_SAVE_PERIOD_S: f32 = 5.0;

/// Le dossier du monde en cours. Absent (tests) : rien n'est lu ni écrit.
#[derive(Resource)]
pub struct WorldSave(pub Save);

/// Écrit les chunks modifiés cette frame. Les modifications sont rares
/// (un clic) : écrire tout de suite évite de dépendre de la fermeture.
pub fn save_edited_chunks(mut game: ResMut<GameWorld>, save: Option<Res<WorldSave>>) {
    let Some(save) = save else { return };
    for pos in game.world.take_edited() {
        if let Err(err) = save.0.write_chunk(&game.world, pos) {
            error!("sauvegarde du chunk {pos:?} impossible : {err}");
        }
    }
}

/// Écrit le joueur (position, regard, inventaire) et les items au sol :
/// périodiquement, et à la fermeture (le `AppExit` est émis dans `Last`,
/// d'où l'ordre). Les items bougent sans cesse (chute, ramassage) : les
/// écrire au même rythme que le joueur plutôt qu'à chaque changement.
pub fn save_player_and_items(
    time: Res<Time>,
    mut since: Local<f32>,
    mut exit: MessageReader<AppExit>,
    save: Option<Res<WorldSave>>,
    player: Query<&Player>,
    inventory: Res<Inventory>,
    items: Query<&DroppedItem>,
) {
    *since += time.delta_secs();
    let exiting = exit.read().count() > 0;
    if !exiting && *since < PLAYER_SAVE_PERIOD_S {
        return;
    }
    *since = 0.0;
    let Some(save) = save else { return };
    let dropped = items.iter().map(|i| (i.content, i.feet().to_array())).collect();
    if let Err(err) = save.0.write_items(&dropped) {
        error!("sauvegarde des items au sol impossible : {err}");
    }
    let Ok(player) = player.single() else { return };
    let (yaw, pitch) = player.look();
    let (inventory, selected) = inventory.to_save();
    let state = PlayerSave { feet: player.feet().to_array(), yaw, pitch, inventory, selected };
    if let Err(err) = save.0.write_player(&state) {
        error!("sauvegarde du joueur impossible : {err}");
    }
}

/// Remet le joueur où il était (après `spawn_player`, qui le pose sur le
/// terrain d'un monde neuf).
pub fn restore_player(
    save: Option<Res<WorldSave>>,
    mut inventory: ResMut<Inventory>,
    mut player: Query<(&mut Player, &mut Transform), Without<PlayerCamera>>,
    mut camera: Query<&mut Transform, With<PlayerCamera>>,
) {
    let Some(save) = save else { return };
    let state = match save.0.read_player() {
        Ok(Some(state)) => state,
        Ok(None) => return,
        // Joueur illisible : on repart du spawn plutôt que de refuser tout
        // le monde (rien de perdu côté voxels).
        Err(err) => return error!("joueur non restauré : {err}"),
    };
    let Ok((mut player, mut transform)) = player.single_mut() else { return };
    let feet = Vec3::from_array(state.feet);
    *player = Player::restored(feet, state.yaw, state.pitch);
    transform.translation = feet;
    transform.rotation = Quat::from_rotation_y(state.yaw);
    if let Ok(mut cam) = camera.single_mut() {
        cam.rotation = Quat::from_rotation_x(state.pitch);
    }
    inventory.restore(state.inventory, state.selected);
}

/// Refait apparaître les items au sol sauvés.
pub fn restore_items(mut commands: Commands, save: Option<Res<WorldSave>>) {
    let Some(save) = save else { return };
    let items = match save.0.read_items() {
        Ok(items) => items,
        // Comme le joueur : illisible, on perd les items plutôt que le monde.
        Err(err) => return error!("items au sol non restaurés : {err}"),
    };
    for (id, feet) in items {
        let item = DroppedItem::restored(id, Vec3::from_array(feet));
        commands.spawn((item, ItemVisual(id), Transform::default()));
    }
}
