//! Jour et nuit (#30, design doc §6). L'heure vient de l'horloge du monde,
//! le ciel et les lumières de `sky.ron` (voir `voxel_core::sky`). Rien n'est
//! re-meshé : la lumière du ciel des sommets multiplie déjà le soleil et
//! l'ambiance que ce module règle.

use bevy::light::FogVolume;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{DistanceFog, MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;
use voxel_core::sky::{light_direction, sun_direction, Sky};

use crate::player::PlayerCamera;
use crate::{GameWorld, VoxelMaterial};

/// Rayon de la sphère du ciel : au-delà de ce qui est chargé (96 m), en
/// deçà du plan lointain de la caméra (1 000 m).
const SKY_RADIUS_M: f32 = 400.0;

/// Le ciel du monde, lu depuis sa save.
#[derive(Resource)]
pub struct WorldSky(pub Sky);

/// L'heure courante, pour le HUD de debug.
#[derive(Resource, Default)]
pub struct Hour(pub f32);

/// Heures ajoutées par la touche de debug (F5). Jamais sauvées : aucune
/// règle ne lit encore l'heure (#57).
#[derive(Resource, Default)]
struct HourShift(f32);

#[derive(Clone, Copy, Default, Debug, Reflect, ShaderType)]
struct SkyUniform {
    /// Couleurs en linéaire.
    zenith: Vec4,
    horizon: Vec4,
    /// xyz : direction vers le soleil ; w : force du jour, dans [0, 1].
    sun: Vec4,
    sun_color: Vec4,
}

/// Le matériau de la sphère du ciel (`sky.wgsl`).
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
struct SkyMaterial {
    #[uniform(0)]
    sky: SkyUniform,
}

impl Material for SkyMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/sky.wgsl".into()
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    // La caméra est dans la sphère : on en voit l'intérieur.
    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

#[derive(Component)]
struct SkyDome;

pub struct SkyPlugin;

impl Plugin for SkyPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<SkyMaterial>::default())
            .init_resource::<ClearColor>()
            .init_resource::<Hour>()
            .init_resource::<HourShift>()
            .add_systems(Startup, spawn_dome)
            .add_systems(Update, (shift_hour.run_if(crate::ui::not_typing), update_sky).chain());
    }
}

fn spawn_dome(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<SkyMaterial>>) {
    commands.spawn((
        SkyDome,
        Mesh3d(meshes.add(Sphere::new(SKY_RADIUS_M).mesh().uv(32, 16))),
        MeshMaterial3d(materials.add(SkyMaterial::default())),
    ));
}

fn shift_hour(keys: Res<ButtonInput<KeyCode>>, mut shift: ResMut<HourShift>) {
    if keys.just_pressed(KeyCode::F5) {
        shift.0 += 1.0;
    }
}

/// Règle soleil (ou lune), ambiance, ciel et brouillards sur l'heure.
#[allow(clippy::too_many_arguments)]
fn update_sky(
    sky: Res<WorldSky>,
    game: Res<GameWorld>,
    shift: Res<HourShift>,
    mut hour: ResMut<Hour>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut clear: ResMut<ClearColor>,
    mut sun: Query<(&mut DirectionalLight, &mut Transform), Without<SkyDome>>,
    mut dome: Query<(&MeshMaterial3d<SkyMaterial>, &mut Transform), Without<DirectionalLight>>,
    camera: Query<&GlobalTransform, With<PlayerCamera>>,
    mut fog: Query<&mut DistanceFog>,
    mut volume: Query<&mut FogVolume>,
    mut sky_materials: ResMut<Assets<SkyMaterial>>,
    mut voxel_materials: ResMut<Assets<VoxelMaterial>>,
    mut last_daylight: Local<Option<f32>>,
) {
    hour.0 = (sky.0.hour(game.world.tick()) + shift.0).rem_euclid(24.0);
    let moment = sky.0.at(hour.0);
    let daylight = sky.0.daylight(&moment);
    let srgb = |c: [f32; 3]| Color::srgb(c[0], c[1], c[2]);
    let (horizon, light_color) = (srgb(moment.horizon), srgb(moment.light_color));

    for (mut light, mut transform) in &mut sun {
        light.color = light_color;
        light.illuminance = moment.light;
        // La lumière va de l'astre vers le sol.
        *transform = Transform::IDENTITY.looking_to(-Vec3::from(light_direction(hour.0)), Vec3::Y);
    }
    ambient.color = light_color;
    ambient.brightness = moment.ambient;
    clear.0 = horizon;
    for mut fog in &mut fog {
        fog.color = horizon;
    }
    for mut volume in &mut volume {
        volume.fog_color = horizon;
    }

    if let Ok((material, mut transform)) = dome.single_mut() {
        if let Ok(camera) = camera.single() {
            transform.translation = camera.translation();
        }
        if let Some(mut material) = sky_materials.get_mut(&material.0) {
            let linear = |c: Color| Vec4::from_array(c.to_linear().to_f32_array());
            material.sky = SkyUniform {
                zenith: linear(srgb(moment.zenith)),
                horizon: linear(horizon),
                sun: Vec3::from(sun_direction(hour.0)).extend(daylight),
                sun_color: linear(light_color),
            };
        }
    }

    // Un matériau modifié est renvoyé au GPU : seulement quand ça se voit.
    if last_daylight.is_none_or(|last| (last - daylight).abs() > 0.01) {
        *last_daylight = Some(daylight);
        for handle in [&game.material, &game.blend_material] {
            if let Some(mut material) = voxel_materials.get_mut(handle) {
                material.extension.daylight = daylight;
            }
        }
    }
}
