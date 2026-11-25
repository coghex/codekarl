//! Fixture events for the event log. These are view data, never simulation state.

use std::{collections::BTreeSet, path::Path};

use ck_sim::{Date, GameTime};
use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EventKind {
    Storm {
        number: u64,
        snow: bool,
        cell: [u32; 2],
        knots: u32,
    },
    Blight {
        good: String,
        place: String,
        place_order: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Action {
    Begins,
    Ends,
    Merged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Event {
    pub(crate) at: GameTime,
    pub(crate) action: Action,
    pub(crate) kind: EventKind,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    storm: Vec<toml::Value>,
    #[serde(default)]
    blight: Vec<toml::Value>,
}
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Point {
    at: [u16; 4],
    cell: [u32; 2],
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Storm {
    number: u64,
    #[serde(default)]
    snow: bool,
    begins: Point,
    strength: Vec<([u16; 4], u32)>,
    ends: Option<Point>,
    merged: Option<Point>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Blight {
    good: String,
    place: String,
    begins: [u16; 4],
    ends: Option<[u16; 4]>,
}

#[derive(Clone, Debug)]
struct StormData {
    number: u64,
    snow: bool,
    begins: (GameTime, [u32; 2]),
    strength: Vec<(GameTime, u32)>,
    terminal: Option<(GameTime, [u32; 2], Action)>,
}
#[derive(Clone, Debug)]
struct BlightData {
    good: String,
    place: String,
    place_order: usize,
    begins: GameTime,
    ends: Option<GameTime>,
}
#[derive(Clone, Debug, Default)]
pub(crate) struct Fixture {
    storms: Vec<StormData>,
    blights: Vec<BlightData>,
}

pub(crate) fn good_name(id: &str) -> String {
    if id == "peas_beans" {
        "peas and beans".into()
    } else {
        id.replace('_', " ")
    }
}

fn date(value: [u16; 4]) -> Result<GameTime, String> {
    let [year, month, day, hour] = value;
    let field = |n| u8::try_from(n).map_err(|_| format!("invalid date {value:?}"));
    GameTime::from_date(Date {
        year,
        month: field(month)?,
        day: field(day)?,
        hour: field(hour)?,
    })
    .map_err(|error| format!("invalid date {value:?}: {error}"))
}
fn point(value: Point) -> Result<(GameTime, [u32; 2]), String> {
    if value.cell.iter().any(|&n| n > 47_765) {
        return Err(format!("cell {:?} is outside the world", value.cell));
    }
    Ok((date(value.at)?, value.cell))
}

impl Fixture {
    pub(crate) fn load(path: &Path, goods: &[String], places: &[String]) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&text, goods, places).map_err(|e| format!("{}: {e}", path.display()))
    }
    fn parse(text: &str, goods: &[String], places: &[String]) -> Result<Self, String> {
        let file: File = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut result = Self::default();
        let mut numbers = BTreeSet::new();
        for (index, raw) in file.storm.into_iter().enumerate() {
            let storm: Storm = raw
                .try_into()
                .map_err(|e| format!("storm {}: {e}", index + 1))?;
            let convert = || -> Result<StormData, String> {
                if !numbers.insert(storm.number) {
                    return Err(format!("duplicate number {}", storm.number));
                }
                let begins = point(storm.begins)?;
                let terminal = match (storm.ends, storm.merged) {
                    (Some(_), Some(_)) => return Err("both ends and merged are present".into()),
                    (Some(p), None) => {
                        let (t, c) = point(p)?;
                        Some((t, c, Action::Ends))
                    }
                    (None, Some(p)) => {
                        let (t, c) = point(p)?;
                        Some((t, c, Action::Merged))
                    }
                    (None, None) => None,
                };
                if terminal.is_some_and(|(t, _, _)| t < begins.0) {
                    return Err("terminal before beginning".into());
                }
                let strength = storm
                    .strength
                    .into_iter()
                    .map(|(at, n)| Ok((date(at)?, n)))
                    .collect::<Result<Vec<_>, String>>()?;
                if !strength.iter().any(|(at, _)| *at == begins.0) {
                    return Err("no strength at beginning".into());
                }
                if strength
                    .iter()
                    .any(|(at, _)| *at < begins.0 || terminal.is_some_and(|(end, _, _)| *at > end))
                {
                    return Err("strength outside storm lifetime".into());
                }
                Ok(StormData {
                    number: storm.number,
                    snow: storm.snow,
                    begins,
                    strength,
                    terminal,
                })
            };
            result
                .storms
                .push(convert().map_err(|e| format!("storm {}: {e}", index + 1))?);
        }
        for (index, raw) in file.blight.into_iter().enumerate() {
            let blight: Blight = raw
                .try_into()
                .map_err(|e| format!("blight {}: {e}", index + 1))?;
            let convert = || -> Result<BlightData, String> {
                if !goods.contains(&blight.good) {
                    return Err(format!("unknown good {}", blight.good));
                }
                let place_order = places
                    .iter()
                    .position(|p| p == &blight.place)
                    .ok_or_else(|| format!("unknown place {}", blight.place))?;
                let begins = date(blight.begins)?;
                let ends = blight.ends.map(date).transpose()?;
                if ends.is_some_and(|end| end < begins) {
                    return Err("end before beginning".into());
                }
                Ok(BlightData {
                    good: blight.good,
                    place: blight.place,
                    place_order,
                    begins,
                    ends,
                })
            };
            result
                .blights
                .push(convert().map_err(|e| format!("blight {}: {e}", index + 1))?);
        }
        Ok(result)
    }
    pub(crate) fn at(&self, now: GameTime) -> Vec<Event> {
        let mut rows = Vec::new();
        for s in &self.storms {
            if s.begins.0 > now {
                continue;
            }
            let knots = s
                .strength
                .iter()
                .filter(|(at, _)| *at <= now)
                .map(|(_, n)| *n)
                .max()
                .unwrap_or_default();
            let make = |at, cell, action| Event {
                at,
                action,
                kind: EventKind::Storm {
                    number: s.number,
                    snow: s.snow,
                    cell,
                    knots,
                },
            };
            rows.push(make(s.begins.0, s.begins.1, Action::Begins));
            if let Some((at, cell, action)) = s.terminal
                && at <= now
            {
                rows.push(make(at, cell, action));
            }
        }
        for b in &self.blights {
            let make = |at, action| Event {
                at,
                action,
                kind: EventKind::Blight {
                    good: b.good.clone(),
                    place: b.place.clone(),
                    place_order: b.place_order,
                },
            };
            if b.begins <= now {
                rows.push(make(b.begins, Action::Begins));
            }
            if let Some(at) = b.ends
                && at <= now
            {
                rows.push(make(at, Action::Ends));
            }
        }
        rows.sort_by(|a, b| {
            a.at.cmp(&b.at)
                .then_with(|| match (&a.kind, &b.kind) {
                    (EventKind::Storm { number: x, .. }, EventKind::Storm { number: y, .. }) => {
                        x.cmp(y)
                    }
                    (EventKind::Storm { .. }, EventKind::Blight { .. }) => std::cmp::Ordering::Less,
                    (EventKind::Blight { .. }, EventKind::Storm { .. }) => {
                        std::cmp::Ordering::Greater
                    }
                    (
                        EventKind::Blight {
                            place_order: x,
                            good: gx,
                            ..
                        },
                        EventKind::Blight {
                            place_order: y,
                            good: gy,
                            ..
                        },
                    ) => x.cmp(y).then_with(|| good_name(gx).cmp(&good_name(gy))),
                })
                .then_with(|| a.action.cmp(&b.action))
        });
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(text: &str) -> Result<Fixture, String> {
        Fixture::parse(text, &["oats".into()], &["clonmacnoise".into()])
    }
    #[test]
    fn shipped_fixture_rows_track_peak_and_lifetimes() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/event-log-fixture.toml");
        let f = Fixture::load(&path, &["oats".into()], &["clonmacnoise".into()]).unwrap();
        let rows = |d| f.at(date(d).unwrap());
        assert_eq!(rows([768, 10, 10, 12]).len(), 2);
        let early = rows([768, 10, 10, 18]);
        assert!(matches!(early[0].kind, EventKind::Storm { knots: 44, .. }));
        let before = rows([768, 10, 12, 8]);
        let after = rows([768, 10, 12, 9]);
        assert!(before.iter().any(|r| matches!(
            r.kind,
            EventKind::Storm {
                number: 5,
                knots: 33,
                ..
            }
        )));
        assert!(after.iter().any(|r| matches!(
            r.kind,
            EventKind::Storm {
                number: 5,
                knots: 34,
                ..
            }
        )));
        assert_eq!(rows([768, 10, 13, 0]).len(), 10);
        assert!(
            after
                .iter()
                .filter(|r| matches!(r.kind, EventKind::Storm { number: 3, .. }))
                .all(|r| matches!(r.kind, EventKind::Storm { knots: 50, .. }))
        );
        assert_eq!(rows([768, 10, 11, 17]), rows([768, 10, 11, 18]));
    }
    #[test]
    fn rejects_bad_fixture_entries() {
        let base = "[[storm]]\nnumber=1\nbegins={at=[768,10,10,6],cell=[21361,15777]}\nstrength=[[[768,10,10,6],30]]\nmerged={at=[768,10,11,6],cell=[1,1]}\n";
        assert!(
            fixture(&base.replace("[768,10,10,6],30", "[768,10,10,5],30"))
                .unwrap_err()
                .contains("storm 1")
        );
        assert!(
            fixture(&base.replace(
                "[[[768,10,10,6],30]]",
                "[[[768,10,10,6],30],[[768,10,11,7],40]]"
            ))
            .unwrap_err()
            .contains("lifetime")
        );
        assert!(
            fixture(&base.replace("21361", "47766"))
                .unwrap_err()
                .contains("cell")
        );
        assert!(
            fixture(&base.replace("768,10,10,6", "768,2,30,6"))
                .unwrap_err()
                .contains("date")
        );
        assert!(
            fixture(&format!(
                "{base}\n[[blight]]\ngood='unknown'\nplace='clonmacnoise'\nbegins=[768,10,10,6]\n"
            ))
            .unwrap_err()
            .contains("unknown good")
        );
        assert!(
            fixture(&format!("{base}\n{base}"))
                .unwrap_err()
                .contains("duplicate number")
        );
    }
    #[test]
    fn equal_hour_rows_are_ordered_and_validation_names_the_entry() {
        let text = "[[storm]]\nnumber=7\nbegins={at=[768,10,10,6],cell=[1,1]}\nstrength=[[[768,10,10,6],30]]\nends={at=[768,10,10,6],cell=[1,1]}\n[[blight]]\ngood='oats'\nplace='clonmacnoise'\nbegins=[768,10,10,6]\n";
        let rows = fixture(text).unwrap().at(date([768, 10, 10, 6]).unwrap());
        assert_eq!(
            rows.iter().map(|r| r.action).collect::<Vec<_>>(),
            [Action::Begins, Action::Ends, Action::Begins]
        );
        assert!(matches!(rows[2].kind, EventKind::Blight { .. }));
        assert!(
            fixture(&text.replace("number=7", "number=7\nunexpected=1"))
                .unwrap_err()
                .contains("storm 1")
        );
        assert!(
            fixture(&text.replace("place='clonmacnoise'", "place='missing'"))
                .unwrap_err()
                .contains("blight 1")
        );
        assert!(
            fixture(&text.replace("768,10,10,6", "768,10,10,24"))
                .unwrap_err()
                .contains("date")
        );
        assert!(fixture(&text.replace("cell=[1,1]", "cell=[0,0]")).is_ok());
    }
}
