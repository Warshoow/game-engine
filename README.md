# Moteur Voxel

Un moteur voxel construit **à la main** en Rust + [Bevy 0.19](https://bevy.org),
comme projet d'apprentissage : le cœur voxel (stockage de chunk, meshing,
streaming) est écrit soi-même, pas délégué à une crate — c'est là qu'est
l'apprentissage. Bevy fournit la boucle ECS, le rendu et la fenêtre.

## Ce qui existe

- **Terrain procédural déterministe** — heightmap fBm maison (value noise,
  hash SplitMix64 de `(seed, coordonnées)`) : même seed → même monde,
  cross-machine, sans état.
- **Chunks palettés** — tableau dense d'indices `u16` + palette locale
  mappant vers un registre de contenu append-only (le monde possède son
  contenu ; les blocs sont de la *donnée*, jamais du code).
- **Greedy meshing** — fusion des faces coplanaires de même matériau
  (×12,8 de quads en moins vs le culling naïf, conservé comme oracle de
  test), avec raccord inter-chunks (pas de faces cachées aux frontières).
- **Streaming** — le monde démarre vide ; les chunks se génèrent et se
  meshent autour du joueur (budget par frame, hystérésis de déchargement),
  les modifications survivent en mémoire au déchargement.
- **Contrôleur FPS** — simulation à tick fixe, collision AABB balayée
  (anti-tunneling), gameplay exprimé en **mètres** (jamais en blocs).
- **Pose/casse data-driven** — raycast DDA (Amanatides & Woo), hotbar
  *découverte* depuis le registre : ajouter un bloc au registre suffit à le
  rendre posable, aucun système à modifier.

## Lancer

```bash
cargo run
```

| Entrée | Action |
|---|---|
| Clic gauche | entrer en mode FPS / casser un bloc |
| Clic droit | poser le bloc en main |
| Molette | changer le bloc en main |
| WASD + Espace | se déplacer / sauter |
| F | plein écran |
| Échap | libérer la souris |

Sous WSL2 : le rendu passe par llvmpipe (CPU) et la souris a ses
particularités (voir `docs/journal.md`, « La saga de la souris ») — le jeu
force X11 et désactive le recentrage curseur automatiquement.

## Architecture

```
src/                  binaire Bevy : branche le cœur dans l'ECS
  main.rs             setup (registre → monde), conversion mesh, HUD
  player.rs           contrôleur FPS (simu en FixedUpdate)
  interact.rs         pose/casse + sélection de bloc
  streaming.rs        chargement/déchargement des chunks autour du joueur
crates/voxel_core/    TOUT le cœur voxel — pur, zéro dépendance Bevy
  registry.rs         registre de contenu append-only
  chunk.rs            chunk paletté
  world.rs            VoxelWorld (registre + chunks + conversions voxel↔mètres)
  worldgen.rs         génération procédurale (trait + heightmap fBm)
  mesher.rs           greedy meshing → tampons purs
  physics.rs          collision AABB vs grille
  raycast.rs          DDA (visée voxel)
docs/
  brief/voxel-engine-design.md   le design doc — canonique
  journal.md                     l'historique raisonné (le *pourquoi*)
  passation.md                   état courant + invariants (reprise de session)
```

La séparation est stricte : toute la logique voxel vit dans `voxel_core`,
testable sans fenêtre.

```bash
cargo test -p voxel_core                  # tests headless (~0 s)
cargo clippy --workspace --all-targets    # zéro warning
cargo run -p voxel_core --example mesh_stats   # stats naïf vs greedy
```

## Principes (résumé du design doc)

1. Le contenu est de la **donnée**, jamais du code hardcodé.
2. Le **monde possède son contenu** (registre embarqué).
3. **Voxel-space ≠ world-space** : physique, entités et gameplay en
   **mètres** ; la résolution voxel n'est qu'un facteur de conversion.
4. La **simulation est déterministe** (tick fixe, seedé).
5. **Fige le data model, laisse le reste mou.**
