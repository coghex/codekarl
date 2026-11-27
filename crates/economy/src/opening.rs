//! Trade, part F: the opening (CK-434; trade spec section 10 with rules 11b.6 and 11c.5, and the owner's r134 to r142).
//!
//! A new game opens with trade already under way, made procedurally at every start (r134): after the herds are set and before the first
//! tick, in this order, with every draw keyed by the seed, the ids and the purpose (never a list position, as CK-426's are):
//!
//! 1. each pop is given, free, goods it wants and its place does not make but another place does, fewer the farther that place is;
//! 2. each noble and monastic pop is given its fleet, already built with its animal;
//! 3. every trader looks once, in a seeded order, by the chooser of part D with vehicles it lacks given free and ranked by the cost of a
//!    year of the trip (`choose::run_first_looks`): a trip chosen starts part way out if it can start at once, or starts the game
//!    gathering;
//! 4. a trader whose first look made no job is given a seeded last look within the look timer before the start, so that its next looks
//!    spread over the first 30 days.
//!
//! The opening's hour is the hour before the first tick: the engine's own convention that a job started in an hour first moves in the next
//! one then gives a trip that has travelled `k` hours the departure hour `start - 1 - k` and the saved clock `start` finds it where the
//! unbroken game would have.

use std::collections::{BTreeMap, BTreeSet};

use crate::choose::{FirstLooks, LookObserver, Party, Ways, run_first_looks};
use crate::expedition::{Hours, ratio, species_of, whole};
use crate::swap;
use crate::trade::{self, Look};
use crate::{DataError, EconomyState, PlaceState, Rules, herd_levels, missing_data};

/// What the opening did, for the evidence and the cases.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpeningReport {
    /// Goods given, by good in rules order: the units and the pops that were given any.
    pub goods: Vec<(u64, u64)>,
    /// The vehicles given as fleets: `(place, pop slot, vehicle kind)`.
    pub fleets: Vec<(usize, u32, usize)>,
    /// The first looks.
    pub looks: FirstLooks,
    /// Traders given a last look before the start (those whose first look made no job).
    pub last_looks: u64,
}

/// A draw in `[0, bound)` for a purpose, keyed by the seed, the pop's place id and slot, and any more keys (11b.6 rule 6).
fn draw(seed: u64, place: &str, slot: u32, purpose: &str, more: &[&str], bound: u64) -> u64 {
    let slot = slot.to_string();
    let mut keys = vec![place, slot.as_str(), purpose];
    keys.extend_from_slice(more);
    crate::herd_draw(seed, &keys) % bound.max(1)
}

/// Opens a new game with trade under way (section 10). `start` is the hour of the first tick; `ways` is the network. Nothing is done
/// when the rules have the looks off (trade is not on in that game).
///
/// # Errors
/// A malformed state or rules, or a way the network cannot give. The state is left as far as it got: a new game that fails to open is
/// not played.
pub fn run_opening(
    rules: &Rules,
    state: &mut EconomyState,
    start: u64,
    seed: u64,
    ways: &mut dyn Ways,
    observer: &mut dyn LookObserver,
) -> Result<OpeningReport, DataError> {
    if !rules.expedition.looks {
        return Ok(OpeningReport::default());
    }
    // The first looks are at the hour before the first tick and the others up to a look timer before it: all within the calendar.
    let hour = start
        .checked_sub(rules.expedition.look_hours.saturating_add(1))
        .and_then(|_| start.checked_sub(1))
        .ok_or_else(|| {
            missing_data(
                "trade.opening".to_owned(),
                "the scenario starts less than a look timer after the calendar's first hour",
            )
        })?;
    let mut report = OpeningReport {
        goods: vec![(0, 0); rules.goods.len()],
        ..OpeningReport::default()
    };
    give_goods(rules, state, ways, seed, &mut report)?;
    give_fleets(rules, state, ways, &mut report)?;
    let order = first_look_order(state, seed);
    report.looks = run_first_looks(
        rules,
        state,
        hour,
        start,
        &order,
        ways,
        observer,
        &|place, slot| draw(seed, place, slot, "opening-start", &[], 1000),
    )?;
    // A trader whose look made no job looked at some hour in the timer before the start, so its next look is in the first 30 days.
    for &(place, slot) in &report.looks.idle {
        let id = state.places[place].id.clone();
        let before = draw(
            seed,
            &id,
            slot,
            "opening-last-look",
            &[],
            rules.expedition.look_hours,
        );
        state.places[place].trade.looks.insert(
            slot,
            Look {
                last: start - before.min(start),
                rest: None,
            },
        );
        report.last_looks += 1;
    }
    Ok(report)
}

/// The traders in the order they first look (11b.6 rule 23): by a hash of the seed and the pop's place id and slot, ties by place id and
/// then slot.
fn first_look_order(state: &EconomyState, seed: u64) -> Vec<(usize, usize)> {
    let mut order: Vec<(u64, &str, usize, usize)> = Vec::new();
    for (place, item) in state.places.iter().enumerate() {
        for pop in 0..item.pops.len() {
            let key = draw(
                seed,
                &item.id,
                trade::convert(pop),
                "opening-look-order",
                &[],
                u64::MAX,
            );
            order.push((key, item.id.as_str(), pop, place));
        }
    }
    order.sort_unstable();
    order
        .into_iter()
        .map(|(_, _, pop, place)| (place, pop))
        .collect()
}

/// The goods a place makes at the start (section 10 rule 1): the outputs of the methods its buildings have levels on, and the recurring
/// products and slaughter yields of the herds they keep.
pub(crate) fn made_goods(rules: &Rules, place: &PlaceState) -> BTreeSet<usize> {
    let index = |id: &str| rules.goods.iter().position(|good| good.id == id);
    let mut made = BTreeSet::new();
    for building in place
        .buildings
        .iter()
        .filter(|building| !building.abandoned)
    {
        let Some(kind) = rules
            .buildings
            .iter()
            .find(|candidate| candidate.id == building.kind)
        else {
            continue;
        };
        for (method, levels) in kind.methods.iter().zip(&building.method_levels) {
            if *levels == 0 {
                continue;
            }
            for (good, output) in &method.annual_output {
                if *output > 0
                    && let Some(good) = index(good)
                {
                    made.insert(good);
                }
            }
        }
        for (species, head) in rules.herd_species.iter().zip(&building.herds) {
            if *head == 0 {
                continue;
            }
            // A herd makes its meat and hides as surely as its milk or wool (the designer): its recurring products and its slaughter yields.
            for good in species
                .products
                .iter()
                .map(|product| &product.good)
                .chain(species.yields.iter().map(|item| &item.good))
            {
                if let Some(good) = index(good) {
                    made.insert(good);
                }
            }
        }
    }
    made
}

/// The parties whose reach the opening reads the nearness of a producer by: the shipped profiles, the cart's and each boat's.
fn shipped_parties(rules: &Rules) -> Vec<Party> {
    let mut profiles = BTreeSet::from(["cart".to_owned()]);
    for vehicle in &rules.vehicles {
        if let Some(water) = &vehicle.water {
            profiles.insert(water.clone());
        }
    }
    profiles
        .into_iter()
        .map(|profile| Party {
            profile,
            boat_only: false,
        })
        .collect()
}

/// The hours of the clipped one-way way from `home` to `other` at walking pace by the quickest of the parties (the designer: the
/// clipped hours, the quickest of the cart, barge and currach), or none if no party has a way.
fn clipped_hours(
    ways: &mut dyn Ways,
    parties: &[Party],
    home: &str,
    other: &str,
) -> Result<Option<Hours>, DataError> {
    let mut best: Option<Hours> = None;
    for party in parties {
        let found = ways
            .clipped_hours(home, other, party)
            .map_err(|problem| missing_data("trade.opening.nearness".to_owned(), &problem))?;
        if let Some(hours) = found
            && best.as_ref().is_none_or(|least| hours < *least)
        {
            best = Some(hours);
        }
    }
    Ok(best)
}

/// Gives each pop the traded goods of the opening (section 10 rule 1, 11b.6 rule 22, 11c.5 rules 17 and 18): for each good it has a buy
/// order for that no building at its place makes and some place does, a seeded share of 30 days of its target times the nearness of the
/// nearest producer, rounded down once, in the goods' data order, each as far as its store takes it. Live animals are never given, and a
/// good no place makes (from abroad) has no producer.
fn give_goods(
    rules: &Rules,
    state: &mut EconomyState,
    ways: &mut dyn Ways,
    seed: u64,
    report: &mut OpeningReport,
) -> Result<(), DataError> {
    let opening = &rules.expedition.opening;
    let made: Vec<BTreeSet<usize>> = state
        .places
        .iter()
        .map(|place| made_goods(rules, place))
        .collect();
    let mut producers: Vec<Vec<usize>> = vec![Vec::new(); rules.goods.len()];
    for (place, goods) in made.iter().enumerate() {
        for good in goods {
            producers[*good].push(place);
        }
    }
    let parties = shipped_parties(rules);
    for (place, made_here) in made.iter().enumerate() {
        // The exact clipped hours to a producer, asked once for all this place's pops and goods.
        let mut clipped: BTreeMap<usize, Option<Hours>> = BTreeMap::new();
        for pop in 0..state.places[place].pops.len() {
            let kind = state.places[place].pops[pop].kind.clone();
            let Some(limit) = rules.expedition.limit_hours.get(&kind).copied() else {
                continue;
            };
            let view = swap::trader_view(rules, &state.places[place], pop)?;
            let (id, slot) = (state.places[place].id.clone(), trade::convert(pop));
            for (good, makers) in producers.iter().enumerate() {
                if view.deficit[good] == 0
                    || species_of(rules, good).is_some()
                    || made_here.contains(&good)
                {
                    continue;
                }
                let mut nearest: Option<Hours> = None;
                for producer in makers.iter().copied().filter(|producer| *producer != place) {
                    if let std::collections::btree_map::Entry::Vacant(slot) =
                        clipped.entry(producer)
                    {
                        slot.insert(clipped_hours(
                            ways,
                            &parties,
                            &state.places[place].id,
                            &state.places[producer].id,
                        )?);
                    }
                    if let Some(found) = &clipped[&producer]
                        && nearest.as_ref().is_none_or(|best| found < best)
                    {
                        nearest = Some(found.clone());
                    }
                }
                // An unreachable producer gives none.
                let Some(nearest) = nearest else {
                    continue;
                };
                let share = opening.share_pm[0]
                    + draw(
                        seed,
                        &id,
                        slot,
                        "opening-share",
                        &[rules.goods[good].id.as_str()],
                        opening.share_pm[1] - opening.share_pm[0] + 1,
                    );
                let units = opening_amount(opening, view.target[good], share, &nearest, limit);
                if units == 0 {
                    continue;
                }
                let left =
                    crate::receive_pop(rules, &mut state.places[place].pops[pop], good, units)?;
                if units > left {
                    report.goods[good].0 += units - left;
                    report.goods[good].1 += 1;
                }
            }
        }
    }
    Ok(())
}

/// The units of a good a pop is given (11b.6 rule 22, 11c.5 rule 17): the base amount, `target` (the pop's 240-day target) over the target
/// days times the goods days, times the seeded `share` per-mille and the nearness, the whole per-mille `floor(1000 - reach * t / limit)`
/// of the producer `t` hours away for a pop whose round-trip limit is `limit` (rule 17 floors it, as its example does), the units
/// rounded down at the end. A producer half the limit away or farther gives none.
pub(crate) fn opening_amount(
    opening: &crate::expedition::OpeningRules,
    target: u128,
    share: u64,
    t: &Hours,
    limit: u64,
) -> u64 {
    let nearness = (ratio(1000, 1) - t * ratio(u128::from(opening.reach_pm), u128::from(limit)))
        .floor()
        .to_integer();
    if nearness <= 0.into() {
        return 0;
    }
    let nearness = whole(u64::try_from(nearness).unwrap_or(u64::MAX));
    // The base amount is whole first (rule 22: the floor of the target over its 240 days times 30), then the share and the nearness.
    let base = whole(
        u64::try_from(target * u128::from(opening.goods_days) / u128::from(opening.target_days))
            .unwrap_or(u64::MAX),
    ) * ratio(u128::from(share), 1000);
    u64::try_from((base * nearness / whole(1000)).floor().to_integer()).unwrap_or(u64::MAX)
}

/// Whether a pop of `culture` may build the vehicle kind (a kind with no cultures named may be built by anyone).
fn may_build(rules: &Rules, kind: usize, culture: &str) -> bool {
    let cultures = &rules.vehicles[kind].cultures;
    cultures.is_empty() || cultures.iter().any(|name| name == culture)
}

/// The id of a vehicle kind in the rules.
fn kind_of(rules: &Rules, id: Option<&String>) -> Option<usize> {
    let id = id?;
    rules.vehicles.iter().position(|vehicle| vehicle.id == *id)
}

/// Gives the fleets (section 10 rule 2, r141 and r142): each noble pop a covered wagon if its demesne's horses are above the standard
/// level, else an ox-cart; and a simple barge if its place has a landing a barge can use, else a currach if its place is on an island;
/// each monastic pop an ox-cart, or a currach if its place is on an island. Free, with their animals, no herd losing one.
fn give_fleets(
    rules: &Rules,
    state: &mut EconomyState,
    ways: &mut dyn Ways,
    report: &mut OpeningReport,
) -> Result<(), DataError> {
    let fleets = &rules.expedition.opening.fleets;
    let horses = rules
        .herd_species
        .iter()
        .position(|species| species.animal == "horses");
    let mut counters = state.trade.clone();
    for place in 0..state.places.len() {
        let id = state.places[place].id.clone();
        let island = ways.island(&id);
        let mut gifts: Vec<(usize, usize)> = Vec::new();
        for pop in 0..state.places[place].pops.len() {
            match state.places[place].pops[pop].kind.as_str() {
                "noble" => {
                    let above = match horses {
                        Some(species) => {
                            let mut above = false;
                            for building in state.places[place]
                                .buildings
                                .iter()
                                .filter(|building| building.kind == "demesne")
                            {
                                above |= herd_levels(rules, building)?[species].0 > 0;
                            }
                            above
                        }
                        None => false,
                    };
                    let land = if above {
                        kind_of(rules, fleets.noble_horses.as_ref())
                    } else {
                        kind_of(rules, fleets.noble.as_ref())
                    };
                    gifts.extend(land.map(|kind| (pop, kind)));
                    // A landing a barge can use is one from which a barge alone reaches another place.
                    let barge = kind_of(rules, fleets.barge.as_ref());
                    let landing = match barge.and_then(|kind| rules.vehicles[kind].water.clone()) {
                        Some(profile) => !ways
                            .destinations(
                                &id,
                                &Party {
                                    profile,
                                    boat_only: true,
                                },
                            )
                            .map_err(|problem| {
                                missing_data("trade.opening.fleets".to_owned(), &problem)
                            })?
                            .is_empty(),
                        None => false,
                    };
                    let boat = if landing {
                        barge
                    } else if island {
                        kind_of(rules, fleets.sea.as_ref())
                    } else {
                        None
                    };
                    gifts.extend(boat.map(|kind| (pop, kind)));
                }
                "monk" => {
                    // On an island a currach where its culture may build one, else the cart (the designer).
                    let culture = &state.places[place].pops[pop].culture;
                    let boat = kind_of(rules, fleets.monastic_island.as_ref())
                        .filter(|kind| island && may_build(rules, *kind, culture));
                    let kind = boat.or_else(|| kind_of(rules, fleets.monastic.as_ref()));
                    gifts.extend(kind.map(|kind| (pop, kind)));
                }
                _ => {}
            }
        }
        for (pop, kind) in gifts {
            // A kind its culture may not build is not given (r47: only the Irish build currachs).
            if !may_build(rules, kind, &state.places[place].pops[pop].culture) {
                continue;
            }
            trade::give_vehicle(&mut state.places[place], &mut counters, pop, kind)?;
            report.fleets.push((place, trade::convert(pop), kind));
        }
    }
    state.trade = counters;
    Ok(())
}
