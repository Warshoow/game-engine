//! Fenêtres de jeu (#45) : la base commune, avant d'en multiplier.
//!
//! Une fenêtre est une entité UI marquée [`GameWindow`]. **Une seule à la
//! fois** ; tant qu'elle existe, la souris est libre (`cursor_grab`,
//! player.rs) et le joueur ne bouge, ne vise ni n'interagit plus — tous ces
//! systèmes lisent [`CursorCaptured`]. Échap la ferme.
//!
//! Ce qu'une fenêtre affiche vient du registre (§3.11) : la brique
//! [`slot`] tire icône et quantité de l'entrée, jamais d'une liste en dur.

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use voxel_core::icon::icon;
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

/// Champ de saisie : le texte tapé au clavier, affiché par son [`Text`].
/// Tant qu'il existe, les touches de raccourci (C, R, G, I, T, F) sont
/// coupées — voir [`not_typing`] (R : dans `content::apply_content`).
#[derive(Component, Default)]
pub struct TextField(pub String);

/// Condition de système : aucun champ de saisie ouvert.
pub fn not_typing(fields: Query<(), With<TextField>>) -> bool {
    fields.is_empty()
}

/// Tape dans le champ ouvert : caractères, retour arrière. Entrée est lue
/// par la fenêtre qui porte le champ.
pub fn type_text(mut keys: MessageReader<KeyboardInput>, mut fields: Query<(&mut TextField, &mut Text)>) {
    let Ok((mut field, mut text)) = fields.single_mut() else {
        keys.clear();
        return;
    };
    for key in keys.read().filter(|k| k.state == ButtonState::Pressed) {
        match (&key.logical_key, &key.text) {
            (Key::Backspace, _) => {
                field.0.pop();
            }
            (_, Some(typed)) => field.0.extend(typed.chars().filter(|c| !c.is_control())),
            _ => {}
        }
    }
    if field.is_changed() || text.0.is_empty() {
        text.0 = format!("{}_", field.0);
    }
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

/// Côté d'une icône en pixels : l'intérieur d'une case, pour un pixel
/// d'icône par pixel d'écran.
const ICON_PX: u32 = 38;

/// L'icône de chaque entrée, indexée par `ContentId` ; `None` sans
/// apparence (#70).
#[derive(Resource, Default)]
pub struct Icons(Vec<Option<Handle<Image>>>);

/// Calcule les icônes du registre (au chargement et à la touche R).
pub fn icons(registry: &Registry, images: &mut Assets<Image>) -> Icons {
    let size = Extent3d { width: ICON_PX, height: ICON_PX, depth_or_array_layers: 1 };
    Icons(
        registry
            .iter()
            .map(|(id, _)| {
                let rgba = icon(registry, id, ICON_PX)?;
                let image = Image::new(size, TextureDimension::D2, rgba, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD);
                Some(images.add(image))
            })
            .collect(),
    )
}

/// Case d'item : icône de l'entrée (sa couleur sans icône), quantité en bas
/// à droite, bordure au choix (blanche = choisie). Partagée par la barre et
/// les fenêtres.
pub fn slot(registry: &Registry, icons: &Icons, id: ContentId, count: u32, border: Color) -> impl Bundle {
    let icon = icons.0.get(id.0 as usize).cloned().flatten();
    let background = if icon.is_some() { Color::srgba(0.0, 0.0, 0.0, 0.3) } else { inventory::color(registry, id) };
    let image = ImageNode { image: icon.clone().unwrap_or_default(), color: if icon.is_some() { Color::WHITE } else { Color::NONE }, ..default() };
    (
        Node {
            width: Val::Px(44.0),
            height: Val::Px(44.0),
            border: UiRect::all(Val::Px(3.0)),
            justify_content: JustifyContent::End,
            align_items: AlignItems::End,
            ..default()
        },
        BackgroundColor(background),
        BorderColor::all(border),
        children![
            (image, Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() }),
            (Text::new(count.to_string()), TextFont::from_font_size(14.0)),
        ],
    )
}
