
// Minimal offscreen cleanup shader for cached window textures.
// This targets transparent-edge RGB contamination (dark GTK corner wedges)
// without needing any extra uniforms.
//
// Compile with:
// renderer.compile_custom_texture_shader(include_str!("../../shaders/clipped_surface.frag"), &[])
//
// Then use the returned GlesTexProgram in frame.render_texture_from_to(..., Some(&program), &[])

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision highp float;

#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

void main() {
    vec4 color = texture2D(tex, v_coords);

#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0);
#endif

    // Kill black halos from transparent-edge texels by premultiplying sampled RGB.
    // This is the important bit for offscreen cached textures with rounded corners.
    color.rgb *= color.a;

    // Drop near-zero alpha junk so barely-visible edge noise disappears.
    if (color.a < 0.003) {
        color = vec4(0.0);
    }

    color *= alpha;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
