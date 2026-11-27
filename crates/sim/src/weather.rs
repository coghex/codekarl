//! Deterministic climate temperatures derived from the map and weather tables.
use serde::Deserialize;

#[derive(Debug, Deserialize)]
/// Weather tables and swing settings loaded from the rules file.
pub struct WeatherRules {
    /// Swing at the sea edge, in tenths.
    pub swing_coast: i64,
    /// Swing far from the sea, in tenths.
    pub swing_inland: i64,
    /// Warmest moment after local noon, in minutes.
    pub lag_minutes: i64,
    /// Day-length fade interval, in minutes.
    pub fade_minutes: i64,
    /// Sea friction in thousand-millionths per second.
    pub wind_sea_friction: i64,
    /// Sea speed factor in thousandths.
    pub wind_sea_factor: i64,
    /// Land friction in thousand-millionths per second.
    pub wind_land_friction: i64,
    /// Land speed factor in thousandths.
    pub wind_land_factor: i64,
    /// Pressure slope radius in blocks.
    pub wind_slope_blocks: i64,
    /// Equatorial belt half width in ten-thousandths of a degree.
    pub wind_belt_latitude: i64,
    /// Friction used to steer lows.
    pub steer_friction: i64,
    /// Speed factor used to steer lows.
    pub steer_factor: i64,
    /// Temperature contrast in pascals per ten degrees.
    pub steer_contrast: i64,
    /// Smoothing radius in blocks.
    pub steer_smooth_blocks: i64,
    /// Number of smoothing passes.
    pub steer_smooth_passes: i64,
    /// Westward drift in centimetres per second.
    pub steer_drift_west: i64,
    /// Poleward drift in centimetres per second.
    pub steer_drift_pole: i64,
    /// Meeting distance as thousandths of the larger radius.
    pub meet_share: i64,
    /// Cosines in millionths at tenth-degree steps.
    pub cosine: Vec<i64>,
    /// Latitude tangents in millionths at tenth-degree steps.
    pub tangent: Vec<i64>,
    /// Daily solar declination tangents in millionths, including February 29.
    pub declination: Vec<i64>,
    /// Half-day minutes indexed by the cosine in thousandths.
    pub half_day: Vec<i64>,
}

impl WeatherRules {
    /// Parse and validate a weather rules file.
    ///
    /// # Errors
    /// Returns an error for invalid rules or a day outside the supported range.
    pub fn parse(text: &str) -> Result<Self, String> {
        let rules: Self = toml::from_str(text).map_err(|e| format!("weather.toml: {e}"))?;
        for (name, values, length) in [
            ("cosine", &rules.cosine, 3600),
            ("tangent", &rules.tangent, 851),
            ("declination", &rules.declination, 366),
            ("half_day", &rules.half_day, 2001),
        ] {
            if values.len() != length {
                return Err(format!("weather.toml: {name} length"));
            }
        }
        if rules
            .cosine
            .iter()
            .any(|v| !(-1_000_000..=1_000_000).contains(v))
        {
            return Err("weather.toml: cosine range".into());
        }
        if rules.tangent.iter().any(|v| *v < 0) || rules.tangent.windows(2).any(|v| v[0] > v[1]) {
            return Err("weather.toml: tangent order or range".into());
        }
        if rules.half_day.iter().any(|v| !(0..=720).contains(v))
            || rules.half_day.windows(2).any(|v| v[0] < v[1])
        {
            return Err("weather.toml: half_day order or range".into());
        }
        for (name, value, lower, upper) in [
            ("swing_coast", rules.swing_coast, 0, 300),
            ("swing_inland", rules.swing_inland, 0, 300),
            ("lag_minutes", rules.lag_minutes, 0, 720),
            ("fade_minutes", rules.fade_minutes, 1, 720),
            ("wind_sea_friction", rules.wind_sea_friction, 1, 1_000_000),
            ("wind_sea_factor", rules.wind_sea_factor, 0, 2_000),
            ("wind_land_friction", rules.wind_land_friction, 1, 1_000_000),
            ("wind_land_factor", rules.wind_land_factor, 0, 2_000),
            ("wind_slope_blocks", rules.wind_slope_blocks, 1, 8),
            ("wind_belt_latitude", rules.wind_belt_latitude, 0, 300_000),
            ("steer_friction", rules.steer_friction, 1, 1_000_000),
            ("steer_factor", rules.steer_factor, 0, 10_000),
            ("steer_contrast", rules.steer_contrast, 0, 20_000),
            ("steer_smooth_blocks", rules.steer_smooth_blocks, 0, 32),
            ("steer_smooth_passes", rules.steer_smooth_passes, 0, 8),
            ("steer_drift_west", rules.steer_drift_west, 0, 1_000),
            ("steer_drift_pole", rules.steer_drift_pole, 0, 1_000),
            ("meet_share", rules.meet_share, 1, 2_000),
        ] {
            if !(lower..=upper).contains(&value) {
                return Err(format!("weather.toml: {name} range"));
            }
        }
        Ok(rules)
    }
}

#[derive(Clone, Copy, Debug)]
/// Map and authored coordinate inputs for one place.
pub struct WeatherPlace {
    /// Authored latitude in ten-thousandths of a degree.
    pub latitude_e4: i64,
    /// Authored longitude in ten-thousandths of a degree.
    pub longitude_e4: i64,
    /// Cell mean in tenths of a degree.
    pub mean: i64,
    /// Cell seasonal half range in tenths.
    pub half_range: i64,
    /// Cell sea nearness in thousandths.
    pub nearness: i64,
    /// Warmest month phase in hundredths.
    pub phase: i64,
}

fn sun_time(longitude_e4: i64, hour: i64) -> i64 {
    (hour * 60 + round(longitude_e4 * 4, 10_000)).rem_euclid(1440)
}

fn round_wide(n: i128, d: i128) -> i128 {
    let quotient = n / d;
    let remainder = n % d;
    quotient
        + if remainder.abs() * 2 >= d {
            n.signum()
        } else {
            0
        }
}

fn round(n: i64, d: i64) -> i64 {
    n.signum() * ((2 * n.abs() + d) / (2 * d))
}

/// Julian day number for a Julian date.
#[must_use]
pub fn julian_day(year: i64, month: i64, day: i64) -> i64 {
    let a = (14 - month) / 12;
    let y = year + 4800 - a;
    let m = month + 12 * a - 3;
    day + (153 * m + 2) / 5 + 365 * y + y / 4 - 32_083
}

/// Gregorian date at a signed day offset from the clock's first day.
///
/// # Errors
/// Returns an error when the day is outside the supported range.
#[expect(
    clippy::many_single_char_names,
    reason = "the standard Gregorian day-number conversion uses these conventional intermediate names"
)]
pub fn gregorian(offset: i64) -> Result<(i64, i64, i64), String> {
    if !(-2..=julian_day(9999, 12, 31) - julian_day(1, 1, 1)).contains(&offset) {
        return Err("weather day outside supported range".into());
    }
    let a = julian_day(1, 1, 1) + offset + 32_044;
    let b = (4 * a + 3) / 146_097;
    let c = a - 146_097 * b / 4;
    let d = (4 * c + 3) / 1461;
    let e = c - 1461 * d / 4;
    let m = (5 * e + 2) / 153;
    Ok((
        100 * b + d - 4800 + m / 10,
        m + 3 - 12 * (m / 10),
        e - (153 * m + 2) / 5 + 1,
    ))
}

fn month_days(year: i64, month: i64, greg: bool) -> i64 {
    match month {
        2 if year % 4 == 0 && (!greg || year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

impl WeatherPlace {
    /// Seasonal contribution for a signed clock-day offset.
    ///
    /// # Errors
    /// Returns an error for invalid rules or a day outside the supported range.
    pub fn seasonal(self, rules: &WeatherRules, offset: i64) -> Result<i64, String> {
        let (year, month, day) = gregorian(offset)?;
        let length = month_days(year, month, true);
        let phase = self.phase + if self.latitude_e4 < 0 { 600 } else { 0 };
        let numerator = 300 * (2 * length * (100 * (month - 1) - 50 - phase) + 100 * (2 * day - 1));
        let index =
            usize::try_from(round(numerator, 200 * length).rem_euclid(3600)).unwrap_or_default();
        Ok(round(self.half_range * rules.cosine[index], 1_000_000))
    }
    /// Mean of the completed noon-to-noon day, without swing.
    ///
    /// # Errors
    /// Returns an error for invalid rules or a day outside the supported range.
    pub fn daily_mean(self, rules: &WeatherRules, offset: i64) -> Result<i64, String> {
        Ok(round(
            2 * self.mean + self.seasonal(rules, offset - 1)? + self.seasonal(rules, offset)?,
            2,
        ))
    }
    /// Monthly expected temperatures for the Julian starting year.
    ///
    /// # Errors
    /// Returns an error for invalid rules or a day outside the supported range.
    pub fn expected_year(self, rules: &WeatherRules, year: i64) -> Result<[i64; 12], String> {
        let mut result = [0; 12];
        for month in 1..=12 {
            let length = month_days(year, month, false);
            let start = julian_day(year, month, 1) - julian_day(1, 1, 1);
            let mut sum = 0;
            for day in 0..length {
                sum += self.daily_mean(rules, start + day)?;
            }
            result[usize::try_from(month - 1).unwrap_or_default()] = round(sum, length);
        }
        Ok(result)
    }
    /// Half of the daylight length in minutes.
    ///
    /// # Errors
    /// Returns an error for invalid rules or a day outside the supported range.
    pub fn half_day(self, rules: &WeatherRules, offset: i64) -> Result<i64, String> {
        let (_, month, day) = gregorian(offset)?;
        let starts = [0, 31, 60, 91, 121, 152, 182, 213, 244, 274, 305, 335];
        let tilt = rules.declination[usize::try_from(
            starts[usize::try_from(month - 1).unwrap_or_default()] + day - 1,
        )
        .unwrap_or_default()];
        let mag = self.latitude_e4.abs();
        if mag > 850_000 {
            return Err("latitude outside 85 degrees".into());
        }
        let index = usize::try_from(mag / 1000).unwrap_or_default();
        let rest = mag % 1000;
        let lower = i128::from(rules.tangent[index]);
        let tan = lower
            + if rest == 0 {
                0
            } else {
                round_wide(
                    (i128::from(rules.tangent[index + 1]) - lower) * i128::from(rest),
                    1000,
                )
            };
        let product = round_wide(
            -tan * i128::from(self.latitude_e4.signum()) * i128::from(tilt),
            1_000_000_000,
        )
        .clamp(-1000, 1000);
        Ok(rules.half_day[usize::try_from(product + 1000).unwrap_or_default()])
    }
    /// Temperature at a UTC hour in tenths of a degree.
    ///
    /// # Errors
    /// Returns an error for invalid rules or a day outside the supported range.
    pub fn hour(self, rules: &WeatherRules, offset: i64, hour: i64) -> Result<i64, String> {
        let half = self.half_day(rules, offset)?;
        let length = 2 * half;
        let mut swing = 0;
        if length != 0 && length != 1440 {
            let size = rules.swing_coast * 1000
                + (rules.swing_inland - rules.swing_coast) * (1000 - self.nearness);
            let fade = 1000
                .min(length * 1000 / rules.fade_minutes)
                .min((1440 - length) * 1000 / rules.fade_minutes);
            let reach = round(size * (1440 + length) * fade, 2160 * 1000 * 1000 * 2);
            let sun = sun_time(self.longitude_e4, hour);
            let rise = 720 - half;
            let peak = 720 + rules.lag_minutes.min(half / 2);
            swing = if (rise..=peak).contains(&sun) {
                let step =
                    usize::try_from(round(1800 * (sun - rise), peak - rise)).unwrap_or_default();
                round(-reach * rules.cosine[step], 1_000_000)
            } else {
                let step = usize::try_from(round(
                    1800 * (sun - peak).rem_euclid(1440),
                    1440 - (peak - rise),
                ))
                .unwrap_or_default();
                round(reach * rules.cosine[step], 1_000_000)
            };
        }
        Ok(self.mean + self.seasonal(rules, offset)? + swing)
    }

    /// Tenths of degree-hours below `base` in the 24 hours ending at noon.
    ///
    /// # Errors
    /// Returns an error if a temperature cannot be computed or the sum overflows.
    pub fn noon_degree_hours(
        self,
        rules: &WeatherRules,
        offset: i64,
        base: i64,
    ) -> Result<u64, String> {
        let previous = offset.checked_sub(1).ok_or("weather day underflow")?;
        let mut total = 0_u64;
        for (day, hours) in [(previous, 12..24), (offset, 0..12)] {
            for hour in hours {
                let temperature = self.hour(rules, day, hour)?;
                let difference = (i128::from(base) - i128::from(temperature)).max(0);
                let difference = u64::try_from(difference).map_err(|_| "degree-hours overflow")?;
                total = total
                    .checked_add(difference)
                    .ok_or("degree-hours overflow")?;
            }
        }
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> WeatherRules {
        WeatherRules::parse(include_str!("../../../assets/game/weather.toml")).unwrap()
    }

    #[test]
    fn noon_degree_hours_uses_the_previous_afternoon_and_current_morning() {
        let rules = rules();
        let place = WeatherPlace {
            latitude_e4: 530_000,
            longitude_e4: -80_000,
            mean: 90,
            half_range: 50,
            nearness: 500,
            phase: 0,
        };
        let expected: u64 =
            (12..24)
                .map(|h| u64::try_from((140 - place.hour(&rules, -1, h).unwrap()).max(0)).unwrap())
                .chain((0..12).map(|h| {
                    u64::try_from((140 - place.hour(&rules, 0, h).unwrap()).max(0)).unwrap()
                }))
                .sum();
        assert_eq!(place.noon_degree_hours(&rules, 0, 140).unwrap(), expected);
        assert_eq!(place.noon_degree_hours(&rules, 0, -10_000).unwrap(), 0);
    }

    #[test]
    fn hourly_reference_rows_follow_latitude_and_longitude() {
        let rules = rules();
        let places = [
            (
                531_000,
                -79_000,
                94,
                44,
                170,
                313,
                vec![
                    119, 131, 139, 142, 141, 140, 137, 132, 128, 122, 116, 109, 103, 96, 90, 85,
                    81, 77, 75, 74, 75, 82, 92, 106,
                ],
            ),
            (
                531_000,
                71_000,
                94,
                44,
                170,
                313,
                vec![
                    131, 139, 142, 141, 140, 137, 132, 128, 122, 116, 109, 103, 96, 90, 85, 81, 77,
                    75, 74, 75, 82, 92, 106, 119,
                ],
            ),
            (
                0,
                0,
                270,
                10,
                500,
                360,
                vec![
                    292, 299, 303, 304, 303, 300, 297, 292, 287, 281, 275, 268, 262, 257, 252, 248,
                    245, 243, 242, 244, 250, 259, 270, 281,
                ],
            ),
            (
                -340_000,
                180_000,
                170,
                60,
                800,
                384,
                vec![
                    175, 178, 178, 176, 174, 170, 166, 161, 156, 150, 145, 139, 136, 131, 128, 126,
                    125, 126, 129, 136, 144, 153, 163, 171,
                ],
            ),
            (
                700_000,
                250_000,
                -20,
                150,
                600,
                260,
                vec![
                    55, 56, 55, 53, 51, 48, 44, 40, 35, 30, 25, 21, 13, 9, 6, 3, 1, -1, -1, 2, 11,
                    23, 36, 46,
                ],
            ),
        ];
        let day = julian_day(768, 10, 9) - julian_day(1, 1, 1);
        for (latitude_e4, longitude_e4, mean, half_range, nearness, half, hours) in places {
            let place = WeatherPlace {
                latitude_e4,
                longitude_e4,
                mean,
                half_range,
                nearness,
                phase: 650,
            };
            assert_eq!(place.half_day(&rules, day).unwrap(), half);
            let actual: Vec<_> = (12..24)
                .map(|h| place.hour(&rules, day - 1, h).unwrap())
                .chain((0..12).map(|h| place.hour(&rules, day, h).unwrap()))
                .collect();
            assert_eq!(actual, hours);
        }
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the reference rows are compared together across each season and location"
    )]
    fn seasonal_year_and_extrema_match_the_remaining_reference_rows() {
        let rules = rules();
        assert_eq!(round(-78_750 * 4, 10_000), -32);
        assert_eq!(sun_time(-78_750, 12), 688);
        let specimens = [
            (
                WeatherPlace {
                    latitude_e4: 519_000,
                    longitude_e4: -103_000,
                    mean: 109,
                    half_range: 40,
                    nearness: 1000,
                    phase: 650,
                },
                (13, 12, 122),
                [70, 72, 83, 101, 122, 139, 148, 147, 135, 117, 97, 80],
                [(246, 49, 91, 70), (476, 121, 175, 148)],
            ),
            (
                WeatherPlace {
                    latitude_e4: 531_000,
                    longitude_e4: -79_000,
                    mean: 94,
                    half_range: 44,
                    nearness: 170,
                    phase: 650,
                },
                (14, 14, 108),
                [51, 53, 65, 86, 108, 127, 137, 135, 123, 103, 81, 62],
                [(240, 20, 82, 51), (482, 98, 176, 137)],
            ),
            (
                WeatherPlace {
                    latitude_e4: -340_000,
                    longitude_e4: 180_000,
                    mean: 170,
                    half_range: 60,
                    nearness: 800,
                    phase: 650,
                },
                (-19, -18, 152),
                [228, 226, 210, 182, 152, 126, 112, 114, 131, 158, 189, 215],
                [(419, 201, 256, 229), (300, 86, 137, 112)],
            ),
            (
                WeatherPlace {
                    latitude_e4: 700_000,
                    longitude_e4: 250_000,
                    mean: -20,
                    half_range: 150,
                    nearness: 600,
                    phase: 650,
                },
                (49, 46, 28),
                [-166, -161, -119, -50, 27, 91, 126, 121, 78, 10, -67, -131],
                [(0, -167, -166, -167), (720, 126, 127, 127)],
            ),
        ];
        let day = julian_day(768, 10, 9) - julian_day(1, 1, 1);
        for (place, (before, today, mean), year, extremes) in specimens {
            assert_eq!(
                (
                    place.seasonal(&rules, day - 1).unwrap(),
                    place.seasonal(&rules, day).unwrap(),
                    place.daily_mean(&rules, day).unwrap()
                ),
                (before, today, mean)
            );
            assert_eq!(place.expected_year(&rules, 768).unwrap(), year);
            for (month, expected) in [1, 7].into_iter().zip(extremes) {
                let date = julian_day(769, month, 15) - julian_day(1, 1, 1);
                let hours: Vec<_> = (12..24)
                    .map(|h| place.hour(&rules, date - 1, h).unwrap())
                    .chain((0..12).map(|h| place.hour(&rules, date, h).unwrap()))
                    .collect();
                assert_eq!(
                    (
                        place.half_day(&rules, date).unwrap(),
                        *hours.iter().min().unwrap(),
                        *hours.iter().max().unwrap(),
                        place.daily_mean(&rules, date).unwrap()
                    ),
                    expected
                );
            }
        }
        let equator = WeatherPlace {
            latitude_e4: 0,
            longitude_e4: 0,
            mean: 270,
            half_range: 10,
            nearness: 500,
            phase: 650,
        };
        assert_eq!(
            equator.expected_year(&rules, 768).unwrap(),
            [260, 261, 263, 268, 273, 277, 280, 279, 277, 272, 267, 263]
        );
        let inland = WeatherPlace {
            latitude_e4: 531_000,
            longitude_e4: -79_000,
            mean: 94,
            half_range: 44,
            nearness: 170,
            phase: 650,
        };
        assert_eq!(inland.daily_mean(&rules, day - 1).unwrap(), 109);
        assert_eq!(
            WeatherPlace {
                longitude_e4: 71_000,
                ..inland
            }
            .daily_mean(&rules, day - 1)
            .unwrap(),
            109
        );
    }

    #[test]
    fn clock_boundaries_and_polar_day_follow_the_reference() {
        let rules = rules();
        let north = WeatherPlace {
            latitude_e4: 700_000,
            longitude_e4: 250_000,
            mean: -20,
            half_range: 150,
            nearness: 600,
            phase: 650,
        };
        assert_eq!(north.daily_mean(&rules, -1).unwrap(), -146);
        assert_eq!(north.daily_mean(&rules, 0).unwrap(), -148);
        assert_eq!(north.expected_year(&rules, 1).unwrap()[11], -121);
        let inland = WeatherPlace {
            latitude_e4: 531_000,
            longitude_e4: -79_000,
            mean: 94,
            half_range: 44,
            nearness: 170,
            phase: 650,
        };
        let last = julian_day(9999, 12, 31) - julian_day(1, 1, 1);
        assert_eq!(gregorian(last).unwrap(), (10_000, 3, 13));
        assert_eq!(inland.daily_mean(&rules, last).unwrap(), 61);
        let january = julian_day(769, 1, 15) - julian_day(1, 1, 1);
        let july = julian_day(769, 7, 15) - julian_day(1, 1, 1);
        assert_eq!(north.half_day(&rules, january).unwrap(), 0);
        assert_eq!(north.half_day(&rules, july).unwrap(), 720);
    }

    #[test]
    fn reviewed_calendar_and_temperature_edges() {
        let rules = rules();
        let inland = WeatherPlace {
            latitude_e4: 531_000,
            longitude_e4: -79_000,
            mean: 94,
            half_range: 44,
            nearness: 170,
            phase: 650,
        };
        let epoch = julian_day(1, 1, 1);
        let leap = julian_day(768, 2, 29) - epoch;
        assert_eq!(gregorian(leap).unwrap(), (768, 3, 4));
        assert_eq!(inland.daily_mean(&rules, leap).unwrap(), 57);
        assert_eq!(
            gregorian(julian_day(769, 2, 28) - epoch).unwrap(),
            (769, 3, 4)
        );
        assert_eq!(inland.daily_mean(&rules, 0).unwrap(), 57);
        assert_eq!(
            inland.expected_year(&rules, 1).unwrap(),
            [53, 52, 62, 81, 104, 124, 136, 137, 126, 107, 85, 65]
        );
        let south = WeatherPlace {
            latitude_e4: -850_000,
            longitude_e4: 0,
            mean: 0,
            half_range: 100,
            nearness: 500,
            phase: 650,
        };
        let october9 = julian_day(768, 10, 9) - epoch;
        assert_eq!(south.daily_mean(&rules, october9).unwrap(), -32);
        let north = WeatherPlace {
            latitude_e4: 700_000,
            longitude_e4: 250_000,
            mean: -20,
            half_range: 150,
            nearness: 600,
            phase: 650,
        };
        let september30 = julian_day(768, 9, 30) - epoch;
        assert_eq!(north.daily_mean(&rules, september30).unwrap(), 48);
        assert_eq!(north.daily_mean(&rules, september30 + 1).unwrap(), 46);
        let year = north.expected_year(&rules, 768).unwrap();
        assert_eq!((year[8], year[9]), (78, 10));
    }

    #[test]
    fn invalid_settings_and_tables_are_named() {
        let source = include_str!("../../../assets/game/weather.toml");
        for (from, to, name) in [
            ("swing_coast = 48", "swing_coast = 301", "swing_coast"),
            ("swing_inland = 75", "swing_inland = -1", "swing_inland"),
            ("lag_minutes = 150", "lag_minutes = 721", "lag_minutes"),
            ("fade_minutes = 120", "fade_minutes = 0", "fade_minutes"),
            (
                "cosine = [\n    1000000,",
                "cosine = [\n    1000001,",
                "cosine",
            ),
            ("tangent = [\n    0,", "tangent = [\n    -1,", "tangent"),
            (
                "half_day = [\n    720,",
                "half_day = [\n    721,",
                "half_day",
            ),
            ("declination = [", "declination = [ 0,", "declination"),
        ] {
            let changed = source.replacen(from, to, 1);
            assert_ne!(changed, source);
            assert!(WeatherRules::parse(&changed).unwrap_err().contains(name));
        }
    }

    #[test]
    fn wind_settings_are_required_and_range_checked() {
        let source = include_str!("../../../assets/game/weather.toml");
        for (name, value, low, high) in [
            ("wind_sea_friction", 15000, 1, 1_000_000),
            ("wind_sea_factor", 690, 0, 2_000),
            ("wind_land_friction", 70000, 1, 1_000_000),
            ("wind_land_factor", 330, 0, 2_000),
            ("wind_slope_blocks", 2, 1, 8),
            ("wind_belt_latitude", 60000, 0, 300_000),
            ("steer_friction", 1000, 1, 1_000_000),
            ("steer_factor", 1000, 0, 10_000),
            ("steer_contrast", 1225, 0, 20_000),
            ("steer_smooth_blocks", 5, 0, 32),
            ("steer_smooth_passes", 3, 0, 8),
            ("steer_drift_west", 100, 0, 1_000),
            ("steer_drift_pole", 200, 0, 1_000),
            ("meet_share", 500, 1, 2_000),
        ] {
            let line = format!("{name} = {value}\n");
            assert!(source.contains(&line), "{name}");
            assert!(
                WeatherRules::parse(&source.replace(&line, ""))
                    .unwrap_err()
                    .contains(name)
            );
            for invalid in [low - 1, high + 1] {
                let changed = source.replace(&line, &format!("{name} = {invalid}\n"));
                assert!(WeatherRules::parse(&changed).unwrap_err().contains(name));
            }
        }
    }

    #[test]
    fn inland_and_southern_reference_years() {
        let rules = rules();
        let inland = WeatherPlace {
            latitude_e4: 531_000,
            longitude_e4: -79_000,
            mean: 94,
            half_range: 44,
            nearness: 170,
            phase: 650,
        };
        let south = WeatherPlace {
            latitude_e4: -340_000,
            longitude_e4: 180_000,
            mean: 170,
            half_range: 60,
            nearness: 800,
            phase: 650,
        };
        let day = julian_day(768, 10, 9) - julian_day(1, 1, 1);
        assert_eq!(inland.daily_mean(&rules, day).unwrap(), 108);
        assert_eq!(south.daily_mean(&rules, day).unwrap(), 152);
        assert_eq!(
            inland.expected_year(&rules, 768).unwrap(),
            [51, 53, 65, 86, 108, 127, 137, 135, 123, 103, 81, 62]
        );
        assert_eq!(
            south.expected_year(&rules, 768).unwrap(),
            [228, 226, 210, 182, 152, 126, 112, 114, 131, 158, 189, 215]
        );
        let edge = WeatherPlace {
            latitude_e4: 850_000,
            longitude_e4: 0,
            mean: 0,
            half_range: 100,
            nearness: 500,
            phase: 650,
        };
        assert_eq!(edge.half_day(&rules, day).unwrap(), 0);
        assert_eq!(edge.daily_mean(&rules, day).unwrap(), 32);
        assert_eq!(
            WeatherPlace {
                latitude_e4: -850_000,
                ..edge
            }
            .half_day(&rules, day)
            .unwrap(),
            720
        );
        assert!(
            WeatherPlace {
                latitude_e4: 850_001,
                ..edge
            }
            .half_day(&rules, day)
            .is_err()
        );
    }

    #[test]
    fn reference_numbers_and_calendar_boundaries() {
        let rules = rules();
        assert_eq!(rules.cosine[630], 453_990);
        assert_eq!(rules.tangent[417], 890_967);
        assert_eq!(rules.tangent[850], 11_430_052);
        assert_eq!(rules.half_day[439], 496);
        assert_eq!((round(-12, 24), round(12, 24), round(-11, 24)), (-1, 1, 0));
        assert_eq!(gregorian(-2).unwrap(), (0, 12, 28));
        assert!(gregorian(-3).is_err());
        let coast = WeatherPlace {
            latitude_e4: 519_000,
            longitude_e4: -103_000,
            mean: 109,
            half_range: 40,
            nearness: 1000,
            phase: 650,
        };
        let day = julian_day(768, 10, 9) - julian_day(1, 1, 1);
        assert_eq!(gregorian(day).unwrap(), (768, 10, 13));
        assert_eq!(coast.half_day(&rules, day).unwrap(), 315);
        assert_eq!(coast.daily_mean(&rules, day).unwrap(), 122);
        assert_eq!(
            coast.expected_year(&rules, 768).unwrap(),
            [70, 72, 83, 101, 122, 139, 148, 147, 135, 117, 97, 80]
        );
        let hours: Vec<_> = (12..24)
            .map(|h| coast.hour(&rules, day - 1, h).unwrap())
            .chain((0..12).map(|h| coast.hour(&rules, day, h).unwrap()))
            .collect();
        assert_eq!(
            hours,
            [
                128, 137, 142, 145, 145, 144, 142, 139, 136, 132, 128, 123, 118, 114, 110, 106,
                103, 101, 99, 98, 99, 102, 109, 118
            ]
        );
    }
}
