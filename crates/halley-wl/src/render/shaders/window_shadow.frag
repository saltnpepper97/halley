precision highp float;
//_DEFINES

varying vec2 v_coords;
uniform sampler2D tex;
uniform float alpha;
uniform vec2 rect_size;
uniform vec2 caster_size;
uniform vec2 caster_center;
uniform float corner_radius;
uniform float spread;
uniform float shadow_radius;
uniform vec4 shadow_color;

float rounded_rect_sdf(vec2 p, vec2 size, float radius) {
    vec2 half_size = size * 0.5;
    vec2 q = abs(p) - (half_size - vec2(radius));
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
}

// Abramowitz & Stegun 7.1.26 approximation of the error function.
// Max error ~1.5e-7, far more than enough for an 8-bit shadow.
float erf_approx(float x) {
    float s = sign(x);
    float a = abs(x);
    float t = 1.0 / (1.0 + 0.3275911 * a);
    float y = 1.0 - (((((1.061405429 * t - 1.453152027) * t)
        + 1.421413741) * t - 0.284496736) * t + 0.254829592) * t * exp(-a * a);
    return s * y;
}

void main() {
    vec2 size = max(rect_size, vec2(1.0));
    vec2 caster = max(caster_size, vec2(1.0));
    float radius = min(max(corner_radius, 0.0), min(caster.x, caster.y) * 0.5);

    // The shadow quad is padded and placed according to the shadow offset.
    // The caster center is now correctly centered in the quad.
    vec2 p = v_coords * size - caster_center;
    float dist = rounded_rect_sdf(p, caster, radius);

    float blur = max(shadow_radius, 1.0);
    float outset = max(spread, 0.0);
    // Expanding the fade slightly makes the shadow tail look more natural
    float fade_end = outset + blur * 3.0;

    // Only the outside falloff matters for the visible shadow. The window itself
    // covers the inner region, but keeping this stable avoids odd edge behavior.
    float outside_dist = max(dist, 0.0);
    if (outside_dist >= fade_end) {
        discard;
    }

    // A real drop shadow is the caster shape convolved with a Gaussian, so the
    // coverage at the geometric edge is ~50% and falls off along the Gaussian
    // integral (the error function). This soft contact reads as a true shadow
    // rather than a hard outline hugging the window border.
    //
    // Map the configured blur radius to a Gaussian sigma. blur_radius is the
    // user-facing "softness"; sigma is the actual bell width. Keeping sigma at
    // half the blur radius means the existing padded quad (pad = blur*3 in
    // shadow.rs) always covers the visible tail, so no Rust changes are needed.
    float sigma = max(blur * 0.5, 0.5);

    // Signed distance past the spread band:
    //   negative -> inside the spread band, coverage rises above 0.5 toward 1.0
    //   zero     -> the (spread-expanded) edge, coverage is exactly 0.5
    //   positive -> outside, coverage falls off following erfc
    float d = dist - outset;
    float falloff = 0.5 * (1.0 - erf_approx(d / (sigma * 1.41421356)));

    float a = shadow_color.a * alpha * falloff;

    // Kill tiny fringe values. On bright wallpapers, very low-alpha tinted pixels
    // can read as a visible halo/bounds rectangle even when the math reaches 0.
    if (a <= 0.003) {
        discard;
    }

    // Output premultiplied RGB so colored/tinted shadows do not leak color in the
    // transparent tail of the blur. This is the important anti-halo bit.
    gl_FragColor = vec4(shadow_color.rgb * a, a);
}
