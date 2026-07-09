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

use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, MonitorSelection, PrimaryWindow, WindowMode};

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
            )
            .add_systems(Update, toggle_fullscreen);
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

/// État du regard souris entre deux frames (voir [`mouse_look`]).
#[derive(Default)]
struct LookState {
    /// Dernière position lue — le delta se mesure entre deux lectures.
    last_position: Option<Vec2>,
    /// Saut de warp attendu (cible − position d'émission) et nombre de
    /// frames restantes avant d'abandonner sa détection.
    pending_jump: Option<(Vec2, u8)>,
    /// Alternance ±½ px de la cible de warp (contournement du cache winit).
    warp_parity: bool,
    /// Le recentrage du curseur fonctionne-t-il ici ? (résolu à la première
    /// frame : faux sous WSL, où le curseur hôte gagne toujours).
    recenter_works: Option<bool>,
}

/// Frames pendant lesquelles on guette l'écho d'un warp. Généreux : un écho
/// arrivé après le délai serait compté comme un coup de caméra.
const WARP_ECHO_FRAMES: u8 = 30;

/// Sous WSLg, le pointeur affiché est le curseur *Windows* de l'hôte : les
/// warps X11 ne le déplacent pas (l'état interne se fait re-écraser au
/// mouvement suivant → à-coups) et le masquage est ignoré. Mesuré au
/// diagnostic du 2026-07-09 — voir docs/journal.md. Dans ce cas : pas de
/// recentrage du tout, on s'appuie sur le confinement (qui, lui, marche) et
/// le plein écran (F11) pour donner de l'amplitude au regard.
fn recentering_works() -> bool {
    std::fs::read_to_string("/proc/version")
        .map(|v| !v.to_lowercase().contains("microsoft"))
        .unwrap_or(true)
}

/// Regard souris — frame variable, uniquement en mode FPS.
///
/// Contraintes mesurées sous WSLg (voir journal) : les deltas « raw » sont
/// inutilisables (périphérique absolu émulé — valeurs ~1000× trop grandes),
/// et les warps de recentrage atterrissent en retard ou jamais.
///
/// Le schéma qui tient malgré ça :
/// - delta = différence entre deux **positions successives** du curseur —
///   chaque mouvement n'est compté qu'une fois, quelle que soit la latence ;
/// - le curseur n'est recentré qu'en approche du bord de la fenêtre ;
/// - le mouvement n'est JAMAIS avalé pendant qu'un warp est en vol : quand
///   l'écho du warp atterrit, on reconnaît sa signature (saut ~colinéaire
///   au warp émis, amplitude comparable) et on **soustrait ce saut** du
///   delta de la frame — un warp perdu ne coûte alors rien du tout.
///
/// Piège bevy_winit au passage : un warp vers une cible égale au précédent
/// warp est silencieusement ignoré (comparaison au cache de la *demande*,
/// pas à l'état réel) — d'où la cible alternée d'un demi-pixel.
fn mouse_look(
    captured: Res<CursorCaptured>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut player_query: Query<&mut Player>,
    mut camera_query: Query<&mut Transform, With<PlayerCamera>>,
    mut state: Local<LookState>,
) {
    if !captured.0 {
        state.last_position = None;
        state.pending_jump = None;
        return;
    }
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    let recenter = *state
        .recenter_works
        .get_or_insert_with(recentering_works);
    let center = Vec2::new(window.width() / 2.0, window.height() / 2.0);

    let Some(position) = window.cursor_position() else {
        // Curseur hors fenêtre : on le rapatrie si possible, et on repartira
        // de zéro (pas de delta calculable à travers la sortie).
        if recenter && state.pending_jump.is_none() {
            let target = warp_target(&mut state, center);
            window.set_cursor_position(Some(target));
            state.pending_jump = Some((Vec2::ZERO, WARP_ECHO_FRAMES));
        }
        state.last_position = None;
        return;
    };

    let Some(last) = state.last_position else {
        state.last_position = Some(position);
        return;
    };
    let mut delta = position - last;
    state.last_position = Some(position);

    // Écho de warp attendu ? S'il est dans ce delta, on l'en retire ; le
    // reste du delta est du vrai mouvement et compte normalement.
    if let Some((jump, frames_left)) = state.pending_jump {
        let denom = jump.length_squared();
        if denom > 1.0 && delta.dot(jump) / denom > 0.6 {
            delta -= jump;
            state.pending_jump = None;
        } else if frames_left == 0 {
            state.pending_jump = None;
        } else {
            state.pending_jump = Some((jump, frames_left - 1));
        }
    }

    if delta != Vec2::ZERO {
        for mut player in &mut player_query {
            player.yaw -= delta.x * MOUSE_SENSITIVITY;
            player.pitch = (player.pitch - delta.y * MOUSE_SENSITIVITY)
                .clamp(-1.54, 1.54); // ±88° : jamais tout à fait la verticale
            for mut camera in &mut camera_query {
                camera.rotation = Quat::from_rotation_x(player.pitch);
            }
        }
    }

    // Recentre seulement près du bord (au-delà du quart de la fenêtre), et
    // jamais deux warps en vol à la fois. Jamais sous WSL (à-coups garantis).
    let offset = position - center;
    if recenter
        && state.pending_jump.is_none()
        && (offset.x.abs() > window.width() * 0.25 || offset.y.abs() > window.height() * 0.25)
    {
        let target = warp_target(&mut state, center);
        window.set_cursor_position(Some(target));
        state.pending_jump = Some((target - position, WARP_ECHO_FRAMES));
    }
}

/// F11 : bascule fenêtré ↔ plein écran sans bordure. Sous WSLg (pas de
/// recentrage possible), le plein écran donne au regard l'amplitude de
/// l'écran entier avant de buter au bord.
fn toggle_fullscreen(
    keys: Res<ButtonInput<KeyCode>>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if !keys.just_pressed(KeyCode::F11) {
        return;
    }
    if let Ok(mut window) = windows.single_mut() {
        window.mode = match window.mode {
            WindowMode::Windowed => {
                WindowMode::BorderlessFullscreen(MonitorSelection::Current)
            }
            _ => WindowMode::Windowed,
        };
    }
}

/// Cible de recentrage, alternée d'un demi-pixel pour ne jamais être égale
/// au warp précédent (sinon bevy_winit l'ignore — comparaison à son cache).
fn warp_target(state: &mut LookState, center: Vec2) -> Vec2 {
    state.warp_parity = !state.warp_parity;
    center + Vec2::new(if state.warp_parity { 0.5 } else { -0.5 }, 0.0)
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
        // `Confined` et pas `Locked` : c'est `mouse_look` qui recentre le
        // curseur lui-même — le lock émulé par warp de winit entrerait en
        // conflit avec ce recentrage. Peut échouer (WSLg…) : bevy_winit
        // loggue et remet grab_mode à None — pas grave, `CursorCaptured`
        // reste notre source de vérité. Pas de warp ici : son écho ferait un
        // à-coup de caméra ; `mouse_look` recentre si besoin, proprement.
        options.grab_mode = CursorGrabMode::Confined;
        options.visible = false;
    }
    if keys.just_pressed(KeyCode::Escape) && captured.0 {
        captured.0 = false;
        options.grab_mode = CursorGrabMode::None;
        options.visible = true;
    }
}
