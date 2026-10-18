precision highp float;
//_DEFINES

varying vec2 v_coords;
uniform sampler2D tex;
uniform vec2 pixel_step;
uniform float center_weight;
// Positive taps paired using linear texture filtering: (offset, weight).
uniform vec2 kernel[8];

void main() {
    vec4 sum = texture2D(tex, v_coords) * center_weight;
    for (int i = 0; i < 8; i++) {
        if (kernel[i].y > 0.0) {
            vec2 delta = pixel_step * kernel[i].x;
            sum += (texture2D(tex, v_coords + delta)
                  + texture2D(tex, v_coords - delta)) * kernel[i].y;
        }
    }
    gl_FragColor = sum;
}
