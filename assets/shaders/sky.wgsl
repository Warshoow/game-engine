// Ciel (#30) : intérieur d'une grande sphère qui suit la caméra. Dégradé de
// l'horizon au zénith, soleil, lune à l'opposé, étoiles quand le jour
// baisse. Couleurs réglées par `sky.rs` depuis `sky.ron`. L'horizon a la
// couleur du brouillard : pas de raccord au bord du monde chargé.

#import bevy_pbr::{forward_io::VertexOutput, mesh_view_bindings::view}
#ifdef TONEMAP_IN_SHADER
#import bevy_core_pipeline::tonemapping::tone_mapping
#endif

struct Sky {
    zenith: vec4<f32>,
    horizon: vec4<f32>,
    // xyz : direction vers le soleil ; w : force du jour.
    sun: vec4<f32>,
    sun_color: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> sky: Sky;

// Taille apparente du soleil et de la lune : cosinus de leur rayon (~1,5°,
// trois fois le vrai : lisible à l'écran).
const DISC: f32 = 0.99966;
// Le soleil dépasse 1 en HDR : le bloom lui fait un halo.
const SUN_STRENGTH: f32 = 20.0;
// Grille des étoiles sur la sphère, et part des cases qui en ont une.
const STAR_GRID: f32 = 150.0;
const STAR_SHARE: f32 = 0.004;

fn hash(p: vec3<f32>) -> f32 {
    return fract(sin(dot(p, vec3(12.9898, 78.233, 45.164))) * 43758.5453);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let dir = normalize(in.world_position.xyz - view.world_position);
    let up = max(dir.y, 0.0);
    let day = sky.sun.w;
    // Sous l'horizon : sa couleur, comme le brouillard.
    var color = mix(sky.horizon.rgb, sky.zenith.rgb, sqrt(up));

    // Le sol cache ce qui est sous l'horizon.
    let above = smoothstep(-0.02, 0.01, dir.y);
    // Halo autour du soleil, plus fort quand il est bas (lever, coucher).
    let to_sun = dot(dir, sky.sun.xyz);
    color += above * sky.sun_color.rgb * pow(max(to_sun, 0.0), 32.0) * 0.4 * (1.0 - 0.5 * sky.sun.y);
    color += above * sky.sun_color.rgb * SUN_STRENGTH * smoothstep(DISC - 0.00005, DISC, to_sun);

    let night = 1.0 - day;
    let moon = above * smoothstep(DISC - 0.00005, DISC, -to_sun);
    color += vec3(0.8, 0.85, 1.0) * moon * (0.3 + 1.7 * night);

    // Un point rond au centre des cases tirées (la case seule, découpée
    // par la sphère, ferait un éclat anguleux).
    let cell = floor(dir * STAR_GRID);
    let round = 1.0 - smoothstep(0.1, 0.2, length(dir * STAR_GRID - cell - 0.5));
    let star = step(1.0 - STAR_SHARE, hash(cell)) * hash(cell + 1.0) * round;
    color += vec3(star) * night * night * smoothstep(0.0, 0.2, up) * (1.0 - moon);

    var out = vec4(color, 1.0);
#ifdef TONEMAP_IN_SHADER
    out = tone_mapping(out, view.color_grading);
#endif
    return out;
}
