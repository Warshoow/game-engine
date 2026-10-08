# Jalons après la tranche verticale

> Ordre décidé avec Warshow le 2026-10-07. Le design doc
> (`docs/brief/voxel-engine-design.md`) reste canonique pour le socle ; ce
> fichier porte les **choix de jeu** et l'ordre des chantiers. Chaque jalon
> dit quel non-goal de §7 il lève : on le lève au démarrage du jalon, pas
> avant.

## 1. Items et inventaire

Casser un bloc donne de quoi poser, poser le consomme. Sans ça, rien à
fabriquer. (Fait et validé en jeu le 2026-10-08, #9.)

- **Lève (en partie) :** « UI riche » — affichage de l'inventaire en
  `bevy_ui`, réflexif du registre (§3.11).
- **Fait quand :** casser ajoute l'item à l'inventaire, poser le retire,
  l'inventaire s'affiche ; la hotbar ne liste plus tous les blocs solides
  mais ce que le joueur possède.
- **Bloc ↔ item (décidé 2026-10-08) :** deux données indépendantes. Le
  bloc dit ce qu'il donne à la casse (souvent son propre item, mais pas
  forcément : rien pour du verre, une gemme pour un minerai) ; l'item dit
  quel bloc il pose, s'il en pose un (une pioche n'en pose pas).
  *Mise en œuvre :* un bloc se range lui-même dans l'inventaire (pas
  d'item « double » par bloc) ; `drops` absent = lui-même. `places` attend
  le premier vrai item (graines…) : aucun contenu n'en a besoin encore.
- **Ramassage (décidé 2026-10-08) :** un bloc cassé tombe au sol en entité
  « item au sol » (§3.1), qui retombe avec la collision du joueur
  (`move_and_collide`) et se ramasse à portée, en mètres. Rendu : un petit
  cube de la couleur du bloc, pas de modèle.

## 2. Hooks et règles

Étage 2 de §3.6 : premiers hooks (`utilisé` au clic droit, `posé`,
`cassé`), premières conditions et effets. `solid` reste une propriété.

- **Clic droit :** si le bloc visé a une règle sur `utilisé`, le clic
  droit l'utilise ; Maj + clic droit pose quand même.
- *Fait le 2026-10-08 (#10)* : vocabulaire `Used`/`Placed`/`Broken`,
  `Holding`, `ReplaceSelf`/`Drop` ; lampe dans `core.ron` ; touche G (debug)
  pour obtenir un bloc absent du monde généré.
- **Fait quand :** un bloc défini **uniquement en donnée** a un
  comportement (ex. une lampe qui bascule entre deux blocs quand on
  l'utilise), sans code Rust propre à ce bloc.

## 3. Craft posé sur l'établi

Choix de jeu (révisé le 2026-10-08, à la place du craft par proximité) :
**pas de menu.** On pose ses items *sur* l'établi (clic droit, item en
main), ils s'y affichent ; un clic droit main vide fabrique si ce qui est
posé correspond à une recette (le résultat tombe au sol), sinon rend les
items.

- **Pourquoi :** on voit ce qu'on fabrique, pas d'UI riche à lever, et
  c'est l'occasion de construire les **block-entities** (§3.3), figées au
  doc mais jamais codées : l'établi est le cas d'école d'un bloc à état.
- **Socle :**
  - block-entity : propriété `storage` (capacité en items) sur l'entrée du
    bloc ; l'état est créé à la pose, supprimé à la casse — son contenu
    tombe au sol (§3.3) ;
  - vocabulaire de règles (append-only) : conditions `EmptyHand`,
    `HoldingAny` ; effets `StoreHeld`, `Craft` ;
  - recettes portées par l'entrée produite (§3.1) : entrées (sans ordre),
    quantité, station.
- **Fait quand :** poser des items sur l'établi les affiche ; main vide,
  une recette valide produit le résultat, sinon les items reviennent ;
  casser l'établi rend son contenu ; tests headless du stockage et des
  recettes.
- *Fait et validé en jeu (Windows) le 2026-10-08 (#11)* : établi, lampe et sable fabricables ; case
  « main vide » ajoutée à l'inventaire (nécessaire à `EmptyHand`).
- La proximité et le menu (piste « à la Minecraft ») restent possibles
  plus tard pour d'autres stations.

## 4. Textures

Choix de Warshow (2026-10-08). Remplace les couleurs par sommet par des
textures déclarées dans l'entrée (§3.1, apparence).

- **Socle :**
  - l'entrée nomme ses textures (`side`, `top` et `bottom` en option) ; le
    registre charge les PNG de `assets/textures/` et garde les pixels ;
  - rendu par **texture array** (une pile d'images de même taille, chacune
    repérée par un numéro) plutôt qu'un atlas : le GPU répète une image
    seul, alors qu'avec un atlas la répétition déborderait sur les images
    voisines ;
  - coordonnées de texture en mètres, calculées par le mesher : une face
    fusionnée de 5 × 3 m répète la texture 5 × 3 fois au lieu de l'étirer ;
  - shader : extension de `StandardMaterial` (garde l'éclairage de Bevy).
- **Fait quand :** les blocs de `core.ron` sont texturés, l'herbe a un
  dessus et des côtés différents, un bloc sans texture garde sa couleur ;
  tests headless (PNG manquant refusé au chargement, coordonnées de
  texture d'une face fusionnée).
- Items au sol et barre d'inventaire gardent la couleur dans ce jalon.
- *Fait et validé en jeu (Windows) le 2026-10-08 (#21)* : 10 textures
  16 × 16 générées par script (à redessiner librement).

## Plus tard (non ordonné)

- **Script Lua** (`mlua`, étage 3 de §3.6) — quand un bloc concret ne
  s'exprime pas en règles. Lève « script runtime complet ».
- **Façonner voxel par voxel** sur une station (forge, poterie…) : avec
  `voxels_per_meter` > 1, une piste propre à ce moteur, après le jalon 3.
- **Modèles non cubiques** — à écrire dans le design doc (§3.1, forme de
  « l'apparence ») quand un contenu concret en aura besoin. Pistes :
  blocs = liste de boîtes (forme et collision) ou finesse via
  `voxels_per_meter` > 1 ; entités = petite grille voxel meshée par notre
  mesher, découpée en parties avec pivot pour l'animation (plutôt que du
  glTF, qui vivrait hors de la save). Taille du modèle en mètres,
  indépendante de la résolution du monde ; collision séparée du visuel.
- **Persistance disque** — ticket #8, à arbitrer.
- Distance de vue (#6), ambient occlusion (#7).
