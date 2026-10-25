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

use crate::chunk::{Chunk, ChunkPos};
use crate::liquids;
use crate::registry::{Alpha, ContentId, Registry};
use crate::world::VoxelWorld;

/// Hauteur d'une face pleine dans la clé de fusion (sur 255).
const FULL: u8 = u8::MAX;

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
    mesh_chunk_with(chunk, registry, voxel_size_m, false, |_| None, |_| None)
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
    let chunk = world.chunk(pos)?;
    let size = world.chunk_size() as i64;
    let base = [
        pos.x as i64 * size,
        pos.y as i64 * size,
        pos.z as i64 * size,
    ];
    let at = |p: [i32; 3]| [base[0] + p[0] as i64, base[1] + p[1] as i64, base[2] + p[2] as i64];
    let reg = &world.registry;
    let vpm = world.voxels_per_meter();
    let mesh = |blend| {
        mesh_chunk_with(chunk, reg, voxel_size_m, blend, |p| world.voxel(at(p)), |p| {
            world.liquid(world.voxel(at(p))?).map(|l| liquids::height(world.liquid_level(at(p)), liquids::reach(&l, vpm)))
        })
    };
    Some([mesh(false), mesh(true)])
}

/// Cœur du greedy : `outside` donne le contenu des coordonnées (locales au
/// chunk) hors bornes, `None` = rien (air) — c'est là que se joue le raccord.
/// `blend` : la passe semi-transparente, sinon le reste. `surface` : la
/// hauteur d'un liquide sous de l'air (part de voxel), `None` pour un bloc
/// plein.
fn mesh_chunk_with(
    chunk: &Chunk,
    registry: &Registry,
    voxel_size_m: f32,
    blend: bool,
    outside: impl Fn([i32; 3]) -> Option<ContentId>,
    surface: impl Fn([i32; 3]) -> Option<f32>,
) -> MeshData {
    let size = chunk.size() as i32;
    let n = chunk.size() as usize;
    let resolved = resolve_palette(chunk, registry);
    let inside = |q: [i32; 3]| q.iter().all(|&c| (0..size).contains(&c));
    let id_at = |q: [i32; 3]| {
        if inside(q) { Some(resolved[chunk.get_local(q[0] as u32, q[1] as u32, q[2] as u32) as usize].id) } else { outside(q) }
    };

    // Hauteur du liquide `id` en `p` (sur 255) : sous autre chose que
    // lui-même, sa surface est abaissée.
    let height_at = |p: [i32; 3], id: ContentId| {
        if id_at([p[0], p[1] + 1, p[2]]) == Some(id) {
            FULL
        } else {
            surface(p).map_or(FULL, |h| (h * f32::from(FULL)).round() as u8)
        }
    };

    // (index de palette, haut, bas) de `p` si sa face vers `q` se voit ;
    // haut et bas sur 255, `FULL` et 0 pour une face entière. `p` est
    // toujours dans le chunk, `q` peut en sortir.
    let visible_face = |p: [i32; 3], q: [i32; 3]| -> Option<(u16, u8, u8)> {
        let idx = chunk.get_local(p[0] as u32, p[1] as u32, p[2] as u32);
        let block = &resolved[idx as usize];
        if !block.drawn || block.blend != blend {
            return None;
        }
        let height = if block.liquid { height_at(p, block.id) } else { FULL };
        let beyond;
        let neighbor = if inside(q) {
            Some(&resolved[chunk.get_local(q[0] as u32, q[1] as u32, q[2] as u32) as usize])
        } else {
            beyond = outside(q).map(|id| resolve(registry, id));
            beyond.as_ref()
        };
        // Côté d'un liquide contre le même liquide plus bas : seule la
        // différence de hauteur se voit (une pente d'eau, une chute dans un
        // lac).
        if block.liquid && q[1] == p[1] && neighbor.is_some_and(|o| o.id == block.id) {
            let theirs = height_at(q, block.id);
            return (theirs < height).then_some((idx, height, theirs));
        }
        let hidden = neighbor.is_some_and(|other| hides(other, block));
        // Surface abaissée : un bloc posé dessus ne la touche pas.
        let lowered_top = height < FULL && q[1] > p[1];
        (lowered_top || !hidden).then_some((idx, height, 0))
    };

    let mut mesh = MeshData::default();
    // Masque 2D réutilisé pour chaque tranche : None = pas de face visible,
    // Some(clé) = face visible, la clé de fusion étant (index de palette,
    // haut, bas).
    let mut mask: Vec<Option<(u16, u8, u8)>> = vec![None; n * n];

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
                        let cell = visible_face(p, q);
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
                        emit_rect(&mut mesh, &quad, normal, &resolved[key.0 as usize], [key.1, key.2], voxel_size_m);
                    }
                }
            }
        }
    }
    mesh
}

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

/// `[haut, bas]` : la part de voxel couverte (sur 255), voir `visible_face`.
fn emit_rect(mesh: &mut MeshData, quad: &SliceQuad, normal: [f32; 3], block: &Resolved, [top_h, bottom_h]: [u8; 2], scale: f32) {
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
    for [cu, cv] in corners {
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
        push_vertex(mesh, pos, normal, block);
    }
    // Deux triangles CCW sur les coins [0,1,2] et [0,2,3].
    mesh.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// Un sommet : position, normale, couleur, et sa texture (couche +
/// coordonnées en mètres, prises dans le plan de la face).
fn push_vertex(mesh: &mut MeshData, pos: [f32; 3], normal: [f32; 3], block: &Resolved) {
    let [x, y, z] = pos;
    // Sur les côtés, v descend avec y : l'image est à l'endroit (le haut du
    // PNG en haut du bloc).
    let (uv, layer) = if normal[1] > 0.0 {
        ([x, z], block.layers[1])
    } else if normal[1] < 0.0 {
        ([x, z], block.layers[2])
    } else if normal[0] != 0.0 {
        ([z, -y], block.layers[0])
    } else {
        ([x, -y], block.layers[0])
    };
    mesh.positions.push(pos);
    mesh.normals.push(normal);
    mesh.colors.push(block.color);
    mesh.uvs.push(uv);
    mesh.layers.push(layer);
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
    color: [f32; 4],
    /// Couche du côté, du dessus, du dessous.
    layers: [u32; 3],
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
            color: [1.0, 0.0, 1.0, 1.0],
            layers: [NO_TEXTURE; 3],
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
        color,
        layers,
    }
}

/// `neighbor` cache-t-il la face de `block` (dessiné) qui le touche ? Un
/// bloc opaque et solide cache tout ; sinon seul un bloc identique cache
/// (la face commune ne se verrait pas). Un liquide, même opaque, ne cache
/// rien d'autre : sa surface est abaissée.
fn hides(neighbor: &Resolved, block: &Resolved) -> bool {
    neighbor.id == block.id || (neighbor.solid && neighbor.opaque)
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
    let hidden_by = |x: i32, y: i32, z: i32, block: &Resolved| -> bool {
        if x < 0 || y < 0 || z < 0 || x >= size as i32 || y >= size as i32 || z >= size as i32 {
            return false; // hors chunk = air
        }
        hides(&resolved[chunk.get_local(x as u32, y as u32, z as u32) as usize], block)
    };

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
                    if !hidden_by(nx, ny, nz, block) {
                        emit_quad(&mut mesh, [x, y, z], face, block, voxel_size_m);
                    }
                }
            }
        }
    }
    mesh
}

fn emit_quad(mesh: &mut MeshData, voxel: [u32; 3], face: &Face, block: &Resolved, scale: f32) {
    let base = mesh.positions.len() as u32;
    for corner in &face.corners {
        let pos = [
            (voxel[0] + corner[0]) as f32 * scale,
            (voxel[1] + corner[1]) as f32 * scale,
            (voxel[2] + corner[2]) as f32 * scale,
        ];
        push_vertex(mesh, pos, face.normal, block);
    }
    // Deux triangles CCW sur les coins [0,1,2] et [0,2,3].
    mesh.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{ContentEntry, Registry};

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
}
