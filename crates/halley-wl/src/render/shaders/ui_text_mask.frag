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
uniform vec4 text_color;

void main() {
    vec4 sampled = texture2D(tex, clamp(v_coords, vec2(0.0), vec2(1.0)));
    if (sampled.a <= 0.003) {
        discard;
    }

    float out_alpha = text_color.a * sampled.a * alpha;
    gl_FragColor = vec4(text_color.rgb * out_alpha, out_alpha);
}
