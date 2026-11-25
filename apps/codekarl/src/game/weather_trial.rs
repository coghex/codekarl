//! Headless report and pictures for manually placed weather systems.
use std::{collections::BTreeSet, fmt::Write, path::Path};

use ck_sim::{Date, GameTime};
use serde::Deserialize;

use super::{
    lows::{EndKind, Lows},
    wind::{self, WindField},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Trial {
    start: [u16; 4],
    hours: u64,
    every: u64,
    #[serde(default)]
    low: Vec<LowInput>,
    #[serde(default)]
    point: Vec<Point>,
    picture: Option<Picture>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LowInput {
    cell: [u32; 2],
    radius_m: i64,
    depth_pa: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Point {
    name: String,
    cell: [u32; 2],
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Picture {
    west: u32,
    north: u32,
    east: u32,
    south: u32,
    cells_per_pixel: u32,
}

fn date(start: [u16; 4]) -> Result<GameTime, String> {
    let byte = |value| u8::try_from(value).map_err(|_| "start: invalid date".to_owned());
    GameTime::from_date(Date {
        year: start[0],
        month: byte(start[1])?,
        day: byte(start[2])?,
        hour: byte(start[3])?,
    })
    .map_err(|e| format!("start: {e}"))
}
fn gregorian(date: Date) -> (i64, usize, i64) {
    let offset = ck_sim::weather::julian_day(
        i64::from(date.year),
        i64::from(date.month),
        i64::from(date.day),
    ) - ck_sim::weather::julian_day(1, 1, 1);
    let (y, m, d) = ck_sim::weather::gregorian(offset).expect("valid date");
    (y, usize::try_from(m).expect("valid month"), d)
}
fn knots(speed: i64) -> i64 {
    i64::try_from((i128::from(speed) * 9 + 231) / 463).expect("knots fit")
}

#[expect(
    clippy::too_many_lines,
    reason = "the report follows the reviewed trial protocol in order"
)]
pub(crate) fn command(arguments: impl IntoIterator<Item = String>) -> Result<String, String> {
    let args: Vec<String> = arguments.into_iter().collect();
    if args.is_empty() || args.len() > 2 {
        return Err("expected trial file and optional picture directory".into());
    }
    let text = std::fs::read_to_string(&args[0]).map_err(|e| e.to_string())?;
    let trial: Trial = toml::from_str(&text).map_err(|e| e.to_string())?;
    if trial.hours > 100_000 {
        return Err("hours: outside 0 to 100000".into());
    }
    if !(1..=100_000).contains(&trial.every) {
        return Err("every: outside 1 to 100000".into());
    }
    if trial.low.len() > 1000 {
        return Err("low: too many".into());
    }
    if trial.point.len() > 1000 {
        return Err("point: too many".into());
    }
    let start = date(trial.start)?;
    let last = start
        .hours()
        .checked_add(trial.hours)
        .ok_or("hours: overflow")?;
    GameTime::from_hours(last)
        .date()
        .map_err(|_| "hours: beyond last calendar hour")?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let backdrop = ck_map::load_backdrop(&root.join("map/backdrop")).map_err(|e| e.to_string())?;
    let rules = ck_sim::weather::WeatherRules::parse(
        &std::fs::read_to_string(root.join("game/weather.toml")).map_err(|e| e.to_string())?,
    )?;
    let field = WindField::new(&backdrop, &rules)?;
    let mut lows = Lows::default();
    for low in &trial.low {
        field
            .place(
                &mut lows,
                low.cell[0],
                low.cell[1],
                low.radius_m,
                low.depth_pa,
            )
            .map_err(|e| format!("low: {e}"))?;
    }
    let mut names = BTreeSet::new();
    for point in &trial.point {
        if point.name.is_empty()
            || point.name.len() > 32
            || !point
                .name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_')
            || !names.insert(&point.name)
        {
            return Err("point name: invalid or duplicate".into());
        }
        if field.monthly(point.cell[0], point.cell[1], 0).is_none() {
            return Err("point cell: outside backdrop".into());
        }
    }
    if let Some(pic) = &trial.picture {
        if args.len() != 2 {
            return Err("picture: directory required".into());
        }
        pic.validate(&backdrop)?;
        std::fs::create_dir_all(&args[1]).map_err(|e| e.to_string())?;
    }
    let mut output = String::new();
    writeln!(
        output,
        "trial start {:04}-{:02}-{:02} {:02} hours {} every {}",
        trial.start[0], trial.start[1], trial.start[2], trial.start[3], trial.hours, trial.every
    )
    .expect("validated trial or writing to a string");
    for hour in 0..=trial.hours {
        let today = GameTime::from_hours(start.hours() + hour)
            .date()
            .map_err(|e| e.to_string())?;
        let (year, month, day) = gregorian(today);
        if hour % trial.every == 0 || hour == trial.hours {
            writeln!(
                output,
                "hour {hour} {:04}-{:02}-{:02} {:02}",
                today.year, today.month, today.day, today.hour
            )
            .expect("validated trial or writing to a string");
            for low in &lows.entries {
                let i =
                    u32::try_from(low.x / 1000).expect("validated trial or writing to a string");
                let j =
                    u32::try_from(low.y / 1000).expect("validated trial or writing to a string");
                let (_, _, p) = field
                    .at(&lows, i, j, year, month, day)
                    .expect("validated trial or writing to a string");
                writeln!(
                    output,
                    "low {} centre {} {} radius {} depth {} pressure {} strength {} strongest {}",
                    low.number,
                    low.x,
                    low.y,
                    low.radius,
                    low.depth,
                    p,
                    knots(field.strength(low)),
                    knots(low.strongest)
                )
                .expect("validated trial or writing to a string");
            }
            for point in &trial.point {
                let (u, v, p) = field
                    .at(&lows, point.cell[0], point.cell[1], year, month, day)
                    .expect("validated trial or writing to a string");
                let (speed, direction) = wind::display(u, v);
                if speed == 0 {
                    writeln!(
                        output,
                        "point {} pressure {p} wind {u} {v} calm",
                        point.name
                    )
                    .expect("validated trial or writing to a string");
                } else {
                    writeln!(
                        output,
                        "point {} pressure {p} wind {u} {v} {speed} kn {direction}",
                        point.name
                    )
                    .expect("validated trial or writing to a string");
                }
            }
            if let Some(pic) = &trial.picture {
                pic.draw(
                    &field,
                    &backdrop,
                    &lows,
                    (year, month, day),
                    today,
                    &Path::new(&args[1]).join(format!("hour-{hour:06}.png")),
                )?;
            }
        }
        if hour == trial.hours {
            break;
        }
        for ending in field.step(&mut lows, year, month, day) {
            match ending.kind {
                EndKind::Merged(into) => writeln!(
                    output,
                    "end {} merged {into} cell {} {} strongest {} hour {hour}",
                    ending.number,
                    ending.cell.0,
                    ending.cell.1,
                    knots(ending.strongest)
                )
                .expect("validated trial or writing to a string"),
                EndKind::Left => writeln!(
                    output,
                    "end {} left cell {} {} strongest {} hour {hour}",
                    ending.number,
                    ending.cell.0,
                    ending.cell.1,
                    knots(ending.strongest)
                )
                .expect("validated trial or writing to a string"),
            }
        }
    }
    Ok(output)
}

impl Picture {
    fn validate(&self, backdrop: &ck_map::Backdrop) -> Result<(), String> {
        if self.west >= self.east || self.east > 47_766 {
            return Err("picture east: invalid bounds".into());
        }
        let first = backdrop.first_row() * backdrop.block_cells();
        let end = (backdrop.first_row() + backdrop.rows()) * backdrop.block_cells();
        if self.north < first || self.north >= self.south || self.south > end {
            return Err("picture north/south: invalid bounds".into());
        }
        let c = self.cells_per_pixel;
        if !(1..=1024).contains(&c) {
            return Err("picture cells_per_pixel: outside range".into());
        }
        for (name, length) in [
            ("east", self.east - self.west),
            ("south", self.south - self.north),
        ] {
            if length % c != 0 || length / c > 4096 {
                return Err(format!("picture {name}: size not divisible or too large"));
            }
        }
        Ok(())
    }

    #[expect(
        clippy::many_single_char_names,
        clippy::similar_names,
        clippy::too_many_lines,
        reason = "picture drawing follows the reviewed wind and annotation sequence"
    )]
    fn draw(
        &self,
        field: &WindField<'_>,
        backdrop: &ck_map::Backdrop,
        lows: &Lows,
        date: (i64, usize, i64),
        today: Date,
        path: &Path,
    ) -> Result<(), String> {
        let c = self.cells_per_pixel;
        let width = (self.east - self.west) / c;
        let height = (self.south - self.north) / c;
        let mut pixels = vec![0u8; (width * height * 3) as usize];
        let mut pressures = vec![0i64; (width * height) as usize];
        let mut lands = vec![false; (width * height) as usize];
        let palette = [
            [244, 248, 249],
            [201, 229, 250],
            [130, 192, 238],
            [86, 161, 220],
            [96, 190, 169],
            [129, 203, 110],
            [208, 221, 100],
            [245, 217, 83],
            [246, 169, 65],
            [230, 112, 60],
            [210, 68, 65],
            [165, 60, 119],
            [79, 40, 100],
        ];
        let bands = [1, 4, 7, 11, 17, 22, 28, 34, 41, 48, 56, 64];
        let cached = wind::PictureField::new(field, lows);
        for y in 0..height {
            for x in 0..width {
                let i = self.west + x * c + c / 2;
                let j = self.north + y * c + c / 2;
                let (u, v, p) = cached
                    .at(i, j, date.0, date.1, date.2)
                    .expect("validated picture");
                let speed = wind::display(u, v).0;
                let band = bands.iter().position(|limit| speed <= *limit).unwrap_or(12);
                let land = backdrop
                    .sample(i / backdrop.block_cells(), j / backdrop.block_cells(), 0)
                    .expect("valid block")
                    .land;
                let index = (y * width + x) as usize;
                pressures[index] = p;
                lands[index] = land;
                for channel in 0..3 {
                    pixels[index * 3 + channel] = if land {
                        u8::try_from((u16::from(palette[band][channel]) * 4) / 5)
                            .expect("colour fits")
                    } else {
                        palette[band][channel]
                    };
                }
            }
        }
        for y in 0..height {
            for x in 0..width {
                let idx = (y * width + x) as usize;
                let boundary = (x > 0
                    && (pressures[idx].div_euclid(400) != pressures[idx - 1].div_euclid(400)
                        || lands[idx] != lands[idx - 1]))
                    || (y > 0
                        && (pressures[idx].div_euclid(400)
                            != pressures[idx - width as usize].div_euclid(400)
                            || lands[idx] != lands[idx - width as usize]));
                if boundary {
                    pixels[idx * 3..idx * 3 + 3].copy_from_slice(&[35, 35, 40]);
                }
            }
        }
        for low in &lows.entries {
            let (i, j) = (low.x / 1000, low.y / 1000);
            if i >= i64::from(self.west)
                && i < i64::from(self.east)
                && j >= i64::from(self.north)
                && j < i64::from(self.south)
            {
                let x = (i - i64::from(self.west)) / i64::from(c);
                let y = (j - i64::from(self.north)) / i64::from(c);
                for delta in -3..=3 {
                    for (xx, yy) in [(x + delta, y), (x, y + delta)] {
                        if xx >= 0 && yy >= 0 && xx < i64::from(width) && yy < i64::from(height) {
                            let n = (usize::try_from(yy).expect("inside picture") * width as usize
                                + usize::try_from(xx).expect("inside picture"))
                                * 3;
                            pixels[n..n + 3].copy_from_slice(&[0, 0, 0]);
                        }
                    }
                }
            }
        }
        // Arrows point towards the wind's destination, on a sparse regular grid.
        for y in (20..height).step_by(40) {
            for x in (20..width).step_by(40) {
                let i = self.west + x * c + c / 2;
                let j = self.north + y * c + c / 2;
                let (u, v, _) = cached
                    .at(i, j, date.0, date.1, date.2)
                    .expect("inside picture");
                let scale = u.abs().max(v.abs());
                if scale > 0 {
                    let dx = u * 7 / scale;
                    let dy = -v * 7 / scale;
                    for t in -4..=4 {
                        let px = i64::from(x) + dx * t / 4;
                        let py = i64::from(y) + dy * t / 4;
                        set_pixel(&mut pixels, width, height, px, py, [0, 0, 0]);
                    }
                    let tipx = i64::from(x) + dx;
                    let tipy = i64::from(y) + dy;
                    set_pixel(
                        &mut pixels,
                        width,
                        height,
                        tipx - dx.signum() + dy.signum(),
                        tipy - dy.signum() - dx.signum(),
                        [0, 0, 0],
                    );
                    set_pixel(
                        &mut pixels,
                        width,
                        height,
                        tipx - dx.signum() - dy.signum(),
                        tipy - dy.signum() + dx.signum(),
                        [0, 0, 0],
                    );
                }
            }
        }
        // Compact labelled colour key and a Julian date/time stamp.
        if width >= 104 && height >= 16 {
            let key_y = height - 9;
            let band_width = (width / 13).min(24);
            for (index, colour) in palette.iter().enumerate() {
                for y in key_y..key_y + 4 {
                    for x in (u32::try_from(index).expect("palette index fits") * band_width)
                        ..((u32::try_from(index).expect("palette index fits") + 1) * band_width)
                    {
                        set_pixel(
                            &mut pixels,
                            width,
                            height,
                            i64::from(x),
                            i64::from(y),
                            *colour,
                        );
                    }
                }
                if index < bands.len() {
                    draw_digits(
                        &mut pixels,
                        width,
                        height,
                        u32::try_from(index).expect("palette index fits") * band_width,
                        key_y + 4,
                        &bands[index].to_string(),
                    );
                }
            }
        }
        draw_digits(
            &mut pixels,
            width,
            height,
            2,
            2,
            &format!(
                "{:04}-{:02}-{:02} {:02}",
                today.year, today.month, today.day, today.hour
            ),
        );
        let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
        let mut encoder = png::Encoder::new(file, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&pixels).map_err(|e| e.to_string())
    }
}

fn set_pixel(pixels: &mut [u8], width: u32, height: u32, x: i64, y: i64, colour: [u8; 3]) {
    if x >= 0 && y >= 0 && x < i64::from(width) && y < i64::from(height) {
        let index = (usize::try_from(y).expect("valid y") * width as usize
            + usize::try_from(x).expect("valid x"))
            * 3;
        pixels[index..index + 3].copy_from_slice(&colour);
    }
}

fn draw_digits(pixels: &mut [u8], width: u32, height: u32, x: u32, y: u32, text: &str) {
    const DIGITS: [&str; 10] = [
        "111101101101111",
        "010110010010111",
        "111001111100111",
        "111001111001111",
        "101101111001001",
        "111100111001111",
        "111100111101111",
        "111001001001001",
        "111101111101111",
        "111101111001111",
    ];
    for (index, ch) in text.chars().enumerate() {
        let glyph = if let Some(digit) = ch.to_digit(10) {
            DIGITS[digit as usize]
        } else if ch == '-' {
            "000000111000000"
        } else {
            "000000000000000"
        };
        for (bit, value) in glyph.bytes().enumerate() {
            if value == b'1' {
                set_pixel(
                    pixels,
                    width,
                    height,
                    i64::from(x) + i64::try_from(index * 4 + bit % 3).expect("label fits"),
                    i64::from(y) + i64::try_from(bit / 3).expect("label fits"),
                    [0, 0, 0],
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run_trial(text: &str) -> Result<String, String> {
        let dir = crate::testing::Dir::new();
        let path = dir.0.join("trial.toml");
        std::fs::write(&path, text).unwrap();
        command([path.display().to_string()])
    }

    #[test]
    fn joined_low_can_leave_and_reports_joined_strength() {
        let output = run_trial(
            r"start = [769, 1, 3, 0]
hours = 1
every = 1
[[low]]
cell = [17120, 8480]
radius_m = 400000
depth_pa = 1000
[[low]]
cell = [17120, 8480]
radius_m = 400000
depth_pa = 1000
",
        )
        .unwrap();
        assert_eq!(
            output,
            "trial start 0769-01-03 00 hours 1 every 1\nhour 0 0769-01-03 00\nlow 1 centre 17120500 8480500 radius 400000 depth 1000 pressure 99416 strength 20 strongest 20\nlow 2 centre 17120500 8480500 radius 400000 depth 1000 pressure 99416 strength 20 strongest 20\nend 2 merged 1 cell 17120 8480 strongest 20 hour 0\nend 1 left cell 17058 8327 strongest 34 hour 0\nhour 1 0769-01-03 01\n"
        );
    }

    #[test]
    fn trial_validates_input_and_calendar_end() {
        let base = "start = [9999, 12, 31, 22]\nhours = 1\nevery = 1\n";
        assert!(run_trial(base).unwrap().contains("hour 1 9999-12-31 23"));
        for (input, key) in [
            (base.replace("22", "23"), "hours"),
            (format!("{base}bogus = 1\n"), "bogus"),
            (base.replace("every = 1", "every = 0"), "every"),
            (
                format!("{base}[[low]]\ncell = [1, 9000]\nradius_m = 49999\ndepth_pa = 1\n"),
                "radius",
            ),
            (
                format!("{base}[[point]]\nname = \"X\"\ncell = [1, 0]\n"),
                "point",
            ),
        ] {
            assert!(run_trial(&input).unwrap_err().contains(key), "{key}");
        }
    }

    #[test]
    fn picture_and_point_inputs_are_refused_with_key() {
        let dir = crate::testing::Dir::new();
        let path = dir.0.join("trial.toml");
        let base = "start = [769, 1, 3, 0]\nhours = 0\nevery = 1\n";
        let picture = "[picture]\nwest = 18600\nnorth = 12600\neast = 25800\nsouth = 17800\ncells_per_pixel = 8\n";
        for (input, key) in [
            (picture.replace("east = 25800", "east = 18600"), "east"),
            (picture.replace("east = 25800", "east = 47767"), "east"),
            (picture.replace("north = 12600", "north = 8447"), "north"),
            (picture.replace("east = 25800", "east = 25801"), "east"),
            (
                picture.replace("cells_per_pixel = 8", "cells_per_pixel = 0"),
                "cells_per_pixel",
            ),
        ] {
            std::fs::write(&path, format!("{base}{input}")).unwrap();
            let error = command([
                path.display().to_string(),
                dir.0.join("images").display().to_string(),
            ])
            .unwrap_err();
            assert!(error.contains(key), "{error}");
        }
        std::fs::write(&path, format!("{base}{picture}")).unwrap();
        assert!(
            command([path.display().to_string()])
                .unwrap_err()
                .contains("picture")
        );
        assert!(run_trial(&format!("{base}[[point]]\nname = \"Birr\"\ncell = [22833, 15538]\n[[point]]\nname = \"Birr\"\ncell = [22833, 15538]\n")).unwrap_err().contains("point"));
    }

    #[test]
    fn picture_is_deterministic_and_has_requested_dimensions() {
        let dir = crate::testing::Dir::new();
        let path = dir.0.join("trial.toml");
        std::fs::write(&path, "start = [769, 1, 3, 0]\nhours = 48\nevery = 6\n[[low]]\ncell = [21361, 15777]\nradius_m = 500000\ndepth_pa = 3000\n[picture]\nwest = 18600\nnorth = 12600\neast = 25800\nsouth = 17800\ncells_per_pixel = 8\n").unwrap();
        let first = dir.0.join("one");
        let second = dir.0.join("two");
        for out in [&first, &second] {
            command([path.display().to_string(), out.display().to_string()]).unwrap();
        }
        for hour in (0..=48).step_by(6) {
            let name = format!("hour-{hour:06}.png");
            let a = std::fs::read(first.join(&name)).unwrap();
            let b = std::fs::read(second.join(&name)).unwrap();
            assert_eq!(a, b);
            let reader = png::Decoder::new(std::io::Cursor::new(a))
                .read_info()
                .unwrap();
            assert_eq!((reader.info().width, reader.info().height), (900, 650));
        }
    }

    #[test]
    fn trial_a_full_report() {
        let input = "start = [769,1,3,0]\nhours = 48\nevery = 6\n[[low]]\ncell = [21361,15777]\nradius_m = 500000\ndepth_pa = 3000\n[[point]]\nname = \"Birr\"\ncell = [22833,15538]\n[[point]]\nname = \"Skellig\"\ncell = [22484,15826]\n";
        let expected = "trial start 0769-01-03 00 hours 48 every 6\nhour 0 0769-01-03 00\nlow 1 centre 21361500 15777500 radius 500000 depth 3000 pressure 97894 strength 45 strongest 45\npoint Birr pressure 101209 wind 91 190 4 kn SW\npoint Skellig pressure 101221 wind 410 379 11 kn SW\nhour 6 0769-01-03 06\nlow 1 centre 21676500 15633241 radius 500000 depth 3000 pressure 97892 strength 45 strongest 45\npoint Birr pressure 101209 wind 91 190 4 kn SW\npoint Skellig pressure 101001 wind 592 2120 43 kn S\nhour 12 0769-01-03 12\nlow 1 centre 21976840 15449379 radius 500000 depth 3000 pressure 97893 strength 44 strongest 45\npoint Birr pressure 101016 wind -200 813 16 kn S\npoint Skellig pressure 100193 wind 1529 2385 55 kn SW\nhour 18 0769-01-03 18\nlow 1 centre 22272508 15286864 radius 500000 depth 3000 pressure 97886 strength 44 strongest 45\npoint Birr pressure 100033 wind -20 1120 22 kn S\npoint Skellig pressure 99925 wind 2387 1471 55 kn SW\nhour 24 0769-01-04 00\nlow 1 centre 22571274 15078059 radius 500000 depth 3000 pressure 97882 strength 44 strongest 45\npoint Birr pressure 99612 wind 523 963 21 kn SW\npoint Skellig pressure 100650 wind 2592 415 51 kn W\nhour 30 0769-01-04 06\nlow 1 centre 22857146 14810837 radius 500000 depth 3000 pressure 97885 strength 44 strongest 45\npoint Birr pressure 100449 wind 902 643 22 kn SW\npoint Skellig pressure 101215 wind 414 383 11 kn SW\nhour 36 0769-01-04 12\nlow 1 centre 23155458 14526403 radius 500000 depth 3000 pressure 97880 strength 44 strongest 45\npoint Birr pressure 101204 wind 116 199 4 kn SW\npoint Skellig pressure 101215 wind 414 383 11 kn SW\nhour 42 0769-01-04 18\nlow 1 centre 23477794 14326871 radius 500000 depth 3000 pressure 97877 strength 44 strongest 45\npoint Birr pressure 101204 wind 90 192 4 kn SW\npoint Skellig pressure 101215 wind 414 383 11 kn SW\nhour 48 0769-01-05 00\nlow 1 centre 23726775 14120389 radius 500000 depth 3000 pressure 97879 strength 43 strongest 45\npoint Birr pressure 101200 wind 89 194 4 kn SW\npoint Skellig pressure 101210 wind 418 386 11 kn SW\n";
        assert_eq!(run_trial(input).unwrap(), expected);
    }

    #[test]
    fn trial_d_full_report() {
        let input = "start = [769,1,3,0]\nhours = 3\nevery = 1\n[[low]]\ncell = [17120,8608]\nradius_m = 400000\ndepth_pa = 1000\n";
        let expected = "trial start 0769-01-03 00 hours 3 every 1\nhour 0 0769-01-03 00\nlow 1 centre 17120500 8608500 radius 400000 depth 1000 pressure 100398 strength 21 strongest 21\nhour 1 0769-01-03 01\nlow 1 centre 17060239 8452961 radius 400000 depth 1000 pressure 100393 strength 20 strongest 21\nend 1 left cell 16995 8294 strongest 21 hour 1\nhour 2 0769-01-03 02\nhour 3 0769-01-03 03\n";
        assert_eq!(run_trial(input).unwrap(), expected);
    }

    #[test]
    fn empty_list_report() {
        let input = "start = [768,10,9,0]\nhours = 24\nevery = 12\n[[point]]\nname = \"Birr\"\ncell = [22833,15538]\n";
        let expected = "trial start 0768-10-09 00 hours 24 every 12\nhour 0 0768-10-09 00\npoint Birr pressure 101263 wind 36 74 2 kn SW\nhour 12 0768-10-09 12\npoint Birr pressure 101263 wind 36 74 2 kn SW\nhour 24 0768-10-10 00\npoint Birr pressure 101253 wind 36 73 2 kn SW\n";
        assert_eq!(run_trial(input).unwrap(), expected);
    }

    #[test]
    fn trial_b_report_matches_spec() {
        let dir = crate::testing::Dir::new();
        let input = dir.0.join("trial.toml");
        std::fs::write(
            &input,
            r#"start = [769, 1, 3, 22]
hours = 3
every = 2
[[low]]
cell = [21361, 15777]
radius_m = 500000
depth_pa = 3000
[[low]]
cell = [21892, 15336]
radius_m = 800000
depth_pa = 2000
[[point]]
name = "Birr"
cell = [22833, 15538]
"#,
        )
        .unwrap();
        let output = command([input.display().to_string()]).unwrap();
        assert_eq!(
            output,
            "trial start 0769-01-03 22 hours 3 every 2\nhour 0 0769-01-03 22\nlow 1 centre 21361500 15777500 radius 500000 depth 3000 pressure 96581 strength 45 strongest 45\nlow 2 centre 21892500 15336500 radius 800000 depth 2000 pressure 98017 strength 26 strongest 26\npoint Birr pressure 100389 wind -90 721 14 kn S\nend 2 merged 1 cell 21696 15499 strongest 26 hour 0\nhour 2 0769-01-04 00\nlow 1 centre 21795927 15436689 radius 800000 depth 3172 pressure 97652 strength 38 strongest 45\npoint Birr pressure 100161 wind -253 915 18 kn S\nhour 3 0769-01-04 01\nlow 1 centre 21844536 15404909 radius 800000 depth 3172 pressure 97650 strength 38 strongest 45\npoint Birr pressure 100022 wind -227 929 19 kn S\n"
        );
    }
}
