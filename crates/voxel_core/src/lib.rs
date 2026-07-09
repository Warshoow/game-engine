//! Cœur voxel — logique pure, sans Bevy, testable headless.
//!
//! Source de vérité : `docs/brief/voxel-engine-design.md`.
//! - Le voxel canonique = un `material_id` seul (§3.2).
//! - Le registre est append-only, world-owned, à kinds unifiés (§3.1).
//! - Voxel-space ≠ world-space : le monde vit en mètres (§2).

pub mod chunk;
pub mod mesher;
pub mod physics;
pub mod raycast;
pub mod registry;
pub mod worldgen;
pub mod world;
