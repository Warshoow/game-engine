//! Cœur voxel — logique pure, sans Bevy, testable headless.
//!
//! Source de vérité : `docs/brief/voxel-engine-design.md`.
//! - Le voxel canonique = un `material_id` seul (§3.2).
//! - Le registre est append-only, world-owned, à kinds unifiés (§3.1).
//! - Voxel-space ≠ world-space : le monde vit en mètres (§2).

pub mod chunk;
pub mod crafting;
pub mod generate;
pub mod light;
pub mod liquids;
pub mod mesher;
pub mod mob;
pub mod model;
pub mod path;
pub mod physics;
pub mod raycast;
pub mod registry;
pub mod rules;
pub mod save;
pub mod shape;
pub mod sky;
pub mod spawn;
pub mod texgen;
pub mod ticks;
pub mod worldgen;
pub mod world;
