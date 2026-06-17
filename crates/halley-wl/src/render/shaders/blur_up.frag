precision highp float;
//_DEFINES

// Dual Kawase upsample pass. Samples the source texture (bound at a lower
// resolution) with an 8-tap tent and writes into a higher-resolution target.
// Paired with blur_down.frag to reconstruct a smooth, near-Gaussian blur.

varying vec2 v_coords;
uniform sampler2D tex;
uniform float alpha;

// Half-texel size of the SOURCE (lower-res) texture and the spread multiplier.
uniform vec2 halfpixel;
uniform float offset;

void main() {
    vec2 uv = clamp(v_coords, vec2(0.0), vec2(1.0));
    vec2 o = halfpixel * offset;

    vec4 sum = texture2D(tex, uv + vec2(-o.x * 2.0, 0.0));
    sum += texture2D(tex, uv + vec2(-o.x, o.y)) * 2.0;
    sum += texture2D(tex, uv + vec2(0.0, o.y * 2.0));
    sum += texture2D(tex, uv + vec2(o.x, o.y)) * 2.0;
    sum += texture2D(tex, uv + vec2(o.x * 2.0, 0.0));
    sum += texture2D(tex, uv + vec2(o.x, -o.y)) * 2.0;
    sum += texture2D(tex, uv + vec2(0.0, -o.y * 2.0));
    sum += texture2D(tex, uv + vec2(-o.x, -o.y)) * 2.0;

    gl_FragColor = (sum / 12.0) * alpha;
}
