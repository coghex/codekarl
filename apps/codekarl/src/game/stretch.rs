//! Canonical state of the physical stretches in the scenario's land network.

use std::collections::BTreeMap;

use ck_sim::{Canonical, CanonicalError, Reader};

/// The saved category tag: 0 is a path, 1 is a road. Other tags are retained for load validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StretchState {
    pub(super) category: u8,
    pub(super) level: u8,
    pub(super) condition: u16,
}

impl Canonical for StretchState {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.category.encode(out)?;
        self.level.encode(out)?;
        self.condition.encode(out)
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        Ok(Self {
            category: u8::decode(input)?,
            level: u8::decode(input)?,
            condition: u16::decode(input)?,
        })
    }
}

pub(super) fn starting(network: Option<&ck_map::LandNetwork>) -> BTreeMap<u64, StretchState> {
    network
        .into_iter()
        .flat_map(|n| &n.stretches)
        .map(|s| {
            let category = match s.category {
                ck_map::LandCategory::Path => 0,
                ck_map::LandCategory::Road => 1,
            };
            (
                s.id,
                StretchState {
                    category,
                    level: s.level,
                    condition: 1000,
                },
            )
        })
        .collect()
}

/// Validate the saved values and the exact set of ids before installing a loaded game.
pub(super) fn validate(
    states: &BTreeMap<u64, StretchState>,
    expected: &std::collections::BTreeSet<u64>,
) -> Result<(), String> {
    if let Some(id) = expected.iter().find(|id| !states.contains_key(id)) {
        return Err(format!("missing stretch {id}"));
    }
    if let Some(id) = states.keys().find(|id| !expected.contains(id)) {
        return Err(format!("unexpected stretch {id}"));
    }
    for (id, state) in states {
        if state.category > 1 {
            return Err(format!("stretch {id}: invalid category {}", state.category));
        }
        if state.level > 5 {
            return Err(format!("stretch {id}: invalid level {}", state.level));
        }
        if state.condition > 1000 {
            return Err(format!(
                "stretch {id}: invalid condition {}",
                state.condition
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn fixture_stretches_start_in_id_order_with_authored_values() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/map/tests/network/scenarios/fixture");
        let scenario = ck_map::load_scenario(&path).unwrap();
        let network = scenario.network.as_ref().unwrap();
        let states = starting(Some(network));
        assert_eq!(states.len(), network.stretches.len());
        assert!(network.stretches.iter().any(|s| s.tidal));
        for s in &network.stretches {
            let state = &states[&s.id];
            assert_eq!(
                state.category,
                match s.category {
                    ck_map::LandCategory::Path => 0,
                    ck_map::LandCategory::Road => 1,
                }
            );
            assert_eq!(state.level, s.level);
            assert_eq!(state.condition, 1000);
        }
        assert!(states.keys().zip(states.keys().skip(1)).all(|(a, b)| a < b));
        let bytes = states.to_bytes().unwrap();
        assert_eq!(
            BTreeMap::<u64, StretchState>::from_bytes(&bytes).unwrap(),
            states
        );
    }

    #[test]
    fn validation_reports_missing_before_extra_and_rejects_bad_values() {
        let mut states = BTreeMap::from([
            (
                2,
                StretchState {
                    category: 1,
                    level: 1,
                    condition: 0,
                },
            ),
            (
                9,
                StretchState {
                    category: 0,
                    level: 0,
                    condition: 1000,
                },
            ),
        ]);
        let expected = [1, 2, 3].into_iter().collect();
        assert_eq!(
            validate(&states, &expected).unwrap_err(),
            "missing stretch 1"
        );
        let expected = [2].into_iter().collect();
        assert_eq!(
            validate(&states, &expected).unwrap_err(),
            "unexpected stretch 9"
        );
        let expected = [2, 9].into_iter().collect();
        assert!(validate(&states, &expected).is_ok());
        states.get_mut(&2).unwrap().category = 2;
        assert!(
            validate(&states, &expected)
                .unwrap_err()
                .contains("category 2")
        );
        states.get_mut(&2).unwrap().category = 1;
        states.get_mut(&2).unwrap().level = 6;
        assert!(
            validate(&states, &expected)
                .unwrap_err()
                .contains("level 6")
        );
        states.get_mut(&2).unwrap().level = 1;
        states.get_mut(&2).unwrap().condition = 1001;
        assert!(
            validate(&states, &expected)
                .unwrap_err()
                .contains("condition 1001")
        );
    }

    #[test]
    fn each_stretch_field_affects_the_digest() {
        use crate::game::{GameCommand, GameState, apply};
        use ck_sim::{Date, GameTime, Scheduler, Simulation};
        let start = GameTime::from_date(Date {
            year: 768,
            month: 1,
            day: 1,
            hour: 0,
        })
        .unwrap();
        let stretches = BTreeMap::from([(
            2,
            StretchState {
                category: 1,
                level: 1,
                condition: 0,
            },
        )]);
        let make = |stretches| {
            Simulation::<GameState, GameCommand>::new(
                start,
                42,
                GameState {
                    stretches,
                    ..GameState::default()
                },
                Scheduler::new(),
                apply,
            )
        };
        let game = make(stretches.clone());
        let digest = game.digest().unwrap();
        for field in 0..3 {
            let mut changed = stretches.clone();
            let stretch = changed.get_mut(&2).unwrap();
            match field {
                0 => stretch.category = 0,
                1 => stretch.level = 2,
                _ => stretch.condition = 1,
            }
            assert_ne!(make(changed).digest().unwrap(), digest);
        }
    }
}
