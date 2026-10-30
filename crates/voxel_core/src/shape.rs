//! Formes de blocs (#14, design doc §3.1 et §3.2) : un bloc qui n'est pas
//! un cube plein déclare des boîtes, en seizièmes de case ; tourné à la
//! pose, il garde une des 24 rotations du cube dans une table creuse du
//! monde.
//!
//! **Pourquoi des seizièmes entiers** : savoir si une boîte couvre toute la
//! face d'une case est une comparaison exacte (`== 16`), sans marge de
//! flottant ; et tourner une boîte d'un quart de tour reste entier.
//!
//! **Pourquoi les 24 rotations** : un dessus parmi six directions, puis un
//! quart de tour autour de lui. Tourner plus tard une construction entière
//! envoie toute rotation sur une autre, sans cas propre à chaque bloc.

use serde::{Deserialize, Serialize};

/// Arête d'une case, en unités de boîte.
pub const CELL: u8 = 16;

/// Boîtes au plus par forme : borne ce qu'une entrée générée peut coûter.
pub const MAX_BOXES: usize = 16;

/// Une boîte dans la case : coin bas, coin haut, en seizièmes (0 à 16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct Cuboid(pub [u8; 3], pub [u8; 3]);

impl Cuboid {
    /// La case entière.
    pub const FULL: Cuboid = Cuboid([0; 3], [CELL; 3]);

    /// Vide, retournée ou hors de la case.
    pub fn invalid(&self) -> bool {
        (0..3).any(|a| self.0[a] >= self.1[a] || self.1[a] > CELL)
    }

    /// La boîte tournée de `r` autour du centre de la case.
    pub fn rotated(self, r: Rotation) -> Cuboid {
        let turn = |p: [u8; 3]| {
            let centered = p.map(|c| 2 * i32::from(c) - i32::from(CELL));
            r.apply(centered).map(|c| ((c + i32::from(CELL)) / 2) as u8)
        };
        let (a, b) = (turn(self.0), turn(self.1));
        Cuboid(std::array::from_fn(|k| a[k].min(b[k])), std::array::from_fn(|k| a[k].max(b[k])))
    }

    /// Couvre toute la face de la case du côté `dir` (±1 sur un axe).
    pub fn covers(&self, dir: [i32; 3]) -> bool {
        let axis = dir.iter().position(|&c| c != 0).expect("une direction");
        let on_side = if dir[axis] > 0 { self.1[axis] == CELL } else { self.0[axis] == 0 };
        on_side && (0..3).filter(|&a| a != axis).all(|a| self.0[a] == 0 && self.1[a] == CELL)
    }

    /// Coins en cases, en flottants (`/ 16`).
    pub fn to_cells(self) -> [[f32; 3]; 2] {
        [self.0, self.1].map(|p| p.map(|c| f32::from(c) / f32::from(CELL)))
    }
}

/// Comment un bloc se tourne à la pose (§3.1). Append-only (§3.6) : une
/// save le cite par son nom.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum Orient {
    /// Toujours comme dessiné.
    #[default]
    Fixed,
    /// Quatre sens selon le regard du joueur ; à l'envers posé contre le
    /// haut d'une face ou sous un bloc (escalier).
    Facing,
    /// Seulement à l'envers, aux mêmes conditions (dalle).
    Half,
    /// Le dessus du modèle pointe à l'opposé du bloc cliqué (bûche, torche).
    Face,
}

/// Une des 24 rotations du cube : `index = dessus × 4 + quart de tour`. Le
/// quart de tour (autour de +Y) se fait d'abord, puis +Y va sur le dessus.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Rotation(u8);

impl Rotation {
    /// Le modèle tel que dessiné ; absent de la table du monde.
    pub const IDENTITY: Rotation = Rotation(0);

    /// `None` hors des 24 (fichier abîmé).
    pub fn from_index(i: u8) -> Option<Rotation> {
        (i < 24).then_some(Rotation(i))
    }

    pub fn index(self) -> u8 {
        self.0
    }

    fn all() -> impl Iterator<Item = Rotation> {
        (0..24).map(Rotation)
    }

    /// Tourne le vecteur `v`.
    pub fn apply(self, [x, y, z]: [i32; 3]) -> [i32; 3] {
        // Quart de tour autour de +Y : +Z va vers +Z, +X, -Z, -X.
        let [x, y, z] = match self.0 % 4 {
            0 => [x, y, z],
            1 => [z, y, -x],
            2 => [-x, y, -z],
            _ => [-z, y, x],
        };
        // Puis +Y vers le dessus, dans l'ordre +Y, -Y, +X, -X, +Z, -Z
        // (une rotation, jamais un miroir).
        match self.0 / 4 {
            0 => [x, y, z],
            1 => [x, -y, -z],
            2 => [y, -x, z],
            3 => [-y, x, z],
            4 => [x, -z, y],
            _ => [x, z, -y],
        }
    }

    /// Le vecteur du modèle qui devient `world` une fois tourné (la rotation
    /// inverse : sa matrice est la transposée).
    pub fn model_dir(self, world: [i32; 3]) -> [i32; 3] {
        [[1, 0, 0], [0, 1, 0], [0, 0, 1]].map(|e| {
            let a = self.apply(e);
            a[0] * world[0] + a[1] * world[1] + a[2] * world[2]
        })
    }

    /// La rotation qui envoie +Y sur `up` et +Z sur `front`.
    fn with(up: [i32; 3], front: [i32; 3]) -> Option<Rotation> {
        Self::all().find(|r| r.apply([0, 1, 0]) == up && r.apply([0, 0, 1]) == front)
    }

    /// La rotation d'un bloc posé sur la face `face` (normale vers la case
    /// posée) d'un bloc, le joueur regardant vers `look`. `high` : le
    /// regard touche la moitié haute de la face.
    pub fn placed(orient: Orient, look: [f32; 3], face: [i32; 3], high: bool) -> Rotation {
        let upside_down = face == [0, -1, 0] || (face[1] == 0 && high);
        let up = if upside_down { [0, -1, 0] } else { [0, 1, 0] };
        let front = if look[0].abs() > look[2].abs() {
            [look[0].signum() as i32, 0, 0]
        } else {
            [0, 0, if look[2] < 0.0 { -1 } else { 1 }]
        };
        let found = match orient {
            Orient::Fixed => None,
            Orient::Facing => Self::with(up, front),
            Orient::Half => Self::with(up, [0, 0, 1]),
            Orient::Face => Self::all().find(|r| r.apply([0, 1, 0]) == face),
        };
        found.unwrap_or(Self::IDENTITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_24_rotations_are_distinct_rotations() {
        let mut seen = Vec::new();
        for r in Rotation::all() {
            let m = [[1, 0, 0], [0, 1, 0], [0, 0, 1]].map(|v| r.apply(v));
            // Déterminant +1 : une rotation, pas un miroir.
            let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
                + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
            assert_eq!(det, 1, "{r:?}");
            assert!(!seen.contains(&m), "{r:?} en double");
            seen.push(m);
        }
        assert_eq!(Rotation::IDENTITY.apply([1, 2, 3]), [1, 2, 3]);
    }

    #[test]
    fn a_rotated_box_stays_in_the_cell() {
        let slab = Cuboid([0, 0, 0], [16, 8, 16]);
        for r in Rotation::all() {
            let b = slab.rotated(r);
            assert!(!b.invalid(), "{r:?} : {b:?}");
            // Une dalle couvre toujours une face entière : celle de son dessous.
            assert!(b.covers(r.apply([0, -1, 0])));
        }
        let upside_down = Rotation::placed(Orient::Half, [0.0, 0.0, 1.0], [0, -1, 0], false);
        assert_eq!(slab.rotated(upside_down), Cuboid([0, 8, 0], [16, 16, 16]));
    }

    #[test]
    fn covers_needs_the_whole_face() {
        let slab = Cuboid([0, 0, 0], [16, 8, 16]);
        assert!(slab.covers([0, -1, 0]));
        assert!(!slab.covers([0, 1, 0]));
        assert!(!slab.covers([1, 0, 0]));
        assert!(Cuboid::FULL.covers([0, 0, -1]));
    }

    #[test]
    fn placement_follows_look_click_and_half() {
        // Escalier : l'avant du modèle (+Z) suit le regard.
        let r = Rotation::placed(Orient::Facing, [0.9, -0.3, 0.1], [0, 1, 0], false);
        assert_eq!((r.apply([0, 0, 1]), r.apply([0, 1, 0])), ([1, 0, 0], [0, 1, 0]));
        // Contre le haut d'une face de côté : à l'envers, même regard.
        let r = Rotation::placed(Orient::Facing, [0.0, 0.0, -1.0], [1, 0, 0], true);
        assert_eq!((r.apply([0, 0, 1]), r.apply([0, 1, 0])), ([0, 0, -1], [0, -1, 0]));
        // Bûche, torche : le dessus à l'opposé du bloc cliqué.
        let r = Rotation::placed(Orient::Face, [0.0, 0.0, 1.0], [0, 0, -1], false);
        assert_eq!(r.apply([0, 1, 0]), [0, 0, -1]);
        assert_eq!(r.model_dir([0, 0, -1]), [0, 1, 0]);
        assert!(Rotation::all().all(|r| r.model_dir(r.apply([1, -2, 3])) == [1, -2, 3]));
        assert_eq!(Rotation::placed(Orient::Fixed, [1.0, 0.0, 0.0], [0, -1, 0], true), Rotation::IDENTITY);
    }
}
