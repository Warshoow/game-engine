//! Inventaire du joueur et barre d'inventaire (jalon 1, #9).
//!
//! L'inventaire tient des `ContentId` du registre : n'importe quelle entrée
//! (un bloc s'y range lui-même, sans item « double »). Une pile par entrée,
//! dans l'ordre de première obtention, sans limite de taille. Après la
//! dernière pile, une case **main vide** (les règles `EmptyHand`, §3.6).
//!
//! La barre est **réflexive du registre** (§3.11) : couleur et nom viennent
//! de l'entrée, l'UI ne connaît aucun bloc.

use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;

use voxel_core::crafting;
use voxel_core::registry::{ContentId, Kind, Registry};

use crate::player::CursorCaptured;
use crate::ui;
use crate::{GameWorld, HeldBlockText};

#[derive(Resource, Default)]
pub struct Inventory {
    slots: Vec<(ContentId, u32)>,
    /// Dans `0..=slots.len()` ; `slots.len()` = la main vide.
    selected: usize,
}

impl Inventory {
    pub fn add(&mut self, id: ContentId) {
        match self.slots.iter_mut().find(|(c, _)| *c == id) {
            Some((_, n)) => *n += 1,
            None => {
                // Main vide choisie exprès : elle le reste (la nouvelle pile
                // s'insère avant elle). Inventaire vide : le premier item
                // ramassé passe en main.
                if self.selected == self.slots.len() && !self.slots.is_empty() {
                    self.selected += 1;
                }
                self.slots.push((id, 1));
            }
        }
    }

    /// L'entrée en main ; `None` = main vide.
    pub fn selected(&self) -> Option<ContentId> {
        self.slots.get(self.selected).map(|&(id, _)| id)
    }

    /// Retire un exemplaire de l'entrée en main. Une pile vide disparaît ;
    /// la sélection passe à la case suivante (au pire la main vide).
    pub fn take_selected(&mut self) {
        if let Some(id) = self.selected() {
            self.remove(id);
        }
    }

    /// Retire un exemplaire de `id`. Une pile vide disparaît ; la sélection
    /// reste sur la même entrée (ou passe à la suivante si c'était elle).
    pub fn remove(&mut self, id: ContentId) {
        let Some(i) = self.slots.iter().position(|&(c, _)| c == id) else { return };
        self.slots[i].1 -= 1;
        if self.slots[i].1 == 0 {
            self.slots.remove(i);
            if i < self.selected {
                self.selected -= 1;
            }
        }
    }

    /// Piles et case choisie, pour la save.
    pub fn to_save(&self) -> (Vec<(ContentId, u32)>, usize) {
        (self.slots.clone(), self.selected)
    }

    /// Inventaire relu dans une save.
    pub fn restore(&mut self, slots: Vec<(ContentId, u32)>, selected: usize) {
        self.selected = selected.min(slots.len());
        self.slots = slots;
    }

    /// Décale la sélection, cyclique sur les piles + la main vide (`step` = ±1).
    pub fn scroll(&mut self, step: isize) {
        let n = self.slots.len() as isize + 1;
        self.selected = (self.selected as isize + step).rem_euclid(n) as usize;
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

/// Menu de fabrication (#46), fenêtre de `ui.rs`.
#[derive(Component)]
pub struct CraftMenu;

/// Une ligne du menu : la `n`-ième recette sans station
/// (`crafting::without_station`, ordre du registre).
#[derive(Component)]
pub struct RecipeButton(usize);

/// Touche C : ouvre ou ferme le menu de fabrication.
pub fn toggle_craft_menu(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    opened: Query<Entity, With<ui::GameWindow>>,
    menu: Query<Entity, With<CraftMenu>>,
) {
    if !keys.just_pressed(KeyCode::KeyC) {
        return;
    }
    match menu.single() {
        Ok(menu) => {
            commands.entity(menu).despawn();
        }
        Err(_) => {
            let window = ui::open(&mut commands, &opened, "Fabrication");
            commands.entity(window).insert(CraftMenu);
        }
    }
}

/// Remplit le menu à son ouverture et quand l'inventaire change : une
/// ligne par recette sans station, grisée si les items manquent. Tout vient
/// du registre — une recette ajoutée au contenu apparaît sans code.
pub fn fill_craft_menu(
    mut commands: Commands,
    game: Res<GameWorld>,
    inventory: Res<Inventory>,
    menu: Query<(Entity, Ref<CraftMenu>)>,
    rows: Query<Entity, With<RecipeButton>>,
) {
    let Ok((menu, added)) = menu.single() else { return };
    if !added.is_added() && !inventory.is_changed() {
        return;
    }
    for row in &rows {
        commands.entity(row).despawn();
    }
    let registry = &game.world.registry;
    let recipes: Vec<_> = crafting::without_station(registry).collect();
    if recipes.is_empty() {
        commands.entity(menu).with_child((RecipeButton(0), Text::new("aucune recette sans station")));
        return;
    }
    commands.entity(menu).with_children(|list| {
        for (n, (product, recipe)) in recipes.into_iter().enumerate() {
            let makeable = crafting::inputs_from(registry, recipe, &inventory.slots).is_some();
            list.spawn((
                RecipeButton(n),
                Button,
                Node { column_gap: Val::Px(6.0), padding: UiRect::all(Val::Px(4.0)), align_items: AlignItems::Center, ..default() },
                BackgroundColor(if makeable { Color::srgb(0.25, 0.35, 0.25) } else { Color::srgb(0.2, 0.2, 0.2) }),
            ))
            .with_children(|row| {
                row.spawn(ui::slot(registry, product, recipe.count, Color::BLACK));
                row.spawn(Text::new(format!("{}  ←", identifier(registry, product))));
                // Entrées groupées par entrée, dans l'ordre de la recette.
                let mut inputs: Vec<(ContentId, u32)> = Vec::new();
                for id in recipe.inputs.iter().filter_map(|s| registry.lookup(s)) {
                    match inputs.iter_mut().find(|(c, _)| *c == id) {
                        Some((_, n)) => *n += 1,
                        None => inputs.push((id, 1)),
                    }
                }
                for (id, count) in inputs {
                    row.spawn(ui::slot(registry, id, count, Color::BLACK));
                }
            });
        }
    });
}

/// Clic sur une recette faisable : consomme ses entrées, ajoute le produit.
/// Le menu reste ouvert (l'inventaire change → il se remplit à nouveau).
pub fn click_recipe(
    game: Res<GameWorld>,
    mut inventory: ResMut<Inventory>,
    clicked: Query<(&Interaction, &RecipeButton), Changed<Interaction>>,
) {
    for (interaction, &RecipeButton(n)) in &clicked {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let registry = &game.world.registry;
        let Some((product, recipe)) = crafting::without_station(registry).nth(n) else { continue };
        let Some(used) = crafting::inputs_from(registry, recipe, &inventory.slots) else { continue };
        used.into_iter().for_each(|id| inventory.remove(id));
        (0..recipe.count).for_each(|_| inventory.add(product));
    }
}

/// Debug — touche G : un exemplaire de chaque bloc solide du registre, pour
/// essayer un contenu qui n'existe pas dans le monde généré. Découvert dans
/// le registre, aucune liste en dur.
pub fn give_all_blocks(
    keys: Res<ButtonInput<KeyCode>>,
    game: Res<GameWorld>,
    mut inventory: ResMut<Inventory>,
) {
    if !keys.just_pressed(KeyCode::KeyG) {
        return;
    }
    for (id, entry) in game.world.registry.iter() {
        if entry.block().is_some_and(|b| b.solid) {
            inventory.add(id);
        }
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
            None if inventory.slots.is_empty() => "inventaire vide : casse des blocs".to_string(),
            None => "main vide".to_string(),
        };
    }
    let Ok(root) = root.single() else { return };
    commands.entity(root).despawn_children().with_children(|bar| {
        for (i, &(id, count)) in inventory.slots.iter().enumerate() {
            let border = if i == inventory.selected { Color::WHITE } else { Color::BLACK };
            bar.spawn(ui::slot(registry, id, count, border));
        }
        // La case main vide, après les piles.
        let border = if inventory.selected == inventory.slots.len() { Color::WHITE } else { Color::BLACK };
        bar.spawn((
            Node {
                width: Val::Px(44.0),
                height: Val::Px(44.0),
                border: UiRect::all(Val::Px(3.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.3)),
            BorderColor::all(border),
        ));
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
        inv.take_selected(); // main vide : sans effet

        inv.add(A); // inventaire vide : le premier item passe en main
        assert_eq!(inv.selected(), Some(A));
        inv.add(B);
        inv.add(A);
        assert_eq!(inv.slots, vec![(A, 2), (B, 1)]);

        inv.scroll(1);
        assert_eq!(inv.selected(), Some(B));
        inv.take_selected(); // B épuisé : la case disparaît, on tombe sur la main vide
        assert_eq!(inv.slots, vec![(A, 2)]);
        assert_eq!(inv.selected(), None);

        inv.scroll(1); // cyclique : main vide → A
        assert_eq!(inv.selected(), Some(A));
        inv.scroll(-1);
        assert_eq!(inv.selected(), None);
    }

    #[test]
    fn removing_another_stack_keeps_the_selection() {
        let mut inv = Inventory::default();
        inv.add(A);
        inv.add(B);
        inv.scroll(1);
        inv.remove(A); // pile avant la sélection : B reste en main
        assert_eq!(inv.selected(), Some(B));
    }

    #[test]
    fn empty_hand_stays_empty_when_picking_up() {
        let mut inv = Inventory::default();
        inv.add(A);
        inv.scroll(1); // main vide choisie exprès
        inv.add(B);
        assert_eq!(inv.selected(), None);
        assert_eq!(inv.slots, vec![(A, 1), (B, 1)]);
    }
}
