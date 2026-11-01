//! Icônes d'inventaire (#70, design doc §3.11) : le moteur dessine lui-même
//! l'apparence d'une entrée en pixels, vue de 3/4, une fois au chargement
//! du contenu. Une entrée n'apporte jamais d'icône à part : un item généré
//! a sa case d'inventaire sans rien d'autre.
//!
//! **Pourquoi dessiner nous-mêmes** plutôt qu'une caméra Bevy : c'est
//! testable sans fenêtre, identique sur toute machine, et coûte 0 par image
//! (options écartées : #70). Le principe est celui d'un GPU en petit :
//! projeter chaque triangle, remplir ses pixels, garder le plus proche
//! (tampon de profondeur), lire la texture au point touché.

use crate::mesher::{mesh_item, MeshData, NO_TEXTURE};
use crate::registry::{ContentId, Registry, Texture};

/// L'icône de `id`, RGBA 8 bits de `size` × `size` ligne par ligne depuis
/// le haut ; `None` sans apparence (item sans `model`).
pub fn icon(registry: &Registry, id: ContentId, size: u32) -> Option<Vec<u8>> {
    mesh_item(registry, id, 1.0).map(|mesh| draw(&mesh, registry.textures(), size))
}

/// Dessine `mesh` vu d'en haut, de l'avant-droite (on voit les faces +X,
/// +Y et +Z), en projection orthogonale, cadré pour remplir l'image. Faces
/// éclairées d'une lumière fixe : dessus clair, côtés plus sombres.
pub fn draw(mesh: &MeshData, textures: &[Texture], size: u32) -> Vec<u8> {
    // Repère de l'écran : tourné de 45° autour de y, penché de 30°.
    let (s, c) = (0.5_f32, 0.75_f32.sqrt()); // sin et cos de 30°
    let h = std::f32::consts::FRAC_1_SQRT_2;
    let right = [h, 0.0, -h];
    let up = [-s * h, c, -s * h];
    let toward = [c * h, s, c * h]; // vers l'œil : plus grand = plus proche
    let dot = |p: [f32; 3], d: [f32; 3]| p[0] * d[0] + p[1] * d[1] + p[2] * d[2];
    let projected: Vec<[f32; 3]> = mesh.positions.iter().map(|&p| [dot(p, right), dot(p, up), dot(p, toward)]).collect();

    let mut rgba = vec![0u8; (size * size * 4) as usize];
    if projected.is_empty() {
        return rgba;
    }
    // Cadrage : la plus grande dimension remplit l'image, une marge d'un pixel.
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for p in &projected {
        for a in 0..2 {
            lo[a] = lo[a].min(p[a]);
            hi[a] = hi[a].max(p[a]);
        }
    }
    let span = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(f32::EPSILON);
    let scale = (size as f32 - 2.0) / span;
    let mid = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    let half = size as f32 / 2.0;
    let screen = |p: [f32; 3]| [half + (p[0] - mid[0]) * scale, half - (p[1] - mid[1]) * scale];

    let mut depth = vec![f32::MIN; (size * size) as usize];
    for tri in mesh.indices.chunks_exact(3) {
        let [i, j, k] = [tri[0], tri[1], tri[2]].map(|i| i as usize);
        let [a, b, d] = [i, j, k].map(|v| screen(projected[v]));
        let area = edge(a, b, d);
        if area.abs() < f32::EPSILON {
            continue;
        }
        let n = mesh.normals[i];
        let shade = if n[1] > 0.0 { 1.0 } else if n[1] < 0.0 { 0.5 } else if n[0] != 0.0 { 0.8 } else { 0.6 };
        let texture = textures.get(mesh.layers[i] as usize).filter(|_| mesh.layers[i] != NO_TEXTURE);
        let x0 = a[0].min(b[0]).min(d[0]).floor().max(0.0) as u32;
        let x1 = (a[0].max(b[0]).max(d[0]).ceil() as u32).min(size);
        let y0 = a[1].min(b[1]).min(d[1]).floor().max(0.0) as u32;
        let y1 = (a[1].max(b[1]).max(d[1]).ceil() as u32).min(size);
        for y in y0..y1 {
            for x in x0..x1 {
                let p = [x as f32 + 0.5, y as f32 + 0.5];
                // Coordonnées barycentriques : le poids de chaque sommet.
                let w = [edge(b, d, p) / area, edge(d, a, p) / area, edge(a, b, p) / area];
                if w.iter().any(|&w| w < 0.0) {
                    continue;
                }
                let mix = |f: &dyn Fn(usize) -> f32| w[0] * f(i) + w[1] * f(j) + w[2] * f(k);
                let z = mix(&|v| projected[v][2]);
                let at = (y * size + x) as usize;
                if z <= depth[at] {
                    continue;
                }
                let texel = match texture {
                    Some(t) => sample(t, [mix(&|v| mesh.uvs[v][0]), mix(&|v| mesh.uvs[v][1])]),
                    None => [255; 4],
                };
                // Pixel transparent (feuilles, verre) : on voit derrière.
                if texel[3] < 128 {
                    continue;
                }
                depth[at] = z;
                let color = mesh.colors[i];
                for ch in 0..3 {
                    rgba[at * 4 + ch] = (f32::from(texel[ch]) * color[ch] * shade).round().clamp(0.0, 255.0) as u8;
                }
                rgba[at * 4 + 3] = (255.0 * color[3]).round() as u8;
            }
        }
    }
    rgba
}

/// Deux fois l'aire signée du triangle (`a`, `b`, `p`).
fn edge(a: [f32; 2], b: [f32; 2], p: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}

/// Le pixel de `t` en `uv`, répété comme le sampler du GPU (une texture
/// par unité de coordonnée).
fn sample(t: &Texture, uv: [f32; 2]) -> [u8; 4] {
    let px = |c: f32| ((c.rem_euclid(1.0) * t.size as f32) as u32).min(t.size - 1);
    let at = ((px(uv[1]) * t.size + px(uv[0])) * 4) as usize;
    [t.rgba[at], t.rgba[at + 1], t.rgba[at + 2], t.rgba[at + 3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Registry {
        Registry::from_ron(
            r#"[(identifier: "t:air", kind: Block((solid: false, color: (0.0, 0.0, 0.0)))),
                (identifier: "t:box", kind: Block((solid: true, color: (1.0, 1.0, 1.0)))),
                (identifier: "t:bread", kind: Item),
                (identifier: "t:pork", kind: Item, model: [((-4, 0, -3), (4, 3, 3), "meat")])]"#,
        )
        .unwrap()
    }

    fn pixel(rgba: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * size + x) * 4) as usize;
        [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
    }

    #[test]
    fn a_block_icon_shows_its_top_and_its_sides_shaded_and_textured_the_right_way_up() {
        let reg = registry();
        let size = 32;
        // Textures 2 × 2 : côtés moitié haute rouge, moitié basse bleue ;
        // dessus vert. Couche 0 sur les côtés, 1 dessus.
        let px = |c: [u8; 4]| c.repeat(2);
        let side = Texture { name: "side".into(), size: 2, rgba: [px([255, 0, 0, 255]), px([0, 0, 255, 255])].concat() };
        let top = Texture { name: "top".into(), size: 2, rgba: px([0, 255, 0, 255]).repeat(2) };
        let mut mesh = mesh_item(&reg, reg.lookup("t:box").unwrap(), 1.0).unwrap();
        mesh.layers = mesh.normals.iter().map(|n| u32::from(n[1] > 0.0)).collect();
        let rgba = draw(&mesh, &[side, top], size);
        // Dessus, en haut au centre : vert plein.
        assert_eq!(pixel(&rgba, size, 16, 6), [0, 255, 0, 255]);
        // Côté gauche (+Z) : rouge en haut, bleu en bas, ombré à 0,6.
        assert_eq!(pixel(&rgba, size, 6, 14), [153, 0, 0, 255]);
        assert_eq!(pixel(&rgba, size, 6, 22), [0, 0, 153, 255]);
        // Côté droit (+X) : ombré à 0,8.
        assert_eq!(pixel(&rgba, size, 26, 14), [204, 0, 0, 255]);
        // Les coins de l'image restent vides.
        assert_eq!(pixel(&rgba, size, 0, 0)[3], 0);
    }

    #[test]
    fn an_item_with_a_model_gets_an_icon_and_one_without_does_not() {
        let reg = registry();
        let rgba = icon(&reg, reg.lookup("t:pork").unwrap(), 16).unwrap();
        assert!(rgba.chunks(4).any(|p| p[3] == 255), "des pixels dessinés");
        assert!(icon(&reg, reg.lookup("t:bread").unwrap(), 16).is_none());
    }
}
