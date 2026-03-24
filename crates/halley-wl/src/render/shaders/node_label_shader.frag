precision mediump float;
//_DEFINES

varying vec2 v_coords;
uniform sampler2D tex;
uniform float alpha;
uniform vec4 node_color;
uniform vec4 fill_color;
uniform vec2 rect_size;
uniform float corner_radius;
uniform float border_px;

void main() {
    vec2 size = max(rect_size, vec2(1.0));
    float radius = min(corner_radius, min(size.x, size.y) * 0.5);

    vec2 p = v_coords * size - size * 0.5;
    vec2 half_extents = size * 0.5 - vec2(radius);
    vec2 q = abs(p) - half_extents;

    float dist = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
    if (dist > 0.0) { discard; }

    float inner_radius = max(radius - border_px, 0.0);
    vec2 inner_half_extents = max(half_extents - vec2(border_px), vec2(0.0));
    vec2 inner_q = abs(p) - inner_half_extents;

    float inner_dist =
        length(max(inner_q, 0.0)) + min(max(inner_q.x, inner_q.y), 0.0) - inner_radius;

    float in_border = 1.0 - step(0.0, inner_dist);
    in_border = 1.0 - in_border;

    vec2 norm_p = vec2(
        size.x > 0.0 ? p.x / (size.x * 0.5) : 0.0,
        size.y > 0.0 ? p.y / (size.y * 0.5) : 0.0
    );

    vec2 light_dir = normalize(vec2(-0.55, -0.65));

    float light = dot(norm_p, light_dir) * 0.5 + 0.5;
    light = light * 0.55 + 0.225;

    vec3 shaded_fill = mix(
        mix(fill_color.rgb, vec3(0.0), 0.10),
        mix(fill_color.rgb, vec3(1.0), 0.12),
        light
    );

    float border_light = dot(norm_p, light_dir) * 0.5 + 0.5;
    border_light = border_light * 0.55 + 0.225;

    vec3 shaded_border = mix(
        mix(node_color.rgb, vec3(0.0), 0.10),
        mix(node_color.rgb, vec3(1.0), 0.10),
        border_light
    );

    vec3 color = mix(shaded_fill, shaded_border, in_border);
    float edge_aa = 1.0 - smoothstep(-1.0, 0.0, dist);
    float final_alpha = alpha * edge_aa;

    gl_FragColor = vec4(color * final_alpha, final_alpha);
}
