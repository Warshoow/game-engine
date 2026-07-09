# Setup développement

> Environnement de référence : WSL2 (Ubuntu) sous Windows, affichage via WSLg (Wayland).
> Le design du socle vit dans [`brief/voxel-engine-design.md`](brief/voxel-engine-design.md) — à lire avant de coder.

## Prérequis système

Rust stable (via rustup), plus les paquets système suivants :

```bash
sudo apt install -y \
  build-essential clang mold \        # toolchain C + linker rapide
  pkg-config \
  libasound2-dev libudev-dev \        # audio / input (Bevy sous Linux)
  libwayland-dev libxkbcommon-dev     # fenêtrage (WSLg = Wayland, winit)
```

Pourquoi chacun (appris à la dure au bootstrap du projet) :

- **`build-essential`** — sans `cc`, rustc ne peut linker *aucun* binaire, même
  les tests d'un crate pur. Sous WSL, l'erreur est trompeuse : un `cc` Windows
  traîne dans le PATH et donne `Permission denied` au lieu de `not found`.
- **`clang` + `mold`** — le linker dev. Configuré dans `.cargo/config.toml` ;
  si mold n'est pas installé, **tout link échoue** (la config ne fait pas de
  fallback). Ne pas désactiver cette config sans le signaler.
- **`libwayland-dev` / `libxkbcommon-dev`** — headers de fenêtrage exigés par
  `winit` au *build* (pas seulement au run). WSLg expose un serveur Wayland.
- **`libasound2-dev` / `libudev-dev`** — dépendances Linux classiques de Bevy.

## Layout du workspace

```
Cargo.toml            # workspace + binaire `voxel_engine` (l'app Bevy)
src/main.rs           # l'app : uniquement du branchement, pas de logique
crates/voxel_core/    # cœur voxel PUR : aucune dépendance Bevy
  src/registry.rs     #   registre append-only, world-owned (§3.1)
  src/chunk.rs        #   chunk paletté : dense u16 + palette locale (§3.2)
  src/worldgen.rs     #   heightmap seedée, déterministe, en mètres (§2)
```

Règle de séparation : **toute logique testable sans fenêtre va dans
`voxel_core`** (`cargo test -p voxel_core` tourne headless, en ~0 s, sans
compiler Bevy). Le binaire racine ne fait que brancher cette logique dans
l'ECS/rendu de Bevy.

## Commandes courantes

```bash
cargo test -p voxel_core     # tests du cœur — rapide, headless
cargo clippy --workspace --all-targets   # doit être clean avant de conclure une étape
cargo run                    # l'app (dev : dynamic_linking actif par défaut)
cargo build --release --no-default-features   # release : SANS dynamic_linking
```

- La feature `dev` (par défaut) active `bevy/dynamic_linking` → itération
  rapide. Un binaire release doit être construit avec `--no-default-features`.
- Profils : `opt-level = 1` pour notre code en dev, `3` pour les dépendances
  (recommandation Bevy — les deps ne se recompilent qu'une fois).

## Pièges connus

- **Ne jamais conclure au succès d'un build via un pipeline** (`cargo build | tail`
  renvoie le code de sortie de `tail`). Vérifier `$?` de cargo, ou `echo EXIT_CODE=$?`.
- L'API Bevy bouge à chaque release : vérifier contre la version du
  `Cargo.toml` (0.19), pas contre sa mémoire ou un tutoriel daté.
