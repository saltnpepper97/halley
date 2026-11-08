/// Backend-independent settings retained by a running compositor session.
///
/// Configuration parsing remains in `halley-config`; this aggregate owns the
/// applied values and their reload lifecycle. Keeping them together prevents
/// the session coordinator from exposing each setting as unrelated mutable
/// state.
pub struct RuntimeSettings {
    pub overlays: halley_config::Overlays,
    configured_overlays: halley_config::Overlays,
    system_color_scheme: halley_config::SystemColorScheme,
    pub apogee: halley_config::Apogee,
    pub input: halley_config::Input,
    pub animations: halley_config::Animations,
    pub decorations: halley_config::Decorations,
    pub font: halley_config::Font,
    pub effects: halley_config::Effects,
    pub background: halley_config::Background,
    pub field: halley_config::Field,
    pub zoom: halley_config::Zoom,
    pub screenshot: halley_config::Screenshot,
    pub debug: halley_config::Debug,
    pub layer_rules: Vec<halley_config::LayerRule>,
}

impl RuntimeSettings {
    pub fn new(config: &halley_config::RuntimeConfig, applied_input: halley_config::Input) -> Self {
        Self::new_with_color_scheme(
            config,
            applied_input,
            halley_config::SystemColorScheme::NoPreference,
        )
    }

    pub fn new_with_color_scheme(
        config: &halley_config::RuntimeConfig,
        applied_input: halley_config::Input,
        system_color_scheme: halley_config::SystemColorScheme,
    ) -> Self {
        Self {
            overlays: config.overlays.resolved_for_system(system_color_scheme),
            configured_overlays: config.overlays,
            system_color_scheme,
            apogee: config.apogee,
            input: applied_input,
            animations: config.animations.clone(),
            decorations: config.decorations,
            font: config.font.clone(),
            effects: config.effects,
            background: config.background.clone(),
            field: config.field,
            zoom: config.field.zoom,
            screenshot: config.screenshot.clone(),
            debug: config.debug,
            layer_rules: config.layer_rules.clone(),
        }
    }

    pub fn visuals_changed(&self, config: &halley_config::RuntimeConfig) -> bool {
        self.decorations != config.decorations
            || self.animations != config.animations
            || self.font != config.font
            || self.effects != config.effects
            || self.background != config.background
            || self.zoom != config.field.zoom
            || self.field.pins != config.field.pins
            || self.configured_overlays != config.overlays
            || self.debug.show_decay_zone != config.debug.show_decay_zone
            || self.debug.overlay_fps != config.debug.overlay_fps
            || self.layer_rules != config.layer_rules
    }

    /// Replaces settings that do not need protocol-specific application.
    /// Input is applied separately so a failed XKB reload can retain the last
    /// working keyboard configuration.
    pub fn reload_non_input(&mut self, config: &halley_config::RuntimeConfig) {
        self.apogee = config.apogee;
        self.animations = config.animations.clone();
        self.decorations = config.decorations;
        self.font = config.font.clone();
        self.effects = config.effects;
        self.background = config.background.clone();
        self.field = config.field;
        self.configured_overlays = config.overlays;
        self.overlays = config
            .overlays
            .resolved_for_system(self.system_color_scheme);
        self.zoom = config.field.zoom;
        self.screenshot = config.screenshot.clone();
        // Protocol advertisement is fixed at session startup. Preserve its
        // applied value while allowing the visual debug settings to reload.
        self.debug.show_decay_zone = config.debug.show_decay_zone;
        self.debug.overlay_fps = config.debug.overlay_fps;
        self.layer_rules.clone_from(&config.layer_rules);
    }

    pub fn set_system_color_scheme(&mut self, scheme: halley_config::SystemColorScheme) -> bool {
        if self.system_color_scheme == scheme {
            return false;
        }
        self.system_color_scheme = scheme;
        let overlays = self.configured_overlays.resolved_for_system(scheme);
        let changed = self.overlays != overlays;
        self.overlays = overlays;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::RuntimeSettings;

    #[test]
    fn reload_preserves_explicit_sync_policy_until_a_new_session() {
        for initially_disabled in [false, true] {
            let mut config = halley_config::RuntimeConfig::default();
            config.debug.disable_explicit_sync = initially_disabled;
            let mut settings = RuntimeSettings::new(&config, config.input.clone());
            config.debug.disable_explicit_sync = !initially_disabled;

            assert!(!settings.visuals_changed(&config));
            config.debug.overlay_fps = true;
            config.debug.show_decay_zone = true;
            settings.reload_non_input(&config);
            assert_eq!(settings.debug.disable_explicit_sync, initially_disabled);
            assert!(settings.debug.overlay_fps);
            assert!(settings.debug.show_decay_zone);
            assert!(!settings.visuals_changed(&config));

            let restarted = RuntimeSettings::new(&config, config.input.clone());
            assert_eq!(restarted.debug.disable_explicit_sync, !initially_disabled);
        }
    }

    #[test]
    fn system_scheme_updates_only_explicit_system_colours() {
        let mut config = halley_config::RuntimeConfig::default();
        config.overlays.background_color = halley_config::OverlayColorMode::System;
        let mut settings = RuntimeSettings::new_with_color_scheme(
            &config,
            config.input.clone(),
            halley_config::SystemColorScheme::PreferDark,
        );
        assert_eq!(
            settings.overlays.background_color,
            halley_config::OverlayColorMode::Dark
        );
        assert!(!settings.set_system_color_scheme(halley_config::SystemColorScheme::PreferDark));
        assert!(settings.set_system_color_scheme(halley_config::SystemColorScheme::PreferLight));
        assert_eq!(
            settings.overlays.background_color,
            halley_config::OverlayColorMode::Light
        );
        assert_eq!(
            settings.overlays.border_color,
            halley_config::Overlays::default().border_color
        );
    }

    #[test]
    fn visual_change_detection_ignores_non_visual_settings() {
        let config = halley_config::RuntimeConfig::default();
        let mut settings = RuntimeSettings::new(&config, config.input.clone());
        let mut changed = config.clone();
        changed.screenshot.directory = "different".into();

        assert!(!settings.visuals_changed(&changed));

        changed.debug.overlay_fps = true;
        assert!(settings.visuals_changed(&changed));

        settings.reload_non_input(&changed);
        assert!(!settings.visuals_changed(&changed));

        changed.field.pins.size = 1.5;
        assert!(settings.visuals_changed(&changed));

        changed.decorations.border_width_px += 1;
        assert!(settings.visuals_changed(&changed));

        settings.reload_non_input(&changed);
        assert!(!settings.visuals_changed(&changed));
    }
}
