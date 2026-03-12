use super::render_utils::preview_proxy_size;

#[inline]
pub(crate) fn active_surface_scale(anim_scale: f32, zoom_lock_scale: f32) -> f32 {
    let _ = zoom_lock_scale;
    // Keep live client surfaces at animation-only scale. This avoids
    // toolkit-specific compositor-scale artifacts (kitty residue on zoom-out).
    let raw = anim_scale.clamp(0.66, 1.10);
    if raw <= 1.0 {
        // Finer quantization below 1.0 so transition steps are smoother.
        (raw * 16.0).round() / 16.0
    } else {
        // Preserve overshoot/pulse above 1.0 so bump feedback is visible.
        raw
    }
}

#[inline]
pub(crate) fn active_surface_morph_scale(anim_scale: f32, real_w: f32, real_h: f32) -> f32 {
    // Preview -> Active should visually "unminimize":
    // start near preview proxy size, then smoothly reach full window size.
    let (pw, ph) = preview_proxy_size(real_w, real_h);
    let start = (pw / real_w.max(1.0))
        .min(ph / real_h.max(1.0))
        .clamp(0.24, 1.0);
    let t = ((anim_scale - 0.30) / (1.0 - 0.30)).clamp(0.0, 1.0);
    let e = ease_in_out_cubic(t);
    let mut out = start + (1.0 - start) * e;
    if anim_scale > 1.0 {
        out += (anim_scale - 1.0) * 0.35;
    }
    (out.clamp(0.24, 1.08) * 16.0).round() / 16.0
}

#[inline]
pub(crate) fn active_surface_render_scale(
    anim_scale: f32,
    zoom_lock_scale: f32,
    real_w: f32,
    real_h: f32,
    transition_alpha: f32,
) -> f32 {
    let quantized = active_surface_scale(anim_scale, zoom_lock_scale);
    if transition_alpha > 0.0 {
        let (pw, ph) = preview_proxy_size(real_w, real_h);
        let start = (pw / real_w.max(1.0))
            .min(ph / real_h.max(1.0))
            .clamp(0.24, 1.0);
        let t = (1.0 - transition_alpha).clamp(0.0, 1.0);
        let e = ease_out_back(t, 1.42).clamp(0.0, 1.08);
        start + (1.0 - start) * e
    } else {
        let morph = active_surface_morph_scale(anim_scale, real_w, real_h);
        if anim_scale < 1.0 {
            morph
        } else {
            quantized.min(morph)
        }
    }
}

#[inline]
pub(crate) fn proxy_anim_scale(anim_scale: f32) -> f32 {
    // Allow much more of the Active/Preview/Node transition range to be visible.
    anim_scale.clamp(0.22, 1.4)
}

pub(crate) fn ease_in_out_cubic(t: f32) -> f32 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powf(3.0) * 0.5
    }
}

pub(crate) fn ease_out_back(t: f32, s: f32) -> f32 {
    let u = t - 1.0;
    1.0 + u * u * ((s + 1.0) * u + s)
}
