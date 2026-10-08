//! Binaire du moteur : l'app Bevy. Toute la logique voxel pure vit dans
//! `voxel_core` (testable headless) ; ici on ne fait que brancher :
//! registre → worldgen → mesher → entités Bevy, et le contrôleur joueur.

mod interact;
mod inventory;
mod items;
mod player;
mod save;
mod streaming;

use std::collections::{HashMap, HashSet};

use bevy::asset::RenderAssetUsages;
use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, Mesh};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, PrimitiveTopology, TextureDimension, TextureFormat, TextureViewDescriptor,
    TextureViewDimension,
};
use bevy::shader::ShaderRef;

use voxel_core::chunk::{ChunkPos, CHUNK_SIZE};
use voxel_core::mesher::{mesh_chunk_in_world, MeshData, NO_TEXTURE};
use voxel_core::registry::{ContentId, Registry};
use voxel_core::save::{Save, WorldMeta, FORMAT_VERSION};
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
    /// Matériau partagé des chunks : les textures du registre, couleurs aux
    /// sommets pour les blocs sans texture.
    pub material: Handle<VoxelMaterial>,
}

/// `StandardMaterial` (éclairage de Bevy) + les textures des blocs.
pub type VoxelMaterial = ExtendedMaterial<StandardMaterial, BlockTextures>;

/// Les textures du registre en **texture array** : une pile d'images de
/// même taille, la couche choisie par sommet (`UV_1.x`, voir
/// [`to_bevy_mesh`]). Contrairement à un atlas, le GPU répète une couche
/// seul : c'est ce qui permet aux faces fusionnées du greedy de répéter la
/// texture.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct BlockTextures {
    #[texture(100, dimension = "2d_array")]
    #[sampler(101)]
    array: Handle<Image>,
}

impl MaterialExtension for BlockTextures {
    fn fragment_shader() -> ShaderRef {
        "shaders/voxel.wgsl".into()
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
        .add_plugins((
            player::PlayerPlugin,
            FrameTimeDiagnosticsPlugin::default(),
            MaterialPlugin::<VoxelMaterial>::default(),
        ))
        .init_resource::<DirtyChunks>()
        .init_resource::<inventory::Inventory>()
        .init_resource::<items::StorageChanged>()
        .add_systems(
            Startup,
            (setup_world, player::spawn_player, save::restore_player, save::restore_items, inventory::spawn_hotbar).chain(),
        )
        // Pose/casse puis streaming notent les chunks sales ; on meshe après.
        .add_systems(
            Update,
            (
                streaming::stream_chunks.after(interact::interact),
                remesh_dirty.after(streaming::stream_chunks),
                update_debug_text,
                items::add_item_visuals,
                items::place_items,
                items::show_stored.after(interact::interact),
                inventory::scroll_selection,
                inventory::give_all_blocks,
                inventory::craft_from_inventory,
                inventory::update_hotbar.after(inventory::scroll_selection),
                save::save_edited_chunks.after(interact::interact),
            ),
        )
        .add_systems(Last, save::save_player_and_items.after(bevy::window::ExitSystems))
        .add_systems(
            FixedUpdate,
            items::simulate_items.after(player::physics_step),
        )
        .run();
}

fn setup_world(
    mut commands: Commands,
    mut materials: ResMut<Assets<VoxelMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    // --- Le monde possède son contenu (§3.1) : tout part du registre, lu
    //     depuis un fichier de données — aucun bloc n'est défini en Rust. ---
    let path = assets_dir().join("content/core.ron");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("lecture de {} impossible : {err}", path.display()));
    let content = Registry::from_ron(&text)
        .unwrap_or_else(|err| panic!("contenu invalide dans {} : {err}", path.display()));

    // --- Save (§3.10) : un monde existant impose ses IDs, sa seed et sa
    //     résolution ; le contenu du jeu y est fusionné. Une save illisible
    //     arrête le jeu : elle n'est jamais écrasée. ---
    let save = Save::new(base_dir().join("saves/world"));
    let refuse = |err: String| -> ! { panic!("save {} refusée : {err}", save.dir().display()) };
    let (mut registry, meta) = match save.read_meta().unwrap_or_else(|e| refuse(e)) {
        Some(meta) => {
            let mut registry = save.read_registry().unwrap_or_else(|e| refuse(e));
            registry.merge(content).unwrap_or_else(|e| refuse(e.to_string()));
            (registry, meta)
        }
        None => (
            content,
            WorldMeta { format: FORMAT_VERSION, seed: 42, voxels_per_meter: 1.0, chunk_size: CHUNK_SIZE },
        ),
    };
    registry
        .load_textures(&assets_dir().join("textures"))
        .unwrap_or_else(|err| panic!("{err}"));
    let array = images.add(texture_array(&registry));
    save.write_world(&meta, &registry).unwrap_or_else(|e| refuse(e));
    // Le worldgen a besoin de quelques matériaux : résolus par identifier,
    // une fois ici. Le reste du contenu, aucun système ne le nomme.
    let id = |identifier: &str| {
        registry
            .lookup(identifier)
            .unwrap_or_else(|| panic!("{identifier} absent de {}", path.display()))
    };
    let (air, grass, stone) = (id("core:air"), id("core:grass"), id("core:stone"));

    // Gameplay en mètres (§2) : le relief est défini en mètres, la
    // résolution voxel ne fait que convertir.
    let generator = HeightmapGenerator {
        seed: meta.seed,
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
    let world = VoxelWorld::new(registry, meta.chunk_size, meta.voxels_per_meter);
    let material = materials.add(ExtendedMaterial {
        base: StandardMaterial::from(Color::WHITE), // la couleur vient des sommets et des textures
        extension: BlockTextures { array },
    });

    commands.insert_resource(save::WorldSave(save));
    commands.insert_resource(GameWorld {
        world,
        generator,
        air,
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
        Text::new("Clic gauche : jouer · Échap : libérer la souris · F : plein écran\nWASD/Espace : bouger · gauche : casser · droit : utiliser/poser · Maj+droit : poser · molette : bloc · C : fabriquer · G : un de chaque bloc (debug)"),
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
        Text::default(), // rempli par inventory::update_hotbar
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
                    .remove::<(Mesh3d, MeshMaterial3d<VoxelMaterial>)>();
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

/// Dossier du jeu : celui du `Cargo.toml` sous `cargo run`, celui de
/// l'exécutable sinon (build Windows : copier `assets/` avec le `.exe`).
/// Même règle que les assets Bevy. `assets/` et `saves/` y vivent.
fn base_dir() -> std::path::PathBuf {
    std::env::var_os("CARGO_MANIFEST_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_exe().ok()?.parent().map(Into::into))
        .unwrap_or_default()
}

fn assets_dir() -> std::path::PathBuf {
    base_dir().join("assets")
}

/// Empile les textures du registre (couche = index dans le registre). Un
/// registre sans texture donne une couche blanche 1 × 1 : un texture array
/// vide n'existe pas côté GPU.
fn texture_array(registry: &Registry) -> Image {
    let textures = registry.textures();
    let size = textures.first().map_or(1, |t| t.size);
    let data: Vec<u8> = if textures.is_empty() {
        vec![255; 4]
    } else {
        textures.iter().flat_map(|t| t.rgba.iter().copied()).collect()
    };
    let mut image = Image::new(
        Extent3d { width: size, height: size, depth_or_array_layers: textures.len().max(1) as u32 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    // Vue « array » même avec une seule couche (sinon Bevy en ferait une 2D).
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::D2Array),
        ..default()
    });
    // Répétition (les coordonnées vont au-delà de 1 sur une face fusionnée)
    // et filtrage au plus proche : des pixels nets, pas un flou.
    // ponytail: sans mipmaps, les textures fourmillent au loin ; à générer
    // si ça gêne.
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        ..ImageSamplerDescriptor::nearest()
    });
    image
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
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, data.uvs)
    // La couche passe par le 2ᵉ jeu d'UV, que le shader standard transmet
    // déjà au fragment : pas de vertex shader à écrire. -1 : pas de texture.
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_UV_1,
        data.layers
            .iter()
            .map(|&l| [if l == NO_TEXTURE { -1.0 } else { l as f32 }, 0.0])
            .collect::<Vec<[f32; 2]>>(),
    )
    .with_inserted_indices(Indices::U32(data.indices))
}
