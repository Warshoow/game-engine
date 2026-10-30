//! Entités posées (#15, design doc §3.1) : le modèle d'une entrée
//! `EntityType`, dessiné partie par partie, éclairé par la grille à sa
//! position. Elles ne bougent pas encore : déplacement, IA et animation
//! viennent avec #32.

use bevy::mesh::MeshTag;
use bevy::prelude::*;

use voxel_core::registry::ContentId;

use crate::{to_bevy_mesh, GameWorld};

/// Une entité posée : son entrée, ses pieds (mètres) et son cap (radians
/// autour de +Y, 0 : regarde vers +Z). Ses parties sont ses enfants.
#[derive(Component)]
#[require(Transform, Visibility)]
pub struct Mob {
    pub content: ContentId,
    pub feet: Vec3,
    pub yaw: f32,
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
        commands
            .entity(entity)
            .insert(Transform::from_translation(mob.feet).with_rotation(Quat::from_rotation_y(mob.yaw)));
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
