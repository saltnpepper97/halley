precision highp float;
//_DEFINES

varying vec2 v_coords;
uniform sampler2D tex;
uniform float alpha;
uniform vec4 node_color;
uniform vec4 fill_color;
uniform vec2 rect_size;
uniform vec2 inner_rect_size;
uniform vec2 inner_rect_offset;
uniform float corner_radius;
uniform float inner_corner_radius;
uniform float border_px;

float rect_sdf(vec2 p, vec2 size) {
    vec2 half_size = size * 0.5;
    vec2 q = abs(p) - half_size;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0);
}

float sdf_alpha(float dist) {
    float aa = 0.75;
    return 1.0 - smoothstep(-aa, aa, dist);
}

void main() {
    vec2 size = max(rect_size, vec2(1.0));
    vec2 p = v_coords * size - size * 0.5;
    float dist = rect_sdf(p, size);
    float outer_alpha = sdf_alpha(dist);
    if (outer_alpha <= 0.0) { discard; }

    vec2 inner_size = max(inner_rect_size, vec2(1.0));
    vec2 inner_center = inner_rect_offset + inner_size * 0.5 - size * 0.5;
    float inner_dist = rect_sdf(p - inner_center, inner_size);
    float inner_alpha = border_px > 0.0 ? sdf_alpha(inner_dist) : outer_alpha;
    float border_alpha = max(outer_alpha - inner_alpha, 0.0);

    vec4 fill = fill_color * inner_alpha;
    vec4 border = node_color * border_alpha;

    gl_FragColor = (fill + border) * alpha;
}
