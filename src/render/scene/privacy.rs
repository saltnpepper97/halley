//! Capture privacy decisions shared by scene composition stages.

use crate::window::rules::ResolvedWindowRule;

/// Rules are resolved before composition so every scene stage uses the same
/// capture mode and conservative treatment of cached content.
#[derive(Clone, Copy)]
pub(super) struct CapturePrivacy {
    enabled: bool,
    screenshot: bool,
    window_rules: bool,
    layer_rules: bool,
}

impl CapturePrivacy {
    pub(super) fn new(
        enabled: bool,
        screenshot: bool,
        window_rules: bool,
        layer_rules: bool,
    ) -> Self {
        Self {
            enabled,
            screenshot,
            window_rules,
            layer_rules,
        }
    }

    pub(super) fn enabled(self) -> bool {
        self.enabled
    }

    pub(super) fn redact_layers(self) -> bool {
        self.enabled && !self.screenshot
    }

    pub(super) fn redact_cached_windows(self) -> bool {
        self.enabled && self.window_rules
    }

    pub(super) fn redact_cached_scene(self) -> bool {
        self.enabled && (self.window_rules || (!self.screenshot && self.layer_rules))
    }

    pub(super) fn redact_window(
        self,
        rule: Option<&ResolvedWindowRule>,
        override_redirect: bool,
    ) -> bool {
        self.enabled
            && (rule.is_some_and(|rule| rule.blocks_capture(self.screenshot))
                || (self.window_rules && override_redirect))
    }

    pub(super) fn redact_rule(self, rule: &ResolvedWindowRule) -> bool {
        self.enabled && rule.blocks_capture(self.screenshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_matches_capture_rules_for_every_mode() {
        for enabled in [false, true] {
            for screenshot in [false, true] {
                for window_rules in [false, true] {
                    for layer_rules in [false, true] {
                        let policy =
                            CapturePrivacy::new(enabled, screenshot, window_rules, layer_rules);
                        assert_eq!(policy.redact_layers(), enabled && !screenshot);
                        assert_eq!(policy.redact_cached_windows(), enabled && window_rules);
                        assert_eq!(
                            policy.redact_cached_scene(),
                            enabled && (window_rules || (!screenshot && layer_rules))
                        );
                        for blocked in [false, true] {
                            let rule = ResolvedWindowRule {
                                block_screenshot: blocked,
                                block_screencast: blocked,
                                ..Default::default()
                            };
                            for override_redirect in [false, true] {
                                assert_eq!(
                                    policy.redact_window(Some(&rule), override_redirect),
                                    enabled && (blocked || (window_rules && override_redirect))
                                );
                                assert_eq!(
                                    policy.redact_window(None, override_redirect),
                                    enabled && window_rules && override_redirect
                                );
                                assert_eq!(policy.redact_rule(&rule), enabled && blocked);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn capture_modes_keep_window_and_layer_rules_separate() {
        let cast_only = ResolvedWindowRule {
            block_screencast: true,
            ..Default::default()
        };
        let screenshot_only = ResolvedWindowRule {
            block_screenshot: true,
            ..Default::default()
        };
        let ordinary = ResolvedWindowRule::default();
        let normal = CapturePrivacy::new(false, false, true, true);
        assert!(!normal.redact_window(Some(&cast_only), true));
        assert!(!normal.redact_cached_scene());
        assert!(!normal.redact_layers());
        let cast = CapturePrivacy::new(true, false, true, true);
        assert!(cast.redact_window(Some(&cast_only), false));
        assert!(!cast.redact_window(Some(&screenshot_only), false));
        assert!(cast.redact_window(Some(&ordinary), true));
        assert!(cast.redact_cached_windows());
        assert!(cast.redact_cached_scene());
        assert!(cast.redact_layers());
        let screenshot = CapturePrivacy::new(true, true, true, true);
        assert!(screenshot.redact_window(Some(&screenshot_only), false));
        assert!(!screenshot.redact_window(Some(&cast_only), false));
        assert!(screenshot.redact_window(Some(&ordinary), true));
        assert!(screenshot.redact_cached_windows());
        assert!(screenshot.redact_cached_scene());
        assert!(!screenshot.redact_layers());
        let layer_only = CapturePrivacy::new(true, false, false, true);
        assert!(layer_only.redact_cached_scene());
        assert!(!layer_only.redact_cached_windows());
        assert!(!layer_only.redact_window(Some(&ordinary), true));
        let screenshot_layer_only = CapturePrivacy::new(true, true, false, true);
        assert!(!screenshot_layer_only.redact_cached_scene());
    }
}
