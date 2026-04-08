precision mediump float;
//_DEFINES

varying vec2 v_coords;
uniform sampler2D tex;
uniform float alpha;
uniform vec4 node_color;
uniform vec4 fill_color;
uniform float flat_fill;
uniform float center_flat_fill;

void main() {
    vec2 p = v_coords * 2.0 - 1.0;
    vec2 a = abs(p);
    float dist = pow(pow(a.x, 4.0) + pow(a.y, 4.0), 0.25);
    if (dist > 1.0) { discard; }

    float border_w    = node_color.a;
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
    float shadow_w_effective = mix(shadow_w, shadow_w * 0.35, center_flat_fill);
    float shadow_t = smoothstep(border_edge - shadow_w_effective, border_edge, dist);
    float fill_mask = 1.0 - in_border;
    float shadow_strength = mix(0.13, 0.045, center_flat_fill);
    shaded_fill = mix(shaded_fill, vec3(0.0), shadow_t * fill_mask * shadow_strength);
    shaded_fill = mix(shaded_fill, fill_color.rgb, flat_fill * fill_mask);
    float center_mask = 1.0 - smoothstep(0.18, 0.70, dist);
    shaded_fill = mix(shaded_fill, fill_color.rgb, center_flat_fill * center_mask * (1.0 - flat_fill));

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
