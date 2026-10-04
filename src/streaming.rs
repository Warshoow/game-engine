//! Streaming de chunks — charge/décharge autour du joueur (§3.4).
//!
//! Le doc est explicite : « streaming identique à l'infini » — on charge et
//! décharge les chunks autour du joueur comme pour un monde sans bornes,
//! les limites du monde ne seront qu'un check en plus. Concrètement, chaque
//! frame :
//!
//! 1. **Charger** : les chunks dans le rayon de vue qui n'ont pas de données
//!    sont générés (du plus proche au plus loin), sous un **budget par
//!    frame** — le coût se lisse sur plusieurs frames au lieu d'un gros
//!    hitch quand on franchit une frontière de chunk.
//! 2. **Mesher** : les chunks générés ET leurs voisins déjà affichés sont
//!    (re)meshés — le culling inter-chunks fait que la bordure d'un chunk
//!    dépend de ses voisins ; quand un voisin apparaît, la couture doit se
//!    refermer. Le meshing est différé en fin de passe, dédupliqué : les
//!    entités spawnées via `Commands` ne sont visibles dans la `Query` qu'à
//!    la frame suivante, re-mesher au fil de l'eau dupliquerait des meshes.
//! 3. **Décharger** : les entités-mesh au-delà du rayon + une marge
//!    d'hystérésis sont despawnées (et leur asset GPU libéré). Les
//!    **données** du chunk restent en mémoire : les modifications du joueur
//!    survivent à l'aller-retour — la persistance *disque* est un non-goal
//!    (§7), la persistance *mémoire* est juste du bon sens.
//!
//! La marge d'hystérésis évite le charge/décharge en boucle quand le joueur
//! oscille autour d'une frontière : on charge à `VIEW_DISTANCE_M`, on ne
//! décharge qu'au-delà de `VIEW_DISTANCE_M + UNLOAD_MARGIN_M`.

use std::collections::HashSet;

use bevy::prelude::*;

use voxel_core::chunk::ChunkPos;
use voxel_core::worldgen::WorldGenerator;

use crate::player::Player;
use crate::{remesh_chunk, ChunkMesh, GameWorld};

/// Rayon de vue, en **mètres** (§2 : le gameplay ne parle jamais « en
/// chunks » — la conversion se fait ici et nulle part ailleurs).
const VIEW_DISTANCE_M: f32 = 96.0;
/// Hystérésis de déchargement, en mètres.
const UNLOAD_MARGIN_M: f32 = 32.0;
/// Chunks générés/meshés par frame — lisse le coût du streaming.
const GEN_BUDGET_PER_FRAME: usize = 4;

pub fn stream_chunks(
    mut commands: Commands,
    mut game: ResMut<GameWorld>,
    player: Query<&Transform, With<Player>>,
    mut meshes: ResMut<Assets<Mesh>>,
    chunk_meshes: Query<(Entity, &ChunkMesh, &Mesh3d)>,
) {
    let Ok(player) = player.single() else { return };
    let p = player.translation;
    let extent_m = game.world.chunk_size() as f32 / game.world.voxels_per_meter();

    // Chunks ayant actuellement une entité-mesh (affichés).
    let displayed: HashSet<ChunkPos> = chunk_meshes.iter().map(|(_, cm, _)| cm.0).collect();

    // Distance horizontale (mètres) du joueur au centre d'une colonne de chunk.
    let dist_m = |pos: ChunkPos| -> f32 {
        let cx = (pos.x as f32 + 0.5) * extent_m;
        let cz = (pos.z as f32 + 0.5) * extent_m;
        ((cx - p.x).powi(2) + (cz - p.z).powi(2)).sqrt()
    };

    // --- 1. L'ensemble voulu : un disque de chunks autour du joueur. ---
    // Une seule couche verticale (y = 0) : le terrain de la heightmap tient
    // dans [0, 32) voxels de haut. La verticalité (caves, y < 0) élargira
    // cette boucle, pas la logique.
    let (pcx, pcz) = (
        (p.x / extent_m).floor() as i32,
        (p.z / extent_m).floor() as i32,
    );
    let radius_chunks = (VIEW_DISTANCE_M / extent_m).ceil() as i32;
    let mut desired: Vec<ChunkPos> = Vec::new();
    for dz in -radius_chunks..=radius_chunks {
        for dx in -radius_chunks..=radius_chunks {
            let pos = ChunkPos { x: pcx + dx, y: 0, z: pcz + dz };
            if dist_m(pos) <= VIEW_DISTANCE_M {
                desired.push(pos);
            }
        }
    }
    // Du plus proche au plus loin : le sol sous les pieds arrive en premier.
    desired.sort_by(|a, b| dist_m(*a).total_cmp(&dist_m(*b)));

    // --- 2. Générer (budget), en notant tout ce qui devra être meshé. ---
    let mut need_mesh: Vec<ChunkPos> = Vec::new();
    let mut budget = GEN_BUDGET_PER_FRAME;
    for pos in desired {
        if budget == 0 {
            break;
        }
        let has_data = game.world.chunk(pos).is_some();
        if has_data && displayed.contains(&pos) {
            continue; // déjà chargé et affiché : rien à faire
        }
        if !has_data {
            let chunk = game.generator.generate_chunk(
                pos,
                game.world.chunk_size(),
                game.world.voxels_per_meter(),
            );
            game.world.insert_chunk(pos, chunk);
            // La bordure des voisins déjà affichés doit se recoudre contre
            // ce nouveau chunk (culling inter-chunks).
            for n in neighbors(pos) {
                if displayed.contains(&n) {
                    need_mesh.push(n);
                }
            }
        }
        // Chunk fraîchement généré, ou données conservées d'un passage
        // précédent (édits du joueur inclus) qui revient dans le rayon.
        need_mesh.push(pos);
        budget -= 1;
    }

    // --- 3. Mesher, une seule fois par chunk. ---
    let mut seen = HashSet::new();
    for pos in need_mesh {
        if seen.insert(pos) {
            remesh_chunk(&mut commands, &game, pos, &mut meshes, &chunk_meshes);
        }
    }

    // --- 4. Décharger les meshes trop loin (les données restent). ---
    for (entity, cm, mesh3d) in &chunk_meshes {
        if dist_m(cm.0) > VIEW_DISTANCE_M + UNLOAD_MARGIN_M {
            meshes.remove(mesh3d.id());
            commands.entity(entity).despawn();
        }
    }
}

/// Les 6 chunks adjacents (les diagonales ne partagent pas de face).
fn neighbors(pos: ChunkPos) -> [ChunkPos; 6] {
    let ChunkPos { x, y, z } = pos;
    [
        ChunkPos { x: x + 1, y, z },
        ChunkPos { x: x - 1, y, z },
        ChunkPos { x, y: y + 1, z },
        ChunkPos { x, y: y - 1, z },
        ChunkPos { x, y, z: z + 1 },
        ChunkPos { x, y, z: z - 1 },
    ]
}
