// Fragment des chunks : le shader standard de Bevy (éclairage compris), dont
// la couleur de base est multipliée par la texture du bloc.
//
// `in.uv` : coordonnées en mètres (1 image = 1 m, répétée par le sampler).
// `in.uv_b.x` : couche du texture array, -1 pour un bloc sans texture.

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var block_textures: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var block_sampler: sampler;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    // Échantillonné sans condition : WGSL interdit `textureSample` dans une
    // branche qui dépend d'une valeur interpolée.
    let layer = in.uv_b.x;
    let texel = textureSample(block_textures, block_sampler, in.uv, max(i32(round(layer)), 0));
    pbr_input.material.base_color *= select(vec4(1.0), texel, layer >= 0.0);

    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
