//! Jauges du joueur (#33) : au tick fixe, leurs barres, la mort. Les jauges
//! et leurs règles viennent de `survival.ron` (voir `voxel_core::survival`) ;
//! ce module ne connaît aucune jauge par son nom.

use bevy::prelude::*;

use voxel_core::save::Save;
use voxel_core::survival::{Show, Survival, Vitals};

use crate::inventory::Inventory;
use crate::items::DroppedItem;
use crate::player::{Player, EYE_HEIGHT_M, PLAYER_HEIGHT_M, PLAYER_WIDTH_M};
use crate::sky::WorldSky;
use crate::ui::{self, GameWindow};
use crate::{assets_dir, GameWorld};

/// Le contenu de `survival.ron`.
#[derive(Resource, Default)]
pub struct SurvivalRules(pub Survival);

/// Les jauges du joueur.
#[derive(Component)]
pub struct PlayerVitals(pub Vitals);

/// Comme le ciel : le fichier du jeu prime et la save en garde une copie,
/// qui ne sert qu'à un monde dont le jeu n'a pas de `survival.ron`. Sans
/// l'un ni l'autre : aucune jauge, le joueur ne meurt pas.
pub fn load(save: &Save) -> Result<Survival, String> {
    let path = assets_dir().join("content/survival.ron");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => {
            save.write_copy("survival.ron", &text)?;
            Some(text)
        }
        Err(_) => save.read_copy("survival.ron")?,
    };
    text.map_or(Ok(Survival::default()), |t| Survival::from_ron(&t))
}

/// Un tick des jauges, après le pas de physique. À la mort : inventaire
/// lâché sur place (sauf `keep_inventory`), retour au départ du monde,
/// jauges à leur départ.
// Paramètres de système Bevy : c'est la forme normale (cf. `interact`).
#[allow(clippy::too_many_arguments)]
pub fn step_vitals(
    mut commands: Commands,
    time: Res<Time>,
    rules: Res<SurvivalRules>,
    game: Res<GameWorld>,
    sky: Option<Res<WorldSky>>,
    mut inventory: ResMut<Inventory>,
    mut player: Query<(&mut Player, &mut PlayerVitals)>,
    opened: Query<Entity, With<GameWindow>>,
) {
    let world = &game.world;
    let Ok((mut player, mut vitals)) = player.single_mut() else { return };
    // Figé comme la physique tant que le sol n'est pas chargé.
    if world.chunk(world.split(world.voxel_at_m(player.body().feet)).0).is_none() {
        return;
    }
    let daylight = sky.map_or(1.0, |s| s.0.daylight(&s.0.at(s.0.hour(world.tick()))));
    let size = [PLAYER_WIDTH_M, PLAYER_HEIGHT_M];
    let dt = time.delta_secs(); // dans FixedUpdate : le pas fixe
    if !vitals.0.step(&rules.0, world, player.body(), size, EYE_HEIGHT_M, daylight, dt) {
        return;
    }
    let keep = rules.0.death.as_ref().is_some_and(|d| d.keep_inventory);
    if !keep {
        let (slots, _) = inventory.to_save();
        inventory.restore(Vec::new(), 0);
        let center = player.feet() + Vec3::Y * PLAYER_HEIGHT_M / 2.0;
        for (id, count) in slots {
            for _ in 0..count {
                commands.spawn(DroppedItem::new(id, center));
            }
        }
    }
    let (yaw, pitch) = player.look();
    let start = crate::player::spawn_point(&game);
    *player = Player::restored(start, yaw, pitch);
    vitals.0 = Vitals::new(&rules.0);
    let window = ui::open(&mut commands, &opened, "Vous êtes mort");
    let what = if keep { "inventaire gardé" } else { "inventaire lâché là où tu es mort" };
    commands.entity(window).with_children(|w| {
        w.spawn(Text::new(format!("Retour au départ du monde, {what}.")));
    });
}

/// Une barre de jauge : son fond, et l'indice de la jauge.
#[derive(Component)]
pub struct GaugeBar(usize);

/// Le remplissage d'une barre.
#[derive(Component)]
pub struct GaugeFill(usize);

/// Les barres des jauges qui en déclarent une, dans l'ordre du fichier,
/// au-dessus de la barre d'inventaire.
pub fn spawn_bars(mut commands: Commands, rules: Res<SurvivalRules>) {
    commands
        .spawn(Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(100.0),
            width: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            column_gap: Val::Px(8.0),
            ..default()
        })
        .with_children(|row| {
            for (i, gauge) in rules.0.gauges.iter().enumerate() {
                let Some(bar) = gauge.bar else { continue };
                let [r, g, b] = bar.color;
                row.spawn((
                    GaugeBar(i),
                    Node { width: Val::Px(160.0), height: Val::Px(10.0), ..default() },
                    BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.5)),
                ))
                .with_child((
                    GaugeFill(i),
                    Node { width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
                    BackgroundColor(Color::srgb(r, g, b)),
                ));
            }
        });
}

pub fn update_bars(
    rules: Res<SurvivalRules>,
    player: Query<&PlayerVitals>,
    mut bars: Query<(&GaugeBar, &mut Node), Without<GaugeFill>>,
    mut fills: Query<(&GaugeFill, &mut Node), Without<GaugeBar>>,
) {
    let Ok(vitals) = player.single() else { return };
    let fraction = |i: usize| vitals.0.values[i] / rules.0.gauges[i].max;
    for (fill, mut node) in &mut fills {
        node.width = Val::Percent(100.0 * fraction(fill.0));
    }
    for (bar, mut node) in &mut bars {
        let hidden = rules.0.gauges[bar.0].bar.is_some_and(|b| b.show == Show::NotFull) && fraction(bar.0) >= 1.0;
        node.display = if hidden { Display::None } else { Display::Flex };
    }
}
