//! Building travel measurements from the loaded map and network.

use ck_sim::travel::{Edge, TravelProfile, cell_step_mm, half_cell_mm, water_travel_hours};
use ck_sim::travel::{LandCell, LandMeasure, LandWay, TravelRules, measure_course};
use num_rational::BigRational;
use std::{collections::BTreeMap, sync::Arc};

/// Results of one exact, single-source travel search.
pub struct Routes {
    answers: BTreeMap<u64, ck_sim::travel::ItineraryPath>,
    transitions: Vec<Transition>,
    water_steps: Arc<Vec<WaterStep>>,
    transition_hours: Vec<BigRational>,
}

/// Immutable measurements and water graph for repeated queries with one profile.
pub struct TravelPlanner {
    ways: Arc<Vec<LandWay>>,
    water: Option<WaterGraph>,
    profile: TravelProfile,
    rules: TravelRules,
}

impl TravelPlanner {
    /// Measure a loaded scenario once for the given travel profile.
    ///
    /// # Errors
    /// Returns a message if the scenario or profile cannot form a travel graph.
    pub fn new(
        scenario: &ck_map::Scenario,
        profile: &TravelProfile,
        rules: &TravelRules,
    ) -> Result<Self, String> {
        Self::with_ways(
            scenario,
            profile,
            rules,
            Arc::new(measure_ways(scenario, rules)?),
        )
    }

    /// A planner for a profile over ways already measured for the same scenario and rules (the measurement does not depend on the
    /// profile, so planners of one network share it: [`measure_ways`] is what [`Self::new`] makes it by).
    ///
    /// # Errors
    /// Returns a message if the scenario or profile cannot form a travel graph.
    pub fn with_ways(
        scenario: &ck_map::Scenario,
        profile: &TravelProfile,
        rules: &TravelRules,
        ways: Arc<Vec<LandWay>>,
    ) -> Result<Self, String> {
        profile.validate()?;
        let water = if profile.water_class.is_some() {
            Some(water_graph(scenario, profile)?)
        } else {
            None
        };
        Ok(Self {
            ways,
            water,
            profile: profile.clone(),
            rules: rules.clone(),
        })
    }

    /// Measured ways, in stable ID order, for reading itinerary legs.
    #[must_use]
    pub fn ways(&self) -> &[LandWay] {
        &self.ways
    }

    /// Search using the current stretch states without remeasuring the network.
    ///
    /// The scenario must be the same one used to construct this planner.
    ///
    /// # Errors
    /// Returns a message if the start or current stretch states are invalid.
    pub fn routes_from(
        &self,
        scenario: &ck_map::Scenario,
        states: &BTreeMap<u64, ck_sim::travel::WayState>,
        start: u64,
    ) -> Result<Routes, String> {
        routes_from_inner(
            scenario,
            &self.ways,
            states,
            &self.profile,
            &self.rules,
            start,
            self.water.as_ref(),
            false,
        )
    }

    /// The reverse single-source search (CK-433, trade spec 11d.2 rule 3): the quickest way from every place **to** `target`, found
    /// over the same transitions as [`Self::routes_from`] with each reversed, by the same exact-time ordering and fixed tie order. Its
    /// itineraries are read in travel order, from the place to the target, so the way home of an expedition is reckoned in its own
    /// direction (a climb is counted going up, a road's condition as it is).
    ///
    /// # Errors
    /// Returns a message if the target or current stretch states are invalid.
    pub fn routes_to(
        &self,
        scenario: &ck_map::Scenario,
        states: &BTreeMap<u64, ck_sim::travel::WayState>,
        target: u64,
    ) -> Result<Routes, String> {
        routes_from_inner(
            scenario,
            &self.ways,
            states,
            &self.profile,
            &self.rules,
            target,
            self.water.as_ref(),
            true,
        )
    }
}

impl Routes {
    /// All places at or below an exact whole-hour limit, in network order.
    #[must_use]
    pub fn places_within(
        &self,
        scenario: &ck_map::Scenario,
        hours: u64,
    ) -> Vec<(String, BigRational)> {
        let limit = BigRational::from_integer(hours.into());
        let Some(n) = &scenario.network else {
            return Vec::new();
        };
        n.nodes
            .iter()
            .filter(|node| node.kind == ck_map::LandNodeKind::Place)
            .filter_map(|node| {
                let name = node.place.as_ref()?;
                let path = self.to(node.id)?;
                (path.hours <= limit).then(|| (name.clone(), path.hours.clone()))
            })
            .collect()
    }
    /// A water piece recorded on an itinerary, including a reach's direction.
    #[must_use]
    pub fn water_step(&self, index: usize) -> Option<&WaterStep> {
        self.water_steps.get(index)
    }
    /// The exact path to a network node, if reachable.
    #[must_use]
    pub fn to(&self, node: u64) -> Option<&ck_sim::travel::ItineraryPath> {
        self.answers.get(&node)
    }

    /// Expand a path into its ordered, timed legs.
    #[must_use]
    #[expect(
        clippy::too_many_lines,
        reason = "Each transition is expanded into its exact ordered itinerary legs here."
    )]
    pub fn itinerary(
        &self,
        scenario: &ck_map::Scenario,
        ways: &[LandWay],
        node: u64,
        rules: &TravelRules,
    ) -> Option<Vec<TripLeg>> {
        let n = scenario.network.as_ref()?;
        let mut result = Vec::new();
        let mut water: Option<(usize, Vec<usize>, BigRational, BigRational)> = None;
        for &id in &self.to(node)?.edges {
            let index = usize::try_from(id).ok()?;
            let hours = self.transition_hours.get(index)?.clone();
            match self.transitions.get(index)? {
                Transition::Land { way, backward } => {
                    let measured = ways.get(*way)?;
                    let stretch = n
                        .stretches
                        .iter()
                        .find(|stretch| stretch.id == measured.id)?;
                    let kind = if stretch.tidal {
                        TripKind::Tidal {
                            way: *way,
                            backward: *backward,
                        }
                    } else {
                        TripKind::Land {
                            way: *way,
                            backward: *backward,
                        }
                    };
                    let length_mm = if stretch.tidal {
                        BigRational::from_integer(num_bigint::BigInt::from(stretch.length_m) * 1000)
                    } else {
                        stretch.cells.windows(2).try_fold(
                            BigRational::from_integer(0.into()),
                            |total, pair| {
                                Some(total + land_step_length(scenario, pair[0], pair[1])?)
                            },
                        )?
                    };
                    result.push(TripLeg {
                        kind,
                        length_mm,
                        hours,
                    });
                }
                Transition::Board { landing } => {
                    let entry = n.landings.get(*landing)?;
                    result.push(TripLeg {
                        kind: TripKind::Transfer {
                            landing: Some(*landing),
                            node: entry.node,
                            boarding: true,
                        },
                        length_mm: BigRational::from_integer(
                            num_bigint::BigInt::from(entry.approach_m) * 1000,
                        ),
                        hours,
                    });
                    water = Some((
                        *landing,
                        Vec::new(),
                        BigRational::from_integer(0.into()),
                        BigRational::from_integer(0.into()),
                    ));
                }
                Transition::Water { step } => {
                    let (_, pieces, length, time) = water.as_mut()?;
                    pieces.push(*step);
                    *length += self.water_steps.get(*step)?.length(scenario)?;
                    *time += hours;
                }
                Transition::LandAt { landing } => {
                    let (from, pieces, length, time) = water.take()?;
                    result.push(TripLeg {
                        kind: TripKind::Water {
                            from,
                            to: *landing,
                            pieces,
                        },
                        length_mm: length,
                        hours: time,
                    });
                    let entry = n.landings.get(*landing)?;
                    result.push(TripLeg {
                        kind: TripKind::Transfer {
                            landing: Some(*landing),
                            node: entry.node,
                            boarding: false,
                        },
                        length_mm: BigRational::from_integer(
                            num_bigint::BigInt::from(entry.approach_m) * 1000,
                        ),
                        hours,
                    });
                }
                Transition::Strait { index, backward } => {
                    let strait = n.straits.get(*index)?;
                    let transfer = BigRational::new(rules.transfer_thousandths.into(), 1000.into());
                    for boarding in [true, false] {
                        if !boarding {
                            result.push(TripLeg {
                                kind: TripKind::Strait {
                                    index: *index,
                                    backward: *backward,
                                },
                                length_mm: BigRational::from_integer(
                                    num_bigint::BigInt::from(strait.length_m) * 1000,
                                ),
                                hours: &hours - &transfer * num_bigint::BigInt::from(2),
                            });
                        }
                        let from = if *backward { strait.to } else { strait.from };
                        let to = if *backward { strait.from } else { strait.to };
                        result.push(TripLeg {
                            kind: TripKind::Transfer {
                                landing: None,
                                node: if boarding { from } else { to },
                                boarding,
                            },
                            length_mm: BigRational::from_integer(0.into()),
                            hours: transfer.clone(),
                        });
                    }
                }
            }
        }
        if water.is_some() {
            return None;
        }
        Some(result)
    }

    /// Drawing position at a distance along a leg, in global fixed-point cell units.
    #[must_use]
    #[expect(
        clippy::too_many_lines,
        reason = "Each leg kind maps distance into its own stored geometry."
    )]
    pub fn position(
        &self,
        scenario: &ck_map::Scenario,
        ways: &[LandWay],
        leg: &TripLeg,
        distance_mm: &BigRational,
    ) -> Option<(BigRational, BigRational)> {
        let n = scenario.network.as_ref()?;
        if *distance_mm < BigRational::from_integer(0.into()) {
            return None;
        }
        let distance = distance_mm.min(&leg.length_mm).clone();
        let node_point = |id| {
            n.nodes
                .iter()
                .find(|node| node.id == id)
                .map(|node| centre(scenario, node.cell))
        };
        let ratio = |length: &BigRational, along: &BigRational| {
            if *length == BigRational::from_integer(0.into()) {
                BigRational::from_integer(0.into())
            } else {
                along / length
            }
        };
        match &leg.kind {
            TripKind::Land { way, backward } | TripKind::Tidal { way, backward } => {
                let way_id = ways.get(*way)?.id;
                let stretch = n.stretches.iter().find(|stretch| stretch.id == way_id)?;
                let mut cells = stretch.cells.clone();
                if *backward {
                    cells.reverse();
                }
                if stretch.tidal {
                    return Some(ck_sim::travel::interpolate(
                        &centre(scenario, *cells.first()?),
                        &centre(scenario, *cells.last()?),
                        &ratio(&leg.length_mm, &distance),
                    ));
                }
                let mut left = distance;
                for pair in cells.windows(2) {
                    let length = land_step_length(scenario, pair[0], pair[1])?;
                    if left <= length {
                        return Some(ck_sim::travel::interpolate(
                            &centre(scenario, pair[0]),
                            &centre(scenario, pair[1]),
                            &ratio(&length, &left),
                        ));
                    }
                    left -= length;
                }
                Some(centre(scenario, *cells.last()?))
            }
            TripKind::Transfer {
                landing,
                node,
                boarding,
            } => {
                let from = node_point(*node)?;
                if leg.length_mm == BigRational::from_integer(0.into()) {
                    return Some(from);
                }
                let shore = landing
                    .and_then(|index| n.landings.get(index))
                    .map_or_else(|| from.clone(), |landing| centre(scenario, landing.shore));
                let (a, b) = if *boarding {
                    (from, shore)
                } else {
                    (shore, from)
                };
                Some(ck_sim::travel::interpolate(
                    &a,
                    &b,
                    &ratio(&leg.length_mm, &distance),
                ))
            }
            TripKind::Strait { index, backward } => {
                let strait = n.straits.get(*index)?;
                let (from, to) = if *backward {
                    (strait.to, strait.from)
                } else {
                    (strait.from, strait.to)
                };
                Some(ck_sim::travel::interpolate(
                    &node_point(from)?,
                    &node_point(to)?,
                    &ratio(&leg.length_mm, &distance),
                ))
            }
            TripKind::Water { from, pieces, .. } => {
                let mut left = distance;
                for index in pieces {
                    let step = self.water_steps.get(*index)?;
                    let length = step.length(scenario)?;
                    if left <= length {
                        return step.position(scenario, &ratio(&length, &left));
                    }
                    left -= length;
                }
                let landing = n.landings.get(*from)?;
                if let Some(cell) = landing.cell {
                    Some(centre(scenario, cell))
                } else {
                    joint_position(scenario, landing.joint?)
                }
            }
        }
    }
}

impl Routes {
    /// The distances along a leg **in the direction it is travelled**, in millimetres, where its path bends: the cell centres of a land
    /// stretch, the boundaries of the water steps of a water leg and the surveyed bends inside each river reach. Between two of them the
    /// path is a straight line, which lets a search for where the path comes within a distance of a point test each straight piece
    /// exactly. Empty for a leg that is one straight line.
    #[must_use]
    pub(super) fn breakpoints(
        &self,
        scenario: &ck_map::Scenario,
        ways: &[LandWay],
        leg: &TripLeg,
    ) -> Vec<BigRational> {
        let mut at = Vec::new();
        let zero = BigRational::from_integer(0.into());
        let Some(n) = scenario.network.as_ref() else {
            return at;
        };
        match &leg.kind {
            TripKind::Land { way, backward } => {
                let Some(stretch) = ways.get(*way).and_then(|measured| {
                    n.stretches.iter().find(|stretch| stretch.id == measured.id)
                }) else {
                    return at;
                };
                // The same order of cells `position` walks.
                let mut cells = stretch.cells.clone();
                if *backward {
                    cells.reverse();
                }
                let mut total = zero;
                for pair in cells.windows(2) {
                    let Some(length) = land_step_length(scenario, pair[0], pair[1]) else {
                        return Vec::new();
                    };
                    total += length;
                    at.push(total.clone());
                }
            }
            TripKind::Water { pieces, .. } => {
                let mut cursor = zero;
                for index in pieces {
                    let Some(step) = self.water_steps.get(*index) else {
                        return Vec::new();
                    };
                    let Some(length) = step.length(scenario) else {
                        return Vec::new();
                    };
                    // A river reach bends where its survey does; the bends are placed from the end the step is travelled from.
                    if let WaterStep::Reach { reach, downstream } = step
                        && let Some(entry) = n.reaches.get(*reach)
                    {
                        let mut inside: Vec<BigRational> = n
                            .water()
                            .bends_along(entry)
                            .into_iter()
                            .map(|bend| BigRational::from_integer(bend.into()))
                            .collect();
                        if !*downstream {
                            inside = inside.into_iter().map(|bend| &length - bend).collect();
                        }
                        inside.sort();
                        at.extend(inside.into_iter().map(|bend| &cursor + bend));
                    }
                    cursor += length;
                    at.push(cursor.clone());
                }
            }
            TripKind::Tidal { .. } | TripKind::Transfer { .. } | TripKind::Strait { .. } => {}
        }
        at
    }
}

/// An itinerary leg with its exact length and time.
#[derive(Clone, Debug)]
pub struct TripLeg {
    /// The route and direction of this leg.
    pub kind: TripKind,
    /// Ground length in millimetres.
    pub length_mm: BigRational,
    /// Exact time in hours.
    pub hours: BigRational,
}

pub(super) fn centre(scenario: &ck_map::Scenario, cell: [i32; 2]) -> (BigRational, BigRational) {
    let origin = scenario.map.data.crop.origin;
    (
        BigRational::from_integer(
            ((i64::from(origin.x) + i64::from(cell[0])) * 32768 + 16384).into(),
        ),
        BigRational::from_integer(
            ((i64::from(origin.y) + i64::from(cell[1])) * 32768 + 16384).into(),
        ),
    )
}

fn joint_position(scenario: &ck_map::Scenario, joint: usize) -> Option<(BigRational, BigRational)> {
    let n = scenario.network.as_ref()?;
    let reach = n
        .reaches
        .iter()
        .find(|reach| reach.usable && (reach.from == joint || reach.to == joint))?;
    let point = n.water().position_along(
        reach,
        if reach.from == joint {
            0
        } else {
            reach.length_mm
        },
    )?;
    Some((
        BigRational::from_integer(point.x.into()),
        BigRational::from_integer(point.y.into()),
    ))
}

pub(super) fn land_step_length(
    scenario: &ck_map::Scenario,
    from: [i32; 2],
    to: [i32; 2],
) -> Option<BigRational> {
    let n = scenario.network.as_ref()?;
    let a = *n.cell_side_mm.get(usize::try_from(from[1]).ok()?)?;
    let b = *n.cell_side_mm.get(usize::try_from(to[1]).ok()?)?;
    ck_sim::travel::land_step_mm(
        a,
        b,
        i64::from(to[0]) - i64::from(from[0]),
        i64::from(to[1]) - i64::from(from[1]),
    )
}

/// The kind and stored identifiers of an itinerary leg.
#[derive(Clone, Debug)]
pub enum TripKind {
    /// A land stretch, indexed in the measured ways, with its direction.
    Land {
        /// Index in the measured ways.
        way: usize,
        /// Whether travelled backwards.
        backward: bool,
    },
    /// A tidal stretch, indexed in the measured ways, with its direction.
    Tidal {
        /// Index in the measured ways.
        way: usize,
        /// Whether travelled backwards.
        backward: bool,
    },
    /// A landing approach, or a zero-length transfer at a strait.
    Transfer {
        /// Landing index, absent at a strait.
        landing: Option<usize>,
        /// Node ID.
        node: u64,
        /// True when boarding.
        boarding: bool,
    },
    /// A water way between landing indices, with its stored piece indices.
    Water {
        /// Starting landing index.
        from: usize,
        /// Ending landing index.
        to: usize,
        /// Internal water piece IDs.
        pieces: Vec<usize>,
    },
    /// A strait in the network, with its direction.
    Strait {
        /// Strait index.
        index: usize,
        /// Whether travelled backwards.
        backward: bool,
    },
}

pub(super) fn thousandths(value: &BigRational) -> String {
    let scaled = (value.numer() * 2000 + value.denom()) / (value.denom() * 2);
    let whole = &scaled / 1000;
    let part = &scaled % 1000;
    format!("{whole}.{part:03}")
}

fn format_itinerary(
    scenario: &ck_map::Scenario,
    ways: &[LandWay],
    routes: &Routes,
    node: u64,
    rules: &TravelRules,
) -> String {
    use std::fmt::Write;
    let Some(path) = routes.to(node) else {
        return "cannot be reached\n".to_owned();
    };
    let Some(legs) = routes.itinerary(scenario, ways, node, rules) else {
        return "cannot describe itinerary\n".to_owned();
    };
    let n = scenario.network.as_ref().expect("routes require network");
    let mut output = format!("total: {} h\n", ck_sim::travel::shown_hours(&path.hours));
    for leg in legs {
        let kind = match &leg.kind {
            TripKind::Land { way, backward } | TripKind::Tidal { way, backward } => {
                let way = &ways[*way];
                let kind = if matches!(leg.kind, TripKind::Tidal { .. }) {
                    "tidal"
                } else {
                    "land"
                };
                let wade_delay = BigRational::new(
                    num_bigint::BigInt::from(way.wades) * rules.wade_thousandths,
                    1000.into(),
                );
                let ford_delay =
                    way.fords
                        .iter()
                        .fold(BigRational::from_integer(0.into()), |sum, width| {
                            sum + BigRational::new(rules.ford_thousandths.into(), 1000.into())
                                + BigRational::new(
                                    num_bigint::BigInt::from(*width)
                                        * rules.ford_per_metre_thousandths,
                                    10_000.into(),
                                )
                        });
                format!(
                    "{kind} stretch {} {} (wades {}: {} h, fords {}: {} h)",
                    way.id,
                    if *backward { "backward" } else { "forward" },
                    way.wades,
                    thousandths(&wade_delay),
                    way.fords.len(),
                    thousandths(&ford_delay)
                )
            }
            TripKind::Transfer {
                landing, boarding, ..
            } => match landing {
                Some(index) => format!(
                    "transfer {} landing {}",
                    if *boarding { "at" } else { "from" },
                    n.landings[*index].id
                ),
                None => "strait transfer".to_owned(),
            },
            TripKind::Water { from, to, pieces } => format!(
                "water landing {} to {} ({} steps)",
                n.landings[*from].id,
                n.landings[*to].id,
                pieces.len()
            ),
            TripKind::Strait { index, backward } => {
                let strait = &n.straits[*index];
                format!(
                    "strait {} {}",
                    strait.place,
                    if *backward { "backward" } else { "forward" }
                )
            }
        };
        let metres = &leg.length_mm / num_bigint::BigInt::from(1000);
        writeln!(
            output,
            "{kind}: {} m, {} h",
            thousandths(&metres),
            thousandths(&leg.hours)
        )
        .expect("string write");
    }
    output
}

pub(super) fn current_way_states(
    states: &BTreeMap<u64, super::stretch::StretchState>,
) -> BTreeMap<u64, ck_sim::travel::WayState> {
    states
        .iter()
        .map(|(&id, state)| {
            (
                id,
                ck_sim::travel::WayState {
                    road: state.category == 1,
                    level: state.level,
                    condition: state.condition,
                },
            )
        })
        .collect()
}

pub(crate) fn command(arguments: impl IntoIterator<Item = String>) -> Result<String, String> {
    use std::fmt::Write;
    let args: Vec<String> = arguments.into_iter().collect();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let scenario = ck_map::load_scenario(root.join("assets/scenarios/ireland-768"))
        .map_err(|error| error.to_string())?;
    let rules = TravelRules::parse(
        &std::fs::read_to_string(root.join("assets/game/travel.toml"))
            .map_err(|error| error.to_string())?,
    )?;
    let profile_name = args.first().map_or("cart", String::as_str);
    let profile = rules
        .profiles
        .get(profile_name)
        .ok_or_else(|| format!("unknown profile {profile_name}"))?;
    let n = scenario.network.as_ref().ok_or("scenario has no network")?;
    let planner = TravelPlanner::new(&scenario, profile, &rules)?;
    let ways = planner.ways();
    let states = current_way_states(&super::stretch::starting(Some(n)));
    let find = |name: &str| {
        n.place_node(name)
            .map(|node| node.id)
            .ok_or_else(|| format!("unknown place {name}"))
    };
    if let [_, from, to, flag, rest @ ..] = args.as_slice()
        && flag == "--expedition"
    {
        let pace: u32 = rest
            .first()
            .map_or(Ok(1000), |pace| pace.parse())
            .map_err(|_| "pace must be a whole number of per-mille")?;
        let (from, to) = (find(from)?, find(to)?);
        return super::expedition_route::report(
            &scenario,
            &rules,
            profile,
            &states,
            [(from, to), (to, from)],
            pace,
        );
    }
    if args.len() == 1 || args.is_empty() {
        let mut output = String::new();
        for (from, to) in [
            ("tara", "clonmacnoise"),
            ("clonmacnoise", "clonfert"),
            ("tara", "killeany"),
            ("tara", "inishmacsaint"),
        ] {
            let routes = planner.routes_from(&scenario, &states, find(from)?)?;
            let answer = routes.to(find(to)?).map_or_else(
                || "unreachable".to_owned(),
                |path| format!("{} h", ck_sim::travel::shown_hours(&path.hours)),
            );
            writeln!(output, "{from} -> {to}: {answer}").expect("string write");
        }
        return Ok(output);
    }
    let start = find(&args[1])?;
    let routes = planner.routes_from(&scenario, &states, start)?;
    match args.as_slice() {
        [_, _, destination] => Ok(format_itinerary(
            &scenario,
            ways,
            &routes,
            find(destination)?,
            &rules,
        )),
        [_, _, flag, hours] if flag == "--within" => {
            let hours: u64 = hours.parse().map_err(|_| "limit must be whole hours")?;
            let mut output = String::new();
            for (name, time) in routes.places_within(&scenario, hours) {
                writeln!(output, "{name}: {} h", ck_sim::travel::shown_hours(&time))
                    .expect("string write");
            }
            Ok(output)
        }
        _ => Err(
            "usage: travel-times [PROFILE [FROM [TO [--expedition [PACE]] | --within HOURS]]]"
                .into(),
        ),
    }
}

#[derive(Clone, Debug)]
enum Transition {
    Land { way: usize, backward: bool },
    Board { landing: usize },
    LandAt { landing: usize },
    Water { step: usize },
    Strait { index: usize, backward: bool },
}

/// Find all reachable nodes for a profile and the current state of each stretch.
///
/// # Errors
/// Returns a message if the scenario, profile or network is inconsistent.
pub fn routes_from(
    scenario: &ck_map::Scenario,
    ways: &[LandWay],
    states: &BTreeMap<u64, ck_sim::travel::WayState>,
    profile: &TravelProfile,
    rules: &TravelRules,
    start: u64,
) -> Result<Routes, String> {
    routes_from_inner(scenario, ways, states, profile, rules, start, None, false)
}

#[expect(
    clippy::too_many_lines,
    clippy::too_many_arguments,
    reason = "This builds the connected land, water and boat-only transfer states for one search, forward or reverse."
)]
fn routes_from_inner(
    scenario: &ck_map::Scenario,
    ways: &[LandWay],
    states: &BTreeMap<u64, ck_sim::travel::WayState>,
    profile: &TravelProfile,
    rules: &TravelRules,
    start: u64,
    prepared_water: Option<&WaterGraph>,
    reverse: bool,
) -> Result<Routes, String> {
    profile.validate()?;
    let n = scenario.network.as_ref().ok_or("scenario has no network")?;
    let node_ids = n
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| Ok((node.id, u64::try_from(index).map_err(|_| "too many nodes")?)))
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let source = *node_ids.get(&start).ok_or("unknown starting node")?;
    for way in ways {
        let state = states
            .get(&way.id)
            .ok_or_else(|| format!("missing state for way {}", way.id))?;
        if state.level > 5 || state.condition > 1000 {
            return Err(format!("invalid state for way {}", way.id));
        }
    }
    let mut edges = Vec::<ck_sim::travel::ItineraryEdge>::new();
    let mut transitions = Vec::<Transition>::new();
    let mut transition_hours = Vec::<BigRational>::new();
    let mut next_leg = 0_u64;
    let mut push = |from: u64,
                    to: u64,
                    hours: BigRational,
                    kind: Transition,
                    leg_count: u8,
                    water_step: bool|
     -> Result<(), String> {
        let id = u64::try_from(transitions.len()).map_err(|_| "too many transitions")?;
        let mut leg_ids = Vec::new();
        for _ in 0..leg_count {
            leg_ids.push(next_leg);
            next_leg = next_leg.checked_add(1).ok_or("too many legs")?;
        }
        transition_hours.push(hours.clone());
        edges.push(ck_sim::travel::ItineraryEdge {
            from,
            to,
            id,
            hours,
            leg_ids,
            water_step,
        });
        transitions.push(kind);
        Ok(())
    };
    for edge in ck_sim::travel::land_edges(ways, states, profile, rules) {
        let index = usize::try_from(edge.id / 2).map_err(|_| "way index too large")?;
        push(
            *node_ids.get(&edge.from).ok_or("unknown way start")?,
            *node_ids.get(&edge.to).ok_or("unknown way end")?,
            edge.hours,
            Transition::Land {
                way: index,
                backward: edge.id % 2 == 1,
            },
            1,
            false,
        )?;
    }
    let mut water_steps = Arc::new(Vec::new());
    let mut endings = Vec::<(u64, u64, usize, BigRational)>::new();
    if let Some(class_name) = &profile.water_class {
        let class = n
            .classes
            .iter()
            .position(|class| &class.id == class_name)
            .ok_or("unknown water class")?;
        let fallback;
        let graph = if let Some(graph) = prepared_water {
            graph
        } else {
            fallback = water_graph(scenario, profile)?;
            &fallback
        };
        let base = u64::try_from(n.nodes.len()).map_err(|_| "too many nodes")?;
        let width = usize::try_from(scenario.map.data.crop.width).map_err(|_| "width too large")?;
        let contact = |landing: &ck_map::Landing| -> Result<u64, String> {
            let node = if let Some(joint) = landing.joint {
                u64::try_from(joint).map_err(|_| "joint too large")?
            } else if let Some(cell) = landing.cell {
                let x = usize::try_from(cell[0]).map_err(|_| "negative column")?;
                let y = usize::try_from(cell[1]).map_err(|_| "negative row")?;
                u64::try_from(n.joints.len() + y * width + x).map_err(|_| "cell ID too large")?
            } else {
                return Err("landing has no contact".into());
            };
            base.checked_add(node)
                .ok_or_else(|| "water node overflow".into())
        };
        water_steps = Arc::clone(&graph.steps);
        for (index, edge) in graph.edges.iter().cloned().enumerate() {
            push(
                base.checked_add(edge.from).ok_or("water node overflow")?,
                base.checked_add(edge.to).ok_or("water node overflow")?,
                edge.hours,
                Transition::Water { step: index },
                0,
                true,
            )?;
        }
        let land = profile.path.is_some();
        let mut next_node = base
            .checked_add(
                u64::try_from(
                    n.joints.len()
                        + width
                            * usize::try_from(scenario.map.data.crop.height)
                                .map_err(|_| "height too large")?,
                )
                .map_err(|_| "too many water nodes")?,
            )
            .ok_or("node overflow")?;
        for (index, landing) in n.landings.iter().enumerate() {
            if landing.bodies.get(class).copied().unwrap_or(-1) < 0 {
                continue;
            }
            let Some(transfer) = ck_sim::travel::transfer_hours(
                landing.approach_m,
                profile.path.map(|speeds| speeds[0]),
                profile.slowing,
                rules,
            ) else {
                continue;
            };
            let hub = *node_ids.get(&landing.node).ok_or("landing node missing")?;
            let water = contact(landing)?;
            push(
                hub,
                water,
                transfer.clone(),
                Transition::Board { landing: index },
                2,
                false,
            )?;
            if land {
                push(
                    water,
                    hub,
                    transfer,
                    Transition::LandAt { landing: index },
                    1,
                    false,
                )?;
            } else {
                // A terminal transition is considered only at the queried destination, so it cannot portage a boat.
                endings.push((landing.node, water, index, transfer.clone()));
                for (strait_index, strait) in n
                    .straits
                    .iter()
                    .enumerate()
                    .filter(|(_, strait)| strait.classes.contains(class_name))
                {
                    let (destination, backward) = if strait.from == landing.node {
                        (strait.to, false)
                    } else if strait.to == landing.node {
                        (strait.from, true)
                    } else {
                        continue;
                    };
                    let gate = next_node;
                    next_node = next_node.checked_add(1).ok_or("node overflow")?;
                    push(
                        water,
                        gate,
                        transfer.clone(),
                        Transition::LandAt { landing: index },
                        1,
                        false,
                    )?;
                    let hours = ck_sim::travel::strait_hours(
                        strait.length_m,
                        profile.sea.ok_or("missing sea speed")?,
                        profile.slowing,
                        rules,
                    );
                    push(
                        gate,
                        node_ids[&destination],
                        hours,
                        Transition::Strait {
                            index: strait_index,
                            backward,
                        },
                        3,
                        false,
                    )?;
                }
            }
        }
        for (index, strait) in n
            .straits
            .iter()
            .enumerate()
            .filter(|(_, strait)| strait.classes.contains(class_name))
        {
            let hours = ck_sim::travel::strait_hours(
                strait.length_m,
                profile.sea.ok_or("missing sea speed")?,
                profile.slowing,
                rules,
            );
            push(
                node_ids[&strait.from],
                node_ids[&strait.to],
                hours.clone(),
                Transition::Strait {
                    index,
                    backward: false,
                },
                3,
                false,
            )?;
            push(
                node_ids[&strait.to],
                node_ids[&strait.from],
                hours,
                Transition::Strait {
                    index,
                    backward: true,
                },
                3,
                false,
            )?;
        }
    }
    if reverse {
        return reverse_routes(
            source,
            start,
            &edges,
            transitions,
            transition_hours,
            water_steps,
            endings,
            next_leg,
            &node_ids,
        );
    }
    let search = ck_sim::travel::itinerary_search(source, &edges);
    let mut answers = node_ids
        .iter()
        .filter_map(|(&node, &id)| search.path(id).map(|path| (node, path)))
        .collect::<BTreeMap<_, _>>();
    for (node, water, landing, transfer) in endings {
        let Some(mut candidate) = search.path(water) else {
            continue;
        };
        let id = u64::try_from(transitions.len()).map_err(|_| "too many transitions")?;
        candidate.hours += &transfer;
        candidate.legs.push(next_leg);
        next_leg = next_leg.checked_add(1).ok_or("too many legs")?;
        candidate.edges.push(id);
        transitions.push(Transition::LandAt { landing });
        transition_hours.push(transfer);
        let replace = answers.get(&node).is_none_or(|old| {
            (&candidate.hours, candidate.legs.len(), &candidate.legs)
                < (&old.hours, old.legs.len(), &old.legs)
        });
        if replace {
            answers.insert(node, candidate);
        }
    }
    Ok(Routes {
        answers,
        transitions,
        water_steps,
        transition_hours,
    })
}

/// The reverse search over the edges a forward search would use, into `source`: for every node a quickest itinerary home (exact time,
/// then the legs in the order they are travelled, `ck_sim::travel::itinerary_search_to`, whose fixed tie rule is documented there), in
/// travel order. Its time and legs are a forward search's from that node; among ways of equal time and legs its own rule chooses, and
/// the itinerary it gives is the one evaluated and the one frozen. The terminal landing of a boat-only way, which a forward search
/// considers only at the queried place, ends the way at the target.
#[expect(
    clippy::too_many_arguments,
    reason = "The search's edges, transitions, water pieces and terminal landings are all the graph the reversal needs."
)]
fn reverse_routes(
    source: u64,
    target: u64,
    edges: &[ck_sim::travel::ItineraryEdge],
    mut transitions: Vec<Transition>,
    mut transition_hours: Vec<BigRational>,
    water_steps: Arc<Vec<WaterStep>>,
    endings: Vec<(u64, u64, usize, BigRational)>,
    mut next_leg: u64,
    node_ids: &BTreeMap<u64, u64>,
) -> Result<Routes, String> {
    let mut into = edges.to_vec();
    for (node, water, landing, transfer) in endings {
        if node != target {
            continue;
        }
        let id = u64::try_from(transitions.len()).map_err(|_| "too many transitions")?;
        transitions.push(Transition::LandAt { landing });
        transition_hours.push(transfer.clone());
        into.push(ck_sim::travel::ItineraryEdge {
            from: water,
            to: source,
            id,
            hours: transfer,
            leg_ids: vec![next_leg],
            water_step: false,
        });
        next_leg = next_leg.checked_add(1).ok_or("too many legs")?;
    }
    let search = ck_sim::travel::itinerary_search_to(source, &into);
    let answers = node_ids
        .iter()
        .filter_map(|(&node, &id)| Some((node, search.path(id)?)))
        .collect::<BTreeMap<_, _>>();
    Ok(Routes {
        answers,
        transitions,
        water_steps,
        transition_hours,
    })
}

/// A directed piece of a water way.
#[derive(Clone, Debug)]
pub enum WaterStep {
    /// A stored river reach.
    Reach {
        /// Reach index.
        reach: usize,
        /// Whether the reach is travelled downstream.
        downstream: bool,
    },
    /// A zero-length join between surveyed stretches.
    ReachJoin {
        /// Starting joint index.
        from: usize,
        /// Ending joint index.
        to: usize,
    },
    /// A joint-to-open-cell join.
    CellJoin {
        /// Joint index.
        joint: usize,
        /// Open cell coordinates.
        cell: [i32; 2],
        /// Whether travelling from the joint to the cell.
        to_cell: bool,
    },
    /// A legal move between two open cells.
    Open {
        /// Starting cell.
        from: [i32; 2],
        /// Ending cell.
        to: [i32; 2],
    },
}

struct WaterGraph {
    edges: Vec<Edge>,
    steps: Arc<Vec<WaterStep>>,
}

impl WaterStep {
    pub(super) fn length(&self, scenario: &ck_map::Scenario) -> Option<BigRational> {
        let n = scenario.network.as_ref()?;
        match self {
            Self::Reach { reach, .. } => Some(BigRational::from_integer(
                n.reaches.get(*reach)?.length_mm.into(),
            )),
            Self::ReachJoin { .. } => Some(BigRational::from_integer(0.into())),
            Self::CellJoin { cell, .. } => Some(half_cell_mm(
                *n.cell_side_mm.get(usize::try_from(cell[1]).ok()?)?,
            )),
            Self::Open { from, to } => Some(cell_step_mm(
                *n.cell_side_mm.get(usize::try_from(from[1]).ok()?)?,
                *n.cell_side_mm.get(usize::try_from(to[1]).ok()?)?,
                from[0] != to[0] && from[1] != to[1],
            )),
        }
    }
    /// Position at a fraction of a water step, in global fixed-point cell units.
    pub(super) fn position(
        &self,
        scenario: &ck_map::Scenario,
        proportion: &BigRational,
    ) -> Option<(BigRational, BigRational)> {
        let n = scenario.network.as_ref()?;
        let centre = |cell: [i32; 2]| {
            let origin = scenario.map.data.crop.origin;
            (
                BigRational::from_integer(
                    ((i64::from(origin.x) + i64::from(cell[0])) * 32768 + 16384).into(),
                ),
                BigRational::from_integer(
                    ((i64::from(origin.y) + i64::from(cell[1])) * 32768 + 16384).into(),
                ),
            )
        };
        let joint_point = |joint: usize| {
            let reach = n
                .reaches
                .iter()
                .find(|reach| reach.usable && (reach.from == joint || reach.to == joint))?;
            let distance = if reach.from == joint {
                0
            } else {
                reach.length_mm
            };
            let point = n.water().position_along(reach, distance)?;
            Some((
                BigRational::from_integer(point.x.into()),
                BigRational::from_integer(point.y.into()),
            ))
        };
        match self {
            Self::Reach { reach, downstream } => {
                let reach = n.reaches.get(*reach)?;
                let p = if *downstream {
                    proportion.clone()
                } else {
                    BigRational::from_integer(1.into()) - proportion
                };
                let distance = u64::try_from(
                    (p * BigRational::from_integer(reach.length_mm.into())).to_integer(),
                )
                .ok()?;
                let point = n.water().position_along(reach, distance)?;
                Some((
                    BigRational::from_integer(point.x.into()),
                    BigRational::from_integer(point.y.into()),
                ))
            }
            Self::ReachJoin { from, to } => {
                let _ = joint_point(*to)?;
                joint_point(*from)
            }
            Self::CellJoin {
                joint,
                cell,
                to_cell,
            } => {
                let j = joint_point(*joint)?;
                let c = centre(*cell);
                Some(if *to_cell {
                    ck_sim::travel::interpolate(&j, &c, proportion)
                } else {
                    ck_sim::travel::interpolate(&c, &j, proportion)
                })
            }
            Self::Open { from, to } => Some(ck_sim::travel::interpolate(
                &centre(*from),
                &centre(*to),
                proportion,
            )),
        }
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "All stored reaches, joins and legal open-water moves form one directed graph."
)]
fn water_graph(scenario: &ck_map::Scenario, profile: &TravelProfile) -> Result<WaterGraph, String> {
    let n = scenario.network.as_ref().ok_or("scenario has no network")?;
    let class_name = profile
        .water_class
        .as_ref()
        .ok_or("party has no water class")?;
    let class = n
        .classes
        .iter()
        .position(|item| &item.id == class_name)
        .ok_or("unknown water class")?;
    let river = profile.river.ok_or("missing river speed")?;
    let lake = profile.lough.ok_or("missing lough speed")?;
    let sea = profile.sea.ok_or("missing sea speed")?;
    let width = usize::try_from(scenario.map.data.crop.width).map_err(|_| "width too large")?;
    let height = usize::try_from(scenario.map.data.crop.height).map_err(|_| "height too large")?;
    let offset = u64::try_from(n.joints.len()).map_err(|_| "too many joints")?;
    let cell_id = |cell: [i32; 2]| -> Result<u64, String> {
        let x = usize::try_from(cell[0]).map_err(|_| "negative cell column")?;
        let y = usize::try_from(cell[1]).map_err(|_| "negative cell row")?;
        if x >= width || y >= height {
            return Err("cell outside map".into());
        }
        offset
            .checked_add(u64::try_from(y * width + x).map_err(|_| "cell index too large")?)
            .ok_or_else(|| "node ID overflow".into())
    };
    let speed = |cell: [i32; 2]| -> Result<u64, String> {
        let x = usize::try_from(cell[0]).map_err(|_| "negative cell column")?;
        let y = usize::try_from(cell[1]).map_err(|_| "negative cell row")?;
        match scenario.map.data.cells.get(y * width + x) {
            Some(ck_map::Surface::Lake) => Ok(lake),
            Some(ck_map::Surface::Sea { .. }) => Ok(sea),
            _ => Err("water move enters land".into()),
        }
    };
    let mut graph = WaterGraph {
        edges: Vec::new(),
        steps: Arc::new(Vec::new()),
    };
    let mut push =
        |from: u64, to: u64, hours: BigRational, step: WaterStep| -> Result<(), String> {
            let id = u64::try_from(graph.steps.len()).map_err(|_| "too many water steps")?;
            graph.edges.push(Edge {
                from,
                to,
                id,
                hours,
            });
            Arc::get_mut(&mut graph.steps)
                .expect("water graph is not shared while building")
                .push(step);
            Ok(())
        };
    for reach in n.reaches.iter().filter(|reach| reach.usable) {
        let speed = match reach.kind {
            ck_map::ReachKind::River => river,
            ck_map::ReachKind::Lough => lake,
            ck_map::ReachKind::Tidal => sea,
        };
        let hours = water_travel_hours(
            &BigRational::from_integer(reach.length_mm.into()),
            speed,
            profile.slowing,
        );
        let a = u64::try_from(reach.from).map_err(|_| "joint ID too large")?;
        let b = u64::try_from(reach.to).map_err(|_| "joint ID too large")?;
        push(
            a,
            b,
            hours.clone(),
            WaterStep::Reach {
                reach: reach.id,
                downstream: true,
            },
        )?;
        push(
            b,
            a,
            hours,
            WaterStep::Reach {
                reach: reach.id,
                downstream: false,
            },
        )?;
    }
    for join in &n.water_joins {
        if !join.classes.contains(class_name) {
            continue;
        }
        let first = &n.reaches[join.reach];
        if let Some(other) = join.to_reach {
            let from = first.to;
            let to = n.reaches[other].from;
            let zero = BigRational::from_integer(0.into());
            push(
                u64::try_from(from).map_err(|_| "joint too large")?,
                u64::try_from(to).map_err(|_| "joint too large")?,
                zero.clone(),
                WaterStep::ReachJoin { from, to },
            )?;
            push(
                u64::try_from(to).map_err(|_| "joint too large")?,
                u64::try_from(from).map_err(|_| "joint too large")?,
                zero,
                WaterStep::ReachJoin { from: to, to: from },
            )?;
        } else if let Some(cell) = join.cell {
            let joint = first.from;
            let row = usize::try_from(cell[1]).map_err(|_| "negative row")?;
            let length = half_cell_mm(n.cell_side_mm[row]);
            let hours = water_travel_hours(&length, speed(cell)?, profile.slowing);
            let j = u64::try_from(joint).map_err(|_| "joint too large")?;
            let c = cell_id(cell)?;
            push(
                j,
                c,
                hours.clone(),
                WaterStep::CellJoin {
                    joint,
                    cell,
                    to_cell: true,
                },
            )?;
            push(
                c,
                j,
                hours,
                WaterStep::CellJoin {
                    joint,
                    cell,
                    to_cell: false,
                },
            )?;
        }
    }
    let mut open_hours: std::collections::HashMap<(u64, u64, bool, u64, u64), BigRational> =
        std::collections::HashMap::new();
    for y in 0..height {
        for x in 0..width {
            let cell = [
                i32::try_from(x).map_err(|_| "column too large")?,
                i32::try_from(y).map_err(|_| "row too large")?,
            ];
            for next in n.water().moves(class, cell) {
                let from_speed = speed(cell)?;
                let to_speed = speed(next)?;
                let row = usize::try_from(next[1]).map_err(|_| "negative row")?;
                let diagonal = cell[0] != next[0] && cell[1] != next[1];
                // The hours of a move depend only on the sides of the two cells' rows, whether it is diagonal and the two speeds, of
                // which there are few: each is worked once (CK-455).
                let hours = open_hours
                    .entry((
                        n.cell_side_mm[y],
                        n.cell_side_mm[row],
                        diagonal,
                        from_speed,
                        to_speed,
                    ))
                    .or_insert_with(|| {
                        let length = cell_step_mm(n.cell_side_mm[y], n.cell_side_mm[row], diagonal);
                        ck_sim::travel::mixed_water_hours(
                            &length,
                            from_speed,
                            to_speed,
                            profile.slowing,
                        )
                    })
                    .clone();
                push(
                    cell_id(cell)?,
                    cell_id(next)?,
                    hours,
                    WaterStep::Open {
                        from: cell,
                        to: next,
                    },
                )?;
            }
        }
    }
    Ok(graph)
}

/// Precompute land geometry without reading the changing way state.
/// Measure each land stretch in both directions, once after loading its network.
///
/// # Errors
/// Returns a message if a cell, terrain, length or stretch is inconsistent.
pub fn measure_ways(
    scenario: &ck_map::Scenario,
    rules: &TravelRules,
) -> Result<Vec<LandWay>, String> {
    let network = scenario.network.as_ref().ok_or("scenario has no network")?;
    let map = &scenario.map;
    let width = usize::try_from(map.data.crop.width).map_err(|_| "crop width is too large")?;
    let mut result = Vec::new();
    for way in &network.stretches {
        let (forward, backward) = if way.tidal {
            let empty = LandMeasure::from_steps(&[], rules.climb_thousandths);
            (empty.clone(), empty)
        } else {
            let mut cells = Vec::with_capacity(way.cells.len());
            for [x, y] in &way.cells {
                let row = usize::try_from(*y).map_err(|_| "negative row")?;
                let column = usize::try_from(*x).map_err(|_| "negative column")?;
                let index = row
                    .checked_mul(width)
                    .and_then(|n| n.checked_add(column))
                    .ok_or("cell index overflow")?;
                let terrain_index =
                    usize::from(map.grid_a.pixels.get(index).ok_or("cell outside map")?[0]);
                let terrain = map
                    .terrain_types
                    .get(terrain_index)
                    .ok_or("terrain index outside map")?;
                let ground = *rules
                    .ground
                    .get(&terrain.name)
                    .ok_or_else(|| format!("missing travel ground for {}", terrain.name))?;
                let elevation_m = i32::from(
                    *map.data
                        .elevations
                        .get(index)
                        .ok_or("elevation outside map")?,
                ) - 11_000;
                let side_mm = *network.cell_side_mm.get(row).ok_or("missing row side")?;
                cells.push(LandCell {
                    x: *x,
                    y: *y,
                    side_mm,
                    elevation_m,
                    ground,
                });
            }
            (
                measure_course(&cells, false, rules.climb_thousandths)
                    .ok_or("unsupported land step")?,
                measure_course(&cells, true, rules.climb_thousandths)
                    .ok_or("unsupported land step")?,
            )
        };
        let land_passable = way.crossings.as_ref().is_none_or(|crossings| {
            crossings
                .iter()
                .all(|crossing| crossing.kind == ck_map::LandCrossingKind::Ford)
        });
        result.push(LandWay {
            id: way.id,
            from: way.from,
            to: way.to,
            forward,
            backward,
            wades: way.wades.len(),
            fords: way.crossings.as_ref().map_or_else(Vec::new, |crossings| {
                crossings
                    .iter()
                    .map(|crossing| crossing.width_tenths_m)
                    .collect()
            }),
            land_passable,
            tidal_length_m: way.tidal.then_some(way.length_m),
        });
    }
    result.sort_by_key(|way| way.id);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    // Each Ireland search builds a large transient routing graph. Keep these tests
    // from multiplying that peak when the application suite runs in parallel.
    static IRELAND_SEARCH: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn fixture_network_measures_both_directions_from_loaded_data() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let scenario =
            ck_map::load_scenario(root.join("crates/map/tests/network/scenarios/fixture")).unwrap();
        let rules = TravelRules::parse(
            &std::fs::read_to_string(root.join("assets/game/travel.toml")).unwrap(),
        )
        .unwrap();
        let ways = measure_ways(&scenario, &rules).unwrap();
        assert_eq!(
            ways.len(),
            scenario.network.as_ref().unwrap().stretches.len()
        );
        assert!(ways.windows(2).all(|pair| pair[0].id < pair[1].id));
        let states =
            current_way_states(&super::super::stretch::starting(scenario.network.as_ref()));
        let network = scenario.network.as_ref().unwrap();
        let first = network
            .nodes
            .iter()
            .find(|node| node.place.is_some())
            .unwrap()
            .id;
        let answer = routes_from(
            &scenario,
            &ways,
            &states,
            &rules.profiles["cart"],
            &rules,
            first,
        )
        .unwrap();
        assert_eq!(
            answer.to(first).unwrap().hours,
            BigRational::from_integer(0.into())
        );
        let mut planner = TravelPlanner::new(&scenario, &rules.profiles["cart"], &rules).unwrap();
        let one = planner
            .ways
            .iter()
            .find(|way| way.land_passable && way.tidal_length_m.is_none())
            .unwrap()
            .clone();
        planner.ways = Arc::new(vec![one.clone()]);
        let mut current = states;
        let state = current.get_mut(&one.id).unwrap();
        state.road = true;
        state.condition = 1000;
        let good = planner
            .routes_from(&scenario, &current, one.from)
            .unwrap()
            .to(one.to)
            .unwrap()
            .hours
            .clone();
        current.get_mut(&one.id).unwrap().condition = 0;
        let ruined = planner
            .routes_from(&scenario, &current, one.from)
            .unwrap()
            .to(one.to)
            .unwrap()
            .hours
            .clone();
        current.get_mut(&one.id).unwrap().road = false;
        let path = planner
            .routes_from(&scenario, &current, one.from)
            .unwrap()
            .to(one.to)
            .unwrap()
            .hours
            .clone();
        assert!(good > BigRational::from_integer(0.into()));
        assert_eq!(ruined, path);
        current.get_mut(&one.id).unwrap().road = true;
        current.get_mut(&one.id).unwrap().condition = 1000;
        assert_eq!(
            planner
                .routes_from(&scenario, &current, one.from)
                .unwrap()
                .to(one.to)
                .unwrap()
                .hours,
            good
        );
    }

    #[test]
    fn crossing_fixture_counts_shared_stretches_and_refuses_unforded_crossings() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut scenario =
            ck_map::load_scenario(root.join("crates/map/tests/network/scenarios/fixture")).unwrap();
        let crossings: ck_map::LandNetwork = toml::from_str(
            &std::fs::read_to_string(
                root.join("crates/map/tests/network/scenarios/fixture/crossings-roads.toml"),
            )
            .unwrap(),
        )
        .unwrap();
        let network = scenario.network.as_mut().unwrap();
        network.nodes = crossings.nodes;
        network.stretches = crossings.stretches;
        let rules = TravelRules::parse(
            &std::fs::read_to_string(root.join("assets/game/travel.toml")).unwrap(),
        )
        .unwrap();
        let ways = measure_ways(&scenario, &rules).unwrap();
        let states =
            current_way_states(&super::super::stretch::starting(scenario.network.as_ref()));
        let cart = routes_from(
            &scenario,
            &ways,
            &states,
            &rules.profiles["cart"],
            &rules,
            1,
        )
        .unwrap();
        let trip = cart.itinerary(&scenario, &ways, 2, &rules).unwrap();
        let ids: Vec<_> = trip
            .iter()
            .filter_map(|leg| match leg.kind {
                TripKind::Land { way, .. } => Some(ways[way].id),
                _ => None,
            })
            .collect();
        assert_eq!(
            ids.iter().filter(|id| **id == 5).count(),
            1,
            "shared trunk is travelled once"
        );
        assert!(ids.contains(&4), "both ford delays belong to the crossing");
        let leg = trip
            .iter()
            .find(|leg| matches!(leg.kind, TripKind::Land { way, .. } if ways[way].id == 5))
            .unwrap();
        let zero = BigRational::from_integer(0.into());
        let half = BigRational::new(1.into(), 2.into());
        let start = cart.position(&scenario, &ways, leg, &zero).unwrap();
        let end = cart
            .position(&scenario, &ways, leg, &leg.length_mm)
            .unwrap();
        assert_eq!(
            cart.position(&scenario, &ways, leg, &(&leg.length_mm * &half))
                .unwrap(),
            ck_sim::travel::interpolate(&start, &end, &half)
        );

        let blocked = routes_from(
            &scenario,
            &ways,
            &states,
            &rules.profiles["cart"],
            &rules,
            4,
        )
        .unwrap();
        assert!(
            blocked.to(13).is_none(),
            "an unforded crossing is not a land edge"
        );
    }

    #[test]
    fn water_graph_keeps_reaches_and_zero_cost_joins_as_distinct_steps() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let scenario =
            ck_map::load_scenario(root.join("crates/map/tests/network/scenarios/fixture")).unwrap();
        let rules = TravelRules::parse(
            &std::fs::read_to_string(root.join("assets/game/travel.toml")).unwrap(),
        )
        .unwrap();
        let graph = water_graph(&scenario, &rules.profiles["currach"]).unwrap();
        let n = scenario.network.as_ref().unwrap();
        assert_eq!(
            graph
                .steps
                .iter()
                .filter(|step| matches!(step, WaterStep::Reach { .. }))
                .count(),
            n.reaches.iter().filter(|r| r.usable).count() * 2
        );
        assert_eq!(
            graph
                .steps
                .iter()
                .filter(|step| matches!(step, WaterStep::ReachJoin { .. }))
                .count(),
            n.water_joins
                .iter()
                .filter(|join| join.to_reach.is_some())
                .count()
                * 2
        );
        let step = graph
            .steps
            .iter()
            .find(|step| matches!(step, WaterStep::Open { .. }))
            .unwrap();
        let width = usize::try_from(scenario.map.data.crop.width).unwrap();
        let surface = |cell: [i32; 2]| {
            scenario.map.data.cells
                [usize::try_from(cell[1]).unwrap() * width + usize::try_from(cell[0]).unwrap()]
        };
        let (edge, from, to) = graph
            .edges
            .iter()
            .zip(graph.steps.iter())
            .find_map(|(edge, step)| match step {
                WaterStep::Open { from, to }
                    if matches!(
                        (surface(*from), surface(*to)),
                        (ck_map::Surface::Lake, ck_map::Surface::Sea { .. })
                    ) =>
                {
                    Some((edge, *from, *to))
                }
                _ => None,
            })
            .unwrap();
        let sides = &n.cell_side_mm;
        let length = cell_step_mm(
            sides[usize::try_from(from[1]).unwrap()],
            sides[usize::try_from(to[1]).unwrap()],
            from[0] != to[0] && from[1] != to[1],
        );
        assert_eq!(
            edge.hours,
            ck_sim::travel::mixed_water_hours(&length, 1700, 2000, 1000)
        );
        let zero = BigRational::from_integer(0.into());
        let one = BigRational::from_integer(1.into());
        let half = BigRational::new(1.into(), 2.into());
        let start = step.position(&scenario, &zero).unwrap();
        let end = step.position(&scenario, &one).unwrap();
        assert_eq!(
            step.position(&scenario, &half).unwrap(),
            ck_sim::travel::interpolate(&start, &end, &half)
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "The shared Ireland graph verifies joins, moves, and positions together."
    )]
    fn ireland_water_graph_contains_reaches_and_legal_open_moves() {
        let _search = IRELAND_SEARCH.lock().unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let scenario = ck_map::load_scenario(root.join("assets/scenarios/ireland-768")).unwrap();
        let rules = TravelRules::parse(
            &std::fs::read_to_string(root.join("assets/game/travel.toml")).unwrap(),
        )
        .unwrap();
        let graph = water_graph(&scenario, &rules.profiles["currach"]).unwrap();
        assert!(
            graph
                .steps
                .iter()
                .any(|step| matches!(step, WaterStep::Reach { .. }))
        );
        assert!(
            graph
                .steps
                .iter()
                .any(|step| matches!(step, WaterStep::Open { .. }))
        );
        let first = graph.edges.first().unwrap();
        let paths = ck_sim::travel::water_search(first.from, &graph.edges);
        assert!(paths.path(first.to).is_some());
        let n = scenario.network.as_ref().unwrap();
        let class = n
            .classes
            .iter()
            .position(|class| class.id == "currach")
            .unwrap();
        let open: std::collections::HashSet<_> = graph
            .steps
            .iter()
            .filter_map(|step| match step {
                WaterStep::Open { from, to } => Some((*from, *to)),
                _ => None,
            })
            .collect();
        let mut allowed = false;
        let mut refused = false;
        for y in 0..i32::try_from(scenario.map.data.crop.height).unwrap() - 1 {
            for x in 0..i32::try_from(scenario.map.data.crop.width).unwrap() - 1 {
                let a = [x, y];
                let b = [x + 1, y + 1];
                if !n.water().cell_usable(class, a) || !n.water().cell_usable(class, b) {
                    continue;
                }
                let legal = n.water().cell_usable(class, [x + 1, y])
                    && n.water().cell_usable(class, [x, y + 1]);
                let present = open.contains(&(a, b));
                assert_eq!(present, legal);
                allowed |= legal;
                refused |= !legal;
            }
        }
        assert!(allowed && refused, "Ireland exercises both corner cases");
        let join = n
            .water_joins
            .iter()
            .find(|join| join.to_reach.is_some())
            .unwrap();
        let upper = &n.reaches[join.reach];
        let lower = &n.reaches[join.to_reach.unwrap()];
        let selected: Vec<_> = graph
            .edges
            .iter()
            .zip(graph.steps.iter())
            .filter_map(|(edge, step)| match step {
                WaterStep::Reach { reach, .. } if *reach == upper.id || *reach == lower.id => {
                    Some(edge.clone())
                }
                WaterStep::ReachJoin { from, to }
                    if (*from == upper.to && *to == lower.from)
                        || (*from == lower.from && *to == upper.to) =>
                {
                    Some(edge.clone())
                }
                _ => None,
            })
            .collect();
        let upper_start = u64::try_from(upper.from).unwrap();
        let lower_start = u64::try_from(lower.from).unwrap();
        let lower_end = u64::try_from(lower.to).unwrap();
        let down = ck_sim::travel::water_search(upper_start, &selected)
            .path(lower_end)
            .unwrap();
        assert_eq!(down.legs.len(), 3, "two reaches and their zero-length join");
        let upper_time = &selected
            .iter()
            .find(|edge| edge.from == upper_start && edge.to == u64::try_from(upper.to).unwrap())
            .unwrap()
            .hours;
        let lower_time = &selected
            .iter()
            .find(|edge| edge.from == lower_start && edge.to == lower_end)
            .unwrap()
            .hours;
        assert_eq!(down.hours, upper_time + lower_time);
        let reverse = ck_sim::travel::water_search(lower_end, &selected)
            .path(lower_start)
            .unwrap();
        assert_eq!(
            reverse.legs.len(),
            1,
            "the shortest way does not loop through the join"
        );
        let (joint, cell) = graph
            .steps
            .iter()
            .find_map(|step| match step {
                WaterStep::CellJoin {
                    joint,
                    cell,
                    to_cell: true,
                } if joint_position(&scenario, *joint)
                    .is_some_and(|point| point != centre(&scenario, *cell)) =>
                {
                    Some((*joint, *cell))
                }
                _ => None,
            })
            .unwrap();
        let forward = WaterStep::CellJoin {
            joint,
            cell,
            to_cell: true,
        };
        let reverse = WaterStep::CellJoin {
            joint,
            cell,
            to_cell: false,
        };
        let zero = BigRational::from_integer(0.into());
        let one = BigRational::from_integer(1.into());
        let half = BigRational::new(1.into(), 2.into());
        let joint_point = joint_position(&scenario, joint).unwrap();
        let cell_point = centre(&scenario, cell);
        assert_eq!(forward.position(&scenario, &zero).unwrap(), joint_point);
        assert_eq!(forward.position(&scenario, &one).unwrap(), cell_point);
        assert_eq!(reverse.position(&scenario, &zero).unwrap(), cell_point);
        assert_eq!(reverse.position(&scenario, &one).unwrap(), joint_point);
        assert_eq!(
            forward.position(&scenario, &half),
            reverse.position(&scenario, &half)
        );
    }

    #[test]
    fn ireland_currach_reaches_killeany_with_water_and_transfers() {
        let _search = IRELAND_SEARCH.lock().unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let scenario = ck_map::load_scenario(root.join("assets/scenarios/ireland-768")).unwrap();
        let rules = TravelRules::parse(
            &std::fs::read_to_string(root.join("assets/game/travel.toml")).unwrap(),
        )
        .unwrap();
        let ways = measure_ways(&scenario, &rules).unwrap();
        let states =
            current_way_states(&super::super::stretch::starting(scenario.network.as_ref()));
        let routes = routes_from(
            &scenario,
            &ways,
            &states,
            &rules.profiles["currach"],
            &rules,
            136,
        )
        .unwrap();
        let trip = routes.itinerary(&scenario, &ways, 87, &rules).unwrap();
        assert!(
            trip.iter()
                .any(|leg| matches!(leg.kind, TripKind::Land { .. }))
        );
        assert!(
            trip.iter()
                .any(|leg| matches!(leg.kind, TripKind::Water { .. }))
        );
        assert!(
            trip.iter()
                .filter(|leg| matches!(leg.kind, TripKind::Transfer { .. }))
                .count()
                >= 2
        );
        let sum: BigRational = trip.iter().map(|leg| &leg.hours).sum();
        assert_eq!(sum, routes.to(87).unwrap().hours);
        for leg in &trip {
            let zero = BigRational::from_integer(0.into());
            let half = &leg.length_mm / num_bigint::BigInt::from(2);
            assert!(routes.position(&scenario, &ways, leg, &zero).is_some());
            assert!(routes.position(&scenario, &ways, leg, &half).is_some());
            assert!(
                routes
                    .position(&scenario, &ways, leg, &leg.length_mm)
                    .is_some()
            );
        }
        let n = scenario.network.as_ref().unwrap();
        for node in n
            .nodes
            .iter()
            .filter(|node| node.kind == ck_map::LandNodeKind::Place)
        {
            assert!(
                routes.to(node.id).is_some(),
                "{}",
                node.place.as_deref().unwrap()
            );
        }
        let held_shore = n.place_node("church_island_currane").unwrap().id;
        assert!(
            routes
                .itinerary(&scenario, &ways, held_shore, &rules)
                .unwrap()
                .iter()
                .any(|leg| matches!(leg.kind, TripKind::Strait { .. }))
        );
        assert!(
            routes
                .to(n.place_node("inishmacsaint").unwrap().id)
                .is_some()
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "One crossing is reused to compare both banks, a refused bank, and disconnected waters."
    )]
    fn an_unforded_river_needs_both_landings_and_two_transfers() {
        let _search = IRELAND_SEARCH.lock().unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut scenario =
            ck_map::load_scenario(root.join("assets/scenarios/ireland-768")).unwrap();
        let rules = TravelRules::parse(
            &std::fs::read_to_string(root.join("assets/game/travel.toml")).unwrap(),
        )
        .unwrap();
        let network = scenario.network.as_mut().unwrap();
        // Ireland has a real crossing whose banks contact the same river joint.
        // Treat its ford as absent to exercise an otherwise identical unforded crossing.
        let crossing = network
            .stretches
            .iter()
            .find_map(|stretch| {
                let ends: Vec<_> = network
                    .landings
                    .iter()
                    .filter(|landing| landing.crossing == Some(stretch.id))
                    .cloned()
                    .collect();
                (ends.len() == 2 && ends[0].joint.is_some() && ends[0].joint == ends[1].joint)
                    .then_some((stretch.id, stretch.from, stretch.to, ends))
            })
            .unwrap();
        let class = network
            .classes
            .iter()
            .position(|class| class.id == "currach")
            .unwrap();
        let first_body = crossing.3[0].bodies[class];
        let mut other_river = network
            .landings
            .iter()
            .find(|landing| {
                landing.joint.is_some()
                    && landing.bodies[class] >= 0
                    && landing.bodies[class] != first_body
            })
            .unwrap()
            .clone();
        other_river.node = crossing.2;
        other_river.approach_m = 0;
        network.landings = crossing.3;
        network.straits.clear();
        let mut ways: Vec<_> = measure_ways(&scenario, &rules)
            .unwrap()
            .into_iter()
            .filter(|way| way.id == crossing.0)
            .collect();
        ways[0].land_passable = false;
        let states =
            current_way_states(&super::super::stretch::starting(scenario.network.as_ref()));
        let cart = routes_from(
            &scenario,
            &ways,
            &states,
            &rules.profiles["cart"],
            &rules,
            crossing.1,
        )
        .unwrap();
        assert!(cart.to(crossing.2).is_none());
        let boat = routes_from(
            &scenario,
            &ways,
            &states,
            &rules.profiles["currach"],
            &rules,
            crossing.1,
        )
        .unwrap();
        let trip = boat
            .itinerary(&scenario, &ways, crossing.2, &rules)
            .unwrap();
        assert_eq!(
            trip.iter()
                .filter(|leg| matches!(leg.kind, TripKind::Transfer { .. }))
                .count(),
            2
        );
        assert_eq!(
            boat.to(crossing.2).unwrap().hours,
            BigRational::from_integer(4.into())
        );
        assert!(
            trip.iter()
                .filter(|leg| matches!(leg.kind, TripKind::Water { .. }))
                .all(|leg| leg.length_mm == BigRational::from_integer(0.into()))
        );
        scenario
            .network
            .as_mut()
            .unwrap()
            .landings
            .retain(|landing| landing.node != crossing.2);
        let refused = routes_from(
            &scenario,
            &ways,
            &states,
            &rules.profiles["currach"],
            &rules,
            crossing.1,
        )
        .unwrap();
        assert!(
            refused.to(crossing.2).is_none(),
            "a boat cannot cross with the far bank refused"
        );
        // Give the far bank a contact on a different, disconnected river.
        scenario
            .network
            .as_mut()
            .unwrap()
            .landings
            .push(other_river);
        let separate = routes_from(
            &scenario,
            &ways,
            &states,
            &rules.profiles["currach"],
            &rules,
            crossing.1,
        )
        .unwrap();
        assert!(
            separate.to(crossing.2).is_none(),
            "landings on separate waters do not connect them"
        );
    }

    #[test]
    fn ireland_cart_and_barge_cannot_reach_killeany() {
        let _search = IRELAND_SEARCH.lock().unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let scenario = ck_map::load_scenario(root.join("assets/scenarios/ireland-768")).unwrap();
        let rules = TravelRules::parse(
            &std::fs::read_to_string(root.join("assets/game/travel.toml")).unwrap(),
        )
        .unwrap();
        let ways = measure_ways(&scenario, &rules).unwrap();
        let states =
            current_way_states(&super::super::stretch::starting(scenario.network.as_ref()));
        let n = scenario.network.as_ref().unwrap();
        let held_shore = n.place_node("church_island_currane").unwrap().id;
        for profile in ["cart", "barge"] {
            let answer = routes_from(
                &scenario,
                &ways,
                &states,
                &rules.profiles[profile],
                &rules,
                136,
            )
            .unwrap();
            assert!(answer.to(87).is_none(), "{profile} must not reach Killeany");
            if profile == "cart" {
                assert!(answer.to(held_shore).is_none());
                assert!(
                    answer
                        .to(n.place_node("inishmacsaint").unwrap().id)
                        .is_none()
                );
                let mut reachable = std::collections::BTreeSet::from([136]);
                loop {
                    let before = reachable.len();
                    for way in ways.iter().filter(|way| way.land_passable) {
                        if reachable.contains(&way.from) {
                            reachable.insert(way.to);
                        }
                        if reachable.contains(&way.to) {
                            reachable.insert(way.from);
                        }
                    }
                    if reachable.len() == before {
                        break;
                    }
                }
                for node in n
                    .nodes
                    .iter()
                    .filter(|node| node.kind == ck_map::LandNodeKind::Place)
                {
                    assert_eq!(
                        answer.to(node.id).is_some(),
                        reachable.contains(&node.id),
                        "{}",
                        node.place.as_deref().unwrap()
                    );
                }
            }
        }
    }

    #[test]
    fn command_prints_a_named_profile_and_place_pair() {
        let _search = IRELAND_SEARCH.lock().unwrap();
        assert_eq!(
            command(["cart".into(), "ardfert".into(), "ciarraige_seat".into()]).unwrap(),
            "total: 2 h\nland stretch 106 forward (wades 0: 0.000 h, fords 0: 0.000 h): 1146.427 m, 1.166 h\n"
        );
        let at_one = command([
            "cart".into(),
            "ardfert".into(),
            "--within".into(),
            "1".into(),
        ])
        .unwrap();
        let at_two = command([
            "cart".into(),
            "ardfert".into(),
            "--within".into(),
            "2".into(),
        ])
        .unwrap();
        assert!(
            !at_one
                .lines()
                .any(|line| line.starts_with("ciarraige_seat:"))
        );
        assert!(at_two.lines().any(|line| line == "ciarraige_seat: 2 h"));
    }

    #[test]
    fn ireland_slige_mor_crossing_has_two_boat_transfers() {
        let _search = IRELAND_SEARCH.lock().unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let scenario = ck_map::load_scenario(root.join("assets/scenarios/ireland-768")).unwrap();
        let rules = TravelRules::parse(
            &std::fs::read_to_string(root.join("assets/game/travel.toml")).unwrap(),
        )
        .unwrap();
        let ways = measure_ways(&scenario, &rules).unwrap();
        let states =
            current_way_states(&super::super::stretch::starting(scenario.network.as_ref()));
        for profile in ["currach", "barge", "cart"] {
            let routes = routes_from(
                &scenario,
                &ways,
                &states,
                &rules.profiles[profile],
                &rules,
                268,
            )
            .unwrap();
            let trip = routes.itinerary(&scenario, &ways, 269, &rules).unwrap();
            if profile == "cart" {
                assert!(!trip.iter().any(
                    |leg| matches!(leg.kind, TripKind::Land { way, .. } if ways[way].id == 260)
                ));
            } else {
                assert_eq!(
                    routes.to(269).unwrap().hours,
                    BigRational::from_integer(4.into())
                );
                assert_eq!(
                    trip.iter()
                        .filter(|leg| matches!(leg.kind, TripKind::Transfer { .. }))
                        .count(),
                    2
                );
                let water: Vec<_> = trip
                    .iter()
                    .filter(|leg| matches!(leg.kind, TripKind::Water { .. }))
                    .collect();
                assert_eq!(water.len(), 1);
                assert_eq!(water[0].length_mm, BigRational::from_integer(0.into()));
            }
        }
    }

    #[test]
    fn ireland_cart_detours_via_fords_between_clonmacnoise_and_clonfert() {
        let _search = IRELAND_SEARCH.lock().unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let scenario = ck_map::load_scenario(root.join("assets/scenarios/ireland-768")).unwrap();
        let rules = TravelRules::parse(
            &std::fs::read_to_string(root.join("assets/game/travel.toml")).unwrap(),
        )
        .unwrap();
        let ways = measure_ways(&scenario, &rules).unwrap();
        let states =
            current_way_states(&super::super::stretch::starting(scenario.network.as_ref()));
        let n = scenario.network.as_ref().unwrap();
        let start = n.place_node("clonmacnoise").unwrap().id;
        let end = n.place_node("clonfert").unwrap().id;
        let routes = routes_from(
            &scenario,
            &ways,
            &states,
            &rules.profiles["cart"],
            &rules,
            start,
        )
        .unwrap();
        let trip = routes.itinerary(&scenario, &ways, end, &rules).unwrap();
        assert!(trip.iter().any(
            |leg| matches!(leg.kind, TripKind::Land { way, .. } if !ways[way].fords.is_empty())
        ));
        let distance: BigRational = trip.iter().map(|leg| &leg.length_mm).sum();
        let unrestricted: Vec<Edge> = n
            .stretches
            .iter()
            .filter(|way| !way.tidal)
            .flat_map(|way| {
                [
                    Edge {
                        from: way.from,
                        to: way.to,
                        id: way.id * 2,
                        hours: BigRational::from_integer(
                            num_bigint::BigInt::from(way.length_m) * 1000,
                        ),
                    },
                    Edge {
                        from: way.to,
                        to: way.from,
                        id: way.id * 2 + 1,
                        hours: BigRational::from_integer(
                            num_bigint::BigInt::from(way.length_m) * 1000,
                        ),
                    },
                ]
            })
            .collect();
        let direct = &ck_sim::travel::shortest_paths(start, &unrestricted)[&end].hours;
        assert!(distance > *direct);
    }

    #[test]
    fn ireland_hilly_stretch_has_repeatable_directional_times() {
        let _search = IRELAND_SEARCH.lock().unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let scenario = ck_map::load_scenario(root.join("assets/scenarios/ireland-768")).unwrap();
        let rules = TravelRules::parse(
            &std::fs::read_to_string(root.join("assets/game/travel.toml")).unwrap(),
        )
        .unwrap();
        let mut planner = TravelPlanner::new(&scenario, &rules.profiles["cart"], &rules).unwrap();
        let speed = ck_sim::travel::land_speed(1000, 1400, None);
        let way = planner
            .ways
            .iter()
            .find(|way| {
                way.land_passable
                    && way.tidal_length_m.is_none()
                    && way.forward.hours(1, &speed) != way.backward.hours(1, &speed)
            })
            .unwrap()
            .clone();
        planner.ways = Arc::new(vec![way.clone()]);
        let states =
            current_way_states(&super::super::stretch::starting(scenario.network.as_ref()));
        let forward = planner
            .routes_from(&scenario, &states, way.from)
            .unwrap()
            .to(way.to)
            .unwrap()
            .hours
            .clone();
        let backward = planner
            .routes_from(&scenario, &states, way.to)
            .unwrap()
            .to(way.from)
            .unwrap()
            .hours
            .clone();
        assert_ne!(forward, backward);
        assert_eq!(
            planner
                .routes_from(&scenario, &states, way.from)
                .unwrap()
                .to(way.to)
                .unwrap()
                .hours,
            forward
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "The same fixture checks boat-only connections and transfer positions."
    )]
    fn boat_only_can_use_water_and_straits_but_cannot_portage() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let scenario =
            ck_map::load_scenario(root.join("crates/map/tests/network/scenarios/fixture")).unwrap();
        let rules = TravelRules::parse(
            &std::fs::read_to_string(root.join("assets/game/travel.toml")).unwrap(),
        )
        .unwrap();
        let ways = measure_ways(&scenario, &rules).unwrap();
        let states =
            current_way_states(&super::super::stretch::starting(scenario.network.as_ref()));
        let mut boat = rules.profiles["currach"].clone();
        boat.path = None;
        boat.road = None;
        let routes = routes_from(&scenario, &ways, &states, &boat, &rules, 1).unwrap();
        assert!(routes.to(5).is_some(), "same water reaches e");
        let water_only = routes_from(&scenario, &[], &states, &boat, &rules, 1).unwrap();
        assert!(
            water_only.to(2).is_none(),
            "the boat-only party cannot walk an inland approach"
        );
        let carrying_boat = routes_from(
            &scenario,
            &[],
            &states,
            &rules.profiles["currach"],
            &rules,
            1,
        )
        .unwrap();
        let inland = carrying_boat.itinerary(&scenario, &[], 2, &rules).unwrap();
        assert!(
            inland.iter().any(|leg| matches!(
                leg.kind,
                TripKind::Transfer {
                    landing: Some(_),
                    ..
                }
            ) && leg.length_mm > BigRational::from_integer(0.into())),
            "the party with land speed pays the landing approach"
        );

        assert!(
            routes.to(4).is_none(),
            "two waters at a node do not permit portage"
        );
        let strait = routes_from(&scenario, &ways, &states, &boat, &rules, 6).unwrap();
        assert!(
            strait.to(7).is_some(),
            "a boat can cross a strait without land travel"
        );
        assert_eq!(
            strait.to(7).unwrap().legs.len(),
            3,
            "the strait has a transfer at either end"
        );
        let itinerary = strait.itinerary(&scenario, &ways, 7, &rules).unwrap();
        assert_eq!(itinerary.len(), 3);
        assert!(matches!(itinerary[1].kind, TripKind::Strait { .. }));
        assert_eq!(
            itinerary.iter().map(|leg| &leg.hours).sum::<BigRational>(),
            strait.to(7).unwrap().hours
        );
        assert_eq!(
            format_itinerary(&scenario, &ways, &strait, 7, &rules),
            "total: 5 h\nstrait transfer: 0.000 m, 2.000 h\nstrait isle forward: 500.000 m, 0.250 h\nstrait transfer: 0.000 m, 2.000 h\n"
        );
        let leg = &itinerary[1];
        let zero = BigRational::from_integer(0.into());
        let start = strait.position(&scenario, &ways, leg, &zero).unwrap();
        let end = strait
            .position(&scenario, &ways, leg, &leg.length_mm)
            .unwrap();
        let half = BigRational::new(1.into(), 2.into());
        assert_eq!(
            strait
                .position(&scenario, &ways, leg, &(&leg.length_mm * &half))
                .unwrap(),
            ck_sim::travel::interpolate(&start, &end, &half)
        );
        let n = scenario.network.as_ref().unwrap();
        let (index, landing) = n
            .landings
            .iter()
            .enumerate()
            .find(|(_, landing)| landing.approach_m > 0)
            .unwrap();
        let length = BigRational::from_integer(num_bigint::BigInt::from(landing.approach_m) * 1000);
        let mut approach = TripLeg {
            kind: TripKind::Transfer {
                landing: Some(index),
                node: landing.node,
                boarding: true,
            },
            length_mm: length.clone(),
            hours: zero.clone(),
        };
        let start = strait.position(&scenario, &ways, &approach, &zero).unwrap();
        let end = strait
            .position(&scenario, &ways, &approach, &length)
            .unwrap();
        assert_eq!(end, centre(&scenario, landing.shore));
        assert_eq!(
            strait
                .position(&scenario, &ways, &approach, &(&length * &half))
                .unwrap(),
            ck_sim::travel::interpolate(&start, &end, &half)
        );
        approach.kind = TripKind::Transfer {
            landing: Some(index),
            node: landing.node,
            boarding: false,
        };
        assert_eq!(
            strait.position(&scenario, &ways, &approach, &zero).unwrap(),
            end
        );
        assert_eq!(
            strait
                .position(&scenario, &ways, &approach, &length)
                .unwrap(),
            start
        );
        let mut joined = scenario.clone();
        let landing = joined
            .network
            .as_mut()
            .unwrap()
            .landings
            .iter_mut()
            .find(|landing| landing.node == 7)
            .unwrap();
        landing.approach_m = 0;
        let crossing = routes_from(&joined, &ways, &states, &boat, &rules, 1).unwrap();
        let trip = crossing.itinerary(&joined, &ways, 6, &rules).unwrap();
        assert!(
            trip.iter()
                .any(|leg| matches!(leg.kind, TripKind::Water { .. }))
        );
        assert!(
            trip.iter()
                .any(|leg| matches!(leg.kind, TripKind::Strait { .. }))
        );
        assert_eq!(
            trip.iter()
                .filter(|leg| matches!(leg.kind, TripKind::Transfer { .. }))
                .count(),
            4
        );
    }

    /// Two ways home of equal time and equal length whose legs are [1, 9] and [2, 3] in the order they are travelled: the reverse search
    /// into home chooses, for the place they start from, what the forward search from that place chooses (a search that compared the
    /// legs from home back would take the second).
    #[test]
    fn a_tied_way_home_is_the_one_the_forward_search_from_the_place_chooses() {
        let edge = |from, to, id, leg| ck_sim::travel::ItineraryEdge {
            from,
            to,
            id,
            hours: BigRational::from_integer(1.into()),
            leg_ids: vec![leg],
            water_step: false,
        };
        let edges = [
            edge(0, 1, 0, 1),
            edge(1, 3, 1, 9),
            edge(0, 2, 2, 2),
            edge(2, 3, 3, 3),
        ];
        let node_ids: BTreeMap<u64, u64> = (0..4).map(|node| (node + 100, node)).collect();
        let transitions = (0..4)
            .map(|_| Transition::Strait {
                index: 0,
                backward: false,
            })
            .collect();
        let reverse = reverse_routes(
            3,
            103,
            &edges,
            transitions,
            vec![BigRational::from_integer(1.into()); 4],
            Arc::new(Vec::new()),
            Vec::new(),
            50,
            &node_ids,
        )
        .unwrap();
        let forward = ck_sim::travel::itinerary_search(0, &edges).path(3).unwrap();
        let chosen = reverse.to(100).expect("a way home");
        assert_eq!(forward.legs, [1, 9]);
        assert_eq!(
            (&chosen.legs, &chosen.edges, &chosen.hours),
            (&forward.legs, &forward.edges, &forward.hours)
        );
    }
}
