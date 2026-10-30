//! Mobs (#15, #32, design doc §3.1 et §3.6) : le modèle d'une entrée
//! `EntityType`, dessiné partie par partie, éclairé par la grille à sa
//! position ; simulé au tick fixe par `voxel_core::mob` (comportements,
//! chemin, physique du joueur), affiché interpolé et animé.

use bevy::mesh::MeshTag;
use bevy::prelude::*;

use voxel_core::{mob, spawn};
use voxel_core::model::Motion;
use voxel_core::registry::ContentId;

use crate::inventory::Inventory;
use crate::player::{Player, EYE_HEIGHT_M};
use crate::sky::WorldSky;
use crate::{to_bevy_mesh, GameWorld};

/// Un mob : son état simulé (`voxel_core`). Ses parties sont ses enfants.
#[derive(Component, Deref, DerefMut)]
#[require(Transform, Visibility)]
pub struct Mob(pub mob::Mob);

/// Les entités Bevy de ses parties, dans l'ordre du modèle.
#[derive(Component)]
pub struct Parts(Vec<Entity>);

/// Ids des mobs : le prochain à donner, et le dernier servi par la
/// recherche de chemin (tour à tour, voir `mob::step_all`).
#[derive(Resource, Default)]
pub struct MobIds {
    pub next: u64,
    turn: u64,
}

/// Fait naître un mob, avec un id neuf. `spawned` : apparu seul (il
/// disparaîtra loin du joueur), pas posé.
pub fn spawn_mob(commands: &mut Commands, content: ContentId, feet: Vec3, yaw: f32, spawned: bool) {
    commands.queue(move |world: &mut World| {
        let mut ids = world.resource_mut::<MobIds>();
        let id = ids.next;
        ids.next += 1;
        let mut mob = mob::Mob::new(id, content, feet.to_array(), yaw);
        mob.spawned = spawned;
        world.spawn(Mob(mob));
    });
}

/// Un mesh éclairé par la grille à sa position (ciel et blocs, §3.8) plutôt
/// que comme un chunk. La lumière passe au shader par `MeshTag`.
#[derive(Component)]
#[require(MeshTag)]
pub struct GridLit;

/// Habille une entité fraîchement posée : une entité Bevy par partie, au
/// pivot de la partie, enfant de sa partie parente (ou de l'entité).
// shortcut: un mesh par partie et par entité posée, aucun partage entre
// entités du même type ; mettre en cache par (entrée, partie) si des
// centaines de mobs se voient au profilage.
pub fn add_mob_models(
    mut commands: Commands,
    game: Res<GameWorld>,
    mut meshes: ResMut<Assets<Mesh>>,
    mobs: Query<(Entity, &Mob), Added<Mob>>,
) {
    let registry = &game.world.registry;
    for (entity, mob) in &mobs {
        let at = Transform::from_translation(Vec3::from_array(mob.body.feet)).with_rotation(Quat::from_rotation_y(mob.yaw));
        commands.entity(entity).insert(at);
        let Some(data) = registry.get(mob.content).and_then(|e| e.entity()) else { continue };
        // Entité Bevy et pivot (mètres) de chaque partie déjà posée.
        let mut placed: Vec<(Entity, Vec3)> = Vec::with_capacity(data.model.len());
        for part in &data.model {
            let pivot = Vec3::from_array(part.pivot_m());
            let (parent, origin) = part
                .parent
                .as_ref()
                .and_then(|name| data.model.iter().position(|p| &p.name == name))
                .map_or((entity, Vec3::ZERO), |i| placed[i]);
            let child = commands
                .spawn((
                    Mesh3d(meshes.add(to_bevy_mesh(part.mesh(registry)))),
                    MeshMaterial3d(game.material.clone()),
                    GridLit,
                    Transform::from_translation(pivot - origin),
                    ChildOf(parent),
                ))
                .id();
            placed.push((child, pivot));
        }
        commands.entity(entity).insert(Parts(placed.into_iter().map(|(e, _)| e).collect()));
    }
}

/// Un tick de tous les mobs, dans l'ordre de leur id (§2) ; puis ceux
/// qui disparaissent et ceux qui apparaissent (#60).
#[allow(clippy::too_many_arguments)]
pub fn simulate_mobs(
    mut commands: Commands,
    time: Res<Time>,
    game: Res<GameWorld>,
    sky: Option<Res<WorldSky>>,
    inventory: Res<Inventory>,
    mut ids: ResMut<MobIds>,
    player: Query<&Player>,
    mut mobs: Query<(Entity, &mut Mob)>,
) {
    let player = player.single().ok().map(|p| {
        let feet = p.feet();
        mob::Player {
            feet: feet.to_array(),
            eyes: (feet + Vec3::Y * EYE_HEIGHT_M).to_array(),
            holding: inventory.selected(),
        }
    });
    let mut list: Vec<_> = mobs.iter_mut().map(|(e, m)| (e, m.into_inner())).collect();
    list.sort_by_key(|(_, m)| m.id);
    let (entities, mut list): (Vec<Entity>, Vec<&mut mob::Mob>) = list.into_iter().map(|(e, m)| (e, &mut m.0)).unzip();
    let seed = game.generator.terrain.seed;
    let world = &game.world;
    mob::step_all(world, &mut list, player.as_ref(), seed, time.delta_secs(), &mut ids.turn);
    let Some(player) = player else { return };
    for (entity, mob) in entities.iter().zip(&list) {
        if spawn::gone(world, mob, player.feet) {
            commands.entity(*entity).despawn();
        }
    }
    let daylight = sky.map_or(1.0, |s| s.0.daylight(&s.0.at(s.0.hour(world.tick()))));
    let all: Vec<&mob::Mob> = list.iter().map(|m| &**m).collect();
    for born in spawn::spawn(world, &all, player.feet, seed, daylight) {
        spawn_mob(&mut commands, born.content, Vec3::from_array(born.feet), born.yaw, true);
    }
}

/// Position interpolée entre les deux derniers ticks (comme le joueur),
/// cap, et mouvements des parties.
pub fn show_mobs(
    fixed: Res<Time<Fixed>>,
    game: Res<GameWorld>,
    mobs: Query<(Entity, &Mob, &Parts)>,
    mut transforms: Query<&mut Transform>,
) {
    let t = fixed.overstep_fraction();
    for (entity, mob, parts) in &mobs {
        let feet = Vec3::from_array(mob.prev_feet).lerp(Vec3::from_array(mob.body.feet), t);
        let body = Quat::from_rotation_y(mob.yaw);
        if let Ok(mut transform) = transforms.get_mut(entity) {
            transform.translation = feet;
            transform.rotation = body;
        }
        let Some(data) = game.world.registry.get(mob.content).and_then(|e| e.entity()) else { continue };
        for motion in &data.animation {
            let Some(i) = data.model.iter().position(|p| p.name == motion.part()) else { continue };
            let Ok(mut transform) = transforms.get_mut(parts.0[i]) else { continue };
            transform.rotation = match motion {
                Motion::Swing { angle, stride, phase, .. } => {
                    let wave = (std::f32::consts::TAU * (mob.walked / stride + phase)).sin();
                    Quat::from_rotation_x(angle.to_radians() * mob.swing * wave)
                }
                Motion::Look { max, .. } => {
                    let Some(target) = mob.look_at else {
                        transform.rotation = Quat::IDENTITY;
                        continue;
                    };
                    // La cible dans le repère du corps, depuis le pivot de la partie.
                    let pivot = feet + body * Vec3::from_array(data.model[i].pivot_m());
                    let local = body.inverse() * (Vec3::from_array(target) - pivot);
                    let max = max.to_radians();
                    let yaw = local.x.atan2(local.z).clamp(-max, max);
                    let pitch = (-local.y).atan2(local.x.hypot(local.z)).clamp(-max, max);
                    Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch)
                }
            };
        }
    }
}

/// Lumière de la grille sous chaque mesh [`GridLit`], codée pour le
/// shader : ciel sur 8 bits, blocs sur les 8 suivants, et le bit 16 pour
/// dire « entité » (0 : un chunk, sa lumière est aux sommets).
pub fn light_grid_lit(game: Res<GameWorld>, mut lit: Query<(&GlobalTransform, &mut MeshTag), With<GridLit>>) {
    let world = &game.world;
    let full = u32::from(world.full_light().max(1));
    for (transform, mut tag) in &mut lit {
        let [sky, block] = world.light_at(world.voxel_at_m(transform.translation().to_array())).map(u32::from);
        let code = 1 << 16 | (sky * 255 / full) | (block * 255 / full) << 8;
        // Comparer avant d'écrire : écrire marque le composant changé.
        if tag.0 != code {
            tag.0 = code;
        }
    }
}
