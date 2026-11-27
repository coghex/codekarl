//! Immutable hourly expedition positions and routes for the map layer (CK-435).

use ck_economy::{EconomyState, Expedition, Hours, Leg, Phase, Rules};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExpeditionMap {
    pub(crate) number: u64,
    pub(crate) icon: String,
    pub(crate) position: [i64; 2],
    pub(crate) past: Vec<Vec<[i64; 2]>>,
    pub(crate) ahead: Vec<Vec<[i64; 2]>>,
    pub(crate) panel: Option<super::expedition_panel::PanelView>,
}

fn point((x, y): (Hours, Hours)) -> Option<[i64; 2]> {
    Some([
        i64::try_from(x.floor().to_integer()).ok()?,
        i64::try_from(y.floor().to_integer()).ok()?,
    ])
}

fn whole_paths(legs: &[Leg], target: &mut Vec<Vec<[i64; 2]>>) {
    target.extend(
        legs.iter()
            .filter(|leg| !leg.path.is_empty())
            .map(|leg| leg.path.clone()),
    );
}

fn one(trip: &Expedition, rules: &Rules) -> Option<ExpeditionMap> {
    let position = trip.position().and_then(point).or(trip.plan.arrival)?;
    let icon = trip
        .map_vehicle(rules)
        .and_then(|kind| rules.vehicles.get(kind as usize))
        .map_or("on_foot", |kind| kind.id.as_str())
        .to_owned();
    let mut past = Vec::new();
    let mut ahead = Vec::new();
    let legs = match trip.phase {
        Phase::Outbound => &trip.plan.out,
        Phase::Waiting => {
            whole_paths(&trip.plan.out, &mut past);
            whole_paths(&trip.plan.back, &mut ahead);
            return Some(ExpeditionMap {
                number: trip.number,
                icon,
                position,
                past,
                ahead,
                panel: None,
            });
        }
        Phase::Homeward => {
            whole_paths(&trip.plan.out, &mut past);
            &trip.plan.back
        }
    };
    let index = usize::try_from(trip.leg).ok()?.min(legs.len());
    whole_paths(&legs[..index], &mut past);
    if let Some(leg) = legs.get(index) {
        if let Some((segment, _)) = leg.path_segment(&trip.done) {
            let mut before = leg.path[..=segment].to_vec();
            before.push(position);
            let mut after = vec![position];
            after.extend_from_slice(&leg.path[segment + 1..]);
            past.push(before);
            ahead.push(after);
        } else if !leg.path.is_empty() {
            past.push(leg.path.clone());
        }
        whole_paths(&legs[index + 1..], &mut ahead);
    }
    if trip.phase == Phase::Outbound {
        whole_paths(&trip.plan.back, &mut ahead);
    }
    Some(ExpeditionMap {
        number: trip.number,
        icon,
        position,
        past,
        ahead,
        panel: None,
    })
}

pub(crate) fn build(state: &EconomyState, rules: &Rules) -> Vec<ExpeditionMap> {
    let mut trips: Vec<_> = state
        .places
        .iter()
        .enumerate()
        .flat_map(|(home, place)| {
            place.trade.expeditions.iter().filter_map(move |trip| {
                let mut view = one(trip, rules)?;
                view.panel = super::expedition_panel::build(state, home, trip, rules);
                Some(view)
            })
        })
        .collect();
    trips.sort_by_key(|trip| trip.number);
    trips
}

#[cfg(test)]
mod tests {
    use super::*;
    use ck_economy::{Delays, ExpeditionVehicle, Group, Limiter, Mode, Plan, Speed, Term};
    use num_rational::BigRational;

    fn ratio(n: i64, d: i64) -> Hours {
        BigRational::new(n.into(), d.into())
    }

    #[test]
    fn map_readback_splits_the_bent_route_at_the_current_hour() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml");
        let rules = Rules::load(path).unwrap();
        let n = rules.goods.len();
        let leg = Leg {
            mode: Mode::Land,
            terms: vec![Term {
                work: ratio(400, 1),
                speed: Speed::Plain(100),
            }],
            fixed: ratio(0, 1),
            delays: Delays::Along,
            key: [0; 4],
            path: vec![[0, 0], [100, 0], [100, 300]],
            along: vec![0, 100, 400],
        };
        let trip = Expedition {
            number: 2,
            trader: 0,
            plan: Plan {
                destination: 1,
                out: vec![leg.clone()],
                back: vec![],
                vehicles: vec![],
                people: vec![(0, 1)],
                cargo: vec![0; n],
                wanted: vec![0; n],
                supplies: vec![0; n],
                arrival: Some([100, 300]),
            },
            groups: vec![Group {
                pop: 0,
                count: 1,
                by_building: vec![],
                paid: vec![],
            }],
            vehicles: Vec::<ExpeditionVehicle>::new(),
            cargo: vec![0; n],
            supplies: vec![0; n],
            pantry_centi: 0,
            gone: false,
            fed: None,
            phase: Phase::Outbound,
            traded: false,
            leg: 0,
            done: ratio(1, 2),
            pace_pm: 1000,
            limiter: Limiter::Walkers,
            reckoned: vec![0; n],
            reckoned_mode: Mode::Land,
            left: 0,
            arrived: None,
            daily: false,
            loaded: vec![0; n],
        };
        let view = one(&trip, &rules).unwrap();
        assert_eq!(view.position, [100, 100]);
        assert_eq!(view.icon, "on_foot");
        assert_eq!(view.past, vec![vec![[0, 0], [100, 0], [100, 100]]]);
        assert_eq!(view.ahead, vec![vec![[100, 100], [100, 300]]]);
        let mut waiting = trip.clone();
        waiting.phase = Phase::Waiting;
        assert_eq!(one(&waiting, &rules).unwrap().position, [100, 300]);
        assert_eq!(waiting.remaining_hours(&rules), Some(ratio(0, 1)));
        let mut empty = trip.clone();
        empty.plan.out.clear();
        assert_eq!(empty.remaining_hours(&rules), Some(ratio(1, 1)));
    }
}
