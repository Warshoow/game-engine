# Moteur Voxel — Contexte projet

## Source de vérité

Le design du socle est spécifié dans **`docs/brief/voxel-engine-design.md`**.

**Lis ce document avant d'écrire du code.** Il est canonique.

Si une tâche demande de contredire une décision du doc : **modifie d'abord le doc** (décision + justification + coût de changement), puis code. Ne contourne jamais une décision en douce dans le code. Si le doc est ambigu ou silencieux sur un point qui bloque, demande — ne devine pas.

## Rappels non-négociables

(Détail et justification dans le doc — §0 et §2.)

1. Le contenu est de la **donnée**, jamais du code hardcodé.
2. Le **monde possède son propre contenu** (registre sérialisé dans la save).
3. **Voxel-space ≠ world-space** : le monde, la physique, les entités sont en **mètres**.
4. Le **gameplay s'exprime en mètres**, jamais en nombre de blocs.
5. La **simulation est déterministe** (tick fixe, seedé).
6. **Fige le data model, laisse le reste mou.**

Les décisions du data model (§3) sont **figées** — coût de changement Day-1.
Les zones de §4 sont **volontairement molles** : ne les sur-spécifie pas.
Les décisions de §5 sont **ouvertes** : suis les recos, signale si tu veux trancher autrement.

## Objectif courant

**La tranche verticale (§7).** Rien d'autre.

Générer un chunk → mesher blocky → poser/casser un voxel data-driven (défini via le registre, pas hardcodé) → s'y déplacer.

Les **non-goals** de §7 sont explicites : éclairage réel, smooth/densité, véhicules, script runtime complet, persistance disque, UI riche, multi. Le data model les **prévoit** ; la slice ne les **implémente pas**. Ne les code pas, même si l'occasion se présente.

**Règle d'or :** le socle se prouve en portant du concret, pas en ajoutant une couche d'abstraction. Si une abstraction ne sert pas la tranche verticale, elle attend.

## Stack & conventions

- **Rust + Bevy 0.19** (ECS, plugin-first). L'API de Bevy bouge : vérifie contre la version du `Cargo.toml`, ne te fie pas à ta mémoire.
- **Le cœur voxel se construit à la main** (stockage de chunk, meshing, streaming) — c'est le but d'apprentissage du projet. Les crates voxel (`block_mesh`, `bevy_voxel_world`) servent de **référence**, pas de dépendance.
- Compile dev : garde `bevy/dynamic_linking` + `mold` actifs. Signale si un changement casse ça.
- La logique pure (registre, format de chunk, palette, worldgen, meshing) doit être **testable sans fenêtre** — `cargo test` headless.
- `cargo check` / `clippy` propres avant de considérer une étape terminée.

## Pédagogie

Ce projet est un projet d'apprentissage. Quand un concept est en jeu (meshing, layout mémoire, ECS, déterminisme) : **explique le raisonnement** — pourquoi cette approche, quels compromis. Pas de solution finie balancée sans le pourquoi.