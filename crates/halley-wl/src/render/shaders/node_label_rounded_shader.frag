precision highp float;
//_DEFINES

varying vec2 v_coords;
uniform sampler2D tex;
uniform float alpha;
uniform vec4 node_color;
uniform vec4 fill_color;
uniform vec2 rect_size;
uniform float corner_radius;
uniform float border_px;

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
    vec2 size = max(rect_size, vec2(1.0));
    float radius = min(corner_radius, min(size.x, size.y) * 0.5);

    vec2 p = v_coords * size - size * 0.5;
    float dist = rounded_rect_sdf(p, size, radius);
    float outer_alpha = sdf_alpha(dist);
    if (outer_alpha <= 0.0) { discard; }

    float inner_border = clamp(border_px, 0.0, min(size.x, size.y) * 0.5);
    vec2 inner_size = max(size - vec2(inner_border * 2.0), vec2(1.0));
    float inner_radius = max(radius - inner_border, 0.0);
    float inner_dist = rounded_rect_sdf(p, inner_size, inner_radius);
    float inner_alpha = border_px > 0.0 ? sdf_alpha(inner_dist) : outer_alpha;
    float border_alpha = max(outer_alpha - inner_alpha, 0.0);

    vec3 shaded_fill = fill_color.rgb;
    vec3 shaded_border = node_color.rgb;

    vec3 color = shaded_fill * inner_alpha + shaded_border * border_alpha;
    float final_alpha = alpha * max(inner_alpha, border_alpha);

    gl_FragColor = vec4(color * alpha, final_alpha);
}
