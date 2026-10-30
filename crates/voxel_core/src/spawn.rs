//! Apparition et disparition des mobs (#60, design doc §3.6, « Apparition
//! des mobs ») : des règles dans l'entrée, le moteur les essaie près du
//! joueur.
//!
//! **Pourquoi des essais au hasard près du joueur** (à la Minecraft) : le
//! coût est fixe par tick, quelle que soit la taille du monde exploré, et
//! seule la zone que le joueur peut voir se peuple. La hauteur est tirée
//! elle aussi, puis on descend jusqu'au premier sol : un tirage en (x, z)
//! seul ne tomberait jamais dans une grotte.
//!
//! **Déterminisme** (§2) : le hasard dérive de la seed, du tick et du
//! numéro d'essai, jamais du nombre de mobs ; les mobs sont lus dans
//! l'ordre de leur id. Options écartées ou reportées : #62.

use serde::{Deserialize, Serialize};

use crate::mob::Mob;
use crate::path;
use crate::registry::ContentId;
use crate::world::VoxelWorld;
use crate::worldgen::hash64;

/// La zone d'apparition, à l'horizontale autour du joueur : pas sous ses
/// yeux, et pas au-delà de la distance de vue.
pub const SPAWN_MIN_M: f32 = 24.0;
pub const SPAWN_MAX_M: f32 = 96.0;
/// Hauteur tirée autour des pieds du joueur, de part et d'autre.
const SPAWN_HEIGHT_M: f32 = 16.0;
/// Descente au plus pour trouver un sol sous le point tiré.
const DROP_M: f32 = 24.0;
/// Essais par tick, toutes entrées confondues.
const ATTEMPTS_PER_TICK: u64 = 2;
/// La densité se compte dans ce rayon autour du point d'apparition.
const DENSITY_RADIUS_M: f32 = 64.0;
/// Écart au plus entre les membres d'un groupe et le premier.
const GROUP_SPREAD_M: f32 = 3.0;
/// Bornes de validation d'une règle.
const MAX_GROUP: u32 = 16;
const MAX_DESPAWN_M: f32 = 1024.0;
/// Sépare ce hasard de celui des mobs (`Mob::roll`).
const SALT: u64 = 0x5EA5_0B0D;

/// Une règle d'apparition de l'entrée. Distances en mètres.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct SpawnRule {
    /// Blocs sur lesquels le mob peut apparaître (sous ses pieds).
    pub on: Vec<String>,
    /// Plage de lumière à ses pieds, dans [0, 1] : le maximum de « ciel ×
    /// force du jour » et des blocs.
    pub light: [f32; 2],
    /// Mobs de cette entrée par m² au plus, autour du point d'apparition.
    pub density: f32,
    /// Taille du groupe, min et max : il apparaît entier ou pas du tout.
    pub group: [u32; 2],
    /// Un mob apparu disparaît au-delà de cette distance du joueur.
    pub despawn: f32,
}

impl SpawnRule {
    /// Ce qui cloche dans les réglages, s'il y a quelque chose.
    pub(crate) fn invalid(&self) -> Option<String> {
        let [lo, hi] = self.light;
        let [min, max] = self.group;
        let fine = !self.on.is_empty()
            && (0.0..=1.0).contains(&lo)
            && (lo..=1.0).contains(&hi)
            && self.density > 0.0
            && self.density <= 1.0
            && (1..=MAX_GROUP).contains(&min)
            && (min..=MAX_GROUP).contains(&max)
            // Plus loin que la zone d'apparition, sinon le mob disparaît en naissant.
            && self.despawn > SPAWN_MAX_M
            && self.despawn <= MAX_DESPAWN_M;
        (!fine).then(|| format!("règle d'apparition {self:?} : réglage hors bornes"))
    }
}

/// Un mob à faire naître : l'appelant lui donne un id, dans cet ordre.
#[derive(Debug, Clone, PartialEq)]
pub struct Born {
    pub content: ContentId,
    pub feet: [f32; 3],
    pub yaw: f32,
}

/// Les essais d'apparition du tick autour du joueur. `mobs` : tous les
/// mobs, gelés compris, triés par id ; `daylight` : force du jour dans
/// [0, 1].
pub fn spawn(world: &VoxelWorld, mobs: &[&Mob], player_feet: [f32; 3], seed: u64, daylight: f32) -> Vec<Born> {
    let rules: Vec<(ContentId, [f32; 2], &SpawnRule)> = world
        .registry
        .iter()
        .filter_map(|(id, e)| e.entity().map(|d| (id, d)))
        .flat_map(|(id, d)| d.spawn.iter().map(move |r| (id, d.size, r)))
        .collect();
    let mut born = Vec::new();
    if rules.is_empty() {
        return born;
    }
    let tick = world.tick();
    for attempt in 0..ATTEMPTS_PER_TICK {
        let roll = |purpose: u64| {
            let h = [SALT, tick, attempt, purpose].iter().fold(seed, |h, &v| hash64(h ^ v));
            (h >> 40) as f32 / (1u64 << 24) as f32
        };
        let (content, size, rule) = rules[((roll(0) * rules.len() as f32) as usize).min(rules.len() - 1)];
        let angle = roll(1) * std::f32::consts::TAU;
        let far = SPAWN_MIN_M + (SPAWN_MAX_M - SPAWN_MIN_M) * roll(2);
        let [px, py, pz] = player_feet;
        let top = [px + angle.sin() * far, py + (roll(3) * 2.0 - 1.0) * SPAWN_HEIGHT_M, pz + angle.cos() * far];
        let Some(first) = ground(world, top, size, DROP_M) else { continue };
        if !accepts(world, rule, first, daylight) {
            continue;
        }
        let n = rule.group[0] + (roll(4) * (rule.group[1] - rule.group[0] + 1) as f32) as u32;
        let n = n.min(rule.group[1]);
        let near = |f: [f32; 3]| (f[0] - first[0]).hypot(f[2] - first[2]) <= DENSITY_RADIUS_M;
        let count = mobs.iter().filter(|m| m.content == content && near(m.body.feet)).count()
            + born.iter().filter(|b: &&Born| b.content == content && near(b.feet)).count();
        let cap = rule.density * std::f32::consts::PI * DENSITY_RADIUS_M * DENSITY_RADIUS_M;
        if (count + n as usize) as f32 > cap {
            continue;
        }
        // Le groupe entier, ou rien.
        let group: Option<Vec<Born>> = (0..u64::from(n))
            .map(|i| {
                let feet = if i == 0 {
                    first
                } else {
                    let (a, d) = (roll(10 + i) * std::f32::consts::TAU, roll(30 + i) * GROUP_SPREAD_M);
                    let above = [first[0] + a.sin() * d, first[1] + 2.0, first[2] + a.cos() * d];
                    ground(world, above, size, 4.0)?
                };
                Some(Born { content, feet, yaw: roll(50 + i) * std::f32::consts::TAU })
            })
            .collect();
        born.extend(group.into_iter().flatten());
    }
    born
}

/// Ce mob doit-il disparaître ? Un mob apparu seul, au-delà de la plus
/// grande distance `despawn` des règles de son entrée (à l'horizontale).
pub fn gone(world: &VoxelWorld, mob: &Mob, player_feet: [f32; 3]) -> bool {
    let Some(data) = world.registry.get(mob.content).and_then(|e| e.entity()) else { return false };
    let Some(despawn) = data.spawn.iter().map(|r| r.despawn).reduce(f32::max) else { return false };
    let f = mob.body.feet;
    mob.spawned && (f[0] - player_feet[0]).hypot(f[2] - player_feet[2]) > despawn
}

/// Les pieds du premier sol sous `from` où le corps tient debout, à moins
/// de `drop_m` ; `None` hors des chunks chargés.
fn ground(world: &VoxelWorld, from: [f32; 3], size: [f32; 2], drop_m: f32) -> Option<[f32; 3]> {
    let cell = world.voxel_at_m(from);
    let cells = (drop_m * world.voxels_per_meter()).ceil() as i64;
    for dy in 0..=cells {
        let c = [cell[0], cell[1] - dy, cell[2]];
        world.chunk(world.split(c).0)?;
        if let Some(y) = path::stand(world, c, size) {
            let vpm = world.voxels_per_meter();
            return Some([(c[0] as f32 + 0.5) / vpm, y, (c[2] as f32 + 0.5) / vpm]);
        }
    }
    None
}

/// Le sol et la lumière conviennent-ils à la règle ?
fn accepts(world: &VoxelWorld, rule: &SpawnRule, feet: [f32; 3], daylight: f32) -> bool {
    let Some(under) = world.voxel(world.voxel_at_m([feet[0], feet[1] - 0.01, feet[2]])) else { return false };
    if !rule.on.iter().any(|name| world.registry.lookup(name) == Some(under)) {
        return false;
    }
    let full = f32::from(world.full_light().max(1));
    let [sky, blocks] = world.light_at(world.voxel_at_m([feet[0], feet[1] + 0.01, feet[2]])).map(f32::from);
    let light = (sky / full * daylight).max(blocks / full);
    (rule.light[0]..=rule.light[1]).contains(&light)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{Chunk, ChunkPos};
    use crate::registry::Registry;

    /// Monde de 7 × 7 chunks de 32 m à 1 vox/m autour de l'origine, sol
    /// d'herbe jusqu'à y = 4 (de pierre si `stone`), et un cochon qui
    /// apparaît sur l'herbe, en plein jour, par groupes de 2 à 3.
    fn world(stone: bool) -> VoxelWorld {
        world_with(stone, 0.0005)
    }

    fn world_with(stone: bool, density: f32) -> VoxelWorld {
        let reg = Registry::from_ron(&format!(
            r#"[
                (identifier: "t:air", kind: Block((solid: false, color: (0.0, 0.0, 0.0)))),
                (identifier: "t:stone", kind: Block((solid: true, color: (0.5, 0.5, 0.5)))),
                (identifier: "t:grass", kind: Block((solid: true, color: (0.2, 0.6, 0.2)))),
                (identifier: "t:pig", kind: EntityType((size: (0.9, 0.9),
                    model: [(name: "body", boxes: [((-5, 0, -8), (5, 14, 8), "t")])],
                    spawn: [(on: ["t:grass"], light: (0.5, 1.0), density: {density}, group: (2, 3), despawn: 128.0)]))),
            ]"#
        ))
        .unwrap();
        let air = reg.lookup("t:air").unwrap();
        let floor = reg.lookup(if stone { "t:stone" } else { "t:grass" }).unwrap();
        let mut w = VoxelWorld::new(reg, 32, 1.0);
        let mut chunk = Chunk::filled(32, air);
        for (x, z) in (0..32).flat_map(|x| (0..32).map(move |z| (x, z))) {
            for y in 0..4 {
                chunk.set(x, y, z, floor);
            }
        }
        for (x, z) in (-3..=3).flat_map(|x| (-3..=3).map(move |z| (x, z))) {
            w.insert_chunk(ChunkPos { x, y: 0, z }, chunk.clone());
        }
        w
    }

    const PLAYER: [f32; 3] = [16.0, 4.0, 16.0];

    /// `ticks` ticks d'essais ; les nés s'ajoutent aux mobs, ids dans l'ordre.
    fn run(w: &mut VoxelWorld, ticks: u32, seed: u64, daylight: f32) -> Vec<Mob> {
        let mut mobs: Vec<Mob> = Vec::new();
        for _ in 0..ticks {
            let born = spawn(w, &mobs.iter().collect::<Vec<_>>(), PLAYER, seed, daylight);
            for b in born {
                let mut m = Mob::new(mobs.len() as u64, b.content, b.feet, b.yaw);
                m.spawned = true;
                mobs.push(m);
            }
            w.advance();
        }
        mobs
    }

    #[test]
    fn pigs_appear_on_grass_in_daylight_up_to_the_density_and_the_run_repeats() {
        let mut w = world(false);
        let mobs = run(&mut w, 600, 3, 1.0);
        assert!(mobs.len() >= 2, "des cochons sont apparus : {}", mobs.len());
        // π × 64² × 0,0005 ≈ 6,4 par point d'apparition : 600 ticks de plus
        // n'en ajoutent presque pas, le plafond est atteint.
        let more = run(&mut world(false), 1200, 3, 1.0).len();
        assert!(more < mobs.len() + 6, "{} puis {more}", mobs.len());
        for m in &mobs {
            let d = (m.body.feet[0] - PLAYER[0]).hypot(m.body.feet[2] - PLAYER[2]);
            assert!((SPAWN_MIN_M - GROUP_SPREAD_M - 1.0..=SPAWN_MAX_M + GROUP_SPREAD_M + 1.0).contains(&d), "à {d} m");
            assert!((m.body.feet[1] - 4.0).abs() < 0.01, "sur le sol : {:?}", m.body.feet);
        }
        let feet = |mobs: Vec<Mob>| mobs.iter().map(|m| m.body.feet).collect::<Vec<_>>();
        assert_eq!(feet(mobs), feet(run(&mut world(false), 600, 3, 1.0)), "même seed, mêmes cochons");
    }

    #[test]
    fn no_pig_on_stone_or_at_night() {
        assert!(run(&mut world(true), 300, 3, 1.0).is_empty(), "sur la pierre");
        assert!(run(&mut world(false), 300, 3, 0.2).is_empty(), "de nuit");
        // Plafond (π × 64² × 0,0001 ≈ 1,3) sous la taille du groupe : jamais rien.
        assert!(run(&mut world_with(false, 0.0001), 300, 3, 1.0).is_empty(), "densité trop basse pour un groupe de 2");
    }

    #[test]
    fn only_a_spawned_mob_far_from_the_player_is_gone() {
        let w = world(false);
        let pig = w.registry.lookup("t:pig").unwrap();
        let mut far = Mob::new(0, pig, [16.0 + 130.0, 4.0, 16.0], 0.0);
        assert!(!gone(&w, &far, PLAYER), "posé : reste");
        far.spawned = true;
        assert!(gone(&w, &far, PLAYER), "apparu, à 130 m : disparaît");
        let mut near = far.clone();
        near.body.feet[0] = 16.0 + 100.0;
        assert!(!gone(&w, &near, PLAYER), "à 100 m : reste");
    }
}
