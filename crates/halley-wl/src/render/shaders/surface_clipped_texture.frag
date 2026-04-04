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

    vec4 sampled = texture2D(tex, clamp(v_coords, vec2(0.0), vec2(1.0)));
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
