//! Console de debug (#19, design doc §3.11) : T ouvre une fenêtre de jeu
//! avec un champ de texte ; Entrée exécute la ligne, la réponse s'affiche
//! dessous. Les noms (entrées, jauges) viennent du registre et de
//! `survival.ron`, jamais d'une liste en dur. Reportés : #67.

use bevy::prelude::*;

use crate::entities;
use crate::inventory::Inventory;
use crate::player::Player;
use crate::sky::{HourShift, WorldSky};
use crate::survival::{PlayerVitals, SurvivalRules};
use crate::ui::{self, GameWindow, TextField};
use crate::GameWorld;

pub const HELP: &str = "/tp x y z · /give <id> [nombre] · /gauge <nom> <valeur> · /time <heure> · /mob <id> · /help";

/// Une ligne lue, pas encore exécutée.
#[derive(Debug, PartialEq)]
pub enum Command {
    /// Pieds du joueur, en mètres.
    Tp([f32; 3]),
    Give(String, u32),
    Gauge(String, f32),
    /// Heure du jour, en heures.
    Time(f32),
    Mob(String),
    Help,
}

const MAX_GIVE: u32 = 999;

/// Lit une ligne de console. L'erreur est le message à afficher.
pub fn parse(line: &str) -> Result<Command, String> {
    let mut words = line.split_whitespace();
    let name = words.next().ok_or_else(|| HELP.to_string())?;
    let args: Vec<&str> = words.collect();
    let number = |s: &str| s.parse::<f32>().ok().filter(|v| v.is_finite()).ok_or(format!("nombre attendu : « {s} »"));
    match (name, args.as_slice()) {
        ("/tp", [x, y, z]) => Ok(Command::Tp([number(x)?, number(y)?, number(z)?])),
        ("/give", [id]) => Ok(Command::Give(id.to_string(), 1)),
        ("/give", [id, n]) => match n.parse::<u32>() {
            Ok(n @ 1..=MAX_GIVE) => Ok(Command::Give(id.to_string(), n)),
            _ => Err(format!("nombre entre 1 et {MAX_GIVE} attendu : « {n} »")),
        },
        ("/gauge", [gauge, value]) => Ok(Command::Gauge(gauge.to_string(), number(value)?)),
        ("/time", [hour]) => Ok(Command::Time(number(hour)?)),
        ("/mob", [id]) => Ok(Command::Mob(id.to_string())),
        ("/help", []) => Ok(Command::Help),
        ("/tp" | "/give" | "/gauge" | "/time" | "/mob" | "/help", _) => Err(format!("arguments invalides · {HELP}")),
        _ => Err(format!("commande inconnue : « {name} » · {HELP}")),
    }
}

#[derive(Component)]
pub struct ConsoleWindow;

/// La réponse à la dernière commande.
#[derive(Component)]
pub struct Reply;

/// Touche T : ouvre la console.
pub fn open_console(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    opened: Query<Entity, With<GameWindow>>,
) {
    if !keys.just_pressed(KeyCode::KeyT) {
        return;
    }
    let window = ui::open(&mut commands, &opened, "Console");
    commands.entity(window).insert(ConsoleWindow).with_children(|w| {
        w.spawn((TextField::default(), Text::default(), Node { min_width: Val::Px(500.0), ..default() }));
        w.spawn((Reply, Text::new(HELP)));
    });
}

/// Entrée dans le champ de la console : exécute la ligne.
// Paramètres de système Bevy : c'est la forme normale (cf. `interact`).
#[allow(clippy::too_many_arguments)]
pub fn run_command(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    game: Res<GameWorld>,
    rules: Res<SurvivalRules>,
    sky: Option<Res<WorldSky>>,
    mut shift: ResMut<HourShift>,
    mut inventory: ResMut<Inventory>,
    mut player: Query<(&mut Player, &mut PlayerVitals)>,
    console: Query<(), With<ConsoleWindow>>,
    mut field: Query<&mut TextField>,
    mut reply: Query<&mut Text, With<Reply>>,
) {
    if console.is_empty() || !keys.just_pressed(KeyCode::Enter) {
        return;
    }
    let (Ok(mut field), Ok(mut reply), Ok((mut player, mut vitals))) = (field.single_mut(), reply.single_mut(), player.single_mut())
    else {
        return;
    };
    let registry = &game.world.registry;
    let answer = match parse(&field.0) {
        Err(e) => e,
        Ok(Command::Help) => HELP.to_string(),
        Ok(Command::Tp(feet)) => {
            let (yaw, pitch) = player.look();
            *player = Player::restored(Vec3::from_array(feet), yaw, pitch);
            vitals.0.teleported(feet[1]);
            format!("téléporté en {:.1} {:.1} {:.1}", feet[0], feet[1], feet[2])
        }
        Ok(Command::Give(id, n)) => match registry.lookup(&id) {
            Some(content) => {
                (0..n).for_each(|_| inventory.add(content));
                format!("{n} × {id} donnés")
            }
            None => format!("entrée inconnue : « {id} »"),
        },
        Ok(Command::Gauge(name, value)) => match rules.0.index(&name) {
            Some(i) => {
                let value = value.clamp(0.0, rules.0.gauges[i].max);
                vitals.0.values[i] = value;
                format!("{name} = {value}")
            }
            None => format!("jauge inconnue : « {name} »"),
        },
        Ok(Command::Time(hour)) => match sky {
            Some(sky) => {
                shift.0 = hour - sky.0.hour(game.world.tick());
                format!("il est {hour} h (non sauvé)")
            }
            None => "pas de ciel dans ce monde".to_string(),
        },
        Ok(Command::Mob(id)) => match registry.lookup(&id).filter(|&c| registry.get(c).is_some_and(|e| e.entity().is_some())) {
            Some(content) => {
                let (yaw, _) = player.look();
                // 3 m devant, dans le sens du regard, tourné comme le joueur.
                let feet = player.feet() + Vec3::new(-yaw.sin(), 0.0, -yaw.cos()) * 3.0;
                entities::spawn_mob(&mut commands, content, feet, yaw, false);
                format!("{id} posé")
            }
            None => format!("pas une entité : « {id} »"),
        },
    };
    reply.0 = answer;
    field.0.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_each_command() {
        assert_eq!(parse("/tp 1 2.5 -3"), Ok(Command::Tp([1.0, 2.5, -3.0])));
        assert_eq!(parse("  /give core:bread  "), Ok(Command::Give("core:bread".into(), 1)));
        assert_eq!(parse("/give core:bread 5"), Ok(Command::Give("core:bread".into(), 5)));
        assert_eq!(parse("/gauge health 3"), Ok(Command::Gauge("health".into(), 3.0)));
        assert_eq!(parse("/time 21.5"), Ok(Command::Time(21.5)));
        assert_eq!(parse("/mob core:pig"), Ok(Command::Mob("core:pig".into())));
        assert_eq!(parse("/help"), Ok(Command::Help));
    }

    #[test]
    fn refuses_bad_lines() {
        for line in ["", "tp 1 2 3", "/fly", "/tp 1 2", "/tp 1 x 3", "/tp NaN 0 0", "/give a 0", "/give a 1000", "/give a -1", "/help me"] {
            assert!(parse(line).is_err(), "{line}");
        }
    }
}
