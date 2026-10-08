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
- **Fait quand :** un bloc défini **uniquement en donnée** a un
  comportement (ex. une lampe qui bascule entre deux blocs quand on
  l'utilise), sans code Rust propre à ce bloc.

## 3. Craft par proximité

Choix de jeu : **pas de grille à la Minecraft.** Être à portée d'une
station débloque ses recettes dans un menu de craft.

- **Pourquoi :** le moins de code (une station n'a pas d'état, donc pas de
  block-entity ni de grille), la portée s'exprime en mètres (§2), et ça ne
  copie pas Minecraft.
- **Socle :** propriété `station` (type + portée en mètres) sur l'entrée
  du bloc ; recettes portées par l'entrée produite (§3.1).
- **Lève (en partie) :** « UI riche » — menu de craft.
- **Fait quand :** près d'un établi, le menu liste les recettes faisables
  avec l'inventaire, fabriquer consomme les entrées et ajoute la sortie ;
  loin, les recettes de l'établi disparaissent.

## Plus tard (non ordonné)

- **Script Lua** (`mlua`, étage 3 de §3.6) — quand un bloc concret ne
  s'exprime pas en règles. Lève « script runtime complet ».
- **Craft dans le monde** pour certaines stations (forge, poterie…) :
  poser les items sur le bloc, frapper avec un outil. Avec
  `voxels_per_meter` > 1, façonner l'objet voxel par voxel est une piste
  propre à ce moteur.
- **Modèles non cubiques** — à écrire dans le design doc (§3.1, forme de
  « l'apparence ») quand un contenu concret en aura besoin. Pistes :
  blocs = liste de boîtes (forme et collision) ou finesse via
  `voxels_per_meter` > 1 ; entités = petite grille voxel meshée par notre
  mesher, découpée en parties avec pivot pour l'animation (plutôt que du
  glTF, qui vivrait hors de la save). Taille du modèle en mètres,
  indépendante de la résolution du monde ; collision séparée du visuel.
- **Persistance disque** — ticket #8, à arbitrer.
- Distance de vue (#6), ambient occlusion (#7).
