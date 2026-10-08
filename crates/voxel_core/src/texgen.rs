//! Textures calculées (#44, §3.1) : une entrée déclare un motif, une
//! couleur, un niveau de bruit et une graine ; le moteur en tire les
//! pixels. C'est ce qu'écrit une IA (quelques valeurs, pas 256 pixels), et
//! un humain peut s'en servir aussi.
//!
//! Déterministe comme le worldgen : même graine → mêmes pixels, sur toute
//! machine (hash entier, pas de générateur aléatoire de la plateforme).

use serde::{Deserialize, Serialize};

use crate::registry::Texture;
use crate::worldgen::hash64;

/// Une texture calculée, citée par son `name` dans [`crate::registry::Faces`].
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct GeneratedTexture {
    pub name: String,
    pub pattern: Pattern,
    /// Couleur de base, RGB dans [0, 1].
    pub color: [f32; 3],
    /// Variation de luminosité par pixel, dans [0, 1] (0 : uni).
    #[serde(default)]
    pub noise: f32,
    #[serde(default)]
    pub seed: u32,
}

/// Les motifs connus du moteur. Append-only comme le vocabulaire des
/// règles (§3.6) : une save cite un motif par son nom.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
pub enum Pattern {
    /// Couleur + bruit seuls.
    Plain,
    /// Lignes sombres qui serpentent (pierre, minerai).
    Cracks,
    /// Bandes horizontales avec joints (bois).
    Planks,
    /// Taches claires et sombres (terre, sable, lave).
    Spots,
    /// Bord sombre et liseré clair (caisse, lampe).
    Frame,
}

/// Calcule les pixels de `def` en `size` × `size`. Erreur si une valeur
/// sort de ses bornes.
pub fn paint(def: &GeneratedTexture, size: u32) -> Result<Texture, String> {
    if def.color.iter().any(|c| !(0.0..=1.0).contains(c)) {
        return Err(format!("couleur {:?} hors de [0, 1]", def.color));
    }
    if !(0.0..=1.0).contains(&def.noise) {
        return Err(format!("bruit {} hors de [0, 1]", def.noise));
    }
    let s = size as usize;
    let seed = u64::from(def.seed);
    // Un nombre stable dans [0, 1) par (graine, n).
    let rand = |n: u64| (hash64(seed.wrapping_mul(0x1_0000_01B3) ^ n) >> 40) as f32 / (1u64 << 24) as f32;
    // Luminosité par pixel : 1 = la couleur de base.
    let mut light: Vec<f32> = (0..s * s).map(|i| 1.0 + def.noise * (rand(i as u64) - 0.5)).collect();
    // Épaisseur d'un trait : 1 px pour 16 px de côté.
    let w = (s / 16).max(1);
    let mut shade = |x: usize, y: usize, k: f32| light[(y % s) * s + x % s] *= k;
    match def.pattern {
        Pattern::Plain => {}
        Pattern::Cracks => {
            // Trois marches au hasard, qui repassent de l'autre côté du bord
            // pour que la texture se répète sans couture.
            for c in 0..3u64 {
                let (mut x, mut y) = ((rand(1000 + c) * s as f32) as usize, (rand(2000 + c) * s as f32) as usize);
                for step in 0..s as u64 {
                    shade(x, y, 0.6);
                    let r = rand(3000 + c * 1000 + step);
                    x = (x + s + [0, 1, s - 1][(r * 3.0) as usize]) % s;
                    y = (y + 1) % s;
                }
            }
        }
        Pattern::Planks => {
            let band = (s / 4).max(1);
            for y in 0..s {
                let b = (y / band) as u64;
                let tone = 0.9 + 0.2 * rand(4000 + b);
                let seam = (rand(5000 + b) * s as f32) as usize;
                for x in 0..s {
                    let edge = y % band < w || (x / w == seam / w);
                    shade(x, y, if edge { 0.65 } else { tone });
                }
            }
        }
        Pattern::Spots => {
            for c in 0..s as u64 {
                let (cx, cy) = ((rand(6000 + c) * s as f32) as usize, (rand(7000 + c) * s as f32) as usize);
                let k = if rand(8000 + c) < 0.5 { 0.65 } else { 1.35 };
                for dy in 0..w + 1 {
                    for dx in 0..w + 1 {
                        shade(cx + dx, cy + dy, k);
                    }
                }
            }
        }
        Pattern::Frame => {
            for y in 0..s {
                for x in 0..s {
                    let d = x.min(y).min(s - 1 - x).min(s - 1 - y);
                    if d < w {
                        shade(x, y, 0.6);
                    } else if d < 2 * w {
                        shade(x, y, 1.2);
                    }
                }
            }
        }
    }
    let rgba = light
        .iter()
        .flat_map(|&l| {
            let [r, g, b] = def.color.map(|c| ((c * l).clamp(0.0, 1.0) * 255.0).round() as u8);
            [r, g, b, 255]
        })
        .collect();
    Ok(Texture { name: def.name.clone(), size, rgba })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(pattern: Pattern, seed: u32) -> GeneratedTexture {
        GeneratedTexture { name: "t".into(), pattern, color: [0.5, 0.3, 0.2], noise: 0.2, seed }
    }

    #[test]
    fn same_seed_same_pixels() {
        for pattern in [Pattern::Plain, Pattern::Cracks, Pattern::Planks, Pattern::Spots, Pattern::Frame] {
            let a = paint(&def(pattern, 7), 16).unwrap();
            assert_eq!(a.rgba.len(), 16 * 16 * 4);
            assert_eq!(a, paint(&def(pattern, 7), 16).unwrap());
            assert_ne!(a, paint(&def(pattern, 8), 16).unwrap(), "{pattern:?}");
        }
    }

    #[test]
    fn out_of_range_values_are_refused() {
        let mut d = def(Pattern::Plain, 0);
        d.color = [1.5, 0.0, 0.0];
        assert!(paint(&d, 16).is_err());
        let mut d = def(Pattern::Plain, 0);
        d.noise = -0.1;
        assert!(paint(&d, 16).is_err());
    }
}
