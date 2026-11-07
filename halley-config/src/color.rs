//! The one hex colour parser every config section shares.
//!
//! Accepted forms, with or without the leading `#` and ignoring surrounding
//! whitespace: `rgb`, `rgba`, `rrggbb` and `rrggbbaa`. Anything else is
//! rejected, including non-ASCII text, which must never reach byte slicing.

/// A colour with each component in `0.0..=1.0`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

/// Parse a hex colour in any accepted form. Short forms repeat each digit, so
/// `#f80` is `#ff8800`; forms without alpha are fully opaque.
pub(crate) fn parse_hex(value: &str) -> Option<Rgba> {
    let value = value.trim();
    let hex = value.strip_prefix('#').unwrap_or(value).as_bytes();
    if !hex.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let digit = |index: usize| (hex[index] as char).to_digit(16).map(|digit| digit as u8);
    let short = |index: usize| digit(index).map(|digit| digit * 17);
    let pair = |index: usize| Some(digit(index)? * 16 + digit(index + 1)?);
    let [r, g, b, a] = match hex.len() {
        3 => [short(0)?, short(1)?, short(2)?, 255],
        4 => [short(0)?, short(1)?, short(2)?, short(3)?],
        6 => [pair(0)?, pair(2)?, pair(4)?, 255],
        8 => [pair(0)?, pair(2)?, pair(4)?, pair(6)?],
        _ => return None,
    };
    let unit = |channel: u8| channel as f32 / 255.0;
    Some(Rgba {
        r: unit(r),
        g: unit(g),
        b: unit(b),
        a: unit(a),
    })
}

/// Parse a hex colour for a setting that has no alpha channel. The alpha
/// forms are rejected rather than silently ignored, so a config never appears
/// to set a transparency that Halley will not draw.
pub(crate) fn parse_hex_rgb(value: &str) -> Option<[f32; 3]> {
    let value = value.trim();
    let digits = value.strip_prefix('#').unwrap_or(value).len();
    if digits != 3 && digits != 6 {
        return None;
    }
    parse_hex(value).map(|Rgba { r, g, b, .. }| [r, g, b])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(value: &str) -> Option<[u8; 4]> {
        parse_hex(value)
            .map(|Rgba { r, g, b, a }| [r, g, b, a].map(|channel| (channel * 255.0).round() as u8))
    }

    #[test]
    fn accepts_every_documented_form() {
        assert_eq!(rgba("#d65d26"), Some([0xd6, 0x5d, 0x26, 0xff]));
        assert_eq!(rgba("#D65D26"), Some([0xd6, 0x5d, 0x26, 0xff]));
        assert_eq!(rgba("#d65d2680"), Some([0xd6, 0x5d, 0x26, 0x80]));
        assert_eq!(rgba("#f80"), Some([0xff, 0x88, 0x00, 0xff]));
        assert_eq!(rgba("#f808"), Some([0xff, 0x88, 0x00, 0x88]));
    }

    #[test]
    fn the_hash_and_surrounding_spaces_are_optional() {
        assert_eq!(rgba("d65d26"), rgba("#d65d26"));
        assert_eq!(rgba("  #d65d26 "), rgba("#d65d26"));
    }

    /// Byte slicing used to panic on these, which crashed `config verify` and
    /// a live reload.
    #[test]
    fn non_ascii_text_is_rejected_without_panicking() {
        for value in ["#aéaaa", "#ééé", "é", "#ffffé", "#🎨🎨", "#ａｂｃ"] {
            assert_eq!(parse_hex(value), None, "{value:?}");
            assert_eq!(parse_hex_rgb(value), None, "{value:?}");
        }
    }

    /// Every hex colour the generated config sets, replaced in turn with text
    /// that used to panic, must load or fail cleanly. New colour settings are
    /// covered automatically because the template lists them.
    #[test]
    fn every_colour_setting_survives_non_ascii_text() {
        let lines = crate::DEFAULT_CONFIG.lines().collect::<Vec<_>>();
        let mut checked = 0;
        for (index, line) in lines.iter().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with('#') {
                continue;
            }
            let Some(start) = line.find("\"#") else {
                continue;
            };
            let Some(length) = line[start + 1..].find('"') else {
                continue;
            };
            let mut edited = lines.clone();
            let replaced = format!(
                "{}\"#aéaaa\"{}",
                &line[..start],
                &line[start + length + 2..]
            );
            edited[index] = &replaced;
            let source = edited.join("\n");
            let result = std::panic::catch_unwind(|| {
                rune_cfg::RuneConfig::from_str(&source)
                    .map(|config| crate::parse_runtime_config(&config).is_ok())
            });
            assert!(result.is_ok(), "line {}: {line:?} panicked", index + 1);
            checked += 1;
        }
        assert!(checked >= 15, "only {checked} colour settings found");
    }

    #[test]
    fn malformed_values_are_rejected() {
        for value in [
            "",
            "#",
            "#ff",
            "#fffff",
            "#fffffff",
            "#fffffffff",
            "#ggg",
            "#+f+f+f",
            "##fff",
            "#ff ff ff",
            "auto",
        ] {
            assert_eq!(parse_hex(value), None, "{value:?}");
        }
    }

    #[test]
    fn rgb_settings_reject_alpha_forms() {
        assert_eq!(parse_hex_rgb("#f80"), Some([1.0, 0x88 as f32 / 255.0, 0.0]));
        assert!(parse_hex_rgb("#d65d26").is_some());
        assert_eq!(parse_hex_rgb("#f808"), None);
        assert_eq!(parse_hex_rgb("#d65d2680"), None);
    }
}
