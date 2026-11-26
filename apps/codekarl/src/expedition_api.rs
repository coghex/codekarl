//! The hourly expedition map readback projected through the current camera.

use crate::game::slot::SimSlot;
use crate::map_app::{self, SharedController};
use ck_lua::{ConfigValue, HostFunction};
use std::collections::BTreeMap;
use std::sync::Arc;

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
        ]))])
    })
}
