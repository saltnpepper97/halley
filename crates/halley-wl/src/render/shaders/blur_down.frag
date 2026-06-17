precision highp float;
//_DEFINES

// Dual Kawase downsample pass. Samples the source texture (bound at a higher
// resolution) with a 4-tap diamond plus center and writes the average into a
// half-resolution target. Run repeatedly down a mip chain, then reverse with
// blur_up.frag, to approximate a wide Gaussian cheaply.

varying vec2 v_coords;
uniform sampler2D tex;
uniform float alpha;

// Half-texel size of the SOURCE texture (0.5 / source_size) and a scalar spread
// multiplier derived from the configured blur radius.
uniform vec2 halfpixel;
uniform float offset;

void main() {
    vec2 uv = clamp(v_coords, vec2(0.0), vec2(1.0));
    vec2 o = halfpixel * offset;

    vec4 sum = texture2D(tex, uv) * 4.0;
    sum += texture2D(tex, uv - o);
    sum += texture2D(tex, uv + o);
    sum += texture2D(tex, uv + vec2(o.x, -o.y));
    sum += texture2D(tex, uv - vec2(o.x, -o.y));

    gl_FragColor = (sum / 8.0) * alpha;
}
