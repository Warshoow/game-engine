//! Binaire du moteur : l'app Bevy. Toute la logique voxel pure vit dans
//! `voxel_core` (testable headless) ; ici on ne fait que brancher :
//! registre → worldgen → mesher → entités Bevy.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, Mesh};
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;

use voxel_core::chunk::{ChunkPos, CHUNK_SIZE};
use voxel_core::mesher::{mesh_chunk, MeshData};
use voxel_core::registry::{BlockData, ContentEntry, Kind, Registry};
use voxel_core::worldgen::{HeightmapGenerator, WorldGenerator};

/// Le monde côté app : registre world-owned + paramètres de génération.
/// Resource ECS pour que les systèmes suivants (pose/casse) y accèdent.
#[derive(Resource)]
struct VoxelWorld {
    registry: Registry,
    generator: HeightmapGenerator,
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Moteur Voxel".into(),
                ..default()
            }),
            ..default()
        }))
        .add_systems(Startup, setup)
        .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
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

    // --- Génère + meshe une grille de chunks autour de l'origine. ---
    let voxel_size_m = 1.0 / voxels_per_meter;
    let chunk_extent_m = CHUNK_SIZE as f32 * voxel_size_m;
    let material = materials.add(Color::WHITE); // blanc : les couleurs viennent des sommets

    for cx in -2..=2 {
        for cz in -2..=2 {
            let pos = ChunkPos { x: cx, y: 0, z: cz };
            let chunk = generator.generate_chunk(pos, CHUNK_SIZE);
            let data = mesh_chunk(&chunk, &registry, voxel_size_m);
            if data.is_empty() {
                continue;
            }
            commands.spawn((
                Mesh3d(meshes.add(to_bevy_mesh(data))),
                MeshMaterial3d(material.clone()),
                Transform::from_xyz(
                    pos.x as f32 * chunk_extent_m,
                    pos.y as f32 * chunk_extent_m,
                    pos.z as f32 * chunk_extent_m,
                ),
            ));
        }
    }

    commands.insert_resource(VoxelWorld { registry, generator });

    // --- Éclairage full-bright-ish (non-goal §7 : pas de vrai éclairage). ---
    commands.spawn((
        DirectionalLight::default(),
        Transform::from_xyz(50.0, 80.0, 30.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // Caméra fixe en surplomb pour l'instant — le contrôleur de déplacement
    // est l'étape suivante de la slice.
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(-40.0, 45.0, -40.0).looking_at(Vec3::new(0.0, 14.0, 0.0), Vec3::Y),
    ));
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
