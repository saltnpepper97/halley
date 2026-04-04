use rune_cfg::RuneConfig;

use crate::keybinds::parse_modifiers;
use crate::layout::RuntimeTuning;

use super::super::keybinds::apply_explicit_keybind_overrides;

pub(crate) fn load_keybind_sections(
    cfg: &RuneConfig,
    out: &mut RuntimeTuning,
) -> Result<(), String> {
    if let Ok(Some(raw)) = cfg.get_optional::<String>("keybinds.mod") {
        let Some(modifiers) = parse_modifiers(raw.as_str()) else {
            return Err(format!("invalid keybind modifier: {raw}"));
        };
        out.keybinds.modifier = modifiers;
    }
    out.compositor_bindings.clear();
    out.launch_bindings.clear();
    out.pointer_bindings.clear();
    apply_explicit_keybind_overrides(cfg, out)
}
