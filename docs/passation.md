# Passation de session — 2026-10-30

> Document de reprise pour une session Claude fraîche. Lire ceci, puis
> `CLAUDE.md` (racine) et `docs/brief/voxel-engine-design.md` (canonique)
> avant d'écrire du code. Le journal détaillé des décisions est dans
> `docs/journal.md`.

## Où en est le projet

**La tranche verticale (§7) est complète et dépassée** : les trois piliers
d'apprentissage du cœur voxel sont construits — stockage (chunk paletté),
meshing (greedy, ×2,6 vs naïf avec l'ombrage des coins sur `mesh_stats`, fluidité validée en jeu par Warshow même
sous llvmpipe), et **streaming** (§3.4 : monde qui démarre vide, chunks
générés/déchargés autour du joueur). Les **trois jalons de gameplay** de
`docs/jalons.md` sont faits (items/inventaire, règles, craft sur l'établi),
plus les **textures** (jalon 4), la **sauvegarde sur disque** (jalon 5,
items au sol compris), les **recettes sans station** et les **fenêtres de
jeu** avec le menu de fabrication (#45, #46).
Plus le **rechargement du contenu en jeu** (touche R, #17) et les **blocs
générés par IA en jeu** (touche I, #18 + #44), validés sous Windows. Et le
**worldgen en donnée** (#27 : biomes, minerais, arbres), les **blocs à
trous** (#22 : feuilles, verre), les **blocs qui évoluent seuls** (#34 :
minuteurs `After` + tick aléatoire `Tick`, le blé) et les **liquides** (#28 :
eau, lave, mer, nage, semi-transparent), validés sous Windows le
2026-10-25 (sauf la mer d'un monde neuf, pas encore essayée). Puis
l'**ombrage des coins** (#7), vu sous Windows. Puis la **lumière du ciel et
des blocs** (#23 : grottes noires, lampe allumée, lave, eau qui assombrit),
validée sous Windows par Warshow le 2026-10-26 sur le PC sans GPU. Premier
passage sur le PC avec RTX 4060 le 2026-10-28 : 144 FPS après le
chargement, mais 2 à 5 s à ~3 FPS au chargement. Corrigé par #53 le
2026-10-29 (meshing et génération en tâches, chunk meshé une fois),
à re-mesurer sur la 4060. La force des lampes n'a pas encore été jugée à
l'œil.

1. Chunk généré depuis la seed (heightmap fBm maison, **grottes** par
   bruit 3D, **biomes, minerais, arbres** décrits dans
   `assets/content/worldgen.ron`, déterministe), **en continu autour du
   joueur** sur plusieurs couches (cylindre 96 m × ±48 m, borné à −128 /
   384 m ; génération en tâches, 1 chunk inséré et 2 meshes lancés par
   image, hystérésis).
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
clic droit utilise (bloc dont une règle `Used` s'applique à ce qu'on tient) ou pose, Maj+clic droit pose
toujours, molette change la case, C ouvre le menu de fabrication
(recettes sans `station`), I génère un bloc (IA, `claude` dans le PATH),
R recharge le contenu, G debug, Échap libère la souris, F plein
écran. **Pour juger le ressenti : build Windows** (`cargo windows`, ~10-16
min ; copier l'exe ET `assets/` dans `Téléchargements\voxel_engine\`). Sous
WSL le rendu est logiciel et rame — normal.

## Invariants à ne pas casser (au-delà du design doc)

- **Le mesh d'un chunk dépend de ses voisins** (culling inter-chunks).
  Deux obligations en découlent, déjà codées mais faciles à casser :
  un chunk qui apparaît → re-mesh de ses 26 voisins affichés ; un voxel
  édité en bordure → re-mesh des chunks voisins, diagonales comprises
  (jusqu'à 7, `chunk::meshes_reading`). Oublier l'une = trous ; oublier
  les diagonales = ombre des coins fausse le long des arêtes de chunk (#7).
  La lumière ajoute les siens : les chunks dont une case a changé de
  lumière (`take_light_changed`, re-meshés par `remesh_dirty` s'ils sont
  affichés).
- **Une face est cachée par un voisin solide opaque, ou par un voisin
  identique** (`mesher::hides`, #22, #28). Le greedy et l'oracle naïf
  partagent cette règle : la changer dans l'un seul casse les tests
  d'équivalence. `solid` = collidable ; meshé = solide ou liquide. Deux
  meshes par chunk (`mesh_chunk_in_world` rend `[reste, Blend]`), deux
  entités (`ChunkMesh`, `BlendMesh`). La clé de fusion du greedy est
  (index de palette, haut, bas, ombres et lumière des 4 coins) : une surface de
  liquide abaissée ne fusionne pas avec une pleine, ni deux faces
  d'ombres différentes.
- **Ombrage des coins** (#7) : calculé au meshing (`mesher::face_ao`),
  jamais stocké ; fait de l'ombre ce qui est solide et opaque
  (`Resolved::casts_shadow`, la même règle que `hides`). L'ombre d'un coin
  ne dépend que de sa position : c'est ce qui rend la fusion exacte, et
  l'oracle naïf la vérifie sommet par sommet.
- **Lumière** (#23, `voxel_core::light`) : calculée par `insert_chunk` et
  `set_voxel`, jamais sauvée. **Un chunk sans données au-dessus est sous le
  ciel ouvert** : le streaming charge d'abord les chunks qui pourraient le
  couvrir (jusqu'à `Worldgen::top_m`, ou dans la save) ; insérer à la main
  un chunk sous un autre absent le croit au soleil (rattrapé si le haut
  arrive ensuite). Changer le registre (touche R) → `relight()`. Le test
  `random_edits_match_a_fresh_computation` compare l'incrémental à un
  calcul de zéro : toute modification de la propagation doit le garder
  vert.
- **Vertex shader à nous** (#23, `assets/shaders/voxel.wgsl`) : la
  lumière passe par un attribut à soi (`ATTRIBUTE_LIGHT`, emplacement 8).
  Tout mesh rendu avec `VoxelMaterial` doit l'avoir (`to_bevy_mesh`).
  `BlockTextures::specialize` ne touche qu'à la passe principale (sort
  tout de suite si `PREPASS_PIPELINE`) : la passe de profondeur (ombres)
  garde les emplacements de Bevy (`prepass_io.wgsl`), avec notre fragment
  `voxel_prepass.wgsl` qui découpe les trous des blocs `Mask` (#51).
- **Effets de rendu** (#51, `src/effects.rs`) : F1 ombres, F2 brouillard,
  F3 brouillard en volume, F4 bloom ; coupés au démarrage en rendu
  logiciel (llvmpipe sous WSL : 1 FPS avec, 6 sans). L'eau (`BlendMesh`)
  est `NotShadowCaster`. Les faces d'un bloc à `emits_light` sont en
  pleine lumière de bloc (mesher) : c'est d'elles que part le halo.
- **Jour et nuit** (#30, `src/sky.rs`, `voxel_core::sky`, `sky.ron`) :
  l'heure se calcule depuis l'horloge du monde, jamais stockée à part.
  `update_sky` règle chaque image la `DirectionalLight` (soleil de 6 h à
  18 h, lune sinon), l'ambiance (`GlobalAmbientLight`), `ClearColor` et la
  couleur des deux brouillards (= horizon), le matériau du ciel, et
  `BlockTextures::daylight` (uniform 102 de `voxel.wgsl`, renvoyé au GPU
  seulement quand il bouge de plus de 0,01). La sphère du ciel suit la
  caméra (rayon 400 m). `sky.ron` : le fichier du jeu prime, la save en
  garde une copie (au contraire de `worldgen.ron`). F5 : +1 h, non sauvé.
- **Blocs à forme** (#14, `voxel_core::shape`) : `shape` et `collision`
  en seizièmes de case (`Cuboid`, entiers 0 à 16), `orient` à la pose.
  La rotation (une des 24, `Rotation`) est dans `VoxelWorld::rotations`
  (absent = identité). `set_voxel` l'efface : écrire la rotation
  **après** (`set_rotation`), comme le niveau d'un liquide. Physique et
  visée lisent `VoxelWorld::boxes` (`collision` pour la physique, `shape`
  pour la visée : une torche se casse sans arrêter le joueur). Le mesher
  émet les blocs à forme **et les cubes tournés** (bûche couchée) hors du
  greedy (`Region::mesh_shapes`), lumière de leur propre case, sans
  ombrage, texture prise sur le modèle avant rotation ; les blocs à forme
  n'arrêtent pas la lumière. Le
  joueur au sol monte seul 0,6 m (`move_and_step`).
- **Modèles d'entités** (#15, `voxel_core::model`, `src/entities.rs`) :
  `Kind::EntityType(EntityData)` porte `model` (parties avec parent
  déclaré avant, pivot et boîtes en seizièmes de mètre, repère du modèle)
  et `size` (collision en mètres). `Part::mesh` rend les boîtes **autour
  du pivot** ; chaque partie est une entité Bevy enfant de sa parente, à
  `pivot - pivot du parent`. Lumière : un mesh `GridLit` reçoit dans
  `MeshTag` la lumière de la grille à sa position (bit 16 = entité, ciel
  et blocs sur 8 bits) ; `voxel.wgsl` la prend à la place de celle des
  sommets. Un chunk n'a pas de `MeshTag` (0). Items de bloc au sol :
  `mesh_block_item`, même matériau et même lumière. Mobs dans
  `mobs.ron` (id, entrée, pieds, cap ; l'ancien format sans id se relit).
- **Mobs** (#32, `voxel_core::mob`, `voxel_core::path`) : `behaviors`
  et `animation` dans `EntityData`, vocabulaire append-only. Le pas de
  physique est `physics::Body::step`, **le même pour le joueur et les
  mobs** : un changement de marche, de saut ou de nage touche les deux.
  `mob::step_all` prend les mobs **triés par id** (déterminisme) ; la
  recherche de chemin (`path::Search`) avance par tranches, plafond
  `PATH_NODES_PER_TICK` pour tous les mobs : ne pas la lancer d'une traite
  dans le jeu (512 cases ≈ 2,4 ms). Une case est praticable avec le test
  de la physique (`path::stand`) : changer la physique, c'est changer les
  chemins. Affichage : `show_mobs` interpole et tourne les parties
  (`Parts`), rien de visuel dans la simulation sauf `walked` et `swing`.
- **Apparition des mobs** (#60, `voxel_core::spawn`) : `spawn` dans
  `EntityData` (règles en donnée). Les essais dérivent de (seed, tick,
  numéro d'essai), **jamais du nombre de mobs**. `Mob::spawned` : apparu
  seul, disparaît au-delà de `despawn` même gelé ; posé (`G`) jamais ;
  `Follow { keep: Some(s) }` le rend gardé après `s` secondes de suivi
  (élevage ; compteur `followed`, pas sauvé). Un mob à plus de
  `mob::SIMULATION_M` (128 m) du joueur est gelé : les données d'un chunk
  ne quittent pas la mémoire, sans ce gel les mobs marchaient partout où
  le joueur était passé. `SIMULATION_M` = vue + marge de déchargement et
  `SPAWN_MAX_M` = vue : vérifié à la compilation dans `streaming.rs`.
- **Liquides** (#28) : le voxel reste l'ID du liquide, son niveau est dans
  `VoxelWorld::levels` (absent = source, `Some(0)` = chute, sinon distance
  à plat en voxels). `set_voxel` efface le niveau : écrire le niveau
  **après** (`set_liquid_level`). `set_voxel` programme aussi le réexamen
  du voxel et de ses six voisins (`take_due_updates`, plafond
  `MAX_UPDATES_PER_TICK`) ; c'est ce qui fait couler l'eau et déclenche
  `NeighborChanged`. Un liquide n'entre que dans l'air du worldgen
  (`GameWorld::air`). Ne rien faire tourner « pour tous les liquides » :
  le coût au repos doit rester nul.
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
  nouvelle méthode qui modifie un chunk) : encodé dans l'image, le fichier
  écrit par un fil à part (`save::ChunkWriter`, #54), attendu sur
  `AppExit` ; joueur et items au sol toutes
  les 5 s et sur `AppExit`. Une save illisible arrête le jeu, jamais
  écrasée. Changer un format → incrémenter `save::FORMAT_VERSION` ; un
  fichier nouveau et optionnel (comme `items.ron`) n'en a pas besoin.
- **Fenêtres** (`ui.rs`) : une entité `GameWindow`, une seule à la fois
  (`ui::open` ferme l'autre). Tant qu'elle existe, `cursor_grab` libère la
  souris et ne recapture pas au clic ; tout système d'entrée du jeu doit
  lire `CursorCaptured` (déplacement, visée, pose/casse, molette le font).
- **Le registre peut changer en cours de partie** (touche R, `content.rs`).
  Ne rien en mettre en cache qui ne se rafraîchisse pas : `reload_content`
  re-meshe tous les chunks, remplace le texture array, marque les
  `ItemVisual` et l'inventaire modifiés. Les IDs, eux, ne bougent jamais.
- **Le worldgen est world-owned** (`worldgen.ron` dans la save, prime sur
  celui du jeu). Une feature ne doit dépendre que de sa colonne et le
  parcours des candidats se fait dans l'ordre du monde : c'est ce qui rend
  un arbre identique des deux côtés d'un bord de chunk (test
  `small_chunks_match_one_big_chunk`).
- **Minuteurs (`After`, #34)** : tout bloc qui apparaît lance les siens
  dans `Edit::set` (interact.rs) — pose, règle ou autre minuteur ; une
  nouvelle façon d'écrire un voxel doit passer par là ou appeler
  `start_timers`. `set_voxel` les tue avec le bloc. Une chaîne compte
  depuis l'échéance précédente (rattrapage au rechargement), bornée par
  `MAX_CATCH_UP`. Le worldgen refuse les blocs à `After`. Échéances en
  ticks de 1/64 s (`TICKS_PER_SECOND`, sauvé dans `clock.ron`) : ne pas
  changer ce pas. Les minuteurs tournent pour tout chunk en mémoire, mais
  `tick_blocks` ne re-meshe que les chunks affichés (sinon `remesh_dirty`
  ferait apparaître des chunks lointains).
- **Le hasard des règles (`Chance`) vient de `rules::event_seed`** (seed,
  tick, voxel), jamais d'un aléa libre. Le contexte d'une règle se
  construit par `interact::context`, pour le voxel de la règle.
- **Un contenu généré n'entre qu'après `generate::accept`** (#18) :
  namespace `gen:`, références et textures vérifiées sur une copie du
  registre. Ne jamais enregistrer une réponse de générateur sans passer
  par là. Un champ texte ouvert (`ui::TextField`) coupe les raccourcis
  (`ui::not_typing`) ; un raccourci ajouté doit en tenir compte.
- **RON lu avec `IMPLICIT_SOME`** (`registry::parse`) : un champ
  `Option` s'écrit sans `Some(…)`. Ne pas revenir à `ron::from_str` (les
  saves dont la recette s'écrit `station: "…"` ne se reliraient plus).
- **L'inventaire tient des `ContentId`** : un bloc s'y range lui-même (pas
  d'item « double »). Casser fait tomber `Registry::drops` en items au sol
  (`items.rs`, tick fixe), ramassés à 1,5 m ; poser consomme. Aucune liste
  de blocs en dur ; la barre lit couleur et nom dans le registre (§3.11).
- **Un seul chemin de meshing** : génération, streaming et pose/casse
  notent les chunks à re-mesher dans `DirtyChunks` ; seul `remesh_dirty`
  (main.rs) meshe et crée les entités-chunk.
  Ne pas re-mesher ailleurs : deux systèmes qui spawnent la même frame
  créent deux entités pour un chunk (spawn invisible aux `Query` avant la
  frame suivante).
- **Meshing en tâches (#53, design doc §6)** : `remesh_dirty` copie chaque
  chunk avec une case de bordure (`mesher::Region`, ~0,5 ms dans l'image)
  et le meshe dans une tâche. Les meshes lancés à la même image forment un
  lot (`Meshing`), appliqué d'un coup et dans l'ordre : pas de trou entre
  deux chunks touchés par un même edit. Un chunk déchargé abandonne ses
  meshes en cours (`Meshing::forget`), sinon ils recréeraient son entité.
- **Streaming (#53)** : colonnes lues ou générées en tâches (`Generating`) ;
  une colonne arrêtée sous un chunk encore en chargement attend qu'il soit
  inséré (lumière du ciel, §3.8). Un chunk n'est meshé que quand ses
  voisins de la zone ont leurs données (une fois au chargement, test
  `each_chunk_is_meshed_once_on_load`) ; un chunk affiché dont un voisin
  arrive ou dont la lumière change à cause du streaming est re-meshé sous
  le même budget. Mesure : `cargo test --release --bin voxel_engine --
  --ignored --nocapture mesure_chargement` (images cadencées à 144 Hz).
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
  `Used`/`Placed`/`Broken`/`Tick`/`After`/`NeighborChanged`, conditions
  `Holding`/`EmptyHand`/`HoldingAny`/`Chance`/`Above`/`Touching`, effets
  `ReplaceSelf`/`Drop`/`StoreHeld`/`Craft`.
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
src/content.rs        charge core.ron + textures (démarrage, touche R, bloc généré via Incoming)
src/generate.rs       fenêtre de génération (I), trait Generator, ClaudeCli (thread + OnceLock)
src/ui.rs             fenêtres de jeu : une à la fois, souris libérée, case d'item, champ texte
src/save.rs           branche voxel_core::save sur l'ECS (écritures, restauration)
assets/content/       core.ron (le contenu) ; assets/textures/ (PNG) ; assets/shaders/voxel.wgsl
crates/voxel_core/    TOUT le cœur voxel, PUR (zéro dépendance Bevy, testable headless)
  registry.rs         registre append-only §3.1 (IDs = index d'insertion)
  chunk.rs            chunk paletté §3.2 (dense u16 + palette locale)
  world.rs            VoxelWorld : registre + chunks + conversions voxel↔mètres, horloge, minuteurs, niveaux des liquides, réexamens
  worldgen.rs         trait WorldGenerator, HeightmapGenerator (relief, grottes), Worldgen (biomes, minerais, features en donnée)
  mesher.rs           greedy meshing (naïf conservé en oracle) → MeshData (tampons purs)
  physics.rs          AABB vs grille, région balayée (anti-tunneling), Body (joueur et mobs)
  mob.rs              comportements des mobs, un tick pour tous (triés par id)
  path.rs             A* sur les cases, par tranches (Search)
  spawn.rs            apparition des mobs près du joueur, disparition au loin
  raycast.rs          DDA Amanatides & Woo (visée voxel + face d'entrée)
  rules.rs            règles hook → condition → effet (évaluation pure)
  ticks.rs            tick aléatoire : quels voxels reçoivent le hook Tick
  liquids.rs          écoulement : chaque case se recalcule d'après ses voisins
  crafting.rs         recherche de recette (station ou inventaire)
  texgen.rs           textures calculées (motif + couleur + bruit + graine), déterministes
  generate.rs         prompt envoyé au générateur, validation de sa réponse (accept)
  save.rs             dossier du monde, format binaire des chunks
```

Règle de séparation stricte : logique voxel → `voxel_core` (96 tests
headless, ~0 s), le binaire ne fait que brancher dans l'ECS.

## Vérifications avant de conclure une étape

```bash
cargo test --workspace                    # 104 tests (96 cœur + 8 binaire), headless
cargo test -- --ignored claude            # génération réelle par claude (~6 s)
cargo clippy --workspace --all-targets    # zéro warning exigé
cargo run                                 # smoke test à l'occasion
cargo windows                             # .exe Windows (README, « Build Windows natif »)
cargo windows-profile                     # le même, avec le profilage (#41, README)
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
- **Trace de profilage** : à 144 FPS elle grossit d'environ 100 Mo par
  seconde ; `profilage.bat` la zippe, `trace_stats.py` lit le zip (Perfetto
  sature au-delà de ~1 Go). 20 s maximum par scénario. Les FPS de référence
  se prennent avec l'exe normal ; sur la 4060 la vsync plafonne à 144, le
  coût d'un effet se lit dans les lignes `elapsed_gpu` du log.
- **Clé USB de Warshow** (D: sous Windows) : non montée dans WSL. La lire
  par `cmd.exe /c copy D:\<fichier> <dossier C:>`, puis depuis `/mnt/c`.
- **Piège ECS** : les entités spawnées via `Commands` ne sont visibles dans
  les `Query` qu'à la frame suivante — d'où `DirtyChunks` + un seul système
  de meshing (`remesh_dirty`), voir les invariants.

## Conventions de travail avec Warshow

- **Français** pour le code commenté et les docs ; **commits en anglais**
  depuis le 2026-10-08 (règle dans `CLAUDE.md`).
- **Jamais de trailer `Co-Authored-By`** dans les commits (demande explicite).
- Commits soignés et descriptifs, un par étape logique, **sans demander**
  une fois l'étape vérifiée (tests + clippy) ; **push seulement sur
  demande** (Warshow pousse).
- **Pas de questionnaire** : proposer une reco tranchée avec ses raisons,
  Warshow corrige (il a écarté le craft par proximité, préféré l'établi).
- **Explorer avant de coder un mécanisme** : la méthode est dans
  `CLAUDE.md` (« Explorer avant de coder un mécanisme ») — solution de
  référence et ses défauts, alternatives comparées sur les mêmes coûts,
  reco, ticket des options écartées (#48, #49).
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
- Ordre de tous les tickets ouverts décidé (`docs/jalons.md`, « Suite »).
  Design doc §2 : principe « le moteur fournit des mécanismes, le contenu
  fait le jeu » ; commits en anglais (`CLAUDE.md`).
- Fenêtres de jeu + menu de fabrication (`26d8a70`, #45 #46) ;
  rechargement du contenu, touche R (`e4c03bb`, #17). Validés sous Windows.
- Blocs générés par IA en jeu (#18 + #44) : trait `Generator`, CLI
  `claude` sans outils, validation `generate::accept`, textures calculées
  (`texgen`). Validé sous Windows. Correctif au passage : au rechargement, le
  matériau reçoit le texture array sous un nouveau handle (sinon une
  couche ajoutée restait invisible, ramenée à la dernière).
- Worldgen en donnée (#27, `4bc3951`), blocs à trous (#22, `6eb1e50`) et
  blocs qui évoluent seuls (#34) : validés sous Windows le 2026-10-25.

## Fait dans la session du 2026-10-21 → 22

- #27 worldgen en donnée (`4bc3951`), #22 blocs à trous (`6eb1e50`), #34
  blocs qui évoluent seuls (`4c69645`). Détail dans `docs/journal.md`.
- Tickets créés : #47 (mesh opaque séparé si le profilage le montre), #48
  (options écartées ou reportées de #34 : voisin modifié → avec #28,
  rattrapage du tick aléatoire, avance à heure fixe).
- Poussé par Warshow le 2026-10-25 ; #22, #27 et #34 fermés.

## Fait dans la session du 2026-10-25

- #28 liquides : simulation et rendu (`d43064c`), nage (`fbb4e18`),
  eau, lave, mer (`79c5b79`), G donne les liquides (`d7b223c`). Design doc
  amendé d'abord (§3.1, §3.2, §3.6, §3.10). Options écartées : #49.
- Validé sous Windows par Warshow : écoulement, reflux, lave → pierre au
  contact de l'eau, nage, blocs à trous, blé. Un défaut trouvé et corrigé
  (`c823006`) : les côtés des liquides n'étaient pas dessinés entre deux
  niveaux. Pas encore essayé : la mer d'un monde neuf.
- Méthode « explorer avant de coder un mécanisme » ajoutée à `CLAUDE.md`.
- Poussé par Warshow jusqu'à `55e8d02` ; #28 et #7 fermés.
- #7 ombrage des coins (`30d7f35`). Design doc amendé d'abord (§3.8).
  Coût mesuré (`mesh_stats`, 50 chunks denses) : 88 655 → 160 375 quads,
  greedy 3,2 → 4,3 ms par chunk. Options écartées : #50. Effets de rendu
  (ombres du soleil, brouillard, bloom, SSAO, reflets) : #51, après #23.
  Vu sous Windows (PC sans GPU) : rien de particulier.

## Fait dans la session du 2026-10-26

- #23 lumière du ciel et des blocs. Phase exploratoire, option 1 retenue
  par Warshow ; design doc amendé d'abord (§3.8, §5, §7), non-goal levé
  dans `CLAUDE.md`. Options écartées : #52. Données : `emits_light`
  (portée en m), `light_loss` (eau 0,5). Vu dans WSL : un monde neuf (la
  mer comprise) rend comme avant en plein jour. Testé sous Windows par
  Warshow (PC sans GPU) : « cela fonctionne bien ». Reste à voir sur le
  PC avec RTX 4060 : le rendu et le coût (scénarios ajoutés à #41).
- Poussé par Warshow jusqu'à `0545d1b` ; #23 fermé.

## Fait dans la session du 2026-10-28

- #51 : phase exploratoire présentée (options, coûts, reco en commentaire
  du ticket). **En attente de la décision de Warshow** ; ticket des options
  écartées à créer à la décision.
- #41 : outillage de profilage (feature `profile`, `cargo windows-profile`,
  `tools/profilage.bat`, `tools/trace_stats.py`), testé sur le PC sans GPU
  puis premier passage de Warshow sur la 4060 (résultats dans #41).
  Ouvert : #54 (sauvegarde d'un chunk dans l'image).
- Commits jusqu'à `b01c67e`, à pousser par Warshow.

## Fait dans la session du 2026-10-29

- Cible de perf revue avec Warshow : écran 144 Hz de la 4060, 7 ms par
  image ; le PC sans GPU n'est plus une cible (un jeu demande au moins une
  puce graphique).
- #53 fait (meshing et génération hors de l'image, chunk
  meshé une fois ; options écartées : #55). Sur le PC de dev : pire image
  du chargement 366 ms → 6 à 8 ms, course à 8 m/s médiane 1 ms et 0 à 11
  images sur 1440 au-delà de 7 ms (la lumière d'un chunk inséré, 1 à 7 ms,
  reste dans l'image). **À re-mesurer sur la 4060** (`profilage.bat
  chargement` puis `course`) avant de fermer #53. Le dossier
  `Downloads\voxel_engine_rtx` a encore les exe d'avant #53 : à refaire
  (`cargo windows`, `cargo windows-profile`) quand Warshow prévoit la
  mesure, pas avant (demandé le 2026-10-29) ; ajouter alors le scénario
  « chargement » (monde neuf) au `LISEZMOI.txt`.
- Commits jusqu'à `cf63639` (plus celui de cette passation), à pousser
  par Warshow.

## Fait dans la session du 2026-10-30

- #54 fait : la saccade venait de l'écriture du fichier (12 ms sur le
  disque Windows depuis WSL, 0,25 ms sur ext4 ; 90 ms mesurées sur la
  4060), pas de l'encodage (0,02 ms). Le chunk est encodé dans l'image et
  écrit par un fil à part, dans l'ordre d'envoi ; la fermeture attend les
  écritures en cours.
- #51 codé : ombres du soleil (prepass à nous, feuilles trouées), brouillard
  de distance couleur du ciel (le fond devient bleu ciel), brouillard en
  volume et rayons de lumière (repris par rapport à la reco du 2026-10-28 :
  la cible est la 4060), bloom avec faces émissives ; options reportées :
  #56. **Pas vu en vrai** : sous WSL le rendu logiciel tombe à 1 FPS avec
  les effets, et Warshow a signalé que le PC ne suivait plus ; arrêté là.
  Une seule capture, prise avant que le monde soit chargé : image
  uniformément gris-bleu, d'où la densité du voile baissée à l'estime
  (`HAZE_DENSITY`). **À voir et mesurer sur la 4060** avant de fermer #51 :
  rendu de chaque effet (F1 à F4), réglages (densité du voile, biais
  d'ombre, force du bloom), temps GPU par passe (`elapsed_gpu`).
  **Juger le bloom dans une grotte avec une lampe, pas en surface** : la
  lumière propre d'un bloc est multipliée par « 1 − lumière du ciel »
  (`voxel.wgsl`), donc une lampe ou de la lave en plein air ne brille pas
  et n'a pas de halo (voulu, comme Minecraft ; la nuit viendra avec #30).
- #39 fermé : fait par #53.
- Les exe du dossier `Downloads\voxel_engine_rtx` datent d'avant #53, #54
  et #51 : à refaire (`cargo windows`, `cargo windows-profile`) quand
  Warshow prévoit le passage sur la 4060, avec au `LISEZMOI.txt` le
  scénario « chargement » (monde neuf), les touches F1 à F4 (effets) et
  F5 (+1 h, #30) : juger la nuit, lampe dehors comprise.
- Bevy 0.20.1 existe (signalé par Warshow le 2026-10-30) : rien dans
  #51 n'en avait besoin ; montée de version à faire en ticket à part si
  un besoin apparaît.
- Commits jusqu'à `df33ae9` (plus celui de cette passation), à pousser
  par Warshow.
- #30 jour/nuit codé (exploration présentée et validée le même jour ;
  options reportées : #57, dont l'idée de Warshow : la météo comme un cas
  d'événements du monde lisibles par les règles, une lune rouge qui
  change les mobs). Vu sous WSL, effets coupés : 8 h, 17 h 30, minuit
  rendent comme voulu. Les étoiles étaient des éclats anguleux (case de
  grille découpée par la sphère) : points ronds au centre des cases.
  **À voir sur la 4060**, avec les effets : raccord du brouillard avec
  l'horizon, halo du soleil et des lampes la nuit (bloom), tremblement
  des bords d'ombre quand le soleil tourne (sinon : le tourner par pas,
  #57), brouillard en volume la nuit (sa lumière ambiante, réglage de
  Bevy, pourrait le faire luire), couleurs et forces de `sky.ron`
  (réglées à l'estime ; une retouche s'applique au monde existant).
  Captures WSL : la souris de Warshow sur la fenêtre tourne la caméra.
- Commits jusqu'à `2407296` (plus celui de cette passation), à pousser
  par Warshow.
- #14 blocs à forme fait (exploration présentée, Warshow a délégué ;
  avis extérieur de DeepSeek V4.1 Flash via opencode, ce qui en est
  retenu ou écarté est dans #58). Boîtes en seizièmes de case, rotation
  dans une table creuse, montée de marche. Contenu d'exemple : dalle et
  escalier en planches, torche, bûche qui se couche. Vu sous WSL (effets
  coupés, worldgen modifié hors du dépôt pour en poser) : torches et
  escaliers rendent. **À essayer en jeu par Warshow** : pose orientée
  (escalier selon le regard, dalle haute, torche et bûche sur un mur),
  marche sur dalles et escaliers, visée d'une torche.
  Pas fait : deux dalles qui fusionnent en un bloc. Aucun effet de règle
  ne consomme l'objet tenu, la fusion dupliquerait des planches (#58).
  Formes en seizièmes de case validées par Warshow (figé).
  Ensuite : la texture tourne avec le bloc, et correction d'une bûche
  couchée qui restait dessinée debout (le greedy ignorait sa rotation ;
  un cube tourné passe maintenant par le chemin des formes).
- #15 modèles d'entités fait (exploration présentée, Warshow a validé les
  boîtes ; avis de DeepSeek retenu ou écarté dans #59). Cochon d'exemple
  (`core:pig`) posé depuis l'inventaire (touche G puis clic droit), immobile ;
  items de bloc au sol dessinés comme leur bloc. Vu sous WSL (effets
  coupés, cochons et items écrits dans la save d'un monde neuf hors du
  dépôt) : textures, yeux, pattes, cap. **À essayer en jeu par Warshow** :
  poser un cochon, le voir dans une grotte (sombre) et près d'une lampe,
  items de bloc au sol, recharger le monde (les cochons restent).
  Pas fait : animation et déplacement (#32), `model` d'item (#59).
- Essai Windows de Warshow le 2026-10-30 : chargement, effets, jour et
  nuit, formes, cochons validés. Corrigé ensuite (`2d86fe7`, `93ff582`) :
  une dalle posée par le vide d'une autre la remplaçait ; un bloc à règle
  `Used` empêchait de poser contre lui même quand aucune règle ne
  s'appliquait (blocs générés) ; le reflet et la lumière du soleil
  passaient la roche (taches blanches, orangées au coucher, au fond des
  grottes : la lumière du ciel règle maintenant aussi le reflet, et le
  plancher d'une grotte est une lueur fixe `DARK` dans `voxel.wgsl`) ;
  une bûche couchée était noire (lumière de sa propre case, pleine).
  Revus et validés par Warshow le même jour (grotte sans lampe, bûche
  couchée, pose contre un bloc généré). L'heure « non gardée » venait de
  F5, qui ne fait que décaler l'affichage (non sauvé, voulu) : le HUD le
  dit maintenant (`ef14ee6`).
- Commits jusqu'à `3423ab0` (plus celui de cette passation), à pousser
  par Warshow. `3423ab0` porte `Closes #14` : pousser après l'essai en
  jeu, ou rouvrir #14 si l'essai montre un souci.
- Puis `b24deff`, `32eab4c`, `2c72711` (#15, `Closes #15`), `2d86fe7`,
  `93ff582`, `d36ed8c`, `ef14ee6` et le commit de cette passation, à
  pousser par Warshow : l'essai en jeu de #14 et #15 est fait et validé.

- #32 mobs, exploré puis tranché avec Warshow (reco suivie ; design doc
  §3.6 « Comportement des mobs » ; apparition reportée à #60 ; toutes les
  options citées dans #61). Physique du joueur sortie dans
  `physics::Body`, comportements en donnée, A* par tranches (mesuré : 512
  cases d'un coup = 2,4 ms, d'où le plafond de 64 cases par tick), id
  stable par mob, animation `Swing` et `Look`. Le cochon suit le joueur
  qui tient du blé, le regarde, erre. Vu sous WSL (effets coupés) : les
  cochons tombent, errent, montent des blocs, l'un vient regarder la
  caméra ; l'ancien `mobs.ron` sans id se relit. Essayé et validé sous
  Windows par Warshow (pattes, tête, suivi au blé, eau, sauvegarde, dix
  cochons fluides). Seul défaut trouvé : il sautait sur un escalier (la
  case d'escalier vue par A* est à 1 m) ; corrigé, il ne saute plus que
  si le pas automatique l'a bloqué au tick d'avant (`4a6e6fe`).
- Commits à pousser par Warshow : `e58c46e`, `5cea975`, `0c31c5c`
  (#32), `4a6e6fe`, `0c89327`, puis le design doc de #60, `acf30aa`
  (`Closes #60`) et celui de cette passation. #32 et #60 se ferment au
  push.
- #60 apparition des mobs, exploré puis tranché avec Warshow (reco B
  suivie ; design doc §3.6 « Apparition des mobs » ; options : #62).
  Règles `spawn` en donnée, essais près du joueur, disparition au loin,
  gel au-delà de 128 m ; rangement par chunk reporté (il ira avec le
  déchargement des données de chunks). Ajout demandé par Warshow
  (« élevage oui ») : `Follow { keep }`, un cochon qui a suivi le blé
  10 s reste (une durée : le croiser blé en main ne le garde pas).
  Vu sous WSL (monde neuf, 75 s) : 11 cochons apparus sur l'herbe, sauvés
  avec « apparu seul ». **À essayer sous Windows : #63.**
- **Tickets de QA** (demandé par Warshow) : label `qa`, une liste de
  points à cocher par fonctionnalité après un build Windows (#63, mobs),
  ou par machine (#64 : tout ce qui attend la RTX 4060, mesures et rendu).
  Écrire la liste de test dans un ticket `qa` plutôt que seulement dans la
  conversation. Convention : `docs/agents/triage-labels.md`.

## Prochaines étapes

Ordre décidé avec Warshow (2026-10-08) : **`docs/jalons.md`, « Suite :
ordre des tickets ouverts »**. #45, #46, #17, #18 + #44, #27, #22, #34, #28
faits et validés sous Windows (reste à voir : la mer, dans un monde neuf).
#7 coins assombris fait, vu sous Windows par Warshow : rien de
particulier. Mais le PC de test n'a pas de GPU (rendu logiciel) et le jeu
y était déjà peu fluide avant #7 : **#41 profilage à faire sur le PC avec
RTX 4060** (méthode, scénarios et solutions possibles dans le ticket).
#23 éclairage fait le 2026-10-26, validé sous Windows sur le PC sans GPU.
**À faire sur le PC avec RTX 4060** : #41 (profilage, scénarios de la
lumière compris) et un coup d'œil au rendu de la lumière (force des
lampes réglée à l'estime : `BLOCK_LIGHT` dans `voxel.wgsl`). Outillage
prêt le 2026-10-28 : feature `profile` (`cargo windows-profile`), lanceur
`tools/profilage.bat`, résumé d'une trace par `tools/trace_stats.py` ;
dossier `Downloads\voxel_engine_rtx` (les deux exe + `LISEZMOI.txt`, le
protocole) remis à Warshow. FPS à relever avec l'exe normal, le profilage
ralentit. Premier passage le 2026-10-28 (PC portable, « RTX 4060 Laptop
GPU », branché ; toujours mesurer branché) : GPU quasi au repos, 144 FPS
(vsync : à 144 Hz le compteur cache le coût d'un effet, mesurer #51 avec
les lignes `elapsed_gpu` du log de profilage) après le chargement ; le chargement passe 2 à 5 s à ~3 FPS à cause
de `remesh_dirty` → **#53**, fait le 2026-10-29, à re-mesurer sur la
4060 ; une saccade de 90 ms de `save_edited_chunks` → **#54**, fait le
2026-10-30. Détail dans
#41. Ensuite,
choisi avec Warshow le 2026-10-25 : **#51 effets de rendu** (codé le
2026-10-30, à voir et mesurer sur la 4060 ; options reportées : #56),
puis #30 jour/nuit (codé le 2026-10-30, à voir sur la 4060 ; options
reportées : #57). Puis #14 blocs à forme (fait le 2026-10-30, validé
en jeu ; options écartées : #58). #15 modèles d'entités (fait le
2026-10-30, boîtes en parties ; options écartées : #59). #32 mobs
(fait le 2026-10-30 : physique partagée, `Wander`/`Follow`/`LookAt`, A*
par tranches, `Swing`/`Look` ; validé sous Windows ; options : #61).
#60 apparition des mobs (fait le 2026-10-30, **à essayer en jeu : #63** ;
options : #62). **Tout ce qui attend la 4060** (#53, #51, #30, #23, la
mer, le coût des mobs) est rassemblé dans **#64** (label `qa`).
Ce qui a été reporté (#18, #27, #28) est listé
dans `docs/jalons.md`, « Plus tard », et dans #49.
Principe (design doc §2) : le moteur fournit des mécanismes, `core.ron`
n'est qu'un jeu d'exemple. Mipmaps si les textures scintillent au loin.

Limitations assumées (ne pas « corriger » sans besoin) : palette non
compactée, re-mesh complet du chunk au moindre voxel, chunks sauvés sans
compression, items au sol sauvés sans leur vitesse ; les items et le bloc
tenu ne reçoivent pas la lumière du monde (pleine lumière dans une grotte).
