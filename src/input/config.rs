use smithay::input::Seat;
use smithay::input::keyboard::XkbConfig;

use crate::session::{Session, SessionDriver};

pub fn xkb_config(keyboard: &halley_config::KeyboardConfig) -> XkbConfig<'_> {
    XkbConfig {
        rules: "",
        model: &keyboard.model,
        layout: &keyboard.layout,
        variant: &keyboard.variant,
        options: (!keyboard.options.is_empty()).then(|| keyboard.options.clone()),
    }
}

pub fn add_keyboard<D>(
    seat: &mut Seat<Session<D>>,
    input: &halley_config::Input,
) -> Result<halley_config::KeyboardConfig, smithay::input::keyboard::Error>
where
    D: SessionDriver,
{
    match seat.add_keyboard(
        xkb_config(&input.keyboard),
        input.repeat_delay,
        input.repeat_rate,
    ) {
        Ok(_) => Ok(input.keyboard.clone()),
        Err(_) => {
            let fallback = halley_config::KeyboardConfig::default();
            eventline::warn!(
                "input: failed to compile startup XKB layout {:?}; using {:?}",
                input.keyboard.layout,
                fallback.layout
            );
            seat.add_keyboard(xkb_config(&fallback), input.repeat_delay, input.repeat_rate)?;
            Ok(fallback)
        }
    }
}

/// Applies seat-level input policy while preserving the last working keymap
/// when a syntactically valid config names an XKB layout that cannot compile.
pub fn reload<D>(session: &mut Session<D>, requested: &halley_config::Input)
where
    D: SessionDriver,
{
    let keyboard = session
        .seat
        .get_keyboard()
        .expect("keyboard capability added at seat setup");
    let mut applied = requested.clone();

    if requested.keyboard != session.settings.input.keyboard {
        let previous_modifiers = keyboard.modifier_state();
        if keyboard
            .set_xkb_config(session, xkb_config(&requested.keyboard))
            .is_err()
        {
            eventline::warn!(
                "input: failed to compile XKB layout {:?}; keeping the last working keymap",
                requested.keyboard.layout
            );
            applied.keyboard = session.settings.input.keyboard.clone();
        } else {
            let mut modifiers = keyboard.modifier_state();
            if modifiers.caps_lock != previous_modifiers.caps_lock
                || modifiers.num_lock != previous_modifiers.num_lock
            {
                modifiers.caps_lock = previous_modifiers.caps_lock;
                modifiers.num_lock = previous_modifiers.num_lock;
                keyboard.set_modifier_state(modifiers);
                keyboard.advertise_modifier_state(session);
            }
        }
        // set_modifier_state updates XKB LEDs but has no SeatHandler callback.
        session.driver.update_keyboard_leds(keyboard.led_state());
    }

    if requested.repeat_rate != session.settings.input.repeat_rate
        || requested.repeat_delay != session.settings.input.repeat_delay
    {
        keyboard.change_repeat_info(requested.repeat_rate, requested.repeat_delay);
        session
            .xwayland
            .configure_key_repeat(requested.repeat_delay, requested.repeat_rate);
    }

    session.settings.input = applied;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_keyboard_config_without_forcing_empty_options() {
        let keyboard = halley_config::KeyboardConfig {
            layout: "ca".to_string(),
            variant: "multix".to_string(),
            options: String::new(),
            model: "pc105".to_string(),
        };
        let xkb = xkb_config(&keyboard);

        assert_eq!(xkb.layout, "ca");
        assert_eq!(xkb.variant, "multix");
        assert_eq!(xkb.model, "pc105");
        assert_eq!(xkb.options, None);
    }

    #[test]
    fn passes_configured_xkb_options_as_one_owned_value() {
        let keyboard = halley_config::KeyboardConfig {
            options: "caps:escape,compose:ralt".to_string(),
            ..halley_config::KeyboardConfig::default()
        };

        assert_eq!(
            xkb_config(&keyboard).options.as_deref(),
            Some("caps:escape,compose:ralt")
        );
    }
}
