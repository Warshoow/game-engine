//! Item au sol (§3.1 : une entité, kind unifié) — jalon 1, #9.
//!
//! Un bloc cassé fait apparaître ses drops ici. L'item tombe avec la même
//! collision que le joueur (`move_and_collide`) et se ramasse à portée. Tout
//! cela est de la simulation (l'inventaire change) : tick fixe (§3.9).

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use voxel_core::physics::{move_and_collide, Aabb};
use voxel_core::registry::ContentId;

use crate::inventory::{self, Inventory};
use crate::player::{Player, GRAVITY_M_S2};
use crate::GameWorld;

/// Côté du cube affiché et de sa boîte de collision, en mètres.
const ITEM_SIZE_M: f32 = 0.25;
/// Distance (mètres) entre l'item et le centre du joueur pour le ramasser.
const PICKUP_RANGE_M: f32 = 1.5;
/// Petit saut à l'apparition, pour que l'item « sorte » du bloc cassé.
const POP_SPEED_M_S: f32 = 3.0;

/// Un cube d'item affiché, de la couleur de son entrée : item au sol, ou
/// item posé sur un bloc (établi).
#[derive(Component)]
pub struct ItemVisual(pub ContentId);

#[derive(Component)]
#[require(Transform)]
pub struct DroppedItem {
    pub content: ContentId,
    feet: Vec3,
    velocity: Vec3,
}

impl DroppedItem {
    pub fn new(content: ContentId, center_m: Vec3) -> Self {
        Self {
            content,
            feet: center_m - Vec3::Y * ITEM_SIZE_M / 2.0,
            velocity: Vec3::Y * POP_SPEED_M_S,
        }
    }
}

/// Gravité + collision, puis ramassage. Tick fixe.
pub fn simulate_items(
    mut commands: Commands,
    time: Res<Time>,
    game: Res<GameWorld>,
    mut inventory: ResMut<Inventory>,
    player: Query<&Player>,
    mut items: Query<(Entity, &mut DroppedItem)>,
) {
    let dt = time.delta_secs();
    let player_center = player
        .single()
        .ok()
        .map(|p| p.feet() + Vec3::Y * crate::player::PLAYER_HEIGHT_M / 2.0);

    for (entity, mut item) in &mut items {
        if player_center.is_some_and(|c| c.distance(item.feet) <= PICKUP_RANGE_M) {
            inventory.add(item.content);
            commands.entity(entity).despawn();
            continue;
        }
        // Comme le joueur : on ne simule pas dans un chunk non chargé.
        let (chunk, _) = game.world.split(game.world.voxel_at_m(item.feet.to_array()));
        if game.world.chunk(chunk).is_none() {
            continue;
        }
        item.velocity.y -= GRAVITY_M_S2 * dt;
        let aabb = Aabb::from_feet(item.feet.to_array(), ITEM_SIZE_M, ITEM_SIZE_M);
        let moved = move_and_collide(&game.world, aabb, (item.velocity * dt).to_array());
        for axis in 0..3 {
            if moved.collided[axis] {
                item.velocity[axis] = 0.0;
            }
        }
        item.feet = Vec3::from_array(moved.aabb.feet());
    }
}

/// Habille un item fraîchement apparu : un cube de la couleur de son
/// entrée. Mesh et matériaux partagés (un matériau par entrée).
pub fn add_item_visuals(
    mut commands: Commands,
    game: Res<GameWorld>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut cube: Local<Option<Handle<Mesh>>>,
    mut material_of: Local<HashMap<ContentId, Handle<StandardMaterial>>>,
    added: Query<(Entity, &ItemVisual), Added<ItemVisual>>,
) {
    for (entity, &ItemVisual(content)) in &added {
        let cube = cube
            .get_or_insert_with(|| meshes.add(Cuboid::from_length(ITEM_SIZE_M)))
            .clone();
        let material = material_of
            .entry(content)
            .or_insert_with(|| materials.add(inventory::color(&game.world.registry, content)))
            .clone();
        commands.entity(entity).insert((Mesh3d(cube), MeshMaterial3d(material)));
    }
}

/// Blocs dont le contenu posé (block-entity, §3.3) a changé : leurs cubes
/// sont à refaire.
#[derive(Resource, Default)]
pub struct StorageChanged(pub HashSet<[i64; 3]>);

/// Cube d'un item posé sur le bloc `.0`.
#[derive(Component)]
pub struct StoredVisual([i64; 3]);

/// Refait les cubes posés sur les blocs notés dans [`StorageChanged`] :
/// une grille 3 × 3 sur la face du dessus, puis une couche au-dessus.
// ponytail: les cubes restent affichés si le chunk est déchargé ; à lier au
// streaming si ça se voit.
pub fn show_stored(
    mut commands: Commands,
    game: Res<GameWorld>,
    mut changed: ResMut<StorageChanged>,
    visuals: Query<(Entity, &StoredVisual)>,
) {
    if changed.0.is_empty() {
        return;
    }
    for (entity, StoredVisual(v)) in &visuals {
        if changed.0.contains(v) {
            commands.entity(entity).despawn();
        }
    }
    let size_m = 1.0 / game.world.voxels_per_meter();
    for v in changed.0.drain() {
        let top = (Vec3::new(v[0] as f32 + 0.5, v[1] as f32 + 1.0, v[2] as f32 + 0.5)) * size_m;
        for (i, &id) in game.world.stored(v).unwrap_or_default().iter().enumerate() {
            let (layer, cell) = (i / 9, i % 9);
            let offset = Vec3::new(
                (cell % 3) as f32 - 1.0,
                0.0,
                (cell / 3) as f32 - 1.0,
            ) * ITEM_SIZE_M * 1.2
                + Vec3::Y * ITEM_SIZE_M * (layer as f32 + 0.5);
            commands.spawn((
                ItemVisual(id),
                StoredVisual(v),
                Transform::from_translation(top + offset),
            ));
        }
    }
}

/// Place le cube sur la position simulée.
// ponytail: suit le dernier tick, sans interpolation (cf. smooth_transform du
// joueur) ; à ajouter si la chute des items saccade.
pub fn place_items(mut items: Query<(&DroppedItem, &mut Transform)>) {
    for (item, mut transform) in &mut items {
        transform.translation = item.feet + Vec3::Y * ITEM_SIZE_M / 2.0;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::ecs::system::RunSystemOnce;
    use voxel_core::chunk::{Chunk, ChunkPos};
    use voxel_core::registry::{ContentEntry, Registry};
    use voxel_core::world::VoxelWorld;
    use voxel_core::worldgen::HeightmapGenerator;

    use super::*;

    /// Un chunk 16³ au sol plein sous y = 4 m ; joueur loin ou près.
    fn world_with_player(player_feet: Vec3) -> (World, ContentId) {
        let mut registry = Registry::new();
        let air = registry.register(ContentEntry::new_block("t:air", false, [0.0; 3])).unwrap();
        let stone = registry.register(ContentEntry::new_block("t:stone", true, [0.5; 3])).unwrap();
        let mut chunk = Chunk::filled(16, air);
        for (x, y, z) in (0..16).flat_map(|x| (0..4).flat_map(move |y| (0..16).map(move |z| (x, y, z)))) {
            chunk.set(x, y, z, stone);
        }
        let mut voxels = VoxelWorld::new(registry, 16, 1.0);
        voxels.insert_chunk(ChunkPos { x: 0, y: 0, z: 0 }, chunk);

        let mut world = World::new();
        world.insert_resource(GameWorld {
            world: voxels,
            generator: HeightmapGenerator {
                seed: 0, air, ground: stone, stone,
                ground_level_m: 4.0, amplitude_m: 0.0, feature_size_m: 1.0,
            },
            air,
            material: Handle::default(),
        });
        world.init_resource::<Inventory>();
        world.spawn(Player::at(player_feet));
        (world, stone)
    }

    /// Avance de `ticks` ticks de 1/64 s.
    fn run(world: &mut World, ticks: u32) {
        for _ in 0..ticks {
            let mut time = Time::<()>::default();
            time.advance_by(Duration::from_secs_f32(1.0 / 64.0));
            world.insert_resource(time);
            world.run_system_once(simulate_items).unwrap();
        }
    }

    #[test]
    fn item_lands_on_floor_then_is_picked_up_in_range() {
        // Joueur à 8 m : hors de portée, l'item tombe et se pose sur y = 4.
        let (mut world, stone) = world_with_player(Vec3::new(12.0, 4.0, 8.0));
        world.spawn(DroppedItem::new(stone, Vec3::new(4.0, 8.0, 8.0)));
        run(&mut world, 128);
        let mut q = world.query::<&DroppedItem>();
        let item = q.single(&world).unwrap();
        assert!((item.feet.y - 4.0).abs() < 1e-2, "posé à y = {}", item.feet.y);
        assert_eq!(world.resource::<Inventory>().selected(), None);

        // Le joueur s'approche : l'item est ramassé et disparaît.
        let mut p = world.query::<&mut Player>();
        *p.single_mut(&mut world).unwrap() = Player::at(Vec3::new(4.5, 4.0, 8.0));
        run(&mut world, 1);
        assert_eq!(world.resource::<Inventory>().selected(), Some(stone));
        assert_eq!(world.query::<&DroppedItem>().iter(&world).count(), 0);
    }
}
