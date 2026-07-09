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

**Prochaine étape.** Pose/casse data-driven (§7.3) : raycast voxel (DDA)
depuis la caméra, écriture via le registre, re-mesh du chunk touché — ça
fermera les critères de la slice. Ensuite : greedy meshing (étape 2 du
mesher) et l'interpolation caméra si besoin.
