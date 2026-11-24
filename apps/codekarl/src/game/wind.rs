//! Integer steady-pressure wind derived from the validated backdrop.
use ck_map::Backdrop;
use ck_sim::weather::WeatherRules;

const WORLD: i64 = 47_766;
fn round(n: i128, d: i128) -> i64 {
    let q = (n.abs() + d / 2) / d;
    i64::try_from(if n < 0 { -q } else { q }).expect("validated wind fits i64")
}

#[expect(
    clippy::many_single_char_names,
    reason = "symbols match the reviewed equation"
)]
fn pressure_wind(
    dx: i64,
    dy: i64,
    sx: i64,
    sy: i64,
    c: i64,
    s: i64,
    pair: (i64, i64),
) -> (i64, i64) {
    let f = round(i128::from(145_842 * s), 1_000_000);
    let (e, f, dx, dy, sx, sy, c, factor) = (
        i128::from(pair.0),
        i128::from(f),
        i128::from(dx),
        i128::from(dy),
        i128::from(sx),
        i128::from(sy),
        i128::from(c),
        i128::from(pair.1),
    );
    let (u, v, span) = if sy == 0 {
        (-e * dx, f * dx, sx)
    } else {
        (
            -(e * dx * sy + f * dy * sx),
            f * dx * sy - e * dy * sx,
            sx * sy,
        )
    };
    let den = span * 839 * c * 1225 * (e * e + f * f);
    (
        round(u * factor * 100_000_000_000_000_000, den),
        round(v * factor * 100_000_000_000_000_000, den),
    )
}

pub(super) struct WindField<'a> {
    backdrop: &'a Backdrop,
    rules: &'a WeatherRules,
    count: i64,
}
impl<'a> WindField<'a> {
    pub(super) fn new(backdrop: &'a Backdrop, rules: &'a WeatherRules) -> Result<Self, String> {
        let b = i64::from(backdrop.block_cells());
        let count = (WORLD - 1 - b / 2) / b + 1;
        let field = Self {
            backdrop,
            rules,
            count,
        };
        if 2 * rules.wind_slope_blocks >= count {
            return Err("weather.toml: wind_slope_blocks too wide for backdrop".into());
        }
        for r in 0..i64::from(backdrop.rows()) {
            if field.trig(r).0 <= 0 {
                return Err(format!(
                    "weather.toml: cosine table gives row {} a nonpositive cosine",
                    r + i64::from(backdrop.first_row())
                ));
            }
        }
        Ok(field)
    }
    fn latitude(&self, r: i64) -> i64 {
        i64::from(
            self.backdrop
                .row_latitude_e4(
                    u32::try_from(r + i64::from(self.backdrop.first_row())).expect("validated row"),
                )
                .expect("validated row"),
        )
    }
    fn cos(&self, a: i64) -> i64 {
        let k = usize::try_from(a / 1000).expect("valid angle");
        let rem = a % 1000;
        if rem == 0 {
            return self.rules.cosine[k];
        }
        round(
            i128::from(self.rules.cosine[k] * (1000 - rem) + self.rules.cosine[k + 1] * rem),
            1000,
        )
    }
    fn trig(&self, r: i64) -> (i64, i64) {
        let lat = self.latitude(r);
        (
            self.cos(lat.abs()),
            self.cos(900_000 - lat.abs()) * lat.signum(),
        )
    }
    fn gap(&self, k: i64) -> i64 {
        let b = i64::from(self.backdrop.block_cells());
        if k + 1 == self.count {
            WORLD - (k * b + b / 2) + b / 2
        } else {
            b
        }
    }
    fn sample(&self, k: i64, r: i64, month: usize) -> ck_map::BackdropSample {
        self.backdrop
            .sample(
                u32::try_from(k).expect("valid column"),
                u32::try_from(r + i64::from(self.backdrop.first_row())).expect("validated row"),
                u32::try_from(month).expect("valid month"),
            )
            .expect("validated block")
    }
    #[expect(
        clippy::many_single_char_names,
        reason = "symbols match the reviewed equations"
    )]
    fn rule(&self, k: i64, r: i64, month: usize, pair: (i64, i64)) -> (i64, i64) {
        let n = self.rules.wind_slope_blocks;
        let east = (k + n).rem_euclid(self.count);
        let west = (k - n).rem_euclid(self.count);
        let sx: i64 = (0..n)
            .map(|s| self.gap((k + s) % self.count) + self.gap((k - 1 - s).rem_euclid(self.count)))
            .sum();
        let north = (r - n).max(0);
        let south = (r + n).min(i64::from(self.backdrop.rows()) - 1);
        let sy = (south - north) * i64::from(self.backdrop.block_cells());
        let dx = i64::from(self.sample(east, r, month).pressure_pa)
            - i64::from(self.sample(west, r, month).pressure_pa);
        let dy = i64::from(self.sample(k, north, month).pressure_pa)
            - i64::from(self.sample(k, south, month).pressure_pa);
        let (c, s) = self.trig(r);
        pressure_wind(dx, dy, sx, sy, c, s, pair)
    }

    fn block(&self, k: i64, r: i64, month: usize) -> (i64, i64) {
        let pair = if self.sample(k, r, month).land {
            (self.rules.wind_land_friction, self.rules.wind_land_factor)
        } else {
            (self.rules.wind_sea_friction, self.rules.wind_sea_factor)
        };
        let lat = self.latitude(r);
        let belt = self.rules.wind_belt_latitude;
        if lat.abs() < belt {
            let north = (0..i64::from(self.backdrop.rows()))
                .filter(|&q| self.latitude(q) >= belt)
                .next_back();
            let south = (0..i64::from(self.backdrop.rows())).find(|&q| self.latitude(q) <= -belt);
            if let (Some(n), Some(s)) = (north, south) {
                let a = self.rule(k, n, month, pair);
                let b = self.rule(k, s, month, pair);
                let whole = self.latitude(n) - self.latitude(s);
                let part = self.latitude(n) - lat;
                return (
                    round(
                        i128::from(a.0 * (whole - part) + b.0 * part),
                        i128::from(whole),
                    ),
                    round(
                        i128::from(a.1 * (whole - part) + b.1 * part),
                        i128::from(whole),
                    ),
                );
            }
        }
        self.rule(k, r, month, pair)
    }
    #[expect(
        clippy::many_single_char_names,
        reason = "symbols match the reviewed equations"
    )]
    pub(super) fn monthly(&self, i: u32, j: u32, month: usize) -> Option<(i64, i64, i64)> {
        let b = i64::from(self.backdrop.block_cells());
        let first = i64::from(self.backdrop.first_row());
        let rows = i64::from(self.backdrop.rows());
        let (i, j) = (i64::from(i), i64::from(j));
        if i >= WORLD || j < first * b || j >= (first + rows) * b || month >= 12 {
            return None;
        }
        let mut x = 2 * i + 1;
        let k = if x < b {
            x += 2 * WORLD;
            self.count - 1
        } else {
            ((x - b) / (2 * b)).min(self.count - 1)
        };
        let wide = 2 * self.gap(k);
        let ox = x - 2 * (k * b + b / 2);
        let y = 2 * j + 1 - 2 * first * b - b;
        let tall = 2 * b;
        let (r, oy) = if y < 0 {
            (0, 0)
        } else if y >= tall * (rows - 1) {
            (rows - 1, 0)
        } else {
            (y / tall, y % tall)
        };
        let k2 = (k + 1) % self.count;
        let r2 = (r + 1).min(rows - 1);
        let corners = [(k, r), (k2, r), (k, r2), (k2, r2)];
        let weights = [
            (wide - ox) * (tall - oy),
            ox * (tall - oy),
            (wide - ox) * oy,
            ox * oy,
        ];
        let mut sums = [0i128; 3];
        for ((col, row), w) in corners.into_iter().zip(weights) {
            let (u, v) = self.block(col, row, month);
            let p = i64::from(self.sample(col, row, month).pressure_pa);
            for (sum, value) in sums.iter_mut().zip([u, v, p]) {
                *sum += i128::from(w) * i128::from(value);
            }
        }
        let den = i128::from(wide * tall);
        Some((
            round(sums[0], den),
            round(sums[1], den),
            round(sums[2], den),
        ))
    }
}

/// Interpolate monthly values for a Gregorian day.
pub(super) fn day(
    months: &[(i64, i64, i64); 12],
    year: i64,
    month: usize,
    date: i64,
) -> (i64, i64, i64) {
    fn days(year: i64, month: usize) -> i64 {
        match month {
            2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        }
    }
    let here = days(year, month);
    let x = 2 * date - 1;
    let (other, other_year, part) = if x >= here {
        (month % 12 + 1, year + i64::from(month == 12), x - here)
    } else {
        (
            if month == 1 { 12 } else { month - 1 },
            year - i64::from(month == 1),
            here - x,
        )
    };
    let whole = here + days(other_year, other);
    let a = months[month - 1];
    let b = months[other - 1];
    let mix = |x: i64, y: i64| round(i128::from(x * (whole - part) + y * part), i128::from(whole));
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}
/// Whole knots and the compass point the wind comes from, or calm.
pub(super) fn display(u: i64, v: i64) -> (i64, &'static str) {
    let square = u128::try_from(i128::from(u) * i128::from(u) + i128::from(v) * i128::from(v))
        .expect("sum of squares");
    let q = square.isqrt();
    let speed = q + u128::from(square - q * q > q);
    let knots = round(i128::try_from(speed * 9).expect("speed fits"), 463);
    if knots == 0 {
        return (0, "");
    }
    let (au, av) = (i128::from(u).abs(), i128::from(v).abs());
    let point = if 1_000_000 * av <= 414_214 * au {
        if u > 0 { "W" } else { "E" }
    } else if 1_000_000 * au <= 414_214 * av {
        if v > 0 { "S" } else { "N" }
    } else {
        match (u > 0, v > 0) {
            (true, true) => "SW",
            (true, false) => "NW",
            (false, true) => "SE",
            (false, false) => "NE",
        }
    };
    (knots, point)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shipped_blocks_edges_wrap_and_dates() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let backdrop = ck_map::load_backdrop(&root.join("map/backdrop")).unwrap();
        let rules =
            WeatherRules::parse(&std::fs::read_to_string(root.join("game/weather.toml")).unwrap())
                .unwrap();
        let f = WindField::new(&backdrop, &rules).unwrap();
        assert_eq!((f.count, f.gap(745)), (746, 86));
        for (row, c, s, rate) in [
            (242, 599_313, 800_514, 116_749),
            (373, 999_996, -2814, -410),
            (529, 500_354, -865_820, -126_273),
        ] {
            let r = row - i64::from(backdrop.first_row());
            let trig = f.trig(r);
            assert_eq!(trig, (c, s));
            assert_eq!(round(i128::from(145_842 * trig.1), 1_000_000), rate);
        }
        for (col, row, jan, jul) in [
            (348, 246, (443, 325), (309, 14)),
            (351, 246, (425, 437), (291, 12)),
            (357, 234, (349, 440), (209, 22)),
            (290, 339, (-592, -193), (-525, -263)),
            (559, 493, (785, -61), (708, -58)),
            (311, 373, (-414, -96), (-297, 201)),
            (207, 373, (-116, -57), (-167, -55)),
        ] {
            let r = row - i64::from(backdrop.first_row());
            assert_eq!(f.block(col, r, 0), jan);
            assert_eq!(f.block(col, r, 6), jul);
        }
        for (i, expected) in [
            (47700, (-137, -330)),
            (47744, (-140, -331)),
            (47765, (-142, -330)),
            (0, (-142, -330)),
            (31, (-145, -330)),
            (64, (-149, -330)),
        ] {
            let (u, v, _) = f.monthly(i, 23904, 0).unwrap();
            assert_eq!((u, v), expected);
        }
        assert_eq!(f.block(356, 0, 0), (-270, -295));
        assert_eq!(f.block(356, 1, 0), (-262, -306));
        assert_eq!(f.block(356, i64::from(backdrop.rows()) - 1, 0), (187, 36));
        assert_eq!(f.monthly(22816, 8448, 0), f.monthly(22816, 8479, 0));
        assert_eq!(f.monthly(22816, 33916, 0), f.monthly(22816, 33919, 0));
        assert!(f.monthly(22816, 8447, 0).is_none());
        assert!(f.monthly(22816, 33920, 0).is_none());
        let months = std::array::from_fn(|m| f.monthly(22833, 15538, m).unwrap());
        for (year, month, date, expected) in [
            (768, 10, 12, (37, 74, 101_273)),
            (768, 10, 13, (36, 74, 101_263)),
            (769, 1, 13, (84, 202, 101_182)),
            (769, 1, 14, (83, 204, 101_178)),
            (772, 2, 29, (71, 70, 101_403)),
        ] {
            assert_eq!(day(&months, year, month, date), expected);
        }
    }

    #[test]
    fn direction_and_month_boundaries() {
        for (u, v, expected) in [
            (100, 0, "W"),
            (-100, 0, "E"),
            (0, 100, "S"),
            (0, -100, "N"),
            (100, 100, "SW"),
            (100, -100, "NW"),
            (-100, 100, "SE"),
            (-100, -100, "NE"),
            (1_000_000, 414_214, "W"),
            (1_000_000, 414_215, "SW"),
            (414_214, 1_000_000, "S"),
        ] {
            assert_eq!(display(u, v).1, expected);
        }
        assert_eq!(display(257, 0).0, 5);
        assert_eq!(display(0, -26).0, 1);
        assert_eq!(display(300, 400).0, 10);
        assert_eq!(display(25, 0).0, 0);
        let mut m = [(0, 0, 0); 12];
        m[0] = (100, 100, 100);
        m[1] = (200, 200, 200);
        assert_eq!(day(&m, 769, 1, 16).0, 100);
        assert_eq!(day(&m, 769, 1, 31).0, 151);
        assert_eq!(day(&m, 769, 1, 1).0, 52);
    }

    #[test]
    fn monthly_values_at_three_places() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let backdrop = ck_map::load_backdrop(&root.join("map/backdrop")).unwrap();
        let rules =
            WeatherRules::parse(&std::fs::read_to_string(root.join("game/weather.toml")).unwrap())
                .unwrap();
        let f = WindField::new(&backdrop, &rules).unwrap();
        for (i, j, expected) in [
            (
                22833,
                15538,
                [
                    (81, 208),
                    (82, 86),
                    (58, 52),
                    (-12, 35),
                    (35, -2),
                    (65, 28),
                    (92, 40),
                    (100, 44),
                    (50, 91),
                    (35, 72),
                    (84, 87),
                    (116, 146),
                ],
            ),
            (
                22999,
                15257,
                [
                    (91, 196),
                    (97, 138),
                    (70, 79),
                    (1, 49),
                    (16, 10),
                    (72, 25),
                    (94, 43),
                    (99, 61),
                    (97, 103),
                    (80, 82),
                    (60, 117),
                    (110, 150),
                ],
            ),
            (
                22484,
                15826,
                [
                    (447, 412),
                    (228, 226),
                    (156, 99),
                    (121, 78),
                    (87, 21),
                    (255, 18),
                    (292, 7),
                    (286, 17),
                    (207, 170),
                    (264, 102),
                    (272, 246),
                    (320, 298),
                ],
            ),
        ] {
            for (month, want) in expected.into_iter().enumerate() {
                let (u, v, _) = f.monthly(i, j, month).unwrap();
                assert_eq!((u, v), want, "{i},{j} month {month}");
            }
        }
        let pressures = [
            101_169, 101_422, 101_381, 101_464, 101_576, 101_593, 101_546, 101_513, 101_535,
            101_233, 101_251, 101_306,
        ];
        for (month, want) in pressures.into_iter().enumerate() {
            assert_eq!(f.monthly(22833, 15538, month).unwrap().2, want);
        }
    }

    #[test]
    fn start_rejects_zero_or_negative_cosines_and_wide_slope() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let backdrop = ck_map::load_backdrop(&root.join("map/backdrop")).unwrap();
        let text = std::fs::read_to_string(root.join("game/weather.toml")).unwrap();
        for value in [0, -1] {
            let changed = text.replacen("600420, 599024", &format!("{value}, {value}"), 1);
            assert_ne!(changed, text);
            let rules = WeatherRules::parse(&changed).unwrap();
            let error = WindField::new(&backdrop, &rules).err().unwrap();
            assert!(error.contains("cosine") && error.contains("242"), "{error}");
        }
        let fixture =
            ck_map::load_backdrop(&root.join("../crates/map/tests/fixtures/backdrop")).unwrap();
        let mut rules = WeatherRules::parse(&text).unwrap();
        rules.wind_slope_blocks = 5;
        assert!(WindField::new(&fixture, &rules).is_ok());
        rules.wind_slope_blocks = 6;
        assert!(
            WindField::new(&fixture, &rules)
                .err()
                .unwrap()
                .contains("wind_slope_blocks")
        );
    }

    #[test]
    fn pressure_slope_worked_examples() {
        for (dx, dy, sx, sy, cosine, sine, pair, want) in [
            (8, 0, 256, 256, 600_000, 800_000, (15_000, 690), (-4, 29)),
            (8, 0, 256, 256, 600_000, -800_000, (15_000, 690), (-4, -29)),
            (8, 0, 256, 256, 600_000, 800_000, (70_000, 330), (-6, 11)),
            (8, 0, 256, 256, 1_000_000, 0, (15_000, 690), (-140, 0)),
            (0, 10, 256, 256, 600_000, 800_000, (15_000, 690), (-37, -5)),
            (8, 10, 256, 256, 600_000, 800_000, (15_000, 690), (-41, 25)),
            (8, 10, 256, 128, 600_000, 800_000, (15_000, 690), (-77, 20)),
            (8, 0, 278, 0, 600_000, 800_000, (15_000, 690), (-3, 27)),
            (-1, 0, 256, 256, 600_000, 800_000, (15_000, 690), (0, -4)),
            (0, 0, 256, 256, 600_000, 800_000, (15_000, 690), (0, 0)),
        ] {
            assert_eq!(pressure_wind(dx, dy, sx, sy, cosine, sine, pair), want);
        }
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one fixture compares both belt pairs and a truncated backdrop"
    )]
    fn fixture_belt_uses_the_carried_blocks_pair_at_both_edges() {
        use std::{fs, io::BufWriter};
        fn write_image(
            path: &std::path::Path,
            width: u32,
            height: u32,
            depth: png::BitDepth,
            data: &[u8],
        ) {
            let file = BufWriter::new(fs::File::create(path).unwrap());
            let mut encoder = png::Encoder::new(file, width, height);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(depth);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(data)
                .unwrap();
        }
        let dir = crate::testing::Dir::new();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let fixture = root.join("../crates/map/tests/fixtures/backdrop");
        fs::copy(fixture.join("backdrop.toml"), dir.0.join("backdrop.toml")).unwrap();
        let mut land = vec![0u8; 84];
        // Row 5 is the belt. Column 5 is land between sea edges; column 6 is sea between land edges.
        land[3 * 12 + 5] = 255;
        land[2 * 12 + 6] = 255;
        land[4 * 12 + 6] = 255;
        write_image(&dir.0.join("land.png"), 12, 7, png::BitDepth::Eight, &land);
        let words = |f: &dyn Fn(usize, usize) -> u16| {
            (0..12)
                .flat_map(|_| (0..7).flat_map(|r| (0..12).flat_map(move |c| f(r, c).to_be_bytes())))
                .collect::<Vec<_>>()
        };
        let pressure = words(&|r, c| 41000 + u16::try_from(80 * r + 150 * c).unwrap());
        let air = words(&|_, _| 1035);
        let sea = words(&|r, c| if land[r * 12 + c] == 255 { 0 } else { 1000 });
        for (name, data) in [
            ("pressure.png", pressure),
            ("air_temperature.png", air),
            ("sea_temperature.png", sea),
        ] {
            write_image(&dir.0.join(name), 12, 84, png::BitDepth::Sixteen, &data);
        }
        let backdrop = ck_map::load_backdrop(&dir.0).unwrap();
        let mut rules =
            WeatherRules::parse(&fs::read_to_string(root.join("game/weather.toml")).unwrap())
                .unwrap();
        rules.wind_belt_latitude = 150_000;
        let field = WindField::new(&backdrop, &rules).unwrap();
        for (col, pair) in [(5, (70_000, 330)), (6, (15_000, 690))] {
            let (north, south) = (field.rule(col, 2, 0, pair), field.rule(col, 4, 0, pair));
            let whole = field.latitude(2) - field.latitude(4);
            let part = field.latitude(2) - field.latitude(3);
            let interpolate = |a: i64, b: i64| {
                round(i128::from(a * (whole - part) + b * part), i128::from(whole))
            };
            let actual = field.block(col, 3, 0);
            assert_eq!(
                actual,
                (interpolate(north.0, south.0), interpolate(north.1, south.1))
            );
            let other = if col == 5 {
                (15_000, 690)
            } else {
                (70_000, 330)
            };
            let (a, b) = (field.rule(col, 2, 0, other), field.rule(col, 4, 0, other));
            assert_ne!(actual, (interpolate(a.0, b.0), interpolate(a.1, b.1)));
        }
        rules.wind_belt_latitude = 0;
        let field = WindField::new(&backdrop, &rules).unwrap();
        assert_eq!(field.block(5, 3, 0), field.rule(5, 3, 0, (70_000, 330)));
        // Truncating the backdrop removes the southern belt edge; rule 7 then applies directly.
        let manifest = fs::read_to_string(dir.0.join("backdrop.toml"))
            .unwrap()
            .replace("rows = 7", "rows = 4")
            .replace(
                "[711279, 582007, 379549, 101559, -202277, -457261, -632957]",
                "[711279, 582007, 379549, 101559]",
            );
        fs::write(dir.0.join("backdrop.toml"), manifest).unwrap();
        write_image(
            &dir.0.join("land.png"),
            12,
            4,
            png::BitDepth::Eight,
            &[0; 48],
        );
        let words4 = |value: u16| {
            (0..12 * 4 * 12)
                .flat_map(|_| value.to_be_bytes())
                .collect::<Vec<_>>()
        };
        let pressure4 = (0..12)
            .flat_map(|_| {
                (0..4).flat_map(|r| {
                    (0..12).flat_map(move |c| {
                        (41000u16 + u16::try_from(80 * r + 150 * c).unwrap()).to_be_bytes()
                    })
                })
            })
            .collect::<Vec<_>>();
        write_image(
            &dir.0.join("pressure.png"),
            12,
            48,
            png::BitDepth::Sixteen,
            &pressure4,
        );
        for (name, value) in [("air_temperature.png", 1035), ("sea_temperature.png", 1000)] {
            write_image(
                &dir.0.join(name),
                12,
                48,
                png::BitDepth::Sixteen,
                &words4(value),
            );
        }
        let short = ck_map::load_backdrop(&dir.0).unwrap();
        rules.wind_belt_latitude = 150_000;
        let field = WindField::new(&short, &rules).unwrap();
        assert_eq!(field.block(5, 3, 0), field.rule(5, 3, 0, (15_000, 690)));
    }

    #[test]
    fn shipped_birr_months_and_display() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let backdrop = ck_map::load_backdrop(&root.join("map/backdrop")).unwrap();
        let rules =
            WeatherRules::parse(&std::fs::read_to_string(root.join("game/weather.toml")).unwrap())
                .unwrap();
        let field = WindField::new(&backdrop, &rules).unwrap();
        assert_eq!(
            field.block(356, 242 - i64::from(backdrop.first_row()), 0),
            (78, 211)
        );
        assert_eq!(field.monthly(22833, 15538, 0), Some((81, 208, 101_169)));
        assert_eq!(field.monthly(22833, 15538, 6), Some((92, 40, 101_546)));
        assert_eq!(display(257, 257), (7, "SW"));
        assert_eq!(display(25, 0), (0, ""));
    }
}
