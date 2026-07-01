precision highp float;
//_DEFINES

varying vec2 v_coords;
uniform sampler2D tex;
uniform float alpha;
uniform vec2 u_resolution;
uniform vec2 u_camera_center;
uniform vec2 u_camera_size;
uniform float u_time;
uniform float u_intensity;
uniform vec3 u_base_color;
uniform vec3 u_accent_color;

const float TAU = 6.2831853;

// Cells before the star identity repeats (×cell_size = world-unit period). Only
// the *hash input* is wrapped at this period (see hcell), so star identity stays
// exact arbitrarily far from the origin without any visible repeat. The world
// coordinate itself is never wrapped, so there is no seam and no jump as the
// camera pans across period boundaries.
const float STAR_CELL_PERIOD = 8192.0;

float hash21(vec2 p) {
    p = fract(p * vec2(0.1031, 0.11369));
    p += dot(p, p.yx + 19.19);
    return fract((p.x + p.y) * p.x);
}

float value_noise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    float a = hash21(i);
    float b = hash21(i + vec2(1.0, 0.0));
    float c = hash21(i + vec2(0.0, 1.0));
    float d = hash21(i + vec2(1.0, 1.0));
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

// Fractal Brownian motion: rotated octaves break up axis-aligned grid artifacts
// and give the gas its soft, billowing structure. Octave count is a parameter so
// each term spends only as many noise lookups as it visibly needs.
float fbm(vec2 p, int oct) {
    float v = 0.0;
    float a = 0.5;
    mat2 rot = mat2(0.8, 0.6, -0.6, 0.8);
    for (int i = 0; i < 5; i++) {
        if (i >= oct) break;
        v += a * value_noise(p);
        p = rot * p * 2.0;
        a *= 0.5;
    }
    return v;
}

// Ridged noise: sharp creases for filamentary dust lanes.
float ridge(vec2 p, int oct) {
    float v = 0.0;
    float a = 0.5;
    mat2 rot = mat2(0.8, 0.6, -0.6, 0.8);
    for (int i = 0; i < 5; i++) {
        if (i >= oct) break;
        float n = value_noise(p);
        n = 1.0 - abs(n - 0.5) * 2.0;
        n = n * n;
        v += a * n;
        p = rot * p * 2.0;
        a *= 0.5;
    }
    return v;
}

float small_star(vec2 p, float radius) {
    float d = length(p);
    float core = 1.0 - smoothstep(radius * 0.35, radius, d);
    float glow = (1.0 - smoothstep(radius, radius * 3.5, d)) * 0.18;
    return core + glow;
}

float glint_star(vec2 p, float radius) {
    float d = length(p);
    float core = 1.0 - smoothstep(radius * 0.25, radius, d);
    float glow = (1.0 - smoothstep(radius, radius * 5.0, d)) * 0.26;
    float arm_x = (1.0 - smoothstep(0.0, radius * 5.0, abs(p.x)))
        * (1.0 - smoothstep(0.0, radius * 0.35, abs(p.y)));
    float arm_y = (1.0 - smoothstep(0.0, radius * 5.0, abs(p.y)))
        * (1.0 - smoothstep(0.0, radius * 0.35, abs(p.x)));
    return core + glow + (arm_x + arm_y) * 0.18;
}

// Star colour by temperature: mostly white, with cool-blue and warm-gold/red
// minorities. Far more convincing than a single accent-tinted field.
vec3 star_tint(float t) {
    vec3 blue = vec3(0.70, 0.81, 1.00);
    vec3 white = vec3(1.00, 0.98, 0.95);
    vec3 gold = vec3(1.00, 0.87, 0.64);
    vec3 red = vec3(1.00, 0.72, 0.58);
    if (t < 0.22) {
        return mix(blue, white, t / 0.22);
    } else if (t < 0.78) {
        return white;
    } else if (t < 0.91) {
        return mix(white, gold, (t - 0.78) / 0.13);
    }
    return mix(gold, red, (t - 0.91) / 0.09);
}

// One density layer of stars. Returns a premultiplied colour contribution so
// each star carries its own temperature tint.
vec3 star_field(vec2 world, float cell_size, float radius, float threshold, float salt, float glints) {
    vec2 grid = world / cell_size;
    vec2 cell = floor(grid);
    vec2 local = fract(grid) - 0.5;
    // Wrap the cell index used for hashing at a large period so the hash inputs
    // stay small and exact no matter how far the canvas has been panned. The
    // period is huge (STAR_CELL_PERIOD * cell_size world units, ~thousands of
    // screens) so the repetition is never perceptible — but it keeps star
    // identity rock-stable instead of re-shuffling at large coordinates. Near the
    // origin hcell == cell, so the home view is pixel-identical to before.
    vec2 hcell = mod(cell, STAR_CELL_PERIOD);
    vec2 jitter = vec2(hash21(hcell + salt), hash21(hcell + salt + 23.7)) - 0.5;
    vec2 p = local - jitter * 0.72;
    float seed = hash21(hcell + salt * 2.3);
    float gate = step(threshold, seed);
    float size_seed = hash21(hcell + salt * 4.9);
    float twinkle = 0.78 + 0.22 * sin(u_time * (0.75 + seed * 1.5) + seed * TAU);
    float is_glint = step(1.0 - glints, seed);
    float star = mix(
        small_star(p, radius * mix(0.65, 1.35, size_seed)),
        glint_star(p, radius * mix(0.80, 1.55, size_seed)),
        is_glint
    );
    // Square the seed so most stars are faint and only a few are bright.
    float bright = mix(0.30, 1.45, seed * seed);
    vec3 tint = star_tint(hash21(hcell + salt * 7.1));
    return tint * (star * gate * twinkle * bright);
}

void main() {
    vec2 uv = v_coords;
    // World position, computed with the exact same mapping content uses
    // (presentation.rs world_to_screen): world = center + (uv-0.5)*view. No
    // wrapping of the world coordinate, so the field translates/zooms cleanly
    // with the camera — no seam, and no re-arrange when crossing any boundary.
    vec2 world = u_camera_center + (uv - 0.5) * u_camera_size;

    vec3 base = max(u_base_color, vec3(0.0));
    vec3 accent = max(u_accent_color, vec3(0.0));
    float intensity = max(u_intensity, 0.0);

    // Domain-warped nebula. A very slow drift animates the gas; a low-frequency
    // term makes the density grow/vary continuously across pans (no tiling).
    vec2 drift = vec2(u_time * 4.0, -u_time * 2.0);
    vec2 nb = (world + drift) * 0.0009;
    vec2 warp = vec2(fbm(nb + vec2(4.1, 1.7), 3), fbm(nb + vec2(8.2, 2.3), 3));
    nb += (warp - 0.5) * 1.3;

    float clouds = smoothstep(0.35, 0.95, fbm(nb, 4));
    float dust = smoothstep(0.45, 0.95, ridge(nb * 1.7 + vec2(3.3, 7.1), 4));
    float large = fbm(world * 0.00018 + vec2(11.0, 5.0), 2);
    float paper = fbm(world * 0.0016, 3) * 0.5;

    vec3 nebula_dust = accent.brg * 0.8; // derived second hue for the lanes
    float density = clouds * (0.45 + large * 0.85);

    vec3 color = base * (0.90 + paper * 0.12);
    color = mix(color, accent, density * 0.12 * intensity);
    color = mix(color, nebula_dust, dust * 0.06 * intensity);

    // Screen pixels per world unit: ~1 at home zoom, <1 when zoomed out. Once a
    // star drops below ~1px it can no longer be resolved and just crawls/shimmers
    // as you pan, which reads as the field "re-arranging". Fade the small dense
    // layers out as the view zooms out. zoom_fade == 1.0 at/above home zoom, so
    // the home view is untouched — this only engages once you zoom way out.
    float px_per_world = u_resolution.x / max(u_camera_size.x, 1.0);
    float zoom_fade = smoothstep(0.25, 0.70, px_per_world);

    vec3 stars = vec3(0.0);
    stars += star_field(world, 52.0, 0.020, 0.72, 3.0, 0.010) * 0.30 * mix(1.0, zoom_fade, 0.85);
    stars += star_field(world + vec2(311.0, -127.0), 92.0, 0.018, 0.66, 17.0, 0.030) * 0.58 * mix(1.0, zoom_fade, 0.55);
    stars += star_field(world + vec2(-919.0, 541.0), 180.0, 0.017, 0.58, 41.0, 0.070) * 0.86 * mix(1.0, zoom_fade, 0.30);
    // Faint, dense dust-star layer for depth (most shimmer-prone — fully faded out).
    stars += star_field(world + vec2(157.0, 803.0), 34.0, 0.016, 0.82, 67.0, 0.0) * 0.16 * zoom_fade;

    vec2 aspect_uv = (uv - 0.5) * vec2(u_resolution.x / max(u_resolution.y, 1.0), 1.0);
    float center_lift = 1.0 - smoothstep(0.20, 1.20, length(aspect_uv));
    color += stars * intensity;
    color += accent * center_lift * 0.018 * intensity;

    float final_alpha = alpha;
    gl_FragColor = vec4(color * final_alpha, final_alpha);
}
