//! Liquides (#28, §3.6) : des blocs qui coulent, réglés en donnée.
//!
//! **Modèle source + écoulement.** Une source (voxel liquide sans niveau)
//! ne s'épuise pas. Le liquide qui en coule perd un niveau par voxel
//! parcouru à plat et s'arrête après `spread` mètres ; il tombe d'abord,
//! s'étale ensuite. Retirer la source fait refluer ce qui en coulait.
//!
//! **Chaque voxel se demande ce qu'il devrait être** d'après ses voisins
//! ([`update`]), au lieu qu'un liquide pousse dans les cases d'à côté. Le
//! résultat ne dépend donc pas de l'ordre de traitement d'un même tick, et
//! le reflux n'est pas un cas à part : une case qui ne reçoit plus rien
//! redevient de l'air.
//!
//! **Coût nul au repos.** Rien ne tourne en continu : un voxel n'est
//! réexaminé que si lui ou un voisin a changé (`VoxelWorld::set_voxel`
//! programme le réexamen). Un océan immobile ne coûte rien.
//!
//! Le niveau vit hors du voxel, dans une table creuse (§3.2) : voir
//! [`VoxelWorld::liquid_level`].

use crate::registry::{ContentId, Liquid};
use crate::world::{neighbors, VoxelWorld, TICKS_PER_SECOND};

/// Hauteur d'une source ou d'une chute sous de l'air, en part de voxel :
/// sous 1, la surface se distingue d'un bloc plein.
pub const SURFACE: f32 = 0.9;

/// Ticks fixes d'un pas d'écoulement (au moins un).
pub fn step_ticks(liquid: &Liquid) -> u64 {
    ((liquid.delay * TICKS_PER_SECOND as f32).ceil() as u64).max(1)
}

/// Voxels parcourus à plat depuis une source : `spread` en mètres converti
/// à la résolution du monde (§2).
pub fn reach(liquid: &Liquid, voxels_per_meter: f32) -> u8 {
    (liquid.spread * voxels_per_meter).round().clamp(1.0, 254.0) as u8
}

/// Hauteur de la surface d'un liquide de niveau `level` (voir
/// [`VoxelWorld::liquid_level`]), en part de voxel.
pub fn height(level: Option<u8>, reach: u8) -> f32 {
    match level {
        None | Some(0) => SURFACE,
        Some(d) => SURFACE * (1.0 - f32::from(d) / (f32::from(reach) + 1.0)),
    }
}

/// Réexamine le voxel `v` : ce qu'il doit devenir (bloc, niveau), ou `None`
/// s'il ne change pas. `air` est le seul bloc où un liquide entre : il
/// n'efface jamais rien. C'est l'appelant qui écrit le changement
/// (`set_voxel` puis `set_liquid_level`), avec ses propres effets
/// (re-mesh…).
pub fn update(world: &mut VoxelWorld, air: ContentId, v: [i64; 3]) -> Option<(ContentId, Option<u8>)> {
    let block = world.voxel(v)?;
    let liquid = world.liquid(block);
    if block != air && liquid.is_none() {
        return None;
    }
    let now = (block != air).then(|| (block, world.liquid_level(v)));
    // Une source ne bouge jamais ; le reste suit ses voisins.
    let wanted = if matches!(now, Some((_, None))) { now } else { feed(world, air, v) };
    if wanted == now {
        // Rien ne change ici, mais l'air d'à côté ou du dessous a pu se
        // libérer (un bloc cassé) : c'est lui qui doit se réexaminer.
        if let Some(l) = liquid {
            let [px, nx, pz, nz, _, down] = neighbors(v);
            for n in [px, nx, pz, nz, down] {
                if world.voxel(n) == Some(air) {
                    world.schedule(n, step_ticks(&l));
                }
            }
        }
        return None;
    }
    Some(wanted.unwrap_or((air, None)))
}

/// Ce que les voisins de `v` y font couler, s'il y a quelque chose.
fn feed(world: &VoxelWorld, air: ContentId, v: [i64; 3]) -> Option<(ContentId, Option<u8>)> {
    let [x, y, z] = v;
    let below = [x, y - 1, z];
    let sides: Vec<([i64; 3], ContentId, Liquid)> = neighbors(v)[..4]
        .iter()
        .filter_map(|&n| {
            let id = world.voxel(n)?;
            Some((n, id, world.liquid(id)?))
        })
        .collect();

    // Deux sources voisines d'un liquide renouvelable, posées sur du solide
    // ou sur une source, en créent une : la plus petite ID gagne (ordre
    // fixe).
    let supported = |id| {
        world.voxel(below).is_some_and(|b| world.is_solid(below) || (b == id && world.liquid_level(below).is_none()))
    };
    let renewed = sides
        .iter()
        .filter(|(n, id, l)| {
            l.renewable
                && world.liquid_level(*n).is_none()
                && sides.iter().filter(|(m, j, _)| j == id && world.liquid_level(*m).is_none()).count() >= 2
                && supported(*id)
        })
        .map(|&(_, id, _)| id)
        .min_by_key(|id| id.0);
    if let Some(id) = renewed {
        return Some((id, None));
    }

    // Un liquide au-dessus tombe ici.
    if let Some(up) = world.voxel([x, y + 1, z])
        && world.liquid(up).is_some()
    {
        return Some((up, Some(0)));
    }

    // Sinon, le plus proche d'une source à côté s'étale ici. Un voisin qui
    // peut tomber ne s'étale pas (l'eau descend d'abord).
    let vpm = world.voxels_per_meter();
    sides
        .iter()
        .filter(|(n, id, _)| world.liquid_level(*n).is_none() || !falls(world, air, *n, *id))
        .filter_map(|&(n, id, l)| {
            let d = world.liquid_level(n).unwrap_or(0) + 1;
            (d <= reach(&l, vpm)).then_some((d, id))
        })
        .min_by_key(|&(d, id)| (d, id.0))
        .map(|(d, id)| (id, Some(d)))
}

/// Le liquide `id` en `n` coule-t-il vers le bas plutôt que sur les côtés ?
/// Oui sur de l'air, ou au-dessus de sa propre chute ; non sur du solide ou
/// un lac. Un dessous non chargé compte comme une chute : on ne s'étale pas
/// au bord du monde chargé.
fn falls(world: &VoxelWorld, air: ContentId, n: [i64; 3], id: ContentId) -> bool {
    let below = [n[0], n[1] - 1, n[2]];
    match world.voxel(below) {
        None => true,
        Some(b) => b == air || (b == id && world.liquid_level(below) == Some(0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{Chunk, ChunkPos};
    use crate::registry::Registry;
    use crate::world::MAX_UPDATES_PER_TICK;

    /// Monde 16³ : sol de pierre en y = 0, air au-dessus. Eau : 3 m,
    /// un pas par tick ; lave : 1 m, renouvelable non.
    fn world() -> (VoxelWorld, ContentId, ContentId, ContentId) {
        let reg = Registry::from_ron(
            r#"[
                (identifier: "t:air", kind: Block((solid: false, color: (0.0, 0.0, 0.0)))),
                (identifier: "t:stone", kind: Block((solid: true, color: (0.5, 0.5, 0.5)))),
                (identifier: "t:water", kind: Block((solid: false, color: (0.2, 0.3, 0.9),
                    liquid: Some((spread: 3.0, delay: 0.0, renewable: true))))),
            ]"#,
        )
        .unwrap();
        let id = |s| reg.lookup(s).unwrap();
        let (air, stone, water) = (id("t:air"), id("t:stone"), id("t:water"));
        let mut w = VoxelWorld::new(reg, 16, 1.0);
        let mut chunk = Chunk::filled(16, air);
        for (x, z) in (0..16).flat_map(|x| (0..16).map(move |z| (x, z))) {
            chunk.set(x, 0, z, stone);
        }
        w.insert_chunk(ChunkPos { x: 0, y: 0, z: 0 }, chunk);
        (w, air, stone, water)
    }

    /// Fait tourner l'écoulement `ticks` ticks, comme le jeu.
    fn run(w: &mut VoxelWorld, air: ContentId, ticks: u32) {
        for _ in 0..ticks {
            w.advance();
            for v in w.take_due_updates(MAX_UPDATES_PER_TICK) {
                if let Some((id, level)) = update(w, air, v) {
                    w.set_voxel(v, id);
                    w.set_liquid_level(v, level);
                }
            }
        }
    }

    fn state(w: &VoxelWorld, v: [i64; 3]) -> (ContentId, Option<u8>) {
        (w.voxel(v).unwrap(), w.liquid_level(v))
    }

    #[test]
    fn a_source_spreads_its_reach_then_stops() {
        let (mut w, air, _, water) = world();
        w.set_voxel([8, 1, 8], water);
        run(&mut w, air, 20);
        assert_eq!(state(&w, [8, 1, 8]), (water, None), "la source reste");
        assert_eq!(state(&w, [9, 1, 8]), (water, Some(1)));
        assert_eq!(state(&w, [11, 1, 8]), (water, Some(3)));
        assert_eq!(state(&w, [12, 1, 8]), (air, None), "3 m, pas plus");
        assert_eq!(state(&w, [9, 1, 9]), (water, Some(2)), "distance à plat, pas en diagonale");
        assert_eq!(state(&w, [8, 2, 8]), (air, None), "ne monte pas");
        assert_eq!(w.take_due_updates(usize::MAX), Vec::<[i64; 3]>::new(), "au repos, plus rien à faire");
    }

    #[test]
    fn removing_the_source_drains_what_flowed() {
        let (mut w, air, _, water) = world();
        w.set_voxel([8, 1, 8], water);
        run(&mut w, air, 20);
        w.set_voxel([8, 1, 8], air);
        run(&mut w, air, 20);
        for x in 5..12 {
            assert_eq!(state(&w, [x, 1, 8]), (air, None), "x = {x}");
        }
    }

    #[test]
    fn water_falls_first_then_spreads_where_it_lands() {
        let (mut w, air, stone, water) = world();
        // Une marche : source sur un pilier en (8, 4, 8), le sol est en y = 0.
        for y in 1..4 {
            w.set_voxel([8, y, 8], stone);
        }
        w.set_voxel([8, 4, 8], water);
        run(&mut w, air, 30);
        // Sur le pilier elle s'étale d'un cran, puis tombe à côté.
        assert_eq!(state(&w, [9, 4, 8]), (water, Some(1)));
        assert_eq!(state(&w, [10, 4, 8]), (air, None), "ne s'étale pas au-dessus du vide");
        assert_eq!(state(&w, [9, 2, 8]), (water, Some(0)), "chute");
        // Arrivée au sol, la chute repart comme une source : 3 m.
        assert_eq!(state(&w, [10, 1, 8]), (water, Some(1)));
        assert_eq!(state(&w, [12, 1, 8]), (water, Some(3)));
        assert_eq!(state(&w, [13, 1, 8]), (air, None));
    }

    #[test]
    fn two_sources_fill_the_gap_between_them_with_a_source() {
        let (mut w, air, _, water) = world();
        w.set_voxel([7, 1, 8], water);
        w.set_voxel([9, 1, 8], water);
        run(&mut w, air, 5);
        assert_eq!(state(&w, [8, 1, 8]), (water, None));
        // Les deux autres retirées, celle du milieu reste : c'est une source.
        w.set_voxel([7, 1, 8], air);
        w.set_voxel([9, 1, 8], air);
        run(&mut w, air, 5);
        assert_eq!(state(&w, [8, 1, 8]), (water, None));
    }

    #[test]
    fn a_broken_wall_lets_a_still_pool_flow() {
        let (mut w, air, stone, water) = world();
        // Une source murée : rien ne bouge, rien n'est programmé.
        for n in &neighbors([8, 1, 8])[..4] {
            w.set_voxel(*n, stone);
        }
        w.set_voxel([8, 1, 8], water);
        run(&mut w, air, 10);
        assert_eq!(w.take_due_updates(usize::MAX), Vec::<[i64; 3]>::new());
        // On casse le mur : l'eau passe.
        w.set_voxel([9, 1, 8], air);
        run(&mut w, air, 10);
        assert_eq!(state(&w, [9, 1, 8]), (water, Some(1)));
        assert_eq!(state(&w, [10, 1, 8]), (water, Some(2)));
    }

    #[test]
    fn a_flow_saved_midway_ends_the_same_after_reload() {
        let pos = ChunkPos { x: 0, y: 0, z: 0 };
        let (mut whole, air, _, water) = world();
        let (mut half, _, _, _) = world();
        whole.set_voxel([8, 1, 8], water);
        half.set_voxel([8, 1, 8], water);
        run(&mut half, air, 2);
        // Sauvé en plein écoulement (niveaux et réexamens), puis relu.
        let mut back = VoxelWorld::new(half.registry.clone(), 16, 1.0);
        back.set_tick(half.tick());
        back.insert_saved_chunk(pos, half.chunk(pos).unwrap().clone(), half.chunk_state(pos));
        assert!(!half.chunk_state(pos).levels.is_empty() && !half.chunk_state(pos).updates.is_empty());
        run(&mut back, air, 20);
        run(&mut whole, air, 22);
        for (x, z) in (4..13).flat_map(|x| (4..13).map(move |z| (x, z))) {
            assert_eq!(state(&back, [x, 1, z]), state(&whole, [x, 1, z]), "({x}, {z})");
        }
    }

    #[test]
    fn the_budget_spreads_work_over_ticks_in_the_same_order() {
        let (mut w, air, _, water) = world();
        w.set_voxel([8, 1, 8], water);
        let mut slow = VoxelWorld::new(w.registry.clone(), 16, 1.0);
        slow.insert_chunk(ChunkPos { x: 0, y: 0, z: 0 }, w.chunk(ChunkPos { x: 0, y: 0, z: 0 }).unwrap().clone());
        slow.set_voxel([8, 1, 8], water);
        // Un réexamen par tick : plus lent, même résultat à la fin.
        for _ in 0..400 {
            slow.advance();
            for v in slow.take_due_updates(1) {
                if let Some((id, level)) = update(&mut slow, air, v) {
                    slow.set_voxel(v, id);
                    slow.set_liquid_level(v, level);
                }
            }
        }
        run(&mut w, air, 20);
        for x in 4..13 {
            for z in 4..13 {
                assert_eq!(state(&slow, [x, 1, z]), state(&w, [x, 1, z]), "({x}, {z})");
            }
        }
    }
}
