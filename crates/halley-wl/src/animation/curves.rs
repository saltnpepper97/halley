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
