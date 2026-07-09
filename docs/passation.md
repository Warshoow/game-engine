# Passation de session — 2026-07-09

> Document de reprise pour une session Claude fraîche. Lire ceci, puis
> `CLAUDE.md` (racine) et `docs/brief/voxel-engine-design.md` (canonique)
> avant d'écrire du code. Le journal détaillé des décisions est dans
> `docs/journal.md`.

## Où en est le projet

**La tranche verticale (§7 du design doc) est fonctionnellement complète**
et validée en jeu par Joffrey. Dernier commit : `5259461`. Les quatre
critères tiennent :

1. Chunk généré depuis la seed (heightmap fBm maison, déterministe).
2. Mesher blocky (culling naïf) → mesh Bevy affiché.
3. Pose/casse data-driven (raycast DDA + écriture registre + re-mesh).
4. Déplacement FPS avec collision (AABB balayée, axe par axe).

`cargo run` → clic gauche pour jouer, WASD/Espace, clic gauche casse,
clic droit pose, Échap libère la souris, F11 plein écran.

## Architecture (résumé)

```
Cargo.toml            workspace + binaire voxel_engine (Bevy 0.19)
src/main.rs           setup : registre → worldgen → mesher → entités ; GameWorld (Resource)
src/player.rs         contrôleur FPS : simu en FixedUpdate, regard/curseur en Update
src/interact.rs       pose/casse : raycast depuis la caméra, re-mesh du chunk touché
crates/voxel_core/    TOUT le cœur voxel, PUR (zéro dépendance Bevy, testable headless)
  registry.rs         registre append-only §3.1 (IDs = index d'insertion)
  chunk.rs            chunk paletté §3.2 (dense u16 + palette locale)
  world.rs            VoxelWorld : registre + chunks + conversions voxel↔mètres
  worldgen.rs         trait WorldGenerator + HeightmapGenerator (value noise/fBm maison)
  mesher.rs           culling naïf → MeshData (tampons purs)
  physics.rs          AABB vs grille, région balayée (anti-tunneling)
  raycast.rs          DDA Amanatides & Woo (visée voxel + face d'entrée)
```

Règle de séparation stricte : logique voxel → `voxel_core` (35 tests
headless, ~0 s), le binaire ne fait que brancher dans l'ECS.

## Vérifications avant de conclure une étape

```bash
cargo test -p voxel_core                  # 35 tests, doivent passer
cargo clippy --workspace --all-targets    # zéro warning exigé
cargo run                                 # smoke test à l'occasion
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
  `mesa-vulkan-drivers` suggéré à Joffrey, non confirmé. FPS modeste = normal.
- Paquets système installés pendant le bootstrap : build-essential, clang,
  mold, pkg-config, libasound2-dev, libudev-dev, libwayland-dev,
  libxkbcommon-dev, libxkbcommon-x11-0. mold est actif via `.cargo/config.toml`.
- **Bevy 0.19** : vérifier l'API contre les exemples du crate installé
  (`~/.cargo/registry/src/*/bevy-0.19.0/examples/`), pas contre sa mémoire —
  la notation BSN est arrivée mais `commands.spawn` classique reste valable.
  Piège découvert : `Window::set_cursor_position` ignoré si la demande égale
  la précédente (cache bevy_winit).

## Conventions de travail avec Joffrey

- **Français** partout (code commenté en français, commits en français).
- **Jamais de trailer `Co-Authored-By`** dans les commits (demande explicite).
- Commits soignés et descriptifs, un par étape logique ; il apprécie qu'on
  commite les jalons sans redemander.
- **Pédagogie** : projet d'apprentissage — expliquer le *pourquoi* des
  concepts (meshing, layout mémoire, déterminisme…) dans les réponses ET
  dans `docs/journal.md`, tenu à jour à chaque étape.
- Le design doc est canonique : le modifier AVANT de coder toute entorse.

## Prochaines étapes

**Fait depuis** : le greedy meshing (étape 2 du mesher) est implémenté et
committé — `mesh_chunk` est greedy, le naïf reste comme oracle de test
(`mesh_chunk_naive`), gain mesuré ×12,8 (voir `docs/journal.md` et
`cargo run -p voxel_core --example mesh_stats`).

Candidat suivant discuté, non arbitré :
- **Sélection de blocs** — 2ᵉ bloc dans le registre + molette pour choisir
  ce qu'on pose (prouve le data-driven en action, rapide).

Pistes notées plus loin : surbrillance du voxel visé, interpolation caméra
entre ticks (si le 64 Hz se sent), HUD debug egui, **build Windows natif**
pour les tests de feel (proposé — demande mingw-w64, non mis en place).

Limitations assumées de la slice (ne pas « corriger » sans besoin) :
faces de bordure de chunk toujours émises (raccord viendra avec le
streaming), palette non compactée, re-mesh complet du chunk au moindre
voxel, un seul bloc posable, full-bright.
