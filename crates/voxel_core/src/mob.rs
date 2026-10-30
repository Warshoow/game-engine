//! Mobs (#32, design doc §3.6, « Comportement des mobs ») : ce qu'un mob
//! décide de faire, puis le même pas de physique que le joueur.
//!
//! **Une liste ordonnée de comportements**, déclarée par l'entrée, prise
//! dans un vocabulaire append-only : le moteur code chaque comportement une
//! fois, le contenu choisit et règle. Le premier applicable gagne ; celui
//! en cours garde la main tant qu'il reste valable (avec une marge sur sa
//! portée), sauf si un comportement placé avant lui devient applicable.
//! Sans cette marge, un joueur posté au bord de la portée ferait changer
//! le mob d'avis à chaque tick.
//!
//! **Déterminisme** (§2) : le hasard d'un mob dérive de la seed, du tick,
//! de son id et de l'usage ; les mobs passent dans l'ordre de leur id
//! ([`step_all`]). Rien ici ne lit l'horloge ni un ordre de table de
//! hachage.

use serde::{Deserialize, Serialize};

use crate::model::EntityData;
use crate::path::{self, Node, Search};
use crate::physics::{Body, STEP_HEIGHT_M};
use crate::registry::ContentId;
use crate::world::{VoxelWorld, TICKS_PER_SECOND};
use crate::worldgen::hash64;

/// Cases qu'une recherche de chemin explore au plus.
const PATH_NODES_PER_MOB: u32 = 512;
/// Cases explorées par tick, tous mobs confondus : une case coûte quelques
/// µs (mesuré : 512 cases ≈ 2,4 ms), une longue recherche s'étale donc sur
/// plusieurs ticks ; au-delà, les mobs attendent leur tour.
const PATH_NODES_PER_TICK: u32 = 64;
/// Marge sur la portée d'un comportement en cours.
const KEEP_RANGE: f32 = 1.25;
/// `Follow` s'arrête à cette distance du joueur.
const FOLLOW_STOP_M: f32 = 2.0;
/// Le chemin de `Follow` est refait quand le joueur s'est éloigné de sa
/// fin de plus que ça.
const FOLLOW_REPATH_M: f32 = 1.5;
/// Ticks sans avancer avant d'abandonner un chemin (bloc posé dessus…).
const STUCK_TICKS: u32 = 48;
/// Rotation du corps, en radians par seconde.
const TURN_RAD_S: f32 = 5.0;
/// Vitesse à partir de laquelle les pattes se balancent pleinement.
const FULL_SWING_M_S: f32 = 1.0;

/// Un comportement de mob. Distances en mètres, durées en secondes,
/// vitesses en m/s. **Vocabulaire append-only** (§3.6) : jamais renommé ni
/// retiré.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub enum Behavior {
    /// Errer : marche vers un point tiré au hasard à moins de `range`, puis
    /// s'arrête jusqu'à `pause` secondes. Toujours applicable.
    Wander { speed: f32, range: f32, pause: f32 },
    /// Suivre le joueur qui tient l'entrée `holding`, à moins de `range`.
    Follow { holding: String, range: f32, speed: f32 },
    /// S'arrêter et regarder le joueur à moins de `range`, pendant `time`
    /// secondes ; pas avant `time` secondes de nouveau après.
    LookAt { range: f32, time: f32 },
}

impl Behavior {
    /// Ce qui cloche dans les réglages, s'il y a quelque chose.
    pub(crate) fn invalid(&self) -> Option<String> {
        let ok = |v: f32, max: f32| v > 0.0 && v <= max;
        let fine = match self {
            Behavior::Wander { speed, range, pause } => ok(*speed, 20.0) && ok(*range, 64.0) && ok(*pause, 600.0),
            Behavior::Follow { range, speed, .. } => ok(*range, 64.0) && ok(*speed, 20.0),
            Behavior::LookAt { range, time } => ok(*range, 64.0) && ok(*time, 600.0),
        };
        (!fine).then(|| format!("comportement {self:?} : réglage nul, négatif ou trop grand"))
    }

    /// L'identifier cité, à valider au chargement du registre.
    pub fn references(&self) -> Option<&String> {
        match self {
            Behavior::Follow { holding, .. } => Some(holding),
            _ => None,
        }
    }
}

/// Ce que le mob voit du joueur.
#[derive(Debug, Clone, Copy)]
pub struct Player {
    pub feet: [f32; 3],
    pub eyes: [f32; 3],
    pub holding: Option<ContentId>,
}

/// Le comportement en cours : son rang dans la liste, et jusqu'à quel tick
/// il attend (pause de `Wander`, fin de `LookAt`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Doing {
    index: usize,
    until: u64,
}

/// Un mob : sauvé, son id, son entrée, ses pieds et son cap ; le reste
/// repart au chargement.
#[derive(Debug, Clone)]
pub struct Mob {
    pub id: u64,
    pub content: ContentId,
    pub body: Body,
    /// Radians autour de +Y ; 0 : regarde vers +Z.
    pub yaw: f32,
    /// Les pieds au tick d'avant, pour l'affichage interpolé.
    pub prev_feet: [f32; 3],
    /// Distance parcourue à l'horizontale (m) : le balancement des pattes.
    pub walked: f32,
    /// Ampleur du balancement, de 0 (arrêté) à 1, lissée.
    pub swing: f32,
    /// Ce que regarde la tête, s'il y a une cible.
    pub look_at: Option<[f32; 3]>,
    doing: Option<Doing>,
    /// Pas avant ce tick pour `LookAt`.
    look_rest_until: u64,
    path: Vec<Node>,
    /// La recherche du prochain chemin, en cours.
    search: Option<Search>,
    stuck: u32,
}

impl Mob {
    pub fn new(id: u64, content: ContentId, feet: [f32; 3], yaw: f32) -> Self {
        Self {
            id,
            content,
            body: Body::at(feet),
            yaw,
            prev_feet: feet,
            walked: 0.0,
            swing: 0.0,
            look_at: None,
            doing: None,
            look_rest_until: 0,
            path: Vec::new(),
            search: None,
            stuck: 0,
        }
    }

    /// Un tirage dans [0, 1), propre à ce mob, ce tick et cet usage.
    fn roll(&self, seed: u64, tick: u64, purpose: u64) -> f32 {
        let h = [tick, self.id, purpose].iter().fold(seed, |h, &v| hash64(h ^ v));
        (h >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Le comportement `b` peut-il démarrer (`keep` : continuer, avec la
    /// marge de portée) ?
    fn applies(&self, b: &Behavior, player: Option<&Player>, world: &VoxelWorld, tick: u64, keep: bool) -> bool {
        let near = |range: f32| {
            let reach = if keep { range * KEEP_RANGE } else { range };
            player.is_some_and(|p| dist(p.feet, self.body.feet) <= reach)
        };
        match b {
            Behavior::Wander { .. } => true,
            Behavior::Follow { holding, range, .. } => {
                near(*range) && player.and_then(|p| p.holding) == world.registry.lookup(holding)
            }
            Behavior::LookAt { range, .. } => {
                near(*range) && (if keep { tick < self.doing.map_or(0, |d| d.until) } else { tick >= self.look_rest_until })
            }
        }
    }

    /// Choisit le comportement du tick et ce qu'il veut : vitesse de marche
    /// et case visée.
    fn think(&mut self, data: &EntityData, player: Option<&Player>, world: &VoxelWorld, seed: u64, tick: u64) -> f32 {
        let current = self.doing.map(|d| d.index);
        let keeps = current.is_some_and(|i| self.applies(&data.behaviors[i], player, world, tick, true));
        // Un regard fini se repose avant de pouvoir reprendre.
        if let (false, Some(Behavior::LookAt { time, .. })) = (keeps, current.map(|i| &data.behaviors[i])) {
            self.look_rest_until = tick + ticks(*time);
        }
        let first = data.behaviors.iter().position(|b| self.applies(b, player, world, tick, false));
        let chosen = match (first, current) {
            (Some(f), Some(c)) if keeps && c <= f => Some(c),
            _ => first,
        };
        if chosen != current || !keeps {
            self.doing = chosen.map(|index| Doing { index, until: 0 });
            self.path.clear();
            self.search = None;
            if let Some(Behavior::LookAt { time, .. }) = chosen.map(|i| &data.behaviors[i]) {
                self.doing = chosen.map(|index| Doing { index, until: tick + ticks(*time) });
            }
        }
        let Some(doing) = self.doing else { return 0.0 };
        self.look_at = None;
        match &data.behaviors[doing.index] {
            Behavior::Wander { speed, range, pause } => {
                if self.path.is_empty() && self.search.is_none() && tick >= doing.until {
                    if doing.until == 0 {
                        // Arrivé (ou rien trouvé) : une pause d'abord.
                        let until = tick + 1 + ticks(pause * self.roll(seed, tick, 1));
                        self.doing = Some(Doing { until, ..doing });
                    } else {
                        self.search = self.wander_target(world, data, *range, seed, tick).map(|to| self.search_to(world, data, to));
                        self.doing = Some(Doing { until: 0, ..doing });
                    }
                }
                *speed
            }
            Behavior::Follow { speed, .. } => {
                let p = player.expect("Follow ne s'applique qu'avec un joueur");
                self.look_at = Some(p.eyes);
                if dist(p.feet, self.body.feet) <= FOLLOW_STOP_M {
                    self.path.clear();
                    self.search = None;
                    return 0.0;
                }
                let end = self.path.last().map(|n| world_center(world, n.cell));
                if self.search.is_none() && end.is_none_or(|e| dist(e, p.feet) > FOLLOW_REPATH_M) {
                    self.search = Some(self.search_to(world, data, world.voxel_at_m(p.feet)));
                }
                *speed
            }
            Behavior::LookAt { .. } => {
                self.look_at = player.map(|p| p.eyes);
                0.0
            }
        }
    }

    fn search_to(&self, world: &VoxelWorld, data: &EntityData, to: [i64; 3]) -> Search {
        let from = Node { cell: world.voxel_at_m(self.body.feet), feet_y: self.body.feet[1] };
        Search::new(from, to, data.size, PATH_NODES_PER_MOB)
    }

    /// Une case où le corps tient, à moins de `range` à l'horizontale.
    fn wander_target(&self, world: &VoxelWorld, data: &EntityData, range: f32, seed: u64, tick: u64) -> Option<[i64; 3]> {
        let angle = self.roll(seed, tick, 2) * std::f32::consts::TAU;
        let far = range * (0.5 + 0.5 * self.roll(seed, tick, 3));
        let f = self.body.feet;
        let spot = world.voxel_at_m([f[0] + angle.sin() * far, f[1], f[2] + angle.cos() * far]);
        // La case la plus proche en hauteur, à quelques mètres près.
        let reach = (3.0 * world.voxels_per_meter()).ceil() as i64;
        (0..=reach)
            .flat_map(|d| [d, -d])
            .map(|dy| [spot[0], spot[1] + dy, spot[2]])
            .find(|&c| path::stand(world, c, data.size).is_some())
    }

    /// Marche le long du chemin, à `speed`, et tourne le corps.
    fn walk(&mut self, world: &VoxelWorld, data: &EntityData, speed: f32, dt: f32) {
        let vpm = world.voxels_per_meter();
        let feet = self.body.feet;
        // Les cases déjà atteintes sortent du chemin.
        while let Some(next) = self.path.first() {
            let c = world_center(world, next.cell);
            let close = (c[0] - feet[0]).hypot(c[2] - feet[2]) < 0.35 / vpm.max(1.0);
            if close && (feet[1] - next.feet_y).abs() < 0.5 {
                self.path.remove(0);
            } else {
                break;
            }
        }
        let mut wish = [0.0, 0.0];
        let mut jump = false;
        if let Some(next) = self.path.first() {
            let c = world_center(world, next.cell);
            let (dx, dz) = (c[0] - feet[0], c[2] - feet[2]);
            let len = dx.hypot(dz).max(1e-6);
            wish = [dx / len * speed, dz / len * speed];
            // Saute seulement si la marche ne suffit pas (bloqué au tick d'avant) :
            // un escalier se monte marche par marche, même si la case est à 1 m.
            jump = self.body.grounded && self.stuck > 0 && next.feet_y - feet[1] > STEP_HEIGHT_M;
            self.turn_to(dx.atan2(dz), dt);
        } else if let Some(target) = self.look_at {
            let (dx, dz) = (target[0] - feet[0], target[2] - feet[2]);
            if dx.hypot(dz) > 0.1 {
                self.turn_to(dx.atan2(dz), dt);
            }
        }
        // Dans un liquide, le mob nage vers la surface (il ne coule pas).
        let wet = world.voxel(world.voxel_at_m([feet[0], feet[1] + 0.1, feet[2]])).and_then(|id| world.liquid(id));
        let eyes = data.size[1] * 0.85;
        self.body.step(world, data.size, eyes, wish, jump || wet.is_some(), dt);

        let moved = (self.body.feet[0] - feet[0]).hypot(self.body.feet[2] - feet[2]);
        self.walked += moved;
        let target = (moved / dt / FULL_SWING_M_S).min(1.0);
        self.swing += (target - self.swing) * 0.2;
        // Bloqué (un bloc posé sur le chemin, un autre mob…) : on abandonne.
        if !self.path.is_empty() && moved < speed * dt * 0.25 {
            self.stuck += 1;
            if self.stuck > STUCK_TICKS {
                self.path.clear();
                self.stuck = 0;
            }
        } else {
            self.stuck = 0;
        }
    }

    fn turn_to(&mut self, yaw: f32, dt: f32) {
        let diff = (yaw - self.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        self.yaw += diff.clamp(-TURN_RAD_S * dt, TURN_RAD_S * dt);
    }
}

/// Un tick pour tous les mobs, `mobs` triés par id. `turn` : l'id après
/// lequel la recherche de chemin reprend au tick suivant (tour à tour, pour
/// qu'aucun mob n'attende toujours) ; à garder d'un tick à l'autre.
pub fn step_all(world: &VoxelWorld, mobs: &mut [&mut Mob], player: Option<&Player>, seed: u64, dt: f32, turn: &mut u64) {
    let tick = world.tick();
    let mut speeds = vec![0.0; mobs.len()];
    for (mob, speed) in mobs.iter_mut().zip(&mut speeds) {
        mob.prev_feet = mob.body.feet;
        if !loaded(world, mob.body.feet) {
            continue;
        }
        let Some(data) = world.registry.get(mob.content).and_then(|e| e.entity()) else { continue };
        *speed = mob.think(data, player, world, seed, tick);
    }
    // Les recherches de chemin avancent tour à tour à partir de `turn`,
    // dans la limite du budget du tick.
    let start = mobs.iter().position(|m| m.id > *turn).unwrap_or(0);
    let mut budget = PATH_NODES_PER_TICK;
    for i in (start..mobs.len()).chain(0..start) {
        if budget == 0 {
            break;
        }
        let mob = &mut mobs[i];
        let Some(search) = &mut mob.search else { continue };
        let (used, found) = search.run(world, budget);
        budget -= used;
        *turn = mob.id;
        if let Some(found) = found {
            mob.path = found;
            mob.search = None;
            mob.stuck = 0;
        }
    }
    for (mob, speed) in mobs.iter_mut().zip(speeds) {
        if !loaded(world, mob.body.feet) {
            continue;
        }
        let Some(data) = world.registry.get(mob.content).and_then(|e| e.entity()) else { continue };
        mob.walk(world, data, speed, dt);
    }
}

/// On ne simule pas dans un chunk absent, comme le joueur : il tomberait
/// à travers un monde pas encore chargé.
fn loaded(world: &VoxelWorld, feet: [f32; 3]) -> bool {
    world.chunk(world.split(world.voxel_at_m(feet)).0).is_some()
}

fn ticks(seconds: f32) -> u64 {
    (seconds * TICKS_PER_SECOND as f32).round() as u64
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f32>().sqrt()
}

/// Le centre au sol d'une case, en mètres.
fn world_center(world: &VoxelWorld, cell: [i64; 3]) -> [f32; 3] {
    let vpm = world.voxels_per_meter();
    [(cell[0] as f32 + 0.5) / vpm, cell[1] as f32 / vpm, (cell[2] as f32 + 0.5) / vpm]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{Chunk, ChunkPos};
    use crate::registry::Registry;

    const DT: f32 = 1.0 / TICKS_PER_SECOND as f32;

    /// Monde 32³ à 1 vox/m, sol jusqu'à y = 4, et un cochon aux
    /// comportements `behaviors`.
    fn world(behaviors: &str) -> VoxelWorld {
        let reg = Registry::from_ron(&format!(
            r#"[
                (identifier: "t:air", kind: Block((solid: false, color: (0.0, 0.0, 0.0)))),
                (identifier: "t:stone", kind: Block((solid: true, color: (0.5, 0.5, 0.5)))),
                (identifier: "t:wheat", kind: Item),
                (identifier: "t:slab", kind: Block((solid: true, color: (0.5, 0.5, 0.5),
                    shape: Some([((0, 0, 0), (16, 8, 16))])))),
                (identifier: "t:stairs", kind: Block((solid: true, color: (0.5, 0.5, 0.5),
                    shape: Some([((0, 0, 0), (16, 8, 16)), ((0, 8, 8), (16, 16, 16))])))),
                (identifier: "t:pig", kind: EntityType((size: (0.9, 0.9),
                    model: [(name: "body", boxes: [((-5, 0, -8), (5, 14, 8), "t")])],
                    behaviors: [{behaviors}]))),
            ]"#
        ))
        .unwrap();
        let (air, stone) = (reg.lookup("t:air").unwrap(), reg.lookup("t:stone").unwrap());
        let mut w = VoxelWorld::new(reg, 32, 1.0);
        let mut chunk = Chunk::filled(32, air);
        for (x, z) in (0..32).flat_map(|x| (0..32).map(move |z| (x, z))) {
            for y in 0..4 {
                chunk.set(x, y, z, stone);
            }
        }
        w.insert_chunk(ChunkPos { x: 0, y: 0, z: 0 }, chunk);
        w
    }

    /// `seconds` de simulation des `count` premiers cochons de [`pigs`].
    fn run(w: &mut VoxelWorld, count: usize, player: Option<Player>, seed: u64, seconds: f32) -> Vec<Mob> {
        let mobs = pigs(w)[..count].to_vec();
        resume(w, mobs, player, seed, seconds)
    }

    /// `seconds` de simulation de plus ; rend les mobs.
    fn resume(w: &mut VoxelWorld, mut mobs: Vec<Mob>, player: Option<Player>, seed: u64, seconds: f32) -> Vec<Mob> {
        let mut turn = 0;
        for _ in 0..ticks(seconds) {
            step_all(w, &mut mobs.iter_mut().collect::<Vec<_>>(), player.as_ref(), seed, DT, &mut turn);
            w.advance();
        }
        mobs
    }

    fn pigs(w: &VoxelWorld) -> Vec<Mob> {
        let pig = w.registry.lookup("t:pig").unwrap();
        vec![Mob::new(1, pig, [10.5, 6.0, 10.5], 0.0), Mob::new(2, pig, [20.5, 4.0, 20.5], 0.0)]
    }

    #[test]
    fn a_wandering_pig_falls_then_walks_and_the_run_repeats() {
        let wander = "Wander(speed: 1.0, range: 6.0, pause: 2.0)";
        let mut w = world(wander);
        let after = run(&mut w, 2, None, 7, 30.0);
        for (mob, start) in after.iter().zip(pigs(&w)) {
            assert!((mob.body.feet[1] - 4.0).abs() < 0.01, "au sol : {:?}", mob.body.feet);
            assert!(dist(mob.body.feet, start.body.feet) > 1.0 || mob.walked > 3.0, "a erré : {}", mob.walked);
        }
        // Même seed, même monde, mêmes entrées : mêmes positions ; autre seed, autres.
        let state = |mobs: Vec<Mob>| mobs.iter().map(|m| (m.id, m.body, m.yaw, m.walked)).collect::<Vec<_>>();
        let mut again = world(wander);
        assert_eq!(state(after.clone()), state(run(&mut again, 2, None, 7, 30.0)));
        let mut other = world(wander);
        assert_ne!(state(after), state(run(&mut other, 2, None, 8, 30.0)));
    }

    #[test]
    fn a_pig_follows_only_the_player_holding_its_item() {
        let follow = r#"Follow(holding: "t:wheat", range: 16.0, speed: 2.0)"#;
        let mut w = world(follow);
        let wheat = w.registry.lookup("t:wheat");
        let player = |holding| Player { feet: [25.5, 4.0, 10.5], eyes: [25.5, 5.6, 10.5], holding };
        let pig = &run(&mut w, 1, Some(player(wheat)), 1, 15.0)[0];
        assert!(dist(pig.body.feet, [25.5, 4.0, 10.5]) < FOLLOW_STOP_M + 0.5, "{:?}", pig.body.feet);
        assert!(pig.look_at.is_some());
        let mut w = world(follow);
        let pig = &run(&mut w, 1, Some(player(None)), 1, 15.0)[0];
        assert!((pig.body.feet[0] - 10.5).abs() < 0.01, "main vide : il ne bouge pas");
    }

    #[test]
    fn a_pig_stares_for_a_while_then_rests_before_staring_again() {
        let mut w = world("LookAt(range: 8.0, time: 2.0), Wander(speed: 1.0, range: 6.0, pause: 1.0)");
        let player = Player { feet: [13.5, 4.0, 10.5], eyes: [13.5, 5.6, 10.5], holding: None };
        let mut mobs = run(&mut w, 1, Some(player), 1, 1.5);
        assert_eq!(mobs[0].look_at, Some(player.eyes), "regarde");
        assert!(mobs[0].walked < 0.01, "sans marcher");
        mobs = resume(&mut w, mobs, Some(player), 1, 1.0);
        assert_eq!(mobs[0].look_at, None, "au bout de 2 s, repos");
        mobs = resume(&mut w, mobs, Some(player), 1, 2.0);
        assert_eq!(mobs[0].look_at, Some(player.eyes), "puis regarde de nouveau");
    }

    #[test]
    fn a_pig_walks_up_a_slab_or_a_stair_and_jumps_a_full_block() {
        for (step, jumps) in [("t:slab", false), ("t:stairs", false), ("t:stone", true)] {
            let mut w = world(r#"Follow(holding: "t:wheat", range: 16.0, speed: 2.0)"#);
            let (step_id, stone) = (w.registry.lookup(step).unwrap(), w.registry.lookup("t:stone").unwrap());
            // Une marche en z = 13 sur toute la largeur, puis un plateau d'1 m.
            for x in 0..32 {
                w.set_voxel([x, 4, 13], step_id);
                for z in 14..32 {
                    w.set_voxel([x, 4, z], stone);
                }
            }
            let wheat = w.registry.lookup("t:wheat");
            let player = Player { feet: [10.5, 5.0, 20.5], eyes: [10.5, 6.6, 20.5], holding: wheat };
            let mut mobs = run(&mut w, 1, Some(player), 1, 0.0);
            let mut rise = 0.0f32;
            for _ in 0..ticks(10.0) {
                mobs = resume(&mut w, mobs, Some(player), 1, DT);
                rise = rise.max(mobs[0].body.velocity[1]);
            }
            assert!((mobs[0].body.feet[1] - 5.0).abs() < 0.01, "{step} : monté sur le plateau, {:?}", mobs[0].body.feet);
            assert_eq!(rise > 0.1, jumps, "{step} : vitesse verticale {rise}");
        }
    }
}
