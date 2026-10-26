//! Worldgen — pipeline pluggable (§4), volontairement simple : un bruit de
//! hauteur → sol/air, de la pierre sous une couche de sol, et des grottes
//! creusées par un bruit 3D. La *forme* figée est le trait
//! [`WorldGenerator`] ; la richesse (biomes, features) viendra en implémentant
//! d'autres générateurs, pas en gonflant celui-ci.
//!
//! Deux invariants du doc s'appliquent ici :
//! - **Déterminisme (§2)** : tout dérive de la seed par hash entier — pas de
//!   RNG à état, pas de dépendance à l'ordre de génération des chunks. Le
//!   même (seed, position) donne toujours le même voxel, sur toute machine.
//! - **Gameplay en mètres (§2)** : hauteur du sol et amplitude du relief sont
//!   exprimées en mètres. `voxels_per_meter` ne fait que convertir à la fin.

use crate::chunk::{Chunk, ChunkPos};
use serde::{Deserialize, Serialize};

use crate::registry::{parse, ContentId, Registry};

/// La forme figée du pipeline : un générateur produit un chunk depuis sa
/// position, de façon pure (pas d'état mutable → parallélisable, déterministe).
pub trait WorldGenerator {
    /// `chunk_size` et `voxels_per_meter` sont des métadonnées du monde (§3.5),
    /// passées par l'appelant : le générateur n'en garde pas de copie qui
    /// pourrait diverger de celle du monde.
    fn generate_chunk(&self, pos: ChunkPos, chunk_size: u32, voxels_per_meter: f32) -> Chunk;
}

/// Épaisseur de la couche de `ground` au-dessus de la pierre, en mètres.
const SOIL_DEPTH_M: f32 = 1.0;
/// Taille caractéristique des grottes (longueur d'un coude de tunnel), en mètres.
const CAVE_SIZE_M: f32 = 16.0;
/// Demi-largeur de la bande « proche de 0,5 » dans chaque bruit de grotte.
/// Plus large = tunnels plus gros et plus nombreux.
const CAVE_BAND: f32 = 0.06;

/// Générateur : heightmap fBm → sol/air, pierre dessous, grottes.
#[derive(Debug, Clone)]
pub struct HeightmapGenerator {
    pub seed: u64,
    /// Matériaux à poser — injectés depuis le registre, jamais hardcodés.
    pub air: ContentId,
    pub ground: ContentId,
    /// Sous la couche de sol (et autour des grottes).
    pub stone: ContentId,
    /// Altitude moyenne du sol, en **mètres**.
    pub ground_level_m: f32,
    /// Amplitude du relief autour de cette moyenne, en **mètres**.
    pub amplitude_m: f32,
    /// Largeur caractéristique des collines, en **mètres**.
    pub feature_size_m: f32,
}

impl HeightmapGenerator {
    /// Hauteur du sol en **mètres** à la position monde (x, z) en mètres.
    /// Publique pour que les tests (et plus tard le spawn du joueur)
    /// interrogent le terrain sans générer de chunk.
    pub fn height_m(&self, x_m: f32, z_m: f32) -> f32 {
        let n = fbm2(self.seed, x_m / self.feature_size_m, z_m / self.feature_size_m, 4);
        // fbm2 rend ~[0, 1] → recentré sur [-1, 1] puis mis à l'échelle.
        self.ground_level_m + (n * 2.0 - 1.0) * self.amplitude_m
    }

    /// Le point (mètres) est-il dans un tunnel ?
    ///
    /// Un bruit 3D vaut ~0,5 sur une **surface** ondulée de l'espace ; la
    /// bande `|n − 0,5| < CAVE_BAND` est une plaque épaisse autour de cette
    /// surface. Deux bruits indépendants donnent deux plaques, et leur
    /// **intersection** est un tube qui serpente : un tunnel. Un seul bruit
    /// seuillé (`n > s`) donnerait plutôt des bulles isolées.
    pub fn is_cave_m(&self, x_m: f32, y_m: f32, z_m: f32) -> bool {
        let (x, y, z) = (x_m / CAVE_SIZE_M, y_m / CAVE_SIZE_M, z_m / CAVE_SIZE_M);
        let a = fbm3(hash64(self.seed ^ 0xCA7E_0001), x, y, z, 2);
        if (a - 0.5).abs() >= CAVE_BAND {
            return false; // le second bruit n'est calculé que si besoin
        }
        let b = fbm3(hash64(self.seed ^ 0xCA7E_0002), x, y, z, 2);
        (b - 0.5).abs() < CAVE_BAND
    }
}

impl WorldGenerator for HeightmapGenerator {
    fn generate_chunk(&self, pos: ChunkPos, chunk_size: u32, voxels_per_meter: f32) -> Chunk {
        let mut chunk = Chunk::filled(chunk_size, self.air);
        let size = chunk_size as i64;
        let vpm = voxels_per_meter;

        for z in 0..chunk_size {
            for x in 0..chunk_size {
                // Coordonnées monde du centre de la colonne, en mètres.
                let wx = (pos.x as i64 * size + x as i64) as f32 / vpm;
                let wz = (pos.z as i64 * size + z as i64) as f32 / vpm;
                let h = self.height_m(wx, wz);
                // La colonne est solide sous la surface. On compare le centre
                // du voxel (y + 0.5) pour un arrondi symétrique.
                for y in 0..chunk_size {
                    let wy = ((pos.y as i64 * size + y as i64) as f32 + 0.5) / vpm;
                    if wy >= h || self.is_cave_m(wx, wy, wz) {
                        continue; // ciel ou grotte : reste de l'air
                    }
                    let material = if wy < h - SOIL_DEPTH_M { self.stone } else { self.ground };
                    chunk.set(x, y, z, material);
                }
            }
        }
        chunk
    }
}

// --- Worldgen en donnée (#27) --------------------------------------------
//
// Les réglages viennent de `assets/content/worldgen.ron` (puis de la save,
// §3.10) : relief, biomes, minerais, features. Le moteur fournit les étapes
// (relief → surface du biome → minerais → features), le contenu choisit les
// blocs et les nombres — aucun bloc nommé en Rust (§2).
//
// Les structures sont génériques sur `Id` : `String` dans le fichier
// (identifiers), `ContentId` une fois résolues contre le registre.

/// Un biome : la surface et le dessous d'une zone de climat.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Biome<Id> {
    pub name: String,
    /// Plage de climat [min, max) — bruit 2D dans ~[0, 1].
    pub climate: (f32, f32),
    /// Le voxel du dessus.
    pub surface: Id,
    /// Sous la surface, jusqu'à `depth` mètres.
    pub under: Id,
    pub depth: f32,
}

/// Un minerai : remplace la pierre là où son bruit 3D dépasse `threshold`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Ore<Id> {
    pub block: Id,
    /// Profondeur sous la surface, en mètres [min, max).
    pub depth: (f32, f32),
    /// Taille d'un amas, en mètres.
    pub size: f32,
    /// Plus haut = plus rare (0,7 – 0,85 utile).
    pub threshold: f32,
}

/// Une feature posée sur la surface : une colonne, surmontée en option
/// d'une boule. Arbre = tronc + feuillage, cactus = colonne seule, rocher
/// = hauteur 0 + boule.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Feature<Id> {
    pub block: Id,
    /// Hauteur de la colonne en mètres, tirée dans [min, max].
    pub height: (f32, f32),
    #[serde(default)]
    pub top: Option<Top<Id>>,
    /// Probabilité par m² de surface.
    pub density: f32,
    /// Biomes où elle pousse (par nom) ; vide : partout.
    #[serde(default)]
    pub biomes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Top<Id> {
    pub block: Id,
    /// Rayon de la boule, en mètres.
    pub radius: f32,
}

/// Mer (#28) : `block` remplit l'air sous l'altitude `level` (mètres), hors
/// grottes. Des sources : immobiles tant que rien ne les touche.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Sea<Id> {
    pub level: f32,
    pub block: Id,
}

/// Le fichier `worldgen.ron`.
#[derive(Debug, Clone, Deserialize, Serialize)]
struct Settings {
    air: String,
    stone: String,
    /// Relief, en mètres (voir [`HeightmapGenerator`]).
    ground_level: f32,
    amplitude: f32,
    hill_size: f32,
    /// Largeur caractéristique d'une zone de biome, en mètres.
    climate_size: f32,
    biomes: Vec<Biome<String>>,
    #[serde(default)]
    ores: Vec<Ore<String>>,
    #[serde(default)]
    features: Vec<Feature<String>>,
    #[serde(default)]
    sea: Option<Sea<String>>,
}

/// Le générateur du jeu : relief et grottes de [`HeightmapGenerator`],
/// puis biomes, minerais et features lus en donnée.
#[derive(Debug, Clone)]
pub struct Worldgen {
    pub terrain: HeightmapGenerator,
    climate_size_m: f32,
    biomes: Vec<Biome<ContentId>>,
    ores: Vec<Ore<ContentId>>,
    features: Vec<Feature<ContentId>>,
    sea: Option<Sea<ContentId>>,
}

/// Un seul biome (le sol de [`HeightmapGenerator`]), ni minerai ni
/// feature : le relief seul (sert aux tests).
impl From<HeightmapGenerator> for Worldgen {
    fn from(terrain: HeightmapGenerator) -> Self {
        let g = terrain.ground;
        let biome = Biome { name: String::new(), climate: (0.0, 1.0), surface: g, under: g, depth: SOIL_DEPTH_M };
        Self { terrain, climate_size_m: 1.0, biomes: vec![biome], ores: Vec::new(), features: Vec::new(), sea: None }
    }
}

impl Worldgen {
    /// Lit `worldgen.ron` et résout ses identifiers contre le registre.
    pub fn from_ron(text: &str, seed: u64, registry: &Registry) -> Result<Self, String> {
        let s: Settings = parse(text).map_err(|e| e.to_string())?;
        if s.biomes.is_empty() {
            return Err("worldgen : au moins un biome".into());
        }
        let id = |name: &String| {
            let id = registry.lookup(name).ok_or_else(|| format!("worldgen cite {name}, absent du registre"))?;
            // §3.6 : un minuteur par bloc posé, le coût suivrait la taille du
            // monde exploré.
            if crate::rules::delays(registry, id).next().is_some() {
                return Err(format!("worldgen pose {name}, un bloc à minuteur (After) : refusé"));
            }
            Ok(id)
        };
        let biomes = s
            .biomes
            .into_iter()
            .map(|b| Ok(Biome { surface: id(&b.surface)?, under: id(&b.under)?, name: b.name, climate: b.climate, depth: b.depth }))
            .collect::<Result<Vec<_>, String>>()?;
        let ores = s
            .ores
            .into_iter()
            .map(|o| Ok(Ore { block: id(&o.block)?, depth: o.depth, size: o.size, threshold: o.threshold }))
            .collect::<Result<Vec<_>, String>>()?;
        let features = s
            .features
            .into_iter()
            .map(|f| {
                if let Some(name) = f.biomes.iter().find(|n| !biomes.iter().any(|b| &b.name == *n)) {
                    return Err(format!("worldgen : biome {name} inconnu"));
                }
                let top = match f.top {
                    Some(t) => Some(Top { block: id(&t.block)?, radius: t.radius }),
                    None => None,
                };
                Ok(Feature { block: id(&f.block)?, height: f.height, top, density: f.density, biomes: f.biomes })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let sea = match s.sea {
            Some(sea) => Some(Sea { level: sea.level, block: id(&sea.block)? }),
            None => None,
        };
        let (air, stone) = (id(&s.air)?, id(&s.stone)?);
        let terrain = HeightmapGenerator {
            seed,
            air,
            ground: stone,
            stone,
            ground_level_m: s.ground_level,
            amplitude_m: s.amplitude,
            feature_size_m: s.hill_size,
        };
        Ok(Self { terrain, climate_size_m: s.climate_size, biomes, ores, features, sea })
    }

    /// Hauteur maximale d'une feature au-dessus du sol, en mètres.
    fn reach_m(&self) -> f32 {
        self.features.iter().map(|f| f.height.1 + f.top.as_ref().map_or(0.0, |t| t.radius)).fold(0.0, f32::max)
    }

    /// Altitude en mètres au-dessus de laquelle ce générateur ne pose rien :
    /// un chunk plus haut, s'il n'est pas dans la save, est vide sans le
    /// générer (le ciel de la lumière, §3.8).
    pub fn top_m(&self) -> f32 {
        let t = &self.terrain;
        let land = t.ground_level_m + t.amplitude_m + self.reach_m();
        self.sea.as_ref().map_or(land, |sea| land.max(sea.level))
    }

    /// Le biome de la colonne (x, z), en mètres. Hors de toute plage : le
    /// dernier.
    pub fn biome_m(&self, x_m: f32, z_m: f32) -> &Biome<ContentId> {
        let last = self.biomes.last().expect("au moins un biome");
        let seed = hash64(self.terrain.seed ^ 0xC11_4A7E);
        let c = fbm2(seed, x_m / self.climate_size_m, z_m / self.climate_size_m, 2);
        self.biomes.iter().find(|b| b.climate.0 <= c && c < b.climate.1).unwrap_or(last)
    }

    /// Le minerai à ce point (mètres), `depth_m` sous la surface.
    fn ore_m(&self, x_m: f32, y_m: f32, z_m: f32, depth_m: f32) -> Option<ContentId> {
        self.ores.iter().enumerate().find_map(|(i, o)| {
            let inside = o.depth.0 <= depth_m && depth_m < o.depth.1;
            let seed = hash64(self.terrain.seed ^ 0x0BE_0000 ^ i as u64);
            (inside && value_noise3(seed, x_m / o.size, y_m / o.size, z_m / o.size) > o.threshold).then_some(o.block)
        })
    }

    /// Pose les features dont un voxel tombe dans ce chunk. Chaque chunk
    /// examine aussi les colonnes d'une marge autour de lui : un arbre
    /// planté chez le voisin peut déborder ici. Le tirage ne dépend que de
    /// la colonne et l'ordre de parcours est celui du monde : tous les
    /// chunks voient les mêmes arbres, quel que soit l'ordre de chargement.
    fn place_features(&self, chunk: &mut Chunk, pos: ChunkPos, vpm: f32) {
        if self.features.is_empty() {
            return;
        }
        let size = chunk.size() as i64;
        let (ox, oy, oz) = (pos.x as i64 * size, pos.y as i64 * size, pos.z as i64 * size);
        let reach_m = self.reach_m();
        let (lowest, highest) = (
            self.terrain.ground_level_m - self.terrain.amplitude_m - reach_m,
            self.terrain.ground_level_m + self.terrain.amplitude_m + reach_m,
        );
        if (oy + size) as f32 / vpm < lowest || oy as f32 / vpm > highest {
            return; // trop haut ou trop bas pour qu'une feature y arrive
        }
        let margin = self
            .features
            .iter()
            .map(|f| f.top.as_ref().map_or(0.0, |t| t.radius))
            .fold(0.0, f32::max);
        let margin = (margin * vpm).ceil() as i64;
        let tops: Vec<ContentId> = self.features.iter().filter_map(|f| f.top.as_ref().map(|t| t.block)).collect();
        let air = self.terrain.air;
        // Écrit (x, y, z) monde s'il est dans ce chunk et que la place est
        // libre (air ; le feuillage cède aussi la place à un tronc).
        let mut put = |x: i64, y: i64, z: i64, block: ContentId, over_tops: bool| {
            let (lx, ly, lz) = (x - ox, y - oy, z - oz);
            if [lx, ly, lz].iter().all(|&c| (0..size).contains(&c)) {
                let (lx, ly, lz) = (lx as u32, ly as u32, lz as u32);
                let here = chunk.get(lx, ly, lz);
                if here == air || (over_tops && tops.contains(&here)) {
                    chunk.set(lx, ly, lz, block);
                }
            }
        };
        let seed = hash64(self.terrain.seed ^ 0xFEA7_0000);
        let rand = |vx: i64, vz: i64, n: u64| {
            let h = hash64(seed ^ (vx as u64).wrapping_mul(0x8DA6_B343) ^ (vz as u64).wrapping_mul(0xD816_3841_5C58_35CB) ^ n);
            (h >> 40) as f32 / (1u64 << 24) as f32
        };
        for vz in oz - margin..oz + size + margin {
            for vx in ox - margin..ox + size + margin {
                let (wx, wz) = (vx as f32 / vpm, vz as f32 / vpm);
                let biome = self.biome_m(wx, wz);
                let chosen = self.features.iter().enumerate().find(|(i, f)| {
                    (f.biomes.is_empty() || f.biomes.contains(&biome.name))
                        && rand(vx, vz, *i as u64) < f.density / (vpm * vpm)
                });
                let Some((i, feature)) = chosen else { continue };
                // Le voxel solide le plus haut : son centre est sous la surface.
                let h = self.terrain.height_m(wx, wz);
                if self.sea.as_ref().is_some_and(|sea| h < sea.level) {
                    continue; // sous l'eau : le tronc dépasserait de la mer, sans pied
                }
                let ground = (h * vpm - 0.5).ceil() as i64 - 1;
                if self.terrain.is_cave_m(wx, (ground as f32 + 0.5) / vpm, wz) {
                    continue; // pas d'arbre au-dessus d'un trou
                }
                let t = rand(vx, vz, 1000 + i as u64);
                let height = ((feature.height.0 + (feature.height.1 - feature.height.0) * t) * vpm).round() as i64;
                for k in 1..=height {
                    put(vx, ground + k, vz, feature.block, true);
                }
                if let Some(top) = &feature.top {
                    let r = top.radius * vpm;
                    let ri = r.ceil() as i64;
                    let cy = ground + height;
                    for dy in -ri..=ri {
                        for dz in -ri..=ri {
                            for dx in -ri..=ri {
                                if ((dx * dx + dy * dy + dz * dz) as f32) <= r * r + 0.5 {
                                    put(vx + dx, cy + dy, vz + dz, top.block, false);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

impl WorldGenerator for Worldgen {
    fn generate_chunk(&self, pos: ChunkPos, chunk_size: u32, voxels_per_meter: f32) -> Chunk {
        let mut chunk = Chunk::filled(chunk_size, self.terrain.air);
        let size = chunk_size as i64;
        let vpm = voxels_per_meter;
        for z in 0..chunk_size {
            for x in 0..chunk_size {
                let wx = (pos.x as i64 * size + x as i64) as f32 / vpm;
                let wz = (pos.z as i64 * size + z as i64) as f32 / vpm;
                let h = self.terrain.height_m(wx, wz);
                let biome = self.biome_m(wx, wz);
                for y in 0..chunk_size {
                    let wy = ((pos.y as i64 * size + y as i64) as f32 + 0.5) / vpm;
                    if wy >= h {
                        if let Some(sea) = &self.sea
                            && wy < sea.level
                        {
                            chunk.set(x, y, z, sea.block);
                        }
                        continue;
                    }
                    if self.terrain.is_cave_m(wx, wy, wz) {
                        continue;
                    }
                    // Profondeur sous la surface du centre du voxel.
                    let depth = h - wy;
                    let material = if depth < 1.0 / vpm {
                        biome.surface
                    } else if depth < biome.depth {
                        biome.under
                    } else {
                        self.ore_m(wx, wy, wz, depth).unwrap_or(self.terrain.stone)
                    };
                    chunk.set(x, y, z, material);
                }
            }
        }
        self.place_features(&mut chunk, pos, vpm);
        chunk
    }
}

// --- Bruit maison ---------------------------------------------------------
//
// Value noise 2D : un hash entier donne une valeur stable à chaque coin de
// cellule de la grille, interpolée en douceur entre les coins. Le fBm
// (fractional Brownian motion) superpose plusieurs octaves — chacune deux
// fois plus fine et deux fois plus faible — pour passer de « collines
// molles » à un relief avec du détail.
//
// Pourquoi maison plutôt qu'une crate ? Déterminisme garanti (hash entier,
// aucune table statique, aucun état), zéro dépendance, et c'est un des
// concepts qu'on veut comprendre en le construisant.

/// Hash 64-bit type SplitMix64 : avalanche forte, réversible, sans état.
/// C'est notre unique source d'« aléatoire » — tout le worldgen en dérive.
pub(crate) fn hash64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Valeur stable en [0, 1) au coin de grille (ix, iz) pour cette seed.
fn lattice(seed: u64, ix: i64, iz: i64) -> f32 {
    // Mélange coordonnées et seed en un seul u64 à hasher. Les multiplications
    // par de grands premiers évitent que (1,2) et (2,1) donnent le même hash.
    let h = hash64(
        seed ^ (ix as u64).wrapping_mul(0x8DA6_B343)
            ^ (iz as u64).wrapping_mul(0xD816_3841_5C58_35CB),
    );
    // 24 bits de poids fort → f32 uniforme dans [0, 1).
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// Interpolation smoothstep : dérivée nulle aux bornes, pas de cassure
/// visible aux frontières de cellules (contrairement au lerp brut).
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Value noise 2D, ~[0, 1].
fn value_noise2(seed: u64, x: f32, z: f32) -> f32 {
    let ix = x.floor() as i64;
    let iz = z.floor() as i64;
    let fx = x - ix as f32;
    let fz = z - iz as f32;

    let v00 = lattice(seed, ix, iz);
    let v10 = lattice(seed, ix + 1, iz);
    let v01 = lattice(seed, ix, iz + 1);
    let v11 = lattice(seed, ix + 1, iz + 1);

    let sx = smooth(fx);
    let sz = smooth(fz);
    let a = v00 + (v10 - v00) * sx;
    let b = v01 + (v11 - v01) * sx;
    a + (b - a) * sz
}

/// Valeur stable en [0, 1) au coin de grille 3D (ix, iy, iz).
fn lattice3(seed: u64, ix: i64, iy: i64, iz: i64) -> f32 {
    lattice(seed ^ (iy as u64).wrapping_mul(0x9FB2_1C65_1E98_DF25), ix, iz)
}

/// Value noise 3D, ~[0, 1] : le 2D avec un axe de plus — 8 coins au lieu
/// de 4, interpolés en x, puis y, puis z.
fn value_noise3(seed: u64, x: f32, y: f32, z: f32) -> f32 {
    let (ix, iy, iz) = (x.floor() as i64, y.floor() as i64, z.floor() as i64);
    let (sx, sy, sz) = (
        smooth(x - ix as f32),
        smooth(y - iy as f32),
        smooth(z - iz as f32),
    );
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let plane = |dz: i64| {
        let row = |dy: i64| {
            lerp(
                lattice3(seed, ix, iy + dy, iz + dz),
                lattice3(seed, ix + 1, iy + dy, iz + dz),
                sx,
            )
        };
        lerp(row(0), row(1), sy)
    };
    lerp(plane(0), plane(1), sz)
}

/// fBm 3D — même recette que [`fbm2`].
fn fbm3(seed: u64, x: f32, y: f32, z: f32, octaves: u32) -> f32 {
    let (mut total, mut amplitude, mut frequency, mut max) = (0.0, 1.0, 1.0, 0.0);
    for octave in 0..octaves {
        let s = hash64(seed ^ octave as u64);
        total += value_noise3(s, x * frequency, y * frequency, z * frequency) * amplitude;
        max += amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
    }
    total / max
}

/// fBm : superposition d'octaves de value noise, normalisée vers ~[0, 1].
fn fbm2(seed: u64, x: f32, z: f32, octaves: u32) -> f32 {
    let mut total = 0.0;
    let mut amplitude = 1.0;
    let mut frequency = 1.0;
    let mut max = 0.0;
    for octave in 0..octaves {
        // Une seed dérivée par octave, sinon les octaves se ressemblent.
        let s = hash64(seed ^ octave as u64);
        total += value_noise2(s, x * frequency, z * frequency) * amplitude;
        max += amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
    }
    total / max
}

#[cfg(test)]
mod tests {
    use super::*;

    const AIR: ContentId = ContentId(0);
    const GROUND: ContentId = ContentId(1);
    const STONE: ContentId = ContentId(2);

    fn generator(seed: u64) -> HeightmapGenerator {
        HeightmapGenerator {
            seed,
            air: AIR,
            ground: GROUND,
            stone: STONE,
            ground_level_m: 16.0,
            amplitude_m: 6.0,
            feature_size_m: 24.0,
        }
    }

    fn chunks_equal(a: &Chunk, b: &Chunk) -> bool {
        let s = a.size();
        (0..s).all(|z| (0..s).all(|y| (0..s).all(|x| a.get(x, y, z) == b.get(x, y, z))))
    }

    #[test]
    fn same_seed_same_chunk() {
        let pos = ChunkPos { x: 3, y: 0, z: -2 };
        let a = generator(42).generate_chunk(pos, 16, 1.0);
        let b = generator(42).generate_chunk(pos, 16, 1.0);
        assert!(chunks_equal(&a, &b));
    }

    #[test]
    fn different_seeds_differ() {
        let pos = ChunkPos { x: 0, y: 0, z: 0 };
        let a = generator(1).generate_chunk(pos, 16, 1.0);
        let b = generator(2).generate_chunk(pos, 16, 1.0);
        assert!(!chunks_equal(&a, &b));
    }

    #[test]
    fn underground_is_stone_with_caves_and_sky_is_air() {
        let generator = generator(42);
        // ground_level 16 m ± 6 m → y ∈ [10, 22]. Chunks y=-1 (y monde
        // [-16, 0)) : sous terre. Chunk y=2 (y monde [32, 48)) : tout ciel.
        let sky = generator.generate_chunk(ChunkPos { x: 0, y: 2, z: 0 }, 16, 1.0);
        assert!(chunks_equal(&sky, &Chunk::filled(16, AIR)));

        let (mut stone, mut air) = (0, 0);
        for x in 0..4 {
            let deep = generator.generate_chunk(ChunkPos { x, y: -1, z: 0 }, 16, 1.0);
            for (zz, yy, xx) in (0..16).flat_map(|z| (0..16).flat_map(move |y| (0..16).map(move |x| (z, y, x)))) {
                match deep.get(xx, yy, zz) {
                    STONE => stone += 1,
                    AIR => air += 1,
                    other => panic!("sous terre : ni pierre ni grotte ({other:?})"),
                }
            }
        }
        // Des grottes, mais la roche domine.
        assert!(air > 0, "aucune grotte");
        assert!(air * 5 < stone, "trop de vide : {air} air / {stone} pierre");
    }

    #[test]
    fn surface_is_continuous_across_chunk_borders() {
        // La colonne monde (x=16, z=0) est la première du chunk x=1. Sa
        // surface doit coller à celle de (x=15) — dernière du chunk x=0 —
        // à un voxel près, sinon le bruit est discontinu aux frontières.
        let generator = generator(42);
        let left = generator.generate_chunk(ChunkPos { x: 0, y: 0, z: 0 }, 16, 1.0);
        let right = generator.generate_chunk(ChunkPos { x: 1, y: 0, z: 0 }, 16, 1.0);

        let surface = |chunk: &Chunk, x: u32| -> i32 {
            (0..16).rev().find(|&y| chunk.get(x, y, 0) == GROUND).map_or(-1, |y| y as i32)
        };
        let delta = (surface(&left, 15) - surface(&right, 0)).abs();
        assert!(delta <= 1, "surface discontinue au bord de chunk: Δ={delta}");
    }

    #[test]
    fn chunks_match_height_and_caves_in_world_coordinates() {
        // Chaque voxel doit valoir ce que disent height_m et is_cave_m à sa
        // position MONDE. Chunks hors origine, y négatif compris : attrape
        // un calcul fait en coordonnées locales au lieu de monde — qui
        // casserait les grottes et le relief aux frontières de chunk.
        let generator = generator(7);
        for pos in [ChunkPos { x: 2, y: 0, z: -3 }, ChunkPos { x: 2, y: -1, z: -3 }] {
            let chunk = generator.generate_chunk(pos, 16, 1.0);
            for (x, y, z) in (0..16u32).flat_map(|x| (0..16u32).flat_map(move |y| (0..16u32).map(move |z| (x, y, z)))) {
                let wx = (pos.x * 16 + x as i32) as f32;
                let wy = (pos.y * 16 + y as i32) as f32 + 0.5;
                let wz = (pos.z * 16 + z as i32) as f32;
                let h = generator.height_m(wx, wz);
                let expected = if wy >= h || generator.is_cave_m(wx, wy, wz) {
                    AIR
                } else if wy < h - SOIL_DEPTH_M {
                    STONE
                } else {
                    GROUND
                };
                assert_eq!(chunk.get(x, y, z), expected, "{pos:?} ({x},{y},{z}), h={h}");
            }
        }
    }

    fn shipped() -> (Worldgen, Registry) {
        let registry = Registry::from_ron(include_str!("../../../assets/content/core.ron")).unwrap();
        let worldgen = Worldgen::from_ron(include_str!("../../../assets/content/worldgen.ron"), 42, &registry).unwrap();
        (worldgen, registry)
    }

    #[test]
    fn small_chunks_match_one_big_chunk() {
        // Un chunk 32³ et les huit 16³ qui le couvrent doivent être
        // identiques voxel par voxel : arbres qui débordent, minerais et
        // biomes ne dépendent pas du découpage en chunks.
        let (worldgen, _) = shipped();
        for (bx, bz) in [(0, 0), (5, -3), (-7, 11)] {
            let big = worldgen.generate_chunk(ChunkPos { x: bx, y: 0, z: bz }, 32, 1.0);
            for (dx, dy, dz) in (0..2).flat_map(|x| (0..2).flat_map(move |y| (0..2).map(move |z| (x, y, z)))) {
                let pos = ChunkPos { x: bx * 2 + dx, y: dy, z: bz * 2 + dz };
                let small = worldgen.generate_chunk(pos, 16, 1.0);
                for (x, y, z) in (0..16u32).flat_map(|x| (0..16u32).flat_map(move |y| (0..16u32).map(move |z| (x, y, z)))) {
                    let (gx, gy, gz) = (dx as u32 * 16 + x, dy as u32 * 16 + y, dz as u32 * 16 + z);
                    assert_eq!(small.get(x, y, z), big.get(gx, gy, gz), "{pos:?} ({x},{y},{z})");
                }
            }
        }
    }

    #[test]
    fn shipped_worldgen_has_its_biomes_ores_and_trees() {
        let (worldgen, registry) = shipped();
        let mut seen = std::collections::HashSet::new();
        // Une bande de 32 × 4 chunks : assez large pour croiser les biomes.
        for x in -16..16 {
            for z in -2..2 {
                for y in -2..2 {
                    let chunk = worldgen.generate_chunk(ChunkPos { x, y, z }, 16, 1.0);
                    for (a, b, c) in (0..16u32).flat_map(|a| (0..16u32).flat_map(move |b| (0..16u32).map(move |c| (a, b, c)))) {
                        seen.insert(chunk.get(a, b, c));
                    }
                }
            }
        }
        for name in ["core:sand", "core:grass", "core:dirt", "core:coal_ore", "core:iron_ore", "core:log", "core:leaves", "core:cactus"] {
            assert!(seen.contains(&registry.lookup(name).unwrap()), "{name} jamais généré");
        }
    }

    #[test]
    fn unknown_identifier_in_worldgen_is_refused() {
        let (_, registry) = shipped();
        let text = include_str!("../../../assets/content/worldgen.ron").replace("core:coal_ore", "core:gold_ore");
        assert!(Worldgen::from_ron(&text, 42, &registry).unwrap_err().contains("core:gold_ore"));
    }

    #[test]
    fn the_sea_fills_the_lows_and_grows_no_tree() {
        let (_, registry) = shipped();
        let text = include_str!("../../../assets/content/worldgen.ron");
        let worldgen = Worldgen::from_ron(text, 42, &registry).unwrap();
        let sea = worldgen.sea.clone().expect("le worldgen du jeu a une mer");
        let (water, log) = (registry.lookup("core:water").unwrap(), registry.lookup("core:log").unwrap());
        let mut wet = 0;
        for (cx, cz) in (-3..3).flat_map(|x| (-3..3).map(move |z| (x, z))) {
            let pos = ChunkPos { x: cx, y: 0, z: cz };
            let chunk = worldgen.generate_chunk(pos, 32, 1.0);
            for (x, z) in (0..32).flat_map(|x| (0..32).map(move |z| (x, z))) {
                let (wx, wz) = ((cx * 32 + x as i32) as f32, (cz * 32 + z as i32) as f32);
                let h = worldgen.terrain.height_m(wx, wz);
                let column: Vec<_> = (0..32).map(|y| chunk.get(x, y, z)).collect();
                if h + 1.0 < sea.level {
                    wet += 1;
                    assert_eq!(column[(sea.level - 0.5) as usize], water, "({wx}, {wz})");
                    assert!(!column.contains(&log), "arbre dans la mer en ({wx}, {wz})");
                }
                assert_ne!(column[sea.level as usize + 1], water, "rien au-dessus du niveau");
            }
        }
        assert!(wet > 0, "aucune colonne sous la mer : le test ne prouve rien");
    }

    #[test]
    fn worldgen_refuses_blocks_with_a_timer() {
        let (_, registry) = shipped();
        let text = include_str!("../../../assets/content/worldgen.ron").replace("core:coal_ore", "core:wheat_seeds");
        assert!(Worldgen::from_ron(&text, 42, &registry).unwrap_err().contains("After"));
    }
}
