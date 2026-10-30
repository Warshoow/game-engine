//! Pose / casse de voxels — critère §7.3 de la slice.
//!
//! Clic gauche : casser le voxel visé, qui laisse tomber ses drops
//! (`Registry::drops`) en items au sol. Clic droit : *utiliser* le bloc visé
//! s'il a une règle `Used` (§3.6), sinon poser le bloc en main (pris dans
//! l'[`Inventory`]) sur la face visée ; Maj + clic droit pose toujours. Les
//! hooks `Placed` et `Broken` déclenchent les règles du bloc concerné. Un
//! bloc qui déclare `orient` (#14) se tourne selon le regard et la face
//! visée. Tout est **data-driven** :
//! des `ContentId` du registre, jamais un type en dur — le système ne sait
//! pas ce qu'il casse ni ce qu'il pose.
//!
//! Blocs qui évoluent seuls (#34) : au tick fixe, [`tick_blocks`] avance
//! l'horloge du monde, passe le hook `Tick` aux voxels tirés par
//! `voxel_core::ticks` et déclenche les minuteurs (`After`) échus. Leurs
//! règles passent par le même chemin que le joueur, sans main ni
//! inventaire. Un bloc qui apparaît, quelle qu'en soit la cause, lance ses
//! minuteurs ([`Edit::set`]).
//!
//! Liquides et voisinage (#28) : [`tick_blocks`] traite aussi les réexamens
//! échus — l'écoulement (`voxel_core::liquids`) puis le hook
//! `NeighborChanged`.
//!
//! Après une écriture, le chunk touché est noté à re-mesher intégralement
//! ([`DirtyChunks`]). C'est brut (on reconstruit 32³ voxels pour un
//! changement d'un seul) mais le meshing tourne hors de l'image (#53) — et
//! c'est le *même* chemin de meshing que le streaming (`remesh_dirty`,
//! main.rs). Les chunks touchés par un même edit changent à la même image.

use bevy::prelude::*;

use voxel_core::chunk::meshes_reading;
use voxel_core::crafting;
use voxel_core::registry::ContentId;
use voxel_core::rules::{self, Action, Context, Hook};
use voxel_core::ticks;
use voxel_core::physics::Aabb;
use voxel_core::raycast::{raycast, RayHit};
use voxel_core::liquids;
use voxel_core::shape::{Cuboid, Rotation};
use voxel_core::world::{neighbors, VoxelWorld, MAX_UPDATES_PER_TICK, TICKS_PER_SECOND};

use crate::player::{CursorCaptured, Player, PLAYER_HEIGHT_M, PLAYER_WIDTH_M, PlayerCamera};
use crate::inventory::Inventory;
use crate::entities;
use crate::items::{DroppedItem, ItemVisual, StorageChanged};
use crate::{ChunkMesh, DirtyChunks, GameWorld};

/// Portée de la main, en mètres (§2 — jamais « en blocs »).
const REACH_M: f32 = 5.0;

/// Temps moyen entre deux ticks d'un même bloc, en secondes.
const TICK_PERIOD_S: f64 = 60.0;

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

/// Contour du voxel visé (gizmo, redessiné à chaque frame) : ses boîtes
/// pour un bloc à forme (#14). Rien hors mode
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
    for [min, max] in game.world.boxes(hit.voxel, false) {
        let (min, max) = (Vec3::from_array(min) * size_m, Vec3::from_array(max) * size_m);
        // Un poil plus grand que la boîte : sinon les lignes se confondent
        // avec les faces et clignotent (z-fighting).
        gizmos.cube(
            Transform::from_translation((min + max) / 2.0).with_scale((max - min) + size_m * 0.01),
            Color::BLACK,
        );
    }
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
    let now = game.world.tick();
    let holding = inventory.selected();
    let ctx = context(&game, holding, now, hit.voxel);
    let mut edit = Edit {
        commands: &mut commands,
        game: &mut game,
        dirty: &mut dirty,
        storage: &mut storage,
        inventory: Some(&mut *inventory),
        from: now,
        late: 0,
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

    // Clic droit sur un bloc dont une règle `Used` s'applique à ce qu'on
    // tient : on l'utilise. Sinon, ou avec Maj, on pose contre lui.
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    if let Some(target) = edit.game.world.voxel(hit.voxel)
        && !shift
        && rules::usable(&edit.game.world.registry, target, ctx)
    {
        let actions = rules::actions(&edit.game.world.registry, target, Hook::Used, ctx);
        edit.apply(hit.voxel, &actions);
        return;
    }

    // Une entité en main (#15) : posée sur la case devant la face visée,
    // tournée vers le joueur.
    if let Some(id) = holding.filter(|&id| edit.game.world.registry.get(id).is_some_and(|e| e.entity().is_some())) {
        if hit.normal == [0; 3] {
            return;
        }
        let size_m = 1.0 / edit.game.world.voxels_per_meter();
        let cell = Vec3::new(
            (hit.voxel[0] + hit.normal[0] as i64) as f32,
            (hit.voxel[1] + hit.normal[1] as i64) as f32,
            (hit.voxel[2] + hit.normal[2] as i64) as f32,
        );
        let feet = (cell + Vec3::new(0.5, 0.0, 0.5)) * size_m;
        let look = cam.forward().as_vec3();
        entities::spawn_mob(edit.commands, id, feet, (-look.x).atan2(-look.z), false);
        edit.inventory.as_deref_mut().map(Inventory::take_selected);
        return;
    }

    // Poser. Seul un bloc se pose : un item en main (sans `places`, pas
    // encore implémenté) ne fait rien.
    let Some(held) = holding.filter(|&id| {
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
    // Tourné selon le regard et le point visé sur la face (#14).
    let world = &edit.game.world;
    // La case peut être occupée : le rayon traverse le vide d'un bloc à
    // forme (le bas d'une dalle haute, autour d'une torche) jusqu'au bloc
    // derrière. On ne le remplace pas (fusion de deux dalles : #58).
    if world.voxel(target).and_then(|v| world.registry.get(v)?.block()).is_some_and(|b| b.solid) {
        return;
    }
    let orient = world.registry.get(held).and_then(|e| e.block()).map(|b| b.orient).unwrap_or_default();
    let look = cam.forward().as_vec3();
    let point_y = (cam.translation().y + look.y * hit.distance_m) * world.voxels_per_meter();
    let rotation = Rotation::placed(orient, look.to_array(), hit.normal, point_y - hit.voxel[1] as f32 > 0.5);
    // Refuse de poser un bloc dans le volume du joueur.
    if player
        .single()
        .is_ok_and(|p| block_overlaps_player(world, target, held, rotation, p.feet()))
    {
        return;
    }
    if edit.set(target, held) {
        if rotation != Rotation::IDENTITY {
            edit.game.world.set_rotation(target, rotation);
        }
        edit.inventory.as_deref_mut().map(Inventory::take_selected);
        let ctx = context(edit.game, holding, now, target);
        let actions = rules::actions(&edit.game.world.registry, held, Hook::Placed, ctx);
        edit.apply(target, &actions);
    }
}

/// Ce que les conditions d'une règle peuvent lire, pour le bloc `voxel` au
/// tick `tick`.
fn context(game: &GameWorld, holding: Option<ContentId>, tick: u64, voxel: [i64; 3]) -> Context {
    Context {
        holding,
        seed: rules::event_seed(game.generator.terrain.seed, tick, voxel),
        above: game.world.voxel([voxel[0], voxel[1] + 1, voxel[2]]),
        neighbors: neighbors(voxel).map(|n| game.world.voxel(n)),
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
    /// `None` hors d'une action du joueur (tick) : `StoreHeld` ne fait rien.
    inventory: Option<&'a mut Inventory>,
    /// Tick depuis lequel comptent les minuteurs d'un bloc posé : maintenant,
    /// ou l'échéance du minuteur qui l'a posé (rattrapage, §3.6).
    from: u64,
    /// Étapes de retard déjà rattrapées par cette chaîne de minuteurs.
    late: u8,
}

impl Edit<'_, '_, '_> {
    /// Écrit un voxel et note les chunks à re-mesher. `false` si le chunk
    /// n'est pas chargé (rien n'a changé).
    fn set(&mut self, voxel: [i64; 3], id: ContentId) -> bool {
        // Le contenu posé sur l'ancien bloc tombe au sol (§3.3 : l'état
        // disparaît avec le bloc).
        let stored = self.game.world.take_stored(voxel);
        let Some(pos) = self.game.world.set_voxel(voxel, id) else { return false };
        self.game.world.start_timers(voxel, self.from, self.late);
        for item in stored {
            self.drop(voxel, item);
        }
        self.storage.0.insert(voxel);
        // Un voxel en bordure change aussi les chunks voisins : leurs faces
        // contre lui (culling inter-chunks) et l'ombre de leurs coins, y
        // compris en diagonale (arête ou coin du chunk).
        debug_assert_eq!(pos, self.game.world.split(voxel).0);
        self.dirty.0.extend(meshes_reading(voxel, self.game.world.chunk_size()));
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
                    if let Some(inventory) = self.inventory.as_deref_mut()
                        && let Some(held) = inventory.selected()
                        && self.game.world.store(voxel, held)
                    {
                        inventory.take_selected();
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

/// Le bloc `id` posé en `voxel`, tourné de `rotation`, arrêterait-il le
/// joueur ? Seules ses boîtes de collision comptent : une torche se pose
/// à ses pieds.
fn block_overlaps_player(
    world: &VoxelWorld,
    voxel: [i64; 3],
    id: ContentId,
    rotation: Rotation,
    player_feet: Vec3,
) -> bool {
    let vpm = world.voxels_per_meter();
    let boxes = world.registry.get(id).and_then(|e| e.block()).and_then(|b| b.collision_boxes()).unwrap_or(&[Cuboid::FULL]);
    let p = Aabb::from_feet(player_feet.to_array(), PLAYER_WIDTH_M, PLAYER_HEIGHT_M);
    boxes.iter().any(|c| {
        let [lo, hi] = c.rotated(rotation).to_cells().map(|q| std::array::from_fn::<f32, 3, _>(|a| (voxel[a] as f32 + q[a]) / vpm));
        (0..3).all(|a| lo[a] < p.max[a] && hi[a] > p.min[a])
    })
}

/// Blocs qui évoluent seuls (#34), au tick fixe : tick aléatoire (`Tick`)
/// dans les chunks affichés, puis minuteurs échus (`After`) et réexamens
/// échus (liquides, `NeighborChanged`, #28) partout où le chunk est en
/// mémoire.
pub fn tick_blocks(
    mut commands: Commands,
    mut game: ResMut<GameWorld>,
    mut dirty: ResMut<DirtyChunks>,
    mut storage: ResMut<StorageChanged>,
    chunks: Query<&ChunkMesh>,
) {
    game.world.advance();
    let now = game.world.tick();
    let world_seed = game.generator.terrain.seed;
    // Assez de tirages par chunk pour qu'un bloc en reçoive un toutes les
    // TICK_PERIOD_S secondes en moyenne.
    let volume = f64::from(game.world.chunk_size()).powi(3);
    let picks = (volume / (TICK_PERIOD_S * f64::from(TICKS_PER_SECOND))).round().max(1.0) as u32;
    let displayed: Vec<_> = chunks.iter().map(|c| c.0).collect();
    let picked = ticks::pick(&game.world, &displayed, picks, rules::event_seed(world_seed, now, [0; 3]));
    let due = game.world.take_due_timers();

    // Un minuteur peut changer un chunk hors de vue : on ne re-meshe que les
    // chunks affichés (sinon `remesh_dirty` les ferait apparaître) ; les
    // autres le seront quand le streaming les raffichera.
    let mut changed = DirtyChunks::default();
    let mut edit = Edit {
        commands: &mut commands,
        game: &mut game,
        dirty: &mut changed,
        storage: &mut storage,
        inventory: None,
        from: now,
        late: 0,
    };
    for voxel in picked {
        // Un tick précédent de ce même tour a pu changer le voxel.
        let Some(block) = edit.game.world.voxel(voxel) else { continue };
        let ctx = context(edit.game, None, now, voxel);
        let actions = rules::actions(&edit.game.world.registry, block, Hook::Tick, ctx);
        edit.apply(voxel, &actions);
    }
    for (voxel, timer) in due {
        let Some(block) = edit.game.world.voxel(voxel) else { continue };
        let ctx = context(edit.game, None, timer.due, voxel);
        let actions = rules::rule_actions(&edit.game.world.registry, block, timer.rule as usize, ctx);
        // Le bloc suivant compte depuis l'échéance, pas depuis maintenant :
        // une chaîne en retard (chunk relu de la save) rattrape.
        edit.from = timer.due;
        edit.late = timer.late.saturating_add(u8::from(timer.due < now));
        edit.apply(voxel, &actions);
    }
    // Réexamens échus (#28) : le liquide suit ses voisins, puis le bloc
    // réagit à son voisinage s'il a une règle `NeighborChanged`.
    (edit.from, edit.late) = (now, 0);
    let air = edit.game.air;
    for voxel in edit.game.world.take_due_updates(MAX_UPDATES_PER_TICK) {
        if let Some((id, level)) = liquids::update(&mut edit.game.world, air, voxel) {
            edit.set(voxel, id);
            edit.game.world.set_liquid_level(voxel, level);
        }
        let Some(block) = edit.game.world.voxel(voxel) else { continue };
        if rules::has_rule(&edit.game.world.registry, block, Hook::NeighborChanged) {
            let ctx = context(edit.game, None, now, voxel);
            let actions = rules::actions(&edit.game.world.registry, block, Hook::NeighborChanged, ctx);
            edit.apply(voxel, &actions);
        }
    }
    dirty.0.extend(changed.0.into_iter().filter(|p| displayed.contains(p)));
}
