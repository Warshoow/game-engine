//! Inventaire du joueur et barre d'inventaire (jalon 1, #9).
//!
//! L'inventaire tient des `ContentId` du registre : n'importe quelle entrée
//! (un bloc s'y range lui-même, sans item « double »). Une pile par entrée,
//! dans l'ordre de première obtention, sans limite de taille.
//!
//! La barre est **réflexive du registre** (§3.11) : couleur et nom viennent
//! de l'entrée, l'UI ne connaît aucun bloc.

use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;

use voxel_core::registry::{ContentId, Kind, Registry};

use crate::player::CursorCaptured;
use crate::{GameWorld, HeldBlockText};

#[derive(Resource, Default)]
pub struct Inventory {
    slots: Vec<(ContentId, u32)>,
    selected: usize,
}

impl Inventory {
    pub fn add(&mut self, id: ContentId) {
        match self.slots.iter_mut().find(|(c, _)| *c == id) {
            Some((_, n)) => *n += 1,
            None => self.slots.push((id, 1)),
        }
    }

    /// L'entrée en main, s'il y en a une.
    pub fn selected(&self) -> Option<ContentId> {
        self.slots.get(self.selected).map(|&(id, _)| id)
    }

    /// Retire un exemplaire de l'entrée en main. Une pile vide disparaît ;
    /// la sélection reste sur une case existante.
    pub fn take_selected(&mut self) {
        let Some((_, n)) = self.slots.get_mut(self.selected) else { return };
        *n -= 1;
        if *n == 0 {
            self.slots.remove(self.selected);
            self.selected = self.selected.min(self.slots.len().saturating_sub(1));
        }
    }

    /// Décale la sélection, cyclique (`step` = ±1).
    pub fn scroll(&mut self, step: isize) {
        let n = self.slots.len() as isize;
        if n > 0 {
            self.selected = (self.selected as isize + step).rem_euclid(n) as usize;
        }
    }
}

/// Molette : change l'entrée en main.
pub fn scroll_selection(
    mut wheel: MessageReader<MouseWheel>,
    captured: Res<CursorCaptured>,
    mut inventory: ResMut<Inventory>,
) {
    // Somme des crans de la frame (trackpads : plusieurs petits événements).
    let scroll: f32 = wheel.read().map(|w| w.y).sum();
    if captured.0 && scroll != 0.0 {
        inventory.scroll(if scroll > 0.0 { 1 } else { -1 });
    }
}

/// Racine UI de la barre d'inventaire (bas de l'écran, centrée).
#[derive(Component)]
pub struct HotbarRoot;

pub fn spawn_hotbar(mut commands: Commands) {
    commands.spawn((
        HotbarRoot,
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(48.0),
            width: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            column_gap: Val::Px(6.0),
            ..default()
        },
    ));
}

/// Reconstruit la barre quand l'inventaire change (rare : ramassage, pose,
/// molette) plutôt que de la synchroniser case par case.
pub fn update_hotbar(
    mut commands: Commands,
    inventory: Res<Inventory>,
    game: Res<GameWorld>,
    root: Query<Entity, With<HotbarRoot>>,
    mut held_text: Query<&mut Text, With<HeldBlockText>>,
) {
    if !inventory.is_changed() {
        return;
    }
    let registry = &game.world.registry;
    if let Ok(mut text) = held_text.single_mut() {
        text.0 = match inventory.selected() {
            Some(id) => format!("en main : {}", identifier(registry, id)),
            None => "inventaire vide : casse des blocs".to_string(),
        };
    }
    let Ok(root) = root.single() else { return };
    commands.entity(root).despawn_children().with_children(|bar| {
        for (i, &(id, count)) in inventory.slots.iter().enumerate() {
            let border = if i == inventory.selected { Color::WHITE } else { Color::BLACK };
            bar.spawn((
                Node {
                    width: Val::Px(44.0),
                    height: Val::Px(44.0),
                    border: UiRect::all(Val::Px(3.0)),
                    justify_content: JustifyContent::End,
                    align_items: AlignItems::End,
                    ..default()
                },
                BackgroundColor(color(registry, id)),
                BorderColor::all(border),
            ))
            .with_child((Text::new(count.to_string()), TextFont::from_font_size(14.0)));
        }
    });
}

fn identifier(registry: &Registry, id: ContentId) -> &str {
    registry.get(id).map_or("???", |e| e.identifier.as_str())
}

/// Couleur d'affichage d'une entrée : celle du bloc, gris sinon (un item
/// n'a pas encore d'apparence au registre).
pub fn color(registry: &Registry, id: ContentId) -> Color {
    match registry.get(id).map(|e| &e.kind) {
        Some(Kind::Block(b)) => Color::srgb(b.color[0], b.color[1], b.color[2]),
        _ => Color::srgb(0.5, 0.5, 0.5),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: ContentId = ContentId(1);
    const B: ContentId = ContentId(2);

    #[test]
    fn stacks_take_and_selection_stay_consistent() {
        let mut inv = Inventory::default();
        assert_eq!(inv.selected(), None);
        inv.take_selected(); // vide : sans effet

        inv.add(A);
        inv.add(B);
        inv.add(A);
        assert_eq!(inv.slots, vec![(A, 2), (B, 1)]);

        inv.scroll(1);
        assert_eq!(inv.selected(), Some(B));
        inv.take_selected(); // B épuisé : la case disparaît, sélection ramenée sur A
        assert_eq!(inv.slots, vec![(A, 2)]);
        assert_eq!(inv.selected(), Some(A));

        inv.scroll(-1); // cyclique sur une seule case
        assert_eq!(inv.selected(), Some(A));
        inv.take_selected();
        inv.take_selected();
        assert_eq!(inv.selected(), None);
    }
}
