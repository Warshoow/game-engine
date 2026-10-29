//! Effets de rendu (#51, design doc §6) : ombres du soleil, brouillard de
//! distance, brouillard en volume (rayons de lumière), bloom. Réglages de la
//! machine, jamais de la save. Une touche par effet, pour la mesure
//! avant/après ; l'état s'affiche dans le HUD de debug.

use bevy::camera::Hdr;
use bevy::light::{CascadeShadowConfigBuilder, FogVolume, VolumetricFog, VolumetricLight};
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::render::renderer::RenderAdapterInfo;

use crate::player::PlayerCamera;
use crate::streaming::{VERTICAL_VIEW_M, VIEW_DISTANCE_M};

/// Couleur du ciel : fond de l'image et brouillard (le vrai ciel attend
/// #30).
const SKY: Color = Color::srgb(0.62, 0.76, 0.92);

/// Densité du brouillard en volume : un léger voile, ~80 % de la lumière
/// passe à la distance de vue (Bevy : absorption + diffusion = 0,6 par
/// unité de densité et par mètre). Le ciel n'a rien derrière lui : chaque
/// rayon du ciel traverse tout le volume, d'où une densité faible. Réglé à
/// l'estime, à revoir sur la 4060.
const HAZE_DENSITY: f32 = 0.004;

/// Les effets actifs. Touches F1 à F4.
#[derive(Resource)]
pub struct Effects {
    pub shadows: bool,
    pub fog: bool,
    pub volumetric: bool,
    pub bloom: bool,
}

impl Default for Effects {
    fn default() -> Self {
        Self { shadows: true, fog: true, volumetric: true, bloom: true }
    }
}

impl Effects {
    /// Pour le HUD de debug.
    pub fn summary(&self) -> String {
        let on = |b: bool| if b { "oui" } else { "non" };
        format!(
            "F1 ombres : {} · F2 brouillard : {} · F3 rayons : {} · F4 bloom : {}",
            on(self.shadows),
            on(self.fog),
            on(self.volumetric),
            on(self.bloom)
        )
    }
}

pub struct EffectsPlugin;

impl Plugin for EffectsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ClearColor(SKY))
            .init_resource::<Effects>()
            .add_systems(Startup, (spawn_sun, off_without_gpu))
            .add_systems(
                Update,
                (toggle_effects.run_if(crate::ui::not_typing), apply_effects, follow_camera).chain(),
            );
    }
}

/// Le soleil : sa lumière passe par celle du ciel de chaque sommet (#23,
/// `voxel.wgsl`), même au-delà de la carte d'ombre. Cascades jusqu'à la
/// distance de vue.
fn spawn_sun(mut commands: Commands) {
    commands.spawn((
        DirectionalLight::default(),
        VolumetricLight,
        CascadeShadowConfigBuilder {
            num_cascades: 3,
            first_cascade_far_bound: 12.0,
            maximum_distance: VIEW_DISTANCE_M,
            ..default()
        }
        .build(),
        Transform::from_xyz(50.0, 80.0, 30.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn(FogVolume { fog_color: SKY, density_factor: HAZE_DENSITY, ..default() });
}

/// Rendu logiciel (llvmpipe sous WSL, PC sans carte graphique) : tout
/// coupé, chaque image y coûte déjà ~8 ms.
fn off_without_gpu(adapter: Option<Res<RenderAdapterInfo>>, mut effects: ResMut<Effects>) {
    // `DeviceType` vient de wgpu, que Bevy ne ré-exporte pas : son nom suffit.
    if adapter.is_some_and(|a| format!("{:?}", a.device_type) == "Cpu") {
        *effects = Effects { shadows: false, fog: false, volumetric: false, bloom: false };
        info!("rendu logiciel : effets coupés (F1 à F4 pour les activer)");
    }
}

fn toggle_effects(keys: Res<ButtonInput<KeyCode>>, mut effects: ResMut<Effects>) {
    if keys.just_pressed(KeyCode::F1) {
        effects.shadows = !effects.shadows;
    }
    if keys.just_pressed(KeyCode::F2) {
        effects.fog = !effects.fog;
    }
    if keys.just_pressed(KeyCode::F3) {
        effects.volumetric = !effects.volumetric;
    }
    if keys.just_pressed(KeyCode::F4) {
        effects.bloom = !effects.bloom;
    }
}

/// Applique [`Effects`] au soleil et à la caméra, quand ils changent ou
/// quand la caméra apparaît.
fn apply_effects(
    mut commands: Commands,
    effects: Res<Effects>,
    cameras: Query<Entity, With<PlayerCamera>>,
    added: Query<(), Added<PlayerCamera>>,
    mut sun: Query<&mut DirectionalLight>,
) {
    if !effects.is_changed() && added.is_empty() {
        return;
    }
    for mut light in &mut sun {
        light.shadow_maps_enabled = effects.shadows;
    }
    for camera in &cameras {
        let mut camera = commands.entity(camera);
        if effects.fog {
            // Finit à la distance de vue : cache le bord du monde chargé.
            camera.insert(DistanceFog {
                color: SKY,
                falloff: FogFalloff::Linear { start: VIEW_DISTANCE_M * 0.5, end: VIEW_DISTANCE_M },
                ..default()
            });
        } else {
            camera.remove::<DistanceFog>();
        }
        if effects.volumetric {
            camera.insert(VolumetricFog::default());
        } else {
            camera.remove::<VolumetricFog>();
        }
        if effects.bloom {
            camera.insert(Bloom::NATURAL);
        } else {
            camera.remove::<(Bloom, Hdr)>();
        }
    }
}

/// Le volume de brouillard (un cube) suit la caméra et couvre la zone vue.
fn follow_camera(camera: Query<&GlobalTransform, With<PlayerCamera>>, mut volume: Query<&mut Transform, With<FogVolume>>) {
    let (Ok(camera), Ok(mut volume)) = (camera.single(), volume.single_mut()) else { return };
    volume.translation = camera.translation();
    volume.scale = Vec3::new(2.0 * VIEW_DISTANCE_M, 2.0 * VERTICAL_VIEW_M, 2.0 * VIEW_DISTANCE_M);
}
