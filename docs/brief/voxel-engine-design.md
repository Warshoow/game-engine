# Moteur Voxel — Design du Socle

> **Statut :** v0.1 — v0 figé pour le prototypage ; 2026-10-07 : recettes (§3.1), cycle de vie des block-entities (§3.3), forme du comportement (§3.6) ; 2026-10-08 : apparence (§3.1), persistance (§3.10), principe « mécanismes, pas règles de jeu » (§2), textures calculées (§3.1), génération en jeu (§3.6), réglages du worldgen dans la save (§3.10) ; 2026-10-21 : blocs à trous (§3.1) ; 2026-10-22 : blocs qui évoluent seuls (§3.6, §3.10) ; 2026-10-25 : liquides et semi-transparent (§3.1, §3.2, §3.6, §3.10), ombrage des coins (§3.8).
> **Nature :** ceci est un `intent/` — la source de vérité du socle. On lit **avant** d'écrire. Toute décision qui contredit ce doc doit d'abord **modifier ce doc** (avec sa justification), pas le contourner en douce dans le code.
> **Portée :** ce document décrit un **socle**, pas un jeu. Aucun système de gameplay (survie, magie, armes, quêtes…) n'est spécifié ici. Le socle est le substrat sur lequel ces systèmes se branchent sans toucher au core.

---

## 0. Intention & façon de lire ce doc

### Ce qu'on construit
Un socle voxel **modulaire, extensible, qui n'impose aucun cadre**. Un substrat sur lequel on construit à l'envie : survie *si on veut*, véhicules *si on veut*, magie *si on veut*. Le core ne connaît aucun de ces systèmes — il fournit les fondations qui les rendent tous possibles.

### Les non-négociables (invariants)
Tout le reste du doc en découle. Si une décision future viole un de ces points, c'est le doc qui a un problème, pas l'invariant.

1. **Le contenu est de la donnée, jamais du code hardcodé.** Un bloc, un item, une entité = une entrée de registre avec des composants attachés.
2. **Le monde possède son propre contenu.** La définition de ce qui existe vit *dans la save*, pas seulement dans le code.
3. **Le voxel-space est découplé du world-space.** Le monde, la physique, les entités vivent en **mètres**. La résolution voxel n'est qu'une densité.
4. **Le gameplay s'exprime en unités monde (mètres), jamais en nombre de blocs.**
5. **La simulation est déterministe** (tick fixe, seedée).
6. **On fige le data model, on laisse tout le reste mou.**

### Le garde-fou (à relire quand la tentation d'abstraire arrive)
Un socle se prouve en **portant un système concret**, pas en théorie. La cible immédiate n'est pas « l'architecture parfaite », c'est **une tranche verticale jouable** (§7). Toute abstraction qui ne sert pas cette tranche attend son tour.

### Comment lire les sections « figé »
Chaque décision du modèle de données porte un **coût de changement**. `Day-1` = ça structure le format/les types ; le changer plus tard = migration douloureuse ou resample complet du monde → on le décide **maintenant**. Le reste peut évoluer.

---

## 1. Stack technique

**Langage : Rust. Moteur : Bevy.**

Bevy est un moteur **ECS où tout est un plugin**, par philosophie — c'est l'incarnation la plus pure de « modulaire, extensible, sans imposer de cadre ». On est en **0.19** ; on assume le churn d'API à chaque release (migrations à prévoir) et les temps de compile.

**Répartition du travail :**

- **Bevy fournit** : la boucle ECS, le rendu, la fenêtre/input, le scheduling des systèmes, l'asset pipeline.
- **On construit nous-mêmes** : le **cœur voxel** (stockage de chunk, meshing, streaming). C'est *là* qu'est l'apprentissage — on ne délègue pas la partie qu'on veut comprendre. On s'appuie sur des algos de référence (greedy meshing) et on lit des crates comme `block_mesh` / `bevy_voxel_world` **comme référence**, pas comme dépendance clé-en-main.
- **Crates support** (branchées quand le besoin arrive, pas avant) : `bevy_ui` + `egui` (UI/debug), `mlua` (script — voir §5), un moteur physique (`avian`/`rapier`) plus tard.

> Rappel philosophie : on refuse la solution finie quand le but est de comprendre. Le meshing et le streaming se construisent à la main, avec de l'aide socratique si on bloque sur un concept — pas en copiant un moteur voxel entier.

---

## 2. Principes fondateurs

Les règles transverses dont dérive tout le modèle de données.

**Découplage voxel-space / world-space.** L'erreur naïve est de baker « 1 voxel = 1 mètre » partout. On ne le fait jamais. Les tailles (perso, entités, structures) sont en mètres. La résolution voxel est un facteur `voxels_per_meter` séparé. Conséquence directe : changer la résolution ne change **aucune** taille — juste la granularité du Lego.

**Gameplay en mètres.** Le rayon d'une torche = *X mètres*, pas *X blocs*. Sinon la sémantique du jeu se casse dès qu'on change la résolution. La résolution ne doit **jamais fuiter dans les règles du jeu**.

**Contenu data-driven & world-owned.** Voir §3.1. C'est le pivot entre « clone figé » et « socle à construire à l'envie » — et c'est ce qui rend possible la génération de contenu en runtime (y compris par IA).

**Le moteur fournit des mécanismes, le contenu fait le jeu** (ajout 2026-10-08, Warshow). Les fichiers de contenu livrés (`core.ron`) ne sont **qu'un jeu d'exemple** : un autre jeu les remplace sans toucher au Rust. Un besoin de jeu se règle d'abord en donnée ; si la donnée n'y suffit pas, on ajoute au moteur un **mécanisme général**, utilisable par n'importe quel contenu — jamais une règle propre à un bloc ou à une manière de jouer. Exemple : l'établi n'était pas fabricable (toute recette exigeait une station) → le moteur a gagné les recettes sans station, et la recette de l'établi est une ligne de contenu.

**Composition depuis primitives.** Le comportement se compose depuis un vocabulaire de briques (« émet de la lumière », « inflammable », « flotte »…), jamais depuis du code arbitraire. C'est ce qui rend la génération de contenu *safe* et cohérente par construction.

**Déterminisme.** La simu tourne en tick fixe, reproductible depuis la seed. Worldgen reproductible, saves cohérentes, porte multi/replay ouverte.

**Fige le data model, laisse le reste mou.** Le contrat de survie du projet. §3 est dur ; §4 est mou.

---

## 3. Modèle de données — **FIGÉ**

### 3.1 Registre de contenu

**Décision.** Un **registre unique, append-only, à kinds unifiés** (`block` / `item` / `entity_type`), **sérialisé dans la save**.

- **Append-only, pas frozen.** On interdit le *réordonnancement* et la *suppression*, pas l'ajout. Ajouter une entrée avec un ID neuf jamais réutilisé ne casse rien : les anciens IDs ne bougent pas, les vieilles saves restent valides. C'est une contrainte bien plus faible que le freeze de Minecraft, et elle suffit.
- **IDs entiers stables.** Chaque entrée a un ID entier stable pour la session. Les données voxel (palette) et la persistance référencent ces IDs. Le mapping `int ↔ identifier` ne change jamais après attribution.
- **World-owned.** La définition complète d'une entrée (composants, apparence, comportement) est sérialisée **dans la save**, à côté des données voxel. La save n'est plus « quels blocs sont où » mais « quels blocs *existent* dans ce monde **+** où ». C'est ce qui permet à un contenu généré en runtime (par une IA, plus tard) d'exister sans registre statique externe.
- **Kinds unifiés.** Bloc, item, mob, item au sol, véhicule : côté ECS ce sont tous des entités ; côté contenu, une seule table avec un champ `kind`. L'item et le bloc générés par IA sortent de la même table. Pas de registres séparés qui divergent.
- **Le modèle d'une entité est une donnée de l'entrée** (ajout 2026-10-30, #15). Une entrée `EntityType` déclare **`model`** : des **parties** (16 au plus), chacune avec un nom, un parent éventuel (déclaré avant elle), un **pivot** (le point autour duquel elle tourne) et des **boîtes** (64 en tout au plus). Unité : le **seizième de mètre**, en entiers, quelle que soit la résolution du monde (une entité vit en mètres, §2) ; à 16 pixels par texture, un seizième vaut un pixel, la densité des blocs à 1 vox/m. Chaque boîte nomme une texture du registre (PNG ou calculée), coordonnées en mètres comme les blocs. Le modèle se dessine pieds en y = 0, centré, regardant vers +Z. L'entrée déclare aussi sa **boîte de collision** en mètres (`size: (largeur, hauteur)`), séparée du visuel. Une entité prend la lumière de la grille à sa position (ciel et blocs, §3.8), pas seulement celle du soleil. Un item au sol issu d'un bloc se dessine comme ce bloc, en petit ; un autre item garde le cube coloré (un `model` d'item attendra un besoin). **Animation** : les parties tournent autour de leur pivot, d'un angle libre, à l'affichage seulement (hors simulation) ; ce qui les fait tourner viendra avec les mobs (#32) : une liste courte de mouvements, append-only (§3.6), choisis et réglés dans l'entrée (balancement selon la vitesse, tête vers la cible). Écartés : grille de voxels colorés (Veloren : hors du texture array, pénible à écrire pour une IA), grille de blocs du monde (format des véhicules, §3.7), glTF dans la save (opaque, ni générable ni lisible), poses clés (format de pistes, une IA en écrit mal). Détail : #59. *Coût de changement :* Day-1 pour l'unité, les parties et les pivots (les modèles sont dans les saves).
- **Les recettes sont une donnée de l'entrée qu'elles produisent** (ajout 2026-10-07). Une entrée peut porter zéro, une ou plusieurs recettes (entrées consommées, quantité produite, station requise éventuelle). Pas de kind `recipe` : une recette n'est pas une chose qui existe dans le monde, et la porter sur l'entrée produite rend un contenu généré **autonome** — une seule entrée à ajouter pour qu'un nouvel item existe *et* soit fabricable. Lister les recettes d'une station = parcourir le registre (même principe que la hotbar découverte).
- **L'apparence est une donnée de l'entrée, pixels compris** (ajout 2026-10-08). Une entrée de bloc nomme ses textures (une pour les côtés, et en option une pour le dessus et une pour le dessous). Le nom désigne un PNG de `assets/textures/` : c'est l'outil d'écriture, on dessine dans un vrai logiciel. Au chargement, le registre lit et garde **les pixels**, et c'est eux que la save stockera, pas le chemin : une save reste lisible sans les fichiers d'origine, et un contenu généré peut apporter ses propres pixels. Toutes les textures d'un registre ont la même taille (un texture array GPU l'exige). Une texture couvre **1 m** de surface, quelle que soit la résolution voxel (§2 : en mètres). `color` reste : couleur de secours sans texture, et couleur des items tant qu'ils n'ont pas d'apparence propre. Une texture peut aussi être **calculée par le moteur** au lieu d'être lue dans un PNG (ajout 2026-10-08, #44) : l'entrée déclare un motif d'une liste connue, une couleur, un niveau de bruit et une graine, et le moteur en tire les pixels (même graine → mêmes pixels). C'est ce qu'écrit une IA, qui dessine mal en pixel art ; un humain peut s'en servir aussi. La liste des motifs est append-only comme le vocabulaire (§3.6). Une entrée peut déclarer **`alpha: Mask`** (ajout 2026-10-21, #22) : ses pixels transparents laissent voir derrière (feuilles, verre), chaque pixel est plein ou vide. Le mesher ne cache plus une face derrière un tel bloc, sauf entre deux blocs identiques. Le semi-transparent **`alpha: Blend(opacité)`** (ajout 2026-10-25, #28) : ces blocs vont dans un second mesh par chunk, dessiné après les autres et trié par distance ; l'opacité multiplie celle des pixels. *Coût de changement :* Day-1 pour « les pixels sont dans le registre » ; le format d'image et la façon de déclarer les faces sont mous.

- **La forme est une donnée de l'entrée** (ajout 2026-10-30, #14). Un bloc qui n'est pas un cube plein (dalle, escalier, torche) déclare **`shape`** : une liste de boîtes, de 1 à 16, en **seizièmes de case** (entiers de 0 à 16, comme les modèles de blocs de Minecraft). La forme est relative à la case, pas en mètres : une dalle reste une demi-case quelle que soit la résolution, comme le cube reste une case ; en mètres, une forme déborderait sur plusieurs cases dès 2 vox/m (le cas des objets sur plusieurs cases reste à #15). Des entiers : les comparaisons (une boîte couvre-t-elle toute la face voisine ?) sont exactes, et une IA écrit « de 7 à 9 » plus sûrement qu'un flottant. **`collision`** est une seconde liste, la même par défaut ; une torche déclare `collision: []` : on la traverse, mais on la vise et on la casse quand même (le regard teste `shape`). La texture suit le modèle : couche (dessus, dessous, côté) et coordonnées (en mètres, prises sur la position) se calculent sur le modèle avant rotation ; le flanc d'une dalle montre la moitié de la texture, une bûche couchée a ses fibres dans son sens. Un cube plein qui déclare `orient` (bûche) passe par le même chemin une fois tourné (sans fusion de faces ni ombrage des coins) ; debout, il reste un cube ordinaire. **`orient`** dit comment le bloc se tourne à la pose, dans une liste append-only (§3.6) : `Fixed` (défaut), `Facing` (quatre sens selon le regard du joueur, à l'envers posé contre le haut d'une face ou sous un bloc : escalier), `Half` (seulement à l'envers : dalle), `Face` (le dessus du modèle pointe à l'opposé du bloc cliqué : bûche, torche). Un modèle se dessine pour un joueur qui regarde vers +Z. L'orientation vit dans une table creuse (§3.2). Écartés : un ID par orientation (à la Minecraft), monter la résolution du monde (×8 partout pour quelques dalles), une grille de voxels par entrée (format de #15, mais une torche fine demande du 16³). Détail : #58.

**Implémentation (piste).** Arène append-only, ou `Arc<RegistrySnapshot>` swappé en RCU à chaque ajout pour garder des lectures lock-free côté systèmes ECS.

**Coût de changement :** Day-1. Structure la save et le netcode futur.

### 3.2 Représentation du voxel & du chunk

**Décision.** Un voxel = **un `material_id`** (index dans une palette). Rien d'autre. Rendu **blocky** en premier.

- **Tableau dense paletté par chunk.** Chaque chunk stocke ses voxels comme un tableau dense d'indices dans une **palette locale au chunk** (les IDs globaux du registre sont mappés via la palette). La densité mémoire est le nerf de la guerre : chaque octet compte sur des milliards de voxels.
- **Pas de champ densité baké.** Le blocky n'en a pas besoin ; réserver un octet SDF partout serait payer un coût per-voxel jamais utilisé.
- **Format de chunk versionné.** La porte du smooth reste ouverte **sans payer maintenant** : la densité deviendra un **canal optionnel par chunk** ajouté dans une version future du format (exactement comme le canal creux des block-entities). Le blocky paie zéro, le smooth est un ajout purement additif.
- **La lumière ne fait PAS partie de l'identité du voxel** (voir §3.8).
- **Niveau des liquides** (ajout 2026-10-25, #28) : le voxel d'une eau qui coule reste `core:water` ; son niveau vit dans une **table creuse à part**, position → niveau, comme les block-entities (§3.3). Absent = source. Un lac ou un océan immobile n'y a donc aucune ligne ; seules les cases qui coulent (bords, cascades) coûtent quelques octets. Écartés : un ID par niveau (à la Minecraft : le nombre d'IDs dépendrait de la distance et de la résolution, changer la distance d'un liquide décalerait les IDs d'une save, et les règles verraient `water#3` au lieu de `water`) ; un octet par voxel dans les chunks à liquide (32 Kio par chunk d'océan). Détail et autres options : #49.
- **Orientation** (ajout 2026-10-30, #14) : comme le niveau des liquides, une **table creuse** position → rotation, une des 24 rotations du cube (le sens du dessus × un quart de tour autour de lui), un octet. Absent = le modèle tel que dessiné : un bloc non tourné ne coûte rien, le worldgen n'y écrit pas. Effacée quand le voxel change, sauvée avec le chunk. Les 24 plutôt que les seules orientations atteignables à la pose : tourner une construction entière (plus tard) envoie toute rotation sur une autre, sans cas par bloc. Écartés : un ID par orientation (à la Minecraft : les IDs se multiplient, un escalier en vaut 8, et les règles verraient `escalier#est` au lieu de `escalier`), un octet de plus par voxel (à la Luanti, `param2` : 32 Kio par chunk qui en contient un). Limite assumée : une case n'a qu'un bloc, donc une dalle posée dans l'eau fait un trou dans l'eau (pas d'équivalent du « waterlogged » de Minecraft).

**Coût de changement :** Day-1 pour « material_id only » et le versioning du format. Le mesher (blocky→smooth) est un ajout ultérieur grâce au versioning.

### 3.3 Block-entities (canal creux)

**Décision.** Un **canal séparé, creux**, `position → état riche`, pour les rares blocs qui ont une « âme ».

Un coffre a un inventaire ; un bloc-IA peut avoir de l'état. Cet état ne peut **pas** vivre dans le tableau voxel dense (ça ferait exploser la RAM). Comme Minecraft : tableau dense pour « quel bloc », map creuse pour l'état riche des rares blocs concernés. C'est aussi ce que lit une fenêtre d'inventaire (§3.11).

**Cycle de vie** (ajout 2026-10-07). L'entrée du registre déclare l'état initial de ses instances ; poser le bloc crée l'entrée creuse à sa position, le casser la supprime. Un bloc dont l'entrée ne déclare pas d'état n'a jamais de ligne dans le canal creux. *Premier état codé (2026-10-08, établi) : `storage`, une capacité en items ; le contenu tombe au sol quand le bloc disparaît.*

**Coût de changement :** Day-1. Le split dense/creux structure le format.

### 3.4 Coordonnées & taille de monde

**Décision.** Monde **fini**. Coords de chunk en **`i32`**. World-space en **`f32`**.

- **Fini par *policy*, pas par *type*.** Un `i32` de chunk × la taille de chunk donne des milliards de blocs d'amplitude — largement au-delà du bord. Le monde fini est délimité par un simple **check de bordure**, pas par la limite du type.
- **Taille recommandée : 8192 × 8192** blocs au sol (≈ 8,2 km de côté, ≈ 67 km²) — assez pour ne jamais voir le bord en jeu normal, avec de la marge. **Hauteur ≈ 512** (ex. −128 à 384) pour caves + hauteur de construction.
- **Précision float : non-problème.** À quelques km de l'origine, un `f32` donne une précision ~millimétrique ; l'imprécision ne devient sérieuse qu'aux *millions* de blocs. Donc **pas de floating origin, pas de coords locales par région** — world-space `f32` tout bête.
- **Streaming identique à l'infini.** On charge/décharge les chunks autour du joueur exactement comme un monde infini, avec juste des bornes. On apprend la *vraie* archi de streaming, pas une version au rabais.
- **Porte v2 (infini) quasi gratuite.** Passer à l'infini = relever le check de bordure + résoudre la précision float. **Les types ne changent pas.**

**Coût de changement :** Day-1 pour le type de coords. La taille exacte est un paramètre de création (voir plus bas).

### 3.5 Résolution (voxels par mètre)

**Décision.** `voxels_per_meter` est un **paramètre de création de monde**, gelé dans la save.

- Découle du découplage voxel/world (§2). Change la granularité, pas les tailles.
- **Coût cubique** — d'où « valeurs acceptables » : diviser l'arête par 2 = ×8 données/mémoire/meshing pour le même volume ; par 4 = ×64.
  - `1 vox/m` → MC classique.
  - `2 vox/m` (÷8 volume) → sweet spot raisonnable, plus granulaire.
  - `4 vox/m` → ça pique, à ne justifier que par le gameplay.
- **Gelé dans la save** : la résolution détermine *comment* toutes les données voxel sont stockées ; la changer en cours de partie = resample du monde entier. Donc figée à la création, immuable ensuite.

**Coût de changement :** Day-1 (métadonnée de monde immuable).

### 3.6 Comportement — couche script/data composable

**Décision.** Le comportement des blocs/items/entités vit dans une **couche script/data**, **jamais en Rust natif compilé**. Le contenu est produit en **composant des primitives**, pas en écrivant du code arbitraire.

- **Pourquoi pas du natif.** On ne compile pas du Rust au runtime pour le hot-loader. Si une IA (ou un moddeur) doit fabriquer un bloc en jeu, son comportement *doit* être du data ou du script.
- **Le moteur fournit des mécanismes, le contenu fait le jeu** (ajout 2026-10-08, Warshow). Les fichiers de contenu livrés (`core.ron`) ne sont **qu'un jeu d'exemple** : un autre jeu les remplace sans toucher au Rust. Un besoin de jeu se règle d'abord en donnée ; si la donnée n'y suffit pas, on ajoute au moteur un **mécanisme général**, utilisable par n'importe quel contenu — jamais une règle propre à un bloc ou à une manière de jouer. Exemple : l'établi n'était pas fabricable (toute recette exigeait une station) → le moteur a gagné les recettes sans station, et la recette de l'établi est une ligne de contenu.

**Composition depuis primitives.** L'IA/le moddeur assemble un vocabulaire de briques existantes (`emits_light`, `flammable`, `damage_on_contact`, `floats`…). Failure modes bien plus safe ; cohérence quasi gratuite, car tout est fait de morceaux qui respectent déjà les règles du monde.
- **C'est le levier n°1** de « construire à l'envie » **et** ce qui rend la génération runtime possible. Le socle modulaire pur et le rêve « IA qui génère du contenu in-game » sont **la même architecture**. Cette contrainte discipline le core dès maintenant.
- **Runtime précis : décision ouverte** (voir §5), mais la *forme* est figée : hooks événementiels + API capability-scoped.

**Forme précisée** (ajout 2026-10-07). Trois étages, du plus sûr au plus libre ; on ne monte d'un étage que quand un contenu concret ne s'exprime pas à l'étage du dessous.

1. **Propriétés** — des primitives sans logique, lues par les systèmes du moteur (`solid` aujourd'hui ; `emits_light`, `flammable`… plus tard).
2. **Règles « déclencheur → condition → effet »**, en donnée dans l'entrée. Le moteur code une fois chaque **hook** (utilisé, posé, cassé, tick, voisin modifié…), chaque condition et chaque effet (poser/retirer un voxel, faire apparaître un item, modifier l'état block-entity…) ; le contenu les combine. Des comportements que personne n'a codés naissent des combinaisons. C'est l'étage visé pour le contenu généré : une donnée se valide contre un schéma avant d'être acceptée.
3. **Script** — quand une règle ne suffit pas (boucle, calcul, état complexe). Le texte du script vit **dans l'entrée, donc dans la save**, et il est appelé sur les mêmes hooks. Garde-fous non négociables :
   - **API seulement** : le script ne voit que les fonctions que le moteur lui donne — ni fichiers, ni réseau, ni accès global au monde ;
   - **portée** : il ne lit/écrit qu'autour de son bloc, rayon en mètres ;
   - **budget** : nombre d'instructions borné par tick, coupé au-delà ;
   - **erreur isolée** : un script qui plante désactive ce comportement et le signale, le jeu continue ;
   - **déterminisme** (§2) : pas d'horloge ni de hasard libre — le hasard passe par l'API, dérivé de la seed.

**Le vocabulaire est un contrat avec la save, append-only comme le registre.** Hooks, propriétés, conditions, effets et fonctions de l'API de script ne sont jamais renommés ni supprimés, seulement ajoutés. Une save qui cite un élément inconnu de cette version du moteur est **refusée au chargement**, avec un message qui le nomme — l'ignorer changerait en silence le comportement du monde.

**Génération en jeu** (ajout 2026-10-08, #18). Le joueur décrit un bloc en texte dans une fenêtre ; un **générateur** (aujourd'hui la CLI `claude` sans outils, plus tard une clé API du joueur — une implémentation de plus, rien d'autre ne change) rend une entrée en RON. Cette réponse est une donnée **non fiable** : elle n'est acceptée qu'après validation par le moteur — lecture avec les types du registre (un élément de vocabulaire inconnu est refusé), identifier neuf sous le namespace `gen:`, références vers des entrées existantes, textures calculées (§3.1) dans leurs bornes. Acceptée, elle est ajoutée au registre comme n'importe quelle entrée, donc sauvée avec le monde. Le générateur n'est pas déterministe, mais il ne tourne pas dans la simulation : une fois acceptée, l'entrée n'est que de la donnée. Rien n'est donné au joueur : l'entrée vit selon les règles du jeu (ses recettes, l'outil de debug).

**Blocs qui évoluent seuls** (ajout 2026-10-22, #34). Deux hooks, deux coûts ; le contenu choisit :
- **`After(secondes)`** — minuteur par bloc. Il part quand le bloc apparaît, quelle qu'en soit la cause (pose, règle, autre minuteur) ; il s'arrête si le bloc change. Durée exacte, sauvé avec le chunk, et il tourne tant que le chunk est en mémoire ; au rechargement d'un chunk, les minuteurs échus s'appliquent, et le suivant part de l'échéance du précédent (pas du rechargement) : une culture a poussé pendant l'absence du joueur, au même stade que s'il était resté. Pour ce que le joueur suit : culture, feu, four. Coût : quelques dizaines d'octets par bloc en attente. **Le worldgen ne pose jamais de bloc à `After`** (refusé au chargement) : le coût suivrait la taille du monde exploré. Un cycle (un bloc qui revient à lui-même) ne rattrape qu'un nombre borné d'étapes, puis reprend au rythme normal.
- **`Tick`** — tick aléatoire : à chaque tick fixe, quelques voxels tirés au hasard par chunk chargé. Coût fixe par chunk, rien de sauvé, valable pour des blocs posés par milliers par le worldgen. Durée très variable, et rien ne bouge hors des chunks chargés. Pour ce qui se passe en fond : herbe, neige, mousse.

Le hasard des règles (condition `Chance`) dérive de la seed, du numéro de tick et du voxel (§2, déterminisme). Options écartées ou reportées (rattrapage du tick aléatoire, avance à heure fixe) : ticket #48.

**Liquides** (ajout 2026-10-25, #28). Un bloc devient liquide en déclarant `liquid` : distance d'écoulement à plat (`spread`, en mètres), temps d'un pas (`delay`, en secondes), `renewable` (deux sources voisines en créent une), freinage de ce qui le traverse (`drag`). Le moteur code l'écoulement une fois ; eau, lave ou autre ne sont que des réglages. Modèle source + écoulement : une source ne s'épuise pas, l'eau qui en coule perd un niveau par case, tombe d'abord, s'étale ensuite. Un liquide n'entre que dans l'air (le bloc vide du worldgen) : il n'efface jamais un bloc. Écartés : volume conservé (une étendue d'eau ne se stabilise jamais, coût permanent) et liquide statique (#49).

**Mises à jour de voisinage** (ajout 2026-10-25, #28). Quand un voxel change, lui et ses six voisins sont programmés pour être réexaminés : un liquide recalcule son état, un bloc à règle `NeighborChanged` la déclenche (avec la condition `Touching(bloc)` : lave + eau → pierre s'écrit en donnée). Un liquide programme ses voisins après son `delay` ; le reste au tick suivant. Rien n'est réexaminé là où rien ne change : un océan immobile ne coûte rien. Un nombre maximum de réexamens par tick borne les réactions en chaîne : le surplus attend le tick suivant, dans un ordre fixe (échéance puis position), donc toujours déterministe.

**La limite assumée :** un contenu, même scripté, ne fait que combiner ce que le moteur expose. Il invente de la logique, pas un nouveau rendu, un nouveau type de physique ou un nouveau genre de fenêtre. La surface du vocabulaire borne la créativité ; elle s'élargit quand un contenu concret en a besoin (§4).

**Coût de changement :** Day-1 pour le principe (comportement = data/script) et pour le contrat append-only du vocabulaire. Le runtime concret et la surface exacte du vocabulaire sont mous.

### 3.7 Entités & sous-grilles mobiles (véhicules)

**Décision.** Le monde **ne suppose pas une grille voxel globale unique**. Une structure mobile (véhicule, objet volant) est une **sous-grille de voxels détachée**, avec **son propre espace voxel local + origine locale + physique**.

- Les mods MC galèrent là-dessus depuis 10 ans (Create, Valkyrien Skies) précisément parce que le core y suppose une grille globale. On ne fait pas cette erreur : le modèle de données prévoit les sous-grilles **dès le départ** — ça ne se boulonne pas après.
- **Bonus précision :** chaque sous-grille a sa propre origine locale → ça *aide* la précision float au lieu de la casser.

**Coût de changement :** Day-1. L'hypothèse « grille unique » contamine tout si on la laisse s'installer.

### 3.8 Éclairage (invariant de data model)

**Décision.** La lumière **ne fait pas partie de l'identité canonique du voxel**. C'est une **couche dérivée / cachée par chunk**, pas une donnée d'identité.

- Le voxel canonique reste `material_id` seul (§3.2). Le niveau de lumière, s'il est stocké, l'est dans un **tableau parallèle par chunk**, dérivé et recalculable — jamais dans le tableau d'identité, jamais dans la save comme vérité (au pire un cache).
- **Ce qui est figé ici**, c'est cet invariant (« la lumière n'est pas de l'identité »). **L'algorithme reste ouvert** (voir §5) : flood-fill propagé façon MC vs. calcul différé/shader.

**Coût de changement :** Day-1 pour l'invariant (il protège la décision « material_id only »). L'algo est mou.

**Ombrage des coins** (ajout 2026-10-25, #7). Le mesher assombrit chaque sommet d'une face selon les blocs qui l'entourent, devant la face (trois voisins : deux côtés et le coin). Calculé à chaque meshing, jamais stocké ni sauvé : il suit l'invariant ci-dessus. Un bloc fait de l'ombre s'il est solide et opaque, ce que le registre dit déjà ; les liquides et le semi-transparent n'en reçoivent pas. Le greedy ne fusionne que des faces au même ombrage. Écartés : l'ombrage calculé sur l'image par Bevy (coût à chaque image, dépend de l'angle de vue ; reste cumulable plus tard), et le calcul par pixel depuis une grille du chunk envoyée au GPU (un matériau par chunk). Détail : #50.

**Lumière du ciel et des blocs** (ajout 2026-10-26, #23 ; lève le non-goal « éclairage réel » de §7). Propagation de case en case, à la Minecraft, sur deux lumières séparées par case : celle du ciel et celle des blocs.
- **Ciel** : pleine sous un ciel ouvert, elle descend sans perte tant que rien ne l'arrête, et perd en s'étalant sur le côté ou vers le haut. Sa portée sur le côté est une constante du moteur en mètres (15 m). Un chunk sans données au-dessus de lui est sous le ciel ouvert : le streaming charge d'abord ce qui est au-dessus, sauf si le générateur garantit qu'il n'y a rien (au-dessus de son altitude maximale, et pas dans la save).
- **Blocs** : un bloc qui déclare `emits_light` (portée en mètres) éclaire autour de lui, la lumière baisse jusqu'à zéro à cette distance.
- **Ce qui arrête la lumière** : un bloc solide et opaque, comme pour l'ombrage des coins. Un bloc peut déclarer `light_loss` : la distance traversée en lui compte en plus (1,0 = un mètre d'eau coûte deux mètres de portée) ; c'est ce qui assombrit la mer avec la profondeur.
- Les niveaux sont gardés en quarts de case (portée en mètres convertie à la résolution du monde, §3.5), un octet par lumière et par case, une seule valeur pour un chunk uniforme (air au soleil, roche pleine). **Jamais sauvés** : recalculés à l'arrivée d'un chunk et à chaque voxel modifié, dans la même frame (pas au tick suivant : une torche posée s'allume tout de suite). La lumière est une fonction des voxels, donc déterministe : une règle pourra la lire.
- **Rendu** : le mesher lit la lumière des mêmes cases que l'ombrage des coins (moyenne des cases non opaques autour du coin) et la met dans chaque sommet, ciel et blocs séparés. Le shader multiplie l'éclairage du soleil et de l'ambiance par la lumière du ciel et ajoute celle des blocs, d'une teinte chaude unique. Le jour/nuit (#30) baisse le soleil et l'ambiance que multiplie la lumière du ciel, sans re-mesher. Deux faces ne fusionnent que si leurs coins ont la même lumière.

**Blocs à forme** (ajout 2026-10-30, #14) : un bloc qui déclare `shape` laisse passer la lumière et ne fait pas d'ombre aux coins ; ses faces prennent la lumière de sa propre case, sans ombrage. Il cache la face d'un voisin seulement si une de ses boîtes (tournée) couvre toute la face commune. Ses faces ne fusionnent pas (greedy) : deux quads par dalle de sol.

Écartés ou reportés : lumière lue par pixel (texture 3D par chunk), vraies lumières de Bevy (gardées pour le soleil, #51), ciel par hauteur seule, couleur par bloc, éclairage par lancer de rayons, lumière sauvée avec le chunk. Détail et coûts : #52.

### 3.9 Simulation — tick fixe / frame variable

**Décision.** Logique de jeu en **tick fixe déterministe** (`FixedUpdate`, seedé) ; rendu en **frame variable**.

- Physique, ticks de bloc, IA de mob → tick fixe. Rendu, interpolation visuelle → frame variable.
- Offre : worldgen reproductible, saves cohérentes, porte multi/replay ouverte pour la v2 **sans re-architecturer**.
- Ce n'est pas du code en plus, c'est **où on range quoi** — mais ça se décide avant d'écrire la simu.

**Coût de changement :** Day-1 (discipline gratuite maintenant, cauchemar à rétrofitter).

**Heure du jour** (ajout 2026-10-30, #30). L'heure n'est pas un état de plus : elle se calcule depuis l'horloge du monde (§3.10, point 7), l'heure de départ et la durée d'une journée, données de `sky.ron` (§3.10, point 9). Elle est donc déterministe : une règle pourra la lire. Pour avancer ou reculer l'heure (dormir, commande de console), un décalage sauvé s'ajoutera ; aujourd'hui seule une touche de debug la décale, sans la sauver. Reportés : #57.

### 3.10 Persistance (structure logique)

**Décision.** La **structure logique** de la save est figée ; le **backend de stockage** est mou (§4).

Contenu logique d'une save :
1. **Données voxel** (chunks : palette + tableau dense).
2. **Snapshot du registre** (world-owned, append-only — §3.1).
3. **Block-entities** (canal creux — §3.3).
4. **Entités** (état ECS persistant).
5. **Métadonnées de monde** : seed, `voxels_per_meter`, taille de monde, version de format.
6. **Réglages du worldgen** (ajout 2026-10-08, #27) : relief, biomes, minerais, features, en donnée (`assets/content/worldgen.ron`). Copiés dans la save à la création du monde, puis la copie de la save prime : un chunk jamais modifié n'est pas sauvé mais régénéré, et il doit se régénérer à l'identique — comme la seed. Changer le worldgen du jeu ne touche que les mondes neufs.
7. **Horloge du monde et minuteurs** (ajout 2026-10-22, #34) : le numéro du tick fixe courant, et pour chaque chunk ses minuteurs en attente (position, échéance en ticks, règle). Le tick fixe vaut 1/64 s, constante du moteur : changer ce pas change le sens des échéances sauvées.
8. **Liquides** (ajout 2026-10-25, #28) : par chunk, le niveau des cases qui coulent et les réexamens programmés (position, échéance). Sans eux, une eau sauvée en plein écoulement resterait figée au rechargement.
9. **Ciel et jour** (ajout 2026-10-30, #30) : durée d'une journée, heure de départ, et pour quelques moments de la journée les couleurs du ciel et la force des lumières, en donnée (`assets/content/sky.ron`). Comme pour le registre, le fichier du jeu prime : une retouche s'applique aux mondes existants (changer la durée du jour ne fait que décaler l'heure, rien n'a à se régénérer à l'identique). La save en garde une copie, utilisée seulement si le jeu n'a pas de `sky.ron` : un monde garde son ciel. Un autre jeu (deux soleils, un ciel rouge) se fait sans toucher au Rust.

**Coût de changement :** Day-1 pour la structure. Le moteur (region-files vs `redb`/`sled`) est un choix ultérieur.

**Rechargement du registre** (ajout 2026-10-08, jalon 5). Au chargement d'une save, les **IDs viennent de la save** et ne bougent jamais. Les fichiers de contenu du jeu (`core.ron`) sont ensuite fusionnés par identifier : une entrée qu'ils déclarent prend **leur** définition (corriger une recette ou une texture s'applique aux mondes existants) ; une entrée nouvelle est ajoutée avec un ID neuf ; une entrée que seule la save connaît (contenu généré en jeu) est gardée telle quelle, pixels compris. Une save qui contient un élément de vocabulaire inconnu est refusée (§3.6), jamais écrasée. *Coût de changement :* Day-1 (fixe ce qu'un monde garde quand le jeu évolue).

**Backend v1** (mou, 2026-10-08). Un dossier par monde : métadonnées et registre en RON, un fichier binaire par chunk **modifié** (palette + tableau dense + ses block-entities, en-tête versionné). Un chunk jamais modifié n'est pas écrit : il se régénère depuis la seed. Un chunk est écrit dès qu'il change : encodé dans l'image, son fichier écrit par un fil à part qui garde l'ordre (ajout 2026-10-30, #54 : l'écriture coûtait jusqu'à 90 ms par image sous Windows) ; la fermeture attend les écritures en cours. Joueur et inventaire périodiquement et à la fermeture.

### 3.11 UI réflexive du registre

**Décision.** L'UI est **réflexive du registre** : un slot d'inventaire tire la def de son item (icône, nom, taille de stack) du **même registre world-owned**.

- Sinon, dès qu'une IA génère un item, l'UI ne sait pas l'afficher. L'UI lit le registre comme tout le reste.
- L'inventaire d'un coffre = de l'état **block-entity** (§3.3) qu'une fenêtre lit.
- **Outillage :** `bevy_ui` (retained) pour HUD / inventaires / fenêtres de quête ; `egui` pour le debug/tooling.
- **Ce qui est mou :** thème, widgets, layout (§4).

**Coût de changement :** seul le principe « UI lit le registre » est Day-1. Les internals sont mous.

---

## 4. Zones volontairement molles

On ne les fige **pas**. On fige seulement la *forme* indiquée, et on laisse le contenu émerger en construisant.

| Zone | Forme figée | Ce qui reste libre |
|---|---|---|
| **Internals UI** | l'UI lit le registre (§3.11) | thème, widgets, layout |
| **API de script/mod** | hooks événementiels + API capability-scoped ; propriétés → règles → script (§3.6) ; vocabulaire append-only | la surface exacte (se découvre à l'usage) |
| **Worldgen** | pipeline *pluggable* ; réglages en donnée, world-owned (§3.10) | richesse : noise → biome → features peut rester bête au début |
| **Backend de persistance** | structure logique de la save (§3.10) | moteur : region-files vs `redb`/`sled` |
| **Multijoueur** | déterminisme préserve la porte (§3.9) | **tout** — c'est de la v2 |

**Principe :** fige le data model, laisse tout le reste mou.

---

## 5. Décisions encore ouvertes

À trancher, mais qui ne bloquent pas le démarrage de la tranche verticale.

**Runtime de script — `mlua` (Lua) vs. wasm (`wasmtime`/`extism`).**
- *Lua/`mlua`* : embarquement simple, excellent DX, hot-reload trivial. Idéal v1, surtout avec l'approche « composition de primitives » (l'IA sort de la donnée qui compose des briques safe, donc pas besoin de sandbox lourd contre du code hostile).
- *Wasm* : sandboxé, multi-langage, la réponse « écosystème de mods sérieux / untrusted ». Plus lourd.
- **Reco : `mlua` en v1**, wasm comme chemin de graduation si un jour on ouvre les mods à du code tiers non-confiance.

**Algorithme d'éclairage.** L'invariant est figé (§3.8), pas l'algo. **Reco : différer** — full-bright (ou skylight trivial) pour la tranche verticale, vrai éclairage ensuite. *Tranché le 2026-10-26 (#23) : propagation de case en case, ciel et blocs séparés, voir §3.8.*

**Dimensions de chunk.** **Reco : 32³** comme défaut (le format stocke ses propres dims → ajustable). Compromis draw-calls / coût de meshing.

**Largeur d'index de palette.** **Reco : `u16`** par voxel (65k matériaux/chunk, large) pour démarrer ; largeur adaptative (1/2/4/8/16 bits façon paletted containers MC) = optimisation mémoire ultérieure.

---

## 6. Esquisse d'architecture

```mermaid
flowchart TB
    subgraph World["Monde (source de vérité — sérialisé dans la save)"]
        Meta["Métadonnées<br/>seed · voxels_per_meter · taille · version format"]
        Registry["Registre de contenu<br/>append-only · kinds unifiés<br/>(block / item / entity_type)"]
    end

    subgraph Storage["Stockage voxel"]
        Chunks["Chunks<br/>palette locale + tableau dense (material_id)<br/>+ canal densité (option future)"]
        BE["Block-entities<br/>canal creux : position → état riche"]
        Light["Lumière<br/>couche dérivée / cache par chunk"]
        SubGrids["Sous-grilles mobiles<br/>véhicules : espace + origine locaux"]
    end

    subgraph Sim["Simulation — tick fixe déterministe"]
        ECS["ECS (Bevy)<br/>entités + systèmes"]
        Behavior["Comportement<br/>couche script/data · primitives composables"]
    end

    subgraph Render["Rendu — frame variable"]
        Mesher["Mesher blocky (greedy)<br/>→ mesh Bevy"]
        UI["UI réflexive du registre<br/>HUD · inventaires · quêtes"]
    end

    Registry -->|IDs stables| Chunks
    Registry -->|defs| Behavior
    Registry -->|defs item| UI
    Chunks --> Mesher
    Chunks --> Light
    Light --> Mesher
    BE --> UI
    ECS --> Behavior
    Behavior -->|hooks| Registry
    SubGrids --> Mesher
    Meta -.->|paramètre| Chunks
```

**Lecture du flux :** le **Monde** possède ses métadonnées et son **registre**. Le registre attribue les IDs stables que les **chunks** stockent (via palette) et dont les fenêtres d'**UI** tirent les defs d'item. Le **comportement** (script/data) lit les defs du registre et s'y branche par hooks. La **simu** tourne en tick fixe ; le **rendu** (meshing + UI) en frame variable. Les **sous-grilles mobiles** et la **lumière** alimentent le mesher sans polluer l'identité du voxel.

**Meshing hors de l'image** (ajout 2026-10-29, #53).
- **Cible.** Le joueur visé a au moins une puce graphique ; le repère est un écran à 144 Hz, soit 7 ms par image. Un mesh coûte ~10 ms : il ne tient pas dans l'image.
- **Tâches en parallèle.** Le meshing tourne dans des tâches, sur une copie du chunk et d'une case de bordure autour (blocs, lumière, hauteur des liquides). L'image ne fait que la copie et l'envoi du mesh au GPU.
- **Application groupée.** Les meshes lancés dans la même image sont appliqués ensemble, dans l'ordre de lancement. Deux chunks voisins changent donc à la même image : pas de trou entre eux quand on casse un bloc à leur frontière.
- **Voisins d'abord.** Le streaming ne meshe un chunk qu'une fois les données de ses voisins de la zone chargée connues. Chaque chunk est meshé une fois au chargement (5,9 fois avant). Ceux du bord le sont une fois de plus quand la zone avance.
- **Écartés** : budget de meshes seul (le travail reste), anneau de chunks générés autour de la zone (×3 de données), mesh sur le GPU. Détail : #55.

**Effets de rendu** (ajout 2026-10-30, #51). Ce sont des réglages de la machine, jamais de la save ni du registre.
- **Ombres du soleil.** Une `DirectionalLight` avec cartes d'ombre en cascades jusqu'à la distance de vue. La lumière du ciel du sommet (§3.8) multiplie toujours le soleil : pas de soleil dans une grotte, même au-delà de la carte d'ombre. La passe de profondeur (qui sert aux ombres) a son shader à nous : les trous des blocs `Mask` y sont découpés, les feuilles projettent une ombre trouée. Le semi-transparent (eau) ne projette pas d'ombre.
- **Brouillard de distance**, couleur du ciel, qui finit à la distance de vue : cache le bord du monde chargé.
- **Brouillard en volume et rayons de lumière** (`VolumetricFog`) : un léger voile dans un volume qui suit la caméra, éclairé par le soleil à travers sa carte d'ombre. Le plus cher des effets (64 pas par pixel) ; gardé parce que la cible est la 4060 (7 ms par image, passe opaque à 0,2–0,75 ms) et qu'il mène au rendu des packs de shaders.
- **Bloom** (halo des lumières fortes), qui demande le rendu HDR. Les faces d'un bloc à `emits_light` sont en pleine lumière de bloc : le halo part de la lampe, pas du mur voisin.
- **Une touche par effet**, l'état dans le HUD de debug : sert à la mesure avant/après. Défaut : tout activé, sauf sur un rendu logiciel (pas de carte graphique).
- **Reportés ou écartés** : SSAO et ombres de contact (doublons de l'ombrage des coins), reflets sur l'eau, ciel physique `Atmosphere` (voir #30 ci-dessous), ombres par rayons, réglages sauvés, brouillard sous l'eau. Détail : #56.

**Jour et nuit** (ajout 2026-10-30, #30). L'heure vient de l'horloge du monde (§3.9) ; tout le reste se lit dans `sky.ron`, interpolé entre deux moments de la journée.
- **Soleil et lune** : une seule `DirectionalLight`. Elle suit le soleil de 6 h à 18 h, la lune (à l'opposé) le reste du temps : une seule carte d'ombre, la lune a ses ombres. Sa couleur et sa force, et celle de l'ambiance, viennent du moment de la journée. Rien n'est re-meshé : le ciel du sommet (§3.8) multiplie déjà ces lumières.
- **Lampes la nuit** : la lumière des blocs n'est ajoutée que là où le ciel n'éclaire pas déjà, en tenant compte de la force du jour (un uniform du matériau des chunks). Une lampe éclaire donc dehors la nuit, et son halo (bloom) s'y voit.
- **Ciel** : un shader à nous sur une grande sphère qui suit la caméra. Dégradé de l'horizon au zénith, disque du soleil (assez fort pour le bloom), lune, étoiles qui apparaissent quand le jour baisse. Le brouillard prend la couleur de l'horizon : pas de raccord visible au bord du monde chargé.
- **Debug** : une touche avance l'heure, l'heure s'affiche dans le HUD.
- **Écartés ou reportés** : heure de l'horloge murale (non déterministe), ciel `Atmosphere` de Bevy (physique, brouillard qui ne raccorde pas, lux réels), météo (peut-être un cas d'événements du monde lisibles par les règles), phases de la lune, condition de règle sur la lumière ou l'heure, soleil tourné par pas. Détail : #57.

---

## 7. La tranche verticale — définition de « fait »

Le premier livrable qui **prouve que les fondations tiennent**. Pas une couche d'abstraction de plus : du jouable.

**Critères d'acceptation :**
1. **Générer un chunk** depuis la seed (worldgen bête : un noise → sol/air suffit).
2. **Le mesher blocky** (greedy) transforme le chunk en mesh Bevy affiché.
3. **Poser / casser un voxel data-driven** : au moins **un vrai bloc** défini via le registre world-owned (pas hardcodé), avec sa palette qui se met à jour et le chunk re-meshé.
4. **Se déplacer** dessus : caméra + collision basique.

**Non-goals explicites de la tranche** (à ne PAS faire maintenant, pour ne pas déraper) :
- éclairage réel (full-bright OK) — *levé le 2026-10-26 par #23 (§3.8)* ;
- smooth / densité ;
- sous-grilles mobiles / véhicules ;
- script runtime complet (le « un vrai bloc » peut passer par une def data minimale, la couche script vient juste après) ;
- persistance disque (in-memory OK pour la slice) — *levé le 2026-10-08 par le jalon 5 (#8)* ;
- UI riche (un HUD debug egui suffit) — *levé en partie le 2026-10-08 par le jalon 1 (#9) : barre d'inventaire `bevy_ui`, réflexive du registre ; levé le 2026-10-08 par #45 : fenêtres de jeu en `bevy_ui`, toujours réflexives du registre (§3.11)* ;
- multi.

Ces non-goals sont *prévus par le data model* (§3) mais *pas implémentés* dans la slice. C'est ça, un socle qui tient : le format les accueille, la slice ne les code pas encore.

---

## 8. Journal des décisions

| Sujet | Décision | Statut | Coût de changement |
|---|---|---|---|
| Stack | Rust + Bevy (ECS, plugin-first) | Figé | — |
| Cœur voxel | Construit main (learning) ; crates en référence | Figé | — |
| Registre | Append-only, kinds unifiés, world-owned, dans la save | Figé | Day-1 |
| Voxel | `material_id` seul, palette locale, tableau dense | Figé | Day-1 |
| Format chunk | Versionné (densité = canal optionnel futur) | Figé | Day-1 |
| Block-entities | Canal creux `position → état` | Figé | Day-1 |
| Coords | Fini par policy, chunk `i32`, world `f32` | Figé | Day-1 |
| Taille monde | 8192² × 512 (reco), param de création gelé | Figé | Day-1 |
| Résolution | `voxels_per_meter`, param de création gelé | Figé | Day-1 |
| Gameplay | En mètres, jamais en blocs | Figé | — |
| Comportement | Couche script/data, primitives composables | Figé | Day-1 |
| Recettes | Donnée de l'entrée produite, pas de kind dédié | Figé (2026-10-07) | Day-1 |
| Apparence | Textures nommées par l'entrée, pixels gardés dans le registre (donc dans la save), 1 texture = 1 m | Figé (2026-10-08) | Day-1 |
| Forme et orientation | `shape` (boîtes en seizièmes de case) et `collision` dans l'entrée ; `orient` à la pose ; rotation (24) dans une table creuse comme les niveaux d'eau (#14) | Figé (2026-10-30) | Day-1 |
| Modèles d'entités | `model` de l'entrée : parties avec parent et pivot, boîtes en seizièmes de mètre texturées comme les blocs ; collision en mètres à part ; lumière de la grille ; animation par mouvements en donnée avec #32 (#15) | Figé (2026-10-30) | Day-1 |
| Block-entities (vie) | État initial déclaré par l'entrée ; créé à la pose, supprimé à la casse | Figé (2026-10-07) | Day-1 |
| Forme du comportement | Propriétés → règles déclencheur/condition/effet → script ; garde-fous script | Figé (2026-10-07) | Day-1 (forme) |
| Vocabulaire | Append-only ; save à élément inconnu refusée | Figé (2026-10-07) | Day-1 |
| Sous-grilles mobiles | Prévues au data model, espace/origine locaux | Figé | Day-1 |
| Éclairage | Pas dans l'identité voxel (couche dérivée) | Figé (invariant) | Day-1 |
| Simulation | Tick fixe déterministe / frame variable | Figé | Day-1 |
| Persistance | Structure logique figée ; backend mou | Figé (structure) | Day-1 |
| Rechargement du registre | IDs de la save ; définitions des fichiers du jeu par identifier ; entrées de la save seule gardées | Figé (2026-10-08) | Day-1 |
| UI | Réflexive du registre | Figé (principe) | — |
| Runtime script | `mlua` v1 (reco), wasm en graduation | **Ouvert** | — |
| Algo éclairage | Différé, full-bright pour la slice | **Ouvert** | — |
| Meshing | Tâches en parallèle sur une copie avec bordure ; un chunk meshé quand ses voisins sont chargés (#53) | Mou (2026-10-29) | faible |
| Effets de rendu | Ombres du soleil, brouillard de distance et en volume, bloom ; une touche par effet (#51) | Mou (2026-10-30) | faible |
| Jour et nuit | Heure tirée de l'horloge du monde ; ciel et lumières en donnée (le fichier du jeu prime, copie dans la save) ; soleil et lune sur une lumière ; ciel par shader à nous (#30) | Mou (2026-10-30) | faible (rendu) ; `sky.ron` dans la save |
| Dims chunk | 32³ (reco) | **Ouvert** | faible (format porte ses dims) |
| Index palette | `u16` (reco), adaptatif plus tard | **Ouvert** | faible |
| Internals UI / API mod / worldgen / backend / multi | Laissés mous | **Mou** | — |

---

## Glossaire

- **Voxel** — un point/cube de la grille 3D ; ici, un index de palette (`material_id`).
- **Chunk** — bloc de voxels (32³ par défaut) unité de stockage, meshing et streaming.
- **Palette** — table locale au chunk mappant les indices locaux vers les IDs globaux du registre (compression mémoire).
- **Block-entity** — bloc à état riche (coffre, machine) stocké dans un canal creux séparé.
- **Registre** — table de contenu append-only, world-owned, à kinds unifiés.
- **Kind** — catégorie d'entrée du registre : `block`, `item`, `entity_type`.
- **Mesher** — l'algo qui transforme les données voxel en mesh rendable (greedy pour le blocky).
- **Greedy meshing** — fusion des faces voxel coplanaires adjacentes pour réduire les triangles.
- **Densité / SDF** — champ scalaire permettant une surface *smooth* (marching cubes / dual contouring / Transvoxel). Canal optionnel futur.
- **Tick fixe** — pas de simulation à cadence constante, déterministe depuis la seed.
- **Floating origin** — technique (non nécessaire ici) recentrant le monde pour préserver la précision float loin de l'origine.
- **Hook** — événement du moteur auquel un comportement se branche (bloc utilisé, posé, cassé, tick…).
- **Primitive** — brique de comportement codée dans le moteur et composée en donnée par le contenu : propriété, condition ou effet.
- **Règle** — « quand (hook) / si (conditions) / faire (effets) », en donnée dans une entrée du registre.
- **Capability-scoped** — API dont chaque capacité est explicitement accordée (principe de moindre privilège pour les scripts/mods).