//! Pose / casse de voxels — critère §7.3 de la slice.
//!
//! Clic gauche : casser le voxel visé. Clic droit : poser le bloc « en
//! main » sur la face visée. Le bloc posé est **data-driven** : c'est un
//! `ContentId` du registre (`GameWorld::held`), jamais un type en dur — le
//! système ne sait pas ce qu'il pose.
//!
//! Après une écriture, le chunk touché est re-meshé intégralement. C'est
//! brut (on reconstruit 32³ voxels pour un changement d'un seul) mais
//! largement assez rapide pour la slice — et c'est le *même* chemin de
//! meshing que la génération : un seul code à faire évoluer vers le greedy.

use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;

use voxel_core::chunk::ChunkPos;
use voxel_core::physics::Aabb;
use voxel_core::raycast::raycast;

use crate::player::{CursorCaptured, Player, PLAYER_HEIGHT_M, PLAYER_WIDTH_M, PlayerCamera};
use crate::{held_label, remesh_chunk, ChunkMesh, GameWorld, HeldBlockText};

/// Molette : fait défiler la hotbar (cyclique). La hotbar est découverte
/// depuis le registre au setup — ce système ne connaît aucun bloc, il ne
/// fait que déplacer un index.
pub fn select_held_block(
    mut wheel: MessageReader<MouseWheel>,
    captured: Res<CursorCaptured>,
    mut game: ResMut<GameWorld>,
    mut hud: Query<&mut Text, With<HeldBlockText>>,
) {
    // Somme des crans de la frame (trackpads : plusieurs petits événements).
    let scroll: f32 = wheel.read().map(|w| w.y).sum();
    if !captured.0 || scroll == 0.0 {
        return;
    }
    let n = game.hotbar.len();
    // rem_euclid : modulo toujours positif, même en reculant depuis 0.
    let step = if scroll > 0.0 { 1 } else { n - 1 };
    game.held_idx = (game.held_idx + step).rem_euclid(n);

    if let Ok(mut text) = hud.single_mut() {
        text.0 = held_label(&game.world.registry, game.held());
    }
}

/// Portée de la main, en mètres (§2 — jamais « en blocs »).
const REACH_M: f32 = 5.0;

// Les systèmes ECS prennent leurs dépendances en paramètres : 8 arguments
// est normal ici, pas un smell de design.
#[allow(clippy::too_many_arguments)]
pub fn interact(
    mut commands: Commands,
    mouse: Res<ButtonInput<MouseButton>>,
    captured: Res<CursorCaptured>,
    mut game: ResMut<GameWorld>,
    camera: Query<&GlobalTransform, With<PlayerCamera>>,
    player: Query<&Transform, With<Player>>,
    mut meshes: ResMut<Assets<Mesh>>,
    chunk_meshes: Query<(Entity, &ChunkMesh, &Mesh3d)>,
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

    let Some(hit) = raycast(
        &game.world,
        cam.translation().to_array(),
        cam.forward().as_vec3().to_array(),
        REACH_M,
    ) else {
        return;
    };

    let edited: [i64; 3];
    let touched: Option<ChunkPos> = if breaking {
        let air = game.air;
        edited = hit.voxel;
        game.world.set_voxel(hit.voxel, air)
    } else {
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
            .is_ok_and(|t| voxel_overlaps_player(&game.world, target, t.translation))
        {
            return;
        }
        let held = game.held();
        edited = target;
        game.world.set_voxel(target, held)
    };

    if let Some(pos) = touched {
        remesh_chunk(&mut commands, &game, pos, &mut meshes, &chunk_meshes);
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
            remesh_chunk(&mut commands, &game, npos, &mut meshes, &chunk_meshes);
        }
    }
}

fn voxel_overlaps_player(
    world: &voxel_core::world::VoxelWorld,
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

// (remesh_chunk vit dans main.rs : partagé entre pose/casse et streaming.)
