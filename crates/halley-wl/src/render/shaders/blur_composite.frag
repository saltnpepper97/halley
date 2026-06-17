precision highp float;
//_DEFINES

// Composite a blurred backdrop patch beneath a translucent surface. Samples the
// pre-blurred full-resolution texture (mapped to this surface's screen rect via
// the src rectangle), applies saturation compensation and a tiny dither to avoid
// banding, and clips to the surface's rounded-rect so nothing bleeds past the
// corners.

varying vec2 v_coords;
uniform sampler2D tex;
uniform float alpha;

uniform vec2 rect_size;
uniform vec2 patch_origin_uv;
uniform vec2 patch_size_uv;
uniform float corner_radius;
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

// Cheap hash for dithering; breaks up gradient banding in the blurred result.
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

    float radius = min(max(corner_radius, 0.0), min(size.x, size.y) * 0.5);
    float dist = rounded_rect_sdf(px - size * 0.5, size, radius);
    float mask = sdf_alpha(dist);
    if (mask <= 0.0) {
        discard;
    }

    vec3 color = texture2D(tex, clamp(v_coords, vec2(0.0), vec2(1.0))).rgb;

    // Saturation compensation: blurring averages toward grey, so push chroma back.
    float luma = dot(color, vec3(0.2126, 0.7152, 0.0722));
    color = mix(vec3(luma), color, saturation);

    // Symmetric dither in [-noise, +noise].
    float d = (hash(px) - 0.5) * 2.0 * noise;
    color = clamp(color + d, 0.0, 1.0);

    float a = mask * alpha;
    gl_FragColor = vec4(color * a, a);
}
