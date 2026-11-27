//! Hourly vehicles and jobs for each pop's panel.
use super::expedition_panel::{self, VehicleView};
use ck_economy::{EconomyState, JobKind, Rules};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct JobView {
    pub label: String,
    pub state: &'static str,
    pub done: Option<u64>,
    pub lacks: Vec<(String, u64)>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PopTrade {
    pub place: String,
    pub pop: u32,
    pub vehicles: Vec<VehicleView>,
    pub jobs: Vec<JobView>,
}

pub(crate) fn build(state: &EconomyState, rules: &Rules) -> Vec<PopTrade> {
    let mut views = Vec::new();
    for (home, place) in state.places.iter().enumerate() {
        for (index, _) in place.pops.iter().enumerate() {
            let Ok(pop) = u32::try_from(index) else {
                continue;
            };
            let mut vehicles: Vec<_> = place
                .trade
                .vehicles
                .iter()
                .filter(|v| v.pop == pop)
                .filter_map(|v| expedition_panel::vehicle_base(place, v, rules))
                .collect();
            let mut jobs = Vec::new();
            for job in place
                .trade
                .jobs
                .iter()
                .filter(|job| job.pop == pop && job.given_up.is_none())
            {
                let label = match &job.kind {
                    JobKind::Build { vehicle, .. } => rules
                        .vehicles
                        .get(*vehicle as usize)
                        .map(|v| format!("building {}", v.id)),
                    JobKind::Trade { plan } => state
                        .places
                        .get(plan.destination as usize)
                        .map(|p| format!("trip to {}", p.id)),
                    JobKind::Held { .. } => None,
                };
                let Some(label) = label else { continue };
                let mut lacks: Vec<_> = rules
                    .goods
                    .iter()
                    .zip(&job.need_goods)
                    .zip(&job.stock)
                    .filter(|((_, need), held)| need > held)
                    .map(|((good, need), held)| (good.id.clone(), need - held))
                    .collect();
                lacks.extend(
                    rules
                        .herd_species
                        .iter()
                        .zip(&job.need_animals)
                        .zip(&job.animals)
                        .filter(|((_, need), held)| need > held)
                        .map(|((animal, need), held)| (animal.animal.clone(), need - held)),
                );
                jobs.push(JobView {
                    label,
                    state: if job.hands.is_some() {
                        "building"
                    } else {
                        "gathering"
                    },
                    done: job.hands.as_ref().map(|h| h.done),
                    lacks,
                });
            }
            for trip in place.trade.expeditions.iter().filter(|t| t.trader == pop) {
                if let Some(panel) = expedition_panel::build(state, home, trip, rules) {
                    for vehicle in panel.vehicles {
                        if let Some(existing) =
                            vehicles.iter_mut().find(|v| v.number == vehicle.number)
                        {
                            *existing = vehicle;
                        }
                    }
                    jobs.push(JobView {
                        label: format!("trip to {}", panel.destination),
                        state: panel.phase,
                        done: None,
                        lacks: Vec::new(),
                    });
                }
            }
            views.push(PopTrade {
                place: place.id.clone(),
                pop,
                vehicles,
                jobs,
            });
        }
    }
    views
}
