//! Streaming de chunks — charge/décharge autour du joueur (§3.4).
//!
//! Le doc est explicite : « streaming identique à l'infini » — on charge et
//! décharge les chunks autour du joueur comme pour un monde sans bornes,
//! les limites du monde ne seront qu'un check en plus. Concrètement, chaque
//! frame :
//!
//! 1. **Charger** : les chunks dans le rayon de vue qui n'ont pas de données
//!    sont lus dans la save ou générés (du plus proche au plus loin) dans
//!    des **tâches** en parallèle ([`Generating`], #53). L'image n'en insère
//!    que quelques-uns dans le monde : leur lumière se calcule là.
//! 2. **Mesher** ([`DirtyChunks`]) : un chunk l'est quand ses voisins de la
//!    zone ont leurs données — la bordure d'un chunk dépend de ses voisins
//!    (culling inter-chunks, ombre des coins). Il est ainsi meshé une fois
//!    au chargement, et non une fois par voisin arrivé après lui ; un chunk
//!    affiché dont un voisin arrive plus tard est re-meshé une fois de plus.
//!    Le meshing lui-même tourne dans des tâches (`remesh_dirty`, main.rs).
//! 3. **Décharger** : les entités-chunk au-delà du rayon + une marge
//!    d'hystérésis sont despawnées (et leur asset GPU libéré). Les
//!    **données** du chunk restent en mémoire : les modifications du joueur
//!    survivent à l'aller-retour. Sur disque, un chunk modifié est écrit
//!    dès qu'il change (`save.rs`) ; au chargement, le fichier d'un chunk
//!    passe avant la génération.
//!
//! La marge d'hystérésis évite le charge/décharge en boucle quand le joueur
//! oscille autour d'une frontière : on charge à `VIEW_DISTANCE_M`, on ne
//! décharge qu'au-delà de `VIEW_DISTANCE_M + UNLOAD_MARGIN_M`.
//!
//! La zone chargée est un **cylindre** : un disque de `VIEW_DISTANCE_M` à
//! l'horizontale, `VERTICAL_VIEW_M` au-dessus et en dessous du joueur — on
//! voit loin à l'horizon, mais charger 96 m de roche sous les pieds ne
//! servirait à rien. Le monde est borné en hauteur (§3.4) : aucune couche
//! hors de `[WORLD_MIN_Y_M, WORLD_MAX_Y_M)` n'est générée.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use bevy::tasks::{block_on, AsyncComputeTaskPool, Task};

use voxel_core::chunk::{Chunk, ChunkPos};
use voxel_core::world::ChunkState;
use voxel_core::worldgen::WorldGenerator;

use crate::items::StorageChanged;
use crate::player::Player;
use crate::save::WorldSave;
use crate::{ChunkMesh, DirtyChunks, GameWorld, Meshing};

/// Rayon de vue, en **mètres** (§2 : le gameplay ne parle jamais « en
/// chunks » — la conversion se fait ici et nulle part ailleurs).
pub const VIEW_DISTANCE_M: f32 = 96.0;
/// Hystérésis de déchargement, en mètres.
const UNLOAD_MARGIN_M: f32 = 32.0;
// Les mobs gèlent là où l'affichage des chunks se décharge : aucun mob figé en vue.
const _: () = assert!(VIEW_DISTANCE_M + UNLOAD_MARGIN_M == voxel_core::mob::SIMULATION_M);
const _: () = assert!(VIEW_DISTANCE_M == voxel_core::spawn::SPAWN_MAX_M);
/// Demi-hauteur de la zone chargée, en mètres (distance verticale max entre
/// le joueur et le centre d'un chunk).
pub const VERTICAL_VIEW_M: f32 = 48.0;
/// Bornes verticales du monde, en mètres (§3.4 : −128 à 384).
const WORLD_MIN_Y_M: f32 = -128.0;
const WORLD_MAX_Y_M: f32 = 384.0;
/// Générations en cours au plus, dans des tâches (#53).
const GEN_TASKS: usize = 8;
/// Chunks insérés dans le monde par image, au plus une colonne près : leur
/// lumière (§3.8) se calcule dans l'image, 1 à 3,5 ms chacun.
const INSERT_BUDGET_PER_FRAME: usize = 1;
/// Meshings lancés par image : la copie du chunk se fait dans l'image,
/// 0,5 à 2 ms chacune (plus en course : les données ne sont plus en cache).
const MESH_BUDGET_PER_FRAME: usize = 2;

/// Une colonne chargée par une tâche : le chunk demandé, puis ceux au-dessus
/// qui pourraient le couvrir (§3.8), du bas vers le haut ; avec leur état
/// s'ils viennent de la save.
type Column = Vec<(ChunkPos, Chunk, Option<ChunkState>)>;

/// Colonnes en cours de chargement dans des tâches (#53), par chunk
/// demandé : le chunk où la colonne s'arrête, et la tâche.
#[derive(Resource, Default)]
pub struct Generating(HashMap<ChunkPos, (ChunkPos, Task<Result<Column, String>>)>);

// Paramètres de système Bevy : c'est la forme normale (cf. `interact`).
#[allow(clippy::too_many_arguments)]
pub fn stream_chunks(
    mut commands: Commands,
    mut game: ResMut<GameWorld>,
    mut dirty: ResMut<DirtyChunks>,
    mut meshing: ResMut<Meshing>,
    mut generating: ResMut<Generating>,
    // Chunks affichés dont un voisin a reçu ses données depuis leur mesh.
    mut stale: Local<HashSet<ChunkPos>>,
    mut storage: ResMut<StorageChanged>,
    save: Option<Res<WorldSave>>,
    player: Query<&Transform, With<Player>>,
    mut meshes: ResMut<Assets<Mesh>>,
    chunk_meshes: Query<(Entity, &ChunkMesh, Option<&Mesh3d>)>,
) {
    let Ok(player) = player.single() else { return };
    let p = player.translation;
    let extent_m = game.world.chunk_size() as f32 / game.world.voxels_per_meter();

    // Chunks ayant une entité (affichés — y compris ceux au mesh vide).
    let displayed: HashSet<ChunkPos> = chunk_meshes.iter().map(|(_, cm, _)| cm.0).collect();

    // Distances (mètres) du joueur au centre d'un chunk : horizontale et
    // verticale, séparées car la zone chargée est un cylindre.
    let horiz_m = |pos: ChunkPos| -> f32 {
        let cx = (pos.x as f32 + 0.5) * extent_m;
        let cz = (pos.z as f32 + 0.5) * extent_m;
        ((cx - p.x).powi(2) + (cz - p.z).powi(2)).sqrt()
    };
    let vert_m = |pos: ChunkPos| ((pos.y as f32 + 0.5) * extent_m - p.y).abs();

    // --- 1. L'ensemble voulu : un cylindre de chunks autour du joueur. ---
    let (pcx, pcz) = (
        (p.x / extent_m).floor() as i32,
        (p.z / extent_m).floor() as i32,
    );
    let radius_chunks = (VIEW_DISTANCE_M / extent_m).ceil() as i32;
    // Couches dont le centre est à moins de VERTICAL_VIEW_M, dans le monde.
    let y_min = ((p.y - VERTICAL_VIEW_M) / extent_m - 0.5)
        .ceil()
        .max(WORLD_MIN_Y_M / extent_m) as i32;
    let y_max = ((p.y + VERTICAL_VIEW_M) / extent_m - 0.5)
        .floor()
        .min(WORLD_MAX_Y_M / extent_m - 1.0) as i32;
    let mut desired: Vec<ChunkPos> = Vec::new();
    for y in y_min..=y_max {
        for dz in -radius_chunks..=radius_chunks {
            for dx in -radius_chunks..=radius_chunks {
                let pos = ChunkPos { x: pcx + dx, y, z: pcz + dz };
                if horiz_m(pos) <= VIEW_DISTANCE_M {
                    desired.push(pos);
                }
            }
        }
    }
    // Du plus proche au plus loin (en 3D) : le chunk sous les pieds arrive
    // en premier.
    let dist_m = |pos: ChunkPos| horiz_m(pos).hypot(vert_m(pos));
    desired.sort_by(|a, b| dist_m(*a).total_cmp(&dist_m(*b)));

    // --- 2. Insérer les colonnes chargées, sous budget. ---
    // Lumière changée avant (pose/casse, blocs qui changent seuls) : re-meshé
    // tout de suite, avec le reste de l'edit.
    let lit = game.world.take_light_changed();
    dirty.0.extend(lit.into_iter().filter(|n| displayed.contains(n) || meshing.contains(*n)));
    // Une colonne arrêtée sous un chunk encore en chargement attend qu'il
    // soit inséré : sinon elle se croirait sous le ciel ouvert.
    let finished: Vec<ChunkPos> = generating
        .0
        .iter()
        .filter(|(_, (stop, task))| task.is_finished() && !generating.0.contains_key(stop))
        .map(|(pos, _)| *pos)
        .collect();
    let mut inserted = 0;
    for key in finished {
        if inserted >= INSERT_BUDGET_PER_FRAME {
            break;
        }
        // Un fichier illisible arrête le jeu plutôt que d'être écrasé par un
        // chunk régénéré au prochain édit.
        let column = block_on(generating.0.remove(&key).unwrap().1).unwrap_or_else(|err| panic!("{err}"));
        // Du haut vers le bas : la lumière du ciel descend.
        for (pos, chunk, state) in column.into_iter().rev() {
            if game.world.chunk(pos).is_some() {
                continue; // apporté par une autre colonne
            }
            match state {
                Some(state) => {
                    game.world.insert_saved_chunk(pos, chunk, state);
                    storage.0.extend(game.world.block_entity_positions(pos));
                }
                None => game.world.insert_chunk(pos, chunk),
            }
            inserted += 1;
            // La bordure des voisins déjà meshés doit se recoudre contre ce
            // nouveau chunk (culling inter-chunks, ombre des coins).
            stale.extend(neighbors(pos).filter(|n| displayed.contains(n) || meshing.contains(*n)));
        }
    }
    // La lumière que ces chunks ont changée chez leurs voisins : re-meshés
    // sous le budget de l'étape 4, comme les bordures.
    let lit = game.world.take_light_changed();
    stale.extend(lit.into_iter().filter(|n| displayed.contains(n) || meshing.contains(*n)));

    // --- 3. Lancer le chargement des chunks sans données, du plus proche au
    // plus loin. ---
    for &pos in &desired {
        if generating.0.len() >= GEN_TASKS {
            break;
        }
        let pending = |pos: ChunkPos| game.world.chunk(pos).is_some() || generating.0.contains_key(&pos);
        if pending(pos) {
            continue;
        }
        // La lumière (§3.8) tient un chunk sans données au-dessus de lui
        // pour sous le ciel ouvert : la colonne monte jusqu'à un chunk déjà
        // là ou en cours de chargement, ou sûrement vide (plus haut que tout
        // ce que pose le générateur, et pas dans la save). Ces chunks-là ont
        // des données mais pas d'entité.
        let mut stop = ChunkPos { y: pos.y + 1, ..pos };
        while (stop.y as f32) * extent_m < WORLD_MAX_Y_M && !pending(stop) {
            stop.y += 1;
        }
        let (generator, save) = (game.generator.clone(), save.as_ref().map(|s| s.0.clone()));
        let (size, vpm) = (game.world.chunk_size(), game.world.voxels_per_meter());
        let task = AsyncComputeTaskPool::get().spawn(async move {
            // Sauvé (modifié par le joueur) : relu ; sinon généré depuis la seed.
            let load = |pos: ChunkPos| -> Result<_, String> {
                Ok(match save.as_ref().map_or(Ok(None), |s| s.read_chunk(pos))? {
                    Some((chunk, state)) => (pos, chunk, Some(state)),
                    None => (pos, generator.generate_chunk(pos, size, vpm), None),
                })
            };
            let mut column = vec![load(pos)?];
            for y in pos.y + 1..stop.y {
                let above = ChunkPos { y, ..pos };
                let saved = save.as_ref().map_or(Ok(None), |s| s.read_chunk(above))?;
                if saved.is_none() && y as f32 * extent_m >= generator.top_m() {
                    break;
                }
                column.push(match saved {
                    Some((chunk, state)) => (above, chunk, Some(state)),
                    None => load(above)?,
                });
            }
            Ok(column)
        });
        generating.0.insert(pos, (stop, task));
    }

    // --- 4. Mesher ce qui est prêt. ---
    let wanted: HashSet<ChunkPos> = desired.iter().copied().collect();
    let mut budget = MESH_BUDGET_PER_FRAME;
    for &pos in &desired {
        if budget == 0 {
            break;
        }
        // Jamais affiché (chunk neuf, ou données d'un passage précédent qui
        // reviennent dans le rayon), ou un voisin est arrivé depuis.
        let new = !displayed.contains(&pos) && !meshing.contains(pos);
        if !(new || stale.contains(&pos)) || game.world.chunk(pos).is_none() {
            continue;
        }
        // Ses voisins de la zone d'abord : meshé une fois, et non une fois
        // par voisin qui arrive après lui (#53).
        if neighbors(pos).any(|n| wanted.contains(&n) && game.world.chunk(n).is_none()) {
            continue;
        }
        stale.remove(&pos);
        dirty.0.insert(pos);
        budget -= 1;
    }

    // --- 3. Décharger les chunks trop loin (les données restent). ---
    for (entity, cm, mesh3d) in &chunk_meshes {
        if horiz_m(cm.0) > VIEW_DISTANCE_M + UNLOAD_MARGIN_M
            || vert_m(cm.0) > VERTICAL_VIEW_M + UNLOAD_MARGIN_M
        {
            if let Some(mesh3d) = mesh3d {
                meshes.remove(mesh3d.id());
            }
            commands.entity(entity).despawn();
            // Plus d'entité : ne pas le re-mesher (ça la recréerait).
            dirty.0.remove(&cm.0);
            meshing.forget(cm.0);
            stale.remove(&cm.0);
        }
    }
}

/// Les 26 chunks autour : ceux d'une face (culling inter-chunks), mais aussi
/// d'une arête ou d'un coin, dont l'ombre des coins de faces dépend.
fn neighbors(pos: ChunkPos) -> impl Iterator<Item = ChunkPos> {
    (-1..=1)
        .flat_map(|x| (-1..=1).flat_map(move |y| (-1..=1).map(move |z| [x, y, z])))
        .filter(|&d| d != [0, 0, 0])
        .map(move |[x, y, z]| ChunkPos { x: pos.x + x, y: pos.y + y, z: pos.z + z })
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::*;
    use voxel_core::chunk::CHUNK_SIZE;
    use voxel_core::registry::{ContentEntry, Registry};
    use voxel_core::world::VoxelWorld;
    use voxel_core::save::Save;
    use voxel_core::worldgen::HeightmapGenerator;

    use super::*;
    use crate::remesh_dirty;

    /// App headless : monde **tout en air** (sol à −1000 m, chaque chunk a
    /// un mesh vide), joueur à l'altitude `y_m`, streaming lancé jusqu'à
    /// tout charger. Retourne l'app et les chunks qui ont une entité.
    fn stream_all_air(y_m: f32) -> (App, Vec<ChunkPos>) {
        stream_all_air_with(y_m, None)
    }

    fn stream_all_air_with(y_m: f32, save: Option<Save>) -> (App, Vec<ChunkPos>) {
        stream_under(-1000.0, y_m, save)
    }

    /// Comme [`stream_all_air`], avec un sol de pierre à `ground_level_m`.
    fn stream_under(ground_level_m: f32, y_m: f32, save: Option<Save>) -> (App, Vec<ChunkPos>) {
        let mut app = app_under(ground_level_m, 1.0, y_m, save);
        settle(&mut app);
        let mut q = app.world_mut().query::<&ChunkMesh>();
        let loaded = q.iter(app.world()).map(|c| c.0).collect();
        (app, loaded)
    }

    /// L'app headless du streaming, avant la première image.
    fn app_under(ground_level_m: f32, amplitude_m: f32, y_m: f32, save: Option<Save>) -> App {
        let mut registry = Registry::new();
        let air = registry
            .register(ContentEntry::new_block("core:air", false, [0.0; 3]))
            .unwrap();
        let stone = registry
            .register(ContentEntry::new_block("core:stone", true, [0.5; 3]))
            .unwrap();
        let generator = HeightmapGenerator {
            seed: 1,
            air,
            ground: stone,
            stone,
            ground_level_m,
            amplitude_m,
            feature_size_m: 24.0,
        };
        let mut app = App::new();
        app.add_plugins(TaskPoolPlugin::default())
            .init_resource::<Assets<Mesh>>()
            .init_resource::<DirtyChunks>()
            .init_resource::<Meshing>()
            .init_resource::<Generating>()
            .init_resource::<StorageChanged>()
            .insert_resource(GameWorld {
                world: VoxelWorld::new(registry, CHUNK_SIZE, 1.0),
                generator: generator.into(),
                air,
                material: Handle::default(),
                blend_material: Handle::default(),
            })
            .add_systems(Update, (stream_chunks, remesh_dirty).chain());
        if let Some(save) = save {
            app.insert_resource(WorldSave(save));
        }
        app.world_mut().spawn((
            Player::default(),
            Transform::from_xyz(0.5, y_m, 0.5),
        ));
        app
    }

    /// Fait tourner l'app jusqu'à ce que le streaming n'ait plus rien en
    /// cours : les tâches finissent à leur rythme, pas en un nombre
    /// d'images connu d'avance.
    fn settle(app: &mut App) {
        let mut quiet = 0;
        for _ in 0..100_000 {
            app.update();
            let busy = !app.world().resource::<DirtyChunks>().0.is_empty()
                || !app.world().resource::<Meshing>().0.is_empty()
                || !app.world().resource::<Generating>().0.is_empty();
            quiet = if busy { 0 } else { quiet + 1 };
            // Deux images calmes : le streaming a pu demander ce qui restait.
            if quiet == 2 {
                return;
            }
            std::thread::yield_now();
        }
        panic!("le streaming ne s'arrête pas");
    }

    /// Régression : un chunk vide n'avait pas d'entité, passait pour jamais
    /// chargé, et était re-meshé à chaque frame en mangeant le budget.
    #[test]
    fn empty_chunks_load_once_and_stay_loaded() {
        let (mut app, loaded) = stream_all_air(0.0);
        let unique: HashSet<ChunkPos> = loaded.iter().copied().collect();
        assert!(!loaded.is_empty());
        assert_eq!(loaded.len(), unique.len(), "entité-chunk en double");

        // Une fois tout chargé, le streaming ne doit plus rien demander.
        app.world_mut().run_system_once(stream_chunks).unwrap();
        assert!(app.world().resource::<DirtyChunks>().0.is_empty());
        assert!(app.world().resource::<Generating>().0.is_empty());
    }

    #[test]
    fn layers_follow_player_and_stop_at_world_bottom() {
        // Joueur à y = 0 : couches -2..=1 (centres à ≤ 48 m de lui).
        let (_, loaded) = stream_all_air(0.0);
        let layers: HashSet<i32> = loaded.iter().map(|c| c.y).collect();
        assert_eq!(layers, HashSet::from([-2, -1, 0, 1]));

        // Près du fond du monde (−128 m = bas de la couche −4) : la couche
        // −5 serait à portée (centre à 24 m) mais hors du monde.
        let (_, loaded) = stream_all_air(-120.0);
        let layers: HashSet<i32> = loaded.iter().map(|c| c.y).collect();
        assert_eq!(layers, HashSet::from([-4, -3]));
    }

    #[test]
    fn saved_chunk_is_read_instead_of_generated() {
        // Un chunk sauvé porte un voxel `marker` que le générateur (tout
        // en air) ne produit jamais : s'il est là, le fichier a été relu.
        let mut registry = Registry::new();
        let air = registry.register(ContentEntry::new_block("core:air", false, [0.0; 3])).unwrap();
        let marker = registry.register(ContentEntry::new_block("t:marker", false, [0.0; 3])).unwrap();
        let mut world = VoxelWorld::new(registry, CHUNK_SIZE, 1.0);
        let pos = ChunkPos { x: 0, y: 0, z: 0 };
        world.insert_chunk(pos, voxel_core::chunk::Chunk::filled(CHUNK_SIZE, air));
        world.set_voxel([1, 2, 3], marker);
        let dir = std::env::temp_dir().join(format!("voxel_stream_test_{}", std::process::id()));
        let save = Save::new(&dir);
        save.write_chunk(&world, pos).unwrap();

        let (app, _) = stream_all_air_with(0.0, Some(save));
        let game = app.world().resource::<GameWorld>();
        assert_eq!(game.world.voxel([1, 2, 3]), Some(marker), "relu, pas régénéré");
        assert_eq!(game.world.voxel([1, 2, 4]), Some(air));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// La lumière du ciel (§3.8) : sous une montagne plus haute que la zone
    /// chargée, le streaming charge d'abord ce qui la couvre (des données,
    /// sans entité), et le joueur est dans le noir.
    #[test]
    fn chunks_above_the_view_are_loaded_for_the_sky() {
        let (app, loaded) = stream_under(150.0, 0.0, None);
        let world = &app.world().resource::<GameWorld>().world;
        assert_eq!(world.light_at([0, 0, 0])[voxel_core::light::SKY], 0);
        let covering = ChunkPos { x: 0, y: 4, z: 0 }; // 128 à 160 m : le sommet
        assert!(world.chunk(covering).is_some());
        assert!(!loaded.contains(&covering), "chargé pour le ciel, pas affiché");
        assert!(world.chunk(ChunkPos { x: 0, y: 5, z: 0 }).is_none(), "au-dessus de tout relief : rien");
    }

    /// Meshes demandés depuis la mise à zéro.
    #[derive(Resource, Default)]
    struct Asked(usize);

    fn count_asked(app: &mut App) {
        app.init_resource::<Asked>().add_systems(
            Update,
            (|dirty: Res<DirtyChunks>, mut asked: ResMut<Asked>| asked.0 += dirty.0.len())
                .after(stream_chunks)
                .before(remesh_dirty),
        );
    }

    /// #53 : un chunk attend ses voisins de la zone avant d'être meshé, au
    /// lieu d'être re-meshé à l'arrivée de chacun (5,9 meshes par chunk).
    #[test]
    fn each_chunk_is_meshed_once_on_load() {
        let mut app = app_under(16.0, 6.0, 20.0, None);
        count_asked(&mut app);
        settle(&mut app);
        let chunks = app.world_mut().query::<&ChunkMesh>().iter(app.world()).count();
        assert!(chunks > 50, "{chunks} chunks");
        assert_eq!(app.world().resource::<Asked>().0, chunks);
    }

    /// Mesure (#53) : temps de l'image (la part du streaming et du
    /// meshing qui reste dans l'image) au chargement d'un monde neuf, puis en
    /// courant à 8 m/s. Images cadencées à 144 Hz, comme l'écran de la 4060.
    /// `cargo test --release --bin voxel_engine -- --ignored --nocapture
    /// mesure_chargement`
    #[test]
    #[ignore]
    fn mesure_chargement() {
        let mut app = app_under(16.0, 6.0, 20.0, None);
        count_asked(&mut app);
        let frame = std::time::Duration::from_secs_f64(1.0 / 144.0);
        let report = |app: &mut App, name: &str, frames: usize, speed_m_s: f32| {
            app.world_mut().resource_mut::<Asked>().0 = 0;
            let mut times = Vec::new();
            for _ in 0..frames {
                let start = std::time::Instant::now();
                let mut q = app.world_mut().query_filtered::<&mut Transform, With<Player>>();
                q.single_mut(app.world_mut()).unwrap().translation.x += speed_m_s * frame.as_secs_f32();
                app.update();
                let t = start.elapsed();
                times.push(t.as_secs_f64() * 1000.0);
                std::thread::sleep(frame.saturating_sub(t));
            }
            let chunks = app.world_mut().query::<&ChunkMesh>().iter(app.world()).count();
            times.sort_by(f64::total_cmp);
            eprintln!(
                "{name} : {chunks} chunks affichés, {} meshes demandés, image médiane {:.2} ms, 99 % {:.2} ms, pire {:.2} ms, {} images > 7 ms",
                app.world().resource::<Asked>().0,
                times[times.len() / 2],
                times[times.len() * 99 / 100],
                times[times.len() - 1],
                times.iter().filter(|&&t| t > 7.0).count(),
            );
        };
        report(&mut app, "chargement (3 s)", 3 * 144, 0.0);
        report(&mut app, "course (10 s)", 10 * 144, 8.0);
    }
}
