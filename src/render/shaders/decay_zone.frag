precision highp float;
//_DEFINES

varying vec2 v_coords;
uniform sampler2D tex;
uniform float alpha;
uniform vec4 ring_color;
uniform vec2 rect_size;
uniform vec2 radii;
uniform float thickness;
uniform float dash_period;
uniform float dash_length;

const float PI = 3.14159265358979323846;

/// Gradient-normalised distance to the ellipse boundary. Exact enough for a
/// thin ring and far cheaper than an iterative closest-point solve.
float ellipse_distance(vec2 p, vec2 r) {
    vec2 pr = p / r;
    vec2 prr = pr / r;
    float g = length(prr);
    if (g < 1e-6) {
        return -min(r.x, r.y);
    }
    return (length(pr) - 1.0) / g;
}

void main() {
    vec2 size = max(rect_size, vec2(1.0));
    vec2 r = max(radii, vec2(1.0));
    vec2 p = v_coords * size - size * 0.5;

    // Radial falloff: keep only a band of `thickness` centred on the ellipse.
    float dist = abs(ellipse_distance(p, r));
    float half_thickness = max(thickness, 1.0) * 0.5;
    float band = 1.0 - smoothstep(half_thickness - 0.75, half_thickness + 0.75, dist);
    if (band <= 0.0) { discard; }

    // Dash falloff along the perimeter. Using the mean radius to convert the
    // polar angle into an arc length keeps dash spacing even on an ellipse.
    float mean_radius = (r.x + r.y) * 0.5;
    float arc = atan(p.y, p.x) * mean_radius;
    float period = max(dash_period, 1.0);
    float phase = mod(arc, period);
    float on = clamp(dash_length, 1.0, period);
    float dash = (1.0 - smoothstep(on - 0.75, on + 0.75, phase))
        * smoothstep(-0.75, 0.75, phase);

    // The seam at atan's +/-pi wrap would otherwise cut a dash in half; fade
    // the two ends together across one dash so the join is invisible.
    float seam = smoothstep(0.0, 1.5, abs(abs(arc) - PI * mean_radius));
    dash = mix(1.0, dash, seam);

    float coverage = band * dash;
    if (coverage <= 0.0) { discard; }
    gl_FragColor = ring_color * (coverage * alpha);
}
