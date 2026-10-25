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

**La tranche verticale (§7) est faite et dépassée** : les trois piliers du cœur voxel sont construits — stockage paletté, greedy meshing (raccord inter-chunks), streaming autour du joueur. L'état exact, les invariants à ne pas casser et les pistes ouvertes sont dans **`docs/passation.md`** (à lire en début de session) ; l'historique raisonné dans `docs/journal.md`.

On avance maintenant **jalon par jalon, arbitré avec Warshow** — propose, ne décide pas seul d'un gros chantier. L'ordre décidé des jalons est dans **`docs/jalons.md`**.

Les **non-goals** de §7 restent non-goals tant que non arbitrés : éclairage réel, smooth/densité, véhicules, script runtime complet, multi. Le data model les **prévoit** ; ne les code pas, même si l'occasion se présente. (Levés : persistance disque, jalon 5 ; UI riche, #45.)

**Principe (Warshow, 2026-10-08) :** le moteur ne fige pas une manière de jouer. Il fournit des mécanismes (règles, recettes, block-entities…) ; `assets/content/core.ron` n'est qu'un jeu d'exemple, remplaçable sans toucher au Rust. Un besoin de jeu se règle en donnée ; s'il n'y arrive pas, on ajoute un mécanisme général, jamais une règle propre à un contenu.

**Règle d'or :** le socle se prouve en portant du concret, pas en ajoutant une couche d'abstraction. Si une abstraction ne sert pas le jalon en cours, elle attend.

## Explorer avant de coder un mécanisme

Tout nouveau mécanisme (eau, minuteurs, lumière…) passe par une phase **exploratoire**, présentée à Warshow avant la première ligne de code :

1. Lire le ticket, le design doc et le code touché.
2. Voir large : la solution de référence (souvent Minecraft) avec ses défauts connus, puis 2 à 4 vraies alternatives, y compris des idées propres à ce moteur (mètres, données, déterminisme). Comparer chacune sur les mêmes critères : coût en perf (au repos et en pointe), coût en mémoire, effet sur la save, ce qu'elle fige.
3. Recommander une option. Dire ce qui en devient donnée de l'entrée : tout ce qui varie d'un contenu à l'autre se règle dans le registre, le moteur ne code que le mécanisme.
4. Warshow tranche. S'il délègue (« je te fais confiance »), chercher encore une fois large avant de partir sur la reco, et signaler toute meilleure idée trouvée.
5. Amender le design doc, ouvrir un ticket qui garde les options écartées ou reportées et leurs coûts (exemples : #48, #49), puis coder.

Fini quand : options comparées sur les mêmes critères, reco tranchée, ticket des options écartées créé. Exemples : #34 (minuteurs + tick aléatoire), #28 (liquides).

## Stack & conventions

- **Rust + Bevy 0.19** (ECS, plugin-first). L'API de Bevy bouge : vérifie contre la version du `Cargo.toml`, ne te fie pas à ta mémoire.
- **Le cœur voxel se construit à la main** (stockage de chunk, meshing, streaming) — c'est le but d'apprentissage du projet. Les crates voxel (`block_mesh`, `bevy_voxel_world`) servent de **référence**, pas de dépendance.
- Compile dev : garde `bevy/dynamic_linking` + `mold` actifs. Signale si un changement casse ça.
- La logique pure (registre, format de chunk, palette, worldgen, meshing) doit être **testable sans fenêtre** — `cargo test` headless.
- `cargo check` / `clippy` propres avant de considérer une étape terminée.

## Pédagogie

Ce projet est un projet d'apprentissage. Quand un concept est en jeu (meshing, layout mémoire, ECS, déterminisme) : **explique le raisonnement** — pourquoi cette approche, quels compromis. Pas de solution finie balancée sans le pourquoi.

## Commits

- **En anglais** à partir du 2026-10-08 (les plus anciens sont en français, on ne les réécrit pas). Conventional Commits, sujet + corps qui explique le pourquoi, `Closes #N` si un ticket est concerné.
- **Ne plus ajouter les mentions "Co-Authored-By:" dans aucun commit.**
- Code, commentaires et docs restent en français.

## Agent skills

### Issue tracker

Issues live in GitHub Issues (`Warshoow/game-engine`), via the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Default vocabulary: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context; the design doc `docs/brief/voxel-engine-design.md` stays canonical. See `docs/agents/domain.md`.
