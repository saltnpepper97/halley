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

// Geometry rect within the dst (bbox) rect, in pixels relative to dst top-left.
// When geo_size is zero (old callers / border-only draws) we fall back to rect_size.
uniform vec2 geo_offset;
uniform vec2 geo_size;

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

    // Pixel position within the dst rect.
    vec2 px = v_coords * outer_size;

    // ----------------------------------------------------------------
    // Outer clip: rounded rect of the full dst (border + bbox extent).
    // ----------------------------------------------------------------
    float outer_radius = min(corner_radius, min(outer_size.x, outer_size.y) * 0.5);
    vec2 p_outer = px - outer_size * 0.5;
    float outer_dist = rounded_rect_sdf(p_outer, outer_size, outer_radius);
    float outer_alpha = sdf_alpha(outer_dist);
    if (outer_alpha <= 0.0) {
        discard;
    }

    // ----------------------------------------------------------------
    // Geometry clip: the content lives inside the geometry rect, which
    // may be inset from the full dst because of CSD shadows / decorations.
    // We round the geometry rect's corners to clip the actual window
    // pixels — this is what prevents Firefox et al. from poking past
    // the border.
    // ----------------------------------------------------------------
    vec2 eff_geo_size = (geo_size.x > 0.0 && geo_size.y > 0.0) ? geo_size : outer_size;
    vec2 eff_geo_offset = (geo_size.x > 0.0 && geo_size.y > 0.0) ? geo_offset : vec2(0.0);

    float border = clamp(border_px, 0.0, min(eff_geo_size.x, eff_geo_size.y) * 0.5);
    vec2 inner_size = max(eff_geo_size - vec2(border * 2.0), vec2(1.0));
    float inner_radius = max(outer_radius - border, 0.0);

    // Centre of the geometry rect in dst-pixel space.
    vec2 geo_center = eff_geo_offset + eff_geo_size * 0.5;
    vec2 p_inner = px - geo_center;

    float inner_dist = rounded_rect_sdf(p_inner, inner_size, inner_radius);
    float inner_alpha = (border > 0.0 || eff_geo_size != outer_size)
        ? sdf_alpha(inner_dist)
        : outer_alpha;

    // Border occupies the band between the outer clip and the inner geometry clip.
    float border_alpha = max(outer_alpha - inner_alpha, 0.0);

    vec4 sampled = texture2D(tex, clamp(v_coords, vec2(0.0), vec2(1.0)));
#if defined(NO_ALPHA)
    sampled = vec4(sampled.rgb, 1.0);
#endif
    if (sampled.a < 0.003) {
        sampled = vec4(0.0);
    }

    float base_content_a = sampled.a * content_alpha_scale;
    vec4 base_content = sampled * content_alpha_scale;
    vec4 base_composed = base_content + fill_color * max(1.0 - base_content_a, 0.0);
    vec4 composed = base_composed * inner_alpha;
    vec4 border_fill = border_color * border_alpha;

    gl_FragColor = (border_fill + composed) * alpha;
}

