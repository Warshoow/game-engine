// Shader des chunks : le shader standard de Bevy (éclairage compris), dont
// la couleur de base est multipliée par la texture du bloc et éclairée par
// la lumière du monde (#23).
//
// `in.uv` : coordonnées en mètres (1 image = 1 m, répétée par le sampler).
// `in.uv_b.x` : couche du texture array, -1 pour un bloc sans texture.
// `in.light` : [ciel, blocs] dans [0, 1], 1 = ciel ouvert (voir
// `voxel_core::light`).

#import bevy_pbr::{
    mesh_functions,
    view_transformations::position_world_to_clip,
    forward_io::{VertexOutput, FragmentOutput},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var block_textures: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var block_sampler: sampler;
// Force du jour dans [0, 1] (#30), voir `BlockTextures::daylight`.
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var<uniform> daylight: f32;

// Le noir d'une grotte sans lampe : presque, pas tout à fait.
const MIN_LIGHT: f32 = 0.02;
// Teinte de la lumière des blocs, une seule pour tous (couleur par bloc :
// #52).
const BLOCK_TINT: vec3<f32> = vec3(1.0, 0.82, 0.6);
// Force de la lumière des blocs, à côté du soleil par défaut de Bevy
// (10 000 lux) : une lampe éclaire sa voisine à peu près comme le soleil.
// Réglée à l'œil.
const BLOCK_LIGHT: f32 = 3000.0;

// Ce que lit le mesh : les attributs du shader standard, plus la lumière.
struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) uv_b: vec2<f32>,
    @location(5) color: vec4<f32>,
    @location(8) light: vec2<f32>,
};

// `VertexOutput` de Bevy, plus la lumière (Bevy n'a pas de champ libre).
struct VoxelVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) uv_b: vec2<f32>,
    @location(5) color: vec4<f32>,
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    @location(6) @interpolate(flat) instance_index: u32,
#endif
#ifdef VISIBILITY_RANGE_DITHER
    @location(7) @interpolate(flat) visibility_range_dither: i32,
#endif
    @location(8) light: vec2<f32>,
};

// Le vertex shader de Bevy (`bevy_pbr::render::mesh`), sans squelette ni
// morph : les chunks n'en ont pas.
@vertex
fn vertex(vertex: Vertex) -> VoxelVertexOutput {
    var out: VoxelVertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    out.world_normal = mesh_functions::mesh_normal_local_to_world(vertex.normal, vertex.instance_index);
    out.world_position = mesh_functions::mesh_position_local_to_world(world_from_local, vec4(vertex.position, 1.0));
    out.position = position_world_to_clip(out.world_position.xyz);
    out.uv = vertex.uv;
    out.uv_b = vertex.uv_b;
    out.color = vertex.color;
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif
#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        vertex.instance_index, world_from_local[3]);
#endif
    // Entité (#15) : la lumière de la grille à sa position, passée par le
    // tag de l'instance (0 : un chunk, sa lumière est aux sommets).
    let tag = mesh_functions::get_tag(vertex.instance_index);
    let grid = vec2(f32(tag & 0xffu), f32((tag >> 8u) & 0xffu)) / 255.0;
    out.light = select(vertex.light, grid, tag != 0u);
    return out;
}

// Lumière perçue d'un niveau dans [0, 1] : la courbe de Minecraft, qui
// tombe vite vers le noir loin de la source.
fn brightness(level: f32) -> f32 {
    let x = clamp(level, 0.0, 1.0);
    return x / (4.0 - 3.0 * x);
}

@fragment
fn fragment(voxel: VoxelVertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var in: VertexOutput;
    in.position = voxel.position;
    in.world_position = voxel.world_position;
    in.world_normal = voxel.world_normal;
    in.uv = voxel.uv;
    in.uv_b = voxel.uv_b;
    in.color = voxel.color;
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    in.instance_index = voxel.instance_index;
#endif
#ifdef VISIBILITY_RANGE_DITHER
    in.visibility_range_dither = voxel.visibility_range_dither;
#endif
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    // Échantillonné sans condition : WGSL interdit `textureSample` dans une
    // branche qui dépend d'une valeur interpolée.
    let layer = in.uv_b.x;
    let texel = textureSample(block_textures, block_sampler, in.uv, max(i32(round(layer)), 0));
    pbr_input.material.base_color *= select(vec4(1.0), texel, layer >= 0.0);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);

    // Le ciel règle ce que voient le soleil et l'ambiance (que le jour/nuit
    // baisse, #30) ; les blocs ajoutent leur lumière à part (une lampe
    // éclaire toutes les faces pareil), là où le ciel n'éclaire pas déjà :
    // dehors la nuit, une lampe éclaire.
    let sky = brightness(voxel.light.x);
    let block = brightness(voxel.light.y);
    let base = pbr_input.material.base_color.rgb;
    pbr_input.material.base_color = vec4(base * max(sky, MIN_LIGHT), pbr_input.material.base_color.a);
    // Alpha 1 : l'exposition de la caméra s'applique, comme au soleil.
    pbr_input.material.emissive = vec4(base * BLOCK_TINT * block * (1.0 - sky * daylight) * BLOCK_LIGHT, 1.0);

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
