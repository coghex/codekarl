//! The hourly expedition map readback projected through the current camera.

use crate::game::slot::SimSlot;
use crate::map_app::{self, SharedController};
use ck_lua::{ConfigValue, HostFunction};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Move to a live trip only in the named world, or read the manual-input generation.
pub(crate) fn camera_function(slot: Arc<SimSlot>, map: Option<SharedController>) -> HostFunction {
    Arc::new(move |args| {
        let map = map.as_ref().ok_or("game.expedition_camera: no map")?;
        let movement = match args {
            [] | [Some(ConfigValue::Bool(false))] => None,
            [
                Some(ConfigValue::String(id)),
                Some(ConfigValue::String(world)),
                Some(ConfigValue::Bool(zoom)),
                expected,
            ] => {
                let snapshot = slot.latest().ok_or("game.expedition_camera: no world")?;
                if snapshot.world.to_string() != *world || !snapshot.layers.expeditions {
                    return Err("game.expedition_camera: world or layer changed".into());
                }
                let trip = snapshot
                    .expeditions
                    .iter()
                    .find(|trip| trip.number.to_string() == *id)
                    .ok_or("game.expedition_camera: expedition ended")?;
                let point = ck_map::FixedPoint {
                    x: i32::try_from(trip.position[0]).map_err(|_| "bad x")?,
                    y: i32::try_from(trip.position[1]).map_err(|_| "bad y")?,
                };
                let expected = match expected {
                    Some(ConfigValue::Integer(value)) => {
                        Some(u64::try_from(*value).map_err(|_| "bad generation")?)
                    }
                    Some(ConfigValue::Bool(false)) => None,
                    _ => {
                        return Err(
                            "game.expedition_camera: expected a manual generation or false".into(),
                        );
                    }
                };
                Some((point, *zoom, expected))
            }
            _ => {
                return Err(
                    "game.expedition_camera: expected id, world, zoom and generation or false"
                        .into(),
                );
            }
        };
        let mut camera = map_app::lock(map);
        if let Some((point, zoom, expected)) = movement {
            if expected.is_some_and(|generation| generation != camera.manual_generation()) {
                return Err("game.expedition_camera: manual input stopped follow".into());
            }
            camera.go_to(point, zoom)?;
        }
        Ok(vec![integer(u128::from(camera.manual_generation()))])
    })
}

fn integer(value: u128) -> ConfigValue {
    ConfigValue::Integer(i64::try_from(value).unwrap_or(i64::MAX))
}
fn string(value: &str) -> ConfigValue {
    ConfigValue::String(value.to_owned())
}
fn table(entries: impl IntoIterator<Item = (&'static str, ConfigValue)>) -> ConfigValue {
    ConfigValue::Table(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}
fn vehicle_value(vehicle: &crate::game::expedition_panel::VehicleView) -> ConfigValue {
    table([
        ("number", string(&vehicle.number.to_string())),
        ("kind", string(&vehicle.kind)),
        ("free", ConfigValue::Bool(vehicle.free)),
        (
            "expedition",
            vehicle
                .expedition
                .map_or(ConfigValue::Bool(false), |n| string(&n.to_string())),
        ),
        ("owner", string(&vehicle.owner)),
        (
            "materials",
            vehicle
                .materials
                .as_ref()
                .map_or(ConfigValue::Bool(false), |items| {
                    ConfigValue::Array(
                        items
                            .iter()
                            .map(|(id, n)| {
                                table([("id", string(id)), ("amount", integer(u128::from(*n)))])
                            })
                            .collect(),
                    )
                }),
        ),
        (
            "carried_g",
            vehicle
                .carried_g
                .map_or(ConfigValue::Bool(false), |n| integer(u128::from(n))),
        ),
        (
            "carried_ml",
            vehicle
                .carried_ml
                .map_or(ConfigValue::Bool(false), |n| integer(u128::from(n))),
        ),
        ("crew_fewest", integer(u128::from(vehicle.crew_fewest))),
        (
            "pace_pm",
            vehicle
                .pace_pm
                .map_or(ConfigValue::Bool(false), |n| integer(u128::from(n))),
        ),
        ("build_hours", integer(u128::from(vehicle.build_hours))),
        ("build_hands", integer(u128::from(vehicle.build_hands))),
        (
            "land_pace_pm",
            vehicle
                .land_pace_pm
                .map_or(ConfigValue::Bool(false), |n| integer(u128::from(n))),
        ),
        (
            "water_profile",
            vehicle
                .water
                .as_ref()
                .map_or(ConfigValue::Bool(false), |s| string(s)),
        ),
        ("wear", string(&vehicle.wear)),
        ("crew", integer(u128::from(vehicle.crew))),
        ("full_crew", integer(u128::from(vehicle.full_crew))),
        ("load_g", integer(vehicle.load_g)),
        ("load_ml", integer(vehicle.load_ml)),
        ("capacity_g", integer(u128::from(vehicle.capacity_g))),
        ("capacity_ml", integer(u128::from(vehicle.capacity_ml))),
        (
            "carrier",
            vehicle
                .carrier
                .map_or(ConfigValue::Bool(false), |n| string(&n.to_string())),
        ),
    ])
}

pub(crate) fn pop_trade_function(slot: Arc<SimSlot>) -> HostFunction {
    Arc::new(move |args| {
        let [
            Some(ConfigValue::String(place)),
            Some(ConfigValue::Integer(pop)),
            Some(ConfigValue::String(world)),
        ] = args
        else {
            return Err("game.pop_trade: expected place, pop and world".into());
        };
        let snapshot = slot.latest().ok_or("game.pop_trade: no world")?;
        if snapshot.world.to_string() != *world {
            return Err("game.pop_trade: world changed".into());
        }
        let row = snapshot
            .pop_trade
            .iter()
            .find(|row| row.place == *place && i64::from(row.pop) == *pop)
            .ok_or("game.pop_trade: no pop")?;
        let jobs = row
            .jobs
            .iter()
            .map(|job| {
                table([
                    ("label", string(&job.label)),
                    ("state", string(job.state)),
                    (
                        "hours_left",
                        job.done.map_or(ConfigValue::Bool(false), |n| {
                            integer(u128::from(n.saturating_sub(snapshot.tick.hours())))
                        }),
                    ),
                    (
                        "lacks",
                        ConfigValue::Array(
                            job.lacks
                                .iter()
                                .map(|(id, n)| {
                                    table([("id", string(id)), ("amount", integer(u128::from(*n)))])
                                })
                                .collect(),
                        ),
                    ),
                ])
            })
            .collect();
        Ok(vec![table([
            ("tick", integer(u128::from(snapshot.tick.hours()))),
            (
                "vehicles",
                ConfigValue::Array(row.vehicles.iter().map(vehicle_value).collect()),
            ),
            ("jobs", ConfigValue::Array(jobs)),
        ])])
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "The table binds the complete panel snapshot to one hour."
)]
fn panel(view: &crate::game::expedition_panel::PanelView, now: u64) -> ConfigValue {
    let trade_hour = |arrival: u64| {
        let day = arrival - arrival % 24;
        day.saturating_add(if arrival % 24 < 12 { 12 } else { 36 })
    };
    let arrival = view
        .arrived
        .unwrap_or_else(|| now.saturating_add(view.remaining));
    let trade = trade_hour(arrival);
    let start = if view.phase == "homeward" {
        trade
    } else {
        view.left
    };
    let elapsed = (if view.phase == "waiting" {
        arrival
    } else {
        now
    })
    .saturating_sub(start);
    let date = |hour: u64| {
        ck_sim::GameTime::from_hours(hour).date().ok().map_or_else(
            || "—".to_owned(),
            |d| format!("{:04}-{:02}-{:02} {:02}:00", d.year, d.month, d.day, d.hour),
        )
    };
    let goods = |items: &[crate::game::expedition_panel::GoodView]| {
        ConfigValue::Array(
            items
                .iter()
                .map(|good| {
                    table([
                        ("id", string(&good.id)),
                        ("amount", integer(u128::from(good.amount))),
                        ("weight_g", integer(good.weight_g)),
                        ("animal", ConfigValue::Bool(good.animal)),
                    ])
                })
                .collect(),
        )
    };
    let people = ConfigValue::Array(
        view.people
            .iter()
            .map(|person| {
                table([
                    ("pop", integer(u128::from(person.pop))),
                    ("kind", string(&person.kind)),
                    ("count", integer(u128::from(person.count))),
                    ("own", ConfigValue::Bool(person.own)),
                    (
                        "paid",
                        ConfigValue::Array(
                            person
                                .paid
                                .iter()
                                .map(|(good, amount)| {
                                    table([
                                        ("id", string(good)),
                                        ("amount", integer(u128::from(*amount))),
                                    ])
                                })
                                .collect(),
                        ),
                    ),
                    (
                        "roles",
                        ConfigValue::Array(
                            person
                                .roles
                                .iter()
                                .map(|(role, amount)| {
                                    table([
                                        ("role", string(role)),
                                        ("amount", integer(u128::from(*amount))),
                                    ])
                                })
                                .collect(),
                        ),
                    ),
                ])
            })
            .collect(),
    );
    let vehicles = ConfigValue::Array(view.vehicles.iter().map(vehicle_value).collect());
    table([
        ("home", string(&view.home)),
        ("destination", string(&view.destination)),
        ("trader", string(&view.trader)),
        ("trader_slot", integer(u128::from(view.trader_slot))),
        ("phase", string(view.phase)),
        ("pace_pm", integer(u128::from(view.pace_pm))),
        ("limiter", string(&view.limiter)),
        ("water", ConfigValue::Bool(view.water)),
        ("people", people),
        ("drovers", integer(u128::from(view.drovers))),
        ("walkers", integer(u128::from(view.walkers))),
        ("vehicles", vehicles),
        ("cargo", goods(&view.cargo)),
        ("supplies", goods(&view.supplies)),
        ("wanted", goods(&view.wanted)),
        ("load_g", integer(view.load_g)),
        ("load_ml", integer(view.load_ml)),
        ("capacity_g", integer(view.capacity_g)),
        ("capacity_ml", integer(view.capacity_ml)),
        ("left", integer(u128::from(view.left))),
        ("remaining", integer(u128::from(view.remaining))),
        ("food_days", integer(u128::from(view.food_days))),
        ("travelled", integer(u128::from(elapsed))),
        (
            "total",
            integer(u128::from(elapsed.saturating_add(view.remaining))),
        ),
        (
            "arrives_text",
            string(&date(if view.phase == "homeward" {
                now.saturating_add(view.remaining)
            } else {
                arrival
            })),
        ),
        ("trades_text", string(&date(trade))),
        ("now", integer(u128::from(now))),
        (
            "arrived",
            view.arrived
                .map_or(ConfigValue::Bool(false), |n| integer(u128::from(n))),
        ),
    ])
}

pub(crate) fn function(slot: Arc<SimSlot>, map: Option<SharedController>) -> HostFunction {
    Arc::new(move |args| {
        let selected = match args {
            [Some(ConfigValue::String(id))] => Some(id.as_str()),
            [Some(ConfigValue::Bool(false))] | [] => None,
            _ => return Err("game.expeditions: expected a selected id or false".into()),
        };
        let Some(snapshot) = slot.latest() else {
            return Ok(Vec::new());
        };
        let Some(map) = &map else {
            return Ok(Vec::new());
        };
        let map = map_app::lock(map);
        let point = |p| {
            map.project_ui(p).map(|[x, y]| {
                ConfigValue::Array(vec![ConfigValue::Float(x), ConfigValue::Float(y)])
            })
        };
        let paths = |items: &[Vec<[i64; 2]>]| {
            ConfigValue::Array(
                items
                    .iter()
                    .map(|path| ConfigValue::Array(path.iter().filter_map(|p| point(*p)).collect()))
                    .collect(),
            )
        };
        let trips = if snapshot.layers.expeditions {
            snapshot
                .expeditions
                .iter()
                .filter_map(|trip| {
                    let id = trip.number.to_string();
                    let chosen = selected == Some(id.as_str());
                    Some(ConfigValue::Table(BTreeMap::from([
                        ("id".into(), ConfigValue::String(id)),
                        ("icon".into(), ConfigValue::String(trip.icon.clone())),
                        (
                            "panel".into(),
                            if chosen {
                                trip.panel
                                    .as_ref()
                                    .map_or(ConfigValue::Bool(false), |view| {
                                        panel(view, snapshot.tick.hours())
                                    })
                            } else {
                                ConfigValue::Bool(false)
                            },
                        ),
                        ("position".into(), point(trip.position)?),
                        (
                            "past".into(),
                            if chosen {
                                paths(&trip.past)
                            } else {
                                ConfigValue::Array(Vec::new())
                            },
                        ),
                        (
                            "ahead".into(),
                            if chosen {
                                paths(&trip.ahead)
                            } else {
                                ConfigValue::Array(Vec::new())
                            },
                        ),
                    ])))
                })
                .collect()
        } else {
            Vec::new()
        };
        Ok(vec![ConfigValue::Table(BTreeMap::from([
            (
                "world".into(),
                ConfigValue::String(snapshot.world.to_string()),
            ),
            (
                "shown".into(),
                ConfigValue::Bool(snapshot.layers.expeditions),
            ),
            ("trips".into(), ConfigValue::Array(trips)),
            ("tick".into(), integer(u128::from(snapshot.tick.hours()))),
        ]))])
    })
}
