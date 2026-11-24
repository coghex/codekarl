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
        self.belt_block(k, r, month, pair, |field, k, r, month, pair| {
            field.rule(k, r, month, pair)
        })
    }

    fn belt_block(
        &self,
        k: i64,
        r: i64,
        month: usize,
        pair: (i64, i64),
        rule: impl Fn(&Self, i64, i64, usize, (i64, i64)) -> (i64, i64),
    ) -> (i64, i64) {
        let lat = self.latitude(r);
        let belt = self.rules.wind_belt_latitude;
        if lat.abs() < belt {
            let north = (0..i64::from(self.backdrop.rows()))
                .filter(|&q| self.latitude(q) >= belt)
                .next_back();
            let south = (0..i64::from(self.backdrop.rows())).find(|&q| self.latitude(q) <= -belt);
            if let (Some(n), Some(s)) = (north, south) {
                let a = rule(self, k, n, month, pair);
                let b = rule(self, k, s, month, pair);
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
        rule(self, k, r, month, pair)
    }
    pub(super) fn monthly(&self, i: u32, j: u32, month: usize) -> Option<(i64, i64, i64)> {
        self.interpolate(i, j, month, |field, col, row, month| {
            let (u, v) = field.block(col, row, month);
            (u, v, i64::from(field.sample(col, row, month).pressure_pa))
        })
    }

    #[expect(
        clippy::many_single_char_names,
        reason = "symbols match the reviewed weather equations"
    )]
    fn interpolate(
        &self,
        i: u32,
        j: u32,
        month: usize,
        block: impl Fn(&Self, i64, i64, usize) -> (i64, i64, i64),
    ) -> Option<(i64, i64, i64)> {
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
            let (u, v, p) = block(self, col, row, month);
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

impl WindField<'_> {
    #[expect(
        clippy::many_single_char_names,
        reason = "symbols match the reviewed weather equations"
    )]
    fn low_rule(&self, k: i64, r: i64, pair: (i64, i64), lows: &super::lows::Lows) -> (i64, i64) {
        let n = self.rules.wind_slope_blocks;
        let east = (k + n).rem_euclid(self.count);
        let west = (k - n).rem_euclid(self.count);
        let sx: i64 = (0..n)
            .map(|s| self.gap((k + s) % self.count) + self.gap((k - 1 - s).rem_euclid(self.count)))
            .sum();
        let north = (r - n).max(0);
        let south = (r + n).min(i64::from(self.backdrop.rows()) - 1);
        let b = i64::from(self.backdrop.block_cells());
        let sy = (south - north) * b;
        let lowered = |col: i64, row: i64| {
            let x = (col * b + b / 2) * 1000;
            let y = ((row + i64::from(self.backdrop.first_row())) * b + b / 2) * 1000;
            lows.entries
                .iter()
                .map(|low| self.lowering(low, x, y))
                .sum::<i64>()
        };
        let dx = lowered(west, r) - lowered(east, r);
        let dy = lowered(k, south) - lowered(k, north);
        let (c, sine) = self.trig(r);
        pressure_wind(dx, dy, sx, sy, c, sine, pair)
    }

    fn low_block(&self, k: i64, r: i64, month: usize, lows: &super::lows::Lows) -> (i64, i64) {
        let pair = if self.sample(k, r, month).land {
            (self.rules.wind_land_friction, self.rules.wind_land_factor)
        } else {
            (self.rules.wind_sea_friction, self.rules.wind_sea_factor)
        };
        self.belt_block(k, r, month, pair, |field, col, row, _, pair| {
            field.low_rule(col, row, pair, lows)
        })
    }

    #[expect(
        clippy::many_single_char_names,
        reason = "symbols match the reviewed weather equations"
    )]
    pub(super) fn at(
        &self,
        lows: &super::lows::Lows,
        i: u32,
        j: u32,
        year: i64,
        month: usize,
        date: i64,
    ) -> Option<(i64, i64, i64)> {
        let months = std::array::from_fn(|m| self.monthly(i, j, m).unwrap_or_default());
        self.monthly(i, j, 0)?;
        let (u, v, p) = day(&months, year, month, date);
        if lows.entries.is_empty() {
            return Some((u, v, p));
        }
        let (du, dv, _) = self.interpolate(i, j, 0, |field, col, row, m| {
            let (u, v) = field.low_block(col, row, m, lows);
            (u, v, 0)
        })?;
        let x = i64::from(i) * 1000 + 500;
        let y = i64::from(j) * 1000 + 500;
        let pressure = p - lows
            .entries
            .iter()
            .map(|low| self.lowering(low, x, y))
            .sum::<i64>();
        Some((u + du, v + dv, pressure))
    }

    fn steering(&self, i: u32, j: u32, year: i64, month: usize, date: i64) -> Option<(i64, i64)> {
        let pair = (self.rules.steer_friction, self.rules.steer_factor);
        let mut months = [(0, 0, 0); 12];
        for (m, value) in months.iter_mut().enumerate() {
            *value = self.interpolate(i, j, m, |field, col, row, m| {
                let (u, v) = field.belt_block(col, row, m, pair, Self::rule);
                (u, v, 0)
            })?;
        }
        let (u, v, _) = day(&months, year, month, date);
        Some((u, v))
    }

    #[expect(
        clippy::many_single_char_names,
        clippy::too_many_lines,
        reason = "the hourly sequence follows the reviewed equations"
    )]
    pub(super) fn step(
        &self,
        list: &mut super::lows::Lows,
        year: i64,
        month: usize,
        date: i64,
    ) -> Vec<super::lows::Ending> {
        use super::lows::{EndKind, Ending};
        let mut pairs = Vec::new();
        for (i, a) in list.entries.iter().enumerate() {
            for b in &list.entries[i + 1..] {
                let d = self.distance_squared(a.x, a.y, b.x, b.y);
                let threshold = self.rules.meet_share * a.radius.max(b.radius);
                if i128::from(d) * 1_000_000 < i128::from(threshold).pow(2) {
                    pairs.push((d, a.number, b.number));
                }
            }
        }
        pairs.sort_unstable();
        let mut joined = std::collections::BTreeSet::new();
        let mut ended = Vec::new();
        for (_, a, b) in pairs {
            if joined.contains(&a) || joined.contains(&b) {
                continue;
            }
            let ia = list
                .entries
                .iter()
                .position(|low| low.number == a)
                .expect("pair exists");
            let ib = list
                .entries
                .iter()
                .position(|low| low.number == b)
                .expect("pair exists");
            let left = &list.entries[ia];
            let right = &list.entries[ib];
            let bigger_left = (
                self.strength(left),
                left.radius,
                std::cmp::Reverse(left.number),
            ) >= (
                self.strength(right),
                right.radius,
                std::cmp::Reverse(right.number),
            );
            let (big, small) = if bigger_left {
                (left.clone(), right.clone())
            } else {
                (right.clone(), left.clone())
            };
            let amount_big = i128::from(big.depth) * i128::from(big.radius).pow(2);
            let amount_small = i128::from(small.depth) * i128::from(small.radius).pow(2);
            let total = amount_big + amount_small;
            let world = WORLD * 1000;
            let mut dx = (small.x - big.x).rem_euclid(world);
            if dx * 2 > world {
                dx -= world;
            }
            let mut merged = big.clone();
            merged.x = (big.x + round(i128::from(dx) * amount_small, total)).rem_euclid(world);
            merged.y = round(
                i128::from(big.y) * amount_big + i128::from(small.y) * amount_small,
                total,
            );
            merged.radius = big.radius.max(small.radius);
            merged.depth = round(total, i128::from(merged.radius).pow(2)).min(15_000);
            merged.strongest = merged.strongest.max(self.strength(&merged));
            ended.push(Ending {
                number: small.number,
                kind: EndKind::Merged(big.number),
                cell: (merged.x / 1000, merged.y / 1000),
                strongest: small.strongest,
            });
            joined.insert(a);
            joined.insert(b);
            list.entries.retain(|low| low.number != small.number);
            *list
                .entries
                .iter_mut()
                .find(|low| low.number == big.number)
                .expect("survivor") = merged;
        }
        let b = i64::from(self.backdrop.block_cells()) * 1000;
        let first = i64::from(self.backdrop.first_row()) * b;
        let end = (i64::from(self.backdrop.first_row()) + i64::from(self.backdrop.rows())) * b;
        for low in &mut list.entries {
            let i = u32::try_from(low.x / 1000).expect("valid x");
            let j = u32::try_from(low.y / 1000).expect("valid y");
            let (u, v) = self
                .steering(i, j, year, month, date)
                .expect("valid centre");
            let c = self.cos(self.point_latitude(low.y).abs());
            low.x = (low.x + round(i128::from(u) * 36_000_000_000, i128::from(839 * c)))
                .rem_euclid(WORLD * 1000);
            low.y -= round(i128::from(v) * 36_000_000_000, i128::from(839 * c));
            if (first..end).contains(&low.y) {
                low.strongest = low.strongest.max(self.strength(low));
            } else {
                ended.push(Ending {
                    number: low.number,
                    kind: EndKind::Left,
                    cell: (low.x.div_euclid(1000), low.y.div_euclid(1000)),
                    strongest: low.strongest,
                });
            }
        }
        list.entries.retain(|low| (first..end).contains(&low.y));
        ended
    }

    pub(super) fn validate_list(&self, list: &super::lows::Lows) -> Result<(), String> {
        let b = i64::from(self.backdrop.block_cells()) * 1000;
        let first = i64::from(self.backdrop.first_row()) * b;
        let end = (i64::from(self.backdrop.first_row()) + i64::from(self.backdrop.rows())) * b;
        let mut previous = 0;
        if list.next == 0 {
            return Err("weather list: next number".into());
        }
        for low in &list.entries {
            if low.number <= previous || low.number >= list.next {
                return Err("weather list: number order".into());
            }
            if !(0..WORLD * 1000).contains(&low.x) || !(first..end).contains(&low.y) {
                return Err("weather list: centre".into());
            }
            if !(50_000..=3_000_000).contains(&low.radius) {
                return Err("weather list: radius".into());
            }
            if !(1..=15_000).contains(&low.depth) {
                return Err("weather list: depth".into());
            }
            if low.strongest < 0 {
                return Err("weather list: strongest wind".into());
            }
            previous = low.number;
        }
        Ok(())
    }

    pub(super) fn place(
        &self,
        list: &mut super::lows::Lows,
        i: u32,
        j: u32,
        radius: i64,
        depth: i64,
    ) -> Result<(), String> {
        let next = list
            .next
            .checked_add(1)
            .ok_or("weather list: next number exhausted")?;
        let mut low = super::lows::Low {
            number: list.next,
            x: i64::from(i) * 1000 + 500,
            y: i64::from(j) * 1000 + 500,
            radius,
            depth,
            strongest: 0,
        };
        let mut candidate = list.clone();
        candidate.next = next;
        candidate.entries.push(low.clone());
        self.validate_list(&candidate)?;
        low.strongest = self.strength(&low);
        list.next = next;
        list.entries.push(low);
        Ok(())
    }

    /// Latitude at a point measured in thousandths of a cell.
    #[expect(
        clippy::many_single_char_names,
        reason = "symbols match the latitude equation"
    )]
    fn point_latitude(&self, y: i64) -> i64 {
        let b = i64::from(self.backdrop.block_cells()) * 1000;
        let t = y - (i64::from(self.backdrop.first_row()) * b + b / 2);
        let last = i64::from(self.backdrop.rows()) - 1;
        if t <= 0 {
            return self.latitude(0);
        }
        if t >= b * last {
            return self.latitude(last);
        }
        let r = t / b;
        let o = t % b;
        round(
            i128::from(self.latitude(r)) * i128::from(b - o)
                + i128::from(self.latitude(r + 1)) * i128::from(o),
            i128::from(b),
        )
    }

    fn distance_squared(&self, ax: i64, ay: i64, bx: i64, by: i64) -> i64 {
        let (la, lb) = (self.point_latitude(ay), self.point_latitude(by));
        let world = WORLD * 1000;
        let mut dx = (ax - bx).rem_euclid(world);
        if dx * 2 > world {
            dx -= world;
        }
        let c = self.cos(round(i128::from((la + lb).abs()), 2));
        let east = round(i128::from(dx) * 839 * i128::from(c), 1_000_000_000);
        let north = round(i128::from(la - lb) * 11_131_949, 1_000_000);
        east * east + north * north
    }

    fn lowering(&self, low: &super::lows::Low, x: i64, y: i64) -> i64 {
        let d2 = self.distance_squared(low.x, low.y, x, y);
        shape(d2, low.radius, low.depth)
    }

    pub(super) fn strength(&self, low: &super::lows::Low) -> i64 {
        let mut lat = self.point_latitude(low.y).abs();
        let belt = self.rules.wind_belt_latitude;
        let has_belt = belt > 0
            && self.latitude(0) >= belt
            && self.latitude(i64::from(self.backdrop.rows()) - 1) <= -belt;
        if has_belt {
            lat = lat.max(belt);
        }
        let sine = self.cos(900_000 - lat);
        let f = round(i128::from(145_842 * sine), 1_000_000);
        let e = self.rules.wind_sea_friction;
        let square = u64::try_from(e * e + f * f).expect("validated rates");
        let mut root = square.isqrt();
        if square - root * root > root {
            root += 1;
        }
        round(
            i128::from(self.rules.wind_sea_factor) * 15_396 * i128::from(low.depth) * 10_000_000,
            i128::from(low.radius) * 1_225 * i128::from(root),
        )
    }
}

type SteadyCache =
    std::cell::RefCell<std::collections::HashMap<(i64, i64, usize), (i64, i64, i64)>>;

fn shape(distance_squared: i64, radius: i64, depth: i64) -> i64 {
    let radius_squared = radius * radius;
    if distance_squared >= radius_squared {
        return 0;
    }
    let q = round(
        i128::from(distance_squared) * 1_000_000,
        i128::from(radius_squared),
    );
    let g = round(i128::from(1_000_000 - q).pow(2), 1_000_000);
    round(i128::from(depth) * i128::from(g), 1_000_000)
}

/// Per-picture block cache; a new instance binds the unchanged list for one image.
pub(super) struct PictureField<'a, 'b> {
    field: &'a WindField<'b>,
    lows: &'a super::lows::Lows,
    steady: SteadyCache,
    low: std::cell::RefCell<std::collections::HashMap<(i64, i64), (i64, i64)>>,
}

impl<'a, 'b> PictureField<'a, 'b> {
    pub(super) fn new(field: &'a WindField<'b>, lows: &'a super::lows::Lows) -> Self {
        Self {
            field,
            lows,
            steady: std::cell::RefCell::default(),
            low: std::cell::RefCell::default(),
        }
    }
    #[expect(
        clippy::many_single_char_names,
        reason = "symbols match the interpolation equations"
    )]
    pub(super) fn at(
        &self,
        i: u32,
        j: u32,
        year: i64,
        month: usize,
        date: i64,
    ) -> Option<(i64, i64, i64)> {
        let (other, _) = month_pair(year, month, date);
        let mut months = [(0, 0, 0); 12];
        for m in [month - 1, other - 1] {
            months[m] = self.field.interpolate(i, j, m, |field, col, row, m| {
                let key = (col, row, m);
                if let Some(value) = self.steady.borrow().get(&key) {
                    return *value;
                }
                let (u, v) = field.block(col, row, m);
                let result = (u, v, i64::from(field.sample(col, row, m).pressure_pa));
                self.steady.borrow_mut().insert(key, result);
                result
            })?;
        }
        let (u, v, p) = day(&months, year, month, date);
        if self.lows.entries.is_empty() {
            return Some((u, v, p));
        }
        let (du, dv, _) = self.field.interpolate(i, j, 0, |field, col, row, m| {
            let key = (col, row);
            let cached = self.low.borrow().get(&key).copied();
            let (u, v) = if let Some(value) = cached {
                value
            } else {
                let value = field.low_block(col, row, m, self.lows);
                self.low.borrow_mut().insert(key, value);
                value
            };
            (u, v, 0)
        })?;
        let x = i64::from(i) * 1000 + 500;
        let y = i64::from(j) * 1000 + 500;
        let lowered: i64 = self
            .lows
            .entries
            .iter()
            .map(|low| self.field.lowering(low, x, y))
            .sum();
        Some((u + du, v + dv, p - lowered))
    }
}

fn month_pair(year: i64, month: usize, date: i64) -> (usize, i64) {
    let days = match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if 2 * date > days {
        (month % 12 + 1, year + i64::from(month == 12))
    } else {
        (
            if month == 1 { 12 } else { month - 1 },
            year - i64::from(month == 1),
        )
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
    fn placed_low_has_reviewed_strength_and_shape() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let backdrop = ck_map::load_backdrop(&root.join("map/backdrop")).unwrap();
        let rules =
            WeatherRules::parse(&std::fs::read_to_string(root.join("game/weather.toml")).unwrap())
                .unwrap();
        let field = WindField::new(&backdrop, &rules).unwrap();
        let mut list = super::super::lows::Lows::default();
        field
            .place(&mut list, 21361, 15777, 500_000, 3_000)
            .unwrap();
        let low = &list.entries[0];
        assert_eq!(
            (low.number, low.x, low.y, low.strongest),
            (1, 21_361_500, 15_777_500, 4489)
        );
        assert_eq!(field.lowering(low, low.x, low.y), 3000);
        assert_eq!(field.distance_squared(low.x, low.y, low.x, low.y), 0);
        assert!(field.place(&mut list, 21361, 15777, 49_999, 3000).is_err());
        assert_eq!(list.next, 2);
        let mut invalid = list.clone();
        invalid.entries.push(invalid.entries[0].clone());
        assert!(field.validate_list(&invalid).is_err());
    }

    #[test]
    fn trial_b_merges_and_travels() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let backdrop = ck_map::load_backdrop(&root.join("map/backdrop")).unwrap();
        let rules =
            WeatherRules::parse(&std::fs::read_to_string(root.join("game/weather.toml")).unwrap())
                .unwrap();
        let field = WindField::new(&backdrop, &rules).unwrap();
        let mut list = super::super::lows::Lows::default();
        field.place(&mut list, 21361, 15777, 500_000, 3000).unwrap();
        field.place(&mut list, 21892, 15336, 800_000, 2000).unwrap();
        assert_eq!(
            field.distance_squared(
                list.entries[0].x,
                list.entries[0].y,
                list.entries[1].x,
                list.entries[1].y
            ),
            121_442_909_458
        );
        let offset = ck_sim::weather::julian_day(769, 1, 3) - ck_sim::weather::julian_day(1, 1, 1);
        let (y, m, d) = ck_sim::weather::gregorian(offset).unwrap();
        let ended = field.step(&mut list, y, usize::try_from(m).unwrap(), d);
        assert_eq!(ended[0].cell, (21696, 15499));
        let low = &list.entries[0];
        assert_eq!(
            (
                low.number,
                low.x,
                low.y,
                low.radius,
                low.depth,
                low.strongest
            ),
            (1, 21_746_472, 15_468_291, 800_000, 3172, 4489)
        );
    }

    #[test]
    fn equal_distance_order_is_stable_for_every_input_permutation() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let backdrop = ck_map::load_backdrop(&root.join("map/backdrop")).unwrap();
        let rules =
            WeatherRules::parse(&std::fs::read_to_string(root.join("game/weather.toml")).unwrap())
                .unwrap();
        let field = WindField::new(&backdrop, &rules).unwrap();
        let mut original = super::super::lows::Lows::default();
        for i in [20002, 19902, 20102] {
            field.place(&mut original, i, 16199, 600_000, 2000).unwrap();
        }
        for order in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let entries = order
                .into_iter()
                .map(|i| original.entries[i].clone())
                .collect();
            let mut list = super::super::lows::Lows::from_entries(4, entries);
            assert_eq!(list, original);
            let events = field.step(&mut list, 769, 1, 7);
            assert_eq!(
                (events[0].number, &events[0].kind, events[0].cell),
                (2, &super::super::lows::EndKind::Merged(1), (19952, 16199))
            );
            let low = &list.entries[0];
            assert_eq!(
                (low.x, low.y, low.depth, low.strongest),
                (20_003_699, 16_182_679, 4000, 5129)
            );
            let events = field.step(&mut list, 769, 1, 7);
            assert_eq!(
                (events[0].number, &events[0].kind, events[0].cell),
                (3, &super::super::lows::EndKind::Merged(1), (20054, 16182))
            );
            let low = &list.entries[0];
            assert_eq!(
                (low.x, low.y, low.depth, low.strongest),
                (20_106_478, 16_165_338, 6000, 7684)
            );
        }
    }

    #[test]
    fn merged_low_leaving_in_same_hour_keeps_joined_strength() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let backdrop = ck_map::load_backdrop(&root.join("map/backdrop")).unwrap();
        let rules =
            WeatherRules::parse(&std::fs::read_to_string(root.join("game/weather.toml")).unwrap())
                .unwrap();
        let field = WindField::new(&backdrop, &rules).unwrap();
        let mut list = super::super::lows::Lows::default();
        for _ in 0..2 {
            field.place(&mut list, 17120, 8480, 400_000, 1000).unwrap();
        }
        let endings = field.step(&mut list, 769, 1, 7);
        assert_eq!(endings.len(), 2);
        assert_eq!((endings[0].number, endings[0].strongest), (2, 1531));
        assert_eq!(
            (endings[1].number, endings[1].cell, endings[1].strongest),
            (1, (17058, 8327), 3061)
        );
        assert!(list.entries.is_empty());
        assert_eq!(list.next, 3);
    }

    #[test]
    fn three_lows_remain_ordered_across_a_save_and_reload() {
        use ck_sim::Canonical;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let backdrop = ck_map::load_backdrop(&root.join("map/backdrop")).unwrap();
        let rules =
            WeatherRules::parse(&std::fs::read_to_string(root.join("game/weather.toml")).unwrap())
                .unwrap();
        let field = WindField::new(&backdrop, &rules).unwrap();
        let mut original = super::super::lows::Lows::default();
        for (i, depth) in [(19902, 1500), (20202, 2500), (20462, 2000)] {
            field
                .place(&mut original, i, 16199, 600_000, depth)
                .unwrap();
        }
        for order in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let entries = order
                .into_iter()
                .map(|i| original.entries[i].clone())
                .collect();
            let mut list = super::super::lows::Lows::from_entries(4, entries);
            let events = field.step(&mut list, 769, 1, 7);
            assert_eq!(
                (
                    events[0].number,
                    &events[0].kind,
                    events[0].cell,
                    events[0].strongest
                ),
                (
                    3,
                    &super::super::lows::EndKind::Merged(2),
                    (20318, 16199),
                    2564
                )
            );
            assert_eq!(
                (list.entries[0].x, list.entries[0].y),
                (19_953_298, 16_183_146)
            );
            assert_eq!(
                (
                    list.entries[1].number,
                    list.entries[1].x,
                    list.entries[1].y,
                    list.entries[1].depth,
                    list.entries[1].strongest
                ),
                (2, 20_369_922, 16_179_608, 4500, 5770)
            );
            list = super::super::lows::Lows::from_bytes(&list.to_bytes().unwrap()).unwrap();
            let events = field.step(&mut list, 769, 1, 7);
            assert_eq!(
                (
                    events[0].number,
                    &events[0].kind,
                    events[0].cell,
                    events[0].strongest
                ),
                (
                    1,
                    &super::super::lows::EndKind::Merged(2),
                    (20265, 16180),
                    1923
                )
            );
            assert_eq!(
                (
                    list.entries[0].number,
                    list.entries[0].x,
                    list.entries[0].y,
                    list.entries[0].depth,
                    list.entries[0].strongest
                ),
                (2, 20_318_334, 16_161_165, 6000, 7683)
            );
            field.step(&mut list, 769, 1, 7);
            assert_eq!(
                (
                    list.entries[0].x,
                    list.entries[0].y,
                    list.entries[0].strongest
                ),
                (20_369_865, 16_140_526, 7683)
            );
        }
    }

    #[test]
    fn wrap_pressure_wind_and_merge_match_worked_example() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let backdrop = ck_map::load_backdrop(&root.join("map/backdrop")).unwrap();
        let rules =
            WeatherRules::parse(&std::fs::read_to_string(root.join("game/weather.toml")).unwrap())
                .unwrap();
        let field = WindField::new(&backdrop, &rules).unwrap();
        let mut list = super::super::lows::Lows::default();
        field.place(&mut list, 47700, 15776, 500_000, 3000).unwrap();
        let picture = PictureField::new(&field, &list);
        for (i, pressure, u, v) in [
            (47200, 98958, 3112, -3368),
            (47600, 97756, 3643, -551),
            (47765, 97724, 3413, 1121),
            (0, 97725, 3411, 1129),
            (200, 98098, 2925, 3032),
            (600, 99632, 1433, 3472),
        ] {
            assert_eq!(field.at(&list, i, 16076, 769, 1, 7), Some((u, v, pressure)));
            assert_eq!(picture.at(i, 16076, 769, 1, 7), Some((u, v, pressure)));
        }
        drop(picture);
        field.place(&mut list, 100, 15776, 800_000, 2000).unwrap();
        let events = field.step(&mut list, 769, 1, 7);
        assert_eq!(events[0].cell, (39, 15776));
        assert_eq!(
            (list.entries[0].x, list.entries[0].y, list.entries[0].depth),
            (55_829, 15_776_221, 3172)
        );
    }

    #[test]
    fn shape_and_strength_match_the_reviewed_tables() {
        for (distance, expected) in [
            (0_i64, 3000),
            (100_000, 2765),
            (250_000, 1688),
            (288_675, 1333),
            (400_000, 389),
            (499_999, 0),
            (500_000, 0),
        ] {
            assert_eq!(shape(distance * distance, 500_000, 3000), expected);
        }
        assert_eq!(shape(440_000_i64.pow(2), 800_000, 1), 0);
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let backdrop = ck_map::load_backdrop(&root.join("map/backdrop")).unwrap();
        let mut rules =
            WeatherRules::parse(&std::fs::read_to_string(root.join("game/weather.toml")).unwrap())
                .unwrap();
        let field = WindField::new(&backdrop, &rules).unwrap();
        let mut low = super::super::lows::Low {
            number: 1,
            x: 22_816_000,
            y: 15_520_000,
            radius: 500_000,
            depth: 3000,
            strongest: 0,
        };
        for (row, radius, depth, expected) in [
            (242, 500_000, 3000, 4420),
            (242, 1_200_000, 3000, 1842),
            (242, 800_000, 2000, 1842),
            (330, 500_000, 3000, 9923),
            (373, 500_000, 3000, 24329),
            (360, 500_000, 3000, 24128),
            (242, 3_000_000, 1, 0),
            (360, 50_000, 15000, 1_206_404),
        ] {
            low.y = (row * 64 + 32) * 1000;
            low.radius = radius;
            low.depth = depth;
            assert_eq!(field.strength(&low), expected);
        }
        rules.wind_belt_latitude = 0;
        let field = WindField::new(&backdrop, &rules).unwrap();
        low.y = (373 * 64 + 32) * 1000;
        low.radius = 500_000;
        low.depth = 3000;
        assert_eq!(field.strength(&low), 34_674);
    }

    #[test]
    fn low_block_winds_and_rounding_witness() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let backdrop = ck_map::load_backdrop(&root.join("map/backdrop")).unwrap();
        let rules =
            WeatherRules::parse(&std::fs::read_to_string(root.join("game/weather.toml")).unwrap())
                .unwrap();
        let field = WindField::new(&backdrop, &rules).unwrap();
        let low = super::super::lows::Low {
            number: 1,
            x: 22_816_000,
            y: 15_520_000,
            radius: 500_000,
            depth: 3000,
            strongest: 0,
        };
        let list = super::super::lows::Lows::from_entries(2, vec![low]);
        for (col, row, expected) in [
            (356, 242, (4, 2)),
            (360, 242, (-559, 971)),
            (352, 242, (393, -2682)),
            (356, 238, (-943, -557)),
            (356, 246, (981, 598)),
            (362, 236, (-3263, 2643)),
            (348, 246, (2544, -3560)),
            (370, 242, (-355, 662)),
        ] {
            assert_eq!(
                field.low_block(col, row - i64::from(backdrop.first_row()), 0, &list),
                expected
            );
        }
        let mut trial = super::super::lows::Lows::default();
        field
            .place(&mut trial, 21361, 15777, 500_000, 3000)
            .unwrap();
        let row = 246 - i64::from(backdrop.first_row());
        assert_eq!(field.block(333, row, 0), (470, 258));
        assert_eq!(field.low_block(333, row, 0, &trial), (19, -205));
        let steady = field.block(333, row, 0);
        let added = field.low_block(333, row, 0, &trial);
        assert_eq!((steady.0 + added.0, steady.1 + added.1), (489, 53));
    }

    #[test]
    fn equal_lows_choose_lower_number_and_clamp_depth() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let backdrop = ck_map::load_backdrop(&root.join("map/backdrop")).unwrap();
        let rules =
            WeatherRules::parse(&std::fs::read_to_string(root.join("game/weather.toml")).unwrap())
                .unwrap();
        let field = WindField::new(&backdrop, &rules).unwrap();
        for (depth, joined_depth, strongest) in [(2000, 4000, 5129), (9000, 15000, 19233)] {
            let mut list = super::super::lows::Lows {
                next: 4,
                entries: vec![],
            };
            field
                .place(&mut list, 19902, 16199, 600_000, depth)
                .unwrap();
            field
                .place(&mut list, 20002, 16199, 600_000, depth)
                .unwrap();
            let endings = field.step(&mut list, 769, 1, 7);
            assert_eq!(
                (endings[0].number, &endings[0].kind, endings[0].cell),
                (5, &super::super::lows::EndKind::Merged(4), (19952, 16199))
            );
            assert_eq!(
                (
                    list.entries[0].number,
                    list.entries[0].depth,
                    list.entries[0].strongest
                ),
                (4, joined_depth, strongest)
            );
        }
    }

    #[test]
    fn placement_validates_ranges_and_numbers_survive_a_departure() {
        use ck_sim::Canonical;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let backdrop = ck_map::load_backdrop(&root.join("map/backdrop")).unwrap();
        let rules =
            WeatherRules::parse(&std::fs::read_to_string(root.join("game/weather.toml")).unwrap())
                .unwrap();
        let field = WindField::new(&backdrop, &rules).unwrap();
        let mut list = super::super::lows::Lows {
            next: 7,
            entries: vec![],
        };
        for (i, j, radius, depth) in [
            (47766, 8608, 400_000, 1000),
            (1, 8447, 400_000, 1000),
            (1, 33920, 400_000, 1000),
            (1, 8608, 49_999, 1000),
            (1, 8608, 3_000_001, 1000),
            (1, 8608, 400_000, 0),
            (1, 8608, 400_000, 15_001),
        ] {
            assert!(field.place(&mut list, i, j, radius, depth).is_err());
            assert_eq!(list.next, 7);
        }
        field.place(&mut list, 17120, 8608, 400_000, 1000).unwrap();
        field.step(&mut list, 769, 1, 7);
        field.step(&mut list, 769, 1, 7);
        assert!(list.entries.is_empty());
        assert_eq!(list.next, 8);
        let mut loaded = super::super::lows::Lows::from_bytes(&list.to_bytes().unwrap()).unwrap();
        field
            .place(&mut loaded, 17120, 8608, 400_000, 1000)
            .unwrap();
        assert_eq!(loaded.entries[0].number, 8);
        assert_eq!(loaded.next, 9);
    }

    #[test]
    fn latitude_and_distance_tables() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let backdrop = ck_map::load_backdrop(&root.join("map/backdrop")).unwrap();
        let rules =
            WeatherRules::parse(&std::fs::read_to_string(root.join("game/weather.toml")).unwrap())
                .unwrap();
        let field = WindField::new(&backdrop, &rules).unwrap();
        for (y, latitude) in [
            (8_448_001, 749_780),
            (8_480_000, 749_780),
            (15_520_000, 531_793),
            (15_536_000, 531_068),
            (15_538_500, 530_954),
            (15_584_000, 528_892),
            (33_919_999, -599_766),
        ] {
            assert_eq!(field.point_latitude(y), latitude);
        }
        // Find a point at each table latitude without duplicating the distance equation.
        let at_latitude = |wanted: i64| {
            let (mut north, mut south) = (8_448_000, 33_919_999);
            while north < south {
                let mid = i64::midpoint(north, south);
                if field.point_latitude(mid) > wanted {
                    north = mid + 1;
                } else {
                    south = mid;
                }
            }
            assert_eq!(field.point_latitude(north), wanted);
            north
        };
        for (ax, ay, bx, by, square) in [
            (
                0,
                at_latitude(535_000),
                0,
                at_latitude(525_000),
                12_391_919_761,
            ),
            (1_000_000, 15_520_000, 0, 15_520_000, 252_831_974_976),
            (
                47_700_500,
                at_latitude(0),
                100_500,
                at_latitude(0),
                19_397_247_076,
            ),
            (
                23_883_000,
                at_latitude(600_000),
                0,
                at_latitude(600_000),
                100_378_737_928_561,
            ),
            (
                22_833_500,
                15_538_500,
                22_484_500,
                15_826_500,
                53_566_091_204,
            ),
        ] {
            assert_eq!(field.distance_squared(ax, ay, bx, by), square);
        }
    }

    #[test]
    fn centre_block_and_cell_worked_example_in_full() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let backdrop = ck_map::load_backdrop(&root.join("map/backdrop")).unwrap();
        let rules =
            WeatherRules::parse(&std::fs::read_to_string(root.join("game/weather.toml")).unwrap())
                .unwrap();
        let field = WindField::new(&backdrop, &rules).unwrap();
        let low = super::super::lows::Low {
            number: 1,
            x: 22_816_000,
            y: 15_520_000,
            radius: 500_000,
            depth: 3000,
            strongest: 0,
        };
        let block_centre = |col: i64, row: i64| ((col * 64 + 32) * 1000, (row * 64 + 32) * 1000);
        for (col, row, expected) in [
            (362, 242, 2172),
            (358, 242, 2901),
            (360, 240, 2530),
            (360, 244, 2517),
        ] {
            let (x, y) = block_centre(col, row);
            assert_eq!(field.lowering(&low, x, y), expected);
        }
        let list = super::super::lows::Lows::from_entries(2, vec![low]);
        let first = i64::from(backdrop.first_row());
        for (col, row, expected) in [
            (360, 242, (-559, 971)),
            (361, 242, (-351, 3231)),
            (360, 243, (-319, 1113)),
            (361, 243, (290, 3311)),
        ] {
            assert_eq!(field.low_block(col, row - first, 0, &list), expected);
        }
        let (du, dv, _) = field
            .interpolate(23080, 15538, 0, |f, c, r, m| {
                let (u, v) = f.low_block(c, r, m, &list);
                (u, v, 0)
            })
            .unwrap();
        assert_eq!((du, dv), (-447, 1310));
        assert_eq!(
            field.at(&list, 23080, 15538, 769, 1, 7),
            Some((-308, 1465, 98679))
        );
        assert_eq!(shape(0, 800_000, 1), 1);
    }

    #[test]
    fn trial_a_first_steps_and_birr_at_hour_eighteen() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let backdrop = ck_map::load_backdrop(&root.join("map/backdrop")).unwrap();
        let rules =
            WeatherRules::parse(&std::fs::read_to_string(root.join("game/weather.toml")).unwrap())
                .unwrap();
        let field = WindField::new(&backdrop, &rules).unwrap();
        let mut list = super::super::lows::Lows::default();
        field.place(&mut list, 21361, 15777, 500_000, 3000).unwrap();
        for (u, v, x, y) in [
            (728, 309, 21_412_238, 15_755_964),
            (738, 316, 21_463_788, 15_733_891),
            (755, 328, 21_516_646, 15_710_927),
        ] {
            let low = &list.entries[0];
            assert_eq!(
                field.steering(
                    u32::try_from(low.x / 1000).unwrap(),
                    u32::try_from(low.y / 1000).unwrap(),
                    769,
                    1,
                    7
                ),
                Some((u, v))
            );
            field.step(&mut list, 769, 1, 7);
            assert_eq!((list.entries[0].x, list.entries[0].y), (x, y));
        }
        for _ in 3..18 {
            field.step(&mut list, 769, 1, 7);
        }
        let low = &list.entries[0];
        for (col, row, expected) in [
            (358, 242, 754),
            (354, 242, 1807),
            (356, 240, 1477),
            (356, 244, 989),
        ] {
            assert_eq!(
                field.lowering(low, (col * 64 + 32) * 1000, (row * 64 + 32) * 1000),
                expected
            );
        }
        let first = i64::from(backdrop.first_row());
        for (col, row, expected) in [
            (356, 242, (-189, 1774)),
            (357, 242, (-244, 1722)),
            (356, 243, (-23, 1772)),
            (357, 243, (-93, 1699)),
        ] {
            assert_eq!(field.low_block(col, row - first, 0, &list), expected);
        }
        let (u, v, _) = field
            .interpolate(22833, 15538, 0, |f, c, r, m| {
                let (u, v) = f.low_block(c, r, m, &list);
                (u, v, 0)
            })
            .unwrap();
        assert_eq!((u, v), (-157, 1758));
        assert_eq!(field.lowering(low, 22_833_500, 15_538_500), 1176);
        assert_eq!(
            field.at(&list, 22833, 15538, 769, 1, 7),
            Some((-66, 1948, 100_033))
        );
    }

    #[test]
    fn empty_list_matches_steady_values_for_all_three_places_and_table_days() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let backdrop = ck_map::load_backdrop(&root.join("map/backdrop")).unwrap();
        let rules =
            WeatherRules::parse(&std::fs::read_to_string(root.join("game/weather.toml")).unwrap())
                .unwrap();
        let field = WindField::new(&backdrop, &rules).unwrap();
        let empty = super::super::lows::Lows::default();
        // The designer's independent reference gives each (u, v, pressure) triple.
        for (year, month, date, values) in [
            (
                768,
                10,
                12,
                [(37, 74, 101_273), (82, 85, 101_216), (257, 111, 101_305)],
            ),
            (
                768,
                10,
                13,
                [(36, 74, 101_263), (82, 84, 101_206), (258, 109, 101_295)],
            ),
            (
                768,
                10,
                20,
                [(41, 74, 101_235), (77, 87, 101_180), (265, 121, 101_269)],
            ),
            (
                768,
                11,
                5,
                [(67, 82, 101_245), (67, 105, 101_188), (269, 196, 101_277)],
            ),
            (
                769,
                1,
                13,
                [(84, 202, 101_182), (93, 192, 101_113), (435, 401, 101_188)],
            ),
            (
                769,
                1,
                14,
                [(83, 204, 101_178), (92, 193, 101_110), (439, 405, 101_182)],
            ),
            (
                769,
                1,
                16,
                [(81, 208, 101_169), (91, 196, 101_104), (447, 412, 101_171)],
            ),
            (
                769,
                1,
                17,
                [(81, 204, 101_178), (91, 194, 101_113), (440, 406, 101_181)],
            ),
            (
                769,
                3,
                1,
                [(70, 69, 101_402), (84, 109, 101_342), (193, 164, 101_431)],
            ),
            (
                769,
                7,
                16,
                [(92, 40, 101_546), (94, 43, 101_471), (292, 7, 101_643)],
            ),
            (
                772,
                2,
                29,
                [(71, 70, 101_403), (84, 110, 101_343), (194, 167, 101_433)],
            ),
        ] {
            for ((i, j), expected) in [(22833, 15538), (22999, 15257), (22484, 15826)]
                .into_iter()
                .zip(values)
            {
                assert_eq!(field.at(&empty, i, j, year, month, date), Some(expected));
            }
        }
    }

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
            .replace("rows = 7", "rows = 5")
            .replace(
                "[711279, 582007, 379549, 101559, -202277, -457261, -632957]",
                "[711279, 582007, 379549, 101559, -202277]",
            );
        fs::write(dir.0.join("backdrop.toml"), manifest).unwrap();
        write_image(
            &dir.0.join("land.png"),
            12,
            5,
            png::BitDepth::Eight,
            &[0; 60],
        );
        let words5 = |value: u16| {
            (0..12 * 5 * 12)
                .flat_map(|_| value.to_be_bytes())
                .collect::<Vec<_>>()
        };
        let pressure5 = (0..12)
            .flat_map(|_| {
                (0..5).flat_map(|r| {
                    (0..12).flat_map(move |c| {
                        (41000u16 + u16::try_from(80 * r + 150 * c).unwrap()).to_be_bytes()
                    })
                })
            })
            .collect::<Vec<_>>();
        write_image(
            &dir.0.join("pressure.png"),
            12,
            60,
            png::BitDepth::Sixteen,
            &pressure5,
        );
        for (name, value) in [("air_temperature.png", 1035), ("sea_temperature.png", 1000)] {
            write_image(
                &dir.0.join(name),
                12,
                60,
                png::BitDepth::Sixteen,
                &words5(value),
            );
        }
        let short = ck_map::load_backdrop(&dir.0).unwrap();
        rules.wind_belt_latitude = 300_000;
        let field = WindField::new(&short, &rules).unwrap();
        assert_eq!(field.block(5, 3, 0), field.rule(5, 3, 0, (15_000, 690)));
        let low = super::super::lows::Low {
            number: 1,
            x: 10_000_000,
            y: 23_897_112,
            radius: 500_000,
            depth: 3000,
            strongest: 0,
        };
        assert_eq!(field.point_latitude(low.y), 0);
        assert_eq!(field.strength(&low), 34_688);
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
