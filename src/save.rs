//! Persistance côté app (jalon 5, #8) : branche `voxel_core::save` sur
//! l'ECS. Le monde est chargé dans `setup_world` (main.rs), les chunks par
//! le streaming ; ici, ce qui écrit, et la restauration du joueur et des
//! items au sol.

use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;

use bevy::prelude::*;

use voxel_core::chunk::ChunkPos;
use voxel_core::save::{encode_chunk, PlayerSave, Save};

use crate::inventory::Inventory;
use crate::entities::{Mob, MobIds};
use crate::items::{DroppedItem, ItemVisual};
use crate::player::{Player, PlayerCamera};
use crate::GameWorld;

/// Toutes les combien de secondes joueur et items au sol sont écrits (en
/// plus de la fermeture) : ce qu'on perd au pire si le jeu plante.
const PLAYER_SAVE_PERIOD_S: f32 = 5.0;

/// Le dossier du monde en cours. Absent (tests) : rien n'est lu ni écrit.
#[derive(Resource)]
pub struct WorldSave(pub Save);

/// Encode les chunks modifiés cette frame et les passe au fil d'écriture.
/// Les modifications sont rares (un clic) : écrire tout de suite évite de
/// dépendre de la fermeture.
pub fn save_edited_chunks(mut game: ResMut<GameWorld>, writer: Option<Res<ChunkWriter>>) {
    let Some(writer) = writer else { return };
    for pos in game.world.take_edited() {
        if let Some(chunk) = game.world.chunk(pos) {
            writer.send(pos, encode_chunk(chunk, &game.world.chunk_state(pos)));
        }
    }
}

/// Le fil qui écrit les chunks sur le disque (#54) : encoder un chunk prend
/// 0,02 ms, écrire son fichier jusqu'à 90 ms sous Windows. Un seul fil, qui
/// écrit dans l'ordre d'envoi : deux écritures du même chunk ne se croisent
/// pas, la dernière gagne.
#[derive(Resource)]
pub struct ChunkWriter {
    send: Option<Sender<(ChunkPos, Vec<u8>)>>,
    thread: Option<JoinHandle<()>>,
}

impl ChunkWriter {
    pub fn new(save: Save) -> Self {
        let (send, receive) = mpsc::channel::<(ChunkPos, Vec<u8>)>();
        let thread = std::thread::Builder::new()
            .name("sauvegarde des chunks".into())
            .spawn(move || {
                for (pos, bytes) in receive {
                    if let Err(err) = save.write_chunk_bytes(pos, &bytes) {
                        error!("sauvegarde du chunk {pos:?} impossible : {err}");
                    }
                }
            })
            .expect("lancement du fil de sauvegarde");
        Self { send: Some(send), thread: Some(thread) }
    }

    fn send(&self, pos: ChunkPos, bytes: Vec<u8>) {
        if let Some(send) = &self.send {
            send.send((pos, bytes)).expect("fil de sauvegarde vivant");
        }
    }

    /// Attend que tout ce qui a été envoyé soit écrit.
    fn finish(&mut self) {
        self.send = None;
        if let Some(thread) = self.thread.take() {
            thread.join().expect("fil de sauvegarde sans panique");
        }
    }
}

/// À la fermeture, attend les écritures de chunks en cours : sans ça, le
/// dernier edit peut être perdu.
pub fn finish_chunk_writes(mut exit: MessageReader<AppExit>, writer: Option<ResMut<ChunkWriter>>) {
    if exit.read().count() > 0
        && let Some(mut writer) = writer
    {
        writer.finish();
    }
}

/// Écrit le joueur (position, regard, inventaire), les items au sol, les
/// entités posées et l'horloge du monde :
/// périodiquement, et à la fermeture (le `AppExit` est émis dans `Last`,
/// d'où l'ordre). Les items bougent sans cesse (chute, ramassage) : les
/// écrire au même rythme que le joueur plutôt qu'à chaque changement.
// Paramètres de système Bevy : c'est la forme normale (cf. `interact`).
#[allow(clippy::too_many_arguments)]
pub fn save_player_and_items(
    time: Res<Time>,
    mut since: Local<f32>,
    mut exit: MessageReader<AppExit>,
    save: Option<Res<WorldSave>>,
    player: Query<&Player>,
    inventory: Res<Inventory>,
    items: Query<&DroppedItem>,
    mobs: Query<&Mob>,
    game: Res<GameWorld>,
) {
    *since += time.delta_secs();
    let exiting = exit.read().count() > 0;
    if !exiting && *since < PLAYER_SAVE_PERIOD_S {
        return;
    }
    *since = 0.0;
    let Some(save) = save else { return };
    if let Err(err) = save.0.write_clock(game.world.tick()) {
        error!("sauvegarde de l'horloge impossible : {err}");
    }
    let dropped = items.iter().map(|i| (i.content, i.feet().to_array())).collect();
    let placed = mobs.iter().map(|m| (m.id, m.content, m.body.feet, m.yaw)).collect();
    if let Err(err) = save.0.write_mobs(&placed) {
        error!("sauvegarde des entités impossible : {err}");
    }
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

/// Refait apparaître les entités posées sauvées (#15).
pub fn restore_mobs(mut commands: Commands, mut ids: ResMut<MobIds>, save: Option<Res<WorldSave>>) {
    let Some(save) = save else { return };
    match save.0.read_mobs() {
        Ok(mobs) => {
            for (id, content, feet, yaw) in mobs {
                ids.next = ids.next.max(id + 1);
                commands.spawn(Mob(voxel_core::mob::Mob::new(id, content, feet, yaw)));
            }
        }
        // Comme les items : illisibles, on les perd plutôt que le monde.
        Err(err) => error!("entités non restaurées : {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_keeps_the_last_version_and_finish_waits_for_it() {
        let dir = std::env::temp_dir().join(format!("voxel_writer_test_{}", std::process::id()));
        let mut writer = ChunkWriter::new(Save::new(&dir));
        let pos = ChunkPos { x: 1, y: 0, z: -2 };
        for i in 0..50u32 {
            writer.send(pos, i.to_le_bytes().to_vec());
        }
        writer.finish();
        let bytes = std::fs::read(dir.join("chunks/1_0_-2.bin")).unwrap();
        assert_eq!(bytes, 49u32.to_le_bytes(), "la dernière version gagne");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
