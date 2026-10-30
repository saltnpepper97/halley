precision highp float;
//_DEFINES

varying vec2 v_coords;
uniform sampler2D tex;
uniform float alpha;
uniform vec2 rect_size;
uniform vec2 patch_origin_uv;
uniform vec2 patch_size_uv;
uniform float corner_radius;
uniform vec2 clip_origin;
uniform vec2 clip_size;
uniform vec2 clip_radii;
uniform float use_clip;
uniform float saturation;
uniform float noise;

float rounded_rect_sdf(vec2 p, vec2 size, float radius) {
    vec2 half_size = size * 0.5;
    vec2 q = abs(p) - (half_size - vec2(radius));
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
}

float sdf_alpha(float dist) {
    float aa = 0.75;
    return 1.0 - smoothstep(-aa, aa, dist);
}

float hash(vec2 p) {
    return fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453);
}

void main() {
    vec2 size = max(rect_size, vec2(1.0));
    vec2 local_uv = (v_coords - patch_origin_uv) / max(patch_size_uv, vec2(0.000001));
    if (local_uv.x < 0.0 || local_uv.x > 1.0 || local_uv.y < 0.0 || local_uv.y > 1.0) {
        discard;
    }
    vec2 px = local_uv * size;
    // A bottom-rounded clip belongs to content beneath a titlebar. Where the
    // blur patch starts at that join, keep its shared top edge fully covered.
    float patch_top = patch_origin_uv.y * (size.y / max(patch_size_uv.y, 0.000001));
    bool joined_top = use_clip > 0.5 && clip_radii.x == 0.0 && clip_radii.y > 0.0
        && abs(patch_top - clip_origin.y) < 0.01;
    if (joined_top) {
        px.y = max(px.y, 0.75);
    }

    float radius = min(max(corner_radius, 0.0), min(size.x, size.y) * 0.5);
    float dist = rounded_rect_sdf(px - size * 0.5, size, radius);
    float mask = sdf_alpha(dist);
    if (use_clip > 0.5) {
        // v_coords samples the output-sized blur texture, so recover output
        // pixels directly rather than deriving them from the patch-local quad.
        vec2 output_px = v_coords * (rect_size / max(patch_size_uv, vec2(0.000001)));
        vec2 clip_local = output_px - clip_origin;
        if (joined_top) {
            clip_local.y = max(clip_local.y, 0.75);
        }
        float clip_r = clip_local.y < clip_size.y * 0.5 ? clip_radii.x : clip_radii.y;
        clip_r = min(max(clip_r, 0.0), min(clip_size.x, clip_size.y) * 0.5);
        float clip_dist = rounded_rect_sdf(
            clip_local - clip_size * 0.5,
            max(clip_size, vec2(1.0)),
            clip_r
        );
        mask *= sdf_alpha(clip_dist);
    }
    if (mask <= 0.0) {
        discard;
    }

    vec3 color = texture2D(tex, clamp(v_coords, vec2(0.0), vec2(1.0))).rgb;
    float luma = dot(color, vec3(0.2126, 0.7152, 0.0722));
    color = mix(vec3(luma), color, saturation);
    float d = (hash(px) - 0.5) * 2.0 * noise;
    color = clamp(color + d, 0.0, 1.0);

    float a = mask * alpha;
    gl_FragColor = vec4(color * a, a);
}
