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
//! 2. **Noter à mesher** ([`DirtyChunks`]) : les chunks générés ET leurs
//!    voisins déjà affichés — le culling inter-chunks fait que la bordure
//!    d'un chunk dépend de ses voisins ; quand un voisin apparaît, la couture
//!    doit se refermer. Le meshing lui-même est fait après, par
//!    `remesh_dirty` (main.rs), une fois par chunk.
//! 3. **Décharger** : les entités-chunk au-delà du rayon + une marge
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
use crate::{ChunkMesh, DirtyChunks, GameWorld};

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
    mut dirty: ResMut<DirtyChunks>,
    player: Query<&Transform, With<Player>>,
    mut meshes: ResMut<Assets<Mesh>>,
    chunk_meshes: Query<(Entity, &ChunkMesh, Option<&Mesh3d>)>,
) {
    let Ok(player) = player.single() else { return };
    let p = player.translation;
    let extent_m = game.world.chunk_size() as f32 / game.world.voxels_per_meter();

    // Chunks ayant une entité (affichés — y compris ceux au mesh vide).
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
                    dirty.0.insert(n);
                }
            }
        }
        // Chunk fraîchement généré, ou données conservées d'un passage
        // précédent (édits du joueur inclus) qui revient dans le rayon.
        dirty.0.insert(pos);
        budget -= 1;
    }

    // --- 3. Décharger les chunks trop loin (les données restent). ---
    for (entity, cm, mesh3d) in &chunk_meshes {
        if dist_m(cm.0) > VIEW_DISTANCE_M + UNLOAD_MARGIN_M {
            if let Some(mesh3d) = mesh3d {
                meshes.remove(mesh3d.id());
            }
            commands.entity(entity).despawn();
            // Plus d'entité : ne pas le re-mesher (ça la recréerait).
            dirty.0.remove(&cm.0);
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

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::*;
    use voxel_core::chunk::CHUNK_SIZE;
    use voxel_core::registry::{ContentEntry, Registry};
    use voxel_core::world::VoxelWorld;
    use voxel_core::worldgen::HeightmapGenerator;

    use super::*;
    use crate::remesh_dirty;

    /// Monde **tout en air** (sol à −100 m) : chaque chunk a un mesh vide.
    /// Régression : un chunk vide n'avait pas d'entité, passait pour jamais
    /// chargé, et était re-meshé à chaque frame en mangeant le budget.
    #[test]
    fn empty_chunks_load_once_and_stay_loaded() {
        let mut registry = Registry::new();
        let air = registry
            .register(ContentEntry::new_block("core:air", false, [0.0; 3]))
            .unwrap();
        let generator = HeightmapGenerator {
            seed: 1,
            air,
            ground: air,
            ground_level_m: -100.0,
            amplitude_m: 1.0,
            feature_size_m: 24.0,
        };
        let mut app = App::new();
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<DirtyChunks>()
            .insert_resource(GameWorld {
                world: VoxelWorld::new(registry, CHUNK_SIZE, 1.0),
                generator,
                air,
                hotbar: vec![air],
                held_idx: 0,
                material: Handle::default(),
            })
            .add_systems(Update, (stream_chunks, remesh_dirty).chain());
        app.world_mut().spawn((
            Player::default(),
            Transform::from_xyz(0.5, 0.0, 0.5),
        ));

        // Assez de frames pour vider le disque au budget de 4/frame.
        for _ in 0..30 {
            app.update();
        }
        let mut q = app.world_mut().query::<&ChunkMesh>();
        let loaded: Vec<ChunkPos> = q.iter(app.world()).map(|c| c.0).collect();
        let unique: HashSet<ChunkPos> = loaded.iter().copied().collect();
        assert!(!loaded.is_empty());
        assert_eq!(loaded.len(), unique.len(), "entité-chunk en double");

        // Une fois tout chargé, le streaming ne doit plus rien demander.
        app.world_mut().run_system_once(stream_chunks).unwrap();
        assert!(app.world().resource::<DirtyChunks>().0.is_empty());
    }
}
