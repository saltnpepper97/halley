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

    // A Gaussian-like or exponential falloff looks much better than smoothstep (which is an S-curve).
    // Shadows should drop off quickly near the caster and have a long, soft tail.
    float t = clamp((outside_dist - outset) / (fade_end - outset), 0.0, 1.0);
    
    // A simple approximation for a soft, natural tail:
    float falloff = pow(1.0 - t, 2.5);

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
