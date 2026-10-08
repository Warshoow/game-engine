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
