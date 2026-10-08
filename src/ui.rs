//! Fenêtres de jeu (#45) : la base commune, avant d'en multiplier.
//!
//! Une fenêtre est une entité UI marquée [`GameWindow`]. **Une seule à la
//! fois** ; tant qu'elle existe, la souris est libre (`cursor_grab`,
//! player.rs) et le joueur ne bouge, ne vise ni n'interagit plus — tous ces
//! systèmes lisent [`CursorCaptured`]. Échap la ferme.
//!
//! Ce qu'une fenêtre affiche vient du registre (§3.11) : la brique
//! [`slot`] tire couleur et quantité de l'entrée, jamais d'une liste en dur.

use bevy::prelude::*;

use voxel_core::registry::{ContentId, Registry};

use crate::inventory;
#[cfg(doc)]
use crate::player::CursorCaptured;

/// Racine de la fenêtre ouverte.
#[derive(Component)]
pub struct GameWindow;

/// Ouvre une fenêtre titrée au centre de l'écran, en fermant celle déjà
/// ouverte. Rend la racine, à remplir par l'appelant.
pub fn open(commands: &mut Commands, opened: &Query<Entity, With<GameWindow>>, title: &str) -> Entity {
    for window in opened {
        commands.entity(window).despawn();
    }
    commands
        .spawn((
            GameWindow,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(50.0),
                top: Val::Percent(50.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(8.0),
                padding: UiRect::all(Val::Px(16.0)),
                ..default()
            },
            // Centrée : décalée de la moitié de sa propre taille.
            UiTransform::from_translation(Val2::percent(-50.0, -50.0)),
            BackgroundColor(Color::srgba(0.1, 0.1, 0.1, 0.9)),
            children![(Text::new(format!("{title}   (Échap : fermer)")), TextFont::from_font_size(18.0))],
        ))
        .id()
}

/// Échap ferme la fenêtre ouverte.
pub fn close_on_escape(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    opened: Query<Entity, With<GameWindow>>,
) {
    if keys.just_pressed(KeyCode::Escape) {
        for window in &opened {
            commands.entity(window).despawn();
        }
    }
}

/// Case d'item : couleur de l'entrée, quantité en bas à droite, bordure au
/// choix (blanche = choisie). Partagée par la barre et les fenêtres.
pub fn slot(registry: &Registry, id: ContentId, count: u32, border: Color) -> impl Bundle {
    (
        Node {
            width: Val::Px(44.0),
            height: Val::Px(44.0),
            border: UiRect::all(Val::Px(3.0)),
            justify_content: JustifyContent::End,
            align_items: AlignItems::End,
            ..default()
        },
        BackgroundColor(inventory::color(registry, id)),
        BorderColor::all(border),
        children![(Text::new(count.to_string()), TextFont::from_font_size(14.0))],
    )
}
