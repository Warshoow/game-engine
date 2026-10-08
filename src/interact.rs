//! Pose / casse de voxels — critère §7.3 de la slice.
//!
//! Clic gauche : casser le voxel visé, qui laisse tomber ses drops
//! (`Registry::drops`) en items au sol. Clic droit : *utiliser* le bloc visé
//! s'il a une règle `Used` (§3.6), sinon poser le bloc en main (pris dans
//! l'[`Inventory`]) sur la face visée ; Maj + clic droit pose toujours. Les
//! hooks `Placed` et `Broken` déclenchent les règles du bloc concerné. Tout est **data-driven** :
//! des `ContentId` du registre, jamais un type en dur — le système ne sait
//! pas ce qu'il casse ni ce qu'il pose.
//!
//! Après une écriture, le chunk touché est noté à re-mesher intégralement
//! ([`DirtyChunks`]). C'est brut (on reconstruit 32³ voxels pour un
//! changement d'un seul) mais largement assez rapide — et c'est le *même*
//! chemin de meshing que le streaming (`remesh_dirty`, main.rs).

use bevy::prelude::*;

use voxel_core::crafting;
use voxel_core::registry::ContentId;
use voxel_core::rules::{self, Action, Context, Hook};
use voxel_core::physics::Aabb;
use voxel_core::raycast::{raycast, RayHit};
use voxel_core::world::VoxelWorld;

use crate::player::{CursorCaptured, Player, PLAYER_HEIGHT_M, PLAYER_WIDTH_M, PlayerCamera};
use crate::inventory::Inventory;
use crate::items::{DroppedItem, ItemVisual, StorageChanged};
use crate::{DirtyChunks, GameWorld};

/// Portée de la main, en mètres (§2 — jamais « en blocs »).
const REACH_M: f32 = 5.0;

/// Le voxel visé depuis l'œil, à portée de main. Partagé par la pose/casse
/// et la surbrillance : le contour montre exactement ce qu'un clic touchera.
fn aim(world: &VoxelWorld, cam: &GlobalTransform) -> Option<RayHit> {
    raycast(
        world,
        cam.translation().to_array(),
        cam.forward().as_vec3().to_array(),
        REACH_M,
    )
}

/// Contour du voxel visé (gizmo, redessiné à chaque frame). Rien hors mode
/// FPS — on ne peut pas interagir — ni quand le regard ne touche rien.
pub fn highlight_target(
    captured: Res<CursorCaptured>,
    game: Res<GameWorld>,
    camera: Query<&GlobalTransform, With<PlayerCamera>>,
    mut gizmos: Gizmos,
) {
    if !captured.0 {
        return;
    }
    let Ok(cam) = camera.single() else { return };
    let Some(hit) = aim(&game.world, cam) else { return };
    let size_m = 1.0 / game.world.voxels_per_meter();
    let center = (Vec3::from_array(hit.voxel.map(|v| v as f32)) + 0.5) * size_m;
    // Un poil plus grand que le voxel : sinon les lignes se confondent avec
    // les faces et clignotent (z-fighting).
    gizmos.cube(
        Transform::from_translation(center).with_scale(Vec3::splat(size_m * 1.01)),
        Color::BLACK,
    );
}

// Un système Bevy prend ses ressources en paramètres : 10 ici (entrée,
// monde, inventaire, caméra, joueur…), c'est la forme normale.
#[allow(clippy::too_many_arguments)]
pub fn interact(
    mut commands: Commands,
    mut inventory: ResMut<Inventory>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    captured: Res<CursorCaptured>,
    mut game: ResMut<GameWorld>,
    mut dirty: ResMut<DirtyChunks>,
    mut storage: ResMut<StorageChanged>,
    camera: Query<&GlobalTransform, With<PlayerCamera>>,
    player: Query<&Player>,
) {
    // On n'interagit qu'en mode FPS — et comme ce système tourne AVANT
    // `cursor_grab` (voir l'ordre du plugin), le clic qui active le mode
    // ne casse pas de bloc au passage.
    let breaking = mouse.just_pressed(MouseButton::Left);
    let right_click = mouse.just_pressed(MouseButton::Right);
    if !captured.0 || (!breaking && !right_click) {
        return;
    }
    let Ok(cam) = camera.single() else { return };
    let Some(hit) = aim(&game.world, cam) else { return };
    let ctx = Context { holding: inventory.selected() };
    let mut edit = Edit {
        commands: &mut commands,
        game: &mut game,
        dirty: &mut dirty,
        storage: &mut storage,
        inventory: &mut inventory,
    };

    if breaking {
        let Some(broken) = edit.game.world.voxel(hit.voxel) else { return };
        let air = edit.game.air;
        if !edit.set(hit.voxel, air) {
            return;
        }
        for drop in edit.game.world.registry.drops(broken) {
            edit.drop(hit.voxel, drop);
        }
        let actions = rules::actions(&edit.game.world.registry, broken, Hook::Broken, ctx);
        edit.apply(hit.voxel, &actions);
        return;
    }

    // Clic droit sur un bloc qui a une règle `Used` : on l'utilise. Maj +
    // clic droit pose quand même (sinon impossible de poser contre lui).
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    if let Some(target) = edit.game.world.voxel(hit.voxel)
        && !shift
        && rules::has_rule(&edit.game.world.registry, target, Hook::Used)
    {
        let actions = rules::actions(&edit.game.world.registry, target, Hook::Used, ctx);
        edit.apply(hit.voxel, &actions);
        return;
    }

    // Poser. Seul un bloc se pose : un item en main (sans `places`, pas
    // encore implémenté) ne fait rien.
    let Some(held) = edit.inventory.selected().filter(|&id| {
        edit.game.world.registry.get(id).is_some_and(|e| e.block().is_some())
    }) else {
        return;
    };
    // Sur la face d'entrée du rayon. Normale nulle = l'œil est dans un
    // solide, pas de face → rien.
    if hit.normal == [0; 3] {
        return;
    }
    let target = [
        hit.voxel[0] + hit.normal[0] as i64,
        hit.voxel[1] + hit.normal[1] as i64,
        hit.voxel[2] + hit.normal[2] as i64,
    ];
    // Refuse de poser un bloc dans le volume du joueur.
    if player
        .single()
        .is_ok_and(|p| voxel_overlaps_player(&edit.game.world, target, p.feet()))
    {
        return;
    }
    if edit.set(target, held) {
        edit.inventory.take_selected();
        let actions = rules::actions(&edit.game.world.registry, held, Hook::Placed, ctx);
        edit.apply(target, &actions);
    }
}

/// Ce qu'une modification du monde touche : le monde, les chunks à
/// re-mesher, les blocs dont le contenu posé change, l'inventaire, et les
/// entités à faire apparaître.
struct Edit<'a, 'w, 's> {
    commands: &'a mut Commands<'w, 's>,
    game: &'a mut GameWorld,
    dirty: &'a mut DirtyChunks,
    storage: &'a mut StorageChanged,
    inventory: &'a mut Inventory,
}

impl Edit<'_, '_, '_> {
    /// Écrit un voxel et note les chunks à re-mesher. `false` si le chunk
    /// n'est pas chargé (rien n'a changé).
    fn set(&mut self, voxel: [i64; 3], id: ContentId) -> bool {
        // Le contenu posé sur l'ancien bloc tombe au sol (§3.3 : l'état
        // disparaît avec le bloc).
        let stored = self.game.world.take_stored(voxel);
        let Some(pos) = self.game.world.set_voxel(voxel, id) else { return false };
        for item in stored {
            self.drop(voxel, item);
        }
        self.storage.0.insert(voxel);
        self.dirty.0.insert(pos);
        // Culling inter-chunks : un voxel en bordure change aussi les faces
        // du chunk voisin (sa face culled peut devoir (ré)apparaître).
        let size = self.game.world.chunk_size();
        let (_, local) = self.game.world.split(voxel);
        for (axis, &l) in local.iter().enumerate() {
            let offset: i32 = match l {
                0 => -1,
                l if l == size - 1 => 1,
                _ => continue,
            };
            let mut npos = pos;
            match axis {
                0 => npos.x += offset,
                1 => npos.y += offset,
                _ => npos.z += offset,
            }
            self.dirty.0.insert(npos);
        }
        true
    }

    /// Fait tomber un item au centre du voxel — juste au-dessus s'il est
    /// solide (produit d'un établi) : né dans un solide, l'item y resterait
    /// coincé, son petit saut ne suffisant pas à en sortir.
    fn drop(&mut self, voxel: [i64; 3], id: ContentId) {
        let size_m = 1.0 / self.game.world.voxels_per_meter();
        let lift = if self.game.world.is_solid(voxel) { 1.0 } else { 0.0 };
        let center = (Vec3::from_array(voxel.map(|v| v as f32)) + 0.5 + Vec3::Y * lift) * size_m;
        self.commands.spawn((
            DroppedItem::new(id, center),
            ItemVisual(id),
            Transform::from_translation(center),
        ));
    }

    /// Applique les actions d'une règle au bloc `voxel`. Un `SetSelf` ne
    /// redéclenche aucun hook (pas de cascade de règles).
    fn apply(&mut self, voxel: [i64; 3], actions: &[Action]) {
        for &action in actions {
            match action {
                Action::SetSelf(id) => {
                    self.set(voxel, id);
                }
                Action::Drop(id) => self.drop(voxel, id),
                Action::StoreHeld => {
                    if let Some(held) = self.inventory.selected()
                        && self.game.world.store(voxel, held)
                    {
                        self.inventory.take_selected();
                        self.storage.0.insert(voxel);
                    }
                }
                Action::Craft => {
                    let items = self.game.world.take_stored(voxel);
                    let station = self.game.world.voxel(voxel);
                    let product = station.and_then(|s| crafting::find(&self.game.world.registry, s, &items));
                    match product {
                        Some((id, count)) => (0..count).for_each(|_| self.drop(voxel, id)),
                        None => items.into_iter().for_each(|id| self.drop(voxel, id)),
                    }
                    self.storage.0.insert(voxel);
                }
            }
        }
    }
}

fn voxel_overlaps_player(
    world: &VoxelWorld,
    voxel: [i64; 3],
    player_feet: Vec3,
) -> bool {
    let vpm = world.voxels_per_meter();
    let vmin = [
        voxel[0] as f32 / vpm,
        voxel[1] as f32 / vpm,
        voxel[2] as f32 / vpm,
    ];
    let vmax = [vmin[0] + 1.0 / vpm, vmin[1] + 1.0 / vpm, vmin[2] + 1.0 / vpm];
    let p = Aabb::from_feet(player_feet.to_array(), PLAYER_WIDTH_M, PLAYER_HEIGHT_M);
    (0..3).all(|a| vmin[a] < p.max[a] && vmax[a] > p.min[a])
}
