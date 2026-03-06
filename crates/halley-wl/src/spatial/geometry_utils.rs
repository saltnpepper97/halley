use crate::state::HalleyWlState;

pub(crate) fn screen_to_world(
    st: &HalleyWlState,
    w: i32,
    h: i32,
    sx: f32,
    sy: f32,
) -> halley_core::field::Vec2 {
    let w = (w as f32).max(1.0);
    let h = (h as f32).max(1.0);
    let nx = sx / w;
    let ny = sy / h;
    let wx = st.viewport.center.x + (nx - 0.5) * st.viewport.size.x;
    let wy = st.viewport.center.y + (0.5 - ny) * st.viewport.size.y;
    halley_core::field::Vec2 { x: wx, y: wy }
}
