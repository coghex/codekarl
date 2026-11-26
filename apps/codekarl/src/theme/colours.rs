//! Colour derivation shared by managed UI kinds.
use std::collections::BTreeMap;

fn rgb(value: &str) -> [f64; 3] {
    let byte =
        |i| f64::from(u8::from_str_radix(&value[i..i + 2], 16).expect("validated colour")) / 255.0;
    [byte(1), byte(3), byte(5)]
}

fn hsl(c: [f64; 3]) -> (f64, f64, f64) {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let delta = max - min;
    let light = f64::midpoint(max, min);
    if delta == 0.0 {
        return (0.0, 0.0, light);
    }
    let sat = delta / (1.0 - (2.0 * light - 1.0).abs());
    let hue = if c[0] >= c[1] && c[0] >= c[2] {
        ((c[1] - c[2]) / delta).rem_euclid(6.0)
    } else if c[1] >= c[2] {
        (c[2] - c[0]) / delta + 2.0
    } else {
        (c[0] - c[1]) / delta + 4.0
    };
    (hue * 60.0, sat, light)
}

#[expect(
    clippy::many_single_char_names,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "HSL components and rounded channels are bounded to 0..255"
)]
fn hex(hue: f64, sat: f64, light: f64) -> String {
    let chroma = (1.0 - (2.0 * light - 1.0).abs()) * sat;
    let sector = hue.rem_euclid(360.0) / 60.0;
    let x = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = if sector < 1.0 {
        (chroma, x, 0.0)
    } else if sector < 2.0 {
        (x, chroma, 0.0)
    } else if sector < 3.0 {
        (0.0, chroma, x)
    } else if sector < 4.0 {
        (0.0, x, chroma)
    } else if sector < 5.0 {
        (x, 0.0, chroma)
    } else {
        (chroma, 0.0, x)
    };
    let m = light - chroma / 2.0;
    format!(
        "#{:02x}{:02x}{:02x}",
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8
    )
}

fn luminance(value: &str) -> f64 {
    let c = rgb(value).map(|v| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    });
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

pub(super) fn contrast(a: &str, b: &str) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

pub(super) fn derive(base: &str, palette: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let (h, s, l) = hsl(rgb(base));
    let hot = hex(h, s, (1.4 * l + 0.03).min(0.92));
    let choose = |a: &str, b: &str, fill: &str| {
        if contrast(a, fill) >= contrast(b, fill) {
            a.to_owned()
        } else {
            b.to_owned()
        }
    };
    let text = &palette["text"];
    let body = &palette["body"];
    let (nh, ns, _) = hsl(rgb(&palette["number"]));
    let light = hex(nh + 180.0, ns, 0.75);
    let dark = hex(nh + 180.0, ns, 0.30);
    let minimum = |ink: &str| contrast(ink, base).min(contrast(ink, &hot));
    let number = if minimum(&light) >= minimum(&dark) {
        light
    } else {
        dark
    };
    let over = &palette["over_limit"];
    let over_dark = &palette["over_limit_dark"];
    let limit = if minimum(over) >= minimum(over_dark) {
        over
    } else {
        over_dark
    };
    BTreeMap::from([
        ("outline".into(), base.to_owned()),
        ("hot".into(), hot.clone()),
        ("title_text".into(), choose(text, body, base)),
        ("pressed_text".into(), choose(text, body, &hot)),
        ("highlight_number".into(), number),
        ("highlight_over_limit".into(), limit.clone()),
    ])
}
