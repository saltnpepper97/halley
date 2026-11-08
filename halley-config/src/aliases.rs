//! Every older config key spelling Halley still reads, in one place.
//!
//! Lookups name only a setting's current key; [`spellings`] expands it into
//! every spelling that sets the same value. Old spellings keep working, but
//! new ones are never added here: documentation and generated configs use the
//! current key only.
//!
//! `snake_case` needs no entry: rune-cfg already matches `bg_colour` against
//! `bg-colour` in every key segment.

use rune_cfg::{RuneConfig, RuneError, Value};

/// Sections that were renamed. Every key under the old prefix reads the
/// setting under the new one, e.g. `gesso.mode` sets `wallpaper.mode`.
pub(crate) const SECTION_RENAMES: &[(&str, &str)] = &[
    ("background.", "wallpaper."),
    ("gesso.", "wallpaper."),
    ("overlay.", "overlays."),
    ("tile.", "clusters.tiling."),
    ("stacking.", "clusters.stacking."),
];

/// Individual keys that were renamed, as `(old, current)`. An old key inside a
/// renamed section is written with its old section, e.g. `tile.queue-show-icons`.
pub(crate) const KEY_RENAMES: &[(&str, &str)] = &[
    ("clusters.cluster-dwell-ms", "clusters.join-dwell-ms"),
    ("clusters.dwell-ms", "clusters.join-dwell-ms"),
    ("clusters.distance-px", "clusters.join-distance-px"),
    (
        "tile.queue-show-icons",
        "clusters.tiling.overflow-show-icons",
    ),
    ("field.pins.badge-corner", "field.pins.corner"),
    ("field.pins.pin-colour", "field.pins.colour"),
    ("field.pins.bg-colour", "field.pins.background-colour"),
    ("debug.show-focus-ring", "debug.show-decay-zone"),
];

/// Every key that sets the same value as `key`, starting with `key` itself.
///
/// The American `color` spelling of any `colour` key is accepted everywhere,
/// as documented. Old keys and old sections from the tables above follow.
pub(crate) fn spellings(key: &str) -> Vec<String> {
    let mut keys = vec![key.to_string()];
    keys.extend(
        KEY_RENAMES
            .iter()
            .filter(|(_, current)| *current == key)
            .map(|(old, _)| old.to_string()),
    );
    for (old_prefix, new_prefix) in SECTION_RENAMES {
        let renamed = keys
            .iter()
            .filter_map(|key| key.strip_prefix(new_prefix))
            .map(|rest| format!("{old_prefix}{rest}"))
            .collect::<Vec<_>>();
        keys.extend(renamed);
    }
    let mut spelled = Vec::with_capacity(keys.len() * 2);
    for key in keys {
        let american = key
            .contains("colour")
            .then(|| key.replace("colour", "color"));
        spelled.push(key);
        spelled.extend(american);
    }
    let mut seen = std::collections::HashSet::new();
    spelled.retain(|key| seen.insert(key.clone()));
    spelled
}

/// The value of the first spelling of `key` that the config sets.
pub(crate) fn lookup<T>(config: &RuneConfig, key: &str) -> Result<Option<T>, RuneError>
where
    T: TryFrom<Value, Error = RuneError>,
{
    for path in spellings(key) {
        if let Some(value) = config.get_optional::<T>(&path)? {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Each setting with the spellings Halley accepted before the table
    /// existed. The table must keep every one of them.
    const ACCEPTED_BEFORE: &[&[&str]] = &[
        &["wallpaper.mode", "background.mode", "gesso.mode"],
        &["wallpaper.fit", "background.fit", "gesso.fit"],
        &[
            "wallpaper.intensity",
            "background.intensity",
            "gesso.intensity",
        ],
        &["wallpaper.path", "background.path", "gesso.path"],
        &["wallpaper.shader", "background.shader", "gesso.shader"],
        &[
            "wallpaper.animated",
            "background.animated",
            "gesso.animated",
        ],
        &[
            "wallpaper.colour",
            "wallpaper.color",
            "background.colour",
            "background.color",
            "gesso.colour",
            "gesso.color",
        ],
        &[
            "wallpaper.accent-colour",
            "wallpaper.accent_colour",
            "wallpaper.accent-color",
            "wallpaper.accent_color",
            "background.accent-colour",
            "background.accent_colour",
            "background.accent-color",
            "background.accent_color",
            "gesso.accent-colour",
            "gesso.accent_colour",
            "gesso.accent-color",
            "gesso.accent_color",
        ],
        &["clusters.default-layout"],
        &["clusters.bloom-direction"],
        &[
            "clusters.join-dwell-ms",
            "clusters.cluster-dwell-ms",
            "clusters.dwell-ms",
        ],
        &["clusters.join-distance-px", "clusters.distance-px"],
        &["clusters.show-icons"],
        &["clusters.tiling.new-on-top", "tile.new-on-top"],
        &["clusters.tiling.gaps-inner", "tile.gaps-inner"],
        &["clusters.tiling.gaps-outer", "tile.gaps-outer"],
        &["clusters.tiling.max-stack", "tile.max-stack"],
        &[
            "clusters.tiling.overflow-show-icons",
            "tile.queue-show-icons",
        ],
        &["clusters.stacking.max-visible", "stacking.max-visible"],
        &[
            "decorations.border.colour-focused",
            "decorations.border.color-focused",
        ],
        &[
            "decorations.border.colour-unfocused",
            "decorations.border.color-unfocused",
        ],
        &[
            "decorations.titlebars.colour-focused",
            "decorations.titlebars.color-focused",
        ],
        &[
            "decorations.titlebars.colour-unfocused",
            "decorations.titlebars.color-unfocused",
        ],
        &[
            "decorations.titlebars.foreground-colour-focused",
            "decorations.titlebars.foreground-color-focused",
        ],
        &[
            "decorations.titlebars.foreground-colour-unfocused",
            "decorations.titlebars.foreground-color-unfocused",
        ],
        &[
            "decorations.titlebars.button-hover-colour",
            "decorations.titlebars.button-hover-color",
        ],
        &[
            "decorations.titlebars.button-pressed-colour",
            "decorations.titlebars.button-pressed-color",
        ],
        &[
            "decorations.screencast-indicator.colour",
            "decorations.screencast-indicator.color",
        ],
        &[
            "field.pins.corner",
            "field.pins.badge-corner",
            "field.pins.badge_corner",
        ],
        &[
            "field.pins.colour",
            "field.pins.color",
            "field.pins.pin-colour",
            "field.pins.pin_color",
            "field.pins.pin-color",
        ],
        &[
            "field.pins.background-colour",
            "field.pins.background_colour",
            "field.pins.background-color",
            "field.pins.background_color",
            "field.pins.bg-colour",
            "field.pins.bg_colour",
            "field.pins.bg-color",
            "field.pins.bg_color",
        ],
        &["node.border-colour", "node.border-color"],
        &[
            "node.border-colour-highlighted",
            "node.border-color-highlighted",
        ],
        &[
            "overlays.background-colour",
            "overlays.background-color",
            "overlay.background-colour",
            "overlay.background-color",
        ],
        &[
            "overlays.text-colour",
            "overlays.text-color",
            "overlay.text-colour",
            "overlay.text-color",
        ],
        &[
            "overlays.error-colour",
            "overlays.error-color",
            "overlay.error-colour",
            "overlay.error-color",
        ],
        &[
            "overlays.border-colour",
            "overlays.border-color",
            "overlay.border-colour",
            "overlay.border-color",
        ],
        &["overlays.radius", "overlay.radius"],
        &[
            "overlays.notifications.position",
            "overlay.notifications.position",
        ],
        &["overlays.zoom-indicator.position"],
        &["overlays.zoom-indicator.hold-duration-ms"],
        &["overlays.zoom-indicator.fade-duration-ms"],
        &[
            "overlays.zoom-indicator.text-colour",
            "overlays.zoom-indicator.text-color",
        ],
        &[
            "overlays.zoom-indicator.background-colour",
            "overlays.zoom-indicator.background-color",
        ],
        &[
            "overlays.zoom-indicator.border-colour",
            "overlays.zoom-indicator.border-color",
        ],
        &[
            "overlays.notifications.success-duration-ms",
            "overlay.notifications.success-duration-ms",
        ],
        &[
            "overlays.notifications.error-duration-ms",
            "overlay.notifications.error-duration-ms",
        ],
        &["overlays.borders", "overlay.borders"],
        &["overlays.border-size", "overlay.border-size"],
        &[
            "overlays.notifications.offset-x",
            "overlay.notifications.offset-x",
        ],
        &[
            "overlays.notifications.offset-y",
            "overlay.notifications.offset-y",
        ],
        &["overlays.zoom-indicator.enabled"],
        &["overlays.zoom-indicator.background"],
        &["overlays.zoom-indicator.text-size"],
        &["overlays.zoom-indicator.opacity"],
        &["overlays.zoom-indicator.borders"],
        &["overlays.zoom-indicator.radius"],
    ];

    /// No spelling is lost. The only new ones are old section names applied
    /// to keys added after the rename, so `overlay.` now reaches every
    /// `overlays.` key instead of only the older ones.
    #[test]
    fn the_table_keeps_every_spelling_accepted_before() {
        for accepted in ACCEPTED_BEFORE {
            // rune-cfg matches `_` and `-` alike, so compare them that way.
            let expected = accepted
                .iter()
                .map(|key| key.replace('_', "-"))
                .collect::<BTreeSet<_>>();
            let derived = spellings(accepted[0]).into_iter().collect::<BTreeSet<_>>();
            let lost = expected.difference(&derived).collect::<Vec<_>>();
            assert!(lost.is_empty(), "{} lost {lost:?}", accepted[0]);
            for added in derived.difference(&expected) {
                assert!(
                    SECTION_RENAMES
                        .iter()
                        .any(|(old_prefix, _)| added.starts_with(old_prefix)),
                    "{} gained {added}, which is not under an old section",
                    accepted[0]
                );
            }
        }
    }

    /// A config written only with old spellings still sets every value.
    #[test]
    fn old_spellings_still_configure_halley() {
        let config = rune_cfg::RuneConfig::from_str(
            r##"
keybinds:
  mod "super"
end
gesso:
  intensity 0.25
  color "#102030"
end
overlay:
  radius 7
end
clusters:
  dwell-ms 900
  distance-px 64
end
tile:
  gaps-inner 13
  queue-show-icons false
end
field:
  pins:
    badge_corner "top-left"
    pin_color "#00ff00"
    bg_colour "#ff0000"
  end
end
node:
  border-color "#010203"
end
"##,
        )
        .unwrap();
        let runtime = crate::parse_runtime_config(&config).unwrap();
        assert_eq!(runtime.background.intensity, 0.25);
        assert_eq!(
            runtime.background.color,
            crate::BackgroundColor {
                r: 0x10 as f32 / 255.0,
                g: 0x20 as f32 / 255.0,
                b: 0x30 as f32 / 255.0,
            }
        );
        assert_eq!(runtime.overlays.radius_px, 7);
        assert_eq!(runtime.clusters.join_dwell_ms, 900);
        assert_eq!(runtime.clusters.join_distance_px, 64.0);
        assert_eq!(runtime.clusters.tiling.gaps_inner_px, 13.0);
        assert!(!runtime.clusters.tiling.overflow_show_icons);
        assert_eq!(runtime.field.pins.corner, crate::PinBadgeCorner::TopLeft);
        let fixed = |r: f32, g: f32, b: f32| crate::OverlayColorMode::Fixed { r, g, b, a: 1.0 };
        assert_eq!(runtime.field.pins.color, fixed(0.0, 1.0, 0.0));
        assert_eq!(runtime.field.pins.background_color, fixed(1.0, 0.0, 0.0));
        assert_eq!(
            runtime.nodes.border_color,
            crate::BorderColor {
                r: 1.0 / 255.0,
                g: 2.0 / 255.0,
                b: 3.0 / 255.0,
            }
        );
    }

    #[test]
    fn the_current_key_is_always_tried_first() {
        for accepted in ACCEPTED_BEFORE {
            assert_eq!(spellings(accepted[0])[0], accepted[0]);
        }
    }

    #[test]
    fn every_old_key_points_at_a_current_key_that_is_not_itself_old() {
        for (old, current) in KEY_RENAMES {
            assert_ne!(old, current);
            assert!(
                KEY_RENAMES.iter().all(|(other, _)| other != current),
                "{current} is itself renamed"
            );
            assert!(
                SECTION_RENAMES
                    .iter()
                    .all(|(old_prefix, _)| !current.starts_with(old_prefix)),
                "{current} uses an old section"
            );
        }
    }
}
