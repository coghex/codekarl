//! Hourly, immutable panel data for a travelling expedition.
use ck_economy::{EconomyState, Expedition, Limiter, Mode, Phase, Rules};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GoodView {
    pub id: String,
    pub amount: u64,
    pub weight_g: u128,
    pub animal: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PersonView {
    pub pop: u32,
    pub kind: String,
    pub count: u64,
    pub own: bool,
    pub paid: Vec<(String, u64)>,
    pub roles: Vec<(String, u64)>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VehicleView {
    pub number: u64,
    pub kind: String,
    pub crew: u64,
    pub full_crew: u64,
    pub load_g: u128,
    pub load_ml: u128,
    pub capacity_g: u64,
    pub capacity_ml: u64,
    pub carrier: Option<u64>,
    pub owner: String,
    pub materials: Option<Vec<(String, u64)>>,
    pub carried_g: Option<u64>,
    pub carried_ml: Option<u64>,
    pub crew_fewest: u64,
    pub pace_pm: Option<u32>,
    pub build_hours: u32,
    pub build_hands: u64,
    pub land_pace_pm: Option<u32>,
    pub water: Option<String>,
    pub wear: String,
    pub free: bool,
    pub expedition: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PanelView {
    pub home: String,
    pub destination: String,
    pub trader: String,
    pub trader_slot: u32,
    pub phase: &'static str,
    pub pace_pm: u32,
    pub limiter: String,
    pub water: bool,
    pub people: Vec<PersonView>,
    pub drovers: u64,
    pub walkers: u64,
    pub vehicles: Vec<VehicleView>,
    pub cargo: Vec<GoodView>,
    pub supplies: Vec<GoodView>,
    pub wanted: Vec<GoodView>,
    pub load_g: u128,
    pub load_ml: u128,
    pub capacity_g: u128,
    pub capacity_ml: u128,
    pub left: u64,
    pub arrived: Option<u64>,
    pub remaining: u64,
    pub food_days: u64,
}

pub(crate) fn vehicle_base(
    place: &ck_economy::PlaceState,
    owned: &ck_economy::Vehicle,
    rules: &Rules,
) -> Option<VehicleView> {
    let kind = rules.vehicles.get(owned.kind as usize)?;
    let materials = owned.materials.as_ref().map(|m| {
        let mut items: Vec<_> = rules
            .goods
            .iter()
            .zip(&m.goods)
            .filter(|(_, n)| **n > 0)
            .map(|(good, n)| (good.id.clone(), *n))
            .collect();
        items.extend(
            rules
                .herd_species
                .iter()
                .zip(&m.animals)
                .filter(|(_, n)| **n > 0)
                .map(|(animal, n)| (animal.animal.clone(), *n)),
        );
        items
    });
    Some(VehicleView {
        number: owned.number,
        kind: kind.id.clone(),
        crew: 0,
        full_crew: kind.crew_full,
        load_g: 0,
        load_ml: 0,
        capacity_g: kind.carries_g,
        capacity_ml: kind.carries_ml,
        carrier: None,
        owner: place.pops.get(owned.pop as usize)?.kind.clone(),
        materials,
        carried_g: kind.carried_g,
        carried_ml: kind.carried_ml,
        crew_fewest: kind.crew_fewest,
        pace_pm: None,
        build_hours: kind.build_hours,
        build_hands: kind.hands,
        land_pace_pm: kind.land_pace_pm,
        water: kind.water.clone(),
        wear: format!("{} {} per 30 days", kind.wear.units, kind.wear.good),
        free: owned.free,
        expedition: None,
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "One snapshot binds the assignment and all its displayed members together."
)]
pub(crate) fn build(
    state: &EconomyState,
    home: usize,
    trip: &Expedition,
    rules: &Rules,
) -> Option<PanelView> {
    let place = state.places.get(home)?;
    let destination = state.places.get(trip.plan.destination as usize)?;
    let assignment = trip.assignment(rules)?;
    let paces = trip.vehicle_paces(rules)?;
    let count: u64 = trip.groups.iter().map(|g| g.count).sum();
    let goods = |amounts: &[u64]| {
        rules
            .goods
            .iter()
            .zip(amounts)
            .filter(|(_, amount)| **amount > 0)
            .map(|(good, amount)| GoodView {
                id: good.id.clone(),
                amount: *amount,
                weight_g: u128::from(*amount) * u128::from(good.weight_g),
                animal: rules.herd_species.iter().any(|s| s.animal == good.id),
            })
            .collect()
    };
    let mut people: Vec<PersonView> = trip
        .groups
        .iter()
        .filter_map(|g| {
            Some(PersonView {
                pop: g.pop,
                kind: place.pops.get(g.pop as usize)?.kind.clone(),
                count: g.count,
                own: g.pop == trip.trader,
                paid: g
                    .paid
                    .iter()
                    .filter_map(|(i, n)| Some((rules.goods.get(*i as usize)?.id.clone(), *n)))
                    .collect(),
                roles: Vec::new(),
            })
        })
        .collect();
    // Give the simulation's aggregate assignments to groups in their stable order.
    let mut allocations: Vec<(String, u64)> = assignment
        .moving
        .iter()
        .filter_map(|(index, count, _)| {
            Some((
                format!(
                    "crew of {}",
                    rules
                        .vehicles
                        .get(trip.vehicles.get(*index)?.kind as usize)?
                        .id
                ),
                *count,
            ))
        })
        .collect();
    allocations.push(("drover".to_owned(), assignment.drovers));
    allocations.push(("walker".to_owned(), assignment.walkers));
    let mut group = 0;
    let mut left = people.first().map_or(0, |p| p.count);
    for (role, mut number) in allocations {
        while number > 0 && group < people.len() {
            let used = number.min(left);
            if used > 0 {
                people[group].roles.push((role.clone(), used));
            }
            number -= used;
            left -= used;
            if left == 0 {
                group += 1;
                left = people.get(group).map_or(0, |p| p.count);
            }
        }
    }
    let mut vehicles = Vec::new();
    for (i, vehicle) in trip.vehicles.iter().enumerate() {
        let moving = assignment
            .moving
            .iter()
            .position(|(index, _, _)| *index == i);
        let (crew, load_g, load_ml) = moving.map_or((0, 0, 0), |slot| {
            (
                assignment.moving[slot].1,
                assignment.loads[slot].0,
                assignment.loads[slot].1,
            )
        });
        let carrier = assignment
            .carried
            .iter()
            .find(|(carried, _)| *carried == i)
            .and_then(|(_, carrier)| trip.vehicles.get(*carrier))
            .map(|v| v.number);
        let owned = place
            .trade
            .vehicles
            .iter()
            .find(|v| v.number == vehicle.number)?;
        let mut view = vehicle_base(place, owned, rules)?;
        view.crew = crew;
        view.load_g = load_g;
        view.load_ml = load_ml;
        view.carrier = carrier;
        view.pace_pm = moving.map(|slot| {
            if trip.gone {
                u32::try_from(
                    u64::from(paces[slot]) * u64::from(rules.expedition.no_supplies_pace_pm) / 1000,
                )
                .unwrap_or(u32::MAX)
            } else {
                paces[slot]
            }
        });
        view.expedition = Some(trip.number);
        vehicles.push(view);
    }
    let (capacity_g, capacity_ml) = if trip.vehicles.is_empty() {
        (
            u128::from(count) * u128::from(rules.expedition.pack_g),
            u128::from(count) * u128::from(rules.expedition.pack_ml),
        )
    } else {
        assignment.moving.iter().fold((0, 0), |(g, ml), (i, _, _)| {
            let kind = &rules.vehicles[trip.vehicles[*i].kind as usize];
            (
                g + u128::from(kind.carries_g),
                ml + u128::from(kind.carries_ml),
            )
        })
    };
    let limiter = if trip.gone {
        "no supplies".to_owned()
    } else {
        match trip.limiter {
            Limiter::Vehicle(i) => rules.vehicles.get(i as usize)?.id.clone(),
            Limiter::Species(i) => rules.herd_species.get(i as usize)?.animal.clone(),
            Limiter::Walkers => "walkers".to_owned(),
            Limiter::Nothing => "—".to_owned(),
        }
    };
    Some(PanelView {
        home: place.id.clone(),
        destination: destination.id.clone(),
        trader: place.pops.get(trip.trader as usize)?.kind.clone(),
        trader_slot: trip.trader,
        phase: match trip.phase {
            Phase::Outbound => "outbound",
            Phase::Waiting => "waiting",
            Phase::Homeward => "homeward",
        },
        pace_pm: trip.pace_pm,
        limiter,
        water: trip.reckoned_mode == Mode::Water,
        people,
        drovers: assignment.drovers,
        walkers: assignment.walkers,
        vehicles,
        cargo: goods(&trip.cargo),
        supplies: goods(&trip.supplies),
        wanted: goods(&trip.plan.wanted),
        load_g: assignment.loads.iter().map(|x| x.0).sum(),
        load_ml: assignment.loads.iter().map(|x| x.1).sum(),
        capacity_g,
        capacity_ml,
        left: trip.left,
        arrived: trip.arrived,
        remaining: u64::try_from(trip.remaining_hours(rules)?.ceil().to_integer())
            .unwrap_or(u64::MAX),
        food_days: if count == 0 {
            0
        } else {
            let food = rules
                .goods
                .iter()
                .zip(&trip.supplies)
                .fold(u128::from(trip.pantry_centi), |sum, (good, n)| {
                    sum.saturating_add(u128::from(*n) * u128::from(good.hunger_centi))
                });
            u64::try_from(
                food / (u128::from(count) * u128::from(rules.expedition.daily_hunger_centi)),
            )
            .unwrap_or(u64::MAX)
        },
    })
}
