//! Persistance disque — §3.10 (structure logique figée, backend mou).
//!
//! Un dossier par monde :
//! - `world.ron` : métadonnées (version de format, seed, résolution, taille
//!   de chunk) — ce qui est gelé à la création (§3.5) ;
//! - `registry.ron` : le registre complet, pixels compris (§3.1) ;
//! - `chunks/x_y_z.bin` : un fichier par chunk **modifié**, block-entities
//!   et minuteurs compris. Un chunk absent se régénère depuis la seed ;
//! - `clock.ron` : le numéro du tick fixe courant (horloge des minuteurs) ;
//! - `player.ron` : position, regard, inventaire ;
//! - `items.ron` : les items au sol (entrée + position des pieds, en mètres) ;
//! - `mobs.ron` : les mobs (id, entrée, pieds en mètres, cap, apparu seul).
//!
//! Écritures atomiques (fichier temporaire puis renommage) : un arrêt en
//! pleine écriture laisse l'ancienne version, jamais un fichier tronqué.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::chunk::{Chunk, ChunkPos};
use crate::registry::{ContentId, Registry};
use crate::shape::Rotation;
use crate::world::{ChunkState, Timer, VoxelWorld};

/// Version du format de save. À incrémenter à tout changement de format ;
/// une save d'une autre version est refusée.
pub const FORMAT_VERSION: u32 = 1;

/// En-tête d'un fichier de chunk.
const CHUNK_MAGIC: &[u8; 4] = b"VXC1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldMeta {
    pub format: u32,
    pub seed: u64,
    pub voxels_per_meter: f32,
    pub chunk_size: u32,
}

/// Ce que la save garde du joueur.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerSave {
    pub feet: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    /// Piles de l'inventaire, dans l'ordre.
    pub inventory: Vec<(ContentId, u32)>,
    /// Case choisie (`inventory.len()` = main vide).
    pub selected: usize,
}

/// Items au sol : entrée et position des pieds en mètres. La vitesse n'est
/// pas gardée : un item sauvé en pleine chute repart à l'arrêt.
pub type DroppedItems = Vec<(ContentId, [f32; 3])>;

/// Entités posées (#15) : entrée `EntityType`, pieds en mètres, cap en
/// radians (autour de +Y, 0 : regarde vers +Z).
pub type Mobs = Vec<(u64, ContentId, [f32; 3], f32, bool)>;

/// Le dossier d'un monde.
#[derive(Debug, Clone)]
pub struct Save {
    dir: PathBuf,
}

impl Save {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Métadonnées du monde ; `None` s'il n'existe pas encore.
    pub fn read_meta(&self) -> Result<Option<WorldMeta>, String> {
        let Some(meta) = self.read_ron::<WorldMeta>("world.ron")? else { return Ok(None) };
        if meta.format != FORMAT_VERSION {
            return Err(format!("save au format {}, ce moteur lit le format {FORMAT_VERSION}", meta.format));
        }
        Ok(Some(meta))
    }

    pub fn read_registry(&self) -> Result<Registry, String> {
        let path = self.dir.join("registry.ron");
        let text = fs::read_to_string(&path).map_err(|e| format!("{} : {e}", path.display()))?;
        Registry::from_snapshot(&text).map_err(|e| format!("{} : {e}", path.display()))
    }

    /// Écrit métadonnées et registre (création du monde, ou registre fusionné
    /// au chargement).
    pub fn write_world(&self, meta: &WorldMeta, registry: &Registry) -> Result<(), String> {
        self.write_ron("world.ron", meta)?;
        self.write_registry(registry)
    }

    /// Réécrit le registre seul (contenu rechargé en cours de partie).
    pub fn write_registry(&self, registry: &Registry) -> Result<(), String> {
        self.write("registry.ron", registry.to_snapshot().as_bytes())
    }

    /// Copie d'un fichier de contenu du jeu (§3.10 : `worldgen.ron`,
    /// `sky.ron`), telle qu'écrite à la création du monde ; `None` pour un
    /// monde créé avant qu'elle soit sauvée.
    pub fn read_copy(&self, name: &str) -> Result<Option<String>, String> {
        let path = self.dir.join(name);
        match fs::read_to_string(&path) {
            Ok(text) => Ok(Some(text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{} : {e}", path.display())),
        }
    }

    /// Gardé tel quel (commentaires compris) : c'est le fichier du jeu.
    pub fn write_copy(&self, name: &str, text: &str) -> Result<(), String> {
        self.write(name, text.as_bytes())
    }

    pub fn read_player(&self) -> Result<Option<PlayerSave>, String> {
        self.read_ron("player.ron")
    }

    pub fn write_player(&self, player: &PlayerSave) -> Result<(), String> {
        self.write_ron("player.ron", player)
    }

    /// Absent (monde neuf ou sans items) : aucun item.
    pub fn read_items(&self) -> Result<DroppedItems, String> {
        Ok(self.read_ron("items.ron")?.unwrap_or_default())
    }

    pub fn write_items(&self, items: &DroppedItems) -> Result<(), String> {
        self.write_ron("items.ron", items)
    }

    /// Absent : aucune entité.
    pub fn read_mobs(&self) -> Result<Mobs, String> {
        let e = match self.read_ron::<Mobs>("mobs.ron") {
            Ok(mobs) => return Ok(mobs.unwrap_or_default()),
            Err(e) => e,
        };
        // Sauvés avant #60, sans « apparu seul » : tous posés.
        if let Ok(old) = self.read_ron::<Vec<(u64, ContentId, [f32; 3], f32)>>("mobs.ron") {
            return Ok(old.unwrap_or_default().into_iter().map(|(id, c, f, y)| (id, c, f, y, false)).collect());
        }
        // Sauvés avant #32, sans id : numérotés dans l'ordre du fichier.
        self.read_ron::<Vec<(ContentId, [f32; 3], f32)>>("mobs.ron")
            .map_err(|_| e)
            .map(|old| old.unwrap_or_default().into_iter().zip(0..).map(|((c, f, y), id)| (id, c, f, y, false)).collect())
    }

    pub fn write_mobs(&self, mobs: &Mobs) -> Result<(), String> {
        self.write_ron("mobs.ron", mobs)
    }

    /// L'horloge du monde ; `None` pour un monde sauvé avant qu'elle existe
    /// (elle repart de 0, ses chunks n'ont aucun minuteur).
    pub fn read_clock(&self) -> Result<Option<u64>, String> {
        self.read_ron("clock.ron")
    }

    pub fn write_clock(&self, tick: u64) -> Result<(), String> {
        self.write_ron("clock.ron", &tick)
    }

    /// Le chunk `pos` s'il a été sauvé ; `None` s'il est à générer.
    pub fn read_chunk(&self, pos: ChunkPos) -> Result<Option<(Chunk, ChunkState)>, String> {
        let path = self.dir.join(chunk_file(pos));
        match fs::read(&path) {
            Ok(bytes) => decode_chunk(&bytes).map(Some).map_err(|e| format!("{} : {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{} : {e}", path.display())),
        }
    }

    /// Écrit le chunk `pos` du monde (s'il est chargé).
    pub fn write_chunk(&self, world: &VoxelWorld, pos: ChunkPos) -> Result<(), String> {
        let Some(chunk) = world.chunk(pos) else { return Ok(()) };
        self.write_chunk_bytes(pos, &encode_chunk(chunk, &world.chunk_state(pos)))
    }

    /// Écrit un chunk déjà encodé (`encode_chunk`) : l'app encode dans
    /// l'image et écrit le fichier à part (#54).
    pub fn write_chunk_bytes(&self, pos: ChunkPos, bytes: &[u8]) -> Result<(), String> {
        self.write(&chunk_file(pos), bytes)
    }

    fn read_ron<T: for<'de> Deserialize<'de>>(&self, name: &str) -> Result<Option<T>, String> {
        let path = self.dir.join(name);
        match fs::read_to_string(&path) {
            Ok(text) => ron::from_str(&text).map(Some).map_err(|e| format!("{} : {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{} : {e}", path.display())),
        }
    }

    fn write_ron<T: Serialize>(&self, name: &str, value: &T) -> Result<(), String> {
        let text = ron::ser::to_string_pretty(value, ron::ser::PrettyConfig::default())
            .map_err(|e| e.to_string())?;
        self.write(name, text.as_bytes())
    }

    /// Écriture atomique : temporaire puis renommage.
    fn write(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let path = self.dir.join(name);
        let tmp = path.with_extension("tmp");
        let err = |e: std::io::Error| format!("{} : {e}", path.display());
        fs::create_dir_all(path.parent().expect("chemin dans le dossier du monde")).map_err(err)?;
        fs::write(&tmp, bytes).map_err(err)?;
        fs::rename(&tmp, &path).map_err(err)
    }
}

fn chunk_file(pos: ChunkPos) -> String {
    format!("chunks/{}_{}_{}.bin", pos.x, pos.y, pos.z)
}

/// Format binaire d'un chunk, entiers little-endian :
///
/// ```text
/// "VXC1" | taille u32 | n palette u32 | palette : n × ContentId u32
///        | voxels : taille³ × index u16 (layout de Chunk)
///        | n block-entities u32 | par entité : x y z u32, n items u32, items u32…
///        | n minuteurs u32 | par minuteur : x y z u32, échéance u64, règle u32, retard u8
///        | n niveaux u32 | par niveau : x y z u32, niveau u8
///        | n réexamens u32 | par réexamen : x y z u32, échéance u64
///        | n rotations u32 | par rotation : x y z u32, rotation u8
/// ```
///
/// Les sections des minuteurs (#34) et des liquides (#28, niveaux et
/// réexamens) et des rotations (#14) sont optionnelles à la lecture : un chunk sauvé avant elles
/// se relit sans. Pas de `FORMAT_VERSION` à incrémenter, comme pour un
/// fichier optionnel.
///
/// Brut, sans compression : 64 Kio par chunk de 32³, et seuls les chunks
/// modifiés sont écrits.
// ponytail: pas de compression ; un RLE sur les voxels si la save grossit.
pub fn encode_chunk(chunk: &Chunk, state: &ChunkState) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + chunk.voxels().len() * 2);
    let u32_ = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
    out.extend_from_slice(CHUNK_MAGIC);
    u32_(&mut out, chunk.size());
    u32_(&mut out, chunk.palette().len() as u32);
    for id in chunk.palette() {
        u32_(&mut out, id.0);
    }
    for v in chunk.voxels() {
        out.extend_from_slice(&v.to_le_bytes());
    }
    u32_(&mut out, state.entities.len() as u32);
    for (local, items) in &state.entities {
        local.iter().for_each(|&c| u32_(&mut out, c));
        u32_(&mut out, items.len() as u32);
        items.iter().for_each(|id| u32_(&mut out, id.0));
    }
    u32_(&mut out, state.timers.len() as u32);
    for (local, t) in &state.timers {
        local.iter().for_each(|&c| u32_(&mut out, c));
        out.extend_from_slice(&t.due.to_le_bytes());
        u32_(&mut out, t.rule);
        out.push(t.late);
    }
    u32_(&mut out, state.levels.len() as u32);
    for (local, level) in &state.levels {
        local.iter().for_each(|&c| u32_(&mut out, c));
        out.push(*level);
    }
    u32_(&mut out, state.updates.len() as u32);
    for (local, due) in &state.updates {
        local.iter().for_each(|&c| u32_(&mut out, c));
        out.extend_from_slice(&due.to_le_bytes());
    }
    u32_(&mut out, state.rotations.len() as u32);
    for (local, r) in &state.rotations {
        local.iter().for_each(|&c| u32_(&mut out, c));
        out.push(r.index());
    }
    out
}

pub fn decode_chunk(bytes: &[u8]) -> Result<(Chunk, ChunkState), String> {
    let mut r = Reader(bytes);
    if r.take(4)? != CHUNK_MAGIC {
        return Err("pas un fichier de chunk (ou autre version)".into());
    }
    let size = r.u32()?;
    if size == 0 || size > 1024 {
        return Err(format!("taille de chunk invalide : {size}"));
    }
    let palette = (0..r.u32()?).map(|_| r.u32().map(ContentId)).collect::<Result<Vec<_>, _>>()?;
    let voxels = r
        .take((size as usize).pow(3) * 2)?
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    let chunk = Chunk::from_parts(size, palette, voxels).ok_or("voxels incohérents avec la palette")?;
    let mut state = ChunkState::default();
    for _ in 0..r.u32()? {
        let local = r.local()?;
        let items = (0..r.u32()?).map(|_| r.u32().map(ContentId)).collect::<Result<Vec<_>, _>>()?;
        state.entities.push((local, items));
    }
    if !r.0.is_empty() {
        for _ in 0..r.u32()? {
            let local = r.local()?;
            let due = r.u64()?;
            let (rule, late) = (r.u32()?, r.take(1)?[0]);
            state.timers.push((local, Timer { due, rule, late }));
        }
    }
    if !r.0.is_empty() {
        for _ in 0..r.u32()? {
            state.levels.push((r.local()?, r.take(1)?[0]));
        }
        for _ in 0..r.u32()? {
            state.updates.push((r.local()?, r.u64()?));
        }
    }
    if !r.0.is_empty() {
        for _ in 0..r.u32()? {
            let local = r.local()?;
            let rotation = Rotation::from_index(r.take(1)?[0]).ok_or("rotation hors des 24")?;
            state.rotations.push((local, rotation));
        }
    }
    if !r.0.is_empty() {
        return Err("octets en trop en fin de fichier".into());
    }
    Ok((chunk, state))
}

/// Lecture séquentielle avec erreur sur fichier tronqué.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.0.len() < n {
            return Err("fichier tronqué".into());
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn u32(&mut self) -> Result<u32, String> {
        self.take(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Result<u64, String> {
        self.take(8).map(|b| u64::from_le_bytes(b.try_into().expect("8 octets")))
    }

    /// Une position locale au chunk : x y z.
    fn local(&mut self) -> Result<[u32; 3], String> {
        Ok([self.u32()?, self.u32()?, self.u32()?])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ContentEntry;

    fn world() -> (VoxelWorld, ContentId, ContentId) {
        let reg = Registry::from_ron(
            r#"[
                (identifier: "t:air", kind: Block((solid: false, color: (0.0, 0.0, 0.0)))),
                (identifier: "t:bench", kind: Block((solid: true, color: (1.0, 1.0, 1.0), storage: Some(9)))),
                (identifier: "t:stone", kind: Block((solid: true, color: (1.0, 1.0, 1.0)))),
            ]"#,
        )
        .unwrap();
        let id = |s| reg.lookup(s).unwrap();
        let (air, bench, stone) = (id("t:air"), id("t:bench"), id("t:stone"));
        let mut w = VoxelWorld::new(reg, 8, 1.0);
        w.insert_chunk(ChunkPos { x: -1, y: 0, z: 2 }, Chunk::filled(8, air));
        (w, bench, stone)
    }

    #[test]
    fn edited_chunk_round_trips_through_disk_with_block_entities() {
        let (mut w, bench, stone) = world();
        let pos = ChunkPos { x: -1, y: 0, z: 2 };
        let v = [-3, 4, 17]; // dans le chunk (-1, 0, 2)
        w.set_voxel([-8, 0, 16], stone);
        w.set_voxel(v, bench);
        w.store(v, stone);
        assert_eq!(w.take_edited(), vec![pos]);

        let dir = std::env::temp_dir().join(format!("voxel_save_test_{}", std::process::id()));
        let save = Save::new(&dir);
        save.write_chunk(&w, pos).unwrap();
        assert!(save.read_chunk(ChunkPos { x: 0, y: 0, z: 0 }).unwrap().is_none(), "jamais sauvé");

        let (chunk, state) = save.read_chunk(pos).unwrap().unwrap();
        assert!(state.timers.is_empty());
        let mut back = VoxelWorld::new(Registry::new(), 8, 1.0);
        back.insert_saved_chunk(pos, chunk, state);
        assert_eq!(back.voxel([-8, 0, 16]), Some(stone));
        assert_eq!(back.voxel(v), Some(bench));
        assert_eq!(back.stored(v), Some(&[stone][..]));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn corrupted_chunk_is_refused() {
        let mut reg = Registry::new();
        let air = reg.register(ContentEntry::new_block("t:air", false, [0.0; 3])).unwrap();
        let state = ChunkState {
            entities: vec![([0, 0, 1], vec![air])],
            timers: vec![([1, 2, 3], Timer { due: 1 << 40, rule: 2, late: 3 })],
            levels: vec![([3, 0, 0], 2)],
            updates: vec![([0, 3, 0], 1 << 33)],
            rotations: vec![([1, 1, 1], Rotation::from_index(23).unwrap())],
        };
        let bytes = encode_chunk(&Chunk::filled(4, air), &state);
        assert_eq!(decode_chunk(&bytes).unwrap().1, state);
        // Chunk sauvé avant les rotations, les liquides, puis les minuteurs :
        // relu sans.
        let empty = encode_chunk(&Chunk::filled(4, air), &ChunkState::default());
        for cut in [4, 12, 16] {
            assert_eq!(decode_chunk(&empty[..empty.len() - cut]).unwrap().1, ChunkState::default());
        }
        let mut rotated = bytes.clone();
        *rotated.last_mut().unwrap() = 24;
        assert!(decode_chunk(&rotated).is_err(), "rotation hors des 24");
        assert!(decode_chunk(&bytes[..bytes.len() - 1]).is_err(), "tronqué");
        let mut bad = bytes.clone();
        bad[16] = 9; // premier voxel → index hors palette
        assert!(decode_chunk(&bad).is_err());
        assert!(decode_chunk(b"PNG!").is_err());
    }

    #[test]
    fn world_meta_and_player_round_trip() {
        let dir = std::env::temp_dir().join(format!("voxel_meta_test_{}", std::process::id()));
        let save = Save::new(&dir);
        assert_eq!(save.read_meta().unwrap(), None);
        let (w, bench, _) = world();
        let meta = WorldMeta { format: FORMAT_VERSION, seed: 42, voxels_per_meter: 1.0, chunk_size: 8 };
        save.write_world(&meta, &w.registry).unwrap();
        assert_eq!(save.read_meta().unwrap(), Some(meta));
        assert_eq!(save.read_registry().unwrap().lookup("t:bench"), Some(bench));

        let player = PlayerSave { feet: [1.0, 2.0, 3.0], yaw: 0.5, pitch: -0.2, inventory: vec![(bench, 3)], selected: 1 };
        save.write_player(&player).unwrap();
        assert_eq!(save.read_player().unwrap(), Some(player));

        assert_eq!(save.read_items().unwrap(), vec![], "pas encore de fichier");
        let items = vec![(bench, [0.5, 4.0, -2.5])];
        save.write_items(&items).unwrap();
        assert_eq!(save.read_items().unwrap(), items);
        assert_eq!(save.read_mobs().unwrap(), vec![]);
        let mobs = vec![(3, bench, [1.5, 4.0, 2.5], 1.25, true)];
        save.write_mobs(&mobs).unwrap();
        assert_eq!(save.read_mobs().unwrap(), mobs);
        save.write_ron("mobs.ron", &vec![(3, bench, [1.5, 4.0, 2.5], 1.25)]).unwrap();
        assert_eq!(save.read_mobs().unwrap(), vec![(3, bench, [1.5, 4.0, 2.5], 1.25, false)], "avant #60 : posés");
        // Un fichier d'avant #32, sans id, se relit.
        save.write_ron("mobs.ron", &vec![(bench, [1.5, 4.0, 2.5], 1.25), (bench, [0.0; 3], 0.0)]).unwrap();
        assert_eq!(save.read_mobs().unwrap(), vec![(0, bench, [1.5, 4.0, 2.5], 1.25, false), (1, bench, [0.0; 3], 0.0, false)]);

        save.write_ron("world.ron", &WorldMeta { format: 99, seed: 0, voxels_per_meter: 1.0, chunk_size: 8 }).unwrap();
        assert!(save.read_meta().is_err(), "autre version refusée");
        fs::remove_dir_all(&dir).unwrap();
    }
}
