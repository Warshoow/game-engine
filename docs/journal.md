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
