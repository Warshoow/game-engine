// Passe de profondeur des chunks (sert aux ombres du soleil, #51) : le
// vertex shader de prepass de Bevy, ce fragment-ci pour découper les trous
// des blocs `Mask` (feuilles, verre) dans la texture du bloc. Bevy, lui, lit
// la texture de `StandardMaterial`, que nos blocs n'ont pas.
//
// Mêmes attributs que la passe de Bevy (`prepass_io`) : `uv` en mètres,
// `uv_b.x` la couche du texture array, -1 sans texture.

#import bevy_pbr::prepass_io::VertexOutput
#ifdef PREPASS_FRAGMENT
#import bevy_pbr::prepass_io::FragmentOutput
#endif

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var block_textures: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var block_sampler: sampler;

// Le seuil de `AlphaMode::Mask(0.5)` du matériau des chunks.
const CUTOFF: f32 = 0.5;

fn discard_holes(in: VertexOutput) {
    let layer = in.uv_b.x;
    // Échantillonné sans condition, comme dans `voxel.wgsl`.
    let texel = textureSample(block_textures, block_sampler, in.uv, max(i32(round(layer)), 0));
    if layer >= 0.0 && texel.a < CUTOFF {
        discard;
    }
}

#ifdef PREPASS_FRAGMENT
@fragment
fn fragment(in: VertexOutput) -> FragmentOutput {
    discard_holes(in);
    var out: FragmentOutput;
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.frag_depth = in.unclipped_depth;
#endif
#ifdef NORMAL_PREPASS
    out.normal = vec4(in.world_normal * 0.5 + vec3(0.5), 1.0);
#endif
    return out;
}
#else
@fragment
fn fragment(in: VertexOutput) {
    discard_holes(in);
}
#endif
