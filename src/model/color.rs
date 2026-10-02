//! CSS color parsing and formatting.

/// A straight-alpha RGBA color with 8-bit channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Color {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
    /// Alpha channel; 255 is opaque.
    pub a: u8,
}

impl Color {
    /// Fully transparent black.
    pub const TRANSPARENT: Self = Self::rgba(0, 0, 0, 0);
    /// Opaque white.
    pub const WHITE: Self = Self::rgb(255, 255, 255);
    /// Opaque black.
    pub const BLACK: Self = Self::rgb(0, 0, 0);

    /// Opaque color from channels.
    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Color from channels and alpha.
    #[must_use]
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Pack as `0xRRGGBBAA`.
    #[must_use]
    pub const fn to_u32(self) -> u32 {
        ((self.r as u32) << 24) | ((self.g as u32) << 16) | ((self.b as u32) << 8) | self.a as u32
    }

    /// Alpha as a fraction.
    #[must_use]
    pub fn alpha(self) -> f32 {
        f32::from(self.a) / 255.0
    }

    /// Same color with a new alpha fraction.
    #[must_use]
    pub fn with_alpha(self, alpha: f32) -> Self {
        Self {
            a: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
            ..self
        }
    }

    /// Canonical CSS text: `#rrggbb`, `#rrggbbaa`, or `transparent`.
    #[must_use]
    pub fn to_css(self) -> String {
        if self.a == 0 && self.r == 0 && self.g == 0 && self.b == 0 {
            "transparent".to_owned()
        } else if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }

    /// Six-digit uppercase hex without alpha, as shown in a design panel.
    #[must_use]
    pub fn to_hex6(self) -> String {
        format!("{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// Parse any supported CSS color value.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim().to_ascii_lowercase();
        if let Some(hex) = value.strip_prefix('#') {
            return parse_hex(hex);
        }
        if let Some(args) = function_args(&value, &["rgb", "rgba"]) {
            return parse_rgb_args(&args);
        }
        if let Some(args) = function_args(&value, &["hsl", "hsla"]) {
            return parse_hsl_args(&args);
        }
        named(&value)
    }

    /// Parse a hex string with or without `#`, as typed into a panel field.
    #[must_use]
    pub fn parse_loose(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        Self::parse(trimmed).or_else(|| parse_hex(trimmed.trim_start_matches('#')))
    }
}

fn function_args(value: &str, names: &[&str]) -> Option<Vec<String>> {
    let open = value.find('(')?;
    let name = value[..open].trim();
    if !names.contains(&name) || !value.ends_with(')') {
        return None;
    }
    let inner = &value[open + 1..value.len() - 1];
    Some(
        inner
            .split([',', ' ', '/'])
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
    )
}

fn parse_channel(value: &str) -> Option<u8> {
    if let Some(percent) = value.strip_suffix('%') {
        let fraction: f32 = percent.parse().ok()?;
        return Some((fraction.clamp(0.0, 100.0) * 2.55).round() as u8);
    }
    let number: f32 = value.parse().ok()?;
    Some(number.clamp(0.0, 255.0).round() as u8)
}

fn parse_alpha(value: Option<&String>) -> Option<u8> {
    let Some(value) = value else {
        return Some(255);
    };
    let fraction = if let Some(percent) = value.strip_suffix('%') {
        percent.parse::<f32>().ok()? / 100.0
    } else {
        value.parse::<f32>().ok()?
    };
    Some((fraction.clamp(0.0, 1.0) * 255.0).round() as u8)
}

fn parse_rgb_args(args: &[String]) -> Option<Color> {
    if args.len() < 3 {
        return None;
    }
    Some(Color::rgba(
        parse_channel(&args[0])?,
        parse_channel(&args[1])?,
        parse_channel(&args[2])?,
        parse_alpha(args.get(3))?,
    ))
}

fn parse_hsl_args(args: &[String]) -> Option<Color> {
    if args.len() < 3 {
        return None;
    }
    let hue: f32 = args[0].trim_end_matches("deg").parse().ok()?;
    let saturation: f32 = args[1].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
    let lightness: f32 = args[2].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
    let (r, g, b) = hsl_to_rgb(hue, saturation, lightness);
    Some(Color::rgba(r, g, b, parse_alpha(args.get(3))?))
}

fn hsl_to_rgb(hue: f32, saturation: f32, lightness: f32) -> (u8, u8, u8) {
    let hue = hue.rem_euclid(360.0) / 360.0;
    let saturation = saturation.clamp(0.0, 1.0);
    let lightness = lightness.clamp(0.0, 1.0);
    if saturation == 0.0 {
        let value = (lightness * 255.0).round() as u8;
        return (value, value, value);
    }
    let q = if lightness < 0.5 {
        lightness * (1.0 + saturation)
    } else {
        lightness + saturation - lightness * saturation
    };
    let p = 2.0 * lightness - q;
    let channel = |t: f32| {
        let t = t.rem_euclid(1.0);
        let value = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (value * 255.0).round() as u8
    };
    (
        channel(hue + 1.0 / 3.0),
        channel(hue),
        channel(hue - 1.0 / 3.0),
    )
}

fn parse_hex(hex: &str) -> Option<Color> {
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let nibble = |index: usize| u8::from_str_radix(&hex[index..=index], 16).ok();
    let byte = |index: usize| u8::from_str_radix(&hex[index..index + 2], 16).ok();
    match hex.len() {
        3 => Some(Color::rgb(
            nibble(0)? * 17,
            nibble(1)? * 17,
            nibble(2)? * 17,
        )),
        4 => Some(Color::rgba(
            nibble(0)? * 17,
            nibble(1)? * 17,
            nibble(2)? * 17,
            nibble(3)? * 17,
        )),
        6 => Some(Color::rgb(byte(0)?, byte(2)?, byte(4)?)),
        8 => Some(Color::rgba(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
        _ => None,
    }
}

fn named(value: &str) -> Option<Color> {
    let rgb = match value {
        "transparent" => return Some(Color::TRANSPARENT),
        "black" => 0x000000,
        "white" => 0xffffff,
        "red" => 0xff0000,
        "green" => 0x008000,
        "lime" => 0x00ff00,
        "blue" => 0x0000ff,
        "yellow" => 0xffff00,
        "orange" => 0xffa500,
        "purple" => 0x800080,
        "pink" => 0xffc0cb,
        "gray" | "grey" => 0x808080,
        "silver" => 0xc0c0c0,
        "maroon" => 0x800000,
        "navy" => 0x000080,
        "teal" => 0x008080,
        "aqua" | "cyan" => 0x00ffff,
        "fuchsia" | "magenta" => 0xff00ff,
        "olive" => 0x808000,
        "indigo" => 0x4b0082,
        "violet" => 0xee82ee,
        "gold" => 0xffd700,
        "coral" => 0xff7f50,
        "salmon" => 0xfa8072,
        "tomato" => 0xff6347,
        "crimson" => 0xdc143c,
        "brown" => 0xa52a2a,
        "beige" => 0xf5f5dc,
        "ivory" => 0xfffff0,
        "khaki" => 0xf0e68c,
        "lavender" => 0xe6e6fa,
        "turquoise" => 0x40e0d0,
        "tan" => 0xd2b48c,
        "chocolate" => 0xd2691e,
        "plum" => 0xdda0dd,
        "orchid" => 0xda70d6,
        "skyblue" => 0x87ceeb,
        "steelblue" => 0x4682b4,
        "slategray" | "slategrey" => 0x708090,
        "darkgray" | "darkgrey" => 0xa9a9a9,
        "lightgray" | "lightgrey" => 0xd3d3d3,
        "gainsboro" => 0xdcdcdc,
        "whitesmoke" => 0xf5f5f5,
        "dimgray" | "dimgrey" => 0x696969,
        "royalblue" => 0x4169e1,
        "dodgerblue" => 0x1e90ff,
        "deepskyblue" => 0x00bfff,
        "seagreen" => 0x2e8b57,
        "forestgreen" => 0x228b22,
        "limegreen" => 0x32cd32,
        "darkgreen" => 0x006400,
        "darkblue" => 0x00008b,
        "darkred" => 0x8b0000,
        "hotpink" => 0xff69b4,
        "rebeccapurple" => 0x663399,
        _ => return None,
    };
    Some(Color::rgb(
        ((rgb >> 16) & 0xff) as u8,
        ((rgb >> 8) & 0xff) as u8,
        (rgb & 0xff) as u8,
    ))
}

#[cfg(test)]
mod tests {
    use super::Color;

    #[test]
    fn parses_common_forms() {
        assert_eq!(Color::parse("#fff"), Some(Color::WHITE));
        assert_eq!(
            Color::parse("#11223380"),
            Some(Color::rgba(0x11, 0x22, 0x33, 0x80))
        );
        assert_eq!(Color::parse("rgb(1, 2, 3)"), Some(Color::rgb(1, 2, 3)));
        assert_eq!(
            Color::parse("rgba(1 2 3 / 50%)"),
            Some(Color::rgba(1, 2, 3, 128))
        );
        assert_eq!(
            Color::parse("hsl(0, 100%, 50%)"),
            Some(Color::rgb(255, 0, 0))
        );
        assert_eq!(Color::parse("Tomato"), Some(Color::rgb(255, 99, 71)));
        assert_eq!(Color::parse("nope"), None);
        assert_eq!(
            Color::parse_loose("6e7bff"),
            Some(Color::rgb(0x6e, 0x7b, 0xff))
        );
    }

    #[test]
    fn formats_canonically() {
        assert_eq!(Color::rgb(0x6e, 0x7b, 0xff).to_css(), "#6e7bff");
        assert_eq!(Color::rgba(0, 0, 0, 0x80).to_css(), "#00000080");
        assert_eq!(Color::TRANSPARENT.to_css(), "transparent");
    }
}
