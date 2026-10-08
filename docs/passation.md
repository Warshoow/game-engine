# Passation de session — 2026-10-08

> Document de reprise pour une session Claude fraîche. Lire ceci, puis
> `CLAUDE.md` (racine) et `docs/brief/voxel-engine-design.md` (canonique)
> avant d'écrire du code. Le journal détaillé des décisions est dans
> `docs/journal.md`.

## Où en est le projet

**La tranche verticale (§7) est complète et dépassée** : les trois piliers
d'apprentissage du cœur voxel sont construits — stockage (chunk paletté),
meshing (greedy, ×12,8 vs naïf, fluidité validée en jeu par Warshow même
sous llvmpipe), et **streaming** (§3.4 : monde qui démarre vide, chunks
générés/déchargés autour du joueur). Les **trois jalons de gameplay** de
`docs/jalons.md` sont faits (items/inventaire, règles, craft sur l'établi),
plus les **textures** (jalon 4), la **sauvegarde sur disque** (jalon 5,
items au sol compris), les **recettes sans station** et les **fenêtres de
jeu** avec le menu de fabrication (#45, #46).
Dernier commit de code : `4005d35` (recettes sans station).

1. Chunk généré depuis la seed (heightmap fBm maison, pierre sous 1 m de
   sol, **grottes** par bruit 3D, déterministe), **en continu autour du
   joueur** sur plusieurs couches (cylindre 96 m × ±48 m, borné à −128 /
   384 m ; budget 4 chunks/frame, hystérésis).
2. Mesher blocky **greedy** avec **raccord inter-chunks** (le naïf reste
   comme oracle de test).
3. Pose/casse data-driven : casser fait tomber les drops du bloc en items
   au sol, ramassés à portée ; **inventaire** (barre `bevy_ui`, molette,
   case « main vide »), poser consomme ; **contour noir du bloc visé**.
4. **Contenu en donnée** dans `assets/content/core.ron` (blocs, drops,
   règles, recettes) ; **règles** déclencheur → condition → effet (lampe) ;
   **établi** : on pose les items dessus, main vide → fabrique ; une
   recette **sans `station`** se fait depuis l'inventaire, dans le menu de
   fabrication (touche C, clic sur la recette) — c'est ainsi qu'on obtient
   l'établi (2 terre + 2 pierre).
5. **Textures** (texture array, PNG de `assets/textures/` nommés par
   l'entrée, pixels gardés dans le registre ; 1 image = 1 m, répétée sur
   les faces fusionnées). Shader `assets/shaders/voxel.wgsl`.
6. **Save** dans `saves/world/` (`voxel_core::save`, `src/save.rs`) :
   métadonnées, registre complet, un fichier par chunk modifié, joueur,
   items au sol (`items.ron`).
   Une save illisible arrête le jeu, jamais écrasée. Effacer le dossier =
   monde neuf.
7. HUD debug (FPS, chunks), caméra interpolée entre ticks, touche **G**
   (debug) = un exemplaire de chaque bloc solide.
8. Déplacement FPS avec collision (AABB balayée), **figé si le chunk sous
   les pieds n'est pas chargé**.

`cargo run` → clic gauche pour jouer, WASD/Espace, clic gauche casse,
clic droit utilise (bloc à règle `Used`) ou pose, Maj+clic droit pose
toujours, molette change la case, C ouvre le menu de fabrication
(recettes sans `station`), G debug, Échap libère la souris, F plein
écran. **Pour juger le ressenti : build Windows** (`cargo windows`, ~10-16
min ; copier l'exe ET `assets/` dans `Téléchargements\voxel_engine\`). Sous
WSL le rendu est logiciel et rame — normal.

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
  les édits du joueur survivent (et sont aussi sur disque, voir la save).
- **Save** : les IDs viennent de `registry.ron` de la save, puis `core.ron`
  est fusionné par identifier (`Registry::merge`). Un chunk est écrit dès
  qu'il est édité (`VoxelWorld::take_edited`, à marquer dans toute
  nouvelle méthode qui modifie un chunk) ; joueur et items au sol toutes
  les 5 s et sur `AppExit`. Une save illisible arrête le jeu, jamais
  écrasée. Changer un format → incrémenter `save::FORMAT_VERSION` ; un
  fichier nouveau et optionnel (comme `items.ron`) n'en a pas besoin.
- **Fenêtres** (`ui.rs`) : une entité `GameWindow`, une seule à la fois
  (`ui::open` ferme l'autre). Tant qu'elle existe, `cursor_grab` libère la
  souris et ne recapture pas au clic ; tout système d'entrée du jeu doit
  lire `CursorCaptured` (déplacement, visée, pose/casse, molette le font).
- **RON lu avec `IMPLICIT_SOME`** (`registry::parse`) : un champ
  `Option` s'écrit sans `Some(…)`. Ne pas revenir à `ron::from_str` (les
  saves dont la recette s'écrit `station: "…"` ne se reliraient plus).
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
  `Used`/`Placed`/`Broken`, conditions `Holding`/`EmptyHand`/`HoldingAny`,
  effets `ReplaceSelf`/`Drop`/`StoreHeld`/`Craft`.
  Vocabulaire **append-only** (§3.6) : ajouter des variantes, ne jamais en
  renommer ni supprimer. Un `ReplaceSelf` ne redéclenche aucun hook.
- **Block-entities** (§3.3) : `VoxelWorld` garde une map position →
  items posés, pour les blocs dont l'entrée déclare `storage`. Créée par
  `set_voxel` quand le bloc posé en déclare, supprimée quand le voxel
  change ; `interact.rs` (`Edit::set`) fait d'abord tomber le contenu.
  Recettes : `ContentEntry::recipes` ; `crafting::find` (sur une
  station, entrées exactes) et `crafting::craftable` (sans station, depuis
  l'inventaire). Aucune recette en Rust.
- **`voxels_per_meter` n'existe qu'une fois** : `VoxelWorld::voxels_per_meter()`
  (gelé, §3.5). Le générateur le reçoit en paramètre.

## Architecture (résumé)

```
Cargo.toml            workspace + binaire voxel_engine (Bevy 0.19)
src/main.rs           setup : save/registre → worldgen → entités ; texture array ; GameWorld (Resource)
src/player.rs         contrôleur FPS : simu en FixedUpdate, regard/curseur en Update
src/interact.rs       pose/casse/utilisation : raycast, règles, re-mesh du chunk touché (+ voisin si bordure)
src/streaming.rs      charge/décharge les chunks (relit la save, sinon génère)
src/items.rs          items au sol (tick fixe) et cubes posés sur les blocs
src/inventory.rs      inventaire, barre, menu de fabrication (C), G (debug)
src/ui.rs             fenêtres de jeu : une à la fois, souris libérée, case d'item partagée
src/save.rs           branche voxel_core::save sur l'ECS (écritures, restauration)
assets/content/       core.ron (le contenu) ; assets/textures/ (PNG) ; assets/shaders/voxel.wgsl
crates/voxel_core/    TOUT le cœur voxel, PUR (zéro dépendance Bevy, testable headless)
  registry.rs         registre append-only §3.1 (IDs = index d'insertion)
  chunk.rs            chunk paletté §3.2 (dense u16 + palette locale)
  world.rs            VoxelWorld : registre + chunks + conversions voxel↔mètres
  worldgen.rs         trait WorldGenerator + HeightmapGenerator (value noise/fBm maison)
  mesher.rs           greedy meshing (naïf conservé en oracle) → MeshData (tampons purs)
  physics.rs          AABB vs grille, région balayée (anti-tunneling)
  raycast.rs          DDA Amanatides & Woo (visée voxel + face d'entrée)
  rules.rs            règles hook → condition → effet (évaluation pure)
  crafting.rs         recherche de recette (station ou inventaire)
  save.rs             dossier du monde, format binaire des chunks
```

Règle de séparation stricte : logique voxel → `voxel_core` (60 tests
headless, ~0 s), le binaire ne fait que brancher dans l'ECS.

## Vérifications avant de conclure une étape

```bash
cargo test --workspace                    # 67 tests (60 cœur + 7 binaire), headless
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
- **Push** : la clé SSH n'est pas toujours chargée dans la session (« Permission
  denied (publickey) »). Soit Warshow fait `! ssh-add`, soit pousser une fois
  en HTTPS via gh, sans toucher la config :
  `git -c credential.helper= -c 'credential.helper=!gh auth git-credential' push https://github.com/Warshoow/game-engine.git master`.
- **`rtk`** (proxy de sortie) masque parfois la sortie de `cargo test` /
  `cargo run` : préfixer `rtk proxy` pour voir la sortie brute.
- **Exe de debug** : `target/debug/voxel_engine` lancé directement échoue
  (`libbevy_dylib…so` introuvable, linking dynamique) → passer par
  `cargo run`.
- **Voir le rendu sous WSL** : `xwd` échoue sous WSLg. Ajouter un système
  temporaire qui spawn `Screenshot::primary_window()` avec
  `.observe(save_to_disk(path))` après ~25 s, lancer, lire le PNG, puis
  retirer le code (fait pour vérifier les textures).
- **Save locale** : `cargo run` lit/écrit `saves/world/` à la racine du
  dépôt (gitignoré) ; l'effacer pour un monde neuf, notamment si un test
  dépend du terrain généré.
- **Piège ECS** : les entités spawnées via `Commands` ne sont visibles dans
  les `Query` qu'à la frame suivante — d'où `DirtyChunks` + un seul système
  de meshing (`remesh_dirty`), voir les invariants.

## Conventions de travail avec Warshow

- **Français** pour le code commenté et les docs ; **commits en anglais**
  depuis le 2026-10-08 (règle dans `CLAUDE.md`).
- **Jamais de trailer `Co-Authored-By`** dans les commits (demande explicite).
- Commits soignés et descriptifs, un par étape logique, **seulement sur
  demande explicite** (« commit », ou validation après « je commite quand tu
  valides ») ; push seulement sur demande.
- **Pas de questionnaire** : proposer une reco tranchée avec ses raisons,
  Warshow corrige (il a écarté le craft par proximité, préféré l'établi).
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

## Fait dans la session du 2026-10-07 → 08

Détail dans `docs/journal.md`. Tickets GitHub fermés ou à fermer au push.

- Verticalité + grottes (#1), contour du bloc visé (#2), HUD debug (#3),
  caméra interpolée + yaw par frame (#4), build Windows `cargo windows` (#5).
- Design doc v0.1 : recettes sur l'entrée produite (§3.1), cycle de vie des
  block-entities (§3.3), forme du comportement en 3 étages + vocabulaire
  append-only (§3.6). `docs/jalons.md` créé.
- 5 epics (label `epic`, sous-tickets GitHub) : #20 Contenu et outillage,
  #26 Rendu, #31 Monde, #38 Simulation et jeu, #43 Technique.
- Registre depuis `core.ron` (#16), jalon 1 items/inventaire (#9), jalon 2
  règles (#10), jalon 3 craft sur l'établi + block-entities (#11, #13
  fusionné), jalon 4 textures (#21), jalon 5 save (#8). Tous validés en
  jeu par Warshow. Ticket #44 (textures générées par IA) ouvert.
- Complément du jalon 5 : items au sol sauvés (`9b91d4d`). Recettes sans
  station + touche C (`4005d35`), validé sous Windows.
- Tickets ouverts : #45 base commune des fenêtres/menus (epic #43, lève
  « UI riche »), #46 menu de fabrication pour choisir la recette (epic
  #38, dépend de #45).
- Poussé jusqu'à `4005d35` ; le commit de docs qui suit ne l'est pas.

## Prochaines étapes

Ordre décidé avec Warshow (2026-10-08) : **`docs/jalons.md`, « Suite :
ordre des tickets ouverts »**. #45 et #46 faits ; suivant : #17.
Principe (design doc §2) : le moteur fournit des mécanismes, `core.ron`
n'est qu'un jeu d'exemple. Mipmaps si les textures scintillent au loin.

Limitations assumées (ne pas « corriger » sans besoin) : palette non
compactée, re-mesh complet du chunk au moindre voxel, chunks sauvés sans
compression, items au sol sauvés sans leur vitesse, full-bright.
