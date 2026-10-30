//! Binaire du moteur : l'app Bevy. Toute la logique voxel pure vit dans
//! `voxel_core` (testable headless) ; ici on ne fait que brancher :
//! registre → worldgen → mesher → entités Bevy, et le contrôleur joueur.

mod content;
mod effects;
mod entities;
mod generate;
mod interact;
mod inventory;
mod items;
mod player;
mod save;
mod sky;
mod streaming;
mod ui;

use std::collections::{HashMap, HashSet, VecDeque};

use bevy::asset::RenderAssetUsages;
use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, Mesh, MeshVertexAttribute, MeshVertexBufferLayoutRef};
use bevy::pbr::{ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline};
use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, PrimitiveTopology, RenderPipelineDescriptor, SpecializedMeshPipelineError, TextureDimension,
    TextureFormat, TextureViewDescriptor, TextureViewDimension, VertexFormat,
};
use bevy::shader::ShaderRef;
use bevy::tasks::{block_on, AsyncComputeTaskPool, Task};

use voxel_core::chunk::{ChunkPos, CHUNK_SIZE};
use voxel_core::mesher::{MeshData, Region, NO_TEXTURE};
use voxel_core::registry::{ContentId, Registry};
use voxel_core::save::{Save, WorldMeta, FORMAT_VERSION};
use voxel_core::sky::Sky;
use voxel_core::world::{VoxelWorld, TICKS_PER_SECOND};
use voxel_core::worldgen::Worldgen;

/// Le monde côté app : le `VoxelWorld` (qui possède registre + chunks, §3.1)
/// et le générateur. Ressource ECS : physique, pose/casse et remeshing y
/// accèdent.
#[derive(Resource)]
pub struct GameWorld {
    pub world: VoxelWorld,
    pub generator: Worldgen,
    /// IDs résolus une fois au setup — le gameplay manipule des `ContentId`,
    /// jamais des identifiers en dur dans les systèmes.
    pub air: ContentId,
    /// Matériau partagé des chunks : les textures du registre, couleurs aux
    /// sommets pour les blocs sans texture.
    pub material: Handle<VoxelMaterial>,
    /// Le même, semi-transparent : blocs `alpha: Blend` (eau, #28).
    pub blend_material: Handle<VoxelMaterial>,
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
    /// Force du jour dans [0, 1] (#30) : la lumière des blocs ne compte que
    /// là où le ciel n'éclaire pas déjà autant. Tenue à jour par `sky`.
    #[uniform(102)]
    daylight: f32,
}

/// Lumière d'un sommet (#23) : `[ciel, blocs]` dans [0, 1]. Id au hasard,
/// grand, comme le demande Bevy pour un attribut à soi.
const ATTRIBUTE_LIGHT: MeshVertexAttribute = MeshVertexAttribute::new("Light", 230_118_771, VertexFormat::Float32x2);

impl MaterialExtension for BlockTextures {
    fn vertex_shader() -> ShaderRef {
        "shaders/voxel.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "shaders/voxel.wgsl".into()
    }

    /// La passe de profondeur (ombres, #51) découpe les trous des blocs
    /// `Mask` dans nos textures ; son vertex shader reste celui de Bevy.
    fn prepass_fragment_shader() -> ShaderRef {
        "shaders/voxel_prepass.wgsl".into()
    }

    /// Les attributs que lit le vertex shader, lumière comprise (les
    /// emplacements de `voxel.wgsl`). Passe principale seulement : Bevy
    /// appelle aussi `specialize` pour la passe de profondeur, qui garde
    /// ses propres emplacements (`prepass_io.wgsl`).
    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if descriptor.vertex.shader_defs.contains(&"PREPASS_PIPELINE".into()) {
            return Ok(());
        }
        descriptor.vertex.buffers = vec![layout.0.get_layout(&[
            Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
            Mesh::ATTRIBUTE_NORMAL.at_shader_location(1),
            Mesh::ATTRIBUTE_UV_0.at_shader_location(2),
            Mesh::ATTRIBUTE_UV_1.at_shader_location(3),
            Mesh::ATTRIBUTE_COLOR.at_shader_location(5),
            ATTRIBUTE_LIGHT.at_shader_location(8),
        ])?];
        Ok(())
    }
}

/// Marque l'entité d'un chunk **chargé** (côté affichage). L'entité existe
/// même quand le mesh est vide (chunk d'air) : elle n'a alors pas de
/// `Mesh3d`. « Chargé » = « a une entité », sans exception — sinon un chunk
/// vide passerait pour jamais chargé et serait re-meshé à chaque frame.
#[derive(Component)]
pub struct ChunkMesh(pub ChunkPos);

/// L'entité qui porte le mesh semi-transparent d'un chunk (#28), créée avec
/// celle de [`ChunkMesh`] au même endroit. Une entité à part : Bevy trie les
/// objets semi-transparents par entité, du plus loin au plus proche.
#[derive(Component)]
pub struct BlendMesh(pub ChunkPos);

/// Chunks à (re)mesher cette frame. La pose/casse et le streaming ne meshent
/// pas eux-mêmes : ils notent ici, et [`remesh_dirty`] meshe une fois par
/// chunk, en fin de frame. Un seul système crée les entités-chunk : deux
/// systèmes ne peuvent plus spawner chacun la sienne pour le même chunk
/// (une entité spawnée via `Commands` n'est visible des `Query` qu'à la
/// frame suivante — chacun aurait cru qu'elle n'existait pas).
#[derive(Resource, Default)]
pub struct DirtyChunks(pub HashSet<ChunkPos>);

/// Meshes en cours dans des tâches (#53, design doc §6), par lot : un lot
/// réunit les meshes lancés à la même image. Il est appliqué d'un coup quand
/// toutes ses tâches ont fini, et les lots dans l'ordre : deux chunks
/// voisins changent à la même image, et un vieux mesh n'écrase jamais un
/// plus récent.
#[derive(Resource, Default)]
pub struct Meshing(VecDeque<Vec<(ChunkPos, Task<[MeshData; 2]>)>>);

impl Meshing {
    pub fn contains(&self, pos: ChunkPos) -> bool {
        self.0.iter().flatten().any(|(p, _)| *p == pos)
    }

    /// Abandonne les meshes de `pos` en cours (chunk déchargé) : appliqués,
    /// ils recréeraient son entité.
    pub fn forget(&mut self, pos: ChunkPos) {
        for batch in &mut self.0 {
            batch.retain(|(p, _)| *p != pos);
        }
    }
}

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

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Moteur Voxel".into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins((
            player::PlayerPlugin,
            effects::EffectsPlugin,
            sky::SkyPlugin,
            FrameTimeDiagnosticsPlugin::default(),
            MaterialPlugin::<VoxelMaterial>::default(),
        ))
        .init_resource::<DirtyChunks>()
        .init_resource::<Meshing>()
        .init_resource::<streaming::Generating>()
        .init_resource::<inventory::Inventory>()
        .init_resource::<items::StorageChanged>()
        .init_resource::<content::ReloadStatus>()
        .init_resource::<content::Incoming>()
        .init_resource::<generate::Generation>()
        .insert_resource(Time::<Fixed>::from_hz(f64::from(TICKS_PER_SECOND)))
        .add_systems(
            Startup,
            (setup_world, player::spawn_player, save::restore_player, save::restore_items, save::restore_mobs, inventory::spawn_hotbar).chain(),
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
                entities::add_mob_models,
                entities::light_grid_lit,
                inventory::scroll_selection,
                // Raccourcis coupés pendant la saisie d'un texte.
                (
                    inventory::give_all_blocks,
                    inventory::toggle_craft_menu,
                    generate::toggle_generate_window,
                )
                    .run_if(ui::not_typing),
                content::apply_content.before(remesh_dirty),
                ui::type_text,
                generate::start_generation,
                generate::finish_generation.before(content::apply_content),
                generate::show_status.after(generate::finish_generation),
                inventory::fill_craft_menu.after(inventory::toggle_craft_menu),
                inventory::click_recipe,
                ui::close_on_escape,
                inventory::update_hotbar.after(inventory::scroll_selection),
                save::save_edited_chunks.after(interact::interact),
            ),
        )
        .add_systems(
            Last,
            (save::save_player_and_items, save::finish_chunk_writes)
                .after(bevy::window::ExitSystems),
        )
        .add_systems(
            FixedUpdate,
            (items::simulate_items.after(player::physics_step), interact::tick_blocks),
        );
    // Diagnostics toutes les 5 s dans la console : images, entités, CPU et
    // mémoire, temps GPU par passe (DX12 et Vulkan seulement).
    #[cfg(feature = "profile")]
    app.add_plugins((
        bevy::diagnostic::LogDiagnosticsPlugin {
            wait_duration: std::time::Duration::from_secs(5),
            ..default()
        },
        bevy::diagnostic::EntityCountDiagnosticsPlugin::default(),
        bevy::diagnostic::SystemInformationDiagnosticsPlugin,
        bevy::render::diagnostic::RenderDiagnosticsPlugin,
    ));
    app.run();
}

fn setup_world(
    mut commands: Commands,
    mut materials: ResMut<Assets<VoxelMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    // --- Le monde possède son contenu (§3.1) : tout part du registre, lu
    //     depuis un fichier de données — aucun bloc n'est défini en Rust.
    //     Save (§3.10) : un monde existant impose ses IDs, sa seed et sa
    //     résolution ; le contenu du jeu y est fusionné (`content::load`).
    //     Une save illisible arrête le jeu : elle n'est jamais écrasée. ---
    let save = Save::new(base_dir().join("saves/world"));
    let refuse = |err: String| -> ! { panic!("save {} refusée : {err}", save.dir().display()) };
    let (base, meta) = match save.read_meta().unwrap_or_else(|e| refuse(e)) {
        Some(meta) => (save.read_registry().unwrap_or_else(|e| refuse(e)), meta),
        None => (
            Registry::new(),
            WorldMeta { format: FORMAT_VERSION, seed: 42, voxels_per_meter: 1.0, chunk_size: CHUNK_SIZE },
        ),
    };
    let registry = content::load(base).unwrap_or_else(|e| panic!("contenu invalide : {e}"));
    let array = images.add(texture_array(&registry));
    save.write_world(&meta, &registry).unwrap_or_else(|e| refuse(e));
    // Worldgen en donnée (#27), world-owned (§3.10) : celui de la save s'il
    // y est, sinon celui du jeu, copié dans la save. Aucun bloc nommé ici.
    let worldgen_text = match save.read_copy("worldgen.ron").unwrap_or_else(|e| refuse(e)) {
        Some(text) => text,
        None => {
            let path = assets_dir().join("content/worldgen.ron");
            let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} : {e}", path.display()));
            save.write_copy("worldgen.ron", &text).unwrap_or_else(|e| refuse(e));
            text
        }
    };
    let generator = Worldgen::from_ron(&worldgen_text, meta.seed, &registry)
        .unwrap_or_else(|e| panic!("worldgen invalide : {e}"));
    // Ciel (#30) : comme le registre, le fichier du jeu prime (une retouche
    // s'applique aux mondes existants) et la save en garde une copie ; elle
    // ne sert qu'à un monde dont le jeu n'a pas de ciel.
    let sky_path = assets_dir().join("content/sky.ron");
    let sky_text = match std::fs::read_to_string(&sky_path) {
        Ok(text) => {
            save.write_copy("sky.ron", &text).unwrap_or_else(|e| refuse(e));
            text
        }
        Err(e) => save
            .read_copy("sky.ron")
            .unwrap_or_else(|e| refuse(e))
            .unwrap_or_else(|| panic!("{} : {e}", sky_path.display())),
    };
    let sky = Sky::from_ron(&sky_text).unwrap_or_else(|e| panic!("ciel invalide : {e}"));
    let air = generator.terrain.air;

    // --- Le monde démarre VIDE : c'est le streaming (Update) qui génère et
    //     meshe les chunks autour du joueur, dès la première frame. La
    //     physique se fige tant que le sol sous le joueur n'est pas chargé.
    let mut world = VoxelWorld::new(registry, meta.chunk_size, meta.voxels_per_meter);
    world.set_tick(save.read_clock().unwrap_or_else(|e| refuse(e)).unwrap_or(0));
    // La couleur vient des sommets et des textures. Mask : un pixel
    // transparent est jeté (feuilles, verre, #22). Blend : mélangé à ce
    // qui est derrière (eau, #28).
    // shortcut: tout le chunk passe par le test d'alpha, opaque compris ;
    // séparer un mesh opaque si le profilage (#41) le montre (#47).
    let mut chunk_material = |alpha_mode| {
        materials.add(ExtendedMaterial {
            base: StandardMaterial { alpha_mode, ..StandardMaterial::from(Color::WHITE) },
            extension: BlockTextures { array: array.clone(), daylight: 1.0 },
        })
    };
    let material = chunk_material(AlphaMode::Mask(0.5));
    let blend_material = chunk_material(AlphaMode::Blend);

    commands.insert_resource(sky::WorldSky(sky));
    commands.insert_resource(save::ChunkWriter::new(save.clone()));
    commands.insert_resource(save::WorldSave(save));
    commands.insert_resource(GameWorld {
        world,
        generator,
        air,
        material,
        blend_material,
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
        Text::new("Clic gauche : jouer · Échap : libérer la souris · F : plein écran\nWASD/Espace : bouger · gauche : casser · droit : utiliser/poser · Maj+droit : poser · molette : bloc · C : fabriquer (menu) · I : générer un bloc (IA) · R : recharger le contenu · G : un de chaque bloc (debug) · F1-F4 : effets"),
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
}

/// (Re)meshe les chunks notés dans [`DirtyChunks`] — l'unique chemin de
/// meshing (génération, streaming, pose/casse). Copie chaque chunk avec sa
/// bordure et le meshe dans une tâche ; applique les lots de [`Meshing`]
/// finis. Chaque chunk a deux entités créées ensemble ([`ChunkMesh`],
/// [`BlendMesh`]), chacune avec son mesh quand il n'est pas vide.
pub fn remesh_dirty(
    mut commands: Commands,
    mut game: ResMut<GameWorld>,
    mut dirty: ResMut<DirtyChunks>,
    mut meshing: ResMut<Meshing>,
    mut meshes: ResMut<Assets<Mesh>>,
    chunk_meshes: Query<(Entity, &ChunkMesh, Option<&Mesh3d>)>,
    blend_meshes: Query<(Entity, &BlendMesh, Option<&Mesh3d>)>,
) {
    let existing: HashMap<ChunkPos, (Entity, Option<&Mesh3d>)> =
        chunk_meshes.iter().map(|(e, cm, m)| (cm.0, (e, m))).collect();
    // Lumière changée (#23) : seuls les chunks affichés ou en cours de
    // meshing sont re-meshés, les autres le seront quand le streaming les
    // affichera.
    let lit = game.world.take_light_changed();
    dirty.0.extend(lit.into_iter().filter(|pos| existing.contains_key(pos) || meshing.contains(*pos)));
    let voxel_size_m = 1.0 / game.world.voxels_per_meter();
    let batch: Vec<_> = dirty
        .0
        .drain()
        .filter_map(|pos| {
            let region = Region::copy(&game.world, pos)?; // pas de données : rien à mesher
            Some((pos, AsyncComputeTaskPool::get().spawn(async move { region.mesh(voxel_size_m) })))
        })
        .collect();
    if !batch.is_empty() {
        meshing.0.push_back(batch);
    }

    let blend: HashMap<ChunkPos, (Entity, Option<&Mesh3d>)> =
        blend_meshes.iter().map(|(e, b, m)| (b.0, (e, m))).collect();
    let extent = game.world.chunk_size() as f32 * voxel_size_m;
    // Le plus récent par chunk : deux lots finis à la même image ne doivent
    // pas créer deux entités pour le même chunk.
    let mut ready = HashMap::new();
    while meshing.0.front().is_some_and(|batch| batch.iter().all(|(_, task)| task.is_finished())) {
        ready.extend(meshing.0.pop_front().unwrap().into_iter().map(|(pos, task)| (pos, block_on(task))));
    }
    for (pos, [solid, transparent]) in ready {
        let at = Transform::from_xyz(pos.x as f32 * extent, pos.y as f32 * extent, pos.z as f32 * extent);
        let (entity, mesh) =
            existing.get(&pos).copied().unwrap_or_else(|| (commands.spawn((ChunkMesh(pos), at)).id(), None));
        // Le semi-transparent (l'eau) ne projette pas d'ombre (#51).
        let (blend_entity, blend_mesh) =
            blend.get(&pos).copied().unwrap_or_else(|| (commands.spawn((BlendMesh(pos), at, NotShadowCaster)).id(), None));
        put_mesh(&mut commands, &mut meshes, (entity, mesh), solid, &game.material);
        put_mesh(&mut commands, &mut meshes, (blend_entity, blend_mesh), transparent, &game.blend_material);
    }
}

/// Donne le mesh `data` à l'entité : remplace le contenu de l'asset existant
/// (l'entité et son handle ne bougent pas, le GPU reçoit les nouveaux
/// tampons), l'ajoute, ou le retire si `data` est vide.
fn put_mesh(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    (entity, current): (Entity, Option<&Mesh3d>),
    data: MeshData,
    material: &Handle<VoxelMaterial>,
) {
    match (current, data.is_empty()) {
        (Some(mesh3d), false) => {
            if let Err(err) = meshes.insert(mesh3d.id(), to_bevy_mesh(data)) {
                error!("re-mesh impossible : {err}");
            }
        }
        (Some(mesh3d), true) => {
            meshes.remove(mesh3d.id());
            commands.entity(entity).remove::<(Mesh3d, MeshMaterial3d<VoxelMaterial>)>();
        }
        (None, false) => {
            commands
                .entity(entity)
                .insert((Mesh3d(meshes.add(to_bevy_mesh(data))), MeshMaterial3d(material.clone())));
        }
        (None, true) => {}
    }
}

/// HUD debug : FPS (moyenne glissante de Bevy) et nombre de chunks qui ont
/// une entité — la zone chargée par le streaming.
fn update_debug_text(
    diagnostics: Res<DiagnosticsStore>,
    chunks: Query<(), With<ChunkMesh>>,
    reload: Res<content::ReloadStatus>,
    effects: Res<effects::Effects>,
    hour: Res<sky::Hour>,
    mut text: Query<&mut Text, With<DebugText>>,
) {
    let Ok(mut text) = text.single_mut() else { return };
    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|d| d.smoothed())
        .unwrap_or(0.0);
    let (h, min) = (hour.0 as u32, (hour.0.fract() * 60.0) as u32);
    text.0 = format!(
        "{fps:.0} FPS · {} chunks · {h:02} h {min:02} (F5 : +1 h)\n{}\n{}",
        chunks.iter().count(),
        effects.summary(),
        reload.0
    );
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
pub(crate) fn to_bevy_mesh(data: MeshData) -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, data.positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, data.normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, data.colors)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, data.uvs)
    // La couche passe par le 2ᵉ jeu d'UV. -1 : pas de texture.
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_UV_1,
        data.layers
            .iter()
            .map(|&l| [if l == NO_TEXTURE { -1.0 } else { l as f32 }, 0.0])
            .collect::<Vec<[f32; 2]>>(),
    )
    .with_inserted_attribute(ATTRIBUTE_LIGHT, data.lights)
    .with_inserted_indices(Indices::U32(data.indices))
}
