//! Contrôleur première personne — critère §7.4 de la slice.
//!
//! Répartition des rôles, dictée par §3.9 (tick fixe / frame variable) :
//! - **`FixedUpdate`** : la simulation — intention de mouvement, gravité,
//!   saut, collision (`voxel_core::physics`). Déterministe, cadence fixe.
//! - **`Update`** : ce qui est purement visuel/input — regard souris,
//!   capture du curseur, et le `Transform` affiché (voir
//!   [`smooth_transform`]). La rotation de caméra n'influence la simu qu'au
//!   tick suivant, via le yaw stocké sur le joueur.
//!
//! Toutes les grandeurs sont en **mètres** (§2) : taille du joueur, vitesse,
//! gravité. Aucun « nombre de blocs » ici.

use std::sync::LazyLock;

use bevy::input::mouse::AccumulatedMouseMotion;
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
pub const GRAVITY_M_S2: f32 = 22.0;
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
                (
                    mouse_look,
                    smooth_transform,
                    crate::interact::interact,
                    cursor_grab,
                )
                    .chain(),
            )
            .add_systems(
                Update,
                (
                    toggle_fullscreen,
                    // Après la pose/casse : le contour voit déjà le monde modifié.
                    crate::interact::highlight_target.after(crate::interact::interact),
                ),
            );
    }
}

/// L'état simulation du joueur. `feet` = position des **pieds** (centre de
/// la boîte au sol). Le `Transform` n'est que l'affichage, interpolé entre
/// `prev_feet` et `feet` par [`smooth_transform`] : la simu ne le lit pas.
#[derive(Component, Default)]
pub struct Player {
    feet: Vec3,
    /// `feet` au tick précédent.
    prev_feet: Vec3,
    velocity: Vec3,
    yaw: f32,
    pitch: f32,
    grounded: bool,
}

impl Player {
    pub fn at(feet: Vec3) -> Self {
        Self { feet, prev_feet: feet, ..default() }
    }

    /// Position simulée des pieds (≠ `Transform`, qui est interpolé).
    pub fn feet(&self) -> Vec3 {
        self.feet
    }

    /// Joueur relu dans une save.
    pub fn restored(feet: Vec3, yaw: f32, pitch: f32) -> Self {
        Self { yaw, pitch, ..Self::at(feet) }
    }

    /// (yaw, pitch), en radians.
    pub fn look(&self) -> (f32, f32) {
        (self.yaw, self.pitch)
    }
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
    let feet = Vec3::new(0.5, game.generator.height_m(0.5, 0.5) + 1.0, 0.5);
    commands
        .spawn((
            Player::at(feet),
            Transform::from_translation(feet),
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
pub fn physics_step(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    captured: Res<CursorCaptured>,
    game: Res<GameWorld>,
    mut query: Query<&mut Player>,
) {
    // Hors mode FPS (souris libérée, fenêtre ouverte) : les touches ne
    // pilotent plus le joueur, mais la gravité continue.
    let keys_on = |key| captured.0 && keys.pressed(key);
    let dt = time.delta_secs(); // dans FixedUpdate : le pas fixe
    for mut player in &mut query {
        player.prev_feet = player.feet;
        // Streaming : on ne simule PAS dans du non-chargé. `is_solid` traite
        // un chunk absent comme de l'air — sans cette garde, le joueur
        // tomberait à travers un monde pas encore généré (premières frames,
        // ou si la génération ne suit pas). Figé ≠ cassé : la simu reprend
        // dès que le sol existe.
        let feet_voxel = game.world.voxel_at_m(player.feet.to_array());
        let (chunk_pos, _) = game.world.split(feet_voxel);
        if game.world.chunk(chunk_pos).is_none() {
            continue;
        }
        // Intention de déplacement dans le plan horizontal, repère joueur.
        let mut wish = Vec3::ZERO;
        if keys_on(KeyCode::KeyW) {
            wish.z -= 1.0;
        }
        if keys_on(KeyCode::KeyS) {
            wish.z += 1.0;
        }
        if keys_on(KeyCode::KeyA) {
            wish.x -= 1.0;
        }
        if keys_on(KeyCode::KeyD) {
            wish.x += 1.0;
        }
        // Tournée par le yaw seul : regarder le sol ne ralentit pas la marche.
        let dir = (Quat::from_rotation_y(player.yaw) * wish).normalize_or_zero();

        // Vitesse horizontale directe (pas d'inertie — style Minecraft),
        // verticale intégrée (gravité + saut).
        player.velocity.x = dir.x * WALK_SPEED_M_S;
        player.velocity.z = dir.z * WALK_SPEED_M_S;
        player.velocity.y -= GRAVITY_M_S2 * dt;
        if player.grounded && keys_on(KeyCode::Space) {
            player.velocity.y = JUMP_SPEED_M_S;
        }

        let aabb = Aabb::from_feet(player.feet.to_array(), PLAYER_WIDTH_M, PLAYER_HEIGHT_M);
        let moved = move_and_collide(&game.world, aabb, (player.velocity * dt).to_array());

        // Un impact vertical annule la vitesse verticale ; vers le bas, il
        // signifie « au sol » (le saut ne s'arme que là).
        if moved.collided[1] {
            player.grounded = player.velocity.y < 0.0;
            player.velocity.y = 0.0;
        } else {
            player.grounded = false;
        }

        player.feet = Vec3::from_array(moved.aabb.feet());
    }
}

/// Le `Transform` affiché, recalculé à chaque frame.
///
/// La simu avance par ticks de 1/64 s ; une frame tombe en général entre
/// deux ticks. Afficher la position du dernier tick fait avancer le joueur
/// par à-coups (une frame rattrape 1 tick, la suivante 2…). On affiche
/// plutôt la position **interpolée** entre les deux derniers ticks, selon
/// le temps écoulé depuis le dernier (`overstep_fraction`, 0 → 1). Coût :
/// l'affichage a jusqu'à un tick de retard. La simu n'est pas touchée
/// (déterminisme, §2).
///
/// Le yaw est appliqué ici aussi, pas au tick : sinon tourner la tête
/// saccaderait à 64 Hz alors que le pitch suit chaque frame.
fn smooth_transform(fixed: Res<Time<Fixed>>, mut query: Query<(&mut Transform, &Player)>) {
    let t = fixed.overstep_fraction();
    for (mut transform, player) in &mut query {
        transform.translation = player.prev_feet.lerp(player.feet, t);
        transform.rotation = Quat::from_rotation_y(player.yaw);
    }
}

/// Sous WSLg, les deltas « raw » de la souris sont inutilisables
/// (périphérique absolu émulé — valeurs ~1000× trop grandes) et le curseur
/// affiché est celui de *Windows* : ni warp ni verrouillage n'y ont d'effet.
/// Mesuré au diagnostic du 2026-07-09 — voir docs/journal.md. Lu une fois.
static ON_WSL: LazyLock<bool> = LazyLock::new(|| {
    std::fs::read_to_string("/proc/version")
        .is_ok_and(|v| v.to_lowercase().contains("microsoft"))
});

/// Regard souris — frame variable, uniquement en mode FPS.
///
/// - **Hors WSL** : le déplacement relatif fourni par Bevy
///   (`AccumulatedMouseMotion`), curseur verrouillé par [`cursor_grab`].
/// - **Sous WSL** : delta = différence entre deux **positions successives**
///   du curseur (confiné à la fenêtre). Le regard bute au bord de la
///   fenêtre — d'où le plein écran (touche F) pour lui donner l'amplitude
///   de l'écran entier.
fn mouse_look(
    captured: Res<CursorCaptured>,
    motion: Res<AccumulatedMouseMotion>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut player_query: Query<&mut Player>,
    mut camera_query: Query<&mut Transform, With<PlayerCamera>>,
    mut last_position: Local<Option<Vec2>>,
) {
    if !captured.0 {
        *last_position = None;
        return;
    }
    let delta = if *ON_WSL {
        let Some(position) = windows.single().ok().and_then(Window::cursor_position) else {
            // Curseur hors fenêtre : pas de delta calculable à travers la
            // sortie, on repartira de la prochaine position lue.
            *last_position = None;
            return;
        };
        let delta = last_position.map_or(Vec2::ZERO, |last| position - last);
        *last_position = Some(position);
        delta
    } else {
        motion.delta
    };

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
}

/// F : bascule fenêtré ↔ plein écran sans bordure. Sous WSLg (curseur
/// confiné, pas de verrouillage), le plein écran donne au regard l'amplitude de
/// l'écran entier avant de buter au bord. (Lettre plutôt que F11 : les
/// touches de fonction sont souvent interceptées par l'hôte/le terminal.)
fn toggle_fullscreen(
    keys: Res<ButtonInput<KeyCode>>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if !keys.just_pressed(KeyCode::KeyF) {
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

/// Clic gauche : passe en mode FPS (et demande le grab OS). Échap ou une
/// fenêtre de jeu ouverte (`ui.rs`) : sort, et le clic ne recapture pas
/// tant que la fenêtre est là (il sert à cliquer dedans).
fn cursor_grab(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut captured: ResMut<CursorCaptured>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
    windows: Query<(), With<crate::ui::GameWindow>>,
) {
    let Ok(mut options) = cursor.single_mut() else {
        return;
    };
    let window_open = !windows.is_empty();
    if mouse.just_pressed(MouseButton::Left) && !captured.0 && !window_open {
        captured.0 = true;
        // Hors WSL : `Locked` (bevy_winit retombe sur `Confined` là où le
        // lock n'existe pas, X11). Sous WSL, `mouse_look` lit la position du
        // curseur : il doit bouger, donc seulement confiné. Peut échouer
        // (WSLg…) : bevy_winit loggue et remet grab_mode à None — pas grave,
        // `CursorCaptured` reste notre source de vérité.
        options.grab_mode = if *ON_WSL {
            CursorGrabMode::Confined
        } else {
            CursorGrabMode::Locked
        };
        options.visible = false;
    }
    if (keys.just_pressed(KeyCode::Escape) || window_open) && captured.0 {
        captured.0 = false;
        options.grab_mode = CursorGrabMode::None;
        options.visible = true;
    }
}
