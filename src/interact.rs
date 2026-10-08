//! Pose / casse de voxels — critère §7.3 de la slice.
//!
//! Clic gauche : casser le voxel visé, qui laisse tomber ses drops
//! (`Registry::drops`) en items au sol. Clic droit : poser le bloc en main
//! (pris dans l'[`Inventory`]) sur la face visée. Tout est **data-driven** :
//! des `ContentId` du registre, jamais un type en dur — le système ne sait
//! pas ce qu'il casse ni ce qu'il pose.
//!
//! Après une écriture, le chunk touché est noté à re-mesher intégralement
//! ([`DirtyChunks`]). C'est brut (on reconstruit 32³ voxels pour un
//! changement d'un seul) mais largement assez rapide — et c'est le *même*
//! chemin de meshing que le streaming (`remesh_dirty`, main.rs).

use bevy::prelude::*;

use voxel_core::chunk::ChunkPos;
use voxel_core::physics::Aabb;
use voxel_core::raycast::{raycast, RayHit};
use voxel_core::world::VoxelWorld;

use crate::player::{CursorCaptured, Player, PLAYER_HEIGHT_M, PLAYER_WIDTH_M, PlayerCamera};
use crate::inventory::Inventory;
use crate::items::DroppedItem;
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

// Un système Bevy prend ses ressources en paramètres : 8 ici (entrée,
// monde, inventaire, caméra, joueur…), c'est la forme normale.
#[allow(clippy::too_many_arguments)]
pub fn interact(
    mut commands: Commands,
    mut inventory: ResMut<Inventory>,
    mouse: Res<ButtonInput<MouseButton>>,
    captured: Res<CursorCaptured>,
    mut game: ResMut<GameWorld>,
    mut dirty: ResMut<DirtyChunks>,
    camera: Query<&GlobalTransform, With<PlayerCamera>>,
    player: Query<&Player>,
) {
    // On n'interagit qu'en mode FPS — et comme ce système tourne AVANT
    // `cursor_grab` (voir l'ordre du plugin), le clic qui active le mode
    // ne casse pas de bloc au passage.
    let breaking = mouse.just_pressed(MouseButton::Left);
    let placing = mouse.just_pressed(MouseButton::Right);
    if !captured.0 || (!breaking && !placing) {
        return;
    }
    let Ok(cam) = camera.single() else { return };

    let Some(hit) = aim(&game.world, cam) else { return };

    let edited: [i64; 3];
    let touched: Option<ChunkPos> = if breaking {
        let air = game.air;
        edited = hit.voxel;
        let broken = game.world.voxel(hit.voxel);
        let touched = game.world.set_voxel(hit.voxel, air);
        if let (Some(broken), Some(_)) = (broken, touched) {
            let size_m = 1.0 / game.world.voxels_per_meter();
            let center = (Vec3::from_array(hit.voxel.map(|v| v as f32)) + 0.5) * size_m;
            for drop in game.world.registry.drops(broken) {
                commands.spawn((DroppedItem::new(drop, center), Transform::from_translation(center)));
            }
        }
        touched
    } else {
        // Seul un bloc se pose : un item en main (sans `places`, pas encore
        // implémenté) ne fait rien.
        let Some(held) = inventory.selected().filter(|&id| {
            game.world.registry.get(id).is_some_and(|e| e.block().is_some())
        }) else {
            return;
        };
        // Poser : sur la face d'entrée du rayon. Normale nulle = l'œil est
        // dans un solide, pas de face → rien.
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
            .is_ok_and(|p| voxel_overlaps_player(&game.world, target, p.feet()))
        {
            return;
        }
        edited = target;
        let touched = game.world.set_voxel(target, held);
        if touched.is_some() {
            inventory.take_selected();
        }
        touched
    };

    if let Some(pos) = touched {
        dirty.0.insert(pos);
        // Culling inter-chunks : un voxel en bordure change aussi les faces
        // du chunk voisin (sa face culled peut devoir (ré)apparaître).
        let size = game.world.chunk_size();
        let (_, local) = game.world.split(edited);
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
            dirty.0.insert(npos);
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
