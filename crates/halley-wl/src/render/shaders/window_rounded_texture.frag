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

// Maps the incoming texture coordinate (v_coords, which spans the rendered `src`
// sub-rect in normalised texture space) back to [0,1] across the dst quad. When a
// caller renders a cropped `src` (CSD/GTK apps whose preview is inset to the window
// geometry), v_coords no longer spans [0,1], which would miscentre the rounding SDF
// and leave square corners. src_uv_scale <= 0 means "unset" -> identity (full src).
uniform vec2 src_uv_offset;
uniform vec2 src_uv_scale;

// Zoom magnification: dst_pixels / src_pixels for this draw. > 1.0 means the client
// buffer is being upscaled (the camera is zoomed in), which enables a Catmull-Rom
// bicubic resample plus an optional unsharp pass so magnified content stays crisp
// instead of bilinear-soft. 1.0 (the default / the `filter=bilinear` config path)
// keeps the single cheap tap, so unzoomed frames are pixel-identical to before.
uniform float magnify;
uniform vec2 tex_size;   // source texture size in px (for texel-sized steps)
uniform float sharpen;   // unsharp strength [0,1], only applied when magnified

float rounded_rect_sdf(vec2 p, vec2 size, float radius) {
    vec2 half_size = size * 0.5;
    vec2 q = abs(p) - (half_size - vec2(radius));
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
}

float sdf_alpha(float dist) {
    float aa = 0.75;
    return 1.0 - smoothstep(-aa, aa, dist);
}

vec4 tap(vec2 c, vec2 lo, vec2 hi) {
    return texture2D(tex, clamp(c, lo, hi));
}

// Catmull-Rom weights for the four taps at offsets (-1, 0, 1, 2) around a sample.
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

// 4x4 Catmull-Rom bicubic resample. Sharper than bilinear when upscaling; the mild
// overshoot on edges is what restores crispness and is clamped by the framebuffer.
vec4 sample_bicubic(vec2 coord, vec2 lo, vec2 hi) {
    vec2 texel = 1.0 / tex_size;
    vec2 uv = coord * tex_size - 0.5;
    vec2 f = fract(uv);
    vec2 base = (floor(uv) + 0.5) * texel;
    vec4 wx = cubic_weights(f.x);
    vec4 wy = cubic_weights(f.y);

    vec4 row0 =
        wx.x * tap(base + vec2(-1.0, -1.0) * texel, lo, hi) +
        wx.y * tap(base + vec2(0.0, -1.0) * texel, lo, hi) +
        wx.z * tap(base + vec2(1.0, -1.0) * texel, lo, hi) +
        wx.w * tap(base + vec2(2.0, -1.0) * texel, lo, hi);
    vec4 row1 =
        wx.x * tap(base + vec2(-1.0, 0.0) * texel, lo, hi) +
        wx.y * tap(base + vec2(0.0, 0.0) * texel, lo, hi) +
        wx.z * tap(base + vec2(1.0, 0.0) * texel, lo, hi) +
        wx.w * tap(base + vec2(2.0, 0.0) * texel, lo, hi);
    vec4 row2 =
        wx.x * tap(base + vec2(-1.0, 1.0) * texel, lo, hi) +
        wx.y * tap(base + vec2(0.0, 1.0) * texel, lo, hi) +
        wx.z * tap(base + vec2(1.0, 1.0) * texel, lo, hi) +
        wx.w * tap(base + vec2(2.0, 1.0) * texel, lo, hi);
    vec4 row3 =
        wx.x * tap(base + vec2(-1.0, 2.0) * texel, lo, hi) +
        wx.y * tap(base + vec2(0.0, 2.0) * texel, lo, hi) +
        wx.z * tap(base + vec2(1.0, 2.0) * texel, lo, hi) +
        wx.w * tap(base + vec2(2.0, 2.0) * texel, lo, hi);

    return wy.x * row0 + wy.y * row1 + wy.z * row2 + wy.w * row3;
}

void main() {
    vec2 outer_size = max(rect_size, vec2(1.0));

    // Remap the texture coordinate to dst-normalised space so the rounding geometry
    // is independent of any `src` crop (see src_uv_* docs above).
    vec2 dst_uv = (src_uv_scale.x > 0.0 && src_uv_scale.y > 0.0)
        ? (v_coords - src_uv_offset) / src_uv_scale
        : v_coords;

    // Pixel position within the dst rect.
    vec2 px = dst_uv * outer_size;

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

    vec4 sampled;
    if (magnify > 1.0 && tex_size.x > 0.0 && tex_size.y > 0.0) {
        // Clamp neighbour taps to the rendered src sub-rect so bicubic/sharpen never
        // bleed content from outside the window (matches the src_uv_* crop convention).
        bool has_crop = src_uv_scale.x > 0.0 && src_uv_scale.y > 0.0;
        vec2 uv_lo = has_crop ? src_uv_offset : vec2(0.0);
        vec2 uv_hi = has_crop ? (src_uv_offset + src_uv_scale) : vec2(1.0);
        vec2 sample_coord = clamp(v_coords, uv_lo, uv_hi);

        sampled = sample_bicubic(sample_coord, uv_lo, uv_hi);

        if (sharpen > 0.0) {
            vec2 texel = 1.0 / tex_size;
            vec4 neigh =
                tap(sample_coord + vec2(texel.x, 0.0), uv_lo, uv_hi) +
                tap(sample_coord - vec2(texel.x, 0.0), uv_lo, uv_hi) +
                tap(sample_coord + vec2(0.0, texel.y), uv_lo, uv_hi) +
                tap(sample_coord - vec2(0.0, texel.y), uv_lo, uv_hi);
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

    float base_content_a = sampled.a * content_alpha_scale;
    vec4 base_content = sampled * content_alpha_scale;
    vec4 base_composed = base_content + fill_color * max(1.0 - base_content_a, 0.0);
    vec4 composed = base_composed * inner_alpha;
    vec4 border_fill = border_color * border_alpha;

    gl_FragColor = (border_fill + composed) * alpha;
}

