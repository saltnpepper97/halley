use rune_cfg::RuneConfig;

use crate::layout::{RuntimeTuning, default_compositor_bindings, default_pointer_bindings};

use super::super::keybinds::apply_explicit_keybind_overrides;
use super::super::primitives::pick_modifiers;

pub(crate) fn load_keybind_sections(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.keybinds.modifier = pick_modifiers(cfg, &["keybinds.mod"], out.keybinds.modifier);
    out.compositor_bindings = default_compositor_bindings(out.keybinds.modifier);
    out.launch_bindings.clear();
    out.pointer_bindings = default_pointer_bindings(out.keybinds.modifier);
    apply_explicit_keybind_overrides(cfg, out);
}
