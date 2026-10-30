# Journal de bord

Trace des étapes construites, avec le *pourquoi* — complément pédagogique du
design doc (qui dit ce qui est figé, pas comment on y est arrivé).

## 2026-07-09 — Bootstrap + premières briques du data model

**Squelette.** Workspace Cargo à deux niveaux : `voxel_core` (logique pure,
zéro dépendance Bevy, testable headless) + binaire racine (l'app Bevy). Cette
séparation n'est pas cosmétique : elle *force* le découplage — le registre ou
le mesher ne peuvent pas accidentellement dépendre du rendu, le compilateur
l'interdit. Et `cargo test -p voxel_core` tourne en ~0 s sans compiler Bevy.

**Registre (§3.1).** `Vec<ContentEntry>` où l'ID est l'index d'insertion.
L'invariant append-only est porté par l'API (aucune méthode de suppression ni
de réordonnancement n'existe), pas par la discipline. Identifiers namespacés
(`core:stone`) uniques à vie ; lookup par nom en scan linéaire — assumé, le
hot path passe par les IDs entiers, jamais par les strings.

**Chunk paletté (§3.2).** Tableau dense d'indices `u16` + palette locale
`Vec<ContentId>`. Le calcul qui justifie la palette : chunk 32³ = 32 768
voxels ; en `u32` global partout = 128 KiB/chunk ; en index `u16` paletté =
64 KiB, et la porte est ouverte vers des largeurs adaptatives (1/2/4 bits)
puisqu'un chunk réel utilise rarement plus de quelques dizaines de matériaux.
Layout mémoire x-majeur (`x + size·(y + size·z)`) : boucler sur x en interne =
accès séquentiels, cache-friendly. La palette ne se compacte pas quand un
matériau disparaît — optimisation notée, non faite (elle ne sert pas la slice).

**Worldgen (§2, §4).** Trait `WorldGenerator` (la forme pluggable figée) +
`HeightmapGenerator` (l'impl bête voulue par la slice : fBm → sol/air).
Deux invariants du doc s'y matérialisent :

- *Déterminisme sans état* : pas de RNG, tout dérive d'un hash SplitMix64 de
  `(seed, coordonnées)`. Le même couple donne le même voxel quel que soit
  l'ordre de génération → parallélisable gratuitement, testable exactement.
- *Mètres, pas blocs* : `ground_level_m`, `amplitude_m`, `feature_size_m` ;
  `voxels_per_meter` n'intervient qu'à la conversion finale. Changer la
  résolution change la granularité du terrain, pas son relief.

Le bruit (value noise interpolé smoothstep + 4 octaves de fBm) est écrit à la
main : c'est un concept qu'on voulait comprendre, et ça garantit le
déterminisme cross-machine sans dépendance.

**Tests (13, headless).** Les plus importants : stabilité des IDs de registre
après ajouts ; indépendance des palettes entre chunks ; même seed → même
chunk ; **continuité de la surface aux frontières de chunks** (le bug
classique d'un noise mal recollé) ; cohérence `height_m` ↔ voxels générés.

**Leçons de bootstrap** (détail dans [dev-setup.md](dev-setup.md)) : machine
sans toolchain C → erreur `cc: Permission denied` trompeuse (un cc Windows
dans le PATH WSL) ; headers Wayland requis au *build* par winit sous WSLg ;
et ne jamais lire le succès d'un build à travers un pipe (`| tail` masque le
code de sortie).

## 2026-07-09 (suite) — Mesher naïf + premier terrain affiché

**Mesher, étape 1 : culling naïf (§7.2).** Principe : seules les faces au
contact d'un voxel non-solide existent. Pour chaque voxel solide, on émet un
quad par voisin non-solide — un chunk plein n'émet que sa surface. Le greedy
meshing (fusion des faces coplanaires) sera l'étape 2, sur cette base.

Choix structurants :
- Le mesher vit dans `voxel_core`, **sans Bevy** : il sort des tampons bruts
  (`MeshData` : positions/normales/couleurs/indices), l'app les convertit en
  `Mesh` Bevy. On teste en comptant des faces, pas des pixels.
- **Data-driven** : solidité et couleur viennent du registre, résolues *une
  fois par entrée de palette* (pas par voxel — d'où `Chunk::get_local`).
  Un ID absent du registre rend magenta : un bug de contenu doit se *voir*.
- Positions en **mètres** (`voxel × voxel_size_m`), origine locale au chunk —
  le placement world-space est l'affaire du `Transform` ECS.
- Enroulement CCW vu de l'extérieur (convention wgpu) — testé par produit
  vectoriel contre la normale déclarée, sinon le back-face culling mange tout.
- Hors du chunk = air, assumé : les faces de bordure sont émises même si le
  chunk voisin les cache. Le raccord viendra avec le streaming.

Tests clés : voxel isolé = 6 faces ; deux voxels adjacents = 10 (les faces au
contact disparaissent) ; voxel enterré = 0 ; échelle en mètres ; winding.
19 tests headless au total, clippy clean.

**Branchement Bevy (`src/main.rs`).** Au `Startup` : registre (`core:air`,
`core:grass`) → `HeightmapGenerator` → grille 5×5 de chunks → `mesh_chunk` →
`Mesh` Bevy (`Mesh3d` + `StandardMaterial` blanc, les couleurs viennent des
sommets). Caméra fixe en surplomb, `DirectionalLight` (full-bright assumé,
non-goal §7). Registre + générateur posés en `Resource` pour la suite
(pose/casse). Vérifié : l'API 0.19 contre les exemples du crate installé
(la notation BSN est arrivée, mais `commands.spawn` classique reste valable).

**Environnement.** Sous WSL2, wgpu tombe sur `llvmpipe` (rendu logiciel) —
lent mais suffisant pour la slice. Piste : `mesa-vulkan-drivers` (driver
« dozen » D3D12) pour l'accélération GPU. Erreurs ALSA au lancement =
bénignes (pas de périphérique audio WSL).

## 2026-07-09 (suite) — Déplacement : monde, collision, contrôleur FPS

**`VoxelWorld` (§3.1, §3.10).** La structure qui *possède* registre + chunks
+ métadonnées — celle que la save sérialisera et que physique/pose/casse
interrogent. Elle parle deux langues avec une frontière nette : coordonnées
voxel monde (`i64`, la grille) et mètres (`f32`, le world-space) ;
`voxels_per_meter` est l'unique pont. Piège classique traité et testé : la
conversion voxel → chunk exige la division **euclidienne** (`div_euclid`) —
avec `/` tronqué, x = −1 donnerait chunk 0/local −1 au lieu de chunk −1/31.

**Collision AABB (§7.4).** Résolution **axe par axe** façon Minecraft :
trois passes 1D au lieu d'un solveur 3D, et le glissement le long des murs
tombe gratuitement. Leçon du jour : la première version ne testait que la
*position d'arrivée* — les 4 tests de chute/mur ont échoué d'un coup, parce
qu'une chute de 10 m en un tick « saute » par-dessus le sol sans jamais le
chevaucher (tunneling). Le fix : tester la **région balayée** départ →
arrivée et clamper contre la première face dans le sens du mouvement (avec
une marge de peau pour que les floats ne re-collent pas la boîte au tick
suivant — régression testée aussi). Les tests headless ont attrapé le bug
avant la première partie ; c'est exactement leur travail.

**Contrôleur FPS (`src/player.rs`).** La répartition suit §3.9 à la lettre :
la *simulation* (WASD → intention, gravité 22 m/s², saut, collision) vit en
`FixedUpdate` ; le *regard* (souris, capture curseur) en `Update`, frame
variable — la caméra n'influence la simu qu'à travers le yaw stocké. Tout
est en mètres : joueur 0,6 × 1,8 m, yeux à 1,62 m, marche 5 m/s. Clic gauche
capture le curseur, Échap le relâche. Pas encore d'interpolation visuelle
entre ticks (le rendu échantillonne le dernier état simulé) — viendra si le
64 Hz se voit.

28 tests headless, clippy clean workspace.

## 2026-07-09 (suite) — Pose/casse : la slice est fonctionnellement complète

**Raycast DDA (Amanatides & Woo).** Pour viser un voxel, on ne « marche »
pas le long du rayon à petits pas (ça rate des voxels dans les coins et
gaspille des tests partout ailleurs) : le DDA saute de frontière de grille
en frontière de grille, sur l'axe dont la prochaine frontière est la plus
proche le long du rayon (`t_max` minimal). On visite ainsi *exactement* la
suite des voxels traversés. Le hit rapporte la **face d'entrée** (sa
normale) : casser cible le voxel, poser cible `voxel + normale`. Testé
notamment : rayon rasant (le piège des implémentations à pas fixe) et
indépendance à la résolution — le même monde en 2 vox/m donne les mêmes
distances en mètres (invariant §2).

**Pose/casse (`src/interact.rs`).** Clic gauche casse, clic droit pose. Le
bloc posé est un `ContentId` (`GameWorld::held`) — le système ne sait pas
*ce qu'il* pose, il écrit un ID du registre (§7.3 : data-driven, pas
hardcodé). Garde-fous : pas de pose dans le volume du joueur (test AABB),
pas d'interaction curseur libre, et l'ordre des systèmes (`interact` avant
`cursor_grab`) évite que le clic qui capture le curseur casse un bloc.
Après écriture, le chunk touché est **re-meshé entièrement** — brut mais
suffisant, et c'est le même chemin que le meshing initial (un seul code à
faire évoluer vers le greedy). Détail Bevy : `meshes.insert(id, mesh)`
remplace l'asset en place — l'entité et son handle ne bougent pas, le GPU
reçoit juste les nouveaux tampons.

**La tranche verticale (§7) est fonctionnellement complète** : chunk généré
depuis la seed ✓, meshé blocky ✓, pose/casse data-driven avec re-mesh ✓,
déplacement dessus ✓. 35 tests headless, clippy clean.

**Prochaines pistes** (après validation en jeu) : greedy meshing (étape 2),
interpolation caméra entre ticks si le 64 Hz se sent, curseur pointé sur le
voxel visé (surbrillance), HUD debug egui.

## 2026-07-09 (suite) — La saga de la souris sous WSLg

Faire tourner une caméra FPS sous WSLg s'est révélé être une enquête en
cinq actes. Documenté en détail parce que chaque acte est un piège générique.

**Acte 1 — Wayland WSLg : aucun delta.** Le compositeur Wayland de WSLg ne
fournit ni pointer lock ni mouvements relatifs. Fix : masquer
`WAYLAND_DISPLAY` au début de `main()` pour forcer winit sur X11/XWayland
(Bevy compile les deux backends et préfère Wayland).

**Acte 2 — Lock émulé : rétroaction.** Sous XWayland, `CursorGrabMode::
Locked` est émulé par téléportations au centre… comptées comme des
mouvements → deltas géants, caméra qui plonge et spinne. Fix : abandonner
les deltas « raw » pour la position absolue + recentrage manuel.

**Acte 3 — Cache bevy_winit : warp fantôme.** `Window::set_cursor_position`
n'est poussé vers l'OS que si la *demande* diffère de la demande précédente
(comparaison au cache, pas à l'état réel — `bevy_winit/src/system.rs`).
Redemander « pile le centre » chaque frame n'est appliqué qu'une fois → le
curseur dérive, delta persistant ∝ distance au centre : caméra-joystick.
Fix : cible alternée d'un ±½ px.

**Acte 4 — Warp lent : recomptage.** Mesurer le delta « depuis le centre »
recompte le même offset à chaque frame de latence du warp → accélération
fantôme. Fix : delta entre deux positions *successives* (correct quelle que
soit la latence), recentrage seulement près du bord, écho du warp d'abord
avalé… ce qui mangeait du mouvement réel (atténuation perçue). Raffiné en
*soustraction* de l'écho (signature : saut colinéaire au warp émis,
amplitude comparable) — le mouvement n'est plus jamais avalé.

**Acte 5 — Le verdict WSLg (diagnostic instrumenté).** Logs à l'appui :
les deltas raw sont ~1000× trop grands (périphérique *absolu* émulé par
RDP — inutilisables), et les warps ne déplacent que l'état interne de
XWayland : le curseur **hôte Windows** se réimpose au premier mouvement
physique (à-coup), et le masquage du curseur est ignoré. Conclusion : sous
WSL, on ne warp **pas du tout** (détection `/proc/version`) ; on s'appuie
sur le confinement (qui marche) + F11 plein écran sans bordure pour donner
de l'amplitude. Sur Linux natif, le recentrage se réactive tout seul.

**Leçons.** (1) Instrumenter avant de raffiner : le diagnostic d'une heure
a invalidé deux « fixes » plausibles. (2) La chaîne
souris→RDP→WSLg→XWayland→winit→Bevy a six maillons ; chaque symptôme
(« joystick », « accélération », « atténuation ») désignait un maillon
différent. (3) Pour le vrai test de feel, un build Windows natif reste la
solution propre — WSL est l'environnement de dev, pas de jeu.

## 2026-07-09 (suite) — Mesher, étape 2 : greedy meshing

**Le problème du naïf.** Un quad 1×1 par face visible : un sol plat 32×32,
géométriquement *un* rectangle, coûte 1024 quads. Le GPU s'en moque un peu
(il avale des millions de triangles), mais chaque quad = 4 sommets à
transformer, de la bande passante, de la mémoire — et sous llvmpipe (rendu
CPU en WSL), chaque triangle compte vraiment.

**L'algo (Lysenko, 2012).** Trois idées :

1. **Balayer par tranches.** Pour chacune des 6 directions de face, le chunk
   est découpé en tranches perpendiculaires à la normale. Les faces visibles
   d'une tranche sont coplanaires par construction → le problème 3D devient
   32 problèmes 2D (un masque `size × size` par tranche).
2. **Clé de fusion.** Chaque case du masque porte l'index de palette du
   voxel, ou rien. Deux faces ne fusionnent que si leur clé est identique —
   c'est ce qui préserve le data-driven : pierre et terre adjacentes restent
   deux quads, chacun sa couleur. (Plus tard, la clé s'enrichira : lumière,
   UV… tout ce qui distingue visuellement deux faces doit la casser.)
3. **Croissance gourmande.** Première case non consommée → étendre en
   largeur tant que la clé matche → étendre en hauteur tant que la *ligne
   entière* matche → émettre UN quad w×h → effacer le rectangle du masque.
   « Gourmand » = localement optimal : on ne cherche pas le pavage minimal
   (NP-difficile), et c'est très bien comme ça.

**Le piège de l'enroulement.** Avec un quad de taille variable sur 6
orientations, l'erreur classique est un winding CW → face mangée par le
back-face culling. Solution structurelle plutôt que 6 cas particuliers :
(d, u, v) reste une permutation *cyclique* de (x, y, z), donc û × v̂ = d̂ ;
parcourir les coins u-d'abord est CCW pour la face +d, v-d'abord pour −d.
Le test `winding_is_ccw_seen_from_outside` (produit vectoriel vs normale
déclarée, sur une dalle qui exerce les 6 directions) verrouille ça.

**L'oracle.** Le naïf reste dans le code (`mesh_chunk_naive`) comme
*oracle de test* : sur un terrain irrégulier à deux matériaux, greedy et
naïf doivent couvrir **exactement la même aire, direction par direction**
(`greedy_covers_same_surface_as_naive`). Toute face manquante, dupliquée ou
débordante casse cette égalité — c'est un test bien plus puissant que
compter des quads sur des cas simples. Pattern général : quand on optimise,
garder la version lente comme référence exécutable.

**Le gain, mesuré** (`cargo run -p voxel_core --example mesh_stats`, les
25 chunks réels du binaire, seed 42) : 110 500 quads naïfs → **8 661 quads
greedy, ×12,8** (−92 %). Et rien à changer dans le binaire : génération et
re-mesh passaient déjà tous deux par `mesh_chunk` — l'intérêt d'avoir un
seul point d'entrée de meshing.

**Limitation assumée** (comme pour le naïf) : les faces de bordure de chunk
sont toujours émises, et fusionnées entre elles sous le terrain — des quads
invisibles subsistent aux frontières. Le raccord inter-chunks viendra avec
le streaming.

## 2026-07-09 (suite) — Sélection de blocs : le data-driven en action

**Le but.** Prouver que « le contenu est de la donnée » (§0) tient la route :
ajouter des blocs posables sans toucher un seul système.

**Ce qui a été fait.** Trois blocs de plus au registre (`core:dirt`,
`core:stone`, `core:sand`) — trois lignes de *donnée* dans le setup. La
hotbar n'est pas une liste écrite à la main : elle se **découvre** en
filtrant le registre (tout bloc solide est posable), via un nouvel
itérateur en lecture seule `Registry::iter()`. Conséquence vérifiable :
l'air n'y est pas (non solide), l'herbe y est, et le prochain bloc ajouté
au registre apparaîtra dans la hotbar sans qu'aucun système ne change.

**Molette.** `select_held_block` fait défiler un index cyclique dans la
hotbar — le système ne manipule que des `ContentId`, il ne sait pas ce
qu'il sélectionne. Le HUD affiche l'identifier du bloc en main, lu du
registre au moment du changement (pas de table de noms côté UI). Bevy
0.19 : les événements bufferisés se lisent via `MessageReader` (l'ancien
`EventReader`, renommé) ; les crans de la frame sont sommés car un
trackpad émet plusieurs petits événements là où une molette en émet un.

**Au passage** : plein écran remappé F11 → **F** (les touches de fonction
sont souvent interceptées par l'hôte ou le terminal, surtout via WSLg).

## 2026-07-09 (suite) — Streaming de chunks + raccord inter-chunks

**Le pilier 3.** Stockage ✓, meshing ✓ — restait le streaming (§3.4 :
« identique à l'infini » — charger/décharger autour du joueur, les bornes
du monde ne seront qu'un check en plus). Le monde ne pré-génère plus
rien : il démarre **vide**, et `stream_chunks` (Update) fait tout.

**Le raccord d'abord.** Le streaming rendait le problème des bordures
inévitable : jusqu'ici « hors chunk = air » était codé en dur, chaque
chunk émettait ses 4 murs de bordure sous le terrain. La solidité hors
chunk devient une **fermeture injectée** : `mesh_chunk` garde le
comportement « chunk isolé » (les tests headless restent purs), et
`mesh_chunk_in_world(world, pos)` branche la fermeture sur le monde —
les faces au contact d'un voisin solide disparaissent. Corollaire assumé :
le mesh d'un chunk **dépend de ses voisins**. Deux conséquences en
cascade, faciles à oublier :

1. Quand un chunk *apparaît*, ses voisins déjà affichés doivent être
   re-meshés (leur couture se referme).
2. Quand on édite un voxel *en bordure*, le chunk voisin doit être
   re-meshé aussi (sa face culled doit (ré)apparaître). Sinon : trou.

**La boucle de streaming.** Chaque frame : (1) l'ensemble voulu = un
disque de chunks autour du joueur, rayon en **mètres** (`VIEW_DISTANCE_M`,
§2 — la conversion en chunks se fait dans le système, nulle part
ailleurs) ; (2) génération du plus proche au plus loin sous un **budget
par frame** (4) — le coût se lisse, pas de hitch en franchissant une
frontière ; (3) meshing différé en fin de passe et dédupliqué — piège
ECS : les entités spawnées via `Commands` ne sont visibles dans les
`Query` qu'à la frame suivante, re-mesher au fil de l'eau aurait dupliqué
des meshes ; (4) déchargement au-delà du rayon + **hystérésis** (32 m),
sinon un joueur qui oscille sur une frontière fait charger/décharger en
boucle.

**Ce qu'on décharge — et ce qu'on garde.** On despawn l'entité et on
libère l'asset GPU (`meshes.remove` — sinon fuite : despawner l'entité ne
libère pas l'asset). Les **données** du chunk restent en mémoire : les
édits du joueur survivent à l'aller-retour. La persistance *disque* reste
un non-goal (§7) ; la persistance *mémoire* est du bon sens.

**La garde physique.** `is_solid` traite un chunk absent comme de l'air —
la doc de `world.rs` prévenait déjà : « quand le streaming arrivera, la
physique devra refuser de simuler dans du non-chargé ». C'est fait : si
le chunk sous les pieds n'a pas de données, le tick ne simule pas ce
joueur. Figé ≠ cassé — la simu reprend dès que le sol existe. C'est aussi
ce qui rend le démarrage « monde vide » sûr : le joueur flotte une
fraction de seconde, le temps que le budget génère son chunk.

**Vertical.** Une seule couche de chunks (y = 0) : le terrain de la
heightmap tient dans [0, 32). La verticalité (caves, ciel) élargira la
boucle de l'ensemble voulu, pas la logique.

## 2026-10-05 — Passe de relecture

Relecture complète du code à froid. Tests et clippy étaient verts ; quatre
corrections.

**Chunk vide = re-meshé à chaque frame.** Le streaming jugeait « affiché »
un chunk qui a une entité-mesh, et `remesh_chunk` n'en créait pas pour un
mesh vide. Un chunk d'air n'était donc jamais vu comme chargé : re-meshé
(32³ voxels) à chaque frame, et il mangeait le budget de 4. Invisible tant
qu'on ne charge que y = 0 (le sol est toujours dans ce chunk), bloquant
dès la verticalité. Correctif : l'entité `ChunkMesh` existe toujours, le
`Mesh3d` seulement si le mesh n'est pas vide. Test headless dans
`streaming.rs` (monde tout en air), vérifié en échec sur l'ancien
comportement.

**Deux entités pour un chunk.** `interact` et `stream_chunks` meshaient
chacun de leur côté, dans `Update`, sans ordre. Une entité spawnée via
`Commands` n'est visible des `Query` qu'à la frame suivante : si les deux
touchaient le même chunk la même frame, chacun spawnait la sienne, et la
seconde gardait une géométrie périmée. Correctif : ils ne meshent plus, ils
notent dans une ressource `DirtyChunks` (un `HashSet`, donc dédoublonné), et
un seul système, `remesh_dirty`, ordonné après eux, meshe. Le dédoublonnage
manuel du streaming disparaît.

**`voxels_per_meter` en double.** Il vivait dans `VoxelWorld` (champ `pub`,
modifiable alors que §3.5 le veut gelé) ET dans `HeightmapGenerator` —
rien n'empêchait les deux de diverger. Désormais champ privé + accesseur ;
le générateur le reçoit en paramètre de `generate_chunk`, comme
`chunk_size`.

**`Kind` porte ses données.** `ContentEntry { kind, block: Option<BlockData> }`
autorisait `Kind::Block` sans données ou un `Item` avec. Devenu
`Kind::Block(BlockData)` : l'incohérence ne s'écrit plus. Registre toujours
unique à kinds unifiés (§3.1 inchangé) ; lecture via `entry.block()`.

**Laissé de côté (noté, pas urgent)** : seed, version de format et bord du
monde ne sont pas encore dans `VoxelWorld` (à ranger avant la persistance) ;
les données de chunk ne sont jamais libérées (~64 Kio/chunk, tient jusqu'à
la persistance disque) ; `WAYLAND_DISPLAY` retiré même hors WSL.

## 2026-10-05 (suite) — Passe « moins de code » (audit de complexité)

Audit du dépôt entier à la recherche de code en trop. Plan, écrit avant
de toucher au code :

1. **Regard souris hors WSL** (`player.rs`) : toute la mécanique de warp
   (recentrage près du bord, détection de l'écho du warp, cible alternée
   d'un demi-pixel) ne servait que hors WSL — chemin jamais exécuté, le
   jeu n'ayant tourné que sous WSLg. Hors WSL, Bevy fournit déjà le
   déplacement relatif de la souris (`AccumulatedMouseMotion`) avec le
   curseur verrouillé (`CursorGrabMode::Locked`, qui retombe tout seul
   sur `Confined` sous X11). Sous WSL, rien ne change : différence entre
   deux positions successives du curseur, sans warp, en `Confined`.
2. **Constructeur `ContentEntry::new_block(identifier, solid, color)`** :
   remplace les ~17 littéraux `ContentEntry { kind: Kind::Block(BlockData
   { … }) }` des tests, de `main.rs` et de l'example.
3. **Tests du mesher** : le produit vectoriel, écrit deux fois, devient une
   fonction `cross`.
4. **Supprimés car jamais appelés** : `Registry::len` / `is_empty`,
   `VoxelWorld::chunks()`, `PartialOrd`/`Ord` sur `ContentId`.

Gardés exprès : le trait `WorldGenerator` (une seule implémentation, mais
§4 fige la forme « pipeline pluggable » — l'enlever demande d'abord de
modifier le doc), le mesher naïf (oracle des tests du greedy et de
`mesh_stats`), `Kind::Item` / `Kind::EntityType` (§3.1).

Contrôle de non-régression : `cargo test --workspace` et `cargo clippy
--workspace --all-targets` identiques avant/après (43 tests, 0 warning),
`mesh_stats` donne les mêmes chiffres, smoke test `cargo run`. Le regard
souris sous WSL est vérifié par lecture : son chemin de code doit rester
le même qu'avant.

**Résultat.** −105 lignes nettes. 43 tests verts, clippy `-D warnings`
propre, `mesh_stats` à l'identique (8661 quads greedy, ×12,8), `cargo run`
démarre avec les mêmes logs qu'avant (avertissements d'environnement
seulement : rendu logiciel, pas d'audio). Non vérifié faute de build
natif : le regard souris hors WSL — à tester au premier build Windows.

## 2026-10-07 — Verticalité : plusieurs couches de chunks + grottes (#1)

**Problème.** Le streaming ne chargeait que la couche y = 0 : creuser sous
0 m tombait dans du non-chargé (physique figée), et rien n'existait sous
la surface.

**Streaming (`streaming.rs`).** La zone chargée devient un **cylindre** :
disque de 96 m à l'horizontale, ±48 m en vertical (distance joueur →
centre du chunk). Pourquoi pas une sphère de 96 m : on regarde loin à
l'horizon, rarement à 96 m sous ses pieds ; charger toute cette roche
coûterait ~3× plus de chunks pour rien. Tri par distance 3D (le chunk
sous les pieds d'abord). Déchargement : au-delà de la marge sur l'un ou
l'autre axe. Bornes du monde §3.4 (−128 à 384 m) : aucune couche hors de
cet intervalle n'est générée — c'est le « simple check de bordure » du doc.

**Worldgen (`worldgen.rs`).** Pierre sous 1 m de sol. Grottes par
**intersection de deux bruits 3D** : un bruit vaut ~0,5 sur une surface
ondulée ; « proche de 0,5 » est une plaque épaisse autour d'elle ; deux
plaques indépendantes se coupent le long d'un tube qui serpente — un
tunnel. Un seul bruit seuillé donnerait des bulles isolées. Bruit 3D =
value noise 2D existant avec un axe de plus (8 coins), même hash, donc
même déterminisme. Le second bruit n'est calculé que si le premier est
dans la bande. Mesuré (seed 42) : ~10 % du sous-sol en vide, tunnels de
3 à 5 m, 1,8 ms/chunk 32³ en profil dev.

**Tests.** Worldgen : chaque voxel de chunks hors origine (y négatif
compris) vaut ce que disent `height_m` / `is_cave_m` en coordonnées
monde ; le sous-sol ne contient que pierre et vide, avec des grottes mais
moins de 20 % de vide. Streaming : couches chargées autour du joueur, et
rien sous le fond du monde. 44 tests, clippy propre.

**Pas vérifié en jeu.** Aspect des grottes, descente dans les couches
inférieures.

## 2026-10-08 — Registre chargé depuis un fichier de données (#16)

**Problème.** Le registre était bien de la donnée, mais remplie en Rust
dans `main.rs` : ajouter un bloc demandait de recompiler, et le jalon 1
(#9) allait ajouter items, drops et « ce qu'un item pose » au même endroit.

**Choix.** Fichier `assets/content/core.ron`, lu au démarrage avec
`std::fs` (le registre doit exister avant le premier système ; le chargeur
d'assets de Bevy est asynchrone). RON plutôt que JSON : commentaires, et
les `enum` Rust s'y écrivent tels quels (`Block((solid: …))`). `serde` et
`ron` entrent dans `voxel_core` : c'est la première dépendance du cœur,
justifiée par le besoin (§1 : crates branchées quand le besoin arrive).
Dossier résolu comme les assets Bevy : `CARGO_MANIFEST_DIR` sous
`cargo run`, sinon à côté de l'exécutable.

**Tests.** Le fichier livré est chargé par un test (un RON cassé casse
les tests, pas le démarrage) ; ordre → IDs, doublon refusé, champ
manquant signalé avec sa position.

## 2026-10-08 (suite) — Jalon 1 : items au sol et inventaire (#9)

**Bloc ↔ item.** Un bloc se range **lui-même** dans l'inventaire : pas
d'item « pierre » doublant chaque bloc, qui aurait doublé le fichier de
contenu pour rien. `BlockData::drops` dit ce qu'il donne (absent :
lui-même ; `Some([])` : rien ; sinon une liste d'identifiers, validés au
chargement). Exemple réel : l'herbe donne de la terre. `places` (un item
qui pose un bloc) attend le premier vrai item.

**Item au sol** (`items.rs`). Entité simulée au tick fixe (l'inventaire
est un état de simulation) : gravité + `move_and_collide`, comme le
joueur, figée hors chunk chargé. Ramassée à 1,5 m du centre du joueur.
Rendu : cube de 25 cm de la couleur de l'entrée, mesh et matériaux
partagés. Pas d'interpolation entre ticks (commentaire `ponytail:`).

**Inventaire** (`inventory.rs`). Une pile par `ContentId`, ordre de
première obtention, sans limite. Le joueur démarre vide. La barre
`bevy_ui` est reconstruite quand l'inventaire change (rare), plutôt que
synchronisée case par case. Lève en partie « UI riche » (§7, noté dans le
design doc).

**Tests.** Drops (défaut, vide, autre bloc, référence inconnue refusée),
inventaire (piles, retrait, sélection), et un test headless où un item
tombe, se pose sur le sol, puis est ramassé quand le joueur s'approche.
49 tests, clippy propre.

## 2026-10-08 (suite) — Jalon 2 : hooks et règles (#10)

**Forme** (étage 2 de §3.6). Une règle est de la donnée dans l'entrée du
bloc : `(on: Used, when: [Holding("…")], then: [ReplaceSelf("…")])`.
Vocabulaire de départ : hooks `Used` (clic droit), `Placed`, `Broken` ;
condition `Holding` ; effets `ReplaceSelf`, `Drop`. Le tick attend #34.

**Séparation évaluer / appliquer.** `voxel_core::rules::actions` dit
quelles actions une règle déclenche (identifiers résolus), sans toucher au
monde : pur, testable, déterministe. `interact.rs` applique, via un petit
`Edit` qui regroupe écrire un voxel + noter les chunks à re-mesher + faire
tomber un item (le code de bordure de chunk n'existe plus qu'une fois).
Un `ReplaceSelf` ne redéclenche aucun hook : pas de cascade, donc pas de
boucle possible entre deux règles.

**Refus au chargement.** Un hook, une condition ou un effet inconnu est
une variante d'enum inconnue : le parseur RON refuse déjà, et la nomme.
Une référence vers une entrée absente est refusée par la validation du
registre, comme pour `drops`.

**Clic droit.** Un bloc qui a une règle `Used` est *utilisé*, même si les
conditions sont fausses (sinon le même clic poserait ou utiliserait selon
ce qu'on tient — imprévisible). Maj + clic droit pose toujours.

**Contenu.** Une lampe (`core:lamp` ⇄ `core:lamp_lit`), sans une ligne de
Rust qui la connaisse. Comme elle n'existe pas dans le monde généré, une
touche de debug G donne un exemplaire de chaque bloc solide (découvert
dans le registre).

## 2026-10-08 (suite) — Jalon 3 : craft posé sur l'établi (#11)

**Changement de cap.** Le craft par proximité ne plaisait pas à Warshow.
Retenu : poser les items *sur* l'établi, puis fabriquer main vide. Pas de
menu, on voit ce qu'on fabrique, et surtout c'est le premier usage des
**block-entities** (§3.3), figées au doc depuis le début mais jamais
codées. #13 (craft dans le monde) est fusionné dans #11.

**Block-entities.** `BlockData::storage` (capacité) ; `VoxelWorld` garde
une map position → items, créée par `set_voxel` quand le bloc posé
déclare `storage`, supprimée quand le voxel change. Une seule map pour
tout le monde (commentaire `ponytail:`) : par chunk quand la save en aura
besoin. Le contenu d'un établi cassé tombe au sol (`Edit::set`).

**Recettes** (§3.1). `ContentEntry::recipes` sur l'entrée produite :
entrées sans ordre, quantité, station. `crafting::find` compare des
listes triées et prend la première recette dans l'ordre du registre
(déterministe). Références validées au chargement.

**Vocabulaire** (append-only). Conditions `EmptyHand`, `HoldingAny` ;
effets `StoreHeld`, `Craft` (produit si recette, sinon rend les items).
L'établi n'est qu'une entrée de `core.ron` qui combine ces mots.

**Main vide.** L'inventaire n'avait pas de main vide (toujours une pile
choisie) : une case est ajoutée après les piles. Ramasser en main vide
la garde vide, sauf le tout premier item.

**Piège évité.** Un produit apparaît au centre de l'établi, donc dans un
solide ; son petit saut (≈ 20 cm) ne l'en sort pas et il y reste coincé.
Un item qui naît dans un solide apparaît donc un voxel au-dessus.

**Tests.** Stockage (capacité, vidage, disparition avec le bloc),
recherche de recette (ordre, station, manque, surplus), conditions de
main, inventaire avec main vide. 56 tests, clippy propre.

## 2026-10-08 (suite) — Jalon 4 : textures (#21)

Choix de Warshow. Deux décisions avant de coder, écrites dans §3.1.

**Où vivent les pixels.** §3.1 veut l'apparence dans la save (world-owned).
Un chemin de fichier ne suffit pas : une save sans le PNG donnerait un
bloc invisible. L'entrée nomme donc un PNG (outil d'écriture), mais le
registre lit et garde les pixels (`Registry::load_textures`, crate `png`
déjà présente via Bevy) : c'est eux que la save stockera.

**Texture array plutôt qu'atlas.** Le greedy fusionne les faces : un quad
de 5 × 3 m doit répéter l'image 5 × 3 fois. Dans un atlas, répéter une
image déborderait sur les voisines ; dans un array, chaque couche est une
image à part que le sampler répète seul (mode `Repeat`). Contrainte : toutes
les textures ont la même taille, vérifié au chargement.

**Coordonnées en mètres.** Le mesher calcule les UV depuis la position du
sommet, en mètres : une image couvre 1 m quelle que soit la résolution
voxel. Sur les côtés, v descend avec y pour que l'image soit à l'endroit.

**Shader minimal.** Extension de `StandardMaterial` (éclairage de Bevy
conservé). La couche passe par le 2ᵉ jeu d'UV, que le shader standard
transmet déjà au fragment : pas de vertex shader à écrire. Piège WGSL :
`textureSample` est interdit dans une branche dépendant d'une valeur
interpolée — on échantillonne toujours et on choisit avec `select`.

**Vérification visuelle sous WSL.** `xwd` échoue sous WSLg ; une capture
via `Screenshot::primary_window()` de Bevy (code temporaire, retiré) a
suffi pour vérifier l'orientation avant le test Windows.

**Tests.** PNG des blocs livrés chargés, PNG manquant refusé, face
fusionnée 5 × 3 aux UV couvrant 5 × 3, bloc sans texture en couleur.
59 tests, clippy propre.

## 2026-10-08 (suite) — Jalon 5 : sauvegarde sur disque (#8)

Lève le non-goal « persistance disque » de §7. La structure logique était
figée depuis le début (§3.10) ; restaient deux décisions, écrites dans le
doc avant de coder.

**Rechargement du registre.** Les IDs viennent de la save (les chunks les
citent, ils ne bougent jamais). Puis `core.ron` est fusionné par
identifier : une entrée connue prend la définition du fichier (sinon
corriger une recette n'aurait aucun effet sur un monde existant), une
nouvelle est ajoutée à la fin, une entrée que seule la save connaît
(contenu généré en jeu, plus tard) est gardée avec ses pixels —
`load_textures` garde les pixels d'une texture sans PNG.

**Backend v1.** Un dossier par monde : `world.ron`, `registry.ron`,
`player.ron`, et `chunks/x_y_z.bin` pour les seuls chunks modifiés (les
autres se régénèrent depuis la seed). Le binaire est écrit à la main
(en-tête `VXC1`, palette, tableau dense u16, block-entities) : 64 Kio par
chunk, sans compression. `decode_chunk` refuse un fichier tronqué ou des
index hors palette.

**Quand écrire.** Un chunk dès qu'il change (`VoxelWorld::take_edited`,
marqué par `set_voxel`, `store`, `take_stored`) : les édits sont rares,
et on ne dépend pas d'une fermeture propre. Le joueur toutes les 5 s et
sur `AppExit` (système dans `Last`, après `ExitSystems`). Écritures
atomiques (temporaire puis renommage).

**Refuser plutôt qu'écraser.** Save d'une autre version, registre au
vocabulaire inconnu, chunk illisible : le jeu s'arrête avec un message.
Régénérer un chunk illisible l'aurait écrasé au prochain édit.

**Tests.** Aller-retour d'un chunk avec établi, chunk corrompu refusé,
métadonnées et joueur, registre (aller-retour, fusion, vocabulaire
inconnu), streaming qui relit un chunk sauvé. 65 tests, clippy propre.

## 2026-10-08 (suite) — Items au sol sauvés (complément du jalon 5)

**Un fichier, pas dans les chunks.** `items.ron` : la liste des items au
sol (entrée, position des pieds en mètres). Les ranger dans le fichier de
leur chunk aurait demandé de réécrire le chunk à chaque chute ou ramassage,
et les items ne se déchargent pas avec leur chunk (ils restent des
entités, juste figés) : une liste globale suffit. À revoir si les items
se comptent par milliers.

**Même rythme que le joueur** (toutes les 5 s et à la fermeture) : un item
bouge à chaque tick, l'écrire à chaque changement serait de l'écriture
continue. On perd au pire 5 s de chute ou un ramassage — qui est aussi
dans l'inventaire, sauvé au même moment, donc pas de duplication.

**Sans la vitesse.** Un item sauvé en l'air repart à l'arrêt et retombe :
invisible en pratique. Nouveau fichier optionnel (absent = aucun item) :
les saves existantes restent lisibles, `FORMAT_VERSION` ne bouge pas.
Illisible : erreur dans le log, items perdus, monde gardé (comme le
joueur).

## 2026-10-08 (suite) — Recettes sans station, touche C

**Le besoin.** L'établi ne s'obtenait que par G (debug) : toute recette
exigeait une station, et la seule station était l'établi lui-même.

**Mécanisme, pas règle de jeu.** Warshow veut un moteur qui ne fige pas une
manière de jouer (les joueurs feront leur propre jeu). Le moteur gagne donc
une capacité générale — une recette peut ne pas avoir de `station` (§3.1
le prévoyait : « station requise éventuelle ») — et la recette de l'établi
n'est qu'une ligne de `core.ron`. Un autre jeu peut n'en avoir aucune.

**Déclencheur : touche C.** Fabrique depuis l'inventaire la première
recette sans station faisable, dans l'ordre du registre (déterministe).
Pas de choix si plusieurs sont faisables : un menu le jour où ça gêne.

**Compatibilité.** `station` devient `Option<String>` ; le RON est lu avec
`IMPLICIT_SOME`, donc `station: "core:workbench"` s'écrit toujours sans
`Some(…)` et les `registry.ron` des saves existantes se relisent.

**Tests.** `craftable` (inventaire suffisant ou non, recette à station
exclue et inversement), `Inventory::remove` garde la sélection.

## 2026-10-08 (suite) — Fenêtres de jeu et menu de fabrication (#45, #46)

**Pourquoi les deux ensemble.** #45 est la base commune des fenêtres ; sans
fenêtre concrète, rien ne la prouve (règle d'or : le socle se prouve en
portant du concret). Le menu de fabrication (#46) est sa première fenêtre.

**Une fenêtre = une entité.** `ui::open` crée une racine `GameWindow` en
fermant celle déjà ouverte : une seule à la fois, pas de pile tant que
rien n'en a besoin. Pas de ressource « fenêtre ouverte » à tenir à jour :
l'existence de l'entité *est* l'état, impossible de désynchroniser les deux.

**La souris.** Le jeu avait déjà sa source de vérité, `CursorCaptured`
(mode FPS voulu). Une fenêtre ouverte le passe à faux dans `cursor_grab`,
et le clic ne recapture plus tant qu'elle est là — il sert à cliquer
dedans. Visée, pose/casse et molette lisaient déjà `CursorCaptured` ; le
déplacement non (on marchait souris libérée) : corrigé, la gravité
continue.

**Le menu.** Réflexif du registre (§3.11) : une ligne par recette sans
station (`crafting::without_station`), grisée si `inputs_from` dit que
les items manquent. Reconstruit à l'ouverture et quand l'inventaire
change, plutôt que synchronisé ligne par ligne (même choix que la barre).
Un clic est lu via `Interaction` de `bevy_ui` (composant `Button`).
`crafting::craftable` (« la première faisable ») disparaît : le joueur
choisit.

## 2026-10-08 (suite) — Recharger le contenu en cours de partie (#17)

**Le même chemin que la save.** Charger une save fusionnait déjà `core.ron`
dans le registre gardé (§3.10). Recharger, c'est la même fusion, dans le
registre courant : `content::load(base)` sert au démarrage (base = registre
de la save, ou vide pour un monde neuf — fusionner dans du vide redonne
l'ordre du fichier) et à la touche R. Une entrée retirée du fichier reste :
des chunks la citent peut-être.

**Tout ou rien.** La fusion se fait sur une copie du registre (`Registry`
devient `Clone`) ; on ne remplace l'original que si tout a réussi. Une
erreur (RON mal écrit, PNG absent ou de mauvaise taille) s'affiche dans le
HUD et la partie continue — au démarrage, la même erreur arrête le jeu.

**Ce qui dépend du registre.** Les couleurs et couches de texture sont
dans les sommets : tous les chunks affichés sont re-meshés (`DirtyChunks`,
le chemin habituel). Le texture array est remplacé dans son asset. Les
cubes d'items gardaient un matériau par entrée : désormais un par couleur,
et le rechargement marque les `ItemVisual` modifiés pour qu'ils se
rhabillent. Barre et menu se refont quand l'inventaire est marqué modifié.

**Pourquoi une touche plutôt que surveiller les fichiers.** Explicite, et
pas d'erreur affichée à chaque sauvegarde d'un fichier à moitié écrit.
Surveiller les fichiers reste possible plus tard.

## 2026-10-08 (suite) — Blocs générés par IA en jeu (#18, #44)

**La réponse de l'IA est une donnée non fiable.** Rien de ce qu'elle écrit
n'est exécuté : c'est du RON, lu avec les types du registre. Un hook ou un
effet inventé ne se lit pas (même refus qu'une save d'une version plus
récente, §3.6). Puis `accept` vérifie ce que les types ne disent pas :
identifier neuf sous `gen:` (une entrée générée ne peut pas remplacer
`core:stone`), références existantes (les contrôles de `from_ron`, sur une
copie du registre), textures nouvelles et dans leurs bornes. Un refus
s'affiche avec sa raison ; on ne demande pas au modèle de se corriger.

**Pourquoi un trait `Generator`.** Warshow veut tester avec Claude Code
maintenant et brancher une clé API plus tard. Le reste du moteur ne voit
qu'« un texte entre, un texte sort » : changer de source ne touche ni la
validation ni le jeu. L'appel prend plusieurs secondes ; il tourne dans un
thread, et le résultat arrive par un `OnceLock` que le jeu regarde à chaque
frame. `claude -p --tools "" --safe-mode` : le modèle ne peut que répondre,
ni commande, ni fichier, ni hook de la machine.

**Textures : des réglages, pas des pixels (#44, option 1).** Un modèle de
langage dessine mal en pixel art et 256 pixels coûtent cher à écrire.
L'entrée déclare un motif d'une liste connue, une couleur, un bruit, une
graine ; `texgen::paint` calcule les pixels (hash entier du worldgen, donc
déterministe). C'est un mécanisme général : `core.ron` peut s'en servir
aussi. Les pixels restent dans le registre et la save comme ceux d'un PNG.

**Déterminisme.** Le modèle ne l'est pas, mais il ne tourne pas dans la
simulation : une fois acceptée, l'entrée est une donnée comme une autre.

**Rien n'est donné au joueur.** Le bloc vit selon les règles du jeu : sa
recette (le prompt en demande une sans station), ou G en debug.

**Saisie de texte.** Premier champ texte (`ui::TextField`) : tant qu'il
est ouvert, les raccourcis (C, R, G, I, F) sont coupés, sinon taper « r »
rechargerait le contenu.

**Bug trouvé en jeu : tous les blocs générés en bois.** Le rechargement
remplaçait l'image du texture array sous le même handle ; le matériau
restait lié à l'ancien array. Une couche ajoutée (la 10e) sortait donc de
l'array et le GPU la ramenait à la dernière, `workbench_top`. La touche R
ne l'avait pas montré : elle ne changeait que des pixels, pas le nombre de
couches. Correctif : un nouveau handle, que Bevy attend avant de refaire le
matériau.

**Limite connue.** Une seule entrée par génération : un bloc qui bascule
entre deux états (comme la lampe) demanderait deux entrées.

## 2026-10-08 (suite) — Biomes, minerais, arbres (#27)

**Le worldgen nommait des blocs en Rust** (`core:grass`, `core:stone`) et
gardait ses réglages dans `main.rs` : contraire au principe §2. Il devient
un fichier de données, `worldgen.ron` ; le moteur fournit les étapes
(relief → surface du biome → minerais → features), le contenu choisit les
blocs et les nombres. `HeightmapGenerator` n'est pas gonflé : il garde le
relief et les grottes, le nouveau `Worldgen` s'en sert.

**World-owned (§3.10).** Seuls les chunks modifiés sont sauvés ; les autres
se régénèrent. Si les réglages du jeu changeaient sous un monde existant,
les zones visitées changeraient d'aspect. Les réglages sont donc copiés
dans la save à la création, comme la seed. Prix : pour voir une
modification, un monde neuf.

**Biomes.** Un bruit 2D lent (« climat ») découpé en plages. Le relief est
le même partout : un relief par biome demande de mélanger les hauteurs
aux frontières, reporté.

**Minerais.** Un bruit 3D par minerai, seuillé, entre deux profondeurs
sous la surface. Calcul voxel par voxel : rien ne dépend du voisin.

**Arbres qui débordent sur le chunk voisin.** Le vrai problème du ticket.
Chaque chunk examine aussi les colonnes d'une marge autour de lui (le
rayon du plus grand feuillage) ; un hash de la colonne décide s'il y pousse
une feature ; le chunk n'écrit que les voxels qui tombent chez lui. Aucun
état partagé, aucune dépendance à l'ordre de chargement. Le test le
prouve : un chunk 32³ est identique, voxel par voxel, aux huit 16³ qui le
couvrent.

**Une forme générale plutôt que « arbre ».** Une feature est une colonne
d'un bloc, avec en option une boule au sommet : arbre, cactus, rocher sont
trois lignes de données. Les structures en modèle de voxels attendront un
besoin.

**Spawn 8 m au-dessus du sol** : un arbre peut pousser à l'origine ; le
joueur tombe dessus au lieu de naître dans le tronc.

## 2026-10-21 — Blocs à trous (#22)

**`solid` faisait trois métiers** : collision, « à mesher », « cache la
face voisine ». Une feuille est solide (on marche dessus) mais ne cache
pas la terre derrière elle. Le troisième métier sort dans un champ à part,
`alpha`, nom repris de Bevy (`AlphaMode`).

**Mask seulement, pas Blend.** Mask : chaque pixel est plein ou vide, le
pixel vide est jeté dans le shader (`alpha_discard`, déjà là). Rien à
trier : le tampon de profondeur suffit, le bloc reste dans le mesh du
chunk. Blend (semi-transparent) mélange avec ce qui est derrière, donc il
faut dessiner de l'arrière vers l'avant : un mesh à part par chunk, trié
par distance. Seule l'eau en a besoin ; ça part avec #28.

**Règle de culling.** Une face est cachée si le voisin est opaque, ou si
c'est le même bloc à trous. La seconde moitié évite de dessiner chaque
face interne d'un feuillage ou d'un mur de verre (un feuillage de 5³ en
aurait 600 de plus). Prix : par un trou on voit à travers tout le
feuillage d'un coup, pas les feuilles de l'intérieur. Au bord d'un chunk,
le monde rend maintenant l'ID du voisin, plus seulement sa solidité.

**Un seul matériau en Mask.** Les blocs opaques passent aussi par le test
d'alpha ; leurs pixels valent 255, rien n'est jeté. Plus simple que deux
meshes par chunk ; à séparer si le profilage (#41) le montre.

**Textures.** `holes` sur une texture calculée : un tirage par pixel,
sauf sur les traits du motif (pixels assombris d'au moins 30 %). Feuilles :
`Spots` + 0,3. Verre : `Frame` + 1,0, seul le cadre reste.

## 2026-10-22 — Blocs qui évoluent seuls (#34)

**Premier jet : tick aléatoire seul** (à la Minecraft : quelques voxels
tirés au hasard par chunk et par tick). Warshow a demandé de ne pas
reprendre les défauts de Minecraft : rien ne pousse loin du joueur ni jeu
fermé, et la durée varie beaucoup (un plant sur vingt met plus de 3× la
moyenne). Comparaison faite (ticket #48 pour le détail) ; retenu :
**hybride**, deux hooks, le contenu choisit.

**`After(s)` : minuteur par bloc.** Une map position → minuteurs (la
vérité, filtrée par chunk à la save comme les block-entities) plus une
file triée par échéance (`BinaryHeap`) pour ne lire que les échus. Une
entrée de la file devenue fausse (bloc cassé) n'est jamais cherchée : elle
ne trouve plus rien dans la map à sa sortie. Le minuteur suivant d'une
chaîne compte depuis l'échéance du précédent : un chunk relu après une
heure rattrape graine → pousse → blé en deux ticks, au même état que si le
joueur était resté. Un cycle ne rattrape que `MAX_CATCH_UP` étapes, sinon
« toutes les 10 s » absent une journée ferait 8 640 étapes au rechargement.

**Pourquoi le worldgen refuse `After`.** Un minuteur coûte ~50 octets et
une écriture de save ; posé par le worldgen, le coût suivrait la surface
explorée. Le tick aléatoire n'a pas ce problème : coût fixe par chunk.

**Save.** Section des minuteurs en fin de fichier de chunk, optionnelle à
la lecture : les chunks d'avant se relisent, pas de `FORMAT_VERSION` à
incrémenter (même logique qu'un fichier optionnel). L'horloge du monde
(`clock.ron`) est écrite avec le joueur, toutes les 5 s : après un arrêt
brutal, les minuteurs ont au pire 5 s de retard.

**Hasard déterministe.** `Chance` tire dans un hash (seed, tick, voxel),
un tirage par règle. `TICKS_PER_SECOND` (64) devient une constante du cœur
et règle `Time<Fixed>` : les échéances sauvées en dépendent.

**`Above`.** Sans elle, la terre enterrée deviendrait de l'herbe. Premier
contenu qui lit le monde autour de son bloc ; le contexte d'une règle se
construit pour le voxel de la règle (le bloc posé, pas le bloc visé).


## 2026-10-25 — Liquides (#28)

Warshow : « on rend aussi les liquides dispo en données, comme cela on
pourra créer toute sorte de liquide avec différentes propriétés ». Avant
de coder, comparaison des options (détail dans #49) ; retenu ce qui suit.

**Source + écoulement, corrigé.** Le modèle de Minecraft (une source
infinie, le liquide perd un niveau par case) reste le bon compromis : le
volume conservé (Dwarf Fortress) ne se stabilise jamais et coûte en
permanence. Ce qu'on ne reprend pas : la distance en blocs (ici `spread`
en mètres, convertie à la résolution du monde), la vitesse unique
(`delay` par liquide), le pic de lag d'un lac qui se vide (plafond de
réexamens par tick, le surplus attend le tick suivant).

**Où vit le niveau.** Pas d'ID par niveau (le nombre d'IDs dépendrait de
la distance et de la résolution, changer `spread` décalerait les IDs d'une
save, les règles verraient `water#3`). Une table creuse position → niveau,
absente = source : un océan immobile n'y a aucune ligne. Même forme que
les block-entities et les minuteurs : une map pour tout le monde, filtrée
par chunk à la save.

**Chaque case se recalcule d'après ses voisins** (`liquids::update`), au
lieu qu'un liquide pousse dans les cases d'à côté. Conséquences : le
résultat ne dépend pas de l'ordre de traitement (testé : un réexamen par
tick donne le même état final), et le reflux n'est pas un cas à part. Une
case qui coule et a perdu sa source se fait d'abord nourrir par sa voisine
plus loin de la source, monte de niveau en niveau, puis disparaît : le
reflux prend `spread` pas, comme chez Minecraft.

**Un seul déclencheur : `set_voxel`.** Tout changement de voxel programme
le réexamen du voxel et de ses six voisins (une file triée par échéance,
comme les minuteurs). Un liquide qui arrive ou part les programme après
son `delay`, le reste au tick suivant. Un liquide réexaminé qui ne change
pas « réveille » l'air d'à côté et du dessous : c'est ce qui fait couler
une mare murée quand on casse le mur. Le même réexamen déclenche le hook
`NeighborChanged` : lave + eau → pierre est une règle de `core.ron`.
Les réexamens en attente et les niveaux sont sauvés avec le chunk (testé :
sauvé à mi-chemin, l'écoulement finit pareil).

**Rendu.** `alpha: Blend(opacité)` va dans un second mesh par chunk, sur
une entité à part : Bevy trie les objets semi-transparents par entité. Un
liquide est meshé sans être solide ; sous autre chose que lui-même, sa
surface est abaissée selon son niveau, et la hauteur entre dans la clé de
fusion du greedy. Un liquide ne cache que lui-même : la pierre sous l'eau
garde sa face.

**Joueur.** `drag` freine la marche et la gravité ; saut tenu = nager vers
le haut ; tête hors du liquide, c'est un vrai saut (sinon impossible de
remonter sur une berge d'un bloc).

**Worldgen.** `sea` optionnel : l'air sous un niveau devient le liquide
(des sources, immobiles tant que rien ne les touche). Pas d'arbre dont le
pied est sous la mer : le tronc dépassait de l'eau sans pied.

**Correctif après le test Windows (même jour).** Warshow : on ne voyait
que les dessus des liquides. Le mesher cachait toute face entre deux cases
du même liquide ; sur une pente, les dessus descendent en marches et rien
ne ferme les marches, et au bord l'eau n'a que quelques centimètres. Une
face de côté contre le même liquide est maintenant dessinée de la surface
du voisin à la sienne, quand le voisin est plus bas ; haut et bas entrent
dans la clé de fusion du greedy.

## 2026-10-25 — Coins assombris (#7)

Warshow craignait un système trop simple, loin du rendu des packs de
shaders Minecraft. Ces packs ajoutent des effets au rendu de l'image
(ombres du soleil, brouillard, bloom, reflets) et gardent l'ombrage des
coins de Minecraft de base : les deux se cumulent. Les effets ont leur
ticket (#51), après l'éclairage réel.

**Retenu : l'ombre par sommet, calculée au meshing.** Comparé à l'ombrage
calculé sur l'image par Bevy (coût à chaque image, dépend de l'angle de
vue) et à un calcul par pixel depuis une grille du chunk envoyée au GPU
(un matériau par chunk). C'est la seule option qui ne coûte rien au repos,
et un éclairage lissé par sommet (#23) lira les mêmes voisins. Détail :
#50.

**Le calcul.** Un sommet regarde, devant la face, ses deux voisins de côté
et celui du coin : 0 à 3 de lumière, deux côtés pleins ferment le coin.
L'ombre multiplie la couleur du sommet, pas d'attribut en plus. Fait de
l'ombre ce qui est solide et opaque ; rien de nouveau dans le registre.

**La fusion reste exacte.** Les ombres des quatre coins entrent dans la
clé du greedy. Comme l'ombre d'un coin ne dépend que de sa position, deux
faces voisines de même clé partagent leurs coins communs, donc l'ombre est
constante le long de la fusion : le rectangle fusionné la rend sans
l'étirer. L'oracle naïf calcule la même ombre ; le test vérifie chaque
sommet du greedy contre lui, et un mutant qui fusionne malgré des ombres
différentes casse deux tests.

**La diagonale.** Le GPU interpole la couleur dans chaque triangle. Coupé
au mauvais endroit, un coin sombre isolé tire une traînée jusqu'au coin
opposé : on coupe le long de la diagonale la plus claire.

**Le coût.** Sur `mesh_stats` (50 chunks denses) : 88 655 → 160 375 quads
(la réduction face au naïf passe de ×4,7 à ×2,6), greedy de 3,2 à 4,3 ms
par chunk. L'ombre d'une face dépend aussi des chunks en diagonale : un
chunk qui arrive re-meshe ses 26 voisins affichés au lieu de 6. Si le
streaming saccade sous Windows : une copie du chunk avec sa bordure
accélérerait le meshing (#50), et #41 dirait où part le temps.


## 2026-10-26 — Lumière du ciel et des blocs (#23)

Lève le non-goal « éclairage réel » de §7. Phase exploratoire présentée à
Warshow : quatre options comparées sur le coût par image, en pointe, la
mémoire, la save, ce qu'elles figent, et si elles permettent plus tard le
rendu des packs de shaders Minecraft. Warshow a suivi la reco. Écartées ou
reportées : #52.

**Retenu : la propagation de case en case, à la Minecraft, ciel et blocs
séparés.** Rien n'est calculé à chaque image (le PC de test n'a pas de
GPU), rien ne va dans la save, et la lumière est une fonction des voxels,
donc déterministe : une règle pourra la lire. Les packs de shaders
reçoivent de Minecraft exactement ces deux valeurs par sommet ; les
ombres du soleil (#51) s'ajouteront par-dessus. Les vraies lumières de
Bevy ont été écartées comme base : rien n'assombrit une grotte, et une
lampe éclaire à travers les murs sans ses ombres, qui coûtent cher.

**La propagation** (`voxel_core::light`). Une file : chaque case donne à
ses voisines son niveau moins un pas, et une voisine ne prend que si c'est
plus que ce qu'elle a. Le ciel plein descend sans perte. Pour éteindre, on
ne peut pas « retirer » une lumière : on éteint tout ce qui pouvait venir
de la case (les voisines plus sombres qu'elle d'un pas ou plus), puis on
repropage depuis les cases restées allumées au bord. Le test principal
fait mille poses et casses au hasard et compare, toutes les cent, à un
calcul repris de zéro ; il attrape un mutant qui oublie de repropager.

**En mètres et en quarts de case.** Une portée en mètres devient
`mètres × résolution × 4` niveaux : à 2 voxels par mètre, une lampe de
14 m éclaire toujours à 14 m. Le quart de case laisse `light_loss`
prendre des valeurs comme 0,5 (l'eau : on voit le fond jusqu'à ~10 m ; à
1,0 tout serait noir sous 8 m).

**Le ciel, avec des chunks cubiques.** On ne charge que ±48 m en hauteur.
Un chunk sans données au-dessus de lui est tenu pour sous le ciel ouvert ;
c'est donc au streaming de charger d'abord ce qui pourrait le couvrir. Le
générateur sait jusqu'où il pose des blocs (`Worldgen::top_m` : relief,
arbres, mer) : au-dessus, un chunk absent de la save est vide sans le
générer. Sous une montagne plus haute que la zone, les chunks du sommet
sont chargés en données, sans entité.

**Le rendu.** Le mesher lit la lumière des cases autour de chaque coin
(celles de l'ombrage, sans les opaques) et la met dans un attribut de
sommet à soi ; il a fallu écrire le vertex shader (celui de Bevy, plus la
lumière). Le ciel multiplie ce que voient le soleil et l'ambiance ; la
lumière des blocs s'ajoute à part, d'une teinte chaude unique. Le
jour/nuit (#30) baissera le ciel dans le shader, sans re-mesher. La
force de la lumière des blocs est réglée à l'estime : à vérifier à l'œil.

**Le coût.** `mesh_stats` : calcul de la lumière ~1,5 ms par chunk à
l'arrivée ; la fusion n'en souffre presque pas (161 617 quads dans le
monde). Le greedy isolé passe de ~4,3 à ~5,1 ms par chunk (lumière des
coins) ; dans le monde, avec le raccord et les deux passes, ~9,8 ms. À
mesurer sur le PC avec GPU (#41).

## 2026-10-28 — Profilage sur la RTX 4060 (#41)

**Le besoin.** Le PC de test n'a pas de GPU : ses mesures ne disaient
rien. Warshow allait tester sur son portable avec RTX 4060 ; le compteur
de FPS seul aurait dit « ça va » ou « ça rame », pas pourquoi.

**L'outillage.** Une feature `profile`, hors du build normal : la trace
de Bevy (`trace_chrome`, le temps de chaque système à chaque image) et,
toutes les 5 s dans la console, FPS, CPU, mémoire et le temps GPU de
chaque passe de rendu. Un lanceur par scénario, et un script qui résume
la trace. Premier écueil : à 144 FPS, la trace grossit d'environ 100 Mo
par seconde (5 Go en 20 s chez Warshow, impossible à ouvrir). Le lanceur
la zippe (~22× plus petit) et le script lit le zip, ligne à ligne, et
donne les systèmes de chaque seconde lente.

**Ce que la mesure a montré.** La carte ne force pas : la passe des blocs
prend 0,2 à 0,75 ms par image, et les 144 FPS sont la limite de l'écran
(vsync). Le problème est au chargement : 2 à 5 s à ~3 FPS, toutes dues à
`remesh_dirty`, qui meshe dans l'image tous les chunks sales, et chaque
nouveau chunk salit ses voisins. Une fois chargé, le monde ne coûte
presque rien. Une saccade isolée de 90 ms vient de l'écriture d'un chunk
sur le disque, elle aussi dans l'image. Deux tickets : #53, #54.

**Leçons.** Les FPS de l'exe de profilage ne sont pas une référence
(l'enregistrement coûte). Sur un écran 144 Hz, la vsync cache le coût d'un
effet : mesurer #51 au temps GPU par passe. Un portable se mesure branché.


## 2026-10-29 — Meshing hors de l'image (#53)

**Problème.** Au chargement, chaque chunk était meshé 5,9 fois en moyenne
(566 meshes pour 96 chunks, pire image 366 ms). On chargeait du plus
proche au plus loin, et chaque nouveau chunk faisait re-mesher ses voisins
déjà affichés, jusqu'à 26, pour recoudre bordures et ombres de coins.

**Décision de Warshow** : la meilleure option, pas la moins chère. Le jeu
vise un PC avec au moins une puce graphique ; le repère devient un écran à
144 Hz, 7 ms par image. Le PC sans GPU n'est plus une cible de perf.

**Ce qui a été fait.**
- Un chunk n'est meshé que quand ses voisins de la zone ont leurs
  données : une fois au chargement, plus une fois pour ceux du bord quand
  la zone avance. On ne génère pas d'anneau en plus, comme Minecraft :
  300 chunks de données pour 96 affichés.
- Le mesher lit une copie du chunk avec une case de bordure
  (`mesher::Region`) au lieu du monde. La copie se fait dans l'image
  (~0,5 ms), le meshing dans une tâche. Effet de bord : le mesh dans le
  monde passe de 8,7 à 5,0 ms, car l'ancien code résolvait le registre à
  chaque lecture de bordure.
- Les meshes lancés à la même image sont appliqués ensemble et dans
  l'ordre. Un bloc cassé à la frontière de deux chunks ne laisse jamais
  voir de trou. Minecraft évite ce trou en meshant les edits dans l'image,
  ce qui coûterait ~10 ms par clic ici.
- La génération passe aussi en tâches (une colonne par tâche). La lumière
  reste dans l'image car elle a besoin du monde : 1 chunk inséré par image.

**Résultat (PC de dev, headless, images cadencées à 144 Hz).** Au
chargement, la pire image passe de 366 ms à 6-8 ms. En course à 8 m/s, la
médiane est à 1 ms et 0 à 11 images sur 1440 dépassent 7 ms, selon les
passages. À re-mesurer sur la 4060.

**Leçons.**
- Compter les meshes avant d'optimiser un mesh : le gain ×6 venait
  d'éviter du travail, pas de le faire plus vite.
- Un coût mesuré au calme ne vaut pas en course. La copie passe de
  0,5 ms à 2 ms quand les données ne sont plus en cache ; ce n'est pas la
  concurrence entre threads (même résultat avec une seule tâche à la fois).

## 2026-10-30 — Sauvegarde des chunks hors de l'image (#54)

**Problème.** Sur la 4060, une image à 90 ms à cause de
`save_edited_chunks` : chaque pose ou casse écrivait le fichier du chunk
sur le thread principal.

**Mesure d'abord.** Encoder un chunk : 0,02 ms. Écrire son fichier :
0,25 ms sur le disque Linux, 12 ms sur le disque Windows vu de WSL. Tout
le coût est dans l'écriture, donc seule l'écriture sort de l'image.

**Ce qui a été fait.** Un fil dédié reçoit les chunks encodés par un
canal et les écrit dans l'ordre d'envoi : deux versions du même chunk ne
se croisent pas, la dernière gagne. Sur `AppExit`, le jeu ferme le canal
et attend le fil, sinon le dernier edit pourrait se perdre. Écarté :
regrouper les écritures toutes les N secondes (un plantage perdrait les
dernières secondes).

## 2026-10-30 — Effets de rendu (#51)

**Décision.** Warshow a suivi la reco de l'exploration du 2026-10-28 :
ombres du soleil, brouillard de distance, bloom. Une remarque a fait
reprendre le brouillard en volume et les rayons de lumière, que la reco
reportait pour leur coût : elle datait d'avant la nouvelle cible (la 4060 à
144 Hz, plus le PC sans GPU), et ce sont eux qui mènent le plus droit au
rendu des packs de shaders.

**Ce qui a été fait.**
- La passe de profondeur (qui sert aux ombres) cassait : notre
  `specialize` imposait nos emplacements d'attributs à toutes les passes.
  Il ne touche plus que la passe principale, et un fragment de prepass à
  nous découpe les trous des feuilles dans nos textures.
- Les faces d'une lampe recevaient la lumière de la case devant elles, à
  peine plus que le mur voisin : le mesher les met en pleine lumière de
  bloc, pour que le halo du bloom parte d'elles.
- Une touche par effet (F1 à F4), l'état dans le HUD : c'est l'outil de la
  mesure avant/après. Tout est coupé en rendu logiciel.

**Leçon.** Une reco se relit quand sa cible change. Celle-ci avait été
écrite pour un PC sans GPU ; personne ne l'avait recalculée pour la 4060.

**Limite.** Rien n'a été vu en vrai : en rendu logiciel le jeu tombe à
1 FPS avec les effets. Réglages et coûts restent à voir sur la 4060.

## 2026-10-30 — Jour et nuit (#30)

**Décision.** Exploration présentée, validée par Warshow telle quelle :
l'heure tirée de l'horloge du monde (déterministe, une règle pourra la
lire), une seule lumière pour le soleil et la lune, un ciel dessiné par
un shader à nous, ciel et lumières en donnée (`sky.ron`). Écartés : le
ciel physique `Atmosphere` de Bevy (réaliste mais pas stylisé, brouillard
qui ne raccorde pas à son dégradé, lux réels à refaire partout) et l'heure
de l'horloge murale (non déterministe). La météo sort de #30 : Warshow y
voit peut-être un cas d'événements du monde plus larges (#57).

**Pourquoi rien n'est re-meshé.** Depuis #23, chaque sommet porte la
lumière du ciel à part de celle des blocs, et le shader s'en sert pour
multiplier le soleil et l'ambiance. Baisser ces deux lumières suffit à
faire la nuit dehors, sans toucher aux grottes. Seul ajout au shader des
chunks : la lumière des blocs était masquée là où le ciel éclaire ;
elle l'est maintenant selon la force du jour, sinon une lampe n'aurait
rien éclairé dehors la nuit.

**Le fichier du jeu prime.** D'abord copié dans la save comme le
worldgen, `sky.ron` suit finalement la règle du registre : le fichier du
jeu gagne, la copie de la save ne sert que s'il manque. Une note d'un
agent annexe a pointé le piège : en réglant les couleurs sur la 4060, une
retouche n'aurait rien changé au monde existant. Le worldgen a besoin de
sa copie (un chunk doit se régénérer à l'identique) ; le ciel non.

**Ce qui a été vu.** Trois captures sous WSL, effets coupés : matin,
crépuscule, minuit. Le rendu suit `sky.ron`. Les étoiles, tirées par case
d'une grille, sortaient en éclats anguleux : la case découpée par la
sphère n'est pas un point. Un point rond au centre de la case règle ça.

**Limite.** Rien n'est vu avec les effets : réglages et tremblement des
ombres à voir sur la 4060.

## 2026-10-30 — Blocs à forme (#14)

**Décision.** Exploration présentée, Warshow a délégué (« je te fais
confiance »). Un bloc non cubique déclare des boîtes (`shape`), une
seconde liste pour la collision, et sa façon de se tourner à la pose
(`orient`). La rotation vit dans une table creuse, comme le niveau des
liquides. Écartés : un ID par orientation (à la Minecraft), un octet de
plus par voxel (à la Luanti), monter la résolution du monde, une grille
de voxels par entrée (#58).

**Pourquoi des seizièmes de case entiers.** Savoir si une boîte cache
la face voisine est une égalité exacte, et une boîte tournée d'un quart
de tour reste entière. En mètres, une forme déborderait sur plusieurs
cases dès 2 vox/m. La forme suit la case comme le cube la suit : à
2 vox/m, une dalle est une demi-brique d'un Lego plus fin. Ce qui est du
gameplay reste en mètres (hauteur de marche, portée, lumière).

**Pourquoi les 24 rotations plutôt que le sens.** Un octet dans les deux
cas. Les 24 se composent : tourner une construction entière (plus tard)
envoie chaque rotation sur une autre, sans cas propre à « escalier » ou
« bûche ».

**Avis extérieur.** Sur proposition de Warshow, la conception a été
soumise à DeepSeek V4.1 Flash (opencode). Retenu : boîtes en entiers,
nombre de boîtes borné, viser la forme et non la collision (une torche
qu'on traverse doit se casser). Écarté : borner la montée de marche en
cases, ce qui contredirait §2.

**Ce qui a été vu.** Tests headless : rotations (24 distinctes, sans
miroir), save, faces cachées, marche, visée. Une capture sous WSL, effets
coupés, avec un worldgen modifié hors du dépôt (colonnes d'escaliers et
boules de torches dans le désert) : formes et torches allumées rendent.

**Limites.** Pas de fusion des faces des blocs à forme (2 quads par dalle
de sol). Une dalle dans l'eau y fait un trou. La texture ne tourne pas
avec le bloc. La fusion de deux dalles attend un effet qui consomme
l'objet tenu (#58).
