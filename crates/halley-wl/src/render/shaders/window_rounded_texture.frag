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
uniform vec2 rect_size;
uniform float corner_radius;
uniform float border_px;
uniform vec4 border_color;
uniform vec4 fill_color;
uniform float content_alpha_scale;

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
    vec2 outer_size = max(rect_size, vec2(1.0));
    float outer_radius = min(corner_radius, min(outer_size.x, outer_size.y) * 0.5);
    vec2 p = v_coords * outer_size - outer_size * 0.5;

    float outer_dist = rounded_rect_sdf(p, outer_size, outer_radius);
    float outer_alpha = sdf_alpha(outer_dist);
    if (outer_alpha <= 0.0) {
        discard;
    }

    float border = clamp(border_px, 0.0, min(outer_size.x, outer_size.y) * 0.5);
    vec2 inner_size = max(outer_size - vec2(border * 2.0), vec2(1.0));
    float inner_radius = max(outer_radius - border, 0.0);
    float inner_dist = rounded_rect_sdf(p, inner_size, inner_radius);
    float inner_alpha = border > 0.0 ? sdf_alpha(inner_dist) : outer_alpha;
    float border_alpha = max(outer_alpha - inner_alpha, 0.0);

    vec4 sampled = texture2D(tex, clamp(v_coords, vec2(0.0), vec2(1.0)));
#if defined(NO_ALPHA)
    sampled = vec4(sampled.rgb, 1.0);
#endif
    if (sampled.a < 0.003) {
        sampled = vec4(0.0);
    }

    vec4 fill = fill_color * inner_alpha;
    vec4 content = sampled * (inner_alpha * content_alpha_scale);
    vec4 composed = content + fill * max(1.0 - content.a, 0.0);
    vec4 border_fill = border_color * border_alpha;

    gl_FragColor = (border_fill + composed) * alpha;
}
