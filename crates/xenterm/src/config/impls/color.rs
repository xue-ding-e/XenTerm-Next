//! Shared color input and storage, independent of the windowing toolkit.
//!
//! CSS colors with literal components and the HSV extension use `csscolorparser`;
//! finite RGB/HSL/HSV channels follow that parser's clamping behavior and hue
//! wraps around the color wheel. Nested color expressions are outside this field.
//! CMYK is an explicit-percentage, uncalibrated approximation, not an ICC profile.
//! Empty input is an error here: each setting decides what its empty default means.

use std::fmt;

use csscolorparser::Color;

pub(crate) const MAX_COLOR_INPUT_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ColorFormat {
    #[default]
    Hex,
    Rgb,
    Hsl,
    Hsv,
    Cmyk,
}

impl ColorFormat {
    pub(crate) const ALL: [Self; 5] = [Self::Hex, Self::Rgb, Self::Hsl, Self::Hsv, Self::Cmyk];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Hex => "HEX",
            Self::Rgb => "RGB / RGBA",
            Self::Hsl => "HSL / HSLA",
            Self::Hsv => "HSV / HSVA",
            Self::Cmyk => "CMYK (approx.)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColorParseError {
    Empty,
    TooLong,
    NonFinite,
    UnsupportedExpression,
    Css(csscolorparser::ParseColorError),
    InvalidCmyk,
    CmykOutOfRange,
}

impl fmt::Display for ColorParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("Enter a color"),
            Self::TooLong => write!(f, "Color input exceeds {MAX_COLOR_INPUT_BYTES} bytes"),
            Self::NonFinite => f.write_str("Color components must be finite numbers"),
            Self::UnsupportedExpression => f.write_str("Use a color with literal components"),
            Self::Css(error) => write!(f, "{error}"),
            Self::InvalidCmyk => f.write_str("Use cmyk(C% M% Y% K%) with an optional / alpha"),
            Self::CmykOutOfRange => f.write_str("CMYK must be 0–100%; alpha must be 0–1 or 0–100%"),
        }
    }
}

impl std::error::Error for ColorParseError {}

/// Canonical straight-alpha sRGB bytes. Parsing quantizes to the nearest byte once;
/// changing display format never changes the stored color or discards its alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ColorValue([u8; 4]);

impl ColorValue {
    pub(crate) fn parse(input: &str) -> Result<Self, ColorParseError> {
        if input.len() > MAX_COLOR_INPUT_BYTES {
            return Err(ColorParseError::TooLong);
        }
        let input = input.trim();
        if input.is_empty() {
            return Err(ColorParseError::Empty);
        }

        let color = if let Some((name, body)) = input.split_once('(') {
            // The field accepts color values, not nested relative-color/calc
            // expressions. Upstream calculations can overflow before its HSL
            // channel clamping, hiding a non-finite intermediate result.
            if body.contains('(')
                || body
                    .split_ascii_whitespace()
                    .next()
                    .is_some_and(|token| token.eq_ignore_ascii_case("from"))
            {
                return Err(ColorParseError::UnsupportedExpression);
            }
            // Check literals before the upstream parser can clamp them. Its own
            // number parser rejects NaN/Inf, but a huge finite angle can overflow.
            // A bare hex color such as 1e9999 must not be treated as a number.
            check_finite_literals(body)?;
            if name.trim_end().eq_ignore_ascii_case("cmyk")
                || name.trim_end().eq_ignore_ascii_case("device-cmyk")
            {
                parse_cmyk(body.strip_suffix(')').ok_or(ColorParseError::InvalidCmyk)?)?
            } else {
                csscolorparser::parse(input).map_err(ColorParseError::Css)?
            }
        } else {
            csscolorparser::parse(input).map_err(ColorParseError::Css)?
        };
        if ![color.r, color.g, color.b, color.a]
            .iter()
            .all(|component| component.is_finite())
        {
            return Err(ColorParseError::NonFinite);
        }

        // csscolorparser::Color::to_rgba8 rounds, rather than truncates, each
        // normalized component. This is the only precision boundary for text input.
        Ok(Self(color.to_rgba8()))
    }

    pub(crate) const fn from_rgba8(rgba: [u8; 4]) -> Self {
        Self(rgba)
    }

    pub(crate) const fn to_rgba8(self) -> [u8; 4] {
        self.0
    }

    /// Config storage is always #RRGGBB, or #RRGGBBAA when alpha is not opaque.
    pub(crate) fn canonical_hex(self) -> String {
        let [r, g, b, a] = self.0;
        if a == 255 {
            format!("#{r:02X}{g:02X}{b:02X}")
        } else {
            format!("#{r:02X}{g:02X}{b:02X}{a:02X}")
        }
    }

    pub(crate) fn format(self, format: ColorFormat) -> String {
        let [r, g, b, a] = self.0;
        let color = Color::from_rgba8(r, g, b, a);
        match format {
            ColorFormat::Hex => self.canonical_hex(),
            ColorFormat::Rgb => self.format_function("rgb", format!("{r}, {g}, {b}")),
            ColorFormat::Hsl | ColorFormat::Hsv => {
                let (name, [h, s, value, _]) = if format == ColorFormat::Hsl {
                    ("hsl", color.to_hsla())
                } else {
                    ("hsv", color.to_hsva())
                };
                self.format_function(
                    name,
                    format!(
                        "{}, {}%, {}%",
                        decimal(f64::from(h)),
                        decimal(f64::from(s) * 100.0),
                        decimal(f64::from(value) * 100.0),
                    ),
                )
            }
            ColorFormat::Cmyk => {
                // The W3C naive inverse is intentionally not print/ICC accurate:
                // https://www.w3.org/TR/css-color-5/#cmyk-rgb
                let [r, g, b] = [r, g, b].map(|value| f64::from(value) / 255.0);
                let max = r.max(g).max(b);
                let [c, m, y] = if max == 0.0 {
                    [0.0; 3]
                } else {
                    [r, g, b].map(|value| (max - value) / max)
                };
                let alpha = if a == 255 {
                    String::new()
                } else {
                    format!(" / {}", decimal(f64::from(a) / 255.0))
                };
                format!(
                    "cmyk({}% {}% {}% {}%{alpha})",
                    decimal(c * 100.0),
                    decimal(m * 100.0),
                    decimal(y * 100.0),
                    decimal((1.0 - max) * 100.0),
                )
            }
        }
    }

    fn format_function(self, name: &str, components: String) -> String {
        let alpha = self.0[3];
        if alpha == 255 {
            format!("{name}({components})")
        } else {
            format!(
                "{name}a({components}, {})",
                decimal(f64::from(alpha) / 255.0),
            )
        }
    }
}

fn decimal(value: f64) -> String {
    // Upstream CSS formatters round alpha to whole percentages, losing byte
    // precision (254/255 can even become opaque). Six decimals round-trip our
    // canonical bytes, including near-black, near-white and translucent colors.
    let formatted = format!("{value:.6}");
    let trimmed = formatted.trim_end_matches('0').trim_end_matches('.');
    if trimmed == "-0" {
        "0".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn check_finite_literals(input: &str) -> Result<(), ColorParseError> {
    for token in input.split(|ch: char| ch.is_ascii_whitespace() || "(),/%".contains(ch)) {
        let lower = token.to_ascii_lowercase();
        let (number, multiplier) = if let Some(number) = lower.strip_suffix("turn") {
            (number, 360.0)
        } else if let Some(number) = lower.strip_suffix("grad") {
            // Match the upstream operation order, including its overflow boundary.
            (number, 360.0)
        } else if let Some(number) = lower.strip_suffix("rad") {
            (number, 180.0 / std::f32::consts::PI)
        } else {
            (lower.strip_suffix("deg").unwrap_or(&lower), 1.0)
        };
        if let Ok(number) = number.parse::<f32>() {
            if !number.is_finite() || !(number * multiplier).is_finite() {
                return Err(ColorParseError::NonFinite);
            }
        }
    }
    Ok(())
}

fn parse_cmyk(body: &str) -> Result<Color, ColorParseError> {
    let (components, alpha) = if let Some((components, alpha)) = body.split_once('/') {
        (components, parse_unit_interval(alpha.trim(), false)?)
    } else {
        (body, 1.0)
    };
    // Permit either comma or whitespace separators, but reject missing components
    // and mixed delimiters rather than silently repairing malformed input.
    let values: Vec<_> = if components.contains(',') {
        components.split(',').map(str::trim).collect()
    } else {
        components.split_ascii_whitespace().collect()
    };
    let [c, m, y, k] = values.as_slice() else {
        return Err(ColorParseError::InvalidCmyk);
    };
    let [c, m, y, k] = [
        parse_unit_interval(c, true)?,
        parse_unit_interval(m, true)?,
        parse_unit_interval(y, true)?,
        parse_unit_interval(k, true)?,
    ];

    // W3C's generic, uncalibrated CMYK-to-sRGB approximation, with unchanged
    // alpha: https://www.w3.org/TR/css-color-5/#cmyk-rgb . This is not proofing.
    Ok(Color::new(
        ((1.0 - c) * (1.0 - k)) as f32,
        ((1.0 - m) * (1.0 - k)) as f32,
        ((1.0 - y) * (1.0 - k)) as f32,
        alpha as f32,
    ))
}

fn parse_unit_interval(input: &str, require_percent: bool) -> Result<f64, ColorParseError> {
    let (number, scale) = if let Some(number) = input.strip_suffix('%') {
        (number, 100.0)
    } else if require_percent {
        return Err(ColorParseError::InvalidCmyk);
    } else {
        (input, 1.0)
    };
    let value = number
        .parse::<f64>()
        .map_err(|_| ColorParseError::InvalidCmyk)?;
    if !value.is_finite() {
        return Err(ColorParseError::NonFinite);
    }
    if !(0.0..=scale).contains(&value) {
        return Err(ColorParseError::CmykOutOfRange);
    }
    Ok(value / scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_formats_resolve_to_canonical_bytes() {
        for (input, expected) in [
            (" a1B2c3 ", [161, 178, 195, 255]),
            ("1e9999", [30, 153, 153, 255]),
            ("#abc", [170, 187, 204, 255]),
            ("#abcd", [170, 187, 204, 221]),
            ("11223344", [17, 34, 51, 68]),
            ("rgb(255, 0, 128)", [255, 0, 128, 255]),
            ("rgb(100% 0% 50% / 50%)", [255, 0, 128, 128]),
            ("rgba(12, 34, 56, .25)", [12, 34, 56, 64]),
            ("hsl(120 100% 50%)", [0, 255, 0, 255]),
            ("hsla(240, 100%, 50%, .5)", [0, 0, 255, 128]),
            ("hsv(60, 100%, 100%)", [255, 255, 0, 255]),
            ("hsva(180 100% 100% / .5)", [0, 255, 255, 128]),
            ("cmyk(0% 100% 100% 0%)", [255, 0, 0, 255]),
            ("CMYK(100%, 0%, 0%, 0% / 25%)", [0, 255, 255, 64]),
            ("device-cmyk(0% 0% 0% 100% / .5)", [0, 0, 0, 128]),
            ("cmyk(0% 0% 0% 0%)", [255, 255, 255, 255]),
            ("cmyk(20% 40% 60% 50%)", [102, 77, 51, 255]),
            ("transparent", [0, 0, 0, 0]),
        ] {
            assert_eq!(
                ColorValue::parse(input).unwrap().to_rgba8(),
                expected,
                "{input}"
            );
        }
    }

    #[test]
    fn library_clamps_finite_channels_and_wraps_hue() {
        for (input, expected) in [
            ("rgba(300, -20, 128, 2)", [255, 0, 128, 255]),
            ("rgba(0, 0, 0, -1)", [0, 0, 0, 0]),
            ("hsl(480deg 120% 50%)", [0, 255, 0, 255]),
            ("hsl(-120deg 100% 50%)", [0, 0, 255, 255]),
            ("hsl(.5turn 100% 50%)", [0, 255, 255, 255]),
            ("hsv(720 100% 150%)", [255, 0, 0, 255]),
            ("hsv(0 -20% 50%)", [128, 128, 128, 255]),
        ] {
            assert_eq!(
                ColorValue::parse(input).unwrap().to_rgba8(),
                expected,
                "{input}"
            );
        }
    }

    #[test]
    fn malformed_nonfinite_and_ambiguous_inputs_are_errors() {
        for input in [
            "",
            "   ",
            "#12",
            "#12345",
            "#gggggg",
            "#123456789",
            "rgb(1,2)",
            "rgba(1,2,3,4,5)",
            "rgb(1,2,3) trailing",
            "hsl(0,50%)",
            "hsv(0,1,2",
            "rgb(NaN 0 0)",
            "rgba(0 0 0 / inf)",
            "hsl(inf 50% 50%)",
            "hsv(0 NaN 50%)",
            "rgb(1e999 0 0)",
            "hsl(1e38turn 50% 50%)",
            "hsl(1e38grad 50% 50%)",
            "hsl(from red calc(1e38 * 1e38) s l)",
            "rgb(from red r g b)",
            "hsl(1e38rad 50% 50%)",
            "cmyk(0 0 0 0)",
            "cmyk(0% 0% 0%)",
            "cmyk(0% 0% 0% 0% 0%)",
            "cmyk(0%,,0%,0%)",
            "cmyk(0%, 0% 0%, 0%)",
            "cmyk(0% 0% 0% 0% /)",
            "cmyk(0% 0% 0% 0% / .5 / .5)",
            "cmyk(NaN% 0% 0% 0%)",
            "cmyk(0% 0% 0% inf%)",
            "cmyk(-1% 0% 0% 0%)",
            "cmyk(0% 0% 0% 101%)",
            "cmyk(0% 0% 0% 0% / 1.01)",
            "cmyk(0% 0% 0% 0% / -1%)",
            "device-cmyk(0% 0% 0% 0%, red)",
        ] {
            assert!(
                ColorValue::parse(input).is_err(),
                "unexpectedly accepted {input:?}"
            );
        }
        assert_eq!(
            ColorValue::parse(&"a".repeat(MAX_COLOR_INPUT_BYTES + 1)),
            Err(ColorParseError::TooLong),
        );
        let at_limit = format!("{}#123456", " ".repeat(MAX_COLOR_INPUT_BYTES - 7));
        assert_eq!(
            ColorValue::parse(&at_limit).unwrap().to_rgba8(),
            [18, 52, 86, 255]
        );
        assert_eq!(
            ColorValue::parse(&(at_limit + " ")),
            Err(ColorParseError::TooLong)
        );
    }

    #[test]
    fn storage_uses_uppercase_and_keeps_transparent_rgb() {
        for (rgba, expected) in [
            ([1, 171, 255, 255], "#01ABFF"),
            ([1, 171, 255, 254], "#01ABFFFE"),
            ([1, 171, 255, 0], "#01ABFF00"),
        ] {
            let value = ColorValue::from_rgba8(rgba);
            assert_eq!(value.canonical_hex(), expected);
            assert_eq!(ColorValue::parse(expected).unwrap(), value);
        }
    }

    #[test]
    fn all_formats_preserve_every_alpha_byte() {
        for alpha in 0..=255 {
            let color = ColorValue::from_rgba8([17, 129, 253, alpha]);
            for format in ColorFormat::ALL {
                let text = color.format(format);
                assert_eq!(
                    ColorValue::parse(&text).unwrap(),
                    color,
                    "{format:?}: {text}"
                );
            }
        }
    }

    #[test]
    fn format_cycles_do_not_drift_at_channel_boundaries() {
        // Cover achromatic values, channel extrema, tiny chroma, and near-white
        // saturation where decimal rounding or a zero CMYK denominator can bite.
        let channels = [0, 1, 2, 63, 127, 128, 129, 253, 254, 255];
        for r in channels {
            for g in channels {
                for b in channels {
                    let original = ColorValue::from_rgba8([r, g, b, 254]);
                    let mut current = original;
                    for _ in 0..3 {
                        for format in ColorFormat::ALL {
                            let text = current.format(format);
                            current = ColorValue::parse(&text).unwrap();
                            assert_eq!(current, original, "{format:?}: {text}");
                        }
                    }
                }
            }
        }
    }
}
