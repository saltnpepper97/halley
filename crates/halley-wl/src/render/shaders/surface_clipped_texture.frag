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
uniform float clip_scale;
uniform vec2 geo_size;
uniform vec4 corner_radius;
uniform vec3 input_to_geo_row_0;
uniform vec3 input_to_geo_row_1;
uniform vec3 input_to_geo_row_2;

// Zoom crispness: `clip_scale` is the render scale (content * camera). When it is
// > 1 the live buffer is being upscaled (zoomed in), so we resample with a
// Catmull-Rom bicubic + optional unsharp instead of the plain bilinear tap.
// `tex_size` is the source buffer size in px (for texel-sized steps); `sharpen`
// (0..1) is the unsharp strength. clip_scale <= 1 keeps the single cheap tap.
uniform vec2 tex_size;
uniform float sharpen;

vec4 tap(vec2 c) {
    return texture2D(tex, clamp(c, vec2(0.0), vec2(1.0)));
}

vec4 cubic_weights(float t) {
    float t2 = t * t;
    float t3 = t2 * t;
    return vec4(
        -0.5 * t3 + t2 - 0.5 * t,
        1.5 * t3 - 2.5 * t2 + 1.0,
        -1.5 * t3 + 2.0 * t2 + 0.5 * t,
        0.5 * t3 - 0.5 * t2
    );
}

vec4 sample_bicubic(vec2 coord) {
    vec2 texel = 1.0 / tex_size;
    vec2 uv = coord * tex_size - 0.5;
    vec2 f = fract(uv);
    vec2 base = (floor(uv) + 0.5) * texel;
    vec4 wx = cubic_weights(f.x);
    vec4 wy = cubic_weights(f.y);

    vec4 row0 =
        wx.x * tap(base + vec2(-1.0, -1.0) * texel) + wx.y * tap(base + vec2(0.0, -1.0) * texel) +
        wx.z * tap(base + vec2(1.0, -1.0) * texel) + wx.w * tap(base + vec2(2.0, -1.0) * texel);
    vec4 row1 =
        wx.x * tap(base + vec2(-1.0, 0.0) * texel) + wx.y * tap(base + vec2(0.0, 0.0) * texel) +
        wx.z * tap(base + vec2(1.0, 0.0) * texel) + wx.w * tap(base + vec2(2.0, 0.0) * texel);
    vec4 row2 =
        wx.x * tap(base + vec2(-1.0, 1.0) * texel) + wx.y * tap(base + vec2(0.0, 1.0) * texel) +
        wx.z * tap(base + vec2(1.0, 1.0) * texel) + wx.w * tap(base + vec2(2.0, 1.0) * texel);
    vec4 row3 =
        wx.x * tap(base + vec2(-1.0, 2.0) * texel) + wx.y * tap(base + vec2(0.0, 2.0) * texel) +
        wx.z * tap(base + vec2(1.0, 2.0) * texel) + wx.w * tap(base + vec2(2.0, 2.0) * texel);

    return wy.x * row0 + wy.y * row1 + wy.z * row2 + wy.w * row3;
}

float rounding_alpha(vec2 coords, vec2 size) {
    vec2 center;
    float radius;

    if (coords.x < corner_radius.x && coords.y < corner_radius.x) {
        radius = corner_radius.x;
        center = vec2(radius, radius);
    } else if (size.x - corner_radius.y < coords.x && coords.y < corner_radius.y) {
        radius = corner_radius.y;
        center = vec2(size.x - radius, radius);
    } else if (size.x - corner_radius.z < coords.x && size.y - corner_radius.z < coords.y) {
        radius = corner_radius.z;
        center = vec2(size.x - radius, size.y - radius);
    } else if (coords.x < corner_radius.w && size.y - corner_radius.w < coords.y) {
        radius = corner_radius.w;
        center = vec2(radius, size.y - radius);
    } else {
        return 1.0;
    }

    float dist = distance(coords, center);
    float half_px = 0.5 / max(clip_scale, 1.0);
    return 1.0 - smoothstep(radius - half_px, radius + half_px, dist);
}

void main() {
    mat3 input_to_geo = mat3(input_to_geo_row_0, input_to_geo_row_1, input_to_geo_row_2);
    vec3 coords_geo = input_to_geo * vec3(v_coords, 1.0);

    vec4 sampled;
    if (clip_scale > 1.0 && tex_size.x > 0.0 && tex_size.y > 0.0) {
        vec2 sc = clamp(v_coords, vec2(0.0), vec2(1.0));
        sampled = sample_bicubic(sc);
        if (sharpen > 0.0) {
            vec2 texel = 1.0 / tex_size;
            vec4 neigh = tap(sc + vec2(texel.x, 0.0)) + tap(sc - vec2(texel.x, 0.0)) +
                         tap(sc + vec2(0.0, texel.y)) + tap(sc - vec2(0.0, texel.y));
            sampled += (sampled - neigh * 0.25) * sharpen;
        }
    } else {
        sampled = texture2D(tex, clamp(v_coords, vec2(0.0), vec2(1.0)));
    }
#if defined(NO_ALPHA)
    sampled = vec4(sampled.rgb, 1.0);
#endif
    if (sampled.a < 0.003) {
        sampled = vec4(0.0);
    }

    vec2 size = max(geo_size, vec2(1.0));
    if (coords_geo.x < 0.0 || coords_geo.x > 1.0 || coords_geo.y < 0.0 || coords_geo.y > 1.0) {
        sampled = vec4(0.0);
    } else {
        sampled *= rounding_alpha(coords_geo.xy * size, size);
    }

    gl_FragColor = sampled * alpha;
}
