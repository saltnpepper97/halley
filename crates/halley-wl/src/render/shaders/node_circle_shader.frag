precision mediump float;
//_DEFINES

varying vec2 v_coords;
uniform sampler2D tex;
uniform float alpha;
uniform vec4 node_color;
uniform vec4 fill_color;

void main() {
    vec2 p = v_coords * 2.0 - 1.0;
    float dist = length(p);
    if (dist > 1.0) { discard; }

    float border_w = node_color.a;
    float border_edge = 1.0 - border_w;

    float inner_aa = min(border_w * 0.45, 0.035);
    float in_border = smoothstep(border_edge - inner_aa, border_edge + inner_aa, dist);

    vec2 light_dir = normalize(vec2(-0.55, -0.65));

    float light = dot(p, light_dir) * 0.5 + 0.5;
    light = light * 0.55 + 0.225;
    vec3 shaded_fill = mix(
        mix(fill_color.rgb, vec3(0.0), 0.10),
        mix(fill_color.rgb, vec3(1.0), 0.12),
        light
    );

    float shadow_w = min(border_w * 0.5, 0.06);
    float shadow_t = smoothstep(border_edge - shadow_w, border_edge, dist);
    float fill_mask = 1.0 - in_border;
    shaded_fill = mix(shaded_fill, vec3(0.0), shadow_t * fill_mask * 0.13);

    float border_light = dot(p, light_dir) * 0.5 + 0.5;
    border_light = border_light * 0.55 + 0.225;
    vec3 shaded_border = mix(
        mix(node_color.rgb, vec3(0.0), 0.10),
        mix(node_color.rgb, vec3(1.0), 0.10),
        border_light
    );

    vec3 color = mix(shaded_fill, shaded_border, in_border);
    float edge_aa = 1.0 - smoothstep(0.96, 1.0, dist);
    float final_alpha = alpha * edge_aa;

    gl_FragColor = vec4(color * final_alpha, final_alpha);
}
