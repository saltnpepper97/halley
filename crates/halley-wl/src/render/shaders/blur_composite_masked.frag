precision highp float;
//_DEFINES

varying vec2 v_coords;
uniform sampler2D tex;
uniform sampler2D mask_tex;
uniform float alpha;
uniform vec2 patch_origin_uv;
uniform vec2 patch_size_uv;
uniform vec2 mask_uv_scale;

uniform float saturation;
uniform float noise;

float hash(vec2 p) {
    return fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453);
}

void main() {
    vec2 uv = clamp(v_coords, vec2(0.0), vec2(1.0));
    vec2 local_uv = (v_coords - patch_origin_uv) / max(patch_size_uv, vec2(0.000001));
    if (local_uv.x < 0.0 || local_uv.x > 1.0 || local_uv.y < 0.0 || local_uv.y > 1.0) {
        discard;
    }
    vec4 mask = texture2D(mask_tex, local_uv * mask_uv_scale);
    float mask_alpha = mask.a;
    if (mask_alpha <= 0.003) {
        discard;
    }

    vec3 color = texture2D(tex, uv).rgb;
    float luma = dot(color, vec3(0.2126, 0.7152, 0.0722));
    color = mix(vec3(luma), color, saturation);

    float d = (hash(local_uv * 2048.0) - 0.5) * 2.0 * noise;
    color = clamp(color + d, 0.0, 1.0);

    float a = mask_alpha * alpha;
    gl_FragColor = vec4(color * a, a);
}
