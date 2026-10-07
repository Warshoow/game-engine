//! Binaire du moteur : l'app Bevy. Toute la logique voxel pure vit dans
//! `voxel_core` (testable headless) ; ici on ne fait que brancher :
//! registre → worldgen → mesher → entités Bevy, et le contrôleur joueur.

mod interact;
mod player;
mod streaming;

use std::collections::{HashMap, HashSet};

use bevy::asset::RenderAssetUsages;
use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::mesh::{Indices, Mesh};
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;

use voxel_core::chunk::{ChunkPos, CHUNK_SIZE};
use voxel_core::mesher::{mesh_chunk_in_world, MeshData};
use voxel_core::registry::{ContentEntry, ContentId, Registry};
use voxel_core::world::VoxelWorld;
use voxel_core::worldgen::HeightmapGenerator;

/// Le monde côté app : le `VoxelWorld` (qui possède registre + chunks, §3.1)
/// et le générateur. Ressource ECS : physique, pose/casse et remeshing y
/// accèdent.
#[derive(Resource)]
pub struct GameWorld {
    pub world: VoxelWorld,
    pub generator: HeightmapGenerator,
    /// IDs résolus une fois au setup — le gameplay manipule des `ContentId`,
    /// jamais des identifiers en dur dans les systèmes.
    pub air: ContentId,
    /// Blocs posables, **découverts** depuis le registre (tout bloc solide) —
    /// jamais une liste de noms en dur. L'index est la sélection courante.
    pub hotbar: Vec<ContentId>,
    pub held_idx: usize,
    /// Matériau partagé des chunks (blanc, couleurs aux sommets).
    pub material: Handle<StandardMaterial>,
}

impl GameWorld {
    /// Le bloc « en main » pour la pose (§7.3) — data-driven par ID.
    pub fn held(&self) -> ContentId {
        self.hotbar[self.held_idx]
    }
}

/// Marque l'entité d'un chunk **chargé** (côté affichage). L'entité existe
/// même quand le mesh est vide (chunk d'air) : elle n'a alors pas de
/// `Mesh3d`. « Chargé » = « a une entité », sans exception — sinon un chunk
/// vide passerait pour jamais chargé et serait re-meshé à chaque frame.
#[derive(Component)]
pub struct ChunkMesh(pub ChunkPos);

/// Chunks à (re)mesher cette frame. La pose/casse et le streaming ne meshent
/// pas eux-mêmes : ils notent ici, et [`remesh_dirty`] meshe une fois par
/// chunk, en fin de frame. Un seul système crée les entités-chunk : deux
/// systèmes ne peuvent plus spawner chacun la sienne pour le même chunk
/// (une entité spawnée via `Commands` n'est visible des `Query` qu'à la
/// frame suivante — chacun aurait cru qu'elle n'existait pas).
#[derive(Resource, Default)]
pub struct DirtyChunks(pub HashSet<ChunkPos>);

/// Marque le texte HUD affichant le bloc en main.
#[derive(Component)]
pub struct HeldBlockText;

/// Marque le texte HUD de debug (FPS, chunks chargés).
#[derive(Component)]
struct DebugText;

fn main() {
    // WSLg : le compositeur Wayland ne fournit ni pointer lock ni mouvements
    // relatifs de souris → caméra FPS morte. On masque WAYLAND_DISPLAY pour
    // que winit retombe sur X11 (XWayland), qui gère les deux. À retirer le
    // jour où WSLg le supporte (ou hors WSL).
    // SAFETY: avant la création de tout thread (première ligne de main).
    unsafe { std::env::remove_var("WAYLAND_DISPLAY") };

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Moteur Voxel".into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins((player::PlayerPlugin, FrameTimeDiagnosticsPlugin::default()))
        .init_resource::<DirtyChunks>()
        .add_systems(Startup, (setup_world, player::spawn_player).chain())
        // Pose/casse puis streaming notent les chunks sales ; on meshe après.
        .add_systems(
            Update,
            (
                streaming::stream_chunks.after(interact::interact),
                remesh_dirty.after(streaming::stream_chunks),
                update_debug_text,
            ),
        )
        .run();
}

fn setup_world(mut commands: Commands, mut materials: ResMut<Assets<StandardMaterial>>) {
    // --- Le monde possède son contenu (§3.1) : tout part du registre. ---
    let mut registry = Registry::new();
    let air = registry
        .register(ContentEntry::new_block("core:air", false, [0.0; 3]))
        .expect("registre vide");
    let grass = registry
        .register(ContentEntry::new_block("core:grass", true, [0.35, 0.6, 0.25]))
        .expect("identifier unique");
    let stone = registry
        .register(ContentEntry::new_block("core:stone", true, [0.55, 0.55, 0.58]))
        .expect("identifier unique");
    // Du contenu, pas du code (§0) : ces blocs n'existent qu'ici, en donnée.
    // Aucun système ne les connaît — ils arrivent dans la hotbar par
    // découverte du registre, et le worldgen n'en pose aucun.
    for (identifier, color) in [
        ("core:dirt", [0.45, 0.30, 0.15]),
        ("core:sand", [0.85, 0.78, 0.55]),
    ] {
        registry
            .register(ContentEntry::new_block(identifier, true, color))
            .expect("identifier unique");
    }

    // Gameplay en mètres (§2) : le relief est défini en mètres, la
    // résolution voxel ne fait que convertir.
    let voxels_per_meter = 1.0;
    let generator = HeightmapGenerator {
        seed: 42,
        air,
        ground: grass,
        stone,
        ground_level_m: 16.0,
        amplitude_m: 6.0,
        feature_size_m: 24.0,
    };

    // --- Le monde démarre VIDE : c'est le streaming (Update) qui génère et
    //     meshe les chunks autour du joueur, dès la première frame. La
    //     physique se fige tant que le sol sous le joueur n'est pas chargé.
    let world = VoxelWorld::new(registry, CHUNK_SIZE, voxels_per_meter);
    let material = materials.add(Color::WHITE); // blanc : les couleurs viennent des sommets

    // La hotbar se **découvre** : tout bloc solide du registre est posable.
    // Ajouter un bloc au registre suffit à le rendre disponible — aucun
    // système à toucher. (grass est solide → présent, air non → absent.)
    let hotbar: Vec<ContentId> = world
        .registry
        .iter()
        .filter(|(_, e)| e.block().is_some_and(|b| b.solid))
        .map(|(id, _)| id)
        .collect();
    let held_idx = hotbar.iter().position(|&id| id == grass).unwrap_or(0);
    let held_label = held_label(&world.registry, hotbar[held_idx]);

    commands.insert_resource(GameWorld {
        world,
        generator,
        air,
        hotbar,
        held_idx,
        material,
    });

    // Crosshair + aide minimale (HUD debug — §7 : pas d'UI riche).
    commands.spawn((
        Text::new("+"),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(50.0),
            top: Val::Percent(50.0),
            ..default()
        },
    ));
    commands.spawn((
        Text::new("Clic gauche : jouer · Échap : libérer la souris · F : plein écran\nWASD/Espace : bouger · gauche : casser · droit : poser · molette : bloc"),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(12.0),
            bottom: Val::Px(12.0),
            ..default()
        },
    ));
    commands.spawn((
        DebugText,
        Text::default(),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(12.0),
            top: Val::Px(12.0),
            ..default()
        },
    ));
    // Bloc en main (HUD debug — §7 : pas d'UI riche).
    commands.spawn((
        HeldBlockText,
        Text::new(held_label),
        Node {
            position_type: PositionType::Absolute,
            right: Val::Px(12.0),
            bottom: Val::Px(12.0),
            ..default()
        },
    ));

    // --- Éclairage full-bright-ish (non-goal §7 : pas de vrai éclairage). ---
    commands.spawn((
        DirectionalLight::default(),
        Transform::from_xyz(50.0, 80.0, 30.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

/// (Re)meshe les chunks notés dans [`DirtyChunks`] — l'unique chemin de
/// meshing (génération, streaming, pose/casse). Met à jour l'asset existant,
/// ajoute/retire le `Mesh3d` si le chunk passe de/à vide, ou spawn l'entité
/// d'un chunk qui n'en a pas encore.
pub fn remesh_dirty(
    mut commands: Commands,
    game: Res<GameWorld>,
    mut dirty: ResMut<DirtyChunks>,
    mut meshes: ResMut<Assets<Mesh>>,
    chunk_meshes: Query<(Entity, &ChunkMesh, Option<&Mesh3d>)>,
) {
    if dirty.0.is_empty() {
        return;
    }
    let existing: HashMap<ChunkPos, (Entity, Option<&Mesh3d>)> =
        chunk_meshes.iter().map(|(e, cm, m)| (cm.0, (e, m))).collect();
    let voxel_size_m = 1.0 / game.world.voxels_per_meter();

    for pos in dirty.0.drain() {
        let Some(data) = mesh_chunk_in_world(&game.world, pos, voxel_size_m) else {
            continue; // pas de données : rien à mesher
        };
        match (existing.get(&pos), data.is_empty()) {
            (Some(&(_, Some(mesh3d))), false) => {
                // Remplace le contenu de l'asset : l'entité et son handle ne
                // bougent pas, le GPU reçoit les nouveaux tampons.
                if let Err(err) = meshes.insert(mesh3d.id(), to_bevy_mesh(data)) {
                    error!("re-mesh du chunk {pos:?} impossible : {err}");
                }
            }
            (Some(&(entity, Some(mesh3d))), true) => {
                meshes.remove(mesh3d.id());
                commands
                    .entity(entity)
                    .remove::<(Mesh3d, MeshMaterial3d<StandardMaterial>)>();
            }
            (Some(&(entity, None)), false) => {
                commands.entity(entity).insert((
                    Mesh3d(meshes.add(to_bevy_mesh(data))),
                    MeshMaterial3d(game.material.clone()),
                ));
            }
            (Some(&(_, None)), true) => {}
            (None, empty) => {
                let extent = game.world.chunk_size() as f32 * voxel_size_m;
                let mut entity = commands.spawn((
                    ChunkMesh(pos),
                    Transform::from_xyz(
                        pos.x as f32 * extent,
                        pos.y as f32 * extent,
                        pos.z as f32 * extent,
                    ),
                ));
                if !empty {
                    entity.insert((
                        Mesh3d(meshes.add(to_bevy_mesh(data))),
                        MeshMaterial3d(game.material.clone()),
                    ));
                }
            }
        }
    }
}

/// HUD debug : FPS (moyenne glissante de Bevy) et nombre de chunks qui ont
/// une entité — la zone chargée par le streaming.
fn update_debug_text(
    diagnostics: Res<DiagnosticsStore>,
    chunks: Query<(), With<ChunkMesh>>,
    mut text: Query<&mut Text, With<DebugText>>,
) {
    let Ok(mut text) = text.single_mut() else { return };
    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|d| d.smoothed())
        .unwrap_or(0.0);
    text.0 = format!("{fps:.0} FPS · {} chunks", chunks.iter().count());
}

/// Libellé HUD du bloc en main — l'identifier vient du registre, le HUD ne
/// connaît aucun nom de bloc.
pub fn held_label(registry: &Registry, id: ContentId) -> String {
    match registry.get(id) {
        Some(entry) => format!("en main : {}", entry.identifier),
        None => "en main : ???".to_string(),
    }
}

/// Convertit les tampons purs du mesher en `Mesh` Bevy.
fn to_bevy_mesh(data: MeshData) -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, data.positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, data.normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, data.colors)
    .with_inserted_indices(Indices::U32(data.indices))
}
