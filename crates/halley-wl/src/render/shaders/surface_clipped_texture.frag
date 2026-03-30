precision highp float;
//_DEFINES

varying vec2 v_coords;

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
uniform vec2 geo_size;
uniform vec2 elem_size;
uniform vec2 elem_offset;
uniform float corner_radius;

float rounded_rect_sdf(vec2 p, vec2 size, float radius) {
    vec2 half_size = size * 0.5;
    vec2 q = abs(p) - (half_size - vec2(radius));
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
}

float sdf_alpha(float dist) {
    float aa = 0.75;
    return 1.0 - smoothstep(-aa, aa, dist);
}

void main() {
    vec2 size = max(geo_size, vec2(1.0));
    vec2 local = elem_offset + v_coords * max(elem_size, vec2(1.0));
    vec2 p = local - size * 0.5;
    float radius = min(corner_radius, min(size.x, size.y) * 0.5);
    float mask = sdf_alpha(rounded_rect_sdf(p, size, radius));
    if (mask <= 0.0) {
        discard;
    }

    vec4 sampled = texture2D(tex, clamp(v_coords, vec2(0.0), vec2(1.0)));
#if defined(NO_ALPHA)
    sampled = vec4(sampled.rgb, 1.0);
#endif
    if (sampled.a < 0.003) {
        sampled = vec4(0.0);
    }

    gl_FragColor = sampled * (mask * alpha);
}
