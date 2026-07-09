//! Binaire du moteur : l'app Bevy. Toute la logique voxel pure vit dans
//! `voxel_core` (testable headless) ; ici on ne fait que brancher :
//! registre → worldgen → mesher → entités Bevy, et le contrôleur joueur.

mod interact;
mod player;
mod streaming;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, Mesh};
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;

use voxel_core::chunk::{ChunkPos, CHUNK_SIZE};
use voxel_core::mesher::{mesh_chunk_in_world, MeshData};
use voxel_core::registry::{BlockData, ContentEntry, ContentId, Kind, Registry};
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

/// Marque l'entité-mesh d'un chunk — pour retrouver quoi re-mesher quand un
/// voxel change (pose/casse, prochaine étape).
#[derive(Component)]
pub struct ChunkMesh(pub ChunkPos);

/// Marque le texte HUD affichant le bloc en main.
#[derive(Component)]
pub struct HeldBlockText;

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
        .add_plugins(player::PlayerPlugin)
        .add_systems(Startup, (setup_world, player::spawn_player).chain())
        .add_systems(Update, streaming::stream_chunks)
        .run();
}

fn setup_world(mut commands: Commands, mut materials: ResMut<Assets<StandardMaterial>>) {
    // --- Le monde possède son contenu (§3.1) : tout part du registre. ---
    let mut registry = Registry::new();
    let air = registry
        .register(ContentEntry {
            identifier: "core:air".into(),
            kind: Kind::Block,
            block: Some(BlockData { solid: false, color: [0.0; 3] }),
        })
        .expect("registre vide");
    let grass = registry
        .register(ContentEntry {
            identifier: "core:grass".into(),
            kind: Kind::Block,
            block: Some(BlockData { solid: true, color: [0.35, 0.6, 0.25] }),
        })
        .expect("identifier unique");
    // Du contenu, pas du code (§0) : ces blocs n'existent qu'ici, en donnée.
    // Aucun système ne les connaît — ils arrivent dans la hotbar par
    // découverte du registre, et le worldgen n'en pose aucun.
    for (identifier, color) in [
        ("core:dirt", [0.45, 0.30, 0.15]),
        ("core:stone", [0.55, 0.55, 0.58]),
        ("core:sand", [0.85, 0.78, 0.55]),
    ] {
        registry
            .register(ContentEntry {
                identifier: identifier.into(),
                kind: Kind::Block,
                block: Some(BlockData { solid: true, color }),
            })
            .expect("identifier unique");
    }

    // Gameplay en mètres (§2) : le relief est défini en mètres, la
    // résolution voxel ne fait que convertir.
    let voxels_per_meter = 1.0;
    let generator = HeightmapGenerator {
        seed: 42,
        air,
        ground: grass,
        ground_level_m: 16.0,
        amplitude_m: 6.0,
        feature_size_m: 24.0,
        voxels_per_meter,
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
        .filter(|(_, e)| e.block.as_ref().is_some_and(|b| b.solid))
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

/// (Re)construit le mesh du chunk `pos` : met à jour l'asset existant, ou
/// spawn/despawn l'entité si le chunk passe de/à vide. Utilisé par la
/// pose/casse ET le streaming — un seul chemin de meshing.
pub fn remesh_chunk(
    commands: &mut Commands,
    game: &GameWorld,
    pos: ChunkPos,
    meshes: &mut Assets<Mesh>,
    chunk_meshes: &Query<(Entity, &ChunkMesh, &Mesh3d)>,
) {
    let voxel_size_m = 1.0 / game.world.voxels_per_meter;
    let Some(data) = mesh_chunk_in_world(&game.world, pos, voxel_size_m) else {
        return; // chunk non chargé : rien à mesher
    };
    let existing = chunk_meshes.iter().find(|(_, cm, _)| cm.0 == pos);

    match (existing, data.is_empty()) {
        (Some((entity, _, mesh3d)), true) => {
            meshes.remove(mesh3d.id());
            commands.entity(entity).despawn();
        }
        (Some((_, _, mesh3d)), false) => {
            // Remplace le contenu de l'asset : l'entité et son handle ne
            // bougent pas, le GPU reçoit les nouveaux tampons.
            if let Err(err) = meshes.insert(mesh3d.id(), to_bevy_mesh(data)) {
                error!("re-mesh du chunk {pos:?} impossible : {err}");
            }
        }
        (None, false) => {
            let extent = game.world.chunk_size() as f32 * voxel_size_m;
            commands.spawn((
                ChunkMesh(pos),
                Mesh3d(meshes.add(to_bevy_mesh(data))),
                MeshMaterial3d(game.material.clone()),
                Transform::from_xyz(
                    pos.x as f32 * extent,
                    pos.y as f32 * extent,
                    pos.z as f32 * extent,
                ),
            ));
        }
        (None, true) => {}
    }
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
