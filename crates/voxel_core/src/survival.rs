//! Jauges et survie (#33, design doc §3.6). Le moteur ne connaît ni la vie
//! ni la faim : `assets/content/survival.ron` déclare des jauges et des
//! règles qui les font varier tant que leurs conditions sont vraies. Vie,
//! faim, souffle ne sont que des jauges écrites ainsi ; un autre jeu ajoute
//! température ou oxygène sans toucher au Rust.
//!
//! Au tick fixe, toutes les règles lisent les valeurs du début du tick, puis
//! leurs variations s'additionnent : l'ordre des règles ne change rien. Les
//! blocs touchés (`touching`), la chute et la nourriture (`eaten`) font
//! varier les jauges que la donnée nomme. Une jauge reste dans [0, max].

use serde::{Deserialize, Serialize};

use crate::physics::{Aabb, Body};
use crate::registry::{parse, ContentId, Registry};
use crate::world::VoxelWorld;

/// Bornes de la donnée : elle vient d'un fichier, pas du code.
const MAX_GAUGES: usize = 32;
const MAX_RULES: usize = 128;
pub(crate) const MAX_PER_RULE: usize = 16;
const MAX_PUSH_M_S: f32 = 30.0;

/// Distance à laquelle un bloc solide compte comme touché (un cactus contre
/// lequel on marche : la collision arrête le corps juste avant lui).
const TOUCH_MARGIN_M: f32 = 0.05;

/// Une jauge : une valeur dans [0, max] portée par le joueur.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Gauge {
    pub name: String,
    pub max: f32,
    /// Valeur d'un monde neuf et après la mort ; absent : `max`.
    #[serde(default)]
    pub start: Option<f32>,
    /// Absent : pas de barre à l'écran.
    #[serde(default)]
    pub bar: Option<Bar>,
}

impl Gauge {
    pub fn start(&self) -> f32 {
        self.start.unwrap_or(self.max)
    }
}

/// Barre à l'écran : couleur sRGB et quand l'afficher.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
pub struct Bar {
    pub color: [f32; 3],
    #[serde(default)]
    pub show: Show,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum Show {
    #[default]
    Always,
    /// Seulement quand la jauge n'est pas pleine (le souffle).
    NotFull,
}

/// Tant que toutes les conditions sont vraies, chaque jauge citée varie
/// de tant par seconde.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct GaugeRule {
    #[serde(default)]
    pub when: Vec<Condition>,
    pub change: Vec<(String, f32)>,
}

/// Ce qu'une règle de jauge peut lire du joueur. **Vocabulaire
/// append-only** (§3.6).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub enum Condition {
    /// Les yeux dans un liquide.
    HeadInLiquid,
    /// Les pieds sous cette hauteur, en mètres.
    Below(f32),
    /// La jauge nommée dans cette plage, bornes comprises.
    Gauge(String, [f32; 2]),
    /// La force du jour (dans [0, 1], voir `Sky::daylight`) dans cette plage.
    Daylight([f32; 2]),
    Not(Box<Condition>),
}

/// La chute retire `per_m` par mètre au-delà de `safe` à la jauge nommée.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Fall {
    pub gauge: String,
    pub safe: f32,
    pub per_m: f32,
}

/// Un coup porté à un mob (#68) : `change`, variations d'un coup sur ses
/// jauges (comme `eaten`), et `push`, recul en m/s.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Hit {
    pub change: Vec<(String, f32)>,
    #[serde(default)]
    pub push: f32,
}

impl Hit {
    pub(crate) fn invalid(&self) -> Option<String> {
        let fine = self.change.len() <= MAX_PER_RULE
            && self.change.iter().all(|(_, v)| v.is_finite())
            && (0.0..=MAX_PUSH_M_S).contains(&self.push);
        (!fine).then(|| format!("hit : {MAX_PER_RULE} variations finies au plus, push dans [0, {MAX_PUSH_M_S}]"))
    }
}

/// Le joueur meurt quand la jauge nommée tombe à 0.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Death {
    pub gauge: String,
    #[serde(default)]
    pub keep_inventory: bool,
}

/// Le contenu de `survival.ron`. Par défaut : aucune jauge, pas de mort.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct Survival {
    #[serde(default)]
    pub gauges: Vec<Gauge>,
    #[serde(default)]
    pub rules: Vec<GaugeRule>,
    #[serde(default)]
    pub fall: Option<Fall>,
    #[serde(default)]
    pub death: Option<Death>,
    /// Le coup de la main nue (#68) ; absent : elle ne frappe pas.
    #[serde(default)]
    pub hand: Option<Hit>,
}

impl Survival {
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let s: Self = parse(text).map_err(|e| e.to_string())?;
        s.invalid().map_or(Ok(s), |reason| Err(format!("survival : {reason}")))
    }

    /// Indice de la jauge `name`.
    pub fn index(&self, name: &str) -> Option<usize> {
        self.gauges.iter().position(|g| g.name == name)
    }

    pub(crate) fn invalid(&self) -> Option<String> {
        if self.gauges.len() > MAX_GAUGES || self.rules.len() > MAX_RULES {
            return Some(format!("{MAX_GAUGES} jauges et {MAX_RULES} règles au plus"));
        }
        for (i, g) in self.gauges.iter().enumerate() {
            if g.name.is_empty() || self.index(&g.name) != Some(i) {
                return Some(format!("nom de jauge vide ou en double : « {} »", g.name));
            }
            if !(g.max.is_finite() && g.max > 0.0 && (0.0..=g.max).contains(&g.start())) {
                return Some(format!("{} : max > 0 et start dans [0, max]", g.name));
            }
        }
        let known = |name: &String| self.index(name).map(|_| ()).ok_or(format!("jauge inconnue : « {name} »"));
        let range = |r: [f32; 2]| r[0] <= r[1]; // faux aussi pour NaN
        for rule in &self.rules {
            if rule.when.len() > MAX_PER_RULE || rule.change.len() > MAX_PER_RULE {
                return Some(format!("{MAX_PER_RULE} conditions et variations par règle au plus"));
            }
            for (name, rate) in &rule.change {
                if let Err(e) = known(name) {
                    return Some(e);
                }
                if !rate.is_finite() {
                    return Some(format!("{name} : variation {rate} non finie"));
                }
            }
            let mut conditions: Vec<&Condition> = rule.when.iter().collect();
            while let Some(c) = conditions.pop() {
                let bad = match c {
                    Condition::HeadInLiquid => None,
                    Condition::Below(m) => (!m.is_finite()).then(|| format!("Below({m})")),
                    Condition::Gauge(name, r) => known(name).err().or((!range(*r)).then(|| format!("{name} : plage {r:?}"))),
                    Condition::Daylight(r) => (!range(*r)).then(|| format!("Daylight : plage {r:?}")),
                    Condition::Not(inner) => {
                        conditions.push(inner);
                        None
                    }
                };
                if bad.is_some() {
                    return bad;
                }
            }
        }
        if let Some(f) = &self.fall {
            if let Err(e) = known(&f.gauge) {
                return Some(e);
            }
            if !(f.safe >= 0.0 && f.per_m >= 0.0 && f.safe.is_finite() && f.per_m.is_finite()) {
                return Some("fall : safe et per_m positifs".into());
            }
        }
        if let Some(reason) = self.hand.as_ref().and_then(Hit::invalid) {
            return Some(reason);
        }
        self.death.as_ref().and_then(|d| known(&d.gauge).err())
    }

    /// Les jauges que citent `touching` et `eaten` du registre sans que ce
    /// fichier les déclare. Pas une erreur (un monde doit se charger dans un
    /// jeu sans survie) : la variation est ignorée, à signaler.
    pub fn unknown_gauges(&self, registry: &Registry) -> Vec<String> {
        let mut unknown: Vec<String> = Vec::new();
        for (_, entry) in registry.iter() {
            let touching = entry.block().map_or(&[][..], |b| &b.touching);
            for (name, _) in touching.iter().chain(&entry.eaten) {
                if self.index(name).is_none() && !unknown.contains(name) {
                    unknown.push(name.clone());
                }
            }
        }
        unknown
    }
}

/// Les jauges d'un corps (joueur ou mob), dans l'ordre de
/// [`Survival::gauges`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Vitals {
    pub values: Vec<f32>,
    /// Le point le plus haut depuis le dernier contact avec le sol ou un
    /// liquide, en mètres : la chute se mesure à partir de lui. `None`
    /// jusqu'au premier contact : le départ du monde est 8 m au-dessus du
    /// sol, y tomber ne blesse pas.
    fall_top: Option<f32>,
}

impl Vitals {
    /// Téléporté (pieds à `feet_y`, en mètres) : la chute se mesure depuis
    /// là, pas depuis le sol quitté.
    pub fn teleported(&mut self, feet_y: f32) {
        self.fall_top = Some(feet_y);
    }

    /// Remises à leur départ si elles ne suivent pas `survival` (un mob
    /// neuf, ou dont l'entrée a changé de jauges).
    pub fn fit(&mut self, survival: &Survival) {
        if self.values.len() != survival.gauges.len() {
            *self = Self::new(survival);
        }
    }

    /// Toutes les jauges à leur départ (monde neuf, après la mort).
    pub fn new(survival: &Survival) -> Self {
        Self { values: survival.gauges.iter().map(Gauge::start).collect(), fall_top: None }
    }

    /// Relues d'une save, par nom : une jauge absente repart à son départ.
    pub fn restored(survival: &Survival, saved: &[(String, f32)]) -> Self {
        let mut vitals = Self::new(survival);
        for (name, value) in saved {
            if let Some(i) = survival.index(name) {
                vitals.values[i] = value.clamp(0.0, survival.gauges[i].max);
            }
        }
        vitals
    }

    /// À sauver avec le joueur, par nom.
    pub fn saved(&self, survival: &Survival) -> Vec<(String, f32)> {
        survival.gauges.iter().map(|g| g.name.clone()).zip(self.values.iter().copied()).collect()
    }

    /// Valeur de la jauge `name`.
    pub fn get(&self, survival: &Survival, name: &str) -> Option<f32> {
        survival.index(name).map(|i| self.values[i])
    }

    /// Un tick de `dt` secondes, après le pas de physique de `body` (taille
    /// `size`, yeux à `eyes_m`). Rend vrai si le joueur meurt, y compris
    /// d'un coup reçu depuis le tick d'avant (`apply`) : sans ça, une
    /// règle qui fait remonter la vie l'empêcherait de rester à 0.
    #[allow(clippy::too_many_arguments)]
    pub fn step(&mut self, survival: &Survival, world: &VoxelWorld, body: &Body, size: [f32; 2], eyes_m: f32, daylight: f32, dt: f32) -> bool {
        let liquid_at = |height_m: f32| {
            let v = world.voxel_at_m([body.feet[0], body.feet[1] + height_m, body.feet[2]]);
            world.voxel(v).and_then(|id| world.liquid(id)).is_some()
        };
        if self.dead(survival) {
            return true;
        }
        let senses = Senses::of(world, body, eyes_m, daylight);
        let mut delta = vec![0.0; self.values.len()];
        let mut add = |name: &str, amount: f32| {
            if let Some(i) = survival.index(name) {
                delta[i] += amount;
            }
        };
        for rule in &survival.rules {
            if rule.when.iter().all(|c| senses.holds(c, survival, &self.values)) {
                rule.change.iter().for_each(|(name, rate)| add(name, rate * dt));
            }
        }
        for id in touched(world, body, size) {
            let touching = world.registry.get(id).and_then(|e| e.block()).map_or(&[][..], |b| &b.touching);
            touching.iter().for_each(|(name, rate)| add(name, rate * dt));
        }
        // Chute : même seuil que la physique pour « dans un liquide ».
        let feet_y = body.feet[1];
        if liquid_at(0.1) {
            self.fall_top = Some(feet_y);
        } else if body.grounded {
            if let (Some(f), Some(top)) = (&survival.fall, self.fall_top) {
                let drop = top - feet_y;
                if drop > f.safe {
                    add(&f.gauge, -(drop - f.safe) * f.per_m);
                }
            }
            self.fall_top = Some(feet_y);
        } else {
            self.fall_top = self.fall_top.map(|top| top.max(feet_y));
        }
        for ((value, d), g) in self.values.iter_mut().zip(delta).zip(&survival.gauges) {
            *value = (*value + d).clamp(0.0, g.max);
        }
        self.dead(survival)
    }

    /// Toutes les conditions de `when` sont vraies pour ce corps (`Attack`
    /// d'un mob, #72), comme pour une règle de jauge.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn hold(&self, when: &[Condition], survival: &Survival, world: &VoxelWorld, body: &Body, eyes_m: f32, daylight: f32) -> bool {
        let senses = Senses::of(world, body, eyes_m, daylight);
        when.iter().all(|c| senses.holds(c, survival, &self.values))
    }

    /// La jauge de `death` est à 0.
    pub fn dead(&self, survival: &Survival) -> bool {
        survival.death.as_ref().and_then(|d| self.get(survival, &d.gauge)).is_some_and(|v| v <= 0.0)
    }

    /// Des variations d'un coup (`eaten`, un coup reçu). Rend faux si
    /// aucune jauge connue n'en change (rien n'est mangé).
    pub fn apply(&mut self, survival: &Survival, eaten: &[(String, f32)]) -> bool {
        let mut changed = false;
        for (name, amount) in eaten {
            let Some(i) = survival.index(name) else { continue };
            let after = (self.values[i] + amount).clamp(0.0, survival.gauges[i].max);
            changed |= after != self.values[i];
            self.values[i] = after;
        }
        changed
    }
}

/// Ce que les conditions lisent du joueur, une fois par tick.
struct Senses {
    head_in_liquid: bool,
    feet_y: f32,
    daylight: f32,
}

impl Senses {
    fn of(world: &VoxelWorld, body: &Body, eyes_m: f32, daylight: f32) -> Self {
        let v = world.voxel_at_m([body.feet[0], body.feet[1] + eyes_m, body.feet[2]]);
        let head_in_liquid = world.voxel(v).and_then(|id| world.liquid(id)).is_some();
        Self { head_in_liquid, feet_y: body.feet[1], daylight }
    }

    fn holds(&self, c: &Condition, survival: &Survival, values: &[f32]) -> bool {
        let within = |v: f32, r: [f32; 2]| r[0] <= v && v <= r[1];
        match c {
            Condition::HeadInLiquid => self.head_in_liquid,
            Condition::Below(m) => self.feet_y < *m,
            Condition::Gauge(name, r) => survival.index(name).is_some_and(|i| within(values[i], *r)),
            Condition::Daylight(r) => within(self.daylight, *r),
            Condition::Not(inner) => !self.holds(inner, survival, values),
        }
    }
}

/// Les sortes de blocs à `touching` que touche le corps, chacune une fois :
/// un liquide où il trempe, un bloc solide à moins de [`TOUCH_MARGIN_M`].
fn touched(world: &VoxelWorld, body: &Body, size: [f32; 2]) -> Vec<ContentId> {
    let aabb = Aabb::from_feet(body.feet, size[0], size[1]);
    let near = Aabb { min: aabb.min.map(|c| c - TOUCH_MARGIN_M), max: aabb.max.map(|c| c + TOUCH_MARGIN_M) };
    let vpm = world.voxels_per_meter();
    let overlaps = |a: &Aabb, min: [f32; 3], max: [f32; 3]| (0..3).all(|k| min[k] / vpm < a.max[k] && max[k] / vpm > a.min[k]);
    let lo = |v: f32| (v * vpm).floor() as i64;
    let hi = |v: f32| ((v * vpm) - 1e-6).floor() as i64;
    let mut ids = Vec::new();
    for x in lo(near.min[0])..=hi(near.max[0]) {
        for y in lo(near.min[1])..=hi(near.max[1]) {
            for z in lo(near.min[2])..=hi(near.max[2]) {
                let v = [x, y, z];
                let Some(id) = world.voxel(v) else { continue };
                let Some(b) = world.registry.get(id).and_then(|e| e.block()) else { continue };
                if b.touching.is_empty() || ids.contains(&id) {
                    continue;
                }
                let cell = v.map(|c| c as f32);
                let touches = if b.solid {
                    world.boxes(v, true).into_iter().any(|[min, max]| overlaps(&near, min, max))
                } else {
                    overlaps(&aabb, cell, cell.map(|c| c + 1.0))
                };
                if touches {
                    ids.push(id);
                }
            }
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{Chunk, ChunkPos};

    const SIZE: [f32; 2] = [0.6, 1.8];
    const EYES: f32 = 1.6;
    const DT: f32 = 1.0 / 64.0;

    fn survival() -> Survival {
        Survival::from_ron(
            r#"(
                gauges: [
                    (name: "health", max: 20.0),
                    (name: "hunger", max: 100.0),
                    (name: "breath", max: 10.0, bar: Some((color: (0.4, 0.7, 1.0), show: NotFull))),
                    (name: "oxygen", max: 60.0),
                ],
                rules: [
                    (change: [("hunger", -1.0)]),
                    (when: [Gauge("hunger", (0.0, 0.0))], change: [("health", -1.0)]),
                    (when: [HeadInLiquid], change: [("breath", -1.0)]),
                    (when: [Not(HeadInLiquid)], change: [("breath", 5.0)]),
                    (when: [Gauge("breath", (0.0, 0.0))], change: [("health", -2.0)]),
                    (when: [Below(2.0)], change: [("oxygen", -1.0)]),
                ],
                fall: Some((gauge: "health", safe: 3.5, per_m: 1.0)),
                death: Some((gauge: "health")),
            )"#,
        )
        .unwrap()
    }

    /// Monde 16³ (1 vox/m) : sol de pierre en y ∈ [0, 4), un bassin d'eau
    /// de 4 m de profond en x ∈ [8, 12), un bloc de lave en (2, 4, 2).
    fn world() -> VoxelWorld {
        let reg = Registry::from_ron(
            r#"[
                (identifier: "t:air", kind: Block((solid: false, color: (0.0, 0.0, 0.0)))),
                (identifier: "t:stone", kind: Block((solid: true, color: (0.5, 0.5, 0.5)))),
                (identifier: "t:water", kind: Block((solid: false, color: (0.0, 0.0, 1.0), liquid: Some((spread: 7.0, delay: 0.25))))),
                (identifier: "t:lava", kind: Block((solid: false, color: (1.0, 0.4, 0.0), liquid: Some((spread: 3.0, delay: 1.0)),
                    touching: [("health", -4.0)]))),
            ]"#,
        )
        .unwrap();
        let id = |name| reg.lookup(name).unwrap();
        let (air, stone, water, lava) = (id("t:air"), id("t:stone"), id("t:water"), id("t:lava"));
        let mut chunk = Chunk::filled(16, air);
        for z in 0..16 {
            for x in 0..16 {
                for y in 0..4 {
                    chunk.set(x, y, z, stone);
                }
            }
        }
        for z in 0..16 {
            for x in 8..12 {
                for y in 4..10 {
                    chunk.set(x, y, z, water);
                }
            }
        }
        chunk.set(2, 4, 2, lava);
        let mut w = VoxelWorld::new(reg, 16, 1.0);
        w.insert_chunk(ChunkPos { x: 0, y: 0, z: 0 }, chunk);
        w
    }

    /// `seconds` de jeu au tick fixe, sans bouger : rend vrai si mort.
    fn live(s: &Survival, w: &VoxelWorld, body: &mut Body, vitals: &mut Vitals, seconds: f32) -> bool {
        for _ in 0..(seconds / DT).round() as usize {
            body.step(w, SIZE, EYES, [0.0, 0.0], false, DT);
            if vitals.step(s, w, body, SIZE, EYES, 1.0, DT) {
                return true;
            }
        }
        false
    }

    #[test]
    fn rules_drain_gauges_and_an_empty_one_kills() {
        let (s, w) = (survival(), world());
        let mut body = Body::at([4.5, 4.0, 4.5]);
        let mut vitals = Vitals::new(&s);
        assert!(!live(&s, &w, &mut body, &mut vitals, 10.0));
        let hunger = vitals.get(&s, "hunger").unwrap();
        assert!((hunger - 90.0).abs() < 0.1, "faim {hunger}");
        assert_eq!(vitals.get(&s, "health"), Some(20.0));
        assert_eq!(vitals.get(&s, "oxygen"), Some(60.0), "au-dessus de 2 m : pas d'oxygène perdu");
        // Faim vide : 1 point de vie par seconde, mort en 20 s.
        vitals.values[s.index("hunger").unwrap()] = 0.0;
        assert!(!live(&s, &w, &mut body, &mut vitals, 19.5));
        assert!(live(&s, &w, &mut body, &mut vitals, 1.0));
    }

    #[test]
    fn head_under_water_empties_breath_then_hurts() {
        let (s, w) = (survival(), world());
        // Au fond du bassin : 6 m d'eau, les yeux dedans.
        let mut body = Body::at([10.0, 4.0, 4.5]);
        let mut vitals = Vitals::new(&s);
        live(&s, &w, &mut body, &mut vitals, 9.0);
        assert!(body.feet[1] < 6.0, "le corps coule");
        assert!((vitals.get(&s, "breath").unwrap() - 1.0).abs() < 0.1);
        assert_eq!(vitals.get(&s, "health"), Some(20.0));
        live(&s, &w, &mut body, &mut vitals, 2.0);
        let health = vitals.get(&s, "health").unwrap();
        assert!((health - 18.0).abs() < 0.1, "1 s sans souffle à 2/s : {health}");
    }

    #[test]
    fn a_fall_hurts_per_metre_beyond_safe_and_a_jump_does_not() {
        let (s, w) = (survival(), world());
        // Le départ du monde est en l'air : y tomber ne blesse pas.
        let mut body = Body::at([4.5, 14.0, 4.5]);
        let mut vitals = Vitals::new(&s);
        live(&s, &w, &mut body, &mut vitals, 2.0);
        assert_eq!(vitals.get(&s, "health"), Some(20.0));
        // Après un contact avec le sol, si.
        body.feet[1] = 14.0;
        live(&s, &w, &mut body, &mut vitals, 2.0);
        let health = vitals.get(&s, "health").unwrap();
        // 10 m de chute, 3,5 m sans dégât : 6,5 points.
        assert!((health - 13.5).abs() < 0.05, "vie {health}");
        // Un saut (≈ 1,3 m) ne coûte rien.
        body.step(&w, SIZE, EYES, [0.0, 0.0], true, DT);
        live(&s, &w, &mut body, &mut vitals, 1.0);
        assert_eq!(vitals.get(&s, "health"), Some(health));
        // Tomber dans l'eau ne coûte rien non plus.
        let mut body = Body::at([12.5, 4.0, 4.5]);
        let mut vitals = Vitals::new(&s);
        live(&s, &w, &mut body, &mut vitals, 0.5);
        body.feet = [10.0, 15.0, 4.5];
        live(&s, &w, &mut body, &mut vitals, 2.0);
        assert_eq!(vitals.get(&s, "health"), Some(20.0));
    }

    #[test]
    fn a_teleport_measures_the_fall_from_where_it_lands() {
        let (s, w) = (survival(), world());
        let mut body = Body::at([4.5, 4.0, 4.5]);
        let mut vitals = Vitals::new(&s);
        live(&s, &w, &mut body, &mut vitals, 0.5);
        // En l'air à 14 m, puis téléporté au sol : pas de chute de 10 m.
        body.feet[1] = 14.0;
        live(&s, &w, &mut body, &mut vitals, DT);
        body.feet[1] = 4.0;
        vitals.teleported(4.0);
        live(&s, &w, &mut body, &mut vitals, 1.0);
        assert_eq!(vitals.get(&s, "health"), Some(20.0));
        // Téléporté 10 m au-dessus du sol : la chute compte.
        body.feet[1] = 14.0;
        vitals.teleported(14.0);
        live(&s, &w, &mut body, &mut vitals, 2.0);
        let health = vitals.get(&s, "health").unwrap();
        assert!((health - 13.5).abs() < 0.05, "vie {health}");
    }

    #[test]
    fn touching_lava_hurts_and_standing_next_to_it_does_not() {
        let (s, w) = (survival(), world());
        let mut beside = Body::at([1.5, 4.0, 4.5]); // 1 m de la lave en z
        let mut vitals = Vitals::new(&s);
        live(&s, &w, &mut beside, &mut vitals, 1.0);
        assert_eq!(vitals.get(&s, "health"), Some(20.0));
        let mut inside = Body::at([2.5, 4.0, 2.5]);
        live(&s, &w, &mut inside, &mut vitals, 1.0);
        let health = vitals.get(&s, "health").unwrap();
        assert!((health - 16.0).abs() < 0.1, "4/s pendant 1 s : {health}");
    }

    #[test]
    fn a_blow_to_zero_kills_even_when_a_rule_heals() {
        let s = Survival::from_ron(
            r#"(gauges: [(name: "health", max: 20.0)], rules: [(change: [("health", 0.1)])], death: Some((gauge: "health")))"#,
        )
        .unwrap();
        let w = world();
        let mut body = Body::at([4.5, 4.0, 4.5]);
        let mut vitals = Vitals::new(&s);
        vitals.apply(&s, &[("health".to_string(), -25.0)]);
        assert!(live(&s, &w, &mut body, &mut vitals, DT));
    }

    #[test]
    fn eating_changes_gauges_only_when_it_matters() {
        let s = survival();
        let mut vitals = Vitals::new(&s);
        let bread = [("hunger".to_string(), 30.0), ("unknown".to_string(), 5.0)];
        assert!(!vitals.apply(&s, &bread), "faim pleine : rien n'est mangé");
        vitals.values[s.index("hunger").unwrap()] = 80.0;
        assert!(vitals.apply(&s, &bread));
        assert_eq!(vitals.get(&s, "hunger"), Some(100.0), "bornée au max");
    }

    #[test]
    fn values_are_saved_by_name() {
        let s = survival();
        let mut vitals = Vitals::new(&s);
        vitals.values[0] = 7.0;
        let mut saved = vitals.saved(&s);
        saved.push(("renamed".into(), 1.0));
        saved.retain(|(name, _)| name != "hunger");
        let back = Vitals::restored(&s, &saved);
        assert_eq!(back.get(&s, "health"), Some(7.0));
        assert_eq!(back.get(&s, "hunger"), Some(100.0), "absente : à son départ");
    }

    #[test]
    fn bad_data_is_refused() {
        for (text, needle) in [
            ("(gauges: [(name: \"a\", max: 1.0), (name: \"a\", max: 1.0)])", "double"),
            ("(gauges: [(name: \"a\", max: 1.0, start: Some(2.0))])", "start"),
            ("(gauges: [(name: \"a\", max: 1.0)], rules: [(change: [(\"b\", 1.0)])])", "inconnue"),
            ("(gauges: [(name: \"a\", max: 1.0)], rules: [(when: [Not(Gauge(\"b\", (0.0, 1.0)))], change: [])])", "inconnue"),
            ("(gauges: [(name: \"a\", max: 1.0)], rules: [(when: [Daylight((1.0, 0.0))], change: [])])", "plage"),
            ("(gauges: [(name: \"a\", max: 1.0)], death: Some((gauge: \"b\")))", "inconnue"),
            ("(gauges: [(name: \"a\", max: 1.0)], fall: Some((gauge: \"a\", safe: -1.0, per_m: 1.0)))", "fall"),
        ] {
            let err = Survival::from_ron(text).unwrap_err();
            assert!(err.contains(needle), "{text} → {err}");
        }
    }

    #[test]
    fn the_game_survival_file_is_valid() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content/survival.ron");
        let s = Survival::from_ron(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert!(s.death.is_some());
    }
}
