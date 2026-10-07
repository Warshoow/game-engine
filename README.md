# Voxel engine

A voxel engine built **by hand** in Rust on [Bevy 0.19](https://bevy.org), as a
learning project. The voxel core (chunk storage, meshing, streaming) is written
from scratch rather than pulled from a crate, because that is where the learning
is. Bevy provides the ECS loop, rendering and the window.

## What exists

- **Deterministic procedural terrain.** Hand-written fBm heightmap (value noise,
  SplitMix64 hash of `(seed, coordinates)`): same seed, same world, on any
  machine, with no state.
- **Palette chunks.** A dense array of `u16` indices plus a local palette mapping
  to an append-only content registry. The world owns its content; blocks are
  *data*, never code.
- **Greedy meshing.** Coplanar faces of the same material are merged (12.8× fewer
  quads than naive culling, which is kept as a test oracle), with seams handled
  across chunks so no hidden faces are left at the borders.
- **Streaming.** The world starts empty; chunks are generated and meshed around
  the player (per-frame budget, unload hysteresis), and edits survive unloading
  in memory.
- **FPS controller.** Fixed-tick simulation, swept AABB collision
  (no tunnelling), gameplay expressed in **metres**, never in blocks.
- **Data-driven place/break.** DDA raycast (Amanatides & Woo), and a hotbar
  *discovered* from the registry: adding a block to the registry is enough to
  make it placeable, no system to change.

## Run

```bash
cargo run
```

| Input | Action |
|---|---|
| Left click | enter FPS mode / break a block |
| Right click | place the held block |
| Mouse wheel | change the held block |
| WASD + Space | move / jump |
| F | fullscreen |
| Esc | release the mouse |

On WSL2, rendering goes through llvmpipe (CPU) and the mouse has its quirks
(see `docs/journal.md`, "La saga de la souris"). The game forces X11 and turns
off cursor recentring automatically.

### Native Windows build (from WSL)

To play with the GPU and the real Windows mouse:

```bash
sudo apt install mingw-w64                 # once
rustup target add x86_64-pc-windows-gnu    # once
cargo windows                              # alias from .cargo/config.toml
```

The executable lands in `target/x86_64-pc-windows-gnu/release/voxel_engine.exe`;
copy it to the Windows side (e.g. `/mnt/c/Users/<you>/`) and run it.

## Layout

```
src/                  Bevy binary: plugs the core into the ECS
  main.rs             setup (registry → world), mesh conversion, HUD
  player.rs           FPS controller (simulated in FixedUpdate)
  interact.rs         place/break + block selection
  streaming.rs        loading/unloading chunks around the player
crates/voxel_core/    the WHOLE voxel core: pure, no Bevy dependency
  registry.rs         append-only content registry
  chunk.rs            palette chunk
  world.rs            VoxelWorld (registry + chunks + voxel↔metre conversions)
  worldgen.rs         procedural generation (trait + fBm heightmap)
  mesher.rs           greedy meshing → plain buffers
  physics.rs          AABB collision against the grid
  raycast.rs          DDA (voxel picking)
docs/                 in French
  brief/voxel-engine-design.md   the design doc, the reference
  journal.md                     the reasoned history (the *why*)
  passation.md                   current state + invariants, to resume a session
```

The split is strict: all voxel logic lives in `voxel_core` and can be tested
without a window.

```bash
cargo test -p voxel_core                        # headless tests (~0 s)
cargo clippy --workspace --all-targets          # zero warnings
cargo run -p voxel_core --example mesh_stats    # naive vs greedy stats
```

## Principles (from the design doc)

1. Content is **data**, never hardcoded.
2. The **world owns its content** (embedded registry).
3. **Voxel space ≠ world space**: physics, entities and gameplay are in
   **metres**; the voxel resolution is only a conversion factor.
4. The **simulation is deterministic** (fixed tick, seeded).
5. **Freeze the data model, keep the rest soft.**
