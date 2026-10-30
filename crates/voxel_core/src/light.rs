//! Lumière (#23, §3.8) : deux lumières par case, celle du ciel et celle des
//! blocs, propagées de case en case comme dans Minecraft.
//!
//! Dérivée des voxels, jamais sauvée : calculée quand un chunk arrive
//! ([`Light::insert`]) et mise à jour quand un voxel change
//! ([`Light::update`]), dans la même frame.
//!
//! **Niveaux** : en quarts de case ([`STEPS_PER_VOXEL`]) ; traverser une case
//! d'air en coûte 4. Une portée en mètres devient `mètres × résolution × 4`,
//! plafonnée à 255 (un octet). Le quart de case rend `light_loss`
//! fractionnaire possible (0,5 = 2 quarts de plus par case).
//!
//! **Ciel** : une case sous le ciel ouvert est pleine ([`full_sky`]). La
//! lumière pleine descend sans perte dans ce qui ne la freine pas ; partout
//! ailleurs, elle perd une case par pas, plus le `light_loss` de la case où
//! elle entre. **Un chunk sans données au-dessus de lui est sous le ciel
//! ouvert** : c'est au streaming de charger d'abord ce qui pourrait le
//! couvrir.
//!
//! **Propagation** : en largeur (une file). Pour éteindre (bloc posé,
//! lampe retirée), on retire d'abord tout ce qui pouvait venir de la case,
//! puis on repropage depuis les cases restées allumées au bord de la zone
//! éteinte.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::chunk::{Chunk, ChunkPos, meshes_reading};
use crate::registry::{Alpha, ContentId, Registry};
use crate::world::neighbors;

/// Index de la lumière du ciel dans `[Channel; 2]`.
pub const SKY: usize = 0;
/// Index de la lumière des blocs.
pub const BLOCK: usize = 1;

/// Niveaux par case traversée.
pub const STEPS_PER_VOXEL: f32 = 4.0;

/// Distance qu'étale la lumière du ciel sur le côté, en mètres (sous un
/// surplomb, à l'entrée d'une grotte).
pub const SKY_RANGE_M: f32 = 15.0;

/// Rang de « vers le bas » dans [`neighbors`].
const DOWN: usize = 5;

/// Niveau d'une portée en mètres, à cette résolution.
pub fn level_of(range_m: f32, voxels_per_meter: f32) -> u8 {
    (range_m * voxels_per_meter * STEPS_PER_VOXEL).round().clamp(0.0, 255.0) as u8
}

/// Niveau du ciel ouvert, l'unité des deux lumières côté rendu.
pub fn full_sky(voxels_per_meter: f32) -> u8 {
    level_of(SKY_RANGE_M, voxels_per_meter).max(1)
}

/// Une lumière d'un chunk : une valeur par case (layout de
/// [`Chunk::get_local`]), ou une seule quand le chunk est uniforme (air au
/// soleil, roche pleine).
#[derive(Debug, Clone, PartialEq)]
pub enum Channel {
    Uniform(u8),
    Dense(Box<[u8]>),
}

impl Channel {
    pub fn get(&self, i: usize) -> u8 {
        match self {
            Self::Uniform(v) => *v,
            Self::Dense(a) => a[i],
        }
    }

    fn set(&mut self, i: usize, v: u8, volume: usize) {
        match self {
            Self::Uniform(u) if *u == v => {}
            Self::Uniform(u) => {
                let mut a = vec![*u; volume].into_boxed_slice();
                a[i] = v;
                *self = Self::Dense(a);
            }
            Self::Dense(a) => a[i] = v,
        }
    }

    fn compact(values: Vec<u8>) -> Self {
        if values.iter().all(|&v| v == values[0]) { Self::Uniform(values[0]) } else { Self::Dense(values.into()) }
    }
}

/// Ce que la lumière lit d'un bloc.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Props {
    /// Cube solide et opaque : arrête la lumière (comme l'ombrage des
    /// coins). Un bloc à forme la laisse passer (#14).
    stops: bool,
    /// Niveaux perdus en plus en entrant dans la case.
    loss: u8,
    /// Lumière émise.
    emits: u8,
}

fn props(registry: &Registry, id: ContentId, vpm: f32) -> Props {
    let Some(b) = registry.get(id).and_then(|e| e.block()) else { return Props::default() };
    Props {
        stops: b.solid && b.alpha == Alpha::Opaque && b.shape.is_none(),
        loss: (b.light_loss * STEPS_PER_VOXEL).round() as u8,
        emits: b.emits_light.map_or(0, |r| level_of(r, vpm)),
    }
}

/// Niveau qu'une case à `level` donne à sa voisine `to` (`down` : la
/// voisine est dessous).
fn spread(c: usize, full: u8, level: u8, down: bool, to: Props) -> u8 {
    if to.stops {
        return 0;
    }
    let step = if c == SKY && down && level == full && to.loss == 0 { 0 } else { STEPS_PER_VOXEL as u8 };
    level.saturating_sub(step.saturating_add(to.loss))
}

/// Ce que la lumière lit du monde : les voxels et le registre.
pub(crate) struct Ctx<'a> {
    pub chunks: &'a HashMap<ChunkPos, Chunk>,
    pub registry: &'a Registry,
    pub size: u32,
    pub vpm: f32,
}

impl Ctx<'_> {
    fn split(&self, v: [i64; 3]) -> (ChunkPos, usize) {
        let s = self.size as i64;
        let pos = ChunkPos { x: v[0].div_euclid(s) as i32, y: v[1].div_euclid(s) as i32, z: v[2].div_euclid(s) as i32 };
        let [x, y, z] = v.map(|c| c.rem_euclid(s) as usize);
        (pos, x + self.size as usize * (y + self.size as usize * z))
    }

    /// `None` : pas de données à cet endroit.
    fn props_at(&self, v: [i64; 3]) -> Option<Props> {
        let (pos, i) = self.split(v);
        let chunk = self.chunks.get(&pos)?;
        Some(props(self.registry, chunk.palette()[chunk.voxels()[i] as usize], self.vpm))
    }

    fn full(&self) -> u8 {
        full_sky(self.vpm)
    }
}

/// La lumière de tous les chunks qui ont des données.
#[derive(Debug, Default)]
pub struct Light {
    chunks: HashMap<ChunkPos, [Channel; 2]>,
    /// Chunks dont le mesh lit une case qui a changé, à re-mesher.
    changed: HashSet<ChunkPos>,
}

impl Light {
    /// `[ciel, blocs]` du chunk `pos`.
    pub fn chunk(&self, pos: ChunkPos) -> Option<&[Channel; 2]> {
        self.chunks.get(&pos)
    }

    /// Les chunks à re-mesher depuis le dernier appel.
    pub fn take_changed(&mut self) -> Vec<ChunkPos> {
        self.changed.drain().collect()
    }

    fn level(&self, ctx: &Ctx, c: usize, v: [i64; 3]) -> Option<u8> {
        let (pos, i) = ctx.split(v);
        self.chunks.get(&pos).map(|l| l[c].get(i))
    }

    pub(crate) fn levels_at(&self, ctx: &Ctx, v: [i64; 3]) -> Option<[u8; 2]> {
        Some([self.level(ctx, SKY, v)?, self.level(ctx, BLOCK, v)?])
    }

    fn set(&mut self, ctx: &Ctx, c: usize, v: [i64; 3], value: u8) {
        let (pos, i) = ctx.split(v);
        if let Some(l) = self.chunks.get_mut(&pos) {
            l[c].set(i, value, (ctx.size as usize).pow(3));
            self.changed.extend(meshes_reading(v, ctx.size));
        }
    }

    /// Ce que la case `v` reçoit sans voisin : la lumière qu'elle émet, ou
    /// le ciel ouvert si rien n'a de données au-dessus d'elle.
    fn source(&self, ctx: &Ctx, c: usize, v: [i64; 3], p: Props) -> u8 {
        if c == BLOCK {
            p.emits
        } else if ctx.props_at([v[0], v[1] + 1, v[2]]).is_none() {
            let full = ctx.full();
            spread(SKY, full, full, true, p)
        } else {
            0
        }
    }

    /// Calcule la lumière du chunk `pos`, qui vient d'arriver, et la
    /// propage chez ses voisins (et la leur chez lui).
    pub(crate) fn insert(&mut self, ctx: &Ctx, pos: ChunkPos) {
        let chunk = &ctx.chunks[&pos];
        let s = ctx.size as usize;
        let volume = s * s * s;
        let full = ctx.full();
        let index = |x: usize, y: usize, z: usize| x + s * (y + s * z);
        let palette: Vec<Props> = chunk.palette().iter().map(|&id| props(ctx.registry, id, ctx.vpm)).collect();
        let props_of = |i: usize| palette[chunk.voxels()[i] as usize];

        // 1. Chaque colonne : le ciel qui descend d'en haut, les blocs qui
        // émettent.
        let above = self.chunks.get(&ChunkPos { y: pos.y + 1, ..pos });
        let mut values = [vec![0u8; volume], vec![0u8; volume]];
        for z in 0..s {
            for x in 0..s {
                let mut level = above.map_or(full, |l| l[SKY].get(index(x, 0, z)));
                for y in (0..s).rev() {
                    let i = index(x, y, z);
                    level = spread(SKY, full, level, true, props_of(i));
                    values[SKY][i] = level;
                    values[BLOCK][i] = props_of(i).emits;
                }
            }
        }

        // 2. Les cases d'où la lumière doit encore s'étaler dans le chunk.
        let mut seeds: [Vec<[i64; 3]>; 2] = [Vec::new(), Vec::new()];
        let s32 = s as i64;
        let origin = [pos.x as i64 * s32, pos.y as i64 * s32, pos.z as i64 * s32];
        for c in [SKY, BLOCK] {
            let vals = &values[c];
            if vals.iter().all(|&v| v == vals[0]) {
                continue;
            }
            for z in 0..s {
                for y in 0..s {
                    for x in 0..s {
                        let i = index(x, y, z);
                        if vals[i] == 0 {
                            continue;
                        }
                        let p = [x as i64, y as i64, z as i64];
                        let lights_neighbor = neighbors(p).into_iter().enumerate().any(|(dir, n)| {
                            n.iter().all(|&k| (0..s32).contains(&k)) && {
                                let j = index(n[0] as usize, n[1] as usize, n[2] as usize);
                                spread(c, full, vals[i], dir == DOWN, props_of(j)) > vals[j]
                            }
                        });
                        if lights_neighbor {
                            seeds[c].push([origin[0] + p[0], origin[1] + p[1], origin[2] + p[2]]);
                        }
                    }
                }
            }
        }
        let [sky, block] = values;
        self.chunks.insert(pos, [Channel::compact(sky), Channel::compact(block)]);
        self.changed.insert(pos);

        // 3. Le chunk dessous se croyait sous le ciel ouvert (le streaming
        // charge d'habitude le haut d'abord) : ce que ce chunk lui cache
        // s'éteint.
        if self.chunks.contains_key(&ChunkPos { y: pos.y - 1, ..pos }) {
            let mut starts = Vec::new();
            for z in 0..s32 {
                for x in 0..s32 {
                    let t = [origin[0] + x, origin[1] - 1, origin[2] + z];
                    let (Some(lt), Some(pt)) = (self.level(ctx, SKY, t), ctx.props_at(t)) else { continue };
                    let lit = spread(SKY, full, self.level(ctx, SKY, [t[0], t[1] + 1, t[2]]).unwrap_or(0), true, pt);
                    if lt > lit && lt == spread(SKY, full, full, true, pt) {
                        self.set(ctx, SKY, t, lit);
                        starts.push((t, lt));
                    }
                }
            }
            let relight = self.unspread(ctx, SKY, starts);
            seeds[SKY].extend(relight);
        }

        // 4. Les deux côtés de chaque face partagée avec un voisin chargé :
        // la lumière passe dans les deux sens.
        let level_and_props = |v: [i64; 3], c: usize| Some((self.level(ctx, c, v)?, ctx.props_at(v)?));
        for (dir, d) in neighbors([0, 0, 0]).into_iter().enumerate() {
            let axis = d.iter().position(|&k| k != 0).unwrap_or(0);
            let (u, w) = ((axis + 1) % 3, (axis + 2) % 3);
            // Opposé de `dir` dans l'ordre de `neighbors` : 0↔1, 2↔3, 4↔5.
            let back = dir ^ 1;
            for a in 0..s32 {
                for b in 0..s32 {
                    let mut inner = [0i64; 3];
                    inner[axis] = if d[axis] > 0 { s32 - 1 } else { 0 };
                    inner[u] = a;
                    inner[w] = b;
                    let here = [origin[0] + inner[0], origin[1] + inner[1], origin[2] + inner[2]];
                    let there = [here[0] + d[0], here[1] + d[1], here[2] + d[2]];
                    for c in [SKY, BLOCK] {
                        let (Some((lh, ph)), Some((lt, pt))) = (level_and_props(here, c), level_and_props(there, c)) else {
                            continue;
                        };
                        if spread(c, full, lh, dir == DOWN, pt) > lt {
                            seeds[c].push(here);
                        }
                        if spread(c, full, lt, back == DOWN, ph) > lh {
                            seeds[c].push(there);
                        }
                    }
                }
            }
        }
        for c in [SKY, BLOCK] {
            let queue = std::mem::take(&mut seeds[c]).into();
            self.spread_from(ctx, c, queue);
        }
    }

    /// Met à jour la lumière autour du voxel `v`, qui était `old`.
    pub(crate) fn update(&mut self, ctx: &Ctx, v: [i64; 3], old: ContentId) {
        let Some(p) = ctx.props_at(v) else { return };
        if p == props(ctx.registry, old, ctx.vpm) {
            return; // pierre → terre : la lumière ne voit pas la différence
        }
        for c in [SKY, BLOCK] {
            let Some(before) = self.level(ctx, c, v) else { return };
            let src = self.source(ctx, c, v, p);
            self.set(ctx, c, v, src);
            // Les voisins encore allumés reviennent de `unspread` : ils
            // rééclairent `v` si c'est un bloc cassé.
            let mut relight = self.unspread(ctx, c, vec![(v, before)]);
            relight.push_back(v);
            self.spread_from(ctx, c, relight);
        }
    }

    /// Étale la lumière depuis les cases de `queue`.
    fn spread_from(&mut self, ctx: &Ctx, c: usize, mut queue: VecDeque<[i64; 3]>) {
        let full = ctx.full();
        while let Some(v) = queue.pop_front() {
            let Some(level) = self.level(ctx, c, v).filter(|&l| l > 0) else { continue };
            for (dir, n) in neighbors(v).into_iter().enumerate() {
                let (Some(p), Some(there)) = (ctx.props_at(n), self.level(ctx, c, n)) else { continue };
                let new = spread(c, full, level, dir == DOWN, p);
                if new > there {
                    self.set(ctx, c, n, new);
                    queue.push_back(n);
                }
            }
        }
    }

    /// Éteint ce qui pouvait venir des cases `starts` (case, ancien niveau).
    /// Rend les cases restées allumées au bord de la zone éteinte, d'où
    /// repropager.
    fn unspread(&mut self, ctx: &Ctx, c: usize, starts: Vec<([i64; 3], u8)>) -> VecDeque<[i64; 3]> {
        let full = ctx.full();
        let mut queue: VecDeque<_> = starts.into();
        let mut relight = VecDeque::new();
        while let Some((v, old)) = queue.pop_front() {
            for (dir, n) in neighbors(v).into_iter().enumerate() {
                let (Some(p), Some(level)) = (ctx.props_at(n), self.level(ctx, c, n)) else { continue };
                if level == 0 {
                    continue;
                }
                let src = self.source(ctx, c, n, p);
                if level <= spread(c, full, old, dir == DOWN, p) && level > src {
                    // Elle tenait peut-être sa lumière de `v` : éteinte, sauf
                    // ce qu'elle reçoit d'elle-même.
                    self.set(ctx, c, n, src);
                    queue.push_back((n, level));
                    if src > 0 {
                        relight.push_back(n);
                    }
                } else {
                    // Éclairée par ailleurs : elle rallumera la zone.
                    relight.push_back(n);
                }
            }
        }
        relight
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ContentEntry;
    use crate::world::VoxelWorld;

    const SIZE: u32 = 8;

    struct Ids {
        air: ContentId,
        stone: ContentId,
        lamp: ContentId,
        water: ContentId,
    }

    fn registry() -> (Registry, Ids) {
        let mut reg = Registry::new();
        let air = reg.register(ContentEntry::new_block("t:air", false, [0.0; 3])).unwrap();
        let stone = reg.register(ContentEntry::new_block("t:stone", true, [0.5; 3])).unwrap();
        let mut lamp = ContentEntry::new_block("t:lamp", true, [1.0; 3]);
        let mut water = ContentEntry::new_block("t:water", false, [0.2; 3]);
        if let (crate::registry::Kind::Block(l), crate::registry::Kind::Block(w)) = (&mut lamp.kind, &mut water.kind) {
            l.emits_light = Some(3.0);
            w.alpha = Alpha::Blend(0.5);
            w.light_loss = 1.0;
        }
        let lamp = reg.register(lamp).unwrap();
        let water = reg.register(water).unwrap();
        (reg, Ids { air, stone, lamp, water })
    }

    /// Chunks de `SIZE` sur `[0, n)³`, remplis par `fill` (coordonnées monde),
    /// insérés du haut vers le bas comme le fait le streaming.
    fn world(n: i32, fill: impl Fn([i64; 3]) -> ContentId) -> VoxelWorld {
        let (reg, ids) = registry();
        let mut world = VoxelWorld::new(reg, SIZE, 1.0);
        for y in (0..n).rev() {
            for z in 0..n {
                for x in 0..n {
                    let mut chunk = Chunk::filled(SIZE, ids.air);
                    let s = SIZE as i64;
                    for (lx, ly, lz) in (0..SIZE).flat_map(|a| (0..SIZE).flat_map(move |b| (0..SIZE).map(move |c| (a, b, c)))) {
                        let v = [x as i64 * s + lx as i64, y as i64 * s + ly as i64, z as i64 * s + lz as i64];
                        chunk.set(lx, ly, lz, fill(v));
                    }
                    world.insert_chunk(ChunkPos { x, y, z }, chunk);
                }
            }
        }
        world
    }

    fn ids() -> Ids {
        registry().1
    }

    /// La lumière de `world` est celle d'un calcul repris de zéro sur les
    /// mêmes voxels.
    fn assert_same_as_fresh(world: &VoxelWorld, n: i32) {
        let fresh = self::world(n, |v| world.voxel(v).unwrap());
        let span = n as i64 * SIZE as i64;
        for z in 0..span {
            for y in 0..span {
                for x in 0..span {
                    assert_eq!(world.light_at([x, y, z]), fresh.light_at([x, y, z]), "en {:?}", [x, y, z]);
                }
            }
        }
    }

    #[test]
    fn open_sky_is_full_and_ground_is_dark() {
        let ids = ids();
        let world = world(2, |v| if v[1] < 4 { ids.stone } else { ids.air });
        let full = world.full_light();
        assert_eq!(full, 60, "15 m × 1 vox/m × 4");
        assert_eq!(world.light_at([5, 15, 5]), [full, 0]);
        assert_eq!(world.light_at([5, 4, 5]), [full, 0], "descend sans perte jusqu'au sol");
        assert_eq!(world.light_at([5, 3, 5]), [0, 0], "la pierre arrête tout");
    }

    #[test]
    fn sky_light_fades_under_an_overhang() {
        let ids = ids();
        // Toit à y = 12 sur x < 10, sol à y < 2 : sous le toit, la lumière
        // vient du côté ouvert et perd une case (4 niveaux) par pas.
        let world = world(2, |v| if v[1] < 2 || (v[1] == 12 && v[0] < 10) { ids.stone } else { ids.air });
        let full = world.full_light();
        assert_eq!(world.light_at([10, 5, 3]), [full, 0]);
        assert_eq!(world.light_at([9, 5, 3]), [full - 4, 0]);
        assert_eq!(world.light_at([6, 5, 3]), [full - 16, 0]);
        assert_same_as_fresh(&world, 2);
    }

    #[test]
    fn a_lamp_lights_its_cave_and_goes_out() {
        let ids = ids();
        // Tout en pierre sauf une grotte horizontale à y = 5.
        let mut world = world(2, |v| if v[1] == 5 && v[2] == 3 { ids.air } else { ids.stone });
        let lamp = [3, 5, 3];
        world.set_voxel(lamp, ids.lamp);
        assert_eq!(world.light_at(lamp), [0, 12], "3 m × 4");
        assert_eq!(world.light_at([4, 5, 3]), [0, 8]);
        assert_eq!(world.light_at([6, 5, 3]), [0, 0], "portée atteinte");
        assert_same_as_fresh(&world, 2);
        world.set_voxel(lamp, ids.air);
        assert_eq!(world.light_at([4, 5, 3]), [0, 0]);
        assert_same_as_fresh(&world, 2);
    }

    #[test]
    fn a_roof_darkens_what_is_below_and_breaking_it_relights() {
        let ids = ids();
        // Puits d'un bloc de large dans la pierre, ouvert sur le ciel.
        let mut world = world(2, |v| if (v[0] == 4 && v[2] == 4 && v[1] >= 2) || v[1] >= 13 { ids.air } else { ids.stone });
        let full = world.full_light();
        assert_eq!(world.light_at([4, 2, 4])[SKY], full, "le fond du puits voit le ciel");
        world.set_voxel([4, 12, 4], ids.stone);
        assert_eq!(world.light_at([4, 2, 4])[SKY], 0);
        assert_same_as_fresh(&world, 2);
        world.set_voxel([4, 12, 4], ids.air);
        assert_eq!(world.light_at([4, 2, 4])[SKY], full);
        assert_same_as_fresh(&world, 2);
    }

    #[test]
    fn water_dims_the_sky_with_depth() {
        let ids = ids();
        let world = world(2, |v| if v[1] < 2 { ids.stone } else if v[1] < 12 { ids.water } else { ids.air });
        let full = world.full_light();
        // `light_loss: 1.0` : chaque case d'eau coûte deux cases.
        assert_eq!(world.light_at([3, 11, 3])[SKY], full - 8);
        assert_eq!(world.light_at([3, 10, 3])[SKY], full - 16);
        assert_eq!(world.light_at([3, 5, 3])[SKY], full - 7 * 8);
        assert_eq!(world.light_at([3, 4, 3])[SKY], 0, "8 m d'eau : noir");
    }

    #[test]
    fn a_chunk_arriving_above_shades_the_one_below() {
        let ids = ids();
        let (reg, _) = registry();
        let mut world = VoxelWorld::new(reg, SIZE, 1.0);
        // Dessous d'abord : il se croit sous le ciel ouvert...
        world.insert_chunk(ChunkPos { x: 0, y: 0, z: 0 }, Chunk::filled(SIZE, ids.air));
        assert_eq!(world.light_at([3, 3, 3])[SKY], world.full_light());
        // ... puis un chunk de pierre arrive au-dessus.
        world.insert_chunk(ChunkPos { x: 0, y: 1, z: 0 }, Chunk::filled(SIZE, ids.stone));
        assert_eq!(world.light_at([3, 3, 3])[SKY], 0);
    }

    /// Un millier de poses et de casses au hasard (graine fixe) : la
    /// lumière incrémentale reste celle d'un calcul de zéro.
    #[test]
    fn random_edits_match_a_fresh_computation() {
        let ids = ids();
        let mut world = world(2, |v| if v[1] < 6 { ids.stone } else { ids.air });
        let blocks = [ids.air, ids.stone, ids.lamp, ids.water];
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut next = |m: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % m
        };
        for round in 0..1000 {
            let v = [next(16) as i64, next(16) as i64, next(16) as i64];
            world.set_voxel(v, blocks[next(4) as usize]);
            if round % 100 == 99 {
                assert_same_as_fresh(&world, 2);
            }
        }
    }

    /// Contenu rechargé : un bloc déjà posé se met à éclairer.
    #[test]
    fn relight_follows_a_registry_change() {
        let ids = ids();
        let mut world = world(1, |v| if v == [3, 3, 3] { ids.stone } else { ids.air });
        assert_eq!(world.light_at([4, 3, 3])[BLOCK], 0);
        let mut entry = world.registry.get(ids.stone).unwrap().clone();
        if let crate::registry::Kind::Block(b) = &mut entry.kind {
            b.emits_light = Some(2.0);
        }
        let mut registry = Registry::new();
        for (id, e) in world.registry.iter() {
            registry.register(if id == ids.stone { entry.clone() } else { e.clone() }).unwrap();
        }
        world.registry = registry;
        world.relight();
        assert_eq!(world.light_at([4, 3, 3])[BLOCK], 4);
    }
}
