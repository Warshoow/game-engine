//! Recherche de chemin des mobs (#32, design doc §3.6) : A* sur les cases
//! du monde.
//!
//! **A\*** explore les cases de proche en proche, la plus prometteuse
//! d'abord : coût déjà payé + estimation du reste (jamais plus que le vrai
//! coût, sinon le chemin trouvé n'est plus le plus court). Borné en nombre
//! de cases explorées : une cible hors d'atteinte coûte ce plafond, pas le
//! monde entier ; on rend alors le chemin vers la case la plus proche de la
//! cible, comme Minecraft.
//!
//! **Une case est praticable avec le test de la physique** : le corps posé
//! en haut de la case, puis lâché d'une case (`move_and_collide`). Sans ça,
//! A* proposerait des passages que le mob ne peut pas suivre (dalle,
//! plafond bas, corps plus large qu'une case à 4 vox/m).
//!
//! Coûts en entiers : l'ordre d'exploration ne dépend pas d'arrondis, et
//! les égalités se départagent par ordre d'ajout (déterminisme, §2).

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use crate::physics::{move_and_collide, overlaps, Aabb, JUMP_HEIGHT_M};
use crate::world::VoxelWorld;

/// Chute qu'un chemin accepte, en mètres.
const MAX_DROP_M: f32 = 3.0;
/// Coûts d'un pas : droit, en diagonale, par case montée, par case descendue.
const STRAIGHT: u32 = 10;
const DIAGONAL: u32 = 14;
const UP: u32 = 10;
const DOWN: u32 = 4;

/// Une case du chemin, et la hauteur des pieds quand le corps s'y tient
/// (mètres : une dalle les met à mi-case).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Node {
    pub cell: [i64; 3],
    pub feet_y: f32,
}

/// Les pieds d'un corps `size` (largeur, hauteur) centré sur `cell`, s'il
/// y tient debout : pieds dans la case, sur un appui, hors d'un liquide.
pub fn stand(world: &VoxelWorld, cell: [i64; 3], size: [f32; 2]) -> Option<f32> {
    let vpm = world.voxels_per_meter();
    let center = |i: i64| (i as f32 + 0.5) / vpm;
    let (bottom, top) = (cell[1] as f32 / vpm, (cell[1] + 1) as f32 / vpm);
    if world.voxel(cell).and_then(|id| world.liquid(id)).is_some() {
        return None;
    }
    let start = Aabb::from_feet([center(cell[0]), top, center(cell[2])], size[0], size[1]);
    if overlaps(world, &start) {
        return None;
    }
    // Un peu plus d'une case : la région balayée doit toucher la case du
    // dessous pour y trouver l'appui.
    let fall = move_and_collide(world, start, [0.0, -1.01 / vpm, 0.0]);
    let feet = fall.aabb.min[1];
    (fall.collided[1] && feet >= bottom - 1e-3 && feet < top - 1e-3).then_some(feet)
}

/// Une recherche de chemin en cours. Elle avance par tranches
/// ([`Search::run`]) : une longue recherche s'étale sur plusieurs ticks
/// au lieu de coûter d'un coup (une case explorée coûte quelques µs).
#[derive(Debug, Clone)]
pub struct Search {
    to: [i64; 3],
    size: [f32; 2],
    max_nodes: u32,
    /// (case, coût payé, case d'avant) ; l'index sert d'identité.
    nodes: Vec<(Node, u32, usize)>,
    seen: HashMap<[i64; 3], usize>,
    open: BinaryHeap<Reverse<(u32, usize)>>,
    /// Une case est testée depuis chacune de ses voisines : on garde le
    /// résultat (le test coûte quelques lectures de boîtes).
    stands: HashMap<[i64; 3], Option<f32>>,
    /// (estimation du reste, index) de la case la plus proche de la cible.
    best: (u32, usize),
    explored: u32,
}

impl Search {
    /// Le chemin de `from` vers `to` (cases), pour un corps `size`
    /// (largeur, hauteur), en explorant au plus `max_nodes` cases.
    pub fn new(from: Node, to: [i64; 3], size: [f32; 2], max_nodes: u32) -> Self {
        let h = estimate(from.cell, to);
        Self {
            to,
            size,
            max_nodes,
            nodes: vec![(from, 0, usize::MAX)],
            seen: HashMap::from([(from.cell, 0)]),
            open: BinaryHeap::from([Reverse((h, 0))]),
            stands: HashMap::new(),
            best: (h, 0),
            explored: 0,
        }
    }

    /// Explore au plus `budget` cases de plus. Rend les cases explorées, et
    /// le chemin (sans la case de départ) quand la recherche est finie :
    /// cible atteinte, ou plafond, ou plus rien à explorer ; dans les deux
    /// derniers cas, le chemin vers la case explorée la plus proche.
    pub fn run(&mut self, world: &VoxelWorld, budget: u32) -> (u32, Option<Vec<Node>>) {
        let vpm = world.voxels_per_meter();
        let climb = (JUMP_HEIGHT_M * vpm).floor() as i64;
        let drop = (MAX_DROP_M * vpm).ceil() as i64;
        let size = self.size;
        // Le corps à la colonne de `c`, pieds à `feet_y` : passe-t-il ?
        let clear = |c: [i64; 3], feet_y: f32| {
            let center = |i: i64| (i as f32 + 0.5) / vpm;
            !overlaps(world, &Aabb::from_feet([center(c[0]), feet_y, center(c[2])], size[0], size[1]))
        };
        let mut used = 0;
        while let Some(&Reverse((_, i))) = self.open.peek() {
            let (node, paid, _) = self.nodes[i];
            if node.cell == self.to {
                self.best = (0, i);
                break;
            }
            if self.explored == self.max_nodes {
                break;
            }
            if used == budget {
                return (used, None);
            }
            self.open.pop();
            used += 1;
            self.explored += 1;
            let [x, y, z] = node.cell;
            for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
                // La plus haute case où le corps tient, de `climb` au-dessus
                // à `drop` en dessous.
                let Some((cell, feet_y)) = (-drop..=climb).rev().map(|dy| [x + dx, y + dy, z + dz]).find_map(|c| {
                    let feet = *self.stands.entry(c).or_insert_with(|| stand(world, c, size));
                    feet.map(|f| (c, f))
                }) else {
                    continue;
                };
                if feet_y - node.feet_y > JUMP_HEIGHT_M {
                    continue;
                }
                // Monter : la tête passe au-dessus de la case de départ ;
                // descendre : le corps passe au bord avant de tomber ; à la
                // même hauteur, il tient déjà dans les deux cases. En
                // diagonale, les deux cases d'angle laissent passer.
                let high = feet_y.max(node.feet_y);
                let level = (feet_y - node.feet_y).abs() < 1e-4;
                if !level && (!clear(node.cell, high) || !clear(cell, high)) {
                    continue;
                }
                if dx != 0 && dz != 0 && !(clear([x + dx, 0, z], high) && clear([x, 0, z + dz], high)) {
                    continue;
                }
                let dy = cell[1] - y;
                let step = if dx != 0 && dz != 0 { DIAGONAL } else { STRAIGHT }
                    + if dy > 0 { UP * dy as u32 } else { DOWN * dy.unsigned_abs() as u32 };
                let cost = paid + step;
                if self.seen.get(&cell).is_some_and(|&j| self.nodes[j].1 <= cost) {
                    continue;
                }
                let j = self.nodes.len();
                self.nodes.push((Node { cell, feet_y }, cost, i));
                self.seen.insert(cell, j);
                let h = estimate(cell, self.to);
                self.best = self.best.min((h, j));
                self.open.push(Reverse((cost + h, j)));
            }
        }
        let mut path = Vec::new();
        let mut i = self.best.1;
        while i != 0 {
            path.push(self.nodes[i].0);
            i = self.nodes[i].2;
        }
        path.reverse();
        (used, Some(path))
    }
}

/// [`Search`] d'une traite : le chemin et le nombre de cases explorées.
pub fn find(world: &VoxelWorld, from: Node, to: [i64; 3], size: [f32; 2], max_nodes: u32) -> (Vec<Node>, u32) {
    let (used, path) = Search::new(from, to, size, max_nodes).run(world, max_nodes);
    (path.expect("le budget couvre le plafond"), used)
}

/// Estimation du coût de `c` à `to`, jamais plus que le vrai : le chemin
/// en diagonale puis droit, et la descente au moins.
fn estimate(c: [i64; 3], to: [i64; 3]) -> u32 {
    let (dx, dz) = ((c[0] - to[0]).unsigned_abs(), (c[2] - to[2]).unsigned_abs());
    let (lo, hi) = (dx.min(dz) as u32, dx.max(dz) as u32);
    DIAGONAL * lo + STRAIGHT * (hi - lo) + DOWN * (c[1] - to[1]).unsigned_abs() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{Chunk, ChunkPos};
    use crate::registry::{ContentId, Registry};

    const PIG: [f32; 2] = [0.9, 0.9];

    /// Monde 32³ à 1 vox/m, sol plein jusqu'à y = 4 (pieds à 4 m), slab
    /// en (3, 4, 3), et ce que `build` pose en plus.
    fn world(build: impl Fn(&mut VoxelWorld, ContentId, ContentId)) -> VoxelWorld {
        let reg = Registry::from_ron(
            r#"[
                (identifier: "t:air", kind: Block((solid: false, color: (0.0, 0.0, 0.0)))),
                (identifier: "t:stone", kind: Block((solid: true, color: (0.5, 0.5, 0.5)))),
                (identifier: "t:slab", kind: Block((solid: true, color: (0.5, 0.5, 0.5),
                    shape: Some([((0, 0, 0), (16, 8, 16))])))),
            ]"#,
        )
        .unwrap();
        let ids = ["t:air", "t:stone", "t:slab"].map(|n| reg.lookup(n).unwrap());
        let mut w = VoxelWorld::new(reg, 32, 1.0);
        let mut chunk = Chunk::filled(32, ids[0]);
        for (x, z) in (0..32).flat_map(|x| (0..32).map(move |z| (x, z))) {
            for y in 0..4 {
                chunk.set(x, y, z, ids[1]);
            }
        }
        w.insert_chunk(ChunkPos { x: 0, y: 0, z: 0 }, chunk);
        build(&mut w, ids[1], ids[2]);
        w
    }

    fn start(w: &VoxelWorld, cell: [i64; 3]) -> Node {
        Node { cell, feet_y: stand(w, cell, PIG).unwrap() }
    }

    #[test]
    fn a_body_stands_on_the_floor_and_on_a_slab_not_in_a_wall_or_in_the_air() {
        let w = world(|w, stone, slab| {
            w.set_voxel([3, 4, 3], slab);
            w.set_voxel([5, 4, 5], stone);
        });
        let feet = |c| stand(&w, c, PIG).map(|f| (f * 100.0).round() / 100.0);
        assert_eq!(feet([1, 4, 1]), Some(4.0));
        assert_eq!(feet([3, 4, 3]), Some(4.5), "sur la dalle");
        assert_eq!(feet([5, 4, 5]), None, "dans le bloc");
        assert_eq!(feet([5, 5, 5]), Some(5.0), "dessus");
        assert_eq!(feet([1, 6, 1]), None, "en l'air");
    }

    #[test]
    fn the_path_goes_around_a_wall_and_climbs_one_block_not_two() {
        // Mur de 2 m en x = 10, de z = 0 à 20 ; une marche d'un bloc à partir de x = 20.
        let w = world(|w, stone, _| {
            for z in 0..20 {
                w.set_voxel([10, 4, z], stone);
                w.set_voxel([10, 5, z], stone);
            }
            for (x, z) in (20..32).flat_map(|x| (0..32).map(move |z| (x, z))) {
                w.set_voxel([x, 4, z], stone);
            }
        });
        let (path, _) = find(&w, start(&w, [5, 4, 5]), [15, 4, 5], PIG, 4000);
        assert_eq!(path.last().unwrap().cell, [15, 4, 5]);
        assert!(path.iter().any(|n| n.cell[2] >= 20), "contourne le mur par z ≥ 20");
        assert!(path.iter().all(|n| n.cell[0] != 10 || n.cell[2] >= 20));
        let (up, _) = find(&w, start(&w, [18, 4, 25]), [22, 5, 25], PIG, 4000);
        assert_eq!(up.last().unwrap().cell, [22, 5, 25], "monte la marche d'un bloc");
        assert!(up.windows(2).all(|p| p[1].feet_y - p[0].feet_y <= JUMP_HEIGHT_M));
    }

    #[test]
    fn an_unreachable_target_costs_the_bound_and_gives_the_closest_cell() {
        // La cible est dans une fosse fermée de murs de 3 m.
        let w = world(|w, stone, _| {
            for i in 10..15 {
                for y in 4..7 {
                    for [x, z] in [[i, 10], [i, 14], [10, i], [14, i]] {
                        w.set_voxel([x, y, z], stone);
                    }
                }
            }
        });
        let (path, explored) = find(&w, start(&w, [2, 4, 2]), [12, 4, 12], PIG, 300);
        assert_eq!(explored, 300);
        let end = path.last().unwrap().cell;
        assert!((end[0] - 12).abs() <= 3 && (end[2] - 12).abs() <= 3, "{end:?}");
        // Même entrée, même chemin, d'une traite ou par tranches.
        assert_eq!(path, find(&w, start(&w, [2, 4, 2]), [12, 4, 12], PIG, 300).0);
        let mut search = Search::new(start(&w, [2, 4, 2]), [12, 4, 12], PIG, 300);
        let mut slices = 0;
        let sliced = loop {
            slices += 1;
            if let (_, Some(p)) = search.run(&w, 64) {
                break p;
            }
        };
        assert_eq!((sliced, slices), (path, 5));
    }
}
