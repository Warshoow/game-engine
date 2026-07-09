//! Contrôleur première personne — critère §7.4 de la slice.
//!
//! Répartition des rôles, dictée par §3.9 (tick fixe / frame variable) :
//! - **`FixedUpdate`** : la simulation — intention de mouvement, gravité,
//!   saut, collision (`voxel_core::physics`). Déterministe, cadence fixe.
//! - **`Update`** : ce qui est purement visuel/input — regard souris,
//!   capture du curseur. La rotation de caméra n'influence la simu qu'au
//!   tick suivant, via le yaw stocké sur le joueur.
//!
//! Toutes les grandeurs sont en **mètres** (§2) : taille du joueur, vitesse,
//! gravité. Aucun « nombre de blocs » ici.

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use voxel_core::physics::{move_and_collide, Aabb};

use crate::GameWorld;

// Gabarit et dynamique du joueur — en mètres et mètres/seconde.
pub const PLAYER_WIDTH_M: f32 = 0.6;
pub const PLAYER_HEIGHT_M: f32 = 1.8;
const EYE_HEIGHT_M: f32 = 1.62;
const WALK_SPEED_M_S: f32 = 5.0;
/// Plus fort que 9,81 : la gravité « réaliste » donne un saut flottant en
/// jeu — la quasi-totalité des jeux la gonflent.
const GRAVITY_M_S2: f32 = 22.0;
const JUMP_SPEED_M_S: f32 = 7.5;
const MOUSE_SENSITIVITY: f32 = 0.002;

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        // `interact` avant `cursor_grab` : le clic qui capture le curseur ne
        // doit pas aussi casser un bloc (interact ne voit pas encore le grab).
        app.init_resource::<CursorCaptured>()
            .add_systems(FixedUpdate, physics_step)
            .add_systems(
                Update,
                (mouse_look, crate::interact::interact, cursor_grab).chain(),
            );
    }
}

/// L'état simulation du joueur. La translation du `Transform` est la
/// position des **pieds** (centre de la boîte au sol).
#[derive(Component)]
pub struct Player {
    velocity: Vec3,
    yaw: f32,
    pitch: f32,
    grounded: bool,
}

#[derive(Component)]
pub struct PlayerCamera;

/// « Mode FPS » voulu par le joueur — découplé de l'état réel du grab OS.
///
/// Sous certains compositeurs (WSLg notamment), le pointer lock échoue et
/// `bevy_winit` remet `CursorOptions::grab_mode` à `None` — si le regard et
/// les clics étaient conditionnés à cet état, tout resterait mort. Les
/// mouvements *relatifs* de souris arrivent même sans lock : on suit donc
/// notre intention à nous, et le grab OS n'est qu'un confort en plus.
#[derive(Resource, Default)]
pub struct CursorCaptured(pub bool);

pub fn spawn_player(mut commands: Commands, game: Res<GameWorld>) {
    // Spawn posé sur le terrain, interrogé en mètres — jamais en blocs.
    let ground = game.generator.height_m(0.5, 0.5);
    commands
        .spawn((
            Player {
                velocity: Vec3::ZERO,
                yaw: 0.0,
                pitch: 0.0,
                grounded: false,
            },
            Transform::from_xyz(0.5, ground + 1.0, 0.5),
            Visibility::default(),
        ))
        .with_children(|parent| {
            parent.spawn((
                PlayerCamera,
                Camera3d::default(),
                Transform::from_xyz(0.0, EYE_HEIGHT_M, 0.0),
            ));
        });
}

/// Simulation du mouvement — tick fixe (§3.9).
fn physics_step(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    game: Res<GameWorld>,
    mut query: Query<(&mut Transform, &mut Player)>,
) {
    let dt = time.delta_secs(); // dans FixedUpdate : le pas fixe
    for (mut transform, mut player) in &mut query {
        // Intention de déplacement dans le plan horizontal, repère joueur.
        let mut wish = Vec3::ZERO;
        if keys.pressed(KeyCode::KeyW) {
            wish.z -= 1.0;
        }
        if keys.pressed(KeyCode::KeyS) {
            wish.z += 1.0;
        }
        if keys.pressed(KeyCode::KeyA) {
            wish.x -= 1.0;
        }
        if keys.pressed(KeyCode::KeyD) {
            wish.x += 1.0;
        }
        // Tournée par le yaw seul : regarder le sol ne ralentit pas la marche.
        let dir = (Quat::from_rotation_y(player.yaw) * wish).normalize_or_zero();

        // Vitesse horizontale directe (pas d'inertie — style Minecraft),
        // verticale intégrée (gravité + saut).
        player.velocity.x = dir.x * WALK_SPEED_M_S;
        player.velocity.z = dir.z * WALK_SPEED_M_S;
        player.velocity.y -= GRAVITY_M_S2 * dt;
        if player.grounded && keys.pressed(KeyCode::Space) {
            player.velocity.y = JUMP_SPEED_M_S;
        }

        let feet = transform.translation;
        let aabb = Aabb::from_feet(feet.to_array(), PLAYER_WIDTH_M, PLAYER_HEIGHT_M);
        let moved = move_and_collide(&game.world, aabb, (player.velocity * dt).to_array());

        // Un impact vertical annule la vitesse verticale ; vers le bas, il
        // signifie « au sol » (le saut ne s'arme que là).
        if moved.collided[1] {
            player.grounded = player.velocity.y < 0.0;
            player.velocity.y = 0.0;
        } else {
            player.grounded = false;
        }

        transform.translation = Vec3::from_array(moved.aabb.feet());
        transform.rotation = Quat::from_rotation_y(player.yaw);
    }
}

/// Regard souris — frame variable, uniquement en mode FPS.
fn mouse_look(
    motion: Res<AccumulatedMouseMotion>,
    captured: Res<CursorCaptured>,
    mut player_query: Query<&mut Player>,
    mut camera_query: Query<&mut Transform, With<PlayerCamera>>,
) {
    if !captured.0 || motion.delta == Vec2::ZERO {
        return;
    }
    for mut player in &mut player_query {
        player.yaw -= motion.delta.x * MOUSE_SENSITIVITY;
        player.pitch = (player.pitch - motion.delta.y * MOUSE_SENSITIVITY)
            .clamp(-1.54, 1.54); // ±88° : jamais tout à fait la verticale
        for mut camera in &mut camera_query {
            camera.rotation = Quat::from_rotation_x(player.pitch);
        }
    }
}

/// Clic gauche : passe en mode FPS (et demande le grab OS). Échap : sort.
fn cursor_grab(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut captured: ResMut<CursorCaptured>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    let Ok(mut options) = cursor.single_mut() else {
        return;
    };
    if mouse.just_pressed(MouseButton::Left) && !captured.0 {
        captured.0 = true;
        // Peut échouer (WSLg…) : bevy_winit loggue et remet grab_mode à
        // None — pas grave, `CursorCaptured` reste notre source de vérité.
        options.grab_mode = CursorGrabMode::Locked;
        options.visible = false;
    }
    if keys.just_pressed(KeyCode::Escape) && captured.0 {
        captured.0 = false;
        options.grab_mode = CursorGrabMode::None;
        options.visible = true;
    }
}
