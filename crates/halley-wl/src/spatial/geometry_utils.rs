use crate::state::Halley;

pub(crate) fn screen_to_world(
    st: &Halley,
    w: i32,
    h: i32,
    sx: f32,
    sy: f32,
) -> halley_core::field::Vec2 {
    let w = (w as f32).max(1.0);
    let h = (h as f32).max(1.0);
    let view = st.camera_view_size();
    let nx = (sx / w) - 0.5;
    let ny = (sy / h) - 0.5;

    halley_core::field::Vec2 {
        x: st.model.viewport.center.x + nx * view.x.max(1.0),
        y: st.model.viewport.center.y + ny * view.y.max(1.0),
    }
}
