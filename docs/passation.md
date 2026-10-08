# Passation de session — 2026-10-07

> Document de reprise pour une session Claude fraîche. Lire ceci, puis
> `CLAUDE.md` (racine) et `docs/brief/voxel-engine-design.md` (canonique)
> avant d'écrire du code. Le journal détaillé des décisions est dans
> `docs/journal.md`.

## Où en est le projet

**La tranche verticale (§7) est complète et dépassée** : les trois piliers
d'apprentissage du cœur voxel sont construits — stockage (chunk paletté),
meshing (greedy, ×12,8 vs naïf, fluidité validée en jeu par Warshow même
sous llvmpipe), et **streaming** (§3.4 : monde qui démarre vide, chunks
générés/déchargés autour du joueur). Dernier commit : `f135fb0`.

1. Chunk généré depuis la seed (heightmap fBm maison, pierre sous 1 m de
   sol, **grottes** par bruit 3D, déterministe), **en continu autour du
   joueur** sur plusieurs couches (cylindre 96 m × ±48 m, borné à −128 /
   384 m ; budget 4 chunks/frame, hystérésis).
2. Mesher blocky **greedy** avec **raccord inter-chunks** (le naïf reste
   comme oracle de test).
3. Pose/casse data-driven : casser fait tomber les drops du bloc en items
   au sol, ramassés à portée ; **inventaire** (barre `bevy_ui`, molette),
   poser consomme ; **contour noir du bloc visé** (gizmo, même raycast que
   le clic).
4. Déplacement FPS avec collision (AABB balayée), **figé si le chunk sous
   les pieds n'est pas chargé**.

`cargo run` → clic gauche pour jouer, WASD/Espace, clic gauche casse,
clic droit pose, molette change le bloc en main, Échap libère la souris,
F plein écran.

## Invariants à ne pas casser (au-delà du design doc)

- **Le mesh d'un chunk dépend de ses voisins** (culling inter-chunks).
  Deux obligations en découlent, déjà codées mais faciles à casser :
  un chunk qui apparaît → re-mesh de ses voisins affichés ; un voxel
  édité en bordure → re-mesh du chunk voisin. Oublier l'une = trous.
- **La solidité hors chunk est une fermeture injectée** dans le mesher :
  `mesh_chunk` = chunk isolé (air dehors, pour les tests purs),
  `mesh_chunk_in_world` = raccordé au monde. Ne pas re-hardcoder.
- **La physique ne simule jamais dans du non-chargé** (chunk absent =
  air pour `is_solid` → sans la garde, on tombe à travers le monde).
- **Décharger un mesh ≠ oublier le chunk** : l'entité et l'asset GPU
  (`meshes.remove`, sinon fuite) partent, les données restent en mémoire —
  les édits du joueur survivent. Pas de persistance disque (non-goal §7).
- **L'inventaire tient des `ContentId`** : un bloc s'y range lui-même (pas
  d'item « double »). Casser fait tomber `Registry::drops` en items au sol
  (`items.rs`, tick fixe), ramassés à 1,5 m ; poser consomme. Aucune liste
  de blocs en dur ; la barre lit couleur et nom dans le registre (§3.11).
- **Un seul chemin de meshing** : génération, streaming et pose/casse
  notent les chunks à re-mesher dans `DirtyChunks` ; seul `remesh_dirty`
  (main.rs) meshe (→ `mesh_chunk_in_world`) et crée les entités-chunk.
  Ne pas re-mesher ailleurs : deux systèmes qui spawnent la même frame
  créent deux entités pour un chunk (spawn invisible aux `Query` avant la
  frame suivante).
- **Un chunk chargé a toujours une entité `ChunkMesh`**, même au mesh
  vide (alors sans `Mesh3d`). Sinon un chunk d'air passe pour non chargé
  et est re-meshé à chaque frame (test `streaming::tests`).
- **La position du joueur est `Player::feet`**, pas son `Transform` : le
  `Transform` n'est que l'affichage, interpolé entre deux ticks
  (`smooth_transform`). La simu et la pose de bloc lisent `feet()`.
- **Le contenu est dans `assets/content/core.ron`**, chargé au démarrage
  (`Registry::from_ron`). L'ordre du fichier fixe les IDs : ajouter à la
  fin, ne jamais réordonner ni supprimer. Aucun bloc défini en Rust côté
  jeu (les tests et `mesh_stats` construisent encore leurs registres en
  code, c'est voulu).
- **Comportement = règles en donnée** (`BlockData::rules`, évaluées par
  `voxel_core::rules::actions`, appliquées par `interact.rs`). Hooks
  `Used`/`Placed`/`Broken`, condition `Holding`, effets `ReplaceSelf`/`Drop`.
  Vocabulaire **append-only** (§3.6) : ajouter des variantes, ne jamais en
  renommer ni supprimer. Un `ReplaceSelf` ne redéclenche aucun hook.
- **`voxels_per_meter` n'existe qu'une fois** : `VoxelWorld::voxels_per_meter()`
  (gelé, §3.5). Le générateur le reçoit en paramètre.

## Architecture (résumé)

```
Cargo.toml            workspace + binaire voxel_engine (Bevy 0.19)
src/main.rs           setup : registre → worldgen → mesher → entités ; GameWorld (Resource)
src/player.rs         contrôleur FPS : simu en FixedUpdate, regard/curseur en Update
src/interact.rs       pose/casse : raycast depuis la caméra, re-mesh du chunk touché (+ voisin si bordure)
src/streaming.rs      charge/décharge les chunks autour du joueur (budget/frame, hystérésis)
crates/voxel_core/    TOUT le cœur voxel, PUR (zéro dépendance Bevy, testable headless)
  registry.rs         registre append-only §3.1 (IDs = index d'insertion)
  chunk.rs            chunk paletté §3.2 (dense u16 + palette locale)
  world.rs            VoxelWorld : registre + chunks + conversions voxel↔mètres
  worldgen.rs         trait WorldGenerator + HeightmapGenerator (value noise/fBm maison)
  mesher.rs           greedy meshing (naïf conservé en oracle) → MeshData (tampons purs)
  physics.rs          AABB vs grille, région balayée (anti-tunneling)
  raycast.rs          DDA Amanatides & Woo (visée voxel + face d'entrée)
```

Règle de séparation stricte : logique voxel → `voxel_core` (42 tests
headless, ~0 s), le binaire ne fait que brancher dans l'ECS.

## Vérifications avant de conclure une étape

```bash
cargo test --workspace                    # 42 tests cœur + 2 streaming (headless)
cargo clippy --workspace --all-targets    # zéro warning exigé
cargo run                                 # smoke test à l'occasion
cargo windows                             # .exe Windows (README, « Build Windows natif »)
```

⚠️ Ne jamais lire le succès d'un build via un pipe (`cargo build | tail`
retourne le code de `tail`). Vérifier `EXIT=$?` explicitement.

## Environnement — pièges connus (tous vécus)

- **WSL2 + WSLg.** Souris : voir `docs/journal.md` « La saga de la souris »
  — 5 actes, résumé : `WAYLAND_DISPLAY` masqué dans `main()` (forçage X11),
  deltas raw inutilisables, **aucun warp curseur en WSL** (le curseur hôte
  Windows écrase tout ; détection `/proc/version` dans `player.rs`), le
  masquage du curseur est ignoré, le confinement marche. Ne pas « réparer »
  ça avec des warps : c'est mesuré, pas supposé.
- **Rendu llvmpipe (CPU)** : WSL n'expose pas le GPU à Vulkan ici.
  `mesa-vulkan-drivers` suggéré à Warshow, non confirmé. FPS modeste = normal.
- Paquets système installés pendant le bootstrap : build-essential, clang,
  mold, pkg-config, libasound2-dev, libudev-dev, libwayland-dev,
  libxkbcommon-dev, libxkbcommon-x11-0. mold est actif via `.cargo/config.toml`.
- **Bevy 0.19** : vérifier l'API contre les exemples du crate installé
  (`~/.cargo/registry/src/*/bevy-0.19.0/examples/`), pas contre sa mémoire —
  la notation BSN est arrivée mais `commands.spawn` classique reste valable.
  Piège découvert : `Window::set_cursor_position` ignoré si la demande égale
  la précédente (cache bevy_winit). Autre renommage 0.19 : les événements
  bufferisés se lisent via `MessageReader` (ex-`EventReader`).
- **Piège ECS** : les entités spawnées via `Commands` ne sont visibles dans
  les `Query` qu'à la frame suivante — d'où `DirtyChunks` + un seul système
  de meshing (`remesh_dirty`), voir les invariants.

## Conventions de travail avec Warshow

- **Français** partout (code commenté en français, commits en français).
- **Jamais de trailer `Co-Authored-By`** dans les commits (demande explicite).
- Commits soignés et descriptifs, un par étape logique, **seulement sur
  demande explicite** ; jamais de push (c'est lui qui pousse).
- **Pédagogie** : projet d'apprentissage — expliquer le *pourquoi* des
  concepts (meshing, layout mémoire, déterminisme…) dans les réponses ET
  dans `docs/journal.md`, tenu à jour à chaque étape.
- Le design doc est canonique : le modifier AVANT de coder toute entorse.

## Fait dans la session du 2026-07-09 (après-midi)

Trois jalons, chacun committé et détaillé dans `docs/journal.md` :

1. **Greedy meshing** (`3ebeca4`) — fusion des faces coplanaires par
   matériau (clé = index de palette), le naïf conservé comme oracle
   (égalité d'aire par direction). Gain mesuré ×12,8 sur les chunks réels
   (`cargo run -p voxel_core --example mesh_stats`). Fluidité confirmée
   en jeu.
2. **Sélection de blocs** (`04ab52a`) — dirt/stone/sand en donnée, hotbar
   découverte, molette, HUD du bloc en main. Plein écran F11 → F (les
   touches de fonction sont souvent interceptées par l'hôte).
3. **Streaming + raccord inter-chunks** (`34e7aa1`) — voir les invariants
   ci-dessus. Rayon de vue 96 m + marge 32 m, exprimés en mètres (§2), la
   conversion en chunks reste locale à `streaming.rs`.

## Fait dans la session du 2026-10-05 → 07

Reprise après une pause. Relecture complète, puis audit de complexité ;
détail dans `docs/journal.md` (deux entrées du 2026-10-05).

1. `48416b5` **refactor** — `Kind::Block(BlockData)` (plus d'état
   incohérent), `voxels_per_meter` privé et unique (le générateur le reçoit
   en paramètre).
2. `978395c` **fix** — un chunk vide garde son entité (sinon re-meshé à
   chaque frame : bloquant pour la verticalité) ; `DirtyChunks` +
   `remesh_dirty` = un seul système meshe (fin des entités en double).
   Test headless dans `streaming.rs`.
3. `510d667` **refactor** — regard souris : hors WSL `Locked` +
   `AccumulatedMouseMotion` (toute la mécanique de warp supprimée) ; sous
   WSL inchangé. `ContentEntry::new_block`, code mort retiré. −105 lignes.
4. `f135fb0` **chore** — config des skills mattpocock (`docs/agents/`,
   tickets GitHub, labels de triage par défaut).

**Pas encore vérifié en jeu par Warshow** : regard souris sous WSL après
le refactor, pose/casse en bordure de chunk. **Jamais testé** : le regard
hors WSL (pas de build natif).

**Firetower** (agents sur un worker, lancés depuis des tickets GitHub) :
possible mais pas branché — le dépôt n'est pas déclaré dans Firetower, et
le worker doit avoir Rust + clang + mold + les libs système de Bevy.
Proposé, non fait : ajouter à `CLAUDE.md` une règle « sans écran : prouver
par `cargo test`/`clippy`, signaler dans la PR ce qui demande un test en
jeu ».

## Prochaines étapes

Tickets #1 à #5 faits et fermés (verticalité, contour du bloc visé, HUD
debug, caméra interpolée, build Windows — tous validés en jeu).

**Ordre des jalons : `docs/jalons.md`** (décidé avec Warshow le
2026-10-07) — 1. items et inventaire, 2. hooks et règles, 3. craft par
proximité. Les décisions de socle correspondantes sont dans le design doc
(§3.1 recettes, §3.3 cycle de vie des block-entities, §3.6 forme du
comportement). Tickets ouverts hors jalons : #6 distance de vue, #7
ambient occlusion, #8 persistance (à arbitrer).

Limitations assumées (ne pas « corriger » sans besoin) : palette non
compactée, re-mesh complet du chunk au moindre voxel, pas de persistance
disque (les édits vivent en mémoire), full-bright.
