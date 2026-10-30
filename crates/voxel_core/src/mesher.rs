//! Mesher blocky — étape 2 : greedy meshing (fusion des faces coplanaires).
//!
//! **Quelle face est cachée** (#22) : celle collée à un bloc opaque, ou à un
//! bloc identique quand il est à trous (`alpha: Mask`). Derrière une feuille
//! on voit à travers les trous, donc la terre dessous garde sa face ; entre
//! deux feuilles, la face commune n'apporterait que du travail au GPU.
//!
//! **Liquides et semi-transparent** (#28) : un chunk donne deux meshes. Le
//! premier porte l'opaque et les blocs à trous ; le second les blocs
//! `alpha: Blend` (l'eau), que le moteur de rendu dessine après le reste. Un
//! liquide est meshé sans être solide ; quand rien du même liquide n'est
//! au-dessus de lui, sa surface est abaissée selon son niveau
//! ([`crate::liquids::height`]). Entre deux cases du même liquide, seule
//! la différence de hauteur a une face (une pente d'eau). Deux faces de
//! hauteurs différentes ne fusionnent pas : la hauteur fait partie de la
//! clé de fusion.
//!
//! **Ombrage des coins** (#7) : chaque sommet d'une face est assombri selon
//! les blocs qui l'entourent devant la face ([`corner_ao`]) ; un coin de
//! grotte ou le pied d'un mur se lit sans vrai éclairage. L'ombre multiplie
//! la couleur du sommet : aucun attribut en plus. Deux faces ne fusionnent
//! que si leurs quatre coins ont la même ombre : comme l'ombre d'un coin ne
//! dépend que de sa position, des faces voisines de même ombre ont une
//! ombre constante le long de la fusion, et le rectangle fusionné la rend
//! exactement.
//!
//! **Lumière** (#23) : chaque sommet porte la lumière du ciel et celle des
//! blocs ([`crate::light`]), moyennées sur les cases autour du coin qui ne
//! font pas d'ombre — les mêmes cases que l'ombrage. Comme l'ombre, elle
//! entre dans la clé de fusion, et pour la même raison la fusion reste
//! exacte.
//!
//! **Étape 1 (culling naïf)** : un chunk plein de pierre n'a pas besoin de
//! 32³ cubes — seules les faces **au contact de l'air** existent
//! visuellement. On émet une face uniquement si le voisin dans cette
//! direction est non-solide. Conservé ici sous [`mesh_chunk_naive`] : il
//! sert d'oracle dans les tests (même surface, découpage différent).
//!
//! **Étape 2 (greedy, algo de Lysenko)** : le culling émet un quad 1×1 par
//! face visible — un sol plat 32×32 coûte 1024 quads pour ce qui est
//! géométriquement *un* rectangle. Le greedy balaie le chunk en tranches
//! perpendiculaires à chaque direction de face, projette les faces visibles
//! de la tranche dans un masque 2D, puis fusionne les cases adjacentes de
//! **même matériau** en rectangles maximaux (largeur d'abord, puis hauteur
//! tant que la ligne entière matche). Un rectangle = un quad, quelle que
//! soit sa taille. « Gourmand » : localement optimal, pas globalement — le
//! premier rectangle trouvé est pris, sans chercher le pavage minimal
//! (NP-difficile) ; en pratique le gain est déjà massif.
//!
//! La clé de fusion est l'**index de palette locale** du voxel : deux faces
//! ne fusionnent que si elles portent le même contenu. C'est ce qui garde le
//! résultat data-driven correct — pierre et terre adjacentes restent deux
//! quads, chacun avec sa couleur.
//!
//! Le mesher est pur et sans Bevy : il produit des tampons bruts
//! ([`MeshData`]) que l'app convertit en `Mesh` Bevy. Ça le rend testable
//! headless — on compte des faces, pas des pixels.
//!
//! Sorties en **mètres** (§2) : les positions sont `coordonnée voxel ×
//! voxel_size_m`, dans le repère local du chunk (l'origine du chunk est
//! placée par l'ECS, pas par le mesher).

use std::collections::HashMap;

use crate::chunk::{Chunk, ChunkPos};
use crate::liquids;
use crate::registry::{Alpha, ContentId, Registry};
use crate::shape::{Cuboid, Rotation, CELL};
use crate::world::VoxelWorld;

/// Hauteur d'une face pleine dans la clé de fusion (sur 255).
const FULL: u8 = u8::MAX;

/// Lumière d'un sommet selon son ombre, de 0 (coin fermé) à 3 (rien
/// autour).
const AO_LIGHT: [f32; 4] = [0.45, 0.65, 0.82, 1.0];

/// Ombres d'une face sans ombrage : les quatre coins à 3.
const NO_SHADE: u8 = u8::MAX;

/// Lumière des quatre coins d'une face, au rang `cu + 2 cv` comme les
/// ombres : `[ciel, blocs]` en niveaux × 12 / nombre de cases moyennées
/// (12 se divise par 1 à 4 cases : la moyenne reste entière, donc
/// comparable dans la clé de fusion).
type Lights = [[u16; 2]; 4];

/// Couche d'un bloc sans texture.
pub const NO_TEXTURE: u32 = u32::MAX;

/// Tampons de mesh bruts, agnostiques du moteur de rendu.
#[derive(Debug, Default, Clone)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub colors: Vec<[f32; 4]>,
    /// Coordonnées de texture **en mètres** : une texture couvre 1 m, donc
    /// une face fusionnée de 5 × 3 m va de 0 à 5 et de 0 à 3 — le sampler en
    /// mode répétition répète l'image au lieu de l'étirer.
    pub uvs: Vec<[f32; 2]>,
    /// Couche du texture array par sommet ; [`NO_TEXTURE`] : couleur seule.
    pub layers: Vec<u32>,
    /// `[ciel, blocs]` par sommet, dans [0, 1] (1 : le ciel ouvert).
    pub lights: Vec<[f32; 2]>,
    /// Triangles, en sens antihoraire (CCW) vu de l'extérieur — la
    /// convention de face avant de Bevy/wgpu.
    pub indices: Vec<u32>,
}

impl MeshData {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn face_count(&self) -> usize {
        // Une face = un quad = 2 triangles = 6 indices.
        self.indices.len() / 6
    }
}

/// Meshe un chunk par greedy meshing — le chemin de production.
///
/// La solidité et la couleur sont résolues **une fois par entrée de palette**
/// (pas une fois par voxel) via le registre — c'est le chemin data-driven :
/// le mesher ne connaît aucun bloc par son nom.
///
/// Hors du chunk, le voisin est traité comme de l'air : chaque face de
/// bordure est émise. C'est la variante « chunk isolé » (tests, outillage) —
/// en jeu, préférer [`mesh_chunk_in_world`] qui raccorde les chunks entre eux.
pub fn mesh_chunk(chunk: &Chunk, registry: &Registry, voxel_size_m: f32) -> MeshData {
    Region::isolated(chunk, registry).mesh_pass(voxel_size_m, false)
}

/// Meshe le chunk `pos` en interrogeant le **monde** pour les voxels hors
/// chunk : les faces cachées par un voisin d'un autre chunk sont
/// culled — c'est le raccord inter-chunks. Un chunk voisin **non chargé**
/// compte comme de l'air : ses faces de bordure sont émises, et il faudra
/// re-mesher ce chunk quand le voisin apparaîtra (streaming).
///
/// Rend `[le reste, le semi-transparent]` ; `None` si le chunk n'est pas
/// chargé.
pub fn mesh_chunk_in_world(world: &VoxelWorld, pos: ChunkPos, voxel_size_m: f32) -> Option<[MeshData; 2]> {
    Some(Region::copy(world, pos)?.mesh(voxel_size_m))
}

/// Case sans données (chunk voisin non chargé) : comptée comme de l'air.
const NOTHING: u16 = u16::MAX;

/// Ce que le mesher lit d'un chunk : ses voxels et une case de bordure
/// autour, avec la lumière et la hauteur des liquides. Copié du monde, il se
/// meshe sans lui, donc dans une autre tâche pendant que le monde change
/// (#53). ~200 Ko pour un chunk de 32³.
pub struct Region {
    /// Arête du chunk ; la copie en fait `size + 2`, de -1 à `size`.
    size: i32,
    /// Index dans `blocks` de chaque case, ou [`NOTHING`]. Au centre, c'est
    /// l'index de palette du chunk.
    cells: Vec<u16>,
    blocks: Vec<Resolved>,
    /// `[ciel, blocs]` de chaque case.
    lights: Vec<[u8; 2]>,
    /// Hauteur de la surface d'un liquide sous de l'air (sur 255), `FULL`
    /// pour le reste.
    heights: Vec<u8>,
    /// Rotation des blocs à forme tournés (#14), par index de case.
    rotations: HashMap<usize, Rotation>,
    /// Niveau du ciel ouvert.
    full: u8,
}

impl Region {
    /// Le chunk seul : rien autour, tout au ciel ouvert.
    fn isolated(chunk: &Chunk, registry: &Registry) -> Self {
        let size = chunk.size() as i32;
        let side = (size + 2) as usize;
        let mut region = Region {
            size,
            cells: vec![NOTHING; side.pow(3)],
            blocks: resolve_palette(chunk, registry),
            lights: vec![[1, 0]; side.pow(3)],
            heights: vec![FULL; side.pow(3)],
            rotations: HashMap::new(),
            full: 1,
        };
        for_each_local(size, |p| {
            let i = region.index(p);
            region.cells[i] = chunk.get_local(p[0] as u32, p[1] as u32, p[2] as u32);
        });
        region
    }

    /// Le chunk `pos` et sa bordure, lus dans le monde ; `None` si le chunk
    /// n'est pas chargé.
    pub fn copy(world: &VoxelWorld, pos: ChunkPos) -> Option<Self> {
        let chunk = world.chunk(pos)?;
        let mut region = Self::isolated(chunk, &world.registry);
        region.full = world.full_light();
        let size = region.size;
        let base = [pos.x, pos.y, pos.z].map(|c| i64::from(c) * i64::from(size));
        let at = |p: [i32; 3]| [0, 1, 2].map(|a| base[a] + i64::from(p[a]));
        if let Some([sky, block]) = world.chunk_light(pos) {
            for_each_local(size, |p| {
                let (i, k) = ((p[0] + size * (p[1] + size * p[2])) as usize, region.index(p));
                region.lights[k] = [sky.get(i), block.get(i)];
            });
        }
        // La bordure : les cases dont une coordonnée vaut -1 ou `size`.
        for z in -1..=size {
            for y in -1..=size {
                for x in -1..=size {
                    let p = [x, y, z];
                    if p.iter().all(|&c| (0..size).contains(&c)) {
                        continue;
                    }
                    let i = region.index(p);
                    region.lights[i] = world.light_at(at(p));
                    if let Some(id) = world.voxel(at(p)) {
                        region.cells[i] = match region.blocks.iter().position(|b| b.id == id) {
                            Some(k) => k as u16,
                            None => {
                                region.blocks.push(resolve(&world.registry, id));
                                (region.blocks.len() - 1) as u16
                            }
                        };
                    }
                }
            }
        }
        let vpm = world.voxels_per_meter();
        for z in -1..=size {
            for y in -1..=size {
                for x in -1..=size {
                    let p = [x, y, z];
                    let Some(block) = region.block(p) else { continue };
                    let (shaped, liquid) = (!block.shape.is_empty(), block.liquid.then(|| world.liquid(block.id)).flatten());
                    let r = if shaped { world.rotation(at(p)) } else { Rotation::IDENTITY };
                    if r != Rotation::IDENTITY {
                        let i = region.index(p);
                        region.rotations.insert(i, r);
                    }
                    if let Some(l) = liquid {
                        let h = liquids::height(world.liquid_level(at(p)), liquids::reach(&l, vpm));
                        let i = region.index(p);
                        region.heights[i] = (h * f32::from(FULL)).round() as u8;
                    }
                }
            }
        }
        Some(region)
    }

    /// `[le reste, le semi-transparent]`.
    pub fn mesh(&self, voxel_size_m: f32) -> [MeshData; 2] {
        let blend = if self.blocks.iter().any(|b| b.blend) { self.mesh_pass(voxel_size_m, true) } else { MeshData::default() };
        [self.mesh_pass(voxel_size_m, false), blend]
    }

    fn index(&self, p: [i32; 3]) -> usize {
        let side = self.size + 2;
        ((p[0] + 1) + side * ((p[1] + 1) + side * (p[2] + 1))) as usize
    }

    fn block(&self, p: [i32; 3]) -> Option<&Resolved> {
        let k = self.cells[self.index(p)];
        (k != NOTHING).then(|| &self.blocks[k as usize])
    }

    fn rotation(&self, p: [i32; 3]) -> Rotation {
        self.rotations.get(&self.index(p)).copied().unwrap_or_default()
    }

    /// `neighbor`, en `q`, cache-t-il la face de `block` tournée vers lui ?
    /// `toward` : de `q` vers `block`. Un bloc à forme ne cache que si une
    /// de ses boîtes, tournée, couvre toute la face commune.
    fn hides_at(&self, neighbor: &Resolved, q: [i32; 3], block: &Resolved, toward: [i32; 3]) -> bool {
        if neighbor.shape.is_empty() {
            return hides(neighbor, block);
        }
        let r = self.rotation(q);
        neighbor.opaque && neighbor.shape.iter().any(|c| c.rotated(r).covers(toward))
    }

    /// Les faces des blocs à forme (#14) de la passe `blend` : chaque boîte,
    /// tournée, sans fusion. Une face au bord de la case disparaît derrière
    /// un voisin qui la cache. Lumière de la case elle-même, sans ombrage.
    // shortcut: pas de greedy, 2 quads par dalle de sol ; fusionner si un
    // sol de dalles se voit au profilage (#58).
    fn mesh_shapes(&self, mesh: &mut MeshData, voxel_size_m: f32, blend: bool) {
        let full = f32::from(self.full);
        for_each_local(self.size, |p| {
            let Some(block) = self.block(p).filter(|b| !b.shape.is_empty() && b.blend == blend) else { return };
            let r = self.rotation(p);
            let [sky, lit] = self.lights[self.index(p)];
            let lights = [f32::from(sky) / full, if block.emits { 1.0 } else { f32::from(lit) / full }];
            for c in &block.shape {
                let Cuboid(lo, hi) = c.rotated(r);
                for face in &FACES {
                    let dir = face.neighbor;
                    let d = dir.iter().position(|&c| c != 0).unwrap();
                    let on_side = if dir[d] > 0 { hi[d] == CELL } else { lo[d] == 0 };
                    let q = [0, 1, 2].map(|a| p[a] + dir[a]);
                    if on_side && self.block(q).is_some_and(|o| self.hides_at(o, q, block, dir.map(|c| -c))) {
                        continue;
                    }
                    // Dessus, dessous ou côté : choisi sur le modèle avant rotation.
                    let layer = match r.model_dir(dir) {
                        [0, 1, 0] => block.layers[1],
                        [0, -1, 0] => block.layers[2],
                        _ => block.layers[0],
                    };
                    let base = mesh.positions.len() as u32;
                    for corner in face.corners {
                        let pos: [f32; 3] = std::array::from_fn(|a| {
                            let sixteenths = if corner[a] == 1 { hi[a] } else { lo[a] };
                            (p[a] as f32 + f32::from(sixteenths) / f32::from(CELL)) * voxel_size_m
                        });
                        push_vertex(mesh, pos, face.normal, block, layer, 1.0, lights);
                    }
                    push_quad_indices(mesh, base, [3; 4]);
                }
            }
        });
    }

    /// Cœur du greedy. `blend` : la passe semi-transparente, sinon le reste.
    fn mesh_pass(&self, voxel_size_m: f32, blend: bool) -> MeshData {
        let size = self.size;
        let n = size as usize;
        let resolved = &self.blocks;
        let full = self.full;
        let id_at = |q: [i32; 3]| self.block(q).map(|b| b.id);
        let light = |q: [i32; 3]| self.lights[self.index(q)];

        // Hauteur du liquide `id` en `p` (sur 255) : sous autre chose que
        // lui-même, sa surface est abaissée.
        let height_at = |p: [i32; 3], id: ContentId| {
            if id_at([p[0], p[1] + 1, p[2]]) == Some(id) { FULL } else { self.heights[self.index(p)] }
        };

        // Fait de l'ombre aux coins des faces voisines.
        let occludes = |q: [i32; 3]| self.block(q).is_some_and(Resolved::casts_shadow);

        // (index de palette, haut, bas, ombres, lumière) de `p` si sa face vers
        // `q` se voit ; haut et bas sur 255, `FULL` et 0 pour une face entière ;
        // ombres et lumière des quatre coins ([`face_ao`], [`face_light`]). `p`
        // est toujours dans le chunk, `q` peut en sortir.
        let visible_face = |p: [i32; 3], q: [i32; 3], axes: [usize; 3]| -> Option<Key> {
            let idx = self.cells[self.index(p)];
            let block = &resolved[idx as usize];
            if !block.drawn || block.blend != blend || !block.shape.is_empty() {
                return None;
            }
            let height = if block.liquid { height_at(p, block.id) } else { FULL };
            let lights = |q| {
                let mut lights = face_light(occludes, light, q, axes);
                if block.emits {
                    lights.iter_mut().for_each(|l| l[1] = 12 * u16::from(full));
                }
                lights
            };
            let neighbor = self.block(q);
            // Côté d'un liquide contre le même liquide plus bas : seule la
            // différence de hauteur se voit (une pente d'eau, une chute dans un
            // lac).
            if block.liquid && q[1] == p[1] && neighbor.is_some_and(|o| o.id == block.id) {
                let theirs = height_at(q, block.id);
                return (theirs < height).then(|| (idx, height, theirs, NO_SHADE, lights(q)));
            }
            let toward = [0, 1, 2].map(|a| p[a] - q[a]);
            let hidden = neighbor.is_some_and(|other| self.hides_at(other, q, block, toward));
            // Surface abaissée : un bloc posé dessus ne la touche pas.
            let lowered_top = height < FULL && q[1] > p[1];
            if hidden && !lowered_top {
                return None;
            }
            // Les liquides et le semi-transparent ne reçoivent pas d'ombre.
            let ao = if block.liquid || blend { NO_SHADE } else { face_ao(occludes, q, axes) };
            Some((idx, height, 0, ao, lights(q)))
        };

        let mut mesh = MeshData::default();
        // Masque 2D réutilisé pour chaque tranche : None = pas de face visible,
        // Some(clé) = face visible, la clé de fusion étant (index de palette,
        // haut, bas, ombres, lumière).
        let mut mask: Vec<Option<Key>> = vec![None; n * n];

        // `d` est l'axe de la normale ; `u`/`v` les deux axes du plan de la
        // tranche. (d, u, v) reste une permutation cyclique de (x, y, z) pour
        // que û × v̂ = d̂ — c'est ce qui rend l'enroulement CCW prévisible.
        for d in 0..3 {
            let u = (d + 1) % 3;
            let v = (d + 2) % 3;
            for positive in [true, false] {
                let step: i32 = if positive { 1 } else { -1 };
                let mut normal = [0.0f32; 3];
                normal[d] = step as f32;

                for layer in 0..size {
                    // 1. Projeter les faces visibles de la tranche dans le masque.
                    let mut any = false;
                    for iv in 0..size {
                        for iu in 0..size {
                            let mut p = [0i32; 3];
                            p[d] = layer;
                            p[u] = iu;
                            p[v] = iv;
                            let mut q = p;
                            q[d] += step;
                            let cell = visible_face(p, q, [d, u, v]);
                            any |= cell.is_some();
                            mask[(iu + iv * size) as usize] = cell;
                        }
                    }
                    if !any {
                        continue;
                    }

                    // 2. Fusion gourmande : rectangles maximaux de même clé.
                    for iv in 0..n {
                        for iu in 0..n {
                            let Some(key) = mask[iu + iv * n] else { continue };
                            // Largeur : étendre le long de u tant que la clé matche.
                            let mut w = 1;
                            while iu + w < n && mask[iu + w + iv * n] == Some(key) {
                                w += 1;
                            }
                            // Hauteur : étendre le long de v tant que la *ligne
                            // entière* [iu, iu+w) matche — condition nécessaire
                            // pour que le résultat reste un rectangle.
                            let mut h = 1;
                            'grow: while iv + h < n {
                                for k in 0..w {
                                    if mask[iu + k + (iv + h) * n] != Some(key) {
                                        break 'grow;
                                    }
                                }
                                h += 1;
                            }
                            // Consommer le rectangle pour ne pas le réémettre.
                            for dv in 0..h {
                                for du in 0..w {
                                    mask[iu + du + (iv + dv) * n] = None;
                                }
                            }
                            let quad = SliceQuad {
                                axes: [d, u, v],
                                positive,
                                layer,
                                origin: [iu as i32, iv as i32],
                                extent: [w as i32, h as i32],
                            };
                            emit_rect(&mut mesh, &quad, normal, &resolved[key.0 as usize], [key.1, key.2], (key.3, key.4, full), voxel_size_m);
                        }
                    }
                }
            }
        }
        self.mesh_shapes(&mut mesh, voxel_size_m, blend);
        mesh
    }
}

/// Appelle `f` sur chaque case `[x, y, z]` d'un chunk d'arête `size`.
fn for_each_local(size: i32, mut f: impl FnMut([i32; 3])) {
    for z in 0..size {
        for y in 0..size {
            for x in 0..size {
                f([x, y, z]);
            }
        }
    }
}

/// Clé de fusion du greedy : (index de palette, haut, bas, ombres, lumière).
type Key = (u16, u8, u8, u8, Lights);

/// Un rectangle fusionné dans le plan d'une tranche, avant projection en 3D.
struct SliceQuad {
    /// (d, u, v) : axe de la normale puis les deux axes du plan.
    axes: [usize; 3],
    /// Sens de la normale le long de `d`.
    positive: bool,
    /// Index de la tranche le long de `d` (coordonnée voxel).
    layer: i32,
    /// Coin bas du rectangle, en (u, v).
    origin: [i32; 2],
    /// Largeur/hauteur du rectangle, en (u, v).
    extent: [i32; 2],
}

/// `[haut, bas]` : la part de voxel couverte (sur 255) ; `ao`, `lights` : les
/// ombres et la lumière des coins, `full` le niveau du ciel ouvert ; voir
/// `visible_face`.
fn emit_rect(mesh: &mut MeshData, quad: &SliceQuad, normal: [f32; 3], block: &Resolved, [top_h, bottom_h]: [u8; 2], (ao, lights, full): (u8, Lights, u8), scale: f32) {
    let [d, u, v] = quad.axes;
    // La face d'un voxel `layer` côté +d est dans le plan `layer + 1` ;
    // côté -d, dans le plan `layer`.
    let plane = quad.layer + i32::from(quad.positive);
    let [w, h] = quad.extent;
    // Liquide : le haut du rectangle descend à sa surface, le bas d'un côté
    // monte à la surface du voisin plus bas.
    let drop = f32::from(FULL - top_h) / f32::from(FULL);
    let lift = f32::from(bottom_h) / f32::from(FULL);
    let (top, bottom) = match (d, u, v) {
        (1, _, _) => (if quad.positive { plane } else { i32::MAX }, i32::MIN),
        (_, 1, _) => (quad.origin[0] + w, quad.origin[0]),
        _ => (quad.origin[1] + h, quad.origin[1]),
    };
    // Ordre des coins pour un enroulement CCW vu de l'extérieur : comme
    // û × v̂ = d̂ (permutation cyclique), parcourir u puis v est CCW pour la
    // face +d ; pour -d on parcourt v puis u (miroir).
    let corners: [[i32; 2]; 4] = if quad.positive {
        [[0, 0], [w, 0], [w, h], [0, h]]
    } else {
        [[0, 0], [0, h], [w, h], [w, 0]]
    };
    let base = mesh.positions.len() as u32;
    let mut shades = [0; 4];
    for (i, [cu, cv]) in corners.into_iter().enumerate() {
        shades[i] = corner_shade(ao, cu > 0, cv > 0);
        let light = corner_light(lights, full, cu > 0, cv > 0);
        let mut p = [0i32; 3];
        p[d] = plane;
        p[u] = quad.origin[0] + cu;
        p[v] = quad.origin[1] + cv;
        let mut y = p[1] as f32;
        if p[1] == top {
            y -= drop;
        }
        if p[1] == bottom {
            y += lift;
        }
        let pos = [p[0] as f32 * scale, y * scale, p[2] as f32 * scale];
        push_vertex(mesh, pos, normal, block, face_layer(block, normal), AO_LIGHT[shades[i] as usize], light);
    }
    push_quad_indices(mesh, base, shades);
}

/// Ombre d'un coin de face, de 0 (le plus sombre) à 3 (rien autour) :
/// devant la face, les deux voisins de côté et celui du coin. Deux côtés
/// pleins ferment le coin, quel que soit le troisième.
fn corner_ao(side_u: bool, side_v: bool, corner: bool) -> u8 {
    if side_u && side_v { 0 } else { 3 - u8::from(side_u) - u8::from(side_v) - u8::from(corner) }
}

/// Les ombres des quatre coins de la face dont la cellule devant est `q`,
/// de normale le long de `d`, dans le plan (u, v). Deux bits par coin, le
/// coin (cu, cv) au rang `cu + 2 cv` (cu, cv : côté bas ou haut de l'axe).
fn face_ao(occludes: impl Fn([i32; 3]) -> bool, q: [i32; 3], [_, u, v]: [usize; 3]) -> u8 {
    let mut packed = 0;
    for (cu, cv) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        let (su, sv) = (if cu == 1 { 1 } else { -1 }, if cv == 1 { 1 } else { -1 });
        let mut side_u = q;
        side_u[u] += su;
        let mut side_v = q;
        side_v[v] += sv;
        let mut corner = side_u;
        corner[v] += sv;
        packed |= corner_ao(occludes(side_u), occludes(side_v), occludes(corner)) << (2 * (cu + 2 * cv));
    }
    packed
}

/// L'ombre du coin (`high_u`, `high_v`) dans les ombres empaquetées.
fn corner_shade(ao: u8, high_u: bool, high_v: bool) -> u8 {
    (ao >> (2 * (u8::from(high_u) + 2 * u8::from(high_v)))) & 3
}

/// La lumière des quatre coins de la face dont la cellule devant est `q` :
/// pour chaque coin, la moyenne des cases qui l'entourent devant la face
/// (celles de [`face_ao`] et `q`), sans celles qui font de l'ombre — elles
/// sont noires, la lumière ne les traverse pas. Un coin fermé par ses deux
/// côtés ne voit pas la case du coin.
fn face_light(occludes: impl Fn([i32; 3]) -> bool, light: impl Fn([i32; 3]) -> [u8; 2], q: [i32; 3], [_, u, v]: [usize; 3]) -> Lights {
    let mut lights = [[0; 2]; 4];
    for (cu, cv) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        let (su, sv) = (if cu == 1 { 1 } else { -1 }, if cv == 1 { 1 } else { -1 });
        let mut side_u = q;
        side_u[u] += su;
        let mut side_v = q;
        side_v[v] += sv;
        let mut corner = side_u;
        corner[v] += sv;
        let (ou, ov) = (occludes(side_u), occludes(side_v));
        let cells = [(q, true), (side_u, !ou), (side_v, !ov), (corner, !(ou && ov || occludes(corner)))];
        let (mut sum, mut count) = ([0u16; 2], 0u16);
        for (cell, counted) in cells {
            if counted {
                let l = light(cell);
                sum = [sum[0] + u16::from(l[0]), sum[1] + u16::from(l[1])];
                count += 1;
            }
        }
        lights[cu + 2 * cv] = sum.map(|x| x * 12 / count);
    }
    lights
}

/// La lumière du coin (`high_u`, `high_v`), dans [0, 1].
fn corner_light(lights: Lights, full: u8, high_u: bool, high_v: bool) -> [f32; 2] {
    let [sky, block] = lights[usize::from(high_u) + 2 * usize::from(high_v)];
    let unit = 12.0 * f32::from(full);
    [f32::from(sky) / unit, f32::from(block) / unit]
}

/// Deux triangles CCW sur les 4 coins du quad. Le GPU interpole la couleur
/// dans chaque triangle : on coupe le long de la diagonale la plus claire,
/// pour qu'un coin sombre isolé reste dans un seul triangle au lieu de
/// tirer une traînée jusqu'au coin opposé.
fn push_quad_indices(mesh: &mut MeshData, base: u32, shades: [u8; 4]) {
    let [a, b, c, d] = shades;
    let tris = if b + d > a + c { [1, 2, 3, 1, 3, 0] } else { [0, 1, 2, 0, 2, 3] };
    mesh.indices.extend(tris.map(|i| base + i));
}

/// La couche du côté, du dessus ou du dessous, selon la normale.
fn face_layer(block: &Resolved, normal: [f32; 3]) -> u32 {
    if normal[1] > 0.0 {
        block.layers[1]
    } else if normal[1] < 0.0 {
        block.layers[2]
    } else {
        block.layers[0]
    }
}

/// Un sommet : position, normale, couleur, et sa texture (couche +
/// coordonnées en mètres, prises dans le plan de la face).
fn push_vertex(mesh: &mut MeshData, pos: [f32; 3], normal: [f32; 3], block: &Resolved, layer: u32, light: f32, sky_block: [f32; 2]) {
    let [x, y, z] = pos;
    // Sur les côtés, v descend avec y : l'image est à l'endroit (le haut du
    // PNG en haut du bloc).
    let uv = if normal[1] != 0.0 {
        [x, z]
    } else if normal[0] != 0.0 {
        [z, -y]
    } else {
        [x, -y]
    };
    mesh.positions.push(pos);
    mesh.normals.push(normal);
    let [r, g, b, a] = block.color;
    mesh.colors.push([r * light, g * light, b * light, a]);
    mesh.uvs.push(uv);
    mesh.layers.push(layer);
    mesh.lights.push(sky_block);
}

/// Ce que le mesher lit d'une entrée de palette, résolu une fois.
struct Resolved {
    id: ContentId,
    solid: bool,
    /// A des faces : solide ou liquide.
    drawn: bool,
    liquid: bool,
    /// Dessiné dans la passe semi-transparente.
    blend: bool,
    /// Cache entièrement ce qu'il y a derrière lui.
    opaque: bool,
    /// Émet de la lumière (`emits_light`) : ses faces sont en pleine
    /// lumière de bloc, pour que le halo (bloom, #51) parte de lui.
    emits: bool,
    color: [f32; 4],
    /// Couche du côté, du dessus, du dessous.
    layers: [u32; 3],
    /// Boîtes d'un bloc à forme (#14) ; vide : le cube.
    shape: Vec<Cuboid>,
}

impl Resolved {
    /// Assombrit les coins des faces voisines : il cache tout derrière lui.
    fn casts_shadow(&self) -> bool {
        self.solid && self.opaque && self.shape.is_empty()
    }
}

fn resolve_palette(chunk: &Chunk, registry: &Registry) -> Vec<Resolved> {
    chunk.palette().iter().map(|&id| resolve(registry, id)).collect()
}

fn resolve(registry: &Registry, id: ContentId) -> Resolved {
    let Some(b) = registry.get(id).and_then(|e| e.block()) else {
        // ID inconnu du registre : on le rend visible et criard plutôt
        // qu'invisible — un bug de contenu doit se voir.
        return Resolved {
            id,
            solid: true,
            drawn: true,
            liquid: false,
            blend: false,
            opaque: true,
            emits: false,
            color: [1.0, 0.0, 1.0, 1.0],
            layers: [NO_TEXTURE; 3],
            shape: Vec::new(),
        };
    };
    // Texture absente ou non chargée : couleur seule.
    let layer = |name: &String| registry.texture_layer(name).unwrap_or(NO_TEXTURE);
    let layers = match &b.texture {
        None => [NO_TEXTURE; 3],
        Some(t) => {
            let side = layer(&t.side);
            [side, t.top.as_ref().map_or(side, layer), t.bottom.as_ref().map_or(side, layer)]
        }
    };
    // Texturé : sommets blancs, la couleur vient de l'image. L'opacité
    // d'un bloc `Blend` passe par l'alpha des sommets.
    let opacity = if let Alpha::Blend(o) = b.alpha { o } else { 1.0 };
    let color = if layers[0] == NO_TEXTURE { [b.color[0], b.color[1], b.color[2], opacity] } else { [1.0, 1.0, 1.0, opacity] };
    let liquid = b.liquid.is_some();
    Resolved {
        id,
        solid: b.solid,
        drawn: b.solid || liquid,
        liquid,
        blend: matches!(b.alpha, Alpha::Blend(_)),
        opaque: b.alpha == Alpha::Opaque,
        emits: b.emits_light.is_some(),
        color,
        layers,
        shape: b.shape.clone().unwrap_or_default(),
    }
}

/// `neighbor` cache-t-il la face de `block` (dessiné) qui le touche ? Un
/// bloc opaque et solide cache tout ; sinon seul un bloc identique cache
/// (la face commune ne se verrait pas). Un liquide, même opaque, ne cache
/// rien d'autre : sa surface est abaissée.
fn hides(neighbor: &Resolved, block: &Resolved) -> bool {
    neighbor.id == block.id || neighbor.casts_shadow()
}

// ---------------------------------------------------------------------------
// Étape 1 conservée : culling naïf. Sert d'oracle aux tests du greedy —
// les deux meshers doivent couvrir exactement la même surface.
// ---------------------------------------------------------------------------

/// Les 6 directions de face d'un cube. L'ordre des 4 coins de chaque face
/// est choisi pour un enroulement CCW vu de l'extérieur.
const FACES: [Face; 6] = [
    // +X
    Face { normal: [1.0, 0.0, 0.0], neighbor: [1, 0, 0],
           corners: [[1, 0, 0], [1, 1, 0], [1, 1, 1], [1, 0, 1]] },
    // -X
    Face { normal: [-1.0, 0.0, 0.0], neighbor: [-1, 0, 0],
           corners: [[0, 0, 1], [0, 1, 1], [0, 1, 0], [0, 0, 0]] },
    // +Y
    Face { normal: [0.0, 1.0, 0.0], neighbor: [0, 1, 0],
           corners: [[0, 1, 0], [0, 1, 1], [1, 1, 1], [1, 1, 0]] },
    // -Y
    Face { normal: [0.0, -1.0, 0.0], neighbor: [0, -1, 0],
           corners: [[0, 0, 1], [0, 0, 0], [1, 0, 0], [1, 0, 1]] },
    // +Z
    Face { normal: [0.0, 0.0, 1.0], neighbor: [0, 0, 1],
           corners: [[1, 0, 1], [1, 1, 1], [0, 1, 1], [0, 0, 1]] },
    // -Z
    Face { normal: [0.0, 0.0, -1.0], neighbor: [0, 0, -1],
           corners: [[0, 0, 0], [0, 1, 0], [1, 1, 0], [1, 0, 0]] },
];

struct Face {
    normal: [f32; 3],
    neighbor: [i32; 3],
    corners: [[u32; 3]; 4],
}

/// Meshe un chunk par culling naïf : un quad 1×1 par face visible.
pub fn mesh_chunk_naive(chunk: &Chunk, registry: &Registry, voxel_size_m: f32) -> MeshData {
    let size = chunk.size();
    let resolved = resolve_palette(chunk, registry);
    let at = |[x, y, z]: [i32; 3]| -> Option<&Resolved> {
        if x < 0 || y < 0 || z < 0 || x >= size as i32 || y >= size as i32 || z >= size as i32 {
            return None; // hors chunk = air
        }
        Some(&resolved[chunk.get_local(x as u32, y as u32, z as u32) as usize])
    };
    let occludes = |q: [i32; 3]| at(q).is_some_and(Resolved::casts_shadow);

    let mut mesh = MeshData::default();
    for z in 0..size {
        for y in 0..size {
            for x in 0..size {
                let block = &resolved[chunk.get_local(x, y, z) as usize];
                if !block.solid {
                    continue;
                }
                for face in &FACES {
                    let nx = x as i32 + face.neighbor[0];
                    let ny = y as i32 + face.neighbor[1];
                    let nz = z as i32 + face.neighbor[2];
                    if !at([nx, ny, nz]).is_some_and(|other| hides(other, block)) {
                        let d = face.normal.iter().position(|&c| c != 0.0).unwrap();
                        let ao = face_ao(occludes, [nx, ny, nz], [d, (d + 1) % 3, (d + 2) % 3]);
                        emit_quad(&mut mesh, [x, y, z], face, block, ao, voxel_size_m);
                    }
                }
            }
        }
    }
    mesh
}

fn emit_quad(mesh: &mut MeshData, voxel: [u32; 3], face: &Face, block: &Resolved, ao: u8, scale: f32) {
    let base = mesh.positions.len() as u32;
    let d = face.normal.iter().position(|&c| c != 0.0).unwrap();
    let shades = face.corners.map(|c| corner_shade(ao, c[(d + 1) % 3] == 1, c[(d + 2) % 3] == 1));
    for (corner, shade) in face.corners.iter().zip(shades) {
        let pos = [
            (voxel[0] + corner[0]) as f32 * scale,
            (voxel[1] + corner[1]) as f32 * scale,
            (voxel[2] + corner[2]) as f32 * scale,
        ];
        push_vertex(mesh, pos, face.normal, block, face_layer(block, face.normal), AO_LIGHT[shade as usize], [1.0, 0.0]); // l'oracle ignore la lumière
    }
    push_quad_indices(mesh, base, shades);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{ContentEntry, Registry};
    use crate::world::VoxelWorld;

    fn test_registry() -> (Registry, ContentId, ContentId) {
        let mut reg = Registry::new();
        let air = reg
            .register(ContentEntry::new_block("core:air", false, [0.0; 3]))
            .unwrap();
        let stone = reg
            .register(ContentEntry::new_block("core:stone", true, [0.5, 0.5, 0.5]))
            .unwrap();
        (reg, air, stone)
    }

    /// Registre à deux blocs solides distincts, pour tester la clé de fusion.
    fn test_registry_two_solids() -> (Registry, ContentId, ContentId, ContentId) {
        let (mut reg, air, stone) = test_registry();
        let dirt = reg
            .register(ContentEntry::new_block("core:dirt", true, [0.4, 0.25, 0.1]))
            .unwrap();
        (reg, air, stone, dirt)
    }

    fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
    }

    fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    }

    /// Aire d'un quad du mesh (produit vectoriel des deux côtés).
    fn quad_area(mesh: &MeshData, quad_idx: usize) -> f32 {
        let base = quad_idx * 4;
        let (p0, p1, p3) = (
            mesh.positions[base],
            mesh.positions[base + 1],
            mesh.positions[base + 3],
        );
        let c = cross(sub(p1, p0), sub(p3, p0));
        (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt()
    }

    /// Aire totale par direction de normale — la signature géométrique d'un
    /// mesh blocky. Deux meshers corrects doivent produire la même.
    fn area_by_normal(mesh: &MeshData) -> std::collections::BTreeMap<[i32; 3], i64> {
        let mut map = std::collections::BTreeMap::new();
        for i in 0..mesh.face_count() {
            let n = mesh.normals[i * 4];
            let key = [n[0] as i32, n[1] as i32, n[2] as i32];
            // Aires entières (voxel_size 1.0) : pas de flottant dans la clé.
            *map.entry(key).or_insert(0) += quad_area(mesh, i).round() as i64;
        }
        map
    }

    #[test]
    fn empty_chunk_produces_empty_mesh() {
        let (reg, air, _) = test_registry();
        let chunk = Chunk::filled(8, air);
        assert!(mesh_chunk(&chunk, &reg, 1.0).is_empty());
        assert!(mesh_chunk_naive(&chunk, &reg, 1.0).is_empty());
    }

    #[test]
    fn lone_voxel_has_six_faces() {
        let (reg, air, stone) = test_registry();
        let mut chunk = Chunk::filled(8, air);
        chunk.set(3, 3, 3, stone);
        for mesh in [mesh_chunk(&chunk, &reg, 1.0), mesh_chunk_naive(&chunk, &reg, 1.0)] {
            assert_eq!(mesh.face_count(), 6);
            assert_eq!(mesh.positions.len(), 24); // 6 faces × 4 sommets
        }
    }

    #[test]
    fn naive_touching_faces_are_culled() {
        // Deux voxels côte à côte : 12 faces − 2 au contact = 10.
        let (reg, air, stone) = test_registry();
        let mut chunk = Chunk::filled(8, air);
        chunk.set(3, 3, 3, stone);
        chunk.set(4, 3, 3, stone);
        assert_eq!(mesh_chunk_naive(&chunk, &reg, 1.0).face_count(), 10);
    }

    #[test]
    fn mask_block_shows_its_neighbours_but_not_its_twin() {
        // Pierre, feuille, feuille en ligne : la pierre garde sa face vers la
        // feuille (on la voit par les trous), les deux feuilles perdent leur
        // face commune ; celle collée à la pierre reste cachée. Faces :
        // pierre 6, feuilles 5 + 4.
        let (mut reg, air, stone) = test_registry();
        let mut leaves = ContentEntry::new_block("core:leaves", true, [0.2, 0.5, 0.2]);
        if let crate::registry::Kind::Block(b) = &mut leaves.kind {
            b.alpha = Alpha::Mask;
        }
        let leaves = reg.register(leaves).unwrap();
        let mut chunk = Chunk::filled(8, air);
        chunk.set(2, 3, 3, stone);
        chunk.set(3, 3, 3, leaves);
        chunk.set(4, 3, 3, leaves);
        assert_eq!(mesh_chunk_naive(&chunk, &reg, 1.0).face_count(), 15);
        assert_eq!(area_by_normal(&mesh_chunk(&chunk, &reg, 1.0)), area_by_normal(&mesh_chunk_naive(&chunk, &reg, 1.0)));
    }

    #[test]
    fn greedy_merges_coplanar_faces() {
        // Deux voxels côte à côte, même matériau : les 4 faces latérales
        // fusionnent chacune en un quad 2×1 → 6 quads au lieu de 10.
        let (reg, air, stone) = test_registry();
        let mut chunk = Chunk::filled(8, air);
        chunk.set(3, 3, 3, stone);
        chunk.set(4, 3, 3, stone);
        assert_eq!(mesh_chunk(&chunk, &reg, 1.0).face_count(), 6);
    }

    #[test]
    fn greedy_full_chunk_is_six_quads() {
        // Un chunk 8³ plein : le naïf émet 6×8×8 = 384 quads de surface,
        // le greedy exactement 6 (un par face du cube).
        let (reg, _, stone) = test_registry();
        let chunk = Chunk::filled(8, stone);
        assert_eq!(mesh_chunk(&chunk, &reg, 1.0).face_count(), 6);
        assert_eq!(mesh_chunk_naive(&chunk, &reg, 1.0).face_count(), 384);
    }

    #[test]
    fn greedy_does_not_merge_different_materials() {
        // Pierre et terre côte à côte : la clé de fusion (index de palette)
        // interdit la fusion à travers la frontière de matériau. 2 faces en
        // bout + 4×2 faces latérales non fusionnables = 10 quads.
        let (reg, air, stone, dirt) = test_registry_two_solids();
        let mut chunk = Chunk::filled(8, air);
        chunk.set(3, 3, 3, stone);
        chunk.set(4, 3, 3, dirt);
        let mesh = mesh_chunk(&chunk, &reg, 1.0);
        assert_eq!(mesh.face_count(), 10);
    }

    #[test]
    fn buried_voxel_emits_nothing() {
        // Cube 3×3×3 plein : seule la surface est émise — le voxel central
        // ne contribue à rien. Naïf : 54 quads 1×1 ; greedy : 6 quads 3×3.
        let (reg, air, stone) = test_registry();
        let mut chunk = Chunk::filled(8, air);
        for z in 0..3 {
            for y in 0..3 {
                for x in 0..3 {
                    chunk.set(x, y, z, stone);
                }
            }
        }
        assert_eq!(mesh_chunk_naive(&chunk, &reg, 1.0).face_count(), 54);
        assert_eq!(mesh_chunk(&chunk, &reg, 1.0).face_count(), 6);
    }

    #[test]
    fn greedy_covers_same_surface_as_naive() {
        // L'oracle : sur un terrain irrégulier (motif déterministe mélangeant
        // air, pierre et terre), le greedy doit couvrir exactement la même
        // aire que le naïf, direction par direction. Toute face manquante,
        // dupliquée ou débordante casse cette égalité.
        let (reg, air, stone, dirt) = test_registry_two_solids();
        let mut chunk = Chunk::filled(8, air);
        for z in 0..8u32 {
            for y in 0..8u32 {
                for x in 0..8u32 {
                    match (x * 3 + y * 5 + z * 7) % 5 {
                        0 | 1 => chunk.set(x, y, z, stone),
                        2 => chunk.set(x, y, z, dirt),
                        _ => {}
                    }
                }
            }
        }
        let greedy = mesh_chunk(&chunk, &reg, 1.0);
        let naive = mesh_chunk_naive(&chunk, &reg, 1.0);
        assert_eq!(area_by_normal(&greedy), area_by_normal(&naive));
        // Et le gain existe bel et bien.
        assert!(greedy.face_count() < naive.face_count());

        assert_same_shading(&greedy, &naive);
    }

    /// Même ombre : chaque sommet du greedy a la couleur d'un sommet du naïf
    /// au même point, de même normale.
    fn assert_same_shading(greedy: &MeshData, naive: &MeshData) {
        let vertex = |m: &MeshData, i: usize| (m.positions[i].map(|c| c as i32), m.normals[i].map(|c| c as i32), m.colors[i].map(f32::to_bits));
        let naive_vertices: std::collections::HashSet<_> = (0..naive.positions.len()).map(|i| vertex(naive, i)).collect();
        for i in 0..greedy.positions.len() {
            assert!(naive_vertices.contains(&vertex(greedy, i)), "sommet {:?} absent du naïf", vertex(greedy, i));
        }
    }

    /// Lumière du sommet en `pos` de normale `normal` (part de la couleur
    /// du bloc, 0.5 pour la pierre de test).
    fn light_at(mesh: &MeshData, pos: [f32; 3], normal: [f32; 3]) -> f32 {
        let i = (0..mesh.positions.len()).find(|&i| mesh.positions[i] == pos && mesh.normals[i] == normal).unwrap();
        mesh.colors[i][0] / 0.5
    }

    #[test]
    fn corners_against_walls_are_darker() {
        // Un sol de pierre et deux murs le long de x = 0 et z = 0 : le pied
        // d'un mur est assombri, le coin entre les deux murs l'est le plus,
        // le milieu du sol pas du tout.
        let (reg, air, stone) = test_registry();
        let mut chunk = Chunk::filled(8, air);
        for (x, z) in (0..8).flat_map(|x| (0..8).map(move |z| (x, z))) {
            chunk.set(x, 0, z, stone);
            if x == 0 || z == 0 {
                chunk.set(x, 1, z, stone);
            }
        }
        let up = [0.0, 1.0, 0.0];
        let naive = mesh_chunk_naive(&chunk, &reg, 1.0);
        assert_eq!(light_at(&naive, [1.0, 1.0, 1.0], up), AO_LIGHT[0], "coin entre deux murs");
        assert_eq!(light_at(&naive, [1.0, 1.0, 4.0], up), AO_LIGHT[1], "pied d'un mur");
        assert_eq!(light_at(&naive, [4.0, 1.0, 4.0], up), AO_LIGHT[3], "milieu du sol");
        // Le greedy rend la même ombre, en coupant le sol là où elle change.
        let greedy = mesh_chunk(&chunk, &reg, 1.0);
        assert_same_shading(&greedy, &naive);
        assert_eq!(light_at(&greedy, [1.0, 1.0, 1.0], up), AO_LIGHT[0]);
    }

    #[test]
    fn a_lone_dark_corner_stays_in_one_triangle() {
        // Un bloc posé en diagonale d'un sol : le coin du sol qu'il touche
        // est le seul sombre de son quad. Il ne doit être que dans un des
        // deux triangles, sinon l'ombre file jusqu'au coin opposé.
        let (reg, air, stone) = test_registry();
        let mut chunk = Chunk::filled(8, air);
        for (x, z) in (2..5).flat_map(|x| (2..5).map(move |z| (x, z))) {
            chunk.set(x, 0, z, stone);
        }
        chunk.set(1, 1, 1, stone);
        for mesh in [mesh_chunk(&chunk, &reg, 1.0), mesh_chunk_naive(&chunk, &reg, 1.0)] {
            let mut lone = 0;
            for quad in mesh.indices.chunks(6) {
                let mut corners: Vec<u32> = quad.to_vec();
                corners.sort();
                corners.dedup();
                let light = |i: u32| mesh.colors[i as usize][0];
                let darkest = *corners.iter().min_by(|a, b| light(**a).total_cmp(&light(**b))).unwrap();
                let others: Vec<u32> = corners.iter().copied().filter(|&c| c != darkest).collect();
                if others.iter().all(|&c| light(c) == light(others[0]) && light(c) > light(darkest)) {
                    lone += 1;
                    assert_eq!(quad.iter().filter(|&&i| i == darkest).count(), 1, "coin sombre partagé par les deux triangles");
                }
            }
            assert!(lone > 0, "aucun coin sombre isolé dans la scène");
        }
    }

    #[test]
    fn world_meshing_culls_chunk_border_faces() {
        // Deux chunks 8³ pleins côte à côte : la face commune (celle que le
        // meshing « chunk isolé » émet toujours) doit disparaître des deux
        // côtés. 6 quads seul → 5 quads chacun une fois raccordés.
        let (reg, _, stone) = test_registry();
        let mut world = VoxelWorld::new(reg, 8, 1.0);
        let a = ChunkPos { x: 0, y: 0, z: 0 };
        let b = ChunkPos { x: 1, y: 0, z: 0 };
        world.insert_chunk(a, Chunk::filled(8, stone));

        // Seul : 6 faces, comme mesh_chunk.
        assert_eq!(mesh_chunk_in_world(&world, a, 1.0).unwrap()[0].face_count(), 6);

        world.insert_chunk(b, Chunk::filled(8, stone));
        for pos in [a, b] {
            let [mesh, _] = mesh_chunk_in_world(&world, pos, 1.0).unwrap();
            assert_eq!(mesh.face_count(), 5, "face de bordure non culled en {pos:?}");
        }
    }

    #[test]
    fn water_goes_to_the_blend_mesh_with_a_lowered_surface() {
        let reg = Registry::from_ron(
            r#"[
                (identifier: "t:air", kind: Block((solid: false, color: (0.0, 0.0, 0.0)))),
                (identifier: "t:stone", kind: Block((solid: true, color: (0.5, 0.5, 0.5)))),
                (identifier: "t:water", kind: Block((solid: false, color: (0.2, 0.3, 0.9), alpha: Blend(0.6),
                    liquid: Some((spread: 3.0, delay: 0.25))))),
            ]"#,
        )
        .unwrap();
        let id = |s| reg.lookup(s).unwrap();
        let mut chunk = Chunk::filled(4, id("t:air"));
        for (x, z) in (0..4).flat_map(|x| (0..4).map(move |z| (x, z))) {
            chunk.set(x, 0, z, id("t:stone"));
        }
        chunk.set(1, 1, 1, id("t:water"));
        chunk.set(2, 1, 1, id("t:water"));
        let mut world = VoxelWorld::new(reg, 4, 1.0);
        world.insert_chunk(ChunkPos { x: 0, y: 0, z: 0 }, chunk);

        let [solid, blend] = mesh_chunk_in_world(&world, ChunkPos { x: 0, y: 0, z: 0 }, 1.0).unwrap();
        // L'eau ne cache pas la pierre : le sol reste une seule dalle.
        assert_eq!(solid.face_count(), 6);
        // Deux sources côte à côte : dessus fusionné, 2 bouts, 2 côtés
        // fusionnés ; le dessous est caché par la pierre.
        assert_eq!(blend.face_count(), 5);
        let top = blend.positions.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
        assert!((top - (1.0 + liquids::SURFACE)).abs() < 0.01, "surface à {top}");
        assert!(blend.colors.iter().all(|c| (c[3] - 0.6).abs() < 1e-6));

        // L'eau coule à côté de la source, plus bas : le côté de la source
        // se voit, de la surface de l'eau qui coule à la sienne.
        world.set_liquid_level([2, 1, 1], Some(1));
        let [_, blend] = mesh_chunk_in_world(&world, ChunkPos { x: 0, y: 0, z: 0 }, 1.0).unwrap();
        let low = 1.0 + liquids::height(Some(1), 3);
        let step: Vec<f32> = (0..blend.positions.len())
            .filter(|&i| blend.normals[i] == [1.0, 0.0, 0.0] && (blend.positions[i][0] - 2.0).abs() < 1e-6)
            .map(|i| blend.positions[i][1])
            .collect();
        assert_eq!(step.len(), 4, "une face entre la source et l'eau qui coule");
        assert!(step.iter().all(|&y| (y - low).abs() < 0.01 || (y - (1.0 + liquids::SURFACE)).abs() < 0.01), "{step:?}");
    }

    #[test]
    fn world_meshing_of_missing_chunk_is_none() {
        let (reg, _, _) = test_registry();
        let world = VoxelWorld::new(reg, 8, 1.0);
        assert!(mesh_chunk_in_world(&world, ChunkPos { x: 0, y: 0, z: 0 }, 1.0).is_none());
    }

    #[test]
    fn positions_are_scaled_to_meters() {
        // voxel_size 0.5 m (2 vox/m) : un voxel en (2,0,0) s'étend de
        // x=1.0 m à x=1.5 m dans le mesh.
        let (reg, air, stone) = test_registry();
        let mut chunk = Chunk::filled(4, air);
        chunk.set(2, 0, 0, stone);
        let mesh = mesh_chunk(&chunk, &reg, 0.5);
        let xs: Vec<f32> = mesh.positions.iter().map(|p| p[0]).collect();
        assert!(xs.iter().all(|&x| (1.0..=1.5).contains(&x)));
        assert!(xs.contains(&1.0) && xs.contains(&1.5));
    }

    #[test]
    fn merged_face_repeats_texture_in_meters() {
        // Dalle d'herbe 5 × 1 × 3 : le dessus est un seul quad, dont les
        // coordonnées de texture couvrent 5 × 3 (une image par mètre).
        let mut reg = Registry::from_ron(include_str!("../../../assets/content/core.ron")).unwrap();
        reg.load_textures(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/textures"))
            .unwrap();
        let (air, grass) = (reg.lookup("core:air").unwrap(), reg.lookup("core:grass").unwrap());
        let faces = reg.get(grass).unwrap().block().unwrap().texture.clone().unwrap();
        let top_layer = reg.texture_layer(faces.top.as_ref().unwrap()).unwrap();
        let side_layer = reg.texture_layer(&faces.side).unwrap();
        let mut chunk = Chunk::filled(8, air);
        for z in 0..3 {
            for x in 0..5 {
                chunk.set(x, 0, z, grass);
            }
        }
        let mesh = mesh_chunk(&chunk, &reg, 1.0);
        let top: Vec<usize> = (0..mesh.normals.len()).filter(|&i| mesh.normals[i][1] > 0.0).collect();
        assert_eq!(top.len(), 4, "un seul quad");
        assert!(top.iter().all(|&i| mesh.layers[i] == top_layer && mesh.colors[i] == [1.0; 4]));
        let span = |axis: usize| {
            let vals = top.iter().map(|&i| mesh.uvs[i][axis]);
            vals.clone().fold(f32::MIN, f32::max) - vals.fold(f32::MAX, f32::min)
        };
        assert_eq!((span(0), span(1)), (5.0, 3.0));
        let side = (0..mesh.normals.len()).find(|&i| mesh.normals[i][1] == 0.0).unwrap();
        assert_eq!(mesh.layers[side], side_layer);

        // Sans texture : couleur du registre, pas de couche.
        let (reg, air, stone) = test_registry();
        let mut chunk = Chunk::filled(4, air);
        chunk.set(0, 0, 0, stone);
        let mesh = mesh_chunk(&chunk, &reg, 1.0);
        assert!(mesh.layers.iter().all(|&l| l == NO_TEXTURE));
        assert_eq!(mesh.colors[0], [0.5, 0.5, 0.5, 1.0]);
    }

    #[test]
    fn winding_is_ccw_seen_from_outside() {
        // Pour chaque triangle, la normale géométrique (produit vectoriel)
        // doit pointer dans le même sens que la normale déclarée — sinon
        // le back-face culling du GPU mangera la face. Testé sur une dalle
        // 2×1×3 pour exercer des quads fusionnés dans les 6 directions.
        let (reg, air, stone) = test_registry();
        let mut chunk = Chunk::filled(8, air);
        for z in 1..4 {
            for x in 1..3 {
                chunk.set(x, 1, z, stone);
            }
        }
        // Un bloc posé dessus : des coins ombrés, donc des quads coupés
        // selon l'autre diagonale.
        chunk.set(1, 2, 1, stone);
        for mesh in [mesh_chunk(&chunk, &reg, 1.0), mesh_chunk_naive(&chunk, &reg, 1.0)] {
            for tri in mesh.indices.chunks(3) {
                let [a, b, c] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
                let (pa, pb, pc) = (mesh.positions[a], mesh.positions[b], mesh.positions[c]);
                let c = cross(sub(pb, pa), sub(pc, pa));
                let n = mesh.normals[a];
                let dot = c[0] * n[0] + c[1] * n[1] + c[2] * n[2];
                assert!(dot > 0.0, "triangle enroulé à l'envers");
            }
        }
    }

    /// Sous le ciel, le sol est un seul quad en pleine lumière ; une lampe
    /// posée dessus éclaire les coins voisins et coupe la fusion.
    #[test]
    fn faces_carry_sky_and_lamp_light() {
        let (mut reg, air, stone) = test_registry();
        let mut lamp = ContentEntry::new_block("core:lamp", true, [1.0; 3]);
        if let crate::registry::Kind::Block(b) = &mut lamp.kind {
            b.emits_light = Some(4.0);
        }
        let lamp = reg.register(lamp).unwrap();
        let mut chunk = Chunk::filled(8, air);
        for x in 0..8 {
            for z in 0..8 {
                chunk.set(x, 0, z, stone);
            }
        }
        let pos = ChunkPos { x: 0, y: 0, z: 0 };
        let mut world = VoxelWorld::new(reg, 8, 1.0);
        world.insert_chunk(pos, chunk);
        // Sommets du dessus du sol (y = 1, normale +y) : position → lumière.
        let floor = |world: &VoxelWorld| {
            let [mesh, _] = mesh_chunk_in_world(world, pos, 1.0).unwrap();
            (0..mesh.positions.len())
                .filter(|&i| mesh.normals[i] == [0.0, 1.0, 0.0] && mesh.positions[i][1] == 1.0)
                .map(|i| (mesh.positions[i], mesh.lights[i]))
                .collect::<Vec<_>>()
        };

        let open = floor(&world);
        assert_eq!(open.len(), 4, "un seul quad");
        assert!(open.iter().all(|&(_, l)| l == [1.0, 0.0]));

        world.set_voxel([4, 1, 4], lamp);
        let lit = floor(&world);
        assert!(lit.len() > 4, "la lumière coupe la fusion");
        let at = |p: [f32; 3]| lit.iter().find(|&&(q, _)| q == p).unwrap().1;
        // Coin de la lampe : moyenne des trois cases d'air autour (8, 12,
        // 12 niveaux sur 60), la lampe elle-même ne compte pas.
        assert!((at([4.0, 1.0, 4.0])[1] - 32.0 / 3.0 / 60.0).abs() < 1e-6);
        assert_eq!(at([0.0, 1.0, 0.0]), [1.0, 0.0], "hors de portée");

        // Les faces de la lampe elle-même : pleine lumière de bloc, sinon
        // elle ne brillerait pas plus que le sol voisin (bloom, #51).
        let [mesh, _] = mesh_chunk_in_world(&world, pos, 1.0).unwrap();
        let lamp_top = (0..mesh.positions.len())
            .filter(|&i| mesh.normals[i] == [0.0, 1.0, 0.0] && mesh.positions[i][1] == 2.0)
            .map(|i| mesh.lights[i][1])
            .collect::<Vec<_>>();
        assert_eq!(lamp_top, [1.0; 4]);
    }
}
